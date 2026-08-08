use std::sync::Arc;

use uuid::Uuid;

use crate::{
    domain::{DomainError, ErrorCode},
    recycle::{RecycleConfirmation, RecycleService, RecycleStatus},
};

#[tauri::command]
pub fn get_recycle_confirmation(
    service: tauri::State<'_, Arc<RecycleService>>,
) -> Result<RecycleConfirmation, DomainError> {
    service.confirmation()
}

#[tauri::command]
pub fn get_recycle_status(
    service: tauri::State<'_, Arc<RecycleService>>,
) -> Result<RecycleStatus, DomainError> {
    service.status()
}

#[tauri::command]
pub fn start_recycle(
    operation_id: String,
    logical_photo_count: usize,
    physical_file_count: usize,
    total_bytes: i64,
    service: tauri::State<'_, Arc<RecycleService>>,
) -> Result<RecycleStatus, DomainError> {
    service.start(
        parse_operation_id(&operation_id)?,
        logical_photo_count,
        physical_file_count,
        total_bytes,
    )
}

#[tauri::command]
pub fn retry_recycle(
    operation_id: String,
    logical_photo_count: usize,
    physical_file_count: usize,
    total_bytes: i64,
    service: tauri::State<'_, Arc<RecycleService>>,
) -> Result<RecycleStatus, DomainError> {
    service.retry(
        parse_operation_id(&operation_id)?,
        logical_photo_count,
        physical_file_count,
        total_bytes,
    )
}

#[tauri::command]
pub fn cancel_remaining_recycle(
    operation_id: String,
    service: tauri::State<'_, Arc<RecycleService>>,
) -> Result<RecycleStatus, DomainError> {
    service.cancel_remaining(parse_operation_id(&operation_id)?)
}

fn parse_operation_id(value: &str) -> Result<Uuid, DomainError> {
    Uuid::parse_str(value).map_err(|_| DomainError::new(ErrorCode::InvalidId, "删除操作标识无效。"))
}
