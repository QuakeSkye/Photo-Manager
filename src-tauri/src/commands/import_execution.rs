use std::sync::Arc;

use crate::{
    domain::DomainError,
    import_execution::{ImportExecutionService, ImportExecutionStatus, ImportRecoverySummary},
    safety::{FeatureGate, RuntimeSafetyGate},
};

#[tauri::command]
pub fn start_import(
    draft_digest: String,
    service: tauri::State<'_, Arc<ImportExecutionService>>,
    safety_gate: tauri::State<'_, Arc<RuntimeSafetyGate>>,
) -> Result<ImportExecutionStatus, DomainError> {
    let permit = safety_gate.require_feature(FeatureGate::Import)?;
    service.inner().start_background(permit, &draft_digest)
}
#[tauri::command]
pub fn retry_import(
    service: tauri::State<'_, Arc<ImportExecutionService>>,
    safety_gate: tauri::State<'_, Arc<RuntimeSafetyGate>>,
) -> Result<ImportExecutionStatus, DomainError> {
    let permit = safety_gate.require_feature(FeatureGate::Import)?;
    service.inner().retry_background(permit)
}
#[tauri::command]
pub fn get_import_execution_status(
    service: tauri::State<'_, Arc<ImportExecutionService>>,
) -> Result<ImportExecutionStatus, DomainError> {
    service.status()
}
#[tauri::command]
pub fn cancel_remaining_import(
    service: tauri::State<'_, Arc<ImportExecutionService>>,
) -> Result<(), DomainError> {
    service.request_cancel()
}
#[tauri::command]
pub fn cancel_import_before_start(
    service: tauri::State<'_, Arc<ImportExecutionService>>,
) -> Result<ImportExecutionStatus, DomainError> {
    service.cancel_before_start()
}
#[tauri::command]
pub fn get_import_recovery_summary(
    service: tauri::State<'_, Arc<ImportExecutionService>>,
) -> Result<ImportRecoverySummary, DomainError> {
    service.recovery_summary()
}

#[tauri::command]
pub fn reselect_import_source(
    path: String,
    service: tauri::State<'_, Arc<ImportExecutionService>>,
    safety_gate: tauri::State<'_, Arc<RuntimeSafetyGate>>,
) -> Result<crate::import_preflight::ImportSetupSnapshot, DomainError> {
    service.reselect_source(
        &safety_gate.require_feature(FeatureGate::Import)?,
        std::path::Path::new(&path),
    )
}
#[tauri::command]
pub fn get_import_recovery_items(
    service: tauri::State<'_, Arc<ImportExecutionService>>,
) -> Result<Vec<crate::import_execution::RecoveryItem>, DomainError> {
    service.recovery_items()
}
#[tauri::command]
pub fn exclude_import_recovery_items(
    ownership_ids: Vec<String>,
    service: tauri::State<'_, Arc<ImportExecutionService>>,
) -> Result<(), DomainError> {
    service.exclude_recovery_items(ownership_ids)
}
