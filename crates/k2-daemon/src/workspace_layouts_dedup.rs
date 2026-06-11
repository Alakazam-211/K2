//! 0.38.0 — one-shot heal passes for `workspace_layouts.layout_json`.
//!
//! Two passes, each gated by its own `code_migrations` marker so they
//! run at most once and are independently re-runnable in future versions:
//!
//! - **`0.38.0-layout-dedup`** — collapses tabs sharing canonical
//!   paneGroup-id sets. Pre-0.38.0 the mount-time `sync:tabs-request`
//!   broadcast race caused non-main windows to adopt tabs from the main
//!   window on top of their own restored set. When the bloated state was
//!   saved back, the layout JSON grew duplicate `tabs[]` entries that
//!   all pointed at the same paneGroup id.
//!
//! - **`0.38.0-layout-v2-emit`** — migrates `version: 1` layouts to v2.
//!   v2 makes the layout metadata-only for daemon-backed tabs: terminal
//!   items carry only `paneGroupId` plus heartbeat metadata. The
//!   renderer (`tabs.ts::migrateLayoutToV2`) does the same migration on
//!   read; this daemon pass eagerly converges workspaces the user
//!   hasn't opened in a while.
//!
//! The renderer also runs dedup on every restore (see
//! `tabs.ts::dedupTabsBySignature`) — these passes are belt-and-braces
//! cleanup so corrupt or pre-v2 rows get repaired even when the
//! workspace doesn't get opened soon.

use serde_json::Value;
use std::collections::BTreeSet;

use k2_core::log_debug;

const MIGRATION_ID: &str = "0.38.0-layout-dedup";
const MIGRATION_V2_ID: &str = "0.38.0-layout-v2-emit";
const LAYOUT_SCHEMA_VERSION: u64 = 2;

/// Canonical identity of one tab: the sorted set of paneGroup IDs it
/// owns, joined as a comma-separated string. Two tabs with identical
/// signatures point at the same daemon-side session(s) and are
/// duplicates regardless of distinct renderer-side tab UUIDs.
fn tab_signature(tab: &Value) -> String {
    let mut ids: BTreeSet<String> = BTreeSet::new();
    if let Some(pgs) = tab.get("paneGroups").and_then(|v| v.as_object()) {
        for (k, _) in pgs {
            ids.insert(k.clone());
        }
    }
    ids.into_iter().collect::<Vec<_>>().join(",")
}

/// Walk a `tabs[]` array, dropping duplicates by signature.
/// Returns (cleaned_tabs, removed_count, id_remap). The id_remap maps
/// every removed tab's `id` to the kept tab's `id` so callers can
/// rewrite `activeTabId` pointers that referenced a dropped duplicate.
fn dedup_tabs_array(
    tabs: Vec<Value>,
) -> (Vec<Value>, usize, std::collections::HashMap<String, String>) {
    use std::collections::HashMap;
    let mut seen: HashMap<String, String> = HashMap::new(); // signature -> kept tab id
    let mut id_remap: HashMap<String, String> = HashMap::new();
    let mut kept: Vec<Value> = Vec::with_capacity(tabs.len());
    let mut removed = 0usize;
    for tab in tabs {
        let sig = tab_signature(&tab);
        let tab_id = tab
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();
        if let Some(kept_id) = seen.get(&sig) {
            if !tab_id.is_empty() {
                id_remap.insert(tab_id, kept_id.clone());
            }
            removed += 1;
        } else {
            seen.insert(sig, tab_id.clone());
            kept.push(tab);
        }
    }
    (kept, removed, id_remap)
}

/// Returns (changed, total_removed). `changed` is whether any tab was
/// dropped; if false, callers skip the UPDATE entirely.
fn dedup_layout_json(layout: &mut Value) -> (bool, usize) {
    let mut total_removed = 0usize;

    // Main group: tabs + activeTabId
    if let Some(tabs_arr) = layout.get_mut("tabs").and_then(|v| v.as_array_mut()) {
        let original = std::mem::take(tabs_arr);
        let (cleaned, removed, id_remap) = dedup_tabs_array(original);
        *tabs_arr = cleaned;
        total_removed += removed;

        // Rewrite activeTabId if it pointed at a dropped duplicate.
        if removed > 0 {
            if let Some(active) = layout.get("activeTabId").and_then(|v| v.as_str()) {
                if let Some(new_active) = id_remap.get(active) {
                    layout["activeTabId"] = Value::String(new_active.clone());
                }
            }
        }
    }

    // Extra groups (split columns): each has its own tabs[] + activeTabId.
    if let Some(extra) = layout.get_mut("extraGroups").and_then(|v| v.as_array_mut()) {
        for group in extra.iter_mut() {
            if let Some(group_tabs) = group.get_mut("tabs").and_then(|v| v.as_array_mut()) {
                let original = std::mem::take(group_tabs);
                let (cleaned, removed, id_remap) = dedup_tabs_array(original);
                *group_tabs = cleaned;
                total_removed += removed;
                if removed > 0 {
                    if let Some(active) = group.get("activeTabId").and_then(|v| v.as_str()) {
                        if let Some(new_active) = id_remap.get(active) {
                            group["activeTabId"] = Value::String(new_active.clone());
                        }
                    }
                }
            }
        }
    }

    (total_removed > 0, total_removed)
}

/// Run the one-shot heal pass. Idempotent — gated by a
/// `code_migrations` marker that's stamped on successful completion.
/// Safe to call on every daemon boot; subsequent calls are O(1) lookups.
pub fn run_once() {
    let db = k2_core::db::shared();
    let conn = db.lock();

    if k2_core::db::has_code_migration_applied(&conn, MIGRATION_ID) {
        return;
    }

    log_debug!("[daemon/boot] running {MIGRATION_ID} pass over workspace_layouts");

    // SELECT all rows first so the prepared statement's row iterator
    // doesn't hold a borrow across the UPDATE statements.
    let mut rows: Vec<(String, String, String, String)> = Vec::new();
    match conn.prepare(
        "SELECT id, project_id, workspace_id, layout_json FROM workspace_layouts",
    ) {
        Ok(mut stmt) => {
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            });
            if let Ok(it) = mapped {
                for r in it.flatten() {
                    rows.push(r);
                }
            }
        }
        Err(e) => {
            log_debug!("[daemon/boot] {MIGRATION_ID}: SELECT failed: {e}");
            return;
        }
    }

    let total = rows.len();
    let mut healed = 0usize;
    let mut tabs_removed = 0usize;
    for (id, project_id, workspace_id, json_str) in rows {
        let mut layout: Value = match serde_json::from_str(&json_str) {
            Ok(v) => v,
            Err(e) => {
                log_debug!(
                    "[daemon/boot] {MIGRATION_ID}: skip row id={id} (parse: {e})"
                );
                continue;
            }
        };

        let (changed, removed) = dedup_layout_json(&mut layout);
        if !changed {
            continue;
        }

        let new_json = match serde_json::to_string(&layout) {
            Ok(s) => s,
            Err(e) => {
                log_debug!(
                    "[daemon/boot] {MIGRATION_ID}: skip row id={id} (re-serialize: {e})"
                );
                continue;
            }
        };

        match conn.execute(
            "UPDATE workspace_layouts SET layout_json = ?1, updated_at = unixepoch() WHERE id = ?2",
            rusqlite::params![new_json, id],
        ) {
            Ok(_) => {
                healed += 1;
                tabs_removed += removed;
                log_debug!(
                    "[daemon/boot] {MIGRATION_ID}: healed project={project_id} workspace={workspace_id} (-{removed} tab rows)"
                );
            }
            Err(e) => {
                log_debug!(
                    "[daemon/boot] {MIGRATION_ID}: UPDATE failed for id={id}: {e}"
                );
            }
        }
    }

    let notes = format!("rows_scanned={total} rows_healed={healed} tabs_removed={tabs_removed}");
    k2_core::db::mark_code_migration_applied(&conn, MIGRATION_ID, Some(&notes));
    log_debug!(
        "[daemon/boot] {MIGRATION_ID} complete: {notes}"
    );
}

/// v2 makes the layout metadata-only for daemon-backed tabs. v1 terminal
/// items carried `cwd`, `command`, `args`, `sessionId`, `renderer` —
/// daemon-owned fields that drift from the real daemon state and corrupt
/// the layout over time. v2 keeps only `paneGroupId` plus heartbeat
/// metadata (which the daemon's list endpoint doesn't yet expose).
///
/// Walks every paneGroup item in `layout` and rewrites terminal items to
/// the v2 shape. Returns `true` if the layout was modified (v1 input).
/// Idempotent — a `version: 2` layout is a no-op.
fn migrate_layout_to_v2(layout: &mut Value) -> bool {
    let current_version = layout
        .get("version")
        .and_then(|v| v.as_u64())
        .unwrap_or(1);
    if current_version >= LAYOUT_SCHEMA_VERSION {
        return false;
    }

    let migrate_tab = |tab: &mut Value| {
        let Some(pgs) = tab.get_mut("paneGroups").and_then(|v| v.as_object_mut()) else {
            return;
        };
        for (pg_id, pg) in pgs.iter_mut() {
            let Some(items) = pg.get_mut("items").and_then(|v| v.as_array_mut()) else {
                continue;
            };
            for item in items.iter_mut() {
                let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if kind != "terminal" {
                    continue;
                }
                // Build a fresh v2 object preserving only the canonical
                // fields. id + type are always present; paneGroupId is
                // the parent pgId; heartbeat metadata is carried over
                // when present.
                let id = item.get("id").cloned().unwrap_or(Value::Null);
                let mut v2 = serde_json::Map::new();
                v2.insert("id".to_string(), id);
                v2.insert("type".to_string(), Value::String("terminal".to_string()));
                v2.insert("paneGroupId".to_string(), Value::String(pg_id.clone()));
                for field in [
                    "heartbeatName",
                    "surfacedAgentName",
                    "attachAgentName",
                    "projectPath",
                ] {
                    if let Some(v) = item.get(field) {
                        if !v.is_null() {
                            v2.insert(field.to_string(), v.clone());
                        }
                    }
                }
                *item = Value::Object(v2);
            }
        }
    };

    if let Some(tabs) = layout.get_mut("tabs").and_then(|v| v.as_array_mut()) {
        for tab in tabs.iter_mut() {
            migrate_tab(tab);
        }
    }
    if let Some(extra) = layout.get_mut("extraGroups").and_then(|v| v.as_array_mut()) {
        for group in extra.iter_mut() {
            if let Some(group_tabs) = group.get_mut("tabs").and_then(|v| v.as_array_mut()) {
                for tab in group_tabs.iter_mut() {
                    migrate_tab(tab);
                }
            }
        }
    }

    layout["version"] = Value::Number(LAYOUT_SCHEMA_VERSION.into());
    true
}

/// One-shot v1→v2 schema migration. Same gating pattern as
/// `run_once`: `code_migrations` marker stamped on completion, so the
/// pass is cheap on every subsequent boot.
pub fn run_v2_emit_once() {
    let db = k2_core::db::shared();
    let conn = db.lock();

    if k2_core::db::has_code_migration_applied(&conn, MIGRATION_V2_ID) {
        return;
    }

    log_debug!(
        "[daemon/boot] running {MIGRATION_V2_ID} pass over workspace_layouts"
    );

    let mut rows: Vec<(String, String, String, String)> = Vec::new();
    match conn.prepare(
        "SELECT id, project_id, workspace_id, layout_json FROM workspace_layouts",
    ) {
        Ok(mut stmt) => {
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            });
            if let Ok(it) = mapped {
                for r in it.flatten() {
                    rows.push(r);
                }
            }
        }
        Err(e) => {
            log_debug!("[daemon/boot] {MIGRATION_V2_ID}: SELECT failed: {e}");
            return;
        }
    }

    let total = rows.len();
    let mut migrated = 0usize;
    for (id, project_id, workspace_id, json_str) in rows {
        let mut layout: Value = match serde_json::from_str(&json_str) {
            Ok(v) => v,
            Err(e) => {
                log_debug!(
                    "[daemon/boot] {MIGRATION_V2_ID}: skip row id={id} (parse: {e})"
                );
                continue;
            }
        };

        if !migrate_layout_to_v2(&mut layout) {
            continue;
        }

        let new_json = match serde_json::to_string(&layout) {
            Ok(s) => s,
            Err(e) => {
                log_debug!(
                    "[daemon/boot] {MIGRATION_V2_ID}: skip row id={id} (re-serialize: {e})"
                );
                continue;
            }
        };

        match conn.execute(
            "UPDATE workspace_layouts SET layout_json = ?1, updated_at = unixepoch() WHERE id = ?2",
            rusqlite::params![new_json, id],
        ) {
            Ok(_) => {
                migrated += 1;
                log_debug!(
                    "[daemon/boot] {MIGRATION_V2_ID}: migrated project={project_id} workspace={workspace_id} → v2"
                );
            }
            Err(e) => {
                log_debug!(
                    "[daemon/boot] {MIGRATION_V2_ID}: UPDATE failed for id={id}: {e}"
                );
            }
        }
    }

    let notes = format!("rows_scanned={total} rows_migrated={migrated}");
    k2_core::db::mark_code_migration_applied(&conn, MIGRATION_V2_ID, Some(&notes));
    log_debug!("[daemon/boot] {MIGRATION_V2_ID} complete: {notes}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dedup_collapses_tabs_sharing_panegroups() {
        let mut layout = json!({
            "tabs": [
                { "id": "t1", "title": "Chat",   "paneGroups": { "pg-a": {} } },
                { "id": "t2", "title": "Claude", "paneGroups": { "pg-b": {} } },
                { "id": "t3", "title": "Claude", "paneGroups": { "pg-b": {} } },
                { "id": "t4", "title": "Claude", "paneGroups": { "pg-b": {} } }
            ],
            "activeTabId": "t3"
        });
        let (changed, removed) = dedup_layout_json(&mut layout);
        assert!(changed);
        assert_eq!(removed, 2);
        let tabs = layout["tabs"].as_array().unwrap();
        assert_eq!(tabs.len(), 2);
        // activeTabId t3 was a dup of t2 → remapped
        assert_eq!(layout["activeTabId"].as_str().unwrap(), "t2");
    }

    #[test]
    fn dedup_noop_on_clean_layout() {
        let mut layout = json!({
            "tabs": [
                { "id": "t1", "paneGroups": { "pg-a": {} } },
                { "id": "t2", "paneGroups": { "pg-b": {} } }
            ],
            "activeTabId": "t1"
        });
        let before = layout.clone();
        let (changed, removed) = dedup_layout_json(&mut layout);
        assert!(!changed);
        assert_eq!(removed, 0);
        assert_eq!(layout, before);
    }

    #[test]
    fn dedup_handles_extra_groups() {
        let mut layout = json!({
            "tabs": [
                { "id": "t1", "paneGroups": { "pg-a": {} } }
            ],
            "extraGroups": [
                {
                    "tabs": [
                        { "id": "g1t1", "paneGroups": { "pg-x": {} } },
                        { "id": "g1t2", "paneGroups": { "pg-x": {} } }
                    ],
                    "activeTabId": "g1t2"
                }
            ]
        });
        let (changed, removed) = dedup_layout_json(&mut layout);
        assert!(changed);
        assert_eq!(removed, 1);
        let g0 = layout["extraGroups"][0]["tabs"].as_array().unwrap();
        assert_eq!(g0.len(), 1);
        assert_eq!(layout["extraGroups"][0]["activeTabId"].as_str().unwrap(), "g1t1");
    }

    #[test]
    fn v2_emit_strips_daemon_owned_fields_from_terminal_items() {
        let mut layout = json!({
            "tabs": [
                {
                    "id": "t1",
                    "title": "Claude",
                    "paneGroups": {
                        "pg-a": {
                            "id": "pg-a",
                            "items": [
                                {
                                    "id": "it-1",
                                    "type": "terminal",
                                    "cwd": "/Users/z3thon/proj",
                                    "command": "claude",
                                    "args": ["--resume", "abc"],
                                    "sessionId": "abc-123",
                                    "renderer": "alacritty-v2",
                                    "heartbeatName": "hb-foo",
                                    "surfacedAgentName": "agent-bar",
                                    "attachAgentName": "tab-attach-baz",
                                    "projectPath": "/Users/z3thon/proj"
                                }
                            ],
                            "activeItemIndex": 0
                        }
                    }
                }
            ],
            "activeTabId": "t1"
        });
        let changed = migrate_layout_to_v2(&mut layout);
        assert!(changed);
        assert_eq!(layout["version"].as_u64().unwrap(), 2);
        let item = &layout["tabs"][0]["paneGroups"]["pg-a"]["items"][0];
        // Daemon-owned fields are gone.
        assert!(item.get("cwd").is_none());
        assert!(item.get("command").is_none());
        assert!(item.get("args").is_none());
        assert!(item.get("sessionId").is_none());
        assert!(item.get("renderer").is_none());
        // Canonical key + identifier preserved.
        assert_eq!(item["id"].as_str().unwrap(), "it-1");
        assert_eq!(item["type"].as_str().unwrap(), "terminal");
        assert_eq!(item["paneGroupId"].as_str().unwrap(), "pg-a");
        // Heartbeat metadata kept (load-bearing for close-as-minimize).
        assert_eq!(item["heartbeatName"].as_str().unwrap(), "hb-foo");
        assert_eq!(item["surfacedAgentName"].as_str().unwrap(), "agent-bar");
        assert_eq!(item["attachAgentName"].as_str().unwrap(), "tab-attach-baz");
        assert_eq!(item["projectPath"].as_str().unwrap(), "/Users/z3thon/proj");
    }

    #[test]
    fn v2_emit_is_idempotent_on_v2_layout() {
        let mut layout = json!({
            "version": 2,
            "tabs": [
                {
                    "id": "t1",
                    "title": "Claude",
                    "paneGroups": {
                        "pg-a": {
                            "id": "pg-a",
                            "items": [
                                {
                                    "id": "it-1",
                                    "type": "terminal",
                                    "paneGroupId": "pg-a"
                                }
                            ],
                            "activeItemIndex": 0
                        }
                    }
                }
            ],
            "activeTabId": "t1"
        });
        let before = layout.clone();
        let changed = migrate_layout_to_v2(&mut layout);
        assert!(!changed);
        assert_eq!(layout, before);
    }

    #[test]
    fn v2_emit_preserves_agent_and_file_viewer_items() {
        let mut layout = json!({
            "tabs": [
                {
                    "id": "t1",
                    "title": "Agent",
                    "paneGroups": {
                        "pg-a": {
                            "id": "pg-a",
                            "items": [
                                {
                                    "id": "it-1",
                                    "type": "agent",
                                    "agentName": "rust-eng",
                                    "projectPath": "/p",
                                    "section": "chat",
                                    "sessionId": "session-1"
                                },
                                {
                                    "id": "it-2",
                                    "type": "file-viewer",
                                    "filePath": "/p/README.md",
                                    "pinned": true,
                                    "scrollTop": 42,
                                    "cursorPos": 100
                                }
                            ],
                            "activeItemIndex": 0
                        }
                    }
                }
            ],
            "activeTabId": "t1"
        });
        let agent_before = layout["tabs"][0]["paneGroups"]["pg-a"]["items"][0].clone();
        let fv_before = layout["tabs"][0]["paneGroups"]["pg-a"]["items"][1].clone();
        let changed = migrate_layout_to_v2(&mut layout);
        assert!(changed);
        assert_eq!(layout["version"].as_u64().unwrap(), 2);
        // Non-terminal items survive unchanged.
        assert_eq!(layout["tabs"][0]["paneGroups"]["pg-a"]["items"][0], agent_before);
        assert_eq!(layout["tabs"][0]["paneGroups"]["pg-a"]["items"][1], fv_before);
    }

    #[test]
    fn v2_emit_migrates_extra_groups_too() {
        let mut layout = json!({
            "tabs": [],
            "extraGroups": [
                {
                    "tabs": [
                        {
                            "id": "g1t1",
                            "paneGroups": {
                                "pg-x": {
                                    "id": "pg-x",
                                    "items": [
                                        {
                                            "id": "it-9",
                                            "type": "terminal",
                                            "cwd": "/tmp",
                                            "command": "bash"
                                        }
                                    ],
                                    "activeItemIndex": 0
                                }
                            }
                        }
                    ],
                    "activeTabId": "g1t1"
                }
            ]
        });
        let changed = migrate_layout_to_v2(&mut layout);
        assert!(changed);
        let item = &layout["extraGroups"][0]["tabs"][0]["paneGroups"]["pg-x"]["items"][0];
        assert!(item.get("cwd").is_none());
        assert!(item.get("command").is_none());
        assert_eq!(item["paneGroupId"].as_str().unwrap(), "pg-x");
    }

    #[test]
    fn v2_emit_marks_version_when_missing_or_v1() {
        // Missing version → treated as v1.
        let mut layout = json!({ "tabs": [], "activeTabId": null });
        assert!(migrate_layout_to_v2(&mut layout));
        assert_eq!(layout["version"].as_u64().unwrap(), 2);

        // Explicit version: 1.
        let mut layout = json!({ "version": 1, "tabs": [], "activeTabId": null });
        assert!(migrate_layout_to_v2(&mut layout));
        assert_eq!(layout["version"].as_u64().unwrap(), 2);
    }
}
