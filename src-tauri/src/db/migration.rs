use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    domain::{DomainError, ErrorCode},
    safety::{FaultInjector, FaultPoint},
};

pub const LATEST_SCHEMA_VERSION: u32 = 11;
pub const MIGRATION_SQL: &str = include_str!("../../migrations/0001_phase_zero.sql");
pub const PHASE_ONE_MIGRATION_SQL: &str = include_str!("../../migrations/0002_phase_one.sql");
pub const PHASE_THREE_MIGRATION_SQL: &str = include_str!("../../migrations/0003_phase_three.sql");
pub const PHASE_FOUR_MIGRATION_SQL: &str = include_str!("../../migrations/0004_phase_four.sql");
pub const PHASE_FIVE_MIGRATION_SQL: &str = include_str!("../../migrations/0005_phase_five.sql");
pub const PHASE_SIX_MIGRATION_SQL: &str = include_str!("../../migrations/0006_phase_six.sql");
pub const PHASE_SEVEN_MIGRATION_SQL: &str = include_str!("../../migrations/0007_phase_seven.sql");
pub const PHASE_EIGHT_MIGRATION_SQL: &str = include_str!("../../migrations/0008_phase_eight.sql");
pub const PHOTO_CAPTURE_METADATA_MIGRATION_SQL: &str =
    include_str!("../../migrations/0009_photo_capture_metadata.sql");
pub const IMPORT_DEDUP_DUPLICATE_REVIEW_MIGRATION_SQL: &str =
    include_str!("../../migrations/0010_import_dedup_duplicate_review.sql");
pub const DUPLICATE_RESULT_RECONCILIATION_MIGRATION_SQL: &str =
    include_str!("../../migrations/0011_duplicate_result_reconciliation.sql");

const MIGRATIONS: &[(u32, &str, &str)] = &[
    (1, "0001_phase_zero.sql", MIGRATION_SQL),
    (2, "0002_phase_one.sql", PHASE_ONE_MIGRATION_SQL),
    (3, "0003_phase_three.sql", PHASE_THREE_MIGRATION_SQL),
    (4, "0004_phase_four.sql", PHASE_FOUR_MIGRATION_SQL),
    (5, "0005_phase_five.sql", PHASE_FIVE_MIGRATION_SQL),
    (6, "0006_phase_six.sql", PHASE_SIX_MIGRATION_SQL),
    (7, "0007_phase_seven.sql", PHASE_SEVEN_MIGRATION_SQL),
    (8, "0008_phase_eight.sql", PHASE_EIGHT_MIGRATION_SQL),
    (
        9,
        "0009_photo_capture_metadata.sql",
        PHOTO_CAPTURE_METADATA_MIGRATION_SQL,
    ),
    (
        10,
        "0010_import_dedup_duplicate_review.sql",
        IMPORT_DEDUP_DUPLICATE_REVIEW_MIGRATION_SQL,
    ),
    (
        11,
        "0011_duplicate_result_reconciliation.sql",
        DUPLICATE_RESULT_RECONCILIATION_MIGRATION_SQL,
    ),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationFingerprint {
    pub version: u32,
    pub filename: String,
    pub sha256: String,
}

pub fn migration_manifest() -> Vec<MigrationFingerprint> {
    MIGRATIONS
        .iter()
        .map(|(version, filename, sql)| MigrationFingerprint {
            version: *version,
            filename: (*filename).to_string(),
            sha256: migration_file_checksum(sql),
        })
        .collect()
}

pub fn migration_chain_checksum() -> String {
    let mut digest = Sha256::new();
    for migration in migration_manifest() {
        digest.update(migration.version.to_le_bytes());
        digest.update([0]);
        digest.update(migration.filename.as_bytes());
        digest.update([0]);
        digest.update(migration.sha256.as_bytes());
        digest.update(b"\n");
    }
    hex::encode(digest.finalize())
}

pub fn schema_checksum(connection: &Connection) -> Result<String, DomainError> {
    let version = current_schema_version(connection)?;
    let mut statement = connection
        .prepare(
            "SELECT type,name,tbl_name,sql FROM sqlite_schema
             WHERE sql IS NOT NULL
             ORDER BY type,name,tbl_name,sql",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(map_sqlite_error)?;
    let mut digest = Sha256::new();
    digest.update(b"local-photo-manager-schema-v1\n");
    digest.update(version.to_le_bytes());
    digest.update(b"\n");
    for row in rows {
        let (kind, name, table, sql) = row.map_err(map_sqlite_error)?;
        for value in [kind, name, table, sql] {
            digest.update(value.as_bytes());
            digest.update([0]);
        }
        digest.update(b"\n");
    }
    Ok(hex::encode(digest.finalize()))
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
            "数据库由更新版本的应用创建；当前程序版本过旧，请安装最新版本后继续。",
        ));
    }
    if current == LATEST_SCHEMA_VERSION {
        verify_migration_history(connection)?;
        return Ok(());
    }

    let pending = MIGRATIONS
        .iter()
        .copied()
        .filter(|(version, _, _)| *version > current)
        .collect::<Vec<_>>();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(map_sqlite_error)?;
    for (version, _filename, sql) in pending {
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
    verify_migration_history_through(connection, LATEST_SCHEMA_VERSION)
}

pub fn verify_migration_history_through(
    connection: &Connection,
    through_version: u32,
) -> Result<(), DomainError> {
    if through_version > LATEST_SCHEMA_VERSION {
        return Err(DomainError::new(
            ErrorCode::ReadOnlySafeMode,
            "数据库来自更新版本，无法验证迁移历史。",
        ));
    }
    for (version, _filename, sql) in MIGRATIONS
        .iter()
        .filter(|(version, _, _)| *version <= through_version)
    {
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

pub(crate) fn install_schema_through(
    connection: &mut Connection,
    target_version: u32,
) -> Result<(), DomainError> {
    if target_version == 0 || target_version > LATEST_SCHEMA_VERSION {
        return Err(DomainError::new(
            ErrorCode::InvalidTransition,
            "测试 schema 版本超出已发布迁移范围。",
        ));
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(map_sqlite_error)?;
    for (version, _filename, sql) in MIGRATIONS
        .iter()
        .copied()
        .filter(|(version, _, _)| *version <= target_version)
    {
        transaction.execute_batch(sql).map_err(map_sqlite_error)?;
        transaction
            .execute(
                "INSERT INTO schema_migration(version, checksum, applied_at, app_build_hash)
                 VALUES (?1, ?2, ?3, 'published-test-fixture')",
                (
                    i64::from(version),
                    migration_file_checksum(sql),
                    now_micros(),
                ),
            )
            .map_err(map_sqlite_error)?;
    }
    transaction.commit().map_err(map_sqlite_error)
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
            rusqlite::ErrorCode::ConstraintViolation => ErrorCode::InvalidTransition,
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
