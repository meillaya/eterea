//! Opt-in metadata repair from X's embed backend.
//!
//! The reader can only show what an export carried. A text-only bookmarks CSV
//! has no media or quote data, so those posts stay text until a richer source
//! supplies it. This module fetches the public tweet payload that X's embedded
//! tweet widget itself uses (`cdn.syndication.twimg.com/tweet-result`), which
//! needs no account, cookie or bearer token. It is deliberately **not** the
//! official API and X may change it at any time; callers must keep it opt-in,
//! paced, and treat every failure as ordinary missing data.
//!
//! The module only fetches and parses JSON. It never downloads media bytes and
//! never writes to SQLite; mapping the payload onto the archive and merging it
//! is the application's job.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::MediaError;
use crate::http::map_transport_error;
use crate::{HTTP_TIMEOUT, USER_AGENT};

/// Base URL of the public embed payload endpoint.
const SYNDICATION_BASE: &str = "https://cdn.syndication.twimg.com/tweet-result";

/// Hard cap on one embed payload. A normal tweet is a few KiB; quote chains a
/// few tens of KiB.
pub const MAX_SYNDICATION_BYTES: usize = 2 * 1024 * 1024;

/// One media attachment in an embed payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyndicatedMedia {
    /// Attachment kind (`photo`, `video`, `animated_gif`).
    pub kind: SyndicatedMediaKind,
    /// Image URL, or the video/GIF poster image.
    pub url: String,
    /// Pixel width from `original_info`, when the payload carries it.
    pub width: Option<i64>,
    /// Pixel height from `original_info`, when the payload carries it.
    pub height: Option<i64>,
    /// Video/GIF duration in milliseconds.
    pub duration_ms: Option<i64>,
    /// Alt text, when the payload carries it.
    pub alt_text: Option<String>,
    /// Video variants (progressive MP4 and HLS), empty for photos.
    pub variants: Vec<SyndicatedVariant>,
}

/// Kind of an embed media attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyndicatedMediaKind {
    /// Still image.
    Photo,
    /// Video.
    Video,
    /// Animated GIF.
    AnimatedGif,
}

/// One video variant as the embed payload reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyndicatedVariant {
    /// `content_type` (`video/mp4`, `application/x-mpegURL`, ...).
    pub content_type: Option<String>,
    /// Declared bit rate, when present.
    pub bitrate: Option<u64>,
    /// Variant URL.
    pub url: String,
}

/// Author of an embed payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyndicatedAuthor {
    /// Stable numeric user id (`id_str`).
    pub id: String,
    /// Screen name, without a leading `@`.
    pub handle: String,
    /// Display name.
    pub name: String,
    /// Current profile image URL.
    pub avatar_url: Option<String>,
}

/// One URL entity in the post text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyndicatedLink {
    /// Shortened URL as it appears in the text (`t.co`).
    pub url: String,
    /// Resolved destination.
    pub expanded_url: Option<String>,
    /// Human-readable form.
    pub display_url: Option<String>,
}

/// A post quoted by the fetched post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyndicatedQuote {
    /// Numeric post id.
    pub id: String,
    /// Quoted text.
    pub text: String,
    /// RFC 3339 creation time.
    pub created_at: Option<String>,
    /// Quoted author.
    pub author: SyndicatedAuthor,
    /// Quoted media.
    pub media: Vec<SyndicatedMedia>,
}

/// One parsed embed payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyndicatedTweet {
    /// Numeric post id.
    pub id: String,
    /// Post text.
    pub text: String,
    /// Extended note-tweet text for long posts, when present.
    pub note_text: Option<String>,
    /// RFC 3339 creation time.
    pub created_at: Option<String>,
    /// Post author.
    pub author: SyndicatedAuthor,
    /// Attached media.
    pub media: Vec<SyndicatedMedia>,
    /// Quoted post, when this post quotes one.
    pub quoted: Option<SyndicatedQuote>,
    /// URL entities from the post text.
    pub links: Vec<SyndicatedLink>,
    /// Hashtags from the post text (without `#`).
    pub hashtags: Vec<String>,
}

/// Whether the payload says the post is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyndicationOutcome {
    /// The post exists and was parsed.
    Tweet(Box<SyndicatedTweet>),
    /// Deleted, protected or otherwise unavailable (`TweetTombstone`).
    Unavailable,
}

/// Fetch the public embed payload for one post.
///
/// `tweet_id` must be the decimal id. The endpoint is not the official API;
/// callers should treat it as best-effort and keep it opt-in.
pub struct SyndicationClient {
    agent: ureq::Agent,
}

impl SyndicationClient {
    /// Build the client with the crate's stable UA and timeout.
    pub fn new() -> Result<Self, MediaError> {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(HTTP_TIMEOUT))
            .https_only(true)
            .max_redirects(0)
            .user_agent(USER_AGENT)
            .build();
        Ok(Self {
            agent: ureq::Agent::new_with_config(config),
        })
    }

    /// Fetch and parse the payload. `cancel` is checked before the request and
    /// after the body is read; a tombstone is reported as
    /// [`SyndicationOutcome::Unavailable`] rather than a transport error.
    pub fn fetch(
        &self,
        tweet_id: &str,
        cancel: &AtomicBool,
    ) -> Result<SyndicationOutcome, MediaError> {
        let url = endpoint_url(tweet_id)?;
        if cancel.load(Ordering::Relaxed) {
            return Err(MediaError::Cancelled);
        }
        let mut response = match self.agent.get(&url).call() {
            Ok(response) => response,
            Err(error) => return Err(map_transport_error(error)),
        };
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(MediaError::HttpStatus(status));
        }
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_SYNDICATION_BYTES as u64)
            .read_to_string()
            .map_err(|_| MediaError::Connection)?;
        if cancel.load(Ordering::Relaxed) {
            return Err(MediaError::Cancelled);
        }
        parse_tweet_payload(&body)
    }
}

/// Build the request URL for one post id.
pub fn endpoint_url(tweet_id: &str) -> Result<String, MediaError> {
    let token = embed_token(tweet_id).ok_or(MediaError::InvalidConfig)?;
    Ok(format!(
        "{SYNDICATION_BASE}?id={tweet_id}&token={token}&lang=en"
    ))
}

/// Extract the decimal post id from an `x.com`/`twitter.com` status URL.
pub fn tweet_id_from_url(url: &str) -> Option<String> {
    let (_, after) = url.split_once("/status/")?;
    let digits: String = after.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if (5..=25).contains(&digits.len()) && digits.chars().all(|ch| ch.is_ascii_digit()) {
        Some(digits)
    } else {
        None
    }
}

/// The `token` query parameter X's embed widget sends for a post id.
///
/// This mirrors the published JS expression
/// `((Number(id) / 1e15) * Math.PI).toString(36).replace(/(0+|\.)/g, "")`.
pub fn embed_token(tweet_id: &str) -> Option<String> {
    let id: u64 = tweet_id.parse().ok()?;
    let value = (id as f64 / 1e15) * std::f64::consts::PI;
    let digits = "0123456789abcdefghijklmnopqrstuvwxyz";
    let mut integer = value.trunc() as u64;
    let mut fraction = value.fract();
    let mut text = String::new();
    if integer == 0 {
        text.push('0');
    }
    while integer > 0 {
        text.push(digits.as_bytes()[(integer % 36) as usize] as char);
        integer /= 36;
    }
    // Reverse the integer digits.
    let integer_len = text.len();
    text = text.chars().rev().collect();
    let _ = integer_len;
    for _ in 0..17 {
        fraction *= 36.0;
        let digit = fraction.trunc() as usize;
        fraction -= digit as f64;
        text.push(digits.as_bytes()[digit.min(35)] as char);
    }
    Some(text.replace(['.', '0'], ""))
}

/// Parse one embed payload body.
pub fn parse_tweet_payload(body: &str) -> Result<SyndicationOutcome, MediaError> {
    let root: serde_json::Value =
        serde_json::from_str(body).map_err(|_| MediaError::SyndicationResponse)?;
    let typename = root
        .get("__typename")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if typename == "TweetTombstone" || root.get("tombstone").is_some() {
        return Ok(SyndicationOutcome::Unavailable);
    }
    if typename != "Tweet" {
        return Err(MediaError::SyndicationResponse);
    }
    let tweet = tweet_from_value(&root)?;
    Ok(SyndicationOutcome::Tweet(Box::new(tweet)))
}

fn tweet_from_value(root: &serde_json::Value) -> Result<SyndicatedTweet, MediaError> {
    let id = string_field(root, "id_str").ok_or(MediaError::SyndicationResponse)?;
    let text = string_field(root, "text").unwrap_or_default();
    let note_text = root
        .get("note_tweet")
        .and_then(|note| note.get("text"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let created_at = string_field(root, "created_at");
    let author = author_from_value(root.get("user"))?;
    let media = media_from_array(root);
    let quoted = root
        .get("quoted_tweet")
        .filter(|value| value.is_object())
        .map(quote_from_value)
        .transpose()?;
    let (links, hashtags) = entities_from_value(root.get("entities"));
    Ok(SyndicatedTweet {
        id,
        text,
        note_text,
        created_at,
        author,
        media,
        quoted,
        links,
        hashtags,
    })
}

fn quote_from_value(root: &serde_json::Value) -> Result<SyndicatedQuote, MediaError> {
    let id = string_field(root, "id_str").ok_or(MediaError::SyndicationResponse)?;
    let text = string_field(root, "text").unwrap_or_default();
    let created_at = string_field(root, "created_at");
    let author = author_from_value(root.get("user"))?;
    let media = media_from_array(root);
    Ok(SyndicatedQuote {
        id,
        text,
        created_at,
        author,
        media,
    })
}

fn author_from_value(value: Option<&serde_json::Value>) -> Result<SyndicatedAuthor, MediaError> {
    let user = value.ok_or(MediaError::SyndicationResponse)?;
    Ok(SyndicatedAuthor {
        id: string_field(user, "id_str").unwrap_or_default(),
        handle: string_field(user, "screen_name").unwrap_or_default(),
        name: string_field(user, "name").unwrap_or_default(),
        avatar_url: string_field(user, "profile_image_url_https"),
    })
}

fn media_from_array(root: &serde_json::Value) -> Vec<SyndicatedMedia> {
    let Some(items) = root
        .get("mediaDetails")
        .or_else(|| root.get("media"))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    items.iter().filter_map(media_from_value).collect()
}

fn media_from_value(value: &serde_json::Value) -> Option<SyndicatedMedia> {
    let kind = match string_field(value, "type")?.as_str() {
        "photo" => SyndicatedMediaKind::Photo,
        "video" => SyndicatedMediaKind::Video,
        "animated_gif" => SyndicatedMediaKind::AnimatedGif,
        _ => return None,
    };
    let url = string_field(value, "media_url_https")?;
    let (width, height) = value
        .get("original_info")
        .map(|info| {
            (
                info.get("width").and_then(serde_json::Value::as_i64),
                info.get("height").and_then(serde_json::Value::as_i64),
            )
        })
        .unwrap_or((None, None));
    let video_info = value.get("video_info");
    let duration_ms = video_info
        .and_then(|info| info.get("duration_millis"))
        .and_then(serde_json::Value::as_i64);
    let variants = video_info
        .and_then(|info| info.get("variants"))
        .and_then(serde_json::Value::as_array)
        .map(|variants| {
            variants
                .iter()
                .filter_map(|variant| {
                    Some(SyndicatedVariant {
                        content_type: string_field(variant, "content_type"),
                        bitrate: variant.get("bitrate").and_then(serde_json::Value::as_u64),
                        url: string_field(variant, "url")?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let alt_text = string_field(value, "ext_alt_text").or_else(|| {
        value
            .get("additional_media_info")
            .and_then(|info| info.get("description"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    });
    Some(SyndicatedMedia {
        kind,
        url,
        width,
        height,
        duration_ms,
        alt_text,
        variants,
    })
}

type Entities = (Vec<SyndicatedLink>, Vec<String>);

fn entities_from_value(value: Option<&serde_json::Value>) -> Entities {
    let Some(entities) = value else {
        return (Vec::new(), Vec::new());
    };
    let links = entities
        .get("urls")
        .and_then(serde_json::Value::as_array)
        .map(|urls| {
            urls.iter()
                .filter_map(|url| {
                    Some(SyndicatedLink {
                        url: string_field(url, "url")?,
                        expanded_url: string_field(url, "expanded_url"),
                        display_url: string_field(url, "display_url"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let hashtags = entities
        .get("hashtags")
        .and_then(serde_json::Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| string_field(tag, "text"))
                .collect()
        })
        .unwrap_or_default();
    (links, hashtags)
}

fn string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(|value| match value {
        serde_json::Value::String(text) if !text.is_empty() => Some(text.clone()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_token_matches_known_vectors() {
        assert_eq!(
            embed_token("2122399349695303865").as_deref(),
            Some("557pply4exyb1halwhfr")
        );
        assert_eq!(
            embed_token("2122298644456198572").as_deref(),
            Some("557ebl5vp37vtjnfz2g")
        );
        assert!(embed_token("not-a-number").is_none());
    }

    #[test]
    fn tweet_ids_parse_from_status_urls() {
        assert_eq!(
            tweet_id_from_url("https://x.com/ke_keroscene/status/2122399349695303865").as_deref(),
            Some("2122399349695303865")
        );
        assert_eq!(
            tweet_id_from_url("https://twitter.com/nnikks_/status/484770841087840256?ref=abc")
                .as_deref(),
            Some("484770841087840256")
        );
        assert!(tweet_id_from_url("https://x.com/a").is_none());
        assert!(tweet_id_from_url("https://x.com/a/status/").is_none());
    }

    #[test]
    fn tombstone_is_reported_unavailable() {
        let body = r#"{"__typename":"TweetTombstone","tombstone":{"text":"gone"}}"#;
        assert_eq!(
            parse_tweet_payload(body).unwrap(),
            SyndicationOutcome::Unavailable
        );
    }

    #[test]
    fn non_tweet_payload_is_rejected() {
        assert_eq!(
            parse_tweet_payload(r#"{"ok":true}"#),
            Err(MediaError::SyndicationResponse)
        );
    }
}
