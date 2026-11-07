//! The Magic Remote's own microphone through LG's `voiceinput` service, for
//! a hold-to-talk run (`[remote] mic_button = "ptt"`,
//! docs/design/remote-mic-button.md).
//!
//! One session per run:
//!
//! 1. `luna-send -i luna://com.webos.service.voiceinput/startStreaming
//!    {"deviceType":"remote","subscribe":true}` on a pseudo-terminal (the
//!    power watch's helper). On c3-a this answered `{"subscribed":true,
//!    "returnValue":true,"socketPath":"/tmp/voiceinput_remote1.soc"}` 29 ms
//!    after the press (EXP-022 B). The socket path is taken from the reply,
//!    never assumed. A refusal (`2003 audio data is already capturing` was
//!    seen once, cleared within 0.65 s) is retried after a short pause, a few
//!    times; then the session reports itself lost and the run keeps the TV
//!    microphone.
//! 2. A thread connects to the Unix socket and reads 16 kHz mono S16LE
//!    (31.6 kB/s measured, EXP-022 B), runs it through its own front-end
//!    (high-pass and a fixed gain, **no echo canceller**: see the design
//!    note) and sends 512-sample chunks.
//! 3. The stream does **not** end at the button release on webOS 8.3: it
//!    ends when the subscription does. [`RemoteMic::close`] kills the
//!    `luna-send` child (which ends the subscription, as in EXP-022 B4) and
//!    stops the thread; dropping the [`RemoteMic`] does the same.
//!
//! Every event carries the session's generation, so a late event of a
//! closed session is told apart. No audio is written anywhere; logs carry
//! sizes, codes and the socket path (it holds no identifier) only.

use std::io::Read as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use webos_dsp::{ChannelMode, Frontend, FrontendConfig};

use crate::capture::Chunker;
use crate::power::{self, json_bool, json_str};

/// The method (private group of `voiceinput`, declared on c3-a, D2).
pub(crate) const START_STREAMING: &str = "luna://com.webos.service.voiceinput/startStreaming";
/// The parameters that worked on c3-a (no `keyType`, no `id`; EXP-022 B).
pub(crate) const PAYLOAD: &str = r#"{"deviceType":"remote","subscribe":true}"#;
/// The stream's rate (16 kHz mono S16LE).
pub(crate) const RATE: u32 = 16_000;
/// A Unix socket path is at most this long (`sun_path` minus the NUL).
const MAX_SOCKET_PATH: usize = 107;
/// Read timeout of the socket, so the thread notices a close.
const READ_TIMEOUT: Duration = Duration::from_millis(200);

/// What a session reports (with its generation).
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum StreamEvent {
    /// The socket is connected; chunks follow.
    Opened,
    /// 512 samples of the remote microphone after the front-end.
    Chunk(Vec<i16>),
    /// The session could not be opened, or it ended or failed: the reason
    /// is for the log.
    Lost(String),
}

impl std::fmt::Debug for StreamEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Opened => f.write_str("Opened"),
            Self::Chunk(c) => write!(f, "Chunk(<{} samples>)", c.len()),
            Self::Lost(why) => f.debug_tuple("Lost").field(why).finish(),
        }
    }
}

/// How a session is opened.
#[derive(Debug, Clone)]
pub(crate) struct MicConfig {
    /// `luna-send`; `None` (not found, e.g. on a PC): every open is lost.
    pub(crate) luna: Option<PathBuf>,
    /// `startStreaming` calls before giving up.
    pub(crate) attempts: u32,
    /// Pause between two calls.
    pub(crate) retry: Duration,
    /// How long one call may take to answer.
    pub(crate) reply_wait: Duration,
    /// How long the socket may take to accept the connection.
    pub(crate) connect_wait: Duration,
    /// The front-end of the remote's audio.
    pub(crate) frontend: FrontendConfig,
}

impl MicConfig {
    /// The defaults: 3 calls 300 ms apart (the `2003` refusal of EXP-022
    /// cleared within 0.65 s), 1.5 s per answer, 1 s to connect; high-pass
    /// as the TV microphone's, `gain_db` of `[remote]`.
    pub(crate) fn new(luna: Option<PathBuf>, hpf_hz: f32, gain_db: f32) -> Self {
        Self {
            luna,
            attempts: 3,
            retry: Duration::from_millis(300),
            reply_wait: Duration::from_millis(1500),
            connect_wait: Duration::from_secs(1),
            frontend: FrontendConfig {
                channel: ChannelMode::Left,
                gain_db,
                hpf_hz,
            },
        }
    }
}

/// One line of `luna-send`'s output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reply {
    /// The stream is ready at this path.
    Socket(String),
    /// A refusal: `errorCode` and `errorText` (either may be empty).
    Refused(String, String),
    /// Anything else.
    Other,
}

pub(crate) fn parse_reply(line: &str) -> Reply {
    let line = line.trim();
    if !line.starts_with('{') {
        return Reply::Other;
    }
    if json_bool(line, "returnValue") == Some(false) {
        return Reply::Refused(
            json_str(line, "errorCode").unwrap_or_default(),
            json_str(line, "errorText").unwrap_or_default(),
        );
    }
    match json_str(line, "socketPath") {
        Some(p) => Reply::Socket(p),
        None => Reply::Other,
    }
}

/// A socket path from the reply is used only when it is absolute, short
/// enough, and has no `..` or control character.
pub(crate) fn usable_socket_path(p: &str) -> bool {
    p.starts_with('/')
        && p.len() <= MAX_SOCKET_PATH
        && !p.chars().any(char::is_control)
        && !Path::new(p).components().any(|c| c.as_os_str() == "..")
}

struct Session {
    stop: Arc<AtomicBool>,
    stop_tx: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

/// The remote microphone: at most one session at a time.
pub(crate) struct RemoteMic {
    cfg: MicConfig,
    tx: mpsc::UnboundedSender<(u64, StreamEvent)>,
    generation: u64,
    session: Option<Session>,
    /// Tasks of closed sessions that may still be killing and reaping their
    /// child; awaited by [`RemoteMic::shutdown`].
    closing: Vec<tokio::task::JoinHandle<()>>,
}

impl RemoteMic {
    pub(crate) fn new(cfg: MicConfig, tx: mpsc::UnboundedSender<(u64, StreamEvent)>) -> Self {
        Self {
            cfg,
            tx,
            generation: 0,
            session: None,
            closing: Vec::new(),
        }
    }

    /// The generation of the current (or last) session.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Opens a new session (closing one that is still there). Needs the
    /// tokio runtime.
    pub(crate) fn open(&mut self) {
        self.close();
        self.generation += 1;
        let stop = Arc::new(AtomicBool::new(false));
        let (stop_tx, stop_rx) = watch::channel(false);
        let task = tokio::spawn(session(
            self.cfg.clone(),
            self.generation,
            self.tx.clone(),
            Arc::clone(&stop),
            stop_rx,
        ));
        self.session = Some(Session {
            stop,
            stop_tx,
            task,
        });
    }

    /// Ends the session: the subscription child is killed and reaped by its
    /// task, the reader thread stops within [`READ_TIMEOUT`]. The task is
    /// kept until it has finished, for [`RemoteMic::shutdown`].
    pub(crate) fn close(&mut self) {
        self.closing.retain(|t| !t.is_finished());
        if let Some(s) = self.session.take() {
            s.stop.store(true, Ordering::Relaxed);
            let _ = s.stop_tx.send(true);
            self.closing.push(s.task);
        }
    }

    /// Closes the session and waits (3 s at most in all) until every
    /// session's child is killed and reaped, also of sessions closed before.
    pub(crate) async fn shutdown(&mut self) {
        self.close();
        let deadline = Instant::now() + Duration::from_secs(3);
        for mut task in std::mem::take(&mut self.closing) {
            if tokio::time::timeout_at(deadline, &mut task).await.is_err() {
                task.abort();
            }
        }
    }
}

impl Drop for RemoteMic {
    fn drop(&mut self) {
        self.close();
    }
}

/// One `startStreaming` call: the socket path, or why not. The child stays
/// alive (it holds the subscription) only on success.
async fn call(
    luna: &Path,
    wait: Duration,
    stop_rx: &mut watch::Receiver<bool>,
) -> Result<
    (
        tokio::process::Child,
        mpsc::UnboundedReceiver<String>,
        String,
    ),
    Option<String>,
> {
    let (mut child, mut lines) = power::start_child(luna, START_STREAMING, PAYLOAD)
        .map_err(|e| Some(format!("cannot start {}: {e}", luna.display())))?;
    let deadline = Instant::now() + wait;
    let outcome = loop {
        tokio::select! {
            biased;
            _ = stop_rx.changed() => break Err(None),
            line = lines.recv() => match line {
                None => break Err(Some("luna-send ended without an answer".to_owned())),
                Some(l) => match parse_reply(&l) {
                    Reply::Socket(p) if usable_socket_path(&p) => break Ok(p),
                    Reply::Socket(p) => break Err(Some(format!("unusable socket path ({} bytes)", p.len()))),
                    Reply::Refused(code, text) => break Err(Some(format!("refused: {code} {text}"))),
                    Reply::Other => {}
                },
            },
            () = tokio::time::sleep_until(deadline) => {
                break Err(Some(format!("no answer within {} ms", wait.as_millis())));
            }
        }
    };
    match outcome {
        Ok(p) => Ok((child, lines, p)),
        Err(e) => {
            let _ = child.kill().await;
            Err(e)
        }
    }
}

async fn session(
    cfg: MicConfig,
    generation: u64,
    tx: mpsc::UnboundedSender<(u64, StreamEvent)>,
    stop: Arc<AtomicBool>,
    mut stop_rx: watch::Receiver<bool>,
) {
    let report_lost = |why: String| {
        let _ = tx.send((generation, StreamEvent::Lost(why)));
    };
    let Some(luna) = cfg.luna.clone() else {
        report_lost("luna-send not found".to_owned());
        return;
    };
    let mut last_why = String::new();
    let mut got = None;
    for attempt in 1..=cfg.attempts.max(1) {
        if attempt > 1 {
            tokio::select! {
                biased;
                _ = stop_rx.changed() => return,
                () = tokio::time::sleep(cfg.retry) => {}
            }
        }
        match call(&luna, cfg.reply_wait, &mut stop_rx).await {
            Ok(ok) => {
                got = Some(ok);
                break;
            }
            Err(None) => return,
            Err(Some(why)) => {
                log::info!("remote mic: startStreaming attempt {attempt}: {why}");
                last_why = why;
            }
        }
    }
    let Some((mut child, mut lines, path)) = got else {
        report_lost(format!(
            "startStreaming failed {} times (last: {last_why})",
            cfg.attempts.max(1)
        ));
        return;
    };
    log::info!(
        "remote mic: subscribed (luna-send pid {}), stream at {path}",
        child.id().unwrap_or(0)
    );
    let reader = {
        let tx = tx.clone();
        let stop = Arc::clone(&stop);
        let fe = cfg.frontend;
        let wait = cfg.connect_wait;
        std::thread::Builder::new()
            .name("remote-mic".into())
            .spawn(move || read_socket(&PathBuf::from(path), wait, fe, generation, &tx, &stop))
    };
    if let Err(e) = reader {
        let _ = child.kill().await;
        report_lost(format!("cannot start the reader thread: {e}"));
        return;
    }
    loop {
        tokio::select! {
            biased;
            _ = stop_rx.changed() => break,
            line = lines.recv() => match line {
                None => {
                    if !stop.load(Ordering::Relaxed) {
                        report_lost("the luna-send subscription ended".to_owned());
                    }
                    break;
                }
                Some(l) => {
                    if let Reply::Refused(code, text) = parse_reply(&l) {
                        log::info!("remote mic: voiceinput reported {code} {text} during the stream");
                    } else {
                        log::debug!("remote mic: luna-send line ({} bytes)", l.len());
                    }
                }
            },
        }
    }
    stop.store(true, Ordering::Relaxed);
    // Ends the subscription, which ends LG's stream (EXP-022 B4).
    let _ = child.kill().await;
    log::info!("remote mic: subscription closed");
}

/// Converts S16LE bytes (with a byte left over from the last read) to the
/// front-end's S24-in-i32 samples.
fn to_s24(carry: &mut Option<u8>, bytes: &[u8], out: &mut Vec<i32>) {
    out.clear();
    let mut it = bytes.iter().copied();
    if let Some(lo) = carry.take() {
        let Some(hi) = it.next() else {
            *carry = Some(lo);
            return;
        };
        out.push(i32::from(i16::from_le_bytes([lo, hi])) << 8);
    }
    let rest: Vec<u8> = it.collect();
    let mut pairs = rest.chunks_exact(2);
    for p in &mut pairs {
        out.push(i32::from(i16::from_le_bytes([p[0], p[1]])) << 8);
    }
    if let [b] = pairs.remainder() {
        *carry = Some(*b);
    }
}

fn read_socket(
    path: &Path,
    connect_wait: Duration,
    fe: FrontendConfig,
    generation: u64,
    tx: &mpsc::UnboundedSender<(u64, StreamEvent)>,
    stop: &AtomicBool,
) {
    let send = |ev: StreamEvent| tx.send((generation, ev)).is_ok();
    let deadline = std::time::Instant::now() + connect_wait;
    let mut sock = loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match UnixStream::connect(path) {
            Ok(s) => break s,
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(30));
            }
            Err(e) => {
                send(StreamEvent::Lost(format!(
                    "cannot connect to the stream: {e}"
                )));
                return;
            }
        }
    };
    if let Err(e) = sock.set_read_timeout(Some(READ_TIMEOUT)) {
        send(StreamEvent::Lost(format!("socket: {e}")));
        return;
    }
    if !send(StreamEvent::Opened) {
        return;
    }
    let mut frontend = Frontend::new(fe, RATE);
    let mut chunker = Chunker::default();
    let mut carry = None;
    let mut buf = vec![0_u8; 4096];
    let mut samples = Vec::with_capacity(2048);
    let mut mono = Vec::with_capacity(2048);
    let mut total = 0_u64;
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match sock.read(&mut buf) {
            Ok(0) => {
                if !stop.load(Ordering::Relaxed) {
                    send(StreamEvent::Lost("the stream closed".to_owned()));
                }
                break;
            }
            Ok(n) => {
                total += n as u64;
                to_s24(&mut carry, &buf[..n], &mut samples);
                frontend.process(&samples, 1, &mut mono);
                for c in chunker.push(&mono) {
                    if !send(StreamEvent::Chunk(c)) {
                        return;
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => {
                if !stop.load(Ordering::Relaxed) {
                    send(StreamEvent::Lost(format!("read failed: {e}")));
                }
                break;
            }
        }
    }
    let st = frontend.take_stats();
    // Sizes and levels only, never audio.
    log::info!(
        "remote mic: stream done ({total} bytes, in rms {:.1} dBFS peak {:.1} dBFS)",
        st.in_rms_dbfs,
        st.in_peak_dbfs
    );
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use std::io::Write as _;
    use std::os::unix::net::UnixListener;

    use super::*;
    use crate::capture::CHUNK_SAMPLES;

    #[test]
    fn replies_are_read_as_on_c3_a() {
        assert_eq!(
            parse_reply(
                r#"{"subscribed":true,"returnValue":true,"socketPath":"/tmp/voiceinput_remote1.soc"}"#
            ),
            Reply::Socket("/tmp/voiceinput_remote1.soc".into())
        );
        assert_eq!(
            parse_reply(
                r#"{"errorCode":"2003","errorText":"audio data is already capturing","returnValue":false,"subscribed":true}"#
            ),
            Reply::Refused("2003".into(), "audio data is already capturing".into())
        );
        assert_eq!(
            parse_reply(r#"{"returnValue":false}"#),
            Reply::Refused(String::new(), String::new())
        );
        assert_eq!(parse_reply(r#"{"subscribed":true}"#), Reply::Other);
        assert_eq!(parse_reply("luna-send: noise"), Reply::Other);
    }

    #[test]
    fn only_plain_absolute_socket_paths_are_used() {
        assert!(usable_socket_path("/tmp/voiceinput_remote1.soc"));
        assert!(usable_socket_path("/var/voiceinput/voiceinput_remote1.soc"));
        for bad in [
            "tmp/x.soc",
            "",
            "/tmp/../etc/x",
            "/tmp/a\nb",
            &format!("/{}", "a".repeat(200)),
        ] {
            assert!(!usable_socket_path(bad), "{bad:?}");
        }
    }

    #[test]
    fn s16le_bytes_become_s24_samples_across_reads() {
        let mut carry = None;
        let mut out = Vec::new();
        // 1, -2 and the first byte of 0x0302.
        to_s24(&mut carry, &[1, 0, 0xfe, 0xff, 0x02], &mut out);
        assert_eq!(out, [1 << 8, -2 << 8]);
        assert_eq!(carry, Some(0x02));
        to_s24(&mut carry, &[0x03], &mut out);
        assert_eq!(out, [0x0302 << 8]);
        assert_eq!(carry, None);
        to_s24(&mut carry, &[], &mut out);
        assert!(out.is_empty());
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("webos-sat-rmic-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn cfg(luna: Option<PathBuf>) -> MicConfig {
        MicConfig {
            retry: Duration::from_millis(50),
            reply_wait: Duration::from_secs(5),
            ..MicConfig::new(luna, 0.0, 0.0)
        }
    }

    async fn next(rx: &mut mpsc::UnboundedReceiver<(u64, StreamEvent)>) -> (u64, StreamEvent) {
        tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap()
    }

    fn gone(pid: i32) -> bool {
        !Path::new(&format!("/proc/{pid}")).exists()
            || std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .unwrap_or_default()
                .contains(") Z ")
    }

    fn read_pid(p: &Path) -> i32 {
        std::fs::read_to_string(p).unwrap().trim().parse().unwrap()
    }

    /// A fake LG stream: a listener on `sock`; the fake `luna-send` writes
    /// its pid, refuses `refusals` times with 2003, then answers with the
    /// socket path and keeps the subscription (sleep).
    fn fake(dir: &Path, sock: &Path, refusals: u32) -> PathBuf {
        let luna = dir.join("luna-send");
        let count = dir.join("count");
        crate::testutil::write_script(
            &luna,
            &format!(
                r#"[ "$1" = -i ] && [ "$2" = "{START_STREAMING}" ] && [ "$3" = '{PAYLOAD}' ] || exit 3
n=$(cat '{c}' 2>/dev/null || echo 0); echo $((n + 1)) > '{c}'
if [ "$n" -lt {refusals} ]; then
  printf '{{"errorCode":"2003","errorText":"audio data is already capturing","returnValue":false,"subscribed":true}}\n'
  exec sleep 30
fi
echo $$ > '{p}'
printf '{{"subscribed":true,"returnValue":true,"socketPath":"{s}"}}\n'
exec sleep 30"#,
                c = count.display(),
                p = dir.join("pid").display(),
                s = sock.display(),
            ),
        );
        luna
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_session_retries_a_refusal_streams_chunks_and_close_ends_the_subscription() {
        let dir = tmp("ok");
        let sock = dir.join("remote1.soc");
        let listener = UnixListener::bind(&sock).unwrap();
        let luna = fake(&dir, &sock, 1);
        // The fake LG side: 0.25 s of a constant level, then it keeps the
        // connection open (the stream does not end by itself).
        let server = std::thread::spawn(move || {
            let (mut c, _) = listener.accept().unwrap();
            let pcm: Vec<u8> = std::iter::repeat_n(1000_i16.to_le_bytes(), 4000)
                .flatten()
                .collect();
            // In odd pieces, to cross sample boundaries.
            for piece in pcm.chunks(333) {
                c.write_all(piece).unwrap();
            }
            std::thread::sleep(Duration::from_secs(3));
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut mic = RemoteMic::new(cfg(Some(luna)), tx);
        mic.open();
        assert_eq!(mic.generation(), 1);
        assert_eq!(next(&mut rx).await, (1, StreamEvent::Opened));
        let mut samples = 0;
        while samples < 3 * CHUNK_SAMPLES {
            match next(&mut rx).await {
                (1, StreamEvent::Chunk(c)) => {
                    assert_eq!(c.len(), CHUNK_SAMPLES);
                    // No high-pass, 0 dB: the level passes unchanged.
                    assert!(c.iter().all(|&v| v == 1000), "{:?}", &c[..4]);
                    samples += c.len();
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("count")).unwrap().trim(),
            "2"
        );
        let pid = read_pid(&dir.join("pid"));
        assert!(!gone(pid));
        mic.shutdown().await;
        assert!(gone(pid), "the subscription child {pid} is still running");
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_after_close_still_waits_for_the_child() {
        let dir = tmp("closewait");
        let sock = dir.join("remote1.soc");
        let listener = UnixListener::bind(&sock).unwrap();
        let luna = fake(&dir, &sock, 0);
        let server = std::thread::spawn(move || {
            let (_c, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_secs(2));
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut mic = RemoteMic::new(cfg(Some(luna)), tx);
        mic.open();
        assert_eq!(next(&mut rx).await, (1, StreamEvent::Opened));
        let pid = read_pid(&dir.join("pid"));
        // The machine closes the stream (release, halt), then the daemon
        // shuts down: the child must be reaped when shutdown returns, with
        // no polling.
        mic.close();
        mic.shutdown().await;
        assert!(gone(pid), "the subscription child {pid} outlived shutdown");
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn refusals_every_time_report_the_session_lost() {
        let dir = tmp("refused");
        let luna = fake(&dir, &dir.join("none.soc"), 99);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut mic = RemoteMic::new(cfg(Some(luna)), tx);
        mic.open();
        match next(&mut rx).await {
            (1, StreamEvent::Lost(why)) => {
                assert!(
                    why.contains("failed 3 times") && why.contains("2003"),
                    "{why}"
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("count")).unwrap().trim(),
            "3"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn no_luna_send_or_no_socket_is_lost_at_once() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut mic = RemoteMic::new(cfg(None), tx);
        mic.open();
        assert_eq!(
            next(&mut rx).await,
            (1, StreamEvent::Lost("luna-send not found".into()))
        );
        // A reply whose socket nobody listens on.
        let dir = tmp("nosock");
        let luna = fake(&dir, &dir.join("none.soc"), 0);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut mic = RemoteMic::new(cfg(Some(luna)), tx);
        mic.open();
        match next(&mut rx).await {
            (1, StreamEvent::Lost(why)) => assert!(why.contains("cannot connect"), "{why}"),
            other => panic!("{other:?}"),
        }
        let pid = read_pid(&dir.join("pid"));
        mic.shutdown().await;
        assert!(gone(pid));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_end_of_the_stream_is_reported_and_a_new_open_is_a_new_generation() {
        let dir = tmp("eof");
        let sock = dir.join("remote1.soc");
        let listener = UnixListener::bind(&sock).unwrap();
        let luna = fake(&dir, &sock, 0);
        let server = std::thread::spawn(move || {
            let (mut c, _) = listener.accept().unwrap();
            c.write_all(&[0; 2048]).unwrap();
            // Closed: the stream ends.
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut mic = RemoteMic::new(cfg(Some(luna)), tx);
        mic.open();
        let mut lost = None;
        while lost.is_none() {
            if let (1, StreamEvent::Lost(why)) = next(&mut rx).await {
                lost = Some(why);
            }
        }
        assert_eq!(lost.as_deref(), Some("the stream closed"));
        server.join().unwrap();
        // The session that lost its stream still holds the subscription
        // until it is closed: a new open closes it (and its child).
        let pid = read_pid(&dir.join("pid"));
        assert!(!gone(pid));
        mic.open();
        assert_eq!(mic.generation(), 2);
        drop(mic);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !gone(pid) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(gone(pid));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
