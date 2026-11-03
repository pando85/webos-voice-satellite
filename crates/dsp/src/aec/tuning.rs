//! Experimental AEC3 parameter overrides (`[aec.aec3]`, `aec_offline --aec3`).
//!
//! A small, named subset of sonora-aec3's `EchoCanceller3Config`: the
//! residual-echo suppressor's masking thresholds (normal and near-end
//! tuning), the dominant near-end detector, the echo-path strength model,
//! echo audibility, the comfort-noise floor, the ERLE bounds and the filter
//! lengths. Names are the config's field paths. Any override runs the
//! canceller through the direct engine (`direct.rs`), because sonora's
//! `AudioProcessing` takes AEC3's defaults only. With no override the stage
//! is unchanged.
//!
//! One extra knob is not an AEC3 field: `linear_mix` (0 to 1) blends the
//! linear filter's output, aligned to the final output, into the final
//! output: `(1 - m) * final + m * linear`, which bounds the suppressor's
//! attenuation of everything (talker and residual echo) at about
//! `20 * log10(m)` dB.
//!
//! Every value is range-checked against sonora-aec3's own validation limits;
//! integer and boolean (0 or 1) parameters must be whole numbers.

use sonora_aec3::config::{EchoCanceller3Config, MaskingThresholds, Tuning};

/// Largest power value AEC3 accepts (S16 full scale squared).
const MAX_POWER: f32 = 32_768.0 * 32_768.0;

/// What kind of value a parameter takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Float,
    Int,
    Bool,
}

macro_rules! params {
    ($( $(#[$m:meta])* $v:ident => $name:literal, $kind:ident, $lo:expr, $hi:expr; )+) => {
        /// One overridable AEC3 parameter (see the module docs).
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[allow(missing_docs)] // the names are the documentation
        pub enum Aec3Param {
            $( $(#[$m])* $v, )+
        }

        impl Aec3Param {
            /// Every parameter, in declaration order.
            pub const ALL: &'static [Aec3Param] = &[ $( Aec3Param::$v, )+ ];

            /// The parameter's name (sonora-aec3's config field path).
            #[must_use]
            pub fn name(self) -> &'static str {
                match self { $( Aec3Param::$v => $name, )+ }
            }

            fn kind(self) -> Kind {
                match self { $( Aec3Param::$v => Kind::$kind, )+ }
            }

            /// Accepted range (inclusive).
            #[must_use]
            pub fn range(self) -> (f32, f32) {
                match self { $( Aec3Param::$v => ($lo, $hi), )+ }
            }
        }
    };
}

params! {
    NormalLfEnrTransparent => "suppressor.normal_tuning.mask_lf.enr_transparent", Float, 0.0, 100.0;
    NormalLfEnrSuppress => "suppressor.normal_tuning.mask_lf.enr_suppress", Float, 0.0, 100.0;
    NormalLfEmrTransparent => "suppressor.normal_tuning.mask_lf.emr_transparent", Float, 0.0, 100.0;
    NormalHfEnrTransparent => "suppressor.normal_tuning.mask_hf.enr_transparent", Float, 0.0, 100.0;
    NormalHfEnrSuppress => "suppressor.normal_tuning.mask_hf.enr_suppress", Float, 0.0, 100.0;
    NormalHfEmrTransparent => "suppressor.normal_tuning.mask_hf.emr_transparent", Float, 0.0, 100.0;
    NormalMaxIncFactor => "suppressor.normal_tuning.max_inc_factor", Float, 0.0, 100.0;
    NormalMaxDecFactorLf => "suppressor.normal_tuning.max_dec_factor_lf", Float, 0.0, 100.0;
    NearendLfEnrTransparent => "suppressor.nearend_tuning.mask_lf.enr_transparent", Float, 0.0, 100.0;
    NearendLfEnrSuppress => "suppressor.nearend_tuning.mask_lf.enr_suppress", Float, 0.0, 100.0;
    NearendLfEmrTransparent => "suppressor.nearend_tuning.mask_lf.emr_transparent", Float, 0.0, 100.0;
    NearendHfEnrTransparent => "suppressor.nearend_tuning.mask_hf.enr_transparent", Float, 0.0, 100.0;
    NearendHfEnrSuppress => "suppressor.nearend_tuning.mask_hf.enr_suppress", Float, 0.0, 100.0;
    NearendHfEmrTransparent => "suppressor.nearend_tuning.mask_hf.emr_transparent", Float, 0.0, 100.0;
    NearendMaxIncFactor => "suppressor.nearend_tuning.max_inc_factor", Float, 0.0, 100.0;
    NearendMaxDecFactorLf => "suppressor.nearend_tuning.max_dec_factor_lf", Float, 0.0, 100.0;
    DneEnrThreshold => "suppressor.dominant_nearend_detection.enr_threshold", Float, 0.0, 1.0e6;
    DneEnrExitThreshold => "suppressor.dominant_nearend_detection.enr_exit_threshold", Float, 0.0, 1.0e6;
    DneSnrThreshold => "suppressor.dominant_nearend_detection.snr_threshold", Float, 0.0, 1.0e6;
    DneHoldDuration => "suppressor.dominant_nearend_detection.hold_duration", Int, 0.0, 10_000.0;
    DneTriggerThreshold => "suppressor.dominant_nearend_detection.trigger_threshold", Int, 0.0, 10_000.0;
    EpDefaultGain => "ep_strength.default_gain", Float, 0.0, 1.0e6;
    EpDefaultLen => "ep_strength.default_len", Float, -1.0, 1.0;
    EpNearendLen => "ep_strength.nearend_len", Float, -1.0, 1.0;
    EpBoundedErl => "ep_strength.bounded_erl", Bool, 0.0, 1.0;
    AudLowRenderLimit => "echo_audibility.low_render_limit", Float, 0.0, MAX_POWER;
    AudNormalRenderLimit => "echo_audibility.normal_render_limit", Float, 0.0, MAX_POWER;
    AudFloorPower => "echo_audibility.floor_power", Float, 0.0, MAX_POWER;
    AudThresholdLf => "echo_audibility.audibility_threshold_lf", Float, 0.0, MAX_POWER;
    AudThresholdMf => "echo_audibility.audibility_threshold_mf", Float, 0.0, MAX_POWER;
    AudThresholdHf => "echo_audibility.audibility_threshold_hf", Float, 0.0, MAX_POWER;
    AudUseStationarity => "echo_audibility.use_stationarity_properties", Bool, 0.0, 1.0;
    ComfortNoiseFloorDbfs => "comfort_noise.noise_floor_dbfs", Float, -200.0, 0.0;
    ErleMin => "erle.min", Float, 1.0, 1.0e5;
    ErleMaxL => "erle.max_l", Float, 1.0, 1.0e5;
    ErleMaxH => "erle.max_h", Float, 1.0, 1.0e5;
    RefinedLengthBlocks => "filter.refined.length_blocks", Int, 1.0, 100.0;
    CoarseLengthBlocks => "filter.coarse.length_blocks", Int, 1.0, 100.0;
    /// Not an AEC3 field: share of the aligned linear output blended into
    /// the final output.
    LinearMix => "linear_mix", Float, 0.0, 1.0;
}

impl Aec3Param {
    /// Looks a parameter up by name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|p| p.name() == name)
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|&p| p == self).unwrap_or(0)
    }

    #[allow(clippy::many_single_char_names)] // short paths keep the table readable
    fn apply(self, c: &mut EchoCanceller3Config, v: f32) {
        fn mask(t: &mut Tuning, hf: bool) -> &mut MaskingThresholds {
            if hf { &mut t.mask_hf } else { &mut t.mask_lf }
        }
        let s = &mut c.suppressor;
        let (n, ne) = (&mut s.normal_tuning, &mut s.nearend_tuning);
        let d = &mut s.dominant_nearend_detection;
        let int = v.round() as i32;
        let len = v.round().max(1.0) as usize;
        let on = v >= 0.5;
        match self {
            Self::NormalLfEnrTransparent => mask(n, false).enr_transparent = v,
            Self::NormalLfEnrSuppress => mask(n, false).enr_suppress = v,
            Self::NormalLfEmrTransparent => mask(n, false).emr_transparent = v,
            Self::NormalHfEnrTransparent => mask(n, true).enr_transparent = v,
            Self::NormalHfEnrSuppress => mask(n, true).enr_suppress = v,
            Self::NormalHfEmrTransparent => mask(n, true).emr_transparent = v,
            Self::NormalMaxIncFactor => n.max_inc_factor = v,
            Self::NormalMaxDecFactorLf => n.max_dec_factor_lf = v,
            Self::NearendLfEnrTransparent => mask(ne, false).enr_transparent = v,
            Self::NearendLfEnrSuppress => mask(ne, false).enr_suppress = v,
            Self::NearendLfEmrTransparent => mask(ne, false).emr_transparent = v,
            Self::NearendHfEnrTransparent => mask(ne, true).enr_transparent = v,
            Self::NearendHfEnrSuppress => mask(ne, true).enr_suppress = v,
            Self::NearendHfEmrTransparent => mask(ne, true).emr_transparent = v,
            Self::NearendMaxIncFactor => ne.max_inc_factor = v,
            Self::NearendMaxDecFactorLf => ne.max_dec_factor_lf = v,
            Self::DneEnrThreshold => d.enr_threshold = v,
            Self::DneEnrExitThreshold => d.enr_exit_threshold = v,
            Self::DneSnrThreshold => d.snr_threshold = v,
            Self::DneHoldDuration => d.hold_duration = int,
            Self::DneTriggerThreshold => d.trigger_threshold = int,
            Self::EpDefaultGain => c.ep_strength.default_gain = v,
            Self::EpDefaultLen => c.ep_strength.default_len = v,
            Self::EpNearendLen => c.ep_strength.nearend_len = v,
            Self::EpBoundedErl => c.ep_strength.bounded_erl = on,
            Self::AudLowRenderLimit => c.echo_audibility.low_render_limit = v,
            Self::AudNormalRenderLimit => c.echo_audibility.normal_render_limit = v,
            Self::AudFloorPower => c.echo_audibility.floor_power = v,
            Self::AudThresholdLf => c.echo_audibility.audibility_threshold_lf = v,
            Self::AudThresholdMf => c.echo_audibility.audibility_threshold_mf = v,
            Self::AudThresholdHf => c.echo_audibility.audibility_threshold_hf = v,
            Self::AudUseStationarity => c.echo_audibility.use_stationarity_properties = on,
            Self::ComfortNoiseFloorDbfs => c.comfort_noise.noise_floor_dbfs = v,
            Self::ErleMin => c.erle.min = v,
            Self::ErleMaxL => c.erle.max_l = v,
            Self::ErleMaxH => c.erle.max_h = v,
            Self::RefinedLengthBlocks => {
                c.filter.refined.length_blocks = len;
                c.filter.refined_initial.length_blocks =
                    c.filter.refined_initial.length_blocks.min(len);
            }
            Self::CoarseLengthBlocks => {
                c.filter.coarse.length_blocks = len;
                c.filter.coarse_initial.length_blocks =
                    c.filter.coarse_initial.length_blocks.min(len);
            }
            Self::LinearMix => {}
        }
    }
}

/// A set of AEC3 overrides; empty by default (AEC3's own defaults).
#[derive(Clone, Copy, PartialEq)]
pub struct Aec3Tuning {
    values: [Option<f32>; Aec3Param::ALL.len()],
}

impl Default for Aec3Tuning {
    fn default() -> Self {
        Self {
            values: [None; Aec3Param::ALL.len()],
        }
    }
}

impl std::fmt::Debug for Aec3Tuning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.iter().map(|(p, v)| (p.name(), v)))
            .finish()
    }
}

impl std::fmt::Display for Aec3Tuning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut first = true;
        for (p, v) in self.iter() {
            if !first {
                f.write_str(",")?;
            }
            first = false;
            write!(f, "{}={v}", p.name())?;
        }
        Ok(())
    }
}

impl Aec3Tuning {
    /// Sets `param` to `value` after checking its range and kind.
    ///
    /// # Errors
    ///
    /// Out of range, not finite, or not a whole number (0 or 1 for a
    /// switch) where one is needed.
    pub fn set(&mut self, param: Aec3Param, value: f32) -> Result<(), String> {
        let (lo, hi) = param.range();
        if !value.is_finite() || value < lo || value > hi {
            return Err(format!("{}: {value} is outside {lo} to {hi}", param.name()));
        }
        if param.kind() != Kind::Float && value.fract() != 0.0 {
            return Err(format!("{}: {value} must be a whole number", param.name()));
        }
        self.values[param.index()] = Some(value);
        Ok(())
    }

    /// Parses and sets `name=value`.
    ///
    /// # Errors
    ///
    /// Unknown name, unparsable value, or a value [`Aec3Tuning::set`]
    /// rejects.
    pub fn set_assignment(&mut self, s: &str) -> Result<(), String> {
        let (name, value) = s
            .split_once('=')
            .ok_or_else(|| format!("{s:?}: expected name=value"))?;
        let p = Aec3Param::from_name(name.trim())
            .ok_or_else(|| format!("{name:?}: unknown AEC3 parameter"))?;
        let v: f32 = value
            .trim()
            .parse()
            .map_err(|e| format!("{}: {e}", p.name()))?;
        self.set(p, v)
    }

    /// The override of `param`, if any.
    #[must_use]
    pub fn get(&self, param: Aec3Param) -> Option<f32> {
        self.values[param.index()]
    }

    /// True if nothing is overridden.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.iter().all(Option::is_none)
    }

    /// The overrides in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = (Aec3Param, f32)> + '_ {
        Aec3Param::ALL
            .iter()
            .zip(&self.values)
            .filter_map(|(&p, v)| v.map(|v| (p, v)))
    }

    /// Writes the AEC3 overrides into `cfg`.
    pub(crate) fn apply(&self, cfg: &mut EchoCanceller3Config) {
        for (p, v) in self.iter() {
            p.apply(cfg, v);
        }
    }

    /// The linear-output share (0 if not set).
    pub(crate) fn linear_mix(&self) -> f32 {
        self.get(Aec3Param::LinearMix).unwrap_or(0.0)
    }
}
