//! Data models for eterea
//!
//! Provides a unified bookmark model that can be populated from various
//! source formats (legacy CSV, new CSV, JSON exports, X API v2 JSON).

mod bookmark;
mod history;
mod link;
mod media;
mod quote;
mod tag;

pub use bookmark::{Author, Bookmark, BookmarkBuilder};
pub use history::{
    HistoryDetail, HistoryEvent, HistoryItem, HistoryKind, HistoryState, HistorySummary, UndoResult,
};
pub use link::{normalize_links, LinkEntity, LinkKind};
pub use media::{playback_url_from_variants_json, Media, MediaType, MAX_MEDIA_PER_BOOKMARK};
pub use quote::QuotedTweet;
pub use tag::{TagColor, TagMutation};
