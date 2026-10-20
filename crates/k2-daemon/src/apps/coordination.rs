//! Coordination between agents on one box (prd-agent-app-permissions-v1
//! §4.1–4.2, F7, N6).
//!
//! - **The box view** (`GET /cli/app/list?box=1`, `k2 app list --box`):
//!   every App on the box with its maintaining agent and footprint — memory
//!   and disk budgets, hostnames, open ports, box services, the agent's
//!   switches. Any credential may read it, passports included. Never
//!   logs, env, caps detail, data, guests or folder paths.
//! - **Who maintains it**: [`maintainer_of`] names a workspace's agent by
//!   display name, handle and federated address (`handle::host`, what
//!   `k2 msg` takes), never its folder. Clash refusals (label, port,
//!   admission) carry it as `owner: {agent, handle, address}` so the
//!   caller can talk to that agent instead of guessing.

use std::collections::HashMap;

use k2_core::app_grace::config as appcfg;
use k2_core::deploy::store::{self, AppRow};
use rusqlite::Connection;
use serde_json::{json, Value as J};

use super::routes::{db_err, need_credential, no, Who, R};

/// A port another App already opens.
pub const PORT_OWNED: &str = "port_owned";

/// This box's Connect host (`<sub>.k2.dev`), when it has one.
pub fn this_host() -> Option<String> {
    let primary = k2_core::tunnel::subdomains::current().primary.trim().to_ascii_lowercase();
    let sub = if primary.is_empty() {
        k2_core::tunnel::config::load().map(|c| c.subdomain.trim().to_ascii_lowercase()).unwrap_or_default()
    } else {
        primary
    };
    if sub.is_empty() {
        None
    } else if sub.contains('.') {
        Some(sub)
    } else {
        Some(format!("{sub}.{}", k2_core::tunnel::config::SUBDOMAIN_HOST))
    }
}

/// Who maintains an App, as any agent may be told (never a path or id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Maintainer {
    /// The agent's display name.
    pub agent: Option<String>,
    /// Its handle on this box (`k2 msg <handle>`).
    pub handle: Option<String>,
    /// The federated address (`handle::host`), or the handle on a box
    /// with no Connect host.
    pub address: Option<String>,
    /// `agent` | `parked` (owner-held, no agent) | `removed` (the
    /// workspace is gone; the boot sweep parks it).
    pub kind: &'static str,
}

impl Maintainer {
    pub fn json(&self) -> J {
        json!({
            "agent": self.agent,
            "handle": self.handle,
            "address": self.address,
            "kind": self.kind,
            "msg": self.handle.as_ref().map(|h| format!("k2 msg {h} \"…\"")),
        })
    }

    /// Plain words: `maintained by Sales (sales::rosson.k2.dev)`.
    pub fn phrase(&self) -> String {
        match (self.kind, &self.agent, &self.address) {
            ("agent", Some(a), Some(addr)) => format!("maintained by {a} ({addr})"),
            ("parked", _, _) => "parked: held by the server's owner".into(),
            _ => "its workspace was removed: held by the server's owner".into(),
        }
    }
}

/// [`Maintainer`] for a workspace id, on a connection the caller holds.
pub fn maintainer_with(conn: &Connection, workspace_id: &str) -> Maintainer {
    let ws = workspace_id.trim();
    if ws.is_empty() {
        return Maintainer { agent: None, handle: None, address: None, kind: "parked" };
    }
    let path: Option<String> =
        conn.query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![ws], |r| r.get(0)).ok();
    let Some(path) = path else {
        return Maintainer { agent: None, handle: None, address: None, kind: "removed" };
    };
    let handle = k2_core::workspace_session_handles::workspace_address_name(conn, ws).ok();
    let agent = Some(k2_core::workspace::display::agent_display_name(&path));
    let address = handle.as_ref().map(|h| match this_host() {
        Some(host) => format!("{h}::{host}"),
        None => h.clone(),
    });
    Maintainer { agent, handle, address, kind: "agent" }
}

/// [`Maintainer`] for a workspace id (takes the shared DB).
pub fn maintainer_of(workspace_id: &str) -> Maintainer {
    let db = k2_core::db::shared();
    let conn = db.lock();
    maintainer_with(&conn, workspace_id)
}

/// The `owner` object every clash refusal carries.
pub fn owner_json(workspace_id: &str) -> J {
    maintainer_of(workspace_id).json()
}

// ── clashes ──────────────────────────────────────────────────────────

/// A port `app` asks for that another App on the box already opens (the
/// open path's extra ports are host ports): `409 port_owned`, naming the
/// App and its agent.
pub(crate) fn port_clash(app_id: &str, port: &appcfg::PortSpec) -> R<()> {
    let holder = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let apps = store::list_apps(&conn, None).map_err(db_err)?;
        let mut found = None;
        for a in apps.iter().filter(|a| a.app_id != app_id) {
            let cfg = appcfg::load(&conn, &a.app_id).map_err(db_err)?;
            if cfg.ports.iter().any(|p| p.port == port.port && p.proto == port.proto) {
                found = Some(a.clone());
                break;
            }
        }
        found
    };
    let Some(other) = holder else { return Ok(()) };
    let m = maintainer_of(&other.owner_workspace);
    let ask = match &m.address {
        Some(addr) if m.kind == "agent" => format!(" Ask it: k2 msg {addr} \"…\""),
        _ => " Ask the server's owner.".to_string(),
    };
    let mut r = no(
        PORT_OWNED,
        409,
        format!("port {}/{} is used by App {}, {}.{ask}", port.port, port.proto, other.app_id, m.phrase()),
    );
    r.extra = Some(json!({ "port": port.port, "proto": port.proto, "app": other.app_id, "owner": m.json() }));
    Err(r)
}

/// Every port the config ops add, checked against the box.
pub(crate) fn port_clashes(app_id: &str, ops: &[appcfg::Op]) -> R<()> {
    for op in ops {
        if let appcfg::Op::PortAdd(p) = op {
            port_clash(app_id, p)?;
        }
    }
    Ok(())
}

// ── the box view ─────────────────────────────────────────────────────

fn switches(conn: &Connection, ws: &str) -> J {
    if ws.is_empty() {
        return J::Null;
    }
    let build: Option<i64> =
        conn.query_row("SELECT agents_can_manage_skin FROM projects WHERE id = ?1", rusqlite::params![ws], |r| r.get(0)).ok();
    let Some(build) = build else { return J::Null };
    json!({
        "buildAndRun": build == 1,
        "manageEndToEnd": super::limits::full_manage_with(conn, ws),
    })
}

fn hostnames(conn: &Connection, app: &AppRow) -> Vec<String> {
    let Ok(Some(row)) = k2_core::published_services::get_by_id(conn, &app.service_id) else { return vec![] };
    let svc = crate::publish_runtime::to_json(&row);
    svc.url
        .as_deref()
        .map(|u| u.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_string())
        .filter(|h| !h.is_empty())
        .into_iter()
        .collect()
}

/// One App in the box view: footprint and maintainer, nothing inside it.
fn box_row(conn: &Connection, app: &AppRow, names: &mut HashMap<String, Maintainer>) -> J {
    let m = names.entry(app.owner_workspace.clone()).or_insert_with(|| maintainer_with(conn, &app.owner_workspace)).clone();
    let cfg = appcfg::load(conn, &app.app_id).unwrap_or_default();
    let services: Vec<String> = k2_core::deploy::box_service::list(conn, &app.app_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.status == k2_core::deploy::box_service::STATUS_ALLOWED)
        .map(|r| r.name)
        .collect();
    json!({
        "appId": app.app_id,
        "state": app.state,
        "parked": app.state == store::STATE_PARKED,
        "maintainer": m.json(),
        "memory": {
            "limitBytes": app.memory_limit_bytes,
            "countedBytes": super::admission::counted(app.memory_limit_bytes),
            // Live usage arrives with the full shared budget (0.46.4).
            "usedBytes": J::Null,
        },
        "disk": { "budgetBytes": cfg.resources.disk_bytes, "usedBytes": J::Null },
        "cpuWeight": cfg.resources.cpu_weight,
        "hostnames": hostnames(conn, app),
        "ports": cfg.ports.iter().map(|p| json!({ "port": p.port, "proto": p.proto })).collect::<Vec<_>>(),
        "boxServices": services,
        "switches": switches(conn, &app.owner_workspace),
    })
}

/// GET `/cli/app/list?box=1`: every App on the box (any credential).
pub(crate) fn box_view(who: &Who) -> R<J> {
    need_credential(who)?;
    let pool = super::admission::pool();
    let db = k2_core::db::shared();
    let conn = db.lock();
    let apps = store::list_apps(&conn, None).map_err(db_err)?;
    let mut names = HashMap::new();
    let rows: Vec<J> = apps.iter().map(|a| box_row(&conn, a, &mut names)).collect();
    let used: i64 =
        apps.iter().filter(|a| a.state != store::STATE_PARKED).map(|a| super::admission::counted(a.memory_limit_bytes)).sum();
    Ok(json!({
        "ok": true,
        "box": true,
        "apps": rows,
        "memory": {
            "pool": pool.json(),
            "usedBytes": used,
            "freeBytes": pool.size.saturating_sub(used).max(0),
        },
        "note": "Every App on this box with the agent that maintains it. Talk to that agent (k2 msg <address>) before \
                 you take a hostname, a port or memory it holds. Logs, data and settings stay with the App's own agent.",
    }))
}

/// The refusal's `owner` for a label held by `workspace_id` (any caller).
pub fn label_owner_json(conn: &Connection, workspace_id: &str) -> J {
    maintainer_with(conn, workspace_id).json()
}
