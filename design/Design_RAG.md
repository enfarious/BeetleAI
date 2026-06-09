# Design: RAG Implementation (Draft)

## 1. Objective
To provide the Agent loop with high-fidelity, relevant context from the codebase, design documents, and project history, allowing it to make informed decisions without exceeding token limits.

## 2. Components

### 2.1 Ingestion Engine
- **File Watcher**: Monitors the workspace for changes (using `notify` crate in Rust).
- **Chunking Strategy**:
    - **Code**: Semantic chunking (functions, classes, modules) to preserve logical structure.
    - **Docs**: Markdown header-based chunking.
- **Embedding Generation**: Asynchronous generation of embeddings for each chunk.

### 2.2 Vector Storage
- **Local-first**: Use **LanceDB** (embedded, fast, handles large datasets) to ensure privacy and speed.
- **Schema**:
    - `content`: The raw text chunk.
    - `metadata`: `{ file_path, line_start, line_end, type (code/doc/git), hash }`.
    - `embedding`: Vector representation.

### 2.3 Retrieval Mechanism
- **Hybrid Search**: Combine vector similarity search with keyword/BM25 search (to handle specific function names/identifiers).
- **Re-ranking**: Use a lightweight cross-encoder to re-rank top results for maximum relevance before injection.

### 2.4 Context Injection (The "Context Window" Manager)
- **Dynamic Prompt Construction**:
    - System Prompt (Design Doc)
    - Retrieved Code Context (Relevant snippets)
    - Retrieved Doc Context (Relevant design sections)
    - Active File (The file currently being edited)
- **Token Budgeting**: Prioritize "Design Doc" and "Active File" context over RAG results.

## 3. Integration with Agent Loop
- The `run_engine` will query the RAG service during the "thought" phase or when the model asks for "more context".
- RAG results are injected as `context` blocks in the conversation history.

## 4. Roadmap
- [ ] Implement basic file indexing.
- [ ] Integrate LanceDB.
- [ ] Implement semantic chunking for Rust/TS.
- [ ] Implement hybrid search.