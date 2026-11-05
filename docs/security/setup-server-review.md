# Security review: the phone setup server (`webos-satellite-setup`)

- **Status:** review done on the host, two Low findings fixed, the rest accepted or open (below). This is part of item 1 of the [release gate in phone setup, section 13.2](../design/phone-setup.md#132-security-review-gate-backlog-3-before-the-040-release). It does **not** complete the gate: the TV app's overlay was not reviewed, and nothing was run on a TV.
- **Date:** 2026-11-05. **Reviewer:** an independent agent review (not the author of the crate). The owner still has to read and sign off (gate item 6).
- **Threat model and intended guarantees:** [ADR 0009](../decisions/0009-phone-setup-web-server.md), [phone setup](../design/phone-setup.md) sections 4 to 9 and 13, and [install and configuration UX, section 3.2](../design/install-and-config-ux.md#32-option-b-embedded-web-config-server-plus-a-qr-code-on-the-tv). The review checked the code against these documents and did not assume they were met.
- **Evidence labels:** everything here is from reading the code and from host runs (`documented` for the repository code, host measurements as stated). Nothing here is `verified-on-target`.

## Scope

| Part | Version reviewed |
| --- | --- |
| `crates/setup-server` (binary `webos-satellite-setup`): `main.rs`, `http.rs`, `session.rs`, `actions.rs`, `worker.rs`, `mirror.rs`, `files.rs`, `secret.rs`, `log.rs`, `config.rs`, `assets.rs`, `build.rs`, `tests/server.rs` | commits `4bb7b00` (P4) and `6de54a1` (P6) |
| Phone page assets: `assets/phone.html`, `phone.js`, `phone.css`, `no-session.html`, `link-used.html`; the embedded `packaging/webos/app/voicepower.js`, scanned for DOM sinks only | `6de54a1` |
| `packaging/webos/ctl.sh` and `lib.sh`: `set` (including `set reply-volume`), `mic`, `pair`, `status`, and `setup` with its helpers | P3 as committed in `47b9288` (reviewed first as uncommitted work in the shared tree, then re-checked on the commit: the same code, plus a pid-only liveness check in the start loop). See [items for the P3 author](#ctlsh-items-for-the-p3-author) |

**Method:** a line-by-line reading of the crate and the shell code. Host runs:

- a parser fuzz run of 3,000,000 mutated requests: no panic (the committed test runs 20,000);
- a connection flood against a loopback server: 32 threads opening and closing connections for 5 s;
- a slow-reading client against a loopback server;
- `cargo audit` on the whole lock file;
- a license listing of the crate's dependency tree from `cargo metadata`.

## Verdict

No High or Medium finding. The HTTP subset, the token, cookie and CSRF handling, the action table and the confirmation state machine meet the design. Two Low findings are fixed with regression tests (SR-01, SR-02). The rest are accepted design trade-offs, or open items for the owner and for the on-TV probe.

**Supervised test on c3-a:** acceptable from the server's side, with items C3 to C5 below checked during the test. `ctl.sh setup` (P3) is on `main` since `47b9288`. **Update 2026-11-05 (0.4.0 candidate on c3-a):** C2, C7 and SR-03 are fixed; the overlay was reviewed (one hardening: OK ignored for 700 ms after it opens); C3, C4 and C5 and the token, environment, listener, file-mode and cleanup checks are **verified on the target** ([P4, 0.4.0](../discovery/c3-a/P4-permanent-install.md#update-to-040-2026-11-05-release-candidate-with-the-phone-setup-server)). **Release (0.4.0):** not yet. Still open: the owner's phone test (the overlay with the real remote, an action end to end, a phone reaching the TV) and the owner's sign-off. Not checked on the TV: an `nmap` scan from another host, the end conditions other than Done (10 minute limit, app gone, idle, parse error).

## Findings

Severity: High (root or TV control without the OK), Medium (a guarantee of the design broken in a realistic setting), Low (a weaker guarantee or a bounded denial of service), Info (noted, no change needed now).

| ID | Severity | Finding | Status |
| --- | --- | --- | --- |
| SR-01 | Low | The 3 s write limit applied to each `write` call, not to the whole response | **fixed** |
| SR-02 | Low | The session directory's safety depended on an unchecked parent (`/tmp` sticky) | **fixed** (fails closed) |
| SR-03 | Low | The state refresh runs `ctl.sh status --json` and `tv-settings --json` about every 4 to 15 s for the whole active session | **fixed** (2026-11-05: only while `/api/state` is polled, at least 5 s apart) |
| SR-04 | Low | Any LAN host can end a session or keep the one connection slot busy | accepted (design) |
| SR-05 | Info | `answer.json` is read, then unlinked: a second answer in between would be lost | accepted |
| SR-06 | Info | The log is opened following symbolic links, and a pre-existing file's mode is not tightened | accepted |
| SR-07 | Info | Cookies are not port-scoped; `SameSite` treats other ports on the TV as the same site | accepted |
| SR-08 | Info | Local processes on the TV pass the peer filter through the TV's own LAN address | accepted |
| SR-09 | Info | "Microphone off" runs without the OK on the TV | accepted (owner decision) |
| SR-10 | Info | An action still running at the end of a session is not killed; a worker panic would leave it `running` | accepted |

### SR-01: the write limit was per call (Low, fixed)

`connection()` set `SO_SNDTIMEO` to 3 s and called `write_all`. The timeout limits each `write` call. A call that makes some progress returns, and `write_all` starts another one.

A client that reads a few bytes now and then could therefore stretch one response over many 3 s periods. The server has one thread for HTTP: while it waits, it checks neither the TV's stop file nor the lifetime, the heartbeat or the answer file.

- **Bound:** responses larger than the socket buffer need a session cookie (the page and the assets, up to 80 KiB). Error responses are under 1 KiB and never block.
- **Host run:** on loopback the buffers absorb a whole asset, so the run could not show the stall. Over Wi-Fi with a 16 KiB send buffer, a few such periods are plausible.
- **Fix:** `http::write_response` writes against one deadline for the whole response (3 s), and lowers the socket timeout to the time left before each call.
- **Test:** `http::tests::a_slow_reader_cannot_hold_the_loop_past_the_write_deadline`. A fake client takes 16 bytes per call. The old per-call limit would have taken 4096 calls; the response now ends at the 150 ms deadline.

### SR-02: an unchecked parent of the session directory (Low, fixed)

`Dir::prepare` refused a symbolic link or a directory of another owner, then called `chmod` on the path (which follows links). Every later file operation also goes by path.

That is safe only if no other user can rename entries in the parent directory, which is true when `/tmp` is mode 1777 (sticky). That mode is the Linux default but is **not recorded for c3-a**: D0 lists `/tmp` as a tmpfs, without its mode.

On a non-sticky, world-writable parent, a non-root process on the TV could swap the directory. The root server would then:

- change the mode of a target of the attacker's choice to 0700 (between the check and the `chmod`);
- write `state.json`, with the one-time token, into the attacker's directory.

**Fix:** `files::ancestors_safe`. Before anything is created, every directory above the session directory (after resolving links) must be owned by root or by the server's user. If group or others can write to it, it must have the sticky bit. Otherwise the server refuses to start ("… is owned by another user, or writable by others without the sticky bit").

**Test:** `files::tests::a_parent_writable_by_others_without_sticky_is_refused`. Modes 777, 775 and 757 are refused and nothing is created; 1777, 755 and 700 are accepted. A parent reached through a link is checked at its real path, and another owner is refused.

The check is stricter than before. If the TV's `/tmp` is not 1777, the server will not start (item C4).

### SR-03: the refresh cost during a session (Low, fixed)

The worker refreshes the status mirror whenever the cache is 3 s old. A refresh is `ctl.sh status --json`, about 950 ms and 0.57 CPU-seconds per call on c3-a according to [EXP-012](../experiments/EXP-012-phone-setup-probe.md), plus `ctl.sh tv-settings --json`, which makes luna calls and was not measured.

- **Cost:** with a 3 s pause after each refresh, that is at least 0.57 CPU-seconds every 4 to 15 s, or about 4 to 15 % of a core, plus the luna cost. It lasts for the whole session, up to 10 minutes.
- **Hidden page:** the refresh also continues while the phone page is hidden. The page stops polling then, so the session ends only at the 180 s idle limit.
- **Not an amplifier:** a phone cannot raise the rate. Actions are serialized and a refresh follows each one.
- **Recommendation:** refresh only while `/api/state` was asked for in the last 10 s or so, and measure the session cost in EXP-012 Part B.
- **Fix (owner request, 2026-11-05):** the refresh runs only while a phone has a session and `/api/state` was asked for in the last 10 s (the use of the token counts), at least 5 s after the previous refresh ended (2 s after an action). Tests: `session::tests::no_refresh_runs_while_the_page_is_not_polling`, `refreshing_stops_ten_seconds_after_the_last_poll_and_keeps_its_interval`, `the_refresh_after_an_action_waits_for_the_floor`. The cost of an idle session on c3-a is measured in EXP-012 Part B.

### SR-04: denial of service by any LAN host (Low, accepted)

These are availability problems only, and part of the design's choices.

**Ending a session.** Any host on a private address can:

- end the session after the token is used, with one malformed request (the owner's rule for open question 2);
- end it before the token is used, with three malformed requests;
- end it at any time, with 20 unauthenticated requests.

**Blocking the phone.**

- A connection that sends nothing holds the only connection slot for 3 s. It counts neither as a parse error nor as a bad request, because a browser's speculative connection looks the same.
- The limit of 20 requests per second and the cap of 2000 requests per session are shared by all peers.

**CPU (measured on the host).** A connect-and-close flood from 32 threads for 5 s got 26 to 86 connections through. The kernel backlog of 4 and the 100 ms accept tick throttle it, and the server used no measurable CPU (0 clock ticks), so a flood is not a CPU attack on the TV.

**Recovery:** the owner restarts the session on the TV.

### SR-05 to SR-10 (Info)

- **SR-05:** `take_answer` reads `answer.json` and then unlinks it. An answer moved into place between the read and the unlink is deleted unread. That needs two answers within microseconds. The lost answer then expires, which counts as a refusal (fail-safe). If the code is touched again, rename the file to a private name first, then read it.
- **SR-06:** `Log::open` uses `O_APPEND` without `O_NOFOLLOW`, and keeps the mode of an existing file. `install.sh` makes `PREFIX` and `PREFIX/log` mode 0700, and `ctl.sh setup start` creates the file under `umask 077`. Only root could plant a link, and root is already the trust boundary.
- **SR-07:** the browser sends `sat_session` to every port on the TV's address. `SameSite=Strict` counts pages on other TV ports as the same site. The server requires `Origin` to equal its own origin exactly, `Sec-Fetch-Site` to be `same-origin` or `none`, and `Host` to equal the socket address. A page on another port therefore cannot use the cookie, and script cannot read it (`HttpOnly`). Another TV service would see the cookie only if the phone browsed to it, and could use it only from the phone's address.
- **SR-08:** a process on the TV that connects to the TV's LAN address arrives with a private peer address. It still needs the token, which is in a root-only 0700 directory, or the cookie. This is the same boundary as the threat table's "local app" row.
- **SR-09:** `Action::needs_confirmation` is false only for `mic off`. That action also skips the cooldown and the `503 app_gone` check. A session holder can therefore mute the satellite without the OK; the TV app shows a notice. This is the owner's decision of 2026-11-05 (the safe direction).
- **SR-10:** when a session ends while an approved command runs, the command keeps running in its own process group after the server exits. The command was approved, and `ctl.sh` serializes through its lock. A panic in the worker thread (none found) would leave `worker_busy` set, and the action would show `running` until the 10-minute limit.

## Checked and found sound

**HTTP parsing**

- Framing:
  - one request per connection, with `Connection: close` on every response;
  - `HTTP/1.1` only;
  - no `Transfer-Encoding`, `Expect`, `Upgrade` or `HTTP2-Settings`;
  - exactly one `Content-Length` (digits only, no leading zero, at most 5 digits, at most 1024), and no body except on `POST`;
  - extra bytes after the request are a parse error, so there is no smuggling or desync path.
- Bytes: CRLF only; no bare CR or LF; only visible ASCII, space and tab in the head.
- Limits: a head of at most 4 KiB with at most 24 headers; a target of at most 128 characters from `[A-Za-z0-9-._~/]`, with no query and no percent-encoding.
- No repeats of `Host`, `Cookie`, `Origin`, `Sec-Fetch-Site`, `X-Setup-CSRF`, `Content-Type` or `Content-Length`.
- Reads stop at 5 KiB and at a 3 s deadline, and check for a stop every 100 ms.
- No `unwrap` and no unchecked index on input (3,000,000 fuzz inputs on the host); response header values are filtered for control characters.

**Authentication**

- Token: 32 bytes from `/dev/urandom` (fails closed), base64url. It is compared in constant time, consumed only by `GET`, followed by `302` to `/` so that it leaves the address bar, `410` after use, and `404` when wrong.
- Cookie: 32 bytes, made by the server at start, so there is no session fixation. `HttpOnly; SameSite=Strict; Path=/`, with `Max-Age` set to the time left; constant-time compare; bound to the peer address; a duplicate `sat_session` is refused.
- CSRF and cross-origin:
  - a 16-byte CSRF token, `X-Setup-CSRF` and `application/json` on every `POST`;
  - `Origin` and `Sec-Fetch-Site` checked on every request;
  - DNS rebinding is blocked by the `Host` check;
  - `OPTIONS` gets `405`, and no `Access-Control-*` header is ever sent.
- Brute force: 20 bad requests end the session.

**Authorization**

- Every route is in a fixed table, and an asset is found by exact name, with no filesystem serving.
- Actions are an enum parsed with `deny_unknown_fields` (objects only). The argv is built from enumerated words and one validated integer (0 to 100), so no request byte reaches a command.
- Children run as follows:
  - `env_clear` plus a fixed `PATH` and an allow-list of five `WEBOS_SAT_*` variables;
  - working directory `/`, stdin `/dev/null`, stderr discarded;
  - stdout capped at 64 KiB;
  - in their own process group, which is killed at 25 s;
  - every descriptor of the server is close-on-exec (the host test checks this).

**TV confirmation**

- An answer applies only to the current pending request with a matching id, and only while it waits for the OK.
- Ids are 4 random bytes; there is one pending request at a time, a 60 s expiry, and a 3 s cooldown after a refusal or an expiry. Three strikes end the session, and the app must have sent a heartbeat within 8 s.
- `answer.json` is at most 256 bytes and has exactly the `id` and `verdict` keys; stale answers are removed at start and at the end.

**Files**

- The session directory is created 0700, and an existing one is refused if it is a link or has another owner.
- `state.json` is written through a 0600 `O_EXCL` temporary file and a `rename`.
- The token is in `state.json` only while the state is `waiting`, and in no argv, environment or log.

**Information leaks**

- Log lines are fixed words, IP literals and sanitized one-line errors (printable ASCII, at most 160 characters). Request paths, headers and bodies are never logged.
- `Debug` of a request hides the target, the cookie and the CSRF token.
- `/api/state` copies an allow-list field by field: no prefix, name, log or firmware. The test canaries cover this.
- `device.ha` (added after P3 at the coordinator's request) carries only:
  - `state`, one of `connected`, `disconnected`, `unpaired` or `unknown`, else null;
  - `since_s`, a whole number of at most ten years, else null.

  No address, name or version is copied. Test: `mirror::tests::ha_connection_is_allow_listed`.
- Responses carry `Referrer-Policy: no-referrer`, and the page also has a meta tag.

**Phone page**

- Text is set only with `textContent`; there is no `innerHTML` or `eval` in `phone.js` or `voicepower.js`.
- No inline script or style, and no remote assets. The CSP has `default-src 'none'`, `frame-ancestors 'none'` and `form-action 'none'`, plus `X-Frame-Options: DENY`.
- The Home Assistant link has `rel="noopener noreferrer"`.

**Lifetime**

- A hard 600 s limit on the monotonic clock, 180 s idle, and 15 s without a heartbeat (the heartbeat is a change of the file's mtime).
- The stop file is checked every 100 ms, also during a read.
- The listener is closed before the `ended` state is written.

**Dependencies**

- `cargo audit` (2026-11-05, 1403 advisories, whole lock file): no advisory.
- The crate's tree is serde, serde_json, rustix, bitflags, itoa, memchr, linux-raw-sys, zmij and the proc-macro crates. Every license is `MIT OR Apache-2.0`, `MIT`, `Unlicense OR MIT`, `Apache-2.0 WITH LLVM-exception` as an alternative, or `Unicode-3.0`.

## ctl.sh items for the P3 author

`ctl.sh` was not changed by this review. Items C2 to C5 should be settled before a test on the TV.

- **C1 (resolved):** when the review started, `main` had neither `setup` nor `set reply-volume`, so no session could start. P3 landed in `47b9288`.
- **C2 (fixed 2026-11-05):** `setup start` now runs the server with `env -i` and only `PATH`, `WEBOS_SAT_PREFIX` and `WEBOS_SAT_SETUP_DIR`; tested with hostile variables in the caller's environment. Was: `setup start` passes its whole inherited environment to the server; its comment says only the session and install directories. The server forwards only its allow-list to its children, but `WEBOS_SAT_LUNA_SEND`, `WEBOS_SAT_INITD`, `WEBOS_SAT_LOCK`, `WEBOS_SAT_ALLOW_NONROOT` and `WEBOS_SAT_SETTLE` from the exec environment would still reach the root commands. The trust is equal today, because the exec caller is root. Still, start the server with `env -i PATH=… WEBOS_SAT_SETUP_DIR=… WEBOS_SAT_PREFIX=…` and have the tests add their variables explicitly, so that the comment holds.
- **C3 (verified on c3-a 2026-11-05):** `setup.pid` is the server's own pid (`ppid` 1, own session; `setsid` did not fork). The concern was: `setup.pid` is the `$!` of `$SETSID nohup …`. `setsid` forks when its caller leads a process group (util-linux and BusyBox). `setup.pid` would then name a process that exits at once: `setup start` would report a failure, and `stop_setup_server` would remove the directory, which ends the server. That fails safe, but would make the feature unusable. Verify on c3-a that `/proc/$(cat setup.pid)/cmdline` is the server itself.
- **C4 (verified on c3-a 2026-11-05: `1777 root`):** `/tmp` must be 1777 and owned by root on the TV: `setup start` (`rm -rf` then `mkdir -m 700`) and the server (SR-02, now enforced) rely on it. Record `stat -c '%a %U' /tmp` in EXP-012 Part B.
- **C5 (verified on c3-a 2026-11-05: the mtime moves on every call, 110 polls, no early end):** the heartbeat is `: >heartbeat` on an empty file. Linux updates the mtime on an `O_TRUNC` open, but the server's host tests use `touch` and `set_modified`. Check on c3-a that two `setup status` calls 1 s apart change the mtime; otherwise every session ends as `app-gone` after 15 s.
- **C6:** `setup start` in the moment between the server writing `ended` and its exit fails with "already active". Benign: the app can retry.
- **C7 (pinned 2026-11-05):** `packaging/webos/tests/no_self_stop.py`, run by `tests/setup.sh`, follows every function that `set`, `mic`, `pair`, `status` and `tv-settings` call and fails if one reaches `stop_daemon`, `stop_setup_server` or the stop, start, install, uninstall or purge code (with positive controls). The invariant: keep the invariant that none of the commands the server runs (`status --json`, `tv-settings --json`, `set …`, `mic`, `pair`) reaches `stop_daemon` or `stop_setup_server`; otherwise the server would end itself in the middle of an action. That is true today, and a packaging test could pin it.
- **Checked and fine:**
  - `setup answer` validates the id (eight lowercase hex characters) and the verdict before `need_root`, and writes atomically under `umask 077`;
  - `set reply-volume` accepts exactly `0|[1-9]|[1-9][0-9]|100`, like the server;
  - the server is started without the install lock variable (its children take the lock themselves), with descriptors closed, stdin from `/dev/null`, and stdout and stderr appended to `setup.log` (the server prints no secret);
  - `setup status` needs no root and reads `state.json` only.

## Residual risks

- **Plain HTTP:**
  - a passive sniffer on the LAN sees the cookie, the CSRF token and the status mirror;
  - an attacker on the same segment who also spoofs the phone's IP can send requests;
  - every request except "microphone off" still needs the OK on the TV, which shows the peer and the app's own action text.
- **First scanner wins:** whoever scans the QR code first owns the session. The TV shows the peer, and the owner can end it.
- **Everything depends on the overlay:** for writes, the only control left is the TV app's overlay and its allow-list of `setup answer`, which this review did not cover.
- **A root process on the LAN:** for up to 10 minutes, a memory-safety bug in the dependencies would be root code execution. The crate itself has `forbid(unsafe_code)`, but `rustix` and `serde_json` contain `unsafe`.

## Not reviewed

- The TV app (`packaging/webos/app/app.js` and others): **reviewed on 2026-11-05 on the host** (never allows by itself; Back and the 60 s end refuse; unknown, malformed or non-IPv4 requests are refused without asking; text only from app tables keyed by action and value; the id is `^[0-9a-f]{8}$`; `ctl` accepts only `setup answer <8 hex> allow|refuse`; one change, the 700 ms OK guard, see [phone setup 11.1](../design/phone-setup.md#111-as-built-in-p5-and-p7-tv-app-2026-11-05)). Not run on a TV or with the real remote.
- On the TV (gate item 4):
  - `nmap` while idle, while waiting and after each end;
  - `ps` and `/proc/*/cmdline` for the token;
  - the file modes;
  - the log content;
  - items C3 to C5.
- Real phone browsers: whether `Sec-Fetch-Site` is `none` when a QR scanner app or a messenger opens the link, and in-app web views that block cookies.
- The daemon's reply-volume reload (P2) and the packaging tests (`packaging/webos/tests/setup.sh`).
- `cargo deny` with a configuration file: licenses were listed from `cargo metadata` instead.
- A sanitizer or coverage-guided fuzzing: only the deterministic mutation corpus was run.
