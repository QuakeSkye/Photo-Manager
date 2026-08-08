use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::{
    domain::{DomainError, ErrorCode},
    safety::{DatabaseMode, FaultInjector},
};

use super::{
    backup::{BackupKind, BackupService},
    migration::{
        LATEST_SCHEMA_VERSION, apply_pending_migrations, current_schema_version, map_sqlite_error,
    },
    writer::DbWriter,
};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionPragmas {
    pub foreign_keys: bool,
    pub journal_mode_wal: bool,
    pub synchronous_full: bool,
    pub busy_timeout_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointResult {
    pub busy: bool,
    pub log_frames: i64,
    pub checkpointed_frames: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseOpenOutcome {
    ReadWrite,
    ReadOnlySafe,
}

#[derive(Debug)]
pub struct Database {
    path: PathBuf,
    backup_service: BackupService,
    writer: Option<Arc<DbWriter>>,
    mode: DatabaseMode,
    safe_mode_reason: Option<DomainError>,
    write_health: Arc<AtomicBool>,
}

impl Database {
    pub fn open(app_data_directory: &Path) -> Result<Self, DomainError> {
        Self::open_with_injector(app_data_directory, FaultInjector::default())
    }

    pub fn open_with_injector(
        app_data_directory: &Path,
        fault_injector: FaultInjector,
    ) -> Result<Self, DomainError> {
        fs::create_dir_all(app_data_directory).map_err(|error| {
            DomainError::new(
                ErrorCode::DatabaseDurability,
                "无法创建应用数据目录，照片写入功能已关闭。",
            )
            .with_diagnostic(error.to_string())
        })?;
        let path = app_data_directory.join("catalog.sqlite3");
        let backup_service = BackupService::new(app_data_directory.join("Backups"));

        let open_result = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        );
        let mut connection = match open_result {
            Ok(connection) => connection,
            Err(error) => {
                let reason = map_sqlite_error(error);
                return Ok(Self {
                    path,
                    backup_service,
                    writer: None,
                    mode: DatabaseMode::ReadOnlySafe,
                    safe_mode_reason: Some(reason),
                    write_health: Arc::new(AtomicBool::new(false)),
                });
            }
        };

        if let Err(reason) =
            configure_connection(&connection).and_then(|()| integrity_check(&connection))
        {
            return Ok(Self {
                path,
                backup_service,
                writer: None,
                mode: DatabaseMode::ReadOnlySafe,
                safe_mode_reason: Some(reason),
                write_health: Arc::new(AtomicBool::new(false)),
            });
        }

        let current = current_schema_version(&connection)?;
        if current > 0 && current < LATEST_SCHEMA_VERSION {
            if let Err(reason) = backup_service.create_consistent_snapshot(
                &connection,
                BackupKind::PreUpgrade,
                current,
            ) {
                return Ok(Self {
                    path,
                    backup_service,
                    writer: None,
                    mode: DatabaseMode::ReadOnlySafe,
                    safe_mode_reason: Some(reason),
                    write_health: Arc::new(AtomicBool::new(false)),
                });
            }
        }

        if let Err(error) =
            apply_pending_migrations(&mut connection, env!("CARGO_PKG_VERSION"), &fault_injector)
        {
            return Ok(Self {
                path,
                backup_service,
                writer: None,
                mode: safe_mode_for_domain(&error),
                safe_mode_reason: Some(error),
                write_health: Arc::new(AtomicBool::new(false)),
            });
        }
        // The database was already physically checked before any migration. Migrations are
        // transactional; running the same full quick/FK scan again makes 100k-photo startup
        // scale with the catalog twice without adding a new failure boundary.
        if let Err(reason) = logical_integrity_check(&connection) {
            return Ok(Self {
                path,
                backup_service,
                writer: None,
                mode: DatabaseMode::ReadOnlySafe,
                safe_mode_reason: Some(reason),
                write_health: Arc::new(AtomicBool::new(false)),
            });
        }
        if let Err(reason) = backup_service.create_consistent_snapshot(
            &connection,
            BackupKind::Daily,
            LATEST_SCHEMA_VERSION,
        ) {
            return Ok(Self {
                path,
                backup_service,
                writer: None,
                mode: DatabaseMode::ReadOnlySafe,
                safe_mode_reason: Some(reason),
                write_health: Arc::new(AtomicBool::new(false)),
            });
        }

        let writer = DbWriter::spawn(connection, path.clone(), fault_injector)?;
        let write_health = writer.health_handle();
        Ok(Self {
            path,
            backup_service,
            writer: Some(Arc::new(writer)),
            mode: DatabaseMode::ReadWrite,
            safe_mode_reason: None,
            write_health,
        })
    }

    pub const fn mode(&self) -> DatabaseMode {
        self.mode
    }

    pub fn outcome(&self) -> DatabaseOpenOutcome {
        match self.mode {
            DatabaseMode::ReadWrite => DatabaseOpenOutcome::ReadWrite,
            DatabaseMode::ReadOnlySafe | DatabaseMode::Unavailable => {
                DatabaseOpenOutcome::ReadOnlySafe
            }
        }
    }

    pub(crate) fn writer(&self) -> Result<Arc<DbWriter>, DomainError> {
        self.writer.clone().ok_or_else(|| {
            DomainError::new(
                ErrorCode::ReadOnlySafeMode,
                "应用数据库处于只读安全模式，照片写入功能已关闭。",
            )
        })
    }

    pub fn open_read_connection(&self) -> Result<Connection, DomainError> {
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(map_sqlite_error)?;
        configure_connection(&connection)?;
        Ok(connection)
    }

    pub fn pragmas(&self) -> Result<ConnectionPragmas, DomainError> {
        let connection = self.open_read_connection()?;
        read_pragmas(&connection)
    }

    pub fn controlled_checkpoint(&self) -> Result<CheckpointResult, DomainError> {
        self.writer()?.execute(|connection, _| {
            connection
                .query_row("PRAGMA wal_checkpoint(PASSIVE)", (), |row| {
                    Ok(CheckpointResult {
                        busy: row.get::<_, i64>(0)? != 0,
                        log_frames: row.get(1)?,
                        checkpointed_frames: row.get(2)?,
                    })
                })
                .map_err(map_sqlite_error)
        })
    }

    pub fn backup_service(&self) -> &BackupService {
        &self.backup_service
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn safe_mode_reason(&self) -> Option<&DomainError> {
        self.safe_mode_reason.as_ref()
    }

    pub fn write_health_handle(&self) -> Arc<AtomicBool> {
        self.write_health.clone()
    }

    pub fn is_write_healthy(&self) -> bool {
        self.write_health.load(Ordering::Acquire)
    }
}

pub fn configure_connection(connection: &Connection) -> Result<(), DomainError> {
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(map_sqlite_error)?;
    connection
        .execute_batch(
            "PRAGMA foreign_keys=ON;
             PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;",
        )
        .map_err(map_sqlite_error)?;
    let pragmas = read_pragmas(connection)?;
    if !pragmas.foreign_keys || !pragmas.journal_mode_wal || !pragmas.synchronous_full {
        return Err(DomainError::new(
            ErrorCode::DatabaseDurability,
            "应用数据库耐久配置校验失败，照片写入功能已关闭。",
        ));
    }
    Ok(())
}

pub fn read_pragmas(connection: &Connection) -> Result<ConnectionPragmas, DomainError> {
    let foreign_keys = connection
        .pragma_query_value(None, "foreign_keys", |row| row.get::<_, i64>(0))
        .map_err(map_sqlite_error)?
        == 1;
    let journal_mode = connection
        .pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
        .map_err(map_sqlite_error)?;
    let synchronous = connection
        .pragma_query_value(None, "synchronous", |row| row.get::<_, i64>(0))
        .map_err(map_sqlite_error)?;
    let busy_timeout_ms = connection
        .pragma_query_value(None, "busy_timeout", |row| row.get::<_, i64>(0))
        .map_err(map_sqlite_error)?;

    Ok(ConnectionPragmas {
        foreign_keys,
        journal_mode_wal: journal_mode.eq_ignore_ascii_case("wal"),
        synchronous_full: synchronous == 2,
        busy_timeout_ms,
    })
}

pub fn integrity_check(connection: &Connection) -> Result<(), DomainError> {
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
            "应用数据库完整性检查失败，已进入只读安全模式。",
        ));
    }
    Ok(())
}

fn logical_integrity_check(connection: &Connection) -> Result<(), DomainError> {
    let contradiction = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM volume_policy WHERE deny_write != 1
                UNION ALL
                SELECT 1
                FROM root AS root
                JOIN root_binding AS binding
                  ON binding.binding_id=root.current_binding_id
                WHERE binding.root_id != root.root_id
                UNION ALL
                SELECT 1
                FROM task_attempt AS attempt
                JOIN task_intent AS task ON task.task_id=attempt.task_id
                WHERE attempt.attempt_kind != task.task_kind
                UNION ALL
                SELECT 1
                FROM import_attempt AS import_attempt
                JOIN task_attempt AS attempt
                  ON attempt.attempt_id=import_attempt.attempt_id
                WHERE attempt.attempt_kind != 'Import'
                   OR attempt.task_id != import_attempt.job_id
                UNION ALL
                SELECT 1
                FROM deletion_attempt AS deletion_attempt
                JOIN task_attempt AS attempt
                  ON attempt.attempt_id=deletion_attempt.attempt_id
                WHERE attempt.attempt_kind != 'Recycle'
                   OR attempt.task_id != deletion_attempt.batch_id
                UNION ALL
                SELECT 1
                FROM task_attempt AS attempt
                WHERE (
                    SELECT COUNT(*) FROM operation_report
                    WHERE task_attempt_id=attempt.attempt_id
                ) != 1
                UNION ALL
                SELECT 1
                FROM operation_reservation AS reservation
                JOIN task_attempt AS attempt
                  ON attempt.attempt_id=reservation.attempt_id
                JOIN file_write_lease AS lease ON lease.slot=1
                WHERE reservation.owner_kind='TaskAttempt'
                  AND reservation.released_at IS NULL
                  AND (
                      reservation.task_id != attempt.task_id
                      OR reservation.task_id != lease.task_id
                      OR reservation.lease_generation != lease.lease_generation
                  )
                LIMIT 1
             )",
            (),
            |row| row.get::<_, bool>(0),
        )
        .map_err(map_sqlite_error)?;

    if contradiction {
        return Err(DomainError::new(
            ErrorCode::ReadOnlySafeMode,
            "检测到未协调的 Attempt 或领域约束矛盾；阶段 0 保守进入只读安全模式。",
        ));
    }
    Ok(())
}

fn safe_mode_for_domain(_error: &DomainError) -> DatabaseMode {
    DatabaseMode::ReadOnlySafe
}
