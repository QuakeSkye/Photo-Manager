use std::{str::FromStr, sync::Arc};

use tauri_plugin_dialog::DialogExt;

use crate::{
    domain::{DomainError, ErrorCode, PhotoId, ToolId},
    external_tools::{ExternalToolService, PhotoshopStatus},
};

#[tauri::command]
pub fn get_photoshop_status(
    tools: tauri::State<'_, Arc<ExternalToolService>>,
) -> Result<PhotoshopStatus, DomainError> {
    tools.photoshop_status()
}

#[tauri::command]
pub async fn choose_photoshop(
    application: tauri::AppHandle,
    tools: tauri::State<'_, Arc<ExternalToolService>>,
) -> Result<Option<PhotoshopStatus>, DomainError> {
    let selected = application
        .dialog()
        .file()
        .set_title("选择 Photoshop.exe")
        .add_filter("Photoshop", &["exe"])
        .blocking_pick_file();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected.into_path().map_err(|error| {
        DomainError::new(ErrorCode::UnsafePath, "无法读取所选 Photoshop.exe 路径。")
            .with_diagnostic(error.to_string())
    })?;
    tools.register_photoshop(&path).map(Some)
}

#[tauri::command]
pub fn open_raf_in_photoshop(
    tool_id: String,
    photo_id: String,
    photo_version: i64,
    tools: tauri::State<'_, Arc<ExternalToolService>>,
) -> Result<(), DomainError> {
    tools.open_raf_in_photoshop(
        ToolId::from_str(&tool_id)?,
        PhotoId::from_str(&photo_id)?,
        photo_version,
    )
}

#[tauri::command]
pub fn open_photo_file_location(
    photo_id: String,
    photo_version: i64,
    tools: tauri::State<'_, Arc<ExternalToolService>>,
) -> Result<(), DomainError> {
    tools.open_file_location(PhotoId::from_str(&photo_id)?, photo_version)
}
