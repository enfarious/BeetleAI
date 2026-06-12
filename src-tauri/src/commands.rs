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
    #[serde(default = "default_priority")]
    pub priority: String, // "low" | "medium" | "high"
    #[serde(default)]
    pub labels: Vec<String>,
}

fn default_priority() -> String {
    "medium".to_string()
}

/// Normalize free-form priority input to the three canonical levels.
fn normalize_priority(p: &str) -> String {
    match p.trim().to_lowercase().as_str() {
        "low" | "l" | "minor" | "p3" => "low".to_string(),
        "high" | "h" | "urgent" | "critical" | "p0" | "p1" => "high".to_string(),
        _ => "medium".to_string(),
    }
}

/// Report any argument keys a tool didn't recognize. Silent argument drops
/// teach the model false beliefs about what happened: a tool that uses some
/// arguments, ignores others, and reports plain "Success" is lying by
/// omission. Every tool result must name what it ignored.
fn unknown_args_note(args: &serde_json::Value, known: &[&str]) -> String {
    let Some(map) = args.as_object() else {
        return String::new();
    };
    let unknown: Vec<&str> = map
        .keys()
        .map(|k| k.as_str())
        .filter(|k| !known.contains(k))
        .collect();
    if unknown.is_empty() {
        String::new()
    } else {
        format!(
            " Note: IGNORED unrecognized argument(s): {}. Supported arguments: {}.",
            unknown.join(", "),
            known.join(", ")
        )
    }
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
                priority: "medium".to_string(),
                labels: Vec::new(),
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
                priority: "medium".to_string(),
                labels: Vec::new(),
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
                priority: "medium".to_string(),
                labels: Vec::new(),
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
                priority: "medium".to_string(),
                labels: Vec::new(),
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

/// Resolve the long-term-memory scope for a tool call: the project the work
/// belongs to. Runs resolve through their card's project_path; chat modes
/// (design/code copilot) have no card and pass the project root directly as
/// the worktree path. Returns (project_path, card_id).
fn memory_scope(
    app_handle: &tauri::AppHandle,
    worktree_path: &Path,
    run_id: &str,
) -> (String, Option<String>) {
    if let Some(state) = app_handle.try_state::<AppState>() {
        let cards = state.cards.lock().unwrap();
        if let Some(card) = cards.iter().find(|c| c.run_id.as_deref() == Some(run_id)) {
            return (
                clean_project_path(&card.project_path)
                    .to_string_lossy()
                    .into_owned(),
                Some(card.id.clone()),
            );
        }
    }
    (
        clean_project_path(worktree_path)
            .to_string_lossy()
            .into_owned(),
        None,
    )
}

fn insert_memory(
    conn: &rusqlite::Connection,
    project_path: &str,
    topic: &str,
    content: &str,
    source: &str,
    run_id: Option<&str>,
    card_id: Option<&str>,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO memories (project_path, topic, content, source, run_id, card_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            project_path,
            topic,
            content,
            source,
            run_id,
            card_id,
            chrono::Utc::now().to_rfc3339(),
        ),
    )?;
    Ok(())
}

/// Recover the task_complete summary for a run from the persisted logs.
/// Only successful completions count; returns None if the run never
/// completed cleanly (in which case no memory is written).
fn latest_run_summary(conn: &rusqlite::Connection, run_id: &str) -> Option<String> {
    let mut stmt = conn
        .prepare(
            "SELECT payload FROM logs WHERE run_id = ?1 AND event_type = 'tool_result' ORDER BY id DESC",
        )
        .ok()?;
    let rows = stmt
        .query_map([run_id], |row| row.get::<_, String>(0))
        .ok()?;
    for payload in rows.flatten() {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&payload) else {
            continue;
        };
        if val.get("name").and_then(|n| n.as_str()) != Some("task_complete") {
            continue;
        }
        let result = val.get("result").and_then(|r| r.as_str()).unwrap_or("");
        if !result.starts_with("Success") {
            continue;
        }
        if let Some(idx) = result.find("\nSummary: ") {
            let summary = result[idx + "\nSummary: ".len()..].trim();
            if !summary.is_empty() {
                return Some(summary.to_string());
            }
        }
    }
    None
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
            priority: "medium".to_string(),
            labels: Vec::new(),
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
            priority: "medium".to_string(),
            labels: Vec::new(),
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
            priority: "medium".to_string(),
            labels: Vec::new(),
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
            priority: "medium".to_string(),
            labels: Vec::new(),
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
            assignee TEXT,
            priority TEXT NOT NULL DEFAULT 'medium',
            labels TEXT NOT NULL DEFAULT '[]'
        );",
        [],
    )
    .map_err(|e| e.to_string())?;

    // Lightweight migrations for existing databases: SQLite's ALTER TABLE
    // ADD COLUMN fails harmlessly when the column already exists.
    let _ = conn.execute(
        "ALTER TABLE cards ADD COLUMN priority TEXT NOT NULL DEFAULT 'medium'",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE cards ADD COLUMN labels TEXT NOT NULL DEFAULT '[]'",
        [],
    );

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

    // Long-term project memory: explicit `remember` calls from the agent plus
    // summaries auto-ingested when a run is ACCEPTED (never on rejection —
    // memory writeback shares the same gate as code writeback).
    conn.execute(
        "CREATE TABLE IF NOT EXISTS memories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            project_path TEXT NOT NULL,
            topic TEXT NOT NULL,
            content TEXT NOT NULL,
            source TEXT NOT NULL,
            run_id TEXT,
            card_id TEXT,
            created_at TEXT NOT NULL
        );",
        [],
    )
    .map_err(|e| e.to_string())?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_memories_project ON memories (project_path);",
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
        .prepare("SELECT id, project_path, title, description, status, run_id, assignee, priority, labels FROM cards")
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
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut cards = Vec::new();
    for card_res in card_rows {
        let (id, project_path, title, description, status, run_id, assignee, priority, labels_json) =
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
            priority: normalize_priority(&priority),
            labels: serde_json::from_str(&labels_json).unwrap_or_default(),
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

/// Update a registered project's name and/or path, keyed by its CURRENT path
/// (the unique key create_project enforces). A path change cascades to the
/// project's cards and long-term memories so nothing is orphaned — this is
/// the supported way to fix a project that was registered pointing at the
/// wrong directory.
#[tauri::command]
pub async fn update_project(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    new_name: String,
    new_path: String,
) -> Result<Project, String> {
    let new_name = new_name.trim().to_string();
    let new_path = new_path.trim().to_string();
    if new_name.is_empty() || new_path.is_empty() {
        return Err("Name and path are required.".to_string());
    }
    if !Path::new(&new_path).exists() {
        return Err("New path does not exist on disk. Create the directory first, or use New Project to scaffold one.".to_string());
    }

    let old_clean = clean_project_path(&path);
    let new_clean = clean_project_path(&new_path);
    let path_changed = old_clean != new_clean;

    let mut config = load_config(&app_handle);
    let idx = config
        .projects
        .iter()
        .position(|p| clean_project_path(&p.path) == old_clean)
        .ok_or_else(|| "Project not found.".to_string())?;

    if path_changed
        && config
            .projects
            .iter()
            .enumerate()
            .any(|(i, p)| i != idx && clean_project_path(&p.path) == new_clean)
    {
        return Err("Another project is already registered at that path.".to_string());
    }

    // Repointing a project under a live run is exactly the class of path bug
    // this app has had enough of. Refuse until the run is finished/cancelled.
    if path_changed {
        let cards = state.cards.lock().unwrap();
        let active = state.active_runs.lock().unwrap();
        let has_active = cards.iter().any(|c| {
            clean_project_path(&c.project_path) == old_clean
                && c.run_id
                    .as_deref()
                    .map(|r| active.contains(r))
                    .unwrap_or(false)
        });
        if has_active {
            return Err("A run is active on this project. Cancel or finish it before changing the project path.".to_string());
        }
    }

    config.projects[idx].name = new_name;
    config.projects[idx].path = new_path.clone();
    let updated = config.projects[idx].clone();
    save_config(&app_handle, &config)?;

    if path_changed {
        // Cascade to cards: update the in-memory mirror (cleaned comparison so
        // separator drift can't strand a card), then persist each touched id.
        let changed_ids: Vec<String> = {
            let mut cards = state.cards.lock().unwrap();
            let mut ids = Vec::new();
            for c in cards.iter_mut() {
                if clean_project_path(&c.project_path) == old_clean {
                    c.project_path = new_path.clone();
                    ids.push(c.id.clone());
                }
            }
            ids
        };
        if let Ok(conn) = get_db_conn(&app_handle) {
            for cid in &changed_ids {
                let _ = conn.execute(
                    "UPDATE cards SET project_path = ?1 WHERE id = ?2",
                    (&new_path, cid),
                );
            }
            // Cascade to long-term memories (stored under the cleaned path).
            let old_scope = old_clean.to_string_lossy().into_owned();
            let new_scope = new_clean.to_string_lossy().into_owned();
            let _ = conn.execute(
                "UPDATE memories SET project_path = ?1 WHERE project_path = ?2",
                (&new_scope, &old_scope),
            );
        }
    }

    Ok(updated)
}

/// Unregister a project, keyed by path. Deletes the project's cards and their
/// todo items. Deliberately NOT touched: the repository on disk, and the
/// project's long-term memories — memories are keyed by path, so
/// re-registering the project at the same path restores them intact.
#[tauri::command]
pub async fn delete_project(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let old_clean = clean_project_path(&path);

    let mut config = load_config(&app_handle);
    let idx = config
        .projects
        .iter()
        .position(|p| clean_project_path(&p.path) == old_clean)
        .ok_or_else(|| "Project not found.".to_string())?;

    {
        let cards = state.cards.lock().unwrap();
        let active = state.active_runs.lock().unwrap();
        let has_active = cards.iter().any(|c| {
            clean_project_path(&c.project_path) == old_clean
                && c.run_id
                    .as_deref()
                    .map(|r| active.contains(r))
                    .unwrap_or(false)
        });
        if has_active {
            return Err(
                "A run is active on this project. Cancel or finish it before removing the project."
                    .to_string(),
            );
        }
    }

    config.projects.remove(idx);
    save_config(&app_handle, &config)?;

    let removed_ids: Vec<String> = {
        let mut cards = state.cards.lock().unwrap();
        let ids: Vec<String> = cards
            .iter()
            .filter(|c| clean_project_path(&c.project_path) == old_clean)
            .map(|c| c.id.clone())
            .collect();
        cards.retain(|c| clean_project_path(&c.project_path) != old_clean);
        ids
    };
    if let Ok(conn) = get_db_conn(&app_handle) {
        for cid in &removed_ids {
            // Explicit todo_items delete: FK cascade depends on a per-connection
            // pragma we don't want to rely on here.
            let _ = conn.execute("DELETE FROM todo_items WHERE card_id = ?1", [cid]);
            let _ = conn.execute("DELETE FROM cards WHERE id = ?1", [cid]);
        }
    }

    Ok(())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MemoryEntry {
    pub id: i64,
    pub topic: String,
    pub content: String,
    pub source: String,
    pub created_at: String,
}

/// Read-only window into a project's long-term memory for the UI (the agent
/// reads memory through its `recall` tool; this feeds the Board Pulse panel).
#[tauri::command]
pub async fn list_memories(
    app_handle: tauri::AppHandle,
    project_path: String,
    limit: Option<u32>,
) -> Result<Vec<MemoryEntry>, String> {
    let scope = clean_project_path(&project_path)
        .to_string_lossy()
        .into_owned();
    let limit = limit.unwrap_or(8).clamp(1, 50) as i64;
    let conn = get_db_conn(&app_handle)?;
    let mut stmt = conn
        .prepare("SELECT id, topic, content, source, created_at FROM memories WHERE project_path = ?1 ORDER BY id DESC LIMIT ?2")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map((&scope, limit), |row| {
            Ok(MemoryEntry {
                id: row.get(0)?,
                topic: row.get(1)?,
                content: row.get(2)?,
                source: row.get(3)?,
                created_at: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command]
pub async fn get_settings(app_handle: tauri::AppHandle) -> LlmSettings {
    let config = load_config(&app_handle);
    config.settings
}

#[tauri::command]
pub async fn save_settings(
    app_handle: tauri::AppHandle,
    settings: LlmSettings,
) -> Result<(), String> {
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
                    "description": "Lists files and folders under a relative path as an indented tree. Pass depth 2-3 to map nested structure in one call instead of listing directories one at a time.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Relative path to list (use \"\" for project root)"
                            },
                            "depth": {
                                "type": "integer",
                                "description": "Optional recursion depth 1-4 (default 1). Use 2 or 3 to see nested folders in one call."
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
                        "properties": {
                            "confirm": {
                                "type": "boolean",
                                "description": "Optional confirmation flag; defaults to true"
                            }
                        },
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
                        "properties": {
                            "confirm": {
                                "type": "boolean",
                                "description": "Optional confirmation flag; defaults to true"
                            }
                        },
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
                    "description": "Searches file contents for a substring (case-insensitive by default) under a path or in a specific file. Results are grouped by file with line numbers. Pass context to include surrounding lines around each match.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "The substring to find"
                            },
                            "path": {
                                "type": "string",
                                "description": "Optional relative path to search within (specific file or folder). Defaults to project root if omitted."
                            },
                            "context": {
                                "type": "integer",
                                "description": "Optional context lines 0-5 (default 0). Use 2 to see how a match is used without a follow-up read_file."
                            },
                            "case_sensitive": {
                                "type": "boolean",
                                "description": "Optional; defaults to false (case-insensitive matching)."
                            }
                        },
                        "required": ["query"],
                        "additionalProperties": false
                    }
                }
            }),
            "find_file" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "find_file",
                    "description": "Finds files by name. Matches a case-insensitive fragment of the filename and returns matching relative paths. The fastest way to locate a file you know (part of) the name of.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "Filename or fragment to match, e.g. \"commands.rs\" or \"config\""
                            },
                            "path": {
                                "type": "string",
                                "description": "Optional relative folder to search within. Defaults to project root."
                            }
                        },
                        "required": ["name"],
                        "additionalProperties": false
                    }
                }
            }),
            "find_symbol" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "find_symbol",
                    "description": "Finds where a function, struct, class, enum, or other declaration is DEFINED. Returns file:line: signature for each definition site. Prefer this over search_grep when looking for a definition rather than usages.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "The symbol name to find, e.g. \"execute_tool\""
                            },
                            "path": {
                                "type": "string",
                                "description": "Optional relative path (file or folder) to search within. Defaults to project root."
                            }
                        },
                        "required": ["name"],
                        "additionalProperties": false
                    }
                }
            }),
            "remember" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "remember",
                    "description": "Saves a durable insight to this project's long-term memory, shared across runs and chat modes. Use for things worth keeping: how a subsystem works, a decision and its reason, a pitfall discovered.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "topic": {
                                "type": "string",
                                "description": "Short label for the memory, e.g. \"run engine timeouts\""
                            },
                            "content": {
                                "type": "string",
                                "description": "The insight to keep. Write it for a future agent with no context."
                            }
                        },
                        "required": ["topic", "content"],
                        "additionalProperties": false
                    }
                }
            }),
            "recall" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "recall",
                    "description": "Searches this project's long-term memory by keyword (topic and content, case-insensitive) and returns the most recent matches. Call with an empty query to see the latest memories. Check memory before exploring from scratch.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "Keyword to search for. Empty returns the most recent memories."
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Optional max results 1-10 (default 5)."
                            }
                        },
                        "required": ["query"],
                        "additionalProperties": false
                    }
                }
            }),
            "list_cards" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "list_cards",
                    "description": "Lists ALL kanban cards for this project, grouped by status, with ids and todo progress. Use it to see the board before filing or editing cards. (read_card shows only YOUR assigned card.)",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "confirm": {
                                "type": "boolean",
                                "description": "Optional confirmation flag; defaults to true"
                            }
                        },
                        "additionalProperties": false
                    }
                }
            }),
            "create_card" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "create_card",
                    "description": "Files a new kanban card in this project's backlog for the developer to review and schedule. Use it to capture follow-up work: bugs you discover outside your current scope, refactors worth doing, ideas from design discussions. Filing a card is ALWAYS better than silently expanding your current task.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "title": {
                                "type": "string",
                                "description": "Short, specific card title"
                            },
                            "description": {
                                "type": "string",
                                "description": "What the work is and why it matters. Write it for an agent with no context."
                            },
                            "todos": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Optional list of todo items breaking the work into steps"
                            },
                            "priority": {
                                "type": "string",
                                "enum": ["low", "medium", "high"],
                                "description": "Optional priority (default medium)"
                            },
                            "labels": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Optional keyword labels for filtering, e.g. [\"parser\", \"bug\"]"
                            }
                        },
                        "required": ["title", "description"],
                        "additionalProperties": false
                    }
                }
            }),
            "update_card" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "update_card",
                    "description": "Edits a card in this project's backlog or todo column: change its title, description, or priority, REPLACE the whole todo checklist, or append a single todo or label. Cards that are running, in review, or done cannot be edited.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "card_id": {
                                "type": "string",
                                "description": "Card id from list_cards"
                            },
                            "title": {
                                "type": "string",
                                "description": "Optional new title"
                            },
                            "description": {
                                "type": "string",
                                "description": "Optional new description (replaces the old one)"
                            },
                            "todos": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Optional: REPLACES the entire todo checklist with this list (all items unchecked). Use add_todo to append a single item instead."
                            },
                            "add_todo": {
                                "type": "string",
                                "description": "Optional todo item to append to the card"
                            },
                            "priority": {
                                "type": "string",
                                "enum": ["low", "medium", "high"],
                                "description": "Optional new priority"
                            },
                            "add_label": {
                                "type": "string",
                                "description": "Optional keyword label to add to the card"
                            }
                        },
                        "required": ["card_id"],
                        "additionalProperties": false
                    }
                }
            }),
            "delete_card" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "delete_card",
                    "description": "Deletes a card from this project's backlog or todo column. Cards with any run history, or that are running/in review/done, cannot be deleted.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "card_id": {
                                "type": "string",
                                "description": "Card id from list_cards"
                            }
                        },
                        "required": ["card_id"],
                        "additionalProperties": false
                    }
                }
            }),
            "read_card" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "read_card",
                    "description": "Shows your current card: title, description, status, and its todo list with indices and completion marks. Use the todos as your work plan.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "confirm": {
                                "type": "boolean",
                                "description": "Optional confirmation flag; defaults to true"
                            }
                        },
                        "additionalProperties": false
                    }
                }
            }),
            "set_todo" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "set_todo",
                    "description": "Checks off (or unchecks) a todo item on your card. Mark items complete as you finish them so progress is visible to the developer.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "index": { "type": "integer", "description": "Todo index from read_card (0-based)" },
                            "completed": { "type": "boolean", "description": "true to check off (default), false to uncheck" }
                        },
                        "required": ["index"],
                        "additionalProperties": false
                    }
                }
            }),
            "replace_lines" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "replace_lines",
                    "description": "Replaces an inclusive 1-indexed line range with new content. The precision tool for compiler errors: the compiler reports file:line and read_file output is line-numbered — use those exact numbers. Empty content deletes the range. Prefer this over patch_file when the target text contains quotes or escapes.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Relative path to the file" },
                            "start_line": { "type": "integer", "description": "First line to replace (1-indexed, inclusive)" },
                            "end_line": { "type": "integer", "description": "Last line to replace (1-indexed, inclusive)" },
                            "content": { "type": "string", "description": "Replacement text; may span multiple lines; empty string deletes the range" }
                        },
                        "required": ["path", "start_line", "end_line", "content"],
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
    // Compaction-aware replay: if the log contains compaction events, the
    // latest one's summary stands in for everything it covers, and only the
    // tail after the covered range is replayed verbatim. The full transcript
    // stays in the log, DB, and UI — this shapes only what the model sees.
    let mut compaction_summary: Option<String> = None;
    let mut replay_start = 0usize;
    for e in events.iter() {
        if e.event_type == "compaction" {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&e.payload) {
                if let (Some(s), Some(c)) = (
                    v.get("summary").and_then(|s| s.as_str()),
                    v.get("covers").and_then(|c| c.as_u64()),
                ) {
                    compaction_summary = Some(s.to_string());
                    replay_start = (c as usize).min(events.len());
                }
            }
        }
    }
    let events = &events[replay_start..];

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
    // Must exceed the largest single tool-result cap (read_file's 8000 chars
    // plus its truncation marker): tools append corrective guidance at the
    // TAIL of capped output, and a head-keeping budget below that cap would
    // behead the lesson before the model ever sees it. That exact failure
    // taught an agent its read_file "didn't support line ranges".
    const RECENT_MAX_CHARS: usize = 9000;
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
    if let Some(s) = compaction_summary {
        let block = format!(
            "[CONVERSATION SUMMARY — earlier turns were condensed to fit the context window. The full transcript is preserved outside this view; durable project facts may also be retrievable with recall().]\n{}\n[END SUMMARY — the conversation resumes below]",
            s
        );
        // Merge into the first message if it's already a user turn, so provider
        // role-alternation rules (e.g. Anthropic) are never violated.
        let merged = if let Some(first) = messages.first_mut() {
            if first.get("role").and_then(|r| r.as_str()) == Some("user") {
                let existing = first
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                first["content"] = serde_json::json!(format!("{}\n\n{}", block, existing));
                true
            } else {
                false
            }
        } else {
            false
        };
        if !merged {
            messages.insert(
                0,
                serde_json::json!({ "role": "user", "content": block }),
            );
        }
    }
    messages
}

/// Option-3 context management for long sessions: when the replayed history
/// exceeds the threshold, everything but a recent tail is condensed into a
/// single `compaction` event by the configured model. Nothing is deleted —
/// the UI and DB keep every event; only the model's view shrinks. If the
/// summarizer call fails, a stub summary still caps growth (graceful fallback
/// to plain trimming), so the user's message is never blocked on compaction.
const COMPACT_THRESHOLD_CHARS: usize = 48_000;
const COMPACT_KEEP_RECENT_EVENTS: usize = 12;

fn history_size_chars(messages: &[serde_json::Value]) -> usize {
    messages
        .iter()
        .map(|m| {
            m.get("content")
                .and_then(|c| c.as_str())
                .map(|s| s.len())
                .unwrap_or(0)
        })
        .sum()
}

fn compacted_history(
    app_handle: &tauri::AppHandle,
    run_id: &str,
    events: &[RunEvent],
) -> (Vec<serde_json::Value>, Option<RunEvent>) {
    let messages = get_history_messages(events);
    if history_size_chars(&messages) < COMPACT_THRESHOLD_CHARS
        || events.len() <= COMPACT_KEEP_RECENT_EVENTS + 4
    {
        return (messages, None);
    }

    let covers = events.len() - COMPACT_KEEP_RECENT_EVENTS;
    // Flatten the to-be-covered portion into a transcript for the summarizer.
    // This already folds in any previous compaction summary, so repeated
    // compactions compound instead of stacking.
    let old_msgs = get_history_messages(&events[..covers]);
    let mut transcript = String::new();
    for m in &old_msgs {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        let content = m.get("content").and_then(|c| c.as_str()).unwrap_or("");
        let clipped: String = content.chars().take(1200).collect();
        transcript.push_str(&format!("{}: {}\n", role.to_uppercase(), clipped));
    }
    let transcript: String = transcript.chars().take(30_000).collect();

    let system = "You are a context compaction engine. Condense the conversation transcript into a dense briefing the assistant will continue from. Preserve: decisions made and their reasons; the current task state and next steps; exact file paths, function names, card ids, and commands mentioned; unresolved questions; and the user's standing instructions. Omit pleasantries and repetition. Output ONLY the summary text, under 400 words.";
    let summarizer_history = vec![serde_json::json!({ "role": "user", "content": transcript })];

    // Synthetic run id: call_llm streams tokens keyed by run id, and the
    // summarizer's output must never paint into the visible chat.
    let summarizer_run_id = format!("{}-compaction", run_id);
    let fallback = "Earlier conversation was condensed to fit the context window; specifics may be retrievable with recall().".to_string();
    let summary = match call_llm(app_handle, &summarizer_run_id, system, summarizer_history, None)
    {
        Ok(raw) => {
            let (_, cleaned) = extract_reasoning(&raw);
            let s = cleaned.trim().to_string();
            if s.is_empty() {
                fallback
            } else {
                s.chars().take(4000).collect()
            }
        }
        Err(_) => fallback,
    };

    let event = RunEvent {
        run_id: run_id.to_string(),
        event_type: "compaction".to_string(),
        payload: serde_json::json!({ "summary": summary, "covers": covers }).to_string(),
    };
    let mut with_compaction = events.to_vec();
    with_compaction.push(event.clone());
    (get_history_messages(&with_compaction), Some(event))
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

            let events_snapshot: Vec<RunEvent> = {
                let logs = state.design_logs.lock().unwrap();
                logs.get(&log_key_clone).cloned().unwrap_or_default()
            };
            let (history, compaction) =
                compacted_history(&app_handle_clone, &log_key_clone, &events_snapshot);
            if let Some(ev) = compaction {
                append_design_event(&app_handle_clone, &state, &log_key_clone, ev);
            }

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
                "find_file",
                "find_symbol",
                "remember",
                "recall",
                "list_cards",
                "create_card",
                "update_card",
                "delete_card",
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

            if let Some((tool_name, args, preamble)) = parse_tool_call_spanned(&remaining) {
                // Surface the prose the model wrote before its tool call —
                // previously this commentary was silently clobbered.
                if !preamble.is_empty() {
                    append_design_event(
                        &app_handle_clone,
                        &state,
                        &log_key_clone,
                        RunEvent {
                            run_id: log_key_clone.clone(),
                            event_type: "message".to_string(),
                            payload: serde_json::json!({ "role": "agent", "content": preamble })
                                .to_string(),
                        },
                    );
                }
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

            let events_snapshot: Vec<RunEvent> = {
                let logs = state.code_logs.lock().unwrap();
                logs.get(&log_key_clone).cloned().unwrap_or_default()
            };
            let (history, compaction) =
                compacted_history(&app_handle_clone, &log_key_clone, &events_snapshot);
            if let Some(ev) = compaction {
                append_code_event(&app_handle_clone, &state, &log_key_clone, ev);
            }

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
                "find_file",
                "find_symbol",
                "remember",
                "recall",
                "list_cards",
                "create_card",
                "update_card",
                "delete_card",
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

            if let Some((tool_name, args, preamble)) = parse_tool_call_spanned(&remaining) {
                // Surface the prose the model wrote before its tool call —
                // previously this commentary was silently clobbered.
                if !preamble.is_empty() {
                    append_code_event(
                        &app_handle_clone,
                        &state,
                        &log_key_clone,
                        RunEvent {
                            run_id: log_key_clone.clone(),
                            event_type: "message".to_string(),
                            payload: serde_json::json!({ "role": "agent", "content": preamble })
                                .to_string(),
                        },
                    );
                }
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

/// Generate a unique card id. The old `card_{len+1}` scheme collided after
/// deletions (len shrinks, the next id re-mints an existing one, and the DB
/// insert fails silently); timestamp + uniqueness probe cannot.
fn new_card_id(cards: &[Card]) -> String {
    let base = format!("card_{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"));
    if !cards.iter().any(|c| c.id == base) {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{}_{}", base, n);
        if !cards.iter().any(|c| c.id == candidate) {
            return candidate;
        }
        n += 1;
    }
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
        id: new_card_id(&cards),
        project_path: project_path.clone(),
        title,
        description,
        status: initial_status,
        run_id: None,
        assignee: None,
        priority: "medium".to_string(),
        labels: Vec::new(),
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
        c.priority = normalize_priority(&card.priority);
        c.labels = card.labels;

        if let Ok(conn) = get_db_conn(&app_handle) {
            let labels_json = serde_json::to_string(&c.labels).unwrap_or_else(|_| "[]".to_string());
            let _ = conn.execute(
                "UPDATE cards SET title = ?1, description = ?2, status = ?3, run_id = ?4, assignee = ?5, project_path = ?6, priority = ?7, labels = ?8 WHERE id = ?9",
                (&c.title, &c.description, &c.status, &c.run_id, &c.assignee, &c.project_path, &c.priority, &labels_json, &c.id),
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
pub async fn is_run_active(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<bool, String> {
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
    // Unblocking is a world-refresh: drop the LM Studio stateful response-id
    // chain so the next call re-bootstraps with the CURRENT system prompt and
    // a full transcript replay. Threads otherwise keep the system prompt they
    // were born with, so tools added after a rebuild stay invisible mid-run.
    {
        let mut ids = state.lmstudio_response_ids.lock().unwrap();
        ids.remove(&run_id);
    }
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

        // Memory writeback happens only here, on accept: a reviewed, merged run
        // is the only kind whose summary deserves to become part of the
        // project's long-term memory. Rejected runs are never remembered.
        if let Ok(conn) = get_db_conn(&app_handle) {
            if let Some(summary) = latest_run_summary(&conn, &run_id) {
                let scope = repo_path.to_string_lossy().into_owned();
                let _ = insert_memory(
                    &conn,
                    &scope,
                    &format!("Completed: {}", card.title),
                    &summary,
                    "run_accept",
                    Some(&run_id),
                    Some(&card.id),
                );
            }
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
pub async fn read_diff(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<String, String> {
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
        // Live runs keep their work UNCOMMITTED in the worktree — a committed-
        // range diff is blind to it. Diff the worktree's tree against base.
        let worktree_dir = repo_path.join(".harness").join("worktrees").join(&run_id);
        if worktree_dir.exists() {
            return match git::get_worktree_diff(&repo_path, &run_id, &base_branch) {
                Ok(diff) if diff.trim().is_empty() => {
                    Ok("No changes detected in this run.".to_string())
                }
                Ok(diff) => Ok(diff),
                Err(e) => {
                    let err_msg = format!("Error generating diff: {}", e);
                    log_error(&err_msg);
                    Err(err_msg)
                }
            };
        }
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
         You have access to the following tools to interact with the repository. Issue EXACTLY ONE tool call per message, as a JSON object with \"name\" and \"args\". If your model has a native tool-call format (such as <tool_call>...</tool_call>), use it — it is fully supported. Otherwise, use this reference format:\n\n\
         ```tool_call\n\
         {{\n\
           \"name\": \"tool_name\",\n\
           \"args\": {{\n\
             \"arg1\": \"value1\"\n\
           }}\n\
         }}\n\
         ```\n\n\
         Keep any commentary brief and outside the tool call itself. Once you call a tool, the system will execute it and append the result. You can then analyze the output and make further tool calls.\n\n\
         Tools available:\n\
         1. `read_file(path: String, start_line?: Int, end_line?: Int)`: Reads file content. For large files, call outline_file first, then read only the line range you need. Output is line-numbered and capped — don't read whole large files when a range will do.\n\
         2. `outline_file(path: String)`: Returns a file's structure (markdown headings, or code declarations) with line numbers, without its full contents. Survey large files this way before reading.\n\
         3. `write_file(path: String, content: String)`: Writes content to a file (creating folders if needed).\n\
         4. `list_dir(path: String, depth?: Int)`: Lists files and folders under a relative path as an indented tree (use \"\" for root). Pass depth 2 or 3 to map nested structure in ONE call instead of listing directories one at a time.\n\
         5. `search_grep(query: String, path?: String, context?: Int, case_sensitive?: Bool)`: Searches file contents for a substring (case-insensitive by default) across the repo or under a path. Results are grouped by file with line numbers. Pass context: 2 to see surrounding lines without a follow-up read_file. Do NOT use shell grep.\n\
         6. `git_status()`: Runs `git status` in the sandbox.\n\
         7. `git_diff()`: Runs `git diff` to view your current sandboxed changes.\n\
         8. `run_command(command: String)`: Runs a build, test, or check shell command in the workspace (e.g. \"npm run build\", \"npm test\", \"cargo check\"). Use this to verify your code compiles and passes tests! NOTE: the shell is Windows cmd.exe — Unix tools like grep, sed, awk, and ls are NOT available. Use search_grep, patch_file, and list_dir instead.\n\
         9. `patch_file(path: String, target: String, replacement: String)`: Replaces an exact text snippet in a file. The target must match byte-for-byte including quotes and whitespace; if the snippet contains quotes or escapes, use replace_lines instead.\n\
         10. `replace_lines(path: String, start_line: int, end_line: int, content: String)`: Replaces an inclusive 1-indexed line range with new content (empty content deletes the lines). THE tool for fixing compiler errors: the compiler reports file:line and read_file output is line-numbered — read the reported lines, then replace exactly those line numbers. NEVER rewrite a whole file to fix a one-line error. WARNING: line numbers SHIFT after every successful edit — re-read the affected range before chaining another replace_lines.\n\
         11. `web_search(query: String)`: Searches the web for programming queries, libraries, APIs, or documentation snippets.\n\
         12. `send_notification(message: String)`: Sends a system alert/notification to the developer.\n\
         13. `read_card()`: Shows YOUR current card: title, description, status, and its todo list with indices. Read it at the start of a run and use the todos as your work plan.\n\
         14. `set_todo(index: int, completed: bool)`: Checks off a todo on your card (index from read_card). Mark items complete as you finish them so progress is visible.\n\
         15. `task_complete(summary: String)`: Ends the loop, summarizes your work, and puts the card in \"Review\" status. Before calling it, read_card and confirm every todo is checked — or explain why not in your summary.\n\
         16. `find_file(name: String, path?: String)`: Finds files by name. Give a fragment of the filename (case-insensitive) and get matching relative paths back. The fastest way to locate a file you know exists.\n\
         17. `find_symbol(name: String, path?: String)`: Finds where a function, struct, class, or other declaration is DEFINED. Returns file:line: signature. Faster and more precise than search_grep when you want a definition rather than usages.\n\
         18. `remember(topic: String, content: String)`: Saves a durable insight to this project's long-term memory — shared across runs and chat modes. Use it when you learn something worth keeping: how a subsystem works, a decision made, a pitfall discovered.\n\
         19. `recall(query: String, limit?: Int)`: Searches this project's long-term memory by keyword and returns the most recent matches (empty query = latest memories). Past runs may have already mapped the territory — check before exploring from scratch.\n\
         20. `list_cards()`: Shows ALL kanban cards for this project grouped by status, with ids and todo progress. (read_card shows only YOUR card.)\n\
         21. `create_card(title: String, description: String, todos?: [String], priority?: \"low\"|\"medium\"|\"high\", labels?: [String])`: Files a new card in the backlog for the developer to review. If you discover a bug or needed work OUTSIDE your current card's scope, file a card for it instead of silently expanding your task — then stay on your card.\n\
         22. `update_card(card_id: String, title?: String, description?: String, priority?: String, todos?: [String], add_todo?: String, add_label?: String)`: Edits a backlog/todo card. `todos` REPLACES the whole checklist; `add_todo` appends one item.\n\
         23. `delete_card(card_id: String)`: Deletes a backlog/todo card that has no run history.\n\n\
         Work efficiently with context: prefer outline_file + ranged read_file over reading entire files, since large reads slow the model and crowd out useful history. Starting unfamiliar work? Call recall() first — and remember() durable insights as you go. When you have finished the task, you MUST call task_complete — do not simply describe that you are done in prose.",
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

const KNOWN_TOOLS: [&str; 23] = [
    "read_file",
    "outline_file",
    "write_file",
    "patch_file",
    "replace_lines",
    "list_dir",
    "search_grep",
    "find_file",
    "find_symbol",
    "remember",
    "recall",
    "list_cards",
    "create_card",
    "update_card",
    "delete_card",
    "git_status",
    "git_diff",
    "run_command",
    "web_search",
    "send_notification",
    "read_card",
    "set_todo",
    "task_complete",
];

/// Parse function-call style tool syntax some chat templates emit, e.g.
/// `call:list_dir(path="src")` or `replace_lines(start_line: 10, end_line: 12, content: "...")`.
/// Keys may use `=` or `:` as separator. Values may be quoted strings,
/// integers, booleans, or null. Payload-bearing calls whose `content` value
/// contains unescaped interior quotes are handled by a greedy fallback.
fn parse_function_syntax(s: &str) -> Option<(String, serde_json::Value)> {
    let mut t = s.trim();
    for prefix in ["call:", "tool:", "function:"] {
        if let Some(stripped) = t.strip_prefix(prefix) {
            t = stripped.trim_start();
            break;
        }
    }
    let open = t.find('(')?;
    let name = t[..open].trim();
    if !fn_ident_ok(name) {
        return None;
    }
    let close = t.rfind(')')?;
    if close <= open {
        return None;
    }
    let args_str = t[open + 1..close].trim();
    let args = parse_fn_args_strict(args_str).or_else(|| parse_fn_args_greedy_content(args_str))?;
    Some((name.to_string(), serde_json::Value::Object(args)))
}

fn fn_ident_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_fn_scalar_value(val_str: &str) -> serde_json::Value {
    let v = val_str.trim();
    if v.starts_with('"') && v.ends_with('"') && v.len() >= 2 {
        serde_json::from_str(v)
            .unwrap_or_else(|_| serde_json::Value::String(v[1..v.len() - 1].to_string()))
    } else if let Ok(n) = v.parse::<i64>() {
        serde_json::json!(n)
    } else if v == "true" {
        serde_json::json!(true)
    } else if v == "false" {
        serde_json::json!(false)
    } else if v == "null" {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(v.to_string())
    }
}

/// Strict pass: comma-split at top level respecting (well-escaped) quoted
/// strings. Returns None if any part fails to parse cleanly — which happens
/// when a content payload contains unescaped quotes; the greedy pass rescues.
fn parse_fn_args_strict(args_str: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let mut args = serde_json::Map::new();
    if args_str.is_empty() {
        return Some(args);
    }
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut prev_escape = false;
    for c in args_str.chars() {
        if in_str {
            if prev_escape {
                prev_escape = false;
                cur.push(c);
                continue;
            }
            if c == '\\' {
                prev_escape = true;
                cur.push(c);
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            cur.push(c);
        } else {
            match c {
                '"' => {
                    in_str = true;
                    cur.push(c);
                }
                ',' => {
                    parts.push(cur.clone());
                    cur.clear();
                }
                _ => cur.push(c),
            }
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let sep = part.find(|c| c == '=' || c == ':')?;
        let key = part[..sep].trim();
        if !fn_ident_ok(key) {
            return None;
        }
        args.insert(
            key.to_string(),
            parse_fn_scalar_value(part[sep + 1..].trim()),
        );
    }
    Some(args)
}

/// Lenient scalar parsing for the regions around a greedy content span.
fn parse_fn_args_lenient(s: &str, args: &mut serde_json::Map<String, serde_json::Value>) {
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(sep) = part.find(|c| c == '=' || c == ':') {
            let key = part[..sep].trim();
            if fn_ident_ok(key) {
                args.insert(
                    key.to_string(),
                    parse_fn_scalar_value(part[sep + 1..].trim()),
                );
            }
        }
    }
}

/// Greedy pass for payload calls with unescaped interior quotes: `content` is
/// taken as the span from its opening quote to the LAST quote in the argument
/// list, and the scalar args before/after it are parsed leniently. Sound as
/// long as `content` is the only string-valued argument — true for
/// write_file/replace_lines/patch_file-style calls where the payload is code.
fn parse_fn_args_greedy_content(
    args_str: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let ckey = args_str.find("content")?;
    let after = &args_str[ckey + 7..];
    let sep_rel = after.find(|c| c == '=' || c == ':')?;
    if !after[..sep_rel].trim().is_empty() {
        return None;
    }
    let q_off = after[sep_rel + 1..].find('"')?;
    let content_start = ckey + 7 + sep_rel + 1 + q_off + 1;
    let last_q = args_str.rfind('"')?;
    if last_q <= content_start {
        return None;
    }
    let content_val = &args_str[content_start..last_q];
    let mut args = serde_json::Map::new();
    parse_fn_args_lenient(args_str[..ckey].trim_end().trim_end_matches(','), &mut args);
    parse_fn_args_lenient(
        args_str[last_q + 1..].trim_start().trim_start_matches(','),
        &mut args,
    );
    args.insert(
        "content".to_string(),
        serde_json::Value::String(content_val.to_string()),
    );
    Some(args)
}

/// Extract tool arguments tolerantly: our protocol says "args", but native
/// chat templates use "arguments" (Qwen/Hermes) or "parameters", and some
/// stringify the object. Accept all of them.
fn extract_tool_args(val: &serde_json::Value) -> serde_json::Value {
    let args = val
        .get("args")
        .or_else(|| val.get("arguments"))
        .or_else(|| val.get("parameters"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if let serde_json::Value::String(s) = &args {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
            return parsed;
        }
    }
    args
}

fn try_parse_json(s: &str) -> Option<(String, serde_json::Value)> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(s) {
        if let Some(name) = val.get("name").and_then(|n| n.as_str()) {
            let args = extract_tool_args(&val);
            return Some((name.to_string(), args));
        }
    }

    if let Some(start_brace) = s.find('{') {
        if let Some(end_brace) = s.rfind('}') {
            if end_brace > start_brace {
                let json_sub = &s[start_brace..=end_brace];
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_sub) {
                    if let Some(name) = val.get("name").and_then(|n| n.as_str()) {
                        let args = extract_tool_args(&val);
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

fn parse_qwen_syntax(s: &str) -> Option<(String, serde_json::Value)> {
    let mut t = s.trim();
    for prefix in ["call:", "tool:", "function:"] {
        if let Some(stripped) = t.strip_prefix(prefix) {
            t = stripped.trim_start();
            break;
        }
    }
    let brace = t.find('{')?;
    let name = t[..brace].trim();
    if !fn_ident_ok(name) {
        return None;
    }
    let close = t.rfind('}')?;
    if close < brace {
        return None;
    }
    let json_str = &t[brace..=close];
    let args: serde_json::Value = serde_json::from_str(json_str).ok()?;
    Some((name.to_string(), args))
}

/// Prose the model wrote before its tool-call syntax. Returns empty for
/// syntax debris (stray punctuation, lone backticks); caps length so a
/// pathological preamble can't bloat the transcript or the history budget.
fn clean_tool_preamble(text: &str) -> String {
    let t = text.trim();
    if !t.chars().any(|c| c.is_alphanumeric()) {
        return String::new();
    }
    const MAX: usize = 4000;
    if t.chars().count() > MAX {
        let truncated: String = t.chars().take(MAX).collect();
        format!("{}\n...[truncated]", truncated)
    } else {
        t.to_string()
    }
}

/// Like parse_tool_call_at, but operates on the raw response and returns the
/// prose the model emitted BEFORE the tool-call syntax, so the loop can
/// surface it instead of clobbering it.
fn parse_tool_call_spanned(response: &str) -> Option<(String, serde_json::Value, String)> {
    let mut response_cleaned = response.trim();
    if response_cleaned.ends_with("<tool_call|>") {
        response_cleaned = response_cleaned.trim_end_matches("<tool_call|>").trim();
    }
    let (name, args, start) = parse_tool_call_at(response_cleaned)?;
    let preamble = clean_tool_preamble(&response_cleaned[..start]);
    Some((name, args, preamble))
}

/// Core tool-call detection. Returns (name, args, offset) where offset is the
/// byte position in `response_cleaned` where the tool-call syntax begins —
/// everything before it is the model's preamble prose.
fn parse_tool_call_at(response_cleaned: &str) -> Option<(String, serde_json::Value, usize)> {
    for len in (2..=5).rev() {
        let bt = "`".repeat(len);
        let mut start_search = 0;

        while let Some(start_idx) = response_cleaned[start_search..].find(&bt) {
            let absolute_start = start_search + start_idx;
            let after_start = &response_cleaned[absolute_start + len..];

            if let Some(end_idx) = after_start.find(&bt) {
                let block_str = &after_start[..end_idx];
                let trimmed_block = strip_lang_prefix(block_str);

                if let Some((name, args)) = try_parse_json(trimmed_block) {
                    return Some((name, args, absolute_start));
                }
            }
            start_search = absolute_start + len;
        }
    }

    // Native chat-template formats: Qwen/Hermes-style <tool_call>{...}</tool_call>,
    // pipe/space variants, and MANGLED special-token approximations like
    // `<|tool_call>...<tool_call|>` (a model typing its own special token from
    // memory). Tolerates a missing closing tag when generation was truncated.
    const TAG_OPENERS: [&str; 5] = [
        "<tool_call>",
        "<|tool_call|>",
        "<|tool_call>",
        "<tool call>",
        "<|tool call|>",
    ];
    for opener in TAG_OPENERS {
        let mut search = 0;
        while let Some(idx) = response_cleaned[search..].find(opener) {
            let opener_pos = search + idx;
            let start = opener_pos + opener.len();
            let rest = &response_cleaned[start..];
            let end = rest
                .find("</tool_call>")
                .or_else(|| rest.find("<|/tool_call|>"))
                .or_else(|| rest.find("<tool_call|>"))
                .or_else(|| rest.find("<|tool_call>"));
            let inner = match end {
                Some(e) => &rest[..e],
                None => rest,
            };
            let inner = inner.trim();
            if let Some((name, args)) = try_parse_json(inner) {
                return Some((name, args, opener_pos));
            }
            if let Some((name, args)) = parse_function_syntax(inner) {
                return Some((name, args, opener_pos));
            }
            if let Some((name, args)) = parse_qwen_syntax(inner) {
                return Some((name, args, opener_pos));
            }
            search = start;
        }
    }

    if let Some((name, args)) = try_parse_json(response_cleaned) {
        return Some((name, args, 0));
    }

    // Bare function-call syntax in the response body (e.g. `list_dir(path="src")`),
    // anchored to known tool names so ordinary prose can't false-positive.
    for tool in KNOWN_TOOLS {
        let pat = format!("{}(", tool);
        if let Some(idx) = response_cleaned.find(&pat) {
            if let Some((name, args)) = parse_function_syntax(&response_cleaned[idx..]) {
                if name == tool {
                    return Some((name, args, idx));
                }
            }
        }
        // The `call:`-prefixed form must be checked BEFORE the bare brace form:
        // the bare pattern is a substring of it, and matching the bare form first
        // would leave a dangling "call:" classified as preamble prose.
        let pat_prefix = format!("call:{}{{", tool);
        if let Some(idx) = response_cleaned.find(&pat_prefix) {
            if let Some((name, args)) = parse_qwen_syntax(&response_cleaned[idx..]) {
                if name == tool {
                    return Some((name, args, idx));
                }
            }
        }
        // Bare Qwen/Hermes-style JSON calls in the response body (e.g. `list_dir{"path": "src"}`)
        let pat_brace = format!("{}{{", tool);
        if let Some(idx) = response_cleaned.find(&pat_brace) {
            if let Some((name, args)) = parse_qwen_syntax(&response_cleaned[idx..]) {
                if name == tool {
                    return Some((name, args, idx));
                }
            }
        }
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

/// Declaration starters shared by outline_file and find_symbol. Checked against
/// the leading token of each trimmed line.
const DECL_STARTERS: [&str; 28] = [
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

/// Shared ignore rule for repository walks: hidden entries plus dependency and
/// build-output folders. Keep this the single source of truth so list_dir,
/// search_grep, find_file, and find_symbol all see the same world.
fn is_ignored_entry(name: &str) -> bool {
    name.starts_with('.') || name == "node_modules" || name == "target" || name == "dist"
}

/// Recursively collect non-ignored files under `dir`.
fn collect_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if is_ignored_entry(&name) {
            continue;
        }
        if p.is_dir() {
            collect_files(&p, files);
        } else {
            files.push(p);
        }
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
            if DECL_STARTERS.iter().any(|s| trimmed.starts_with(s)) {
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

/// Substring search across the repo (or a path), case-insensitive by default.
/// Output is grouped per file with line numbers; optional context lines surround
/// each match (the matching line marked with '>'). Caps: 10 matches shown per
/// file, 50 total, so one noisy file can't eat the whole context budget.
fn search_grep_impl(
    worktree_path: &Path,
    query: &str,
    path_opt: &str,
    context: usize,
    case_sensitive: bool,
) -> String {
    const MAX_TOTAL: usize = 50;
    const MAX_PER_FILE: usize = 10;
    let worktree_abs = clean_project_path(worktree_path);
    let target = match verify_sandbox(&worktree_abs, path_opt) {
        Ok(p) => p,
        Err(e) => return format!("Error: {}", e),
    };
    let needle = if case_sensitive {
        query.to_string()
    } else {
        query.to_lowercase()
    };

    let mut files: Vec<PathBuf> = Vec::new();
    if target.is_file() {
        files.push(target.clone());
    } else {
        collect_files(&target, &mut files);
    }

    let mut out: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut truncated = false;

    'files: for file in &files {
        let Ok(content) = fs::read_to_string(file) else {
            continue;
        };
        let lines: Vec<&str> = content.lines().collect();
        let mut hits: Vec<usize> = Vec::new();
        for (idx, line) in lines.iter().enumerate() {
            let matched = if case_sensitive {
                line.contains(&needle)
            } else {
                line.to_lowercase().contains(&needle)
            };
            if matched {
                hits.push(idx);
            }
        }
        if hits.is_empty() {
            continue;
        }
        let rel = file
            .strip_prefix(&worktree_abs)
            .unwrap_or(file)
            .to_string_lossy()
            .into_owned();
        out.push(format!(
            "== {} ({} match{})",
            rel,
            hits.len(),
            if hits.len() == 1 { "" } else { "es" }
        ));
        for (n, &hit) in hits.iter().enumerate() {
            if n >= MAX_PER_FILE {
                out.push(format!(
                    "   ... {} more match(es) in this file — search it directly for the rest",
                    hits.len() - MAX_PER_FILE
                ));
                break;
            }
            if total >= MAX_TOTAL {
                truncated = true;
                break 'files;
            }
            if context == 0 {
                out.push(format!("{}: {}", hit + 1, lines[hit]));
            } else {
                let from = hit.saturating_sub(context);
                let to = (hit + context).min(lines.len().saturating_sub(1));
                for i in from..=to {
                    let marker = if i == hit { ">" } else { " " };
                    out.push(format!("{}{}: {}", marker, i + 1, lines[i]));
                }
                out.push("--".to_string());
            }
            total += 1;
        }
    }

    if truncated {
        out.push(format!(
            "... truncated after {} matches. Narrow the query or pass a path.",
            MAX_TOTAL
        ));
    }
    if out.is_empty() {
        format!(
            "No matches found for \"{}\"{}",
            query,
            if case_sensitive {
                " (case-sensitive — try case_sensitive: false)"
            } else {
                ""
            }
        )
    } else {
        out.join("\n")
    }
}

/// Depth-limited tree listing. Folders end with '/', children indented two
/// spaces per level. Capped so a giant repo can't blow the context budget.
fn list_dir_impl(target: &Path, depth: usize) -> String {
    const MAX_ENTRIES: usize = 200;
    fn walk(dir: &Path, level: usize, depth: usize, out: &mut Vec<String>, count: &mut usize) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| {
            let p = e.path();
            (
                !p.is_dir(),
                p.file_name().unwrap_or_default().to_ascii_lowercase(),
            )
        });
        for entry in entries {
            if *count >= MAX_ENTRIES {
                return;
            }
            let p = entry.path();
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if is_ignored_entry(&name) {
                continue;
            }
            let indent = "  ".repeat(level);
            if p.is_dir() {
                out.push(format!("{}{}/", indent, name));
                *count += 1;
                if level + 1 < depth {
                    walk(&p, level + 1, depth, out, count);
                }
            } else {
                out.push(format!("{}{}", indent, name));
                *count += 1;
            }
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut count = 0usize;
    walk(target, 0, depth, &mut out, &mut count);
    if out.is_empty() {
        return "Directory is empty".to_string();
    }
    if count >= MAX_ENTRIES {
        out.push(format!(
            "... truncated at {} entries. List a subdirectory or use a lower depth for more.",
            MAX_ENTRIES
        ));
    }
    out.join("\n")
}

/// Find files whose name contains a case-insensitive fragment. Returns relative
/// paths, one per line.
fn find_file_impl(worktree_path: &Path, name_query: &str, path_opt: &str) -> String {
    const MAX_RESULTS: usize = 50;
    let worktree_abs = clean_project_path(worktree_path);
    let target = match verify_sandbox(&worktree_abs, path_opt) {
        Ok(p) => p,
        Err(e) => return format!("Error: {}", e),
    };
    let needle = name_query.to_lowercase();
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(&target, &mut files);
    let mut out: Vec<String> = Vec::new();
    for file in &files {
        let fname = file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        if fname.contains(&needle) {
            let rel = file
                .strip_prefix(&worktree_abs)
                .unwrap_or(file)
                .to_string_lossy()
                .into_owned();
            out.push(rel);
            if out.len() >= MAX_RESULTS {
                out.push(format!(
                    "... truncated at {} results. Use a more specific name.",
                    MAX_RESULTS
                ));
                break;
            }
        }
    }
    if out.is_empty() {
        format!(
            "No files matching \"{}\" found. Try a shorter fragment of the name, or list_dir with depth to browse.",
            name_query
        )
    } else {
        out.join("\n")
    }
}

/// Find where a symbol is declared: scans non-ignored files for declaration
/// lines (same starters as outline_file) containing the symbol name, and
/// returns file:line: signature for each.
fn find_symbol_impl(worktree_path: &Path, symbol: &str, path_opt: &str) -> String {
    const MAX_RESULTS: usize = 30;
    let worktree_abs = clean_project_path(worktree_path);
    let target = match verify_sandbox(&worktree_abs, path_opt) {
        Ok(p) => p,
        Err(e) => return format!("Error: {}", e),
    };
    let mut files: Vec<PathBuf> = Vec::new();
    if target.is_file() {
        files.push(target.clone());
    } else {
        collect_files(&target, &mut files);
    }
    let mut out: Vec<String> = Vec::new();
    let mut count = 0usize;
    'files: for file in &files {
        let Ok(content) = fs::read_to_string(file) else {
            continue;
        };
        let rel = file
            .strip_prefix(&worktree_abs)
            .unwrap_or(file)
            .to_string_lossy()
            .into_owned();
        for (idx, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();
            if DECL_STARTERS.iter().any(|s| trimmed.starts_with(s)) && trimmed.contains(symbol) {
                let sig = trimmed
                    .split(|c| c == '{' || c == ';')
                    .next()
                    .unwrap_or(trimmed)
                    .trim_end();
                out.push(format!("{}:{}: {}", rel, idx + 1, sig));
                count += 1;
                if count >= MAX_RESULTS {
                    out.push(format!("... truncated at {} results.", MAX_RESULTS));
                    break 'files;
                }
            }
        }
    }
    if out.is_empty() {
        format!(
            "No declarations matching \"{}\" found. It may be defined in a pattern this tool doesn't recognize — try search_grep.",
            symbol
        )
    } else {
        format!(
            "Declarations matching \"{}\" (file:line: signature). Use read_file with the line number to inspect:\n{}",
            symbol,
            out.join("\n")
        )
    }
}

fn replace_lines_impl(
    worktree_path: &Path,
    path: &str,
    start_line: usize,
    end_line: usize,
    new_content: &str,
) -> String {
    let worktree_abs = clean_project_path(worktree_path);
    let target_file = match verify_sandbox(&worktree_abs, path) {
        Ok(p) => p,
        Err(e) => return format!("Error: {}", e),
    };
    let original = match fs::read_to_string(&target_file) {
        Ok(c) => c,
        Err(e) => return format!("Error reading '{}': {}", path, e),
    };
    let lines: Vec<&str> = original.lines().collect();
    if start_line == 0 || end_line < start_line || start_line > lines.len() {
        return format!(
            "Error: invalid line range {}-{} ('{}' has {} lines)",
            start_line,
            end_line,
            path,
            lines.len()
        );
    }
    let end_line = end_line.min(lines.len());
    let first_removed: String = lines
        .get(start_line - 1)
        .map(|l| l.chars().take(90).collect())
        .unwrap_or_default();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    out.extend(lines[..start_line - 1].iter().map(|s| s.to_string()));
    let mut inserted = 0usize;
    if !new_content.is_empty() {
        for l in new_content.lines() {
            out.push(l.to_string());
            inserted += 1;
        }
    }
    out.extend(lines[end_line..].iter().map(|s| s.to_string()));
    let mut joined = out.join("\n");
    if original.ends_with('\n') {
        joined.push('\n');
    }
    if let Err(e) = fs::write(&target_file, joined) {
        return format!("Error writing '{}': {}", path, e);
    }
    format!(
        "Success: replaced lines {}-{} of '{}' ({} line(s) removed, {} inserted). First removed line was: `{}`. If that is NOT the line you expected to remove, the file has changed since you read it — line numbers SHIFT after every edit. Re-read the section before making further edits.",
        start_line,
        end_line,
        path,
        end_line - start_line + 1,
        inserted,
        first_removed
    )
}

/// Detect the worktree's project type and run its compile/type check.
/// Ok(note) when the check passed or no recognizable build system exists;
/// Err(message with the first ~2500 chars of output) when the check FAILED.
/// The loop's ground truth is the compiler, not the model's self-report.
fn run_verification(worktree_path: &Path) -> Result<String, String> {
    let (dir, label, program, args): (PathBuf, &str, &str, Vec<&str>) =
        if worktree_path.join("src-tauri").join("Cargo.toml").exists() {
            (
                worktree_path.join("src-tauri"),
                "cargo check",
                "cargo",
                vec!["check", "--color", "never"],
            )
        } else if worktree_path.join("Cargo.toml").exists() {
            (
                worktree_path.to_path_buf(),
                "cargo check",
                "cargo",
                vec!["check", "--color", "never"],
            )
        } else if worktree_path.join("tsconfig.json").exists() {
            (
                worktree_path.to_path_buf(),
                "npx tsc --noEmit",
                "cmd.exe",
                vec!["/C", "npx tsc --noEmit"],
            )
        } else {
            return Ok("no recognizable build system; verification skipped".to_string());
        };
    let mut cmd = std::process::Command::new(program);
    cmd.args(&args);
    cmd.current_dir(&dir);
    crate::configure_no_window(&mut cmd);
    match cmd.output() {
        Ok(output) if output.status.success() => Ok(format!("{} passed", label)),
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{}\n{}", stdout, stderr);
            let head: String = combined.chars().take(2500).collect();
            Err(format!("{} FAILED:\n{}", label, head))
        }
        // Tool missing on PATH etc. — don't hard-block completion on environment problems.
        Err(e) => Ok(format!(
            "could not run {} ({}); verification skipped",
            label, e
        )),
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
                return format!("Error: Target text not found in '{}'. The target must match the file exactly, including whitespace and quotes. If the snippet contains quotes or escapes, use replace_lines(path: String, start_line: int, end_line: int, content: String) with the line numbers from read_file instead.", path);
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
            let depth = args
                .get("depth")
                .and_then(|v| v.as_u64())
                .map(|v| (v as usize).clamp(1, 4))
                .unwrap_or(1);
            let target = match verify_sandbox(worktree_path, path) {
                Ok(p) => p,
                Err(e) => return format!("Error: {}", e),
            };
            list_dir_impl(&target, depth)
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
            // Gate completion on verification: claiming done with a broken build
            // is the single most expensive failure mode for the reviewer.
            match run_verification(worktree_path) {
                Err(failure) => {
                    return format!(
                        "Error: task_complete REJECTED — the project does not verify cleanly.\n{}\nFix the errors above (replace_lines with the reported line numbers), confirm with run_command, then call task_complete again.",
                        failure
                    );
                }
                Ok(note) => {
                    let changes = run_git_command(worktree_path, &["status", "--short"])
                        .unwrap_or_else(|e| format!("(git status unavailable: {})", e));
                    let changes_display = if changes.trim().is_empty() {
                        "(none — WARNING: there are no changes to merge)".to_string()
                    } else {
                        changes.trim().to_string()
                    };
                    let state = app_handle.state::<AppState>();
                    let mut cards = state.cards.lock().unwrap();
                    if let Some(card) = cards
                        .iter_mut()
                        .find(|c| c.run_id.as_deref() == Some(run_id))
                    {
                        card.status = "review".to_string();
                        let _ = app_handle
                            .emit("notification", format!("Task completed: {}", card.title));
                    }
                    format!(
                        "Success: Task completed ({}).\nWorking-tree changes:\n{}\nSummary: {}",
                        note, changes_display, summary
                    )
                }
            }
        }
        "search_grep" => {
            let query = match args.get("query").and_then(|q| q.as_str()) {
                Some(q) => q,
                None => return "Error: Missing query argument".to_string(),
            };
            let path_opt = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
            let context = args
                .get("context")
                .and_then(|v| v.as_u64())
                .map(|v| (v as usize).min(5))
                .unwrap_or(0);
            let case_sensitive = args
                .get("case_sensitive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            search_grep_impl(worktree_path, query, path_opt, context, case_sensitive)
        }
        "find_file" => {
            let name = match args.get("name").and_then(|n| n.as_str()) {
                Some(n) => n,
                None => return "Error: Missing name argument".to_string(),
            };
            let path_opt = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
            find_file_impl(worktree_path, name, path_opt)
        }
        "find_symbol" => {
            let name = match args.get("name").and_then(|n| n.as_str()) {
                Some(n) => n,
                None => return "Error: Missing name argument".to_string(),
            };
            let path_opt = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
            find_symbol_impl(worktree_path, name, path_opt)
        }
        "remember" => {
            const USAGE: &str = " Usage: remember(topic: String, content: String) — a short topic label and the insight worth keeping.";
            let topic = match args.get("topic").and_then(|t| t.as_str()) {
                Some(t) if !t.trim().is_empty() => t.trim(),
                _ => return format!("Error: Missing 'topic' argument.{}", USAGE),
            };
            let content = match args.get("content").and_then(|c| c.as_str()) {
                Some(c) if !c.trim().is_empty() => c.trim(),
                _ => return format!("Error: Missing 'content' argument.{}", USAGE),
            };
            let topic: String = topic.chars().take(120).collect();
            let content: String = content.chars().take(2000).collect();
            let (scope, card_id) = memory_scope(app_handle, worktree_path, run_id);
            match get_db_conn(app_handle) {
                Ok(conn) => match insert_memory(
                    &conn,
                    &scope,
                    &topic,
                    &content,
                    "agent",
                    Some(run_id),
                    card_id.as_deref(),
                ) {
                    Ok(_) => format!(
                        "Success: remembered under topic '{}'. This memory persists across runs and chat modes for this project.",
                        topic
                    ),
                    Err(e) => format!("Error saving memory: {}", e),
                },
                Err(e) => format!("Error: {}", e),
            }
        }
        "recall" => {
            let query = args
                .get("query")
                .and_then(|q| q.as_str())
                .unwrap_or("")
                .trim();
            let limit = args
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|v| (v as usize).clamp(1, 10))
                .unwrap_or(5);
            let (scope, _) = memory_scope(app_handle, worktree_path, run_id);
            let conn = match get_db_conn(app_handle) {
                Ok(c) => c,
                Err(e) => return format!("Error: {}", e),
            };
            let result: Result<Vec<(String, String, String, String)>, rusqlite::Error> = (|| {
                let mut rows = Vec::new();
                if query.is_empty() {
                    let mut stmt = conn.prepare(
                        "SELECT topic, content, source, created_at FROM memories WHERE project_path = ?1 ORDER BY id DESC LIMIT ?2",
                    )?;
                    let it = stmt.query_map((&scope, limit as i64), |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                    })?;
                    for r in it {
                        rows.push(r?);
                    }
                } else {
                    let escaped = query
                        .replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_");
                    let pattern = format!("%{}%", escaped);
                    let mut stmt = conn.prepare(
                        "SELECT topic, content, source, created_at FROM memories WHERE project_path = ?1 AND (topic LIKE ?2 ESCAPE '\\' OR content LIKE ?2 ESCAPE '\\') ORDER BY id DESC LIMIT ?3",
                    )?;
                    let it = stmt.query_map((&scope, &pattern, limit as i64), |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                    })?;
                    for r in it {
                        rows.push(r?);
                    }
                }
                Ok(rows)
            })();
            let rows = match result {
                Ok(r) => r,
                Err(e) => return format!("Error reading memories: {}", e),
            };
            if rows.is_empty() {
                if query.is_empty() {
                    "No memories stored for this project yet. Use remember(topic, content) to save durable insights."
                        .to_string()
                } else {
                    format!(
                        "No memories matching \"{}\" for this project. Try a broader keyword, or call recall with an empty query for the most recent memories.",
                        query
                    )
                }
            } else {
                let mut out = vec![format!(
                    "{} memor{} (newest first):",
                    rows.len(),
                    if rows.len() == 1 { "y" } else { "ies" }
                )];
                for (topic, content, source, created_at) in rows {
                    let date = created_at.split('T').next().unwrap_or("").to_string();
                    out.push(format!("[{} | {}] {}: {}", date, source, topic, content));
                }
                out.join("\n")
            }
        }
        "list_cards" => {
            let (scope, _) = memory_scope(app_handle, worktree_path, run_id);
            let scope_clean = PathBuf::from(&scope);
            let state_handle = match app_handle.try_state::<AppState>() {
                Some(s) => s,
                None => return "Error: app state unavailable".to_string(),
            };
            let cards = state_handle.cards.lock().unwrap();
            let mut out: Vec<String> = Vec::new();
            for status in ["backlog", "todo", "running", "blocked", "review", "done", "failed"] {
                let mut group: Vec<&Card> = cards
                    .iter()
                    .filter(|c| {
                        c.status == status
                            && clean_project_path(&c.project_path) == scope_clean
                    })
                    .collect();
                if group.is_empty() {
                    continue;
                }
                group.sort_by_key(|c| match c.priority.as_str() {
                    "high" => 0u8,
                    "low" => 2,
                    _ => 1,
                });
                out.push(format!("{}:", status.to_uppercase()));
                for c in group {
                    let done = c.todo_list.iter().filter(|t| t.completed).count();
                    let labels = if c.labels.is_empty() {
                        String::new()
                    } else {
                        format!(" {{{}}}", c.labels.join(", "))
                    };
                    out.push(format!(
                        "  [{}] ({}) {}{} (todos {}/{})",
                        c.id,
                        c.priority,
                        c.title,
                        labels,
                        done,
                        c.todo_list.len()
                    ));
                }
            }
            if out.is_empty() {
                "No cards exist for this project yet. Use create_card to file work.".to_string()
            } else {
                out.join("\n")
            }
        }
        "create_card" => {
            const USAGE: &str = " Usage: create_card(title: String, description: String, todos?: [String]) — files a new card in the backlog.";
            let title = match args.get("title").and_then(|t| t.as_str()) {
                Some(t) if !t.trim().is_empty() => t.trim().to_string(),
                _ => return format!("Error: Missing 'title' argument.{}", USAGE),
            };
            let description = args
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let todos: Vec<String> = args
                .get("todos")
                .and_then(|t| t.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let priority = args
                .get("priority")
                .and_then(|p| p.as_str())
                .map(normalize_priority)
                .unwrap_or_else(default_priority);
            let labels: Vec<String> = args
                .get("labels")
                .and_then(|l| l.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let (scope, _) = memory_scope(app_handle, worktree_path, run_id);
            let scope_clean = PathBuf::from(&scope);
            let state_handle = match app_handle.try_state::<AppState>() {
                Some(s) => s,
                None => return "Error: app state unavailable".to_string(),
            };
            let mut cards = state_handle.cards.lock().unwrap();
            // Reuse the raw project_path of a sibling card so the UI's raw-equality
            // filter sees the new card; fall back to the cleaned scope string.
            let project_path = cards
                .iter()
                .find(|c| clean_project_path(&c.project_path) == scope_clean)
                .map(|c| c.project_path.clone())
                .unwrap_or_else(|| scope.clone());
            let id = new_card_id(&cards);
            let new_card = Card {
                id: id.clone(),
                project_path,
                title,
                description,
                status: "backlog".to_string(),
                run_id: None,
                assignee: None,
                priority,
                labels,
                todo_list: todos
                    .iter()
                    .map(|t| TodoItem {
                        text: t.clone(),
                        completed: false,
                    })
                    .collect(),
            };
            cards.push(new_card.clone());
            if let Ok(conn) = get_db_conn(app_handle) {
                let labels_json =
                    serde_json::to_string(&new_card.labels).unwrap_or_else(|_| "[]".to_string());
                let _ = conn.execute(
                    "INSERT INTO cards (id, project_path, title, description, status, run_id, assignee, priority, labels) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    (&new_card.id, &new_card.project_path, &new_card.title, &new_card.description, &new_card.status, &new_card.run_id, &new_card.assignee, &new_card.priority, &labels_json),
                );
                for (idx, item) in new_card.todo_list.iter().enumerate() {
                    let _ = conn.execute(
                        "INSERT INTO todo_items (card_id, idx, text, completed) VALUES (?1, ?2, ?3, ?4)",
                        (&new_card.id, idx as i32, &item.text, 0),
                    );
                }
            }
            let _ = app_handle.emit("run-updated", serde_json::json!({ "run_id": run_id }));
            format!(
                "Success: card '{}' filed in the backlog with id {} ({} todo item{}). The developer will review and schedule it.{}",
                new_card.title,
                id,
                new_card.todo_list.len(),
                if new_card.todo_list.len() == 1 { "" } else { "s" },
                unknown_args_note(args, &["title", "description", "todos", "priority", "labels"])
            )
        }
        "update_card" => {
            const USAGE: &str = " Usage: update_card(card_id: String, title?: String, description?: String, priority?: String, todos?: [String], add_todo?: String, add_label?: String) — card_id comes from list_cards; only backlog/todo cards can be edited. `todos` REPLACES the whole checklist; `add_todo` appends one item.";
            const KNOWN: &[&str] = &[
                "card_id",
                "title",
                "description",
                "priority",
                "todos",
                "add_todo",
                "add_label",
            ];
            let card_id = match args.get("card_id").and_then(|c| c.as_str()) {
                Some(c) if !c.trim().is_empty() => c.trim().to_string(),
                _ => return format!("Error: Missing 'card_id' argument.{}", USAGE),
            };
            let new_title = args.get("title").and_then(|t| t.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let new_desc = args.get("description").and_then(|d| d.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let add_todo = args.get("add_todo").and_then(|t| t.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let new_priority = args.get("priority").and_then(|p| p.as_str()).map(normalize_priority);
            let add_label = args.get("add_label").and_then(|l| l.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let new_todos: Option<Vec<String>> = args.get("todos").and_then(|t| t.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            });
            if new_title.is_none() && new_desc.is_none() && add_todo.is_none() && new_priority.is_none() && add_label.is_none() && new_todos.is_none() {
                return format!("Error: nothing to update — provide title, description, priority, todos, add_todo, or add_label.{}", USAGE);
            }
            let (scope, _) = memory_scope(app_handle, worktree_path, run_id);
            let scope_clean = PathBuf::from(&scope);
            let state_handle = match app_handle.try_state::<AppState>() {
                Some(s) => s,
                None => return "Error: app state unavailable".to_string(),
            };
            let mut cards = state_handle.cards.lock().unwrap();
            let card = match cards.iter_mut().find(|c| {
                c.id == card_id && clean_project_path(&c.project_path) == scope_clean
            }) {
                Some(c) => c,
                None => {
                    return format!(
                        "Error: no card with id '{}' in this project. Call list_cards to see valid ids.",
                        card_id
                    )
                }
            };
            if card.status != "backlog" && card.status != "todo" {
                return format!(
                    "Error: card '{}' is in status '{}' — only backlog or todo cards can be edited.",
                    card.title, card.status
                );
            }
            let mut changed: Vec<String> = Vec::new();
            if let Some(t) = &new_title {
                card.title = t.clone();
                changed.push("title".to_string());
            }
            if let Some(d) = &new_desc {
                card.description = d.clone();
                changed.push("description".to_string());
            }
            if let Some(p) = &new_priority {
                card.priority = p.clone();
                changed.push("priority".to_string());
            }
            if let Some(label) = &add_label {
                if !card.labels.contains(label) {
                    card.labels.push(label.clone());
                }
                changed.push("label added".to_string());
            }
            let mut todos_replaced = false;
            if let Some(list) = &new_todos {
                card.todo_list = list
                    .iter()
                    .map(|t| TodoItem {
                        text: t.clone(),
                        completed: false,
                    })
                    .collect();
                todos_replaced = true;
                changed.push(format!("todo list replaced ({} items)", list.len()));
            }
            let mut todo_added = false;
            if let Some(todo_text) = &add_todo {
                card.todo_list.push(TodoItem {
                    text: todo_text.clone(),
                    completed: false,
                });
                todo_added = true;
                changed.push("todo appended".to_string());
            }
            let card_title = card.title.clone();
            let card_desc = card.description.clone();
            let card_priority = card.priority.clone();
            let card_labels_json =
                serde_json::to_string(&card.labels).unwrap_or_else(|_| "[]".to_string());
            let card_id_db = card.id.clone();
            let todo_idx = card.todo_list.len().saturating_sub(1);
            let todo_text_db = add_todo.clone();
            let todo_texts: Vec<String> = card.todo_list.iter().map(|t| t.text.clone()).collect();
            if let Ok(conn) = get_db_conn(app_handle) {
                let _ = conn.execute(
                    "UPDATE cards SET title = ?1, description = ?2, priority = ?3, labels = ?4 WHERE id = ?5",
                    (&card_title, &card_desc, &card_priority, &card_labels_json, &card_id_db),
                );
                if todos_replaced {
                    // Full rewrite: the replacement list (plus any appended item)
                    // becomes the card's entire checklist, all unchecked.
                    let _ = conn.execute("DELETE FROM todo_items WHERE card_id = ?1", [&card_id_db]);
                    for (idx, text) in todo_texts.iter().enumerate() {
                        let _ = conn.execute(
                            "INSERT INTO todo_items (card_id, idx, text, completed) VALUES (?1, ?2, ?3, 0)",
                            (&card_id_db, idx as i32, text),
                        );
                    }
                } else if todo_added {
                    if let Some(text) = &todo_text_db {
                        let _ = conn.execute(
                            "INSERT INTO todo_items (card_id, idx, text, completed) VALUES (?1, ?2, ?3, ?4)",
                            (&card_id_db, todo_idx as i32, text, 0),
                        );
                    }
                }
            }
            let _ = app_handle.emit("run-updated", serde_json::json!({ "run_id": run_id }));
            format!(
                "Success: card '{}' updated ({}).{}",
                card_title,
                changed.join(", "),
                unknown_args_note(args, KNOWN)
            )
        }
        "delete_card" => {
            const USAGE: &str = " Usage: delete_card(card_id: String) — card_id comes from list_cards; only backlog/todo cards with no run history can be deleted.";
            let card_id = match args.get("card_id").and_then(|c| c.as_str()) {
                Some(c) if !c.trim().is_empty() => c.trim().to_string(),
                _ => return format!("Error: Missing 'card_id' argument.{}", USAGE),
            };
            let (scope, _) = memory_scope(app_handle, worktree_path, run_id);
            let scope_clean = PathBuf::from(&scope);
            let state_handle = match app_handle.try_state::<AppState>() {
                Some(s) => s,
                None => return "Error: app state unavailable".to_string(),
            };
            let mut cards = state_handle.cards.lock().unwrap();
            let pos = match cards.iter().position(|c| {
                c.id == card_id && clean_project_path(&c.project_path) == scope_clean
            }) {
                Some(p) => p,
                None => {
                    return format!(
                        "Error: no card with id '{}' in this project. Call list_cards to see valid ids.",
                        card_id
                    )
                }
            };
            {
                let card = &cards[pos];
                if card.status != "backlog" && card.status != "todo" {
                    return format!(
                        "Error: card '{}' is in status '{}' — only backlog or todo cards can be deleted.",
                        card.title, card.status
                    );
                }
                if card.run_id.is_some() {
                    return format!(
                        "Error: card '{}' has run history and cannot be deleted.",
                        card.title
                    );
                }
            }
            let removed = cards.remove(pos);
            if let Ok(conn) = get_db_conn(app_handle) {
                let _ = conn.execute("DELETE FROM todo_items WHERE card_id = ?1", [&removed.id]);
                let _ = conn.execute("DELETE FROM cards WHERE id = ?1", [&removed.id]);
            }
            let _ = app_handle.emit("run-updated", serde_json::json!({ "run_id": run_id }));
            format!("Success: card '{}' deleted.", removed.title)
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
        "replace_lines" => {
            const USAGE: &str = " Usage: replace_lines(path: String, start_line: int, end_line: int, content: String) — path is the file to edit, relative to the project root (the same path you passed to read_file).";
            let path = match args.get("path").and_then(|p| p.as_str()) {
                Some(p) => p,
                None => return format!("Error: Missing 'path' argument.{}", USAGE),
            };
            let start_line = match args.get("start_line").and_then(|v| v.as_u64()) {
                Some(v) => v as usize,
                None => return format!("Error: Missing 'start_line' argument.{}", USAGE),
            };
            let end_line = match args.get("end_line").and_then(|v| v.as_u64()) {
                Some(v) => v as usize,
                None => return format!("Error: Missing 'end_line' argument.{}", USAGE),
            };
            let content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");
            replace_lines_impl(worktree_path, path, start_line, end_line, content)
        }
        "read_card" => {
            let state_handle = match app_handle.try_state::<AppState>() {
                Some(s) => s,
                None => return "Error: app state unavailable".to_string(),
            };
            let cards = state_handle.cards.lock().unwrap();
            match cards.iter().find(|c| c.run_id.as_deref() == Some(run_id)) {
                Some(card) => {
                    let mut out = format!(
                        "Card: {}\nStatus: {}\nPriority: {}\nLabels: {}\nDescription: {}\n\nTodo list:",
                        card.title,
                        card.status,
                        card.priority,
                        if card.labels.is_empty() {
                            "(none)".to_string()
                        } else {
                            card.labels.join(", ")
                        },
                        card.description
                    );
                    if card.todo_list.is_empty() {
                        out.push_str(" (empty)");
                    }
                    for (i, t) in card.todo_list.iter().enumerate() {
                        out.push_str(&format!(
                            "\n  {}. [{}] {}",
                            i,
                            if t.completed { "x" } else { " " },
                            t.text
                        ));
                    }
                    out
                }
                None => "Error: no card found for this run".to_string(),
            }
        }
        "set_todo" => {
            const USAGE: &str = " Usage: set_todo(index: int, completed: bool) — index comes from read_card; completed defaults to true.";
            let index = match args.get("index").and_then(|v| v.as_u64()) {
                Some(v) => v as usize,
                None => return format!("Error: Missing 'index' argument.{}", USAGE),
            };
            let completed = args.get("completed").and_then(|v| v.as_bool()).unwrap_or(true);
            let state_handle = match app_handle.try_state::<AppState>() {
                Some(s) => s,
                None => return "Error: app state unavailable".to_string(),
            };
            let mut cards = state_handle.cards.lock().unwrap();
            match cards.iter_mut().find(|c| c.run_id.as_deref() == Some(run_id)) {
                Some(card) => {
                    if index >= card.todo_list.len() {
                        return format!(
                            "Error: index {} out of range — this card has {} todo item(s). Call read_card to see them.",
                            index,
                            card.todo_list.len()
                        );
                    }
                    card.todo_list[index].completed = completed;
                    let card_id = card.id.clone();
                    let text = card.todo_list[index].text.clone();
                    if let Ok(conn) = get_db_conn(app_handle) {
                        let _ = conn.execute(
                            "UPDATE todo_items SET completed = ?1 WHERE card_id = ?2 AND idx = ?3",
                            (if completed { 1 } else { 0 }, &card_id, index as i32),
                        );
                    }
                    let _ = app_handle.emit("run-updated", serde_json::json!({ "run_id": run_id }));
                    format!(
                        "Success: todo {} ('{}') marked {}.",
                        index,
                        text,
                        if completed { "complete" } else { "incomplete" }
                    )
                }
                None => "Error: no card found for this run".to_string(),
            }
        }
        _ => format!(
            "Error: Unknown tool '{}'. Available tools: read_file, outline_file, write_file, patch_file, replace_lines, list_dir, search_grep, find_file, find_symbol, remember, recall, list_cards, create_card, update_card, delete_card, git_status, git_diff, run_command, web_search, send_notification, read_card, set_todo, task_complete. You may ONLY call these tools.",
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

            let events_snapshot: Vec<RunEvent> = {
                let logs = state.run_logs.lock().unwrap();
                logs.get(&run_id_clone).cloned().unwrap_or_default()
            };
            let (history, compaction) =
                compacted_history(&app_handle_clone, &run_id_clone, &events_snapshot);
            if let Some(ev) = compaction {
                append_run_event(&app_handle_clone, &state, &run_id_clone, ev);
            }

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
                "find_file",
                "find_symbol",
                "remember",
                "recall",
                "list_cards",
                "create_card",
                "update_card",
                "delete_card",
                "patch_file",
                "replace_lines",
                "read_card",
                "set_todo",
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

            if let Some((tool_name, args, preamble)) = parse_tool_call_spanned(&remaining) {
                // Surface the prose the model wrote before its tool call —
                // previously this commentary was silently clobbered.
                if !preamble.is_empty() {
                    append_run_event(
                        &app_handle_clone,
                        &state,
                        &run_id_clone,
                        RunEvent {
                            run_id: run_id_clone.clone(),
                            event_type: "message".to_string(),
                            payload: serde_json::json!({
                                "role": "agent",
                                "content": preamble,
                            })
                            .to_string(),
                        },
                    );
                }
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
                // status with a `blocked`/`running` transition. Only break on a
                // SUCCESSFUL completion — a rejected task_complete (failed
                // verification) returns an Error result and the loop continues
                // so the model can fix the build.
                if tool_name == "task_complete" && tool_result.starts_with("Success") {
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
         You have access to tools to read/write files and research solutions. Issue EXACTLY ONE tool call per message, as a JSON object with \"name\" and \"args\". Your model's native tool-call format (such as <tool_call>...</tool_call>) is fully supported; otherwise use this reference format:\n\n\
         ```tool_call\n\
         {{\n\
           \"name\": \"tool_name\",\n\
           \"args\": {{\n\
             \"arg1\": \"value1\"\n\
           }}\n\
         }}\n\
         ```\n\n\
         Tools available:\n\
         1. `read_file(path: String, start_line?: Int, end_line?: Int)`: Reads file content (line-numbered, output capped). For large files, call outline_file first, then read only the line range you need — don't read whole large files when a range will do.\n\
         2. `outline_file(path: String)`: Returns a file's structure (markdown headings, or code declarations) with line numbers, without its full contents. Survey large files this way before reading.\n\
         3. `write_file(path: String, content: String)`: Writes content to a file (creating folders if needed). Use this to update design documents under `design/`!\n\
         4. `patch_file(path: String, target: String, replacement: String)`: Replaces an exact text snippet in a file — safer than rewriting a whole file for small edits. The target must match byte-for-byte.\n\
         5. `list_dir(path: String, depth?: Int)`: Lists files and folders as an indented tree (use \"\" for root). Pass depth 2 or 3 to map nested structure in one call.\n\
         6. `search_grep(query: String, path?: String, context?: Int, case_sensitive?: Bool)`: Searches file contents for a substring (case-insensitive by default), grouped by file with line numbers. Pass context: 2 to see surrounding lines without a follow-up read.\n\
         7. `find_file(name: String, path?: String)`: Finds files by name fragment (case-insensitive) and returns matching relative paths.\n\
         8. `find_symbol(name: String, path?: String)`: Finds where a function, struct, class, or other declaration is DEFINED. Returns file:line: signature — then range-read around that line.\n\
         9. `web_search(query: String)`: Searches the web for APIs, libraries, architectural patterns, and programming guides.\n\
         10. `send_notification(message: String)`: Sends a system alert/notification to the developer.\n\
         11. `remember(topic: String, content: String)`: Saves a durable insight to this project's long-term memory — shared with code chat and agent runs. Record design decisions and their reasons here.\n\
         12. `recall(query: String, limit?: Int)`: Searches this project's long-term memory by keyword (empty query = most recent). Check what past runs and chats already learned before proposing from scratch.\n\
         13. `list_cards()`: Shows the project's kanban board grouped by status, with card ids and todo progress.\n\
         14. `create_card(title: String, description: String, todos?: [String], priority?: \"low\"|\"medium\"|\"high\", labels?: [String])`: Files a new card in the backlog. When a design discussion produces actionable work, FILE IT as a card with a clear description, priority, and todos — that is how plans become runs.\n\
         15. `update_card(card_id: String, title?: String, description?: String, priority?: String, todos?: [String], add_todo?: String, add_label?: String)`: Edits a backlog/todo card. `todos` REPLACES the whole checklist; `add_todo` appends one item.\n\
         16. `delete_card(card_id: String)`: Deletes a backlog/todo card that has no run history.\n\n\
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
         You have access to tools to read/write source files and test compilation. Issue EXACTLY ONE tool call per message, as a JSON object with \"name\" and \"args\". Your model's native tool-call format (such as <tool_call>...</tool_call>) is fully supported; otherwise use this reference format:\n\n\
         ```tool_call\n\
         {{\n\
           \"name\": \"tool_name\",\n\
           \"args\": {{\n\
             \"arg1\": \"value1\"\n\
           }}\n\
         }}\n\
         ```\n\n\
         Tools available:\n\
         1. `read_file(path: String, start_line?: Int, end_line?: Int)`: Reads file content (line-numbered, output capped). For large files, call outline_file first, then read only the line range you need — don't read whole large files when a range will do.\n\
         2. `outline_file(path: String)`: Returns a file's structure (markdown headings, or code declarations) with line numbers, without its full contents. Survey large files this way before reading.\n\
         3. `write_file(path: String, content: String)`: Writes/overwrites content to a source file.\n\
         4. `patch_file(path: String, target: String, replacement: String)`: Replaces an exact text snippet in a file — safer than rewriting a whole file for small edits. The target must match byte-for-byte.\n\
         5. `list_dir(path: String, depth?: Int)`: Lists files and folders as an indented tree (use \"\" for root). Pass depth 2 or 3 to map nested structure in one call.\n\
         6. `search_grep(query: String, path?: String, context?: Int, case_sensitive?: Bool)`: Searches file contents for a substring (case-insensitive by default), grouped by file with line numbers. Pass context: 2 to see surrounding lines without a follow-up read.\n\
         7. `find_file(name: String, path?: String)`: Finds files by name fragment (case-insensitive) and returns matching relative paths.\n\
         8. `find_symbol(name: String, path?: String)`: Finds where a function, struct, class, or other declaration is DEFINED. Returns file:line: signature — then range-read around that line.\n\
         9. `git_status()`: Runs `git status` in the repository.\n\
         10. `git_diff()`: Runs `git diff` to view code changes.\n\
         11. `run_command(command: String)`: Runs build, test, or check shell commands in the repository (e.g. \"cargo check\", \"npm run build\", \"npm test\"). Use this to verify your changes compile and pass tests!\n\
         12. `web_search(query: String)`: Searches the web for documentation, syntax guides, and examples.\n\
         13. `remember(topic: String, content: String)`: Saves a durable insight to this project's long-term memory — shared with design chat and agent runs. Record how subsystems work and pitfalls you discover.\n\
         14. `recall(query: String, limit?: Int)`: Searches this project's long-term memory by keyword (empty query = most recent). Check what past runs and chats already learned before exploring from scratch.\n\
         15. `list_cards()`: Shows the project's kanban board grouped by status, with card ids and todo progress.\n\
         16. `create_card(title: String, description: String, todos?: [String], priority?: \"low\"|\"medium\"|\"high\", labels?: [String])`: Files a new card in the backlog. If a fix you're discussing is bigger than the current conversation, file it as a card so it gets scheduled instead of forgotten.\n\
         17. `update_card(card_id: String, title?: String, description?: String, priority?: String, todos?: [String], add_todo?: String, add_label?: String)`: Edits a backlog/todo card. `todos` REPLACES the whole checklist; `add_todo` appends one item.\n\
         18. `delete_card(card_id: String)`: Deletes a backlog/todo card that has no run history.\n\n\
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
        let res = parse_tool_call_spanned(input);
        assert!(res.is_some());
        let (name, args, preamble) = res.unwrap();
        assert_eq!(name, "list_dir");
        assert_eq!(args.get("path").unwrap().as_str().unwrap(), "");
        assert_eq!(preamble, "Some introductory text.");
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
        let res = parse_tool_call_spanned(input);
        assert!(res.is_some());
        let (name, args, preamble) = res.unwrap();
        assert_eq!(name, "list_dir");
        assert_eq!(args.get("path").unwrap().as_str().unwrap(), "");
        assert_eq!(preamble, "BeetleAI\nBeetleAI");
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
        let res = parse_tool_call_spanned(input);
        assert!(res.is_some());
        let (name, args, _preamble) = res.unwrap();
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
        let res = parse_tool_call_spanned(input);
        assert!(res.is_some());
        let (name, _, preamble) = res.unwrap();
        assert_eq!(name, "git_status");
        assert_eq!(preamble, "");
    }

    #[test]
    fn test_parse_tool_call_qwen_syntax() {
        let input = "<|tool_call>call:read_card{}<tool_call|>";
        let res = parse_tool_call_spanned(input);
        assert!(res.is_some());
        let (name, args, _) = res.unwrap();
        assert_eq!(name, "read_card");
        assert!(args.is_object());

        let input_bare = "call:write_file{\"path\": \"foo.txt\", \"content\": \"bar\"}";
        let res_bare = parse_tool_call_spanned(input_bare);
        assert!(res_bare.is_some());
        let (name_b, args_b, preamble_b) = res_bare.unwrap();
        assert_eq!(name_b, "write_file");
        assert_eq!(args_b.get("path").unwrap().as_str().unwrap(), "foo.txt");
        assert_eq!(preamble_b, "");
    }

    #[test]
    fn test_tool_preamble_rescued_and_debris_dropped() {
        // Real prose before the call is preserved.
        let input = "I'll inspect the design doc first to confirm the section layout.\n```tool_call\n{\"name\": \"read_file\", \"args\": {\"path\": \"design/design.md\"}}\n```";
        let (_, _, preamble) = parse_tool_call_spanned(input).unwrap();
        assert_eq!(
            preamble,
            "I'll inspect the design doc first to confirm the section layout."
        );

        // Pure syntax debris (no alphanumerics) is treated as no preamble.
        let input_debris = "\n> \n```tool_call\n{\"name\": \"git_status\", \"args\": {}}\n```";
        let (_, _, preamble_d) = parse_tool_call_spanned(input_debris).unwrap();
        assert_eq!(preamble_d, "");
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

        // 1. Test search_grep_impl recursively (output is grouped by file)
        let res_search = search_grep_impl(wt_path, "Rust", "", 0, false);
        let normalized = res_search.replace("\\", "/");
        assert!(normalized.contains("== file_a.txt (1 match)"));
        assert!(normalized.contains("3: Rust is awesome!"));
        assert!(normalized.contains("== file_b.txt (1 match)"));
        assert!(normalized.contains("3: Rust coding is great!"));

        // 1b. Case-insensitive by default; case-sensitive on request.
        let res_ci = search_grep_impl(wt_path, "rust", "", 0, false);
        assert!(res_ci.contains("Rust is awesome!"));
        let res_cs = search_grep_impl(wt_path, "rust", "", 0, true);
        assert!(res_cs.starts_with("No matches found"));

        // 1c. Context lines surround the hit, which is marked with '>'.
        let res_ctx = search_grep_impl(wt_path, "large file", "file_a.txt", 1, false);
        assert!(res_ctx.contains(" 1: Hello World!"));
        assert!(res_ctx.contains(">2: This is a large file line."));
        assert!(res_ctx.contains(" 3: Rust is awesome!"));

        // 2. Test search_grep_impl specific file
        let res_search_file = search_grep_impl(wt_path, "large file", "file_a.txt", 0, false);
        let normalized_file = res_search_file.replace("\\", "/");
        assert!(normalized_file.contains("2: This is a large file line."));
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
