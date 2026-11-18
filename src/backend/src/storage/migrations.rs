//! Idempotent SQLite schema initialization and data migrations.

use super::database::{Database, AUTHOR_STATS_METADATA_KEY};
use super::schema::{PRAGMAS, SCHEMA};
use crate::{Error, Result};
use rusqlite::types::ValueRef;
use tracing::debug;

pub const CURRENT_SCHEMA_USER_VERSION: i64 = 5;

/// Pre-v3 FTS external-content document table. The name ended with `_content`,
/// which is an FTS5 shadow-table suffix for the `bookmarks_fts` virtual table;
/// SQLite releases after the bundled one refuse to create triggers on shadow
/// tables, so such a database cannot be opened by newer clients. Schema v3
/// renames it to `bookmarks_search_docs` (see `migrate_fts_shadow_table`).
const LEGACY_FTS_CONTENT_TABLE: &str = "bookmarks_fts_content";

const BOOKMARK_V2_COLUMNS: &[(&str, &str)] = &[
    ("is_read", "INTEGER NOT NULL DEFAULT 0"),
    ("links_json", "TEXT NOT NULL DEFAULT '[]'"),
];

const MEDIA_METADATA_COLUMNS: &[(&str, &str)] = &[
    ("alt_text", "TEXT"),
    ("width", "INTEGER"),
    ("height", "INTEGER"),
    ("source_media_key", "TEXT"),
    ("source_type", "TEXT"),
    ("preview_url", "TEXT"),
    ("variant_url", "TEXT"),
    ("variants_json", "TEXT"),
    ("duration_ms", "INTEGER"),
    ("poster_url", "TEXT"),
    ("playback_url", "TEXT"),
];

impl Database {
    pub(crate) fn initialize(&self) -> Result<()> {
        // Set performance pragmas. These are connection settings, not schema
        // changes, so they run outside the migration transaction.
        self.conn.execute_batch(PRAGMAS)?;

        self.ensure_supported_user_version()?;
        self.ensure_supported_legacy_shape()?;

        // Run every schema/data migration in one transaction: a failed
        // migration must leave the previous database byte-identical instead of
        // claiming a newer user_version with a half-applied shape.
        self.conn.execute("BEGIN IMMEDIATE", [])?;
        let migrated = (|| -> Result<()> {
            self.ensure_legacy_bookmark_columns()?;
            self.ensure_bookmark_v2_columns()?;

            // Repair any earlier development quote-table shape before `SCHEMA`
            // creates the quote indexes that reference those columns.
            self.ensure_quoted_tweets_table()?;

            // Rename the legacy shadow-named FTS document table before `SCHEMA`
            // runs: `CREATE VIRTUAL TABLE IF NOT EXISTS bookmarks_fts` would
            // keep the old table when the virtual table still exists.
            let fts_documents_migrated = self.migrate_fts_shadow_table()?;

            // Schema v5: index the quoted post, media alt text and link targets
            // alongside the body. This rebuilds the FTS table on v4 archives;
            // it runs before SCHEMA so the triggers are recreated from the new
            // definitions.
            let reshaped = self.ensure_search_doc_text_coverage()?;
            if fts_documents_migrated && !reshaped {
                // A shadow-table migration rebuilt the document rows but found
                // the new shape already present.
                self.rebuild_search_index()?;
            }

            // Create schema (fresh tables, indexes, triggers).
            self.conn.execute_batch(SCHEMA)?;

            self.ensure_is_favorite_column()?;
            self.ensure_media_metadata_columns()?;
            self.ensure_has_media_column()?;
            self.ensure_author_stats_snapshot()?;
            self.backfill_playback_urls()?;
            self.stamp_schema_version()?;
            Ok(())
        })();

        match migrated {
            Ok(()) => {
                self.conn.execute("COMMIT", [])?;
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", []);
                return Err(error);
            }
        }

        self.conn.execute_batch("PRAGMA optimize;")?;
        debug!("Database initialized");
        Ok(())
    }

    pub fn observed_pragma_settings(&self) -> Result<Vec<(String, String)>> {
        [
            "journal_mode",
            "synchronous",
            "cache_size",
            "temp_store",
            "mmap_size",
            "foreign_keys",
        ]
        .into_iter()
        .map(|name| Ok((name.to_string(), self.pragma_value(name)?)))
        .collect()
    }

    /// Schema v5: FTS coverage for quoted text, media alt text and link targets.
    ///
    /// Adds the three columns to the document table, recreates the FTS virtual
    /// table with the new shape, backfills every existing document from its
    /// tables and rebuilds the postings. Runs before `SCHEMA`, which recreates
    /// the triggers against the new shape.
    fn ensure_search_doc_text_coverage(&self) -> Result<bool> {
        if !self.table_exists("bookmarks_search_docs")? {
            return Ok(false);
        }
        let docs_have = self.table_column_exists("bookmarks_search_docs", "quote_text")?;
        let fts_have = self.table_column_exists("bookmarks_fts", "quote_text")?;
        if docs_have && fts_have {
            return Ok(false);
        }
        debug!("migrating FTS documents to cover quote, alt and link text");
        if !self.table_column_exists("bookmarks_search_docs", "quote_text")? {
            self.conn.execute_batch(
                r#"ALTER TABLE bookmarks_search_docs ADD COLUMN quote_text TEXT;
                   ALTER TABLE bookmarks_search_docs ADD COLUMN alt_text TEXT;
                   ALTER TABLE bookmarks_search_docs ADD COLUMN links_text TEXT;"#,
            )?;
        }
        self.conn.execute_batch(
            r#"DROP TRIGGER IF EXISTS bookmarks_fts_insert;
               DROP TRIGGER IF EXISTS bookmarks_fts_delete;
               DROP TRIGGER IF EXISTS bookmarks_fts_update;
               DROP TABLE IF EXISTS bookmarks_fts;

               CREATE VIRTUAL TABLE bookmarks_fts USING fts5(
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
               );"#,
        )?;
        // Backfill while no triggers exist: an FTS5 external-content delete
        // against a freshly created empty table is invalid, so the rebuild
        // happens after the data is in place and `SCHEMA` adds the triggers.
        let quote_text = if self.table_exists("quoted_tweets")? {
            "COALESCE((SELECT content || ' ' || author_name || ' ' || author_handle
                       FROM quoted_tweets
                       WHERE bookmark_id = bookmarks_search_docs.bookmark_id), '')"
        } else {
            "''"
        };
        let alt_text = if self.table_exists("media")? {
            "COALESCE((SELECT group_concat(alt_text, ' ') FROM media
                       WHERE bookmark_id = bookmarks_search_docs.bookmark_id
                         AND alt_text IS NOT NULL AND alt_text <> ''), '')"
        } else {
            "''"
        };
        let links_text = if self.table_column_exists("bookmarks", "links_json")? {
            "COALESCE((SELECT group_concat(target, ' ') FROM (
                           SELECT json_extract(value, '$.expanded_url') AS target
                           FROM json_each((SELECT links_json FROM bookmarks
                                           WHERE id = bookmarks_search_docs.bookmark_id))
                           UNION
                           SELECT json_extract(value, '$.url')
                           FROM json_each((SELECT links_json FROM bookmarks
                                           WHERE id = bookmarks_search_docs.bookmark_id))
                       ) WHERE target IS NOT NULL AND target <> ''), '')"
        } else {
            "''"
        };
        self.conn.execute_batch(&format!(
            "UPDATE bookmarks_search_docs SET quote_text = {quote_text}, \
             alt_text = {alt_text}, links_text = {links_text};"
        ))?;
        self.rebuild_search_index()?;
        Ok(true)
    }

    fn ensure_supported_user_version(&self) -> Result<()> {
        let user_version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if user_version > CURRENT_SCHEMA_USER_VERSION {
            return Err(Error::Other(format!(
                "database user_version {user_version} is newer than supported {CURRENT_SCHEMA_USER_VERSION}"
            )));
        }
        Ok(())
    }

    fn ensure_supported_legacy_shape(&self) -> Result<()> {
        if self.table_exists("media")? && !self.table_column_exists("media", "bookmark_id")? {
            return Err(Error::Other(
                "unsupported legacy media table shape: missing bookmark_id".to_string(),
            ));
        }
        Ok(())
    }

    fn ensure_legacy_bookmark_columns(&self) -> Result<()> {
        if !self.table_exists("bookmarks")? {
            return Ok(());
        }

        if !self.bookmarks_column_exists("is_favorite")? {
            self.conn.execute(
                "ALTER TABLE bookmarks ADD COLUMN is_favorite INTEGER DEFAULT 0",
                [],
            )?;
        }

        Ok(())
    }

    /// Add the schema-v2 reader/links columns to a v1 database.
    ///
    /// Runs before `SCHEMA` because the unread partial index in `SCHEMA`
    /// references `is_read`. Existing rows read as unread with no links.
    fn ensure_bookmark_v2_columns(&self) -> Result<()> {
        if !self.table_exists("bookmarks")? {
            return Ok(());
        }

        for (column_name, column_definition) in BOOKMARK_V2_COLUMNS {
            if !self.bookmarks_column_exists(column_name)? {
                let statement =
                    format!("ALTER TABLE bookmarks ADD COLUMN {column_name} {column_definition}");
                self.conn.execute(&statement, [])?;
            }
        }

        Ok(())
    }

    fn table_exists(&self, table_name: &str) -> Result<bool> {
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table_name],
            |row| row.get(0),
        )?;
        Ok(exists > 0)
    }

    fn bookmarks_column_exists(&self, column_name: &str) -> Result<bool> {
        self.table_column_exists("bookmarks", column_name)
    }

    fn table_column_exists(&self, table_name: &str, column_name: &str) -> Result<bool> {
        let pragma = format!("PRAGMA table_info({table_name})");
        let mut stmt = self.conn.prepare(&pragma)?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            if name.eq_ignore_ascii_case(column_name) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn pragma_value(&self, name: &str) -> Result<String> {
        let pragma = format!("PRAGMA {name}");
        self.conn
            .query_row(&pragma, [], |row| {
                let value = row.get_ref(0)?;
                let rendered = match value {
                    ValueRef::Null => "null".to_string(),
                    ValueRef::Integer(value) => value.to_string(),
                    ValueRef::Real(value) => value.to_string(),
                    ValueRef::Text(value) => String::from_utf8_lossy(value).into_owned(),
                    ValueRef::Blob(value) => format!("<{} byte blob>", value.len()),
                };
                Ok(rendered)
            })
            .map_err(Error::from)
    }

    fn ensure_author_stats_snapshot(&self) -> Result<()> {
        if self.get_metadata(AUTHOR_STATS_METADATA_KEY)?.as_deref() == Some("ready") {
            return Ok(());
        }

        self.rebuild_author_stats_snapshot()?;
        self.set_metadata(AUTHOR_STATS_METADATA_KEY, "ready")?;
        Ok(())
    }

    fn rebuild_author_stats_snapshot(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
DELETE FROM author_stats;

INSERT INTO author_stats (
    author_handle,
    author_name,
    author_profile_image,
    bookmark_count,
    favorite_count
)
SELECT author_handle,
       COALESCE(NULLIF(MAX(author_name), ''), author_handle) AS author_name,
       MAX(NULLIF(author_profile_image, '')) AS author_profile_image,
       COUNT(*) AS bookmark_count,
       SUM(CASE WHEN is_favorite = 1 THEN 1 ELSE 0 END) AS favorite_count
FROM bookmarks
GROUP BY author_handle;
"#,
        )?;

        Ok(())
    }

    fn ensure_media_metadata_columns(&self) -> Result<()> {
        if !self.table_exists("media")? {
            return Ok(());
        }

        for (column_name, column_type) in MEDIA_METADATA_COLUMNS {
            if !self.table_column_exists("media", column_name)? {
                let statement = format!("ALTER TABLE media ADD COLUMN {column_name} {column_type}");
                self.conn.execute(&statement, [])?;
            }
        }

        Ok(())
    }

    /// Backfill `media.playback_url` for rows written before the column
    /// existed: choose the best progressive MP4 from `variants_json`, falling
    /// back to a legacy `.mp4` `variant_url`. Idempotent because it only
    /// touches NULL playback URLs.
    fn backfill_playback_urls(&self) -> Result<()> {
        if !self.table_exists("media")? {
            return Ok(());
        }

        let rows: Vec<(i64, Option<String>, Option<String>)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, variants_json, variant_url FROM media WHERE playback_url IS NULL",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };

        if rows.is_empty() {
            return Ok(());
        }

        let mut update = self
            .conn
            .prepare("UPDATE media SET playback_url = ?1 WHERE id = ?2")?;
        for (id, variants_json, variant_url) in rows {
            let playback_url = crate::models::playback_url_from_variants_json(
                variants_json.as_deref(),
            )
            .or_else(|| {
                variant_url.filter(|url| {
                    let lower = url.to_ascii_lowercase();
                    lower.ends_with(".mp4") && !lower.contains(".m3u8")
                })
            });
            if let Some(playback_url) = playback_url {
                update.execute(rusqlite::params![playback_url, id])?;
            }
        }

        Ok(())
    }

    /// Bring the display-only quote table up to the current shape.
    ///
    /// The base `SCHEMA` batch already creates the full table on fresh and
    /// migrated databases; this keeps databases written by earlier development
    /// builds (quotes existed, display columns did not) repairable instead of
    /// failing on hydration.
    fn ensure_quoted_tweets_table(&self) -> Result<()> {
        if !self.table_exists("quoted_tweets")? {
            return Ok(());
        }

        if !self.table_column_exists("quoted_tweets", "bookmark_id")? {
            return Err(Error::Other(
                "unsupported quoted_tweets table shape: missing bookmark_id".to_string(),
            ));
        }

        for (column_name, column_definition) in [
            ("content", "TEXT NOT NULL DEFAULT ''"),
            ("tweeted_at", "INTEGER"),
            ("author_handle", "TEXT NOT NULL DEFAULT ''"),
            ("author_name", "TEXT NOT NULL DEFAULT ''"),
            ("author_profile_image", "TEXT"),
            ("media_json", "TEXT NOT NULL DEFAULT '[]'"),
            ("unavailable", "INTEGER NOT NULL DEFAULT 0"),
        ] {
            if !self.table_column_exists("quoted_tweets", column_name)? {
                let statement = format!(
                    "ALTER TABLE quoted_tweets ADD COLUMN {column_name} {column_definition}"
                );
                self.conn.execute(&statement, [])?;
            }
        }

        Ok(())
    }

    /// Rename the pre-v3 FTS external-content document table out of the FTS5
    /// shadow-table namespace and drop the old virtual table/triggers.
    ///
    /// `bookmarks_fts_content` matched the shadow-table name pattern of the
    /// `bookmarks_fts` virtual table. SQLite allows creating triggers on shadow
    /// tables only in older releases, so any newer client rejects the whole
    /// database with "cannot create triggers on shadow tables". This step is
    /// safe to attempt on every open: it returns `false` (no-op) once the
    /// legacy table is gone, so reopening a migrated database performs no work.
    ///
    /// Data handling: the rows are copied with their `rowid` values first, so
    /// the follow-up [`Self::rebuild_search_index`] rebuilds the same postings.
    /// FTS5 only drops the `<vtab>_content` table when it owns normalized
    /// content; this virtual table is external-content, so the legacy document
    /// table is dropped explicitly after the virtual table.
    fn migrate_fts_shadow_table(&self) -> Result<bool> {
        if !self.table_exists(LEGACY_FTS_CONTENT_TABLE)? {
            return Ok(false);
        }

        debug!("migrating legacy FTS document table out of the shadow namespace");
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS bookmarks_search_docs (
                rowid INTEGER PRIMARY KEY,
                bookmark_id TEXT NOT NULL,
                content TEXT,
                note_text TEXT,
                author_handle TEXT,
                author_name TEXT,
                tags_text TEXT,
                FOREIGN KEY (bookmark_id) REFERENCES bookmarks(id) ON DELETE CASCADE
            );
            "#,
        )?;
        self.conn.execute(
            r#"INSERT OR IGNORE INTO bookmarks_search_docs
                   (rowid, bookmark_id, content, note_text, author_handle, author_name, tags_text)
               SELECT rowid, bookmark_id, content, note_text, author_handle, author_name, tags_text
               FROM bookmarks_fts_content"#,
            [],
        )?;
        self.conn.execute_batch(
            r#"
            DROP TRIGGER IF EXISTS bookmarks_fts_insert;
            DROP TRIGGER IF EXISTS bookmarks_fts_delete;
            DROP TRIGGER IF EXISTS bookmarks_fts_update;
            DROP TABLE IF EXISTS bookmarks_fts;
            DROP TABLE IF EXISTS bookmarks_fts_content;
            DROP INDEX IF EXISTS idx_fts_content_bookmark;
            "#,
        )?;
        Ok(true)
    }

    /// Rebuild the FTS5 index from the external-content document table.
    ///
    /// Runs inside the caller-owned migration transaction. The FTS5 'rebuild'
    /// command walks `bookmarks_search_docs` by `rowid` and recreates every
    /// posting, which is how a migrated database keeps matching old searches.
    fn rebuild_search_index(&self) -> Result<()> {
        self.conn.execute(
            "INSERT INTO bookmarks_fts(bookmarks_fts) VALUES('rebuild')",
            [],
        )?;
        Ok(())
    }

    /// Stamp the schema version after every migration step has succeeded.
    ///
    /// A fresh database and a database created by the previous schema both
    /// start at `user_version = 0`; both end at [`CURRENT_SCHEMA_USER_VERSION`].
    /// Reopening a stamped database is a no-op. The stamp lands last, so a
    /// failed migration never claims a newer schema.
    fn stamp_schema_version(&self) -> Result<()> {
        let user_version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if user_version < CURRENT_SCHEMA_USER_VERSION {
            self.conn.execute_batch(&format!(
                "PRAGMA user_version = {CURRENT_SCHEMA_USER_VERSION};"
            ))?;
        }
        Ok(())
    }

    fn ensure_has_media_column(&self) -> Result<()> {
        let has_column = self.bookmarks_column_exists("has_media")?;

        // Runs inside the caller-owned migration transaction.
        if !has_column {
            self.conn.execute(
                "ALTER TABLE bookmarks ADD COLUMN has_media INTEGER DEFAULT 0",
                [],
            )?;
        }
        self.conn.execute(
            "UPDATE bookmarks SET has_media = (SELECT CASE WHEN COUNT(*) > 0 THEN 1 ELSE 0 END FROM media WHERE bookmark_id = bookmarks.id)",
            [],
        )?;

        // These DDL statements depend on has_media existing, so run them after the migration above.
        self.conn.execute_batch(
            r#"
CREATE INDEX IF NOT EXISTS idx_bookmarks_has_media ON bookmarks(has_media) WHERE has_media = 1;

CREATE TRIGGER IF NOT EXISTS media_insert_set_has_media AFTER INSERT ON media BEGIN
    UPDATE bookmarks SET has_media = 1 WHERE id = NEW.bookmark_id;
END;

CREATE TRIGGER IF NOT EXISTS media_delete_update_has_media AFTER DELETE ON media BEGIN
    UPDATE bookmarks SET has_media = (
        SELECT CASE WHEN COUNT(*) > 0 THEN 1 ELSE 0 END FROM media WHERE bookmark_id = OLD.bookmark_id
    ) WHERE id = OLD.bookmark_id;
END;
"#,
        )?;

        Ok(())
    }

    fn ensure_is_favorite_column(&self) -> Result<()> {
        let mut stmt = self.conn.prepare("PRAGMA table_info(bookmarks)")?;
        let mut rows = stmt.query([])?;
        let mut has_column = false;
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            if name.eq_ignore_ascii_case("is_favorite") {
                has_column = true;
                break;
            }
        }

        if !has_column {
            self.conn.execute(
                "ALTER TABLE bookmarks ADD COLUMN is_favorite INTEGER DEFAULT 0",
                [],
            )?;
        }

        Ok(())
    }
}

#[cfg(test)]
#[path = "migrations_tests.rs"]
mod tests;
