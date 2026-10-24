//! Moving a public label from an agent's own legacy `--cmd` service to its
//! own App (0.46.3, Rosson 2026-10-20: "he can migrate how his current app
//! works into the runtime" himself).
//!
//! ```text
//! POST /cli/app/label/take   {app, label}   the label's hostname now serves the App
//! POST /cli/app/label/return {app, label}   …and back to the legacy service
//! ```
//!
//! - **Who:** the owner, an Admin, a shell tab, or the agent of the App's
//!   workspace with "Build and run Apps" (switch 1; without it the move is
//!   a held `manage` request). The legacy service must be the same
//!   workspace's: a label another workspace holds is refused (that is the
//!   owner's hand-over, `k2 app handover` / `--transfer`), never moved here.
//! - **Take:** refused unless the App is healthy (running, a live instance,
//!   its front port answering). One control-plane write points the label
//!   at the App's front port (the same hostname: no DNS change). The legacy
//!   service keeps running, **unrouted**: its row becomes `local`, so a
//!   restart of it never points the label back. The App records the label
//!   (`AppConfig.labels`), so its public origins include it from its next
//!   start.
//! - **Return:** refused unless the legacy service answers on its port;
//!   the label points back at it and its row is `tunnel` again.
//! - **Uninstall** of an App gives its taken labels back the same way
//!   ([`give_back_all`]), or drops the hostname when the legacy service is
//!   gone.

use std::time::Duration;

use k2_core::app_grace::config::{self as appcfg, TakenLabel};
use k2_core::deploy::{self, codes, store::AppRow};
use k2_core::published_services as ps;
use serde_json::{json, Value as J};

use super::routes::{actor, app_for, db_err, json_body, need_build, no, s, Refusal, Who, R};

pub const ROUTES: &[&str] = &["/cli/app/label/return", "/cli/app/label/take"];

pub const NOT_HEALTHY: &str = "app_not_healthy";
pub const LABEL_FOREIGN: &str = "label_owned_by_other_workspace";
pub const NO_PUBLIC_LABEL: &str = "no_public_label";
pub const NOT_TAKEN: &str = "label_not_taken";
pub const LEGACY_DOWN: &str = "legacy_not_running";
pub const MOVE_FAILED: &str = "label_move_failed";
/// A label that already routes somewhere on this box and that the App's
/// workspace doesn't hold (security review, 0.46.4).
pub const LABEL_EXISTS: &str = "label_exists_elsewhere";

/// One lock per App around its config's load → change → save (domain
/// add/rm, label take/return, uninstall's give-back), so two of them at
/// once never lose each other's change (security review, 0.46.4).
pub(super) fn cfg_lock(app_id: &str) -> std::sync::Arc<std::sync::Mutex<()>> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let mut m = LOCKS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    m.entry(app_id.to_string()).or_default().clone()
}

/// The workspace a nested label is attributed to (the 0074 row).
pub(super) fn holder_of(label: &str) -> R<Option<String>> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::SubdomainWorkspace::owner_of(&conn, label).map_err(db_err)
}

/// May `who` point `label` at an App of workspace `ws` (domain add, App
/// install)? A new label, or one `ws` holds, goes ahead. A label that
/// already routes somewhere on this box and that `ws` doesn't hold (a
/// hand-made `k2 publish subdomain create` hostname, say) is never
/// repointed by an agent; a person repoints it only deliberately, with
/// `transfer`. (Another workspace's label is refused before this, by
/// `label_owner`.) `Ok(true)` = a deliberate transfer.
pub(super) fn may_repoint(who: &Who, ws: &str, label: &str, transfer: bool) -> R<bool> {
    if holder_of(label)?.as_deref() == Some(ws) {
        return Ok(false);
    }
    let Some(target) = k2_core::tunnel::subdomains::current().targets.get(label).cloned() else {
        return Ok(false);
    };
    if super::routes::agent_ws(who).is_some() {
        return Err(no(
            LABEL_EXISTS,
            403,
            format!(
                "'{label}' already serves something on this box and isn't your workspace's: an agent never repoints it. \
                 Pick a new label, or ask your owner to move it deliberately (--transfer, from a terminal outside K2)"
            ),
        ));
    }
    if !transfer {
        return Err(no(
            LABEL_EXISTS,
            409,
            format!("'{label}' already serves {target} on this box and no workspace holds it: repoint it deliberately with --transfer"),
        ));
    }
    Ok(true)
}

/// Is `label` still `app`'s to give back? Only while it is attributed to
/// the App's workspace and points at the App's front port: a label moved
/// to another workspace, or repointed since, is left as it is (security
/// review, 0.46.4). `Err(why)` = leave it.
pub(crate) fn still_this_apps(app: &AppRow, label: &str) -> Result<(), String> {
    let held = holder_of(label).map_err(|r| r.hint)?;
    let front = app.front_port.and_then(|p| u16::try_from(p).ok()).unwrap_or(0);
    release_check(&app.owner_workspace, front, held.as_deref(), super::domains::current_target(&app.owner_workspace, label).as_deref())
}

/// [`still_this_apps`]'s rule on plain values.
pub(crate) fn release_check(ws: &str, front: u16, held: Option<&str>, target: Option<&str>) -> Result<(), String> {
    match held {
        Some(h) if h == ws => {}
        Some(_) => return Err("it belongs to another workspace now".into()),
        None => return Err("no workspace holds it now".into()),
    }
    match target.and_then(super::domains::loopback_port) {
        Some(p) if front != 0 && p == front => Ok(()),
        Some(p) => Err(format!("it points at port {p} now, not the App's front port")),
        None => Err("it doesn't point at the App now".into()),
    }
}

pub(crate) fn handle(path: &str, who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let label = s(&v, "label")
        .map(|l| l.to_ascii_lowercase())
        .ok_or_else(|| no(codes::BAD_REQUEST, 400, "label is required (the legacy service's publish name)"))?;
    let lock = cfg_lock(&app.app_id);
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    match path {
        "/cli/app/label/take" => take(who, &app, &label),
        "/cli/app/label/return" => give_back(who, &app, &label),
        _ => Err(no("not_found", 404, "no such App route")),
    }
}

pub(super) fn accepts(port: u16) -> bool {
    port != 0 && std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(800)).is_ok()
}

pub(super) fn load_cfg(app_id: &str) -> R<appcfg::AppConfig> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    appcfg::load(&conn, app_id).map_err(db_err)
}

pub(super) fn save_cfg(app_id: &str, cfg: &appcfg::AppConfig) -> R<()> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "UPDATE apps SET config_json = ?2, updated_at = ?3 WHERE app_id = ?1",
        rusqlite::params![app_id, serde_json::to_string(cfg).unwrap_or_default(), deploy::now()],
    )
    .map_err(db_err)?;
    Ok(())
}

pub(super) fn audit(app_id: &str, event: &str, by: &str, payload: J) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, app_id, event, by, None, &payload) {
        k2_core::log_debug!("[apps] audit {app_id} {event}: {e}");
    }
}

/// Point `label` at `port` (one control-plane write; the label's owner is
/// checked first by `attach_hostname`).
pub(super) fn point(project_id: &str, label: &str, port: u16) -> Result<(), String> {
    let cfg = k2_core::tunnel::config::load().map_err(|e| format!("the tunnel config: {e}"))?;
    if cfg.token.trim().is_empty() {
        return Err("this server has no K2 Connect tunnel token".into());
    }
    crate::publish_runtime::attach_hostname(project_id, label, port, &cfg.token, &cfg.subdomain, false)
}

/// The App answers: running, a live instance, its front port accepting.
pub(super) fn healthy(app: &AppRow) -> Result<u16, String> {
    if app.state != deploy::store::STATE_RUNNING {
        return Err(format!("{} is {}", app.app_id, app.state));
    }
    if super::supervisor::live_instance(&app.app_id).is_none() {
        return Err(format!("{} has no live instance", app.app_id));
    }
    let front = super::supervisor::ensure_front(app).map_err(|f| f.message)?;
    if !accepts(front) {
        return Err(format!("{}'s front port {front} doesn't answer", app.app_id));
    }
    Ok(front)
}

/// Another workspace holds `label`: refused naming the agent that holds it
/// and its `k2 msg` address (N6), never its folder or id.
pub(super) fn foreign_refusal(label: &str, holder: &str) -> Refusal {
    let m = super::coordination::maintainer_of(holder);
    let ask = match (&m.address, m.kind) {
        (Some(a), "agent") => format!(" Ask it: k2 msg {a} \"…\""),
        _ => String::new(),
    };
    let mut r = no(
        LABEL_FOREIGN,
        403,
        format!(
            "'{label}' belongs to another workspace on this server, {}: an App only takes its own workspace's labels (moving one \
             across workspaces is the owner's hand-over).{ask}",
            m.phrase()
        ),
    );
    r.extra = Some(json!({ "owner": m.json() }));
    r
}

fn take(who: &Who, app: &AppRow, label: &str) -> R<J> {
    let ws = app.owner_workspace.clone();
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::get_by_project_name(&conn, &ws, label).map_err(db_err)?
    };
    let Some(row) = row else {
        // Another workspace's label (a service row or the subdomain
        // attribution): never across workspaces.
        let foreign = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let by_row: Option<String> = conn
                .query_row("SELECT project_id FROM published_services WHERE name = ?1 AND project_id != ?2 LIMIT 1", rusqlite::params![label, ws], |r| r.get(0))
                .ok();
            by_row.or_else(|| k2_core::db::schema::SubdomainWorkspace::owner_of(&conn, label).ok().flatten().filter(|o| o != &ws))
        };
        if let Some(holder) = foreign {
            return Err(foreign_refusal(label, &holder));
        }
        return Err(no(codes::APP_NOT_FOUND, 404, format!("your workspace has no published service named '{label}' (k2 publish list)")));
    };
    if !super::legacy::is_legacy(&row) {
        return Err(no(codes::BAD_REQUEST, 409, format!("'{label}' isn't a --cmd service (it is an App or a --skin App): nothing to take over")));
    }
    let mut cfg = load_cfg(&app.app_id)?;
    if cfg.labels.iter().any(|l| l.label == label) {
        return Ok(json!({ "ok": true, "already": true, "app": app.app_id, "label": label }));
    }
    if row.expose != ps::EXPOSE_TUNNEL {
        return Err(no(
            NO_PUBLIC_LABEL,
            409,
            format!("'{label}' runs without a public hostname (--no-tunnel): nothing to move. Point what calls 127.0.0.1:{} at the App's front port instead", row.port),
        ));
    }
    let front = healthy(app).map_err(|why| {
        no(
            NOT_HEALTHY,
            409,
            format!("{why}: deploy it and wait until it's live before it takes '{label}' (k2 app deployment {} ; k2 app logs {})", app.app_id, app.app_id),
        )
    })?;
    crate::label_owner::check(label, &ws, false).map_err(|r| {
        let mut out = no(LABEL_FOREIGN, 403, r.message());
        if let Some(o) = r.body.get("owner") {
            out.extra = Some(json!({ "owner": o }));
        }
        out
    })?;
    point(&ws, label, front).map_err(|e| no(MOVE_FAILED, 409, format!("'{label}' still serves the legacy service: {e}")))?;
    // Unrouted: the legacy service runs on, and a restart of it never
    // points the label back.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::set_expose(&conn, &row.id, ps::EXPOSE_LOCAL).map_err(db_err)?;
    }
    let by = actor(who);
    cfg.labels.push(TakenLabel {
        label: label.to_string(),
        service_id: row.id.clone(),
        legacy_port: u16::try_from(row.port).unwrap_or(0),
        taken_at: deploy::now(),
        taken_by: by.clone(),
    });
    save_cfg(&app.app_id, &cfg)?;
    audit(&app.app_id, "label_take", &by, json!({ "label": label, "service": row.id, "legacyPort": row.port, "frontPort": front }));
    super::routes::mirror(&app.app_id);
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::PublishServicesChanged { project_id: ws.clone() });
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "label": label,
        "frontPort": front,
        "from": { "service": row.id, "port": row.port, "running": "yes, unrouted" },
        "note": format!(
            "https://{label}.<your box>.k2.dev now serves {}. {label} keeps running without a public label; stop it when you're sure \
             (k2 publish stop {label}), or move the label back: k2 app label return {} {label}. The App sees the label in K2_APP_ORIGINS \
             from its next start (k2 app restart {}).",
            app.app_id, app.app_id, app.app_id
        ),
    }))
}

fn give_back(who: &Who, app: &AppRow, label: &str) -> R<J> {
    let mut cfg = load_cfg(&app.app_id)?;
    let Some(i) = cfg.labels.iter().position(|l| l.label == label) else {
        return Err(no(NOT_TAKEN, 404, format!("{} doesn't serve '{label}' (k2 app label take {} {label} moved one)", app.app_id, app.app_id)));
    };
    let t = cfg.labels[i].clone();
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::get_by_id(&conn, &t.service_id).map_err(db_err)?
    }
    .ok_or_else(|| no(LEGACY_DOWN, 409, format!("the legacy service '{label}' was removed: the label stays with {}", app.app_id)))?;
    let port = u16::try_from(row.port).unwrap_or(t.legacy_port);
    if !accepts(port) {
        return Err(no(LEGACY_DOWN, 409, format!("'{label}' isn't answering on port {port}: start it first (k2 publish start {label}), then move the label back")));
    }
    point(&app.owner_workspace, label, port).map_err(|e| no(MOVE_FAILED, 409, format!("'{label}' still serves {}: {e}", app.app_id)))?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::set_expose(&conn, &row.id, ps::EXPOSE_TUNNEL).map_err(db_err)?;
    }
    cfg.labels.remove(i);
    save_cfg(&app.app_id, &cfg)?;
    let by = actor(who);
    audit(&app.app_id, "label_return", &by, json!({ "label": label, "service": row.id, "port": port }));
    super::routes::mirror(&app.app_id);
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::PublishServicesChanged { project_id: app.owner_workspace.clone() });
    Ok(json!({ "ok": true, "app": app.app_id, "label": label, "returnedTo": { "service": row.id, "port": port } }))
}

/// Uninstall: every taken label goes back to its legacy service when that
/// one answers, else its hostname is removed. Best effort; never blocks
/// the uninstall.
pub(crate) fn give_back_all(app: &AppRow) {
    let lock = cfg_lock(&app.app_id);
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(cfg) = load_cfg(&app.app_id) else { return };
    // 0.46.4: hostnames attached with `k2 app domain add` go back to what
    // they served before, or are removed (only while still this App's).
    for d in &cfg.subdomains {
        match super::domains::release(app, d) {
            Ok(went) => k2_core::log_debug!("[apps] uninstall {}: {} → {went}", app.app_id, d.label),
            Err(e) => k2_core::log_debug!("[apps] uninstall {}: {} stays as it was: {e}", app.app_id, d.label),
        }
    }
    for t in cfg.labels {
        // A label moved to another workspace or repointed since is left alone.
        if let Err(why) = still_this_apps(app, &t.label) {
            k2_core::log_debug!("[apps] uninstall {}: {} left as it is: {why}", app.app_id, t.label);
            continue;
        }
        let row = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            ps::get_by_id(&conn, &t.service_id).ok().flatten()
        };
        let back = row.as_ref().map(|r| u16::try_from(r.port).unwrap_or(t.legacy_port)).filter(|p| accepts(*p));
        match (row, back) {
            (Some(r), Some(port)) => {
                if point(&app.owner_workspace, &t.label, port).is_ok() {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    let _ = ps::set_expose(&conn, &r.id, ps::EXPOSE_TUNNEL);
                }
            }
            _ => {
                if let Ok(c) = k2_core::tunnel::config::load() {
                    if !c.token.trim().is_empty() {
                        let _ = k2_core::tunnel::subdomains::delete_subdomain(&c.token, &t.label);
                    }
                }
            }
        }
    }
}

/// The App serving a legacy service's label (`k2 publish list`).
pub fn moved_to(service_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let apps = deploy::store::list_apps(&conn, None).ok()?;
    apps.into_iter()
        .find(|a| appcfg::load(&conn, &a.app_id).is_ok_and(|c| c.labels.iter().any(|l| l.service_id == service_id)))
        .map(|a| a.app_id)
}

/// The labels an App took, and the nested hostnames attached to it
/// (`k2 app domain add`): its public origins add them.
pub fn taken(app_id: &str) -> Vec<String> {
    load_cfg(app_id)
        .map(|c| c.labels.into_iter().map(|l| l.label).chain(c.subdomains.into_iter().map(|d| d.label)).collect())
        .unwrap_or_default()
}
