//! Quoted-post extraction for archive JSON and X API v2 JSON payloads.
//!
//! Two source shapes are supported:
//!
//! - Archive/v1.1-shaped entries embed a `quoted_status` object and/or a
//!   `quoted_status_id_str` id.
//! - X API v2 entries reference a quoted post through `referenced_tweets` and
//!   resolve the body from root `includes.tweets`, `includes.users`, and
//!   `includes.media` (handled by the v2 parser, sharing the helpers here).
//!
//! When only an id is present the quote becomes [`QuotedTweet::unavailable`]:
//! the URL is kept and no text, author, or date is invented.

use super::date_parse::{parse_date_candidates, tweet_url_for};
use super::media_metadata::{extract_media, media_from_value};
use crate::models::{Media, QuotedTweet};
use serde_json::Value;
use std::collections::HashMap;

/// Object embedded in an archive entry that holds the quoted post body.
pub(crate) fn embedded_quoted_object(raw: &Value) -> Option<&Value> {
    for path in [
        &["quoted_status"][..],
        &["quoted_status_result", "result"][..],
        &["quoted_tweet"][..],
        &["quotedTweet"][..],
        &["tweet", "quoted_status"][..],
    ] {
        if let Some(quoted) = value_at_path(raw, path) {
            if quoted.is_object() {
                return Some(quoted);
            }
        }
    }
    None
}

/// Quoted post id carried directly by the entry, without a body.
pub(crate) fn bare_quoted_id(raw: &Value) -> Option<String> {
    for path in [
        &["quoted_status_id_str"][..],
        &["quoted_status_id"][..],
        &["quoted_tweet_id_str"][..],
        &["quoted_tweet_id"][..],
    ] {
        if let Some(id) = value_at_path(raw, path).and_then(scalar_to_string) {
            return Some(id);
        }
    }
    None
}

/// Id of the post referenced by an X API v2 `referenced_tweets` list when the
/// reference type is `quoted`.
pub(crate) fn quoted_reference_id(raw: &Value) -> Option<String> {
    let references = value_at_path(raw, &["referenced_tweets"])?;
    let Value::Array(references) = references else {
        return None;
    };

    references.iter().find_map(|reference| {
        let reference_type = value_at_path(reference, &["type"]).and_then(scalar_to_string)?;
        if reference_type != "quoted" {
            return None;
        }
        value_at_path(reference, &["id"]).and_then(scalar_to_string)
    })
}

/// Build an available quote from an embedded tweet-like object.
///
/// Returns `None` when the object carries no id/url and therefore cannot be
/// canonicalised; the caller then falls back to other quote sources.
pub(crate) fn quoted_tweet_from_object(quoted: &Value) -> Option<QuotedTweet> {
    let tweet_id = string_at(
        quoted,
        &[
            &["id_str"],
            &["id"],
            &["tweet_id"],
            &["tweetId"],
            &["rest_id"],
        ],
    );
    let author_handle = string_at(
        quoted,
        &[
            &["user", "screen_name"],
            &["user", "legacy", "screen_name"],
            &["screen_name"],
            &["author_handle"],
            &["core", "user_results", "result", "legacy", "screen_name"],
        ],
    )
    .or_else(|| {
        string_at(
            quoted,
            &[
                &["tweet_url"],
                &["url"],
                &["expanded_url"],
                &["expandedUrl"],
            ],
        )
        .and_then(|url| handle_from_url(&url))
    })
    .unwrap_or_default();

    let tweet_url = string_at(
        quoted,
        &[
            &["tweet_url"],
            &["url"],
            &["expanded_url"],
            &["expandedUrl"],
            &["legacy", "expanded_url"],
        ],
    )
    .filter(|url| url_has_status_id(url))
    .or_else(|| {
        tweet_id
            .as_deref()
            .map(|id| tweet_url_for(&author_handle, id))
    })?;

    let content = string_at(
        quoted,
        &[
            &["full_text"],
            &["text"],
            &["content"],
            &["legacy", "full_text"],
        ],
    )
    .unwrap_or_default();

    let author_name = string_at(
        quoted,
        &[
            &["user", "name"],
            &["name"],
            &["author_name"],
            &["core", "user_results", "result", "legacy", "name"],
        ],
    )
    .unwrap_or_else(|| author_handle.clone());

    let author_profile_image = string_at(
        quoted,
        &[
            &["user", "profile_image_url_https"],
            &["profile_image_url_https"],
            &["profile_image"],
            &["user", "legacy", "profile_image_url_https"],
            &[
                "core",
                "user_results",
                "result",
                "legacy",
                "profile_image_url_https",
            ],
        ],
    );

    let tweeted_at = date_from_value(quoted);
    let media = extract_media(quoted);

    Some(
        QuotedTweet::new(tweet_url, content, tweeted_at, author_handle, author_name)
            .with_media(media)
            .with_author_profile_image(author_profile_image.unwrap_or_default()),
    )
}

/// Build an available quote from an X API v2 tweet object resolved from
/// `includes.tweets`.
pub(crate) fn quoted_tweet_from_v2(
    quoted: &Value,
    users_by_id: &HashMap<String, &Value>,
    media_by_key: &HashMap<String, &Value>,
) -> Option<QuotedTweet> {
    let tweet_id = string_at(quoted, &[&["id"], &["id_str"]])?;
    let author = string_at(quoted, &[&["author_id"]])
        .and_then(|author_id| users_by_id.get(&author_id).copied());
    let author_handle = author
        .and_then(|user| string_at(user, &[&["username"], &["screen_name"]]))
        .or_else(|| {
            string_at(quoted, &[&["tweet_url"], &["url"], &["expanded_url"]])
                .and_then(|url| handle_from_url(&url))
        })
        .unwrap_or_default();
    let author_name = author
        .and_then(|user| string_at(user, &[&["name"]]))
        .unwrap_or_else(|| author_handle.clone());
    let author_profile_image = author.and_then(|user| {
        string_at(
            user,
            &[
                &["profile_image_url"],
                &["profile_image_url_https"],
                &["profile_image"],
            ],
        )
    });
    let content = string_at(quoted, &[&["text"], &["full_text"]]).unwrap_or_default();
    let tweeted_at = string_at(quoted, &[&["created_at"]])
        .as_deref()
        .and_then(|value| parse_date_candidates([Some(value), None, None]).ok());
    let media = media_from_media_keys(quoted, media_by_key);

    Some(
        QuotedTweet::new(
            tweet_url_for(&author_handle, &tweet_id),
            content,
            tweeted_at,
            author_handle,
            author_name,
        )
        .with_media(media)
        .with_author_profile_image(author_profile_image.unwrap_or_default()),
    )
}

/// Media for an X API v2 tweet, resolved through `attachments.media_keys` and
/// the root `includes.media` list.
pub(crate) fn media_from_media_keys(
    tweet: &Value,
    media_by_key: &HashMap<String, &Value>,
) -> Vec<Media> {
    let mut media = Vec::new();
    let Some(Value::Array(keys)) = value_at_path(tweet, &["attachments", "media_keys"]) else {
        return media;
    };

    for key in keys {
        let Some(key) = scalar_to_string(key) else {
            continue;
        };
        let Some(raw_media) = media_by_key.get(&key) else {
            continue;
        };
        let Some(mut candidate) = media_from_value(raw_media) else {
            continue;
        };
        if candidate.source_media_key.is_none() {
            candidate.source_media_key = Some(key);
        }
        media.push(candidate);
    }

    media
}

fn date_from_value(raw: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
    let candidates = [
        string_at(raw, &[&["tweeted_at"]]),
        string_at(raw, &[&["created_at"]]),
        string_at(raw, &[&["legacy", "created_at"]]),
    ];
    parse_date_candidates([
        candidates[0].as_deref(),
        candidates[1].as_deref(),
        candidates[2].as_deref(),
    ])
    .ok()
}

fn handle_from_url(url: &str) -> Option<String> {
    let trimmed = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let path = trimmed.split_once('/').map(|(_, rest)| rest)?;
    let handle = path.split('/').next()?.trim();
    if handle.is_empty() || handle == "i" {
        return None;
    }
    Some(handle.trim_start_matches('@').to_string())
}

fn url_has_status_id(url: &str) -> bool {
    url.contains("/status/")
}

fn string_at(raw: &Value, paths: &[&[&str]]) -> Option<String> {
    paths
        .iter()
        .find_map(|path| value_at_path(raw, path))
        .and_then(scalar_to_string)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn value_at_path<'a>(raw: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = raw;
    for segment in path {
        current = current.get(*segment)?;
    }
    Some(current)
}
