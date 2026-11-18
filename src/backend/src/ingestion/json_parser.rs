//! JSON parsing for Twitter bookmark exports.
//!
//! Three JSON shapes are supported:
//!
//! - Flat bookmark arrays (`[{ "tweet_url": ..., "full_text": ... }]`).
//! - Generic JSON/archive-JS wrappers (`window.YTD... = [...]`) with
//!   `quoted_status` / `quoted_status_id_str` quote data when present.
//! - X API v2 responses: root `data` plus `includes.tweets`, `includes.users`,
//!   and `includes.media`, with `referenced_tweets` entries of type `quoted`.

use super::date_parse::{fill_extracted_hashtags, parse_date_candidates, tweet_url_for};
use super::export_parser::parse_eterea_export_json;
use super::media_metadata::extract_media;
use super::quote_metadata::{
    bare_quoted_id, embedded_quoted_object, media_from_media_keys, quoted_reference_id,
    quoted_tweet_from_object, quoted_tweet_from_v2,
};
use super::url_entities::extract_links;
use crate::models::{Bookmark, BookmarkBuilder, QuotedTweet};
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, warn};

/// Parser for JSON bookmark exports.
pub struct JsonParser;

impl JsonParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse(&self, path: &Path) -> Result<Vec<Bookmark>> {
        // There is intentionally no serde "fast path" over a narrower row
        // struct here: a second row-to-bookmark mapping silently dropped the
        // fields only `convert_raw` understands (content, author_handle, tags,
        // quotes, links). One parser path keeps every supported shape equal.
        let raw = std::fs::read_to_string(path)?;
        self.parse_str(&raw)
    }

    pub fn parse_str(&self, raw: &str) -> Result<Vec<Bookmark>> {
        let payload = self.extract_payload(raw)?;

        let root: Value = serde_json::from_str(payload)?;

        // eterea's own export format carries full bookmark values (tags,
        // media, quote, links, read/favorite flags) and must round-trip.
        if let Some(bookmarks) = parse_eterea_export_json(&root)? {
            debug!(
                "Parsed {} bookmarks from eterea JSON export",
                bookmarks.len()
            );
            return Ok(bookmarks);
        }

        if let Some(bookmarks) = self.parse_v2(&root)? {
            debug!("Parsed {} bookmarks from X API v2 payload", bookmarks.len());
            return Ok(bookmarks);
        }

        let raw_bookmarks = self.collect_entries(&root);

        let mut bookmarks = Vec::new();
        for (idx, raw) in raw_bookmarks.into_iter().enumerate() {
            match self.convert_raw(raw) {
                Ok(bookmark) => bookmarks.push(bookmark),
                Err(e) => warn!("Skipping JSON entry {}: {}", idx, e),
            }
        }

        debug!(
            "Parsed {} bookmarks from JSON fallback path",
            bookmarks.len()
        );
        Ok(bookmarks)
    }

    /// Parse an X API v2 response (`data` + `includes`), returning `None` when
    /// the payload is not shaped like one.
    fn parse_v2(&self, root: &Value) -> Result<Option<Vec<Bookmark>>> {
        let Some((entries, includes_value)) = v2_sections(root) else {
            return Ok(None);
        };
        let includes = V2Includes::from_value(includes_value);

        let mut bookmarks = Vec::new();
        for (idx, entry) in entries.iter().enumerate() {
            match self.convert_v2_tweet(entry, &includes) {
                Ok(bookmark) => bookmarks.push(bookmark),
                Err(error) => warn!("Skipping v2 data entry {idx}: {error}"),
            }
        }

        Ok(Some(bookmarks))
    }

    fn extract_payload<'a>(&self, raw: &'a str) -> Result<&'a str> {
        let trimmed = raw.trim_start_matches('\u{feff}').trim();

        if trimmed.starts_with("window.YTD") {
            let start = trimmed.find(['[', '{']).ok_or_else(|| {
                Error::InvalidFormat("Could not locate JSON payload in archive JS".into())
            })?;
            let end = trimmed.rfind([']', '}']).ok_or_else(|| {
                Error::InvalidFormat("Could not locate end of JSON payload in archive JS".into())
            })?;

            if end < start {
                return Err(Error::InvalidFormat(
                    "Archive JS has a closing bracket before its JSON payload".into(),
                ));
            }
            return Ok(trimmed[start..=end].trim());
        }

        Ok(trimmed)
    }

    fn collect_entries<'a>(&self, root: &'a Value) -> Vec<&'a Value> {
        match root {
            Value::Array(items) => items.iter().collect(),
            Value::Object(map) => {
                for key in ["bookmarks", "items", "data"] {
                    if let Some(Value::Array(items)) = map.get(key) {
                        return items.iter().collect();
                    }
                }
                vec![root]
            }
            _ => Vec::new(),
        }
    }

    fn convert_raw(&self, raw: &Value) -> Result<Bookmark> {
        let raw = self.unwrap_entry(raw);

        let author_handle = self
            .extract_string(
                raw,
                &[
                    &["screen_name"],
                    &["author_handle"],
                    &["username"],
                    &["user", "screen_name"],
                    &["user", "legacy", "screen_name"],
                    &["core", "user_results", "result", "legacy", "screen_name"],
                ],
            )
            .or_else(|| {
                self.extract_string(
                    raw,
                    &[
                        &["tweet_url"],
                        &["url"],
                        &["expanded_url"],
                        &["expandedUrl"],
                    ],
                )
                .and_then(|url| self.extract_handle_from_url(&url))
            })
            .unwrap_or_default();

        let author_name = self
            .extract_string(
                raw,
                &[
                    &["name"],
                    &["author_name"],
                    &["display_name"],
                    &["user", "name"],
                    &["user", "legacy", "name"],
                    &["core", "user_results", "result", "legacy", "name"],
                ],
            )
            .unwrap_or_else(|| author_handle.clone());

        let tweet_id =
            self.extract_string(raw, &[&["tweet_id"], &["tweetId"], &["id_str"], &["id"]]);
        let tweet_url = self
            .extract_string(
                raw,
                &[
                    &["tweet_url"],
                    &["url"],
                    &["expanded_url"],
                    &["expandedUrl"],
                    &["tweet", "url"],
                    &["bookmark", "url"],
                ],
            )
            .or_else(|| {
                tweet_id
                    .as_deref()
                    .map(|id| tweet_url_for(&author_handle, id))
            })
            .ok_or_else(|| Error::Other("Missing tweet URL".into()))?;

        let content = self
            .extract_string(
                raw,
                &[
                    &["full_text"],
                    &["text"],
                    &["content"],
                    &["legacy", "full_text"],
                    &["tweet", "full_text"],
                ],
            )
            .unwrap_or_default();

        let note_text = self.extract_string(
            raw,
            &[
                &["note_tweet_text"],
                &["noteTweetText"],
                &["note_tweet", "text"],
            ],
        );
        let tweeted_at = self.parse_date(raw)?;

        let mut builder = BookmarkBuilder::new()
            .tweet_url(tweet_url)
            .content(content)
            .note_text(note_text.unwrap_or_default())
            .tweeted_at(tweeted_at)
            .author_handle(author_handle)
            .author_name(author_name);
        if let Some(conversation_id) = self.extract_string(
            raw,
            &[
                &["conversation_id"],
                &["conversationId"],
                &["legacy", "conversation_id"],
                &["tweet", "conversation_id"],
            ],
        ) {
            builder = builder.conversation_id(conversation_id);
        }

        if let Some(img) = self.extract_string(
            raw,
            &[
                &["profile_image_url_https"],
                &["profile_image"],
                &["user", "profile_image_url_https"],
                &["user", "legacy", "profile_image_url_https"],
                &[
                    "core",
                    "user_results",
                    "result",
                    "legacy",
                    "profile_image_url_https",
                ],
            ],
        ) {
            builder = builder.author_profile_image(img);
        }

        if let Some(profile_url) =
            self.extract_string(raw, &[&["author_profile_url"], &["profile_url"]])
        {
            builder = builder.author_profile_url(profile_url);
        }

        if let Some(tags) = self.extract_tags(raw) {
            for tag in tags {
                builder = builder.add_tag(tag);
            }
        }

        for media in extract_media(raw) {
            builder = builder.add_media_metadata(media);
        }

        for link in extract_links(raw) {
            builder = builder.add_link(link);
        }

        if let Some(quote) = self.extract_quote(raw) {
            builder = builder.quoted(quote);
        }

        let mut bookmark = builder.build().map_err(|e| Error::Other(e.to_string()))?;
        fill_extracted_hashtags(&mut bookmark);

        Ok(bookmark)
    }

    /// Convert one X API v2 tweet entry, resolving authors, media, and quotes
    /// through the root `includes` payload.
    fn convert_v2_tweet(&self, raw: &Value, includes: &V2Includes<'_>) -> Result<Bookmark> {
        let raw = self.unwrap_entry(raw);

        let tweet_id =
            self.extract_string(raw, &[&["id_str"], &["id"], &["tweet_id"], &["tweetId"]]);
        let author_id = self.extract_string(raw, &[&["author_id"]]);
        let author = author_id
            .as_deref()
            .and_then(|id| includes.users_by_id.get(id).copied());

        let author_handle = author
            .and_then(|user| self.extract_string(user, &[&["username"], &["screen_name"]]))
            .or_else(|| {
                self.extract_string(
                    raw,
                    &[
                        &["screen_name"],
                        &["author_handle"],
                        &["username"],
                        &["user", "screen_name"],
                    ],
                )
            })
            .unwrap_or_default();

        let author_name = author
            .and_then(|user| self.extract_string(user, &[&["name"]]))
            .or_else(|| self.extract_string(raw, &[&["name"], &["author_name"], &["display_name"]]))
            .unwrap_or_else(|| author_handle.clone());

        let tweet_url = self
            .extract_string(raw, &[&["tweet_url"], &["expanded_url"], &["expandedUrl"]])
            .filter(|url| url.contains("/status/"))
            .or_else(|| {
                tweet_id
                    .as_deref()
                    .map(|id| tweet_url_for(&author_handle, id))
            })
            .ok_or_else(|| Error::Other("Missing tweet URL".into()))?;

        let content = self
            .extract_string(raw, &[&["text"], &["full_text"], &["content"]])
            .unwrap_or_default();
        let note_text = self.extract_string(
            raw,
            &[
                &["note_tweet", "text"],
                &["note_tweet_text"],
                &["noteTweetText"],
            ],
        );
        let tweeted_at = self.parse_date(raw)?;

        let mut builder = BookmarkBuilder::new()
            .tweet_url(tweet_url)
            .content(content)
            .note_text(note_text.unwrap_or_default())
            .tweeted_at(tweeted_at)
            .author_handle(author_handle)
            .author_name(author_name);
        if let Some(conversation_id) =
            self.extract_string(raw, &[&["conversation_id"], &["conversationId"]])
        {
            builder = builder.conversation_id(conversation_id);
        }

        if let Some(img) = author
            .and_then(|user| {
                self.extract_string(
                    user,
                    &[
                        &["profile_image_url"],
                        &["profile_image_url_https"],
                        &["profile_image"],
                    ],
                )
            })
            .or_else(|| {
                self.extract_string(
                    raw,
                    &[
                        &["profile_image_url_https"],
                        &["profile_image"],
                        &["user", "profile_image_url_https"],
                    ],
                )
            })
        {
            builder = builder.author_profile_image(img);
        }

        if let Some(tags) = self.extract_tags(raw) {
            for tag in tags {
                builder = builder.add_tag(tag);
            }
        }

        let media = if has_media_keys(raw) {
            media_from_media_keys(raw, &includes.media_by_key)
        } else {
            extract_media(raw)
        };
        for item in media {
            builder = builder.add_media_metadata(item);
        }

        for link in extract_links(raw) {
            builder = builder.add_link(link);
        }

        if let Some(quote) = self.v2_quote(raw, includes) {
            builder = builder.quoted(quote);
        }

        let mut bookmark = builder.build().map_err(|e| Error::Other(e.to_string()))?;
        fill_extracted_hashtags(&mut bookmark);

        Ok(bookmark)
    }

    /// Resolve the quoted post of a v2 tweet: embedded body first, then a bare
    /// id, then `referenced_tweets` through `includes.tweets`.
    fn v2_quote(&self, raw: &Value, includes: &V2Includes<'_>) -> Option<QuotedTweet> {
        if let Some(quote) = embedded_quoted_object(raw).and_then(quoted_tweet_from_object) {
            return Some(quote);
        }

        if let Some(id) = bare_quoted_id(raw) {
            return Some(QuotedTweet::unavailable(tweet_url_for("", &id)));
        }

        let quoted_id = quoted_reference_id(raw)?;
        includes
            .tweets_by_id
            .get(&quoted_id)
            .copied()
            .and_then(|quoted| {
                quoted_tweet_from_v2(quoted, &includes.users_by_id, &includes.media_by_key)
            })
            .or_else(|| Some(QuotedTweet::unavailable(tweet_url_for("", &quoted_id))))
    }

    /// Extract the quote attached to a generic (archive/v1.1-ish) entry.
    ///
    /// An embedded quote body is preferred; a bare quoted id becomes an
    /// unavailable quote that keeps its URL.
    fn extract_quote(&self, raw: &Value) -> Option<QuotedTweet> {
        embedded_quoted_object(raw)
            .and_then(quoted_tweet_from_object)
            .or_else(|| {
                bare_quoted_id(raw).map(|id| QuotedTweet::unavailable(tweet_url_for("", &id)))
            })
            .or_else(|| {
                quoted_reference_id(raw).map(|id| QuotedTweet::unavailable(tweet_url_for("", &id)))
            })
    }

    fn unwrap_entry<'a>(&self, raw: &'a Value) -> &'a Value {
        let mut current = raw;

        loop {
            let Some(object) = current.as_object() else {
                return current;
            };

            let mut next = None;
            for key in ["bookmark", "tweet", "item", "entry", "data", "value"] {
                if let Some(candidate) = object.get(key) {
                    next = Some(candidate);
                    break;
                }
            }

            match next {
                Some(candidate) => current = candidate,
                None => return current,
            }
        }
    }

    fn extract_string(&self, raw: &Value, paths: &[&[&str]]) -> Option<String> {
        paths
            .iter()
            .find_map(|path| Self::value_at_path(raw, path))
            .and_then(Self::value_to_string)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    fn extract_tags(&self, raw: &Value) -> Option<Vec<String>> {
        let tags = Self::value_at_path(raw, &["tags"])
            .or_else(|| Self::value_at_path(raw, &["entities", "hashtags"]))
            .or_else(|| Self::value_at_path(raw, &["legacy", "entities", "hashtags"]))?;

        let Value::Array(items) = tags else {
            return None;
        };

        let mut result = Vec::new();
        for item in items {
            if let Some(tag) = Self::value_to_string(item)
                .or_else(|| Self::value_at_path(item, &["text"]).and_then(Self::value_to_string))
                .or_else(|| Self::value_at_path(item, &["tag"]).and_then(Self::value_to_string))
            {
                let normalized = tag.trim().trim_start_matches('#').to_lowercase();
                if !normalized.is_empty() && !result.contains(&normalized) {
                    result.push(normalized);
                }
            }
        }

        Some(result)
    }

    fn extract_handle_from_url(&self, url: &str) -> Option<String> {
        let trimmed = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
        let path = trimmed.split_once('/').map(|(_, rest)| rest)?;
        let handle = path.split('/').next()?.trim();
        if handle.is_empty() || handle == "i" {
            return None;
        }
        Some(handle.trim_start_matches('@').to_string())
    }

    fn parse_date(&self, raw: &Value) -> Result<DateTime<Utc>> {
        let date_str = self.extract_string(
            raw,
            &[
                &["tweeted_at"],
                &["created_at"],
                &["createdAt"],
                &["bookmark_date"],
                &["date"],
                &["timestamp"],
                &["tweet", "created_at"],
                &["bookmark", "createdAt"],
            ],
        );

        let Some(s) = date_str else {
            return Err(Error::Other("Missing date".into()));
        };

        parse_date_candidates([Some(s.as_str()), None, None])
    }

    fn value_at_path<'a>(raw: &'a Value, path: &[&str]) -> Option<&'a Value> {
        let mut current = raw;
        for segment in path {
            current = current.get(*segment)?;
        }
        Some(current)
    }

    fn value_to_string(value: &Value) -> Option<String> {
        match value {
            Value::String(s) => Some(s.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        }
    }
}

/// `(data rows, includes object)` when the root looks like an X API v2 response.
fn v2_sections(root: &Value) -> Option<(&Vec<Value>, &Value)> {
    let includes = root.get("includes")?;
    if !includes.is_object() {
        return None;
    }

    for key in ["data", "bookmarks", "items", "tweets"] {
        if let Some(Value::Array(items)) = root.get(key) {
            return Some((items, includes));
        }
    }

    None
}

fn has_media_keys(raw: &Value) -> bool {
    matches!(
        JsonParser::value_at_path(raw, &["attachments", "media_keys"]),
        Some(Value::Array(keys)) if !keys.is_empty()
    )
}

/// Lookup indexes over an X API v2 `includes` object.
struct V2Includes<'a> {
    tweets_by_id: HashMap<String, &'a Value>,
    users_by_id: HashMap<String, &'a Value>,
    media_by_key: HashMap<String, &'a Value>,
}

impl<'a> V2Includes<'a> {
    fn from_value(includes: &'a Value) -> Self {
        Self {
            tweets_by_id: index_array(includes, "tweets", "id"),
            users_by_id: index_array(includes, "users", "id"),
            media_by_key: index_array(includes, "media", "media_key"),
        }
    }
}

fn index_array<'a>(value: &'a Value, key: &str, id_key: &str) -> HashMap<String, &'a Value> {
    let mut index = HashMap::new();
    let Some(Value::Array(items)) = value.get(key) else {
        return index;
    };

    for item in items {
        if let Some(id) = item.get(id_key).and_then(value_to_scalar_string) {
            index.insert(id, item);
        }
    }

    index
}

fn value_to_scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

impl Default for JsonParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::MediaType;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn parses_flat_json_row() {
        let bookmarks = JsonParser::new()
            .parse_str(
                r#"[{"bookmark_date":"2025-08-25T17:25:01.047Z","profile_image_url_https":"https://pbs.twimg.com/profile.jpg","screen_name":"cinesansar","name":"persona","full_text":"hello #rust","tweeted_at":"2025-08-25T10:52:35.000Z","extended_media":[{"media_url_https":"https://pbs.twimg.com/media/test.jpg","type":"photo"}],"tweet_url":"https://x.com/cinesansar/status/123"}]"#,
            )
            .unwrap();
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks[0].author_handle, "cinesansar");
        assert_eq!(bookmarks[0].tags, vec!["rust"]);
        assert_eq!(bookmarks[0].media.len(), 1);
    }

    #[test]
    fn array_and_wrapper_shapes_produce_equal_bookmarks() {
        // Regression for B-CD-2: the removed serde fast path mapped only
        // full_text/screen_name and silently blanked rows that carried the
        // generic keys (content/author_handle/tags). Both shapes must route
        // through the same converter and produce identical Bookmarks.
        let row = r#"{"tweet_url":"https://x.com/a/status/1","tweeted_at":"2024-05-01T00:00:00Z","content":"hello world","author_handle":"a","author_name":"A","tags":["rust","design"]}"#;
        let array = JsonParser::new().parse_str(&format!("[{row}]")).unwrap();
        let wrapper = JsonParser::new()
            .parse_str(&format!(r#"{{"items":[{row}]}}"#))
            .unwrap();

        assert_eq!(array.len(), 1);
        assert_eq!(wrapper.len(), 1);
        assert_eq!(array[0].content, "hello world");
        assert_eq!(array[0].author_handle, "a");
        assert_eq!(array[0].author_name, "A");
        assert_eq!(
            array[0].tags,
            vec!["rust".to_string(), "design".to_string()]
        );

        // ids/import timestamps are generated per parse; equalize them so the
        // whole model value can be compared field-for-field.
        let mut actual = array[0].clone();
        let mut expected = wrapper[0].clone();
        expected.id = actual.id.clone();
        expected.imported_at = actual.imported_at;
        assert_eq!(actual, expected);
        actual.id.push_str("-moved");
        assert_ne!(actual, expected, "comparison must actually be value-based");
    }

    #[test]
    fn millisecond_epoch_timestamp_parses_to_the_true_date() {
        // Regression for B-CD-3: Date.getTime()-style 13-digit epochs used to
        // be stored as seconds (year 56302) and broke the calendar.
        let bookmarks = JsonParser::new()
            .parse_str(
                r#"[{"tweet_url":"https://x.com/a/status/1","timestamp":1714564800000,"full_text":"ms epoch","screen_name":"a","name":"A"}]"#,
            )
            .unwrap();
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(
            bookmarks[0].tweeted_at,
            chrono::TimeZone::with_ymd_and_hms(&Utc, 2024, 5, 1, 12, 0, 0).unwrap()
        );
    }

    #[test]
    fn parses_flat_media_metadata_row() {
        let bookmarks = JsonParser::new()
            .parse_str(
                r#"[{"bookmark_date":"2025-08-25T17:25:01.047Z","screen_name":"media","name":"Media","full_text":"hello","tweeted_at":"2025-08-25T10:52:35.000Z","extended_media":[{"media_key":"7_fast","type":"video","preview_image_url":"https://pbs.twimg.com/media/preview.jpg","alt_text":"Alt from export","width":640,"height":480,"video_info":{"duration_millis":42000,"variants":[{"url":"https://video.twimg.com/fast/360.mp4","bit_rate":832000,"content_type":"video/mp4"},{"url":"https://video.twimg.com/fast/hls.m3u8","content_type":"application/x-mpegURL"}]} }],"tweet_url":"https://x.com/media/status/123"}]"#,
            )
            .unwrap();

        let media = &bookmarks[0].media[0];
        assert_eq!(media.source_media_key.as_deref(), Some("7_fast"));
        assert_eq!(media.source_type.as_deref(), Some("video"));
        assert_eq!(media.media_type, MediaType::Video);
        assert_eq!(
            media.variant_url.as_deref(),
            Some("https://video.twimg.com/fast/360.mp4")
        );
        let variants_json = media.variants_json.as_deref().unwrap_or_default();
        assert!(variants_json.contains("bit_rate"));
        assert!(variants_json.contains("content_type"));
        assert_eq!(media.alt_text.as_deref(), Some("Alt from export"));
        assert_eq!(media.width, Some(640));
        assert_eq!(media.height, Some(480));
        assert_eq!(
            media.preview_url.as_deref(),
            Some("https://pbs.twimg.com/media/preview.jpg")
        );
        assert_eq!(
            media.poster_url.as_deref(),
            Some("https://pbs.twimg.com/media/preview.jpg")
        );
        assert_eq!(media.duration_ms, Some(42000));
    }

    #[test]
    fn parses_included_x_media_metadata_fallback_path() {
        let bookmarks = JsonParser::new()
            .parse_str(
                r#"{"bookmarks":[{"tweet_id":"123","created_at":"2025-08-25T10:52:35.000Z","username":"media","full_text":"video bookmark","attachments":{"media_keys":["7_video"]},"includes":{"media":[{"media_key":"7_video","type":"video","preview_image_url":"https://pbs.twimg.com/ext_tw_video/thumb.jpg","duration_ms":9000,"variants":[{"url":"https://video.twimg.com/ext_tw_video/360.mp4","bit_rate":832000},{"url":"https://video.twimg.com/ext_tw_video/720.mp4","bit_rate":2176000}],"width":1280,"height":720}]}}]}"#,
            )
            .unwrap();

        let media = &bookmarks[0].media[0];
        assert_eq!(media.media_type, MediaType::Video);
        assert_eq!(media.source_media_key.as_deref(), Some("7_video"));
        assert_eq!(
            media.preview_url.as_deref(),
            Some("https://pbs.twimg.com/ext_tw_video/thumb.jpg")
        );
        assert_eq!(
            media.variant_url.as_deref(),
            Some("https://video.twimg.com/ext_tw_video/360.mp4")
        );
        let variants_json = media.variants_json.as_deref().unwrap_or_default();
        assert!(variants_json.contains("360.mp4"));
        assert!(variants_json.contains("720.mp4"));
        assert_eq!(media.width, Some(1280));
        assert_eq!(media.height, Some(720));
    }

    #[test]
    fn parses_array_json_export() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bookmarks.json");
        fs::write(
            &path,
            r#"[{"tweet_url":"https://x.com/rustlang/status/123","full_text":"Rust release day #rust","tweeted_at":"2024-05-01T14:30:00Z","screen_name":"rustlang","name":"Rust Language","media":[{"url":"https://pbs.twimg.com/media/test.jpg"}]}]"#,
        )
        .unwrap();

        let bookmarks = JsonParser::new().parse(&path).unwrap();
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks[0].author_handle, "rustlang");
        assert_eq!(bookmarks[0].tags, vec!["rust"]);
        assert_eq!(bookmarks[0].media.len(), 1);
    }

    #[test]
    fn parses_archive_js_wrapper() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bookmarks.js");
        fs::write(
            &path,
            r#"window.YTD.bookmark.part0 = [{"bookmark": {"tweetId":"999","createdAt":"2024-08-25T10:52:35.000Z","screen_name":"sveltejs","name":"Svelte","full_text":"Runes are here","expandedUrl":"https://x.com/sveltejs/status/999"}}];"#,
        )
        .unwrap();

        let bookmarks = JsonParser::new().parse(&path).unwrap();
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks[0].tweet_url, "https://x.com/sveltejs/status/999");
        assert_eq!(bookmarks[0].author_name, "Svelte");
        assert_eq!(bookmarks[0].author_handle, "sveltejs");
    }

    #[test]
    fn malformed_archive_wrapper_is_an_error_not_a_panic() {
        let error = JsonParser::new()
            .parse_str("window.YTD.tweets.part0 = K];[\n")
            .expect_err("malformed archive wrapper must be rejected");
        assert!(matches!(error, Error::InvalidFormat(_)));
    }

    #[test]
    fn parses_archive_quoted_status_body() {
        let bookmarks = JsonParser::new()
            .parse_str(
                r#"[{"tweet_url":"https://x.com/ada_dev/status/1","full_text":"worth quoting #rust","tweeted_at":"2026-05-04T10:00:00Z","screen_name":"ada_dev","name":"Ada","quoted_status":{"id_str":"9001","full_text":"the quoted original","created_at":"2026-05-03T09:00:00Z","user":{"screen_name":"quoted_author","name":"Quoted Author","profile_image_url_https":"https://example.com/q.png"},"extended_entities":{"media":[{"type":"photo","media_url_https":"https://example.com/quote.png","alt_text":"quote alt"}]}}}]"#,
            )
            .unwrap();

        let quote = bookmarks[0].quoted.as_ref().expect("quote should parse");
        assert!(!quote.unavailable);
        assert_eq!(quote.tweet_url, "https://x.com/quoted_author/status/9001");
        assert_eq!(quote.content, "the quoted original");
        assert_eq!(quote.author_handle, "quoted_author");
        assert_eq!(quote.author_name, "Quoted Author");
        assert_eq!(
            quote.author_profile_image.as_deref(),
            Some("https://example.com/q.png")
        );
        assert_eq!(
            quote.tweeted_at,
            Some(
                DateTime::parse_from_rfc3339("2026-05-03T09:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
        assert_eq!(quote.media.len(), 1);
        assert_eq!(quote.media[0].alt_text.as_deref(), Some("quote alt"));
        assert!(bookmarks[0].media.is_empty(), "quote media stays separate");
    }

    #[test]
    fn missing_archive_quote_body_becomes_unavailable_with_url() {
        let bookmarks = JsonParser::new()
            .parse_str(
                r#"[{"tweet_url":"https://x.com/ada_dev/status/2","full_text":"quoting a deleted post","tweeted_at":"2026-05-04T10:00:00Z","screen_name":"ada_dev","name":"Ada","quoted_status_id_str":"9002"}]"#,
            )
            .unwrap();

        let quote = bookmarks[0].quoted.as_ref().expect("quote should parse");
        assert!(quote.unavailable);
        assert_eq!(quote.tweet_url, "https://x.com/i/web/status/9002");
        assert!(quote.content.is_empty());
        assert_eq!(quote.author_handle, "");
        assert_eq!(quote.tweeted_at, None);
        assert!(quote.media.is_empty());
    }

    #[test]
    fn flat_archive_row_with_numeric_quoted_status_id_stays_unavailable() {
        let bookmarks = JsonParser::new()
            .parse_str(
                r#"[{"tweet_url":"https://x.com/ada_dev/status/3","full_text":"quoting by numeric id","tweeted_at":"2026-05-04T10:00:00Z","screen_name":"ada_dev","name":"Ada","quoted_status_id":9003}]"#,
            )
            .unwrap();

        let quote = bookmarks[0].quoted.as_ref().expect("quote should parse");
        assert!(quote.unavailable);
        assert_eq!(quote.tweet_url, "https://x.com/i/web/status/9003");
    }

    #[test]
    fn parses_x_api_v2_payload_with_quoted_post_and_includes() {
        let payload = r#"{
          "data": [{
            "id": "2001",
            "text": "quoting a post #rust",
            "created_at": "2026-05-04T10:00:00.000Z",
            "author_id": "u1",
            "attachments": { "media_keys": ["3_img"] },
            "referenced_tweets": [{ "type": "quoted", "id": "9001" }],
            "entities": { "hashtags": [{ "tag": "rust" }] }
          }],
          "includes": {
            "users": [
              { "id": "u1", "name": "Ada Developer", "username": "ada_dev", "profile_image_url": "https://example.com/ada.png" },
              { "id": "u2", "name": "Quoted Author", "username": "quoted_author", "profile_image_url": "https://example.com/q.png" }
            ],
            "tweets": [{
              "id": "9001",
              "text": "the quoted original",
              "created_at": "2026-05-03T09:00:00.000Z",
              "author_id": "u2",
              "attachments": { "media_keys": ["7_vid"] }
            }],
            "media": [
              { "media_key": "3_img", "type": "photo", "url": "https://example.com/img.png", "width": 1200, "height": 800, "alt_text": "A diagram" },
              { "media_key": "7_vid", "type": "video", "preview_image_url": "https://example.com/poster.jpg", "duration_ms": 42000, "variants": [{ "url": "https://example.com/v.mp4", "content_type": "video/mp4", "bit_rate": 832000 }] }
            ]
          }
        }"#;

        let bookmarks = JsonParser::new().parse_str(payload).unwrap();
        assert_eq!(bookmarks.len(), 1);
        let bookmark = &bookmarks[0];

        assert_eq!(bookmark.tweet_url, "https://x.com/ada_dev/status/2001");
        assert_eq!(bookmark.author_handle, "ada_dev");
        assert_eq!(bookmark.author_name, "Ada Developer");
        assert_eq!(
            bookmark.author_profile_image.as_deref(),
            Some("https://example.com/ada.png")
        );
        assert_eq!(bookmark.tags, vec!["rust"]);
        assert_eq!(bookmark.media.len(), 1);
        assert_eq!(
            bookmark.media[0].url.as_str(),
            "https://example.com/img.png"
        );
        assert_eq!(bookmark.media[0].alt_text.as_deref(), Some("A diagram"));

        let quote = bookmark.quoted.as_ref().expect("v2 quote should resolve");
        assert!(!quote.unavailable);
        assert_eq!(quote.tweet_url, "https://x.com/quoted_author/status/9001");
        assert_eq!(quote.content, "the quoted original");
        assert_eq!(quote.author_handle, "quoted_author");
        assert_eq!(quote.author_name, "Quoted Author");
        assert_eq!(
            quote.author_profile_image.as_deref(),
            Some("https://example.com/q.png")
        );
        assert_eq!(quote.media.len(), 1);
        assert_eq!(quote.media[0].media_type, MediaType::Video);
        assert_eq!(quote.media[0].duration_ms, Some(42000));
        assert_eq!(
            quote.media[0].poster_url.as_deref(),
            Some("https://example.com/poster.jpg")
        );
    }

    #[test]
    fn v2_quote_missing_from_includes_is_unavailable_with_url() {
        let bookmarks = JsonParser::new()
            .parse_str(
                r#"{
                  "data": [{
                    "id": "2002",
                    "text": "quoting a post that is not included",
                    "created_at": "2026-05-04T10:00:00.000Z",
                    "author_id": "u1",
                    "referenced_tweets": [{ "type": "quoted", "id": "9002" }]
                  }],
                  "includes": {
                    "users": [{ "id": "u1", "name": "Ada", "username": "ada_dev" }]
                  }
                }"#,
            )
            .unwrap();

        let quote = bookmarks[0].quoted.as_ref().expect("quote should parse");
        assert!(quote.unavailable);
        assert_eq!(quote.tweet_url, "https://x.com/i/web/status/9002");
        assert!(quote.content.is_empty());
    }

    #[test]
    fn plain_v2_payload_survives_round_trip_through_graph_and_calendar_shape() {
        // A v2 payload without a quote is still parsed through the v2 path and
        // keeps import parity with the archive parsers.
        let bookmarks = JsonParser::new()
            .parse_str(
                r#"{
                  "data": [{
                    "id": "3001",
                    "text": "plain v2 post #design",
                    "created_at": "2026-05-05T12:00:00.000Z",
                    "author_id": "u9",
                    "entities": { "hashtags": [{ "tag": "design" }] }
                  }],
                  "includes": {
                    "users": [{ "id": "u9", "name": "Bea", "username": "bea_design" }]
                  }
                }"#,
            )
            .unwrap();

        assert_eq!(bookmarks.len(), 1);
        assert_eq!(
            bookmarks[0].tweet_url,
            "https://x.com/bea_design/status/3001"
        );
        assert_eq!(bookmarks[0].author_name, "Bea");
        assert_eq!(bookmarks[0].tags, vec!["design"]);
        assert!(bookmarks[0].media.is_empty());
        assert!(bookmarks[0].quoted.is_none());
    }

    #[test]
    fn v2_payload_quoting_an_archive_style_embedded_status_keeps_the_body() {
        let bookmarks = JsonParser::new()
            .parse_str(
                r#"{"data":[{"id":"4001","text":"v2 entry with legacy quote","created_at":"2026-05-06T12:00:00Z","author_id":"u4","quoted_status_id_str":"not-used-because-embedded","quoted_status":{"id_str":"7001","full_text":"legacy quoted body","created_at":"2026-05-01T08:00:00Z","user":{"screen_name":"legacy_quote","name":"Legacy Quote"}}}],"includes":{"users":[{"id":"u4","name":"Cy","username":"cy_ops"}]}}"#,
            )
            .unwrap();

        let quote = bookmarks[0].quoted.as_ref().expect("quote should resolve");
        assert!(!quote.unavailable);
        assert_eq!(quote.content, "legacy quoted body");
        assert_eq!(quote.author_handle, "legacy_quote");
        assert_eq!(quote.tweet_url, "https://x.com/legacy_quote/status/7001");
    }
}
