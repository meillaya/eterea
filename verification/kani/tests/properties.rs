//! Plain-stable-Rust checks for the property helpers shared with the Kani
//! proofs. `cargo test` in this crate does not need Kani; it runs the concrete
//! corpora so the harness crate is useful in CI even when the model checker is
//! unavailable.
//!
//! The Kani harnesses in `src/proofs/` are compiled only under `cargo kani`.
//!
//! These corpora pin `src/logic` as it is today. When that crate changes and a
//! test here fails, the change is a *contract change*: update the corpora and
//! `verification/COVERAGE.md` deliberately instead of loosening the assertion.

use eterea_logic::{is_image_mime, is_safe_https_url};
use eterea_ui_model::{action_for, Action};

use eterea_verification_kani::keymap::{
    contract_violation as key_contract_violation, FUTURE_KEYS, KEYMAP, UNMAPPED,
};
use eterea_verification_kani::mime::{
    contract_violation as mime_contract_violation, MUST_ACCEPT as MIME_MUST_ACCEPT,
    MUST_REJECT as MIME_MUST_REJECT,
};
use eterea_verification_kani::url_policy::{
    contract_violation as url_contract_violation, KNOWN_GAPS, MUST_ACCEPT, MUST_REJECT,
};

// --- URL policy: baseline contract (runs today, no Kani) ---------------------

#[test]
fn must_reject_literals_are_rejected() {
    for url in MUST_REJECT {
        assert!(!is_safe_https_url(url), "INV-MED-01..04: accepted {url}");
    }
}

#[test]
fn must_accept_literals_are_accepted() {
    for url in MUST_ACCEPT {
        assert!(is_safe_https_url(url), "INV-MED-01..04: rejected {url}");
    }
}

#[test]
fn accepted_urls_satisfy_the_url_contract() {
    for url in MUST_ACCEPT {
        assert_eq!(
            url_contract_violation(url),
            None,
            "contract violation for {url}"
        );
    }
    // Bounded composition sweep: scheme case x host x port x path. No fragment,
    // no userinfo, no control characters: those are covered by MUST_REJECT.
    for scheme in ["https://", "HTTPS://", "hTtPs://"] {
        for host in [
            "pbs.twimg.com",
            "x.com",
            "example.com",
            "example.com.",
            "sub.example.com:8443",
            "[2606:4700:4700::1111]",
        ] {
            for path in ["", "/", "/a/status/1", "/media/x.jpg?format=jpg"] {
                let url = format!("{scheme}{host}{path}");
                assert!(is_safe_https_url(&url), "sweep: rejected {url}");
                assert_eq!(url_contract_violation(&url), None, "sweep: {url}");
            }
        }
    }
}

// --- URL policy: known gaps (documented, not hidden) -------------------------

/// The known-gap list is a review artifact, not dead code: it must stay empty
/// unless a failing invariant is deliberately recorded there (see
/// `verification/FINDINGS.md`). If this fails, update FINDINGS.md, COVERAGE.md
/// and the opt-in `known-gaps` Kani proof together.
#[test]
fn known_gaps_corpus_is_empty_and_reviewed() {
    assert!(
        KNOWN_GAPS.is_empty(),
        "KNOWN_GAPS must be empty or accompanied by a recorded finding: {KNOWN_GAPS:?}"
    );
}

/// If a future KNOWN_GAPS entry is added, the specification says it should be
/// rejected; this proves the corpus is still violating the spec (so the entry
/// is not stale). Vacuous today.
#[test]
#[ignore = "only meaningful when KNOWN_GAPS is non-empty; see FINDINGS.md"]
fn spec_rejects_known_gap_hosts() {
    let mut violations = Vec::new();
    for url in KNOWN_GAPS {
        if is_safe_https_url(url) {
            violations.push(*url);
        }
    }
    assert!(
        violations.is_empty(),
        "INV-MED-02: policy still accepts specified-private hosts: {violations:?}"
    );
}

// --- Image MIME allowlist ----------------------------------------------------

#[test]
fn image_mime_accepts_allowlisted_types() {
    for mime in MIME_MUST_ACCEPT {
        assert!(is_image_mime(mime), "INV-MED-09: rejected {mime}");
        assert_eq!(
            mime_contract_violation(mime),
            None,
            "INV-MED-09: contract violation for {mime}"
        );
    }
}

#[test]
fn image_mime_rejects_non_images() {
    for mime in MIME_MUST_REJECT {
        assert!(!is_image_mime(mime), "INV-MED-09: accepted {mime}");
    }
}

// --- UI model keymap ---------------------------------------------------------

#[test]
fn keymap_table_matches_action_for() {
    for (key, action) in KEYMAP {
        assert_eq!(action_for(key), Some(action.clone()), "INV-UI-01: {key}");
        assert_eq!(key_contract_violation(key), None, "INV-UI-01: {key}");
    }
}

#[test]
fn unmapped_keys_have_no_action() {
    for key in UNMAPPED {
        assert_eq!(action_for(key), None, "INV-UI-01: {key} unexpectedly mapped");
    }
}

#[test]
fn future_sequence_keys_are_not_global_actions() {
    for key in FUTURE_KEYS {
        assert_eq!(action_for(key), None, "INV-UI-02: {key} claimed early");
    }
}

#[test]
fn action_for_is_total_on_a_bounded_key_sweep() {
    // Exhaustive one-byte ASCII sweep: mirrors the symbolic Kani harness.
    for byte in 0u8..0x80 {
        let key = (byte as char).to_string();
        let actual = action_for(&key);
        let expected = match byte as char {
            'j' => Some(Action::MoveNext),
            'k' => Some(Action::MovePrevious),
            'f' => Some(Action::Favorite),
            'c' => Some(Action::CopyLink),
            'o' => Some(Action::OpenOnX),
            'm' => Some(Action::LoadMedia),
            '/' => Some(Action::Search),
            ':' => Some(Action::Command),
            '?' => Some(Action::Keys),
            ',' => Some(Action::Settings),
            'h' => Some(Action::History),
            'e' => Some(Action::Export),
            'x' => Some(Action::Select),
            'u' | 'r' => Some(Action::ToggleRead),
            't' => Some(Action::Tag),
            'd' => Some(Action::Delete),
            _ => None,
        };
        assert_eq!(actual, expected, "INV-UI-01: byte {byte:#04x}");
    }
    // Multi-byte key names from the table and a few names that must not map.
    for key in ["down", "up", "enter", "escape", "space", "Down", "J", "gg"] {
        let expected = KEYMAP
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, action)| action.clone());
        assert_eq!(action_for(key), expected, "INV-UI-01: {key}");
    }
}
