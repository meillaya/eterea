//! URL entities attached to a post.
//!
//! X exports list URLs found in post text as *entities* (`entities.urls` in API
//! v2, `entities.urls` in archive JSON, `links`/`urls` in equivalent exports).
//! Each entity keeps the compact URL plus, when the export carries them, the
//! expanded and display forms. Links are display/filter data only: eterea never
//! fetches them.

use eterea_logic::{classify_link, LinkKind as LogicLinkKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Storage/display representation of one URL entity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct LinkEntity {
    /// The URL as it appeared in the post (usually a `t.co` short link).
    pub url: String,

    /// Resolved destination from the export, when present.
    #[serde(default)]
    pub expanded_url: Option<String>,

    /// Human-readable form the platform shows for the link.
    #[serde(default)]
    pub display_url: Option<String>,

    /// Classification: another post, a media file, or an external page.
    #[serde(default)]
    pub kind: LinkKind,
}

/// Classified URL kind stored with a link entity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// Another X/Twitter post.
    Tweet,
    /// A media file host or image/video URL.
    Media,
    /// Any other external destination.
    #[default]
    External,
}

impl From<LogicLinkKind> for LinkKind {
    fn from(kind: LogicLinkKind) -> Self {
        match kind {
            LogicLinkKind::Tweet => Self::Tweet,
            LogicLinkKind::Media => Self::Media,
            LogicLinkKind::External => Self::External,
        }
    }
}

impl From<LinkKind> for LogicLinkKind {
    fn from(kind: LinkKind) -> Self {
        match kind {
            LinkKind::Tweet => Self::Tweet,
            LinkKind::Media => Self::Media,
            LinkKind::External => Self::External,
        }
    }
}

impl LinkEntity {
    /// Build a link entity, classifying it from the available URL forms.
    pub fn new(
        url: impl Into<String>,
        expanded_url: Option<String>,
        display_url: Option<String>,
    ) -> Self {
        let url = url.into();
        let expanded_url = non_empty(expanded_url);
        let display_url = non_empty(display_url);
        let kind = classify_link(&url, expanded_url.as_deref()).into();
        Self {
            url,
            expanded_url,
            display_url,
            kind,
        }
    }

    /// Build a link entity with an explicit kind (used by import fallbacks that
    /// already know the classification).
    pub fn with_kind(
        url: impl Into<String>,
        expanded_url: Option<String>,
        display_url: Option<String>,
        kind: LinkKind,
    ) -> Self {
        Self {
            url: url.into(),
            expanded_url: non_empty(expanded_url),
            display_url: non_empty(display_url),
            kind,
        }
    }

    /// The resolved destination when present, otherwise the compact URL.
    pub fn destination(&self) -> &str {
        self.expanded_url.as_deref().unwrap_or(&self.url)
    }
}

/// Deduplicate and sort URL entities deterministically.
///
/// Entities are keyed by their compact URL: an entity repeated by multiple
/// source shapes is merged (first non-empty expanded/display form wins), and a
/// stronger classification (Tweet > Media > External) is kept. The result is
/// sorted by URL so the stored `links_json` is stable across imports.
pub fn normalize_links(links: Vec<LinkEntity>) -> Vec<LinkEntity> {
    let mut by_url: BTreeMap<String, LinkEntity> = BTreeMap::new();

    for link in links {
        let url = link.url.trim().to_string();
        if url.is_empty() {
            continue;
        }

        match by_url.get_mut(&url) {
            Some(existing) => {
                if existing.expanded_url.is_none() {
                    existing.expanded_url = non_empty(link.expanded_url);
                }
                if existing.display_url.is_none() {
                    existing.display_url = non_empty(link.display_url);
                }
                existing.kind = stronger_kind(existing.kind, link.kind);
            }
            None => {
                by_url.insert(
                    url.clone(),
                    LinkEntity {
                        url,
                        expanded_url: non_empty(link.expanded_url),
                        display_url: non_empty(link.display_url),
                        kind: link.kind,
                    },
                );
            }
        }
    }

    by_url.into_values().collect()
}

fn stronger_kind(left: LinkKind, right: LinkKind) -> LinkKind {
    fn rank(kind: LinkKind) -> u8 {
        match kind {
            LinkKind::Tweet => 2,
            LinkKind::Media => 1,
            LinkKind::External => 0,
        }
    }
    if rank(right) > rank(left) {
        right
    } else {
        left
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|text| {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_compact_or_expanded_url() {
        let tweet = LinkEntity::new(
            "https://t.co/abc",
            Some("https://x.com/ada/status/1".to_string()),
            Some("x.com/ada/status/1".to_string()),
        );
        assert_eq!(tweet.kind, LinkKind::Tweet);

        let media = LinkEntity::new("https://pic.twitter.com/xyz", None, None);
        assert_eq!(media.kind, LinkKind::Media);

        let external = LinkEntity::new("https://example.com/post", None, None);
        assert_eq!(external.kind, LinkKind::External);
    }

    #[test]
    fn normalization_is_sorted_deduped_and_merged() {
        let links = vec![
            LinkEntity::new("https://t.co/b", None, None),
            LinkEntity::new(
                "https://t.co/a",
                Some("https://x.com/ada/status/9".to_string()),
                None,
            ),
            LinkEntity::with_kind(
                "https://t.co/b",
                Some("https://example.com/b".to_string()),
                None,
                LinkKind::External,
            ),
            LinkEntity::new(
                "https://t.co/a",
                None,
                Some("x.com/ada/status/9".to_string()),
            ),
            LinkEntity::new("   ", None, None),
        ];

        let normalized = normalize_links(links);

        assert_eq!(normalized.len(), 2);
        assert_eq!(normalized[0].url, "https://t.co/a");
        assert_eq!(
            normalized[0].expanded_url.as_deref(),
            Some("https://x.com/ada/status/9")
        );
        assert_eq!(
            normalized[0].display_url.as_deref(),
            Some("x.com/ada/status/9")
        );
        assert_eq!(normalized[0].kind, LinkKind::Tweet);
        assert_eq!(normalized[1].url, "https://t.co/b");
        assert_eq!(
            normalized[1].expanded_url.as_deref(),
            Some("https://example.com/b")
        );
    }

    #[test]
    fn normalization_keeps_stronger_kind() {
        let links = normalize_links(vec![
            LinkEntity::with_kind("https://x.com/a/status/1", None, None, LinkKind::External),
            LinkEntity::with_kind("https://x.com/a/status/1", None, None, LinkKind::Tweet),
        ]);
        assert_eq!(links[0].kind, LinkKind::Tweet);
    }
}
