use std::{
    str::FromStr,
    sync::{Arc, Mutex, TryLockError},
};

use rusqlite::{OptionalExtension, Transaction};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    catalog::SortMode,
    db::{
        Database,
        migration::{map_sqlite_error, now_micros},
    },
    domain::{DeletionMarkId, DomainError, ErrorCode, FileSnapshotId, OperationId, PhotoId},
    roots::{Capability, CapabilitySet},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MarkReviewState {
    Marked,
    Problem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ProblemLifecycle {
    Open,
    Resolved,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ProblemKind {
    RootOffline,
    PhotoVersionChanged,
    MembersChanged,
    FileMissing,
    FileIdentityChanged,
    UnsupportedHardLink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletionProblem {
    pub lifecycle: ProblemLifecycle,
    pub kind: ProblemKind,
    pub message: String,
    pub can_rescan: bool,
    pub can_rebind: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingDeleteSummary {
    pub total_count: usize,
    pub executable_count: usize,
    pub problem_count: usize,
    pub needs_attention_count: usize,
    pub executable_file_count: usize,
    pub executable_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPreviewReference {
    pub asset_id: String,
    pub asset_version: i64,
    pub media_kind: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingDeleteItem {
    pub photo_id: String,
    pub photo_version: i64,
    pub file_name: String,
    pub capture_local_key: Option<String>,
    pub preview: Option<PendingPreviewReference>,
    pub jpeg_present: bool,
    pub raf_present: bool,
    pub relative_directory: String,
    pub physical_file_count: usize,
    pub total_size_bytes: i64,
    pub problem: Option<DeletionProblem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingDeleteSnapshot {
    pub summary: PendingDeleteSummary,
    pub items: Vec<PendingDeleteItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletionMutationResult {
    pub photo_id: String,
    pub marked: bool,
    pub summary: PendingDeleteSummary,
    pub undone_operation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionEffect {
    Marked,
    Unmarked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionAction {
    operation_id: OperationId,
    photo_id: PhotoId,
    mark_id: DeletionMarkId,
    frozen_photo_version: i64,
    effect: SessionEffect,
}

#[derive(Debug)]
pub struct PendingDeleteService {
    database: Arc<Database>,
    write_gate: Mutex<()>,
    session_actions: Mutex<Vec<SessionAction>>,
}

impl PendingDeleteService {
    pub fn new(database: Arc<Database>) -> Self {
        Self {
            database,
            write_gate: Mutex::new(()),
            session_actions: Mutex::new(Vec::new()),
        }
    }

    pub fn snapshot(&self, sort_mode: SortMode) -> Result<PendingDeleteSnapshot, DomainError> {
        load_pending_snapshot(&self.database, sort_mode)
    }

    pub fn summary(&self) -> Result<PendingDeleteSummary, DomainError> {
        Ok(self.snapshot(SortMode::CaptureDesc)?.summary)
    }

    pub fn toggle(
        &self,
        photo_id: PhotoId,
        photo_version: i64,
        operation_id: OperationId,
    ) -> Result<DeletionMutationResult, DomainError> {
        let _gate = self.try_write_gate()?;
        let request_hash = request_hash("ToggleDeletionMark", photo_id, photo_version, None);
        let (action, created_now) = self.database.writer()?.transaction(move |transaction| {
            if let Some(existing) = existing_operation(transaction, operation_id, &request_hash)? {
                return Ok((
                    action_from_existing(transaction, operation_id, photo_id, existing)?,
                    false,
                ));
            }

            let active_mark = active_mark(transaction, photo_id)?;
            let action = if let Some((mark_id, frozen_version)) = active_mark {
                insert_operation(
                    transaction,
                    operation_id,
                    "ToggleDeletionMark",
                    photo_id,
                    request_hash,
                    "Unmarked",
                    mark_id.as_bytes(),
                )?;
                release_mark(transaction, mark_id, operation_id, "UserToggle")?;
                SessionAction {
                    operation_id,
                    photo_id,
                    mark_id,
                    frozen_photo_version: frozen_version,
                    effect: SessionEffect::Unmarked,
                }
            } else {
                let mark_id = create_mark(
                    transaction,
                    photo_id,
                    photo_version,
                    operation_id,
                    request_hash,
                    "ToggleDeletionMark",
                    "Marked",
                )?;
                SessionAction {
                    operation_id,
                    photo_id,
                    mark_id,
                    frozen_photo_version: photo_version,
                    effect: SessionEffect::Marked,
                }
            };
            Ok((action, true))
        })?;

        let mut history = self.session_actions.lock().map_err(lock_error)?;
        if created_now {
            history.push(action);
        }
        drop(history);
        Ok(DeletionMutationResult {
            photo_id: photo_id.to_string(),
            marked: action.effect == SessionEffect::Marked,
            summary: self.summary()?,
            undone_operation_id: None,
        })
    }

    pub fn undo(&self, operation_id: OperationId) -> Result<DeletionMutationResult, DomainError> {
        let _gate = self.try_write_gate()?;
        if let Some((photo_id, marked)) = self.replayed_undo(operation_id)? {
            return Ok(DeletionMutationResult {
                photo_id: photo_id.to_string(),
                marked,
                summary: self.summary()?,
                undone_operation_id: None,
            });
        }
        let action = self
            .session_actions
            .lock()
            .map_err(lock_error)?
            .last()
            .copied()
            .ok_or_else(|| {
                DomainError::new(
                    ErrorCode::InvalidTransition,
                    "当前会话没有可撤销的待删除标记操作。",
                )
            })?;
        let request_hash = request_hash(
            "UndoDeletionMark",
            action.photo_id,
            action.frozen_photo_version,
            Some(action.operation_id),
        );

        let marked = self.database.writer()?.transaction(move |transaction| {
            if let Some(existing) = existing_operation(transaction, operation_id, &request_hash)? {
                return match existing.as_str() {
                    "UndoMarked" => Ok(true),
                    "UndoUnmarked" => Ok(false),
                    _ => Err(DomainError::new(
                        ErrorCode::DuplicateOperation,
                        "重复操作标识的既有结果不属于撤销标记。",
                    )),
                };
            }

            match action.effect {
                SessionEffect::Marked => {
                    let current = active_mark(transaction, action.photo_id)?;
                    if current.map(|value| value.0) != Some(action.mark_id) {
                        return Err(DomainError::stale_version("DeletionMark"));
                    }
                    insert_operation(
                        transaction,
                        operation_id,
                        "UndoDeletionMark",
                        action.photo_id,
                        request_hash,
                        "UndoUnmarked",
                        action.mark_id.as_bytes(),
                    )?;
                    release_mark(transaction, action.mark_id, operation_id, "SessionUndo")?;
                    Ok(false)
                }
                SessionEffect::Unmarked => {
                    if active_mark(transaction, action.photo_id)?.is_some() {
                        return Err(DomainError::stale_version("DeletionMark"));
                    }
                    let new_mark_id = create_mark(
                        transaction,
                        action.photo_id,
                        action.frozen_photo_version,
                        operation_id,
                        request_hash,
                        "UndoDeletionMark",
                        "UndoMarked",
                    )?;
                    transaction
                        .execute(
                            "UPDATE operation_event SET related_entity_id=?1
                             WHERE operation_id=?2",
                            (
                                new_mark_id.as_bytes().as_slice(),
                                operation_id.as_bytes().as_slice(),
                            ),
                        )
                        .map_err(map_sqlite_error)?;
                    Ok(true)
                }
            }
        })?;

        let mut history = self.session_actions.lock().map_err(lock_error)?;
        if history.last().copied() == Some(action) {
            history.pop();
        }
        drop(history);
        Ok(DeletionMutationResult {
            photo_id: action.photo_id.to_string(),
            marked,
            summary: self.summary()?,
            undone_operation_id: Some(action.operation_id.to_string()),
        })
    }

    pub fn session_undo_count(&self) -> Result<usize, DomainError> {
        Ok(self.session_actions.lock().map_err(lock_error)?.len())
    }

    fn replayed_undo(
        &self,
        operation_id: OperationId,
    ) -> Result<Option<(PhotoId, bool)>, DomainError> {
        let connection = self.database.open_read_connection()?;
        let existing = connection
            .query_row(
                "SELECT operation_kind, target_id, final_result_code
                 FROM operation_event WHERE operation_id=?1",
                [operation_id.as_bytes().as_slice()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<Vec<u8>>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sqlite_error)?;
        let Some((kind, target, result)) = existing else {
            return Ok(None);
        };
        if kind != "UndoDeletionMark" {
            return Err(DomainError::new(
                ErrorCode::DuplicateOperation,
                "重复操作标识已用于其他请求，已拒绝执行。",
            ));
        }
        let photo_id = id_from_bytes::<PhotoId>(target.as_deref().ok_or_else(|| {
            DomainError::new(ErrorCode::DatabaseCorrupt, "撤销操作缺少目标照片。")
        })?)?;
        let marked = match result.as_deref() {
            Some("UndoMarked") => true,
            Some("UndoUnmarked") => false,
            _ => {
                return Err(DomainError::new(
                    ErrorCode::DatabaseCorrupt,
                    "撤销操作记录尚未完整终结。",
                ));
            }
        };
        Ok(Some((photo_id, marked)))
    }

    fn try_write_gate(&self) -> Result<std::sync::MutexGuard<'_, ()>, DomainError> {
        self.write_gate.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => DomainError::new(
                ErrorCode::DatabaseBusy,
                "上一项待删除标记仍在提交，请稍候再试。",
            ),
            TryLockError::Poisoned(_) => {
                DomainError::new(ErrorCode::Internal, "待删除标记串行器不可用。")
            }
        })
    }
}

fn lock_error<T>(_error: std::sync::PoisonError<T>) -> DomainError {
    DomainError::new(ErrorCode::Internal, "待删除会话状态不可用。")
}

fn request_hash(
    kind: &str,
    photo_id: PhotoId,
    photo_version: i64,
    target_operation_id: Option<OperationId>,
) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(kind.as_bytes());
    digest.update(photo_id.as_bytes());
    digest.update(photo_version.to_le_bytes());
    if let Some(target) = target_operation_id {
        digest.update(target.as_bytes());
    }
    digest.finalize().to_vec()
}

fn existing_operation(
    transaction: &Transaction<'_>,
    operation_id: OperationId,
    request_hash: &[u8],
) -> Result<Option<String>, DomainError> {
    let existing = transaction
        .query_row(
            "SELECT request_hash, final_result_code FROM operation_event WHERE operation_id=?1",
            [operation_id.as_bytes().as_slice()],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    match existing {
        None => Ok(None),
        Some((hash, result)) if hash == request_hash => result.map(Some).ok_or_else(|| {
            DomainError::new(ErrorCode::DatabaseCorrupt, "待删除操作记录尚未完整终结。")
        }),
        Some(_) => Err(DomainError::new(
            ErrorCode::DuplicateOperation,
            "重复操作标识携带了不同请求，已拒绝执行。",
        )),
    }
}

fn action_from_existing(
    transaction: &Transaction<'_>,
    operation_id: OperationId,
    photo_id: PhotoId,
    result: String,
) -> Result<SessionAction, DomainError> {
    let (mark_bytes, frozen_version): (Vec<u8>, i64) = transaction
        .query_row(
            "SELECT event.related_entity_id, mark.photo_version
             FROM operation_event event
             JOIN deletion_mark mark ON mark.mark_id=event.related_entity_id
             WHERE event.operation_id=?1 AND event.target_id=?2",
            (
                operation_id.as_bytes().as_slice(),
                photo_id.as_bytes().as_slice(),
            ),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_sqlite_error)?;
    let mark_id = id_from_bytes::<DeletionMarkId>(&mark_bytes)?;
    let effect = match result.as_str() {
        "Marked" => SessionEffect::Marked,
        "Unmarked" => SessionEffect::Unmarked,
        _ => {
            return Err(DomainError::new(
                ErrorCode::DuplicateOperation,
                "重复操作标识的既有结果不属于切换标记。",
            ));
        }
    };
    Ok(SessionAction {
        operation_id,
        photo_id,
        mark_id,
        frozen_photo_version: frozen_version,
        effect,
    })
}

fn active_mark(
    transaction: &Transaction<'_>,
    photo_id: PhotoId,
) -> Result<Option<(DeletionMarkId, i64)>, DomainError> {
    transaction
        .query_row(
            "SELECT mark_id, photo_version FROM deletion_mark
             WHERE photo_id=?1 AND released_at IS NULL",
            [photo_id.as_bytes().as_slice()],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?
        .map(|(bytes, version)| Ok((id_from_bytes::<DeletionMarkId>(&bytes)?, version)))
        .transpose()
}

fn insert_operation(
    transaction: &Transaction<'_>,
    operation_id: OperationId,
    operation_kind: &str,
    photo_id: PhotoId,
    request_hash: Vec<u8>,
    result: &str,
    related_entity_id: &[u8],
) -> Result<(), DomainError> {
    transaction
        .execute(
            "INSERT INTO operation_event(
                operation_id, operation_kind, target_id, request_hash,
                first_received_at, final_result_code, related_entity_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                operation_id.as_bytes().as_slice(),
                operation_kind,
                photo_id.as_bytes().as_slice(),
                request_hash,
                now_micros(),
                result,
                related_entity_id,
            ],
        )
        .map_err(map_sqlite_error)?;
    Ok(())
}

fn release_mark(
    transaction: &Transaction<'_>,
    mark_id: DeletionMarkId,
    operation_id: OperationId,
    reason: &str,
) -> Result<(), DomainError> {
    let changed = transaction
        .execute(
            "UPDATE deletion_mark
             SET released_at=?1, released_operation_id=?2, release_reason=?3
             WHERE mark_id=?4 AND released_at IS NULL",
            (
                now_micros(),
                operation_id.as_bytes().as_slice(),
                reason,
                mark_id.as_bytes().as_slice(),
            ),
        )
        .map_err(map_sqlite_error)?;
    if changed != 1 {
        return Err(DomainError::stale_version("DeletionMark"));
    }
    Ok(())
}

#[derive(Debug)]
struct CurrentMember {
    asset_id: Vec<u8>,
    asset_version: i64,
    role: String,
    root_id: Vec<u8>,
    root_binding_id: Vec<u8>,
    volume_id: Vec<u8>,
    relative_path: String,
    path_key: Vec<u8>,
    path_key_version: i64,
    file_id: Option<Vec<u8>>,
    size_bytes: i64,
    mtime_filetime: i64,
}

fn load_current_members(
    transaction: &Transaction<'_>,
    photo_id: PhotoId,
    expected_photo_version: i64,
) -> Result<Vec<CurrentMember>, DomainError> {
    let (current_version, lifecycle): (i64, String) = transaction
        .query_row(
            "SELECT photo_version, lifecycle FROM photo WHERE photo_id=?1",
            [photo_id.as_bytes().as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?
        .ok_or_else(|| DomainError::stale_version("Photo"))?;
    if lifecycle != "Active" || current_version != expected_photo_version {
        return Err(DomainError::stale_version("Photo"));
    }

    let mut statement = transaction
        .prepare(
            "SELECT asset.asset_id, asset.current_version, relation.role,
                    revision.root_id, revision.root_binding_id, binding.volume_id,
                    revision.relative_path, revision.path_key, revision.path_key_version,
                    revision.file_id, revision.size_bytes, revision.mtime_filetime,
                    asset.presence_state, asset.deletion_supported,
                    revision.hard_link_count, root.lifecycle, root.capability_mask,
                    root.current_binding_id, binding.binding_state
             FROM photo_asset relation
             JOIN asset ON asset.asset_id=relation.asset_id
             JOIN asset_revision revision
               ON revision.asset_id=asset.asset_id AND revision.asset_version=asset.current_version
             JOIN root ON root.root_id=revision.root_id
             JOIN root_binding binding ON binding.binding_id=revision.root_binding_id
             WHERE relation.photo_id=?1 AND relation.to_photo_version IS NULL
             ORDER BY relation.role",
        )
        .map_err(map_sqlite_error)?;
    let members = statement
        .query_map([photo_id.as_bytes().as_slice()], |row| {
            Ok((
                CurrentMember {
                    asset_id: row.get(0)?,
                    asset_version: row.get(1)?,
                    role: row.get(2)?,
                    root_id: row.get(3)?,
                    root_binding_id: row.get(4)?,
                    volume_id: row.get(5)?,
                    relative_path: row.get(6)?,
                    path_key: row.get(7)?,
                    path_key_version: row.get(8)?,
                    file_id: row.get(9)?,
                    size_bytes: row.get(10)?,
                    mtime_filetime: row.get(11)?,
                },
                row.get::<_, String>(12)?,
                row.get::<_, bool>(13)?,
                row.get::<_, i64>(14)?,
                row.get::<_, String>(15)?,
                row.get::<_, i64>(16)?,
                row.get::<_, Option<Vec<u8>>>(17)?,
                row.get::<_, String>(18)?,
            ))
        })
        .map_err(map_sqlite_error)?
        .map(|row| row.map_err(map_sqlite_error))
        .collect::<Result<Vec<_>, _>>()?;

    if members.is_empty() || members.len() > 2 {
        return Err(DomainError::stale_version("PhotoAsset"));
    }
    let mut result = Vec::with_capacity(members.len());
    for (
        member,
        presence,
        deletion_supported,
        hard_links,
        root_state,
        capability,
        binding,
        binding_state,
    ) in members
    {
        if root_state != "Online" {
            return Err(DomainError::new(
                ErrorCode::RootOffline,
                "照片文件夹离线时不能新增或恢复待删除标记。",
            ));
        }
        let recycle = CapabilitySet::from_bits(u16::try_from(capability).unwrap_or(0))
            .contains(Capability::Recycle);
        if binding.as_deref() != Some(member.root_binding_id.as_slice())
            || binding_state != "Active"
        {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "照片文件夹身份已变化，请重新扫描后确认。",
            ));
        }
        if presence != "Present" {
            return Err(DomainError::new(
                ErrorCode::IdentityMismatch,
                "照片文件缺失，不能新增或恢复待删除标记。",
            ));
        }
        if !deletion_supported || hard_links != 1 || !recycle {
            return Err(DomainError::new(
                ErrorCode::CapabilityDenied,
                "当前照片不支持安全删除，不能添加待删除标记。",
            ));
        }
        result.push(member);
    }
    Ok(result)
}

fn create_mark(
    transaction: &Transaction<'_>,
    photo_id: PhotoId,
    photo_version: i64,
    operation_id: OperationId,
    request_hash: Vec<u8>,
    operation_kind: &str,
    result_code: &str,
) -> Result<DeletionMarkId, DomainError> {
    let members = load_current_members(transaction, photo_id, photo_version)?;
    let mark_id = DeletionMarkId::new();
    insert_operation(
        transaction,
        operation_id,
        operation_kind,
        photo_id,
        request_hash,
        result_code,
        mark_id.as_bytes(),
    )?;
    let created_at = now_micros();
    transaction
        .execute(
            "INSERT INTO deletion_mark(
                mark_id, photo_id, photo_version, created_operation_id, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                mark_id.as_bytes().as_slice(),
                photo_id.as_bytes().as_slice(),
                photo_version,
                operation_id.as_bytes().as_slice(),
                created_at,
            ),
        )
        .map_err(map_sqlite_error)?;

    for member in members {
        let file_snapshot_id = FileSnapshotId::new();
        transaction
            .execute(
                "INSERT INTO file_identity_snapshot(
                    snapshot_id, root_id, root_binding_id, volume_id,
                    relative_path, path_key, path_key_version, file_id,
                    size_bytes, mtime_filetime, captured_at, evidence_level
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'Strong')",
                rusqlite::params![
                    file_snapshot_id.as_bytes().as_slice(),
                    member.root_id,
                    member.root_binding_id,
                    member.volume_id,
                    member.relative_path,
                    member.path_key,
                    member.path_key_version,
                    member.file_id,
                    member.size_bytes,
                    member.mtime_filetime,
                    created_at,
                ],
            )
            .map_err(map_sqlite_error)?;
        let mark_asset_snapshot_id = FileSnapshotId::new();
        transaction
            .execute(
                "INSERT INTO deletion_mark_asset_snapshot(
                    mark_asset_snapshot_id, mark_id, asset_id, asset_version,
                    role, file_identity_snapshot_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    mark_asset_snapshot_id.as_bytes().as_slice(),
                    mark_id.as_bytes().as_slice(),
                    member.asset_id,
                    member.asset_version,
                    member.role,
                    file_snapshot_id.as_bytes().as_slice(),
                ],
            )
            .map_err(map_sqlite_error)?;
    }
    Ok(mark_id)
}

#[derive(Debug)]
struct ReviewRow {
    mark_id: Vec<u8>,
    photo_id: Vec<u8>,
    frozen_photo_version: i64,
    current_photo_version: i64,
    photo_lifecycle: String,
    file_name: String,
    capture_local_key: Option<String>,
    asset_id: Option<Vec<u8>>,
    frozen_asset_version: Option<i64>,
    frozen_role: Option<String>,
    snapshot_root_id: Option<Vec<u8>>,
    snapshot_binding_id: Option<Vec<u8>>,
    snapshot_relative_path: Option<String>,
    snapshot_path_key: Option<Vec<u8>>,
    snapshot_path_key_version: Option<i64>,
    snapshot_file_id: Option<Vec<u8>>,
    snapshot_size: Option<i64>,
    snapshot_mtime: Option<i64>,
    root_state: Option<String>,
    root_current_binding: Option<Vec<u8>>,
    root_capability: Option<i64>,
    current_asset_version: Option<i64>,
    asset_presence: Option<String>,
    deletion_supported: Option<bool>,
    current_root_id: Option<Vec<u8>>,
    current_binding_id: Option<Vec<u8>>,
    current_relative_path: Option<String>,
    current_path_key: Option<Vec<u8>>,
    current_path_key_version: Option<i64>,
    current_file_id: Option<Vec<u8>>,
    current_size: Option<i64>,
    current_mtime: Option<i64>,
    hard_link_count: Option<i64>,
    current_role: Option<String>,
    preview_state: Option<String>,
    current_member_count: i64,
}

fn load_pending_snapshot(
    database: &Database,
    sort_mode: SortMode,
) -> Result<PendingDeleteSnapshot, DomainError> {
    let connection = database.open_read_connection()?;
    let direction = match sort_mode {
        SortMode::CaptureDesc => "DESC",
        SortMode::CaptureAsc => "ASC",
    };
    let sql = format!(
        "SELECT mark.mark_id, mark.photo_id, mark.photo_version,
                photo.photo_version, photo.lifecycle, photo.natural_name_display,
                photo.capture_local_key, snapshot.asset_id, snapshot.asset_version,
                snapshot.role, identity.root_id, identity.root_binding_id,
                identity.relative_path, identity.path_key, identity.path_key_version,
                identity.file_id, identity.size_bytes, identity.mtime_filetime,
                root.lifecycle, root.current_binding_id, root.capability_mask,
                asset.current_version, asset.presence_state, asset.deletion_supported,
                revision.root_id, revision.root_binding_id, revision.relative_path,
                revision.path_key, revision.path_key_version, revision.file_id,
                revision.size_bytes, revision.mtime_filetime, revision.hard_link_count,
                relation.role, metadata.preview_state,
                (SELECT COUNT(*) FROM photo_asset current_relation
                 WHERE current_relation.photo_id=mark.photo_id
                   AND current_relation.to_photo_version IS NULL)
         FROM deletion_mark mark
         JOIN photo ON photo.photo_id=mark.photo_id
         JOIN current_library current ON current.library_id=photo.library_id AND current.slot=1
         LEFT JOIN deletion_mark_asset_snapshot snapshot ON snapshot.mark_id=mark.mark_id
         LEFT JOIN file_identity_snapshot identity
           ON identity.snapshot_id=snapshot.file_identity_snapshot_id
         LEFT JOIN root ON root.root_id=identity.root_id
         LEFT JOIN asset ON asset.asset_id=snapshot.asset_id
         LEFT JOIN asset_revision revision
           ON revision.asset_id=asset.asset_id AND revision.asset_version=asset.current_version
         LEFT JOIN photo_asset relation
           ON relation.photo_id=mark.photo_id AND relation.asset_id=snapshot.asset_id
          AND relation.to_photo_version IS NULL
         LEFT JOIN asset_media_metadata metadata
           ON metadata.asset_id=asset.asset_id AND metadata.asset_version=asset.current_version
         WHERE mark.released_at IS NULL
         ORDER BY CASE photo.date_class WHEN 'Known' THEN 0 ELSE 1 END,
                  photo.capture_local_key {direction}, photo.natural_sort_key {direction},
                  photo.photo_id {direction}, snapshot.role"
    );
    let mut statement = connection.prepare(&sql).map_err(map_sqlite_error)?;
    let rows = statement
        .query_map((), |row| {
            Ok(ReviewRow {
                mark_id: row.get(0)?,
                photo_id: row.get(1)?,
                frozen_photo_version: row.get(2)?,
                current_photo_version: row.get(3)?,
                photo_lifecycle: row.get(4)?,
                file_name: row.get(5)?,
                capture_local_key: row.get(6)?,
                asset_id: row.get(7)?,
                frozen_asset_version: row.get(8)?,
                frozen_role: row.get(9)?,
                snapshot_root_id: row.get(10)?,
                snapshot_binding_id: row.get(11)?,
                snapshot_relative_path: row.get(12)?,
                snapshot_path_key: row.get(13)?,
                snapshot_path_key_version: row.get(14)?,
                snapshot_file_id: row.get(15)?,
                snapshot_size: row.get(16)?,
                snapshot_mtime: row.get(17)?,
                root_state: row.get(18)?,
                root_current_binding: row.get(19)?,
                root_capability: row.get(20)?,
                current_asset_version: row.get(21)?,
                asset_presence: row.get(22)?,
                deletion_supported: row.get(23)?,
                current_root_id: row.get(24)?,
                current_binding_id: row.get(25)?,
                current_relative_path: row.get(26)?,
                current_path_key: row.get(27)?,
                current_path_key_version: row.get(28)?,
                current_file_id: row.get(29)?,
                current_size: row.get(30)?,
                current_mtime: row.get(31)?,
                hard_link_count: row.get(32)?,
                current_role: row.get(33)?,
                preview_state: row.get(34)?,
                current_member_count: row.get(35)?,
            })
        })
        .map_err(map_sqlite_error)?
        .map(|row| row.map_err(map_sqlite_error))
        .collect::<Result<Vec<_>, _>>()?;

    let mut grouped: Vec<Vec<ReviewRow>> = Vec::new();
    for row in rows {
        if grouped
            .last()
            .is_none_or(|group| group[0].mark_id != row.mark_id)
        {
            grouped.push(Vec::new());
        }
        grouped.last_mut().expect("group exists").push(row);
    }

    let mut summary = PendingDeleteSummary::default();
    let mut items = Vec::with_capacity(grouped.len());
    for group in grouped {
        let item = review_item(group)?;
        summary.total_count += 1;
        if item.problem.is_some() {
            summary.problem_count += 1;
        } else {
            summary.executable_count += 1;
            summary.executable_file_count += item.physical_file_count;
            summary.executable_bytes = summary
                .executable_bytes
                .saturating_add(item.total_size_bytes);
        }
        items.push(item);
    }
    Ok(PendingDeleteSnapshot { summary, items })
}

fn review_item(rows: Vec<ReviewRow>) -> Result<PendingDeleteItem, DomainError> {
    let first = rows
        .first()
        .ok_or_else(|| DomainError::new(ErrorCode::DatabaseCorrupt, "待删除标记缺少成员快照。"))?;
    let photo_id = id_from_bytes::<PhotoId>(&first.photo_id)?;
    let snapshot_count = rows.iter().filter(|row| row.asset_id.is_some()).count();
    let total_size_bytes = rows
        .iter()
        .filter_map(|row| row.snapshot_size)
        .fold(0_i64, i64::saturating_add);
    let relative_directory = rows
        .iter()
        .find_map(|row| row.snapshot_relative_path.as_deref())
        .map(relative_parent)
        .unwrap_or_else(|| "根目录".to_string());
    let jpeg_present = rows.iter().any(|row| {
        row.current_role.as_deref() == Some("Jpg")
            && row.asset_presence.as_deref() == Some("Present")
    });
    let raf_present = rows.iter().any(|row| {
        row.current_role.as_deref() == Some("Raf")
            && row.asset_presence.as_deref() == Some("Present")
    });
    let preview = rows
        .iter()
        .filter_map(|row| {
            Some(PendingPreviewReference {
                asset_id: id_from_bytes::<crate::domain::AssetId>(row.asset_id.as_deref()?)
                    .ok()?
                    .to_string(),
                asset_version: row.current_asset_version?,
                media_kind: row.current_role.clone()?,
                state: row.preview_state.clone()?,
            })
        })
        .min_by_key(|preview| (preview.state == "Unavailable", preview.media_kind != "Jpg"));

    let problem = classify_problem(&rows, snapshot_count);
    Ok(PendingDeleteItem {
        photo_id: photo_id.to_string(),
        photo_version: first.current_photo_version,
        file_name: first.file_name.clone(),
        capture_local_key: first.capture_local_key.clone(),
        preview,
        jpeg_present,
        raf_present,
        relative_directory,
        physical_file_count: snapshot_count,
        total_size_bytes,
        problem,
    })
}

fn classify_problem(rows: &[ReviewRow], snapshot_count: usize) -> Option<DeletionProblem> {
    let first = &rows[0];
    if rows
        .iter()
        .any(|row| row.root_state.as_deref() != Some("Online"))
    {
        return Some(problem(
            ProblemKind::RootOffline,
            "照片文件夹离线；标记已保留，但不能重新标记或执行删除。",
            false,
            true,
        ));
    }
    if rows.iter().any(|row| {
        row.current_asset_version.is_none() || row.asset_presence.as_deref() != Some("Present")
    }) {
        return Some(problem(
            ProblemKind::FileMissing,
            "一个或多个照片文件已缺失，当前标记不能执行。",
            true,
            false,
        ));
    }
    if snapshot_count == 0
        || usize::try_from(first.current_member_count).ok() != Some(snapshot_count)
        || rows.iter().any(|row| row.current_role != row.frozen_role)
    {
        return Some(problem(
            ProblemKind::MembersChanged,
            "JPG/RAF 成员关系在标记后发生变化，需要重新确认。",
            true,
            false,
        ));
    }
    if rows.iter().any(|row| {
        !row.deletion_supported.unwrap_or(false)
            || row.hard_link_count != Some(1)
            || !CapabilitySet::from_bits(
                u16::try_from(row.root_capability.unwrap_or_default()).unwrap_or(0),
            )
            .contains(Capability::Recycle)
    }) {
        return Some(problem(
            ProblemKind::UnsupportedHardLink,
            "文件存在多个目录入口或不支持安全删除。",
            true,
            false,
        ));
    }
    if rows.iter().any(|row| {
        row.current_asset_version != row.frozen_asset_version
            || row.current_root_id != row.snapshot_root_id
            || row.current_binding_id != row.snapshot_binding_id
            || row.root_current_binding != row.snapshot_binding_id
            || row.current_relative_path != row.snapshot_relative_path
            || row.current_path_key != row.snapshot_path_key
            || row.current_path_key_version != row.snapshot_path_key_version
            || row.current_file_id != row.snapshot_file_id
            || row.current_size != row.snapshot_size
            || row.current_mtime != row.snapshot_mtime
    }) {
        return Some(problem(
            ProblemKind::FileIdentityChanged,
            "文件身份、路径、大小、时间或对象版本在标记后发生变化。",
            true,
            false,
        ));
    }
    if first.photo_lifecycle != "Active"
        || first.current_photo_version != first.frozen_photo_version
    {
        return Some(problem(
            ProblemKind::PhotoVersionChanged,
            "照片版本在标记后发生变化，需要取消失效标记并重新确认。",
            true,
            false,
        ));
    }
    None
}

fn problem(
    kind: ProblemKind,
    message: &str,
    can_rescan: bool,
    can_rebind: bool,
) -> DeletionProblem {
    DeletionProblem {
        lifecycle: ProblemLifecycle::Open,
        kind,
        message: message.to_string(),
        can_rescan,
        can_rebind,
    }
}

pub fn derive_problem_lifecycle(
    mark_is_active: bool,
    mark_is_valid: bool,
    has_newer_active_mark: bool,
) -> Option<ProblemLifecycle> {
    if mark_is_valid {
        None
    } else if mark_is_active {
        Some(ProblemLifecycle::Open)
    } else if has_newer_active_mark {
        Some(ProblemLifecycle::Superseded)
    } else {
        Some(ProblemLifecycle::Resolved)
    }
}

fn relative_parent(path: &str) -> String {
    path.rsplit_once(['\\', '/'])
        .map(|(parent, _)| parent.to_string())
        .filter(|parent| !parent.is_empty())
        .unwrap_or_else(|| "根目录".to_string())
}

fn id_from_bytes<T: FromStr<Err = DomainError>>(bytes: &[u8]) -> Result<T, DomainError> {
    let uuid = uuid::Uuid::from_slice(bytes)
        .map_err(|_| DomainError::new(ErrorCode::DatabaseCorrupt, "数据库对象标识损坏。"))?;
    T::from_str(&uuid.to_string())
}

#[cfg(test)]
mod tests;
