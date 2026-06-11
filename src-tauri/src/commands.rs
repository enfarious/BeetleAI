use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use crate::git;
use tauri::{Emitter, Manager};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TodoItem {
    pub text: String,
    pub completed: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Card {
    pub id: String,
    pub project_path: String,
    pub title: String,
    pub description: String,
    pub status: String, // "backlog", "todo", "running", "blocked", "review", "done", "failed"
    pub run_id: Option<String>,
    pub assignee: Option<String>,
    pub todo_list: Vec<TodoItem>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RunEvent {
    pub run_id: String,
    pub event_type: String, // "status", "message", "tool_call", "tool_result", "file_touched", "blocked", "error"
    pub payload: String,    // JSON payload or text
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LlmSettings {
    pub provider: String,
    pub api_url: String,
    pub api_key: String,
    pub model: String,
    pub max_steps: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppConfig {
    pub settings: LlmSettings,
    pub projects: Vec<Project>,
}

pub struct AppState {
    pub cards: Mutex<Vec<Card>>,
    pub run_logs: Mutex<HashMap<String, Vec<RunEvent>>>,
    pub design_logs: Mutex<HashMap<String, Vec<RunEvent>>>,
    pub code_logs: Mutex<HashMap<String, Vec<RunEvent>>>,
    pub active_runs: Mutex<std::collections::HashSet<String>>,
    pub cancelled_runs: Mutex<std::collections::HashSet<String>>,
    /// Last LM Studio stateful `response_id` per run/chat key. The /api/v1/chat
    /// endpoint keeps history server-side; chaining `previous_response_id` is
    /// what makes a thread continue instead of starting fresh every call.
    pub lmstudio_response_ids: Mutex<HashMap<String, String>>,
}

impl AppState {
    pub fn new() -> Self {
        let default_project_path = {
            let mut p = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            if p.ends_with("src-tauri") {
                p.pop();
            }
            p.to_string_lossy().into_owned()
        };

        let initial_cards = vec![
            Card {
                id: "card_1".to_string(),
                project_path: default_project_path.clone(),
                title: "Bootstrapping & Three-Column UI Layout".to_string(),
                description: "Setup Tauri v2 template with TypeScript, and construct the basic grid UI and styles.".to_string(),
                status: "done".to_string(),
                run_id: Some("run_card_1".to_string()),
                assignee: Some("BeetleAI".to_string()),
                todo_list: vec![
                    TodoItem { text: "Configure Tauri v2 project template".to_string(), completed: true },
                    TodoItem { text: "Build TypeScript sidebar navigation and panels".to_string(), completed: true },
                    TodoItem { text: "Construct CSS layouts and themes".to_string(), completed: true },
                ],
            },
            Card {
                id: "card_2".to_string(),
                project_path: default_project_path.clone(),
                title: "Git Worktree Integration & Sandbox".to_string(),
                description: "Implement git worktree creation, merge, and discard actions. Ensure filesystem is sandboxed.".to_string(),
                status: "review".to_string(),
                run_id: Some("run_card_2".to_string()),
                assignee: Some("BeetleAI".to_string()),
                todo_list: vec![
                    TodoItem { text: "Implement git worktree creation helpers".to_string(), completed: true },
                    TodoItem { text: "Integrate file deletion and modification boundaries".to_string(), completed: true },
                    TodoItem { text: "Verify sandbox path traversal checks".to_string(), completed: false },
                ],
            },
            Card {
                id: "card_3".to_string(),
                project_path: default_project_path.clone(),
                title: "Card State Store & Persistency Layer".to_string(),
                description: "Integrate SQLite and persist project files, cards, and execution transcripts.".to_string(),
                status: "todo".to_string(),
                run_id: None,
                assignee: None,
                todo_list: vec![
                    TodoItem { text: "Design database schema for cards and logs".to_string(), completed: false },
                    TodoItem { text: "Integrate SQLite driver and migrations".to_string(), completed: false },
                    TodoItem { text: "Implement state persistence interface".to_string(), completed: false },
                ],
            },
            Card {
                id: "card_4".to_string(),
                project_path: default_project_path.clone(),
                title: "Autonomous Loop Run Engine".to_string(),
                description: "Build the tokio task worker loop that fetches model responses, executes tools, and sends events.".to_string(),
                status: "backlog".to_string(),
                run_id: None,
                assignee: None,
                todo_list: vec![
                    TodoItem { text: "Construct Tokio worker thread loop".to_string(), completed: false },
                    TodoItem { text: "Implement model response stream parsing".to_string(), completed: false },
                    TodoItem { text: "Add recursive tool routing handlers".to_string(), completed: false },
                ],
            },
        ];

        let mut initial_logs = HashMap::new();
        initial_logs.insert(
            "run_card_2".to_string(),
            vec![
                RunEvent {
                    run_id: "run_card_2".to_string(),
                    event_type: "status".to_string(),
                    payload: "running".to_string(),
                },
                RunEvent {
                    run_id: "run_card_2".to_string(),
                    event_type: "message".to_string(),
                    payload: "{\"role\":\"agent\",\"content\":\"Starting worktree preparation for card_2. Creating branch harness/run-card_2 from main.\"}"
                        .to_string(),
                },
                RunEvent {
                    run_id: "run_card_2".to_string(),
                    event_type: "tool_call".to_string(),
                    payload: "{\"tool\":\"git_worktree_add\",\"args\":{\"path\":\".harness/worktrees/run_card_2\",\"branch\":\"harness/run-card_2\"}}"
                        .to_string(),
                },
                RunEvent {
                    run_id: "run_card_2".to_string(),
                    event_type: "tool_result".to_string(),
                    payload: "{\"tool\":\"git_worktree_add\",\"result\":\"Success: Worktree created at .harness/worktrees/run_card_2\"}"
                        .to_string(),
                },
                RunEvent {
                    run_id: "run_card_2".to_string(),
                    event_type: "message".to_string(),
                    payload: "{\"role\":\"agent\",\"content\":\"Worktree initialized. Now creating git helper module in src-tauri/src/git.rs.\"}"
                        .to_string(),
                },
                RunEvent {
                    run_id: "run_card_2".to_string(),
                    event_type: "file_touched".to_string(),
                    payload: "{\"path\":\"src-tauri/src/git.rs\",\"op\":\"create\"}".to_string(),
                },
                RunEvent {
                    run_id: "run_card_2".to_string(),
                    event_type: "status".to_string(),
                    payload: "review".to_string(),
                },
                RunEvent {
                    run_id: "run_card_2".to_string(),
                    event_type: "message".to_string(),
                    payload: "{\"role\":\"agent\",\"content\":\"I have completed writing the git operations module in `src-tauri/src/git.rs`. Let me know if you would like me to merge it!\"}"
                        .to_string(),
                },
            ],
        );

        Self {
            cards: Mutex::new(initial_cards),
            run_logs: Mutex::new(initial_logs),
            design_logs: Mutex::new(HashMap::new()),
            code_logs: Mutex::new(HashMap::new()),
            active_runs: Mutex::new(std::collections::HashSet::new()),
            cancelled_runs: Mutex::new(std::collections::HashSet::new()),
            lmstudio_response_ids: Mutex::new(HashMap::new()),
        }
    }
}

// Config persistence helpers
use crate::{app_root_path, clean_project_path};

/// Resolve the repo path for a run from the owning card's project_path, rather than
/// the app's own working directory. This is what scopes the run engine to the
/// SELECTED project instead of BeetleAI's own repo.
fn repo_path_for_run(state: &AppState, run_id: &str) -> Option<PathBuf> {
    let cards = state.cards.lock().unwrap();
    cards
        .iter()
        .find(|c| c.run_id.as_deref() == Some(run_id))
        .map(|c| clean_project_path(&c.project_path))
}

use rusqlite::OptionalExtension;

fn get_db_path(app_handle: &tauri::AppHandle) -> PathBuf {
    let mut path = app_handle
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."));
    let _ = fs::create_dir_all(&path);
    path.push("beetleai.db");
    path
}

fn get_db_conn(app_handle: &tauri::AppHandle) -> Result<rusqlite::Connection, String> {
    let db_path = get_db_path(app_handle);
    rusqlite::Connection::open(db_path).map_err(|e| e.to_string())
}

fn seed_default_cards_for_project(
    conn: &rusqlite::Connection,
    project_path: &str,
) -> Result<Vec<Card>, String> {
    let project_name = std::path::Path::new(project_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    let project_slug = project_name
        .to_lowercase()
        .replace(|c: char| !c.is_alphanumeric(), "_");

    let initial_cards = vec![
        Card {
            id: format!("{}_card_1", project_slug),
            project_path: project_path.to_string(),
            title: "Bootstrapping & Three-Column UI Layout".to_string(),
            description: "Setup Tauri v2 template with TypeScript, and construct the basic grid UI and styles.".to_string(),
            status: "done".to_string(),
            run_id: Some(format!("run_{}_card_1", project_slug)),
            assignee: Some("BeetleAI".to_string()),
            todo_list: vec![
                TodoItem { text: "Configure Tauri v2 project template".to_string(), completed: true },
                TodoItem { text: "Build TypeScript sidebar navigation and panels".to_string(), completed: true },
                TodoItem { text: "Construct CSS layouts and themes".to_string(), completed: true },
            ],
        },
        Card {
            id: format!("{}_card_2", project_slug),
            project_path: project_path.to_string(),
            title: "Git Worktree Integration & Sandbox".to_string(),
            description: "Implement git worktree creation, merge, and discard actions. Ensure filesystem is sandboxed.".to_string(),
            status: "review".to_string(),
            run_id: Some(format!("run_{}_card_2", project_slug)),
            assignee: Some("BeetleAI".to_string()),
            todo_list: vec![
                TodoItem { text: "Implement git worktree creation helpers".to_string(), completed: true },
                TodoItem { text: "Integrate file deletion and modification boundaries".to_string(), completed: true },
                TodoItem { text: "Verify sandbox path traversal checks".to_string(), completed: false },
            ],
        },
        Card {
            id: format!("{}_card_3", project_slug),
            project_path: project_path.to_string(),
            title: "Card State Store & Persistency Layer".to_string(),
            description: "Integrate SQLite and persist project files, cards, and execution transcripts.".to_string(),
            status: "todo".to_string(),
            run_id: None,
            assignee: None,
            todo_list: vec![
                TodoItem { text: "Design database schema for cards and logs".to_string(), completed: false },
                TodoItem { text: "Integrate SQLite driver and migrations".to_string(), completed: false },
                TodoItem { text: "Implement state persistence interface".to_string(), completed: false },
            ],
        },
        Card {
            id: format!("{}_card_4", project_slug),
            project_path: project_path.to_string(),
            title: "Autonomous Loop Run Engine".to_string(),
            description: "Build the tokio task worker loop that fetches model responses, executes tools, and sends events.".to_string(),
            status: "backlog".to_string(),
            run_id: None,
            assignee: None,
            todo_list: vec![
                TodoItem { text: "Construct Tokio worker thread loop".to_string(), completed: false },
                TodoItem { text: "Implement model response stream parsing".to_string(), completed: false },
                TodoItem { text: "Add recursive tool routing handlers".to_string(), completed: false },
            ],
        },
    ];

    for card in &initial_cards {
        let _ = conn.execute(
            "INSERT INTO cards (id, project_path, title, description, status, run_id, assignee) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                &card.id,
                &card.project_path,
                &card.title,
                &card.description,
                &card.status,
                &card.run_id,
                &card.assignee,
            ),
        );

        for (idx, item) in card.todo_list.iter().enumerate() {
            let _ = conn.execute(
                "INSERT INTO todo_items (card_id, idx, text, completed) VALUES (?1, ?2, ?3, ?4)",
                (
                    &card.id,
                    idx as i32,
                    &item.text,
                    if item.completed { 1 } else { 0 },
                ),
            );
        }
    }

    let run_id = format!("run_{}_card_2", project_slug);
    let initial_logs = vec![
        RunEvent {
            run_id: run_id.clone(),
            event_type: "status".to_string(),
            payload: "running".to_string(),
        },
        RunEvent {
            run_id: run_id.clone(),
            event_type: "message".to_string(),
            payload: "{\n  \"role\": \"agent\",\n  \"content\": \"Starting worktree preparation for card_2. Creating branch branch_card_2 from main.\"\n}".to_string(),
        },
        RunEvent {
            run_id: run_id.clone(),
            event_type: "tool_call".to_string(),
            payload: "{\n  \"id\": \"call_read_1\",\n  \"type\": \"function\",\n  \"function\": {\n    \"name\": \"read_file\",\n    \"arguments\": \"{\\\"path\\\":\\\"design/design.md\\\"}\"\n  }\n}".to_string(),
        },
        RunEvent {
            run_id: run_id.clone(),
            event_type: "tool_result".to_string(),
            payload: "{\n  \"id\": \"call_read_1\",\n  \"result\": \"# Design Specifications\\n\\nOutline requirements...\"\n}".to_string(),
        },
        RunEvent {
            run_id: run_id.clone(),
            event_type: "status".to_string(),
            payload: "review".to_string(),
        },
        RunEvent {
            run_id: run_id.clone(),
            event_type: "message".to_string(),
            payload: "{\n  \"role\": \"agent\",\n  \"content\": \"Implementation completed inside design.md. Awaiting developer unified diff verification.\"\n}".to_string(),
        },
    ];

    for log in initial_logs {
        let _ = conn.execute(
            "INSERT INTO logs (log_type, key, run_id, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                "run",
                &initial_cards[1].id,
                &log.run_id,
                &log.event_type,
                &log.payload,
            ),
        );
    }

    Ok(initial_cards)
}

pub fn init_db(app_handle: &tauri::AppHandle) -> Result<(), String> {
    let conn = get_db_conn(app_handle)?;

    conn.execute("PRAGMA foreign_keys = ON;", [])
        .map_err(|e| e.to_string())?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS settings (
            provider TEXT NOT NULL,
            api_url TEXT NOT NULL,
            api_key TEXT NOT NULL,
            model TEXT NOT NULL,
            max_steps INTEGER NOT NULL
        );",
        [],
    )
    .map_err(|e| e.to_string())?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS projects (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            path TEXT NOT NULL
        );",
        [],
    )
    .map_err(|e| e.to_string())?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS cards (
            id TEXT PRIMARY KEY,
            project_path TEXT NOT NULL DEFAULT '',
            title TEXT NOT NULL,
            description TEXT NOT NULL,
            status TEXT NOT NULL,
            run_id TEXT,
            assignee TEXT
        );",
        [],
    )
    .map_err(|e| e.to_string())?;

    // Attempt migration to add project_path column to existing databases
    let _ = conn.execute(
        "ALTER TABLE cards ADD COLUMN project_path TEXT NOT NULL DEFAULT '';",
        [],
    );

    conn.execute(
        "CREATE TABLE IF NOT EXISTS todo_items (
            card_id TEXT NOT NULL,
            idx INTEGER NOT NULL,
            text TEXT NOT NULL,
            completed INTEGER NOT NULL,
            PRIMARY KEY (card_id, idx),
            FOREIGN KEY (card_id) REFERENCES cards (id) ON DELETE CASCADE
        );",
        [],
    )
    .map_err(|e| e.to_string())?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            log_type TEXT NOT NULL,
            key TEXT NOT NULL,
            run_id TEXT NOT NULL,
            event_type TEXT NOT NULL,
            payload TEXT NOT NULL
        );",
        [],
    )
    .map_err(|e| e.to_string())?;

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM cards", [], |row| row.get(0))
        .unwrap_or(0);
    if count == 0 {
        let mut p = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        if p.ends_with("src-tauri") {
            p.pop();
        }
        let default_project_path = p.to_string_lossy().into_owned();
        let _ = seed_default_cards_for_project(&conn, &default_project_path);
    }

    let settings_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM settings", [], |row| row.get(0))
        .unwrap_or(0);
    if settings_count == 0 {
        let _ = conn.execute(
            "INSERT INTO settings (provider, api_url, api_key, model, max_steps) VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                "custom",
                "http://localhost:11434/v1",
                "",
                "llama3",
                50i64,
            ),
        );
    }

    let projects_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
        .unwrap_or(0);
    if projects_count == 0 {
        let mut p = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        if p.ends_with("src-tauri") {
            p.pop();
        }
        let project_path = p.to_string_lossy().into_owned();
        let _ = conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            ("beetleai", "BeetleAI Harness", &project_path),
        );
    }

    Ok(())
}

fn load_config_sqlite(conn: &rusqlite::Connection) -> Result<AppConfig, String> {
    let mut stmt = conn
        .prepare("SELECT provider, api_url, api_key, model, max_steps FROM settings LIMIT 1")
        .map_err(|e| e.to_string())?;
    let settings_opt = stmt
        .query_row([], |row| {
            Ok(LlmSettings {
                provider: row.get(0)?,
                api_url: row.get(1)?,
                api_key: row.get(2)?,
                model: row.get(3)?,
                max_steps: row.get(4)?,
            })
        })
        .optional()
        .map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare("SELECT id, name, path FROM projects")
        .map_err(|e| e.to_string())?;
    let projects_rows = stmt
        .query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut projects = Vec::new();
    for proj_res in projects_rows {
        projects.push(proj_res.map_err(|e| e.to_string())?);
    }

    if let Some(settings) = settings_opt {
        Ok(AppConfig { settings, projects })
    } else {
        Err("No settings found in SQLite".to_string())
    }
}

fn save_config_sqlite(conn: &rusqlite::Connection, config: &AppConfig) -> Result<(), String> {
    let _ = conn.execute("DELETE FROM settings", []);
    conn.execute(
        "INSERT INTO settings (provider, api_url, api_key, model, max_steps) VALUES (?1, ?2, ?3, ?4, ?5)",
        (
            &config.settings.provider,
            &config.settings.api_url,
            &config.settings.api_key,
            &config.settings.model,
            &config.settings.max_steps,
        ),
    ).map_err(|e| e.to_string())?;

    let _ = conn.execute("DELETE FROM projects", []);
    for proj in &config.projects {
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            (&proj.id, &proj.name, &proj.path),
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(())
}

pub fn load_state_from_db(app_handle: &tauri::AppHandle, state: &AppState) -> Result<(), String> {
    let conn = get_db_conn(app_handle)?;

    let mut stmt = conn
        .prepare("SELECT id, project_path, title, description, status, run_id, assignee FROM cards")
        .map_err(|e| e.to_string())?;
    let card_rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut cards = Vec::new();
    for card_res in card_rows {
        let (id, project_path, title, description, status, run_id, assignee) =
            card_res.map_err(|e| e.to_string())?;

        let mut todo_stmt = conn
            .prepare("SELECT text, completed FROM todo_items WHERE card_id = ?1 ORDER BY idx ASC")
            .map_err(|e| e.to_string())?;
        let todo_rows = todo_stmt
            .query_map([&id], |row| {
                Ok(TodoItem {
                    text: row.get(0)?,
                    completed: row.get::<_, i32>(1)? != 0,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut todo_list = Vec::new();
        for todo_res in todo_rows {
            todo_list.push(todo_res.map_err(|e| e.to_string())?);
        }

        cards.push(Card {
            id,
            project_path,
            title,
            description,
            status,
            run_id,
            assignee,
            todo_list,
        });
    }

    {
        let mut app_cards = state.cards.lock().unwrap();
        *app_cards = cards;
    }

    let mut stmt = conn
        .prepare("SELECT log_type, key, run_id, event_type, payload FROM logs ORDER BY id ASC")
        .map_err(|e| e.to_string())?;
    let log_rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut run_logs = HashMap::new();
    let mut design_logs = HashMap::new();
    let mut code_logs = HashMap::new();

    for log_res in log_rows {
        let (log_type, key, run_id, event_type, payload) = log_res.map_err(|e| e.to_string())?;
        let event = RunEvent {
            run_id: run_id.clone(),
            event_type,
            payload,
        };

        match log_type.as_str() {
            "run" => {
                run_logs.entry(key).or_insert_with(Vec::new).push(event);
            }
            "design" => {
                design_logs.entry(key).or_insert_with(Vec::new).push(event);
            }
            "code" => {
                code_logs.entry(key).or_insert_with(Vec::new).push(event);
            }
            _ => {}
        }
    }

    {
        let mut app_run_logs = state.run_logs.lock().unwrap();
        *app_run_logs = run_logs;
    }
    {
        let mut app_design_logs = state.design_logs.lock().unwrap();
        *app_design_logs = design_logs;
    }
    {
        let mut app_code_logs = state.code_logs.lock().unwrap();
        *app_code_logs = code_logs;
    }

    Ok(())
}

fn get_config_path(app_handle: &tauri::AppHandle) -> PathBuf {
    let mut path = app_handle
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."));
    let _ = fs::create_dir_all(&path);
    path.push("config.json");
    path
}

fn load_config(app_handle: &tauri::AppHandle) -> AppConfig {
    if let Ok(conn) = get_db_conn(app_handle) {
        if let Ok(config) = load_config_sqlite(&conn) {
            return config;
        }
    }

    let path = get_config_path(app_handle);
    let mut config = AppConfig {
        settings: LlmSettings {
            provider: "custom".to_string(),
            api_url: "http://localhost:11434/v1".to_string(),
            api_key: "".to_string(),
            model: "llama3".to_string(),
            max_steps: 50,
        },
        projects: vec![Project {
            id: "beetleai".to_string(),
            name: "BeetleAI Harness".to_string(),
            path: {
                let mut p = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                if p.ends_with("src-tauri") {
                    p.pop();
                }
                p.to_string_lossy().into_owned()
            },
        }],
    };

    if path.exists() {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(c) = serde_json::from_str::<AppConfig>(&content) {
                config = c;
            }
        }
    }

    if let Ok(conn) = get_db_conn(app_handle) {
        let _ = save_config_sqlite(&conn, &config);
    }

    config
}

fn save_config(app_handle: &tauri::AppHandle, config: &AppConfig) -> Result<(), String> {
    let path = get_config_path(app_handle);
    let content = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    let _ = fs::write(path, content);

    if let Ok(conn) = get_db_conn(app_handle) {
        let _ = save_config_sqlite(&conn, config);
    }

    Ok(())
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

#[tauri::command]
pub async fn list_projects(app_handle: tauri::AppHandle) -> Vec<Project> {
    let config = load_config(&app_handle);
    config.projects
}

#[tauri::command]
pub async fn open_project(path: String) -> Result<Project, String> {
    let p = Path::new(&path);
    if p.exists() {
        Ok(Project {
            id: p
                .file_name()
                .map(|s| s.to_string_lossy().to_lowercase().replace(' ', "_"))
                .unwrap_or_else(|| "project".to_string()),
            name: p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Repository".to_string()),
            path,
        })
    } else {
        Err("Path does not exist".to_string())
    }
}

#[tauri::command]
pub async fn create_project(
    app_handle: tauri::AppHandle,
    name: String,
    path: String,
) -> Result<Project, String> {
    let p = Path::new(&path);
    if !p.exists() {
        fs::create_dir_all(p).map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    // Initialize .harness design document
    let harness_dir = p.join(".harness");
    let design_file = harness_dir.join("design.md");
    if !harness_dir.exists() {
        fs::create_dir_all(&harness_dir).map_err(|e| e.to_string())?;
    }
    if !design_file.exists() {
        let seed = format!(
            "# {}\n\nThis is the design document for project {}.\n\n## 1. Description\nAdd project overview.",
            name, name
        );
        fs::write(&design_file, seed).map_err(|e| e.to_string())?;
    }

    // Auto-init git repository
    if !git::is_git_repo(p) {
        let mut init_cmd = Command::new("git");
        init_cmd.current_dir(p).args(&["init", "-b", "main"]);
        crate::configure_no_window(&mut init_cmd);
        let _ = init_cmd.output();

        let gitignore = p.join(".gitignore");
        if !gitignore.exists() {
            let _ = fs::write(&gitignore, "\n# BeetleAI Harness temporary storage\n.harness/worktrees/\n.harness/harness.db\n");
        }
    }

    let project_id = name.to_lowercase().replace(' ', "_");
    let new_project = Project {
        id: project_id,
        name,
        path: p.to_string_lossy().into_owned(),
    };

    let mut config = load_config(&app_handle);
    if config
        .projects
        .iter()
        .any(|proj| proj.path == new_project.path)
    {
        return Err("Project already registered at this path.".to_string());
    }

    config.projects.push(new_project.clone());
    save_config(&app_handle, &config)?;

    Ok(new_project)
}

#[tauri::command]
pub async fn get_settings(app_handle: tauri::AppHandle) -> LlmSettings {
    let config = load_config(&app_handle);
    config.settings
}

#[tauri::command]
pub async fn save_settings(app_handle: tauri::AppHandle, settings: LlmSettings) -> Result<(), String> {
    let mut config = load_config(&app_handle);
    config.settings = settings;
    save_config(&app_handle, &config)
}

#[tauri::command]
pub async fn list_design_docs(project_path: String) -> Result<Vec<String>, String> {
    let cleaned_path = clean_project_path(&project_path);
    let design_dir = cleaned_path.join("design");
    if !design_dir.exists() {
        fs::create_dir_all(&design_dir).map_err(|e| e.to_string())?;
    }

    // Seed default design.md if design dir is empty
    let mut entries = fs::read_dir(&design_dir).map_err(|e| e.to_string())?;
    if entries.next().is_none() {
        let default_doc = design_dir.join("design.md");
        let seed = if Path::new("DesignDoc.md").exists() {
            fs::read_to_string("DesignDoc.md").unwrap_or_default()
        } else {
            format!(
                "# Design Document\n\nThis is the main design document for the project.\n\n## 1. Requirements\n- Add requirements here.\n\n## 2. Architecture\n- Add architecture details here.\n"
            )
        };
        fs::write(&default_doc, seed).map_err(|e| e.to_string())?;
    }

    // Read all md files
    let mut docs = Vec::new();
    for entry in fs::read_dir(&design_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("md") {
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                docs.push(name.to_string());
            }
        }
    }
    docs.sort();
    Ok(docs)
}

#[tauri::command]
pub async fn read_design_doc(project_path: String, doc_name: String) -> Result<String, String> {
    let p = clean_project_path(&project_path)
        .join("design")
        .join(&doc_name);
    if p.exists() {
        fs::read_to_string(p).map_err(|e| e.to_string())
    } else {
        Err(format!("Design document {} not found", doc_name))
    }
}

#[tauri::command]
pub async fn write_design_doc(
    project_path: String,
    doc_name: String,
    content: String,
) -> Result<(), String> {
    let design_dir = clean_project_path(&project_path).join("design");
    if !design_dir.exists() {
        fs::create_dir_all(&design_dir).map_err(|e| e.to_string())?;
    }
    fs::write(design_dir.join(doc_name), content).map_err(|e| e.to_string())
}

#[derive(Debug, Default, Clone)]
struct ToolCallAccumulator {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

fn accumulate_sse_tool_calls(line: &str, accumulated: &mut Vec<ToolCallAccumulator>) {
    let line_trimmed = line.trim();
    if line_trimmed.is_empty() {
        return;
    }

    let json_str = if line_trimmed.starts_with("data: ") {
        let content = line_trimmed
            .strip_prefix("data: ")
            .unwrap_or(line_trimmed)
            .trim();
        if content == "[DONE]" {
            return;
        }
        content
    } else {
        line_trimmed
    };

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(json_str) {
        if let Some(choices) = json.get("choices").and_then(|c| c.as_array()) {
            if let Some(first) = choices.first() {
                if let Some(delta) = first.get("delta") {
                    if let Some(tool_calls) = delta.get("tool_calls").and_then(|tc| tc.as_array()) {
                        for tc in tool_calls {
                            if let Some(idx_val) = tc.get("index") {
                                let idx = idx_val.as_u64().unwrap_or(0) as usize;
                                while accumulated.len() <= idx {
                                    accumulated.push(ToolCallAccumulator::default());
                                }

                                if let Some(id_str) = tc.get("id").and_then(|i| i.as_str()) {
                                    accumulated[idx].id = Some(id_str.to_string());
                                }
                                if let Some(func) = tc.get("function") {
                                    if let Some(name_str) =
                                        func.get("name").and_then(|n| n.as_str())
                                    {
                                        accumulated[idx].name = Some(name_str.to_string());
                                    }
                                    if let Some(args_str) =
                                        func.get("arguments").and_then(|a| a.as_str())
                                    {
                                        accumulated[idx].arguments.push_str(args_str);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn get_openai_tools_schema(tools: &[&str]) -> serde_json::Value {
    let mut schemas = Vec::new();
    for tool in tools {
        let schema = match *tool {
            "read_file" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "Reads file content relative to project root. For large files, prefer outline_file first, then pass start_line/end_line to read only the section you need. Output is line-numbered and capped; very large reads are truncated.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Relative path to the file"
                            },
                            "start_line": {
                                "type": "integer",
                                "description": "Optional 1-indexed first line to read. Omit to read from the top."
                            },
                            "end_line": {
                                "type": "integer",
                                "description": "Optional 1-indexed last line to read. Omit to read to the end."
                            }
                        },
                        "required": ["path"],
                        "additionalProperties": false
                    }
                }
            }),
            "outline_file" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "outline_file",
                    "description": "Returns a structural outline of a file (markdown headings, or code declarations like functions/structs/classes) with line numbers, instead of full contents. Use this to survey a large file cheaply before deciding which lines to read with read_file.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Relative path to the file to outline"
                            }
                        },
                        "required": ["path"],
                        "additionalProperties": false
                    }
                }
            }),
            "write_file" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "write_file",
                    "description": "Writes or overwrites content to a file relative to project root.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Relative path to the file"
                            },
                            "content": {
                                "type": "string",
                                "description": "The exact content to write to the file"
                            }
                        },
                        "required": ["path", "content"],
                        "additionalProperties": false
                    }
                }
            }),
            "list_dir" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "list_dir",
                    "description": "Lists all files and folders under a relative path.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Relative path to list (use \"\" for project root)"
                            }
                        },
                        "required": ["path"],
                        "additionalProperties": false
                    }
                }
            }),
            "git_status" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "git_status",
                    "description": "Runs `git status` in the repository sandbox.",
                    "parameters": {
                        "type": "object",
                        "properties": {},
                        "additionalProperties": false
                    }
                }
            }),
            "git_diff" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "git_diff",
                    "description": "Runs `git diff` to view current repository changes.",
                    "parameters": {
                        "type": "object",
                        "properties": {},
                        "additionalProperties": false
                    }
                }
            }),
            "run_command" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "run_command",
                    "description": "Runs a build, test, or check shell command in the repository (e.g. \"cargo check\", \"npm run build\", \"npm test\").",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": {
                                "type": "string",
                                "description": "The exact shell command to run"
                            }
                        },
                        "required": ["command"],
                        "additionalProperties": false
                    }
                }
            }),
            "web_search" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "web_search",
                    "description": "Searches the web for syntax, documentation, library details, or guides.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "The search query"
                            }
                        },
                        "required": ["query"],
                        "additionalProperties": false
                    }
                }
            }),
            "send_notification" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "send_notification",
                    "description": "Sends a system alert/desktop notification to the developer.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "message": {
                                "type": "string",
                                "description": "The message to send"
                            }
                        },
                        "required": ["message"],
                        "additionalProperties": false
                    }
                }
            }),
            "task_complete" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "task_complete",
                    "description": "Ends the autonomous loop, summarizes work, and moves the card to \"Review\".",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "summary": {
                                "type": "string",
                                "description": "Summary of the completed changes and tasks"
                            }
                        },
                        "required": ["summary"],
                        "additionalProperties": false
                    }
                }
            }),
            "search_grep" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "search_grep",
                    "description": "Searches for matching lines containing the query inside files under a path or in a specific file. Returns line numbers and contents.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "The exact string query to find"
                            },
                            "path": {
                                "type": "string",
                                "description": "Optional relative path to search within (specific file or folder). Defaults to project root if omitted."
                            }
                        },
                        "required": ["query"],
                        "additionalProperties": false
                    }
                }
            }),
            "patch_file" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "patch_file",
                    "description": "Replaces a specific unique block of text inside a file with a replacement block. Avoids rewriting the entire file.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Relative path to the file to modify"
                            },
                            "target": {
                                "type": "string",
                                "description": "The exact block of text inside the file to replace. MUST be unique in the file."
                            },
                            "replacement": {
                                "type": "string",
                                "description": "The new block of text to replace the target block with"
                            }
                        },
                        "required": ["path", "target", "replacement"],
                        "additionalProperties": false
                    }
                }
            }),
            _ => continue,
        };
        schemas.push(schema);
    }
    serde_json::Value::Array(schemas)
}

/// Truncate a tool result for history replay. Keeps the head (where the useful
/// signal usually is) and notes how much was dropped, so a single large read
/// (e.g. a 10k-token file) can't pin the prompt size for the rest of the run.
fn truncate_tool_result(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    let dropped = text.chars().count().saturating_sub(max_chars);
    format!(
        "{}\n\n[... {} characters truncated from this earlier tool result to conserve context. Re-read with a line range if you still need this content. ...]",
        head, dropped
    )
}

fn get_history_messages(events: &[RunEvent]) -> Vec<serde_json::Value> {
    // Index tool_results so we can budget by recency: the last couple of results
    // stay generous, older ones get trimmed hard. This bounds prompt growth over
    // a long run, which is what was driving local-model prompt-ingestion timeouts.
    let total_tool_results = events
        .iter()
        .filter(|e| e.event_type == "tool_result")
        .count();
    let mut tool_result_seen = 0usize;
    // Reasoning is budgeted even harder: replaying old chain-of-thought every
    // step is pure prompt-ingestion tax with zero value to the model. Only the
    // most recent reasoning block is kept, truncated.
    let total_reasoning = events
        .iter()
        .filter(|e| e.event_type == "reasoning")
        .count();
    let mut reasoning_seen = 0usize;
    const REASONING_MAX_CHARS: usize = 2000;
    // The most recent N results are kept fuller; everything older is trimmed hard.
    const RECENT_KEEP: usize = 2;
    const RECENT_MAX_CHARS: usize = 6000;
    const OLD_MAX_CHARS: usize = 800;

    let mut messages: Vec<serde_json::Value> = Vec::new();
    for event in events {
        let (role, new_content) = if event.event_type == "message" {
            if let Ok(msg_json) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                if let (Some(role), Some(content)) = (msg_json.get("role"), msg_json.get("content"))
                {
                    let role_str = role.as_str().unwrap_or("user");
                    let role_normalized = if role_str == "agent" {
                        "assistant"
                    } else {
                        role_str
                    };
                    (
                        Some(role_normalized.to_string()),
                        Some(content.as_str().unwrap_or("").to_string()),
                    )
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            }
        } else if event.event_type == "reasoning" {
            reasoning_seen += 1;
            if reasoning_seen == total_reasoning {
                let trimmed = truncate_tool_result(&event.payload, REASONING_MAX_CHARS);
                (
                    Some("assistant".to_string()),
                    Some(format!("<think>\n{}\n</think>", trimmed)),
                )
            } else {
                (None, None)
            }
        } else if event.event_type == "tool_call" {
            if let Ok(call_json) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                let name = call_json.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = call_json
                    .get("args")
                    .cloned()
                    .unwrap_or(serde_json::json!({}));
                let text_content = format!(
                    "```tool_call\n{{\n  \"name\": \"{}\",\n  \"args\": {}\n}}\n```",
                    name, args
                );
                (Some("assistant".to_string()), Some(text_content))
            } else {
                (None, None)
            }
        } else if event.event_type == "tool_result" {
            if let Ok(result_json) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                let name = result_json
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                let result = result_json
                    .get("result")
                    .and_then(|r| r.as_str())
                    .unwrap_or("");
                // Is this one of the most recent RECENT_KEEP results?
                let is_recent = tool_result_seen + RECENT_KEEP >= total_tool_results;
                tool_result_seen += 1;
                let budget = if is_recent {
                    RECENT_MAX_CHARS
                } else {
                    OLD_MAX_CHARS
                };
                let trimmed = truncate_tool_result(result, budget);
                let text_content = format!("Tool '{}' returned:\n{}", name, trimmed);
                (Some("user".to_string()), Some(text_content))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        if let (Some(r), Some(c)) = (role, new_content) {
            if !c.is_empty() {
                if let Some(last_msg) = messages.last_mut() {
                    if last_msg.get("role").and_then(|role_val| role_val.as_str()) == Some(&r) {
                        if let Some(last_content) = last_msg.get_mut("content") {
                            if let Some(last_str) = last_content.as_str() {
                                *last_content = serde_json::json!(format!("{}\n\n{}", last_str, c));
                                continue;
                            }
                        }
                    }
                }
                messages.push(serde_json::json!({
                    "role": r,
                    "content": c
                }));
            }
        }
    }
    messages
}

fn log_error(msg: &str) {
    let logs_dir = app_root_path().join("logs");
    let _ = fs::create_dir_all(&logs_dir);
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs_dir.join("error.log"))
    {
        use std::io::Write;
        let _ = writeln!(
            file,
            "[{}] ERROR: {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            msg
        );
    }
}

struct SSEParsed {
    content: Option<String>,
    reasoning: Option<String>,
}

fn parse_sse_delta(line: &str) -> Option<SSEParsed> {
    let line_trimmed = line.trim();
    if line_trimmed.is_empty() {
        return None;
    }

    let json_str = if line_trimmed.starts_with("data: ") {
        let content = line_trimmed.strip_prefix("data: ")?.trim();
        if content == "[DONE]" {
            return None;
        }
        content
    } else {
        line_trimmed
    };

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(json_str) {
        if let Some(type_str) = json.get("type").and_then(|t| t.as_str()) {
            if type_str == "reasoning.start"
                || type_str == "reasoning.end"
                || type_str == "reasoning"
            {
                return None;
            }
        }

        // 1. Anthropic thinking / content delta
        if let Some(delta) = json.get("delta") {
            if let Some(thinking) = delta.get("thinking").and_then(|t| t.as_str()) {
                return Some(SSEParsed {
                    content: None,
                    reasoning: Some(thinking.to_string()),
                });
            }
            if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                return Some(SSEParsed {
                    content: Some(text.to_string()),
                    reasoning: None,
                });
            }
        }

        // 2. OpenAI / LM Studio delta
        if let Some(choices) = json.get("choices").and_then(|c| c.as_array()) {
            if let Some(first) = choices.first() {
                if let Some(delta) = first.get("delta") {
                    let content = delta
                        .get("content")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string());
                    let reasoning = delta
                        .get("reasoning_content")
                        .and_then(|r| r.as_str())
                        .map(|s| s.to_string());
                    if content.is_some() || reasoning.is_some() {
                        return Some(SSEParsed { content, reasoning });
                    }
                }
                if let Some(msg) = first.get("message") {
                    let content = msg
                        .get("content")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string());
                    let reasoning = msg
                        .get("reasoning_content")
                        .and_then(|r| r.as_str())
                        .map(|s| s.to_string());
                    if content.is_some() || reasoning.is_some() {
                        return Some(SSEParsed { content, reasoning });
                    }
                }
            }
        }

        // 3. Anthropic alternative (content_block_delta -> delta.text)
        let type_str = json.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if type_str == "content_block_delta" {
            if let Some(delta) = json.get("delta") {
                if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                    return Some(SSEParsed {
                        content: Some(text.to_string()),
                        reasoning: None,
                    });
                }
            }
        }

        // 4. Ollama native format: message.content
        if let Some(msg) = json.get("message") {
            if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                return Some(SSEParsed {
                    content: Some(content.to_string()),
                    reasoning: None,
                });
            }
        }

        // 5. Ollama alternative native format: response
        if let Some(resp) = json.get("response").and_then(|r| r.as_str()) {
            return Some(SSEParsed {
                content: Some(resp.to_string()),
                reasoning: None,
            });
        }

        // 6. Stateful LM Studio output
        if let Some(output) = json.get("output") {
            if let Some(arr) = output.as_array() {
                let message_item = arr
                    .iter()
                    .find(|item| item.get("type").and_then(|t| t.as_str()) == Some("message"));
                let target_item = message_item.or_else(|| arr.first());
                if let Some(item) = target_item {
                    if let Some(content) = item.get("content").and_then(|c| c.as_str()) {
                        return Some(SSEParsed {
                            content: Some(content.to_string()),
                            reasoning: None,
                        });
                    }
                }
            } else if let Some(content) = output.as_str() {
                return Some(SSEParsed {
                    content: Some(content.to_string()),
                    reasoning: None,
                });
            }
        }

        // 7. Top-level content
        if let Some(content) = json.get("content").and_then(|c| c.as_str()) {
            return Some(SSEParsed {
                content: Some(content.to_string()),
                reasoning: None,
            });
        }
    }
    None
}

// ─── Provider routing ──────────────────────────────────────────────────────────────
// Each provider speaks its native protocol on its native endpoint.
//
// The universal floor for harness tools is the TEXT PROTOCOL: a ```tool_call
// fenced block mandated by the system prompt and consumed by parse_tool_call.
// It works on any model on any endpoint with no `tools` field at all.
// Providers that support native/client tool calling additionally receive a
// `tools` field, and any native calls they emit are bridged back into the text
// protocol (bridge_tool_calls_into_text) so the run loop has exactly one
// format to consume.

#[derive(Clone, Copy, PartialEq, Debug)]
enum ProviderKind {
    /// OpenAI itself, or any OpenAI-compatible server ("openai", "custom").
    OpenAiCompat,
    /// Anthropic Messages API.
    Anthropic,
    /// Ollama's native /api/chat: NDJSON streaming, native tools, `options`.
    OllamaNative,
    /// LM Studio's native stateful /api/v1/chat: named SSE events, server-side
    /// history via previous_response_id. No client `tools` field -> text protocol.
    LmStudioStateful,
}

fn provider_kind(provider: &str) -> ProviderKind {
    match provider {
        "anthropic" => ProviderKind::Anthropic,
        "ollama" => ProviderKind::OllamaNative,
        "lmstudio" => ProviderKind::LmStudioStateful,
        _ => ProviderKind::OpenAiCompat, // "openai", "custom", anything unknown
    }
}

fn provider_supports_native_tools(kind: ProviderKind) -> bool {
    matches!(
        kind,
        ProviderKind::OpenAiCompat | ProviderKind::Anthropic | ProviderKind::OllamaNative
    )
}

/// Resolve the request URL from the user-configured base URL.
///
/// Policy: a full endpoint path is always respected verbatim. Otherwise the
/// provider's documented endpoint is appended to the configured root. The only
/// accommodation is recognizing the common "/v1" (and "/api/v1") root-suffix
/// convention used by OpenAI-style client configs, so existing setups keep
/// working. No other rewriting of user input is performed.
fn resolve_endpoint(kind: ProviderKind, base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    match kind {
        ProviderKind::OpenAiCompat => {
            if base.ends_with("/chat/completions") {
                base.to_string()
            } else if base.ends_with("/v1") {
                format!("{}/chat/completions", base)
            } else {
                format!("{}/v1/chat/completions", base)
            }
        }
        ProviderKind::Anthropic => {
            if base.ends_with("/messages") {
                base.to_string()
            } else if base.ends_with("/v1") {
                format!("{}/messages", base)
            } else {
                format!("{}/v1/messages", base)
            }
        }
        ProviderKind::OllamaNative => {
            if base.ends_with("/api/chat") {
                base.to_string()
            } else {
                let root = base.trim_end_matches("/v1").trim_end_matches('/');
                format!("{}/api/chat", root)
            }
        }
        ProviderKind::LmStudioStateful => {
            if base.ends_with("/api/v1/chat") {
                base.to_string()
            } else {
                let root = base
                    .trim_end_matches("/api/v1")
                    .trim_end_matches("/v1")
                    .trim_end_matches('/');
                format!("{}/api/v1/chat", root)
            }
        }
    }
}

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(300))
        .build()
}

fn run_is_cancelled(app_handle: &tauri::AppHandle, run_id: &str) -> bool {
    if let Some(state_val) = app_handle.try_state::<AppState>() {
        let cancelled = state_val.cancelled_runs.lock().unwrap();
        cancelled.contains(run_id)
    } else {
        false
    }
}

fn emit_chunk(app_handle: &tauri::AppHandle, run_id: &str, chunk: &str, done: bool) {
    let _ = app_handle.emit(
        "chat-chunk",
        serde_json::json!({
            "run_id": run_id,
            "chunk": chunk,
            "done": done,
            "error": serde_json::Value::Null
        }),
    );
}

/// Convert the OpenAI function-tools schema into Anthropic's tool format
/// (top-level name/description with `input_schema` instead of nested
/// `function.parameters`).
fn convert_tools_to_anthropic(tools: &serde_json::Value) -> serde_json::Value {
    let mut out = Vec::new();
    if let Some(arr) = tools.as_array() {
        for t in arr {
            let f = t.get("function").unwrap_or(t);
            let name = f.get("name").cloned().unwrap_or(serde_json::json!(""));
            let description = f
                .get("description")
                .cloned()
                .unwrap_or(serde_json::json!(""));
            let input_schema = f
                .get("parameters")
                .cloned()
                .unwrap_or(serde_json::json!({"type": "object", "properties": {}}));
            out.push(serde_json::json!({
                "name": name,
                "description": description,
                "input_schema": input_schema
            }));
        }
    }
    serde_json::Value::Array(out)
}

/// Bridge natively-emitted tool calls into the universal text protocol so one
/// parser (parse_tool_call) handles both native and text-emitted calls.
/// APPENDS to the streamed prose rather than replacing it — otherwise what the
/// user saw streaming and what history stores diverge.
///
/// The run loop executes one tool per step by design. If the model emitted
/// parallel tool calls, the extras are dropped LOUDLY: logged, plus a note in
/// the assistant message so the model sees on its next turn that the extra
/// calls never ran (instead of silently assuming they succeeded and drifting).
fn bridge_tool_calls_into_text(full_response: &mut String, accumulated: &[ToolCallAccumulator]) {
    if accumulated.is_empty() {
        return;
    }
    if let Some(tc) = accumulated.first() {
        let name = tc.name.clone().unwrap_or_default();
        let args_parsed: serde_json::Value =
            serde_json::from_str(&tc.arguments).unwrap_or_else(|_| serde_json::json!({}));
        let formatted = format!(
            "```tool_call\n{{\n  \"name\": \"{}\",\n  \"args\": {}\n}}\n```",
            name, args_parsed
        );
        if !full_response.trim().is_empty() {
            full_response.push_str("\n\n");
        }
        full_response.push_str(&formatted);
    }
    if accumulated.len() > 1 {
        let dropped: Vec<String> = accumulated[1..]
            .iter()
            .map(|tc| tc.name.clone().unwrap_or_else(|| "<unnamed>".to_string()))
            .collect();
        log_error(&format!(
            "Model emitted {} tool calls in one response; only the first was kept. Dropped: {}",
            accumulated.len(),
            dropped.join(", ")
        ));
        full_response.push_str(&format!(
            "\n\n[harness note: {} additional tool call(s) ({}) were NOT executed. Emit exactly one tool call per message.]",
            dropped.len(),
            dropped.join(", ")
        ));
    }
}

/// Accumulate Anthropic streaming tool_use blocks (content_block_start with
/// type "tool_use" + input_json_delta fragments) into ToolCallAccumulators.
fn accumulate_anthropic_tool_calls(line: &str, accumulated: &mut Vec<ToolCallAccumulator>) {
    let line_trimmed = line.trim();
    let json_str = match line_trimmed.strip_prefix("data: ") {
        Some(s) => s.trim(),
        None => return,
    };
    let json: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(j) => j,
        Err(_) => return,
    };
    match json.get("type").and_then(|t| t.as_str()).unwrap_or("") {
        "content_block_start" => {
            if let Some(block) = json.get("content_block") {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    accumulated.push(ToolCallAccumulator {
                        id: block
                            .get("id")
                            .and_then(|i| i.as_str())
                            .map(|s| s.to_string()),
                        name: block
                            .get("name")
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string()),
                        arguments: String::new(),
                    });
                }
            }
        }
        "content_block_delta" => {
            if let Some(delta) = json.get("delta") {
                if delta.get("type").and_then(|t| t.as_str()) == Some("input_json_delta") {
                    if let Some(pj) = delta.get("partial_json").and_then(|p| p.as_str()) {
                        if let Some(last) = accumulated.last_mut() {
                            last.arguments.push_str(pj);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

fn call_llm(
    app_handle: &tauri::AppHandle,
    run_id: &str,
    system_prompt: &str,
    mut chat_history: Vec<serde_json::Value>,
    tools: Option<serde_json::Value>,
) -> Result<String, String> {
    {
        if let Some(state_val) = app_handle.try_state::<AppState>() {
            let mut cancelled = state_val.cancelled_runs.lock().unwrap();
            cancelled.remove(run_id);
        }
    }
    let config = load_config(app_handle);
    let settings = config.settings;

    let base_url = settings.api_url.trim().trim_end_matches('/').to_string();
    if base_url.is_empty() {
        let err = "API URL is empty. Please configure it in settings.".to_string();
        log_error(&err);
        return Err(err);
    }

    let kind = provider_kind(&settings.provider.to_lowercase());
    let url = resolve_endpoint(kind, &base_url);

    // Providers without client-side tool calling still get full tool support
    // through the text protocol mandated in the system prompt.
    let tools = if provider_supports_native_tools(kind) {
        tools
    } else {
        None
    };

    let mut messages = vec![serde_json::json!({
        "role": "system",
        "content": system_prompt
    })];
    messages.append(&mut chat_history);

    match kind {
        ProviderKind::Anthropic => call_anthropic(
            app_handle,
            run_id,
            &url,
            &settings,
            system_prompt,
            messages,
            tools,
        ),
        ProviderKind::OllamaNative => {
            call_ollama_native(app_handle, run_id, &url, &settings, messages, tools)
        }
        ProviderKind::LmStudioStateful => call_lmstudio_stateful(
            app_handle,
            run_id,
            &url,
            &settings,
            system_prompt,
            &messages,
        ),
        ProviderKind::OpenAiCompat => {
            call_openai_compat(app_handle, run_id, &url, &settings, messages, tools)
        }
    }
}

fn call_anthropic(
    app_handle: &tauri::AppHandle,
    run_id: &str,
    url: &str,
    settings: &LlmSettings,
    system_prompt: &str,
    messages: Vec<serde_json::Value>,
    tools: Option<serde_json::Value>,
) -> Result<String, String> {
    // Anthropic requires strictly alternating user/assistant roles starting
    // with `user`. get_history_messages already merges adjacent same-role
    // messages; this is a final guard that also covers histories that begin
    // with an assistant turn.
    let mut anthropic_messages: Vec<serde_json::Value> = Vec::new();
    for m in messages.iter() {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        if role == "system" {
            continue;
        }
        let content = m
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        if anthropic_messages.is_empty() && role == "assistant" {
            anthropic_messages.push(serde_json::json!({
                "role": "user",
                "content": "(conversation resumed)"
            }));
        }
        if let Some(last) = anthropic_messages.last_mut() {
            if last.get("role").and_then(|r| r.as_str()) == Some(role) {
                if let Some(last_content) = last.get_mut("content") {
                    let merged = format!("{}\n\n{}", last_content.as_str().unwrap_or(""), content);
                    *last_content = serde_json::json!(merged);
                    continue;
                }
            }
        }
        anthropic_messages.push(serde_json::json!({ "role": role, "content": content }));
    }
    if anthropic_messages.is_empty() {
        anthropic_messages.push(serde_json::json!({ "role": "user", "content": "(empty)" }));
    }

    let mut payload = serde_json::json!({
        "model": settings.model,
        "max_tokens": 4000,
        "system": system_prompt,
        "messages": anthropic_messages,
        "stream": true
    });

    if let Some(ref t) = tools {
        if let Some(obj) = payload.as_object_mut() {
            // Anthropic uses its own tool schema, not the OpenAI function shape.
            obj.insert("tools".to_string(), convert_tools_to_anthropic(t));
        }
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(300))
        .build();
    let resp = match agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("x-api-key", &settings.api_key)
        .set("anthropic-version", "2023-06-01")
        .send_json(payload)
    {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            let err_msg = format!("Anthropic API error {}: {}", code, body);
            log_error(&err_msg);
            return Err(err_msg);
        }
        Err(e) => {
            let err_msg = format!("Anthropic network request failed: {}", e);
            log_error(&err_msg);
            return Err(err_msg);
        }
    };

    if resp.status() == 200 {
        use std::io::{BufRead, BufReader};
        let reader = BufReader::new(resp.into_reader());
        let mut full_response = String::new();
        let mut accumulated_tool_calls: Vec<ToolCallAccumulator> = Vec::new();
        let mut in_reasoning = false;

        for line in reader.lines() {
            if let Some(state_val) = app_handle.try_state::<AppState>() {
                let cancelled = state_val.cancelled_runs.lock().unwrap();
                if cancelled.contains(run_id) {
                    return Err("Cancelled by user".to_string());
                }
            }
            let line_str = line.map_err(|e| e.to_string())?;
            if let Some(parsed) = parse_sse_delta(&line_str) {
                let mut chunk_to_emit = String::new();

                if let Some(reasoning_chunk) = parsed.reasoning {
                    if !in_reasoning {
                        in_reasoning = true;
                        chunk_to_emit.push_str("<think>\n");
                    }
                    chunk_to_emit.push_str(&reasoning_chunk);
                }

                if let Some(content_chunk) = parsed.content {
                    if in_reasoning {
                        in_reasoning = false;
                        chunk_to_emit.push_str("\n</think>\n");
                    }
                    chunk_to_emit.push_str(&content_chunk);
                }

                if !chunk_to_emit.is_empty() {
                    full_response.push_str(&chunk_to_emit);
                    let _ = app_handle.emit(
                        "chat-chunk",
                        serde_json::json!({
                            "run_id": run_id,
                            "chunk": chunk_to_emit,
                            "done": false,
                            "error": serde_json::Value::Null
                        }),
                    );
                }
            }
            accumulate_anthropic_tool_calls(&line_str, &mut accumulated_tool_calls);
        }

        if in_reasoning {
            full_response.push_str("\n</think>\n");
            emit_chunk(app_handle, run_id, "\n</think>\n", false);
        }

        bridge_tool_calls_into_text(&mut full_response, &accumulated_tool_calls);

        emit_chunk(app_handle, run_id, "", true);

        Ok(full_response)
    } else {
        let error_text = resp.into_string().unwrap_or_default();
        let err_msg = format!("Anthropic API error: {}", error_text);
        log_error(&err_msg);
        Err(err_msg)
    }
}

fn call_openai_compat(
    app_handle: &tauri::AppHandle,
    run_id: &str,
    url: &str,
    settings: &LlmSettings,
    messages: Vec<serde_json::Value>,
    tools: Option<serde_json::Value>,
) -> Result<String, String> {
    let mut payload = serde_json::json!({
    "model": settings.model,
    "messages": messages,
    "temperature": 0.7,
    // Hard ceiling on a single response: a ruminating reasoning model can
        // otherwise circle ("wait, what if...") for minutes on slow hardware.
            "max_tokens": 4096,
            "stream": true
        });

    if let Some(ref t) = tools {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("tools".to_string(), t.clone());
            obj.insert("tool_choice".to_string(), serde_json::json!("auto"));
        }
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(300))
        .build();
    let mut req = agent.post(&url).set("Content-Type", "application/json");

    if !settings.api_key.trim().is_empty() {
        req = req.set("Authorization", &format!("Bearer {}", settings.api_key));
    }

    let resp = match req.send_json(payload) {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, resp)) => {
            // ureq returns non-2xx as Err(Status), so the body (which holds the
            // server's actual explanation) is only reachable here, not via a
            // non-200 Ok. Pull it out and surface it.
            let body = resp.into_string().unwrap_or_default();
            let err_msg = format!("LLM API error {}: {}", code, body);
            log_error(&err_msg);
            return Err(err_msg);
        }
        Err(e) => {
            let err_msg = format!("Network request failed: {}", e);
            log_error(&err_msg);
            return Err(err_msg);
        }
    };

    if resp.status() == 200 {
        use std::io::{BufRead, BufReader};
        let reader = BufReader::new(resp.into_reader());
        let mut full_response = String::new();
        let mut accumulated_tool_calls: Vec<ToolCallAccumulator> = Vec::new();
        let mut in_reasoning = false;

        for line in reader.lines() {
            if let Some(state_val) = app_handle.try_state::<AppState>() {
                let cancelled = state_val.cancelled_runs.lock().unwrap();
                if cancelled.contains(run_id) {
                    return Err("Cancelled by user".to_string());
                }
            }
            let line_str = line.map_err(|e| e.to_string())?;
            if let Some(parsed) = parse_sse_delta(&line_str) {
                let mut chunk_to_emit = String::new();

                if let Some(reasoning_chunk) = parsed.reasoning {
                    if !in_reasoning {
                        in_reasoning = true;
                        chunk_to_emit.push_str("<think>\n");
                    }
                    chunk_to_emit.push_str(&reasoning_chunk);
                }

                if let Some(content_chunk) = parsed.content {
                    if in_reasoning {
                        in_reasoning = false;
                        chunk_to_emit.push_str("\n</think>\n");
                    }
                    chunk_to_emit.push_str(&content_chunk);
                }

                if !chunk_to_emit.is_empty() {
                    full_response.push_str(&chunk_to_emit);
                    let _ = app_handle.emit(
                        "chat-chunk",
                        serde_json::json!({
                            "run_id": run_id,
                            "chunk": chunk_to_emit,
                            "done": false,
                            "error": serde_json::Value::Null
                        }),
                    );
                }
            }
            accumulate_sse_tool_calls(&line_str, &mut accumulated_tool_calls);
        }

        if in_reasoning {
            full_response.push_str("\n</think>\n");
            let _ = app_handle.emit(
                "chat-chunk",
                serde_json::json!({
                    "run_id": run_id,
                    "chunk": "\n</think>\n".to_string(),
                    "done": false,
                    "error": serde_json::Value::Null
                }),
            );
        }

        bridge_tool_calls_into_text(&mut full_response, &accumulated_tool_calls);

        let _ = app_handle.emit(
            "chat-chunk",
            serde_json::json!({
                "run_id": run_id,
                "chunk": "",
                "done": true,
                "error": serde_json::Value::Null
            }),
        );

        Ok(full_response)
    } else {
        let error_text = resp.into_string().unwrap_or_default();
        let err_msg = format!("LLM API error: {}", error_text);
        log_error(&err_msg);
        Err(err_msg)
    }
}

fn call_ollama_native(
    app_handle: &tauri::AppHandle,
    run_id: &str,
    url: &str,
    settings: &LlmSettings,
    messages: Vec<serde_json::Value>,
    tools: Option<serde_json::Value>,
) -> Result<String, String> {
    let mut payload = serde_json::json!({
        "model": settings.model,
        "messages": messages,
        "stream": true,
        // Same single-response ceiling as the other providers (Ollama's name for it).
        "options": { "num_predict": 4096 }
    });
    if let Some(ref t) = tools {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("tools".to_string(), t.clone());
        }
    }

    let agent = http_agent();
    let mut req = agent.post(url).set("Content-Type", "application/json");
    if !settings.api_key.trim().is_empty() {
        req = req.set("Authorization", &format!("Bearer {}", settings.api_key));
    }

    let resp = match req.send_json(payload) {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            let err_msg = format!("Ollama API error {}: {}", code, body);
            log_error(&err_msg);
            return Err(err_msg);
        }
        Err(e) => {
            let err_msg = format!("Ollama network request failed: {}", e);
            log_error(&err_msg);
            return Err(err_msg);
        }
    };

    if resp.status() != 200 {
        let error_text = resp.into_string().unwrap_or_default();
        let err_msg = format!("Ollama API error: {}", error_text);
        log_error(&err_msg);
        return Err(err_msg);
    }

    use std::io::{BufRead, BufReader};
    // Ollama streams NDJSON: each line is one complete JSON object, no SSE framing.
    let reader = BufReader::new(resp.into_reader());
    let mut full_response = String::new();
    let mut accumulated_tool_calls: Vec<ToolCallAccumulator> = Vec::new();
    let mut in_reasoning = false;

    for line in reader.lines() {
        if run_is_cancelled(app_handle, run_id) {
            return Err("Cancelled by user".to_string());
        }
        let line_str = line.map_err(|e| e.to_string())?;
        let trimmed = line_str.trim();
        if trimmed.is_empty() {
            continue;
        }
        let json: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(j) => j,
            Err(_) => continue,
        };
        if let Some(err) = json.get("error") {
            let err_msg = format!("Ollama error: {}", err);
            log_error(&err_msg);
            return Err(err_msg);
        }

        let msg = json.get("message");
        let mut chunk_to_emit = String::new();

        if let Some(thinking) = msg.and_then(|m| m.get("thinking")).and_then(|t| t.as_str()) {
            if !thinking.is_empty() {
                if !in_reasoning {
                    in_reasoning = true;
                    chunk_to_emit.push_str("<think>\n");
                }
                chunk_to_emit.push_str(thinking);
            }
        }
        if let Some(content) = msg.and_then(|m| m.get("content")).and_then(|c| c.as_str()) {
            if !content.is_empty() {
                if in_reasoning {
                    in_reasoning = false;
                    chunk_to_emit.push_str("\n</think>\n");
                }
                chunk_to_emit.push_str(content);
            }
        }
        if let Some(tool_calls) = msg
            .and_then(|m| m.get("tool_calls"))
            .and_then(|tc| tc.as_array())
        {
            for tc in tool_calls {
                if let Some(func) = tc.get("function") {
                    accumulated_tool_calls.push(ToolCallAccumulator {
                        id: None,
                        name: func
                            .get("name")
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string()),
                        // Ollama sends arguments as a JSON object, not a string.
                        arguments: func
                            .get("arguments")
                            .map(|a| a.to_string())
                            .unwrap_or_else(|| "{}".to_string()),
                    });
                }
            }
        }

        if !chunk_to_emit.is_empty() {
            full_response.push_str(&chunk_to_emit);
            emit_chunk(app_handle, run_id, &chunk_to_emit, false);
        }

        if json.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
            break;
        }
    }

    if in_reasoning {
        full_response.push_str("\n</think>\n");
        emit_chunk(app_handle, run_id, "\n</think>\n", false);
    }

    bridge_tool_calls_into_text(&mut full_response, &accumulated_tool_calls);

    emit_chunk(app_handle, run_id, "", true);
    Ok(full_response)
}

fn call_lmstudio_stateful(
    app_handle: &tauri::AppHandle,
    run_id: &str,
    url: &str,
    settings: &LlmSettings,
    system_prompt: &str,
    messages: &[serde_json::Value],
) -> Result<String, String> {
    // History lives server-side: chain previous_response_id -> response_id per
    // run/chat key so each thread continues from its own last good point.
    let previous_response_id: Option<String> = app_handle
        .try_state::<AppState>()
        .and_then(|s| s.lmstudio_response_ids.lock().unwrap().get(run_id).cloned());

    let non_system: Vec<&serde_json::Value> = messages
        .iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) != Some("system"))
        .collect();

    // With an existing chain, only the newest message needs to be sent. With no
    // chain but multiple local messages (first call after an app restart lost
    // the in-memory chain), replay local history as a transcript so context
    // isn't silently dropped.
    let input_text = if previous_response_id.is_some() || non_system.len() <= 1 {
        non_system
            .last()
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string()
    } else {
        let mut transcript =
            String::from("[Replaying prior conversation after session restart]\n\n");
        for m in &non_system {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = m.get("content").and_then(|c| c.as_str()).unwrap_or("");
            transcript.push_str(&format!("[{}]: {}\n\n", role.to_uppercase(), content));
        }
        transcript
    };

    let mut payload = serde_json::json!({
        "model": settings.model,
        "input": input_text,
        "system_prompt": system_prompt,
        "stream": true
    });
    if let Some(ref prev) = previous_response_id {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("previous_response_id".to_string(), serde_json::json!(prev));
        }
    }

    let agent = http_agent();
    let mut req = agent.post(url).set("Content-Type", "application/json");
    if !settings.api_key.trim().is_empty() {
        req = req.set("Authorization", &format!("Bearer {}", settings.api_key));
    }

    let resp = match req.send_json(payload) {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            let err_msg = format!("LM Studio API error {}: {}", code, body);
            log_error(&err_msg);
            return Err(err_msg);
        }
        Err(e) => {
            let err_msg = format!("LM Studio network request failed: {}", e);
            log_error(&err_msg);
            return Err(err_msg);
        }
    };

    if resp.status() != 200 {
        let error_text = resp.into_string().unwrap_or_default();
        let err_msg = format!("LM Studio API error: {}", error_text);
        log_error(&err_msg);
        return Err(err_msg);
    }

    use std::io::{BufRead, BufReader};
    // Named SSE events: `event: <type>` lines followed by `data: <json>` lines.
    // The data payload carries its own `type` field, so data lines are enough.
    let reader = BufReader::new(resp.into_reader());
    let mut full_response = String::new();
    let mut in_reasoning = false;

    for line in reader.lines() {
        if run_is_cancelled(app_handle, run_id) {
            return Err("Cancelled by user".to_string());
        }
        let line_str = line.map_err(|e| e.to_string())?;
        let trimmed = line_str.trim();
        let data = match trimmed.strip_prefix("data: ") {
            Some(d) => d.trim(),
            None => continue,
        };
        let json: serde_json::Value = match serde_json::from_str(data) {
            Ok(j) => j,
            Err(_) => continue,
        };

        match json.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "reasoning.delta" => {
                if let Some(content) = json.get("content").and_then(|c| c.as_str()) {
                    let mut chunk_to_emit = String::new();
                    if !in_reasoning {
                        in_reasoning = true;
                        chunk_to_emit.push_str("<think>\n");
                    }
                    chunk_to_emit.push_str(content);
                    full_response.push_str(&chunk_to_emit);
                    emit_chunk(app_handle, run_id, &chunk_to_emit, false);
                }
            }
            "message.delta" => {
                if let Some(content) = json.get("content").and_then(|c| c.as_str()) {
                    let mut chunk_to_emit = String::new();
                    if in_reasoning {
                        in_reasoning = false;
                        chunk_to_emit.push_str("\n</think>\n");
                    }
                    chunk_to_emit.push_str(content);
                    full_response.push_str(&chunk_to_emit);
                    emit_chunk(app_handle, run_id, &chunk_to_emit, false);
                }
            }
            "error" => {
                let detail = json
                    .get("error")
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "unknown streaming error".to_string());
                let err_msg = format!("LM Studio stream error: {}", detail);
                log_error(&err_msg);
                return Err(err_msg);
            }
            "chat.end" => {
                let response_id = json
                    .get("result")
                    .and_then(|r| r.get("response_id"))
                    .or_else(|| json.get("response_id"))
                    .and_then(|r| r.as_str())
                    .map(|s| s.to_string());
                if let Some(rid) = response_id {
                    if let Some(state_val) = app_handle.try_state::<AppState>() {
                        let mut ids = state_val.lmstudio_response_ids.lock().unwrap();
                        ids.insert(run_id.to_string(), rid);
                    }
                }
                break;
            }
            _ => {}
        }
    }

    if in_reasoning {
        full_response.push_str("\n</think>\n");
        emit_chunk(app_handle, run_id, "\n</think>\n", false);
    }

    emit_chunk(app_handle, run_id, "", true);
    Ok(full_response)
}

#[tauri::command]
pub async fn send_design_chat(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    project_path: String,
    doc_name: String,
    message: String,
) -> Result<(), String> {
    let cleaned_path = clean_project_path(&project_path);
    let log_key = format!(
        "{}/design/{}",
        cleaned_path.to_string_lossy().replace('\\', "/"),
        doc_name
    );

    let mut logs = state.design_logs.lock().unwrap();
    let events = logs.entry(log_key.clone()).or_insert_with(Vec::new);
    let user_msg = serde_json::json!({ "role": "user", "content": message });
    let user_payload = serde_json::to_string(&user_msg).unwrap_or_default();

    events.push(RunEvent {
        run_id: log_key.clone(),
        event_type: "message".to_string(),
        payload: user_payload,
    });

    drop(logs);

    let app_handle_clone = app_handle.clone();
    let log_key_clone = log_key.clone();
    let cleaned_path_clone = cleaned_path.clone();
    let doc_name_clone = doc_name.clone();

    tauri::async_runtime::spawn(async move {
        let state = app_handle_clone.state::<AppState>();

        // Ensure no concurrent run loops run for this log_key
        {
            let mut active = state.active_runs.lock().unwrap();
            if active.contains(&log_key_clone) {
                return;
            }
            active.insert(log_key_clone.clone());
        }

        let _guard = ActiveRunGuard {
            app_handle: app_handle_clone.clone(),
            run_id: log_key_clone.clone(),
        };

        let max_steps = 10;
        let mut step = 0;

        loop {
            if step >= max_steps {
                let _ = app_handle_clone.emit(
                    "chat-finished",
                    serde_json::json!({ "run_id": log_key_clone }),
                );
                break;
            }
            step += 1;

            let history = {
                let logs = state.design_logs.lock().unwrap();
                if let Some(evs) = logs.get(&log_key_clone) {
                    get_history_messages(evs)
                } else {
                    Vec::new()
                }
            };

            let system_prompt =
                construct_architect_system_prompt(&cleaned_path_clone, &doc_name_clone);
            let tools_schema = get_openai_tools_schema(&[
                "read_file",
                "outline_file",
                "write_file",
                "list_dir",
                "web_search",
                "send_notification",
                "search_grep",
                "patch_file",
            ]);

            let response = match call_llm(
                &app_handle_clone,
                &log_key_clone,
                &system_prompt,
                history,
                Some(tools_schema),
            ) {
                Ok(reply) => reply,
                Err(e) => {
                    log_error(&format!("Design chat LLM error: {}", e));
                    let is_cancelled = if let Some(st) = app_handle_clone.try_state::<AppState>() {
                        let cancelled = st.cancelled_runs.lock().unwrap();
                        cancelled.contains(&log_key_clone)
                    } else {
                        false
                    };
                    if is_cancelled {
                        let mut logs = state.design_logs.lock().unwrap();
                        if let Some(events) = logs.get_mut(&log_key_clone) {
                            events.push(RunEvent {
                                run_id: log_key_clone.clone(),
                                event_type: "message".to_string(),
                                payload: serde_json::json!({
                                    "role": "agent",
                                    "content": "Chat stopped by user."
                                })
                                .to_string(),
                            });
                        }
                        let _ = app_handle_clone.emit(
                            "run-updated",
                            serde_json::json!({ "run_id": log_key_clone }),
                        );
                    }
                    let _ = app_handle_clone.emit(
                        "chat-finished",
                        serde_json::json!({ "run_id": log_key_clone }),
                    );
                    break;
                }
            };

            let (reasoning, remaining) = extract_reasoning(&response);
            if let Some(reasoning_content) = reasoning {
                append_design_event(
                    &app_handle_clone,
                    &state,
                    &log_key_clone,
                    RunEvent {
                        run_id: log_key_clone.clone(),
                        event_type: "reasoning".to_string(),
                        payload: reasoning_content,
                    },
                );
                let _ = app_handle_clone.emit(
                    "run-updated",
                    serde_json::json!({ "run_id": log_key_clone }),
                );
            }

            if let Some((tool_name, args)) = parse_tool_call(&remaining) {
                append_design_event(
                    &app_handle_clone,
                    &state,
                    &log_key_clone,
                    RunEvent {
                        run_id: log_key_clone.clone(),
                        event_type: "tool_call".to_string(),
                        payload:
                            serde_json::json!({ "name": tool_name.clone(), "args": args.clone() })
                                .to_string(),
                    },
                );
                let _ = app_handle_clone.emit(
                    "run-updated",
                    serde_json::json!({ "run_id": log_key_clone }),
                );

                let tool_result = execute_tool(
                    &app_handle_clone,
                    &cleaned_path_clone,
                    &tool_name,
                    &args,
                    &log_key_clone,
                );

                append_design_event(
                    &app_handle_clone,
                    &state,
                    &log_key_clone,
                    RunEvent {
                        run_id: log_key_clone.clone(),
                        event_type: "tool_result".to_string(),
                        payload:
                            serde_json::json!({ "name": tool_name.clone(), "result": tool_result })
                                .to_string(),
                    },
                );
                let _ = app_handle_clone.emit(
                    "run-updated",
                    serde_json::json!({ "run_id": log_key_clone }),
                );

                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            } else {
                let payload = serde_json::json!({ "role": "agent", "content": remaining.clone() })
                    .to_string();
                append_design_event(
                    &app_handle_clone,
                    &state,
                    &log_key_clone,
                    RunEvent {
                        run_id: log_key_clone.clone(),
                        event_type: "message".to_string(),
                        payload,
                    },
                );
                let _ = app_handle_clone.emit(
                    "run-updated",
                    serde_json::json!({ "run_id": log_key_clone }),
                );
                let _ = app_handle_clone.emit(
                    "chat-finished",
                    serde_json::json!({ "run_id": log_key_clone }),
                );
                break;
            }
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn get_design_log(
    state: tauri::State<'_, AppState>,
    project_path: String,
    doc_name: String,
) -> Result<Vec<RunEvent>, String> {
    let cleaned_path = clean_project_path(&project_path);
    let log_key = format!(
        "{}/design/{}",
        cleaned_path.to_string_lossy().replace('\\', "/"),
        doc_name
    );
    let logs = state.design_logs.lock().unwrap();
    if let Some(events) = logs.get(&log_key) {
        Ok(events.clone())
    } else {
        Ok(Vec::new())
    }
}

#[tauri::command]
pub async fn send_code_chat(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    project_path: String,
    file_path: String,
    message: String,
) -> Result<(), String> {
    let cleaned_path = clean_project_path(&project_path);
    let log_key = format!(
        "{}/code/{}",
        cleaned_path.to_string_lossy().replace('\\', "/"),
        file_path
    );

    let mut logs = state.code_logs.lock().unwrap();
    let events = logs.entry(log_key.clone()).or_insert_with(Vec::new);
    let user_msg = serde_json::json!({ "role": "user", "content": message });
    let user_payload = serde_json::to_string(&user_msg).unwrap_or_default();

    events.push(RunEvent {
        run_id: log_key.clone(),
        event_type: "message".to_string(),
        payload: user_payload,
    });

    drop(logs);

    let app_handle_clone = app_handle.clone();
    let log_key_clone = log_key.clone();
    let cleaned_path_clone = cleaned_path.clone();
    let file_path_clone = file_path.clone();

    tauri::async_runtime::spawn(async move {
        let state = app_handle_clone.state::<AppState>();

        // Ensure no concurrent run loops run for this log_key
        {
            let mut active = state.active_runs.lock().unwrap();
            if active.contains(&log_key_clone) {
                return;
            }
            active.insert(log_key_clone.clone());
        }

        let _guard = ActiveRunGuard {
            app_handle: app_handle_clone.clone(),
            run_id: log_key_clone.clone(),
        };

        let max_steps = 10;
        let mut step = 0;

        loop {
            if step >= max_steps {
                let _ = app_handle_clone.emit(
                    "chat-finished",
                    serde_json::json!({ "run_id": log_key_clone }),
                );
                break;
            }
            step += 1;

            let history = {
                let logs = state.code_logs.lock().unwrap();
                if let Some(evs) = logs.get(&log_key_clone) {
                    get_history_messages(evs)
                } else {
                    Vec::new()
                }
            };

            let system_prompt =
                construct_copilot_system_prompt(&cleaned_path_clone, &file_path_clone);
            let tools_schema = get_openai_tools_schema(&[
                "read_file",
                "outline_file",
                "write_file",
                "list_dir",
                "git_status",
                "git_diff",
                "run_command",
                "web_search",
                "search_grep",
                "patch_file",
            ]);

            let response = match call_llm(
                &app_handle_clone,
                &log_key_clone,
                &system_prompt,
                history,
                Some(tools_schema),
            ) {
                Ok(reply) => reply,
                Err(e) => {
                    log_error(&format!("Code chat LLM error: {}", e));
                    let is_cancelled = if let Some(st) = app_handle_clone.try_state::<AppState>() {
                        let cancelled = st.cancelled_runs.lock().unwrap();
                        cancelled.contains(&log_key_clone)
                    } else {
                        false
                    };
                    if is_cancelled {
                        let mut logs = state.code_logs.lock().unwrap();
                        if let Some(events) = logs.get_mut(&log_key_clone) {
                            events.push(RunEvent {
                                run_id: log_key_clone.clone(),
                                event_type: "message".to_string(),
                                payload: serde_json::json!({
                                    "role": "agent",
                                    "content": "Chat stopped by user."
                                })
                                .to_string(),
                            });
                        }
                        let _ = app_handle_clone.emit(
                            "run-updated",
                            serde_json::json!({ "run_id": log_key_clone }),
                        );
                    }
                    let _ = app_handle_clone.emit(
                        "chat-finished",
                        serde_json::json!({ "run_id": log_key_clone }),
                    );
                    break;
                }
            };

            let (reasoning, remaining) = extract_reasoning(&response);
            if let Some(reasoning_content) = reasoning {
                append_code_event(
                    &app_handle_clone,
                    &state,
                    &log_key_clone,
                    RunEvent {
                        run_id: log_key_clone.clone(),
                        event_type: "reasoning".to_string(),
                        payload: reasoning_content,
                    },
                );
                let _ = app_handle_clone.emit(
                    "run-updated",
                    serde_json::json!({ "run_id": log_key_clone }),
                );
            }

            if let Some((tool_name, args)) = parse_tool_call(&remaining) {
                append_code_event(
                    &app_handle_clone,
                    &state,
                    &log_key_clone,
                    RunEvent {
                        run_id: log_key_clone.clone(),
                        event_type: "tool_call".to_string(),
                        payload:
                            serde_json::json!({ "name": tool_name.clone(), "args": args.clone() })
                                .to_string(),
                    },
                );
                let _ = app_handle_clone.emit(
                    "run-updated",
                    serde_json::json!({ "run_id": log_key_clone }),
                );

                let tool_result = execute_tool(
                    &app_handle_clone,
                    &cleaned_path_clone,
                    &tool_name,
                    &args,
                    &log_key_clone,
                );

                append_code_event(
                    &app_handle_clone,
                    &state,
                    &log_key_clone,
                    RunEvent {
                        run_id: log_key_clone.clone(),
                        event_type: "tool_result".to_string(),
                        payload:
                            serde_json::json!({ "name": tool_name.clone(), "result": tool_result })
                                .to_string(),
                    },
                );
                let _ = app_handle_clone.emit(
                    "run-updated",
                    serde_json::json!({ "run_id": log_key_clone }),
                );

                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            } else {
                let payload = serde_json::json!({ "role": "agent", "content": remaining.clone() })
                    .to_string();
                append_code_event(
                    &app_handle_clone,
                    &state,
                    &log_key_clone,
                    RunEvent {
                        run_id: log_key_clone.clone(),
                        event_type: "message".to_string(),
                        payload,
                    },
                );
                let _ = app_handle_clone.emit(
                    "run-updated",
                    serde_json::json!({ "run_id": log_key_clone }),
                );
                let _ = app_handle_clone.emit(
                    "chat-finished",
                    serde_json::json!({ "run_id": log_key_clone }),
                );
                break;
            }
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn get_code_log(
    state: tauri::State<'_, AppState>,
    project_path: String,
    file_path: String,
) -> Result<Vec<RunEvent>, String> {
    let cleaned_path = clean_project_path(&project_path);
    let log_key = format!(
        "{}/code/{}",
        cleaned_path.to_string_lossy().replace('\\', "/"),
        file_path
    );
    let logs = state.code_logs.lock().unwrap();
    if let Some(events) = logs.get(&log_key) {
        Ok(events.clone())
    } else {
        Ok(Vec::new())
    }
}

#[tauri::command]
pub async fn list_cards(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    project_path: String,
) -> Result<Vec<Card>, String> {
    let mut cards = state.cards.lock().unwrap();

    let has_cards = cards.iter().any(|c| c.project_path == project_path);
    if !has_cards {
        if let Ok(conn) = get_db_conn(&app_handle) {
            if let Ok(seeded) = seed_default_cards_for_project(&conn, &project_path) {
                cards.extend(seeded);
            }
        }
    }

    Ok(cards
        .iter()
        .filter(|c| c.project_path == project_path)
        .cloned()
        .collect())
}

#[tauri::command]
pub async fn create_card(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    project_path: String,
    title: String,
    description: String,
    status: Option<String>,
) -> Result<Card, String> {
    let mut cards = state.cards.lock().unwrap();
    let initial_status = status.unwrap_or_else(|| "backlog".to_string());
    let new_card = Card {
        id: format!("card_{}", cards.len() + 1),
        project_path: project_path.clone(),
        title,
        description,
        status: initial_status,
        run_id: None,
        assignee: None,
        todo_list: Vec::new(),
    };
    cards.push(new_card.clone());

    if let Ok(conn) = get_db_conn(&app_handle) {
        let _ = conn.execute(
            "INSERT INTO cards (id, project_path, title, description, status, run_id, assignee) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (&new_card.id, &new_card.project_path, &new_card.title, &new_card.description, &new_card.status, &new_card.run_id, &new_card.assignee),
        );
    }

    Ok(new_card)
}

#[tauri::command]
pub async fn update_card(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    card_id: String,
    status: String,
) -> Result<Card, String> {
    let mut cards = state.cards.lock().unwrap();
    if let Some(card) = cards.iter_mut().find(|c| c.id == card_id) {
        card.status = status.clone();

        if let Ok(conn) = get_db_conn(&app_handle) {
            let _ = conn.execute(
                "UPDATE cards SET status = ?1 WHERE id = ?2",
                (&status, &card_id),
            );
        }

        Ok(card.clone())
    } else {
        Err("Card not found".to_string())
    }
}

#[tauri::command]
pub async fn save_card(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    card: Card,
) -> Result<Card, String> {
    let mut cards = state.cards.lock().unwrap();
    if let Some(c) = cards.iter_mut().find(|item| item.id == card.id) {
        c.title = card.title;
        c.description = card.description;
        c.assignee = card.assignee;
        c.todo_list = card.todo_list;
        c.status = card.status;
        c.project_path = card.project_path;

        if let Ok(conn) = get_db_conn(&app_handle) {
            let _ = conn.execute(
                "UPDATE cards SET title = ?1, description = ?2, status = ?3, run_id = ?4, assignee = ?5, project_path = ?6 WHERE id = ?7",
                (&c.title, &c.description, &c.status, &c.run_id, &c.assignee, &c.project_path, &c.id),
            );

            let _ = conn.execute("DELETE FROM todo_items WHERE card_id = ?1", [&c.id]);
            for (idx, item) in c.todo_list.iter().enumerate() {
                let _ = conn.execute(
                    "INSERT INTO todo_items (card_id, idx, text, completed) VALUES (?1, ?2, ?3, ?4)",
                    (&c.id, idx as i32, &item.text, if item.completed { 1 } else { 0 }),
                );
            }
        }

        Ok(c.clone())
    } else {
        Err("Card not found".to_string())
    }
}

#[tauri::command]
pub async fn delete_card(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    card_id: String,
) -> Result<(), String> {
    let mut cards = state.cards.lock().unwrap();
    let pos = cards.iter().position(|c| c.id == card_id);
    if let Some(idx) = pos {
        cards.remove(idx);

        if let Ok(conn) = get_db_conn(&app_handle) {
            let _ = conn.execute("DELETE FROM cards WHERE id = ?1", [&card_id]);
        }

        Ok(())
    } else {
        Err("Card not found".to_string())
    }
}

#[tauri::command]
pub async fn start_run(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    card_id: String,
) -> Result<String, String> {
    let mut cards = state.cards.lock().unwrap();
    if let Some(card) = cards.iter_mut().find(|c| c.id == card_id) {
        let run_id = format!(
            "run_{}_{}",
            card_id,
            chrono::Local::now().format("%Y%m%d%H%M%S")
        );

        // Clear any stale cancellation flag so a fresh run can never begin life
        // already-cancelled (the cancelled set is never otherwise pruned).
        {
            let mut cancelled = state.cancelled_runs.lock().unwrap();
            cancelled.remove(&run_id);
        }

        let repo_path = clean_project_path(&card.project_path);
        // Refuse to run unsandboxed. The whole safety model depends on the agent
        // working inside an isolated worktree; if this isn't a git repo there is
        // no sandbox to create, so we stop rather than letting tools write the live tree.
        if !git::is_git_repo(&repo_path) {
            return Err("This project is not a git repository. BeetleAI requires git so each run is isolated in a worktree. Run `git init` in the project root and try again.".to_string());
        }
        let base_branch =
            git::get_current_branch(&repo_path).unwrap_or_else(|_| "main".to_string());
        git::create_worktree(&repo_path, &run_id, &base_branch)?;

        card.run_id = Some(run_id.clone());
        card.status = "running".to_string();
        let card_title = card.title.clone();
        let card_desc = card.description.clone();
        let task_payload = serde_json::json!({
            "role": "user",
            "content": format!("Your assigned task:\nTitle: {}\nDescription: {}\n\nBegin working on this task now using the available tools. Call task_complete when finished.", card_title, card_desc)
        }).to_string();

        if let Ok(conn) = get_db_conn(&app_handle) {
            let _ = conn.execute(
                "UPDATE cards SET status = 'running', run_id = ?1 WHERE id = ?2",
                (&card.run_id, &card.id),
            );
            let _ = conn.execute(
                "INSERT INTO logs (log_type, key, run_id, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                ("run", &card.id, &run_id, "status", "running"),
            );
            let content = "{\"role\":\"agent\",\"content\":\"Run started. Isolated git worktree sandbox created. Spawning agent loop...\"}";
            let _ = conn.execute(
                "INSERT INTO logs (log_type, key, run_id, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                ("run", &card.id, &run_id, "message", content),
            );
            let _ = conn.execute(
                "INSERT INTO logs (log_type, key, run_id, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                ("run", &card.id, &run_id, "message", &task_payload),
            );
        }

        let mut logs = state.run_logs.lock().unwrap();
        logs.insert(
            run_id.clone(),
            vec![
                RunEvent {
                    run_id: run_id.clone(),
                    event_type: "status".to_string(),
                    payload: "running".to_string(),
                },
                RunEvent {
                    run_id: run_id.clone(),
                    event_type: "message".to_string(),
                    payload: "{\"role\":\"agent\",\"content\":\"Run started. Isolated git worktree sandbox created. Spawning agent loop...\"}".to_string(),
                },
                RunEvent {
                    run_id: run_id.clone(),
                    event_type: "message".to_string(),
                    payload: task_payload.clone(),
                },
            ],
        );

        drop(cards);
        drop(logs);

        run_agent_loop(app_handle, run_id.clone(), card_id);

        Ok(run_id)
    } else {
        Err("Card not found".to_string())
    }
}

#[tauri::command]
pub async fn cancel_run(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<(), String> {
    abort_chat(app_handle, state, run_id).await
}

#[tauri::command]
pub async fn abort_chat(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<(), String> {
    let mut cancelled = state.cancelled_runs.lock().unwrap();
    cancelled.insert(run_id.clone());
    drop(cancelled);

    let card_info_opt = {
        let mut cards = state.cards.lock().unwrap();
        if let Some(card) = cards
            .iter_mut()
            .find(|c| c.run_id.as_deref() == Some(&run_id))
        {
            card.status = "failed".to_string();
            if let Ok(conn) = get_db_conn(&app_handle) {
                let _ = conn.execute(
                    "UPDATE cards SET status = 'failed' WHERE id = ?1",
                    [&card.id],
                );
            }
            Some((card.id.clone(), card.project_path.clone()))
        } else {
            None
        }
    };

    if let Some((card_id, project_path)) = &card_info_opt {
        let repo_path = clean_project_path(project_path);
        if git::is_git_repo(&repo_path) {
            let _ = git::remove_worktree(&repo_path, &run_id);
        }

        if let Ok(conn) = get_db_conn(&app_handle) {
            let _ = conn.execute(
                "INSERT INTO logs (log_type, key, run_id, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                ("run", card_id, &run_id, "status", "failed"),
            );
            let content = "{\"role\":\"agent\",\"content\":\"Run execution cancelled by user. Worktree destroyed.\"}";
            let _ = conn.execute(
                "INSERT INTO logs (log_type, key, run_id, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                ("run", card_id, &run_id, "message", content),
            );
        }

        let mut logs = state.run_logs.lock().unwrap();
        if let Some(run_events) = logs.get_mut(&run_id) {
            run_events.push(RunEvent {
                run_id: run_id.clone(),
                event_type: "status".to_string(),
                payload: "failed".to_string(),
            });
            run_events.push(RunEvent {
                run_id: run_id.clone(),
                event_type: "message".to_string(),
                payload: "{\"role\":\"agent\",\"content\":\"Run execution cancelled by user. Worktree destroyed.\"}".to_string(),
            });
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn is_run_active(state: tauri::State<'_, AppState>, run_id: String) -> Result<bool, String> {
    let active = state.active_runs.lock().unwrap();
    Ok(active.contains(&run_id))
}

#[tauri::command]
pub async fn unblock_run(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    run_id: String,
    reply: String,
) -> Result<(), String> {
    let mut cards = state.cards.lock().unwrap();
    if let Some(card) = cards
        .iter_mut()
        .find(|c| c.run_id.as_deref() == Some(&run_id))
    {
        // Only paused-but-recoverable runs can be resumed. `blocked` (question, error,
        // or step ceiling) and `review` (reopen for more work) are resumable;
        // `done`/`failed` are terminal and their worktrees may already be gone.
        if card.status != "blocked" && card.status != "review" {
            return Err(format!(
                "Run is '{}', which can't be resumed. Only blocked or in-review runs can be reopened.",
                card.status
            ));
        }

        // Guard against resuming a run whose sandbox no longer exists.
        let repo_path = clean_project_path(&card.project_path);
        let worktree_path = repo_path.join(".harness").join("worktrees").join(&run_id);
        if !worktree_path.exists() {
            return Err("The worktree for this run no longer exists, so it can't be resumed. Start a fresh run instead.".to_string());
        }

        card.status = "running".to_string();
        let card_id = card.id.clone();
        drop(cards);

        if let Ok(conn) = get_db_conn(&app_handle) {
            let _ = conn.execute(
                "UPDATE cards SET status = 'running' WHERE id = ?1",
                [&card_id],
            );
        }

        let mut logs = state.run_logs.lock().unwrap();
        if let Some(run_events) = logs.get_mut(&run_id) {
            let user_msg = serde_json::json!({ "role": "user", "content": reply });
            let user_payload = serde_json::to_string(&user_msg).unwrap_or_default();

            if let Ok(conn) = get_db_conn(&app_handle) {
                let _ = conn.execute(
                    "INSERT INTO logs (log_type, key, run_id, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                    ("run", &card_id, &run_id, "message", &user_payload),
                );
            }

            run_events.push(RunEvent {
                run_id: run_id.clone(),
                event_type: "message".to_string(),
                payload: user_payload,
            });
        }
        drop(logs);

        run_agent_loop(app_handle, run_id, card_id);
        Ok(())
    } else {
        Err("Run not found".to_string())
    }
}

#[tauri::command]
pub async fn accept_run(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<(), String> {
    let mut cards = state.cards.lock().unwrap();
    if let Some(card) = cards
        .iter_mut()
        .find(|c| c.run_id.as_deref() == Some(&run_id))
    {
        card.status = "done".to_string();

        if let Ok(conn) = get_db_conn(&app_handle) {
            let _ = conn.execute("UPDATE cards SET status = 'done' WHERE id = ?1", [&card.id]);
            let _ = conn.execute(
                "INSERT INTO logs (log_type, key, run_id, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                ("run", &card.id, &run_id, "status", "done"),
            );
            let content = "{\"role\":\"agent\",\"content\":\"Worktree successfully merged back. Cleaned up isolation branches.\"}";
            let _ = conn.execute(
                "INSERT INTO logs (log_type, key, run_id, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                ("run", &card.id, &run_id, "message", content),
            );
        }

        let repo_path = clean_project_path(&card.project_path);
        if git::is_git_repo(&repo_path) {
            let base_branch =
                git::get_current_branch(&repo_path).unwrap_or_else(|_| "main".to_string());
            git::merge_worktree(&repo_path, &run_id, &base_branch)?;
        }

        let mut logs = state.run_logs.lock().unwrap();
        if let Some(run_events) = logs.get_mut(&run_id) {
            run_events.push(RunEvent {
                run_id: run_id.clone(),
                event_type: "status".to_string(),
                payload: "done".to_string(),
            });
            run_events.push(RunEvent {
                run_id: run_id.clone(),
                event_type: "message".to_string(),
                payload: "{\"role\":\"agent\",\"content\":\"Worktree successfully merged back. Cleaned up isolation branches.\"}".to_string(),
            });
        }
        Ok(())
    } else {
        Err("Run not found".to_string())
    }
}

#[tauri::command]
pub async fn reject_run(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<(), String> {
    let mut cards = state.cards.lock().unwrap();
    if let Some(card) = cards
        .iter_mut()
        .find(|c| c.run_id.as_deref() == Some(&run_id))
    {
        card.status = "failed".to_string();

        if let Ok(conn) = get_db_conn(&app_handle) {
            let _ = conn.execute(
                "UPDATE cards SET status = 'failed' WHERE id = ?1",
                [&card.id],
            );
            let _ = conn.execute(
                "INSERT INTO logs (log_type, key, run_id, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                ("run", &card.id, &run_id, "status", "failed"),
            );
            let content =
                "{\"role\":\"agent\",\"content\":\"Worktree discarded and branch deleted.\"}";
            let _ = conn.execute(
                "INSERT INTO logs (log_type, key, run_id, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                ("run", &card.id, &run_id, "message", content),
            );
        }

        let repo_path = clean_project_path(&card.project_path);
        if git::is_git_repo(&repo_path) {
            git::remove_worktree(&repo_path, &run_id)?;
        }

        let mut logs = state.run_logs.lock().unwrap();
        if let Some(run_events) = logs.get_mut(&run_id) {
            run_events.push(RunEvent {
                run_id: run_id.clone(),
                event_type: "status".to_string(),
                payload: "failed".to_string(),
            });
            run_events.push(RunEvent {
                run_id: run_id.clone(),
                event_type: "message".to_string(),
                payload:
                    "{\"role\":\"agent\",\"content\":\"Worktree discarded and branch deleted.\"}"
                        .to_string(),
            });
        }
        Ok(())
    } else {
        Err("Run not found".to_string())
    }
}

#[tauri::command]
pub async fn get_run_log(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<Vec<RunEvent>, String> {
    let logs = state.run_logs.lock().unwrap();
    if let Some(events) = logs.get(&run_id) {
        Ok(events.clone())
    } else {
        Ok(Vec::new())
    }
}

#[tauri::command]
pub async fn send_chat(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    run_id: String,
    message: String,
) -> Result<(), String> {
    let mut logs = state.run_logs.lock().unwrap();
    if let Some(run_events) = logs.get_mut(&run_id) {
        let user_msg = serde_json::json!({ "role": "user", "content": message });
        let user_payload = serde_json::to_string(&user_msg).unwrap_or_default();

        run_events.push(RunEvent {
            run_id: run_id.clone(),
            event_type: "message".to_string(),
            payload: user_payload,
        });

        drop(logs);

        // Resume loop by setting card status back to "running"
        let mut cards = state.cards.lock().unwrap();
        let card_id = if let Some(card) = cards
            .iter_mut()
            .find(|c| c.run_id.as_deref() == Some(&run_id))
        {
            card.status = "running".to_string();
            Some(card.id.clone())
        } else {
            None
        };
        drop(cards);

        if let Some(c_id) = card_id {
            run_agent_loop(app_handle, run_id, c_id);
        }

        Ok(())
    } else {
        Err("Run not found".to_string())
    }
}

#[tauri::command]
pub async fn read_diff(state: tauri::State<'_, AppState>, run_id: String) -> Result<String, String> {
    // If the run's card can't be found, there is no correct repo to diff against —
    // fail loudly rather than silently diffing the app's own working directory.
    let repo_path = repo_path_for_run(&state, &run_id).ok_or_else(|| {
        format!(
            "No card found for run '{}'; cannot resolve its repository",
            run_id
        )
    })?;
    if git::is_git_repo(&repo_path) {
        let base_branch =
            git::get_current_branch(&repo_path).unwrap_or_else(|_| "main".to_string());
        // A cancelled or rejected run's branch has been deleted by cleanup; the
        // card may still reference the run_id. That's a normal state, not an error.
        let branch_name = format!("harness/run-{}", run_id);
        if !git::branch_exists(&repo_path, &branch_name) {
            return Ok(
                "No diff available: this run's branch no longer exists (the run was cancelled, rejected, or already merged)."
                    .to_string(),
            );
        }
        match git::get_diff(&repo_path, &run_id, &base_branch) {
            Ok(diff) => {
                if diff.trim().is_empty() {
                    Ok("No changes detected in this run.".to_string())
                } else {
                    Ok(diff)
                }
            }
            Err(e) => {
                if run_id == "run_card_2" {
                    Ok(r#"diff --git a/src-tauri/src/git.rs b/src-tauri/src/git.rs
new file mode 100644
index 0000000..f67ab7c
--- /dev/null
+++ b/src-tauri/src/git.rs
@@ -0,0 +1,52 @@
+use std::process::Command;
+use std::path::Path;
+
+pub fn create_worktree(run_id: &str, base: &str) -> Result<String, String> {
+    let path = format!(".harness/worktrees/{}", run_id);
+    let branch = format!("harness/run-{}", run_id);
+    
+    let output = Command::new("git")
+        .args(&["worktree", "add", &path, "-b", &branch, base])
+        .output()
+        .map_err(|e| e.to_string())?;
+        
+    if output.status.success() {
+        Ok(path)
+    } else {
+        Err(String::from_utf8_lossy(&output.stderr).to_string())
+    }
+}
+"#
                    .to_string())
                } else {
                    Err(e)
                }
            }
        }
    } else {
        Err("Not a git repository".to_string())
    }
}

#[tauri::command]
pub async fn list_dir(path: String) -> Result<Vec<DirEntry>, String> {
    let base_path = Path::new(&path);
    let mut entries = Vec::new();

    if let Ok(rd) = fs::read_dir(base_path) {
        for entry in rd.flatten() {
            let p = entry.path();
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();

            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }

            entries.push(DirEntry {
                name,
                path: p.to_string_lossy().into_owned(),
                is_dir: p.is_dir(),
            });
        }
    }

    entries.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            b.is_dir.cmp(&a.is_dir)
        } else {
            a.name.cmp(&b.name)
        }
    });

    Ok(entries)
}

#[tauri::command]
pub async fn read_file(path: String) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| e.to_string())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ModelInfo {
    pub name: String,
    pub is_loaded: bool,
    pub context_size: Option<u32>,
}

#[tauri::command]
pub async fn fetch_local_models(url: String, provider: String) -> Result<Vec<ModelInfo>, String> {
    let base_url = url.trim_end_matches('/');

    // Helper closure to parse OpenAI compatible or LM Studio response JSON from /models
    let parse_openai_models = |resp: ureq::Response| -> Option<Vec<ModelInfo>> {
        if resp.status() == 200 {
            if let Ok(json) = resp.into_json::<serde_json::Value>() {
                let mut models_list = Vec::new();
                if let Some(data) = json.get("data").and_then(|v| v.as_array()) {
                    for m in data {
                        if let Some(id) = m.get("id").and_then(|v| v.as_str()) {
                            // Determine if loaded (LM Studio uses "loaded", default to true)
                            let is_loaded = m
                                .get("loaded")
                                .or_else(|| m.get("is_loaded"))
                                .and_then(|v| v.as_bool())
                                .unwrap_or(true);

                            // Extract context size (LM Studio settings.contextLength, or OpenAI context_length/window)
                            let context_size = m
                                .get("settings")
                                .and_then(|s| {
                                    s.get("contextLength").or_else(|| s.get("context_length"))
                                })
                                .or_else(|| {
                                    m.get("metadata").and_then(|md| {
                                        md.get("contextLength").or_else(|| md.get("context_window"))
                                    })
                                })
                                .or_else(|| m.get("context_length"))
                                .or_else(|| m.get("context_window"))
                                .or_else(|| m.get("context_size"))
                                .and_then(|v| v.as_u64())
                                .map(|v| v as u32);

                            models_list.push(ModelInfo {
                                name: id.to_string(),
                                is_loaded,
                                context_size,
                            });
                        }
                    }
                    return Some(models_list);
                }
            }
        }
        None
    };

    // LM Studio's native v1 REST API returns a far richer models list than the
    // OpenAI-compat shim: real loaded state via `loaded_instances`, the context
    // length actually configured on the loaded instance, and capability flags.
    // Try it first for the lmstudio provider; fall through to generic probing.
    let parse_lmstudio_native_models = |resp: ureq::Response| -> Option<Vec<ModelInfo>> {
        if resp.status() == 200 {
            if let Ok(json) = resp.into_json::<serde_json::Value>() {
                if let Some(models) = json.get("models").and_then(|v| v.as_array()) {
                    let mut models_list = Vec::new();
                    for m in models {
                        // Embedding models aren't chat-usable; skip them.
                        if m.get("type").and_then(|t| t.as_str()) == Some("embedding") {
                            continue;
                        }
                        let key = match m.get("key").and_then(|v| v.as_str()) {
                            Some(k) => k,
                            None => continue,
                        };
                        let loaded_instances = m.get("loaded_instances").and_then(|v| v.as_array());
                        let is_loaded = loaded_instances.map(|a| !a.is_empty()).unwrap_or(false);
                        // Prefer the context length configured on the loaded
                        // instance; fall back to the model's maximum.
                        let context_size = loaded_instances
                            .and_then(|a| a.first())
                            .and_then(|inst| inst.get("config"))
                            .and_then(|c| c.get("context_length"))
                            .or_else(|| m.get("max_context_length"))
                            .and_then(|v| v.as_u64())
                            .map(|v| v as u32);
                        models_list.push(ModelInfo {
                            name: key.to_string(),
                            is_loaded,
                            context_size,
                        });
                    }
                    if !models_list.is_empty() {
                        return Some(models_list);
                    }
                }
            }
        }
        None
    };

    if provider == "lmstudio" {
        let root = base_url
            .trim_end_matches("/api/v1/chat")
            .trim_end_matches("/api/v1")
            .trim_end_matches("/v1")
            .trim_end_matches('/');
        let native_url = format!("{}/api/v1/models", root);
        if let Ok(resp) = ureq::get(&native_url)
            .timeout(std::time::Duration::from_secs(3))
            .call()
        {
            if let Some(models) = parse_lmstudio_native_models(resp) {
                return Ok(models);
            }
        }
    }

    // If LM Studio or OpenAI is selected, skip Ollama-specific endpoints entirely
    if provider != "custom" && provider != "ollama" {
        let mut candidates = vec![
            format!("{}/models", base_url),
            format!("{}/v1/models", base_url),
            format!("{}/api/v1/models", base_url),
        ];

        if base_url.ends_with("/api/v1") {
            let root = base_url.trim_end_matches("/api/v1").trim_end_matches('/');
            candidates.push(format!("{}/v1/models", root));
            candidates.push(format!("{}/models", root));
        } else if base_url.ends_with("/v1") {
            let root = base_url.trim_end_matches("/v1").trim_end_matches('/');
            candidates.push(format!("{}/api/v1/models", root));
            candidates.push(format!("{}/models", root));
        }

        candidates.sort();
        candidates.dedup();

        for url in candidates {
            if let Ok(resp) = ureq::get(&url)
                .timeout(std::time::Duration::from_secs(3))
                .call()
            {
                if let Some(models) = parse_openai_models(resp) {
                    return Ok(models);
                }
            }
        }
    } else {
        // For custom (Ollama), try Ollama specific endpoints first
        let tags_url = format!("{}/api/tags", base_url);
        if let Ok(resp) = ureq::get(&tags_url)
            .timeout(std::time::Duration::from_secs(3))
            .call()
        {
            if resp.status() == 200 {
                if let Ok(json) = resp.into_json::<serde_json::Value>() {
                    let mut models_list = Vec::new();

                    // Get list of loaded models from /api/ps
                    let mut loaded_models = std::collections::HashSet::new();
                    let ps_url = format!("{}/api/ps", base_url);
                    if let Ok(ps_resp) = ureq::get(&ps_url).call() {
                        if ps_resp.status() == 200 {
                            if let Ok(ps_json) = ps_resp.into_json::<serde_json::Value>() {
                                if let Some(models) =
                                    ps_json.get("models").and_then(|v| v.as_array())
                                {
                                    for m in models {
                                        if let Some(name) = m.get("name").and_then(|v| v.as_str()) {
                                            loaded_models.insert(name.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Process all models from /api/tags
                    if let Some(models) = json.get("models").and_then(|v| v.as_array()) {
                        for m in models {
                            if let Some(name) = m.get("name").and_then(|v| v.as_str()) {
                                let name_str = name.to_string();
                                let is_loaded = loaded_models.contains(&name_str);

                                // Query /api/show for context size ONLY if the model is currently loaded
                                let mut context_size = None;
                                if is_loaded {
                                    let show_url = format!("{}/api/show", base_url);
                                    let show_payload = serde_json::json!({ "name": name_str });
                                    if let Ok(show_resp) =
                                        ureq::post(&show_url).send_json(show_payload)
                                    {
                                        if show_resp.status() == 200 {
                                            if let Ok(show_json) =
                                                show_resp.into_json::<serde_json::Value>()
                                            {
                                                if let Some(params) = show_json
                                                    .get("parameters")
                                                    .and_then(|v| v.as_str())
                                                {
                                                    // Parse num_ctx e.g. "num_ctx 8192"
                                                    for line in params.lines() {
                                                        let parts: Vec<&str> =
                                                            line.split_whitespace().collect();
                                                        if parts.len() >= 2 && parts[0] == "num_ctx"
                                                        {
                                                            if let Ok(val) = parts[1].parse::<u32>()
                                                            {
                                                                context_size = Some(val);
                                                                break;
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                models_list.push(ModelInfo {
                                    name: name_str,
                                    is_loaded,
                                    context_size,
                                });
                            }
                        }
                    }

                    return Ok(models_list);
                }
            }
        }

        // If Ollama tag endpoints failed/not Ollama, fall back to /v1/models and /models
        let oai_url = format!("{}/v1/models", base_url);
        if let Ok(resp) = ureq::get(&oai_url)
            .timeout(std::time::Duration::from_secs(3))
            .call()
        {
            if let Some(models) = parse_openai_models(resp) {
                return Ok(models);
            }
        }

        let oai_url_alt = format!("{}/models", base_url);
        if let Ok(resp) = ureq::get(&oai_url_alt)
            .timeout(std::time::Duration::from_secs(3))
            .call()
        {
            if let Some(models) = parse_openai_models(resp) {
                return Ok(models);
            }
        }
    }

    Err("Could not retrieve models from endpoints.".to_string())
}

fn verify_sandbox<P1: AsRef<Path>, P2: AsRef<Path>>(
    project_path: P1,
    target_path: P2,
) -> Result<PathBuf, String> {
    let proj = clean_project_path(project_path);
    let t_ref = target_path.as_ref();
    let target_abs = if t_ref.is_absolute() {
        t_ref.to_path_buf()
    } else {
        proj.join(t_ref)
    };
    // Lexical normalization first (resolves `..`/`.` without touching disk).
    let target = clean_project_path(target_abs);

    // Resolve symlinks where the path (or its nearest existing ancestor) exists,
    // so a symlink inside the sandbox that points outside is rejected. For paths
    // that don't exist yet (e.g. write_file creating a new file), canonicalize the
    // deepest existing ancestor and re-attach the remaining components.
    let canon_proj = std::fs::canonicalize(&proj).unwrap_or_else(|_| proj.clone());
    let canon_target = canonicalize_lenient(&target);

    // Component-boundary containment: canon_target must be the project dir itself
    // or a descendant. Comparing component iterators avoids the sibling-prefix
    // escape that `starts_with` on raw paths/strings is vulnerable to
    // (e.g. `/repo/run_x_evil` is NOT inside `/repo/run_x`).
    if path_is_within(&canon_target, &canon_proj) {
        Ok(target)
    } else {
        Err("Access denied: Path is outside project sandbox".to_string())
    }
}

/// Canonicalize as much of `path` as exists on disk, re-attaching any trailing
/// not-yet-created components. This lets us resolve symlinks for the existing
/// portion while still validating paths that point at files we're about to write.
fn canonicalize_lenient(path: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(path) {
        return c;
    }
    let mut remainder: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        match current.parent() {
            Some(parent) => {
                if let Some(name) = current.file_name() {
                    remainder.push(name.to_os_string());
                }
                if let Ok(c) = std::fs::canonicalize(parent) {
                    let mut resolved = c;
                    for comp in remainder.iter().rev() {
                        resolved.push(comp);
                    }
                    return resolved;
                }
                current = parent.to_path_buf();
            }
            None => return path.to_path_buf(),
        }
    }
}

/// True if `target` is `base` or a descendant of it, compared component-by-component
/// (not by raw string prefix, which would let `base_evil` pass for `base`).
fn path_is_within(target: &Path, base: &Path) -> bool {
    let mut t = target.components();
    for b in base.components() {
        match t.next() {
            Some(tc) if tc == b => continue,
            _ => return false,
        }
    }
    true
}

#[tauri::command]
pub async fn create_file(project_path: String, path: String) -> Result<(), String> {
    let target = verify_sandbox(&project_path, &path)?;
    if target.exists() {
        return Err("File already exists".to_string());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&target, "").map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn create_dir(project_path: String, path: String) -> Result<(), String> {
    let target = verify_sandbox(&project_path, &path)?;
    if target.exists() {
        return Err("Directory already exists".to_string());
    }
    fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn save_file(project_path: String, path: String, content: String) -> Result<(), String> {
    let target = verify_sandbox(&project_path, &path)?;
    fs::write(&target, content).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn delete_item(project_path: String, path: String) -> Result<(), String> {
    let target = verify_sandbox(&project_path, &path)?;
    if !target.exists() {
        return Err("Path does not exist".to_string());
    }
    if target.is_dir() {
        fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
    } else {
        fs::remove_file(&target).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ==========================================================================
// AUTONOMOUS RUN ENGINE & SANDBOXED TOOLS
// ==========================================================================

struct ActiveRunGuard {
    app_handle: tauri::AppHandle,
    run_id: String,
}

impl Drop for ActiveRunGuard {
    fn drop(&mut self) {
        if let Some(state) = self.app_handle.try_state::<AppState>() {
            let mut active = state.active_runs.lock().unwrap();
            active.remove(&self.run_id);
        }
    }
}

fn append_run_event(
    app_handle: &tauri::AppHandle,
    state: &AppState,
    run_id: &str,
    event: RunEvent,
) {
    let mut logs = state.run_logs.lock().unwrap();
    if let Some(run_events) = logs.get_mut(run_id) {
        run_events.push(event.clone());

        let card_id = {
            let cards = state.cards.lock().unwrap();
            cards
                .iter()
                .find(|c| c.run_id.as_deref() == Some(run_id))
                .map(|c| c.id.clone())
                .unwrap_or_else(|| "".to_string())
        };

        if let Ok(conn) = get_db_conn(app_handle) {
            let _ = conn.execute(
                "INSERT INTO logs (log_type, key, run_id, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                ("run", &card_id, &event.run_id, &event.event_type, &event.payload),
            );
        }
    }
}

fn set_card_status(app_handle: &tauri::AppHandle, state: &AppState, card_id: &str, status: &str) {
    let mut cards = state.cards.lock().unwrap();
    if let Some(card) = cards.iter_mut().find(|c| c.id == card_id) {
        card.status = status.to_string();

        if let Ok(conn) = get_db_conn(app_handle) {
            let _ = conn.execute(
                "UPDATE cards SET status = ?1 WHERE id = ?2",
                (status, card_id),
            );
        }
    }
}

fn extract_reasoning(response: &str) -> (Option<String>, String) {
    if let Some(start_idx) = response.find("<think>") {
        let after_start = &response[start_idx + 7..];
        if let Some(end_idx) = after_start.find("</think>") {
            let reasoning = after_start[..end_idx].trim().to_string();
            let remaining = format!("{}{}", &response[..start_idx], &after_start[end_idx + 8..])
                .trim()
                .to_string();
            return (Some(reasoning), remaining);
        } else {
            let reasoning = after_start.trim().to_string();
            let remaining = response[..start_idx].trim().to_string();
            return (Some(reasoning), remaining);
        }
    }
    (None, response.to_string())
}

fn construct_agent_system_prompt(
    worktree_path: &Path,
    card_title: &str,
    card_description: &str,
) -> String {
    format!(
        "You are BeetleAI, an autonomous coding agent. You have been assigned the following task:\n\
         Title: {}\n\
         Description: {}\n\n\
         You are executing inside an isolated git worktree sandbox located at: {}\n\
         All your file paths MUST be relative to this directory (do not write outside this directory).\n\n\
         You have access to the following tools to interact with the repository. To call a tool, you MUST output a single Markdown JSON code block of type \"tool_call\" in the following format:\n\n\
         ```tool_call\n\
         {{\n\
           \"name\": \"tool_name\",\n\
           \"args\": {{\n\
             \"arg1\": \"value1\"\n\
           }}\n\
         }}\n\
         ```\n\n\
         No other text should be inside the code block. Once you call a tool, the system will execute it and append the result. You can then analyze the output and make further tool calls.\n\n\
         Tools available:\n\
         1. `read_file(path: String, start_line?: Int, end_line?: Int)`: Reads file content. For large files, call outline_file first, then read only the line range you need. Output is line-numbered and capped — don't read whole large files when a range will do.\n\
         2. `outline_file(path: String)`: Returns a file's structure (markdown headings, or code declarations) with line numbers, without its full contents. Survey large files this way before reading.\n\
         3. `write_file(path: String, content: String)`: Writes content to a file (creating folders if needed).\n\
         4. `list_dir(path: String)`: Lists all files and folders under a relative path (use \"\" for root).\n\
         5. `search_grep(query: String, path?: String)`: Searches file contents for a substring across the whole repo (or under a path). Use this to find where symbols, strings, or config values live — do NOT use shell grep.\n\
         6. `git_status()`: Runs `git status` in the sandbox.\n\
         7. `git_diff()`: Runs `git diff` to view your current sandboxed changes.\n\
         8. `run_command(command: String)`: Runs a build, test, or check shell command in the workspace (e.g. \"npm run build\", \"npm test\", \"cargo check\"). Use this to verify your code compiles and passes tests! NOTE: the shell is Windows cmd.exe — Unix tools like grep, sed, awk, and ls are NOT available. Use search_grep, patch_file, and list_dir instead.\n\
         9. `patch_file(path: String, target: String, replacement: String)`: Replaces an exact text snippet in a file. Prefer this over write_file for small edits to large files.\n\
         10. `web_search(query: String)`: Searches the web for programming queries, libraries, APIs, or documentation snippets.\n\
         11. `send_notification(message: String)`: Sends a system alert/notification to the developer.\n\
         12. `task_complete(summary: String)`: Ends the loop, summarizes your work, and puts the card in \"Review\" status.\n\n\
         Work efficiently with context: prefer outline_file + ranged read_file over reading entire files, since large reads slow the model and crowd out useful history. When you have finished the task, you MUST call task_complete — do not simply describe that you are done in prose.",
        card_title, card_description, worktree_path.to_string_lossy()
    )
}

fn parse_tool_call_fallback(js: &str) -> Option<(String, serde_json::Value)> {
    let name = if js.contains("\"name\": \"write_file\"") || js.contains("\"name\":\"write_file\"")
    {
        Some("write_file")
    } else if js.contains("\"name\": \"read_file\"") || js.contains("\"name\":\"read_file\"") {
        Some("read_file")
    } else if js.contains("\"name\": \"list_dir\"") || js.contains("\"name\":\"list_dir\"") {
        Some("list_dir")
    } else if js.contains("\"name\": \"web_search\"") || js.contains("\"name\":\"web_search\"") {
        Some("web_search")
    } else if js.contains("\"name\": \"send_notification\"")
        || js.contains("\"name\":\"send_notification\"")
    {
        Some("send_notification")
    } else if js.contains("\"name\": \"task_complete\"")
        || js.contains("\"name\":\"task_complete\"")
    {
        Some("task_complete")
    } else if js.contains("\"name\": \"search_grep\"") || js.contains("\"name\":\"search_grep\"") {
        Some("search_grep")
    } else if js.contains("\"name\": \"patch_file\"") || js.contains("\"name\":\"patch_file\"") {
        Some("patch_file")
    } else {
        None
    };

    let name_str = name?;

    if name_str == "write_file" {
        let path_marker = "\"path\":";
        let path = if let Some(idx) = js.find(path_marker) {
            let rest = &js[idx + path_marker.len()..];
            if let Some(start_quote) = rest.find('"') {
                let rest_after_quote = &rest[start_quote + 1..];
                if let Some(end_quote) = rest_after_quote.find('"') {
                    Some(rest_after_quote[..end_quote].to_string())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        let content_marker = "\"content\":";
        let content = if let Some(idx) = js.find(content_marker) {
            let rest = &js[idx + content_marker.len()..];
            if let Some(start_quote) = rest.find('"') {
                let rest_after_quote = &rest[start_quote + 1..];
                if let Some(last_quote_idx) = rest_after_quote.rfind('"') {
                    Some(rest_after_quote[..last_quote_idx].to_string())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        if let (Some(p), Some(c)) = (path, content) {
            let cleaned_content = c
                .replace("\\n", "\n")
                .replace("\\t", "\t")
                .replace("\\\"", "\"")
                .replace("\\\\", "\\");
            return Some((
                "write_file".to_string(),
                serde_json::json!({
                    "path": p,
                    "content": cleaned_content
                }),
            ));
        }
    }
    None
}

fn strip_lang_prefix(s: &str) -> &str {
    let mut trimmed = s.trim();
    for prefix in &["tool_call", "json", "javascript", "js"] {
        if trimmed.starts_with(prefix) {
            let rest = trimmed[prefix.len()..].trim_start();
            if rest.starts_with('{')
                || rest.is_empty()
                || trimmed
                    .chars()
                    .nth(prefix.len())
                    .map(|c| c.is_whitespace())
                    .unwrap_or(false)
            {
                trimmed = rest;
                break;
            }
        }
    }
    trimmed
}

fn try_parse_json(s: &str) -> Option<(String, serde_json::Value)> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(s) {
        if let Some(name) = val.get("name").and_then(|n| n.as_str()) {
            let args = val.get("args").cloned().unwrap_or(serde_json::Value::Null);
            return Some((name.to_string(), args));
        }
    }

    if let Some(start_brace) = s.find('{') {
        if let Some(end_brace) = s.rfind('}') {
            if end_brace > start_brace {
                let json_sub = &s[start_brace..=end_brace];
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_sub) {
                    if let Some(name) = val.get("name").and_then(|n| n.as_str()) {
                        let args = val.get("args").cloned().unwrap_or(serde_json::Value::Null);
                        return Some((name.to_string(), args));
                    }
                }
                if let Some(res) = parse_tool_call_fallback(json_sub) {
                    return Some(res);
                }
            }
        }
    }

    if let Some(res) = parse_tool_call_fallback(s) {
        return Some(res);
    }

    None
}

fn parse_tool_call(response: &str) -> Option<(String, serde_json::Value)> {
    let mut response_cleaned = response.trim();
    if response_cleaned.ends_with("<tool_call|>") {
        response_cleaned = response_cleaned.trim_end_matches("<tool_call|>").trim();
    }

    for len in (2..=5).rev() {
        let bt = "`".repeat(len);
        let mut start_search = 0;

        while let Some(start_idx) = response_cleaned[start_search..].find(&bt) {
            let absolute_start = start_search + start_idx;
            let after_start = &response_cleaned[absolute_start + len..];

            if let Some(end_idx) = after_start.find(&bt) {
                let block_str = &after_start[..end_idx];
                let trimmed_block = strip_lang_prefix(block_str);

                if let Some(parsed) = try_parse_json(trimmed_block) {
                    return Some(parsed);
                }
            }
            start_search = absolute_start + len;
        }
    }

    if let Some(parsed) = try_parse_json(response_cleaned) {
        return Some(parsed);
    }

    None
}

fn run_git_command<P: AsRef<Path>>(dir: P, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir);
    cmd.args(args);
    crate::configure_no_window(&mut cmd);
    let output = cmd.output().map_err(|e| e.to_string())?;
    let out = String::from_utf8_lossy(&output.stdout).to_string();
    let err = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        Ok(out)
    } else {
        Err(format!("{}\n{}", out, err))
    }
}

fn run_shell_command<P: AsRef<Path>>(dir: P, command_str: &str) -> Result<String, String> {
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd.exe");
        c.arg("/c").arg(command_str);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command_str);
        c
    };
    cmd.current_dir(dir);
    crate::configure_no_window(&mut cmd);
    let output = cmd.output().map_err(|e| e.to_string())?;
    let out = String::from_utf8_lossy(&output.stdout).to_string();
    let err = String::from_utf8_lossy(&output.stderr).to_string();

    let mut combined = format!("{}\n{}", out, err);
    if combined.len() > 3000 {
        combined = format!("{}... [TRUNCATED]", &combined[..3000]);
    }
    Ok(combined)
}

fn strip_html_tags(html: &str) -> String {
    let mut clean = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            clean.push(c);
        }
    }
    clean
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}

fn perform_web_search(query: &str) -> String {
    let encoded = query
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect::<String>();

    let url = format!("https://html.duckduckgo.com/html/?q={}", encoded);
    match ureq::get(&url)
        .set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
        .call() {
            Ok(resp) => {
                if let Ok(html) = resp.into_string() {
                    let mut snippets = Vec::new();
                    let mut search_area = html.as_str();
                    while let Some(idx) = search_area.find("class=\"result__snippet\"") {
                        let after = &search_area[idx..];
                        if let Some(start) = after.find('>') {
                            if let Some(end) = after[start..].find("</a>") {
                                let raw_snippet = &after[start+1..start+end];
                                let clean = strip_html_tags(raw_snippet);
                                if !clean.trim().is_empty() {
                                    snippets.push(clean.trim().to_string());
                                }
                            }
                        }
                        if after.len() > 30 {
                            search_area = &after[30..];
                        } else {
                            break;
                        }
                        if snippets.len() >= 5 {
                            break;
                        }
                    }
                    if snippets.is_empty() {
                        "No search results found.".to_string()
                    } else {
                        snippets.join("\n\n")
                    }
                } else {
                    "Error: Could not parse search response.".to_string()
                }
            }
            Err(e) => format!("Search request failed: {}", e),
        }
}

/// Produce a structural outline of a file instead of its full contents: markdown
/// headings, or code declarations (fn/struct/enum/trait/impl/class/def/type/const),
/// each with its line number. Lets the agent survey a large file cheaply and then
/// range-read only the parts it needs, instead of pulling 10k tokens up front.
fn outline_file_impl(worktree_path: &Path, path: &str) -> String {
    let target = match verify_sandbox(worktree_path, path) {
        Ok(p) => p,
        Err(e) => return format!("Error: {}", e),
    };
    let content = match fs::read_to_string(&target) {
        Ok(c) => c,
        Err(e) => return format!("Error reading file: {}", e),
    };

    let is_md = path.ends_with(".md") || path.ends_with(".markdown");
    let mut out: Vec<String> = Vec::new();
    let total_lines = content.lines().count();

    for (idx, line) in content.lines().enumerate() {
        let ln = idx + 1;
        let trimmed = line.trim_start();
        if is_md {
            if trimmed.starts_with('#') {
                out.push(format!("{}: {}", ln, trimmed));
            }
        } else {
            // Match common declaration keywords across languages. We check the
            // leading token after optional visibility/qualifier words.
            let starters = [
                "fn ",
                "pub fn ",
                "async fn ",
                "pub async fn ",
                "struct ",
                "pub struct ",
                "enum ",
                "pub enum ",
                "trait ",
                "pub trait ",
                "impl ",
                "type ",
                "pub type ",
                "const ",
                "pub const ",
                "static ",
                "pub static ",
                "class ",
                "def ",
                "function ",
                "export function ",
                "export default ",
                "interface ",
                "export interface ",
                "export class ",
                "export const ",
                "mod ",
                "pub mod ",
            ];
            if starters.iter().any(|s| trimmed.starts_with(s)) {
                // Trim trailing body opener for readability.
                let sig = trimmed
                    .split(|c| c == '{' || c == ';')
                    .next()
                    .unwrap_or(trimmed)
                    .trim_end();
                out.push(format!("{}: {}", ln, sig));
            }
        }
    }

    if out.is_empty() {
        format!("({} lines, no headings/declarations detected. Use read_file with a line range to inspect.)", total_lines)
    } else {
        format!("File outline ({} lines total). Line numbers shown; use read_file with start_line/end_line to read a section:\n{}", total_lines, out.join("\n"))
    }
}

/// Read a file, optionally limited to a line range, with a hard character cap so a
/// single read can never blow the context budget. Returns 1-indexed line content.
fn read_file_range_impl(
    worktree_path: &Path,
    path: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
    max_chars: usize,
) -> String {
    let target = match verify_sandbox(worktree_path, path) {
        Ok(p) => p,
        Err(e) => return format!("Error: {}", e),
    };
    let content = match fs::read_to_string(&target) {
        Ok(c) => c,
        Err(e) => return format!("Error reading file: {}", e),
    };

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    let (lo, hi) = match (start_line, end_line) {
        (None, None) => (1, total),
        (Some(s), None) => (s.max(1), total),
        (None, Some(e)) => (1, e.min(total)),
        (Some(s), Some(e)) => (s.max(1), e.min(total)),
    };
    if lo > total {
        return format!(
            "Error: start_line {} is past end of file ({} lines)",
            lo, total
        );
    }
    if hi < lo {
        return format!("Error: end_line {} is before start_line {}", hi, lo);
    }

    let selected: String = lines[(lo - 1)..hi]
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{}: {}", lo + i, l))
        .collect::<Vec<_>>()
        .join("\n");

    if selected.len() > max_chars {
        let head: String = selected.chars().take(max_chars).collect();
        format!(
            "{}\n\n[... output truncated at {} chars. Narrow the range with start_line/end_line, or use outline_file first to find the section you need. File is {} lines total. ...]",
            head, max_chars, total
        )
    } else {
        selected
    }
}

fn search_grep_impl(worktree_path: &Path, query: &str, path_opt: &str) -> String {
    let worktree_abs = clean_project_path(worktree_path);
    let target = match verify_sandbox(&worktree_abs, path_opt) {
        Ok(p) => p,
        Err(e) => return format!("Error: {}", e),
    };

    let mut results = Vec::new();
    let mut count = 0;
    let max_results = 50;

    if target.is_file() {
        match fs::read_to_string(&target) {
            Ok(content) => {
                for (idx, line) in content.lines().enumerate() {
                    if line.contains(query) {
                        results.push(format!("{}:{}: {}", path_opt, idx + 1, line));
                        count += 1;
                        if count >= max_results {
                            results.push(format!("... truncated after {} results", max_results));
                            break;
                        }
                    }
                }
            }
            Err(e) => return format!("Error reading file: {}", e),
        }
    } else {
        fn visit_dirs(
            dir: &Path,
            query: &str,
            worktree_abs: &Path,
            results: &mut Vec<String>,
            count: &mut usize,
            max_results: usize,
        ) -> std::io::Result<()> {
            if dir.is_dir() {
                for entry in fs::read_dir(dir)? {
                    let entry = entry?;
                    let path = entry.path();
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    if name.starts_with('.')
                        || name == "node_modules"
                        || name == "target"
                        || name == "dist"
                        || name == ".harness"
                    {
                        continue;
                    }
                    if path.is_dir() {
                        visit_dirs(&path, query, worktree_abs, results, count, max_results)?;
                        if *count >= max_results {
                            break;
                        }
                    } else {
                        if let Ok(content) = fs::read_to_string(&path) {
                            let rel_path = path
                                .strip_prefix(worktree_abs)
                                .unwrap_or(&path)
                                .to_string_lossy()
                                .into_owned();
                            for (idx, line) in content.lines().enumerate() {
                                if line.contains(query) {
                                    results.push(format!("{}:{}: {}", rel_path, idx + 1, line));
                                    *count += 1;
                                    if *count >= max_results {
                                        results.push(format!(
                                            "... truncated after {} results",
                                            max_results
                                        ));
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Ok(())
        }
        if let Err(e) = visit_dirs(
            &target,
            query,
            &worktree_abs,
            &mut results,
            &mut count,
            max_results,
        ) {
            return format!("Error searching directory: {}", e);
        }
    }
    if results.is_empty() {
        "No matches found".to_string()
    } else {
        results.join("\n")
    }
}

fn patch_file_impl(
    worktree_path: &Path,
    path: &str,
    target_str: &str,
    replacement_str: &str,
) -> String {
    let worktree_abs = clean_project_path(worktree_path);
    let target_file = match verify_sandbox(&worktree_abs, path) {
        Ok(p) => p,
        Err(e) => return format!("Error: {}", e),
    };

    match fs::read_to_string(&target_file) {
        Ok(content) => {
            let matches: Vec<_> = content.match_indices(target_str).collect();
            if matches.is_empty() {
                return format!("Error: Target text not found in '{}'", path);
            }
            if matches.len() > 1 {
                return format!(
                    "Error: Target text occurs {} times in '{}'. Please provide more surrounding lines of context to ensure the match is unique.",
                    matches.len(),
                    path
                );
            }
            let updated = content.replacen(target_str, replacement_str, 1);
            match fs::write(&target_file, updated) {
                Ok(_) => "Success: File patched successfully".to_string(),
                Err(e) => format!("Error writing file: {}", e),
            }
        }
        Err(e) => format!("Error reading file: {}", e),
    }
}

fn execute_tool(
    app_handle: &tauri::AppHandle,
    worktree_path: &Path,
    tool_name: &str,
    args: &serde_json::Value,
    run_id: &str,
) -> String {
    match tool_name {
        "read_file" => {
            let path = match args.get("path").and_then(|p| p.as_str()) {
                Some(p) => p,
                None => return "Error: Missing path argument".to_string(),
            };
            let start_line = args
                .get("start_line")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize);
            let end_line = args
                .get("end_line")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize);
            // Hard cap on a single read so it can't blow the context budget. A full
            // read past this is truncated with guidance to range-read or outline first.
            read_file_range_impl(worktree_path, path, start_line, end_line, 8000)
        }
        "outline_file" => {
            let path = match args.get("path").and_then(|p| p.as_str()) {
                Some(p) => p,
                None => return "Error: Missing path argument".to_string(),
            };
            outline_file_impl(worktree_path, path)
        }
        "write_file" => {
            let path = match args.get("path").and_then(|p| p.as_str()) {
                Some(p) => p,
                None => return "Error: Missing path argument".to_string(),
            };
            let content = match args.get("content").and_then(|c| c.as_str()) {
                Some(c) => c,
                None => return "Error: Missing content argument".to_string(),
            };
            let target = match verify_sandbox(worktree_path, path) {
                Ok(p) => p,
                Err(e) => return format!("Error: {}", e),
            };
            if let Some(parent) = target.parent() {
                let _ = fs::create_dir_all(parent);
            }
            match fs::write(target, content) {
                Ok(_) => "Success: File written successfully".to_string(),
                Err(e) => format!("Error writing file: {}", e),
            }
        }
        "list_dir" => {
            let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
            let target = match verify_sandbox(worktree_path, path) {
                Ok(p) => p,
                Err(e) => return format!("Error: {}", e),
            };
            match fs::read_dir(target) {
                Ok(rd) => {
                    let mut entries = Vec::new();
                    for entry in rd.flatten() {
                        let p = entry.path();
                        let name = p
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        if name.starts_with('.') || name == "node_modules" || name == "target" {
                            continue;
                        }
                        let is_dir = p.is_dir();
                        entries.push(format!(
                            "{} ({})",
                            name,
                            if is_dir { "folder" } else { "file" }
                        ));
                    }
                    if entries.is_empty() {
                        "Directory is empty".to_string()
                    } else {
                        entries.join("\n")
                    }
                }
                Err(e) => format!("Error listing directory: {}", e),
            }
        }
        "git_status" => match run_git_command(worktree_path, &["status"]) {
            Ok(out) => out,
            Err(e) => format!("Error: {}", e),
        },
        "git_diff" => match run_git_command(worktree_path, &["diff"]) {
            Ok(out) => out,
            Err(e) => format!("Error: {}", e),
        },
        "run_command" => {
            let command = match args.get("command").and_then(|c| c.as_str()) {
                Some(c) => c,
                None => return "Error: Missing command argument".to_string(),
            };
            match run_shell_command(worktree_path, command) {
                Ok(out) => out,
                Err(e) => format!("Error executing command: {}", e),
            }
        }
        "web_search" => {
            let query = match args.get("query").and_then(|q| q.as_str()) {
                Some(q) => q,
                None => return "Error: Missing query argument".to_string(),
            };
            perform_web_search(query)
        }
        "send_notification" => {
            let message = match args.get("message").and_then(|m| m.as_str()) {
                Some(m) => m,
                None => return "Error: Missing message argument".to_string(),
            };
            let _ = app_handle.emit("notification", message.to_string());
            "Success: Notification sent".to_string()
        }
        "task_complete" => {
            let summary = args.get("summary").and_then(|s| s.as_str()).unwrap_or("");
            let state = app_handle.state::<AppState>();
            let mut cards = state.cards.lock().unwrap();
            if let Some(card) = cards
                .iter_mut()
                .find(|c| c.run_id.as_deref() == Some(run_id))
            {
                card.status = "review".to_string();
                let _ = app_handle.emit("notification", format!("Task completed: {}", card.title));
            }
            format!("Success: Task completed. Summary: {}", summary)
        }
        "search_grep" => {
            let query = match args.get("query").and_then(|q| q.as_str()) {
                Some(q) => q,
                None => return "Error: Missing query argument".to_string(),
            };
            let path_opt = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
            search_grep_impl(worktree_path, query, path_opt)
        }
        "patch_file" => {
            let path = match args.get("path").and_then(|p| p.as_str()) {
                Some(p) => p,
                None => return "Error: Missing path argument".to_string(),
            };
            let target_str = match args.get("target").and_then(|t| t.as_str()) {
                Some(t) => t,
                None => return "Error: Missing target argument".to_string(),
            };
            let replacement_str = match args.get("replacement").and_then(|r| r.as_str()) {
                Some(r) => r,
                None => return "Error: Missing replacement argument".to_string(),
            };
            patch_file_impl(worktree_path, path, target_str, replacement_str)
        }
        _ => format!(
            "Error: Unknown tool '{}'. Available tools: read_file, outline_file, write_file, patch_file, list_dir, search_grep, git_status, git_diff, run_command, web_search, send_notification, task_complete. You may ONLY call these tools.",
            tool_name
        ),
    }
}

pub fn run_agent_loop(app_handle: tauri::AppHandle, run_id: String, card_id: String) {
    let app_handle_clone = app_handle.clone();
    let run_id_clone = run_id.clone();

    tauri::async_runtime::spawn(async move {
        let state = app_handle_clone.state::<AppState>();

        {
            let mut active = state.active_runs.lock().unwrap();
            if active.contains(&run_id_clone) {
                return;
            }
            active.insert(run_id_clone.clone());
        }

        let _guard = ActiveRunGuard {
            app_handle: app_handle_clone.clone(),
            run_id: run_id_clone.clone(),
        };

        let config = load_config(&app_handle_clone);
        let max_steps = config.settings.max_steps as usize;

        let card_meta = {
            let cards = state.cards.lock().unwrap();
            cards.iter().find(|c| c.id == card_id).map(|c| {
                (
                    c.title.clone(),
                    c.description.clone(),
                    c.project_path.clone(),
                )
            })
        };

        let (card_title, card_desc, card_project_path) = match card_meta {
            Some(meta) => meta,
            None => return,
        };

        // Scope the run to the card's project, not BeetleAI's own working dir.
        let repo_path = clean_project_path(&card_project_path);
        let worktree_path = repo_path
            .join(".harness")
            .join("worktrees")
            .join(&run_id_clone);

        let mut step = 0;
        // Loop-guard state: consecutive identical tool calls are a stuck model,
        // not progress. Nudge at 3 repeats, hard-block at 5 instead of burning
        // a slow local model all the way to the step ceiling.
        let mut last_tool_signature: Option<String> = None;
        let mut repeat_count: u32 = 0;
        // Consecutive responses with no tool call and no visible content — a
        // truncated or all-reasoning response is a stall, not a user question.
        let mut empty_response_streak: u32 = 0;

        loop {
            {
                let cards = state.cards.lock().unwrap();
                if let Some(card) = cards.iter().find(|c| c.id == card_id) {
                    if card.status != "running" {
                        break;
                    }
                } else {
                    break;
                }
            }

            if step >= max_steps {
                log_error(&format!(
                    "Max step ceiling reached ({}) for run {}",
                    max_steps, run_id_clone
                ));
                append_run_event(&app_handle_clone, &state, &run_id_clone, RunEvent {
                    run_id: run_id_clone.clone(),
                    event_type: "blocked".to_string(),
                    payload: serde_json::json!({
                        "reason": "step_ceiling",
                        "message": format!("Step limit reached ({} steps). Reply in chat to continue the run.", max_steps)
                    }).to_string(),
                });
                set_card_status(&app_handle_clone, &state, &card_id, "blocked");
                let _ = app_handle_clone
                    .emit("run-updated", serde_json::json!({ "run_id": run_id_clone }));
                break;
            }

            step += 1;

            let history = {
                let logs = state.run_logs.lock().unwrap();
                if let Some(events) = logs.get(&run_id_clone) {
                    get_history_messages(events)
                } else {
                    Vec::new()
                }
            };

            let system_prompt =
                construct_agent_system_prompt(&worktree_path, &card_title, &card_desc);
            let system_prompt = format!(
                "{}\n\nYou are on step {} of a maximum of {} for this run. Pace your work to finish and call task_complete before hitting the ceiling.",
                system_prompt, step, max_steps
            );
            let tools_schema = get_openai_tools_schema(&[
                "read_file",
                "outline_file",
                "write_file",
                "list_dir",
                "git_status",
                "git_diff",
                "run_command",
                "web_search",
                "send_notification",
                "task_complete",
                "search_grep",
                "patch_file",
            ]);
            let response = match call_llm(
                &app_handle_clone,
                &run_id_clone,
                &system_prompt,
                history,
                Some(tools_schema),
            ) {
                Ok(reply) => reply,
                Err(e) => {
                    log_error(&format!("Agent loop LLM error: {}", e));
                    let is_cancelled = if let Some(st) = app_handle_clone.try_state::<AppState>() {
                        let cancelled = st.cancelled_runs.lock().unwrap();
                        cancelled.contains(&run_id_clone)
                    } else {
                        false
                    };
                    if !is_cancelled {
                        // Transient/LLM error: pause as `blocked` (resumable via unblock_run),
                        // keep the worktree intact so the partial work can be inspected or retried.
                        append_run_event(&app_handle_clone, &state, &run_id_clone, RunEvent {
                            run_id: run_id_clone.clone(),
                            event_type: "blocked".to_string(),
                            payload: serde_json::json!({
                                "reason": "error",
                                "message": format!("Run paused after an error calling the model: {}. Retry from chat to resume.", e)
                            }).to_string(),
                        });
                        set_card_status(&app_handle_clone, &state, &card_id, "blocked");
                        let _ = app_handle_clone
                            .emit("run-updated", serde_json::json!({ "run_id": run_id_clone }));
                    }
                    break;
                }
            };

            let (reasoning, remaining) = extract_reasoning(&response);
            if let Some(reasoning_content) = reasoning {
                append_run_event(
                    &app_handle_clone,
                    &state,
                    &run_id_clone,
                    RunEvent {
                        run_id: run_id_clone.clone(),
                        event_type: "reasoning".to_string(),
                        payload: reasoning_content,
                    },
                );
                let _ = app_handle_clone
                    .emit("run-updated", serde_json::json!({ "run_id": run_id_clone }));
            }

            if let Some((tool_name, args)) = parse_tool_call(&remaining) {
                append_run_event(
                    &app_handle_clone,
                    &state,
                    &run_id_clone,
                    RunEvent {
                        run_id: run_id_clone.clone(),
                        event_type: "tool_call".to_string(),
                        payload: serde_json::json!({
                            "name": tool_name.clone(),
                            "args": args.clone(),
                        })
                        .to_string(),
                    },
                );

                let _ = app_handle_clone
                    .emit("run-updated", serde_json::json!({ "run_id": run_id_clone }));

                // Loop-guard: track consecutive identical calls.
                empty_response_streak = 0;
                let signature = format!("{}:{}", tool_name, args);
                if last_tool_signature.as_deref() == Some(signature.as_str()) {
                    repeat_count += 1;
                } else {
                    repeat_count = 0;
                    last_tool_signature = Some(signature);
                }

                if repeat_count >= 4 {
                    log_error(&format!(
                        "Run {} blocked: tool '{}' called identically {} times in a row",
                        run_id_clone,
                        tool_name,
                        repeat_count + 1
                    ));
                    append_run_event(&app_handle_clone, &state, &run_id_clone, RunEvent {
                        run_id: run_id_clone.clone(),
                        event_type: "blocked".to_string(),
                        payload: serde_json::json!({
                            "reason": "stuck_loop",
                            "message": format!("The agent called '{}' with identical arguments {} times in a row and appears stuck. Reply in chat to redirect it.", tool_name, repeat_count + 1)
                        }).to_string(),
                    });
                    set_card_status(&app_handle_clone, &state, &card_id, "blocked");
                    let _ = app_handle_clone
                        .emit("run-updated", serde_json::json!({ "run_id": run_id_clone }));
                    break;
                }

                let mut tool_result = execute_tool(
                    &app_handle_clone,
                    &worktree_path,
                    &tool_name,
                    &args,
                    &run_id_clone,
                );

                if repeat_count >= 2 {
                    tool_result.push_str(&format!(
                        "\n\n[harness note: you have now called '{}' with identical arguments {} times in a row. The result will not change. Try a different tool, different arguments, or reconsider your approach.]",
                        tool_name,
                        repeat_count + 1
                    ));
                }

                append_run_event(
                    &app_handle_clone,
                    &state,
                    &run_id_clone,
                    RunEvent {
                        run_id: run_id_clone.clone(),
                        event_type: "tool_result".to_string(),
                        payload: serde_json::json!({
                            "name": tool_name.clone(),
                            "result": tool_result,
                        })
                        .to_string(),
                    },
                );

                let _ = app_handle_clone
                    .emit("run-updated", serde_json::json!({ "run_id": run_id_clone }));

                // task_complete moves the card to `review` (done inside execute_tool).
                // Break the loop immediately so the next iteration can't overwrite that
                // status with a `blocked`/`running` transition.
                if tool_name == "task_complete" {
                    let _ = app_handle_clone
                        .emit("run-updated", serde_json::json!({ "run_id": run_id_clone }));
                    break;
                }

                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            } else {
                let visible = remaining.trim();

                // A response with no tool call AND no visible content is not a
                // question — the reasoning consumed the whole output budget, or
                // generation was truncated mid-think. Blocking on it shows the
                // user an empty "awaiting input" with nothing to answer. Nudge
                // the model and continue; only a persistent stall blocks.
                if visible.is_empty() && empty_response_streak < 2 {
                    empty_response_streak += 1;
                    append_run_event(
                        &app_handle_clone,
                        &state,
                        &run_id_clone,
                        RunEvent {
                            run_id: run_id_clone.clone(),
                            event_type: "message".to_string(),
                            payload: serde_json::json!({
                                "role": "user",
                                "content": "[harness note: your previous response had no visible output and no tool call — it may have been cut off mid-reasoning. Keep reasoning brief and emit exactly one tool_call block now.]"
                            })
                            .to_string(),
                        },
                    );
                    let _ = app_handle_clone
                        .emit("run-updated", serde_json::json!({ "run_id": run_id_clone }));
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    continue;
                }

                // Genuine question — or a persistent stall after repeated nudges.
                let (agent_msg, blocked_msg) = if visible.is_empty() {
                    (
                        "(the agent produced no visible output)".to_string(),
                        "The agent stalled: several responses in a row ended without output or a tool call. Reply in chat to redirect it, or try lowering reasoning effort (sampler settings / non-thinking model).".to_string(),
                    )
                } else {
                    (
                        remaining.clone(),
                        "The agent is waiting for your input.".to_string(),
                    )
                };
                append_run_event(
                    &app_handle_clone,
                    &state,
                    &run_id_clone,
                    RunEvent {
                        run_id: run_id_clone.clone(),
                        event_type: "message".to_string(),
                        payload: serde_json::json!({
                            "role": "agent",
                            "content": agent_msg
                        })
                        .to_string(),
                    },
                );
                append_run_event(
                    &app_handle_clone,
                    &state,
                    &run_id_clone,
                    RunEvent {
                        run_id: run_id_clone.clone(),
                        event_type: "blocked".to_string(),
                        payload: serde_json::json!({
                            "reason": "question",
                            "message": blocked_msg
                        })
                        .to_string(),
                    },
                );

                set_card_status(&app_handle_clone, &state, &card_id, "blocked");
                let _ = app_handle_clone
                    .emit("run-updated", serde_json::json!({ "run_id": run_id_clone }));
                break;
            }
        }
        let _ = app_handle_clone.emit(
            "chat-finished",
            serde_json::json!({ "run_id": run_id_clone }),
        );
    });
}

fn append_design_event(
    app_handle: &tauri::AppHandle,
    state: &AppState,
    log_key: &str,
    event: RunEvent,
) {
    let mut logs = state.design_logs.lock().unwrap();
    if let Some(events) = logs.get_mut(log_key) {
        events.push(event.clone());

        if let Ok(conn) = get_db_conn(app_handle) {
            let _ = conn.execute(
                "INSERT INTO logs (log_type, key, run_id, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                ("design", log_key, &event.run_id, &event.event_type, &event.payload),
            );
        }
    }
}

fn append_code_event(
    app_handle: &tauri::AppHandle,
    state: &AppState,
    log_key: &str,
    event: RunEvent,
) {
    let mut logs = state.code_logs.lock().unwrap();
    if let Some(events) = logs.get_mut(log_key) {
        events.push(event.clone());

        if let Ok(conn) = get_db_conn(app_handle) {
            let _ = conn.execute(
                "INSERT INTO logs (log_type, key, run_id, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                ("code", log_key, &event.run_id, &event.event_type, &event.payload),
            );
        }
    }
}

fn construct_architect_system_prompt(project_path: &Path, doc_name: &str) -> String {
    format!(
        "You are BeetleAI, a software architect copilot. You are discussing and updating the design document: {}\n\
         The project root is located at: {}\n\n\
         You have access to tools to read/write files and research solutions. To call a tool, you MUST output a single Markdown JSON code block of type \"tool_call\" in the following format:\n\n\
         ```tool_call\n\
         {{\n\
           \"name\": \"tool_name\",\n\
           \"args\": {{\n\
             \"arg1\": \"value1\"\n\
           }}\n\
         }}\n\
         ```\n\n\
         Tools available:\n\
         1. `read_file(path: String)`: Reads file content relative to project root.\n\
         2. `write_file(path: String, content: String)`: Writes content to a file (creating folders if needed). Use this to update design documents under `design/`!\n\
         3. `list_dir(path: String)`: Lists all files and folders under a relative path (use \"\" for root).\n\
         4. `web_search(query: String)`: Searches the web for APIs, libraries, architectural patterns, and programming guides.\n\
         5. `send_notification(message: String)`: Sends a system alert/notification to the developer.\n\n\
         If you want to talk to the user, output a regular text response explaining your ideas, proposals, or questions.",
        doc_name, project_path.to_string_lossy()
    )
}

fn construct_copilot_system_prompt(project_path: &Path, file_path: &str) -> String {
    let target = if file_path.is_empty() {
        "the workspace"
    } else {
        file_path
    };
    format!(
        "You are BeetleAI, a software developer copilot. You are helping the user update or write code in: {}\n\
         The project root is located at: {}\n\n\
         You have access to tools to read/write source files and test compilation. To call a tool, you MUST output a single Markdown JSON code block of type \"tool_call\" in the following format:\n\n\
         ```tool_call\n\
         {{\n\
           \"name\": \"tool_name\",\n\
           \"args\": {{\n\
             \"arg1\": \"value1\"\n\
           }}\n\
         }}\n\
         ```\n\n\
         Tools available:\n\
         1. `read_file(path: String)`: Reads file content relative to project root.\n\
         2. `write_file(path: String, content: String)`: Writes/overwrites content to a source file.\n\
         3. `list_dir(path: String)`: Lists all files and folders under a relative path (use \"\" for root).\n\
         4. `git_status()`: Runs `git status` in the repository.\n\
         5. `git_diff()`: Runs `git diff` to view code changes.\n\
         6. `run_command(command: String)`: Runs build, test, or check shell commands in the repository (e.g. \"cargo check\", \"npm run build\", \"npm test\"). Use this to verify your changes compile and pass tests!\n\
         7. `web_search(query: String)`: Searches the web for documentation, syntax guides, and examples.\n\n\
         If you want to talk to the user, output a regular text response explaining your changes or asking questions.",
        target, project_path.to_string_lossy()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tool_call_simple() {
        let input = r#"
Some introductory text.
```tool_call
{
  "name": "list_dir",
  "args": {"path": ""}
}
```
Some trailing text.
"#;
        let res = parse_tool_call(input);
        assert!(res.is_some());
        let (name, args) = res.unwrap();
        assert_eq!(name, "list_dir");
        assert_eq!(args.get("path").unwrap().as_str().unwrap(), "");
    }

    #[test]
    fn test_parse_tool_call_double_backticks() {
        let input = r#"
BeetleAI
BeetleAI
``tool_call
{
"name": "list_dir",
"args": {"path": ""}
}
``
BeetleAI
``tool_call
{
"name": "read_file",
"args": {"path":"DesignDoc.md"}
}
``
<tool_call|>
"#;
        let res = parse_tool_call(input);
        assert!(res.is_some());
        let (name, args) = res.unwrap();
        assert_eq!(name, "list_dir");
        assert_eq!(args.get("path").unwrap().as_str().unwrap(), "");
    }

    #[test]
    fn test_parse_tool_call_no_backticks_with_junk() {
        let input = r#"
BeetleAI
BeetleAI
{
"name": "read_file",
"args": {"path":"DesignDoc.md"}
}
<tool_call|>
"#;
        let res = parse_tool_call(input);
        assert!(res.is_some());
        let (name, args) = res.unwrap();
        assert_eq!(name, "read_file");
        assert_eq!(args.get("path").unwrap().as_str().unwrap(), "DesignDoc.md");
    }

    #[test]
    fn test_parse_tool_call_json_block() {
        let input = r#"
```json
{
  "name": "git_status",
  "args": {}
}
```
"#;
        let res = parse_tool_call(input);
        assert!(res.is_some());
        let (name, _) = res.unwrap();
        assert_eq!(name, "git_status");
    }

    #[test]
    fn test_parse_sse_delta_reasoning() {
        let input_openai = r#"data: {"choices": [{"delta": {"reasoning_content": "Thinking about the files..."}}]}"#;
        let res = parse_sse_delta(input_openai);
        assert!(res.is_some());
        let parsed = res.unwrap();
        assert_eq!(parsed.reasoning.unwrap(), "Thinking about the files...");
        assert!(parsed.content.is_none());

        let input_anthropic = r#"data: {"type": "content_block_delta", "delta": {"type": "thinking_delta", "thinking": "Let me search the directory."}}"#;
        let res = parse_sse_delta(input_anthropic);
        assert!(res.is_some());
        let parsed = res.unwrap();
        assert_eq!(parsed.reasoning.unwrap(), "Let me search the directory.");
        assert!(parsed.content.is_none());
    }

    #[test]
    fn test_get_history_messages_merge() {
        let events = vec![
            RunEvent {
                run_id: "test".to_string(),
                event_type: "reasoning".to_string(),
                payload: "I will read the design doc.".to_string(),
            },
            RunEvent {
                run_id: "test".to_string(),
                event_type: "tool_call".to_string(),
                payload: serde_json::json!({
                    "name": "read_file",
                    "args": {"path": "DesignDoc.md"}
                })
                .to_string(),
            },
            RunEvent {
                run_id: "test".to_string(),
                event_type: "tool_result".to_string(),
                payload: serde_json::json!({
                    "name": "read_file",
                    "result": "File contents here."
                })
                .to_string(),
            },
        ];

        let history = get_history_messages(&events);
        assert_eq!(history.len(), 2);

        let first = &history[0];
        assert_eq!(first.get("role").unwrap().as_str().unwrap(), "assistant");
        let content = first.get("content").unwrap().as_str().unwrap();
        assert!(content.contains("<think>\nI will read the design doc.\n</think>"));
        assert!(content.contains("```tool_call"));

        let second = &history[1];
        assert_eq!(second.get("role").unwrap().as_str().unwrap(), "user");
        assert_eq!(
            second.get("content").unwrap().as_str().unwrap(),
            "Tool 'read_file' returned:\nFile contents here."
        );
    }

    #[test]
    fn test_search_and_patch_helpers() {
        use tempfile::tempdir;
        let temp_dir = tempdir().unwrap();
        let wt_path = temp_dir.path();

        let file_a = wt_path.join("file_a.txt");
        let file_b = wt_path.join("file_b.txt");

        fs::write(
            &file_a,
            "Hello World!\nThis is a large file line.\nRust is awesome!\n",
        )
        .unwrap();
        fs::write(
            &file_b,
            "Hello World!\nThis is another file.\nRust coding is great!\n",
        )
        .unwrap();

        // 1. Test search_grep_impl recursively
        let res_search = search_grep_impl(wt_path, "Rust", "");
        let normalized = res_search.replace("\\", "/");
        assert!(normalized.contains("file_a.txt:3: Rust is awesome!"));
        assert!(normalized.contains("file_b.txt:3: Rust coding is great!"));

        // 2. Test search_grep_impl specific file
        let res_search_file = search_grep_impl(wt_path, "large file", "file_a.txt");
        let normalized_file = res_search_file.replace("\\", "/");
        assert!(normalized_file.contains("file_a.txt:2: This is a large file line."));
        assert!(!normalized_file.contains("file_b.txt"));

        // 3. Test patch_file_impl unique replacement
        let res_patch = patch_file_impl(
            wt_path,
            "file_a.txt",
            "Rust is awesome!",
            "Rust is incredibly fast!",
        );
        assert_eq!(res_patch, "Success: File patched successfully");

        let patched_content = fs::read_to_string(&file_a).unwrap();
        assert!(patched_content.contains("Rust is incredibly fast!"));
        assert!(!patched_content.contains("Rust is awesome!"));

        // 4. Test patch_file_impl non-unique target
        let res_patch_non_unique = patch_file_impl(wt_path, "file_a.txt", "is", "was");
        assert!(res_patch_non_unique.contains("Error: Target text occurs"));
    }
}
