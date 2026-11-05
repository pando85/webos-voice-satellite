//! Music on the satellite's media player (design milestone M5,
//! docs/design/music-playback.md, ADR 0010).
//!
//! [`Music`] owns the music engine of `webos-playback` (one music slot,
//! gain, ducking, pacer, a second `pacat`) and the policy around it that
//! needs the daemon: what HA's `play_media` starts, which voice-side events
//! duck the music, the shared volume, the power and disconnect rules, and
//! the state reported to HA and to `run/voice.json`.
//!
//! - **Source.** A `play_media` URL is opened with
//!   [`webos_playback::stream::open`] (the guarded streaming fetch on the
//!   `music-fetch` and `music-decode` threads) and handed to the engine. A
//!   URL is never retried (HA's proxy would restart the track). A URL the
//!   satellite will not play (a refused `ENQUEUE`, a `play_media` in Active
//!   Standby) is **opened and closed** ([`Music::release`]) so HA's proxy
//!   frees the conversion slot an unfetched URL would hold (D12).
//! - **Ducking** (D4, D5): the daemon calls [`Music::sync`] after every event
//!   with the voice run state and what the reply player plays; a source is
//!   begun or ended on the engine only when it changes, so the engine's
//!   reference counts stay balanced.
//! - **Volume** (D2): one volume shared with the replies: the engine's
//!   [`MusicVolume`] is `sink.gain` times the reply volume's gain (capped by
//!   `max_volume_percent`), set whenever either changes. No mute (D13).
//! - **Power** (D6): Active Standby (or a suspend) stops the music and new
//!   `play_media` is refused there; the satellite never wakes the TV. After
//!   a resume from suspend the music stays stopped.
//! - **HA disconnect** (D9): music keeps playing for `disconnect_stop_s`,
//!   then stops; a reconnect within that time keeps it.
//! - **State.** `playing` once the first audio of a track was queued (and
//!   while a `play_media` announcement plays), `paused`, `idle`. No URL,
//!   title or address is ever logged or reported: URLs are redacted
//!   (scheme, host, first path segment).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tokio::time::Instant;
use webos_esphome_api::{AppCommand, MusicState as ApiState};
use webos_playback::SinkFormat;
use webos_playback::guard::redact_url;
use webos_playback::music::{
    DuckSource, EndReason, MusicEngine, MusicEvent, MusicOutput, MusicSource, MusicState,
    MusicVolume, OutFuture, OutputFactory, PacatConfig, PacatFactory, TrackId, WavStreamFactory,
};
use webos_playback::stream::{self, StreamConfig};

use crate::config::{MusicSection, SinkKind, SinkSection};
use crate::player::Kind;
use crate::volume;

/// Name of the second `pacat` stream.
pub(crate) const STREAM_NAME: &str = "music";

/// How often [`Music::release`] looks whether the response has arrived.
const RELEASE_POLL: Duration = Duration::from_millis(20);

/// How a track ended, as `run/voice.json` reports it (design section K).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    /// The stream ended and the output drained.
    Ended,
    /// HA (or the stop order of D5) stopped it.
    Stopped,
    /// The source or the output failed.
    Error,
    /// The stream delivered no byte for `idle_timeout_s`.
    IdleTimeout,
    /// Paused longer than `pause_timeout_s`.
    PauseTimeout,
    /// HA stayed disconnected longer than `disconnect_stop_s` (D9).
    HaGone,
    /// The TV went to Active Standby or suspended (D6).
    Standby,
}

impl End {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Ended => "ended",
            Self::Stopped => "stopped",
            Self::Error => "error",
            Self::IdleTimeout => "idle_timeout",
            Self::PauseTimeout => "pause_timeout",
            Self::HaGone => "ha_gone",
            Self::Standby => "standby",
        }
    }
}

/// The track the satellite started last.
#[derive(Debug, Clone)]
struct Current {
    id: TrackId,
    /// `ha_proxy` or `external` (never an address or a name).
    source: &'static str,
    /// The codec named by the URL's extension, if any.
    codec: Option<&'static str>,
    /// The first audio was queued (the output is open).
    started: bool,
    /// Decoded rate and channels, once started.
    format: Option<(u32, u16)>,
    /// Why the satellite itself ended it, for the end report.
    ending: Option<End>,
}

/// The engine's outputs that discard everything (`[sink] type = "null"`).
#[derive(Debug, Default)]
struct NullFactory;

struct NullOutput;

impl MusicOutput for NullOutput {
    fn write<'a>(&'a mut self, _bytes: &'a [u8]) -> OutFuture<'a> {
        Box::pin(async { Ok(()) })
    }
    fn finish(&mut self) -> OutFuture<'_> {
        Box::pin(async { Ok(()) })
    }
    fn abort(&mut self) -> OutFuture<'_> {
        Box::pin(async { Ok(()) })
    }
}

impl OutputFactory for NullFactory {
    fn open(&mut self, _format: SinkFormat) -> std::io::Result<Box<dyn MusicOutput>> {
        Ok(Box::new(NullOutput))
    }
}

/// The output of the music slot for `[sink]`: a second `pacat` (with
/// `[music] latency_msec`), a growing WAV file per track in `sink.dir`
/// (`music-<epoch>-<n>.wav`), or nothing.
fn output_factory(sink: &SinkSection, m: &MusicSection) -> Box<dyn OutputFactory> {
    match sink.kind {
        SinkKind::Pacat => Box::new(PacatFactory::new(PacatConfig {
            binary: sink.binary.clone(),
            device: sink.device.clone(),
            latency_msec: m.latency_msec,
            stream_name: STREAM_NAME.to_owned(),
        })),
        SinkKind::Wav => {
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            Box::new(WavStreamFactory::new(
                sink.dir.clone(),
                &format!("music-{secs}"),
            ))
        }
        SinkKind::Null => Box::new(NullFactory),
    }
}

/// `ha_proxy` for HA's ffmpeg proxy path, `external` for anything else
/// (Music Assistant, a direct media URL). Only the class is kept.
fn source_class(url: &str) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or("");
    if path.contains("/api/esphome/ffmpeg_proxy/") {
        "ha_proxy"
    } else {
        "external"
    }
}

/// The codec the URL's extension names, when it is one the decoder knows.
fn codec_of(url: &str) -> Option<&'static str> {
    let path = url.split(['?', '#']).next()?;
    let (_, ext) = path.rsplit('/').next()?.rsplit_once('.')?;
    match ext.to_ascii_lowercase().as_str() {
        "flac" => Some("flac"),
        "wav" => Some("wav"),
        "mp3" => Some("mp3"),
        "ogg" | "oga" => Some("ogg"),
        _ => None,
    }
}

/// The power states in which music stops and `play_media` is refused (D6):
/// LG's Active Standby (the output is silent there, verified for a tone on
/// c3-b) and a suspend. `key` is [`crate::power::PowerUpdate::state_key`]
/// (no spaces).
pub(crate) fn is_standby(key: &str) -> bool {
    key == "ActiveStandby" || key.contains("Suspend")
}

/// The music controller owned by the daemon loop.
pub(crate) struct Music {
    engine: MusicEngine,
    cfg: MusicSection,
    stream_cfg: StreamConfig,
    volume: MusicVolume,
    sink_gain: f32,
    /// The gain last given to [`MusicVolume`].
    applied_gain: Option<f32>,
    current: Option<Current>,
    /// What HA was told last.
    reported: ApiState,
    cmd_tx: mpsc::Sender<AppCommand>,
    /// In Active Standby or suspended: no music (D6).
    standby: bool,
    /// HA disconnected while music played: stop at this time (D9).
    stop_at: Option<Instant>,
    /// Duck sources begun on the engine: wake word (a voice run), reply,
    /// announcement, alarm.
    ducks: [bool; 4],
    /// When the last duck source ended (the duck is held after it).
    duck_ended: Option<Instant>,
    last_end: Option<End>,
}

const DUCK_SOURCES: [DuckSource; 4] = [
    DuckSource::WakeWord,
    DuckSource::Reply,
    DuckSource::Announcement,
    DuckSource::Alarm,
];

impl Music {
    /// Starts the engine when `[music] enabled`; `None` otherwise. The
    /// receiver carries the engine's events for [`Music::on_event`].
    /// `fetch_loopback` is the `--test-fetch-loopback` test aid.
    pub(crate) fn start(
        m: &MusicSection,
        sink: &SinkSection,
        fetch_loopback: bool,
        cmd_tx: mpsc::Sender<AppCommand>,
    ) -> Option<(Self, mpsc::UnboundedReceiver<MusicEvent>)> {
        if !m.enabled {
            return None;
        }
        let volume = MusicVolume::new(0.0);
        let (engine, events) =
            MusicEngine::spawn(m.engine(), output_factory(sink, m), volume.clone());
        let stream_cfg = StreamConfig {
            idle_timeout: Duration::from_secs(u64::from(m.idle_timeout_s)),
            insecure_allow_loopback: fetch_loopback,
            ..StreamConfig::default()
        };
        if fetch_loopback {
            log::warn!("music: --test-fetch-loopback: streams may come from 127.0.0.1 (test aid)");
        }
        Some((
            Self {
                engine,
                cfg: *m,
                stream_cfg,
                volume,
                sink_gain: sink.gain,
                applied_gain: None,
                current: None,
                reported: ApiState::Idle,
                cmd_tx,
                standby: false,
                stop_at: None,
                ducks: [false; 4],
                duck_ended: None,
                last_end: None,
            },
            events,
        ))
    }

    /// Something is loaded, playing or paused.
    #[cfg(test)]
    pub(crate) fn is_active(&self) -> bool {
        self.current.is_some()
    }

    /// Why a new `play_media` is refused now, if it is.
    pub(crate) fn refusal(&self) -> Option<&'static str> {
        self.standby
            .then_some("the TV is in Active Standby or suspended")
    }

    /// `play_media` (not an announcement): replaces the current music.
    pub(crate) fn play(&mut self, url: &str) {
        let shown = redact_url(url);
        if let Some(why) = self.refusal() {
            log::info!("music: play_media {shown} refused: {why}; the URL is released");
            self.release(url);
            return;
        }
        match stream::open(url, &self.stream_cfg) {
            Ok(s) => {
                let source = source_class(url);
                log::info!("music: play_media {shown} (source {source})");
                self.play_source(Box::new(s), source, codec_of(url));
            }
            Err(e) => log::warn!("music: play_media {shown} refused: {e}"),
        }
    }

    fn play_source(
        &mut self,
        source: Box<dyn MusicSource>,
        class: &'static str,
        codec: Option<&'static str>,
    ) {
        let id = self.engine.play(source);
        self.current = Some(Current {
            id,
            source: class,
            codec,
            started: false,
            format: None,
            ending: None,
        });
    }

    /// Tests: plays a generated source as if it were an external stream.
    #[cfg(test)]
    pub(crate) fn play_test_source(&mut self, source: Box<dyn MusicSource>) {
        self.play_source(source, "external", None);
    }

    /// `media_pause`.
    pub(crate) fn pause(&mut self) {
        if self.current.is_some() {
            log::info!("music: paused by Home Assistant");
            self.engine.pause();
        } else {
            log::info!("music: pause ignored (no music)");
        }
    }

    /// `media_play`: resumes a paused track (the stream is held, D8).
    pub(crate) fn resume(&mut self) {
        if self.current.is_some() {
            log::info!("music: resumed by Home Assistant");
            self.engine.resume();
        } else {
            log::info!("music: resume ignored (no music; a stream is never fetched again)");
        }
    }

    /// Stops the music; returns false when nothing was playing.
    pub(crate) fn stop(&mut self, why: End) -> bool {
        let Some(c) = self.current.as_mut() else {
            return false;
        };
        c.ending = Some(why);
        log::info!("music: stopping ({})", why.word());
        self.engine.stop();
        true
    }

    /// Opens `url` and closes it as soon as the response headers (or an
    /// error) arrived, without playing it: HA's proxy then frees the slot
    /// the URL holds (D12). Runs in the background; the guard rules apply
    /// as for playback, so a refused target is never connected to.
    pub(crate) fn release(&self, url: &str) {
        let shown = redact_url(url);
        let s = match stream::open(url, &self.stream_cfg) {
            Ok(s) => s,
            Err(e) => {
                log::info!("music: {shown} not released: {e}");
                return;
            }
        };
        let limit = self.stream_cfg.connect_timeout + self.stream_cfg.response_timeout;
        tokio::spawn(async move {
            let deadline = Instant::now() + limit;
            while s.content_type().is_none()
                && s.stats().bytes_received == 0
                && !s.is_finished()
                && Instant::now() < deadline
            {
                tokio::time::sleep(RELEASE_POLL).await;
            }
            drop(s); // cancels: the socket is shut down
            log::info!("music: {shown} opened and closed without playing (frees HA's proxy slot)");
        });
    }

    /// The Home Assistant connection changed (D9).
    pub(crate) fn on_ha(&mut self, connected: bool, now: Instant) {
        if connected {
            if self.stop_at.take().is_some() {
                log::info!("music: Home Assistant is back; the music goes on");
            }
        } else if self.current.is_some() && self.stop_at.is_none() {
            let grace = Duration::from_secs(u64::from(self.cfg.disconnect_stop_s));
            log::info!(
                "music: Home Assistant disconnected; the music stops in {} s unless it reconnects",
                grace.as_secs()
            );
            self.stop_at = Some(now + grace);
        }
    }

    /// A power state (`state_key`, no spaces): Active Standby or a suspend
    /// stops the music and blocks new music until another state (D6).
    pub(crate) fn on_power(&mut self, state_key: &str) {
        let standby = is_standby(state_key);
        if standby == self.standby {
            return;
        }
        self.standby = standby;
        if standby {
            log::info!("music: the TV is in {state_key}; music is stopped and refused there");
            self.stop(End::Standby);
        } else {
            log::info!("music: the TV left standby ({state_key}); play_media is accepted again");
        }
    }

    /// The system resumed from a suspend: whatever stream there was is not
    /// trusted; the music stays stopped (D6).
    pub(crate) fn on_resume(&mut self) {
        self.stop(End::Standby);
    }

    /// An event of the engine task.
    pub(crate) fn on_event(&mut self, ev: MusicEvent) {
        match ev {
            MusicEvent::Started { id, rate, channels } => {
                if let Some(c) = self.current.as_mut().filter(|c| c.id == id) {
                    c.started = true;
                    c.format = Some((rate, channels));
                    log::info!(
                        "music: track {} playing {} {rate} Hz {channels} ch from {}",
                        id.0,
                        c.codec.unwrap_or("audio"),
                        c.source
                    );
                }
            }
            MusicEvent::Ended { id, reason } => {
                let Some(c) = self.current.take_if(|c| c.id == id) else {
                    log::debug!("music: track {} ended ({reason:?}), already replaced", id.0);
                    return;
                };
                let end = match &reason {
                    EndReason::Finished => End::Ended,
                    EndReason::Stopped | EndReason::Replaced => c.ending.unwrap_or(End::Stopped),
                    EndReason::PauseTimeout => End::PauseTimeout,
                    EndReason::Error(m) if m.contains("stalled") => End::IdleTimeout,
                    EndReason::Error(_) => End::Error,
                };
                match &reason {
                    EndReason::Error(m) => {
                        log::warn!(
                            "music: track {} ended: {} ({m}); not retried",
                            id.0,
                            end.word()
                        );
                    }
                    _ => log::info!("music: track {} ended: {}", id.0, end.word()),
                }
                self.last_end = Some(end);
                self.stop_at = None;
            }
        }
    }

    /// Called by the daemon after every event: the duck sources, the shared
    /// volume, the HA-disconnect deadline and the state for HA.
    ///
    /// `in_run`: a voice run is in progress (wake word heard or push-to-talk
    /// until the run ends); `playing`: what the reply player plays;
    /// `reply_volume`: the shared volume in percent.
    pub(crate) async fn sync(
        &mut self,
        in_run: bool,
        playing: Option<Kind>,
        reply_volume: u8,
        now: Instant,
    ) {
        let want = [
            in_run,
            playing == Some(Kind::Tts),
            matches!(playing, Some(Kind::Announce | Kind::MediaAnnounce)),
            playing == Some(Kind::Alarm),
        ];
        let was_ducked = self.ducks.iter().any(|&d| d);
        for ((have, want), source) in self.ducks.iter_mut().zip(want).zip(DUCK_SOURCES) {
            if *have != want {
                *have = want;
                if want {
                    self.engine.duck_begin(source);
                } else {
                    self.engine.duck_end(source);
                }
                log::debug!(
                    "music: duck source {source:?} {}",
                    if want { "on" } else { "off" }
                );
            }
        }
        if was_ducked && !self.ducks.iter().any(|&d| d) {
            self.duck_ended = Some(now);
        }

        let gain = self.sink_gain * volume::gain_for(reply_volume.min(self.cfg.max_volume_percent));
        if self.applied_gain != Some(gain) {
            self.applied_gain = Some(gain);
            self.volume.set(gain);
        }

        if self.stop_at.is_some_and(|t| now >= t) {
            self.stop_at = None;
            log::warn!(
                "music: Home Assistant stayed disconnected for {} s; stopping the music",
                self.cfg.disconnect_stop_s
            );
            self.stop(End::HaGone);
        }

        let state = self.api_state(playing);
        if state != self.reported {
            self.reported = state;
            log::info!("music: state {state:?} reported to Home Assistant");
            if self
                .cmd_tx
                .send(AppCommand::SetMusicState(state))
                .await
                .is_err()
            {
                log::warn!("music: API server is gone");
            }
        }
    }

    /// The state HA sees: `playing` once audio was queued (a track that is
    /// loading keeps the state it replaces), `paused`, `playing` while a
    /// `play_media` announcement plays, else `idle`.
    fn api_state(&self, playing: Option<Kind>) -> ApiState {
        let music = match (self.engine.state(), self.current.as_ref()) {
            (MusicState::Paused, Some(_)) => ApiState::Paused,
            (MusicState::Playing, Some(c)) if c.started || self.reported != ApiState::Idle => {
                ApiState::Playing
            }
            _ => ApiState::Idle,
        };
        if music == ApiState::Idle && playing == Some(Kind::MediaAnnounce) {
            ApiState::Playing
        } else {
            music
        }
    }

    /// The duck is held: a source is active, or the last one ended less than
    /// `duck_hold_ms` ago.
    fn ducked(&self, now: Instant) -> bool {
        self.ducks.iter().any(|&d| d)
            || self.duck_ended.is_some_and(|t| {
                now.saturating_duration_since(t)
                    < Duration::from_millis(u64::from(self.cfg.duck_hold_ms))
            })
    }

    /// The `music` object of `run/voice.json` (design section K): no URL,
    /// title, address or name, only the state, the decoded format, the
    /// source class and counters.
    pub(crate) fn status_json(&self, now: Instant) -> String {
        let state = match self.reported {
            ApiState::Idle => "idle",
            ApiState::Playing => "playing",
            ApiState::Paused => "paused",
        };
        let (format, rate, channels) = match self
            .current
            .as_ref()
            .and_then(|c| c.format.map(|(r, ch)| (c.codec, r, ch)))
        {
            Some((codec, r, ch)) => (
                format!("\"{} {r} Hz {ch} ch\"", codec.unwrap_or("audio")),
                r.to_string(),
                ch.to_string(),
            ),
            None => ("null".to_owned(), "null".to_owned(), "null".to_owned()),
        };
        let quoted = |s: Option<&str>| s.map_or_else(|| "null".to_owned(), |s| format!("\"{s}\""));
        format!(
            "{{\"enabled\":true,\"state\":\"{state}\",\"format\":{format},\"rate\":{rate},\"channels\":{channels},\"source\":{},\"ducked\":{},\"duck_db\":{},\"standby\":{},\"last_end\":{},\"underruns\":{}}}",
            quoted(self.current.as_ref().map(|c| c.source)),
            self.ducked(now),
            self.cfg.duck_db,
            self.standby,
            quoted(self.last_end.map(End::word)),
            self.engine.stats().underruns,
        )
    }

    /// Stops the music and the engine task (bounded wait; `pacat` is gone
    /// afterwards).
    pub(crate) async fn shutdown(self) {
        self.engine.shutdown().await;
    }
}

/// The `music` object of `run/voice.json` with music off.
pub(crate) const DISABLED_JSON: &str = "{\"enabled\":false}";

/// For tests: the WAV files the music slot wrote in `dir`.
#[cfg(test)]
pub(crate) fn music_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .map(|r| {
            r.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                        n.starts_with("music-")
                            && std::path::Path::new(n)
                                .extension()
                                .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::float_cmp)]

    use super::*;
    use webos_playback::music::{ToneSource, Wave, read_wav};

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("webos-sat-music-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn start(
        d: &std::path::Path,
        m: MusicSection,
    ) -> (
        Music,
        mpsc::UnboundedReceiver<MusicEvent>,
        mpsc::Receiver<AppCommand>,
    ) {
        let sink = SinkSection {
            kind: SinkKind::Wav,
            dir: d.to_path_buf(),
            ..SinkSection::default()
        };
        let (tx, rx) = mpsc::channel(64);
        let (music, ev) =
            Music::start(&MusicSection { enabled: true, ..m }, &sink, false, tx).unwrap();
        (music, ev, rx)
    }

    fn states(rx: &mut mpsc::Receiver<AppCommand>) -> Vec<ApiState> {
        let mut v = Vec::new();
        while let Ok(c) = rx.try_recv() {
            if let AppCommand::SetMusicState(s) = c {
                v.push(s);
            }
        }
        v
    }

    async fn pump(
        music: &mut Music,
        ev: &mut mpsc::UnboundedReceiver<MusicEvent>,
        until: Duration,
    ) {
        let end = Instant::now() + until;
        while Instant::now() < end {
            tokio::select! {
                Some(e) = ev.recv() => music.on_event(e),
                () = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
            music.sync(false, None, 100, Instant::now()).await;
        }
    }

    fn tone(ms: u64) -> Box<dyn MusicSource> {
        Box::new(ToneSource::new(
            48_000,
            2,
            Wave::Dc,
            10_000,
            Some(Duration::from_millis(ms)),
        ))
    }

    #[test]
    fn disabled_starts_nothing() {
        let (tx, _rx) = mpsc::channel(1);
        assert!(
            Music::start(&MusicSection::default(), &SinkSection::default(), false, tx).is_none()
        );
    }

    #[test]
    fn url_classes_and_codecs() {
        assert_eq!(
            source_class("http://ha:8123/api/esphome/ffmpeg_proxy/dev/abc.flac"),
            "ha_proxy"
        );
        assert_eq!(source_class("http://ma:8097/flow/x/queue.flac"), "external");
        assert_eq!(
            source_class("http://x/a?u=/api/esphome/ffmpeg_proxy/"),
            "external"
        );
        assert_eq!(codec_of("http://ha/api/x/abc.FLAC?t=1"), Some("flac"));
        assert_eq!(codec_of("http://ha/a.wav"), Some("wav"));
        assert_eq!(codec_of("http://ha/stream"), None);
        assert_eq!(codec_of("http://ha/a.opus"), None);
        assert!(is_standby("ActiveStandby"));
        assert!(is_standby("Suspend"));
        assert!(!is_standby("Active"));
        assert!(!is_standby("ScreenOff"));
        assert!(!is_standby("ScreenSaver"));
    }

    #[tokio::test]
    async fn plays_reports_and_ends_with_the_shared_volume() {
        let d = dir("plays");
        let (mut music, mut ev, mut rx) = start(
            &d,
            MusicSection {
                max_volume_percent: 100,
                ..MusicSection::default()
            },
        );
        music.play_source(tone(300), "external", Some("flac"));
        music.sync(false, None, 80, Instant::now()).await;
        // Nothing queued yet: still idle for HA.
        assert!(states(&mut rx).is_empty());
        pump(&mut music, &mut ev, Duration::from_millis(150)).await;
        assert_eq!(states(&mut rx), vec![ApiState::Playing]);
        let json = music.status_json(Instant::now());
        assert!(json.contains("\"state\":\"playing\""), "{json}");
        assert!(json.contains("\"format\":\"flac 48000 Hz 2 ch\""), "{json}");
        assert!(json.contains("\"source\":\"external\""), "{json}");
        pump(&mut music, &mut ev, Duration::from_millis(600)).await;
        assert_eq!(states(&mut rx), vec![ApiState::Idle]);
        assert!(!music.is_active());
        assert_eq!(music.last_end, Some(End::Ended));
        let json = music.status_json(Instant::now());
        assert!(json.contains("\"last_end\":\"ended\""), "{json}");
        assert!(!json.contains("http"), "{json}");
        // The volume in the file: pump syncs at 100 % (unity), the DC is kept.
        let files = music_files(&d);
        assert_eq!(files.len(), 1);
        let (fmt, samples) = read_wav(&files[0]).unwrap();
        assert_eq!((fmt.rate_hz, fmt.channels), (48_000, 2));
        assert!(samples.contains(&10_000), "unity gain somewhere");
        music.shutdown().await;
        std::fs::remove_dir_all(d).unwrap();
    }

    #[tokio::test]
    async fn duck_sources_follow_the_daemon_state_and_stay_balanced() {
        let d = dir("duck");
        let (mut music, _ev, _rx) = start(&d, MusicSection::default());
        let now = Instant::now();
        music.sync(true, None, 100, now).await;
        assert_eq!(music.ducks, [true, false, false, false]);
        // Repeated syncs do not begin again.
        music.sync(true, Some(Kind::Tts), 100, now).await;
        music.sync(true, Some(Kind::Tts), 100, now).await;
        assert_eq!(music.ducks, [true, true, false, false]);
        music.sync(false, Some(Kind::MediaAnnounce), 100, now).await;
        assert_eq!(music.ducks, [false, false, true, false]);
        assert!(music.ducked(now));
        music.sync(false, Some(Kind::Alarm), 100, now).await;
        assert_eq!(music.ducks, [false, false, false, true]);
        let later = now + Duration::from_millis(100);
        music.sync(false, None, 100, later).await;
        assert_eq!(music.ducks, [false; 4]);
        assert!(music.ducked(later), "held after the last source");
        assert!(!music.ducked(later + Duration::from_millis(900)));
        music.shutdown().await;
        std::fs::remove_dir_all(d).unwrap();
    }

    #[tokio::test]
    async fn volume_is_sink_gain_times_the_reply_volume_with_the_cap() {
        let d = dir("volume");
        let (mut music, _ev, _rx) = start(
            &d,
            MusicSection {
                max_volume_percent: 60,
                ..MusicSection::default()
            },
        );
        music.sink_gain = 0.5;
        music.sync(false, None, 100, Instant::now()).await;
        assert_eq!(music.applied_gain, Some(0.5 * volume::gain_for(60)));
        music.sync(false, None, 40, Instant::now()).await;
        assert_eq!(music.applied_gain, Some(0.5 * volume::gain_for(40)));
        music.sync(false, None, 0, Instant::now()).await;
        assert_eq!(music.applied_gain, Some(0.0));
        assert_eq!(music.volume.get_q15(), 0);
        music.shutdown().await;
        std::fs::remove_dir_all(d).unwrap();
    }

    #[tokio::test]
    async fn standby_stops_and_refuses_then_allows_again() {
        let d = dir("standby");
        let (mut music, mut ev, mut rx) = start(&d, MusicSection::default());
        music.play_source(tone(5_000), "ha_proxy", None);
        pump(&mut music, &mut ev, Duration::from_millis(100)).await;
        assert_eq!(states(&mut rx), vec![ApiState::Playing]);
        music.on_power("ActiveStandby");
        assert!(music.refusal().is_some());
        pump(&mut music, &mut ev, Duration::from_millis(100)).await;
        assert_eq!(states(&mut rx), vec![ApiState::Idle]);
        assert_eq!(music.last_end, Some(End::Standby));
        // A play_media there is refused (a refused URL is only released).
        music.play("http://192.0.2.1:9/api/esphome/ffmpeg_proxy/d/x.flac");
        assert!(!music.is_active());
        music.on_power("Active");
        assert!(music.refusal().is_none());
        music.shutdown().await;
        std::fs::remove_dir_all(d).unwrap();
    }

    #[tokio::test]
    async fn ha_disconnect_grace_stops_or_keeps_the_music() {
        let d = dir("grace");
        let (mut music, mut ev, mut rx) = start(
            &d,
            MusicSection {
                disconnect_stop_s: 1,
                ..MusicSection::default()
            },
        );
        music.play_source(tone(10_000), "external", None);
        pump(&mut music, &mut ev, Duration::from_millis(100)).await;
        let now = Instant::now();
        music.on_ha(false, now);
        // A reconnect within the grace keeps it.
        music.on_ha(true, now + Duration::from_millis(500));
        music
            .sync(false, None, 100, now + Duration::from_secs(2))
            .await;
        assert!(music.is_active());
        // Without one it stops.
        music.on_ha(false, now);
        music
            .sync(false, None, 100, now + Duration::from_secs(2))
            .await;
        pump(&mut music, &mut ev, Duration::from_millis(100)).await;
        assert!(!music.is_active());
        assert_eq!(music.last_end, Some(End::HaGone));
        assert_eq!(states(&mut rx), vec![ApiState::Playing, ApiState::Idle]);
        music.shutdown().await;
        std::fs::remove_dir_all(d).unwrap();
    }

    #[tokio::test]
    async fn pause_resume_stop_and_the_media_announce_state() {
        let d = dir("pause");
        let (mut music, mut ev, mut rx) = start(&d, MusicSection::default());
        // A play_media announcement with no music reads playing.
        music
            .sync(false, Some(Kind::MediaAnnounce), 100, Instant::now())
            .await;
        music.sync(false, None, 100, Instant::now()).await;
        assert_eq!(states(&mut rx), vec![ApiState::Playing, ApiState::Idle]);
        music.play_source(tone(10_000), "external", None);
        pump(&mut music, &mut ev, Duration::from_millis(100)).await;
        music.pause();
        pump(&mut music, &mut ev, Duration::from_millis(100)).await;
        music.resume();
        pump(&mut music, &mut ev, Duration::from_millis(100)).await;
        assert!(music.stop(End::Stopped));
        pump(&mut music, &mut ev, Duration::from_millis(100)).await;
        assert!(!music.stop(End::Stopped), "nothing left to stop");
        assert_eq!(
            states(&mut rx),
            vec![
                ApiState::Playing,
                ApiState::Paused,
                ApiState::Playing,
                ApiState::Idle
            ]
        );
        assert_eq!(music.last_end, Some(End::Stopped));
        music.shutdown().await;
        std::fs::remove_dir_all(d).unwrap();
    }
}
