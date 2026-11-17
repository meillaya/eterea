//! Quoted-tweet data model.
//!
//! Quotes are display-only data attached to a bookmark. A quote can be
//! *available* (the export carried the quoted post body) or *unavailable* (the
//! export only carried a quoted post id/url). Unavailable quotes keep their URL
//! and never invent text: [`QuotedTweet::unavailable`] is the honest state for
//! "missing quote body".

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::Media;

/// A post quoted by a bookmark.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuotedTweet {
    /// Canonical URL of the quoted post (`https://x.com/<handle>/status/<id>`).
    pub tweet_url: String,

    /// Quoted post text. Empty when the quote is unavailable in the export.
    pub content: String,

    /// When the quoted post was published. `None` when the export did not
    /// carry a usable timestamp (typically an unavailable quote).
    #[serde(default)]
    pub tweeted_at: Option<DateTime<Utc>>,

    /// Quoted author handle, without a leading `@`. Empty when unavailable.
    pub author_handle: String,

    /// Quoted author display name. Falls back to the handle for available
    /// quotes; empty when unavailable.
    pub author_name: String,

    /// Remote HTTPS URL of the quoted author's avatar.
    #[serde(default)]
    pub author_profile_image: Option<String>,

    /// Media attached to the quoted post. Display metadata only; quote media
    /// never joins the bookmark `media` table.
    #[serde(default)]
    pub media: Vec<Media>,

    /// True when the export pointed at a quoted post whose body is missing.
    #[serde(default)]
    pub unavailable: bool,
}

impl QuotedTweet {
    /// Build an available quote (the export carried the quoted post body).
    pub fn new(
        tweet_url: impl Into<String>,
        content: impl Into<String>,
        tweeted_at: Option<DateTime<Utc>>,
        author_handle: impl Into<String>,
        author_name: impl Into<String>,
    ) -> Self {
        Self {
            tweet_url: tweet_url.into(),
            content: content.into(),
            tweeted_at,
            author_handle: author_handle.into(),
            author_name: author_name.into(),
            author_profile_image: None,
            media: Vec::new(),
            unavailable: false,
        }
    }

    /// Build an unavailable quote. Only the URL is known; the UI must show the
    /// URL with the "not in archive" state instead of invented text.
    pub fn unavailable(tweet_url: impl Into<String>) -> Self {
        Self {
            tweet_url: tweet_url.into(),
            content: String::new(),
            tweeted_at: None,
            author_handle: String::new(),
            author_name: String::new(),
            author_profile_image: None,
            media: Vec::new(),
            unavailable: true,
        }
    }

    pub fn with_author_profile_image(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        if !url.trim().is_empty() {
            self.author_profile_image = Some(url.trim().to_string());
        }
        self
    }

    pub fn with_media(mut self, media: Vec<Media>) -> Self {
        self.media = media;
        self
    }

    /// Whether the quoted post body is present in the archive.
    pub fn is_available(&self) -> bool {
        !self.unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_quote_keeps_only_the_url() {
        let quote = QuotedTweet::unavailable("https://x.com/quoted_author/status/42");

        assert!(quote.unavailable);
        assert!(!quote.is_available());
        assert_eq!(quote.tweet_url, "https://x.com/quoted_author/status/42");
        assert!(quote.content.is_empty());
        assert_eq!(quote.tweeted_at, None);
        assert!(quote.media.is_empty());
    }

    #[test]
    fn available_quote_keeps_author_and_body() {
        let quote = QuotedTweet::new(
            "https://x.com/quoted_author/status/42",
            "the quoted body",
            None,
            "quoted_author",
            "Quoted Author",
        );

        assert!(quote.is_available());
        assert_eq!(quote.content, "the quoted body");
        assert_eq!(quote.author_handle, "quoted_author");
    }
}
