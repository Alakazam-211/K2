# K2SO — Project Context (workspace-specific)

This file is for **this workspace's** specific architectural rules — content here gets adopted into `SKILL.md` on regen via the SOURCE region mechanism (per `crates/k2so-core/src/skills/version.rs` docs). It does NOT ship to other K2 users.

## Foundational rule: the thin client is connection-only

**`src-tauri/` is NOT where features belong.** It is connection points + OS integration only. Features live in `crates/k2so-core` (shared library) or `crates/k2so-daemon` (daemon-side).

### Why

K2 Connect (the paid hosted tier launching at 0.40.0) lets a thin client connect to a remote daemon — your agent runs in Alakazam infrastructure, the local app is just a window into it. For this to work, the thin client MUST be transport-agnostic. Every line of business logic in `src-tauri/` is a line K2 Connect can't reach without an additional bridge. **Fat thin client = K2 Connect blocked or vastly more expensive to ship.**

### What belongs WHERE

**🟢 STAYS in `src-tauri/`**:
- Tauri command stubs that proxy webview → daemon (target: ≤5 lines each)
- Native macOS APIs (tray, menu, file dialogs, permissions, keychain, launchctl)
- Hook script generation (writes user's `~/.claude`, `~/.cursor`, `~/.config/gemini` configs — host file I/O)
- Window/webview hosting

**🔴 DOES NOT belong in `src-tauri/`** (move to `k2so-core` or `k2so-daemon`):
- HTTP servers, route dispatchers
- Database access (the daemon owns `db::shared()`)
- Business decisions, validation, authorization
- Workspace state management
- Migration helpers (run from daemon startup, not Tauri startup)
- Template generation
- Anything duplicating `k2so-core` logic

### The K2 Connect litmus test

Before adding code to `src-tauri/`, ask: **"If a user's daemon were running on Alakazam's servers in another country, would this code still work?"**

- ✅ Tauri command takes a string, forwards to daemon via HTTP → works regardless of where daemon is
- ✅ macOS tray icon updates from daemon events → daemon notifies via WS, Tauri updates OS
- ❌ HTTP server inside Tauri the daemon sends events to → remote daemon can't reach local Tauri through firewall
- ❌ SQLite queries in Tauri commands → DB lives where the daemon is
- ❌ Migration walker at Tauri startup → migrations are the daemon's job

If the answer is NO, the code belongs in `k2so-core` or `k2so-daemon`, not `src-tauri/`.

### Current state (2026-05-25)

`src-tauri/src/` is 8,053 lines — up from ~3,200 pre-refactor. Audit subagent confirmed most growth is LEGITIMATE OS integration (hooks, launchctl, menu, tray, permissions). The genuine K2-Connect-blocker bloat is concentrated in:

1. `commands/daemon.rs` (403 lines) — launchctl wrappers; should expose `DaemonPlist` generation via `k2so-core::daemon_lifecycle` so K2 Connect can call the same code
2. `commands/k2so_agents.rs` (1,749 lines) — has dead-code shims (lines ~577-623) and one `teardown_workspace_harness_files` helper that should move to `k2so-core::workspace`
3. `lib.rs` (1,071 lines) — legacy agent-type migration at lines 325-380 runs only on Tauri startup; should move to daemon first-boot so K2 Connect's headless flow also migrates

Slim-down tracked as task #574. Target: ~7,900 lines after the immediate cleanup; ~3,500 once `daemon_lifecycle` is extracted.

### When reviewing PRs to `src-tauri/`

- Reject anything where a Tauri command is >10 lines and not OS-integration-specific
- Reject new HTTP servers, route handlers, state machines in `src-tauri/`
- Reject embedded business logic — push to `k2so-core::*` and have Tauri call it

This rule trumps convenience. If it "feels easier" to add logic to `src-tauri/` because it needs OS access, check whether the daemon could do it instead (it usually can — daemons spawn processes, write to keychain, etc.).

## Related memory

- `project_thin_client_is_connection_only` (foundational, all conversations should respect this)
- `project_workspace_equals_agent_foundational` (workspace IS an agent)
- `project_open_source_closed_source_strategy` (K2 Connect is the proprietary product)
- `feedback_daemon_first` (daemon-first principle)
