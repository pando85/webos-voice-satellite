//! Raw ALSA PCM ioctl layer. This is the only module in the crate that uses
//! `unsafe`. Everything is `pub(crate)` and wrapped in safe methods on [`Pcm`].
//!
//! The struct layouts mirror `include/uapi/sound/asound.h`. They contain
//! `unsigned long` fields (`snd_pcm_uframes_t`, `snd_pcm_sframes_t`) and a
//! pointer, so their sizes differ between 32-bit ARM (the TV userland, where
//! `snd_pcm_hw_params` is 604 bytes) and 64-bit hosts (608 bytes). The ioctl
//! numbers embed those sizes, so they are computed from `size_of`.
//! A 64-bit kernel serving a 32-bit process translates them through its
//! compat layer, which expects exactly the 32-bit sizes.
#![allow(unsafe_code)]

use std::fs::File;
use std::io;
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use libc::{c_int, c_long, c_ulong, c_void};

// Linux generic ioctl encoding (x86, arm, aarch64, riscv): dir:2 size:14 type:8 nr:8.
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;
const PCM_TYPE: u32 = b'A' as u32;

#[allow(clippy::cast_possible_truncation)]
const fn ioc(dir: u32, nr: u32, size: usize) -> u32 {
    (dir << 30) | ((size as u32) << 16) | (PCM_TYPE << 8) | nr
}

/// `SNDRV_PCM_VERSION` of the kernel headers this layout was written against (2.0.15).
const PCM_VERSION: c_int = 0x0002_000f;
const PCM_VERSION_U32: u32 = 0x0002_000f;

const MASK_WORDS: usize = 8; // SNDRV_MASK_MAX (256) / 32
const N_MASKS: usize = 3; // ACCESS, FORMAT, SUBFORMAT
const N_MASK_RES: usize = 5;
const N_INTERVALS: usize = 12; // SAMPLE_BITS ..= TICK_TIME
const N_INTERVAL_RES: usize = 9;

// Interval indices, relative to SNDRV_PCM_HW_PARAM_FIRST_INTERVAL (= 8).
const IV_CHANNELS: usize = 2;
const IV_RATE: usize = 3;
const IV_PERIOD_SIZE: usize = 5;
const IV_BUFFER_SIZE: usize = 9;

const ACCESS_RW_INTERLEAVED: u32 = 3;
const FORMAT_S24_LE: u32 = 6;
const SUBFORMAT_STD: u32 = 0;

#[repr(C)]
#[derive(Clone, Copy)]
struct Mask {
    bits: [u32; MASK_WORDS],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Interval {
    min: u32,
    max: u32,
    /// Bitfield: openmin:1, openmax:1, integer:1, empty:1 (little-endian bit order).
    flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct HwParams {
    flags: u32,
    masks: [Mask; N_MASKS],
    mres: [Mask; N_MASK_RES],
    intervals: [Interval; N_INTERVALS],
    ires: [Interval; N_INTERVAL_RES],
    rmask: u32,
    cmask: u32,
    info: u32,
    msbits: u32,
    rate_num: u32,
    rate_den: u32,
    fifo_size: c_ulong,
    reserved: [u8; 64],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SwParams {
    tstamp_mode: c_int,
    period_step: u32,
    sleep_min: u32,
    avail_min: c_ulong,
    xfer_align: c_ulong,
    start_threshold: c_ulong,
    stop_threshold: c_ulong,
    silence_threshold: c_ulong,
    silence_size: c_ulong,
    boundary: c_ulong,
    proto: u32,
    tstamp_type: u32,
    reserved: [u8; 56],
}

#[repr(C)]
struct Xferi {
    result: c_long,
    buf: *mut c_void,
    frames: c_ulong,
}

#[cfg(target_pointer_width = "32")]
const _: () = {
    assert!(size_of::<HwParams>() == 604);
    assert!(size_of::<SwParams>() == 104);
    assert!(size_of::<Xferi>() == 12);
};
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(size_of::<HwParams>() == 608);
    assert!(size_of::<SwParams>() == 136);
    assert!(size_of::<Xferi>() == 24);
};

const IOCTL_PVERSION: u32 = ioc(IOC_READ, 0x00, size_of::<c_int>());
const IOCTL_USER_PVERSION: u32 = ioc(IOC_WRITE, 0x04, size_of::<c_int>());
const IOCTL_HW_PARAMS: u32 = ioc(IOC_READ | IOC_WRITE, 0x11, size_of::<HwParams>());
const IOCTL_SW_PARAMS: u32 = ioc(IOC_READ | IOC_WRITE, 0x13, size_of::<SwParams>());
const IOCTL_PREPARE: u32 = ioc(0, 0x40, 0);
const IOCTL_START: u32 = ioc(0, 0x42, 0);
const IOCTL_DROP: u32 = ioc(0, 0x43, 0);
const IOCTL_READI_FRAMES: u32 = ioc(IOC_READ, 0x51, size_of::<Xferi>());

/// The hardware parameters requested for a capture stream. The format is
/// always S24_LE, interleaved read access.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HwRequest {
    pub(crate) channels: u32,
    pub(crate) rate: u32,
    pub(crate) period_frames: u32,
    pub(crate) buffer_frames: u32,
}

/// What the driver actually configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HwActual {
    pub(crate) channels: u32,
    pub(crate) rate: u32,
    pub(crate) period_frames: u32,
    pub(crate) buffer_frames: u32,
}

fn single_bit_mask(bit: u32) -> Mask {
    let mut bits = [0; MASK_WORDS];
    bits[(bit / 32) as usize] = 1 << (bit % 32);
    Mask { bits }
}

fn exact_interval(v: u32) -> Interval {
    Interval {
        min: v,
        max: v,
        flags: 0b100, // integer
    }
}

fn build_hw_params(req: HwRequest) -> HwParams {
    let any_mask = Mask {
        bits: [u32::MAX; MASK_WORDS],
    };
    let any_interval = Interval {
        min: 0,
        max: u32::MAX,
        flags: 0,
    };
    let mut p = HwParams {
        flags: 0,
        masks: [any_mask; N_MASKS],
        mres: [Mask {
            bits: [0; MASK_WORDS],
        }; N_MASK_RES],
        intervals: [any_interval; N_INTERVALS],
        ires: [Interval {
            min: 0,
            max: 0,
            flags: 0,
        }; N_INTERVAL_RES],
        rmask: u32::MAX,
        cmask: 0,
        info: u32::MAX,
        msbits: 0,
        rate_num: 0,
        rate_den: 0,
        fifo_size: 0,
        reserved: [0; 64],
    };
    p.masks[0] = single_bit_mask(ACCESS_RW_INTERLEAVED);
    p.masks[1] = single_bit_mask(FORMAT_S24_LE);
    p.masks[2] = single_bit_mask(SUBFORMAT_STD);
    p.intervals[IV_CHANNELS] = exact_interval(req.channels);
    p.intervals[IV_RATE] = exact_interval(req.rate);
    p.intervals[IV_PERIOD_SIZE] = exact_interval(req.period_frames);
    p.intervals[IV_BUFFER_SIZE] = exact_interval(req.buffer_frames);
    p
}

fn sw_params(avail_min: u32, buffer_frames: u32, proto: u32) -> SwParams {
    SwParams {
        tstamp_mode: 0, // SNDRV_PCM_TSTAMP_NONE
        period_step: 1,
        sleep_min: 0,
        avail_min: c_ulong::from(avail_min),
        xfer_align: 1,
        // We start the stream explicitly.
        start_threshold: c_ulong::from(buffer_frames),
        // Stop (XRUN) when the ring is full.
        stop_threshold: c_ulong::from(buffer_frames),
        silence_threshold: 0,
        silence_size: 0,
        boundary: 0, // output only: the kernel fills it in
        proto,
        tstamp_type: 0,
        reserved: [0; 56],
    }
}

/// An open ALSA PCM capture device node.
#[derive(Debug)]
pub(crate) struct Pcm {
    file: File,
}

fn check(ret: c_int) -> io::Result<()> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl Pcm {
    /// Opens a PCM node read-only and non-blocking, so a busy device fails
    /// with `EBUSY`/`EAGAIN` instead of sleeping in the kernel, then switches
    /// the descriptor back to blocking reads.
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let file = File::options()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;
        let fd = file.as_raw_fd();
        // SAFETY: `fd` is a valid open descriptor owned by `file`; F_GETFL and
        // F_SETFL take no pointer arguments.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        check(flags)?;
        // SAFETY: as above.
        check(unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) })?;
        Ok(Self { file })
    }

    fn ioctl_none(&self, request: u32) -> io::Result<()> {
        // SAFETY: the request takes no argument; the fd is valid for the
        // lifetime of `self`.
        check(unsafe { libc::ioctl(self.file.as_raw_fd(), request as libc::Ioctl) })
    }

    fn ioctl_ref<T>(&self, request: u32, arg: &mut T) -> io::Result<()> {
        debug_assert_eq!(((request >> 16) & 0x3fff) as usize, size_of::<T>());
        // SAFETY: `arg` is a valid, exclusive reference to a `repr(C)` struct
        // whose size is the one encoded in `request`, so the kernel reads and
        // writes only within it. The fd is valid for the lifetime of `self`.
        check(unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                request as libc::Ioctl,
                std::ptr::from_mut::<T>(arg),
            )
        })
    }

    /// Kernel PCM protocol version (`SNDRV_PCM_IOCTL_PVERSION`).
    pub(crate) fn pversion(&self) -> io::Result<u32> {
        let mut v: c_int = 0;
        self.ioctl_ref(IOCTL_PVERSION, &mut v)?;
        #[allow(clippy::cast_sign_loss)]
        Ok(v as u32)
    }

    /// Installs the hardware parameters, then the software parameters, and
    /// returns the values the driver settled on.
    pub(crate) fn configure(&self, req: HwRequest) -> io::Result<HwActual> {
        // Tell the kernel which protocol we speak. Best effort: very old
        // kernels lack this ioctl, and the defaults work without it.
        let mut ver = PCM_VERSION;
        let _ = self.ioctl_ref(IOCTL_USER_PVERSION, &mut ver);

        let mut hw = build_hw_params(req);
        self.ioctl_ref(IOCTL_HW_PARAMS, &mut hw)?;
        let actual = HwActual {
            channels: hw.intervals[IV_CHANNELS].min,
            rate: hw.intervals[IV_RATE].min,
            period_frames: hw.intervals[IV_PERIOD_SIZE].min,
            buffer_frames: hw.intervals[IV_BUFFER_SIZE].min,
        };
        let mut sw = sw_params(actual.period_frames, actual.buffer_frames, PCM_VERSION_U32);
        self.ioctl_ref(IOCTL_SW_PARAMS, &mut sw)?;
        Ok(actual)
    }

    /// `SNDRV_PCM_IOCTL_PREPARE`: moves SETUP or XRUN to PREPARED.
    pub(crate) fn prepare(&self) -> io::Result<()> {
        self.ioctl_none(IOCTL_PREPARE)
    }

    /// `SNDRV_PCM_IOCTL_START`.
    pub(crate) fn start(&self) -> io::Result<()> {
        self.ioctl_none(IOCTL_START)
    }

    /// `SNDRV_PCM_IOCTL_DROP`: stops immediately, discarding pending frames.
    pub(crate) fn drop_stream(&self) -> io::Result<()> {
        self.ioctl_none(IOCTL_DROP)
    }

    /// Waits until the PCM is readable (or in an error state that a read will
    /// report, such as an overrun) for at most `timeout_ms` milliseconds.
    /// Returns `false` on timeout. `EINTR` counts as readable so the caller's
    /// read reports or retries it.
    pub(crate) fn wait_readable(&self, timeout_ms: u32) -> io::Result<bool> {
        let mut pfd = libc::pollfd {
            fd: self.file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout = c_int::try_from(timeout_ms).unwrap_or(c_int::MAX);
        // SAFETY: `pfd` is one valid, exclusively borrowed `pollfd` and the
        // count passed is 1. The fd is valid for the lifetime of `self`.
        let ret = unsafe { libc::poll(&raw mut pfd, 1, timeout) };
        if ret < 0 {
            let e = io::Error::last_os_error();
            return if e.raw_os_error() == Some(libc::EINTR) {
                Ok(true)
            } else {
                Err(e)
            };
        }
        // Any revents (POLLIN, POLLERR on xrun, POLLHUP) means a read returns.
        Ok(ret > 0)
    }

    /// Reads up to `frames` interleaved frames into `buf`
    /// (`SNDRV_PCM_IOCTL_READI_FRAMES`); returns the frame count read.
    ///
    /// `frame_samples` is the number of `i32` samples per frame (channels);
    /// `buf` must hold at least `frames * frame_samples` samples.
    pub(crate) fn readi(
        &self,
        buf: &mut [i32],
        frame_samples: usize,
        frames: usize,
    ) -> io::Result<usize> {
        assert!(
            frame_samples > 0
                && frames
                    .checked_mul(frame_samples)
                    .is_some_and(|n| n <= buf.len()),
            "readi buffer too small"
        );
        let mut x = Xferi {
            result: 0,
            buf: buf.as_mut_ptr().cast::<c_void>(),
            frames: frames as c_ulong,
        };
        // SAFETY: `x.buf` points to `buf`, which holds at least
        // `frames * frame_samples` i32 samples = `frames` frames of 4-byte
        // S24_LE containers per channel (checked above), and stays borrowed for
        // the duration of the call. The fd is valid for the lifetime of `self`.
        check(unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                IOCTL_READI_FRAMES as libc::Ioctl,
                std::ptr::from_mut::<Xferi>(&mut x),
            )
        })?;
        usize::try_from(x.result)
            .map_err(|_| io::Error::other("READI_FRAMES reported a negative frame count"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_numbers_match_kernel_headers() {
        // Values from <sound/asound.h> on 32-bit ARM / x86 (generic ioctl encoding).
        #[cfg(target_pointer_width = "32")]
        {
            assert_eq!(IOCTL_HW_PARAMS, 0xC25C_4111);
            assert_eq!(IOCTL_SW_PARAMS, 0xC068_4113);
            assert_eq!(IOCTL_READI_FRAMES, 0x800C_4151);
        }
        // Layout-independent ones.
        assert_eq!(IOCTL_PVERSION, 0x8004_4100);
        assert_eq!(IOCTL_USER_PVERSION, 0x4004_4104);
        assert_eq!(IOCTL_PREPARE, 0x4140);
        assert_eq!(IOCTL_START, 0x4142);
        assert_eq!(IOCTL_DROP, 0x4143);
    }

    #[test]
    fn hw_params_request_is_exact() {
        let p = build_hw_params(HwRequest {
            channels: 2,
            rate: 16_000,
            period_frames: 256,
            buffer_frames: 8192,
        });
        assert_eq!(p.masks[0].bits[0], 1 << 3);
        assert_eq!(p.masks[1].bits[0], 1 << 6);
        assert_eq!(p.masks[2].bits[0], 1);
        assert_eq!(
            (p.intervals[IV_RATE].min, p.intervals[IV_RATE].max),
            (16_000, 16_000)
        );
        assert_eq!(p.intervals[IV_BUFFER_SIZE].min, 8192);
        assert_eq!(p.rmask, u32::MAX);
    }
}
