use super::import::file_format;
use crate::types::{ImportPreview, ImportPreviewItem};
use eterea_core::Bookmark;
use std::path::Path;

pub(super) fn build_import_preview_with_duplicates(
    path: &Path,
    bookmarks: &[Bookmark],
    duplicate_count: usize,
) -> ImportPreview {
    let source_label = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("bookmark export")
        .to_string();
    let format = file_format(path);
    let sample = bookmarks
        .iter()
        .take(5)
        .map(|bookmark| ImportPreviewItem {
            author_handle: bookmark.author_handle.clone(),
            content: preview_content(&bookmark.content),
            tag_count: bookmark.tags.len(),
            has_media: !bookmark.media.is_empty(),
        })
        .collect();

    ImportPreview {
        source_label,
        format,
        bookmark_count: bookmarks.len(),
        duplicate_count,
        sample,
        duplicate_policy:
            "Existing tweet URLs and ids are skipped, including repeats inside the file; a failed parse or write leaves the archive unchanged."
                .to_string(),
        error: None,
    }
}

/// Preview placeholder for a file the batch could not parse.
pub(super) fn import_error_preview(source_label: &str, format: String) -> ImportPreview {
    ImportPreview {
        source_label: source_label.to_string(),
        format,
        bookmark_count: 0,
        duplicate_count: 0,
        sample: Vec::new(),
        duplicate_policy: "This file could not be parsed; nothing from it was imported."
            .to_string(),
        error: Some("could not parse this file".to_string()),
    }
}

fn preview_content(content: &str) -> String {
    const LIMIT: usize = 140;
    let mut preview: String = content.chars().take(LIMIT).collect();
    if content.chars().count() > LIMIT {
        preview.push('…');
    }
    preview
}
