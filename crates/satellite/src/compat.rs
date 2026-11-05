//! Compatibility status in the daemon (ADR 0006): the entities, the
//! compatibility report and the status files for `ctl.sh` and the TV app.
//!
//! [`Compat`] is plain state with no I/O besides the files it is asked to
//! write. It gets the firmware verdict (with the model name once read), the
//! capture thread's [`Note`]s (self-check results), TV control's per-control
//! results and the power states seen, and produces:
//!
//! * the entity states ([`Compat::commands`]): `firmware_verified`,
//!   `capture_problem`, `firmware_changed`, `tv_firmware`, `tv_model`,
//!   `compat_status`, `selfcheck`, `report_status`;
//! * the allow-listed report (`webos-compat`; never a serial, MAC, IP, host,
//!   HA address, key, audio or log line) and its files in `[report] dir`.
//!
//! Nothing is sent anywhere by the daemon. The model name comes from one
//! `luna-send` call that asks for `modelName` only
//! ([`read_model`]); `apiadapter/system/getSystemInfo` (which returns the
//! serial number) is never used.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use webos_compat::check::{check_c1, check_c8};
use webos_compat::engine::{Overall, SelfCheckResult};
use webos_compat::report::{
    AppFacts, AudioFacts, ControlKind, ControlResult, Policy, PowerFacts, Report, ReportError,
    ReportInput, TvFacts, build,
};
use webos_compat::sanitize::sanitize_token;
use webos_esphome_api::proto::EntityCategory;
use webos_esphome_api::{
    AppCommand, BinarySensorEntity, ButtonEntity, Config as EspConfig, TextSensorEntity,
};

use crate::config::Settings;
use crate::firmware::{FirmwareInfo, Seen, Verdict};
use crate::selfcheck::{Finished, Note};

/// Object ids of the entities.
pub(crate) const FIRMWARE_VERIFIED: &str = "firmware_verified";
pub(crate) const CAPTURE_PROBLEM: &str = "capture_problem";
pub(crate) const FIRMWARE_CHANGED: &str = "firmware_changed";
pub(crate) const TV_FIRMWARE: &str = "tv_firmware";
pub(crate) const TV_MODEL: &str = "tv_model";
pub(crate) const COMPAT_STATUS: &str = "compat_status";
pub(crate) const SELFCHECK: &str = "selfcheck";
pub(crate) const REPORT_STATUS: &str = "report_status";
pub(crate) const SELFCHECK_RUN: &str = "selfcheck_run";
pub(crate) const FIRMWARE_ACK: &str = "firmware_ack";

/// Trigger files in the report directory (`ctl.sh selfcheck`,
/// `ctl.sh ack-firmware`).
pub(crate) const SELFCHECK_TRIGGER: &str = "selfcheck-run";
pub(crate) const ACK_TRIGGER: &str = "firmware-ack";

/// The model query: `modelName` only.
const MODEL_URI: &str = "luna://com.webos.service.tv.systemproperty/getSystemInfo";
const MODEL_PAYLOAD: &str = r#"{"keys":["modelName"]}"#;

fn text(object_id: &str, name: &str, icon: &str) -> TextSensorEntity {
    TextSensorEntity {
        object_id: object_id.to_owned(),
        name: name.to_owned(),
        icon: icon.to_owned(),
        device_class: String::new(),
        entity_category: EntityCategory::Diagnostic,
    }
}

fn binary(object_id: &str, name: &str, icon: &str, device_class: &str) -> BinarySensorEntity {
    BinarySensorEntity {
        object_id: object_id.to_owned(),
        name: name.to_owned(),
        icon: icon.to_owned(),
        device_class: device_class.to_owned(),
        entity_category: EntityCategory::Diagnostic,
    }
}

fn button(object_id: &str, name: &str, icon: &str) -> ButtonEntity {
    ButtonEntity {
        object_id: object_id.to_owned(),
        name: name.to_owned(),
        icon: icon.to_owned(),
        device_class: String::new(),
        entity_category: EntityCategory::Config,
    }
}

/// Adds the compatibility entities to the server configuration. The
/// self-check ones are left out with `safety.self_check = false`.
pub(crate) fn add_entities(esp: &mut EspConfig, self_check: bool) {
    esp.binary_sensors.extend([
        binary(
            FIRMWARE_VERIFIED,
            "Firmware verified",
            "mdi:shield-check",
            "",
        ),
        binary(FIRMWARE_CHANGED, "Firmware changed", "mdi:update", ""),
    ]);
    esp.text_sensors.extend([
        text(TV_FIRMWARE, "TV firmware", "mdi:tag"),
        text(TV_MODEL, "TV model", "mdi:television"),
        text(COMPAT_STATUS, "Compatibility", "mdi:check-decagram"),
        text(REPORT_STATUS, "Compatibility report", "mdi:file-document"),
    ]);
    esp.buttons.push(button(
        FIRMWARE_ACK,
        "Acknowledge firmware change",
        "mdi:check",
    ));
    if self_check {
        esp.binary_sensors.push(binary(
            CAPTURE_PROBLEM,
            "Capture problem",
            "mdi:microphone-alert",
            "problem",
        ));
        esp.text_sensors.push(text(
            SELFCHECK,
            "Microphone self-check",
            "mdi:microphone-question",
        ));
        esp.buttons.push(button(
            SELFCHECK_RUN,
            "Re-run microphone check",
            "mdi:microphone-settings",
        ));
    }
}

/// The model name from the TV, `None` when it is not read. One call with the
/// key `modelName`; the reply is parsed for that one value and dropped. A
/// pty first (how TV control's calls answer on the TV), then a pipe.
pub(crate) async fn read_model(luna: &Path) -> Option<String> {
    let mut reply = crate::power::call_pty(luna, MODEL_URI, MODEL_PAYLOAD, 4096).await;
    if !matches!(&reply, Ok(t) if t.contains("modelName")) {
        reply = crate::power::call(luna, MODEL_URI, MODEL_PAYLOAD).await;
    }
    parse_model(&reply.ok()?)
}

/// The `modelName` value of a `getSystemInfo` reply, sanitized.
pub(crate) fn parse_model(reply: &str) -> Option<String> {
    sanitize_token(&crate::firmware::json_string(reply, "modelName")?)
}

/// What the daemon knows about the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModelState {
    Known(String),
    NotRead,
}

/// Compatibility state.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)] // independent switches copied from the settings
pub(crate) struct Compat {
    repo: String,
    report_on: bool,
    dir: PathBuf,
    self_check_on: bool,
    aec_on: bool,
    seen_file: PathBuf,
    strict: bool,
    firmware: Option<FirmwareInfo>,
    model: ModelState,
    verdict: Verdict,
    seen: Seen,
    kernel: (Option<String>, Option<String>),
    checking: bool,
    last: Option<Finished>,
    audio: Option<AudioFacts>,
    reference: Option<bool>,
    tv_control: Vec<(ControlKind, ControlResult)>,
    power_states: Vec<String>,
    screen_off_capture: Option<bool>,
    report_status: String,
    written: Option<String>,
}

impl Compat {
    /// Builds the state. `info` is the firmware read from the TV, `model`
    /// what `luna-send` said.
    pub(crate) fn new(
        settings: &Settings,
        info: Result<FirmwareInfo, String>,
        model: ModelState,
        verdict: Verdict,
    ) -> Self {
        let seen_file = settings.safety.firmware_seen_path();
        let firmware = info.ok();
        let seen = match &firmware {
            Some(i) => crate::firmware::check_seen(&seen_file, &i.version),
            None => Seen::Same,
        };
        Self {
            repo: settings.report.repo.clone(),
            report_on: settings.report.enabled,
            dir: settings.report.dir_or_default(),
            self_check_on: settings.safety.self_check,
            aec_on: settings.aec_active(),
            seen_file,
            strict: settings.safety.firmware_policy == crate::config::FirmwarePolicy::Strict,
            firmware,
            model,
            verdict,
            seen,
            kernel: kernel_facts(),
            checking: false,
            last: None,
            audio: None,
            reference: None,
            tv_control: Vec::new(),
            power_states: Vec::new(),
            screen_off_capture: None,
            report_status: if settings.report.enabled {
                "not built yet".to_owned()
            } else {
                "off (report.enabled = false)".to_owned()
            },
            written: None,
        }
    }

    /// First start on this install: remember the firmware (no change yet).
    /// Call once at startup. Returns a log line when something was stored.
    pub(crate) fn remember_first_start(&mut self) -> Option<String> {
        if self.seen != Seen::First {
            return None;
        }
        let version = self.firmware.as_ref()?.version.clone();
        match crate::firmware::write_seen(&self.seen_file, &version) {
            Ok(()) => {
                self.seen = Seen::Same;
                Some(format!(
                    "firmware {version} remembered in {}",
                    self.seen_file.display()
                ))
            }
            Err(e) => Some(format!(
                "cannot remember the firmware in {}: {e} (a firmware change will not be noticed)",
                self.seen_file.display()
            )),
        }
    }

    /// The firmware differs from the last acknowledged one.
    pub(crate) fn firmware_changed(&self) -> Option<&str> {
        match &self.seen {
            Seen::Changed(old) => Some(old.as_str()),
            _ => None,
        }
    }

    /// Acknowledges a firmware change. Returns a log line.
    pub(crate) fn acknowledge(&mut self) -> String {
        let Some(version) = self.firmware.as_ref().map(|i| i.version.clone()) else {
            return "firmware change acknowledged: nothing to store (firmware not read)".to_owned();
        };
        if self.firmware_changed().is_none() {
            return "firmware change acknowledged: there was no change".to_owned();
        }
        match crate::firmware::write_seen(&self.seen_file, &version) {
            Ok(()) => {
                self.seen = Seen::Same;
                format!("firmware change acknowledged: {version} stored")
            }
            Err(e) => format!(
                "firmware change NOT acknowledged: cannot write {}: {e}",
                self.seen_file.display()
            ),
        }
    }

    /// TV control's per-control results (check C10).
    /// Returns true when they changed.
    pub(crate) fn set_tv_control(&mut self, v: Vec<(ControlKind, ControlResult)>) -> bool {
        if self.tv_control == v {
            return false;
        }
        self.tv_control = v;
        true
    }

    /// `safety.self_check` is on.
    pub(crate) fn self_check_on(&self) -> bool {
        self.self_check_on
    }

    /// A power state was seen (`Active`, `Screen Off`, ...). `mic_open`: the
    /// microphone PCM is open right now.
    pub(crate) fn on_power_state(&mut self, state: &str, mic_open: bool) {
        let Some(s) = sanitize_token(state) else {
            return;
        };
        if s.to_ascii_lowercase().replace(' ', "") == "screenoff" {
            self.screen_off_capture = Some(self.screen_off_capture == Some(true) || mic_open);
        }
        if !self.power_states.contains(&s) && self.power_states.len() < 8 {
            self.power_states.push(s);
        }
    }

    /// A note from the capture thread.
    pub(crate) fn on_note(&mut self, note: Note) {
        match note {
            Note::Checking => self.checking = true,
            Note::NotRun => self.checking = false,
            Note::Audio(facts, reference) => {
                if !facts.cards.is_empty() || facts.chosen.is_some() {
                    self.audio = Some(*facts);
                }
                if reference.is_some() {
                    self.reference = reference;
                }
            }
            Note::Result(done) => {
                self.checking = false;
                let mut done = *done;
                self.fill_c1_c8(&mut done.result);
                self.last = Some(done);
            }
        }
    }

    fn fill_c1_c8(&self, r: &mut SelfCheckResult) {
        r.set_check(check_c1(
            matches!(self.model, ModelState::Known(_)),
            self.firmware.is_some(),
        ));
        // A configured but absent reference counts as "silent".
        r.set_check(check_c8(self.aec_on, self.reference.unwrap_or(false), true));
    }

    fn overall(&self) -> Option<Overall> {
        self.last.as_ref().map(|d| d.result.overall())
    }

    /// The last self-check closed the microphone or failed.
    pub(crate) fn capture_problem(&self) -> bool {
        self.self_check_on && self.overall() == Some(Overall::Failed)
    }

    fn verdict_status(&self) -> &'static str {
        match &self.verdict {
            Verdict::Verified(..) => "verified",
            Verdict::SeriesOnly(..) => "series verified (firmware not yet reported)",
            Verdict::NotVerified { .. } | Verdict::Off => "not verified",
            Verdict::Unreadable { .. } => "unknown",
        }
    }

    /// Value of `compat_status`.
    pub(crate) fn status_text(&self) -> &'static str {
        if self.capture_problem() {
            "capture problem"
        } else if self.firmware_changed().is_some() {
            "firmware changed"
        } else if self.checking {
            "checking"
        } else {
            self.verdict_status()
        }
    }

    fn verified_state(&self) -> Option<bool> {
        match &self.verdict {
            Verdict::Verified(..) => Some(true),
            Verdict::SeriesOnly(..) | Verdict::NotVerified { .. } | Verdict::Off => Some(false),
            Verdict::Unreadable { .. } => None,
        }
    }

    /// Value of `selfcheck`.
    pub(crate) fn selfcheck_text(&self) -> String {
        if !self.self_check_on {
            "not checked (safety.self_check = false)".to_owned()
        } else if self.checking {
            "checking".to_owned()
        } else if let Some(d) = &self.last {
            let mut r = d.result.clone();
            self.fill_c1_c8(&mut r);
            r.summary_text()
        } else {
            "not run (microphone closed)".to_owned()
        }
    }

    fn firmware_text(&self) -> Option<String> {
        self.firmware
            .as_ref()
            .and_then(|i| sanitize_token(&i.version))
    }

    fn model_text(&self) -> Option<String> {
        match &self.model {
            ModelState::Known(m) => Some(m.clone()),
            ModelState::NotRead => None,
        }
    }

    /// The server commands for every compatibility entity (the server does
    /// not repeat an unchanged state).
    pub(crate) fn commands(&self) -> Vec<AppCommand> {
        let sensor = |id: &str, state: Option<bool>| AppCommand::SetBinarySensor {
            object_id: id.to_owned(),
            state,
        };
        let txt = |id: &str, state: Option<String>| AppCommand::SetTextSensor {
            object_id: id.to_owned(),
            state,
        };
        let mut out = vec![
            sensor(FIRMWARE_VERIFIED, self.verified_state()),
            sensor(FIRMWARE_CHANGED, Some(self.firmware_changed().is_some())),
            txt(TV_FIRMWARE, self.firmware_text()),
            txt(TV_MODEL, self.model_text()),
            txt(COMPAT_STATUS, Some(self.status_text().to_owned())),
            txt(REPORT_STATUS, Some(self.report_status.clone())),
        ];
        if self.self_check_on {
            out.push(sensor(CAPTURE_PROBLEM, Some(self.capture_problem())));
            out.push(txt(SELFCHECK, Some(self.selfcheck_text())));
        }
        out
    }

    fn policy(&self) -> Policy {
        if self.firmware_changed().is_some() {
            return Policy::FirmwareChanged;
        }
        match &self.verdict {
            Verdict::Verified(..) => Policy::Verified,
            Verdict::SeriesOnly(..) => Policy::SeriesVerified,
            Verdict::NotVerified { .. } | Verdict::Off => Policy::NotVerified,
            Verdict::Unreadable { .. } => Policy::Unknown,
        }
    }

    /// The report input: typed allow-listed facts only.
    pub(crate) fn report_input(&self) -> ReportInput {
        let mut selfcheck = self.last.as_ref().map(|d| d.result.clone());
        if let Some(r) = selfcheck.as_mut() {
            self.fill_c1_c8(r);
        }
        ReportInput {
            app: AppFacts {
                version: crate::VERSION.to_owned(),
                arch: crate::TARGET
                    .split('-')
                    .next()
                    .unwrap_or("unknown")
                    .to_owned(),
            },
            tv: TvFacts {
                model: self.model_text(),
                webos_release: self.firmware.as_ref().and_then(|i| i.release.clone()),
                firmware: self.firmware_text(),
                kernel_arch: self.kernel.0.clone(),
                kernel_version: self.kernel.1.clone(),
            },
            policy: Some(self.policy()),
            audio: self.audio.clone().unwrap_or_default(),
            selfcheck,
            counters: self.last.as_ref().map(|d| d.counters),
            power: PowerFacts {
                states: self.power_states.clone(),
                screen_off_capture_seen: self.screen_off_capture,
            },
            tv_control: self.tv_control.clone(),
        }
    }

    /// Builds the report for `[report] repo`.
    pub(crate) fn build_report(&self) -> Result<Report, ReportError> {
        build(&self.report_input(), &self.repo)
    }

    fn report_status_for(r: &Result<Report, ReportError>) -> String {
        match r {
            Ok(_) => "ready: TV app > Report compatibility, or ctl.sh report".to_owned(),
            Err(ReportError::InvalidRepo) => "off: report.repo is not owner/name".to_owned(),
            Err(ReportError::Withheld(rule)) => {
                format!("withheld by the privacy scan ({})", rule.name())
            }
            Err(ReportError::TooLong) => {
                "report too long for a link; use ctl.sh report --full".to_owned()
            }
        }
    }

    /// Rebuilds the report and rewrites the status files when something
    /// changed. Returns the new `report_status` text when it changed.
    pub(crate) fn refresh_files(&mut self) -> Option<String> {
        if !self.report_on {
            return None;
        }
        let report = self.build_report();
        let status = Self::report_status_for(&report);
        let changed = status != self.report_status;
        self.report_status.clone_from(&status);
        let json = self.compat_json();
        let (report_json, short, full, url) = match &report {
            Ok(r) => (r.to_json(), r.short.clone(), r.full.clone(), r.url.clone()),
            Err(_) => (
                format!("{{\"schema\":1,\"error\":{}}}", json_str(&status)),
                String::new(),
                String::new(),
                String::new(),
            ),
        };
        let fingerprint = format!("{json}\n{report_json}");
        if self.written.as_deref() != Some(fingerprint.as_str()) {
            match self.write_files(&json, &report_json, &short, &full, &url) {
                Ok(()) => self.written = Some(fingerprint),
                Err(e) => log::warn!(
                    "compat: cannot write the status files in {}: {e}",
                    self.dir.display()
                ),
            }
        }
        changed.then_some(status)
    }

    fn write_files(
        &self,
        compat_json: &str,
        report_json: &str,
        short: &str,
        full: &str,
        url: &str,
    ) -> std::io::Result<()> {
        use crate::volume::write_atomic;
        std::fs::create_dir_all(&self.dir)?;
        let d = |n: &str| self.dir.join(n);
        let line = |s: &str| {
            if s.is_empty() {
                String::new()
            } else {
                format!("{s}\n")
            }
        };
        write_atomic(&d("report.json"), &line(report_json))?;
        write_atomic(&d("report.short.txt"), &line(short))?;
        write_atomic(&d("report.full.txt"), &line(full))?;
        write_atomic(&d("report.url"), &line(url))?;
        write_atomic(&d("compat.json"), &line(compat_json))
    }

    /// `compat.json`: the status for `ctl.sh status --json` and the app.
    pub(crate) fn compat_json(&self) -> String {
        let opt_str = |v: Option<String>| v.map_or_else(|| "null".to_owned(), |s| json_str(&s));
        let mut o = String::from("{\"schema\":1");
        let _ = write!(o, ",\"compat_status\":{}", json_str(self.status_text()));
        let _ = write!(o, ",\"selfcheck\":{}", json_str(&self.selfcheck_text()));
        let _ = write!(o, ",\"tv_model\":{}", opt_str(self.model_text()));
        let _ = write!(o, ",\"tv_firmware\":{}", opt_str(self.firmware_text()));
        let _ = write!(
            o,
            ",\"firmware_verified\":{}",
            self.verified_state()
                .map_or("null", |b| if b { "true" } else { "false" })
        );
        let _ = write!(
            o,
            ",\"firmware_changed\":{},\"capture_problem\":{}",
            self.firmware_changed().is_some(),
            self.capture_problem()
        );
        let _ = write!(
            o,
            ",\"policy\":{},\"report_status\":{}}}",
            json_str(if self.strict { "strict" } else { "warn" }),
            json_str(&self.report_status)
        );
        o
    }
}

pub(crate) fn json_str(s: &str) -> String {
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

/// Kernel architecture and numeric version. Only `machine` and `release` are
/// read from `uname`, never the node (host) name.
fn kernel_facts() -> (Option<String>, Option<String>) {
    let u = rustix::system::uname();
    let arch = sanitize_token(&u.machine().to_string_lossy());
    let release = u.release().to_string_lossy().into_owned();
    (arch, Some(release))
}

/// A static report for `webos-satellite --report` (no live self-check): the
/// firmware from nyx, the model from `luna-send` when present, the sound
/// cards from `/proc/asound` (read-only) and the policy result.
pub(crate) async fn static_report(settings: &Settings) -> Result<Report, ReportError> {
    let info = crate::firmware::read(&settings.safety.firmware_info);
    let luna = crate::power::find_binary(&settings.power.luna_send);
    let model = match luna {
        Some(l) => read_model(&l).await,
        None => None,
    };
    let model_state = model.map_or(ModelState::NotRead, ModelState::Known);
    let fm = match &model_state {
        ModelState::Known(m) => crate::firmware::Model::Known(m),
        ModelState::NotRead => crate::firmware::Model::NotRead,
    };
    let verdict = crate::firmware::evaluate(settings, info.clone(), fm);
    let mut c = Compat::new(settings, info, model_state, verdict);
    let asound = Path::new("/proc/asound");
    if let (Ok(cards), Ok(pcm)) = (
        std::fs::read_to_string(asound.join("cards")),
        std::fs::read_to_string(asound.join("pcm")),
    ) {
        let d = webos_compat::pcm::PcmDiscovery::parse(&cards, &pcm);
        c.audio = Some(AudioFacts::from_discovery(&d, None));
    }
    c.build_report()
}

/// The text `--report` prints.
pub(crate) fn render_report(r: &Report, full: bool, json: bool) -> String {
    if json {
        return r.to_json();
    }
    if full {
        return r.full.clone();
    }
    format!(
        "{}\n\nCreate the GitHub issue (prefilled; you review it and press Submit):\n{}",
        r.short, r.url
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::many_single_char_names)]

    use std::time::Duration;

    use webos_compat::engine::{Config, OpenOutcome, SelfCheck};
    use webos_compat::report::CaptureCounters;
    use webos_compat::window::FrameStats;

    use super::*;
    use crate::firmware::{Model, evaluate};

    fn info(v: &str) -> FirmwareInfo {
        FirmwareInfo {
            version: v.to_owned(),
            release: Some("8.3.0".to_owned()),
            build: Some("20240226022140".to_owned()),
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("webos-sat-compat-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make(dir: &Path, fw: &str, model: Option<&str>) -> Compat {
        let mut s = Settings::default();
        // No echo reference in these tests (C8 is covered by check_c8's
        // table): AEC off, so C8 is skipped.
        s.aec.enabled = false;
        s.report.dir = Some(dir.join("run"));
        s.safety.firmware_seen_file = Some(dir.join("firmware_seen"));
        let m = model.map_or(ModelState::NotRead, |m| ModelState::Known(m.to_owned()));
        let fm = match &m {
            ModelState::Known(m) => Model::Known(m),
            ModelState::NotRead => Model::NotRead,
        };
        let v = evaluate(&s, Ok(info(fw)), fm);
        Compat::new(&s, Ok(info(fw)), m, v)
    }

    /// A real engine run over `frames` of 10 ms.
    fn run_engine(stats: impl Fn(u32) -> FrameStats, secs: u64) -> Finished {
        let mut e = SelfCheck::new(Config::default());
        let t0 = Duration::ZERO;
        let _ = e.on_open(t0, OpenOutcome::Opened);
        let mut t = t0;
        let mut i = 0;
        while t < Duration::from_secs(secs) {
            let step = e.on_frame(t, &stats(i));
            if let Some(result) = step.result {
                return Finished {
                    result,
                    counters: CaptureCounters::default(),
                };
            }
            t += Duration::from_millis(10);
            i += 1;
        }
        panic!("no result");
    }

    fn noise(amp: i32) -> impl Fn(u32) -> FrameStats {
        move |i| {
            let mut seed = i.wrapping_mul(2_654_435_761).wrapping_add(7);
            let v: Vec<i32> = (0..320)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    i32::try_from(seed >> 8).unwrap_or(0) % (2 * amp + 1) - amp
                })
                .collect();
            FrameStats::from_samples(&v, 2, 24)
        }
    }

    fn state(c: &Compat, id: &str) -> Option<String> {
        c.commands().into_iter().find_map(|cmd| match cmd {
            AppCommand::SetTextSensor { object_id, state } if object_id == id => state,
            AppCommand::SetBinarySensor { object_id, state } if object_id == id => {
                state.map(|b| b.to_string())
            }
            _ => None,
        })
    }

    #[test]
    fn status_follows_the_policy_rows() {
        let d = tmp("status");
        let c = make(&d, "03.30.74", Some("OLED42C34LA"));
        assert_eq!(c.status_text(), "verified");
        assert_eq!(state(&c, FIRMWARE_VERIFIED).as_deref(), Some("true"));
        assert_eq!(state(&c, TV_FIRMWARE).as_deref(), Some("03.30.74"));
        assert_eq!(state(&c, TV_MODEL).as_deref(), Some("OLED42C34LA"));
        assert_eq!(
            state(&c, SELFCHECK).as_deref(),
            Some("not run (microphone closed)")
        );
        assert_eq!(state(&c, CAPTURE_PROBLEM).as_deref(), Some("false"));

        let c = make(&d, "03.40.00", Some("OLED65C3PUA"));
        assert_eq!(
            c.status_text(),
            "series verified (firmware not yet reported)"
        );
        assert_eq!(state(&c, FIRMWARE_VERIFIED).as_deref(), Some("false"));

        let c = make(&d, "99.99.99", Some("SOMETHING"));
        assert_eq!(c.status_text(), "not verified");

        // Model not read: unknown, and no value for the verified sensor.
        let c = make(&d, "03.30.74", None);
        assert_eq!(c.status_text(), "unknown");
        assert_eq!(state(&c, FIRMWARE_VERIFIED), None);
        assert_eq!(state(&c, TV_MODEL), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn selfcheck_results_drive_status_and_problem() {
        let d = tmp("results");
        let mut c = make(&d, "99.99.99", Some("X"));
        c.on_note(Note::Checking);
        assert_eq!(c.status_text(), "checking");
        assert_eq!(state(&c, SELFCHECK).as_deref(), Some("checking"));
        // A pass.
        c.on_note(Note::Result(Box::new(run_engine(noise(16_000), 13))));
        assert!(!c.capture_problem());
        assert_eq!(c.status_text(), "not verified");
        assert!(state(&c, SELFCHECK).unwrap().starts_with("passed"));
        // A stuck signal: failed, the status says capture problem.
        c.on_note(Note::Result(Box::new(run_engine(
            |_| FrameStats::from_samples(&[5; 320], 2, 24),
            13,
        ))));
        assert!(c.capture_problem());
        assert_eq!(c.status_text(), "capture problem");
        assert_eq!(state(&c, CAPTURE_PROBLEM).as_deref(), Some("true"));
        assert!(state(&c, SELFCHECK).unwrap().starts_with("failed:"));
        // A silent (very quiet) signal is a warning, not a problem.
        c.on_note(Note::Result(Box::new(run_engine(noise(1), 13))));
        assert!(!c.capture_problem());
        assert!(state(&c, SELFCHECK).unwrap().starts_with("warning:"));
        // Closed before a run finished.
        c.on_note(Note::Checking);
        c.on_note(Note::NotRun);
        assert_eq!(c.status_text(), "not verified");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn firmware_change_is_kept_until_acknowledged() {
        let d = tmp("change");
        // First start: remembered, no change.
        let mut c = make(&d, "03.30.74", Some("OLED42C34LA"));
        assert!(c.remember_first_start().unwrap().contains("remembered"));
        assert_eq!(c.firmware_changed(), None);
        // The firmware changes under it.
        let mut c = make(&d, "03.30.80", Some("OLED42C34LA"));
        assert_eq!(c.firmware_changed(), Some("03.30.74"));
        assert_eq!(c.status_text(), "firmware changed");
        assert_eq!(state(&c, FIRMWARE_CHANGED).as_deref(), Some("true"));
        assert!(c.remember_first_start().is_none(), "not written by itself");
        let r = c.report_input();
        assert_eq!(r.policy, Some(Policy::FirmwareChanged));
        // Another start still sees the change.
        let mut c2 = make(&d, "03.30.80", Some("OLED42C34LA"));
        assert_eq!(c2.status_text(), "firmware changed");
        assert!(c.acknowledge().contains("acknowledged"));
        assert_eq!(c.firmware_changed(), None);
        assert_eq!(
            c.status_text(),
            "series verified (firmware not yet reported)"
        );
        assert!(
            c2.acknowledge().contains("there was no change") || c2.firmware_changed().is_none()
        );
        let c3 = make(&d, "03.30.80", Some("OLED42C34LA"));
        assert_eq!(c3.firmware_changed(), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_report_files_are_written_and_the_link_decodes() {
        let d = tmp("files");
        let mut c = make(&d, "03.30.74", Some("OLED42C34LA"));
        c.on_note(Note::Result(Box::new(run_engine(noise(16_000), 13))));
        c.set_tv_control(vec![(ControlKind::Screen, ControlResult::Ok)]);
        c.on_power_state("Active", true);
        let status = c.refresh_files().unwrap();
        assert!(status.starts_with("ready"), "{status}");
        let run = d.join("run");
        let url = std::fs::read_to_string(run.join("report.url")).unwrap();
        let parsed = webos_compat::link::parse_issue_url(url.trim()).unwrap();
        assert_eq!(parsed.repo, webos_compat::link::DEFAULT_REPO);
        let short = std::fs::read_to_string(run.join("report.short.txt")).unwrap();
        assert_eq!(parsed.report.trim(), short.trim());
        assert!(short.contains("model=OLED42C34LA"), "{short}");
        assert!(short.contains("policy: verified"), "{short}");
        let full = std::fs::read_to_string(run.join("report.full.txt")).unwrap();
        assert!(full.contains("tv-control: screen=ok"), "{full}");
        let rj = std::fs::read_to_string(run.join("report.json")).unwrap();
        assert!(rj.starts_with("{\"schema\":1,\"verdict\":"), "{rj}");
        let cj = std::fs::read_to_string(run.join("compat.json")).unwrap();
        assert!(cj.contains("\"compat_status\":\"verified\""), "{cj}");
        assert!(cj.contains("\"tv_model\":\"OLED42C34LA\""), "{cj}");
        assert!(cj.contains("\"firmware_verified\":true"), "{cj}");
        assert!(cj.contains("\"policy\":\"warn\""), "{cj}");
        // Unchanged: no new status text.
        assert!(c.refresh_files().is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Privacy: nothing identifying reaches the report or the status files
    /// even when the settings and the inputs carry it.
    #[test]
    fn identifiers_never_reach_the_report_or_files() {
        let d = tmp("privacy");
        let mut s = Settings::default();
        s.report.dir = Some(d.join("run"));
        s.safety.firmware_seen_file = Some(d.join("firmware_seen"));
        s.device.name = "my-secret-host-name".to_owned();
        s.device.friendly_name = "Grandma's Living Room".to_owned();
        s.device.mac = Some("AA:BB:CC:DD:EE:FF".to_owned());
        s.device.noise_psk = Some("c2VjcmV0LWtleS1zZWNyZXQta2V5LXNlY3JldC1rZXk=".to_owned());
        s.device.bind = "192.168.77.5".parse().unwrap();
        let hostile_fw = FirmwareInfo {
            version: "03.30.74".to_owned(),
            release: Some("8.3.0".to_owned()),
            build: Some("serial-LGTV1234567890ABCDEF".to_owned()),
        };
        // A model reply that carries a serial number next to the name.
        let model = parse_model(
            r#"{"returnValue":true,"modelName":"OLED42C34LA","serialNumber":"SERIAL123456789XYZ","ip":"192.168.1.9"}"#,
        )
        .unwrap();
        assert_eq!(model, "OLED42C34LA");
        let v = evaluate(&s, Ok(hostile_fw.clone()), Model::Known(&model));
        let mut c = Compat::new(&s, Ok(hostile_fw), ModelState::Known(model), v);
        c.on_note(Note::Result(Box::new(run_engine(noise(16_000), 13))));
        c.on_power_state("Active", true);
        c.on_power_state("AA:BB:CC:DD:EE:FF", true);
        let _ = c.refresh_files();
        for name in [
            "report.json",
            "report.short.txt",
            "report.full.txt",
            "report.url",
            "compat.json",
        ] {
            let t = std::fs::read_to_string(d.join("run").join(name)).unwrap();
            for secret in [
                "my-secret-host-name",
                "Grandma",
                "AA:BB:CC",
                "AA%3ABB",
                "c2VjcmV0",
                "192.168",
                "SERIAL123",
                "LGTV1234",
            ] {
                assert!(!t.contains(secret), "{name} leaks {secret}: {t}");
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn model_parser_keeps_only_the_name() {
        assert_eq!(
            parse_model(r#"{"modelName":"OLED55C36LC","returnValue":true}"#).as_deref(),
            Some("OLED55C36LC")
        );
        // Whitespace and a terminal's line ends.
        assert_eq!(
            parse_model("{\r\n  \"modelName\" : \"OLED42C34LA\"\r\n}\r\n").as_deref(),
            Some("OLED42C34LA")
        );
        assert_eq!(parse_model("{}"), None);
        assert_eq!(parse_model("not json"), None);
        // An identifier-looking value is dropped whole.
        assert_eq!(parse_model(r#"{"modelName":"AA:BB:CC:DD:EE:FF"}"#), None);
    }

    #[test]
    fn disabled_self_check_has_no_problem_state() {
        let d = tmp("off");
        let mut s = Settings::default();
        s.safety.self_check = false;
        s.report.dir = Some(d.join("run"));
        s.safety.firmware_seen_file = Some(d.join("fs"));
        let v = evaluate(&s, Ok(info("03.30.74")), Model::Known("OLED42C34LA"));
        let c = Compat::new(
            &s,
            Ok(info("03.30.74")),
            ModelState::Known("OLED42C34LA".into()),
            v,
        );
        assert!(!c.capture_problem());
        assert!(state(&c, SELFCHECK).is_none() && state(&c, CAPTURE_PROBLEM).is_none());
        assert!(c.selfcheck_text().starts_with("not checked"));
        let mut esp = EspConfig::default();
        add_entities(&mut esp, false);
        assert!(esp.buttons.iter().all(|b| b.object_id != SELFCHECK_RUN));
        add_entities(&mut esp, true);
        assert!(esp.buttons.iter().any(|b| b.object_id == SELFCHECK_RUN));
        let _ = std::fs::remove_dir_all(&d);
    }
}
