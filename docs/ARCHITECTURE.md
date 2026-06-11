# K2SO Architecture

Technical architecture of K2SO, covering the system design, backend modules, frontend structure, IPC layer, database schema, and the AI assistant pipeline.

## System Overview

```
┌──────────────────────────────────────────────────────────────────┐
│                         macOS Window                            │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │                    React 19 Frontend                       │  │
│  │                                                            │  │
│  │  ┌──────────┐  ┌──────────┐  ┌───────────┐  ┌──────────┐  │  │
│  │  │ Sidebar  │  │ TabBar   │  │ PaneLayout│  │ Assistant│  │  │
│  │  │ Projects │  │ Workspace│  │ Terminals │  │ LLM Chat │  │  │
│  │  │ FocusGrp │  │ Tabs     │  │ Viewers   │  │ Cmd+L    │  │  │
│  │  └──────────┘  └──────────┘  └───────────┘  └──────────┘  │  │
│  │                                                            │  │
│  │  Zustand Stores ──── xterm.js (WebGL) ──── react-mosaic   │  │
│  └────────────────────────────┬───────────────────────────────┘  │
│                               │ Tauri IPC                        │
│                               │ (invoke + events)                │
│  ┌────────────────────────────┴───────────────────────────────┐  │
│  │                      Rust Backend                          │  │
│  │                                                            │  │
│  │  ┌────────────┐  ┌──────────┐  ┌───────┐  ┌────────────┐  │  │
│  │  │ Terminal   │  │ Database │  │ Git   │  │ LLM        │  │  │
│  │  │ portable-  │  │ rusqlite │  │ git2  │  │ llama-cpp-2│  │  │
│  │  │ pty        │  │ SQLite   │  │       │  │ Metal GPU  │  │  │
│  │  └────────────┘  └──────────┘  └───────┘  └────────────┘  │  │
│  │                                                            │  │
│  │  ┌────────────┐  ┌──────────┐  ┌──────────────────────┐   │  │
│  │  │ Filesystem │  │ Settings │  │ Workspace Ops        │   │  │
│  │  │ read/write │  │ KV store │  │ split/close/arrange  │   │  │
│  │  └────────────┘  └──────────┘  └──────────────────────┘   │  │
│  └────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────┘
```

## Backend Modules

All Rust source lives under `src-tauri/src/`.

### `terminal/` -- PTY Management

- **TerminalManager** holds a `HashMap<String, TerminalInstance>` where each instance contains the PTY master, child process, resize handle, and a 256KB scrollback ring buffer.
- `terminal_create` spawns a shell via `portable-pty`, starts a read loop on a background thread, and streams output to the frontend via Tauri events (`terminal:data:{id}`). Accepts an optional `id` parameter for stable terminal IDs across reattach.
- `terminal_write` writes bytes into the PTY master.
- `terminal_resize` sends a resize signal to the PTY.
- `terminal_kill` terminates the child process (SIGHUP then SIGKILL) and removes the entry.
- `terminal_exists` checks if a PTY is alive (used by frontend to decide create vs reattach).
- `terminal_get_buffer` returns the scrollback buffer for replay on reattach.
- **Persistence**: PTYs survive tab switches. When a TerminalView unmounts (tab switch), the PTY keeps running and buffering output. On remount, the frontend replays the buffer and resubscribes to events.
- SIGPIPE is ignored at startup to prevent writes to dead PTYs from crashing the app.
- All PTYs are killed on window close to prevent zombie processes.

### `llm/` -- Local LLM Inference

- **LlmManager** wraps `llama-cpp-2` for GGUF model loading and inference.
- Models are loaded with full GPU layer offloading (Metal on macOS).
- The system prompt (in `tools.rs`) defines 7 workspace tools the LLM can call: `split_panes`, `open_document`, `open_terminal`, `close_pane`, `arrange_layout`, `list_files`, `switch_workspace`.
- Responses are parsed as JSON tool calls or plain messages.
- `download.rs` handles downloading a default model from HuggingFace with progress events.

### `db/` -- SQLite Database

- Database lives at `~/.k2so/k2so.db`.
- Uses WAL mode and foreign key enforcement.
- Migrations are embedded in the binary (via `include_str!`) and tracked in a `_migrations` table.
- `schema.rs` defines typed structs and CRUD methods for all 7 tables.

### `commands/` -- Tauri IPC Handlers

All `#[tauri::command]` functions grouped by domain:

| Module | Commands |
|--------|----------|
| `projects.rs` | CRUD, icon detection, editor launch, folder picker, git init |
| `workspaces.rs` | List, create, delete workspaces |
| `focus_groups.rs` | CRUD, project assignment, reconciliation |
| `workspace_sections.rs` | CRUD, reorder, workspace assignment |
| `agents.rs` | Preset CRUD, reorder, reset built-ins |
| `terminal.rs` | PTY lifecycle (create, write, resize, kill) |
| `git.rs` | Repo info, branches, worktrees, changes |
| `filesystem.rs` | Read dir, read/write files, open in Finder |
| `workspace_ops.rs` | Split pane, close pane, open doc/terminal, arrange |
| `assistant.rs` | Chat, model status, load model, download |
| `settings.rs` | Get/update/reset key-value settings |
| `project_config.rs` | Per-project config, run commands |

### `git/` -- Git Operations

- Uses `git2` crate for repository inspection.
- Provides branch listing, worktree management (create, remove, reopen), and change detection.
- Worktree paths are stored in the `workspaces` table.

### Other Backend Files

- `state.rs` -- `AppState` struct holding `Mutex<Connection>`, `Mutex<TerminalManager>`, `Mutex<LlmManager>`.
- `editors.rs` -- Detects installed editors (VS Code, Cursor, Zed, etc.) for "Open in Editor" functionality.
- `menu.rs` -- Native macOS menu bar construction and event handling.
- `window.rs` -- Saves/restores window position, size, and maximized state to `~/.k2so/window-state.json`.
- `project_config.rs` -- Reads `.k2so.toml` per-project config files.

## Frontend

All TypeScript/React source lives under `src/renderer/`.

### Stores (Zustand)

State is managed through Zustand stores, each responsible for one domain:

| Store | Responsibility |
|-------|----------------|
| `projects.ts` | Project list, selection, CRUD operations |
| `tabs.ts` | Workspace tabs, pane tree, active tab tracking |
| `panels.ts` | Panel layout state (sidebar, file tree, etc.) |
| `presets.ts` | Agent preset list and management |
| `filetree.ts` | File tree expansion state, directory contents |
| `focus-groups.ts` | Focus group tabs and project assignment |
| `assistant.ts` | LLM chat messages, model status |
| `settings.ts` | App settings (theme, font, etc.) |
| `sidebar.ts` | Sidebar visibility and active section |
| `command-palette.ts` | Command palette visibility and filtering |
| `context-menu.ts` | Right-click context menu state |
| `terminal-settings.ts` | Terminal font, size, theme |
| `toast.ts` | Toast notification queue |
| `git-init-dialog.ts` | Git initialization dialog state |

### Key Components

- **Terminal** -- Wraps xterm.js with the WebGL renderer and fit addon. Communicates with the Rust PTY via Tauri events.
- **PaneLayout** -- Uses `react-mosaic-component` for tiled pane management. Each leaf is a terminal or document viewer.
- **FileViewerPane** -- Dispatches on file extension to render Markdown (react-markdown + remark-gfm), PDF (pdfjs-dist), or DOCX (mammoth).
- **WorkspaceAssistant** -- Chat interface that sends user messages to the local LLM and executes returned tool calls.
- **PresetsBar** -- Row of buttons for launching agent CLI tools. Each button opens a new terminal with the preset's command.
- **Sidebar** -- Project list with focus group tabs, drag-to-reorder, context menus.
- **FocusWindow** -- Detachable window for a focused view of a single project.
- **CommandPalette** -- Cmd+K overlay for quick actions.

### Layout System

The pane layout is a binary tree (from react-mosaic) where:
- **Branch nodes** have a `direction` (row/column) and `splitPercentage`.
- **Leaf nodes** are pane IDs mapped to either a terminal or document viewer.

Layout state is stored in the `tabs` Zustand store and persisted to the database per-workspace so layouts restore when switching workspaces.

## IPC: Tauri Commands + Events

Communication between frontend and backend uses two mechanisms:

### Commands (Frontend -> Backend)

Frontend calls `invoke("command_name", { args })` which maps to a `#[tauri::command]` Rust function. Returns a `Result<T, String>`.

```
Frontend                    Backend
invoke("terminal_create")  →  terminal::terminal_create()
                           ←  Ok(terminal_id)
```

### Events (Backend -> Frontend)

Backend emits events via `app.emit()` for streaming data:

- `terminal:data:{id}` -- PTY output bytes
- `workspace:split-pane` -- Layout manipulation from assistant
- `workspace:open-document` -- Open a file from assistant
- `workspace:open-terminal` -- Open a terminal from assistant
- `assistant:download-progress` -- Model download progress

Frontend listens with `listen("event_name", callback)` from `@tauri-apps/api/event`.

## Database Schema

SQLite database at `~/.k2so/k2so.db` with 7 tables (plus `_migrations`):

```sql
focus_groups
├── id          TEXT PRIMARY KEY
├── name        TEXT NOT NULL
├── color       TEXT
├── tab_order   INTEGER NOT NULL DEFAULT 0
└── created_at  INTEGER NOT NULL DEFAULT (unixepoch())

projects
├── id              TEXT PRIMARY KEY
├── name            TEXT NOT NULL
├── path            TEXT NOT NULL
├── color           TEXT NOT NULL DEFAULT '#6366f1'
├── tab_order       INTEGER NOT NULL DEFAULT 0
├── last_opened_at  INTEGER
├── worktree_mode   INTEGER NOT NULL DEFAULT 0
├── icon_url        TEXT
├── focus_group_id  TEXT → focus_groups(id)
└── created_at      INTEGER NOT NULL DEFAULT (unixepoch())

workspace_sections
├── id           TEXT PRIMARY KEY
├── project_id   TEXT NOT NULL → projects(id)
├── name         TEXT NOT NULL
├── color        TEXT
├── is_collapsed INTEGER NOT NULL DEFAULT 0
├── tab_order    INTEGER NOT NULL DEFAULT 0
└── created_at   INTEGER NOT NULL DEFAULT (unixepoch())

workspaces
├── id             TEXT PRIMARY KEY
├── project_id     TEXT NOT NULL → projects(id)
├── section_id     TEXT → workspace_sections(id)
├── type           TEXT NOT NULL DEFAULT 'default'
├── branch         TEXT
├── name           TEXT NOT NULL
├── tab_order      INTEGER NOT NULL DEFAULT 0
├── worktree_path  TEXT
└── created_at     INTEGER NOT NULL DEFAULT (unixepoch())

agent_presets
├── id          TEXT PRIMARY KEY
├── label       TEXT NOT NULL
├── command     TEXT NOT NULL
├── icon        TEXT
├── enabled     INTEGER NOT NULL DEFAULT 1
├── sort_order  INTEGER NOT NULL DEFAULT 0
├── is_built_in INTEGER NOT NULL DEFAULT 0
└── created_at  INTEGER NOT NULL DEFAULT (unixepoch())

terminal_tabs
├── id            TEXT PRIMARY KEY
├── workspace_id  TEXT NOT NULL → workspaces(id)
├── title         TEXT NOT NULL
├── tab_order     INTEGER NOT NULL DEFAULT 0
└── created_at    INTEGER NOT NULL DEFAULT (unixepoch())

terminal_panes
├── id              TEXT PRIMARY KEY
├── tab_id          TEXT NOT NULL → terminal_tabs(id)
├── split_direction TEXT
├── split_ratio     REAL
├── pane_order      INTEGER NOT NULL DEFAULT 0
└── created_at      INTEGER NOT NULL DEFAULT (unixepoch())
```

## AI Assistant Pipeline

The workspace assistant (Cmd+L) uses a local LLM for natural language workspace control:

```
User Input
    │
    ▼
┌──────────────────┐
│  Frontend        │
│  assistant store  │  Sends user message via invoke("assistant_chat")
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  Rust Backend    │
│  assistant.rs    │  Passes message to LlmManager
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  LlmManager     │
│  llm/mod.rs      │  Runs inference with system prompt + chat history
│                  │  Model: GGUF format, Metal GPU acceleration
│                  │  Temperature: 0.1 (deterministic tool calling)
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  Response Parser │
│  llm/tools.rs    │  Extracts JSON from LLM output
│                  │  Parses into ToolCall[] or Message
└────────┬─────────┘
         │
         ▼
┌──────────────────────────────────────────┐
│  Tool Execution (backend workspace_ops)  │
│                                          │
│  split_panes    → emit workspace:split   │
│  open_document  → emit workspace:open-doc│
│  open_terminal  → emit workspace:open-term│
│  close_pane     → emit workspace:close   │
│  arrange_layout → emit workspace:arrange │
│  list_files     → returns file listing   │
│  switch_workspace → emit workspace:switch│
└────────┬─────────────────────────────────┘
         │
         ▼
┌──────────────────┐
│  Frontend        │
│  Event listeners │  Receives events, updates Zustand stores,
│  in tabs store   │  re-renders pane layout
└──────────────────┘
```

The system prompt defines the available tools and expected JSON format. The LLM responds with either:
- `{ "tool_calls": [{ "tool": "...", "args": {...} }] }` -- executed as workspace operations
- `{ "message": "..." }` -- displayed as a chat response
