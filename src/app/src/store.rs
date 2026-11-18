//! Background database actor for the desktop shell.
//!
//! One dedicated thread owns [`AppServices`] and answers typed [`Request`]s on
//! a crossbeam channel. The shell holds a cheap, cloneable [`StoreHandle`] and
//! never touches SQLite from the UI thread or a Rune script thread.
//!
//! Every request carries a caller-visible [`RequestId`] and every response
//! repeats it. The UI keeps the id of the newest request per screen; responses
//! whose id differs are stale and can be dropped. That keeps out-of-order or
//! superseded work from overwriting fresher state without blocking.
//!
//! There is no Rune dependency here on purpose: the actor is a plain Rust API
//! that any shell (Rune today, tests tomorrow) can wrap.

use crate::types::{
    AuthorSummary, BookmarkPage, BookmarkQuery, BookmarkSelection, BookmarkStats, BulkResult,
    CalendarData, CalendarRequest, CalendarYearData, CalendarYearRequest, DirectoryPageRequest,
    ExportRequest, ExportSummary, FilterSpec, GraphData, GraphRequest, HistoryDetail,
    HistoryListRequest, HistoryPage, ImportBatchPreview, ImportBatchSummary, ImportPreview,
    ImportProgress, PaginatedResponse, SavedSearch, TagColor, TagMutation, TopicSummary,
    UndoResult,
};
use crate::AppServices;
use crossbeam_channel::{unbounded, Receiver, Sender};
use eterea_core::{BackupInfo, Bookmark, IntegrityReport, RestoreSummary};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Caller-chosen identity for one request. Monotonic per [`StoreHandle`].
pub type RequestId = u64;

/// An error surfaced to the shell as data rather than a panic.
///
/// The actor never propagates a panic across the channel: every service error
/// becomes a [`StoreError`] in the matching response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError {
    pub message: String,
}

impl StoreError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for StoreError {}

impl From<anyhow::Error> for StoreError {
    fn from(error: anyhow::Error) -> Self {
        Self::new(format!("{error:#}"))
    }
}

/// One unit of work the actor can perform.
///
/// `Shutdown` asks the actor thread to stop after the requests queued before
/// it; it has no response.
#[derive(Debug, Clone)]
pub enum Command {
    Feed(BookmarkQuery),
    Detail(String),
    Thread(String),
    ToggleFavorite(String),
    DeleteBookmark(String),
    Stats,
    AuthorIndex,
    TopicIndex,
    Graph(GraphRequest),
    Calendar(CalendarRequest),
    CalendarYear(CalendarYearRequest),
    BackupNow,
    BackupList,
    IntegrityCheck,
    RestoreBackup(PathBuf),
    RestoreDeleted,
    TrashCount,
    ImportPreviewFile(PathBuf),
    ImportFile(PathBuf),
    ImportPreviewContent {
        filename: String,
        content: String,
    },
    ImportContent {
        filename: String,
        content: String,
    },
    // --- schema-v2 commands -------------------------------------------------
    SetTags {
        bookmark_id: String,
        tags: Vec<String>,
    },
    SetNote {
        bookmark_id: String,
        note: Option<String>,
    },
    RenameTag {
        from: String,
        to: String,
    },
    ListSavedSearches,
    SaveSearch {
        name: String,
        spec: FilterSpec,
    },
    DeleteSavedSearch(String),
    PruneTags,
    TagSuggestions {
        prefix: String,
        limit: usize,
    },
    ResetTagColor(String),
    BulkTag {
        selection: BookmarkSelection,
        mutation: TagMutation,
    },
    BulkDelete {
        selection: BookmarkSelection,
    },
    BulkSetRead {
        selection: BookmarkSelection,
        read: bool,
    },
    SetRead {
        bookmark_id: String,
        read: bool,
    },
    PreviewFiles(Vec<PathBuf>),
    /// Import the listed files. `progress` receives ticks while the actor
    /// works; `cancel` asks the parse/write pass to stop cooperatively.
    ImportFiles {
        paths: Vec<PathBuf>,
        progress: Option<Sender<ImportProgress>>,
        cancel: Arc<std::sync::atomic::AtomicBool>,
    },
    AuthorPage(DirectoryPageRequest),
    TopicPage(DirectoryPageRequest),
    HistoryList(HistoryListRequest),
    HistoryDetail(i64),
    UndoLatest,
    Export(ExportRequest),
    TagColors,
    SetTagColor {
        tag: String,
        color: String,
    },
    Shutdown,
}

/// A command plus the id the matching response will repeat.
#[derive(Debug, Clone)]
pub struct Request {
    pub id: RequestId,
    pub command: Command,
}

impl Request {
    pub fn new(id: RequestId, command: Command) -> Self {
        Self { id, command }
    }
}

/// Actor answer. Each variant repeats the originating [`RequestId`].
#[derive(Debug, PartialEq)]
pub enum Response {
    Feed {
        id: RequestId,
        result: Result<BookmarkPage, StoreError>,
    },
    Detail {
        id: RequestId,
        result: Box<Result<Option<Bookmark>, StoreError>>,
    },
    Thread {
        id: RequestId,
        result: Result<Vec<Bookmark>, StoreError>,
    },
    ToggleFavorite {
        id: RequestId,
        result: Result<bool, StoreError>,
    },
    DeleteBookmark {
        id: RequestId,
        result: Result<bool, StoreError>,
    },
    Stats {
        id: RequestId,
        result: Result<BookmarkStats, StoreError>,
    },
    AuthorIndex {
        id: RequestId,
        result: Result<Vec<AuthorSummary>, StoreError>,
    },
    TopicIndex {
        id: RequestId,
        result: Result<Vec<TopicSummary>, StoreError>,
    },
    Graph {
        id: RequestId,
        result: Result<GraphData, StoreError>,
    },
    Calendar {
        id: RequestId,
        result: Result<CalendarData, StoreError>,
    },
    CalendarYear {
        id: RequestId,
        result: Result<CalendarYearData, StoreError>,
    },
    BackupNow {
        id: RequestId,
        result: Result<BackupInfo, StoreError>,
    },
    BackupList {
        id: RequestId,
        result: Result<Vec<BackupInfo>, StoreError>,
    },
    IntegrityCheck {
        id: RequestId,
        result: Result<IntegrityReport, StoreError>,
    },
    RestoreBackup {
        id: RequestId,
        result: Result<RestoreSummary, StoreError>,
    },
    RestoreDeleted {
        id: RequestId,
        result: Result<Option<Box<Bookmark>>, StoreError>,
    },
    TrashCount {
        id: RequestId,
        result: Result<i64, StoreError>,
    },
    ImportPreview {
        id: RequestId,
        result: Result<ImportPreview, StoreError>,
    },
    Import {
        id: RequestId,
        result: Result<usize, StoreError>,
    },
    SetTags {
        id: RequestId,
        result: Result<BulkResult, StoreError>,
    },
    SetNote {
        id: RequestId,
        result: Result<BulkResult, StoreError>,
    },
    RenameTag {
        id: RequestId,
        result: Result<eterea_core::TagRenameReport, StoreError>,
    },
    SavedSearches {
        id: RequestId,
        result: Result<Vec<SavedSearch>, StoreError>,
    },
    PruneTags {
        id: RequestId,
        result: Result<usize, StoreError>,
    },
    TagSuggestions {
        id: RequestId,
        result: Result<Vec<eterea_core::TagSuggestion>, StoreError>,
    },
    ResetTagColor {
        id: RequestId,
        result: Result<bool, StoreError>,
    },
    BulkTag {
        id: RequestId,
        result: Result<BulkResult, StoreError>,
    },
    BulkDelete {
        id: RequestId,
        result: Result<BulkResult, StoreError>,
    },
    BulkSetRead {
        id: RequestId,
        result: Result<BulkResult, StoreError>,
    },
    SetRead {
        id: RequestId,
        result: Result<BulkResult, StoreError>,
    },
    PreviewFiles {
        id: RequestId,
        result: Result<ImportBatchPreview, StoreError>,
    },
    ImportFiles {
        id: RequestId,
        result: Result<ImportBatchSummary, StoreError>,
    },
    AuthorPage {
        id: RequestId,
        result: Result<PaginatedResponse<AuthorSummary>, StoreError>,
    },
    TopicPage {
        id: RequestId,
        result: Result<PaginatedResponse<TopicSummary>, StoreError>,
    },
    HistoryList {
        id: RequestId,
        result: Result<HistoryPage, StoreError>,
    },
    HistoryDetail {
        id: RequestId,
        result: Box<Result<Option<HistoryDetail>, StoreError>>,
    },
    UndoLatest {
        id: RequestId,
        result: Result<UndoResult, StoreError>,
    },
    Export {
        id: RequestId,
        result: Result<ExportSummary, StoreError>,
    },
    TagColors {
        id: RequestId,
        result: Result<Vec<TagColor>, StoreError>,
    },
    SetTagColor {
        id: RequestId,
        result: Result<TagColor, StoreError>,
    },
}

impl Response {
    /// The request this response answers. Compare it with the newest issued
    /// id to detect stale UI work.
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Feed { id, .. }
            | Self::Detail { id, .. }
            | Self::Thread { id, .. }
            | Self::ToggleFavorite { id, .. }
            | Self::DeleteBookmark { id, .. }
            | Self::Stats { id, .. }
            | Self::AuthorIndex { id, .. }
            | Self::TopicIndex { id, .. }
            | Self::Graph { id, .. }
            | Self::Calendar { id, .. }
            | Self::CalendarYear { id, .. }
            | Self::BackupNow { id, .. }
            | Self::BackupList { id, .. }
            | Self::IntegrityCheck { id, .. }
            | Self::RestoreBackup { id, .. }
            | Self::RestoreDeleted { id, .. }
            | Self::TrashCount { id, .. }
            | Self::ImportPreview { id, .. }
            | Self::Import { id, .. }
            | Self::SetTags { id, .. }
            | Self::SetNote { id, .. }
            | Self::RenameTag { id, .. }
            | Self::SavedSearches { id, .. }
            | Self::PruneTags { id, .. }
            | Self::TagSuggestions { id, .. }
            | Self::ResetTagColor { id, .. }
            | Self::BulkTag { id, .. }
            | Self::BulkDelete { id, .. }
            | Self::BulkSetRead { id, .. }
            | Self::SetRead { id, .. }
            | Self::PreviewFiles { id, .. }
            | Self::ImportFiles { id, .. }
            | Self::AuthorPage { id, .. }
            | Self::TopicPage { id, .. }
            | Self::HistoryList { id, .. }
            | Self::HistoryDetail { id, .. }
            | Self::UndoLatest { id, .. }
            | Self::Export { id, .. }
            | Self::TagColors { id, .. }
            | Self::SetTagColor { id, .. } => *id,
        }
    }
}

/// Cheap handle to the background store. Clone it into whatever owns UI state.
///
/// `send`/`try_issue` never block: requests go onto an unbounded queue and the
/// caller polls `try_recv` (or waits with `recv_timeout`). Opening a
/// [`StoreHandle`] opens the database once on the actor thread.
pub struct StoreHandle {
    requests: Sender<Request>,
    responses: Receiver<Response>,
    next_request_id: Arc<AtomicU64>,
    import_cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl Clone for StoreHandle {
    fn clone(&self) -> Self {
        Self {
            requests: self.requests.clone(),
            responses: self.responses.clone(),
            next_request_id: Arc::clone(&self.next_request_id),
            import_cancel: Arc::clone(&self.import_cancel),
        }
    }
}

impl fmt::Debug for StoreHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoreHandle")
            .field("next_request_id", &self.next_request_id())
            .finish_non_exhaustive()
    }
}

impl StoreHandle {
    /// Open the default on-disk archive and spawn the actor thread.
    pub fn open_default() -> anyhow::Result<Self> {
        Self::from_services(AppServices::open_default()?)
    }

    /// Open an archive at `path` and spawn the actor thread.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        Self::from_services(AppServices::open(path)?)
    }

    /// Open an in-memory archive for tests and previews.
    pub fn open_memory() -> anyhow::Result<Self> {
        Self::from_services(AppServices::open_memory()?)
    }

    /// Spawn the actor thread around an already-open service layer.
    pub fn from_services(services: AppServices) -> anyhow::Result<Self> {
        let (request_sender, request_receiver) = unbounded();
        let (response_sender, response_receiver) = unbounded();
        thread::Builder::new()
            .name("eterea-store".to_string())
            .spawn(move || run_actor(services, request_receiver, response_sender))
            .map_err(|error| anyhow::anyhow!("failed to spawn eterea store actor: {error}"))?;

        Ok(Self {
            requests: request_sender,
            responses: response_receiver,
            next_request_id: Arc::new(AtomicU64::new(1)),
            import_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    /// The id the next issued request will carry.
    pub fn next_request_id(&self) -> RequestId {
        self.next_request_id.load(Ordering::Relaxed)
    }

    /// Queue one raw command and return its request id.
    pub fn issue(&self, command: Command) -> RequestId {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        // A dead actor simply never answers; the caller's id comparison treats
        // the request as stale instead of surfacing a channel panic.
        let _ = self.requests.send(Request { id, command });
        id
    }

    /// Queue one raw request, reporting a closed channel.
    ///
    /// The caller owns the [`RequestId`] here; convenience methods and
    /// [`StoreHandle::issue`] are preferred because they allocate ids.
    pub fn try_send(&self, request: Request) -> Result<(), StoreError> {
        self.requests
            .send(request)
            .map_err(|_| StoreError::new("store actor is not running"))
    }

    pub fn feed(&self, query: BookmarkQuery) -> RequestId {
        self.issue(Command::Feed(query))
    }

    pub fn thread(&self, bookmark_id: impl Into<String>) -> RequestId {
        self.issue(Command::Thread(bookmark_id.into()))
    }

    pub fn detail(&self, bookmark_id: impl Into<String>) -> RequestId {
        self.issue(Command::Detail(bookmark_id.into()))
    }

    pub fn toggle_favorite(&self, bookmark_id: impl Into<String>) -> RequestId {
        self.issue(Command::ToggleFavorite(bookmark_id.into()))
    }

    pub fn delete_bookmark(&self, bookmark_id: impl Into<String>) -> RequestId {
        self.issue(Command::DeleteBookmark(bookmark_id.into()))
    }

    pub fn stats(&self) -> RequestId {
        self.issue(Command::Stats)
    }

    pub fn author_index(&self) -> RequestId {
        self.issue(Command::AuthorIndex)
    }

    pub fn topic_index(&self) -> RequestId {
        self.issue(Command::TopicIndex)
    }

    pub fn graph(&self, request: GraphRequest) -> RequestId {
        self.issue(Command::Graph(request))
    }

    pub fn calendar(&self, request: CalendarRequest) -> RequestId {
        self.issue(Command::Calendar(request))
    }

    pub fn calendar_year(&self, request: CalendarYearRequest) -> RequestId {
        self.issue(Command::CalendarYear(request))
    }

    /// Write a manual, consistent backup beside the archive.
    pub fn backup_now(&self) -> RequestId {
        self.issue(Command::BackupNow)
    }

    /// List the backup copies already on disk, newest first.
    pub fn backup_list(&self) -> RequestId {
        self.issue(Command::BackupList)
    }

    /// Run SQLite's integrity and foreign-key checks.
    pub fn integrity_check(&self) -> RequestId {
        self.issue(Command::IntegrityCheck)
    }

    /// Replace the live archive with the backup at `path`.
    pub fn restore_backup(&self, path: impl Into<PathBuf>) -> RequestId {
        self.issue(Command::RestoreBackup(path.into()))
    }

    /// Restore the newest recoverable deletion.
    pub fn restore_deleted(&self) -> RequestId {
        self.issue(Command::RestoreDeleted)
    }

    /// Count recoverable deletions.
    pub fn trash_count(&self) -> RequestId {
        self.issue(Command::TrashCount)
    }

    pub fn import_preview_file(&self, path: impl Into<PathBuf>) -> RequestId {
        self.issue(Command::ImportPreviewFile(path.into()))
    }

    pub fn import_file(&self, path: impl Into<PathBuf>) -> RequestId {
        self.issue(Command::ImportFile(path.into()))
    }

    pub fn import_preview_content(
        &self,
        filename: impl Into<String>,
        content: impl Into<String>,
    ) -> RequestId {
        self.issue(Command::ImportPreviewContent {
            filename: filename.into(),
            content: content.into(),
        })
    }

    pub fn import_content(
        &self,
        filename: impl Into<String>,
        content: impl Into<String>,
    ) -> RequestId {
        self.issue(Command::ImportContent {
            filename: filename.into(),
            content: content.into(),
        })
    }

    // --- schema-v2 convenience methods ------------------------------------

    pub fn saved_searches(&self) -> RequestId {
        self.issue(Command::ListSavedSearches)
    }

    pub fn save_search(&self, name: impl Into<String>, spec: FilterSpec) -> RequestId {
        self.issue(Command::SaveSearch {
            name: name.into(),
            spec,
        })
    }

    pub fn delete_saved_search(&self, name: impl Into<String>) -> RequestId {
        self.issue(Command::DeleteSavedSearch(name.into()))
    }

    pub fn rename_tag(&self, from: impl Into<String>, to: impl Into<String>) -> RequestId {
        self.issue(Command::RenameTag {
            from: from.into(),
            to: to.into(),
        })
    }

    pub fn prune_tags(&self) -> RequestId {
        self.issue(Command::PruneTags)
    }

    pub fn tag_suggestions(&self, prefix: impl Into<String>, limit: usize) -> RequestId {
        self.issue(Command::TagSuggestions {
            prefix: prefix.into(),
            limit,
        })
    }

    pub fn reset_tag_color(&self, tag: impl Into<String>) -> RequestId {
        self.issue(Command::ResetTagColor(tag.into()))
    }

    pub fn set_note(&self, bookmark_id: impl Into<String>, note: Option<String>) -> RequestId {
        self.issue(Command::SetNote {
            bookmark_id: bookmark_id.into(),
            note,
        })
    }

    pub fn set_tags(&self, bookmark_id: impl Into<String>, tags: Vec<String>) -> RequestId {
        self.issue(Command::SetTags {
            bookmark_id: bookmark_id.into(),
            tags,
        })
    }

    pub fn bulk_tag(&self, selection: BookmarkSelection, mutation: TagMutation) -> RequestId {
        self.issue(Command::BulkTag {
            selection,
            mutation,
        })
    }

    pub fn bulk_delete(&self, selection: BookmarkSelection) -> RequestId {
        self.issue(Command::BulkDelete { selection })
    }

    pub fn bulk_set_read(&self, selection: BookmarkSelection, read: bool) -> RequestId {
        self.issue(Command::BulkSetRead { selection, read })
    }

    pub fn set_read(&self, bookmark_id: impl Into<String>, read: bool) -> RequestId {
        self.issue(Command::SetRead {
            bookmark_id: bookmark_id.into(),
            read,
        })
    }

    pub fn preview_files(&self, paths: Vec<PathBuf>) -> RequestId {
        self.issue(Command::PreviewFiles(paths))
    }

    pub fn import_files(&self, paths: Vec<PathBuf>) -> RequestId {
        self.import_files_with_progress(paths).0
    }

    /// Import files while receiving progress ticks from the actor thread.
    pub fn import_files_with_progress(
        &self,
        paths: Vec<PathBuf>,
    ) -> (RequestId, Receiver<ImportProgress>) {
        let (sender, receiver) = unbounded();
        self.import_cancel
            .store(false, std::sync::atomic::Ordering::Relaxed);
        let id = self.issue(Command::ImportFiles {
            paths,
            progress: Some(sender),
            cancel: Arc::clone(&self.import_cancel),
        });
        (id, receiver)
    }

    /// Ask a running import to stop before the write. Safe when none runs.
    pub fn cancel_import(&self) {
        self.import_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn author_page(&self, request: DirectoryPageRequest) -> RequestId {
        self.issue(Command::AuthorPage(request))
    }

    pub fn topic_page(&self, request: DirectoryPageRequest) -> RequestId {
        self.issue(Command::TopicPage(request))
    }

    pub fn history_list(&self, request: HistoryListRequest) -> RequestId {
        self.issue(Command::HistoryList(request))
    }

    pub fn history_detail(&self, event_id: i64) -> RequestId {
        self.issue(Command::HistoryDetail(event_id))
    }

    pub fn undo_latest(&self) -> RequestId {
        self.issue(Command::UndoLatest)
    }

    pub fn export(&self, request: ExportRequest) -> RequestId {
        self.issue(Command::Export(request))
    }

    pub fn tag_colors(&self) -> RequestId {
        self.issue(Command::TagColors)
    }

    pub fn set_tag_color(&self, tag: impl Into<String>, color: impl Into<String>) -> RequestId {
        self.issue(Command::SetTagColor {
            tag: tag.into(),
            color: color.into(),
        })
    }

    /// Ask the actor to stop. Earlier queued requests still run first.
    pub fn shutdown(&self) {
        self.issue(Command::Shutdown);
    }

    /// Non-blocking poll for one finished response.
    pub fn try_recv(&self) -> Option<Response> {
        self.responses.try_recv().ok()
    }

    /// Wait up to `timeout` for one finished response.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Response> {
        self.responses.recv_timeout(timeout).ok()
    }

    /// Drain every finished response without blocking.
    pub fn drain(&self) -> Vec<Response> {
        let mut responses = Vec::new();
        while let Some(response) = self.try_recv() {
            responses.push(response);
        }
        responses
    }
}

fn run_actor(mut services: AppServices, requests: Receiver<Request>, responses: Sender<Response>) {
    while let Ok(request) = requests.recv() {
        if matches!(request.command, Command::Shutdown) {
            break;
        }
        let response = dispatch(&mut services, request);
        if responses.send(response).is_err() {
            break;
        }
    }
}

fn dispatch(services: &mut AppServices, request: Request) -> Response {
    let Request { id, command } = request;
    match command {
        Command::Feed(query) => Response::Feed {
            id,
            result: services.query_bookmarks(&query).map_err(StoreError::from),
        },
        Command::Thread(bookmark_id) => Response::Thread {
            id,
            result: services
                .thread_posts(&bookmark_id)
                .map_err(StoreError::from),
        },
        Command::Detail(bookmark_id) => Response::Detail {
            id,
            result: Box::new(
                services
                    .bookmark_detail(&bookmark_id)
                    .map_err(StoreError::from),
            ),
        },
        Command::ToggleFavorite(bookmark_id) => Response::ToggleFavorite {
            id,
            result: services
                .toggle_favorite(&bookmark_id)
                .map_err(StoreError::from),
        },
        Command::DeleteBookmark(bookmark_id) => Response::DeleteBookmark {
            id,
            result: services
                .delete_bookmark(&bookmark_id)
                .map_err(StoreError::from),
        },
        Command::Stats => Response::Stats {
            id,
            result: services.stats().map_err(StoreError::from),
        },
        Command::AuthorIndex => Response::AuthorIndex {
            id,
            result: services.author_index().map_err(StoreError::from),
        },
        Command::TopicIndex => Response::TopicIndex {
            id,
            result: services.topic_index().map_err(StoreError::from),
        },
        Command::Graph(graph_request) => Response::Graph {
            id,
            result: services
                .graph_data(&graph_request)
                .map_err(StoreError::from),
        },
        Command::Calendar(calendar_request) => Response::Calendar {
            id,
            result: services
                .calendar_data(&calendar_request)
                .map_err(StoreError::from),
        },
        Command::CalendarYear(year_request) => Response::CalendarYear {
            id,
            result: services
                .calendar_year_data(&year_request)
                .map_err(StoreError::from),
        },
        Command::BackupNow => Response::BackupNow {
            id,
            result: services.backup_now().map_err(StoreError::from),
        },
        Command::BackupList => Response::BackupList {
            id,
            result: services.backup_list().map_err(StoreError::from),
        },
        Command::IntegrityCheck => Response::IntegrityCheck {
            id,
            result: services.integrity_check().map_err(StoreError::from),
        },
        Command::RestoreBackup(path) => Response::RestoreBackup {
            id,
            result: services.restore_backup(&path).map_err(StoreError::from),
        },
        Command::RestoreDeleted => Response::RestoreDeleted {
            id,
            result: services
                .restore_deleted()
                .map(|bookmark| bookmark.map(Box::new))
                .map_err(StoreError::from),
        },
        Command::TrashCount => Response::TrashCount {
            id,
            result: services.trash_count().map_err(StoreError::from),
        },
        Command::ImportPreviewFile(path) => Response::ImportPreview {
            id,
            result: services
                .preview_import_file(&path)
                .map_err(StoreError::from),
        },
        Command::ImportFile(path) => Response::Import {
            id,
            result: services.import_file(&path).map_err(StoreError::from),
        },
        Command::ImportPreviewContent { filename, content } => Response::ImportPreview {
            id,
            result: services
                .preview_import_content(&filename, &content)
                .map_err(StoreError::from),
        },
        Command::ImportContent { filename, content } => Response::Import {
            id,
            result: services
                .import_content(&filename, &content)
                .map_err(StoreError::from),
        },
        Command::SetTags { bookmark_id, tags } => Response::SetTags {
            id,
            result: services
                .set_bookmark_tags(&bookmark_id, &tags)
                .map_err(StoreError::from),
        },
        Command::ListSavedSearches => Response::SavedSearches {
            id,
            result: services.saved_searches().map_err(StoreError::from),
        },
        Command::SaveSearch { name, spec } => Response::SavedSearches {
            id,
            result: services.save_search(&name, spec).map_err(StoreError::from),
        },
        Command::DeleteSavedSearch(name) => Response::SavedSearches {
            id,
            result: services
                .delete_saved_search(&name)
                .map_err(StoreError::from),
        },
        Command::RenameTag { from, to } => Response::RenameTag {
            id,
            result: services.rename_tag(&from, &to).map_err(StoreError::from),
        },
        Command::PruneTags => Response::PruneTags {
            id,
            result: services.prune_tags().map_err(StoreError::from),
        },
        Command::TagSuggestions { prefix, limit } => Response::TagSuggestions {
            id,
            result: services
                .tag_suggestions(&prefix, limit)
                .map_err(StoreError::from),
        },
        Command::ResetTagColor(tag) => Response::ResetTagColor {
            id,
            result: services.reset_tag_color(&tag).map_err(StoreError::from),
        },
        Command::SetNote { bookmark_id, note } => {
            let set = services
                .set_bookmark_note(&bookmark_id, note.as_deref())
                .map_err(StoreError::from);
            Response::SetNote {
                id,
                result: set.map(|changed| BulkResult {
                    affected: usize::from(changed),
                    ..BulkResult::default()
                }),
            }
        }
        Command::BulkTag {
            selection,
            mutation,
        } => Response::BulkTag {
            id,
            result: services
                .apply_tag_mutation(&selection, &mutation)
                .map_err(StoreError::from),
        },
        Command::BulkDelete { selection } => Response::BulkDelete {
            id,
            result: services.bulk_delete(&selection).map_err(StoreError::from),
        },
        Command::BulkSetRead { selection, read } => Response::BulkSetRead {
            id,
            result: services
                .bulk_set_read(&selection, read)
                .map_err(StoreError::from),
        },
        Command::SetRead { bookmark_id, read } => Response::SetRead {
            id,
            result: services
                .set_read(&bookmark_id, read)
                .map_err(StoreError::from),
        },
        Command::PreviewFiles(paths) => Response::PreviewFiles {
            id,
            result: services
                .preview_import_files(&paths)
                .map_err(StoreError::from),
        },
        Command::ImportFiles {
            paths,
            progress,
            cancel,
        } => Response::ImportFiles {
            id,
            result: services
                .import_files_with_progress(&paths, &cancel, |tick| {
                    if let Some(sender) = progress.as_ref() {
                        let _ = sender.send(tick);
                    }
                })
                .map_err(StoreError::from),
        },
        Command::AuthorPage(request) => Response::AuthorPage {
            id,
            result: services.author_page(&request).map_err(StoreError::from),
        },
        Command::TopicPage(request) => Response::TopicPage {
            id,
            result: services.topic_page(&request).map_err(StoreError::from),
        },
        Command::HistoryList(request) => Response::HistoryList {
            id,
            result: services.history_list(&request).map_err(StoreError::from),
        },
        Command::HistoryDetail(event_id) => Response::HistoryDetail {
            id,
            result: Box::new(services.history_detail(event_id).map_err(StoreError::from)),
        },
        Command::UndoLatest => Response::UndoLatest {
            id,
            result: services.undo_latest().map_err(StoreError::from),
        },
        Command::Export(request) => Response::Export {
            id,
            result: services.export(&request).map_err(StoreError::from),
        },
        Command::TagColors => Response::TagColors {
            id,
            result: services.tag_colors().map_err(StoreError::from),
        },
        Command::SetTagColor { tag, color } => Response::SetTagColor {
            id,
            result: services
                .set_tag_color(&tag, &color)
                .map_err(StoreError::from),
        },
        Command::Shutdown => unreachable!("shutdown is handled before dispatch"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    #[test]
    fn store_handle_is_send_and_sync_for_shell_state() {
        assert_send::<StoreHandle>();
        assert_sync::<StoreHandle>();
        assert_send::<Request>();
        assert_send::<Response>();
        assert_send::<Command>();
    }

    #[test]
    fn request_ids_start_at_one_and_advance_per_issue() {
        let handle = StoreHandle::open_memory().expect("in-memory actor should open");
        assert_eq!(handle.next_request_id(), 1);
        let first = handle.stats();
        let second = handle.stats();
        assert_eq!(first, 1);
        assert_eq!(second, 2);
        assert_eq!(handle.next_request_id(), 3);
        handle.shutdown();
    }
}
