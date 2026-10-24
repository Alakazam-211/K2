//! Extra nested hostnames for an App (0.46.4, NSI #6): `k2 app label take`
//! moves only a `--cmd` service's own label; a BFF also served on other
//! subdomains of its workspace (hand-made with `k2 publish subdomain
//! create … --target localhost:<port>`) had no way onto the App.
//!
//! ```text
//! GET  /cli/app/domains        ?app=           the App's hostnames: its own, taken labels, attached ones
//! POST /cli/app/domain/add     {app, label}    <label>.<box>.k2.dev serves the App
//! POST /cli/app/domain/rm      {app, label}    …and back to what it served before (or removed)
//! ```
//!
//! - **Who:** as `label take`: the owner, an Admin, a shell tab, or the
//!   agent of the App's workspace with "Build and run Apps" (without it the
//!   change is a held `manage` request, S42).
//! - **Which labels:** any nested label this App's workspace holds (the
//!   0074 attribution) or that nobody holds yet. Another workspace's label
//!   is refused naming its agent (N6); a reserved door (`meet`, `chat`,
//!   `skin`) is refused; a `--cmd` service's own label is `k2 app label
//!   take` (it unroutes that service); another App's label is refused.
//! - **Routed by App id:** the hostname points at the App's front port, the
//!   stable listener the daemon owns for that App, never at an instance's
//!   port, so deploys and rollbacks never touch it. The front port coming
//!   back on another number after a box restart is S2's R36 (the re-point
//!   of every App hostname happens there, from [`hostnames`]).
//! - The App sees the hostname in `K2_APP_ORIGINS` from its next start.
//! - **Existing hostnames** (security review): a label that already routes
//!   somewhere and that the workspace doesn't hold is never an agent's to
//!   repoint; the owner does it with `--transfer` (audited).
//! - **Remove** points the hostname back at what it served before the add
//!   (when that still answers), else removes it; uninstall does the same.
//!   Only while the hostname is still the App's (its workspace holds it and
//!   it points at the App's front port); otherwise it is left as it is.

use std::collections::HashMap;

use k2_core::app_grace::config::AttachedSubdomain;
use k2_core::deploy::{self, codes, store::AppRow};
use k2_core::published_services as ps;
use serde_json::{json, Value as J};

use super::labels;
use super::routes::{actor, app_for, b, db_err, json_body, need_build, need_credential, no, s, Refusal, Who, R};

pub const ROUTES: &[&str] = &["/cli/app/domains", "/cli/app/domain/add", "/cli/app/domain/rm"];
pub const POST_ROUTES: &[&str] = &["/cli/app/domain/add", "/cli/app/domain/rm"];

pub const RESERVED: &str = "reserved_label";
pub const USE_LABEL_TAKE: &str = "use_label_take";
pub const OTHER_APP: &str = "label_serves_another_app";
pub const NOT_ATTACHED: &str = "domain_not_attached";

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        "/cli/app/domains" => list(who, params),
        "/cli/app/domain/add" => add(who, body),
        "/cli/app/domain/rm" => rm(who, body),
        _ => Err(no("not_found", 404, "no such App route")),
    }
}

/// `jobs`, or `jobs.<box>.k2.dev` → `jobs`.
pub(crate) fn label_of(raw: &str) -> R<String> {
    let t = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    let suffix = format!(".{}", k2_core::tunnel::config::SUBDOMAIN_HOST);
    let first = if t.ends_with(&suffix) { t.split('.').next().unwrap_or("").to_string() } else { t };
    k2_core::tunnel::client_ip_labels::normalize_label(&first).map_err(|e| no(codes::BAD_REQUEST, 400, e))
}

/// Every hostname label an App answers on: its own, the labels it took,
/// the ones attached here. The re-point after a front-port change (S2's
/// R36) walks this list.
pub fn hostnames(app: &AppRow) -> Vec<String> {
    let mut out = vec![app.app_id.clone()];
    for l in labels::taken(&app.app_id) {
        if !out.contains(&l) {
            out.push(l);
        }
    }
    out
}

/// GET `/cli/app/domains?app=`.
fn list(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let cfg = labels::load_cfg(&app.app_id)?;
    let sub = super::origins::box_subdomain();
    let host = |l: &str| (!sub.is_empty()).then(|| format!("{l}.{sub}.{}", k2_core::tunnel::config::SUBDOMAIN_HOST));
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "own": { "label": app.app_id, "host": host(&app.app_id) },
        "taken": cfg.labels.iter().map(|l| json!({ "label": l.label, "host": host(&l.label), "from": l.service_id, "takenAt": l.taken_at, "takenBy": l.taken_by })).collect::<Vec<_>>(),
        "attached": cfg.subdomains.iter().map(|d| json!({ "label": d.label, "host": host(&d.label), "addedAt": d.added_at, "addedBy": d.added_by, "previousTarget": d.previous_target })).collect::<Vec<_>>(),
    }))
}

/// Which App (other than `app_id`) already serves `label`.
fn served_by_other_app(app_id: &str, label: &str) -> R<Option<String>> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let apps = deploy::store::list_apps(&conn, None).map_err(db_err)?;
    for a in apps.into_iter().filter(|a| a.app_id != app_id) {
        if a.app_id == label {
            return Ok(Some(a.app_id));
        }
        let c = k2_core::app_grace::config::load(&conn, &a.app_id).map_err(db_err)?;
        if c.labels.iter().any(|l| l.label == label) || c.subdomains.iter().any(|d| d.label == label) {
            return Ok(Some(a.app_id));
        }
    }
    Ok(None)
}

/// Where `label` points now (the control plane's map, else the last target
/// this box stored for it).
pub(crate) fn current_target(ws: &str, label: &str) -> Option<String> {
    if let Some(t) = k2_core::tunnel::subdomains::current().targets.get(label) {
        return Some(t.clone());
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::SubdomainWorkspace::list_for_project(&conn, ws)
        .ok()?
        .into_iter()
        .find(|(l, _)| l == label)
        .map(|(_, t)| t)
        .filter(|t| !t.trim().is_empty())
}

/// The loopback port of a `127.0.0.1:<port>` / `localhost:<port>` target.
pub(crate) fn loopback_port(target: &str) -> Option<u16> {
    let (h, p) = target.trim().rsplit_once(':')?;
    matches!(h, "127.0.0.1" | "localhost").then(|| p.parse().ok()).flatten().filter(|p| *p != 0)
}

fn foreign(label: &str, r: crate::label_owner::Refusal) -> Refusal {
    let mut out = no(labels::LABEL_FOREIGN, 403, r.message());
    if let Some(o) = r.body.get("owner") {
        out.extra = Some(json!({ "owner": o, "label": label }));
    }
    out
}

/// POST `/cli/app/domain/add {app, label}`.
fn add(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let label = label_of(s(&v, "label").ok_or_else(|| no(codes::BAD_REQUEST, 400, "label is required (the subdomain, e.g. agents)"))?)?;
    let ws = app.owner_workspace.clone();
    if label == app.app_id {
        return Ok(json!({ "ok": true, "already": true, "app": app.app_id, "label": label, "note": "an App always serves its own label" }));
    }
    let lock = labels::cfg_lock(&app.app_id);
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut cfg = labels::load_cfg(&app.app_id)?;
    if cfg.subdomains.iter().any(|d| d.label == label) || cfg.labels.iter().any(|l| l.label == label) {
        return Ok(json!({ "ok": true, "already": true, "app": app.app_id, "label": label }));
    }
    if k2_core::skin::is_reserved_nested_label(&label) {
        return Err(no(RESERVED, 400, k2_core::skin::reserved_nested_label_error(&label)));
    }
    if let Some(other) = served_by_other_app(&app.app_id, &label)? {
        return Err(no(OTHER_APP, 409, format!("'{label}' already serves the App {other}: remove it there first (k2 app domain rm {other} {label})")));
    }
    // A published service's own label: this workspace's --cmd service is
    // `label take` (it unroutes that service); anything else is refused.
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT id, project_id FROM published_services WHERE name = ?1 LIMIT 1", rusqlite::params![label], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .ok()
    };
    if let Some((id, holder)) = row {
        if holder != ws {
            return Err(match crate::label_owner::check(&label, &ws, false) {
                Err(r) => foreign(&label, r),
                Ok(_) => labels::foreign_refusal(&label, &holder),
            });
        }
        let legacy = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            ps::get_by_id(&conn, &id).map_err(db_err)?.is_some_and(|r| super::legacy::is_legacy(&r))
        };
        let hint = if legacy {
            format!("'{label}' is your --cmd service's own label: move it with k2 app label take {} {label} (that also unroutes the old service)", app.app_id)
        } else {
            format!("'{label}' is a published service's label in your workspace (k2 publish list): stop it there first")
        };
        return Err(no(USE_LABEL_TAKE, 409, hint));
    }
    crate::label_owner::check(&label, &ws, false).map_err(|r| foreign(&label, r))?;
    // An existing hostname nobody holds: never an agent's to repoint; a
    // person only with --transfer.
    let transfer = labels::may_repoint(who, &ws, &label, b(&v, "transfer"))?;
    let front = labels::healthy(&app).map_err(|why| {
        no(
            labels::NOT_HEALTHY,
            409,
            format!("{why}: deploy it and wait until it's live before it serves '{label}' (k2 app deployment {} ; k2 app logs {})", app.app_id, app.app_id),
        )
    })?;
    let previous = current_target(&ws, &label);
    labels::point(&ws, &label, front).map_err(|e| no(labels::MOVE_FAILED, 409, format!("'{label}' is unchanged: {e}")))?;
    let by = actor(who);
    cfg.subdomains.push(AttachedSubdomain { label: label.clone(), added_at: deploy::now(), added_by: by.clone(), previous_target: previous.clone() });
    labels::save_cfg(&app.app_id, &cfg)?;
    labels::audit(&app.app_id, "domain_add", &by, json!({ "label": label, "frontPort": front, "previousTarget": previous, "transfer": transfer }));
    if transfer {
        k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
            "label-transfer",
            &by,
            format!("repointed the existing hostname {label} (no workspace held it) at App {}", app.app_id),
            "cli",
            "-",
            "apps",
        ));
    }
    super::routes::mirror(&app.app_id);
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::PublishServicesChanged { project_id: ws.clone() });
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "label": label,
        "frontPort": front,
        "previousTarget": previous,
        "note": format!(
            "https://{label}.<your box>.k2.dev now serves {}. It is in K2_APP_ORIGINS from the App's next start (k2 app restart {}). \
             Undo: k2 app domain rm {} {label}{}",
            app.app_id,
            app.app_id,
            app.app_id,
            if previous.is_some() { " (it points back where it pointed before)" } else { "" }
        ),
    }))
}

/// Give an attached hostname back: where it pointed before when that
/// answers, else remove it. Best effort (uninstall); `Ok(where)` says
/// which happened. Only while it is still this App's (attributed to its
/// workspace, pointing at its front port): otherwise it is left as it is
/// and only drops out of the App's config.
pub(crate) fn release(app: &AppRow, d: &AttachedSubdomain) -> Result<String, String> {
    if let Err(why) = labels::still_this_apps(app, &d.label) {
        return Ok(format!("left as it is ({why})"));
    }
    let ws = app.owner_workspace.as_str();
    if let Some(port) = d.previous_target.as_deref().and_then(loopback_port).filter(|p| labels::accepts(*p)) {
        labels::point(ws, &d.label, port)?;
        return Ok(format!("127.0.0.1:{port}"));
    }
    let c = k2_core::tunnel::config::load().map_err(|e| format!("the tunnel config: {e}"))?;
    if c.token.trim().is_empty() {
        return Err("this server has no K2 Connect tunnel token".into());
    }
    k2_core::tunnel::subdomains::delete_subdomain(&c.token, &d.label)?;
    Ok("removed".into())
}

/// POST `/cli/app/domain/rm {app, label}`.
fn rm(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let label = label_of(s(&v, "label").ok_or_else(|| no(codes::BAD_REQUEST, 400, "label is required"))?)?;
    let lock = labels::cfg_lock(&app.app_id);
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut cfg = labels::load_cfg(&app.app_id)?;
    let Some(i) = cfg.subdomains.iter().position(|d| d.label == label) else {
        let hint = if cfg.labels.iter().any(|l| l.label == label) {
            format!("'{label}' was taken from your --cmd service: k2 app label return {} {label}", app.app_id)
        } else {
            format!("{} has no attached '{label}' (k2 app domains {})", app.app_id, app.app_id)
        };
        return Err(no(NOT_ATTACHED, 404, hint));
    };
    let d = cfg.subdomains[i].clone();
    let went = release(&app, &d).map_err(|e| no(labels::MOVE_FAILED, 409, format!("'{label}' still serves {}: {e}", app.app_id)))?;
    cfg.subdomains.remove(i);
    labels::save_cfg(&app.app_id, &cfg)?;
    let by = actor(who);
    labels::audit(&app.app_id, "domain_rm", &by, json!({ "label": label, "now": went }));
    super::routes::mirror(&app.app_id);
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::PublishServicesChanged { project_id: app.owner_workspace.clone() });
    Ok(json!({ "ok": true, "app": app.app_id, "label": label, "now": went }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_hosts_normalise() {
        assert_eq!(label_of("Agents").unwrap(), "agents");
        assert_eq!(label_of(&format!("agents.nsi.{}", k2_core::tunnel::config::SUBDOMAIN_HOST)).unwrap(), "agents");
        assert!(label_of("-bad").is_err());
        assert!(label_of("a_b").is_err());
        assert_eq!(loopback_port("127.0.0.1:3001"), Some(3001));
        assert_eq!(loopback_port("localhost:8788"), Some(8788));
        assert_eq!(loopback_port("10.0.0.2:80"), None, "never off loopback");
        assert_eq!(loopback_port("127.0.0.1:0"), None);
    }

    /// A hostname goes back (or is removed) only while it is still the
    /// App's: its workspace holds it and it points at the App's front port.
    #[test]
    fn release_only_while_still_the_apps() {
        use super::labels::release_check;
        assert_eq!(release_check("ws1", 41_000, Some("ws1"), Some("127.0.0.1:41000")), Ok(()));
        assert_eq!(release_check("ws1", 41_000, Some("ws1"), Some("localhost:41000")), Ok(()));
        assert!(release_check("ws1", 41_000, Some("ws2"), Some("127.0.0.1:41000")).is_err(), "moved to another workspace");
        assert!(release_check("ws1", 41_000, None, Some("127.0.0.1:41000")).is_err(), "nobody holds it");
        assert!(release_check("ws1", 41_000, Some("ws1"), Some("127.0.0.1:3000")).is_err(), "repointed since");
        assert!(release_check("ws1", 41_000, Some("ws1"), None).is_err(), "points nowhere");
        assert!(release_check("ws1", 0, Some("ws1"), Some("127.0.0.1:0")).is_err(), "no front port");
    }
}
