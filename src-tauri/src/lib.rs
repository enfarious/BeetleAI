use std::path::{Path, PathBuf};
use tauri::Manager;

mod commands;
mod git;

pub fn clean_project_path<P: AsRef<Path>>(path: P) -> PathBuf {
    let p = path.as_ref();
    let abs_path = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(p)
    };

    let mut stack = Vec::new();
    for component in abs_path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                stack.pop();
            }
            std::path::Component::Normal(c) => {
                stack.push(c);
            }
            c => {
                stack.push(c.as_os_str());
            }
        }
    }

    let mut normalized = PathBuf::new();
    for c in stack {
        normalized.push(c);
    }

    normalized
}

/// The application's own root directory: the cwd, stepping out of `src-tauri`
/// if launched from there (cargo/tauri dev runs). This src-tauri quirk applies
/// ONLY to resolving the app's own location — never to user project or tool
/// paths, where silently dropping a path component corrupts file operations on
/// any project that legitimately contains a `src-tauri` directory.
pub fn app_root_path() -> PathBuf {
    let mut p = clean_project_path(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    if p.ends_with("src-tauri") {
        p.pop();
    }
    p
}

/// On Windows, prevent spawned child processes (git, cmd.exe) from flashing a
/// console window. Windowed-subsystem release builds have no parent console,
/// so every child otherwise gets its own conhost: a visible flash plus
/// 100-300ms of spawn overhead per call. Dev builds never show this because
/// the dev exe has a console the children quietly attach to.
pub fn configure_no_window(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    std::panic::set_hook(Box::new(|panic_info| {
        let backtrace = std::backtrace::Backtrace::capture();
        let payload = panic_info.payload();
        let message = if let Some(s) = payload.downcast_ref::<&str>() {
            *s
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.as_str()
        } else {
            "Unknown panic payload"
        };
        let location = panic_info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "Unknown location".to_string());

        let log_content = format!(
            "--- PANIC DETECTED ---\nTimestamp: {}\nLocation: {}\nMessage: {}\n\nBacktrace:\n{:?}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            location,
            message,
            backtrace
        );

        let logs_dir = app_root_path().join("logs");
        let _ = std::fs::create_dir_all(&logs_dir);
        let _ = std::fs::write(logs_dir.join("error.log"), &log_content);
    }));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::AppState::new())
        .setup(|app| {
            let app_handle = app.handle().clone();
            if let Err(e) = commands::init_db(&app_handle) {
                eprintln!("Failed to initialize database: {}", e);
            } else {
                let state = app.state::<commands::AppState>();
                if let Err(e) = commands::load_state_from_db(&app_handle, &state) {
                    eprintln!("Failed to load state from database: {}", e);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_projects,
            commands::open_project,
            commands::create_project,
            commands::update_project,
            commands::delete_project,
            commands::get_settings,
            commands::save_settings,
            commands::fetch_local_models,
            commands::list_design_docs,
            commands::send_design_chat,
            commands::get_design_log,
            commands::send_code_chat,
            commands::get_code_log,
            commands::read_design_doc,
            commands::write_design_doc,
            commands::list_cards,
            commands::create_card,
            commands::update_card,
            commands::save_card,
            commands::delete_card,
            commands::start_run,
            commands::cancel_run,
            commands::abort_chat,
            commands::is_run_active,
            commands::unblock_run,
            commands::accept_run,
            commands::reject_run,
            commands::get_run_log,
            commands::send_chat,
            commands::read_diff,
            commands::list_dir,
            commands::read_file,
            commands::create_file,
            commands::create_dir,
            commands::save_file,
            commands::delete_item
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
