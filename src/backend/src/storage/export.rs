//! Streaming bookmark export to CSV and JSON.
//!
//! Exports stream row-by-row into a caller-owned path, hydrate related data in
//! bounded chunks, then atomically rename a temp file into place. Only metadata
//! is exported; media bytes never enter the file. Both formats are
//! round-trippable through the importers in [`crate::ingestion`].

use super::database::Database;
use super::query::{
    BookmarkQuery, BookmarkSelection, ExportFormat, ExportRequest, ExportScope, ExportSummary,
};
use crate::models::Bookmark;
use crate::{Error, Result};
use rusqlite::params_from_iter;
use rusqlite::types::Value;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Stable identifier written into JSON exports and asserted by round-trip tests.
pub const EXPORT_JSON_FORMAT: &str = "eterea-bookmarks";
/// Bumped when the exported field set changes in a way importers must know.
pub const EXPORT_JSON_VERSION: u32 = 2;
/// First CSV header cell; exact app spelling `eterea`, used for detection.
pub const EXPORT_CSV_MARKER: &str = "eterea_export_version";
/// Current CSV field-set version.
pub const EXPORT_CSV_VERSION: u32 = 1;

/// CSV columns, in order. Complex fields are JSON cells so tags/media/quotes/
/// links round-trip through a plain CSV reader.
pub const EXPORT_CSV_HEADERS: &[&str] = &[
    EXPORT_CSV_MARKER,
    "id",
    "tweet_url",
    "content",
    "note_text",
    "tweeted_at",
    "imported_at",
    "author_handle",
    "author_name",
    "author_profile_url",
    "author_profile_image",
    "tags",
    "media",
    "quoted",
    "links",
    "is_read",
    "is_favorite",
    "comments",
];

/// Rows hydrated per chunk while streaming, bounding memory for 10k+ exports.
const EXPORT_CHUNK: usize = 500;

impl Database {
    /// Export a scope as CSV or JSON to `request.path`.
    pub fn export(&self, request: &ExportRequest) -> Result<ExportSummary> {
        let selection = match &request.scope {
            ExportScope::All => BookmarkSelection::Query(BookmarkQuery::default()),
            ExportScope::Query(query) => BookmarkSelection::Query(query.clone()),
            ExportScope::Ids(ids) => BookmarkSelection::Ids(ids.clone()),
        };
        let temp_path = temp_path_for(&request.path);
        let count = {
            let file = File::create(&temp_path).map_err(|error| {
                Error::Other(format!(
                    "failed to create export temp file {}: {error}",
                    temp_path.display()
                ))
            })?;
            let writer = BufWriter::new(file);
            match request.format {
                ExportFormat::Csv => self.write_csv(&selection, writer)?,
                ExportFormat::Json => self.write_json(&selection, writer)?,
            }
        };

        std::fs::rename(&temp_path, &request.path).map_err(|error| {
            let _ = std::fs::remove_file(&temp_path);
            Error::Other(format!(
                "failed to move export into {}: {error}",
                request.path.display()
            ))
        })?;

        let bytes = std::fs::metadata(&request.path)
            .map(|meta| meta.len())
            .unwrap_or(0);
        Ok(ExportSummary {
            path: request.path.clone(),
            format: request.format,
            count,
            bytes,
        })
    }

    /// Stream bookmarks into a CSV writer, returning the row count.
    fn write_csv<W: Write>(&self, selection: &BookmarkSelection, writer: W) -> Result<usize> {
        let mut csv = csv::WriterBuilder::new()
            .has_headers(false)
            .from_writer(writer);
        csv.write_record(EXPORT_CSV_HEADERS)?;

        let mut count = 0;
        self.for_each_bookmark_chunk(selection, |bookmark| {
            let tags = to_json_cell(&bookmark.tags)?;
            let media = to_json_cell(&bookmark.media)?;
            let quoted = to_json_cell(&bookmark.quoted)?;
            let links = to_json_cell(&bookmark.links)?;
            csv.write_record([
                EXPORT_CSV_VERSION.to_string(),
                bookmark.id.clone(),
                bookmark.tweet_url.clone(),
                bookmark.content.clone(),
                bookmark.note_text.clone().unwrap_or_default(),
                bookmark.tweeted_at.to_rfc3339(),
                bookmark.imported_at.to_rfc3339(),
                bookmark.author_handle.clone(),
                bookmark.author_name.clone(),
                bookmark.author_profile_url.clone().unwrap_or_default(),
                bookmark.author_profile_image.clone().unwrap_or_default(),
                tags,
                media,
                quoted,
                links,
                bool_cell(bookmark.is_read),
                bool_cell(bookmark.is_favorite),
                bookmark.comments.clone().unwrap_or_default(),
            ])?;
            count += 1;
            Ok(())
        })?;

        csv.flush()
            .map_err(|error| Error::Other(format!("failed to flush CSV export: {error}")))?;
        Ok(count)
    }

    /// Stream bookmarks into a JSON writer, returning the item count.
    fn write_json<W: Write>(&self, selection: &BookmarkSelection, mut writer: W) -> Result<usize> {
        writer
            .write_all(
                format!(
                    "{{\"format\":\"{EXPORT_JSON_FORMAT}\",\"version\":{EXPORT_JSON_VERSION},\"exported_at\":\"{}\",\"bookmarks\":[",
                    chrono::Utc::now().to_rfc3339()
                )
                .as_bytes(),
            )
            .map_err(io_error)?;

        let mut count = 0;
        self.for_each_bookmark_chunk(selection, |bookmark| {
            if count > 0 {
                writer.write_all(b",").map_err(io_error)?;
            }
            serde_json::to_writer(&mut writer, bookmark).map_err(|error| {
                Error::Other(format!(
                    "failed to serialize bookmark for JSON export: {error}"
                ))
            })?;
            count += 1;
            Ok(())
        })?;

        writer.write_all(b"]}").map_err(io_error)?;
        writer.flush().map_err(io_error)?;
        Ok(count)
    }

    /// Walk a selection in bounded chunks, hydrating each chunk once.
    fn for_each_bookmark_chunk<F>(&self, selection: &BookmarkSelection, mut visit: F) -> Result<()>
    where
        F: FnMut(&Bookmark) -> Result<()>,
    {
        let (where_clause, where_params) = self.selection_where(selection)?;
        if where_clause == "0" {
            return Ok(());
        }

        let mut sql = String::from(
            r#"SELECT b.id, b.tweet_url, b.content, b.note_text, b.tweeted_at, b.imported_at,
                      b.author_handle, b.author_name, b.author_profile_url, b.author_profile_image,
                      b.comments, b.is_favorite, b.is_read, b.links_json,
                      b.conversation_id,
                      b.unavailable
               FROM bookmarks b"#,
        );
        if !where_clause.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&where_clause);
        }
        sql.push_str(" ORDER BY b.tweeted_at DESC, b.id DESC LIMIT ? OFFSET ?");

        let mut offset = 0usize;
        loop {
            let mut params: Vec<Value> = where_params.clone();
            params.push(Value::Integer(EXPORT_CHUNK as i64));
            params.push(Value::Integer(offset as i64));

            let mut chunk: Vec<Bookmark> = {
                let mut stmt = self.conn.prepare(&sql)?;
                let rows = stmt.query_map(params_from_iter(params.iter()), |row| {
                    self.row_to_bookmark(row)
                })?;
                rows.collect::<std::result::Result<Vec<_>, _>>()?
            };
            if chunk.is_empty() {
                break;
            }
            self.hydrate_bookmarks(&mut chunk)?;
            for bookmark in &chunk {
                visit(bookmark)?;
            }
            offset += chunk.len();
            if chunk.len() < EXPORT_CHUNK {
                break;
            }
        }

        Ok(())
    }
}

fn to_json_cell<T: serde::Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|error| Error::Other(format!("failed to serialize export cell: {error}")))
}

fn bool_cell(value: bool) -> String {
    if value {
        "1".to_string()
    } else {
        "0".to_string()
    }
}

fn io_error(error: std::io::Error) -> Error {
    Error::Io(error)
}

/// Temp file next to the destination so the final rename is same-filesystem
/// and atomic.
fn temp_path_for(path: &Path) -> PathBuf {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("export");
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    parent.join(format!(".{file_name}.tmp-{}-{unique}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::BookmarkBuilder;
    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;

    fn bookmark(tweet_id: &str, content: &str, tag: &str) -> Bookmark {
        let mut bookmark = BookmarkBuilder::new()
            .tweet_url(format!("https://x.com/export/status/{tweet_id}"))
            .content(content)
            .tweeted_at(Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap())
            .author_handle("export")
            .author_name("Export")
            .add_tag(tag)
            .build()
            .unwrap();
        bookmark.is_read = true;
        bookmark
    }

    #[test]
    fn export_paths_are_atomic_and_temp_files_are_not_left_behind() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("archive.json");
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&[bookmark("1", "one row", "rust")])
            .unwrap();

        let summary = db
            .export(&ExportRequest {
                scope: ExportScope::All,
                format: ExportFormat::Json,
                path: path.clone(),
            })
            .unwrap();
        assert_eq!(summary.count, 1);
        assert!(summary.bytes > 0);
        assert!(path.exists());
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(leftovers, 0, "atomic temp file must be renamed away");
    }

    #[test]
    fn json_and_csv_exports_carry_the_round_trip_markers() {
        let dir = tempdir().unwrap();
        let db = Database::open_memory().unwrap();
        db.insert_bookmarks(&[
            bookmark("1", "héllo 世界", "rust"),
            bookmark("2", "second row", "design"),
        ])
        .unwrap();

        let json_path = dir.path().join("archive.json");
        let csv_path = dir.path().join("archive.csv");
        db.export(&ExportRequest {
            scope: ExportScope::All,
            format: ExportFormat::Json,
            path: json_path.clone(),
        })
        .unwrap();
        db.export(&ExportRequest {
            scope: ExportScope::All,
            format: ExportFormat::Csv,
            path: csv_path.clone(),
        })
        .unwrap();

        let json = std::fs::read_to_string(&json_path).unwrap();
        assert!(json.starts_with(&format!(
            "{{\"format\":\"{EXPORT_JSON_FORMAT}\",\"version\":{EXPORT_JSON_VERSION}"
        )));
        assert!(json.contains("héllo 世界"));
        let csv = std::fs::read_to_string(&csv_path).unwrap();
        assert!(csv.starts_with(EXPORT_CSV_MARKER));
        assert!(csv.contains("héllo 世界"));
    }
}
