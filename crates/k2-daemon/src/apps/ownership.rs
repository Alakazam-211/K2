//! Who maintains an App when its workspace goes (prd-agent-app-permissions-v1
//! §6, Q8, F4). Slice D.
//!
//! - **The delete guard** ([`removal_block`]): removing a workspace that
//!   maintains Apps is refused with `409 workspace_has_apps`, listing the
//!   Apps and the three ways out. Every removal route asks it first
//!   (`/cli/workspace/remove`, `/cli/projects/delete`, `/cli/agent/retire`);
//!   `k2_core` refuses under it as the floor.
//! - **The ways out** (people only, never a passport):
//!   - `POST /cli/app/handover {app, to}` — the owner gives an App to
//!     another workspace (minimal S26: `apps.owner_workspace`, its service
//!     row and its URL label move; admission re-checked; audited; both
//!     agents get an inbox note). Owner only (owner token or Owner login).
//!   - `POST /cli/app/uninstall {app}` — the existing path (no purge).
//!   - `POST /cli/app/park {app}` — stop it and hold it for the owner
//!     (`state = parked`, `owner_workspace = ''`): it doesn't serve or
//!     restart, no agent sees it as its own, its URL label stays reserved.
//!     Owner or Admin (parking lowers).
//!   - `POST /cli/app/unpark {app, to}` — the owner gives a parked App to a
//!     workspace (stopped; `k2 app start` brings it back).
//!   - `POST /cli/app/release {workspace, apps: handover:<ws>|uninstall|park}`
//!     — one of the three for every App a workspace maintains (what
//!     `k2 workspace remove --apps …` runs before the removal).
//! - **The boot sweep** ([`sweep_orphans`]): Apps whose workspace is
//!   already gone (removed before 0.46.3) are parked, never deleted, and
//!   the owner gets one note listing them.

use k2_core::connect_users::Role;
use k2_core::deploy::{self, codes, store, store::AppRow};
use serde_json::{json, Value as J};

use crate::cli_response::CliResponse;

use super::routes::{actor, app_for, db_err, json_body, need_credential, no, s, ws_by_token, Who, Ws, R};

pub const WORKSPACE_HAS_APPS: &str = "workspace_has_apps";
pub const APP_PARKED: &str = "app_parked";
pub const NOT_PARKED: &str = "not_parked";
pub const RELEASE_INCOMPLETE: &str = "release_incomplete";

pub const ROUTES: &[&str] = &["/cli/app/handover", "/cli/app/park", "/cli/app/unpark", "/cli/app/release"];

/// The owner in person: the owner token or an Owner login (never a K2
/// tab's passport, the shell included: an agent can type in one).
fn owner(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner, .. })
}

fn owner_or_admin(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner | Role::Admin, .. })
}

fn need_owner(who: &Who, what: &str) -> R<()> {
    need_credential(who)?;
    if owner(who) {
        return Ok(());
    }
    Err(no(codes::OWNER_ONLY, 403, format!("{what} is the server owner's, from Settings or a terminal outside K2")))
}

fn need_owner_or_admin(who: &Who, what: &str) -> R<()> {
    need_credential(who)?;
    if owner_or_admin(who) {
        return Ok(());
    }
    Err(no(codes::OWNER_ONLY, 403, format!("{what} is for the server's owner or an admin, from Settings or a terminal outside K2")))
}

pub(crate) fn handle(path: &str, who: &Who, body: &[u8]) -> R<J> {
    match path {
        "/cli/app/handover" => handover_route(who, body, false),
        "/cli/app/unpark" => handover_route(who, body, true),
        "/cli/app/park" => park_route(who, body),
        "/cli/app/release" => release_route(who, body),
        _ => Err(no("not_found", 404, "no such App route")),
    }
}

/// Refuse a parked App's start, deploy, restart and rollback.
pub(crate) fn refuse_if_parked(app: &AppRow) -> R<()> {
    if app.state == store::STATE_PARKED {
        return Err(no(
            APP_PARKED,
            409,
            format!("{} is parked (held by the server's owner): give it to a workspace first (k2 app unpark {} --to <workspace>)", app.app_id, app.app_id),
        ));
    }
    Ok(())
}

// ── notes ────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) static NOTES: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// An inbox note to a workspace's agent, never a wake (§5).
fn note(ws_id: &str, title: &str, text: &str) {
    #[cfg(test)]
    {
        NOTES.lock().unwrap_or_else(|e| e.into_inner()).push((ws_id.to_string(), format!("{title}\n{text}")));
    }
    #[cfg(not(test))]
    {
        let Some(w) = super::routes::ws_by_id(ws_id) else { return };
        if let Err(e) = k2_core::inbox::compose(std::path::Path::new(&w.path), title, text, Some("normal"), Some("apps"), Some("k2-apps")) {
            k2_core::log_debug!("[apps] note to {ws_id}: {e}");
        }
    }
}

fn audit(app_id: &str, event: &str, by: &str, payload: J) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, app_id, event, by, None, &payload) {
        k2_core::log_debug!("[apps] audit {app_id} {event}: {e}");
    }
}

fn changed(ws: &str) {
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::PublishServicesChanged { project_id: ws.to_string() });
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({ "apps": "ownership" }));
}

// ── the delete guard ─────────────────────────────────────────────────

/// The Apps a workspace maintains.
pub fn owned_apps(ws: &str) -> Result<Vec<AppRow>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::list_apps(&conn, Some(ws)).map_err(|e| e.to_string())
}

/// `None` = no Apps, the removal may go on. `Some` = `409
/// workspace_has_apps` with the Apps and the three ways out (a DB error
/// fails closed with a 500).
pub fn removal_block(project_id: &str, display_name: &str) -> Option<CliResponse> {
    let apps = match owned_apps(project_id) {
        Ok(a) => a,
        Err(e) => return Some(CliResponse::internal_error(format!("checking the workspace's Apps: {e}"))),
    };
    if apps.is_empty() {
        return None;
    }
    let handle = super::routes::ws_handle(project_id);
    let names: Vec<&str> = apps.iter().map(|a| a.app_id.as_str()).collect();
    let hint = format!(
        "{display_name} maintains {} {}: removing it would leave {} serving with no agent. First hand {} over \
         (k2 app release {handle} --apps handover:<workspace>, owner only), uninstall {} (--apps uninstall; the data stays) \
         or park {} for the owner (--apps park). k2 workspace remove {handle} --apps … does both steps.",
        if apps.len() == 1 { "an App" } else { "Apps" },
        names.join(", "),
        if apps.len() == 1 { "it" } else { "them" },
        if apps.len() == 1 { "it" } else { "them" },
        if apps.len() == 1 { "it" } else { "them" },
        if apps.len() == 1 { "it" } else { "them" },
    );
    Some(CliResponse {
        status: "409 Conflict",
        content_type: "application/json",
        body: json!({
            "ok": false,
            "error": {
                "code": WORKSPACE_HAS_APPS,
                "hint": hint,
                "apps": apps.iter().map(|a| json!({ "app": a.app_id, "state": a.state })).collect::<Vec<_>>(),
                "ways": [
                    { "way": "handover", "who": "owner", "command": format!("k2 app release {handle} --apps handover:<workspace>") },
                    { "way": "uninstall", "who": "owner or admin", "command": format!("k2 app release {handle} --apps uninstall") },
                    { "way": "park", "who": "owner or admin", "command": format!("k2 app release {handle} --apps park") },
                ],
            },
        })
        .to_string(),
    })
}

// ── park ─────────────────────────────────────────────────────────────

/// Stop `app` and hold it for the owner. `from` is the workspace it
/// leaves (its row's, possibly already gone).
fn park_row(app: &AppRow, by: &str, reason: &str) -> R<()> {
    if app.state == store::STATE_PARKED {
        return Ok(());
    }
    super::supervisor::stop_app(app, by)?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let tx = conn.unchecked_transaction().map_err(db_err)?;
        tx.execute(
            "UPDATE apps SET owner_workspace = '', state = ?2, state_reason = ?3, updated_at = ?4 WHERE app_id = ?1",
            rusqlite::params![app.app_id, store::STATE_PARKED, reason, deploy::now()],
        )
        .map_err(db_err)?;
        tx.execute(
            "UPDATE published_services SET project_id = '', desired = ?2 WHERE id = ?1",
            rusqlite::params![app.service_id, k2_core::published_services::DESIRED_STOPPED],
        )
        .map_err(db_err)?;
        // The URL label stays reserved for the parked App ('' = the owner's).
        tx.execute(
            "UPDATE subdomain_workspaces SET project_id = '' WHERE label = ?1 AND project_id = ?2",
            rusqlite::params![app.app_id, app.owner_workspace],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
    }
    audit(&app.app_id, "park", by, json!({ "from": app.owner_workspace, "reason": reason }));
    super::routes::mirror(&app.app_id);
    Ok(())
}

/// POST `/cli/app/park {app, reason?}` (owner or admin).
fn park_route(who: &Who, body: &[u8]) -> R<J> {
    need_owner_or_admin(who, "Parking an App")?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    if app.state == store::STATE_PARKED {
        return Ok(json!({ "ok": true, "app": app.app_id, "state": store::STATE_PARKED, "already": true }));
    }
    let from = app.owner_workspace.clone();
    let reason = format!("parked by {}{}", actor(who), s(&v, "reason").map(|r| format!(": {r}")).unwrap_or_default());
    park_row(&app, &actor(who), &reason)?;
    if !from.is_empty() {
        note(
            &from,
            &format!("App {} was parked", app.app_id),
            &format!(
                "{} parked your App {}: it is stopped and held by the server's owner, and it is no longer yours. Ask your owner \
                 if you need it back.",
                actor(who),
                app.app_id
            ),
        );
    }
    changed(&from);
    Ok(json!({ "ok": true, "app": app.app_id, "state": store::STATE_PARKED, "from": super::routes::ws_handle(&from) }))
}

// ── hand over / unpark ───────────────────────────────────────────────

/// Give `app` to workspace `to` (owner only). A parked App comes back
/// stopped; a running App keeps running.
fn assign(app: &AppRow, to: &Ws, by: &str) -> R<()> {
    // The App's budget joins the pool again (parked Apps don't count) and
    // the transfer is a §4.3 check point.
    super::admission::check_memory(&app.app_id, app.memory_limit_bytes)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let clash: Option<String> = conn
        .query_row(
            "SELECT id FROM published_services WHERE project_id = ?1 AND name = ?2 AND id != ?3",
            rusqlite::params![to.id, app.app_id, app.service_id],
            |r| r.get(0),
        )
        .ok();
    if clash.is_some() {
        return Err(no(
            codes::APP_EXISTS,
            409,
            format!("that workspace already publishes a service named '{}'; remove it there first (k2 publish rm)", app.app_id),
        ));
    }
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let (state, reason) = if app.state == store::STATE_PARKED {
        (store::STATE_STOPPED, Some(format!("unparked by {by}; start it with k2 app start {}", app.app_id)))
    } else {
        (app.state.as_str(), app.state_reason.clone())
    };
    tx.execute(
        "UPDATE apps SET owner_workspace = ?2, state = ?3, state_reason = ?4, updated_at = ?5 WHERE app_id = ?1",
        rusqlite::params![app.app_id, to.id, state, reason, deploy::now()],
    )
    .map_err(db_err)?;
    tx.execute("UPDATE published_services SET project_id = ?2 WHERE id = ?1", rusqlite::params![app.service_id, to.id]).map_err(db_err)?;
    tx.execute(
        "UPDATE subdomain_workspaces SET project_id = ?3 WHERE label = ?1 AND project_id = ?2",
        rusqlite::params![app.app_id, app.owner_workspace, to.id],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;
    Ok(())
}

/// POST `/cli/app/handover {app, to}` / `/cli/app/unpark {app, to}`.
fn handover_route(who: &Who, body: &[u8], unpark: bool) -> R<J> {
    need_owner(who, if unpark { "Unparking an App" } else { "Handing an App over" })?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let to_raw = s(&v, "to").ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the workspace that takes it (to)"))?;
    let to = ws_by_token(to_raw).ok_or_else(|| no(codes::BAD_REQUEST, 404, format!("no workspace '{to_raw}'")))?;
    if unpark && app.state != store::STATE_PARKED {
        return Err(no(NOT_PARKED, 409, format!("{} isn't parked; hand it over instead (k2 app handover {} --to <workspace>)", app.app_id, app.app_id)));
    }
    if app.owner_workspace == to.id {
        return Ok(json!({ "ok": true, "app": app.app_id, "workspace": super::routes::ws_handle(&to.id), "already": true }));
    }
    let by = actor(who);
    assign(&app, &to, &by)?;
    let from = app.owner_workspace.clone();
    let event = if app.state == store::STATE_PARKED { "unpark" } else { "handover" };
    audit(&app.app_id, event, &by, json!({ "from": from, "to": to.id }));
    super::routes::mirror(&app.app_id);
    let to_h = super::routes::ws_handle(&to.id);
    note(
        &to.id,
        &format!("You now maintain App {}", app.app_id),
        &format!(
            "{by} gave you App {}{}. It is yours now: k2 app inspect {}{}",
            app.app_id,
            if from.is_empty() { String::new() } else { format!(" (it was {}'s)", super::routes::ws_handle(&from)) },
            app.app_id,
            if app.state == store::STATE_PARKED { format!(". It is stopped: k2 app start {}", app.app_id) } else { String::new() },
        ),
    );
    if !from.is_empty() && super::routes::ws_by_id(&from).is_some() {
        note(&from, &format!("App {} moved to {to_h}", app.app_id), &format!("{by} handed your App {} to {to_h}; it is no longer yours.", app.app_id));
    }
    changed(&to.id);
    let after = app_for(&Who::Owner, Some(&app.app_id))?;
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "workspace": to_h,
        "state": after.state,
        "note": "the App's own data, Seeds and settings moved with it; a running App keeps its link until its next restart",
    }))
}

// ── release (every App of a workspace) ───────────────────────────────

/// POST `/cli/app/release {workspace, apps}`: one way out for every App
/// the workspace maintains. `apps` = `handover:<ws>` (owner) | `uninstall`
/// | `park` (owner or admin).
fn release_route(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let way = s(&v, "apps").ok_or_else(|| no(codes::BAD_REQUEST, 400, "apps is handover:<workspace>, uninstall or park"))?;
    let handover_to = way.strip_prefix("handover:").map(str::trim).filter(|w| !w.is_empty());
    match (way, handover_to) {
        (_, Some(_)) => need_owner(who, "Handing Apps over")?,
        ("uninstall" | "park", None) => need_owner_or_admin(who, "Releasing a workspace's Apps")?,
        _ => return Err(no(codes::BAD_REQUEST, 400, format!("apps is handover:<workspace>, uninstall or park, not '{way}'"))),
    }
    let ws_raw = s(&v, "workspace").ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the workspace (workspace)"))?;
    let ws = ws_by_token(ws_raw).ok_or_else(|| no(codes::BAD_REQUEST, 404, format!("no workspace '{ws_raw}'")))?;
    let apps = owned_apps(&ws.id).map_err(|e| no("db_error", 500, e))?;
    let mut results = Vec::new();
    let mut failed = 0;
    for app in &apps {
        let one = json!({ "app": app.app_id }).to_string();
        let r = match handover_to {
            Some(to) => handover_route(who, json!({ "app": app.app_id, "to": to }).to_string().as_bytes(), false),
            None if way == "uninstall" => super::routes::uninstall(who, one.as_bytes()),
            None => park_route(who, one.as_bytes()),
        };
        match r {
            Ok(_) => results.push(json!({ "app": app.app_id, "ok": true })),
            Err(e) => {
                failed += 1;
                results.push(json!({ "app": app.app_id, "ok": false, "code": e.code, "hint": e.hint }));
            }
        }
    }
    if failed > 0 {
        let mut r = no(
            RELEASE_INCOMPLETE,
            409,
            format!("{failed} of {} Apps weren't released; the workspace still maintains them", apps.len()),
        );
        r.extra = Some(json!({ "results": results }));
        return Err(r);
    }
    Ok(json!({ "ok": true, "workspace": super::routes::ws_handle(&ws.id), "way": way, "results": results }))
}

// ── the boot sweep ───────────────────────────────────────────────────

/// Park every App whose workspace is gone (F4: removed before 0.46.3, when
/// nothing stopped it). Never deletes. The owner gets one note listing
/// the Apps parked this time; an App already parked is left alone, so a
/// later boot says nothing new. Returns the Apps parked now.
pub fn sweep_orphans() -> Vec<String> {
    let orphans: Vec<AppRow> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match store::list_apps(&conn, None) {
            Ok(apps) => apps
                .into_iter()
                .filter(|a| a.state != store::STATE_PARKED)
                .filter(|a| {
                    a.owner_workspace.trim().is_empty()
                        || conn
                            .query_row("SELECT 1 FROM projects WHERE id = ?1", rusqlite::params![a.owner_workspace], |_| Ok(()))
                            .is_err()
                })
                .collect(),
            Err(e) => {
                k2_core::log_debug!("[apps] orphan sweep: {e}");
                return vec![];
            }
        }
    };
    let mut parked = Vec::new();
    for app in &orphans {
        let reason = "its workspace was removed: parked for the owner by the boot sweep (k2 app unpark <app> --to <workspace>, or k2 app uninstall)";
        match park_row(app, "system", reason) {
            Ok(()) => parked.push(app.app_id.clone()),
            Err(e) => k2_core::log_debug!("[apps] orphan sweep: {} not parked: {}", app.app_id, e.hint),
        }
    }
    if !parked.is_empty() {
        let title = format!(
            "{} App{} parked: {} workspace was removed",
            parked.len(),
            if parked.len() == 1 { "" } else { "s" },
            if parked.len() == 1 { "its" } else { "their" }
        );
        let body = format!(
            "These Apps kept running after the workspace that maintained them was removed, with no agent looking after them. \
             K2 stopped them and holds them for you; nothing was deleted.\n\n{}\n\nGive one to an agent: k2 app unpark <app> --to <workspace>\n\
             Or remove it: k2 app uninstall <app> (its data stays unless you add --purge).",
            parked.iter().map(|a| format!("- {a}")).collect::<Vec<_>>().join("\n")
        );
        if let Err(e) = crate::feedback_routes::file_owner_notice(k2_core::db::AUDIT_SENTINEL_IDS[0], "k2-apps", &title, &body) {
            k2_core::log_debug!("[apps] orphan sweep note: {e}");
        }
        k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, json!({ "apps": "parked", "parked": parked }));
    }
    parked
}
