use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

use sha2::{Digest, Sha256};

use crate::{
    catalog::{AssetObservation, CatalogService, MediaKind},
    db::Database,
    import_preflight::ImportPreflightService,
    roots::{FujiLibraryService, path_key, probe_directory},
    safety::{RuntimeSafetyGate, TestRoot},
};

use super::*;

fn ready_gate(database: &Arc<Database>) -> Arc<RuntimeSafetyGate> {
    let gate = Arc::new(RuntimeSafetyGate::new(database.clone()));
    gate.mark_ready().unwrap();
    gate
}

fn generated_jpeg(payload: &[u8]) -> Vec<u8> {
    let date = "2026:08:07 10:11:12";
    let mut jpeg = vec![0xff, 0xd8];
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II");
    tiff.extend_from_slice(&42_u16.to_le_bytes());
    tiff.extend_from_slice(&8_u32.to_le_bytes());
    tiff.extend_from_slice(&1_u16.to_le_bytes());
    tiff.extend_from_slice(&0x8769_u16.to_le_bytes());
    tiff.extend_from_slice(&4_u16.to_le_bytes());
    tiff.extend_from_slice(&1_u32.to_le_bytes());
    tiff.extend_from_slice(&26_u32.to_le_bytes());
    tiff.extend_from_slice(&0_u32.to_le_bytes());
    tiff.extend_from_slice(&1_u16.to_le_bytes());
    tiff.extend_from_slice(&0x9003_u16.to_le_bytes());
    tiff.extend_from_slice(&2_u16.to_le_bytes());
    tiff.extend_from_slice(&20_u32.to_le_bytes());
    tiff.extend_from_slice(&44_u32.to_le_bytes());
    tiff.extend_from_slice(&0_u32.to_le_bytes());
    tiff.extend_from_slice(date.as_bytes());
    tiff.push(0);
    let mut exif = b"Exif\0\0".to_vec();
    exif.extend(tiff);
    jpeg.extend_from_slice(&[0xff, 0xe1]);
    jpeg.extend_from_slice(&u16::try_from(exif.len() + 2).unwrap().to_be_bytes());
    jpeg.extend(exif);
    jpeg.extend_from_slice(payload);
    jpeg.extend_from_slice(&[0xff, 0xd9]);
    jpeg
}

fn generated_raf(jpeg: &[u8]) -> Vec<u8> {
    let mut raf = vec![0; 92];
    raf[..16].copy_from_slice(b"FUJIFILMCCD-RAW ");
    raf[84..88].copy_from_slice(&92_u32.to_be_bytes());
    raf[88..92].copy_from_slice(&u32::try_from(jpeg.len()).unwrap().to_be_bytes());
    raf.extend_from_slice(jpeg);
    raf
}

fn file_hash(path: &Path) -> [u8; 32] {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    Sha256::digest(bytes).into()
}

#[cfg(windows)]
struct ImportFixture {
    _app_data: TestRoot,
    target: TestRoot,
    _source_parent: tempfile::TempDir,
    source_file: PathBuf,
    source_before: ([u8; 32], std::time::SystemTime),
    database: Arc<Database>,
    preflight: Arc<ImportPreflightService>,
    catalog: Arc<CatalogService>,
    draft_digest: String,
    preexisting_target: Option<PathBuf>,
}

#[cfg(windows)]
#[derive(Clone, Copy)]
enum TargetFixture {
    Duplicate,
    Conflict,
}

#[cfg(windows)]
fn import_fixture_with_target(
    faults: FaultInjector,
    suffix: &str,
    target_fixture: Option<TargetFixture>,
) -> Option<ImportFixture> {
    let workspace_target = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    fs::create_dir_all(&workspace_target).unwrap();
    let app_data = TestRoot::create_in(&workspace_target).unwrap();
    let target = TestRoot::create_in(&workspace_target).unwrap();
    let source_parent = tempfile::tempdir().unwrap();
    let source = source_parent.path().join("DCIM").join("100_FUJI");
    fs::create_dir_all(&source).unwrap();
    let source_file = source.join(format!("DSCF{suffix}.JPG"));
    let source_bytes = generated_jpeg(suffix.as_bytes());
    fs::write(&source_file, &source_bytes).unwrap();
    let source_before = (
        file_hash(&source_file),
        fs::metadata(&source_file).unwrap().modified().unwrap(),
    );
    if probe_directory(&source).unwrap().volume_identity_key
        == probe_directory(target.path()).unwrap().volume_identity_key
    {
        return None;
    }
    let preexisting_target = target_fixture.map(|kind| {
        let path = target
            .path()
            .join("2026")
            .join("20260807")
            .join(format!("DSCF{suffix}.JPG"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            match kind {
                TargetFixture::Duplicate => source_bytes.as_slice(),
                TargetFixture::Conflict => b"preexisting formal file".as_slice(),
            },
        )
        .unwrap();
        path
    });
    let database = Arc::new(Database::open_with_injector(app_data.path(), faults).unwrap());
    FujiLibraryService::new(database.clone(), app_data.path())
        .bind(target.path())
        .unwrap();
    let catalog = Arc::new(CatalogService::new(database.clone()));
    if matches!(target_fixture, Some(TargetFixture::Duplicate)) {
        let target_file = preexisting_target.as_ref().unwrap();
        let connection = database.open_read_connection().unwrap();
        let (library, root, binding): (Vec<u8>, Vec<u8>, Vec<u8>) = connection
            .query_row(
                "SELECT library.library_id,root.root_id,root.current_binding_id FROM library JOIN root ON root.library_id=library.library_id WHERE library.lifecycle='Active' AND root.root_kind='FujiLibrary'",
                (),
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        drop(connection);
        let file = crate::roots::open_read_only(target_file).unwrap();
        let probe = crate::roots::probe_file(&file).unwrap();
        catalog
            .observe_asset(AssetObservation {
                library_id: library_id(&library).unwrap(),
                root_id: root_id(&root).unwrap(),
                root_binding_id: binding_id(&binding).unwrap(),
                media_kind: MediaKind::Jpg,
                relative_path: format!("2026\\20260807\\DSCF{suffix}.JPG"),
                path_key: path_key(Path::new(&format!("2026\\20260807\\DSCF{suffix}.JPG"))),
                file_id: probe.file_id,
                size_bytes: i64::try_from(probe.size_bytes).unwrap(),
                mtime_filetime: probe.mtime_filetime,
                observed_at: now_micros(),
            })
            .unwrap();
    }
    let preflight = Arc::new(ImportPreflightService::new(
        database.clone(),
        app_data.path(),
    ));
    preflight
        .select_source(&preflight.test_permit(), &source)
        .unwrap();
    let draft = preflight.run_preflight(&preflight.test_permit()).unwrap();
    Some(ImportFixture {
        _app_data: app_data,
        target,
        _source_parent: source_parent,
        source_file,
        source_before,
        database,
        preflight,
        catalog,
        draft_digest: draft.draft_digest,
        preexisting_target,
    })
}

#[cfg(windows)]
fn import_fixture(faults: FaultInjector, suffix: &str) -> Option<ImportFixture> {
    import_fixture_with_target(faults, suffix, None)
}

#[cfg(windows)]
fn assert_source_unchanged(fixture: &ImportFixture) {
    assert_eq!(
        (
            file_hash(&fixture.source_file),
            fs::metadata(&fixture.source_file)
                .unwrap()
                .modified()
                .unwrap()
        ),
        fixture.source_before
    );
}

#[cfg(windows)]
#[test]
fn background_import_returns_acceptance_before_execution_finishes() {
    let Some(fixture) = import_fixture(FaultInjector::default(), "6100") else {
        return;
    };
    let execution = Arc::new(ImportExecutionService::new(
        fixture.database.clone(),
        fixture.preflight.clone(),
        fixture.catalog.clone(),
        ready_gate(&fixture.database),
    ));

    let accepted = execution
        .start_background(execution.test_permit(), &fixture.draft_digest)
        .unwrap();
    assert_eq!(accepted.phase, ImportExecutionPhase::ValidatingPlan);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let completed = loop {
        let status = execution.status().unwrap();
        if matches!(
            status.phase,
            ImportExecutionPhase::Completed
                | ImportExecutionPhase::CompletedWithErrors
                | ImportExecutionPhase::Cancelled
                | ImportExecutionPhase::NeedsAttention
        ) {
            break status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "background import timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!(completed.phase, ImportExecutionPhase::Completed);
    assert_eq!(completed.published_files, 1);
    let metadata: (Option<String>, String) = fixture
        .database
        .open_read_connection()
        .unwrap()
        .query_row(
            "SELECT capture_local_key, preview_state
             FROM asset_media_metadata
             ORDER BY observed_at DESC LIMIT 1",
            (),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(metadata.0.as_deref(), Some("2026-08-07T10:11:12"));
    assert_eq!(metadata.1, "JpegSource");
    assert_source_unchanged(&fixture);
}

#[cfg(windows)]
#[test]
fn same_import_group_indexes_jpg_and_raf_as_one_photo() {
    let workspace_target = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    fs::create_dir_all(&workspace_target).unwrap();
    let app_data = TestRoot::create_in(&workspace_target).unwrap();
    let target = TestRoot::create_in(&workspace_target).unwrap();
    let source_parent = tempfile::tempdir().unwrap();
    let source = source_parent.path().join("DCIM").join("100_FUJI");
    let raw = source.join("raw");
    fs::create_dir_all(&raw).unwrap();
    let jpeg = generated_jpeg(b"same-import-group");
    let raf = generated_raf(&jpeg);
    let jpg_source = source.join("_DSF3545.JPG");
    let raf_source = raw.join("_DSF3545.RAF");
    fs::write(&jpg_source, &jpeg).unwrap();
    fs::write(&raf_source, &raf).unwrap();
    let source_before = (file_hash(&jpg_source), file_hash(&raf_source));

    if probe_directory(&source).unwrap().volume_identity_key
        == probe_directory(target.path()).unwrap().volume_identity_key
    {
        return;
    }

    let database = Arc::new(Database::open(app_data.path()).unwrap());
    let binding = FujiLibraryService::new(database.clone(), app_data.path())
        .bind(target.path())
        .unwrap();
    let catalog = Arc::new(CatalogService::new(database.clone()));
    let preflight = Arc::new(ImportPreflightService::new(
        database.clone(),
        app_data.path(),
    ));
    preflight
        .select_source(&preflight.test_permit(), &source)
        .unwrap();
    let draft = preflight.run_preflight(&preflight.test_permit()).unwrap();
    assert_eq!(draft.summary.new_files, 2);
    assert_eq!(draft.items[0].logical_group, "_DSF3545");
    assert_eq!(draft.items[1].logical_group, "_DSF3545");

    let execution = ImportExecutionService::new(
        database.clone(),
        preflight,
        catalog.clone(),
        ready_gate(&database),
    );
    let status = execution
        .start(execution.test_permit(), &draft.draft_digest)
        .unwrap();
    assert_eq!(status.phase, ImportExecutionPhase::Completed);

    let connection = database.open_read_connection().unwrap();
    let active_photos: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM photo WHERE library_id=?1 AND lifecycle='Active'",
            [binding.library_id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    let roles: (i64, i64, i64) = connection
        .query_row(
            "SELECT SUM(relation.role='Jpg'), SUM(relation.role='Raf'),
                    COUNT(DISTINCT relation.photo_id)
             FROM photo_asset AS relation
             JOIN photo ON photo.photo_id=relation.photo_id
             WHERE photo.library_id=?1 AND photo.lifecycle='Active'
               AND relation.to_photo_version IS NULL",
            [binding.library_id.as_bytes().as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let import_evidence: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pair_evidence WHERE evidence_kind='SameImportGroup'",
            (),
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active_photos, 1);
    assert_eq!(roles, (1, 1, 1));
    assert_eq!(import_evidence, 1);
    drop(connection);

    database
        .writer()
        .unwrap()
        .transaction(|transaction| {
            transaction
                .execute_batch("DROP TRIGGER asset_media_metadata_no_update")
                .unwrap();
            transaction
                .execute(
                    "UPDATE asset_media_metadata
                     SET capture_local_key=NULL, camera_model=NULL, camera_serial=NULL",
                    (),
                )
                .unwrap();
            transaction
                .execute(
                    "UPDATE import_group SET pair_evidence=NULL
                     WHERE pair_evidence='SameImportGroup'",
                    (),
                )
                .unwrap();
            Ok(())
        })
        .unwrap();
    catalog.reconcile_library(binding.library_id).unwrap();
    let split_photos: i64 = database
        .open_read_connection()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM photo WHERE library_id=?1 AND lifecycle='Active'",
            [binding.library_id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(split_photos, 2);

    database
        .writer()
        .unwrap()
        .transaction(|transaction| {
            transaction
                .execute(
                    "UPDATE import_group SET pair_evidence='SameImportGroup'
                     WHERE pair_evidence IS NULL",
                    (),
                )
                .unwrap();
            Ok(())
        })
        .unwrap();
    catalog.reconcile_library(binding.library_id).unwrap();
    let repaired_photos: i64 = database
        .open_read_connection()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM photo WHERE library_id=?1 AND lifecycle='Active'",
            [binding.library_id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(repaired_photos, 1);

    assert_eq!(file_hash(&jpg_source), source_before.0);
    assert_eq!(file_hash(&raf_source), source_before.1);
    assert_eq!(
        fs::read(
            target
                .path()
                .join("2026")
                .join("20260807")
                .join("_DSF3545.JPG")
        )
        .unwrap(),
        jpeg
    );
    assert_eq!(
        fs::read(
            target
                .path()
                .join("2026")
                .join("20260807")
                .join("raw")
                .join("_DSF3545.RAF")
        )
        .unwrap(),
        raf
    );
}

#[cfg(windows)]
#[test]
fn disposable_two_volume_import_publishes_without_source_or_existing_target_changes() {
    let workspace_target = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    fs::create_dir_all(&workspace_target).unwrap();
    let app_data = TestRoot::create_in(&workspace_target).unwrap();
    let target = TestRoot::create_in(&workspace_target).unwrap();
    let source_parent = tempfile::tempdir().unwrap();
    let source = source_parent.path().join("DCIM").join("100_FUJI");
    fs::create_dir_all(&source).unwrap();
    let bytes = generated_jpeg(b"phase-six-real-copy-fixture");
    let source_file = source.join("DSCF6001.JPG");
    fs::write(&source_file, &bytes).unwrap();
    let source_before = (
        file_hash(&source_file),
        fs::metadata(&source_file).unwrap().modified().unwrap(),
    );

    if probe_directory(&source).unwrap().volume_identity_key
        == probe_directory(target.path()).unwrap().volume_identity_key
    {
        return;
    }

    let database = Arc::new(Database::open(app_data.path()).unwrap());
    FujiLibraryService::new(database.clone(), app_data.path())
        .bind(target.path())
        .unwrap();
    let catalog = Arc::new(CatalogService::new(database.clone()));
    let preflight = Arc::new(ImportPreflightService::new(
        database.clone(),
        app_data.path(),
    ));
    preflight
        .select_source(&preflight.test_permit(), &source)
        .unwrap();
    let draft = preflight.run_preflight(&preflight.test_permit()).unwrap();
    assert_eq!(draft.summary.new_files, 1);
    let execution =
        ImportExecutionService::new(database.clone(), preflight, catalog, ready_gate(&database));
    let status = execution
        .start(execution.test_permit(), &draft.draft_digest)
        .unwrap();
    assert_eq!(status.phase, ImportExecutionPhase::Completed);
    assert_eq!(status.published_files, 1);
    assert_eq!(status.source_write_requests, 0);

    let published = target
        .path()
        .join("2026")
        .join("20260807")
        .join("DSCF6001.JPG");
    assert_eq!(fs::read(&published).unwrap(), bytes);
    assert_eq!(
        (
            file_hash(&source_file),
            fs::metadata(&source_file).unwrap().modified().unwrap()
        ),
        source_before
    );
    assert_eq!(
        fs::read(target.path().join(".fuji-photo-manager-test-root-v1")).unwrap(),
        b"generated-test-root-v1\n"
    );

    let connection = database.open_read_connection().unwrap();
    let facts: (i64, i64, i64) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM import_published_fact),
                    (SELECT COUNT(*) FROM import_indexed_fact),
                    (SELECT COUNT(*) FROM import_history)",
            (),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(facts, (1, 1, 1));
}

#[cfg(windows)]
#[test]
fn handle_publish_never_replaces_a_concurrently_created_formal_file() {
    let workspace_target = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    fs::create_dir_all(&workspace_target).unwrap();
    let target = TestRoot::create_in(&workspace_target).unwrap();
    let partial = target.path().join("owned.partial");
    let formal = target.path().join("formal.JPG");
    fs::write(&partial, b"new bytes").unwrap();
    fs::write(&formal, b"existing bytes must survive").unwrap();
    let formal_before = file_hash(&formal);
    let partial_before = file_hash(&partial);
    let handle = VerifiedPartialHandle::open(&partial).unwrap();
    assert!(
        handle
            .publish_no_replace(&formal, &FaultInjector::default())
            .is_err()
    );
    assert_eq!(file_hash(&formal), formal_before);
    assert_eq!(file_hash(&partial), partial_before);
}

#[cfg(windows)]
#[test]
fn create_new_never_adopts_or_truncates_an_existing_partial() {
    let workspace_target = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    let target = TestRoot::create_in(&workspace_target).unwrap();
    let partial = target.path().join("reserved.partial");
    fs::write(&partial, b"unknown existing bytes").unwrap();
    let before = file_hash(&partial);
    assert!(PartialWriteHandle::create_new(&partial, &FaultInjector::default()).is_err());
    assert_eq!(file_hash(&partial), before);
}

#[cfg(windows)]
#[test]
fn task_creation_commit_fault_rolls_back_before_any_file_action() {
    let faults = FaultInjector::default();
    let Some(fixture) = import_fixture(faults.clone(), "6101") else {
        return;
    };
    faults
        .fail_on_hit(FaultPoint::SqliteBeforeCommit, 1)
        .unwrap();
    let execution = ImportExecutionService::with_faults(
        fixture.database.clone(),
        fixture.preflight.clone(),
        fixture.catalog.clone(),
        ready_gate(&fixture.database),
        faults,
    );
    assert_eq!(
        execution
            .start(execution.test_permit(), &fixture.draft_digest)
            .unwrap_err()
            .code,
        ErrorCode::FaultInjected
    );
    let connection = fixture.database.open_read_connection().unwrap();
    let rows: (i64, i64, i64) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM task_intent),(SELECT COUNT(*) FROM import_job),(SELECT COUNT(*) FROM import_attempt)",
            (),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(rows, (0, 0, 0));
    assert_source_unchanged(&fixture);
}

#[cfg(windows)]
#[test]
fn crash_between_create_new_and_owned_commit_never_adopts_or_deletes_partial() {
    let faults = FaultInjector::default();
    let Some(fixture) = import_fixture(faults.clone(), "6102") else {
        return;
    };
    faults
        .fail_on_hit(FaultPoint::SqliteBeforeCommit, 7)
        .unwrap();
    let execution = ImportExecutionService::with_faults(
        fixture.database.clone(),
        fixture.preflight.clone(),
        fixture.catalog.clone(),
        ready_gate(&fixture.database),
        faults,
    );
    assert_eq!(
        execution
            .start(execution.test_permit(), &fixture.draft_digest)
            .unwrap_err()
            .code,
        ErrorCode::FaultInjected
    );
    let partial: String = fixture
        .database
        .open_read_connection()
        .unwrap()
        .query_row(
            "SELECT relative_path FROM import_partial_ownership WHERE state='Planned'",
            (),
            |row| row.get(0),
        )
        .unwrap();
    let partial = fixture.target.path().join(partial);
    assert!(partial.exists());
    let before = file_hash(&partial);
    let status = execution.retry(execution.test_permit()).unwrap();
    assert_eq!(status.phase, ImportExecutionPhase::NeedsAttention);
    assert_eq!(file_hash(&partial), before);
    assert_eq!(
        fixture
            .database
            .open_read_connection()
            .unwrap()
            .query_row("SELECT state FROM import_partial_ownership", (), |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "NeedsAttention"
    );
    assert_source_unchanged(&fixture);
}

#[cfg(windows)]
#[test]
fn publish_fact_commit_crash_is_reconciled_without_second_rename_or_overwrite() {
    let faults = FaultInjector::default();
    let Some(fixture) = import_fixture(faults.clone(), "6103") else {
        return;
    };
    faults
        .fail_on_hit(FaultPoint::SqliteBeforeCommit, 11)
        .unwrap();
    let execution = ImportExecutionService::with_faults(
        fixture.database.clone(),
        fixture.preflight.clone(),
        fixture.catalog.clone(),
        ready_gate(&fixture.database),
        faults,
    );
    assert_eq!(
        execution
            .start(execution.test_permit(), &fixture.draft_digest)
            .unwrap_err()
            .code,
        ErrorCode::FaultInjected
    );
    let connection = fixture.database.open_read_connection().unwrap();
    let state: (String, i64) = connection
        .query_row(
            "SELECT intent_state,(SELECT COUNT(*) FROM import_published_fact) FROM import_publish_intent",
            (),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, ("Publishing".into(), 0));
    drop(connection);
    assert_eq!(
        execution.retry(execution.test_permit()).unwrap().phase,
        ImportExecutionPhase::Completed
    );
    let facts: (i64, i64) = fixture
        .database
        .open_read_connection()
        .unwrap()
        .query_row(
            "SELECT (SELECT COUNT(*) FROM import_published_fact),(SELECT COUNT(*) FROM import_indexed_fact)",
            (),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(facts, (1, 1));
    assert_source_unchanged(&fixture);
}

#[cfg(windows)]
#[test]
fn crash_after_handle_delete_records_observed_missing_then_uses_new_incarnation() {
    let faults = FaultInjector::default();
    let Some(fixture) = import_fixture(faults.clone(), "6104") else {
        return;
    };
    faults
        .fail_on_hit(FaultPoint::SqliteAfterCommit, 7)
        .unwrap();
    let execution = ImportExecutionService::with_faults(
        fixture.database.clone(),
        fixture.preflight.clone(),
        fixture.catalog.clone(),
        ready_gate(&fixture.database),
        faults.clone(),
    );
    assert_eq!(
        execution
            .start(execution.test_permit(), &fixture.draft_digest)
            .unwrap_err()
            .code,
        ErrorCode::FaultInjected
    );
    faults.fail_on_hit(FaultPoint::Win32AfterCall, 1).unwrap();
    assert_eq!(
        execution.retry(execution.test_permit()).unwrap_err().code,
        ErrorCode::FaultInjected
    );
    let connection = fixture.database.open_read_connection().unwrap();
    let cleaning: (String, i64) = connection
        .query_row(
            "SELECT state,(SELECT COUNT(*) FROM import_partial_cleanup_fact) FROM import_partial_ownership",
            (),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(cleaning, ("CleaningOwnedPartial".into(), 0));
    drop(connection);
    assert_eq!(
        execution.retry(execution.test_permit()).unwrap().phase,
        ImportExecutionPhase::Completed
    );
    let connection = fixture.database.open_read_connection().unwrap();
    let cleanup: (String, i64, i64) = connection
        .query_row(
            "SELECT outcome,(SELECT COUNT(*) FROM import_partial_ownership),(SELECT MAX(ownership_ordinal) FROM import_partial_ownership) FROM import_partial_cleanup_fact",
            (),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(cleanup, ("ObservedMissingAfterCleaningIntent".into(), 2, 2));
    assert_source_unchanged(&fixture);
}

#[cfg(windows)]
#[test]
fn duplicate_is_reproved_in_validation_and_execution_without_target_change() {
    let faults = FaultInjector::default();
    let Some(fixture) =
        import_fixture_with_target(faults.clone(), "6105", Some(TargetFixture::Duplicate))
    else {
        return;
    };
    let target = fixture.preexisting_target.as_ref().unwrap();
    let target_before = (
        file_hash(target),
        fs::metadata(target).unwrap().modified().unwrap(),
    );
    let execution = ImportExecutionService::with_faults(
        fixture.database.clone(),
        fixture.preflight.clone(),
        fixture.catalog.clone(),
        ready_gate(&fixture.database),
        faults,
    );
    let status = execution
        .start(execution.test_permit(), &fixture.draft_digest)
        .unwrap();
    assert_eq!(status.phase, ImportExecutionPhase::Completed);
    assert_eq!((status.skipped_files, status.published_files), (1, 0));
    let proof_counts: (i64, i64, i64) = fixture
        .database
        .open_read_connection()
        .unwrap()
        .query_row(
            "SELECT (SELECT COUNT(*) FROM import_duplicate_proof WHERE proof_scope='Validation'),(SELECT COUNT(*) FROM import_duplicate_proof WHERE proof_scope='Execution'),(SELECT COUNT(*) FROM import_duplicate_path_binding_proof)",
            (),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(proof_counts, (1, 1, 2));
    assert_eq!(
        (
            file_hash(target),
            fs::metadata(target).unwrap().modified().unwrap()
        ),
        target_before
    );
    assert_source_unchanged(&fixture);
}

#[cfg(windows)]
#[test]
fn changed_plan_stops_before_space_and_attempt_then_cancels_without_file_change() {
    let faults = FaultInjector::default();
    let Some(fixture) =
        import_fixture_with_target(faults.clone(), "6106", Some(TargetFixture::Conflict))
    else {
        return;
    };
    // Remove the conflict before preflight, then introduce it only after the user saw the draft.
    let target = fixture.preexisting_target.as_ref().unwrap();
    fs::remove_file(target).unwrap();
    let draft = fixture
        .preflight
        .run_preflight(&fixture.preflight.test_permit())
        .unwrap();
    fs::write(target, b"concurrent formal bytes").unwrap();
    let before = file_hash(target);
    let execution = ImportExecutionService::with_faults(
        fixture.database.clone(),
        fixture.preflight.clone(),
        fixture.catalog.clone(),
        ready_gate(&fixture.database),
        faults,
    );
    let status = execution
        .start(execution.test_permit(), &draft.draft_digest)
        .unwrap();
    assert_eq!(status.phase, ImportExecutionPhase::AwaitingPlanConfirmation);
    let connection = fixture.database.open_read_connection().unwrap();
    let counts: (i64, i64, i64) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM import_plan_generation WHERE generation_state='Proposed'),(SELECT COUNT(*) FROM import_space_check),(SELECT COUNT(*) FROM import_attempt)",
            (),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(counts, (1, 0, 0));
    drop(connection);
    assert_eq!(
        execution.cancel_before_start().unwrap().phase,
        ImportExecutionPhase::Cancelled
    );
    assert_eq!(file_hash(target), before);
    assert_source_unchanged(&fixture);
}

#[cfg(windows)]
#[test]
fn gate_closing_after_publish_does_not_reject_published_fact_commit() {
    let faults = FaultInjector::default();
    let Some(fixture) = import_fixture(faults.clone(), "6107") else {
        return;
    };
    let gate = ready_gate(&fixture.database);
    faults
        .latch_gate_on_hit(
            FaultPoint::ImportAfterPhysicalBeforeFact,
            1,
            gate.clone(),
            crate::safety::GateCloseReason::CapabilityBoundary,
        )
        .unwrap();
    let execution = ImportExecutionService::with_faults(
        fixture.database.clone(),
        fixture.preflight.clone(),
        fixture.catalog.clone(),
        gate.clone(),
        faults,
    );

    let status = execution
        .start(execution.test_permit(), &fixture.draft_digest)
        .unwrap();

    assert_eq!(status.phase, ImportExecutionPhase::Completed);
    assert_eq!(
        fixture
            .database
            .open_read_connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM import_published_fact", (), |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        1
    );
    assert_eq!(
        gate.status().global_write_gate,
        crate::safety::GateState::Closed
    );
    assert_source_unchanged(&fixture);
}

#[cfg(windows)]
#[test]
fn permit_from_another_gate_instance_cannot_start_import() {
    let Some(fixture) = import_fixture(FaultInjector::default(), "6108") else {
        return;
    };
    let service_gate = ready_gate(&fixture.database);
    let foreign_gate = ready_gate(&fixture.database);
    let foreign_permit = foreign_gate.require_feature(FeatureGate::Import).unwrap();
    let execution = ImportExecutionService::new(
        fixture.database.clone(),
        fixture.preflight.clone(),
        fixture.catalog.clone(),
        service_gate,
    );

    let error = execution
        .start(foreign_permit, &fixture.draft_digest)
        .unwrap_err();

    assert_eq!(error.code, ErrorCode::CapabilityDenied);
    assert_eq!(
        fixture
            .database
            .open_read_connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM import_job", (), |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_source_unchanged(&fixture);
}
