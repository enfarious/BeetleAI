import { invoke } from "@tauri-apps/api/core";

// Interfaces from Rust commands contract
interface Project {
  id: string;
  name: string;
  path: string;
}

interface Card {
  id: string;
  title: string;
  description: string;
  status: string; // "backlog", "todo", "running", "blocked", "review", "done", "failed"
  run_id: string | null;
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
  | { kind: "doc" }
  | { kind: "kanban" }
  | { kind: "file"; path: string; name: string }
  | { kind: "diff"; runId: string };

// App State
let currentProject: Project | null = null;
let cardsList: Card[] = [];
let activeCard: Card | null = null;
let viewerStack: ViewerState[] = [{ kind: "doc" }];
let currentMode: "plan" | "kanban" | "code" = "plan";

// DOM References
const projectSelect = document.getElementById("project-select") as HTMLSelectElement;
const fileTreeContainer = document.getElementById("file-tree") as HTMLDivElement;
const activeCardTitle = document.getElementById("active-card-title") as HTMLHeadingElement;
const runStatusBadge = document.getElementById("run-status") as HTMLDivElement;
const runStatusText = document.getElementById("run-status-text") as HTMLSpanElement;
const chatMessages = document.getElementById("chat-messages") as HTMLDivElement;
const chatInput = document.getElementById("chat-input") as HTMLInputElement;
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

// Mode tabs
const tabPlan = document.getElementById("tab-plan") as HTMLButtonElement;
const tabKanban = document.getElementById("tab-kanban") as HTMLButtonElement;
const tabCode = document.getElementById("tab-code") as HTMLButtonElement;

// Initialize App
window.addEventListener("DOMContentLoaded", async () => {
  setupEventListeners();
  await loadProjects();
});

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
    }
  });

  // Chat input listener
  btnSendChat.addEventListener("click", () => submitChat());
  chatInput.addEventListener("keypress", (e) => {
    if (e.key === "Enter") {
      submitChat();
    }
  });

  // Viewer back button
  btnViewerBack.addEventListener("click", () => popView());
}

// Switch navigation mode
function switchMode(mode: "plan" | "kanban" | "code") {
  currentMode = mode;
  tabPlan.classList.toggle("active", mode === "plan");
  tabKanban.classList.toggle("active", mode === "kanban");
  tabCode.classList.toggle("active", mode === "code");

  if (mode === "plan") {
    pushView({ kind: "doc" });
  } else if (mode === "kanban") {
    pushView({ kind: "kanban" });
  } else if (mode === "code") {
    if (activeCard && activeCard.run_id) {
      pushView({ kind: "diff", runId: activeCard.run_id });
    } else {
      // Find files if possible
      pushView({ kind: "doc" });
    }
  }
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
    }
  } catch (err) {
    console.error("Failed to load projects:", err);
  }
}

// Open active project
async function selectProject(project: Project) {
  currentProject = project;
  await refreshState();
  
  // Load initial root directory tree
  if (currentProject) {
    await renderFileTreeRoot(currentProject.path);
  }

  // Switch to Plan mode (Design doc view)
  switchMode("plan");
}

// Reload cards, controls and chat transcript
async function refreshState() {
  if (!currentProject) return;
  try {
    cardsList = await invoke<Card[]>("list_cards");
    
    // Refresh selected card if it is in progress
    if (activeCard) {
      const updated = cardsList.find((c) => c.id === activeCard!.id);
      if (updated) {
        activeCard = updated;
        updateActiveCardUI();
      }
    }
    
    // Render right-side panel
    renderRightPanel();
  } catch (err) {
    console.error("Error refreshing state:", err);
  }
}

// Set active card detail view
async function selectCard(card: Card) {
  activeCard = card;
  updateActiveCardUI();
  
  if (card.run_id) {
    pushView({ kind: "diff", runId: card.run_id });
  } else {
    pushView({ kind: "kanban" });
  }
}

// Render center column active card and run controls
async function updateActiveCardUI() {
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

  // Toggle Controls based on state machine
  controlsRun.style.display = "none";
  controlsActive.style.display = "none";
  controlsReview.style.display = "none";
  
  chatInput.disabled = true;
  btnSendChat.disabled = true;
  chatInput.placeholder = "Agent is offline...";

  if (activeCard.status === "backlog" || activeCard.status === "todo" || activeCard.status === "failed") {
    controlsRun.style.display = "block";
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
  }

  // Fetch and display transcript logs
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
}

// Render run execution transcript logs
function renderLogs(logs: RunEvent[]) {
  chatMessages.innerHTML = "";
  if (logs.length === 0) {
    chatMessages.innerHTML = `<p class="empty-state">No execution logs yet.</p>`;
    return;
  }

  logs.forEach((log) => {
    if (log.event_type === "status") {
      const div = document.createElement("div");
      div.className = "bubble-meta";
      div.style.alignSelf = "center";
      div.style.margin = "8px 0";
      div.textContent = `⚙ State transitioned: ${log.payload.toUpperCase()}`;
      chatMessages.appendChild(div);
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
    }

    if (log.event_type === "tool_call" || log.event_type === "tool_result") {
      try {
        const details = JSON.parse(log.payload);
        const wrapper = document.createElement("div");
        wrapper.className = "chat-bubble tool";
        
        const isCall = log.event_type === "tool_call";
        wrapper.innerHTML = `
          <div class="tool-header">
            <span>${isCall ? '🛠 Executing Tool' : '⚙ Tool Output'}</span>
            <code style="color: var(--accent-primary);">${details.tool}</code>
          </div>
          <pre class="tool-output"><code>${isCall ? JSON.stringify(details.args, null, 2) : details.result}</code></pre>
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

// Submit chat message to agent loop
async function submitChat() {
  const text = chatInput.value.trim();
  if (!text || !activeCard || !activeCard.run_id) return;
  chatInput.value = "";

  try {
    if (activeCard.status === "blocked") {
      // Call unblock_run
      await invoke("unblock_run", { runId: activeCard.run_id, reply: text });
      activeCard.status = "running";
    } else {
      // Mid-run interjection
      await invoke("send_chat", { runId: activeCard.run_id, message: text });
    }
    await refreshState();
  } catch (err) {
    console.error(err);
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
  node.innerHTML = `
    <span class="icon">${entry.is_dir ? '📁' : '📄'}</span>
    <span>${entry.name}</span>
  `;
  
  wrapper.appendChild(node);

  if (entry.is_dir) {
    const childrenContainer = document.createElement("div");
    childrenContainer.className = "tree-children";
    childrenContainer.style.display = "none";
    wrapper.appendChild(childrenContainer);

    let loaded = false;
    node.addEventListener("click", async (e) => {
      e.stopPropagation();
      const isExpanded = childrenContainer.style.display !== "none";
      childrenContainer.style.display = isExpanded ? "none" : "block";
      node.querySelector(".icon")!.textContent = isExpanded ? '📁' : '📂';

      if (!loaded && !isExpanded) {
        try {
          const children = await invoke<DirEntry[]>("list_dir", { path: entry.path });
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
    node.addEventListener("click", (e) => {
      e.stopPropagation();
      document.querySelectorAll(".tree-node.file").forEach((n) => n.classList.remove("active"));
      node.classList.add("active");
      pushView({ kind: "file", path: entry.path, name: entry.name });
    });
  }

  return wrapper;
}

// Viewer Stack navigation helpers
function pushView(state: ViewerState) {
  // If the view type is doc or kanban, don't duplicate on top of stack
  if (state.kind === "doc" || state.kind === "kanban") {
    viewerStack = [state];
  } else {
    viewerStack.push(state);
  }
  renderRightPanel();
}

function popView() {
  if (viewerStack.length > 1) {
    viewerStack.pop();
    renderRightPanel();
  }
}

// Render Polymorphic Viewer based on top view in stack
async function renderRightPanel() {
  const currentView = viewerStack[viewerStack.length - 1];
  
  // Show back button if we are nested
  btnViewerBack.style.display = viewerStack.length > 1 ? "inline-block" : "none";

  if (!currentView) {
    viewerTitle.textContent = "Empty";
    viewerContainer.innerHTML = `<div class="empty-state">No view active</div>`;
    return;
  }

  switch (currentView.kind) {
    case "doc":
      viewerTitle.textContent = "Design Doc";
      viewerContainer.innerHTML = `<div class="empty-state">Loading design document...</div>`;
      try {
        const text = await invoke<string>("read_design_doc");
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
        viewerContainer.innerHTML = `
          <div class="code-viewer-container">
            <pre class="code-viewer"><code>${escapeHtml(code)}</code></pre>
          </div>
        `;
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
    `;

    const listDiv = colDiv.querySelector(".kanban-cards-list") as HTMLDivElement;

    filtered.forEach((card) => {
      const cardDiv = document.createElement("div");
      cardDiv.className = `kanban-card ${activeCard?.id === card.id ? 'selected' : ''}`;
      cardDiv.innerHTML = `
        <div class="kanban-card-title">${card.title}</div>
        <div class="kanban-card-description">${card.description}</div>
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

    let leftLine = 0;
    let rightLine = 0;

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

// Markdown parser utilities
function parseMarkdown(md: string): string {
  let html = md;
  // Escapes html tag markers
  html = escapeHtml(html);

  // Headers
  html = html.replace(/^# (.*?)$/gm, "<h1>$1</h1>");
  html = html.replace(/^## (.*?)$/gm, "<h2>$1</h2>");
  html = html.replace(/^### (.*?)$/gm, "<h3>$1</h3>");

  // Blockquotes/Alerts
  html = html.replace(/^> \[\!IMPORTANT\](.*?)$/gm, '<div style="border-left: 4px solid var(--accent-primary); background-color: var(--bg-secondary); padding: 10px; margin: 12px 0; border-radius: 4px;"><strong>IMPORTANT</strong>');
  html = html.replace(/^> (.*?)$/gm, "<blockquote>$1</blockquote>");

  // Bold
  html = html.replace(/\*\*(.*?)\*\*/g, "<strong>$1</strong>");

  // Inline Code
  html = html.replace(/`(.*?)`/g, "<code>$1</code>");

  // Code blocks
  html = html.replace(/```(.*?)\n([\s\S]*?)```/gm, "<pre><code>$2</code></pre>");

  // Lists
  html = html.replace(/^\- (.*?)$/gm, "<li>$1</li>");
  
  // Wrap list items in ul
  html = html.replace(/(<li>.*?<\/li>)+/gs, "<ul>$&</ul>");

  // Paragraphs
  html = html.replace(/^(?!(?:<h|<ul|<li|<blockquote|<pre|<\/pre|<\/ul|<\/li|<\/blockquote))(.*?)$/gm, "<p>$1</p>");
  
  // Clean empty paragraphs
  html = html.replace(/<p><\/p>/g, "");

  return html;
}

// Inline Markdown formatter inside chat bubbles
function formatMarkdownInChat(text: string): string {
  let formatted = escapeHtml(text);
  // Bold
  formatted = formatted.replace(/\*\*(.*?)\*\*/g, "<strong>$1</strong>");
  // Code block
  formatted = formatted.replace(/`([^`]+)`/g, "<code>$1</code>");
  // Linebreaks
  formatted = formatted.replace(/\n/g, "<br>");
  return formatted;
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
