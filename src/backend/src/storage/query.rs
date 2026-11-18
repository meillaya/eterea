//! Bookmark query, selection, and export request types shared by the service
//! layer and the background store actor.
//!
//! [`BookmarkQuery`] is the one filter shape used for paged reads, bulk
//! mutations, and exports. `from_date`/`to_date` stay RFC 3339 strings so the
//! type serializes cleanly across the UI boundary; the data layer parses them
//! once per request.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Page + filter description for bookmark reads and bulk selection.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BookmarkQuery {
    pub query: Option<String>,
    pub tag: Option<String>,
    /// Extra tags that must all be present (composable filters/saved searches).
    #[serde(default)]
    pub tags: Vec<String>,
    pub author: Option<String>,
    /// RFC 3339 lower bound on the posted date (inclusive).
    pub from_date: Option<String>,
    /// RFC 3339 upper bound on the posted date (inclusive).
    pub to_date: Option<String>,
    pub favorites_only: bool,
    /// Only bookmarks the reader has not marked read.
    #[serde(default)]
    pub unread_only: bool,
    pub has_media: Option<bool>,
    pub offset: usize,
    pub limit: usize,
}

impl BookmarkQuery {
    /// True when any filter field is set. Pagination alone is not filtering.
    pub fn is_filtered(&self) -> bool {
        self.query
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || self.tag.is_some()
            || !self.tags.is_empty()
            || self.author.is_some()
            || self.from_date.is_some()
            || self.to_date.is_some()
            || self.favorites_only
            || self.unread_only
            || self.has_media.is_some()
    }
}

/// The set of bookmarks a bulk mutation or export applies to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "selection", content = "payload", rename_all = "snake_case")]
pub enum BookmarkSelection {
    /// Exactly the listed bookmark ids (unknown ids are skipped and counted).
    Ids(Vec<String>),
    /// Every bookmark matching the query; matching uses SQL predicates, never
    /// a materialized id list.
    Query(BookmarkQuery),
}

impl BookmarkSelection {
    /// Ids selection helper for single-bookmark operations.
    pub fn single(id: impl Into<String>) -> Self {
        Self::Ids(vec![id.into()])
    }

    /// `true` when the selection is an explicit id list.
    pub fn is_ids(&self) -> bool {
        matches!(self, Self::Ids(_))
    }
}

/// Export output format.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Csv,
    Json,
}

impl ExportFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }
}

/// What an export covers: the whole archive, the current filter, or an
/// explicit selection of bookmarks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "scope", content = "payload", rename_all = "snake_case")]
pub enum ExportScope {
    All,
    Query(BookmarkQuery),
    Ids(Vec<String>),
}

impl ExportScope {
    /// The scope equivalent of a selection.
    pub fn from_selection(selection: &BookmarkSelection) -> Self {
        match selection {
            BookmarkSelection::Ids(ids) => Self::Ids(ids.clone()),
            BookmarkSelection::Query(query) => Self::Query(query.clone()),
        }
    }
}

/// One export request: scope, format, and the caller-owned destination path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExportRequest {
    pub scope: ExportScope,
    pub format: ExportFormat,
    pub path: PathBuf,
}

/// Result of a completed export.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExportSummary {
    pub path: PathBuf,
    pub format: ExportFormat,
    pub count: usize,
    pub bytes: u64,
}

impl BookmarkQuery {
    /// Normalize the whitespace-only query text so a blank search is no filter.
    pub fn normalized_query(&self) -> Option<String> {
        self.query.as_deref().and_then(|candidate| {
            let trimmed = candidate.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unfiltered_query_is_recognized() {
        assert!(!BookmarkQuery::default().is_filtered());
        assert!(!BookmarkQuery {
            offset: 10,
            limit: 48,
            ..BookmarkQuery::default()
        }
        .is_filtered());
        assert!(BookmarkQuery {
            unread_only: true,
            ..BookmarkQuery::default()
        }
        .is_filtered());
        assert!(
            !BookmarkQuery {
                query: Some("   ".to_string()),
                ..BookmarkQuery::default()
            }
            .is_filtered(),
            "a whitespace-only search is not a filter"
        );
    }

    #[test]
    fn selection_and_export_scope_round_trip() {
        let selection = BookmarkSelection::Ids(vec!["a".to_string()]);
        assert!(selection.is_ids());
        assert_eq!(
            ExportScope::from_selection(&selection),
            ExportScope::Ids(vec!["a".to_string()])
        );

        let json = serde_json::to_string(&selection).unwrap();
        assert!(json.contains("\"selection\":\"ids\""));
        assert_eq!(
            serde_json::from_str::<BookmarkSelection>(&json).unwrap(),
            selection
        );

        let query = BookmarkQuery {
            tag: Some("rust".to_string()),
            ..BookmarkQuery::default()
        };
        let scope = ExportScope::from_selection(&BookmarkSelection::Query(query.clone()));
        assert_eq!(scope, ExportScope::Query(query));
    }
}
