//! Pure interface state. No I/O, no framework types.

#![forbid(unsafe_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Feed,
    Detail,
    Authors,
    Topics,
    Graph,
    Calendar,
    Import,
    History,
    Settings,
    Onboarding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Command,
    Search,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    MoveNext,
    MovePrevious,
    Open,
    Back,
    Favorite,
    CopyLink,
    OpenOnX,
    LoadMedia,
    Search,
    Command,
    Keys,
    Settings,
    Top,
    End,
    PlayPause,
    History,
    Export,
    Select,
    ToggleRead,
    Tag,
    Delete,
}

/// What the keyboard layer decided for one key event.
pub fn action_for(key: &str) -> Option<Action> {
    match key {
        "j" | "down" => Some(Action::MoveNext),
        "k" | "up" => Some(Action::MovePrevious),
        "enter" => Some(Action::Open),
        "escape" => Some(Action::Back),
        "f" => Some(Action::Favorite),
        "c" => Some(Action::CopyLink),
        "o" => Some(Action::OpenOnX),
        "m" => Some(Action::LoadMedia),
        "/" => Some(Action::Search),
        ":" => Some(Action::Command),
        "?" => Some(Action::Keys),
        "," => Some(Action::Settings),
        "space" => Some(Action::PlayPause),
        "h" => Some(Action::History),
        "e" => Some(Action::Export),
        "x" => Some(Action::Select),
        "u" | "r" => Some(Action::ToggleRead),
        "t" => Some(Action::Tag),
        "d" => Some(Action::Delete),
        _ => None,
    }
}
