//! Basic bookmark read queries.

use super::database::Database;
use crate::models::Bookmark;
use crate::Result;
use rusqlite::params;
use tracing::debug;

impl Database {
    pub fn get_bookmarks(&self, offset: usize, limit: usize) -> Result<Vec<Bookmark>> {
        let overall_started = std::time::Instant::now();
        let mut stmt = self.conn.prepare(
            r#"SELECT id, tweet_url, content, note_text, tweeted_at, imported_at,
                      author_handle, author_name, author_profile_url, author_profile_image,
                      comments, is_favorite,
                      is_read, links_json, conversation_id
               FROM bookmarks
               ORDER BY tweeted_at DESC, id DESC
               LIMIT ?1 OFFSET ?2"#,
        )?;

        let query_started = std::time::Instant::now();
        let mut bookmarks: Vec<Bookmark> = stmt
            .query_map(params![limit as i64, offset as i64], |row| {
                self.row_to_bookmark(row)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let query_elapsed = query_started.elapsed();

        let hydrate_started = std::time::Instant::now();
        self.hydrate_bookmarks(&mut bookmarks)?;
        debug!(
            target: "eterea::db",
            offset,
            limit,
            rows = bookmarks.len(),
            query_ms = query_elapsed.as_millis(),
            hydrate_ms = hydrate_started.elapsed().as_millis(),
            total_ms = overall_started.elapsed().as_millis(),
            "loaded bookmark page"
        );

        Ok(bookmarks)
    }

    pub fn count_bookmarks(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM bookmarks", [], |row| row.get(0))
            .map_err(Into::into)
    }

    /// Every bookmark in a thread, oldest first, anchored on the requested id.
    ///
    /// Uses `conversation_id` when the source carried one; archives without
    /// thread ids fall back to the single bookmark so detail never breaks.
    pub fn thread_for_bookmark(&self, bookmark_id: &str) -> Result<Vec<Bookmark>> {
        let conversation: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT conversation_id FROM bookmarks WHERE id = ?1",
                params![bookmark_id],
                |row| row.get(0),
            )
            .map(Some)
            .unwrap_or(None);
        let Some(Some(conversation)) = conversation else {
            return Ok(self.get_bookmark(bookmark_id)?.into_iter().collect());
        };
        if conversation.trim().is_empty() {
            return Ok(self.get_bookmark(bookmark_id)?.into_iter().collect());
        }
        let mut stmt = self.conn.prepare(
            r#"SELECT id, tweet_url, content, note_text, tweeted_at, imported_at,
                      author_handle, author_name, author_profile_url, author_profile_image,
                      comments, is_favorite,
                      is_read, links_json, conversation_id
               FROM bookmarks
               WHERE conversation_id = ?1
               ORDER BY tweeted_at ASC, id ASC"#,
        )?;
        let mut bookmarks: Vec<Bookmark> = stmt
            .query_map(params![conversation], |row| self.row_to_bookmark(row))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        self.hydrate_bookmarks(&mut bookmarks)?;
        Ok(bookmarks)
    }

    /// Get bookmarks by tag
    pub fn get_bookmarks_by_tag(
        &self,
        tag: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Bookmark>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT b.id, b.tweet_url, b.content, b.note_text, b.tweeted_at, b.imported_at,
                      b.author_handle, b.author_name, b.author_profile_url, b.author_profile_image,
                      b.comments, b.is_favorite,
                      b.is_read, b.links_json, b.conversation_id
               FROM bookmarks b
               JOIN bookmark_tags bt ON bt.bookmark_id = b.id
               JOIN tags t ON t.id = bt.tag_id
               WHERE t.name = ?1
               ORDER BY b.tweeted_at DESC, b.id DESC
               LIMIT ?2 OFFSET ?3"#,
        )?;

        let mut bookmarks: Vec<Bookmark> = stmt
            .query_map(params![tag, limit as i64, offset as i64], |row| {
                self.row_to_bookmark(row)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        self.hydrate_bookmarks(&mut bookmarks)?;

        Ok(bookmarks)
    }

    /// Get bookmarks by author
    pub fn get_bookmarks_by_author(
        &self,
        handle: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Bookmark>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, tweet_url, content, note_text, tweeted_at, imported_at,
                      author_handle, author_name, author_profile_url, author_profile_image,
                      comments, is_favorite,
                      is_read, links_json, conversation_id
               FROM bookmarks
               WHERE author_handle = ?1
               ORDER BY tweeted_at DESC, id DESC
               LIMIT ?2 OFFSET ?3"#,
        )?;

        let mut bookmarks: Vec<Bookmark> = stmt
            .query_map(params![handle, limit as i64, offset as i64], |row| {
                self.row_to_bookmark(row)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        self.hydrate_bookmarks(&mut bookmarks)?;

        Ok(bookmarks)
    }

    /// Get a single bookmark by ID
    pub fn get_bookmark(&self, id: &str) -> Result<Option<Bookmark>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, tweet_url, content, note_text, tweeted_at, imported_at,
                      author_handle, author_name, author_profile_url, author_profile_image,
                      comments, is_favorite,
                      is_read, links_json, conversation_id
               FROM bookmarks WHERE id = ?1"#,
        )?;

        let result = stmt.query_row(params![id], |row| self.row_to_bookmark(row));

        match result {
            Ok(mut bookmark) => {
                bookmark.tags = self.load_bookmark_tags(&bookmark.id)?;
                bookmark.media = self.load_bookmark_media(&bookmark.id)?;
                bookmark.quoted = self.load_bookmark_quote(&bookmark.id)?;
                Ok(Some(bookmark))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get all favorite bookmarks
    pub fn get_favorites(&self, offset: usize, limit: usize) -> Result<Vec<Bookmark>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, tweet_url, content, note_text, tweeted_at, imported_at,
                      author_handle, author_name, author_profile_url, author_profile_image,
                      comments, is_favorite,
                      is_read, links_json, conversation_id
               FROM bookmarks
               WHERE is_favorite = 1
               ORDER BY tweeted_at DESC, id DESC
               LIMIT ?1 OFFSET ?2"#,
        )?;

        let mut bookmarks: Vec<Bookmark> = stmt
            .query_map(params![limit as i64, offset as i64], |row| {
                self.row_to_bookmark(row)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        self.hydrate_bookmarks(&mut bookmarks)?;

        Ok(bookmarks)
    }

    /// Get bookmarks within a date range
    pub fn get_bookmarks_by_date_range(
        &self,
        from: Option<chrono::DateTime<chrono::Utc>>,
        to: Option<chrono::DateTime<chrono::Utc>>,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Bookmark>> {
        let from_ts = from.map(|d| d.timestamp()).unwrap_or(0);
        let to_ts = to.map(|d| d.timestamp()).unwrap_or(i64::MAX);

        let mut stmt = self.conn.prepare(
            r#"SELECT id, tweet_url, content, note_text, tweeted_at, imported_at,
                      author_handle, author_name, author_profile_url, author_profile_image,
                      comments, is_favorite,
                      is_read, links_json, conversation_id
               FROM bookmarks
               WHERE tweeted_at >= ?1 AND tweeted_at <= ?2
               ORDER BY tweeted_at DESC, id DESC
               LIMIT ?3 OFFSET ?4"#,
        )?;

        let mut bookmarks: Vec<Bookmark> = stmt
            .query_map(
                params![from_ts, to_ts, limit as i64, offset as i64],
                |row| self.row_to_bookmark(row),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        self.hydrate_bookmarks(&mut bookmarks)?;

        Ok(bookmarks)
    }
}

#[cfg(test)]
mod thread_tests {
    use crate::models::BookmarkBuilder;
    use crate::storage::Database;
    use chrono::{TimeZone, Utc};

    fn bookmark(id: &str, conversation: Option<&str>, minute: u32) -> crate::models::Bookmark {
        let mut builder = BookmarkBuilder::new()
            .tweet_url(format!("https://x.com/thread/status/{id}"))
            .content(format!("thread post {id}"))
            .tweeted_at(Utc.with_ymd_and_hms(2024, 5, 1, 12, minute, 0).unwrap())
            .author_handle("thread")
            .author_name("Thread");
        if let Some(conversation) = conversation {
            builder = builder.conversation_id(conversation);
        }
        builder.build().unwrap()
    }

    #[test]
    fn thread_posts_are_returned_oldest_first() {
        let db = Database::open_memory().unwrap();
        let third = bookmark("3", Some("conv-1"), 3);
        let first = bookmark("1", Some("conv-1"), 1);
        let second = bookmark("2", Some("conv-1"), 2);
        db.insert_bookmarks(&[
            third,
            first.clone(),
            second.clone(),
            bookmark("9", Some("conv-2"), 5),
            bookmark("8", None, 6),
        ])
        .unwrap();

        let thread = db.thread_for_bookmark(&second.id).unwrap();
        assert_eq!(thread.len(), 3);
        assert_eq!(
            thread
                .iter()
                .map(|post| post.content.as_str())
                .collect::<Vec<_>>(),
            vec!["thread post 1", "thread post 2", "thread post 3"]
        );
    }

    #[test]
    fn thread_falls_back_to_the_post_when_the_source_had_no_conversation() {
        let db = Database::open_memory().unwrap();
        let saved = bookmark("8", None, 6);
        db.insert_bookmarks(std::slice::from_ref(&saved)).unwrap();
        let thread = db.thread_for_bookmark(&saved.id).unwrap();
        assert_eq!(thread.len(), 1);
        assert_eq!(thread[0].id, saved.id);
    }

    #[test]
    fn reimporting_a_post_fills_a_missing_conversation_id() {
        let db = Database::open_memory().unwrap();
        let before = bookmark("4", None, 4);
        db.insert_bookmarks(std::slice::from_ref(&before)).unwrap();
        db.insert_bookmarks(&[bookmark("4", Some("conv-4"), 4)])
            .unwrap();
        let saved = db.get_bookmark(&before.id).unwrap().unwrap();
        assert_eq!(saved.conversation_id.as_deref(), Some("conv-4"));
    }
}
