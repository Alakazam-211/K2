<p align="center">
  <img src="docs/hero.png" width="100%" alt="K2 by Alakazam Labs — The workspace where your AI agents run">
</p>

<p align="center">
  <a href="https://github.com/Alakazam-211/K2/blob/main/LICENSE.md"><img src="https://img.shields.io/badge/license-FSL--1.1--Apache--2.0-blue.svg" alt="FSL-1.1-Apache-2.0"></a>
  <a href="https://k2.dev"><img src="https://img.shields.io/badge/k2.dev-8B5CF6.svg" alt="k2.dev"></a>
  <img src="https://img.shields.io/badge/platform-macOS-lightgrey.svg" alt="macOS">
  <img src="https://img.shields.io/badge/built_with-Tauri_v2-24C8D8.svg" alt="Tauri v2">
</p>

---

K2 is **not a traditional IDE**. It's agent server software — a workspace that hosts, runs, and monitors your AI coding agents through the CLI tools you already use, manages git worktrees, and reviews documents, all from a terminal-first interface built with Tauri and Rust.

Think of it as a command center: you add your projects, launch AI agents (Claude, Codex, Gemini, Copilot, Aider, and more) in GPU-accelerated terminals, orchestrate them autonomously, review markdown and PDFs in dark-themed viewers, and reach the whole thing remotely from any device.

<!-- Hero screenshot — drop the image at docs/screenshot.png, then uncomment: -->
<!-- ![K2 Screenshot](docs/screenshot.png) -->

## Features

### Remote Access — K2 Connect
Reach your machine from anywhere. **K2 Connect** exposes your local K2 daemon over a secure tunnel so you can drive your workspaces — terminals, agents, and files — from another device through a custom `your-name.k2.dev` subdomain.

- **Remote workspaces** — open and operate a folder on your home or work machine from across the network
- **Remote files & Clone-to** — browse, upload, and one-click migrate an entire workspace to another K2 machine
- **Multi-user access** — owner-managed connect users with roles (Owner / Admin / Member), a configurable password policy, and session controls

K2 Connect's hosting backbone (tunnel control plane + relay) runs on dedicated infrastructure and lives in a **separate, private repository** — this open-core repo is the K2 app and daemon; the hosted relay is the closed half.

### Styles
K2's whole visual language is data. Pick a **Style** in Settings → Styles — **Square** (the classic flat look), **Liquid Glass** (frosted translucency), or **Bezel** (layered-ring cards) — then a palette within it (including **Paper**, a warm light mode), a Light/Dark/**Auto** scheme, and a density from flush to spacious. Terminal ANSI colors switch with the palette. Styles are schema-validated packages under [`styles/`](styles/) — community Styles and palettes arrive by PR ([CONTRIBUTING-STYLES.md](CONTRIBUTING-STYLES.md)).

### AI Workspace Assistant (Cmd+L)
A local LLM (GGUF via llama.cpp with Metal acceleration) translates natural language into workspace operations -- split panes, open files, launch terminals, arrange layouts -- without leaving the keyboard.

### CLI Agent Integration
Quick-launch buttons for the agents you already use:
- **Claude** -- `claude --dangerously-skip-permissions`
- **Codex** -- `codex` with configurable reasoning effort
- **Gemini** -- `gemini --yolo`
- **Copilot** -- `copilot --allow-all`
- **Aider**, **Cursor Agent**, **OpenCode**, **Code Puppy**

Add your own custom agent presets or edit the built-ins. Set a default agent and launch it instantly with Cmd+Shift+T.

### Autonomous Agent System (BETA)

K2 includes a closed-loop agent orchestration system where agents at every level are self-aware, self-configuring, and event-driven.

**Agent Hierarchy:**
- **K2 Agent** -- Top-level planner that coordinates across workspaces, creates PRDs and milestones
- **Coordinator** -- Per-workspace orchestrator that delegates work to agent templates, reviews completed branches
- **Agent Templates** -- Specialized agents (backend-eng, frontend-eng, qa-tester) that work in isolated worktrees
- **Custom Agents** -- User-defined agents with adaptive heartbeat timing

**Adaptive Heartbeat with Local LLM Triage:**
The heartbeat system uses a two-tier cost model. A local LLM (Qwen 1.5B, running on-device via llama.cpp with Metal acceleration) evaluates whether expensive cloud sessions should be launched. The flow: filesystem check ($0) -> lock check ($0) -> quality gate ($0) -> local LLM decision ($0) -> cloud model session (only if the LLM approves). Agents self-adjust their check-in frequency (1 min during active work, 1 hour when idle). Auto-backoff increases the interval by 1.5x after 3 consecutive idle wakes. Active hours windows prevent overnight cost waste.

**Event-Driven via Claude Code Channels:**
Coordinators can run as persistent Claude sessions with MCP channel integration (`--channels`). K2 implements an MCP channel server that pushes events (new work items, git changes, CI results, agent lifecycle events) directly into the running session. No polling delay, no context reload -- the agent maintains full session context across events.

**Self-Configuring Agents:**
Coordinators create and configure skill profiles via CLI (`k2 skills create`, `k2 skills profile`). Skill profiles (`SKILL.md`) are editable with AI assistance via the built-in AIFileEditor. Each Coordinator knows its workspace's available skills by reading those files before applying a role to work.

**Decentralized Work Discovery:**
Each workspace knows where to find work (GitHub Issues, Linear, PRDs) via its agent profile. No central scanner or integration layer needed -- agents use CLI tools (`gh`, `git`, `curl`) already available in the terminal. The filesystem work queue at `.k2/inbox/` is the always-on mechanism, scanned by the local LLM triage at near-zero cost.

**Multi-Terminal Execution:**
Agents can spawn parallel sub-terminals for concurrent tasks (`k2 terminal spawn`). Sub-terminals appear as pane splits within the agent's tab.

**Session Resume & Transcript Pruning:**
Heartbeat agents use Claude Code's `--resume` flag to continue from their last session, avoiding full context reload on each wake. No-op sessions (agent woke up, found nothing to do) are pruned automatically -- the session ID is cleared so the next launch starts fresh.

**Virtual Terminal I/O:**
The `k2` CLI can read from and write to any running terminal session. Agents can communicate with each other across workspaces:
- `k2 workspace list --running` -- list all workspaces with live agents right now
- `k2 terminal write <id> "message"` -- (advanced/internal) send text to a running terminal by id (raw PTY keystrokes)
- `k2 terminal read <id> --lines 50` -- (advanced/internal) read the last N lines from a terminal buffer

The `terminal write` and `terminal read` verbs are internal-tier — used by orchestrators that integrate with K2. For everyday use, prefer `k2 msg <workspace> "..."` (live cross-workspace delivery) and `k2 inbox` (async email-style inbox).

**Running Agents Panel (Cmd+J):**
A searchable overlay showing all active CLI LLM sessions across workspaces. Click to navigate, copy terminal ID for CLI reference, or send messages directly to running agents. TopBar button shows active agent count.

**Coordinator Automation:**
- `k2 heartbeat wake` -- detects inbox work, wakes the workspace-agent (resumes previous session), and sends a triage message
- `k2 agent complete` -- work-item completion that auto-merges (Build state) or submits for review (Managed Service) based on workspace state capabilities
- Delegated worktrees include a completion protocol in CLAUDE.md so the workspace-agent knows how to finish each work item

**Chat Tab Live Terminal Connection:**
The Chat tab for Coordinators and Worktrees follows a three-step lifecycle:
1. **Attach** -- if a terminal is already running, connect to it with real-time grid updates
2. **Resume** -- if no terminal but a previous session exists, launch with `--resume`
3. **Fresh** -- if no terminal and no session, start a new Claude session

**Launch Failure Detection:**
If an agent's terminal exits within 5 seconds of starting, K2 treats it as a launch failure, notifies the user, and retries once after 30 seconds.

### Workspace States

Each workspace operates under a configurable **state** that controls what agents can do automatically. States define per-capability permissions using a three-level model:

- **Auto** -- agents build and merge without asking
- **Gated** -- agents build PRs and publish review URLs, but wait for human approval before merging
- **Off** -- agents don't act on this type of work

Work items are tagged by source type (feature, issue, crash, security, audit, manual), and the workspace state gates which sources agents can auto-act on.

**Default States:**

| State | Features | Issues | Crashes | Security | Audits |
|-------|----------|--------|---------|----------|--------|
| **Build** | Auto | Auto | Auto | Auto | Auto |
| **Managed Service** | Gated | Auto | Auto | Auto | Gated |
| **Maintenance** | Off | Gated | Gated | Gated | Gated |
| **Locked** | Off | Off | Off | Off | Off |

Users can create custom states with any combination of capability levels. States are defined in Settings and assigned per-workspace.

### Agent Lifecycle Detection
K2 detects when AI agents start, stop, or request permissions via a hook system that integrates with Claude Code, Cursor, and Gemini CLI. Active agents show real-time status indicators (working, awaiting permission, done) in the sidebar.

### Active Workspaces Dock
A collapsible dock at the bottom of the sidebar shows projects with running agents or recent activity. Switch between active workspaces with Cmd+1-9. Projects appear automatically when agents run and can be pinned or dismissed.

### Document Review
View `.md`, `.pdf`, and `.docx` files inline with a dark-themed viewer. Markdown renders with GFM support; PDFs use pdf.js; Word docs convert via mammoth.

### Terminal-First
GPU-accelerated terminals powered by [Alacritty](https://github.com/alacritty/alacritty)'s terminal emulator library, rendered via a custom DOM-based frontend. Split into up to 3 independent tab group columns, each with their own tab bar. Drag tabs between columns. Resize columns freely. Natural text editing (macOS-style Opt+Arrow word navigation, Cmd+Arrow line navigation) enabled by default.

### Chat History & Session Resume
View Claude and Cursor chat history in the sidebar. Click a session to resume it. When the app closes, terminal sessions are saved and resumed on reopen with `--resume` flags. Fresh chat tabs auto-rename to match the conversation title.

### Terminal Persistence
Terminal PTYs survive tab switches via a scrollback buffer architecture. Switch tabs freely without losing terminal state -- output is buffered and replayed on reattach.

### Workspace Tab
A unified **Workspace** panel (replacing the old Agents + Review tabs) shows two sections:
- **Status** -- current mode, work summary counters (Inbox/Active/Review), Launch button, heartbeat toggle, workspace state selector
- **Worktrees** -- all open worktrees with Task/Chat/Review tabs

The status section shows color-coded work counts across all agents. The Launch button triggers `k2 heartbeat wake` to wake the coordinator. Click a worktree to view its assigned task, chat with the running agent, or review completed work. Click **Open Full Workspace** to promote it to a full nav entry with file tree and changes panel.

### Git Worktree Management
First-class support for git worktrees. Create worktrees from new or existing branches via the "+" button in the Workspace panel. Projects can run in worktree mode or standard mode.

### Focus Groups & Pinned Workspaces
Group related projects together with focus groups. Pin specific workspaces above the focus group filter so they're always accessible regardless of which group is active. Cmd+Shift+1-9 switches between pinned workspaces (swappable with active shortcuts in settings).

### Layout Persistence
Workspace layouts (tab groups, open documents, terminal sessions) save and restore automatically when you switch between workspaces. Panel tab arrangement and sidebar state persist across app restarts.

### Keyboard Shortcuts
- **Cmd+1-9** -- Switch active workspaces
- **Cmd+Option+1-9** -- Switch agents/pinned workspaces
- **Cmd+Shift+T** -- Launch default AI agent
- **Cmd+T** -- New terminal tab
- **Cmd+D** -- Split pane
- **Cmd+L** -- AI Workspace Assistant
- **Cmd+K** -- Quick switcher
- **Cmd+J** -- Running Agents panel
- **Cmd+P** -- Review queue

### Icon Cropping
Upload custom workspace icons with a built-in crop dialog -- drag to position, scroll to zoom, apply to save.

### Built with Tauri + Rust
~5MB binary. Native PTY management via `portable-pty`. Alacritty terminal emulator for rendering. SQLite database via `rusqlite`. Git operations via `git2`. Local LLM inference via `llama-cpp-2`. No Electron, no bloat.

## Installation

### Download
Get the latest release from the [GitHub Releases](https://github.com/Alakazam-211/K2/releases) page. Project home: [k2.dev](https://k2.dev).

### Build from Source

**Prerequisites:**
- [Rust](https://rustup.rs/) (stable)
- [Bun](https://bun.sh/) (or [Node.js](https://nodejs.org/) 18+)
- cmake (for llama.cpp compilation)
- Xcode Command Line Tools (macOS)

```bash
git clone https://github.com/Alakazam-211/K2.git
cd K2
bun install
cargo tauri dev
```

For a release build:

```bash
cargo tauri build
```

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                   React 19 Frontend                 │
│  Zustand stores  │  Alacritty terminals  │  Viewers │
├─────────────────────────────────────────────────────┤
│                  Tauri v2 IPC Bridge                │
├─────────────────────────────────────────────────────┤
│                    Rust Backend                     │
│  portable-pty │ rusqlite │ git2 │ llama-cpp-2       │
├─────────────────────────────────────────────────────┤
│              Agent Orchestration Layer              │
│  Heartbeat scheduler │ CLI bridge │ Event queues    │
│  MCP channel server  │ Work queues │ Worktrees      │
└─────────────────────────────────────────────────────┘
```

| Layer | Tech | Purpose |
|-------|------|---------|
| Frontend | React 19, TailwindCSS v4, Zustand | UI and state management |
| Terminals | Alacritty terminal library, custom DOM renderer | GPU-accelerated terminal emulation |
| IPC | Tauri commands + events | Frontend-backend communication |
| Backend | Rust, Tauri v2 | Terminal PTY, database, git, LLM |
| Database | SQLite (rusqlite, WAL mode) | Projects, workspaces, presets, settings |
| AI | llama-cpp-2 (Metal) | Local LLM for workspace assistant + heartbeat triage |
| Agent Hooks | HTTP notification server | Lifecycle detection for Claude/Cursor/Gemini |
| Agent System | k2 CLI, heartbeat scheduler, MCP channels | Autonomous agent orchestration |
| Git | git2 | Worktree and branch management |

For the full technical architecture, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Testing

Tests live in the `tests/` directory. Run them against a running K2 instance.

### Test Suites

| Suite | Tests | Prerequisites | What it validates |
|-------|-------|--------------|-------------------|
| `cli-integration-test.sh` | 69 pass, 0 skip | Running K2 | Every CLI command, delegate, review approve/reject, cross-workspace work, heartbeat, terminal I/O |
| `behavior-test-tier1.sh` | 25 pass | Running K2 | Auto-backoff math, lock prevention, priority ordering, session resume, CLAUDE.md content, event queue flow, transcript pruning |
| `behavior-test-tier2.sh` | 8+ pass | Running K2 + registered workspace | Source gating by state, locked state blocking, state persistence |
| `behavior-test-tier3.sh` | 22 pass | sqlite3 only | Migration safety, heartbeat script correctness, agent templates, LLM triage prompt validation |

Tests auto-register workspaces via `k2 workspace open` -- no manual UI setup required. Total: **116+ tests, 0 failures, 0 skipped**.

### Running Tests

```bash
# 1. Start K2
cargo tauri dev

# 2. Run all test suites (workspaces auto-register)
./tests/cli-integration-test.sh    # Full CLI + agent orchestration
./tests/behavior-test-tier1.sh     # Behavioral (filesystem-based)
./tests/behavior-test-tier3.sh     # Unit-style (no K2 needed)

# 3. For DB-dependent tests:
./tests/behavior-test-tier2.sh
```

Set `TEST_WORKSPACE` to use a different workspace:
```bash
TEST_WORKSPACE=/path/to/workspace ./tests/behavior-test-tier1.sh
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, project structure, and how to add new agent presets, document viewers, and workspace primitives.

## License

[FSL-1.1-Apache-2.0](LICENSE) — free for personal and internal/business use; it
converts to Apache 2.0 two years after each release.

Hosting K2 commercially for others is additionally permitted **via the official
K2 Connect tunnel service** — see the [Commercial Hosting Grant](COMMERCIAL_HOSTING_GRANT.md).
