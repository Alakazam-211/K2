//! App handover: one App's maintenance moves from one workspace (agent)
//! to another on the same box (prd-app-handoff-v1 rev 2; Rosson
//! 2026-10-26: one verb, `k2 app handover`, improved).
//!
//! ```text
//! GET  /cli/app/handover/plan  ?app=&to=[&db=share&collisions=keep|use_b]   what would move (read only; --dry-run)
//! ```
//!
//! **The plan** (`handover_plan.rs`) lists, from live rows, everything
//! §4.2 names: labels, domains, guests by class, rooms, secrets and
//! `.env.k2` references, the database case, App keys, A's waiting
//! requests, B's switch and budget, the re-key mode. Its `planHash` binds
//! the card to what it showed.
//!
//! **Who** may see a plan: the App's own agent (its passport; another
//! workspace's App is `app_not_found`, no existence leak), the owner, a
//! shell tab, an Owner or Admin login. A Member login is refused (the plan
//! names guests).
//!
//! **`K2_APP_HANDOVER`**: `legacy` keeps 0.46.3's handover for one release
//! (the new routes answer 404); `off` refuses every handover. Retirement
//! ledger: removal in 0.46.6.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::deploy::{codes, store::AppRow};
use serde_json::{json, Value as J};

use super::handover_plan::{self as hp, Options};
use super::routes::{app_for, need_credential, no, ws_by_token, Who, Ws, R};

pub const ROUTES: &[&str] = &["/cli/app/handover/plan"];
pub const POST_ROUTES: &[&str] = &[];

/// The `route` the handover card's held request carries (never an HTTP
/// route: the approval runs [`apply`]).
pub const APPLY_ROUTE: &str = "/cli/app/handover/apply";

pub const HANDOVER_OFF: &str = "handover_off";
pub const SAME_WORKSPACE: &str = "handover_same_workspace";

/// `K2_APP_HANDOVER`: `legacy` | `off` | anything else (the handover).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    On,
    Legacy,
    Off,
}

pub fn mode() -> Mode {
    match std::env::var("K2_APP_HANDOVER").ok().map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("legacy") => Mode::Legacy,
        Some("off") => Mode::Off,
        _ => Mode::On,
    }
}

pub(crate) fn off_refusal() -> super::routes::Refusal {
    no(HANDOVER_OFF, 403, "App handovers are off on this server (K2_APP_HANDOVER=off)")
}

/// May `who` read a handover plan for `app`? The App's own agent, the
/// owner, a shell tab, an Owner or Admin login.
fn may_read(who: &Who) -> R<()> {
    need_credential(who)?;
    match who {
        Who::Login { role: Role::Member, .. } => {
            Err(no(codes::OWNER_ONLY, 403, "a Member login can't see a handover plan (it names the App's guests): ask the owner or an admin"))
        }
        _ => Ok(()),
    }
}

pub(crate) fn handle(path: &str, who: &Who, _body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match mode() {
        Mode::Legacy => return Err(no("not_found", 404, "no such App route (K2_APP_HANDOVER=legacy)")),
        Mode::Off => return Err(off_refusal()),
        Mode::On => {}
    }
    match path {
        "/cli/app/handover/plan" => plan_route(who, params),
        _ => Err(no("not_found", 404, "no such App route")),
    }
}

fn param<'a>(params: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    params.get(k).map(|s| s.trim()).filter(|s| !s.is_empty())
}

/// The options a request names (body or query).
pub(crate) fn options_from(get: impl Fn(&str) -> Option<String>, b: impl Fn(&str) -> bool) -> R<Options> {
    let db = get("db");
    if let Some(d) = db.as_deref() {
        if d != "share" && d != "wait" {
            return Err(no(codes::BAD_REQUEST, 400, format!("db is share or wait, not '{d}'")));
        }
    }
    let collisions = get("collisions");
    if let Some(c) = collisions.as_deref() {
        if c != "keep" && c != "use_b" {
            return Err(no(codes::BAD_REQUEST, 400, format!("collisions is keep or use_b, not '{c}'")));
        }
    }
    Ok(Options { db, collisions, reset_shared: b("resetShared"), copy_source: b("copySource") })
}

/// The App and the taking workspace a call names.
pub(crate) fn target(who: &Who, app: Option<&str>, to: Option<&str>) -> R<(AppRow, Ws)> {
    let app = app_for(who, app)?;
    let to_raw = to.ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the workspace that takes the App (--to)"))?;
    let to = ws_by_token(to_raw).ok_or_else(|| no(codes::BAD_REQUEST, 404, format!("no workspace '{to_raw}'")))?;
    if to.id == app.owner_workspace {
        return Err(no(SAME_WORKSPACE, 409, format!("{} already belongs to {}", app.app_id, super::routes::ws_handle(&to.id))));
    }
    Ok((app, to))
}

/// GET `/cli/app/handover/plan?app=&to=`: the plan, read only.
fn plan_route(who: &Who, params: &HashMap<String, String>) -> R<J> {
    may_read(who)?;
    let (app, to) = target(who, param(params, "app"), param(params, "to"))?;
    let opts = options_from(|k| param(params, k).map(str::to_string), |k| matches!(param(params, k), Some("1" | "true" | "yes")))?;
    let p = hp::compute(&app, &to, &opts, None)?;
    let mut out = p.json(true);
    out["ok"] = json!(true);
    out["ready"] = json!(p.blockers.is_empty());
    Ok(out)
}
