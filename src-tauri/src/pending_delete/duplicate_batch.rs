use std::{
    collections::{BTreeMap, HashSet},
    panic::{AssertUnwindSafe, catch_unwind},
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    DuplicateSubsetSelection, PendingDeleteService, ResolvedDuplicateSelection, active_mark,
    create_duplicate_subset_mark, duplicate_subset_request_hash, existing_operation, id_from_bytes,
    load_duplicate_selection, lock_error,
};
use crate::{
    domain::{DeletionMarkId, DomainError, ErrorCode, OperationId, PhotoId},
    duplicate_review::{StablePairProofInput, stable_pair_stream_proof},
};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartDuplicatePendingBatchRequest {
    pub run_id: String,
    pub operation_id: String,
    pub selections: Vec<DuplicateSubsetSelection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DuplicatePendingBatchPhase {
    Validating,
    Committing,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicatePendingBatchFailure {
    pub code: ErrorCode,
    pub message: String,
    pub deleted_path: Option<String>,
    pub retained_path: Option<String>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicatePendingBatchStatus {
    pub task_id: String,
    pub operation_id: String,
    pub phase: DuplicatePendingBatchPhase,
    pub cancellation_requested: bool,
    pub total_files: usize,
    pub validated_files: usize,
    pub total_bytes: i64,
    pub validated_bytes: i64,
    pub current_deleted_path: Option<String>,
    pub current_retained_path: Option<String>,
    pub marked_photos: usize,
    pub marked_files: usize,
    pub marked_bytes: i64,
    pub failure: Option<DuplicatePendingBatchFailure>,
}

impl DuplicatePendingBatchStatus {
    pub fn can_cancel(&self) -> bool {
        self.phase == DuplicatePendingBatchPhase::Validating && !self.cancellation_requested
    }
}

#[derive(Debug, Default)]
pub(crate) struct DuplicateBatchCoordinator {
    state: Mutex<Option<DuplicatePendingBatchStatus>>,
    cancellation: Mutex<Option<Arc<AtomicBool>>>,
}

impl DuplicateBatchCoordinator {
    fn begin(
        &self,
        status: DuplicatePendingBatchStatus,
        cancellation: Arc<AtomicBool>,
    ) -> Result<(), DomainError> {
        let mut state = self.state.lock().map_err(lock_error)?;
        if state.as_ref().is_some_and(|current| {
            matches!(
                current.phase,
                DuplicatePendingBatchPhase::Validating | DuplicatePendingBatchPhase::Committing
            )
        }) {
            return Err(DomainError::new(
                ErrorCode::DatabaseBusy,
                "上一批重复文件仍在处理，请稍候再试。",
            ));
        }
        *state = Some(status);
        *self.cancellation.lock().map_err(lock_error)? = Some(cancellation);
        Ok(())
    }

    fn update(
        &self,
        task_id: &str,
        update: impl FnOnce(&mut DuplicatePendingBatchStatus),
    ) -> Result<(), DomainError> {
        let mut state = self.state.lock().map_err(lock_error)?;
        let current = state.as_mut().ok_or_else(no_batch_error)?;
        if current.task_id != task_id {
            return Err(DomainError::new(
                ErrorCode::StaleVersion,
                "批量任务已被更新，请重新打开确认窗口。",
            ));
        }
        update(current);
        Ok(())
    }

    fn finish(&self, task_id: &str, update: impl FnOnce(&mut DuplicatePendingBatchStatus)) {
        let _ = self.update(task_id, update);
        if let Ok(mut cancellation) = self.cancellation.lock() {
            *cancellation = None;
        }
    }

    fn status(&self) -> Result<DuplicatePendingBatchStatus, DomainError> {
        self.state
            .lock()
            .map_err(lock_error)?
            .clone()
            .ok_or_else(no_batch_error)
    }

    fn cancel(&self) -> Result<DuplicatePendingBatchStatus, DomainError> {
        let mut state = self.state.lock().map_err(lock_error)?;
        let current = state.as_mut().ok_or_else(no_batch_error)?;
        if current.phase == DuplicatePendingBatchPhase::Validating {
            current.cancellation_requested = true;
            if let Some(flag) = self.cancellation.lock().map_err(lock_error)?.as_ref() {
                flag.store(true, Ordering::Release);
            }
        }
        Ok(current.clone())
    }
}

impl PendingDeleteService {
    pub fn start_duplicate_pending_batch(
        self: &Arc<Self>,
        request: StartDuplicatePendingBatchRequest,
    ) -> Result<DuplicatePendingBatchStatus, DomainError> {
        if request.selections.is_empty() {
            return Err(DomainError::new(
                ErrorCode::InvalidTransition,
                "至少选择一个重复文件。",
            ));
        }
        uuid::Uuid::parse_str(&request.run_id)
            .map_err(|_| DomainError::new(ErrorCode::InvalidId, "重复检测 Run ID 无效。"))?;
        OperationId::from_str(&request.operation_id)?;
        let task_id = uuid::Uuid::new_v4().to_string();
        let cancellation = Arc::new(AtomicBool::new(false));
        let status = DuplicatePendingBatchStatus {
            task_id: task_id.clone(),
            operation_id: request.operation_id.clone(),
            phase: DuplicatePendingBatchPhase::Validating,
            cancellation_requested: false,
            total_files: request.selections.len(),
            validated_files: 0,
            total_bytes: 0,
            validated_bytes: 0,
            current_deleted_path: None,
            current_retained_path: None,
            marked_photos: 0,
            marked_files: 0,
            marked_bytes: 0,
            failure: None,
        };
        self.duplicate_batch
            .begin(status.clone(), cancellation.clone())?;

        let service = Arc::clone(self);
        let worker_task_id = task_id.clone();
        std::thread::Builder::new()
            .name("duplicate-pending-batch".to_owned())
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    run_batch(&service, &worker_task_id, request, cancellation)
                }));
                match outcome {
                    Ok(Ok(())) => {}
                    Ok(Err(failure)) => service.duplicate_batch.finish(&worker_task_id, |status| {
                        status.phase = DuplicatePendingBatchPhase::Failed;
                        status.failure = Some(failure);
                        status.current_deleted_path = None;
                        status.current_retained_path = None;
                    }),
                    Err(_) => service.duplicate_batch.finish(&worker_task_id, |status| {
                        status.phase = DuplicatePendingBatchPhase::Failed;
                        status.failure = Some(DuplicatePendingBatchFailure {
                            code: ErrorCode::Internal,
                            message: "批量验证任务意外终止，请重新确认整批。".to_owned(),
                            deleted_path: status.current_deleted_path.clone(),
                            retained_path: status.current_retained_path.clone(),
                            diagnostic: Some("duplicate pending batch worker panicked".to_owned()),
                        });
                    }),
                }
            })
            .map_err(|error| {
                self.duplicate_batch.finish(&task_id, |status| {
                    status.phase = DuplicatePendingBatchPhase::Failed;
                });
                DomainError::new(ErrorCode::Internal, "无法启动批量验证任务。")
                    .with_diagnostic(error.to_string())
            })?;
        Ok(status)
    }

    pub fn duplicate_pending_batch_status(
        &self,
    ) -> Result<DuplicatePendingBatchStatus, DomainError> {
        self.duplicate_batch.status()
    }

    pub fn cancel_duplicate_pending_batch(
        &self,
    ) -> Result<DuplicatePendingBatchStatus, DomainError> {
        self.duplicate_batch.cancel()
    }
}

fn run_batch(
    service: &Arc<PendingDeleteService>,
    task_id: &str,
    request: StartDuplicatePendingBatchRequest,
    cancellation: Arc<AtomicBool>,
) -> Result<(), DuplicatePendingBatchFailure> {
    let run_id = uuid::Uuid::parse_str(&request.run_id).map_err(|error| {
        failure_from_error(
            DomainError::new(ErrorCode::InvalidId, "重复检测 Run ID 无效。")
                .with_diagnostic(error.to_string()),
        )
    })?;
    let batch_operation_id =
        OperationId::from_str(&request.operation_id).map_err(failure_from_error)?;
    let mut resolved = Vec::with_capacity(request.selections.len());
    for selection in &request.selections {
        if cancellation.load(Ordering::Acquire) {
            finish_cancelled(service, task_id);
            return Ok(());
        }
        resolved.push(
            load_duplicate_selection(&service.database, run_id, selection)
                .map_err(failure_from_error)?,
        );
    }
    normalize_selections(&mut resolved).map_err(failure_from_error)?;
    let batch_request_hash = duplicate_pending_batch_request_hash(run_id, &resolved);
    let total_bytes = resolved.iter().fold(0_i64, |total, selection| {
        total.saturating_add(
            i64::try_from(selection.deleted.expected.size_bytes)
                .unwrap_or(i64::MAX)
                .saturating_add(
                    i64::try_from(selection.retained.expected.size_bytes).unwrap_or(i64::MAX),
                ),
        )
    });
    service
        .duplicate_batch
        .update(task_id, |status| status.total_bytes = total_bytes)
        .map_err(failure_from_error)?;
    let marked_bytes = resolved.iter().fold(0_i64, |total, selection| {
        total.saturating_add(
            i64::try_from(selection.deleted.expected.size_bytes).unwrap_or(i64::MAX),
        )
    });
    let marked_photos = resolved
        .iter()
        .map(|selection| selection.deleted.photo_id)
        .collect::<HashSet<_>>()
        .len();
    if existing_batch_result(
        service,
        run_id,
        batch_operation_id,
        &batch_request_hash,
        &resolved,
    )
    .map_err(failure_from_error)?
    {
        service.duplicate_batch.finish(task_id, |status| {
            status.phase = DuplicatePendingBatchPhase::Completed;
            status.validated_files = status.total_files;
            status.validated_bytes = status.total_bytes;
            status.marked_photos = marked_photos;
            status.marked_files = status.total_files;
            status.marked_bytes = marked_bytes;
        });
        return Ok(());
    }
    preflight_active_marks(service, &resolved).map_err(failure_from_error)?;

    for selection in &mut resolved {
        if cancellation.load(Ordering::Acquire) {
            finish_cancelled(service, task_id);
            return Ok(());
        }
        let deleted_path = selection
            .deleted
            .root_path
            .join(&selection.deleted.relative_path)
            .display()
            .to_string();
        let retained_path = selection
            .retained
            .root_path
            .join(&selection.retained.relative_path)
            .display()
            .to_string();
        service
            .duplicate_batch
            .update(task_id, |status| {
                status.current_deleted_path = Some(deleted_path.clone());
                status.current_retained_path = Some(retained_path.clone());
            })
            .map_err(failure_from_error)?;
        let proof = stable_pair_stream_proof(
            &service.scheduler,
            &cancellation,
            StablePairProofInput {
                root: &selection.deleted.root_path,
                deleted_relative: &selection.deleted.relative_path,
                deleted_expected: &selection.deleted.expected,
                retained_relative: &selection.retained.relative_path,
                retained_expected: &selection.retained.expected,
            },
            |bytes| {
                let _ = service.duplicate_batch.update(task_id, |status| {
                    status.validated_bytes = status
                        .validated_bytes
                        .saturating_add(i64::try_from(bytes).unwrap_or(i64::MAX));
                });
            },
        )
        .map_err(|error| DuplicatePendingBatchFailure {
            code: error.code,
            message: error.user_message,
            deleted_path: Some(deleted_path.clone()),
            retained_path: Some(retained_path.clone()),
            diagnostic: error.diagnostic,
        });
        let proof = match proof {
            Ok(proof) => proof,
            Err(_) if cancellation.load(Ordering::Acquire) => {
                finish_cancelled(service, task_id);
                return Ok(());
            }
            Err(failure) => return Err(failure),
        };
        selection.reproof = Some(proof);
        service
            .duplicate_batch
            .update(task_id, |status| status.validated_files += 1)
            .map_err(failure_from_error)?;
    }

    if cancellation.load(Ordering::Acquire) {
        finish_cancelled(service, task_id);
        return Ok(());
    }
    let groups = group_by_photo(resolved);
    service
        .duplicate_batch
        .update(task_id, |status| {
            status.phase = DuplicatePendingBatchPhase::Committing;
            status.current_deleted_path = None;
            status.current_retained_path = None;
        })
        .map_err(failure_from_error)?;

    let _gate = service.try_write_gate().map_err(failure_from_error)?;
    let marked_files = groups.values().map(Vec::len).sum::<usize>();
    let marked_bytes = groups.values().flatten().fold(0_i64, |total, selection| {
        total.saturating_add(
            i64::try_from(selection.deleted.expected.size_bytes).unwrap_or(i64::MAX),
        )
    });
    let marked_photos = groups.len();
    service
        .database
        .writer()
        .map_err(failure_from_error)?
        .transaction(move |transaction| {
            commit_groups(
                transaction,
                run_id,
                batch_operation_id,
                batch_request_hash,
                groups,
            )
        })
        .map_err(failure_from_error)?;

    service.duplicate_batch.finish(task_id, |status| {
        status.phase = DuplicatePendingBatchPhase::Completed;
        status.marked_photos = marked_photos;
        status.marked_files = marked_files;
        status.marked_bytes = marked_bytes;
        status.validated_bytes = status.total_bytes;
    });
    Ok(())
}

fn normalize_selections(resolved: &mut [ResolvedDuplicateSelection]) -> Result<(), DomainError> {
    resolved.sort_by_key(|selection| {
        (
            selection.deleted.photo_id,
            selection.deleted.photo_version,
            selection.deleted.asset_id,
            selection.retained.asset_id,
        )
    });
    let mut deleted = HashSet::new();
    for selection in resolved.iter() {
        if !deleted.insert(selection.deleted.asset_id) {
            return Err(DomainError::new(
                ErrorCode::InvalidTransition,
                "同一个待删文件在整批中被重复选择。",
            ));
        }
    }
    if resolved
        .iter()
        .any(|selection| deleted.contains(&selection.retained.asset_id))
    {
        return Err(DomainError::new(
            ErrorCode::InvalidTransition,
            "整批中的待删文件不能同时作为另一项的保留副本。",
        ));
    }
    Ok(())
}

fn preflight_active_marks(
    service: &PendingDeleteService,
    resolved: &[ResolvedDuplicateSelection],
) -> Result<(), DomainError> {
    let connection = service.database.open_read_connection()?;
    for selection in resolved.iter() {
        let has_mark: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM deletion_mark WHERE photo_id=?1 AND released_at IS NULL)",
                [selection.deleted.photo_id.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .map_err(crate::db::migration::map_sqlite_error)?;
        if has_mark {
            return Err(DomainError::new(
                ErrorCode::InvalidTransition,
                "整批中有照片已存在待删除标记，请先处理原标记。",
            ));
        }
    }
    Ok(())
}

fn existing_batch_result(
    service: &PendingDeleteService,
    run_id: uuid::Uuid,
    batch_operation_id: OperationId,
    batch_request_hash: &[u8],
    resolved: &[ResolvedDuplicateSelection],
) -> Result<bool, DomainError> {
    type ExistingOperationRow = (Vec<u8>, Option<String>, Option<Vec<u8>>);
    let mut groups = BTreeMap::<(PhotoId, i64), Vec<&ResolvedDuplicateSelection>>::new();
    for selection in resolved {
        groups
            .entry((selection.deleted.photo_id, selection.deleted.photo_version))
            .or_default()
            .push(selection);
    }
    let connection = service.database.open_read_connection()?;
    let parent: Option<(Vec<u8>, Option<String>)> = connection
        .query_row(
            "SELECT request_hash,final_result_code FROM operation_event WHERE operation_id=?1",
            [batch_operation_id.as_bytes().as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(crate::db::migration::map_sqlite_error)?;
    let parent_exists = match parent {
        None => false,
        Some((hash, result))
            if hash == batch_request_hash
                && result.as_deref() == Some("MarkedDuplicatePendingBatch") =>
        {
            true
        }
        Some(_) => {
            return Err(DomainError::new(
                ErrorCode::DuplicateOperation,
                "批量操作标识已用于不同的整批请求，已拒绝执行。",
            ));
        }
    };
    let mut existing = 0;
    for ((photo_id, _), selections) in &groups {
        let operation_id = child_operation_id(batch_operation_id, *photo_id);
        let request_hash = duplicate_subset_request_hash_refs(run_id, selections);
        let row: Option<ExistingOperationRow> = connection
            .query_row(
                "SELECT request_hash,final_result_code,related_entity_id
                 FROM operation_event WHERE operation_id=?1",
                [operation_id.as_bytes().as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(crate::db::migration::map_sqlite_error)?;
        if let Some((hash, result, related)) = row {
            if hash != request_hash
                || result.as_deref() != Some("MarkedDuplicateSubset")
                || related.as_deref().is_none_or(|value| value.len() != 16)
            {
                return Err(DomainError::new(
                    ErrorCode::DuplicateOperation,
                    "批量操作标识已用于不同请求，已拒绝执行。",
                ));
            }
            existing += 1;
        }
    }
    if !parent_exists && existing == 0 {
        Ok(false)
    } else if parent_exists && existing == groups.len() {
        Ok(true)
    } else {
        Err(DomainError::new(
            ErrorCode::DuplicateOperation,
            "批量操作记录不完整，已拒绝补写；请保留数据库并联系支持。",
        ))
    }
}

fn duplicate_pending_batch_request_hash(
    run_id: uuid::Uuid,
    selections: &[ResolvedDuplicateSelection],
) -> Vec<u8> {
    let references = selections.iter().collect::<Vec<_>>();
    let mut digest = Sha256::new();
    digest.update(b"AddDuplicatePendingBatch/v1");
    digest.update(run_id.as_bytes());
    for selection in references {
        digest.update(&selection.set_id);
        digest.update(selection.deleted.asset_id.as_bytes());
        digest.update(selection.deleted.asset_version.to_le_bytes());
        digest.update(selection.retained.asset_id.as_bytes());
        digest.update(selection.retained.asset_version.to_le_bytes());
    }
    digest.finalize().to_vec()
}

fn duplicate_subset_request_hash_refs(
    run_id: uuid::Uuid,
    selections: &[&ResolvedDuplicateSelection],
) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(b"AddDuplicateSubset");
    digest.update(run_id.as_bytes());
    for selection in selections {
        digest.update(&selection.set_id);
        digest.update(selection.deleted.asset_id.as_bytes());
        digest.update(selection.deleted.asset_version.to_le_bytes());
        digest.update(selection.retained.asset_id.as_bytes());
        digest.update(selection.retained.asset_version.to_le_bytes());
    }
    digest.finalize().to_vec()
}

fn group_by_photo(
    resolved: Vec<ResolvedDuplicateSelection>,
) -> BTreeMap<(PhotoId, i64), Vec<ResolvedDuplicateSelection>> {
    let mut groups = BTreeMap::new();
    for selection in resolved {
        groups
            .entry((selection.deleted.photo_id, selection.deleted.photo_version))
            .or_insert_with(Vec::new)
            .push(selection);
    }
    groups
}

fn commit_groups(
    transaction: &rusqlite::Transaction<'_>,
    run_id: uuid::Uuid,
    batch_operation_id: OperationId,
    batch_request_hash: Vec<u8>,
    groups: BTreeMap<(PhotoId, i64), Vec<ResolvedDuplicateSelection>>,
) -> Result<Vec<DeletionMarkId>, DomainError> {
    struct PendingGroup {
        photo_id: PhotoId,
        photo_version: i64,
        operation_id: OperationId,
        request_hash: Vec<u8>,
        selections: Vec<ResolvedDuplicateSelection>,
    }
    let mut pending = Vec::with_capacity(groups.len());
    let mut existing_marks = Vec::with_capacity(groups.len());
    let mut existing_count = 0;
    let parent_exists =
        match existing_operation(transaction, batch_operation_id, &batch_request_hash)? {
            None => false,
            Some(result) if result == "MarkedDuplicatePendingBatch" => true,
            Some(_) => {
                return Err(DomainError::new(
                    ErrorCode::DuplicateOperation,
                    "批量操作标识已用于其他请求。",
                ));
            }
        };
    for ((photo_id, photo_version), selections) in groups {
        let operation_id = child_operation_id(batch_operation_id, photo_id);
        let request_hash = duplicate_subset_request_hash(run_id, &selections);
        if let Some(result) = existing_operation(transaction, operation_id, &request_hash)? {
            if result != "MarkedDuplicateSubset" {
                return Err(DomainError::new(
                    ErrorCode::DuplicateOperation,
                    "批量子操作标识已用于其他请求。",
                ));
            }
            let bytes: Vec<u8> = transaction
                .query_row(
                    "SELECT related_entity_id FROM operation_event WHERE operation_id=?1",
                    [operation_id.as_bytes().as_slice()],
                    |row| row.get(0),
                )
                .map_err(crate::db::migration::map_sqlite_error)?;
            existing_marks.push(id_from_bytes::<DeletionMarkId>(&bytes)?);
            existing_count += 1;
        }
        pending.push(PendingGroup {
            photo_id,
            photo_version,
            operation_id,
            request_hash,
            selections,
        });
    }
    if (parent_exists && existing_count != pending.len()) || (!parent_exists && existing_count > 0)
    {
        return Err(DomainError::new(
            ErrorCode::DuplicateOperation,
            "批量操作记录不完整，已拒绝补写；请保留数据库并联系支持。",
        ));
    }
    if parent_exists && existing_count == pending.len() {
        return Ok(existing_marks);
    }
    transaction
        .execute(
            "INSERT INTO operation_event(
                operation_id,operation_kind,target_id,request_hash,
                first_received_at,final_result_code
             ) VALUES(?1,'AddDuplicatePendingBatch',?2,?3,?4,'MarkedDuplicatePendingBatch')",
            rusqlite::params![
                batch_operation_id.as_bytes().as_slice(),
                run_id.as_bytes().as_slice(),
                batch_request_hash,
                crate::db::migration::now_micros(),
            ],
        )
        .map_err(crate::db::migration::map_sqlite_error)?;
    let mut marks = Vec::with_capacity(pending.len());
    for group in pending {
        if active_mark(transaction, group.photo_id)?.is_some() {
            return Err(DomainError::new(
                ErrorCode::InvalidTransition,
                "提交前发现照片已有活动待删除标记，整批未提交。",
            ));
        }
        marks.push(create_duplicate_subset_mark(
            transaction,
            run_id,
            group.photo_id,
            group.photo_version,
            group.operation_id,
            group.request_hash,
            group.selections,
        )?);
    }
    Ok(marks)
}

fn child_operation_id(batch: OperationId, photo: PhotoId) -> OperationId {
    let mut digest = Sha256::new();
    digest.update(b"DuplicatePendingBatchPhoto/v1");
    digest.update(batch.as_bytes());
    digest.update(photo.as_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    OperationId::from_uuid(uuid::Uuid::from_bytes(bytes))
}

fn finish_cancelled(service: &PendingDeleteService, task_id: &str) {
    service.duplicate_batch.finish(task_id, |status| {
        status.phase = DuplicatePendingBatchPhase::Cancelled;
        status.current_deleted_path = None;
        status.current_retained_path = None;
        status.failure = None;
    });
}

fn failure_from_error(error: DomainError) -> DuplicatePendingBatchFailure {
    DuplicatePendingBatchFailure {
        code: error.code,
        message: error.user_message,
        deleted_path: None,
        retained_path: None,
        diagnostic: error.diagnostic,
    }
}

fn no_batch_error() -> DomainError {
    DomainError::new(
        ErrorCode::InvalidTransition,
        "当前没有可查询的重复文件批量任务。",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_operation_ids_are_stable_and_photo_scoped() {
        let batch = OperationId::new();
        let first = PhotoId::new();
        let second = PhotoId::new();
        assert_eq!(
            child_operation_id(batch, first),
            child_operation_id(batch, first)
        );
        assert_ne!(
            child_operation_id(batch, first),
            child_operation_id(batch, second)
        );
    }

    #[test]
    fn contract_serializes_terminal_phase_in_camel_case() {
        let status = DuplicatePendingBatchStatus {
            task_id: uuid::Uuid::new_v4().to_string(),
            operation_id: OperationId::new().to_string(),
            phase: DuplicatePendingBatchPhase::Completed,
            cancellation_requested: false,
            total_files: 2,
            validated_files: 2,
            total_bytes: 10,
            validated_bytes: 10,
            current_deleted_path: None,
            current_retained_path: None,
            marked_photos: 2,
            marked_files: 2,
            marked_bytes: 5,
            failure: None,
        };
        let json = serde_json::to_value(status).unwrap();
        assert_eq!(json["phase"], "completed");
        assert_eq!(json["markedPhotos"], 2);
    }

    #[test]
    fn validating_task_accepts_cancellation_request() {
        let coordinator = DuplicateBatchCoordinator::default();
        let cancellation = Arc::new(AtomicBool::new(false));
        let status = DuplicatePendingBatchStatus {
            task_id: uuid::Uuid::new_v4().to_string(),
            operation_id: OperationId::new().to_string(),
            phase: DuplicatePendingBatchPhase::Validating,
            cancellation_requested: false,
            total_files: 1,
            validated_files: 0,
            total_bytes: 1,
            validated_bytes: 0,
            current_deleted_path: None,
            current_retained_path: None,
            marked_photos: 0,
            marked_files: 0,
            marked_bytes: 0,
            failure: None,
        };
        coordinator
            .begin(status, Arc::clone(&cancellation))
            .unwrap();
        let cancelled = coordinator.cancel().unwrap();
        assert!(cancelled.cancellation_requested);
        assert!(cancellation.load(Ordering::Acquire));
    }
}
