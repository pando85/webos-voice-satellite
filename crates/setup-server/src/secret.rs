//! Random values, their encodings and constant-time comparison (section 6).

use std::fs::File;
use std::io::{self, Read};

/// Source of random bytes (the kernel's in production, a counter in tests).
pub(crate) trait Rng {
    fn fill(&mut self, buf: &mut [u8]) -> io::Result<()>;
}

/// `/dev/urandom`, opened for each fill. Any failure is an error: the caller
/// fails closed.
pub(crate) struct Urandom;

impl Rng for Urandom {
    fn fill(&mut self, buf: &mut [u8]) -> io::Result<()> {
        File::open("/dev/urandom")?.read_exact(buf)
    }
}

/// base64url without padding (RFC 4648 section 5).
pub(crate) fn b64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        let chars = c.len() + 1;
        for i in 0..chars {
            out.push(char::from(A[((n >> (18 - 6 * i)) & 63) as usize]));
        }
    }
    out
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(H[usize::from(b >> 4)]));
        out.push(char::from(H[usize::from(b & 15)]));
    }
    out
}

/// Constant time for equal lengths (the lengths are public: fixed formats).
pub(crate) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in a.iter().zip(b) {
        d |= x ^ y;
    }
    std::hint::black_box(d) == 0
}

/// `n` random bytes, base64url.
pub(crate) fn random_b64(rng: &mut dyn Rng, n: usize) -> io::Result<String> {
    let mut buf = vec![0u8; n];
    rng.fill(&mut buf)?;
    Ok(b64url(&buf))
}

/// `n` random bytes, lowercase hex.
pub(crate) fn random_hex(rng: &mut dyn Rng, n: usize) -> io::Result<String> {
    let mut buf = vec![0u8; n];
    rng.fill(&mut buf)?;
    Ok(hex(&buf))
}

/// Eight lowercase hex characters (a request id).
pub(crate) fn is_id(s: &str) -> bool {
    s.len() == 8
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::too_many_lines,
        clippy::assert_is_empty,
        clippy::format_push_string,
        clippy::cast_possible_truncation
    )]

    use super::*;

    /// Deterministic bytes for tests.
    pub(crate) struct Counter(pub(crate) u8);

    impl Rng for Counter {
        fn fill(&mut self, buf: &mut [u8]) -> io::Result<()> {
            for b in buf {
                self.0 = self.0.wrapping_add(37);
                *b = self.0;
            }
            Ok(())
        }
    }

    #[test]
    fn base64url_vectors() {
        // RFC 4648 test vectors, url alphabet, no padding.
        for (i, o) in [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(b64url(i.as_bytes()), o);
        }
        assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
        assert_eq!(b64url(&[0u8; 32]).len(), 43);
    }

    #[test]
    fn hex_and_ids() {
        assert_eq!(hex(&[0, 0xab, 0x10]), "00ab10");
        assert!(is_id("a1b2c3d4"));
        for bad in ["A1B2C3D4", "a1b2c3d", "a1b2c3d45", "g1b2c3d4", "a1b2c3d\n"] {
            assert!(!is_id(bad), "{bad}");
        }
    }

    #[test]
    fn constant_time_compare() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn urandom_gives_distinct_tokens() {
        let a = random_b64(&mut Urandom, 32).expect("urandom");
        let b = random_b64(&mut Urandom, 32).expect("urandom");
        assert_eq!(a.len(), 43);
        assert_ne!(a, b);
    }
}
