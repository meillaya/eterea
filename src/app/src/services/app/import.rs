use super::import_preview::{build_import_preview_with_duplicates, import_error_preview};
use super::AppServices;
use crate::types::{
    ImportBatchPreview, ImportBatchSummary, ImportFileResult, ImportPhase, ImportPreview,
    ImportProgress, ImportSummary,
};
use anyhow::{bail, Context, Result};
use eterea_core::{Bookmark, Ingester};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

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
        let duplicates = self.duplicate_count(&bookmarks)?;
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
        let duplicates = self.duplicate_count(&bookmarks)?;
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
        let mut parsed_any = false;
        for path in paths {
            match self.preview_import_file(path) {
                Ok(preview) => {
                    parsed_any = true;
                    total_parsed += preview.bookmark_count;
                    total_duplicates += preview.duplicate_count;
                    files.push(preview);
                }
                Err(error) => {
                    let mut failed = import_error_preview(&source_label(path), file_format(path));
                    failed.error = Some(format!("{error:#}"));
                    files.push(failed);
                }
            }
        }
        if !parsed_any {
            if let Some(first) = files.first() {
                anyhow::bail!(
                    "{}",
                    first
                        .error
                        .clone()
                        .unwrap_or_else(|| "no importable files".to_string())
                );
            }
        }
        Ok(ImportBatchPreview {
            files,
            total_parsed,
            total_duplicates,
        })
    }

    /// Import several files in ONE transaction (one history event).
    pub fn import_files(&self, paths: &[PathBuf]) -> Result<ImportBatchSummary> {
        self.import_files_with_progress(paths, &AtomicBool::new(false), |_| {})
    }

    /// Import several files with cooperative cancellation and progress ticks.
    ///
    /// Parsing happens file by file (so cancelling stops before the write),
    /// files that fail to parse are reported individually while the rest of the
    /// batch continues, and the dry-run duplicate estimate is computed with the
    /// same rules the insert uses — including duplicates inside the batch.
    pub fn import_files_with_progress(
        &self,
        paths: &[PathBuf],
        cancel: &AtomicBool,
        progress: impl Fn(ImportProgress),
    ) -> Result<ImportBatchSummary> {
        if paths.is_empty() {
            bail!("choose at least one file to import");
        }
        let total_files = paths.len();
        let mut parsed_files: Vec<(String, String, Vec<Bookmark>)> = Vec::new();
        let mut failures: Vec<ImportFileResult> = Vec::new();
        let mut total_parsed = 0usize;

        for (index, path) in paths.iter().enumerate() {
            progress(ImportProgress {
                phase: ImportPhase::Reading,
                files_done: index,
                files_total: total_files,
                rows: total_parsed,
            });
            if cancel.load(Ordering::Relaxed) {
                return Ok(cancelled_summary(parsed_files, failures, total_parsed));
            }
            match self.parse_import_file(path) {
                Ok(bookmarks) => {
                    total_parsed += bookmarks.len();
                    parsed_files.push((source_label(path), file_format(path), bookmarks));
                }
                Err(error) => {
                    failures.push(ImportFileResult {
                        source_label: source_label(path),
                        format: file_format(path),
                        parsed: 0,
                        inserted: 0,
                        duplicates: 0,
                        error: Some(format!("{error:#}")),
                    });
                }
            }
            progress(ImportProgress {
                phase: ImportPhase::Reading,
                files_done: index + 1,
                files_total: total_files,
                rows: total_parsed,
            });
        }

        if parsed_files.is_empty() {
            return Ok(ImportBatchSummary {
                preview: ImportBatchPreview {
                    files: failures
                        .iter()
                        .map(|file| import_error_preview(&file.source_label, file.format.clone()))
                        .collect(),
                    total_parsed: 0,
                    total_duplicates: 0,
                },
                inserted: 0,
                duplicates: 0,
                files: failures,
                history_id: None,
                cancelled: false,
            });
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(cancelled_summary(parsed_files, failures, total_parsed));
        }

        // Duplicate estimate with the exact rules `INSERT OR IGNORE` applies:
        // conflicts on the existing archive or on an earlier row of this batch.
        let all_urls: Vec<String> = parsed_files
            .iter()
            .flat_map(|(_, _, bookmarks)| bookmarks.iter().map(|b| b.tweet_url.clone()))
            .collect();
        let all_ids: Vec<String> = parsed_files
            .iter()
            .flat_map(|(_, _, bookmarks)| bookmarks.iter().map(|b| b.id.clone()))
            .collect();
        let (existing_urls, existing_ids) = self
            .db
            .existing_bookmark_keys(&all_urls, &all_ids)
            .context("failed to count import duplicates")?;
        let mut seen_urls = HashSet::new();
        let mut seen_ids = HashSet::new();
        let mut previews = Vec::with_capacity(parsed_files.len());
        let mut files = Vec::with_capacity(parsed_files.len() + failures.len());
        let mut batch = Vec::with_capacity(parsed_files.len());
        let mut total_duplicates = 0usize;
        for (label, format, bookmarks) in &parsed_files {
            let mut file_duplicates = 0usize;
            for bookmark in bookmarks {
                let duplicate = existing_urls.contains(&bookmark.tweet_url)
                    || existing_ids.contains(&bookmark.id)
                    || seen_urls.contains(&bookmark.tweet_url)
                    || seen_ids.contains(&bookmark.id);
                if duplicate {
                    file_duplicates += 1;
                }
                seen_urls.insert(bookmark.tweet_url.clone());
                seen_ids.insert(bookmark.id.clone());
            }
            total_duplicates += file_duplicates;
            previews.push(build_import_preview_with_duplicates(
                Path::new(label),
                bookmarks,
                file_duplicates,
            ));
            files.push(ImportFileResult {
                source_label: label.clone(),
                format: format.clone(),
                parsed: bookmarks.len(),
                inserted: 0,
                duplicates: file_duplicates,
                error: None,
            });
            batch.push((label.clone(), bookmarks.clone()));
        }
        files.extend(failures);

        if cancel.load(Ordering::Relaxed) {
            return Ok(cancelled_summary(parsed_files, files, total_parsed));
        }
        progress(ImportProgress {
            phase: ImportPhase::Saving,
            files_done: total_files,
            files_total: total_files,
            rows: total_parsed,
        });
        let outcome = self
            .db
            .insert_bookmarks_batch(&batch)
            .context("failed to store batch import")?;
        let mut parsed_index = 0usize;
        for file in files.iter_mut() {
            if file.error.is_some() {
                continue;
            }
            if let Some(parsed) = outcome.files.get(parsed_index) {
                file.inserted = parsed.inserted;
                file.duplicates = parsed.duplicates;
            }
            parsed_index += 1;
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
            cancelled: false,
        })
    }

    fn parse_import_file(&self, path: &Path) -> Result<Vec<Bookmark>> {
        let ingester = Ingester::new();
        ingester
            .parse_file(path)
            .with_context(|| format!("failed to parse import file at {}", path.display()))
    }

    /// Dry-run duplicate count using the insert's conflict rules.
    fn duplicate_count(&self, bookmarks: &[Bookmark]) -> Result<usize> {
        let urls: Vec<String> = bookmarks.iter().map(|b| b.tweet_url.clone()).collect();
        let ids: Vec<String> = bookmarks.iter().map(|b| b.id.clone()).collect();
        let (existing_urls, existing_ids) = self
            .db
            .existing_bookmark_keys(&urls, &ids)
            .context("failed to count import duplicates")?;
        let mut seen_urls = HashSet::new();
        let mut seen_ids = HashSet::new();
        let mut duplicates = 0usize;
        for bookmark in bookmarks {
            if existing_urls.contains(&bookmark.tweet_url)
                || existing_ids.contains(&bookmark.id)
                || seen_urls.contains(&bookmark.tweet_url)
                || seen_ids.contains(&bookmark.id)
            {
                duplicates += 1;
            }
            seen_urls.insert(bookmark.tweet_url.clone());
            seen_ids.insert(bookmark.id.clone());
        }
        Ok(duplicates)
    }
}

fn cancelled_summary(
    parsed_files: Vec<(String, String, Vec<Bookmark>)>,
    failures: Vec<ImportFileResult>,
    total_parsed: usize,
) -> ImportBatchSummary {
    let mut previews = Vec::new();
    let mut files = failures;
    for (label, format, bookmarks) in &parsed_files {
        previews.push(build_import_preview_with_duplicates(
            Path::new(label),
            bookmarks,
            0,
        ));
        files.push(ImportFileResult {
            source_label: label.clone(),
            format: format.clone(),
            parsed: bookmarks.len(),
            inserted: 0,
            duplicates: 0,
            error: None,
        });
    }
    ImportBatchSummary {
        preview: ImportBatchPreview {
            files: previews,
            total_parsed,
            total_duplicates: 0,
        },
        inserted: 0,
        duplicates: 0,
        files,
        history_id: None,
        cancelled: true,
    }
}

pub(super) fn source_label(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("bookmark export")
        .to_string()
}

pub(super) fn file_format(path: &Path) -> String {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_ascii_uppercase();
    if extension == "ZIP" {
        "X ARCHIVE".to_string()
    } else {
        extension
    }
}
