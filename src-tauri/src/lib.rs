pub mod cache;
pub mod catalog;
mod commands;
pub mod db;
pub mod domain;
pub mod external_tools;
pub mod media;
pub mod pending_delete;
pub mod roots;
pub mod safety;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(
            |application, _arguments, _working_directory| {
                if let Some(window) = application.get_webview_window("main") {
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            },
        ))
        .setup(|application| {
            let app_data_directory = resolve_app_data_directory(application)?;
            let database = db::Database::open(&app_data_directory)?;
            let safety_state = safety::SafetyState::from_database_mode(database.mode());
            let database = std::sync::Arc::new(database);
            let catalog = std::sync::Arc::new(catalog::CatalogService::new(database.clone()));
            catalog.recover_interrupted_scans()?;
            let scan_coordinator = catalog::ScanCoordinator::new(catalog.clone());
            let library_service = std::sync::Arc::new(roots::FujiLibraryService::new(
                database.clone(),
                app_data_directory.clone(),
            ));
            let cache_service = std::sync::Arc::new(cache::CacheService::new(
                database.clone(),
                app_data_directory,
                safety::FaultInjector::default(),
            ));
            let _ = cache_service.ensure_default_root();
            let preview_service = std::sync::Arc::new(cache::PreviewService::new(
                database.clone(),
                cache_service.clone(),
            ));
            let pending_delete_service =
                std::sync::Arc::new(pending_delete::PendingDeleteService::new(database.clone()));
            let external_tool_service =
                std::sync::Arc::new(external_tools::ExternalToolService::new(database.clone()));
            let _ = scan_coordinator.start_background_if_bound();
            application.manage(database);
            application.manage(catalog);
            application.manage(library_service);
            application.manage(preview_service);
            application.manage(cache_service.clone());
            application.manage(pending_delete_service);
            application.manage(external_tool_service);
            application.manage(scan_coordinator);
            application.manage(safety_state);
            commands::cache::start_cache_maintenance(cache_service);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_safety_status,
            commands::library::get_library_snapshot,
            commands::library::get_photo_detail,
            commands::library::get_photo_preview,
            commands::library::save_viewing_cursor,
            commands::library::validate_library_folder,
            commands::library::bind_library_folder,
            commands::library::get_scan_status,
            commands::library::cancel_library_scan,
            commands::library::retry_library_scan,
            commands::external_tools::get_photoshop_status,
            commands::external_tools::choose_photoshop,
            commands::external_tools::open_raf_in_photoshop,
            commands::external_tools::open_photo_file_location,
            commands::cache::get_cache_settings,
            commands::cache::validate_cache_location,
            commands::cache::update_cache_location,
            commands::cache::update_cache_capacity,
            commands::pending_delete::get_pending_delete_snapshot,
            commands::pending_delete::toggle_deletion_mark,
            commands::pending_delete::undo_deletion_mark,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the Tauri application");

    app.run(|_application, _event| {});
}

fn resolve_app_data_directory(
    application: &tauri::App,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    if let Some(test_root_path) = std::env::var_os("FPM_TEST_ROOT") {
        let test_root = safety::TestRoot::verify(std::path::Path::new(&test_root_path))?;
        let app_data = test_root.path().join("app-data");
        std::fs::create_dir_all(&app_data)?;
        return Ok(app_data);
    }

    Ok(application.path().app_local_data_dir()?)
}
