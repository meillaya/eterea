//! Export service: scope/format/path validation around the streaming data-layer
//! exporter.

use super::AppServices;
use crate::types::{ExportRequest, ExportScope, ExportSummary};
use anyhow::{Context, Result};
use std::path::Path;

impl AppServices {
    /// Export a scope as CSV or JSON to a caller-owned path.
    ///
    /// The destination directory is created when missing; the data layer
    /// streams into a temp file and atomically renames it into place.
    pub fn export(&self, request: &ExportRequest) -> Result<ExportSummary> {
        if request.path.as_os_str().is_empty() {
            anyhow::bail!("choose an export destination");
        }
        if let Some(parent) = request.path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create export directory {}", parent.display())
                })?;
            }
        }

        match &request.scope {
            ExportScope::Ids(ids) if ids.is_empty() => {
                anyhow::bail!("nothing to export: the selection is empty")
            }
            _ => {}
        }

        self.db
            .export(request)
            .with_context(|| format!("failed to export bookmarks to {}", request.path.display()))
    }

    /// Export to `path` with the format implied by the extension (unknown
    /// extensions default to CSV only when the caller asks for CSV explicitly).
    pub fn export_with_format(
        &self,
        scope: ExportScope,
        format: crate::types::ExportFormat,
        path: impl AsRef<Path>,
    ) -> Result<ExportSummary> {
        self.export(&ExportRequest {
            scope,
            format,
            path: path.as_ref().to_path_buf(),
        })
    }
}
