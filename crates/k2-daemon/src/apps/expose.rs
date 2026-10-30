//! `k2 app expose` (App-governed Spaces S7, PRD §9.1, vs-live AG-14): give
//! an App installed without `--tunnel` its public URL, or take it away,
//! with no re-create.
//!
//! ```text
//! POST /cli/app/expose {app, tunnel: true, transfer?}   <app>.<box>.k2.dev serves the App
//! POST /cli/app/expose {app, tunnel: false}             …and no longer (local only)
//! ```
//!
//! - **No re-create:** the same App row and the same service id, so its
//!   logins, grants, secret shares and Space bindings all stay. Before this,
//!   uninstall + `install --tunnel` was the only way, and it dropped them.
//! - **`tunnel: true`** takes the same steps `install --tunnel` takes: the
//!   label's owner check (another workspace's label is refused naming its
//!   agent), an existing hostname nobody holds is never an agent's to
//!   repoint (a person repoints it with `transfer`), the agent's default
//!   limits (one public hostname; past it, a held request for the owner),
//!   the Connect preflight, the one control-plane write that points the
//!   label at the App's front port; then the service's `expose` is `tunnel`.
//! - **`tunnel: false`** removes the hostname (only while it is still this
//!   App's: its workspace holds it and it points at the App's front port;
//!   otherwise it is left as it is) and sets `expose` back to `local`.
//! - **Who:** as `install --tunnel`: the owner, an Admin, a shell tab, or
//!   the App's own agent with "Build and run Apps" (without it the change
//!   is a held `manage` request, S42). Member logins read only.
//! - **Then:** a sweep is asked for at once, so Sew's origin for the App
//!   follows within seconds (and an App that lost its URL sends
//!   `origin: null`). The App's own `K2_APP_ORIGIN` follows on its next
//!   start: the answer says `k2 app restart <app>`.

use k2_core::deploy::{codes, store::AppRow};
use k2_core::published_services as ps;
use serde_json::{json, Value as J};

use super::routes::{actor, app_for, b, db_err, json_body, need_build, no, s, Who, R};

pub const ROUTES: &[&str] = &["/cli/app/expose"];
pub const POST_ROUTES: &[&str] = &["/cli/app/expose"];

/// `POST /cli/app/expose`.
pub(crate) fn route(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let tunnel = v
        .get("tunnel")
        .and_then(J::as_bool)
        .ok_or_else(|| no(codes::BAD_REQUEST, 400, "tunnel is required: true (a public URL) or false (local only)"))?;
    let app = app_for(who, s(&v, "app"))?;
    let lock = super::labels::cfg_lock(&app.app_id);
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    let svc = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::get_by_id(&conn, &app.service_id).map_err(db_err)?
    }
    .ok_or_else(|| no(codes::APP_NOT_FOUND, 404, format!("App '{}' has no service row", app.app_id)))?;
    let now_tunnel = svc.expose == ps::EXPOSE_TUNNEL;
    if now_tunnel == tunnel {
        return Ok(answer(&app, tunnel, false, None));
    }
    if tunnel {
        expose(who, &v, &app)
    } else {
        unexpose(who, &app)
    }
}

fn expose(who: &Who, v: &J, app: &AppRow) -> R<J> {
    let ws = app.owner_workspace.clone();
    // The label's owner (N6): another workspace's label is refused naming
    // its agent, as at install.
    crate::label_owner::check(&app.app_id, &ws, false).map_err(|r| {
        let mut out = no(codes::LABEL_TAKEN, 409, r.message());
        if let Some(o) = r.body.get("owner") {
            out.extra = Some(json!({ "owner": o }));
        }
        out
    })?;
    // An existing hostname nobody holds: never an agent's; a person's only
    // with `transfer` (install's and domain add's rule).
    let transfer = super::labels::may_repoint(who, &ws, &app.app_id, b(v, "transfer"))?;
    // Switch 1's default limits: one public hostname per agent; past it,
    // the change waits for the owner.
    if let Some(bound) = super::limits::bound(who) {
        if let Some(over) = super::limits::public_hostname_overage(&bound)? {
            return super::limits::hold(who, "/cli/app/expose", v, &bound, vec![over]);
        }
    }
    let port = point(&ws, app)?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::set_expose(&conn, &app.service_id, ps::EXPOSE_TUNNEL).map_err(db_err)?;
    }
    super::labels::audit(&app.app_id, "expose", &actor(who), json!({ "tunnel": true, "frontPort": port, "transfer": transfer }));
    finish(app);
    Ok(answer(app, true, true, None))
}

fn unexpose(who: &Who, app: &AppRow) -> R<J> {
    // Only while the hostname is still this App's (its workspace holds it
    // and it points at the App's front port): else it is left as it is.
    let left = match super::labels::still_this_apps(app, &app.app_id) {
        Ok(()) => {
            drop_hostname(&app.app_id);
            None
        }
        Err(why) => Some(format!("{} was left as it is: {why}", app.app_id)),
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::set_expose(&conn, &app.service_id, ps::EXPOSE_LOCAL).map_err(db_err)?;
    }
    super::labels::audit(&app.app_id, "expose", &actor(who), json!({ "tunnel": false, "hostnameLeft": left }));
    finish(app);
    Ok(answer(app, false, true, left))
}

/// After a change: the mirror, the Publish view, and a sweep now so Sew's
/// origin for the App follows (AG-13).
fn finish(app: &AppRow) {
    super::routes::mirror(&app.app_id);
    crate::session_events::emit_tunnel_subdomains_changed();
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::PublishServicesChanged { project_id: app.owner_workspace.clone() });
    k2_core::tool_hook::kick_reconcile();
}

fn answer(app: &AppRow, tunnel: bool, changed: bool, left: Option<String>) -> J {
    let url = super::origins::nested_origin(
        &app.app_id,
        if tunnel { ps::EXPOSE_TUNNEL } else { ps::EXPOSE_LOCAL },
        &super::origins::box_subdomain(),
    );
    let next = if changed {
        format!(
            "Chat's origin for the App follows within seconds. The App sees its new K2_APP_ORIGIN on its next start: k2 app restart {}",
            app.app_id
        )
    } else if tunnel {
        format!("{} already has its public URL.", app.app_id)
    } else {
        format!("{} is already local only.", app.app_id)
    };
    let mut out = json!({
        "ok": true,
        "app": app.app_id,
        "expose": if tunnel { ps::EXPOSE_TUNNEL } else { ps::EXPOSE_LOCAL },
        "changed": changed,
        "url": url,
        "restartNeeded": changed,
        "next": next,
    });
    if let Some(l) = left {
        out["hostnameLeft"] = json!(l);
    }
    out
}

// ── the hostname steps (a seam for the unit tests) ───────────────────

/// Point `<app>.<box>.k2.dev` at the App's front port (Connect preflight,
/// then one control-plane write). Returns the port.
fn point(ws: &str, app: &AppRow) -> R<u16> {
    #[cfg(test)]
    if let Some(port) = test_seam::point(ws, app) {
        return Ok(port);
    }
    let (cfg, _) = crate::publish_runtime::nested_preflight().map_err(|e| no(codes::ENGINE_UNAVAILABLE, 503, e.message))?;
    let port = super::supervisor::ensure_front(app)?;
    crate::publish_runtime::attach_hostname(ws, &app.app_id, port, &cfg.token, &cfg.subdomain, false)
        .map_err(|e| no(codes::ENGINE_UNAVAILABLE, 503, format!("the URL step failed: {e}")))?;
    Ok(port)
}

/// Remove the App's hostname from the control plane (best effort, as
/// uninstall does).
fn drop_hostname(label: &str) {
    #[cfg(test)]
    if test_seam::dropped(label) {
        return;
    }
    if let Ok(c) = k2_core::tunnel::config::load() {
        if !c.token.trim().is_empty() {
            if let Err(e) = k2_core::tunnel::subdomains::delete_subdomain(&c.token, label) {
                k2_core::log_debug!("[apps] expose --local {label}: {e}");
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod test_seam {
    use std::cell::RefCell;

    use k2_core::deploy::store::AppRow;

    thread_local! {
        /// `Some(port)`: the hostname steps succeed at this front port, and
        /// each step is logged.
        static PORT: RefCell<Option<u16>> = const { RefCell::new(None) };
        static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    pub fn set(port: Option<u16>) {
        PORT.with(|p| *p.borrow_mut() = port);
        LOG.with(|l| l.borrow_mut().clear());
    }

    pub fn log() -> Vec<String> {
        LOG.with(|l| l.borrow().clone())
    }

    pub(super) fn point(ws: &str, app: &AppRow) -> Option<u16> {
        let port = PORT.with(|p| *p.borrow())?;
        LOG.with(|l| l.borrow_mut().push(format!("point {} {ws} {port}", app.app_id)));
        Some(port)
    }

    pub(super) fn dropped(label: &str) -> bool {
        if PORT.with(|p| p.borrow().is_none()) {
            return false;
        }
        LOG.with(|l| l.borrow_mut().push(format!("drop {label}")));
        true
    }
}
