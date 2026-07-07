//! Wakeup scaffolding for agent creation.
//!
//! Phase 2.5d: extracted from the monolithic `agents/commands.rs`.
//! The workspace-level multi-heartbeat surface lives in
//! [`crate::heartbeats`] (its `mod.rs`).
//!
//! 0.40.31: the legacy per-agent adaptive-backoff API that used to
//! live here (`get_heartbeat`, `set_heartbeat`, `heartbeat_noop`,
//! `heartbeat_action` — the `legacy-per-agent-heartbeat` tag) is
//! DELETED along with its `<agent>/heartbeat.json` reader/writer in
//! `workspace::scheduler`. Nothing consumed the JSON file since the
//! custom-agent scheduler loop was retired in 0.39.0d. Only
//! `ensure_agent_wakeup` (called during agent creation + first-boot
//! migrations) remains.

use crate::workspace::wake_prompts::{agent_wakeup_path, wakeup_template_for};
use crate::fs_atomic::{atomic_write_str, log_if_err};

// ── Wakeup scaffolding ────────────────────────────────────────────────

/// Create `<agent_dir>/WAKEUP.md` from the matching template if it
/// doesn't exist. No-op when the agent type doesn't use wake-up or
/// when a heartbeat folder has already claimed ownership of the wake
/// source of truth.
pub fn ensure_agent_wakeup(project_path: &str, agent_name: &str, agent_type: &str) {
    let Some(template) = wakeup_template_for(agent_type) else {
        return;
    };
    let path = agent_wakeup_path(project_path, agent_name);
    if path.exists() {
        return;
    }
    // Multi-heartbeat lives at .k2so/heartbeats/<name>/WAKEUP.md
    // (post-0.37.0 workspace-level layout). If any heartbeat folder
    // already exists, we're past the legacy single-slot world and
    // the agent-root wakeup.md is no longer the source of truth.
    // Skip scaffolding to avoid tricking the repair pass into
    // clobbering real content.
    let hb_default = crate::workspace::agent_identity::workspace_heartbeats_dir(project_path)
        .join("default")
        .join("WAKEUP.md");
    if hb_default.exists() {
        return;
    }
    log_if_err(
        "ensure_agent_wakeup",
        &path,
        atomic_write_str(&path, template),
    );
}
