use super::*;

#[cfg(windows)]
use std::{
    collections::VecDeque,
    fs,
    sync::atomic::{AtomicUsize, Ordering},
};

#[cfg(windows)]
use crate::{
    catalog::{CatalogService, LibraryScanner, ScanRequest, SortMode},
    domain::OperationId,
    roots::FujiLibraryService,
    safety::TestRoot,
};

#[test]
fn confirmation_counts_logical_photos_physical_files_and_bytes() {
    let asset = |size_bytes| FrozenAsset {
        mark_id: vec![1; 16],
        mark_snapshot_id: vec![2; 16],
        _source_snapshot_id: vec![3; 16],
        photo_id: vec![4; 16],
        photo_version: 1,
        asset_id: vec![5; 16],
        asset_version: 1,
        role: "Jpg".into(),
        library_id: vec![6; 16],
        root_id: vec![7; 16],
        root_binding_id: vec![8; 16],
        volume_id: vec![9; 16],
        root_path: PathBuf::from("unused"),
        root_path_key: b"unused".to_vec(),
        root_directory_file_id: None,
        relative_path: "DSCF0001.JPG".into(),
        path_key: b"dscf0001.jpg".to_vec(),
        path_key_version: 1,
        file_id: vec![10; 12],
        size_bytes,
        mtime_filetime: 1,
    };
    let photos = vec![
        FrozenPhoto {
            mark_id: vec![1; 16],
            photo_id: vec![4; 16],
            photo_version: 1,
            assets: vec![asset(10), asset(20)],
        },
        FrozenPhoto {
            mark_id: vec![11; 16],
            photo_id: vec![12; 16],
            photo_version: 1,
            assets: vec![asset(30)],
        },
    ];

    assert_eq!(
        confirmation_for_photos(&photos, false),
        RecycleConfirmation {
            logical_photo_count: 2,
            physical_file_count: 3,
            total_bytes: 60,
            retry: false,
            can_execute: true,
            selection_digest: selection_digest(&photos, false),
        }
    );
}

#[cfg(windows)]
#[derive(Clone, Copy)]
enum FakeOutcome {
    Recycled,
    FailedInPlace,
    EmptyRecycleObject,
    ReplaceBeforePre,
}

#[cfg(windows)]
struct FakeShell {
    outcomes: Mutex<VecDeque<FakeOutcome>>,
    calls: AtomicUsize,
    photos: Mutex<Vec<usize>>,
}

#[cfg(windows)]
impl FakeShell {
    fn new(outcomes: impl IntoIterator<Item = FakeOutcome>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into_iter().collect()),
            calls: AtomicUsize::new(0),
            photos: Mutex::new(Vec::new()),
        }
    }

    fn push(&self, outcome: FakeOutcome) {
        self.outcomes.lock().unwrap().push_back(outcome);
    }
}

#[cfg(windows)]
impl RecycleShell for FakeShell {
    fn recycle_photo(
        &self,
        photo: ShellPhoto,
        callbacks: Arc<dyn RecycleCallbacks>,
    ) -> Result<ShellPhotoResult, DomainError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.photos.lock().unwrap().push(photo.assets.len());
        for asset in &photo.assets {
            let outcome = self
                .outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(FakeOutcome::Recycled);
            if matches!(outcome, FakeOutcome::ReplaceBeforePre) {
                let old = asset.absolute_path.with_extension("race-old");
                fs::rename(&asset.absolute_path, &old).unwrap();
                fs::write(&asset.absolute_path, b"replacement").unwrap();
            }
            callbacks.pre_delete(asset)?;
            match outcome {
                FakeOutcome::Recycled | FakeOutcome::EmptyRecycleObject => {
                    let holding = asset
                        .absolute_path
                        .parent()
                        .unwrap()
                        .join(format!(".fake-recycle-{}", Uuid::new_v4()));
                    fs::rename(&asset.absolute_path, &holding).unwrap();
                    callbacks.post_delete(
                        asset,
                        0,
                        matches!(outcome, FakeOutcome::Recycled)
                            .then(|| holding.to_string_lossy().into_owned().into_bytes()),
                    )?;
                }
                FakeOutcome::FailedInPlace => callbacks.post_delete(asset, -1, None)?,
                FakeOutcome::ReplaceBeforePre => unreachable!(),
            }
        }
        Ok(ShellPhotoResult {
            perform_operations_result: 0,
            operations_aborted: false,
        })
    }
}

#[cfg(windows)]
struct Fixture {
    _parent: tempfile::TempDir,
    _app_root: TestRoot,
    database: Arc<Database>,
    pending: Arc<PendingDeleteService>,
    database_faults: FaultInjector,
}

#[cfg(windows)]
fn fixture() -> Fixture {
    fixture_with_pair(false)
}

#[cfg(windows)]
fn fixture_with_pair(include_second_photo: bool) -> Fixture {
    let parent = tempfile::tempdir().unwrap();
    let app_root = TestRoot::create_in(parent.path()).unwrap();
    let library_root = TestRoot::create_in(parent.path()).unwrap();
    fs::write(
        library_root.path().join("DSCF7001.JPG"),
        b"phase-seven-fixture",
    )
    .unwrap();
    if include_second_photo {
        fs::write(
            library_root.path().join("DSCF7002.JPG"),
            b"phase-seven-second-jpg-fixture",
        )
        .unwrap();
    }
    let database_faults = FaultInjector::default();
    let database =
        Arc::new(Database::open_with_injector(app_root.path(), database_faults.clone()).unwrap());
    let binding = FujiLibraryService::new(database.clone(), app_root.path())
        .bind(library_root.path())
        .unwrap();
    let catalog = Arc::new(CatalogService::new(database.clone()));
    LibraryScanner::new(catalog.clone())
        .scan(ScanRequest {
            library_id: binding.library_id,
            root_id: binding.root_id,
            root_binding_id: binding.root_binding_id,
            root_path: library_root.path().to_path_buf(),
        })
        .unwrap();
    let photos = catalog
        .timeline(binding.library_id, SortMode::CaptureDesc)
        .unwrap();
    let pending = Arc::new(PendingDeleteService::new(database.clone()));
    for photo in photos.into_iter().filter(|photo| {
        photo.natural_name_display == "DSCF7001.JPG"
            || (include_second_photo && photo.natural_name_display == "DSCF7002.JPG")
    }) {
        pending
            .toggle(photo.photo_id, photo.photo_version, OperationId::new())
            .unwrap();
    }
    Fixture {
        _parent: parent,
        _app_root: app_root,
        database,
        pending,
        database_faults,
    }
}

#[cfg(windows)]
fn service(fixture: &Fixture, shell: Arc<FakeShell>) -> RecycleService {
    let safety_gate = Arc::new(RuntimeSafetyGate::new(fixture.database.clone()));
    safety_gate.mark_ready().unwrap();
    RecycleService::with_shell(
        fixture.database.clone(),
        fixture.pending.clone(),
        shell,
        safety_gate,
        FaultInjector::default(),
    )
}

#[cfg(windows)]
fn service_with_faults(
    fixture: &Fixture,
    shell: Arc<FakeShell>,
    safety_gate: Arc<RuntimeSafetyGate>,
    faults: FaultInjector,
) -> RecycleService {
    RecycleService::with_shell(
        fixture.database.clone(),
        fixture.pending.clone(),
        shell,
        safety_gate,
        faults,
    )
}

#[cfg(windows)]
fn count(database: &Database, table: &str) -> i64 {
    database
        .open_read_connection()
        .unwrap()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), (), |row| {
            row.get(0)
        })
        .unwrap()
}

#[cfg(windows)]
#[test]
fn successful_callback_commits_fact_before_finishing_and_clears_pending_entry() {
    let fixture = fixture();
    let shell = Arc::new(FakeShell::new([FakeOutcome::Recycled]));
    let service = service(&fixture, shell.clone());
    let confirmation = service.confirmation().unwrap();
    assert_eq!(confirmation.logical_photo_count, 1);
    assert_eq!(confirmation.physical_file_count, 1);

    let status = service
        .start(
            service.test_permit(),
            Uuid::new_v4(),
            confirmation.logical_photo_count,
            confirmation.physical_file_count,
            confirmation.total_bytes,
            confirmation.selection_digest.clone(),
        )
        .unwrap();

    assert_eq!(status.phase, RecyclePhase::Completed);
    assert_eq!(count(&fixture.database, "deletion_recycled_fact"), 1);
    assert_eq!(fixture.pending.summary().unwrap().total_count, 0);
    assert_eq!(shell.calls.load(Ordering::SeqCst), 1);
    assert_eq!(shell.photos.lock().unwrap().as_slice(), &[1]);
}

#[cfg(windows)]
#[test]
fn permit_from_another_gate_instance_cannot_start_recycle() {
    let fixture = fixture();
    let shell = Arc::new(FakeShell::new([FakeOutcome::Recycled]));
    let service_gate = Arc::new(RuntimeSafetyGate::new(fixture.database.clone()));
    service_gate.mark_ready().unwrap();
    let foreign_gate = Arc::new(RuntimeSafetyGate::new(fixture.database.clone()));
    foreign_gate.mark_ready().unwrap();
    let foreign_permit = foreign_gate.require_feature(FeatureGate::Recycle).unwrap();
    let service = service_with_faults(
        &fixture,
        shell.clone(),
        service_gate,
        FaultInjector::default(),
    );
    let confirmation = service.confirmation().unwrap();

    let error = service
        .start(
            foreign_permit,
            Uuid::new_v4(),
            confirmation.logical_photo_count,
            confirmation.physical_file_count,
            confirmation.total_bytes,
            confirmation.selection_digest.clone(),
        )
        .unwrap_err();

    assert_eq!(error.code, ErrorCode::CapabilityDenied);
    assert_eq!(count(&fixture.database, "deletion_batch"), 0);
    assert_eq!(shell.calls.load(Ordering::SeqCst), 0);
}

#[cfg(windows)]
#[test]
fn initial_transaction_fault_rolls_back_every_intent_and_never_calls_shell() {
    let fixture = fixture();
    let shell = Arc::new(FakeShell::new([FakeOutcome::Recycled]));
    let service = service(&fixture, shell.clone());
    let confirmation = service.confirmation().unwrap();
    fixture
        .database_faults
        .fail_on_hit(FaultPoint::SqliteBeforeCommit, 1)
        .unwrap();

    assert!(
        service
            .start(
                service.test_permit(),
                Uuid::new_v4(),
                1,
                1,
                confirmation.total_bytes,
                confirmation.selection_digest.clone(),
            )
            .is_err()
    );
    assert_eq!(count(&fixture.database, "deletion_batch"), 0);
    assert_eq!(count(&fixture.database, "deletion_attempt"), 0);
    assert_eq!(shell.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.pending.summary().unwrap().total_count, 1);
}

#[cfg(windows)]
#[test]
fn ordinary_failure_is_retryable_and_retry_creates_a_new_attempt() {
    let fixture = fixture();
    let shell = Arc::new(FakeShell::new([FakeOutcome::FailedInPlace]));
    let service = service(&fixture, shell.clone());
    let confirmation = service.confirmation().unwrap();
    let first = service
        .start(
            service.test_permit(),
            Uuid::new_v4(),
            1,
            1,
            confirmation.total_bytes,
            confirmation.selection_digest.clone(),
        )
        .unwrap();
    assert_eq!(first.phase, RecyclePhase::AwaitingUser);
    assert_eq!(first.retryable_files, 1);
    shell.push(FakeOutcome::Recycled);

    let retry = service.confirmation().unwrap();
    assert!(retry.retry);
    let final_status = service
        .retry(
            service.test_permit(),
            Uuid::new_v4(),
            1,
            1,
            retry.total_bytes,
            retry.selection_digest.clone(),
        )
        .unwrap();
    assert_eq!(final_status.phase, RecyclePhase::Completed);
    assert_eq!(count(&fixture.database, "deletion_attempt"), 2);
    assert_eq!(count(&fixture.database, "deletion_recycled_fact"), 1);
}

#[cfg(windows)]
#[test]
fn empty_recycle_object_and_predelete_replacement_stop_as_needs_attention() {
    for outcome in [
        FakeOutcome::EmptyRecycleObject,
        FakeOutcome::ReplaceBeforePre,
    ] {
        let fixture = fixture();
        let shell = Arc::new(FakeShell::new([outcome]));
        let service = service(&fixture, shell);
        let confirmation = service.confirmation().unwrap();
        service
            .start(
                service.test_permit(),
                Uuid::new_v4(),
                1,
                1,
                confirmation.total_bytes,
                confirmation.selection_digest.clone(),
            )
            .unwrap();
        let status = service.status().unwrap();
        assert_eq!(status.phase, RecyclePhase::NeedsAttention);
        assert_eq!(count(&fixture.database, "deletion_recycled_fact"), 0);
        assert_eq!(fixture.pending.summary().unwrap().total_count, 1);
    }
}

#[cfg(windows)]
#[test]
fn postdelete_database_failure_stops_and_preserves_unknown_result() {
    let fixture = fixture();
    let shell = Arc::new(FakeShell::new([FakeOutcome::Recycled]));
    let service = service(&fixture, shell);
    let confirmation = service.confirmation().unwrap();
    fixture
        .database_faults
        .fail_on_hit(FaultPoint::SqliteBeforeCommit, 3)
        .unwrap();

    assert!(
        service
            .start(
                service.test_permit(),
                Uuid::new_v4(),
                1,
                1,
                confirmation.total_bytes,
                confirmation.selection_digest.clone(),
            )
            .is_err()
    );
    let status = service.status().unwrap();
    assert_eq!(status.phase, RecyclePhase::Running);
    assert_eq!(count(&fixture.database, "deletion_recycled_fact"), 0);
    assert_eq!(count(&fixture.database, "operation_reservation"), 1);
}

#[cfg(windows)]
#[test]
fn interrupted_attempt_is_reconciled_without_automatic_shell_replay() {
    let fixture = fixture();
    let shell = Arc::new(FakeShell::new([]));
    let service = service(&fixture, shell.clone());
    let confirmation = service.confirmation().unwrap();
    service
        .create_initial_attempt(Uuid::new_v4(), confirmation)
        .unwrap();

    service.reconcile_interrupted().unwrap();

    assert_eq!(shell.calls.load(Ordering::SeqCst), 0);
    assert_eq!(service.status().unwrap().phase, RecyclePhase::AwaitingUser);
    assert_eq!(count(&fixture.database, "deletion_recycled_fact"), 0);
}

#[cfg(windows)]
#[test]
fn gate_closing_after_shell_does_not_reject_recycled_fact_commit() {
    let fixture = fixture();
    let shell = Arc::new(FakeShell::new([FakeOutcome::Recycled]));
    let gate = Arc::new(RuntimeSafetyGate::new(fixture.database.clone()));
    gate.mark_ready().unwrap();
    let faults = FaultInjector::default();
    faults
        .latch_gate_on_hit(
            FaultPoint::RecycleAfterPhysicalBeforeFact,
            1,
            gate.clone(),
            crate::safety::GateCloseReason::CapabilityBoundary,
        )
        .unwrap();
    let service = service_with_faults(&fixture, shell.clone(), gate.clone(), faults);
    let confirmation = service.confirmation().unwrap();

    let status = service
        .start(
            service.test_permit(),
            Uuid::new_v4(),
            confirmation.logical_photo_count,
            confirmation.physical_file_count,
            confirmation.total_bytes,
            confirmation.selection_digest.clone(),
        )
        .unwrap();

    assert_eq!(status.phase, RecyclePhase::Completed);
    assert_eq!(count(&fixture.database, "deletion_recycled_fact"), 1);
    assert_eq!(shell.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        gate.status().global_write_gate,
        crate::safety::GateState::Closed
    );
}

#[cfg(windows)]
#[test]
fn gate_closing_after_first_asset_stops_before_second_shell_action() {
    let fixture = fixture_with_pair(true);
    let shell = Arc::new(FakeShell::new([
        FakeOutcome::Recycled,
        FakeOutcome::Recycled,
    ]));
    let gate = Arc::new(RuntimeSafetyGate::new(fixture.database.clone()));
    gate.mark_ready().unwrap();
    let faults = FaultInjector::default();
    faults
        .latch_gate_on_hit(
            FaultPoint::RecycleAfterPhysicalBeforeFact,
            1,
            gate.clone(),
            crate::safety::GateCloseReason::CapabilityBoundary,
        )
        .unwrap();
    let service = service_with_faults(&fixture, shell.clone(), gate, faults);
    let confirmation = service.confirmation().unwrap();
    assert_eq!(confirmation.physical_file_count, 2);

    let error = service
        .start(
            service.test_permit(),
            Uuid::new_v4(),
            confirmation.logical_photo_count,
            confirmation.physical_file_count,
            confirmation.total_bytes,
            confirmation.selection_digest.clone(),
        )
        .unwrap_err();

    assert_eq!(error.code, ErrorCode::CapabilityDenied);
    assert_eq!(count(&fixture.database, "deletion_recycled_fact"), 1);
    assert_eq!(shell.calls.load(Ordering::SeqCst), 1);
    assert_eq!(shell.photos.lock().unwrap().as_slice(), &[1]);
}

#[cfg(windows)]
#[test]
fn switching_libraries_keeps_marks_but_rejects_old_confirmation() {
    let fixture = fixture();
    let shell = Arc::new(FakeShell::new([]));
    let service = service(&fixture, shell.clone());
    let old = service.confirmation().unwrap();
    let other = TestRoot::create_in(fixture._parent.path()).unwrap();
    fs::write(
        other.path().join("DSCF9000.JPG"),
        b"phase-seven-jpg-fixture",
    )
    .unwrap();
    let binding = FujiLibraryService::new(fixture.database.clone(), fixture._app_root.path())
        .bind(other.path())
        .unwrap();
    let catalog = Arc::new(CatalogService::new(fixture.database.clone()));
    LibraryScanner::new(catalog.clone())
        .scan(ScanRequest {
            library_id: binding.library_id,
            root_id: binding.root_id,
            root_binding_id: binding.root_binding_id,
            root_path: other.path().to_path_buf(),
        })
        .unwrap();
    assert_eq!(service.confirmation().unwrap().physical_file_count, 0);
    let photo = catalog
        .timeline(binding.library_id, SortMode::CaptureDesc)
        .unwrap()
        .remove(0);
    fixture
        .pending
        .toggle(photo.photo_id, photo.photo_version, OperationId::new())
        .unwrap();
    let current = service.confirmation().unwrap();
    assert_eq!(current.physical_file_count, 1);
    assert_eq!(fixture.pending.summary().unwrap().total_count, 1);
    assert_eq!(count(&fixture.database, "deletion_mark"), 2);
    assert_ne!(current.selection_digest, old.selection_digest);
    // Counts/bytes from the CURRENT set with an OLD authorization must still fail.
    let error = service
        .start(
            service.test_permit(),
            Uuid::new_v4(),
            current.logical_photo_count,
            current.physical_file_count,
            current.total_bytes,
            old.selection_digest,
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::StaleVersion);
    assert_eq!(shell.calls.load(Ordering::SeqCst), 0);
}
