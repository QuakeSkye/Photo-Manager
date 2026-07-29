pub mod catalog;
mod commands;
pub mod db;
pub mod domain;
pub mod roots;
pub mod safety;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
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
            application.manage(std::sync::Arc::new(database));
            application.manage(safety_state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![commands::get_safety_status])
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
