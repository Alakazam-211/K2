//! Default limits for an agent with "Build and run Apps" (switch 1,
//! prd-agent-app-permissions-v1 §4.3, F2). The envelope copies the hire
//! limits' pattern (`hire/limits.rs`):
//!
//! | | default |
//! |---|---|
//! | Apps per agent | 3 |
//! | memory per App | 512 MiB when none is given (every caller, F2); a raise up to 2 GiB applies |
//! | disk budgets, all its Apps | 10 GiB |
//! | CPU weight | ≤ 100 |
//! | public hostnames | 1 |
//!
//! Past the envelope the change is **held for the owner** (the Approvals
//! queue), never refused: an install becomes a held `manage` request
//! (replayed as the agent when approved), a config budget past it a held
//! `config` request. Every App keeps a limit: an agent can't set `memory
//! none` (held). Humans aren't bound; admission (`admission.rs`) binds
//! everyone.
//!
//! Switch 2 ("…and manage them end to end", 0197
//! `projects.agents_can_manage_apps_full`) lifts the envelope: the agent
//! sets its own budgets inside admission. [`full_manage_with`] reads the
//! column when it exists (it reads off until migration 0197 lands).

use rusqlite::Connection;
use serde_json::{json, Value as J};

use k2_core::app_grace::config::{Op, State};
use k2_core::app_grace::requests as reqs;
use k2_core::deploy::store::{self, AppRow};

use super::routes::{agent_ws, db_err, s, Who, R};

pub const MAX_APPS: usize = 3;
/// The memory an install gets when none is given (every caller).
pub const DEFAULT_MEMORY: i64 = 512 << 20;
/// How far an agent raises one App's memory on its own.
pub const MEMORY_MAX: i64 = 2 << 30;
/// The disk budgets of all an agent's Apps together.
pub const DISK_TOTAL: i64 = 10 << 30;
pub const CPU_WEIGHT_MAX: u32 = 100;
pub const PUBLIC_HOSTNAMES: usize = 1;

/// Switch 2 on this workspace? (`agents_can_manage_apps_full`, 0197;
/// `false` while the column doesn't exist.)
pub fn full_manage_with(conn: &Connection, ws: &str) -> bool {
    conn.query_row("SELECT agents_can_manage_apps_full FROM projects WHERE id = ?1", rusqlite::params![ws], |r| r.get::<_, i64>(0))
        .map(|v| v == 1)
        .unwrap_or(false)
}

pub fn full_manage(ws: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    full_manage_with(&conn, ws)
}

/// The workspace whose default limits bind this call: an agent passport
/// whose switch 2 is off, outside the replay of a request the owner
/// approved. Humans → `None`.
pub(crate) fn bound(who: &Who) -> Option<String> {
    let ws = agent_ws(who)?;
    if super::approvals::manage_approved_now() || full_manage(ws) {
        return None;
    }
    Some(ws.to_string())
}

fn mib(b: i64) -> String {
    if b >= 1 << 30 && b % (1 << 30) == 0 {
        format!("{} GiB", b >> 30)
    } else {
        format!("{} MiB", b >> 20)
    }
}

/// Where an install by a bound agent goes past the envelope (empty = fits).
pub(crate) fn install_overages(ws: &str, memory: Option<i64>, tunnel: bool) -> R<Vec<String>> {
    let (apps, public) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let apps = store::owned_app_ids(&conn, ws).map_err(db_err)?.len();
        let public: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM apps a JOIN published_services p ON p.id = a.service_id \
                 WHERE a.owner_workspace = ?1 AND p.expose = ?2",
                rusqlite::params![ws, k2_core::published_services::EXPOSE_TUNNEL],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        (apps, public as usize)
    };
    let mut out = Vec::new();
    if apps >= MAX_APPS {
        out.push(format!("App number {} for this agent: the default limit is {MAX_APPS} Apps per agent", apps + 1));
    }
    match memory {
        None => out.push("no memory limit: every App of an agent keeps one (the default is 512 MiB)".into()),
        Some(m) if m > MEMORY_MAX => out.push(format!("memory {}: an agent sets up to {} per App on its own", mib(m), mib(MEMORY_MAX))),
        Some(_) => {}
    }
    if tunnel && public >= PUBLIC_HOSTNAMES {
        out.push(format!("public hostname number {}: the default limit is {PUBLIC_HOSTNAMES} per agent", public + 1));
    }
    Ok(out)
}

/// Where one budget op by a bound agent goes past the envelope.
pub(crate) fn config_overage(ws: &str, app: &AppRow, op: &Op, _state: &State) -> R<Option<String>> {
    Ok(match op {
        Op::Memory(None) => Some("no memory limit: every App of an agent keeps one".into()),
        Op::Memory(Some(m)) if *m > MEMORY_MAX => {
            Some(format!("memory {}: an agent sets up to {} per App on its own", mib(*m), mib(MEMORY_MAX)))
        }
        Op::CpuWeight(None) => Some(format!("no CPU weight limit: an agent sets up to {CPU_WEIGHT_MAX}")),
        Op::CpuWeight(Some(w)) if *w > CPU_WEIGHT_MAX => Some(format!("CPU weight {w}: an agent sets up to {CPU_WEIGHT_MAX}")),
        Op::Disk(None) => Some(format!("no disk budget: an agent's Apps share {} at most", mib(DISK_TOTAL))),
        Op::Disk(Some(d)) => {
            let others: i64 = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let mut sum = 0i64;
                for id in store::owned_app_ids(&conn, ws).map_err(db_err)? {
                    if id != app.app_id {
                        let c = k2_core::app_grace::config::load(&conn, &id).map_err(db_err)?;
                        sum += c.resources.disk_bytes.unwrap_or(0);
                    }
                }
                sum
            };
            (others + d > DISK_TOTAL).then(|| {
                format!("disk {}: this agent's Apps would hold {} of the {} default", mib(*d), mib(others + d), mib(DISK_TOTAL))
            })
        }
        _ => None,
    })
}

/// Hold a call past the envelope for the owner: a `manage` request with
/// the route and body, replayed as the agent when approved (the envelope
/// doesn't bind the replay; admission still does).
pub(crate) fn hold(who: &Who, path: &str, v: &J, ws: &str, reasons: Vec<String>) -> R<J> {
    let label = s(v, "app").map(str::to_ascii_lowercase).unwrap_or_else(|| "(new App)".into());
    let app = s(v, "app").and_then(|a| super::routes::app_for(who, Some(a)).ok());
    let verb = path.trim_start_matches("/cli/app/").replace('/', " ");
    let r = super::approvals::file(&super::approvals::Filing {
        who,
        kind: reqs::KIND_MANAGE,
        app: app.as_ref(),
        label: &label,
        workspace: ws,
        body: json!({ "route": path, "body": v }),
        diff: json!({ "lines": reasons, "route": path }),
        summary: format!("k2 app {verb} {label} (past the agent's default limits)"),
        reasons: reasons.clone(),
        reason: s(v, "reason"),
    })?;
    let mut out = super::approvals::held_answer(&r);
    out["limits"] = json!(reasons);
    out["hint"] = json!(format!(
        "held for the owner: this goes past your default App limits ({}). They approve it in person (k2 app approve {} \
         outside K2, or Settings → App Approvals).",
        reasons.join("; "),
        r.id
    ));
    Ok(out)
}
