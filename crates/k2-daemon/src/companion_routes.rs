//! H4 of Phase 4 — daemon-side `/cli/companion/sessions` +
//! `/cli/companion/projects-summary`.
//!
//! Both endpoints enumerate live sessions across every workspace
//! and project them against the `projects` table so companion
//! clients (mobile, web viewer, desktop sidebar) can show a
//! cross-workspace summary without reaching into the Tauri
//! process.
//!
//! Before Phase 4, these lived in Tauri's `agent_hooks.rs` and
//! walked `AppState.terminal_manager.list_terminal_ids()` — which
//! only knew about Tauri-spawned terminals. With session ownership
//! now daemon-side (Phase 3.1 onward), those walks miss every
//! daemon-owned session. H4 rewrites the logic to source live
//! sessions from `session_map::snapshot()`.
//!
//! **Response shapes match the legacy Tauri endpoints** so
//! existing companion clients don't need to branch:
//!
//! - `sessions` → array of per-session records with workspace
//!   attribution (matched by longest cwd-prefix).
//! - `projects-summary` → one record per project with counts,
//!   focus-group info, and pending-review tallies.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::companion_host;
use crate::session_lookup;

/// Minimal projects row shape the companion routes need.
struct ProjectRow {
    id: String,
    name: String,
    path: String,
    color: String,
}

fn list_projects() -> Vec<ProjectRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    // Skip sentinel rows (_orphan, _broadcast) — audit buckets, not
    // real workspaces. Their string "paths" would never match a
    // real cwd anyway, but filtering at SQL-time is cheaper.
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, name, path, color FROM projects \
         WHERE id NOT IN ('_orphan', '_broadcast') \
         ORDER BY name ASC",
    ) else {
        return Vec::new();
    };
    let rows = stmt
        .query_map([], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                color: row.get(3).unwrap_or_default(),
            })
        })
        .ok();
    let Some(rows) = rows else { return Vec::new() };
    rows.filter_map(Result::ok).collect()
}

/// Projects row + focus-group fields for the summary endpoint.
struct ProjectSummaryRow {
    id: String,
    name: String,
    path: String,
    color: String,
    agent_mode: String,
    pinned: bool,
    tab_order: i32,
    fg_id: Option<String>,
    fg_name: Option<String>,
    fg_color: Option<String>,
}

fn list_projects_for_summary() -> Vec<ProjectSummaryRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    // Sentinel rows (`_orphan`, `_broadcast`) are audit-bucket
    // infrastructure seeded by `db::seed_audit_sentinels`; they
    // aren't real workspaces and shouldn't clutter companion UIs.
    // Filter in SQL to keep the response clean.
    let Ok(mut stmt) = conn.prepare(
        "SELECT p.id, p.name, p.path, p.color, p.agent_mode, p.pinned, p.tab_order, \
                p.focus_group_id, fg.name, fg.color \
         FROM projects p \
         LEFT JOIN focus_groups fg ON p.focus_group_id = fg.id \
         WHERE p.id NOT IN ('_orphan', '_broadcast') \
         ORDER BY p.pinned DESC, p.tab_order ASC, p.name ASC",
    ) else {
        return Vec::new();
    };
    let rows = stmt
        .query_map([], |row| {
            Ok(ProjectSummaryRow {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                color: row.get(3).unwrap_or_default(),
                agent_mode: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                pinned: row.get::<_, i64>(5).unwrap_or(0) == 1,
                tab_order: row.get::<_, i64>(6).unwrap_or(0) as i32,
                fg_id: row.get(7)?,
                fg_name: row.get(8)?,
                fg_color: row.get(9)?,
            })
        })
        .ok();
    let Some(rows) = rows else { return Vec::new() };
    rows.filter_map(Result::ok).collect()
}

/// Match a session's cwd to the workspace whose path is its
/// longest matching prefix. Returns `None` if no project
/// contains the cwd. "Longest prefix wins" covers the common
/// case of nested projects (e.g. `/foo` and `/foo/bar` both
/// registered; a session in `/foo/bar/deep` belongs to
/// `/foo/bar`).
fn match_workspace<'a>(
    cwd: &str,
    projects: &'a [ProjectRow],
) -> Option<&'a ProjectRow> {
    projects
        .iter()
        .filter(|p| cwd.starts_with(&p.path))
        .max_by_key(|p| p.path.len())
}

/// Derive (agent_name, label) for a session's display. The
/// legacy rules:
///
/// 1. If cwd is under `<ws_path>/.k2so/worktrees/<name>/...` →
///    the first path component after `worktrees/` is both agent
///    and label. Matches the delegate-creates-worktree pattern.
/// 2. Otherwise use `<cwd>`'s final path component as the label
///    and `"shell"` as the agent name.
///
/// H4 adds a third rule: if the daemon's session_map already
/// tagged the session with a real agent_name (not the
/// `terminal-<hex>` synthesized form from H3's background
/// spawns), prefer that over cwd-derivation. Gives named
/// sessions a stable label even when they run in the workspace
/// root.
fn derive_agent_and_label(
    session_agent_name: &str,
    cwd: &str,
    ws_path: &str,
    ws_name: &str,
) -> (String, String) {
    let worktree_prefix = format!(
        "{}/worktrees/",
        k2_core::workspace_dot_dir(ws_path).display()
    );
    if let Some(rest) = cwd.strip_prefix(&worktree_prefix) {
        let name = rest.split('/').next().unwrap_or("agent");
        return (name.to_string(), name.to_string());
    }
    // Prefer the session_map-registered agent name when it's a
    // real agent (not a background-spawn synthetic like
    // "terminal-abcd1234").
    if !session_agent_name.is_empty() && !session_agent_name.starts_with("terminal-") {
        return (session_agent_name.to_string(), session_agent_name.to_string());
    }
    let folder = std::path::Path::new(cwd)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| ws_name.to_string());
    ("shell".to_string(), folder)
}

/// Find the first object key named `tabs` whose value is an array,
/// searching the top-level object and one level of nesting
/// (`extraGroups[*].tabs`). Layout JSON shape varies across versions —
/// walk it defensively rather than deserializing into a fixed struct.
fn find_tabs_array(layout: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    if let Some(tabs) = layout.get("tabs").and_then(|v| v.as_array()) {
        return Some(tabs);
    }
    // Nested form: { extraGroups: [ { tabs: [...] }, ... ] }.
    if let Some(groups) = layout.get("extraGroups").and_then(|v| v.as_array()) {
        for g in groups {
            if let Some(tabs) = g.get("tabs").and_then(|v| v.as_array()) {
                return Some(tabs);
            }
        }
    }
    None
}

/// Canonical resolution for a tab-driven session's display title.
/// Built once per request from `workspace_layouts.layout_json`:
/// `project_id -> (pane_group_id -> (tab_id, tab_title))`.
///
/// A tab-driven session's `agent_name` is `tab-<pane_group_id>`, so
/// the mobile companion can map its live session back to the SAME
/// canonical tab the desktop renders (and rename it via
/// `(projectId, tabId)`).
type TabIndex = HashMap<String, HashMap<String, (String, String)>>;

/// `project_id -> tab_id -> canonical title` from the authoritative
/// `tab_titles` table (what `set-tab-title` writes). Overlaid on the layout
/// tab.title so a rename shows up immediately: the layout blob lags until the
/// renderer re-saves it, and a mobile-only rename never touches the blob.
fn build_tab_titles_index() -> HashMap<String, HashMap<String, String>> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare("SELECT project_id, tab_id, title FROM tab_titles") else {
        return HashMap::new();
    };
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .ok();
    let Some(rows) = rows else { return HashMap::new() };
    let mut idx: HashMap<String, HashMap<String, String>> = HashMap::new();
    for (pid, tid, title) in rows.filter_map(Result::ok) {
        idx.entry(pid).or_default().insert(tid, title);
    }
    idx
}

fn build_tab_index() -> TabIndex {
    let mut index: TabIndex = HashMap::new();
    let Ok(layouts) = k2_core::db_ops::workspace_layout_load_all() else {
        return index;
    };
    for layout in layouts {
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&layout.layout_json) else {
            continue;
        };
        let Some(tabs) = find_tabs_array(&parsed) else {
            continue;
        };
        let per_project = index.entry(layout.project_id.clone()).or_default();
        for tab in tabs {
            let tab_id = tab.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            if tab_id.is_empty() {
                continue;
            }
            let tab_title = tab
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let Some(pane_groups) = tab.get("paneGroups").and_then(|v| v.as_object()) else {
                continue;
            };
            for pane_group_id in pane_groups.keys() {
                per_project.insert(
                    pane_group_id.clone(),
                    (tab_id.to_string(), tab_title.clone()),
                );
            }
        }
    }
    index
}

/// Handler for `GET /cli/companion/sessions`.
///
/// Global session enumeration — every live session in the
/// daemon, joined against `projects` by longest cwd-prefix.
/// Sessions whose cwd doesn't match any registered project are
/// dropped (same behavior as the legacy Tauri endpoint).
///
/// Tab-driven sessions (`agent_name == "tab-<paneGroupId>"`) resolve
/// their `label` to the CANONICAL tab title from
/// `workspace_layouts` — the same value the desktop shows — and carry
/// `tabId` + `projectId` so the mobile companion can rename the same
/// canonical tab. Non-tab sessions (e.g. pinned chat) keep the
/// derived label and omit `tabId`.
pub fn handle_companion_sessions(_params: &HashMap<String, String>) -> CliResponse {
    let projects = list_projects();
    let tab_index = build_tab_index();
    let tab_titles_index = build_tab_titles_index();
    let live = session_lookup::snapshot_all();
    let mut out: Vec<serde_json::Value> = Vec::with_capacity(live.len());
    for (agent_name, session) in live {
        let cwd = session.cwd();
        let Some(ws) = match_workspace(&cwd, &projects) else {
            continue;
        };
        let (agent, derived_label) =
            derive_agent_and_label(&agent_name, &cwd, &ws.path, &ws.name);

        // The pinned/main chat session is keyed by `agent_name ==
        // project_id` (v2_spawn `is_pinned`). `ws.id` is that project_id.
        let is_main_chat = agent_name == ws.id;

        // Canonical tab-title resolution for tab-driven sessions.
        let mut label = derived_label;
        let mut tab_id: Option<String> = None;
        if let Some(pgid) = agent_name.strip_prefix("tab-") {
            if let Some((resolved_tab_id, resolved_title)) =
                tab_index.get(&ws.id).and_then(|m| m.get(pgid))
            {
                tab_id = Some(resolved_tab_id.clone());
                // Canonical title = the authoritative `tab_titles` row (what
                // set-tab-title writes — fresh after a rename from EITHER
                // client) overlaid on the layout's tab.title (which lags and
                // never updates from a mobile-only rename).
                let canonical = tab_titles_index
                    .get(&ws.id)
                    .and_then(|m| m.get(resolved_tab_id))
                    .filter(|t| !t.is_empty());
                if let Some(t) = canonical {
                    label = t.clone();
                } else if !resolved_title.is_empty() {
                    label = resolved_title.clone();
                }
            }
        }

        out.push(serde_json::json!({
            "workspaceName": ws.name,
            "workspaceId": ws.id,
            "workspaceColor": ws.color,
            "agentName": agent,
            "label": label,
            "isMainChat": is_main_chat,
            "tabId": tab_id,
            "projectId": ws.id,
            "terminalId": session.session_id().to_string(),
            "command": session.command(),
            "cwd": cwd,
        }));
    }
    CliResponse::ok_json(serde_json::to_string(&out).unwrap_or_else(|_| "[]".into()))
}

/// Handler for `GET /cli/companion/projects-summary`.
///
/// One record per registered project with live-session counts
/// and pending-review counts. Matches the pre-Phase-4 Tauri shape
/// byte-for-byte so the companion UI can swap endpoints without
/// change.
///
/// Review-pending count routes through
/// `k2_core::workspace::reviews::review_queue` — the same
/// canonical source the Tauri `k2so_agents_review_queue` command
/// uses. Pre-Phase-2.5b this walked `<project>/.k2so/agents/*/work/done/`
/// on the filesystem; that directory tree was retired by the 2.5b
/// unification migration, so on every upgraded workspace the walk
/// returned 0 (silent — no error) and the companion's "Pending
/// Reviews" badge always showed zero.
pub fn handle_companion_projects_summary(
    _params: &HashMap<String, String>,
) -> CliResponse {
    let workspaces = list_projects_for_summary();
    let live_sessions = session_lookup::snapshot_all();

    // Tally live sessions per workspace via longest-prefix match,
    // identical to the /cli/companion/sessions grouping rule.
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (_, session) in &live_sessions {
        let cwd = session.cwd();
        let matched = workspaces
            .iter()
            .filter(|p| cwd.starts_with(&p.path))
            .max_by_key(|p| p.path.len());
        if let Some(ws) = matched {
            *counts.entry(ws.id.clone()).or_insert(0) += 1;
        }
    }

    let mut summaries: Vec<serde_json::Value> = Vec::with_capacity(workspaces.len());
    for ws in &workspaces {
        let review_count = count_pending_reviews(&ws.path);
        let focus_group = match (&ws.fg_id, &ws.fg_name) {
            (Some(id), Some(name)) => serde_json::json!({
                "id": id,
                "name": name,
                "color": ws.fg_color,
            }),
            _ => serde_json::Value::Null,
        };
        summaries.push(serde_json::json!({
            "id": ws.id,
            "name": ws.name,
            "path": ws.path,
            "color": ws.color,
            "agentMode": ws.agent_mode,
            "pinned": ws.pinned,
            "tabOrder": ws.tab_order,
            "focusGroup": focus_group,
            "agentsRunning": counts.get(&ws.id).copied().unwrap_or(0),
            "reviewsPending": review_count,
        }));
    }
    CliResponse::ok_json(
        serde_json::to_string(&summaries).unwrap_or_else(|_| "[]".into()),
    )
}

/// Body shape accepted by `POST /cli/companion/set-password`. The
/// renderer (and any other client) sends `{"password": "..."}`. We
/// hash the raw password into argon2id, try to store the hash in the
/// macOS Keychain (preferred), and fall back to settings.json when
/// the Keychain is unavailable. Either way, every live companion
/// session is invalidated so a rotated password can't be used with a
/// stale token.
#[derive(serde::Deserialize)]
struct SetPasswordBody {
    password: String,
}

/// Handler for `POST /cli/companion/set-password`.
///
/// Phase 2 Unit 1 — daemon equivalent of the Tauri-side
/// `companion_set_password` command. Lives in the daemon so a
/// remote client (K2SO Connect, Mobile Companion's "rotate password"
/// flow) can drive it without going through a Tauri shim.
pub fn handle_companion_set_password(body: &[u8]) -> CliResponse {
    let parsed: SetPasswordBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if parsed.password.is_empty() {
        return CliResponse::bad_request("password must not be empty");
    }
    let hash = match k2_core::companion::auth::hash_password(&parsed.password) {
        Ok(h) => h,
        Err(e) => return CliResponse::bad_request(e),
    };

    // Prefer the macOS Keychain. On success the on-disk copy gets
    // blanked so only the Keychain entry is authoritative.
    let keychain_ok =
        k2_core::companion::keychain::write_password_hash(&hash).is_ok();
    let (persisted_hash, persisted_flag) = if keychain_ok {
        ("", true)
    } else {
        (hash.as_str(), true)
    };
    if let Err(e) =
        companion_host::persist_companion_password_fields(persisted_hash, persisted_flag)
    {
        return CliResponse::bad_request(format!("failed to persist password: {e}"));
    }

    // Tauri's settings_update path would invalidate sessions itself
    // on password_hash change; we mirror that directly here so the
    // Keychain branch (which writes an empty hash) still kicks live
    // tokens.
    k2_core::companion::invalidate_all_sessions("password changed");

    CliResponse::ok_json(r#"{"success":true}"#.to_string())
}

/// Body shape accepted by `POST /cli/companion/disconnect-session`.
/// Accepts `sessionToken` (camelCase, matches the renderer's existing
/// payload shape) OR `session_token` (snake_case, for CLI clients).
#[derive(serde::Deserialize)]
struct DisconnectSessionBody {
    #[serde(rename = "sessionToken", alias = "session_token")]
    session_token: String,
}

/// Handler for `POST /cli/companion/disconnect-session`.
///
/// Phase 2 Unit 1 — removes a single live companion session row plus
/// any WS clients still attached to that token. Idempotent: an
/// unknown token returns success (the goal is "this token cannot
/// authenticate any more", which is trivially true if it never
/// could).
pub fn handle_companion_disconnect_session(body: &[u8]) -> CliResponse {
    let parsed: DisconnectSessionBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if parsed.session_token.is_empty() {
        return CliResponse::bad_request("sessionToken must not be empty");
    }
    let guard = k2_core::companion::STATE.lock();
    let state = match guard.as_ref() {
        Some(s) => s,
        None => return CliResponse::bad_request("Companion is not running"),
    };
    state.sessions.lock().remove(&parsed.session_token);
    state
        .ws_clients
        .lock()
        .retain(|c| c.session_token != parsed.session_token);
    CliResponse::ok_json(r#"{"success":true}"#.to_string())
}

/// Count items on the review queue for a given project.
///
/// Delegates to `k2_core::workspace::reviews::review_queue` — the
/// canonical "items awaiting review" surface that powers the Tauri
/// `k2so_agents_review_queue` command and the daemon's
/// `/cli/reviews` endpoint. We sum the `work_items` lengths so the
/// count matches what a human reviewer sees in the panel (one entry
/// per done-item, not per agent-with-done-items).
///
/// Returns 0 if the project hasn't been registered, the agents
/// directory is missing, or the queue is empty — a broken
/// filesystem or a fresh workspace doesn't 500 the endpoint.
///
/// **Pre-2.5b behaviour (replaced).** Used to walk
/// `<project>/.k2so/agents/*/work/done/` directly. That tree was
/// retired by the 2.5b unification migration so the walk silently
/// returned 0 on every upgraded workspace, leaving the mobile
/// companion's pending-reviews badge perpetually stuck on zero.
fn count_pending_reviews(project_path: &str) -> usize {
    k2_core::workspace::reviews::review_queue(project_path)
        .map(|items| items.iter().map(|i| i.work_items.len()).sum())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn unique_tmp_project() -> std::path::PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!(
            "k2so-companion-routes-test-{}-{}-{}",
            std::process::id(),
            nanos,
            n
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// `count_pending_reviews` must not panic on a path that doesn't
    /// resolve to a registered project — the silent-zero behaviour is
    /// the contract for fresh/unregistered workspaces.
    #[test]
    fn count_pending_reviews_returns_zero_for_unknown_project() {
        let _ = k2_core::db::init_for_tests();
        let p = unique_tmp_project();
        let count = count_pending_reviews(&p.to_string_lossy());
        assert_eq!(count, 0);
        let _ = std::fs::remove_dir_all(&p);
    }

    /// Regression for the pre-Phase-2.5b silent-zero bug. The
    /// companion's pending-reviews badge always returned 0 because
    /// `review_queue` walked `.k2so/agents/<n>/work/done/` — a
    /// directory retired by the 2.5b unification migration. With
    /// `review_queue` migrated to read from the workspace's unified
    /// inbox (`.k2so/inbox/done/`), seeding a done item there must
    /// produce a non-zero count via the canonical surface.
    ///
    /// (Pre-fix this test seeded the LEGACY `.k2so/agents/<n>/work/done/`
    /// shape and still passed because `review_queue` was reading from
    /// it. After the 0.39.0 review_queue migration, the canonical
    /// surface ignores the legacy tree entirely — so the test seeds
    /// the unified-inbox shape to exercise the new code path.)
    #[test]
    fn count_pending_reviews_uses_review_queue_as_source_of_truth() {
        let _ = k2_core::db::init_for_tests();
        let p = unique_tmp_project();

        // Register the project so `review_queue` can do its scan.
        let project_id = format!(
            "proj-companion-review-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::SeqCst)
        );
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT OR REPLACE INTO projects \
                 (id, path, name, color, agent_mode, pinned, tab_order) \
                 VALUES (?1, ?2, ?3, '#123456', 'manager', 0, 0)",
                rusqlite::params![project_id, p.to_string_lossy(), "test"],
            )
            .unwrap();
        }

        // Seed the post-2.5b unified-inbox shape. `review_queue` now
        // walks `<workspace>/.k2so/inbox/done/*.md` (NOT the retired
        // `.k2so/agents/<n>/work/done/` tree), so the seed file lands
        // here. Frontmatter carries the `branch:` field that
        // `review_queue` parses to group items by agent.
        let inbox_done = p.join(".k2so/inbox/done");
        std::fs::create_dir_all(&inbox_done).unwrap();
        std::fs::write(
            inbox_done.join("oauth.md"),
            "---\ntitle: OAuth\nbranch: agent/backend/oauth\n---\n\nbody",
        )
        .unwrap();

        let count = count_pending_reviews(&p.to_string_lossy());
        assert!(
            count >= 1,
            "expected >=1 from canonical review_queue (got {count})",
        );

        let _ = std::fs::remove_dir_all(&p);
    }
}
