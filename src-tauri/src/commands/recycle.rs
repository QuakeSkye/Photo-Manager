use std::sync::Arc;

use uuid::Uuid;

use crate::{
    domain::{DomainError, ErrorCode},
    recycle::{RecycleConfirmation, RecycleService, RecycleStatus},
    safety::{FeatureGate, RuntimeSafetyGate},
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
    selection_digest: String,
    service: tauri::State<'_, Arc<RecycleService>>,
    safety_gate: tauri::State<'_, Arc<RuntimeSafetyGate>>,
) -> Result<RecycleStatus, DomainError> {
    let permit = safety_gate.require_feature(FeatureGate::Recycle)?;
    service.start(
        permit,
        parse_operation_id(&operation_id)?,
        logical_photo_count,
        physical_file_count,
        total_bytes,
        selection_digest,
    )
}

#[tauri::command]
pub fn retry_recycle(
    operation_id: String,
    logical_photo_count: usize,
    physical_file_count: usize,
    total_bytes: i64,
    selection_digest: String,
    service: tauri::State<'_, Arc<RecycleService>>,
    safety_gate: tauri::State<'_, Arc<RuntimeSafetyGate>>,
) -> Result<RecycleStatus, DomainError> {
    let permit = safety_gate.require_feature(FeatureGate::Recycle)?;
    service.retry(
        permit,
        parse_operation_id(&operation_id)?,
        logical_photo_count,
        physical_file_count,
        total_bytes,
        selection_digest,
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
