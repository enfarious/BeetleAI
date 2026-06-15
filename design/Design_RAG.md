# Design: RAG Implementation

> **Status (implemented).** A first version ships. It favors zero new
> dependencies and the existing SQLite/`ureq` stack over the heavier tooling
> sketched below — so several specifics here describe the *aspiration*, not the
> current build. What's live today:
>
> - **Configurable embedding provider** in Settings → *Embeddings & RAG*
>   (Ollama / OpenAI / Voyage / LM Studio / custom) — `call_embedding()`.
> - **Codebase index**: `.gitignore`-aware walk → line-window chunking
>   (markdown split on headers) → embeddings stored as little-endian f32 BLOBs in
>   SQLite (`rag_chunks`). Incremental via per-file content hashes (`rag_files`).
> - **Retrieval**: brute-force cosine (`search_codebase` tool). Per-project
>   reindex + a staleness indicator live under the project selector.
> - **Context injection**: top task-matches auto-injected once per run (toggle:
>   `embedding_auto_inject`), alongside the on-demand tool.
> - **Semantic memory**: `recall()` ranks by embedding similarity when configured,
>   keyword `LIKE` otherwise.
>
> Divergences from the draft below: SQLite f32 BLOBs instead of **LanceDB**;
> line-window instead of AST/semantic chunking; `DefaultHasher` instead of
> SHA-256; reindex-at-run-start instead of a live `notify` watcher. Hybrid/BM25
> search and cross-encoder re-ranking remain unbuilt (see §4).

## 1. Objective
To provide the Agent loop with high-fidelity, relevant context from the codebase, design documents, and project history, allowing it to make informed decisions without exceeding token limits.

## 2. Components

### 2.1 Ingestion Engine
- **File Watcher**: Monitors the workspace for changes (using `notify` crate in Rust).
- **Chunking Strategy**:
    - **Code**: Semantic chunking (functions, classes, modules) to preserve logical structure.
    - **Docs**: Markdown header-based chunking.
- **Embedding Generation**: Asynchronous generation of embeddings for each chunk.

### 2.2 Vector Storage (LanceDB)
- **Local-first**: Use **LanceDB** (embedded, fast, handles large datasets) to ensure privacy and speed.
- **Detailed Metadata Schema**:

| Field | Type | Purpose |
| :--- | :--- | :--- |
| `content` | `String` | The actual text chunk (code snippet or markdown text). |
| `embedding` | `Vector` | High-dimensional vector (e.g., 768 or 1536 dims). |
| `file_path` | `String` | Absolute or relative path to the source file. |
| `file_extension`| `String` | `rs`, `ts`, `md`, `json`, etc. (used to select parsers). |
| `chunk_type` | `Enum` | `code_block`, `doc_section`, `git_diff`, `file_header`. |
| `line_range` | `Interval` | `[start_line, end_line]` for jumping directly to code. |
| `semantic_tag` | `String` | The "Parent" identifier (e.g., `mod::struct::method`). |
| `symbol_info` | `JSON` | `{ "name": "my_func", "type": "function", "args": [...] }`. |
| `content_hash` | `String` | SHA-256 of the chunk to detect if a re-index is needed. |
| `token_count` | `Int` | To assist the "Token Budgeting" logic during retrieval. |

### 2.3 Retrieval Mechanism
- **Hybrid Search**: Combine vector similarity search with keyword/BM25 search (to handle specific function names/identifiers).
- **Re-ranking**: Use a lightweight cross-encoder to re-rank top results for maximum relevance before injection.

### 2.4 Context Injection (The "Context Window" Manager)
- **Dynamic Prompt Construction**:
    - System Prompt (Design Doc)
    - Retrieved Code Context (Relevant snippets)
    - Retrieved Doc Context (Relevant design sections)
    - Active File (The file currently being edited)
    - Token Budgeting: Prioritize "Design Doc" and "Active File" context over RAG results.

## 3. Integration with Agent Loop
- The `run_engine` will query the RAG service during the "thought" phase or when the model asks for "more context".
- RAG results are injected as `context` blocks in the conversation history.

## 4. Roadmap
- [x] Basic file indexing (incremental, content-hash based).
- [x] Vector storage — SQLite f32 BLOBs + brute-force cosine (LanceDB deferred).
- [x] Configurable embedding provider in Settings.
- [x] `search_codebase` retrieval tool + semantic `recall()`.
- [x] Per-project reindex + staleness indicator.
- [x] Auto-inject top task-matches into agent context (toggle).

### Deferred
- [ ] Semantic (AST/tree-sitter) chunking for Rust/TS, replacing line windows.
- [ ] Hybrid search: blend vector similarity with BM25/keyword for exact identifiers.
- [ ] Cross-encoder re-ranking of top results.
- [ ] Live file watching (`notify`) instead of reindex-at-run-start.
- [ ] ANN index (LanceDB / `sqlite-vec`) if linear scan gets too slow at scale.
- [ ] Richer chunk metadata (semantic_tag, symbol_info, token_count per §2.2).