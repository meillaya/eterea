//! URL entity extraction for JSON/archive payloads.
//!
//! Exports carry the URLs found in post text in a few equivalent shapes:
//!
//! - X API v2 / archive: `entities.urls` entries with `url`, `expanded_url`,
//!   `display_url` (and sometimes `media_key`).
//! - Nested legacy forms: `legacy.entities.urls`, `tweet.entities.urls`,
//!   `extended_tweet.entities.urls`.
//! - Flat forms: a `links`/`urls` array of objects or plain strings.
//!
//! Every shape funnels into [`LinkEntity`] and is normalized (sorted, deduped)
//! by the bookmark builder before it is stored.

use crate::models::{LinkEntity, LinkKind};
use serde_json::Value;

/// Extract URL entities from any supported JSON entry shape.
pub(crate) fn extract_links(raw: &Value) -> Vec<LinkEntity> {
    let mut links = Vec::new();

    for path in [
        &["entities", "urls"][..],
        &["legacy", "entities", "urls"][..],
        &["tweet", "entities", "urls"][..],
        &["extended_tweet", "entities", "urls"][..],
        &["legacy", "extended_entities", "urls"][..],
        &["urls"][..],
        &["links"][..],
    ] {
        let Some(Value::Array(items)) = value_at_path(raw, path) else {
            continue;
        };
        for item in items {
            if let Some(link) = link_from_value(item) {
                links.push(link);
            }
        }
    }

    links
}

fn link_from_value(value: &Value) -> Option<LinkEntity> {
    match value {
        Value::String(url) => {
            let url = url.trim();
            if url.is_empty() {
                return None;
            }
            Some(LinkEntity::new(url, None, None))
        }
        Value::Object(_) => {
            let url = string_at(value, &["url"])
                .or_else(|| string_at(value, &["expanded_url"]))
                .or_else(|| string_at(value, &["expandedUrl"]))?;
            let expanded_url = string_at(value, &["expanded_url"])
                .or_else(|| string_at(value, &["expandedUrl"]))
                .or_else(|| string_at(value, &["unwound_url"]));
            let display_url =
                string_at(value, &["display_url"]).or_else(|| string_at(value, &["displayUrl"]));

            // A media-key-bearing entity is an attachment link, not a post link.
            let kind = if string_at(value, &["media_key"]).is_some() {
                LinkKind::Media
            } else {
                LinkEntity::new(&url, expanded_url.clone(), display_url.clone()).kind
            };
            Some(LinkEntity::with_kind(url, expanded_url, display_url, kind))
        }
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

fn string_at(raw: &Value, path: &[&str]) -> Option<String> {
    value_at_path(raw, path)
        .and_then(|value| match value {
            Value::String(text) => Some(text.trim().to_string()),
            _ => None,
        })
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_v2_and_archive_entity_shapes() {
        let raw = json!({
            "entities": {
                "urls": [
                    {
                        "url": "https://t.co/a",
                        "expanded_url": "https://x.com/ada/status/9",
                        "display_url": "x.com/ada/status/9"
                    },
                    {
                        "url": "https://t.co/b",
                        "expanded_url": "https://pbs.twimg.com/media/x.jpg",
                        "media_key": "3_1"
                    }
                ]
            },
            "legacy": {
                "entities": {
                    "urls": [
                        { "url": "https://t.co/c", "expanded_url": "https://example.com/post" }
                    ]
                }
            }
        });

        let links = extract_links(&raw);
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].kind, LinkKind::Tweet);
        assert_eq!(links[1].kind, LinkKind::Media);
        assert_eq!(links[2].kind, LinkKind::External);
    }

    #[test]
    fn extracts_flat_string_and_object_link_forms() {
        let raw = json!({
            "links": ["https://example.com/a", "https://x.com/ada/status/1"],
            "urls": [{ "url": "https://example.com/b" }]
        });
        let links = extract_links(&raw);
        assert_eq!(links.len(), 3);
    }
}
