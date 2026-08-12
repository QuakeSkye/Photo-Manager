pub(crate) mod shell;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    catalog::CatalogService,
    db::{
        Database,
        migration::{map_sqlite_error, now_micros},
    },
    domain::{DomainError, ErrorCode},
    duplicate_review::{DuplicateReconciliationState, DuplicateResultReconciler},
    pending_delete::PendingDeleteService,
    roots::{
        FilesystemKind, open_read_only, path_key, probe_directory, probe_file,
        resolve_contained_path,
    },
    safety::{
        FaultInjector, FaultPoint, FeatureGate, GateCloseReason, RuntimeSafetyGate, SafetyPermit,
    },
};

pub use shell::REQUIRED_RECYCLE_FLAGS;
use shell::{
    RecycleCallbacks, RecycleShell, ShellAsset, ShellPhoto, ShellPhotoResult, StaRecycleWorker,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecyclePhase {
    Idle,
    Running,
    AwaitingUser,
    NeedsAttention,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DuplicateReconciliationPhase {
    NotApplicable,
    Pending,
    Applied,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecycleConfirmation {
    pub logical_photo_count: usize,
    pub physical_file_count: usize,
    pub total_bytes: i64,
    pub retry: bool,
    pub can_execute: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecycleStatus {
    pub phase: RecyclePhase,
    pub batch_id: Option<String>,
    pub recycled_files: usize,
    pub retryable_files: usize,
    pub needs_attention_files: usize,
    pub cancelled_files: usize,
    pub duplicate_reconciliation: DuplicateReconciliationPhase,
    pub successful_duplicate_file_count: usize,
    pub successful_duplicate_bytes: i64,
    pub remaining_problem_count: usize,
    pub duplicate_review_run_id: Option<String>,
    pub reconciliation_message: Option<String>,
    pub message: Option<String>,
}

impl Default for RecycleStatus {
    fn default() -> Self {
        Self {
            phase: RecyclePhase::Idle,
            batch_id: None,
            recycled_files: 0,
            retryable_files: 0,
            needs_attention_files: 0,
            cancelled_files: 0,
            duplicate_reconciliation: DuplicateReconciliationPhase::NotApplicable,
            successful_duplicate_file_count: 0,
            successful_duplicate_bytes: 0,
            remaining_problem_count: 0,
            duplicate_review_run_id: None,
            reconciliation_message: None,
            message: None,
        }
    }
}

#[derive(Debug, Clone)]
struct FrozenAsset {
    mark_id: Vec<u8>,
    mark_snapshot_id: Vec<u8>,
    _source_snapshot_id: Vec<u8>,
    photo_id: Vec<u8>,
    photo_version: i64,
    asset_id: Vec<u8>,
    asset_version: i64,
    role: String,
    library_id: Vec<u8>,
    root_id: Vec<u8>,
    root_binding_id: Vec<u8>,
    volume_id: Vec<u8>,
    root_path: PathBuf,
    root_path_key: Vec<u8>,
    root_directory_file_id: Option<Vec<u8>>,
    relative_path: String,
    path_key: Vec<u8>,
    path_key_version: i64,
    file_id: Vec<u8>,
    size_bytes: i64,
    mtime_filetime: i64,
}

#[derive(Debug, Clone)]
struct FrozenPhoto {
    mark_id: Vec<u8>,
    photo_id: Vec<u8>,
    photo_version: i64,
    assets: Vec<FrozenAsset>,
}

#[derive(Debug, Clone)]
struct AttemptAssetWork {
    attempt_asset_id: Vec<u8>,
    attempt_id: Vec<u8>,
    batch_id: Vec<u8>,
    batch_photo_id: Vec<u8>,
    batch_asset_id: Vec<u8>,
    lease_generation: i64,
    frozen: FrozenAsset,
}

#[derive(Debug, Clone)]
struct AttemptPhotoWork {
    assets: Vec<AttemptAssetWork>,
}

#[derive(Debug, Clone)]
struct AttemptContext {
    batch_id: Vec<u8>,
    attempt_id: Vec<u8>,
    report_id: Vec<u8>,
    lease_generation: i64,
    photos: Vec<AttemptPhotoWork>,
}

struct InterruptedAttempt {
    attempt_id: Vec<u8>,
    batch_id: Vec<u8>,
    report_id: Vec<u8>,
    lease_generation: i64,
}

type StoredReconciliation = (String, Option<Vec<u8>>, i64, i64, Option<String>);

struct NewAttempt<'a> {
    batch_id: &'a [u8],
    operation_id: &'a [u8],
    photos: &'a [FrozenPhoto],
    reason: &'a str,
    ordinal: i64,
    now: i64,
}

pub struct RecycleService {
    database: Arc<Database>,
    safety_gate: Arc<RuntimeSafetyGate>,
    pending_delete: Arc<PendingDeleteService>,
    catalog: Arc<CatalogService>,
    duplicate_reconciler: DuplicateResultReconciler,
    shell: Arc<dyn RecycleShell>,
    status: Arc<Mutex<RecycleStatus>>,
    execution_gate: Mutex<()>,
    faults: FaultInjector,
}

impl std::fmt::Debug for RecycleService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RecycleService")
            .finish_non_exhaustive()
    }
}

impl RecycleService {
    pub fn new(
        database: Arc<Database>,
        pending_delete: Arc<PendingDeleteService>,
        safety_gate: Arc<RuntimeSafetyGate>,
    ) -> Result<Self, DomainError> {
        let shell = Arc::new(StaRecycleWorker::spawn()?);
        let service = Self::with_shell(
            database,
            pending_delete,
            shell,
            safety_gate,
            FaultInjector::default(),
        );
        Ok(service)
    }

    pub(crate) fn with_shell(
        database: Arc<Database>,
        pending_delete: Arc<PendingDeleteService>,
        shell: Arc<dyn RecycleShell>,
        safety_gate: Arc<RuntimeSafetyGate>,
        faults: FaultInjector,
    ) -> Self {
        let catalog = Arc::new(CatalogService::new(database.clone()));
        let duplicate_reconciler = DuplicateResultReconciler::new(database.clone());
        Self {
            database,
            safety_gate,
            pending_delete,
            catalog,
            duplicate_reconciler,
            shell,
            status: Arc::new(Mutex::new(RecycleStatus::default())),
            execution_gate: Mutex::new(()),
            faults,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_permit(&self) -> SafetyPermit {
        self.safety_gate
            .require_feature(FeatureGate::Recycle)
            .expect("healthy test recycle gate")
    }

    pub fn confirmation(&self) -> Result<RecycleConfirmation, DomainError> {
        let connection = self.database.open_read_connection()?;
        if let Some(batch_id) = active_batch_id(&connection)? {
            return remaining_confirmation(&connection, &batch_id);
        }
        let photos = load_eligible_marks(&connection)?;
        Ok(confirmation_for_photos(&photos, false))
    }

    pub fn status(&self) -> Result<RecycleStatus, DomainError> {
        let mut status = self.status.lock().map_err(lock_error)?.clone();
        if let Some(batch_id) = active_batch_id(&self.database.open_read_connection()?)? {
            status = status_for_batch(&self.database.open_read_connection()?, &batch_id)?;
        }
        Ok(status)
    }

    pub fn reconcile_startup(&self) -> Result<(), DomainError> {
        self.reconcile_interrupted()?;
        let connection = self.database.open_read_connection()?;
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT batch.batch_id FROM deletion_batch batch
                 JOIN deletion_batch_photo photo ON photo.batch_id=batch.batch_id
                 JOIN deletion_batch_asset asset ON asset.batch_photo_id=photo.batch_photo_id
                 WHERE batch.batch_state IN ('Completed','Cancelled')
                   AND asset.resolution_state='Recycled'
                 ORDER BY batch.confirmed_at",
            )
            .map_err(map_sqlite_error)?;
        let batches = statement
            .query_map((), |row| row.get::<_, Vec<u8>>(0))
            .map_err(map_sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite_error)?;
        drop(statement);
        drop(connection);
        for batch in batches {
            self.finalize_stable_batch(&batch)?;
        }
        Ok(())
    }

    pub fn start(
        &self,
        permit: SafetyPermit,
        operation_id: Uuid,
        expected_photos: usize,
        expected_files: usize,
        expected_bytes: i64,
    ) -> Result<RecycleStatus, DomainError> {
        self.safety_gate.revalidate(&permit, FeatureGate::Recycle)?;
        let _gate = self.execution_gate.try_lock().map_err(|_| {
            DomainError::new(ErrorCode::LeaseUnavailable, "已有回收站任务正在执行。")
        })?;
        if active_batch_id(&self.database.open_read_connection()?)?.is_some() {
            return Err(DomainError::new(
                ErrorCode::InvalidTransition,
                "已有待处理删除批次；请重试或取消剩余项。",
            ));
        }
        let expected = RecycleConfirmation {
            logical_photo_count: expected_photos,
            physical_file_count: expected_files,
            total_bytes: expected_bytes,
            retry: false,
            can_execute: true,
        };
        self.safety_gate.revalidate(&permit, FeatureGate::Recycle)?;
        let attempt = self.create_initial_attempt(operation_id, expected)?;
        let batch_id = attempt.batch_id.clone();
        self.pending_delete.clear_session_history()?;
        self.execute_attempt(attempt)?;
        self.finalize_stable_batch(&batch_id)?;
        self.status()
    }

    pub fn retry(
        &self,
        permit: SafetyPermit,
        operation_id: Uuid,
        expected_photos: usize,
        expected_files: usize,
        expected_bytes: i64,
    ) -> Result<RecycleStatus, DomainError> {
        self.safety_gate.revalidate(&permit, FeatureGate::Recycle)?;
        let _gate = self.execution_gate.try_lock().map_err(|_| {
            DomainError::new(ErrorCode::LeaseUnavailable, "已有回收站任务正在执行。")
        })?;
        let batch_id =
            active_batch_id(&self.database.open_read_connection()?)?.ok_or_else(|| {
                DomainError::new(ErrorCode::InvalidTransition, "没有可重试的删除批次。")
            })?;
        let actual = remaining_confirmation(&self.database.open_read_connection()?, &batch_id)?;
        if actual.logical_photo_count != expected_photos
            || actual.physical_file_count != expected_files
            || actual.total_bytes != expected_bytes
        {
            return Err(DomainError::new(
                ErrorCode::StaleVersion,
                "剩余照片数量或容量已变化，请重新确认。",
            ));
        }
        self.safety_gate.revalidate(&permit, FeatureGate::Recycle)?;
        let attempt = self.create_retry_attempt(operation_id, &batch_id)?;
        self.execute_attempt(attempt)?;
        self.finalize_stable_batch(&batch_id)?;
        self.status()
    }

    pub fn cancel_remaining(&self, operation_id: Uuid) -> Result<RecycleStatus, DomainError> {
        let _gate = self
            .execution_gate
            .try_lock()
            .map_err(|_| DomainError::new(ErrorCode::LeaseUnavailable, "回收站任务仍在执行。"))?;
        let batch_id =
            active_batch_id(&self.database.open_read_connection()?)?.ok_or_else(|| {
                DomainError::new(ErrorCode::InvalidTransition, "没有可取消的剩余删除。")
            })?;
        let operation = operation_id.as_bytes().to_vec();
        let batch = batch_id.clone();
        self.database.writer()?.transaction(move |transaction| {
            let now = now_micros();
            insert_operation(transaction, &operation, "CancelRemainingRecycle", &batch)?;
            transaction.execute(
                "UPDATE deletion_batch_asset SET resolution_state='CancelledRemaining'
                 WHERE batch_asset_id IN (
                   SELECT asset.batch_asset_id FROM deletion_batch_asset asset
                   JOIN deletion_batch_photo photo ON photo.batch_photo_id=asset.batch_photo_id
                   WHERE photo.batch_id=?1 AND asset.resolution_state IN ('Pending','RetryableFailed','NeedsAttention')
                 )",
                [&batch],
            ).map_err(map_sqlite_error)?;
            release_batch_marks(transaction, &batch, &operation, "CancelledRemaining", now)?;
            transaction.execute(
                "UPDATE deletion_batch SET batch_state='Cancelled',row_version=row_version+1 WHERE batch_id=?1 AND batch_state IN ('CompletedWithErrors','NeedsAttention','CancelRequested','Running')",
                [&batch],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "UPDATE task_intent SET lifecycle='Cancelled',resolved_at=?2,row_version=row_version+1 WHERE task_id=?1 AND lifecycle NOT IN ('Completed','Cancelled')",
                params![&batch,now],
            ).map_err(map_sqlite_error)?;
            Ok(())
        })?;
        self.finalize_stable_batch(&batch_id)?;
        let status = status_for_batch(&self.database.open_read_connection()?, &batch_id)?;
        *self.status.lock().map_err(lock_error)? = status.clone();
        Ok(status)
    }

    fn finalize_stable_batch(&self, batch_id: &[u8]) -> Result<(), DomainError> {
        let state: String = self
            .database
            .open_read_connection()?
            .query_row(
                "SELECT batch_state FROM deletion_batch WHERE batch_id=?1",
                [batch_id],
                |row| row.get(0),
            )
            .map_err(map_sqlite_error)?;
        if !matches!(state.as_str(), "Completed" | "Cancelled") {
            return Ok(());
        }
        let mut status = status_for_batch(&self.database.open_read_connection()?, batch_id)?;
        match self.catalog.reconcile_recycled_batch(batch_id) {
            Ok(catalog_result) => {
                let _ = (
                    catalog_result.event_id.as_ref(),
                    catalog_result.from_revision,
                    catalog_result.to_revision,
                    catalog_result.changed_assets,
                );
                let outcome = self.duplicate_reconciler.reconcile_batch(batch_id)?;
                status.duplicate_reconciliation = match outcome.state {
                    DuplicateReconciliationState::NotApplicable => {
                        DuplicateReconciliationPhase::NotApplicable
                    }
                    DuplicateReconciliationState::Applied => DuplicateReconciliationPhase::Applied,
                    DuplicateReconciliationState::Rejected => {
                        DuplicateReconciliationPhase::Rejected
                    }
                };
                status.successful_duplicate_file_count = outcome.recycled_files;
                status.successful_duplicate_bytes = outcome.recycled_bytes;
                status.duplicate_review_run_id = outcome.run_id;
                status.reconciliation_message = outcome.message;
            }
            Err(error) => {
                let reason = format!(
                    "文件已成功移入回收站，但图库索引无法安全协调：{}",
                    error.user_message
                );
                let outcome = self.duplicate_reconciler.reject_batch(batch_id, &reason)?;
                status.duplicate_reconciliation = match outcome.state {
                    DuplicateReconciliationState::NotApplicable => {
                        DuplicateReconciliationPhase::NotApplicable
                    }
                    DuplicateReconciliationState::Applied => DuplicateReconciliationPhase::Applied,
                    DuplicateReconciliationState::Rejected => {
                        DuplicateReconciliationPhase::Rejected
                    }
                };
                status.successful_duplicate_file_count = outcome.recycled_files;
                status.successful_duplicate_bytes = outcome.recycled_bytes;
                status.duplicate_review_run_id = outcome.run_id;
                status.reconciliation_message = outcome.message;
            }
        }
        *self.status.lock().map_err(lock_error)? = status;
        Ok(())
    }

    fn create_initial_attempt(
        &self,
        operation_id: Uuid,
        expected: RecycleConfirmation,
    ) -> Result<AttemptContext, DomainError> {
        let batch_id = new_id();
        let operation = operation_id.as_bytes().to_vec();
        let batch_return = batch_id.clone();
        let faults = self.faults.clone();
        self.database.writer()?.transaction(move |transaction| {
            let photos = load_eligible_marks(transaction)?;
            let actual = confirmation_for_photos(&photos, false);
            if actual.logical_photo_count == 0 || actual != expected {
                return Err(DomainError::new(
                    ErrorCode::StaleVersion,
                    "最终确认中的照片数量、文件数量或容量已变化。",
                ));
            }
            let root = common_root(&photos)?;
            if root.root_filesystem()? != FilesystemKind::Ntfs {
                return Err(DomainError::new(
                    ErrorCode::CapabilityDenied,
                    "Windows 回收站动作只允许身份已验证的 NTFS Fuji 图库。",
                ));
            }
            let now = now_micros();
            insert_operation(transaction, &operation, "StartRecycle", &batch_return)?;
            faults.checkpoint(FaultPoint::SqliteBeforeCommit)?;
            transaction.execute(
                "INSERT INTO task_intent(task_id,task_kind,lifecycle,row_version,created_at) VALUES(?1,'Recycle','Prepared',1,?2)",
                params![&batch_return,now],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "INSERT INTO deletion_batch(batch_id,library_id,root_binding_id,batch_state,total_photos,total_files,total_bytes,confirmed_operation_id,confirmed_at,row_version)
                 VALUES(?1,?2,?3,'Prepared',?4,?5,?6,?7,?8,1)",
                params![&batch_return,&root.library_id,&root.root_binding_id,expected.logical_photo_count as i64,expected.physical_file_count as i64,expected.total_bytes,&operation,now],
            ).map_err(map_sqlite_error)?;
            faults.checkpoint(FaultPoint::SqliteBeforeCommit)?;
            let attempt = insert_attempt(
                transaction,
                &faults,
                NewAttempt {
                    batch_id: &batch_return,
                    operation_id: &operation,
                    photos: &photos,
                    reason: "Initial",
                    ordinal: 1,
                    now,
                },
            )?;
            transaction.execute(
                "UPDATE deletion_attempt SET attempt_state='Running',started_at=?2,row_version=row_version+1 WHERE attempt_id=?1",
                params![&attempt.attempt_id,now],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "UPDATE deletion_batch SET batch_state='Running',row_version=row_version+1 WHERE batch_id=?1",
                [&batch_return],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "UPDATE task_intent SET lifecycle='Active',row_version=row_version+1 WHERE task_id=?1",
                [&batch_return],
            ).map_err(map_sqlite_error)?;
            Ok(attempt)
        })
    }

    fn create_retry_attempt(
        &self,
        operation_id: Uuid,
        batch_id: &[u8],
    ) -> Result<AttemptContext, DomainError> {
        let operation = operation_id.as_bytes().to_vec();
        let batch = batch_id.to_vec();
        let faults = self.faults.clone();
        self.database.writer()?.transaction(move |transaction| {
            let state: String = transaction.query_row(
                "SELECT batch_state FROM deletion_batch WHERE batch_id=?1",
                [&batch],
                |row| row.get(0),
            ).map_err(map_sqlite_error)?;
            if state != "CompletedWithErrors" && state != "NeedsAttention" {
                return Err(DomainError::invalid_transition("DeletionBatch", &state, "Running"));
            }
            let photos = load_remaining_batch(transaction, &batch)?;
            for photo in &photos {
                for asset in &photo.assets {
                    validate_frozen_asset(asset)?;
                }
            }
            let ordinal: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(attempt_ordinal),0)+1 FROM task_attempt WHERE task_id=?1",
                [&batch],
                |row| row.get(0),
            ).map_err(map_sqlite_error)?;
            insert_operation(transaction, &operation, "RetryRecycle", &batch)?;
            let now = now_micros();
            let attempt = insert_attempt(
                transaction,
                &faults,
                NewAttempt {
                    batch_id: &batch,
                    operation_id: &operation,
                    photos: &photos,
                    reason: if state == "NeedsAttention" {
                        "Reconcile"
                    } else {
                        "Retry"
                    },
                    ordinal,
                    now,
                },
            )?;
            transaction.execute("UPDATE deletion_attempt SET attempt_state='Running',started_at=?2,row_version=row_version+1 WHERE attempt_id=?1",params![&attempt.attempt_id,now]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE deletion_batch SET batch_state='Running',row_version=row_version+1 WHERE batch_id=?1",[&batch]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE task_intent SET lifecycle='Active',row_version=row_version+1 WHERE task_id=?1",[&batch]).map_err(map_sqlite_error)?;
            Ok(attempt)
        })
    }

    fn execute_attempt(&self, attempt: AttemptContext) -> Result<(), DomainError> {
        self.set_running(&attempt.batch_id)?;
        let callbacks = Arc::new(CallbackBarrier {
            database: self.database.clone(),
            faults: self.faults.clone(),
            by_attempt_asset: attempt
                .photos
                .iter()
                .flat_map(|photo| photo.assets.iter().cloned())
                .map(|asset| (asset.attempt_asset_id.clone(), asset))
                .collect(),
        });
        let mut critical = None;
        'photos: for photo in &attempt.photos {
            for asset in &photo.assets {
                let single = AttemptPhotoWork {
                    assets: vec![asset.clone()],
                };
                if let Err(error) = self
                    .pending_delete
                    .reprove_batch_retention(&asset.batch_asset_id)
                {
                    self.record_preparation_failure(&single, &error)?;
                    continue;
                }
                if let Err(error) = self.prepare_photo(&single) {
                    self.record_preparation_failure(&single, &error)?;
                    if is_critical(&error) {
                        self.latch_critical_error(&error);
                        critical = Some(error);
                        break 'photos;
                    }
                    continue;
                }
                let permit = self.safety_gate.require_feature(FeatureGate::Recycle)?;
                self.safety_gate.revalidate(&permit, FeatureGate::Recycle)?;
                let shell_photo = ShellPhoto {
                    assets: vec![ShellAsset {
                        attempt_asset_id: asset.attempt_asset_id.clone(),
                        absolute_path: absolute_path(&asset.frozen)
                            .unwrap_or_else(|_| asset.frozen.root_path.clone()),
                    }],
                };
                match self.shell.recycle_photo(shell_photo, callbacks.clone()) {
                    Ok(result) => {
                        self.record_shell_summary(&single, result)?;
                        if result.operations_aborted {
                            self.reconcile_unreported(&single, "ShellCancelled")?;
                        }
                    }
                    Err(error)
                        if matches!(
                            error.code,
                            ErrorCode::DatabaseBusy
                                | ErrorCode::DatabaseDurability
                                | ErrorCode::DatabaseCorrupt
                                | ErrorCode::ReadOnlySafeMode
                                | ErrorCode::CapabilityDenied
                                | ErrorCode::FaultInjected
                        ) =>
                    {
                        // The Shell action may already have completed. Preserve the committed
                        // intent, reservation, lease and nonterminal callback state so startup
                        // reconciliation can observe the outcome. Never overwrite it here.
                        return Err(error);
                    }
                    Err(error) => {
                        self.latch_critical_error(&error);
                        let _ = self.reconcile_unreported(&single, "ShellOrCommitFailure");
                        critical = Some(error);
                        break 'photos;
                    }
                }
            }
        }
        if critical.is_some() {
            self.mark_unstarted_needs_attention(&attempt)?;
        }
        self.close_attempt(&attempt)?;
        Ok(())
    }

    fn latch_critical_error(&self, error: &DomainError) {
        let reason = match error.code {
            ErrorCode::IdentityMismatch | ErrorCode::RootOffline | ErrorCode::UnsafePath => {
                GateCloseReason::RootIdentity
            }
            ErrorCode::DatabaseBusy
            | ErrorCode::DatabaseDurability
            | ErrorCode::DatabaseCorrupt
            | ErrorCode::ReadOnlySafeMode => GateCloseReason::WriterUnhealthy,
            _ => GateCloseReason::CapabilityBoundary,
        };
        self.safety_gate.latch_fault(reason);
    }

    fn prepare_photo(&self, photo: &AttemptPhotoWork) -> Result<(), DomainError> {
        for asset in &photo.assets {
            validate_frozen_asset(&asset.frozen)?;
        }
        let photo = photo.clone();
        self.database.writer()?.transaction(move |transaction| {
            for asset in &photo.assets {
                assert_callback_chain(transaction, asset)?;
                let snapshot_id = new_id();
                let now = now_micros();
                transaction.execute(
                    "INSERT INTO file_identity_snapshot(snapshot_id,root_id,root_binding_id,volume_id,relative_path,path_key,path_key_version,file_id,size_bytes,mtime_filetime,captured_at,evidence_level)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'Strong')",
                    params![&snapshot_id,&asset.frozen.root_id,&asset.frozen.root_binding_id,&asset.frozen.volume_id,&asset.frozen.relative_path,&asset.frozen.path_key,asset.frozen.path_key_version,&asset.frozen.file_id,asset.frozen.size_bytes,asset.frozen.mtime_filetime,now],
                ).map_err(map_sqlite_error)?;
                transaction.execute(
                    "UPDATE deletion_attempt_asset SET asset_state='Validating',started_at=?2 WHERE attempt_asset_id=?1 AND asset_state='Prepared'",
                    params![&asset.attempt_asset_id,now],
                ).map_err(map_sqlite_error)?;
                transaction.execute(
                    "UPDATE deletion_attempt_asset SET asset_state='Recycling',validation_snapshot_id=?2 WHERE attempt_asset_id=?1 AND asset_state='Validating'",
                    params![&asset.attempt_asset_id,&snapshot_id],
                ).map_err(map_sqlite_error)?;
            }
            Ok(())
        })
    }

    fn record_preparation_failure(
        &self,
        photo: &AttemptPhotoWork,
        error: &DomainError,
    ) -> Result<(), DomainError> {
        let photo = photo.clone();
        let code = format!("{:?}", error.code);
        self.database.writer()?.transaction(move |transaction| {
            let now = now_micros();
            for asset in &photo.assets {
                transaction.execute(
                    "UPDATE deletion_attempt_asset SET asset_state='NeedsAttention',error_code=?2,finished_at=?3,committed_at=?3 WHERE attempt_asset_id=?1 AND asset_state IN ('Prepared','Validating')",
                    params![&asset.attempt_asset_id,&code,now],
                ).map_err(map_sqlite_error)?;
                transaction.execute(
                    "UPDATE deletion_batch_asset SET resolution_state='NeedsAttention' WHERE batch_asset_id=?1 AND resolution_state IN ('Pending','RetryableFailed','NeedsAttention')",
                    [&asset.batch_asset_id],
                ).map_err(map_sqlite_error)?;
            }
            Ok(())
        })
    }

    fn record_shell_summary(
        &self,
        photo: &AttemptPhotoWork,
        result: ShellPhotoResult,
    ) -> Result<(), DomainError> {
        let ids = photo
            .assets
            .iter()
            .map(|asset| asset.attempt_asset_id.clone())
            .collect::<Vec<_>>();
        self.database.writer()?.transaction(move |transaction| {
            for id in ids {
                transaction.execute(
                    "UPDATE deletion_attempt_asset SET perform_operations_result=?2,operations_aborted=?3 WHERE attempt_asset_id=?1",
                    params![id,result.perform_operations_result,result.operations_aborted],
                ).map_err(map_sqlite_error)?;
            }
            Ok(())
        })
    }

    fn reconcile_unreported(
        &self,
        photo: &AttemptPhotoWork,
        reason: &str,
    ) -> Result<(), DomainError> {
        let mut observations = Vec::new();
        for asset in &photo.assets {
            observations.push((asset.clone(), observe_source(&asset.frozen)));
        }
        let reason = reason.to_string();
        self.database.writer()?.transaction(move |transaction| {
            let now = now_micros();
            for (asset, observation) in observations {
                let state: Option<String> = transaction.query_row(
                    "SELECT asset_state FROM deletion_attempt_asset WHERE attempt_asset_id=?1",
                    [&asset.attempt_asset_id], |row| row.get(0),
                ).optional().map_err(map_sqlite_error)?;
                if state.as_deref() == Some("Recycled") { continue; }
                let retryable = observation == SourceObservation::Original;
                let next = if retryable { "RetryableFailed" } else { "NeedsAttention" };
                let attempt_next = if state.as_deref() == Some("Prepared") {
                    "Interrupted"
                } else {
                    next
                };
                transaction.execute(
                    "UPDATE deletion_attempt_asset SET asset_state=?2,error_code=?3,source_path_missing=?4,finished_at=?5,committed_at=?5 WHERE attempt_asset_id=?1 AND asset_state IN ('Prepared','Validating','Recycling')",
                    params![&asset.attempt_asset_id,attempt_next,&reason,observation == SourceObservation::Missing,now],
                ).map_err(map_sqlite_error)?;
                transaction.execute(
                    "UPDATE deletion_batch_asset SET resolution_state=?2 WHERE batch_asset_id=?1 AND resolution_state IN ('Pending','RetryableFailed','NeedsAttention')",
                    params![&asset.batch_asset_id,next],
                ).map_err(map_sqlite_error)?;
            }
            Ok(())
        })
    }

    fn mark_unstarted_needs_attention(&self, attempt: &AttemptContext) -> Result<(), DomainError> {
        let attempt_id = attempt.attempt_id.clone();
        self.database.writer()?.transaction(move |transaction| {
            let now = now_micros();
            transaction.execute(
                "UPDATE deletion_attempt_asset SET asset_state='NeedsAttention',error_code='StoppedAfterCritical',finished_at=?2,committed_at=?2 WHERE attempt_id=?1 AND asset_state IN ('Prepared','Validating','Recycling')",
                params![&attempt_id,now],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "UPDATE deletion_batch_asset SET resolution_state='NeedsAttention' WHERE batch_asset_id IN (
                   SELECT batch_asset_id FROM deletion_attempt_asset WHERE attempt_id=?1 AND asset_state='NeedsAttention'
                 ) AND resolution_state IN ('Pending','RetryableFailed','NeedsAttention')",
                [&attempt_id],
            ).map_err(map_sqlite_error)?;
            Ok(())
        })
    }

    fn close_attempt(&self, attempt: &AttemptContext) -> Result<(), DomainError> {
        let context = attempt.clone();
        self.database.writer()?.transaction(move |transaction| {
            let (recycled,retryable,attention,cancelled):(i64,i64,i64,i64)=transaction.query_row(
                "SELECT SUM(batch.resolution_state='Recycled'),SUM(batch.resolution_state='RetryableFailed'),SUM(batch.resolution_state='NeedsAttention'),SUM(batch.resolution_state IN ('CancelledRemaining','ResolvedWithoutDelete')) FROM deletion_attempt_asset attempt JOIN deletion_batch_asset batch ON batch.batch_asset_id=attempt.batch_asset_id WHERE attempt.attempt_id=?1",
                [&context.attempt_id], |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            ).map_err(map_sqlite_error)?;
            let now = now_micros();
            let (attempt_state,batch_state,task_state,report_result,resolved)=if attention>0 {
                ("NeedsAttention","NeedsAttention","NeedsAttention","NeedsAttention",false)
            } else if retryable>0 {
                ("CompletedWithErrors","CompletedWithErrors","AwaitingUser","Partial",false)
            } else if cancelled>0 {
                ("Cancelled","Cancelled","Cancelled","Cancelled",true)
            } else {
                ("Completed","Completed","Completed","Success",true)
            };
            transaction.execute("UPDATE deletion_attempt SET attempt_state=?2,finished_at=?3,row_version=row_version+1 WHERE attempt_id=?1 AND attempt_state='Running'",params![&context.attempt_id,attempt_state,now]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE operation_report SET report_state='Finalized',completed_at=?2,result=?3,item_count=(SELECT COUNT(*) FROM deletion_attempt_asset WHERE attempt_id=?1),total_bytes=(SELECT COALESCE(SUM(identity.size_bytes),0) FROM deletion_attempt_asset aa JOIN deletion_batch_asset ba ON ba.batch_asset_id=aa.batch_asset_id JOIN deletion_mark_asset_snapshot ms ON ms.mark_asset_snapshot_id=ba.mark_asset_snapshot_id JOIN file_identity_snapshot identity ON identity.snapshot_id=ms.file_identity_snapshot_id WHERE aa.attempt_id=?1) WHERE report_id=?4 AND report_state='Open'",params![&context.attempt_id,now,report_result,&context.report_id]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE deletion_batch SET batch_state=?2,row_version=row_version+1 WHERE batch_id=?1 AND batch_state IN ('Running','CancelRequested')",params![&context.batch_id,batch_state]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE task_intent SET lifecycle=?2,resolved_at=?3,row_version=row_version+1 WHERE task_id=?1 AND lifecycle='Active'",params![&context.batch_id,task_state,if resolved{Some(now)}else{None}]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE operation_reservation SET released_at=?2,release_reason='AttemptFinalized' WHERE attempt_id=?1 AND released_at IS NULL",params![&context.attempt_id,now]).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE file_write_lease SET task_id=NULL,lease_generation=lease_generation+1,acquired_at=NULL,released_at=?2,row_version=row_version+1 WHERE slot=1 AND task_id=?1 AND lease_generation=?3",params![&context.batch_id,now,context.lease_generation]).map_err(map_sqlite_error)?;
            let remaining: i64=transaction.query_row("SELECT COUNT(*) FROM deletion_batch_asset asset JOIN deletion_batch_photo photo ON photo.batch_photo_id=asset.batch_photo_id WHERE photo.batch_id=?1 AND asset.resolution_state NOT IN ('Recycled','CancelledRemaining','ResolvedWithoutDelete')",[&context.batch_id],|row|row.get(0)).map_err(map_sqlite_error)?;
            if remaining==0 {
                let operation:Vec<u8>=transaction.query_row("SELECT confirmed_operation_id FROM deletion_batch WHERE batch_id=?1",[&context.batch_id],|row|row.get(0)).map_err(map_sqlite_error)?;
                release_batch_marks(transaction,&context.batch_id,&operation,"BatchResolved",now)?;
            }
            let _ = recycled;
            Ok(())
        })?;
        let status = status_for_batch(&self.database.open_read_connection()?, &attempt.batch_id)?;
        *self.status.lock().map_err(lock_error)? = status;
        Ok(())
    }

    fn reconcile_interrupted(&self) -> Result<(), DomainError> {
        let connection = self.database.open_read_connection()?;
        let attempt: Option<InterruptedAttempt> = connection.query_row(
            "SELECT deletion.attempt_id,deletion.batch_id,report.report_id,lease.lease_generation
             FROM deletion_attempt deletion JOIN operation_report report ON report.task_attempt_id=deletion.attempt_id
             JOIN file_write_lease lease ON lease.task_id=deletion.batch_id
             WHERE deletion.attempt_state IN ('Prepared','Running') LIMIT 1",
            (), |row|Ok(InterruptedAttempt {
                attempt_id: row.get(0)?,
                batch_id: row.get(1)?,
                report_id: row.get(2)?,
                lease_generation: row.get(3)?,
            }),
        ).optional().map_err(map_sqlite_error)?;
        let Some(attempt) = attempt else {
            return Ok(());
        };
        let context = load_attempt_context(
            &connection,
            &attempt.attempt_id,
            attempt.batch_id,
            attempt.report_id,
            attempt.lease_generation,
        )?;
        for photo in &context.photos {
            let _ = self.reconcile_unreported(photo, "InterruptedRecovery");
        }
        self.close_attempt(&context)
    }

    fn set_running(&self, batch_id: &[u8]) -> Result<(), DomainError> {
        *self.status.lock().map_err(lock_error)? = RecycleStatus {
            phase: RecyclePhase::Running,
            batch_id: Some(uuid_string(batch_id)?),
            recycled_files: 0,
            retryable_files: 0,
            needs_attention_files: 0,
            cancelled_files: 0,
            duplicate_reconciliation: DuplicateReconciliationPhase::Pending,
            successful_duplicate_file_count: 0,
            successful_duplicate_bytes: 0,
            remaining_problem_count: 0,
            duplicate_review_run_id: None,
            reconciliation_message: None,
            message: Some("正在逐张移入 Windows 回收站。".into()),
        };
        Ok(())
    }
}

struct CallbackBarrier {
    database: Arc<Database>,
    faults: FaultInjector,
    by_attempt_asset: HashMap<Vec<u8>, AttemptAssetWork>,
}

impl RecycleCallbacks for CallbackBarrier {
    fn pre_delete(&self, asset: &ShellAsset) -> Result<(), DomainError> {
        self.faults.checkpoint(FaultPoint::ShellBeforeCallback)?;
        let work = self
            .by_attempt_asset
            .get(&asset.attempt_asset_id)
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::IdentityMismatch,
                    "Shell 回调不属于当前 Attempt。",
                )
            })?;
        validate_frozen_asset(&work.frozen)?;
        self.faults.checkpoint(FaultPoint::ShellAfterCallback)
    }

    fn post_delete(
        &self,
        asset: &ShellAsset,
        hr_delete: i32,
        evidence: Option<Vec<u8>>,
    ) -> Result<(), DomainError> {
        self.faults.checkpoint(FaultPoint::ShellBeforeCallback)?;
        self.faults
            .checkpoint(FaultPoint::RecycleAfterPhysicalBeforeFact)?;
        let work = self
            .by_attempt_asset
            .get(&asset.attempt_asset_id)
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::IdentityMismatch,
                    "Shell 回调不属于当前 Attempt。",
                )
            })?
            .clone();
        let observation = observe_source(&work.frozen);
        let success = hr_delete >= 0
            && evidence.as_ref().is_some_and(|value| !value.is_empty())
            && observation == SourceObservation::Missing;
        let critical = hr_delete >= 0 && !success
            || (hr_delete < 0 && observation != SourceObservation::Original);
        let state = if success {
            "Recycled"
        } else if critical {
            "NeedsAttention"
        } else {
            "RetryableFailed"
        };
        let evidence_for_fact = evidence.clone();
        self.database.writer()?.transaction(move |transaction| {
            assert_callback_chain(transaction,&work)?;
            let now=now_micros();
            transaction.execute(
                "UPDATE deletion_attempt_asset SET asset_state=?2,shell_result=?3,recycle_object_present=?4,recycle_object_evidence=?5,source_path_missing=?6,error_code=?7,finished_at=?8,committed_at=?8 WHERE attempt_asset_id=?1 AND asset_state='Recycling'",
                params![&work.attempt_asset_id,state,hr_delete,evidence.is_some(),evidence,observation==SourceObservation::Missing,if success{None::<String>}else{Some(if critical{"CriticalSafetyViolation"}else{"ShellDeleteFailed"}.to_string())},now],
            ).map_err(map_sqlite_error)?;
            transaction.execute("UPDATE deletion_batch_asset SET resolution_state=?2 WHERE batch_asset_id=?1 AND resolution_state IN ('Pending','RetryableFailed','NeedsAttention')",params![&work.batch_asset_id,state]).map_err(map_sqlite_error)?;
            if success {
                transaction.execute("INSERT INTO deletion_recycled_fact(recycled_fact_id,batch_asset_id,attempt_asset_id,evidence_snapshot,recycled_at) VALUES(?1,?2,?3,?4,?5)",params![new_id(),&work.batch_asset_id,&work.attempt_asset_id,evidence_for_fact.expect("success evidence"),now]).map_err(map_sqlite_error)?;
            }
            let report_id:Vec<u8>=transaction.query_row("SELECT report_id FROM operation_report WHERE task_attempt_id=?1",[&work.attempt_id],|row|row.get(0)).map_err(map_sqlite_error)?;
            transaction.execute("INSERT INTO operation_report_item(report_item_id,report_id,item_kind,stable_item_id,result_code,warning_code,display_path_snapshot,size_bytes) VALUES(?1,?2,'RecycleAsset',?3,?4,?5,?6,?7)",params![new_id(),report_id,&work.batch_asset_id,if success{"Recycled"}else if critical{"NeedsAttention"}else{"RetryableFailed"},if critical{Some("CriticalSafetyViolation")}else{None},&work.frozen.relative_path,work.frozen.size_bytes]).map_err(map_sqlite_error)?;
            if success {
                let remaining:i64=transaction.query_row("SELECT COUNT(*) FROM deletion_batch_asset WHERE batch_photo_id=?1 AND resolution_state!='Recycled'",[&work.batch_photo_id],|row|row.get(0)).map_err(map_sqlite_error)?;
                if remaining==0 {
                    let operation:Vec<u8>=transaction.query_row("SELECT confirmed_operation_id FROM deletion_batch WHERE batch_id=?1",[&work.batch_id],|row|row.get(0)).map_err(map_sqlite_error)?;
                    transaction.execute("UPDATE deletion_mark SET released_at=?2,released_operation_id=?3,release_reason='Recycled' WHERE mark_id=?1 AND released_at IS NULL",params![&work.frozen.mark_id,now,operation]).map_err(map_sqlite_error)?;
                }
            }
            Ok(())
        })?;
        self.faults.checkpoint(FaultPoint::ShellAfterCallback)?;
        if critical {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "回收结果无法完整证明，已停止剩余删除并进入需要处理。",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceObservation {
    Original,
    Missing,
    Different,
    Unavailable,
}

fn observe_source(asset: &FrozenAsset) -> SourceObservation {
    let Ok(path) = absolute_path(asset) else {
        return SourceObservation::Unavailable;
    };
    match open_read_only(&path) {
        Ok(file) => match probe_file(&file) {
            Ok(probe)
                if probe.file_id.as_deref() == Some(asset.file_id.as_slice())
                    && i64::try_from(probe.size_bytes).ok() == Some(asset.size_bytes)
                    && probe.mtime_filetime == asset.mtime_filetime =>
            {
                SourceObservation::Original
            }
            Ok(_) => SourceObservation::Different,
            Err(_) => SourceObservation::Unavailable,
        },
        Err(error) if !path.exists() && error.code == ErrorCode::RootOffline => {
            SourceObservation::Missing
        }
        Err(_) => SourceObservation::Unavailable,
    }
}

fn validate_frozen_asset(asset: &FrozenAsset) -> Result<(), DomainError> {
    let root = probe_directory(&asset.root_path)?;
    if root.filesystem_kind != FilesystemKind::Ntfs
        || root.absolute_path_key != asset.root_path_key
        || root.directory_file_id != asset.root_directory_file_id
    {
        return Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "Fuji 图库 Root 身份或 NTFS 能力已变化。",
        ));
    }
    let path = absolute_path(asset)?;
    let file = open_read_only(&path)?;
    let probe = probe_file(&file)?;
    if probe.file_id.as_deref() != Some(asset.file_id.as_slice())
        || i64::try_from(probe.size_bytes).ok() != Some(asset.size_bytes)
        || probe.mtime_filetime != asset.mtime_filetime
        || probe.hard_link_count != 1
        || probe.reparse_point
        || path_key(Path::new(&asset.relative_path)) != asset.path_key
    {
        return Err(DomainError::new(
            ErrorCode::IdentityMismatch,
            "文件身份、路径、大小、mtime、hard link 或 reparse 状态已变化。",
        ));
    }
    Ok(())
}

fn absolute_path(asset: &FrozenAsset) -> Result<PathBuf, DomainError> {
    resolve_contained_path(&asset.root_path, Path::new(&asset.relative_path))
}

fn is_critical(error: &DomainError) -> bool {
    matches!(
        error.code,
        ErrorCode::DatabaseBusy
            | ErrorCode::DatabaseDurability
            | ErrorCode::DatabaseCorrupt
            | ErrorCode::IdentityMismatch
            | ErrorCode::UnsafePath
            | ErrorCode::FaultInjected
    )
}

fn insert_attempt(
    transaction: &Transaction<'_>,
    faults: &FaultInjector,
    new_attempt: NewAttempt<'_>,
) -> Result<AttemptContext, DomainError> {
    let NewAttempt {
        batch_id,
        operation_id,
        photos,
        reason,
        ordinal,
        now,
    } = new_attempt;
    let attempt_id = new_id();
    let report_id = new_id();
    transaction.execute("INSERT INTO task_attempt(attempt_id,task_id,attempt_kind,attempt_ordinal,operation_id,created_at) VALUES(?1,?2,'Recycle',?3,?4,?5)",params![&attempt_id,batch_id,ordinal,operation_id,now]).map_err(map_sqlite_error)?;
    transaction.execute("INSERT INTO deletion_attempt(attempt_id,batch_id,attempt_reason,attempt_state,row_version) VALUES(?1,?2,?3,'Prepared',1)",params![&attempt_id,batch_id,reason]).map_err(map_sqlite_error)?;
    let root = &photos[0].assets[0];
    transaction.execute("INSERT INTO operation_report(report_id,report_kind,library_id,root_id,task_attempt_id,report_state,started_at,item_count,total_bytes) VALUES(?1,'Recycle',?2,?3,?4,'Open',?5,0,0)",params![&report_id,&root.library_id,&root.root_id,&attempt_id,now]).map_err(map_sqlite_error)?;
    let (lease_task, generation, row_version): (Option<Vec<u8>>, i64, i64) = transaction
        .query_row(
            "SELECT task_id,lease_generation,row_version FROM file_write_lease WHERE slot=1",
            (),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(map_sqlite_error)?;
    if lease_task.is_some() {
        return Err(DomainError::new(
            ErrorCode::LeaseUnavailable,
            "全局文件写入 Lease 已被占用。",
        ));
    }
    let lease_generation = generation + 1;
    transaction.execute("UPDATE file_write_lease SET task_id=?1,lease_generation=?2,acquired_at=?3,released_at=NULL,row_version=?4 WHERE slot=1 AND task_id IS NULL AND lease_generation=?5 AND row_version=?6",params![batch_id,lease_generation,now,row_version+1,generation,row_version]).map_err(map_sqlite_error)?;
    let mut photo_work = Vec::new();
    for photo in photos {
        let existing_batch_photo: Option<Vec<u8>> = transaction
            .query_row(
                "SELECT batch_photo_id FROM deletion_batch_photo WHERE batch_id=?1 AND photo_id=?2",
                params![batch_id, &photo.photo_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite_error)?;
        let batch_photo_id = if let Some(id) = existing_batch_photo {
            id
        } else {
            let id = new_id();
            let bytes = photo.assets.iter().map(|a| a.size_bytes).sum::<i64>();
            transaction.execute("INSERT INTO deletion_batch_photo(batch_photo_id,batch_id,photo_id,photo_version,mark_id,expected_members,expected_bytes) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![&id,batch_id,&photo.photo_id,photo.photo_version,&photo.mark_id,photo.assets.len() as i64,bytes]).map_err(map_sqlite_error)?;
            id
        };
        let mut assets = Vec::new();
        for frozen in &photo.assets {
            let batch_asset_id: Vec<u8> = if let Some(id) = transaction
                .query_row(
                    "SELECT batch_asset_id FROM deletion_batch_asset WHERE batch_photo_id=?1 AND asset_id=?2",
                    params![&batch_photo_id, &frozen.asset_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(map_sqlite_error)?
            {
                id
            } else {
                let id = new_id();
                transaction.execute(
                    "INSERT INTO deletion_batch_asset(batch_asset_id,batch_photo_id,asset_id,role,asset_version,mark_asset_snapshot_id,resolution_state)
                     VALUES(?1,?2,?3,?4,?5,?6,'Pending')",
                    params![&id, &batch_photo_id, &frozen.asset_id, &frozen.role, frozen.asset_version, &frozen.mark_snapshot_id],
                ).map_err(map_sqlite_error)?;
                id
            };
            transaction
                .execute(
                    "INSERT OR IGNORE INTO deletion_batch_retention_guard(
                    batch_guard_id,batch_asset_id,source_guard_id,retained_asset_id,
                    retained_asset_version,retained_snapshot_id,equivalence_proof_id)
                 SELECT ?1,?2,guard_id,retained_asset_id,retained_asset_version,
                        retained_snapshot_id,equivalence_proof_id
                 FROM duplicate_retention_guard WHERE mark_asset_snapshot_id=?3",
                    params![new_id(), &batch_asset_id, &frozen.mark_snapshot_id],
                )
                .map_err(map_sqlite_error)?;
            let attempt_asset_id = new_id();
            transaction.execute("INSERT INTO deletion_attempt_asset(attempt_asset_id,attempt_id,batch_asset_id,asset_state) VALUES(?1,?2,?3,'Prepared')",params![&attempt_asset_id,&attempt_id,&batch_asset_id]).map_err(map_sqlite_error)?;
            transaction.execute("INSERT INTO operation_reservation(reservation_id,owner_kind,task_id,attempt_id,lease_generation,reservation_kind,root_id,root_binding_id,asset_id,file_id,path_key,created_at) VALUES(?1,'TaskAttempt',?2,?3,?4,'RecycleAsset',?5,?6,?7,?8,?9,?10)",params![new_id(),batch_id,&attempt_id,lease_generation,&frozen.root_id,&frozen.root_binding_id,&frozen.asset_id,&frozen.file_id,&frozen.path_key,now]).map_err(map_sqlite_error)?;
            assets.push(AttemptAssetWork {
                attempt_asset_id,
                attempt_id: attempt_id.clone(),
                batch_id: batch_id.to_vec(),
                batch_photo_id: batch_photo_id.clone(),
                batch_asset_id,
                lease_generation,
                frozen: frozen.clone(),
            });
            faults.checkpoint(FaultPoint::SqliteBeforeCommit)?;
        }
        photo_work.push(AttemptPhotoWork { assets });
    }
    Ok(AttemptContext {
        batch_id: batch_id.to_vec(),
        attempt_id,
        report_id,
        lease_generation,
        photos: photo_work,
    })
}

fn assert_callback_chain(
    transaction: &Transaction<'_>,
    work: &AttemptAssetWork,
) -> Result<(), DomainError> {
    let valid:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM deletion_attempt_asset aa JOIN deletion_attempt da ON da.attempt_id=aa.attempt_id JOIN operation_reservation reservation ON reservation.attempt_id=aa.attempt_id AND reservation.asset_id=?2 AND reservation.released_at IS NULL JOIN file_write_lease lease ON lease.slot=1 WHERE aa.attempt_asset_id=?1 AND aa.batch_asset_id=?3 AND da.batch_id=?4 AND da.attempt_state='Running' AND reservation.lease_generation=?5 AND lease.task_id=?4 AND lease.lease_generation=?5)",params![&work.attempt_asset_id,&work.frozen.asset_id,&work.batch_asset_id,&work.batch_id,work.lease_generation],|row|row.get(0)).map_err(map_sqlite_error)?;
    if !valid {
        return Err(DomainError::new(
            ErrorCode::LeaseGenerationMismatch,
            "旧回调或失效 Lease 被拒绝。",
        ));
    }
    Ok(())
}

fn load_eligible_marks(connection: &Connection) -> Result<Vec<FrozenPhoto>, DomainError> {
    let mut statement=connection.prepare("SELECT mark.mark_id,mark.photo_id,mark.photo_version,photo.photo_version,photo.lifecycle,snapshot.mark_asset_snapshot_id,snapshot.asset_id,snapshot.asset_version,snapshot.role,snapshot.file_identity_snapshot_id,identity.root_id,identity.root_binding_id,identity.volume_id,identity.relative_path,identity.path_key,identity.path_key_version,identity.file_id,identity.size_bytes,identity.mtime_filetime,asset.library_id,asset.current_version,asset.presence_state,asset.deletion_supported,revision.root_id,revision.root_binding_id,revision.relative_path,revision.path_key,revision.path_key_version,revision.file_id,revision.size_bytes,revision.mtime_filetime,revision.hard_link_count,relation.role,root.lifecycle,root.current_binding_id,root.capability_mask,binding.binding_state,binding.absolute_path_display,binding.absolute_path_key,binding.root_directory_file_id,binding.filesystem_kind,binding.reparse_checked,(CASE WHEN mark.mark_scope='DuplicateSubset' THEN (SELECT COUNT(*) FROM deletion_mark_asset_snapshot selected_snapshot WHERE selected_snapshot.mark_id=mark.mark_id) ELSE (SELECT COUNT(*) FROM photo_asset current_member WHERE current_member.photo_id=mark.photo_id AND current_member.to_photo_version IS NULL) END) FROM deletion_mark mark JOIN photo ON photo.photo_id=mark.photo_id JOIN deletion_mark_asset_snapshot snapshot ON snapshot.mark_id=mark.mark_id JOIN file_identity_snapshot identity ON identity.snapshot_id=snapshot.file_identity_snapshot_id JOIN asset ON asset.asset_id=snapshot.asset_id JOIN asset_revision revision ON revision.asset_id=asset.asset_id AND revision.asset_version=asset.current_version LEFT JOIN photo_asset relation ON relation.photo_id=mark.photo_id AND relation.asset_id=asset.asset_id AND relation.to_photo_version IS NULL JOIN root ON root.root_id=identity.root_id JOIN root_binding binding ON binding.binding_id=identity.root_binding_id WHERE mark.released_at IS NULL AND NOT EXISTS(SELECT 1 FROM deletion_batch_photo consumed WHERE consumed.mark_id=mark.mark_id) ORDER BY mark.mark_id,snapshot.role").map_err(map_sqlite_error)?;
    let rows = statement
        .query_map((), |row| {
            Ok((
                FrozenAsset {
                    mark_id: row.get(0)?,
                    photo_id: row.get(1)?,
                    photo_version: row.get(2)?,
                    mark_snapshot_id: row.get(5)?,
                    asset_id: row.get(6)?,
                    asset_version: row.get(7)?,
                    role: row.get(8)?,
                    _source_snapshot_id: row.get(9)?,
                    root_id: row.get(10)?,
                    root_binding_id: row.get(11)?,
                    volume_id: row.get(12)?,
                    relative_path: row.get(13)?,
                    path_key: row.get(14)?,
                    path_key_version: row.get(15)?,
                    file_id: row.get::<_, Option<Vec<u8>>>(16)?.unwrap_or_default(),
                    size_bytes: row.get(17)?,
                    mtime_filetime: row.get(18)?,
                    library_id: row.get(19)?,
                    root_path: PathBuf::from(row.get::<_, String>(37)?),
                    root_path_key: row.get(38)?,
                    root_directory_file_id: row.get(39)?,
                },
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(20)?,
                row.get::<_, String>(21)?,
                row.get::<_, bool>(22)?,
                row.get::<_, Vec<u8>>(23)?,
                row.get::<_, Vec<u8>>(24)?,
                row.get::<_, String>(25)?,
                row.get::<_, Vec<u8>>(26)?,
                row.get::<_, i64>(27)?,
                row.get::<_, Option<Vec<u8>>>(28)?.unwrap_or_default(),
                row.get::<_, i64>(29)?,
                row.get::<_, i64>(30)?,
                row.get::<_, i64>(31)?,
                row.get::<_, Option<String>>(32)?,
                row.get::<_, String>(33)?,
                row.get::<_, Option<Vec<u8>>>(34)?.unwrap_or_default(),
                row.get::<_, i64>(35)?,
                row.get::<_, String>(36)?,
                row.get::<_, String>(40)?,
                row.get::<_, bool>(41)?,
                row.get::<_, i64>(42)?,
            ))
        })
        .map_err(map_sqlite_error)?
        .map(|r| r.map_err(map_sqlite_error))
        .collect::<Result<Vec<_>, _>>()?;
    let mut rows = rows.into_iter().peekable();
    let mut photos = Vec::new();
    while let Some(first_row) = rows.next() {
        let mark_id = first_row.0.mark_id.clone();
        let mut group = vec![first_row];
        while rows.peek().is_some_and(|next| next.0.mark_id == mark_id) {
            group.push(rows.next().expect("peeked row"));
        }
        let count = group.len() as i64;
        let valid = group.iter().all(
            |(
                a,
                pv,
                pl,
                av,
                ap,
                ds,
                rr,
                rb,
                rpath,
                rkey,
                rpkv,
                rfid,
                rsize,
                rmtime,
                links,
                role,
                root_state,
                current_binding,
                cap,
                binding_state,
                fs,
                reparse,
                members,
            )| {
                *pv == a.photo_version
                    && pl == "Active"
                    && *av == a.asset_version
                    && ap == "Present"
                    && *ds
                    && *rr == a.root_id
                    && *rb == a.root_binding_id
                    && rpath == &a.relative_path
                    && rkey == &a.path_key
                    && *rpkv == a.path_key_version
                    && rfid == &a.file_id
                    && *rsize == a.size_bytes
                    && *rmtime == a.mtime_filetime
                    && *links == 1
                    && role.as_deref() == Some(&a.role)
                    && root_state == "Online"
                    && current_binding == &a.root_binding_id
                    && (*cap & (1 << 6)) != 0
                    && binding_state == "Active"
                    && fs == "NTFS"
                    && *reparse
                    && *members == count
                    && !a.file_id.is_empty()
            },
        );
        if valid {
            let first = &group[0].0;
            photos.push(FrozenPhoto {
                mark_id: first.mark_id.clone(),
                photo_id: first.photo_id.clone(),
                photo_version: first.photo_version,
                assets: group.into_iter().map(|r| r.0).collect(),
            })
        }
    }
    Ok(photos)
}

fn load_remaining_batch(
    connection: &Connection,
    batch_id: &[u8],
) -> Result<Vec<FrozenPhoto>, DomainError> {
    let mut statement=connection.prepare("SELECT photo.mark_id,photo.photo_id,photo.photo_version,asset.mark_asset_snapshot_id,snapshot.file_identity_snapshot_id,asset.asset_id,asset.asset_version,asset.role,identity.root_id,identity.root_binding_id,identity.volume_id,identity.relative_path,identity.path_key,identity.path_key_version,identity.file_id,identity.size_bytes,identity.mtime_filetime,current_asset.library_id,binding.absolute_path_display,binding.absolute_path_key,binding.root_directory_file_id FROM deletion_batch_photo photo JOIN deletion_batch_asset asset ON asset.batch_photo_id=photo.batch_photo_id JOIN deletion_mark_asset_snapshot snapshot ON snapshot.mark_asset_snapshot_id=asset.mark_asset_snapshot_id JOIN file_identity_snapshot identity ON identity.snapshot_id=snapshot.file_identity_snapshot_id JOIN asset current_asset ON current_asset.asset_id=asset.asset_id JOIN root_binding binding ON binding.binding_id=identity.root_binding_id WHERE photo.batch_id=?1 AND asset.resolution_state IN ('Pending','RetryableFailed','NeedsAttention') ORDER BY photo.batch_photo_id,asset.role").map_err(map_sqlite_error)?;
    let rows = statement
        .query_map([batch_id], |row| {
            Ok(FrozenAsset {
                mark_id: row.get(0)?,
                photo_id: row.get(1)?,
                photo_version: row.get(2)?,
                mark_snapshot_id: row.get(3)?,
                _source_snapshot_id: row.get(4)?,
                asset_id: row.get(5)?,
                asset_version: row.get(6)?,
                role: row.get(7)?,
                root_id: row.get(8)?,
                root_binding_id: row.get(9)?,
                volume_id: row.get(10)?,
                relative_path: row.get(11)?,
                path_key: row.get(12)?,
                path_key_version: row.get(13)?,
                file_id: row.get::<_, Option<Vec<u8>>>(14)?.unwrap_or_default(),
                size_bytes: row.get(15)?,
                mtime_filetime: row.get(16)?,
                library_id: row.get(17)?,
                root_path: PathBuf::from(row.get::<_, String>(18)?),
                root_path_key: row.get(19)?,
                root_directory_file_id: row.get(20)?,
            })
        })
        .map_err(map_sqlite_error)?
        .map(|r| r.map_err(map_sqlite_error))
        .collect::<Result<Vec<_>, _>>()?;
    let mut photos = Vec::new();
    for asset in rows {
        if photos
            .last()
            .is_none_or(|p: &FrozenPhoto| p.photo_id != asset.photo_id)
        {
            photos.push(FrozenPhoto {
                mark_id: asset.mark_id.clone(),
                photo_id: asset.photo_id.clone(),
                photo_version: asset.photo_version,
                assets: Vec::new(),
            })
        }
        photos.last_mut().unwrap().assets.push(asset)
    }
    Ok(photos)
}

fn load_attempt_context(
    connection: &Connection,
    attempt_id: &[u8],
    batch_id: Vec<u8>,
    report_id: Vec<u8>,
    lease_generation: i64,
) -> Result<AttemptContext, DomainError> {
    let photos = load_remaining_batch(connection, &batch_id)?;
    let mut result = Vec::new();
    for photo in photos {
        let mut assets = Vec::new();
        for frozen in photo.assets {
            let (attempt_asset_id,batch_asset_id,batch_photo_id):(Vec<u8>,Vec<u8>,Vec<u8>)=connection.query_row("SELECT aa.attempt_asset_id,aa.batch_asset_id,ba.batch_photo_id FROM deletion_attempt_asset aa JOIN deletion_batch_asset ba ON ba.batch_asset_id=aa.batch_asset_id WHERE aa.attempt_id=?1 AND ba.asset_id=?2",params![attempt_id,&frozen.asset_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(map_sqlite_error)?;
            assets.push(AttemptAssetWork {
                attempt_asset_id,
                attempt_id: attempt_id.to_vec(),
                batch_id: batch_id.clone(),
                batch_photo_id,
                batch_asset_id,
                lease_generation,
                frozen,
            })
        }
        result.push(AttemptPhotoWork { assets })
    }
    Ok(AttemptContext {
        batch_id,
        attempt_id: attempt_id.to_vec(),
        report_id,
        lease_generation,
        photos: result,
    })
}

fn confirmation_for_photos(photos: &[FrozenPhoto], retry: bool) -> RecycleConfirmation {
    let physical_file_count = photos.iter().map(|p| p.assets.len()).sum();
    let total_bytes = photos
        .iter()
        .flat_map(|p| &p.assets)
        .map(|a| a.size_bytes)
        .fold(0_i64, i64::saturating_add);
    RecycleConfirmation {
        logical_photo_count: photos.len(),
        physical_file_count,
        total_bytes,
        retry,
        can_execute: !photos.is_empty(),
    }
}
fn remaining_confirmation(
    connection: &Connection,
    batch_id: &[u8],
) -> Result<RecycleConfirmation, DomainError> {
    Ok(confirmation_for_photos(
        &load_remaining_batch(connection, batch_id)?,
        true,
    ))
}
fn common_root(photos: &[FrozenPhoto]) -> Result<&FrozenAsset, DomainError> {
    let first = photos
        .first()
        .and_then(|p| p.assets.first())
        .ok_or_else(|| {
            DomainError::new(ErrorCode::InvalidTransition, "没有可执行的有效待删除照片。")
        })?;
    if photos
        .iter()
        .flat_map(|p| &p.assets)
        .any(|a| a.root_id != first.root_id || a.root_binding_id != first.root_binding_id)
    {
        return Err(DomainError::new(
            ErrorCode::RootConflict,
            "删除批次不能跨 Root。",
        ));
    }
    Ok(first)
}
impl FrozenAsset {
    fn root_filesystem(&self) -> Result<FilesystemKind, DomainError> {
        Ok(probe_directory(&self.root_path)?.filesystem_kind)
    }
}
fn active_batch_id(connection: &Connection) -> Result<Option<Vec<u8>>, DomainError> {
    connection.query_row("SELECT batch_id FROM deletion_batch WHERE batch_state NOT IN ('Completed','Cancelled') ORDER BY confirmed_at DESC LIMIT 1",(),|row|row.get(0)).optional().map_err(map_sqlite_error)
}
fn status_for_batch(
    connection: &Connection,
    batch_id: &[u8],
) -> Result<RecycleStatus, DomainError> {
    let (state,recycled,retryable,attention,cancelled,duplicate_files,duplicate_bytes):(String,i64,i64,i64,i64,i64,i64)=connection.query_row("SELECT batch.batch_state,SUM(asset.resolution_state='Recycled'),SUM(asset.resolution_state='RetryableFailed'),SUM(asset.resolution_state='NeedsAttention'),SUM(asset.resolution_state IN ('CancelledRemaining','ResolvedWithoutDelete')),SUM(asset.resolution_state='Recycled' AND mark.mark_scope='DuplicateSubset'),COALESCE(SUM(CASE WHEN asset.resolution_state='Recycled' AND mark.mark_scope='DuplicateSubset' THEN identity.size_bytes ELSE 0 END),0) FROM deletion_batch batch JOIN deletion_batch_photo photo ON photo.batch_id=batch.batch_id JOIN deletion_mark mark ON mark.mark_id=photo.mark_id JOIN deletion_batch_asset asset ON asset.batch_photo_id=photo.batch_photo_id JOIN deletion_mark_asset_snapshot snapshot ON snapshot.mark_asset_snapshot_id=asset.mark_asset_snapshot_id JOIN file_identity_snapshot identity ON identity.snapshot_id=snapshot.file_identity_snapshot_id WHERE batch.batch_id=?1 GROUP BY batch.batch_id",[batch_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?))).map_err(map_sqlite_error)?;
    let phase = match state.as_str() {
        "Completed" => RecyclePhase::Completed,
        "Cancelled" => RecyclePhase::Cancelled,
        "NeedsAttention" => RecyclePhase::NeedsAttention,
        "CompletedWithErrors" => RecyclePhase::AwaitingUser,
        "Running" | "CancelRequested" => RecyclePhase::Running,
        _ => RecyclePhase::Idle,
    };
    let reconciliation:Option<StoredReconciliation>=connection.query_row("SELECT reconciliation_state,source_run_id,removed_asset_count,removed_bytes,rejection_reason FROM duplicate_result_reconciliation WHERE recycle_batch_id=?1 AND reconciliation_state!='Pending' ORDER BY completed_at DESC LIMIT 1",[batch_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional().map_err(map_sqlite_error)?;
    let (
        duplicate_reconciliation,
        run_id,
        reconciled_files,
        reconciled_bytes,
        reconciliation_message,
    ) = match reconciliation {
        Some((state, run, files, bytes, _reason)) if state == "Applied" => (
            DuplicateReconciliationPhase::Applied,
            run.map(|value| uuid_string(&value)).transpose()?,
            files,
            bytes,
            Some("图库索引和重复结果已更新，无须重新检测。".into()),
        ),
        Some((_, run, files, bytes, reason)) => (
            DuplicateReconciliationPhase::Rejected,
            run.map(|value| uuid_string(&value)).transpose()?,
            files,
            bytes,
            reason,
        ),
        None if duplicate_files > 0 => (
            DuplicateReconciliationPhase::Pending,
            None,
            duplicate_files,
            duplicate_bytes,
            None,
        ),
        None => (
            DuplicateReconciliationPhase::NotApplicable,
            None,
            0,
            0,
            None,
        ),
    };
    Ok(RecycleStatus {
        phase,
        batch_id: Some(uuid_string(batch_id)?),
        recycled_files: recycled as usize,
        retryable_files: retryable as usize,
        needs_attention_files: attention as usize,
        cancelled_files: cancelled as usize,
        duplicate_reconciliation,
        successful_duplicate_file_count: reconciled_files as usize,
        successful_duplicate_bytes: reconciled_bytes,
        remaining_problem_count: (retryable + attention) as usize,
        duplicate_review_run_id: run_id,
        reconciliation_message,
        message: Some(
            match phase {
                RecyclePhase::Completed => "全部文件已移入 Windows 回收站。",
                RecyclePhase::NeedsAttention => "存在结果不明项；已停止并需要重新扫描。",
                RecyclePhase::AwaitingUser => "部分文件仍在原位，可重试或取消剩余删除。",
                RecyclePhase::Cancelled => "剩余删除意图已取消；历史结果已保留。",
                RecyclePhase::Running => "正在逐张移入 Windows 回收站。",
                RecyclePhase::Idle => "没有活动回收站任务。",
            }
            .into(),
        ),
    })
}
fn release_batch_marks(
    transaction: &Transaction<'_>,
    batch_id: &[u8],
    operation: &[u8],
    reason: &str,
    now: i64,
) -> Result<(), DomainError> {
    transaction.execute("UPDATE deletion_mark SET released_at=?2,released_operation_id=?3,release_reason=?4 WHERE mark_id IN (SELECT mark_id FROM deletion_batch_photo WHERE batch_id=?1) AND released_at IS NULL",params![batch_id,now,operation,reason]).map_err(map_sqlite_error)?;
    Ok(())
}
fn insert_operation(
    transaction: &Transaction<'_>,
    operation: &[u8],
    kind: &str,
    target: &[u8],
) -> Result<(), DomainError> {
    let mut hash = Sha256::new();
    hash.update(kind.as_bytes());
    hash.update(target);
    transaction.execute("INSERT INTO operation_event(operation_id,operation_kind,target_id,request_hash,first_received_at,final_result_code,related_entity_id) VALUES(?1,?2,?3,?4,?5,'Committed',?3)",params![operation,kind,target,hash.finalize().to_vec(),now_micros()]).map_err(map_sqlite_error)?;
    Ok(())
}
fn new_id() -> Vec<u8> {
    Uuid::new_v4().as_bytes().to_vec()
}
fn uuid_string(bytes: &[u8]) -> Result<String, DomainError> {
    Uuid::from_slice(bytes)
        .map(|id| id.to_string())
        .map_err(|_| DomainError::new(ErrorCode::DatabaseCorrupt, "删除任务 ID 损坏。"))
}
fn lock_error<T>(_: std::sync::PoisonError<T>) -> DomainError {
    DomainError::new(ErrorCode::Internal, "回收站服务状态不可用。")
}

#[cfg(test)]
mod tests;
