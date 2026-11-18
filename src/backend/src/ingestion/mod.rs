//! Ingestion module for importing bookmarks from various formats
//!
//! Supports:
//! - Legacy CSV format (Dewey export)
//! - New CSV format (Twitter/X export)
//! - JSON format
//! - Archive JS format (`window.YTD... = [...]`)
//! - X API v2 JSON (`data` + `includes.tweets/users/media`)

mod csv_parser;
mod date_parse;
mod export_parser;
mod json_parser;
mod media_metadata;
mod quote_metadata;
mod url_entities;

pub use csv_parser::{CsvFormat, LegacyCsvParser, NewCsvParser};
pub use export_parser::ExportCsvParser;
pub use json_parser::JsonParser;

use crate::models::Bookmark;
use crate::storage::Database;
use crate::{Error, Result};
use std::path::Path;
use tracing::info;

/// Main ingestion engine that auto-detects format and imports bookmarks
#[derive(Default)]
pub struct Ingester;

impl Ingester {
    pub fn new() -> Self {
        Self
    }

    /// Ingest bookmarks from a file, auto-detecting the format
    pub fn ingest_file(&self, path: &Path, db: &Database) -> Result<usize> {
        let bookmarks = self.parse_file(path)?;
        self.insert_bookmarks(bookmarks, db)
    }

    /// Parse bookmarks from a file without inserting them.
    pub fn parse_file(&self, path: &Path) -> Result<Vec<Bookmark>> {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        match extension.as_str() {
            "zip" => self.parse_zip(path),
            "csv" => self.parse_csv(path),
            "json" | "js" => self.parse_json(path),
            _ => Err(Error::UnsupportedFileType(extension)),
        }
    }

    /// Walk an X archive `.zip` and parse every supported entry.
    ///
    /// The downloaded X archive can be imported directly: bookmark entries
    /// (`.js`, `.json`, `.csv`) are parsed and concatenated; other archive
    /// entries are ignored, and malformed entries are skipped as long as at
    /// least one entry parses.
    fn parse_zip(&self, path: &Path) -> Result<Vec<Bookmark>> {
        let file = std::fs::File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|error| Error::InvalidFormat(format!("not a zip archive: {error}")))?;
        let mut bookmarks = Vec::new();
        let mut parsed_entries = 0usize;
        let mut skipped = Vec::new();
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|error| Error::Other(format!("cannot read zip entry: {error}")))?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().to_string();
            let extension = std::path::Path::new(&name)
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.to_lowercase())
                .unwrap_or_default();
            if !matches!(extension.as_str(), "js" | "json" | "csv") {
                continue;
            }
            let mut content = String::new();
            std::io::Read::read_to_string(&mut entry, &mut content)?;
            match self.parse_content(&extension, &content) {
                Ok(mut parsed) => {
                    parsed_entries += 1;
                    bookmarks.append(&mut parsed);
                }
                Err(error) => skipped.push(format!("{name}: {error}")),
            }
        }
        if parsed_entries == 0 {
            return Err(Error::InvalidFormat(
                "zip has no .js, .json or .csv entries".to_string(),
            ));
        }
        if bookmarks.is_empty() {
            return Err(Error::InvalidFormat(format!(
                "zip contains no bookmarks ({parsed_entries} entry(s) parsed)"
            )));
        }
        if !skipped.is_empty() {
            info!(
                "Skipped {} zip entries: {}",
                skipped.len(),
                skipped.join("; ")
            );
        }
        info!(
            "Parsed {} bookmarks from {} zip entries",
            bookmarks.len(),
            parsed_entries
        );
        Ok(bookmarks)
    }

    /// Parse bookmarks from raw content using the file extension.
    pub fn parse_content(&self, extension: &str, content: &str) -> Result<Vec<Bookmark>> {
        match extension.to_lowercase().as_str() {
            "csv" => self.parse_csv_content(content),
            "json" | "js" => self.parse_json_content(content),
            other => Err(Error::UnsupportedFileType(other.to_string())),
        }
    }

    /// Parse from CSV, auto-detecting the format variant
    fn parse_csv(&self, path: &Path) -> Result<Vec<Bookmark>> {
        let format = CsvFormat::detect(path)?;
        info!("Detected CSV format: {:?}", format);

        match format {
            CsvFormat::Legacy => {
                let parser = LegacyCsvParser::new();
                parser.parse(path)
            }
            CsvFormat::New => {
                let parser = NewCsvParser::new();
                parser.parse(path)
            }
            CsvFormat::EtereaExport => {
                let parser = ExportCsvParser::new();
                let file = std::fs::File::open(path)?;
                parser.parse(std::io::BufReader::new(file))
            }
        }
    }

    /// Parse from JSON / archive-JS
    fn parse_json(&self, path: &Path) -> Result<Vec<Bookmark>> {
        let parser = JsonParser::new();
        parser.parse(path)
    }

    fn parse_csv_content(&self, content: &str) -> Result<Vec<Bookmark>> {
        match CsvFormat::detect_from_content(content)? {
            CsvFormat::Legacy => LegacyCsvParser::new().parse_str(content),
            CsvFormat::New => NewCsvParser::new().parse_str(content),
            CsvFormat::EtereaExport => ExportCsvParser::new().parse_str(content),
        }
    }

    fn parse_json_content(&self, content: &str) -> Result<Vec<Bookmark>> {
        JsonParser::new().parse_str(content)
    }

    /// Insert every parsed bookmark in one transaction.
    fn insert_bookmarks(&self, bookmarks: Vec<Bookmark>, db: &Database) -> Result<usize> {
        let total = bookmarks.len();
        info!("Inserting {} bookmarks", total);

        let inserted = db.insert_bookmarks(&bookmarks)?;
        info!("Successfully inserted {} bookmarks", inserted);
        Ok(inserted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const BOOKMARKS_JS: &str = r#"window.YTD.bookmark.part0 = [{"bookmark": {"tweetId":"999","createdAt":"2024-08-25T10:52:35.000Z","screen_name":"sveltejs","name":"Svelte","full_text":"Runes are here","expandedUrl":"https://x.com/sveltejs/status/999"}}];"#;

    fn archive_zip(entries: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("twitter-archive.zip");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        for (name, content) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        (dir, path)
    }

    #[test]
    fn zip_archives_are_parsed_entry_by_entry() {
        let (_dir, path) = archive_zip(&[
            ("data/bookmark.js", BOOKMARKS_JS),
            ("data/like.js", "window.YTD.like.part0 = [];"),
            ("README.txt", "not bookmarks"),
        ]);

        let bookmarks = Ingester::new().parse_file(&path).unwrap();
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks[0].author_handle, "sveltejs");
        assert_eq!(bookmarks[0].tweet_url, "https://x.com/sveltejs/status/999");
    }

    #[test]
    fn zip_without_bookmark_entries_is_an_error() {
        let (_dir, path) = archive_zip(&[("data/like.js", "window.YTD.like.part0 = [];")]);
        let error = Ingester::new().parse_file(&path).unwrap_err();
        assert!(matches!(error, Error::InvalidFormat(_)), "{error:?}");
    }

    #[test]
    fn zip_with_malformed_entries_skips_them_and_keeps_the_good_ones() {
        let (_dir, path) = archive_zip(&[
            ("data/broken.js", "window.YTD.tweets.part0 = K];[\n"),
            ("data/bookmark.js", BOOKMARKS_JS),
        ]);
        let bookmarks = Ingester::new().parse_file(&path).unwrap();
        assert_eq!(bookmarks.len(), 1);
    }
}
