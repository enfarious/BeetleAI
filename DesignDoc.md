# Agent Harness — Design Doc

> Status: v0 foundation. This is the north-star document. Every run in this project gets
> this doc (or a summary of it) prepended to its system prompt. Keep it current; if the
> code and this doc disagree, the doc is a bug.

## 1. What this is

A desktop agent harness for autonomous, card-driven software work. You write a project
design doc and a kanban board; you select a card and hit run; an agent works the card to
completion against your real repository, in isolation, and parks the result for your review.

It is **not** a chat-driven pair programmer where you steer every step. The chat exists to
brief, interrupt, and unblock — but the unit of work is the card, and the agent owns the loop
from pickup to review.

### Core decisions (locked)

- **Stack:** Tauri. Rust backend is the execution engine; the webview is pure presentation.
- **Execution target:** the user's real local repository on disk. No container, no cloud sandbox.
- **Agent relationship to cards:** autonomous. A card is a job, not just a context bundle.
- **Safety model:** every run executes in a dedicated git worktree. Nothing touches the user's
  working tree, and nothing merges without an explicit accept click.

The second and third decisions are what make the fourth non-negotiable. An autonomous agent
editing a real repo will eventually do something the user wants gone. The worktree + accept gate
is the thing that makes that recoverable, and it gets built *first*.

## 2. Architecture at a glance

Three layers:

1. **Webview (presentation)** — three columns. Holds no business logic and never owns the
   agent loop. Subscribes to event streams and renders.
2. **Tauri IPC boundary** — `invoke` commands go down, events stream up. This is the
   frontend/backend contract (section 6).
3. **Rust backend (engine)** — run engine, git worktree layer, filesystem/process layer,
   state store. This is where the work actually happens.

### The three columns

- **Left — navigation + context.** Mode switcher (plan / kanban / code), project tree, file
  browser.
- **Center — agent interaction.** Chat transcript for the active run, run controls
  (run/cancel/unblock), quick settings.
- **Right — the viewer stack.** Polymorphic: file, diff, kanban board, design doc. One active
  view; clicking things in the left column or events from the agent push views onto it.

Mode is lightweight. It sets the *default* right-panel view and the agent's framing, but never
locks the columns — "I'm in chat mode but want to glance at the board" must stay frictionless.

## 3. The run engine (priority section)

The heart of the system. Lives entirely in Rust as an async task. The frontend never holds the
loop — it spawns a run and subscribes.

### Run lifecycle state machine

```
                ┌─────────┐
                │ queued  │  card has a run requested, not yet started
                └────┬────┘
                     │ engine picks up
                ┌────▼────┐
         ┌──────│ running │──────┐
         │      └────┬────┘      │
   needs input?      │        error/panic?
         │       completes         │
     ┌────▼────┐      │         ┌───▼────┐
     │ blocked │      │         │ failed │
     └────┬────┘      │         └────────┘
          │ user      │
          │ unblocks  ▼
          └────►  ┌────────┐
                  │ review │  agent done, diff awaits human accept
                  └───┬────┘
                      │ accept (merge) → done
                      │ reject (discard worktree) → failed/archived
                      ▼
                  ┌──────┐
                  │ done │
                  └──────┘
```

States:

- **queued** — a run has been requested for a card but the engine hasn't started it. Lets you
  queue several cards.
- **running** — the agent loop is active: call model → receive tool calls → execute against the
  FS/process layer → stream results back → repeat. Holds a cancellation token.
- **blocked** — the agent has asked a question or hit a gate that needs human input. The loop is
  suspended, not killed. Unblocking resumes the same task with the user's reply injected.
- **review** — the agent finished. The worktree holds its changes. Nothing has merged. This is the
  human gate.
- **done** — the user accepted; the worktree merged to base and was removed.
- **failed** — the run errored, was cancelled, or the user rejected it at review. Worktree discarded.

### The loop, concretely

A run is an async task (`tokio::spawn`) holding:

- the conversation history (system prompt = design doc + card context + tool definitions)
- a `CancellationToken`
- a handle to its git worktree path
- the FS/process layer scoped to that worktree
- an mpsc sender for streaming events to the frontend

Each iteration: send history to the model → parse tool-use blocks → execute each tool (file
read/write/edit, spawn process, run tests) → append results → emit a `run:event` per step → check
the cancellation token → loop until the model returns no tool calls (done) or asks a blocking
question (blocked).

### Guardrails inside the loop

- **Tool allowlist.** The agent can only call tools you've registered. No arbitrary shell unless
  you explicitly add a `run_command` tool, and even then it runs *in the worktree* under the
  process layer's controls.
- **Step ceiling.** Hard cap on iterations per run (configurable, default ~50). Hitting it moves
  the run to `blocked` with a "step limit reached, continue?" prompt rather than burning forever.
- **Cancellation is cooperative + hard.** The token is checked between iterations (graceful) and
  any spawned child process gets killed on cancel (hard).

## 4. Git worktree safety layer (build this first)

The isolation primitive. Build and test this before the agent loop does anything real.

- **On run start:** `git worktree add .harness/worktrees/<run-id> -b harness/run-<run-id>` from
  current HEAD (or a chosen base). The agent works only inside that directory.
- **During the run:** the FS/process layer is scoped to the worktree path. The agent cannot
  resolve paths outside it (reject `..` traversal, symlink escapes).
- **At review:** the diff viewer shows `harness/run-<run-id>` vs base. This is the same renderer
  as the file viewer with a mode flag.
- **Accept:** merge or squash-commit the branch to base, then `git worktree remove`. The user's
  real tree only ever changes here, on an explicit click.
- **Reject:** `git worktree remove --force` + delete the branch. The work vanishes cleanly.

Consequences worth noting:

- Concurrent runs on different cards don't collide — separate worktrees.
- "Undo" is free — it's just discarding a worktree.
- The user's working tree is never the agent's canvas, so an in-progress run can't corrupt
  uncommitted local work.

Edge cases to handle early: dirty base tree at run start (stash? warn? require clean?), merge
conflicts at accept time (surface in the diff viewer, let the user resolve or kick it back to the
agent as a new card), and worktree cleanup on crash (orphaned worktrees should be reaped on app
start via `git worktree prune`).

## 5. Kanban / run state coupling

Run status is **canonical**. Do not store column position as a separate source of truth that you
also try to keep in sync with run status — they will desync.

Column mapping:

- **Backlog / Todo** — cards with no run yet. This distinction is manual and user-owned.
- **In Progress** — a run in `running`.
- **Blocked** — a run in `blocked`.
- **Review** — a run in `review`. The card sits here until the human accepts; it does not
  self-advance.
- **Done** — a run in `done` (accepted).

The one explicit human gate in the column flow is Review → Done. Everything else is derived from
run state.

## 6. IPC command surface (priority section)

This is the frontend/backend contract. Commands are `invoke`d from the webview; events stream back.

### Commands (frontend → backend)

| Command | Args | Returns | Notes |
|---|---|---|---|
| `list_projects` | — | `Project[]` | |
| `open_project` | `path` | `Project` | loads design doc + board + cards |
| `read_design_doc` | `project_id` | `string` (markdown) | from `.harness/design.md` in repo |
| `write_design_doc` | `project_id, content` | `()` | versioned with the code |
| `list_cards` | `project_id` | `Card[]` | |
| `create_card` | `project_id, card` | `Card` | |
| `update_card` | `card_id, patch` | `Card` | manual moves between backlog/todo only |
| `start_run` | `card_id` | `run_id` | creates worktree, queues the run |
| `cancel_run` | `run_id` | `()` | trips cancellation token, kills children |
| `unblock_run` | `run_id, reply` | `()` | resumes a blocked run with user input |
| `accept_run` | `run_id` | `()` | merges worktree, removes it, card → done |
| `reject_run` | `run_id` | `()` | discards worktree, run → failed |
| `get_run_log` | `run_id` | `RunEvent[]` | replay for reconnect |
| `read_file` | `project_id, path` | `string` | scoped to repo |
| `read_diff` | `run_id` | `Diff` | worktree branch vs base |
| `list_dir` | `project_id, path` | `DirEntry[]` | file browser |
| `send_chat` | `run_id, message` | `()` | mid-run interjection; routed into the loop |
| `get_settings` / `set_settings` | … | … | model, step ceiling, tool allowlist |

### Events (backend → frontend, streamed)

Channel naming: `run:<run-id>:event` or a single `run:event` carrying `run_id` — pick one and
stick to it (recommendation: single channel with `run_id` in the payload, simpler subscription).

| Event | Payload | Meaning |
|---|---|---|
| `run:status` | `{run_id, status}` | state machine transition |
| `run:message` | `{run_id, role, content}` | a chat turn (agent or tool result) |
| `run:tool_call` | `{run_id, tool, args}` | agent invoked a tool |
| `run:tool_result` | `{run_id, tool, result}` | tool returned |
| `run:file_touched` | `{run_id, path, op}` | agent created/edited/deleted a file — frontend may auto-push a diff view |
| `run:blocked` | `{run_id, question}` | needs human input |
| `run:error` | `{run_id, error}` | failure detail |

Design note: `run:file_touched` is the hook that lets the right panel auto-surface a diff as the
agent works, without the frontend polling.

## 7. Right panel: the viewer stack

A single discriminated union drives it:

```ts
type ViewerState =
  | { kind: 'file';    path: string }
  | { kind: 'diff';    runId: string }
  | { kind: 'kanban' }
  | { kind: 'doc' }                       // design doc
  | { kind: 'browser'; dir: string };
```

`file` and `diff` are the same renderer with a mode flag (working-tree-vs-HEAD becomes
branch-vs-base). The stack supports push/pop so the user can drill into a diff and come back to
the board. Mode (section 2) sets the default push but never locks the stack.

## 8. State & persistence

- **Design doc:** markdown on disk in the repo (`.harness/design.md`). Versioned with the code.
  Prepended (or summarized) into every run's system prompt — this is what makes autonomous
  card-work coherent.
- **Projects, cards, run history:** SQLite per project (`.harness/harness.db`). Keep run logs here
  so a run can be replayed on reconnect (`get_run_log`).
- **Worktrees:** `.harness/worktrees/<run-id>/`, reaped on accept/reject and pruned on app start.

Add `.harness/worktrees/` and `.harness/harness.db` to `.gitignore`; keep `.harness/design.md`
tracked.

## 9. Build order

Sequenced so the safety layer exists before the agent can do anything real.

1. **Tauri shell + three-column layout.** Static panels, mode switcher, no agent. Prove the IPC
   round-trip with `list_projects` / `open_project`.
2. **Git worktree layer + diff viewer.** `start_run` creates a worktree, you manually drop a file
   in it, the diff viewer shows it, `accept_run` merges, `reject_run` discards. No agent yet — this
   is the sandbox, tested in isolation.
3. **State store.** Projects, cards, board, design doc read/write. Kanban renders from real data.
4. **Run engine — minimal loop.** Model call → file read/write/edit tools → streaming events →
   the run lands in `review` with a real diff. Step ceiling and cancellation from day one.
5. **Blocking + unblock + mid-run chat.** The `blocked` state, `unblock_run`, `send_chat`.
6. **Process tools.** `run_command` / test runner, scoped to the worktree, killed on cancel.
7. **Polish:** queue management, concurrent runs, settings, orphan reaping.

Steps 1–2 are the whole game for safety. Resist the urge to get the loop running (step 4) before
the sandbox (step 2) is solid.

## 10. Open questions

- **Dirty base tree at run start** — require clean, auto-stash, or warn-and-proceed?
- **Merge conflicts at accept** — resolve in the diff viewer, or kick back to the agent as a new card?
- **Design doc size** — at what point do we summarize it into the system prompt rather than
  prepend whole? Token budget vs. fidelity.
- **Multi-card dependencies** — does a card know it depends on another card's `done` state, or is
  ordering purely manual for v0? (Recommend manual for v0.)
- **Self-hosting milestone** — once steps 1–5 are done, point the harness at its own repo and let
  it work its own backlog. Good forcing function for the tool surface.
