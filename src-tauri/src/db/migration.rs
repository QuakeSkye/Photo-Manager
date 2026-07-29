use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, TransactionBehavior};
use sha2::{Digest, Sha256};

use crate::{
    domain::{DomainError, ErrorCode},
    safety::{FaultInjector, FaultPoint},
};

pub const LATEST_SCHEMA_VERSION: u32 = 2;
pub const MIGRATION_SQL: &str = include_str!("../../migrations/0001_phase_zero.sql");
pub const PHASE_ONE_MIGRATION_SQL: &str = include_str!("../../migrations/0002_phase_one.sql");

const MIGRATIONS: &[(u32, &str)] = &[(1, MIGRATION_SQL), (2, PHASE_ONE_MIGRATION_SQL)];

pub fn migration_checksum() -> String {
    migration_file_checksum(PHASE_ONE_MIGRATION_SQL)
}

fn migration_file_checksum(sql: &str) -> String {
    hex::encode(Sha256::digest(sql.as_bytes()))
}

pub fn current_schema_version(connection: &Connection) -> Result<u32, DomainError> {
    connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
        .map_err(map_sqlite_error)
}

pub fn apply_pending_migrations(
    connection: &mut Connection,
    build_hash: &str,
    fault_injector: &FaultInjector,
) -> Result<(), DomainError> {
    let current = current_schema_version(connection)?;
    if current > LATEST_SCHEMA_VERSION {
        return Err(DomainError::new(
            ErrorCode::ReadOnlySafeMode,
            "数据库来自更新版本，当前应用只能进入只读安全模式。",
        ));
    }
    if current == LATEST_SCHEMA_VERSION {
        verify_migration_history(connection)?;
        return Ok(());
    }

    let pending = MIGRATIONS
        .iter()
        .copied()
        .filter(|(version, _)| *version > current)
        .collect::<Vec<_>>();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(map_sqlite_error)?;
    for (version, sql) in pending {
        let checksum = migration_file_checksum(sql);
        transaction.execute_batch(sql).map_err(map_sqlite_error)?;
        transaction
            .execute(
                "INSERT INTO schema_migration(version, checksum, applied_at, app_build_hash)
                 VALUES (?1, ?2, ?3, ?4)",
                (i64::from(version), &checksum, now_micros(), build_hash),
            )
            .map_err(map_sqlite_error)?;
    }
    fault_injector.checkpoint(FaultPoint::SqliteBeforeCommit)?;
    transaction.commit().map_err(map_sqlite_error)?;
    fault_injector.checkpoint(FaultPoint::SqliteAfterCommit)?;

    verify_migration_history(connection)
}

pub fn verify_migration_history(connection: &Connection) -> Result<(), DomainError> {
    for (version, sql) in MIGRATIONS {
        let checksum = connection
            .query_row(
                "SELECT checksum FROM schema_migration WHERE version=?1",
                [i64::from(*version)],
                |row| row.get::<_, String>(0),
            )
            .map_err(map_sqlite_error)?;
        if checksum != migration_file_checksum(sql) {
            return Err(DomainError::new(
                ErrorCode::ReadOnlySafeMode,
                "数据库迁移记录与当前构建不一致，已关闭照片写入功能。",
            ));
        }
    }

    Ok(())
}

pub fn map_sqlite_error(error: rusqlite::Error) -> DomainError {
    let mut domain = DomainError::new(
        ErrorCode::DatabaseDurability,
        "应用数据库未能可靠保存更改，照片写入功能已关闭。",
    )
    .with_diagnostic(error.to_string());

    if let rusqlite::Error::SqliteFailure(failure, _) = &error {
        domain.code = match failure.code {
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
                ErrorCode::DatabaseBusy
            }
            rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase => {
                ErrorCode::DatabaseCorrupt
            }
            rusqlite::ErrorCode::DiskFull | rusqlite::ErrorCode::SystemIoFailure => {
                ErrorCode::DatabaseDurability
            }
            _ => ErrorCode::DatabaseDurability,
        };
    }

    domain
}

pub fn now_micros() -> i64 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}
