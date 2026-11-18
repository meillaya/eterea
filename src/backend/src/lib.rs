//! eterea Core - Lightning-fast Twitter bookmarks manager
//!
//! This library provides the core functionality for ingesting, processing,
//! storing, and searching Twitter bookmarks with maximum performance.

pub mod error;
pub mod ingestion;
pub mod models;
pub mod search;
pub mod storage;

pub use error::{Error, Result};
pub use ingestion::Ingester;
pub use models::{
    Bookmark, BookmarkBuilder, HistoryDetail, HistoryEvent, HistoryItem, HistoryKind, HistoryState,
    HistorySummary, LinkEntity, LinkKind, Media, MediaType, QuotedTweet, TagColor, TagMutation,
    UndoResult,
};
pub use storage::{
    BackupInfo, BookmarkQuery, BookmarkSelection, Database, ExportFormat, ExportRequest,
    ExportScope, ExportSummary, IntegrityReport, RestoreSummary, TagRenameReport, TagSuggestion,
};
