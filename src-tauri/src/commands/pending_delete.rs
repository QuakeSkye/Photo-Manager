use std::{str::FromStr, sync::Arc};

use crate::{
    catalog::SortMode,
    domain::{DomainError, ErrorCode, OperationId, PhotoId},
    pending_delete::{DeletionMutationResult, PendingDeleteService, PendingDeleteSnapshot},
};

#[tauri::command]
pub fn get_pending_delete_snapshot(
    sort_mode: String,
    pending_delete: tauri::State<'_, Arc<PendingDeleteService>>,
) -> Result<PendingDeleteSnapshot, DomainError> {
    pending_delete.snapshot(parse_sort_mode(&sort_mode)?)
}

#[tauri::command]
pub fn toggle_deletion_mark(
    photo_id: String,
    photo_version: i64,
    operation_id: String,
    pending_delete: tauri::State<'_, Arc<PendingDeleteService>>,
) -> Result<DeletionMutationResult, DomainError> {
    pending_delete.toggle(
        PhotoId::from_str(&photo_id)?,
        photo_version,
        OperationId::from_str(&operation_id)?,
    )
}

#[tauri::command]
pub fn undo_deletion_mark(
    operation_id: String,
    pending_delete: tauri::State<'_, Arc<PendingDeleteService>>,
) -> Result<DeletionMutationResult, DomainError> {
    pending_delete.undo(OperationId::from_str(&operation_id)?)
}

fn parse_sort_mode(value: &str) -> Result<SortMode, DomainError> {
    match value {
        "captureDesc" => Ok(SortMode::CaptureDesc),
        "captureAsc" => Ok(SortMode::CaptureAsc),
        _ => Err(DomainError::new(ErrorCode::InvalidId, "排序方式无效。")),
    }
}
