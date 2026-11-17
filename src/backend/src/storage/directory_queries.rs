//! Directory-style aggregate read queries.

use super::database::Database;
use super::queries::AuthorStats;
use crate::Result;

impl Database {
    pub fn get_all_authors(&self) -> Result<Vec<AuthorStats>> {
        let mut stmt = self.conn.prepare_cached(
            r#"SELECT author_handle,
                      author_name,
                      author_profile_image,
                      bookmark_count,
                      favorite_count
               FROM author_stats
               ORDER BY bookmark_count DESC, author_handle ASC"#,
        )?;

        let authors = stmt
            .query_map([], |row| {
                Ok(AuthorStats {
                    handle: row.get(0)?,
                    name: row.get(1)?,
                    profile_image: row.get(2)?,
                    bookmark_count: row.get(3)?,
                    favorite_count: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(authors)
    }

    /// Paged author directory with an optional case-insensitive name/handle
    /// filter. Returns `(authors, total)` where total ignores pagination.
    pub fn author_page(
        &self,
        offset: usize,
        limit: usize,
        filter: Option<&str>,
    ) -> Result<(Vec<AuthorStats>, i64)> {
        let pattern = filter
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(|value| format!("%{value}%"));

        let total: i64 = match &pattern {
            Some(pattern) => self.conn.query_row(
                "SELECT COUNT(*) FROM author_stats                  WHERE author_handle LIKE ?1 COLLATE NOCASE OR author_name LIKE ?1 COLLATE NOCASE",
                rusqlite::params![pattern],
                |row| row.get(0),
            )?,
            None => self
                .conn
                .query_row("SELECT COUNT(*) FROM author_stats", [], |row| row.get(0))?,
        };

        let sql = match &pattern {
            Some(_) => {
                r#"SELECT author_handle, author_name, author_profile_image,
                          bookmark_count, favorite_count
                   FROM author_stats
                   WHERE author_handle LIKE ?1 COLLATE NOCASE OR author_name LIKE ?1 COLLATE NOCASE
                   ORDER BY bookmark_count DESC, author_handle ASC
                   LIMIT ?2 OFFSET ?3"#
            }
            None => {
                r#"SELECT author_handle, author_name, author_profile_image,
                          bookmark_count, favorite_count
                   FROM author_stats
                   ORDER BY bookmark_count DESC, author_handle ASC
                   LIMIT ?1 OFFSET ?2"#
            }
        };
        let mut stmt = self.conn.prepare(sql)?;
        let authors = match &pattern {
            Some(pattern) => stmt
                .query_map(
                    rusqlite::params![pattern, limit as i64, offset as i64],
                    author_stats_from_row,
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?,
            None => stmt
                .query_map(
                    rusqlite::params![limit as i64, offset as i64],
                    author_stats_from_row,
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?,
        };
        Ok((authors, total))
    }

    /// Paged topic directory with an optional case-insensitive tag filter.
    pub fn topic_page(
        &self,
        offset: usize,
        limit: usize,
        filter: Option<&str>,
    ) -> Result<(Vec<(String, i64)>, i64)> {
        let pattern = filter
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(|value| format!("%{value}%"));

        let total: i64 = match &pattern {
            Some(pattern) => self.conn.query_row(
                "SELECT COUNT(*) FROM tags WHERE name LIKE ?1 COLLATE NOCASE",
                rusqlite::params![pattern],
                |row| row.get(0),
            )?,
            None => self
                .conn
                .query_row("SELECT COUNT(*) FROM tags", [], |row| row.get(0))?,
        };

        let sql = match &pattern {
            Some(_) => {
                r#"SELECT t.name, COUNT(bt.bookmark_id) AS bookmark_count
                   FROM tags t
                   LEFT JOIN bookmark_tags bt ON bt.tag_id = t.id
                   WHERE t.name LIKE ?1 COLLATE NOCASE
                   GROUP BY t.id
                   ORDER BY bookmark_count DESC, t.name ASC
                   LIMIT ?2 OFFSET ?3"#
            }
            None => {
                r#"SELECT t.name, COUNT(bt.bookmark_id) AS bookmark_count
                   FROM tags t
                   LEFT JOIN bookmark_tags bt ON bt.tag_id = t.id
                   GROUP BY t.id
                   ORDER BY bookmark_count DESC, t.name ASC
                   LIMIT ?1 OFFSET ?2"#
            }
        };
        let mut stmt = self.conn.prepare(sql)?;
        let topics = match &pattern {
            Some(pattern) => stmt
                .query_map(
                    rusqlite::params![pattern, limit as i64, offset as i64],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?,
            None => stmt
                .query_map(rusqlite::params![limit as i64, offset as i64], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?,
        };
        Ok((topics, total))
    }

    /// Get all unique tags with counts
    pub fn get_all_tags(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT t.name, COUNT(bt.bookmark_id) as count
               FROM tags t
               LEFT JOIN bookmark_tags bt ON bt.tag_id = t.id
               GROUP BY t.id
               ORDER BY count DESC"#,
        )?;

        let tags = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(tags)
    }
}

fn author_stats_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuthorStats> {
    Ok(AuthorStats {
        handle: row.get(0)?,
        name: row.get(1)?,
        profile_image: row.get(2)?,
        bookmark_count: row.get(3)?,
        favorite_count: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::BookmarkBuilder;
    use chrono::{TimeZone, Utc};

    #[test]
    fn author_and_topic_pages_filter_and_paginate() {
        let db = Database::open_memory().unwrap();
        let mut bookmarks = Vec::new();
        for (index, handle) in ["ada", "bea", "cy"].into_iter().enumerate() {
            bookmarks.push(
                BookmarkBuilder::new()
                    .tweet_url(format!("https://x.com/{handle}/status/{index}"))
                    .content(format!("directory {handle}"))
                    .tweeted_at(Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap())
                    .author_handle(handle)
                    .author_name(handle)
                    .add_tag(if index == 0 { "rust" } else { "design" })
                    .build()
                    .unwrap(),
            );
        }
        db.insert_bookmarks(&bookmarks).unwrap();

        let (authors, total) = db.author_page(0, 2, None).unwrap();
        assert_eq!(total, 3);
        assert_eq!(authors.len(), 2);

        let (filtered, total) = db.author_page(0, 10, Some("be")).unwrap();
        assert_eq!(total, 1);
        assert_eq!(filtered[0].handle, "bea");

        let (topics, total) = db.topic_page(0, 10, None).unwrap();
        assert_eq!(total, 2);
        assert!(topics
            .iter()
            .any(|(tag, count)| tag == "rust" && *count == 1));

        let (filtered_topics, total) = db.topic_page(0, 10, Some("DES")).unwrap();
        assert_eq!(total, 1);
        assert_eq!(filtered_topics[0].0, "design");
    }
}
