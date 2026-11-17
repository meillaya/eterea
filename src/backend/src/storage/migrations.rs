//! Idempotent SQLite schema initialization and data migrations.

use super::database::{Database, AUTHOR_STATS_METADATA_KEY};
use super::schema::{PRAGMAS, SCHEMA};
use crate::{Error, Result};
use rusqlite::types::ValueRef;
use tracing::debug;

const CURRENT_SCHEMA_USER_VERSION: i64 = 2;

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
