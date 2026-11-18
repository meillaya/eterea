//! Database schema definitions

pub const SCHEMA: &str = r#"
-- Main bookmarks table
CREATE TABLE IF NOT EXISTS bookmarks (
    id TEXT PRIMARY KEY,
    tweet_url TEXT UNIQUE NOT NULL,
    content TEXT NOT NULL,
    note_text TEXT,
    tweeted_at INTEGER NOT NULL,  -- Unix timestamp for fast sorting
    imported_at INTEGER NOT NULL,
    author_handle TEXT NOT NULL,
    author_name TEXT NOT NULL,
    author_profile_url TEXT,
    author_profile_image TEXT,
    comments TEXT,
    is_favorite INTEGER DEFAULT 0,  -- Boolean as integer (0/1)
    has_media INTEGER DEFAULT 0,    -- Denormalized: 1 if any media rows exist (avoids JOIN on filter)
    is_read INTEGER NOT NULL DEFAULT 0,  -- Reader state: 1 once the post is read
    links_json TEXT NOT NULL DEFAULT '[]' -- URL entities from the export, sorted JSON
);

-- Materialized author directory. The Authors screen must not GROUP BY the
-- full bookmarks table on every navigation; writes keep this table hot.
CREATE TABLE IF NOT EXISTS author_stats (
    author_handle TEXT PRIMARY KEY,
    author_name TEXT NOT NULL,
    author_profile_image TEXT,
    bookmark_count INTEGER NOT NULL DEFAULT 0,
    favorite_count INTEGER NOT NULL DEFAULT 0
);

-- Tags table (normalized for efficient filtering)
CREATE TABLE IF NOT EXISTS tags (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT UNIQUE NOT NULL COLLATE NOCASE
);

-- Bookmark-Tag junction table
CREATE TABLE IF NOT EXISTS bookmark_tags (
    bookmark_id TEXT NOT NULL,
    tag_id INTEGER NOT NULL,
    PRIMARY KEY (bookmark_id, tag_id),
    FOREIGN KEY (bookmark_id) REFERENCES bookmarks(id) ON DELETE CASCADE,
    FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
);

-- Media table
CREATE TABLE IF NOT EXISTS media (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    bookmark_id TEXT NOT NULL,
    url TEXT NOT NULL,
    media_type TEXT NOT NULL,
    alt_text TEXT,
    width INTEGER,
    height INTEGER,
    source_media_key TEXT,
    source_type TEXT,
    preview_url TEXT,
    variant_url TEXT,
    variants_json TEXT,
    duration_ms INTEGER,          -- Video/GIF duration for the mono duration label
    poster_url TEXT,              -- Remote poster image for a video/GIF frame
    playback_url TEXT,            -- Best progressive MP4 URL (never an HLS playlist)
    FOREIGN KEY (bookmark_id) REFERENCES bookmarks(id) ON DELETE CASCADE
);

-- Bookmarks can quote another post. Quotes are display-only and keyed 1:1 to a
-- bookmark, so quote media is serialized here instead of joining the media
-- table. An unavailable quote keeps the URL and the unavailable flag; no text
-- is invented for a missing body.
CREATE TABLE IF NOT EXISTS quoted_tweets (
    bookmark_id TEXT PRIMARY KEY,
    tweet_url TEXT NOT NULL,
    content TEXT NOT NULL DEFAULT '',
    tweeted_at INTEGER,           -- NULL when the export omitted the quoted date
    author_handle TEXT NOT NULL DEFAULT '',
    author_name TEXT NOT NULL DEFAULT '',
    author_profile_image TEXT,
    media_json TEXT NOT NULL DEFAULT '[]',
    unavailable INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (bookmark_id) REFERENCES bookmarks(id) ON DELETE CASCADE
);

-- Reader-chosen tag colors. One row per tag; colors are validated #rrggbb.
CREATE TABLE IF NOT EXISTS tag_colors (
    tag TEXT PRIMARY KEY COLLATE NOCASE,
    color TEXT NOT NULL
);

-- Undo journal. Payloads are JSON blobs; only the newest five events are kept
-- and only the newest non-undone event can be undone.
CREATE TABLE IF NOT EXISTS trash (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    bookmark_id TEXT NOT NULL,
    deleted_at INTEGER NOT NULL,
    payload_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_trash_deleted_at ON trash(deleted_at DESC, id DESC);

CREATE TABLE IF NOT EXISTS history_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    summary_json TEXT NOT NULL DEFAULT '{}',
    undo_json TEXT NOT NULL DEFAULT '{}',
    undone_at INTEGER
);

-- App metadata (sync status, feature state, lightweight preferences)
CREATE TABLE IF NOT EXISTS app_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- FTS5 virtual table for full-text search
CREATE VIRTUAL TABLE IF NOT EXISTS bookmarks_fts USING fts5(
    content,
    note_text,
    author_handle,
    author_name,
    tags_text,
    quote_text,
    alt_text,
    links_text,
    content='bookmarks_search_docs',
    content_rowid='rowid',
    tokenize='porter unicode61'
);

-- External-content document table for FTS5.
--
-- The name deliberately avoids the FTS5 shadow-table namespace: FTS5 owns
-- `<vtab>_data`, `<vtab>_idx`, `<vtab>_content`, `<vtab>_docsize` and
-- `<vtab>_config`, and newer SQLite refuses to create triggers on shadow
-- tables. Do not rename it back to `bookmarks_fts_content`.
CREATE TABLE IF NOT EXISTS bookmarks_search_docs (
    rowid INTEGER PRIMARY KEY,
    bookmark_id TEXT NOT NULL,
    content TEXT,
    note_text TEXT,
    author_handle TEXT,
    author_name TEXT,
    tags_text TEXT,
    quote_text TEXT,
    alt_text TEXT,
    links_text TEXT,
    FOREIGN KEY (bookmark_id) REFERENCES bookmarks(id) ON DELETE CASCADE
);

-- Indexes for common queries
CREATE INDEX IF NOT EXISTS idx_bookmarks_tweeted_at ON bookmarks(tweeted_at DESC);
CREATE INDEX IF NOT EXISTS idx_bookmarks_tweeted_at_id ON bookmarks(tweeted_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_bookmarks_author_handle ON bookmarks(author_handle);
CREATE INDEX IF NOT EXISTS idx_bookmarks_author_date ON bookmarks(author_handle, tweeted_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_author_stats_count_handle ON author_stats(bookmark_count DESC, author_handle ASC);
CREATE INDEX IF NOT EXISTS idx_bookmarks_imported_at ON bookmarks(imported_at DESC);
CREATE INDEX IF NOT EXISTS idx_bookmarks_favorite ON bookmarks(is_favorite) WHERE is_favorite = 1;
CREATE INDEX IF NOT EXISTS idx_bookmarks_fav_date ON bookmarks(is_favorite, tweeted_at DESC) WHERE is_favorite = 1;
CREATE INDEX IF NOT EXISTS idx_bookmarks_unread_date ON bookmarks(tweeted_at DESC, id DESC) WHERE is_read = 0;
CREATE INDEX IF NOT EXISTS idx_history_events_created ON history_events(created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_bookmark_tags_bookmark ON bookmark_tags(bookmark_id);
CREATE INDEX IF NOT EXISTS idx_bookmark_tags_tag ON bookmark_tags(tag_id);
CREATE INDEX IF NOT EXISTS idx_media_bookmark ON media(bookmark_id);
CREATE INDEX IF NOT EXISTS idx_quoted_tweets_tweet_url ON quoted_tweets(tweet_url);
CREATE INDEX IF NOT EXISTS idx_quoted_tweets_author_handle ON quoted_tweets(author_handle);
CREATE INDEX IF NOT EXISTS idx_bookmarks_search_docs_bookmark ON bookmarks_search_docs(bookmark_id);

-- Triggers to keep FTS index in sync
CREATE TRIGGER IF NOT EXISTS bookmarks_fts_insert AFTER INSERT ON bookmarks_search_docs BEGIN
    INSERT INTO bookmarks_fts(rowid, content, note_text, author_handle, author_name, tags_text,
                              quote_text, alt_text, links_text)
    VALUES (NEW.rowid, NEW.content, NEW.note_text, NEW.author_handle, NEW.author_name,
            NEW.tags_text, NEW.quote_text, NEW.alt_text, NEW.links_text);
END;

CREATE TRIGGER IF NOT EXISTS bookmarks_fts_delete AFTER DELETE ON bookmarks_search_docs BEGIN
    INSERT INTO bookmarks_fts(bookmarks_fts, rowid, content, note_text, author_handle, author_name,
                              tags_text, quote_text, alt_text, links_text)
    VALUES ('delete', OLD.rowid, OLD.content, OLD.note_text, OLD.author_handle, OLD.author_name,
            OLD.tags_text, OLD.quote_text, OLD.alt_text, OLD.links_text);
END;

CREATE TRIGGER IF NOT EXISTS bookmarks_fts_update AFTER UPDATE ON bookmarks_search_docs BEGIN
    INSERT INTO bookmarks_fts(bookmarks_fts, rowid, content, note_text, author_handle, author_name,
                              tags_text, quote_text, alt_text, links_text)
    VALUES ('delete', OLD.rowid, OLD.content, OLD.note_text, OLD.author_handle, OLD.author_name,
            OLD.tags_text, OLD.quote_text, OLD.alt_text, OLD.links_text);
    INSERT INTO bookmarks_fts(rowid, content, note_text, author_handle, author_name, tags_text,
                              quote_text, alt_text, links_text)
    VALUES (NEW.rowid, NEW.content, NEW.note_text, NEW.author_handle, NEW.author_name,
            NEW.tags_text, NEW.quote_text, NEW.alt_text, NEW.links_text);
END;

-- Keep author directory summaries current without forcing read-time GROUP BYs.
CREATE TRIGGER IF NOT EXISTS author_stats_bookmark_insert AFTER INSERT ON bookmarks BEGIN
    INSERT INTO author_stats (
        author_handle,
        author_name,
        author_profile_image,
        bookmark_count,
        favorite_count
    )
    VALUES (
        NEW.author_handle,
        COALESCE(NULLIF(NEW.author_name, ''), NEW.author_handle),
        NULLIF(NEW.author_profile_image, ''),
        1,
        CASE WHEN NEW.is_favorite = 1 THEN 1 ELSE 0 END
    )
    ON CONFLICT(author_handle) DO UPDATE SET
        author_name = CASE
            WHEN NEW.author_name IS NOT NULL AND NEW.author_name <> '' THEN NEW.author_name
            ELSE author_stats.author_name
        END,
        author_profile_image = COALESCE(NULLIF(NEW.author_profile_image, ''), author_stats.author_profile_image),
        bookmark_count = author_stats.bookmark_count + 1,
        favorite_count = author_stats.favorite_count + CASE WHEN NEW.is_favorite = 1 THEN 1 ELSE 0 END;
END;

CREATE TRIGGER IF NOT EXISTS author_stats_bookmark_delete AFTER DELETE ON bookmarks BEGIN
    UPDATE author_stats
    SET bookmark_count = bookmark_count - 1,
        favorite_count = favorite_count - CASE WHEN OLD.is_favorite = 1 THEN 1 ELSE 0 END
    WHERE author_handle = OLD.author_handle;

    DELETE FROM author_stats
    WHERE author_handle = OLD.author_handle
      AND bookmark_count <= 0;
END;

CREATE TRIGGER IF NOT EXISTS author_stats_bookmark_favorite_update
AFTER UPDATE OF is_favorite ON bookmarks
WHEN OLD.author_handle = NEW.author_handle AND OLD.is_favorite IS NOT NEW.is_favorite
BEGIN
    UPDATE author_stats
    SET favorite_count = favorite_count
        + CASE WHEN NEW.is_favorite = 1 THEN 1 ELSE 0 END
        - CASE WHEN OLD.is_favorite = 1 THEN 1 ELSE 0 END
    WHERE author_handle = NEW.author_handle;
END;

CREATE TRIGGER IF NOT EXISTS author_stats_bookmark_author_update
AFTER UPDATE OF author_handle ON bookmarks
WHEN OLD.author_handle IS NOT NEW.author_handle
BEGIN
    UPDATE author_stats
    SET bookmark_count = bookmark_count - 1,
        favorite_count = favorite_count - CASE WHEN OLD.is_favorite = 1 THEN 1 ELSE 0 END
    WHERE author_handle = OLD.author_handle;

    DELETE FROM author_stats
    WHERE author_handle = OLD.author_handle
      AND bookmark_count <= 0;

    INSERT INTO author_stats (
        author_handle,
        author_name,
        author_profile_image,
        bookmark_count,
        favorite_count
    )
    VALUES (
        NEW.author_handle,
        COALESCE(NULLIF(NEW.author_name, ''), NEW.author_handle),
        NULLIF(NEW.author_profile_image, ''),
        1,
        CASE WHEN NEW.is_favorite = 1 THEN 1 ELSE 0 END
    )
    ON CONFLICT(author_handle) DO UPDATE SET
        author_name = CASE
            WHEN NEW.author_name IS NOT NULL AND NEW.author_name <> '' THEN NEW.author_name
            ELSE author_stats.author_name
        END,
        author_profile_image = COALESCE(NULLIF(NEW.author_profile_image, ''), author_stats.author_profile_image),
        bookmark_count = author_stats.bookmark_count + 1,
        favorite_count = author_stats.favorite_count + CASE WHEN NEW.is_favorite = 1 THEN 1 ELSE 0 END;
END;

CREATE TRIGGER IF NOT EXISTS author_stats_bookmark_profile_update
AFTER UPDATE OF author_name, author_profile_image ON bookmarks
WHEN OLD.author_handle = NEW.author_handle
BEGIN
    UPDATE author_stats
    SET author_name = CASE
            WHEN NEW.author_name IS NOT NULL AND NEW.author_name <> '' THEN NEW.author_name
            ELSE author_name
        END,
        author_profile_image = COALESCE(NULLIF(NEW.author_profile_image, ''), author_profile_image)
    WHERE author_handle = NEW.author_handle;
END;

"#;

pub const PRAGMAS: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA cache_size = -64000;
PRAGMA temp_store = MEMORY;
PRAGMA mmap_size = 268435456;
PRAGMA foreign_keys = ON;
"#;
