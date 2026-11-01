//! Audio conversion (stub).
//!
//! Phase 1.2 implements S24 to S16 conversion with fixed gain and saturation,
//! channel selection and a high-pass filter. The function below fixes the API
//! style and gives the test harness something to exercise.

/// Sign-extends a 24-bit sample held in the low bits of an `i32`.
#[must_use]
pub fn sign_extend_s24(raw: i32) -> i32 {
    (raw << 8) >> 8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_extension() {
        assert_eq!(sign_extend_s24(0x00_7F_FF_FF), 0x7F_FFFF);
        assert_eq!(sign_extend_s24(0x00_FF_FF_FF), -1);
        assert_eq!(sign_extend_s24(0x00_80_00_00), -0x80_0000);
    }
}
