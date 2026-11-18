use anyhow::{bail, Context, Result};
use eterea_core::{BackupInfo, Database, IntegrityReport, RestoreSummary};
use std::path::{Path, PathBuf};

mod directory_detail;
mod discovery;
mod export;
mod history;
mod import;
mod import_preview;
mod mutations;
mod query;
mod saved_searches;

#[cfg(test)]
mod tests;

pub struct AppServices {
    db: Database,
    /// Path of the file-backed archive, when there is one; backups and restore
    /// need it. In-memory archives (tests/previews) leave this `None`.
    db_path: Option<PathBuf>,
}

impl AppServices {
    pub fn open_default() -> Result<Self> {
        let path = Database::default_path();
        let db = Database::open(&path).context("failed to open default eterea database")?;
        Ok(Self {
            db,
            db_path: Some(path),
        })
    }

    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::open(path)
            .with_context(|| format!("failed to open database at {}", path.display()))?;
        Ok(Self {
            db,
            db_path: Some(path.to_path_buf()),
        })
    }

    pub fn open_memory() -> Result<Self> {
        let db = Database::open_memory().context("failed to open in-memory database")?;
        Ok(Self { db, db_path: None })
    }

    /// Write a consistent manual copy of the archive beside the database.
    pub fn backup_now(&self) -> Result<BackupInfo> {
        let Some(path) = self.db_path.as_deref() else {
            bail!("backups need a file-backed archive");
        };
        Ok(self.db.backup_now(path)?)
    }

    /// Copies already on disk, newest first.
    pub fn backup_list(&self) -> Result<Vec<BackupInfo>> {
        let Some(path) = self.db_path.as_deref() else {
            bail!("backups need a file-backed archive");
        };
        Ok(Database::list_backups(path)?)
    }

    /// Run SQLite's integrity and foreign-key checks.
    pub fn integrity_check(&self) -> Result<IntegrityReport> {
        Ok(self.db.integrity_check()?)
    }

    /// Replace the live archive with a validated backup copy.
    pub fn restore_backup(&mut self, path: &Path) -> Result<RestoreSummary> {
        let summary = self.db.restore_from(path)?;
        Ok(summary)
    }

    /// Number of recoverable deleted posts.
    pub fn trash_count(&self) -> Result<i64> {
        Ok(self.db.trash_count()?)
    }

    /// Restore the newest deleted post from the trash.
    pub fn restore_deleted(&self) -> Result<Option<eterea_core::Bookmark>> {
        Ok(self.db.restore_latest_trash()?)
    }

    /// Drop every recoverable deletion.
    pub fn empty_trash(&self) -> Result<usize> {
        Ok(self.db.empty_trash()?)
    }
}
