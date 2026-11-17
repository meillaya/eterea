#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let safe = eterea_logic::is_safe_https_url(text);
    if safe {
        // The policy matches the scheme case-insensitively (RFC 3986); this
        // assertion is about the scheme, not its spelling.
        assert!(
            text.to_ascii_lowercase().starts_with("https://"),
            "accepted a non-HTTPS URL"
        );
        assert!(!text.contains('#'), "accepted a URL with a fragment");
    }
    let _ = eterea_logic::is_image_mime(text);
});
