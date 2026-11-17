use super::import_preview::build_import_preview_with_duplicates;
use super::AppServices;
use crate::types::{
    ImportBatchPreview, ImportBatchSummary, ImportFileResult, ImportPreview, ImportSummary,
};
use anyhow::{Context, Result};
use eterea_core::Ingester;
use std::path::{Path, PathBuf};

impl AppServices {
    pub fn import_file(&self, path: &Path) -> Result<usize> {
        let ingester = Ingester::new();
        let bookmarks = ingester
            .parse_file(path)
            .with_context(|| format!("failed to parse import file at {}", path.display()))?;
        self.db
            .insert_bookmarks(&bookmarks)
            .with_context(|| format!("failed to store imported bookmarks from {}", path.display()))
    }

    pub fn preview_import_file(&self, path: &Path) -> Result<ImportPreview> {
        let bookmarks = self.parse_import_file(path)?;
        let duplicates = self
            .db
            .count_existing_tweet_urls(
                &bookmarks
                    .iter()
                    .map(|b| b.tweet_url.clone())
                    .collect::<Vec<_>>(),
            )
            .context("failed to count import duplicates")?;
        Ok(build_import_preview_with_duplicates(
            path, &bookmarks, duplicates,
        ))
    }

    pub fn import_file_with_preview(&self, path: &Path) -> Result<ImportSummary> {
        let preview = self.preview_import_file(path)?;
        let imported_count = self.import_file(path)?;
        Ok(ImportSummary {
            preview,
            imported_count,
        })
    }

    pub fn import_content(&self, filename: &str, content: &str) -> Result<usize> {
        let extension = Path::new(filename)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let ingester = Ingester::new();
        let bookmarks = ingester
            .parse_content(extension, content)
            .with_context(|| format!("failed to parse imported content for {filename}"))?;
        self.db
            .insert_bookmarks(&bookmarks)
            .with_context(|| format!("failed to store imported bookmarks for {filename}"))
    }

    pub fn preview_import_content(&self, filename: &str, content: &str) -> Result<ImportPreview> {
        let path = Path::new(filename);
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let ingester = Ingester::new();
        let bookmarks = ingester
            .parse_content(extension, content)
            .with_context(|| format!("failed to preview imported content for {filename}"))?;
        let duplicates = self
            .db
            .count_existing_tweet_urls(
                &bookmarks
                    .iter()
                    .map(|b| b.tweet_url.clone())
                    .collect::<Vec<_>>(),
            )
            .context("failed to count import duplicates")?;
        Ok(build_import_preview_with_duplicates(
            path, &bookmarks, duplicates,
        ))
    }

    pub fn import_content_with_preview(
        &self,
        filename: &str,
        content: &str,
    ) -> Result<ImportSummary> {
        let preview = self.preview_import_content(filename, content)?;
        let imported_count = self.import_content(filename, content)?;
        Ok(ImportSummary {
            preview,
            imported_count,
        })
    }

    /// Dry-run preview for several files; parses everything, writes nothing.
    pub fn preview_import_files(&self, paths: &[PathBuf]) -> Result<ImportBatchPreview> {
        if paths.is_empty() {
            anyhow::bail!("choose at least one file to import");
        }
        let mut files = Vec::with_capacity(paths.len());
        let mut total_parsed = 0;
        let mut total_duplicates = 0;
        for path in paths {
            let preview = self.preview_import_file(path)?;
            total_parsed += preview.bookmark_count;
            total_duplicates += preview.duplicate_count;
            files.push(preview);
        }
        Ok(ImportBatchPreview {
            files,
            total_parsed,
            total_duplicates,
        })
    }

    /// Import several files in ONE transaction (one history event).
    pub fn import_files(&self, paths: &[PathBuf]) -> Result<ImportBatchSummary> {
        if paths.is_empty() {
            anyhow::bail!("choose at least one file to import");
        }

        let mut files = Vec::with_capacity(paths.len());
        let mut batch = Vec::with_capacity(paths.len());
        let mut previews = Vec::with_capacity(paths.len());
        let mut total_parsed = 0;
        let mut total_duplicates = 0;

        for path in paths {
            let bookmarks = self.parse_import_file(path)?;
            let duplicate_count = self
                .db
                .count_existing_tweet_urls(
                    &bookmarks
                        .iter()
                        .map(|bookmark| bookmark.tweet_url.clone())
                        .collect::<Vec<_>>(),
                )
                .context("failed to count import duplicates")?;
            total_parsed += bookmarks.len();
            total_duplicates += duplicate_count;
            previews.push(build_import_preview_with_duplicates(
                path,
                &bookmarks,
                duplicate_count,
            ));
            let label = source_label(path);
            let format = file_format(path);
            files.push(ImportFileResult {
                source_label: label.clone(),
                format,
                parsed: bookmarks.len(),
                inserted: 0,
                duplicates: duplicate_count,
            });
            batch.push((label, bookmarks));
        }

        let outcome = self
            .db
            .insert_bookmarks_batch(&batch)
            .context("failed to store batch import")?;

        for (file, parsed_file) in files.iter_mut().zip(outcome.files.iter()) {
            file.inserted = parsed_file.inserted;
            file.duplicates = parsed_file.duplicates;
        }

        Ok(ImportBatchSummary {
            preview: ImportBatchPreview {
                files: previews,
                total_parsed,
                total_duplicates,
            },
            inserted: outcome.inserted,
            duplicates: outcome.duplicates,
            files,
            history_id: outcome.history_id,
        })
    }

    fn parse_import_file(&self, path: &Path) -> Result<Vec<eterea_core::Bookmark>> {
        Ingester::new()
            .parse_file(path)
            .with_context(|| format!("failed to parse import file at {}", path.display()))
    }
}

fn source_label(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("bookmark export")
        .to_string()
}

fn file_format(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_ascii_uppercase()
}
