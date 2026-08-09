#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(exit_code) = local_photo_manager::phase_eight::try_run_cli() {
        std::process::exit(exit_code);
    }
    local_photo_manager::run();
}
