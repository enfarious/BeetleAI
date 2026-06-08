mod commands;
mod git;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(commands::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::list_projects,
            commands::open_project,
            commands::read_design_doc,
            commands::write_design_doc,
            commands::list_cards,
            commands::create_card,
            commands::update_card,
            commands::start_run,
            commands::cancel_run,
            commands::unblock_run,
            commands::accept_run,
            commands::reject_run,
            commands::get_run_log,
            commands::send_chat,
            commands::read_diff,
            commands::list_dir,
            commands::read_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
