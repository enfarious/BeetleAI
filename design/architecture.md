# BeetleAI Design Document

## 1. Vision & Mission
BeetleAI is an **Agentic AI Workspace** designed to bridge the gap between high-level human intent and low-level technical execution. Unlike standard LLM interfaces, BeetleAI acts as a proactive collaborator capable of managing project documentation, executing code changes through supervised "runs," tracking tasks via integrated Kanban boards, and navigating complex file systems.

## 2. System Architecture
BeetleAI utilizes a modern desktop architecture built on **Tauri v2**, separating the high-performance Rust backend from a reactive frontend.

### 2.1 Core Architecture Pattern: The Command Bridge
- **Frontend**: (Planned) A responsive UI that communicates via Tauri's IPC (Inter-Process Communication) layer.
- **Backend (Rust)**: Acts as the "Brain" and "Executor." It manages system access, local data persistence, and long-running asynchronous tasks.
- **The Bridge**: All user intentions are converted into `Tauri Commands`, which trigger specific Rust modules (e.g., `commands::git`, `commands::file_system`).

### 2.2 Data Persistence & State Management
- **Local Database**: SQLite (via `rusqlite`) serves as the primary store for user settings, project metadata, chat history, and Kanban card states.
- **Managed App State**: A centralized `AppState` struct is injected into the Tauri runtime, ensuring thread-safe access to database connections and application configuration during the app lifecycle.

## 3. Functional Domains

### 3.1 Agentic Execution Engine (The "Run" Lifecycle)
To ensure safety and control, AI-proposed changes follow a supervised execution loop:
1.  **Start**: The agent initiates a sequence of actions.
2.  **Propose**: The agent generates a diff or a plan (visible to the user).
3.  **Review/Intervention**: The user can `unblock_run`, `accept_run`, `reject_run`, or `cancel_run` in real-time.
4.  **Apply**: Upon acceptance, the changes are committed to the filesystem or database.

### 3.2 Knowledge & Context Management
- **File System Interface**: Direct capabilities to read, write, and create files/directories within a project context.
- **Git Integration**: Intelligence regarding version control state to provide context-aware code suggestions.
- **Design Doc Interaction**: The ability to read and edit `.md` documentation directly (Agentic Documentation Management).

### 3.3 Task & Project Orchestration
- **Kanban System**: Integrated card management (`create`, `update`, `delete`) to track development progress and agentic goals.
- **Project Scoping**: Multi-project support with automated path normalization for secure filesystem access.

## 4. Technical Stack
| Layer | Technology |
| :--- | :--- |
| **Runtime** | Tauri v2 (Rust + Webview) |
| **Language (Backend)** | Rust (Edition 2021) |
| **Database** | SQLite (`rusqlite`) |
| **Networking** | `ureq` (HTTP/API communication) |
| **Serialization** | `serde`, `serde_json` |
| **Concurrency** | `tokio` |
| **System Utilities** | `chrono` (Time), `git` integration |

## 5. Future Roadmap (Agentic Evolution)
These items represent the next phase of BeetleAI's intelligence:
- [ ] **RAG Implementation**: Integrating Retrieval-Augmented Generation to allow the agent to "index" and reason over entire large-scale codebases.
- [ ] **Advanced Navigation Tools**: AI-driven semantic search and codebase tree traversal.
- [ ] **Autonomous Agent Workflows**: Transitioning from supervised "Runs" to semi-autonomous background tasks for complex refactoring.
- [ ] **Enhanced Context Windows**: Optimized management of long-running conversation history in SQLite.