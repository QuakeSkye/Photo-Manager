use std::{fs, sync::Arc};

use crate::{
    catalog::{CatalogService, LibraryScanner, ScanRequest},
    db::Database,
    io_scheduler::IoScheduler,
    pending_delete::{AddDuplicateSubsetRequest, DuplicateSubsetSelection, PendingDeleteService},
    roots::FujiLibraryService,
    safety::TestRoot,
};

use super::{DuplicateReviewService, DuplicateReviewState};

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
