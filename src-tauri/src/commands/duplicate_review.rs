use std::{str::FromStr, sync::Arc};

use crate::{
    domain::{AssetId, DomainError},
    duplicate_review::{DuplicateReviewResults, DuplicateReviewService, DuplicateReviewStatus},
    external_tools::ExternalToolService,
};

#[tauri::command]
pub async fn get_duplicate_review_status(
    service: tauri::State<'_, Arc<DuplicateReviewService>>,
) -> Result<DuplicateReviewStatus, DomainError> {
    let service = Arc::clone(service.inner());
    tauri::async_runtime::spawn_blocking(move || service.status())
        .await
        .map_err(|_| background_error())?
}

#[tauri::command]
pub async fn start_duplicate_review_scan(
    service: tauri::State<'_, Arc<DuplicateReviewService>>,
) -> Result<DuplicateReviewStatus, DomainError> {
    service.inner().start()
}

#[tauri::command]
pub async fn cancel_duplicate_review_scan(
    service: tauri::State<'_, Arc<DuplicateReviewService>>,
) -> Result<DuplicateReviewStatus, DomainError> {
    service.cancel()
}

#[tauri::command]
pub async fn get_duplicate_review_results(
    service: tauri::State<'_, Arc<DuplicateReviewService>>,
) -> Result<DuplicateReviewResults, DomainError> {
    let service = Arc::clone(service.inner());
    tauri::async_runtime::spawn_blocking(move || service.results())
        .await
        .map_err(|_| background_error())?
}

#[tauri::command]
pub fn open_duplicate_asset_location(
    asset_id: String,
    asset_version: i64,
    tools: tauri::State<'_, Arc<ExternalToolService>>,
) -> Result<(), DomainError> {
    tools.open_asset_location(AssetId::from_str(&asset_id)?, asset_version)
}

fn background_error() -> DomainError {
    DomainError::new(
        crate::domain::ErrorCode::Internal,
        "重复检测后台状态不可用。",
    )
}
