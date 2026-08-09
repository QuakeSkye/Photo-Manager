use std::{path::Path, sync::Arc};

use crate::{
    catalog::ScanCoordinator,
    domain::{DomainError, ErrorCode},
    import_preflight::{
        ImportPreflightDraft, ImportPreflightService, ImportPreflightStatus, ImportSetupSnapshot,
    },
    safety::{FeatureGate, RuntimeSafetyGate},
};

#[tauri::command]
pub fn get_import_setup(
    service: tauri::State<'_, Arc<ImportPreflightService>>,
) -> Result<ImportSetupSnapshot, DomainError> {
    service.setup_snapshot()
}

#[tauri::command]
pub fn select_import_source(
    path: String,
    service: tauri::State<'_, Arc<ImportPreflightService>>,
    scans: tauri::State<'_, ScanCoordinator>,
    safety_gate: tauri::State<'_, Arc<RuntimeSafetyGate>>,
) -> Result<ImportSetupSnapshot, DomainError> {
    if scans.is_active() {
        return Err(DomainError::new(
            ErrorCode::LeaseUnavailable,
            "请先完成或取消当前图库扫描。",
        ));
    }
    let permit = safety_gate.require_feature(FeatureGate::Import)?;
    service.select_source(&permit, Path::new(&path))
}

#[tauri::command]
pub async fn start_import_preflight(
    service: tauri::State<'_, Arc<ImportPreflightService>>,
    scans: tauri::State<'_, ScanCoordinator>,
    safety_gate: tauri::State<'_, Arc<RuntimeSafetyGate>>,
) -> Result<ImportPreflightDraft, DomainError> {
    if scans.is_active() {
        return Err(DomainError::new(
            ErrorCode::LeaseUnavailable,
            "请先完成或取消当前图库扫描。",
        ));
    }
    let permit = safety_gate.require_feature(FeatureGate::Import)?;
    let service = Arc::clone(service.inner());
    tauri::async_runtime::spawn_blocking(move || service.run_preflight(&permit))
        .await
        .map_err(|_| {
            DomainError::new(
                ErrorCode::Internal,
                "只读预检后台任务异常结束，请重新启动应用后重试。",
            )
        })?
}

#[tauri::command]
pub fn get_import_preflight_status(
    service: tauri::State<'_, Arc<ImportPreflightService>>,
) -> Result<ImportPreflightStatus, DomainError> {
    service.status()
}

#[tauri::command]
pub fn cancel_import_preflight(
    service: tauri::State<'_, Arc<ImportPreflightService>>,
) -> Result<(), DomainError> {
    service.cancel()
}
