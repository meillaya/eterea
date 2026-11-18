//! Bookmark write and mutation persistence.
//!
//! Every write path that can be undone runs in one transaction that also
//! journals its history event: imports (`INSERT OR IGNORE` + `changes()` for
//! duplicate accounting), single/bulk deletes, and tag edits. Tag, FTS content,
//! and media rows are written per inserted bookmark; the aggregate stats
//! snapshot is refreshed once per write batch.

use super::database::Database;
use crate::models::{Bookmark, HistorySummary};
use crate::{Error, Result};
use rusqlite::{params, CachedStatement, Connection, OptionalExtension};
use std::collections::HashMap;

/// Result of one file inside a batch import.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct FileImportOutcome {
    pub source_label: String,
    pub parsed: usize,
    pub inserted: usize,
    pub duplicates: usize,
}

/// Result of importing one or more files in one transaction.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BatchImportOutcome {
    pub inserted: usize,
    pub duplicates: usize,
    pub files: Vec<FileImportOutcome>,
    pub history_id: Option<i64>,
}

struct BookmarkInsertStatements<'conn> {
    bookmark: CachedStatement<'conn>,
    tag: CachedStatement<'conn>,
    tag_id: CachedStatement<'conn>,
    bookmark_tag: CachedStatement<'conn>,
    media: CachedStatement<'conn>,
    quoted_tweet: CachedStatement<'conn>,
    fts_content: CachedStatement<'conn>,
    tag_ids: HashMap<String, i64>,
}

impl<'conn> BookmarkInsertStatements<'conn> {
    fn prepare(conn: &'conn Connection) -> Result<Self> {
        Ok(Self {
            bookmark: conn.prepare_cached(
                r#"INSERT OR IGNORE INTO bookmarks
                   (id, tweet_url, content, note_text, tweeted_at, imported_at,
                    author_handle, author_name, author_profile_url, author_profile_image,
                    comments, is_favorite, has_media, is_read, links_json)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"#,
            )?,
            tag: conn.prepare_cached("INSERT OR IGNORE INTO tags (name) VALUES (?1)")?,
            tag_id: conn.prepare_cached("SELECT id FROM tags WHERE name = ?1")?,
            bookmark_tag: conn.prepare_cached(
                "INSERT OR IGNORE INTO bookmark_tags (bookmark_id, tag_id) VALUES (?1, ?2)",
            )?,
            media: conn.prepare_cached(
                r#"INSERT INTO media
                   (bookmark_id, url, media_type, alt_text, width, height,
                    source_media_key, source_type, preview_url, variant_url, variants_json,
                    duration_ms, poster_url, playback_url)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"#,
            )?,
            quoted_tweet: conn.prepare_cached(
                r#"INSERT INTO quoted_tweets
                   (bookmark_id, tweet_url, content, tweeted_at, author_handle,
                    author_name, author_profile_image, media_json, unavailable)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
            )?,
            fts_content: conn.prepare_cached(
                r#"INSERT INTO bookmarks_search_docs
                   (bookmark_id, content, note_text, author_handle, author_name, tags_text)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
            )?,
            tag_ids: HashMap::new(),
        })
    }
}

impl Database {
    /// Insert bookmarks in one transaction and journal one import event.
    ///
    /// Returns the number of newly inserted bookmarks; duplicate tweet URLs and
    /// ids are skipped. A batch that inserts nothing writes no history event.
    pub fn insert_bookmarks(&self, bookmarks: &[Bookmark]) -> Result<usize> {
        let files = vec![("import".to_string(), bookmarks.to_vec())];
        Ok(self.insert_bookmarks_batch(&files)?.inserted)
    }

    /// Insert one or more parsed files in a single transaction.
    ///
    /// Duplicates are counted with `INSERT OR IGNORE` + `changes()` per row.
    /// Author stats and the stats snapshot refresh once for the whole batch,
    /// and exactly one import history event is journaled.
    pub fn insert_bookmarks_batch(
        &self,
        files: &[(String, Vec<Bookmark>)],
    ) -> Result<BatchImportOutcome> {
        let conn = &self.conn;
        conn.execute("BEGIN IMMEDIATE", [])?;

        let result = (|| -> Result<BatchImportOutcome> {
            let mut statements = BookmarkInsertStatements::prepare(conn)?;
            let mut outcome = BatchImportOutcome {
                inserted: 0,
                duplicates: 0,
                files: Vec::with_capacity(files.len()),
                history_id: None,
            };
            let mut created_ids = Vec::new();
            let mut labels = Vec::new();

            for (source_label, bookmarks) in files {
                let mut file = FileImportOutcome {
                    source_label: source_label.clone(),
                    parsed: bookmarks.len(),
                    inserted: 0,
                    duplicates: 0,
                };
                for bookmark in bookmarks {
                    if Self::insert_bookmark_internal(bookmark, &mut statements)? {
                        file.inserted += 1;
                        created_ids.push(bookmark.id.clone());
                    } else {
                        // INSERT OR IGNORE skipped an existing id/tweet_url.
                        file.duplicates += 1;
                    }
                }
                outcome.inserted += file.inserted;
                outcome.duplicates += file.duplicates;
                if file.inserted > 0 || !source_label.is_empty() {
                    labels.push(source_label.clone());
                }
                outcome.files.push(file);
            }

            // Author stats are maintained by triggers; the heavier snapshots
            // refresh once for the batch.
            self.ensure_author_stats_consistent()?;
            self.refresh_stats_snapshot()?;

            if !created_ids.is_empty() {
                let summary = HistorySummary {
                    affected: outcome.inserted,
                    skipped: outcome.duplicates,
                    label: import_label(outcome.inserted, outcome.duplicates),
                    tags: Vec::new(),
                    files: labels,
                };
                outcome.history_id = Some(self.journal_import(summary, created_ids)?);
            }

            Ok(outcome)
        })();

        match result {
            Ok(outcome) => {
                conn.execute("COMMIT", [])?;
                Ok(outcome)
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", []);
                Err(error)
            }
        }
    }

    /// Insert full bookmark snapshots without journaling, inside the caller's
    /// transaction. Used by delete undo. Returns the number of inserted rows.
    pub(crate) fn reinsert_bookmarks(&self, bookmarks: &[Bookmark]) -> Result<usize> {
        let mut statements = BookmarkInsertStatements::prepare(&self.conn)?;
        let mut inserted = 0;
        for bookmark in bookmarks {
            if Self::insert_bookmark_internal(bookmark, &mut statements)? {
                inserted += 1;
            }
        }
        Ok(inserted)
    }

    /// Insert one bookmark. Returns `true` when the row was new (duplicates are
    /// ignored by the `OR IGNORE` insert), which is the duplicate accounting
    /// hook for batch imports.
    fn insert_bookmark_internal(
        bookmark: &Bookmark,
        statements: &mut BookmarkInsertStatements<'_>,
    ) -> Result<bool> {
        let has_media_flag = if bookmark.media.is_empty() {
            0i32
        } else {
            1i32
        };
        let links_json =
            serde_json::to_string(&crate::models::normalize_links(bookmark.links.clone()))
                .map_err(|error| {
                    Error::Other(format!("Failed to serialize bookmark links: {error}"))
                })?;

        let changed = statements.bookmark.execute(params![
            bookmark.id,
            bookmark.tweet_url,
            bookmark.content,
            bookmark.note_text,
            bookmark.tweeted_at.timestamp(),
            bookmark.imported_at.timestamp(),
            bookmark.author_handle,
            bookmark.author_name,
            bookmark.author_profile_url,
            bookmark.author_profile_image,
            bookmark.comments,
            bookmark.is_favorite as i32,
            has_media_flag,
            bookmark.is_read as i32,
            links_json,
        ])?;

        if changed == 0 {
            // Existing id or tweet_url: do not write related rows.
            return Ok(false);
        }

        for tag in &bookmark.tags {
            let tag_id = match statements.tag_ids.get(tag).copied() {
                Some(tag_id) => tag_id,
                None => {
                    statements.tag.execute(params![tag])?;
                    let tag_id: i64 = statements
                        .tag_id
                        .query_row(params![tag], |row| row.get(0))?;
                    statements.tag_ids.insert(tag.clone(), tag_id);
                    tag_id
                }
            };
            statements
                .bookmark_tag
                .execute(params![bookmark.id, tag_id])?;
        }

        for media in &bookmark.media {
            statements.media.execute(params![
                bookmark.id,
                media.url,
                media.media_type.as_storage_str(),
                media.alt_text.as_deref(),
                media.width,
                media.height,
                media.source_media_key.as_deref(),
                media.source_type.as_deref(),
                media.preview_url.as_deref(),
                media.variant_url.as_deref(),
                media.variants_json.as_deref(),
                media.duration_ms,
                media.poster_url.as_deref(),
                media.playback_url.as_deref(),
            ])?;
        }

        if let Some(quoted) = &bookmark.quoted {
            let media_json = serde_json::to_string(&quoted.media).map_err(|error| {
                Error::Other(format!("Failed to serialize quoted tweet media: {error}"))
            })?;
            statements.quoted_tweet.execute(params![
                bookmark.id,
                quoted.tweet_url,
                quoted.content,
                quoted.tweeted_at.map(|tweeted_at| tweeted_at.timestamp()),
                quoted.author_handle,
                quoted.author_name,
                quoted.author_profile_image,
                media_json,
                quoted.unavailable as i32,
            ])?;
        }

        let tags_text = bookmark.tags.join(" ");
        statements.fts_content.execute(params![
            bookmark.id,
            bookmark.content,
            bookmark.note_text,
            bookmark.author_handle,
            bookmark.author_name,
            tags_text,
        ])?;

        Ok(true)
    }

    /// Delete one bookmark and journal a delete event in one transaction.
    pub fn delete_bookmark(&self, id: &str) -> Result<bool> {
        self.conn.execute("BEGIN IMMEDIATE", [])?;
        let result = (|| -> Result<bool> {
            let Some(bookmark) = self.get_bookmark(id)? else {
                return Ok(false);
            };
            let deleted = self.delete_bookmark_ids(std::slice::from_ref(&bookmark.id))?;
            if deleted == 0 {
                return Ok(false);
            }
            let summary = HistorySummary {
                affected: deleted,
                skipped: 0,
                label: format!("deleted {deleted} posts"),
                tags: Vec::new(),
                files: Vec::new(),
            };
            self.journal_delete(summary, vec![bookmark])?;
            self.refresh_stats_snapshot()?;
            Ok(true)
        })();

        match result {
            Ok(value) => {
                self.conn.execute("COMMIT", [])?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", []);
                Err(error)
            }
        }
    }

    /// Delete bookmark rows by id inside the caller's transaction.
    ///
    /// `bookmarks_search_docs` is deleted explicitly: relying on the foreign-key
    /// cascade alone left stale FTS rows behind, so a deleted bookmark could
    /// still match a search. Related rows (tags junction, media, quote) cascade.
    pub(crate) fn delete_bookmark_ids(&self, ids: &[String]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let placeholders = vec!["?"; ids.len()].join(", ");
        let params = ids.iter().map(|id| id.as_str()).collect::<Vec<&str>>();

        self.conn.execute(
            &format!("DELETE FROM bookmarks_search_docs WHERE bookmark_id IN ({placeholders})"),
            rusqlite::params_from_iter(params.iter()),
        )?;
        self.conn.execute(
            &format!("DELETE FROM quoted_tweets WHERE bookmark_id IN ({placeholders})"),
            rusqlite::params_from_iter(params.iter()),
        )?;
        let deleted = self.conn.execute(
            &format!("DELETE FROM bookmarks WHERE id IN ({placeholders})"),
            rusqlite::params_from_iter(params.iter()),
        )?;
        Ok(deleted)
    }

    /// Rebuild author_stats from bookmarks (used when batch writes could have
    /// been partially applied by earlier development builds).
    pub(crate) fn ensure_author_stats_consistent(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
DELETE FROM author_stats;
INSERT INTO author_stats (
    author_handle, author_name, author_profile_image, bookmark_count, favorite_count
)
SELECT author_handle,
       COALESCE(NULLIF(MAX(author_name), ''), author_handle),
       MAX(NULLIF(author_profile_image, '')),
       COUNT(*),
       SUM(CASE WHEN is_favorite = 1 THEN 1 ELSE 0 END)
FROM bookmarks
GROUP BY author_handle;
"#,
        )?;
        Ok(())
    }

    pub fn toggle_favorite(&self, id: &str) -> Result<bool> {
        let previous: bool = self.conn.query_row(
            "SELECT is_favorite FROM bookmarks WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;

        self.conn.execute(
            "UPDATE bookmarks SET is_favorite = NOT is_favorite WHERE id = ?1",
            params![id],
        )?;

        let is_favorite = !previous;
        self.adjust_favorite_snapshot(if is_favorite { 1 } else { -1 })?;
        Ok(is_favorite)
    }

    pub fn set_favorite(&self, id: &str, favorite: bool) -> Result<()> {
        let previous: Option<bool> = self
            .conn
            .query_row(
                "SELECT is_favorite FROM bookmarks WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(previous) = previous else {
            return Ok(());
        };

        if previous == favorite {
            return Ok(());
        }

        self.conn.execute(
            "UPDATE bookmarks SET is_favorite = ?2 WHERE id = ?1",
            params![id, favorite as i32],
        )?;
        self.adjust_favorite_snapshot(if favorite { 1 } else { -1 })?;
        Ok(())
    }
}

fn import_label(inserted: usize, duplicates: usize) -> String {
    match (inserted, duplicates) {
        (0, 0) => "imported no posts".to_string(),
        (0, duplicates) => format!("imported 0 posts, {duplicates} duplicates skipped"),
        (inserted, 0) => format!("imported {inserted} posts"),
        (inserted, duplicates) => {
            format!("imported {inserted} posts, {duplicates} duplicates skipped")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{BookmarkBuilder, HistoryKind};
    use chrono::{TimeZone, Utc};

    fn bookmark(tweet_id: &str, tag: &str) -> Bookmark {
        BookmarkBuilder::new()
            .tweet_url(format!("https://x.com/write/status/{tweet_id}"))
            .content(format!("write fixture {tweet_id} #{tag}"))
            .tweeted_at(Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap())
            .author_handle("write")
            .author_name("Write")
            .add_tag(tag)
            .build()
            .unwrap()
    }

    #[test]
    fn batch_insert_counts_duplicates_with_changes_and_journals_once() {
        let db = Database::open_memory().unwrap();
        let first = bookmark("1", "rust");
        let duplicate = first.clone();
        let second = bookmark("2", "design");

        let outcome = db
            .insert_bookmarks_batch(&[
                (
                    "first.json".to_string(),
                    vec![first.clone(), second.clone()],
                ),
                ("second.json".to_string(), vec![duplicate]),
            ])
            .unwrap();

        assert_eq!(outcome.inserted, 2);
        assert_eq!(outcome.duplicates, 1);
        assert_eq!(outcome.files.len(), 2);
        assert_eq!(outcome.files[0].inserted, 2);
        assert_eq!(outcome.files[1].duplicates, 1);
        assert!(outcome.history_id.is_some());
        assert_eq!(db.count_bookmarks().unwrap(), 2);

        let (events, total) = db.history_list(0, 10, true).unwrap();
        assert_eq!(total, 1, "one import event for the whole batch");
        assert_eq!(events[0].kind, HistoryKind::Import);
        assert_eq!(events[0].summary.affected, 2);
        assert_eq!(events[0].summary.skipped, 1);
    }

    #[test]
    fn duplicate_only_import_does_not_journal() {
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&[bookmark("1", "rust")]).unwrap();
        let history_before = db.history_list(0, 10, true).unwrap().1;

        let outcome = db.insert_bookmarks(&[bookmark("1", "rust")]).unwrap();
        assert_eq!(outcome, 0);
        assert_eq!(
            db.history_list(0, 10, true).unwrap().1,
            history_before,
            "a no-op import must not add a history event"
        );
    }

    #[test]
    fn single_delete_removes_fts_content_and_journals_snapshot() {
        let db = Database::open_memory().unwrap();
        let stored = bookmark("fts", "rust");
        let id = stored.id.clone();
        db.insert_bookmarks(&[stored]).unwrap();
        assert_eq!(db.search("write fixture", 10).unwrap().len(), 1);

        assert!(db.delete_bookmark(&id).unwrap());
        assert!(
            db.search("write fixture", 10).unwrap().is_empty(),
            "deleted bookmarks must not stay in the search index"
        );
        let fts_rows: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM bookmarks_search_docs WHERE bookmark_id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts_rows, 0);

        let (events, _) = db.history_list(0, 10, true).unwrap();
        assert_eq!(events[0].kind, HistoryKind::Delete);
    }
}
