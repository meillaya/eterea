//! Tag curation: rename/merge, prune unused tags, colour reset and suggestions.
//!
//! Tags are the only organization primitive in the archive, so duplicates are
//! permanent unless they can be merged. Renaming into an existing name merges
//! the two tags in one transaction (bookmark associations, persisted colour and
//! the FTS `tags_text` document all follow), and unused tags can be pruned.

use super::database::Database;
use crate::{Error, Result};
use rusqlite::params;

/// Outcome of a tag rename/merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagRenameReport {
    pub tag: String,
    pub affected: usize,
    /// True when the destination tag already existed and the two were merged.
    pub merged: bool,
}

/// One existing tag with its usage count, for autocomplete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagSuggestion {
    pub tag: String,
    pub count: i64,
}

impl Database {
    /// Rename one tag, merging into the destination when it already exists.
    pub fn rename_tag(&self, from: &str, to: &str) -> Result<TagRenameReport> {
        let from = from.trim().trim_start_matches('#').trim();
        let to = to.trim().trim_start_matches('#').trim();
        if from.is_empty() || to.is_empty() {
            return Err(Error::Other("a tag name cannot be empty".to_string()));
        }
        let report = TagRenameReport {
            tag: to.to_string(),
            affected: 0,
            merged: false,
        };
        if from.eq_ignore_ascii_case(to) {
            return Ok(report);
        }

        self.conn.execute("BEGIN IMMEDIATE", [])?;
        let result = (|| -> Result<TagRenameReport> {
            let old_id: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM tags WHERE name = ?1",
                    params![from],
                    |row| row.get(0),
                )
                .ok();
            let Some(old_id) = old_id else {
                return Ok(report);
            };
            let existing_new: Option<i64> = self
                .conn
                .query_row("SELECT id FROM tags WHERE name = ?1", params![to], |row| {
                    row.get(0)
                })
                .ok();
            let merged = existing_new.is_some();
            self.conn
                .execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", params![to])?;
            let new_id: i64 =
                self.conn
                    .query_row("SELECT id FROM tags WHERE name = ?1", params![to], |row| {
                        row.get(0)
                    })?;
            self.conn.execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS _eterea_tag_edit (id TEXT PRIMARY KEY);
                 DELETE FROM _eterea_tag_edit;",
            )?;
            self.conn.execute(
                "INSERT OR IGNORE INTO _eterea_tag_edit (id)
                 SELECT bookmark_id FROM bookmark_tags WHERE tag_id = ?1",
                params![old_id],
            )?;
            self.conn.execute(
                r#"INSERT OR IGNORE INTO bookmark_tags (bookmark_id, tag_id)
                   SELECT bookmark_id, ?2 FROM bookmark_tags WHERE tag_id = ?1"#,
                params![old_id, new_id],
            )?;
            self.conn.execute(
                "DELETE FROM bookmark_tags WHERE tag_id = ?1",
                params![old_id],
            )?;
            self.conn
                .execute("DELETE FROM tags WHERE id = ?1", params![old_id])?;
            // Carry the persisted colour to the new name unless it already has one.
            self.conn.execute(
                r#"UPDATE tag_colors SET tag = ?2
                   WHERE tag = ?1
                     AND NOT EXISTS (SELECT 1 FROM tag_colors WHERE tag = ?2)"#,
                params![from, to],
            )?;
            self.conn
                .execute("DELETE FROM tag_colors WHERE tag = ?1", params![from])?;
            // Keep the FTS tags document in sync for every affected bookmark.
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
                   WHERE bookmark_id IN (SELECT id FROM _eterea_tag_edit)"#,
                [],
            )?;
            let affected: i64 =
                self.conn
                    .query_row("SELECT COUNT(*) FROM _eterea_tag_edit", [], |row| {
                        row.get(0)
                    })?;
            Ok(TagRenameReport {
                tag: to.to_string(),
                affected: affected as usize,
                merged,
            })
        })();

        match result {
            Ok(report) => {
                self.conn.execute("COMMIT", [])?;
                Ok(report)
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", []);
                Err(error)
            }
        }
    }

    /// Delete tags no bookmark uses, plus their stale colour overrides.
    pub fn prune_unused_tags(&self) -> Result<usize> {
        self.conn.execute("BEGIN IMMEDIATE", [])?;
        let result = (|| -> Result<usize> {
            let deleted = self.conn.execute(
                "DELETE FROM tags WHERE id NOT IN (SELECT DISTINCT tag_id FROM bookmark_tags)",
                [],
            )?;
            self.conn.execute(
                "DELETE FROM tag_colors WHERE tag NOT IN (SELECT name FROM tags)",
                [],
            )?;
            Ok(deleted)
        })();
        match result {
            Ok(deleted) => {
                self.conn.execute("COMMIT", [])?;
                Ok(deleted)
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", []);
                Err(error)
            }
        }
    }

    /// Remove a persisted colour override so the tag returns to its seeded colour.
    pub fn reset_tag_color(&self, tag: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM tag_colors WHERE tag = ?1", params![tag])?
            > 0)
    }

    /// Tags whose name starts with `prefix`, most-used first, for autocomplete.
    pub fn tag_suggestions(&self, prefix: &str, limit: usize) -> Result<Vec<TagSuggestion>> {
        let prefix = prefix.trim().trim_start_matches('#').trim();
        let like = format!("{}%", prefix.replace(['%', '_'], ""));
        let mut stmt = self.conn.prepare(
            r#"SELECT t.name, COUNT(bt.bookmark_id) AS usage
               FROM tags t
               LEFT JOIN bookmark_tags bt ON bt.tag_id = t.id
               WHERE t.name LIKE ?1 COLLATE NOCASE
               GROUP BY t.id
               ORDER BY usage DESC, t.name ASC
               LIMIT ?2"#,
        )?;
        let rows = stmt.query_map(params![like, limit as i64], |row| {
            Ok(TagSuggestion {
                tag: row.get(0)?,
                count: row.get(1)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::BookmarkBuilder;
    use chrono::{TimeZone, Utc};

    fn bookmark(id: &str, tags: &[&str]) -> crate::models::Bookmark {
        let mut builder = BookmarkBuilder::new()
            .tweet_url(format!("https://x.com/tags/status/{id}"))
            .content(format!("tag fixture {id}"))
            .tweeted_at(Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap())
            .author_handle("tags")
            .author_name("Tags");
        for tag in tags {
            builder = builder.add_tag(*tag);
        }
        builder.build().unwrap()
    }

    #[test]
    fn renaming_moves_associations_and_merges_into_an_existing_tag() {
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&[
            bookmark("1", &["rustlang", "systems"]),
            bookmark("2", &["rustlang"]),
            bookmark("3", &["rust"]),
        ])
        .unwrap();

        let merged = db.rename_tag("rustlang", "rust").unwrap();
        assert!(merged.merged);
        assert_eq!(merged.affected, 2);
        assert!(db
            .get_top_tags(10)
            .unwrap()
            .iter()
            .all(|(tag, _)| tag != "rustlang"));
        let count = db
            .get_top_tags(10)
            .unwrap()
            .into_iter()
            .find(|(tag, _)| tag == "rust")
            .map(|(_, count)| count)
            .unwrap();
        assert_eq!(count, 3);
        assert_eq!(db.search("rustlang", 10).unwrap().len(), 0);
        assert_eq!(db.search("rust", 10).unwrap().len(), 3);
    }

    #[test]
    fn pruning_removes_unused_tags_and_their_colours() {
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&[bookmark("1", &["kept", "orphan"])])
            .unwrap();
        db.set_tag_color("orphan", "#123456").unwrap();
        db.conn
            .execute("DELETE FROM bookmark_tags WHERE tag_id = (SELECT id FROM tags WHERE name = 'orphan')", [])
            .unwrap();
        let pruned = db.prune_unused_tags().unwrap();
        assert_eq!(pruned, 1);
        assert!(db.tag_suggestions("orphan", 5).unwrap().is_empty());
    }

    #[test]
    fn suggestions_filter_by_prefix_and_rank_by_usage() {
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&[
            bookmark("1", &["design", "design-system"]),
            bookmark("2", &["design"]),
            bookmark("3", &["systems"]),
        ])
        .unwrap();
        let suggestions = db.tag_suggestions("des", 10).unwrap();
        assert_eq!(suggestions[0].tag, "design");
        assert_eq!(suggestions[0].count, 2);
        assert!(suggestions.iter().any(|item| item.tag == "design-system"));
    }

    #[test]
    fn colour_reset_removes_only_the_override() {
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&[bookmark("1", &["colourful"])])
            .unwrap();
        db.set_tag_color("colourful", "#123456").unwrap();
        assert!(db.reset_tag_color("colourful").unwrap());
        assert!(db.tag_colors().unwrap().is_empty());
        assert!(!db.reset_tag_color("colourful").unwrap());
    }
}
