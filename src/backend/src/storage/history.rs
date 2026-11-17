//! Undo history: journal writes, paged reads, and newest-event undo.
//!
//! Every journaled action writes its history row inside the same transaction as
//! the action itself, so a crash can never leave an action without its undo
//! step (or the reverse). History keeps only the newest [`HISTORY_LIMIT`]
//! events; older events and their payloads are pruned on every journal write.
//! Only the newest non-undone event can be undone ([`Database::undo_latest`]);
//! older events show their exact before/after but are not undoable.

use super::database::Database;
use crate::models::{
    Bookmark, HistoryDetail, HistoryEvent, HistoryItem, HistoryKind, HistoryState, HistorySummary,
    TagMutation, UndoResult,
};
use crate::{Error, Result};
use chrono::{TimeZone, Utc};
use rusqlite::{params, params_from_iter};
use serde::{Deserialize, Serialize};
use std::io;

/// Number of history rows kept. Older rows (and their payloads) are deleted.
pub const HISTORY_LIMIT: i64 = 5;

#[derive(Debug, Default, Serialize, Deserialize)]
struct ImportUndoPayload {
    #[serde(default)]
    created_ids: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TagUndoPayload {
    #[serde(default)]
    mutation: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    items: Vec<TagUndoItem>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TagUndoItem {
    bookmark_id: String,
    #[serde(default)]
    before_tags: Vec<String>,
    #[serde(default)]
    after_tags: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DeleteUndoPayload {
    #[serde(default)]
    bookmarks: Vec<Bookmark>,
}

impl Database {
    /// Journal an import inside the caller's transaction. Returns the event id.
    pub(crate) fn journal_import(
        &self,
        summary: HistorySummary,
        created_ids: Vec<String>,
    ) -> Result<i64> {
        let payload = ImportUndoPayload { created_ids };
        self.insert_history_event(HistoryKind::Import, &summary, &payload)
    }

    /// Journal a delete inside the caller's transaction.
    pub(crate) fn journal_delete(
        &self,
        summary: HistorySummary,
        snapshots: Vec<Bookmark>,
    ) -> Result<i64> {
        let payload = DeleteUndoPayload {
            bookmarks: snapshots,
        };
        self.insert_history_event(HistoryKind::Delete, &summary, &payload)
    }

    /// Journal a tag edit inside the caller's transaction.
    pub(crate) fn journal_tag(
        &self,
        summary: HistorySummary,
        mutation: &TagMutation,
        items: Vec<(String, Vec<String>, Vec<String>)>,
    ) -> Result<i64> {
        let payload = TagUndoPayload {
            mutation: mutation.kind_str().to_string(),
            tags: mutation.tags().to_vec(),
            items: items
                .into_iter()
                .map(|(bookmark_id, before_tags, after_tags)| TagUndoItem {
                    bookmark_id,
                    before_tags,
                    after_tags,
                })
                .collect(),
        };
        self.insert_history_event(HistoryKind::Tag, &summary, &payload)
    }

    fn insert_history_event(
        &self,
        kind: HistoryKind,
        summary: &HistorySummary,
        payload: &impl Serialize,
    ) -> Result<i64> {
        let summary_json = serde_json::to_string(summary).map_err(|error| {
            Error::Other(format!("failed to serialize history summary: {error}"))
        })?;
        let undo_json = serde_json::to_string(payload).map_err(|error| {
            Error::Other(format!("failed to serialize history undo payload: {error}"))
        })?;

        self.conn.execute(
            r#"INSERT INTO history_events (kind, created_at, summary_json, undo_json)
               VALUES (?1, ?2, ?3, ?4)"#,
            params![
                kind.as_str(),
                Utc::now().timestamp(),
                summary_json,
                undo_json
            ],
        )?;
        let event_id = self.conn.last_insert_rowid();
        self.prune_history()?;
        Ok(event_id)
    }

    fn prune_history(&self) -> Result<()> {
        self.conn.execute(
            &format!(
                "DELETE FROM history_events WHERE id NOT IN \
                 (SELECT id FROM history_events ORDER BY id DESC LIMIT {HISTORY_LIMIT})"
            ),
            [],
        )?;
        Ok(())
    }

    /// Paged history list, newest event first.
    pub fn history_list(
        &self,
        offset: usize,
        limit: usize,
        include_undone: bool,
    ) -> Result<(Vec<HistoryEvent>, i64)> {
        let filter = if include_undone {
            ""
        } else {
            " WHERE undone_at IS NULL"
        };
        let total: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM history_events{filter}"),
            [],
            |row| row.get(0),
        )?;

        let sql = format!(
            r#"SELECT id, kind, created_at, summary_json, undone_at,
                      (SELECT MAX(id) FROM history_events WHERE undone_at IS NULL) AS undoable_id
               FROM history_events{filter}
               ORDER BY id DESC
               LIMIT ?1 OFFSET ?2"#
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let events = stmt
            .query_map(params![limit as i64, offset as i64], |row| {
                event_from_row(row)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok((events, total))
    }

    /// Full detail of one event: the row plus exactly the affected items.
    pub fn history_detail(&self, event_id: i64) -> Result<Option<HistoryDetail>> {
        let row = {
            let mut stmt = self.conn.prepare(
                r#"SELECT id, kind, created_at, summary_json, undone_at,
                          (SELECT MAX(id) FROM history_events WHERE undone_at IS NULL) AS undoable_id
                   FROM history_events WHERE id = ?1"#,
            )?;
            let mut rows = stmt.query(params![event_id])?;
            match rows.next()? {
                Some(row) => Some(event_from_row(row)?),
                None => None,
            }
        };
        let Some(event) = row else {
            return Ok(None);
        };

        let (undo_json, kind) = self.history_payload(event_id)?;
        let items = self.history_items(kind, &undo_json)?;
        Ok(Some(HistoryDetail { event, items }))
    }

    fn history_payload(&self, event_id: i64) -> Result<(String, HistoryKind)> {
        let row = self.conn.query_row(
            "SELECT kind, undo_json FROM history_events WHERE id = ?1",
            params![event_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;
        let kind = HistoryKind::parse(&row.0)
            .ok_or_else(|| Error::Other(format!("unknown history kind '{}'", row.0)))?;
        Ok((row.1, kind))
    }

    fn history_items(&self, kind: HistoryKind, undo_json: &str) -> Result<Vec<HistoryItem>> {
        match kind {
            HistoryKind::Import => {
                let payload: ImportUndoPayload =
                    serde_json::from_str(undo_json).map_err(|error| {
                        Error::Other(format!("invalid import history payload: {error}"))
                    })?;
                Ok(self
                    .lookup_history_items(&payload.created_ids)?
                    .into_iter()
                    .map(|mut item| {
                        item.before = None;
                        item.after = None;
                        item
                    })
                    .collect())
            }
            HistoryKind::Delete => {
                let payload: DeleteUndoPayload =
                    serde_json::from_str(undo_json).map_err(|error| {
                        Error::Other(format!("invalid delete history payload: {error}"))
                    })?;
                Ok(payload
                    .bookmarks
                    .into_iter()
                    .map(|bookmark| HistoryItem {
                        bookmark_id: bookmark.id.clone(),
                        tweet_url: bookmark.tweet_url.clone(),
                        author_handle: bookmark.author_handle.clone(),
                        content: bookmark.content.clone(),
                        before: Some(HistoryState::Bookmark(Box::new(bookmark))),
                        after: None,
                    })
                    .collect())
            }
            HistoryKind::Tag => {
                let payload: TagUndoPayload = serde_json::from_str(undo_json).map_err(|error| {
                    Error::Other(format!("invalid tag history payload: {error}"))
                })?;
                let ids = payload
                    .items
                    .iter()
                    .map(|item| item.bookmark_id.clone())
                    .collect::<Vec<_>>();
                let mut by_id = self
                    .lookup_history_items(&ids)?
                    .into_iter()
                    .map(|item| (item.bookmark_id.clone(), item))
                    .collect::<std::collections::HashMap<_, _>>();
                Ok(payload
                    .items
                    .into_iter()
                    .map(|item| {
                        let mut entry = by_id.remove(&item.bookmark_id).unwrap_or(HistoryItem {
                            bookmark_id: item.bookmark_id.clone(),
                            tweet_url: String::new(),
                            author_handle: String::new(),
                            content: String::new(),
                            before: None,
                            after: None,
                        });
                        entry.before = Some(HistoryState::Tags(item.before_tags));
                        entry.after = Some(HistoryState::Tags(item.after_tags));
                        entry
                    })
                    .collect())
            }
        }
    }

    /// Minimal per-bookmark display data for ids that still exist.
    fn lookup_history_items(&self, ids: &[String]) -> Result<Vec<HistoryItem>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids = ids.to_vec();
        let mut sql = String::from(
            "SELECT id, tweet_url, author_handle, content FROM bookmarks WHERE id IN (",
        );
        sql.push_str(&vec!["?"; ids.len()].join(", "));
        sql.push(')');

        let mut stmt = self.conn.prepare(&sql)?;
        let mut found = stmt
            .query_map(params_from_iter(ids.iter().map(|id| id.as_str())), |row| {
                Ok(HistoryItem {
                    bookmark_id: row.get(0)?,
                    tweet_url: row.get(1)?,
                    author_handle: row.get(2)?,
                    content: row.get(3)?,
                    before: None,
                    after: None,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        // Preserve the payload order, and keep stubs for items already gone.
        let mut by_id: std::collections::HashMap<String, HistoryItem> = found
            .drain(..)
            .map(|item| (item.bookmark_id.clone(), item))
            .collect();
        let ordered = ids
            .into_iter()
            .map(|id| {
                by_id.remove(&id).unwrap_or(HistoryItem {
                    bookmark_id: id,
                    tweet_url: String::new(),
                    author_handle: String::new(),
                    content: String::new(),
                    before: None,
                    after: None,
                })
            })
            .collect();
        Ok(ordered)
    }

    /// Undo the newest non-undone event. All effects and the `undone_at` mark
    /// land in one transaction; conflicts are skipped and reported.
    pub fn undo_latest(&self) -> Result<UndoResult> {
        self.conn.execute("BEGIN IMMEDIATE", [])?;
        let result = (|| -> Result<UndoResult> {
            let newest: Option<(i64, String, String)> = self
                .conn
                .query_row(
                    r#"SELECT id, kind, undo_json FROM history_events
                       WHERE undone_at IS NULL ORDER BY id DESC LIMIT 1"#,
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?;

            let Some((event_id, kind_text, undo_json)) = newest else {
                return Ok(UndoResult::default());
            };
            let kind = HistoryKind::parse(&kind_text)
                .ok_or_else(|| Error::Other(format!("unknown history kind '{kind_text}'")))?;

            let mut result = match kind {
                HistoryKind::Import => self.undo_import(&undo_json)?,
                HistoryKind::Delete => self.undo_delete(&undo_json)?,
                HistoryKind::Tag => self.undo_tag(&undo_json)?,
            };
            result.kind = Some(kind);
            result.event_id = Some(event_id);

            self.conn.execute(
                "UPDATE history_events SET undone_at = ?2 WHERE id = ?1",
                params![event_id, Utc::now().timestamp()],
            )?;
            self.refresh_stats_snapshot()?;
            Ok(result)
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

    fn undo_import(&self, undo_json: &str) -> Result<UndoResult> {
        let payload: ImportUndoPayload = serde_json::from_str(undo_json)
            .map_err(|error| Error::Other(format!("invalid import history payload: {error}")))?;
        if payload.created_ids.is_empty() {
            return Ok(UndoResult::default());
        }

        let existing: i64 = self.conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM bookmarks WHERE id IN ({})",
                vec!["?"; payload.created_ids.len()].join(", ")
            ),
            params_from_iter(payload.created_ids.iter().map(|id| id.as_str())),
            |row| row.get(0),
        )?;
        let removed = self.delete_bookmark_ids(&payload.created_ids)?;

        Ok(UndoResult {
            affected: removed,
            skipped: (existing - removed as i64).max(0) as usize,
            ..UndoResult::default()
        })
    }

    fn undo_delete(&self, undo_json: &str) -> Result<UndoResult> {
        let payload: DeleteUndoPayload = serde_json::from_str(undo_json)
            .map_err(|error| Error::Other(format!("invalid delete history payload: {error}")))?;
        let mut result = UndoResult::default();

        for bookmark in payload.bookmarks {
            let exists: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM bookmarks WHERE id = ?1 OR tweet_url = ?2",
                params![bookmark.id, bookmark.tweet_url],
                |row| row.get(0),
            )?;
            if exists > 0 {
                result.skipped += 1;
                result.skipped_ids.push(bookmark.id);
                continue;
            }
            if self.reinsert_bookmarks(std::slice::from_ref(&bookmark))? > 0 {
                result.affected += 1;
            } else {
                result.skipped += 1;
                result.skipped_ids.push(bookmark.id);
            }
        }

        Ok(result)
    }

    fn undo_tag(&self, undo_json: &str) -> Result<UndoResult> {
        let payload: TagUndoPayload = serde_json::from_str(undo_json)
            .map_err(|error| Error::Other(format!("invalid tag history payload: {error}")))?;
        let mut result = UndoResult::default();

        for item in payload.items {
            let exists: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM bookmarks WHERE id = ?1",
                params![item.bookmark_id],
                |row| row.get(0),
            )?;
            if exists == 0 {
                result.skipped += 1;
                result.skipped_ids.push(item.bookmark_id);
                continue;
            }
            self.replace_bookmark_tags(&item.bookmark_id, &item.before_tags)?;
            result.affected += 1;
        }

        self.delete_orphan_tags()?;
        Ok(result)
    }
}

/// Map one history row. `can_undo` is true only for the newest non-undone event.
fn event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEvent> {
    let kind_text: String = row.get(1)?;
    let Some(kind) = HistoryKind::parse(&kind_text) else {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown history kind '{kind_text}'"),
            )),
        ));
    };
    let created_at: i64 = row.get(2)?;
    let summary_json: String = row.get(3)?;
    let undone_at: Option<i64> = row.get(4)?;
    let undoable_id: Option<i64> = row.get(5)?;
    let id: i64 = row.get(0)?;

    Ok(HistoryEvent {
        id,
        kind,
        created_at: timestamp_from_row(created_at, 2)?,
        summary: serde_json::from_str(&summary_json).unwrap_or_default(),
        undone_at: undone_at
            .map(|value| timestamp_from_row(value, 4))
            .transpose()?,
        can_undo: undone_at.is_none() && undoable_id == Some(id),
    })
}

fn timestamp_from_row(value: i64, column: usize) -> rusqlite::Result<chrono::DateTime<Utc>> {
    Utc.timestamp_opt(value, 0).single().ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Integer,
            Box::new(io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid history timestamp: {value}"),
            )),
        )
    })
}

/// `OptionalExtension` keeps the newest-event lookup readable.
trait OptionalExtension<T> {
    fn optional(self) -> rusqlite::Result<Option<T>>;
}

impl<T> OptionalExtension<T> for rusqlite::Result<T> {
    fn optional(self) -> rusqlite::Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::BookmarkBuilder;
    use chrono::TimeZone;

    fn bookmark(tweet_id: &str, tag: &str) -> Bookmark {
        BookmarkBuilder::new()
            .tweet_url(format!("https://x.com/hist/status/{tweet_id}"))
            .content(format!("history fixture {tweet_id}"))
            .tweeted_at(Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap())
            .author_handle("hist")
            .author_name("Hist")
            .add_tag(tag)
            .build()
            .unwrap()
    }

    #[test]
    fn journal_keeps_only_the_latest_five_events() {
        let db = Database::open_memory().unwrap();
        for index in 0..7 {
            let parsed = crate::ingestion::Ingester::new()
                .parse_content("json", &format!(
                    r#"[{{"tweet_url":"https://x.com/hist/status/h{index}","full_text":"row {index}","tweeted_at":"2024-05-01T12:00:00Z","screen_name":"hist","name":"Hist"}}]"#
                ))
                .unwrap();
            let _ = db.insert_bookmarks(&parsed).unwrap();
        }

        let (events, total) = db.history_list(0, 10, true).unwrap();
        assert_eq!(total, HISTORY_LIMIT);
        assert_eq!(events.len(), 5);
        assert!(events[0].id > events[4].id, "newest first");
        assert!(events[0].can_undo);
        assert!(!events[1].can_undo, "only the newest is undoable");
    }

    #[test]
    fn undo_import_deletes_exactly_the_imported_ids() {
        let db = Database::open_memory().unwrap();
        let parsed = crate::ingestion::Ingester::new()
            .parse_content(
                "json",
                r#"[
                    {"tweet_url":"https://x.com/hist/status/a","full_text":"a","tweeted_at":"2024-05-01T12:00:00Z","screen_name":"hist","name":"Hist"},
                    {"tweet_url":"https://x.com/hist/status/b","full_text":"b","tweeted_at":"2024-05-01T12:00:00Z","screen_name":"hist","name":"Hist"}
                ]"#,
            )
            .unwrap();
        assert_eq!(db.insert_bookmarks(&parsed).unwrap(), 2);
        assert_eq!(db.count_bookmarks().unwrap(), 2);

        let result = db.undo_latest().unwrap();
        assert_eq!(result.kind, Some(HistoryKind::Import));
        assert_eq!(result.affected, 2);
        assert_eq!(db.count_bookmarks().unwrap(), 0);

        // The search index no longer returns the removed rows.
        assert!(db.search("history", 10).unwrap().is_empty());
        // A second undo has nothing to do.
        assert_eq!(db.undo_latest().unwrap().kind, None);
    }

    #[test]
    fn history_detail_shows_before_and_after_items() {
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&[bookmark("d1", "before")]).unwrap();
        let stored = db.get_bookmarks(0, 10).unwrap().remove(0);
        db.set_bookmark_tags(&stored.id, &["after".to_string()])
            .unwrap();
        let (events, _) = db.history_list(0, 10, true).unwrap();
        assert_eq!(events[0].kind, HistoryKind::Tag);

        let detail = db.history_detail(events[0].id).unwrap().unwrap();
        assert_eq!(detail.items.len(), 1);
        assert_eq!(
            detail.items[0].before,
            Some(HistoryState::Tags(vec!["before".to_string()]))
        );
        assert_eq!(
            detail.items[0].after,
            Some(HistoryState::Tags(vec!["after".to_string()]))
        );
    }
}
