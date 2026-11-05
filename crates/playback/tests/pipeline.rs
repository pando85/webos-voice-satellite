//! Offline tests: synthetic audio, a loopback HTTP server and a fake `pacat`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    missing_docs
)]

use std::f64::consts::PI;
use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use webos_playback::{
    FetchConfig, NullSink, PacatSink, Pcm, PlaybackError, Sink, WavFileSink, decode, fetch,
    mix_channels, play_url, resample, wav_bytes,
};

fn sine(rate: u32, channels: u16, hz: f64, secs: f64) -> Pcm {
    let frames = (f64::from(rate) * secs) as usize;
    let mut samples = Vec::with_capacity(frames * usize::from(channels));
    for n in 0..frames {
        let v = (0.5 * (2.0 * PI * hz * n as f64 / f64::from(rate)).sin() * 32767.0) as i16;
        for _ in 0..channels {
            samples.push(v);
        }
    }
    Pcm {
        rate,
        channels,
        samples,
    }
}

fn rms(s: &[i16]) -> f64 {
    (s.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / s.len() as f64).sqrt()
}

fn zero_crossings(s: &[i16]) -> usize {
    s.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count()
}

fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("webos-playback-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Serves one canned HTTP response per connection forever; returns the base URL.
async fn serve(status: &str, ctype: &str, body: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let head = head.clone();
            let body = body.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&body).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    format!("http://{addr}/api/tts_proxy/test")
}

// ------------------------------------------------------------------ decode

#[test]
fn decode_synthetic_wav() {
    let src = sine(22_050, 1, 440.0, 0.5);
    let out = decode(&wav_bytes(&src), Some("wav")).unwrap();
    assert_eq!(out.rate, 22_050);
    assert_eq!(out.channels, 1);
    assert_eq!(out.samples.len(), src.samples.len());
    let max_err = src
        .samples
        .iter()
        .zip(&out.samples)
        .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
        .max()
        .unwrap();
    assert!(max_err <= 1, "wav decode should be lossless, err {max_err}");
}

#[test]
fn decode_stereo_wav_without_hint() {
    let src = sine(44_100, 2, 1000.0, 0.1);
    let out = decode(&wav_bytes(&src), None).unwrap();
    assert_eq!((out.rate, out.channels), (44_100, 2));
    assert_eq!(out.frames(), src.frames());
}

#[test]
fn decode_rejects_non_audio() {
    let err = decode(b"<html>not audio, just a login page</html>", Some("mp3")).unwrap_err();
    assert!(matches!(err, PlaybackError::Decode(_)), "{err:?}");
    assert!(matches!(
        decode(&[], None).unwrap_err(),
        PlaybackError::Decode(_)
    ));
}

#[test]
fn decode_truncated_wav_keeps_what_it_has_or_errors_cleanly() {
    let mut b = wav_bytes(&sine(16_000, 1, 300.0, 0.5));
    b.truncate(44 + 1000);
    // Either a shorter buffer or a Decode error is fine; it must not panic.
    if let Ok(p) = decode(&b, None) {
        assert!(p.samples.len() <= 500);
    }
}

// ---------------------------------------------------------------- resample

#[test]
fn resample_length_and_rate() {
    for (from, to) in [
        (16_000, 44_100),
        (24_000, 48_000),
        (44_100, 16_000),
        (22_050, 44_100),
    ] {
        let src = sine(from, 1, 440.0, 1.0);
        let out = resample(&src, to, 1);
        assert_eq!(out.rate, to);
        assert_eq!(out.channels, 1);
        let diff = out.frames().abs_diff(to as usize);
        assert!(diff <= 1, "{from}->{to}: {} frames", out.frames());
        assert!((out.duration().as_secs_f64() - 1.0).abs() < 0.001);
    }
}

#[test]
fn resample_preserves_tone_and_level() {
    let src = sine(16_000, 1, 1000.0, 1.0);
    let out = resample(&src, 44_100, 1);
    // Ignore the edges, where the kernel is truncated.
    let mid = &out.samples[2000..out.samples.len() - 2000];
    let ratio = rms(mid) / rms(&src.samples);
    assert!((0.98..1.02).contains(&ratio), "level ratio {ratio}");
    // 1 kHz over ~0.9 s has about 2 zero crossings per period.
    let secs = mid.len() as f64 / 44_100.0;
    let hz = zero_crossings(mid) as f64 / 2.0 / secs;
    assert!((hz - 1000.0).abs() < 10.0, "tone moved to {hz} Hz");
}

#[test]
fn downsample_attenuates_above_new_nyquist() {
    // 15 kHz at 44.1 kHz cannot exist at 16 kHz (Nyquist 8 kHz): must be removed.
    let src = sine(44_100, 1, 15_000.0, 0.5);
    let out = resample(&src, 16_000, 1);
    let mid = &out.samples[500..out.samples.len() - 500];
    assert!(
        rms(mid) < rms(&src.samples) * 0.02,
        "aliasing rms {}",
        rms(mid)
    );
}

#[test]
fn resample_same_rate_is_identity() {
    let src = sine(44_100, 2, 500.0, 0.1);
    assert_eq!(resample(&src, 44_100, 2), src);
}

#[test]
fn channel_mixing() {
    let mono = Pcm {
        rate: 8000,
        channels: 1,
        samples: vec![1, 2, 3],
    };
    let st = mix_channels(&mono, 2);
    assert_eq!(st.samples, vec![1, 1, 2, 2, 3, 3]);
    let back = mix_channels(&st, 1);
    assert_eq!(back.samples, mono.samples);
    let stereo = Pcm {
        rate: 8000,
        channels: 2,
        samples: vec![100, -100, 200, 0],
    };
    assert_eq!(mix_channels(&stereo, 1).samples, vec![0, 100]);
    // Combined with a rate change: mono 16k -> stereo 48k.
    let out = resample(&sine(16_000, 1, 440.0, 0.2), 48_000, 2);
    assert_eq!(out.channels, 2);
    assert_eq!(out.samples.len() % 2, 0);
    assert!(out.samples.chunks(2).all(|f| f[0] == f[1]));
}

#[test]
fn silence_stays_silent_after_resample() {
    let out = resample(&Pcm::silence(22_050, 1, 22_050), 44_100, 2);
    assert!(out.is_silent());
    assert_eq!(out.channels, 2);
}

// ------------------------------------------------------------------- fetch

#[tokio::test]
async fn fetch_serves_wav_and_play_url_writes_wav() {
    let src = sine(22_050, 1, 440.0, 0.5);
    let url = serve("200 OK", "audio/wav", wav_bytes(&src)).await;
    let body = fetch(&url, &local()).await.unwrap();
    assert_eq!(body, wav_bytes(&src));

    let dir = tmp_dir("wavsink");
    let out_path = dir.join("out.wav");
    let mut sink = WavFileSink::new(&out_path);
    let t = play_url(&url, &local(), &mut sink).await.unwrap();
    assert!((t.audio.as_secs_f64() - 0.5).abs() < 0.01);
    let written = decode(&std::fs::read(&out_path).unwrap(), Some("wav")).unwrap();
    assert_eq!((written.rate, written.channels), (44_100, 2));
    assert!(written.frames().abs_diff(22_050) <= 1);
    std::fs::remove_dir_all(dir).unwrap();
}

/// The test servers listen on 127.0.0.1, which the fetch guard refuses.
fn local() -> FetchConfig {
    FetchConfig {
        insecure_allow_loopback: true,
        ..FetchConfig::default()
    }
}

#[tokio::test]
async fn fetch_errors() {
    let cfg = local();
    assert!(matches!(
        fetch("not a url", &cfg).await.unwrap_err(),
        PlaybackError::BadUrl(_)
    ));
    assert!(matches!(
        fetch("file:///etc/passwd", &cfg).await.unwrap_err(),
        PlaybackError::BadUrl(_)
    ));
    assert!(matches!(
        fetch("http://", &cfg).await.unwrap_err(),
        PlaybackError::BadUrl(_) | PlaybackError::Http(_)
    ));

    let nf = serve("404 Not Found", "text/plain", b"nope".to_vec()).await;
    assert!(matches!(
        fetch(&nf, &cfg).await.unwrap_err(),
        PlaybackError::Status(404)
    ));

    let big = serve("200 OK", "audio/wav", vec![0u8; 5000]).await;
    let small = FetchConfig {
        max_bytes: 1000,
        ..local()
    };
    assert!(matches!(
        fetch(&big, &small).await.unwrap_err(),
        PlaybackError::TooLarge { limit: 1000 }
    ));
}

#[tokio::test]
async fn fetch_connection_refused_is_http_error() {
    // Bind then drop to get a port nobody listens on.
    let port = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let err = fetch(&format!("http://127.0.0.1:{port}/x"), &local())
        .await
        .unwrap_err();
    assert!(matches!(err, PlaybackError::Http(_)), "{err:?}");
}

#[tokio::test]
async fn fetch_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (_s, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    let cfg = FetchConfig {
        timeout: Duration::from_millis(300),
        ..local()
    };
    let err = fetch(&format!("http://{addr}/slow"), &cfg)
        .await
        .unwrap_err();
    assert!(matches!(err, PlaybackError::Timeout), "{err:?}");
}

#[tokio::test]
async fn play_url_non_audio_body_is_decode_error() {
    let url = serve("200 OK", "text/html", b"<html>hello</html>".to_vec()).await;
    let mut sink = NullSink::default();
    let err = play_url(&url, &local(), &mut sink).await.unwrap_err();
    assert!(matches!(err, PlaybackError::Decode(_)), "{err:?}");
    assert_eq!(sink.plays, 0);
}

#[tokio::test]
async fn bad_ca_file_is_rejected() {
    let dir = tmp_dir("ca");
    let ca = dir.join("ca.pem");
    std::fs::write(&ca, b"this is not a certificate").unwrap();
    let cfg = FetchConfig {
        ca_file: Some(ca),
        ..FetchConfig::default()
    };
    let err = fetch("http://127.0.0.1:1/x", &cfg).await.unwrap_err();
    assert!(matches!(err, PlaybackError::Tls(_)), "{err:?}");
    std::fs::remove_dir_all(dir).unwrap();
}

// -------------------------------------------------------------- PacatSink

fn fake_pacat(dir: &std::path::Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join("pacat");
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

#[test]
fn pacat_args() {
    let sink = pacat_with(|s| {
        s.volume = Some(0);
        s.device = Some("some_sink".into());
        s.extra_args = vec!["--latency-msec=50".into()];
    });
    let a = sink.args();
    for want in [
        "--playback",
        "--raw",
        "--format=s16le",
        "--rate=44100",
        "--channels=2",
        "--volume=0",
        "--device=some_sink",
        "--latency-msec=50",
    ] {
        assert!(a.iter().any(|x| x == want), "missing {want} in {a:?}");
    }
    assert!(
        !PacatSink::new()
            .args()
            .iter()
            .any(|x| x.starts_with("--device"))
    );
    assert!(
        !PacatSink::new()
            .args()
            .iter()
            .any(|x| x.starts_with("--volume"))
    );
}

#[tokio::test]
async fn pacat_sink_streams_pcm_to_child() {
    let dir = tmp_dir("pacat-ok");
    let out = dir.join("captured.raw");
    let args_file = dir.join("args.txt");
    let bin = fake_pacat(
        &dir,
        &format!(
            "echo \"$@\" > {}\ncat > {}",
            args_file.display(),
            out.display()
        ),
    );
    let mut sink = pacat_with(|s| {
        s.binary = bin;
        s.volume = Some(0);
    });
    let pcm = sine(22_050, 1, 440.0, 0.25);
    sink.play(&pcm).await.unwrap();

    let raw = std::fs::read(&out).unwrap();
    let expect = resample(&pcm, 44_100, 2);
    assert_eq!(raw, expect.to_s16le_bytes());
    let args = std::fs::read_to_string(&args_file).unwrap();
    assert!(
        args.contains("--volume=0") && args.contains("--rate=44100"),
        "{args}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn pacat_sink_reports_failure_and_missing_binary() {
    let dir = tmp_dir("pacat-fail");
    let bin = fake_pacat(
        &dir,
        "cat >/dev/null\necho 'Connection failure' >&2\nexit 1",
    );
    let mut sink = pacat_with(|s| {
        s.binary = bin;
    });
    let err = sink.play(&Pcm::silence(44_100, 2, 4410)).await.unwrap_err();
    match err {
        PlaybackError::Sink(m) => assert!(m.contains("Connection failure"), "{m}"),
        other => panic!("{other:?}"),
    }
    let mut missing = pacat_with(|s| {
        s.binary = dir.join("does-not-exist");
    });
    assert!(matches!(
        missing
            .play(&Pcm::silence(44_100, 2, 10))
            .await
            .unwrap_err(),
        PlaybackError::Sink(_)
    ));
    std::fs::remove_dir_all(dir).unwrap();
}

fn pacat_with(f: impl FnOnce(&mut PacatSink)) -> PacatSink {
    let mut s = PacatSink::new();
    f(&mut s);
    s
}

fn pid_alive(pid: u32) -> bool {
    // A killed child is reaped by tokio; /proc/<pid> disappears afterwards.
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[tokio::test]
async fn pacat_sink_stop_kills_child() {
    let dir = tmp_dir("pacat-stop");
    let pidf = dir.join("pid");
    let bin = fake_pacat(
        &dir,
        &format!("echo $$ > {}\nexec sleep 30", pidf.display()),
    );
    let mut sink = pacat_with(|s| {
        s.binary = bin;
    });
    let handle = sink.stop_handle();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        handle.stop();
    });
    let started = std::time::Instant::now();
    let err = sink.play(&Pcm::silence(44_100, 2, 4410)).await.unwrap_err();
    assert!(matches!(err, PlaybackError::Cancelled), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    let pid: u32 = std::fs::read_to_string(&pidf)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!pid_alive(pid), "child {pid} still alive");
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn pacat_sink_drop_kills_child() {
    let dir = tmp_dir("pacat-drop");
    let pidf = dir.join("pid");
    let bin = fake_pacat(
        &dir,
        &format!("echo $$ > {}\nexec sleep 30", pidf.display()),
    );
    let mut sink = pacat_with(|s| {
        s.binary = bin;
    });
    let pcm = Pcm::silence(44_100, 2, 4410);
    let r = tokio::time::timeout(Duration::from_millis(400), sink.play(&pcm)).await;
    assert!(r.is_err(), "play should still be running");
    let pid: u32 = std::fs::read_to_string(&pidf)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!pid_alive(pid), "child {pid} still alive after drop");
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn stop_with_nothing_playing_does_not_affect_next_play() {
    let mut sink = NullSink::default();
    sink.stop();
    let dir = tmp_dir("pacat-late-stop");
    let bin = fake_pacat(&dir, "cat >/dev/null");
    let mut p = pacat_with(|s| {
        s.binary = bin;
    });
    p.stop();
    p.play(&Pcm::silence(44_100, 2, 4410)).await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn pacat_play_timeout_kills_child() {
    let dir = tmp_dir("pacat-timeout");
    let bin = fake_pacat(&dir, "exec sleep 30");
    let mut sink = pacat_with(|s| {
        s.binary = bin;
        s.timeout_margin = Duration::from_millis(300);
    });
    let err = sink.play(&Pcm::silence(44_100, 2, 441)).await.unwrap_err();
    assert!(matches!(err, PlaybackError::PlayTimeout(_)), "{err:?}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn pacat_sink_applies_software_gain() {
    let dir = tmp_dir("pacat-gain");
    let out = dir.join("captured.raw");
    let bin = fake_pacat(&dir, &format!("cat > {}", out.display()));
    let pcm = sine(44_100, 2, 440.0, 0.1);
    for (gain, expect_zero) in [(0.0f32, true), (0.5, false)] {
        let mut sink = pacat_with(|s| {
            s.binary = bin.clone();
            s.gain = gain;
        });
        sink.play(&pcm).await.unwrap();
        let raw = std::fs::read(&out).unwrap();
        assert_eq!(raw.len(), pcm.samples.len() * 2);
        let first: Vec<i16> = raw
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        if expect_zero {
            assert!(first.iter().all(|&s| s == 0));
        } else {
            let ratio = rms(&first) / rms(&pcm.samples);
            assert!((ratio - 0.5).abs() < 0.01, "ratio {ratio}");
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

// ------------------------------------------------------- fetch target guard

/// A server that answers every request with `head` (a redirect) and counts
/// the requests.
async fn serve_head(head: String) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::Ordering;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let h = std::sync::Arc::clone(&hits);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            h.fetch_add(1, Ordering::SeqCst);
            let head = head.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (format!("http://{addr}/api/tts_proxy/x"), hits)
}

fn redirect(to: &str) -> String {
    format!(
        "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
}

#[tokio::test]
async fn guarded_fetch_refuses_loopback_without_connecting() {
    use std::sync::atomic::Ordering;
    let (url, hits) = serve_head(redirect("/again")).await;
    let err = fetch(&url, &FetchConfig::default()).await.unwrap_err();
    assert!(
        matches!(&err, PlaybackError::TargetRefused(r) if r == "loopback address"),
        "{err:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "no TCP connection");
    assert!(
        !err.to_string().contains("tts_proxy"),
        "no URL in the error"
    );
    // Unguarded (the behaviour before M5) reaches it.
    let src = sine(8_000, 1, 440.0, 0.1);
    let ok = serve("200 OK", "audio/wav", wav_bytes(&src)).await;
    let unguarded = FetchConfig {
        guarded: false,
        ..FetchConfig::default()
    };
    assert_eq!(fetch(&ok, &unguarded).await.unwrap(), wav_bytes(&src));
}

#[tokio::test]
async fn guarded_fetch_refuses_this_devices_own_address() {
    use std::sync::atomic::Ordering;
    // The host's outbound address, found without sending anything. The
    // rule reads the interface list, so it holds with ip_nonlocal_bind too.
    let Some(ip) = std::net::UdpSocket::bind("0.0.0.0:0")
        .ok()
        .and_then(|s| s.connect("192.0.2.1:9").ok().map(|()| s))
        .and_then(|s| s.local_addr().ok())
        .map(|a| a.ip())
        .filter(|ip| !ip.is_loopback())
    else {
        eprintln!("no IPv4 route; skipped");
        return;
    };
    let listener = TcpListener::bind(format!("{ip}:0")).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let h = std::sync::Arc::clone(&hits);
    tokio::spawn(async move {
        while listener.accept().await.is_ok() {
            h.fetch_add(1, Ordering::SeqCst);
        }
    });
    let cfg = FetchConfig {
        insecure_allow_loopback: true,
        ..FetchConfig::default()
    };
    let err = fetch(&format!("http://{ip}:{port}/tts"), &cfg)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, PlaybackError::TargetRefused(r) if r == "address of this device"),
        "{err:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "no TCP connection");
}

#[tokio::test]
async fn guarded_fetch_limits_redirects_and_checks_every_hop() {
    use std::sync::atomic::Ordering;
    // An endless redirect loop on the same server: 1 + 3 requests, then refused.
    let (url, hits) = serve_head(redirect("/again")).await;
    let err = fetch(&url, &local()).await.unwrap_err();
    assert!(
        matches!(&err, PlaybackError::TargetRefused(r) if r == "too many redirects"),
        "{err:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 4);
    // A redirect to a link-local address is refused before connecting.
    let (url, _) = serve_head(redirect("http://169.254.169.254/latest/meta-data")).await;
    let err = fetch(&url, &local()).await.unwrap_err();
    assert!(
        matches!(&err, PlaybackError::TargetRefused(r) if r == "link-local address"),
        "{err:?}"
    );
    // Another scheme is refused.
    let (url, _) = serve_head(redirect("file:///etc/passwd")).await;
    let err = fetch(&url, &local()).await.unwrap_err();
    assert!(matches!(err, PlaybackError::BadUrl(_)), "{err:?}");
    // A redirect that ends at a good body is followed.
    let src = sine(8_000, 1, 440.0, 0.1);
    let target = serve("200 OK", "audio/wav", wav_bytes(&src)).await;
    let (url, _) = serve_head(redirect(&target)).await;
    assert_eq!(fetch(&url, &local()).await.unwrap(), wav_bytes(&src));
}
