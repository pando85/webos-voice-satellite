//! Runs the real `webos-satellite-setup` binary on 127.0.0.1 with a fake
//! `ctl.sh`, a temporary install prefix and session directory, and drives it
//! with a std `TcpStream` client (docs/design/phone-setup.md, section 14.1).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::unused_self,
    missing_docs
)]

use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_webos-satellite-setup");

const FAKE_CTL: &str = r#"#!/bin/sh
here=$(dirname "$0")
echo "$*" >>"$here/calls"
ls -l /proc/$$/fd 2>/dev/null | grep -c socket >"$here/sockets.$1"
[ ! -f "$here/hang" ] || sleep 30
case $1 in
status) echo '{"installed":true,"installed_version":"0.4.0","running":true,"key":"unpaired","name":"CANARY","prefix":"/CANARY","port":"6053","reply_volume":"","reply_volume_default":"40","log":["CANARY"]}' ;;
tv-settings) echo '{"schema":1,"luna":true,"always_ready":{"state":"off","raw":"off","error":""},"hi_lg":{"state":"on","thinq":true,"amazon":false,"dangbei":false,"error":""},"remote_off":{"state":"normal","active":false},"mic":{"state":"listening","kill_switch":false},"power_state":"Active","last_change":null}' ;;
set) printf '{"schema":1,"setting":"%s","requested":"%s","before":"x","after":"%s","result":"applied","error":""}\n' "$2" "$3" "$3" ;;
mic | pair) exit 0 ;;
*) exit 2 ;;
esac
"#;

struct Server {
    root: PathBuf,
    dir: PathBuf,
    child: Child,
    port: u16,
    beat: Arc<AtomicBool>,
}

fn scratch() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let p = std::env::temp_dir().join(format!(
        "setup-server-it-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn write_exec(path: &Path, text: &str) {
    // Through a temporary name, so no writable descriptor of the final file
    // can leak into a concurrent fork (ETXTBSY).
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).unwrap();
    fs::rename(&tmp, path).unwrap();
}

fn touch(path: &Path) {
    let f = fs::File::options()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    f.set_modified(SystemTime::now()).unwrap();
}

impl Server {
    fn start(extra: &[&str]) -> Server {
        Self::start_with(extra, true)
    }

    fn start_with(extra: &[&str], heartbeat: bool) -> Server {
        let root = scratch();
        let prefix = root.join("prefix");
        fs::create_dir_all(prefix.join("bin")).unwrap();
        fs::write(prefix.join(".webos-satellite-install"), "x").unwrap();
        write_exec(&prefix.join("bin/ctl.sh"), FAKE_CTL);
        let dir = root.join("setup");
        let child = Command::new(BIN)
            .args(["--prefix", prefix.to_str().unwrap()])
            .args(["--dir", dir.to_str().unwrap()])
            .args(["--log", root.join("setup.log").to_str().unwrap()])
            .args(["--bind", "127.0.0.1", "--allow-loopback"])
            .args(extra)
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let start = Instant::now();
        let state = loop {
            if let Ok(b) = fs::read(dir.join("state.json")) {
                break serde_json::from_slice::<Value>(&b).unwrap();
            }
            assert!(start.elapsed() < Duration::from_secs(5), "no state.json");
            thread::sleep(Duration::from_millis(20));
        };
        let port = u16::try_from(state["port"].as_u64().unwrap()).unwrap();
        let beat = Arc::new(AtomicBool::new(heartbeat));
        let (b, hb) = (beat.clone(), dir.join("heartbeat"));
        thread::spawn(move || {
            while b.load(Ordering::Relaxed) {
                if hb.parent().is_some_and(Path::is_dir) {
                    touch(&hb);
                }
                thread::sleep(Duration::from_millis(250));
            }
        });
        Server {
            root,
            dir,
            child,
            port,
            beat,
        }
    }

    fn state(&self) -> Value {
        serde_json::from_slice(&fs::read(self.dir.join("state.json")).unwrap()).unwrap()
    }

    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    fn raw(&self, req: &[u8]) -> Reply {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.write_all(req).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        Reply::parse(&out)
    }

    fn get(&self, path: &str, cookie: Option<&str>) -> Reply {
        let c = cookie
            .map(|c| format!("Cookie: sat_session={c}\r\n"))
            .unwrap_or_default();
        self.raw(format!("GET {path} HTTP/1.1\r\nHost: {}\r\n{c}\r\n", self.host()).as_bytes())
    }

    fn post(&self, path: &str, cookie: &str, csrf: &str, body: &str) -> Reply {
        self.raw(
            format!(
                "POST {path} HTTP/1.1\r\nHost: {}\r\nCookie: sat_session={cookie}\r\nX-Setup-CSRF: {csrf}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                self.host(),
                body.len()
            )
            .as_bytes(),
        )
    }

    /// Consumes the token; returns the cookie and the CSRF token.
    fn login(&self) -> (String, String) {
        let path = self.state()["path"].as_str().unwrap().to_owned();
        let r = self.get(&path, None);
        // A page that moves on to `/` by itself (not a 302), section 8.3.
        assert_eq!(r.status, 200);
        assert!(r.header("location").is_none());
        let set = r.header("set-cookie").unwrap();
        let cookie = set
            .strip_prefix("sat_session=")
            .and_then(|s| s.split(';').next())
            .unwrap()
            .to_owned();
        let s = self.get("/api/session", Some(&cookie));
        assert_eq!(s.status, 200);
        let csrf = s.json()["csrf"].as_str().unwrap().to_owned();
        (cookie, csrf)
    }

    fn answer(&self, id: &str, verdict: &str) {
        let tmp = self.dir.join(".answer.tmp");
        fs::write(&tmp, format!(r#"{{"id":"{id}","verdict":"{verdict}"}}"#)).unwrap();
        fs::rename(&tmp, self.dir.join("answer.json")).unwrap();
    }

    fn wait_exit(&mut self, limit: Duration) -> ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(st) = self.child.try_wait().unwrap() {
                return st;
            }
            assert!(start.elapsed() < limit, "server did not exit");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for(&self, what: &str, mut f: impl FnMut() -> bool) {
        let start = Instant::now();
        while !f() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "timed out waiting for {what}"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.root.join("prefix/bin/calls")).unwrap_or_default()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.root.join("setup.log")).unwrap_or_default()
    }

    fn port_closed(&self) -> bool {
        TcpStream::connect(("127.0.0.1", self.port)).is_err()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.beat.store(false, Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Reply {
    status: u16,
    head: String,
    body: Vec<u8>,
}

impl Reply {
    fn parse(raw: &[u8]) -> Reply {
        let end = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map_or(raw.len(), |i| i + 4);
        let head = String::from_utf8_lossy(&raw[..end]).into_owned();
        let status = head.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0);
        Reply {
            status,
            head,
            body: raw[end..].to_vec(),
        }
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|l| {
            let (n, v) = l.split_once(':')?;
            n.eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

#[test]
fn version_prints_the_target() {
    let out = Command::new(BIN).arg("--version").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("webos-satellite-setup "), "{text}");
    assert!(text.contains('(') && text.contains("-linux-"), "{text}");
}

#[test]
fn refuses_to_start_without_the_install_marker() {
    let root = scratch();
    let out = Command::new(BIN)
        .args([
            "--prefix",
            root.to_str().unwrap(),
            "--dir",
            root.join("s").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(!root.join("s").exists());
    let out = Command::new(BIN)
        .args(["--lifetime-s", "9999"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn full_flow_on_localhost() {
    let mut s = Server::start(&["--cooldown-s", "0"]);
    let st = s.state();
    assert_eq!(st["state"], "waiting");
    let path = st["path"].as_str().unwrap().to_owned();
    assert_eq!(path.len(), 46);
    let token = path.trim_start_matches("/s/").to_owned();
    // Files and modes.
    assert_eq!(fs::metadata(&s.dir).unwrap().mode() & 0o777, 0o700);
    assert_eq!(
        fs::metadata(s.dir.join("state.json")).unwrap().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(s.root.join("setup.log")).unwrap().mode() & 0o777,
        0o600
    );
    // HEAD does not consume; a wrong host is refused.
    let r = s.raw(format!("HEAD {path} HTTP/1.1\r\nHost: {}\r\n\r\n", s.host()).as_bytes());
    assert_eq!(r.status, 405);
    let r = s.raw(format!("GET {path} HTTP/1.1\r\nHost: localhost:{}\r\n\r\n", s.port).as_bytes());
    assert_eq!(r.status, 400);

    let (cookie, csrf) = s.login();
    assert_eq!(s.get(&path, None).status, 410);
    let st = s.state();
    assert_eq!(st["state"], "active");
    assert!(st.get("path").is_none());
    assert_eq!(st["peer"], "127.0.0.1");

    // The page and its assets, with the CSP.
    let page = s.get("/", Some(&cookie));
    assert_eq!(page.status, 200);
    assert!(
        page.header("content-security-policy")
            .unwrap()
            .contains("default-src 'none'")
    );
    assert_eq!(s.get("/assets/phone.js", Some(&cookie)).status, 200);
    assert_eq!(s.get("/", None).status, 403);

    // The status mirror arrives through the worker, allow-listed.
    s.wait_for("the state mirror", || {
        !s.get("/api/state", Some(&cookie)).json()["device"].is_null()
    });
    let mirror = s.get("/api/state", Some(&cookie));
    let text = String::from_utf8_lossy(&mirror.body).into_owned();
    assert!(!text.contains("CANARY"), "{text}");
    assert_eq!(mirror.json()["device"]["port"], 6053);
    assert_eq!(mirror.json()["tv"]["power_state"], "Active");

    // An approved action.
    let r = s.post(
        "/api/actions",
        &cookie,
        &csrf,
        r#"{"action":"hi-lg","value":"off"}"#,
    );
    assert_eq!(r.status, 202);
    let id = r.json()["id"].as_str().unwrap().to_owned();
    s.wait_for("pending in state.json", || {
        s.state()["pending"]["id"] == id.as_str()
    });
    assert_eq!(s.state()["pending"]["peer"], "127.0.0.1");
    assert_eq!(
        s.post(
            "/api/actions",
            &cookie,
            &csrf,
            r#"{"action":"mic","value":"on"}"#
        )
        .status,
        409
    );
    s.answer(&id, "allow");
    s.wait_for("the result", || {
        s.get(&format!("/api/actions/{id}"), Some(&cookie)).json()["state"] == "done"
    });
    let r = s.get(&format!("/api/actions/{id}"), Some(&cookie)).json();
    assert_eq!(r["result"]["result"], "applied");
    assert!(
        s.calls().lines().any(|l| l == "set hi-lg off"),
        "{}",
        s.calls()
    );
    assert_eq!(s.state()["last_result"]["verdict"], "allowed");

    // A refused one runs nothing.
    let r = s.post(
        "/api/actions",
        &cookie,
        &csrf,
        r#"{"action":"reply-volume","value":"30"}"#,
    );
    let id2 = r.json()["id"].as_str().unwrap().to_owned();
    s.wait_for("pending", || s.state()["pending"]["id"] == id2.as_str());
    s.answer(&id2, "refuse");
    s.wait_for("refused", || {
        s.state()["last_result"]["verdict"] == "refused"
    });
    assert!(!s.calls().contains("reply-volume"));

    // Microphone off runs without the TV (owner decision).
    let r = s.post(
        "/api/actions",
        &cookie,
        &csrf,
        r#"{"action":"mic","value":"off"}"#,
    );
    assert_eq!(r.status, 202);
    assert_eq!(r.json()["state"], "running");
    s.wait_for("auto result", || {
        s.state()["last_result"]["verdict"] == "auto"
    });
    assert!(s.calls().lines().any(|l| l == "mic off"));

    // ctl.sh inherited no socket (the listener and connections are CLOEXEC).
    for f in ["sockets.status", "sockets.set", "sockets.mic"] {
        assert_eq!(
            fs::read_to_string(s.root.join("prefix/bin").join(f))
                .unwrap()
                .trim(),
            "0",
            "{f}"
        );
    }

    // Done from the phone.
    let r = s.post("/api/session/end", &cookie, &csrf, "{}");
    assert_eq!(r.status, 200);
    assert!(s.wait_exit(Duration::from_secs(5)).success());
    let st = s.state();
    assert_eq!(st["state"], "ended");
    assert_eq!(st["reason"], "done");
    assert!(s.port_closed());
    let log = s.log();
    for secret in [&token, &cookie, &csrf] {
        assert!(!log.contains(secret.as_str()), "secret in the log:\n{log}");
        assert!(
            !fs::read_to_string(s.dir.join("state.json"))
                .unwrap()
                .contains(secret.as_str())
        );
    }
    assert!(!log.contains("/s/") && !log.contains("CANARY"), "{log}");
    assert!(log.contains("token consumed peer=127.0.0.1"), "{log}");
    assert!(log.contains("session end reason=done"), "{log}");
}

#[test]
fn hard_lifetime_ends_and_closes_the_port() {
    let mut s = Server::start(&["--lifetime-s", "2"]);
    assert!(s.wait_exit(Duration::from_secs(6)).success());
    assert_eq!(s.state()["reason"], "timeout");
    assert!(s.state().get("path").is_none());
    assert!(s.port_closed());
}

#[test]
fn lost_heartbeat_ends_the_session() {
    let mut s = Server::start_with(&["--heartbeat-grace-s", "2"], false);
    assert!(s.wait_exit(Duration::from_secs(6)).success());
    assert_eq!(s.state()["reason"], "app-gone");
}

#[test]
fn stop_is_immediate_even_with_a_slow_client() {
    let mut s = Server::start(&[]);
    // A client that sends half a request and waits.
    let mut slow = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
    slow.write_all(b"GET / HT").unwrap();
    thread::sleep(Duration::from_millis(200));
    let t = Instant::now();
    fs::write(s.dir.join("stop"), b"").unwrap();
    assert!(s.wait_exit(Duration::from_secs(3)).success());
    assert!(
        t.elapsed() < Duration::from_millis(1000),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(s.state()["reason"], "done");
    assert!(s.port_closed());
}

/// A TLS ClientHello (a phone browser trying `https://` first) and a
/// prefetch of the QR link neither end the session nor use the token; the
/// token's answer is a page whose own navigation carries the cookie.
#[test]
fn tls_hellos_and_prefetches_end_nothing() {
    let s = Server::start(&[]);
    let hello: &[u8] = &[
        0x16, 0x03, 0x01, 0x00, 0x2f, 0x01, 0x00, 0x00, 0x2b, 0x03, 0x03, 0, 0, 0, 0,
    ];
    let tls = |n: usize| {
        for _ in 0..n {
            let mut c = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            c.write_all(hello).unwrap();
            let mut out = Vec::new();
            let _ = c.read_to_end(&mut out);
            assert!(out.is_empty(), "no HTTP answer to TLS");
        }
    };
    tls(5);
    let path = s.state()["path"].as_str().unwrap().to_owned();
    for h in ["Sec-Purpose: prefetch;prerender", "Purpose: prefetch"] {
        let r =
            s.raw(format!("GET {path} HTTP/1.1\r\nHost: {}\r\n{h}\r\n\r\n", s.host()).as_bytes());
        assert_eq!(r.status, 503);
        assert!(r.header("set-cookie").is_none());
    }
    assert_eq!(s.state()["state"], "waiting");
    let r = s.raw(
        format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\nSec-Fetch-Site: none\r\nUser-Agent: Mozilla/5.0 (Linux; Android 15) Chrome/141.0 Mobile Safari/537.36\r\n\r\n",
            s.host()
        )
        .as_bytes(),
    );
    assert_eq!(r.status, 200);
    assert!(r.header("set-cookie").unwrap().contains("SameSite=Strict"));
    assert!(r.header("content-security-policy").is_some());
    let body = String::from_utf8_lossy(&r.body).into_owned();
    assert!(
        body.contains(r#"<meta http-equiv="refresh" content="0; url=/">"#),
        "{body}"
    );
    s.wait_for("active", || s.state()["state"] == "active");
    tls(25);
    // No cookie on `/`: refused, logged with the reason, not an end.
    assert_eq!(s.get("/", None).status, 403);
    assert_eq!(s.state()["state"], "active");
    let log = s.log();
    assert!(log.contains("rejected tls peer=127.0.0.1"), "{log}");
    assert!(
        log.contains("rejected prefetch token peer=127.0.0.1"),
        "{log}"
    );
    assert!(
        log.contains("token consumed peer=127.0.0.1 site=none browser=chrome"),
        "{log}"
    );
    assert!(
        log.contains("rejected no-cookie page peer=127.0.0.1 site=absent browser=absent"),
        "{log}"
    );
    assert!(
        !log.contains("/s/") && !log.contains(path.trim_start_matches("/s/")),
        "{log}"
    );
}

#[test]
fn three_parse_errors_before_the_token_end_it() {
    let mut s = Server::start(&[]);
    for (i, junk) in [
        &b"\x00\x01\x02\x03"[..],
        b"GET /\n\n",
        b"BREW / HTTP/1.0\r\n\r\n",
    ]
    .iter()
    .enumerate()
    {
        let r = s.raw(junk);
        assert!(r.status >= 400, "{i}");
        if i < 2 {
            assert!(s.child.try_wait().unwrap().is_none());
        }
    }
    assert!(s.wait_exit(Duration::from_secs(3)).success());
    assert_eq!(s.state()["reason"], "parse-error");
}

#[test]
fn a_parse_error_after_the_token_ends_it() {
    let mut s = Server::start(&[]);
    let _ = s.login();
    let r = s.raw(
        format!(
            "POST /api/actions HTTP/1.1\r\nHost: {}\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
            s.host()
        )
        .as_bytes(),
    );
    assert_eq!(r.status, 400);
    assert!(s.wait_exit(Duration::from_secs(3)).success());
    assert_eq!(s.state()["reason"], "parse-error");
}

#[test]
fn oversize_and_slow_requests() {
    let s = Server::start(&[]);
    let big = format!(
        "GET / HTTP/1.1\r\nHost: {}\r\nX: {}\r\n\r\n",
        s.host(),
        "a".repeat(5000)
    );
    assert_eq!(s.raw(big.as_bytes()).status, 431);
    let body = format!(
        "POST /api/actions HTTP/1.1\r\nHost: {}\r\nContent-Length: 2000\r\n\r\n",
        s.host()
    );
    assert_eq!(s.raw(body.as_bytes()).status, 413);
    // Slowloris: a partial head, then nothing; 408 after 3 s.
    let mut slow = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
    slow.write_all(b"GET / HTTP/1.1\r\nHost: x").unwrap();
    let t = Instant::now();
    let mut out = Vec::new();
    slow.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let _ = slow.read_to_end(&mut out);
    assert!(
        t.elapsed() >= Duration::from_millis(2500),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(Reply::parse(&out).status, 408);
}

#[test]
fn one_connection_at_a_time() {
    let s = Server::start(&[]);
    let mut first = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
    first.write_all(b"GET /api/state HTTP/1.1\r\n").unwrap();
    thread::sleep(Duration::from_millis(200));
    let host = s.host();
    let port = s.port;
    let t = Instant::now();
    let second = thread::spawn(move || {
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.write_all(format!("GET /api/state HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
            .unwrap();
        let mut out = Vec::new();
        c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let _ = c.read_to_end(&mut out);
        (Reply::parse(&out).status, t.elapsed())
    });
    thread::sleep(Duration::from_millis(700));
    first
        .write_all(format!("Host: {}\r\n\r\n", s.host()).as_bytes())
        .unwrap();
    let mut out = Vec::new();
    let _ = first.read_to_end(&mut out);
    assert_eq!(Reply::parse(&out).status, 403);
    let (status, waited) = second.join().unwrap();
    assert_eq!(status, 403);
    // The second connection was served only after the first one finished.
    assert!(waited >= Duration::from_millis(600), "{waited:?}");
    let _ = first.shutdown(Shutdown::Both);
}

#[test]
fn expiry_and_a_hung_command() {
    let mut s = Server::start(&[
        "--request-timeout-s",
        "1",
        "--command-timeout-s",
        "1",
        "--cooldown-s",
        "0",
    ]);
    let (cookie, csrf) = s.login();
    s.wait_for("the state mirror", || {
        !s.get("/api/state", Some(&cookie)).json()["device"].is_null()
    });
    let r = s.post(
        "/api/actions",
        &cookie,
        &csrf,
        r#"{"action":"remote-off","value":"keep"}"#,
    );
    let id = r.json()["id"].as_str().unwrap().to_owned();
    s.wait_for("expired", || {
        s.get(&format!("/api/actions/{id}"), Some(&cookie)).json()["state"] == "expired"
    });
    assert!(!s.calls().contains("remote-off"));
    // A command that hangs is killed at its limit and reported as failed.
    fs::write(s.root.join("prefix/bin/hang"), b"").unwrap();
    let r = s.post(
        "/api/actions",
        &cookie,
        &csrf,
        r#"{"action":"mic","value":"off"}"#,
    );
    let id = r.json()["id"].as_str().unwrap().to_owned();
    let t = Instant::now();
    s.wait_for("failed", || s.state()["last_result"]["id"] == id.as_str());
    assert!(t.elapsed() < Duration::from_secs(8), "{:?}", t.elapsed());
    assert_eq!(s.state()["last_result"]["result"], "failed");
    assert_eq!(s.state()["last_result"]["error"], "timeout");
    let _ = s.post("/api/session/end", &cookie, &csrf, "{}");
    assert!(s.wait_exit(Duration::from_secs(5)).success());
}

#[test]
fn twenty_bad_requests_end_the_session() {
    let mut s = Server::start(&[]);
    for _ in 0..20 {
        let r = s.get("/api/state", Some("AAAA"));
        assert!(r.status == 403 || r.status == 0);
        thread::sleep(Duration::from_millis(60));
    }
    assert!(s.wait_exit(Duration::from_secs(3)).success());
    assert_eq!(s.state()["reason"], "too-many-bad-requests");
}

fn source(rel: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn serves_the_phone_page_and_its_assets() {
    let s = Server::start(&[]);
    let (cookie, _csrf) = s.login();

    // The page: HTML, never cached, with the strict CSP, byte for byte the file.
    let page = s.get("/", Some(&cookie));
    assert_eq!(page.status, 200);
    assert_eq!(
        page.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(page.header("cache-control"), Some("no-store"));
    assert_eq!(page.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(page.header("x-frame-options"), Some("DENY"));
    let csp = page.header("content-security-policy").unwrap();
    assert!(csp.contains("script-src 'self'") && csp.contains("style-src 'self'"));
    assert!(!csp.contains("unsafe-inline") && !csp.contains("unsafe-eval"));
    assert_eq!(page.body, source("assets/phone.html"));
    let text = String::from_utf8(page.body.clone()).unwrap();

    // Every asset the page names is served from the binary, equal to its
    // source file; voicepower.js is the TV app's own file (one copy).
    let sources = [
        (
            "phone.js",
            "text/javascript; charset=utf-8",
            "assets/phone.js",
        ),
        ("phone.css", "text/css; charset=utf-8", "assets/phone.css"),
        (
            "voicepower.js",
            "text/javascript; charset=utf-8",
            "../../packaging/webos/app/voicepower.js",
        ),
    ];
    let mut total = page.body.len();
    for (name, ctype, file) in sources {
        assert!(
            text.contains(&format!("/assets/{name}")),
            "{name} not in the page"
        );
        let r = s.get(&format!("/assets/{name}"), Some(&cookie));
        assert_eq!(r.status, 200, "{name}");
        assert_eq!(r.header("content-type"), Some(ctype), "{name}");
        assert_eq!(r.header("cache-control"), Some("no-store"), "{name}");
        assert_eq!(
            r.header("x-content-type-options"),
            Some("nosniff"),
            "{name}"
        );
        assert!(r.header("content-security-policy").is_none(), "{name}");
        assert!(r.header("access-control-allow-origin").is_none(), "{name}");
        assert_eq!(r.body, source(file), "{name}");
        assert_eq!(
            r.header("content-length")
                .and_then(|v| v.parse::<usize>().ok()),
            Some(r.body.len())
        );
        assert!(r.body.len() < 128 * 1024, "{name}");
        total += r.body.len();
    }
    assert!(total <= 80 * 1024, "assets are {total} bytes");

    // Unknown assets and paths outside the table; assets need the session.
    assert_eq!(s.get("/assets/nope.js", Some(&cookie)).status, 404);
    assert_eq!(s.get("/assets/", Some(&cookie)).status, 404);
    assert_eq!(s.get("/assets/phone.js", None).status, 403);
    assert_eq!(s.get("/assets/voicepower.js", None).status, 403);
}

/// The page's pure logic and static checks run under node (no DOM): skipped,
/// with a message, when node is not installed.
#[test]
fn phone_page_logic_in_node() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/phone_logic.js");
    match Command::new("node").arg(&script).output() {
        Ok(out) => assert!(
            out.status.success(),
            "node tests/phone_logic.js failed:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        Err(e) => eprintln!("skipped: node is not available ({e})"),
    }
}
