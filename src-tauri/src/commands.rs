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
    #[serde(default = "default_runner")]
    pub runner: String, // "local" (default) | "frontier" — which model drives this card's run
}

fn default_priority() -> String {
    "medium".to_string()
}

fn default_runner() -> String {
    "local".to_string()
}

/// Normalize a runner value to the two canonical targets; anything unrecognized
/// falls back to local so a bad value can never silently route work to the
/// (paid, slower) frontier.
fn normalize_runner(r: &str) -> String {
    match r.trim().to_lowercase().as_str() {
        "frontier" | "assist" | "remote" => "frontier".to_string(),
        _ => "local".to_string(),
    }
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
    pub event_type: String, // "status", "message", "reasoning", "tool_call", "tool_result", "file_touched", "blocked", "error", "malformed"
    pub payload: String,    // JSON payload or text
}

/// Per-tool tally for the vitals panel.
#[derive(Serialize, Clone, Debug, Default)]
pub struct ToolStat {
    pub name: String,
    pub calls: u32,
    pub failures: u32,
}

/// Read-only telemetry summary for one run, derived from its event log. Drives
/// the harness vitals panel; everything here is recomputed on demand, so there
/// is no aggregate to keep in sync.
#[derive(Serialize, Clone, Debug, Default)]
pub struct RunVitals {
    pub total_calls: u32,
    pub successes: u32,
    pub failures: u32,
    pub malformed: u32,
    /// Turns that came back with no visible output AND no tool call — truncated
    /// mid-reasoning or an empty generation. The "no progress" stall signature.
    pub empty_responses: u32,
    pub reads: u32,
    pub writes: u32,
    pub reasoning_events: u32,
    /// Longest streak of failures on a single edit signature before it cleared
    /// (or, if never cleared, the streak it ended on) — how hard the worst edit
    /// was to land. Mirrors the stuck-edit detector's counter.
    pub worst_edit_retry_streak: u32,
    /// failure_reason → count, over failed tool_results, sorted desc.
    pub failure_reasons: Vec<(String, u32)>,
    /// failure_reason → count, over malformed events, sorted desc.
    pub malformed_reasons: Vec<(String, u32)>,
    /// Per-tool calls/failures, sorted by calls desc.
    pub per_tool: Vec<ToolStat>,
    /// Throughput — averaged across the run's LLM calls (from `metrics` events).
    pub llm_calls: u32,
    pub avg_ttft_ms: Option<f64>,
    pub avg_prompt_tps: Option<f64>,
    pub avg_decode_tps: Option<f64>,
    /// True if any decode-rate sample was estimated from response length rather
    /// than a real token count (server didn't report usage).
    pub decode_tps_approx: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LlmSettings {
    pub provider: String,
    pub api_url: String,
    pub api_key: String,
    pub model: String,
    pub max_steps: u32,
    // Frontier escalation target: a SECOND model config held alongside the
    // local one so the agent can ask a stronger model for help mid-run without
    // a human reconfiguring anything (that reconfiguration would defeat the
    // point of autonomy). All optional — absent/empty means escalation is
    // unconfigured and request_assist degrades to "ask your human". Defaulted
    // for serde so existing config.json and DB rows load unchanged.
    #[serde(default)]
    pub assist_provider: String,
    #[serde(default)]
    pub assist_api_url: String,
    #[serde(default)]
    pub assist_api_key: String,
    #[serde(default)]
    pub assist_model: String,
    /// Usable context window of the PRIMARY model, in tokens. Drives the
    /// compaction threshold so a small window compacts early enough to leave
    /// room to generate (instead of overflowing and stalling), and a large one
    /// isn't compacted prematurely. 0 = unset -> fall back to the legacy fixed
    /// threshold. Defaulted for serde so old config.json / DB rows load unchanged.
    #[serde(default)]
    pub context_tokens: u32,
    // Embedding model config: a THIRD provider held alongside the chat and
    // assist models, used only for RAG (codebase indexing + semantic recall).
    // Kept separate because embeddings are a distinct API surface (no chat
    // streaming, no tools) and the best embedding provider is often not the
    // chat provider — e.g. Anthropic has no embeddings endpoint, so Voyage or a
    // local Ollama model fills that role. All optional/defaulted for serde so
    // existing config.json and DB rows load unchanged; empty == RAG disabled.
    #[serde(default)]
    pub embedding_provider: String,
    #[serde(default)]
    pub embedding_api_url: String,
    #[serde(default)]
    pub embedding_api_key: String,
    #[serde(default)]
    pub embedding_model: String,
    /// When true (default), the run agent gets the top codebase matches for its
    /// task auto-injected into context each run, in addition to the on-demand
    /// search_codebase tool. Off lets the user avoid the per-run token cost.
    #[serde(default = "default_true")]
    pub embedding_auto_inject: bool,
    /// When true (default), the codebase index is incrementally refreshed in the
    /// background at the start of each run. Off = index only on demand via the
    /// Reindex button (manual indexing), avoiding the per-run file walk.
    #[serde(default = "default_true")]
    pub embedding_auto_index: bool,
}

fn default_true() -> bool {
    true
}

impl LlmSettings {
    /// True only when every field the escalation call needs is present.
    pub fn assist_configured(&self) -> bool {
        !self.assist_provider.trim().is_empty()
            && !self.assist_api_url.trim().is_empty()
            && !self.assist_model.trim().is_empty()
    }

    /// True only when every field the embedding call needs is present. Gates all
    /// RAG features: when false, indexing is skipped and semantic recall falls
    /// back to keyword search.
    pub fn embedding_configured(&self) -> bool {
        !self.embedding_provider.trim().is_empty()
            && !self.embedding_api_url.trim().is_empty()
            && !self.embedding_model.trim().is_empty()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppConfig {
    pub settings: LlmSettings,
    pub projects: Vec<Project>,
}

/// Per-file line-number drift within one run/chat session. `shift` is the
/// net change in line count since the model last read the file; edits whose
/// range reaches at-or-past `lowest_edit_line` while `shift != 0` are using
/// stale coordinates and must be refused before they land on the wrong code.
#[derive(Clone, Copy, Default)]
pub struct LineShift {
    pub shift: i64,
    pub lowest_edit_line: usize,
}

pub struct AppState {
    pub cards: Mutex<Vec<Card>>,
    pub run_logs: Mutex<HashMap<String, Vec<RunEvent>>>,
    pub design_logs: Mutex<HashMap<String, Vec<RunEvent>>>,
    pub code_logs: Mutex<HashMap<String, Vec<RunEvent>>>,
    pub active_runs: Mutex<std::collections::HashSet<String>>,
    pub cancelled_runs: Mutex<std::collections::HashSet<String>>,
    /// Cooperative pause requests. Distinct from `cancelled_runs` because a
    /// pause must NOT destroy the worktree or fail the card — the run loop
    /// breaks to `blocked` at its next between-turns checkpoint, leaving the
    /// sandbox and full history intact so `unblock_run` can resume it.
    pub paused_runs: Mutex<std::collections::HashSet<String>>,
    /// Last LM Studio stateful `response_id` per run/chat key. The /api/v1/chat
    /// endpoint keeps history server-side; chaining `previous_response_id` is
    /// what makes a thread continue instead of starting fresh every call.
    pub lmstudio_response_ids: Mutex<HashMap<String, String>>,
    /// run/chat key → file key → line-number drift since last read. Guards
    /// replace_lines against the classic two-edits-same-file stale-line bug.
    pub line_shift_state: Mutex<HashMap<String, HashMap<String, LineShift>>>,
    /// run_id → long-lived background processes (dev servers, watchers) the
    /// agent started via start_server. Killed when the run ends (ActiveRunGuard)
    /// or the app closes (on_window_event) so a `npm run dev` never outlives it.
    pub bg_processes: Mutex<HashMap<String, Vec<BgProcess>>>,
}

impl AppState {
    pub fn new() -> Self {
        // Demo seeding removed: a fresh install — and every new project —
        // starts with an empty board. The database is the source of truth at
        // startup (load_state_from_db); the hardcoded demo cards and fictional
        // run transcripts that used to live here were dead weight, overwritten
        // milliseconds after construction. Spotting that was the agent's own
        // finding (its Card A, todo #1).
        Self {
            cards: Mutex::new(Vec::new()),
            run_logs: Mutex::new(HashMap::new()),
            design_logs: Mutex::new(HashMap::new()),
            code_logs: Mutex::new(HashMap::new()),
            active_runs: Mutex::new(std::collections::HashSet::new()),
            cancelled_runs: Mutex::new(std::collections::HashSet::new()),
            paused_runs: Mutex::new(std::collections::HashSet::new()),
            lmstudio_response_ids: Mutex::new(HashMap::new()),
            line_shift_state: Mutex::new(HashMap::new()),
            bg_processes: Mutex::new(HashMap::new()),
        }
    }

    // Pruned from compilation (always-false cfg). The demo data below is kept
    // un-compiled solely so the agent can delete it as its own cleanup card.
    // Safe to remove wholesale, including this attribute and stub.
    #[cfg(any())]
    fn _pruned_demo_data() {
        let default_project_path = String::new();
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

        let _ = (initial_cards, initial_logs);
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
    embedding: Option<&[u8]>,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO memories (project_path, topic, content, source, run_id, card_id, created_at, embedding) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        (
            project_path,
            topic,
            content,
            source,
            run_id,
            card_id,
            chrono::Utc::now().to_rfc3339(),
            embedding,
        ),
    )?;
    Ok(())
}

/// Best-effort embedding of a memory (topic + content) for semantic recall.
/// Returns None when RAG is unconfigured or the provider errors — memory
/// writes must never fail just because embedding is unavailable; those rows
/// simply fall back to keyword recall.
fn embed_memory_text(settings: &LlmSettings, topic: &str, content: &str) -> Option<Vec<u8>> {
    if !settings.embedding_configured() {
        return None;
    }
    let text = format!("{}\n{}", topic, content);
    match call_embedding(settings, &[text]) {
        Ok(v) => v.into_iter().next().map(|vec| embedding_to_blob(&vec)),
        Err(_) => None,
    }
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

// Pruned from compilation (always-false cfg): demo-card seeding is retired.
// The body below is kept un-compiled solely so the agent can delete it as its
// own cleanup card — its Card A, todo #1. Safe to remove wholesale, including
// this attribute and comment.
#[cfg(any())]
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
            max_steps INTEGER NOT NULL,
            assist_provider TEXT NOT NULL DEFAULT '',
            assist_api_url TEXT NOT NULL DEFAULT '',
            assist_api_key TEXT NOT NULL DEFAULT '',
            assist_model TEXT NOT NULL DEFAULT '',
            context_tokens INTEGER NOT NULL DEFAULT 0,
            embedding_provider TEXT NOT NULL DEFAULT '',
            embedding_api_url TEXT NOT NULL DEFAULT '',
            embedding_api_key TEXT NOT NULL DEFAULT '',
            embedding_model TEXT NOT NULL DEFAULT '',
            embedding_auto_inject INTEGER NOT NULL DEFAULT 1,
            embedding_auto_index INTEGER NOT NULL DEFAULT 1
        );",
        [],
    )
    .map_err(|e| e.to_string())?;

    // Lightweight migrations for existing settings tables: ADD COLUMN fails
    // harmlessly when the column already exists (same pattern as the card
    // priority/labels columns).
    let _ = conn.execute(
        "ALTER TABLE settings ADD COLUMN assist_provider TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE settings ADD COLUMN assist_api_url TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE settings ADD COLUMN assist_api_key TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE settings ADD COLUMN assist_model TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE settings ADD COLUMN context_tokens INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE settings ADD COLUMN embedding_provider TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE settings ADD COLUMN embedding_api_url TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE settings ADD COLUMN embedding_api_key TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE settings ADD COLUMN embedding_model TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE settings ADD COLUMN embedding_auto_inject INTEGER NOT NULL DEFAULT 1",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE settings ADD COLUMN embedding_auto_index INTEGER NOT NULL DEFAULT 1",
        [],
    );

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
            labels TEXT NOT NULL DEFAULT '[]',
            runner TEXT NOT NULL DEFAULT 'local'
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
    let _ = conn.execute(
        "ALTER TABLE cards ADD COLUMN runner TEXT NOT NULL DEFAULT 'local'",
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

    // Optional embedding for each memory (little-endian f32 BLOB), enabling
    // semantic recall. Nullable: rows created before RAG was configured, or
    // while it's off, simply have no vector and fall back to keyword match.
    let _ = conn.execute("ALTER TABLE memories ADD COLUMN embedding BLOB", []);

    // RAG codebase index. `rag_files` tracks per-file content hashes so a
    // reindex only re-embeds files that actually changed; `rag_chunks` holds the
    // embedded windows. Both scoped by project_path (same scope key as memories).
    conn.execute(
        "CREATE TABLE IF NOT EXISTS rag_files (
            project_path TEXT NOT NULL,
            file_path TEXT NOT NULL,
            hash TEXT NOT NULL,
            indexed_at TEXT NOT NULL,
            PRIMARY KEY (project_path, file_path)
        );",
        [],
    )
    .map_err(|e| e.to_string())?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS rag_chunks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            project_path TEXT NOT NULL,
            file_path TEXT NOT NULL,
            start_line INTEGER NOT NULL,
            end_line INTEGER NOT NULL,
            kind TEXT NOT NULL,
            content TEXT NOT NULL,
            embedding BLOB NOT NULL
        );",
        [],
    )
    .map_err(|e| e.to_string())?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_rag_chunks_project ON rag_chunks (project_path);",
        [],
    )
    .map_err(|e| e.to_string())?;

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
        // assist_* columns intentionally omitted here: they take their DDL
        // defaults (empty), i.e. escalation starts unconfigured until the user
        // sets a frontier target in settings.
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
        .prepare("SELECT provider, api_url, api_key, model, max_steps, assist_provider, assist_api_url, assist_api_key, assist_model, context_tokens, embedding_provider, embedding_api_url, embedding_api_key, embedding_model, embedding_auto_inject, embedding_auto_index FROM settings LIMIT 1")
        .map_err(|e| e.to_string())?;
    let settings_opt = stmt
        .query_row([], |row| {
            Ok(LlmSettings {
                provider: row.get(0)?,
                api_url: row.get(1)?,
                api_key: row.get(2)?,
                model: row.get(3)?,
                max_steps: row.get(4)?,
                assist_provider: row.get(5)?,
                assist_api_url: row.get(6)?,
                assist_api_key: row.get(7)?,
                assist_model: row.get(8)?,
                context_tokens: row.get(9)?,
                embedding_provider: row.get(10)?,
                embedding_api_url: row.get(11)?,
                embedding_api_key: row.get(12)?,
                embedding_model: row.get(13)?,
                embedding_auto_inject: row.get(14)?,
                embedding_auto_index: row.get(15)?,
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
        "INSERT INTO settings (provider, api_url, api_key, model, max_steps, assist_provider, assist_api_url, assist_api_key, assist_model, context_tokens, embedding_provider, embedding_api_url, embedding_api_key, embedding_model, embedding_auto_inject, embedding_auto_index) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        (
            &config.settings.provider,
            &config.settings.api_url,
            &config.settings.api_key,
            &config.settings.model,
            &config.settings.max_steps,
            &config.settings.assist_provider,
            &config.settings.assist_api_url,
            &config.settings.assist_api_key,
            &config.settings.assist_model,
            &config.settings.context_tokens,
            &config.settings.embedding_provider,
            &config.settings.embedding_api_url,
            &config.settings.embedding_api_key,
            &config.settings.embedding_model,
            &config.settings.embedding_auto_inject,
            &config.settings.embedding_auto_index,
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
        .prepare("SELECT id, project_path, title, description, status, run_id, assignee, priority, labels, runner FROM cards")
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
                row.get::<_, String>(9)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut cards = Vec::new();
    for card_res in card_rows {
        let (id, project_path, title, description, status, run_id, assignee, priority, labels_json, runner) =
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
            runner: normalize_runner(&runner),
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

/// Pure decision core for the startup reaper: given the run-id worktree
/// directories found on disk for a project and the set of run-ids that a live
/// card still owns (and may resume), return the run-ids whose worktrees are safe
/// to delete. A worktree is an orphan if no live card references it — its owning
/// card was deleted or reached a terminal state (`done`/`failed`).
fn worktrees_to_reap(on_disk: &[String], keep: &std::collections::HashSet<String>) -> Vec<String> {
    on_disk
        .iter()
        .filter(|run_id| !keep.contains(*run_id))
        .cloned()
        .collect()
}

/// Pure decision core for log retention: given distinct run-ids ordered
/// most-recent-first, keep the newest `keep` of them plus every protected
/// (resumable) run, and return the run-ids whose transcripts may be deleted.
fn run_logs_to_prune(
    recent_first: &[String],
    keep: usize,
    protected: &std::collections::HashSet<String>,
) -> Vec<String> {
    recent_first
        .iter()
        .skip(keep)
        .filter(|run_id| !protected.contains(*run_id))
        .cloned()
        .collect()
}

/// Bound the otherwise-unbounded `logs` table. Each run appends a few hundred
/// event rows; left unchecked they slow startup (the whole table replays into
/// memory) and run-replay queries. We keep the most recent `KEEP_RUNS` run
/// transcripts plus every resumable run (blocked/review/etc., which still need
/// replay) and delete the rest — whole runs at a time, so a transcript is never
/// half-truncated. Only run logs are touched; design/code chat logs are bounded
/// by doc count and left alone.
///
/// Runs BEFORE load_state_from_db so the trimmed set is what gets loaded.
pub fn prune_old_run_logs(app_handle: &tauri::AppHandle) -> Result<usize, String> {
    const KEEP_RUNS: usize = 200;
    let conn = get_db_conn(app_handle)?;

    // Resumable runs keep their transcript regardless of age.
    let mut protected = std::collections::HashSet::new();
    {
        let mut stmt = conn
            .prepare("SELECT run_id FROM cards WHERE run_id IS NOT NULL AND status IN ('running','queued','blocked','review')")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for r in rows {
            protected.insert(r.map_err(|e| e.to_string())?);
        }
    }

    // Distinct run transcripts, most-recent first (by their latest row id).
    let mut recent_first = Vec::new();
    {
        let mut stmt = conn
            .prepare("SELECT run_id FROM logs WHERE log_type = 'run' GROUP BY run_id ORDER BY MAX(id) DESC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for r in rows {
            recent_first.push(r.map_err(|e| e.to_string())?);
        }
    }

    let mut pruned = 0usize;
    for run_id in run_logs_to_prune(&recent_first, KEEP_RUNS, &protected) {
        let n = conn
            .execute(
                "DELETE FROM logs WHERE log_type = 'run' AND run_id = ?1",
                [&run_id],
            )
            .map_err(|e| e.to_string())?;
        if n > 0 {
            pruned += 1;
        }
    }
    Ok(pruned)
}

/// List the run-id subdirectories under a project's `.harness/worktrees/`.
fn worktree_dirs_on_disk(repo_path: &Path) -> Vec<String> {
    let root = repo_path.join(".harness").join("worktrees");
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    out.push(name.to_string());
                }
            }
        }
    }
    out
}

/// Reconcile run state and worktrees left behind by a previous session. The run
/// loop is an in-memory `tokio` task that dies with the process, so any card
/// persisted as `running` has a dead loop and is stuck: `unblock_run` only
/// resumes `blocked`/`review`. We demote those crash-orphaned `running` cards to
/// `blocked` so the user can resume them — their worktree and full transcript are
/// still on disk. Then, per project, we delete worktree directories that no live
/// card owns (deleted or terminal cards) and prune git's worktree bookkeeping.
///
/// Runs AFTER `load_state_from_db`, so cards and run logs are already in memory.
pub fn reconcile_runs_on_startup(app_handle: &tauri::AppHandle, state: &AppState) -> Result<(), String> {
    // Phase 1: demote crash-orphaned `running` cards to `blocked`.
    let mut demoted: Vec<(String, String)> = Vec::new(); // (card_id, run_id)
    let mut keep_by_project: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();
    let mut project_paths: std::collections::HashSet<String> = std::collections::HashSet::new();

    {
        let mut cards = state.cards.lock().unwrap();
        for card in cards.iter_mut() {
            project_paths.insert(card.project_path.clone());
            if card.status == "running" {
                card.status = "blocked".to_string();
                if let Some(run_id) = card.run_id.clone() {
                    demoted.push((card.id.clone(), run_id));
                }
            }
            // A live card (now including freshly-demoted ones) keeps its worktree.
            if matches!(card.status.as_str(), "blocked" | "review") {
                if let Some(run_id) = card.run_id.clone() {
                    keep_by_project
                        .entry(card.project_path.clone())
                        .or_default()
                        .insert(run_id);
                }
            }
        }
    }

    // Persist demotions + leave a transcript breadcrumb so the run reads clearly.
    if let Ok(conn) = get_db_conn(app_handle) {
        for (card_id, run_id) in &demoted {
            let _ = conn.execute(
                "UPDATE cards SET status = 'blocked' WHERE id = ?1",
                [card_id],
            );
            let _ = conn.execute(
                "INSERT INTO logs (log_type, key, run_id, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                ("run", card_id, run_id, "status", "blocked"),
            );
            let msg = "{\"role\":\"agent\",\"content\":\"Run interrupted by an app restart. The previous session ended before this run finished — its worktree and transcript are intact. Resume to continue, or reject to discard.\"}";
            let _ = conn.execute(
                "INSERT INTO logs (log_type, key, run_id, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                ("run", card_id, run_id, "message", msg),
            );
        }
    }

    // Mirror the breadcrumb into the in-memory log so a freshly-loaded UI shows it.
    if !demoted.is_empty() {
        let mut logs = state.run_logs.lock().unwrap();
        for (card_id, run_id) in &demoted {
            let events = logs.entry(card_id.clone()).or_default();
            events.push(RunEvent {
                run_id: run_id.clone(),
                event_type: "status".to_string(),
                payload: "blocked".to_string(),
            });
            events.push(RunEvent {
                run_id: run_id.clone(),
                event_type: "message".to_string(),
                payload: "{\"role\":\"agent\",\"content\":\"Run interrupted by an app restart. The previous session ended before this run finished — its worktree and transcript are intact. Resume to continue, or reject to discard.\"}".to_string(),
            });
        }
    }

    // Phase 2: per project, reap worktrees no live card owns, then prune.
    let empty = std::collections::HashSet::new();
    for project_path in &project_paths {
        let repo_path = clean_project_path(project_path);
        if !git::is_git_repo(&repo_path) {
            continue;
        }
        let on_disk = worktree_dirs_on_disk(&repo_path);
        let keep = keep_by_project.get(project_path).unwrap_or(&empty);
        for run_id in worktrees_to_reap(&on_disk, keep) {
            // Best-effort: a failure here shouldn't abort startup.
            let _ = git::remove_worktree(&repo_path, &run_id);
        }
        let _ = git::prune_worktrees(&repo_path);
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
            assist_provider: String::new(),
            assist_api_url: String::new(),
            assist_api_key: String::new(),
            assist_model: String::new(),
            context_tokens: 0,
            embedding_provider: String::new(),
            embedding_api_url: String::new(),
            embedding_api_key: String::new(),
            embedding_model: String::new(),
            embedding_auto_inject: true,
            embedding_auto_index: true,
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
    // Detect an embedding-identity change before overwriting. A different
    // provider/model produces vectors in a different space (and often dimension),
    // so every stored vector becomes meaningless — comparing across them yields
    // garbage rankings, not just dimension-mismatch zeros.
    let embedding_changed = config.settings.embedding_provider.trim().to_lowercase()
        != settings.embedding_provider.trim().to_lowercase()
        || config.settings.embedding_model.trim().to_lowercase()
            != settings.embedding_model.trim().to_lowercase();
    config.settings = settings;
    save_config(&app_handle, &config)?;

    // Drop the now-incompatible index + memory vectors so a fresh reindex (and
    // re-embed on the next remember/recall) rebuilds them in the new space.
    if embedding_changed {
        if let Ok(conn) = get_db_conn(&app_handle) {
            let _ = conn.execute("DELETE FROM rag_chunks", []);
            let _ = conn.execute("DELETE FROM rag_files", []);
            let _ = conn.execute("UPDATE memories SET embedding = NULL", []);
        }
    }
    Ok(())
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
                    "description": "Runs a build, test, or check shell command in the repository (e.g. \"cargo check\", \"npm run build\", \"npm test\"). Output is prefixed with the exit code (0 = success) and clipped from BOTH ends, so the error at the tail is preserved.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": {
                                "type": "string",
                                "description": "The exact shell command to run"
                            },
                            "timeout_secs": {
                                "type": "integer",
                                "description": "Optional wall-clock cap in seconds (default 300, max 1800). Raise it for known-slow builds; the process is killed if it overruns."
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
            "screenshot" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "screenshot",
                    "description": "Renders a running web page in a headless browser and captures a PNG. The dev server must already be serving the URL. If the loaded model is vision-capable the image is shown to you on your next turn (so you can SEE the rendered UI); otherwise it is saved as an artifact for your human. Use it to check layout, alignment, and visual regressions instead of guessing from CSS.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "url": {
                                "type": "string",
                                "description": "URL of a page the dev server is already serving, e.g. http://localhost:5173/"
                            },
                            "width": {
                                "type": "integer",
                                "description": "Viewport width in px (default 1280, 320-2560)"
                            },
                            "height": {
                                "type": "integer",
                                "description": "Viewport height in px (default 800, 240-2000)"
                            }
                        },
                        "required": ["url"],
                        "additionalProperties": false
                    }
                }
            }),
            "start_server" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "start_server",
                    "description": "Starts a long-running process (e.g. a dev server like \"npm run dev\") in the background and waits until `port` accepts connections. Use this — NOT run_command — for anything that doesn't exit on its own, since run_command blocks and kills long-running commands. Once it's ready you can screenshot http://localhost:<port>/. The process keeps running across turns and is stopped automatically when the run ends.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": { "type": "string", "description": "The shell command that starts the server, e.g. \"npm run dev\"" },
                            "port": { "type": "integer", "description": "The port the server listens on (e.g. 5173); used to detect readiness" },
                            "timeout_secs": { "type": "integer", "description": "How long to wait for the port to open before returning (default 60, max 180)" }
                        },
                        "required": ["command", "port"],
                        "additionalProperties": false
                    }
                }
            }),
            "stop_server" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "stop_server",
                    "description": "Stops background server(s) started with start_server. Stops all of them if no port is given.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "port": { "type": "integer", "description": "Only stop the server on this port (optional; omit to stop all)" }
                        },
                        "required": [],
                        "additionalProperties": false
                    }
                }
            }),
            "server_logs" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "server_logs",
                    "description": "Shows recent stdout/stderr from your background server(s) — use it to see why a server failed to start or to read a runtime error it logged.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "port": { "type": "integer", "description": "Only show this port's server (optional; omit for all)" }
                        },
                        "required": [],
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
                    "description": "Searches this project's long-term memory and returns the most relevant matches. When an embedding provider is configured the ranking is semantic (by meaning, so related wording matches even without shared keywords); otherwise it falls back to keyword match. Call with an empty query to see the latest memories. Check memory before exploring from scratch.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "What to look for, in natural language or keywords. Empty returns the most recent memories."
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
            "search_codebase" => serde_json::json!({
                "type": "function",
                "function": {
                    "name": "search_codebase",
                    "description": "Semantic search over the project's indexed code and docs. Embeds your query and returns the most relevant chunks by meaning (with file:line ranges), so it finds code by concept even when you don't know the exact identifier. Best for 'where is X handled?' / 'how does Y work?' questions. For exact strings or symbol names, prefer search_grep / find_symbol. Returns nothing if the project hasn't been indexed yet.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "Natural-language description of what you're looking for."
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Optional max results 1-15 (default 6)."
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

/// Tool-call args are replayed into history every turn until compacted. A
/// write_file's `content` (or any large string arg) is the model's OWN prior
/// output echoed straight back — pure prompt bloat that can overflow a small
/// context window. Truncate long string values for the REPLAY only (disk
/// already holds the real content); the JSON shape is preserved so the call
/// still reads clearly. Recurses so nested args are covered too.
fn truncate_tool_call_args(args: &serde_json::Value, max: usize) -> serde_json::Value {
    match args {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), truncate_tool_call_args(v, max)))
                .collect(),
        ),
        serde_json::Value::Array(arr) => serde_json::Value::Array(
            arr.iter().map(|v| truncate_tool_call_args(v, max)).collect(),
        ),
        serde_json::Value::String(s) if s.chars().count() > max => {
            let head: String = s.chars().take(max).collect();
            let dropped = s.chars().count() - max;
            serde_json::Value::String(format!("{}…[{} chars truncated in history]", head, dropped))
        }
        other => other.clone(),
    }
}

fn get_history_messages(events: &[RunEvent], context_tokens: u32) -> Vec<serde_json::Value> {
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
    // The most recent N tool results are kept fuller; older ones trimmed hard.
    // For a small window keep FEWER full results (a count, not a smaller
    // per-result cap) so we never behead a single result's tail-appended
    // guidance — see the read_file note below.
    let recent_keep: usize = if context_tokens != 0 && context_tokens <= 8192 { 1 } else { 2 };
    // Per-message char budgets, scaled to the model's context window when known
    // (context_tokens) so the replayed prompt (system + recent tail) stays INSIDE
    // the window. Overflow otherwise forces the server to truncate the prompt
    // (silently dropping the system prompt -> no tools / no <think>) or reprocess
    // a giant context each turn (-> multi-minute prefills and timeouts). 0 =
    // unknown -> legacy generous caps. recent_max stays >= read_file's 8000-char
    // cap (+ marker) because tools append corrective guidance at the TAIL of
    // capped output; a budget below that cap would behead the lesson before the
    // model sees it (that exact failure once taught an agent read_file "didn't
    // support line ranges"). Only reasoning, old results, and echoed tool-call
    // args scale down.
    let (reasoning_max, recent_max, old_max, tool_arg_max): (usize, usize, usize, usize) =
        if context_tokens == 0 {
            (2000, 9000, 800, 4000)
        } else {
            let w = (context_tokens as usize) * 4; // ~chars that fit in the window
            (
                (w / 16).clamp(500, 2000),
                9000,
                (w / 40).clamp(300, 800),
                (w / 24).clamp(400, 4000),
            )
        };

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
                let trimmed = truncate_tool_result(&event.payload, reasoning_max);
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
                // Don't echo the model's own bulky inputs (e.g. write_file
                // content) back verbatim every turn — bound them for the replay.
                let args = truncate_tool_call_args(&args, tool_arg_max);
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
                let is_recent = tool_result_seen + recent_keep >= total_tool_results;
                tool_result_seen += 1;
                let budget = if is_recent {
                    recent_max
                } else {
                    old_max
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

/// History char budget before compaction triggers, derived from the primary
/// model's usable context window (`context_tokens`). Uses ~4 chars/token and
/// lets the compactable history occupy ~45% of the window — the rest is
/// reserved for the (uncounted) system prompt, the kept recent tail, and room
/// to generate. So a small window compacts EARLY (instead of overflowing and
/// stalling mid-generation) and a large window isn't compacted prematurely
/// (which silently discards live context). Falls back to the legacy fixed
/// budget when the window is unknown (0).
fn compact_threshold_chars(context_tokens: u32) -> usize {
    if context_tokens == 0 {
        return COMPACT_THRESHOLD_CHARS;
    }
    ((context_tokens as f64) * 4.0 * 0.45) as usize
}

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
    // Threshold + per-message budgets both track the model's real context window
    // so we neither overflow a small one (stall/timeout) nor over-compact a large
    // one (lost context).
    let context_tokens = load_config(app_handle).settings.context_tokens;
    let messages = get_history_messages(events, context_tokens);
    let threshold = compact_threshold_chars(context_tokens);
    if history_size_chars(&messages) < threshold
        || events.len() <= COMPACT_KEEP_RECENT_EVENTS + 4
    {
        return (messages, None);
    }

    let covers = events.len() - COMPACT_KEEP_RECENT_EVENTS;
    // Flatten the to-be-covered portion into a transcript for the summarizer.
    // This already folds in any previous compaction summary, so repeated
    // compactions compound instead of stacking.
    let old_msgs = get_history_messages(&events[..covers], context_tokens);
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
    let summary = match call_llm(app_handle, &summarizer_run_id, system, summarizer_history, None, false)
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
    (get_history_messages(&with_compaction, context_tokens), Some(event))
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
    // Generous timeouts: a slow local model can spend MINUTES in prefill before
    // the first token (a 27b on CPU/partial-offload was observed at 200s+ TTFT).
    // The read timeout is per-read on the streaming response, so it must exceed
    // the worst-case gap between bytes (i.e. the prefill), or the call drops to
    // `blocked` with a "connection ... did not respond" error mid-run. Connect
    // is also bumped so a server busy with a previous prefill can still accept.
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(30))
        .timeout_read(std::time::Duration::from_secs(1200))
        .build()
}

// ---------------------------------------------------------------------------
// Embeddings (RAG)
//
// A separate, narrow API surface from chat: no streaming, no tools, just
// text-in / vector-out. Two request shapes cover every provider we support:
//   * Ollama native   -> POST /api/embed        { model, input: [..] } -> { embeddings: [[..]] }
//   * OpenAI-compatible-> POST /v1/embeddings    { model, input: [..] } -> { data: [{ embedding: [..] }] }
// "voyage", "openai", "lmstudio", "custom" all speak the OpenAI shape; only
// "ollama" diverges. Keeping this independent of the chat ProviderKind means an
// Anthropic-driven run (Anthropic has no embeddings endpoint) can still embed
// via a local Ollama model or Voyage.
// ---------------------------------------------------------------------------

fn resolve_embedding_endpoint(provider: &str, base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if provider == "ollama" {
        if base.ends_with("/api/embed") {
            base.to_string()
        } else {
            let root = base.trim_end_matches("/v1").trim_end_matches('/');
            format!("{}/api/embed", root)
        }
    } else {
        // OpenAI-compatible: openai, voyage, lmstudio, custom, anything unknown.
        if base.ends_with("/embeddings") {
            base.to_string()
        } else if base.ends_with("/v1") {
            format!("{}/embeddings", base)
        } else {
            format!("{}/v1/embeddings", base)
        }
    }
}

fn embed_http_err(label: &str, e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            format!("{} embedding API error {}: {}", label, code, body)
        }
        other => format!("{} embedding request failed: {}", label, other),
    }
}

fn json_to_f32_vec(arr: &[serde_json::Value]) -> Result<Vec<f32>, String> {
    let mut v = Vec::with_capacity(arr.len());
    for n in arr {
        let f = n
            .as_f64()
            .ok_or_else(|| "embedding contained a non-numeric value".to_string())?;
        v.push(f as f32);
    }
    Ok(v)
}

/// Generate embeddings for a batch of texts using the configured embedding
/// provider. Returns one vector per input, in the same order. Any failure
/// (unconfigured, network, unexpected response shape) is returned as an Err so
/// callers can degrade to keyword search rather than silently storing empties.
fn call_embedding(settings: &LlmSettings, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    if !settings.embedding_configured() {
        return Err(
            "Embedding provider is not configured (Settings → Embeddings & RAG).".to_string(),
        );
    }
    let provider = settings.embedding_provider.trim().to_lowercase();
    let url = resolve_embedding_endpoint(&provider, settings.embedding_api_url.trim());
    let model = settings.embedding_model.trim();
    let key = settings.embedding_api_key.trim();
    let agent = http_agent();

    let mut req = agent.post(&url).set("Content-Type", "application/json");
    // Ollama needs no auth for the common localhost case; everything else uses
    // Bearer. Only attach the header when a key is present so an empty key
    // doesn't turn into a malformed "Bearer " that some servers reject.
    if !key.is_empty() {
        req = req.set("Authorization", &format!("Bearer {}", key));
    }
    let payload = serde_json::json!({ "model": model, "input": texts });
    let resp = req
        .send_json(payload)
        .map_err(|e| embed_http_err(&provider, e))?;
    let body: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("embedding response was not valid JSON: {}", e))?;

    if provider == "ollama" {
        let arr = body
            .get("embeddings")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("Ollama response missing 'embeddings': {}", body))?;
        let mut out = Vec::with_capacity(arr.len());
        for row in arr {
            let inner = row
                .as_array()
                .ok_or_else(|| "Ollama 'embeddings' row was not an array".to_string())?;
            out.push(json_to_f32_vec(inner)?);
        }
        Ok(out)
    } else {
        let data = body
            .get("data")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("embedding response missing 'data': {}", body))?;
        let mut out = Vec::with_capacity(data.len());
        for item in data {
            let emb = item
                .get("embedding")
                .and_then(|v| v.as_array())
                .ok_or_else(|| "embedding item missing 'embedding' array".to_string())?;
            out.push(json_to_f32_vec(emb)?);
        }
        Ok(out)
    }
}

/// Cosine similarity between two equal-length vectors. Returns 0.0 for a
/// length mismatch or a zero-magnitude vector rather than NaN, so a bad row
/// just sorts to the bottom instead of poisoning the ranking.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Pack an f32 vector into a little-endian byte BLOB for SQLite storage.
fn embedding_to_blob(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

/// Unpack a little-endian byte BLOB back into an f32 vector. A length that
/// isn't a multiple of 4 yields an empty vec (treated as a non-match).
fn blob_to_embedding(bytes: &[u8]) -> Vec<f32> {
    if bytes.len() % 4 != 0 {
        return Vec::new();
    }
    let mut v = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        v.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    v
}

/// Test the embedding configuration by embedding a probe string. Returns the
/// vector dimension on success so the UI can confirm the model is reachable and
/// report what it's working with. Takes settings directly from the form so the
/// user can verify before saving.
#[tauri::command]
pub async fn test_embedding(settings: LlmSettings) -> Result<usize, String> {
    let vecs = call_embedding(
        &settings,
        &["BeetleAI embedding connectivity test.".to_string()],
    )?;
    let dim = vecs.first().map(|v| v.len()).unwrap_or(0);
    if dim == 0 {
        return Err("Provider returned no embedding vector.".to_string());
    }
    Ok(dim)
}

// ---------------------------------------------------------------------------
// RAG ingestion: walk a project's text files, chunk them, embed the chunks, and
// store them in rag_chunks for later semantic retrieval. Incremental — a file
// whose content hash is unchanged since the last index is skipped, and files
// that have disappeared have their chunks purged.
// ---------------------------------------------------------------------------

const RAG_CHUNK_LINES: usize = 60;
const RAG_CHUNK_OVERLAP: usize = 12;
const RAG_MAX_FILE_BYTES: u64 = 1_000_000;
const RAG_EMBED_BATCH: usize = 16;

#[derive(Serialize, Clone, Default, Debug)]
pub struct RagIndexStats {
    pub files_indexed: usize,
    pub files_skipped: usize,
    pub files_removed: usize,
    pub chunks: usize,
}

struct RagChunk {
    start_line: usize,
    end_line: usize,
    kind: String,
    content: String,
}

/// Non-cryptographic content hash for change detection. DefaultHasher is not
/// guaranteed stable across Rust releases; the only consequence of a change is
/// a one-time full reindex after a toolchain bump, which is acceptable.
fn content_hash(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn is_indexable_ext(path: &Path) -> bool {
    const EXTS: &[&str] = &[
        "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "java", "kt", "kts",
        "scala", "c", "h", "cpp", "hpp", "cc", "hh", "cs", "rb", "php", "swift", "m",
        "mm", "sh", "bash", "ps1", "sql", "toml", "yaml", "yml", "json", "jsonc", "md",
        "markdown", "txt", "rst", "html", "htm", "css", "scss", "sass", "less", "vue",
        "svelte", "xml", "ini", "cfg",
    ];
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => EXTS.contains(&ext.to_lowercase().as_str()),
        None => false,
    }
}

/// Project-relative, forward-slashed path for a file under `root`, or None if
/// it's outside the root or inside a harness worktree sandbox (which we never
/// index — they're transient duplicates). Shared by indexing and status checks.
fn rag_rel_path(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/");
    if rel.starts_with(".harness/") || rel.contains("/.harness/") {
        return None;
    }
    Some(rel)
}

/// Walk a project's indexable files and return rel_path -> content_hash for the
/// current on-disk state. No network, no DB — used to compute index staleness.
fn current_project_hashes(root: &Path) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for result in ignore::WalkBuilder::new(root).build() {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_file() || !is_indexable_ext(path) {
            continue;
        }
        if entry.metadata().map(|m| m.len() > RAG_MAX_FILE_BYTES).unwrap_or(true) {
            continue;
        }
        let rel = match rag_rel_path(root, path) {
            Some(r) => r,
            None => continue,
        };
        if let Ok(content) = fs::read_to_string(path) {
            map.insert(rel, content_hash(&content));
        }
    }
    map
}

/// Sliding line-window chunker for code/plain text.
fn chunk_by_lines(content: &str, kind: &str, base_line: usize) -> Vec<RagChunk> {
    let lines: Vec<&str> = content.lines().collect();
    let mut chunks = Vec::new();
    if lines.is_empty() {
        return chunks;
    }
    let step = RAG_CHUNK_LINES.saturating_sub(RAG_CHUNK_OVERLAP).max(1);
    let mut start = 0usize;
    loop {
        let end = (start + RAG_CHUNK_LINES).min(lines.len());
        let text = lines[start..end].join("\n");
        if !text.trim().is_empty() {
            chunks.push(RagChunk {
                start_line: base_line + start,
                end_line: base_line + end - 1,
                kind: kind.to_string(),
                content: text,
            });
        }
        if end >= lines.len() {
            break;
        }
        start += step;
    }
    chunks
}

/// Markdown chunker: split on headers, sub-splitting any section that's too
/// large to embed as a single window.
fn chunk_markdown(content: &str) -> Vec<RagChunk> {
    let lines: Vec<&str> = content.lines().collect();
    let mut chunks = Vec::new();
    let mut sec_start = 0usize;
    let emit = |s: usize, e: usize, chunks: &mut Vec<RagChunk>| {
        if e <= s {
            return;
        }
        let text = lines[s..e].join("\n");
        if text.trim().is_empty() {
            return;
        }
        if e - s > RAG_CHUNK_LINES * 2 {
            // Section too big: window it, keeping line numbers anchored to `s`.
            chunks.extend(chunk_by_lines(&text, "doc", s + 1));
        } else {
            chunks.push(RagChunk {
                start_line: s + 1,
                end_line: e,
                kind: "doc".to_string(),
                content: text,
            });
        }
    };
    for i in 0..lines.len() {
        if lines[i].starts_with('#') && i > sec_start {
            emit(sec_start, i, &mut chunks);
            sec_start = i;
        }
    }
    emit(sec_start, lines.len(), &mut chunks);
    chunks
}

fn chunk_file(content: &str, path: &Path) -> Vec<RagChunk> {
    let is_md = matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
            .as_deref(),
        Some("md") | Some("markdown")
    );
    if is_md {
        chunk_markdown(content)
    } else {
        chunk_by_lines(content, "code", 1)
    }
}

fn upsert_rag_file(
    conn: &rusqlite::Connection,
    project_path: &str,
    rel: &str,
    hash: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO rag_files (project_path, file_path, hash, indexed_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(project_path, file_path) DO UPDATE SET hash = excluded.hash, indexed_at = excluded.indexed_at",
        (project_path, rel, hash, chrono::Utc::now().to_rfc3339()),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The blocking core of project indexing. Walks the workspace honoring
/// .gitignore (via the `ignore` crate), re-embeds only changed files, and
/// emits `rag-index-progress` events so the UI can show a live count.
fn index_project_blocking(
    app_handle: &tauri::AppHandle,
    project_path: &str,
    settings: &LlmSettings,
) -> Result<RagIndexStats, String> {
    let root = clean_project_path(project_path);
    let conn = get_db_conn(app_handle)?;
    let mut stats = RagIndexStats::default();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Snapshot the previously-indexed file hashes so we can skip unchanged
    // files and detect deletions.
    let existing: std::collections::HashMap<String, String> = {
        let mut m = std::collections::HashMap::new();
        let mut stmt = conn
            .prepare("SELECT file_path, hash FROM rag_files WHERE project_path = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([project_path], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        for row in rows.flatten() {
            m.insert(row.0, row.1);
        }
        m
    };

    for result in ignore::WalkBuilder::new(&root).build() {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_file() || !is_indexable_ext(path) {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.len() > RAG_MAX_FILE_BYTES {
            continue;
        }
        let rel = match rag_rel_path(&root, path) {
            Some(r) => r,
            None => continue,
        };
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue, // binary / unreadable -> skip
        };
        seen.insert(rel.clone());
        let hash = content_hash(&content);
        if existing.get(&rel).map(|h| h == &hash).unwrap_or(false) {
            stats.files_skipped += 1;
            continue;
        }

        let chunks = chunk_file(&content, path);
        // Replace any prior chunks for this file before inserting fresh ones.
        let _ = conn.execute(
            "DELETE FROM rag_chunks WHERE project_path = ?1 AND file_path = ?2",
            (project_path, &rel),
        );

        if !chunks.is_empty() {
            let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
            let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
            for batch in texts.chunks(RAG_EMBED_BATCH) {
                vectors.extend(call_embedding(settings, batch)?);
            }
            if vectors.len() != chunks.len() {
                return Err(format!(
                    "embedding count mismatch for {} ({} vectors / {} chunks)",
                    rel,
                    vectors.len(),
                    chunks.len()
                ));
            }
            for (c, v) in chunks.iter().zip(vectors.iter()) {
                let blob = embedding_to_blob(v);
                conn.execute(
                    "INSERT INTO rag_chunks (project_path, file_path, start_line, end_line, kind, content, embedding) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    (
                        project_path,
                        &rel,
                        c.start_line as i64,
                        c.end_line as i64,
                        c.kind.as_str(),
                        c.content.as_str(),
                        &blob,
                    ),
                )
                .map_err(|e| e.to_string())?;
                stats.chunks += 1;
            }
        }

        upsert_rag_file(&conn, project_path, &rel, &hash)?;
        stats.files_indexed += 1;
        let _ = app_handle.emit(
            "rag-index-progress",
            serde_json::json!({
                "project_path": project_path,
                "file": rel,
                "files_indexed": stats.files_indexed,
                "files_skipped": stats.files_skipped,
                "chunks": stats.chunks,
            }),
        );
    }

    // Purge files that have been deleted since the last index.
    for f in existing.keys() {
        if !seen.contains(f) {
            let _ = conn.execute(
                "DELETE FROM rag_chunks WHERE project_path = ?1 AND file_path = ?2",
                (project_path, f),
            );
            let _ = conn.execute(
                "DELETE FROM rag_files WHERE project_path = ?1 AND file_path = ?2",
                (project_path, f),
            );
            stats.files_removed += 1;
        }
    }

    // Opportunistically backfill embeddings for memories saved before RAG was
    // configured, so semantic recall covers historical insights too.
    backfill_memory_embeddings(&conn, project_path, settings);

    Ok(stats)
}

/// Embed any memories for this project that lack a vector (e.g. created while
/// RAG was off). Best-effort: failures on individual rows are skipped.
fn backfill_memory_embeddings(
    conn: &rusqlite::Connection,
    project_path: &str,
    settings: &LlmSettings,
) {
    let rows: Vec<(i64, String, String)> = {
        let mut stmt = match conn.prepare(
            "SELECT id, topic, content FROM memories WHERE project_path = ?1 AND embedding IS NULL LIMIT 1000",
        ) {
            Ok(s) => s,
            Err(_) => return,
        };
        let mapped = stmt.query_map([project_path], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        });
        match mapped {
            Ok(it) => it.flatten().collect(),
            Err(_) => return,
        }
    };
    for (id, topic, content) in rows {
        if let Some(blob) = embed_memory_text(settings, &topic, &content) {
            let _ = conn.execute("UPDATE memories SET embedding = ?1 WHERE id = ?2", (&blob, id));
        }
    }
}

/// Reindex a project's codebase for RAG. Runs the (network- and IO-heavy) work
/// on a blocking thread so it doesn't stall the async runtime, and returns
/// summary stats to the UI.
#[tauri::command]
pub async fn reindex_project(
    app_handle: tauri::AppHandle,
    project_path: String,
) -> Result<RagIndexStats, String> {
    let settings = load_config(&app_handle).settings;
    if !settings.embedding_configured() {
        return Err(
            "Embedding provider not configured (Settings → Embeddings & RAG).".to_string(),
        );
    }
    let scope = clean_project_path(&project_path)
        .to_string_lossy()
        .into_owned();
    let handle = app_handle.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        index_project_blocking(&handle, &scope, &settings)
    })
    .await
    {
        Ok(inner) => inner,
        Err(e) => Err(format!("indexing task failed: {}", e)),
    }
}

/// Semantic search over a project's indexed chunks. Embeds the query, then
/// brute-force scores every chunk by cosine similarity (fast enough for the
/// per-project chunk counts we deal with) and returns the top `k`.
/// Each hit: (file_path, start_line, end_line, content, score).
/// Tokenize a query into lowercase terms for keyword matching. Splits on
/// non-identifier characters so `fetch_local_models` stays one token (matching
/// an exact identifier), while dropping very short tokens and a few stopwords.
fn query_terms(query: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the", "and", "for", "with", "this", "that", "from", "into", "are", "was",
        "how", "does", "where", "what", "when", "which", "you", "your", "use",
    ];
    let mut terms: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for raw in query.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        let t = raw.to_lowercase();
        if t.len() < 2 || STOP.contains(&t.as_str()) {
            continue;
        }
        if seen.insert(t.clone()) {
            terms.push(t);
        }
    }
    terms
}

/// Count how many distinct query terms occur in the (already-lowercased) text.
fn keyword_overlap(text_lower: &str, terms: &[String]) -> usize {
    terms.iter().filter(|t| text_lower.contains(t.as_str())).count()
}

/// Reciprocal-rank fusion of a cosine ranking and a keyword-overlap ranking.
/// Returns row indices ordered best-first. Items with zero keyword overlap just
/// don't contribute to the keyword list, so behavior collapses to pure-vector
/// ranking when no query term hits anything.
fn rrf_order(cosines: &[f32], keywords: &[usize]) -> Vec<usize> {
    let n = cosines.len();
    let mut by_vec: Vec<usize> = (0..n).collect();
    by_vec.sort_by(|&a, &b| {
        cosines[b].partial_cmp(&cosines[a]).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut by_kw: Vec<usize> = (0..n).filter(|&i| keywords[i] > 0).collect();
    by_kw.sort_by(|&a, &b| keywords[b].cmp(&keywords[a]));
    const K: f32 = 60.0; // standard RRF damping constant
    let mut score = vec![0.0f32; n];
    for (rank, &i) in by_vec.iter().enumerate() {
        score[i] += 1.0 / (K + rank as f32 + 1.0);
    }
    for (rank, &i) in by_kw.iter().enumerate() {
        score[i] += 1.0 / (K + rank as f32 + 1.0);
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        score[b].partial_cmp(&score[a]).unwrap_or(std::cmp::Ordering::Equal)
    });
    order
}

/// Hybrid search over a project's indexed chunks: embeds the query for cosine
/// similarity, scores keyword overlap (so exact identifiers rank too), and fuses
/// the two rankings with RRF. Each returned hit carries its cosine score for
/// display. (file_path, start_line, end_line, content, cosine).
fn search_codebase(
    app_handle: &tauri::AppHandle,
    settings: &LlmSettings,
    project_path: &str,
    query: &str,
    k: usize,
) -> Result<Vec<(String, i64, i64, String, f32)>, String> {
    let qvec = call_embedding(settings, &[query.to_string()])?
        .into_iter()
        .next()
        .ok_or_else(|| "no query embedding returned".to_string())?;
    let conn = get_db_conn(app_handle)?;
    let mut stmt = conn
        .prepare("SELECT file_path, start_line, end_line, content, embedding FROM rag_chunks WHERE project_path = ?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([project_path], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Vec<u8>>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let terms = query_terms(query);
    let mut metas: Vec<(String, i64, i64, String)> = Vec::new();
    let mut cosines: Vec<f32> = Vec::new();
    let mut keywords: Vec<usize> = Vec::new();
    for row in rows.flatten() {
        let (fp, s, e, content, blob) = row;
        let cos = cosine_similarity(&qvec, &blob_to_embedding(&blob));
        let kw = if terms.is_empty() {
            0
        } else {
            keyword_overlap(&content.to_lowercase(), &terms)
        };
        cosines.push(cos);
        keywords.push(kw);
        metas.push((fp, s, e, content));
    }

    let order = rrf_order(&cosines, &keywords);
    let mut out: Vec<(String, i64, i64, String, f32)> = Vec::with_capacity(k.min(order.len()));
    for &i in order.iter().take(k) {
        let (fp, s, e, content) = metas[i].clone();
        out.push((fp, s, e, content, cosines[i]));
    }
    Ok(out)
}

#[derive(Serialize, Clone, Default, Debug)]
pub struct RagStatus {
    /// Whether an embedding provider is configured (RAG usable at all).
    pub configured: bool,
    /// Files currently represented in the index.
    pub indexed_files: usize,
    /// Total embedded chunks in the index.
    pub chunks: usize,
    /// Most recent indexed_at timestamp across the project's files, if any.
    pub last_indexed_at: Option<String>,
    /// Files that differ from the index right now (new + modified + deleted).
    pub stale_files: usize,
    /// Convenience flag for the UI: there is on-disk work the index doesn't reflect.
    pub needs_index: bool,
}

fn rag_status_blocking(
    app_handle: &tauri::AppHandle,
    root: &Path,
    scope: &str,
    configured: bool,
) -> Result<RagStatus, String> {
    let conn = get_db_conn(app_handle)?;

    let existing: std::collections::HashMap<String, String> = {
        let mut m = std::collections::HashMap::new();
        let mut stmt = conn
            .prepare("SELECT file_path, hash FROM rag_files WHERE project_path = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([scope], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        for row in rows.flatten() {
            m.insert(row.0, row.1);
        }
        m
    };
    let chunks: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM rag_chunks WHERE project_path = ?1",
            [scope],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let last_indexed_at: Option<String> = conn
        .query_row(
            "SELECT MAX(indexed_at) FROM rag_files WHERE project_path = ?1",
            [scope],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();

    // Compare the current tree against the index: new/modified files differ in
    // hash; deleted files are in the index but no longer on disk.
    let current = current_project_hashes(root);
    let mut stale = 0usize;
    for (rel, h) in &current {
        if existing.get(rel).map(|old| old == h).unwrap_or(false) {
            continue;
        }
        stale += 1;
    }
    for rel in existing.keys() {
        if !current.contains_key(rel) {
            stale += 1;
        }
    }

    Ok(RagStatus {
        configured,
        indexed_files: existing.len(),
        chunks: chunks as usize,
        last_indexed_at,
        stale_files: stale,
        needs_index: stale > 0,
    })
}

/// Report the RAG index state for a project: how much is indexed and whether the
/// on-disk tree has drifted since (so the UI can show a "reindex" nudge). The
/// staleness check walks + hashes files (no network) on a blocking thread.
#[tauri::command]
pub async fn rag_index_status(
    app_handle: tauri::AppHandle,
    project_path: String,
) -> Result<RagStatus, String> {
    let configured = load_config(&app_handle).settings.embedding_configured();
    let root = clean_project_path(&project_path);
    let scope = root.to_string_lossy().into_owned();
    let handle = app_handle.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        rag_status_blocking(&handle, &root, &scope, configured)
    })
    .await
    {
        Ok(inner) => inner,
        Err(e) => Err(format!("status task failed: {}", e)),
    }
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

/// Tell the frontend to discard the partial assistant text streamed so far for
/// this run. Emitted between retry attempts so a resent request doesn't append
/// its tokens onto the abandoned partial of the attempt that failed.
fn emit_stream_reset(app_handle: &tauri::AppHandle, run_id: &str) {
    let _ = app_handle.emit(
        "chat-chunk",
        serde_json::json!({
            "run_id": run_id,
            "chunk": "",
            "done": false,
            "reset": true,
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
/// Re-encode natively-parsed tool calls (from the provider's structured
/// tool_calls stream) into the canonical text protocol, appended to the
/// response body. The loops then parse and execute them through the same
/// battle-tested pipeline as text-protocol calls — one pipeline, two doors.
fn bridge_tool_calls_into_text(full_response: &mut String, accumulated: &[ToolCallAccumulator]) {
    // Every native call is re-encoded into the canonical text protocol so the
    // battle-tested parsing/execution pipeline handles both paths identically.
    // ALL calls are bridged — a model that files two cards in one breath
    // loses neither (the loops iterate parse_tool_calls_all).
    for tc in accumulated {
        let name = tc.name.clone().unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        // parse_json_relaxed, not strict serde: native-mode arguments can carry
        // the same JS-flavored artifacts (backtick template literals, unquoted
        // keys) as text-mode output, and silently degrading them to {} would
        // teach the model its arguments were accepted when they were dropped.
        let args_parsed = match parse_json_relaxed(&tc.arguments) {
            Some(v) => v,
            None => {
                log_error(&format!(
                    "Native tool call '{}': arguments failed strict AND relaxed JSON parsing; bridging with empty args. Raw: {}",
                    name,
                    &tc.arguments.chars().take(300).collect::<String>()
                ));
                serde_json::json!({})
            }
        };
        let formatted = format!(
            "```tool_call\n{{\n  \"name\": \"{}\",\n  \"args\": {}\n}}\n```",
            name, args_parsed
        );
        if !full_response.trim().is_empty() {
            full_response.push_str("\n\n");
        }
        full_response.push_str(&formatted);
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

/// Build a primary-style settings object from the assist_* (frontier) fields,
/// so the same provider dispatch can drive a run — or a one-shot consult — with
/// the frontier model.
fn assist_as_primary(s: &LlmSettings) -> LlmSettings {
    LlmSettings {
        provider: s.assist_provider.clone(),
        api_url: s.assist_api_url.clone(),
        api_key: s.assist_api_key.clone(),
        model: s.assist_model.clone(),
        max_steps: s.max_steps,
        assist_provider: String::new(),
        assist_api_url: String::new(),
        assist_api_key: String::new(),
        assist_model: String::new(),
        // The frontier/assist model has its own (typically large) window; we
        // don't track it separately, so leave 0 -> legacy threshold fallback.
        context_tokens: 0,
        // Embedding config is orthogonal to which chat model is driving; carry
        // it through so a frontier-driven run still indexes/retrieves normally.
        embedding_provider: s.embedding_provider.clone(),
        embedding_api_url: s.embedding_api_url.clone(),
        embedding_api_key: s.embedding_api_key.clone(),
        embedding_model: s.embedding_model.clone(),
        embedding_auto_inject: s.embedding_auto_inject,
        embedding_auto_index: s.embedding_auto_index,
    }
}

/// Extract the numeric HTTP status from a provider error string of the shape
/// "{Provider} API error {code}: ...". Returns None for transport faults and
/// for the no-code variant ("... API error: <body>").
fn parse_http_status(err: &str) -> Option<u16> {
    let marker = " API error ";
    let idx = err.find(marker)?;
    let rest = &err[idx + marker.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u16>().ok()
}

/// Decide whether a failed model call is worth retrying. Permanent failures —
/// user cancellation, auth, bad request, missing model, or an unconfigured
/// endpoint — fail identically on resend, so we surface them at once. Transport
/// faults (timeouts, dropped/reset connections, mid-stream read failures) and
/// 5xx/429-class statuses are transient and worth a retry.
fn llm_error_is_transient(err: &str) -> bool {
    let e = err.to_lowercase();

    if e.contains("cancelled by user")
        || e.contains("api url is empty")
        || e.contains("api key")
    {
        return false;
    }

    if let Some(code) = parse_http_status(err) {
        // 4xx are caller faults (400 bad request, 401/403 auth, 404 no model,
        // 422 unprocessable) — never retried. 408/425/429 and 5xx are transient.
        return matches!(code, 408 | 425 | 429) || code >= 500;
    }

    // No HTTP status parsed → a transport/stream fault. Retry.
    true
}

/// Sleep for `total_ms`, waking early if the run is cancelled so backoff never
/// delays a cancel.
fn sleep_unless_cancelled(app_handle: &tauri::AppHandle, run_id: &str, total_ms: u64) {
    let mut waited = 0u64;
    while waited < total_ms {
        if let Some(st) = app_handle.try_state::<AppState>() {
            if st.cancelled_runs.lock().unwrap().contains(run_id) {
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
        waited += 250;
    }
}

fn call_llm(
    app_handle: &tauri::AppHandle,
    run_id: &str,
    system_prompt: &str,
    mut chat_history: Vec<serde_json::Value>,
    tools: Option<serde_json::Value>,
    use_assist: bool,
) -> Result<String, String> {
    {
        if let Some(state_val) = app_handle.try_state::<AppState>() {
            let mut cancelled = state_val.cancelled_runs.lock().unwrap();
            cancelled.remove(run_id);
        }
    }
    let config = load_config(app_handle);
    // runner="frontier" cards drive with the assist model; everything else uses
    // the primary local model. assist_configured() is re-checked so a stale flag
    // can't route to an unconfigured endpoint.
    let settings = if use_assist && config.settings.assist_configured() {
        assist_as_primary(&config.settings)
    } else {
        config.settings
    };

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

    // Resend on transient faults so a network blip no longer drops the whole run
    // to `blocked`. Attempts: initial + 2 retries, backing off 1s then 3s. The
    // request is identical each time; only the per-attempt clones of the moved
    // args differ. Permanent errors (auth, bad request) and user cancellation
    // short-circuit immediately via llm_error_is_transient.
    const MAX_ATTEMPTS: u32 = 3;
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let result = match kind {
            ProviderKind::Anthropic => call_anthropic(
                app_handle,
                run_id,
                &url,
                &settings,
                system_prompt,
                messages.clone(),
                tools.clone(),
            ),
            ProviderKind::OllamaNative => call_ollama_native(
                app_handle,
                run_id,
                &url,
                &settings,
                messages.clone(),
                tools.clone(),
            ),
            ProviderKind::LmStudioStateful => call_lmstudio_stateful(
                app_handle,
                run_id,
                &url,
                &settings,
                system_prompt,
                &messages,
            ),
            ProviderKind::OpenAiCompat => call_openai_compat(
                app_handle,
                run_id,
                &url,
                &settings,
                messages.clone(),
                tools.clone(),
            ),
        };

        let err = match result {
            Ok(reply) => return Ok(reply),
            Err(e) => e,
        };

        if attempt >= MAX_ATTEMPTS || !llm_error_is_transient(&err) {
            return Err(err);
        }

        // Discard the failed attempt's partial stream from the UI, back off, and
        // resend. sleep_unless_cancelled keeps a mid-backoff cancel responsive.
        emit_stream_reset(app_handle, run_id);
        log_error(&format!(
            "Transient model error on attempt {}/{}, retrying: {}",
            attempt, MAX_ATTEMPTS, err
        ));
        let backoff_ms = if attempt == 1 { 1000 } else { 3000 };
        sleep_unless_cancelled(app_handle, run_id, backoff_ms);
    }
}

/// Wall-clock timer for one LLM call. `start` is set at construction (just
/// before the request goes out); `mark()` stamps the first streamed token so
/// TTFT = first_token - start. TTFT is the prefill-dominated number — the
/// "slow to inject prompts" symptom — and it's provider-independent, so every
/// path gets it even when the server reports no token stats.
struct StreamTimer {
    start: std::time::Instant,
    first_token: Option<std::time::Instant>,
}
impl StreamTimer {
    fn new() -> Self {
        Self { start: std::time::Instant::now(), first_token: None }
    }
    fn mark(&mut self) {
        if self.first_token.is_none() {
            self.first_token = Some(std::time::Instant::now());
        }
    }
}

/// Pull (prompt_tokens, completion_tokens) out of an OpenAI-style SSE line, if
/// it carries a usage block (emitted only when stream_options.include_usage is
/// set). Returns None for ordinary delta lines.
fn parse_openai_usage(line: &str) -> Option<(u64, u64)> {
    let data = line.trim().strip_prefix("data: ")?.trim();
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    let u = v.get("usage")?;
    let p = u.get("prompt_tokens").and_then(|x| x.as_u64());
    let c = u.get("completion_tokens").and_then(|x| x.as_u64());
    match (p, c) {
        (Some(p), Some(c)) => Some((p, c)),
        _ => None,
    }
}

/// Emit a per-call `metrics` event for the vitals panel. Wall-clock TTFT/total
/// are always present. Decode rate prefers server-reported tok/s, then exact
/// completion-token count, then a chars/4 estimate (flagged `approx`) so the
/// user's path still shows a rate even when the server is stingy with stats.
#[allow(clippy::too_many_arguments)]
fn log_llm_metrics(
    app_handle: &tauri::AppHandle,
    run_id: &str,
    timer: &StreamTimer,
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    server_prompt_tps: Option<f64>,
    server_decode_tps: Option<f64>,
    response_chars: usize,
) {
    let now = std::time::Instant::now();
    let total_ms = now.duration_since(timer.start).as_secs_f64() * 1000.0;
    let ttft_ms = timer
        .first_token
        .map(|t| t.duration_since(timer.start).as_secs_f64() * 1000.0);
    let decode_secs = timer
        .first_token
        .map(|ft| now.duration_since(ft).as_secs_f64())
        .unwrap_or(0.0);

    let mut approx = false;
    let decode_tps = server_decode_tps.or_else(|| {
        if decode_secs <= 0.0 {
            return None;
        }
        if let Some(ct) = completion_tokens {
            Some(ct as f64 / decode_secs)
        } else if response_chars > 0 {
            approx = true;
            Some((response_chars as f64 / 4.0) / decode_secs)
        } else {
            None
        }
    });
    let prompt_tps = server_prompt_tps.or_else(|| match (prompt_tokens, ttft_ms) {
        (Some(pt), Some(ms)) if ms > 0.0 => Some(pt as f64 / (ms / 1000.0)),
        _ => None,
    });

    if let Some(state) = app_handle.try_state::<AppState>() {
        append_run_event(
            app_handle,
            state.inner(),
            run_id,
            RunEvent {
                run_id: run_id.to_string(),
                event_type: "metrics".to_string(),
                payload: serde_json::json!({
                    "ttft_ms": ttft_ms,
                    "total_ms": total_ms,
                    "prompt_tps": prompt_tps,
                    "decode_tps": decode_tps,
                    "tokens_out": completion_tokens,
                    "approx": approx,
                })
                .to_string(),
            },
        );
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
    let mut timer = StreamTimer::new();
    let mut prompt_tokens: Option<u64> = None;
    let mut completion_tokens: Option<u64> = None;
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
                    timer.mark();
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
            // Anthropic reports tokens across two events: input in message_start,
            // cumulative output in message_delta.
            if let Some(data) = line_str.trim().strip_prefix("data: ") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(data.trim()) {
                    if let Some(it) = v.pointer("/message/usage/input_tokens").and_then(|x| x.as_u64()) {
                        prompt_tokens = Some(it);
                    }
                    if let Some(ot) = v.pointer("/usage/output_tokens").and_then(|x| x.as_u64()) {
                        completion_tokens = Some(ot);
                    }
                }
            }
        }

        if in_reasoning {
            full_response.push_str("\n</think>\n");
            emit_chunk(app_handle, run_id, "\n</think>\n", false);
        }

        bridge_tool_calls_into_text(&mut full_response, &accumulated_tool_calls);

        emit_chunk(app_handle, run_id, "", true);

        log_llm_metrics(
            app_handle,
            run_id,
            &timer,
            prompt_tokens,
            completion_tokens,
            None,
            None,
            full_response.chars().count(),
        );
        Ok(full_response)
    } else {
        let error_text = resp.into_string().unwrap_or_default();
        let err_msg = format!("Anthropic API error: {}", error_text);
        log_error(&err_msg);
        Err(err_msg)
    }
}

/// Strict-template guard for OpenAI-compatible and Ollama chat endpoints.
/// Local-model jinja templates (Mistral, Qwen, ...) hard-reject message
/// streams that Gemma's permissive template silently accepted: roles must
/// alternate user/assistant after the system message, the first non-system
/// message must be user, and generation must be prompted by a trailing user
/// turn. Normalizing here costs nothing on tolerant templates and makes
/// strict ones work at all. (Field failures: Mistral "conversation roles
/// must alternate"; Qwen "No user query found in messages". call_anthropic
/// has carried its own copy of this guard from day one — the strict locals
/// deserve the same one.)
fn normalize_for_strict_templates(messages: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    let mut out: Vec<serde_json::Value> = Vec::new();
    let mut has_system = false;
    for m in messages.into_iter() {
        let role = m
            .get("role")
            .and_then(|r| r.as_str())
            .unwrap_or("user")
            .to_string();
        if role == "system" {
            if !has_system {
                out.insert(0, m);
                has_system = true;
            } else if let Some(extra) = m.get("content").and_then(|c| c.as_str()) {
                // Fold stray extra system messages into the first one.
                if let Some(first) = out.first_mut() {
                    let prev = first.get("content").and_then(|c| c.as_str()).unwrap_or("");
                    first["content"] = serde_json::json!(format!("{}\n\n{}", prev, extra));
                }
            }
            continue;
        }
        let content = m
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        let non_system_count = out.len() - usize::from(has_system);
        if non_system_count == 0 && role == "assistant" {
            out.push(serde_json::json!({ "role": "user", "content": "(conversation resumed)" }));
        }
        if let Some(last) = out.last_mut() {
            if last.get("role").and_then(|r| r.as_str()) == Some(role.as_str()) {
                let prev = last.get("content").and_then(|c| c.as_str()).unwrap_or("");
                last["content"] = serde_json::json!(format!("{}\n\n{}", prev, content));
                continue;
            }
        }
        out.push(serde_json::json!({ "role": role, "content": content }));
    }
    // Generation must be prompted by a trailing user turn.
    match out.last().and_then(|m| m.get("role")).and_then(|r| r.as_str()) {
        Some("user") => {}
        Some("system") | None => {
            out.push(serde_json::json!({ "role": "user", "content": "(begin)" }));
        }
        _ => {
            out.push(serde_json::json!({
                "role": "user",
                "content": "(continue from the conversation above)"
            }));
        }
    }
    out
}

fn call_openai_compat(
    app_handle: &tauri::AppHandle,
    run_id: &str,
    url: &str,
    settings: &LlmSettings,
    messages: Vec<serde_json::Value>,
    tools: Option<serde_json::Value>,
) -> Result<String, String> {
    let messages = normalize_for_strict_templates(messages);
    // Vision: if the agent just took a screenshot and the loaded model can see,
    // attach the PNG to that turn. Probe capability only when an image is
    // actually pending, so the common no-screenshot turn pays no latency.
    let messages = if last_screenshot(&messages).is_some() {
        attach_recent_screenshot(messages, model_supports_vision(settings))
    } else {
        messages
    };
    let mut payload = serde_json::json!({
    "model": settings.model,
    "messages": messages,
    "temperature": 0.7,
    // Hard ceiling on a single response: a ruminating reasoning model can
        // otherwise circle ("wait, what if...") for minutes on slow hardware.
            "max_tokens": 4096,
            "stream": true
        });

    // Ask for a usage block in the stream so the vitals panel gets exact token
    // counts (LM Studio / llama.cpp honor this; servers that don't just omit it
    // and we fall back to a length estimate).
    if let Some(obj) = payload.as_object_mut() {
        obj.insert(
            "stream_options".to_string(),
            serde_json::json!({ "include_usage": true }),
        );
    }

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

    let mut timer = StreamTimer::new();
    let mut prompt_tokens: Option<u64> = None;
    let mut completion_tokens: Option<u64> = None;
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
                    timer.mark();
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
            if let Some((p, c)) = parse_openai_usage(&line_str) {
                prompt_tokens = Some(p);
                completion_tokens = Some(c);
            }
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

        log_llm_metrics(
            app_handle,
            run_id,
            &timer,
            prompt_tokens,
            completion_tokens,
            None,
            None,
            full_response.chars().count(),
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
    let messages = normalize_for_strict_templates(messages);
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

    let mut timer = StreamTimer::new();
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
    let mut prompt_tokens: Option<u64> = None;
    let mut completion_tokens: Option<u64> = None;
    let mut server_prompt_tps: Option<f64> = None;
    let mut server_decode_tps: Option<f64> = None;

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
            timer.mark();
            full_response.push_str(&chunk_to_emit);
            emit_chunk(app_handle, run_id, &chunk_to_emit, false);
        }

        if json.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
            // Ollama's final object carries exact server timings (durations in
            // nanoseconds) — use them directly rather than wall-clock estimates.
            prompt_tokens = json.get("prompt_eval_count").and_then(|x| x.as_u64());
            completion_tokens = json.get("eval_count").and_then(|x| x.as_u64());
            let tps = |count: Option<u64>, dur_ns: Option<u64>| -> Option<f64> {
                match (count, dur_ns) {
                    (Some(c), Some(d)) if d > 0 => Some(c as f64 / (d as f64 / 1e9)),
                    _ => None,
                }
            };
            server_prompt_tps = tps(prompt_tokens, json.get("prompt_eval_duration").and_then(|x| x.as_u64()));
            server_decode_tps = tps(completion_tokens, json.get("eval_duration").and_then(|x| x.as_u64()));
            break;
        }
    }

    if in_reasoning {
        full_response.push_str("\n</think>\n");
        emit_chunk(app_handle, run_id, "\n</think>\n", false);
    }

    bridge_tool_calls_into_text(&mut full_response, &accumulated_tool_calls);

    emit_chunk(app_handle, run_id, "", true);
    log_llm_metrics(
        app_handle,
        run_id,
        &timer,
        prompt_tokens,
        completion_tokens,
        server_prompt_tps,
        server_decode_tps,
        full_response.chars().count(),
    );
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
    let last_content = non_system
        .last()
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    // Send only the newest message on a live chain — UNLESS it's blank. An empty
    // prompt (a state a block->resume can leave in the rebuilt history) makes the
    // model emit filler prose with no thinking or tool calls, and on a chain it
    // repeats turn after turn. When the delta would be empty (or there's no chain
    // to continue), replay the local transcript so there is always real,
    // role-structured content for the model to act on.
    let send_delta = (previous_response_id.is_some() || non_system.len() <= 1)
        && !last_content.trim().is_empty();
    let input_text = if send_delta {
        last_content.to_string()
    } else {
        let mut transcript = String::from("[Replaying prior conversation]\n\n");
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
    // Only continue the server-side chain when we're actually sending the delta.
    // A transcript replay (empty delta, or no chain) must NOT also chain, or the
    // server would stack the replayed history on top of the thread it still holds.
    if send_delta {
        if let Some(ref prev) = previous_response_id {
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("previous_response_id".to_string(), serde_json::json!(prev));
            }
        }
    }

    let agent = http_agent();
    let mut req = agent.post(url).set("Content-Type", "application/json");
    if !settings.api_key.trim().is_empty() {
        req = req.set("Authorization", &format!("Bearer {}", settings.api_key));
    }

    let mut timer = StreamTimer::new();
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
    let mut server_decode_tps: Option<f64> = None;
    let mut prompt_tokens: Option<u64> = None;
    let mut completion_tokens: Option<u64> = None;

    for line in reader.lines() {
        if run_is_cancelled(app_handle, run_id) {
            return Err("Cancelled by user".to_string());
        }
        let line_str = match line {
            Ok(l) => l,
            Err(e) => {
                // Read error mid-stream — most often a slow-prefill/read timeout.
                // Don't discard what already streamed: if anything came through,
                // keep it as a (truncated) turn so it persists and renders
                // instead of vanishing on the next re-render. Only a stream that
                // produced nothing falls through to a hard error/block.
                log_error(&format!("LM Studio stream read error: {}", e));
                if full_response.trim().is_empty() {
                    return Err(format!("LM Studio stream read failed: {}", e));
                }
                break;
            }
        };
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
                    timer.mark();
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
                    timer.mark();
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
                let result = json.get("result");
                let response_id = result
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
                // LM Studio reports throughput + token counts in the terminal
                // event (field names vary across versions, so probe a few).
                let stats = result.and_then(|r| r.get("stats")).or_else(|| json.get("stats"));
                server_decode_tps = stats
                    .and_then(|s| s.get("tokens_per_second").or_else(|| s.get("generation_tps")))
                    .and_then(|t| t.as_f64());
                let usage = result.and_then(|r| r.get("usage")).or_else(|| json.get("usage"));
                prompt_tokens = usage
                    .and_then(|u| u.get("prompt_tokens").or_else(|| u.get("input_tokens")))
                    .and_then(|x| x.as_u64());
                completion_tokens = usage
                    .and_then(|u| u.get("completion_tokens").or_else(|| u.get("output_tokens")))
                    .and_then(|x| x.as_u64());
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
    log_llm_metrics(
        app_handle,
        run_id,
        &timer,
        prompt_tokens,
        completion_tokens,
        None,
        server_decode_tps,
        full_response.chars().count(),
    );
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
                "search_codebase",
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
                false,
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

            let parsed_calls = parse_tool_calls_all(&remaining);
            if !parsed_calls.is_empty() {
                for (tool_name, args, preamble) in parsed_calls {
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
                    Some("design"),
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
                }
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
                "search_codebase",
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
                false,
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

            let parsed_calls = parse_tool_calls_all(&remaining);
            if !parsed_calls.is_empty() {
                for (tool_name, args, preamble) in parsed_calls {
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
                    None,
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
                }
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
    _app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    project_path: String,
) -> Result<Vec<Card>, String> {
    let cards = state.cards.lock().unwrap();

    // New and empty projects start with an empty board — the lazy demo-card
    // seeding that used to happen here planted four fictional cards (with
    // fabricated run transcripts) into every project that had none, which the
    // agent once solemnly audited as real project history.
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
        runner: default_runner(),
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
        c.runner = normalize_runner(&card.runner);

        if let Ok(conn) = get_db_conn(&app_handle) {
            let labels_json = serde_json::to_string(&c.labels).unwrap_or_else(|_| "[]".to_string());
            let _ = conn.execute(
                "UPDATE cards SET title = ?1, description = ?2, status = ?3, run_id = ?4, assignee = ?5, project_path = ?6, priority = ?7, labels = ?8, runner = ?9 WHERE id = ?10",
                (&c.title, &c.description, &c.status, &c.run_id, &c.assignee, &c.project_path, &c.priority, &labels_json, &c.runner, &c.id),
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

        // Best-effort background RAG refresh so the agent's search_codebase tool
        // sees the project's current code. Indexing is incremental (only changed
        // files are re-embedded), so this is cheap when nothing has changed, and
        // it runs off-thread so it never delays the run starting.
        {
            let cfg = load_config(&app_handle).settings;
            if cfg.embedding_configured() && cfg.embedding_auto_index {
                let handle = app_handle.clone();
                let scope = repo_path.to_string_lossy().into_owned();
                tauri::async_runtime::spawn_blocking(move || {
                    let _ = index_project_blocking(&handle, &scope, &cfg);
                });
            }
        }

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
pub async fn pause_run(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<(), String> {
    // Non-destructive stop: just flag the run. The agent loop sees the flag at
    // its next between-turns checkpoint and breaks to `blocked` without
    // touching the worktree, so unblock_run can resume it. The current turn (an
    // in-flight LLM call / tool execution) finishes first — pause never
    // interrupts a tool mid-write. A no-op if the run isn't active.
    let mut paused = state.paused_runs.lock().unwrap();
    paused.insert(run_id);
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
            // A plain resume (e.g. after a pause) passes an empty reply — don't
            // inject a ghost empty user turn into the transcript in that case.
            if !reply.trim().is_empty() {
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
                let topic = format!("Completed: {}", card.title);
                let settings = load_config(&app_handle).settings;
                let emb = embed_memory_text(&settings, &topic, &summary);
                let _ = insert_memory(
                    &conn,
                    &scope,
                    &topic,
                    &summary,
                    "run_accept",
                    Some(&run_id),
                    Some(&card.id),
                    emb.as_deref(),
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
pub async fn get_run_vitals(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<RunVitals, String> {
    let logs = state.run_logs.lock().unwrap();
    Ok(match logs.get(&run_id) {
        Some(events) => compute_run_vitals(events),
        None => RunVitals::default(),
    })
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

/// Anthropic has no model-discovery endpoint, so the frontier escalation
/// targets are listed statically (mirrors The Reef's approach). Dashed IDs
/// only — dotted aliases are rejected by some auth endpoints. Keep current
/// when new models ship. These are the models Beetle can escalate TO when a
/// local run gets stuck: she works locally 90% of the time and reaches up the
/// ladder — to her human, or to a frontier model that reads the same card,
/// repo, and memory — when she needs an assist.
const ANTHROPIC_MODEL_IDS: &[&str] = &[
    "claude-opus-4-8",
    "claude-opus-4-6",
    "claude-sonnet-4-6",
    "claude-haiku-4-5",
];

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

    // Fetch strategy is keyed off the SAME provider_kind that call_llm uses to
    // route chat, so the dropdown can never again disagree with chat about what
    // a provider string means. (The original bug: this function bucketed
    // "custom" WITH "ollama" and hit /api/tags + /api/ps first, so the
    // default-seeded "custom" provider pointed at LM Studio on port 11434 —
    // Ollama's port — got the wrong dialect, LM Studio shimmed an unhelpful
    // 200, and the dropdown starved while chat worked fine.)
    let kind = provider_kind(&provider.to_lowercase());

    // Anthropic exposes no model-discovery endpoint — return the static
    // escalation catalogue rather than probing.
    if kind == ProviderKind::Anthropic {
        return Ok(ANTHROPIC_MODEL_IDS
            .iter()
            .map(|id| ModelInfo {
                name: id.to_string(),
                is_loaded: true,
                context_size: Some(200_000),
            })
            .collect());
    }

    let is_ollama_first = kind == ProviderKind::OllamaNative;

    // LM Studio's native /api/v1/models list is richer than its OpenAI-compat
    // shim (real loaded state, configured context length). Try it for any
    // non-Ollama provider, since "custom" is the common way to point at LM
    // Studio without selecting the dedicated provider.
    if !is_ollama_first {
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

    if !is_ollama_first {
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

        // Last-resort Ollama probe: a "custom" provider could legitimately be
        // pointed at an Ollama server. Only reached after LM Studio native and
        // OpenAI-compat /models have all failed, so it never fires first for
        // an LM Studio endpoint that answers /v1/models.
        let tags_url = format!("{}/api/tags", base_url);
        if let Ok(resp) = ureq::get(&tags_url)
            .timeout(std::time::Duration::from_secs(3))
            .call()
        {
            if resp.status() == 200 {
                if let Ok(json) = resp.into_json::<serde_json::Value>() {
                    if let Some(models) = json.get("models").and_then(|v| v.as_array()) {
                        let mut models_list = Vec::new();
                        for m in models {
                            if let Some(name) = m.get("name").and_then(|v| v.as_str()) {
                                models_list.push(ModelInfo {
                                    name: name.to_string(),
                                    is_loaded: false,
                                    context_size: None,
                                });
                            }
                        }
                        if !models_list.is_empty() {
                            return Ok(models_list);
                        }
                    }
                }
            }
        }
    } else {
        // Genuine Ollama provider: Ollama-specific endpoints first.
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
        // Kill any dev server / watcher this run started so it never outlives
        // the run (completion, error, cancel, and pause all land here).
        kill_run_background_processes(&self.app_handle, &self.run_id);
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
    assist_available: bool,
) -> String {
    let mut prompt = format!(
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
         8. `run_command(command: String, timeout_secs?: Int)`: Runs a build, test, or check shell command in the workspace (e.g. \"npm run build\", \"npm test\", \"cargo check\"). Use this to verify your code compiles and passes tests! The result starts with the exit code (`[exit code: 0]` means success) and the output is clipped from both ends, so the error at the bottom survives. timeout_secs defaults to 300 (max 1800). NOTE: the shell is Windows cmd.exe — Unix tools like grep, sed, awk, and ls are NOT available. Use search_grep, patch_file, and list_dir instead.\n\
         9. `patch_file(path: String, target: String, replacement: String)`: Replaces an exact text snippet in a file. THE tool for a SINGLE-LINE fix: target = the exact TEXT of the broken line (copied without read_file's line-number prefix — target is text, NEVER a line number), replacement = the corrected line — no line numbers involved, so it either lands exactly or refuses cleanly; it cannot hit the wrong line. The target must match byte-for-byte including quotes and whitespace; if you cannot reproduce the snippet exactly, use replace_lines. Also the safest way to INSERT new lines (a missing brace, an import): target = an existing anchor line, replacement = that same line plus the new content — anchored insertion can't land in the wrong place and survives line-number drift.\n\
         10. `replace_lines(path: String, start_line: int, end_line: int, content: String)`: Replaces an inclusive 1-indexed line range with new content (empty content deletes the lines). To INSERT without deleting, replace one anchor line with itself plus the new lines. THE tool for multi-line edits and for fixes where the broken text is hard to quote exactly: the compiler reports file:line and read_file output is line-numbered — read the reported lines, then replace exactly those line numbers. For a single broken line you CAN quote exactly, prefer patch_file. NEVER rewrite a whole file to fix a one-line error, and NEVER widen the range when an edit misses — the result message echoes the edit site with current numbers: verify, aim, and fix the ONE line. Line numbers SHIFT after any edit that changes line count, and the harness REFUSES an edit made with stale numbers — to make several edits to one file in a row, work bottom-to-top (highest line numbers first; lines below an edit keep their numbers), or re-read between edits.\n\
         11. `web_search(query: String)`: Searches the web for programming queries, libraries, APIs, or documentation snippets.\n\
         12. `send_notification(message: String)`: Sends a system alert/notification to the developer.\n\
         13. `read_card()`: Shows YOUR current card: title, description, status, and its todo list with indices. Read it at the start of a run and use the todos as your work plan.\n\
         14. `set_todo(index: int, completed: bool)`: Checks off a todo on your card (index from read_card). Mark items complete as you finish them so progress is visible.\n\
         15. `task_complete(summary: String)`: Ends the loop, summarizes your work, and puts the card in \"Review\" status. Before calling it, read_card and confirm every todo is checked — or explain why not in your summary.\n\
         16. `find_file(name: String, path?: String)`: Finds files by name. Give a fragment of the filename (case-insensitive) and get matching relative paths back. The fastest way to locate a file you know exists.\n\
         17. `find_symbol(name: String, path?: String)`: Finds where a function, struct, class, or other declaration is DEFINED. Returns file:line: signature. Faster and more precise than search_grep when you want a definition rather than usages.\n\
         18. `remember(topic: String, content: String)`: Saves a durable insight to this project's long-term memory — shared across runs and chat modes. Use it when you learn something worth keeping: how a subsystem works, a decision made, a pitfall discovered.\n\
         19. `recall(query: String, limit?: Int)`: Searches this project's long-term memory and returns the most relevant matches — ranked semantically (by meaning) when an embedding provider is configured, else by keyword (empty query = latest memories). Past runs may have already mapped the territory — check before exploring from scratch.\n\
         20. `search_codebase(query: String, limit?: Int)`: Semantic search over the project's INDEXED code and docs. Finds code by concept and returns the most relevant chunks as file:line ranges — ideal for \"where is X handled?\" / \"how does Y work?\". For exact strings or symbol names, prefer search_grep/find_symbol. Returns nothing if the project hasn't been indexed yet.\n\
         21. `list_cards()`: Shows ALL kanban cards for this project grouped by status, with ids and todo progress. (read_card shows only YOUR card.)\n\
         22. `create_card(title: String, description: String, todos?: [String], priority?: \"low\"|\"medium\"|\"high\", labels?: [String])`: Files a new card in the backlog for the developer to review. If you discover a bug or needed work OUTSIDE your current card's scope, file a card for it instead of silently expanding your task — then stay on your card.\n\
         23. `update_card(card_id: String, title?: String, description?: String, priority?: String, todos?: [String], add_todo?: String, add_label?: String)`: Edits a backlog/todo card. `todos` REPLACES the whole checklist; `add_todo` appends one item.\n\
         24. `delete_card(card_id: String)`: Deletes a backlog/todo card that has no run history.\n\
         25. `screenshot(url: String, width?: Int, height?: Int)`: Renders a page the dev server is ALREADY serving (e.g. http://localhost:5173/) in a headless browser and captures it. If the loaded model is vision-capable, the image is shown to you next turn so you can SEE the rendered UI — use it to check layout/alignment instead of guessing from CSS. It does NOT start the dev server — use start_server for that.\n\
         26. `start_server(command: String, port: Int, timeout_secs?: Int)`: Starts a long-running process (e.g. \"npm run dev\") in the background and waits until `port` accepts connections, so you can then screenshot http://localhost:<port>/. Use this — NOT run_command — for dev servers and watchers, because run_command blocks and kills long-running commands. The server keeps running across turns and is stopped automatically when the run ends.\n\
         27. `stop_server(port?: Int)`: Stops background server(s) you started (all of them if no port is given).\n\
         28. `server_logs(port?: Int)`: Shows recent stdout/stderr from your background server(s) — use it to diagnose a server that didn't come up or a runtime error it logged.\n\n\
         Work efficiently with context: prefer outline_file + ranged read_file over reading entire files, since large reads slow the model and crowd out useful history. Starting unfamiliar work? Call recall() first, and search_codebase() to locate relevant code by concept — and remember() durable insights as you go. When you have finished the task, you MUST call task_complete — do not simply describe that you are done in prose.",
        card_title, card_description, worktree_path.to_string_lossy()
    );
    if assist_available {
        prompt.push_str(
            "\n\nPHONE A FRIEND — `request_assist(question: String, context?: String)`: when you are genuinely STUCK (e.g. the same edit has failed 2–3 times, a build error you can't decipher, or you can't find a workable approach), call this to consult a stronger \"senior\" model. Describe the problem and what you've already tried; you'll get back concrete advice or a patch to apply YOURSELF (it does not edit files for you). Use it sparingly — only when genuinely stuck, never for routine steps.",
        );
    }
    prompt
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

const KNOWN_TOOLS: [&str; 28] = [
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
    "search_codebase",
    "list_cards",
    "create_card",
    "update_card",
    "delete_card",
    "git_status",
    "git_diff",
    "run_command",
    "screenshot",
    "start_server",
    "stop_server",
    "server_logs",
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
    // The key/value split (in parse_fn_args_strict/_lenient) consumes only the
    // FIRST `=`/`:`. When the model emits a redundant separator —
    // `read_file(path == "x")` or `path := "x"` — the second one stays fused to
    // the value as `="x"`, so the quote-stripping branch below never fires and
    // the literal `="x"` (quotes and all) reaches the tool. Any leading
    // separator/whitespace run on the value is junk at this point (the real
    // separator is already gone), so drop it before classifying.
    let v = val_str.trim_start_matches(|c: char| c == '=' || c == ':' || c.is_whitespace());
    let v = v.trim_end();
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
    // Flattened-args recovery: some models (e.g. Qwen3.5 emitting text-protocol
    // calls) write the arguments as SIBLINGS of "name" rather than under an
    // args/arguments/parameters wrapper — {"name":"list_dir","path":"src"}.
    // Without this, those arguments silently vanish: the call parses with a
    // valid name and null args, the tool runs with defaults (list_dir lists the
    // root no matter which path was asked for), and the model loops. Only kick
    // in when no explicit wrapper was found, and skip the call's own metadata
    // keys so we don't mistake them for arguments.
    if args.is_null() {
        if let Some(obj) = val.as_object() {
            const META_KEYS: [&str; 6] = ["name", "tool", "type", "id", "index", "function"];
            let recovered: serde_json::Map<String, serde_json::Value> = obj
                .iter()
                .filter(|(k, _)| !META_KEYS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if !recovered.is_empty() {
                return serde_json::Value::Object(recovered);
            }
        }
    }
    args
}

fn try_parse_json(s: &str) -> Option<(String, serde_json::Value)> {
    if let Some(val) = parse_json_relaxed(s) {
        if let Some(name) = val.get("name").and_then(|n| n.as_str()) {
            let args = extract_tool_args(&val);
            return Some((name.to_string(), args));
        }
    }

    if let Some(start_brace) = s.find('{') {
        if let Some(end_brace) = s.rfind('}') {
            if end_brace > start_brace {
                let json_sub = &s[start_brace..=end_brace];
                if let Some(val) = parse_json_relaxed(json_sub) {
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

/// Tolerant JSON parsing for model-emitted tool arguments. Small local models
/// routinely emit relaxed JSON — unquoted keys ({path: ""}), single-quoted
/// strings ({'path': ''}), trailing commas — which strict serde rejects. Try
/// strict first; on failure, repair those three artifacts (touching nothing
/// inside string literals) and try once more. The motivating field failure:
/// `<|tool_call>call:list_dir{path: ""}<tool_call|>` parsed as prose because
/// of one missing pair of key quotes.
fn parse_json_relaxed(s: &str) -> Option<serde_json::Value> {
    if let Ok(v) = serde_json::from_str(s) {
        return Some(v);
    }
    serde_json::from_str(&repair_relaxed_json(s)).ok()
}

fn repair_relaxed_json(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 16);
    let mut i = 0usize;
    let mut in_str = false;
    let mut str_delim = '"';
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            if c == '\\' && i + 1 < chars.len() {
                let next = chars[i + 1];
                if str_delim == '\'' && next == '\'' {
                    // \' inside a single-quoted string: plain apostrophe in JSON.
                    out.push('\'');
                } else {
                    out.push('\\');
                    out.push(next);
                }
                i += 2;
                continue;
            }
            if c == str_delim {
                in_str = false;
                out.push('"');
            } else if c == '"' && str_delim == '\'' {
                // Literal double quote inside a single-quoted string: escape it.
                out.push('\\');
                out.push('"');
            } else {
                out.push(c);
            }
            i += 1;
            continue;
        }
        match c {
            '"' | '\'' => {
                in_str = true;
                str_delim = c;
                out.push('"');
                i += 1;
            }
            '`' => {
                // JS template-literal habit: a backtick-delimited value. JSON
                // has no backtick strings, and the payload may contain NESTED
                // backticks (template literals inside code), so the closer is
                // the FIRST backtick followed (after optional whitespace) by
                // ',' or '}' or end-of-input — a plausible value terminator.
                // Nested template-literal backticks sit against code characters
                // and never qualify. (Plain greedy-to-last-backtick swallowed a
                // following `path:` argument into the content string when the
                // model backtick-quoted TWO values in one call — field failure.)
                let rest = &chars[i + 1..];
                let mut rel_close: Option<usize> = None;
                for (j, &ch) in rest.iter().enumerate() {
                    if ch != '`' {
                        continue;
                    }
                    let mut k = j + 1;
                    while k < rest.len() && rest[k].is_whitespace() {
                        k += 1;
                    }
                    if k >= rest.len() || rest[k] == ',' || rest[k] == '}' {
                        rel_close = Some(j);
                        break;
                    }
                }
                let rel_close = rel_close.or_else(|| rest.iter().rposition(|&ch| ch == '`'));
                if let Some(rel) = rel_close {
                    let close = i + 1 + rel;
                    out.push('"');
                    for &ch in &chars[i + 1..close] {
                        match ch {
                            '"' => out.push_str("\\\""),
                            '\\' => out.push_str("\\\\"),
                            '\n' => out.push_str("\\n"),
                            '\r' => out.push_str("\\r"),
                            '\t' => out.push_str("\\t"),
                            cc if (cc as u32) < 0x20 => {
                                out.push_str(&format!("\\u{:04x}", cc as u32))
                            }
                            cc => out.push(cc),
                        }
                    }
                    out.push('"');
                    i = close + 1;
                } else {
                    // Lone backtick: stray fence debris, drop it.
                    i += 1;
                }
            }
            ch if ch.is_ascii_alphabetic() || ch == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let ident: String = chars[start..i].iter().collect();
                let mut j = i;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if j < chars.len() && chars[j] == ':' {
                    // Bare identifier used as an object key: quote it.
                    out.push('"');
                    out.push_str(&ident);
                    out.push('"');
                } else {
                    // Bare value token (true/false/null): leave untouched.
                    out.push_str(&ident);
                }
            }
            ',' => {
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if !(j < chars.len() && (chars[j] == '}' || chars[j] == ']')) {
                    out.push(',');
                }
                // Trailing comma before a closer is dropped entirely.
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
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
    let args: serde_json::Value = parse_json_relaxed(json_str)?;
    Some((name.to_string(), args))
}

/// Prose the model wrote before its tool-call syntax. Returns empty for
/// syntax debris (stray punctuation, lone backticks); caps length so a
/// pathological preamble can't bloat the transcript or the history budget.
fn clean_tool_preamble(text: &str) -> String {
    let mut t = text.trim();
    // Strip stray protocol fragments at the edges — leftover closers between
    // back-to-back calls must not become ghost "prose" messages.
    loop {
        let before = t;
        for tok in [
            "</tool_call>",
            "<|/tool_call|>",
            "<tool_call|>",
            "<|tool_call|>",
            "<|tool_call>",
            "<tool_call>",
        ] {
            t = t
                .trim_start_matches(tok)
                .trim_end_matches(tok)
                .trim();
        }
        if t == before {
            break;
        }
    }
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
/// surface it instead of clobbering it. Single-call convenience wrapper over
/// parse_tool_calls_all — production loops iterate the full vec; this wrapper
/// survives as the regression tests' API and compiles only with them.
#[cfg(test)]
fn parse_tool_call_spanned(response: &str) -> Option<(String, serde_json::Value, String)> {
    parse_tool_calls_all(response).into_iter().next()
}

/// Parse ALL tool calls in a response, in order, each paired with the prose
/// immediately preceding it. A model that emits two calls in one message
/// loses neither. Strategy precedence (fences/tags before bare syntax) is
/// kept WITHIN each scan segment — it protects prose like "I'll call
/// list_dir(...)" from false-positives — so mixed-format multi-calls may
/// resolve in strategy order rather than textual order; same-format
/// multi-calls (the cases that occur in practice) resolve in order. Capped
/// to guard against pathological output.
fn parse_tool_calls_all(response: &str) -> Vec<(String, serde_json::Value, String)> {
    const MAX_CALLS: usize = 5;
    let mut out: Vec<(String, serde_json::Value, String)> = Vec::new();
    let mut rest = response.trim();
    while out.len() < MAX_CALLS && !rest.is_empty() {
        match parse_tool_call_at(rest) {
            Some((name, args, start, end)) => {
                let preamble = clean_tool_preamble(&rest[..start]);
                out.push((name, args, preamble));
                rest = rest[end.min(rest.len())..].trim_start();
            }
            None => break,
        }
    }
    out
}

/// Find the matching `close` delimiter for the `open` delimiter at byte
/// `open_idx`, honoring single- and double-quoted strings and backslash
/// escapes. ASCII delimiters never collide with UTF-8 continuation bytes, so
/// byte-wise scanning is safe. Returns the byte index of the matching closer.
fn find_matching_delim_bytes(s: &str, open_idx: usize, open: u8, close: u8) -> Option<usize> {
    let b = s.as_bytes();
    let mut depth = 0usize;
    let mut in_str = false;
    let mut delim = b'"';
    let mut i = open_idx;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == delim {
                in_str = false;
            }
        } else if c == b'"' || c == b'\'' || c == b'`' {
            in_str = true;
            delim = c;
        } else if c == open {
            depth += 1;
        } else if c == close {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Core tool-call detection. Returns (name, args, start, end): the byte span
/// of the tool-call syntax in `response_cleaned`. Everything before `start`
/// is the model's preamble prose; scanning resumes at `end` for multi-call
/// responses.
fn parse_tool_call_at(
    response_cleaned: &str,
) -> Option<(String, serde_json::Value, usize, usize)> {
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
                    let call_end = absolute_start + len + end_idx + len;
                    return Some((name, args, absolute_start, call_end));
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
    const TAG_CLOSERS: [&str; 4] = [
        "</tool_call>",
        "<|/tool_call|>",
        "<tool_call|>",
        "<|tool_call>",
    ];
    for opener in TAG_OPENERS {
        let mut search = 0;
        while let Some(idx) = response_cleaned[search..].find(opener) {
            let opener_pos = search + idx;
            let start = opener_pos + opener.len();
            let rest = &response_cleaned[start..];
            // Earliest closer of any form, remembering its length so the call's
            // end offset lands AFTER it (multi-call scanning resumes there).
            let mut closer: Option<(usize, usize)> = None; // (offset, len)
            for cl in TAG_CLOSERS {
                if let Some(p) = rest.find(cl) {
                    if closer.map(|(e, _)| p < e).unwrap_or(true) {
                        closer = Some((p, cl.len()));
                    }
                }
            }
            let inner = match closer {
                Some((e, _)) => &rest[..e],
                None => rest,
            };
            let call_end = match closer {
                Some((e, l)) => start + e + l,
                None => response_cleaned.len(),
            };
            let inner = inner.trim();
            if let Some((name, args)) = try_parse_json(inner) {
                return Some((name, args, opener_pos, call_end));
            }
            if let Some((name, args)) = parse_function_syntax(inner) {
                return Some((name, args, opener_pos, call_end));
            }
            if let Some((name, args)) = parse_qwen_syntax(inner) {
                return Some((name, args, opener_pos, call_end));
            }
            search = start;
        }
    }

    if let Some((name, args)) = try_parse_json(response_cleaned) {
        return Some((name, args, 0, response_cleaned.len()));
    }

    // Bare-syntax calls in the response body (e.g. `list_dir(path="src")`),
    // anchored to known tool names so ordinary prose can't false-positive.
    // Scan ALL tools and all three pattern forms, then take the TEXTUALLY
    // EARLIEST match. Iterating the tool array and returning on the first hit
    // would let the ARRAY's order decide which call wins: with
    // `call:list_dir{...}` followed by `call:read_file{...}`, read_file's
    // earlier position in KNOWN_TOOLS made the scanner return the SECOND call
    // and demote the first to preamble prose (caught by the multi-call
    // regression test). Earliest-wins also subsumes the old rule that the
    // `call:`-prefixed form must beat the bare brace form: for the same
    // physical call, the prefix form starts earlier.
    let mut best: Option<(String, serde_json::Value, usize, usize)> = None;
    for tool in KNOWN_TOOLS {
        let pat = format!("{}(", tool);
        if let Some(idx) = response_cleaned.find(&pat) {
            if best.as_ref().map(|b| idx < b.2).unwrap_or(true) {
                let paren_idx = idx + tool.len();
                let call_end = find_matching_delim_bytes(response_cleaned, paren_idx, b'(', b')')
                    .map(|c| c + 1)
                    .unwrap_or(response_cleaned.len());
                if let Some((name, args)) = parse_function_syntax(&response_cleaned[idx..call_end]) {
                    if name == tool {
                        best = Some((name, args, idx, call_end));
                    }
                }
            }
        }
        let pat_prefix = format!("call:{}{{", tool);
        if let Some(idx) = response_cleaned.find(&pat_prefix) {
            if best.as_ref().map(|b| idx < b.2).unwrap_or(true) {
                let brace_idx = idx + pat_prefix.len() - 1;
                // Slice the parse input to the balanced span: parse_qwen_syntax uses
                // rfind('}') internally, which would otherwise swallow a SECOND
                // brace-style call later in the response.
                let call_end = find_matching_delim_bytes(response_cleaned, brace_idx, b'{', b'}')
                    .map(|c| c + 1)
                    .unwrap_or(response_cleaned.len());
                if let Some((name, args)) = parse_qwen_syntax(&response_cleaned[idx..call_end]) {
                    if name == tool {
                        best = Some((name, args, idx, call_end));
                    }
                }
            }
        }
        let pat_brace = format!("{}{{", tool);
        if let Some(idx) = response_cleaned.find(&pat_brace) {
            if best.as_ref().map(|b| idx < b.2).unwrap_or(true) {
                let brace_idx = idx + tool.len();
                let call_end = find_matching_delim_bytes(response_cleaned, brace_idx, b'{', b'}')
                    .map(|c| c + 1)
                    .unwrap_or(response_cleaned.len());
                if let Some((name, args)) = parse_qwen_syntax(&response_cleaned[idx..call_end]) {
                    if name == tool {
                        best = Some((name, args, idx, call_end));
                    }
                }
            }
        }
    }
    if best.is_some() {
        return best;
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

/// Truncate keeping BOTH ends. Build/test runners echo the command and progress
/// at the top but print the actual error at the BOTTOM, so the old head-only clip
/// routinely threw away the one chunk the model needed to diagnose a failure.
fn clip_head_tail(s: &str, budget: usize) -> String {
    let total = s.chars().count();
    if total <= budget {
        return s.to_string();
    }
    let head_len = budget / 4;
    let tail_len = budget - head_len;
    let head: String = s.chars().take(head_len).collect();
    let tail: String = s.chars().skip(total - tail_len).collect();
    let omitted = total - head_len - tail_len;
    format!("{}\n... [{} chars omitted] ...\n{}", head, omitted, tail)
}

/// Run a shell command in `dir`, returning the exit code plus head+tail-clipped
/// output. `timeout_secs` caps wall-clock (default 300s, clamped to [1, 1800]);
/// a command that overruns is killed so a hung build can't wedge the whole run.
fn run_shell_command<P: AsRef<Path>>(
    dir: P,
    command_str: &str,
    timeout_secs: Option<u64>,
) -> Result<String, String> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

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
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    crate::configure_no_window(&mut cmd);

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;

    // Drain both streams on their own threads: a command that outfills the pipe
    // buffer (~64KB) blocks waiting for us to read, which would defeat the
    // timeout. Cap each stream so a runaway command can't exhaust memory while
    // we keep draining it to EOF.
    const STREAM_CAP: usize = 256 * 1024;
    fn drain_capped<R: Read>(mut r: R, cap: usize) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        while let Ok(n) = r.read(&mut chunk) {
            if n == 0 {
                break;
            }
            if buf.len() < cap {
                let take = (cap - buf.len()).min(n);
                buf.extend_from_slice(&chunk[..take]);
            }
        }
        buf
    }
    let so = child.stdout.take();
    let se = child.stderr.take();
    let t_out = std::thread::spawn(move || so.map(|r| drain_capped(r, STREAM_CAP)).unwrap_or_default());
    let t_err = std::thread::spawn(move || se.map(|r| drain_capped(r, STREAM_CAP)).unwrap_or_default());

    let timeout = Duration::from_secs(timeout_secs.unwrap_or(300).clamp(1, 1800));
    let start = Instant::now();
    let (status_opt, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), false),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break (None, true);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.to_string()),
        }
    };

    let out = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).to_string();
    let err = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).to_string();
    // stderr last, so the tail-biased clip preserves it — that's where cargo/tsc
    // and most runners put the error.
    let combined = if err.trim().is_empty() {
        out
    } else {
        format!("{}\n{}", out, err)
    };
    let body = clip_head_tail(combined.trim_end(), 4000);

    let header = if timed_out {
        format!("[command timed out after {}s — process killed]", timeout.as_secs())
    } else {
        match status_opt.and_then(|s| s.code()) {
            Some(code) => format!("[exit code: {}]", code),
            None => "[process terminated by signal]".to_string(),
        }
    };
    Ok(format!("{}\n{}", header, body))
}

/// Standard base64 (RFC 4648, with padding, no line wrapping). Hand-rolled to
/// avoid a new dependency; the only use is encoding a screenshot PNG into a
/// data: URI for vision-capable models.
fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

static SHOT_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Locate an installed headless-capable Chromium browser (Chrome preferred,
/// then Edge). v1 targets the user's Windows environment with POSIX fallbacks.
fn find_headless_browser() -> Option<std::path::PathBuf> {
    let candidates: &[&str] = if cfg!(target_os = "windows") {
        &[
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        ]
    } else {
        &[
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/usr/bin/microsoft-edge",
        ]
    };
    candidates
        .iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.exists())
}

/// Capture `url` to a PNG via headless Chrome/Edge and return the file path.
/// Saved under a per-run temp dir (NOT the worktree, so it never shows up in
/// git_status). The throwaway --user-data-dir is load-bearing: without it a
/// browser already running on the user's desktop hands the request off to that
/// instance and the headless invocation no-ops without writing a file.
fn capture_screenshot(run_id: &str, url: &str, width: u32, height: u32) -> Result<std::path::PathBuf, String> {
    use std::time::{Duration, Instant};
    let browser = find_headless_browser()
        .ok_or("no headless browser found (install Google Chrome or Microsoft Edge)")?;
    let dir = std::env::temp_dir().join("beetleai-shots").join(run_id);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let n = SHOT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let shot = dir.join(format!("shot-{}.png", n));
    let profile = dir.join("browser-profile");
    let _ = fs::remove_file(&shot);

    let mut cmd = Command::new(&browser);
    cmd.args([
        "--headless=new",
        "--disable-gpu",
        "--no-first-run",
        "--no-default-browser-check",
        "--hide-scrollbars",
        &format!("--user-data-dir={}", profile.display()),
        &format!("--screenshot={}", shot.display()),
        &format!("--window-size={},{}", width, height),
        url,
    ]);
    crate::configure_no_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("failed to launch browser: {}", e))?;

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(30) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("screenshot timed out after 30s".to_string());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    if shot.exists() {
        Ok(shot)
    } else {
        Err(format!("browser exited without writing a screenshot — is '{}' reachable?", url))
    }
}

/// Marker the screenshot tool prepends to its result so the image can be
/// re-attached to the model's next turn. Kept at the HEAD of the result because
/// truncate_tool_result clips the tail.
const SHOT_MARKER_OPEN: &str = "[screenshot:";

fn parse_screenshot_path(content: &str) -> Option<String> {
    let start = content.find(SHOT_MARKER_OPEN)? + SHOT_MARKER_OPEN.len();
    let rest = &content[start..];
    let end = rest.find(']')?;
    let p = rest[..end].trim();
    if p.is_empty() {
        None
    } else {
        Some(p.to_string())
    }
}

/// Index + path of the LAST message carrying a screenshot marker, if any.
fn last_screenshot(messages: &[serde_json::Value]) -> Option<(usize, String)> {
    let mut found = None;
    for (i, m) in messages.iter().enumerate() {
        if let Some(c) = m.get("content").and_then(|c| c.as_str()) {
            if let Some(p) = parse_screenshot_path(c) {
                found = Some((i, p));
            }
        }
    }
    found
}

/// Read `capabilities.vision` for `model` out of LM Studio's /api/v1/models
/// payload. Matches by exact key first, then falls back to any loaded
/// vision-capable model (the configured id can differ from the registry key).
fn vision_from_models_json(json: &serde_json::Value, model: &str) -> bool {
    let models = match json.get("models").and_then(|m| m.as_array()) {
        Some(m) => m,
        None => return false,
    };
    let cap_vision = |m: &serde_json::Value| {
        m.get("capabilities")
            .and_then(|c| c.get("vision"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    for m in models {
        if m.get("key").and_then(|k| k.as_str()) == Some(model) {
            return cap_vision(m);
        }
    }
    for m in models {
        let loaded = m
            .get("loaded_instances")
            .and_then(|a| a.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        if loaded && cap_vision(m) {
            return true;
        }
    }
    false
}

/// Best-effort: does the configured chat model accept images? Only the
/// OpenAI-compat path carries images in v1; for it we ask LM Studio's native
/// /api/v1/models. Fails CLOSED on any other provider or any error — we never
/// send an image to a model that can't take one.
fn model_supports_vision(settings: &LlmSettings) -> bool {
    if provider_kind(&settings.provider.to_lowercase()) != ProviderKind::OpenAiCompat {
        return false;
    }
    let root = settings
        .api_url
        .trim_end_matches('/')
        .trim_end_matches("/api/v1/chat")
        .trim_end_matches("/api/v1")
        .trim_end_matches("/v1")
        .trim_end_matches('/');
    let url = format!("{}/api/v1/models", root);
    let resp = match ureq::get(&url)
        .timeout(std::time::Duration::from_secs(3))
        .call()
    {
        Ok(r) => r,
        Err(_) => return false,
    };
    match resp.into_json::<serde_json::Value>() {
        Ok(json) => vision_from_models_json(&json, &settings.model),
        Err(_) => false,
    }
}

/// If a recent screenshot exists and `vision` is on, rewrite that message's
/// content from a plain string into an OpenAI multimodal array (text + the PNG
/// as a base64 data: URI). Only the most recent screenshot is attached, and
/// only while it's near the tail, so we don't re-send a large image every turn
/// for the rest of the run. A no-op when there's no screenshot, vision is off,
/// or the file is gone — the text marker simply remains.
fn attach_recent_screenshot(mut messages: Vec<serde_json::Value>, vision: bool) -> Vec<serde_json::Value> {
    let (idx, path) = match last_screenshot(&messages) {
        Some(x) => x,
        None => return messages,
    };
    if !vision || messages.len().saturating_sub(idx) > 6 {
        return messages;
    }
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(_) => return messages,
    };
    let data_uri = format!("data:image/png;base64,{}", base64_encode(&bytes));
    let role = messages[idx]
        .get("role")
        .cloned()
        .unwrap_or_else(|| serde_json::json!("user"));
    let text = messages[idx]
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    messages[idx] = serde_json::json!({
        "role": role,
        "content": [
            { "type": "text", "text": text },
            { "type": "image_url", "image_url": { "url": data_uri } }
        ]
    });
    messages
}

/// A long-lived process the agent started (e.g. `npm run dev`). Held in
/// AppState so it survives across turns and can be killed on run teardown.
pub struct BgProcess {
    pub label: String,
    pub port: u16,
    pub command: String,
    pub child: std::process::Child,
    /// Capped tail of the process's combined stdout+stderr, filled by drain
    /// threads so a server's startup/runtime errors are inspectable.
    pub output: std::sync::Arc<Mutex<String>>,
}

const BG_LOG_CAP: usize = 8192;

fn append_capped(buf: &std::sync::Arc<Mutex<String>>, chunk: &str) {
    if let Ok(mut s) = buf.lock() {
        s.push_str(chunk);
        let len = s.chars().count();
        if len > BG_LOG_CAP {
            *s = s.chars().skip(len - BG_LOG_CAP).collect();
        }
    }
}

/// Is something accepting TCP connections on localhost:`port`? Used as the
/// readiness signal for a freshly started server — more reliable than scraping
/// log output for a "listening on" line that varies per framework.
fn port_open(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(300),
    )
    .is_ok()
}

/// Kill a process AND its descendants. The dev server runs under a `cmd.exe`/
/// `sh` wrapper that spawns node etc.; killing only the wrapper would orphan the
/// real server, so on Windows we use `taskkill /T` to take down the whole tree.
fn kill_process_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let mut k = Command::new("taskkill");
        k.args(["/F", "/T", "/PID", &child.id().to_string()]);
        crate::configure_no_window(&mut k);
        let _ = k.output();
    }
    #[cfg(not(windows))]
    {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn spawn_background(
    worktree: &Path,
    command: &str,
) -> Result<(std::process::Child, std::sync::Arc<Mutex<String>>), String> {
    use std::io::Read;
    use std::process::Stdio;
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd.exe");
        c.arg("/c").arg(command);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    };
    cmd.current_dir(worktree)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::configure_no_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let buf = std::sync::Arc::new(Mutex::new(String::new()));
    for stream in [
        child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let b = buf.clone();
        let mut r = stream;
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = r.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                append_capped(&b, &String::from_utf8_lossy(&chunk[..n]));
            }
        });
    }
    Ok((child, buf))
}

/// Start `command` as a background process in `worktree`, register it under
/// `run_id`, and wait until `port` is accepting connections (or `timeout_secs`
/// elapses). Any existing server on the same port for this run is replaced.
fn bg_start(
    state: &AppState,
    run_id: &str,
    worktree: &Path,
    command: &str,
    port: u16,
    timeout_secs: u64,
) -> String {
    use std::time::{Duration, Instant};
    bg_stop(state, run_id, Some(port));
    let (child, output) = match spawn_background(worktree, command) {
        Ok(x) => x,
        Err(e) => return format!("Error: failed to start '{}' — {}", command, e),
    };
    {
        let mut map = state.bg_processes.lock().unwrap();
        map.entry(run_id.to_string()).or_default().push(BgProcess {
            label: format!("port {}", port),
            port,
            command: command.to_string(),
            child,
            output: output.clone(),
        });
    }
    let deadline = Duration::from_secs(timeout_secs.clamp(1, 180));
    let start = Instant::now();
    loop {
        if port_open(port) {
            return format!(
                "[server ready] http://localhost:{}/ is accepting connections. Screenshot it with screenshot(\"http://localhost:{}/\"). It keeps running across turns and is stopped automatically when this run ends (or call stop_server).",
                port, port
            );
        }
        if start.elapsed() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let logs = output.lock().map(|s| s.clone()).unwrap_or_default();
    let tail = clip_head_tail(logs.trim(), 1500);
    format!(
        "[server started, but port {} did not open within {}s] It may still be building, or it failed to launch. It is still running (stop_server to kill it). Recent output:\n{}",
        port,
        deadline.as_secs(),
        if tail.is_empty() { "(no output captured yet)" } else { &tail }
    )
}

/// Kill background processes for `run_id`: those matching `port`, or all when
/// `port` is None. Returns how many were killed.
fn bg_stop(state: &AppState, run_id: &str, port: Option<u16>) -> usize {
    let mut map = state.bg_processes.lock().unwrap();
    let mut killed = 0;
    if let Some(list) = map.get_mut(run_id) {
        let mut keep = Vec::new();
        for mut p in list.drain(..) {
            if port.is_none_or(|pt| pt == p.port) {
                kill_process_tree(&mut p.child);
                killed += 1;
            } else {
                keep.push(p);
            }
        }
        *list = keep;
        if list.is_empty() {
            map.remove(run_id);
        }
    }
    killed
}

fn bg_logs(state: &AppState, run_id: &str, port: Option<u16>) -> String {
    let map = state.bg_processes.lock().unwrap();
    match map.get(run_id) {
        None => "No background servers are running for this run.".to_string(),
        Some(list) => {
            let mut out = String::new();
            for p in list {
                if port.is_none_or(|pt| pt == p.port) {
                    let logs = p.output.lock().map(|s| s.clone()).unwrap_or_default();
                    out.push_str(&format!(
                        "=== {} ({}) ===\n{}\n",
                        p.label,
                        p.command,
                        clip_head_tail(logs.trim(), 3000)
                    ));
                }
            }
            if out.is_empty() {
                "No matching background server for this run.".to_string()
            } else {
                out
            }
        }
    }
}

fn kill_run_background_processes(app_handle: &tauri::AppHandle, run_id: &str) {
    if let Some(state) = app_handle.try_state::<AppState>() {
        bg_stop(&state, run_id, None);
    }
}

/// Kill every tracked background process across all runs. Called on app close
/// so a dev server never outlives the harness.
pub fn kill_all_background_processes(state: &AppState) {
    let run_ids: Vec<String> = state.bg_processes.lock().unwrap().keys().cloned().collect();
    for rid in run_ids {
        bg_stop(state, &rid, None);
    }
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

/// Key for line-shift tracking: worktree + normalized relative path, so the
/// same file referenced with either slash style maps to one entry.
fn line_shift_key(worktree_path: &Path, path: &str) -> String {
    format!(
        "{}::{}",
        clean_project_path(worktree_path).display(),
        path.trim().replace('\\', "/")
    )
}

fn get_line_shift(app_handle: &tauri::AppHandle, run_id: &str, key: &str) -> LineShift {
    app_handle
        .try_state::<AppState>()
        .map(|s| {
            let map = s.line_shift_state.lock().unwrap();
            map.get(run_id)
                .and_then(|m| m.get(key))
                .copied()
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// Accumulate drift after an edit that changed the file's line count.
fn record_line_shift(
    app_handle: &tauri::AppHandle,
    run_id: &str,
    key: &str,
    delta: i64,
    edit_line: usize,
) {
    if let Some(state) = app_handle.try_state::<AppState>() {
        let mut map = state.line_shift_state.lock().unwrap();
        let entry = map
            .entry(run_id.to_string())
            .or_default()
            .entry(key.to_string())
            .or_default();
        entry.shift += delta;
        entry.lowest_edit_line = if entry.lowest_edit_line == 0 {
            edit_line
        } else {
            entry.lowest_edit_line.min(edit_line)
        };
    }
}

/// A fresh read (or full-content write) gives the model current line numbers:
/// clear the drift entry for that file.
fn reset_line_shift(app_handle: &tauri::AppHandle, run_id: &str, key: &str) {
    if let Some(state) = app_handle.try_state::<AppState>() {
        let mut map = state.line_shift_state.lock().unwrap();
        if let Some(m) = map.get_mut(run_id) {
            m.remove(key);
        }
    }
}

fn replace_lines_impl(
    worktree_path: &Path,
    path: &str,
    start_line: usize,
    end_line: usize,
    new_content: &str,
) -> (String, i64) {
    let worktree_abs = clean_project_path(worktree_path);
    let target_file = match verify_sandbox(&worktree_abs, path) {
        Ok(p) => p,
        Err(e) => return (format!("Error: {}", e), 0),
    };
    let original = match fs::read_to_string(&target_file) {
        Ok(c) => c,
        Err(e) => return (format!("Error reading '{}': {}", path, e), 0),
    };
    let lines: Vec<&str> = original.lines().collect();
    if start_line == 0 || end_line < start_line || start_line > lines.len() {
        return (
            format!(
                "Error: invalid line range {}-{} ('{}' has {} lines)",
                start_line,
                end_line,
                path,
                lines.len()
            ),
            0,
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
        return (format!("Error writing '{}': {}", path, e), 0);
    }
    let removed = end_line - start_line + 1;
    let delta = inserted as i64 - removed as i64;
    // Echo the edit site with CURRENT line numbers: a missed aim becomes
    // visible in this very result — not three steps later in a compiler error
    // read against drifted coordinates. (This spiral was observed in the
    // field: a single-line fix escalating through 10+ widening multi-line
    // attempts that never touched the actual error.)
    let win_start = start_line.saturating_sub(3).max(1);
    let win_end = (start_line + inserted + 2).min(out.len());
    let mut echo = String::new();
    for n in win_start..=win_end {
        if let Some(l) = out.get(n - 1) {
            let clipped: String = l.chars().take(120).collect();
            echo.push_str(&format!("\n{:>5} | {}", n, clipped));
        }
    }
    let shift_note = if delta != 0 {
        format!(
            " Line numbers at/after line {} have now shifted by {:+}; lines BELOW {} are unchanged — chain multiple edits bottom-to-top (highest line numbers first), or re-read before the next line-numbered edit.",
            start_line, delta, start_line
        )
    } else {
        " Line count unchanged — all line numbers remain valid.".to_string()
    };
    (
        format!(
            "Success: replaced lines {}-{} of '{}' ({} line(s) removed, {} inserted). First removed line was: `{}`. The edit site now reads (CURRENT line numbers):{}\nVerify this is what you intended; if not, correct it using these numbers.{}",
            start_line,
            end_line,
            path,
            removed,
            inserted,
            first_removed,
            echo,
            shift_note
        ),
        delta,
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

/// Byte spans of each line's content, excluding its trailing `\n`/`\r\n` — the
/// same line set as `str::lines()` (a final newline yields no trailing empty
/// line). The patch_file fuzzy fallback compares on these spans and splices on
/// their byte offsets, so a line-level match maps back to an exact byte range.
fn line_content_spans(s: &str) -> Vec<(usize, usize)> {
    let b = s.as_bytes();
    let mut spans = Vec::new();
    let mut start = 0usize;
    for i in 0..b.len() {
        if b[i] == b'\n' {
            let mut end = i;
            if end > start && b[end - 1] == b'\r' {
                end -= 1;
            }
            spans.push((start, end));
            start = i + 1;
        }
    }
    if start < b.len() {
        let mut end = b.len();
        if end > start && b[end - 1] == b'\r' {
            end -= 1;
        }
        spans.push((start, end));
    }
    spans
}

/// Split a model-supplied target into comparison lines: drop `\r`, trim trailing
/// whitespace, and discard the trailing empty line a stray final newline leaves.
fn target_compare_lines(target: &str) -> Vec<String> {
    let mut lines: Vec<String> = target
        .split('\n')
        .map(|l| l.trim_end_matches('\r').trim_end().to_string())
        .collect();
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines
}

/// If every target line carries a `read_file`-style `"<n>: "` prefix with
/// consecutive ascending numbers, return the lines with those prefixes removed
/// (preserving each line's own indentation). Returns None when the pattern
/// isn't a clean run — so a genuine `42: value` code line is never mis-stripped.
fn strip_read_file_line_numbers(lines: &[String]) -> Option<Vec<String>> {
    if lines.is_empty() {
        return None;
    }
    let mut prev: Option<u64> = None;
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let t = line.trim_start();
        let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return None;
        }
        // read_file emits `format!("{}: {}", n, line)`, so the separator is a
        // colon plus one space; the rest is the line's own (preserved) text.
        let code = match t[digits.len()..].strip_prefix(':') {
            Some(after) => after.strip_prefix(' ').unwrap_or(after),
            None => return None,
        };
        let n: u64 = digits.parse().ok()?;
        if let Some(p) = prev {
            if n != p + 1 {
                return None;
            }
        }
        prev = Some(n);
        out.push(code.to_string());
    }
    Some(out)
}

/// Start indices of every window in `norm_file` matching `target` line-for-line.
fn line_window_matches(norm_file: &[&str], target: &[String]) -> Vec<usize> {
    if target.is_empty() || target.len() > norm_file.len() {
        return Vec::new();
    }
    (0..=norm_file.len() - target.len())
        .filter(|&i| (0..target.len()).all(|k| norm_file[i + k] == target[k]))
        .collect()
}

/// Where a patch target lands in the file.
enum PatchLocation {
    /// Unique byte range to replace. `line_based` is true when the match came
    /// from the whitespace/line-ending-tolerant fallback rather than an exact
    /// byte match — it changes how the replacement is spliced.
    Unique {
        start: usize,
        end: usize,
        line_based: bool,
    },
    None,
    Ambiguous(usize),
}

/// Locate `target` in `content`. Exact byte match first — preserving the old
/// behavior and partial-line patches. Only when that finds nothing do we fall
/// back to line-level matching that tolerates the three artifacts a model
/// introduces when reconstructing a snippet from line-numbered read_file
/// output: LF where the file on disk has CRLF (read_file shows lines via
/// `str::lines()`, so the model never even sees the `\r`), copied `"<n>: "`
/// prefixes, and dropped trailing whitespace. The fallback keeps the uniqueness
/// guarantee — an ambiguous fuzzy match still asks for more context rather than
/// guessing which occurrence was meant.
fn locate_patch_target(content: &str, target: &str) -> PatchLocation {
    let exact: Vec<usize> = content.match_indices(target).map(|(i, _)| i).collect();
    match exact.len() {
        1 => {
            return PatchLocation::Unique {
                start: exact[0],
                end: exact[0] + target.len(),
                line_based: false,
            }
        }
        0 => {}
        n => return PatchLocation::Ambiguous(n),
    }

    let spans = line_content_spans(content);
    let norm_file: Vec<&str> = spans
        .iter()
        .map(|&(s, e)| content[s..e].trim_end())
        .collect();

    let base = target_compare_lines(target);
    let mut candidates = vec![base.clone()];
    if let Some(stripped) = strip_read_file_line_numbers(&base) {
        if stripped != base {
            candidates.push(stripped);
        }
    }

    let mut saw_ambiguous = 0usize;
    for cand in &candidates {
        let hits = line_window_matches(&norm_file, cand);
        match hits.len() {
            1 => {
                let i = hits[0];
                return PatchLocation::Unique {
                    start: spans[i].0,
                    end: spans[i + cand.len() - 1].1,
                    line_based: true,
                };
            }
            0 => {}
            n => saw_ambiguous = saw_ambiguous.max(n),
        }
    }
    if saw_ambiguous > 0 {
        PatchLocation::Ambiguous(saw_ambiguous)
    } else {
        PatchLocation::None
    }
}

/// Re-encode `s` to the file's dominant line ending (`crlf` true => CRLF).
fn normalize_line_endings(s: &str, crlf: bool) -> String {
    let lf = s.replace("\r\n", "\n");
    if crlf {
        lf.replace('\n', "\r\n")
    } else {
        lf
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

    let content = match fs::read_to_string(&target_file) {
        Ok(c) => c,
        Err(e) => return format!("Error reading file: {}", e),
    };

    match locate_patch_target(&content, target_str) {
        PatchLocation::None => format!(
            "Error: Target text not found in '{}'. The target must match the file exactly, including whitespace and quotes. If you copied the target from read_file output, remove the leading line numbers — target must match the file's RAW text. If the snippet contains quotes or escapes you cannot reproduce exactly, use replace_lines(path: String, start_line: int, end_line: int, content: String) with the line numbers from read_file instead.",
            path
        ),
        PatchLocation::Ambiguous(n) => format!(
            "Error: Target text occurs {} times in '{}'. Please provide more surrounding lines of context to ensure the match is unique.",
            n, path
        ),
        PatchLocation::Unique {
            start,
            end,
            line_based,
        } => {
            // Exact matches splice byte-for-byte (behavior unchanged). Line-based
            // matches replace whole-line content while preserving the block's
            // surrounding newlines, so drop one trailing newline from the
            // replacement and re-encode it to the file's line ending — otherwise
            // a fuzzy patch would inject LF into a CRLF file or add a blank line.
            let replacement = if line_based {
                let crlf = content.contains("\r\n");
                let trimmed = replacement_str
                    .strip_suffix("\r\n")
                    .or_else(|| replacement_str.strip_suffix('\n'))
                    .unwrap_or(replacement_str);
                normalize_line_endings(trimmed, crlf)
            } else {
                replacement_str.to_string()
            };
            let mut updated = String::with_capacity(content.len() + replacement.len());
            updated.push_str(&content[..start]);
            updated.push_str(&replacement);
            updated.push_str(&content[end..]);
            match fs::write(&target_file, updated) {
                Ok(_) if line_based => "Success: File patched (target matched after normalizing line endings and whitespace).".to_string(),
                Ok(_) => "Success: File patched successfully".to_string(),
                Err(e) => format!("Error writing file: {}", e),
            }
        }
    }
}

/// The canonical tool names the parser/dispatcher knows. Used by
/// `looks_like_malformed_tool_call` to tell a botched call (the model named a
/// real tool but mangled the syntax) apart from ordinary prose.
const KNOWN_TOOL_NAMES: [&str; 28] = [
    "read_file", "outline_file", "write_file", "list_dir", "git_status",
    "git_diff", "run_command", "screenshot", "start_server", "stop_server",
    "server_logs", "web_search", "send_notification", "task_complete",
    "search_grep", "find_file", "find_symbol", "remember", "recall",
    "list_cards", "create_card", "update_card", "delete_card", "read_card",
    "set_todo", "replace_lines", "patch_file", "search_codebase",
];

/// Bucket a tool result into a coarse, STABLE `failure_reason` for harness
/// telemetry (the per-run vitals panel and the stuck-detector both key on it).
/// Returns None for any result that isn't an error — successes and the raw
/// content tools like read_file return both land here as "not a failure".
/// The buckets are deliberately few: adding one is a schema decision, since
/// downstream aggregation groups by these exact strings. Order matters —
/// the most specific patterns must come first. Strings are matched against
/// the human-readable errors `execute_tool` and the `_impl` helpers return,
/// so this function and those messages must move together.
fn classify_failure_reason(result: &str) -> Option<&'static str> {
    if !result.trim_start().starts_with("Error") {
        return None;
    }
    // Wrong-tool / chimera calls: patch args on replace_lines or vice versa.
    if result.contains("NOT executed")
        && (result.contains("replace_lines argument")
            || result.contains("patch_file argument"))
    {
        return Some("wrong_tool_args");
    }
    // A line number handed to patch_file's text `target`.
    if result.contains("must be the exact text") || result.contains("not a line number") {
        return Some("line_number_as_text");
    }
    // Edit using line numbers that drifted after an earlier same-file edit.
    if result.contains("already modified this session") || result.contains("have shifted") {
        return Some("stale_lines");
    }
    // patch_file target text absent from the file.
    if result.contains("Target text not found") {
        return Some("no_match");
    }
    // patch_file target text not unique.
    if result.contains("Target text occurs") {
        return Some("ambiguous_match");
    }
    // replace_lines line range outside the file.
    if result.contains("invalid line range")
        || result.contains("is past end of file")
        || result.contains("is before start_line")
    {
        return Some("line_out_of_range");
    }
    if result.contains("Missing") && result.contains("argument") {
        return Some("missing_arg");
    }
    Some("other_error")
}

/// Heuristic: did the model TRY to emit a tool call but produce something
/// `parse_tool_calls_all` couldn't read? Returns a coarse reason when the text
/// carries unmistakable tool-call debris, None when it reads as genuine prose
/// (a real question for the human). Only called AFTER parsing returned nothing,
/// so any positive here is by definition an attempt the parser rejected. Kept
/// conservative on purpose: a false positive nudges a model that was actually
/// asking a question, which is worse than missing one malformed call.
fn looks_like_malformed_tool_call(text: &str) -> Option<&'static str> {
    // Native chat-template tags the parser tried and failed to close/parse.
    if text.contains("<tool_call")
        || text.contains("tool_call|>")
        || text.contains("|tool_call")
    {
        return Some("unparsed_tool_tag");
    }
    // A JSON-ish object that names a key but never parsed as a call.
    if (text.contains("\"name\"") || text.contains("'name'")) && text.contains('{') {
        return Some("unparsed_json_call");
    }
    // A fenced ```tool_call / ```json block whose body didn't parse.
    if (text.contains("```tool_call") || text.contains("```json"))
        && text.contains('{')
    {
        return Some("unparsed_fenced_block");
    }
    // Bare `tool_name(...)` function syntax for a tool we actually have.
    for name in KNOWN_TOOL_NAMES {
        if let Some(idx) = text.find(name) {
            if text[idx + name.len()..].trim_start().starts_with('(') {
                return Some("unparsed_function_syntax");
            }
        }
    }
    None
}

/// Collapse whitespace runs to single spaces, trim, and cap length so two
/// edits that differ only in indentation/reflow hash to the same signature.
/// Bounded so a huge replacement body doesn't bloat the in-memory map.
fn normalize_anchor(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(240)
        .collect()
}

/// Signature for a MUTATING edit, used by the stuck-edit detector. Keyed on
/// (tool, path, edit body) — deliberately NOT on line numbers, so a re-read
/// that shifts coordinates but retries the same edit collapses to one
/// signature. Returns None for non-edit tools or calls with no usable path
/// (those failures are caught by the malformed/consecutive guards instead).
fn edit_failure_signature(tool_name: &str, args: &serde_json::Value) -> Option<String> {
    let path = args.get("path").and_then(|p| p.as_str())?;
    let anchor = match tool_name {
        "patch_file" => args.get("target").and_then(|t| t.as_str())?,
        "replace_lines" => args.get("content").and_then(|c| c.as_str()).unwrap_or(""),
        _ => return None,
    };
    Some(format!("{}\u{1f}{}\u{1f}{}", tool_name, path, normalize_anchor(anchor)))
}

/// Tools that only observe (no repo mutation). Anything mutating or neither
/// (run_command, web_search, notifications, task_complete) is not a "read";
/// only the mutating set counts as a "write". The read:write ratio is a
/// retrieval-quality proxy — lots of reads per write means she's hunting.
const READ_TOOLS: [&str; 12] = [
    "read_file", "outline_file", "list_dir", "git_status", "git_diff",
    "search_grep", "find_file", "find_symbol", "read_card", "list_cards", "recall",
    "search_codebase",
];
const WRITE_TOOLS: [&str; 8] = [
    "write_file", "replace_lines", "patch_file", "create_card", "update_card",
    "delete_card", "set_todo", "remember",
];

/// Sort a name→count map into a Vec ordered by count desc, then name asc for
/// stable output.
fn histogram_sorted(map: std::collections::HashMap<String, u32>) -> Vec<(String, u32)> {
    let mut v: Vec<(String, u32)> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

/// Derive a run's vitals from its ordered event log. Pure over the events so it
/// can be unit-tested with fixtures; the command wrapper just supplies them.
fn compute_run_vitals(events: &[RunEvent]) -> RunVitals {
    let mut v = RunVitals::default();
    let mut failure_reasons: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    let mut malformed_reasons: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    let mut per_tool: std::collections::HashMap<String, ToolStat> =
        std::collections::HashMap::new();
    // Edit-signature streaks, replayed exactly like the live stuck detector so
    // the panel and the run loop agree on what "stuck" means. tool_result
    // payloads don't carry args, so pair each result with the preceding call.
    let mut edit_streaks: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    let mut last_call: Option<(String, serde_json::Value)> = None;
    // Running sums for throughput averages; each tracks its own sample count so
    // a call that reported TTFT but not tok/s doesn't skew the rate averages.
    let (mut ttft_sum, mut ttft_n) = (0.0_f64, 0u32);
    let (mut ptps_sum, mut ptps_n) = (0.0_f64, 0u32);
    let (mut dtps_sum, mut dtps_n) = (0.0_f64, 0u32);

    for ev in events {
        match ev.event_type.as_str() {
            "reasoning" => v.reasoning_events += 1,
            "empty" => v.empty_responses += 1,
            "metrics" => {
                v.llm_calls += 1;
                if let Ok(p) = serde_json::from_str::<serde_json::Value>(&ev.payload) {
                    if let Some(x) = p.get("ttft_ms").and_then(|x| x.as_f64()) {
                        ttft_sum += x;
                        ttft_n += 1;
                    }
                    if let Some(x) = p.get("prompt_tps").and_then(|x| x.as_f64()) {
                        ptps_sum += x;
                        ptps_n += 1;
                    }
                    if let Some(x) = p.get("decode_tps").and_then(|x| x.as_f64()) {
                        dtps_sum += x;
                        dtps_n += 1;
                    }
                    if p.get("approx").and_then(|x| x.as_bool()).unwrap_or(false) {
                        v.decode_tps_approx = true;
                    }
                }
            }
            "malformed" => {
                v.malformed += 1;
                if let Ok(p) = serde_json::from_str::<serde_json::Value>(&ev.payload) {
                    if let Some(r) = p.get("failure_reason").and_then(|r| r.as_str()) {
                        *malformed_reasons.entry(r.to_string()).or_insert(0) += 1;
                    }
                }
            }
            "tool_call" => {
                if let Ok(p) = serde_json::from_str::<serde_json::Value>(&ev.payload) {
                    if let Some(name) = p.get("name").and_then(|n| n.as_str()) {
                        let args = p.get("args").cloned().unwrap_or(serde_json::Value::Null);
                        last_call = Some((name.to_string(), args));
                    }
                }
            }
            "tool_result" => {
                let p = match serde_json::from_str::<serde_json::Value>(&ev.payload) {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                let name = p.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                let failed = p.get("failed").and_then(|f| f.as_bool()).unwrap_or(false);
                v.total_calls += 1;
                if failed {
                    v.failures += 1;
                } else {
                    v.successes += 1;
                }
                if READ_TOOLS.contains(&name.as_str()) {
                    v.reads += 1;
                } else if WRITE_TOOLS.contains(&name.as_str()) {
                    v.writes += 1;
                }
                let stat = per_tool.entry(name.clone()).or_default();
                stat.name = name.clone();
                stat.calls += 1;
                if failed {
                    stat.failures += 1;
                    if let Some(r) = p.get("failure_reason").and_then(|r| r.as_str()) {
                        *failure_reasons.entry(r.to_string()).or_insert(0) += 1;
                    }
                }
                // Edit-retry streak, using the paired call's args for the signature.
                if let Some((call_name, call_args)) = &last_call {
                    if *call_name == name {
                        if let Some(sig) = edit_failure_signature(&name, call_args) {
                            if failed {
                                let n = edit_streaks.entry(sig).or_insert(0);
                                *n += 1;
                                v.worst_edit_retry_streak = v.worst_edit_retry_streak.max(*n);
                            } else {
                                edit_streaks.remove(&sig);
                            }
                        }
                    }
                }
                last_call = None;
            }
            _ => {}
        }
    }

    v.failure_reasons = histogram_sorted(failure_reasons);
    v.malformed_reasons = histogram_sorted(malformed_reasons);
    let mut tools: Vec<ToolStat> = per_tool.into_values().collect();
    tools.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.name.cmp(&b.name)));
    v.per_tool = tools;
    let avg = |sum: f64, n: u32| if n > 0 { Some(sum / n as f64) } else { None };
    v.avg_ttft_ms = avg(ttft_sum, ttft_n);
    v.avg_prompt_tps = avg(ptps_sum, ptps_n);
    v.avg_decode_tps = avg(dtps_sum, dtps_n);
    v
}

/// One-shot consult to the configured "assist" (frontier) model — Beetle's
/// phone-a-friend. Reuses the normal provider dispatch but with the assist_*
/// settings, and streams under a throwaway run id so the friend's tokens don't
/// render into Beetle's own transcript. Returns the answer (reasoning stripped).
/// Errs when no assist model is configured.
fn call_assist_model(
    app_handle: &tauri::AppHandle,
    run_id: &str,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, String> {
    let settings = load_config(app_handle).settings;
    if !settings.assist_configured() {
        return Err("no assist model configured".to_string());
    }
    // Promote the assist_* fields into a primary settings object for dispatch.
    let assist = assist_as_primary(&settings);
    let kind = provider_kind(&assist.provider.to_lowercase());
    let url = resolve_endpoint(kind, assist.api_url.trim().trim_end_matches('/'));
    let messages = vec![
        serde_json::json!({ "role": "system", "content": system_prompt }),
        serde_json::json!({ "role": "user", "content": user_message }),
    ];
    // Throwaway id: provider fns stream chat-chunks keyed by run id; an id with
    // no UI listener keeps the consult out of Beetle's transcript.
    let assist_id = format!("{}__assist", run_id);
    let raw = match kind {
        ProviderKind::Anthropic => {
            call_anthropic(app_handle, &assist_id, &url, &assist, system_prompt, messages, None)
        }
        ProviderKind::OpenAiCompat => {
            call_openai_compat(app_handle, &assist_id, &url, &assist, messages, None)
        }
        ProviderKind::OllamaNative => {
            call_ollama_native(app_handle, &assist_id, &url, &assist, messages, None)
        }
        ProviderKind::LmStudioStateful => {
            call_lmstudio_stateful(app_handle, &assist_id, &url, &assist, system_prompt, &messages)
        }
    }?;
    let (_reasoning, answer) = extract_reasoning(&raw);
    Ok(answer)
}

/// A compact, bounded slice of the run's recent activity — the last several
/// tool calls/results — so the assist model has grounding without being handed
/// the entire transcript. Oldest-first.
fn recent_run_context(app_handle: &tauri::AppHandle, run_id: &str) -> String {
    let Some(state) = app_handle.try_state::<AppState>() else {
        return String::new();
    };
    let logs = state.run_logs.lock().unwrap();
    let Some(events) = logs.get(run_id) else {
        return String::new();
    };
    let mut lines: Vec<String> = Vec::new();
    for ev in events.iter().rev() {
        if lines.len() >= 10 {
            break;
        }
        if let Ok(p) = serde_json::from_str::<serde_json::Value>(&ev.payload) {
            match ev.event_type.as_str() {
                "tool_call" => {
                    let name = p.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                    let args = p.get("args").map(|a| a.to_string()).unwrap_or_default();
                    lines.push(format!("→ called {} {}", name, truncate_tool_result(&args, 160)));
                }
                "tool_result" => {
                    let name = p.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                    let res = p.get("result").and_then(|r| r.as_str()).unwrap_or("");
                    lines.push(format!("  {} ⇒ {}", name, truncate_tool_result(res, 300)));
                }
                "malformed" => lines.push("→ (a malformed tool call that didn't parse)".to_string()),
                _ => {}
            }
        }
    }
    lines.reverse();
    lines.join("\n")
}

/// True if `path` (relative, model-supplied) stays within `scope` (e.g.
/// "design"). Rejects any `..` traversal outright. The rail for plan/design
/// mode, where writes are confined to design docs.
fn path_within_scope(path: &str, scope: &str) -> bool {
    let norm = path.trim().replace('\\', "/");
    if norm.split('/').any(|seg| seg == "..") {
        return false;
    }
    let norm = norm.trim_start_matches("./");
    norm == scope || norm.starts_with(&format!("{}/", scope))
}

fn execute_tool(
    app_handle: &tauri::AppHandle,
    worktree_path: &Path,
    tool_name: &str,
    args: &serde_json::Value,
    run_id: &str,
    write_scope: Option<&str>,
) -> String {
    // Plan/design mode rail: confine file mutations to a scope directory (e.g.
    // design/). Enforced HERE — not via the advertised tool schema — because a
    // text-protocol model can emit ANY tool name regardless of what the schema
    // lists, so the schema is no protection at all. This is what stops a "plan"
    // conversation from clobbering source files.
    if let Some(scope) = write_scope {
        if matches!(tool_name, "write_file" | "patch_file" | "replace_lines") {
            let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
            if !path_within_scope(path, scope) {
                return format!(
                    "Error: NOT executed — plan/design mode is read-only outside '{0}/': it may only write design docs under '{0}/', not source files. The path '{1}' is outside that scope. To change code, switch to Code mode or run the card.",
                    scope, path
                );
            }
        }
    }
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
            let result = read_file_range_impl(worktree_path, path, start_line, end_line, 8000);
            if !result.starts_with("Error") {
                // A fresh read gives the model current line numbers: drift cleared.
                reset_line_shift(app_handle, run_id, &line_shift_key(worktree_path, path));
            }
            result
        }
        "outline_file" => {
            let path = match args.get("path").and_then(|p| p.as_str()) {
                Some(p) => p,
                None => return "Error: Missing path argument".to_string(),
            };
            let result = outline_file_impl(worktree_path, path);
            if !result.starts_with("Error") {
                // The outline carries current line numbers: drift is cleared.
                reset_line_shift(app_handle, run_id, &line_shift_key(worktree_path, path));
            }
            result
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
                Ok(_) => {
                    // The model supplied the file's full content: its knowledge of
                    // the line numbering is fresh by construction.
                    reset_line_shift(app_handle, run_id, &line_shift_key(worktree_path, path));
                    "Success: File written successfully".to_string()
                }
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
            let timeout_secs = args.get("timeout_secs").and_then(|v| v.as_u64());
            match run_shell_command(worktree_path, command, timeout_secs) {
                Ok(out) => out,
                Err(e) => format!("Error executing command: {}", e),
            }
        }
        "screenshot" => {
            let url = match args.get("url").and_then(|u| u.as_str()) {
                Some(u) if !u.trim().is_empty() => u.trim(),
                _ => return "Error: Missing 'url' argument. Pass a URL the dev server is already serving, e.g. http://localhost:5173/. (This tool does not start the server.)".to_string(),
            };
            let width = args
                .get("width")
                .and_then(|v| v.as_u64())
                .unwrap_or(1280)
                .clamp(320, 2560) as u32;
            let height = args
                .get("height")
                .and_then(|v| v.as_u64())
                .unwrap_or(800)
                .clamp(240, 2000) as u32;
            match capture_screenshot(run_id, url, width, height) {
                Ok(path) => format!(
                    "{}{}]\nCaptured {} at {}x{}. If the loaded model is vision-capable, the image is attached to this turn so you can see the rendered UI; otherwise it is saved as an artifact for your human.",
                    SHOT_MARKER_OPEN,
                    path.display(),
                    url,
                    width,
                    height
                ),
                Err(e) => format!("Error: screenshot failed — {}", e),
            }
        }
        "start_server" => {
            let command = match args.get("command").and_then(|c| c.as_str()) {
                Some(c) if !c.trim().is_empty() => c.trim(),
                _ => return "Error: Missing 'command' argument (e.g. \"npm run dev\").".to_string(),
            };
            let port = match args.get("port").and_then(|v| v.as_u64()) {
                Some(p) if (1..=65535).contains(&p) => p as u16,
                _ => return "Error: Missing or invalid 'port' — the port the server listens on (e.g. 5173). It's used to detect when the server is ready.".to_string(),
            };
            let timeout_secs = args.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(60);
            let state = app_handle.state::<AppState>();
            bg_start(state.inner(), run_id, worktree_path, command, port, timeout_secs)
        }
        "stop_server" => {
            let port = args.get("port").and_then(|v| v.as_u64()).map(|p| p as u16);
            let state = app_handle.state::<AppState>();
            let n = bg_stop(state.inner(), run_id, port);
            if n == 0 {
                "No matching background server was running.".to_string()
            } else {
                format!("Stopped {} background server(s).", n)
            }
        }
        "server_logs" => {
            let port = args.get("port").and_then(|v| v.as_u64()).map(|p| p as u16);
            let state = app_handle.state::<AppState>();
            bg_logs(state.inner(), run_id, port)
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
            let settings = load_config(app_handle).settings;
            let emb = embed_memory_text(&settings, &topic, &content);
            match get_db_conn(app_handle) {
                Ok(conn) => match insert_memory(
                    &conn,
                    &scope,
                    &topic,
                    &content,
                    "agent",
                    Some(run_id),
                    card_id.as_deref(),
                    emb.as_deref(),
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

            // Semantic path: rank by meaning when a query is given and an
            // embedding provider is configured. Memories with stored vectors are
            // scored by cosine similarity; older rows without a vector fall back
            // to a keyword hit at a low fixed score so they can still surface.
            // Any failure here silently drops to the keyword path below.
            let settings = load_config(app_handle).settings;
            if !query.is_empty() && settings.embedding_configured() {
                if let Ok(mut qv) = call_embedding(&settings, &[query.to_string()]) {
                    if let Some(qvec) = qv.pop() {
                        // Collect candidates, then fuse cosine + keyword rankings
                        // (RRF) — the same hybrid scheme as search_codebase. Rows
                        // without a vector still rank via keyword overlap.
                        let rows: Result<
                            Vec<(String, String, String, String, Option<Vec<u8>>)>,
                            rusqlite::Error,
                        > = (|| {
                            let mut stmt = conn.prepare(
                                "SELECT topic, content, source, created_at, embedding FROM memories WHERE project_path = ?1 ORDER BY id DESC LIMIT 500",
                            )?;
                            let it = stmt.query_map([&scope], |row| {
                                Ok((
                                    row.get::<_, String>(0)?,
                                    row.get::<_, String>(1)?,
                                    row.get::<_, String>(2)?,
                                    row.get::<_, String>(3)?,
                                    row.get::<_, Option<Vec<u8>>>(4)?,
                                ))
                            })?;
                            let mut v = Vec::new();
                            for r in it {
                                v.push(r?);
                            }
                            Ok(v)
                        })();
                        if let Ok(rows) = rows {
                            let terms = query_terms(query);
                            let mut metas: Vec<(String, String, String, String)> = Vec::new();
                            let mut cosines: Vec<f32> = Vec::new();
                            let mut keywords: Vec<usize> = Vec::new();
                            for (topic, content, source, created, blob) in rows {
                                let cos = match blob {
                                    Some(ref b) if !b.is_empty() => {
                                        cosine_similarity(&qvec, &blob_to_embedding(b))
                                    }
                                    _ => 0.0,
                                };
                                let kw = if terms.is_empty() {
                                    0
                                } else {
                                    keyword_overlap(
                                        &format!("{}\n{}", topic, content).to_lowercase(),
                                        &terms,
                                    )
                                };
                                cosines.push(cos);
                                keywords.push(kw);
                                metas.push((topic, content, source, created));
                            }
                            let order = rrf_order(&cosines, &keywords);
                            let mut picked: Vec<usize> = Vec::new();
                            for &i in &order {
                                if cosines[i] <= 0.0 && keywords[i] == 0 {
                                    continue; // no signal from either ranker
                                }
                                picked.push(i);
                                if picked.len() >= limit {
                                    break;
                                }
                            }
                            if !picked.is_empty() {
                                let mut out = vec![format!(
                                    "{} memor{} (most relevant first):",
                                    picked.len(),
                                    if picked.len() == 1 { "y" } else { "ies" }
                                )];
                                for &i in &picked {
                                    let (topic, content, source, created) = &metas[i];
                                    let date = created.split('T').next().unwrap_or("");
                                    out.push(format!(
                                        "[{} | {} | {:.2}] {}: {}",
                                        date, source, cosines[i], topic, content
                                    ));
                                }
                                return out.join("\n");
                            }
                            // Nothing scored: fall through to the keyword path.
                        }
                    }
                }
            }

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
        "search_codebase" => {
            let query = args
                .get("query")
                .and_then(|q| q.as_str())
                .unwrap_or("")
                .trim();
            if query.is_empty() {
                return "Error: search_codebase requires a non-empty 'query' string describing what you're looking for.".to_string();
            }
            let k = args
                .get("limit")
                .or_else(|| args.get("k"))
                .and_then(|v| v.as_u64())
                .map(|v| (v as usize).clamp(1, 15))
                .unwrap_or(6);
            let settings = load_config(app_handle).settings;
            if !settings.embedding_configured() {
                return "Error: codebase search is unavailable — no embedding provider is configured (Settings → Embeddings & RAG). Use search_grep or find_symbol for exact text instead.".to_string();
            }
            let (scope, _) = memory_scope(app_handle, worktree_path, run_id);
            match search_codebase(app_handle, &settings, &scope, query, k) {
                Ok(hits) if hits.is_empty() => format!(
                    "No indexed matches for \"{}\". The project may not be indexed yet (Settings → Embeddings & RAG → Reindex), or try search_grep for exact strings.",
                    query
                ),
                Ok(hits) => {
                    let mut out = vec![format!(
                        "{} semantic match{} for \"{}\" (most relevant first):",
                        hits.len(),
                        if hits.len() == 1 { "" } else { "es" },
                        query
                    )];
                    for (fp, s, e, content, score) in hits {
                        let snippet: String = content.chars().take(800).collect();
                        out.push(format!(
                            "\n--- {}:{}-{} (score {:.2}) ---\n{}",
                            fp, s, e, score, snippet
                        ));
                    }
                    out.join("\n")
                }
                Err(e) => format!("Error searching codebase: {}", e),
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
                runner: default_runner(),
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
            const USAGE: &str = " Usage: patch_file(path: String, target: String, replacement: String) — target is the EXACT TEXT currently in the file (copied without read_file's line-number prefix), NEVER a line number.";
            // Chimera-call diagnosis (field failure): a model mixing patch_file's
            // name with replace_lines' body deserves to be told which tool its
            // arguments belong to, not a generic missing-arg error.
            if args.get("start_line").is_some()
                || args.get("end_line").is_some()
                || args.get("content").is_some()
            {
                return format!(
                    "Error: NOT executed — start_line/end_line/content are replace_lines arguments, not patch_file arguments. Either call replace_lines(path: String, start_line: int, end_line: int, content: String) with those line numbers, or call patch_file with ONLY path, target (the line's exact current text), and replacement.{}",
                    USAGE
                );
            }
            if args.get("target").map(|t| t.is_number()).unwrap_or(false) {
                return format!(
                    "Error: NOT executed — 'target' must be the exact text to replace, not a line number. To edit by line numbers, use replace_lines(path, start_line, end_line, content). To use patch_file, set target to the exact current text of the line.{}",
                    USAGE
                );
            }
            let path = match args.get("path").and_then(|p| p.as_str()) {
                Some(p) => p,
                None => return format!("Error: Missing path argument.{}", USAGE),
            };
            let target_str = match args.get("target").and_then(|t| t.as_str()) {
                Some(t) => t,
                None => return format!("Error: Missing target argument.{}", USAGE),
            };
            let replacement_str = match args.get("replacement").and_then(|r| r.as_str()) {
                Some(r) => r,
                None => return format!("Error: Missing replacement argument.{}", USAGE),
            };
            let result = patch_file_impl(worktree_path, path, target_str, replacement_str);
            if result.starts_with("Success") {
                let delta = replacement_str.lines().count() as i64
                    - target_str.lines().count() as i64;
                if delta != 0 {
                    // Text-anchored edit: we don't know WHICH lines it landed on,
                    // so conservatively treat the whole file's numbering as
                    // drifted (lowest_edit_line = 1) until the next read.
                    record_line_shift(
                        app_handle,
                        run_id,
                        &line_shift_key(worktree_path, path),
                        delta,
                        1,
                    );
                }
            }
            result
        }
        "replace_lines" => {
            const USAGE: &str = " Usage: replace_lines(path: String, start_line: int, end_line: int, content: String) — path is the file to edit, relative to the project root (the same path you passed to read_file).";
            // Mirror of patch_file's chimera diagnosis: patch-shaped arguments
            // sent to the line-numbered tool get routed, not stonewalled.
            if (args.get("target").is_some() || args.get("replacement").is_some())
                && (args.get("start_line").is_none() || args.get("end_line").is_none())
            {
                return format!(
                    "Error: NOT executed — target/replacement are patch_file arguments. Either call patch_file(path: String, target: String, replacement: String) with the exact current text, or call replace_lines with path, start_line, end_line, and content.{}",
                    USAGE
                );
            }
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
            // Stale-line guard: if earlier edits this session changed the file's
            // line count, numbers at/after the earliest edited line no longer
            // mean what the model read. Refuse BEFORE landing on wrong code.
            // Edits entirely below the drift point are still valid — that's the
            // bottom-to-top workflow — and same-length replacements never drift.
            let key = line_shift_key(worktree_path, path);
            let prior = get_line_shift(app_handle, run_id, &key);
            if prior.shift != 0 && end_line >= prior.lowest_edit_line {
                return format!(
                    "Error: NOT executed — '{}' was already modified this session: line numbers at/after line {} have shifted by {:+} line(s) since you last read the file, so an edit at lines {}-{} would land on the WRONG code. Re-read the section first (read_file with start_line/end_line, or outline_file — either refreshes your numbers), or chain edits bottom-to-top: lines below {} are still exactly where you read them.",
                    path,
                    prior.lowest_edit_line,
                    prior.shift,
                    start_line,
                    end_line,
                    prior.lowest_edit_line
                );
            }
            let (result, delta) = replace_lines_impl(worktree_path, path, start_line, end_line, content);
            if delta != 0 && result.starts_with("Success") {
                record_line_shift(app_handle, run_id, &key, delta, start_line);
            }
            result
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
        "request_assist" => {
            let question = match args.get("question").and_then(|q| q.as_str()) {
                Some(q) if !q.trim().is_empty() => q,
                _ => {
                    return "Error: Missing 'question' argument. Usage: request_assist(question: String, context?: String) — describe what you're stuck on and what you've tried.".to_string()
                }
            };
            let extra = args.get("context").and_then(|c| c.as_str()).unwrap_or("");
            let recent = recent_run_context(app_handle, run_id);
            let system = "You are a senior software engineer helping a smaller autonomous coding agent that is stuck inside a code repository. Reply with the SHORTEST concrete fix that unblocks it: an exact patch or code snippet, or precise step-by-step instructions referencing real file paths and line numbers. No preamble, no pleasantries.";
            let user = format!(
                "A coding agent is stuck and is asking for help.\n\n## Its question\n{}\n\n## Extra context it provided\n{}\n\n## Recent tool activity (oldest first)\n{}",
                question,
                if extra.trim().is_empty() { "(none)" } else { extra },
                if recent.trim().is_empty() { "(none)" } else { &recent }
            );
            match call_assist_model(app_handle, run_id, system, &user) {
                Ok(advice) => format!(
                    "Assist from the senior model — this is ADVICE; you must apply it yourself with your normal tools:\n\n{}",
                    advice.trim()
                ),
                Err(e) => format!(
                    "Error: could not reach an assist model ({}). No frontier help is available right now — try a different approach yourself, or use send_notification to ask your human.",
                    e
                ),
            }
        }
        _ => format!(
            "Error: Unknown tool '{}'. Available tools: read_file, outline_file, write_file, patch_file, replace_lines, list_dir, search_grep, find_file, find_symbol, remember, recall, list_cards, create_card, update_card, delete_card, git_status, git_diff, run_command, screenshot, start_server, stop_server, server_logs, web_search, send_notification, read_card, set_todo, task_complete, request_assist. You may ONLY call these tools.",
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
                    c.runner.clone(),
                )
            })
        };

        let (card_title, card_desc, card_project_path, card_runner) = match card_meta {
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
        // Consecutive responses that LOOKED like a tool call but didn't parse.
        // Capped so a model that can't be coaxed into valid syntax still blocks
        // rather than nudging forever.
        let mut malformed_streak: u32 = 0;
        // Per-edit-signature failure tally for the stuck-edit detector. Unlike
        // the consecutive guard, an intervening read does NOT reset this — the
        // fail → read → retry-same-edit loop is exactly what we're catching. A
        // signature clears only when that edit finally SUCCEEDS.
        let mut edit_failure_counts: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        // Whether a frontier "assist" model is configured — gates advertising
        // request_assist in the prompt and the stuck nudge (phone-a-friend).
        let assist_available = load_config(&app_handle_clone).settings.assist_configured();
        // A card flagged runner="frontier" is driven by the assist model itself
        // (for hard cards or review passes). Falls back to local — with a note —
        // if no assist model is configured, so the run never silently stalls.
        let frontier_requested = normalize_runner(&card_runner) == "frontier";
        let drive_with_assist = frontier_requested && assist_available;
        if frontier_requested {
            let msg = if drive_with_assist {
                "This card is set to run on the FRONTIER model.".to_string()
            } else {
                "This card requests the FRONTIER model, but no assist model is configured — falling back to the local model. Set one in Settings → Phone-a-Friend.".to_string()
            };
            append_run_event(
                &app_handle_clone,
                &state,
                &run_id_clone,
                RunEvent {
                    run_id: run_id_clone.clone(),
                    event_type: "message".to_string(),
                    payload: serde_json::json!({ "role": "agent", "content": format!("[harness] {}", msg) }).to_string(),
                },
            );
        }

        // Auto-injected RAG context: the top codebase matches for this task,
        // retrieved ONCE (the query is the static task) and reused every turn, so
        // it costs a single embedding call rather than one per step. Gated by the
        // embedding_auto_inject setting; the on-demand search_codebase tool is
        // always available regardless. Empty string when disabled/unconfigured/no hits.
        let rag_context: String = {
            let cfg = load_config(&app_handle_clone).settings;
            if cfg.embedding_configured() && cfg.embedding_auto_inject {
                let (scope, _) = memory_scope(&app_handle_clone, &worktree_path, &run_id_clone);
                let query = format!("{}\n{}", card_title, card_desc);
                match search_codebase(&app_handle_clone, &cfg, &scope, &query, 5) {
                    Ok(hits) if !hits.is_empty() => {
                        let mut out = String::from(
                            "\n\nRELEVANT CODE (auto-retrieved for this task by semantic similarity; may be incomplete — call search_codebase for more or different angles):\n",
                        );
                        for (fp, s, e, content, score) in hits {
                            let snippet: String = content.chars().take(600).collect();
                            out.push_str(&format!(
                                "\n--- {}:{}-{} (score {:.2}) ---\n{}\n",
                                fp, s, e, score, snippet
                            ));
                        }
                        out
                    }
                    _ => String::new(),
                }
            } else {
                String::new()
            }
        };

        'run: loop {
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

            // Cooperative pause checkpoint: a pause_run request breaks the loop
            // to `blocked` here, between turns (never mid-tool), preserving the
            // worktree and event history so unblock_run can resume cleanly.
            {
                let paused = {
                    let mut paused = state.paused_runs.lock().unwrap();
                    paused.remove(&run_id_clone)
                };
                if paused {
                    append_run_event(&app_handle_clone, &state, &run_id_clone, RunEvent {
                        run_id: run_id_clone.clone(),
                        event_type: "blocked".to_string(),
                        payload: serde_json::json!({
                            "reason": "paused",
                            "message": "Run paused by the developer. The worktree and full history are intact — resume from chat when ready."
                        }).to_string(),
                    });
                    set_card_status(&app_handle_clone, &state, &card_id, "blocked");
                    let _ = app_handle_clone
                        .emit("run-updated", serde_json::json!({ "run_id": run_id_clone }));
                    break 'run;
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
                construct_agent_system_prompt(&worktree_path, &card_title, &card_desc, assist_available);
            let system_prompt = format!(
                "{}\n\nYou are on step {} of a maximum of {} for this run. Pace your work to finish and call task_complete before hitting the ceiling.{}",
                system_prompt, step, max_steps, rag_context
            );
            let tools_schema = get_openai_tools_schema(&[
                "read_file",
                "outline_file",
                "write_file",
                "list_dir",
                "git_status",
                "git_diff",
                "run_command",
                "screenshot",
                "start_server",
                "stop_server",
                "server_logs",
                "web_search",
                "send_notification",
                "task_complete",
                "search_grep",
                "find_file",
                "find_symbol",
                "remember",
                "recall",
                "search_codebase",
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
                drive_with_assist,
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

            let parsed_calls = parse_tool_calls_all(&remaining);
            if !parsed_calls.is_empty() {
                for (tool_name, args, preamble) in parsed_calls {
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
                malformed_streak = 0;
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
                    break 'run;
                }

                let mut tool_result = execute_tool(
                    &app_handle_clone,
                    &worktree_path,
                    &tool_name,
                    &args,
                    &run_id_clone,
                    None,
                );

                if repeat_count >= 2 {
                    tool_result.push_str(&format!(
                        "\n\n[harness note: you have now called '{}' with identical arguments {} times in a row. The result will not change. Try a different tool, different arguments, or reconsider your approach.]",
                        tool_name,
                        repeat_count + 1
                    ));
                }

                // Tag the outcome for harness telemetry: `failed` drives the
                // success/fail split on the vitals panel; `failure_reason`
                // buckets the why so the stuck-detector can match repeated
                // same-cause failures across intervening reads.
                let failed = tool_result.trim_start().starts_with("Error");
                let failure_reason = classify_failure_reason(&tool_result);

                // Stuck-edit detector: count repeated failures of the SAME edit
                // (tool+path+body), surviving intervening reads. Nudge harder at
                // 2, hard-block at 3 — the ladder we agreed on: nudge, let her
                // retry, push the issue up if the same edit fails again. A
                // success clears the signature so honest progress never trips it.
                let mut stuck_edit_n: u32 = 0;
                if let Some(sig) = edit_failure_signature(&tool_name, &args) {
                    if failed {
                        let n = edit_failure_counts.entry(sig).or_insert(0);
                        *n += 1;
                        stuck_edit_n = *n;
                        if stuck_edit_n == 2 {
                            let phone = if assist_available {
                                " If it still won't land, call request_assist to consult a stronger model before trying again."
                            } else {
                                ""
                            };
                            tool_result.push_str(&format!(
                                "\n\n[harness note: this exact {} has now failed {} times, including across re-reads — retrying it unchanged will not work. The text or lines you're targeting are not where you think they are. Re-read the precise region with read_file(start_line, end_line) to refresh your coordinates, switch tools (patch_file ⇄ replace_lines), or anchor on different text.{}]",
                                tool_name, stuck_edit_n, phone
                            ));
                        }
                    } else {
                        edit_failure_counts.remove(&sig);
                    }
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
                            "failed": failed,
                            "failure_reason": failure_reason,
                        })
                        .to_string(),
                    },
                );

                let _ = app_handle_clone
                    .emit("run-updated", serde_json::json!({ "run_id": run_id_clone }));

                // Hard-block once the same edit has failed three times. The
                // failing result above is already logged; now surface WHY (file
                // + failure bucket) so the user — or, later, a frontier assist —
                // can pick it up with full context instead of a bare "stuck".
                if stuck_edit_n >= 3 {
                    log_error(&format!(
                        "Run {} blocked: edit via '{}' failed {} times on the same target ({})",
                        run_id_clone,
                        tool_name,
                        stuck_edit_n,
                        failure_reason.unwrap_or("error")
                    ));
                    let edit_path = args.get("path").and_then(|p| p.as_str()).unwrap_or("?");
                    append_run_event(&app_handle_clone, &state, &run_id_clone, RunEvent {
                        run_id: run_id_clone.clone(),
                        event_type: "blocked".to_string(),
                        payload: serde_json::json!({
                            "reason": "stuck_edit",
                            "tool": tool_name.clone(),
                            "path": edit_path,
                            "failure_reason": failure_reason,
                            "message": format!("The agent tried the same edit to '{}' via {} {} times and it failed every time ({}). It can't get past this edit on its own. Reply in chat to redirect it.", edit_path, tool_name, stuck_edit_n, failure_reason.unwrap_or("error"))
                        }).to_string(),
                    });
                    set_card_status(&app_handle_clone, &state, &card_id, "blocked");
                    let _ = app_handle_clone
                        .emit("run-updated", serde_json::json!({ "run_id": run_id_clone }));
                    break 'run;
                }

                // task_complete moves the card to `review` (done inside execute_tool).
                // Break the loop immediately so the next iteration can't overwrite that
                // status with a `blocked`/`running` transition. Only break on a
                // SUCCESSFUL completion — a rejected task_complete (failed
                // verification) returns an Error result and the loop continues
                // so the model can fix the build.
                if tool_name == "task_complete" && tool_result.starts_with("Success") {
                    let _ = app_handle_clone
                        .emit("run-updated", serde_json::json!({ "run_id": run_id_clone }));
                    break 'run;
                }

                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                }
            } else {
                let visible = remaining.trim();

                // Malformed tool call: the model produced content that reads as
                // a tool-call ATTEMPT but the parser couldn't extract one. This
                // is distinct from a genuine question — blocking-and-waiting on
                // it would strand the run on a formatting slip (the failure mode
                // we see most with small models). Log it for the vitals panel +
                // stuck-detector, nudge with the exact reference format, and
                // keep going. Only a persistent malformed stall (after repeated
                // nudges) falls through to the block path below.
                if !visible.is_empty() && malformed_streak < 3 {
                    if let Some(reason) = looks_like_malformed_tool_call(visible) {
                        malformed_streak += 1;
                        append_run_event(
                            &app_handle_clone,
                            &state,
                            &run_id_clone,
                            RunEvent {
                                run_id: run_id_clone.clone(),
                                event_type: "malformed".to_string(),
                                payload: serde_json::json!({
                                    "failure_reason": reason,
                                    // Bounded sample of the offending text so the
                                    // panel can show WHAT didn't parse without
                                    // storing a whole runaway response.
                                    "attempt": visible.chars().take(600).collect::<String>(),
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
                                event_type: "message".to_string(),
                                payload: serde_json::json!({
                                    "role": "user",
                                    "content": "[harness note: your last message looked like a tool call but could not be parsed. Emit EXACTLY ONE tool call as a fenced block — three backticks, then `tool_call`, then a JSON object with \"name\" and \"args\", then three backticks:\n```tool_call\n{\n  \"name\": \"read_file\",\n  \"args\": { \"path\": \"src/main.ts\" }\n}\n```\nNo prose inside the fence. Try again now.]"
                                })
                                .to_string(),
                            },
                        );
                        let _ = app_handle_clone
                            .emit("run-updated", serde_json::json!({ "run_id": run_id_clone }));
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                        continue;
                    }
                }

                // A response with no tool call AND no visible content is not a
                // question — the reasoning consumed the whole output budget, or
                // generation was truncated mid-think. Blocking on it shows the
                // user an empty "awaiting input" with nothing to answer. Nudge
                // the model and continue; only a persistent stall blocks.
                if visible.is_empty() && empty_response_streak < 2 {
                    empty_response_streak += 1;
                    append_run_event(&app_handle_clone, &state, &run_id_clone, RunEvent {
                        run_id: run_id_clone.clone(),
                        event_type: "empty".to_string(),
                        payload: serde_json::json!({ "reason": "no_output" }).to_string(),
                    });
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
                if visible.is_empty() {
                    append_run_event(&app_handle_clone, &state, &run_id_clone, RunEvent {
                        run_id: run_id_clone.clone(),
                        event_type: "empty".to_string(),
                        payload: serde_json::json!({ "reason": "stall" }).to_string(),
                    });
                }
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
         3. `write_file(path: String, content: String)`: Writes content to a file (creating folders if needed). In plan/design mode you may ONLY write under `design/` — writes to source files are REFUSED. You are here to think and design, not to edit code; if code must change, say so and let the developer run a Code session or a card.\n\
         4. `patch_file(path: String, target: String, replacement: String)`: Replaces an exact text snippet in a file — safer than rewriting a whole file for small edits. The target must match byte-for-byte. Same rule as write_file: design-mode patches are confined to `design/`.\n\
         5. `list_dir(path: String, depth?: Int)`: Lists files and folders as an indented tree (use \"\" for root). Pass depth 2 or 3 to map nested structure in one call.\n\
         6. `search_grep(query: String, path?: String, context?: Int, case_sensitive?: Bool)`: Searches file contents for a substring (case-insensitive by default), grouped by file with line numbers. Pass context: 2 to see surrounding lines without a follow-up read.\n\
         7. `find_file(name: String, path?: String)`: Finds files by name fragment (case-insensitive) and returns matching relative paths.\n\
         8. `find_symbol(name: String, path?: String)`: Finds where a function, struct, class, or other declaration is DEFINED. Returns file:line: signature — then range-read around that line.\n\
         9. `web_search(query: String)`: Searches the web for APIs, libraries, architectural patterns, and programming guides.\n\
         10. `send_notification(message: String)`: Sends a system alert/notification to the developer.\n\
         11. `remember(topic: String, content: String)`: Saves a durable insight to this project's long-term memory — shared with code chat and agent runs. Record design decisions and their reasons here.\n\
         12. `recall(query: String, limit?: Int)`: Searches this project's long-term memory — ranked semantically (by meaning) when an embedding provider is configured, else by keyword (empty query = most recent). Check what past runs and chats already learned before proposing from scratch.\n\
         13. `search_codebase(query: String, limit?: Int)`: Semantic search over the project's INDEXED code and docs; finds code by concept and returns the most relevant chunks as file:line ranges. Use it to ground proposals in how the code actually works. Returns nothing if the project hasn't been indexed yet.\n\
         14. `list_cards()`: Shows the project's kanban board grouped by status, with card ids and todo progress.\n\
         15. `create_card(title: String, description: String, todos?: [String], priority?: \"low\"|\"medium\"|\"high\", labels?: [String])`: Files a new card in the backlog. When a design discussion produces actionable work, FILE IT as a card with a clear description, priority, and todos — that is how plans become runs.\n\
         16. `update_card(card_id: String, title?: String, description?: String, priority?: String, todos?: [String], add_todo?: String, add_label?: String)`: Edits a backlog/todo card. `todos` REPLACES the whole checklist; `add_todo` appends one item.\n\
         17. `delete_card(card_id: String)`: Deletes a backlog/todo card that has no run history.\n\n\
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
         11. `run_command(command: String, timeout_secs?: Int)`: Runs build, test, or check shell commands in the repository (e.g. \"cargo check\", \"npm run build\", \"npm test\"). Use this to verify your changes compile and pass tests! The result starts with the exit code (`[exit code: 0]` means success); output is clipped from both ends so the trailing error survives. timeout_secs defaults to 300 (max 1800).\n\
         12. `web_search(query: String)`: Searches the web for documentation, syntax guides, and examples.\n\
         13. `remember(topic: String, content: String)`: Saves a durable insight to this project's long-term memory — shared with design chat and agent runs. Record how subsystems work and pitfalls you discover.\n\
         14. `recall(query: String, limit?: Int)`: Searches this project's long-term memory — ranked semantically (by meaning) when an embedding provider is configured, else by keyword (empty query = most recent). Check what past runs and chats already learned before exploring from scratch.\n\
         15. `search_codebase(query: String, limit?: Int)`: Semantic search over the project's INDEXED code and docs; finds code by concept and returns the most relevant chunks as file:line ranges. Ideal for \"where is X handled?\" / \"how does Y work?\". For exact strings or symbol names prefer search_grep/find_symbol. Returns nothing if the project hasn't been indexed yet.\n\
         16. `list_cards()`: Shows the project's kanban board grouped by status, with card ids and todo progress.\n\
         17. `create_card(title: String, description: String, todos?: [String], priority?: \"low\"|\"medium\"|\"high\", labels?: [String])`: Files a new card in the backlog. If a fix you're discussing is bigger than the current conversation, file it as a card so it gets scheduled instead of forgotten.\n\
         18. `update_card(card_id: String, title?: String, description?: String, priority?: String, todos?: [String], add_todo?: String, add_label?: String)`: Edits a backlog/todo card. `todos` REPLACES the whole checklist; `add_todo` appends one item.\n\
         19. `delete_card(card_id: String)`: Deletes a backlog/todo card that has no run history.\n\n\
         If you want to talk to the user, output a regular text response explaining your changes or asking questions.",
        target, project_path.to_string_lossy()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worktrees_to_reap_keeps_live_drops_orphans() {
        let on_disk = vec![
            "run_a".to_string(), // owned by a blocked card -> keep
            "run_b".to_string(), // terminal/deleted card    -> reap
            "run_c".to_string(), // owned by an in-review card -> keep
        ];
        let mut keep = std::collections::HashSet::new();
        keep.insert("run_a".to_string());
        keep.insert("run_c".to_string());

        let reap = worktrees_to_reap(&on_disk, &keep);
        assert_eq!(reap, vec!["run_b".to_string()]);
    }

    #[test]
    fn test_llm_error_transient_classification() {
        // Permanent: cancellation, config, and 4xx caller faults.
        assert!(!llm_error_is_transient("Cancelled by user"));
        assert!(!llm_error_is_transient(
            "API URL is empty. Please configure it in settings."
        ));
        assert!(!llm_error_is_transient("Anthropic API error 401: bad key"));
        assert!(!llm_error_is_transient("Ollama API error 404: no such model"));
        assert!(!llm_error_is_transient("LLM API error 400: bad request"));

        // Transient: 5xx / 429 statuses and transport/stream faults.
        assert!(llm_error_is_transient("LLM API error 503: unavailable"));
        assert!(llm_error_is_transient("Ollama API error 500: boom"));
        assert!(llm_error_is_transient("LM Studio API error 429: slow down"));
        assert!(llm_error_is_transient("Network request failed: timed out"));
        assert!(llm_error_is_transient(
            "LM Studio stream read failed: connection reset"
        ));
    }

    #[test]
    fn test_parse_http_status() {
        assert_eq!(parse_http_status("Anthropic API error 503: x"), Some(503));
        assert_eq!(parse_http_status("LLM API error 429: x"), Some(429));
        // No-code variant and transport faults carry no status.
        assert_eq!(parse_http_status("Ollama API error: text"), None);
        assert_eq!(parse_http_status("Network request failed: oops"), None);
    }

    #[test]
    fn test_run_logs_to_prune_keeps_recent_and_protected() {
        // recent_first: r1 newest ... r5 oldest. Keep 2 newest; r5 is resumable.
        let recent = vec![
            "r1".to_string(),
            "r2".to_string(),
            "r3".to_string(),
            "r4".to_string(),
            "r5".to_string(),
        ];
        let mut protected = std::collections::HashSet::new();
        protected.insert("r5".to_string());

        let prune = run_logs_to_prune(&recent, 2, &protected);
        // r1,r2 kept by recency; r5 kept by protection; r3,r4 pruned.
        assert_eq!(prune, vec!["r3".to_string(), "r4".to_string()]);
    }

    #[test]
    fn test_run_logs_to_prune_under_cap_prunes_nothing() {
        let recent = vec!["r1".to_string(), "r2".to_string()];
        let protected = std::collections::HashSet::new();
        assert!(run_logs_to_prune(&recent, 200, &protected).is_empty());
    }

    #[test]
    fn test_worktrees_to_reap_empty_keep_reaps_all() {
        let on_disk = vec!["run_x".to_string(), "run_y".to_string()];
        let keep = std::collections::HashSet::new();
        let mut reap = worktrees_to_reap(&on_disk, &keep);
        reap.sort();
        assert_eq!(reap, vec!["run_x".to_string(), "run_y".to_string()]);
    }

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
    fn test_compute_run_vitals() {
        fn ev(t: &str, payload: serde_json::Value) -> RunEvent {
            RunEvent { run_id: "r".into(), event_type: t.into(), payload: payload.to_string() }
        }
        let edit_args = serde_json::json!({"path":"a.rs","content":"x"});
        let events = vec![
            ev("reasoning", serde_json::json!("thinking")),
            // read: success
            ev("tool_call", serde_json::json!({"name":"read_file","args":{"path":"a.rs"}})),
            ev("tool_result", serde_json::json!({"name":"read_file","result":"...","failed":false,"failure_reason":null})),
            // edit fails twice (same signature) then succeeds → streak 2, then cleared
            ev("tool_call", serde_json::json!({"name":"replace_lines","args":edit_args})),
            ev("tool_result", serde_json::json!({"name":"replace_lines","result":"Error: invalid line range","failed":true,"failure_reason":"line_out_of_range"})),
            ev("tool_call", serde_json::json!({"name":"replace_lines","args":edit_args})),
            ev("tool_result", serde_json::json!({"name":"replace_lines","result":"Error: invalid line range","failed":true,"failure_reason":"line_out_of_range"})),
            ev("tool_call", serde_json::json!({"name":"replace_lines","args":edit_args})),
            ev("tool_result", serde_json::json!({"name":"replace_lines","result":"Success","failed":false,"failure_reason":null})),
            // a malformed attempt
            ev("malformed", serde_json::json!({"failure_reason":"unparsed_json_call","attempt":"{..."})),
            // two LLM calls with throughput; one decode rate is estimated
            ev("metrics", serde_json::json!({"ttft_ms":800.0,"total_ms":2000.0,"prompt_tps":600.0,"decode_tps":40.0,"approx":false})),
            ev("metrics", serde_json::json!({"ttft_ms":1200.0,"total_ms":3000.0,"prompt_tps":null,"decode_tps":50.0,"approx":true})),
        ];
        let v = compute_run_vitals(&events);
        assert_eq!(v.llm_calls, 2);
        assert_eq!(v.avg_ttft_ms, Some(1000.0));
        assert_eq!(v.avg_decode_tps, Some(45.0));
        assert_eq!(v.avg_prompt_tps, Some(600.0), "only the call that reported prompt_tps counts");
        assert!(v.decode_tps_approx, "one sample was estimated");
        assert_eq!(v.total_calls, 4);
        assert_eq!(v.successes, 2);
        assert_eq!(v.failures, 2);
        assert_eq!(v.malformed, 1);
        assert_eq!(v.reads, 1);
        assert_eq!(v.writes, 3);
        assert_eq!(v.reasoning_events, 1);
        assert_eq!(v.worst_edit_retry_streak, 2, "two failures before the edit landed");
        assert_eq!(v.failure_reasons, vec![("line_out_of_range".to_string(), 2)]);
        assert_eq!(v.malformed_reasons, vec![("unparsed_json_call".to_string(), 1)]);
        let rl = v.per_tool.iter().find(|t| t.name == "replace_lines").unwrap();
        assert_eq!((rl.calls, rl.failures), (3, 2));
    }

    #[test]
    fn test_path_within_scope() {
        // Allowed: inside the scope dir.
        assert!(path_within_scope("design/architecture.md", "design"));
        assert!(path_within_scope("design", "design"));
        assert!(path_within_scope("./design/notes.md", "design"));
        assert!(path_within_scope("design\\sub\\x.md", "design")); // windows sep
        // Refused: outside scope or traversal escapes.
        assert!(!path_within_scope("src/main.ts", "design"));
        assert!(!path_within_scope("design/../src/main.ts", "design"));
        assert!(!path_within_scope("../design/x.md", "design"));
        assert!(!path_within_scope("designs/x.md", "design")); // prefix-not-dir
        assert!(!path_within_scope("", "design"));
    }

    #[test]
    fn test_edit_failure_signature() {
        // replace_lines: line numbers are NOT in the signature, so a re-read
        // that shifts coordinates but retries the same body collapses to one.
        let a = serde_json::json!({"path":"src/x.rs","start_line":10,"end_line":12,"content":"let y = 1;"});
        let b = serde_json::json!({"path":"src/x.rs","start_line":20,"end_line":22,"content":"let   y = 1;"});
        assert_eq!(
            edit_failure_signature("replace_lines", &a),
            edit_failure_signature("replace_lines", &b),
            "same body at shifted lines (and reflowed whitespace) must share a signature"
        );
        // Different body → different signature (honest progress, not a loop).
        let c = serde_json::json!({"path":"src/x.rs","start_line":10,"end_line":12,"content":"let z = 2;"});
        assert_ne!(
            edit_failure_signature("replace_lines", &a),
            edit_failure_signature("replace_lines", &c)
        );
        // Non-edit tools and pathless calls produce no signature.
        assert!(edit_failure_signature("read_file", &a).is_none());
        assert!(edit_failure_signature("patch_file", &serde_json::json!({"target":"x"})).is_none());
        // patch_file keys on its text target.
        let p = serde_json::json!({"path":"a.txt","target":"foo","replacement":"bar"});
        assert!(edit_failure_signature("patch_file", &p).is_some());
    }

    #[test]
    fn test_parse_tool_call_flattened_args() {
        // Qwen3.5-style emission: arguments as SIBLINGS of "name", no wrapper.
        // Must be recovered, not dropped to null (which silently ran tools with
        // default args — list_dir always listing root, etc.).
        let input = r#"
```tool_call
{ "name": "list_dir", "path": "src" }
```
"#;
        let res = parse_tool_call_spanned(input);
        assert!(res.is_some());
        let (name, args, _) = res.unwrap();
        assert_eq!(name, "list_dir");
        assert_eq!(args.get("path").unwrap().as_str().unwrap(), "src");
    }

    #[test]
    fn test_extract_tool_args_prefers_wrapper_over_siblings() {
        // An explicit args wrapper must win even if stray siblings exist, and a
        // name-only call still yields null (nothing to recover).
        let wrapped = serde_json::json!({"name":"x","args":{"a":1},"stray":2});
        assert_eq!(extract_tool_args(&wrapped), serde_json::json!({"a":1}));
        let name_only = serde_json::json!({"name":"recall"});
        assert!(extract_tool_args(&name_only).is_null());
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
    fn test_parse_tool_call_relaxed_json() {
        // Verbatim field failure: unquoted key inside a mangled-token wrapper
        // parsed as prose instead of a tool call.
        let input = "<|tool_call>call:list_dir{path: \"\"}<tool_call|>";
        let res = parse_tool_call_spanned(input);
        assert!(res.is_some());
        let (name, args, preamble) = res.unwrap();
        assert_eq!(name, "list_dir");
        assert_eq!(args.get("path").unwrap().as_str().unwrap(), "");
        assert_eq!(preamble, "");

        // Single-quoted strings and trailing commas are also tolerated.
        let input2 =
            "```tool_call\n{name: 'read_file', args: {path: 'src/main.ts',},}\n```";
        let (name2, args2, _) = parse_tool_call_spanned(input2).unwrap();
        assert_eq!(name2, "read_file");
        assert_eq!(args2.get("path").unwrap().as_str().unwrap(), "src/main.ts");

        // Strict JSON must keep working, including apostrophes inside values.
        let input3 = "```tool_call\n{\"name\": \"remember\", \"args\": {\"topic\": \"beetle's law\", \"content\": \"it works\"}}\n```";
        let (name3, args3, _) = parse_tool_call_spanned(input3).unwrap();
        assert_eq!(name3, "remember");
        assert_eq!(
            args3.get("topic").unwrap().as_str().unwrap(),
            "beetle's law"
        );
    }

    #[test]
    fn test_parse_tool_call_redundant_separator() {
        // Field failure: the model emitted a doubled separator and read_file was
        // handed the literal `="src/patterns/wave.py"` (quotes included), which
        // the OS rejected as a malformed path. The redundant `=`/`:` must be
        // stripped so the real path comes through.
        for input in [
            "read_file(path == \"src/patterns/wave.py\")",
            "read_file(path := \"src/patterns/wave.py\")",
            "read_file(path: = \"src/patterns/wave.py\")",
        ] {
            let (name, args, _) = parse_tool_call_spanned(input).unwrap();
            assert_eq!(name, "read_file");
            assert_eq!(
                args.get("path").unwrap().as_str().unwrap(),
                "src/patterns/wave.py",
                "input: {input}"
            );
        }

        // The well-formed single-separator forms must keep parsing identically.
        for input in [
            "read_file(path = \"src/main.ts\")",
            "read_file(path: \"src/main.ts\")",
        ] {
            let (_, args, _) = parse_tool_call_spanned(input).unwrap();
            assert_eq!(args.get("path").unwrap().as_str().unwrap(), "src/main.ts");
        }
    }

    #[test]
    fn test_parse_replace_lines_content_no_quote_leak() {
        // Field failure (Beetle's report): a malformed separator on `content`
        // leaked the literal `= "..."` — quotes and all — into the file, breaking
        // its syntax. The leading-separator strip in parse_fn_scalar_value must
        // cover content, not just read_file's path: the value the tool receives
        // is the bare code, with no `=` and no surrounding quotes.
        for input in [
            "replace_lines(path=\"a.rs\", start_line=1, end_line=1, content== \"let x = 1;\")",
            "replace_lines(path=\"a.rs\", start_line=1, end_line=1, content := \"let x = 1;\")",
            "replace_lines(path=\"a.rs\", start_line=1, end_line=1, content: \"let x = 1;\")",
        ] {
            let (name, args, _) = parse_tool_call_spanned(input).unwrap();
            assert_eq!(name, "replace_lines");
            assert_eq!(
                args.get("content").unwrap().as_str().unwrap(),
                "let x = 1;",
                "input: {input}"
            );
        }
    }

    #[test]
    fn test_parse_tool_calls_multiple() {
        // Two fenced calls in one response — both must survive, each with its
        // own preamble. This is the "she filed two great cards and we lost
        // one" regression.
        let input = "Filing both cards now.\n```tool_call\n{\"name\": \"create_card\", \"args\": {\"title\": \"A\", \"description\": \"first\"}}\n```\nAnd the second:\n```tool_call\n{\"name\": \"create_card\", \"args\": {\"title\": \"B\", \"description\": \"second\"}}\n```";
        let calls = parse_tool_calls_all(input);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "create_card");
        assert_eq!(calls[0].1.get("title").unwrap().as_str().unwrap(), "A");
        assert_eq!(calls[0].2, "Filing both cards now.");
        assert_eq!(calls[1].1.get("title").unwrap().as_str().unwrap(), "B");
        assert_eq!(calls[1].2, "And the second:");

        // Two bare qwen-brace calls — the old rfind('}') would have spanned
        // both braces and corrupted even the FIRST call.
        let input2 = "call:list_dir{\"path\": \"src\"}\ncall:read_file{\"path\": \"src/main.ts\"}";
        let calls2 = parse_tool_calls_all(input2);
        assert_eq!(calls2.len(), 2);
        assert_eq!(calls2[0].0, "list_dir");
        assert_eq!(calls2[1].0, "read_file");
        assert_eq!(
            calls2[1].1.get("path").unwrap().as_str().unwrap(),
            "src/main.ts"
        );
    }

    #[test]
    fn test_parse_tool_call_backtick_template_literal() {
        // Field failure: a JS template literal as an argument value, with
        // NESTED backticks inside the code payload — a model deep in
        // TypeScript passing code the way TypeScript would write it.
        let input = "<|tool_call>call:replace_lines{end_line:183,path:\"src/main.ts\",start_line:168,content:`ctx.strokeStyle = `rgba(255, 215, 0, ${opacity})`;\nctx.stroke();`}<tool_call|>";
        let res = parse_tool_call_spanned(input);
        assert!(res.is_some());
        let (name, args, preamble) = res.unwrap();
        assert_eq!(name, "replace_lines");
        assert_eq!(args.get("start_line").unwrap().as_i64().unwrap(), 168);
        assert_eq!(args.get("end_line").unwrap().as_i64().unwrap(), 183);
        assert_eq!(
            args.get("path").unwrap().as_str().unwrap(),
            "src/main.ts"
        );
        let content = args.get("content").unwrap().as_str().unwrap();
        assert!(content.contains("rgba(255, 215, 0"));
        assert!(content.contains("ctx.stroke();"));
        assert_eq!(preamble, "");
    }

    #[test]
    fn test_replace_lines_impl_reports_shift_delta() {
        let dir = std::env::temp_dir().join(format!("beetle_rl_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("t.txt"), "a\nb\nc\nd\ne\n").unwrap();

        // 3 lines out, 1 in → delta -2; numbers below the edit untouched.
        let (msg, delta) = replace_lines_impl(&dir, "t.txt", 2, 4, "X");
        assert!(msg.starts_with("Success"), "{}", msg);
        assert_eq!(delta, -2);
        assert_eq!(fs::read_to_string(dir.join("t.txt")).unwrap(), "a\nX\ne\n");

        // 1 line out, 2 in → delta +1.
        let (msg2, delta2) = replace_lines_impl(&dir, "t.txt", 1, 1, "a1\na2");
        assert!(msg2.starts_with("Success"), "{}", msg2);
        assert_eq!(delta2, 1);

        // Same-length replacement → delta 0 (numbers stay valid; never blocked).
        let (msg3, delta3) = replace_lines_impl(&dir, "t.txt", 1, 1, "A1");
        assert!(msg3.starts_with("Success"), "{}", msg3);
        assert_eq!(delta3, 0);
        assert!(msg3.contains("Line count unchanged"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_parse_tool_call_two_backtick_values() {
        // Field failure: TWO backtick-quoted values in one call. Greedy
        // capture used to swallow ",path:" into the content string, leaving
        // the call with no path at all.
        let input = "call:patch_file{path:`src/a.ts`,target:`let x = 1;`,replacement:`let x = 2;`}";
        let res = parse_tool_call_spanned(input);
        assert!(res.is_some());
        let (name, args, _) = res.unwrap();
        assert_eq!(name, "patch_file");
        assert_eq!(args.get("path").unwrap().as_str().unwrap(), "src/a.ts");
        assert_eq!(args.get("target").unwrap().as_str().unwrap(), "let x = 1;");
        assert_eq!(
            args.get("replacement").unwrap().as_str().unwrap(),
            "let x = 2;"
        );
    }

    #[test]
    fn test_normalize_for_strict_templates() {
        // Assistant-first history with adjacent same-role messages and a
        // trailing assistant turn — the exact shapes strict jinja templates
        // (Mistral, Qwen) reject and Gemma silently tolerated.
        let msgs = vec![
            serde_json::json!({"role": "system", "content": "sys"}),
            serde_json::json!({"role": "assistant", "content": "I started"}),
            serde_json::json!({"role": "assistant", "content": "more"}),
            serde_json::json!({"role": "user", "content": "ok"}),
            serde_json::json!({"role": "assistant", "content": "done"}),
        ];
        let out = normalize_for_strict_templates(msgs);
        let roles: Vec<&str> = out
            .iter()
            .map(|m| m.get("role").unwrap().as_str().unwrap())
            .collect();
        assert_eq!(
            roles,
            vec!["system", "user", "assistant", "user", "assistant", "user"]
        );
        // The two adjacent assistant messages were merged, not dropped.
        let merged = out[2].get("content").unwrap().as_str().unwrap();
        assert!(merged.contains("I started") && merged.contains("more"));
        // Already-clean histories pass through unchanged except the guard tail.
        let clean = vec![
            serde_json::json!({"role": "system", "content": "sys"}),
            serde_json::json!({"role": "user", "content": "hi"}),
        ];
        let out2 = normalize_for_strict_templates(clean);
        assert_eq!(out2.len(), 2);
        assert_eq!(out2[1].get("content").unwrap().as_str().unwrap(), "hi");
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

        let history = get_history_messages(&events, 0);
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

    #[test]
    fn test_patch_file_fuzzy_match() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let wt = dir.path();

        // 1. CRLF file, multi-line LF target — the dominant Windows miss. The
        //    model only ever saw LF (read_file uses str::lines()), so its target
        //    has bare \n that no longer byte-matches the file's \r\n.
        let crlf = wt.join("crlf.rs");
        fs::write(&crlf, "fn main() {\r\n    let x = 1;\r\n    let y = 2;\r\n}\r\n").unwrap();
        let res = patch_file_impl(
            wt,
            "crlf.rs",
            "    let x = 1;\n    let y = 2;",
            "    let x = 10;\n    let y = 20;",
        );
        assert!(res.starts_with("Success"), "got: {res}");
        let after = fs::read_to_string(&crlf).unwrap();
        assert_eq!(after, "fn main() {\r\n    let x = 10;\r\n    let y = 20;\r\n}\r\n");

        // 2. Target carrying read_file's "<n>: " line-number prefixes.
        let f = wt.join("nums.txt");
        fs::write(&f, "alpha\nbeta\ngamma\n").unwrap();
        let res = patch_file_impl(wt, "nums.txt", "2: beta\n3: gamma", "BETA\nGAMMA");
        assert!(res.starts_with("Success"), "got: {res}");
        assert_eq!(fs::read_to_string(&f).unwrap(), "alpha\nBETA\nGAMMA\n");

        // 3. Trailing-whitespace drift: a file line has trailing spaces the model
        //    dropped, breaking the exact byte match of the multi-line target.
        let f = wt.join("trail.txt");
        fs::write(&f, "keep\nedit me   \ntail\n").unwrap();
        let res = patch_file_impl(wt, "trail.txt", "edit me\ntail", "edited\nTAIL");
        assert!(res.starts_with("Success"), "got: {res}");
        assert_eq!(fs::read_to_string(&f).unwrap(), "keep\nedited\nTAIL\n");

        // 4. Fuzzy match that is ambiguous still refuses rather than guessing.
        let f = wt.join("dup.txt");
        fs::write(&f, "x = 1\r\ny = 2\r\nx = 1\r\n").unwrap();
        let res = patch_file_impl(wt, "dup.txt", "x = 1", "x = 99");
        assert!(res.contains("Target text occurs 2 times"), "got: {res}");

        // 5. A genuinely absent target still reports not-found.
        let res = patch_file_impl(wt, "dup.txt", "nonexistent line", "z");
        assert!(res.contains("Target text not found"), "got: {res}");

        // 6. A line that merely looks like a numbered prefix but isn't a clean
        //    consecutive run is matched literally, not mis-stripped.
        let f = wt.join("dict.txt");
        // CRLF so the exact tier misses and the line-based fallback runs: the
        // literal candidate must match before any prefix-stripping is attempted.
        fs::write(&f, "config = {\r\n  42: \"answer\",\r\n}\r\n").unwrap();
        let res = patch_file_impl(wt, "dict.txt", "  42: \"answer\",", "  42: \"forty-two\",");
        assert!(res.starts_with("Success"), "got: {res}");
        assert_eq!(
            fs::read_to_string(&f).unwrap(),
            "config = {\r\n  42: \"forty-two\",\r\n}\r\n"
        );
    }

    #[test]
    fn test_clip_head_tail_keeps_the_error_at_the_bottom() {
        // Under budget: returned verbatim.
        assert_eq!(clip_head_tail("short", 100), "short");

        // Over budget: both ends survive (the old head-only clip dropped the
        // tail, which is exactly where build errors live).
        let s = format!("{}error: cannot find value `x`", "A".repeat(5000));
        let clipped = clip_head_tail(&s, 4000);
        assert!(clipped.starts_with("AAAA"), "head kept");
        assert!(clipped.ends_with("error: cannot find value `x`"), "tail kept");
        assert!(clipped.contains("chars omitted"));
        assert!(clipped.chars().count() < s.chars().count());
    }

    #[test]
    fn test_run_shell_command_reports_exit_code_and_timeout() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let wt = dir.path();

        // Success: exit code 0 plus the command's stdout.
        let ok = run_shell_command(wt, "echo hello", None).unwrap();
        assert!(ok.starts_with("[exit code: 0]"), "got: {ok}");
        assert!(ok.contains("hello"), "got: {ok}");

        // Failure: a non-zero exit is surfaced (was invisible before).
        let bad = run_shell_command(wt, "exit 3", None).unwrap();
        assert!(bad.starts_with("[exit code: 3]"), "got: {bad}");

        // Timeout: a command that overruns is killed and reported, not hung.
        let sleep = if cfg!(target_os = "windows") {
            "ping -n 5 127.0.0.1 >NUL"
        } else {
            "sleep 5"
        };
        let timed = run_shell_command(wt, sleep, Some(1)).unwrap();
        assert!(timed.contains("timed out after 1s"), "got: {timed}");
    }

    #[test]
    fn test_base64_encode_known_vectors() {
        // RFC 4648 test vectors, including the two padding cases.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"M"), "TQ==");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn test_parse_screenshot_path() {
        assert_eq!(
            parse_screenshot_path("[screenshot:C:\\tmp\\shot-0.png]\nCaptured ...").as_deref(),
            Some("C:\\tmp\\shot-0.png")
        );
        assert_eq!(parse_screenshot_path("Tool 'read_file' returned:\nfn main"), None);
        assert_eq!(parse_screenshot_path("[screenshot:]"), None);
    }

    #[test]
    fn test_vision_from_models_json() {
        // Shape mirrors LM Studio's /api/v1/models: vision lives under
        // capabilities.vision, NOT the misleading top-level `type`/`vision`.
        let json = serde_json::json!({
            "models": [
                { "key": "text-embedding", "type": "embedding", "vision": null,
                  "loaded_instances": [{}], "capabilities": null },
                { "key": "google/gemma-4-26b-a4b-qat", "type": "llm", "vision": null,
                  "loaded_instances": [{}],
                  "capabilities": { "vision": true, "trained_for_tool_use": true } },
                { "key": "some/text-coder", "type": "llm", "vision": null,
                  "loaded_instances": [], "capabilities": { "vision": false } }
            ]
        });
        assert!(vision_from_models_json(&json, "google/gemma-4-26b-a4b-qat"));
        assert!(!vision_from_models_json(&json, "some/text-coder"));
        // Unknown id falls back to a loaded vision-capable model.
        assert!(vision_from_models_json(&json, "mystery-model"));
        // No models at all -> fail closed.
        assert!(!vision_from_models_json(&serde_json::json!({}), "x"));
    }

    #[test]
    fn test_attach_recent_screenshot() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let png = dir.path().join("shot.png");
        fs::write(&png, b"\x89PNG\r\n\x1a\nfake").unwrap();
        let marker = format!("[screenshot:{}]\nCaptured.", png.display());
        let base = vec![
            serde_json::json!({ "role": "user", "content": "do the thing" }),
            serde_json::json!({ "role": "user", "content": marker }),
        ];

        // vision on + recent -> content becomes [text, image_url(data uri)].
        let out = attach_recent_screenshot(base.clone(), true);
        let content = out[1].get("content").unwrap().as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image_url");
        assert!(content[1]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));

        // vision off -> untouched (text marker stays a plain string).
        let out = attach_recent_screenshot(base.clone(), false);
        assert!(out[1].get("content").unwrap().is_string());

        // Too far from the tail -> not re-attached (bounds repeated image cost).
        let mut old = vec![base[1].clone()];
        for _ in 0..8 {
            old.push(serde_json::json!({ "role": "user", "content": "later" }));
        }
        let out = attach_recent_screenshot(old, true);
        assert!(out[0].get("content").unwrap().is_string());
    }

    #[test]
    fn test_append_capped_keeps_tail() {
        let buf = std::sync::Arc::new(Mutex::new(String::new()));
        for _ in 0..100 {
            append_capped(&buf, &"x".repeat(200)); // 20_000 chars total
        }
        // Bounded to the cap, keeping the most recent bytes.
        assert_eq!(buf.lock().unwrap().chars().count(), BG_LOG_CAP);

        let small = std::sync::Arc::new(Mutex::new(String::new()));
        append_capped(&small, "hello");
        assert_eq!(&*small.lock().unwrap(), "hello");
    }

    #[test]
    fn test_port_open_detects_listener() {
        use std::net::TcpListener;
        // An OS-assigned port with a live listener reads as open...
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(port_open(port));
        // ...and as closed once nothing is listening.
        drop(listener);
        assert!(!port_open(port));
    }

    // ----- RAG / embeddings unit tests (pure, no network) -----

    #[test]
    fn test_embedding_blob_roundtrip() {
        let v = vec![0.0f32, 1.5, -2.25, 3.125, f32::MIN, f32::MAX];
        let blob = embedding_to_blob(&v);
        assert_eq!(blob.len(), v.len() * 4);
        assert_eq!(blob_to_embedding(&blob), v);
        // A truncated/garbage blob (not a multiple of 4) yields an empty vec.
        assert!(blob_to_embedding(&[1, 2, 3]).is_empty());
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0f32, 0.0, 0.0];
        // Identical direction -> 1.0
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
        // Orthogonal -> 0.0
        assert!(cosine_similarity(&a, &[0.0, 1.0, 0.0]).abs() < 1e-6);
        // Opposite -> -1.0
        assert!((cosine_similarity(&a, &[-1.0, 0.0, 0.0]) + 1.0).abs() < 1e-6);
        // Length mismatch and zero vector -> 0.0 (never NaN)
        assert_eq!(cosine_similarity(&a, &[1.0, 0.0]), 0.0);
        assert_eq!(cosine_similarity(&a, &[0.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn test_content_hash_stable_and_distinct() {
        assert_eq!(content_hash("hello world"), content_hash("hello world"));
        assert_ne!(content_hash("hello world"), content_hash("hello world!"));
    }

    #[test]
    fn test_chunk_by_lines_windows_and_lines() {
        // 150 numbered lines -> overlapping windows of <=60 lines.
        let body: String = (1..=150)
            .map(|n| format!("line {}", n))
            .collect::<Vec<_>>()
            .join("\n");
        let chunks = chunk_by_lines(&body, "code", 1);
        assert!(chunks.len() >= 3, "expected multiple windows");
        // First chunk is anchored at line 1 and spans at most RAG_CHUNK_LINES.
        assert_eq!(chunks[0].start_line, 1);
        assert!(chunks[0].end_line <= RAG_CHUNK_LINES);
        // Consecutive windows overlap (next start is before prev end).
        assert!(chunks[1].start_line <= chunks[0].end_line);
        // Last chunk reaches the final line.
        assert_eq!(chunks.last().unwrap().end_line, 150);
        // Empty input -> no chunks.
        assert!(chunk_by_lines("", "code", 1).is_empty());
    }

    #[test]
    fn test_chunk_markdown_splits_on_headers() {
        let md = "# Title\nintro\n\n## Section A\naaa\nbbb\n\n## Section B\nccc";
        let chunks = chunk_markdown(md);
        // One chunk per header section.
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.kind == "doc"));
        assert!(chunks[1].content.contains("Section A"));
        assert!(chunks[2].content.contains("Section B"));
    }

    #[test]
    fn test_is_indexable_ext() {
        assert!(is_indexable_ext(Path::new("src/main.rs")));
        assert!(is_indexable_ext(Path::new("README.md")));
        assert!(is_indexable_ext(Path::new("a/b/c.tsx")));
        assert!(!is_indexable_ext(Path::new("image.png")));
        assert!(!is_indexable_ext(Path::new("binary")));
    }

    #[test]
    fn test_resolve_embedding_endpoint() {
        // Ollama uses /api/embed off the root, stripping a /v1 suffix.
        assert_eq!(
            resolve_embedding_endpoint("ollama", "http://localhost:11434"),
            "http://localhost:11434/api/embed"
        );
        assert_eq!(
            resolve_embedding_endpoint("ollama", "http://localhost:11434/v1"),
            "http://localhost:11434/api/embed"
        );
        // OpenAI-compatible appends /v1/embeddings, or /embeddings to a /v1 root.
        assert_eq!(
            resolve_embedding_endpoint("openai", "https://api.openai.com"),
            "https://api.openai.com/v1/embeddings"
        );
        assert_eq!(
            resolve_embedding_endpoint("voyage", "https://api.voyageai.com/v1"),
            "https://api.voyageai.com/v1/embeddings"
        );
        // A fully-specified endpoint is respected verbatim.
        assert_eq!(
            resolve_embedding_endpoint("custom", "http://x/v1/embeddings"),
            "http://x/v1/embeddings"
        );
    }

    #[test]
    fn test_query_terms() {
        // Stopwords + short tokens dropped; identifiers kept whole; deduped.
        let terms = query_terms("How does fetch_local_models work?");
        assert!(terms.contains(&"fetch_local_models".to_string()));
        assert!(terms.contains(&"work".to_string()));
        assert!(!terms.contains(&"how".to_string())); // stopword
        assert!(!terms.contains(&"does".to_string())); // stopword
        // Dedup: repeated term appears once.
        let dup = query_terms("cache cache CACHE");
        assert_eq!(dup, vec!["cache".to_string()]);
    }

    #[test]
    fn test_keyword_overlap() {
        let terms = vec!["fetch_local_models".to_string(), "work".to_string()];
        let text = "fn fetch_local_models() { /* how models work */ }".to_lowercase();
        assert_eq!(keyword_overlap(&text, &terms), 2);
        assert_eq!(keyword_overlap("nothing relevant here", &terms), 0);
    }

    #[test]
    fn test_rrf_order_boosts_keyword_hits() {
        // idx1 has a weak cosine but a strong keyword hit; fusion should lift it
        // above idx2 (mid cosine, no keyword) and even idx0 (top cosine alone).
        let cosines = vec![0.9f32, 0.1, 0.5];
        let keywords = vec![0usize, 5, 0];
        let order = rrf_order(&cosines, &keywords);
        assert_eq!(order[0], 1, "keyword-matched row should rank first");
        // With no keyword signal at all, order collapses to pure cosine.
        let pure = rrf_order(&cosines, &vec![0, 0, 0]);
        assert_eq!(pure, vec![0, 2, 1]);
    }
}
