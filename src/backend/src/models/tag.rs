//! Tag mutations and tag colors.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One tag edit applied to a bookmark selection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mutation", content = "tags", rename_all = "snake_case")]
pub enum TagMutation {
    /// Add the listed tags, keeping the existing ones.
    Add(Vec<String>),
    /// Remove the listed tags, keeping the rest.
    Remove(Vec<String>),
    /// Replace the bookmark tags with exactly the listed tags.
    Set(Vec<String>),
}

impl TagMutation {
    /// Stable string used in stored history payloads.
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Add(_) => "add",
            Self::Remove(_) => "remove",
            Self::Set(_) => "set",
        }
    }

    /// The tags carried by the mutation.
    pub fn tags(&self) -> &[String] {
        match self {
            Self::Add(tags) | Self::Remove(tags) | Self::Set(tags) => tags,
        }
    }

    /// Trim, drop empty tags, and deduplicate case-insensitively while keeping
    /// the first spelling. The tag tables are `COLLATE NOCASE`, so this keeps
    /// the mutation idempotent.
    pub fn normalized(self) -> Self {
        fn clean(tags: Vec<String>) -> Vec<String> {
            let mut seen = BTreeSet::new();
            let mut cleaned = Vec::new();
            for tag in tags {
                let trimmed = tag.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if seen.insert(trimmed.to_lowercase()) {
                    cleaned.push(trimmed.to_string());
                }
            }
            cleaned
        }

        match self {
            Self::Add(tags) => Self::Add(clean(tags)),
            Self::Remove(tags) => Self::Remove(clean(tags)),
            Self::Set(tags) => Self::Set(clean(tags)),
        }
    }

    /// Whether the mutation would do nothing before touching the database.
    pub fn is_empty(&self) -> bool {
        self.tags().is_empty()
    }
}

/// One tag color row. Colors are validated `#rrggbb` hex triplets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TagColor {
    pub tag: String,
    pub color: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_mutations_normalize_dedupe_and_sort() {
        let mutation = TagMutation::Add(vec![
            " rust ".to_string(),
            "Rust".to_string(),
            "".to_string(),
            "design".to_string(),
        ])
        .normalized();

        assert_eq!(
            mutation,
            TagMutation::Add(vec!["rust".to_string(), "design".to_string()])
        );
        assert_eq!(mutation.kind_str(), "add");
        assert!(!mutation.is_empty());
        assert!(TagMutation::Set(Vec::new()).normalized().is_empty());
    }
}
