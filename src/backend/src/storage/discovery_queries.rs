//! Discovery read queries: tag graph data and calendar day data.
//!
//! Both are read-only aggregates over bookmarks/tags. They never mutate and
//! never depend on the UI; the app layer turns these rows into deterministic
//! display data.

use super::database::Database;
use crate::Result;
use rusqlite::params;
use serde::{Deserialize, Serialize};

/// One undirected tag co-occurrence edge (tag names sorted, count >= 1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TagCooccurrence {
    pub first_tag: String,
    pub second_tag: String,
    pub bookmark_count: i64,
}

impl Database {
    /// Tags ordered by bookmark count (descending) then name, for graph nodes.
    ///
    /// Only tags that still tag at least one saved post are returned: deleting
    /// a post leaves its `tags` rows behind for the explicit prune action, but
    /// an unlinked tag must never render as a graph node.
    pub fn get_top_tags(&self, limit: usize) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT t.name, COUNT(bt.bookmark_id) AS bookmark_count
               FROM tags t
               INNER JOIN bookmark_tags bt ON bt.tag_id = t.id
               GROUP BY t.id
               ORDER BY bookmark_count DESC, t.name ASC
               LIMIT ?1"#,
        )?;

        let tags = stmt
            .query_map(params![limit as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(tags)
    }

    /// Tag pairs that appear on the same bookmark, most shared posts first.
    ///
    /// Names inside a pair are sorted alphabetically, so the graph never
    /// duplicates an undirected edge and orientation does not depend on tag
    /// insertion order.
    pub fn get_tag_cooccurrences(&self, limit: usize) -> Result<Vec<TagCooccurrence>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT
                   CASE WHEN first_tag.name < second_tag.name THEN first_tag.name ELSE second_tag.name END AS pair_first,
                   CASE WHEN first_tag.name < second_tag.name THEN second_tag.name ELSE first_tag.name END AS pair_second,
                   COUNT(*) AS shared_bookmarks
               FROM bookmark_tags a
               JOIN bookmark_tags b
                 ON a.bookmark_id = b.bookmark_id
                AND a.tag_id < b.tag_id
               JOIN tags first_tag ON first_tag.id = a.tag_id
               JOIN tags second_tag ON second_tag.id = b.tag_id
               GROUP BY a.tag_id, b.tag_id
               ORDER BY shared_bookmarks DESC, pair_first ASC, pair_second ASC
               LIMIT ?1"#,
        )?;

        let edges = stmt
            .query_map(params![limit as i64], |row| {
                Ok(TagCooccurrence {
                    first_tag: row.get(0)?,
                    second_tag: row.get(1)?,
                    bookmark_count: row.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(edges)
    }

    /// Saved-post counts per UTC day for one calendar year.
    ///
    /// At most 366 rows; the range scans the `tweeted_at` index, so the year
    /// overview costs one cheap aggregate per request.
    pub fn get_calendar_year_counts(
        &self,
        year_start: i64,
        year_end: i64,
    ) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT date(tweeted_at, 'unixepoch') AS day, COUNT(*)
               FROM bookmarks
               WHERE tweeted_at >= ?1 AND tweeted_at < ?2
               GROUP BY day
               ORDER BY day ASC"#,
        )?;
        let rows = stmt
            .query_map(params![year_start, year_end], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Tag pairs restricted to the named tags, most shared posts first.
    ///
    /// The graph asks this instead of scanning every pair in the archive: with
    /// a thousand selected tags the search space stays inside the node set, so
    /// cost scales with the graph rather than the whole tag vocabulary.
    pub fn get_tag_cooccurrences_for_tags(
        &self,
        tags: &[String],
        limit: usize,
    ) -> Result<Vec<TagCooccurrence>> {
        if tags.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; tags.len()].join(", ");
        let sql = format!(
            r#"SELECT
                   CASE WHEN first_tag.name < second_tag.name THEN first_tag.name ELSE second_tag.name END AS pair_first,
                   CASE WHEN first_tag.name < second_tag.name THEN second_tag.name ELSE first_tag.name END AS pair_second,
                   COUNT(*) AS shared_bookmarks
               FROM bookmark_tags a
               JOIN bookmark_tags b
                 ON a.bookmark_id = b.bookmark_id
                AND a.tag_id < b.tag_id
               JOIN tags first_tag ON first_tag.id = a.tag_id
               JOIN tags second_tag ON second_tag.id = b.tag_id
               WHERE a.tag_id IN (SELECT id FROM tags WHERE name IN ({placeholders}))
                 AND b.tag_id IN (SELECT id FROM tags WHERE name IN ({placeholders}))
               GROUP BY a.tag_id, b.tag_id
               ORDER BY shared_bookmarks DESC, pair_first ASC, pair_second ASC
               LIMIT ?"#
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params = tags
            .iter()
            .map(|tag| tag.as_str())
            .chain(tags.iter().map(|tag| tag.as_str()))
            .chain(std::iter::once(""))
            .collect::<Vec<&str>>();
        // The LIMIT placeholder must be an integer, so bind it separately.
        let mut values: Vec<rusqlite::types::Value> = tags
            .iter()
            .map(|tag| rusqlite::types::Value::Text(tag.clone()))
            .collect();
        values.extend(
            tags.iter()
                .map(|tag| rusqlite::types::Value::Text(tag.clone())),
        );
        values.push(rusqlite::types::Value::Integer(limit as i64));
        let _ = params;
        let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), |row| {
            Ok(TagCooccurrence {
                first_tag: row.get(0)?,
                second_tag: row.get(1)?,
                bookmark_count: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// `(UTC day, bookmark id)` rows for bookmarks posted in a timestamp range.
    ///
    /// The day is rendered by SQLite as `YYYY-MM-DD` in UTC, matching the
    /// RFC 3339 UTC date filters used by feed queries. Rows are ordered by day
    /// ascending and posted time descending so the calendar can group them
    /// without sorting again.
    pub fn get_bookmark_day_entries(
        &self,
        from_timestamp: i64,
        to_timestamp: i64,
    ) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT date(tweeted_at, 'unixepoch') AS day, id
               FROM bookmarks
               WHERE tweeted_at >= ?1 AND tweeted_at <= ?2
               ORDER BY day ASC, tweeted_at DESC, id DESC"#,
        )?;

        let entries = stmt
            .query_map(params![from_timestamp, to_timestamp], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::BookmarkBuilder;
    use chrono::{TimeZone, Utc};

    fn bookmark(
        tweet_id: &str,
        tags: &[&str],
        date: chrono::DateTime<Utc>,
    ) -> crate::models::Bookmark {
        let mut builder = BookmarkBuilder::new()
            .tweet_url(format!("https://x.com/graph/status/{tweet_id}"))
            .content(format!("graph fixture {tweet_id}"))
            .tweeted_at(date)
            .author_handle("graph")
            .author_name("Graph");
        for tag in tags {
            builder = builder.add_tag(*tag);
        }
        builder.build().unwrap()
    }

    #[test]
    fn tag_graph_edges_are_undirected_and_ranked_by_shared_posts() {
        let db = Database::open_memory().unwrap();
        let date = Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap();
        db.insert_bookmarks(&[
            bookmark("1", &["rust", "systems"], date),
            bookmark("2", &["rust", "systems"], date),
            bookmark("3", &["rust", "design"], date),
        ])
        .unwrap();

        let nodes = db.get_top_tags(10).unwrap();
        assert_eq!(
            nodes,
            vec![
                ("rust".to_string(), 3),
                ("systems".to_string(), 2),
                ("design".to_string(), 1),
            ]
        );

        let edges = db.get_tag_cooccurrences(10).unwrap();
        assert_eq!(edges.len(), 2);
        assert_eq!(
            edges[0],
            TagCooccurrence {
                first_tag: "rust".to_string(),
                second_tag: "systems".to_string(),
                bookmark_count: 2,
            }
        );
        assert_eq!(
            edges[1],
            TagCooccurrence {
                first_tag: "design".to_string(),
                second_tag: "rust".to_string(),
                bookmark_count: 1,
            }
        );
    }

    #[test]
    fn unlinked_tags_are_not_graph_nodes() {
        let db = Database::open_memory().unwrap();
        let date = Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap();
        let shared = bookmark("1", &["rust", "systems"], date);
        let second_rust = bookmark("2", &["rust"], date);
        let design = bookmark("3", &["design"], date);
        db.insert_bookmarks(&[shared.clone(), second_rust.clone(), design.clone()])
            .unwrap();
        assert_eq!(
            db.get_top_tags(10).unwrap(),
            vec![
                ("rust".to_string(), 2),
                ("design".to_string(), 1),
                ("systems".to_string(), 1),
            ]
        );

        // The design tag has no other carrier, so deleting post 3 leaves an
        // unlinked `tags` row (pruning is an explicit action) that the graph
        // query must skip.
        assert!(db.delete_bookmark(&design.id).unwrap());
        assert_eq!(
            db.get_top_tags(10).unwrap(),
            vec![("rust".to_string(), 2), ("systems".to_string(), 1)]
        );

        // Counts shrink with the links, and an emptied tag drops out entirely.
        assert!(db.delete_bookmark(&second_rust.id).unwrap());
        assert_eq!(
            db.get_top_tags(10).unwrap(),
            vec![("rust".to_string(), 1), ("systems".to_string(), 1)]
        );
        assert!(db
            .get_top_tags(10)
            .unwrap()
            .iter()
            .all(|(tag, _)| tag != "design"));
        // The LIMIT still caps the ranked node list.
        assert_eq!(db.get_top_tags(1).unwrap(), vec![("rust".to_string(), 1)],);
    }

    #[test]
    fn calendar_year_counts_fill_days_and_stop_at_the_year_edges() {
        let db = Database::open_memory().unwrap();
        let in_year_a = bookmark("1", &[], Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 1).unwrap());
        let in_year_b = bookmark(
            "2",
            &[],
            Utc.with_ymd_and_hms(2025, 1, 1, 23, 59, 59).unwrap(),
        );
        let in_year_c = bookmark(
            "3",
            &[],
            Utc.with_ymd_and_hms(2025, 12, 31, 23, 59, 59).unwrap(),
        );
        let previous = bookmark(
            "4",
            &[],
            Utc.with_ymd_and_hms(2024, 12, 31, 23, 59, 59).unwrap(),
        );
        let next = bookmark("5", &[], Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap());
        db.insert_bookmarks(&[in_year_a, in_year_b, in_year_c, previous, next])
            .unwrap();

        let start = Utc
            .with_ymd_and_hms(2025, 1, 1, 0, 0, 0)
            .unwrap()
            .timestamp();
        let end = Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .unwrap()
            .timestamp();
        let counts = db.get_calendar_year_counts(start, end).unwrap();
        assert_eq!(
            counts,
            vec![("2025-01-01".to_string(), 2), ("2025-12-31".to_string(), 1),]
        );
    }

    #[test]
    fn calendar_day_entries_group_by_utc_day_and_respect_the_range() {
        let db = Database::open_memory().unwrap();
        let day_one = Utc.with_ymd_and_hms(2024, 5, 1, 8, 0, 0).unwrap();
        let day_one_later = Utc.with_ymd_and_hms(2024, 5, 1, 20, 0, 0).unwrap();
        let day_two = Utc.with_ymd_and_hms(2024, 5, 2, 9, 0, 0).unwrap();
        let first = bookmark("1", &["rust"], day_one);
        let second = bookmark("2", &["rust"], day_one_later);
        let third = bookmark("3", &["rust"], day_two);
        db.insert_bookmarks(&[first.clone(), second.clone(), third.clone()])
            .unwrap();

        let from = Utc
            .with_ymd_and_hms(2024, 5, 1, 0, 0, 0)
            .unwrap()
            .timestamp();
        let to = Utc
            .with_ymd_and_hms(2024, 5, 1, 23, 59, 59)
            .unwrap()
            .timestamp();
        let entries = db.get_bookmark_day_entries(from, to).unwrap();

        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|(day, _)| day == "2024-05-01"));
        assert_eq!(entries[0].1, second.id);
        assert_eq!(entries[1].1, first.id);
    }
}
