//! Pre-approved App cores, the **Use** layer (prd-agent-app-permissions-v1
//! rev 2 §3, slice B; 0.46.3, Rosson 2026-10-20).
//!
//! A checkbox per core on the agent's workspace card (`projects.app_cores`,
//! migration 0197): **Database**, **Files/rooms**, **Thread**, **Mail**
//! (Stalwart) and **Sew**. A ticked core lets the agent's Apps use that
//! core inside their own slice; the agent scopes its App users within it.
//!
//! - **Who ticks.** The server's owner ticks (raises); an Admin may only
//!   untick (Q2); an agent never changes its own. One writer:
//!   `POST /cli/agent-access/set {toggle: "app_core"}` ([`set_cores`]),
//!   which writes the Access Audit line (the caller), an `access_changed`
//!   deploy-audit row per App and an inbox note to the agent, no wake.
//! - **What a tick does (enforced for Database, Files/rooms and Thread).**
//!   A deploy or first install whose new caps all sit in ticked cores is
//!   recorded and goes on at once, under switch 1, with no Approvals hold
//!   ([`pre_approves`]). Anything else is held for the owner, as before
//!   (S20), and the hold names the cores to tick ("tick Database for
//!   sales"). Switch 2 never adds a core. `dbPrivate` (the App's own
//!   database flag) applies for an agent only with Database ticked;
//!   `dbRaw` stays a person-present confirm.
//! - **Caps in no core** reach the agent itself (`heartbeats:write`,
//!   `files:agentconfig`, N3): never pre-approved, always the person's.
//! - **Mail** is stored and shown and takes effect when App mail
//!   (MCS11–12) lands. No App cap belongs to it yet, and App mail sends
//!   stay held for the owner even then (N4).
//! - **Sew** (enforced since Sew for Apps A1, 0.46.6): the chat caps
//!   (`chat:read|post|respond`, `voice:join`, `voice:video`) are its caps,
//!   so a ticked Sew pre-approves an App asking for them.
//! - **Unticking** stops future automatic additions. Caps an App already
//!   has stay until the owner narrows that App (Q4: nothing that runs
//!   loses a cap).
//! - **Admin of a core** (Postgres itself, Stalwart domains, Sew policies)
//!   is a separate owner switch (`db_agent_access`, `mail_manage`,
//!   `chat_manage`), never implied by a tick. Reach across (another App's
//!   slice) is slice K, 0.46.4+.

use std::collections::BTreeSet;

use k2_core::deploy;
use k2_core::workspace::settings::{agents_app_cores, app_core_of_cap, normalize_app_cores, set_agents_app_cores, APP_CORES};
use rusqlite::Connection;
use serde_json::{json, Value as J};

use super::access::{self, core_name, Level};
use super::routes::{no, ws_by_id, ws_handle, R};

/// Cores whose tick is enforced (Database, Files and Thread since 0.46.3;
/// Sew since Sew for Apps A1, 0.46.6).
pub const ENFORCED: &[&str] = &["db", "files", "thread", "sew"];

/// Is this core's tick enforced yet (Mail waits for its App slice)?
pub fn enforced(core: &str) -> bool {
    ENFORCED.contains(&core)
}

/// A core's label on the workspace card.
pub fn label(core: &str) -> &'static str {
    match core {
        "db" => "Database",
        "files" => "Files and rooms",
        "thread" => "Thread",
        "mail" => "Mail",
        "sew" => "Sew",
        _ => "?",
    }
}

/// What a core's tick does not do yet, in plain words.
pub fn later(core: &str) -> Option<&'static str> {
    match core {
        "mail" => Some("takes effect when App mail lands"),
        _ => None,
    }
}

/// The ticked cores of a workspace's agent.
pub fn ticked(ws: &str) -> Vec<String> {
    agents_app_cores(ws)
}

/// The cores as Settings and `k2 app access` show them.
pub fn cores_json(ws: &str) -> J {
    let t = ticked(ws);
    json!(APP_CORES
        .iter()
        .map(|c| json!({
            "core": c,
            "label": label(c),
            "ticked": t.iter().any(|x| x == c),
            "enforced": enforced(c),
            "later": later(c),
        }))
        .collect::<Vec<_>>())
}

/// A workspace's App access: both switches and the cores.
pub fn access_json(ws: &str) -> J {
    let l = access::level(ws);
    json!({
        "workspace": ws_handle(ws),
        "workspaceId": ws,
        "level": l.as_str(),
        "buildAndRun": l >= Level::Build,
        "manageEndToEnd": l == Level::Full,
        // "Let this agent manage its Apps' users" (0225): its own switch.
        "manageUsers": k2_core::workspace::settings::agents_can_manage_app_users(ws),
        "cores": ticked(ws),
        "coreList": cores_json(ws),
    })
}

// ── what new caps need ───────────────────────────────────────────────

/// What a set of new caps needs beyond the ticked cores.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Need {
    /// Cores to tick, in display order.
    pub cores: Vec<&'static str>,
    /// Caps that reach the agent itself or belong to no core: the person's.
    pub person: Vec<String>,
}

impl Need {
    pub fn is_empty(&self) -> bool {
        self.cores.is_empty() && self.person.is_empty()
    }
}

/// Pure: the cores `add` needs that `ticked` lacks, and the caps no tick
/// covers.
pub fn need(ticked: &[String], add: &[String]) -> Need {
    let have: BTreeSet<&str> = ticked.iter().map(|s| s.as_str()).collect();
    let mut missing: BTreeSet<&'static str> = BTreeSet::new();
    let mut person = Vec::new();
    for c in add.iter().map(|c| c.trim()) {
        // The loosening caps are in no core today; the filter keeps a future
        // map change from pre-approving one.
        match app_core_of_cap(c).filter(|_| !access::LOOSENING_CAPS.contains(&c)) {
            Some(core) => {
                if !have.contains(core) {
                    missing.insert(core);
                }
            }
            None => person.push(c.to_string()),
        }
    }
    Need { cores: APP_CORES.iter().copied().filter(|c| missing.contains(c)).collect(), person }
}

/// Do the ticked cores of the App's workspace pre-approve `add`? Only for
/// a workspace whose agent has switch 1 on (a parked App, `''`, never).
pub fn pre_approves(app_ws: &str, add: &[String]) -> bool {
    !app_ws.trim().is_empty() && access::level(app_ws) >= Level::Build && need(&ticked(app_ws), add).is_empty()
}

/// The reasons and `cores` diff a held cap request carries for the owner
/// and the agent: which cores to tick, which caps are the person's.
pub fn hold_detail(app_ws: &str, add: &[String]) -> (Vec<String>, J) {
    let t = ticked(app_ws);
    let n = need(&t, add);
    let handle = ws_handle(app_ws);
    let mut reasons = Vec::new();
    let switch_on = !app_ws.trim().is_empty() && access::level(app_ws) >= Level::Build;
    if !n.cores.is_empty() {
        let names: Vec<&str> = n.cores.iter().map(|c| core_name(c)).collect();
        let flags: Vec<String> = n.cores.iter().map(|c| format!("{c}=on")).collect();
        reasons.push(format!(
            "tick {} for {handle} to let its Apps use {} without asking (the owner: Settings → Workspaces → {handle} → \
             Agent → Apps, or k2 app access {handle} --core {} from a terminal outside K2)",
            names.join(" and "),
            if n.cores.len() == 1 { "it" } else { "them" },
            flags.join(","),
        ));
    }
    if !n.person.is_empty() {
        reasons.push(format!(
            "{} reach{} the agent itself: the owner approves {} in person",
            n.person.join(", "),
            if n.person.len() == 1 { "es" } else { "" },
            if n.person.len() == 1 { "it" } else { "them" },
        ));
    }
    if n.is_empty() && !switch_on && !app_ws.trim().is_empty() {
        reasons.push(format!("\"Build and run Apps\" is off for {handle}: its ticked cores apply only while it is on"));
    }
    let diff = json!({
        "ticked": t,
        "missing": n.cores,
        "person": n.person,
        "workspace": handle,
    });
    (reasons, diff)
}

/// The hint a held cap request starts with when a tick would have let it
/// through.
pub fn tick_hint(app_ws: &str, add: &[String]) -> Option<String> {
    let n = need(&ticked(app_ws), add);
    if n.cores.is_empty() || !n.person.is_empty() || app_ws.trim().is_empty() {
        return None;
    }
    let handle = ws_handle(app_ws);
    let names: Vec<&str> = n.cores.iter().map(|c| core_name(c)).collect();
    let flags: Vec<String> = n.cores.iter().map(|c| format!("{c}=on")).collect();
    Some(format!(
        "Tick {} for {handle} (k2 app access {handle} --core {}, the owner, from a terminal outside K2) and deploy again, or",
        names.join(" and "),
        flags.join(","),
    ))
}

// ── the writer ───────────────────────────────────────────────────────

/// Tick or untick cores for a workspace's agent: the owner ticks, an
/// Admin only unticks (Q2; the caller already refused agents and
/// Members). A change writes one `access_changed` row per App the
/// workspace maintains and leaves the agent an inbox note (no wake); the
/// route writes the Access Audit line.
pub(crate) fn set_cores(ws_id: &str, changes: &[(String, bool)], caller_is_owner: bool, by: &str) -> R<J> {
    let ws = ws_by_id(ws_id).ok_or_else(|| no("workspace_not_found", 404, format!("no workspace '{ws_id}'")))?;
    if changes.is_empty() {
        return Err(no(deploy::codes::BAD_REQUEST, 400, format!("name a core: {}", APP_CORES.join(", "))));
    }
    if let Some((bad, _)) = changes.iter().find(|(c, _)| !APP_CORES.contains(&c.trim())) {
        return Err(no(deploy::codes::BAD_REQUEST, 400, format!("unknown core '{bad}' (cores: {})", APP_CORES.join(", "))));
    }
    let before = ticked(&ws.id);
    let mut after = before.clone();
    for (c, on) in changes {
        let c = c.trim().to_string();
        if *on {
            if !after.contains(&c) {
                after.push(c);
            }
        } else {
            after.retain(|x| x != &c);
        }
    }
    let after = normalize_app_cores(&after);
    let raised: Vec<&String> = after.iter().filter(|c| !before.contains(c)).collect();
    if !raised.is_empty() && !caller_is_owner {
        return Err(no(
            deploy::codes::OWNER_ONLY,
            403,
            "only the server's owner ticks an App core for an agent (an Admin may untick one): Settings → Workspaces → \
             <agent> → Agent → Apps, or k2 app access <ws> --core <core>=on from a terminal outside K2",
        ));
    }
    let handle = ws_handle(&ws.id);
    let changed = after != before;
    if changed {
        set_agents_app_cores(&ws.id, &after).map_err(|e| no("workspace_not_found", 404, e))?;
        let apps = access::audit_apps(&ws.id, by, json!({ "workspace": handle, "cores": { "from": before, "to": after } }));
        k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncProjects, J::Null);
        k2_core::agent_hooks::emit(
            k2_core::agent_hooks::HookEvent::SyncSettings,
            json!({ "apps": "access", "workspace": handle, "cores": after, "appIds": apps }),
        );
        access::tell_agent(&ws.id, &handle, by);
    }
    let mut out = access_json(&ws.id);
    out["ok"] = json!(true);
    out["was"] = json!(before);
    out["changed"] = json!(changed);
    Ok(out)
}

// ── 0197's cap → core backfill, corrected ────────────────────────────

/// The one-time code migration id for [`fix_backfill_once`].
pub const BACKFILL_FIX_ID: &str = "0197_app_cores_backfill_fix";

/// The cores a workspace's live Apps imply: their approved caps mapped
/// with `map`, plus Sew when an App has the Sew box-service preset.
fn derive(conn: &Connection, ws: &str, map: fn(&str) -> Option<&'static str>) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    if let Ok(mut st) = conn.prepare("SELECT approved_caps_json FROM apps WHERE owner_workspace = ?1") {
        let sets: Vec<Option<String>> =
            st.query_map([ws], |r| r.get::<_, Option<String>>(0)).map(|it| it.flatten().collect()).unwrap_or_default();
        for raw in sets.into_iter().flatten() {
            let caps: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
            out.extend(caps.iter().filter_map(|c| map(c)).map(str::to_string));
        }
    }
    let sew: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM app_box_services b JOIN apps a ON a.app_id = b.app_id \
             WHERE a.owner_workspace = ?1 AND b.preset IN ('sew-api', 'sew-signal') AND b.status = 'allowed'",
            [ws],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if sew > 0 {
        out.insert("sew".into());
    }
    normalize_app_cores(&out.into_iter().collect::<Vec<_>>())
}

/// 0197's mapping as shipped: it put `heartbeats:write` under Thread and
/// `files:agentconfig` under Files.
fn shipped_0197_map(cap: &str) -> Option<&'static str> {
    match cap.trim() {
        "heartbeats:write" => Some("thread"),
        "files:agentconfig" => Some("files"),
        c => app_core_of_cap(c),
    }
}

/// Correct 0197's backfill once (boot, before the review note). The
/// migration mapped `heartbeats:write` (person-present, N3) to Thread and
/// `files:agentconfig` to Files; neither is in a core (§3.1). For each
/// upgraded workspace whose cores are still exactly what 0197 wrote, they
/// are re-derived with the right map; a row the owner changed since is
/// left alone. Returns the workspaces corrected.
pub fn fix_backfill_once() -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if k2_core::db::has_code_migration_applied(&conn, BACKFILL_FIX_ID) {
        return vec![];
    }
    let ids: Vec<String> = match conn.prepare("SELECT project_id FROM app_access_review ORDER BY project_id") {
        Ok(mut st) => st.query_map([], |r| r.get::<_, String>(0)).map(|it| it.flatten().collect()).unwrap_or_default(),
        Err(e) => {
            k2_core::log_debug!("[apps] cores backfill fix: {e}");
            return vec![];
        }
    };
    let mut fixed = Vec::new();
    for ws in ids {
        let current: Option<String> =
            conn.query_row("SELECT app_cores FROM projects WHERE id = ?1", [&ws], |r| r.get(0)).ok();
        let Some(current) = current else { continue };
        let current = normalize_app_cores(&serde_json::from_str::<Vec<String>>(&current).unwrap_or_default());
        let shipped = derive(&conn, &ws, shipped_0197_map);
        let right = derive(&conn, &ws, app_core_of_cap);
        if current == shipped && right != shipped {
            let json = serde_json::to_string(&right).unwrap_or_else(|_| "[]".into());
            if conn.execute("UPDATE projects SET app_cores = ?1 WHERE id = ?2", rusqlite::params![json, ws]).is_ok() {
                fixed.push(ws);
            }
        }
    }
    k2_core::db::mark_code_migration_applied(&conn, BACKFILL_FIX_ID, Some(&format!("fixed {}", fixed.len())));
    fixed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn need_names_the_cores_to_tick_and_the_persons_caps() {
        let n = need(&v(&["thread"]), &v(&["thread:read", "store:write", "files:read", "wiki:read"]));
        assert_eq!(n.cores, vec!["db", "files"], "display order, de-duplicated");
        assert!(n.person.is_empty());
        let n = need(&v(&["thread", "files"]), &v(&["heartbeats:write", "thread:post"]));
        assert_eq!((n.cores.len(), n.person), (0, v(&["heartbeats:write"])), "heartbeats:write is in no core");
        assert!(need(&v(&["db"]), &v(&["store:read", "store:write"])).is_empty());
        assert!(need(&[], &[]).is_empty(), "asking for nothing needs nothing");
        // Ticking Mail or Sew covers no Thread cap.
        let n = need(&v(&["mail", "sew"]), &v(&["thread:read"]));
        assert_eq!(n.cores, vec!["thread"]);
        // Sew for Apps A1: the chat caps are Sew's.
        let chat = v(&["chat:read", "chat:post", "voice:join", "voice:video"]);
        assert_eq!(need(&v(&["thread"]), &chat).cores, vec!["sew"]);
        assert!(need(&v(&["sew"]), &chat).is_empty(), "a ticked Sew pre-approves the chat caps");
    }

    #[test]
    fn the_shipped_0197_map_differs_only_on_the_agent_reaching_caps() {
        for cap in [
            "files:read", "files:write", "room:members", "wiki:read", "tickets:read", "tickets:post", "activity:read",
            "thread:read", "thread:post", "heartbeats:read", "agent:identity", "agent:handle", "store:read", "store:write",
        ] {
            assert_eq!(shipped_0197_map(cap), app_core_of_cap(cap), "{cap}");
        }
        assert_eq!(app_core_of_cap("heartbeats:write"), None);
        assert_eq!(app_core_of_cap("files:agentconfig"), None);
    }
}
