//! Importers for eterea's own export formats.
//!
//! Exports are written by [`crate::storage`] and must round-trip: the JSON
//! format is an array of full [`Bookmark`] values under a version marker, and
//! the CSV format is header-addressed with JSON cells for tags, media, quotes,
//! and links. Importing an export restores tags, media metadata,
//! quoted posts, URL entities, favorite, and read state.

use crate::models::{
    Bookmark, BookmarkBuilder, LinkEntity, Media, QuotedTweet, MAX_MEDIA_PER_BOOKMARK,
};
use crate::storage::{EXPORT_CSV_MARKER, EXPORT_JSON_FORMAT, EXPORT_JSON_VERSION};
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use csv::StringRecord;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use tracing::{debug, warn};
use uuid::Uuid;

/// Parse an eterea JSON export root, if it is one.
pub(crate) fn parse_eterea_export_json(root: &Value) -> Result<Option<Vec<Bookmark>>> {
    let is_marker = root
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|format| format == EXPORT_JSON_FORMAT);
    if !is_marker {
        return Ok(None);
    }

    let version = root
        .get("version")
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(EXPORT_JSON_VERSION));
    if version > u64::from(EXPORT_JSON_VERSION) {
        return Err(Error::InvalidFormat(format!(
            "eterea export version {version} is newer than supported {EXPORT_JSON_VERSION}"
        )));
    }

    let Some(Value::Array(items)) = root.get("bookmarks") else {
        return Err(Error::InvalidFormat(
            "eterea export is missing the bookmarks array".to_string(),
        ));
    };

    let mut bookmarks = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        match serde_json::from_value::<Bookmark>(item.clone()) {
            Ok(mut bookmark) => {
                normalize_imported_bookmark(&mut bookmark);
                bookmarks.push(bookmark);
            }
            Err(error) => {
                // One malformed object must not silently truncate the export.
                return Err(Error::InvalidFormat(format!(
                    "eterea export bookmark {index} is invalid: {error}"
                )));
            }
        }
    }
    debug!(
        "Parsed {} bookmarks from eterea JSON export",
        bookmarks.len()
    );
    Ok(Some(bookmarks))
}

/// Fill defaults and normalize an imported full bookmark.
pub(crate) fn normalize_imported_bookmark(bookmark: &mut Bookmark) {
    if bookmark.id.trim().is_empty() {
        bookmark.id = Uuid::new_v4().to_string();
    }
    bookmark.links = crate::models::normalize_links(std::mem::take(&mut bookmark.links));
    bookmark.media = std::mem::take(&mut bookmark.media)
        .into_iter()
        .take(MAX_MEDIA_PER_BOOKMARK)
        .collect();
}

/// Parser for the eterea CSV export format.
pub struct ExportCsvParser;

impl ExportCsvParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse<R: Read>(&self, reader: R) -> Result<Vec<Bookmark>> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .flexible(true)
            .from_reader(reader);
        let headers = reader.headers()?.clone();
        let header_index = header_index(&headers);
        if !header_index.contains_key(EXPORT_CSV_MARKER) {
            return Err(Error::InvalidFormat("not an eterea CSV export".to_string()));
        }

        let mut bookmarks = Vec::new();
        for (index, record) in reader.records().enumerate() {
            let record = match record {
                Ok(record) => record,
                Err(error) => {
                    warn!("CSV export row {} is malformed: {error}", index + 2);
                    continue;
                }
            };
            match bookmark_from_record(&record, &header_index) {
                Ok(Some(bookmark)) => bookmarks.push(bookmark),
                Ok(None) => {}
                Err(error) => warn!("Skipping CSV export row {}: {error}", index + 2),
            }
        }

        debug!(
            "Parsed {} bookmarks from eterea CSV export",
            bookmarks.len()
        );
        Ok(bookmarks)
    }

    pub fn parse_str(&self, content: &str) -> Result<Vec<Bookmark>> {
        self.parse(Cursor::new(content.as_bytes()))
    }

    /// Peek at a header row to detect this format.
    pub fn is_export_header(headers: &StringRecord) -> bool {
        headers
            .iter()
            .any(|header| header.trim().eq_ignore_ascii_case(EXPORT_CSV_MARKER))
    }
}

impl Default for ExportCsvParser {
    fn default() -> Self {
        Self::new()
    }
}

fn header_index(headers: &StringRecord) -> HashMap<String, usize> {
    headers
        .iter()
        .enumerate()
        .map(|(index, header)| (header.trim().to_ascii_lowercase(), index))
        .collect()
}

fn bookmark_from_record(
    record: &StringRecord,
    header_index: &HashMap<String, usize>,
) -> Result<Option<Bookmark>> {
    let get = |name: &str| -> &str {
        header_index
            .get(&name.to_ascii_lowercase())
            .and_then(|index| record.get(*index))
            .unwrap_or("")
            .trim()
    };

    let tweet_url = get("tweet_url");
    if tweet_url.is_empty() {
        return Ok(None);
    }
    let tweeted_at = parse_required_date(get("tweeted_at"))?;
    let imported_at = optional_date(get("imported_at")).unwrap_or_else(Utc::now);

    let mut bookmark = BookmarkBuilder::new()
        .tweet_url(tweet_url)
        .content(get("content"))
        .note_text(get("note_text"))
        .tweeted_at(tweeted_at)
        .author_handle(get("author_handle"))
        .author_name(get("author_name"))
        .build()
        .map_err(|error| Error::Other(error.to_string()))?;

    let id = get("id");
    if !id.is_empty() {
        bookmark.id = id.to_string();
    }
    bookmark.imported_at = imported_at;
    bookmark.comments = non_empty(get("comments"));
    bookmark.author_profile_url = non_empty(get("author_profile_url"));
    bookmark.author_profile_image = non_empty(get("author_profile_image"));
    bookmark.tags = parse_json_cell::<Vec<String>>(get("tags"))?.unwrap_or_default();
    bookmark.media = parse_json_cell::<Vec<Media>>(get("media"))?
        .unwrap_or_default()
        .into_iter()
        .take(MAX_MEDIA_PER_BOOKMARK)
        .collect();
    bookmark.quoted = parse_json_cell::<Option<QuotedTweet>>(get("quoted"))?.flatten();
    bookmark.links = crate::models::normalize_links(
        parse_json_cell::<Vec<LinkEntity>>(get("links"))?.unwrap_or_default(),
    );
    bookmark.is_read = parse_flag(get("is_read"));
    bookmark.is_favorite = parse_flag(get("is_favorite"));

    Ok(Some(bookmark))
}

fn parse_json_cell<T: serde::de::DeserializeOwned>(cell: &str) -> Result<Option<T>> {
    if cell.is_empty() || cell == "null" {
        return Ok(None);
    }
    serde_json::from_str(cell).map(Some).map_err(|error| {
        Error::InvalidFormat(format!("invalid JSON export cell '{cell}': {error}"))
    })
}

fn parse_required_date(value: &str) -> Result<DateTime<Utc>> {
    optional_date(value).ok_or_else(|| Error::InvalidFormat(format!("invalid date '{value}'")))
}

fn optional_date(value: &str) -> Option<DateTime<Utc>> {
    if value.trim().is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|date| date.with_timezone(&Utc))
        .or_else(|| {
            value
                .trim()
                .parse::<i64>()
                .ok()
                .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0))
        })
}

fn parse_flag(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    )
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_export_row() {
        let csv = "eterea_export_version,id,tweet_url,content,note_text,tweeted_at,imported_at,author_handle,author_name,author_profile_url,author_profile_image,tags,media,quoted,links,is_read,is_favorite,comments\n\
            1,id-1,https://x.com/export/status/1,hello,note,2026-05-01T10:00:00+00:00,2026-05-02T10:00:00+00:00,export,Export,,, [\"rust\"],\"[{\"\"url\"\":\"\"https://video.twimg.com/a/720.mp4\"\",\"\"media_type\"\":\"\"Video\"\"}]\",null,\"[{\"\"url\"\":\"\"https://t.co/a\"\",\"\"kind\"\":\"\"external\"\"}]\",1,1,comment\n";
        let bookmarks = ExportCsvParser::new().parse_str(csv).unwrap();
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks[0].id, "id-1");
        assert_eq!(bookmarks[0].tags, vec!["rust"]);
        assert_eq!(bookmarks[0].media.len(), 1);
        assert_eq!(bookmarks[0].links[0].url, "https://t.co/a");
        assert!(bookmarks[0].is_read);
        assert!(bookmarks[0].is_favorite);
        assert_eq!(bookmarks[0].comments.as_deref(), Some("comment"));
    }

    #[test]
    fn empty_json_cells_are_allowed() {
        let csv = "eterea_export_version,id,tweet_url,content,note_text,tweeted_at,imported_at,author_handle,author_name,author_profile_url,author_profile_image,tags,media,quoted,links,is_read,is_favorite,comments\n\
            1,,https://x.com/export/status/2,,,2026-05-01T10:00:00+00:00,,,,,,,,,,,,\n";
        let bookmarks = ExportCsvParser::new().parse_str(csv).unwrap();
        assert_eq!(bookmarks.len(), 1);
        assert!(!bookmarks[0].is_read);
        assert!(bookmarks[0].media.is_empty());
    }
}
