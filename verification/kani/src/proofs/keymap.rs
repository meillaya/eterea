//! INV-UI-01..03 model-checking harnesses for the keymap.

use eterea_ui_model::action_for;

use crate::keymap::{Action, FUTURE_KEYS, KEYMAP, UNMAPPED};
use crate::proofs::ascii_str;

/// INV-UI-01: the SPEC §3 single-key table maps to exactly the declared
/// actions. Concrete proof over [`KEYMAP`].
#[kani::proof]
#[kani::unwind(32)]
fn action_for_table_matches_spec() {
    for (key, action) in KEYMAP {
        assert!(
            action_for(key) == Some(action.clone()),
            "INV-UI-01: action_for disagrees with the SPEC key table"
        );
    }
}

/// INV-UI-01: keys outside the SPEC single-key table produce no action.
/// Concrete proof over [`UNMAPPED`].
#[kani::proof]
#[kani::unwind(64)]
fn action_for_unmapped_keys_are_none() {
    for key in UNMAPPED {
        assert!(
            action_for(key).is_none(),
            "INV-UI-01: action_for mapped a key outside the SPEC table"
        );
    }
}

/// INV-UI-02: sequence/mode-scoped keys are not claimed by the single-key
/// layer yet (`g` `g` and `G` belong to the sequence layer).
#[kani::proof]
#[kani::unwind(8)]
fn action_for_future_keys_are_not_global() {
    for key in FUTURE_KEYS {
        assert!(
            action_for(key).is_none(),
            "INV-UI-02: sequence key claimed as a single-key action"
        );
    }
}

/// INV-UI-01: symbolic single-byte keys. Every ASCII one-byte key maps exactly
/// as the SPEC table says; this is the bounded, non-enumerated form of the
/// table proof.
#[kani::proof]
#[kani::unwind(2)]
fn action_for_single_byte_keys() {
    let byte: u8 = kani::any();
    // Exclude non-ASCII: key names are ASCII identifiers.
    kani::assume(byte < 0x80);
    let bytes = [byte];
    let key = ascii_str(&bytes);
    let expected = match byte {
        b'j' => Some(Action::MoveNext),
        b'k' => Some(Action::MovePrevious),
        b'f' => Some(Action::Favorite),
        b'c' => Some(Action::CopyLink),
        b'o' => Some(Action::OpenOnX),
        b'm' => Some(Action::LoadMedia),
        b'/' => Some(Action::Search),
        b':' => Some(Action::Command),
        b'?' => Some(Action::Keys),
        b',' => Some(Action::Settings),
        b'h' => Some(Action::History),
        b'e' => Some(Action::Export),
        b'x' => Some(Action::Select),
        b'u' | b'r' => Some(Action::ToggleRead),
        b't' => Some(Action::Tag),
        b'd' => Some(Action::Delete),
        _ => None,
    };
    assert!(
        action_for(key) == expected,
        "INV-UI-01: single-byte key mapping drifted"
    );
}
