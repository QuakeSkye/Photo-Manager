use std::{path::Path, str::FromStr, sync::Arc};

use crate::{
    cache::{CacheLocationCandidate, CacheService, CacheSettings},
    domain::{DomainError, OperationId},
};

#[tauri::command]
pub fn get_cache_settings(
    cache: tauri::State<'_, Arc<CacheService>>,
) -> Result<CacheSettings, DomainError> {
    cache.settings()
}

#[tauri::command]
pub fn validate_cache_location(
    path: String,
    cache: tauri::State<'_, Arc<CacheService>>,
) -> Result<CacheLocationCandidate, DomainError> {
    cache.inspect_location(Path::new(&path))
}

#[tauri::command]
pub fn update_cache_location(
    path: String,
    operation_id: String,
    cache: tauri::State<'_, Arc<CacheService>>,
) -> Result<CacheSettings, DomainError> {
    let plan = cache.prepare_root(Path::new(&path), OperationId::from_str(&operation_id)?)?;
    cache.execute_root_create(plan.intent_id)?;
    start_cache_maintenance(cache.inner().clone());
    cache.settings()
}

#[tauri::command]
pub fn update_cache_capacity(
    capacity_bytes: i64,
    operation_id: String,
    cache: tauri::State<'_, Arc<CacheService>>,
) -> Result<CacheSettings, DomainError> {
    cache.update_capacity(capacity_bytes, OperationId::from_str(&operation_id)?)?;
    start_cache_maintenance(cache.inner().clone());
    cache.settings()
}

pub(crate) fn start_cache_maintenance(cache: Arc<CacheService>) {
    std::thread::spawn(move || {
        let _ = cache.evict_lru_until_within_limits();
        let _ = cache.drain_progressively();
    });
}
