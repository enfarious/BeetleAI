use serde::{Serialize, Deserialize};
use std::sync::Mutex;
use std::fs;
use std::path::{Path, PathBuf};
use std::collections::HashMap;

use crate::git;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Card {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: String, // "backlog", "todo", "running", "blocked", "review", "done", "failed"
    pub run_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RunEvent {
    pub run_id: String,
    pub event_type: String, // "status", "message", "tool_call", "tool_result", "file_touched", "blocked", "error"
    pub payload: String, // JSON payload or text
}

pub struct AppState {
    pub cards: Mutex<Vec<Card>>,
    pub run_logs: Mutex<HashMap<String, Vec<RunEvent>>>,
}

impl AppState {
    pub fn new() -> Self {
        let initial_cards = vec![
            Card {
                id: "card_1".to_string(),
                title: "Bootstrapping & Three-Column UI Layout".to_string(),
                description: "Setup Tauri v2 template with TypeScript, and construct the basic grid UI and styles.".to_string(),
                status: "done".to_string(),
                run_id: Some("run_card_1".to_string()),
            },
            Card {
                id: "card_2".to_string(),
                title: "Git Worktree Integration & Sandbox".to_string(),
                description: "Implement git worktree creation, merge, and discard actions. Ensure filesystem is sandboxed.".to_string(),
                status: "review".to_string(),
                run_id: Some("run_card_2".to_string()),
            },
            Card {
                id: "card_3".to_string(),
                title: "Card State Store & Persistency Layer".to_string(),
                description: "Integrate SQLite and persist project files, cards, and execution transcripts.".to_string(),
                status: "todo".to_string(),
                run_id: None,
            },
            Card {
                id: "card_4".to_string(),
                title: "Autonomous Loop Run Engine".to_string(),
                description: "Build the tokio task worker loop that fetches model responses, executes tools, and sends events.".to_string(),
                status: "backlog".to_string(),
                run_id: None,
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
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

#[tauri::command]
pub fn list_projects() -> Vec<Project> {
    vec![Project {
        id: "beetleai".to_string(),
        name: "BeetleAI Harness".to_string(),
        path: std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .to_string_lossy()
            .into_owned(),
    }]
}

#[tauri::command]
pub fn open_project(path: String) -> Result<Project, String> {
    let p = Path::new(&path);
    if p.exists() {
        Ok(Project {
            id: "beetleai".to_string(),
            name: p.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Repository".to_string()),
            path,
        })
    } else {
        Err("Path does not exist".to_string())
    }
}

#[tauri::command]
pub fn read_design_doc() -> Result<String, String> {
    fs::read_to_string("DesignDoc.md").map_err(|e| e.to_string())
}

#[tauri::command]
pub fn write_design_doc(content: String) -> Result<(), String> {
    fs::write("DesignDoc.md", content).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_cards(state: tauri::State<'_, AppState>) -> Vec<Card> {
    state.cards.lock().unwrap().clone()
}

#[tauri::command]
pub fn create_card(state: tauri::State<'_, AppState>, title: String, description: String) -> Card {
    let mut cards = state.cards.lock().unwrap();
    let new_card = Card {
        id: format!("card_{}", cards.len() + 1),
        title,
        description,
        status: "backlog".to_string(),
        run_id: None,
    };
    cards.push(new_card.clone());
    new_card
}

#[tauri::command]
pub fn update_card(state: tauri::State<'_, AppState>, card_id: String, status: String) -> Result<Card, String> {
    let mut cards = state.cards.lock().unwrap();
    if let Some(card) = cards.iter_mut().find(|c| c.id == card_id) {
        card.status = status;
        Ok(card.clone())
    } else {
        Err("Card not found".to_string())
    }
}

#[tauri::command]
pub fn start_run(state: tauri::State<'_, AppState>, card_id: String) -> Result<String, String> {
    let mut cards = state.cards.lock().unwrap();
    if let Some(card) = cards.iter_mut().find(|c| c.id == card_id) {
        let run_id = format!("run_{}", card_id);

        // --- REAL GIT INTERNALS ---
        let repo_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if git::is_git_repo(&repo_path) {
            let base_branch = git::get_current_branch(&repo_path).unwrap_or_else(|_| "main".to_string());
            // Create real worktree
            git::create_worktree(&repo_path, &run_id, &base_branch)?;
        }
        // ---------------------------

        card.run_id = Some(run_id.clone());
        card.status = "running".to_string();
        
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
                    payload: "{\"role\":\"agent\",\"content\":\"Run started. Isolated git worktree sandbox created. Analyzing codebase...\"}".to_string(),
                },
            ],
        );
        Ok(run_id)
    } else {
        Err("Card not found".to_string())
    }
}

#[tauri::command]
pub fn cancel_run(state: tauri::State<'_, AppState>, run_id: String) -> Result<(), String> {
    let mut cards = state.cards.lock().unwrap();
    if let Some(card) = cards.iter_mut().find(|c| c.run_id.as_deref() == Some(&run_id)) {
        card.status = "failed".to_string();

        // --- REAL GIT INTERNALS ---
        let repo_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if git::is_git_repo(&repo_path) {
            git::remove_worktree(&repo_path, &run_id)?;
        }
        // ---------------------------

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
        Ok(())
    } else {
        Err("Run not found".to_string())
    }
}

#[tauri::command]
pub fn unblock_run(state: tauri::State<'_, AppState>, run_id: String, reply: String) -> Result<(), String> {
    let mut cards = state.cards.lock().unwrap();
    if let Some(card) = cards.iter_mut().find(|c| c.run_id.as_deref() == Some(&run_id)) {
        card.status = "running".to_string();
        let mut logs = state.run_logs.lock().unwrap();
        if let Some(run_events) = logs.get_mut(&run_id) {
            run_events.push(RunEvent {
                run_id: run_id.clone(),
                event_type: "message".to_string(),
                payload: format!("{{\"role\":\"user\",\"content\":\"{}\"}}", reply),
            });
            run_events.push(RunEvent {
                run_id: run_id.clone(),
                event_type: "status".to_string(),
                payload: "running".to_string(),
            });
            run_events.push(RunEvent {
                run_id: run_id.clone(),
                event_type: "message".to_string(),
                payload: "{\"role\":\"agent\",\"content\":\"Input received. Resuming execution loop...\"}".to_string(),
            });
        }
        Ok(())
    } else {
        Err("Run not found".to_string())
    }
}

#[tauri::command]
pub fn accept_run(state: tauri::State<'_, AppState>, run_id: String) -> Result<(), String> {
    let mut cards = state.cards.lock().unwrap();
    if let Some(card) = cards.iter_mut().find(|c| c.run_id.as_deref() == Some(&run_id)) {
        card.status = "done".to_string();

        // --- REAL GIT INTERNALS ---
        let repo_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if git::is_git_repo(&repo_path) {
            let base_branch = git::get_current_branch(&repo_path).unwrap_or_else(|_| "main".to_string());
            git::merge_worktree(&repo_path, &run_id, &base_branch)?;
        }
        // ---------------------------

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
pub fn reject_run(state: tauri::State<'_, AppState>, run_id: String) -> Result<(), String> {
    let mut cards = state.cards.lock().unwrap();
    if let Some(card) = cards.iter_mut().find(|c| c.run_id.as_deref() == Some(&run_id)) {
        card.status = "failed".to_string();

        // --- REAL GIT INTERNALS ---
        let repo_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if git::is_git_repo(&repo_path) {
            git::remove_worktree(&repo_path, &run_id)?;
        }
        // ---------------------------

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
                payload: "{\"role\":\"agent\",\"content\":\"Worktree discarded and branch deleted.\"}".to_string(),
            });
        }
        Ok(())
    } else {
        Err("Run not found".to_string())
    }
}

#[tauri::command]
pub fn get_run_log(state: tauri::State<'_, AppState>, run_id: String) -> Result<Vec<RunEvent>, String> {
    let logs = state.run_logs.lock().unwrap();
    if let Some(events) = logs.get(&run_id) {
        Ok(events.clone())
    } else {
        Ok(Vec::new())
    }
}

#[tauri::command]
pub fn send_chat(state: tauri::State<'_, AppState>, run_id: String, message: String) -> Result<(), String> {
    let mut logs = state.run_logs.lock().unwrap();
    if let Some(run_events) = logs.get_mut(&run_id) {
        run_events.push(RunEvent {
            run_id: run_id.clone(),
            event_type: "message".to_string(),
            payload: format!("{{\"role\":\"user\",\"content\":\"{}\"}}", message),
        });
        
        // Mock a quick agent reply
        let reply_msg = if message.to_lowercase().contains("test") {
            "Testing suite is operational. Unit tests found inside `src-tauri/src/git.rs` output 100% success."
        } else {
            "Understood. Proceeding to implement the requested details. I am continuing the current run iteration."
        };
        
        run_events.push(RunEvent {
            run_id: run_id.clone(),
            event_type: "message".to_string(),
            payload: format!("{{\"role\":\"agent\",\"content\":\"{}\"}}", reply_msg),
        });
        Ok(())
    } else {
        Err("Run not found".to_string())
    }
}

#[tauri::command]
pub fn read_diff(run_id: String) -> Result<String, String> {
    let repo_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if git::is_git_repo(&repo_path) {
        let base_branch = git::get_current_branch(&repo_path).unwrap_or_else(|_| "main".to_string());
        match git::get_diff(&repo_path, &run_id, &base_branch) {
            Ok(diff) => {
                if diff.trim().is_empty() {
                    Ok("No changes detected in this run.".to_string())
                } else {
                    Ok(diff)
                }
            }
            Err(e) => {
                // Friendly fallback for our seed card
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
+"#.to_string())
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
pub fn list_dir(path: String) -> Result<Vec<DirEntry>, String> {
    let base_path = Path::new(&path);
    let mut entries = Vec::new();
    
    if let Ok(rd) = fs::read_dir(base_path) {
        for entry in rd.flatten() {
            let p = entry.path();
            let name = p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            
            // Skip noisy directories
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
    
    // Sort directory first, then files alphabetically
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
pub fn read_file(path: String) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| e.to_string())
}
