//! The session state machine (sections 4.2, 6, 7 and 8), free of I/O: the
//! main loop feeds it requests, heartbeats, answers and worker results with
//! the current time, and writes out what it produces (responses, the state
//! file, log lines, jobs for the worker).

use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::actions::{Action, CmdOutcome};
use crate::assets;
use crate::files::Answer;
use crate::http::{Method, ParseError, Request, Response};
use crate::mirror::Mirror;
use crate::secret::{Rng, ct_eq, is_id, random_b64, random_hex};
use crate::worker::{Job, JobDone};

/// Unauthenticated requests per session (section 6.4).
pub(crate) const MAX_BAD: u32 = 20;
/// Parse errors accepted before the token is used (owner default for open
/// question 2); after it is used the first parse error ends the session.
pub(crate) const MAX_PARSE_BEFORE_TOKEN: u32 = 3;
pub(crate) const MAX_REFUSALS: u32 = 3;
pub(crate) const MAX_REQUESTS: u32 = 2000;
const RATE_PER_S: usize = 20;
/// How long the latest request stays readable at `GET /api/actions/<id>`.
const KEEP_FINISHED: Duration = Duration::from_secs(60);
const COOKIE: &str = "sat_session";

#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    pub(crate) lifetime: Duration,
    pub(crate) idle: Duration,
    pub(crate) heartbeat_grace: Duration,
    /// Heartbeat age after which a new action is refused with 503.
    pub(crate) app_gone: Duration,
    pub(crate) request_timeout: Duration,
    pub(crate) cooldown: Duration,
    pub(crate) refresh: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            lifetime: Duration::from_secs(600),
            idle: Duration::from_secs(180),
            heartbeat_grace: Duration::from_secs(15),
            app_gone: Duration::from_secs(8),
            request_timeout: Duration::from_secs(60),
            cooldown: Duration::from_secs(3),
            refresh: Duration::from_secs(3),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EndReason {
    Done,
    Timeout,
    Idle,
    AppGone,
    ParseError,
    TooManyRefusals,
    TooManyBadRequests,
}

impl EndReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            EndReason::Done => "done",
            EndReason::Timeout => "timeout",
            EndReason::Idle => "idle",
            EndReason::AppGone => "app-gone",
            EndReason::ParseError => "parse-error",
            EndReason::TooManyRefusals => "too-many-refusals",
            EndReason::TooManyBadRequests => "too-many-bad-requests",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Allowed,
    Refused,
    Expired,
    Aborted,
    /// Ran without the OK on the TV (microphone off, owner decision).
    Auto,
}

impl Verdict {
    fn as_str(self) -> &'static str {
        match self {
            Verdict::Allowed => "allowed",
            Verdict::Refused => "refused",
            Verdict::Expired => "expired",
            Verdict::Aborted => "aborted",
            Verdict::Auto => "auto",
        }
    }
}

#[derive(Debug)]
struct Pending {
    id: String,
    action: Action,
    peer: Ipv4Addr,
    created: Instant,
    running: bool,
    confirm: bool,
}

#[derive(Debug)]
struct Finished {
    id: String,
    action: Action,
    peer: Ipv4Addr,
    verdict: Verdict,
    result: String,
    error: String,
    at: Instant,
}

#[derive(Debug)]
struct Session {
    peer: Ipv4Addr,
    other_subnet: bool,
    last_request: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthFail {
    NoCookie,
    Wrong,
    WrongPeer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route<'a> {
    Token(&'a str),
    Page,
    Asset(&'a str),
    ApiSession,
    ApiState,
    Actions,
    Action(&'a str),
    End,
    Unknown,
}

impl<'a> Route<'a> {
    fn of(path: &'a str) -> Self {
        match path {
            "/" => Route::Page,
            "/api/session" => Route::ApiSession,
            "/api/state" => Route::ApiState,
            "/api/actions" => Route::Actions,
            "/api/session/end" => Route::End,
            _ => {
                if let Some(t) = path.strip_prefix("/s/") {
                    Route::Token(t)
                } else if let Some(n) = path.strip_prefix("/assets/") {
                    Route::Asset(n)
                } else if let Some(id) = path.strip_prefix("/api/actions/") {
                    Route::Action(id)
                } else {
                    Route::Unknown
                }
            }
        }
    }

    /// Fixed names for the log (never the path).
    fn name(self) -> &'static str {
        match self {
            Route::Token(_) => "token",
            Route::Page => "page",
            Route::Asset(_) => "asset",
            Route::ApiSession => "api-session",
            Route::ApiState => "api-state",
            Route::Actions => "api-actions",
            Route::Action(_) => "api-action",
            Route::End => "api-session-end",
            Route::Unknown => "unknown",
        }
    }

    fn method(self) -> Method {
        match self {
            Route::Actions | Route::End => Method::Post,
            _ => Method::Get,
        }
    }
}

pub(crate) struct Core {
    t: Timing,
    rng: Box<dyn Rng>,
    started: Instant,
    port: u16,
    token: String,
    cookie: String,
    csrf: String,
    session: Option<Session>,
    bad: u32,
    parse_errors: u32,
    last_heartbeat: Instant,
    pending: Option<Pending>,
    finished: Option<Finished>,
    refusals: u32,
    cooldown_until: Option<Instant>,
    rate: [Option<Instant>; RATE_PER_S],
    rate_next: usize,
    total: u32,
    mirror: Mirror,
    refresh_due: bool,
    queued: Option<Job>,
    worker_busy: bool,
    end: Option<EndReason>,
    dirty: bool,
    log: Vec<String>,
}

impl Core {
    /// Draws the token, the cookie and the CSRF token at once (a failing
    /// random source fails the start, never a later request).
    pub(crate) fn new(
        t: Timing,
        mut rng: Box<dyn Rng>,
        port: u16,
        now: Instant,
    ) -> std::io::Result<Self> {
        let token = random_b64(rng.as_mut(), 32)?;
        let cookie = random_b64(rng.as_mut(), 32)?;
        let csrf = random_hex(rng.as_mut(), 16)?;
        Ok(Core {
            t,
            rng,
            started: now,
            port,
            token,
            cookie,
            csrf,
            session: None,
            bad: 0,
            parse_errors: 0,
            last_heartbeat: now,
            pending: None,
            finished: None,
            refusals: 0,
            cooldown_until: None,
            rate: [None; RATE_PER_S],
            rate_next: 0,
            total: 0,
            mirror: Mirror::default(),
            refresh_due: false,
            queued: None,
            worker_busy: false,
            end: None,
            dirty: true,
            log: vec![format!("session start port={port}")],
        })
    }

    pub(crate) fn ended(&self) -> Option<EndReason> {
        self.end
    }

    pub(crate) fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }

    /// True once after every change of the state file's content.
    pub(crate) fn take_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.dirty, false)
    }

    fn left(&self, now: Instant) -> u64 {
        (self.started + self.t.lifetime)
            .saturating_duration_since(now)
            .as_secs()
    }

    // --- end conditions -------------------------------------------------

    /// Ends the session (the first reason wins).
    pub(crate) fn end(&mut self, reason: EndReason, now: Instant) {
        if self.end.is_some() {
            return;
        }
        if let Some(p) = self.pending.take() {
            if p.running {
                let v = if p.confirm {
                    Verdict::Allowed
                } else {
                    Verdict::Auto
                };
                self.finish(
                    p,
                    v,
                    "",
                    "the session ended before the result was known",
                    now,
                );
            } else {
                self.finish(p, Verdict::Aborted, "", "", now);
            }
        }
        self.queued = None;
        self.end = Some(reason);
        self.dirty = true;
        self.log
            .push(format!("session end reason={}", reason.as_str()));
    }

    /// The time-driven checks (lifetime, heartbeat, idle, request expiry).
    pub(crate) fn tick(&mut self, now: Instant) {
        if self.end.is_some() {
            return;
        }
        if now.saturating_duration_since(self.started) >= self.t.lifetime {
            self.end(EndReason::Timeout, now);
        } else if now.saturating_duration_since(self.last_heartbeat) >= self.t.heartbeat_grace {
            self.end(EndReason::AppGone, now);
        } else if self
            .session
            .as_ref()
            .is_some_and(|s| now.saturating_duration_since(s.last_request) >= self.t.idle)
        {
            self.end(EndReason::Idle, now);
        } else if let Some(p) = &self.pending
            && p.confirm
            && !p.running
            && now.saturating_duration_since(p.created) >= self.t.request_timeout
        {
            if let Some(p) = self.pending.take() {
                self.log.push(format!("action expired id={}", p.id));
                self.finish(p, Verdict::Expired, "", "", now);
                self.strike(now);
            }
        }
    }

    pub(crate) fn on_heartbeat(&mut self, now: Instant) {
        self.last_heartbeat = now;
    }

    fn bad(&mut self, now: Instant) {
        self.bad += 1;
        if self.bad >= MAX_BAD {
            self.end(EndReason::TooManyBadRequests, now);
        }
    }

    fn strike(&mut self, now: Instant) {
        self.refusals += 1;
        self.cooldown_until = Some(now + self.t.cooldown);
        if self.refusals >= MAX_REFUSALS {
            self.end(EndReason::TooManyRefusals, now);
        }
    }

    // --- connections ------------------------------------------------------

    /// A connection from outside the private ranges, closed without a reply.
    pub(crate) fn on_rejected_peer(&mut self, peer: Ipv4Addr, now: Instant) {
        self.log.push(format!("rejected peer-filter peer={peer}"));
        self.bad(now);
    }

    pub(crate) fn on_parse_error(
        &mut self,
        peer: Ipv4Addr,
        e: ParseError,
        now: Instant,
    ) -> Response {
        self.log
            .push(format!("rejected parse={} peer={peer}", e.kind()));
        if self.session.is_some() {
            self.end(EndReason::ParseError, now);
        } else {
            self.parse_errors += 1;
            if self.parse_errors >= MAX_PARSE_BEFORE_TOKEN {
                self.end(EndReason::ParseError, now);
            }
        }
        self.bad(now);
        Response::error(e.status(), "bad_request", "The request was not understood.")
    }

    fn rate_limited(&mut self, now: Instant) -> bool {
        self.total = self.total.saturating_add(1);
        if self.total > MAX_REQUESTS {
            return true;
        }
        let oldest = self.rate[self.rate_next];
        if oldest.is_some_and(|t| now.saturating_duration_since(t) < Duration::from_secs(1)) {
            return true;
        }
        self.rate[self.rate_next] = Some(now);
        self.rate_next = (self.rate_next + 1) % RATE_PER_S;
        false
    }

    /// Answers one parsed request from `peer`, received on `local`.
    pub(crate) fn handle(
        &mut self,
        peer: Ipv4Addr,
        local: SocketAddrV4,
        req: &Request,
        now: Instant,
    ) -> Response {
        let r = self.route(peer, local, req, now);
        if req.method == Method::Head {
            r.without_body()
        } else {
            r
        }
    }

    fn route(
        &mut self,
        peer: Ipv4Addr,
        local: SocketAddrV4,
        req: &Request,
        now: Instant,
    ) -> Response {
        if self.end.is_some() {
            return Response::error(503, "ended", "The session has ended.");
        }
        if self.rate_limited(now) {
            self.log.push(format!("rejected rate peer={peer}"));
            return Response::error(429, "rate_limited", "Too many requests. Wait a moment.");
        }
        let host = local.to_string();
        if req.host != host {
            self.log.push(format!("rejected host peer={peer}"));
            self.bad(now);
            return Response::error(400, "bad_host", "Open the address shown on the TV.");
        }
        let cross_origin = req
            .origin
            .as_ref()
            .is_some_and(|o| *o != format!("http://{host}"))
            || req
                .sec_fetch_site
                .as_ref()
                .is_some_and(|s| s != "same-origin" && s != "none");
        if cross_origin {
            self.log.push(format!("rejected origin peer={peer}"));
            self.bad(now);
            return Response::error(
                403,
                "cross_origin",
                "Requests from other sites are refused.",
            );
        }
        let route = Route::of(&req.target);
        let auth = self.authenticate(peer, req);
        if let Route::Token(t) = route {
            return self.token(peer, local, req, t, auth, now);
        }
        if route == Route::Unknown || route.method() != req.method {
            if auth.is_err() {
                self.bad(now);
            }
            self.log.push(format!(
                "rejected {} {} peer={peer}",
                req.method.as_str(),
                route.name()
            ));
            if route == Route::Unknown {
                return Response::error(404, "not_found", "Not found.");
            }
            return Response::error(405, "method", "Method not allowed.")
                .header("Allow", route.method().as_str().to_owned());
        }
        if let Err(why) = auth {
            let word = match why {
                AuthFail::NoCookie => "no-session",
                AuthFail::Wrong => "wrong-session",
                AuthFail::WrongPeer => "wrong-peer",
            };
            self.log
                .push(format!("rejected {word} {} peer={peer}", route.name()));
            self.bad(now);
            return if route == Route::Page {
                Response::html(403, assets::NO_SESSION_PAGE)
            } else {
                Response::error(
                    403,
                    "no_session",
                    "No session. Scan the QR code on the TV again.",
                )
            };
        }
        if let Some(s) = self.session.as_mut() {
            s.last_request = now;
        }
        let resp = self.dispatch(route, peer, req, now);
        self.log.push(format!(
            "request {} {} status={} peer={peer}",
            req.method.as_str(),
            route.name(),
            resp.status
        ));
        resp
    }

    /// An authenticated request on a known route with the right method.
    fn dispatch(
        &mut self,
        route: Route<'_>,
        peer: Ipv4Addr,
        req: &Request,
        now: Instant,
    ) -> Response {
        match route {
            Route::Page => Response::html(200, assets::PAGE),
            Route::Asset(name) => match assets::find(name) {
                Some(a) => Response::asset(a.content_type, a.body),
                None => Response::error(404, "not_found", "Not found."),
            },
            Route::ApiSession => Response::json(
                200,
                &json!({
                    "schema": 1,
                    "csrf": self.csrf,
                    "expires_in_s": self.left(now),
                    "idle_timeout_s": self.t.idle.as_secs(),
                }),
            ),
            Route::ApiState => Response::json(
                200,
                &self.mirror.to_json(&self.pending_json(now, false), now),
            ),
            Route::Actions => self.post_action(peer, req, now),
            Route::Action(id) => self.action_state(id, now),
            Route::End => self.post_end(peer, req, now),
            Route::Token(_) | Route::Unknown => Response::error(404, "not_found", "Not found."),
        }
    }

    fn authenticate(&self, peer: Ipv4Addr, req: &Request) -> Result<(), AuthFail> {
        let s = self.session.as_ref().ok_or(AuthFail::NoCookie)?;
        let header = req.cookie.as_deref().ok_or(AuthFail::NoCookie)?;
        let mut found = None;
        for part in header.split(';') {
            if let Some(v) = part
                .trim_matches(' ')
                .strip_prefix(COOKIE)
                .and_then(|r| r.strip_prefix('='))
            {
                if found.is_some() {
                    return Err(AuthFail::Wrong);
                }
                found = Some(v);
            }
        }
        let v = found.ok_or(AuthFail::NoCookie)?;
        if !ct_eq(v.as_bytes(), self.cookie.as_bytes()) {
            return Err(AuthFail::Wrong);
        }
        if peer != s.peer {
            return Err(AuthFail::WrongPeer);
        }
        Ok(())
    }

    fn token(
        &mut self,
        peer: Ipv4Addr,
        local: SocketAddrV4,
        req: &Request,
        t: &str,
        auth: Result<(), AuthFail>,
        now: Instant,
    ) -> Response {
        if req.method != Method::Get {
            self.log.push(format!(
                "rejected {} token peer={peer}",
                req.method.as_str()
            ));
            self.bad(now);
            return Response::error(405, "method", "Method not allowed.")
                .header("Allow", "GET".to_owned());
        }
        if !ct_eq(t.as_bytes(), self.token.as_bytes()) {
            self.log.push(format!("rejected token peer={peer}"));
            self.bad(now);
            return Response::error(404, "not_found", "Not found.");
        }
        if self.session.is_none() {
            let other = peer.octets()[..3] != local.ip().octets()[..3];
            self.session = Some(Session {
                peer,
                other_subnet: other,
                last_request: now,
            });
            self.refresh_due = true;
            self.dirty = true;
            self.log.push(format!("token consumed peer={peer}"));
            return self.redirect(now, true);
        }
        if auth.is_ok() {
            // The same phone opened the link again (history, tab restore).
            if let Some(s) = self.session.as_mut() {
                s.last_request = now;
            }
            return self.redirect(now, false);
        }
        self.log.push(format!("rejected token-used peer={peer}"));
        self.bad(now);
        Response::html(410, assets::LINK_USED_PAGE)
    }

    fn redirect(&self, now: Instant, cookie: bool) -> Response {
        let r = Response::empty(302).header("Location", "/".to_owned());
        if !cookie {
            return r;
        }
        r.header(
            "Set-Cookie",
            format!(
                "{COOKIE}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
                self.cookie,
                self.left(now).max(1)
            ),
        )
    }

    /// CSRF header (403) and JSON content type (415), section 6.3.
    fn check_write(&mut self, peer: Ipv4Addr, req: &Request, now: Instant) -> Result<(), Response> {
        let ok = req
            .csrf
            .as_deref()
            .is_some_and(|c| ct_eq(c.as_bytes(), self.csrf.as_bytes()));
        if !ok {
            self.log.push(format!("rejected csrf peer={peer}"));
            self.bad(now);
            return Err(Response::error(
                403,
                "csrf",
                "Missing or wrong CSRF token. Reload the page.",
            ));
        }
        let json = req.content_type.as_deref().is_some_and(|ct| {
            let mut parts = ct.split(';').map(|p| p.trim_matches(' '));
            parts
                .next()
                .is_some_and(|m| m.eq_ignore_ascii_case("application/json"))
                && parts.all(|p| p.eq_ignore_ascii_case("charset=utf-8"))
        });
        if !json {
            return Err(Response::error(415, "unsupported", "Send JSON."));
        }
        Ok(())
    }

    fn post_action(&mut self, peer: Ipv4Addr, req: &Request, now: Instant) -> Response {
        if let Err(r) = self.check_write(peer, req, now) {
            return r;
        }
        let Some(action) = Action::from_body(&req.body) else {
            return Response::error(400, "bad_request", "Unknown action or value.");
        };
        let confirm = action.needs_confirmation();
        if confirm && now.saturating_duration_since(self.last_heartbeat) > self.t.app_gone {
            return Response::error(
                503,
                "app_gone",
                "The TV app is not answering. Open it on the TV.",
            );
        }
        if self.pending.is_some() {
            return Response::error(409, "busy", "Another request is waiting for the TV.");
        }
        if confirm && self.cooldown_until.is_some_and(|t| now < t) {
            return Response::error(
                429,
                "rate_limited",
                "Wait a moment before the next request.",
            );
        }
        if let Err(msg) = action.check(&self.mirror.facts()) {
            return Response::error(422, "not_applicable", msg);
        }
        let Ok(id) = random_hex(self.rng.as_mut(), 4) else {
            return Response::error(500, "internal", "No random source.");
        };
        let value = action.value().unwrap_or_default();
        self.dirty = true;
        if confirm {
            self.log.push(format!(
                "action requested id={id} action={} value={value} peer={peer}",
                action.id()
            ));
            self.pending = Some(Pending {
                id: id.clone(),
                action,
                peer,
                created: now,
                running: false,
                confirm: true,
            });
            return Response::json(
                202,
                &json!({"id": id, "state": "pending", "timeout_s": self.t.request_timeout.as_secs()}),
            );
        }
        self.log.push(format!(
            "action auto id={id} action={} value={value} peer={peer}",
            action.id()
        ));
        self.pending = Some(Pending {
            id: id.clone(),
            action,
            peer,
            created: now,
            running: true,
            confirm: false,
        });
        self.queued = Some(Job::Action {
            id: id.clone(),
            action,
        });
        Response::json(
            202,
            &json!({"id": id, "state": "running", "timeout_s": null}),
        )
    }

    fn action_state(&self, id: &str, now: Instant) -> Response {
        if !is_id(id) {
            return Response::error(404, "not_found", "Unknown request.");
        }
        if let Some(p) = self.pending.as_ref().filter(|p| p.id == id) {
            return Response::json(
                200,
                &json!({
                    "id": p.id,
                    "state": if p.running { "running" } else { "pending" },
                    "timeout_in_s": self.timeout_in(p, now),
                    "result": null,
                }),
            );
        }
        if let Some(f) = self
            .finished
            .as_ref()
            .filter(|f| f.id == id && now.saturating_duration_since(f.at) < KEEP_FINISHED)
        {
            let (state, result) = match f.verdict {
                Verdict::Allowed | Verdict::Auto => {
                    ("done", json!({"result": f.result, "error": f.error}))
                }
                Verdict::Refused => ("refused", Value::Null),
                Verdict::Expired => ("expired", Value::Null),
                Verdict::Aborted => ("aborted", Value::Null),
            };
            return Response::json(
                200,
                &json!({"id": f.id, "state": state, "timeout_in_s": null, "result": result}),
            );
        }
        Response::error(404, "not_found", "Unknown request.")
    }

    fn post_end(&mut self, peer: Ipv4Addr, req: &Request, now: Instant) -> Response {
        if let Err(r) = self.check_write(peer, req, now) {
            return r;
        }
        let empty_object =
            serde_json::from_slice::<Map<String, Value>>(&req.body).is_ok_and(|m| m.is_empty());
        if !empty_object {
            return Response::error(400, "bad_request", "Send {}.");
        }
        self.end(EndReason::Done, now);
        Response::json(200, &json!({"state": "ended", "reason": "done"}))
    }

    // --- TV answers and the worker ------------------------------------------

    pub(crate) fn on_answer(&mut self, a: &Answer, now: Instant) {
        let matches = self
            .pending
            .as_ref()
            .is_some_and(|p| p.confirm && !p.running && p.id == a.id);
        if !matches {
            self.log.push("answer ignored".to_owned());
            return;
        }
        if a.allow {
            if let Some(p) = self.pending.as_mut() {
                p.running = true;
                self.queued = Some(Job::Action {
                    id: p.id.clone(),
                    action: p.action,
                });
                self.log.push(format!("answer allow id={}", p.id));
            }
        } else if let Some(p) = self.pending.take() {
            self.log.push(format!("answer refuse id={}", p.id));
            self.finish(p, Verdict::Refused, "", "", now);
            self.strike(now);
        }
        self.dirty = true;
    }

    /// True while a request waits for the TV (the answer file is read then).
    #[cfg(test)]
    pub(crate) fn awaiting_answer(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| p.confirm && !p.running)
    }

    /// The next job for an idle worker: an approved action first, then a
    /// state refresh (at once after the token and each action, then every
    /// `refresh` interval while a phone has a session).
    pub(crate) fn next_job(&mut self, now: Instant) -> Option<Job> {
        if self.worker_busy || self.end.is_some() {
            return None;
        }
        let job = self.queued.take().or_else(|| {
            let due = self.session.is_some()
                && (self.refresh_due || self.mirror.age(now).is_none_or(|a| a >= self.t.refresh));
            due.then(|| {
                self.refresh_due = false;
                Job::Refresh
            })
        })?;
        self.worker_busy = true;
        Some(job)
    }

    pub(crate) fn on_job_done(&mut self, done: JobDone, now: Instant) {
        self.worker_busy = false;
        match done {
            JobDone::Refresh { device, tv } => self.mirror.update(device, tv, now),
            JobDone::Action {
                id,
                action,
                outcome,
            } => {
                self.refresh_due = true;
                let (result, error) = action.outcome(&outcome);
                if !matches!(outcome, CmdOutcome::Exited { .. }) || result == "failed" {
                    self.log.push(format!(
                        "action result id={id} result={result} error={error}"
                    ));
                } else {
                    self.log
                        .push(format!("action result id={id} result={result}"));
                }
                if let Some(p) = self.pending.take_if(|p| p.id == id && p.running) {
                    let v = if p.confirm {
                        Verdict::Allowed
                    } else {
                        Verdict::Auto
                    };
                    self.finish(p, v, &result, &error, now);
                }
            }
        }
    }

    fn finish(&mut self, p: Pending, verdict: Verdict, result: &str, error: &str, now: Instant) {
        self.finished = Some(Finished {
            id: p.id,
            action: p.action,
            peer: p.peer,
            verdict,
            result: result.to_owned(),
            error: error.to_owned(),
            at: now,
        });
        self.dirty = true;
    }

    // --- the state file -------------------------------------------------------

    fn timeout_in(&self, p: &Pending, now: Instant) -> Value {
        if p.running || !p.confirm {
            return Value::Null;
        }
        json!(
            (p.created + self.t.request_timeout)
                .saturating_duration_since(now)
                .as_secs()
        )
    }

    fn pending_json(&self, now: Instant, with_peer: bool) -> Value {
        let Some(p) = &self.pending else {
            return Value::Null;
        };
        let mut v = json!({
            "id": p.id,
            "action": p.action.id(),
            "value": p.action.value(),
            "state": if p.running { "running" } else { "pending" },
            "confirm": p.confirm,
            "timeout_in_s": self.timeout_in(p, now),
        });
        if with_peer && let Some(m) = v.as_object_mut() {
            m.insert("peer".into(), json!(p.peer.to_string()));
        }
        v
    }

    fn last_result_json(&self) -> Value {
        let Some(f) = &self.finished else {
            return Value::Null;
        };
        json!({
            "id": f.id,
            "action": f.action.id(),
            "value": f.action.value(),
            "verdict": f.verdict.as_str(),
            "result": f.result,
            "error": f.error,
            "peer": f.peer.to_string(),
        })
    }

    /// `state.json` (section 4.3, schema 1). The token path is present only
    /// while waiting.
    pub(crate) fn state_json(&self, now: Instant) -> Value {
        let state = match (self.end, &self.session) {
            (Some(_), _) => "ended",
            (None, Some(_)) => "active",
            (None, None) => "waiting",
        };
        let mut m = Map::new();
        m.insert("schema".into(), json!(1));
        m.insert("state".into(), json!(state));
        m.insert(
            "reason".into(),
            json!(self.end.map_or("", EndReason::as_str)),
        );
        m.insert("port".into(), json!(self.port));
        if state == "waiting" {
            m.insert("path".into(), json!(format!("/s/{}", self.token)));
        }
        let left = if self.end.is_some() {
            0
        } else {
            self.left(now)
        };
        m.insert("expires_in_s".into(), json!(left));
        let s = self.session.as_ref();
        m.insert("peer".into(), json!(s.map(|s| s.peer.to_string())));
        m.insert("peer_other_subnet".into(), json!(s.map(|s| s.other_subnet)));
        m.insert("pending".into(), self.pending_json(now, true));
        m.insert("last_result".into(), self.last_result_json());
        Value::Object(m)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::too_many_lines,
        clippy::assert_is_empty,
        clippy::format_push_string,
        clippy::cast_possible_truncation
    )]

    use super::*;
    use crate::http::{Parsed, parse};
    use crate::secret::tests::Counter;

    pub(crate) const PHONE: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 23);
    const OTHER: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 99);
    const LOCAL: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 50), 49731);
    const HOST: &str = "192.168.1.50:49731";

    struct T {
        core: Core,
        t0: Instant,
        now: Instant,
    }

    fn req(raw: &str) -> Request {
        match parse(raw.as_bytes()) {
            Ok(Parsed::Done(r)) => r,
            other => panic!("{other:?}"),
        }
    }

    fn body_json(r: &Response) -> Value {
        serde_json::from_slice(&r.body).unwrap_or(Value::Null)
    }

    impl T {
        fn new() -> Self {
            Self::with(Timing::default())
        }

        fn with(t: Timing) -> Self {
            let t0 = Instant::now();
            let core = Core::new(t, Box::new(Counter(0)), LOCAL.port(), t0).expect("core");
            T { core, t0, now: t0 }
        }

        fn at(&mut self, ms: u64) {
            self.now = self.t0 + Duration::from_millis(ms);
            self.core.on_heartbeat(self.now);
            self.core.tick(self.now);
        }

        /// Advances time without a heartbeat.
        fn quiet(&mut self, ms: u64) {
            self.now = self.t0 + Duration::from_millis(ms);
            self.core.tick(self.now);
        }

        fn send_from(&mut self, peer: Ipv4Addr, raw: &str) -> Response {
            let r = req(raw);
            self.core.handle(peer, LOCAL, &r, self.now)
        }

        fn send(&mut self, raw: &str) -> Response {
            self.send_from(PHONE, raw)
        }

        fn path(&self) -> String {
            self.core.state_json(self.now)["path"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        }

        fn cookie(&self) -> String {
            self.core.cookie.clone()
        }

        fn get(&mut self, path: &str, extra: &str) -> Response {
            self.send(&format!(
                "GET {path} HTTP/1.1\r\nHost: {HOST}\r\n{extra}\r\n"
            ))
        }

        fn authed(&mut self, path: &str) -> Response {
            let c = self.cookie();
            self.get(path, &format!("Cookie: sat_session={c}\r\n"))
        }

        fn login(&mut self) {
            let p = self.path();
            let r = self.get(&p, "");
            assert_eq!(r.status, 302);
            // A first refresh, as the worker would deliver it.
            assert!(matches!(self.core.next_job(self.now), Some(Job::Refresh)));
            self.core.on_job_done(
                JobDone::Refresh {
                    device: Ok(
                        br#"{"installed":true,"running":true,"key":"unpaired","port":"6053"}"#
                            .to_vec(),
                    ),
                    tv: Ok(br#"{"luna":true,"always_ready":{"raw":"off"}}"#.to_vec()),
                },
                self.now,
            );
        }

        fn post(&mut self, path: &str, body: &str) -> Response {
            let c = self.cookie();
            let x = self.core.csrf.clone();
            self.send(&format!(
                "POST {path} HTTP/1.1\r\nHost: {HOST}\r\nCookie: sat_session={c}\r\nX-Setup-CSRF: {x}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ))
        }

        fn action(&mut self, body: &str) -> Response {
            self.post("/api/actions", body)
        }

        fn answer(&mut self, id: &str, allow: bool) {
            self.core.on_answer(
                &Answer {
                    id: id.into(),
                    allow,
                },
                self.now,
            );
        }

        fn secrets(&self) -> [String; 3] {
            [
                self.core.token.clone(),
                self.core.cookie.clone(),
                self.core.csrf.clone(),
            ]
        }

        fn assert_no_secret_logged(&mut self) {
            let log = self.core.take_log().join("\n");
            for s in self.secrets() {
                assert!(!log.contains(&s), "secret in log: {log}");
            }
            assert!(!log.contains("/s/"), "{log}");
        }
    }

    #[test]
    fn token_is_single_use_and_sets_a_strict_cookie() {
        let mut t = T::new();
        let state = t.core.state_json(t.now);
        assert_eq!(state["state"], "waiting");
        assert_eq!(state["path"].as_str().map(str::len), Some(46));
        let p = t.path();
        let r = t.get(&p, "");
        assert_eq!(r.status, 302);
        assert_eq!(r.header_value("Location"), Some("/"));
        let c = r.header_value("Set-Cookie").expect("cookie").to_owned();
        for flag in [
            "; Path=/",
            "; HttpOnly",
            "; SameSite=Strict",
            "; Max-Age=600",
        ] {
            assert!(c.contains(flag), "{c}");
        }
        assert!(c.starts_with("sat_session=") && !c.contains("Secure"));
        let state = t.core.state_json(t.now);
        assert_eq!(state["state"], "active");
        assert!(state.get("path").is_none());
        assert_eq!(state["peer"], "192.168.1.23");
        assert_eq!(state["peer_other_subnet"], false);
        assert!(!state.to_string().contains(&t.core.token));
        // A second use without the cookie, from anyone: 410.
        assert_eq!(t.get(&p, "").status, 410);
        assert_eq!(
            t.send_from(OTHER, &format!("GET {p} HTTP/1.1\r\nHost: {HOST}\r\n\r\n"))
                .status,
            410
        );
        // The same phone with its cookie is sent to the page.
        let c = t.cookie();
        assert_eq!(
            t.get(&p, &format!("Cookie: sat_session={c}\r\n")).status,
            302
        );
        t.assert_no_secret_logged();
    }

    #[test]
    fn wrong_tokens_and_head_do_not_consume() {
        let mut t = T::new();
        let p = t.path();
        let mut wrong = p.clone();
        wrong.pop();
        wrong.push('A');
        if wrong == p {
            wrong.pop();
            wrong.push('B');
        }
        assert_eq!(t.get(&wrong, "").status, 404);
        assert_eq!(t.get("/s/short", "").status, 404);
        let r = t.send(&format!("HEAD {p} HTTP/1.1\r\nHost: {HOST}\r\n\r\n"));
        assert_eq!(r.status, 405);
        assert!(r.body.is_empty());
        assert_eq!(
            t.send(&format!("OPTIONS {p} HTTP/1.1\r\nHost: {HOST}\r\n\r\n"))
                .status,
            405
        );
        assert_eq!(
            t.send(&format!(
                "POST {p} HTTP/1.1\r\nHost: {HOST}\r\nContent-Length: 0\r\n\r\n"
            ))
            .status,
            405
        );
        assert_eq!(t.core.state_json(t.now)["state"], "waiting");
        assert_eq!(t.get(&p, "").status, 302);
    }

    #[test]
    fn another_subnet_is_flagged() {
        let mut t = T::new();
        let p = t.path();
        let r = t.send_from(
            Ipv4Addr::new(10, 0, 2, 5),
            &format!("GET {p} HTTP/1.1\r\nHost: {HOST}\r\n\r\n"),
        );
        assert_eq!(r.status, 302);
        assert_eq!(t.core.state_json(t.now)["peer_other_subnet"], true);
    }

    #[test]
    fn session_checks() {
        let mut t = T::new();
        // Before any session: the page says what to do, the API is 403.
        let r = t.get("/", "");
        assert_eq!(r.status, 403);
        assert!(String::from_utf8_lossy(&r.body).contains("QR code"));
        t.login();
        let c = t.cookie();
        assert_eq!(t.authed("/").status, 200);
        assert_eq!(t.authed("/api/session").status, 200);
        assert_eq!(t.get("/api/state", "").status, 403);
        assert_eq!(
            t.get("/api/state", "Cookie: sat_session=AAAA\r\n").status,
            403
        );
        assert_eq!(
            t.get(
                "/api/state",
                &format!("Cookie: other=1; sat_session={c}x\r\n")
            )
            .status,
            403
        );
        assert_eq!(
            t.get(
                "/api/state",
                &format!("Cookie: sat_session={c}; sat_session={c}\r\n")
            )
            .status,
            403
        );
        assert_eq!(
            t.get(
                "/api/state",
                &format!("Cookie: a=b; sat_session={c}; c=d\r\n")
            )
            .status,
            200
        );
        // The right cookie from another address.
        let r = t.send_from(
            OTHER,
            &format!("GET /api/state HTTP/1.1\r\nHost: {HOST}\r\nCookie: sat_session={c}\r\n\r\n"),
        );
        assert_eq!(r.status, 403);
        assert!(
            t.core
                .take_log()
                .iter()
                .any(|l| l.starts_with("rejected wrong-peer"))
        );
        // Unknown paths and assets.
        assert_eq!(t.authed("/nope").status, 404);
        assert_eq!(t.authed("/assets/phone.js").status, 200);
        assert_eq!(t.authed("/assets/../../etc/passwd").status, 404);
        assert_eq!(t.authed("/assets/nope.js").status, 404);
        assert_eq!(t.get("/assets/phone.js", "").status, 403);
        t.assert_no_secret_logged();
    }

    #[test]
    fn host_origin_and_fetch_site_checks() {
        let mut t = T::new();
        t.login();
        let c = t.cookie();
        let ck = format!("Cookie: sat_session={c}\r\n");
        for host in [
            "evil.example:49731",
            "192.168.1.50",
            "192.168.1.50:1",
            "localhost:49731",
        ] {
            let r = t.send(&format!(
                "GET /api/state HTTP/1.1\r\nHost: {host}\r\n{ck}\r\n"
            ));
            assert_eq!(r.status, 400, "{host}");
        }
        assert_eq!(t.authed("/api/state").status, 200);
        assert_eq!(
            t.get(
                "/api/state",
                &format!("{ck}Origin: http://evil.example\r\n")
            )
            .status,
            403
        );
        assert_eq!(
            t.get("/api/state", &format!("{ck}Origin: https://{HOST}\r\n"))
                .status,
            403
        );
        assert_eq!(
            t.get("/api/state", &format!("{ck}Origin: http://{HOST}\r\n"))
                .status,
            200
        );
        assert_eq!(
            t.get("/api/state", &format!("{ck}Sec-Fetch-Site: cross-site\r\n"))
                .status,
            403
        );
        assert_eq!(
            t.get("/api/state", &format!("{ck}Sec-Fetch-Site: same-site\r\n"))
                .status,
            403
        );
        assert_eq!(
            t.get(
                "/api/state",
                &format!("{ck}Sec-Fetch-Site: same-origin\r\n")
            )
            .status,
            200
        );
        assert_eq!(
            t.get("/api/state", &format!("{ck}Sec-Fetch-Site: none\r\n"))
                .status,
            200
        );
    }

    #[test]
    fn csrf_content_type_and_no_cors() {
        let mut t = T::new();
        t.login();
        let c = t.cookie();
        let x = t.core.csrf.clone();
        let body = r#"{"action":"mic","value":"on"}"#;
        let mk = |csrf: &str, ct: &str| {
            format!(
                "POST /api/actions HTTP/1.1\r\nHost: {HOST}\r\nCookie: sat_session={c}\r\n{csrf}{ct}Content-Length: {}\r\n\r\n{body}",
                body.len()
            )
        };
        let ct = "Content-Type: application/json\r\n";
        assert_eq!(t.send(&mk("", ct)).status, 403);
        assert_eq!(t.send(&mk("X-Setup-CSRF: 0000\r\n", ct)).status, 403);
        let good = format!("X-Setup-CSRF: {x}\r\n");
        assert_eq!(
            t.send(&mk(&good, "Content-Type: text/plain\r\n")).status,
            415
        );
        assert_eq!(t.send(&mk(&good, "")).status, 415);
        assert_eq!(
            t.send(&mk(
                &good,
                "Content-Type: application/x-www-form-urlencoded\r\n"
            ))
            .status,
            415
        );
        assert_eq!(
            t.send(&mk(
                &good,
                "Content-Type: application/json; charset=latin1\r\n"
            ))
            .status,
            415
        );
        assert_eq!(
            t.send(&mk(
                &good,
                "Content-Type: application/json; charset=UTF-8\r\n"
            ))
            .status,
            202
        );
        let r = t.send(&format!(
            "OPTIONS /api/actions HTTP/1.1\r\nHost: {HOST}\r\nOrigin: http://evil.example\r\nAccess-Control-Request-Method: POST\r\n\r\n"
        ));
        assert_eq!(r.status, 403);
        let r = t.send(&format!(
            "OPTIONS /api/actions HTTP/1.1\r\nHost: {HOST}\r\n\r\n"
        ));
        assert_eq!(r.status, 405);
        assert_eq!(r.header_value("Allow"), Some("POST"));
        assert!(
            !String::from_utf8_lossy(&r.to_bytes())
                .to_ascii_lowercase()
                .contains("access-control")
        );
        // GET on a POST route and POST on a GET route.
        assert_eq!(t.authed("/api/actions").status, 405);
        assert_eq!(t.post("/api/state", "{}").status, 405);
    }

    #[test]
    fn twenty_unauthenticated_requests_end_the_session() {
        let mut t = T::new();
        for i in 0..MAX_BAD {
            assert_eq!(t.core.ended(), None);
            t.at(u64::from(i) * 100);
            assert_eq!(t.get("/api/state", "").status, 403);
        }
        assert_eq!(t.core.ended(), Some(EndReason::TooManyBadRequests));
        let s = t.core.state_json(t.now);
        assert_eq!(s["state"], "ended");
        assert_eq!(s["reason"], "too-many-bad-requests");
        assert!(s.get("path").is_none());
        assert_eq!(t.get("/", "").status, 503);
    }

    #[test]
    fn filtered_peers_count_as_bad_requests() {
        let mut t = T::new();
        for _ in 0..MAX_BAD {
            t.core.on_rejected_peer(Ipv4Addr::new(8, 8, 8, 8), t.now);
        }
        assert_eq!(t.core.ended(), Some(EndReason::TooManyBadRequests));
    }

    #[test]
    fn parse_errors_before_and_after_the_token() {
        let mut t = T::new();
        let r = t.core.on_parse_error(OTHER, ParseError::Bad("byte"), t.now);
        assert_eq!(r.status, 400);
        assert_eq!(
            t.core
                .on_parse_error(OTHER, ParseError::HeadTooLarge, t.now)
                .status,
            431
        );
        assert_eq!(t.core.ended(), None);
        assert_eq!(
            t.core
                .on_parse_error(OTHER, ParseError::Timeout, t.now)
                .status,
            408
        );
        assert_eq!(t.core.ended(), Some(EndReason::ParseError));

        let mut t = T::new();
        t.core.on_parse_error(OTHER, ParseError::Bad("byte"), t.now);
        t.login();
        assert_eq!(t.core.ended(), None);
        t.core
            .on_parse_error(PHONE, ParseError::BodyTooLarge, t.now);
        assert_eq!(t.core.ended(), Some(EndReason::ParseError));
        assert!(
            t.core
                .take_log()
                .iter()
                .any(|l| l == "rejected parse=body-too-large peer=192.168.1.23")
        );
    }

    #[test]
    fn rate_limit() {
        let mut t = T::new();
        t.login();
        // login used one request.
        for _ in 1..RATE_PER_S {
            assert_eq!(t.authed("/api/state").status, 200);
        }
        assert_eq!(t.authed("/api/state").status, 429);
        t.at(1100);
        assert_eq!(t.authed("/api/state").status, 200);
    }

    #[test]
    fn the_request_cap() {
        let mut t = T::new();
        t.login();
        let mut ms = 0;
        for _ in 1..MAX_REQUESTS {
            ms += 60;
            t.at(ms);
            assert_eq!(t.authed("/api/session").status, 200);
        }
        t.at(ms + 1000);
        assert_eq!(t.authed("/api/session").status, 429);
    }

    fn id_of(r: &Response) -> String {
        body_json(r)["id"].as_str().unwrap_or_default().to_owned()
    }

    #[test]
    fn allowed_action_runs_and_reports() {
        let mut t = T::new();
        t.login();
        let r = t.action(r#"{"action":"hi-lg","value":"off"}"#);
        assert_eq!(r.status, 202, "{:?}", body_json(&r));
        assert_eq!(body_json(&r)["state"], "pending");
        assert_eq!(body_json(&r)["timeout_s"], 60);
        let id = id_of(&r);
        assert!(is_id(&id));
        let s = t.core.state_json(t.now);
        assert_eq!(
            s["pending"],
            json!({"id": id, "action": "hi-lg", "value": "off", "peer": "192.168.1.23", "state": "pending", "confirm": true, "timeout_in_s": 60})
        );
        assert!(t.core.awaiting_answer());
        // Nothing runs before the OK.
        assert!(t.core.next_job(t.now).is_none());
        // A wrong id is ignored.
        t.answer("ffffffff", true);
        assert!(t.core.awaiting_answer());
        t.at(5000);
        assert_eq!(t.core.state_json(t.now)["pending"]["timeout_in_s"], 55);
        t.answer(&id, true);
        assert!(!t.core.awaiting_answer());
        let job = t.core.next_job(t.now);
        assert!(
            matches!(&job, Some(Job::Action { id: j, action: Action::HiLg(false) }) if *j == id)
        );
        assert_eq!(
            body_json(&t.authed(&format!("/api/actions/{id}")))["state"],
            "running"
        );
        assert_eq!(t.core.state_json(t.now)["pending"]["state"], "running");
        // The worker is busy: no second job.
        assert!(t.core.next_job(t.now).is_none());
        t.core.on_job_done(
            JobDone::Action {
                id: id.clone(),
                action: Action::HiLg(false),
                outcome: CmdOutcome::Exited {
                    success: true,
                    code: Some(0),
                    stdout: br#"{"result":"applied","error":""}"#.to_vec(),
                },
            },
            t.now,
        );
        let s = t.core.state_json(t.now);
        assert_eq!(s["pending"], Value::Null);
        assert_eq!(
            s["last_result"],
            json!({"id": id, "action": "hi-lg", "value": "off", "verdict": "allowed", "result": "applied", "error": "", "peer": "192.168.1.23"})
        );
        let r = body_json(&t.authed(&format!("/api/actions/{id}")));
        assert_eq!(
            r,
            json!({"id": id, "state": "done", "timeout_in_s": null, "result": {"result": "applied", "error": ""}})
        );
        // An immediate refresh follows.
        assert!(matches!(t.core.next_job(t.now), Some(Job::Refresh)));
        // Kept for 60 s, then gone.
        t.at(66_000);
        assert_eq!(t.authed(&format!("/api/actions/{id}")).status, 404);
        assert_eq!(t.authed("/api/actions/NOTANID").status, 404);
        t.assert_no_secret_logged();
    }

    #[test]
    fn mic_off_runs_without_the_tv() {
        let mut t = T::new();
        t.login();
        let r = t.action(r#"{"action":"mic","value":"off"}"#);
        assert_eq!(r.status, 202);
        assert_eq!(body_json(&r)["state"], "running");
        let id = id_of(&r);
        let s = t.core.state_json(t.now);
        assert_eq!(s["pending"]["confirm"], false);
        assert_eq!(s["pending"]["state"], "running");
        assert!(!t.core.awaiting_answer());
        assert!(matches!(
            t.core.next_job(t.now),
            Some(Job::Action {
                action: Action::Mic(false),
                ..
            })
        ));
        // An answer cannot touch it.
        t.answer(&id, false);
        t.core.on_job_done(
            JobDone::Action {
                id: id.clone(),
                action: Action::Mic(false),
                outcome: CmdOutcome::Exited {
                    success: true,
                    code: Some(0),
                    stdout: vec![],
                },
            },
            t.now,
        );
        let s = t.core.state_json(t.now);
        assert_eq!(s["last_result"]["verdict"], "auto");
        assert_eq!(s["last_result"]["result"], "ok");
        assert_eq!(
            body_json(&t.authed(&format!("/api/actions/{id}")))["state"],
            "done"
        );
        // Mic on still needs the OK.
        let r = t.action(r#"{"action":"mic","value":"on"}"#);
        assert_eq!(body_json(&r)["state"], "pending");
        assert!(t.core.awaiting_answer());
    }

    #[test]
    fn refusal_expiry_cooldown_and_three_strikes() {
        let mut t = T::new();
        t.login();
        let id = id_of(&t.action(r#"{"action":"remote-off","value":"keep"}"#));
        // One pending at a time.
        assert_eq!(t.action(r#"{"action":"mic","value":"on"}"#).status, 409);
        assert_eq!(t.action(r#"{"action":"mic","value":"off"}"#).status, 409);
        t.answer(&id, false);
        assert!(
            t.core
                .next_job(t.now)
                .is_none_or(|j| matches!(j, Job::Refresh))
        );
        assert_eq!(
            t.core.state_json(t.now)["last_result"]["verdict"],
            "refused"
        );
        assert_eq!(
            body_json(&t.authed(&format!("/api/actions/{id}")))["state"],
            "refused"
        );
        // The cooldown (mic off is exempt).
        assert_eq!(t.action(r#"{"action":"mic","value":"on"}"#).status, 429);
        t.at(3100);
        let id2 = id_of(&t.action(r#"{"action":"mic","value":"on"}"#));
        t.at(3100 + 59_000);
        assert!(t.core.awaiting_answer());
        t.at(3100 + 60_000);
        assert!(!t.core.awaiting_answer());
        assert_eq!(
            t.core.state_json(t.now)["last_result"]["verdict"],
            "expired"
        );
        assert_eq!(
            body_json(&t.authed(&format!("/api/actions/{id2}")))["state"],
            "expired"
        );
        assert_eq!(t.core.ended(), None);
        t.at(70_000);
        let id3 = id_of(&t.action(r#"{"action":"reply-volume","value":"30"}"#));
        t.answer(&id3, false);
        assert_eq!(t.core.ended(), Some(EndReason::TooManyRefusals));
    }

    #[test]
    fn preconditions_and_bad_bodies() {
        let mut t = T::new();
        t.login();
        // login's state: key unpaired, running, luna, raw off.
        assert_eq!(t.action(r#"{"action":"pair"}"#).status, 202);
        let mut t = T::new();
        t.login();
        t.core.on_job_done(
            JobDone::Refresh {
                device: Ok(br#"{"running":true,"key":"paired"}"#.to_vec()),
                tv: Err("timeout".into()),
            },
            t.now,
        );
        let r = t.action(r#"{"action":"pair"}"#);
        assert_eq!(r.status, 422);
        assert_eq!(body_json(&r)["error"], "not_applicable");
        assert_eq!(t.action(r#"{"action":"hi-lg","value":"on"}"#).status, 422);
        for b in [
            r#"{"action":"pair","x":1}"#,
            r#"{"action":"reboot"}"#,
            "not json",
            r#"{"action":"reply-volume","value":"101"}"#,
        ] {
            assert_eq!(t.action(b).status, 400, "{b}");
        }
        assert_eq!(t.core.ended(), None);
    }

    #[test]
    fn a_late_heartbeat_refuses_new_requests() {
        let mut t = T::new();
        t.login();
        t.quiet(8_500);
        assert_eq!(t.action(r#"{"action":"mic","value":"on"}"#).status, 503);
        // Mic off still works (no overlay needed).
        assert_eq!(t.action(r#"{"action":"mic","value":"off"}"#).status, 202);
    }

    #[test]
    fn heartbeat_loss_aborts_the_pending_request() {
        let mut t = T::new();
        t.login();
        let id = id_of(&t.action(r#"{"action":"hi-lg","value":"on"}"#));
        t.quiet(14_900);
        assert_eq!(t.core.ended(), None);
        t.quiet(15_000);
        assert_eq!(t.core.ended(), Some(EndReason::AppGone));
        let s = t.core.state_json(t.now);
        assert_eq!(s["last_result"]["verdict"], "aborted");
        assert_eq!(s["last_result"]["id"], json!(id));
        assert_eq!(s["pending"], Value::Null);
        assert!(t.core.next_job(t.now).is_none());
    }

    #[test]
    fn hard_timeout_and_idle() {
        let mut t = T::new();
        t.at(599_000);
        assert_eq!(t.core.ended(), None);
        assert_eq!(t.core.state_json(t.now)["expires_in_s"], 1);
        t.at(600_000);
        assert_eq!(t.core.ended(), Some(EndReason::Timeout));
        assert_eq!(t.core.state_json(t.now)["expires_in_s"], 0);

        let mut t = T::new();
        t.login();
        t.at(179_000);
        assert_eq!(t.core.ended(), None);
        t.authed("/api/state");
        t.at(179_000 + 179_000);
        assert_eq!(t.core.ended(), None);
        t.at(179_000 + 180_000);
        assert_eq!(t.core.ended(), Some(EndReason::Idle));
        // Waiting (no token used) has no idle end.
        let mut t = T::new();
        t.at(400_000);
        assert_eq!(t.core.ended(), None);
    }

    #[test]
    fn done_from_the_phone_and_running_actions_at_the_end() {
        let mut t = T::new();
        t.login();
        let id = id_of(&t.action(r#"{"action":"hi-lg","value":"on"}"#));
        t.answer(&id, true);
        assert_eq!(t.post("/api/session/end", r#"{"x":1}"#).status, 400);
        let r = t.post("/api/session/end", "{}");
        assert_eq!(r.status, 200);
        assert_eq!(body_json(&r), json!({"state": "ended", "reason": "done"}));
        assert_eq!(t.core.ended(), Some(EndReason::Done));
        let s = t.core.state_json(t.now);
        assert_eq!(s["last_result"]["verdict"], "allowed");
        assert_eq!(s["last_result"]["result"], "");
        assert!(t.core.next_job(t.now).is_none());
        t.assert_no_secret_logged();
    }

    #[test]
    fn end_from_the_tv_and_first_reason_wins() {
        let mut t = T::new();
        t.core.end(EndReason::Done, t.now);
        t.core.end(EndReason::Timeout, t.now);
        assert_eq!(t.core.ended(), Some(EndReason::Done));
        let log = t.core.take_log();
        assert_eq!(
            log.iter().filter(|l| l.starts_with("session end")).count(),
            1
        );
    }

    #[test]
    fn refreshes_follow_the_interval_only_with_a_session() {
        let mut t = T::new();
        assert!(t.core.next_job(t.now).is_none());
        t.login();
        assert!(t.core.next_job(t.now).is_none());
        t.at(2_900);
        assert!(t.core.next_job(t.now).is_none());
        t.at(3_000);
        assert!(matches!(t.core.next_job(t.now), Some(Job::Refresh)));
        assert!(t.core.next_job(t.now).is_none());
        let v = body_json(&t.authed("/api/state"));
        assert_eq!(v["device"]["port"], 6053);
        assert_eq!(v["age_s"], 3.0);
    }
}
