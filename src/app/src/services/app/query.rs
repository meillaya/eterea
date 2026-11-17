use super::AppServices;
use crate::types::{BookmarkPage, BookmarkQuery};
use anyhow::{Context, Result};

impl AppServices {
    pub fn list_bookmarks(&self, offset: usize, limit: usize) -> Result<BookmarkPage> {
        let items = self
            .db
            .get_bookmarks(offset, limit)
            .context("failed to list bookmarks")?;
        let total = self
            .db
            .count_bookmarks()
            .context("failed to count bookmarks")?;
        Ok(BookmarkPage::new(items, total, offset, limit))
    }

    pub fn query_bookmarks(&self, query: &BookmarkQuery) -> Result<BookmarkPage> {
        if !query.is_filtered() {
            return self.list_bookmarks(query.offset, query.limit);
        }

        // The data layer parses RFC 3339 date filters once and applies the one
        // shared SQL predicate used by reads, bulk mutations, and exports.
        let (items, total) = self
            .db
            .search_query_page(query)
            .context("failed to query bookmarks")?;

        Ok(BookmarkPage::new(items, total, query.offset, query.limit))
    }
}
