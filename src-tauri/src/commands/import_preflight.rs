use std::{path::Path, sync::Arc};

use crate::{
    catalog::ScanCoordinator,
    domain::{DomainError, ErrorCode},
    import_preflight::{
        ImportPreflightDraft, ImportPreflightService, ImportPreflightStatus, ImportSetupSnapshot,
    },
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
) -> Result<ImportSetupSnapshot, DomainError> {
    if scans.is_active() {
        return Err(DomainError::new(
            ErrorCode::LeaseUnavailable,
            "请先完成或取消当前图库扫描。",
        ));
    }
    service.select_source(Path::new(&path))
}

#[tauri::command]
pub fn start_import_preflight(
    service: tauri::State<'_, Arc<ImportPreflightService>>,
    scans: tauri::State<'_, ScanCoordinator>,
) -> Result<ImportPreflightDraft, DomainError> {
    if scans.is_active() {
        return Err(DomainError::new(
            ErrorCode::LeaseUnavailable,
            "请先完成或取消当前图库扫描。",
        ));
    }
    service.run_preflight()
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
