//! HA-driven key provisioning ("pairing"): key state, pairing window and the
//! stored key file.
//!
//! Home Assistant 2026.9+ provisions an ESPHome device that has no key by
//! itself (HA 2026.10.4 `esphome/manager.py`, `_handle_dynamic_encryption_key`
//! and `_async_provision_key_over_noise`):
//!
//! 1. HA connects **without a key** (plaintext), reads `DeviceInfo` and sees
//!    `api_encryption_supported` and `api_encryption_provisionable`.
//! 2. HA generates 32 random bytes and opens a second, short-lived Noise
//!    session with the well-known **all-zero PSK** (`ZERO_NOISE_PSK`).
//! 3. HA sends the base64 key in `NoiseEncryptionSetKeyRequest`; on success it
//!    stores the key. The device activates it and closes every keyless
//!    session, so HA reconnects with the key.
//!
//! The zero-PSK session stops passive sniffing (fresh ephemeral X25519) but
//! **authenticates nobody**: HA's own comment says it "does not authenticate
//! either side against an active man in the middle". The only protection is
//! that a device accepts keyless sessions only while it is unpaired **and**
//! its pairing window is open (ESPHome 2026.10.1 `provisioning:` models the
//! same window). This module is the pure state machine; the server does the
//! I/O.
//!
//! Rules (see `docs/design/install-and-config-ux.md` section 5):
//!
//! - A key in the configuration ([`crate::Config::noise_psk`]) is fixed:
//!   `SetKey` is refused (as ESPHome with a YAML key).
//! - Unpaired: keyless sessions (plaintext or zero-PSK Noise) get `Hello`,
//!   `DeviceInfo`, an empty entity list, ping and `SetKey` only, and only
//!   while the window is open. `SetKey` is accepted **only over zero-PSK
//!   Noise**: ESPHome still accepts it over plaintext (deprecated, to be
//!   removed in 2027.2.0); HA never uses that path when the device says
//!   `api_encryption_provisionable`.
//! - Paired through this flow: a session that proved the key may rotate it
//!   (ESPHome behaviour). An empty key ("clear") is refused: resetting the
//!   pairing is a local action only. The all-zero key is always refused.
//! - The window closes on timeout or when a key is accepted. The closed state
//!   is in memory only (a restart reopens it if configured), as in ESPHome.

use std::fmt;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use tokio::time::{Duration, Instant};

/// Minimum time between two `SetKey` attempts (any session).
pub const SET_KEY_MIN_INTERVAL: Duration = Duration::from_secs(1);

/// The well-known all-zero provisioning PSK (`aioesphomeapi.ZERO_NOISE_PSK`).
pub(crate) const ZERO_PSK: [u8; 32] = [0; 32];

/// Pairing (key provisioning) settings of the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provisioning {
    /// File that holds the provisioned key (base64, mode 0600). Written
    /// atomically when HA sets a key, deleted by a pairing reset.
    pub key_file: PathBuf,
    /// How long the pairing window stays open.
    pub window: Duration,
    /// Open the window when the server starts without a stored key.
    pub open_at_start: bool,
}

impl Default for Provisioning {
    fn default() -> Self {
        Self {
            key_file: PathBuf::from("noise_psk"),
            window: Duration::from_secs(600),
            open_at_start: true,
        }
    }
}

/// How a session reached `SetKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    /// Keyless session over zero-PSK Noise (the HA provisioning path).
    ZeroPsk,
    /// Keyless plaintext session.
    KeylessPlaintext,
    /// Session that completed Noise with the active key.
    Authenticated,
    /// Plaintext development mode (no key, no pairing).
    DevPlaintext,
}

/// Why a `SetKey` was refused. `Display` never contains key material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The key comes from the configuration and cannot change at runtime.
    FixedKey,
    /// Plaintext development mode: there is nowhere to store a key.
    DevPlaintext,
    /// Keyless plaintext transport: the key would cross the LAN in clear.
    OverPlaintext,
    /// The pairing window is closed.
    WindowClosed,
    /// Already paired and the session did not prove the key.
    AlreadyPaired,
    /// Empty key ("clear"): resetting the pairing is local only.
    EmptyKey,
    /// Not base64 of exactly 32 bytes.
    Invalid,
    /// The reserved all-zero key.
    AllZero,
    /// Another attempt less than [`SET_KEY_MIN_INTERVAL`] ago.
    RateLimited,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::FixedKey => "the key is fixed in the configuration",
            Self::DevPlaintext => "plaintext development mode has no key storage",
            Self::OverPlaintext => "a key is only accepted over a (zero-PSK) Noise session",
            Self::WindowClosed => "the pairing window is closed",
            Self::AlreadyPaired => "already paired",
            Self::EmptyKey => "an empty key (clear) is refused; reset pairing locally",
            Self::Invalid => "the key is not base64 of 32 bytes",
            Self::AllZero => "the all-zero key is reserved",
            Self::RateLimited => "too many attempts",
        })
    }
}

/// Key state of the server.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum KeyState {
    /// No key and no pairing: plaintext development mode.
    Plaintext,
    /// Key from the configuration.
    Fixed([u8; 32]),
    /// Key provisioned by HA (or found in the key file at start).
    Paired([u8; 32]),
    /// No key yet; the window is open until the given instant, or closed.
    Unpaired {
        /// End of the open window, `None` while closed.
        window_until: Option<Instant>,
    },
}

impl fmt::Debug for KeyState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plaintext => f.write_str("Plaintext"),
            Self::Fixed(_) => f.write_str("Fixed(<redacted>)"),
            Self::Paired(_) => f.write_str("Paired(<redacted>)"),
            Self::Unpaired { window_until } => f
                .debug_struct("Unpaired")
                .field("window_open", &window_until.is_some())
                .finish(),
        }
    }
}

/// What a new connection is offered.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transport {
    /// Noise with this key only.
    Noise([u8; 32]),
    /// Plaintext with full access (development mode).
    Plaintext,
    /// Keyless (plaintext or zero-PSK Noise) with restricted access; `open`
    /// is false when the window is closed (hello, then
    /// `PROVISIONING_CLOSED`).
    Keyless {
        /// The pairing window is open.
        open: bool,
    },
}

impl fmt::Debug for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Noise(_) => f.write_str("Noise(<redacted>)"),
            Self::Plaintext => f.write_str("Plaintext"),
            Self::Keyless { open } => f.debug_struct("Keyless").field("open", open).finish(),
        }
    }
}

/// Result of [`Pairing::open_window`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenResult {
    /// The window is now open (or restarted) for this long.
    Opened(Duration),
    /// A key is already active; reset the pairing first.
    AlreadyPaired,
    /// Pairing is not enabled (fixed key or development mode).
    Disabled,
}

/// Key state, pairing window and `SetKey` policy (no I/O).
#[derive(Debug)]
pub(crate) struct Pairing {
    state: KeyState,
    window: Duration,
    /// Pairing is enabled (a key file is configured).
    enabled: bool,
    /// Bumped whenever sessions opened before must not survive: a key was
    /// activated or reset, or the window closed.
    epoch: u64,
    last_attempt: Option<Instant>,
}

impl Pairing {
    pub(crate) fn new(state: KeyState, window: Duration, enabled: bool) -> Self {
        Self {
            state,
            window,
            enabled,
            epoch: 0,
            last_attempt: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> &KeyState {
        &self.state
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// No key is active and pairing is enabled.
    pub(crate) fn needs_pairing(&self) -> bool {
        matches!(self.state, KeyState::Unpaired { .. })
    }

    /// A key is active (fixed or paired): mDNS sets `api_encryption`.
    pub(crate) fn has_key(&self) -> bool {
        matches!(self.state, KeyState::Fixed(_) | KeyState::Paired(_))
    }

    /// Unpaired with an open window.
    pub(crate) fn window_open(&self) -> bool {
        matches!(
            self.state,
            KeyState::Unpaired {
                window_until: Some(_)
            }
        )
    }

    /// What a connection accepted now is offered.
    pub(crate) fn transport(&self) -> Transport {
        match self.state {
            KeyState::Plaintext => Transport::Plaintext,
            KeyState::Fixed(k) | KeyState::Paired(k) => Transport::Noise(k),
            KeyState::Unpaired { window_until } => Transport::Keyless {
                open: window_until.is_some(),
            },
        }
    }

    /// Opens (or restarts) the pairing window.
    pub(crate) fn open_window(&mut self, now: Instant) -> OpenResult {
        match self.state {
            KeyState::Plaintext | KeyState::Fixed(_) => OpenResult::Disabled,
            KeyState::Paired(_) => OpenResult::AlreadyPaired,
            KeyState::Unpaired { .. } => {
                self.state = KeyState::Unpaired {
                    window_until: Some(now + self.window),
                };
                OpenResult::Opened(self.window)
            }
        }
    }

    /// When [`Pairing::on_tick`] has work to do.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        match self.state {
            KeyState::Unpaired { window_until } => window_until,
            _ => None,
        }
    }

    /// Closes an expired window. Returns true if it closed now.
    pub(crate) fn on_tick(&mut self, now: Instant) -> bool {
        match self.state {
            KeyState::Unpaired {
                window_until: Some(until),
            } if now >= until => {
                self.state = KeyState::Unpaired { window_until: None };
                self.epoch += 1;
                true
            }
            _ => false,
        }
    }

    /// Checks a `SetKey` request. `Ok(Some(key))`: store, then
    /// [`Pairing::activate`]; `Ok(None)`: the key is already active (success,
    /// nothing to do, as ESPHome's "New PSK matches old").
    pub(crate) fn check_set_key(
        &mut self,
        key_b64: &[u8],
        origin: Origin,
        now: Instant,
    ) -> Result<Option<[u8; 32]>, Refusal> {
        match (&self.state, origin) {
            (KeyState::Fixed(_), _) => return Err(Refusal::FixedKey),
            (KeyState::Plaintext, _) | (_, Origin::DevPlaintext) => {
                return Err(Refusal::DevPlaintext);
            }
            (KeyState::Unpaired { .. }, Origin::KeylessPlaintext) => {
                return Err(Refusal::OverPlaintext);
            }
            (KeyState::Unpaired { window_until: None }, _) => return Err(Refusal::WindowClosed),
            // Keyless sessions cannot exist once paired, and an unpaired
            // server has no key to prove; refused for completeness.
            (KeyState::Paired(_), Origin::ZeroPsk | Origin::KeylessPlaintext)
            | (KeyState::Unpaired { .. }, Origin::Authenticated) => {
                return Err(Refusal::AlreadyPaired);
            }
            (KeyState::Unpaired { .. }, Origin::ZeroPsk)
            | (KeyState::Paired(_), Origin::Authenticated) => {}
        }
        if self
            .last_attempt
            .is_some_and(|t| now.saturating_duration_since(t) < SET_KEY_MIN_INTERVAL)
        {
            return Err(Refusal::RateLimited);
        }
        self.last_attempt = Some(now);
        let key = parse_key(key_b64)?;
        if matches!(self.state, KeyState::Paired(k) if k == key) {
            return Ok(None);
        }
        Ok(Some(key))
    }

    /// Activates a stored key: closes the window, invalidates older sessions.
    pub(crate) fn activate(&mut self, key: [u8; 32]) {
        self.state = KeyState::Paired(key);
        self.epoch += 1;
    }

    /// Forgets the key (the caller deleted the file) and opens the window.
    /// Returns false (and changes nothing) if pairing is not enabled.
    pub(crate) fn reset(&mut self, now: Instant) -> bool {
        if !self.enabled {
            return false;
        }
        self.state = KeyState::Unpaired {
            window_until: Some(now + self.window),
        };
        self.epoch += 1;
        true
    }
}

/// Decodes a `SetKey` payload: base64 text of exactly 32 bytes, not all zero.
fn parse_key(key_b64: &[u8]) -> Result<[u8; 32], Refusal> {
    if key_b64.is_empty() {
        return Err(Refusal::EmptyKey);
    }
    let text = std::str::from_utf8(key_b64).map_err(|_| Refusal::Invalid)?;
    let key = crate::frame::decode_psk(text).map_err(|_| Refusal::Invalid)?;
    if key == ZERO_PSK {
        return Err(Refusal::AllZero);
    }
    Ok(key)
}

fn encode_key(key: &[u8; 32]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(key)
}

/// Reads the stored key. `Ok(None)` if the file does not exist.
///
/// # Errors
///
/// An I/O error, or `InvalidData` if the file is not base64 of 32 non-zero
/// bytes. The message never contains the file's content.
pub fn load_key(path: &Path) -> io::Result<Option<[u8; 32]>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    match crate::frame::decode_psk(text.trim()) {
        Ok(k) if k != ZERO_PSK => Ok(Some(k)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not base64 of 32 non-zero bytes",
        )),
    }
}

/// Writes the key atomically: a new temporary file in the same directory
/// (mode 0600, created exclusively), `fsync`, `rename` over `path`, then
/// `fsync` of the directory. The content is the base64 key and a newline,
/// the format of `noise_psk_file`.
///
/// # Errors
///
/// Any I/O error; the temporary file is removed on failure.
pub fn store_key(path: &Path, key: &[u8; 32]) -> io::Result<()> {
    let dir = parent_dir(path);
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "key file has no name"))?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    let tmp = dir.join(tmp_name);
    let res = write_new_private(&tmp, format!("{}\n", encode_key(key)).as_bytes())
        .and_then(|()| std::fs::rename(&tmp, path))
        .and_then(|()| sync_dir(dir));
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// Deletes the stored key. Returns false if there was none.
///
/// # Errors
///
/// Any I/O error other than "not found".
pub fn remove_key(path: &Path) -> io::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            sync_dir(parent_dir(path))?;
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

fn write_new_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_B64: &str = "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA=";

    fn key() -> [u8; 32] {
        crate::frame::decode_psk(KEY_B64).unwrap()
    }

    fn unpaired(open: bool, now: Instant) -> Pairing {
        let mut p = Pairing::new(
            KeyState::Unpaired { window_until: None },
            Duration::from_secs(600),
            true,
        );
        if open {
            assert_eq!(p.open_window(now), OpenResult::Opened(p.window));
        }
        p
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("webos-pairing-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn window_opens_closes_and_is_ram_only() {
        let t0 = Instant::now();
        let mut p = unpaired(false, t0);
        assert!(p.needs_pairing() && !p.window_open());
        assert_eq!(p.transport(), Transport::Keyless { open: false });
        assert_eq!(p.deadline(), None);
        assert_eq!(
            p.open_window(t0),
            OpenResult::Opened(Duration::from_secs(600))
        );
        assert_eq!(p.transport(), Transport::Keyless { open: true });
        assert_eq!(p.deadline(), Some(t0 + Duration::from_secs(600)));
        let e = p.epoch();
        assert!(!p.on_tick(t0 + Duration::from_secs(599)));
        assert!(p.on_tick(t0 + Duration::from_secs(600)));
        assert!(
            !p.window_open() && p.epoch() == e + 1,
            "closing drops keyless sessions"
        );
        assert!(!p.on_tick(t0 + Duration::from_secs(700)));
        // Reopened only by an explicit action.
        assert!(matches!(p.open_window(t0), OpenResult::Opened(_)));
    }

    #[test]
    fn zero_psk_set_key_pairs_and_closes_the_window() {
        let t0 = Instant::now();
        let mut p = unpaired(true, t0);
        let got = p
            .check_set_key(KEY_B64.as_bytes(), Origin::ZeroPsk, t0)
            .unwrap();
        assert_eq!(got, Some(key()));
        let e = p.epoch();
        p.activate(key());
        assert_eq!(p.epoch(), e + 1);
        assert!(!p.needs_pairing() && p.has_key() && !p.window_open());
        assert_eq!(p.transport(), Transport::Noise(key()));
        assert_eq!(p.deadline(), None);
        // The window cannot be reopened while paired.
        assert_eq!(p.open_window(t0), OpenResult::AlreadyPaired);
        // Keyless sessions cannot set a key any more.
        let later = t0 + Duration::from_secs(5);
        assert_eq!(
            p.check_set_key(KEY_B64.as_bytes(), Origin::ZeroPsk, later),
            Err(Refusal::AlreadyPaired)
        );
    }

    #[test]
    fn set_key_policy() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_secs(n * 2);
        // Window closed.
        let mut p = unpaired(false, t0);
        assert_eq!(
            p.check_set_key(KEY_B64.as_bytes(), Origin::ZeroPsk, s(0)),
            Err(Refusal::WindowClosed)
        );
        // Plaintext transport refused even with the window open.
        let mut p = unpaired(true, t0);
        assert_eq!(
            p.check_set_key(KEY_B64.as_bytes(), Origin::KeylessPlaintext, s(0)),
            Err(Refusal::OverPlaintext)
        );
        // Bad keys.
        let zero = encode_key(&ZERO_PSK);
        let short = encode_key(&[1; 32])[..20].to_owned();
        for (i, (bad, why)) in [
            (&b""[..], Refusal::EmptyKey),
            (b"not base64!", Refusal::Invalid),
            (short.as_bytes(), Refusal::Invalid),
            (b"AAAA", Refusal::Invalid),
            (&[0xff, 0xfe][..], Refusal::Invalid),
            (zero.as_bytes(), Refusal::AllZero),
        ]
        .into_iter()
        .enumerate()
        {
            let now = s(1 + u64::try_from(i).unwrap());
            assert_eq!(
                p.check_set_key(bad, Origin::ZeroPsk, now),
                Err(why),
                "{why}"
            );
        }
        assert!(p.needs_pairing(), "nothing accepted");
        // Fixed key: never.
        let mut f = Pairing::new(KeyState::Fixed(key()), Duration::from_secs(1), false);
        assert_eq!(
            f.check_set_key(KEY_B64.as_bytes(), Origin::Authenticated, t0),
            Err(Refusal::FixedKey)
        );
        assert_eq!(f.open_window(t0), OpenResult::Disabled);
        assert!(!f.reset(t0));
        // Development plaintext: never.
        let mut d = Pairing::new(KeyState::Plaintext, Duration::from_secs(1), false);
        assert_eq!(
            d.check_set_key(KEY_B64.as_bytes(), Origin::DevPlaintext, t0),
            Err(Refusal::DevPlaintext)
        );
    }

    #[test]
    fn rate_limit_applies_to_every_attempt() {
        let t0 = Instant::now();
        let mut p = unpaired(true, t0);
        assert_eq!(
            p.check_set_key(b"bad", Origin::ZeroPsk, t0),
            Err(Refusal::Invalid)
        );
        let soon = t0 + Duration::from_millis(500);
        assert_eq!(
            p.check_set_key(KEY_B64.as_bytes(), Origin::ZeroPsk, soon),
            Err(Refusal::RateLimited)
        );
        let later = t0 + Duration::from_millis(1600);
        assert!(
            p.check_set_key(KEY_B64.as_bytes(), Origin::ZeroPsk, later)
                .is_ok()
        );
    }

    #[test]
    fn authenticated_session_may_rotate_but_not_clear() {
        let t0 = Instant::now();
        let mut p = Pairing::new(KeyState::Paired(key()), Duration::from_secs(600), true);
        // Same key: success, nothing to do.
        assert_eq!(
            p.check_set_key(KEY_B64.as_bytes(), Origin::Authenticated, t0),
            Ok(None)
        );
        let t1 = t0 + Duration::from_secs(2);
        assert_eq!(
            p.check_set_key(b"", Origin::Authenticated, t1),
            Err(Refusal::EmptyKey)
        );
        let new = encode_key(&[9; 32]);
        let t2 = t0 + Duration::from_secs(4);
        assert_eq!(
            p.check_set_key(new.as_bytes(), Origin::Authenticated, t2),
            Ok(Some([9; 32]))
        );
        p.activate([9; 32]);
        assert_eq!(p.transport(), Transport::Noise([9; 32]));
    }

    #[test]
    fn reset_reopens_the_window_and_invalidates_sessions() {
        let t0 = Instant::now();
        let mut p = Pairing::new(KeyState::Paired(key()), Duration::from_secs(60), true);
        let e = p.epoch();
        assert!(p.reset(t0));
        assert!(p.needs_pairing() && p.window_open() && p.epoch() == e + 1);
        assert_eq!(p.deadline(), Some(t0 + Duration::from_secs(60)));
    }

    #[test]
    fn debug_output_never_shows_the_key() {
        let p = Pairing::new(KeyState::Paired(key()), Duration::from_secs(1), true);
        let t = p.transport();
        for d in [
            format!("{p:?}"),
            format!("{t:?}"),
            format!("{:?}", p.state()),
        ] {
            assert!(!d.contains("1, 2, 3") && !d.contains("AQID"), "{d}");
            assert!(d.contains("redacted"), "{d}");
        }
        for r in [Refusal::Invalid, Refusal::AllZero, Refusal::EmptyKey] {
            assert!(!r.to_string().contains("AQID"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn key_file_is_written_atomically_with_mode_0600() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tmpdir("store");
        let path = dir.join("noise_psk");
        assert_eq!(load_key(&path).unwrap(), None);
        store_key(&path, &key()).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("{KEY_B64}\n")
        );
        assert_eq!(load_key(&path).unwrap(), Some(key()));
        // Overwrite (rotation) keeps the mode and leaves no temporary file.
        store_key(&path, &[9; 32]).unwrap();
        assert_eq!(load_key(&path).unwrap(), Some([9; 32]));
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from("noise_psk")]);
        assert!(remove_key(&path).unwrap());
        assert!(!remove_key(&path).unwrap());
        assert_eq!(load_key(&path).unwrap(), None);
        let missing = dir.join("no-such-dir").join("noise_psk");
        assert!(store_key(&missing, &key()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_key_file_is_an_error_without_its_content() {
        let dir = tmpdir("corrupt");
        let path = dir.join("noise_psk");
        std::fs::write(&path, "SECRETSECRET\n").unwrap();
        let e = load_key(&path).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert!(!e.to_string().contains("SECRET"));
        std::fs::write(&path, format!("{}\n", encode_key(&ZERO_PSK))).unwrap();
        assert!(load_key(&path).is_err(), "the all-zero key is not a key");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
