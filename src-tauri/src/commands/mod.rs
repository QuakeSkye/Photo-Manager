pub(crate) mod cache;
pub(crate) mod external_tools;
pub(crate) mod import_preflight;
pub(crate) mod library;
pub(crate) mod pending_delete;

use crate::safety::{SafetyState, SafetyStatus};

#[tauri::command]
pub fn get_safety_status(state: tauri::State<'_, SafetyState>) -> SafetyStatus {
    state.status()
}
