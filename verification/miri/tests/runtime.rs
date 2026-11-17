//! Miri tests for the pure crates. Run via `scripts/verify/miri.sh`.
//!
//! These are runtime checks, not proofs: passing means "no UB was observed on
//! these inputs", not "the function satisfies its specification". The
//! specification work lives in `verification/SPEC.md` and is checked by
//! Kani/Verus; this layer only catches interpreter-visible memory/UB issues on
//! the exercised paths, including inside std's string and IP parsing.

use eterea_logic::{is_image_mime, is_safe_https_url};
use eterea_ui_model::action_for;

use eterea_verification_kani::keymap::KEYMAP;
use eterea_verification_kani::url_policy::{contract_violation, MUST_ACCEPT, MUST_REJECT};

#[test]
fn url_policy_corpora_run_cleanly() {
    for url in MUST_ACCEPT {
        assert!(is_safe_https_url(url), "MUST_ACCEPT rejected: {url}");
        assert!(contract_violation(url).is_none(), "contract: {url}");
    }
    for url in MUST_REJECT {
        assert!(!is_safe_https_url(url), "MUST_REJECT accepted: {url}");
    }
}

#[test]
fn url_policy_survives_unicode_and_control_bytes() {
    // Mixed-script, combining marks, emoji, RTL and control bytes. The policy
    // must return a bool without panicking or UB; its answer on these inputs is
    // checked by the corpus tests, not here.
    let inputs = [
        "https://例え.テスト/パス",
        "https://example.com/😀?q=%00",
        "https://example.com/\u{202e}evil",
        "https://exa\u{0000}mple.com/",
        "https://example.com/\u{0007}\u{0008}",
        "https://example.com/\u{200b}\u{200c}",
        "https://example.com/\u{feff}",
        "https://example.com/\u{0301}\u{0302}",
    ];
    for input in inputs {
        let _ = is_safe_https_url(input);
    }
    // Every ASCII byte in a prefix and suffix position, including the NUL byte.
    for byte in 0u8..=0x7f {
        let ch = char::from(byte);
        let url = format!("https://example.com/{ch}");
        let _ = is_safe_https_url(&url);
        let url = format!("https://{ch}example.com/");
        let _ = is_safe_https_url(&url);
    }
}

#[test]
fn url_policy_handles_bounded_pathological_lengths() {
    // Long but bounded inputs: Miri stays fast, and the parser's slicing paths
    // are exercised. Longer fuzzing belongs to cargo-fuzz when targets exist.
    let long_label = "a".repeat(63);
    let long_path = "p/".repeat(120);
    let long = format!("https://{long_label}.example.com/{long_path}");
    assert!(is_safe_https_url(&long), "long public URL rejected");
    let scheme_only = "https://";
    assert!(!is_safe_https_url(scheme_only));
    let many_slashes = format!("https://{}", "/".repeat(200));
    assert!(!is_safe_https_url(&many_slashes));
}

#[test]
fn keymap_actions_run_cleanly() {
    for (key, action) in KEYMAP {
        assert_eq!(action_for(key), Some(action.clone()), "key {key}");
    }
    for key in ["", "gg", "\u{0}", "ünïcode", "enter\n"] {
        let _ = action_for(key);
    }
}

#[test]
fn image_mime_allowlist_runs_cleanly() {
    use eterea_verification_kani::mime::{MUST_ACCEPT, MUST_REJECT};
    for mime in MUST_ACCEPT {
        assert!(is_image_mime(mime), "MUST_ACCEPT rejected: {mime}");
    }
    for mime in MUST_REJECT {
        assert!(!is_image_mime(mime), "MUST_REJECT accepted: {mime}");
    }
}
