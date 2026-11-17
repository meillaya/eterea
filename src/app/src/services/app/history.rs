//! Undo history service: list, detail, and newest-event undo.

use super::AppServices;
use crate::types::{HistoryDetail, HistoryListRequest, HistoryPage, PaginatedResponse, UndoResult};
use anyhow::{Context, Result};

impl AppServices {
    /// Paged history list, newest first.
    pub fn history_list(&self, request: &HistoryListRequest) -> Result<HistoryPage> {
        let (events, total) = self
            .db
            .history_list(request.offset, request.limit, request.include_undone)
            .context("failed to load history")?;
        Ok(PaginatedResponse::new(
            events,
            total,
            request.offset,
            request.limit,
        ))
    }

    /// One event with exactly the affected items and their before/after state.
    pub fn history_detail(&self, event_id: i64) -> Result<Option<HistoryDetail>> {
        self.db
            .history_detail(event_id)
            .with_context(|| format!("failed to load history event {event_id}"))
    }

    /// Undo the newest non-undone event. Returns affected/skipped counts.
    pub fn undo_latest(&self) -> Result<UndoResult> {
        self.db
            .undo_latest()
            .context("failed to undo the latest action")
    }
}
