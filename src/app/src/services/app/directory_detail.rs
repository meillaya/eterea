use super::AppServices;
use crate::types::{
    AuthorSummary, BookmarkPage, BookmarkStats, DirectoryPageRequest, PaginatedResponse,
    TopicSummary,
};
use anyhow::{Context, Result};

impl AppServices {
    pub fn author_index(&self) -> Result<Vec<AuthorSummary>> {
        self.db
            .get_all_authors()
            .context("failed to load author index")
            .map(|authors| authors.into_iter().map(AuthorSummary::from).collect())
    }

    /// Paged author directory with an optional name/handle filter.
    pub fn author_page(
        &self,
        request: &DirectoryPageRequest,
    ) -> Result<PaginatedResponse<AuthorSummary>> {
        let (authors, total) = self
            .db
            .author_page(request.offset, request.limit, request.filter.as_deref())
            .context("failed to load author page")?;
        Ok(PaginatedResponse::new(
            authors.into_iter().map(AuthorSummary::from).collect(),
            total,
            request.offset,
            request.limit,
        ))
    }

    pub fn topic_index(&self) -> Result<Vec<TopicSummary>> {
        self.db
            .get_all_tags()
            .context("failed to load topic index")
            .map(|topics| {
                topics
                    .into_iter()
                    .map(|(tag, bookmark_count)| TopicSummary {
                        tag,
                        bookmark_count,
                    })
                    .collect()
            })
    }

    /// Paged topic directory with an optional tag filter.
    pub fn topic_page(
        &self,
        request: &DirectoryPageRequest,
    ) -> Result<PaginatedResponse<TopicSummary>> {
        let (topics, total) = self
            .db
            .topic_page(request.offset, request.limit, request.filter.as_deref())
            .context("failed to load topic page")?;
        Ok(PaginatedResponse::new(
            topics
                .into_iter()
                .map(|(tag, bookmark_count)| TopicSummary {
                    tag,
                    bookmark_count,
                })
                .collect(),
            total,
            request.offset,
            request.limit,
        ))
    }

    pub fn thread_posts(&self, id: &str) -> Result<Vec<eterea_core::Bookmark>> {
        self.db
            .thread_for_bookmark(id)
            .with_context(|| format!("failed to load the thread for {id}"))
    }

    pub fn bookmark_detail(&self, id: &str) -> Result<Option<eterea_core::Bookmark>> {
        self.db
            .get_bookmark(id)
            .with_context(|| format!("failed to load bookmark detail for {id}"))
    }

    pub fn bookmarks_by_author(
        &self,
        handle: &str,
        offset: usize,
        limit: usize,
    ) -> Result<BookmarkPage> {
        let (items, total) = self
            .db
            .search_with_filters_page(
                None,
                None,
                Some(handle),
                None,
                None,
                false,
                None,
                offset,
                limit,
            )
            .with_context(|| format!("failed to load bookmarks for author {handle}"))?;
        Ok(BookmarkPage::new(items, total, offset, limit))
    }

    pub fn bookmarks_by_tag(&self, tag: &str, offset: usize, limit: usize) -> Result<BookmarkPage> {
        let (items, total) = self
            .db
            .search_with_filters_page(
                None,
                Some(tag),
                None,
                None,
                None,
                false,
                None,
                offset,
                limit,
            )
            .with_context(|| format!("failed to load bookmarks for tag {tag}"))?;
        Ok(BookmarkPage::new(items, total, offset, limit))
    }

    pub fn stats(&self) -> Result<BookmarkStats> {
        self.db.get_stats().context("failed to load bookmark stats")
    }
}

impl From<eterea_core::storage::AuthorStats> for AuthorSummary {
    fn from(author: eterea_core::storage::AuthorStats) -> Self {
        Self {
            handle: author.handle,
            name: author.name,
            profile_image: author.profile_image,
            bookmark_count: author.bookmark_count,
            favorite_count: author.favorite_count,
        }
    }
}
