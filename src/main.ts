import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

// ─── Toast notifications & styled confirm (replaces blocking alert/confirm) ───
type ToastKind = "success" | "error" | "info";

function showToast(message: string, kind: ToastKind = "info", durationMs = 4000) {
  const container = document.getElementById("toast-container");
  if (!container) return;
  const toast = document.createElement("div");
  toast.className = `toast toast-${kind}`;
  toast.textContent = message;
  container.appendChild(toast);
  const remove = () => {
    toast.classList.add("toast-leaving");
    toast.addEventListener("animationend", () => toast.remove(), { once: true });
  };
  // Errors stick around longer; click any toast to dismiss immediately.
  const timer = setTimeout(remove, kind === "error" ? Math.max(durationMs, 7000) : durationMs);
  toast.addEventListener("click", () => {
    clearTimeout(timer);
    remove();
  });
}

function showConfirm(message: string, okLabel = "Delete"): Promise<boolean> {
  return new Promise((resolve) => {
    const modal = document.getElementById("confirm-modal") as HTMLDivElement;
    const msgEl = document.getElementById("confirm-modal-message") as HTMLParagraphElement;
    const okBtn = document.getElementById("btn-confirm-ok") as HTMLButtonElement;
    const cancelBtn = document.getElementById("btn-confirm-cancel") as HTMLButtonElement;
    if (!modal || !msgEl || !okBtn || !cancelBtn) {
      resolve(window.confirm(message));
      return;
    }
    msgEl.textContent = message;
    okBtn.textContent = okLabel;
    modal.style.display = "flex";
    const cleanup = (result: boolean) => {
      modal.style.display = "none";
      okBtn.removeEventListener("click", onOk);
      cancelBtn.removeEventListener("click", onCancel);
      document.removeEventListener("keydown", onKey);
      resolve(result);
    };
    const onOk = () => cleanup(true);
    const onCancel = () => cleanup(false);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") cleanup(false);
    };
    okBtn.addEventListener("click", onOk);
    cancelBtn.addEventListener("click", onCancel);
    document.addEventListener("keydown", onKey);
    // Focus the safe option for destructive confirms.
    cancelBtn.focus();
  });
}

// Detect if running in Tauri container environment
const isTauri = typeof window !== "undefined" && (window as any).__TAURI_INTERNALS__ !== undefined;

// Browser stateful mocks
let mockCards: Card[] = [
  {
    id: "card_1",
    project_path: ".",
    title: "Bootstrapping & Three-Column UI Layout",
    description: "Setup Tauri v2 template with TypeScript, and construct the basic grid UI and styles.",
    status: "done",
    run_id: "run_card_1",
    assignee: "BeetleAI",
    todo_list: [
      { text: "Configure Tauri v2 project template", completed: true },
      { text: "Build TypeScript sidebar navigation and panels", completed: true },
      { text: "Construct CSS layouts and themes", completed: true }
    ]
  },
  {
    id: "card_2",
    project_path: ".",
    title: "Git Worktree Integration & Sandbox",
    description: "Implement git worktree creation, merge, and discard actions. Ensure filesystem is sandboxed.",
    status: "review",
    run_id: "run_card_2",
    assignee: "BeetleAI",
    todo_list: [
      { text: "Implement git worktree creation helpers", completed: true },
      { text: "Integrate file deletion and modification boundaries", completed: true },
      { text: "Verify sandbox path traversal checks", completed: false }
    ]
  },
  {
    id: "card_3",
    project_path: ".",
    title: "Card State Store & Persistency Layer",
    description: "Integrate SQLite and persist project files, cards, and execution transcripts.",
    status: "todo",
    run_id: null,
    assignee: null,
    todo_list: [
      { text: "Design database schema for cards and logs", completed: false },
      { text: "Integrate SQLite driver and migrations", completed: false },
      { text: "Implement state persistence interface", completed: false }
    ]
  },
  {
    id: "card_4",
    project_path: ".",
    title: "Autonomous Loop Run Engine",
    description: "Build the tokio task worker loop that fetches model responses, executes tools, and sends events.",
    status: "backlog",
    run_id: null,
    assignee: null,
    todo_list: [
      { text: "Construct Tokio worker thread loop", completed: false },
      { text: "Implement model response stream parsing", completed: false },
      { text: "Add recursive tool routing handlers", completed: false }
    ]
  }
];

let mockLogs: Record<string, RunEvent[]> = {
  "run_card_2": [
    {
      run_id: "run_card_2",
      event_type: "status",
      payload: "running"
    },
    {
      run_id: "run_card_2",
      event_type: "message",
      payload: JSON.stringify({ role: "agent", content: "Starting worktree preparation for card_2. Creating branch harness/run-card_2 from main." })
    },
    {
      run_id: "run_card_2",
      event_type: "status",
      payload: "review"
    },
    {
      run_id: "run_card_2",
      event_type: "message",
      payload: JSON.stringify({ role: "agent", content: "I have completed writing the git operations module in `src-tauri/src/git.rs`. Let me know if you would like me to merge it!" })
    }
  ]
};

// Mock implementation of invoke for browser mode
const mockInvoke = async (cmd: string, args?: any): Promise<any> => {
  console.log(`[Mock Invoke] command: ${cmd}`, args);
  switch (cmd) {
    case "list_projects":
      return [
        { id: "beetleai", name: "BeetleAI Harness", path: "f:\\Projects\\BeetleAI" }
      ];
    case "open_project":
      return { id: "beetleai", name: "BeetleAI Harness", path: args.path };
    case "create_project":
      return { 
        id: args.name.toLowerCase().replace(/ /g, "_"), 
        name: args.name, 
        path: args.path 
      };
    case "get_settings":
      return {
        provider: "custom",
        api_url: "http://localhost:11434/v1",
        api_key: "mock_api_key",
        model: "llama3",
        max_steps: 50
      };
    case "save_settings":
      return {};
    case "read_design_doc":
      return `# Mock Design Document\n\nThis is a mock design document for testing in browser mode.`;
    case "write_design_doc":
      return {};
    case "list_cards":
      return mockCards;
    case "start_run": {
      const card = mockCards.find(c => c.id === args.cardId);
      const runId = `run_${args.cardId}`;
      if (card) {
        card.status = "running";
        card.run_id = runId;
      }
      mockLogs[runId] = [
        {
          run_id: runId,
          event_type: "status",
          payload: "running"
        },
        {
          run_id: runId,
          event_type: "message",
          payload: JSON.stringify({ role: "agent", content: "Isolated sandbox initialized. Model connection established. Ready to execute code work." })
        }
      ];
      return runId;
    }
    case "cancel_run": {
      const card = mockCards.find(c => c.run_id === args.runId);
      if (card) {
        card.status = "failed";
      }
      if (!mockLogs[args.runId]) mockLogs[args.runId] = [];
      mockLogs[args.runId].push(
        {
          run_id: args.runId,
          event_type: "status",
          payload: "failed"
        },
        {
          run_id: args.runId,
          event_type: "message",
          payload: JSON.stringify({ role: "agent", content: "Run cancelled by developer. Discarded sandbox changes." })
        }
      );
      return {};
    }
    case "unblock_run": {
      const card = mockCards.find(c => c.run_id === args.runId);
      if (card) {
        card.status = "running";
      }
      if (!mockLogs[args.runId]) mockLogs[args.runId] = [];
      mockLogs[args.runId].push(
        {
          run_id: args.runId,
          event_type: "message",
          payload: JSON.stringify({ role: "user", content: args.reply })
        },
        {
          run_id: args.runId,
          event_type: "status",
          payload: "running"
        },
        {
          run_id: args.runId,
          event_type: "message",
          payload: JSON.stringify({ role: "agent", content: "Feedback received. Proceeding with execution loop..." })
        }
      );
      return {};
    }
    case "accept_run": {
      const card = mockCards.find(c => c.run_id === args.runId);
      if (card) {
        card.status = "done";
      }
      if (!mockLogs[args.runId]) mockLogs[args.runId] = [];
      mockLogs[args.runId].push(
        {
          run_id: args.runId,
          event_type: "status",
          payload: "done"
        },
        {
          run_id: args.runId,
          event_type: "message",
          payload: JSON.stringify({ role: "agent", content: "Worktree successfully squashed and merged into main." })
        }
      );
      return {};
    }
    case "reject_run": {
      const card = mockCards.find(c => c.run_id === args.runId);
      if (card) {
        card.status = "failed";
      }
      if (!mockLogs[args.runId]) mockLogs[args.runId] = [];
      mockLogs[args.runId].push(
        {
          run_id: args.runId,
          event_type: "status",
          payload: "failed"
        },
        {
          run_id: args.runId,
          event_type: "message",
          payload: JSON.stringify({ role: "agent", content: "Worktree changes rejected and branch deleted." })
        }
      );
      return {};
    }
    case "send_chat": {
      if (!mockLogs[args.runId]) mockLogs[args.runId] = [];
      mockLogs[args.runId].push({
        run_id: args.runId,
        event_type: "message",
        payload: JSON.stringify({ role: "user", content: args.message })
      });
      
      const reply = args.message.toLowerCase().includes("test")
        ? "All unit tests compiled successfully inside `src-tauri/src/git.rs`."
        : "Understood. Adjusting implementation path in sandbox.";
        
      mockLogs[args.runId].push({
        run_id: args.runId,
        event_type: "message",
        payload: JSON.stringify({ role: "agent", content: reply })
      });
      return {};
    }
    case "get_run_log":
      return mockLogs[args.runId] || [];
    case "save_card": {
      const idx = mockCards.findIndex(c => c.id === args.card.id);
      if (idx !== -1) {
        mockCards[idx] = { ...args.card };
        return mockCards[idx];
      }
      return null;
    }
    case "delete_card": {
      mockCards = mockCards.filter(c => c.id !== args.cardId);
      return {};
    }
    case "list_design_docs":
      return ["design.md", "architecture.md"];
    case "create_file":
      return {};
    case "create_dir":
      return {};
    case "save_file":
      return {};
    case "delete_item":
      return {};
    case "list_dir":
      return [
        { name: "src", path: "src", is_dir: true },
        { name: "src-tauri", path: "src-tauri", is_dir: true },
        { name: "index.html", path: "index.html", is_dir: false },
        { name: "package.json", path: "package.json", is_dir: false }
      ];
    case "read_file":
      return `// Mock file content\nconsole.log("Hello from mock file!");`;
    case "read_diff":
      return `diff --git a/mock.txt b/mock.txt\n--- a/mock.txt\n+++ b/mock.txt\n@@ -1,1 +1,2 @@\n-Mock content\n+Mock content modified in browser\n+Additional line`;
    case "fetch_local_models":
      return [
        { name: "llama3:latest", is_loaded: true, context_size: 8192 },
        { name: "mistral:latest", is_loaded: false, context_size: 32768 },
        { name: "phi3:latest", is_loaded: false, context_size: 4096 }
      ];
    default:
      return {};
  }
};

// Safe invoke wrapper delegating to backend or mock
async function invoke<T>(cmd: string, args?: any): Promise<T> {
  if (isTauri) {
    return await tauriInvoke<T>(cmd, args);
  } else {
    return await mockInvoke(cmd, args) as T;
  }
}

// Interfaces from Rust commands contract
interface Project {
  id: string;
  name: string;
  path: string;
}

interface TodoItem {
  text: string;
  completed: boolean;
}

interface Card {
  id: string;
  project_path: string;
  title: string;
  description: string;
  status: string; // "backlog", "todo", "running", "blocked", "review", "done", "failed"
  run_id: string | null;
  assignee: string | null;
  todo_list: TodoItem[];
}

interface RunEvent {
  run_id: string;
  event_type: string; // "status", "message", "tool_call", "tool_result", "file_touched", "blocked", "error"
  payload: string;
}

interface DirEntry {
  name: string;
  path: string;
  is_dir: boolean;
}

// Viewer panel polymorphic state
type ViewerState =
  | { kind: "doc"; docName?: string }
  | { kind: "kanban" }
  | { kind: "file"; path: string; name: string }
  | { kind: "diff"; runId: string }
  | { kind: "new_project" }
  | { kind: "card_detail"; cardId: string };

interface LlmSettings {
  provider: string;
  api_url: string;
  api_key: string;
  model: string;
  max_steps: number;
}

// App State
let currentProject: Project | null = null;
let cardsList: Card[] = [];
let activeCard: Card | null = null;
let viewerStack: ViewerState[] = [{ kind: "doc", docName: "design.md" }];
let currentMode: "plan" | "kanban" | "code" = "plan";
let selectedDesignDoc: string = "design.md";
let isEditingViewer = false;
let viewerEditBuffer = "";
let activeLlmRunId: string | null = null;

let globalChunkListener: ((event: any) => void) | null = null;
let activeStreams = new Map<string, {
  text: string;
  bubbleElement: HTMLDivElement | null;
}>();

// DOM References
const projectSelect = document.getElementById("project-select") as HTMLSelectElement;
const btnNewProjectToggle = document.getElementById("btn-new-project-toggle") as HTMLButtonElement;
const repoWorkspaceSection = document.getElementById("repo-workspace-section") as HTMLDivElement;
const designDocsSection = document.getElementById("design-docs-section") as HTMLDivElement;
const designDocsList = document.getElementById("design-docs-list") as HTMLDivElement;
const fileTreeContainer = document.getElementById("file-tree") as HTMLDivElement;
const activeCardTitle = document.getElementById("active-card-title") as HTMLHeadingElement;
const runStatusBadge = document.getElementById("run-status") as HTMLDivElement;
const runStatusText = document.getElementById("run-status-text") as HTMLSpanElement;
const chatMessages = document.getElementById("chat-messages") as HTMLDivElement;
const chatInput = document.getElementById("chat-input") as HTMLTextAreaElement;
const btnSendChat = document.getElementById("btn-send-chat") as HTMLButtonElement;

const controlsRun = document.getElementById("controls-run") as HTMLDivElement;
const controlsActive = document.getElementById("controls-active") as HTMLDivElement;
const controlsReview = document.getElementById("controls-review") as HTMLDivElement;

const btnStartRun = document.getElementById("btn-start-run") as HTMLButtonElement;
const btnCancelRun = document.getElementById("btn-cancel-run") as HTMLButtonElement;
const btnAcceptRun = document.getElementById("btn-accept-run") as HTMLButtonElement;
const btnRejectRun = document.getElementById("btn-reject-run") as HTMLButtonElement;

const viewerTitle = document.getElementById("viewer-title") as HTMLHeadingElement;
const viewerContainer = document.getElementById("viewer-container") as HTMLDivElement;
const btnViewerBack = document.getElementById("btn-viewer-back") as HTMLButtonElement;

// Viewer Header Edit Actions
const btnViewerEdit = document.getElementById("btn-viewer-edit") as HTMLButtonElement;
const btnViewerSave = document.getElementById("btn-viewer-save") as HTMLButtonElement;
const btnViewerCancel = document.getElementById("btn-viewer-cancel") as HTMLButtonElement;

// File System Dialog Modal Elements
const fsModal = document.getElementById("fs-modal") as HTMLDivElement;
const fsModalTitle = document.getElementById("fs-modal-title") as HTMLHeadingElement;
const btnFsClose = document.getElementById("btn-fs-close") as HTMLButtonElement;
const fsForm = document.getElementById("fs-form") as HTMLFormElement;
const fsParentPath = document.getElementById("fs-parent-path") as HTMLInputElement;
const fsItemType = document.getElementById("fs-item-type") as HTMLInputElement;
const fsItemName = document.getElementById("fs-item-name") as HTMLInputElement;
const btnFsCancel = document.getElementById("btn-fs-cancel") as HTMLButtonElement;
const fsLabelName = document.getElementById("fs-label-name") as HTMLLabelElement;
const btnFsSubmit = document.getElementById("btn-fs-submit") as HTMLButtonElement;

// Root Buttons
const btnRootAddFile = document.getElementById("btn-root-add-file") as HTMLButtonElement;
const btnRootAddFolder = document.getElementById("btn-root-add-folder") as HTMLButtonElement;
const btnAddDesignDoc = document.getElementById("btn-add-design-doc") as HTMLButtonElement;

// Settings Modal Selectors
const btnSettingsToggle = document.getElementById("btn-settings-toggle") as HTMLButtonElement;
const settingsModal = document.getElementById("settings-modal") as HTMLDivElement;
const btnSettingsClose = document.getElementById("btn-settings-close") as HTMLButtonElement;
const btnSettingsCancel = document.getElementById("btn-settings-cancel") as HTMLButtonElement;
const settingsForm = document.getElementById("settings-form") as HTMLFormElement;
const settingsProvider = document.getElementById("settings-provider") as HTMLSelectElement;
const settingsUrl = document.getElementById("settings-url") as HTMLInputElement;
const settingsKey = document.getElementById("settings-key") as HTMLInputElement;
const settingsModel = document.getElementById("settings-model") as HTMLInputElement;
const settingsModelSelect = document.getElementById("settings-model-select") as HTMLSelectElement;
const btnFetchModels = document.getElementById("btn-fetch-models") as HTMLButtonElement;
const btnToggleModelInput = document.getElementById("btn-toggle-model-input") as HTMLButtonElement;
const modelContextInfo = document.getElementById("model-context-info") as HTMLSpanElement;
const settingsSteps = document.getElementById("settings-steps") as HTMLInputElement;

// Mode tabs
const tabPlan = document.getElementById("tab-plan") as HTMLButtonElement;
const tabKanban = document.getElementById("tab-kanban") as HTMLButtonElement;
const tabCode = document.getElementById("tab-code") as HTMLButtonElement;

// Resize panels via drag handles
function setupResizablePanels() {
  const container = document.querySelector(".app-container") as HTMLDivElement;
  if (!container) return;

  const resizerLeft = document.getElementById("resizer-left") as HTMLDivElement;
  const resizerRight = document.getElementById("resizer-right") as HTMLDivElement;
  if (!resizerLeft || !resizerRight) return;

  const panelIds = ["left-panel", "center-panel", "right-panel"];
  const panels: Record<string, HTMLElement> = {};
  for (const id of panelIds) {
    const el = document.getElementById(id);
    if (!el) return;
    panels[id] = el;
  }

  // Per-panel widths; whichever panel sits in the LAST position flexes (1fr).
  const minWidths: Record<string, number> = { "left-panel": 180, "center-panel": 300, "right-panel": 300 };
  const maxWidths: Record<string, number> = { "left-panel": 500, "center-panel": 800, "right-panel": 900 };
  const widths: Record<string, number> = { "left-panel": 280, "center-panel": 480, "right-panel": 600 };
  let order: string[] = [...panelIds];

  // Restore persisted layout (order + widths).
  try {
    const saved = JSON.parse(localStorage.getItem("beetle-panel-layout") || "null");
    if (saved && Array.isArray(saved.order) && saved.order.length === 3 && panelIds.every(id => saved.order.includes(id))) {
      order = saved.order;
    }
    if (saved && saved.widths) {
      for (const id of panelIds) {
        if (typeof saved.widths[id] === "number") widths[id] = saved.widths[id];
      }
    }
  } catch {
    // Corrupt layout state: fall back to defaults.
  }

  function persist() {
    localStorage.setItem("beetle-panel-layout", JSON.stringify({ order, widths }));
  }

  function clampWidth(id: string, w: number): number {
    return Math.max(minWidths[id], Math.min(maxWidths[id], w));
  }

  function applyWidths() {
    const w0 = clampWidth(order[0], widths[order[0]]);
    const w1 = clampWidth(order[1], widths[order[1]]);
    container.style.gridTemplateColumns = `${w0}px 4px ${w1}px 4px 1fr`;
  }

  function applyLayout() {
    // Grid auto-places by DOM order: panel, resizer, panel, resizer, panel.
    container.appendChild(panels[order[0]]);
    container.appendChild(resizerLeft);
    container.appendChild(panels[order[1]]);
    container.appendChild(resizerRight);
    container.appendChild(panels[order[2]]);
    applyWidths();
  }

  // Resizers operate on positions, not specific panels, so they keep working
  // after a reorder: position 0 sizes the first panel, position 1 the second.
  function attachResizer(resizer: HTMLDivElement, position: 0 | 1) {
    resizer.addEventListener("mousedown", (e) => {
      e.preventDefault();
      document.body.classList.add("resizing-active");
      resizer.classList.add("dragging");

      const onMouseMove = (moveEvent: MouseEvent) => {
        const id = order[position];
        const offset = position === 0 ? 0 : clampWidth(order[0], widths[order[0]]) + 4;
        widths[id] = clampWidth(id, moveEvent.clientX - offset);
        applyWidths();
      };

      const onMouseUp = () => {
        document.body.classList.remove("resizing-active");
        resizer.classList.remove("dragging");
        document.removeEventListener("mousemove", onMouseMove);
        document.removeEventListener("mouseup", onMouseUp);
        persist();
      };

      document.addEventListener("mousemove", onMouseMove);
      document.addEventListener("mouseup", onMouseUp);
    });
  }

  attachResizer(resizerLeft, 0);
  attachResizer(resizerRight, 1);

  // Reorder: drag a panel's header onto another panel to swap their positions.
  for (const id of panelIds) {
    const header = panels[id].querySelector(".panel-header") as HTMLElement | null;
    if (!header) continue;
    header.draggable = true;
    header.style.cursor = "grab";
    header.title = "Drag to swap panel positions";
    header.addEventListener("dragstart", (e) => {
      e.dataTransfer?.setData("text/panel-id", id);
      if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
    });
    panels[id].addEventListener("dragover", (e) => {
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    });
    panels[id].addEventListener("drop", (e) => {
      e.preventDefault();
      const fromId = e.dataTransfer?.getData("text/panel-id");
      if (!fromId || fromId === id || !panelIds.includes(fromId)) return;
      const a = order.indexOf(fromId);
      const b = order.indexOf(id);
      [order[a], order[b]] = [order[b], order[a]];
      applyLayout();
      persist();
    });
  }

  applyLayout();
}

// Initialize App
window.addEventListener("DOMContentLoaded", async () => {
  setupEventListeners();
  setupResizablePanels();
  
  if (typeof Notification !== "undefined" && Notification.permission !== "granted" && Notification.permission !== "denied") {
    Notification.requestPermission().catch((e) => console.error(e));
  }

  await setupTauriEventListeners();
  await loadProjects();
});

// Setup Tauri streaming event listeners
async function setupTauriEventListeners() {
  if (isTauri) {
    try {
      globalChunkListener = async (event: any) => {
        const payload = event.payload;
        const runId = payload.run_id;
        
        let stream = activeStreams.get(runId);
        if (!stream) {
          stream = { text: "", bubbleElement: null };
          activeStreams.set(runId, stream);
        }
        
        stream.text += payload.chunk;
        
        const activeKey = getCurrentLogKey();
        if (activeKey === runId) {
          if (!stream.bubbleElement) {
            removeThinkingBubble();
            
            const wrapper = document.createElement("div");
            wrapper.className = "chat-bubble agent";
            
            const meta = document.createElement("div");
            meta.className = "bubble-meta";
            meta.innerHTML = `<span>BeetleAI</span>`;
            wrapper.appendChild(meta);
            
            const content = document.createElement("div");
            content.className = "bubble-content-text";
            wrapper.appendChild(content);
            
            chatMessages.appendChild(wrapper);
            stream.bubbleElement = wrapper;
          }
          
          const isAtBottom = chatMessages.scrollHeight - chatMessages.scrollTop - chatMessages.clientHeight <= 50;
          const contentDiv = stream.bubbleElement.querySelector(".bubble-content-text") as HTMLDivElement;
          if (contentDiv) {
            contentDiv.innerHTML = formatMarkdownInChat(stream.text);
          }
          if (isAtBottom) {
            chatMessages.scrollTop = chatMessages.scrollHeight;
          }
        }
        
        if (payload.done) {
          activeStreams.delete(runId);
          if (activeKey === runId) {
            removeThinkingBubble();
            await updateActiveCardUI();
            if (currentMode === "kanban") {
              await refreshState();
            }
          }
        }
      };

      await listen("chat-chunk", globalChunkListener);
      
      await listen("chat-finished", async (event: any) => {
        const payload = event.payload;
        const runId = payload.run_id;
        if (activeLlmRunId === runId) {
          activeLlmRunId = null;
          updateSendButtonState();
        }
        const activeKey = getCurrentLogKey();
        if (activeKey === runId) {
          removeThinkingBubble();
          chatInput.disabled = false;
          chatInput.focus();
          await refreshState();
          renderRightPanel();
        }
      });
      
      await listen("run-updated", async () => {
        await refreshState();
        if (currentProject) {
          await renderFileTreeRoot(currentProject.path).catch((err) => console.error(err));
        }
      });

      await listen("notification", (event: any) => {
        showSystemNotification(event.payload);
      });
    } catch (err) {
      console.error("Failed to register Tauri event listener:", err);
    }
  } else {
    globalChunkListener = (event: any) => {
      const payload = event.payload;
      const runId = payload.run_id;
      
      let stream = activeStreams.get(runId);
      if (!stream) {
        stream = { text: "", bubbleElement: null };
        activeStreams.set(runId, stream);
      }
      
      stream.text += payload.chunk;
      
      const activeKey = getCurrentLogKey();
      if (activeKey === runId) {
        if (!stream.bubbleElement) {
          removeThinkingBubble();
          
          const wrapper = document.createElement("div");
          wrapper.className = "chat-bubble agent";
          
          const meta = document.createElement("div");
          meta.className = "bubble-meta";
          meta.innerHTML = `<span>BeetleAI</span>`;
          wrapper.appendChild(meta);
          
          const content = document.createElement("div");
          content.className = "bubble-content-text";
          wrapper.appendChild(content);
          
          chatMessages.appendChild(wrapper);
          stream.bubbleElement = wrapper;
        }
        
        const isAtBottom = chatMessages.scrollHeight - chatMessages.scrollTop - chatMessages.clientHeight <= 50;
        const contentDiv = stream.bubbleElement.querySelector(".bubble-content-text") as HTMLDivElement;
        if (contentDiv) {
          contentDiv.innerHTML = formatMarkdownInChat(stream.text);
        }
        if (isAtBottom) {
          chatMessages.scrollTop = chatMessages.scrollHeight;
        }
      }
      
      if (payload.done) {
        activeStreams.delete(runId);
        if (activeKey === runId) {
          removeThinkingBubble();
        }
      }
    };
  }
}

// Setup DOM event listeners
function setupEventListeners() {
  // Mode switcher listeners
  tabPlan.addEventListener("click", () => switchMode("plan"));
  tabKanban.addEventListener("click", () => switchMode("kanban"));
  tabCode.addEventListener("click", () => switchMode("code"));

  // Project select listener
  projectSelect.addEventListener("change", async () => {
    const path = projectSelect.value;
    if (path) {
      try {
        const proj = await invoke<Project>("open_project", { path });
        await selectProject(proj);
      } catch (err) {
        console.error("Failed to open project:", err);
      }
    }
  });

  // Project creation toggle
  btnNewProjectToggle.addEventListener("click", () => {
    pushView({ kind: "new_project" });
  });

  // Settings Modal actions
  btnSettingsToggle.addEventListener("click", () => openSettingsModal());
  btnSettingsClose.addEventListener("click", () => closeSettingsModal());
  btnSettingsCancel.addEventListener("click", () => closeSettingsModal());
  settingsForm.addEventListener("submit", (e) => {
    e.preventDefault();
    saveSettings();
  });
  btnFetchModels.addEventListener("click", () => fetchModels());
  settingsModelSelect.addEventListener("change", () => onModelSelectChange());
  btnToggleModelInput.addEventListener("click", () => toggleModelInput(true));

  // Provider dropdown listener to auto-update url placeholder
  settingsProvider.addEventListener("change", () => {
    const provider = settingsProvider.value;
    if (provider === "openai") {
      settingsUrl.placeholder = "https://api.openai.com/v1";
    } else if (provider === "anthropic") {
      settingsUrl.placeholder = "https://api.anthropic.com/v1";
    } else if (provider === "lmstudio") {
      settingsUrl.placeholder = "http://localhost:1234";
    } else if (provider === "ollama") {
      settingsUrl.placeholder = "http://localhost:11434";
    } else if (provider === "custom") {
      settingsUrl.placeholder = "http://localhost:8080/v1";
    }
  });

  // URL input listeners to auto-fetch models when entering a URL
  settingsUrl.addEventListener("change", () => {
    fetchModels(true); // silent fetch on blur/change
  });
  settingsUrl.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault(); // prevent form from submitting
      fetchModels(false); // explicit fetch, alerts on error
    }
  });

  // Run controls listeners
  btnStartRun.addEventListener("click", async () => {
    if (!activeCard) return;
    try {
      const runId = await invoke<string>("start_run", { cardId: activeCard.id });
      activeCard.status = "running";
      activeCard.run_id = runId;
      await refreshState();
      pushView({ kind: "diff", runId });
    } catch (err) {
      console.error(err);
      showToast("Failed to start run: " + err, "error");
    }
  });

  btnCancelRun.addEventListener("click", async () => {
    if (!activeCard || !activeCard.run_id) return;
    try {
      await invoke("cancel_run", { runId: activeCard.run_id });
      activeCard.status = "failed";
      await refreshState();
    } catch (err) {
      console.error(err);
      showToast("Failed to cancel run: " + err, "error");
    }
  });

  btnAcceptRun.addEventListener("click", async () => {
    if (!activeCard || !activeCard.run_id) return;
    try {
      await invoke("accept_run", { runId: activeCard.run_id });
      activeCard.status = "done";
      await refreshState();
      switchMode("kanban");
    } catch (err) {
      console.error(err);
      showToast("Failed to accept changes: " + err, "error");
    }
  });

  btnRejectRun.addEventListener("click", async () => {
    if (!activeCard || !activeCard.run_id) return;
    try {
      await invoke("reject_run", { runId: activeCard.run_id });
      activeCard.status = "failed";
      await refreshState();
      switchMode("kanban");
    } catch (err) {
      console.error(err);
      showToast("Failed to reject changes: " + err, "error");
    }
  });

  // Chat input listener
  btnSendChat.addEventListener("click", () => {
    if (activeLlmRunId) {
      abortActiveChat();
    } else {
      submitChat();
    }
  });
  chatInput.addEventListener("keydown", (e) => {
    // Enter sends; Shift+Enter inserts a newline.
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submitChat();
    }
  });
  chatInput.addEventListener("input", () => autosizeChatInput());

  // Viewer back button
  btnViewerBack.addEventListener("click", () => popView());

  // File system modal close buttons
  btnFsClose.addEventListener("click", () => closeFsDialog());
  btnFsCancel.addEventListener("click", () => closeFsDialog());

  // File system creation form submit
  fsForm.addEventListener("submit", async (e) => {
    e.preventDefault();
    if (!currentProject) return;
    const parentPath = fsParentPath.value;
    const type = fsItemType.value;
    let name = fsItemName.value.trim();
    if (!name) return;

    if (type === "doc" && !name.endsWith(".md")) {
      name += ".md";
    }

    try {
      if (type === "doc") {
        const initialContent = `# ${name.replace(/\.md$/, "")}\n\nStart drafting your design here.\n`;
        await invoke("write_design_doc", {
          projectPath: currentProject.path,
          docName: name,
          content: initialContent
        });
        closeFsDialog();
        await loadDesignDocs();
        selectedDesignDoc = name;
        pushView({ kind: "doc", docName: name });
      } else {
        const separator = parentPath.includes("\\") ? "\\" : "/";
        const targetPath = parentPath ? `${parentPath}${separator}${name}` : `${currentProject.path}${separator}${name}`;
        
        if (type === "file") {
          await invoke("create_file", { projectPath: currentProject.path, path: targetPath });
        } else {
          await invoke("create_dir", { projectPath: currentProject.path, path: targetPath });
        }
        closeFsDialog();
        await renderFileTreeRoot(currentProject.path);
      }
    } catch (err) {
      showToast(`Error creating item: ${err}`, "error");
    }
  });

  // Sidebar creation triggers
  btnRootAddFile.addEventListener("click", () => openFsDialog("", "file"));
  btnRootAddFolder.addEventListener("click", () => openFsDialog("", "dir"));
  btnAddDesignDoc.addEventListener("click", () => openFsDialog("", "doc"));

  // Viewer Edit Mode Handlers
  btnViewerEdit.addEventListener("click", async () => {
    const currentView = viewerStack[viewerStack.length - 1];
    if (!currentView) return;
    
    if (currentView.kind === "doc") {
      const docName = currentView.docName || "design.md";
      try {
        const text = await invoke<string>("read_design_doc", { projectPath: currentProject ? currentProject.path : ".", docName });
        viewerEditBuffer = text;
        isEditingViewer = true;
        renderRightPanel();
      } catch (err) {
        showToast("Failed to read document for editing: " + err, "error");
      }
    } else if (currentView.kind === "file") {
      try {
        const code = await invoke<string>("read_file", { path: currentView.path });
        viewerEditBuffer = code;
        isEditingViewer = true;
        renderRightPanel();
      } catch (err) {
        showToast("Failed to read file for editing: " + err, "error");
      }
    } else if (currentView.kind === "card_detail") {
      isEditingViewer = true;
      renderRightPanel();
    }
  });

  btnViewerCancel.addEventListener("click", () => {
    isEditingViewer = false;
    renderRightPanel();
  });

  btnViewerSave.addEventListener("click", async () => {
    const currentView = viewerStack[viewerStack.length - 1];
    if (!currentView) return;
    
    if (currentView.kind === "card_detail") {
      const titleInput = document.getElementById("viewer-card-title-input") as HTMLInputElement;
      const descTextarea = document.getElementById("viewer-card-desc-textarea") as HTMLTextAreaElement;
      if (titleInput && descTextarea) {
        const card = cardsList.find(c => c.id === currentView.cardId);
        if (card) {
          card.title = titleInput.value;
          card.description = descTextarea.value;
          try {
            await saveCardObject(card);
            isEditingViewer = false;
            await refreshState();
          } catch (err) {
            showToast("Failed to save card: " + err, "error");
          }
        }
      }
      return;
    }

    const textarea = document.getElementById("viewer-editor-textarea") as HTMLTextAreaElement;
    if (!textarea) return;
    const newContent = textarea.value;
    
    if (currentView.kind === "doc") {
      const docName = currentView.docName || "design.md";
      try {
        await invoke("write_design_doc", {
          projectPath: currentProject ? currentProject.path : ".",
          docName,
          content: newContent
        });
        isEditingViewer = false;
        renderRightPanel();
      } catch (err) {
        showToast("Failed to save document: " + err, "error");
      }
    } else if (currentView.kind === "file") {
      try {
        await invoke("save_file", {
          projectPath: currentProject ? currentProject.path : ".",
          path: currentView.path,
          content: newContent
        });
        isEditingViewer = false;
        renderRightPanel();
      } catch (err) {
        showToast("Failed to save file: " + err, "error");
      }
    }
  });
}

let fetchedModels: { name: string; is_loaded: boolean; context_size: number | null }[] = [];

async function openSettingsModal() {
  try {
    const settings = await invoke<LlmSettings>("get_settings");
    settingsProvider.value = settings.provider;
    settingsUrl.value = settings.api_url;
    settingsKey.value = settings.api_key;
    settingsModel.value = settings.model;
    settingsSteps.value = settings.max_steps.toString();
    
    // Reset model selection view states
    toggleModelInput(true);
    settingsModal.style.display = "flex";

    // Auto-fetch models in the background if a URL is already present
    if (settingsUrl.value.trim()) {
      fetchModels(true);
    }
  } catch (err) {
    console.error("Failed to load settings:", err);
  }
}

function closeSettingsModal() {
  settingsModal.style.display = "none";
}

async function saveSettings() {
  const settings: LlmSettings = {
    provider: settingsProvider.value,
    api_url: settingsUrl.value,
    api_key: settingsKey.value,
    model: settingsModel.value,
    max_steps: parseInt(settingsSteps.value, 10) || 50
  };

  try {
    await invoke("save_settings", { settings });
    closeSettingsModal();
    showToast("LLM settings saved", "success");
  } catch (err) {
    console.error("Failed to save settings:", err);
    showToast("Error saving settings: " + err, "error");
  }
}

async function fetchModels(silentOnFailure = false) {
  const url = settingsUrl.value.trim();
  if (!url) {
    if (!silentOnFailure) {
      showToast("Please enter a valid API Base URL first.", "info");
    }
    return;
  }

  btnFetchModels.disabled = true;
  btnFetchModels.textContent = "Querying...";

  try {
    const models = await invoke<{ name: string; is_loaded: boolean; context_size: number | null }[]>("fetch_local_models", { url, provider: settingsProvider.value });
    fetchedModels = models;
    
    settingsModelSelect.innerHTML = `<option value="">Select a model...</option>`;
    models.forEach((m) => {
      const option = document.createElement("option");
      option.value = m.name;
      const ctxLabel = m.context_size ? ` (${(m.context_size / 1024).toFixed(0)}k ctx)` : '';
      option.textContent = `${m.name} ${m.is_loaded ? ' (Loaded)' : ' (Idle)'}${ctxLabel}`;
      settingsModelSelect.appendChild(option);
    });

    const hasMatchingModel = settingsModel.value && models.some(m => m.name === settingsModel.value);
    if (models.length > 0 && (!silentOnFailure || hasMatchingModel)) {
      toggleModelInput(false);
      if (hasMatchingModel) {
        settingsModelSelect.value = settingsModel.value;
        onModelSelectChange();
      }
    } else {
      toggleModelInput(true);
    }
  } catch (err) {
    console.error("Failed to retrieve models:", err);
    if (!silentOnFailure) {
      showToast("Failed to retrieve models: " + err, "error");
    }
  } finally {
    btnFetchModels.disabled = false;
    btnFetchModels.textContent = "Fetch";
  }
}

function onModelSelectChange() {
  const selected = settingsModelSelect.value;
  if (!selected) {
    modelContextInfo.style.display = "none";
    return;
  }

  settingsModel.value = selected;
  
  const m = fetchedModels.find(item => item.name === selected);
  if (m) {
    modelContextInfo.textContent = `VRAM Status: ${m.is_loaded ? 'Loaded (Running)' : 'Idle'} | Context Window: ${m.context_size ? m.context_size.toLocaleString() + ' tokens' : 'Unknown'}`;
    modelContextInfo.style.display = "block";
  } else {
    modelContextInfo.style.display = "none";
  }
}

function toggleModelInput(manual: boolean) {
  if (manual) {
    settingsModel.style.display = "block";
    settingsModelSelect.style.display = "none";
    btnToggleModelInput.style.display = "none";
    modelContextInfo.style.display = "none";
  } else {
    settingsModel.style.display = "none";
    settingsModelSelect.style.display = "block";
    btnToggleModelInput.style.display = "block";
  }
}

// Switch navigation mode
function switchMode(mode: "plan" | "kanban" | "code") {
  currentMode = mode;
  tabPlan.classList.toggle("active", mode === "plan");
  tabKanban.classList.toggle("active", mode === "kanban");
  tabCode.classList.toggle("active", mode === "code");

  if (mode === "plan") {
    designDocsSection.style.display = "block";
    repoWorkspaceSection.style.display = "none";
    activeCard = null;
    loadDesignDocs();
  } else if (mode === "kanban") {
    designDocsSection.style.display = "none";
    repoWorkspaceSection.style.display = "none";
    activeCard = null;
    pushView({ kind: "kanban" });
    updateActiveCardUI();
  } else if (mode === "code") {
    designDocsSection.style.display = "none";
    repoWorkspaceSection.style.display = "block";
    const currentView = viewerStack[viewerStack.length - 1];
    if (currentView && currentView.kind === "file") {
      // Keep it
    } else {
      pushView({ kind: "doc", docName: selectedDesignDoc });
    }
    updateActiveCardUI();
  }
}

async function loadDesignDocs() {
  if (!currentProject) return;
  try {
    const docs = await invoke<string[]>("list_design_docs", { projectPath: currentProject.path });
    renderDesignDocs(docs);
    
    if (docs.length > 0) {
      if (!selectedDesignDoc || !docs.includes(selectedDesignDoc)) {
        selectedDesignDoc = docs[0];
      }
      pushView({ kind: "doc", docName: selectedDesignDoc });
    }
  } catch (err) {
    console.error("Failed to load design docs:", err);
    designDocsList.innerHTML = `<div class="empty-state">Failed to load design docs</div>`;
  }
}

function renderDesignDocs(docs: string[]) {
  designDocsList.innerHTML = "";
  if (docs.length === 0) {
    designDocsList.innerHTML = `
      <div class="empty-state">
        <span class="empty-state-icon">📄</span>
        <span>No design docs found</span>
      </div>
    `;
    return;
  }
  
  docs.forEach((doc) => {
    const item = document.createElement("div");
    item.className = `tree-node file-node ${selectedDesignDoc === doc ? "active" : ""}`;
    item.innerHTML = `
      <span class="icon">📄</span>
      <span>${doc}</span>
    `;
    
    item.addEventListener("click", () => {
      selectedDesignDoc = doc;
      document.querySelectorAll("#design-docs-list .tree-node").forEach((el) => el.classList.remove("active"));
      item.classList.add("active");
      pushView({ kind: "doc", docName: doc });
      updateActiveCardUI();
    });
    
    designDocsList.appendChild(item);
  });
}

// Fetch list of projects
async function loadProjects() {
  try {
    const projects = await invoke<Project[]>("list_projects");
    projectSelect.innerHTML = `<option value="">Select a repository...</option>`;
    projects.forEach((p) => {
      const option = document.createElement("option");
      option.value = p.path;
      option.textContent = `${p.name} (${p.path})`;
      projectSelect.appendChild(option);
    });

    if (projects.length > 0) {
      projectSelect.value = projects[0].path;
      await selectProject(projects[0]);
    } else {
      const fallbackProject = {
        id: "beetleai",
        name: "BeetleAI Harness",
        path: "."
      };
      await selectProject(fallbackProject);
    }
  } catch (err) {
    console.error("Failed to load projects:", err);
    const fallbackProject = {
      id: "beetleai",
      name: "BeetleAI Harness",
      path: "."
    };
    await selectProject(fallbackProject);
  }
}

// Open active project
async function selectProject(project: Project) {
  currentProject = project;
  await refreshState();
  
  // Load initial root directory tree
  if (currentProject) {
    await renderFileTreeRoot(currentProject.path).catch((err) => console.error("Failed to render tree:", err));
  }

  // Switch to Kanban mode so cards are immediately visible!
  switchMode("kanban");
}

// Reload cards, controls and chat transcript
async function refreshState() {
  if (!currentProject) {
    currentProject = {
      id: "beetleai",
      name: "BeetleAI Harness",
      path: "."
    };
  }
  try {
    cardsList = await invoke<Card[]>("list_cards", { projectPath: currentProject.path });
    
    // Refresh selected card if it is in progress
    if (activeCard) {
      const updated = cardsList.find((c) => c.id === activeCard!.id);
      if (updated) {
        activeCard = updated;
      }
    }
    
    // Always update chat UI and right panel when refreshing state
    await updateActiveCardUI();
    renderRightPanel();
  } catch (err) {
    console.error("Error refreshing state:", err);
    // Safe client-side fallback if backend calls fail
    if (cardsList.length === 0) {
      cardsList = [
        {
          id: "card_1",
          project_path: ".",
          title: "Bootstrapping & Three-Column UI Layout",
          description: "Setup Tauri v2 template with TypeScript, and construct the basic grid UI and styles.",
          status: "done",
          run_id: "run_card_1",
          assignee: "BeetleAI",
          todo_list: [
            { text: "Configure Tauri v2 project template", completed: true },
            { text: "Build TypeScript sidebar navigation and panels", completed: true },
            { text: "Construct CSS layouts and themes", completed: true }
          ]
        },
        {
          id: "card_2",
          project_path: ".",
          title: "Git Worktree Integration & Sandbox",
          description: "Implement git worktree creation, merge, and discard actions. Ensure filesystem is sandboxed.",
          status: "review",
          run_id: "run_card_2",
          assignee: "BeetleAI",
          todo_list: [
            { text: "Implement git worktree creation helpers", completed: true },
            { text: "Integrate file deletion and modification boundaries", completed: true },
            { text: "Verify sandbox path traversal checks", completed: false }
          ]
        },
        {
          id: "card_3",
          project_path: ".",
          title: "Card State Store & Persistency Layer",
          description: "Integrate SQLite and persist project files, cards, and execution transcripts.",
          status: "todo",
          run_id: null,
          assignee: null,
          todo_list: [
            { text: "Design database schema for cards and logs", completed: false },
            { text: "Integrate SQLite driver and migrations", completed: false },
            { text: "Implement state persistence interface", completed: false }
          ]
        },
        {
          id: "card_4",
          project_path: ".",
          title: "Autonomous Loop Run Engine",
          description: "Build the tokio task worker loop that fetches model responses, executes tools, and sends events.",
          status: "backlog",
          run_id: null,
          assignee: null,
          todo_list: [
            { text: "Construct Tokio worker thread loop", completed: false },
            { text: "Implement model response stream parsing", completed: false },
            { text: "Add recursive tool routing handlers", completed: false }
          ]
        }
      ];
      renderRightPanel();
    }
  }
}

// Set active card detail view
async function selectCard(card: Card) {
  activeCard = card;
  updateActiveCardUI();
  pushView({ kind: "card_detail", cardId: card.id });
}

function updateSendButtonState() {
  if (activeLlmRunId) {
    btnSendChat.disabled = false;
    btnSendChat.innerText = "⏹";
    btnSendChat.classList.remove("btn-primary");
    btnSendChat.classList.add("btn-danger");
    btnSendChat.title = "Stop Generation";
  } else {
    btnSendChat.innerText = "→";
    btnSendChat.classList.remove("btn-danger");
    btnSendChat.classList.add("btn-primary");
    btnSendChat.title = "Send Message";
    
    if (currentMode === "plan") {
      btnSendChat.disabled = false;
    } else if (currentMode === "code") {
      const currentView = viewerStack[viewerStack.length - 1];
      btnSendChat.disabled = !(currentView && currentView.kind === "file");
    } else { // kanban
      if (!activeCard) {
        btnSendChat.disabled = true;
      } else {
        btnSendChat.disabled = !(activeCard.status === "running" || activeCard.status === "blocked");
      }
    }
  }
}

async function abortActiveChat() {
  if (!activeLlmRunId) return;
  const runId = activeLlmRunId;
  btnSendChat.disabled = true;
  if (isTauri) {
    try {
      await invoke("abort_chat", { runId });
    } catch (err) {
      console.error("Failed to abort chat:", err);
    }
  } else {
    const stream = activeStreams.get(runId);
    if (stream) {
      if (globalChunkListener) {
        globalChunkListener({
          payload: {
            run_id: runId,
            chunk: "\n[Chat stopped by user]",
            done: true,
            error: null
          }
        });
      }
    }
  }
  activeLlmRunId = null;
  updateSendButtonState();
}

// Render center column active card and run controls
async function updateActiveCardUI() {
  // 1. Plan Mode Chat UI
  if (currentMode === "plan") {
    activeCardTitle.textContent = `Design Document: ${selectedDesignDoc}`;
    runStatusBadge.style.display = "none";
    controlsRun.style.display = "none";
    controlsActive.style.display = "none";
    controlsReview.style.display = "none";
    
    chatInput.disabled = false;
    btnSendChat.disabled = false;
    chatInput.placeholder = "Suggest changes to design doc...";
    
    try {
      const logs = await invoke<RunEvent[]>("get_design_log", {
        projectPath: currentProject ? currentProject.path : ".",
        docName: selectedDesignDoc
      });
      renderLogs(logs);
    } catch (err) {
      console.error(err);
      chatMessages.innerHTML = `<div class="empty-state">Failed to load chat history</div>`;
    }
    return;
  }

  // 2. Code Mode Chat UI
  if (currentMode === "code") {
    runStatusBadge.style.display = "none";
    controlsRun.style.display = "none";
    controlsActive.style.display = "none";
    controlsReview.style.display = "none";
    
    const currentView = viewerStack[viewerStack.length - 1];
    if (currentView && currentView.kind === "file") {
      activeCardTitle.textContent = `Edit File: ${currentView.name}`;
      chatInput.disabled = false;
      btnSendChat.disabled = false;
      chatInput.placeholder = "Ask agent to edit this file...";
      
      try {
        const logs = await invoke<RunEvent[]>("get_code_log", {
          projectPath: currentProject ? currentProject.path : ".",
          filePath: currentView.path
        });
        renderLogs(logs);
      } catch (err) {
        console.error(err);
        chatMessages.innerHTML = `<div class="empty-state">Failed to load chat history</div>`;
      }
    } else {
      activeCardTitle.textContent = "Code Workspace";
      chatInput.disabled = false;
      btnSendChat.disabled = false;
      chatInput.placeholder = "Ask agent to write code or search files...";
      
      try {
        const logs = await invoke<RunEvent[]>("get_code_log", {
          projectPath: currentProject ? currentProject.path : ".",
          filePath: ""
        });
        if (logs.length === 0) {
          chatMessages.innerHTML = `
            <div class="empty-state">
              <span class="empty-state-icon">💻</span>
              <span>Ask the agent to write new code, search files, or explain parts of the codebase.</span>
            </div>
          `;
        } else {
          renderLogs(logs);
        }
      } catch (err) {
        console.error(err);
        chatMessages.innerHTML = `<div class="empty-state">Failed to load chat history</div>`;
      }
    }
    return;
  }

  // 3. Kanban Mode Chat UI
  if (!activeCard) {
    activeCardTitle.textContent = "Select a Card";
    runStatusBadge.style.display = "none";
    controlsRun.style.display = "none";
    controlsActive.style.display = "none";
    controlsReview.style.display = "none";
    chatInput.disabled = true;
    btnSendChat.disabled = true;
    chatInput.placeholder = "Select a card to chat...";
    chatMessages.innerHTML = `
      <div class="empty-state">
        <span class="empty-state-icon">🤖</span>
        <span>Select a card from the Kanban board to inspect its execution status.</span>
      </div>
    `;
    return;
  }

  activeCardTitle.textContent = activeCard.title;
  runStatusBadge.className = `run-status-badge status-${activeCard.status}`;
  runStatusText.textContent = activeCard.status;
  runStatusBadge.style.display = "flex";

  controlsRun.style.display = "none";
  controlsActive.style.display = "none";
  controlsReview.style.display = "none";
  
  chatInput.disabled = true;
  btnSendChat.disabled = true;
  chatInput.placeholder = "No active run for this card...";

  if (activeCard.status === "backlog" || activeCard.status === "todo" || activeCard.status === "failed") {
    controlsRun.style.display = "block";
    chatInput.placeholder = "Start a run to put the agent to work on this card...";
  } else if (activeCard.status === "running") {
    controlsActive.style.display = "block";
    chatInput.disabled = false;
    btnSendChat.disabled = false;
    chatInput.placeholder = "Interject message to agent...";
  } else if (activeCard.status === "blocked") {
    controlsActive.style.display = "block";
    chatInput.disabled = false;
    btnSendChat.disabled = false;
    chatInput.placeholder = "Provide feedback to unblock agent...";
  } else if (activeCard.status === "review") {
    controlsReview.style.display = "flex";
    chatInput.placeholder = "Run finished — review the diff and accept or reject...";
  }

  if (activeCard.run_id) {
    try {
      const logs = await invoke<RunEvent[]>("get_run_log", { runId: activeCard.run_id });
      renderLogs(logs);
    } catch (err) {
      console.error("Error reading run logs:", err);
    }
  } else {
    chatMessages.innerHTML = `
      <div class="empty-state">
        <span class="empty-state-icon">📋</span>
        <span><strong>Card Scope:</strong></span>
        <p style="color: var(--text-secondary); margin-top: 5px;">${activeCard.description}</p>
        <span style="font-size: 0.85rem; color: var(--text-muted); margin-top: 10px;">Click 'Start Run' to trigger isolated work.</span>
      </div>
    `;
  }

  const activeKey = getCurrentLogKey();
  if (activeKey) {
    try {
      const active = await invoke<boolean>("is_run_active", { runId: activeKey });
      if (active) {
        activeLlmRunId = activeKey;
      } else {
        if (activeLlmRunId === activeKey) {
          activeLlmRunId = null;
        }
      }
    } catch (err) {
      console.error("Failed to check run active state:", err);
    }
  } else {
    activeLlmRunId = null;
  }
  updateSendButtonState();
}

// Render run execution transcript logs
function renderLogs(logs: RunEvent[]) {
  chatMessages.innerHTML = "";
  if (logs.length === 0) {
    chatMessages.innerHTML = `<p class="empty-state">No execution logs yet.</p>`;
    return;
  }

  // Preprocess events to pair tool_call and tool_result
  const processedLogs: any[] = [];
  let pendingToolCall: any = null;

  logs.forEach((log) => {
    if (log.event_type === "tool_call") {
      if (pendingToolCall) {
        processedLogs.push(pendingToolCall);
      }
      pendingToolCall = {
        event_type: "tool_call_paired",
        call: log,
        result: null
      };
    } else if (log.event_type === "tool_result") {
      if (pendingToolCall) {
        pendingToolCall.result = log;
        processedLogs.push(pendingToolCall);
        pendingToolCall = null;
      } else {
        processedLogs.push(log);
      }
    } else {
      if (pendingToolCall) {
        processedLogs.push(pendingToolCall);
        pendingToolCall = null;
      }
      processedLogs.push(log);
    }
  });
  if (pendingToolCall) {
    processedLogs.push(pendingToolCall);
  }

  processedLogs.forEach((log) => {
    if (log.event_type === "status") {
      const div = document.createElement("div");
      div.className = "bubble-meta";
      div.style.alignSelf = "center";
      div.style.margin = "8px 0";
      div.textContent = `⚙ State transitioned: ${log.payload.toUpperCase()}`;
      chatMessages.appendChild(div);
      return;
    }

    if (log.event_type === "blocked") {
      // Surface why the run paused. New payloads are JSON {reason, message};
      // older ones may be a plain string.
      let reason = "";
      let message = "";
      try {
        const parsed = JSON.parse(log.payload);
        reason = parsed.reason || "";
        message = parsed.message || "";
      } catch {
        message = log.payload;
      }
      const reasonLabels: Record<string, string> = {
        question: "Awaiting your input",
        error: "Paused after an error",
        step_ceiling: "Step limit reached"
      };
      const heading = reasonLabels[reason] || "Run paused";
      const wrapper = document.createElement("div");
      wrapper.className = "chat-bubble blocked-notice";
      wrapper.style.alignSelf = "center";
      wrapper.style.width = "95%";
      wrapper.style.maxWidth = "95%";
      wrapper.style.margin = "8px 0";
      wrapper.style.borderLeft = "4px solid var(--status-blocked, #d6a417)";
      wrapper.style.background = "var(--bg-secondary)";
      wrapper.style.padding = "10px 12px";
      wrapper.style.borderRadius = "4px";
      wrapper.innerHTML = `<div style="font-weight:600;font-size:0.85rem;margin-bottom:4px;">⏸ ${escapeHtml(heading)}</div>` +
        (message ? `<div style="font-size:0.85rem;color:var(--text-secondary);">${escapeHtml(message)}</div>` : "");
      chatMessages.appendChild(wrapper);
      return;
    }

    if (log.event_type === "reasoning") {
      const wrapper = document.createElement("div");
      wrapper.className = "chat-bubble agent reasoning-bubble";
      wrapper.style.alignSelf = "flex-start";
      wrapper.style.width = "95%";
      wrapper.style.maxWidth = "95%";
      wrapper.style.padding = "0";
      wrapper.style.border = "none";
      wrapper.style.backgroundColor = "transparent";
      
      const content = document.createElement("div");
      content.innerHTML = formatMarkdownInChat(`<think>${log.payload}</think>`);
      wrapper.appendChild(content);
      
      chatMessages.appendChild(wrapper);
      return;
    }

    if (log.event_type === "message") {
      try {
        const msg = JSON.parse(log.payload);
        const isAgent = msg.role === "agent";
        
        const wrapper = document.createElement("div");
        wrapper.className = `chat-bubble ${isAgent ? 'agent' : 'user'}`;
        
        const meta = document.createElement("div");
        meta.className = "bubble-meta";
        meta.innerHTML = `<span>${isAgent ? 'Agent' : 'You'}</span>`;
        wrapper.appendChild(meta);

        const content = document.createElement("div");
        content.innerHTML = formatMarkdownInChat(msg.content);
        wrapper.appendChild(content);

        chatMessages.appendChild(wrapper);
      } catch (err) {
        console.error(err);
      }
      return;
    }

    if (log.event_type === "tool_call_paired") {
      try {
        const callDetails = JSON.parse(log.call.payload);
        const resultDetails = log.result ? JSON.parse(log.result.payload) : null;
        
        const wrapper = document.createElement("div");
        wrapper.className = "chat-bubble tool";
        
        const toolName = callDetails.tool || callDetails.name || "unknown";
        const argsStr = escapeHtml(JSON.stringify(callDetails.args || callDetails.arguments, null, 2));
        
        let resultSection = "";
        if (resultDetails) {
          const resultStr = escapeHtml(resultDetails.result || "");
          resultSection = `
            <div class="tool-result-header" style="margin-top: 8px; font-weight: 600; font-size: 0.8rem; color: var(--text-secondary);">Result:</div>
            <pre class="tool-details"><code>${resultStr}</code></pre>
          `;
        } else {
          resultSection = `
            <div class="tool-result-header" style="margin-top: 8px; font-weight: 600; font-size: 0.8rem; color: var(--text-muted); font-style: italic;">Executing...</div>
          `;
        }
        
        // Collapsed by default (no 'open' attribute)
        wrapper.innerHTML = `
          <details style="width: 100%;">
            <summary class="tool-summary">
              <span class="tool-status-icon">${resultDetails ? '✔️' : '⚙️'}</span>
              <span>Tool: <strong>${toolName}</strong></span>
            </summary>
            <div class="tool-details-content" style="padding: 4px 8px 8px 8px;">
              <div style="font-weight: 600; font-size: 0.8rem; color: var(--text-secondary);">Arguments:</div>
              <pre class="tool-details"><code>${argsStr}</code></pre>
              ${resultSection}
            </div>
          </details>
        `;
        chatMessages.appendChild(wrapper);
      } catch (err) {
        console.error(err);
      }
      return;
    }

    // Fallback for raw unpaired logs
    if (log.event_type === "tool_call" || log.event_type === "tool_result") {
      try {
        const details = JSON.parse(log.payload);
        const wrapper = document.createElement("div");
        wrapper.className = "chat-bubble tool";
        
        const isCall = log.event_type === "tool_call";
        const toolName = details.tool || details.name || "unknown";
        
        wrapper.innerHTML = `
          <details style="width: 100%;">
            <summary class="tool-summary">
              <span class="tool-status-icon">${isCall ? '⚙️' : '✔️'}</span>
              <span>${isCall ? 'Calling tool:' : 'Response from:'} <strong>${toolName}</strong></span>
            </summary>
            <pre class="tool-details"><code>${isCall ? escapeHtml(JSON.stringify(details.args, null, 2)) : escapeHtml(details.result || '')}</code></pre>
          </details>
        `;
        chatMessages.appendChild(wrapper);
      } catch (err) {
        console.error(err);
      }
    }
  });

  // Auto scroll to bottom
  chatMessages.scrollTop = chatMessages.scrollHeight;
}

// JavaScript clean path helper to mirror backend logic
function cleanProjectPathJS(path: string): string {
  let p = path.replace(/\\/g, "/");
  if (p.endsWith("/src-tauri")) {
    p = p.substring(0, p.length - 10);
  } else if (p === "src-tauri") {
    p = ".";
  }
  return p;
}

// Get the unique identifier/run ID for the current chat channel
function getCurrentLogKey(): string | null {
  if (currentMode === "plan") {
    const projPath = currentProject ? currentProject.path : ".";
    const cleanProj = cleanProjectPathJS(projPath);
    return `${cleanProj}/design/${selectedDesignDoc}`;
  }
  if (currentMode === "code") {
    const currentView = viewerStack[viewerStack.length - 1];
    const filePath = (currentView && currentView.kind === "file") ? currentView.path : "";
    const projPath = currentProject ? currentProject.path : ".";
    const cleanProj = cleanProjectPathJS(projPath);
    return `${cleanProj}/code/${filePath}`;
  }
  if (currentMode === "kanban" && activeCard) {
    return activeCard.run_id;
  }
  return null;
}

function showSystemNotification(message: string) {
  if (typeof Notification !== "undefined") {
    if (Notification.permission === "granted") {
      new Notification("BeetleAI", { body: message });
    } else if (Notification.permission !== "denied") {
      Notification.requestPermission().then((permission) => {
        if (permission === "granted") {
          new Notification("BeetleAI", { body: message });
        }
      });
    }
  }
}

// Simulated stream generator for browser/mock mode
function mockStreamPromise(runId: string, replyText: string): Promise<void> {
  return new Promise((resolve) => {
    const words = replyText.split(" ");
    let wordIndex = 0;

    // Persist mock reply inside browser session if in Kanban mode
    if (currentMode === "kanban") {
      if (!mockLogs[runId]) mockLogs[runId] = [];
      mockLogs[runId].push({
        run_id: runId,
        event_type: "message",
        payload: JSON.stringify({ role: "agent", content: replyText })
      });
    }

    const timer = setInterval(() => {
      if (wordIndex < words.length) {
        const chunk = words[wordIndex] + " ";
        wordIndex++;
        if (globalChunkListener) {
          globalChunkListener({
            payload: {
              run_id: runId,
              chunk: chunk,
              done: false,
              error: null
            }
          });
        }
      } else {
        clearInterval(timer);
        if (globalChunkListener) {
          globalChunkListener({
            payload: {
              run_id: runId,
              chunk: "",
              done: true,
              error: null
            }
          });
        }
        resolve();
      }
    }, 40);
  });
}

// Grow the chat textarea with its content, capped by the CSS max-height.
function autosizeChatInput() {
  chatInput.style.height = "auto";
  chatInput.style.height = `${Math.min(chatInput.scrollHeight, 160)}px`;
}

// Submit chat message to agent loop
async function submitChat() {
  const text = chatInput.value.trim();
  if (!text) return;

  // Disable input immediately to avoid overlapping requests
  chatInput.disabled = true;
  btnSendChat.disabled = true;

  if (currentMode === "plan") {
    chatInput.value = "";
    autosizeChatInput();
    renderOptimisticUserMessage(text);
    renderThinkingBubble();

    const logKey = getCurrentLogKey();
    if (logKey) {
      activeLlmRunId = logKey;
      updateSendButtonState();
    }
    if (isTauri) {
      invoke("send_design_chat", {
        projectPath: currentProject ? currentProject.path : ".",
        docName: selectedDesignDoc,
        message: text
      }).then(() => {
        // Run started asynchronously in background.
      }).catch((err) => {
        activeLlmRunId = null;
        updateSendButtonState();
        removeThinkingBubble();
        chatInput.disabled = false;
        chatInput.focus();
        console.error(err);
        showToast("Failed to send design chat: " + err, "error");
      });
    } else if (logKey) {
      const mockReply = "I suggest adding validation checks and a database persistence layer to the design document. These requirements will align with our SQLite milestone.";
      mockStreamPromise(logKey, mockReply).then(() => {
        activeLlmRunId = null;
        updateSendButtonState();
        removeThinkingBubble();
        chatInput.disabled = false;
        chatInput.focus();
        updateActiveCardUI();
        renderRightPanel();
      });
    }
    return;
  }

  if (currentMode === "code") {
    const currentView = viewerStack[viewerStack.length - 1];
    const filePath = (currentView && currentView.kind === "file") ? currentView.path : "";
    
    chatInput.value = "";
    autosizeChatInput();
    renderOptimisticUserMessage(text);
    renderThinkingBubble();

    const logKey = getCurrentLogKey();
    if (logKey) {
      activeLlmRunId = logKey;
      updateSendButtonState();
    }
    if (isTauri) {
      invoke("send_code_chat", {
        projectPath: currentProject ? currentProject.path : ".",
        filePath: filePath,
        message: text
      }).then(() => {
        // Run started asynchronously in background.
      }).catch((err) => {
        activeLlmRunId = null;
        updateSendButtonState();
        removeThinkingBubble();
        chatInput.disabled = false;
        chatInput.focus();
        console.error(err);
        showToast("Failed to request code edit: " + err, "error");
      });
    } else if (logKey) {
        const mockReply = "I have updated the file to include standard error logging and cleaner option checking in the command handlers.";
        mockStreamPromise(logKey, mockReply).then(() => {
          activeLlmRunId = null;
          updateSendButtonState();
          removeThinkingBubble();
          chatInput.disabled = false;
          chatInput.focus();
          updateActiveCardUI();
          renderRightPanel();
        });
    }
    return;
  }

  // Kanban Mode
  if (!activeCard || !activeCard.run_id) {
    chatInput.disabled = false;
    btnSendChat.disabled = false;
    return;
  }
  
  chatInput.value = "";
  autosizeChatInput();
  renderOptimisticUserMessage(text);
  renderThinkingBubble();

  const runId = activeCard.run_id;
  const isBlocked = activeCard.status === "blocked";
  
  activeLlmRunId = runId;
  updateSendButtonState();

  if (isTauri) {
    const promise = isBlocked
      ? invoke("unblock_run", { runId, reply: text })
      : invoke("send_chat", { runId, message: text });
      
    if (isBlocked) {
      activeCard.status = "running";
    }

    promise.then(() => {
      removeThinkingBubble();
      chatInput.disabled = false;
      chatInput.focus();
      refreshState();
    }).catch((err) => {
      activeLlmRunId = null;
      updateSendButtonState();
      removeThinkingBubble();
      chatInput.disabled = false;
      chatInput.focus();
      console.error(err);
    });
  } else {
    if (isBlocked) {
      activeCard.status = "running";
    }
    const mockReply = "I will proceed with running git worktree operations and checking dependencies inside the sandbox.";
    mockStreamPromise(runId, mockReply).then(() => {
      activeLlmRunId = null;
      updateSendButtonState();
      removeThinkingBubble();
      chatInput.disabled = false;
      chatInput.focus();
      refreshState();
    });
  }
}

function renderOptimisticUserMessage(text: string) {
  const wrapper = document.createElement("div");
  wrapper.className = "chat-bubble user";
  
  const meta = document.createElement("div");
  meta.className = "bubble-meta";
  meta.innerHTML = `<span>You</span>`;
  wrapper.appendChild(meta);

  const content = document.createElement("div");
  content.innerHTML = formatMarkdownInChat(text);
  wrapper.appendChild(content);

  chatMessages.appendChild(wrapper);
  chatMessages.scrollTop = chatMessages.scrollHeight;
}

function renderThinkingBubble() {
  const wrapper = document.createElement("div");
  wrapper.className = "chat-bubble agent thinking";
  wrapper.id = "thinking-bubble";
  
  const meta = document.createElement("div");
  meta.className = "bubble-meta";
  meta.innerHTML = `<span>Agent</span>`;
  wrapper.appendChild(meta);

  const content = document.createElement("div");
  content.innerHTML = `<span class="thinking-dots">Thinking...</span>`;
  wrapper.appendChild(content);

  chatMessages.appendChild(wrapper);
  chatMessages.scrollTop = chatMessages.scrollHeight;
}

function removeThinkingBubble() {
  const bubble = document.getElementById("thinking-bubble");
  if (bubble) {
    bubble.remove();
  }
}

// Render root directory elements
async function renderFileTreeRoot(path: string) {
  try {
    fileTreeContainer.innerHTML = "";
    const entries = await invoke<DirEntry[]>("list_dir", { path });
    
    entries.forEach((entry) => {
      const node = createFileNode(entry);
      fileTreeContainer.appendChild(node);
    });
  } catch (err) {
    fileTreeContainer.innerHTML = `<span style="color: var(--status-failed)">Error loading tree</span>`;
    console.error(err);
  }
}

// Create file tree node item
function createFileNode(entry: DirEntry): HTMLElement {
  const wrapper = document.createElement("div");
  
  const node = document.createElement("div");
  node.className = `tree-node ${entry.is_dir ? 'directory' : 'file'}`;
  
  let actionsHtml = "";
  if (entry.is_dir) {
    actionsHtml = `
      <div class="tree-node-actions">
        <button class="tree-action-btn add-file" title="Add File">📄+</button>
        <button class="tree-action-btn add-folder" title="Add Folder">📁+</button>
        <button class="tree-action-btn delete-item" title="Delete">🗑️</button>
      </div>
    `;
  } else {
    actionsHtml = `
      <div class="tree-node-actions">
        <button class="tree-action-btn delete-item" title="Delete">🗑️</button>
      </div>
    `;
  }

  node.innerHTML = `
    <span class="icon">${entry.is_dir ? '📁' : '📄'}</span>
    <span class="tree-node-name">${entry.name}</span>
    ${actionsHtml}
  `;
  
  wrapper.appendChild(node);

  if (entry.is_dir) {
    const childrenContainer = document.createElement("div");
    childrenContainer.className = "tree-children";
    childrenContainer.style.display = "none";
    wrapper.appendChild(childrenContainer);

    let loaded = false;

    // Actions click handlers
    const btnAddFile = node.querySelector(".tree-action-btn.add-file") as HTMLButtonElement;
    btnAddFile.addEventListener("click", (e) => {
      e.stopPropagation();
      openFsDialog(entry.path, "file");
    });

    const btnAddFolder = node.querySelector(".tree-action-btn.add-folder") as HTMLButtonElement;
    btnAddFolder.addEventListener("click", (e) => {
      e.stopPropagation();
      openFsDialog(entry.path, "dir");
    });

    const btnDelete = node.querySelector(".tree-action-btn.delete-item") as HTMLButtonElement;
    btnDelete.addEventListener("click", async (e) => {
      e.stopPropagation();
      if (!currentProject) return;
      const confirmDelete = await showConfirm(`Delete the folder "${entry.name}" and all its contents?`, "Delete Folder");
      if (confirmDelete) {
        try {
          await invoke("delete_item", { projectPath: currentProject.path, path: entry.path });
          await renderFileTreeRoot(currentProject.path);
        } catch (err) {
          showToast(`Failed to delete folder: ${err}`, "error");
        }
      }
    });

    node.addEventListener("click", async (e) => {
      e.stopPropagation();
      const isExpanded = childrenContainer.style.display !== "none";
      childrenContainer.style.display = isExpanded ? "none" : "block";
      node.querySelector(".icon")!.textContent = isExpanded ? '📁' : '📂';

      if (!loaded && !isExpanded) {
        try {
          const children = await invoke<DirEntry[]>("list_dir", { path: entry.path });
          childrenContainer.innerHTML = "";
          children.forEach((child) => {
            childrenContainer.appendChild(createFileNode(child));
          });
          loaded = true;
        } catch (err) {
          console.error(err);
        }
      }
    });
  } else {
    const btnDelete = node.querySelector(".tree-action-btn.delete-item") as HTMLButtonElement;
    btnDelete.addEventListener("click", async (e) => {
      e.stopPropagation();
      if (!currentProject) return;
      const confirmDelete = await showConfirm(`Delete the file "${entry.name}"?`, "Delete File");
      if (confirmDelete) {
        try {
          await invoke("delete_item", { projectPath: currentProject.path, path: entry.path });
          const currentView = viewerStack[viewerStack.length - 1];
          if (currentView && currentView.kind === "file" && currentView.path === entry.path) {
            popView();
          } else {
            await renderFileTreeRoot(currentProject.path);
          }
        } catch (err) {
          showToast(`Failed to delete file: ${err}`, "error");
        }
      }
    });

    node.addEventListener("click", (e) => {
      e.stopPropagation();
      document.querySelectorAll(".tree-node.file").forEach((n) => n.classList.remove("active"));
      node.classList.add("active");
      pushView({ kind: "file", path: entry.path, name: entry.name });
      updateActiveCardUI();
    });
  }

  return wrapper;
}

// Viewer Stack navigation helpers
async function pushView(state: ViewerState) {
  isEditingViewer = false;
  // If the view type is doc or kanban, don't duplicate on top of stack
  if (state.kind === "doc" || state.kind === "kanban") {
    viewerStack = [state];
  } else {
    viewerStack.push(state);
  }
  await renderRightPanel();
  await updateActiveCardUI();
}

async function popView() {
  isEditingViewer = false;
  if (viewerStack.length > 1) {
    viewerStack.pop();
    await renderRightPanel();
    await updateActiveCardUI();
  }
}

// Render Polymorphic Viewer based on top view in stack
async function renderRightPanel() {
  const currentView = viewerStack[viewerStack.length - 1];
  
  // Show back button if we are nested
  btnViewerBack.style.display = viewerStack.length > 1 ? "inline-block" : "none";

  // Hide edit actions by default
  btnViewerEdit.style.display = "none";
  btnViewerSave.style.display = "none";
  btnViewerCancel.style.display = "none";

  if (!currentView) {
    viewerTitle.textContent = "Empty";
    viewerContainer.innerHTML = `<div class="empty-state">No view active</div>`;
    viewerContainer.style.display = "";
    viewerContainer.style.flexDirection = "";
    return;
  }

  // Set up flex layout on container if editing
  if (isEditingViewer && (currentView.kind === "doc" || currentView.kind === "file")) {
    viewerContainer.style.display = "flex";
    viewerContainer.style.flexDirection = "column";
    
    // Configure buttons
    btnViewerEdit.style.display = "none";
    btnViewerSave.style.display = "inline-block";
    btnViewerCancel.style.display = "inline-block";
    
    if (currentView.kind === "doc") {
      const docName = currentView.docName || "design.md";
      viewerTitle.textContent = docName;
      viewerContainer.innerHTML = `
        <textarea class="doc-editor-textarea" id="viewer-editor-textarea" style="width: 100%; height: 100%;">${escapeHtml(viewerEditBuffer)}</textarea>
      `;
    } else if (currentView.kind === "file") {
      viewerTitle.textContent = currentView.name;
      viewerContainer.innerHTML = `
        <textarea class="code-editor-textarea" id="viewer-editor-textarea" style="width: 100%; height: 100%;">${escapeHtml(viewerEditBuffer)}</textarea>
      `;
    }
    return;
  }

  // If not editing, restore container layout
  viewerContainer.style.display = "";
  viewerContainer.style.flexDirection = "";

  // Show "Edit" button if editable type
  if (currentView.kind === "doc" || currentView.kind === "file" || currentView.kind === "card_detail") {
    if (isEditingViewer) {
      btnViewerEdit.style.display = "none";
      btnViewerSave.style.display = "inline-block";
      btnViewerCancel.style.display = "inline-block";
    } else {
      btnViewerEdit.style.display = "inline-block";
      btnViewerSave.style.display = "none";
      btnViewerCancel.style.display = "none";
    }
  }

  switch (currentView.kind) {
    case "doc":
      const docName = currentView.docName || "design.md";
      viewerTitle.textContent = docName;
      viewerContainer.innerHTML = `<div class="empty-state">Loading design document...</div>`;
      try {
        const text = await invoke<string>("read_design_doc", { projectPath: currentProject ? currentProject.path : ".", docName });
        viewerContainer.innerHTML = `<div class="markdown-body">${parseMarkdown(text)}</div>`;
      } catch (err) {
        viewerContainer.innerHTML = `<div class="empty-state"><span style="color:var(--status-failed)">Error reading design doc: ${err}</span></div>`;
      }
      break;

    case "kanban":
      viewerTitle.textContent = "Kanban Board";
      renderKanbanBoard();
      break;

    case "file":
      viewerTitle.textContent = currentView.name;
      viewerContainer.innerHTML = `<div class="empty-state">Loading file...</div>`;
      try {
        const code = await invoke<string>("read_file", { path: currentView.path });
        if (currentView.path.endsWith(".md")) {
          viewerContainer.innerHTML = `<div class="markdown-body">${parseMarkdown(code)}</div>`;
        } else {
          viewerContainer.innerHTML = `
            <div class="code-viewer-container">
              <pre class="code-viewer"><code>${escapeHtml(code)}</code></pre>
            </div>
          `;
        }
      } catch (err) {
        viewerContainer.innerHTML = `<div class="empty-state"><span style="color:var(--status-failed)">Error loading file: ${err}</span></div>`;
      }
      break;

    case "diff":
      viewerTitle.textContent = `Review Diff — ${activeCard?.title || ''}`;
      viewerContainer.innerHTML = `<div class="empty-state">Calculating git diff...</div>`;
      try {
        const diffText = await invoke<string>("read_diff", { runId: currentView.runId });
        viewerContainer.innerHTML = renderDiffHtml(diffText);
      } catch (err) {
        viewerContainer.innerHTML = `<div class="empty-state"><span style="color:var(--status-failed)">Error generating diff: ${err}</span></div>`;
      }
      break;

    case "new_project":
      viewerTitle.textContent = "Create / Onboard Project";
      renderNewProjectForm();
      break;

    case "card_detail":
      viewerTitle.textContent = "Card Details";
      const card = cardsList.find((c) => c.id === currentView.cardId);
      if (card) {
        renderCardDetail(card);
      } else {
        viewerContainer.innerHTML = `<div class="empty-state">Card not found</div>`;
      }
      break;
  }
}

// Render horizontal Kanban columns
function renderKanbanBoard() {
  viewerContainer.innerHTML = "";
  const board = document.createElement("div");
  board.className = "kanban-board";

  const columns = [
    { key: "backlog", title: "Backlog" },
    { key: "todo", title: "Todo" },
    { key: "running", title: "In Progress" },
    { key: "blocked", title: "Blocked" },
    { key: "review", title: "Review" },
    { key: "done", title: "Done" },
    { key: "failed", title: "Failed" },
  ];

  columns.forEach((col) => {
    const colDiv = document.createElement("div");
    colDiv.className = "kanban-column";

    const filtered = cardsList.filter((c) => c.status === col.key);

    colDiv.innerHTML = `
      <div class="kanban-column-header">
        <h3>${col.title}</h3>
        <span class="card-count">${filtered.length}</span>
      </div>
      <div class="kanban-cards-list" id="column-${col.key}"></div>
      <div class="kanban-column-footer">
        <button class="add-card-btn">+ Add card</button>
      </div>
    `;

    const listDiv = colDiv.querySelector(".kanban-cards-list") as HTMLDivElement;

    filtered.forEach((card) => {
      const cardDiv = document.createElement("div");
      cardDiv.className = `kanban-card ${activeCard?.id === card.id ? 'selected' : ''}`;
      cardDiv.innerHTML = `
        <div class="kanban-card-title">${formatPlainMarkdown(card.title)}</div>
        <div class="kanban-card-description">${formatPlainMarkdown(card.description)}</div>
        <div class="kanban-card-meta">
          <span>ID: ${card.id}</span>
          ${card.run_id ? `<span style="color: var(--accent-primary); font-weight: 500;">Run Active</span>` : ''}
        </div>
      `;

      cardDiv.addEventListener("click", () => {
        selectCard(card);
        // Toggle selected state visually
        document.querySelectorAll(".kanban-card").forEach((d) => d.classList.remove("selected"));
        cardDiv.classList.add("selected");
      });

      listDiv.appendChild(cardDiv);
    });

    const addCardBtn = colDiv.querySelector(".add-card-btn") as HTMLButtonElement;
    addCardBtn.addEventListener("click", () => {
      // Check if there is already an active composer in this column
      if (listDiv.querySelector(".inline-card-composer")) return;
      
      const composer = document.createElement("div");
      composer.className = "inline-card-composer";
      composer.innerHTML = `
        <textarea class="inline-card-textarea" placeholder="Enter card title..." rows="2" style="width: 100%; background: var(--bg-tertiary); border: 1px solid var(--border-color); border-radius: 4px; color: var(--text-primary); padding: 6px; font-size: 0.85rem; resize: none; margin-bottom: 6px; outline: none;"></textarea>
        <div class="inline-card-controls" style="display: flex; gap: 6px; justify-content: flex-end;">
          <button class="btn btn-secondary cancel-inline-card" style="font-size: 0.75rem; padding: 4px 8px; cursor: pointer;">Cancel</button>
          <button class="btn btn-primary save-inline-card" style="font-size: 0.75rem; padding: 4px 8px; cursor: pointer;">Add Card</button>
        </div>
      `;
      
      listDiv.appendChild(composer);
      listDiv.scrollTop = listDiv.scrollHeight; // Scroll to bottom
      
      const textarea = composer.querySelector(".inline-card-textarea") as HTMLTextAreaElement;
      textarea.focus();
      
      const cancelBtn = composer.querySelector(".cancel-inline-card") as HTMLButtonElement;
      cancelBtn.addEventListener("click", () => {
        composer.remove();
      });
      
      const saveBtn = composer.querySelector(".save-inline-card") as HTMLButtonElement;
      const saveCard = async () => {
        const title = textarea.value.trim();
        if (title) {
          try {
            if (isTauri) {
              await invoke("create_card", { projectPath: currentProject ? currentProject.path : ".", title, description: "Description here...", status: col.key });
            } else {
              const newCard: Card = {
                id: `card_${cardsList.length + 1}`,
                project_path: currentProject ? currentProject.path : ".",
                title,
                description: "Description here...",
                status: col.key,
                run_id: null,
                assignee: null,
                todo_list: []
              };
              cardsList.push(newCard);
            }
            await refreshState();
          } catch (err) {
            showToast("Failed to create card: " + err, "error");
          }
        }
        composer.remove();
      };
      
      saveBtn.addEventListener("click", saveCard);
      textarea.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          saveCard();
        } else if (e.key === "Escape") {
          composer.remove();
        }
      });
    });

    board.appendChild(colDiv);
  });

  viewerContainer.appendChild(board);
}

// Render git unified diff lines nicely with colors
function renderDiffHtml(diff: string): string {
  if (!diff || diff.trim() === "No changes detected in this run.") {
    return `
      <div class="empty-state">
        <span class="empty-state-icon">✔</span>
        <span>No changes detected. The sandbox is clean.</span>
      </div>
    `;
  }

  const files = diff.split("diff --git ");
  let html = `<div class="diff-viewer">`;

  files.forEach((fileDiff) => {
    if (!fileDiff.trim()) return;

    const lines = fileDiff.split("\n");
    const headerLine = lines[0];
    
    // Extract file names
    const parts = headerLine.split(" ");
    const filename = parts[parts.length - 1].replace(/^b\//, "");

    html += `
      <div class="diff-file">
        <div class="diff-file-header">
          <span>${filename}</span>
        </div>
        <div class="diff-lines">
    `;

    for (let i = 1; i < lines.length; i++) {
      const line = lines[i];
      if (line.startsWith("index ") || line.startsWith("--- ") || line.startsWith("+++ ")) {
        continue;
      }

      if (line.startsWith("@@")) {
        // Parse chunk header e.g. @@ -1,5 +1,6 @@
        html += `
          <div class="diff-line chunk-header">
            <div class="diff-line-num">@@</div>
            <div class="diff-line-content">${escapeHtml(line)}</div>
          </div>
        `;
        continue;
      }

      if (line.startsWith("+")) {
        html += `
          <div class="diff-line add">
            <div class="diff-line-num">+</div>
            <div class="diff-line-content">${escapeHtml(line)}</div>
          </div>
        `;
      } else if (line.startsWith("-")) {
        html += `
          <div class="diff-line del">
            <div class="diff-line-num">-</div>
            <div class="diff-line-content">${escapeHtml(line)}</div>
          </div>
        `;
      } else if (line.trim() !== "") {
        html += `
          <div class="diff-line">
            <div class="diff-line-num"> </div>
            <div class="diff-line-content">${escapeHtml(line)}</div>
          </div>
        `;
      }
    }

    html += `
        </div>
      </div>
    `;
  });

  html += `</div>`;
  return html;
}

// ─── Shared inline markdown machinery ────────────────────────────────────────
// Local models love emitting LaTeX inline math like `$\rightarrow$`. We don't
// render math, but we can translate the common symbol commands to unicode.
// The map is a strict allowlist, so dollar amounts ("$5 and $10") and Windows
// paths ("F:\\Projects") are never touched — unknown commands pass through.
const LATEX_TOKEN_MAP: Record<string, string> = {
  rightarrow: "→",
  to: "→",
  longrightarrow: "⟶",
  leftarrow: "←",
  gets: "←",
  Rightarrow: "⇒",
  Leftarrow: "⇐",
  leftrightarrow: "↔",
  Leftrightarrow: "⇔",
  uparrow: "↑",
  downarrow: "↓",
  le: "≤",
  leq: "≤",
  ge: "≥",
  geq: "≥",
  ne: "≠",
  neq: "≠",
  times: "×",
  cdot: "·",
  approx: "≈",
  pm: "±",
  infty: "∞",
  checkmark: "✓",
};

function normalizeLatexTokens(s: string): string {
  // Handles `$\rightarrow$` (with optional inner spaces) and bare `\rightarrow`.
  return s.replace(/\$\s*\\([a-zA-Z]+)\s*\$|\\([a-zA-Z]+)/g, (match, inDollars, bare) => {
    const name = inDollars || bare;
    return LATEX_TOKEN_MAP[name] !== undefined ? LATEX_TOKEN_MAP[name] : match;
  });
}

// Shared inline markdown rules for already-HTML-escaped text. Inline code is
// stashed behind placeholders first so emphasis and LaTeX normalization can
// never mangle code spans, and underscores only toggle emphasis at word
// boundaries (CommonMark behavior) so snake_case identifiers survive intact.
function applyInlineMd(escaped: string): string {
  const codeSpans: string[] = [];
  let out = escaped.replace(/`([^`\n]+)`/g, (_m, code) => {
    codeSpans.push(`<code>${code}</code>`);
    return `\u0000IC${codeSpans.length - 1}\u0000`;
  });
  out = normalizeLatexTokens(out);
  out = out.replace(/\*\*(.*?)\*\*/g, "<strong>$1</strong>");
  out = out.replace(/\*(.*?)\*/g, "<em>$1</em>");
  out = out.replace(/(?<![\w\\])_([^_\n]+)_(?![\w])/g, "<em>$1</em>");
  out = out.replace(/~~(.*?)~~/g, "<del>$1</del>");
  out = out.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank">$1</a>');
  return out.replace(/\u0000IC(\d+)\u0000/g, (_m, i) => codeSpans[Number(i)]);
}

function parseMarkdown(md: string): string {
  // Extract fenced code blocks FIRST so inline rules can't mangle their
  // contents. (Previously the single-backtick inline rule ran before fenced
  // handling and ate the ``` fences themselves.)
  const fencedBlocks: string[] = [];
  let html = md.replace(/```([a-zA-Z0-9_]*)\n([\s\S]*?)```/g, (_m, lang, code) => {
    fencedBlocks.push(`<pre class="chat-code-block"><code class="${lang}">${escapeHtml(code)}</code></pre>`);
    return `\u0000FB${fencedBlocks.length - 1}\u0000`;
  });

  // Escapes html tag markers
  html = escapeHtml(html);

  // Inline formatting rules (shared with chat rendering)
  html = applyInlineMd(html);

  // Split into lines to process block elements (headers, lists, blockquotes)
  const lines = html.split("\n");
  const processedLines = lines.map(line => {
    let trimmed = line.trim();
    
    // Fenced code block placeholder: pass through untouched.
    if (/^\u0000FB\d+\u0000$/.test(trimmed)) {
      return trimmed;
    }
    
    // Headers: # Header to ###### Header
    if (trimmed.startsWith("###### ")) {
      return `<h6>${trimmed.slice(7)}</h6>`;
    }
    if (trimmed.startsWith("##### ")) {
      return `<h5>${trimmed.slice(6)}</h5>`;
    }
    if (trimmed.startsWith("#### ")) {
      return `<h4>${trimmed.slice(5)}</h4>`;
    }
    if (trimmed.startsWith("### ")) {
      return `<h3>${trimmed.slice(4)}</h3>`;
    }
    if (trimmed.startsWith("## ")) {
      return `<h2>${trimmed.slice(3)}</h2>`;
    }
    if (trimmed.startsWith("# ")) {
      return `<h1>${trimmed.slice(2)}</h1>`;
    }
    
    // Alerts and Blockquotes: > [!IMPORTANT], > text, etc.
    if (trimmed.startsWith("&gt; ")) {
      let content = trimmed.slice(5).trim();
      if (content.startsWith("[!IMPORTANT]")) {
        return `<div style="border-left: 4px solid var(--status-failed); background-color: var(--bg-secondary); padding: 10px; margin: 12px 0; border-radius: 4px;"><strong>IMPORTANT:</strong> ${content.slice(12)}</div>`;
      }
      if (content.startsWith("[!NOTE]")) {
        return `<div style="border-left: 4px solid var(--accent-primary); background-color: var(--bg-secondary); padding: 10px; margin: 12px 0; border-radius: 4px;"><strong>NOTE:</strong> ${content.slice(7)}</div>`;
      }
      if (content.startsWith("[!WARNING]")) {
        return `<div style="border-left: 4px solid var(--status-blocked); background-color: var(--bg-secondary); padding: 10px; margin: 12px 0; border-radius: 4px;"><strong>WARNING:</strong> ${content.slice(10)}</div>`;
      }
      return `<blockquote>${content}</blockquote>`;
    }
    
    // Unordered lists: - item, * item
    if (trimmed.startsWith("- ") || trimmed.startsWith("* ")) {
      return `<li style="margin-left: 20px; list-style-type: disc;">${trimmed.slice(2)}</li>`;
    }
    
    // Ordered lists: 1. item, etc.
    const olMatch = trimmed.match(/^(\d+)\.\s+(.*)$/);
    if (olMatch) {
      return `<li style="margin-left: 20px; list-style-type: decimal;">${olMatch[2]}</li>`;
    }
    
    // Normal paragraph (if not inside block tags)
    if (trimmed !== "") {
      return `<p>${line}</p>`;
    }
    
    return line;
  });
  
  return processedLines
    .join("")
    .replace(/\u0000FB(\d+)\u0000/g, (_m, i) => fencedBlocks[Number(i)]);
}

// Inline Markdown formatter inside chat bubbles
function formatMarkdownInChat(text: string): string {
  let result = "";
  let index = 0;
  
  while (index < text.length) {
    const thinkStart = text.indexOf("<think>", index);
    if (thinkStart === -1) {
      result += formatBlocks(text.slice(index));
      break;
    }
    
    result += formatBlocks(text.slice(index, thinkStart));
    
    const thinkEnd = text.indexOf("</think>", thinkStart + 7);
    if (thinkEnd === -1) {
      const thinkContent = text.slice(thinkStart + 7);
      result += `
        <details class="reasoning-details" open>
          <summary class="reasoning-summary">💡 Thinking Process...</summary>
          <div class="reasoning-content">${formatPlainMarkdown(thinkContent)}</div>
        </details>
      `;
      break;
    } else {
      const thinkContent = text.slice(thinkStart + 7, thinkEnd);
      result += `
        <details class="reasoning-details">
          <summary class="reasoning-summary">💡 Thinking Process</summary>
          <div class="reasoning-content">${formatPlainMarkdown(thinkContent)}</div>
        </details>
      `;
      index = thinkEnd + 8;
    }
  }
  
  return result;
}

function formatBlocks(text: string): string {
  let result = "";
  let index = 0;
  
  while (index < text.length) {
    const btMatch = text.slice(index).match(/(?:^|\n)(`{2,5})([a-zA-Z0-9_]*)/);
    if (!btMatch) {
      result += formatPlainMarkdown(text.slice(index));
      break;
    }
    
    const matchStart = index + btMatch.index!;
    const leadingNewline = text[matchStart] === '\n';
    const startOfBt = leadingNewline ? matchStart + 1 : matchStart;
    
    result += formatPlainMarkdown(text.slice(index, startOfBt));
    
    const btLen = btMatch[1].length;
    const bt = "`".repeat(btLen);
    const lang = btMatch[2];
    
    const contentStart = startOfBt + btLen + lang.length;
    const startOfContent = text[contentStart] === '\n' ? contentStart + 1 : contentStart;
    
    const nextBtIdx = text.slice(startOfContent).indexOf(bt);
    
    if (nextBtIdx === -1) {
      const codeContent = text.slice(startOfContent);
      result += `<pre class="chat-code-block"><code class="${lang}">${escapeHtml(codeContent)}</code></pre>`;
      break;
    } else {
      const codeContent = text.slice(startOfContent, startOfContent + nextBtIdx);
      let endOfBlock = startOfContent + nextBtIdx + btLen;
      if (text.slice(endOfBlock).startsWith("<tool_call|>")) {
        endOfBlock += "<tool_call|>".length;
      }
      
      result += `<pre class="chat-code-block"><code class="${lang}">${escapeHtml(codeContent)}</code></pre>`;
      index = endOfBlock;
    }
  }
  
  return result;
}

function formatPlainMarkdown(text: string): string {
  // First escape the entire text
  let escaped = escapeHtml(text);
  
  // Inline formatting rules (shared with doc/card rendering)
  escaped = applyInlineMd(escaped);
  
  // Split into lines to process block elements (headers, lists)
  const lines = escaped.split("\n");
  const processedLines = lines.map(line => {
    let trimmed = line.trim();
    
    // Headers: # Header, ## Header, etc.
    if (trimmed.startsWith("###### ")) {
      return `<h6>${trimmed.slice(7)}</h6>`;
    }
    if (trimmed.startsWith("##### ")) {
      return `<h5>${trimmed.slice(6)}</h5>`;
    }
    if (trimmed.startsWith("#### ")) {
      return `<h4>${trimmed.slice(5)}</h4>`;
    }
    if (trimmed.startsWith("### ")) {
      return `<h3>${trimmed.slice(4)}</h3>`;
    }
    if (trimmed.startsWith("## ")) {
      return `<h2>${trimmed.slice(3)}</h2>`;
    }
    if (trimmed.startsWith("# ")) {
      return `<h1>${trimmed.slice(2)}</h1>`;
    }
    
    // Unordered lists: - item, * item
    if (trimmed.startsWith("- ") || trimmed.startsWith("* ")) {
      return `<li style="margin-left: 20px; list-style-type: disc;">${trimmed.slice(2)}</li>`;
    }
    
    // Ordered lists: 1. item, 2. item, etc.
    const olMatch = trimmed.match(/^(\d+)\.\s+(.*)$/);
    if (olMatch) {
      return `<li style="margin-left: 20px; list-style-type: decimal;">${olMatch[2]}</li>`;
    }
    
    // Blockquote: > text
    if (trimmed.startsWith("&gt; ")) {
      return `<blockquote style="border-left: 3px solid var(--border-color); padding-left: 10px; color: var(--text-secondary); margin: 5px 0;">${trimmed.slice(5)}</blockquote>`;
    }
    
    return line;
  });
  
  return processedLines.join("<br>");
}

// Escape HTML utility
function escapeHtml(str: string): string {
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

// File system dialog helper functions
function openFsDialog(parentPath: string, type: "file" | "dir" | "doc") {
  if (!currentProject) {
    showToast("Please select a project first.", "info");
    return;
  }
  fsParentPath.value = parentPath;
  fsItemType.value = type;
  fsItemName.value = "";
  
  if (type === "file") {
    fsModalTitle.textContent = parentPath ? `Create File inside ${parentPath.split(/[\\/]/).pop()}` : "Create File at Root";
    fsLabelName.textContent = "File Name";
    fsItemName.placeholder = "e.g. index.js";
    btnFsSubmit.textContent = "Create File";
  } else if (type === "dir") {
    fsModalTitle.textContent = parentPath ? `Create Folder inside ${parentPath.split(/[\\/]/).pop()}` : "Create Folder at Root";
    fsLabelName.textContent = "Folder Name";
    fsItemName.placeholder = "e.g. components";
    btnFsSubmit.textContent = "Create Folder";
  } else if (type === "doc") {
    fsModalTitle.textContent = "Create New Design Doc";
    fsLabelName.textContent = "Document Name";
    fsItemName.placeholder = "e.g. requirements.md";
    btnFsSubmit.textContent = "Create Doc";
  }
  
  fsModal.style.display = "flex";
  fsItemName.focus();
}

function closeFsDialog() {
  fsModal.style.display = "none";
}

function renderNewProjectForm() {
  viewerContainer.innerHTML = `
    <div class="new-project-view">
      <form id="new-project-form">
        <div class="form-group">
          <label for="proj-name">Project Name</label>
          <input type="text" id="proj-name" class="form-input" placeholder="e.g. My Awesome App" required />
        </div>
        <div class="form-group">
          <label for="proj-path">Local Directory Path</label>
          <div style="display: flex; gap: 8px;">
            <input type="text" id="proj-path" class="form-input" placeholder="e.g. F:\\Projects\\MyApp" required style="flex: 1;" />
            <button type="button" class="btn btn-secondary" id="btn-proj-path-browse" style="flex: 0; white-space: nowrap;">Browse…</button>
          </div>
          <span style="font-size: 0.8rem; color: var(--text-muted); margin-top: 4px;">Must be a local directory path. We will auto-initialize git and seed requirements if not already present.</span>
        </div>
        <div style="margin-top: 24px; display: flex; gap: 12px; justify-content: flex-end;">
          <button type="button" class="btn btn-secondary" id="btn-new-project-cancel">Cancel</button>
          <button type="submit" class="btn btn-primary">Create & Onboard Project</button>
        </div>
      </form>
    </div>
  `;

  const form = document.getElementById("new-project-form") as HTMLFormElement;
  const cancelBtn = document.getElementById("btn-new-project-cancel") as HTMLButtonElement;
  const browseBtn = document.getElementById("btn-proj-path-browse") as HTMLButtonElement;

  browseBtn.addEventListener("click", async () => {
    const selected = await openDialog({
      directory: true,
      multiple: false,
      title: "Select Project Directory",
    });
    if (typeof selected === "string" && selected) {
      const pathInput = document.getElementById("proj-path") as HTMLInputElement;
      const nameInput = document.getElementById("proj-name") as HTMLInputElement;
      pathInput.value = selected;
      // Convenience: seed the project name from the folder name if empty.
      if (!nameInput.value.trim()) {
        const folder = selected.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || "";
        nameInput.value = folder;
      }
    }
  });

  cancelBtn.addEventListener("click", () => {
    switchMode("kanban");
  });

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const nameInput = document.getElementById("proj-name") as HTMLInputElement;
    const pathInput = document.getElementById("proj-path") as HTMLInputElement;

    const name = nameInput.value.trim();
    const path = pathInput.value.trim();

    if (!name || !path) return;

    try {
      const newProj = await invoke<Project>("create_project", { name, path });
      showToast(`Project "${name}" created and onboarded`, "success");
      await loadProjects();
      projectSelect.value = newProj.path;
      await selectProject(newProj);
    } catch (err) {
      showToast("Failed to create project: " + err, "error");
    }
  });
}

function renderCardDetail(card: Card) {
  const todoAddHadFocus = (document.activeElement && document.activeElement.id === "todo-item-add-input-el");
  const assigneeHadFocus = (document.activeElement && document.activeElement.id === "card-detail-assignee-el");
  
  viewerContainer.innerHTML = "";
  
  const detailDiv = document.createElement("div");
  detailDiv.className = "card-detail-view";
  
  // Collect history of unique assignees
  const uniqueAssignees = new Set<string>();
  for (const c of cardsList) {
    if (c.assignee) {
      const names = c.assignee.split(",").map(name => name.trim()).filter(Boolean);
      for (const name of names) {
        uniqueAssignees.add(name);
      }
    }
  }
  
  // Title rendering
  let titleHtml = "";
  if (isEditingViewer) {
    titleHtml = `<input type="text" id="viewer-card-title-input" class="card-detail-title-input" value="${escapeHtml(card.title)}">`;
  } else {
    titleHtml = `<h2 class="card-detail-title">${formatPlainMarkdown(card.title)}</h2>`;
  }

  // Description rendering
  let descHtml = "";
  if (isEditingViewer) {
    descHtml = `<textarea id="viewer-card-desc-textarea" class="card-detail-desc-textarea" placeholder="Enter card description...">${escapeHtml(card.description)}</textarea>`;
  } else {
    descHtml = `<div class="card-detail-desc markdown-body">${parseMarkdown(card.description || "*No description provided.*")}</div>`;
  }

  // Options for status select
  const statusOptions = [
    { key: "backlog", label: "Backlog" },
    { key: "todo", label: "Todo" },
    { key: "running", label: "In Progress" },
    { key: "blocked", label: "Blocked" },
    { key: "review", label: "Review" },
    { key: "done", label: "Done" },
    { key: "failed", label: "Failed" }
  ];

  const statusSelectHtml = `
    <select class="card-detail-status-select" id="card-detail-status-select-el">
      ${statusOptions.map(opt => `<option value="${opt.key}" ${card.status === opt.key ? 'selected' : ''}>${opt.label}</option>`).join("")}
    </select>
  `;

  // Checklist items rendering
  const checklistHtml = `
    <div class="card-detail-checklist">
      <h3 class="todo-list-title">📋 Checklist</h3>
      <div class="todo-items-list">
        ${(card.todo_list || []).map((item, idx) => `
          <div class="todo-item" data-idx="${idx}">
            <input type="checkbox" class="todo-item-checkbox" ${item.completed ? 'checked' : ''}>
            <span class="todo-item-text ${item.completed ? 'completed' : ''}">${escapeHtml(item.text)}</span>
            <button class="todo-item-delete-btn" title="Delete task">🗑️</button>
          </div>
        `).join("")}
      </div>
      <div class="todo-item-add-container">
        <input type="text" class="todo-item-add-input" placeholder="+ Add a task..." id="todo-item-add-input-el">
      </div>
    </div>
  `;

  detailDiv.innerHTML = `
    ${titleHtml}
    ${descHtml}
    
    <div class="card-detail-meta">
      <div class="card-detail-meta-item">
        <span class="card-detail-meta-label">Assigned To</span>
        <div class="assignee-chips-container" id="assignee-chips-container-el">
          ${(card.assignee || "").split(",").map(name => name.trim()).filter(Boolean).map(name => `
            <div class="assignee-chip" data-name="${escapeHtml(name)}">
              <span>${escapeHtml(name)}</span>
              <button type="button" class="remove-chip-btn">&times;</button>
            </div>
          `).join("")}
          <input type="text" class="assignee-chip-input" id="card-detail-assignee-el" placeholder="${(card.assignee || "").trim() ? "" : "Add assignee..."}" list="assignee-history-list">
        </div>
        <datalist id="assignee-history-list">
          ${Array.from(uniqueAssignees).map(name => `<option value="${escapeHtml(name)}">`).join("")}
        </datalist>
      </div>
      <div class="card-detail-meta-item">
        <span class="card-detail-meta-label">Status</span>
        ${statusSelectHtml}
      </div>
    </div>

    ${checklistHtml}

    <div class="card-detail-actions">
      <button class="btn btn-danger card-detail-delete-btn" id="card-detail-delete-btn-el">Delete Card</button>
    </div>
  `;

  // Event listener: Checklist checkbox change
  detailDiv.querySelectorAll(".todo-item-checkbox").forEach(chk => {
    chk.addEventListener("change", async (e) => {
      const idxStr = (chk.closest(".todo-item") as HTMLElement).dataset.idx;
      if (idxStr !== undefined) {
        const idx = parseInt(idxStr, 10);
        card.todo_list[idx].completed = (e.target as HTMLInputElement).checked;
        await saveCardObject(card);
      }
    });
  });

  // Event listener: Checklist item delete
  detailDiv.querySelectorAll(".todo-item-delete-btn").forEach(btn => {
    btn.addEventListener("click", async () => {
      const idxStr = (btn.closest(".todo-item") as HTMLElement).dataset.idx;
      if (idxStr !== undefined) {
        const idx = parseInt(idxStr, 10);
        card.todo_list.splice(idx, 1);
        await saveCardObject(card);
      }
    });
  });

  // Event listener: Checklist add item (Enter key)
  const addInput = detailDiv.querySelector("#todo-item-add-input-el") as HTMLInputElement;
  if (addInput) {
    addInput.addEventListener("keydown", async (e) => {
      if (e.key === "Enter") {
        const text = addInput.value.trim();
        if (text) {
          if (!card.todo_list) card.todo_list = [];
          card.todo_list.push({ text, completed: false });
          await saveCardObject(card);
        }
      }
    });
  }

  // Event listener: Assignee chip remove clicks
  detailDiv.querySelectorAll(".remove-chip-btn").forEach(btn => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      const chip = btn.closest(".assignee-chip") as HTMLElement;
      const nameToRemove = chip.dataset.name;
      if (nameToRemove) {
        const existing = (card.assignee || "").split(",").map(name => name.trim()).filter(Boolean);
        const updated = existing.filter(name => name !== nameToRemove);
        card.assignee = updated.length > 0 ? updated.join(", ") : null;
        await saveCardObject(card);
      }
    });
  });

  // Event listener: Assignee input change / keydown / blur
  const assigneeInput = detailDiv.querySelector("#card-detail-assignee-el") as HTMLInputElement;
  if (assigneeInput) {
    const handleAssigneeAdd = async () => {
      const val = assigneeInput.value.replace(/,$/, "").trim();
      if (val) {
        const existing = (card.assignee || "").split(",").map(name => name.trim()).filter(Boolean);
        if (!existing.includes(val)) {
          existing.push(val);
          card.assignee = existing.join(", ");
          assigneeInput.value = "";
          await saveCardObject(card);
        } else {
          assigneeInput.value = "";
        }
      }
    };

    assigneeInput.addEventListener("blur", handleAssigneeAdd);
    assigneeInput.addEventListener("change", handleAssigneeAdd);
    assigneeInput.addEventListener("keydown", async (e) => {
      if (e.key === "," || e.key === "Tab" || e.key === "Enter") {
        e.preventDefault();
        await handleAssigneeAdd();
      } else if (e.key === "Backspace" && assigneeInput.value === "") {
        const existing = (card.assignee || "").split(",").map(name => name.trim()).filter(Boolean);
        if (existing.length > 0) {
          existing.pop();
          card.assignee = existing.length > 0 ? existing.join(", ") : null;
          await saveCardObject(card);
        }
      }
    });
  }

  // Event listener: Status dropdown change
  const statusSelect = detailDiv.querySelector("#card-detail-status-select-el") as HTMLSelectElement;
  if (statusSelect) {
    statusSelect.addEventListener("change", async () => {
      card.status = statusSelect.value;
      await saveCardObject(card);
    });
  }

  // Event listener: Delete Card
  const deleteBtn = detailDiv.querySelector("#card-detail-delete-btn-el") as HTMLButtonElement;
  if (deleteBtn) {
    deleteBtn.addEventListener("click", async () => {
      if (await showConfirm(`Delete card "${card.title}"?`, "Delete Card")) {
        try {
          if (isTauri) {
            await invoke("delete_card", { cardId: card.id });
          } else {
            await mockInvoke("delete_card", { cardId: card.id });
          }
          // Remove from local list
          cardsList = cardsList.filter(c => c.id !== card.id);
          if (activeCard && activeCard.id === card.id) {
            activeCard = null;
          }
          popView();
          await refreshState();
        } catch (err) {
          showToast("Failed to delete card: " + err, "error");
        }
      }
    });
  }

  viewerContainer.appendChild(detailDiv);

  if (todoAddHadFocus) {
    const newAddInput = detailDiv.querySelector("#todo-item-add-input-el") as HTMLInputElement;
    if (newAddInput) {
      newAddInput.focus();
    }
  }
  if (assigneeHadFocus) {
    const newAssigneeInput = detailDiv.querySelector("#card-detail-assignee-el") as HTMLInputElement;
    if (newAssigneeInput) {
      newAssigneeInput.focus();
      newAssigneeInput.selectionStart = newAssigneeInput.selectionEnd = newAssigneeInput.value.length;
    }
  }
}

function syncCardEditInputs(card: Card) {
  if (isEditingViewer) {
    const titleInput = document.getElementById("viewer-card-title-input") as HTMLInputElement;
    const descTextarea = document.getElementById("viewer-card-desc-textarea") as HTMLTextAreaElement;
    if (titleInput) {
      card.title = titleInput.value;
    }
    if (descTextarea) {
      card.description = descTextarea.value;
    }
  }
}

async function saveCardObject(card: Card) {
  syncCardEditInputs(card);
  try {
    if (isTauri) {
      await invoke("save_card", { card });
    } else {
      await mockInvoke("save_card", { card });
    }
    // Update active card if it's the current one
    if (activeCard && activeCard.id === card.id) {
      activeCard = card;
      await updateActiveCardUI();
    }
    // Refresh states and boards
    await refreshState();
  } catch (err) {
    console.error("Failed to save card:", err);
  }
}
