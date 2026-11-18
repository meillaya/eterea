//! Recoverable deletes.
//!
//! Every delete made through the reader (single or bulk) writes the full
//! bookmark snapshot — tags, media, quote, read/favorite flags — into the
//! `trash` table inside the same transaction as the delete. Undo history stays
//! a short journal of recent actions; trash is the longer safety net behind it,
//! holding the newest [`TRASH_LIMIT`] deleted posts so an accidental bulk
//! delete can always be recovered one by one.

use super::database::Database;
use crate::models::Bookmark;
use crate::{Error, Result};
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};

/// Deleted posts kept in the trash.
pub const TRASH_LIMIT: i64 = 200;

/// One recoverable deletion, without the full snapshot payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashEntry {
    pub id: i64,
    pub bookmark_id: String,
    pub deleted_at: i64,
    pub author_handle: String,
    pub author_name: String,
    pub preview: String,
}

impl Database {
    /// Move bookmark snapshots into the trash. Call inside the delete
    /// transaction, before the bookmark rows are removed.
    pub(crate) fn trash_snapshots(&self, snapshots: &[Bookmark]) -> Result<usize> {
        if snapshots.is_empty() {
            return Ok(0);
        }
        let deleted_at = Utc::now().timestamp();
        let mut stmt = self.conn.prepare(
            r#"INSERT INTO trash (bookmark_id, deleted_at, payload_json)
               VALUES (?1, ?2, ?3)"#,
        )?;
        for snapshot in snapshots {
            let payload = serde_json::to_string(snapshot)?;
            stmt.execute(params![snapshot.id, deleted_at, payload])?;
        }
        drop(stmt);
        self.prune_trash()?;
        Ok(snapshots.len())
    }

    fn prune_trash(&self) -> Result<()> {
        self.conn.execute(
            r#"DELETE FROM trash WHERE id NOT IN (
                   SELECT id FROM trash ORDER BY id DESC LIMIT ?1
               )"#,
            params![TRASH_LIMIT],
        )?;
        Ok(())
    }

    /// Number of posts currently recoverable.
    pub fn trash_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM trash", [], |row| row.get(0))?)
    }

    /// Paged trash listing, newest first.
    pub fn list_trash(&self, offset: usize, limit: usize) -> Result<(Vec<TrashEntry>, i64)> {
        let total = self.trash_count()?;
        let mut stmt = self.conn.prepare(
            r#"SELECT id, bookmark_id, deleted_at, payload_json
               FROM trash ORDER BY id DESC LIMIT ?1 OFFSET ?2"#,
        )?;
        let rows = stmt.query_map(params![limit as i64, offset as i64], |row| {
            let id: i64 = row.get(0)?;
            let bookmark_id: String = row.get(1)?;
            let deleted_at: i64 = row.get(2)?;
            let payload: String = row.get(3)?;
            Ok((id, bookmark_id, deleted_at, payload))
        })?;
        let mut entries = Vec::new();
        for row in rows {
            let (id, bookmark_id, deleted_at, payload) = row?;
            let Ok(bookmark) = serde_json::from_str::<Bookmark>(&payload) else {
                continue;
            };
            let preview: String = bookmark.content.chars().take(120).collect();
            entries.push(TrashEntry {
                id,
                bookmark_id,
                deleted_at,
                author_handle: bookmark.author_handle,
                author_name: bookmark.author_name,
                preview,
            });
        }
        Ok((entries, total))
    }

    /// Restore the newest trashed post. Returns the restored bookmark.
    pub fn restore_latest_trash(&self) -> Result<Option<Bookmark>> {
        let id: Option<i64> = self
            .conn
            .query_row("SELECT id FROM trash ORDER BY id DESC LIMIT 1", [], |row| {
                row.get(0)
            })
            .ok();
        match id {
            Some(id) => self.restore_trash(id),
            None => Ok(None),
        }
    }

    /// Restore one trashed post by trash id.
    pub fn restore_trash(&self, id: i64) -> Result<Option<Bookmark>> {
        let payload: Option<String> = self
            .conn
            .query_row(
                "SELECT payload_json FROM trash WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .ok();
        let Some(payload) = payload else {
            return Ok(None);
        };
        let bookmark: Bookmark = serde_json::from_str(&payload)
            .map_err(|error| Error::Other(format!("invalid trash snapshot: {error}")))?;

        self.conn.execute("BEGIN IMMEDIATE", [])?;
        let result = (|| -> Result<bool> {
            self.conn
                .execute("DELETE FROM trash WHERE id = ?1", params![id])?;
            let restored = self.reinsert_bookmarks(std::slice::from_ref(&bookmark))? > 0;
            self.refresh_stats_snapshot()?;
            self.ensure_author_stats_consistent()?;
            Ok(restored)
        })();
        match result {
            Ok(true) => Ok(Some(bookmark)),
            Ok(false) => {
                self.conn.execute("ROLLBACK", [])?;
                Ok(None)
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", []);
                Err(error)
            }
        }
    }

    /// Drop every recoverable deletion.
    pub fn empty_trash(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM trash", [])?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::BookmarkBuilder;
    use chrono::{TimeZone, Utc};

    fn bookmark(tweet_id: &str) -> Bookmark {
        BookmarkBuilder::new()
            .tweet_url(format!("https://x.com/trash/status/{tweet_id}"))
            .content(format!("trash fixture {tweet_id}"))
            .tweeted_at(Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap())
            .author_handle("trash")
            .author_name("Trash")
            .add_tag("recoverable")
            .build()
            .unwrap()
    }

    #[test]
    fn deleting_moves_a_post_to_trash_and_restoring_reinstates_it() {
        let db = Database::open_memory().unwrap();
        let first = bookmark("1");
        db.insert_bookmarks(&[first.clone(), bookmark("2")])
            .unwrap();
        assert_eq!(db.count_bookmarks().unwrap(), 2);

        assert!(db.delete_bookmark(&first.id).unwrap());
        assert_eq!(db.count_bookmarks().unwrap(), 1);
        assert_eq!(db.trash_count().unwrap(), 1);
        let (entries, total) = db.list_trash(0, 10).unwrap();
        assert_eq!(total, 1);
        assert_eq!(entries[0].bookmark_id, first.id);
        assert_eq!(entries[0].author_handle, "trash");

        let restored = db.restore_latest_trash().unwrap().expect("entry restored");
        assert_eq!(restored.id, first.id);
        assert_eq!(db.count_bookmarks().unwrap(), 2);
        assert_eq!(db.trash_count().unwrap(), 0);
        let restored_row = db.get_bookmark(&first.id).unwrap().unwrap();
        assert_eq!(restored_row.tags, vec!["recoverable".to_string()]);
    }

    #[test]
    fn bulk_delete_trashes_every_snapshot_and_empty_trash_drops_them() {
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&[bookmark("1"), bookmark("2"), bookmark("3")])
            .unwrap();
        let selection =
            crate::storage::BookmarkSelection::Query(crate::storage::BookmarkQuery::default());
        let result = db.bulk_delete(&selection).unwrap();
        assert_eq!(result.affected, 3);
        assert_eq!(db.count_bookmarks().unwrap(), 0);
        assert_eq!(db.trash_count().unwrap(), 3);
        assert_eq!(db.empty_trash().unwrap(), 3);
        assert_eq!(db.trash_count().unwrap(), 0);
    }
}
