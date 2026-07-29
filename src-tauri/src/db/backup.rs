use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, backup::Backup};

use crate::domain::{DomainError, ErrorCode};

use super::migration::{map_sqlite_error, now_micros};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupKind {
    Daily,
    PreUpgrade,
}

#[derive(Debug, Clone)]
pub struct BackupService {
    directory: PathBuf,
}

impl BackupService {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub fn create_consistent_snapshot(
        &self,
        source: &Connection,
        kind: BackupKind,
        schema_version: u32,
    ) -> Result<PathBuf, DomainError> {
        fs::create_dir_all(&self.directory).map_err(map_io_error)?;
        let epoch_day = now_micros() / 86_400_000_000;
        let filename = match kind {
            BackupKind::Daily => format!("daily-{epoch_day}.sqlite3"),
            BackupKind::PreUpgrade => {
                format!(
                    "pre-upgrade-v{schema_version}-{}-{}.sqlite3",
                    now_micros(),
                    uuid::Uuid::new_v4()
                )
            }
        };
        let target_path = self.directory.join(filename);

        if kind == BackupKind::Daily && target_path.exists() {
            self.apply_retention(kind)?;
            return Ok(target_path);
        }

        let mut target = Connection::open_with_flags(
            &target_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .map_err(map_sqlite_error)?;
        let backup = Backup::new(source, &mut target).map_err(map_sqlite_error)?;
        backup
            .run_to_completion(64, Duration::from_millis(10), None)
            .map_err(map_sqlite_error)?;
        drop(backup);
        target
            .execute_batch("PRAGMA quick_check; PRAGMA foreign_key_check;")
            .map_err(map_sqlite_error)?;
        drop(target);

        self.apply_retention(kind)?;
        Ok(target_path)
    }

    pub fn list(&self, kind: BackupKind) -> Result<Vec<PathBuf>, DomainError> {
        if !self.directory.exists() {
            return Ok(Vec::new());
        }
        let prefix = match kind {
            BackupKind::Daily => "daily-",
            BackupKind::PreUpgrade => "pre-upgrade-",
        };
        let mut entries = fs::read_dir(&self.directory)
            .map_err(map_io_error)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().and_then(|extension| extension.to_str()) == Some("sqlite3")
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(prefix))
            })
            .collect::<Vec<_>>();
        entries.sort();
        Ok(entries)
    }

    fn apply_retention(&self, kind: BackupKind) -> Result<(), DomainError> {
        let keep = match kind {
            BackupKind::Daily => 7,
            BackupKind::PreUpgrade => 3,
        };
        let entries = self.list(kind)?;
        let remove_count = entries.len().saturating_sub(keep);
        for expired in entries.into_iter().take(remove_count) {
            fs::remove_file(expired).map_err(map_io_error)?;
        }
        Ok(())
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

fn map_io_error(error: std::io::Error) -> DomainError {
    DomainError::new(
        ErrorCode::DatabaseDurability,
        "无法创建或维护应用数据库备份，照片写入功能已关闭。",
    )
    .with_diagnostic(error.to_string())
}
