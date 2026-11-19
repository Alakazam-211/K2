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

## How to Add a Feature or Command

A feature is a daemon route first. The CLI, the desktop app and the web client all call the same route, so the logic is written once.

1. **Route.** Add the handler in `crates/k2-daemon/src/` (most areas have a file of their own, such as `activity_routes.rs`) and match its `/cli/...` path in `crates/k2-daemon/src/routes/dispatcher.rs`. Keep the dispatcher arm thin: it is one of the biggest files, and `scripts/garden/ratchet.sh` fails if it grows.
2. **Policy row.** Every `/cli/*` route needs a row in the route policy table (`ROUTES` in `crates/k2-daemon/src/routes/route_policy.rs`). The rows live in `crates/k2-daemon/src/routes/policy/`, one file per busy area and `core.rs` for the rest, each sorted by path. A row gives the method and the lowest login role, for example `get("/cli/fs/read-binary", Member)`. A route with no row is refused for logins, and the test `every_cli_route_literal_is_in_the_policy_table` fails.
3. **Logic.** Put it in `crates/k2-daemon` or `crates/k2-core`, never in `src-tauri/` or the renderer.
4. **CLI verb.** Add the command to a part under `cli/src/` (a new part is also listed in `cli/src/ORDER`), wire it in `cli/src/router.sh`, then run `bash scripts/build-cli.sh`. Never edit `cli/k2`: the next build undoes it.
   A new verb also needs its schema entry, which is what `k2 --schema` prints: add it to `cli/src/schema/<noun>.entries` (`build-cli.sh` checks each of those files). The explorer catalog `src/shared/cli-catalog.generated.json` is generated from it by `bash scripts/regen.sh`; never edit it by hand.
5. **Renderer.** Call the route with `daemonCliGet` or `daemonCliPost` from `src/renderer/lib/daemon-cli.ts` and render the answer. No logic here.

Add a Tauri command in `src-tauri/src/commands/` only for connection or OS work.

Checks to run before you open a PR:

```bash
bash scripts/garden/ratchet.sh                      # the biggest source files only get smaller
bash scripts/build-cli.sh --check                   # cli/k2 is the join of its parts
node scripts/gen-cli-explorer-catalog.mjs --check   # the CLI explorer catalog is current
bash scripts/regen.sh --check                       # every generated file is current (runs the two above and more; needs cargo)
bash scripts/private-docs-gate.sh                   # no private docs in the public tree
bun run typecheck
bun run test                                        # vitest
```

`scripts/ci/checks.sh` lists everything CI runs.

## How to Add a New Agent Preset

Agent presets are CLI commands that launch in a terminal. The built-in ones are the `BUILT_IN_AGENT_PRESETS` list in `crates/k2-core/src/db/mod.rs`. `seed_agent_presets` inserts each entry at startup when no built-in row with that label exists, so it never overwrites a preset a user has changed.

To add a new built-in preset:

1. Add an entry to `BUILT_IN_AGENT_PRESETS`. Its shape is `(id, label, command, icon, sort_order)`:

   ```rust
   ("unique-uuid-here", "Agent Name", "cli-command --flags", "", 13),
   ```

   The label must be unique: it is the key the seed matches on. No SQL migration is needed.
2. Optional: add a row for the label to `BUILT_IN_PRESET_METADATA` in the same file. Leave it out when you have not checked the values; a missing row means "unknown".
3. Update the count in the test `seed_agent_presets_creates_expected_entries`.
4. If the agent has its own icon, add an SVG to `src/renderer/assets/llm-icons/` and a rule to `LOBE_ICON_RULES` in `src/renderer/components/AgentIcon/AgentIcon.tsx`. Note its source and licence in `NOTICE.md` in that folder.

Users can also add custom presets in the app without code changes.

## How to Add a New Document Viewer

Document viewers live in `src/renderer/components/FileViewerPane/`. `fileCategory.ts` maps a file extension to a category, and `FileViewerPane.tsx` renders the viewer for that category.

1. Add a viewer component in `FileViewerPane/` (e.g., `EpubViewer.tsx`).
2. In `fileCategory.ts`, add the category to `FileCategory`, add its extension list, and return it from `getFileCategory`. Set its default mode in `getDefaultViewMode`. If the viewer reads bytes and not text, add the category to `isBinaryPreviewCategory`.
3. In `FileViewerPane.tsx`, load the component with `lazy()` like the other viewers and add a branch for the category.
4. Get the file from the daemon, never from a Tauri command or the local disk: the file may be on another server. A text viewer takes the `content` that `FileViewerPane` already loaded (see `CsvViewer.tsx`). A binary viewer calls `loadHostBinary(scope, filePath)` from `src/renderer/lib/load-host-binary.ts`, which reads the `fs/read-binary` route (see `ModelViewer.tsx`).
5. If the format needs a parser, add the npm dependency to `package.json`.
6. Add the new extension to `FileViewerPane.category.test.ts`.

Existing viewers:
- **Markdown** -- `react-markdown` with `remark-gfm`
- **PDF** -- `PDFViewer.tsx`, `pdfjs-dist`
- **DOCX** -- `DocxViewer.tsx`, `mammoth`
- **CSV and TSV** -- `CsvViewer.tsx`
- **Images** -- `ImageViewer.tsx`
- **Audio and video** -- `MediaViewer.tsx`
- **Zip archives** -- `ZipViewer.tsx`
- **Mermaid and other diagram sources** -- `DiagramViewer.tsx`, `mermaid`
- **DXF drawings** -- `DxfViewer.tsx`, `dxf-parser`
- **3D models** -- `ModelViewer.tsx`, `three`
- **Board files** -- `BoardFilePane.tsx`
- **Code and plain text** -- `CodeEditor.tsx`

## Code Style

### Rust
- Run `cargo fmt` before committing.
- Run `cargo clippy` and address warnings.
- A route handler follows the daemon's pattern: the handler sits in its area's `*_routes.rs` file in `crates/k2-daemon/src/`, and the arm in `routes/dispatcher.rs` only matches the path and calls it.
- A route that changes something answers POST only: call `require_post` (in `routes/http.rs`) before doing the work.
- Add a `#[tauri::command]` in `src-tauri/src/commands/` only for connection or OS work, and use `Result<T, String>` for its return type.

### TypeScript
- Use TypeScript strict mode (see `tsconfig.web.json`).
- Use Zustand for state management -- no prop drilling.
- Functional components only.
- TailwindCSS v4 for styling -- no CSS modules or styled-components.

## Pull Request Process

1. Fork the repo and create a feature branch from `main`.
2. Make your changes following the code style guidelines above.
3. Run `cargo fmt`, `cargo clippy`, `cargo test` for the crate you touched (for example `cargo test -p k2-core`; the `k2-daemon` tests run under `cargo nextest run -p k2-daemon`), and `bun run typecheck` before pushing.
4. Open a PR against `main` with a clear description of what changed and why.
5. Keep PRs focused -- one feature or fix per PR.
6. On your first pull request, sign our [Contributor License Agreement](CLA.md) once: a bot asks you to, and you reply with the comment it gives. The CLA lets Alakazam Labs ship your contribution in K2 under its licence and under other terms; you keep your copyright.
