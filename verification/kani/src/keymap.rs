//! Bounded properties for [`eterea_ui_model::action_for`] (INV-UI-01..03).
//!
//! The table mirrors the single-key rows of docs/design/SPEC.md §3. Keys that
//! are two-key sequences (`g` then `g`) or mode-scoped (`←`/`→` while media is
//! focused) belong to the sequence/focus layer above `action_for`; they are
//! listed in [`FUTURE_KEYS`] as the documented extension point.

pub use eterea_ui_model::Action;
use eterea_ui_model::action_for;

/// Single-key normal-mode mapping from docs/design/SPEC.md §3 that
/// `action_for` must implement (INV-UI-01).
pub const KEYMAP: &[(&str, Action)] = &[
    ("j", Action::MoveNext),
    ("down", Action::MoveNext),
    ("k", Action::MovePrevious),
    ("up", Action::MovePrevious),
    ("enter", Action::Open),
    ("escape", Action::Back),
    ("f", Action::Favorite),
    ("c", Action::CopyLink),
    ("o", Action::OpenOnX),
    ("m", Action::LoadMedia),
    ("/", Action::Search),
    (":", Action::Command),
    ("?", Action::Keys),
    (",", Action::Settings),
    ("space", Action::PlayPause),
    ("h", Action::History),
    ("e", Action::Export),
    ("x", Action::Select),
    ("u", Action::ToggleRead),
    ("r", Action::ToggleRead),
    ("t", Action::Tag),
    ("d", Action::Delete),
];

/// Keys that must not produce a single-key action. The lower-case letters not
/// in the table are reserved for the sequence layer; ctrl/cmd names are owned
/// by the OS/clipboard layer; text-editing and navigation key names are not
/// global actions in normal mode.
pub const UNMAPPED: &[&str] = &[
    "q", "w", "y", "i", "p", "a", "s", "l", "z", "v", "b", "n",
    "tab", "backspace", "delete", "pageup", "pagedown", "home", "end", "left", "right",
    "ctrl-p", "ctrl-f", "ctrl-,", "cmd-p", "cmd-f", "cmd-,", "",
];

/// Keys the spec defines for the sequence layer or media focus, not for
/// `action_for` (INV-UI-02, INV-UI-03):
/// * `g` `g` -> top, `G` -> end (sequence/hold layer; `Action::Top`/`End` exist),
/// * `←`/`→` only while media is focused.
/// `action_for` must not claim them as global single-key actions.
pub const FUTURE_KEYS: &[&str] = &["g", "G"];

/// Returns a reason when `key` violates the action_for contract.
///
/// The contract is intentionally weak: `action_for` is a total function from
/// key names to `Option<Action>`, it must never map a key outside [`KEYMAP`],
/// and two distinct table rows must not disagree about a key.
pub fn contract_violation(key: &str) -> Option<&'static str> {
    let expected: Option<Action> = KEYMAP
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, action)| action.clone());
    let actual = action_for(key);
    match (expected, actual) {
        (Some(want), Some(got)) if want == got => None,
        (Some(_), Some(_)) => Some("INV-UI-01: action_for disagrees with the SPEC key table"),
        (Some(_), None) => Some("INV-UI-01: SPEC key is unmapped"),
        (None, Some(_)) => Some("INV-UI-01: action_for maps a key outside the SPEC table"),
        (None, None) => None,
    }
}
