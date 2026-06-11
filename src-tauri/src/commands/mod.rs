pub mod projects;
// Plan B cleanup — `workspaces`, `focus_groups`, `workspace_sections`,
// `agents` (presets), `git`, `states`, `workspace_layouts`, `timer`
// command files deleted. Every command they held routed daemon data and
// the renderer now reaches that data host-aware via `daemonCli*` →
// `/cli/*` on the active daemon. No host-only/lifecycle command remained
// in any of these modules, so the whole files were removed.
pub mod settings;
pub mod workspace_ops;
// Phase 2 Unit 2 — `assistant` command shims deleted. The renderer
// hits `/cli/llm/*` on k2so-daemon directly; the daemon owns the
// LLM worker subprocess + supervisor (timeout/RSS/queue/crash
// isolation). No Tauri-side LLM surface remains.
//
// Phase 2 Unit 6 — `chat_history` command shims deleted alongside
// the rest of Unit 6's batch. The renderer hits `/cli/chat/*` on
// k2so-daemon directly.
pub mod updater;
// Phase 2 Unit 5 — `claude_auth` command shims deleted. The renderer
// hits `/cli/claude-auth/*` on k2so-daemon directly; no Tauri-side
// command surface for it remains. Scheduler ownership (the launchd
// plist `dev.k2.claude-auth.plist`) now lives in the
// daemon's `claude_auth_host` module so refresh keeps working when
// the Tauri app is closed.
pub mod k2so_agents;
pub mod format;
// Phase 2 Unit 1 — `companion` command shims deleted. The renderer
// hits `/cli/companion/*` on k2so-daemon directly; no Tauri-side
// command surface for it remains.
//
// Phase 2 Unit 6 — `filesystem`, `chat_history`, `themes`,
// `skill_layers`, `review_checklist`, `project_config`, `whats_new`
// command shims deleted. The renderer hits `/cli/fs/*`, `/cli/chat/*`,
// `/cli/themes/*`, `/cli/skill-layers/*`, `/cli/review-checklist/*`,
// `/cli/project-config/*`, and `/cli/whats_new` on k2so-daemon
// directly.
pub mod daemon;
pub mod permissions;
pub mod memory_watcher;
// Phase 2.1c Item 2 — workspace inbox primitive Tauri shims. Thin
// wrappers around `k2_core::inbox::*` that mirror the daemon-side
// `/cli/inbox/*` HTTP routes so the renderer + CLI see the same data.
pub mod inbox;
// Phase 2.1 wrap-up — generic worktree filesystem readers. Powers the
// worktree detail pane's Task tab (renders the worktree's CLAUDE.md);
// reusable for any future "show a file from inside a worktree" surface.
pub mod worktree;
// Phase 2.5b follow-up — Tauri verbs for the workspace settings
// "Skills" panel. Thin forwards to `k2_core::skills::crud::*`.
pub mod skills;
// K2 Connect — cross-platform OS keychain bridge for remembered
// remote-host tokens (`k2_secret_{set,get,delete}`). macOS Keychain /
// Linux Secret Service / Windows Credential Manager via the `keyring`
// crate. Tokens NEVER touch connect-hosts.json.
pub mod secrets;
// K2 Connect — client address book persistence (`~/.k2so/connect-hosts.json`,
// non-secret host list only). Tokens live in the keychain (see `secrets`).
pub mod connect_hosts;
// K2 Connect remote-files Phase 2 — read a LOCAL dropped file's bytes
// (base64) so the renderer can POST them to the remote daemon's
// `/cli/fs/upload-binary`. HOST-side exception (the file lives on the
// client's disk; the daemon may be remote).
pub mod local_upload;
