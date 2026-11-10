//! Read-only triage summary: which agents have pending work, their
//! inbox items with priority/type/source labels, which are locked, and
//! the workspace-level inbox waiting on the primary agent.
//!
//! The only caller is the daemon's `/cli/agents/triage` route (public
//! API, `k2_daemon::triage::handle_triage`). No CLI verb calls it: the
//! old `k2so agents triage` verb is gone, and `k2 workspace triage`
//! reads `/cli/inbox/list` instead. Never spawns anything; the
//! destructive heartbeat path is `/cli/scheduler-tick`.
//!
//! History: pre-Phase-4 this was `k2so_agents_triage_summary` in
//! `src-tauri/src/commands/k2so_agents.rs`, served by Tauri's
//! `agent_hooks` listener at the same route.
//!
//! Covered by `crates/k2-daemon/tests/triage_integration.rs`
//! (`triage_summary_is_readonly_and_plaintext`).

use std::fs;
use std::path::Path;

use crate::workspace::agent_identity::{agents_dir, parse_frontmatter, resolve_agent_name};
use crate::workspace::scheduler::{agent_work_dir, is_agent_locked};
use crate::workspace::work_item::{read_work_item, WorkItem};
use crate::inbox::{list_folder as inbox_list_folder, InboxItem};

/// Plain-text triage summary. Walks `.k2so/agents/*/work/inbox` +
/// `.k2so/inbox/` (root-level untriaged arrivals) on disk (no DB
/// access) and renders a human-readable report.
///
/// Workspace States (capability gating by source type) retired — every
/// inbox item is listed without omit/gated labels.
pub fn triage_summary(project_path: &str) -> Result<String, String> {
    let dir = agents_dir(project_path);
    if !dir.exists() {
        return Ok("No agents configured.".to_string());
    }

    let mut summary = String::new();
    let entries = fs::read_dir(&dir).map_err(|e| e.to_string())?;

    for entry in entries.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();

        let inbox = agent_work_dir(project_path, &name, "inbox");
        let active = agent_work_dir(project_path, &name, "active");

        let inbox_items: Vec<WorkItem> = if inbox.exists() {
            fs::read_dir(&inbox)
                .ok()
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|e| {
                            e.path().extension().map_or(false, |ext| ext == "md")
                        })
                        .filter_map(|e| read_work_item(&e.path(), "inbox"))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            vec![]
        };

        let active_count = if active.exists() {
            fs::read_dir(&active)
                .map(|e| {
                    e.flatten()
                        .filter(|e| {
                            e.path().extension().map_or(false, |ext| ext == "md")
                        })
                        .count()
                })
                .unwrap_or(0)
        } else {
            0
        };

        let is_locked = is_agent_locked(project_path, &name);

        if inbox_items.is_empty() && active_count == 0 {
            continue;
        }

        let agent_md_path = entry.path().join("AGENT.md");
        let (agent_type, agent_role) = if agent_md_path.exists() {
            let content = fs::read_to_string(&agent_md_path).unwrap_or_default();
            let fm = parse_frontmatter(&content);
            (
                fm.get("type")
                    .cloned()
                    .unwrap_or_else(|| "agent-template".to_string()),
                fm.get("role").cloned().unwrap_or_default(),
            )
        } else {
            ("agent-template".to_string(), String::new())
        };

        summary.push_str(&format!(
            "Agent: {} (type: {}, role: {})\n",
            name, agent_type, agent_role
        ));
        if is_locked {
            summary.push_str("  Status: LOCKED (active session running)\n");
        }
        if active_count > 0 {
            summary.push_str(&format!(
                "  Active: {} items in progress\n",
                active_count
            ));
        }
        for item in &inbox_items {
            summary.push_str(&format!(
                "  Inbox: \"{}\" (priority: {}, type: {}, source: {})\n",
                item.title, item.priority, item.item_type, item.source
            ));
        }
        summary.push('\n');
    }

    // Post-Phase-2.1: workspace inbox is `.k2so/inbox/`. Root-level
    // items (`folder == ""`) are untriaged arrivals that still need a
    // home — they're the items the workspace's primary agent (the
    // "Coordinator" in human terms) would triage.
    let ws_items: Vec<InboxItem> = inbox_list_folder(Path::new(project_path), "");
    if !ws_items.is_empty() {
        // Resolve the primary agent to check the lock; the display
        // label remains "Coordinator" (human-facing presentation, not
        // a routing key).
        let coordinator_locked = resolve_agent_name(project_path)
            .map(|primary| is_agent_locked(project_path, &primary))
            .unwrap_or(false);
        summary.push_str("Workspace Inbox (unassigned — needs Coordinator):\n");
        if coordinator_locked {
            summary.push_str("  Coordinator: LOCKED (active session running)\n");
        }
        for item in &ws_items {
            // `type` field dropped in the WorkItem → InboxItem
            // migration (no analogue on the new shape). `source` is
            // preserved and now does double duty.
            summary.push_str(&format!(
                "  \"{}\" (priority: {}, source: {})\n",
                item.title, item.priority, item.source
            ));
        }
        summary.push('\n');
    }

    if summary.is_empty() {
        Ok("No agents have pending work.".to_string())
    } else {
        Ok(summary)
    }
}
