//! Check identifiers, grades and reason codes (design section 3.2).
//!
//! Everything a check can say is a typed value: a report never carries a free
//! string from a check, only the fixed codes below.

use std::fmt;

/// The checks of the design table in section 3.2 that appear in a report
/// (C9 and C10 are reported as power and TV control facts instead).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CheckId {
    /// Firmware and model readable.
    C1,
    /// Capture PCM found by card id and capabilities.
    C2,
    /// Open succeeds.
    C3,
    /// Frames flow.
    C4,
    /// Level not stuck.
    C5,
    /// Level not silent.
    C6,
    /// Level not saturated.
    C7,
    /// Reference/playback levels (only with an AEC reference).
    C8,
}

impl CheckId {
    /// All checks in report order.
    pub const ALL: [CheckId; 8] = [
        CheckId::C1,
        CheckId::C2,
        CheckId::C3,
        CheckId::C4,
        CheckId::C5,
        CheckId::C6,
        CheckId::C7,
        CheckId::C8,
    ];

    /// Short label (`C1` to `C8`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            CheckId::C1 => "C1",
            CheckId::C2 => "C2",
            CheckId::C3 => "C3",
            CheckId::C4 => "C4",
            CheckId::C5 => "C5",
            CheckId::C6 => "C6",
            CheckId::C7 => "C7",
            CheckId::C8 => "C8",
        }
    }

    /// Whether a failure of this check is a "measured, nothing useful can come
    /// out" failure that closes the microphone (design 3.3: C2 to C5).
    #[must_use]
    pub const fn closes_mic_on_fail(self) -> bool {
        matches!(self, CheckId::C2 | CheckId::C3 | CheckId::C4 | CheckId::C5)
    }
}

impl fmt::Display for CheckId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Outcome of one check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Grade {
    /// Passed.
    Pass,
    /// Passed with a caveat; the microphone stays open.
    Warn,
    /// Measured failure.
    Fail,
    /// Not run or not applicable.
    Skipped,
}

impl Grade {
    /// Report word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Grade::Pass => "pass",
            Grade::Warn => "warn",
            Grade::Fail => "fail",
            Grade::Skipped => "skipped",
        }
    }
}

/// Fixed reason codes. The code strings only contain `[a-z-]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// C1: the model name could not be read.
    ModelMissing,
    /// C1: the firmware version could not be read.
    FirmwareMissing,
    /// C1: neither could be read.
    ModelFirmwareMissing,
    /// C2: no card with the configured id.
    NoCard,
    /// C2: the card has no capture PCM.
    NoCapturePcm,
    /// C2: the configured capture device is missing.
    DeviceMissing,
    /// C2: the device needs another format or rate than configured.
    FormatDiffers,
    /// C3: the PCM is busy (for example LG hands-free is on).
    Busy,
    /// C3: the open failed.
    OpenFailed,
    /// C3: the open worked only after retries.
    OpenRetried,
    /// C4: no frames arrived.
    NoFrames,
    /// C4: fewer frames than expected.
    FramesLow,
    /// C5: every input sample is identical (or within 1 LSB).
    Stuck,
    /// C6: the input peak is at or below the silence threshold.
    Quiet,
    /// C7: too many clipped samples.
    Clipping,
    /// C7: the RMS level is too high.
    Loud,
    /// C8: the reference stream is silent while the TV plays.
    ReferenceSilent,
    /// A check that needs frames could not run because none arrived.
    NotMeasured,
}

impl Reason {
    /// The fixed code written to a report.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Reason::ModelMissing => "model-missing",
            Reason::FirmwareMissing => "firmware-missing",
            Reason::ModelFirmwareMissing => "model-firmware-missing",
            Reason::NoCard => "no-card",
            Reason::NoCapturePcm => "no-capture-pcm",
            Reason::DeviceMissing => "device-missing",
            Reason::FormatDiffers => "format-differs",
            Reason::Busy => "busy",
            Reason::OpenFailed => "open-failed",
            Reason::OpenRetried => "open-retried",
            Reason::NoFrames => "no-frames",
            Reason::FramesLow => "frames-low",
            Reason::Stuck => "stuck",
            Reason::Quiet => "quiet",
            Reason::Clipping => "clipping",
            Reason::Loud => "loud",
            Reason::ReferenceSilent => "reference-silent",
            Reason::NotMeasured => "not-measured",
        }
    }

    /// Short human phrase for the `selfcheck` text entity.
    #[must_use]
    pub const fn phrase(self) -> &'static str {
        match self {
            Reason::ModelMissing => "TV model unknown",
            Reason::FirmwareMissing => "TV firmware unknown",
            Reason::ModelFirmwareMissing => "TV model and firmware unknown",
            Reason::NoCard | Reason::NoCapturePcm => "no capture device",
            Reason::DeviceMissing => "capture device missing",
            Reason::FormatDiffers => "format differs",
            Reason::Busy => "microphone busy (LG hands-free on?)",
            Reason::OpenFailed => "cannot open microphone",
            Reason::OpenRetried => "opened after retries",
            Reason::NoFrames => "no audio frames",
            Reason::FramesLow => "frames missing",
            Reason::Stuck => "signal stuck",
            Reason::Quiet => "very quiet",
            Reason::Clipping => "clipping",
            Reason::Loud => "very loud",
            Reason::ReferenceSilent => "reference silent",
            Reason::NotMeasured => "not measured",
        }
    }
}

/// One check result: id, grade and an optional reason code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckResult {
    /// Which check.
    pub id: CheckId,
    /// Outcome.
    pub grade: Grade,
    /// Why, when not a plain pass.
    pub reason: Option<Reason>,
}

impl CheckResult {
    /// A pass.
    #[must_use]
    pub const fn pass(id: CheckId) -> Self {
        Self {
            id,
            grade: Grade::Pass,
            reason: None,
        }
    }

    /// A warning with a reason.
    #[must_use]
    pub const fn warn(id: CheckId, reason: Reason) -> Self {
        Self {
            id,
            grade: Grade::Warn,
            reason: Some(reason),
        }
    }

    /// A failure with a reason.
    #[must_use]
    pub const fn fail(id: CheckId, reason: Reason) -> Self {
        Self {
            id,
            grade: Grade::Fail,
            reason: Some(reason),
        }
    }

    /// Not run.
    #[must_use]
    pub const fn skipped(id: CheckId) -> Self {
        Self {
            id,
            grade: Grade::Skipped,
            reason: None,
        }
    }

    /// Not measurable because of an earlier failure.
    #[must_use]
    pub const fn not_measured(id: CheckId) -> Self {
        Self {
            id,
            grade: Grade::Skipped,
            reason: Some(Reason::NotMeasured),
        }
    }

    /// Report text, for example `pass`, `warn(quiet)`.
    #[must_use]
    pub fn text(&self) -> String {
        match self.reason {
            Some(r) => format!("{}({})", self.grade.as_str(), r.code()),
            None => self.grade.as_str().to_string(),
        }
    }
}

/// C1: firmware and model readable. Never a failure (status `unknown`).
#[must_use]
pub const fn check_c1(model_known: bool, firmware_known: bool) -> CheckResult {
    match (model_known, firmware_known) {
        (true, true) => CheckResult::pass(CheckId::C1),
        (false, true) => CheckResult::warn(CheckId::C1, Reason::ModelMissing),
        (true, false) => CheckResult::warn(CheckId::C1, Reason::FirmwareMissing),
        (false, false) => CheckResult::warn(CheckId::C1, Reason::ModelFirmwareMissing),
    }
}

/// C8: reference stream present. Skipped without a configured reference.
#[must_use]
pub const fn check_c8(
    reference_configured: bool,
    reference_present: bool,
    tv_playing: bool,
) -> CheckResult {
    if !reference_configured {
        CheckResult::skipped(CheckId::C8)
    } else if reference_present || !tv_playing {
        CheckResult::pass(CheckId::C8)
    } else {
        CheckResult::warn(CheckId::C8, Reason::ReferenceSilent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c1_table() {
        assert_eq!(check_c1(true, true).grade, Grade::Pass);
        assert_eq!(check_c1(false, true).reason, Some(Reason::ModelMissing));
        assert_eq!(check_c1(true, false).reason, Some(Reason::FirmwareMissing));
        let both = check_c1(false, false);
        assert_eq!(both.grade, Grade::Warn, "C1 is never a failure");
        assert_eq!(both.reason, Some(Reason::ModelFirmwareMissing));
    }

    #[test]
    fn c8_table() {
        assert_eq!(check_c8(false, false, true).grade, Grade::Skipped);
        assert_eq!(check_c8(true, true, true).grade, Grade::Pass);
        assert_eq!(check_c8(true, false, false).grade, Grade::Pass);
        assert_eq!(check_c8(true, false, true).grade, Grade::Warn);
    }

    #[test]
    fn text_and_codes() {
        assert_eq!(CheckResult::pass(CheckId::C2).text(), "pass");
        assert_eq!(
            CheckResult::warn(CheckId::C6, Reason::Quiet).text(),
            "warn(quiet)"
        );
        assert_eq!(CheckId::C7.to_string(), "C7");
    }

    #[test]
    fn reason_codes_are_plain_tokens() {
        for r in [
            Reason::ModelMissing,
            Reason::FirmwareMissing,
            Reason::ModelFirmwareMissing,
            Reason::NoCard,
            Reason::NoCapturePcm,
            Reason::DeviceMissing,
            Reason::FormatDiffers,
            Reason::Busy,
            Reason::OpenFailed,
            Reason::OpenRetried,
            Reason::NoFrames,
            Reason::FramesLow,
            Reason::Stuck,
            Reason::Quiet,
            Reason::Clipping,
            Reason::Loud,
            Reason::ReferenceSilent,
            Reason::NotMeasured,
        ] {
            assert!(
                r.code()
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'-')
            );
            assert!(!r.phrase().is_empty() && r.phrase().len() < 80);
        }
    }

    #[test]
    fn only_c2_to_c5_close_the_mic() {
        let closing: Vec<_> = CheckId::ALL
            .iter()
            .filter(|c| c.closes_mic_on_fail())
            .collect();
        assert_eq!(closing.len(), 4);
    }
}
