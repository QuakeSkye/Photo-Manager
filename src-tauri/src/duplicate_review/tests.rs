use std::{fs, sync::Arc};

#[cfg(windows)]
use std::sync::Mutex;

use crate::{
    catalog::{CatalogService, LibraryScanner, ScanRequest},
    db::Database,
    io_scheduler::IoScheduler,
    pending_delete::{AddDuplicateSubsetRequest, DuplicateSubsetSelection, PendingDeleteService},
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
    fs::write(library_path.join("new").join("renamed.JPG"), &duplicate).unwrap();
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
