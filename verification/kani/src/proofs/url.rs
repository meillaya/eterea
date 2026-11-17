//! INV-MED-01..04 model-checking harnesses for the URL policy.
//!
//! `eterea_logic::is_safe_https_url` is the function under verification. Every
//! symbolic input is a bounded ASCII byte array converted to `&str` without a
//! UTF-8 validation pass (see `super::ascii_str`).

use eterea_logic::is_safe_https_url;

use crate::proofs::{ascii_str, URL_LEN};
use crate::url_policy::{contract_violation, KANI_MUST_ACCEPT, KANI_MUST_REJECT};

/// INV-MED-01: every accepted URL's scheme is HTTPS. RFC 3986 makes the scheme
/// case-insensitive, so `HTTPS://`, `hTtPs://` and `https://` all count; any
/// other scheme (or no scheme) is rejected.
///
/// Bounded to [`URL_LEN`] bytes. With that budget the accepted class is a valid
/// https scheme plus at most one host byte, so Kani exercises the scheme split
/// and early rejections without dragging std's IP parser through symbolic
/// state.
#[kani::proof]
#[kani::unwind(11)] // URL_LEN + 2 (the attribute requires a literal)
fn url_accepts_only_https() {
    let bytes: [u8; URL_LEN] = kani::any();
    for byte in &bytes {
        // Exclude non-ASCII: byte strings are not valid input to the policy in
        // the app and ASCII keeps the unchecked str conversion sound.
        kani::assume(*byte < 0x80);
    }
    let url = ascii_str(&bytes);
    if is_safe_https_url(url) {
        let (scheme, _rest) = url
            .split_once("://")
            .expect("accepted URL must contain a scheme separator");
        assert!(
            scheme.eq_ignore_ascii_case("https"),
            "INV-MED-01: accepted a URL whose scheme is not HTTPS"
        );
    }
}

/// INV-MED-03: a URL whose authority begins with `@` (empty userinfo) is
/// always rejected. The scheme is pinned and byte 8 is `@`; the rest is
/// symbolic ASCII, so the proof covers every continuation.
#[kani::proof]
#[kani::unwind(14)]
fn url_rejects_empty_userinfo() {
    const LEN: usize = 12;
    let mut bytes: [u8; LEN] = kani::any();
    let prefix = *b"https://";
    for (index, byte) in prefix.iter().enumerate() {
        // Pin the scheme: hostile schemes are covered by MUST_REJECT instead.
        bytes[index] = *byte;
    }
    bytes[8] = b'@';
    for byte in &bytes[9..] {
        // Exclude non-ASCII; userinfo rejection is byte-wise regardless.
        kani::assume(*byte < 0x80);
    }
    let url = ascii_str(&bytes);
    assert!(
        !is_safe_https_url(url),
        "INV-MED-03: accepted a URL with empty userinfo"
    );
}

/// INV-MED-04: a URL with a fragment marker is always rejected. The scheme is
/// pinned and byte 8 is `#`; the rest is symbolic ASCII.
#[kani::proof]
#[kani::unwind(14)]
fn url_rejects_fragments() {
    const LEN: usize = 12;
    let mut bytes: [u8; LEN] = kani::any();
    let prefix = *b"https://";
    for (index, byte) in prefix.iter().enumerate() {
        // Pin the scheme so only the fragment rule is under test.
        bytes[index] = *byte;
    }
    bytes[8] = b'#';
    for byte in &bytes[9..] {
        // Exclude non-ASCII; the fragment rule is byte-wise.
        kani::assume(*byte < 0x80);
    }
    let url = ascii_str(&bytes);
    assert!(
        !is_safe_https_url(url),
        "INV-MED-04: accepted a URL with a fragment"
    );
}

/// INV-MED-01..04 regression floor: the Kani subset [`KANI_MUST_REJECT`]
/// is rejected by the current policy. Concrete proof, no symbolic input.
/// The full corpora run in the plain `cargo test` suite.
#[kani::proof]
#[kani::unwind(64)]
fn url_rejects_known_bad_literals() {
    for url in KANI_MUST_REJECT {
        assert!(
            !is_safe_https_url(url),
            "INV-MED-01..04: policy accepted a MUST_REJECT literal"
        );
    }
}

/// INV-MED-01..04 regression floor: the Kani subset [`KANI_MUST_ACCEPT`] is
/// still accepted and satisfies the contract helper.
#[kani::proof]
#[kani::unwind(64)]
fn url_accepts_known_good_literals() {
    for url in KANI_MUST_ACCEPT {
        assert!(
            is_safe_https_url(url),
            "INV-MED-01..04: policy rejected a MUST_ACCEPT literal"
        );
        assert!(
            contract_violation(url).is_none(),
            "INV-MED-01..04: accepted URL violates its own contract"
        );
    }
}
