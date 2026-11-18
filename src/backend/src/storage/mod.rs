//! Storage module for bookmark persistence
//!
//! Uses SQLite with FTS5 for lightning-fast full-text search.

mod backup;
mod connection;
mod database;
mod directory_queries;
mod discovery_queries;
mod export;
mod filtered_queries;
mod history;
mod hydration;
mod metadata;
mod migrations;
mod mutations;
mod queries;
mod query;
mod read_queries;
mod schema;
mod stats;
mod tag_colors;
mod tag_management;
mod trash;
mod writes;

pub use backup::{
    BackupInfo, IntegrityReport, RestoreSummary, AUTOMATIC_BACKUPS_KEPT, BACKUP_DIR_NAME,
};
pub use database::Database;
pub use discovery_queries::TagCooccurrence;
pub use export::{
    EXPORT_CSV_HEADERS, EXPORT_CSV_MARKER, EXPORT_CSV_VERSION, EXPORT_JSON_FORMAT,
    EXPORT_JSON_VERSION,
};
pub use history::HISTORY_LIMIT;
pub use mutations::BulkResult;
pub use queries::{AuthorStats, BookmarkStats};
pub use query::{
    BookmarkQuery, BookmarkSelection, ExportFormat, ExportRequest, ExportScope, ExportSummary,
};
pub use tag_management::{TagRenameReport, TagSuggestion};
pub use trash::{TrashEntry, TRASH_LIMIT};
pub use writes::{BatchImportOutcome, FileImportOutcome};
