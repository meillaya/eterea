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
    pub fn get_top_tags(&self, limit: usize) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT t.name, COUNT(bt.bookmark_id) AS bookmark_count
               FROM tags t
               LEFT JOIN bookmark_tags bt ON bt.tag_id = t.id
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
