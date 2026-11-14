# Contributing to K2

Thanks for your interest in contributing. This guide covers development setup, project structure, and how to extend K2.

## Development Setup

### Prerequisites

- [Rust](https://rustup.rs/) (stable toolchain)
- [Bun](https://bun.sh/) (or [Node.js](https://nodejs.org/) 18+)
- cmake (required to build vendored C dependencies such as aws-lc-sys)
- Xcode Command Line Tools (macOS): `xcode-select --install`

### Running Locally

```bash
git clone https://github.com/Alakazam-211/K2.git
cd K2
bun install
cargo tauri dev
```

This starts Vite's dev server (hot reload) and compiles the Rust backend. The first build takes a few minutes.

### Useful Commands

```bash
bun run vite:dev       # Frontend only (no Tauri)
bun run typecheck      # TypeScript type checking (tsc-rs; see docs/typecheck.md)
cargo fmt              # Format Rust code
cargo clippy           # Lint Rust code
cargo test -p k2-core  # Run Rust tests
```

## Project Structure

K2 is daemon-first: logic lives in `crates/k2-daemon` and `crates/k2-core`, `src-tauri/` is connection and OS work only, and the renderer is a thin client.

```
K2/
├── crates/                    # Rust workspace members (see Cargo.toml)
│   ├── k2-daemon/             # The long-running daemon: scheduler and agents keep working with the app closed. Logic goes here
│   ├── k2-core/               # Core library: db, llm, terminal, scheduler, agent hooks. Logic goes here too
│   │   └── drizzle_sql/       # Numbered SQL migrations
│   ├── k2-client/             # What the thin client needs without k2-core: paths, process bootstrap, account session
│   ├── k2-vault/              # Encrypted store for passwords, tokens and keys (k2-redact: exact-value redaction)
│   ├── k2-ptyd/               # Per-user PTY host: terminal sessions outlive the program that drives them
│   ├── k2-term-grid/          # Terminal grid model for remote rendering (k2-term-wire: its binary wire format)
│   ├── k2-node/               # Compute node runtime (k2-node-proto: the wire to a controller daemon)
│   ├── k2-root-helper/        # The one root door on Linux (k2-root-proto: its protocol)
│   ├── k2-app-engine/         # Drives rootless Podman for K2 Apps on the daemon's orders
│   ├── k2-deskd/              # K2 Screen, the remote desktop helper (k2-deskd-proto: its wire to the daemon)
│   ├── k2-simvideo/           # The Mobile tab's device stream: H.264 encoding and flow control
│   ├── k2-export/             # Export worker: turns a doc version into another format in its own process
│   ├── k2-deckrender/         # .pptx slide renderer, one process per render (k2-scenerender: 3D stills)
│   ├── k2-release-keys/       # Update-signing key list and verify (also k2-signer, k2-approve, kessel-attest)
│   ├── k2-menubar/            # macOS menu-bar helper for the daemon
│   ├── k2-transfer-client/    # Resumable uploads and downloads (k2-winsec: owner-only file ACLs on Windows)
│   ├── kessel-audio/          # Real-time voice audio pipeline (kessel-video, kessel-frame-ring: video and frames)
│   └── vp8-decoder/           # VP8 decoder in pure Rust (vp8-encoder: the encoder)
├── src-tauri/                 # Tauri desktop app: connection and OS work only, no logic
│   └── src/
│       ├── lib.rs             # App entry
│       ├── commands/          # Tauri IPC command handlers
│       ├── daemon_client.rs   # Talks to the daemon
│       └── menu.rs, tray.rs, window.rs  # Native menu, tray and windows
├── src/
│   ├── renderer/              # React frontend, a thin client of the daemon
│   │   ├── components/        # UI components (Terminal/, FileViewerPane/, Sidebar/, Settings/, ...)
│   │   ├── stores/            # Zustand state stores
│   │   ├── hooks/             # React hooks
│   │   ├── lib/               # Shared utilities
│   │   └── types/             # TypeScript type defs
│   └── shared/                # Constants, types and generated data the frontend shares with the Rust crates and the CLI
├── cli/
│   ├── src/                   # The CLI's parts (*.sh), joined in the order cli/src/ORDER lists
│   └── k2                     # The built CLI. Edit a part, then run scripts/build-cli.sh; never edit this file
├── sdk/                       # k2-runtime.js (widget frame runtime), server/, activity/, mail-reader/, generated/
├── styles/                    # Style packages: the data that defines K2's look (see styles/README.md)
├── web/                       # Loader and local Caddyfile for the hosted web client
├── scripts/                   # Build, release, install and CI scripts, and the repo gates
├── tests/                     # Shell tests (cli/), goldens, contract tests and test fakes; Rust tests sit in crates/*/tests
├── packaging/                 # Linux packages: arch/ and omarchy/
├── docs/                      # Design and reference docs
├── vendor/                    # Vendored crates: deckcraft and k2-mpc-*
├── third_party/               # Patched copies of upstream crates (tao, wry, vte, objc2, ...) and the Sew client crates
├── resources/                 # Icons and static assets
├── .github/workflows/         # GitHub Actions workflows
├── Cargo.toml                 # Rust workspace
├── package.json               # Frontend scripts and dependencies
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

## Code Style

### Rust
- Run `cargo fmt` before committing.
- Run `cargo clippy` and address warnings.
- Use `Result<T, String>` for Tauri command return types.
- Follow the existing pattern of `#[tauri::command]` functions in `src-tauri/src/commands/` (connection and OS work only; logic lives in the daemon).

### TypeScript
- Use TypeScript strict mode (see `tsconfig.web.json`).
- Use Zustand for state management -- no prop drilling.
- Functional components only.
- TailwindCSS v4 for styling -- no CSS modules or styled-components.

## Pull Request Process

1. Fork the repo and create a feature branch from `main`.
2. Make your changes following the code style guidelines above.
3. Run `cargo fmt`, `cargo clippy`, and `bun run typecheck` before pushing.
4. Open a PR against `main` with a clear description of what changed and why.
5. Keep PRs focused -- one feature or fix per PR.
