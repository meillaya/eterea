//! History event models shared by the data layer and the store actor.
//!
//! Every journaled action (import, delete, tag) stores a compact summary for
//! the history list and an undo payload for [`UndoResult`]. Only the newest
//! non-undone event can be undone; history keeps the latest five events.

use serde::{Deserialize, Serialize};

use super::Bookmark;

/// The action a history event records.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryKind {
    /// A batch import of one or more files.
    Import,
    /// A single or bulk delete.
    Delete,
    /// A single or bulk tag edit.
    Tag,
}

impl HistoryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Delete => "delete",
            Self::Tag => "tag",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "import" => Some(Self::Import),
            "delete" => Some(Self::Delete),
            "tag" => Some(Self::Tag),
            _ => None,
        }
    }
}

/// Compact, displayable event summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistorySummary {
    /// Bookmarks touched by the action (inserted, deleted, retagged).
    #[serde(default)]
    pub affected: usize,
    /// Items that were already present/absent and were skipped.
    #[serde(default)]
    pub skipped: usize,
    /// Human-readable sentence for the history row, e.g. `imported 1,204 posts`.
    #[serde(default)]
    pub label: String,
    /// Tags involved in a tag event, in mutation order.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Source file labels for an import event.
    #[serde(default)]
    pub files: Vec<String>,
}

/// List row for the history screen.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryEvent {
    pub id: i64,
    pub kind: HistoryKind,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub summary: HistorySummary,
    /// When the event was undone, if it was.
    #[serde(default)]
    pub undone_at: Option<chrono::DateTime<chrono::Utc>>,
    /// True only for the newest non-undone event.
    #[serde(default)]
    pub can_undo: bool,
}

/// Before/after state of one affected bookmark.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum HistoryState {
    /// Tag list before/after a tag edit.
    Tags(Vec<String>),
    /// Full bookmark snapshot for a delete (restored verbatim by undo).
    Bookmark(Box<Bookmark>),
}

/// One affected bookmark inside a history detail.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryItem {
    pub bookmark_id: String,
    pub tweet_url: String,
    pub author_handle: String,
    pub content: String,
    #[serde(default)]
    pub before: Option<HistoryState>,
    #[serde(default)]
    pub after: Option<HistoryState>,
}

/// Full detail of one history event: the row plus exactly the affected items.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryDetail {
    pub event: HistoryEvent,
    pub items: Vec<HistoryItem>,
}

/// Outcome of an undo attempt.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UndoResult {
    /// Kind of the undone event, `None` when nothing could be undone.
    #[serde(default)]
    pub kind: Option<HistoryKind>,
    /// Id of the undone event.
    #[serde(default)]
    pub event_id: Option<i64>,
    /// Bookmarks reverted/deleted/reinserted by the undo.
    #[serde(default)]
    pub affected: usize,
    /// Conflicts skipped during the undo (missing rows, duplicate tweet URLs).
    #[serde(default)]
    pub skipped: usize,
    /// Ids of the skipped items, for the status line.
    #[serde(default)]
    pub skipped_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_kind_round_trips_through_storage_strings() {
        for kind in [HistoryKind::Import, HistoryKind::Delete, HistoryKind::Tag] {
            assert_eq!(HistoryKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(HistoryKind::parse("favorite"), None);
    }
}
