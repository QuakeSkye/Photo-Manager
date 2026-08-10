mod candidate_lookup;
mod coordinator;
mod pairing;
mod scanner;
mod timeline;

use std::sync::Arc;

use rusqlite::{OptionalExtension, Transaction};

use crate::{
    db::{
        Database,
        migration::{map_sqlite_error, now_micros},
    },
    domain::{
        AssetId, DomainError, ErrorCode, LibraryId, PhotoId, RootBindingId, RootId, ScanRunId,
    },
    media::{CaptureMetadata, MEDIA_PARSER_VERSION, PreviewState},
};

pub(crate) use candidate_lookup::find_candidates as find_import_candidates;
pub use candidate_lookup::{CandidateLookup, CandidateLookupInput, ImportCandidate};
pub use coordinator::{BackgroundScan, ScanCoordinator, ScanKind, ScanPhase, ScanStatus};
pub use scanner::{LibraryScanner, ScanIssue, ScanProgress, ScanReport, ScanRequest};
pub use timeline::{SortMode, TimelinePhoto, ViewingCursor};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaKind {
    Jpg,
    Raf,
}

impl MediaKind {
    pub const fn as_sql(self) -> &'static str {
        match self {
            Self::Jpg => "Jpg",
            Self::Raf => "Raf",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetObservation {
    pub library_id: LibraryId,
    pub root_id: RootId,
    pub root_binding_id: RootBindingId,
    pub media_kind: MediaKind,
    pub relative_path: String,
    pub path_key: Vec<u8>,
    pub file_id: Option<Vec<u8>>,
    pub size_bytes: i64,
    pub mtime_filetime: i64,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogObservation {
    pub asset: AssetObservation,
    pub hard_link_count: i64,
    pub deletion_supported: bool,
    pub scan_run_id: Option<ScanRunId>,
    pub capture_local_key: Option<String>,
    pub camera_model: Option<String>,
    pub camera_serial: Option<String>,
    pub capture_metadata: CaptureMetadata,
    pub preview: PreviewState,
}

impl From<AssetObservation> for CatalogObservation {
    fn from(asset: AssetObservation) -> Self {
        let preview = match asset.media_kind {
            MediaKind::Jpg => PreviewState::JpegSource,
            MediaKind::Raf => PreviewState::Unavailable,
        };
        Self {
            asset,
            hard_link_count: 1,
            deletion_supported: true,
            scan_run_id: None,
            capture_local_key: None,
            camera_model: None,
            camera_serial: None,
            capture_metadata: CaptureMetadata::default(),
            preview,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObserveAssetResult {
    pub asset_id: AssetId,
    pub asset_version: i64,
    pub photo_id: PhotoId,
    pub photo_version: i64,
    pub changed: bool,
    pub capture_metadata_inserted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationOutcome {
    Observed(ObserveAssetResult),
    DeferredByReservation,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanCompletion {
    pub observed_assets: usize,
    pub missing_assets: usize,
    pub ignored_files: usize,
    pub skipped_reparse: usize,
    pub errors: usize,
}

#[derive(Debug, Clone)]
pub struct CatalogService {
    database: Arc<Database>,
}

impl CatalogService {
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }

    pub fn observe_asset(
        &self,
        observation: AssetObservation,
    ) -> Result<ObserveAssetResult, DomainError> {
        self.observe_media_asset(observation.into())
    }

    pub fn observe_media_asset(
        &self,
        observation: CatalogObservation,
    ) -> Result<ObserveAssetResult, DomainError> {
        match self.observe_media_asset_outcome(observation)? {
            ObservationOutcome::Observed(result) => Ok(result),
            ObservationOutcome::DeferredByReservation => Err(DomainError::new(
                ErrorCode::ReservationConflict,
                "文件正由活动任务保留，扫描已延迟协调。",
            )),
        }
    }

    pub fn observe_media_asset_outcome(
        &self,
        observation: CatalogObservation,
    ) -> Result<ObservationOutcome, DomainError> {
        validate_observation(&observation)?;
        self.database.writer()?.transaction(move |transaction| {
            if is_reserved(transaction, &observation)? {
                Ok(ObservationOutcome::DeferredByReservation)
            } else {
                observe_in_transaction(transaction, &observation).map(ObservationOutcome::Observed)
            }
        })
    }

    pub fn observe_import_reserved_asset(
        &self,
        observation: CatalogObservation,
        task_id: &[u8],
        attempt_id: &[u8],
        lease_generation: i64,
    ) -> Result<ObserveAssetResult, DomainError> {
        validate_observation(&observation)?;
        let task_id = task_id.to_vec();
        let attempt_id = attempt_id.to_vec();
        self.database.writer()?.transaction(move |transaction| {
            let authorized: bool = transaction
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM operation_reservation AS reservation
                        JOIN import_attempt AS attempt ON attempt.attempt_id=reservation.attempt_id
                        JOIN file_write_lease AS lease ON lease.slot=1
                        WHERE reservation.owner_kind='TaskAttempt'
                          AND reservation.task_id=?1 AND reservation.attempt_id=?2
                          AND reservation.lease_generation=?3
                          AND reservation.reservation_kind='FinalPath'
                          AND reservation.root_id=?4 AND reservation.path_key=?5
                          AND reservation.released_at IS NULL
                          AND attempt.attempt_state='Running'
                          AND lease.task_id=?1 AND lease.lease_generation=?3
                     )",
                    rusqlite::params![
                        &task_id,
                        &attempt_id,
                        lease_generation,
                        observation.asset.root_id.as_bytes().as_slice(),
                        &observation.asset.path_key,
                    ],
                    |row| row.get(0),
                )
                .map_err(map_sqlite_error)?;
            if !authorized {
                return Err(DomainError::new(
                    ErrorCode::LeaseGenerationMismatch,
                    "导入索引授权已失效。",
                ));
            }
            observe_in_transaction(transaction, &observation)
        })
    }

    pub fn begin_scan(
        &self,
        library_id: LibraryId,
        root_id: RootId,
        root_binding_id: RootBindingId,
    ) -> Result<ScanRunId, DomainError> {
        let scan_run_id = ScanRunId::new();
        self.database.writer()?.transaction(move |transaction| {
            let valid: bool = transaction
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM root
                        JOIN root_binding ON root_binding.binding_id=root.current_binding_id
                        WHERE root.root_id=?1 AND root.library_id=?2
                          AND root.lifecycle='Online' AND root_binding.binding_id=?3
                          AND root_binding.binding_state='Active'
                     )",
                    (
                        root_id.as_bytes().as_slice(),
                        library_id.as_bytes().as_slice(),
                        root_binding_id.as_bytes().as_slice(),
                    ),
                    |row| row.get(0),
                )
                .map_err(map_sqlite_error)?;
            if !valid {
                return Err(DomainError::new(
                    ErrorCode::IdentityMismatch,
                    "扫描开始前图库 Root 身份已变化。",
                ));
            }
            let now = now_micros();
            candidate_lookup::ensure_catalog_revision(transaction, library_id, root_binding_id)?;
            transaction
                .execute(
                    "INSERT INTO scan_run(
                        scan_run_id, library_id, root_id, root_binding_id, run_state, started_at
                     ) VALUES (?1, ?2, ?3, ?4, 'Running', ?5)",
                    (
                        scan_run_id.as_bytes().as_slice(),
                        library_id.as_bytes().as_slice(),
                        root_id.as_bytes().as_slice(),
                        root_binding_id.as_bytes().as_slice(),
                        now,
                    ),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO operation_report(
                        report_id, report_kind, library_id, root_id, scan_run_id,
                        report_state, started_at
                     ) VALUES (?1, 'Scan', ?2, ?3, ?1, 'Open', ?4)",
                    (
                        scan_run_id.as_bytes().as_slice(),
                        library_id.as_bytes().as_slice(),
                        root_id.as_bytes().as_slice(),
                        now,
                    ),
                )
                .map_err(map_sqlite_error)?;
            Ok(scan_run_id)
        })
    }

    pub fn complete_scan(
        &self,
        scan_run_id: ScanRunId,
        observed_assets: &[AssetId],
        ignored_files: usize,
        skipped_reparse: usize,
        errors: usize,
    ) -> Result<ScanCompletion, DomainError> {
        self.complete_scan_with_deferred(
            scan_run_id,
            observed_assets,
            &[],
            ignored_files,
            skipped_reparse,
            errors,
        )
    }

    pub fn complete_scan_with_deferred(
        &self,
        scan_run_id: ScanRunId,
        observed_assets: &[AssetId],
        deferred_path_keys: &[Vec<u8>],
        ignored_files: usize,
        skipped_reparse: usize,
        errors: usize,
    ) -> Result<ScanCompletion, DomainError> {
        let observed = observed_assets.to_vec();
        let deferred = deferred_path_keys.to_vec();
        self.database.writer()?.transaction(move |transaction| {
            let (library_bytes, root_bytes, binding_bytes): (Vec<u8>, Vec<u8>, Vec<u8>) =
                transaction
                    .query_row(
                        "SELECT library_id, root_id, root_binding_id FROM scan_run
                     WHERE scan_run_id=?1 AND run_state='Running'",
                        [scan_run_id.as_bytes().as_slice()],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(map_sqlite_error)?;
            let library_id = library_id_from(&library_bytes)?;
            let root_id = root_id_from(&root_bytes)?;
            let binding_id = binding_id_from(&binding_bytes)?;

            transaction
                .execute_batch(
                    "CREATE TEMP TABLE IF NOT EXISTS phase_one_scan_seen(
                    asset_id BLOB PRIMARY KEY
                 ) WITHOUT ROWID;
                 DELETE FROM phase_one_scan_seen;",
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute_batch(
                    "CREATE TEMP TABLE IF NOT EXISTS phase_one_scan_deferred(
                    path_key BLOB PRIMARY KEY
                 ) WITHOUT ROWID;
                 DELETE FROM phase_one_scan_deferred;",
                )
                .map_err(map_sqlite_error)?;
            for asset_id in &observed {
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO phase_one_scan_seen(asset_id) VALUES (?1)",
                        [asset_id.as_bytes().as_slice()],
                    )
                    .map_err(map_sqlite_error)?;
            }
            for key in &deferred {
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO phase_one_scan_deferred(path_key) VALUES (?1)",
                        [key],
                    )
                    .map_err(map_sqlite_error)?;
            }
            let missing = mark_missing_assets(transaction, root_id, binding_id, scan_run_id)?;
            pairing::reconcile_library(transaction, library_id)?;
            let now = now_micros();
            transaction
                .execute(
                    "UPDATE scan_run SET run_state='Completed', completed_at=?1,
                            observed_assets=?2, ignored_files=?3, skipped_reparse=?4, error_count=?5
                     WHERE scan_run_id=?6 AND run_state='Running'",
                    rusqlite::params![
                        now,
                        to_i64(observed.len())?,
                        to_i64(ignored_files)?,
                        to_i64(skipped_reparse)?,
                        to_i64(errors)?,
                        scan_run_id.as_bytes().as_slice(),
                    ],
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE operation_report SET report_state='Finalized', completed_at=?1,
                            result=?2, item_count=?3
                     WHERE scan_run_id=?4 AND report_state='Open'",
                    (
                        now,
                        if errors == 0 { "Success" } else { "Partial" },
                        to_i64(observed.len())?,
                        scan_run_id.as_bytes().as_slice(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            Ok(ScanCompletion {
                observed_assets: observed.len(),
                missing_assets: missing,
                ignored_files,
                skipped_reparse,
                errors,
            })
        })
    }

    pub fn fail_scan(&self, scan_run_id: ScanRunId) -> Result<(), DomainError> {
        self.database.writer()?.transaction(move |transaction| {
            let now = now_micros();
            transaction
                .execute(
                    "UPDATE scan_run SET run_state='Failed', completed_at=?1, error_count=error_count+1
                     WHERE scan_run_id=?2 AND run_state='Running'",
                    (now, scan_run_id.as_bytes().as_slice()),
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE operation_report SET report_state='Finalized', completed_at=?1,
                            result='Failed'
                     WHERE scan_run_id=?2 AND report_state='Open'",
                    (now, scan_run_id.as_bytes().as_slice()),
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })
    }

    pub fn interrupt_scan(
        &self,
        scan_run_id: ScanRunId,
        observed_assets: usize,
        ignored_files: usize,
        skipped_reparse: usize,
        errors: usize,
    ) -> Result<ScanCompletion, DomainError> {
        self.database.writer()?.transaction(move |transaction| {
            let now = now_micros();
            transaction
                .execute(
                    "UPDATE scan_run SET run_state='Interrupted', completed_at=?1,
                            observed_assets=?2, ignored_files=?3, skipped_reparse=?4,
                            error_count=?5
                     WHERE scan_run_id=?6 AND run_state='Running'",
                    rusqlite::params![
                        now,
                        to_i64(observed_assets)?,
                        to_i64(ignored_files)?,
                        to_i64(skipped_reparse)?,
                        to_i64(errors)?,
                        scan_run_id.as_bytes().as_slice(),
                    ],
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE operation_report SET report_state='Finalized', completed_at=?1,
                            result='Cancelled', item_count=?2
                     WHERE scan_run_id=?3 AND report_state='Open'",
                    (
                        now,
                        to_i64(observed_assets)?,
                        scan_run_id.as_bytes().as_slice(),
                    ),
                )
                .map_err(map_sqlite_error)?;
            Ok(ScanCompletion {
                observed_assets,
                missing_assets: 0,
                ignored_files,
                skipped_reparse,
                errors,
            })
        })
    }

    pub fn recover_interrupted_scans(&self) -> Result<(), DomainError> {
        self.database.writer()?.transaction(|transaction| {
            let now = now_micros();
            transaction
                .execute(
                    "UPDATE scan_run SET run_state='Interrupted', completed_at=?1
                     WHERE run_state='Running'",
                    [now],
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE operation_report SET report_state='Finalized', completed_at=?1,
                            result='Cancelled'
                     WHERE report_kind='Scan' AND report_state='Open'",
                    [now],
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })
    }

    pub fn reconcile_library(&self, library_id: LibraryId) -> Result<(), DomainError> {
        self.database
            .writer()?
            .transaction(move |transaction| pairing::reconcile_library(transaction, library_id))
    }

    pub fn reconcile_import_groups(&self, library_id: LibraryId) -> Result<(), DomainError> {
        self.database.writer()?.transaction(move |transaction| {
            pairing::reconcile_import_groups(transaction, library_id)
        })
    }

    pub fn timeline(
        &self,
        library_id: LibraryId,
        sort_mode: SortMode,
    ) -> Result<Vec<TimelinePhoto>, DomainError> {
        timeline::load_timeline(&self.database, library_id, sort_mode)
    }

    pub fn save_viewing_cursor(
        &self,
        library_id: LibraryId,
        sort_mode: SortMode,
        photo_id: PhotoId,
    ) -> Result<ViewingCursor, DomainError> {
        timeline::save_cursor(&self.database, library_id, sort_mode, photo_id)
    }

    pub fn restore_viewing_cursor(
        &self,
        library_id: LibraryId,
        sort_mode: SortMode,
    ) -> Result<Option<PhotoId>, DomainError> {
        timeline::restore_cursor(&self.database, library_id, sort_mode)
    }

    pub fn mark_root_unavailable(
        &self,
        root_id: RootId,
        identity_mismatch: bool,
    ) -> Result<(), DomainError> {
        self.database.writer()?.transaction(move |transaction| {
            let state = if identity_mismatch {
                "IdentityMismatch"
            } else {
                "Offline"
            };
            transaction
                .execute(
                    "UPDATE root SET lifecycle=?1, root_version=root_version+1
                     WHERE root_id=?2 AND lifecycle='Online'",
                    (state, root_id.as_bytes().as_slice()),
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })
    }
}

fn is_reserved(
    transaction: &Transaction<'_>,
    observation: &CatalogObservation,
) -> Result<bool, DomainError> {
    transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM operation_reservation
                WHERE released_at IS NULL AND root_id=?1
                  AND ((path_key IS NOT NULL AND path_key=?2)
                    OR (file_id IS NOT NULL AND file_id IS ?3))
             )",
            (
                observation.asset.root_id.as_bytes().as_slice(),
                &observation.asset.path_key,
                &observation.asset.file_id,
            ),
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)
}

fn observe_in_transaction(
    transaction: &Transaction<'_>,
    observation: &CatalogObservation,
) -> Result<ObserveAssetResult, DomainError> {
    let by_path = find_asset_by_path(transaction, observation)?;
    let by_file = find_asset_by_file(transaction, observation)?;
    if by_path.is_some() && by_file.is_some() && by_path != by_file {
        return Err(DomainError::new(
            ErrorCode::ReservationConflict,
            "同一路径与文件身份指向不同对象，已停止建档。",
        ));
    }

    let mut candidate = by_file.clone().or(by_path.clone());
    if observation.hard_link_count > 1 {
        if let Some(file_asset) = &by_file {
            let (current_key, current_relative): (Vec<u8>, String) = transaction
                .query_row(
                    "SELECT revision.path_key, revision.relative_path
                     FROM asset JOIN asset_revision revision
                       ON revision.asset_id=asset.asset_id AND revision.asset_version=asset.current_version
                     WHERE asset.asset_id=?1",
                    [file_asset],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(map_sqlite_error)?;
            if current_key < observation.asset.path_key {
                let mut canonical = observation.clone();
                canonical.asset.path_key = current_key;
                canonical.asset.relative_path = current_relative;
                return update_existing(transaction, &canonical, file_asset.clone());
            }
        }
    }
    if by_file.is_none() {
        if let (Some(path_asset), Some(observed_file_id)) = (&by_path, &observation.asset.file_id) {
            let old_file_id = transaction
                .query_row(
                    "SELECT current_file_id FROM asset WHERE asset_id=?1",
                    [path_asset],
                    |row| row.get::<_, Option<Vec<u8>>>(0),
                )
                .map_err(map_sqlite_error)?;
            if old_file_id
                .as_ref()
                .is_some_and(|old| old != observed_file_id)
            {
                retire_replaced_asset(transaction, path_asset, observation)?;
                candidate = None;
            }
        }
    }

    match candidate {
        Some(bytes) => update_existing(transaction, observation, bytes),
        None => insert_new(transaction, observation),
    }
}

fn find_asset_by_path(
    transaction: &Transaction<'_>,
    observation: &CatalogObservation,
) -> Result<Option<Vec<u8>>, DomainError> {
    transaction
        .query_row(
            "SELECT asset_id FROM asset
             WHERE root_id=?1 AND current_path_key=?2 AND presence_state!='Tombstoned'",
            (
                observation.asset.root_id.as_bytes().as_slice(),
                &observation.asset.path_key,
            ),
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite_error)
}

fn find_asset_by_file(
    transaction: &Transaction<'_>,
    observation: &CatalogObservation,
) -> Result<Option<Vec<u8>>, DomainError> {
    match &observation.asset.file_id {
        Some(file_id) => transaction
            .query_row(
                "SELECT asset_id FROM asset
                 WHERE root_id=?1 AND current_file_id=?2 AND presence_state!='Tombstoned'",
                (observation.asset.root_id.as_bytes().as_slice(), file_id),
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite_error),
        None => Ok(None),
    }
}

fn update_existing(
    transaction: &Transaction<'_>,
    observation: &CatalogObservation,
    id_bytes: Vec<u8>,
) -> Result<ObserveAssetResult, DomainError> {
    let current = transaction
        .query_row(
            "SELECT a.current_version, a.row_version, a.deletion_supported,
                    r.root_binding_id, r.path_key, r.file_id, r.size_bytes,
                    r.mtime_filetime, r.presence_state, r.hard_link_count
             FROM asset a JOIN asset_revision r
               ON r.asset_id=a.asset_id AND r.asset_version=a.current_version
             WHERE a.asset_id=?1",
            [&id_bytes],
            |row| {
                Ok(CurrentAsset {
                    version: row.get(0)?,
                    row_version: row.get(1)?,
                    deletion_supported: row.get::<_, i64>(2)? == 1,
                    binding_id: row.get(3)?,
                    path_key: row.get(4)?,
                    file_id: row.get(5)?,
                    size_bytes: row.get(6)?,
                    mtime_filetime: row.get(7)?,
                    presence_state: row.get(8)?,
                    hard_link_count: row.get(9)?,
                })
            },
        )
        .map_err(map_sqlite_error)?;
    let asset_id = asset_id_from(&id_bytes)?;
    let current_photo = current_photo_for_asset(transaction, &id_bytes)?;
    let unchanged = current.binding_id == observation.asset.root_binding_id.as_bytes().as_slice()
        && current.path_key == observation.asset.path_key
        && current.file_id == observation.asset.file_id
        && current.size_bytes == observation.asset.size_bytes
        && current.mtime_filetime == observation.asset.mtime_filetime
        && current.presence_state == "Present"
        && current.hard_link_count == observation.hard_link_count
        && current.deletion_supported == observation.deletion_supported;
    if unchanged {
        ensure_metadata(transaction, &id_bytes, current.version, observation)?;
        let capture_metadata_inserted =
            ensure_capture_metadata(transaction, &id_bytes, current.version, observation)?;
        candidate_lookup::ensure_candidate_key(
            transaction,
            &id_bytes,
            current.version,
            observation,
        )?;
        let (photo_id, photo_version) = match current_photo {
            Some(value) => value,
            None => create_photo_for_asset(
                transaction,
                observation.asset.library_id,
                &id_bytes,
                observation,
            )?,
        };
        return Ok(ObserveAssetResult {
            asset_id,
            asset_version: current.version,
            photo_id,
            photo_version,
            changed: false,
            capture_metadata_inserted,
        });
    }

    let next_version = checked_next(current.version, "Asset")?;
    insert_revision(transaction, &id_bytes, next_version, observation, "Present")?;
    let changed = transaction
        .execute(
            "UPDATE asset SET current_version=?1, current_path_key=?2, current_file_id=?3,
                    presence_state='Present', deletion_supported=?4, row_version=row_version+1
             WHERE asset_id=?5 AND current_version=?6 AND row_version=?7",
            rusqlite::params![
                next_version,
                &observation.asset.path_key,
                &observation.asset.file_id,
                i64::from(observation.deletion_supported),
                &id_bytes,
                current.version,
                current.row_version,
            ],
        )
        .map_err(map_sqlite_error)?;
    require_one(changed, "Asset")?;
    insert_metadata(transaction, &id_bytes, next_version, observation)?;
    let capture_metadata_inserted =
        ensure_capture_metadata(transaction, &id_bytes, next_version, observation)?;
    candidate_lookup::ensure_candidate_key(transaction, &id_bytes, next_version, observation)?;
    let (photo_id, photo_version) = match current_photo {
        Some((photo_id, version)) => {
            let next_photo_version = bump_photo(transaction, photo_id, version)?;
            sync_photo_sorting(transaction, photo_id)?;
            (photo_id, next_photo_version)
        }
        None => create_photo_for_asset(
            transaction,
            observation.asset.library_id,
            &id_bytes,
            observation,
        )?,
    };
    candidate_lookup::bump_catalog_revision(
        transaction,
        observation.asset.library_id,
        observation.asset.root_binding_id,
    )?;
    Ok(ObserveAssetResult {
        asset_id,
        asset_version: next_version,
        photo_id,
        photo_version,
        changed: true,
        capture_metadata_inserted,
    })
}

fn insert_new(
    transaction: &Transaction<'_>,
    observation: &CatalogObservation,
) -> Result<ObserveAssetResult, DomainError> {
    let asset_id = AssetId::new();
    transaction
        .execute(
            "INSERT INTO asset(
                asset_id, library_id, root_id, media_kind, current_version,
                current_path_key, current_file_id, presence_state,
                deletion_supported, created_at
             ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, 'Present', ?7, ?8)",
            rusqlite::params![
                asset_id.as_bytes().as_slice(),
                observation.asset.library_id.as_bytes().as_slice(),
                observation.asset.root_id.as_bytes().as_slice(),
                observation.asset.media_kind.as_sql(),
                &observation.asset.path_key,
                &observation.asset.file_id,
                i64::from(observation.deletion_supported),
                observation.asset.observed_at,
            ],
        )
        .map_err(map_sqlite_error)?;
    insert_revision(transaction, asset_id.as_bytes(), 1, observation, "Present")?;
    insert_metadata(transaction, asset_id.as_bytes(), 1, observation)?;
    let capture_metadata_inserted =
        ensure_capture_metadata(transaction, asset_id.as_bytes(), 1, observation)?;
    candidate_lookup::ensure_candidate_key(transaction, asset_id.as_bytes(), 1, observation)?;
    let (photo_id, photo_version) = create_photo_for_asset(
        transaction,
        observation.asset.library_id,
        asset_id.as_bytes(),
        observation,
    )?;
    candidate_lookup::bump_catalog_revision(
        transaction,
        observation.asset.library_id,
        observation.asset.root_binding_id,
    )?;
    Ok(ObserveAssetResult {
        asset_id,
        asset_version: 1,
        photo_id,
        photo_version,
        changed: true,
        capture_metadata_inserted,
    })
}

fn insert_revision(
    transaction: &Transaction<'_>,
    asset_id: &[u8],
    version: i64,
    observation: &CatalogObservation,
    presence: &str,
) -> Result<(), DomainError> {
    transaction
        .execute(
            "INSERT INTO asset_revision(
                asset_id, asset_version, root_id, root_binding_id, relative_path,
                path_key, path_key_version, file_id, size_bytes, mtime_filetime,
                presence_state, hard_link_count, observed_at, scan_run_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                asset_id,
                version,
                observation.asset.root_id.as_bytes().as_slice(),
                observation.asset.root_binding_id.as_bytes().as_slice(),
                &observation.asset.relative_path,
                &observation.asset.path_key,
                &observation.asset.file_id,
                observation.asset.size_bytes,
                observation.asset.mtime_filetime,
                presence,
                observation.hard_link_count,
                observation.asset.observed_at,
                observation
                    .scan_run_id
                    .map(|value| value.as_bytes().to_vec()),
            ],
        )
        .map_err(map_sqlite_error)?;
    Ok(())
}

fn insert_metadata(
    transaction: &Transaction<'_>,
    asset_id: &[u8],
    version: i64,
    observation: &CatalogObservation,
) -> Result<(), DomainError> {
    transaction
        .execute(
            "INSERT INTO asset_media_metadata(
                asset_id, asset_version, capture_local_key, camera_model, camera_serial,
                preview_state, preview_offset, preview_length, parser_version, observed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                asset_id,
                version,
                &observation.capture_local_key,
                &observation.camera_model,
                &observation.camera_serial,
                observation.preview.as_sql(),
                observation.preview.offset(),
                observation.preview.length(),
                MEDIA_PARSER_VERSION,
                observation.asset.observed_at,
            ],
        )
        .map_err(map_sqlite_error)?;
    Ok(())
}

fn ensure_metadata(
    transaction: &Transaction<'_>,
    asset_id: &[u8],
    version: i64,
    observation: &CatalogObservation,
) -> Result<(), DomainError> {
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM asset_media_metadata WHERE asset_id=?1 AND asset_version=?2)",
            (asset_id, version),
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    if !exists {
        insert_metadata(transaction, asset_id, version, observation)?;
    }
    Ok(())
}

fn ensure_capture_metadata(
    transaction: &Transaction<'_>,
    asset_id: &[u8],
    version: i64,
    observation: &CatalogObservation,
) -> Result<bool, DomainError> {
    let capture = &observation.capture_metadata;
    let inserted = transaction
        .execute(
            "INSERT OR IGNORE INTO asset_capture_metadata(
                asset_id, asset_version, parser_version, lens_model,
                f_number_numerator, f_number_denominator,
                exposure_time_numerator, exposure_time_denominator,
                iso_value, focal_length_numerator, focal_length_denominator,
                pixel_width, pixel_height, film_simulation_key, film_simulation_code
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            rusqlite::params![
                asset_id,
                version,
                MEDIA_PARSER_VERSION,
                &capture.lens_model,
                capture.f_number.map(|value| value.numerator),
                capture.f_number.map(|value| value.denominator),
                capture.exposure_time.map(|value| value.numerator),
                capture.exposure_time.map(|value| value.denominator),
                capture.iso.map(i64::from),
                capture.focal_length.map(|value| value.numerator),
                capture.focal_length.map(|value| value.denominator),
                capture.pixel_width.map(i64::from),
                capture.pixel_height.map(i64::from),
                capture
                    .film_simulation
                    .as_ref()
                    .map(|value| value.key.as_str()),
                capture
                    .film_simulation
                    .as_ref()
                    .map(|value| i64::from(value.code)),
            ],
        )
        .map_err(map_sqlite_error)?;
    Ok(inserted == 1)
}

fn create_photo_for_asset(
    transaction: &Transaction<'_>,
    library_id: LibraryId,
    asset_id: &[u8],
    observation: &CatalogObservation,
) -> Result<(PhotoId, i64), DomainError> {
    let photo_id = PhotoId::new();
    let display = file_name_display(&observation.asset.relative_path);
    let sort_key = natural_sort_key(&display);
    transaction
        .execute(
            "INSERT INTO photo(
                photo_id, library_id, photo_version, lifecycle, date_class,
                capture_local_key, natural_name_display, natural_sort_key,
                sort_key_version, created_at
             ) VALUES (?1, ?2, 1, 'Active', ?3, ?4, ?5, ?6, 1, ?7)",
            (
                photo_id.as_bytes().as_slice(),
                library_id.as_bytes().as_slice(),
                if observation.capture_local_key.is_some() {
                    "Known"
                } else {
                    "Unknown"
                },
                &observation.capture_local_key,
                &display,
                &sort_key,
                observation.asset.observed_at,
            ),
        )
        .map_err(map_sqlite_error)?;
    transaction
        .execute(
            "INSERT INTO photo_asset(photo_asset_id, photo_id, asset_id, role, from_photo_version)
             VALUES (?1, ?2, ?3, ?4, 1)",
            (
                uuid::Uuid::new_v4().as_bytes().as_slice(),
                photo_id.as_bytes().as_slice(),
                asset_id,
                observation.asset.media_kind.as_sql(),
            ),
        )
        .map_err(map_sqlite_error)?;
    Ok((photo_id, 1))
}

fn current_photo_for_asset(
    transaction: &Transaction<'_>,
    asset_id: &[u8],
) -> Result<Option<(PhotoId, i64)>, DomainError> {
    transaction
        .query_row(
            "SELECT p.photo_id, p.photo_version FROM photo p
             JOIN photo_asset pa ON pa.photo_id=p.photo_id
             WHERE pa.asset_id=?1 AND pa.to_photo_version IS NULL AND p.lifecycle='Active'",
            [asset_id],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?
        .map(|(bytes, version)| Ok((photo_id_from(&bytes)?, version)))
        .transpose()
}

fn retire_replaced_asset(
    transaction: &Transaction<'_>,
    asset_id: &[u8],
    observation: &CatalogObservation,
) -> Result<(), DomainError> {
    let (version, row_version): (i64, i64) = transaction
        .query_row(
            "SELECT current_version, row_version FROM asset WHERE asset_id=?1",
            [asset_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_sqlite_error)?;
    let current = transaction
        .query_row(
            "SELECT root_id, root_binding_id, relative_path, path_key, file_id,
                    size_bytes, mtime_filetime, hard_link_count
             FROM asset_revision WHERE asset_id=?1 AND asset_version=?2",
            (asset_id, version),
            |row| {
                Ok(RetiredSnapshot {
                    root_id: row.get(0)?,
                    binding_id: row.get(1)?,
                    relative_path: row.get(2)?,
                    path_key: row.get(3)?,
                    file_id: row.get(4)?,
                    size_bytes: row.get(5)?,
                    mtime_filetime: row.get(6)?,
                    hard_link_count: row.get(7)?,
                })
            },
        )
        .map_err(map_sqlite_error)?;
    let next = checked_next(version, "Asset")?;
    transaction
        .execute(
            "INSERT INTO asset_revision(
                asset_id, asset_version, root_id, root_binding_id, relative_path,
                path_key, path_key_version, file_id, size_bytes, mtime_filetime,
                presence_state, hard_link_count, observed_at, scan_run_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8, ?9, 'Tombstoned', ?10, ?11, ?12)",
            rusqlite::params![
                asset_id,
                next,
                current.root_id,
                current.binding_id,
                current.relative_path,
                current.path_key,
                current.file_id,
                current.size_bytes,
                current.mtime_filetime,
                current.hard_link_count,
                observation.asset.observed_at,
                observation
                    .scan_run_id
                    .map(|value| value.as_bytes().to_vec())
            ],
        )
        .map_err(map_sqlite_error)?;
    transaction
        .execute(
            "UPDATE asset SET current_version=?1, presence_state='Tombstoned', tombstoned_at=?2,
                    row_version=row_version+1
             WHERE asset_id=?3 AND current_version=?4 AND row_version=?5",
            (
                next,
                observation.asset.observed_at,
                asset_id,
                version,
                row_version,
            ),
        )
        .map_err(map_sqlite_error)?;
    if let Some((photo_id, photo_version)) = current_photo_for_asset(transaction, asset_id)? {
        let next_photo = checked_next(photo_version, "Photo")?;
        transaction
            .execute(
                "UPDATE photo_asset SET to_photo_version=?1
                 WHERE photo_id=?2 AND asset_id=?3 AND to_photo_version IS NULL",
                (next_photo, photo_id.as_bytes().as_slice(), asset_id),
            )
            .map_err(map_sqlite_error)?;
        let remaining: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM photo_asset WHERE photo_id=?1 AND to_photo_version IS NULL",
                [photo_id.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .map_err(map_sqlite_error)?;
        if remaining == 0 {
            transaction
                .execute(
                    "UPDATE photo SET photo_version=?1, lifecycle='Tombstoned', tombstoned_at=?2,
                            row_version=row_version+1
                     WHERE photo_id=?3 AND photo_version=?4",
                    (
                        next_photo,
                        observation.asset.observed_at,
                        photo_id.as_bytes().as_slice(),
                        photo_version,
                    ),
                )
                .map_err(map_sqlite_error)?;
        } else {
            bump_photo(transaction, photo_id, photo_version)?;
            sync_photo_sorting(transaction, photo_id)?;
        }
    }
    Ok(())
}

fn mark_missing_assets(
    transaction: &Transaction<'_>,
    root_id: RootId,
    binding_id: RootBindingId,
    scan_run_id: ScanRunId,
) -> Result<usize, DomainError> {
    let mut statement = transaction
        .prepare(
            "SELECT a.asset_id, a.current_version, a.row_version, r.relative_path,
                    r.path_key, r.file_id, r.size_bytes, r.mtime_filetime, r.hard_link_count
             FROM asset a JOIN asset_revision r
               ON r.asset_id=a.asset_id AND r.asset_version=a.current_version
             WHERE a.root_id=?1 AND a.presence_state='Present'
               AND NOT EXISTS (SELECT 1 FROM phase_one_scan_seen seen WHERE seen.asset_id=a.asset_id)
               AND NOT EXISTS (
                 SELECT 1 FROM phase_one_scan_deferred deferred
                 WHERE deferred.path_key=a.current_path_key
               )",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map([root_id.as_bytes().as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, Option<Vec<u8>>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })
        .map_err(map_sqlite_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_sqlite_error)?;
    drop(statement);
    let now = now_micros();
    for (asset, version, row_version, relative, key, file, size, mtime, links) in &rows {
        let next = checked_next(*version, "Asset")?;
        transaction
            .execute(
                "INSERT INTO asset_revision(
                    asset_id, asset_version, root_id, root_binding_id, relative_path,
                    path_key, path_key_version, file_id, size_bytes, mtime_filetime,
                    presence_state, hard_link_count, observed_at, scan_run_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8, ?9, 'Missing', ?10, ?11, ?12)",
                rusqlite::params![
                    asset,
                    next,
                    root_id.as_bytes().as_slice(),
                    binding_id.as_bytes().as_slice(),
                    relative,
                    key,
                    file,
                    size,
                    mtime,
                    links,
                    now,
                    scan_run_id.as_bytes().as_slice()
                ],
            )
            .map_err(map_sqlite_error)?;
        require_one(
            transaction
                .execute(
                    "UPDATE asset SET current_version=?1, presence_state='Missing', row_version=row_version+1
                     WHERE asset_id=?2 AND current_version=?3 AND row_version=?4",
                    (next, asset, version, row_version),
                )
                .map_err(map_sqlite_error)?,
            "Asset",
        )?;
        if let Some((photo_id, photo_version)) = current_photo_for_asset(transaction, asset)? {
            bump_photo(transaction, photo_id, photo_version)?;
        }
    }
    if !rows.is_empty() {
        let library_bytes: Vec<u8> = transaction
            .query_row(
                "SELECT library_id FROM root WHERE root_id=?1",
                [root_id.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .map_err(map_sqlite_error)?;
        candidate_lookup::bump_catalog_revision(
            transaction,
            library_id_from(&library_bytes)?,
            binding_id,
        )?;
    }
    Ok(rows.len())
}

pub(super) fn bump_photo(
    transaction: &Transaction<'_>,
    photo_id: PhotoId,
    current_version: i64,
) -> Result<i64, DomainError> {
    let next = checked_next(current_version, "Photo")?;
    require_one(
        transaction
            .execute(
                "UPDATE photo SET photo_version=?1, row_version=row_version+1
                 WHERE photo_id=?2 AND photo_version=?3 AND lifecycle='Active'",
                (next, photo_id.as_bytes().as_slice(), current_version),
            )
            .map_err(map_sqlite_error)?,
        "Photo",
    )?;
    Ok(next)
}

pub(super) fn sync_photo_sorting(
    transaction: &Transaction<'_>,
    photo_id: PhotoId,
) -> Result<(), DomainError> {
    let preferred = transaction
        .query_row(
            "SELECT revision.relative_path, metadata.capture_local_key
             FROM photo_asset relation
             JOIN asset ON asset.asset_id=relation.asset_id
             JOIN asset_revision revision
               ON revision.asset_id=asset.asset_id AND revision.asset_version=asset.current_version
             LEFT JOIN asset_media_metadata metadata
               ON metadata.asset_id=asset.asset_id AND metadata.asset_version=asset.current_version
             WHERE relation.photo_id=?1 AND relation.to_photo_version IS NULL
               AND asset.presence_state='Present'
             ORDER BY CASE relation.role WHEN 'Jpg' THEN 0 ELSE 1 END LIMIT 1",
            [photo_id.as_bytes().as_slice()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    if let Some((relative, capture)) = preferred {
        let display = file_name_display(&relative);
        let key = natural_sort_key(&display);
        transaction
            .execute(
                "UPDATE photo SET date_class=?1, capture_local_key=?2,
                        natural_name_display=?3, natural_sort_key=?4, sort_key_version=1
                 WHERE photo_id=?5",
                (
                    if capture.is_some() {
                        "Known"
                    } else {
                        "Unknown"
                    },
                    capture,
                    display,
                    key,
                    photo_id.as_bytes().as_slice(),
                ),
            )
            .map_err(map_sqlite_error)?;
    }
    Ok(())
}

pub(super) fn file_name_display(relative: &str) -> String {
    relative
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(relative)
        .to_string()
}

pub(super) fn natural_sort_key(value: &str) -> Vec<u8> {
    let mut result = String::new();
    let folded = value.to_lowercase();
    let mut chars = folded.chars().peekable();
    while let Some(character) = chars.next() {
        if character.is_ascii_digit() {
            let mut digits = String::from(character);
            while chars.peek().is_some_and(|value| value.is_ascii_digit()) {
                digits.push(chars.next().unwrap());
            }
            let significant = digits.trim_start_matches('0');
            let significant = if significant.is_empty() {
                "0"
            } else {
                significant
            };
            result.push('\0');
            result.push_str(&format!("{:08}", significant.len()));
            result.push_str(significant);
            result.push_str(&format!("{:08}", digits.len()));
        } else {
            result.push(character);
        }
    }
    result.into_bytes()
}

fn validate_observation(observation: &CatalogObservation) -> Result<(), DomainError> {
    if observation.asset.relative_path.is_empty()
        || observation.asset.size_bytes < 0
        || observation.hard_link_count < 1
        || observation.asset.path_key.is_empty()
    {
        return Err(DomainError::new(
            ErrorCode::UnsafePath,
            "扫描观察包含无效文件身份。",
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct CurrentAsset {
    version: i64,
    row_version: i64,
    deletion_supported: bool,
    binding_id: Vec<u8>,
    path_key: Vec<u8>,
    file_id: Option<Vec<u8>>,
    size_bytes: i64,
    mtime_filetime: i64,
    presence_state: String,
    hard_link_count: i64,
}

#[derive(Debug)]
struct RetiredSnapshot {
    root_id: Vec<u8>,
    binding_id: Vec<u8>,
    relative_path: String,
    path_key: Vec<u8>,
    file_id: Option<Vec<u8>>,
    size_bytes: i64,
    mtime_filetime: i64,
    hard_link_count: i64,
}

fn checked_next(value: i64, entity: &'static str) -> Result<i64, DomainError> {
    value.checked_add(1).ok_or_else(|| {
        DomainError::new(ErrorCode::Internal, "对象版本已耗尽。").with_diagnostic(entity)
    })
}

fn require_one(changed: usize, entity: &'static str) -> Result<(), DomainError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(DomainError::stale_version(entity))
    }
}

fn to_i64(value: usize) -> Result<i64, DomainError> {
    i64::try_from(value).map_err(|_| DomainError::new(ErrorCode::Internal, "扫描计数溢出。"))
}

pub(crate) fn uuid_from(bytes: &[u8]) -> Result<uuid::Uuid, DomainError> {
    uuid::Uuid::from_slice(bytes)
        .map_err(|_| DomainError::new(ErrorCode::DatabaseCorrupt, "数据库对象标识损坏。"))
}

pub(super) fn asset_id_from(bytes: &[u8]) -> Result<AssetId, DomainError> {
    Ok(AssetId::from_uuid(uuid_from(bytes)?))
}

pub(super) fn photo_id_from(bytes: &[u8]) -> Result<PhotoId, DomainError> {
    Ok(PhotoId::from_uuid(uuid_from(bytes)?))
}

fn library_id_from(bytes: &[u8]) -> Result<LibraryId, DomainError> {
    Ok(LibraryId::from_uuid(uuid_from(bytes)?))
}

fn root_id_from(bytes: &[u8]) -> Result<RootId, DomainError> {
    Ok(RootId::from_uuid(uuid_from(bytes)?))
}

fn binding_id_from(bytes: &[u8]) -> Result<RootBindingId, DomainError> {
    Ok(RootBindingId::from_uuid(uuid_from(bytes)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_file_name_key_orders_digit_runs_numerically() {
        assert!(natural_sort_key("DSCF9.JPG") < natural_sort_key("DSCF10.JPG"));
        assert!(natural_sort_key("DSCF009.JPG") > natural_sort_key("DSCF9.JPG"));
    }
}

#[cfg(test)]
mod phase_one_tests;
