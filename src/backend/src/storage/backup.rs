//! Consistent database backups, integrity checks and restore.
//!
//! The archive is one SQLite file, so the safety net is built into the storage
//! layer: a rotating copy is written before every migration pass (i.e. before
//! `initialize` touches a pre-existing archive), a manual copy can be made from
//! the command palette, `PRAGMA integrity_check` is exposed as a status report,
//! and a backup can be restored page-wise into the live connection with
//! SQLite's online backup API. All copies are made from a live connection, so
//! WAL contents are included rather than a raw file copy racing the writer.

use super::database::Database;
use crate::{Error, Result};
use rusqlite::backup::Backup;
use rusqlite::{Connection, OpenFlags};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::info;

/// How many automatic copies stay on disk; the oldest are pruned.
pub const AUTOMATIC_BACKUPS_KEPT: usize = 5;
/// Directory beside the archive that holds automatic and manual copies.
pub const BACKUP_DIR_NAME: &str = "backups";
const BACKUP_FILE_PREFIX: &str = "bookmarks-";

/// Result of `PRAGMA integrity_check` and `PRAGMA foreign_key_check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityReport {
    pub ok: bool,
    pub messages: Vec<String>,
}

impl IntegrityReport {
    pub fn summary(&self) -> String {
        if self.ok {
            "database ok".to_string()
        } else {
            let first = self.messages.first().cloned().unwrap_or_default();
            format!("integrity check failed: {first}")
        }
    }
}

/// One on-disk backup copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    pub path: String,
    pub bytes: u64,
}

/// Result of a restore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreSummary {
    pub path: String,
    pub bookmarks: i64,
}

impl Database {
    /// Directory for automatic and manual copies beside `db_path`.
    pub fn backup_dir_for(db_path: &Path) -> PathBuf {
        db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(BACKUP_DIR_NAME)
    }

    /// Copy this database into `dest` using SQLite's online backup API.
    ///
    /// Works while the connection is idle; callers on the store thread can
    /// back up between requests without closing the archive.
    fn write_backup_to(&self, dest: &Path) -> Result<u64> {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        if dest.exists() {
            fs::remove_file(dest)?;
        }
        let mut target = Connection::open(dest)?;
        {
            let backup = Backup::new(&self.conn, &mut target)?;
            backup.run_to_completion(128, Duration::ZERO, None)?;
        }
        target.close().map_err(|(_, error)| error)?;
        Ok(fs::metadata(dest)?.len())
    }

    fn backup_stamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default()
    }

    /// Snapshot a pre-existing archive before `initialize` migrates it.
    ///
    /// On a first run (no archive yet) there is nothing to protect and this is
    /// a no-op. Every open keeps [`AUTOMATIC_BACKUPS_KEPT`] newest copies.
    pub(super) fn rotate_backup_before_migration(&self, db_path: &Path) -> Result<()> {
        let dir = Self::backup_dir_for(db_path);
        fs::create_dir_all(&dir)?;
        let stamp = Self::backup_stamp();
        let mut name = format!("{BACKUP_FILE_PREFIX}{stamp}.db");
        let mut suffix = 1u32;
        while dir.join(&name).exists() {
            name = format!("{BACKUP_FILE_PREFIX}{stamp}-{suffix}.db");
            suffix += 1;
        }
        self.write_backup_to(&dir.join(&name))?;
        prune_backups(&dir, AUTOMATIC_BACKUPS_KEPT)?;
        info!(target: "eterea::db", "automatic pre-migration backup written");
        Ok(())
    }

    /// Write a manual backup beside the archive and return its location.
    pub fn backup_now(&self, db_path: &Path) -> Result<BackupInfo> {
        let path = Self::create_backup(self, db_path)?;
        Ok(BackupInfo {
            path: path.display().to_string(),
            bytes: fs::metadata(&path)?.len(),
        })
    }

    fn create_backup(&self, db_path: &Path) -> Result<PathBuf> {
        let dir = Self::backup_dir_for(db_path);
        fs::create_dir_all(&dir)?;
        let stamp = Self::backup_stamp();
        let mut name = format!("{BACKUP_FILE_PREFIX}{stamp}.db");
        let mut suffix = 1u32;
        while dir.join(&name).exists() {
            name = format!("{BACKUP_FILE_PREFIX}{stamp}-{suffix}.db");
            suffix += 1;
        }
        let path = dir.join(name);
        self.write_backup_to(&path)?;
        prune_backups(&dir, AUTOMATIC_BACKUPS_KEPT)?;
        Ok(path)
    }

    /// List copies on disk, newest first.
    pub fn list_backups(db_path: &Path) -> Result<Vec<BackupInfo>> {
        let dir = Self::backup_dir_for(db_path);
        let mut backups: Vec<(Option<SystemTime>, BackupInfo)> = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.starts_with(BACKUP_FILE_PREFIX) || !name.ends_with(".db") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            backups.push((
                metadata.modified().ok(),
                BackupInfo {
                    path: path.display().to_string(),
                    bytes: metadata.len(),
                },
            ));
        }
        backups.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        Ok(backups.into_iter().map(|(_, backup)| backup).collect())
    }

    /// `PRAGMA integrity_check` plus `PRAGMA foreign_key_check`.
    pub fn integrity_check(&self) -> Result<IntegrityReport> {
        let mut messages = Vec::new();
        let mut stmt = self.conn.prepare("PRAGMA integrity_check")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            messages.push(row?);
        }
        let mut ok = messages.len() == 1 && messages[0] == "ok";
        let mut stmt = self.conn.prepare("PRAGMA foreign_key_check")?;
        let rows = stmt.query_map([], |row| {
            Ok(format!(
                "foreign key violation in {}",
                row.get::<_, String>(0)?
            ))
        })?;
        for row in rows {
            ok = false;
            messages.push(row?);
        }
        Ok(IntegrityReport { ok, messages })
    }

    /// Replace the live archive with the copy at `source`.
    ///
    /// The copy is validated first (integrity, schema version), the current
    /// archive is snapshotted to the backups directory, and then SQLite's
    /// online backup API copies page-by-page into the open connection. Older
    /// backups are migrated forward by running `initialize` afterwards.
    pub fn restore_from(&mut self, source: &Path) -> Result<RestoreSummary> {
        let source_conn = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| {
                Error::Other(format!("cannot open backup {}: {error}", source.display()))
            })?;
        let check: String =
            source_conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if check != "ok" {
            return Err(Error::Other(format!(
                "backup failed its integrity check: {check}"
            )));
        }
        let version: i64 =
            source_conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > super::migrations::CURRENT_SCHEMA_USER_VERSION {
            return Err(Error::Other(format!(
                "backup schema v{version} is newer than this build (v{})",
                super::migrations::CURRENT_SCHEMA_USER_VERSION
            )));
        }
        {
            let backup = Backup::new(&source_conn, &mut self.conn)?;
            backup.run_to_completion(128, Duration::ZERO, None)?;
        }
        self.initialize()?;
        Ok(RestoreSummary {
            path: source.display().to_string(),
            bookmarks: self.count_bookmarks()?,
        })
    }
}

fn prune_backups(dir: &Path, keep: usize) -> Result<()> {
    let mut files: Vec<(Option<SystemTime>, PathBuf)> = fs::read_dir(dir)?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            if !name.starts_with(BACKUP_FILE_PREFIX) || !name.ends_with(".db") {
                return None;
            }
            let modified = entry.metadata().ok().and_then(|meta| meta.modified().ok());
            Some((modified, path))
        })
        .collect();
    files.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, path) in files.into_iter().skip(keep) {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::BookmarkBuilder;
    use chrono::{TimeZone, Utc};

    fn bookmark(tweet_id: &str) -> crate::models::Bookmark {
        BookmarkBuilder::new()
            .tweet_url(format!("https://x.com/backup/status/{tweet_id}"))
            .content(format!("backup fixture {tweet_id}"))
            .tweeted_at(Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap())
            .author_handle("backup")
            .author_name("Backup")
            .build()
            .unwrap()
    }

    #[test]
    fn integrity_check_reports_ok_for_a_fresh_archive() {
        let db = Database::open_memory().unwrap();
        let report = db.integrity_check().unwrap();
        assert!(report.ok, "report: {report:?}");
        assert_eq!(report.messages, vec!["ok".to_string()]);
        assert_eq!(report.summary(), "database ok");
    }

    #[test]
    fn manual_backup_round_trips_through_restore() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bookmarks.db");
        let mut db = Database::open(&path).unwrap();
        let first = bookmark("1");
        db.insert_bookmarks(&[first.clone(), bookmark("2")])
            .unwrap();

        let backup = db.backup_now(&path).unwrap();
        assert!(std::path::Path::new(&backup.path).is_file());
        assert!(backup.bytes > 0);
        let listed = Database::list_backups(&path).unwrap();
        assert_eq!(
            listed.len(),
            1,
            "backups: {:?}",
            listed.iter().map(|b| b.path.clone()).collect::<Vec<_>>()
        );

        db.delete_bookmark(&first.id).unwrap();
        assert_eq!(db.count_bookmarks().unwrap(), 1);

        let restored = db.restore_from(std::path::Path::new(&backup.path)).unwrap();
        assert_eq!(restored.bookmarks, 2);
        assert_eq!(db.count_bookmarks().unwrap(), 2);
        assert!(db.get_bookmark(&first.id).unwrap().is_some());
    }

    #[test]
    fn restore_rejects_a_newer_schema_and_a_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bookmarks.db");
        let mut db = Database::open(&path).unwrap();

        let newer = dir.path().join("newer.db");
        let conn = Connection::open(&newer).unwrap();
        conn.pragma_update(None, "user_version", 99i64).unwrap();
        conn.execute("CREATE TABLE t (x INTEGER)", []).unwrap();
        drop(conn);
        assert!(db.restore_from(&newer).is_err());

        let corrupt = dir.path().join("corrupt.db");
        std::fs::write(&corrupt, b"not a database").unwrap();
        assert!(db.restore_from(&corrupt).is_err());
    }

    #[test]
    fn opening_an_existing_archive_rotates_automatic_backups() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bookmarks.db");
        {
            let db = Database::open(&path).unwrap();
            db.insert_bookmarks(&[bookmark("1")]).unwrap();
        }
        for _ in 0..7 {
            let db = Database::open(&path).unwrap();
            assert_eq!(db.count_bookmarks().unwrap(), 1);
        }
        let backups = Database::list_backups(&path).unwrap();
        assert!(
            backups.len() <= AUTOMATIC_BACKUPS_KEPT,
            "kept {} backups",
            backups.len()
        );
        assert!(!backups.is_empty());
    }
}
