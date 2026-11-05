//! The redirect and URL rules of `webos_playback::guard` on arbitrary
//! strings: no panic, and every accepted target obeys the rules.
#![no_main]

use libfuzzer_sys::fuzz_target;
use webos_playback::guard::{MAX_URL_LEN, check_url, next_url, redact_url};

fuzz_target!(|input: &[u8]| {
    let Ok(s) = std::str::from_utf8(input) else {
        return;
    };
    let (base, loc) = s.split_once('\n').unwrap_or(("https://ha.lan:8123/a/b.flac", s));
    let _ = redact_url(s);
    if check_url(base).is_err() {
        return;
    }
    if let Ok(next) = next_url(base, loc) {
        assert!(next.len() <= MAX_URL_LEN);
        assert!(check_url(&next).is_ok());
        let lower = next.to_ascii_lowercase();
        assert!(lower.starts_with("http://") || lower.starts_with("https://"));
        if base.to_ascii_lowercase().starts_with("https://") {
            assert!(lower.starts_with("https://"), "downgrade accepted");
        }
    }
});
