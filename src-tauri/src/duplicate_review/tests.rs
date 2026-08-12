use std::{fs, sync::Arc};

#[cfg(windows)]
use std::sync::Mutex;

use crate::{
    catalog::{CatalogService, LibraryScanner, ScanRequest},
    db::Database,
    io_scheduler::IoScheduler,
    pending_delete::{
        AddDuplicateSubsetRequest, DuplicatePendingBatchPhase, DuplicateSubsetSelection,
        PendingDeleteService, StartDuplicatePendingBatchRequest,
    },
    roots::FujiLibraryService,
    safety::TestRoot,
};

#[cfg(windows)]
use crate::{
    recycle::{
        DuplicateReconciliationPhase, RecyclePhase, RecycleService,
        shell::{RecycleCallbacks, RecycleShell, ShellPhoto, ShellPhotoResult},
    },
    safety::{FaultInjector, RuntimeSafetyGate},
};

use super::{DuplicateReviewService, DuplicateReviewState};

fn orphaned_scan_fixture() -> (
    tempfile::TempDir,
    Arc<Database>,
    DuplicateReviewService,
    Vec<u8>,
) {
    let parent = tempfile::tempdir().unwrap();
    let app_root = TestRoot::create_in(parent.path()).unwrap();
    let library_path = parent.path().join("generated-orphaned-scan-library");
    fs::create_dir_all(&library_path).unwrap();
    let database = Arc::new(Database::open(app_root.path()).unwrap());
    let binding = FujiLibraryService::new(database.clone(), app_root.path())
        .bind(&library_path)
        .unwrap();
    let run_id = uuid::Uuid::new_v4().as_bytes().to_vec();
    let run_for_tx = run_id.clone();
    let library_id = binding.library_id.as_bytes().to_vec();
    let binding_id = binding.root_binding_id.as_bytes().to_vec();
    database
        .writer()
        .unwrap()
        .transaction(move |transaction| {
            transaction
                .execute(
                    "INSERT INTO duplicate_scan_run(
                        run_id,library_id,root_binding_id,catalog_revision,
                        run_state,phase,checked_files,candidate_files,hashed_files,started_at
                     ) VALUES (?1,?2,?3,0,'Running','Comparing',3,3,3,1)",
                    (&run_for_tx, &library_id, &binding_id),
                )
                .map_err(crate::db::migration::map_sqlite_error)?;
            Ok(())
        })
        .unwrap();
    let service =
        DuplicateReviewService::new(database.clone(), Arc::new(IoScheduler::for_rotational()));
    (parent, database, service, run_id)
}

#[test]
fn startup_reconciliation_finishes_an_orphaned_running_scan() {
    let (_parent, database, service, run_id) = orphaned_scan_fixture();
    assert_eq!(
        service.status().unwrap().state,
        DuplicateReviewState::Running
    );

    assert_eq!(service.reconcile_startup().unwrap(), 1);

    let status = service.status().unwrap();
    assert_eq!(status.state, DuplicateReviewState::Cancelled);
    assert_eq!(status.phase.as_deref(), Some("Finished"));
    let error_code: String = database
        .open_read_connection()
        .unwrap()
        .query_row(
            "SELECT error_code FROM duplicate_scan_run WHERE run_id=?1",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(error_code, "ProcessInterrupted");
}

#[test]
fn cancel_finishes_a_running_record_when_no_worker_exists() {
    let (_parent, database, service, run_id) = orphaned_scan_fixture();

    let status = service.cancel().unwrap();

    assert_eq!(status.state, DuplicateReviewState::Cancelled);
    let row: (String, String, i64) = database
        .open_read_connection()
        .unwrap()
        .query_row(
            "SELECT phase,error_code,completed_at FROM duplicate_scan_run WHERE run_id=?1",
            [&run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(row.0, "Finished");
    assert_eq!(row.1, "CancelledWithoutWorker");
    assert!(row.2 > 0);
}

#[cfg(windows)]
#[derive(Debug, Default)]
struct SuccessfulFakeRecycle {
    holding: Mutex<Vec<std::path::PathBuf>>,
}

#[cfg(windows)]
impl RecycleShell for SuccessfulFakeRecycle {
    fn recycle_photo(
        &self,
        photo: ShellPhoto,
        callbacks: Arc<dyn RecycleCallbacks>,
    ) -> Result<ShellPhotoResult, crate::domain::DomainError> {
        for asset in &photo.assets {
            callbacks.pre_delete(asset)?;
            let holding = asset
                .absolute_path
                .with_extension(format!("fake-recycle-{}", uuid::Uuid::new_v4()));
            fs::rename(&asset.absolute_path, &holding).unwrap();
            callbacks.post_delete(
                asset,
                0,
                Some(holding.to_string_lossy().into_owned().into_bytes()),
            )?;
            self.holding.lock().unwrap().push(holding);
        }
        Ok(ShellPhotoResult {
            perform_operations_result: 0,
            operations_aborted: false,
        })
    }
}

#[test]
fn scan_is_manual_skips_singletons_and_publishes_only_complete_duplicate_sets() {
    let parent = tempfile::tempdir().unwrap();
    let app_root = TestRoot::create_in(parent.path()).unwrap();
    let library_path = parent.path().join("generated-duplicate-library");
    fs::create_dir_all(library_path.join("old")).unwrap();
    fs::create_dir_all(library_path.join("new")).unwrap();
    let duplicate = [b"\xff\xd8".as_slice(), b"generated duplicate", b"\xff\xd9"].concat();
    fs::write(library_path.join("old").join("DSCF1.JPG"), &duplicate).unwrap();
    fs::write(library_path.join("new").join("DSCF1.JPG"), &duplicate).unwrap();
    fs::write(
        library_path.join("only-one.JPG"),
        [b"\xff\xd8".as_slice(), b"singleton", b"\xff\xd9"].concat(),
    )
    .unwrap();

    let database = Arc::new(Database::open(app_root.path()).unwrap());
    let binding = FujiLibraryService::new(database.clone(), app_root.path())
        .bind(&library_path)
        .unwrap();
    let catalog = Arc::new(CatalogService::new(database.clone()));
    LibraryScanner::new(catalog)
        .scan(ScanRequest {
            library_id: binding.library_id,
            root_id: binding.root_id,
            root_binding_id: binding.root_binding_id,
            root_path: library_path.clone(),
        })
        .unwrap();
    let scheduler = Arc::new(IoScheduler::for_rotational());
    let service = DuplicateReviewService::new(database.clone(), scheduler.clone());

    assert_eq!(service.status().unwrap().state, DuplicateReviewState::Idle);
    assert_eq!(service.status().unwrap().hashed_files, 0);
    service.scan_blocking_for_test().unwrap();
    let status = service.status().unwrap();
    assert_eq!(status.state, DuplicateReviewState::CompletedWithDuplicates);
    assert_eq!(status.hashed_files, 2);
    let results = service.results().unwrap();
    assert_eq!(results.summary.equivalence_sets, 1);
    assert_eq!(results.summary.redundant_files, 1);
    assert_eq!(results.families[0].jpg.as_ref().unwrap().members.len(), 2);
    assert_eq!(
        results.families[0]
            .jpg
            .as_ref()
            .unwrap()
            .members
            .iter()
            .filter(|member| member.suggested_retain)
            .count(),
        1
    );

    let set = results.families[0].jpg.as_ref().unwrap();
    let retained = set
        .members
        .iter()
        .find(|member| member.suggested_retain)
        .unwrap();
    let deleted = set
        .members
        .iter()
        .find(|member| !member.suggested_retain)
        .unwrap();
    let pending = PendingDeleteService::with_scheduler(database, scheduler);
    let mutation = pending
        .add_duplicate_subset(AddDuplicateSubsetRequest {
            run_id: status.run_id.clone().unwrap(),
            operation_id: uuid::Uuid::new_v4().to_string(),
            selections: vec![DuplicateSubsetSelection {
                set_id: set.set_id.clone(),
                deleted_asset_id: deleted.asset_id.clone(),
                deleted_asset_version: deleted.asset_version,
                retained_asset_id: retained.asset_id.clone(),
                retained_asset_version: retained.asset_version,
            }],
        })
        .unwrap();
    let pending_snapshot = pending
        .snapshot(crate::catalog::SortMode::CaptureDesc)
        .unwrap();
    assert_eq!(pending_snapshot.summary.executable_file_count, 1);
    assert_eq!(pending_snapshot.items[0].mark_scope, "DuplicateSubset");
    assert_eq!(pending_snapshot.items[0].source_label, "重复清理");
    assert_eq!(pending_snapshot.items[0].files.len(), 1);
    assert!(
        pending_snapshot.items[0].files[0]
            .retained_full_path
            .is_some()
    );
    assert_eq!(pending.session_undo_count().unwrap(), 0);
    pending
        .toggle(
            mutation.photo_id.parse().unwrap(),
            pending_snapshot.items[0].photo_version,
            uuid::Uuid::new_v4().to_string().parse().unwrap(),
        )
        .unwrap();
    assert_eq!(pending.session_undo_count().unwrap(), 0);

    fs::write(
        library_path.join("added-after-review.JPG"),
        [b"\xff\xd8".as_slice(), b"new catalog member", b"\xff\xd9"].concat(),
    )
    .unwrap();
    let catalog = Arc::new(CatalogService::new(service.database().clone()));
    LibraryScanner::new(catalog)
        .scan(ScanRequest {
            library_id: binding.library_id,
            root_id: binding.root_id,
            root_binding_id: binding.root_binding_id,
            root_path: library_path,
        })
        .unwrap();
    assert_eq!(
        service.status().unwrap().state,
        DuplicateReviewState::Expired
    );
    let expired = service.results().unwrap();
    assert!(expired.expired);
    assert!(expired.families.is_empty());
    assert_eq!(expired.summary.redundant_files, 1);
}

#[test]
fn scan_prefilters_by_case_sensitive_complete_file_name_and_size() {
    let parent = tempfile::tempdir().unwrap();
    let app_root = TestRoot::create_in(parent.path()).unwrap();
    let library_path = parent.path().join("generated-file-name-prefilter-library");
    for directory in [
        "exact-a",
        "exact-b",
        "renamed",
        "case-only",
        "size-a",
        "size-b",
        "different-a",
        "different-b",
    ] {
        fs::create_dir_all(library_path.join(directory)).unwrap();
    }
    let exact = [b"\xff\xd8".as_slice(), b"exact duplicate", b"\xff\xd9"].concat();
    fs::write(library_path.join("exact-a").join("DSCF2823.JPG"), &exact).unwrap();
    fs::write(library_path.join("exact-b").join("DSCF2823.JPG"), &exact).unwrap();
    fs::write(library_path.join("renamed").join("DSCF9999.JPG"), &exact).unwrap();
    fs::write(library_path.join("case-only").join("DSCF2823.jpg"), &exact).unwrap();
    fs::write(
        library_path.join("size-a").join("SIZE.JPG"),
        [b"\xff\xd8".as_slice(), b"short", b"\xff\xd9"].concat(),
    )
    .unwrap();
    fs::write(
        library_path.join("size-b").join("SIZE.JPG"),
        [b"\xff\xd8".as_slice(), b"a little longer", b"\xff\xd9"].concat(),
    )
    .unwrap();
    fs::write(
        library_path.join("different-a").join("DIFF.JPG"),
        [b"\xff\xd8".as_slice(), b"same-size-value-A", b"\xff\xd9"].concat(),
    )
    .unwrap();
    fs::write(
        library_path.join("different-b").join("DIFF.JPG"),
        [b"\xff\xd8".as_slice(), b"same-size-value-B", b"\xff\xd9"].concat(),
    )
    .unwrap();

    let database = Arc::new(Database::open(app_root.path()).unwrap());
    let binding = FujiLibraryService::new(database.clone(), app_root.path())
        .bind(&library_path)
        .unwrap();
    let catalog = Arc::new(CatalogService::new(database.clone()));
    LibraryScanner::new(catalog)
        .scan(ScanRequest {
            library_id: binding.library_id,
            root_id: binding.root_id,
            root_binding_id: binding.root_binding_id,
            root_path: library_path,
        })
        .unwrap();
    let service = DuplicateReviewService::new(database, Arc::new(IoScheduler::for_rotational()));
    service.scan_blocking_for_test().unwrap();

    let status = service.status().unwrap();
    assert_eq!(status.state, DuplicateReviewState::CompletedWithDuplicates);
    assert_eq!(status.candidate_files, 4);
    assert_eq!(status.checked_files, 4);
    assert_eq!(status.hashed_files, 4);
    let results = service.results().unwrap();
    assert_eq!(results.summary.equivalence_sets, 1);
    assert_eq!(results.summary.redundant_files, 1);
    let members = &results.families[0].jpg.as_ref().unwrap().members;
    assert_eq!(members.len(), 2);
    assert!(
        members
            .iter()
            .all(|member| member.relative_path.ends_with("DSCF2823.JPG"))
    );
}

#[test]
fn duplicate_pending_batch_marks_multiple_photos_in_one_atomic_commit() {
    let fixture = duplicate_batch_fixture();
    let pending = Arc::new(PendingDeleteService::with_scheduler(
        fixture.database,
        fixture.scheduler,
    ));
    let started = pending
        .start_duplicate_pending_batch(StartDuplicatePendingBatchRequest {
            run_id: fixture.run_id,
            operation_id: uuid::Uuid::new_v4().to_string(),
            selections: fixture.selections,
        })
        .unwrap();
    assert_eq!(started.phase, DuplicatePendingBatchPhase::Validating);

    let completed = wait_for_duplicate_batch(&pending);
    assert_eq!(completed.phase, DuplicatePendingBatchPhase::Completed);
    assert_eq!(completed.marked_photos, 2);
    assert_eq!(completed.marked_files, 2);
    let snapshot = pending
        .snapshot(crate::catalog::SortMode::CaptureDesc)
        .unwrap();
    assert_eq!(snapshot.items.len(), 2);
    assert_eq!(snapshot.summary.executable_file_count, 2);
    assert!(
        snapshot
            .items
            .iter()
            .all(|item| item.mark_scope == "DuplicateSubset")
    );
}

#[test]
fn duplicate_pending_batch_replays_the_same_operation_without_new_marks() {
    let fixture = duplicate_batch_fixture();
    let pending = Arc::new(PendingDeleteService::with_scheduler(
        fixture.database,
        fixture.scheduler,
    ));
    let operation_id = uuid::Uuid::new_v4().to_string();
    let request = StartDuplicatePendingBatchRequest {
        run_id: fixture.run_id,
        operation_id,
        selections: fixture.selections,
    };
    pending
        .start_duplicate_pending_batch(request.clone())
        .unwrap();
    assert_eq!(
        wait_for_duplicate_batch(&pending).phase,
        DuplicatePendingBatchPhase::Completed
    );
    pending
        .start_duplicate_pending_batch(request.clone())
        .unwrap();
    let replay = wait_for_duplicate_batch(&pending);
    assert_eq!(replay.phase, DuplicatePendingBatchPhase::Completed);
    let snapshot = pending
        .snapshot(crate::catalog::SortMode::CaptureDesc)
        .unwrap();
    assert_eq!(snapshot.items.len(), 2);

    let mut changed_request = request;
    changed_request.selections.pop();
    pending
        .start_duplicate_pending_batch(changed_request)
        .unwrap();
    let rejected = wait_for_duplicate_batch(&pending);
    assert_eq!(rejected.phase, DuplicatePendingBatchPhase::Failed);
    assert_eq!(
        rejected.failure.unwrap().code,
        crate::domain::ErrorCode::DuplicateOperation
    );
    assert_eq!(
        pending
            .snapshot(crate::catalog::SortMode::CaptureDesc)
            .unwrap()
            .items
            .len(),
        2
    );
}

#[test]
fn duplicate_pending_batch_failure_leaves_every_photo_unmarked() {
    let fixture = duplicate_batch_fixture();
    fs::write(
        fixture.changed_path,
        [b"\xff\xd8".as_slice(), b"changed duplicate-B", b"\xff\xd9"].concat(),
    )
    .unwrap();
    let pending = Arc::new(PendingDeleteService::with_scheduler(
        fixture.database,
        fixture.scheduler,
    ));
    pending
        .start_duplicate_pending_batch(StartDuplicatePendingBatchRequest {
            run_id: fixture.run_id,
            operation_id: uuid::Uuid::new_v4().to_string(),
            selections: fixture.selections,
        })
        .unwrap();

    let failed = wait_for_duplicate_batch(&pending);
    assert_eq!(failed.phase, DuplicatePendingBatchPhase::Failed);
    assert!(failed.failure.is_some());
    let snapshot = pending
        .snapshot(crate::catalog::SortMode::CaptureDesc)
        .unwrap();
    assert!(snapshot.items.is_empty());
    assert_eq!(snapshot.summary.executable_file_count, 0);
}

struct DuplicateBatchFixture {
    _parent: tempfile::TempDir,
    database: Arc<Database>,
    scheduler: Arc<IoScheduler>,
    run_id: String,
    selections: Vec<DuplicateSubsetSelection>,
    changed_path: std::path::PathBuf,
}

fn duplicate_batch_fixture() -> DuplicateBatchFixture {
    let parent = tempfile::tempdir().unwrap();
    let app_root = TestRoot::create_in(parent.path()).unwrap();
    let library_path = parent.path().join("generated-duplicate-batch-library");
    for directory in ["keep-a", "delete-a", "keep-b", "delete-b"] {
        fs::create_dir_all(library_path.join(directory)).unwrap();
    }
    let duplicate_a = [b"\xff\xd8".as_slice(), b"duplicate-A", b"\xff\xd9"].concat();
    let duplicate_b = [b"\xff\xd8".as_slice(), b"original duplicate-B", b"\xff\xd9"].concat();
    fs::write(library_path.join("keep-a").join("A.JPG"), &duplicate_a).unwrap();
    fs::write(library_path.join("delete-a").join("A.JPG"), &duplicate_a).unwrap();
    fs::write(library_path.join("keep-b").join("B.JPG"), &duplicate_b).unwrap();
    let changed_path = library_path.join("delete-b").join("B.JPG");
    fs::write(&changed_path, &duplicate_b).unwrap();

    let database = Arc::new(Database::open(app_root.path()).unwrap());
    let binding = FujiLibraryService::new(database.clone(), app_root.path())
        .bind(&library_path)
        .unwrap();
    let catalog = Arc::new(CatalogService::new(database.clone()));
    LibraryScanner::new(catalog)
        .scan(ScanRequest {
            library_id: binding.library_id,
            root_id: binding.root_id,
            root_binding_id: binding.root_binding_id,
            root_path: library_path,
        })
        .unwrap();
    let scheduler = Arc::new(IoScheduler::for_rotational());
    let duplicate = DuplicateReviewService::new(database.clone(), scheduler.clone());
    duplicate.scan_blocking_for_test().unwrap();
    let results = duplicate.results().unwrap();
    let selections = results
        .families
        .iter()
        .flat_map(|family| [family.jpg.as_ref(), family.raf.as_ref()])
        .flatten()
        .map(|set| {
            let retained = set
                .members
                .iter()
                .find(|member| member.suggested_retain)
                .unwrap();
            let deleted = set
                .members
                .iter()
                .find(|member| !member.suggested_retain)
                .unwrap();
            DuplicateSubsetSelection {
                set_id: set.set_id.clone(),
                deleted_asset_id: deleted.asset_id.clone(),
                deleted_asset_version: deleted.asset_version,
                retained_asset_id: retained.asset_id.clone(),
                retained_asset_version: retained.asset_version,
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(selections.len(), 2);
    DuplicateBatchFixture {
        _parent: parent,
        database,
        scheduler,
        run_id: results.run_id.unwrap(),
        selections,
        changed_path,
    }
}

fn wait_for_duplicate_batch(
    pending: &PendingDeleteService,
) -> crate::pending_delete::DuplicatePendingBatchStatus {
    for _ in 0..500 {
        let status = pending.duplicate_pending_batch_status().unwrap();
        if matches!(
            status.phase,
            DuplicatePendingBatchPhase::Completed
                | DuplicatePendingBatchPhase::Failed
                | DuplicatePendingBatchPhase::Cancelled
        ) {
            return status;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("duplicate pending batch did not reach a terminal state");
}

#[cfg(windows)]
#[test]
fn recycled_duplicate_subset_updates_catalog_and_derives_empty_result_without_rescan() {
    let parent = tempfile::tempdir().unwrap();
    let app_root = TestRoot::create_in(parent.path()).unwrap();
    let library_path = parent.path().join("controlled-reconciliation-library");
    fs::create_dir_all(library_path.join("keep")).unwrap();
    fs::create_dir_all(library_path.join("delete")).unwrap();
    let duplicate = [b"\xff\xd8".as_slice(), b"controlled duplicate", b"\xff\xd9"].concat();
    fs::write(library_path.join("keep").join("DSCF9.JPG"), &duplicate).unwrap();
    fs::write(library_path.join("delete").join("DSCF9.JPG"), &duplicate).unwrap();

    let database = Arc::new(Database::open(app_root.path()).unwrap());
    let binding = FujiLibraryService::new(database.clone(), app_root.path())
        .bind(&library_path)
        .unwrap();
    let catalog = Arc::new(CatalogService::new(database.clone()));
    LibraryScanner::new(catalog)
        .scan(ScanRequest {
            library_id: binding.library_id,
            root_id: binding.root_id,
            root_binding_id: binding.root_binding_id,
            root_path: library_path,
        })
        .unwrap();
    let scheduler = Arc::new(IoScheduler::for_rotational());
    let duplicate_service = DuplicateReviewService::new(database.clone(), scheduler.clone());
    duplicate_service.scan_blocking_for_test().unwrap();
    let before = duplicate_service.results().unwrap();
    let set = before.families[0].jpg.as_ref().unwrap();
    let retained = set
        .members
        .iter()
        .find(|member| member.suggested_retain)
        .unwrap();
    let deleted = set
        .members
        .iter()
        .find(|member| !member.suggested_retain)
        .unwrap();
    let pending = Arc::new(PendingDeleteService::with_scheduler(
        database.clone(),
        scheduler,
    ));
    pending
        .add_duplicate_subset(AddDuplicateSubsetRequest {
            run_id: before.run_id.clone().unwrap(),
            operation_id: uuid::Uuid::new_v4().to_string(),
            selections: vec![DuplicateSubsetSelection {
                set_id: set.set_id.clone(),
                deleted_asset_id: deleted.asset_id.clone(),
                deleted_asset_version: deleted.asset_version,
                retained_asset_id: retained.asset_id.clone(),
                retained_asset_version: retained.asset_version,
            }],
        })
        .unwrap();

    let gate = Arc::new(RuntimeSafetyGate::new(database.clone()));
    gate.mark_ready().unwrap();
    let recycle = RecycleService::with_shell(
        database.clone(),
        pending,
        Arc::new(SuccessfulFakeRecycle::default()),
        gate,
        FaultInjector::default(),
    );
    let confirmation = recycle.confirmation().unwrap();
    let status = recycle
        .start(
            recycle.test_permit(),
            uuid::Uuid::new_v4(),
            confirmation.logical_photo_count,
            confirmation.physical_file_count,
            confirmation.total_bytes,
        )
        .unwrap();
    assert_eq!(status.phase, RecyclePhase::Completed);
    assert_eq!(
        status.duplicate_reconciliation,
        DuplicateReconciliationPhase::Applied
    );
    assert_eq!(status.successful_duplicate_file_count, 1);

    let after = duplicate_service.results().unwrap();
    assert!(!after.expired);
    assert!(after.families.is_empty());
    assert_eq!(after.summary.redundant_files, 0);
    assert_eq!(after.last_receipt.unwrap().state, "applied");
    let connection = database.open_read_connection().unwrap();
    let presence: String = connection
        .query_row(
            "SELECT presence_state FROM asset WHERE asset_id=?1",
            [uuid::Uuid::parse_str(&deleted.asset_id)
                .unwrap()
                .as_bytes()
                .as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(presence, "Missing");
}
