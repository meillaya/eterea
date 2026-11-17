//! Shared date parsing for JSON/archive import sources.

use crate::models::Bookmark;
use crate::Result;
use chrono::{DateTime, Utc};

/// Parse the first usable timestamp from up to three candidate strings.
///
/// Accepts unix epoch seconds, RFC 3339, and the classic
/// `Wed Oct 10 20:19:24 +0000 2018` Twitter format. Missing candidates are
/// skipped; an entirely unusable set is an import error for the entry.
pub(crate) fn parse_date_candidates(candidates: [Option<&str>; 3]) -> Result<DateTime<Utc>> {
    for candidate in candidates.into_iter().flatten() {
        let s = candidate.trim().trim_matches('"');
        if let Ok(epoch) = s.parse::<i64>() {
            if let Some(dt) = DateTime::<Utc>::from_timestamp(epoch, 0) {
                return Ok(dt);
            }
        }
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Ok(dt.with_timezone(&Utc));
        }
        if let Ok(dt) = DateTime::parse_from_str(s, "%a %b %d %H:%M:%S %z %Y") {
            return Ok(dt.with_timezone(&Utc));
        }
    }
    Err(crate::Error::Other("Could not parse date".into()))
}

/// Canonical X status URL for a handle/id pair.
///
/// An empty handle falls back to the handle-less web status URL so the quote
/// keeps a usable link without inventing an author.
pub(crate) fn tweet_url_for(handle: &str, tweet_id: &str) -> String {
    let handle = handle.trim().trim_start_matches('@');
    if handle.is_empty() || handle == "i" {
        format!("https://x.com/i/web/status/{tweet_id}")
    } else {
        format!("https://x.com/{handle}/status/{tweet_id}")
    }
}

/// Fill tags from the post text when the export had no explicit tag list.
pub(crate) fn fill_extracted_hashtags(bookmark: &mut Bookmark) {
    if bookmark.tags.is_empty() {
        for tag in bookmark.extract_hashtags() {
            if !bookmark.tags.contains(&tag) {
                bookmark.tags.push(tag);
            }
        }
    }
}
