//! Aggregated agent check-in — the data bundle an agent reads at the
//! top of a session to understand its current state.
//!
//! Serves `/cli/checkin`. Composes:
//!
//! - `task`: first item in the workspace inbox's `active/` folder
//!   (structured). Post-Phase-2.1 the workspace IS the agent, so
//!   "the agent's current task" lives in `.k2so/inbox/active/`,
//!   not the retired per-agent `.k2so/agents/<name>/work/active/`.
//! - `inbox.work`: every item at the workspace inbox root
//!   (`.k2so/inbox/*.md`) — the untriaged arrivals the workspace
//!   agent needs to see on wake.
//! - `inbox.messages`: unread DB messages addressed to this agent.
//!   Marked read on retrieval.
//! - `peers`: `agent_sessions` rows for every connected workspace
//!   (outgoing + incoming `workspace_relations`), with the current
//!   status / status_message / terminal_id.
//! - `reservations`: the JSON map at `.k2so/reservations.json`.
//! - `feed`: last 10 activity-feed entries for this project.
//! - `wakeupInstructions`: the agent's wakeup.md body (or the
//!   workspace-level wakeup for manager-mode primaries); `null`
//!   for agent-template roles that don't use wake-up prompts.
//!
//! Finally logs a `checkin` activity entry so peers can see the
//! agent just checked in.
//!
//! Moved to core so the daemon serves it headlessly.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::workspace::agent_identity::{agent_type_for, resolve_project_id};
use crate::workspace::wake_prompts::{
    compose_wake_prompt_for_agent, compose_wake_prompt_for_workspace,
};
use crate::db::schema::{
    get_unread_messages, log_activity, mark_messages_read, ActivityFeedEntry, WorkspaceSession,
    WorkspaceRelation,
};

/// Parse the minimum frontmatter fields the CLI echoes back for each
/// work item. Kept private: callers that want the full `WorkItem`
/// struct use [`super::work_item::read_work_item`] instead.
fn parse_work_item(filename: &str, content: &str) -> serde_json::Value {
    let mut title = filename.trim_end_matches(".md").to_string();
    let mut priority = "normal".to_string();
    let mut item_type = "task".to_string();
    let mut from = serde_json::Value::Null;
    let mut body = content.to_string();

    if let Some(stripped) = content.strip_prefix("---\n") {
        if let Some(end) = stripped.find("\n---") {
            let fm = &stripped[..end];
            body = stripped[end + 4..].trim().to_string();
            for line in fm.lines() {
                let parts: Vec<&str> = line.splitn(2, ':').collect();
                if parts.len() == 2 {
                    let key = parts[0].trim();
                    let val = parts[1].trim().trim_matches('"');
                    match key {
                        "title" => title = val.to_string(),
                        "priority" => priority = val.to_string(),
                        "type" => item_type = val.to_string(),
                        "from" => from = serde_json::Value::String(val.to_string()),
                        "assigned_by" if from.is_null() => {
                            from = serde_json::Value::String(val.to_string())
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    serde_json::json!({
        "file": filename,
        "title": title,
        "priority": priority,
        "type": item_type,
        "from": from,
        "body": body,
    })
}

/// Build the check-in bundle. Returns JSON string matching the shape
/// the CLI has emitted since 0.32.x.
pub fn checkin(project_path: &str, agent: &str) -> Result<String, String> {
    if agent.is_empty() {
        return Err("Missing 'agent' parameter".to_string());
    }

    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;

    // Current task: first item in the workspace inbox's standard
    // `active/` folder. Post-Phase-2.1 the workspace IS the agent,
    // so the agent's "in-flight" task lives at workspace level
    // (`.k2so/inbox/active/`), not under a per-agent subtree.
    // Touched via the unified `crate::inbox::*` primitive so the
    // checkin / scheduler / triage call sites all share one shape.
    let workspace_path = Path::new(project_path);
    let active_items = crate::inbox::list_folder(workspace_path, "active");
    let task: serde_json::Value = match active_items.into_iter().next() {
        Some(item) => {
            let content = crate::inbox::read_by_id(workspace_path, &item.id)
                .unwrap_or_default();
            parse_work_item(&item.filename, &content)
        }
        None => serde_json::Value::Null,
    };

    // Workspace inbox: root-level items only (sub-foldered items have
    // already been organized by the workspace agent and aren't part
    // of the "untriaged arrivals" the agent needs to see on wake).
    let mut work_items: Vec<serde_json::Value> = Vec::new();
    let ws_inbox_items = crate::inbox::list_folder(workspace_path, "");
    for item in ws_inbox_items {
        let content = crate::inbox::read_by_id(workspace_path, &item.id)
            .unwrap_or_default();
        work_items.push(parse_work_item(&item.filename, &content));
    }

    // Messages (DB-indexed)
    let messages: Vec<serde_json::Value> = get_unread_messages(&conn, &project_id, agent)
        .unwrap_or_default()
        .into_iter()
        .map(|m| {
            let text = m
                .metadata
                .as_deref()
                .and_then(|md| serde_json::from_str::<serde_json::Value>(md).ok())
                .and_then(|v| {
                    v.get("text")
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| m.summary.clone().unwrap_or_default());
            serde_json::json!({
                "type": "message",
                "from": m.from_workspace,
                "text": text,
                "at": m.created_at,
                "id": m.id,
            })
        })
        .collect();

    let _ = mark_messages_read(&conn, &project_id, agent);

    let inbox = serde_json::json!({
        "work": work_items,
        "messages": messages,
    });

    // Peers: agent_sessions across this project + connected workspaces
    let mut peer_project_ids = vec![project_id.clone()];
    if let Ok(rels) = WorkspaceRelation::list_for_source(&conn, &project_id) {
        for r in &rels {
            peer_project_ids.push(r.target_project_id.clone());
        }
    }
    if let Ok(rels) = WorkspaceRelation::list_for_target(&conn, &project_id) {
        for r in &rels {
            peer_project_ids.push(r.source_project_id.clone());
        }
    }

    let mut project_names: HashMap<String, String> = HashMap::new();
    for pid in &peer_project_ids {
        if let Ok(name) = conn.query_row(
            "SELECT name FROM projects WHERE id = ?1",
            rusqlite::params![pid],
            |row| row.get::<_, String>(0),
        ) {
            project_names.insert(pid.clone(), name);
        }
    }

    let mut peers = Vec::new();
    for pid in &peer_project_ids {
        if pid == &project_id {
            // The caller's own workspace — skip; not a peer.
            continue;
        }
        if let Ok(Some(s)) = WorkspaceSession::get(&conn, pid) {
            let pname = project_names.get(pid).cloned().unwrap_or_default();
            peers.push(serde_json::json!({
                "agent": pname.clone(),
                "status": s.status,
                "statusMessage": s.status_message,
                "terminalId": s.terminal_id,
                "project": pname,
                "projectId": s.project_id,
                "harness": s.harness,
            }));
        }
    }

    // Reservations
    let reservations_path =
        crate::workspace_dot_dir(project_path).join("reservations.json");
    let reservations: serde_json::Value = if reservations_path.exists() {
        fs::read_to_string(&reservations_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Recent feed
    let feed: Vec<serde_json::Value> =
        ActivityFeedEntry::list_by_project(&conn, &project_id, 10, 0)
            .unwrap_or_default()
            .into_iter()
            .map(|e| {
                serde_json::json!({
                    "eventType": e.event_type,
                    "agent": e.actor,
                    "from": e.from_workspace,
                    "to": e.to_workspace,
                    "summary": e.summary,
                    "createdAt": e.created_at,
                })
            })
            .collect();

    log_activity(
        &conn,
        &project_id,
        Some(agent),
        "checkin",
        Some(agent),
        None,
        None,
        None,
    );

    // Wake-up instructions — manager-mode primaries use the workspace
    // wake prompt composer (sourced from the workspace `triage`
    // heartbeat row); other agents use their own WAKEUP.md (or null
    // for agent-template roles that don't use wake-up). Dispatch by
    // agent type rather than agent name so we never special-case any
    // particular routing string.
    let wakeup_instructions: serde_json::Value = if agent_type_for(project_path, agent) == "manager" {
        serde_json::Value::String(compose_wake_prompt_for_workspace(project_path))
    } else {
        match compose_wake_prompt_for_agent(project_path, agent) {
            Some(s) => serde_json::Value::String(s),
            None => serde_json::Value::Null,
        }
    };

    Ok(serde_json::json!({
        "agent": agent,
        "project": project_path,
        "task": task,
        "inbox": inbox,
        "peers": peers,
        "reservations": reservations,
        "feed": feed,
        "wakeupInstructions": wakeup_instructions,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_work_item_reads_basic_frontmatter() {
        let md = "---\ntitle: Fix auth bug\npriority: high\ntype: bug\nfrom: qa\n---\n\nDetails here.";
        let v = parse_work_item("bug.md", md);
        assert_eq!(v.get("title").and_then(|t| t.as_str()), Some("Fix auth bug"));
        assert_eq!(v.get("priority").and_then(|t| t.as_str()), Some("high"));
        assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("bug"));
        assert_eq!(v.get("from").and_then(|t| t.as_str()), Some("qa"));
        assert_eq!(v.get("file").and_then(|t| t.as_str()), Some("bug.md"));
    }

    #[test]
    fn parse_work_item_without_frontmatter_defaults_gracefully() {
        let v = parse_work_item("plain.md", "Just a body, no frontmatter.");
        assert_eq!(v.get("title").and_then(|t| t.as_str()), Some("plain"));
        assert_eq!(v.get("priority").and_then(|t| t.as_str()), Some("normal"));
        assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("task"));
        assert!(v.get("from").unwrap().is_null());
    }

    #[test]
    fn parse_work_item_falls_back_to_assigned_by_when_from_missing() {
        let md = "---\ntitle: T\nassigned_by: reviewer\n---\n";
        let v = parse_work_item("t.md", md);
        assert_eq!(v.get("from").and_then(|t| t.as_str()), Some("reviewer"));
    }

    /// Phase 2.5b regression: `checkin()` must source the agent's
    /// in-flight task + inbox from the unified workspace inbox
    /// (`.k2so/inbox/`), not the retired per-agent
    /// `.k2so/agents/<name>/work/` tree. Reproduces the shape of a
    /// post-Phase-2.5b workspace where `.k2so/agents/` is absent
    /// entirely. Pre-fix this test would observe a null task and an
    /// empty work list because the legacy fs paths don't exist.
    #[test]
    fn checkin_reads_active_task_from_workspace_inbox_not_legacy_agents_dir() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let _ = crate::db::init_for_tests();

        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let project_path = std::env::temp_dir().join(format!(
            "k2so-checkin-regress-{}-{}-{}",
            std::process::id(),
            nanos,
            n
        ));
        let _ = fs::remove_dir_all(&project_path);
        fs::create_dir_all(project_path.join(".k2so")).unwrap();

        // Register project + a placeholder workspace_session so the
        // checkin's downstream peer lookup doesn't panic on missing
        // rows. Use a unique id to avoid cross-test interference on
        // the shared in-memory DB.
        let project_id = format!("proj-checkin-{}-{}", std::process::id(), n);
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT OR REPLACE INTO projects \
                 (id, path, name, color, agent_mode, pinned, tab_order) \
                 VALUES (?1, ?2, ?3, '#123456', 'manager', 0, 0)",
                rusqlite::params![project_id, project_path.to_string_lossy(), "test"],
            )
            .unwrap();
        }

        // Set up the post-Phase-2.5b workspace shape:
        //   .k2so/inbox/active/in-flight.md   ← current task
        //   .k2so/inbox/new-arrival.md        ← untriaged inbox item
        // and explicitly DO NOT create `.k2so/agents/`.
        let inbox_root = project_path.join(".k2so").join("inbox");
        fs::create_dir_all(inbox_root.join("active")).unwrap();
        fs::write(
            inbox_root.join("active").join("in-flight.md"),
            "---\ntitle: Working on this\npriority: high\ntype: task\n---\n\nDetails.",
        )
        .unwrap();
        fs::write(
            inbox_root.join("new-arrival.md"),
            "---\ntitle: New arrival\npriority: normal\ntype: task\n---\n\nBody.",
        )
        .unwrap();
        assert!(
            !project_path.join(".k2so").join("agents").exists(),
            "legacy .k2so/agents/ must NOT exist for this regression",
        );

        let result = checkin(&project_path.to_string_lossy(), "any-agent").unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();

        // `task` populated from `.k2so/inbox/active/`.
        assert_eq!(
            v.get("task").and_then(|t| t.get("title")).and_then(|t| t.as_str()),
            Some("Working on this"),
            "task should come from workspace inbox active folder; got: {v}",
        );

        // `inbox.work` populated from `.k2so/inbox/` root (not active/).
        let work = v
            .get("inbox")
            .and_then(|i| i.get("work"))
            .and_then(|w| w.as_array())
            .expect("inbox.work array");
        assert_eq!(work.len(), 1, "expected one root-level inbox item; got {work:?}");
        assert_eq!(
            work[0].get("title").and_then(|t| t.as_str()),
            Some("New arrival"),
        );

        let _ = fs::remove_dir_all(&project_path);
    }
}
