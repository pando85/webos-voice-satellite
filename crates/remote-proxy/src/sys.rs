//! Raw kernel calls: evdev and uinput ioctls, `poll`, signal blocking with
//! `signalfd`, `alarm`, `_exit` and a raw stderr write. This is the only
//! module in the crate that uses `unsafe`; everything is wrapped in safe
//! functions taking `&File` or plain values.
//!
//! Ioctl numbers use the Linux generic encoding (arm, aarch64, x86):
//! dir:2 size:14 type:8 nr:8. Struct layouts mirror
//! `include/uapi/linux/input.h` and `uinput.h`; none of them contains a
//! `long` or a pointer, so they are the same for 32- and 64-bit processes.
#![allow(unsafe_code)]

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};

use libc::c_int;

const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

#[allow(clippy::cast_possible_truncation)]
const fn ioc(dir: u32, ty: u8, nr: u32, size: usize) -> u32 {
    (dir << 30) | ((size as u32) << 16) | ((ty as u32) << 8) | nr
}

/// `struct input_absinfo`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AbsInfo {
    pub(crate) value: i32,
    pub(crate) minimum: i32,
    pub(crate) maximum: i32,
    pub(crate) fuzz: i32,
    pub(crate) flat: i32,
    pub(crate) resolution: i32,
}

/// `struct uinput_setup` (`struct input_id`, name[80], ff_effects_max).
#[repr(C)]
struct UinputSetup {
    id: [u16; 4],
    name: [u8; 80],
    ff_effects_max: u32,
}

/// `struct uinput_abs_setup` (u16 code, 2 bytes padding, `input_absinfo`).
#[repr(C)]
struct UinputAbsSetup {
    code: u16,
    _pad: u16,
    absinfo: AbsInfo,
}

const EVIOCGRAB: u32 = ioc(IOC_WRITE, b'E', 0x90, std::mem::size_of::<c_int>());
const fn eviocgkey(len: usize) -> u32 {
    ioc(IOC_READ, b'E', 0x18, len)
}
const fn eviocgbit(ev: u16, len: usize) -> u32 {
    ioc(IOC_READ, b'E', 0x20 + ev as u32, len)
}
const fn eviocgabs(axis: u16) -> u32 {
    ioc(
        IOC_READ,
        b'E',
        0x40 + axis as u32,
        std::mem::size_of::<AbsInfo>(),
    )
}
const UI_DEV_CREATE: u32 = ioc(0, b'U', 1, 0);
const UI_DEV_DESTROY: u32 = ioc(0, b'U', 2, 0);
const UI_DEV_SETUP: u32 = ioc(IOC_WRITE, b'U', 3, std::mem::size_of::<UinputSetup>());
const UI_ABS_SETUP: u32 = ioc(IOC_WRITE, b'U', 4, std::mem::size_of::<UinputAbsSetup>());
pub(crate) const UI_SET_EVBIT: u32 = ioc(IOC_WRITE, b'U', 100, std::mem::size_of::<c_int>());
pub(crate) const UI_SET_KEYBIT: u32 = ioc(IOC_WRITE, b'U', 101, std::mem::size_of::<c_int>());
pub(crate) const UI_SET_RELBIT: u32 = ioc(IOC_WRITE, b'U', 102, std::mem::size_of::<c_int>());
pub(crate) const UI_SET_ABSBIT: u32 = ioc(IOC_WRITE, b'U', 103, std::mem::size_of::<c_int>());

fn check(ret: c_int) -> io::Result<c_int> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret)
    }
}

#[allow(clippy::cast_possible_wrap, clippy::useless_conversion)]
fn req(r: u32) -> libc::Ioctl {
    r as libc::Ioctl
}

/// `EVIOCGRAB`: take (true) or release (false) exclusive delivery of a node.
pub(crate) fn grab(f: &File, on: bool) -> io::Result<()> {
    // SAFETY: EVIOCGRAB takes an int by value; the fd is valid while `f` lives.
    check(unsafe { libc::ioctl(f.as_raw_fd(), req(EVIOCGRAB), c_int::from(on)) }).map(|_| ())
}

/// `EVIOCGKEY`: the node's current key-down bitmap (bit n in byte n/8).
pub(crate) fn key_state(f: &File) -> io::Result<[u8; 256]> {
    let mut buf = [0u8; 256];
    // SAFETY: the kernel writes at most `len` (256) bytes, the size encoded in
    // the request, into `buf`; the fd is valid while `f` lives.
    check(unsafe { libc::ioctl(f.as_raw_fd(), req(eviocgkey(buf.len())), buf.as_mut_ptr()) })?;
    Ok(buf)
}

/// `EVIOCGBIT(ev)`: the codes bitmap of one event type (`ev` 0: the types
/// themselves), 256 bytes (codes up to 2047; LG's kernel goes above 767).
pub(crate) fn ev_bits(f: &File, ev: u16) -> io::Result<[u8; 256]> {
    let mut buf = [0u8; 256];
    // SAFETY: the kernel writes at most `len` (256) bytes, the size encoded in
    // the request, into `buf`; the fd is valid while `f` lives.
    check(unsafe {
        libc::ioctl(
            f.as_raw_fd(),
            req(eviocgbit(ev, buf.len())),
            buf.as_mut_ptr(),
        )
    })?;
    Ok(buf)
}

/// `EVIOCGABS(axis)`: range and current value of one absolute axis.
pub(crate) fn abs_info(f: &File, axis: u16) -> io::Result<AbsInfo> {
    let mut info = AbsInfo::default();
    // SAFETY: `info` is a repr(C) struct of the size encoded in the request;
    // the kernel writes only within it. The fd is valid while `f` lives.
    check(unsafe {
        libc::ioctl(
            f.as_raw_fd(),
            req(eviocgabs(axis)),
            std::ptr::from_mut(&mut info),
        )
    })?;
    Ok(info)
}

/// One `UI_SET_*BIT` call on a uinput fd.
pub(crate) fn ui_set_bit(f: &File, request: u32, bit: u16) -> io::Result<()> {
    // SAFETY: UI_SET_*BIT take an int by value; the fd is valid while `f` lives.
    check(unsafe { libc::ioctl(f.as_raw_fd(), req(request), c_int::from(bit)) }).map(|_| ())
}

/// `UI_ABS_SETUP` for one axis.
pub(crate) fn ui_abs_setup(f: &File, code: u16, absinfo: AbsInfo) -> io::Result<()> {
    let s = UinputAbsSetup {
        code,
        _pad: 0,
        absinfo,
    };
    // SAFETY: `s` is a repr(C) struct of the size encoded in the request and
    // is only read by the kernel. The fd is valid while `f` lives.
    check(unsafe { libc::ioctl(f.as_raw_fd(), req(UI_ABS_SETUP), std::ptr::from_ref(&s)) })
        .map(|_| ())
}

/// `UI_DEV_SETUP` (name truncated to 79 bytes) then `UI_DEV_CREATE`.
pub(crate) fn ui_create(f: &File, name: &str, id: [u16; 4]) -> io::Result<()> {
    let mut s = UinputSetup {
        id,
        name: [0; 80],
        ff_effects_max: 0,
    };
    let n = name.len().min(79);
    s.name[..n].copy_from_slice(&name.as_bytes()[..n]);
    // SAFETY: `s` is a repr(C) struct of the size encoded in the request and
    // is only read by the kernel. The fd is valid while `f` lives.
    check(unsafe { libc::ioctl(f.as_raw_fd(), req(UI_DEV_SETUP), std::ptr::from_ref(&s)) })?;
    // SAFETY: UI_DEV_CREATE takes no argument.
    check(unsafe { libc::ioctl(f.as_raw_fd(), req(UI_DEV_CREATE)) }).map(|_| ())
}

/// `UI_DEV_DESTROY` (closing the fd also destroys the device).
pub(crate) fn ui_destroy(f: &File) -> io::Result<()> {
    // SAFETY: UI_DEV_DESTROY takes no argument.
    check(unsafe { libc::ioctl(f.as_raw_fd(), req(UI_DEV_DESTROY)) }).map(|_| ())
}

/// `poll` on (fd, readable?) pairs; returns per fd: readable or hung up.
pub(crate) fn poll_readable(fds: &[RawFd], timeout_ms: i32) -> io::Result<Vec<bool>> {
    let mut p: Vec<libc::pollfd> = fds
        .iter()
        .map(|&fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    #[allow(clippy::cast_possible_truncation)]
    let n = p.len() as libc::nfds_t;
    // SAFETY: `p` is a valid array of `n` pollfd structs for the call.
    let ret = unsafe { libc::poll(p.as_mut_ptr(), n, timeout_ms) };
    if ret < 0 {
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::Interrupted {
            return Ok(vec![false; fds.len()]);
        }
        return Err(e);
    }
    Ok(p.iter()
        .map(|x| x.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0)
        .collect())
}

/// Blocks SIGHUP, SIGINT and SIGTERM in this thread (and threads started
/// later) and returns a `signalfd` that reports them, so the main loop handles
/// them like any other input and runs the normal release path. SIGALRM is left
/// alone: its default action kills the process (the cap's backstop).
pub(crate) fn signal_fd() -> io::Result<File> {
    // SAFETY: `set` is a local sigset_t initialised by sigemptyset before use;
    // pthread_sigmask and signalfd only read it. signalfd returns a new fd that
    // we own and wrap in a File exactly once.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&raw mut set);
        for s in [libc::SIGHUP, libc::SIGINT, libc::SIGTERM] {
            libc::sigaddset(&raw mut set, s);
        }
        let r = libc::pthread_sigmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut());
        if r != 0 {
            return Err(io::Error::from_raw_os_error(r));
        }
        let fd = check(libc::signalfd(-1, &raw const set, libc::SFD_CLOEXEC))?;
        Ok(File::from_raw_fd(fd))
    }
}

/// Reads one pending signal number from a `signal_fd`.
pub(crate) fn read_signal(f: &File) -> io::Result<u32> {
    use std::io::Read;
    let mut buf = [0u8; std::mem::size_of::<libc::signalfd_siginfo>()];
    let mut r: &File = f;
    let n = r.read(&mut buf)?;
    if n < 4 {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
    }
    Ok(u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]))
}

/// Ignores SIGTTIN and SIGTTOU, so a read of the terminal from a background
/// process group returns an error instead of **stopping the process** (a
/// stopped process would also stop the watchdog while the remote is grabbed).
pub(crate) fn ignore_tty_stops() {
    // SAFETY: SIG_IGN is a valid disposition; signal has no memory effects here.
    unsafe {
        libc::signal(libc::SIGTTIN, libc::SIG_IGN);
        libc::signal(libc::SIGTTOU, libc::SIG_IGN);
    }
}

/// `None` if stdin is not a terminal; otherwise whether this process is in
/// the terminal's foreground process group (so a hangup reaches it).
pub(crate) fn stdin_tty_foreground() -> Option<bool> {
    // SAFETY: isatty, tcgetpgrp and getpgrp take plain values and have no
    // memory effects.
    unsafe {
        if libc::isatty(0) != 1 {
            return None;
        }
        Some(libc::tcgetpgrp(0) == libc::getpgrp())
    }
}

/// `alarm(secs)`: SIGALRM (default action: terminate) after `secs`.
pub(crate) fn alarm(secs: u32) {
    // SAFETY: alarm has no memory effects.
    unsafe {
        libc::alarm(secs);
    }
}

/// Writes `msg` to fd 2 without taking Rust's stderr lock (used when another
/// thread may be stuck while holding it).
pub(crate) fn raw_stderr(msg: &str) {
    // SAFETY: the pointer and length describe `msg`, valid for the call.
    unsafe {
        libc::write(2, msg.as_ptr().cast(), msg.len());
    }
}

/// `_exit(code)`: ends the process at once, without unwinding or atexit
/// handlers. The kernel closes every fd, which releases an evdev grab and
/// destroys a uinput device.
pub(crate) fn exit_now(code: i32) -> ! {
    // SAFETY: _exit never returns and has no preconditions.
    unsafe { libc::_exit(code) }
}

// The kernel's struct sizes (they are encoded in the ioctl numbers).
const _: () = assert!(std::mem::size_of::<AbsInfo>() == 24);
const _: () = assert!(std::mem::size_of::<UinputSetup>() == 92);
const _: () = assert!(std::mem::size_of::<UinputAbsSetup>() == 28);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_numbers_match_the_kernel_headers() {
        assert_eq!(EVIOCGRAB, 0x4004_4590);
        assert_eq!(eviocgkey(256), 0x8100_4518);
        assert_eq!(eviocgabs(0), 0x8018_4540);
        assert_eq!(eviocgbit(0, 256), 0x8100_4520);
        assert_eq!(eviocgbit(1, 256), 0x8100_4521);
        assert_eq!(eviocgabs(1), 0x8018_4541);
        assert_eq!(UI_SET_EVBIT, 0x4004_5564);
        assert_eq!(UI_SET_KEYBIT, 0x4004_5565);
        assert_eq!(UI_SET_RELBIT, 0x4004_5566);
        assert_eq!(UI_SET_ABSBIT, 0x4004_5567);
        assert_eq!(UI_DEV_SETUP, 0x405c_5503);
        assert_eq!(UI_ABS_SETUP, 0x401c_5504);
        assert_eq!(UI_DEV_CREATE, 0x5501);
        assert_eq!(UI_DEV_DESTROY, 0x5502);
    }
}
