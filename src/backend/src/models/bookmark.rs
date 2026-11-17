//! Core bookmark data model
//!
//! This unified model supports data from multiple Twitter export formats.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{LinkEntity, Media, MediaType, QuotedTweet, MAX_MEDIA_PER_BOOKMARK};
use uuid::Uuid;

/// Represents a single Twitter/X bookmark with all associated metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Bookmark {
    /// Unique identifier (UUID v4)
    pub id: String,

    /// Tweet URL (serves as natural unique key)
    pub tweet_url: String,

    /// Tweet content/text
    pub content: String,

    /// Extended note tweet text (if available)
    pub note_text: Option<String>,

    /// When the tweet was originally posted
    pub tweeted_at: DateTime<Utc>,

    /// When this bookmark was imported into eterea
    pub imported_at: DateTime<Utc>,

    /// Author information
    pub author_handle: String,
    pub author_name: String,
    pub author_profile_url: Option<String>,
    pub author_profile_image: Option<String>,

    /// Categorization
    pub tags: Vec<String>,

    /// User-added comments/notes
    pub comments: Option<String>,

    /// Media attachments (images, videos)
    pub media: Vec<Media>,

    /// Quoted post attached to this bookmark, if any. Quotes are display-only:
    /// their media lives inside [`QuotedTweet::media`] and never joins the
    /// bookmark `media` table.
    #[serde(default)]
    pub quoted: Option<QuotedTweet>,

    /// Whether this bookmark is marked as favorite
    pub is_favorite: bool,

    /// Whether the reader has marked this saved post as read.
    #[serde(default)]
    pub is_read: bool,

    /// URL entities found in the post text, deduplicated and sorted by URL.
    #[serde(default)]
    pub links: Vec<LinkEntity>,
}

/// Author information (denormalized for speed)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Author {
    pub handle: String,
    pub name: String,
    pub profile_url: Option<String>,
    pub profile_image: Option<String>,
}

impl Bookmark {
    /// Create a new bookmark with generated ID and current import timestamp
    pub fn new(
        tweet_url: String,
        content: String,
        tweeted_at: DateTime<Utc>,
        author_handle: String,
        author_name: String,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            tweet_url,
            content,
            note_text: None,
            tweeted_at,
            imported_at: Utc::now(),
            author_handle,
            author_name,
            author_profile_url: None,
            author_profile_image: None,
            tags: Vec::new(),
            comments: None,
            media: Vec::new(),
            quoted: None,
            is_favorite: false,
            is_read: false,
            links: Vec::new(),
        }
    }

    /// Extract hashtags from content.
    pub fn extract_hashtags(&self) -> Vec<String> {
        extract_marked_terms(&self.content, '#')
    }

    /// Extract mentions from content.
    pub fn extract_mentions(&self) -> Vec<String> {
        extract_marked_terms(&self.content, '@')
    }
}

/// Collect the words after each marker, matching the old `#(\w+)` and
/// `@(\w+)` patterns without compiling a regex per import.
fn extract_marked_terms(text: &str, marker: char) -> Vec<String> {
    let mut terms = Vec::new();
    let mut cursor = 0;

    while cursor < text.len() {
        let Some(relative) = text[cursor..].find(marker) else {
            break;
        };
        let marker_index = cursor + relative;
        let start = marker_index + marker.len_utf8();
        let mut end = start;

        while end < text.len() {
            let Some(character) = text[end..].chars().next() else {
                break;
            };
            if character.is_alphanumeric() || character == '_' {
                end += character.len_utf8();
            } else {
                break;
            }
        }

        if end > start {
            terms.push(text[start..end].to_lowercase());
        }
        cursor = if end > marker_index { end } else { start };
    }

    terms
}

/// Builder pattern for constructing bookmarks from various sources
#[derive(Debug, Default)]
pub struct BookmarkBuilder {
    tweet_url: Option<String>,
    content: Option<String>,
    note_text: Option<String>,
    tweeted_at: Option<DateTime<Utc>>,
    author_handle: Option<String>,
    author_name: Option<String>,
    author_profile_url: Option<String>,
    author_profile_image: Option<String>,
    tags: Vec<String>,
    comments: Option<String>,
    media: Vec<Media>,
    quoted: Option<QuotedTweet>,
    links: Vec<LinkEntity>,
    is_read: bool,
}

impl BookmarkBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tweet_url(mut self, url: impl Into<String>) -> Self {
        self.tweet_url = Some(url.into());
        self
    }

    pub fn content(mut self, content: impl Into<String>) -> Self {
        self.content = Some(content.into());
        self
    }

    pub fn note_text(mut self, note: impl Into<String>) -> Self {
        let note = note.into();
        if !note.is_empty() {
            self.note_text = Some(note);
        }
        self
    }

    pub fn tweeted_at(mut self, dt: DateTime<Utc>) -> Self {
        self.tweeted_at = Some(dt);
        self
    }

    pub fn author_handle(mut self, handle: impl Into<String>) -> Self {
        self.author_handle = Some(handle.into());
        self
    }

    pub fn author_name(mut self, name: impl Into<String>) -> Self {
        self.author_name = Some(name.into());
        self
    }

    pub fn author_profile_url(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        if !url.is_empty() {
            self.author_profile_url = Some(url);
        }
        self
    }

    pub fn author_profile_image(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        if !url.is_empty() {
            self.author_profile_image = Some(url);
        }
        self
    }

    pub fn tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    pub fn add_tag(mut self, tag: impl Into<String>) -> Self {
        let tag = tag.into();
        if !tag.is_empty() && !self.tags.contains(&tag) {
            self.tags.push(tag);
        }
        self
    }

    pub fn comments(mut self, comments: impl Into<String>) -> Self {
        let comments = comments.into();
        if !comments.is_empty() {
            self.comments = Some(comments);
        }
        self
    }

    pub fn media(mut self, media: Vec<Media>) -> Self {
        self.media = media.into_iter().take(MAX_MEDIA_PER_BOOKMARK).collect();
        self
    }

    /// Replace the URL entities. They are deduplicated and sorted by URL.
    pub fn links(mut self, links: Vec<LinkEntity>) -> Self {
        self.links = super::normalize_links(links);
        self
    }

    /// Add one URL entity; duplicates are merged when the bookmark is built.
    pub fn add_link(mut self, link: LinkEntity) -> Self {
        self.links.push(link);
        self
    }

    /// Mark the bookmark as read at import time.
    pub fn is_read(mut self, is_read: bool) -> Self {
        self.is_read = is_read;
        self
    }

    pub fn add_media(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        if !url.is_empty() {
            let media_type = MediaType::from_url(&url);
            self.media.push(Media::new(url, media_type));
        }
        self
    }

    pub fn add_media_metadata(mut self, media: Media) -> Self {
        // Data keeps up to 20 attachments per bookmark; the UI decides how
        // many of them to show. Beyond the cap the extras are dropped rather
        // than silently corrupting a gallery.
        if !media.url.is_empty() && self.media.len() < MAX_MEDIA_PER_BOOKMARK {
            self.media.push(media);
        }
        self
    }

    pub fn quoted(mut self, quoted: QuotedTweet) -> Self {
        self.quoted = Some(quoted);
        self
    }

    pub fn build(self) -> Result<Bookmark, &'static str> {
        let tweet_url = self.tweet_url.ok_or("tweet_url is required")?;
        let content = self.content.unwrap_or_default();
        let tweeted_at = self.tweeted_at.ok_or("tweeted_at is required")?;
        let author_handle = self.author_handle.ok_or("author_handle is required")?;
        let author_name = self.author_name.unwrap_or_else(|| author_handle.clone());

        let mut bookmark =
            Bookmark::new(tweet_url, content, tweeted_at, author_handle, author_name);

        bookmark.note_text = self.note_text;
        bookmark.author_profile_url = self.author_profile_url;
        bookmark.author_profile_image = self.author_profile_image;
        bookmark.tags = self.tags;
        bookmark.comments = self.comments;
        bookmark.media = self.media;
        bookmark.quoted = self.quoted;
        bookmark.is_favorite = false;
        bookmark.is_read = self.is_read;
        bookmark.links = super::normalize_links(self.links);

        Ok(bookmark)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bookmark_builder() {
        let bookmark = BookmarkBuilder::new()
            .tweet_url("https://twitter.com/user/status/123")
            .content("Hello world! #rust @rustlang")
            .tweeted_at(Utc::now())
            .author_handle("rustacean")
            .author_name("Rust Developer")
            .add_tag("programming")
            .build()
            .unwrap();

        assert_eq!(bookmark.author_handle, "rustacean");
        assert_eq!(bookmark.tags, vec!["programming"]);

        let hashtags = bookmark.extract_hashtags();
        assert_eq!(hashtags, vec!["rust"]);

        let mentions = bookmark.extract_mentions();
        assert_eq!(mentions, vec!["rustlang"]);
    }
}
