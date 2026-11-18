//! eterea Rune shell.
//!
//! The screen is the approved Daylight surface: a 720px reading column with a
//! 150px marginalia rail. The keyboard is Signal's: j/k, Enter, Esc, f, c, o, m.
//! Data comes from the background store; media bytes from the media workers.

use crossbeam_channel::Receiver as ProgressReceiver;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, NaiveDate, Utc};
use eterea_app::{
    AuthorSummary, BookmarkQuery, BookmarkSelection, BookmarkStats, CalendarData, CalendarRequest,
    CalendarYearData, CalendarYearRequest, DirectoryPageRequest, ExportFormat, ExportRequest,
    ExportScope, GraphData, GraphRequest, HistoryDetail, HistoryEvent, HistoryKind,
    HistoryListRequest, HistoryState, ImportBatchPreview, ImportBatchSummary, ImportProgress,
    RepairEvent, RepairOptions, RequestId, Response, StoreHandle, TagMutation, TagSuggestion,
    TopicSummary,
};
use eterea_media::{ImageKind, VideoCache, VideoCacheConfig, VideoPlayer, VideoPlayerOptions};
use eterea_ui_model::{Mode, Route};
use rune::{
    canvas, col, el, row, text, text_input, Align, AppSpec, Color, Cx, Drag, El, FileDialogOptions,
    Image, ImageCache, Key, KeyEvent, LineCap, Link, Live, MagnifyEvent, Phase, PointerButton,
    ScrollSpec, Shape, Span, SurfaceFit, TextEdit, Tx, Waker, WheelEvent,
};

use crate::images::ImageStore;
use crate::model::{self, Post};
use crate::settings;
use crate::theme::{self, Palette};

const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/Newsreader.ttf"),
    include_bytes!("../assets/fonts/InstrumentSans.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf"),
];
const FAMILIES: (&str, &str) = ("Instrument Sans", "IBM Plex Mono");

const CONTENT: f32 = 720.0;
const DETAIL_CONTENT: f32 = 740.0;
const RAIL: f32 = 86.0;
const GAP: f32 = 12.0;
const PAGE: usize = 48;

struct Status {
    text: String,
    until: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    Palette,
    Keys,
    Lightbox,
    Tags,
    ConfirmDelete,
    Export,
    BulkTag,
    GraphColor,
    Note,
    TagRename,
    GraphSearch,
}

struct TagEdit {
    bookmark_id: String,
    tags: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CommandItem {
    Feed,
    Favorites,
    Unread,
    Authors,
    Topics,
    Graph,
    Calendar,
    Import,
    History,
    Settings,
    Export,
    Favorite,
    LoadMedia,
    CopyLink,
    OpenOnX,
    ClearFilter,
    Reload,
    RepairMedia,
    CancelRepair,
    BackupNow,
    IntegrityCheck,
    RestoreBackup,
    BackupList,
    RestoreDeleted,
    RetryMedia,
    EditNote,
    NextUnread,
    RenameTag,
    PruneTags,
}

const COMMANDS: &[(&str, CommandItem)] = &[
    ("feed", CommandItem::Feed),
    ("favorites", CommandItem::Favorites),
    ("unread", CommandItem::Unread),
    ("authors", CommandItem::Authors),
    ("topics", CommandItem::Topics),
    ("graph", CommandItem::Graph),
    ("calendar", CommandItem::Calendar),
    ("import", CommandItem::Import),
    ("history", CommandItem::History),
    ("settings", CommandItem::Settings),
    ("export", CommandItem::Export),
    ("favorite", CommandItem::Favorite),
    ("load media", CommandItem::LoadMedia),
    ("copy link", CommandItem::CopyLink),
    ("open on x", CommandItem::OpenOnX),
    ("clear filter", CommandItem::ClearFilter),
    ("reload", CommandItem::Reload),
    ("repair media", CommandItem::RepairMedia),
    ("cancel repair", CommandItem::CancelRepair),
    ("backup now", CommandItem::BackupNow),
    ("list backups", CommandItem::BackupList),
    ("check integrity", CommandItem::IntegrityCheck),
    ("restore backup", CommandItem::RestoreBackup),
    ("restore last deleted", CommandItem::RestoreDeleted),
    ("edit note", CommandItem::EditNote),
    ("next unread", CommandItem::NextUnread),
    ("rename tag", CommandItem::RenameTag),
    ("prune tags", CommandItem::PruneTags),
    ("retry media", CommandItem::RetryMedia),
];

enum InitResult {
    Ready(StoreHandle),
    Failed(String),
}

enum ImportState {
    Idle,
    Previewing {
        paths: Vec<PathBuf>,
    },
    Preview {
        paths: Vec<PathBuf>,
        preview: ImportBatchPreview,
    },
    Importing {
        paths: Vec<PathBuf>,
    },
    Done {
        summary: ImportBatchSummary,
    },
    Error {
        message: String,
        paths: Vec<PathBuf>,
    },
}

#[derive(Default)]
struct SelectionState {
    mode: bool,
    ids: HashSet<String>,
    predicate: Option<BookmarkQuery>,
    affected: Option<i64>,
    anchor: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExportScopeChoice {
    All,
    Filter,
    Selection,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExportKind {
    Csv,
    Json,
}

#[derive(Clone, Copy)]
struct ExportSheet {
    scope: ExportScopeChoice,
    format: ExportKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CalendarMode {
    Month,
    Year,
}

/// Where the reader left the feed when a post opened. `top` is the opened
/// post's window y when it is on screen (the layout-accurate anchor); `scroll`
/// is the raw viewport offset used when the selection was off screen.
#[derive(Clone, Copy, Debug)]
struct FeedReturn {
    index: usize,
    top: Option<f32>,
    scroll: f32,
}

#[derive(Clone)]
enum Filter {
    Author(String),
    Topic(String),
    Day(String, String),
    Query(String),
    Favorites,
    Unread,
}

pub(crate) struct State {
    posts: Vec<Post>,
    total: i64,
    selected: usize,
    route: Route,
    /// Routes navigated through, newest last. Mouse back pops one.
    back_stack: Vec<Route>,
    /// Routes the mouse forward button can return to.
    forward_stack: Vec<Route>,
    store: Option<StoreHandle>,
    images: ImageStore,
    logo: Option<Image>,
    logo_light: Option<Image>,
    dark: Option<bool>,
    status: Option<Status>,
    feed_pending: bool,
    /// Whether the archive has feed posts past the ones loaded.
    feed_has_more: bool,
    /// Identity of the newest feed request; older replies are dropped so a
    /// superseded filter or search cannot overwrite fresher results.
    feed_request: Option<RequestId>,
    /// Live viewport offset / selected-row top / viewport height for the
    /// feed-return anchor. Live values update without re-running the view.
    feed_scroll_offset: Live<f32>,
    feed_anchor_top: Live<f32>,
    feed_viewport_top: Live<f32>,
    feed_viewport_height: Live<f32>,
    /// Where to put the feed back when returning from a detail page.
    feed_return: Option<FeedReturn>,
    /// A pending restore to apply when the feed view is next drawn.
    feed_restore: Option<FeedReturn>,
    /// Clock time of a first `g` press, for the `g g` top-of-feed mnemonic.
    pending_g: Option<f64>,
    /// Last laid-out window size and desktop origin, for persistence on close.
    window_size: Option<(f32, f32)>,
    /// Backups listed when Settings opened, for the safety section.
    backup_count: Option<usize>,
    window_origin: Option<(i32, i32)>,
    favorite_request: Option<(u64, String)>,
    delete_request: Option<(u64, String)>,
    demo: bool,
    /// Test/headless switch: load every post's media after the feed arrives.
    auto_media: bool,
    authors: Vec<AuthorSummary>,
    authors_total: i64,
    authors_pending: bool,
    topics: Vec<TopicSummary>,
    topics_total: i64,
    topics_pending: bool,
    /// Path of the archive database, for the opt-in repair worker.
    db_path: Option<PathBuf>,
    /// Progress from the opt-in metadata repair worker.
    repair_rx: Option<Receiver<RepairEvent>>,
    repair_cancel: Option<Arc<AtomicBool>>,
    repair_active: bool,
    /// Visited/total posts from the running repair, for the footer bar.
    repair_progress: Option<(usize, usize)>,
    /// Avatar URLs per handle, newest first. The champion is shown on every
    /// post of that handle, so a changed account stops showing an expired
    /// profile-image URL on its older posts.
    avatar_variants: HashMap<String, Vec<String>>,
    /// Index into the handle's variant list for the avatar currently shown.
    avatar_choice: HashMap<String, usize>,
    /// The shared authors/topics filter text.
    directory_filter: String,
    /// Whether the directory filter field owns the keyboard.
    directory_editing: bool,
    /// A filter keystroke arrived while an authors page was still loading.
    authors_dirty: bool,
    /// The same, for topics.
    topics_dirty: bool,
    authors_request: Option<RequestId>,
    topics_request: Option<RequestId>,
    graph: Option<GraphData>,
    calendar: Option<CalendarData>,
    /// Calendar browsing state: current month, month/year mode, selected week,
    /// focused month in the year view, cached months and year heatmap.
    calendar_month: String,
    calendar_mode: CalendarMode,
    calendar_week: usize,
    calendar_year: i32,
    calendar_year_focus: usize,
    calendar_year_data: Option<CalendarYearData>,
    calendar_cache: HashMap<String, CalendarData>,
    calendar_request: Option<RequestId>,
    calendar_year_request: Option<RequestId>,
    /// A post to open after the day-filtered feed lands.
    pending_calendar_open: Option<String>,
    filter: Option<Filter>,
    pending_views: usize,
    latest_month: Option<String>,
    mode: Mode,
    overlay: Overlay,
    search: String,
    palette: String,
    palette_index: usize,
    lightbox: Option<(usize, usize)>,
    import: ImportState,
    import_paths: Vec<PathBuf>,
    import_picker: Option<Receiver<Vec<PathBuf>>>,
    restore_picker: Option<Receiver<Option<PathBuf>>>,
    import_drop: bool,
    import_pending: bool,
    /// Note editor buffer and the bookmark being edited.
    note_buffer: TextEdit,
    note_target: Option<String>,
    /// Tag rename overlay: the source tag (None until chosen) and its new name.
    tag_rename_from: Option<String>,
    tag_rename_buffer: TextEdit,
    tag_suggestions: Vec<TagSuggestion>,
    /// Lightbox zoom (1.0 fits the window; keys/wheel adjust it).
    lightbox_zoom: f32,
    /// Previewed timeline offset while dragging the video scrub bar.
    video_scrub: Option<f64>,
    /// Thread of the post open in detail, when the source carried thread ids.
    thread: Vec<Post>,
    thread_anchor: Option<String>,
    thread_request: Option<RequestId>,
    /// Committed graph tag filter and the live search buffer while editing it.
    graph_filter: Option<String>,
    graph_search_buffer: TextEdit,
    /// Latest import progress tick and the channel it arrives on.
    import_progress: Option<ImportProgress>,
    import_progress_rx: Option<ProgressReceiver<ImportProgress>>,
    video: Option<VideoPlayer>,
    video_cache: Option<Arc<VideoCache>>,
    video_target: Option<(usize, usize)>,
    video_image: Option<Image>,
    video_pending: Option<Image>,
    auto_play: bool,
    avatars: bool,
    stats: Option<BookmarkStats>,
    text_size: u8,
    motion_reduced: Option<bool>,
    init_rx: Option<Receiver<InitResult>>,
    booted: bool,
    directory_index: usize,
    graph_zoom: f32,
    graph_pan: (f32, f32),
    graph_focus: Option<usize>,
    graph_press: Option<usize>,
    graph_hover: Option<usize>,
    graph_dragged: bool,
    /// Laid-out size of the full-bleed graph field, updated on layout.
    graph_viewport: (f32, f32),
    graph_drag_pan: Option<(f32, f32)>,
    tag_edit: Option<TagEdit>,
    tag_buffer: TextEdit,
    delete_target: Option<String>,
    history: Vec<HistoryEvent>,
    history_total: i64,
    history_selected: Option<i64>,
    history_detail: Option<HistoryDetail>,
    export_sheet: Option<ExportSheet>,
    selection: SelectionState,
    bulk_delete: bool,
    bulk_tag_remove: bool,
    tag_colors: HashMap<String, String>,
    graph_color_tag: Option<String>,
    pending_focus: Option<String>,
}

impl State {
    fn new() -> Self {
        // The real cache/store are created in setup, where the waker exists.
        Self {
            posts: Vec::new(),
            total: 0,
            selected: 0,
            route: Route::Feed,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            store: None,
            images: ImageStore::empty(),
            logo: None,
            logo_light: None,
            dark: None,
            status: None,
            feed_pending: false,
            feed_has_more: false,
            feed_request: None,
            feed_scroll_offset: Live::new(0.0),
            feed_anchor_top: Live::new(0.0),
            feed_viewport_top: Live::new(0.0),
            feed_viewport_height: Live::new(0.0),
            feed_return: None,
            feed_restore: None,
            pending_g: None,
            window_size: None,
            window_origin: None,
            backup_count: None,
            favorite_request: None,
            delete_request: None,
            demo: false,
            auto_media: false,
            authors: Vec::new(),
            authors_total: 0,
            authors_pending: false,
            topics: Vec::new(),
            topics_total: 0,
            topics_pending: false,
            db_path: None,
            repair_rx: None,
            repair_cancel: None,
            repair_active: false,
            repair_progress: None,
            avatar_variants: HashMap::new(),
            avatar_choice: HashMap::new(),
            directory_filter: String::new(),
            directory_editing: false,
            authors_dirty: false,
            topics_dirty: false,
            authors_request: None,
            topics_request: None,
            graph: None,
            calendar: None,
            calendar_month: String::new(),
            calendar_mode: CalendarMode::Month,
            calendar_week: 0,
            calendar_year: 0,
            calendar_year_focus: 0,
            calendar_year_data: None,
            calendar_cache: HashMap::new(),
            calendar_request: None,
            calendar_year_request: None,
            pending_calendar_open: None,
            filter: None,
            pending_views: 0,
            latest_month: None,
            mode: Mode::Normal,
            overlay: Overlay::None,
            search: String::new(),
            palette: String::new(),
            palette_index: 0,
            lightbox: None,
            import: ImportState::Idle,
            import_paths: Vec::new(),
            import_picker: None,
            restore_picker: None,
            import_drop: false,
            import_pending: false,
            note_buffer: TextEdit::new(""),
            note_target: None,
            tag_rename_from: None,
            tag_rename_buffer: TextEdit::new(""),
            tag_suggestions: Vec::new(),
            lightbox_zoom: 1.0,
            video_scrub: None,
            thread: Vec::new(),
            thread_anchor: None,
            thread_request: None,
            graph_filter: None,
            graph_search_buffer: TextEdit::new(""),
            import_progress: None,
            import_progress_rx: None,
            video: None,
            video_cache: None,
            video_target: None,
            video_image: None,
            video_pending: None,
            auto_play: false,
            avatars: true,
            stats: None,
            text_size: 0,
            motion_reduced: None,
            init_rx: None,
            booted: false,
            directory_index: 0,
            graph_zoom: 1.0,
            graph_pan: (0.0, 0.0),
            graph_focus: None,
            graph_press: None,
            graph_hover: None,
            graph_dragged: false,
            graph_viewport: (720.0, 720.0),
            graph_drag_pan: None,
            tag_edit: None,
            tag_buffer: TextEdit::new(""),
            delete_target: None,
            history: Vec::new(),
            history_total: 0,
            history_selected: None,
            history_detail: None,
            export_sheet: None,
            selection: SelectionState::default(),
            bulk_delete: false,
            bulk_tag_remove: false,
            tag_colors: HashMap::new(),
            graph_color_tag: None,
            pending_focus: None,
        }
    }

    fn set_status(&mut self, text: impl Into<String>, now: f64) {
        self.status = Some(Status {
            text: text.into(),
            until: now + 2.0,
        });
    }

    fn current_query(&self) -> BookmarkQuery {
        self.feed_query(0)
    }

    /// One page of the current feed filter, starting at `offset`.
    fn feed_query(&self, offset: usize) -> BookmarkQuery {
        let mut query = BookmarkQuery {
            offset,
            limit: PAGE,
            ..BookmarkQuery::default()
        };
        match &self.filter {
            Some(Filter::Author(handle)) => query.author = Some(handle.clone()),
            Some(Filter::Topic(tag)) => query.tag = Some(tag.clone()),
            Some(Filter::Day(from, to)) => {
                query.from_date = Some(from.clone());
                query.to_date = Some(to.clone());
            }
            Some(Filter::Query(text)) => query.query = Some(text.clone()),
            Some(Filter::Favorites) => query.favorites_only = true,
            Some(Filter::Unread) => query.unread_only = true,
            None => {}
        }
        query
    }

    /// The directory filter, unless it is blank.
    fn directory_filter_value(&self) -> Option<String> {
        let filter = self.directory_filter.trim();
        if filter.is_empty() {
            None
        } else {
            Some(filter.to_string())
        }
    }

    fn request_history(&mut self) {
        if let Some(store) = self.store.as_ref() {
            store.history_list(HistoryListRequest {
                offset: 0,
                limit: 5,
                include_undone: true,
            });
            self.pending_views += 1;
        }
    }

    fn select_history(&mut self, id: i64) {
        self.history_selected = Some(id);
        if let Some(store) = self.store.as_ref() {
            store.history_detail(id);
            self.pending_views += 1;
        }
    }

    fn undo_latest(&mut self) {
        if let Some(store) = self.store.as_ref() {
            store.undo_latest();
            self.pending_views += 1;
        }
    }

    fn open_export(&mut self) {
        let scope = if self.filter.is_some() {
            ExportScopeChoice::Filter
        } else {
            ExportScopeChoice::All
        };
        self.export_sheet = Some(ExportSheet {
            scope,
            format: ExportKind::Csv,
        });
        self.overlay = Overlay::Export;
    }

    fn export_now(&mut self, tx: &mut Tx) {
        let Some(sheet) = self.export_sheet else {
            return;
        };
        let scope = match sheet.scope {
            ExportScopeChoice::All => ExportScope::All,
            ExportScopeChoice::Filter => ExportScope::Query(self.current_query()),
            ExportScopeChoice::Selection => {
                if self.selection.ids.is_empty() && self.selection.predicate.is_none() {
                    ExportScope::All
                } else {
                    ExportScope::from_selection(&current_selection(self))
                }
            }
        };
        let format = match sheet.format {
            ExportKind::Csv => ExportFormat::Csv,
            ExportKind::Json => ExportFormat::Json,
        };
        let extension = match sheet.format {
            ExportKind::Csv => "csv",
            ExportKind::Json => "json",
        };
        let name = format!("eterea-{}.{}", Utc::now().format("%Y-%m-%d"), extension);
        let options = FileDialogOptions {
            title: "export bookmarks".into(),
            file_name: Some(name),
            ..FileDialogOptions::default()
        }
        .filter(extension.to_ascii_uppercase(), &[extension]);
        tx.save_file(options, move |state: &mut State, result, tx| {
            let Ok(Some(path)) = result else {
                if let Err(error) = result {
                    let now = tx.clock();
                    state.set_status(error.to_string(), now);
                }
                return;
            };
            state.overlay = Overlay::None;
            state.export_sheet = None;
            let now = tx.clock();
            state.set_status("exporting", now);
            if let Some(store) = state.store.as_ref() {
                store.export(ExportRequest {
                    scope,
                    format,
                    path,
                });
            }
        });
    }

    fn request_authors(&mut self, offset: usize) {
        if self.authors_pending {
            if offset == 0 {
                self.authors_dirty = true;
            }
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        self.authors_pending = true;
        self.authors_dirty = false;
        self.pending_views += 1;
        self.authors_request = Some(store.author_page(DirectoryPageRequest {
            offset,
            limit: 100,
            filter: self.directory_filter_value(),
        }));
    }

    fn request_topics(&mut self, offset: usize) {
        if self.topics_pending {
            if offset == 0 {
                self.topics_dirty = true;
            }
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        self.topics_pending = true;
        self.topics_dirty = false;
        self.pending_views += 1;
        self.topics_request = Some(store.topic_page(DirectoryPageRequest {
            offset,
            limit: 100,
            filter: self.directory_filter_value(),
        }));
    }

    fn load_more_authors(&mut self) {
        if self.authors.len() < self.authors_total as usize {
            self.request_authors(self.authors.len());
        }
    }

    fn load_more_topics(&mut self) {
        if self.topics.len() < self.topics_total as usize {
            self.request_topics(self.topics.len());
        }
    }

    fn request_feed(&mut self, filter: Option<Filter>) {
        self.feed_return = None;
        self.feed_restore = None;
        self.filter = filter;
        self.feed_has_more = false;
        let Some(store) = self.store.as_ref() else {
            return;
        };
        self.feed_request = Some(store.feed(self.feed_query(0)));
        self.feed_pending = true;
    }

    /// Fetch the next feed page and append it when it arrives.
    fn load_more_feed(&mut self) {
        if self.feed_pending || !self.feed_has_more || self.posts.is_empty() {
            return;
        }
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let offset = self.posts.len();
        self.feed_request = Some(store.feed(self.feed_query(offset)));
        self.feed_pending = true;
    }

    /// Remember the route being left so the mouse back button can return to it.
    fn note_navigation(&mut self, next: Route) {
        if self.route != next {
            self.back_stack.push(self.route);
            if self.back_stack.len() > 32 {
                self.back_stack.remove(0);
            }
            self.forward_stack.clear();
        }
    }

    fn open_route(&mut self, route: Route) {
        self.note_navigation(route);
        self.apply_route(route);
    }

    /// Enter a route and make sure its data is loaded, without touching the
    /// navigation stacks.
    fn apply_route(&mut self, route: Route) {
        let previous = self.route;
        stop_video(self);
        self.route = route;
        self.directory_editing = false;
        if route == Route::Import
            && previous != Route::Import
            && !matches!(
                self.import,
                ImportState::Previewing { .. } | ImportState::Importing { .. }
            )
        {
            self.import = ImportState::Idle;
            self.import_paths.clear();
        }
        match route {
            Route::Feed => self.request_feed(None),
            Route::Authors => {
                self.directory_filter.clear();
                self.directory_index = 0;
                self.authors_dirty = false;
                self.topics_dirty = false;
                self.request_authors(0);
            }
            Route::Topics => {
                self.directory_filter.clear();
                self.directory_index = 0;
                self.authors_dirty = false;
                self.topics_dirty = false;
                self.request_topics(0);
            }
            Route::Graph => {
                if self.graph.is_none() {
                    if let Some(store) = self.store.as_ref() {
                        store.graph(GraphRequest::new(800, 4_000));
                        self.pending_views += 1;
                    }
                }
                if self.tag_colors.is_empty() {
                    if let Some(store) = self.store.as_ref() {
                        store.tag_colors();
                        self.pending_views += 1;
                    }
                }
            }
            Route::History => self.request_history(),
            Route::Settings => {
                // The backup count is filled by an explicit action rather than
                // on open, so the screen renders the same state every time.
                if self.stats.is_none() {
                    if let Some(store) = self.store.as_ref() {
                        store.stats();
                        self.pending_views += 1;
                    }
                }
            }
            Route::Calendar => {
                if self.calendar_month.is_empty() {
                    self.calendar_month = self
                        .latest_month
                        .clone()
                        .unwrap_or_else(|| Utc::now().format("%Y-%m").to_string());
                }
                let month = self.calendar_month.clone();
                self.request_calendar_month(month.clone());
                self.ensure_calendar_year(month_year(&month));
            }
            _ => {}
        }
    }

    fn request_calendar_month(&mut self, month: String) {
        self.calendar_mode = CalendarMode::Month;
        self.calendar_month = month.clone();
        if let Some(cached) = self.calendar_cache.get(&month).cloned() {
            self.calendar = Some(cached);
            self.calendar_request = None;
            self.clamp_calendar_week();
            return;
        }
        let Some(store) = self.store.as_ref() else {
            return;
        };
        self.calendar_request = Some(store.calendar(CalendarRequest::new(month)));
        self.pending_views += 1;
    }

    fn ensure_calendar_year(&mut self, year: i32) {
        self.calendar_year = year;
        if self
            .calendar_year_data
            .as_ref()
            .is_some_and(|data| data.year == year)
        {
            return;
        }
        let Some(store) = self.store.as_ref() else {
            return;
        };
        self.calendar_year_request = Some(store.calendar_year(CalendarYearRequest::new(year)));
        self.pending_views += 1;
    }

    fn clamp_calendar_week(&mut self) {
        let weeks = self.calendar.as_ref().map_or(1, |calendar| calendar.weeks);
        self.calendar_week = self.calendar_week.min(weeks.saturating_sub(1));
    }

    fn shift_calendar_month(&mut self, delta: i32) {
        let Some(next) = shift_month(&self.calendar_month, delta) else {
            return;
        };
        self.calendar_week = 0;
        self.request_calendar_month(next.clone());
        self.ensure_calendar_year(month_year(&next));
    }

    fn shift_calendar_week(&mut self, delta: i32) {
        let weeks = self.calendar.as_ref().map_or(1, |calendar| calendar.weeks) as i32;
        let next = self.calendar_week as i32 + delta;
        if next < 0 {
            self.calendar_week = usize::MAX;
            if let Some(previous) = shift_month(&self.calendar_month, -1) {
                self.request_calendar_month(previous.clone());
                self.ensure_calendar_year(month_year(&previous));
            }
        } else if next >= weeks {
            self.calendar_week = 0;
            if let Some(following) = shift_month(&self.calendar_month, 1) {
                self.request_calendar_month(following.clone());
                self.ensure_calendar_year(month_year(&following));
            }
        } else {
            self.calendar_week = next as usize;
        }
    }

    fn toggle_calendar_mode(&mut self) {
        match self.calendar_mode {
            CalendarMode::Month => {
                self.calendar_mode = CalendarMode::Year;
                if let Some(index) = month_index(&self.calendar_month) {
                    self.calendar_year_focus = index;
                }
                self.ensure_calendar_year(self.calendar_year);
            }
            CalendarMode::Year => {
                let year = self.calendar_year;
                let month = self.calendar_year_focus.min(11) + 1;
                self.request_calendar_month(format!("{year:04}-{month:02}"));
            }
        }
    }

    fn shift_calendar_year(&mut self, delta: i32) {
        let year = self.calendar_year + delta;
        self.ensure_calendar_year(year);
    }

    fn calendar_today(&mut self) {
        let month = Utc::now().format("%Y-%m").to_string();
        self.calendar_week = 0;
        self.request_calendar_month(month.clone());
        self.ensure_calendar_year(month_year(&month));
    }

    fn filter_calendar_week(&mut self) {
        let Some(calendar) = self.calendar.as_ref() else {
            return;
        };
        let week = self.calendar_week.min(calendar.weeks.saturating_sub(1));
        let Some(from) = calendar.days.get(week * 7) else {
            return;
        };
        let Some(to) = calendar.days.get(week * 7 + 6) else {
            return;
        };
        self.filter_day_range(&from.date.clone(), &to.date.clone());
    }

    fn open_calendar_post(&mut self, day: &str, bookmark_id: String) {
        self.pending_calendar_open = Some(bookmark_id);
        self.filter_day(day);
    }

    fn filter_day_range(&mut self, from: &str, to: &str) {
        let (Ok(from_date), Ok(to_date)) = (
            NaiveDate::parse_from_str(from, "%Y-%m-%d"),
            NaiveDate::parse_from_str(to, "%Y-%m-%d"),
        ) else {
            return;
        };
        let Some(start) = from_date.and_hms_opt(0, 0, 0) else {
            return;
        };
        let Some(next_day) = to_date.succ_opt() else {
            return;
        };
        let Some(end) = next_day.and_hms_opt(0, 0, 0) else {
            return;
        };
        self.note_navigation(Route::Feed);
        self.route = Route::Feed;
        self.request_feed(Some(Filter::Day(
            format!("{}Z", start.format("%Y-%m-%dT%H:%M:%S")),
            format!("{}Z", end.format("%Y-%m-%dT%H:%M:%S")),
        )));
    }

    fn filter_author(&mut self, handle: &str) {
        self.note_navigation(Route::Feed);
        self.route = Route::Feed;
        self.request_feed(Some(Filter::Author(handle.to_string())));
    }

    fn filter_topic(&mut self, tag: &str) {
        self.note_navigation(Route::Feed);
        self.route = Route::Feed;
        self.request_feed(Some(Filter::Topic(tag.to_string())));
    }

    fn filter_day(&mut self, day: &str) {
        self.filter_day_range(day, day);
    }

    fn choose_import_files(&mut self) {
        if self.import_picker.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.import_picker = Some(receiver);
        let _ = std::thread::Builder::new()
            .name("eterea-file-picker".into())
            .spawn(move || {
                let picked = rfd::FileDialog::new()
                    .add_filter("Bookmarks", &["csv", "json", "js"])
                    .set_title("import bookmarks exports")
                    .pick_files()
                    .unwrap_or_default();
                let _ = sender.send(picked);
            });
    }

    fn preview_import_paths(&mut self, paths: Vec<PathBuf>) {
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        matches!(ext.to_ascii_lowercase().as_str(), "csv" | "json" | "js")
                    })
            })
            .collect();
        if paths.is_empty() {
            return;
        }
        self.import_paths = paths.clone();
        self.import_pending = true;
        if let Some(store) = self.store.as_ref() {
            self.import = ImportState::Previewing {
                paths: paths.clone(),
            };
            store.preview_files(paths);
        } else {
            self.import = ImportState::Error {
                message: "archive is not ready".into(),
                paths,
            };
            self.import_pending = false;
        }
    }

    fn drop_import_paths(&mut self, paths: &[PathBuf]) {
        self.route = Route::Import;
        self.preview_import_paths(paths.to_vec());
    }

    fn start_import(&mut self) {
        let paths = self.import_paths.clone();
        if paths.is_empty() {
            return;
        }
        self.import = ImportState::Importing {
            paths: paths.clone(),
        };
        self.import_pending = true;
        self.import_progress = None;
        if let Some(store) = self.store.as_ref() {
            let (_, progress) = store.import_files_with_progress(paths);
            self.import_progress_rx = Some(progress);
        }
    }

    fn refresh_feed(&mut self) {
        let filter = self.filter.clone();
        self.request_feed(filter);
    }

    fn filtered_commands(&self) -> Vec<(&'static str, CommandItem)> {
        let needle = self.palette.to_lowercase();
        COMMANDS
            .iter()
            .copied()
            .filter(|(name, _)| needle.is_empty() || name.contains(&needle))
            // Cancelling only exists while a repair is running.
            .filter(|(_, item)| *item != CommandItem::CancelRepair || self.repair_active)
            .collect()
    }

    fn run_command(&mut self, item: CommandItem, tx: &mut Tx) {
        match item {
            CommandItem::Feed => self.open_route(Route::Feed),
            CommandItem::Favorites => {
                self.route = Route::Feed;
                self.request_feed(Some(Filter::Favorites));
            }
            CommandItem::Unread => {
                self.route = Route::Feed;
                self.request_feed(Some(Filter::Unread));
            }
            CommandItem::Authors => self.open_route(Route::Authors),
            CommandItem::Topics => self.open_route(Route::Topics),
            CommandItem::Graph => self.open_route(Route::Graph),
            CommandItem::Calendar => self.open_route(Route::Calendar),
            CommandItem::Import => self.open_route(Route::Import),
            CommandItem::History => self.open_route(Route::History),
            CommandItem::Settings => self.open_route(Route::Settings),
            CommandItem::Export => self.open_export(),
            CommandItem::Favorite => toggle_favorite(self),
            CommandItem::LoadMedia => load_media(self, self.selected),
            CommandItem::CopyLink => {
                if let Some(post) = self.posts.get(self.selected) {
                    tx.copy(post.tweet_url.clone());
                    let now = tx.clock();
                    self.set_status("link copied", now);
                }
            }
            CommandItem::OpenOnX => {
                if let Some(post) = self.posts.get(self.selected) {
                    if eterea_logic::is_safe_https_url(&post.tweet_url) {
                        let _ = tx.open_url(&post.tweet_url);
                    }
                }
            }
            CommandItem::ClearFilter => self.request_feed(None),
            CommandItem::Reload => {
                let filter = self.filter.clone();
                self.request_feed(filter);
            }
            CommandItem::RepairMedia => {
                let now = tx.clock();
                start_media_repair(self, now);
            }
            CommandItem::CancelRepair => request_cancel_repair(self),
            CommandItem::BackupNow => {
                if let Some(store) = self.store.as_ref() {
                    store.backup_now();
                    store.backup_list();
                    self.pending_views += 2;
                    let now = tx.clock();
                    self.set_status("writing backup", now);
                }
            }
            CommandItem::BackupList => {
                if let Some(store) = self.store.as_ref() {
                    store.backup_list();
                    self.pending_views += 1;
                }
            }
            CommandItem::IntegrityCheck => {
                if let Some(store) = self.store.as_ref() {
                    store.integrity_check();
                    self.pending_views += 1;
                    let now = tx.clock();
                    self.set_status("checking database", now);
                }
            }
            CommandItem::RestoreBackup => self.choose_restore_backup(),
            CommandItem::RetryMedia => {
                self.images.retry_failed();
                request_avatars(self);
                request_all_media(self);
                let now = tx.clock();
                self.set_status("retrying media", now);
            }
            CommandItem::EditNote => open_note_editor(self),
            CommandItem::NextUnread => next_unread(self),
            CommandItem::RenameTag => open_tag_rename(self),
            CommandItem::PruneTags => {
                if let Some(store) = self.store.as_ref() {
                    store.prune_tags();
                    self.pending_views += 1;
                }
            }
            CommandItem::RestoreDeleted => {
                if let Some(store) = self.store.as_ref() {
                    store.restore_deleted();
                    self.pending_views += 1;
                    let now = tx.clock();
                    self.set_status("restoring deleted post", now);
                }
            }
        }
    }

    fn choose_restore_backup(&mut self) {
        if self.restore_picker.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.restore_picker = Some(receiver);
        let _ = std::thread::Builder::new()
            .name("eterea-restore-picker".into())
            .spawn(move || {
                let picked = rfd::FileDialog::new()
                    .add_filter("Eterea database", &["db", "sqlite", "sqlite3"])
                    .set_title("restore an eterea backup")
                    .pick_file();
                let _ = sender.send(picked);
            });
    }

    fn close_overlay(&mut self) {
        dismiss_overlay(self);
    }
}

/// Close the top overlay and reset every transient flag it could set. A bulk
/// confirmation used to leave `bulk_delete` behind, so a later single delete
/// ran the bulk path and silently deleted nothing.
fn dismiss_overlay(state: &mut State) {
    state.overlay = Overlay::None;
    state.palette.clear();
    state.palette_index = 0;
    state.lightbox = None;
    state.tag_edit = None;
    state.delete_target = None;
    state.export_sheet = None;
    state.graph_color_tag = None;
    state.bulk_delete = false;
    state.bulk_tag_remove = false;
}

fn setup(state: &mut State, waker: Waker) {
    state.db_path = Some(
        std::env::var_os("ETEREA_DB")
            .map(PathBuf::from)
            .unwrap_or_else(eterea_core::Database::default_path),
    );
    let cache = ImageCache::new(waker.clone());
    // Decode the logo off the UI thread; the view draws whichever is ready.
    state.logo = Some(cache.svg(
        &include_bytes!("../../../docs/assets/brand/eterea-orb-header.svg")[..],
        20.0,
        1.0,
    ));
    state.logo_light = state.logo.clone();
    state.images = ImageStore::with_cache(cache, Some(&media_cache_dir()));
    state.demo = std::env::var_os("ETEREA_DEMO").is_some();
    state.auto_media = std::env::var_os("ETEREA_HIDE_MEDIA").is_none();
    state.auto_play = std::env::var_os("ETEREA_PLAY").is_some();
    let stored = settings::load();
    state.dark = match stored.theme.as_deref() {
        Some("dark") => Some(true),
        Some("light") => Some(false),
        _ => None,
    };
    state.avatars = stored.avatars.unwrap_or(true);
    state.auto_media = if std::env::var_os("ETEREA_HIDE_MEDIA").is_some() {
        false
    } else {
        stored.media.unwrap_or(true)
    };
    state.motion_reduced = match stored.motion.as_deref() {
        Some("reduced") => Some(true),
        Some("system") => Some(false),
        _ => None,
    };
    state.text_size = stored.text_size.unwrap_or(0).min(2);
    theme::set_text_scale(match state.text_size {
        1 => 1.12,
        2 => 1.24,
        _ => 1.0,
    });

    // Demo mode is test-only and uses the local fixture deck synchronously so
    // screenshots stay deterministic. Real startup never blocks the UI here.
    if state.demo {
        state.posts = model::demo_posts();
        state.total = state.posts.len() as i64;
        preload_demo_images(state);
        request_avatars(state);
        for post in &mut state.posts {
            post.media_loaded = true;
        }
        state.booted = true;
        if state.posts.is_empty() {
            state.route = Route::Onboarding;
        }
        if let Some(route) = route_from_env() {
            state.route = route;
        }
        return;
    }

    let (tx, rx) = mpsc::channel();
    state.init_rx = Some(rx);
    std::thread::Builder::new()
        .name("eterea-init".into())
        .spawn(move || {
            let opened = match std::env::var_os("ETEREA_DB") {
                Some(path) => StoreHandle::open(Path::new(&path)),
                None => StoreHandle::open_default(),
            };
            match opened {
                Ok(store) => {
                    let _ = tx.send(InitResult::Ready(store));
                }
                Err(error) => {
                    let _ = tx.send(InitResult::Failed(format!(
                        "database could not open: {error}"
                    )));
                }
            }
            waker();
        })
        .ok();
}

fn install_store(state: &mut State, store: StoreHandle) {
    let request = store.feed(BookmarkQuery {
        offset: 0,
        limit: PAGE,
        ..BookmarkQuery::default()
    });
    state.feed_request = Some(request);
    state.store = Some(store.clone());
    state.feed_pending = true;
    if state.route == Route::Feed {
        state.route = Route::Onboarding;
    }
    if let Some(route) = route_from_env() {
        state.open_route(route);
    }
    if let Ok(path) = std::env::var("ETEREA_IMPORT") {
        state.route = Route::Import;
        state.preview_import_paths(vec![PathBuf::from(path)]);
    }
    if let Ok(value) = std::env::var("ETEREA_SELECTED") {
        if let Ok(index) = value.parse::<usize>() {
            state.selected = index;
        }
    }
    state.booted = true;
}

fn route_from_env() -> Option<Route> {
    match std::env::var("ETEREA_ROUTE").ok()?.as_str() {
        "feed" => Some(Route::Feed),
        "authors" => Some(Route::Authors),
        "topics" => Some(Route::Topics),
        "graph" => Some(Route::Graph),
        "calendar" => Some(Route::Calendar),
        "detail" => Some(Route::Detail),
        "import" => Some(Route::Import),
        "history" => Some(Route::History),
        "settings" => Some(Route::Settings),
        _ => None,
    }
}

fn preload_demo_images(state: &mut State) {
    let mut keys = Vec::new();
    for post in &state.posts {
        if let Some(key) = &post.avatar_key {
            keys.push(key.clone());
        }
        for item in &post.media {
            keys.push(item.key.clone());
        }
        if let Some(quote) = &post.quote {
            if let Some(key) = &quote.avatar_key {
                keys.push(key.clone());
            }
            if let Some(key) = &quote.thumb_key {
                keys.push(key.clone());
            }
        }
    }
    for key in keys {
        if let Some(path) = model::demo_asset(&key) {
            state.images.preload_file(&key, &path, 720.0);
        }
    }
}

fn request_avatars(state: &mut State) {
    if !state.avatars {
        return;
    }
    let mut keys = Vec::new();
    for post in &state.posts {
        if let Some(key) = &post.avatar_key {
            keys.push(key.clone());
        }
        if let Some(quote) = &post.quote {
            if let Some(key) = &quote.avatar_key {
                keys.push(key.clone());
            }
        }
    }
    for key in keys {
        if !state.images.is_failed(&key) {
            state.images.request(&key, ImageKind::Avatar);
        }
    }
}

/// Rebuild the per-handle avatar candidates from one feed page and point every
/// post and quote of that handle at the current champion URL. Feed pages are
/// newest first, so the first candidate is the newest avatar the archive knows.
fn resolve_avatars(state: &mut State, page: &[eterea_core::Bookmark], replace: bool) {
    if !state.avatars {
        return;
    }
    if replace {
        state.avatar_variants.clear();
        state.avatar_choice.clear();
    }
    for bookmark in page {
        if let Some(image) = bookmark
            .author_profile_image
            .as_deref()
            .filter(|url| !url.is_empty())
        {
            push_avatar_candidate(state, &bookmark.author_handle, image);
        }
        if let Some(quoted) = &bookmark.quoted {
            if let Some(image) = quoted
                .author_profile_image
                .as_deref()
                .filter(|url| !url.is_empty())
            {
                push_avatar_candidate(state, &quoted.author_handle, image);
            }
        }
    }
    let mut champions: HashMap<String, String> = HashMap::new();
    for handle in state.avatar_variants.keys() {
        if let Some(champion) = resolved_avatar(state, handle) {
            champions.insert(handle.clone(), champion);
        }
    }
    // The newest row for a handle carries the account's current display name.
    let mut names: HashMap<String, String> = HashMap::new();
    for post in &state.posts {
        let handle = post.handle.trim_start_matches('@').to_string();
        if !post.name.trim().is_empty() {
            names.entry(handle).or_insert_with(|| post.name.clone());
        }
    }
    for post in &mut state.posts {
        let handle = post.handle.trim_start_matches('@').to_string();
        if let Some(champion) = champions.get(&handle) {
            post.avatar_key = Some(champion.clone());
        }
        if let Some(name) = names.get(&handle) {
            post.name = name.clone();
        }
        if let Some(quote) = post.quote.as_mut() {
            let quote_handle = quote.handle.trim_start_matches('@').to_string();
            if let Some(champion) = champions.get(&quote_handle) {
                quote.avatar_key = Some(champion.clone());
            }
        }
    }
}

fn push_avatar_candidate(state: &mut State, handle: &str, image: &str) {
    if handle.is_empty() {
        return;
    }
    let candidates = state.avatar_variants.entry(handle.to_string()).or_default();
    if !candidates.iter().any(|url| url == image) {
        candidates.push(image.to_string());
    }
}

/// The first avatar URL for `handle` that has not failed this session.
fn resolved_avatar(state: &State, handle: &str) -> Option<String> {
    let candidates = state.avatar_variants.get(handle)?;
    let start = *state.avatar_choice.get(handle).unwrap_or(&0);
    candidates
        .iter()
        .skip(start)
        .find(|url| !state.images.is_failed(url))
        .cloned()
}

/// When the avatar currently shown for a handle has failed (X deletes old
/// profile-image renditions, mirrors expire), fall back to the next newest
/// candidate instead of leaving every old post on a monogram.
fn advance_failed_avatars(state: &mut State) -> bool {
    if !state.avatars {
        return false;
    }
    let mut updates: HashMap<String, String> = HashMap::new();
    let handles: Vec<String> = state.avatar_variants.keys().cloned().collect();
    for handle in handles {
        let candidates = match state.avatar_variants.get(&handle) {
            Some(candidates) => candidates.clone(),
            None => continue,
        };
        let current = *state.avatar_choice.get(&handle).unwrap_or(&0);
        let Some(current_url) = candidates.get(current) else {
            continue;
        };
        if !state.images.is_failed(current_url) {
            continue;
        }
        let mut next = current + 1;
        while next < candidates.len() && state.images.is_failed(&candidates[next]) {
            next += 1;
        }
        state.avatar_choice.insert(handle.clone(), next);
        if let Some(url) = candidates.get(next) {
            updates.insert(handle, url.clone());
        }
    }
    if updates.is_empty() {
        return false;
    }
    for post in &mut state.posts {
        let handle = post.handle.trim_start_matches('@').to_string();
        if let Some(url) = updates.get(&handle) {
            post.avatar_key = Some(url.clone());
        }
        if let Some(quote) = post.quote.as_mut() {
            let quote_handle = quote.handle.trim_start_matches('@').to_string();
            if let Some(url) = updates.get(&quote_handle) {
                quote.avatar_key = Some(url.clone());
            }
        }
    }
    for url in updates.values() {
        state.images.request(url, ImageKind::Avatar);
    }
    true
}

/// Load the portraits of the authors directory rows.
fn request_directory_avatars(state: &mut State) {
    if !state.avatars {
        return;
    }
    let keys: Vec<String> = state
        .authors
        .iter()
        .filter_map(|author| author.profile_image.clone())
        .collect();
    for key in keys {
        state.images.request(&key, ImageKind::Avatar);
    }
}

fn poll(state: &mut State, clock: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    if let Some(message) = settings::take_error() {
        state.set_status(message, clock);
        changed = true;
    }
    let mut init = None;
    if let Some(rx) = state.init_rx.as_ref() {
        match rx.try_recv() {
            Ok(result) => init = Some(result),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                init = Some(InitResult::Failed("initialization stopped".into()));
            }
        }
    }
    if let Some(result) = init {
        state.init_rx = None;
        match result {
            InitResult::Ready(store) => install_store(state, store),
            InitResult::Failed(message) => {
                state.booted = true;
                state.set_status(message, clock);
                state.route = Route::Onboarding;
            }
        }
        changed = true;
    }

    if let Some(key) = state.pending_focus.take() {
        tx.focus(key, false);
        changed = true;
    }

    if let Some(store) = state.store.as_ref() {
        let responses = store.drain();
        if !responses.is_empty() {
            changed = true;
        }
        for response in responses {
            handle_response(state, response, clock);
        }
    }

    if let Some(receiver) = state.import_progress_rx.as_ref() {
        loop {
            match receiver.try_recv() {
                Ok(progress) => {
                    state.import_progress = Some(progress);
                    changed = true;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    state.import_progress_rx = None;
                    break;
                }
            }
        }
    }
    let failed_media = state.images.take_new_failures();
    if failed_media > 0 {
        state.set_status(
            if failed_media == 1 {
                "one image unavailable, retry media to try again".to_string()
            } else {
                format!("{failed_media} images unavailable, retry media to try again")
            },
            clock,
        );
        changed = true;
    }
    if let Some(receiver) = state.import_picker.as_ref() {
        match receiver.try_recv() {
            Ok(paths) => {
                state.import_picker = None;
                if !paths.is_empty() {
                    state.preview_import_paths(paths);
                    changed = true;
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                state.import_picker = None;
                changed = true;
            }
        }
    }
    if let Some(receiver) = state.restore_picker.as_ref() {
        match receiver.try_recv() {
            Ok(Some(path)) => {
                state.restore_picker = None;
                if let Some(store) = state.store.as_ref() {
                    store.restore_backup(path);
                    state.pending_views += 1;
                    let now = tx.clock();
                    state.set_status("restoring backup", now);
                }
                changed = true;
            }
            Ok(None) => {
                state.restore_picker = None;
                changed = true;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                state.restore_picker = None;
                changed = true;
            }
        }
    }

    if state.images.poll() {
        changed = true;
    }
    if advance_failed_avatars(state) {
        changed = true;
    }

    if let Some(player) = state.video.as_mut() {
        if state
            .video_pending
            .as_ref()
            .is_some_and(|image| image.is_ready())
        {
            state.video_image = state.video_pending.take();
            changed = true;
        }
        let frame = if state.video_pending.is_none() {
            player.poll().map(|frame| frame.rgba.clone())
        } else {
            None
        };
        if let Some(bytes) = frame {
            if let Some(image) = state.images.rgba(960, 540, bytes) {
                state.video_pending = Some(image);
            }
        }
        if player.is_ended() {
            let error = player.error().map(str::to_string);
            state.video = None;
            state.video_target = None;
            if let Some(error) = error {
                let now = tx.clock();
                state.set_status(error, now);
            }
            changed = true;
        }
    }

    if state.auto_play && !state.posts.is_empty() {
        state.auto_play = false;
        toggle_video(state, tx);
        changed = true;
    }

    if poll_media_repair(state, clock) {
        changed = true;
    }

    if let Some(status) = &state.status {
        if clock > status.until {
            state.status = None;
            changed = true;
        }
    }

    if state.feed_pending
        || state.favorite_request.is_some()
        || state.pending_views > 0
        || state.import_pending
        || state.import_picker.is_some()
        || state.video.is_some()
        || state.images.pending() > 0
        || state.images.has_deferred()
        || state.status.is_some()
    {
        tx.wake_after(Duration::from_millis(33));
    }
    if state.repair_active {
        tx.wake_after(Duration::from_millis(200));
    }

    changed
}

fn handle_response(state: &mut State, response: Response, clock: f64) {
    match response {
        Response::Feed { id, result } => {
            if state.feed_request != Some(id) {
                // A superseded query already answered; keep the fresher state.
                return;
            }
            state.feed_request = None;
            state.feed_pending = false;
            match result {
                Ok(page) => {
                    let append = page.offset > 0 && !state.posts.is_empty();
                    if state.latest_month.is_none() {
                        if let Some(first) = page.items.first() {
                            let month = first.tweeted_at.format("%Y-%m").to_string();
                            state.latest_month = Some(month.clone());
                            // A calendar opened straight from a saved route asked
                            // for the current month before any feed data existed;
                            // once the archive's latest month is known, show that.
                            if state.route == Route::Calendar && state.calendar_month != month {
                                state.request_calendar_month(month);
                            }
                        }
                    }
                    if append {
                        // Append the next page without dropping the posts already
                        // loaded; duplicates are skipped defensively.
                        let known: HashSet<&str> =
                            state.posts.iter().map(|post| post.id.as_str()).collect();
                        let mut added = Vec::new();
                        for bookmark in &page.items {
                            if known.contains(bookmark.id.as_str()) {
                                continue;
                            }
                            added.push(Post::from_bookmark(bookmark));
                        }
                        drop(known);
                        state.posts.extend(added);
                    } else {
                        state.posts = page.items.iter().map(Post::from_bookmark).collect();
                        let selected = std::env::var("ETEREA_SELECTED")
                            .ok()
                            .and_then(|value| value.parse::<usize>().ok())
                            .unwrap_or(state.selected);
                        state.selected = selected.min(state.posts.len().saturating_sub(1));
                    }
                    state.total = page.total;
                    state.feed_has_more = page.has_more;
                    state.selected = state.selected.min(state.posts.len().saturating_sub(1));
                    if !append && matches!(state.route, Route::Feed | Route::Onboarding) {
                        state.route = if state.posts.is_empty() && state.filter.is_none() {
                            Route::Onboarding
                        } else {
                            Route::Feed
                        };
                    }
                    if !append && state.route == Route::Calendar {
                        if let (Some(month), Some(store)) =
                            (state.latest_month.clone(), state.store.as_ref())
                        {
                            store.calendar(CalendarRequest::new(month));
                            state.pending_views += 1;
                        }
                    }
                    resolve_avatars(state, &page.items, !append);
                    if let Some(id) = state.pending_calendar_open.take() {
                        if let Some(index) = state.posts.iter().position(|post| post.id == id) {
                            begin_detail(state, index);
                        }
                    }
                    request_avatars(state);
                    request_all_media(state);
                }
                Err(error) => {
                    state.feed_has_more = false;
                    state.set_status(error.message, clock);
                    if state.posts.is_empty() {
                        state.route = Route::Onboarding;
                    }
                }
            }
        }
        Response::ToggleFavorite { result, .. } => {
            if let Some((_, id)) = state.favorite_request.take() {
                if let Ok(value) = result {
                    if let Some(post) = state.posts.iter_mut().find(|post| post.id == id) {
                        post.favorite = value;
                    }
                }
            }
        }
        Response::AuthorPage { id, result } => {
            if state.authors_request != Some(id) {
                return;
            }
            state.authors_request = None;
            state.pending_views = state.pending_views.saturating_sub(1);
            state.authors_pending = false;
            match result {
                Ok(page) => {
                    let replace = page.offset == 0 || state.authors.is_empty();
                    state.authors_total = page.total;
                    if replace {
                        state.authors = page.items;
                        state.directory_index = 0;
                    } else {
                        state.authors.extend(page.items);
                    }
                    state.directory_index = state
                        .directory_index
                        .min(state.authors.len().saturating_sub(1));
                    request_directory_avatars(state);
                    if state.authors_dirty && state.route == Route::Authors {
                        state.request_authors(0);
                    }
                }
                Err(error) => {
                    let now = clock;
                    state.set_status(error.message, now);
                }
            }
        }
        Response::TopicPage { id, result } => {
            if state.topics_request != Some(id) {
                return;
            }
            state.topics_request = None;
            state.pending_views = state.pending_views.saturating_sub(1);
            state.topics_pending = false;
            match result {
                Ok(page) => {
                    let replace = page.offset == 0 || state.topics.is_empty();
                    state.topics_total = page.total;
                    if replace {
                        state.topics = page.items;
                        state.directory_index = 0;
                    } else {
                        state.topics.extend(page.items);
                    }
                    state.directory_index = state
                        .directory_index
                        .min(state.topics.len().saturating_sub(1));
                    if state.topics_dirty && state.route == Route::Topics {
                        state.request_topics(0);
                    }
                }
                Err(error) => {
                    let now = clock;
                    state.set_status(error.message, now);
                }
            }
        }
        Response::AuthorIndex { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            if let Ok(authors) = result {
                state.authors_total = authors.len() as i64;
                state.authors = authors;
            }
        }
        Response::TopicIndex { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            if let Ok(topics) = result {
                state.topics_total = topics.len() as i64;
                state.topics = topics;
            }
        }
        Response::TagColors { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            if let Ok(colors) = result {
                state.tag_colors = colors
                    .into_iter()
                    .map(|color| (color.tag, color.color))
                    .collect();
            }
        }
        Response::SetTagColor { result, .. } => {
            if let Ok(color) = result {
                state.tag_colors.insert(color.tag, color.color);
            }
        }
        Response::Graph { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            if let Ok(graph) = result {
                state.graph = Some(graph);
            }
        }
        Response::Calendar { id, result } => {
            if state.calendar_request != Some(id) {
                return;
            }
            state.calendar_request = None;
            state.pending_views = state.pending_views.saturating_sub(1);
            match result {
                Ok(calendar) => {
                    state
                        .calendar_cache
                        .insert(calendar.month.clone(), calendar.clone());
                    if state.calendar_cache.len() > 6 {
                        state.calendar_cache.clear();
                        state
                            .calendar_cache
                            .insert(calendar.month.clone(), calendar.clone());
                    }
                    state.calendar = Some(calendar);
                    state.clamp_calendar_week();
                }
                Err(error) => {
                    let now = clock;
                    state.set_status(error.message, now);
                }
            }
        }
        Response::CalendarYear { id, result } => {
            if state.calendar_year_request != Some(id) {
                return;
            }
            state.calendar_year_request = None;
            state.pending_views = state.pending_views.saturating_sub(1);
            match result {
                Ok(data) => {
                    state.calendar_year = data.year;
                    state.calendar_year_data = Some(data);
                }
                Err(error) => {
                    let now = clock;
                    state.set_status(error.message, now);
                }
            }
        }
        Response::Thread { id, result } => {
            if state.thread_request != Some(id) {
                return;
            }
            state.thread_request = None;
            state.pending_views = state.pending_views.saturating_sub(1);
            match result {
                Ok(posts) => {
                    state.thread = posts.iter().map(Post::from_bookmark).collect();
                    for post in &state.thread {
                        for media in &post.media {
                            state.images.request(&media.key, ImageKind::Image);
                        }
                    }
                }
                Err(error) => {
                    let now = clock;
                    state.set_status(error.message, now);
                }
            }
        }
        Response::RestoreDeleted { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            let now = clock;
            match result {
                Ok(Some(bookmark)) => {
                    state.set_status(
                        format!("restored deleted post by @{}", bookmark.author_handle),
                        now,
                    );
                    state.refresh_feed();
                }
                Ok(None) => state.set_status("nothing to restore", now),
                Err(error) => state.set_status(error.message, now),
            }
        }
        Response::TrashCount { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            let now = clock;
            match result {
                Ok(0) => state.set_status("trash is empty", now),
                Ok(count) => state.set_status(format!("{count} deleted posts recoverable"), now),
                Err(error) => state.set_status(error.message, now),
            }
        }
        Response::BackupNow { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            let now = clock;
            match result {
                Ok(backup) => {
                    state.set_status(format!("backup written ({} KB)", backup.bytes / 1024), now)
                }
                Err(error) => state.set_status(error.message, now),
            }
        }
        Response::BackupList { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            let now = clock;
            if let Ok(backups) = &result {
                state.backup_count = Some(backups.len());
            }
            match result {
                Ok(backups) if backups.is_empty() => state.set_status("no backups yet", now),
                Ok(backups) => state.set_status(
                    format!(
                        "{} backups, newest {}",
                        backups.len(),
                        backups
                            .first()
                            .and_then(|backup| std::path::Path::new(&backup.path)
                                .file_name()
                                .and_then(|name| name.to_str())
                                .map(str::to_string))
                            .unwrap_or_default()
                    ),
                    now,
                ),
                Err(error) => state.set_status(error.message, now),
            }
        }
        Response::IntegrityCheck { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            let now = clock;
            match result {
                Ok(report) => state.set_status(report.summary(), now),
                Err(error) => state.set_status(error.message, now),
            }
        }
        Response::RestoreBackup { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            let now = clock;
            match result {
                Ok(summary) => {
                    state.set_status(
                        format!("restored {} bookmarks from backup", summary.bookmarks),
                        now,
                    );
                    // Everything the shell cached belongs to the old archive.
                    state.calendar_cache.clear();
                    state.calendar = None;
                    state.calendar_request = None;
                    state.calendar_year_data = None;
                    state.calendar_year_request = None;
                    state.latest_month = None;
                    state.history = Vec::new();
                    state.history_total = 0;
                    state.selected = 0;
                    state.request_feed(None);
                }
                Err(error) => state.set_status(error.message, now),
            }
        }
        Response::PreviewFiles { result, .. } => {
            state.import_pending = false;
            state.import = match result {
                Ok(preview) => ImportState::Preview {
                    paths: state.import_paths.clone(),
                    preview,
                },
                Err(error) => ImportState::Error {
                    message: error.message,
                    paths: state.import_paths.clone(),
                },
            };
        }
        Response::ImportFiles { result, .. } => {
            state.import_pending = false;
            state.import_progress = None;
            state.import_progress_rx = None;
            match result {
                Ok(summary) => {
                    state.import = ImportState::Done { summary };
                    state.refresh_feed();
                }
                Err(error) => {
                    state.import = ImportState::Error {
                        message: error.message,
                        paths: state.import_paths.clone(),
                    }
                }
            }
        }
        Response::Stats { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            if let Ok(stats) = result {
                state.stats = Some(stats);
            }
        }
        Response::HistoryList { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            match result {
                Ok(page) => {
                    state.history = page.items;
                    state.history_total = page.total;
                    if state.history_selected.is_none() {
                        if let Some(first) =
                            state.history.iter().find(|event| event.undone_at.is_none())
                        {
                            let id = first.id;
                            state.select_history(id);
                        }
                    }
                }
                Err(error) => {
                    let now = clock;
                    state.set_status(error.message, now);
                }
            }
        }
        Response::HistoryDetail { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            match *result {
                Ok(detail) => state.history_detail = detail,
                Err(error) => {
                    let now = clock;
                    state.set_status(error.message, now);
                }
            }
        }
        Response::UndoLatest { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            match result {
                Ok(undo) => {
                    let now = clock;
                    let message = if let Some(kind) = undo.kind {
                        let label = match kind {
                            HistoryKind::Import => "import",
                            HistoryKind::Delete => "delete",
                            HistoryKind::Tag => "tags",
                        };
                        format!("{label} undone · {} changed", undo.affected)
                    } else {
                        "nothing to undo".to_string()
                    };
                    state.set_status(message, now);
                    state.history_detail = None;
                    state.history_selected = None;
                    state.request_history();
                    state.refresh_feed();
                    state.authors.clear();
                    state.topics.clear();
                    state.graph = None;
                }
                Err(error) => {
                    let now = clock;
                    state.set_status(error.message, now);
                }
            }
        }
        Response::Export { result, .. } => match result {
            Ok(summary) => {
                let now = clock;
                state.set_status(
                    format!(
                        "exported {} posts · {}",
                        summary.count,
                        summary
                            .path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("file")
                    ),
                    now,
                );
            }
            Err(error) => {
                let now = clock;
                state.set_status(error.message, now);
            }
        },
        Response::BulkTag { result, .. } => match result {
            Ok(bulk) => {
                let now = clock;
                state.set_status(format!("{} posts tagged", bulk.affected), now);
                clear_selection(state);
                state.refresh_feed();
            }
            Err(error) => {
                let now = clock;
                state.set_status(error.message, now);
            }
        },
        Response::BulkDelete { result, .. } => match result {
            Ok(bulk) => {
                let now = clock;
                state.set_status(
                    format!(
                        "{} posts deleted, restore last deleted to recover",
                        bulk.affected
                    ),
                    now,
                );
                clear_selection(state);
                state.route = Route::Feed;
                state.refresh_feed();
            }
            Err(error) => {
                let now = clock;
                state.set_status(error.message, now);
            }
        },
        Response::BulkSetRead { result, .. } => match result {
            Ok(bulk) => {
                let now = clock;
                state.set_status(format!("{} posts updated", bulk.affected), now);
                clear_selection(state);
                state.refresh_feed();
            }
            Err(error) => {
                let now = clock;
                state.set_status(error.message, now);
            }
        },
        Response::DeleteBookmark { result, .. } => {
            let target = state.delete_request.take().map(|(_, id)| id);
            match result {
                Ok(true) => {
                    if let Some(id) = target {
                        state.posts.retain(|post| post.id != id);
                        state.total = state.total.saturating_sub(1);
                        state.selected = state.selected.min(state.posts.len().saturating_sub(1));
                    }
                    state.route = Route::Feed;
                    let now = clock;
                    state.set_status("post deleted, restore last deleted to recover", now);
                    state.refresh_feed();
                }
                Ok(false) => {}
                Err(error) => {
                    let now = clock;
                    state.set_status(error.message, now);
                }
            }
        }
        Response::SetRead { result, .. } => {
            if let Err(error) = result {
                let now = clock;
                state.set_status(error.message, now);
            }
        }
        Response::TagSuggestions { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            if let Ok(suggestions) = result {
                state.tag_suggestions = suggestions;
            }
        }
        Response::RenameTag { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            let now = clock;
            match result {
                Ok(report) => {
                    let verb = if report.merged { "merged" } else { "renamed" };
                    state.set_status(
                        format!(
                            "{verb} tag · {} post{}",
                            report.affected,
                            plural(report.affected)
                        ),
                        now,
                    );
                    state.refresh_feed();
                }
                Err(error) => state.set_status(error.message, now),
            }
        }
        Response::PruneTags { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            let now = clock;
            match result {
                Ok(0) => state.set_status("no unused tags", now),
                Ok(count) => state.set_status(
                    format!("removed {} unused tag{}", count, plural(count)),
                    now,
                ),
                Err(error) => state.set_status(error.message, now),
            }
        }
        Response::ResetTagColor { result, .. } => {
            state.pending_views = state.pending_views.saturating_sub(1);
            let now = clock;
            match result {
                Ok(true) => state.set_status("colour reset", now),
                Ok(false) => {}
                Err(error) => state.set_status(error.message, now),
            }
        }
        Response::SetNote { result, .. } => {
            if let Err(error) = result {
                let now = clock;
                state.set_status(error.message, now);
            }
        }
        Response::SetTags { result, .. } => {
            if let Err(error) = result {
                let now = clock;
                state.set_status(error.message, now);
            }
        }
        Response::ImportPreview { .. } | Response::Import { .. } => {}
        Response::Detail { .. } => {}
    }
}

fn on_key(state: &mut State, event: &KeyEvent, tx: &mut Tx) -> bool {
    if state.overlay != Overlay::None {
        return overlay_key(state, event, tx);
    }
    if state.mode == Mode::Search {
        return search_key(state, event, tx);
    }
    if state.directory_editing && matches!(state.route, Route::Authors | Route::Topics) {
        return directory_key(state, event);
    }
    if state.route == Route::Import {
        match &event.key {
            Key::Escape => {
                if matches!(state.import, ImportState::Importing { .. }) {
                    if let Some(store) = state.store.as_ref() {
                        store.cancel_import();
                    }
                    let now = tx.clock();
                    state.set_status("cancelling import", now);
                } else {
                    state.open_route(Route::Feed);
                }
                return true;
            }
            Key::Enter => {
                match &state.import {
                    ImportState::Idle => state.choose_import_files(),
                    ImportState::Preview { .. } => state.start_import(),
                    ImportState::Done { .. } => {
                        state.import = ImportState::Idle;
                        state.choose_import_files();
                    }
                    _ => {}
                }
                return true;
            }
            _ => {}
        }
    }
    if state.route == Route::History {
        match &event.key {
            Key::Escape => {
                state.open_route(Route::Feed);
                return true;
            }
            Key::Text(t) if t.as_str() == "j" || matches!(event.key, Key::Down) => {
                let len = state.history.len();
                if len > 0 {
                    let index = state
                        .history
                        .iter()
                        .position(|event| Some(event.id) == state.history_selected)
                        .unwrap_or(0);
                    state.select_history(state.history[(index + 1).min(len - 1)].id);
                }
                return true;
            }
            Key::Text(t) if t.as_str() == "k" || matches!(event.key, Key::Up) => {
                let len = state.history.len();
                if len > 0 {
                    let index = state
                        .history
                        .iter()
                        .position(|event| Some(event.id) == state.history_selected)
                        .unwrap_or(0);
                    state.select_history(state.history[index.saturating_sub(1)].id);
                }
                return true;
            }
            Key::Text(t) if t.as_str() == "u" => {
                state.undo_latest();
                return true;
            }
            _ => {}
        }
    }
    if let Some(handled) = graph_key(state, event) {
        return handled;
    }
    if let Some(handled) = calendar_key(state, event) {
        return handled;
    }
    match &event.key {
        Key::Command('n') => {
            next_unread(state);
            true
        }
        Key::Command('f') => {
            enter_search(state);
            true
        }
        Key::Command('p') => {
            state.overlay = Overlay::Palette;
            state.palette.clear();
            state.palette_index = 0;
            true
        }
        Key::Command(',') => {
            state.open_route(Route::Settings);
            true
        }
        Key::Command('a') => {
            if event.mods.shift {
                select_all_matching(state);
            } else {
                select_page(state);
            }
            true
        }
        Key::Text(t) if t.as_str() == "x" => {
            toggle_selection_current(state);
            true
        }
        Key::Text(t) if t.as_str() == "u" && state.selection.mode => {
            bulk_read(state, false);
            true
        }
        Key::Text(t) if t.as_str() == "r" && state.selection.mode => {
            bulk_read(state, true);
            true
        }
        Key::Text(t) if (t.as_str() == "u" || t.as_str() == "r") => {
            let read = t.as_str() == "r";
            if let Some(post) = state.posts.get_mut(state.selected) {
                post.is_read = read;
                let id = post.id.clone();
                if let Some(store) = state.store.as_ref() {
                    store.set_read(id, read);
                }
            }
            true
        }
        Key::Text(t) if t.as_str() == "/" => {
            if matches!(state.route, Route::Authors | Route::Topics) {
                state.directory_editing = true;
            } else {
                enter_search(state);
            }
            true
        }
        Key::Text(t) if t.as_str() == ":" => {
            state.overlay = Overlay::Palette;
            state.palette.clear();
            state.palette_index = 0;
            true
        }
        Key::Text(t) if t.as_str() == "?" => {
            state.overlay = Overlay::Keys;
            true
        }
        Key::Text(t) if t.as_str() == "g" && !event.mods.shift => {
            let now = tx.clock();
            if state
                .pending_g
                .take()
                .is_some_and(|first| now - first <= 0.8)
            {
                if !state.posts.is_empty() {
                    state.selected = 0;
                }
                tx.scroll_to_y("feed", 0.0);
            } else {
                state.pending_g = Some(now);
            }
            true
        }
        Key::Text(t) if t.as_str() == "G" && event.mods.shift => {
            if !state.posts.is_empty() {
                state.selected = state.posts.len() - 1;
            }
            if state.feed_has_more {
                state.load_more_feed();
            }
            tx.scroll_to_bottom("feed");
            true
        }
        Key::Text(t) if t.as_str() == "n" => {
            open_note_editor(state);
            true
        }
        Key::Text(t) if t.as_str() == "h" => {
            state.open_route(Route::History);
            true
        }
        Key::Text(t) if t.as_str() == "e" => {
            state.open_export();
            true
        }
        Key::Text(t) if t.as_str() == "j" => {
            if event.mods.shift {
                extend_selection(state, 1);
            } else if matches!(state.route, Route::Authors | Route::Topics) {
                move_directory(state, 1);
            } else {
                move_selection(state, 1);
            }
            reveal(state, tx);
            true
        }
        Key::Text(t) if t.as_str() == "k" => {
            if event.mods.shift {
                extend_selection(state, -1);
            } else if matches!(state.route, Route::Authors | Route::Topics) {
                move_directory(state, -1);
            } else {
                move_selection(state, -1);
            }
            reveal(state, tx);
            true
        }
        Key::Down => {
            if matches!(state.route, Route::Authors | Route::Topics) {
                move_directory(state, 1);
            } else {
                move_selection(state, 1);
            }
            reveal(state, tx);
            true
        }
        Key::Up => {
            if matches!(state.route, Route::Authors | Route::Topics) {
                move_directory(state, -1);
            } else {
                move_selection(state, -1);
            }
            reveal(state, tx);
            true
        }
        Key::Enter => {
            if matches!(state.route, Route::Authors | Route::Topics) {
                open_directory_selection(state);
            } else if !state.posts.is_empty() {
                begin_detail(state, state.selected);
            }
            true
        }
        Key::Left => {
            if !matches!(state.route, Route::Authors | Route::Topics) {
                turn_carousel(state, state.selected, -1);
            }
            true
        }
        Key::Right => {
            if !matches!(state.route, Route::Authors | Route::Topics) {
                turn_carousel(state, state.selected, 1);
            }
            true
        }
        Key::Escape | Key::Backspace => {
            if state.selection.mode {
                clear_selection(state);
            } else if state.route == Route::Detail {
                stop_video(state);
                state.thread.clear();
                state.thread_anchor = None;
                state.thread_request = None;
                state.route = Route::Feed;
                if state.back_stack.last() == Some(&Route::Feed) {
                    state.back_stack.pop();
                }
                state.feed_restore = state.feed_return.take();
                restore_feed_position(state, tx);
            } else if state.route != Route::Feed {
                state.open_route(Route::Feed);
            } else if state.filter.is_some() {
                state.request_feed(None);
            } else {
                state.status = None;
            }
            true
        }
        Key::Text(t) if t.as_str() == "," => {
            if state.video.is_some() {
                seek_video(state, tx, -5.0);
            }
            true
        }
        Key::Text(t) if t.as_str() == "." => {
            if state.video.is_some() {
                seek_video(state, tx, 5.0);
            }
            true
        }
        Key::Text(t) if t.as_str() == " " => {
            toggle_video(state, tx);
            true
        }
        Key::Text(t) if t.as_str() == "f" => {
            toggle_favorite(state);
            true
        }
        Key::Text(t) if t.as_str() == "m" => {
            if state.video.is_some() {
                toggle_mute(state, tx);
            } else {
                load_media(state, state.selected);
            }
            true
        }
        Key::Text(t) if t.as_str() == "t" => {
            if state.selection.mode {
                begin_bulk_tag(state);
                state.pending_focus = Some("bulk-tag-input".into());
                let _ = tx;
            } else if state.route == Route::Detail {
                begin_tag_edit(state, tx);
            }
            true
        }
        Key::Text(t) if t.as_str() == "d" => {
            if state.selection.mode {
                begin_bulk_delete(state);
            } else if state.route == Route::Detail {
                begin_delete(state);
            }
            true
        }
        Key::Text(t) if t.as_str() == "," => {
            state.open_route(Route::Settings);
            true
        }
        Key::Text(t) if t.as_str() == "o" => {
            if let Some(post) = state.posts.get(state.selected) {
                if eterea_logic::is_safe_https_url(&post.tweet_url) {
                    let _ = tx.open_url(&post.tweet_url);
                }
            }
            true
        }
        Key::Text(t) if t.as_str() == "c" => {
            if let Some(post) = state.posts.get(state.selected) {
                tx.copy(post.tweet_url.clone());
                let now = tx.clock();
                state.set_status("link copied", now);
            }
            true
        }
        _ => false,
    }
}

fn overlay_key(state: &mut State, event: &KeyEvent, tx: &mut Tx) -> bool {
    match state.overlay {
        Overlay::Palette => match &event.key {
            Key::Escape => state.close_overlay(),
            Key::Up => {
                state.palette_index = state.palette_index.saturating_sub(1);
            }
            Key::Text(t) if t.as_str() == "k" => {
                state.palette_index = state.palette_index.saturating_sub(1);
            }
            Key::Down => {
                let len = state.filtered_commands().len();
                if len > 0 {
                    state.palette_index = (state.palette_index + 1).min(len - 1);
                }
            }
            Key::Text(t) if t.as_str() == "j" => {
                let len = state.filtered_commands().len();
                if len > 0 {
                    state.palette_index = (state.palette_index + 1).min(len - 1);
                }
            }
            Key::Enter => {
                let commands = state.filtered_commands();
                if let Some((_, item)) = commands.get(state.palette_index).copied() {
                    state.close_overlay();
                    state.run_command(item, tx);
                }
            }
            Key::Backspace => {
                state.palette.pop();
                state.palette_index = 0;
            }
            Key::Text(t) => {
                state.palette.push_str(t);
                state.palette_index = 0;
            }
            _ => {}
        },
        Overlay::Keys => match &event.key {
            Key::Escape => dismiss_overlay(state),
            Key::Text(t) if t.as_str() == "?" => dismiss_overlay(state),
            _ => {}
        },
        Overlay::Lightbox => match &event.key {
            Key::Escape => dismiss_overlay(state),
            Key::Text(t) if t.as_str() == "+" || t.as_str() == "=" => {
                state.lightbox_zoom = (state.lightbox_zoom * 1.15).clamp(0.5, 3.0);
            }
            Key::Text(t) if t.as_str() == "-" => {
                state.lightbox_zoom = (state.lightbox_zoom / 1.15).clamp(0.5, 3.0);
            }
            Key::Text(t) if t.as_str() == "0" => {
                state.lightbox_zoom = 1.0;
            }
            Key::Text(t) if t.as_str() == " " || t.as_str() == "Enter" => {
                if let Some((post, index)) = state.lightbox {
                    start_video_at(state, tx, post, index);
                }
            }
            Key::Left => {
                if let Some((post, index)) = state.lightbox {
                    if index > 0 {
                        state.lightbox = Some((post, index - 1));
                        state.lightbox_zoom = 1.0;
                    }
                }
            }
            Key::Right => {
                if let Some((post, index)) = state.lightbox {
                    if let Some(count) = state.posts.get(post).map(|post| post.media.len()) {
                        if index + 1 < count {
                            state.lightbox = Some((post, index + 1));
                            state.lightbox_zoom = 1.0;
                        }
                    }
                }
            }
            _ => {}
        },
        Overlay::Tags => match &event.key {
            Key::Escape => dismiss_overlay(state),
            Key::Enter => add_tag(state),
            _ => {}
        },
        Overlay::GraphColor => {
            if event.key == Key::Escape {
                dismiss_overlay(state);
            }
        }
        Overlay::Note => match &event.key {
            Key::Escape => dismiss_overlay(state),
            Key::Enter if event.mods.primary() => save_note(state),
            _ => {}
        },
        Overlay::TagRename => match &event.key {
            Key::Escape => dismiss_overlay(state),
            Key::Enter => save_tag_rename(state),
            _ => {}
        },
        Overlay::GraphSearch => match &event.key {
            Key::Escape => {
                state.overlay = Overlay::None;
                state.graph_filter = None;
                state.graph_search_buffer = TextEdit::new("");
            }
            Key::Enter => {
                let text = state.graph_search_buffer.text.trim().to_string();
                state.graph_filter = if text.is_empty() { None } else { Some(text) };
                state.overlay = Overlay::None;
            }
            _ => {}
        },
        Overlay::BulkTag => match &event.key {
            Key::Escape => dismiss_overlay(state),
            Key::Enter => apply_bulk_tag(state),
            _ => {}
        },
        Overlay::Export => match &event.key {
            Key::Escape => dismiss_overlay(state),
            Key::Text(t) if t.as_str() == "a" => {
                if let Some(sheet) = state.export_sheet.as_mut() {
                    sheet.scope = ExportScopeChoice::All;
                }
            }
            Key::Text(t) if t.as_str() == "f" => {
                if let Some(sheet) = state.export_sheet.as_mut() {
                    sheet.scope = ExportScopeChoice::Filter;
                }
            }
            Key::Text(t) if t.as_str() == "s" => {
                if let Some(sheet) = state.export_sheet.as_mut() {
                    sheet.scope = ExportScopeChoice::Selection;
                }
            }
            Key::Text(t) if t.as_str() == "c" => {
                if let Some(sheet) = state.export_sheet.as_mut() {
                    sheet.format = ExportKind::Csv;
                }
            }
            Key::Text(t) if t.as_str() == "j" => {
                if let Some(sheet) = state.export_sheet.as_mut() {
                    sheet.format = ExportKind::Json;
                }
            }
            Key::Enter => state.export_now(tx),
            _ => {}
        },
        Overlay::ConfirmDelete => match &event.key {
            Key::Escape => dismiss_overlay(state),
            Key::Enter => confirm_delete(state),
            _ => {}
        },
        Overlay::None => return false,
    }
    true
}

fn search_key(state: &mut State, event: &KeyEvent, _tx: &mut Tx) -> bool {
    match &event.key {
        Key::Escape => {
            state.mode = Mode::Normal;
            state.search.clear();
            if matches!(state.filter, Some(Filter::Query(_))) {
                state.request_feed(None);
            }
        }
        Key::Backspace => {
            state.search.pop();
            if state.search.is_empty() {
                state.request_feed(None);
            } else {
                state.request_feed(Some(Filter::Query(state.search.clone())));
            }
        }
        Key::Enter => {
            state.mode = Mode::Normal;
            if !state.posts.is_empty() {
                stop_video(state);
                begin_detail(state, state.selected);
            }
        }
        Key::Text(t) => {
            state.search.push_str(t);
            state.request_feed(Some(Filter::Query(state.search.clone())));
        }
        _ => return false,
    }
    true
}

/// Open the header search field over the feed.
fn enter_search(state: &mut State) {
    stop_video(state);
    state.note_navigation(Route::Feed);
    state.route = Route::Feed;
    state.mode = Mode::Search;
    state.search.clear();
}

/// The mouse back button (and the key sheet hint describe the same model):
/// close the top overlay, leave a text mode, then walk back one route.
fn navigate_back(state: &mut State, tx: &mut Tx) {
    if state.overlay != Overlay::None {
        dismiss_overlay(state);
        return;
    }
    if state.mode != Mode::Normal {
        state.mode = Mode::Normal;
        state.search.clear();
        state.directory_editing = false;
        if matches!(state.filter, Some(Filter::Query(_))) {
            state.request_feed(None);
        }
        return;
    }
    if state.directory_editing {
        state.directory_editing = false;
        if !state.directory_filter.is_empty() {
            state.directory_filter.clear();
            reload_directory(state);
        }
        return;
    }
    if state.selection.mode {
        clear_selection(state);
        return;
    }
    let Some(previous) = state.back_stack.pop() else {
        return;
    };
    state.forward_stack.push(state.route);
    if previous == Route::Feed && state.route == Route::Detail {
        // Returning to the feed that is still in memory: keep the page and
        // restore the reader's spot instead of re-querying from the top.
        state.thread.clear();
        state.thread_anchor = None;
        state.thread_request = None;
        state.feed_restore = state.feed_return.take();
        state.route = Route::Feed;
        restore_feed_position(state, tx);
        return;
    }
    let filter = state.filter.clone();
    state.apply_route(previous);
    restore_feed_filter(state, filter);
}

/// The mouse forward button: return to the route back left.
fn navigate_forward(state: &mut State, _tx: &mut Tx) {
    if state.overlay != Overlay::None || state.mode != Mode::Normal {
        return;
    }
    let Some(next) = state.forward_stack.pop() else {
        return;
    };
    state.back_stack.push(state.route);
    let filter = state.filter.clone();
    state.apply_route(next);
    restore_feed_filter(state, filter);
}

/// `apply_route(Feed)` clears the filter; walking between routes must not.
fn restore_feed_filter(state: &mut State, filter: Option<Filter>) {
    if state.route == Route::Feed && filter.is_some() {
        state.request_feed(filter);
    }
}

/// Keys while the authors/topics filter field owns the keyboard.
fn directory_key(state: &mut State, event: &KeyEvent) -> bool {
    match &event.key {
        Key::Escape => {
            if state.directory_filter.is_empty() {
                state.directory_editing = false;
            } else {
                state.directory_filter.clear();
                reload_directory(state);
            }
        }
        Key::Backspace => {
            state.directory_filter.pop();
            reload_directory(state);
        }
        Key::Enter => {
            state.directory_editing = false;
            open_directory_selection(state);
        }
        Key::Text(text) => {
            state.directory_filter.push_str(text);
            reload_directory(state);
        }
        _ => return false,
    }
    true
}

fn reload_directory(state: &mut State) {
    match state.route {
        Route::Authors => state.request_authors(0),
        Route::Topics => state.request_topics(0),
        _ => {}
    }
}

/// Start the opt-in repair pass on a worker thread. The worker owns its own
/// database connection (SQLite WAL), so navigation, search and scrolling stay
/// responsive while it runs; it reports progress over a channel.
fn start_media_repair(state: &mut State, now: f64) {
    if state.repair_active {
        return;
    }
    if state.demo {
        state.set_status("repair needs the real archive", now);
        return;
    }
    let Some(path) = state.db_path.clone() else {
        state.set_status("archive path unknown", now);
        return;
    };
    // Test/headless switches mirror the other ETEREA_* overrides; normal runs
    // repair the whole missing set at a polite pace.
    let limit = std::env::var("ETEREA_REPAIR_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(usize::MAX);
    let delay_ms = std::env::var("ETEREA_REPAIR_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(320);
    let options = RepairOptions {
        limit,
        delay: Duration::from_millis(delay_ms.max(1)),
    };
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    state.repair_rx = Some(rx);
    state.repair_cancel = Some(cancel);
    state.repair_active = true;
    state.repair_progress = Some((0, 0));
    state.set_status("repairing media", now);
    let spawned = std::thread::Builder::new()
        .name("eterea-repair".into())
        .spawn(move || {
            let result = eterea_app::repair_missing_media(&path, options, worker_cancel, |event| {
                let _ = tx.send(event);
            });
            if result.is_err() {
                // A finished event with a failure tells the shell to report it.
                let _ = tx.send(RepairEvent {
                    total: 0,
                    failed: 1,
                    finished: true,
                    ..RepairEvent::default()
                });
            }
        });
    if spawned.is_err() {
        state.repair_active = false;
        state.repair_rx = None;
        state.repair_cancel = None;
        state.repair_progress = None;
        state.set_status("repair could not start", now);
    }
}

/// Ask the repair worker to stop after the current post. The footer bar and
/// the palette's `cancel repair` command share this; `esc` stays navigation.
fn request_cancel_repair(state: &mut State) {
    if let Some(cancel) = state.repair_cancel.as_ref() {
        cancel.store(true, Ordering::Relaxed);
    }
}

fn poll_media_repair(state: &mut State, clock: f64) -> bool {
    let Some(rx) = state.repair_rx.take() else {
        return false;
    };
    let mut events = Vec::new();
    let mut disconnected = false;
    loop {
        match rx.try_recv() {
            Ok(event) => events.push(event),
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                disconnected = true;
                break;
            }
        }
    }
    // The worker always sends a final event; a disconnect without one means it
    // died (panic or abrupt stop). Never leave the progress bar stuck.
    let finished = events.last().is_some_and(|event| event.finished) || disconnected;
    if disconnected && !events.last().is_some_and(|event| event.finished) {
        events.push(RepairEvent {
            failed: 1,
            finished: true,
            ..RepairEvent::default()
        });
    }
    if !finished {
        state.repair_rx = Some(rx);
    }
    let mut changed = false;
    for event in events {
        if event.finished {
            state.repair_active = false;
            state.repair_rx = None;
            state.repair_cancel = None;
            state.repair_progress = None;
            let message = if event.cancelled {
                "repair cancelled".to_string()
            } else if event.failed > 0 && event.repaired == 0 {
                "repair failed".to_string()
            } else if event.unavailable > 0 || event.failed > 0 {
                format!(
                    "repaired {} posts, {} unavailable, {} failed",
                    event.repaired, event.unavailable, event.failed
                )
            } else {
                format!("repaired {} posts", event.repaired)
            };
            state.set_status(message, clock);
            state.refresh_feed();
        } else {
            state.repair_progress = Some((event.done, event.total));
        }
        changed = true;
    }
    changed
}

fn stop_video(state: &mut State) {
    if let Some(mut player) = state.video.take() {
        player.stop();
    }
    state.video_target = None;
    state.video_image = None;
    state.video_pending = None;
}

fn media_cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ETEREA_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(dir).join("eterea");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache/eterea")
}

fn video_cache(state: &mut State) -> Option<Arc<VideoCache>> {
    if state.video_cache.is_none() {
        let config = VideoCacheConfig::new(media_cache_dir());
        state.video_cache = VideoCache::open(config).ok().map(Arc::new);
    }
    state.video_cache.clone()
}

fn start_video_at(state: &mut State, tx: &mut Tx, post_index: usize, media_index: usize) {
    start_video_from(state, tx, post_index, media_index, None);
}

fn start_video_from(
    state: &mut State,
    tx: &mut Tx,
    post_index: usize,
    media_index: usize,
    start_at: Option<std::time::Duration>,
) {
    if state.video_target == Some((post_index, media_index)) {
        if let Some(player) = state.video.as_mut() {
            player.toggle_pause();
        }
        return;
    }
    stop_video(state);
    let Some(post) = state.posts.get(post_index) else {
        return;
    };
    let Some(item) = post.media.get(media_index) else {
        return;
    };
    let Some(url) = item.video_url.clone() else {
        return;
    };
    let cache = video_cache(state);
    let muted = state.route == Route::Feed && state.overlay != Overlay::Lightbox;
    let options = VideoPlayerOptions {
        cache,
        muted,
        looping: true,
        start_at,
        ..VideoPlayerOptions::default()
    };
    match VideoPlayer::start_with_options(url, options) {
        Ok(mut player) => {
            let audio_error = player.state().audio_error;
            state.video = Some(player);
            state.video_target = Some((post_index, media_index));
            state.video_image = None;
            let now = tx.clock();
            if let Some(error) = audio_error {
                state.set_status(error, now);
            } else {
                state.set_status("loading video", now);
            }
        }
        Err(error) => {
            let now = tx.clock();
            state.set_status(error.to_string(), now);
        }
    }
}

fn toggle_video(state: &mut State, tx: &mut Tx) {
    if let Some((post, media)) = state.video_target {
        start_video_at(state, tx, post, media);
        return;
    }
    let Some(post) = state.posts.get(state.selected) else {
        return;
    };
    let page_start = carousel_page(post) * 4;
    let page_end = (page_start + 4).min(post.media.len());
    let found = (page_start..page_end)
        .find(|index| post.media[*index].video_url.is_some())
        .or_else(|| post.media.iter().position(|item| item.video_url.is_some()));
    if let Some(index) = found {
        start_video_at(state, tx, state.selected, index);
    }
}

/// Scrub the current video by restarting the decoder at the new offset (input
/// seek). The timeline previews the target smoothly and seeks once on release.
fn seek_video(state: &mut State, tx: &mut Tx, delta_seconds: f64) {
    let Some(player) = state.video.as_ref() else {
        return;
    };
    let target = player.position().as_secs_f64() + delta_seconds;
    seek_video_to(state, tx, target);
}

fn seek_video_to(state: &mut State, tx: &mut Tx, target_seconds: f64) {
    let Some((post_index, media_index)) = state.video_target else {
        return;
    };
    let Some(player) = state.video.as_ref() else {
        return;
    };
    let mut target = target_seconds.max(0.0);
    if let Some(duration) = player.duration() {
        target = target.min((duration.as_secs_f64() - 0.5).max(0.0));
    }
    state.video_scrub = None;
    stop_video(state);
    start_video_from(
        state,
        tx,
        post_index,
        media_index,
        Some(std::time::Duration::from_secs_f64(target)),
    );
}

fn fmt_playback_time(value: std::time::Duration) -> String {
    let seconds = value.as_secs();
    let minutes = seconds / 60;
    let remainder = seconds % 60;
    if minutes >= 60 {
        format!("{}:{:02}:{:02}", minutes / 60, minutes % 60, remainder)
    } else {
        format!("{minutes}:{remainder:02}")
    }
}

fn toggle_mute(state: &mut State, tx: &mut Tx) {
    let Some(player) = state.video.as_mut() else {
        return;
    };
    let muted = !player.is_muted();
    if let Err(error) = player.set_muted(muted) {
        let now = tx.clock();
        state.set_status(error.to_string(), now);
        return;
    }
    let now = tx.clock();
    state.set_status(if muted { "sound off" } else { "sound on" }, now);
}

fn move_selection(state: &mut State, delta: isize) {
    if state.posts.is_empty() {
        return;
    }
    stop_video(state);
    let max = state.posts.len() as isize - 1;
    state.selected = (state.selected as isize + delta).clamp(0, max) as usize;
}

fn move_directory(state: &mut State, delta: isize) {
    let len = match state.route {
        Route::Authors => state.authors.len(),
        Route::Topics => state.topics.len(),
        _ => return,
    };
    if len == 0 {
        return;
    }
    state.directory_index =
        (state.directory_index as isize + delta).clamp(0, len as isize - 1) as usize;
}

fn open_directory_selection(state: &mut State) {
    match state.route {
        Route::Authors => {
            if let Some(author) = state.authors.get(state.directory_index) {
                let handle = author.handle.clone();
                state.filter_author(&handle);
            }
        }
        Route::Topics => {
            if let Some(topic) = state.topics.get(state.directory_index) {
                let tag = topic.tag.clone();
                state.filter_topic(&tag);
            }
        }
        _ => {}
    }
}

fn toggle_selection_current(state: &mut State) {
    let Some(post) = state.posts.get(state.selected) else {
        return;
    };
    let id = post.id.clone();
    state.selection.mode = true;
    state.selection.anchor = Some(state.selected);
    state.selection.predicate = None;
    state.selection.affected = None;
    if !state.selection.ids.remove(&id) {
        state.selection.ids.insert(id);
    }
    if state.selection.ids.is_empty() {
        state.selection.mode = false;
    }
}

fn extend_selection(state: &mut State, delta: isize) {
    if state.posts.is_empty() {
        return;
    }
    state.selection.mode = true;
    state.selection.predicate = None;
    state.selection.affected = None;
    let previous = state.selected;
    let next = (previous as isize + delta).clamp(0, state.posts.len() as isize - 1) as usize;
    state.selected = next;
    let from = state.selection.anchor.unwrap_or(previous).min(next);
    let to = state.selection.anchor.unwrap_or(previous).max(next);
    for post in &state.posts[from..=to] {
        state.selection.ids.insert(post.id.clone());
    }
}

fn select_page(state: &mut State) {
    state.selection.mode = true;
    state.selection.predicate = None;
    state.selection.affected = None;
    for post in &state.posts {
        state.selection.ids.insert(post.id.clone());
    }
}

fn select_all_matching(state: &mut State) {
    state.selection.mode = true;
    state.selection.ids.clear();
    state.selection.predicate = Some(state.current_query());
    state.selection.affected = Some(state.total);
}

fn clear_selection(state: &mut State) {
    state.selection = SelectionState::default();
}

fn current_selection(state: &State) -> BookmarkSelection {
    if let Some(query) = state.selection.predicate.clone() {
        BookmarkSelection::Query(query)
    } else {
        BookmarkSelection::Ids(state.selection.ids.iter().cloned().collect())
    }
}

fn begin_bulk_tag(state: &mut State) {
    if state.selection.ids.is_empty() && state.selection.predicate.is_none() {
        return;
    }
    state.bulk_tag_remove = false;
    state.tag_buffer = TextEdit::new("");
    state.overlay = Overlay::BulkTag;
}

fn apply_bulk_tag(state: &mut State) {
    let tag = state
        .tag_buffer
        .text
        .trim()
        .trim_start_matches('#')
        .to_string();
    if tag.is_empty() {
        return;
    }
    let selection = current_selection(state);
    let mutation = if state.bulk_tag_remove {
        TagMutation::Remove(vec![tag])
    } else {
        TagMutation::Add(vec![tag])
    };
    if let Some(store) = state.store.as_ref() {
        store.bulk_tag(selection, mutation);
    }
    state.overlay = Overlay::None;
}

fn begin_bulk_delete(state: &mut State) {
    if state.selection.ids.is_empty() && state.selection.predicate.is_none() {
        return;
    }
    state.bulk_delete = true;
    state.delete_target = None;
    state.overlay = Overlay::ConfirmDelete;
}

fn bulk_delete(state: &mut State) {
    let selection = current_selection(state);
    if let Some(store) = state.store.as_ref() {
        store.bulk_delete(selection);
    }
    state.bulk_delete = false;
    state.overlay = Overlay::None;
}

fn bulk_read(state: &mut State, read: bool) {
    if state.selection.ids.is_empty() && state.selection.predicate.is_none() {
        return;
    }
    let selection = current_selection(state);
    if let Some(store) = state.store.as_ref() {
        store.bulk_set_read(selection, read);
    }
}

/// Open a post and remember where the feed was so back can put it there.
fn begin_detail(state: &mut State, index: usize) {
    if state.posts.get(index).is_none() {
        return;
    }
    let same_row = state.selected == index;
    let viewport_top = state.feed_viewport_top.get();
    let viewport_height = state.feed_viewport_height.get();
    let anchor = state.feed_anchor_top.get();
    let relative = anchor - viewport_top;
    let top =
        (same_row && relative >= 0.0 && relative <= viewport_height - 8.0).then_some(relative);
    state.feed_return = Some(FeedReturn {
        index,
        top,
        scroll: state.feed_scroll_offset.get(),
    });
    state.selected = index;
    open_detail(state);
}

/// Put the reader back where they left the feed. Uses the opened post as a
/// layout anchor when it was on screen, otherwise the raw viewport offset.
fn restore_feed_position(state: &mut State, tx: &mut Tx) {
    let Some(returning) = state.feed_restore.take() else {
        return;
    };
    if returning.index < state.posts.len() {
        state.selected = returning.index;
    }
    if let Some(top) = returning.top {
        tx.scroll_key_to(format!("post-{}", returning.index), top);
    } else {
        tx.scroll_to_y("feed", returning.scroll);
    }
}

fn open_tag_rename(state: &mut State) {
    state.overlay = Overlay::TagRename;
    state.tag_rename_from = None;
    state.tag_rename_buffer = TextEdit::new("");
    state.tag_suggestions.clear();
    if let Some(store) = state.store.as_ref() {
        store.tag_suggestions("", 50);
        state.pending_views += 1;
    }
}

fn save_tag_rename(state: &mut State) {
    let Some(from) = state.tag_rename_from.clone() else {
        return;
    };
    let to = state
        .tag_rename_buffer
        .text
        .trim()
        .trim_start_matches('#')
        .to_string();
    if to.is_empty() || to.eq_ignore_ascii_case(&from) {
        state.overlay = Overlay::None;
        return;
    }
    if let Some(store) = state.store.as_ref() {
        store.rename_tag(from, to);
        state.pending_views += 1;
    }
    state.overlay = Overlay::None;
    state.tag_rename_from = None;
    state.tag_rename_buffer = TextEdit::new("");
}

/// Move the reading queue forward: open the next unread post, filtering to
/// unread when the current view has none left.
fn next_unread(state: &mut State) {
    let start = if state.route == Route::Detail {
        state.selected + 1
    } else {
        state.selected
    };
    if let Some(index) = state
        .posts
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, post)| !post.is_read)
        .map(|(index, _)| index)
    {
        state.selected = index;
        begin_detail(state, index);
        return;
    }
    if state.route == Route::Detail {
        stop_video(state);
        state.route = Route::Feed;
        if state.back_stack.last() == Some(&Route::Feed) {
            state.back_stack.pop();
        }
    }
    state.selected = 0;
    state.request_feed(Some(Filter::Unread));
}

fn open_note_editor(state: &mut State) {
    let Some(post) = state.posts.get(state.selected) else {
        return;
    };
    state.note_target = Some(post.id.clone());
    state.note_buffer = TextEdit::new(post.note.clone().unwrap_or_default());
    state.overlay = Overlay::Note;
}

fn save_note(state: &mut State) {
    let Some(id) = state.note_target.clone() else {
        state.overlay = Overlay::None;
        return;
    };
    let note = state.note_buffer.text.trim().to_string();
    let value = if note.is_empty() { None } else { Some(note) };
    if let Some(post) = state.posts.iter_mut().find(|post| post.id == id) {
        post.note = value.clone();
    }
    if let Some(store) = state.store.as_ref() {
        store.set_note(id, value);
    }
    state.overlay = Overlay::None;
    state.note_target = None;
    state.note_buffer = TextEdit::new("");
}

fn open_detail(state: &mut State) {
    if state.posts.get(state.selected).is_none() {
        return;
    }
    state.note_navigation(Route::Detail);
    state.route = Route::Detail;
    let id = {
        let Some(post) = state.posts.get_mut(state.selected) else {
            return;
        };
        if post.is_read {
            return;
        }
        post.is_read = true;
        post.id.clone()
    };
    state.thread.clear();
    state.thread_anchor = Some(id.clone());
    if let Some(store) = state.store.as_ref() {
        store.set_read(id.clone(), true);
        state.thread_request = Some(store.thread(id));
        state.pending_views += 1;
    }
}

fn reveal(state: &State, tx: &mut Tx) {
    match state.route {
        Route::Feed => tx.scroll_into_view(format!("post-{}", state.selected)),
        Route::Authors => tx.scroll_into_view(format!("author-{}", state.directory_index)),
        Route::Topics => tx.scroll_into_view(format!("topic-{}", state.directory_index)),
        _ => {}
    }
}

fn begin_tag_edit(state: &mut State, tx: &mut Tx) {
    let Some((id, tags)) = state
        .posts
        .get(state.selected)
        .map(|post| (post.id.clone(), post.tags.clone()))
    else {
        return;
    };
    stop_video(state);
    state.tag_edit = Some(TagEdit {
        bookmark_id: id,
        tags,
    });
    state.tag_buffer = TextEdit::new("");
    state.overlay = Overlay::Tags;
    state.pending_focus = Some("tag-input".into());
    let _ = tx;
}

fn save_tags(state: &mut State) {
    let Some(edit) = state.tag_edit.as_ref() else {
        return;
    };
    let id = edit.bookmark_id.clone();
    let tags = edit.tags.clone();
    if let Some(post) = state.posts.iter_mut().find(|post| post.id == id) {
        post.tags = tags.clone();
    }
    if let Some(store) = state.store.as_ref() {
        store.set_tags(id, tags);
    }
}

fn add_tag(state: &mut State) {
    let value = state
        .tag_buffer
        .text
        .trim()
        .trim_start_matches('#')
        .to_string();
    if value.is_empty() {
        return;
    }
    if let Some(edit) = state.tag_edit.as_mut() {
        if !edit.tags.iter().any(|tag| tag.eq_ignore_ascii_case(&value)) {
            edit.tags.push(value.clone());
        }
    }
    state.tag_buffer = TextEdit::new("");
    save_tags(state);
}

fn remove_tag(state: &mut State, index: usize) {
    if let Some(edit) = state.tag_edit.as_mut() {
        if index < edit.tags.len() {
            edit.tags.remove(index);
        }
    }
    save_tags(state);
}

fn begin_delete(state: &mut State) {
    let Some(id) = state.posts.get(state.selected).map(|post| post.id.clone()) else {
        return;
    };
    stop_video(state);
    state.delete_target = Some(id);
    state.overlay = Overlay::ConfirmDelete;
}

fn confirm_delete(state: &mut State) {
    if state.bulk_delete {
        bulk_delete(state);
        return;
    }
    let Some(id) = state.delete_target.take() else {
        state.overlay = Overlay::None;
        return;
    };
    if let Some(store) = state.store.as_ref() {
        let request = store.delete_bookmark(id.clone());
        state.delete_request = Some((request, id));
    }
    state.overlay = Overlay::None;
}

fn toggle_favorite(state: &mut State) {
    let Some(post) = state.posts.get_mut(state.selected) else {
        return;
    };
    let id = post.id.clone();
    post.favorite = !post.favorite;
    let value = post.favorite;
    if let Some(store) = state.store.as_ref() {
        let request = store.toggle_favorite(id.clone());
        state.favorite_request = Some((request, id));
    } else {
        let _ = value;
    }
}

fn request_all_media(state: &mut State) {
    if !state.auto_media {
        return;
    }
    for index in 0..state.posts.len() {
        load_media(state, index);
    }
}

fn carousel_pages(post: &Post) -> usize {
    if post.media.len() <= 4 {
        1
    } else {
        post.media.len().div_ceil(4)
    }
}

fn carousel_page(post: &Post) -> usize {
    post.media_page.min(carousel_pages(post).saturating_sub(1))
}

fn turn_carousel(state: &mut State, index: usize, delta: isize) {
    let Some(post) = state.posts.get_mut(index) else {
        return;
    };
    let pages = carousel_pages(post);
    if pages <= 1 {
        return;
    }
    let next = (carousel_page(post) as isize + delta).clamp(0, pages as isize - 1) as usize;
    if next == post.media_page {
        return;
    }
    post.media_page = next;
    stop_video(state);
}

fn load_media(state: &mut State, index: usize) {
    let Some(post) = state.posts.get_mut(index) else {
        return;
    };
    if post.media_loaded {
        return;
    }
    post.media_loaded = true;
    let mut requests = Vec::new();
    for item in &post.media {
        requests.push((
            item.key.clone(),
            if item.video {
                ImageKind::Poster
            } else {
                ImageKind::Image
            },
        ));
    }
    if let Some(quote) = &post.quote {
        if let Some(key) = &quote.thumb_key {
            requests.push((key.clone(), ImageKind::Image));
        }
    }
    for (key, kind) in requests {
        state.images.request(&key, kind);
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

fn view(state: &State, cx: &mut Cx<State>) -> El {
    let dark = state.dark.unwrap_or_else(|| cx.system_dark());
    let reduced = state
        .motion_reduced
        .unwrap_or_else(|| cx.system_reduced_motion());
    let p = if dark { theme::night() } else { theme::light() };
    let header = header(state, p, dark).motion(crate::motion::HEADER);
    let body = if !state.booted {
        col().w_full().grow(1.0)
    } else {
        match state.route {
            Route::Onboarding => onboarding(state, p),
            Route::Detail => detail_view(state, p),
            Route::Authors => authors_view(state, p),
            Route::Topics => topics_view(state, p),
            Route::Graph => graph_view(state, p, cx),
            Route::Calendar => calendar_view(state, p),
            Route::Import => import_view(state, p),
            Route::History => history_view(state, p),
            Route::Settings => settings_view(state, p),
            _ => feed_view(state, p),
        }
    };
    let body = body
        .key(format!("route-{:?}", state.route))
        .motion(crate::motion::spring(reduced, crate::motion::DETAIL))
        .transition(crate::motion::route_enter(reduced, crate::motion::DETAIL));
    let body = col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .clip_rounded(0.0)
        .child(body);
    let mut root = col()
        .key("window")
        .role("window")
        .w_full()
        .h_full()
        .fill(p.bg)
        .pointer_buttons([PointerButton::Back, PointerButton::Forward])
        .on_press_tx(|state: &mut State, tx: &mut Tx| match tx.button() {
            PointerButton::Back => navigate_back(state, tx),
            PointerButton::Forward => navigate_forward(state, tx),
            _ => {}
        })
        .on_drop_files(|state: &mut State, files: &[PathBuf], _tx: &mut Tx| {
            state.drop_import_paths(files);
        })
        .accepts_extensions(&["csv", "json", "js"])
        .on_drop_hover(|state: &mut State, over: bool, _tx: &mut Tx| {
            state.import_drop = over;
        })
        .child(header)
        .child(body);
    if state.selection.mode {
        root = root.child(selection_bar(state, p).motion(crate::motion::SELECTION));
    }
    root = root.child(footer(state, p).motion(crate::motion::STATUS));
    if state.overlay != Overlay::None {
        root = root.child(overlay_view(state, p).transition(crate::motion::overlay_enter(reduced)));
    }
    root.on_layout(|state: &mut State, rect: [f32; 4]| {
        state.window_size = Some((rect[2], rect[3]));
    })
    .on_screen_layout(|state: &mut State, rect: [f32; 4]| {
        state.window_origin = Some((rect[0] as i32, rect[1] as i32));
    })
}

/// Persist the last window size and position when the window closes.
pub(crate) fn persist_window_on_close(state: &mut State, _tx: &mut Tx) {
    let (Some((width, height)), Some((x, y))) = (state.window_size, state.window_origin) else {
        return;
    };
    settings::save_window(settings::WindowGeometry {
        width,
        height,
        x,
        y,
    });
}

fn header(state: &State, p: Palette, dark: bool) -> El {
    let active = |route: Route| {
        state.route == route || (route == Route::Feed && state.route == Route::Detail)
    };
    let nav = [
        ("feed", Route::Feed),
        ("authors", Route::Authors),
        ("topics", Route::Topics),
        ("graph", Route::Graph),
        ("calendar", Route::Calendar),
        ("import", Route::Import),
        ("history", Route::History),
        ("settings", Route::Settings),
    ];
    let mut links = row().gap(26.0).center_y();
    for (label, route) in nav {
        links = links.child(
            text(label)
                .font(theme::ui(12.5, 400.0))
                .color(if active(route) { p.ink } else { p.muted })
                .on_press(move |state: &mut State| state.open_route(route)),
        );
    }
    links = links.child(
        text("search")
            .font(theme::ui(12.5, 400.0))
            .color(if state.mode == Mode::Search {
                p.ink
            } else {
                p.muted
            })
            .role("button")
            .label("search")
            .on_press(|state: &mut State| enter_search(state)),
    );
    let brand = row()
        .gap(9.0)
        .center_y()
        .child(
            match if dark {
                state.logo_light.as_ref()
            } else {
                state.logo.as_ref()
            } {
                Some(image) => el().key("brand").size(20.0, 20.0).pixels(image.pixels()),
                None => el().size(20.0, 20.0),
            },
        )
        .child(
            text("eterea")
                .font(theme::serif(20.0))
                .color(p.ink)
                .on_press(|state: &mut State| state.open_route(Route::Feed)),
        );

    if state.mode == Mode::Search {
        let placeholder = state.search.is_empty();
        let shown = if placeholder {
            "search".to_string()
        } else {
            format!("{}|", state.search)
        };
        return row()
            .h(56.0)
            .w_full()
            .px(52.0)
            .center_y()
            .justify_between()
            .child(brand)
            .child(
                row()
                    .w(360.0)
                    .h(32.0)
                    .gap(12.0)
                    .center_y()
                    .border_sides([0.0, 0.0, 1.0, 0.0], p.line)
                    .role("searchbox")
                    .label("search")
                    .child(
                        text(shown)
                            .font(theme::ui(13.0, 400.0))
                            .color(if placeholder { p.muted } else { p.ink })
                            .grow(1.0),
                    )
                    .child(text("esc").font(theme::mono(10.5)).color(p.muted)),
            );
    }

    row()
        .h(56.0)
        .w_full()
        .px(52.0)
        .center_y()
        .justify_between()
        .child(brand)
        .child(links)
}

/// The bottom line: a transient status on the left, the shortcut hint on the
/// right so the key sheet can be found without knowing the `?` key.
fn footer(state: &State, p: Palette) -> El {
    let keys = text("keys")
        .font(theme::mono(10.5))
        .color(p.muted)
        .role("button")
        .label("keyboard shortcuts")
        .on_press(|state: &mut State| {
            state.overlay = if state.overlay == Overlay::Keys {
                Overlay::None
            } else {
                Overlay::Keys
            };
        });
    if let (true, Some((done, total))) = (state.repair_active, state.repair_progress) {
        // A slim rounded track with an accent fill: quiet enough for the
        // Daylight surface, legible at a glance.
        let fraction = if total == 0 {
            0.0
        } else {
            (done as f32 / total as f32).clamp(0.0, 1.0)
        };
        let cancelling = state
            .repair_cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed));
        let label = if cancelling {
            "cancelling"
        } else {
            "repairing media"
        };
        let mut track = row().w(180.0).h(4.0).radius(2.0).fill(p.line);
        if fraction > 0.0 {
            track = track.child(el().w(180.0 * fraction).h(4.0).radius(2.0).fill(p.progress));
        }
        let count = if total == 0 {
            String::new()
        } else {
            format!("{done}/{total}")
        };
        return row()
            .w_full()
            .h(24.0)
            .px(52.0)
            .center_y()
            .justify_between()
            .fill(p.bg)
            .child(
                row()
                    .gap(12.0)
                    .center_y()
                    .child(text(label).font(theme::ui(11.5, 400.0)).color(p.muted))
                    .child(track)
                    .child(text(count).font(theme::mono(10.5)).color(p.muted)),
            )
            .child(
                row()
                    .gap(18.0)
                    .center_y()
                    .child(
                        text("cancel")
                            .font(theme::ui(11.5, 400.0))
                            .color(p.muted)
                            .role("button")
                            .label("cancel repair")
                            .on_press(|state: &mut State| request_cancel_repair(state)),
                    )
                    .child(keys),
            );
    }
    let message = state
        .status
        .as_ref()
        .map(|status| status.text.clone())
        .unwrap_or_default();
    row()
        .w_full()
        .h(24.0)
        .px(52.0)
        .center_y()
        .justify_between()
        .fill(p.bg)
        .child(text(message).font(theme::ui(11.5, 400.0)).color(p.muted))
        .child(keys)
}

fn onboarding(state: &State, p: Palette) -> El {
    let mut lines = col()
        .w_full()
        .h_full()
        .justify_center()
        .items_center()
        .gap(10.0);
    if state.status.is_some() {
        return lines;
    } else {
        lines = lines.child(
            text("nothing here yet")
                .font(theme::serif(24.0))
                .color(p.ink),
        );
        lines = lines.child(
            text("import a bookmarks export")
                .font(theme::ui(13.0, 400.0))
                .color(p.muted)
                .on_press(|state: &mut State| state.route = Route::Import),
        );
    }
    lines
}

fn feed_filter_label(filter: &Filter) -> String {
    match filter {
        Filter::Author(handle) => format!("filtered: @{handle}"),
        Filter::Topic(tag) => format!("filtered: #{tag}"),
        Filter::Day(from, _) => format!("filtered: {}", from.get(..10).unwrap_or(from)),
        Filter::Query(query) => format!("filtered: {query}"),
        Filter::Favorites => "filtered: favorites".to_string(),
        Filter::Unread => "filtered: unread".to_string(),
    }
}

fn feed_filter_line(state: &State, p: Palette) -> Option<El> {
    let filter = state.filter.as_ref()?;
    Some(
        row().w_full().h(20.0).justify_center().child(
            row()
                .w(CONTENT)
                .center_y()
                .gap(12.0)
                .child(
                    text(feed_filter_label(filter))
                        .font(theme::mono(10.5))
                        .color(p.muted),
                )
                .child(
                    text("clear")
                        .font(theme::mono(10.5))
                        .color(p.muted)
                        .role("button")
                        .label("clear filter")
                        .on_press(|state: &mut State| state.request_feed(None)),
                ),
        ),
    )
}

fn feed_view(state: &State, p: Palette) -> El {
    let mut column = col().w_full().h_full();
    if let Some(line) = feed_filter_line(state, p) {
        column = column.child(line);
    }
    if state.posts.is_empty() {
        let message = if state.feed_pending {
            "loading"
        } else {
            "no matches"
        };
        return column
            .justify_center()
            .items_center()
            .child(text(message).font(theme::serif(24.0)).color(p.muted));
    }
    // SPEC section 2: a 64px rhythm between posts. Each post carries 32px of
    // air above and below, so same-day neighbours sit 64px apart and a saved-day
    // rule takes the place of the one 1px line inside that rhythm.
    let mut list = col().w_full().pt(32.0).pb(200.0);
    for (index, post) in state.posts.iter().enumerate() {
        if index > 0 && post.date != state.posts[index - 1].date {
            list = list.child(
                row()
                    .w_full()
                    .justify_center()
                    .child(el().w(CONTENT).h(1.0).fill(p.line)),
            );
        }
        list = list.child(post_view(state, post, index, p, false).pt(32.0).pb(32.0));
    }
    if state.feed_pending && state.feed_has_more {
        list = list.child(
            row()
                .w_full()
                .justify_center()
                .child(text("loading").font(theme::mono(10.5)).color(p.muted)),
        );
    }
    let scroll_offset = state.feed_scroll_offset.clone();
    let viewport_top = state.feed_viewport_top.clone();
    let viewport_height = state.feed_viewport_height.clone();
    let list = list
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .key("feed")
        .grow(1.0)
        .min_h(0.0)
        .on_scroll_offset(move |pos| {
            scroll_offset.set(pos.offset[1]);
        })
        .on_layout_tx(move |_state: &mut State, rect: [f32; 4], tx: &mut Tx| {
            viewport_top.set(rect[1]);
            viewport_height.set(rect[3]);
            tx.live_only();
        })
        .on_at_bottom(|state: &mut State, at| {
            if at {
                state.load_more_feed();
            }
        });
    column.child(list)
}

fn detail_view(state: &State, p: Palette) -> El {
    let Some(post) = state.posts.get(state.selected) else {
        return feed_view(state, p);
    };
    let mut column = col().w_full().grow(1.0).min_h(0.0).pt(56.0).pb(200.0);
    if state.thread.len() > 1 {
        column = column.child(
            row().w_full().justify_center().child(
                row().w(CONTENT).center_y().child(
                    text(format!("thread · {} posts", state.thread.len()))
                        .font(theme::mono(10.5))
                        .color(p.muted),
                ),
            ),
        );
        for thread_post in &state.thread {
            let is_anchor = thread_post.id == post.id;
            if is_anchor {
                column = column.child(post_view(state, post, state.selected, p, true));
                continue;
            }
            column = column.child(
                row().w_full().justify_center().child(
                    col()
                        .w(CONTENT)
                        .gap(6.0)
                        .pt(20.0)
                        .pb(20.0)
                        .border(1.0, p.line)
                        .child(
                            row()
                                .gap(10.0)
                                .center_y()
                                .child(
                                    text(thread_post.name.clone())
                                        .font(theme::ui(12.5, 500.0))
                                        .color(p.ink),
                                )
                                .child(
                                    text(thread_post.handle.clone())
                                        .font(theme::ui(12.0, 400.0))
                                        .color(p.muted),
                                )
                                .child(
                                    text(format!("{} · {}", thread_post.date, thread_post.time))
                                        .font(theme::mono(10.5))
                                        .color(p.muted),
                                ),
                        )
                        .child(
                            text(thread_post.text.clone())
                                .font(theme::serif(16.0))
                                .color(p.ink),
                        ),
                ),
            );
        }
    } else {
        column = column.child(post_view(state, post, state.selected, p, true));
    }
    column
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .grow(1.0)
        .min_h(0.0)
}

fn directory_field(state: &State, p: Palette, placeholder: &str, total: i64) -> El {
    let editing = state.directory_editing;
    let filtering = !state.directory_filter.trim().is_empty();
    let shown = if editing || filtering {
        format!("{}|", state.directory_filter)
    } else {
        placeholder.to_string()
    };
    let color = if filtering || editing { p.ink } else { p.muted };
    let trailing = if editing {
        "esc".to_string()
    } else {
        total.to_string()
    };
    let mut field = row()
        .w(720.0)
        .h(36.0)
        .center_y()
        .justify_between()
        .border_sides(
            [0.0, 0.0, 1.0, 0.0],
            if editing { p.accent } else { p.line },
        )
        .role("searchbox")
        .label(placeholder.to_string())
        .child(text(shown).font(theme::ui(13.0, 400.0)).color(color))
        .child(text(trailing).font(theme::mono(10.5)).color(p.muted));
    if !editing {
        field = field.on_press(|state: &mut State| state.directory_editing = true);
    }
    field
}

fn authors_view(state: &State, p: Palette) -> El {
    let filtering = !state.directory_filter.trim().is_empty();
    let mut list = col().w(720.0).gap(4.0);
    if state.authors.is_empty() {
        let message = if filtering {
            "no matches"
        } else if state.authors_pending {
            "loading"
        } else {
            "no authors"
        };
        list = list.child(
            row()
                .w(720.0)
                .pt(18.0)
                .child(text(message).font(theme::serif(24.0)).color(p.muted)),
        );
    }
    for (index, author) in state.authors.iter().enumerate() {
        let handle = author.handle.clone();
        let selected = index == state.directory_index;
        list = list.child(
            row()
                .key(format!("author-{index}"))
                .w(720.0)
                .h(56.0)
                .center_y()
                .gap(12.0)
                .hover(p.band)
                .on_press(move |state: &mut State| state.filter_author(&handle))
                .child(
                    el().w(2.0)
                        .h(24.0)
                        .fill(if selected { p.accent } else { p.bg }),
                )
                .child(avatar(
                    state,
                    author.profile_image.as_deref(),
                    &author.name,
                    32.0,
                    p,
                ))
                .child(
                    col()
                        .gap(1.0)
                        .child(
                            text(author.name.clone())
                                .font(theme::ui(13.5, 500.0))
                                .color(p.ink),
                        )
                        .child(
                            text(format!("@{}", author.handle))
                                .font(theme::ui(12.0, 400.0))
                                .color(p.muted),
                        ),
                )
                .child(el().grow(1.0))
                .child(
                    text(author.bookmark_count.to_string())
                        .font(theme::mono(10.5))
                        .color(p.muted),
                ),
        );
    }
    if state.authors_pending && !state.authors.is_empty() {
        list = list.child(
            row()
                .w(720.0)
                .h(30.0)
                .center_y()
                .child(text("loading").font(theme::mono(10.5)).color(p.muted)),
        );
    }
    directory_shell(
        col()
            .w_full()
            .pt(64.0)
            .pb(200.0)
            .gap(20.0)
            .child(row().w_full().justify_center().child(directory_field(
                state,
                p,
                "filter authors",
                state.authors_total,
            )))
            .child(row().w_full().justify_center().child(list)),
    )
    .on_at_bottom(|state: &mut State, at| {
        if at {
            state.load_more_authors();
        }
    })
}

fn topics_view(state: &State, p: Palette) -> El {
    let filtering = !state.directory_filter.trim().is_empty();
    let mut list = col().w(720.0).gap(4.0);
    if state.topics.is_empty() {
        let message = if filtering {
            "no matches"
        } else if state.topics_pending {
            "loading"
        } else {
            "no topics"
        };
        list = list.child(
            row()
                .w(720.0)
                .pt(18.0)
                .child(text(message).font(theme::serif(24.0)).color(p.muted)),
        );
    }
    for (index, topic) in state.topics.iter().enumerate() {
        let tag = topic.tag.clone();
        let selected = index == state.directory_index;
        list = list.child(
            row()
                .key(format!("topic-{index}"))
                .w(720.0)
                .h(48.0)
                .center_y()
                .gap(12.0)
                .hover(p.band)
                .on_press(move |state: &mut State| state.filter_topic(&tag))
                .child(
                    el().w(2.0)
                        .h(20.0)
                        .fill(if selected { p.accent } else { p.bg }),
                )
                .child(
                    text(format!("#{}", topic.tag))
                        .font(theme::ui(13.5, 400.0))
                        .color(p.ink),
                )
                .child(el().grow(1.0))
                .child(
                    text(topic.bookmark_count.to_string())
                        .font(theme::mono(10.5))
                        .color(p.muted),
                ),
        );
    }
    if state.topics_pending && !state.topics.is_empty() {
        list = list.child(
            row()
                .w(720.0)
                .h(30.0)
                .center_y()
                .child(text("loading").font(theme::mono(10.5)).color(p.muted)),
        );
    }
    directory_shell(
        col()
            .w_full()
            .pt(64.0)
            .pb(200.0)
            .gap(20.0)
            .child(row().w_full().justify_center().child(directory_field(
                state,
                p,
                "filter topics",
                state.topics_total,
            )))
            .child(row().w_full().justify_center().child(list)),
    )
    .on_at_bottom(|state: &mut State, at| {
        if at {
            state.load_more_topics();
        }
    })
}

fn directory_shell(content: El) -> El {
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(content)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .grow(1.0)
        .min_h(0.0)
}

fn graph_pan_limit(state: &State) -> (f32, f32) {
    (
        (state.graph_viewport.0 * 0.75).max(120.0),
        (state.graph_viewport.1 * 0.75).max(120.0),
    )
}

/// Circle approximated as a polygon for the canvas `Shape` batcher.
fn circle_points(cx: f32, cy: f32, radius: f32) -> Vec<[f32; 2]> {
    const SEGMENTS: usize = 18;
    (0..SEGMENTS)
        .map(|index| {
            let angle = std::f32::consts::TAU * index as f32 / SEGMENTS as f32;
            [cx + angle.cos() * radius, cy + angle.sin() * radius]
        })
        .collect()
}

/// Nearest node within the pointer's grab radius, using the same projection as
/// the view: one canvas target replaces a thousand per-node hit elements.
fn graph_hit_test(state: &State, pointer_x: f32, pointer_y: f32) -> Option<usize> {
    let graph = state.graph.as_ref()?;
    let viewport = state.graph_viewport;
    let center = (viewport.0 / 2.0, viewport.1 / 2.0);
    let radius_x = 0.46 * viewport.0;
    let radius_y = 0.46 * viewport.1;
    let mut best: Option<(usize, f32)> = None;
    for (index, node) in graph.nodes.iter().enumerate() {
        let x = center.0 + state.graph_pan.0 + node.x * radius_x * state.graph_zoom;
        let y = center.1 + state.graph_pan.1 + node.y * radius_y * state.graph_zoom;
        let distance = (pointer_x - x).hypot(pointer_y - y);
        if distance <= 11.0 && best.is_none_or(|(_, best_distance)| distance < best_distance) {
            best = Some((index, distance));
        }
    }
    best.map(|(index, _)| index)
}

fn graph_zoom_at(state: &mut State, pointer: (f32, f32), factor: f32) {
    let before = state.graph_zoom;
    let after = (before * factor).clamp(0.5, 5.0);
    if (after - before).abs() < f32::EPSILON {
        return;
    }
    let center = (state.graph_viewport.0 / 2.0, state.graph_viewport.1 / 2.0);
    let pan = state.graph_pan;
    let ratio = after / before;
    let (limit_x, limit_y) = graph_pan_limit(state);
    state.graph_pan = (
        (pointer.0 - center.0 - (pointer.0 - center.0 - pan.0) * ratio).clamp(-limit_x, limit_x),
        (pointer.1 - center.1 - (pointer.1 - center.1 - pan.1) * ratio).clamp(-limit_y, limit_y),
    );
    state.graph_zoom = after;
}

fn graph_pan_by(state: &mut State, dx: f32, dy: f32) {
    let (limit_x, limit_y) = graph_pan_limit(state);
    state.graph_pan = (
        (state.graph_pan.0 + dx).clamp(-limit_x, limit_x),
        (state.graph_pan.1 + dy).clamp(-limit_y, limit_y),
    );
}

fn tag_color(state: &State, tag: &str, p: Palette) -> Color {
    if let Some(hex) = state.tag_colors.get(tag) {
        if let Some(color) = rune::color::hex(hex) {
            return color;
        }
    }
    const DEFAULT: [&str; 8] = [
        "#2447b8", "#6b4fbb", "#8a5a9e", "#2f6f6a", "#9a5a3c", "#5f6b3a", "#8a3f58", "#3f5f8a",
    ];
    let hash = tag.bytes().fold(0u64, |hash, byte| {
        hash.wrapping_mul(1099511628211).wrapping_add(byte as u64)
    });
    let index = (hash % DEFAULT.len() as u64) as usize;
    rune::color::hex(DEFAULT[index]).unwrap_or(p.accent)
}

fn graph_view(state: &State, p: Palette, cx: &mut Cx<State>) -> El {
    let size = cx.layout_size("graph-field");
    let viewport = size.unwrap_or([state.graph_viewport.0, state.graph_viewport.1]);
    let mut field = col()
        .w_full()
        .h_full()
        .key("graph-field")
        .clip_rounded(0.0)
        .on_layout_tx(|state: &mut State, rect: [f32; 4], _tx: &mut Tx| {
            state.graph_viewport = (rect[2], rect[3]);
        });
    let Some(graph) = &state.graph else {
        return col().w_full().h_full().padding(24.0).child(field);
    };
    let center = (viewport[0] / 2.0, viewport[1] / 2.0);
    // The layout is a unit disc; scale it to the field's aspect ratio so a wide
    // window uses its width instead of leaving the constellation cooped up.
    let radius_x = 0.46 * viewport[0];
    let radius_y = 0.46 * viewport[1];
    let point = |x: f32, y: f32| -> (f32, f32) {
        (
            center.0 + state.graph_pan.0 + x * radius_x * state.graph_zoom,
            center.1 + state.graph_pan.1 + y * radius_y * state.graph_zoom,
        )
    };
    let points: Vec<(f32, f32)> = graph
        .nodes
        .iter()
        .map(|node| point(node.x, node.y))
        .collect();
    let mut edges = Shape::new();
    for edge in &graph.edges {
        let Some(from) = points.get(edge.source) else {
            continue;
        };
        let Some(to) = points.get(edge.target) else {
            continue;
        };
        let focused =
            state.graph_focus == Some(edge.source) || state.graph_focus == Some(edge.target);
        edges = edges.line(
            [from.0, from.1],
            [to.0, to.1],
            if focused { 1.4 } else { 0.9 },
            LineCap::Butt,
            if focused { p.accent.alpha(0.5) } else { p.line },
        );
    }
    let active_filter = if state.overlay == Overlay::GraphSearch {
        state.graph_search_buffer.text.trim().to_lowercase()
    } else {
        state
            .graph_filter
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
    };
    // Canvas-batched dots: one draw call and one pointer target for the whole
    // constellation instead of a thousand hit elements.
    let mut shapes = edges;
    for (index, node) in graph.nodes.iter().enumerate() {
        let (x, y) = points[index];
        let focused = state.graph_focus == Some(index);
        let node_color = tag_color(state, &node.tag, p);
        let weight = (node.bookmark_count.max(1) as f32).ln();
        let side = (3.4 + weight * 0.7).clamp(4.0, 8.5);
        let dimmed = !active_filter.is_empty() && !node.tag.to_lowercase().contains(&active_filter);
        let color = if dimmed {
            p.line.alpha(0.6)
        } else if focused {
            p.accent
        } else {
            node_color
        };
        let radius = if focused { 4.6 } else { side / 2.0 };
        shapes = shapes.fill(circle_points(x, y, radius), color);
    }
    let dots = canvas(shapes)
        .key("graph-edges")
        .size(viewport[0], viewport[1])
        .role("img")
        .label("tag graph; tab cycles tags, enter filters")
        .on_pointer_move_local(
            |state: &mut State, event: rune::PointerEvent, _tx: &mut Tx| {
                state.graph_hover = graph_hit_test(state, event.x, event.y);
                if let Some(index) = state.graph_hover {
                    state.graph_focus = Some(index);
                }
            },
        )
        .on_press(|state: &mut State| {
            if let Some(index) = state.graph_hover {
                if let Some(graph) = state.graph.as_ref() {
                    if let Some(node) = graph.nodes.get(index) {
                        let tag = node.tag.clone();
                        state.graph_focus = Some(index);
                        state.filter_topic(&tag);
                    }
                }
            }
        });
    field = field
        .child(dots)
        .on_wheel(|state: &mut State, event: WheelEvent, _tx: &mut Tx| {
            if event.mods.shift {
                graph_pan_by(state, event.dx, event.dy);
            } else {
                let factor = (1.0 - event.dy * 0.0015).clamp(0.8, 1.25);
                graph_zoom_at(state, (event.x, event.y), factor);
            }
            true
        })
        .on_magnify(|state: &mut State, event: MagnifyEvent, _tx: &mut Tx| {
            graph_zoom_at(state, (event.x, event.y), 1.0 + event.delta);
            true
        })
        .on_drag(
            |state: &mut State, drag: &mut Drag, _tx: &mut Tx| match drag.phase {
                Phase::Down => {
                    state.graph_drag_pan = Some(state.graph_pan);
                    state.graph_press = graph_hit_test(state, drag.start_x, drag.start_y);
                    state.graph_dragged = false;
                }
                Phase::Move => {
                    if drag.dx.abs() > 3.0 || drag.dy.abs() > 3.0 {
                        state.graph_dragged = true;
                    }
                    if let Some(start) = state.graph_drag_pan {
                        let (limit_x, limit_y) = graph_pan_limit(state);
                        state.graph_pan = (
                            (start.0 + drag.dx).clamp(-limit_x, limit_x),
                            (start.1 + drag.dy).clamp(-limit_y, limit_y),
                        );
                    }
                }
                Phase::Up => {
                    if !state.graph_dragged {
                        if let Some(index) = state.graph_press.take() {
                            if let Some(graph) = state.graph.as_ref() {
                                if let Some(node) = graph.nodes.get(index) {
                                    let tag = node.tag.clone();
                                    state.graph_focus = Some(index);
                                    state.filter_topic(&tag);
                                }
                            }
                        }
                    }
                    state.graph_press = None;
                    state.graph_drag_pan = None;
                }
            },
        )
        .on_double_press(|state: &mut State| {
            state.graph_zoom = 1.0;
            state.graph_pan = (0.0, 0.0);
        });
    // Labels are the expensive part of a thousand-node graph: draw them for
    // the focused node and the most-saved tags, and let the rest be dots.
    let label_all = graph.nodes.len() <= 140;
    let label_budget = if label_all { graph.nodes.len() } else { 36 };
    let filter = if state.overlay == Overlay::GraphSearch {
        state.graph_search_buffer.text.trim().to_lowercase()
    } else {
        state
            .graph_filter
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
    };
    // Approximate label collision avoidance: labels go to the right of their
    // dot, so skip one whose anchor overlaps an already-labelled anchor.
    let mut label_anchors: Vec<(f32, f32)> = Vec::new();
    for (index, node) in graph.nodes.iter().enumerate() {
        let (x, y) = points[index];
        let tag = node.tag.clone();
        let focused = state.graph_focus == Some(index);
        let matches_filter = filter.is_empty() || node.tag.to_lowercase().contains(&filter);
        let dimmed = !filter.is_empty() && !matches_filter;
        let wants_label = !dimmed && (focused || label_all || index < label_budget);
        if !wants_label {
            continue;
        }
        let collides = label_anchors
            .iter()
            .any(|(ax, ay)| (x - ax).abs() < 88.0 && (y - ay).abs() < 15.0);
        if !focused && collides {
            continue;
        }
        label_anchors.push((x, y));
        field = field.child(
            text(node.tag.clone())
                .absolute(x + 7.0, y - 8.0)
                .font(theme::ui(12.5, 400.0))
                .color(if focused { p.accent } else { p.ink })
                .on_hover(move |state: &mut State| state.graph_focus = Some(index))
                .on_press(move |state: &mut State| {
                    state.graph_focus = Some(index);
                    state.filter_topic(&tag);
                }),
        );
    }
    col().w_full().h_full().padding(24.0).child(field)
}

fn graph_key(state: &mut State, event: &KeyEvent) -> Option<bool> {
    if state.route != Route::Graph {
        return None;
    }
    match &event.key {
        Key::Left => {
            graph_pan_by(state, 32.0, 0.0);
            Some(true)
        }
        Key::Right => {
            graph_pan_by(state, -32.0, 0.0);
            Some(true)
        }
        Key::Up => {
            graph_pan_by(state, 0.0, 32.0);
            Some(true)
        }
        Key::Down => {
            graph_pan_by(state, 0.0, -32.0);
            Some(true)
        }
        Key::Text(t) if t.as_str() == "+" || t.as_str() == "=" => {
            graph_zoom_at(
                state,
                (state.graph_viewport.0 / 2.0, state.graph_viewport.1 / 2.0),
                1.15,
            );
            Some(true)
        }
        Key::Text(t) if t.as_str() == "-" => {
            graph_zoom_at(
                state,
                (state.graph_viewport.0 / 2.0, state.graph_viewport.1 / 2.0),
                1.0 / 1.15,
            );
            Some(true)
        }
        Key::Text(t) if t.as_str() == "0" => {
            state.graph_zoom = 1.0;
            state.graph_pan = (0.0, 0.0);
            Some(true)
        }
        Key::Text(t) if t.as_str() == "/" => {
            state.overlay = Overlay::GraphSearch;
            state.graph_search_buffer =
                TextEdit::new(state.graph_filter.clone().unwrap_or_default());
            Some(true)
        }
        Key::Text(t) if t.as_str() == "c" => {
            if let (Some(index), Some(graph)) = (state.graph_focus, state.graph.as_ref()) {
                if let Some(node) = graph.nodes.get(index) {
                    state.graph_color_tag = Some(node.tag.clone());
                    state.overlay = Overlay::GraphColor;
                }
            }
            Some(true)
        }
        Key::Tab => {
            let len = state.graph.as_ref().map_or(0, |graph| graph.nodes.len());
            if len > 0 {
                state.graph_focus = Some(match state.graph_focus {
                    Some(index) => (index + 1) % len,
                    None => 0,
                });
            }
            Some(true)
        }
        Key::Enter => {
            if let (Some(index), Some(graph)) = (state.graph_focus, state.graph.as_ref()) {
                if let Some(node) = graph.nodes.get(index) {
                    let tag = node.tag.clone();
                    state.filter_topic(&tag);
                }
            }
            Some(true)
        }
        Key::Escape => {
            if state.graph_filter.is_some() {
                state.graph_filter = None;
                state.graph_search_buffer = TextEdit::new("");
                Some(true)
            } else {
                state.open_route(Route::Feed);
                Some(true)
            }
        }
        _ => None,
    }
}

fn calendar_view(state: &State, p: Palette) -> El {
    let year_mode = state.calendar_mode == CalendarMode::Year;
    let title = if year_mode {
        state.calendar_year.to_string()
    } else {
        state
            .calendar
            .as_ref()
            .map(|calendar| calendar_month(&calendar.month))
            .unwrap_or_else(|| calendar_month(&state.calendar_month))
    };
    let header = row()
        .w(710.0)
        .center_y()
        .justify_between()
        .child(calendar_chevron(
            "‹",
            if year_mode {
                "previous year"
            } else {
                "previous month"
            },
            p,
            move |state: &mut State| {
                if year_mode {
                    state.shift_calendar_year(-1);
                } else {
                    state.shift_calendar_month(-1);
                }
            },
        ))
        .child(
            text(title)
                .font(theme::serif(24.0))
                .color(p.ink)
                .on_press(|state: &mut State| state.toggle_calendar_mode()),
        )
        .child(calendar_chevron(
            "›",
            if year_mode { "next year" } else { "next month" },
            p,
            move |state: &mut State| {
                if year_mode {
                    state.shift_calendar_year(1);
                } else {
                    state.shift_calendar_month(1);
                }
            },
        ));
    let body = if year_mode {
        calendar_year_body(state, p)
    } else {
        calendar_month_body(state, p)
    };
    col()
        .w_full()
        .h_full()
        .justify_center()
        .items_center()
        .gap(24.0)
        .child(header)
        .child(body)
}

fn calendar_chevron(
    label: &str,
    aria: &'static str,
    p: Palette,
    on_press: impl Fn(&mut State) + 'static,
) -> El {
    text(label.to_string())
        .font(theme::serif(20.0))
        .color(p.muted)
        .role("button")
        .label(aria)
        .on_press(on_press)
}

fn calendar_month_body(state: &State, p: Palette) -> El {
    let Some(calendar) = state.calendar.as_ref() else {
        return col().w(710.0).h(420.0);
    };
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let mut head = row().w(710.0).gap(4.0);
    head = head.child(el().w(26.0));
    head = head.child(el().w(2.0));
    for day in ["mon", "tue", "wed", "thu", "fri", "sat", "sun"] {
        head = head.child(
            text(day)
                .w(92.8)
                .align(Align::End)
                .font(theme::ui(11.5, 400.0))
                .color(p.muted),
        );
    }
    let mut grid = col().w(710.0).gap(4.0).child(head);
    for (week_index, week) in calendar.days.chunks(7).enumerate() {
        let selected = week_index == state.calendar_week;
        let week_count: i64 = week.iter().map(|day| day.bookmark_count).sum();
        let mut gutter = col().w(26.0).h(64.0).justify_center().items_end();
        if selected {
            gutter = gutter.child(
                text(week_count.to_string())
                    .font(theme::mono(10.5))
                    .color(p.muted),
            );
        }
        let mut cells = row().gap(4.0);
        for day in week {
            let date = day.date.clone();
            let open_date = day.date.clone();
            let mut cell = col()
                .w(92.8)
                .h(64.0)
                .gap(5.0)
                .padding(6.0)
                .hover(p.band)
                .on_press(move |state: &mut State| state.filter_day(&date));
            if let Some(number) = day.date.get(8..10) {
                let is_today = day.date == today;
                cell = cell.child(
                    text(number.trim_start_matches('0').to_string())
                        .font(theme::mono(12.5))
                        .color(if is_today {
                            p.progress
                        } else if day.bookmark_count > 0 {
                            p.ink
                        } else {
                            p.muted
                        }),
                );
            }
            let mut dots = row().gap(3.0);
            for (dot_index, bookmark_id) in day.bookmark_ids.iter().take(4).enumerate() {
                let id = bookmark_id.clone();
                let day = open_date.clone();
                dots = dots.child(
                    el().size(4.0, 4.0)
                        .radius(2.0)
                        .fill(p.progress)
                        .role("button")
                        .label(format!("open saved post {} from this day", dot_index + 1))
                        .on_press(move |state: &mut State| {
                            state.open_calendar_post(&day, id.clone());
                        }),
                );
            }
            cell = cell.child(dots);
            cells = cells.child(cell);
        }
        grid = grid.child(
            row()
                .w(710.0)
                .center_y()
                .gap(4.0)
                .child(gutter)
                .child(
                    el().w(2.0)
                        .h(64.0)
                        .fill(if selected { p.progress } else { p.bg }),
                )
                .child(cells),
        );
    }
    grid
}

const MONTH_NAMES: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

fn calendar_year_body(state: &State, p: Palette) -> El {
    let Some(data) = state.calendar_year_data.as_ref() else {
        return col().w(710.0).h(420.0);
    };
    let year = data.year;
    let counts: std::collections::HashMap<&str, i64> = data
        .days
        .iter()
        .map(|day| (day.date.as_str(), day.bookmark_count))
        .collect();
    let mut plates = col().w(710.0).gap(14.0);
    for row_index in 0..3 {
        let mut plate_row = row().w(710.0).gap(13.0);
        for column_index in 0..4 {
            let month_index = row_index * 4 + column_index;
            plate_row = plate_row.child(calendar_month_plate(
                year,
                month_index,
                state.calendar_year_focus == month_index,
                &counts,
                p,
            ));
        }
        plates = plates.child(plate_row);
    }
    plates
}

fn calendar_month_plate(
    year: i32,
    month_index: usize,
    selected: bool,
    counts: &std::collections::HashMap<&str, i64>,
    p: Palette,
) -> El {
    let month = (month_index + 1) as u32;
    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_default();
    let offset = first.weekday().num_days_from_monday() as usize;
    let days_in_month = days_in_month(year, month);
    let mut cells: Vec<Option<(String, i64)>> = vec![None; offset];
    for day in 1..=days_in_month {
        let date = format!("{year:04}-{month:02}-{day:02}");
        let count = *counts.get(date.as_str()).unwrap_or(&0);
        cells.push(Some((date, count)));
    }
    while !cells.len().is_multiple_of(7) {
        cells.push(None);
    }
    let mut mini = col().gap(3.0);
    for week in cells.chunks(7) {
        let mut mini_row = row().gap(3.0);
        for cell in week {
            let dot = match cell {
                Some((date, count)) if *count > 0 => {
                    let date = date.clone();
                    let alpha = if *count <= 2 {
                        0.28
                    } else if *count <= 8 {
                        0.55
                    } else {
                        1.0
                    };
                    el().size(4.0, 4.0)
                        .radius(2.0)
                        .fill(p.progress.alpha(alpha))
                        .on_press(move |state: &mut State| state.filter_day(&date))
                }
                Some(_) => el().size(4.0, 4.0).radius(2.0).fill(p.line),
                None => el().size(4.0, 4.0),
            };
            mini_row = mini_row.child(dot);
        }
        mini = mini.child(mini_row);
    }
    let month_name = MONTH_NAMES[month_index].to_string();
    let plate = col()
        .w(168.0)
        .h(120.0)
        .padding(10.0)
        .gap(10.0)
        .items_center()
        .radius(2.0)
        .hover(p.band)
        .on_press(move |state: &mut State| {
            state.request_calendar_month(format!("{year:04}-{month:02}"));
        })
        .child(text(month_name).font(theme::mono(10.5)).color(if selected {
            p.ink
        } else {
            p.muted
        }))
        .child(mini);
    if selected {
        row()
            .w(170.0)
            .h(120.0)
            .gap(2.0)
            .child(el().w(2.0).h(120.0).fill(p.progress))
            .child(plate)
    } else {
        plate
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(year, 1, 1).unwrap_or_default());
    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_default();
    first_next.signed_duration_since(first).num_days().max(0) as u32
}

fn shift_month(month: &str, delta: i32) -> Option<String> {
    let (year, month) = month.split_once('-')?;
    let year: i32 = year.parse().ok()?;
    let month: i32 = month.parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    let total = year * 12 + (month - 1) + delta;
    if total < 0 {
        return None;
    }
    Some(format!("{:04}-{:02}", total / 12, total % 12 + 1))
}

fn month_year(month: &str) -> i32 {
    month
        .get(..4)
        .and_then(|year| year.parse().ok())
        .unwrap_or_else(|| Utc::now().format("%Y").to_string().parse().unwrap_or(2026))
}

fn month_index(month: &str) -> Option<usize> {
    month
        .get(5..7)
        .and_then(|month| month.parse::<usize>().ok())
        .map(|month| month.saturating_sub(1).min(11))
}

fn calendar_month(value: &str) -> String {
    let Some((year, month)) = value.split_once('-') else {
        return value.to_string();
    };
    let name = match month {
        "01" => "January",
        "02" => "February",
        "03" => "March",
        "04" => "April",
        "05" => "May",
        "06" => "June",
        "07" => "July",
        "08" => "August",
        "09" => "September",
        "10" => "October",
        "11" => "November",
        "12" => "December",
        _ => month,
    };
    format!("{name} {year}")
}

fn overlay_view(state: &State, p: Palette) -> El {
    let scrim = el().fill(p.scrim).absolute(0.0, 0.0).w_full().h_full();
    match state.overlay {
        Overlay::Palette => {
            let commands = state.filtered_commands();
            // The panel scrolls once commands outgrow the smallest window; at
            // 960x640 the full list still fits without scrolling.
            let mut panel = col()
                .w(560.0)
                .max_h(552.0)
                .fill(p.bg)
                .border(1.0, p.line)
                .radius(2.0)
                .child(
                    row().h(42.0).px(14.0).center_y().child(
                        text(format!("{}|", state.palette))
                            .font(theme::ui(13.0, 400.0))
                            .color(p.ink),
                    ),
                );
            for (index, (name, item)) in commands.iter().enumerate() {
                let item = *item;
                panel = panel.child(
                    row()
                        .h(28.0)
                        .px(14.0)
                        .center_y()
                        .on_press_tx(move |state: &mut State, tx| {
                            state.close_overlay();
                            state.run_command(item, tx);
                        })
                        .child(text(*name).font(theme::ui(12.5, 400.0)).color(
                            if index == state.palette_index {
                                p.ink
                            } else {
                                p.muted
                            },
                        )),
                );
            }
            el().w_full()
                .h_full()
                .absolute(0.0, 0.0)
                .child(scrim)
                .child(
                    row()
                        .w_full()
                        .h_full()
                        .items_start()
                        .justify_center()
                        .pt(80.0)
                        .child(panel.scroll(ScrollSpec {
                            stick_bottom: false,
                        })),
                )
        }
        Overlay::Keys => {
            // A clean, minimizable tooltip anchored at the bottom right: no
            // scrim, and a press anywhere outside folds it back to the hint.
            let rows = [
                "j k move",
                "enter open",
                "esc close",
                "/ search",
                "mouse back back",
                "mouse forward forward",
                "f favorite",
                "c copy link",
                "o open on x",
                "m media / sound",
                "← → carousel",
                "space play",
                "x select",
                "shift j k extend",
                "cmd a page",
                "cmd shift a all",
                "t tags",
                "d delete",
                "u unread · r read",
                "h history",
                "e export",
                ": commands",
            ];
            let mut panel = col()
                .w(320.0)
                .fill(p.bg)
                .border(1.0, p.line)
                .radius(2.0)
                .on_press(|_state: &mut State| {})
                .role("dialog")
                .label("keyboard shortcuts")
                .child(
                    row()
                        .h(38.0)
                        .px(14.0)
                        .center_y()
                        .justify_between()
                        .child(text("keys").font(theme::serif(15.0)).color(p.ink))
                        .child(text("esc").font(theme::mono(10.5)).color(p.muted)),
                );
            for row_text in rows {
                panel = panel.child(
                    row()
                        .h(25.0)
                        .px(14.0)
                        .center_y()
                        .child(text(row_text).font(theme::mono(10.5)).color(p.muted)),
                );
            }
            el().w_full()
                .h_full()
                .absolute(0.0, 0.0)
                .on_press(|state: &mut State| state.overlay = Overlay::None)
                .child(
                    row()
                        .w_full()
                        .h_full()
                        .items_end()
                        .justify_end()
                        .pb(40.0)
                        .pr(52.0)
                        .child(panel),
                )
        }
        Overlay::Lightbox => {
            let (count, current) = state
                .lightbox
                .and_then(|(post, index)| {
                    state
                        .posts
                        .get(post)
                        .map(|post| (post.media.len(), index + 1))
                })
                .unwrap_or((0, 0));
            let mut image_slot = el()
                .size(1100.0, 700.0)
                .radius(2.0)
                .clip_rounded(2.0)
                .scale(state.lightbox_zoom)
                .motion(crate::motion::LIGHTBOX)
                .on_wheel(|state: &mut State, event: WheelEvent, _tx: &mut Tx| {
                    let factor = (1.0 - event.dy * 0.0015).clamp(0.85, 1.2);
                    state.lightbox_zoom = (state.lightbox_zoom * factor).clamp(0.5, 3.0);
                    true
                });
            if let Some((post_index, media_index)) = state.lightbox {
                if let Some(post) = state.posts.get(post_index) {
                    if let Some(item) = post.media.get(media_index) {
                        let frame = if state.video_target == Some((post_index, media_index)) {
                            state.video_image.as_ref()
                        } else {
                            None
                        };
                        image_slot = match frame.or_else(|| state.images.resolve(&item.key)) {
                            Some(image) => image_slot.pixels_fit(image.pixels(), SurfaceFit::Fit),
                            None => image_slot.fill(p.band),
                        };
                    }
                }
            }
            el().w_full()
                .h_full()
                .absolute(0.0, 0.0)
                .pointer_buttons([PointerButton::Back, PointerButton::Forward])
                .on_press_tx(|state: &mut State, tx: &mut Tx| match tx.button() {
                    PointerButton::Back => {
                        if let Some((post, index)) = state.lightbox {
                            if index > 0 {
                                state.lightbox = Some((post, index - 1));
                                state.lightbox_zoom = 1.0;
                            }
                        }
                    }
                    PointerButton::Forward => {
                        if let Some((post, index)) = state.lightbox {
                            let count = state
                                .posts
                                .get(post)
                                .map(|post| post.media.len())
                                .unwrap_or(0);
                            if index + 1 < count {
                                state.lightbox = Some((post, index + 1));
                                state.lightbox_zoom = 1.0;
                            }
                        }
                    }
                    _ => {}
                })
                .child(el().fill(p.scrim).absolute(0.0, 0.0).w_full().h_full())
                .child(
                    row()
                        .w_full()
                        .h_full()
                        .justify_center()
                        .center_y()
                        .child(image_slot),
                )
                .child(
                    el().absolute(0.0, 0.0)
                        .w(120.0)
                        .h_full()
                        .fill(Color::WHITE.alpha(0.001))
                        .role("button")
                        .label("previous image")
                        .on_press(|state: &mut State| {
                            if let Some((post, index)) = state.lightbox {
                                if index > 0 {
                                    state.lightbox = Some((post, index - 1));
                                    state.lightbox_zoom = 1.0;
                                }
                            }
                        }),
                )
                .child(
                    el().absolute(0.0, 0.0)
                        .w(120.0)
                        .h_full()
                        .fill(Color::WHITE.alpha(0.001))
                        .role("button")
                        .label("next image")
                        .on_press(|state: &mut State| {
                            if let Some((post, index)) = state.lightbox {
                                let count = state
                                    .posts
                                    .get(post)
                                    .map(|post| post.media.len())
                                    .unwrap_or(0);
                                if index + 1 < count {
                                    state.lightbox = Some((post, index + 1));
                                    state.lightbox_zoom = 1.0;
                                }
                            }
                        }),
                )
                .child(
                    row()
                        .center_y()
                        .gap(18.0)
                        .pin(None, None, None, Some(26.0))
                        .child(
                            text("‹")
                                .font(theme::ui(16.0, 400.0))
                                .color(Color::WHITE.alpha(0.75))
                                .role("button")
                                .label("previous image")
                                .on_press(|state: &mut State| {
                                    if let Some((post, index)) = state.lightbox {
                                        if index > 0 {
                                            state.lightbox = Some((post, index - 1));
                                            state.lightbox_zoom = 1.0;
                                        }
                                    }
                                }),
                        )
                        .child(
                            text(if count > 0 {
                                format!("{current} / {count}")
                            } else {
                                String::new()
                            })
                            .font(theme::mono(11.0))
                            .color(Color::WHITE.alpha(0.75)),
                        )
                        .child({
                            let playing = state
                                .video
                                .as_ref()
                                .filter(|_| state.video_target == state.lightbox);
                            let (position, duration) = match playing {
                                Some(player) => (
                                    state
                                        .video_scrub
                                        .unwrap_or_else(|| player.position().as_secs_f64()),
                                    player.duration().map(|d| d.as_secs_f64()).unwrap_or(0.0),
                                ),
                                None => (0.0, 0.0),
                            };
                            let fraction = if duration > 0.0 {
                                (position / duration).clamp(0.0, 1.0) as f32
                            } else {
                                0.0
                            };
                            let mut track = row().w(200.0).h(14.0).center_y().on_drag(
                                |state: &mut State, drag: &mut Drag, tx: &mut Tx| {
                                    let fraction = (drag.x / drag.w.max(1.0)).clamp(0.0, 1.0);
                                    let Some(duration) =
                                        state.video.as_ref().and_then(|player| player.duration())
                                    else {
                                        return;
                                    };
                                    let target = f64::from(fraction) * duration.as_secs_f64();
                                    match drag.phase {
                                        Phase::Down | Phase::Move => {
                                            state.video_scrub = Some(target);
                                        }
                                        Phase::Up => {
                                            seek_video_to(state, tx, target);
                                        }
                                    }
                                },
                            );
                            if duration > 0.0 {
                                track = track.child(
                                    row()
                                        .w(200.0)
                                        .h(3.0)
                                        .radius(2.0)
                                        .fill(Color::WHITE.alpha(0.25))
                                        .child(
                                            el().w(200.0 * fraction)
                                                .h(3.0)
                                                .radius(2.0)
                                                .fill(Color::WHITE.alpha(0.85)),
                                        ),
                                );
                            }
                            row().center_y().gap(10.0).child(track).child(
                                text(if duration > 0.0 {
                                    format!(
                                        "{} / {}",
                                        fmt_playback_time(std::time::Duration::from_secs_f64(
                                            position
                                        )),
                                        fmt_playback_time(std::time::Duration::from_secs_f64(
                                            duration
                                        ))
                                    )
                                } else {
                                    String::new()
                                })
                                .font(theme::mono(11.0))
                                .color(Color::WHITE.alpha(0.75)),
                            )
                        })
                        .child(
                            text("›")
                                .font(theme::ui(16.0, 400.0))
                                .color(Color::WHITE.alpha(0.75))
                                .role("button")
                                .label("next image")
                                .on_press(|state: &mut State| {
                                    if let Some((post, index)) = state.lightbox {
                                        let count = state
                                            .posts
                                            .get(post)
                                            .map(|post| post.media.len())
                                            .unwrap_or(0);
                                        if index + 1 < count {
                                            state.lightbox = Some((post, index + 1));
                                            state.lightbox_zoom = 1.0;
                                        }
                                    }
                                }),
                        ),
                )
                .child(
                    text("close")
                        .font(theme::ui(12.5, 400.0))
                        .color(Color::WHITE)
                        .pin(None, Some(28.0), Some(52.0), None)
                        .on_press(|state: &mut State| {
                            state.lightbox = None;
                            state.overlay = Overlay::None;
                        }),
                )
        }
        Overlay::GraphColor => {
            let Some(tag) = state.graph_color_tag.clone() else {
                return el();
            };
            const SWATCHES: [&str; 8] = [
                "#2447b8", "#6b4fbb", "#8a5a9e", "#2f6f6a", "#9a5a3c", "#5f6b3a", "#8a3f58",
                "#3f5f8a",
            ];
            let mut grid = col().w(320.0).gap(10.0);
            let mut swatches = row().w_full().gap(10.0).flex_wrap();
            for hex in SWATCHES {
                let value = hex.to_string();
                let tag_value = tag.clone();
                swatches = swatches.child(
                    el().size(26.0, 26.0)
                        .radius(13.0)
                        .fill(rune::color::hex(hex).unwrap_or(p.accent))
                        .on_press(move |state: &mut State| {
                            if let Some(store) = state.store.as_ref() {
                                store.set_tag_color(tag_value.clone(), value.clone());
                            }
                            state.tag_colors.insert(tag_value.clone(), value.clone());
                            state.overlay = Overlay::None;
                            state.graph_color_tag = None;
                        }),
                );
            }
            grid = grid
                .child(
                    text(format!("#{tag}"))
                        .font(theme::mono(11.0))
                        .color(p.muted),
                )
                .child(swatches)
                .child(
                    text("reset")
                        .font(theme::ui(12.0, 400.0))
                        .color(p.muted)
                        .on_press(|state: &mut State| {
                            if let Some(tag) = state.graph_color_tag.clone() {
                                state.tag_colors.remove(&tag);
                                if let Some(store) = state.store.as_ref() {
                                    store.reset_tag_color(tag);
                                    state.pending_views += 1;
                                }
                            }
                            state.overlay = Overlay::None;
                            state.graph_color_tag = None;
                        }),
                );
            let panel = col()
                .w(360.0)
                .gap(14.0)
                .padding(20.0)
                .fill(p.bg)
                .border(1.0, p.line)
                .radius(2.0)
                .child(text("color").font(theme::serif(20.0)).color(p.ink))
                .child(grid);
            el().w_full()
                .h_full()
                .absolute(0.0, 0.0)
                .child(scrim)
                .child(
                    row()
                        .w_full()
                        .h_full()
                        .justify_center()
                        .center_y()
                        .child(panel),
                )
        }
        Overlay::GraphSearch => {
            let panel = col()
                .w(420.0)
                .gap(12.0)
                .padding(16.0)
                .fill(p.bg)
                .border(1.0, p.line)
                .radius(2.0)
                .child(text("find a tag").font(theme::serif(18.0)).color(p.ink))
                .child(
                    text_input(&state.graph_search_buffer, theme::ui(15.0, 400.0))
                        .key("graph-search-input")
                        .w_full()
                        .h(34.0)
                        .px(8.0)
                        .radius(2.0)
                        .fill(p.band)
                        .border(1.0, p.line)
                        .color(p.ink)
                        .placeholder("tag name", p.muted)
                        .single_line()
                        .on_edit(|state: &mut State| &mut state.graph_search_buffer),
                )
                .child(
                    text("enter filters the graph, esc clears")
                        .font(theme::mono(10.5))
                        .color(p.muted),
                );
            el().w_full()
                .h_full()
                .absolute(0.0, 0.0)
                .child(scrim)
                .child(
                    row()
                        .w_full()
                        .h_full()
                        .justify_center()
                        .center_y()
                        .child(panel),
                )
        }
        Overlay::TagRename => {
            let tags = state
                .posts
                .get(state.selected)
                .map(|post| post.tags.clone())
                .unwrap_or_default();
            let mut panel = col()
                .w(560.0)
                .gap(12.0)
                .padding(18.0)
                .fill(p.bg)
                .border(1.0, p.line)
                .radius(2.0)
                .child(text("rename tag").font(theme::serif(20.0)).color(p.ink));
            match &state.tag_rename_from {
                None => {
                    panel = panel.child(
                        text("choose a tag from this post")
                            .font(theme::ui(12.0, 400.0))
                            .color(p.muted),
                    );
                    let mut rows = col().w_full().gap(6.0);
                    for tag in &tags {
                        let tag_value = tag.clone();
                        rows = rows.child(
                            text(format!("#{tag}"))
                                .font(theme::mono(12.0))
                                .color(p.accent)
                                .on_press(move |state: &mut State| {
                                    state.tag_rename_from = Some(tag_value.clone());
                                    state.tag_rename_buffer = TextEdit::new(tag_value.clone());
                                }),
                        );
                    }
                    if tags.is_empty() {
                        rows = rows.child(
                            text("this post has no tags")
                                .font(theme::ui(12.0, 400.0))
                                .color(p.muted),
                        );
                    }
                    panel = panel.child(rows);
                }
                Some(from) => {
                    let from = from.clone();
                    panel = panel
                        .child(
                            text(format!("renaming #{from} to"))
                                .font(theme::mono(11.0))
                                .color(p.muted),
                        )
                        .child(
                            text_input(&state.tag_rename_buffer, theme::ui(15.0, 400.0))
                                .key("tag-rename-input")
                                .w_full()
                                .h(34.0)
                                .px(8.0)
                                .radius(2.0)
                                .fill(p.band)
                                .border(1.0, p.line)
                                .color(p.ink)
                                .placeholder("new tag name", p.muted)
                                .single_line()
                                .on_edit(|state: &mut State| &mut state.tag_rename_buffer)
                                .on_submit(|state: &mut State, _tx| save_tag_rename(state)),
                        );
                    let prefix = state.tag_rename_buffer.text.trim().to_lowercase();
                    let mut suggestions = row().w_full().gap(10.0).flex_wrap();
                    let mut shown = 0;
                    for suggestion in &state.tag_suggestions {
                        if shown >= 8 {
                            break;
                        }
                        if suggestion.tag.eq_ignore_ascii_case(&from) {
                            continue;
                        }
                        if !prefix.is_empty() && !suggestion.tag.to_lowercase().starts_with(&prefix)
                        {
                            continue;
                        }
                        let value = suggestion.tag.clone();
                        suggestions = suggestions.child(
                            text(format!("#{}", suggestion.tag))
                                .font(theme::mono(11.0))
                                .color(p.muted)
                                .on_press(move |state: &mut State| {
                                    state.tag_rename_buffer = TextEdit::new(value.clone());
                                    save_tag_rename(state);
                                }),
                        );
                        shown += 1;
                    }
                    panel = panel.child(suggestions);
                    panel = panel.child(
                        row()
                            .w_full()
                            .justify_between()
                            .center_y()
                            .child(
                                text("merging into an existing tag folds the two together")
                                    .font(theme::ui(11.5, 400.0))
                                    .color(p.muted),
                            )
                            .child(
                                text("rename")
                                    .font(theme::ui(12.5, 500.0))
                                    .color(p.accent)
                                    .on_press(|state: &mut State| save_tag_rename(state)),
                            ),
                    );
                }
            }
            el().w_full()
                .h_full()
                .absolute(0.0, 0.0)
                .child(scrim)
                .child(
                    row()
                        .w_full()
                        .h_full()
                        .justify_center()
                        .center_y()
                        .child(panel),
                )
        }
        Overlay::BulkTag => {
            let count = selection_count(state);
            let panel = col()
                .w(460.0)
                .gap(14.0)
                .padding(18.0)
                .fill(p.bg)
                .border(1.0, p.line)
                .radius(2.0)
                .child(
                    text(format!("{} selected", count))
                        .font(theme::serif(20.0))
                        .color(p.ink),
                )
                .child(
                    text_input(&state.tag_buffer, theme::ui(15.0, 400.0))
                        .key("bulk-tag-input")
                        .w_full()
                        .h(34.0)
                        .px(8.0)
                        .radius(2.0)
                        .fill(p.band)
                        .border(1.0, p.line)
                        .color(p.ink)
                        .placeholder("tag", p.muted)
                        .single_line()
                        .on_edit(|state: &mut State| &mut state.tag_buffer)
                        .on_submit(|state: &mut State, _tx| apply_bulk_tag(state)),
                )
                .child(
                    row()
                        .w_full()
                        .gap(18.0)
                        .center_y()
                        .child(
                            text("add")
                                .font(theme::ui(12.5, 500.0))
                                .color(if !state.bulk_tag_remove {
                                    p.accent
                                } else {
                                    p.muted
                                })
                                .on_press(|state: &mut State| {
                                    state.bulk_tag_remove = false;
                                    apply_bulk_tag(state);
                                }),
                        )
                        .child(
                            text("remove")
                                .font(theme::ui(12.5, 500.0))
                                .color(if state.bulk_tag_remove {
                                    p.accent
                                } else {
                                    p.muted
                                })
                                .on_press(|state: &mut State| {
                                    state.bulk_tag_remove = true;
                                    apply_bulk_tag(state);
                                }),
                        )
                        .child(el().grow(1.0))
                        .child(
                            text("cancel")
                                .font(theme::ui(12.5, 400.0))
                                .color(p.muted)
                                .on_press(|state: &mut State| state.overlay = Overlay::None),
                        ),
                );
            el().w_full()
                .h_full()
                .absolute(0.0, 0.0)
                .child(scrim)
                .child(
                    row()
                        .w_full()
                        .h_full()
                        .justify_center()
                        .center_y()
                        .child(panel),
                )
        }
        Overlay::Export => {
            let sheet = state.export_sheet.unwrap_or(ExportSheet {
                scope: ExportScopeChoice::All,
                format: ExportKind::Csv,
            });
            let choice = |label: &str, current: bool, p: Palette| {
                text(label.to_string())
                    .font(theme::ui(13.0, 400.0))
                    .color(if current { p.ink } else { p.muted })
            };
            let scope_row = row()
                .w_full()
                .gap(18.0)
                .center_y()
                .child(
                    text("scope")
                        .w(72.0)
                        .font(theme::ui(12.0, 400.0))
                        .color(p.muted),
                )
                .child(
                    choice("all", sheet.scope == ExportScopeChoice::All, p).on_press(
                        |state: &mut State| {
                            if let Some(sheet) = state.export_sheet.as_mut() {
                                sheet.scope = ExportScopeChoice::All;
                            }
                        },
                    ),
                )
                .child(
                    choice("filter", sheet.scope == ExportScopeChoice::Filter, p).on_press(
                        |state: &mut State| {
                            if let Some(sheet) = state.export_sheet.as_mut() {
                                sheet.scope = ExportScopeChoice::Filter;
                            }
                        },
                    ),
                );
            let format_row = row()
                .w_full()
                .gap(18.0)
                .center_y()
                .child(
                    text("format")
                        .w(72.0)
                        .font(theme::ui(12.0, 400.0))
                        .color(p.muted),
                )
                .child(choice("csv", sheet.format == ExportKind::Csv, p).on_press(
                    |state: &mut State| {
                        if let Some(sheet) = state.export_sheet.as_mut() {
                            sheet.format = ExportKind::Csv;
                        }
                    },
                ))
                .child(
                    choice("json", sheet.format == ExportKind::Json, p).on_press(
                        |state: &mut State| {
                            if let Some(sheet) = state.export_sheet.as_mut() {
                                sheet.format = ExportKind::Json;
                            }
                        },
                    ),
                );
            let panel = col()
                .w(420.0)
                .gap(16.0)
                .padding(20.0)
                .fill(p.bg)
                .border(1.0, p.line)
                .radius(2.0)
                .child(text("export").font(theme::serif(20.0)).color(p.ink))
                .child(scope_row)
                .child(format_row)
                .child(
                    row()
                        .w_full()
                        .justify_end()
                        .gap(18.0)
                        .child(
                            text("cancel")
                                .font(theme::ui(12.5, 400.0))
                                .color(p.muted)
                                .on_press(|state: &mut State| {
                                    state.overlay = Overlay::None;
                                    state.export_sheet = None;
                                }),
                        )
                        .child(
                            text("export")
                                .font(theme::ui(12.5, 500.0))
                                .color(p.accent)
                                .on_press_tx(|state: &mut State, tx| state.export_now(tx)),
                        ),
                );
            el().w_full()
                .h_full()
                .absolute(0.0, 0.0)
                .child(scrim)
                .child(
                    row()
                        .w_full()
                        .h_full()
                        .justify_center()
                        .center_y()
                        .child(panel),
                )
        }
        Overlay::Tags => {
            let edit = state.tag_edit.as_ref();
            let mut panel = col()
                .w(460.0)
                .gap(14.0)
                .padding(18.0)
                .fill(p.bg)
                .border(1.0, p.line)
                .radius(2.0)
                .child(text("tags").font(theme::serif(20.0)).color(p.ink));
            let mut tags = row().w_full().gap(8.0).flex_wrap();
            if let Some(edit) = edit {
                for (index, tag) in edit.tags.iter().enumerate() {
                    tags = tags.child(
                        row()
                            .center_y()
                            .gap(6.0)
                            .child(text(format!("#{tag}")).font(theme::mono(11.0)).color(p.ink))
                            .child(
                                text("×")
                                    .font(theme::ui(13.0, 400.0))
                                    .color(p.muted)
                                    .on_press(move |state: &mut State| remove_tag(state, index)),
                            ),
                    );
                }
            }
            panel = panel.child(tags);
            panel = panel.child(
                text_input(&state.tag_buffer, theme::ui(15.0, 400.0))
                    .key("tag-input")
                    .w_full()
                    .h(34.0)
                    .px(8.0)
                    .radius(2.0)
                    .fill(p.band)
                    .border(1.0, p.line)
                    .color(p.ink)
                    .placeholder("add tag", p.muted)
                    .single_line()
                    .on_edit(|state: &mut State| &mut state.tag_buffer)
                    .on_submit(|state: &mut State, _tx| add_tag(state)),
            );
            panel = panel.child(
                row().w_full().justify_end().gap(16.0).child(
                    text("done")
                        .font(theme::ui(12.5, 500.0))
                        .color(p.accent)
                        .on_press(|state: &mut State| {
                            state.overlay = Overlay::None;
                            state.tag_edit = None;
                        }),
                ),
            );
            el().w_full()
                .h_full()
                .absolute(0.0, 0.0)
                .child(scrim)
                .child(
                    row()
                        .w_full()
                        .h_full()
                        .justify_center()
                        .center_y()
                        .child(panel),
                )
        }
        Overlay::Note => {
            let mut panel = col()
                .w(620.0)
                .gap(14.0)
                .padding(18.0)
                .fill(p.bg)
                .border(1.0, p.line)
                .radius(2.0)
                .child(text("note").font(theme::serif(20.0)).color(p.ink))
                .child(
                    text_input(&state.note_buffer, theme::ui(15.0, 400.0))
                        .key("note-input")
                        .w_full()
                        .min_h(140.0)
                        .max_h(320.0)
                        .px(8.0)
                        .py(8.0)
                        .radius(2.0)
                        .fill(p.band)
                        .border(1.0, p.line)
                        .color(p.ink)
                        .placeholder("a private note about this post", p.muted)
                        .on_edit(|state: &mut State| &mut state.note_buffer)
                        .on_submit(|state: &mut State, _tx| save_note(state)),
                );
            panel = panel.child(
                row()
                    .w_full()
                    .justify_between()
                    .center_y()
                    .child(
                        text("cmd-enter saves, esc closes")
                            .font(theme::mono(10.5))
                            .color(p.muted),
                    )
                    .child(
                        row()
                            .gap(16.0)
                            .child(
                                text("clear")
                                    .font(theme::ui(12.0, 400.0))
                                    .color(p.muted)
                                    .on_press(|state: &mut State| {
                                        state.note_buffer = TextEdit::new("");
                                    }),
                            )
                            .child(
                                text("save")
                                    .font(theme::ui(12.5, 500.0))
                                    .color(p.accent)
                                    .on_press(|state: &mut State| save_note(state)),
                            ),
                    ),
            );
            el().w_full()
                .h_full()
                .absolute(0.0, 0.0)
                .child(scrim)
                .child(
                    row()
                        .w_full()
                        .h_full()
                        .justify_center()
                        .center_y()
                        .child(panel),
                )
        }
        Overlay::ConfirmDelete => {
            let panel = col()
                .w(420.0)
                .gap(18.0)
                .padding(20.0)
                .fill(p.bg)
                .border(1.0, p.line)
                .radius(2.0)
                .child(
                    text(if state.bulk_delete {
                        format!("delete {} posts?", selection_count(state))
                    } else {
                        "delete this post?".to_string()
                    })
                    .font(theme::serif(20.0))
                    .color(p.ink),
                )
                .child(
                    row()
                        .w_full()
                        .justify_end()
                        .gap(18.0)
                        .child(
                            text("cancel")
                                .font(theme::ui(12.5, 400.0))
                                .color(p.muted)
                                .on_press(|state: &mut State| {
                                    state.delete_target = None;
                                    state.overlay = Overlay::None;
                                }),
                        )
                        .child(
                            text("delete")
                                .font(theme::ui(12.5, 500.0))
                                .color(p.accent)
                                .on_press(|state: &mut State| confirm_delete(state)),
                        ),
                );
            el().w_full()
                .h_full()
                .absolute(0.0, 0.0)
                .child(scrim)
                .child(
                    row()
                        .w_full()
                        .h_full()
                        .justify_center()
                        .center_y()
                        .child(panel),
                )
        }
        Overlay::None => el(),
    }
}

fn import_view(state: &State, p: Palette) -> El {
    let mut content = col().w(720.0).gap(18.0);
    match &state.import {
        ImportState::Idle => {
            content = content
                .child(
                    text("drop files or choose them")
                        .font(theme::serif(24.0))
                        .color(p.ink),
                )
                .child(
                    text("CSV, JSON or X archive")
                        .font(theme::mono(10.5))
                        .color(p.muted),
                )
                .child(
                    text("choose files")
                        .font(theme::ui(13.0, 500.0))
                        .color(p.accent)
                        .on_press(|state: &mut State| state.choose_import_files()),
                );
            if state.import_picker.is_some() {
                content = content.child(
                    text("choosing files")
                        .font(theme::ui(12.0, 400.0))
                        .color(p.muted),
                );
            }
        }
        ImportState::Previewing { paths } => {
            content = content
                .child(text("reading files").font(theme::serif(24.0)).color(p.ink))
                .child(
                    text(format!("{} file{}", paths.len(), plural(paths.len())))
                        .font(theme::mono(10.5))
                        .color(p.muted),
                );
        }
        ImportState::Preview { paths, preview } => {
            content = content
                .child(
                    text(format!(
                        "{} post{} from {} file{}",
                        preview.total_parsed,
                        plural(preview.total_parsed),
                        paths.len(),
                        plural(paths.len())
                    ))
                    .font(theme::serif(24.0))
                    .color(p.ink),
                )
                .child(
                    text(format!(
                        "{} duplicate{}",
                        preview.total_duplicates,
                        plural(preview.total_duplicates)
                    ))
                    .font(theme::mono(10.5))
                    .color(p.muted),
                );
            for file in &preview.files {
                content = content.child(
                    row()
                        .w_full()
                        .gap(10.0)
                        .child(
                            text(file.source_label.clone())
                                .font(theme::ui(13.0, 500.0))
                                .color(p.ink),
                        )
                        .child(el().grow(1.0))
                        .child(
                            text(match &file.error {
                                Some(_) => format!("{} · could not parse", file.format),
                                None => format!("{} · {}", file.format, file.bookmark_count),
                            })
                            .font(theme::mono(10.5))
                            .color(p.muted),
                        ),
                );
            }
            content = content.child(
                text(format!("import {} posts", preview.total_parsed))
                    .font(theme::ui(13.0, 500.0))
                    .color(p.accent)
                    .on_press(|state: &mut State| state.start_import()),
            );
            content = content.child(
                text("choose another file")
                    .font(theme::ui(12.0, 400.0))
                    .color(p.muted)
                    .on_press(|state: &mut State| state.choose_import_files()),
            );
        }
        ImportState::Importing { paths } => {
            let (title, detail) = match state.import_progress {
                Some(progress) => (
                    match progress.phase {
                        eterea_app::ImportPhase::Reading => "reading files",
                        eterea_app::ImportPhase::Saving => "saving posts",
                    },
                    format!(
                        "{} of {} file{} · {} row{}",
                        progress.files_done,
                        progress.files_total,
                        plural(progress.files_total),
                        progress.rows,
                        plural(progress.rows)
                    ),
                ),
                None => (
                    "importing",
                    format!("{} file{}", paths.len(), plural(paths.len())),
                ),
            };
            content = content
                .child(text(title).font(theme::serif(24.0)).color(p.ink))
                .child(text(detail).font(theme::mono(10.5)).color(p.muted))
                .child(
                    text("esc cancels before anything is written")
                        .font(theme::ui(12.0, 400.0))
                        .color(p.muted),
                );
        }
        ImportState::Done { summary } => {
            let title = if summary.cancelled {
                "import cancelled".to_string()
            } else {
                format!("{} posts imported", summary.inserted)
            };
            content = content
                .child(text(title).font(theme::serif(24.0)).color(p.ink))
                .child(
                    text(if summary.cancelled {
                        "nothing was written".to_string()
                    } else {
                        format!(
                            "{} duplicate{}",
                            summary.duplicates,
                            plural(summary.duplicates)
                        )
                    })
                    .font(theme::mono(10.5))
                    .color(p.muted),
                );
            for file in summary.files.iter().filter(|file| file.error.is_some()) {
                content = content.child(
                    text(format!(
                        "{}: {}",
                        file.source_label,
                        file.error.clone().unwrap_or_default()
                    ))
                    .font(theme::ui(12.0, 400.0))
                    .color(p.muted),
                );
            }
            content = content.child(
                text("import another file")
                    .font(theme::ui(13.0, 500.0))
                    .color(p.accent)
                    .on_press(|state: &mut State| {
                        state.import = ImportState::Idle;
                        state.import_paths.clear();
                        state.choose_import_files();
                    }),
            );
            content = content.child(
                text("back to feed")
                    .font(theme::ui(12.0, 400.0))
                    .color(p.muted)
                    .on_press(|state: &mut State| state.open_route(Route::Feed)),
            );
        }
        ImportState::Error { message, paths } => {
            content = content.child(
                text(truncate(message, 180))
                    .font(theme::ui(13.0, 400.0))
                    .color(p.ink),
            );
            if !paths.is_empty() {
                content = content.child(
                    text(format!("{} file{}", paths.len(), plural(paths.len())))
                        .font(theme::mono(10.5))
                        .color(p.muted),
                );
            }
            content = content.child(
                text("choose files")
                    .font(theme::ui(13.0, 500.0))
                    .color(p.accent)
                    .on_press(|state: &mut State| {
                        state.import = ImportState::Idle;
                        state.choose_import_files();
                    }),
            );
        }
    }
    let panel = col()
        .w(720.0)
        .gap(18.0)
        .padding(24.0)
        .fill(if state.import_drop { p.band } else { p.bg })
        .border(1.0, if state.import_drop { p.accent } else { p.line })
        .on_drop_files(|state: &mut State, files: &[PathBuf], _tx: &mut Tx| {
            state.drop_import_paths(files);
        })
        .accepts_extensions(&["csv", "json", "js"])
        .on_drop_hover(|state: &mut State, over: bool, _tx: &mut Tx| {
            state.import_drop = over;
        })
        .child(content);
    col()
        .w_full()
        .h_full()
        .pt(120.0)
        .child(row().w_full().justify_center().child(panel))
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

fn truncate(text: &str, max: usize) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if out.chars().count() >= max {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn selection_count(state: &State) -> usize {
    state
        .selection
        .affected
        .map(|count| count.max(0) as usize)
        .unwrap_or(state.selection.ids.len())
}

fn selection_bar(state: &State, p: Palette) -> El {
    let count = selection_count(state);
    row()
        .key("selection-bar")
        .w_full()
        .h(36.0)
        .px(52.0)
        .center_y()
        .gap(18.0)
        .border(1.0, p.line)
        .child(
            text(format!("{count} selected"))
                .font(theme::mono(10.5))
                .color(p.ink),
        )
        .child(
            text("tag")
                .font(theme::ui(11.5, 500.0))
                .color(p.accent)
                .on_press(|state: &mut State| begin_bulk_tag(state)),
        )
        .child(
            text("delete")
                .font(theme::ui(11.5, 400.0))
                .color(p.muted)
                .on_press(|state: &mut State| begin_bulk_delete(state)),
        )
        .child(
            text("read")
                .font(theme::ui(11.5, 400.0))
                .color(p.muted)
                .on_press(|state: &mut State| bulk_read(state, true)),
        )
        .child(
            text("unread")
                .font(theme::ui(11.5, 400.0))
                .color(p.muted)
                .on_press(|state: &mut State| bulk_read(state, false)),
        )
        .child(el().grow(1.0))
        .child(
            text("clear")
                .font(theme::ui(11.5, 400.0))
                .color(p.muted)
                .on_press(|state: &mut State| clear_selection(state)),
        )
}

fn history_label(label: &str) -> String {
    label.replace(" 1 posts", " 1 post")
}

fn history_view(state: &State, p: Palette) -> El {
    let mut list = col().w(720.0).gap(2.0);
    if state.history.is_empty() {
        list = list.child(
            text("no actions yet")
                .font(theme::ui(13.0, 400.0))
                .color(p.muted),
        );
    }
    for event in &state.history {
        let id = event.id;
        let selected = state.history_selected == Some(id);
        let kind = match event.kind {
            HistoryKind::Import => "import",
            HistoryKind::Delete => "delete",
            HistoryKind::Tag => "tags",
        };
        let when = event
            .created_at
            .with_timezone(&chrono::Local)
            .format("%d %b %H:%M")
            .to_string();
        list = list.child(
            row()
                .key(format!("history-{id}"))
                .w(720.0)
                .h(44.0)
                .center_y()
                .gap(12.0)
                .on_press(move |state: &mut State| state.select_history(id))
                .child(
                    el().w(2.0)
                        .h(20.0)
                        .fill(if selected { p.accent } else { p.bg }),
                )
                .child(text(kind).w(48.0).font(theme::mono(10.5)).color(p.muted))
                .child(
                    text(history_label(&event.summary.label))
                        .font(theme::ui(13.0, 400.0))
                        .color(if selected { p.ink } else { p.muted })
                        .grow(1.0),
                )
                .child(
                    text(if event.undone_at.is_some() {
                        "undone".to_string()
                    } else {
                        when
                    })
                    .font(theme::mono(10.5))
                    .color(p.muted),
                ),
        );
    }

    let mut detail = col().w(720.0).gap(14.0);
    if let Some(detail_view) = &state.history_detail {
        let event = &detail_view.event;
        detail = detail.child(
            text(history_label(&event.summary.label))
                .font(theme::serif(22.0))
                .color(p.ink),
        );
        if !event.summary.files.is_empty() {
            detail = detail.child(
                text(event.summary.files.join(" · "))
                    .font(theme::mono(10.5))
                    .color(p.muted),
            );
        }
        if !event.summary.tags.is_empty() {
            detail = detail.child(
                text(
                    event
                        .summary
                        .tags
                        .iter()
                        .map(|tag| format!("#{tag}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                )
                .font(theme::mono(10.5))
                .color(p.muted),
            );
        }
        for item in detail_view.items.iter().take(40) {
            let mut row_view = col().w_full().gap(3.0).child(
                row()
                    .gap(8.0)
                    .child(
                        text(format!("@{}", item.author_handle))
                            .font(theme::ui(11.5, 400.0))
                            .color(p.muted),
                    )
                    .child(
                        text(truncate(&item.content, 92))
                            .font(theme::ui(12.5, 400.0))
                            .color(p.ink),
                    ),
            );
            match (&item.before, &item.after) {
                (Some(HistoryState::Tags(before)), Some(HistoryState::Tags(after))) => {
                    row_view = row_view.child(
                        text(format!("{} -> {}", tag_list(before), tag_list(after)))
                            .font(theme::mono(10.0))
                            .color(p.muted),
                    );
                }
                (Some(HistoryState::Bookmark(bookmark)), _) => {
                    row_view = row_view.child(
                        text(format!(
                            "restores {}",
                            bookmark.tweeted_at.format("%d %b %Y")
                        ))
                        .font(theme::mono(10.0))
                        .color(p.muted),
                    );
                }
                _ => {}
            }
            detail = detail.child(row_view);
        }
        if detail_view.items.len() > 40 {
            detail = detail.child(
                text(format!("{} more", detail_view.items.len() - 40))
                    .font(theme::mono(10.0))
                    .color(p.muted),
            );
        }
        if event.can_undo {
            detail = detail.child(
                row()
                    .w_full()
                    .gap(18.0)
                    .child(
                        text("undo")
                            .font(theme::ui(13.0, 500.0))
                            .color(p.accent)
                            .on_press(|state: &mut State| state.undo_latest()),
                    )
                    .child(
                        text("export")
                            .font(theme::ui(12.0, 400.0))
                            .color(p.muted)
                            .on_press(|state: &mut State| state.open_export()),
                    ),
            );
        } else if event.undone_at.is_none() {
            detail = detail.child(
                text("undo newer actions first")
                    .font(theme::ui(11.5, 400.0))
                    .color(p.muted),
            );
        }
    }
    directory_shell(
        col()
            .w_full()
            .pt(64.0)
            .pb(200.0)
            .gap(28.0)
            .child(row().w_full().justify_center().child(list))
            .child(row().w_full().justify_center().child(detail))
            .child(
                row().w_full().justify_center().child(
                    text("export archive")
                        .font(theme::ui(12.5, 400.0))
                        .color(p.muted)
                        .on_press(|state: &mut State| state.open_export()),
                ),
            ),
    )
}

fn tag_list(tags: &[String]) -> String {
    if tags.is_empty() {
        "no tags".into()
    } else {
        tags.iter()
            .map(|tag| format!("#{tag}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn settings_view(state: &State, p: Palette) -> El {
    let mut rows = col().w(720.0).gap(18.0);
    rows = rows.child(
        row()
            .gap(22.0)
            .center_y()
            .child(
                text("appearance")
                    .w(140.0)
                    .font(theme::ui(12.0, 400.0))
                    .color(p.muted),
            )
            .child(setting_choice(
                "system",
                state.dark.is_none(),
                p,
                theme_system,
            ))
            .child(setting_choice(
                "light",
                state.dark == Some(false),
                p,
                theme_light,
            ))
            .child(setting_choice(
                "dark",
                state.dark == Some(true),
                p,
                theme_dark,
            )),
    );
    rows = rows.child(
        row()
            .gap(22.0)
            .center_y()
            .child(
                text("avatars")
                    .w(140.0)
                    .font(theme::ui(12.0, 400.0))
                    .color(p.muted),
            )
            .child(setting_choice("on", state.avatars, p, avatars_on))
            .child(setting_choice("off", !state.avatars, p, avatars_off)),
    );
    rows = rows.child(
        row()
            .gap(22.0)
            .center_y()
            .child(
                text("media")
                    .w(140.0)
                    .font(theme::ui(12.0, 400.0))
                    .color(p.muted),
            )
            .child(setting_choice(
                "load automatically",
                state.auto_media,
                p,
                media_on,
            ))
            .child(setting_choice(
                "load on demand",
                !state.auto_media,
                p,
                media_off,
            )),
    );
    rows = rows.child(
        row()
            .gap(22.0)
            .center_y()
            .child(
                text("motion")
                    .w(140.0)
                    .font(theme::ui(12.0, 400.0))
                    .color(p.muted),
            )
            .child(setting_choice(
                "system",
                state.motion_reduced != Some(true),
                p,
                motion_system,
            ))
            .child(setting_choice(
                "reduced",
                state.motion_reduced == Some(true),
                p,
                motion_reduced,
            )),
    );
    rows = rows.child(
        row()
            .gap(22.0)
            .center_y()
            .child(
                text("cache")
                    .w(140.0)
                    .font(theme::ui(12.0, 400.0))
                    .color(p.muted),
            )
            .child(
                text(format!(
                    "{} images",
                    format_bytes(
                        state
                            .images
                            .cache_stats()
                            .map(|stats| stats.bytes)
                            .unwrap_or(0)
                    )
                ))
                .font(theme::mono(10.5))
                .color(p.muted),
            )
            .child(
                text(
                    state
                        .video_cache
                        .as_ref()
                        .map(|cache| format_bytes(cache.stats().bytes))
                        .unwrap_or_else(|| "0 B video".to_string()),
                )
                .font(theme::mono(10.5))
                .color(p.muted),
            )
            .child(
                text("clear")
                    .font(theme::ui(12.0, 400.0))
                    .color(p.accent)
                    .on_press_tx(|state: &mut State, tx| clear_caches(state, tx.clock())),
            ),
    );
    rows = rows.child(
        row()
            .gap(22.0)
            .center_y()
            .child(
                text("text")
                    .w(140.0)
                    .font(theme::ui(12.0, 400.0))
                    .color(p.muted),
            )
            .child(setting_choice(
                "regular",
                state.text_size == 0,
                p,
                text_regular,
            ))
            .child(setting_choice(
                "larger",
                state.text_size == 1,
                p,
                text_larger,
            ))
            .child(setting_choice(
                "largest",
                state.text_size == 2,
                p,
                text_largest,
            )),
    );
    if let Some(stats) = &state.stats {
        rows = rows
            .child(
                row()
                    .gap(22.0)
                    .center_y()
                    .child(
                        text("posts")
                            .w(140.0)
                            .font(theme::ui(12.0, 400.0))
                            .color(p.muted),
                    )
                    .child(
                        text(stats.total_bookmarks.to_string())
                            .font(theme::mono(10.5))
                            .color(p.muted),
                    ),
            )
            .child(
                row()
                    .gap(22.0)
                    .center_y()
                    .child(
                        text("authors")
                            .w(140.0)
                            .font(theme::ui(12.0, 400.0))
                            .color(p.muted),
                    )
                    .child(
                        text(stats.unique_authors.to_string())
                            .font(theme::mono(10.5))
                            .color(p.muted),
                    ),
            )
            .child(
                row()
                    .gap(22.0)
                    .center_y()
                    .child(
                        text("tags")
                            .w(140.0)
                            .font(theme::ui(12.0, 400.0))
                            .color(p.muted),
                    )
                    .child(
                        text(stats.unique_tags.to_string())
                            .font(theme::mono(10.5))
                            .color(p.muted),
                    ),
            );
    }
    let database = std::env::var_os("ETEREA_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(eterea_core::Database::default_path);
    rows = rows.child(
        row()
            .gap(22.0)
            .center_y()
            .child(
                text("database")
                    .w(140.0)
                    .font(theme::ui(12.0, 400.0))
                    .color(p.muted),
            )
            .child(
                text(database.display().to_string())
                    .font(theme::mono(10.5))
                    .color(p.muted),
            ),
    );
    rows = rows.child(
        row()
            .gap(22.0)
            .center_y()
            .child(
                text("backups")
                    .w(140.0)
                    .font(theme::ui(12.0, 400.0))
                    .color(p.muted),
            )
            .child(
                text(match state.backup_count {
                    Some(0) => "none yet".to_string(),
                    Some(count) => format!("{count} copies"),
                    None => "unknown".to_string(),
                })
                .font(theme::mono(10.5))
                .color(p.muted),
            )
            .child(
                text("backup now")
                    .font(theme::ui(12.0, 400.0))
                    .color(p.accent)
                    .on_press_tx(|state: &mut State, _tx: &mut Tx| {
                        if let Some(store) = state.store.as_ref() {
                            store.backup_now();
                            state.pending_views += 1;
                        }
                    }),
            )
            .child(
                text("check integrity")
                    .font(theme::ui(12.0, 400.0))
                    .color(p.accent)
                    .on_press_tx(|state: &mut State, _tx: &mut Tx| {
                        if let Some(store) = state.store.as_ref() {
                            store.integrity_check();
                            state.pending_views += 1;
                        }
                    }),
            ),
    );
    rows = rows.child(
        row()
            .gap(22.0)
            .center_y()
            .child(
                text("tools")
                    .w(140.0)
                    .font(theme::ui(12.0, 400.0))
                    .color(p.muted),
            )
            .child(
                text(tool_status("ffmpeg"))
                    .font(theme::mono(10.5))
                    .color(p.muted),
            )
            .child(
                text(tool_status("ffplay"))
                    .font(theme::mono(10.5))
                    .color(p.muted),
            ),
    );
    rows = rows.child(
        row()
            .gap(22.0)
            .center_y()
            .child(
                text("source")
                    .w(140.0)
                    .font(theme::ui(12.0, 400.0))
                    .color(p.muted),
            )
            .child(
                text("github.com/meillaya/eterea")
                    .font(theme::ui(12.0, 400.0))
                    .color(p.accent)
                    .on_press_tx(|_state: &mut State, tx: &mut Tx| {
                        let _ = tx.open_url("https://github.com/meillaya/eterea");
                    }),
            ),
    );
    rows = rows.child(
        row()
            .gap(22.0)
            .center_y()
            .child(
                text("version")
                    .w(140.0)
                    .font(theme::ui(12.0, 400.0))
                    .color(p.muted),
            )
            .child(text("0.1.0").font(theme::mono(10.5)).color(p.muted)),
    );
    col()
        .w_full()
        .h_full()
        .pt(120.0)
        .child(row().w_full().justify_center().child(rows))
}

fn theme_system(state: &mut State) {
    state.dark = None;
    settings::save(
        state.dark,
        state.avatars,
        state.auto_media,
        state.text_size,
        state.motion_reduced,
    );
}

fn theme_light(state: &mut State) {
    state.dark = Some(false);
    settings::save(
        state.dark,
        state.avatars,
        state.auto_media,
        state.text_size,
        state.motion_reduced,
    );
}

fn theme_dark(state: &mut State) {
    state.dark = Some(true);
    settings::save(
        state.dark,
        state.avatars,
        state.auto_media,
        state.text_size,
        state.motion_reduced,
    );
}

fn avatars_on(state: &mut State) {
    state.avatars = true;
    request_avatars(state);
    settings::save(
        state.dark,
        state.avatars,
        state.auto_media,
        state.text_size,
        state.motion_reduced,
    );
}

fn media_on(state: &mut State) {
    state.auto_media = true;
    request_all_media(state);
    settings::save(
        state.dark,
        state.avatars,
        state.auto_media,
        state.text_size,
        state.motion_reduced,
    );
}

fn media_off(state: &mut State) {
    state.auto_media = false;
    settings::save(
        state.dark,
        state.avatars,
        state.auto_media,
        state.text_size,
        state.motion_reduced,
    );
}

fn motion_system(state: &mut State) {
    state.motion_reduced = None;
    settings::save(
        state.dark,
        state.avatars,
        state.auto_media,
        state.text_size,
        state.motion_reduced,
    );
}

fn motion_reduced(state: &mut State) {
    state.motion_reduced = Some(true);
    settings::save(
        state.dark,
        state.avatars,
        state.auto_media,
        state.text_size,
        state.motion_reduced,
    );
}

fn clear_caches(state: &mut State, now: f64) {
    state.images.clear_cache();
    if let Some(cache) = state.video_cache.as_ref() {
        let _ = cache.clear();
    }
    for post in &mut state.posts {
        if !post.media.is_empty() {
            post.media_loaded = false;
        }
    }
    request_avatars(state);
    if state.auto_media {
        request_all_media(state);
    }
    state.set_status("cache cleared", now);
}

fn tool_status(program: &str) -> String {
    let found = std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let candidate = dir.join(program);
            candidate.is_file()
        })
    });
    if found {
        format!("{program} found")
    } else {
        format!("{program} missing")
    }
}

fn format_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB)
    } else if bytes >= 1024 {
        format!("{:.0} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn avatars_off(state: &mut State) {
    state.avatars = false;
    settings::save(
        state.dark,
        state.avatars,
        state.auto_media,
        state.text_size,
        state.motion_reduced,
    );
}

fn text_regular(state: &mut State) {
    set_text_size(state, 0);
}

fn text_larger(state: &mut State) {
    set_text_size(state, 1);
}

fn text_largest(state: &mut State) {
    set_text_size(state, 2);
}

fn set_text_size(state: &mut State, text_size: u8) {
    state.text_size = text_size;
    theme::set_text_scale(match text_size {
        1 => 1.12,
        2 => 1.24,
        _ => 1.0,
    });
    settings::save(
        state.dark,
        state.avatars,
        state.auto_media,
        state.text_size,
        state.motion_reduced,
    );
}

fn setting_choice(label: &str, current: bool, p: Palette, action: fn(&mut State)) -> El {
    text(label.to_string())
        .key(format!("setting-{label}"))
        .font(theme::ui(13.0, 400.0))
        .color(if current { p.ink } else { p.muted })
        .on_press(move |state: &mut State| action(state))
}

fn post_url(state: &State, index: usize) -> Option<String> {
    state.posts.get(index).map(|post| post.tweet_url.clone())
}

fn calendar_key(state: &mut State, event: &KeyEvent) -> Option<bool> {
    if state.route != Route::Calendar {
        return None;
    }
    match state.calendar_mode {
        CalendarMode::Month => match &event.key {
            Key::Left => {
                state.shift_calendar_week(-1);
                Some(true)
            }
            Key::Right => {
                state.shift_calendar_week(1);
                Some(true)
            }
            Key::Up => {
                state.shift_calendar_month(-1);
                Some(true)
            }
            Key::Down => {
                state.shift_calendar_month(1);
                Some(true)
            }
            Key::Text(t) if t.as_str() == "[" => {
                state.shift_calendar_month(-1);
                Some(true)
            }
            Key::Text(t) if t.as_str() == "]" => {
                state.shift_calendar_month(1);
                Some(true)
            }
            Key::Text(t) if t.as_str() == "y" => {
                state.toggle_calendar_mode();
                Some(true)
            }
            Key::Text(t) if t.as_str() == "t" => {
                state.calendar_today();
                Some(true)
            }
            Key::Enter => {
                state.filter_calendar_week();
                Some(true)
            }
            Key::Escape => {
                state.open_route(Route::Feed);
                Some(true)
            }
            _ => None,
        },
        CalendarMode::Year => match &event.key {
            Key::Left => {
                state.calendar_year_focus = state.calendar_year_focus.saturating_sub(1);
                Some(true)
            }
            Key::Right => {
                state.calendar_year_focus = (state.calendar_year_focus + 1).min(11);
                Some(true)
            }
            Key::Up => {
                state.calendar_year_focus = state.calendar_year_focus.saturating_sub(4);
                Some(true)
            }
            Key::Down => {
                state.calendar_year_focus = (state.calendar_year_focus + 4).min(11);
                Some(true)
            }
            Key::Text(t) if t.as_str() == "[" || t.as_str() == "-" => {
                state.shift_calendar_year(-1);
                Some(true)
            }
            Key::Text(t) if t.as_str() == "]" || t.as_str() == "=" => {
                state.shift_calendar_year(1);
                Some(true)
            }
            Key::Text(t) if t.as_str() == "y" => {
                state.toggle_calendar_mode();
                Some(true)
            }
            Key::Text(t) if t.as_str() == "t" => {
                state.calendar_today();
                Some(true)
            }
            Key::Enter => {
                state.toggle_calendar_mode();
                Some(true)
            }
            Key::Escape => {
                state.calendar_mode = CalendarMode::Month;
                Some(true)
            }
            _ => None,
        },
    }
}

fn post_text(post: &Post, p: Palette, width: f32, detail: bool, index: usize) -> El {
    let (body, pieces) = body_text(post);
    let mut view = text(body)
        .w(width)
        .wrap()
        .font(theme::serif(if detail { 22.0 } else { 21.0 }))
        .color(p.ink);
    let mut spans = Vec::new();
    let mut at = 0usize;
    for piece in &pieces {
        let start = at;
        at += piece.text.len();
        let end = at;
        if let Some(target) = &piece.target {
            spans.push(Span::new(start, end).color(p.accent));
            view = view.link(Link::new(start..end, format!("url:{target}")));
        } else if let Some(tag) = &piece.tag {
            spans.push(Span::new(start, end).color(p.accent));
            view = view.link(Link::new(start..end, format!("tag:{tag}")));
        } else if let Some(handle) = &piece.mention {
            spans.push(Span::new(start, end).color(p.accent));
            view = view.link(Link::new(start..end, format!("mention:{handle}")));
        }
    }
    if !spans.is_empty() {
        view = view.spans(spans);
    }
    if !detail {
        view = view.on_press(move |state: &mut State| {
            // Clicking a body opens the post under the pointer, not whichever
            // row the keyboard had selected.
            begin_detail(state, index);
        });
    }
    view.on_link(|state: &mut State, id: &str, tx: &mut Tx| {
        if let Some(tag) = id.strip_prefix("tag:") {
            state.filter_topic(tag);
        } else if let Some(handle) = id.strip_prefix("mention:") {
            state.filter_author(handle);
        } else if let Some(url) = id.strip_prefix("url:") {
            if eterea_logic::is_safe_https_url(url) {
                let _ = tx.open_url(url);
            }
        }
    })
}

#[derive(Debug, Default)]
struct BodyPiece {
    text: String,
    target: Option<String>,
    tag: Option<String>,
    mention: Option<String>,
}

fn body_text(post: &Post) -> (String, Vec<BodyPiece>) {
    let source = post.text.as_str();
    let mut pieces = Vec::new();
    let mut plain = String::new();
    let mut index = 0;
    while index < source.len() {
        let rest = &source[index..];
        if rest.starts_with("https://") {
            let mut end = rest
                .find(|ch: char| ch.is_whitespace())
                .map(|offset| index + offset)
                .unwrap_or(source.len());
            while end > index
                && matches!(
                    source.as_bytes()[end - 1],
                    b'.' | b',' | b')' | b']' | b';' | b'!' | b'?'
                )
            {
                end -= 1;
            }
            let raw = &source[index..end];
            if let Some(link) = post.links.iter().find(|link| link.url == raw) {
                match link.kind {
                    eterea_core::LinkKind::External => {
                        push_plain(&mut pieces, &mut plain);
                        let target = link
                            .expanded_url
                            .clone()
                            .filter(|_| true)
                            .or_else(|| Some(raw.to_string()));
                        let display = link
                            .display_url
                            .clone()
                            .or_else(|| target.as_deref().and_then(link_label))
                            .unwrap_or_else(|| "link".to_string());
                        pieces.push(BodyPiece {
                            text: display,
                            target,
                            ..BodyPiece::default()
                        });
                    }
                    eterea_core::LinkKind::Tweet | eterea_core::LinkKind::Media => {
                        // The quote card or the media carousel carries this; the
                        // URL itself is noise in the reading surface.
                    }
                }
            } else if is_tweet_url(raw) {
                // No entity was exported, but the shape is unambiguous.
            } else if raw.contains("://t.co/") {
                push_plain(&mut pieces, &mut plain);
                pieces.push(BodyPiece {
                    text: "link".into(),
                    target: Some(raw.into()),
                    ..BodyPiece::default()
                });
            } else {
                push_plain(&mut pieces, &mut plain);
                pieces.push(BodyPiece {
                    text: raw.into(),
                    target: Some(raw.into()),
                    ..BodyPiece::default()
                });
            }
            index = end;
            if source[index..].starts_with(' ') {
                index += 1;
            }
            continue;
        }
        if (rest.starts_with('#') || rest.starts_with('@'))
            && (index == 0
                || source.as_bytes()[index - 1].is_ascii_whitespace()
                || source.as_bytes()[index - 1] == b'(')
        {
            let marker = rest.as_bytes()[0];
            let mut end = index + 1;
            while end < source.len()
                && (source.as_bytes()[end].is_ascii_alphanumeric()
                    || source.as_bytes()[end] == b'_')
            {
                end += 1;
            }
            if end > index + 1 {
                push_plain(&mut pieces, &mut plain);
                let text = source[index..end].to_string();
                let value = text[1..].to_string();
                if marker == b'#' {
                    pieces.push(BodyPiece {
                        text,
                        tag: Some(value),
                        ..BodyPiece::default()
                    });
                } else {
                    pieces.push(BodyPiece {
                        text,
                        mention: Some(value),
                        ..BodyPiece::default()
                    });
                }
                index = end;
                continue;
            }
        }
        let ch = source[index..].chars().next().expect("char boundary");
        plain.push(ch);
        index += ch.len_utf8();
    }
    push_plain(&mut pieces, &mut plain);
    let mut body = String::new();
    for piece in &pieces {
        body.push_str(&piece.text);
    }
    (tidy_spaces(&body), pieces)
}

fn push_plain(pieces: &mut Vec<BodyPiece>, plain: &mut String) {
    if !plain.is_empty() {
        pieces.push(BodyPiece {
            text: std::mem::take(plain),
            ..BodyPiece::default()
        });
    }
}

fn tidy_spaces(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut space = false;
    for ch in value.chars() {
        if ch == ' ' {
            if !space {
                out.push(ch);
            }
            space = true;
        } else {
            space = false;
            out.push(ch);
        }
    }
    out.trim().to_string()
}

fn is_tweet_url(value: &str) -> bool {
    (value.contains("://x.com/") || value.contains("://twitter.com/")) && value.contains("/status/")
}

fn link_label(value: &str) -> Option<String> {
    let rest = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))?;
    let host = rest.split('/').next()?;
    Some(host.to_string())
}

fn post_view(state: &State, post: &Post, index: usize, p: Palette, detail: bool) -> El {
    let content_w = if detail { DETAIL_CONTENT } else { CONTENT };
    let selected = state.selected == index && !detail;
    let marked = state.selection.mode
        && (state.selection.predicate.is_some() || state.selection.ids.contains(&post.id));

    let rail = col()
        .w(RAIL)
        .items_end()
        .child(
            row()
                .w(RAIL)
                .justify_end()
                .center_y()
                .child(
                    text(post.date.clone())
                        .font(theme::ui(11.5, 400.0))
                        .color(p.muted),
                )
                .child(if post.favorite {
                    el().ml(7.0).size(6.0, 6.0).fill(p.accent)
                } else {
                    el().size(0.0, 0.0)
                })
                .child(if post.is_read {
                    el().size(0.0, 0.0)
                } else {
                    el().ml(7.0).size(4.0, 4.0).radius(2.0).fill(p.muted)
                }),
        )
        .child(
            text(post.time.clone())
                .w(RAIL)
                .align(Align::End)
                .font(theme::mono(10.5))
                .color(p.muted),
        );

    let mut content = col().w(content_w).gap(14.0);

    let handle = post.handle.trim_start_matches('@').to_string();
    let mut byline = row()
        .center_y()
        .on_press(move |state: &mut State| state.filter_author(&handle))
        .child(avatar(
            state,
            post.avatar_key.as_deref(),
            &post.name,
            30.0,
            p,
        ))
        .child(
            text(post.name.clone())
                .ml(11.0)
                .font(theme::ui(13.0, 500.0))
                .color(p.ink),
        );
    if !post.handle.is_empty() {
        byline = byline.child(
            text(post.handle.clone())
                .ml(7.0)
                .font(theme::ui(12.0, 400.0))
                .color(p.muted),
        );
    }
    content = content.child(byline);
    content = content.child(post_text(post, p, content_w, detail, index));

    if detail && !post.tags.is_empty() {
        let mut tags = row().w(content_w).gap(10.0).flex_wrap();
        for tag in &post.tags {
            let tag_value = tag.clone();
            tags = tags.child(
                text(format!("#{tag}"))
                    .font(theme::mono(10.5))
                    .color(p.muted)
                    .on_press(move |state: &mut State| state.filter_topic(&tag_value)),
            );
        }
        content = content.child(tags);
    }

    if let Some(quote) = &post.quote {
        content = content.child(quote_view(state, quote, post, p, content_w, index));
    } else if post
        .links
        .iter()
        .any(|link| link.kind == eterea_core::LinkKind::Tweet)
    {
        content = content.child(
            row()
                .w(content_w)
                .gap(12.0)
                .child(el().w(2.0).h(32.0).fill(p.quote_rule))
                .child(
                    text("quoted post not in archive")
                        .font(theme::ui(12.5, 400.0))
                        .color(p.muted),
                ),
        );
    }

    if !post.media.is_empty() && !post.media_loaded {
        content = content.child(
            el().w(content_w)
                .h(180.0)
                .fill(p.band)
                .on_press(move |s: &mut State| load_media(s, index))
                .child(
                    row()
                        .w_full()
                        .h_full()
                        .justify_center()
                        .center_y()
                        .child(text("load media").font(theme::ui(13.0, 500.0)).color(p.ink)),
                ),
        );
    } else if !post.media.is_empty() {
        content = content.child(media_view(state, post, p, content_w, index));
    }

    if detail {
        let target = post.tweet_url.clone();
        content = content.child(
            row()
                .gap(18.0)
                .center_y()
                .child(
                    text(if post.favorite {
                        "favorited"
                    } else {
                        "favorite"
                    })
                    .font(theme::ui(11.5, 400.0))
                    .color(if post.favorite { p.accent } else { p.muted })
                    .on_press(|state: &mut State| toggle_favorite(state)),
                )
                .child(
                    text("copy link")
                        .font(theme::ui(11.5, 400.0))
                        .color(p.muted)
                        .on_press_tx(move |state: &mut State, tx| {
                            tx.copy(post_url(state, state.selected).unwrap_or_default());
                            let now = tx.clock();
                            state.set_status("link copied", now);
                        }),
                )
                .child(
                    text("open on x")
                        .font(theme::ui(11.5, 400.0))
                        .color(p.muted)
                        .on_press_tx(move |_state: &mut State, tx| {
                            if eterea_logic::is_safe_https_url(&target) {
                                let _ = tx.open_url(&target);
                            }
                        }),
                )
                .child(
                    text("edit tags")
                        .font(theme::ui(11.5, 400.0))
                        .color(p.muted)
                        .on_press_tx(|state: &mut State, tx| begin_tag_edit(state, tx)),
                )
                .child(
                    text("delete")
                        .font(theme::ui(11.5, 400.0))
                        .color(p.muted)
                        .on_press(|state: &mut State| begin_delete(state)),
                ),
        );
    }

    if detail {
        if let Some(note) = post.note.as_deref() {
            content = content.child(
                col()
                    .w((content_w - 24.0).max(240.0))
                    .gap(4.0)
                    .pt(16.0)
                    .child(text("note").font(theme::mono(10.5)).color(p.muted))
                    .child(text(note).font(theme::serif(15.0)).color(p.ink)),
            );
        }
    }
    let mut group = col().w(content_w).child(content);
    group = group.child(rail.absolute(-(RAIL + GAP), 0.0));
    if selected || marked {
        // A short, pale rule aligned with the byline. The old full-height ink
        // bar ran the length of a media post and read as a divider.
        let marker = if marked {
            p.accent
        } else {
            p.accent.alpha(0.32)
        };
        group = group.child(
            el().w(2.0)
                .h(18.0)
                .radius(1.0)
                .fill(marker)
                // Sit inside the 12px rail gap: -14 clipped the date rail.
                .pin(Some(-8.0), Some(6.0), None, None),
        );
    }

    let mut row_view = row()
        .w_full()
        .justify_center()
        .key(format!("post-{index}"))
        .child(group);
    if selected && !detail {
        let anchor = state.feed_anchor_top.clone();
        row_view = row_view.on_layout_tx(move |_state: &mut State, rect: [f32; 4], tx: &mut Tx| {
            anchor.set(rect[1]);
            tx.live_only();
        });
    }
    if detail {
        row_view
    } else {
        // Empty areas of the row select it without opening; the body, byline
        // and media keep their own, more specific handlers.
        row_view.on_press(move |state: &mut State| state.selected = index)
    }
}

fn avatar(state: &State, key: Option<&str>, name: &str, size: f32, p: Palette) -> El {
    if let Some(image) = state
        .avatars
        .then_some(())
        .and_then(|_| key.and_then(|key| state.images.resolve(key)))
    {
        return el()
            .size(size, size)
            .radius(size / 2.0)
            .clip_rounded(size / 2.0)
            .pixels_fit(image.pixels(), SurfaceFit::Fill);
    }
    let initial = name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    el().size(size, size)
        .radius(size / 2.0)
        .fill(p.band)
        .center()
        .child(
            text(initial)
                .font(theme::ui(size * 0.4, 500.0))
                .color(p.muted),
        )
}

fn quote_view(
    state: &State,
    quote: &model::Quote,
    post: &Post,
    p: Palette,
    content_w: f32,
    index: usize,
) -> El {
    quote_view_body(state, quote, post, p, content_w).on_press(move |state: &mut State| {
        if state.route != Route::Detail {
            begin_detail(state, index);
        }
    })
}

fn quote_view_body(
    state: &State,
    quote: &model::Quote,
    post: &Post,
    p: Palette,
    content_w: f32,
) -> El {
    if quote.unavailable {
        return text("quoted post not in archive")
            .font(theme::ui(12.0, 400.0))
            .color(p.muted);
    }
    let width = content_w - 18.0;
    let block = col()
        .gap(6.0)
        .pl(18.0)
        .border_sides([0.0, 0.0, 0.0, 1.0], p.quote_rule)
        .child(
            row()
                .center_y()
                .gap(7.0)
                .child(avatar(
                    state,
                    quote.avatar_key.as_deref(),
                    &quote.name,
                    18.0,
                    p,
                ))
                .child(
                    text(quote.name.clone())
                        .font(theme::ui(13.0, 500.0))
                        .color(p.ink),
                )
                .child(
                    text(quote.handle.clone())
                        .font(theme::ui(12.0, 400.0))
                        .color(p.muted),
                )
                .child(
                    text(quote.date.clone())
                        .font(theme::mono(10.5))
                        .color(p.muted),
                ),
        )
        .child(
            text(quote.text.clone())
                .w(width)
                .wrap()
                .font(theme::ui(14.0, 400.0))
                .color(p.ink),
        );
    if post.media_loaded {
        if let Some(thumb) = quote
            .thumb_key
            .as_deref()
            .and_then(|key| state.images.resolve(key))
        {
            return row()
                .w(content_w)
                .items_start()
                .gap(14.0)
                .child(block)
                .child(
                    el().size(72.0, 72.0)
                        .radius(2.0)
                        .clip_rounded(2.0)
                        .pixels_fit(thumb.pixels(), SurfaceFit::Fill),
                );
        }
    }
    block
}

fn media_view(state: &State, post: &Post, p: Palette, width: f32, post_index: usize) -> El {
    let pages = carousel_pages(post);
    let page = carousel_page(post);
    let start = if pages <= 1 { 0 } else { page * 4 };
    let end = (start + 4).min(post.media.len());
    let items = &post.media[start..end];
    let mut view = match items.len() {
        1 => {
            let item = &items[0];
            let aspect = (item.width / item.height).clamp(0.5, 2.5);
            let height = (width / aspect).clamp(240.0, 560.0);
            media_tile(state, item, p, post_index, start, width, height)
        }
        2 => row()
            .w(width)
            .gap(3.0)
            .child(media_tile(
                state,
                &items[0],
                p,
                post_index,
                start,
                (width - 3.0) / 2.0,
                252.0,
            ))
            .child(media_tile(
                state,
                &items[1],
                p,
                post_index,
                start + 1,
                (width - 3.0) / 2.0,
                252.0,
            )),
        3 => col()
            .w(width)
            .gap(3.0)
            .child(media_tile(
                state, &items[0], p, post_index, start, width, 360.0,
            ))
            .child(
                row()
                    .w(width)
                    .gap(3.0)
                    .child(media_tile(
                        state,
                        &items[1],
                        p,
                        post_index,
                        start + 1,
                        (width - 3.0) / 2.0,
                        252.0,
                    ))
                    .child(media_tile(
                        state,
                        &items[2],
                        p,
                        post_index,
                        start + 2,
                        (width - 3.0) / 2.0,
                        252.0,
                    )),
            ),
        _ => {
            let half = (width - 3.0) / 2.0;
            col()
                .w(width)
                .gap(3.0)
                .child(
                    row()
                        .w(width)
                        .gap(3.0)
                        .child(media_tile(
                            state, &items[0], p, post_index, start, half, 252.0,
                        ))
                        .child(media_tile(
                            state,
                            &items[1],
                            p,
                            post_index,
                            start + 1,
                            half,
                            252.0,
                        )),
                )
                .child(
                    row()
                        .w(width)
                        .gap(3.0)
                        .child(media_tile(
                            state,
                            &items[2],
                            p,
                            post_index,
                            start + 2,
                            half,
                            252.0,
                        ))
                        .child(media_tile(
                            state,
                            &items[3],
                            p,
                            post_index,
                            start + 3,
                            half,
                            252.0,
                        )),
                )
        }
    };

    if pages > 1 {
        let previous = page.saturating_sub(1);
        let next = (page + 1).min(pages - 1);
        let back = text("‹")
            .font(theme::ui(16.0, 500.0))
            .color(if page > 0 { p.ink } else { p.line })
            .px(8.0)
            .on_press(move |state: &mut State| {
                if let Some(post) = state.posts.get_mut(post_index) {
                    post.media_page = previous;
                    stop_video(state);
                }
            });
        let forward = text("›")
            .font(theme::ui(16.0, 500.0))
            .color(if page + 1 < pages { p.ink } else { p.line })
            .px(8.0)
            .on_press(move |state: &mut State| {
                if let Some(post) = state.posts.get_mut(post_index) {
                    post.media_page = next;
                    stop_video(state);
                }
            });
        let controls = row()
            .w(width)
            .justify_end()
            .center_y()
            .gap(2.0)
            .child(back)
            .child(
                text(format!("{}/{}", page + 1, pages))
                    .font(theme::mono(10.5))
                    .color(p.muted)
                    .px(4.0),
            )
            .child(forward);
        let controls = controls.motion(crate::motion::CAROUSEL);
        view = col().w(width).gap(4.0).child(view).child(controls);
    }
    view
}

fn media_tile(
    state: &State,
    item: &model::MediaItem,
    p: Palette,
    post_index: usize,
    media_index: usize,
    width: f32,
    height: f32,
) -> El {
    let is_video = item.video;
    let mut tile = el()
        .w(width)
        .h(height)
        .motion(crate::motion::MEDIA)
        .radius(2.0)
        .clip_rounded(2.0)
        .fill(p.band)
        .role("image")
        .label(item.alt.clone())
        .on_press_tx(move |state: &mut State, tx: &mut Tx| {
            state.selected = post_index;
            if is_video && state.route == Route::Detail {
                start_video_at(state, tx, post_index, media_index);
            } else {
                state.lightbox = Some((post_index, media_index));
                state.overlay = Overlay::Lightbox;
            }
        });
    let playing = if state.video_target == Some((post_index, media_index)) {
        state.video_image.as_ref()
    } else {
        None
    };
    if let Some(image) = playing.or_else(|| state.images.resolve(&item.key)) {
        tile = tile.pixels_fit(image.pixels(), SurfaceFit::Fill);
        if item.video && playing.is_none() {
            tile = tile
                .child(
                    el().size(44.0, 44.0)
                        .radius(22.0)
                        .fill(Color::BLACK.alpha(0.35))
                        .center()
                        .absolute(width / 2.0 - 22.0, height / 2.0 - 22.0)
                        .child(text("▶").font(theme::ui(14.0, 500.0)).color(Color::WHITE)),
                )
                .child(
                    el().size(38.0, 18.0)
                        .radius(2.0)
                        .fill(Color::BLACK.alpha(0.55))
                        .center()
                        .absolute(width - 50.0, height - 28.0)
                        .child(
                            text(model::duration_label(item.duration))
                                .font(theme::mono(10.5))
                                .color(Color::WHITE),
                        ),
                );
        }
    }
    tile
}

pub fn spec() -> AppSpec<State> {
    AppSpec {
        fonts: FONTS,
        families: FAMILIES,
        size: (1280.0, 820.0),
        min_size: Some((960.0, 640.0)),
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        scenes: &[
            ("detail", |s: &mut State| {
                s.selected = 1;
                s.route = Route::Detail;
            }),
            ("selected", |s: &mut State| {
                s.selected = 1;
            }),
            ("quote", |s: &mut State| {
                s.selected = 3;
                s.route = Route::Detail;
            }),
            ("video", |s: &mut State| {
                s.selected = 4;
                s.route = Route::Detail;
            }),
            ("hidden", |s: &mut State| {
                s.selected = 5;
                s.route = Route::Detail;
            }),
            ("palette", |s: &mut State| {
                s.overlay = Overlay::Palette;
            }),
            ("keys", |s: &mut State| {
                s.overlay = Overlay::Keys;
            }),
            ("searching", |s: &mut State| {
                s.mode = Mode::Search;
                s.search = "barbican".to_string();
                s.route = Route::Feed;
            }),
            ("lightbox", |s: &mut State| {
                s.selected = 1;
                s.route = Route::Feed;
                s.lightbox = Some((1, 0));
                s.overlay = Overlay::Lightbox;
            }),
            ("note", |s: &mut State| {
                s.selected = 1;
                s.overlay = Overlay::Note;
                s.note_buffer = TextEdit::new("the reading-room chapter, revisit in winter");
            }),
            ("repair", |s: &mut State| {
                s.repair_active = true;
                s.repair_progress = Some((3869, 6144));
            }),
        ],
        ..AppSpec::new("eterea", (1440.0, 900.0), State::new, view)
    }
}
