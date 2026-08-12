pub mod cache;
pub mod catalog;
mod commands;
pub mod db;
pub mod domain;
pub mod duplicate_review;
pub mod external_tools;
pub mod import_execution;
pub mod import_preflight;
pub mod io_scheduler;
pub mod media;
pub mod pending_delete;
pub mod phase_eight;
pub mod recycle;
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
            let database = std::sync::Arc::new(database);
            let runtime_safety_gate =
                std::sync::Arc::new(safety::RuntimeSafetyGate::new(database.clone()));
            let io_scheduler = std::sync::Arc::new(io_scheduler::IoScheduler::for_rotational());
            let catalog = std::sync::Arc::new(catalog::CatalogService::new(database.clone()));
            if catalog.recover_interrupted_scans().is_err() {
                runtime_safety_gate.latch_fault(safety::GateCloseReason::Reconciler);
            }
            let scan_coordinator =
                catalog::ScanCoordinator::with_scheduler(catalog.clone(), io_scheduler.clone());
            let library_service = std::sync::Arc::new(roots::FujiLibraryService::new(
                database.clone(),
                app_data_directory.clone(),
            ));
            let import_preflight_service =
                std::sync::Arc::new(import_preflight::ImportPreflightService::with_scheduler(
                    database.clone(),
                    app_data_directory.clone(),
                    io_scheduler.clone(),
                    runtime_safety_gate.clone(),
                ));
            let import_execution_service =
                std::sync::Arc::new(import_execution::ImportExecutionService::new(
                    database.clone(),
                    import_preflight_service.clone(),
                    catalog.clone(),
                    runtime_safety_gate.clone(),
                ));
            if import_execution_service.reconcile_startup().is_err() {
                runtime_safety_gate.latch_fault(safety::GateCloseReason::Reconciler);
            }
            let cache_service = std::sync::Arc::new(cache::CacheService::new(
                database.clone(),
                app_data_directory,
                safety::FaultInjector::default(),
            ));
            if cache_service.ensure_default_root().is_err() {
                runtime_safety_gate.latch_fault(safety::GateCloseReason::RootIdentity);
            }
            let preview_service = std::sync::Arc::new(cache::PreviewService::with_scheduler(
                database.clone(),
                cache_service.clone(),
                io_scheduler.clone(),
            ));
            let pending_delete_service =
                std::sync::Arc::new(pending_delete::PendingDeleteService::with_scheduler(
                    database.clone(),
                    io_scheduler.clone(),
                ));
            let recycle_service = std::sync::Arc::new(recycle::RecycleService::new(
                database.clone(),
                pending_delete_service.clone(),
                runtime_safety_gate.clone(),
            )?);
            if recycle_service.reconcile_startup().is_err() {
                runtime_safety_gate.latch_fault(safety::GateCloseReason::Reconciler);
            }
            let external_tool_service =
                std::sync::Arc::new(external_tools::ExternalToolService::new(database.clone()));
            let duplicate_review_service =
                std::sync::Arc::new(duplicate_review::DuplicateReviewService::new(
                    database.clone(),
                    io_scheduler.clone(),
                ));
            let _ = duplicate_review_service.reconcile_startup();
            let phase_eight_ui_fixture = std::env::var_os("FPM_TEST_ROOT").is_some()
                && std::env::var_os("FPM_PHASE8_UI_NO_BACKGROUND_SCAN").is_some();
            if !phase_eight_ui_fixture {
                let _ = scan_coordinator.start_background_if_bound();
            }
            let _ = runtime_safety_gate.mark_ready();
            application.manage(database);
            application.manage(catalog);
            application.manage(library_service);
            application.manage(import_preflight_service);
            application.manage(import_execution_service);
            application.manage(preview_service);
            application.manage(cache_service.clone());
            application.manage(pending_delete_service);
            application.manage(recycle_service);
            application.manage(external_tool_service);
            application.manage(duplicate_review_service);
            application.manage(scan_coordinator);
            application.manage(io_scheduler);
            application.manage(runtime_safety_gate);
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
            commands::import_preflight::get_import_setup,
            commands::import_preflight::select_import_source,
            commands::import_preflight::start_import_preflight,
            commands::import_preflight::get_import_preflight_status,
            commands::import_preflight::cancel_import_preflight,
            commands::import_execution::start_import,
            commands::import_execution::retry_import,
            commands::import_execution::get_import_execution_status,
            commands::import_execution::cancel_remaining_import,
            commands::import_execution::cancel_import_before_start,
            commands::import_execution::get_import_recovery_summary,
            commands::external_tools::get_photoshop_status,
            commands::external_tools::choose_photoshop,
            commands::external_tools::open_raf_in_photoshop,
            commands::external_tools::open_photo_file_location,
            commands::duplicate_review::get_duplicate_review_status,
            commands::duplicate_review::start_duplicate_review_scan,
            commands::duplicate_review::cancel_duplicate_review_scan,
            commands::duplicate_review::get_duplicate_review_results,
            commands::duplicate_review::open_duplicate_asset_location,
            commands::cache::get_cache_settings,
            commands::cache::validate_cache_location,
            commands::cache::update_cache_location,
            commands::cache::update_cache_capacity,
            commands::pending_delete::get_pending_delete_snapshot,
            commands::pending_delete::toggle_deletion_mark,
            commands::pending_delete::undo_deletion_mark,
            commands::pending_delete::add_duplicate_subset_to_pending_delete,
            commands::pending_delete::start_duplicate_pending_batch,
            commands::pending_delete::get_duplicate_pending_batch_status,
            commands::pending_delete::cancel_duplicate_pending_batch,
            commands::recycle::get_recycle_confirmation,
            commands::recycle::get_recycle_status,
            commands::recycle::start_recycle,
            commands::recycle::retry_recycle,
            commands::recycle::cancel_remaining_recycle,
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
