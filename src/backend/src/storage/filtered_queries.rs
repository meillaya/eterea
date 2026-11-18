//! Full-text and filtered bookmark queries.
//!
//! One SQL predicate builder serves paged reads, bulk mutations, and exports,
//! so a filter that matches in the feed also matches in a bulk delete. The
//! predicate has no table alias and never uses outer JOINs: FTS and tag filters
//! are `IN`-subqueries, which keeps row counts exact without `DISTINCT`.

use super::database::Database;
use super::query::{BookmarkQuery, BookmarkSelection};
use crate::models::Bookmark;
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter};
use std::collections::BTreeSet;
use tracing::debug;

impl Database {
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Bookmark>> {
        let query = Self::prepare_fts_query(query);

        let mut stmt = self.conn.prepare(
            r#"SELECT b.id, b.tweet_url, b.content, b.note_text, b.tweeted_at, b.imported_at,
                      b.author_handle, b.author_name, b.author_profile_url, b.author_profile_image,
                      b.comments, b.is_favorite, b.is_read, b.links_json
               FROM bookmarks b
               JOIN bookmarks_search_docs fc ON fc.bookmark_id = b.id
               JOIN bookmarks_fts fts ON fts.rowid = fc.rowid
               WHERE bookmarks_fts MATCH ?1
               ORDER BY bm25(bookmarks_fts), b.tweeted_at DESC, b.id DESC
               LIMIT ?2"#,
        )?;

        let mut bookmarks: Vec<Bookmark> = stmt
            .query_map(params![query, limit as i64], |row| {
                self.row_to_bookmark(row)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        self.hydrate_bookmarks(&mut bookmarks)?;

        Ok(bookmarks)
    }

    /// Prepare FTS5 query (add prefix matching for better UX)
    pub(crate) fn prepare_fts_query(query: &str) -> String {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|term| {
                // Escape special FTS5 characters
                let escaped = term.replace('"', "\"\"");
                format!("\"{}\"*", escaped)
            })
            .collect();

        terms.join(" ")
    }

    #[allow(clippy::too_many_arguments)]
    pub fn search_with_filters(
        &self,
        query: Option<&str>,
        tag: Option<&str>,
        author: Option<&str>,
        from_date: Option<DateTime<Utc>>,
        to_date: Option<DateTime<Utc>>,
        favorites_only: bool,
        has_media: Option<bool>,
        limit: usize,
    ) -> Result<Vec<Bookmark>> {
        let (bookmarks, _) = self.search_with_filters_page(
            query,
            tag,
            author,
            from_date,
            to_date,
            favorites_only,
            has_media,
            0,
            limit,
        )?;

        Ok(bookmarks)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn search_with_filters_page(
        &self,
        query: Option<&str>,
        tag: Option<&str>,
        author: Option<&str>,
        from_date: Option<DateTime<Utc>>,
        to_date: Option<DateTime<Utc>>,
        favorites_only: bool,
        has_media: Option<bool>,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<Bookmark>, i64)> {
        let (where_clause, params) = self.build_raw_predicate(
            query,
            tag,
            author,
            from_date.map(|date| date.timestamp()),
            to_date.map(|date| date.timestamp()),
            favorites_only,
            has_media,
            false,
        );
        let rank = query
            .filter(|query| !query.trim().is_empty())
            .map(Self::prepare_fts_query);
        self.run_filtered_page(where_clause, params, offset, limit, rank)
    }

    /// Paged read for the public [`BookmarkQuery`] shape. Parses RFC 3339
    /// date filters once and applies the shared SQL predicate.
    ///
    /// A text query orders matches by FTS `bm25` relevance (then date, then
    /// id), so the best match is on the first page instead of the newest one.
    pub fn search_query_page(&self, query: &BookmarkQuery) -> Result<(Vec<Bookmark>, i64)> {
        let rank = query
            .normalized_query()
            .map(|query| Self::prepare_fts_query(&query));
        // When ranking, the FTS match lives in the ranking join; keeping the
        // same condition in the predicate would duplicate both work and the
        // bound term. Build the remaining filters without the text query.
        let (where_clause, params) = if rank.is_some() {
            let mut rest = query.clone();
            rest.query = None;
            self.build_query_predicate(&rest)?
        } else {
            self.build_query_predicate(query)?
        };
        self.run_filtered_page(where_clause, params, query.offset, query.limit, rank)
    }

    fn run_filtered_page(
        &self,
        where_clause: String,
        params: Vec<Value>,
        offset: usize,
        limit: usize,
        rank_query: Option<String>,
    ) -> Result<(Vec<Bookmark>, i64)> {
        let overall_started = std::time::Instant::now();

        let mut sql = String::from(
            r#"SELECT b.id, b.tweet_url, b.content, b.note_text, b.tweeted_at, b.imported_at,
                      b.author_handle, b.author_name, b.author_profile_url, b.author_profile_image,
                      b.comments, b.is_favorite, b.is_read, b.links_json
               FROM bookmarks b"#,
        );
        let mut conditions = String::new();
        let mut joins = String::new();
        if rank_query.is_some() {
            joins.push_str(
                " JOIN (SELECT fc.bookmark_id AS ranked_bookmark_id, \
                       bm25(bookmarks_fts) AS match_rank \
                       FROM bookmarks_fts fts \
                       JOIN bookmarks_search_docs fc ON fc.rowid = fts.rowid \
                       WHERE bookmarks_fts MATCH ?) ranked \
                 ON ranked.ranked_bookmark_id = b.id",
            );
            sql.push_str(&joins);
        }
        if !where_clause.is_empty() {
            if !conditions.is_empty() {
                conditions.push_str(" AND (");
                conditions.push_str(&where_clause);
                conditions.push(')');
            } else {
                conditions.push_str(&where_clause);
            }
        }
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions);
        }
        let mut query_params = params;
        if let Some(ranked) = rank_query {
            query_params.insert(0, Value::Text(ranked));
            sql.push_str(" ORDER BY ranked.match_rank, b.tweeted_at DESC, b.id DESC");
        } else {
            sql.push_str(" ORDER BY b.tweeted_at DESC, b.id DESC");
        }
        sql.push_str(" LIMIT ? OFFSET ?");

        let count_params = query_params.clone();
        let mut page_params = query_params;
        page_params.push(Value::Integer(limit as i64));
        page_params.push(Value::Integer(offset as i64));

        let mut stmt = self.conn.prepare(&sql)?;
        let query_started = std::time::Instant::now();

        let mut bookmarks = Vec::new();
        let mut rows = stmt.query(params_from_iter(page_params.iter()))?;
        while let Some(row) = rows.next()? {
            bookmarks.push(self.row_to_bookmark(row)?);
        }
        let query_elapsed = query_started.elapsed();

        // Site the total independently: FTS5's bm25() cannot be combined with a
        // window function, and a page past the last row has no rows from which
        // to read a windowed total anyway.
        let count_sql = format!("SELECT COUNT(*) FROM bookmarks b{joins}");
        let count_sql = if conditions.is_empty() {
            count_sql
        } else {
            format!("{count_sql} WHERE {conditions}")
        };
        let total: i64 =
            self.conn
                .query_row(&count_sql, params_from_iter(count_params.iter()), |row| {
                    row.get(0)
                })?;

        let hydrate_started = std::time::Instant::now();
        self.hydrate_bookmarks(&mut bookmarks)?;
        debug!(
            target: "eterea::db",
            offset,
            limit,
            total,
            rows = bookmarks.len(),
            query_ms = query_elapsed.as_millis(),
            hydrate_ms = hydrate_started.elapsed().as_millis(),
            total_ms = overall_started.elapsed().as_millis(),
            "loaded filtered bookmark page"
        );

        Ok((bookmarks, total))
    }

    /// Build the SQL predicate for a [`BookmarkQuery`] plus its bind values.
    /// An empty predicate means "every bookmark".
    pub(crate) fn build_query_predicate(
        &self,
        query: &BookmarkQuery,
    ) -> Result<(String, Vec<Value>)> {
        let from_date = parse_rfc3339_date(query.from_date.as_deref())?;
        let to_date = parse_rfc3339_date(query.to_date.as_deref())?;
        Ok(self.build_raw_predicate(
            query.normalized_query().as_deref(),
            query.tag.as_deref(),
            query.author.as_deref(),
            from_date.map(|date| date.timestamp()),
            to_date.map(|date| date.timestamp()),
            query.favorites_only,
            query.has_media,
            query.unread_only,
        ))
    }

    /// Build a WHERE clause using subqueries. No outer JOINs means no DISTINCT needed.
    /// FTS and tag filters use IN-subqueries; has_media uses the denormalized column.
    #[allow(clippy::too_many_arguments)]
    fn build_raw_predicate(
        &self,
        query: Option<&str>,
        tag: Option<&str>,
        author: Option<&str>,
        from_timestamp: Option<i64>,
        to_timestamp: Option<i64>,
        favorites_only: bool,
        has_media: Option<bool>,
        unread_only: bool,
    ) -> (String, Vec<Value>) {
        let mut conditions = Vec::<String>::new();
        let mut params = Vec::<Value>::new();

        if let Some(q) = query {
            if !q.trim().is_empty() {
                // FTS via subquery: query the virtual table first (its optimized MATCH path),
                // then look up bookmark_id via the rowid link to our content table.
                // No outer JOIN → no row multiplication, no DISTINCT needed.
                conditions.push(
                    "id IN (SELECT fc.bookmark_id FROM bookmarks_fts fts \
                     JOIN bookmarks_search_docs fc ON fc.rowid = fts.rowid \
                     WHERE bookmarks_fts MATCH ?)"
                        .to_string(),
                );
                params.push(Value::Text(Self::prepare_fts_query(q)));
            }
        }

        if let Some(t) = tag {
            // Tag via subquery: no outer JOIN, no row multiplication
            conditions.push(
                "id IN (SELECT bt.bookmark_id FROM bookmark_tags bt \
                 JOIN tags t ON t.id = bt.tag_id WHERE t.name = ?)"
                    .to_string(),
            );
            params.push(Value::Text(t.to_string()));
        }

        if let Some(a) = author {
            conditions.push("author_handle = ?".to_string());
            params.push(Value::Text(a.to_string()));
        }

        if let Some(from) = from_timestamp {
            conditions.push("tweeted_at >= ?".to_string());
            params.push(Value::Integer(from));
        }
        if let Some(to) = to_timestamp {
            conditions.push("tweeted_at <= ?".to_string());
            params.push(Value::Integer(to));
        }

        if favorites_only {
            conditions.push("is_favorite = 1".to_string());
        }

        if unread_only {
            conditions.push("is_read = 0".to_string());
        }

        if let Some(has) = has_media {
            // Use the denormalized column so no JOIN is needed.
            conditions.push(format!("has_media = {}", if has { 1 } else { 0 }));
        }

        (conditions.join(" AND "), params)
    }

    /// WHERE clause plus bind values selecting a bookmark set.
    ///
    /// `Ids` uses one `IN` list; `Query` uses SQL predicates so a bulk
    /// mutation never materializes 10,000 ids just to match a filter.
    pub(crate) fn selection_where(
        &self,
        selection: &BookmarkSelection,
    ) -> Result<(String, Vec<Value>)> {
        match selection {
            BookmarkSelection::Ids(ids) => {
                let ids = normalize_ids(ids);
                if ids.is_empty() {
                    return Ok(("0".to_string(), Vec::new()));
                }
                let placeholders = vec!["?"; ids.len()].join(", ");
                Ok((
                    format!("id IN ({placeholders})"),
                    ids.into_iter().map(Value::Text).collect(),
                ))
            }
            BookmarkSelection::Query(query) => self.build_query_predicate(query),
        }
    }

    /// Resolve the existing bookmark ids a selection touches, newest first.
    ///
    /// Used for undo payloads and for reporting exact affected/skipped counts;
    /// the mutation itself still runs through [`Database::selection_where`].
    pub(crate) fn selected_ids(&self, selection: &BookmarkSelection) -> Result<Vec<String>> {
        match selection {
            BookmarkSelection::Ids(ids) => {
                let ids = normalize_ids(ids);
                if ids.is_empty() {
                    return Ok(Vec::new());
                }
                let placeholders = vec!["?"; ids.len()].join(", ");
                let sql = format!(
                    "SELECT id FROM bookmarks WHERE id IN ({placeholders}) \
                     ORDER BY tweeted_at DESC, id DESC"
                );
                let mut stmt = self.conn.prepare(&sql)?;
                let params = ids.into_iter().map(Value::Text).collect::<Vec<_>>();
                let rows = stmt
                    .query_map(params_from_iter(params.iter()), |row| row.get(0))?
                    .collect::<std::result::Result<Vec<String>, _>>()?;
                Ok(rows)
            }
            BookmarkSelection::Query(query) => {
                let (where_clause, params) = self.build_query_predicate(query)?;
                let mut sql = String::from("SELECT id FROM bookmarks");
                if !where_clause.is_empty() {
                    sql.push_str(" WHERE ");
                    sql.push_str(&where_clause);
                }
                sql.push_str(" ORDER BY tweeted_at DESC, id DESC");
                let mut stmt = self.conn.prepare(&sql)?;
                let rows = stmt
                    .query_map(params_from_iter(params.iter()), |row| row.get(0))?
                    .collect::<std::result::Result<Vec<String>, _>>()?;
                Ok(rows)
            }
        }
    }

    /// Count how many of `urls` already exist in the archive.
    ///
    /// Used by dry-run import previews (duplicate estimate) and chunked so a
    /// huge file's URL list never exceeds SQLite's bind-variable limit.
    pub fn count_existing_tweet_urls(&self, urls: &[String]) -> Result<usize> {
        const CHUNK: usize = 500;
        let unique = urls
            .iter()
            .filter(|url| !url.trim().is_empty())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        let mut existing = 0usize;
        for chunk in unique.chunks(CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let sql = format!("SELECT COUNT(*) FROM bookmarks WHERE tweet_url IN ({placeholders})");
            let count: i64 = self.conn.query_row(
                &sql,
                params_from_iter(chunk.iter().map(|url| url.as_str())),
                |row| row.get(0),
            )?;
            existing += count as usize;
        }
        Ok(existing)
    }

    /// Load the full (hydrated) bookmark snapshots a selection touches.
    ///
    /// Delete undo needs exact snapshots; tag undo needs the before tag lists.
    pub(crate) fn selected_bookmarks(
        &self,
        selection: &BookmarkSelection,
    ) -> Result<Vec<Bookmark>> {
        let (where_clause, params) = self.selection_where(selection)?;
        if where_clause == "0" {
            return Ok(Vec::new());
        }

        let mut sql = String::from(
            r#"SELECT b.id, b.tweet_url, b.content, b.note_text, b.tweeted_at, b.imported_at,
                      b.author_handle, b.author_name, b.author_profile_url, b.author_profile_image,
                      b.comments, b.is_favorite, b.is_read, b.links_json
               FROM bookmarks b"#,
        );
        if !where_clause.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&where_clause);
        }
        sql.push_str(" ORDER BY b.tweeted_at DESC, b.id DESC");

        let mut stmt = self.conn.prepare(&sql)?;
        let mut bookmarks: Vec<Bookmark> = stmt
            .query_map(params_from_iter(params.iter()), |row| {
                self.row_to_bookmark(row)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        self.hydrate_bookmarks(&mut bookmarks)?;
        Ok(bookmarks)
    }
}

fn parse_rfc3339_date(value: Option<&str>) -> Result<Option<DateTime<Utc>>> {
    value
        .filter(|candidate| !candidate.trim().is_empty())
        .map(|candidate| {
            DateTime::parse_from_rfc3339(candidate.trim())
                .map(|parsed| parsed.with_timezone(&Utc))
                .map_err(|error| {
                    Error::Other(format!("invalid RFC3339 date '{candidate}': {error}"))
                })
        })
        .transpose()
}

fn normalize_ids(ids: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    ids.iter()
        .filter(|id| !id.trim().is_empty())
        .filter(|id| seen.insert(id.to_string()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::BookmarkBuilder;
    use chrono::TimeZone;

    fn fixture() -> Vec<Bookmark> {
        let mut rust = BookmarkBuilder::new()
            .tweet_url("https://x.com/ada/status/1")
            .content("Rust service boundaries #rust")
            .tweeted_at(Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap())
            .author_handle("ada")
            .author_name("Ada")
            .add_tag("rust")
            .build()
            .unwrap();
        rust.is_read = true;
        let design = BookmarkBuilder::new()
            .tweet_url("https://x.com/bea/status/2")
            .content("Editorial layouts #design")
            .tweeted_at(Utc.with_ymd_and_hms(2024, 5, 2, 12, 0, 0).unwrap())
            .author_handle("bea")
            .author_name("Bea")
            .add_tag("design")
            .build()
            .unwrap();
        vec![rust, design]
    }

    #[test]
    fn struct_query_applies_all_filters_including_unread() {
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&fixture()).unwrap();

        let page = db
            .search_query_page(&BookmarkQuery {
                limit: 10,
                ..BookmarkQuery::default()
            })
            .unwrap();
        assert_eq!(page.1, 2);

        let unread = db
            .search_query_page(&BookmarkQuery {
                unread_only: true,
                limit: 10,
                ..BookmarkQuery::default()
            })
            .unwrap();
        assert_eq!(unread.1, 1);
        assert_eq!(unread.0[0].author_handle, "bea");

        let by_tag = db
            .search_query_page(&BookmarkQuery {
                tag: Some("rust".to_string()),
                limit: 10,
                ..BookmarkQuery::default()
            })
            .unwrap();
        assert_eq!(by_tag.1, 1);

        let bad_date = db.search_query_page(&BookmarkQuery {
            from_date: Some("not-a-date".to_string()),
            ..BookmarkQuery::default()
        });
        assert!(bad_date.is_err(), "invalid dates must be typed errors");
    }

    #[test]
    fn selection_where_never_materializes_query_ids() {
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&fixture()).unwrap();

        let (where_clause, params) = db
            .selection_where(&BookmarkSelection::Query(BookmarkQuery {
                tag: Some("rust".to_string()),
                ..BookmarkQuery::default()
            }))
            .unwrap();
        assert!(where_clause.contains("bookmark_tags"));
        assert_eq!(params.len(), 1);

        let ids = db
            .selected_ids(&BookmarkSelection::Query(BookmarkQuery {
                author: Some("bea".to_string()),
                ..BookmarkQuery::default()
            }))
            .unwrap();
        assert_eq!(ids.len(), 1);

        let empty = db
            .selected_ids(&BookmarkSelection::Ids(vec!["missing".to_string()]))
            .unwrap();
        assert!(empty.is_empty());
    }
}

#[cfg(test)]
mod ranked_query_tests {
    use super::*;
    use crate::models::BookmarkBuilder;
    use chrono::{TimeZone, Utc};

    fn bookmark(tweet_id: &str, content: &str, day: u32) -> crate::models::Bookmark {
        BookmarkBuilder::new()
            .tweet_url(format!("https://x.com/ranked/status/{tweet_id}"))
            .content(content.to_string())
            .tweeted_at(Utc.with_ymd_and_hms(2024, 5, day, 12, 0, 0).unwrap())
            .author_handle("ranked")
            .author_name("Ranked")
            .build()
            .unwrap()
    }

    #[test]
    fn text_queries_rank_by_relevance_not_recency() {
        let db = Database::open_memory().unwrap();
        // The older post mentions the term three times; the newer one once.
        db.insert_bookmarks(&[
            bookmark("1", "film film film essay", 1),
            bookmark("2", "a film diary entry", 9),
        ])
        .unwrap();
        let page = db
            .search_query_page(&BookmarkQuery {
                query: Some("film".to_string()),
                limit: 10,
                ..BookmarkQuery::default()
            })
            .unwrap();
        assert_eq!(page.1, 2);
        assert_eq!(page.0[0].content, "film film film essay");
        assert_eq!(page.0[1].content, "a film diary entry");
    }

    #[test]
    fn a_page_past_the_last_result_still_reports_the_true_total() {
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&[
            bookmark("1", "film one", 1),
            bookmark("2", "film two", 2),
            bookmark("3", "film three", 3),
        ])
        .unwrap();
        let (items, total) = db
            .search_query_page(&BookmarkQuery {
                query: Some("film".to_string()),
                offset: 10,
                limit: 10,
                ..BookmarkQuery::default()
            })
            .unwrap();
        assert!(items.is_empty());
        assert_eq!(total, 3);

        let (items, total) = db
            .search_query_page(&BookmarkQuery {
                offset: 10,
                limit: 10,
                ..BookmarkQuery::default()
            })
            .unwrap();
        assert!(items.is_empty());
        assert_eq!(total, 3);
    }
}
