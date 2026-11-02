//! ALSA capture through raw PCM ioctls (Linux only).

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::proc::{PcmInfo, list_pcms_in};
use crate::source::{AudioSource, ReferenceState, SampleFormat, StreamFormat};
use crate::sys::{Format, HwActual, HwRequest, Pcm};
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

fn format_name(f: Format) -> &'static str {
    match f {
        Format::S24Le => "S24_LE",
        Format::S32Le => "S32_LE",
    }
}

/// Opens the PCM node of `info`, installs `req` and prepares the stream.
fn open_configured(info: &PcmInfo, req: HwRequest) -> Result<(Pcm, HwActual), CaptureError> {
    let path = PathBuf::from(format!("/dev/snd/pcmC{}D{}c", info.card, info.device));
    let pcm = Pcm::open(&path).map_err(map_open_error)?;
    // Informational only; failure means this is not a PCM node.
    pcm.pversion().map_err(|e| {
        CaptureError::Unsupported(format!("PCM protocol version query failed: {e}"))
    })?;
    let hw = pcm.configure(req).map_err(|e| {
        CaptureError::Unsupported(format!(
            "driver rejected {}/{} ch/{} Hz/period {}/buffer {}: {e}",
            format_name(req.format),
            req.channels,
            req.rate,
            req.period_frames,
            req.buffer_frames
        ))
    })?;
    if hw.channels != req.channels || hw.rate != req.rate {
        return Err(CaptureError::Unsupported(format!(
            "driver chose {} ch / {} Hz instead of {} ch / {} Hz",
            hw.channels, hw.rate, req.channels, req.rate
        )));
    }
    pcm.prepare()?;
    Ok((pcm, hw))
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
        let cap = Self::open_prepared(cfg)?;
        cap.pcm.start()?;
        Ok(cap)
    }

    /// [`AlsaCapture::open`] without the final start: the stream is PREPARED.
    fn open_prepared(cfg: &CaptureConfig) -> Result<Self, CaptureError> {
        if cfg.channels == 0 || cfg.rate == 0 || cfg.period_frames == 0 || cfg.buffer_frames == 0 {
            return Err(CaptureError::Unsupported(
                "channels, rate, period_frames and buffer_frames must be non-zero".into(),
            ));
        }
        let info = select_pcm(cfg, &list_pcms_in(std::path::Path::new("/proc/asound"))?)?;
        let req = HwRequest {
            format: Format::S24Le,
            channels: u32::from(cfg.channels),
            rate: cfg.rate,
            period_frames: cfg.period_frames,
            buffer_frames: cfg.buffer_frames,
        };
        let (pcm, hw) = open_configured(&info, req)?;
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

/// The echo-reference PCM opened next to the mic by [`AlsaDuplex`].
///
/// The defaults are the values verified on c3-a (P2 session 1) for
/// `hw:LGSIC,3` "SoundEngineCapture": S32_LE, 2 channels, 48 kHz (the only
/// rate it offers), period 256 and buffer 8192 frames. With S32_LE, 8
/// channels were refused (`PERIOD_BYTES` is fixed at 2048).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceConfig {
    /// Substring of the PCM id or name, matched on the **mic's card**.
    pub pcm_name: String,
    /// Sample format.
    pub format: SampleFormat,
    /// Interleaved channels.
    pub channels: u16,
    /// Sample rate in Hz.
    pub rate: u32,
    /// Period size in frames.
    pub period_frames: u32,
    /// Ring buffer size in frames.
    pub buffer_frames: u32,
    /// Link the mic and the reference (`SNDRV_PCM_IOCTL_LINK`) so one start
    /// trigger starts both. If linking fails they are started back to back,
    /// reference first.
    pub link: bool,
}

impl Default for ReferenceConfig {
    fn default() -> Self {
        Self {
            pcm_name: "SoundEngineCapture".to_owned(),
            format: SampleFormat::S32Le,
            channels: 2,
            rate: 48_000,
            period_frames: 256,
            buffer_frames: 8192,
            link: true,
        }
    }
}

#[derive(Debug)]
struct RefStream {
    pcm: Pcm,
    format: SampleFormat,
    channels: u16,
    rate: u32,
    period: usize,
    card: u32,
    device: u32,
}

fn open_reference(card_id: &str, cfg: &ReferenceConfig) -> Result<RefStream, CaptureError> {
    if cfg.channels == 0 || cfg.rate == 0 || cfg.period_frames == 0 || cfg.buffer_frames == 0 {
        return Err(CaptureError::Unsupported(
            "reference channels, rate, period_frames and buffer_frames must be non-zero".into(),
        ));
    }
    let sel = CaptureConfig {
        card_id: card_id.to_owned(),
        pcm_name: Some(cfg.pcm_name.clone()),
        device: None,
        ..CaptureConfig::default()
    };
    let info = select_pcm(&sel, &list_pcms_in(std::path::Path::new("/proc/asound"))?)?;
    let req = HwRequest {
        format: match cfg.format {
            SampleFormat::S24Le => Format::S24Le,
            SampleFormat::S32Le => Format::S32Le,
        },
        channels: u32::from(cfg.channels),
        rate: cfg.rate,
        period_frames: cfg.period_frames,
        buffer_frames: cfg.buffer_frames,
    };
    let (pcm, hw) = open_configured(&info, req)?;
    Ok(RefStream {
        pcm,
        format: cfg.format,
        channels: cfg.channels,
        rate: hw.rate,
        period: usize::try_from(hw.period_frames).unwrap_or(256).max(1),
        card: info.card,
        device: info.device,
    })
}

/// Appends the frames of `raw` (interleaved, `ch` channels, raw containers)
/// to `dst` in the S24 scale, after dropping up to `discard_left` frames.
fn append_converted(
    dst: &mut Vec<i32>,
    raw: &[i32],
    ch: usize,
    format: SampleFormat,
    discard_left: &mut u64,
) {
    let frames = raw.len() / ch;
    let skip = usize::try_from(*discard_left).map_or(frames, |d| d.min(frames));
    *discard_left -= skip as u64;
    dst.extend(
        raw[skip * ch..frames * ch]
            .iter()
            .map(|&v| format.to_s24(v)),
    );
}

/// Pending reference audio is dropped (and the epoch bumped) beyond this many
/// seconds, in case the caller never calls `take_reference`.
const MAX_PENDING_S: usize = 2;

/// The mic plus an echo-reference PCM on the same card, for the echo
/// canceller. On c3-a the reference is `SoundEngineCapture`; the mic is the
/// same PCM [`AlsaCapture`] opens.
///
/// - **Start.** Both are configured and prepared, then linked
///   (`SNDRV_PCM_IOCTL_LINK`, [`ReferenceConfig::link`]) and started with one
///   `START`, as the P2 rig did on c3-a. Without a link they are started back
///   to back, reference first (so any skew keeps the reference ahead of the
///   mic, which the canceller needs), and the gap between the two `START`
///   calls is kept ([`AlsaDuplex::start_skew`]).
/// - **Reading.** [`AudioSource::read`] blocks on the mic as [`AlsaCapture`]
///   does; after each mic read it drains every reference period that is
///   ready without blocking ([`AudioSource::take_reference`]). The startup
///   discard is applied to both streams for the same duration, so they stay
///   aligned.
/// - **Overruns.** An overrun of either stream restarts both (one group
///   restart when linked) and bumps [`AudioSource::reference_epoch`].
/// - **Degradation.** If the reference cannot be opened (busy, missing,
///   parameters refused) or fails while running, the mic keeps running alone
///   and [`AudioSource::reference_state`] says why. The reference never
///   makes the mic fail.
#[derive(Debug)]
pub struct AlsaDuplex {
    mic: AlsaCapture,
    reference: Option<RefStream>,
    state: ReferenceState,
    linked: bool,
    discard_ms: u32,
    mic_discard_left: u64,
    ref_discard_left: u64,
    ref_xruns_in_row: u32,
    pending: Vec<i32>,
    scratch: Vec<i32>,
    epoch: u64,
    start_skew: Option<Duration>,
}

fn describe(e: &CaptureError) -> String {
    match e {
        CaptureError::MicBusy => "busy (another process holds the PCM)".to_owned(),
        other => other.to_string(),
    }
}

impl AlsaDuplex {
    /// Opens the mic (`mic`) and the reference (`reference`, on the mic's
    /// card), links them if asked and starts both.
    ///
    /// # Errors
    /// Only mic failures, as for [`AlsaCapture::open`]. A reference failure
    /// is reported by [`AudioSource::reference_state`].
    pub fn open(mic: &CaptureConfig, reference: &ReferenceConfig) -> Result<Self, CaptureError> {
        let mut cap = AlsaCapture::open_prepared(mic)?;
        // The duplex applies the startup discard to both streams itself.
        cap.discard_frames = 0;
        cap.discard_left = 0;
        let (ref_stream, mut state) = match open_reference(&mic.card_id, reference) {
            Ok(r) => (Some(r), ReferenceState::Off),
            Err(e) => (None, ReferenceState::Unavailable(describe(&e))),
        };
        let mut linked = false;
        let mut unlinked_reason = None;
        if let Some(r) = &ref_stream {
            if reference.link {
                match cap.pcm.link(&r.pcm) {
                    Ok(()) => linked = true,
                    Err(e) => unlinked_reason = Some(format!("SNDRV_PCM_IOCTL_LINK failed: {e}")),
                }
            } else {
                unlinked_reason = Some("link disabled in the configuration".to_owned());
            }
        }
        let mut dup = Self {
            mic: cap,
            reference: ref_stream,
            state: ReferenceState::Off,
            linked,
            discard_ms: mic.discard_ms,
            mic_discard_left: 0,
            ref_discard_left: 0,
            ref_xruns_in_row: 0,
            pending: Vec::new(),
            scratch: Vec::new(),
            epoch: 0,
            start_skew: None,
        };
        if dup.linked {
            if let Err(e) = dup.mic.pcm.start() {
                // Fall back to separate starts.
                if let Some(r) = &dup.reference {
                    let _ = r.pcm.unlink();
                }
                let _ = dup.mic.pcm.unlink();
                dup.linked = false;
                unlinked_reason = Some(format!("linked START failed: {e}"));
                dup.start_separately()?;
            }
        } else {
            dup.start_separately()?;
        }
        if dup.reference.is_some() {
            state = ReferenceState::Active {
                linked: dup.linked,
                unlinked_reason,
            };
        } else if matches!(state, ReferenceState::Off) {
            state = dup.state.clone();
        }
        dup.state = state;
        dup.arm_discard();
        Ok(dup)
    }

    /// Reference first, then the mic. A reference start failure drops the
    /// reference; a mic start failure is an error.
    fn start_separately(&mut self) -> Result<(), CaptureError> {
        let t0 = Instant::now();
        if let Some(r) = &self.reference
            && let Err(e) = r.pcm.start()
        {
            self.lose_reference(&format!("START failed: {e}"));
        }
        self.mic.pcm.start()?;
        if self.reference.is_some() {
            self.start_skew = Some(t0.elapsed());
        }
        Ok(())
    }

    fn arm_discard(&mut self) {
        self.mic_discard_left = u64::from(self.mic.hw.rate) * u64::from(self.discard_ms) / 1000;
        self.ref_discard_left = self
            .reference
            .as_ref()
            .map_or(0, |r| u64::from(r.rate) * u64::from(self.discard_ms) / 1000);
        self.pending.clear();
    }

    /// Closes the reference; the mic keeps running.
    fn lose_reference(&mut self, why: &str) {
        if let Some(r) = self.reference.take() {
            if self.linked {
                // Unlink first: stopping a linked PCM stops the whole group.
                let _ = r.pcm.unlink();
                self.linked = false;
            }
            let _ = r.pcm.drop_stream();
        }
        self.pending.clear();
        self.epoch += 1;
        self.state = ReferenceState::Unavailable(why.to_owned());
    }

    /// After the mic was recovered (prepared and started): restart the
    /// reference too when it is not in the mic's group, re-arm the discard
    /// and start a new epoch.
    fn after_mic_restart(&mut self) {
        if !self.linked
            && let Some(r) = &self.reference
        {
            let res = r
                .pcm
                .drop_stream()
                .and_then(|()| r.pcm.prepare())
                .and_then(|()| r.pcm.start());
            if let Err(e) = res {
                self.lose_reference(&format!("restart after an overrun failed: {e}"));
            }
        }
        self.epoch += 1;
        self.arm_discard();
    }

    /// Reads every reference period that is ready, without blocking.
    /// Returns `true` if both streams were restarted (overrun).
    fn drain_reference(&mut self) -> bool {
        let Some(r) = &self.reference else {
            return false;
        };
        let ch = usize::from(r.channels);
        let period = r.period;
        self.scratch.resize(period * ch, 0);
        // At most one ring's worth per call, so this cannot spin.
        for _ in 0..64 {
            let Some(r) = &self.reference else {
                return false;
            };
            match r.pcm.wait_readable(0) {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => {
                    self.lose_reference(&format!("poll failed: {e}"));
                    return false;
                }
            }
            match r.pcm.readi(&mut self.scratch, ch, period) {
                Ok(n) => {
                    self.ref_xruns_in_row = 0;
                    let fmt = r.format;
                    append_converted(
                        &mut self.pending,
                        &self.scratch[..n * ch],
                        ch,
                        fmt,
                        &mut self.ref_discard_left,
                    );
                }
                Err(e) => match e.raw_os_error() {
                    Some(libc::EINTR) => {}
                    Some(libc::EAGAIN) => break,
                    Some(libc::EPIPE | libc::ESTRPIPE) => {
                        self.ref_xruns_in_row += 1;
                        if self.ref_xruns_in_row > MAX_CONSECUTIVE_XRUNS {
                            self.lose_reference(&format!(
                                "{} consecutive reference overruns",
                                self.ref_xruns_in_row
                            ));
                            return false;
                        }
                        // Restart both: one group restart when linked.
                        if let Err(e) = self.mic.recover() {
                            // The mic read reports the broken stream next.
                            self.lose_reference(&format!("overrun recovery failed: {e}"));
                            return false;
                        }
                        self.after_mic_restart();
                        return true;
                    }
                    _ => {
                        self.lose_reference(&format!("read failed: {e}"));
                        return false;
                    }
                },
            }
        }
        let cap = self.reference.as_ref().map_or(0, |r| {
            r.rate as usize * usize::from(r.channels) * MAX_PENDING_S
        });
        if self.pending.len() > cap {
            self.pending.clear();
            self.epoch += 1;
        }
        false
    }

    /// When the streams are not linked: the time from just before the
    /// reference `START` to just after the mic `START` (an upper bound of the
    /// start skew).
    #[must_use]
    pub fn start_skew(&self) -> Option<Duration> {
        self.start_skew
    }

    /// Card index and PCM number of the reference, if open (diagnostics).
    #[must_use]
    pub fn reference_device(&self) -> Option<(u32, u32)> {
        self.reference.as_ref().map(|r| (r.card, r.device))
    }

    /// The mic stream (diagnostics: card, device, period, buffer).
    #[must_use]
    pub fn mic(&self) -> &AlsaCapture {
        &self.mic
    }
}

impl AudioSource for AlsaDuplex {
    fn read(&mut self, out: &mut [i32]) -> Result<usize, CaptureError> {
        let ch = usize::from(self.mic.channels);
        let frames = out.len() / ch;
        if frames == 0 {
            return Ok(0);
        }
        let mut consecutive = 0;
        loop {
            let want = usize::try_from(self.mic_discard_left)
                .map_or(frames, |d| if d == 0 { frames } else { d.min(frames) });
            let Some(n) = self.mic.read_once(out, want, &mut consecutive)? else {
                // The mic was recovered (and its group with it).
                self.after_mic_restart();
                continue;
            };
            if self.drain_reference() {
                continue;
            }
            if self.mic_discard_left > 0 {
                self.mic_discard_left = self.mic_discard_left.saturating_sub(n as u64);
                continue;
            }
            for s in &mut out[..n * ch] {
                *s = sign_extend_s24(*s);
            }
            return Ok(n);
        }
    }

    fn channels(&self) -> u16 {
        self.mic.channels
    }

    fn rate(&self) -> u32 {
        self.mic.hw.rate
    }

    fn xruns(&self) -> u64 {
        self.mic.xruns
    }

    fn reference_format(&self) -> Option<StreamFormat> {
        self.reference.as_ref().map(|r| StreamFormat {
            channels: r.channels,
            rate: r.rate,
        })
    }

    fn take_reference(&mut self, out: &mut Vec<i32>) {
        out.append(&mut self.pending);
    }

    fn reference_epoch(&self) -> u64 {
        self.epoch
    }

    fn reference_state(&self) -> ReferenceState {
        self.state.clone()
    }
}

impl Drop for AlsaDuplex {
    fn drop(&mut self) {
        if let Some(r) = &self.reference {
            if self.linked {
                let _ = r.pcm.unlink();
            }
            let _ = r.pcm.drop_stream();
        }
        // The mic is stopped by AlsaCapture's own Drop.
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
    fn reference_is_selected_by_name_on_the_mic_card() {
        let mut pcms = fixture();
        pcms.push(pcm(1, "LGSIC", 3, true, "SoundEngineCapture"));
        pcms.push(pcm(0, "lg115x", 3, true, "SoundEngineCapture"));
        let sel = CaptureConfig {
            pcm_name: Some(ReferenceConfig::default().pcm_name),
            ..CaptureConfig::default()
        };
        let p = select_pcm(&sel, &pcms).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((p.card, p.device), (1, 3));
        // Missing on this card: not found (the mic would then run alone).
        assert!(matches!(
            select_pcm(&sel, &fixture()),
            Err(CaptureError::DeviceNotFound(_))
        ));
    }

    #[test]
    fn reference_default_is_the_verified_c3_one() {
        let r = ReferenceConfig::default();
        assert_eq!(
            (
                r.pcm_name.as_str(),
                r.format,
                r.channels,
                r.rate,
                r.period_frames,
                r.buffer_frames,
                r.link
            ),
            (
                "SoundEngineCapture",
                SampleFormat::S32Le,
                2,
                48_000,
                256,
                8192,
                true
            )
        );
    }

    #[test]
    fn reference_frames_are_converted_after_the_discard() {
        let raw = [0x100, 0x200, 0x300, 0x400, 0x500, 0x600];
        let mut out = Vec::new();
        let mut left = 2;
        append_converted(&mut out, &raw, 2, SampleFormat::S32Le, &mut left);
        assert_eq!((out.as_slice(), left), ([5, 6].as_slice(), 0));
        let mut left = 5;
        append_converted(&mut out, &raw, 2, SampleFormat::S32Le, &mut left);
        assert_eq!((out.len(), left), (2, 2));
        append_converted(&mut out, &[0x00FF_FFFF, 1], 2, SampleFormat::S24Le, &mut 0);
        assert_eq!(out, [5, 6, -1, 1]);
    }

    #[test]
    fn busy_reference_is_described() {
        assert!(describe(&CaptureError::MicBusy).contains("busy"));
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
