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

/// Columns read from an existing row when a duplicate import arrives.
type ExistingBookmarkRow = (
    String,
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
);

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
                    comments, is_favorite, has_media, is_read, links_json, conversation_id,
                    unavailable)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                           ?16, ?17)"#,
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
                   (bookmark_id, content, note_text, author_handle, author_name, tags_text,
                    quote_text, alt_text, links_text)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
            )?,
            tag_ids: HashMap::new(),
        })
    }
}

impl Database {
    /// Mark (or clear) a bookmark's availability after a network check.
    pub fn set_bookmark_unavailable(&self, bookmark_id: &str, unavailable: bool) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE bookmarks SET unavailable = ?2 WHERE id = ?1",
            params![bookmark_id, unavailable as i32],
        )? > 0)
    }

    /// Set or clear the reader's note on one post and keep the search document
    /// in sync. Returns true when the row changed.
    pub fn set_note(&self, bookmark_id: &str, note: Option<&str>) -> Result<bool> {
        let trimmed = note.map(str::trim).filter(|note| !note.is_empty());
        let changed = self.conn.execute(
            "UPDATE bookmarks SET note_text = ?2 WHERE id = ?1",
            params![bookmark_id, trimmed],
        )?;
        if changed == 0 {
            return Ok(false);
        }
        self.conn.execute(
            "UPDATE bookmarks_search_docs SET note_text = ?2 WHERE bookmark_id = ?1",
            params![bookmark_id, trimmed],
        )?;
        Ok(true)
    }

    /// Recompute the supplemental FTS text (quote, alt text, link targets) for
    /// one bookmark from its own tables. Called after a duplicate merge adds a
    /// quote, media or links to an existing row.
    pub(crate) fn refresh_search_doc_derived(&self, bookmark_id: &str) -> Result<()> {
        self.conn.execute(
            r#"UPDATE bookmarks_search_docs SET
                 quote_text = COALESCE((
                     SELECT content || ' ' || author_name || ' ' || author_handle
                     FROM quoted_tweets WHERE bookmark_id = bookmarks_search_docs.bookmark_id
                 ), ''),
                 alt_text = COALESCE((
                     SELECT group_concat(alt_text, ' ') FROM media
                     WHERE bookmark_id = bookmarks_search_docs.bookmark_id
                       AND alt_text IS NOT NULL AND alt_text <> ''
                 ), ''),
                 links_text = COALESCE((
                     SELECT group_concat(target, ' ') FROM (
                         SELECT json_extract(value, '$.expanded_url') AS target
                         FROM json_each((SELECT links_json FROM bookmarks
                                         WHERE id = bookmarks_search_docs.bookmark_id))
                         UNION
                         SELECT json_extract(value, '$.url')
                         FROM json_each((SELECT links_json FROM bookmarks
                                         WHERE id = bookmarks_search_docs.bookmark_id))
                     ) WHERE target IS NOT NULL AND target <> ''
                 ), '')
               WHERE bookmark_id = ?1"#,
            params![bookmark_id],
        )?;
        Ok(())
    }

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
                    if self.insert_bookmark_internal(bookmark, &mut statements)? {
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
            if self.insert_bookmark_internal(bookmark, &mut statements)? {
                inserted += 1;
            }
        }
        Ok(inserted)
    }

    /// Insert one bookmark. Returns `true` when the row was new (duplicates are
    /// ignored by the `OR IGNORE` insert), which is the duplicate accounting
    /// hook for batch imports.
    fn insert_bookmark_internal(
        &self,
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
            bookmark.conversation_id,
            bookmark.unavailable as i32,
        ])?;

        if changed == 0 {
            // Existing id or tweet_url: a richer source (for example a JSON
            // export after a text-only CSV) may still carry media, a quote,
            // links, tags or refreshed author metadata for this tweet. Merge
            // what the new copy knows; read/favorite state and existing rows
            // stay untouched.
            self.merge_duplicate_bookmark(bookmark, statements)?;
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
        let (quote_text, alt_text, links_text) = search_doc_extra_texts(bookmark);
        statements.fts_content.execute(params![
            bookmark.id,
            bookmark.content,
            bookmark.note_text,
            bookmark.author_handle,
            bookmark.author_name,
            tags_text,
            quote_text,
            alt_text,
            links_text,
        ])?;

        Ok(true)
    }

    /// Enrich an existing bookmark from a duplicate import row.
    ///
    /// Only additive data is copied: refreshed author name/profile image, a
    /// missing body, links, extra tags, media rows when the archive had none,
    /// and a quote row when it was absent. A duplicate can never clear a field
    /// or overwrite read/favorite state.
    fn merge_duplicate_bookmark(
        &self,
        bookmark: &Bookmark,
        statements: &mut BookmarkInsertStatements<'_>,
    ) -> Result<bool> {
        let existing: Option<ExistingBookmarkRow> = self
            .conn
            .query_row(
                r#"SELECT id, has_media, COALESCE(links_json, ''), content,
                          COALESCE(note_text, ''), author_profile_image, author_profile_url,
                          COALESCE(conversation_id, '')
                   FROM bookmarks WHERE id = ?1 OR tweet_url = ?2 LIMIT 1"#,
                params![bookmark.id, bookmark.tweet_url],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            existing_id,
            existing_has_media,
            existing_links,
            existing_content,
            existing_note,
            _,
            _,
            existing_conversation,
        )) = existing
        else {
            return Ok(false);
        };

        // Author metadata is a snapshot: prefer the newest non-empty values the
        // source provides so renamed accounts and changed avatars stop showing
        // their old identity.
        if !bookmark.author_name.trim().is_empty()
            || bookmark.author_profile_image.is_some()
            || bookmark.author_profile_url.is_some()
        {
            self.conn.execute(
                r#"UPDATE bookmarks SET
                       author_name = CASE WHEN ?2 <> '' THEN ?2 ELSE author_name END,
                       author_profile_image = COALESCE(?3, author_profile_image),
                       author_profile_url = COALESCE(?4, author_profile_url)
                   WHERE id = ?1"#,
                params![
                    existing_id,
                    bookmark.author_name,
                    bookmark.author_profile_image,
                    bookmark.author_profile_url,
                ],
            )?;
        }

        if existing_content.trim().is_empty() && !bookmark.content.trim().is_empty() {
            self.conn.execute(
                "UPDATE bookmarks SET content = ?2 WHERE id = ?1",
                params![existing_id, bookmark.content],
            )?;
            self.conn.execute(
                "UPDATE bookmarks_search_docs SET content = ?2 WHERE bookmark_id = ?1",
                params![existing_id, bookmark.content],
            )?;
        }

        if existing_note.trim().is_empty() {
            if let Some(note) = bookmark
                .note_text
                .as_deref()
                .filter(|note| !note.trim().is_empty())
            {
                self.conn.execute(
                    "UPDATE bookmarks SET note_text = ?2 WHERE id = ?1",
                    params![existing_id, note],
                )?;
                self.conn.execute(
                    "UPDATE bookmarks_search_docs SET note_text = ?2 WHERE bookmark_id = ?1",
                    params![existing_id, note],
                )?;
            }
        }

        if (existing_links.trim().is_empty() || existing_links == "[]")
            && !bookmark.links.is_empty()
        {
            let links_json =
                serde_json::to_string(&crate::models::normalize_links(bookmark.links.clone()))
                    .map_err(|error| {
                        Error::Other(format!("Failed to serialize bookmark links: {error}"))
                    })?;
            self.conn.execute(
                "UPDATE bookmarks SET links_json = ?2 WHERE id = ?1",
                params![existing_id, links_json],
            )?;
        }

        if !bookmark.tags.is_empty() {
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
                    .execute(params![existing_id, tag_id])?;
            }
            self.conn.execute(
                r#"UPDATE bookmarks_search_docs
                   SET tags_text = COALESCE((
                       SELECT group_concat(name, ' ')
                       FROM (
                           SELECT t.name FROM bookmark_tags bt
                           JOIN tags t ON t.id = bt.tag_id
                           WHERE bt.bookmark_id = bookmarks_search_docs.bookmark_id
                           ORDER BY t.name
                       )
                   ), '')
                   WHERE bookmark_id = ?1"#,
                params![existing_id],
            )?;
        }

        if existing_has_media == 0 && !bookmark.media.is_empty() {
            for media in &bookmark.media {
                statements.media.execute(params![
                    existing_id,
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
            self.conn.execute(
                "UPDATE bookmarks SET has_media = 1 WHERE id = ?1",
                params![existing_id],
            )?;
        }

        if let Some(quoted) = &bookmark.quoted {
            let has_quote: bool = self
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM quoted_tweets WHERE bookmark_id = ?1)",
                    params![existing_id],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            if !has_quote {
                let media_json = serde_json::to_string(&quoted.media).map_err(|error| {
                    Error::Other(format!("Failed to serialize quoted tweet media: {error}"))
                })?;
                statements.quoted_tweet.execute(params![
                    existing_id,
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
        }

        if existing_conversation.trim().is_empty() {
            if let Some(conversation_id) = bookmark
                .conversation_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
            {
                self.conn.execute(
                    "UPDATE bookmarks SET conversation_id = ?2 WHERE id = ?1",
                    params![existing_id, conversation_id],
                )?;
            }
        }

        self.refresh_search_doc_derived(&existing_id)?;
        Ok(true)
    }

    /// Merge enriched metadata into existing rows without creating a history
    /// event. Used by the opt-in repair pass; read/favorite state is preserved
    /// and unmatched rows are ignored.
    pub fn enrich_bookmarks(&self, bookmarks: &[Bookmark]) -> Result<usize> {
        let conn = &self.conn;
        conn.execute("BEGIN IMMEDIATE", [])?;
        let result = (|| -> Result<usize> {
            let mut statements = BookmarkInsertStatements::prepare(conn)?;
            let mut enriched = 0;
            for bookmark in bookmarks {
                if self.merge_duplicate_bookmark(bookmark, &mut statements)? {
                    enriched += 1;
                }
            }
            self.ensure_author_stats_consistent()?;
            self.refresh_stats_snapshot()?;
            Ok(enriched)
        })();
        match result {
            Ok(enriched) => {
                conn.execute("COMMIT", [])?;
                Ok(enriched)
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", []);
                Err(error)
            }
        }
    }

    /// Posts that still have no media row, newest first. The repair pass feeds
    /// these ids to the embed endpoint.
    pub fn bookmarks_missing_media(&self, limit: usize) -> Result<Vec<(String, String)>> {
        let mut statement = self.conn.prepare(
            "SELECT id, tweet_url FROM bookmarks WHERE has_media = 0              ORDER BY tweeted_at DESC, id DESC LIMIT ?1",
        )?;
        let rows = statement
            .query_map(params![limit as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Number of posts the repair pass would consider.
    pub fn count_bookmarks_missing_media(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM bookmarks WHERE has_media = 0",
            [],
            |row| row.get(0),
        )?)
    }

    /// Delete one bookmark and journal a delete event in one transaction.
    pub fn delete_bookmark(&self, id: &str) -> Result<bool> {
        self.conn.execute("BEGIN IMMEDIATE", [])?;
        let result = (|| -> Result<bool> {
            let Some(bookmark) = self.get_bookmark(id)? else {
                return Ok(false);
            };
            self.trash_snapshots(std::slice::from_ref(&bookmark))?;
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
       COALESCE(NULLIF((
           SELECT author_name FROM bookmarks newest
           WHERE newest.author_handle = bookmarks.author_handle
           ORDER BY newest.tweeted_at DESC, newest.id DESC LIMIT 1
       ), ''), author_handle),
       (SELECT author_profile_image FROM bookmarks newest
        WHERE newest.author_handle = bookmarks.author_handle
        ORDER BY newest.tweeted_at DESC, newest.id DESC LIMIT 1),
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

/// Text from things a post carries besides its body: the quoted post, media
/// alt text and expanded link targets. They are indexed so a search finds a
/// post by what its quote/alt text/link says, not just its body.
fn search_doc_extra_texts(bookmark: &Bookmark) -> (String, String, String) {
    let quote_text = bookmark
        .quoted
        .as_ref()
        .map(|quote| {
            format!(
                "{} {} {}",
                quote.content, quote.author_name, quote.author_handle
            )
        })
        .unwrap_or_default();
    let alt_text = bookmark
        .media
        .iter()
        .filter_map(|media| media.alt_text.as_deref())
        .filter(|alt| !alt.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let links_text = bookmark
        .links
        .iter()
        .map(|link| link.expanded_url.as_deref().unwrap_or(&link.url))
        .collect::<Vec<_>>()
        .join(" ");
    (quote_text, alt_text, links_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{BookmarkBuilder, HistoryKind, QuotedTweet};
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
    fn duplicate_import_enriches_media_quote_tags_body_and_author() {
        let db = Database::open_memory().unwrap();
        // A text-only source (for example a CSV export) imports first.
        let sparse = BookmarkBuilder::new()
            .tweet_url("https://x.com/rich/status/9")
            .tweeted_at(Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap())
            .author_handle("rich")
            .author_name("Old Name")
            .author_profile_image("https://pbs.twimg.com/profile_images/old_normal.jpg")
            .build()
            .unwrap();
        db.insert_bookmarks(&[sparse]).unwrap();
        let sparse_id = db.get_bookmarks(0, 10).unwrap()[0].id.clone();
        db.set_read(&sparse_id, true).unwrap();

        // A richer source (JSON/archive) re-imports the same tweet.
        let rich = BookmarkBuilder::new()
            .tweet_url("https://x.com/rich/status/9")
            .content("rich body https://t.co/x")
            .tweeted_at(Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap())
            .author_handle("rich")
            .author_name("New Name")
            .author_profile_image("https://pbs.twimg.com/profile_images/new_normal.jpg")
            .add_tag("film")
            .add_media("https://pbs.twimg.com/media/one.jpg")
            .quoted(QuotedTweet::new(
                "https://x.com/other/status/1",
                "quoted body",
                None,
                "other",
                "Other",
            ))
            .build()
            .unwrap();
        let outcome = db
            .insert_bookmarks_batch(&[("rich.json".to_string(), vec![rich])])
            .unwrap();

        assert_eq!(outcome.inserted, 0);
        assert_eq!(outcome.duplicates, 1);
        assert_eq!(db.count_bookmarks().unwrap(), 1, "no duplicate row");

        let stored = db.get_bookmarks(0, 10).unwrap().remove(0);
        assert_eq!(stored.author_name, "New Name");
        assert_eq!(
            stored.author_profile_image.as_deref(),
            Some("https://pbs.twimg.com/profile_images/new_normal.jpg")
        );
        assert_eq!(stored.content, "rich body https://t.co/x");
        assert!(stored.is_read, "read state survives enrichment");
        assert_eq!(stored.tags, vec!["film".to_string()]);
        assert_eq!(stored.media.len(), 1, "media backfilled");
        assert!(stored.media[0].url.contains("one.jpg"));
        assert!(stored.quoted.is_some(), "quote backfilled");
        assert_eq!(db.search("rich body", 10).unwrap().len(), 1);
        assert_eq!(db.search("film", 10).unwrap().len(), 1);
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

#[cfg(test)]
mod fts_coverage_tests {
    use crate::models::{Bookmark, BookmarkBuilder, LinkEntity, Media, MediaType, QuotedTweet};
    use crate::storage::Database;
    use chrono::{TimeZone, Utc};

    fn bookmark(tweet_id: &str) -> BookmarkBuilder {
        BookmarkBuilder::new()
            .tweet_url(format!("https://x.com/coverage/status/{tweet_id}"))
            .content("body without the searchable bits")
            .tweeted_at(Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap())
            .author_handle("coverage")
            .author_name("Coverage")
    }

    fn insert(db: &Database, bookmark: Bookmark) {
        db.insert_bookmarks(&[bookmark]).unwrap();
    }

    #[test]
    fn search_finds_quoted_post_text() {
        let db = Database::open_memory().unwrap();
        let quoted = QuotedTweet {
            tweet_url: "https://x.com/someone/status/9".to_string(),
            content: "the quoted post mentions phlogiston".to_string(),
            tweeted_at: None,
            author_handle: "someone".to_string(),
            author_name: "Someone".to_string(),
            author_profile_image: None,
            media: Vec::new(),
            unavailable: false,
        };
        insert(&db, bookmark("1").quoted(quoted).build().unwrap());

        let hits = db.search("phlogiston", 10).unwrap();
        assert_eq!(hits.len(), 1, "quote text should be searchable");
    }

    #[test]
    fn search_finds_media_alt_text() {
        let db = Database::open_memory().unwrap();
        let media = Media {
            url: "https://pbs.twimg.com/media/x.jpg".to_string(),
            media_type: MediaType::Image,
            alt_text: Some("a heron standing in mist".to_string()),
            width: None,
            height: None,
            source_media_key: None,
            source_type: None,
            preview_url: None,
            variant_url: None,
            variants_json: None,
            duration_ms: None,
            poster_url: None,
            playback_url: None,
        };
        insert(
            &db,
            bookmark("2").add_media_metadata(media).build().unwrap(),
        );

        let hits = db.search("heron", 10).unwrap();
        assert_eq!(hits.len(), 1, "alt text should be searchable");
    }

    #[test]
    fn search_finds_link_targets() {
        let db = Database::open_memory().unwrap();
        let link = LinkEntity {
            url: "https://t.co/abc".to_string(),
            expanded_url: Some("https://example.org/rare-word".to_string()),
            display_url: None,
            kind: crate::models::LinkKind::External,
        };
        insert(&db, bookmark("3").add_link(link).build().unwrap());

        let hits = db.search("example.org", 10).unwrap();
        assert_eq!(hits.len(), 1, "expanded link target should be searchable");
    }

    #[test]
    fn a_duplicate_merge_indexes_new_quote_alt_and_link_text() {
        let db = Database::open_memory().unwrap();
        insert(&db, bookmark("4").build().unwrap());
        let quoted = QuotedTweet {
            tweet_url: "https://x.com/someone/status/10".to_string(),
            content: "later enriched with zeppelin".to_string(),
            tweeted_at: None,
            author_handle: "someone".to_string(),
            author_name: "Someone".to_string(),
            author_profile_image: None,
            media: Vec::new(),
            unavailable: false,
        };
        let link = LinkEntity {
            url: "https://t.co/def".to_string(),
            expanded_url: Some("https://example.net/airship".to_string()),
            display_url: None,
            kind: crate::models::LinkKind::External,
        };
        insert(
            &db,
            bookmark("4").quoted(quoted).add_link(link).build().unwrap(),
        );

        assert_eq!(db.search("zeppelin", 10).unwrap().len(), 1);
        assert_eq!(db.search("airship", 10).unwrap().len(), 1);
    }
}

#[cfg(test)]
mod note_tests {
    use crate::models::BookmarkBuilder;
    use crate::storage::Database;
    use chrono::{TimeZone, Utc};

    #[test]
    fn notes_save_clear_and_stay_searchable() {
        let db = Database::open_memory().unwrap();
        let bookmark = BookmarkBuilder::new()
            .tweet_url("https://x.com/notes/status/1")
            .content("post body")
            .tweeted_at(Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap())
            .author_handle("notes")
            .author_name("Notes")
            .build()
            .unwrap();
        db.insert_bookmarks(std::slice::from_ref(&bookmark))
            .unwrap();
        assert!(db.search("kestrel", 10).unwrap().is_empty());

        assert!(db
            .set_note(&bookmark.id, Some("  remember the kestrel  "))
            .unwrap());
        let saved = db.get_bookmark(&bookmark.id).unwrap().unwrap();
        assert_eq!(saved.note_text.as_deref(), Some("remember the kestrel"));
        assert_eq!(db.search("kestrel", 10).unwrap().len(), 1);

        assert!(db.set_note(&bookmark.id, None).unwrap());
        let cleared = db.get_bookmark(&bookmark.id).unwrap().unwrap();
        assert!(cleared.note_text.is_none());
        assert!(db.search("kestrel", 10).unwrap().is_empty());
    }
}

#[cfg(test)]
mod availability_tests {
    use crate::models::BookmarkBuilder;
    use crate::storage::Database;
    use chrono::{TimeZone, Utc};

    fn bookmark(tweet_id: &str) -> crate::models::Bookmark {
        BookmarkBuilder::new()
            .tweet_url(format!("https://x.com/gone/status/{tweet_id}"))
            .content(format!("availability fixture {tweet_id}"))
            .tweeted_at(Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap())
            .author_handle("gone")
            .author_name("Gone")
            .build()
            .unwrap()
    }

    #[test]
    fn availability_flag_round_trips_and_clears() {
        let db = Database::open_memory().unwrap();
        let first = bookmark("1");
        db.insert_bookmarks(std::slice::from_ref(&first)).unwrap();
        let loaded = db.get_bookmark(&first.id).unwrap().unwrap();
        assert!(!loaded.unavailable);

        assert!(db.set_bookmark_unavailable(&first.id, true).unwrap());
        let marked = db.get_bookmark(&first.id).unwrap().unwrap();
        assert!(marked.unavailable, "flag must hydrate through the model");
        let targets = db.bookmarks_for_availability(10).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].0, first.id);

        // A duplicate import never resurrects a row silently; only a successful
        // availability check clears the flag.
        db.insert_bookmarks(std::slice::from_ref(&first)).unwrap();
        assert!(db.get_bookmark(&first.id).unwrap().unwrap().unavailable);

        assert!(db.set_bookmark_unavailable(&first.id, false).unwrap());
        assert!(!db.get_bookmark(&first.id).unwrap().unwrap().unavailable);
    }
}
