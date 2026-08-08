use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, backup::Backup};

use crate::domain::{DomainError, ErrorCode};

use super::migration::{
    current_schema_version, map_sqlite_error, now_micros, schema_checksum,
    verify_migration_history_through,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupKind {
    Daily,
    PreUpgrade,
}

#[derive(Debug, Clone)]
pub struct BackupService {
    directory: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupValidation {
    pub schema_version: u32,
    pub schema_checksum: String,
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
        validate_snapshot_connection(&target)?;
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

    pub fn validate_snapshot(&self, snapshot: &Path) -> Result<BackupValidation, DomainError> {
        self.require_owned_snapshot(snapshot)?;
        let connection = Connection::open_with_flags(
            snapshot,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(map_sqlite_error)?;
        validate_snapshot_connection(&connection)
    }

    pub fn prepare_restore(&self, snapshot: &Path) -> Result<PathBuf, DomainError> {
        self.validate_snapshot(snapshot)?;
        let app_data = self.directory.parent().ok_or_else(|| {
            DomainError::new(
                ErrorCode::UnsafePath,
                "备份目录没有有效的应用数据父目录，无法准备恢复。",
            )
        })?;
        let target_path =
            app_data.join(format!("catalog.restore-{}.sqlite3", uuid::Uuid::new_v4()));
        if target_path.exists() {
            return Err(DomainError::new(
                ErrorCode::UnsafePath,
                "恢复目标已经存在，未覆盖任何文件。",
            ));
        }

        let source = Connection::open_with_flags(
            snapshot,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(map_sqlite_error)?;
        drop(
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target_path)
                .map_err(map_io_error)?,
        );
        let mut target =
            Connection::open_with_flags(&target_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
                .map_err(map_sqlite_error)?;
        let backup = Backup::new(&source, &mut target).map_err(map_sqlite_error)?;
        backup
            .run_to_completion(64, Duration::from_millis(10), None)
            .map_err(map_sqlite_error)?;
        drop(backup);
        validate_snapshot_connection(&target)?;
        drop(target);

        Ok(target_path)
    }

    fn require_owned_snapshot(&self, snapshot: &Path) -> Result<(), DomainError> {
        let canonical_directory = fs::canonicalize(&self.directory).map_err(map_io_error)?;
        let canonical_snapshot = fs::canonicalize(snapshot).map_err(map_io_error)?;
        if canonical_snapshot.parent() != Some(canonical_directory.as_path())
            || !self
                .list(BackupKind::Daily)?
                .into_iter()
                .chain(self.list(BackupKind::PreUpgrade)?)
                .any(|candidate| {
                    fs::canonicalize(candidate)
                        .is_ok_and(|candidate| candidate == canonical_snapshot)
                })
        {
            return Err(DomainError::new(
                ErrorCode::UnsafePath,
                "只能恢复当前应用 Backup 目录中已登记命名的快照。",
            ));
        }
        Ok(())
    }
}

fn validate_snapshot_connection(connection: &Connection) -> Result<BackupValidation, DomainError> {
    let quick_check: String = connection
        .pragma_query_value(None, "quick_check", |row| row.get(0))
        .map_err(map_sqlite_error)?;
    let foreign_key_violation = connection
        .query_row("PRAGMA foreign_key_check", (), |_| Ok(true))
        .optional()
        .map_err(map_sqlite_error)?
        .unwrap_or(false);
    if quick_check != "ok" || foreign_key_violation {
        return Err(DomainError::new(
            ErrorCode::DatabaseCorrupt,
            "数据库备份完整性失败，未准备恢复。",
        ));
    }
    let schema_version = current_schema_version(connection)?;
    verify_migration_history_through(connection, schema_version)?;
    Ok(BackupValidation {
        schema_version,
        schema_checksum: schema_checksum(connection)?,
    })
}

fn map_io_error(error: std::io::Error) -> DomainError {
    DomainError::new(
        ErrorCode::DatabaseDurability,
        "无法创建或维护应用数据库备份，照片写入功能已关闭。",
    )
    .with_diagnostic(error.to_string())
}
