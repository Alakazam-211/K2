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

pub const ROUTES: &[&str] =
    &["/cli/app/domains", "/cli/app/domain/add", "/cli/app/domain/rm", "/cli/app/domain/check", "/cli/app/domain/primary"];
pub const POST_ROUTES: &[&str] = &["/cli/app/domain/add", "/cli/app/domain/rm", "/cli/app/domain/check", "/cli/app/domain/primary"];

pub const RESERVED: &str = "reserved_label";
pub const USE_LABEL_TAKE: &str = "use_label_take";
pub const OTHER_APP: &str = "label_serves_another_app";
pub const NOT_ATTACHED: &str = "domain_not_attached";

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        "/cli/app/domains" => list(who, params),
        "/cli/app/domain/add" => add(who, body),
        "/cli/app/domain/rm" => rm(who, body),
        "/cli/app/domain/check" => custom_check(who, body),
        "/cli/app/domain/primary" => custom_primary(who, body),
        _ => Err(no("not_found", 404, "no such App route")),
    }
}

/// A dotted name outside k2.dev (`app.customer.com`): a custom domain
/// (0.46.6, vs-live CD-A3). `jobs` and `jobs.<box>.k2.dev` stay labels.
pub(crate) fn is_custom_host(raw: &str) -> bool {
    let t = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    let k2 = k2_core::tunnel::config::SUBDOMAIN_HOST;
    t.contains('.') && t != k2 && !t.ends_with(&format!(".{k2}"))
}

fn host_arg(v: &J) -> R<String> {
    let raw = s(v, "host")
        .or_else(|| s(v, "label"))
        .ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the custom domain (e.g. app.customer.com)"))?;
    k2_core::domains::normalize_hostname(raw).map_err(|e| no("invalid_domain", 400, e))
}

fn cd_refusal(r: super::custom_domains::Refusal) -> Refusal {
    no(r.code, r.status, r.hint)
}

/// One custom name's answer: its state, the DNS to set and the next step.
fn custom_json(d: &k2_core::domains::AppDomain) -> J {
    let mut v = super::custom_domains::status_json(d, false);
    v["hostname"] = json!(d.hostname);
    v["records"] = J::Array(super::custom_domain_flow::records_to_set(d));
    v["next"] = json!(next_step(d));
    v
}

/// The next step in plain words, per state (PRD §9.1).
fn next_step(d: &k2_core::domains::AppDomain) -> String {
    use k2_core::domains::*;
    let front = if k2_core::front443::load().enabled {
        String::new()
    } else {
        " This box's :443 front is off: path (a) needs it (the owner runs k2 front enable).".to_string()
    };
    match d.state.as_str() {
        STATE_LIVE => format!(
            "Served. Make it the App's primary origin with k2 app domain primary {} {} (then k2 app restart {}).",
            d.app, d.hostname, d.app
        ),
        STATE_ISSUING => format!("Proven; K2 is getting its certificate (TLS-ALPN-01 or DNS-01).{front}"),
        STATE_FAILING => format!(
            "Not served: {}. Fix the DNS records below, then: k2 app domain check {} {}",
            d.last_error.as_deref().unwrap_or("its DNS moved away"),
            d.app,
            d.hostname
        ),
        _ => format!(
            "Set these at your DNS host (DNS-only, no proxy), then run: k2 app domain check {} {}.{front}",
            d.app, d.hostname
        ),
    }
}

fn app_domain_of(app: &AppRow, host: &str) -> R<k2_core::domains::AppDomain> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::domains::app_domain(&conn, host)
        .map_err(db_err)?
        .filter(|d| d.app == app.app_id)
        .ok_or_else(|| no(NOT_ATTACHED, 404, format!("{} has no custom domain {host} (k2 app domains {})", app.app_id, app.app_id)))
}

/// POST `/cli/app/domain/add {app, label: app.customer.com}` (custom).
fn custom_add(who: &Who, app: &AppRow, v: &J) -> R<J> {
    use super::custom_domains as cd;
    let host = host_arg(v)?;
    let caller = super::routes::agent_ws(who).map(str::to_string);
    let existing = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::get_name(&conn, &host).map_err(db_err)?
    };
    let role = format!("{}{}", k2_core::domains::ROLE_APP_PREFIX, app.app_id);
    if existing.as_ref().is_some_and(|e| e.role == role) {
        let d = app_domain_of(app, &host)?;
        return Ok(json!({ "ok": true, "already": true, "app": app.app_id, "custom": custom_json(&d) }));
    }
    if let Some(e) = &existing {
        cd::may_change_existing(caller.as_deref(), &host, &e.role).map_err(cd_refusal)?;
    }
    cd::bind_conflict(&host, existing.as_ref().map(|e| e.role.as_str())).map_err(cd_refusal)?;
    let (binding, count) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let b = k2_core::domains::binding_for_name(&conn, &host).map_err(db_err)?.map(|(b, _)| b);
        (b, k2_core::domains::app_domains(&conn).map_err(db_err)?.len())
    };
    if count >= super::custom_domain_flow::MAX_NAMES {
        return Err(no(
            "too_many_custom_domains",
            409,
            format!("this box serves at most {} custom App names", super::custom_domain_flow::MAX_NAMES),
        ));
    }
    let Some(binding) = binding else {
        let apex = host.splitn(2, '.').nth(1).filter(|a| a.contains('.')).unwrap_or(&host).to_string();
        return Err(no(
            NOT_ATTACHED,
            409,
            format!(
                "no attached domain covers {host}: the owner attaches its zone first (k2 domain add {apex}), then run this again"
            ),
        ));
    };
    let d = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::upsert_name(&conn, &host, &binding.apex, &role).map_err(db_err)?;
        k2_core::domains::app_domain(&conn, &host).map_err(db_err)?.ok_or_else(|| db_err("the new row is gone"))?
    };
    // §6: K2 plants the TXT itself on a K2-hosted zone. An agent needs
    // `dns_manage` for that (§9.3); without it the record is shown.
    let mut planted: J = J::Null;
    if binding.dns_write && !binding.is_pending_ns() {
        let may = match caller.as_deref().and_then(super::routes::ws_by_id) {
            Some(ws) => k2_core::workspace::settings::dns_manage_allowed_for_path(&ws.path),
            None => true,
        };
        planted = if !may {
            json!({ "planted": false, "why": "this agent has no DNS write (dns_manage): set the TXT below, or ask the owner" })
        } else {
            match super::custom_domain_flow::plant_txt(&d, &binding) {
                Ok(()) => json!({ "planted": true }),
                Err(e) => json!({ "planted": false, "why": e }),
            }
        };
    }
    let by = actor(who);
    labels::audit(&app.app_id, "custom_domain_add", &by, json!({ "host": host, "apex": binding.apex, "txt": planted }));
    crate::front443::rebuild_now();
    // The first check now (the re-check loop keeps trying every 10 min).
    let _ = super::custom_domain_flow::check_now(&host);
    let d = app_domain_of(app, &host)?;
    super::routes::mirror(&app.app_id);
    Ok(json!({ "ok": true, "app": app.app_id, "custom": custom_json(&d), "txt": planted }))
}

/// POST `/cli/app/domain/rm {app, label: app.customer.com}` (custom): the
/// binding and its route go now (and K2's TXT on a K2-hosted zone); the
/// certificate files in `~/.k2/certs/<host>/` stay (PRD §8.5).
fn custom_rm(who: &Who, app: &AppRow, v: &J) -> R<J> {
    let host = host_arg(v)?;
    let d = app_domain_of(app, &host)?;
    let binding = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let b = k2_core::domains::get_binding(&conn, &d.apex).map_err(db_err)?;
        k2_core::domains::remove_name(&conn, &host).map_err(db_err)?;
        b
    };
    if let Some(b) = binding.filter(|b| b.dns_write && !b.is_pending_ns()) {
        if let Err(e) = super::custom_domain_flow::remove_txt(&d, &b) {
            log_rm(&host, &e);
        }
    }
    crate::front443::rebuild_now();
    labels::audit(&app.app_id, "custom_domain_rm", &actor(who), json!({ "host": host }));
    super::routes::mirror(&app.app_id);
    Ok(json!({ "ok": true, "app": app.app_id, "removed": host, "note": "no longer served; restart the App so its origins drop it" }))
}

fn log_rm(host: &str, e: &str) {
    k2_core::log_debug!("[apps/domains] {host}: the _k2-domain TXT stays in the zone: {e}");
}

/// POST `/cli/app/domain/check {app, host}`: re-run the DNS + TXT checks now.
fn custom_check(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let host = host_arg(&v)?;
    app_domain_of(&app, &host)?;
    if k2_core::airgap::enabled() {
        return Err(no("airgap", 409, "air-gap is on: K2 makes no DNS checks"));
    }
    let (d, o) = super::custom_domain_flow::check_now(&host).ok_or_else(|| no(NOT_ATTACHED, 404, format!("{host} is gone")))?;
    Ok(json!({ "ok": true, "app": app.app_id, "custom": custom_json(&d), "check": { "ok": o.ok, "txt": o.txt_ok, "pointsHere": o.here, "seen": o.seen, "reason": o.reason } }))
}

/// POST `/cli/app/domain/primary {app, host}`: the name `K2_APP_ORIGIN`
/// uses once it is live (CD-A4).
fn custom_primary(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let host = host_arg(&v)?;
    let d = app_domain_of(&app, &host)?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::set_primary(&conn, &app.app_id, &host).map_err(db_err)?;
    }
    labels::audit(&app.app_id, "custom_domain_primary", &actor(who), json!({ "host": host }));
    let d = app_domain_of(&app, &d.hostname)?;
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "primary": host,
        "live": d.is_live(),
        "note": if d.is_live() {
            format!("K2_APP_ORIGIN is https://{host} from the App's next start (k2 app restart {})", app.app_id)
        } else {
            format!("{host} becomes K2_APP_ORIGIN once it is live; until then the App's K2 label stays primary")
        },
    }))
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
    let custom = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::app_domains_for(&conn, &app.app_id).map_err(db_err)?
    };
    let sub = super::origins::box_subdomain();
    let host = |l: &str| (!sub.is_empty()).then(|| format!("{l}.{sub}.{}", k2_core::tunnel::config::SUBDOMAIN_HOST));
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "own": { "label": app.app_id, "host": host(&app.app_id) },
        "taken": cfg.labels.iter().map(|l| json!({ "label": l.label, "host": host(&l.label), "from": l.service_id, "takenAt": l.taken_at, "takenBy": l.taken_by })).collect::<Vec<_>>(),
        "attached": cfg.subdomains.iter().map(|d| json!({ "label": d.label, "host": host(&d.label), "addedAt": d.added_at, "addedBy": d.added_by, "previousTarget": d.previous_target })).collect::<Vec<_>>(),
        // 0.46.6 (CD-A3): custom domains, each with its state, the DNS to
        // set and the next step.
        "custom": custom.iter().map(custom_json).collect::<Vec<_>>(),
        "front443": k2_core::front443::load().enabled,
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
    if s(&v, "label").or_else(|| s(&v, "host")).is_some_and(is_custom_host) {
        return custom_add(who, &app, &v);
    }
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
    if s(&v, "label").or_else(|| s(&v, "host")).is_some_and(is_custom_host) {
        return custom_rm(who, &app, &v);
    }
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

/// 0.46.6: custom domains through `/cli/app/domain/*` (vs-live CD-A3).
#[cfg(test)]
mod custom_tests {
    use super::*;
    use crate::apps::custom_domain_flow as flow;
    use crate::apps::custom_domains::test_util::{build_switch, dns_manage, runtime_app};
    use std::net::IpAddr;

    const HOST: &str = "nsi.agentnsi.com";

    struct Proven(String);
    impl flow::Lookup for Proven {
        fn txt(&self, _: &str) -> Result<Vec<String>, String> {
            Ok(vec![self.0.clone()])
        }
        fn addrs(&self, _: &str) -> Result<Vec<IpAddr>, String> {
            Ok(vec!["203.0.113.10".parse().unwrap()])
        }
        fn cname(&self, _: &str) -> Option<String> {
            None
        }
    }

    fn agent(ws: &str) -> Who {
        Who::Agent { project_id: ws.to_string(), session_id: "s1".into() }
    }

    fn post(who: &Who, path: &str, body: J) -> R<J> {
        handle(path, who, body.to_string().as_bytes(), &HashMap::new())
    }

    fn setup() {
        runtime_app("ws-nsi", "agentnsi");
        runtime_app("ws-other", "crm");
        build_switch("ws-nsi", true);
        build_switch("ws-other", true);
        flow::set_test_box_ips(Some(vec!["203.0.113.10".parse().unwrap()]));
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::upsert_binding(&conn, "agentnsi.com", None, false).unwrap();
    }

    #[test]
    fn custom_hosts_are_told_apart_from_labels() {
        assert!(is_custom_host("nsi.agentnsi.com"));
        assert!(is_custom_host("agentnsi.com."));
        assert!(!is_custom_host("agents"));
        assert!(!is_custom_host("agents.nsi.k2.dev"));
        assert!(!is_custom_host("k2.dev"));
        for p in ["/cli/app/domain/check", "/cli/app/domain/primary", "/cli/app/domain/add", "/cli/app/domain/rm"] {
            assert!(crate::apps::routes::is_post_route(p), "{p} is POST-only (405 on GET, PRD test 10)");
        }
        assert!(!crate::apps::routes::is_post_route("/cli/app/domains"));
    }

    /// The App's own agent binds, sees the DNS to set, checks (proof →
    /// issuing, one order), picks the primary and removes; another
    /// workspace's agent never sees the App.
    #[test]
    fn the_apps_agent_runs_a_custom_domain_end_to_end() {
        let _db = k2_core::db::scoped_for_test();
        let _home = crate::test_support::TempHome::new();
        setup();
        let me = agent("ws-nsi");

        let e = post(&agent("ws-other"), "/cli/app/domain/add", json!({ "app": "agentnsi", "label": HOST })).unwrap_err();
        assert_eq!(e.code, codes::APP_NOT_FOUND, "another workspace's App is not found: {e:?}");

        let v = post(&me, "/cli/app/domain/add", json!({ "app": "agentnsi", "label": HOST })).unwrap();
        assert_eq!(v["custom"]["state"], "waiting_dns", "{v}");
        assert_eq!(v["custom"]["notServedYet"], true);
        let recs = v["custom"]["records"].as_array().unwrap();
        assert!(recs.iter().any(|r| r["type"] == "A" && r["value"] == "203.0.113.10"), "{recs:?}");
        let txt = recs.iter().find(|r| r["type"] == "TXT").unwrap();
        assert_eq!(txt["name"], format!("_k2-domain.{HOST}"));
        assert!(v["custom"]["next"].as_str().unwrap().contains(&format!("k2 app domain check agentnsi {HOST}")));
        let again = post(&me, "/cli/app/domain/add", json!({ "app": "agentnsi", "label": HOST })).unwrap();
        assert_eq!(again["already"], true);

        let list = handle("/cli/app/domains", &me, b"", &HashMap::from([("app".to_string(), "agentnsi".to_string())])).unwrap();
        assert_eq!(list["custom"][0]["hostname"], HOST);

        // Not proven: check leaves it waiting, no order.
        flow::set_test_lookup(Some(Box::new(Proven("unrelated".into()))));
        let v = post(&me, "/cli/app/domain/check", json!({ "app": "agentnsi", "host": HOST })).unwrap();
        assert_eq!((v["custom"]["state"].as_str(), v["check"]["ok"].as_bool()), (Some("waiting_dns"), Some(false)));
        assert!(flow::take_test_issues().is_empty());

        // Proven: issuing, one order.
        let token = txt["value"].as_str().unwrap().to_string();
        flow::set_test_lookup(Some(Box::new(Proven(token))));
        let v = post(&me, "/cli/app/domain/check", json!({ "app": "agentnsi", "host": HOST })).unwrap();
        assert_eq!(v["custom"]["state"], "issuing", "{v}");
        assert_eq!(flow::take_test_issues(), vec![HOST.to_string()]);

        // Primary: accepted, says it waits for live.
        let v = post(&me, "/cli/app/domain/primary", json!({ "app": "agentnsi", "host": HOST })).unwrap();
        assert_eq!((v["primary"].as_str(), v["live"].as_bool()), (Some(HOST), Some(false)));
        let e = post(&me, "/cli/app/domain/primary", json!({ "app": "agentnsi", "host": "other.agentnsi.com" })).unwrap_err();
        assert_eq!(e.code, NOT_ATTACHED);

        let v = post(&me, "/cli/app/domain/rm", json!({ "app": "agentnsi", "label": HOST })).unwrap();
        assert_eq!(v["removed"], HOST);
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(k2_core::domains::get_name(&conn, HOST).unwrap().is_none());
        drop(conn);
        flow::set_test_lookup(None);
    }

    /// PRD test 6: a custom name equal to the mail host is refused at bind,
    /// naming its use; a k2.dev name is never custom; an unattached zone
    /// and another workspace's App name are refused.
    #[test]
    fn conflicts_are_refused_at_bind() {
        let _db = k2_core::db::scoped_for_test();
        let _home = crate::test_support::TempHome::new();
        setup();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::domains::upsert_name(&conn, "mail.agentnsi.com", "agentnsi.com", "mail").unwrap();
            k2_core::domains::upsert_name(&conn, "crm.agentnsi.com", "agentnsi.com", "app:crm").unwrap();
        }
        let me = agent("ws-nsi");
        let e = post(&me, "/cli/app/domain/add", json!({ "app": "agentnsi", "label": "mail.agentnsi.com" })).unwrap_err();
        assert_eq!(e.code, crate::apps::custom_domains::NAME_IN_USE, "{e:?}");
        assert!(e.hint.contains("mail"), "{}", e.hint);
        let e = post(&me, "/cli/app/domain/add", json!({ "app": "agentnsi", "label": "crm.agentnsi.com" })).unwrap_err();
        assert_eq!(e.code, crate::apps::custom_domains::BOUND_ELSEWHERE, "{e:?}");
        let e = post(&me, "/cli/app/domain/add", json!({ "app": "agentnsi", "label": "app.unattached.example" })).unwrap_err();
        assert_eq!(e.code, NOT_ATTACHED);
        assert!(e.hint.contains("k2 domain add unattached.example"), "{}", e.hint);
        // The owner may not take the mail host either.
        let e = post(&Who::Owner, "/cli/app/domain/add", json!({ "app": "agentnsi", "label": "mail.agentnsi.com" })).unwrap_err();
        assert_eq!(e.code, crate::apps::custom_domains::NAME_IN_USE);
    }

    /// §6/§9.3: on a K2-hosted zone K2 plants the TXT itself, for an agent
    /// only with DNS write (dns_manage); without it the record is shown.
    #[test]
    fn k2_plants_the_txt_only_for_a_caller_with_dns_write() {
        let _db = k2_core::db::scoped_for_test();
        let _home = crate::test_support::TempHome::new();
        setup();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::domains::upsert_binding_zone(
                &conn,
                "agentnsi.com",
                &k2_core::domains::BoundZone {
                    zone_id: Some("z-nsi".into()),
                    status: Some("active".into()),
                    nameservers: vec![],
                    dns_write: true,
                    auto_created: false,
                },
            )
            .unwrap();
        }
        let _api = crate::dns::proxy::set_fake_dns_api(|method, path, _| {
            let ok = |b: &str| Ok(crate::dns::proxy::DnsHttpResponse { status: 200, body: b.to_string() });
            match (method, path) {
                ("GET", "/api/dns/zones/z-nsi") => ok(r#"{"records":[]}"#),
                ("POST", "/api/dns/zones/z-nsi/records") => ok(r#"{"record":{"id":"r1"}}"#),
                other => panic!("unexpected DNS API call {other:?}"),
            }
        });
        dns_manage("ws-nsi", false);
        let v = post(&agent("ws-nsi"), "/cli/app/domain/add", json!({ "app": "agentnsi", "label": HOST })).unwrap();
        assert_eq!(v["txt"]["planted"], false, "{v}");
        assert!(!crate::dns::proxy::fake_dns_api_calls().iter().any(|(m, _)| m == "POST"), "no write without dns_manage");
        dns_manage("ws-nsi", true);
        let v = post(&agent("ws-nsi"), "/cli/app/domain/add", json!({ "app": "agentnsi", "label": "www.agentnsi.com" })).unwrap();
        assert_eq!(v["txt"]["planted"], true, "{v}");
    }
}
