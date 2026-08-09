pub(crate) mod cache;
pub(crate) mod external_tools;
pub(crate) mod import_execution;
pub(crate) mod import_preflight;
pub(crate) mod library;
pub(crate) mod pending_delete;
pub(crate) mod recycle;

use std::sync::Arc;

use crate::safety::{RuntimeSafetyGate, SafetyStatus};

#[tauri::command]
pub fn get_safety_status(state: tauri::State<'_, Arc<RuntimeSafetyGate>>) -> SafetyStatus {
    state.status()
}
