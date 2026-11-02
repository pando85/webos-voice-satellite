//! ALSA capture through raw PCM ioctls (Linux only).

use std::io;
use std::path::PathBuf;

use crate::proc::{PcmInfo, list_pcms_in};
use crate::source::AudioSource;
use crate::sys::{HwActual, HwRequest, Pcm};
use crate::{CaptureError, file::sign_extend_s24};

/// Consecutive recoveries without a successful read before giving up.
const MAX_CONSECUTIVE_XRUNS: u32 = 8;

/// Configuration for [`AlsaCapture::open`].
///
/// The defaults are the verified values for the LG C3 (c3-a): card `LGSIC`,
/// PCM "WoV PDM Mic", 2 channels, 16 kHz, period 256 and buffer 8192 frames.
/// Other buffer sizes are rejected by that driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureConfig {
    /// Card id as listed in `/proc/asound/cards` (not the index).
    pub card_id: String,
    /// Substring of the PCM name or id that selects the device on the card.
    /// Ignored when `device` is set.
    pub pcm_name: Option<String>,
    /// Explicit PCM device number. Overrides `pcm_name`.
    pub device: Option<u32>,
    /// Sample rate in Hz.
    pub rate: u32,
    /// Interleaved channels.
    pub channels: u16,
    /// Period size in frames.
    pub period_frames: u32,
    /// Ring buffer size in frames.
    pub buffer_frames: u32,
    /// Milliseconds of audio dropped after every start (and after every
    /// overrun recovery): the stream begins with a full-scale burst of about
    /// 5 ms.
    pub discard_ms: u32,
    /// A read that sees no audio for this long fails with
    /// [`CaptureError::Stalled`] instead of blocking forever (0 disables the
    /// timeout). A period is 16 ms at the c3-a settings.
    pub read_timeout_ms: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            card_id: "LGSIC".to_owned(),
            pcm_name: Some("WoV PDM Mic".to_owned()),
            device: None,
            rate: 16_000,
            channels: 2,
            period_frames: 256,
            buffer_frames: 8192,
            discard_ms: 250,
            read_timeout_ms: 5000,
        }
    }
}

/// A capture stream on an ALSA PCM, S24_LE interleaved.
#[derive(Debug)]
pub struct AlsaCapture {
    pcm: Pcm,
    card: u32,
    device: u32,
    hw: HwActual,
    channels: u16,
    discard_frames: u64,
    discard_left: u64,
    xruns: u64,
    read_timeout_ms: u32,
}

fn select_pcm(cfg: &CaptureConfig, pcms: &[PcmInfo]) -> Result<PcmInfo, CaptureError> {
    let on_card: Vec<&PcmInfo> = pcms
        .iter()
        .filter(|p| p.capture && p.card_id == cfg.card_id)
        .collect();
    if !pcms.iter().any(|p| p.card_id == cfg.card_id) {
        return Err(CaptureError::DeviceNotFound(format!(
            "no sound card with id {:?}",
            cfg.card_id
        )));
    }
    let found = if let Some(dev) = cfg.device {
        on_card.iter().find(|p| p.device == dev)
    } else if let Some(name) = &cfg.pcm_name {
        on_card.iter().find(|p| p.matches_name(name))
    } else {
        on_card.first()
    };
    found.map(|p| (*p).clone()).ok_or_else(|| {
        CaptureError::DeviceNotFound(format!(
            "card {:?} has no capture PCM matching device={:?} name={:?}",
            cfg.card_id, cfg.device, cfg.pcm_name
        ))
    })
}

fn map_open_error(err: io::Error) -> CaptureError {
    match err.raw_os_error() {
        Some(libc::EBUSY | libc::EAGAIN) => CaptureError::MicBusy,
        Some(libc::ENOENT | libc::ENODEV | libc::ENXIO) => {
            CaptureError::DeviceNotFound(format!("cannot open the PCM node: {err}"))
        }
        _ => CaptureError::Io(err),
    }
}

impl AlsaCapture {
    /// Resolves the card by id and the PCM by name, opens it, installs the
    /// parameters and starts the stream.
    ///
    /// # Errors
    /// - [`CaptureError::MicBusy`] if another process holds the PCM.
    /// - [`CaptureError::DeviceNotFound`] if no card or PCM matches.
    /// - [`CaptureError::Unsupported`] if the driver rejects the parameters.
    /// - [`CaptureError::Io`] otherwise.
    pub fn open(cfg: &CaptureConfig) -> Result<Self, CaptureError> {
        if cfg.channels == 0 || cfg.rate == 0 || cfg.period_frames == 0 || cfg.buffer_frames == 0 {
            return Err(CaptureError::Unsupported(
                "channels, rate, period_frames and buffer_frames must be non-zero".into(),
            ));
        }
        let info = select_pcm(cfg, &list_pcms_in(std::path::Path::new("/proc/asound"))?)?;
        let path = PathBuf::from(format!("/dev/snd/pcmC{}D{}c", info.card, info.device));
        let pcm = Pcm::open(&path).map_err(map_open_error)?;
        // Informational only; failure means this is not a PCM node.
        pcm.pversion().map_err(|e| {
            CaptureError::Unsupported(format!("PCM protocol version query failed: {e}"))
        })?;

        let req = HwRequest {
            channels: u32::from(cfg.channels),
            rate: cfg.rate,
            period_frames: cfg.period_frames,
            buffer_frames: cfg.buffer_frames,
        };
        let hw = pcm.configure(req).map_err(|e| {
            CaptureError::Unsupported(format!(
                "driver rejected S24_LE/{} ch/{} Hz/period {}/buffer {}: {e}",
                cfg.channels, cfg.rate, cfg.period_frames, cfg.buffer_frames
            ))
        })?;
        if hw.channels != req.channels || hw.rate != req.rate {
            return Err(CaptureError::Unsupported(format!(
                "driver chose {} ch / {} Hz instead of {} ch / {} Hz",
                hw.channels, hw.rate, req.channels, req.rate
            )));
        }
        pcm.prepare()?;
        pcm.start()?;
        let discard_frames = u64::from(cfg.rate) * u64::from(cfg.discard_ms) / 1000;
        Ok(Self {
            pcm,
            card: info.card,
            device: info.device,
            hw,
            channels: cfg.channels,
            discard_frames,
            discard_left: discard_frames,
            xruns: 0,
            read_timeout_ms: cfg.read_timeout_ms,
        })
    }

    /// Resolved card index (diagnostics only).
    #[must_use]
    pub fn card_index(&self) -> u32 {
        self.card
    }

    /// Resolved PCM device number (diagnostics only).
    #[must_use]
    pub fn device_number(&self) -> u32 {
        self.device
    }

    /// Period size the driver configured, in frames.
    #[must_use]
    pub fn period_frames(&self) -> u32 {
        self.hw.period_frames
    }

    /// Buffer size the driver configured, in frames.
    #[must_use]
    pub fn buffer_frames(&self) -> u32 {
        self.hw.buffer_frames
    }

    /// Overruns recovered since the stream started.
    #[must_use]
    pub fn xruns(&self) -> u64 {
        self.xruns
    }

    /// Re-prepares and restarts the stream after an overrun or suspend, and
    /// re-arms the startup discard.
    fn recover(&mut self) -> io::Result<()> {
        self.xruns += 1;
        self.pcm.prepare()?;
        self.pcm.start()?;
        self.discard_left = self.discard_frames;
        Ok(())
    }

    /// One READI. `Ok(None)` means the stream was recovered and the caller
    /// must restart its logic (the startup discard is re-armed).
    fn read_once(
        &mut self,
        buf: &mut [i32],
        frames: usize,
        consecutive: &mut u32,
    ) -> Result<Option<usize>, CaptureError> {
        loop {
            if self.read_timeout_ms > 0 && !self.pcm.wait_readable(self.read_timeout_ms)? {
                return Err(CaptureError::Stalled(self.read_timeout_ms));
            }
            match self.pcm.readi(buf, usize::from(self.channels), frames) {
                Ok(n) => {
                    *consecutive = 0;
                    return Ok(Some(n));
                }
                Err(e) => match e.raw_os_error() {
                    Some(libc::EINTR) => {}
                    Some(libc::EPIPE | libc::ESTRPIPE) => {
                        *consecutive += 1;
                        if *consecutive > MAX_CONSECUTIVE_XRUNS {
                            return Err(CaptureError::XrunRecoveryFailed {
                                attempts: *consecutive - 1,
                                source: e,
                            });
                        }
                        self.recover()
                            .map_err(|source| CaptureError::XrunRecoveryFailed {
                                attempts: *consecutive,
                                source,
                            })?;
                        return Ok(None);
                    }
                    Some(libc::ENODEV) => return Err(CaptureError::Disconnected),
                    _ => return Err(CaptureError::Io(e)),
                },
            }
        }
    }
}

impl AudioSource for AlsaCapture {
    fn read(&mut self, out: &mut [i32]) -> Result<usize, CaptureError> {
        let ch = usize::from(self.channels);
        let frames = out.len() / ch;
        if frames == 0 {
            return Ok(0);
        }
        let mut consecutive = 0;
        loop {
            // Drop the startup burst, using the caller's buffer as scratch.
            while self.discard_left > 0 {
                let want = usize::try_from(self.discard_left).map_or(frames, |d| d.min(frames));
                // `None`: recovered, discard re-armed, loop again.
                if let Some(n) = self.read_once(out, want, &mut consecutive)? {
                    self.discard_left = self.discard_left.saturating_sub(n as u64);
                }
            }
            if let Some(n) = self.read_once(out, frames, &mut consecutive)? {
                for s in &mut out[..n * ch] {
                    *s = sign_extend_s24(*s);
                }
                return Ok(n);
            }
        }
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn rate(&self) -> u32 {
        self.hw.rate
    }

    fn xruns(&self) -> u64 {
        self.xruns
    }
}

impl Drop for AlsaCapture {
    fn drop(&mut self) {
        // Stop the DMA cleanly; closing the fd would do it too.
        let _ = self.pcm.drop_stream();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(card: u32, id: &str, device: u32, capture: bool, name: &str) -> PcmInfo {
        PcmInfo {
            card,
            card_id: id.to_owned(),
            device,
            capture,
            id: format!("{name} dai"),
            name: name.to_owned(),
            subdevices: 1,
        }
    }

    fn fixture() -> Vec<PcmInfo> {
        vec![
            pcm(0, "lg115x", 6, false, "lg115x.6"),
            pcm(1, "LGSIC", 0, true, "WoV PDM Mic"),
            pcm(1, "LGSIC", 1, true, "SpeakerFeedback"),
            pcm(1, "LGSIC", 2, false, "Out"),
        ]
    }

    #[test]
    fn selects_by_name_not_index() {
        let p = select_pcm(&CaptureConfig::default(), &fixture()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((p.card, p.device), (1, 0));
        // The same names with a different card order still resolve.
        let mut moved = fixture();
        for p in &mut moved {
            p.card = if p.card == 1 { 5 } else { 2 };
        }
        let p = select_pcm(&CaptureConfig::default(), &moved).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((p.card, p.device), (5, 0));
    }

    #[test]
    fn explicit_device_overrides_name() {
        let cfg = CaptureConfig {
            device: Some(1),
            ..CaptureConfig::default()
        };
        assert_eq!(select_pcm(&cfg, &fixture()).map(|p| p.device).ok(), Some(1));
        let cfg = CaptureConfig {
            pcm_name: Some("Speaker".into()),
            ..CaptureConfig::default()
        };
        assert_eq!(select_pcm(&cfg, &fixture()).map(|p| p.device).ok(), Some(1));
        let cfg = CaptureConfig {
            pcm_name: None,
            ..CaptureConfig::default()
        };
        assert_eq!(select_pcm(&cfg, &fixture()).map(|p| p.device).ok(), Some(0));
    }

    #[test]
    fn missing_card_or_pcm_is_not_found() {
        let cfg = CaptureConfig {
            card_id: "nope".into(),
            ..CaptureConfig::default()
        };
        assert!(matches!(
            select_pcm(&cfg, &fixture()),
            Err(CaptureError::DeviceNotFound(_))
        ));
        // Playback-only device number does not count.
        let cfg = CaptureConfig {
            device: Some(2),
            ..CaptureConfig::default()
        };
        assert!(matches!(
            select_pcm(&cfg, &fixture()),
            Err(CaptureError::DeviceNotFound(_))
        ));
    }

    #[test]
    fn ebusy_maps_to_mic_busy() {
        assert!(matches!(
            map_open_error(io::Error::from_raw_os_error(libc::EBUSY)),
            CaptureError::MicBusy
        ));
        assert!(matches!(
            map_open_error(io::Error::from_raw_os_error(libc::ENOENT)),
            CaptureError::DeviceNotFound(_)
        ));
        assert!(matches!(
            map_open_error(io::Error::from_raw_os_error(libc::EACCES)),
            CaptureError::Io(_)
        ));
    }

    #[test]
    fn default_config_is_the_verified_c3_one() {
        let c = CaptureConfig::default();
        assert_eq!(c.card_id, "LGSIC");
        assert_eq!(c.pcm_name.as_deref(), Some("WoV PDM Mic"));
        assert_eq!(
            (
                c.rate,
                c.channels,
                c.period_frames,
                c.buffer_frames,
                c.discard_ms,
                c.read_timeout_ms
            ),
            (16_000, 2, 256, 8192, 250, 5000)
        );
    }
}
