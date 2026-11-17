use super::*;
use rusqlite::{params, Connection};
use std::path::Path;
use tempfile::TempDir;

const SAMPLE_JSON: &str = include_str!("../../../../fixtures/import/bookmarks.json");

#[test]
fn initializes_empty_database_with_current_schema() -> crate::Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("empty.db");
    let db = Database::open(&db_path)?;

    assert_table_exists(&db.conn, "bookmarks")?;
    assert_table_exists(&db.conn, "media")?;
    assert_table_exists(&db.conn, "quoted_tweets")?;
    assert_table_exists(&db.conn, "tag_colors")?;
    assert_table_exists(&db.conn, "history_events")?;
    assert_column_exists(&db.conn, "is_favorite")?;
    assert_column_exists(&db.conn, "has_media")?;
    assert_column_exists(&db.conn, "is_read")?;
    assert_column_exists(&db.conn, "links_json")?;
    assert_media_metadata_columns_exist(&db.conn)?;
    assert_index_exists(&db.conn, "idx_quoted_tweets_tweet_url")?;
    assert_index_exists(&db.conn, "idx_quoted_tweets_author_handle")?;
    assert_index_exists(&db.conn, "idx_bookmarks_unread_date")?;
    assert_index_exists(&db.conn, "idx_history_events_created")?;
    assert_eq!(current_user_version(&db.conn)?, 2);
    assert_eq!(
        db.get_metadata(AUTHOR_STATS_METADATA_KEY)?.as_deref(),
        Some("ready")
    );
    Ok(())
}

#[test]
fn reopens_existing_current_schema_idempotently() -> crate::Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("current.db");
    let first = Database::open(&db_path)?;
    first.conn.execute(
        r#"INSERT INTO bookmarks
           (id, tweet_url, content, tweeted_at, imported_at, author_handle, author_name)
           VALUES ('current-1', 'https://x.com/current/status/1', 'current row',
                   1717243200, 1717243200, 'current', 'Current Author')"#,
        [],
    )?;
    drop(first);

    let reopened = Database::open(&db_path)?;

    let row_count: i64 = reopened
        .conn
        .query_row("SELECT COUNT(*) FROM bookmarks", [], |row| row.get(0))?;
    assert_eq!(row_count, 1);
    assert_column_exists(&reopened.conn, "is_favorite")?;
    assert_column_exists(&reopened.conn, "has_media")?;
    assert_media_metadata_columns_exist(&reopened.conn)?;
    assert_table_exists(&reopened.conn, "quoted_tweets")?;
    assert_eq!(current_user_version(&reopened.conn)?, 2);
    Ok(())
}

#[test]
fn migrates_legacy_bookmarks_shape_idempotently() -> crate::Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("legacy.db");
    create_legacy_database(&db_path)?;

    let db = Database::open(&db_path)?;

    assert_column_exists(&db.conn, "is_favorite")?;
    assert_column_exists(&db.conn, "has_media")?;
    assert_media_metadata_columns_exist(&db.conn)?;
    assert_table_exists(&db.conn, "quoted_tweets")?;
    assert_eq!(current_user_version(&db.conn)?, 2);
    let has_media: i64 = db.conn.query_row(
        "SELECT has_media FROM bookmarks WHERE id = 'legacy-1'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(has_media, 1);
    let author_stats: (i64, i64) = db.conn.query_row(
        "SELECT bookmark_count, favorite_count FROM author_stats WHERE author_handle = 'old_author'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(author_stats, (1, 0));
    assert_eq!(
        db.get_metadata(AUTHOR_STATS_METADATA_KEY)?.as_deref(),
        Some("ready")
    );
    drop(db);

    let reopened = Database::open(&db_path)?;
    let row_count: i64 = reopened
        .conn
        .query_row("SELECT COUNT(*) FROM bookmarks", [], |row| row.get(0))?;
    let author_count: i64 =
        reopened
            .conn
            .query_row("SELECT COUNT(*) FROM author_stats", [], |row| row.get(0))?;
    assert_eq!(row_count, 1);
    assert_eq!(author_count, 1);
    assert_eq!(current_user_version(&reopened.conn)?, 2);
    assert_table_exists(&reopened.conn, "quoted_tweets")?;
    Ok(())
}

#[test]
fn migrates_v1_schema_to_v2_without_data_loss() -> crate::Result<()> {
    // A database from the shipped v1 schema (user_version 1, no reader/links
    // columns, no playback URL, no tag_colors/history_events) must upgrade in
    // place: rows survive, new columns/tables appear, and the media playback
    // URL is backfilled from the stored variants.
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("v1-upgrade.db");
    create_v1_database(&db_path)?;

    let db = Database::open(&db_path)?;

    assert_eq!(current_user_version(&db.conn)?, 2);
    for column_name in ["is_read", "links_json", "is_favorite", "has_media"] {
        assert_column_exists(&db.conn, column_name)?;
    }
    assert_table_column_exists(&db.conn, "media", "playback_url")?;
    assert_table_exists(&db.conn, "tag_colors")?;
    assert_table_exists(&db.conn, "history_events")?;
    assert_index_exists(&db.conn, "idx_bookmarks_unread_date")?;

    let row: (i64, String) = db.conn.query_row(
        "SELECT is_read, links_json FROM bookmarks WHERE id = 'v1-1'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(
        row,
        (0, "[]".to_string()),
        "existing rows read unread with no links"
    );

    let playback_url: Option<String> = db.conn.query_row(
        "SELECT playback_url FROM media WHERE bookmark_id = 'v1-1'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        playback_url.as_deref(),
        Some("https://video.twimg.com/v1/720.mp4"),
        "migration backfills the best MP4 playback URL"
    );
    drop(db);

    // Reopening the upgraded database is a no-op.
    let reopened = Database::open(&db_path)?;
    assert_eq!(current_user_version(&reopened.conn)?, 2);
    assert_eq!(reopened.count_bookmarks()?, 1);
    Ok(())
}

#[test]
fn rejects_future_user_version_without_schema_mutation() -> crate::Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("future-version.db");
    create_future_version_database(&db_path)?;

    let result = Database::open(&db_path);

    assert!(result.is_err());
    let conn = Connection::open(&db_path)?;
    assert_column_absent(&conn, "is_favorite")?;
    assert_column_absent(&conn, "has_media")?;
    Ok(())
}

#[test]
fn rejects_incompatible_media_shape_without_has_media_partial_state() -> crate::Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("bad-media-shape.db");
    create_bad_media_shape_database(&db_path)?;

    let result = Database::open(&db_path);

    assert!(result.is_err());
    let conn = Connection::open(&db_path)?;
    assert_column_absent(&conn, "has_media")?;
    Ok(())
}

#[test]
fn repairs_stale_has_media_values_on_retry() -> crate::Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("stale-has-media.db");
    create_stale_has_media_database(&db_path)?;

    let db = Database::open(&db_path)?;

    let has_media: i64 = db.conn.query_row(
        "SELECT has_media FROM bookmarks WHERE id = 'legacy-1'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(has_media, 1);
    Ok(())
}

#[test]
fn preserves_media_metadata_on_insert_and_reopen() -> crate::Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("media-metadata.db");
    let db = Database::open(&db_path)?;
    let bookmark = crate::models::BookmarkBuilder::new()
        .tweet_url("https://x.com/media/status/1")
        .content("media row")
        .tweeted_at(chrono::Utc::now())
        .author_handle("media")
        .author_name("Media Author")
        .add_media_metadata(
            crate::models::Media::new(
                "https://video.twimg.com/ext_tw_video/full.mp4",
                crate::models::MediaType::Video,
            )
            .with_alt_text(Some("A diagram".to_string()))
            .with_dimensions(Some(1200), Some(900))
            .with_source(Some("3_abc".to_string()), Some("video".to_string()))
            .with_preview_urls(
                Some("https://pbs.twimg.com/media/preview.jpg".to_string()),
                Some("https://video.twimg.com/ext_tw_video/variant.mp4".to_string()),
            )
            .with_variants_json(Some(
                r#"[{"url":"https://video.twimg.com/ext_tw_video/360.mp4","bit_rate":832000},{"url":"https://video.twimg.com/ext_tw_video/720.mp4","bit_rate":2176000}]"#.to_string(),
            ))
            .with_display_metadata(
                Some(42_000),
                Some("https://pbs.twimg.com/media/poster.jpg".to_string()),
            ),
        )
        .build()
        .map_err(|error| crate::Error::Other(error.to_string()))?;
    let bookmark_id = bookmark.id.clone();

    db.insert_bookmarks(&[bookmark])?;
    drop(db);

    let reopened = Database::open(&db_path)?;
    let media = reopened.load_bookmark_media(&bookmark_id)?;

    assert_eq!(media.len(), 1);
    assert_eq!(media[0].alt_text.as_deref(), Some("A diagram"));
    assert_eq!(media[0].width, Some(1200));
    assert_eq!(media[0].height, Some(900));
    assert_eq!(media[0].source_media_key.as_deref(), Some("3_abc"));
    assert_eq!(media[0].source_type.as_deref(), Some("video"));
    assert_eq!(
        media[0].preview_url.as_deref(),
        Some("https://pbs.twimg.com/media/preview.jpg")
    );
    assert_eq!(
        media[0].variant_url.as_deref(),
        Some("https://video.twimg.com/ext_tw_video/variant.mp4")
    );
    let variants_json = media[0].variants_json.as_deref().unwrap_or_default();
    assert!(variants_json.contains("360.mp4"));
    assert!(variants_json.contains("720.mp4"));
    assert_eq!(media[0].duration_ms, Some(42_000));
    assert_eq!(
        media[0].poster_url.as_deref(),
        Some("https://pbs.twimg.com/media/poster.jpg")
    );
    Ok(())
}

#[test]
fn upgrades_previous_schema_and_keeps_fixture_parity() -> crate::Result<()> {
    // A database created by the previous schema (user_version 0, no quote
    // table, no duration/poster columns) must upgrade in place: existing rows
    // survive, the new display columns appear, and the import fixture still
    // yields the locked 3-bookmark parity on top of the old row.
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("upgrade-fixture.db");
    create_previous_schema_database(&db_path)?;

    let db = Database::open(&db_path)?;

    assert_eq!(current_user_version(&db.conn)?, 2);
    assert_table_exists(&db.conn, "quoted_tweets")?;
    assert_media_metadata_columns_exist(&db.conn)?;
    assert_index_exists(&db.conn, "idx_quoted_tweets_tweet_url")?;

    let existing: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM bookmarks", [], |row| row.get(0))?;
    assert_eq!(existing, 1, "pre-upgrade bookmark must survive");

    let bookmarks = crate::ingestion::Ingester::new().parse_content("json", SAMPLE_JSON)?;
    assert_eq!(bookmarks.len(), 3);
    assert_eq!(db.insert_bookmarks(&bookmarks)?, 3);

    let stats = db.get_stats()?;
    assert_eq!(stats.total_bookmarks, 4);
    assert_eq!(stats.unique_authors, 4);
    drop(db);

    let reopened = Database::open(&db_path)?;
    assert_eq!(current_user_version(&reopened.conn)?, 2);
    assert_eq!(reopened.count_bookmarks()?, 4);
    let quotes: i64 = reopened
        .conn
        .query_row("SELECT COUNT(*) FROM quoted_tweets", [], |row| row.get(0))?;
    assert_eq!(quotes, 0, "fixture bookmarks do not quote posts");
    Ok(())
}

#[test]
fn preserves_quoted_tweet_and_quote_media_across_reopen_and_delete() -> crate::Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("quotes.db");
    let db = Database::open(&db_path)?;

    let quoted_media = crate::models::Media::new(
        "https://video.twimg.com/quoted/clip.mp4",
        crate::models::MediaType::Video,
    )
    .with_alt_text(Some("quoted clip".to_string()))
    .with_display_metadata(
        Some(8_000),
        Some("https://pbs.twimg.com/quoted/poster.jpg".to_string()),
    );
    let quote = crate::models::QuotedTweet::new(
        "https://x.com/quoted_author/status/9001",
        "quoted body",
        Some(chrono::Utc::now()),
        "quoted_author",
        "Quoted Author",
    )
    .with_author_profile_image("https://example.com/quoted.png")
    .with_media(vec![quoted_media]);
    let bookmark = crate::models::BookmarkBuilder::new()
        .tweet_url("https://x.com/ada/status/1")
        .content("bookmark that quotes a post")
        .tweeted_at(chrono::Utc::now())
        .author_handle("ada")
        .author_name("Ada")
        .quoted(quote)
        .build()
        .map_err(|error| crate::Error::Other(error.to_string()))?;
    let bookmark_id = bookmark.id.clone();

    db.insert_bookmarks(&[bookmark])?;
    drop(db);

    let reopened = Database::open(&db_path)?;
    let loaded = reopened
        .get_bookmark(&bookmark_id)?
        .expect("bookmark should survive reopen");
    let quote = loaded.quoted.as_ref().expect("quote should hydrate");
    assert!(!quote.unavailable);
    assert_eq!(quote.content, "quoted body");
    assert_eq!(quote.author_handle, "quoted_author");
    assert_eq!(
        quote.author_profile_image.as_deref(),
        Some("https://example.com/quoted.png")
    );
    assert_eq!(quote.media.len(), 1);
    assert_eq!(quote.media[0].duration_ms, Some(8_000));
    assert_eq!(
        quote.media[0].poster_url.as_deref(),
        Some("https://pbs.twimg.com/quoted/poster.jpg")
    );
    assert!(
        loaded.media.is_empty(),
        "quote media must not join the bookmark media table"
    );

    // The batch feed hydration path carries the same quote.
    let page = reopened.get_bookmarks(0, 10)?;
    assert_eq!(page.len(), 1);
    assert!(page[0].quoted.is_some());

    // Deleting the bookmark removes the display-only quote row.
    assert!(reopened.delete_bookmark(&bookmark_id)?);
    let quote_rows: i64 = reopened.conn.query_row(
        "SELECT COUNT(*) FROM quoted_tweets WHERE bookmark_id = ?1",
        params![bookmark_id],
        |row| row.get(0),
    )?;
    assert_eq!(quote_rows, 0);
    Ok(())
}

#[test]
fn repairs_quoted_tweet_table_from_an_earlier_draft_shape() -> crate::Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("draft-quotes.db");
    let conn = Connection::open(&db_path)?;
    create_legacy_schema(&conn)?;
    conn.execute_batch(
        "CREATE TABLE quoted_tweets (bookmark_id TEXT PRIMARY KEY, tweet_url TEXT NOT NULL);",
    )?;
    drop(conn);

    let db = Database::open(&db_path)?;

    for column_name in [
        "tweet_url",
        "content",
        "tweeted_at",
        "author_handle",
        "author_name",
        "author_profile_image",
        "media_json",
        "unavailable",
    ] {
        assert_table_column_exists(&db.conn, "quoted_tweets", column_name)?;
    }
    assert_eq!(current_user_version(&db.conn)?, 2);
    Ok(())
}

#[test]
fn file_backed_database_uses_wal_journal_mode() -> crate::Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("wal-proof.db");
    let db = Database::open(&db_path)?;
    let journal_mode: String = db
        .conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    assert_eq!(journal_mode, "wal");
    assert!(std::path::PathBuf::from(format!("{}-wal", db_path.display())).exists());
    Ok(())
}

fn create_legacy_database(path: &Path) -> crate::Result<()> {
    let conn = Connection::open(path)?;
    create_legacy_schema(&conn)?;
    insert_legacy_bookmark(&conn)?;
    conn.execute(
        "INSERT INTO media (bookmark_id, url, media_type) VALUES (?1, ?2, 'image')",
        params!["legacy-1", "https://pbs.twimg.com/media/legacy.jpg"],
    )?;
    Ok(())
}

fn create_future_version_database(path: &Path) -> crate::Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA user_version = 3;")?;
    create_legacy_schema(&conn)?;
    insert_legacy_bookmark(&conn)
}

/// Recreate the shipped v1 schema (user_version 1) for the v1 -> v2 upgrade.
fn create_v1_database(path: &Path) -> crate::Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        r#"
        PRAGMA user_version = 1;
        CREATE TABLE bookmarks (
            id TEXT PRIMARY KEY,
            tweet_url TEXT UNIQUE NOT NULL,
            content TEXT NOT NULL,
            note_text TEXT,
            tweeted_at INTEGER NOT NULL,
            imported_at INTEGER NOT NULL,
            author_handle TEXT NOT NULL,
            author_name TEXT NOT NULL,
            author_profile_url TEXT,
            author_profile_image TEXT,
            comments TEXT,
            is_favorite INTEGER DEFAULT 0,
            has_media INTEGER DEFAULT 0
        );
        CREATE TABLE author_stats (
            author_handle TEXT PRIMARY KEY,
            author_name TEXT NOT NULL,
            author_profile_image TEXT,
            bookmark_count INTEGER NOT NULL DEFAULT 0,
            favorite_count INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE tags (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT UNIQUE NOT NULL COLLATE NOCASE
        );
        CREATE TABLE bookmark_tags (
            bookmark_id TEXT NOT NULL,
            tag_id INTEGER NOT NULL,
            PRIMARY KEY (bookmark_id, tag_id)
        );
        CREATE TABLE media (
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
            duration_ms INTEGER,
            poster_url TEXT
        );
        CREATE TABLE quoted_tweets (
            bookmark_id TEXT PRIMARY KEY,
            tweet_url TEXT NOT NULL,
            content TEXT NOT NULL DEFAULT '',
            tweeted_at INTEGER,
            author_handle TEXT NOT NULL DEFAULT '',
            author_name TEXT NOT NULL DEFAULT '',
            author_profile_image TEXT,
            media_json TEXT NOT NULL DEFAULT '[]',
            unavailable INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE app_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE bookmarks_fts_content (
            rowid INTEGER PRIMARY KEY,
            bookmark_id TEXT NOT NULL,
            content TEXT,
            note_text TEXT,
            author_handle TEXT,
            author_name TEXT,
            tags_text TEXT
        );
        CREATE VIRTUAL TABLE bookmarks_fts USING fts5(
            content, note_text, author_handle, author_name, tags_text,
            content='bookmarks_fts_content', content_rowid='rowid',
            tokenize='porter unicode61'
        );
        CREATE INDEX idx_media_bookmark ON media(bookmark_id);
        CREATE INDEX idx_quoted_tweets_tweet_url ON quoted_tweets(tweet_url);
        CREATE INDEX idx_quoted_tweets_author_handle ON quoted_tweets(author_handle);
        CREATE INDEX idx_fts_content_bookmark ON bookmarks_fts_content(bookmark_id);
        "#,
    )?;

    conn.execute(
        r#"INSERT INTO bookmarks
           (id, tweet_url, content, tweeted_at, imported_at, author_handle, author_name,
            is_favorite, has_media)
           VALUES ('v1-1', 'https://x.com/v1/status/1', 'v1 row', 1717243200, 1717243200,
                   'v1', 'V1 Author', 1, 1)"#,
        [],
    )?;
    conn.execute(
        r#"INSERT INTO media
           (bookmark_id, url, media_type, variants_json, duration_ms)
           VALUES ('v1-1', 'https://video.twimg.com/v1/poster.jpg', 'video', ?1, 9000)"#,
        params![r#"[{"url":"https://video.twimg.com/v1/hls.m3u8","content_type":"application/x-mpegURL","bit_rate":9999999},{"url":"https://video.twimg.com/v1/720.mp4","content_type":"video/mp4","bit_rate":2176000}]"#],
    )?;
    conn.execute(
        "INSERT INTO bookmarks_fts_content (bookmark_id, content, author_handle, author_name) VALUES ('v1-1', 'v1 row', 'v1', 'V1 Author')",
        [],
    )?;
    Ok(())
}

/// Recreate the schema shape shipped before quote support was added.
///
/// It is deliberately hand-written instead of generated from `SCHEMA`, so the
/// upgrade test keeps testing the migration path even after `SCHEMA` changes.
fn create_previous_schema_database(path: &Path) -> crate::Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        r#"
        CREATE TABLE bookmarks (
            id TEXT PRIMARY KEY,
            tweet_url TEXT UNIQUE NOT NULL,
            content TEXT NOT NULL,
            note_text TEXT,
            tweeted_at INTEGER NOT NULL,
            imported_at INTEGER NOT NULL,
            author_handle TEXT NOT NULL,
            author_name TEXT NOT NULL,
            author_profile_url TEXT,
            author_profile_image TEXT,
            comments TEXT,
            is_favorite INTEGER DEFAULT 0,
            has_media INTEGER DEFAULT 0
        );
        CREATE TABLE author_stats (
            author_handle TEXT PRIMARY KEY,
            author_name TEXT NOT NULL,
            author_profile_image TEXT,
            bookmark_count INTEGER NOT NULL DEFAULT 0,
            favorite_count INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE tags (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT UNIQUE NOT NULL COLLATE NOCASE
        );
        CREATE TABLE bookmark_tags (
            bookmark_id TEXT NOT NULL,
            tag_id INTEGER NOT NULL,
            PRIMARY KEY (bookmark_id, tag_id)
        );
        CREATE TABLE media (
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
            variants_json TEXT
        );
        CREATE TABLE app_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE bookmarks_fts_content (
            rowid INTEGER PRIMARY KEY,
            bookmark_id TEXT NOT NULL,
            content TEXT,
            note_text TEXT,
            author_handle TEXT,
            author_name TEXT,
            tags_text TEXT
        );
        CREATE VIRTUAL TABLE bookmarks_fts USING fts5(
            content,
            note_text,
            author_handle,
            author_name,
            tags_text,
            content='bookmarks_fts_content',
            content_rowid='rowid',
            tokenize='porter unicode61'
        );
        CREATE INDEX idx_media_bookmark ON media(bookmark_id);
        "#,
    )?;

    conn.execute(
        r#"INSERT INTO bookmarks
           (id, tweet_url, content, note_text, tweeted_at, imported_at,
            author_handle, author_name, author_profile_url, author_profile_image,
            comments, is_favorite, has_media)
           VALUES ('previous-1', 'https://x.com/previous/status/1', 'previous schema row',
                   NULL, 1717243200, 1717243200, 'previous', 'Previous Author',
                   NULL, NULL, NULL, 0, 1)"#,
        [],
    )?;
    conn.execute(
        "INSERT INTO media (bookmark_id, url, media_type, width, height) VALUES (?1, ?2, 'image', 640, 480)",
        params!["previous-1", "https://pbs.twimg.com/media/previous.jpg"],
    )?;
    Ok(())
}

fn current_user_version(conn: &Connection) -> crate::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(crate::Error::from)
}

fn assert_index_exists(conn: &Connection, index_name: &str) -> crate::Result<()> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
        [index_name],
        |row| row.get(0),
    )?;
    assert_eq!(exists, 1, "missing index {index_name}");
    Ok(())
}

fn create_bad_media_shape_database(path: &Path) -> crate::Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "CREATE TABLE bookmarks (id TEXT PRIMARY KEY, tweet_url TEXT UNIQUE NOT NULL, content TEXT NOT NULL, tweeted_at INTEGER NOT NULL, imported_at INTEGER NOT NULL, author_handle TEXT NOT NULL, author_name TEXT NOT NULL);
         CREATE TABLE media (id INTEGER PRIMARY KEY AUTOINCREMENT, url TEXT NOT NULL, media_type TEXT NOT NULL);",
    )?;
    conn.execute(
        r#"INSERT INTO bookmarks
           (id, tweet_url, content, tweeted_at, imported_at, author_handle, author_name)
           VALUES ('legacy-1', 'https://x.com/old_author/status/1', 'legacy row',
                   1717243200, 1717243200, 'old_author', 'Old Author')"#,
        [],
    )?;
    Ok(())
}

fn create_stale_has_media_database(path: &Path) -> crate::Result<()> {
    let conn = Connection::open(path)?;
    create_legacy_schema(&conn)?;
    conn.execute(
        "ALTER TABLE bookmarks ADD COLUMN has_media INTEGER DEFAULT 0",
        [],
    )?;
    insert_legacy_bookmark(&conn)?;
    conn.execute(
        "INSERT INTO media (bookmark_id, url, media_type) VALUES (?1, ?2, 'image')",
        params!["legacy-1", "https://pbs.twimg.com/media/legacy.jpg"],
    )?;
    Ok(())
}

fn create_legacy_schema(conn: &Connection) -> crate::Result<()> {
    conn.execute_batch(
        "CREATE TABLE bookmarks (id TEXT PRIMARY KEY, tweet_url TEXT UNIQUE NOT NULL, content TEXT NOT NULL, note_text TEXT, tweeted_at INTEGER NOT NULL, imported_at INTEGER NOT NULL, author_handle TEXT NOT NULL, author_name TEXT NOT NULL, author_profile_url TEXT, author_profile_image TEXT, comments TEXT);
         CREATE TABLE media (id INTEGER PRIMARY KEY AUTOINCREMENT, bookmark_id TEXT NOT NULL, url TEXT NOT NULL, media_type TEXT NOT NULL);",
    )
    .map_err(crate::Error::from)
}

fn insert_legacy_bookmark(conn: &Connection) -> crate::Result<()> {
    conn.execute(
        r#"INSERT INTO bookmarks
           (id, tweet_url, content, note_text, tweeted_at, imported_at,
            author_handle, author_name, author_profile_url, author_profile_image, comments)
           VALUES ('legacy-1', 'https://x.com/old_author/status/1', 'legacy row',
                   NULL, 1717243200, 1717243200, 'old_author', 'Old Author',
                   NULL, NULL, NULL)"#,
        [],
    )?;
    Ok(())
}

fn assert_column_exists(conn: &Connection, column_name: &str) -> crate::Result<()> {
    assert_table_column_exists(conn, "bookmarks", column_name)
}

fn assert_table_column_exists(
    conn: &Connection,
    table_name: &str,
    column_name: &str,
) -> crate::Result<()> {
    let pragma = format!("PRAGMA table_info({table_name})");
    let mut stmt = conn.prepare(&pragma)?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let current_name: String = row.get(1)?;
        if current_name.eq_ignore_ascii_case(column_name) {
            return Ok(());
        }
    }
    Err(crate::Error::Other(format!(
        "missing migrated column {table_name}.{column_name}"
    )))
}

fn assert_media_metadata_columns_exist(conn: &Connection) -> crate::Result<()> {
    for column_name in [
        "alt_text",
        "width",
        "height",
        "source_media_key",
        "source_type",
        "preview_url",
        "variant_url",
        "variants_json",
        "duration_ms",
        "poster_url",
    ] {
        assert_table_column_exists(conn, "media", column_name)?;
    }
    Ok(())
}

fn assert_column_absent(conn: &Connection, column_name: &str) -> crate::Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(bookmarks)")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let current_name: String = row.get(1)?;
        if current_name.eq_ignore_ascii_case(column_name) {
            return Err(crate::Error::Other(format!(
                "unexpected migrated column {column_name}"
            )));
        }
    }
    Ok(())
}

fn assert_table_exists(conn: &Connection, table_name: &str) -> crate::Result<()> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table_name],
        |row| row.get(0),
    )?;
    assert_eq!(exists, 1);
    Ok(())
}
