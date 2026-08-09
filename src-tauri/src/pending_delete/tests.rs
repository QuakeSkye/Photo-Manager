use std::{fs, sync::Arc};

use crate::{
    catalog::{
        AssetObservation, CatalogObservation, CatalogService, LibraryScanner, MediaKind,
        ScanRequest,
    },
    db::Database,
    domain::{ErrorCode, OperationId},
    roots::FujiLibraryService,
    safety::{FaultInjector, FaultPoint, TestRoot},
};

use super::*;

struct Fixture {
    _parent: tempfile::TempDir,
    library_path: std::path::PathBuf,
    database: Arc<Database>,
    catalog: CatalogService,
    service: PendingDeleteService,
    binding: crate::roots::FujiLibraryBinding,
}

fn fixture_with_injector(injector: FaultInjector) -> Fixture {
    let parent = tempfile::tempdir().unwrap();
    let app_root = TestRoot::create_in(parent.path()).unwrap();
    let library_path = parent.path().join("disposable-photo-fixture");
    fs::create_dir(&library_path).unwrap();
    let database = Arc::new(Database::open_with_injector(app_root.path(), injector).unwrap());
    let binding = FujiLibraryService::new(database.clone(), app_root.path())
        .bind(&library_path)
        .unwrap();
    Fixture {
        _parent: parent,
        library_path,
        catalog: CatalogService::new(database.clone()),
        service: PendingDeleteService::new(database.clone()),
        database,
        binding,
    }
}

fn fixture() -> Fixture {
    fixture_with_injector(FaultInjector::default())
}

fn add_photo(fixture: &Fixture, name: &str, file_id: u8) -> (PhotoId, i64) {
    let bytes = format!("disposable-{name}").into_bytes();
    fs::write(fixture.library_path.join(name), &bytes).unwrap();
    let observed = fixture
        .catalog
        .observe_asset(AssetObservation {
            library_id: fixture.binding.library_id,
            root_id: fixture.binding.root_id,
            root_binding_id: fixture.binding.root_binding_id,
            media_kind: if name.to_ascii_lowercase().ends_with(".raf") {
                MediaKind::Raf
            } else {
                MediaKind::Jpg
            },
            relative_path: name.to_string(),
            path_key: name.to_ascii_lowercase().into_bytes(),
            file_id: Some(vec![file_id]),
            size_bytes: i64::try_from(bytes.len()).unwrap(),
            mtime_filetime: 100 + i64::from(file_id),
            observed_at: 1_000 + i64::from(file_id),
        })
        .unwrap();
    (observed.photo_id, observed.photo_version)
}

fn table_count(database: &Database, table: &str) -> i64 {
    database
        .open_read_connection()
        .unwrap()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), (), |row| {
            row.get(0)
        })
        .unwrap()
}

fn generated_pair_bytes() -> (Vec<u8>, Vec<u8>) {
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
    tiff.extend_from_slice(b"2026:07:30 12:00:00\0");
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend(tiff);
    let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
    jpeg.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
    jpeg.extend(payload);
    jpeg.extend_from_slice(&[0xff, 0xd9]);
    let mut raf = vec![0; 92];
    raf[..16].copy_from_slice(b"FUJIFILMCCD-RAW ");
    raf[84..88].copy_from_slice(&92_u32.to_be_bytes());
    raf[88..92].copy_from_slice(&u32::try_from(jpeg.len()).unwrap().to_be_bytes());
    raf.extend_from_slice(&jpeg);
    (jpeg, raf)
}

#[test]
fn d_and_r_are_atomic_idempotent_and_multi_step() {
    let fixture = fixture();
    let before_a = b"disposable-A.JPG".to_vec();
    let before_b = b"disposable-B.JPG".to_vec();
    let (photo_a, version_a) = add_photo(&fixture, "A.JPG", 1);
    let (photo_b, version_b) = add_photo(&fixture, "B.JPG", 2);
    let operation_a = OperationId::new();
    let operation_b = OperationId::new();

    let first = fixture
        .service
        .toggle(photo_a, version_a, operation_a)
        .unwrap();
    assert!(first.marked);
    assert_eq!(first.summary.total_count, 1);
    assert_eq!(
        table_count(&fixture.database, "deletion_mark_asset_snapshot"),
        1
    );
    assert_eq!(table_count(&fixture.database, "file_identity_snapshot"), 1);

    let duplicate = fixture
        .service
        .toggle(photo_a, version_a, operation_a)
        .unwrap();
    assert!(duplicate.marked);
    assert_eq!(duplicate.summary.total_count, 1);
    assert_eq!(fixture.service.session_undo_count().unwrap(), 1);

    fixture
        .service
        .toggle(photo_b, version_b, operation_b)
        .unwrap();
    assert_eq!(fixture.service.summary().unwrap().total_count, 2);

    let undo_b_operation = OperationId::new();
    let undo_b = fixture.service.undo(undo_b_operation).unwrap();
    assert_eq!(undo_b.photo_id, photo_b.to_string());
    assert!(!undo_b.marked);
    assert_eq!(undo_b.summary.total_count, 1);
    let replayed_undo_b = fixture.service.undo(undo_b_operation).unwrap();
    assert_eq!(replayed_undo_b.photo_id, photo_b.to_string());
    assert!(!replayed_undo_b.marked);
    assert_eq!(replayed_undo_b.summary.total_count, 1);
    assert_eq!(fixture.service.session_undo_count().unwrap(), 1);
    let undo_a = fixture.service.undo(OperationId::new()).unwrap();
    assert_eq!(undo_a.photo_id, photo_a.to_string());
    assert_eq!(undo_a.summary.total_count, 0);
    assert_eq!(fixture.service.session_undo_count().unwrap(), 0);

    assert_eq!(
        fs::read(fixture.library_path.join("A.JPG")).unwrap(),
        before_a
    );
    assert_eq!(
        fs::read(fixture.library_path.join("B.JPG")).unwrap(),
        before_b
    );
    assert_eq!(table_count(&fixture.database, "deletion_batch"), 0);
    assert_eq!(table_count(&fixture.database, "deletion_attempt"), 0);
}

#[test]
fn paired_photo_freezes_every_member_in_the_same_commit() {
    let fixture = fixture();
    let (jpeg, raf) = generated_pair_bytes();
    fs::write(fixture.library_path.join("PAIR.JPG"), jpeg).unwrap();
    fs::write(fixture.library_path.join("PAIR.RAF"), raf).unwrap();
    LibraryScanner::new(Arc::new(CatalogService::new(fixture.database.clone())))
        .scan(ScanRequest {
            library_id: fixture.binding.library_id,
            root_id: fixture.binding.root_id,
            root_binding_id: fixture.binding.root_binding_id,
            root_path: fixture.library_path.clone(),
        })
        .unwrap();
    let timeline = fixture
        .catalog
        .timeline(fixture.binding.library_id, SortMode::CaptureDesc)
        .unwrap();
    assert_eq!(timeline.len(), 1);
    let paired = timeline[0].clone();

    fixture
        .service
        .toggle(paired.photo_id, paired.photo_version, OperationId::new())
        .unwrap();
    let connection = fixture.database.open_read_connection().unwrap();
    let members: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM photo_asset WHERE photo_id=?1 AND to_photo_version IS NULL",
            [paired.photo_id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    let snapshots: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM deletion_mark_asset_snapshot snapshot
             JOIN deletion_mark mark ON mark.mark_id=snapshot.mark_id
             WHERE mark.photo_id=?1 AND mark.released_at IS NULL",
            [paired.photo_id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(snapshots, members);
    assert_eq!(members, 2);
    let pending = fixture.service.snapshot(SortMode::CaptureDesc).unwrap();
    assert_eq!(pending.items[0].physical_file_count, 2);
}

#[test]
fn commit_failure_leaves_no_partial_mark_or_snapshot() {
    let injector = FaultInjector::default();
    let fixture = fixture_with_injector(injector.clone());
    let (photo, version) = add_photo(&fixture, "FAIL.JPG", 5);
    let before = fs::read(fixture.library_path.join("FAIL.JPG")).unwrap();
    injector
        .fail_on_hit(FaultPoint::SqliteBeforeCommit, 1)
        .unwrap();

    let error = fixture
        .service
        .toggle(photo, version, OperationId::new())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::FaultInjected);
    assert_eq!(table_count(&fixture.database, "deletion_mark"), 0);
    assert_eq!(
        table_count(&fixture.database, "deletion_mark_asset_snapshot"),
        0
    );
    assert_eq!(table_count(&fixture.database, "file_identity_snapshot"), 0);
    assert_eq!(table_count(&fixture.database, "operation_event"), 0);
    assert_eq!(
        fs::read(fixture.library_path.join("FAIL.JPG")).unwrap(),
        before
    );
}

#[test]
fn concurrent_mark_write_is_rejected_instead_of_queued() {
    let fixture = fixture();
    let (photo, version) = add_photo(&fixture, "BUSY.JPG", 6);
    let _held = fixture.service.write_gate.lock().unwrap();

    let error = fixture
        .service
        .toggle(photo, version, OperationId::new())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::DatabaseBusy);
    assert_eq!(table_count(&fixture.database, "deletion_mark"), 0);
}

#[test]
fn offline_root_allows_removal_but_rejects_add_and_restore() {
    let fixture = fixture();
    let (photo, version) = add_photo(&fixture, "OFFLINE.JPG", 7);
    fixture
        .service
        .toggle(photo, version, OperationId::new())
        .unwrap();
    fixture
        .catalog
        .mark_root_unavailable(fixture.binding.root_id, false)
        .unwrap();

    let snapshot = fixture.service.snapshot(SortMode::CaptureDesc).unwrap();
    assert_eq!(snapshot.summary.problem_count, 1);
    assert_eq!(
        snapshot.items[0].problem.as_ref().unwrap().kind,
        ProblemKind::RootOffline
    );
    let removed = fixture
        .service
        .toggle(photo, version, OperationId::new())
        .unwrap();
    assert_eq!(removed.summary.total_count, 0);
    let restore_error = fixture.service.undo(OperationId::new()).unwrap_err();
    assert_eq!(restore_error.code, ErrorCode::RootOffline);
    let add_error = fixture
        .service
        .toggle(photo, version, OperationId::new())
        .unwrap_err();
    assert_eq!(add_error.code, ErrorCode::RootOffline);
}

#[test]
fn catalog_version_change_derives_open_problem_without_refreshing_snapshot() {
    let fixture = fixture();
    let (photo, version) = add_photo(&fixture, "CHANGED.JPG", 8);
    fixture
        .service
        .toggle(photo, version, OperationId::new())
        .unwrap();
    let original_snapshot_size: i64 = fixture
        .database
        .open_read_connection()
        .unwrap()
        .query_row(
            "SELECT identity.size_bytes
             FROM deletion_mark_asset_snapshot snapshot
             JOIN file_identity_snapshot identity
               ON identity.snapshot_id=snapshot.file_identity_snapshot_id
             JOIN deletion_mark mark ON mark.mark_id=snapshot.mark_id
             WHERE mark.photo_id=?1 AND mark.released_at IS NULL",
            [photo.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();

    let changed = fixture
        .catalog
        .observe_asset(AssetObservation {
            library_id: fixture.binding.library_id,
            root_id: fixture.binding.root_id,
            root_binding_id: fixture.binding.root_binding_id,
            media_kind: MediaKind::Jpg,
            relative_path: "CHANGED.JPG".to_string(),
            path_key: b"changed.jpg".to_vec(),
            file_id: Some(vec![8]),
            size_bytes: original_snapshot_size + 1,
            mtime_filetime: 999,
            observed_at: 9_999,
        })
        .unwrap();
    assert!(changed.photo_version > version);

    let pending = fixture.service.snapshot(SortMode::CaptureDesc).unwrap();
    assert_eq!(pending.summary.executable_count, 0);
    assert_eq!(pending.summary.problem_count, 1);
    assert_eq!(
        pending.items[0].problem.as_ref().unwrap().lifecycle,
        ProblemLifecycle::Open
    );
    assert_eq!(
        pending.items[0].problem.as_ref().unwrap().kind,
        ProblemKind::FileIdentityChanged
    );
    let unchanged_snapshot_size: i64 = fixture
        .database
        .open_read_connection()
        .unwrap()
        .query_row(
            "SELECT identity.size_bytes
             FROM deletion_mark_asset_snapshot snapshot
             JOIN file_identity_snapshot identity
               ON identity.snapshot_id=snapshot.file_identity_snapshot_id
             JOIN deletion_mark mark ON mark.mark_id=snapshot.mark_id
             WHERE mark.photo_id=?1 AND mark.released_at IS NULL",
            [photo.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unchanged_snapshot_size, original_snapshot_size);
}

#[test]
fn member_change_invalidates_the_frozen_mark() {
    let fixture = fixture();
    let (photo, version) = add_photo(&fixture, "MEMBERS.JPG", 10);
    fixture
        .service
        .toggle(photo, version, OperationId::new())
        .unwrap();

    fixture
        .database
        .writer()
        .unwrap()
        .transaction(move |transaction| {
            let next_version = version + 1;
            transaction
                .execute(
                    "UPDATE photo SET photo_version=?1, row_version=row_version+1
                     WHERE photo_id=?2",
                    (next_version, photo.as_bytes().as_slice()),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE photo_asset SET to_photo_version=?1
                     WHERE photo_id=?2 AND to_photo_version IS NULL",
                    (next_version, photo.as_bytes().as_slice()),
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })
        .unwrap();

    let pending = fixture.service.snapshot(SortMode::CaptureDesc).unwrap();
    assert_eq!(pending.summary.problem_count, 1);
    assert_eq!(pending.summary.executable_count, 0);
    assert_eq!(
        pending.items[0].problem.as_ref().unwrap().kind,
        ProblemKind::MembersChanged
    );
}

#[test]
fn missing_scan_fact_is_a_problem_without_changing_the_disposable_file() {
    let fixture = fixture();
    let (photo, version) = add_photo(&fixture, "MISSING.JPG", 12);
    let original = fs::read(fixture.library_path.join("MISSING.JPG")).unwrap();
    fixture
        .service
        .toggle(photo, version, OperationId::new())
        .unwrap();
    let scan = fixture
        .catalog
        .begin_scan(
            fixture.binding.library_id,
            fixture.binding.root_id,
            fixture.binding.root_binding_id,
        )
        .unwrap();
    fixture.catalog.complete_scan(scan, &[], 0, 0, 0).unwrap();

    let pending = fixture.service.snapshot(SortMode::CaptureDesc).unwrap();
    assert_eq!(pending.summary.problem_count, 1);
    assert_eq!(
        pending.items[0].problem.as_ref().unwrap().kind,
        ProblemKind::FileMissing
    );
    assert_eq!(
        fs::read(fixture.library_path.join("MISSING.JPG")).unwrap(),
        original
    );
}

#[test]
fn hard_link_change_invalidates_the_mark_without_touching_the_file() {
    let fixture = fixture();
    let (photo, version) = add_photo(&fixture, "LINKED.JPG", 13);
    let original = fs::read(fixture.library_path.join("LINKED.JPG")).unwrap();
    fixture
        .service
        .toggle(photo, version, OperationId::new())
        .unwrap();
    fixture
        .catalog
        .observe_media_asset(CatalogObservation {
            asset: AssetObservation {
                library_id: fixture.binding.library_id,
                root_id: fixture.binding.root_id,
                root_binding_id: fixture.binding.root_binding_id,
                media_kind: MediaKind::Jpg,
                relative_path: "LINKED.JPG".to_string(),
                path_key: b"linked.jpg".to_vec(),
                file_id: Some(vec![13]),
                size_bytes: i64::try_from(original.len()).unwrap(),
                mtime_filetime: 113,
                observed_at: 10_013,
            },
            hard_link_count: 2,
            deletion_supported: false,
            scan_run_id: None,
            capture_local_key: None,
            camera_model: None,
            camera_serial: None,
            capture_metadata: crate::media::CaptureMetadata::default(),
            preview: crate::media::PreviewState::JpegSource,
        })
        .unwrap();

    let pending = fixture.service.snapshot(SortMode::CaptureDesc).unwrap();
    assert_eq!(pending.summary.problem_count, 1);
    assert_eq!(
        pending.items[0].problem.as_ref().unwrap().kind,
        ProblemKind::UnsupportedHardLink
    );
    assert_eq!(
        fs::read(fixture.library_path.join("LINKED.JPG")).unwrap(),
        original
    );
}

#[test]
fn restart_restores_entry_but_not_session_undo_history() {
    let parent = tempfile::tempdir().unwrap();
    let app_root = TestRoot::create_in(parent.path()).unwrap();
    let library_path = parent.path().join("restart-library");
    fs::create_dir(&library_path).unwrap();
    {
        let database = Arc::new(Database::open(app_root.path()).unwrap());
        let binding = FujiLibraryService::new(database.clone(), app_root.path())
            .bind(&library_path)
            .unwrap();
        fs::write(library_path.join("RECOVER.JPG"), b"recover").unwrap();
        let catalog = CatalogService::new(database.clone());
        let observed = catalog
            .observe_asset(AssetObservation {
                library_id: binding.library_id,
                root_id: binding.root_id,
                root_binding_id: binding.root_binding_id,
                media_kind: MediaKind::Jpg,
                relative_path: "RECOVER.JPG".to_string(),
                path_key: b"recover.jpg".to_vec(),
                file_id: Some(vec![9]),
                size_bytes: 7,
                mtime_filetime: 109,
                observed_at: 1_009,
            })
            .unwrap();
        PendingDeleteService::new(database.clone())
            .toggle(
                observed.photo_id,
                observed.photo_version,
                OperationId::new(),
            )
            .unwrap();
    }

    let reopened = Arc::new(Database::open(app_root.path()).unwrap());
    let recovered = PendingDeleteService::new(reopened);
    assert_eq!(recovered.summary().unwrap().total_count, 1);
    assert_eq!(recovered.session_undo_count().unwrap(), 0);
    let error = recovered.undo(OperationId::new()).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidTransition);
}

#[test]
fn derived_problem_lifecycle_covers_open_resolved_and_superseded() {
    assert_eq!(
        derive_problem_lifecycle(true, false, false),
        Some(ProblemLifecycle::Open)
    );
    assert_eq!(
        derive_problem_lifecycle(false, false, false),
        Some(ProblemLifecycle::Resolved)
    );
    assert_eq!(
        derive_problem_lifecycle(false, false, true),
        Some(ProblemLifecycle::Superseded)
    );
    assert_eq!(derive_problem_lifecycle(true, true, false), None);
}
