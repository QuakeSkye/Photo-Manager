use crate::safety::{SafetyState, SafetyStatus};

#[tauri::command]
pub fn get_safety_status(state: tauri::State<'_, SafetyState>) -> SafetyStatus {
    state.status()
}
