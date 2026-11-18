use eterea_core::Bookmark;
use serde::{Deserialize, Serialize};

pub use eterea_core::storage::BookmarkStats;
pub use eterea_core::storage::{
    BatchImportOutcome, BookmarkQuery, BookmarkSelection, BulkResult, ExportFormat, ExportRequest,
    ExportScope, ExportSummary, FileImportOutcome,
};
pub use eterea_core::{
    HistoryDetail, HistoryEvent, HistoryItem, HistoryKind, HistoryState, HistorySummary,
    LinkEntity, LinkKind, TagColor, TagMutation, UndoResult,
};

/// Composable filter predicate shared by the feed, saved searches and bulk
/// operations. Every populated field narrows the set; an empty spec means "all".
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilterSpec {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub from_date: Option<String>,
    #[serde(default)]
    pub to_date: Option<String>,
    #[serde(default)]
    pub favorites_only: bool,
    #[serde(default)]
    pub unread_only: bool,
    #[serde(default)]
    pub has_media: Option<bool>,
}

impl FilterSpec {
    pub fn is_empty(&self) -> bool {
        self.query
            .as_deref()
            .is_none_or(|query| query.trim().is_empty())
            && self.tags.is_empty()
            && self.author.is_none()
            && self.from_date.is_none()
            && self.to_date.is_none()
            && !self.favorites_only
            && !self.unread_only
            && self.has_media.is_none()
    }

    /// Materialize the predicate as a paged [`BookmarkQuery`].
    pub fn to_query(&self, offset: usize, limit: usize) -> BookmarkQuery {
        BookmarkQuery {
            query: self
                .query
                .as_deref()
                .map(str::trim)
                .filter(|query| !query.is_empty())
                .map(str::to_string),
            tag: None,
            tags: self.tags.clone(),
            author: self.author.clone(),
            from_date: self.from_date.clone(),
            to_date: self.to_date.clone(),
            favorites_only: self.favorites_only,
            unread_only: self.unread_only,
            has_media: self.has_media,
            offset,
            limit,
        }
    }
}

/// One persisted named filter combination.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedSearch {
    pub name: String,
    pub spec: FilterSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
}

impl<T> PaginatedResponse<T> {
    pub fn new(items: Vec<T>, total: i64, offset: usize, limit: usize) -> Self {
        let has_more = offset + items.len() < total as usize;
        Self {
            items,
            total,
            offset,
            limit,
            has_more,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuthorSummary {
    pub handle: String,
    pub name: String,
    pub profile_image: Option<String>,
    pub bookmark_count: i64,
    pub favorite_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TopicSummary {
    pub tag: String,
    pub bookmark_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImportPreviewItem {
    pub author_handle: String,
    pub content: String,
    pub tag_count: usize,
    pub has_media: bool,
}

/// Which part of an import is running, for progress reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportPhase {
    Reading,
    Saving,
}

/// Progress tick from the store thread while an import runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportProgress {
    pub phase: ImportPhase,
    pub files_done: usize,
    pub files_total: usize,
    pub rows: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImportPreview {
    pub source_label: String,
    pub format: String,
    pub bookmark_count: usize,
    /// Parsed rows whose tweet URL is already in the archive (dry-run only).
    #[serde(default)]
    pub duplicate_count: usize,
    pub sample: Vec<ImportPreviewItem>,
    pub duplicate_policy: String,
    /// Set when the file could not be parsed at all (batch previews keep going).
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImportSummary {
    pub preview: ImportPreview,
    pub imported_count: usize,
}

/// One file inside a batch import preview/summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportFileResult {
    pub source_label: String,
    pub format: String,
    pub parsed: usize,
    pub inserted: usize,
    pub duplicates: usize,
    /// Set when this file was rejected while the rest of the batch continued.
    #[serde(default)]
    pub error: Option<String>,
}

/// Dry-run preview of several import files.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ImportBatchPreview {
    pub files: Vec<ImportPreview>,
    pub total_parsed: usize,
    pub total_duplicates: usize,
}

/// Outcome of importing several files in one transaction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImportBatchSummary {
    pub preview: ImportBatchPreview,
    pub inserted: usize,
    pub duplicates: usize,
    pub files: Vec<ImportFileResult>,
    pub history_id: Option<i64>,
    /// True when the reader cancelled before anything was written.
    #[serde(default)]
    pub cancelled: bool,
}

/// Paged directory request for the authors/topics routes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirectoryPageRequest {
    pub offset: usize,
    pub limit: usize,
    /// Case-insensitive name/handle/tag filter.
    pub filter: Option<String>,
}

/// Paged history request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryListRequest {
    pub offset: usize,
    pub limit: usize,
    /// Include already-undone events (rendered as undone by the UI).
    pub include_undone: bool,
}

impl Default for HistoryListRequest {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: 20,
            include_undone: true,
        }
    }
}

/// History list page.
pub type HistoryPage = PaginatedResponse<HistoryEvent>;

/// Export-friendly bookmark selection helper: keep the old name imported from
/// the data layer so the shell has one import path.
pub use eterea_core::storage::BookmarkSelection as Selection;

pub type BookmarkPage = PaginatedResponse<Bookmark>;

/// Requested size of the tag graph.
///
/// The data layer caps the counts so a malformed request cannot ask for an
/// unbounded graph: at most [`GraphRequest::MAX_NODES`] nodes and
/// [`GraphRequest::MAX_EDGES`] edges are computed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphRequest {
    pub max_nodes: usize,
    pub max_edges: usize,
}

impl GraphRequest {
    pub const DEFAULT_MAX_NODES: usize = 40;
    pub const DEFAULT_MAX_EDGES: usize = 240;
    pub const MAX_NODES: usize = 1_200;
    pub const MAX_EDGES: usize = 6_000;

    pub fn new(max_nodes: usize, max_edges: usize) -> Self {
        Self {
            max_nodes,
            max_edges,
        }
    }

    /// Clamp the requested bounds to the data-layer caps.
    pub fn bounded(&self) -> Self {
        Self {
            max_nodes: self.max_nodes.min(Self::MAX_NODES),
            max_edges: self.max_edges.min(Self::MAX_EDGES),
        }
    }
}

impl Default for GraphRequest {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MAX_NODES, Self::DEFAULT_MAX_EDGES)
    }
}

/// A tag node placed deterministically on the unit circle.
///
/// `x` and `y` are in `[-1, 1]`; the UI maps them onto the 720px graph field
/// (centre `(0, 0)`, radius 1 = 360px). Nodes are ordered by bookmark count
/// (descending) then tag name, so the same archive always yields the same
/// layout.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphNode {
    pub tag: String,
    pub bookmark_count: i64,
    pub x: f32,
    pub y: f32,
}

/// An undirected tag co-occurrence edge between two [`GraphNode`] indexes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphEdge {
    pub source: usize,
    pub target: usize,
    pub weight: i64,
}

/// Tag graph data: top tags plus the tag pairs that share bookmarks.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GraphData {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

impl GraphData {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// Requested calendar month, `YYYY-MM` in UTC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalendarRequest {
    pub month: String,
}

impl CalendarRequest {
    pub fn new(month: impl Into<String>) -> Self {
        Self {
            month: month.into(),
        }
    }
}

/// Requested calendar year, for the year overview.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalendarYearRequest {
    pub year: i32,
}

impl CalendarYearRequest {
    pub fn new(year: i32) -> Self {
        Self { year }
    }
}

/// One day with saved posts, for the year overview heatmap.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalendarDayCount {
    pub date: String,
    pub bookmark_count: i64,
}

/// A whole year of saved-post counts (only days that have posts).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CalendarYearData {
    pub year: i32,
    pub days: Vec<CalendarDayCount>,
}

/// One day cell of the calendar grid.
///
/// `date` is an ISO `YYYY-MM-DD` UTC day. `bookmark_ids` are the bookmarks
/// posted that day, newest first; empty for days without saved posts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CalendarDay {
    pub date: String,
    pub bookmark_count: i64,
    pub bookmark_ids: Vec<String>,
}

/// Month grid data: whole Monday-first weeks covering the requested month.
///
/// `grid_start` is always a Monday and `days.len()` is always a multiple of
/// seven, so the UI can render the grid without date arithmetic. The grid is
/// at least five weeks tall and grows to six for months that need it; days from
/// adjacent months are included and keep their own marks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CalendarData {
    pub month: String,
    pub grid_start: String,
    pub grid_end: String,
    pub weeks: usize,
    pub days: Vec<CalendarDay>,
}

impl CalendarData {
    pub fn is_empty(&self) -> bool {
        self.days.iter().all(|day| day.bookmark_ids.is_empty())
    }
}
