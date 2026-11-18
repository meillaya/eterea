use super::AppServices;
use crate::types::{BookmarkSelection, BulkResult, TagColor, TagMutation};
use anyhow::{Context, Result};

impl AppServices {
    pub fn toggle_favorite(&self, id: &str) -> Result<bool> {
        self.db
            .toggle_favorite(id)
            .with_context(|| format!("failed to toggle favorite for bookmark {id}"))
    }

    pub fn delete_bookmark(&self, id: &str) -> Result<bool> {
        self.db
            .delete_bookmark(id)
            .with_context(|| format!("failed to delete bookmark {id}"))
    }

    /// Replace one bookmark's tags exactly.
    pub fn rename_tag(&self, from: &str, to: &str) -> Result<eterea_core::TagRenameReport> {
        Ok(self.db.rename_tag(from, to)?)
    }

    pub fn prune_tags(&self) -> Result<usize> {
        Ok(self.db.prune_unused_tags()?)
    }

    pub fn tag_suggestions(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<eterea_core::TagSuggestion>> {
        Ok(self.db.tag_suggestions(prefix, limit)?)
    }

    pub fn reset_tag_color(&self, tag: &str) -> Result<bool> {
        Ok(self.db.reset_tag_color(tag)?)
    }

    pub fn set_bookmark_note(&self, id: &str, note: Option<&str>) -> Result<bool> {
        self.db
            .set_note(id, note)
            .with_context(|| format!("failed to set the note for bookmark {id}"))
    }

    pub fn set_bookmark_tags(&self, id: &str, tags: &[String]) -> Result<BulkResult> {
        self.db
            .set_bookmark_tags(id, tags)
            .with_context(|| format!("failed to set tags for bookmark {id}"))
    }

    /// Apply one tag mutation to a selection (alias: [`Self::bulk_tag`]).
    pub fn apply_tag_mutation(
        &self,
        selection: &BookmarkSelection,
        mutation: &TagMutation,
    ) -> Result<BulkResult> {
        self.db
            .apply_tag_mutation(selection, mutation)
            .context("failed to edit bookmark tags")
    }

    /// Bulk tag edit; same as [`Self::apply_tag_mutation`].
    pub fn bulk_tag(
        &self,
        selection: &BookmarkSelection,
        mutation: &TagMutation,
    ) -> Result<BulkResult> {
        self.apply_tag_mutation(selection, mutation)
    }

    /// Delete every bookmark in the selection, journaling full snapshots.
    pub fn bulk_delete(&self, selection: &BookmarkSelection) -> Result<BulkResult> {
        self.db
            .bulk_delete(selection)
            .context("failed to delete bookmarks")
    }

    /// Mark every bookmark in the selection read or unread.
    pub fn bulk_set_read(&self, selection: &BookmarkSelection, read: bool) -> Result<BulkResult> {
        self.db
            .bulk_set_read(selection, read)
            .context("failed to update read state")
    }

    /// Mark one bookmark read or unread.
    pub fn set_read(&self, id: &str, read: bool) -> Result<BulkResult> {
        self.db
            .set_read(id, read)
            .with_context(|| format!("failed to update read state for bookmark {id}"))
    }

    /// Every reader-chosen tag color.
    pub fn tag_colors(&self) -> Result<Vec<TagColor>> {
        self.db.tag_colors().context("failed to load tag colors")
    }

    /// Validate and store one tag color (`#rrggbb`).
    pub fn set_tag_color(&self, tag: &str, color: &str) -> Result<TagColor> {
        self.db
            .set_tag_color(tag, color)
            .context("failed to store tag color")
    }

    pub fn get_tag_color(&self, tag: &str) -> Result<Option<TagColor>> {
        self.db
            .get_tag_color(tag)
            .context("failed to load tag color")
    }

    /// Remove a tag color.
    pub fn clear_tag_color(&self, tag: &str) -> Result<bool> {
        self.db
            .clear_tag_color(tag)
            .context("failed to clear tag color")
    }
}
