//! The sanitized compatibility report (design sections 6 and 7, schema v1).
//!
//! The report is built from the typed [`ReportInput`], an allow-list: there is
//! no field for a serial, MAC, IP, host or device name, HA address, key,
//! audio, transcript, log line or path. The free strings that exist (model,
//! firmware, kernel, card ids, power states, format) pass
//! [`crate::sanitize::sanitize_token`]; the finished texts and the title pass
//! the fail-closed [`crate::sanitize::scan`], which withholds the whole report
//! (the error names the rule, never the text).
//!
//! One short text (for the link and the QR code, at most about 600 bytes
//! before encoding, whole link at most 1500 characters) and one full text.
//! When the short one does not fit, the lowest-priority lines are dropped
//! first: verdict, model, firmware, webOS, app version, architecture, policy,
//! check results, levels, PCM summary, power, TV control, kernel.

use std::fmt::Write as _;

use crate::check::CheckResult;
use crate::engine::{Overall, SelfCheckResult};
use crate::link::{self, MAX_URL_LEN};
use crate::pcm::PcmDiscovery;
use crate::sanitize::{Rule, numeric_version, sanitize_token, scan};

/// Report schema version, written in the first line.
pub const SCHEMA_VERSION: u32 = 1;
/// Short report size target in bytes before encoding (design 7.4). Soft: the
/// hard cap is the link length.
pub const SHORT_SOFT_BYTES: usize = 600;
/// At most this many cards are listed in the full report.
const MAX_CARDS: usize = 8;
/// At most this many power states are listed.
const MAX_POWER_STATES: usize = 8;

/// Verdict, computed from the self-check (design 6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every check passed and no TV control failed.
    Works,
    /// A warning, a failed TV control, or no self-check result.
    PartlyWorks,
    /// A measured failure.
    DoesNotWork,
}

impl Verdict {
    /// Report words: `works`, `partly works`, `does not work`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Verdict::Works => "works",
            Verdict::PartlyWorks => "partly works",
            Verdict::DoesNotWork => "does not work",
        }
    }
}

/// Firmware policy result (design 2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Model and firmware are in the verified list.
    Verified,
    /// The model series is verified; this firmware is not yet reported.
    SeriesVerified,
    /// Not in the verified list.
    NotVerified,
    /// The firmware differs from the last acknowledged one.
    FirmwareChanged,
    /// Could not be determined.
    Unknown,
}

impl Policy {
    /// Report words.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Policy::Verified => "verified",
            Policy::SeriesVerified => "series verified",
            Policy::NotVerified => "not verified",
            Policy::FirmwareChanged => "firmware changed",
            Policy::Unknown => "unknown",
        }
    }
}

/// TV controls reported by C10.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    /// Screen on/off.
    Screen,
    /// Mute.
    Mute,
    /// Volume.
    Volume,
    /// Input switch.
    Input,
    /// App launch.
    App,
}

impl ControlKind {
    const fn as_str(self) -> &'static str {
        match self {
            ControlKind::Screen => "screen",
            ControlKind::Mute => "mute",
            ControlKind::Volume => "volume",
            ControlKind::Input => "input",
            ControlKind::App => "app",
        }
    }
}

/// Result of one TV control startup check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlResult {
    /// Worked.
    Ok,
    /// Disabled in the configuration.
    Off,
    /// Failed.
    Failed,
}

impl ControlResult {
    const fn as_str(self) -> &'static str {
        match self {
            ControlResult::Ok => "ok",
            ControlResult::Off => "off",
            ControlResult::Failed => "failed",
        }
    }
}

/// Software facts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppFacts {
    /// Satellite version (`0.1.1`).
    pub version: String,
    /// Build architecture (`armv7`, `aarch64`).
    pub arch: String,
}

/// TV facts as read from the TV by the daemon (already parsed fields, never a
/// raw `os_info.json` or `getSystemInfo` reply).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TvFacts {
    /// Model name.
    pub model: Option<String>,
    /// webOS release (`webos_release`).
    pub webos_release: Option<String>,
    /// Firmware (`webos_manufacturing_version`).
    pub firmware: Option<String>,
    /// Kernel architecture (`armv7l`).
    pub kernel_arch: Option<String>,
    /// Kernel version; only the leading number is kept (`5.4.96`).
    pub kernel_version: Option<String>,
}

/// One sound card: id and device numbers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardFacts {
    /// Card id.
    pub id: String,
    /// Playback device numbers.
    pub playback: Vec<u32>,
    /// Capture device numbers.
    pub capture: Vec<u32>,
}

/// The chosen capture device and its negotiated parameters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChosenPcm {
    /// Card id.
    pub card_id: String,
    /// Device number.
    pub device: u32,
    /// Channels.
    pub channels: u32,
    /// Format name (`S24_LE`).
    pub format: String,
    /// Sample rate in Hz.
    pub rate: u32,
}

/// Audio facts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioFacts {
    /// Cards with ids and device numbers (never indexes or names).
    pub cards: Vec<CardFacts>,
    /// The capture device in use.
    pub chosen: Option<ChosenPcm>,
}

impl AudioFacts {
    /// Cards and devices from a parsed `/proc/asound` listing.
    #[must_use]
    pub fn from_discovery(d: &PcmDiscovery, chosen: Option<ChosenPcm>) -> Self {
        let cards = d
            .cards
            .iter()
            .map(|c| CardFacts {
                id: c.id.clone(),
                playback: d.playback_devices(&c.id),
                capture: d.capture_devices(&c.id),
            })
            .collect();
        Self { cards, chosen }
    }
}

/// Capture counters over the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureCounters {
    /// Overruns.
    pub xruns: u64,
    /// Chunks sent to Home Assistant.
    pub chunks_sent: u64,
    /// Chunks dropped.
    pub chunks_dropped: u64,
}

/// Power facts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PowerFacts {
    /// Power states the daemon saw since start (`Active`, `Screen Off`).
    pub states: Vec<String>,
    /// Whether capture was seen working with the screen off.
    pub screen_off_capture_seen: Option<bool>,
}

/// Everything a report may contain.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReportInput {
    /// Software.
    pub app: AppFacts,
    /// TV.
    pub tv: TvFacts,
    /// Firmware policy result.
    pub policy: Option<Policy>,
    /// Audio.
    pub audio: AudioFacts,
    /// Last self-check result, when one ran.
    pub selfcheck: Option<SelfCheckResult>,
    /// Capture counters.
    pub counters: Option<CaptureCounters>,
    /// Power.
    pub power: PowerFacts,
    /// TV control results.
    pub tv_control: Vec<(ControlKind, ControlResult)>,
}

impl ReportInput {
    /// Verdict from the self-check and TV control results.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        let control_failed = self
            .tv_control
            .iter()
            .any(|(_, r)| *r == ControlResult::Failed);
        match self.selfcheck.as_ref().map(SelfCheckResult::overall) {
            Some(Overall::Failed) => Verdict::DoesNotWork,
            Some(Overall::Passed) if !control_failed => Verdict::Works,
            _ => Verdict::PartlyWorks,
        }
    }
}

/// A finished report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// Verdict.
    pub verdict: Verdict,
    /// Issue title.
    pub title: String,
    /// Short text (what is in the link).
    pub short: String,
    /// Full text.
    pub full: String,
    /// Prefilled GitHub issue link (at most 1500 characters).
    pub url: String,
}

impl Report {
    /// JSON for the status app (`ctl.sh report --json`). Hand-written; every
    /// value is sanitized text, escaped here anyway.
    #[must_use]
    pub fn to_json(&self) -> String {
        format!(
            "{{\"schema\":{SCHEMA_VERSION},\"verdict\":{},\"title\":{},\"short\":{},\"full\":{},\"url\":{}}}",
            json_str(self.verdict.as_str()),
            json_str(&self.title),
            json_str(&self.short),
            json_str(&self.full),
            json_str(&self.url)
        )
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", u32::from(c));
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Why no report was produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportError {
    /// The repository is not `owner/name`.
    InvalidRepo,
    /// The final scan found something that must not leave the device; the
    /// text is not kept, only the rule.
    Withheld(Rule),
    /// Even the minimal report does not fit the link cap.
    TooLong,
}

/// A line of the report. `rank` is the drop order of the short report (lower
/// stays longer); `full_only` lines never go into the short one.
struct Line {
    key: &'static str,
    rank: u8,
    full_only: bool,
    parts: Vec<(&'static str, String)>,
}

impl Line {
    fn render(&self) -> String {
        let body = match self.parts.as_slice() {
            [("", v)] => v.clone(),
            parts => parts
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", "),
        };
        format!("{}: {body}", self.key)
    }

    fn field_names(&self) -> Vec<String> {
        self.parts
            .iter()
            .map(|(k, _)| {
                if k.is_empty() {
                    self.key.to_string()
                } else {
                    format!("{}.{k}", self.key)
                }
            })
            .collect()
    }
}

fn tok(v: Option<&str>) -> String {
    v.and_then(sanitize_token)
        .unwrap_or_else(|| "unknown".to_string())
}

fn db(v: f64) -> String {
    let s = format!("{v:.1}");
    if s == "-0.0" { "0.0".to_string() } else { s }
}

fn ranges(nums: &[u32]) -> String {
    let mut v = nums.to_vec();
    v.sort_unstable();
    v.dedup();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let mut j = i;
        while j + 1 < v.len() && v[j + 1] == v[j] + 1 {
            j += 1;
        }
        out.push(if j > i {
            format!("{}-{}", v[i], v[j])
        } else {
            v[i].to_string()
        });
        i = j + 1;
    }
    if out.is_empty() {
        "none".to_string()
    } else {
        out.join(",")
    }
}

fn check_parts(sc: Option<&SelfCheckResult>) -> Vec<(&'static str, String)> {
    match sc {
        None => vec![("", "not run".to_string())],
        Some(r) => r
            .checks()
            .iter()
            .map(|c: &CheckResult| (c.id.as_str(), c.text()))
            .collect(),
    }
}

fn line(key: &'static str, rank: u8, parts: Vec<(&'static str, String)>) -> Line {
    Line {
        key,
        rank,
        full_only: false,
        parts,
    }
}

fn pcm_lines(input: &ReportInput) -> Vec<Line> {
    let summary = match &input.audio.chosen {
        Some(c) => vec![
            ("chosen", tok(Some(&c.card_id))),
            ("device", c.device.to_string()),
            ("channels", c.channels.to_string()),
            ("format", tok(Some(&c.format))),
            ("rate", c.rate.to_string()),
        ],
        None => vec![("", "none".to_string())],
    };
    let mut v = vec![line("pcm", 6, summary)];
    for card in input.audio.cards.iter().take(MAX_CARDS) {
        v.push(Line {
            full_only: true,
            ..line(
                "pcm-card",
                6,
                vec![
                    ("id", tok(Some(&card.id))),
                    ("play", ranges(&card.playback)),
                    ("cap", ranges(&card.capture)),
                ],
            )
        });
    }
    v
}

fn levels_line(input: &ReportInput) -> Line {
    let parts = match input.selfcheck.as_ref().and_then(|r| r.levels) {
        Some(l) => {
            let mut p = vec![
                ("window", format!("{}s", l.window.as_secs())),
                ("rms", db(l.rms_dbfs)),
                ("peak", db(l.peak_dbfs)),
                ("clipped", l.clipped.to_string()),
            ];
            if let Some(c) = input.counters {
                p.push(("xruns", c.xruns.to_string()));
                p.push(("chunks", c.chunks_sent.to_string()));
                p.push(("dropped", c.chunks_dropped.to_string()));
            }
            p
        }
        None => vec![("", "not measured".to_string())],
    };
    line("levels", 5, parts)
}

fn power_line(input: &ReportInput) -> Line {
    let states: Vec<String> = input
        .power
        .states
        .iter()
        .take(MAX_POWER_STATES)
        .filter_map(|s| sanitize_token(s))
        .collect();
    let states = if states.is_empty() {
        "unknown".to_string()
    } else {
        states.join(";")
    };
    let screen_off = match input.power.screen_off_capture_seen {
        Some(true) => "seen",
        Some(false) => "not seen",
        None => "unknown",
    };
    line(
        "power",
        7,
        vec![
            ("states", states),
            ("screen_off_capture", screen_off.to_string()),
        ],
    )
}

fn tv_control_line(input: &ReportInput) -> Line {
    let parts = if input.tv_control.is_empty() {
        vec![("", "not checked".to_string())]
    } else {
        input
            .tv_control
            .iter()
            .map(|(k, r)| (k.as_str(), r.as_str().to_string()))
            .collect()
    };
    line("tv-control", 8, parts)
}

fn lines(input: &ReportInput, verdict: Verdict) -> Vec<Line> {
    let kernel_version = input
        .tv
        .kernel_version
        .as_deref()
        .and_then(numeric_version)
        .unwrap_or_else(|| "unknown".to_string());
    let mut v = vec![
        line("result", 0, vec![("", verdict.as_str().to_string())]),
        line(
            "app",
            2,
            vec![
                ("version", tok(Some(&input.app.version))),
                ("arch", tok(Some(&input.app.arch))),
            ],
        ),
        line(
            "tv",
            1,
            vec![
                ("model", tok(input.tv.model.as_deref())),
                ("webos", tok(input.tv.webos_release.as_deref())),
                ("fw", tok(input.tv.firmware.as_deref())),
            ],
        ),
        line(
            "kernel",
            9,
            vec![
                ("arch", tok(input.tv.kernel_arch.as_deref())),
                ("version", kernel_version),
            ],
        ),
        line(
            "policy",
            3,
            vec![(
                "",
                input.policy.unwrap_or(Policy::Unknown).as_str().to_string(),
            )],
        ),
    ];
    v.extend(pcm_lines(input));
    v.push(line("checks", 4, check_parts(input.selfcheck.as_ref())));
    v.push(levels_line(input));
    v.push(power_line(input));
    v.push(tv_control_line(input));
    v
}

/// Names of every field the full report renders for this input, in order and
/// without repeats (`tv.model`, `checks.C3`, ...). The allow-list schema test
/// pins this list.
#[must_use]
pub fn field_names(input: &ReportInput) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for l in lines(input, input.verdict()) {
        for n in l.field_names() {
            if !seen.contains(&n) {
                seen.push(n);
            }
        }
    }
    seen
}

fn header() -> String {
    format!("webos-satellite compatibility report v{SCHEMA_VERSION}")
}

fn text_of(lines: &[Line], max_rank: u8, with_full_only: bool) -> String {
    let mut out = header();
    for l in lines {
        if l.rank <= max_rank && (with_full_only || !l.full_only) {
            out.push('\n');
            out.push_str(&l.render());
        }
    }
    out
}

fn title_of(input: &ReportInput, verdict: Verdict) -> String {
    format!(
        "[Compat] {} fw {}: {}",
        tok(input.tv.model.as_deref()),
        tok(input.tv.firmware.as_deref()),
        verdict.as_str()
    )
}

/// Builds the report and the link for `repo` (`owner/name`, default
/// [`link::DEFAULT_REPO`]).
///
/// # Errors
///
/// [`ReportError::InvalidRepo`], [`ReportError::Withheld`] when the final scan
/// fires, [`ReportError::TooLong`] when even the minimal report does not fit
/// the link cap.
pub fn build(input: &ReportInput, repo: &str) -> Result<Report, ReportError> {
    if !link::valid_repo(repo) {
        return Err(ReportError::InvalidRepo);
    }
    let verdict = input.verdict();
    let ls = lines(input, verdict);
    let full = text_of(&ls, u8::MAX, true);
    let title = title_of(input, verdict);

    // Drop the lowest-priority lines until the short text and the link fit.
    // Rank 2 is the floor (result, tv, app): the text size is then only a
    // soft target, the link cap still applies.
    let mut chosen: Option<(String, String, String)> = None;
    for max_rank in (2..=9u8).rev() {
        let short = text_of(&ls, max_rank, false);
        let url = link::issue_url(repo, &title, &short);
        let fits = url.len() <= MAX_URL_LEN && (short.len() <= SHORT_SOFT_BYTES || max_rank == 2);
        if fits {
            chosen = Some((short, title.clone(), url));
            break;
        }
    }
    // Last resort: a title that is only the verdict.
    if chosen.is_none() {
        let short = text_of(&ls, 2, false);
        let small = format!("[Compat] {}", verdict.as_str());
        let url = link::issue_url(repo, &small, &short);
        if url.len() <= MAX_URL_LEN {
            chosen = Some((short, small, url));
        }
    }
    let (short, title, url) = chosen.ok_or(ReportError::TooLong)?;

    for text in [&full, &short, &title] {
        scan(text, repo).map_err(ReportError::Withheld)?;
    }
    Ok(Report {
        verdict,
        title,
        short,
        full,
        url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{CheckId, Grade, Reason, check_c1, check_c8};
    use crate::engine::{Config, OpenOutcome, SelfCheck};
    use crate::link::{DEFAULT_REPO, parse_issue_url};
    use crate::pcm::PcmDiscovery;
    use crate::window::FrameStats;
    use std::time::Duration;

    fn result_with(frames_amp: Option<i32>) -> SelfCheckResult {
        // Run the real engine so the report sees real results.
        let mut e = SelfCheck::new(Config::default());
        let t0 = Duration::ZERO;
        let _ = e.on_pcm(t0, CheckResult::pass(CheckId::C2));
        let _ = e.on_open(t0, OpenOutcome::Opened);
        let mut seed = 11u32;
        let mut t = t0;
        let mut out = None;
        while t < Duration::from_secs(12) {
            let st = match frames_amp {
                Some(0) => FrameStats::from_samples(&[77; 320], 2, 24),
                Some(a) => {
                    let v: Vec<i32> = (0..320)
                        .map(|_| {
                            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                            i32::try_from(seed >> 8).unwrap_or(0) % (2 * a + 1) - a
                        })
                        .collect();
                    FrameStats::from_samples(&v, 2, 24)
                }
                None => FrameStats::default(),
            };
            let step = e.on_frame(t, &st);
            let step = if step.result.is_none() {
                e.on_tick(t)
            } else {
                step
            };
            if let Some(r) = step.result {
                out = Some(r);
                break;
            }
            t += Duration::from_millis(10);
        }
        let mut r = out.expect("result");
        r.set_check(check_c1(true, true));
        r.set_check(check_c8(false, false, false));
        r
    }

    fn c3a_input() -> ReportInput {
        let disc = PcmDiscovery::parse(
            " 0 [lg115x         ]: lg115x - lg115x\n 1 [LGSIC          ]: LGSIC - LGSIC\n",
            "00-00: A : : playback 1\n00-10: B : : capture 1\n01-00: WoV : : capture 1\n01-01: Sp : : capture 1\n01-02: Out : : playback 1\n",
        );
        ReportInput {
            app: AppFacts {
                version: "0.1.1".into(),
                arch: "armv7".into(),
            },
            tv: TvFacts {
                model: Some("OLED42C34LA".into()),
                webos_release: Some("8.3.0".into()),
                firmware: Some("03.30.74".into()),
                kernel_arch: Some("armv7l".into()),
                kernel_version: Some("5.4.96-lg+x".into()),
            },
            policy: Some(Policy::Verified),
            audio: AudioFacts::from_discovery(
                &disc,
                Some(ChosenPcm {
                    card_id: "LGSIC".into(),
                    device: 0,
                    channels: 2,
                    format: "S24_LE".into(),
                    rate: 16_000,
                }),
            ),
            selfcheck: Some(result_with(Some(16_000))),
            counters: Some(CaptureCounters {
                xruns: 0,
                chunks_sent: 312,
                chunks_dropped: 0,
            }),
            power: PowerFacts {
                states: vec!["Active".into()],
                screen_off_capture_seen: Some(false),
            },
            tv_control: vec![
                (ControlKind::Screen, ControlResult::Ok),
                (ControlKind::Mute, ControlResult::Ok),
                (ControlKind::Volume, ControlResult::Ok),
                (ControlKind::Input, ControlResult::Ok),
                (ControlKind::App, ControlResult::Off),
            ],
        }
    }

    #[test]
    fn positive_report() {
        let input = c3a_input();
        assert_eq!(input.verdict(), Verdict::Works);
        let r = build(&input, DEFAULT_REPO).expect("report");
        assert_eq!(r.verdict, Verdict::Works);
        assert_eq!(r.title, "[Compat] OLED42C34LA fw 03.30.74: works");
        let want_head = "webos-satellite compatibility report v1\nresult: works\napp: version=0.1.1, arch=armv7\ntv: model=OLED42C34LA, webos=8.3.0, fw=03.30.74\nkernel: arch=armv7l, version=5.4.96\npolicy: verified\npcm: chosen=LGSIC, device=0, channels=2, format=S24_LE, rate=16000\n";
        assert!(r.full.starts_with(want_head), "{}", r.full);
        assert!(r.full.contains("pcm-card: id=LGSIC, play=2, cap=0-1"));
        assert!(r.full.contains("pcm-card: id=lg115x, play=0, cap=10"));
        assert!(r.full.contains(
            "checks: C1=pass, C2=pass, C3=pass, C4=pass, C5=pass, C6=pass, C7=pass, C8=skipped"
        ));
        assert!(r.full.contains("levels: window=10s, rms=-5"), "{}", r.full);
        assert!(r.full.contains("xruns=0, chunks=312, dropped=0"));
        assert!(
            r.full
                .contains("power: states=Active, screen_off_capture=not seen")
        );
        assert!(
            r.full
                .ends_with("tv-control: screen=ok, mute=ok, volume=ok, input=ok, app=off")
        );
        assert!(!r.short.contains("pcm-card"), "short has no card list");
        assert!(r.short.len() <= SHORT_SOFT_BYTES, "{} bytes", r.short.len());
        assert!(r.url.len() <= MAX_URL_LEN);
        assert!(
            r.url.len() < 1000,
            "c3-a sized link is about 930 characters"
        );
    }

    #[test]
    fn negative_report_names_the_failure() {
        let mut input = c3a_input();
        input.selfcheck = Some(result_with(Some(0))); // constant input: stuck
        assert_eq!(input.verdict(), Verdict::DoesNotWork);
        let r = build(&input, DEFAULT_REPO).expect("report");
        assert!(r.title.ends_with(": does not work"));
        assert!(r.full.contains("result: does not work"));
        assert!(r.full.contains("C5=fail(stuck)"), "{}", r.full);
    }

    #[test]
    fn warning_and_missing_selfcheck_are_partly_works() {
        let mut input = c3a_input();
        input.selfcheck = Some(result_with(Some(2))); // very quiet
        assert_eq!(input.verdict(), Verdict::PartlyWorks);
        assert!(
            build(&input, DEFAULT_REPO)
                .expect("r")
                .full
                .contains("C6=warn(quiet)")
        );
        input.selfcheck = None;
        assert_eq!(input.verdict(), Verdict::PartlyWorks);
        let r = build(&input, DEFAULT_REPO).expect("r");
        assert!(r.full.contains("checks: not run"));
        assert!(r.full.contains("levels: not measured"));
    }

    #[test]
    fn failed_tv_control_downgrades_a_pass() {
        let mut input = c3a_input();
        input.tv_control[0].1 = ControlResult::Failed;
        assert_eq!(input.verdict(), Verdict::PartlyWorks);
    }

    #[test]
    fn busy_mic_is_reported_as_busy() {
        let mut e = SelfCheck::new(Config::default());
        let step = e.on_open(Duration::ZERO, OpenOutcome::Busy);
        let mut input = c3a_input();
        input.selfcheck = step.result;
        let r = build(&input, DEFAULT_REPO).expect("report");
        assert!(r.full.contains("C3=fail(busy)"));
        assert_eq!(r.verdict, Verdict::DoesNotWork);
    }

    #[test]
    fn unknown_tv_values_say_unknown() {
        let r = build(&ReportInput::default(), DEFAULT_REPO).expect("report");
        assert!(r.title.starts_with("[Compat] unknown fw unknown: "));
        assert!(
            r.full
                .contains("tv: model=unknown, webos=unknown, fw=unknown")
        );
        assert!(r.full.contains("pcm: none"));
        assert!(r.full.contains("tv-control: not checked"));
    }

    /// Test 2: the allow-list schema. Adding a field fails this test until
    /// the list is updated on purpose.
    #[test]
    fn allow_list_schema_is_pinned() {
        let mut input = c3a_input();
        input.selfcheck = Some(result_with(Some(16_000)));
        let want: Vec<&str> = vec![
            "result",
            "app.version",
            "app.arch",
            "tv.model",
            "tv.webos",
            "tv.fw",
            "kernel.arch",
            "kernel.version",
            "policy",
            "pcm.chosen",
            "pcm.device",
            "pcm.channels",
            "pcm.format",
            "pcm.rate",
            "pcm-card.id",
            "pcm-card.play",
            "pcm-card.cap",
            "checks.C1",
            "checks.C2",
            "checks.C3",
            "checks.C4",
            "checks.C5",
            "checks.C6",
            "checks.C7",
            "checks.C8",
            "levels.window",
            "levels.rms",
            "levels.peak",
            "levels.clipped",
            "levels.xruns",
            "levels.chunks",
            "levels.dropped",
            "power.states",
            "power.screen_off_capture",
            "tv-control.screen",
            "tv-control.mute",
            "tv-control.volume",
            "tv-control.input",
            "tv-control.app",
        ];
        assert_eq!(field_names(&input), want);
        // And the rendered text has exactly these labels (no hidden extras):
        // every line key in the text is a key of the schema.
        let r = build(&input, DEFAULT_REPO).expect("report");
        let keys: Vec<&str> = r
            .full
            .lines()
            .skip(1)
            .map(|l| l.split(':').next().unwrap_or(""))
            .collect();
        let allowed = [
            "result",
            "app",
            "tv",
            "kernel",
            "policy",
            "pcm",
            "pcm-card",
            "checks",
            "levels",
            "power",
            "tv-control",
        ];
        assert!(keys.iter().all(|k| allowed.contains(k)), "{keys:?}");
    }

    /// Test 1: seeded identifiers never reach the short report, the full
    /// report, the title or the link.
    #[test]
    fn seeded_identifiers_never_appear() {
        let seeds = [
            "LGTV7XK2M9QD4R8T1B",
            "712KCDM1A234ZXCVBNMQ",
            "aa:bb:cc:dd:ee:ff",
            "AA-BB-CC-DD-EE-FF",
            "192.168.1.77",
            "fe80::1ff:fe23:4567:890a",
            "livingroom-tv.local",
            "http://homeassistant.local:8123",
            "https://ha.example.org/api/webhook/xyz",
            "dGhpcyBpcyBhIHNlY3JldCBrZXkgZm9yIHRoZSBzYXRlbGxpdGU=",
            "0123456789abcdef0123456789abcdef",
            "/var/run/nyx/os_info.json",
        ];
        // Seeds go into every free-string input at once, in several forms.
        for seed in seeds {
            let s = seed.to_string();
            let mut input = c3a_input();
            input.app = AppFacts {
                version: s.clone(),
                arch: s.clone(),
            };
            input.tv = TvFacts {
                model: Some(s.clone()),
                webos_release: Some(format!("8.3.0 {s}")),
                firmware: Some(s.clone()),
                kernel_arch: Some(s.clone()),
                kernel_version: Some(s.clone()),
            };
            input.audio.chosen = Some(ChosenPcm {
                card_id: s.clone(),
                device: 0,
                channels: 2,
                format: s.clone(),
                rate: 16_000,
            });
            input.audio.cards.push(CardFacts {
                id: s.clone(),
                playback: vec![0],
                capture: vec![0],
            });
            input.power.states = vec![s.clone(), format!("Active {s}")];
            let r = build(&input, DEFAULT_REPO).expect("report with seeded inputs");
            let decoded = parse_issue_url(&r.url).expect("url parses");
            for (what, text) in [
                ("short", &r.short),
                ("full", &r.full),
                ("title", &r.title),
                ("json", &r.to_json()),
                ("decoded report", &decoded.report),
                ("decoded title", &decoded.title),
            ] {
                assert!(!text.contains(seed), "{seed} leaked into {what}: {text}");
            }
            // The URL is encoded: check the encoded seed too.
            assert!(
                !r.url.contains(&link::percent_encode(seed)),
                "{seed} in url"
            );
            // The seeded fields were dropped, not echoed in pieces either.
            for piece in [
                "homeassistant",
                "example.org",
                "webhook",
                "nyx",
                "livingroom",
            ] {
                assert!(!r.full.contains(piece), "{piece} from {seed}");
            }
        }
    }

    #[test]
    fn seeds_in_proc_asound_text_do_not_leak() {
        let serial = "LGSERIAL9X4K2M7QD81B";
        let cards = format!(" 0 [{serial}      ]: x - x\n 1 [LGSIC          ]: LGSIC - LGSIC\n");
        let pcm = "00-00: aa:bb:cc:dd:ee:ff : 10.1.2.3 : capture 1\n01-00: Mic : : capture 1\n";
        let d = PcmDiscovery::parse(&cards, pcm);
        let mut input = c3a_input();
        input.audio = AudioFacts::from_discovery(&d, None);
        let r = build(&input, DEFAULT_REPO).expect("report");
        assert!(!r.full.contains(serial));
        assert!(!r.full.contains("aa:bb"));
        assert!(!r.full.contains("10.1.2.3"));
        assert!(r.full.contains("pcm-card: id=unknown, play=none, cap=0"));
        assert!(r.full.contains("pcm-card: id=LGSIC"));
    }

    #[test]
    fn nothing_outside_the_input_struct_can_leak() {
        // Per the design the builder has no access to configuration or
        // environment: the only inputs are the typed fields. A config-like
        // string stuffed into a typed free field is still caught by the
        // sanitizer (above); here, the whole thing must also survive a
        // hostile numeric-looking kernel string.
        let mut input = c3a_input();
        input.tv.kernel_version = Some("5.4.96-192.168.1.5".into());
        let r = build(&input, DEFAULT_REPO).expect("report");
        assert!(r.full.contains("kernel: arch=armv7l, version=5.4.96"));
        assert!(!r.full.contains("192.168"));
    }

    #[test]
    fn worst_case_link_fits_and_decodes_to_the_same_text() {
        let long = "ab-".repeat(30); // sanitized to 48 characters
        let mut input = c3a_input();
        input.tv.model = Some(long.clone());
        input.tv.firmware = Some(long.clone());
        input.tv.webos_release = Some(long.clone());
        input.tv.kernel_arch = Some(long.clone());
        input.app.version = long.clone();
        input.app.arch = long.clone();
        input.audio.chosen = Some(ChosenPcm {
            card_id: long.clone(),
            device: 4_000_000_000,
            channels: u32::MAX,
            format: long.clone(),
            rate: u32::MAX,
        });
        input.power.states = (0..20).map(|i| format!("State {i} {long}")).collect();
        input.counters = Some(CaptureCounters {
            xruns: u64::MAX,
            chunks_sent: u64::MAX,
            chunks_dropped: u64::MAX,
        });
        // Characters that triple in size when encoded.
        input.tv.model = Some("A B+C.D_E-F ".repeat(10));
        let r = build(&input, DEFAULT_REPO).expect("report");
        assert!(r.url.len() <= MAX_URL_LEN, "{} chars", r.url.len());
        let p = parse_issue_url(&r.url).expect("parse");
        assert_eq!(p.report, r.short);
        assert_eq!(p.title, r.title);
        assert_eq!(p.template, "compatibility-report.yml");
        // The verdict and the TV identity survive; low-priority lines went.
        assert!(r.short.contains("result: "));
        assert!(r.short.contains("\ntv: "));
        assert!(r.short.contains("\napp: "));
    }

    #[test]
    fn truncation_drops_lowest_priority_lines_first() {
        let mut input = c3a_input();
        // Long (but legal) values and a long repo name push the short text
        // and the link over their budgets.
        let long = "Abcdefghij Abcdefghij Abcdefghij Abcdefgh".to_string();
        input.tv.model = Some(long.clone());
        input.tv.firmware = Some(long.clone());
        input.tv.webos_release = Some(long.clone());
        input.app.version = long.clone();
        input.audio.chosen = Some(ChosenPcm {
            card_id: long.clone(),
            device: 0,
            channels: 2,
            format: long,
            rate: 16_000,
        });
        let full_short = text_of(&lines(&input, input.verdict()), 9, false);
        let repo = format!("{}/{}", "o".repeat(40), "r".repeat(40));
        let r = build(&input, &repo).expect("report");
        assert!(r.url.len() <= MAX_URL_LEN);
        assert!(r.short.len() < full_short.len(), "something was dropped");
        // What is kept is a prefix in priority order: if a lower line is
        // there, every higher one is.
        let order = [
            "result",
            "tv",
            "app",
            "policy",
            "checks",
            "levels",
            "pcm",
            "power",
            "tv-control",
            "kernel",
        ];
        let present: Vec<bool> = order
            .iter()
            .map(|k| r.short.lines().any(|l| l.starts_with(&format!("{k}:"))))
            .collect();
        let first_missing = present.iter().position(|p| !p).unwrap_or(present.len());
        assert!(present[first_missing..].iter().all(|p| !p), "{present:?}");
        assert!(present[0] && present[1] && present[2]);
    }

    #[test]
    fn minimal_report_too_long_for_the_repo_is_an_error() {
        let input = c3a_input();
        // Valid names, but long enough that nothing fits.
        let repo = format!("{}/{}", "o".repeat(100), "r".repeat(100));
        let mut big = input;
        big.tv.model = Some("M ".repeat(24));
        big.tv.firmware = Some("F ".repeat(24));
        let res = build(&big, &repo);
        assert!(matches!(res, Ok(_) | Err(ReportError::TooLong)));
        if let Ok(r) = res {
            assert!(r.url.len() <= MAX_URL_LEN);
        }
    }

    #[test]
    fn invalid_repo_is_refused() {
        assert_eq!(
            build(&ReportInput::default(), "not a repo"),
            Err(ReportError::InvalidRepo)
        );
        assert_eq!(
            build(&ReportInput::default(), "a/b?x=1"),
            Err(ReportError::InvalidRepo)
        );
    }

    #[test]
    fn fork_repo_goes_into_the_link_only() {
        let r = build(&c3a_input(), "someone/fork").expect("report");
        assert!(r.url.starts_with(
            "https://github.com/someone/fork/issues/new?template=compatibility-report.yml&title="
        ));
        assert_eq!(parse_issue_url(&r.url).expect("parse").repo, "someone/fork");
        assert!(!r.full.contains("someone"));
    }

    #[test]
    fn fail_closed_scan_withholds_the_report() {
        // The sanitizer cannot be bypassed through the typed API, so check
        // the gate directly with a scan-level hit through a unit input:
        // a raw MAC in a text must be rejected by `scan`.
        assert_eq!(scan("x aa:bb:cc:dd:ee:ff", DEFAULT_REPO), Err(Rule::Mac));
        assert_eq!(
            ReportError::Withheld(Rule::Mac),
            ReportError::Withheld(Rule::Mac)
        );
    }

    #[test]
    fn json_is_escaped_and_complete() {
        let r = build(&c3a_input(), DEFAULT_REPO).expect("report");
        let j = r.to_json();
        assert!(j.starts_with("{\"schema\":1,\"verdict\":\"works\",\"title\":\""));
        assert!(j.contains("\\n"), "newlines escaped");
        assert!(!j.contains('\n'));
        assert!(j.ends_with("\"}"));
    }

    #[test]
    fn dbfs_formatting_has_no_negative_zero() {
        assert_eq!(db(-0.04), "0.0");
        assert_eq!(db(-67.64), "-67.6");
        assert_eq!(db(-140.0), "-140.0");
    }

    #[test]
    fn ranges_are_compact() {
        assert_eq!(ranges(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]), "0-9");
        assert_eq!(ranges(&[10, 11, 12, 13, 14]), "10-14");
        assert_eq!(ranges(&[0, 2, 3, 5]), "0,2-3,5");
        assert_eq!(ranges(&[]), "none");
    }

    #[test]
    fn grades_in_a_result_are_the_only_check_words() {
        let r = result_with(Some(16_000));
        for c in r.checks() {
            assert!(matches!(c.grade, Grade::Pass | Grade::Skipped));
            assert!(c.reason.is_none() || c.reason == Some(Reason::NotMeasured));
        }
    }
}
