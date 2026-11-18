//! Tag, read-state, and bulk bookmark mutations.
//!
//! Bulk operations take a [`BookmarkSelection`]: `Ids` uses one `IN` list, a
//! `Query` uses the shared SQL predicate so a filter match never materializes
//! 10,000 ids. Every mutation keeps `tags`, `bookmark_tags`,
//! `bookmarks_search_docs.tags_text` (and therefore FTS) in sync in the same
//! transaction as its history event.

use super::database::Database;
use super::query::BookmarkSelection;
use crate::models::{HistorySummary, TagMutation};
use crate::Result;
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter};
use std::collections::{HashMap, HashSet};

/// Result of a bulk mutation: exact counts plus the journaled history event.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct BulkResult {
    /// Bookmarks whose state changed.
    pub affected: usize,
    /// Selected bookmarks that needed no change or no longer exist.
    pub skipped: usize,
    /// Ids whose state changed (newest first).
    pub affected_ids: Vec<String>,
    /// History event id when the action was journaled.
    pub history_id: Option<i64>,
}

impl Database {
    /// Replace one bookmark's tags exactly (`TagMutation::Set` semantics),
    /// journaling one tag event. Returns the affected ids.
    pub fn set_bookmark_tags(&self, id: &str, tags: &[String]) -> Result<BulkResult> {
        self.apply_tag_mutation(
            &BookmarkSelection::single(id),
            &TagMutation::Set(tags.to_vec()),
        )
    }

    /// Apply one tag mutation to a selection.
    pub fn apply_tag_mutation(
        &self,
        selection: &BookmarkSelection,
        mutation: &TagMutation,
    ) -> Result<BulkResult> {
        let mutation = mutation.clone().normalized();
        let requested = requested_count(selection);

        self.conn.execute("BEGIN IMMEDIATE", [])?;
        let result = (|| -> Result<BulkResult> {
            let (where_clause, where_params) = self.selection_where(selection)?;
            let ids = self.selected_ids(selection)?;
            if ids.is_empty() {
                return Ok(BulkResult::default());
            }

            let before_by_id = self.tags_for_bookmark_ids(&ids)?;
            let mut changed: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
            for id in &ids {
                let before = before_by_id.get(id).cloned().unwrap_or_default();
                let after = apply_mutation_to_tags(&before, &mutation);
                if !same_tags_ci(&before, &after) {
                    changed.push((id.clone(), before, after));
                }
            }

            if changed.is_empty() {
                return Ok(BulkResult {
                    affected: 0,
                    skipped: requested,
                    affected_ids: Vec::new(),
                    history_id: None,
                });
            }

            // Set-tag mutations remove junction rows before inserting new
            // ones; re-evaluating a tag/query predicate after that delete would
            // match nothing. Materialize the selection once inside SQLite so
            // the SQL mutations stay predicate-driven but stable.
            self.materialize_selection_ids(&where_clause, &where_params)?;
            self.apply_tag_sql(&mutation)?;
            self.sync_fts_tags_for_selection()?;
            self.delete_orphan_tags()?;

            let summary = HistorySummary {
                affected: changed.len(),
                skipped: requested.saturating_sub(changed.len()),
                label: format!("{} tags on {} posts", mutation.kind_str(), changed.len()),
                tags: mutation.tags().to_vec(),
                files: Vec::new(),
            };
            let history_id = self.journal_tag(summary, &mutation, changed.clone())?;
            self.refresh_stats_snapshot()?;

            Ok(BulkResult {
                affected: changed.len(),
                skipped: requested.saturating_sub(changed.len()),
                affected_ids: changed.into_iter().map(|(id, _, _)| id).collect(),
                history_id: Some(history_id),
            })
        })();

        finish_transaction(&self.conn, result)
    }

    /// Delete every bookmark in the selection in one transaction.
    ///
    /// Delete payloads carry full bookmark snapshots so undo can reinsert the
    /// exact rows (tags, media, quote, read/favorite flags included).
    pub fn bulk_delete(&self, selection: &BookmarkSelection) -> Result<BulkResult> {
        let requested = requested_count(selection);

        self.conn.execute("BEGIN IMMEDIATE", [])?;
        let result = (|| -> Result<BulkResult> {
            let snapshots = self.selected_bookmarks(selection)?;
            if snapshots.is_empty() {
                return Ok(BulkResult::default());
            }
            let (where_clause, where_params) = self.selection_where(selection)?;

            self.trash_snapshots(&snapshots)?;
            self.delete_selection(&where_clause, &where_params)?;

            let summary = HistorySummary {
                affected: snapshots.len(),
                skipped: requested.saturating_sub(snapshots.len()),
                label: format!("deleted {} posts", snapshots.len()),
                tags: Vec::new(),
                files: Vec::new(),
            };
            let ids = snapshots
                .iter()
                .map(|bookmark| bookmark.id.clone())
                .collect::<Vec<_>>();
            let history_id = self.journal_delete(summary, snapshots)?;
            self.refresh_stats_snapshot()?;

            Ok(BulkResult {
                affected: ids.len(),
                skipped: requested.saturating_sub(ids.len()),
                affected_ids: ids,
                history_id: Some(history_id),
            })
        })();

        finish_transaction(&self.conn, result)
    }

    /// Mark every bookmark in the selection read or unread. Not journaled:
    /// undo covers imports, deletes, and tag edits only.
    pub fn bulk_set_read(&self, selection: &BookmarkSelection, read: bool) -> Result<BulkResult> {
        let requested = requested_count(selection);

        self.conn.execute("BEGIN IMMEDIATE", [])?;
        let result = (|| -> Result<BulkResult> {
            let (where_clause, where_params) = self.selection_where(selection)?;
            if where_clause == "0" {
                return Ok(BulkResult::default());
            }
            let ids = self.selected_ids(selection)?;
            if ids.is_empty() {
                return Ok(BulkResult::default());
            }

            let mut sql = String::from("UPDATE bookmarks SET is_read = ?");
            if !where_clause.is_empty() {
                sql.push_str(" WHERE ");
                sql.push_str(&where_clause);
            }
            let mut params: Vec<Value> = vec![Value::Integer(if read { 1 } else { 0 })];
            params.extend(where_params);
            let changed = self.conn.execute(&sql, params_from_iter(params.iter()))?;

            Ok(BulkResult {
                affected: changed,
                skipped: requested.saturating_sub(changed),
                affected_ids: ids,
                history_id: None,
            })
        })();

        finish_transaction(&self.conn, result)
    }

    /// Set one bookmark's read state.
    pub fn set_read(&self, id: &str, read: bool) -> Result<BulkResult> {
        self.bulk_set_read(&BookmarkSelection::single(id), read)
    }

    /// Replace a bookmark's tags without journaling, inside the caller's
    /// transaction. Used by tag undo to restore the exact before list.
    pub(crate) fn replace_bookmark_tags(&self, bookmark_id: &str, tags: &[String]) -> Result<()> {
        self.conn.execute(
            "DELETE FROM bookmark_tags WHERE bookmark_id = ?1",
            params![bookmark_id],
        )?;
        for tag in tags {
            self.conn.execute(
                "INSERT OR IGNORE INTO tags (name) VALUES (?1)",
                params![tag],
            )?;
            self.conn.execute(
                r#"INSERT OR IGNORE INTO bookmark_tags (bookmark_id, tag_id)
                   SELECT ?1, id FROM tags WHERE name = ?2"#,
                params![bookmark_id, tag],
            )?;
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
            params![bookmark_id],
        )?;
        Ok(())
    }

    /// Materialize the selected bookmark ids into a connection-local temp
    /// table. One SQL predicate selects them; no 10k bind parameters cross the
    /// API boundary and later statements can reuse a stable id set.
    fn materialize_selection_ids(&self, where_clause: &str, where_params: &[Value]) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS _eterea_selection (id TEXT PRIMARY KEY);
             DELETE FROM _eterea_selection;",
        )?;
        let predicate = if where_clause.is_empty() {
            "1".to_string()
        } else {
            where_clause.to_string()
        };
        let sql = format!(
            "INSERT OR IGNORE INTO _eterea_selection (id)              SELECT id FROM bookmarks WHERE {predicate}"
        );
        self.conn
            .execute(&sql, params_from_iter(where_params.iter()))?;
        Ok(())
    }

    /// SQL tag mutation over the materialized selection.
    fn apply_tag_sql(&self, mutation: &TagMutation) -> Result<()> {
        let selection = "SELECT id FROM _eterea_selection";
        match mutation {
            TagMutation::Add(tags) => {
                self.ensure_tags_exist(tags)?;
                let placeholders = vec!["?"; tags.len()].join(", ");
                let sql = format!(
                    r#"INSERT OR IGNORE INTO bookmark_tags (bookmark_id, tag_id)
                       SELECT b.id, t.id FROM bookmarks b, tags t
                       WHERE b.id IN ({selection})
                         AND t.name IN ({placeholders})"#
                );
                self.conn
                    .execute(&sql, params_from_iter(tags.iter().map(|tag| tag.as_str())))?;
            }
            TagMutation::Remove(tags) => {
                if tags.is_empty() {
                    return Ok(());
                }
                let placeholders = vec!["?"; tags.len()].join(", ");
                let sql = format!(
                    r#"DELETE FROM bookmark_tags
                       WHERE bookmark_id IN ({selection})
                         AND tag_id IN (SELECT id FROM tags WHERE name IN ({placeholders}))"#
                );
                self.conn
                    .execute(&sql, params_from_iter(tags.iter().map(|tag| tag.as_str())))?;
            }
            TagMutation::Set(tags) => {
                let delete_sql =
                    format!("DELETE FROM bookmark_tags WHERE bookmark_id IN ({selection})");
                self.conn.execute(&delete_sql, [])?;
                if !tags.is_empty() {
                    self.ensure_tags_exist(tags)?;
                    let placeholders = vec!["?"; tags.len()].join(", ");
                    let insert_sql = format!(
                        r#"INSERT OR IGNORE INTO bookmark_tags (bookmark_id, tag_id)
                           SELECT b.id, t.id FROM bookmarks b, tags t
                           WHERE b.id IN ({selection})
                             AND t.name IN ({placeholders})"#
                    );
                    self.conn.execute(
                        &insert_sql,
                        params_from_iter(tags.iter().map(|tag| tag.as_str())),
                    )?;
                }
            }
        }
        Ok(())
    }

    fn ensure_tags_exist(&self, tags: &[String]) -> Result<()> {
        let mut statement = self
            .conn
            .prepare_cached("INSERT OR IGNORE INTO tags (name) VALUES (?1)")?;
        for tag in tags {
            statement.execute(params![tag])?;
        }
        Ok(())
    }

    /// Recompute `bookmarks_search_docs.tags_text` for the materialized
    /// selection. The FTS triggers on the content table keep the search index
    /// in sync.
    fn sync_fts_tags_for_selection(&self) -> Result<()> {
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
               WHERE bookmark_id IN (SELECT id FROM _eterea_selection)"#,
            [],
        )?;
        Ok(())
    }

    /// Delete rows for a selection through SQL predicates (no id list).
    fn delete_selection(&self, where_clause: &str, where_params: &[Value]) -> Result<()> {
        let predicate = if where_clause.is_empty() {
            "1".to_string()
        } else {
            where_clause.to_string()
        };
        self.conn.execute(
            &format!(
                "DELETE FROM bookmarks_search_docs \
                 WHERE bookmark_id IN (SELECT id FROM bookmarks WHERE {predicate})"
            ),
            params_from_iter(where_params.iter()),
        )?;
        self.conn.execute(
            &format!(
                "DELETE FROM quoted_tweets \
                 WHERE bookmark_id IN (SELECT id FROM bookmarks WHERE {predicate})"
            ),
            params_from_iter(where_params.iter()),
        )?;
        self.conn.execute(
            &format!("DELETE FROM bookmarks WHERE {predicate}"),
            params_from_iter(where_params.iter()),
        )?;
        Ok(())
    }

    /// Remove tag rows that no longer tag any bookmark.
    pub(crate) fn delete_orphan_tags(&self) -> Result<()> {
        self.conn.execute(
            "DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM bookmark_tags)",
            [],
        )?;
        Ok(())
    }

    /// Tags of many bookmarks in one query, ordered by tag name.
    pub(crate) fn tags_for_bookmark_ids(
        &self,
        ids: &[String],
    ) -> Result<HashMap<String, Vec<String>>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = vec!["?"; ids.len()].join(", ");
        let sql = format!(
            r#"SELECT bt.bookmark_id, t.name
               FROM bookmark_tags bt
               JOIN tags t ON t.id = bt.tag_id
               WHERE bt.bookmark_id IN ({placeholders})
               ORDER BY bt.bookmark_id, t.name"#
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(ids.iter()))?;
        let mut tags: HashMap<String, Vec<String>> = HashMap::new();
        while let Some(row) = rows.next()? {
            let bookmark_id: String = row.get(0)?;
            let tag: String = row.get(1)?;
            tags.entry(bookmark_id).or_default().push(tag);
        }
        Ok(tags)
    }
}

fn apply_mutation_to_tags(before: &[String], mutation: &TagMutation) -> Vec<String> {
    match mutation {
        TagMutation::Set(tags) => tags.clone(),
        TagMutation::Add(tags) => {
            let mut result = before.to_vec();
            let existing: HashSet<String> = before.iter().map(|tag| tag.to_lowercase()).collect();
            for tag in tags {
                if !existing.contains(&tag.to_lowercase()) && !result.contains(tag) {
                    result.push(tag.clone());
                }
            }
            result.sort_by_key(|tag| tag.to_lowercase());
            result
        }
        TagMutation::Remove(tags) => {
            let remove: HashSet<String> = tags.iter().map(|tag| tag.to_lowercase()).collect();
            before
                .iter()
                .filter(|tag| !remove.contains(&tag.to_lowercase()))
                .cloned()
                .collect()
        }
    }
}

fn same_tags_ci(left: &[String], right: &[String]) -> bool {
    let left = left
        .iter()
        .map(|tag| tag.to_lowercase())
        .collect::<HashSet<_>>();
    let right = right
        .iter()
        .map(|tag| tag.to_lowercase())
        .collect::<HashSet<_>>();
    left == right
}

fn requested_count(selection: &BookmarkSelection) -> usize {
    match selection {
        BookmarkSelection::Ids(ids) => {
            let mut seen = HashSet::new();
            ids.iter()
                .filter(|id| !id.trim().is_empty())
                .filter(|id| seen.insert(id.to_string()))
                .count()
        }
        BookmarkSelection::Query(_) => 0,
    }
}

fn finish_transaction<T>(conn: &rusqlite::Connection, result: Result<T>) -> Result<T> {
    match result {
        Ok(value) => {
            conn.execute("COMMIT", [])?;
            Ok(value)
        }
        Err(error) => {
            let _ = conn.execute("ROLLBACK", []);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::BookmarkBuilder;
    use crate::storage::BookmarkQuery;
    use crate::Bookmark;
    use chrono::{TimeZone, Utc};
    use std::collections::BTreeMap;

    fn fixture() -> Vec<Bookmark> {
        let mut items = Vec::new();
        for (index, (handle, tag)) in [("ada", "rust"), ("bea", "design"), ("cy", "ops")]
            .into_iter()
            .enumerate()
        {
            items.push(
                BookmarkBuilder::new()
                    .tweet_url(format!("https://x.com/{handle}/status/{}", index + 1))
                    .content(format!("mutation fixture {index}"))
                    .tweeted_at(Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap())
                    .author_handle(handle)
                    .author_name(handle)
                    .add_tag(tag)
                    .build()
                    .unwrap(),
            );
        }
        items
    }

    #[test]
    fn set_tags_keeps_junction_fts_and_stats_in_sync() {
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&fixture()).unwrap();
        let target = db.get_bookmarks_by_tag("rust", 0, 10).unwrap().remove(0);

        let result = db
            .set_bookmark_tags(&target.id, &["systems".to_string(), "design".to_string()])
            .unwrap();
        assert_eq!(result.affected, 1);
        assert!(result.history_id.is_some());

        let tags = db.load_bookmark_tags(&target.id).unwrap();
        assert_eq!(tags, vec!["design".to_string(), "systems".to_string()]);

        // Search over the tag text follows the change.
        assert_eq!(db.search("systems", 10).unwrap().len(), 1);
        assert!(db.search("rust", 10).unwrap().is_empty());

        let stats = db.get_stats().unwrap();
        let top: BTreeMap<_, _> = stats.top_tags.into_iter().collect();
        assert_eq!(top.get("systems"), Some(&1));
        assert_eq!(top.get("design"), Some(&2));
        assert_eq!(top.get("rust"), None, "orphan tags are removed");
    }

    #[test]
    fn set_tags_over_a_tag_filtered_query_replaces_the_tag() {
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&fixture()).unwrap();

        let result = db
            .apply_tag_mutation(
                &BookmarkSelection::Query(BookmarkQuery {
                    tag: Some("rust".to_string()),
                    ..BookmarkQuery::default()
                }),
                &TagMutation::Set(vec!["systems".to_string()]),
            )
            .unwrap();
        assert_eq!(result.affected, 1);

        assert_eq!(db.get_bookmarks_by_tag("rust", 0, 10).unwrap().len(), 0);
        let systems = db.get_bookmarks_by_tag("systems", 0, 10).unwrap();
        assert_eq!(systems.len(), 1);
        assert_eq!(systems[0].tags, vec!["systems".to_string()]);
    }

    #[test]
    fn no_op_tag_mutations_do_not_journal() {
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&fixture()).unwrap();
        let events_before = db.history_list(0, 10, true).unwrap().1;

        let target = db.get_bookmarks(0, 10).unwrap().remove(0);
        let current = target.tags.clone();
        let result = db
            .apply_tag_mutation(
                &BookmarkSelection::single(&target.id),
                &TagMutation::Add(current),
            )
            .unwrap();

        assert_eq!(result.affected, 0);
        assert!(result.history_id.is_none());
        assert_eq!(db.history_list(0, 10, true).unwrap().1, events_before);
    }

    #[test]
    fn bulk_tag_over_query_uses_predicate_and_journals_before_after() {
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&fixture()).unwrap();

        let selection = BookmarkSelection::Query(BookmarkQuery {
            author: Some("ada".to_string()),
            ..BookmarkQuery::default()
        });
        let result = db
            .apply_tag_mutation(&selection, &TagMutation::Add(vec!["systems".to_string()]))
            .unwrap();
        assert_eq!(result.affected, 1);
        assert_eq!(
            db.get_bookmarks_by_tag("systems", 0, 10).unwrap().len(),
            1,
            "new tag must be visible to the tag filter"
        );
        let history_id = result.history_id.expect("bulk tag journals");

        let detail = db.history_detail(history_id).unwrap().unwrap();
        assert_eq!(
            detail.items[0].before,
            Some(crate::models::HistoryState::Tags(vec!["rust".to_string()]))
        );

        // Undo restores the exact before tag list.
        let undo = db.undo_latest().unwrap();
        assert_eq!(undo.affected, 1);
        assert_eq!(
            db.load_bookmark_tags(&result.affected_ids[0]).unwrap(),
            vec!["rust".to_string()]
        );
    }

    #[test]
    fn bulk_delete_query_journals_snapshots_and_undo_reinserts_exact_rows() {
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&fixture()).unwrap();
        let cy = db
            .get_bookmarks(0, 10)
            .unwrap()
            .into_iter()
            .find(|bookmark| bookmark.author_handle == "cy")
            .expect("cy fixture bookmark");
        db.set_favorite(&cy.id, true).unwrap();

        let selection = BookmarkSelection::Query(BookmarkQuery {
            favorites_only: true,
            ..BookmarkQuery::default()
        });
        let result = db.bulk_delete(&selection).unwrap();
        assert_eq!(result.affected, 1);
        assert_eq!(db.count_bookmarks().unwrap(), 2);

        let undo = db.undo_latest().unwrap();
        assert_eq!(undo.affected, 1);
        assert_eq!(db.count_bookmarks().unwrap(), 3);
        let restored = db.get_bookmarks(0, 10).unwrap();
        let restored = restored
            .iter()
            .find(|bookmark| bookmark.is_favorite)
            .expect("favorite snapshot restored");
        assert_eq!(restored.author_handle, "cy");
        assert_eq!(restored.tags, vec!["ops".to_string()]);
    }

    #[test]
    fn undo_delete_skips_conflicts_and_reports_them() {
        let db = Database::open_memory().unwrap();
        let books = fixture();
        db.insert_bookmarks(&books).unwrap();
        let ids = books.iter().map(|bookmark| bookmark.id.clone()).collect();
        db.bulk_delete(&BookmarkSelection::Ids(ids)).unwrap();
        assert_eq!(db.count_bookmarks().unwrap(), 0);

        // A conflicting row with the same tweet URL but a different id is
        // written directly, bypassing the journal: delete undo must skip it.
        let conflicting = books[0].clone();
        db.conn
            .execute(
                r#"INSERT INTO bookmarks
                   (id, tweet_url, content, tweeted_at, imported_at, author_handle, author_name)
                   VALUES ('manual-conflict', ?1, 'conflict', 1717243200, 1717243200, 'manual', 'Manual')"#,
                params![conflicting.tweet_url.clone()],
            )
            .unwrap();

        let undo = db.undo_latest().unwrap();
        assert_eq!(undo.affected, 2);
        assert_eq!(undo.skipped, 1);
        assert_eq!(undo.skipped_ids, vec![conflicting.id.clone()]);
        assert_eq!(db.count_bookmarks().unwrap(), 3);
    }

    #[test]
    fn deleting_a_missing_bookmark_is_a_noop_that_does_not_journal() {
        let db = Database::open_memory().unwrap();
        assert!(!db.delete_bookmark("does-not-exist").unwrap());
        assert_eq!(db.history_list(0, 10, true).unwrap().1, 0);
        let result = db
            .set_bookmark_tags("does-not-exist", &["x".to_string()])
            .unwrap();
        assert_eq!(result.affected, 0);
        assert!(result.history_id.is_none());
    }

    #[test]
    fn bulk_set_read_marks_and_counts() {
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&fixture()).unwrap();

        let result = db
            .bulk_set_read(
                &BookmarkSelection::Query(BookmarkQuery {
                    author: Some("ada".to_string()),
                    ..BookmarkQuery::default()
                }),
                true,
            )
            .unwrap();
        assert_eq!(result.affected, 1);
        let unread = db
            .search_query_page(&BookmarkQuery {
                unread_only: true,
                limit: 10,
                ..BookmarkQuery::default()
            })
            .unwrap();
        assert_eq!(unread.1, 2);
    }
}
