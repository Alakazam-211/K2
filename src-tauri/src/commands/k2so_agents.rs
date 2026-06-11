//! K2SO Agent system — autonomous AI workers operating within workspaces.
//!
//! Agents have a work queue (inbox/active/done) of markdown files,
//! a profile (agent.md), and interact with K2SO via the CLI bridge.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::db::schema::{AgentHeartbeat, WorkspaceSession, HeartbeatFire, WorkspaceRelation};
use crate::fs_atomic::{self, atomic_symlink, atomic_write_str, log_if_err, unique_archive_path};

// Core-hosted helpers + heartbeat fns. Re-imported at crate-local paths
// so the 170+ existing call sites below keep resolving via name-in-scope
// without touching each one. External references via
// `crate::commands::k2so_agents::find_primary_agent` (agent_hooks.rs)
// also continue to work because re-exports behave like normal items.
//
// Phase 2.5c relocations: identity helpers moved to
// `k2_core::workspace::agent_identity`; scheduler moved to
// `k2_core::workspace::scheduler`. Sourced at canonical new paths
// here. (Phase 2.5e retired the former agents module entirely.)
pub use k2_core::workspace::agent_identity::{
    agent_dir, agent_type_for, agents_dir, find_primary_agent, parse_frontmatter,
    resolve_project_id,
};
pub use k2_core::workspace::scheduler::{
    agent_work_dir, count_md_files, get_highest_inbox_priority, get_workspace_state,
    is_agent_locked, is_within_active_hours, k2so_agents_scheduler_tick as core_scheduler_tick,
    priority_label, priority_rank, read_heartbeat_config,
    write_heartbeat_config, ActiveHours, AgentHeartbeatConfig,
};

// ── Types ───────────────────────────────────────────────────────────────

// Phase 2.5c: `WorkItem` lives in `k2_core::workspace::work_item`.
// Re-exported here so external callers (agent_hooks.rs,
// commands/review.rs, etc.) keep resolving
// `crate::commands::k2so_agents::WorkItem`.
pub use k2_core::workspace::work_item::{
    atomic_write as _atomic_write_shim, parse_work_item_content, read_work_item as _read_work_item_shim,
    safe_read_to_string, WorkItem, MAX_FILE_SIZE,
};

// Local aliases for the helpers used at privately-scoped call sites.
#[allow(dead_code)]
fn atomic_write(path: &std::path::Path, content: &str) -> Result<(), String> {
    k2_core::workspace::work_item::atomic_write(path, content)
}
#[allow(dead_code)]
fn read_work_item(path: &std::path::Path, folder: &str) -> Option<WorkItem> {
    k2_core::workspace::work_item::read_work_item(path, folder)
}

// Phase 2.5c: skill + CLAUDE.md content generators relocated to
// `k2_core::skills::content`. Re-exported at historical names.
// `generate_agent_claude_md_content` stays as a public alias; new code
// inside core uses `compose_agent_wake_context` which more honestly
// names what the function returns.
pub use k2_core::skills::content::{
    compose_agent_wake_context, extract_section, format_cap,
    generate_agent_claude_md_content, generate_custom_agent_skill_content,
    generate_k2so_agent_skill_content, generate_manager_skill_content,
    generate_template_skill_content, load_custom_layers, CUSTOM_AGENT_HEARTBEAT_DOCS,
};

// Phase 2.5c: delegation path relocated to
// `k2_core::deprecated::delegate` with `#[deprecated]` annotations
// (CLI verb hard-deprecated, Phase 2.1 PRD A23). The
// `#[tauri::command]` wrapper below is a three-line forward; the four
// frontmatter helpers are re-exported at their historical names so
// the 3 call sites elsewhere in this file resolve unchanged.
#[allow(deprecated)]
pub use k2_core::deprecated::delegate::{
    add_worktree_to_frontmatter, shorten_slug, strip_worktree_from_frontmatter,
    update_assigned_by,
};

// Phase 2.5c: harness-agnostic skill writer relocated to
// `k2_core::skills::writer`. Writes canonical SKILL.md + symlinks
// from every discovery path (.claude/, .opencode/, .pi/) + marker-
// injects into AGENTS.md and copilot-instructions.md.
pub use k2_core::skills::writer::{
    force_symlink, generate_default_agent_body, upsert_k2so_section,
    write_agent_skill_file, write_skill_to_all_harnesses, K2SO_SECTION_BEGIN,
    K2SO_SECTION_END,
};

// Agent CRUD + work queue + workspace inbox + channel events live in
// k2so-core so the daemon can serve the same /cli/* routes headlessly.
// Phase 2.5c relocated `events` to `k2_core::workspace::events`.
// Phase 2.5d split the former commands cluster across
// `workspace::agent` (CRUD + log_agent_warning + cleanup_agent_backups +
// update_agent_md_field + K2soAgentInfo) and `heartbeats::control`
// (ensure_agent_wakeup + per-agent heartbeat control). Phase 2.5e
// retired the `agents/` module entirely.
pub use k2_core::heartbeats::control::ensure_agent_wakeup;
pub use k2_core::workspace::agent::{
    cleanup_agent_backups, log_agent_warning, update_agent_md_field, K2soAgentInfo,
};
pub use k2_core::workspace::events::{
    drain_agent_events, push_agent_event, ChannelEvent, MAX_EVENTS_PER_QUEUE,
};

#[tauri::command]
pub fn k2so_agents_get_heartbeat(
    project_path: String,
    agent_name: String,
) -> Result<AgentHeartbeatConfig, String> {
    k2_core::heartbeats::control::get_heartbeat(project_path, agent_name)
}

#[tauri::command]
pub fn k2so_agents_set_heartbeat(
    project_path: String,
    agent_name: String,
    interval: Option<u64>,
    phase: Option<String>,
    mode: Option<String>,
    cost_budget: Option<String>,
    force_wake: Option<bool>,
) -> Result<AgentHeartbeatConfig, String> {
    k2_core::heartbeats::control::set_heartbeat(
        project_path,
        agent_name,
        interval,
        phase,
        mode,
        cost_budget,
        force_wake,
    )
}

#[tauri::command]
pub fn k2so_agents_heartbeat_noop(
    project_path: String,
    agent_name: String,
) -> Result<AgentHeartbeatConfig, String> {
    k2_core::heartbeats::control::heartbeat_noop(project_path, agent_name)
}

#[tauri::command]
pub fn k2so_agents_heartbeat_action(
    project_path: String,
    agent_name: String,
) -> Result<AgentHeartbeatConfig, String> {
    k2_core::heartbeats::control::heartbeat_action(project_path, agent_name)
}

#[tauri::command]
pub fn k2so_agents_list(project_path: String) -> Result<Vec<K2soAgentInfo>, String> {
    k2_core::workspace::agent::list(project_path)
}

#[tauri::command]
pub fn k2so_agents_create(
    project_path: String,
    name: String,
    role: String,
    prompt: Option<String>,
    agent_type: Option<String>,
) -> Result<K2soAgentInfo, String> {
    k2_core::workspace::agent::create(project_path, name, role, prompt, agent_type)
}

#[tauri::command]
pub fn k2so_agents_delete(project_path: String, name: String) -> Result<(), String> {
    k2_core::workspace::agent::delete(project_path, name)
}

// Plan B cleanup — `k2so_workspace_agent_display_name` +
// `k2so_workspace_set_agent_display_name` deleted. Both routed daemon
// data (`/cli/workspace/{agent-display-name,set-agent-display-name}`);
// the renderer now reaches them host-aware via `daemonCli*`.

// `k2so_agents_delete_inner` shim removed in 0.39.0 — zero callers
// across src-tauri/ and the JS frontend. The implementation in
// `k2_core::workspace::agent::delete_inner` stays; callers that
// need it (currently only k2so-core itself) reach it directly.

// k2so_agents_update_field and k2so_agents_update_profile deleted in
// 0.39.0 cleanup — zero React callers; the per-field/per-profile editor
// surface is unused since the Agent Settings UI moved to AGENT.md edits
// via `k2so_agents_save_agent_md`.

#[tauri::command]
pub fn k2so_agents_get_profile(
    project_path: String,
    agent_name: String,
) -> Result<String, String> {
    k2_core::workspace::agent::get_profile(project_path, agent_name)
}

// Phase 2.1c Item 2 — `k2so_agents_work_list`, `k2so_agents_work_create`,
// `k2so_agents_work_move`, and `k2so_agents_workspace_inbox_list`
// removed. The renderer migrated to the workspace inbox primitive
// (`k2so_inbox_*` in `commands::inbox`), which mirrors the daemon's
// `/cli/inbox/*` HTTP routes. The legacy per-agent `.k2so/agents/<name>/work/`
// surface is itself being retired with the Phase 2.1 1:1 (workspace==agent)
// refactor; new code should use the workspace-level inbox.

// Phase 2.1c Item 2 — `k2so_agents_workspace_inbox_create` removed.
// Zero frontend callers (audit-verified).
//
// Phase 2.1 wrap-up (0.39.0f) — the core function `workspace_inbox_create`
// was also deleted, along with its sole caller (daemon's
// `workspace_msg::deliver_to_inbox`). All inbox-delivery callers now
// use `commands::inbox::k2so_inbox_compose` (renderer) or
// `k2_core::inbox::compose` (Rust), which land in the canonical
// post-Phase-2.1 `.k2so/inbox/` layout.

// ── Path helpers ────────────────────────────────────────────────────────
//
// `agents_dir` + `agent_dir` live in `k2_core::workspace::agent_identity`
// (re-exported above so local call sites resolve unchanged).
// `agent_work_dir` lives in `k2_core::workspace::scheduler` alongside
// the rest of the heartbeat-fire dependency closure.
//
// `workspace_inbox_dir` (legacy `.k2so/work/inbox/`) was deleted in
// 0.39.0f Phase 2.1 final-final. The workspace inbox lives at
// `.k2so/inbox/` and reads flow through `k2_core::inbox::*`.

// ── Wake-up templates ──────────────────────────────────────────────────
//
// Shipped with the binary at compile time. On first app launch (or when
// an agent is created), the matching template is copied to
// `.k2so/agents/<name>/wakeup.md` with its `<!-- DEFAULT TEMPLATE -->`
// header intact so users can see the scaffolded defaults and edit them.
//
// The workspace-level template lives at `.k2so/wakeup.md` for the
// workspace's manager-mode primary agent. Agent-templates (the
// `agent-template` type) are intentionally excluded — they're
// dispatched with explicit orders by their manager and never wake
// autonomously.

// Wakeup templates + resolvers + composers live in
// `k2_core::workspace::wake_prompts`. The re-exports below keep the
// historical paths valid: `WAKEUP_TEMPLATE_*`, `wakeup_template_for`,
// `agent_wakeup_path`, `workspace_wakeup_path`, `read_agent_wakeup`,
// `strip_frontmatter`, the four `compose_*` helpers, and
// `default_heartbeat_wakeup_abs` all resolve to the core versions.
pub use k2_core::workspace::wake_prompts::{
    agent_wakeup_path, compose_agent_wake_from_body, compose_manager_wake_from_body,
    compose_wake_prompt_for_agent, compose_wake_prompt_for_workspace,
    compose_wake_prompt_from_path, default_heartbeat_wakeup_abs, read_agent_wakeup,
    strip_frontmatter, wakeup_template_for, workspace_wakeup_path,
    WAKEUP_TEMPLATE_CUSTOM, WAKEUP_TEMPLATE_K2SO, WAKEUP_TEMPLATE_MANAGER,
    WAKEUP_TEMPLATE_WORKSPACE,
};

/// Find the workspace's primary scheduleable agent. A workspace is one-of
/// Custom / K2SO Agent / Workspace Manager (mutually exclusive by design),
/// but agent-mode swaps can leave orphan directories from prior modes on
/// disk. We use `projects.agent_mode` as the source of truth and only
/// return an agent dir whose type matches the workspace's declared mode.
/// Agent-templates are never scheduleable and are always skipped.

/// Multi-heartbeat architecture: CRUD for agent_heartbeats table.
/// See .k2so/prds/multi-schedule-heartbeat.md.

// All heartbeat business logic lives in `k2_core::heartbeats`.
// The `#[tauri::command]` wrappers below are thin forwards so the
// React UI's existing `invoke("k2so_heartbeat_*")` calls keep working;
// the daemon calls the core fns directly from its `/cli/heartbeat/*`
// HTTP routes so scheduled wakes fire while the Tauri app is quit.

#[tauri::command]
pub fn k2so_heartbeat_add(
    project_path: String,
    name: String,
    frequency: String,
    spec_json: String,
) -> Result<serde_json::Value, String> {
    k2_core::heartbeats::k2so_heartbeat_add(project_path, name, frequency, spec_json)
}

#[tauri::command]
pub fn k2so_heartbeat_list(project_path: String) -> Result<Vec<AgentHeartbeat>, String> {
    k2_core::heartbeats::k2so_heartbeat_list(project_path)
}

/// 0.38.3 — most recent heartbeat fire records across ALL projects,
/// joined with project name. Powers the universal audit log on the
/// system-wide Heartbeats settings page (`WakeSchedulerSection`).
/// Default limit 100 fires; bump for deeper investigation.
#[tauri::command]
pub fn k2so_heartbeat_fires_list_all(
    limit: Option<i64>,
) -> Result<Vec<serde_json::Value>, String> {
    k2_core::heartbeats::k2so_heartbeat_fires_list_all(limit)
}

/// 0.38.3 — list every active (non-archived) heartbeat across ALL
/// workspaces, with the parent project's name + path joined in. Used
/// by the system-wide Heartbeats settings page (`WakeSchedulerSection`)
/// so the operator can see and toggle every heartbeat from one place.
#[tauri::command]
pub fn k2so_heartbeat_list_all() -> Result<Vec<serde_json::Value>, String> {
    k2_core::heartbeats::k2so_heartbeat_list_all()
}

#[tauri::command]
pub fn k2so_heartbeat_list_archived(
    project_path: String,
) -> Result<Vec<AgentHeartbeat>, String> {
    k2_core::heartbeats::k2so_heartbeat_list_archived(project_path)
}

#[tauri::command]
pub fn k2so_heartbeat_archive(
    project_path: String,
    name: String,
) -> Result<(), String> {
    k2_core::heartbeats::k2so_heartbeat_archive(project_path, name)
}

#[tauri::command]
pub fn k2so_heartbeat_unarchive(
    project_path: String,
    name: String,
) -> Result<(), String> {
    k2_core::heartbeats::k2so_heartbeat_unarchive(project_path, name)
}

#[tauri::command]
pub fn k2so_heartbeat_remove(project_path: String, name: String) -> Result<(), String> {
    k2_core::heartbeats::k2so_heartbeat_remove(project_path, name)
}

/// Read the workspace's `show_heartbeat_sessions` flag.
///
/// 0 (default) = silent autonomous mode; heartbeat fires never open
/// tabs. Audit via the sidebar Heartbeats panel on demand.
/// 1 = each scheduled heartbeat fire opens a background tab in the
/// Tauri window. Tab persists until the user closes it.
#[tauri::command]
pub fn k2so_workspace_get_show_heartbeat_sessions(
    project_path: String,
) -> Result<bool, String> {
    k2_core::heartbeats::k2so_workspace_get_show_heartbeat_sessions(project_path)
}

/// Flip the workspace's `show_heartbeat_sessions` flag.
#[tauri::command]
pub fn k2so_workspace_set_show_heartbeat_sessions(
    project_path: String,
    enabled: bool,
) -> Result<(), String> {
    k2_core::heartbeats::k2so_workspace_set_show_heartbeat_sessions(
        project_path,
        enabled,
    )
}

#[tauri::command]
pub fn k2so_heartbeat_set_enabled(
    project_path: String,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    k2_core::heartbeats::k2so_heartbeat_set_enabled(project_path, name, enabled)
}

/// 0.37.8 — flip the per-heartbeat opt-in to deliver WAKEUP.md into
/// the workspace's pinned chat session via
/// `workspace_msg::deliver_live`. See migration 0043.
#[tauri::command]
pub fn k2so_heartbeat_set_use_workspace_session(
    project_path: String,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    k2_core::heartbeats::k2so_heartbeat_set_use_workspace_session(
        project_path,
        name,
        enabled,
    )
}

#[tauri::command]
pub fn k2so_heartbeat_edit(
    project_path: String,
    name: String,
    frequency: String,
    spec_json: String,
) -> Result<(), String> {
    k2_core::heartbeats::k2so_heartbeat_edit(project_path, name, frequency, spec_json)
}

// Re-exported so the name stays reachable at its historical path
// (`crate::commands::k2so_agents::HeartbeatFireCandidate`) while the
// struct itself lives in k2so-core.
pub use k2_core::heartbeats::HeartbeatFireCandidate;

// `k2so_agents_heartbeat_tick` + `stamp_heartbeat_fired` shims removed
// in 0.39.0 — both had zero callers in src-tauri/ + frontend. The
// daemon's heartbeat loop calls `k2_core::heartbeats::*` directly.

#[tauri::command]
pub fn k2so_heartbeat_rename(
    project_path: String,
    old_name: String,
    new_name: String,
) -> Result<(), String> {
    k2_core::heartbeats::k2so_heartbeat_rename(project_path, old_name, new_name)
}

#[tauri::command]
pub fn k2so_heartbeat_fires_list(
    project_path: String,
    limit: Option<i64>,
) -> Result<Vec<HeartbeatFire>, String> {
    k2_core::heartbeats::k2so_heartbeat_fires_list(project_path, limit)
}

// Migration-helper shims removed in 0.39.0 (zero callers across
// src-tauri/ + frontend; the daemon's boot sweep at
// `run_workspace_legacy_migrations_sweep` calls
// `k2_core::workspace::migrations::*` directly):
//
//   - archive_orphan_top_tier_agents
//   - repair_mismigrated_heartbeats
//   - promote_legacy_heartbeat
//   - ensure_workspace_wakeups
//   - migrate_filenames_to_uppercase
//   - migrate_or_scaffold_lead_heartbeat
//
// The underlying implementations live in
// `crates/k2so-core/src/workspace/migrations.rs` and are invoked by
// the daemon on every boot.

// ── Frontmatter parsing ────────────────────────────────────────────────

// ── Skill upgrade protocol (universal) ───────────────────────────────
// The full skill lifecycle (markers, versions, wrap/parse, the
// ensure_skill_up_to_date writer) moved to k2_core::skills::version.
// src-tauri re-exports the surface at its historical names so the 30+
// call sites in this file resolve unchanged.
pub use k2_core::skills::version::{
    ensure_skill_up_to_date, parse_skill, skill_checksum_hex,
    skill_source_agent_md_begin, skill_source_agent_md_end, wrap_managed_skill,
    ParsedSkill, SkillUpgradeOutcome, SKILL_BEGIN_MARKER, SKILL_END_MARKER,
    SKILL_SOURCE_PROJECT_MD_BEGIN, SKILL_SOURCE_PROJECT_MD_END,
    SKILL_VERSION_CUSTOM_AGENT, SKILL_VERSION_K2SO_AGENT, SKILL_VERSION_MANAGER,
    SKILL_VERSION_TEMPLATE, SKILL_VERSION_WORKSPACE,
};

// Legacy shim — fn definitions below deleted. Original ParsedSkill
// impl block used `struct ParsedSkill { k2so_skill: ... }` with a
// private constructor; the core version makes all fields pub so the
// in-file call sites that directly destructure it still work.

// ── Heartbeat Configuration ─────────────────────────────────────────────
//
// `AgentHeartbeatConfig`, `ActiveHours`, `read_heartbeat_config`,
// `write_heartbeat_config`, and the per-field default fns all live in
// `k2_core::workspace::scheduler`. The types + functions are
// re-exported at the top of this file so existing call sites resolve
// unchanged.

// ── Tauri Commands ──────────────────────────────────────────────────────

/// Delegate a work item to an agent — creates a worktree,
/// registers it, moves the item to active, writes CLAUDE.md.
/// Body lives in `k2_core::deprecated::delegate`.
#[tauri::command]
pub fn k2so_agents_delegate(
    project_path: String,
    target_agent: String,
    source_file: String,
) -> Result<serde_json::Value, String> {
    k2_core::deprecated::delegate::k2so_agents_delegate(project_path, target_agent, source_file)
}

// ── Lock Files ──────────────────────────────────────────────────────────

// Session lifecycle (lock / unlock / save-session-id / clear-session-id)
// lives in `k2_core::workspace::session`. These #[tauri::command]
// wrappers are thin forwards so the React frontend's existing invokes
// keep working unchanged; the daemon calls the core fns directly from
// its wake path.

#[tauri::command]
pub fn k2so_agents_lock(
    project_path: String,
    agent_name: String,
    terminal_id: Option<String>,
    owner: Option<String>,
) -> Result<(), String> {
    k2_core::workspace::session::k2so_agents_lock(project_path, agent_name, terminal_id, owner)
}

// k2so_agents_unlock deleted in 0.39.0 cleanup — zero React callers
// and unpaired with any `k2so_agents_lock` invocation that would expect
// a matching release. The unlock path is now driven entirely by the
// daemon's session-end hooks (workspace_sessions row delete).

// ── Agent context / SKILL.md regen ─────────────────────────────────────
//
// 0.39.0 cleanup: `k2so_agents_regenerate_agent_context` +
// `k2so_agents_generate_claude_md` (back-compat alias) +
// `k2so_agents_regenerate_claude_md` (back-compat alias) deleted —
// zero React/CLI/test callers. SKILL/CLAUDE.md regen runs implicitly
// from wake builders (`k2so_agents_build_launch`) so the on-demand
// command surface is unused. Core `k2_core::workspace::agent_editor::
// k2so_agents_regenerate_agent_context` stays for in-process callers.

/// Full-fat wake-launch builder (UI "Launch" button +
/// heartbeat auto-launch). Body lives in
/// `k2_core::workspace::agent_launch`; this Tauri wrapper is a
/// thin forward so the React frontend's invoke keeps
/// working.
// `heartbeat_name`: heartbeat-scoped resume target (post-0.36.0). When
// `Some`, the build prefers `agent_heartbeats.last_session_id` for the
// row before falling back to the per-agent global session. React
// callers (Chat tab, manual agent launches) leave this `None`.
#[tauri::command]
pub fn k2so_agents_build_launch(
    project_path: String,
    agent_name: String,
    agent_cli_command: Option<String>,
    wakeup_override: Option<String>,
    skip_fork_session: Option<bool>,
    heartbeat_name: Option<String>,
) -> Result<serde_json::Value, String> {
    k2_core::workspace::agent_launch::k2so_agents_build_launch(
        project_path,
        agent_name,
        agent_cli_command,
        wakeup_override,
        skip_fork_session,
        heartbeat_name,
    )
}

// Plan B cleanup — `k2so_agents_resume_chat_args` deleted. It routed
// daemon data (`/cli/workspace/resume-chat-args`); the renderer now
// reaches it host-aware via `daemonCli*`. The shared resolver lives in
// `k2_core::workspace::resume_chat::resolve_resume_chat_args`.

/// Regenerate the workspace-root SKILL.md — the lead agent's complete
/// operating manual. Written to `<project-root>/SKILL.md` with a
/// matching `<project-root>/CLAUDE.md` symlink so Claude Code auto-
/// discovers it. The SKILL.md is the canonical source of truth;
/// CLAUDE.md is a harness-specific entry point.
///
/// Also auto-scaffolds the `.k2so/` layout on first call (manager +
/// k2so-agent dirs, inbox/active/done folders, prds/, PROJECT.md).
///
/// Pre-0.33.0 this was `k2so_agents_generate_workspace_claude_md` —
/// back-compat alias below.
/// Body lives in `k2_core::workspace::skill_regen::regenerate_workspace_skill`
/// (renamed from `skill_writer` in 0.39.0)
/// (Phase 2 Unit 7d). The Tauri wrapper stays for the invoke handler.
#[tauri::command]
pub fn k2so_agents_regenerate_workspace_skill(
    project_path: String,
) -> Result<String, String> {
    k2_core::workspace::skill_regen::regenerate_workspace_skill(project_path)
}

/// Back-compat alias for [`k2so_agents_regenerate_workspace_skill`].
/// Pre-0.33.0 name. Kept so existing Rust callers (and any React
/// `invoke('k2so_agents_generate_workspace_claude_md')` sites not yet
/// updated) keep working during the rename window.
#[tauri::command]
pub fn k2so_agents_generate_workspace_claude_md(
    project_path: String,
) -> Result<String, String> {
    k2so_agents_regenerate_workspace_skill(project_path)
}

// ── Onboarding (workspace-add three-option flow) ───────────────────
//
// Thin wrappers around `k2_core::workspace::onboarding`. Logic lives
// in core so the CLI (`k2so onboarding ...`) and Tauri share the same
// implementation; the renderer's WorkspaceOnboardingModal only displays
// scan results and forwards button-clicks to these commands.

/// Scan the workspace for harness files (CLAUDE.md, GEMINI.md,
/// .cursor/rules, .goosehints, etc.) with substantive user content.
/// Used by the onboarding modal to decide whether to prompt the user
/// at all (empty result → silently take the "Start Fresh" path) and
/// what to show in the adopt-picker.
#[tauri::command]
pub fn k2so_onboarding_scan(
    project_path: String,
) -> Vec<k2_core::workspace::onboarding::DetectedHarnessFile> {
    k2_core::workspace::onboarding::scan_harness_files(&project_path)
}

// `k2so_onboarding_adopt` / `k2so_onboarding_skip` removed with the
// canonical-agents PRD (§7 Removal Catalog). With harness fan-out off by
// default, the Skip-before-regen flag is meaningless and the single-file
// "adopt into PROJECT.md" is replaced by the K2 Canonical Agent's richer
// organic MERGE step (which weaves harness content into Model A's
// AGENT.md/PROJECT.md, then mirrors out). Core helpers
// (`skip_harness_management` / `adopt_harness_as_project_md`) are kept for
// the daemon CLI routes and as a legacy alias; only the Tauri wrappers and
// their consent-page caller are gone.

/// User-facing "Start Fresh" option. No special logic — just runs
/// the regen pipeline, which already archives any pre-existing
/// harness files to `.k2so/migration/` and replaces them with
/// symlinks. Exposed as its own command so the renderer doesn't
/// have to know that "Start Fresh" is just-the-default — the three
/// options each have a symmetric Tauri entry point.
#[tauri::command]
pub fn k2so_onboarding_start_fresh(project_path: String) -> Result<(), String> {
    // Make sure any prior "skip" flag from a re-onboarding flow is
    // cleared so the regen actually performs the harness fanout.
    k2_core::workspace::onboarding::unskip_harness_management(&project_path)?;
    k2so_agents_regenerate_workspace_skill(project_path).map(|_| ())
}

// ── Canonical Agent Flow (canonical-agents PRD §4 / §5.2 / §11) ──────────
//
// Thin wrappers around the canonical-agents core (`k2_core::workspace`).
// Logic lives in core so the daemon CLI and Tauri share one implementation
// (matching the existing onboarding-toggle pattern above — direct
// Tauri→core, no extra daemon round-trip). The renderer's Settings
// permission checkbox + the Agent-section button labels consume these.

/// Read the per-workspace harness fan-out permission marker
/// (`harness_fanout_enabled`, PRD §4). Off by default; legacy
/// `.skip-harness-management` forces it false. Drives the Settings
/// checkbox state.
#[tauri::command]
pub fn k2so_harness_fanout_enabled(project_path: String) -> bool {
    k2_core::workspace::onboarding::harness_fanout_enabled(&project_path)
}

/// Set the per-workspace harness fan-out permission marker (PRD §4).
/// Checking it enables ongoing programmatic fan-out (symlinks) derived
/// from `.k2so/agent/AGENT.md`; unchecking it stops new fan-out.
#[tauri::command]
pub fn k2so_set_harness_fanout_enabled(
    project_path: String,
    enabled: bool,
) -> Result<(), String> {
    k2_core::workspace::onboarding::set_harness_fanout_enabled(&project_path, enabled)
}

/// Probe per-harness canonical state WITHOUT mutating anything
/// (`detect_canonical_state`, PRD §5.2). Feeds the per-harness state
/// display in the Canonical Agent Flow settings section + the canonical
/// button label ("Set up …" vs "Manage / Undo").
#[tauri::command]
pub fn k2so_detect_canonical_state(
    project_path: String,
) -> Vec<k2_core::workspace::canonical::HarnessProbe> {
    k2_core::workspace::canonical::detect_canonical_state(&project_path)
}

/// Write one of the three opt-in skills to `.k2so/skills/<name>/SKILL.md`
/// on demand (PRD §8.1 "Enable"). `skill` is one of
/// `workspace-manager` | `k2-agent` | `k2-canonical-agent` (the
/// [`k2_core::skills::content::OptInSkill::dir_name`] values). Returns
/// the absolute path written so the UI can confirm it landed.
#[tauri::command]
pub fn k2so_write_opt_in_skill(
    project_path: String,
    skill: String,
) -> Result<String, String> {
    let opt_in = match skill.as_str() {
        "workspace-manager" => k2_core::skills::content::OptInSkill::WorkspaceManager,
        "k2-agent" => k2_core::skills::content::OptInSkill::K2Agent,
        "k2-canonical-agent" => k2_core::skills::content::OptInSkill::K2CanonicalAgent,
        other => return Err(format!("unknown opt-in skill: {other}")),
    };
    let path = k2_core::skills::content::write_opt_in_skill(&project_path, opt_in);
    Ok(path.to_string_lossy().to_string())
}

/// Remove or disable the workspace SKILL.md + CLAUDE.md symlink
/// (when the Agent toggle is turned off). Body lives in
/// `k2_core::workspace::harness::disable_workspace_claude_md`
/// (Phase 2 Unit 7d).
#[tauri::command]
pub fn k2so_agents_disable_workspace_claude_md(project_path: String) -> Result<(), String> {
    k2_core::workspace::harness::disable_workspace_claude_md(project_path)
}

// `CLI_TOOLS_DOCS` removed in 0.39.0. Was a stale duplicate of the
// core version (k2_core::workspace::skill_regen) with zero callers
// in this crate (Phase 2.1 final audit). The core version is the
// authoritative source consumed by the SKILL.md generator and uses
// the Phase 2.1 A25 canonical verbs. Mirrors the parallel removal of
// `WORKFLOW_DOCS` from this same file.

// `WORKFLOW_DOCS` removed in 0.39.0f. The constant was a stale duplicate
// of the core version (now in `k2_core::workspace::skill_regen`); it
// had zero callers in this crate (Phase 2.1 final audit). The core
// version is the authoritative source consumed by the SKILL.md generator.

// (duplicate of k2_core helpers — removed during skill_content migration)

// ── Review Queue ────────────────────────────────────────────────────────
// Core types + logic live in `k2_core::workspace::reviews`. We re-export
// the types so the Tauri command signatures below (and any callers that
// imported from this module) keep their shapes.

pub use k2_core::workspace::reviews::{ReviewDiffFile, ReviewItem};

/// Get the review queue — agents with completed work in worktree branches.
#[tauri::command]
pub async fn k2so_agents_review_queue(project_path: String) -> Result<Vec<ReviewItem>, String> {
    tokio::task::spawn_blocking(move || k2_core::workspace::reviews::review_queue(&project_path))
        .await
        .map_err(|e| format!("review_queue task failed: {}", e))?
}

pub fn k2so_agents_review_queue_inner(project_path: &str) -> Result<Vec<ReviewItem>, String> {
    k2_core::workspace::reviews::review_queue(project_path)
}

/// Sub-agent completion. Core logic in
/// `k2_core::workspace::reviews::agent_complete`.
pub fn k2so_agent_complete(
    project_path: String,
    agent_name: String,
    filename: String,
) -> Result<String, String> {
    k2_core::workspace::reviews::agent_complete(project_path, agent_name, filename)
}

/// Approve the agent's branch — merge + cleanup. Core logic lives in
/// `k2_core::workspace::reviews::review_approve`.
#[tauri::command]
pub fn k2so_agents_review_approve(
    project_path: String,
    branch: String,
    agent_name: String,
) -> Result<String, String> {
    k2_core::workspace::reviews::review_approve(project_path, branch, agent_name)
}

/// Reject the agent's work — clean up worktree, restore inbox, write
/// optional feedback. Core logic lives in
/// `k2_core::workspace::reviews::review_reject`.
#[tauri::command]
pub fn k2so_agents_review_reject(
    project_path: String,
    agent_name: String,
    reason: Option<String>,
) -> Result<(), String> {
    k2_core::workspace::reviews::review_reject(project_path, agent_name, reason)
}

/// Request changes — drop a feedback file in inbox, don't tear down
/// the worktree. Core logic in `k2_core::workspace::reviews::review_request_changes`.
#[tauri::command]
pub fn k2so_agents_review_request_changes(
    project_path: String,
    agent_name: String,
    feedback: String,
) -> Result<(), String> {
    k2_core::workspace::reviews::review_request_changes(project_path, agent_name, feedback)
}

// ── Heartbeat Triage (Workspace State) ──────────────────────────────────

// k2so_agents_triage_summary deleted in 0.39.0 cleanup — zero React
// callers; the triage summary is built and consumed entirely inside
// the daemon's heartbeat fire path via `k2_core::workspace::triage`.

/// Determine what should be launched based on triage.
///
/// Agents are templates — the same agent (e.g., "backend-eng") can run in multiple
/// worktrees simultaneously. Each inbox item gets its own worktree when delegated.
///
/// Triage order:
/// 1. Workspace inbox has items → wake the workspace's primary agent
///    (resolved via `find_primary_agent`).
/// 2. Sub-agent inboxes have items → wake those agents (one launch per inbox item)
///
/// **DEPRECATED — `legacy-per-agent-heartbeat` chokepoint.**
/// Pre-0.30s K2SO used inbox contents to decide whether to autonomously
/// wake an agent. The new model lives in the `agent_heartbeats` table
/// (workspace-scoped, explicit schedules). This function is being kept
/// alive only for the launch-failure-retry path in active-agents.ts;
/// gated on `projects.heartbeat_mode != 'off'` so opted-out workspaces
/// don't get auto-launched even if a stray caller invokes us. Planned
/// for removal in 0.37.x.
#[deprecated(
    note = "Inbox-driven triage — superseded by agent_heartbeats.             Planned for removal in 0.37.x. See `legacy-per-agent-heartbeat` tag."
)]
#[tauri::command]
#[allow(deprecated)]
pub fn k2so_agents_triage_decide(project_path: String) -> Result<Vec<String>, String> {
    k2_core::workspace::triage::triage_decide(&project_path)
}


// ── Adaptive Heartbeat Commands ──────────────────────────────────────────

/// Scheduler tick: check all agents in a project and return those ready to wake.
/// Called by the heartbeat script (via /cli/scheduler-tick).
/// Differentiates between manager agents (inbox-based) and custom agents (timing-based).
#[tauri::command]
pub fn k2so_agents_scheduler_tick(project_path: String) -> Result<Vec<String>, String> {
    let _h = crate::perf_hist!("scheduler_tick");
    core_scheduler_tick(project_path)
}

/// Save the last Claude session ID for an agent (enables --resume on next launch).
/// Stores the session ID in the DB (agent_sessions.session_id).
/// This is the single source of truth — the legacy `.last_session`
/// file was retired, as it was being deleted by the no-op pruner
/// without touching the DB, leading to drift and failed resumes.
#[tauri::command]
pub fn k2so_agents_save_session_id(
    project_path: String,
    agent_name: String,
    session_id: String,
) -> Result<(), String> {
    k2_core::workspace::session::k2so_agents_save_session_id(
        project_path,
        agent_name,
        session_id,
    )
}

#[tauri::command]
pub fn k2so_agents_clear_session_id(
    project_path: String,
    agent_name: String,
) -> Result<(), String> {
    k2_core::workspace::session::k2so_agents_clear_session_id(project_path, agent_name)
}

/// Toggle the per-session `surfaced` flag. When transitioning 0 → 1,
/// emits `HookEvent::SessionSurfaced` so the renderer creates a tab
/// that ATTACHES to the existing PTY (no fresh spawn). When
/// transitioning 1 → 0, the renderer is expected to remove the tab
/// without killing the PTY (the heartbeat session keeps running in
/// the background). See `.k2so/prds/heartbeat-active-session-tracking.md`.
///
/// `terminal_id`, `command`, `args`, `heartbeat_name` are forwarded
/// in the surfaced-event payload so the renderer can construct a tab
/// without re-querying — kept minimal because the event listener is
/// a hot path. Pass empty strings / empty Vec / None when not
/// applicable.
/// Body lives in `k2_core::workspace::session::k2so_session_set_surfaced`
/// (Phase 2 Unit 7d).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn k2so_session_set_surfaced(
    project_path: String,
    agent_name: String,
    surfaced: bool,
    terminal_id: Option<String>,
    command: Option<String>,
    args: Option<Vec<String>>,
    heartbeat_name: Option<String>,
    attach_agent_name: Option<String>,
) -> Result<(), String> {
    k2_core::workspace::session::k2so_session_set_surfaced(
        project_path,
        agent_name,
        surfaced,
        terminal_id,
        command,
        args,
        heartbeat_name,
        attach_agent_name,
    )
}

/// 0.38.0 commit 7 — broadcast cross-window when the pinned-chat
/// refresh button is clicked. Body lives in
/// `k2_core::workspace::session::k2so_chat_refresh_broadcast`
/// (Phase 2 Unit 7d).
#[tauri::command]
pub fn k2so_chat_refresh_broadcast(project_path: String) -> Result<(), String> {
    k2_core::workspace::session::k2so_chat_refresh_broadcast(project_path)
}

// ── Project-Level Schedule Evaluation ─────────────────────────────────────
//
// `should_project_fire` + `matches_ordinal_day` now live in
// k2_core::scheduler so the daemon can call them directly without
// pulling in this commands module. Re-imported for the three
// unqualified call sites elsewhere in this file.
use k2_core::scheduler::should_project_fire;


/// Compute the next N fire times for a schedule (for UI preview).
#[tauri::command]
pub fn k2so_agents_preview_schedule(
    mode: String,
    schedule_json: String,
    count: u32,
) -> Result<Vec<String>, String> {
    let mut results = Vec::new();
    let mut cursor = chrono::Local::now();

    // Step forward in 1-minute increments, checking up to 366 days ahead
    let max_steps = 366 * 24 * 60; // 1 year of minutes
    let mut steps = 0u64;

    while results.len() < count as usize && steps < max_steps {
        if should_project_fire(&mode, Some(&schedule_json), Some(&cursor.to_rfc3339())) {
            // This would fire — but we need to check if it's a NEW fire, not a repeat
            // For scheduled mode, each matching day/time is one fire
            results.push(cursor.format("%Y-%m-%d %H:%M").to_string());
            // Skip ahead past this fire window
            if mode == "hourly" {
                let v: serde_json::Value = serde_json::from_str(&schedule_json).unwrap_or_default();
                let every = v.get("every_seconds").and_then(|s| s.as_u64()).unwrap_or(300);
                cursor = cursor + chrono::Duration::seconds(every as i64);
                steps += every / 60;
                continue;
            } else {
                // Skip to next day for scheduled mode
                cursor = cursor + chrono::Duration::days(1);
                // Reset to start of day
                let next_date = cursor.date_naive();
                if let Some(dt) = next_date.and_hms_opt(0, 0, 0) {
                    use chrono::TimeZone;
                    if let Some(local_dt) = chrono::Local.from_local_datetime(&dt).single() {
                        cursor = local_dt;
                    }
                }
                steps += 24 * 60;
                continue;
            }
        }
        cursor = cursor + chrono::Duration::minutes(1);
        steps += 1;
    }

    Ok(results)
}

// ── Heartbeat Scheduler — Phase 2 Unit 7c ───────────────────────────────
//
// Pre-Unit-7c these commands wrote `~/.k2so/heartbeat.sh` + installed
// the launchd plist (macOS) / crontab entry (Linux) directly from
// Tauri. Unit 7c moved the install/uninstall body to
// `k2_core::heartbeats::install` and added daemon routes at
// `/cli/heartbeat/{install-launchd, uninstall-launchd,
// apply-wake-scheduler}`. The wrappers below are thin daemon-HTTP
// proxies so K2SO Connect (remote daemon, no Tauri) can install its
// own scheduler under its own GUI session and the renderer keeps
// using the same `invoke('k2so_agents_*_heartbeat')` shape.

/// Install the heartbeat scheduler via the daemon. The daemon reads
/// the current `wake_scheduler` settings, generates `heartbeat.sh`,
/// and loads the plist (macOS) or installs the crontab entry (Linux).
#[tauri::command]
pub fn k2so_agents_install_heartbeat() -> Result<(), String> {
    let client = crate::daemon_client::DaemonClient::try_connect()?;
    let settings = k2_core::app_settings::load();
    let cfg = &settings.wake_scheduler;
    let interval_secs = cfg.interval_minutes.max(1) as u32 * 60;
    let body = serde_json::json!({
        "interval_seconds": interval_secs,
        "wake_system": cfg.wake_system,
    });
    let _ = client.cli_post_json("/cli/heartbeat/install-launchd", &body)?;
    Ok(())
}

/// Apply the user's Wake Scheduler settings — daemon decides between
/// install / uninstall based on `mode`. See `heartbeat_launchd_routes`
/// in the daemon for the routing.
#[tauri::command]
pub fn k2so_agents_apply_wake_scheduler() -> Result<String, String> {
    let client = crate::daemon_client::DaemonClient::try_connect()?;
    let settings = k2_core::app_settings::load();
    let cfg = &settings.wake_scheduler;
    let body = serde_json::json!({
        "mode": cfg.mode,
        "interval_minutes": cfg.interval_minutes,
        "wake_system": cfg.wake_system,
    });
    let resp = client.cli_post_json("/cli/heartbeat/apply-wake-scheduler", &body)?;
    let v: serde_json::Value = serde_json::from_str(&resp)
        .map_err(|e| format!("decode apply-wake-scheduler: {e}"))?;
    Ok(v.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string())
}

/// Uninstall the heartbeat scheduler via the daemon.
#[tauri::command]
pub fn k2so_agents_uninstall_heartbeat() -> Result<(), String> {
    let client = crate::daemon_client::DaemonClient::try_connect()?;
    let _ = client.cli_post_json(
        "/cli/heartbeat/uninstall-launchd",
        &serde_json::json!({}),
    )?;
    Ok(())
}

/// Refresh the heartbeat-scheduler lifecycle when the toggle changes.
/// Same idempotent re-apply as `k2so_agents_apply_wake_scheduler` —
/// the daemon's `apply-wake-scheduler` route handles the install /
/// uninstall transition uniformly. Pre-Unit-7c this also wrote the
/// retired `heartbeat-projects.txt`; that file was killed in P5.6
/// when `/cli/heartbeat/active-projects` started serving the live
/// project list from `agent_heartbeats`.
#[tauri::command]
pub fn k2so_agents_update_heartbeat_projects() -> Result<(), String> {
    let _ = k2so_agents_apply_wake_scheduler();
    Ok(())
}

// ── Agent Editor ───────────────────────────────────────────────────────

/// Get full context needed for the AIFileEditor agent editing session.
/// Body lives in `k2_core::workspace::agent_editor::k2so_agents_get_editor_context`
/// (Phase 2 Unit 7d).
#[tauri::command]
pub fn k2so_agents_get_editor_context(
    project_path: String,
    agent_name: String,
) -> Result<serde_json::Value, String> {
    k2_core::workspace::agent_editor::k2so_agents_get_editor_context(project_path, agent_name)
}

// 0.39.0 cleanup: `k2so_agents_preview_agent_context` + the
// `k2so_agents_preview_claude_md` back-compat alias +
// `k2so_agents_regenerate_claude_md` back-compat alias deleted —
// zero React/CLI/test callers. The on-disk preview is reconstructed
// on demand by `k2so_agents_get_editor_context` (which the editor
// actually uses). Core
// `k2_core::workspace::agent_editor::k2so_agents_preview_agent_context`
// stays available for in-process callers.

/// Save an agent's agent.md file, creating a timestamped backup of the
/// previous version. Body lives in
/// `k2_core::workspace::agent_editor::k2so_agents_save_agent_md`
/// (Phase 2 Unit 7d).
#[tauri::command]
pub fn k2so_agents_save_agent_md(
    project_path: String,
    agent_name: String,
    content: String,
) -> Result<(), String> {
    k2_core::workspace::agent_editor::k2so_agents_save_agent_md(project_path, agent_name, content)
}

// ── Workspace Session (DB-tracked) ───────────────────────────────────────

/// Body lives in `k2_core::workspace::relations::workspace_session_get`
/// (Phase 2 Unit 7d). Phase 2 close-out dropped the `_state` parameter
/// alongside the deletion of `AppState`; the renderer's `invoke()`
/// payload never set it, so this is binary-compatible.
#[tauri::command]
pub fn workspace_session_get(
    project_id: String,
) -> Result<Option<WorkspaceSession>, String> {
    k2_core::workspace::relations::workspace_session_get(project_id)
}

/// 0.37.12 — explicitly set the pinned chat tab's Claude session id
/// for a workspace. Thin facade over the daemon HTTP route
/// `/cli/workspace/set-chat-session` — keeps Tauri as the thin
/// client, daemon owns the write. Used by `AgentChatPane`'s
/// chat-history dropdown when the user picks a different chat from
/// the history list (escape hatch for orphaned or deleted sessions).
///
/// Caller is expected to follow up with a pinned-chat refresh
/// (`closeV2Session(projectId)` + re-mount AgentChatPane) so the
/// live PTY swaps to the new session.
#[tauri::command]
pub fn workspace_session_set_session_id(
    project_path: String,
    session_id: String,
) -> Result<String, String> {
    let client = crate::daemon_client::DaemonClient::try_connect()?;
    client.cli_get(
        "/cli/workspace/set-chat-session",
        &[("project", &project_path), ("session_id", &session_id)],
    )
}

// ── Workspace Relations ─────────────────────────────────────────────────

/// Bodies live in `k2_core::workspace::relations::workspace_relations_*`
/// (Phase 2 Unit 7d). Phase 2 close-out dropped the `_state` arg
/// alongside the deletion of `AppState`.
#[tauri::command]
pub fn workspace_relations_list(
    project_id: String,
) -> Result<Vec<WorkspaceRelation>, String> {
    k2_core::workspace::relations::workspace_relations_list(project_id)
}

#[tauri::command]
pub fn workspace_relations_list_incoming(
    project_id: String,
) -> Result<Vec<WorkspaceRelation>, String> {
    k2_core::workspace::relations::workspace_relations_list_incoming(project_id)
}

#[tauri::command]
pub fn workspace_relations_create(
    source_project_id: String,
    target_project_id: String,
    relation_type: Option<String>,
) -> Result<WorkspaceRelation, String> {
    k2_core::workspace::relations::workspace_relations_create(
        source_project_id,
        target_project_id,
        relation_type,
    )
}

#[tauri::command]
pub fn workspace_relations_delete(id: String) -> Result<(), String> {
    k2_core::workspace::relations::workspace_relations_delete(id)
}

// ── Skill File Generation ────────────────────────────────────────────

/// Regenerate SKILL.md files for all agents in a workspace.
/// Called on app startup (migration) and via CLI `k2so skills regenerate`.
/// Core logic lives in `k2_core::skills::crud::regenerate_skills`
/// (relocated during Phase 2.5d).
#[tauri::command]
pub fn k2so_agents_regenerate_skills(
    project_path: String,
) -> Result<serde_json::Value, String> {
    k2_core::skills::crud::regenerate_skills(project_path)
}

/// Write the canonical SKILL.md and symlink from all harness discovery paths.
/// One source of truth — symlinks mean updates propagate instantly.
///
/// Canonical location: .k2so/skills/{name}/SKILL.md
/// Symlinked to: Claude Code, OpenCode, Pi, Cursor (project root)
/// Marker-injected into: AGENTS.md, .github/copilot-instructions.md
// `write_shared_markers`: only the workspace-level skill should set this
// true — per-agent skills would otherwise clobber each other in the
// single K2SO marker block inside AGENTS.md / copilot-instructions.md.

/// Write the workspace-level K2SO skill to all harness locations.
/// Composes the full workspace context into a single canonical file
/// that every CLI LLM discovers via its harness-specific path:
///
///   - Base body (rich workspace manager / AI planner brief if the
///     CLAUDE.md generator passes one; otherwise the lightweight
///     `generate_workspace_skill_content` — user-facing CLI commands)
///   - `.k2so/PROJECT.md` body (if the user has populated it)
///   - Primary agent's `agent.md` body (for single-agent and manager modes)
///
/// The canonical file at `.k2so/skills/k2so/SKILL.md` is then symlinked
/// into every harness discovery path. `./CLAUDE.md` joins that list as
/// of 0.32.7, replacing the separately-generated workspace CLAUDE.md.
pub fn write_workspace_skill_file(project_path: &str) {
    k2_core::workspace::skill_regen::write_workspace_skill_file(project_path)
}

/// Variant that lets callers pass a pre-composed body (typically the
/// rich workspace CLAUDE.md content from `k2so_agents_generate_workspace_claude_md`)
/// so that content lands in the canonical SKILL.md rather than being
/// lost when CLAUDE.md collapsed to a symlink.
///
/// Sequence (Phase 7c):
///   1. Adoption sweep — parse existing canonical SKILL.md SOURCE sub-regions;
///      commit drift back to PROJECT.md / primary agent AGENT.md (mtime-guarded).
///   2. Clear stale SOURCE regions from the canonical's below-END tail so the
///      fresh composition below can lay them down cleanly.
///   3. Compose K2SO-managed body only (no PROJECT.md / AGENT.md appended).
///   4. Write managed body via write_skill_to_all_harnesses with
///      write_shared_markers=false — canonical + Claude/OpenCode/Pi symlinks
///      get just the managed region.
///   5. Append fresh SOURCE regions (PROJECT.md + primary agent AGENT.md)
///      below the canonical's END marker.
///   6. Inject the FULL canonical body (managed + SOURCE regions) into
///      AGENTS.md and .github/copilot-instructions.md — those are plain
///      files, not canonical sources, so they get the full context.
///   7. Symlink project root SKILL.md + CLAUDE.md to canonical.
///   8. Stamp .k2so/.last-skill-regen so subsequent drift-adoption mtime
///      comparisons have a reference point.
pub fn write_workspace_skill_file_with_body(project_path: &str, base_body: Option<&str>) {
    k2_core::workspace::skill_regen::write_workspace_skill_file_with_body(project_path, base_body)
}

// SKILL scaffolding cluster (write_workspace_skill_file_with_body,
// adopt_workspace_skill_drift, strip_workspace_skill_tail,
// append_workspace_source_regions, migrate_and_symlink_root_claude_md,
// import_claude_md_into_user_notes, harvest_per_agent_claude_md_files,
// archive_claude_md_file, inject_first_migration_banner,
// safe_symlink_harness_file, write_workspace_harness_discovery_targets,
// write_cursor_rules_mdc, scaffold_aider_conf,
// teardown_workspace_harness_files, find_latest_archive,
// k2so_agents_teardown_workspace, k2so_agents_preview_workspace_ingest,
// k2so_agents_run_workspace_ingest, ensure_all_skills_up_to_date,
// detect_interrupted_regen) + the migration-safety test module all
// moved into k2so-core during Phase 2 Unit 7b. The Tauri
// #[tauri::command] wrappers that survive this unit forward to the
// k2so-core entry points; the rest is gone from this file.
// Phase 2.5d split that core module into four canonical homes (under
// `k2_core::workspace::{harness, migrations, skill_writer, teardown}`).
// Re-export each symbol from its post-split location; the external
// names at this `commands::k2so_agents::*` boundary stay the same.
pub use k2_core::workspace::harness::{
    HARNESS_WORKSPACE_FILES, WorkspacePreviewEntry,
};
pub use k2_core::workspace::migrations::{
    detect_interrupted_regen, harvest_per_agent_claude_md_files,
};
pub use k2_core::workspace::skill_regen::{
    ensure_all_skills_up_to_date, SKILL_USER_NOTES_SENTINEL, USER_NOTES_PLACEHOLDER,
};
pub use k2_core::workspace::teardown::{
    teardown_workspace_harness_files, TeardownMode, TeardownResult,
};

#[tauri::command]
pub fn k2so_agents_teardown_workspace(
    project_path: String,
    mode: String,
) -> Result<Vec<TeardownResult>, String> {
    k2_core::workspace::teardown::k2so_agents_teardown_workspace(project_path, mode)
}

#[tauri::command]
pub fn k2so_agents_preview_workspace_ingest(
    project_path: String,
) -> Result<Vec<WorkspacePreviewEntry>, String> {
    k2_core::workspace::harness::k2so_agents_preview_workspace_ingest(project_path)
}

#[tauri::command]
pub fn k2so_agents_run_workspace_ingest(project_path: String) -> Result<(), String> {
    k2_core::workspace::harness::k2so_agents_run_workspace_ingest(project_path)
}

// ══════════════════════════════════════════════════════════════════════
// Migration-safety tests (Phase 7c/7d invariants)
// ══════════════════════════════════════════════════════════════════════
//
// These tests pin down the "never lose user data" contract. Every
// migration path we ship must:
//   1. Archive user-authored content before mutating or deleting it.
//   2. Be idempotent (running twice never doubles or re-loses content).
//   3. Return preserved content from strip_workspace_skill_tail so
//      append_workspace_source_regions can re-emit it losslessly.
//   4. Never stack duplicate USER_NOTES sentinels / placeholder comments.

#[cfg(test)]
mod pure_helper_tests {
    //! Tests for the I/O-free helpers extracted from Tauri command
    //! handlers in Phase C of the testability work. Each helper is a
    //! pure function (no fs, no db, no Tauri state) so these tests
    //! run in microseconds and cover edge cases that would otherwise
    //! require scaffolding a full workspace.
    use super::*;

    // ── update_agent_md_field ────────────────────────────────────
    #[test]
    fn update_field_replaces_frontmatter_value() {
        let content = "---\nrole: old role\ntype: custom\n---\n# Agent body\n";
        let updated = update_agent_md_field(content, "role", "new role").unwrap();
        assert!(updated.contains("role: new role"), "got: {}", updated);
        assert!(!updated.contains("role: old role"));
        assert!(updated.contains("type: custom"), "other keys preserved: {}", updated);
        assert!(updated.contains("# Agent body"), "body preserved: {}", updated);
    }

    #[test]
    fn update_field_replaces_section_body() {
        let content = "---\nrole: x\n---\n# Agent\n\n## Work Sources\n\nold content\n\n## Other\n\nkeep\n";
        let updated = update_agent_md_field(content, "Work Sources", "new content").unwrap();
        assert!(updated.contains("## Work Sources\n\nnew content"), "got: {}", updated);
        assert!(!updated.contains("old content"));
        assert!(updated.contains("## Other\n\nkeep"), "trailing section preserved: {}", updated);
    }

    #[test]
    fn update_field_appends_missing_section() {
        let content = "---\nrole: x\n---\n# Agent\n\n## Existing\n\ntext\n";
        let updated = update_agent_md_field(content, "New Section", "added body").unwrap();
        assert!(updated.contains("## New Section\n\nadded body"), "got: {}", updated);
        assert!(updated.contains("## Existing"), "existing preserved");
    }

    #[test]
    fn update_field_replaces_last_section_to_end_of_body() {
        // Edge case: section has no following `## ` so end-of-body
        // is the boundary. Verifies the .unwrap_or(body.len()) path.
        let content = "---\nrole: x\n---\n# Agent\n\n## Tail\n\nold tail content\n";
        let updated = update_agent_md_field(content, "Tail", "new tail").unwrap();
        assert!(updated.contains("## Tail\n\nnew tail"));
        assert!(!updated.contains("old tail content"));
    }

    #[test]
    fn update_field_rejects_missing_frontmatter() {
        let content = "# Just body\n\nno frontmatter here\n";
        let err = update_agent_md_field(content, "role", "x").unwrap_err();
        assert!(err.contains("missing frontmatter"), "got: {}", err);
    }

    #[test]
    fn update_field_rejects_unterminated_frontmatter() {
        let content = "---\nrole: x\nnever-closed\n# body\n";
        let err = update_agent_md_field(content, "role", "y").unwrap_err();
        assert!(err.contains("Invalid frontmatter"), "got: {}", err);
    }

    #[test]
    fn update_field_frontmatter_update_preserves_body_exactly() {
        // The body section after --- must be byte-identical when only
        // a frontmatter key is updated. Regression guard for the
        // extraction: the pre-refactor code stitched body back in
        // verbatim, and we must preserve that.
        let body = "\n# Heading\n\nLine one.\nLine two.\n\n## Sub\n\nMore.\n";
        let content = format!("---\nrole: a\n---{}", body);
        let updated = update_agent_md_field(&content, "role", "b").unwrap();
        assert!(updated.ends_with(body), "body not byte-preserved: {}", updated);
    }

    #[test]
    fn update_field_handles_value_containing_colon() {
        // Values with colons (URLs, ratio notation) must survive the
        // split_once logic and round-trip correctly.
        let content = "---\nrole: old\n---\n";
        let updated = update_agent_md_field(content, "role", "URL: https://example.com/path").unwrap();
        assert!(updated.contains("role: URL: https://example.com/path"), "got: {}", updated);
    }

    // ── compose_manager_wake_from_body ───────────────────────────
    //
    // P8 retired the "K2SO Heartbeat Wake — Workspace Manager" boilerplate
    // preamble that used to wrap the body. The composer now returns the
    // wakeup body verbatim (frontmatter stripped). These tests exercise
    // the post-P8 contract: body content survives, fallback template
    // kicks in for empty/missing input, no boilerplate added.

    #[test]
    fn compose_manager_wake_uses_provided_body() {
        let out = compose_manager_wake_from_body(Some("custom manager instructions"));
        assert!(out.contains("custom manager instructions"), "body inlined");
        // No K2SO boilerplate prefix anymore — body is the message.
        assert!(!out.contains("K2SO Heartbeat Wake"), "preamble retired in P8: {}", out);
    }

    #[test]
    fn compose_manager_wake_falls_back_when_body_none() {
        let out = compose_manager_wake_from_body(None);
        // Fallback uses WAKEUP_TEMPLATE_WORKSPACE — assert its trim()'d
        // first line is in the output.
        let template_lead = WAKEUP_TEMPLATE_WORKSPACE.trim().lines().next().unwrap_or("");
        assert!(!template_lead.is_empty());
        assert!(
            out.contains(template_lead),
            "expected template fallback to contain first line '{}', got: {}",
            template_lead,
            out
        );
    }

    #[test]
    fn compose_manager_wake_falls_back_when_body_is_empty_string() {
        // A disk read returning "" after frontmatter strip must hit
        // the fallback — not silently emit an empty wake prompt.
        let out = compose_manager_wake_from_body(Some(""));
        let template_lead = WAKEUP_TEMPLATE_WORKSPACE.trim().lines().next().unwrap_or("");
        assert!(out.contains(template_lead), "expected template fallback, got: {}", out);
    }

    #[test]
    fn compose_manager_wake_strips_frontmatter_from_body() {
        // If the disk body has its own frontmatter (e.g. a scaffolded
        // WAKEUP.md with metadata), strip_frontmatter must run before
        // the empty-check.
        let body = "---\ntitle: foo\n---\nActual wake instructions here.";
        let out = compose_manager_wake_from_body(Some(body));
        assert!(!out.contains("title: foo"), "frontmatter leaked: {}", out);
        assert!(out.contains("Actual wake instructions here"), "body survived: {}", out);
    }

    // ── compose_agent_wake_from_body ─────────────────────────────
    #[test]
    fn compose_agent_wake_returns_none_on_none_input() {
        assert!(compose_agent_wake_from_body(None).is_none());
    }

    #[test]
    fn compose_agent_wake_returns_body_verbatim() {
        // P8: composer returns the body itself (frontmatter stripped),
        // no boilerplate preamble. The wakeup.md content is the message.
        let out = compose_agent_wake_from_body(Some("agent instructions"))
            .expect("body present -> Some");
        assert!(out.contains("agent instructions"), "body in output: {}", out);
        // No "K2SO Heartbeat Wake" preamble anymore.
        assert!(!out.contains("K2SO Heartbeat Wake"), "preamble retired in P8: {}", out);
    }

    #[test]
    fn compose_agent_wake_strips_frontmatter() {
        // P8: composer now strips frontmatter symmetrically with the
        // manager composer. Pre-P8 it left frontmatter intact and
        // expected callers to strip; post-P8 the composer owns it.
        let body = "---\nname: foo\n---\nbody";
        let out = compose_agent_wake_from_body(Some(body)).unwrap();
        assert!(!out.contains("name: foo"), "frontmatter stripped: {}", out);
        assert!(out.contains("body"), "body survived: {}", out);
    }

    #[test]
    fn compose_agent_wake_returns_none_for_empty_body() {
        // P8: empty body (after frontmatter strip) returns None so
        // smart_launch can record a "wakeup body empty" audit instead
        // of firing claude with no prompt.
        let body = "---\ndescription:\n---\n\n";
        assert!(compose_agent_wake_from_body(Some(body)).is_none());
    }

    // ── parse_work_item_content ──────────────────────────────────
    #[test]
    fn parse_work_item_full_frontmatter() {
        let content = "---\ntitle: Add OAuth\npriority: high\ntype: feature\nsource: feedback\ncreated: 2026-04-01\nassigned_by: user\n---\n\nBody text here that describes the work.";
        let item = parse_work_item_content(content, "add-oauth.md", "inbox");
        assert_eq!(item.title, "Add OAuth");
        assert_eq!(item.priority, "high");
        assert_eq!(item.item_type, "feature");
        assert_eq!(item.source, "feedback");
        assert_eq!(item.assigned_by, "user");
        assert_eq!(item.folder, "inbox");
        assert_eq!(item.filename, "add-oauth.md");
        assert!(item.body_preview.contains("Body text here"));
    }

    #[test]
    fn parse_work_item_missing_fields_use_defaults() {
        let content = "---\ntitle: minimal\n---\nbody";
        let item = parse_work_item_content(content, "m.md", "active");
        assert_eq!(item.title, "minimal");
        assert_eq!(item.priority, "normal"); // default
        assert_eq!(item.item_type, "task"); // default
        assert_eq!(item.source, "manual"); // default
        assert_eq!(item.assigned_by, "unknown"); // default
    }

    #[test]
    fn parse_work_item_no_frontmatter_defaults_all_but_body() {
        let content = "just a body with no metadata";
        let item = parse_work_item_content(content, "raw.md", "inbox");
        assert_eq!(item.title, "");
        assert_eq!(item.body_preview, "just a body with no metadata");
    }

    #[test]
    fn parse_work_item_body_preview_truncates_over_120_chars() {
        let long_body = "x".repeat(300);
        let content = format!("---\ntitle: t\n---\n{}", long_body);
        let item = parse_work_item_content(&content, "l.md", "inbox");
        // Preview is 120 + "..." — exact char count matters.
        assert!(item.body_preview.ends_with("..."), "preview: {:?}", item.body_preview);
        let without_ellipsis = item.body_preview.trim_end_matches("...");
        assert_eq!(without_ellipsis.chars().count(), 120);
    }

    // ── FakeFs-driven demonstration ──────────────────────────────
    //
    // These tests show the end-state pattern: use FakeFs to scaffold
    // a workspace tree, call the Fs trait to read content, then feed
    // that content into the pure parser. No tempdir, no disk I/O.
    //
    // Once `read_work_item` is threaded with `&dyn Fs`, these tests
    // can drop the manual read_to_string and just pass the fs into a
    // higher-level helper. For now they demonstrate the pattern and
    // prove the integration (pure parser + FakeFs storage).

    #[test]
    fn fake_fs_scaffolds_agent_work_tree_and_parses_items() {
        use crate::fs_abstract::{FakeFs, Fs};
        use std::path::Path;

        let fs = FakeFs::new();
        fs.insert_tree(
            Path::new("/proj/.k2so/agents/backend-eng/work"),
            serde_json::json!({
                "inbox": {
                    "build-oauth.md": "---\ntitle: Build OAuth\npriority: high\ntype: feature\n---\n\nOAuth endpoints required.",
                    "fix-crash.md": "---\ntitle: Fix startup crash\npriority: urgent\ntype: bug\nsource: crash\n---\n\nCrashes on launch.",
                },
                "active": {},
                "done": {},
            }),
        );

        let inbox_dir = Path::new("/proj/.k2so/agents/backend-eng/work/inbox");
        let mut entries = fs.read_dir(inbox_dir).unwrap();
        entries.sort();

        let items: Vec<WorkItem> = entries
            .iter()
            .map(|p| {
                let content = fs.read_to_string(p).unwrap();
                let filename = p.file_name().unwrap().to_string_lossy();
                parse_work_item_content(&content, &filename, "inbox")
            })
            .collect();

        assert_eq!(items.len(), 2);
        let oauth = items.iter().find(|i| i.filename == "build-oauth.md").unwrap();
        assert_eq!(oauth.title, "Build OAuth");
        assert_eq!(oauth.priority, "high");
        let crash = items.iter().find(|i| i.filename == "fix-crash.md").unwrap();
        assert_eq!(crash.priority, "urgent");
        assert_eq!(crash.source, "crash");

        // Sanity: FakeFs's write counter shows exactly one write per
        // file (the insert_tree calls). Good regression guard for
        // "does my test accidentally double-write?"
        assert_eq!(fs.write_count(&inbox_dir.join("build-oauth.md")), 1);
        assert_eq!(fs.write_count(&inbox_dir.join("fix-crash.md")), 1);
    }

    #[test]
    fn fake_fs_simulates_missing_agent_work_dir() {
        use crate::fs_abstract::{FakeFs, Fs};
        use std::path::Path;

        let fs = FakeFs::new();
        // Intentionally do NOT scaffold the inbox — simulate a fresh
        // agent directory with no work yet. The caller must handle
        // NotFound gracefully.
        let err = fs
            .read_dir(Path::new("/proj/.k2so/agents/solo/work/inbox"))
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn fake_fs_verifies_frontmatter_round_trip_via_update_field() {
        // End-to-end: scaffold an AGENT.md in FakeFs, read it out,
        // pass through the extracted pure updater, write the result
        // back. Confirm the write went through and content matches.
        use crate::fs_abstract::{FakeFs, Fs};
        use std::path::Path;

        let fs = FakeFs::new();
        let agent_md = Path::new("/proj/.k2so/agents/rust-eng/AGENT.md");
        let original = "---\nrole: rust engineer\ntype: custom\n---\n# Rust engineer\n\nFocus: backend, systems.";
        fs.insert_file(agent_md, original.as_bytes());

        let content = fs.read_to_string(agent_md).unwrap();
        let updated = update_agent_md_field(&content, "role", "principal rust engineer").unwrap();
        fs.write(agent_md, updated.as_bytes()).unwrap();

        let final_content = fs.read_to_string(agent_md).unwrap();
        assert!(final_content.contains("role: principal rust engineer"));
        assert!(final_content.contains("type: custom"));
        assert!(final_content.contains("# Rust engineer"));
        // write_count should be 2: insert_file (1) + write (1).
        assert_eq!(fs.write_count(agent_md), 2);
    }
}
