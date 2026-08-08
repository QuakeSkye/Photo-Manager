use std::{fs, path::Path};

use rusqlite::Connection;
use serde::Serialize;

use crate::{
    db::{BackupKind, Database, LATEST_SCHEMA_VERSION, migration::map_sqlite_error},
    domain::{DomainError, ErrorCode},
    safety::{DatabaseMode, PhaseEightRunRoot},
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseSuiteReport {
    report_version: u32,
    run_id: uuid::Uuid,
    wal_enabled: bool,
    synchronous_full: bool,
    foreign_keys_enabled: bool,
    checkpoint_bounded: bool,
    checkpoint_completed: bool,
    backup_validated: bool,
    restore_to_new_file: bool,
    migrations_verified_from_versions: Vec<u32>,
    corrupt_database_read_only: bool,
    app_data_full_poisoned_writer: bool,
    later_write_rejected: bool,
    schema_checksum: String,
    migration_chain_checksum: String,
}

pub fn run(root: &PhaseEightRunRoot) -> Result<DatabaseSuiteReport, DomainError> {
    root.verify_current_identity()?;
    let suite = root.resolve_owned(Path::new("candidate-database-suite"))?;
    fs::create_dir(&suite).map_err(io_error)?;

    let main_directory = suite.join("main");
    let database = Database::open(&main_directory)?;
    let pragmas = database.pragmas()?;
    database.writer()?.transaction(|transaction| {
        transaction
            .execute(
                "INSERT INTO operation_event(operation_id,operation_kind,request_hash,first_received_at) VALUES(?1,'Phase8CandidateRestore',zeroblob(32),1)",
                [uuid::Uuid::new_v4().as_bytes().as_slice()],
            )
            .map_err(map_sqlite_error)?;
        Ok(())
    })?;
    let checkpoint_started = std::time::Instant::now();
    let checkpoint = database.controlled_checkpoint()?;
    let checkpoint_bounded = checkpoint_started.elapsed() < std::time::Duration::from_secs(2);
    let checkpoint_completed =
        !checkpoint.busy && checkpoint.checkpointed_frames == checkpoint.log_frames;

    let connection = database.open_read_connection()?;
    let snapshot = database.backup_service().create_consistent_snapshot(
        &connection,
        BackupKind::PreUpgrade,
        LATEST_SCHEMA_VERSION,
    )?;
    let validation = database.backup_service().validate_snapshot(&snapshot)?;
    let restored = database.backup_service().prepare_restore(&snapshot)?;
    let restored_connection = Connection::open(&restored).map_err(map_sqlite_error)?;
    let restored_count: i64 = restored_connection
        .query_row(
            "SELECT count(*) FROM operation_event WHERE operation_kind='Phase8CandidateRestore'",
            [],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    let restore_to_new_file = restored != database.path() && restored_count == 1;
    let schema_checksum = crate::db::schema_checksum(&connection)?;
    let backup_validated = validation.schema_version == LATEST_SCHEMA_VERSION
        && validation.schema_checksum == schema_checksum;
    drop(restored_connection);
    drop(connection);
    drop(database);

    let mut migrations_verified_from_versions = Vec::new();
    for version in 1..=LATEST_SCHEMA_VERSION {
        let directory = suite.join(format!("migration-from-{version}"));
        fs::create_dir(&directory).map_err(io_error)?;
        let path = directory.join("catalog.sqlite3");
        let mut connection = Connection::open(&path).map_err(map_sqlite_error)?;
        crate::db::connection::configure_connection(&connection)?;
        crate::db::migration::install_schema_through(&mut connection, version)?;
        drop(connection);
        let migrated = Database::open(&directory)?;
        if migrated.mode() != DatabaseMode::ReadWrite {
            return Err(DomainError::new(
                ErrorCode::ReadOnlySafeMode,
                "已发布 schema 前向迁移失败。",
            ));
        }
        let actual: u32 = migrated
            .open_read_connection()?
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(map_sqlite_error)?;
        if actual != LATEST_SCHEMA_VERSION {
            return Err(DomainError::new(
                ErrorCode::DatabaseCorrupt,
                "前向迁移版本错误。",
            ));
        }
        migrations_verified_from_versions.push(version);
    }

    let corrupt_directory = suite.join("corrupt");
    fs::create_dir(&corrupt_directory).map_err(io_error)?;
    create_new_bytes(
        &corrupt_directory.join("catalog.sqlite3"),
        b"generated corrupt sqlite fixture",
    )?;
    let corrupt = Database::open(&corrupt_directory)?;
    let corrupt_database_read_only =
        corrupt.mode() == DatabaseMode::ReadOnlySafe && corrupt.writer().is_err();

    let full_directory = suite.join("full");
    let full = Database::open(&full_directory)?;
    let writer = full.writer()?;
    let full_error = writer
        .execute(|connection, _| {
            let page_count: i64 = connection
                .pragma_query_value(None, "page_count", |row| row.get(0))
                .map_err(map_sqlite_error)?;
            connection
                .pragma_update(None, "max_page_count", page_count)
                .map_err(map_sqlite_error)?;
            connection
                .execute(
                    "INSERT INTO operation_event(operation_id,operation_kind,request_hash,first_received_at) VALUES(zeroblob(16),'Phase8Full',zeroblob(8388608),1)",
                    [],
                )
                .map(|_| ())
                .map_err(map_sqlite_error)
        })
        .expect_err("max_page_count must produce SQLITE_FULL");
    let app_data_full_poisoned_writer =
        full_error.code == ErrorCode::DatabaseDurability && !full.is_write_healthy();
    let later_write_rejected = writer
        .execute(|_, _| Ok(()))
        .is_err_and(|error| error.code == ErrorCode::ReadOnlySafeMode);

    let report = DatabaseSuiteReport {
        report_version: 1,
        run_id: root.run_id(),
        wal_enabled: pragmas.journal_mode_wal,
        synchronous_full: pragmas.synchronous_full,
        foreign_keys_enabled: pragmas.foreign_keys,
        checkpoint_bounded,
        checkpoint_completed,
        backup_validated,
        restore_to_new_file,
        migrations_verified_from_versions,
        corrupt_database_read_only,
        app_data_full_poisoned_writer,
        later_write_rejected,
        schema_checksum,
        migration_chain_checksum: crate::db::migration_chain_checksum(),
    };
    if !report.wal_enabled
        || !report.synchronous_full
        || !report.foreign_keys_enabled
        || !report.checkpoint_bounded
        || !report.checkpoint_completed
        || !report.backup_validated
        || !report.restore_to_new_file
        || report.migrations_verified_from_versions.len() != LATEST_SCHEMA_VERSION as usize
        || !report.corrupt_database_read_only
        || !report.app_data_full_poisoned_writer
        || !report.later_write_rejected
    {
        return Err(DomainError::new(
            ErrorCode::DatabaseDurability,
            "阶段 8 数据库套件未通过。",
        ));
    }
    root.verify_current_identity()?;
    root.ensure_allocation_within_limit(0)?;
    Ok(report)
}

fn create_new_bytes(path: &Path, bytes: &[u8]) -> Result<(), DomainError> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)
}

fn io_error(error: std::io::Error) -> DomainError {
    DomainError::new(
        ErrorCode::DatabaseDurability,
        "阶段 8 数据库生成夹具 I/O 失败。",
    )
    .with_diagnostic(error.to_string())
}
