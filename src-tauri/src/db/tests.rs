use std::{fs, path::PathBuf};

use rusqlite::{Connection, OptionalExtension};

use crate::{
    catalog::{AssetObservation, CatalogService, MediaKind},
    db::migration::{map_sqlite_error, migration_checksum, now_micros},
    domain::{AttemptId, LibraryId, OperationId, RootBindingId, RootId, TaskId, VolumeId},
    roots::VolumeIdentityKey,
    safety::{DatabaseMode, FaultInjector, FaultPoint, TestRoot},
};

use super::{
    AttemptCloser, BackupKind, Database, DatabaseOpenOutcome, LATEST_SCHEMA_VERSION,
    OperationEventStore, VolumePolicyStore,
};

struct TestDatabase {
    _parent: tempfile::TempDir,
    root: TestRoot,
    database: Database,
}

fn open_test_database() -> TestDatabase {
    let parent = tempfile::tempdir().unwrap();
    let root = TestRoot::create_in(parent.path()).unwrap();
    let database = Database::open(root.path()).unwrap();
    assert_eq!(
        database.outcome(),
        DatabaseOpenOutcome::ReadWrite,
        "{:?}",
        database.safe_mode_reason()
    );
    TestDatabase {
        _parent: parent,
        root,
        database,
    }
}

#[derive(Clone, Copy)]
struct CatalogFixture {
    library_id: LibraryId,
    root_id: RootId,
    binding_id: RootBindingId,
}

fn insert_catalog_fixture(database: &Database) -> CatalogFixture {
    let library_id = LibraryId::new();
    let volume_id = VolumeId::new();
    let root_id = RootId::new();
    let binding_id = RootBindingId::new();
    database
        .writer()
        .unwrap()
        .transaction(move |transaction| {
            transaction
                .execute(
                    "INSERT INTO library(
                        library_id, library_kind, display_name, lifecycle, created_at
                     ) VALUES (?1, 'Fuji', 'Generated test library', 'Active', ?2)",
                    (library_id.as_bytes().as_slice(), now_micros()),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO current_library(slot, library_id) VALUES (1, ?1)",
                    [library_id.as_bytes().as_slice()],
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO volume_identity(
                        volume_id, filesystem_kind, strong_identity_key,
                        identity_format_version, identity_strength, created_at
                     ) VALUES (?1, 'NTFS', ?2, 1, 'Strong', ?3)",
                    (
                        volume_id.as_bytes().as_slice(),
                        format!("test-volume-{volume_id}").as_bytes(),
                        now_micros(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO root(
                        root_id, library_id, root_kind, capability_mask,
                        root_version, lifecycle, created_at
                     ) VALUES (?1, ?2, 'FujiLibrary', 127, 1, 'Online', ?3)",
                    (
                        root_id.as_bytes().as_slice(),
                        library_id.as_bytes().as_slice(),
                        now_micros(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO root_binding(
                        binding_id, root_id, binding_generation, volume_id,
                        absolute_path_display, absolute_path_key, root_directory_file_id,
                        filesystem_kind, evidence_level, reparse_checked,
                        binding_state, verified_at
                     ) VALUES (?1, ?2, 1, ?3, 'GENERATED_TEST_ROOT', ?4, ?5,
                               'NTFS', 'Strong', 1, 'Active', ?6)",
                    (
                        binding_id.as_bytes().as_slice(),
                        root_id.as_bytes().as_slice(),
                        volume_id.as_bytes().as_slice(),
                        b"generated-test-root".as_slice(),
                        b"generated-directory-id".as_slice(),
                        now_micros(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE root SET current_binding_id=?1 WHERE root_id=?2",
                    (
                        binding_id.as_bytes().as_slice(),
                        root_id.as_bytes().as_slice(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })
        .unwrap();

    CatalogFixture {
        library_id,
        root_id,
        binding_id,
    }
}

fn insert_cancel_before_start_fixture(
    database: &Database,
    fixture: CatalogFixture,
) -> (TaskId, AttemptId) {
    let task_id = TaskId::new();
    let attempt_id = AttemptId::new();
    let operation_id = OperationId::new();
    let report_id = uuid::Uuid::new_v4();
    database
        .writer()
        .unwrap()
        .transaction(move |transaction| {
            transaction
                .execute(
                    "INSERT INTO operation_event(
                        operation_id, operation_kind, request_hash, first_received_at
                     ) VALUES (?1, 'CancelImportBeforeStart', ?2, ?3)",
                    (
                        operation_id.as_bytes().as_slice(),
                        b"generated-cancel-request".as_slice(),
                        now_micros(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO task_intent(
                        task_id, task_kind, lifecycle, created_at
                     ) VALUES (?1, 'Import', 'Prepared', ?2)",
                    (task_id.as_bytes().as_slice(), now_micros()),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO import_job(
                        job_id, library_id, source_root_id, target_root_id,
                        target_binding_id, job_state, created_at
                     ) VALUES (?1, ?2, ?3, ?3, ?4, 'Prepared', ?5)",
                    (
                        task_id.as_bytes().as_slice(),
                        fixture.library_id.as_bytes().as_slice(),
                        fixture.root_id.as_bytes().as_slice(),
                        fixture.binding_id.as_bytes().as_slice(),
                        now_micros(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO task_attempt(
                        attempt_id, task_id, attempt_kind, attempt_ordinal,
                        operation_id, created_at
                     ) VALUES (?1, ?2, 'Import', 1, ?3, ?4)",
                    (
                        attempt_id.as_bytes().as_slice(),
                        task_id.as_bytes().as_slice(),
                        operation_id.as_bytes().as_slice(),
                        now_micros(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO import_attempt(
                        attempt_id, job_id, attempt_reason, attempt_state
                     ) VALUES (?1, ?2, 'CancelBeforeStart', 'Prepared')",
                    (
                        attempt_id.as_bytes().as_slice(),
                        task_id.as_bytes().as_slice(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO operation_report(
                        report_id, report_kind, library_id, root_id,
                        task_attempt_id, report_state, started_at
                     ) VALUES (?1, 'Import', ?2, ?3, ?4, 'Open', ?5)",
                    (
                        report_id.as_bytes().as_slice(),
                        fixture.library_id.as_bytes().as_slice(),
                        fixture.root_id.as_bytes().as_slice(),
                        attempt_id.as_bytes().as_slice(),
                        now_micros(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })
        .unwrap();
    (task_id, attempt_id)
}

#[test]
fn migration_opens_with_required_pragmas_and_checksum() {
    let test = open_test_database();
    let pragmas = test.database.pragmas().unwrap();

    assert!(pragmas.foreign_keys);
    assert!(pragmas.journal_mode_wal);
    assert!(pragmas.synchronous_full);
    assert_eq!(pragmas.busy_timeout_ms, 5_000);

    let connection = test.database.open_read_connection().unwrap();
    let user_version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let stored_checksum: String = connection
        .query_row(
            "SELECT checksum FROM schema_migration WHERE version=?1",
            [LATEST_SCHEMA_VERSION],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(user_version, LATEST_SCHEMA_VERSION);
    assert_eq!(stored_checksum, migration_checksum());
}

#[test]
fn foreign_keys_and_core_uniqueness_are_enforced() {
    let test = open_test_database();
    let fixture = insert_catalog_fixture(&test.database);
    let writer = test.database.writer().unwrap();

    let missing_library = LibraryId::new();
    assert!(
        writer
            .transaction(move |transaction| {
                transaction
                    .execute(
                        "INSERT INTO root(
                            root_id, library_id, root_kind, capability_mask,
                            root_version, lifecycle, created_at
                         ) VALUES (?1, ?2, 'FujiLibrary', 1, 1, 'Online', ?3)",
                        (
                            RootId::new().as_bytes().as_slice(),
                            missing_library.as_bytes().as_slice(),
                            now_micros(),
                        ),
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .is_err()
    );

    let task_one = uuid::Uuid::new_v4();
    let task_two = uuid::Uuid::new_v4();
    writer
        .transaction(move |transaction| {
            transaction
                .execute(
                    "INSERT INTO task_intent(
                        task_id, task_kind, lifecycle, created_at
                     ) VALUES (?1, 'Import', 'Prepared', ?2)",
                    (task_one.as_bytes().as_slice(), now_micros()),
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })
        .unwrap();
    assert!(
        writer
            .transaction(move |transaction| {
                transaction
                    .execute(
                        "INSERT INTO task_intent(
                            task_id, task_kind, lifecycle, created_at
                         ) VALUES (?1, 'Recycle', 'Prepared', ?2)",
                        (task_two.as_bytes().as_slice(), now_micros()),
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .is_err()
    );

    let second_binding = RootBindingId::new();
    assert!(
        writer
            .transaction(move |transaction| {
                transaction
                    .execute(
                        "INSERT INTO root_binding(
                            binding_id, root_id, binding_generation, volume_id,
                            absolute_path_display, absolute_path_key, root_directory_file_id,
                            filesystem_kind, evidence_level, reparse_checked,
                            binding_state, verified_at
                         )
                         SELECT ?1, root_id, 2, volume_id, 'SECOND', ?2, ?3,
                                'NTFS', 'Strong', 1, 'Active', ?4
                         FROM root_binding WHERE binding_id=?5",
                        (
                            second_binding.as_bytes().as_slice(),
                            b"second".as_slice(),
                            b"second-id".as_slice(),
                            now_micros(),
                            fixture.binding_id.as_bytes().as_slice(),
                        ),
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .is_err()
    );
}

#[test]
fn tombstones_attempt_identity_and_illegal_transitions_are_database_enforced() {
    let test = open_test_database();
    let fixture = insert_catalog_fixture(&test.database);
    let (task_id, attempt_id) = insert_cancel_before_start_fixture(&test.database, fixture);
    let writer = test.database.writer().unwrap();

    assert!(
        writer
            .transaction(move |transaction| {
                transaction
                    .execute(
                        "UPDATE task_attempt SET attempt_ordinal=2 WHERE attempt_id=?1",
                        [attempt_id.as_bytes().as_slice()],
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .is_err()
    );
    assert!(
        writer
            .transaction(move |transaction| {
                transaction
                    .execute(
                        "UPDATE import_attempt
                         SET attempt_state='Completed', finished_at=?1
                         WHERE attempt_id=?2",
                        (now_micros(), attempt_id.as_bytes().as_slice()),
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .is_err()
    );
    assert!(
        writer
            .transaction(move |transaction| {
                transaction
                    .execute(
                        "UPDATE task_intent
                         SET lifecycle='Completed', resolved_at=?1
                         WHERE task_id=?2",
                        (now_micros(), task_id.as_bytes().as_slice()),
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .is_err()
    );

    writer
        .transaction(move |transaction| {
            transaction
                .execute(
                    "UPDATE root
                     SET lifecycle='Tombstoned', tombstoned_at=?1, root_version=root_version+1
                     WHERE root_id=?2",
                    (now_micros(), fixture.root_id.as_bytes().as_slice()),
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })
        .unwrap();
    assert!(
        writer
            .transaction(move |transaction| {
                transaction
                    .execute(
                        "UPDATE root
                         SET lifecycle='Online', tombstoned_at=NULL, root_version=root_version+1
                         WHERE root_id=?1",
                        [fixture.root_id.as_bytes().as_slice()],
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .is_err()
    );
}

#[test]
fn task_attempt_kind_and_active_reservations_cannot_cross_owners() {
    let test = open_test_database();
    let fixture = insert_catalog_fixture(&test.database);
    let (task_id, attempt_id) = insert_cancel_before_start_fixture(&test.database, fixture);
    let writer = test.database.writer().unwrap();

    let wrong_attempt = AttemptId::new();
    let wrong_operation = OperationId::new();
    assert!(
        writer
            .transaction(move |transaction| {
                transaction
                    .execute(
                        "INSERT INTO operation_event(
                            operation_id, operation_kind, request_hash, first_received_at
                         ) VALUES (?1, 'WrongKind', ?2, ?3)",
                        (
                            wrong_operation.as_bytes().as_slice(),
                            b"wrong-kind".as_slice(),
                            now_micros(),
                        ),
                    )
                    .map_err(map_sqlite_error)?;
                transaction
                    .execute(
                        "INSERT INTO task_attempt(
                            attempt_id, task_id, attempt_kind, attempt_ordinal,
                            operation_id, created_at
                         ) VALUES (?1, ?2, 'Recycle', 2, ?3, ?4)",
                        (
                            wrong_attempt.as_bytes().as_slice(),
                            task_id.as_bytes().as_slice(),
                            wrong_operation.as_bytes().as_slice(),
                            now_micros(),
                        ),
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .is_err()
    );

    writer
        .transaction(move |transaction| {
            transaction
                .execute(
                    "UPDATE file_write_lease
                     SET task_id=?1, lease_generation=1, acquired_at=?2,
                         released_at=NULL, row_version=2
                     WHERE slot=1",
                    (task_id.as_bytes().as_slice(), now_micros()),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO operation_reservation(
                        reservation_id, owner_kind, task_id, attempt_id,
                        lease_generation, reservation_kind, root_id, root_binding_id,
                        path_key, created_at
                     ) VALUES (?1, 'TaskAttempt', ?2, ?3, 1, 'FinalPath', ?4, ?5, ?6, ?7)",
                    (
                        uuid::Uuid::new_v4().as_bytes().as_slice(),
                        task_id.as_bytes().as_slice(),
                        attempt_id.as_bytes().as_slice(),
                        fixture.root_id.as_bytes().as_slice(),
                        fixture.binding_id.as_bytes().as_slice(),
                        b"generated-reserved-path".as_slice(),
                        now_micros(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })
        .unwrap();

    assert!(
        writer
            .transaction(move |transaction| {
                transaction
                    .execute(
                        "INSERT INTO operation_reservation(
                            reservation_id, owner_kind, task_id, attempt_id,
                            lease_generation, reservation_kind, root_id, root_binding_id,
                            path_key, created_at
                         ) VALUES (?1, 'TaskAttempt', ?2, ?3, 1, 'FinalPath', ?4, ?5, ?6, ?7)",
                        (
                            uuid::Uuid::new_v4().as_bytes().as_slice(),
                            task_id.as_bytes().as_slice(),
                            attempt_id.as_bytes().as_slice(),
                            fixture.root_id.as_bytes().as_slice(),
                            fixture.binding_id.as_bytes().as_slice(),
                            b"generated-reserved-path".as_slice(),
                            now_micros(),
                        ),
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .is_err()
    );

    writer
        .transaction(move |transaction| {
            transaction
                .execute(
                    "UPDATE operation_reservation
                     SET released_at=?1, release_reason='GeneratedTestComplete'
                     WHERE path_key=?2",
                    (now_micros(), b"generated-reserved-path".as_slice()),
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })
        .unwrap();
    assert!(
        writer
            .transaction(move |transaction| {
                transaction
                    .execute(
                        "UPDATE operation_reservation
                         SET released_at=?1, release_reason='SecondRelease'
                         WHERE path_key=?2",
                        (now_micros(), b"generated-reserved-path".as_slice()),
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .is_err()
    );
}

#[test]
fn cancel_before_start_closes_report_attempt_job_and_task_atomically() {
    let test = open_test_database();
    let fixture = insert_catalog_fixture(&test.database);
    let (task_id, attempt_id) = insert_cancel_before_start_fixture(&test.database, fixture);
    let closer = AttemptCloser::new(test.database.writer().unwrap());

    let outcome = closer.cancel_import_before_start(attempt_id).unwrap();
    assert!(outcome.targeted_rescan_path_keys.is_empty());

    let connection = test.database.open_read_connection().unwrap();
    let states: (String, String, String, String) = connection
        .query_row(
            "SELECT report.report_state, attempt.attempt_state,
                    job.job_state, task.lifecycle
             FROM operation_report AS report
             JOIN import_attempt AS attempt
               ON attempt.attempt_id=report.task_attempt_id
             JOIN import_job AS job ON job.job_id=attempt.job_id
             JOIN task_intent AS task ON task.task_id=job.job_id
             WHERE task.task_id=?1",
            [task_id.as_bytes().as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        states,
        (
            "Finalized".into(),
            "Cancelled".into(),
            "Cancelled".into(),
            "Cancelled".into()
        )
    );
}

#[test]
fn atomic_attempt_close_commit_failure_exposes_no_half_closed_state() {
    let parent = tempfile::tempdir().unwrap();
    let root = TestRoot::create_in(parent.path()).unwrap();
    let injector = FaultInjector::default();
    let database = Database::open_with_injector(root.path(), injector.clone()).unwrap();
    let fixture = insert_catalog_fixture(&database);
    let (task_id, attempt_id) = insert_cancel_before_start_fixture(&database, fixture);
    injector
        .fail_on_hit(FaultPoint::SqliteBeforeCommit, 1)
        .unwrap();

    let closer = AttemptCloser::new(database.writer().unwrap());
    assert!(closer.cancel_import_before_start(attempt_id).is_err());

    let connection = database.open_read_connection().unwrap();
    let states: (String, String, String, String) = connection
        .query_row(
            "SELECT report.report_state, attempt.attempt_state,
                    job.job_state, task.lifecycle
             FROM operation_report AS report
             JOIN import_attempt AS attempt
               ON attempt.attempt_id=report.task_attempt_id
             JOIN import_job AS job ON job.job_id=attempt.job_id
             JOIN task_intent AS task ON task.task_id=job.job_id
             WHERE task.task_id=?1",
            [task_id.as_bytes().as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        states,
        (
            "Open".into(),
            "Prepared".into(),
            "Prepared".into(),
            "Prepared".into()
        )
    );
}

#[test]
fn volume_policy_is_monotonic_persistent_and_strong_key_deduplicated() {
    let test = open_test_database();
    let store = VolumePolicyStore::new(test.database.writer().unwrap());
    let first_id = VolumeId::new();
    let key = VolumeIdentityKey::parse("strong:v1:generated-source").unwrap();

    assert!(
        store
            .register_import_source_volume(first_id, key.clone())
            .unwrap()
    );
    assert!(
        !store
            .register_import_source_volume(VolumeId::new(), key)
            .unwrap()
    );

    let writer = test.database.writer().unwrap();
    assert!(
        writer
            .transaction(move |transaction| {
                transaction
                    .execute("UPDATE volume_policy SET deny_write=0", ())
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .is_err()
    );
    assert!(
        writer
            .transaction(move |transaction| {
                transaction
                    .execute("DELETE FROM volume_policy", ())
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .is_err()
    );

    let connection = test.database.open_read_connection().unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM volume_policy", (), |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn operation_id_is_idempotent_and_rejects_changed_request() {
    let test = open_test_database();
    let store = OperationEventStore::new(test.database.writer().unwrap());
    let operation_id = OperationId::new();

    assert!(
        store
            .record(operation_id, "Test".into(), b"same".to_vec())
            .unwrap()
    );
    assert!(
        !store
            .record(operation_id, "Test".into(), b"same".to_vec())
            .unwrap()
    );
    assert!(
        store
            .record(operation_id, "Test".into(), b"different".to_vec())
            .is_err()
    );
}

#[test]
fn commit_fault_rolls_back_and_after_commit_fault_preserves_fact_for_reconcile() {
    let parent = tempfile::tempdir().unwrap();
    let root = TestRoot::create_in(parent.path()).unwrap();
    let injector = FaultInjector::default();
    injector
        .fail_on_hit(FaultPoint::SqliteBeforeCommit, 2)
        .unwrap();
    let database = Database::open_with_injector(root.path(), injector).unwrap();
    assert_eq!(database.mode(), DatabaseMode::ReadWrite);
    let store = OperationEventStore::new(database.writer().unwrap());
    let rolled_back = OperationId::new();

    assert!(
        store
            .record(rolled_back, "CommitFault".into(), b"request".to_vec())
            .is_err()
    );
    let connection = database.open_read_connection().unwrap();
    let absent: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM operation_event WHERE operation_id=?1",
            [rolled_back.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    assert!(absent.is_none());
    drop(connection);
    drop(database);

    let parent = tempfile::tempdir().unwrap();
    let root = TestRoot::create_in(parent.path()).unwrap();
    let injector = FaultInjector::default();
    injector
        .fail_on_hit(FaultPoint::SqliteAfterCommit, 2)
        .unwrap();
    let database = Database::open_with_injector(root.path(), injector).unwrap();
    let store = OperationEventStore::new(database.writer().unwrap());
    let committed = OperationId::new();
    assert!(
        store
            .record(committed, "AfterCommitFault".into(), b"request".to_vec())
            .is_err()
    );
    let connection = database.open_read_connection().unwrap();
    let present: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM operation_event WHERE operation_id=?1",
            [committed.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(present, 1);
}

#[test]
fn migration_commit_fault_enters_safe_mode_without_partial_schema() {
    let parent = tempfile::tempdir().unwrap();
    let root = TestRoot::create_in(parent.path()).unwrap();
    let injector = FaultInjector::default();
    injector
        .fail_on_hit(FaultPoint::SqliteBeforeCommit, 1)
        .unwrap();

    let database = Database::open_with_injector(root.path(), injector).unwrap();
    assert_eq!(database.mode(), DatabaseMode::ReadOnlySafe);
    assert!(database.writer().is_err());

    let connection = Connection::open(database.path()).unwrap();
    let user_version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(user_version, 0);
}

#[test]
fn corrupted_database_enters_read_only_safe_mode() {
    let parent = tempfile::tempdir().unwrap();
    let root = TestRoot::create_in(parent.path()).unwrap();
    fs::write(
        root.path().join("catalog.sqlite3"),
        b"not a sqlite database",
    )
    .unwrap();

    let database = Database::open(root.path()).unwrap();
    assert_eq!(database.mode(), DatabaseMode::ReadOnlySafe);
    assert!(database.writer().is_err());
}

#[test]
fn newer_schema_and_unfinished_attempts_reopen_in_read_only_safe_mode() {
    let parent = tempfile::tempdir().unwrap();
    let root = TestRoot::create_in(parent.path()).unwrap();
    let database = Database::open(root.path()).unwrap();
    let fixture = insert_catalog_fixture(&database);
    insert_cancel_before_start_fixture(&database, fixture);
    drop(database);

    let reopened = Database::open(root.path()).unwrap();
    assert_eq!(reopened.mode(), DatabaseMode::ReadOnlySafe);
    assert!(
        reopened
            .safe_mode_reason()
            .unwrap()
            .user_message
            .contains("Attempt")
    );
    drop(reopened);

    let second_parent = tempfile::tempdir().unwrap();
    let second_root = TestRoot::create_in(second_parent.path()).unwrap();
    let connection = Connection::open(second_root.path().join("catalog.sqlite3")).unwrap();
    connection.pragma_update(None, "user_version", 2).unwrap();
    drop(connection);

    let newer = Database::open(second_root.path()).unwrap();
    assert_eq!(newer.mode(), DatabaseMode::ReadOnlySafe);
    assert!(newer.writer().is_err());
}

#[test]
fn schema_installs_cross_entity_chain_and_immutability_guards() {
    let test = open_test_database();
    let connection = test.database.open_read_connection().unwrap();
    let required = [
        "task_attempt_kind_matches_parent",
        "operation_reservation_task_owner_consistent",
        "operation_reservation_release_only",
        "duplicate_path_proof_chain_consistent",
        "import_partial_claim_chain_consistent",
        "verified_partial_chain_consistent",
        "import_publish_chain_consistent",
        "deletion_batch_asset_snapshot_copy",
        "operation_report_task_kind",
        "operation_resolution_event_chain_consistent",
    ];

    for trigger in required {
        let installed: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='trigger' AND name=?1",
                [trigger],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(installed, 1, "missing trigger {trigger}");
    }
}

#[test]
fn backup_uses_consistent_online_snapshot_and_enforces_retention() {
    let test = open_test_database();
    let connection = test.database.open_read_connection().unwrap();
    let backup = test.database.backup_service();

    let snapshot = backup
        .create_consistent_snapshot(&connection, BackupKind::PreUpgrade, 1)
        .unwrap();
    let snapshot_connection = Connection::open(snapshot).unwrap();
    let quick: String = snapshot_connection
        .pragma_query_value(None, "quick_check", |row| row.get(0))
        .unwrap();
    assert_eq!(quick, "ok");
    drop(snapshot_connection);

    for index in 0..10 {
        fs::write(
            backup.directory().join(format!("daily-{index:02}.sqlite3")),
            b"generated retention fixture",
        )
        .unwrap();
    }
    backup
        .create_consistent_snapshot(&connection, BackupKind::Daily, 1)
        .unwrap();
    assert_eq!(backup.list(BackupKind::Daily).unwrap().len(), 7);

    for _ in 0..5 {
        backup
            .create_consistent_snapshot(&connection, BackupKind::PreUpgrade, 1)
            .unwrap();
    }
    let upgrade_backups = backup.list(BackupKind::PreUpgrade).unwrap();
    assert_eq!(upgrade_backups.len(), 3, "{upgrade_backups:?}");
}

#[test]
fn catalog_service_is_idempotent_and_propagates_versions_with_cas() {
    let test = open_test_database();
    let fixture = insert_catalog_fixture(&test.database);
    let database = std::sync::Arc::new(test.database);
    let catalog = CatalogService::new(database.clone());
    let base = AssetObservation {
        library_id: fixture.library_id,
        root_id: fixture.root_id,
        root_binding_id: fixture.binding_id,
        media_kind: MediaKind::Jpg,
        relative_path: "2026/20260729/GENERATED.JPG".into(),
        path_key: b"2026\\20260729\\generated.jpg".to_vec(),
        file_id: Some(b"generated-file-id".to_vec()),
        size_bytes: 4_096,
        mtime_filetime: 123_456,
        observed_at: now_micros(),
    };

    let created = catalog.observe_asset(base.clone()).unwrap();
    assert_eq!(created.asset_version, 1);
    assert_eq!(created.photo_version, 1);
    assert!(created.changed);

    let repeated = catalog.observe_asset(base.clone()).unwrap();
    assert_eq!(repeated, created.with_changed(false));

    let changed = catalog
        .observe_asset(AssetObservation {
            size_bytes: 4_097,
            observed_at: now_micros(),
            ..base
        })
        .unwrap();
    assert_eq!(changed.asset_id, created.asset_id);
    assert_eq!(changed.photo_id, created.photo_id);
    assert_eq!(changed.asset_version, 2);
    assert_eq!(changed.photo_version, 2);

    let connection = database.open_read_connection().unwrap();
    let revisions: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM asset_revision WHERE asset_id=?1",
            [created.asset_id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(revisions, 2);
    assert!(
        connection
            .execute(
                "UPDATE asset_revision SET size_bytes=1 WHERE asset_id=?1",
                [created.asset_id.as_bytes().as_slice()],
            )
            .is_err()
    );
}

trait WithChanged {
    fn with_changed(self, changed: bool) -> Self;
}

impl WithChanged for crate::catalog::ObserveAssetResult {
    fn with_changed(mut self, changed: bool) -> Self {
        self.changed = changed;
        self
    }
}

#[test]
fn migration_checksum_is_a_stable_sha256() {
    assert_eq!(migration_checksum().len(), 64);
    assert_eq!(migration_checksum(), migration_checksum());
}

#[test]
fn generated_test_root_contains_every_database_and_backup_write() {
    let test = open_test_database();
    let catalog_path = fs::canonicalize(test.database.path()).unwrap();
    let root_path = fs::canonicalize(test.root.path()).unwrap();
    assert!(catalog_path.starts_with(&root_path));
    for backup in test
        .database
        .backup_service()
        .list(BackupKind::Daily)
        .unwrap()
    {
        assert!(fs::canonicalize(backup).unwrap().starts_with(&root_path));
    }
}

#[allow(dead_code)]
fn _assert_path_is_generated(path: PathBuf) {
    TestRoot::verify(&path).unwrap();
}
