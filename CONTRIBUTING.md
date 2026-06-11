# Contributing to K2

Thanks for your interest in contributing. This guide covers development setup, project structure, and how to extend K2.

## Development Setup

### Prerequisites

- [Rust](https://rustup.rs/) (stable toolchain)
- [Node.js](https://nodejs.org/) 18+ or [Bun](https://bun.sh/)
- cmake (required for llama.cpp compilation)
- Xcode Command Line Tools (macOS): `xcode-select --install`

### Running Locally

```bash
git clone https://github.com/Alakazam-211/K2.git
cd K2
npm install
cargo tauri dev
```

This starts Vite's dev server (hot reload) and compiles the Rust backend. The first build takes a few minutes due to llama.cpp compilation.

### Useful Commands

```bash
npm run vite:dev       # Frontend only (no Tauri)
npm run typecheck      # TypeScript type checking
cargo fmt              # Format Rust code
cargo clippy           # Lint Rust code
cargo test -p k2-core -p k2-daemon   # Run Rust tests
```

## Project Structure

```
K2/
├── src/
│   └── renderer/              # React frontend
│       ├── components/        # UI components
│       │   ├── Terminal/      # Terminal pane (Alacritty-backed custom renderer)
│       │   ├── FileViewerPane/  # .md / .pdf / .docx viewer
│       │   ├── WorkspaceAssistant/  # LLM chat panel
│       │   ├── PaneLayout/    # Mosaic-based pane tiling
│       │   ├── Sidebar/       # Project list, focus groups
│       │   ├── PresetsBar/    # Agent launch buttons
│       │   ├── CommandPalette/  # Cmd+K command palette
│       │   ├── FocusWindow/   # Detached focus window
│       │   ├── FileTree/      # File browser
│       │   ├── TabBar/        # Workspace tabs
│       │   ├── Settings/      # Settings panel
│       │   └── ...
│       ├── stores/            # Zustand state stores
│       │   ├── projects.ts    # Project CRUD + selection
│       │   ├── tabs.ts        # Tab/pane state
│       │   ├── panels.ts      # Panel layout state
│       │   ├── presets.ts     # Agent preset state
│       │   ├── filetree.ts    # File tree state
│       │   ├── focus-groups.ts  # Focus group management
│       │   ├── assistant.ts   # LLM assistant state
│       │   ├── settings.ts    # App settings
│       │   └── ...
│       ├── hooks/             # React hooks
│       ├── lib/               # Shared utilities
│       └── types/             # TypeScript type defs
├── src-tauri/
│   └── src/
│       ├── lib.rs             # App entry, plugin + command registration
│       ├── state.rs           # AppState (db, terminal manager, LLM)
│       ├── commands/          # Tauri IPC command handlers
│       │   ├── projects.rs    # Project CRUD, icon detection, editor launch
│       │   ├── workspaces.rs  # Workspace CRUD
│       │   ├── focus_groups.rs  # Focus group CRUD
│       │   ├── workspace_sections.rs  # Workspace sections
│       │   ├── agents.rs      # Agent preset management
│       │   ├── terminal.rs    # PTY create/write/resize/kill
│       │   ├── git.rs         # Branch, worktree, changes info
│       │   ├── filesystem.rs  # File read/write/browse
│       │   ├── workspace_ops.rs  # Layout manipulation (split, close, arrange)
│       │   ├── assistant.rs   # LLM chat, model loading, download
│       │   ├── settings.rs    # Key-value settings
│       │   └── project_config.rs  # Per-project config (run commands, etc.)
│       ├── db/                # SQLite database
│       │   ├── mod.rs         # Init, migrations, seeding
│       │   └── schema.rs      # Table structs + queries
│       ├── terminal/          # PTY management
│       │   └── mod.rs         # TerminalManager (portable-pty)
│       ├── llm/               # Local LLM inference
│       │   ├── mod.rs         # LlmManager (llama-cpp-2)
│       │   ├── tools.rs       # System prompt, tool definitions, response parsing
│       │   └── download.rs    # Model download from HuggingFace
│       ├── git/               # Git operations (git2)
│       │   └── mod.rs
│       ├── editors.rs         # External editor detection
│       ├── menu.rs            # Native menu bar
│       ├── window.rs          # Window state save/restore
│       └── project_config.rs  # Project config file parsing
├── resources/                 # Icons and static assets
├── drizzle/                   # Migration definitions (reference)
└── vite.config.ts             # Vite build config
```

## How to Add a New Agent Preset

Agent presets are CLI commands that launch in a terminal. Built-in presets are seeded in `src-tauri/src/db/mod.rs` in the `seed_agent_presets` function.

To add a new built-in preset:

1. Add an entry to the `presets` array in `seed_agent_presets()`:

```rust
("unique-uuid-here", "Agent Name", "cli-command --flags", "emoji", sort_order),
```

2. If the agent has a custom icon, add an SVG to `resources/agent-icons/` and reference it in the frontend's `AgentIcon` component.

3. Users can also add custom presets through the Settings UI without code changes.

## How to Add a New Document Viewer

Document viewers live in `src/renderer/components/FileViewerPane/`. The component dispatches on file extension.

1. Add a new viewer component in `FileViewerPane/` (e.g., `CsvViewer.tsx`).
2. Update the file extension dispatch logic in the main `FileViewerPane` component to route your extension to the new viewer.
3. If the file format needs a parser, add the npm dependency to `package.json`.
4. If binary file reading is needed, use the `fs_read_binary_file` Tauri command.

Existing viewers:
- **Markdown** -- `react-markdown` with `remark-gfm`
- **PDF** -- `pdfjs-dist`
- **DOCX** -- `mammoth`

## How to Add Workspace Primitives

Workspace primitives are Tauri commands that manipulate the pane layout. They live on both sides:

**Backend** (`src-tauri/src/commands/workspace_ops.rs`):
1. Add a new `#[tauri::command]` function that emits an event via `app.emit()`.
2. Register it in `lib.rs` in the `invoke_handler` macro.

**Frontend** (`src/renderer/stores/tabs.ts` or `panels.ts`):
1. Listen for the emitted event with `listen()` from `@tauri-apps/api/event`.
2. Update the Zustand store to reflect the layout change.

Example existing primitives: `workspace_split_pane`, `workspace_close_pane`, `workspace_open_document`, `workspace_open_terminal`, `workspace_arrange`.

## Code Style

### Rust
- Run `cargo fmt` before committing.
- Run `cargo clippy` and address warnings.
- Use `Result<T, String>` for Tauri command return types.
- Follow the existing pattern of `#[tauri::command]` functions in `src-tauri/src/commands/`.

### TypeScript
- Use TypeScript strict mode (see `tsconfig.web.json`).
- Use Zustand for state management -- no prop drilling.
- Functional components only.
- TailwindCSS v4 for styling -- no CSS modules or styled-components.

## Pull Request Process

1. Fork the repo and create a feature branch from `main`.
2. Make your changes following the code style guidelines above.
3. Run `cargo fmt`, `cargo clippy`, and `npm run typecheck` before pushing.
4. Open a PR against `main` with a clear description of what changed and why.
5. Keep PRs focused -- one feature or fix per PR.

## Developer Certificate of Origin (DCO)

Contributions are accepted under the
[Developer Certificate of Origin v1.1](https://developercertificate.org/).
By signing off your commits you certify that you wrote the contribution
(or otherwise have the right to submit it) and that it may be distributed
under this repository's license (FSL-1.1-Apache-2.0 — see
[LICENSE.md](LICENSE.md)).

Sign off every commit with the `-s` flag:

```bash
git commit -s -m "fix: short description"
```

This appends a `Signed-off-by: Your Name <you@example.com>` line matching
your commit author. PRs with unsigned commits will be asked to rebase.
