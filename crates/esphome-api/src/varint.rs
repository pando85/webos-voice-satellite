//! Protobuf-style base-128 varints, as used in the plaintext frame header.

/// Maximum encoded length of a `u32` varint.
pub const MAX_LEN: usize = 5;

/// Error while decoding a varint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overflow;

/// Appends the varint encoding of `value` to `out`.
pub fn encode(mut value: u32, out: &mut Vec<u8>) {
    loop {
        // Truncation intended: the low 7 bits of `value`.
        #[allow(clippy::cast_possible_truncation)]
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Decodes a varint from the start of `buf`.
///
/// Returns `Ok(None)` if `buf` ends before the varint does, `Ok(Some((value,
/// consumed)))` on success.
///
/// # Errors
///
/// [`Overflow`] if the encoding is longer than [`MAX_LEN`] bytes or does not
/// fit a `u32`.
pub fn decode(buf: &[u8]) -> Result<Option<(u32, usize)>, Overflow> {
    let mut value: u32 = 0;
    for (i, &byte) in buf.iter().enumerate() {
        if i >= MAX_LEN {
            return Err(Overflow);
        }
        let bits = u32::from(byte & 0x7f);
        if i == MAX_LEN - 1 && bits > 0x0f {
            return Err(Overflow);
        }
        value |= bits << (7 * i);
        if byte & 0x80 == 0 {
            return Ok(Some((value, i + 1)));
        }
    }
    if buf.len() >= MAX_LEN {
        return Err(Overflow);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(v: u32) -> Vec<u8> {
        let mut out = Vec::new();
        encode(v, &mut out);
        out
    }

    #[test]
    fn known_encodings() {
        assert_eq!(enc(0), [0x00]);
        assert_eq!(enc(1), [0x01]);
        assert_eq!(enc(127), [0x7f]);
        assert_eq!(enc(128), [0x80, 0x01]);
        assert_eq!(enc(300), [0xac, 0x02]);
        assert_eq!(enc(u32::MAX), [0xff, 0xff, 0xff, 0xff, 0x0f]);
    }

    #[test]
    fn round_trip() {
        for v in [
            0,
            1,
            127,
            128,
            255,
            16_383,
            16_384,
            65_535,
            1 << 21,
            u32::MAX,
        ] {
            let bytes = enc(v);
            assert_eq!(decode(&bytes), Ok(Some((v, bytes.len()))));
            // Trailing data is not consumed.
            let mut longer = bytes.clone();
            longer.push(0x55);
            assert_eq!(decode(&longer), Ok(Some((v, bytes.len()))));
        }
    }

    #[test]
    fn incomplete_and_overflow() {
        assert_eq!(decode(&[]), Ok(None));
        assert_eq!(decode(&[0x80]), Ok(None));
        assert_eq!(decode(&[0xff, 0xff, 0xff, 0xff]), Ok(None));
        assert_eq!(decode(&[0xff, 0xff, 0xff, 0xff, 0x10]), Err(Overflow));
        assert_eq!(decode(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x00]), Err(Overflow));
    }
}
