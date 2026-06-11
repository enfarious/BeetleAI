# Project Design Overview

This document serves as a high-level overview and Table of Contents for all major systems in the project.

## 1. Core Architecture (Agent Harness)
*Status: Foundation / North-Star*
- [DesignDoc.md](../DesignDoc.md) - The core architectural blueprint for the agent loop and execution engine.
- [architecture.md](architecture.md) - System architecture overview: command bridge, run lifecycle, functional domains, tech stack, and roadmap.

## 2. Safety & Isolation
*Status: High Priority*
- **Git Worktree Layer**: Provides sandboxed execution for every agent run to prevent corruption of the user's working tree.

## 3. Context & Intelligence
*Status: Drafting*
- **RAG System**: (See [design/Design_RAG.md](Design_RAG.md)) - Provides high-fidelity context via semantic and keyword search.

## 4. User Interface & Interaction
*Status: In Development*
- **Three-Column Layout**: Navigation, Agent Interaction, and Viewer Stack.
- **Viewer Stack**: Polymorphic view system (File, Diff, Kanban, Doc, Browser).
- **IPC Boundary**: Command/Event contract between Tauri backend and Webview.

## 5. Data & State Management
*Status: Planned*
- **SQLite State Store**: Manages projects, kanban cards, and run history.
- **File-based Design Docs**: Versioned markdown docs stored in the project repo.

## 6. Execution & Tooling
*Status: Planned*
- **Filesystem & Process Layer**: Scoped execution of tools (read/write, shell commands) within worktrees.
- **Guardrails**: Step ceilings, cancellation tokens, and tool allowlists.