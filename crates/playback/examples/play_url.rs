//! Manual harness for the playback pipeline (PC and TV).
//!
//! ```text
//! play_url <http(s)-url | local-file> [options]
//!   --sink pacat|wav:<out.wav>|null   output (default pacat)
//!   --volume N                         pacat stream volume, 0..65536
//!   --gain F                           software gain before pacat (0.0 = digital silence)
//!   --device NAME                      pacat sink name
//!   --pacat PATH                       pacat binary (default: pacat)
//!   --ca FILE                          PEM trust anchors replacing the bundled roots
//!   --fetch-only                       download and report size/content type, no decode
//!   --cancel-after-ms N                stop playback after N ms (tests cancellation)
//! ```
#![allow(clippy::print_stdout, clippy::print_stderr, clippy::too_many_lines)]

use std::path::PathBuf;
use std::time::Duration;

use webos_playback::{
    FetchConfig, NullSink, PacatSink, Sink, WavFileSink, fetch_with_meta, play_bytes, play_url,
};

fn usage() -> ! {
    eprintln!(
        "usage: play_url <url|file> [--sink pacat|wav:FILE|null] [--volume N] [--gain F] [--device D] [--pacat PATH] [--ca FILE] [--fetch-only] [--cancel-after-ms N]"
    );
    std::process::exit(2);
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let Some(target) = args.next() else { usage() };
    let mut sink_kind = "pacat".to_owned();
    let mut volume = None;
    let mut gain = 1.0f32;
    let mut device = None;
    let mut pacat = PathBuf::from("pacat");
    let mut cfg = FetchConfig::default();
    let mut fetch_only = false;
    let mut cancel_ms: Option<u64> = None;
    while let Some(a) = args.next() {
        let mut val = || args.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--sink" => sink_kind = val(),
            "--volume" => volume = val().parse::<u32>().ok().or_else(|| usage()),
            "--gain" => gain = val().parse().unwrap_or_else(|_| usage()),
            "--device" => device = Some(val()),
            "--pacat" => pacat = PathBuf::from(val()),
            "--ca" => cfg.ca_file = Some(PathBuf::from(val())),
            "--fetch-only" => fetch_only = true,
            "--cancel-after-ms" => cancel_ms = val().parse().ok().or_else(|| usage()),
            _ => usage(),
        }
    }

    let is_url = target.starts_with("http://") || target.starts_with("https://");
    if fetch_only {
        match fetch_with_meta(&target, &cfg).await {
            Ok(f) => println!(
                "fetch ok: {} bytes, content-type {:?}",
                f.bytes.len(),
                f.content_type
            ),
            Err(e) => {
                println!("fetch failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let mut sink: Box<dyn Sink> = if sink_kind == "null" {
        Box::new(NullSink::default())
    } else if let Some(path) = sink_kind.strip_prefix("wav:") {
        Box::new(WavFileSink::new(path))
    } else if sink_kind == "pacat" {
        let mut s = PacatSink::new();
        s.binary = pacat;
        s.volume = volume;
        s.gain = gain;
        s.device = device;
        Box::new(s)
    } else {
        usage()
    };
    if let Some(ms) = cancel_ms {
        let h = sink.stop_handle();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            println!("cancelling");
            h.stop();
        });
    }

    let result = if is_url {
        play_url(&target, &cfg, sink.as_mut()).await
    } else {
        match std::fs::read(&target) {
            Ok(bytes) => {
                let ext = std::path::Path::new(&target)
                    .extension()
                    .and_then(|e| e.to_str());
                play_bytes(&bytes, ext, sink.as_mut(), Duration::ZERO).await
            }
            Err(e) => {
                eprintln!("cannot read {target}: {e}");
                std::process::exit(1);
            }
        }
    };
    match result {
        Ok(t) => println!(
            "ok: {} bytes, audio {:.3}s, fetch {} ms, decode {} ms, resample {} ms, play {} ms",
            t.bytes,
            t.audio.as_secs_f64(),
            t.fetch.as_millis(),
            t.decode.as_millis(),
            t.resample.as_millis(),
            t.play.as_millis()
        ),
        Err(e) => {
            println!("failed: {e}");
            std::process::exit(1);
        }
    }
}
