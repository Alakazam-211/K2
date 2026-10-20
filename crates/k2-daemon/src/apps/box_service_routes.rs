//! `/cli/app/box-service*` (0.46.0): owner-approved forwards into a walled
//! App for a service on this box's loopback (`k2 app box-service …`).
//!
//! A walled App (guarded container, or the open path's container) has no
//! route to this box's loopback: its egress proxy refuses loopback and
//! "this box" in every mode, by design. A box service is the narrow door:
//! one named TCP service, one host port.
//!
//! **Data path** (built like the 8730 gateway link): the guarded engine
//! adds a pasta forward `-T <appPort>:<hostPort>` to the container, so the
//! App dials `127.0.0.1:<appPort>` inside its wall (`K2_BOX_<NAME>` says
//! where); pasta connects to host `127.0.0.1:<hostPort>` from the App's own
//! cgroup, and the root helper's per-App nft rule (`apps nft app <id> <gw>
//! <box ports…>`) admits that port only while the service is approved.
//! Raw TCP: HTTP, websockets and long uploads ride it unchanged.
//! The process exception has the host's network (loopback only, narrowed
//! by the same nft rule), so it dials the host port directly; the stage-1
//! preview process engine is unwalled and already reaches loopback.
//!
//! **Held, approved, revoked.** A request (manifest `boxServices` at
//! deploy, or `k2 app box-service request`) is a held `box_service`
//! widening (`k2 app approvals|approve|deny`, the owner in person with the
//! `app-approve` vault confirm). Forwards are rendered at start for
//! approved *and* held services, so approving opens the door at once (the
//! nft rule reloads) without a restart; a service asked for after the App
//! started needs one restart for its forward. Revoking shuts the door at
//! once and cuts its open connections; a held or revoked door answers with
//! a TCP reset, never a stall (the helper's nft ruleset v4, 0.46.1).
//!
//! **Deny-list**: `k2_root_proto::apps::box_port_refusal` (system ports
//! below 1024 incl. ssh, K2's App range 38600–38999 so no App reaches
//! another App's gateway or proxy, Postgres 5432, Caddy's admin API, the
//! box router's loopback) plus this daemon's own listeners (its main port
//! and the heartbeat port, read from `~/.k2`). Unix sockets (cell sockets,
//! the root helper) are out of reach by construction (TCP only).
//!
//! Who may: read and propose, anyone who sees the App (a passport, its own
//! workspace's); revoke (narrowing) the same, at once. An agent with
//! "…and manage them end to end" (switch 2) opens its own App's doors
//! itself (prd-agent-app-permissions-v1 §2.1).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};

use k2_core::deploy::box_service::{self as bs, Want};
use k2_core::deploy::{self, codes as dcodes};
use k2_root_proto::engine::BoxForward;
use serde_json::{json, Value as J};

use super::routes::{actor, app_for, db_err, json_body, need_credential, no, s, Refusal, Who, R};

pub const ROUTES: &[&str] = &["/cli/app/box-service", "/cli/app/box-service/request", "/cli/app/box-service/revoke"];
pub const POST_ROUTES: &[&str] = &["/cli/app/box-service/request", "/cli/app/box-service/revoke"];

pub mod codes {
    pub const BOX_SERVICE_INVALID: &str = "box_service_invalid";
    pub const BOX_SERVICE_REFUSED: &str = "box_service_refused";
}

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        "/cli/app/box-service" => show(who, params),
        "/cli/app/box-service/request" => request(who, body),
        "/cli/app/box-service/revoke" => revoke(who, body),
        _ => Err(no("not_found", 404, "no such App box-service route")),
    }
}

// ── helpers ──────────────────────────────────────────────────────────

fn db<T>(f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>) -> R<T> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    f(&conn).map_err(db_err)
}

fn audit(app: &str, event: &str, by: &str, payload: J) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    if let Err(e) = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, app, event, by, None, &payload) {
        k2_core::log_debug!("[apps] audit {app} {event}: {e}");
    }
}

fn workspace_of(who: &Who) -> Option<&str> {
    match who {
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => Some(project_id.as_str()),
        _ => None,
    }
}

fn session_of(who: &Who) -> Option<&str> {
    match who {
        Who::Agent { session_id, .. } | Who::Shell { session_id, .. } => Some(session_id.as_str()),
        _ => None,
    }
}

/// Is the App walled (a forward needed), or the process exception
/// (direct, narrowed by nft), or the unwalled preview engine?
fn wall_of(app: &deploy::store::AppRow) -> &'static str {
    if app.is_process_exception() {
        "process-exception"
    } else if app.isolation == "container" || app.isolation == "vm" {
        "container"
    } else {
        "unwalled"
    }
}

/// This daemon's own TCP listeners on loopback (they move between boots,
/// so they are read, not fixed): the main port and the heartbeat port.
pub(crate) fn daemon_ports() -> Vec<u16> {
    let home = k2_core::paths::k2_home();
    ["daemon.port", "heartbeat.port"]
        .iter()
        .filter_map(|f| std::fs::read_to_string(home.join(f)).ok())
        .filter_map(|t| t.trim().parse::<u16>().ok())
        .collect()
}

/// Why `port` can never be a box service, or None.
pub(crate) fn port_refusal(port: u16, daemon: &[u16]) -> Option<String> {
    if let Some(why) = k2_root_proto::apps::box_port_refusal(port) {
        return Some(why.to_string());
    }
    if daemon.contains(&port) {
        return Some("this K2 daemon's own port (an App reaches K2 through its gateway link, K2_GATEWAY_URL)".into());
    }
    None
}

fn sew_toml() -> Option<String> {
    #[cfg(test)]
    if let Some(t) = test_seams::sew_toml() {
        return t;
    }
    std::fs::read_to_string(bs::SEW_CONFIG).ok()
}

/// Turn a raw ask (name, optional ports) into a checked [`Want`].
pub(crate) fn resolve(name: &str, host_port: Option<u16>, app_port: Option<u16>, proto: Option<&str>) -> R<Want> {
    let name = name.trim().to_ascii_lowercase();
    if !bs::name_ok(&name) {
        return Err(no(
            codes::BOX_SERVICE_INVALID,
            400,
            format!("'{name}' is not a box service name: lowercase letters, digits and '-', up to 32 (like sew-api)"),
        ));
    }
    if proto.is_some_and(|p| p != bs::PROTO_TCP) {
        return Err(no(codes::BOX_SERVICE_INVALID, 400, "a box service is tcp (HTTP and websockets ride it)"));
    }
    let (host_port, preset) = match (host_port, bs::preset(&name)) {
        (Some(p), pre) => (p, pre.map(|p| p.name.to_string())),
        (None, Some(pre)) => {
            let r = bs::resolve_preset(pre, sew_toml().as_deref()).map_err(|e| no(codes::BOX_SERVICE_REFUSED, 409, e))?;
            (r.port, Some(pre.name.to_string()))
        }
        (None, None) => {
            return Err(no(
                codes::BOX_SERVICE_INVALID,
                400,
                format!(
                    "name the host port for '{name}' (only {} are known by name)",
                    bs::PRESETS.iter().map(|p| p.name).collect::<Vec<_>>().join(" and ")
                ),
            ))
        }
    };
    if let Some(why) = port_refusal(host_port, &daemon_ports()) {
        return Err(no(codes::BOX_SERVICE_REFUSED, 403, format!("port {host_port} can never be a box service: {why}")));
    }
    let app_port = app_port.unwrap_or(host_port);
    if !bs::app_port_ok(app_port) {
        return Err(no(
            codes::BOX_SERVICE_INVALID,
            400,
            format!("port {app_port} inside the wall: 1024 or more, not 8730 (the gateway link), outside 38600-38999 (pick one with --app-port)"),
        ));
    }
    Ok(Want { name, host_port, app_port, proto: bs::PROTO_TCP.into(), preset })
}

// ── the forwards a running App holds ─────────────────────────────────

fn live() -> &'static Mutex<HashMap<String, Vec<BoxForward>>> {
    static L: OnceLock<Mutex<HashMap<String, Vec<BoxForward>>>> = OnceLock::new();
    L.get_or_init(Default::default)
}

/// What the running container was started with (the engine's answer, at
/// start or adopt).
pub(crate) fn record_live(app: &str, answer: &J) {
    let f: Vec<BoxForward> = serde_json::from_value(answer["boxForwards"].clone()).unwrap_or_default();
    live().lock().unwrap_or_else(|e| e.into_inner()).insert(app.to_string(), f);
    if let Some(gw) = answer["gwPort"].as_u64().and_then(|g| u16::try_from(g).ok()) {
        live_gw().lock().unwrap_or_else(|e| e.into_inner()).insert(app.to_string(), gw);
    }
}

/// The gateway port a running guarded App was started on (its nft rule's
/// key), for a reload when its link isn't open.
fn live_gw() -> &'static Mutex<HashMap<String, u16>> {
    static G: OnceLock<Mutex<HashMap<String, u16>>> = OnceLock::new();
    G.get_or_init(Default::default)
}

pub(crate) fn forget_live(app: &str) {
    live().lock().unwrap_or_else(|e| e.into_inner()).remove(app);
    live_gw().lock().unwrap_or_else(|e| e.into_inner()).remove(app);
}

fn live_of(app: &str) -> Option<Vec<BoxForward>> {
    live().lock().unwrap_or_else(|e| e.into_inner()).get(app).cloned()
}

/// The forwards a guarded start renders: approved and held services,
/// minus any that would sit on the instance's own port (logged by the
/// caller). Empty when the engine predates box forwards.
pub(crate) fn forwards_for(app: &str, own_port: u16) -> (Vec<BoxForward>, Vec<String>) {
    let rows = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        bs::forwards(&conn, app).unwrap_or_default()
    };
    let daemon = daemon_ports();
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    for r in rows {
        let f = BoxForward { app_port: r.app_port, host_port: r.host_port };
        if port_refusal(r.host_port, &daemon).is_some() || !k2_root_proto::engine::box_forward_ok(&f, own_port) {
            skipped.push(r.name);
            continue;
        }
        if out.len() < k2_root_proto::apps::MAX_BOX_SERVICES {
            out.push(f);
        }
    }
    (out, skipped)
}

/// `K2_BOX_<NAME>=127.0.0.1:<port>` for every approved or held service:
/// the in-wall port for a container, the host port for the process
/// exception (it has the host's loopback).
pub(crate) fn env_for(app: &str, direct: bool) -> BTreeMap<String, String> {
    let rows = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        bs::forwards(&conn, app).unwrap_or_default()
    };
    rows.iter()
        .map(|r| (r.env_name(), format!("127.0.0.1:{}", if direct { r.host_port } else { r.app_port })))
        .collect()
}

/// The approved host ports the App's nft rule admits.
pub(crate) fn allowed_ports(app: &str) -> Vec<u16> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let daemon = daemon_ports();
    bs::allowed_host_ports(&conn, app)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| port_refusal(*p, &daemon).is_none())
        .take(k2_root_proto::apps::MAX_BOX_SERVICES)
        .collect()
}

/// The App's list changed (approve, deny, revoke, expire): reload its nft
/// rule now when it runs (its gateway link is open). `Ok(None)` when the
/// App isn't running (its next start installs the rule).
pub(crate) fn changed(app: &str) -> Result<Option<Vec<u16>>, String> {
    let gw = super::gateway::port_of(app).or_else(|| live_gw().lock().unwrap_or_else(|e| e.into_inner()).get(app).copied());
    let Some(gw) = gw else { return Ok(None) };
    super::guarded::install_gateway_rule(app, gw)?;
    Ok(Some(allowed_ports(app)))
}

/// After an approve: what it means for the running App.
pub(crate) fn applied_note(r: &k2_core::app_grace::requests::AppRequest) -> J {
    let app = r.app_id.clone().unwrap_or_default();
    let nft = changed(&app);
    let live = live_of(&app);
    let names: Vec<Want> = r.box_services();
    let missing: Vec<String> = match &live {
        Some(fw) => names
            .iter()
            .filter(|w| !fw.iter().any(|f| f.app_port == w.app_port && f.host_port == w.host_port))
            .map(|w| w.name.clone())
            .collect(),
        None => Vec::new(),
    };
    let note = match (&nft, live.is_some(), missing.is_empty()) {
        (Err(e), _, _) => format!("approved, but the App's door didn't reload ({e}); restart it (k2 app restart {app})"),
        (Ok(_), true, false) => format!(
            "approved; the running App was started before {} was asked for: restart it once to open the forward (k2 app restart {app})",
            missing.join(", ")
        ),
        (Ok(Some(_)), _, _) => "approved; the door opens now (no restart)".into(),
        (Ok(None), _, _) => "approved; it opens when the App starts".into(),
    };
    json!({ "applied": r.summary, "note": note, "restartNeeded": !missing.is_empty(), "nft": nft.as_ref().ok().cloned().flatten() })
}

// ── read ─────────────────────────────────────────────────────────────

fn service_json(r: &bs::BoxService, wall: &str, live: Option<&Vec<BoxForward>>) -> J {
    let mut v = serde_json::to_value(r).unwrap_or(J::Null);
    v["env"] = json!(r.env_name());
    v["inWall"] = json!(match wall {
        "container" => format!("127.0.0.1:{}", r.app_port),
        _ => format!("127.0.0.1:{}", r.host_port),
    });
    v["forwarded"] = match (wall, live) {
        ("container", Some(fw)) => json!(fw.iter().any(|f| f.app_port == r.app_port && f.host_port == r.host_port)),
        ("container", None) => J::Null,
        _ => json!("direct"),
    };
    v["door"] = json!(if r.status == bs::STATUS_ALLOWED { "open" } else { "shut (waiting on the owner)" });
    v
}

/// The one-line count `k2 app inspect` and `/cli/app/list` show.
pub(crate) fn summary_json(app: &deploy::store::AppRow) -> J {
    let rows = db(|c| bs::list(c, &app.app_id)).unwrap_or_default();
    let allowed = rows.iter().filter(|r| r.status == bs::STATUS_ALLOWED).count();
    json!({
        "allowed": allowed,
        "pending": rows.len() - allowed,
        "services": rows.iter().map(|r| json!({ "name": r.name, "status": r.status, "hostPort": r.host_port, "appPort": r.app_port })).collect::<Vec<_>>(),
    })
}

/// GET `/cli/app/box-service?app=` → services, pending requests, presets.
fn show(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let (rows, pending) = db(|c| {
        let _ = k2_core::app_grace::requests::expire_due(c, deploy::now());
        Ok((
            bs::list(c, &app.app_id)?,
            k2_core::app_grace::requests::list(
                c,
                &k2_core::app_grace::requests::Filter {
                    app_id: Some(&app.app_id),
                    service_id: None,
                    workspace: None,
                    status: Some(k2_core::app_grace::requests::STATUS_PENDING),
                },
                50,
            )?,
        ))
    })?;
    let wall = wall_of(&app);
    let live = live_of(&app.app_id);
    let toml = sew_toml();
    let presets: Vec<J> = bs::PRESETS
        .iter()
        .map(|p| match bs::resolve_preset(p, toml.as_deref()) {
            Ok(r) => json!({ "name": p.name, "what": p.what, "hostPort": r.port, "from": r.from }),
            Err(e) => json!({ "name": p.name, "what": p.what, "error": e }),
        })
        .collect();
    let note = match wall {
        "container" => "Walled: the App dials 127.0.0.1:<in-wall port> (K2_BOX_<NAME>); pasta forwards it to the host port, and the App's own nft rule admits it only while approved.",
        "process-exception" => "The process exception has the host's loopback, narrowed by its nft rule: it dials the host port directly, admitted only while approved.",
        _ => "This App runs on the unwalled process engine (the owner's dev opt-in): it already reaches this box's loopback and nothing here is enforced. Deploy with --engine guarded to wall it.",
    };
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "wall": wall,
        "note": note,
        "services": rows.iter().map(|r| service_json(r, wall, live.as_ref())).collect::<Vec<_>>(),
        "running": live.is_some(),
        "pending": pending.iter().filter(|r| r.kind == bs::KIND_BOX_SERVICE).map(super::approvals::request_json).collect::<Vec<_>>(),
        "presets": presets,
        "refused": "below 1024 (ssh, mail, DNS…), 38600-38999 (every App's gateway and proxy), 5432 (Postgres), 2019 (Caddy admin), 38472 (box router), this daemon's ports",
    }))
}

// ── propose ──────────────────────────────────────────────────────────

fn port_field(v: &J, k: &str) -> R<Option<u16>> {
    match v.get(k) {
        None | Some(J::Null) => Ok(None),
        Some(J::Number(n)) => n
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .filter(|n| *n > 0)
            .map(Some)
            .ok_or_else(|| no(codes::BOX_SERVICE_INVALID, 400, format!("{k} must be 1-65535"))),
        Some(J::String(t)) if !t.trim().is_empty() => t
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|n| *n > 0)
            .map(Some)
            .ok_or_else(|| no(codes::BOX_SERVICE_INVALID, 400, format!("{k} must be 1-65535"))),
        Some(J::String(_)) => Ok(None),
        _ => Err(no(codes::BOX_SERVICE_INVALID, 400, format!("{k} must be a port number"))),
    }
}

/// POST `/cli/app/box-service/request {app, name, hostPort?, appPort?,
/// reason?}` → held (or "already allowed"). Never live without the owner.
fn request(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let name = s(&v, "name").ok_or_else(|| no(dcodes::BAD_REQUEST, 400, "name the service (like sew-api)"))?;
    let want = resolve(name, port_field(&v, "hostPort")?, port_field(&v, "appPort")?, s(&v, "proto"))?;
    ask(who, &app, &[want], s(&v, "reason"), bs::SOURCE_REQUEST, None)
}

fn ask(who: &Who, app: &deploy::store::AppRow, wants: &[Want], reason: Option<&str>, source: &str, deployment: Option<&str>) -> R<J> {
    let by = actor(who);
    let a = bs::Ask { source, reason, deployment_id: deployment, by: &by, workspace: workspace_of(who), session: session_of(who) };
    let (w, already) = db(|c| Ok(bs::request(c, &app.app_id, wants, &a, deploy::now())))?.map_err(|e| no(codes::BOX_SERVICE_INVALID, 409, e))?;
    let Some(w) = w else {
        return Ok(json!({ "ok": true, "status": "allowed", "alreadyAllowed": already, "hint": "already approved for this App" }));
    };
    audit(&app.app_id, "box_service_requested", &by, json!({ "request": w.id, "services": w.box_services(), "reason": w.reason, "source": source }));
    if super::access::self_widens(who) {
        // Switch 2 (prd-agent-app-permissions-v1 §2.1): the agent opens the
        // door for its own App itself; the request is applied and audited.
        let out = super::access::self_apply(&w, who)?;
        return Ok(json!({
            "ok": out["ok"],
            "status": if out["status"] == "approved" { "allowed" } else { "failed" },
            "request": out["request"],
            "services": w.box_services(),
            "alreadyAllowed": already,
            "result": out["result"],
            "hint": "opened under \"…and manage them end to end\"",
        }));
    }
    Ok(held_answer(&w, &already, wall_of(app)))
}

fn held_answer(w: &k2_core::app_grace::requests::AppRequest, already: &[String], wall: &str) -> J {
    let mut v = super::approvals::held_answer(w);
    v["alreadyAllowed"] = json!(already);
    v["services"] = json!(w.box_services());
    v["hint"] = json!(format!(
        "held for the owner: they approve it in person (k2 app approve {} from a terminal outside K2, or Settings → Apps → \
         Approvals). Until then the door stays shut{}.",
        w.id,
        if wall == "container" { " (the forward is in the wall from the next start; approving opens it without a restart)" } else { "" }
    ));
    v
}

/// File the box services a Seed's manifest asks for, before its deploy
/// starts (so the start forwards them; the door opens when the owner
/// approves). The deploy itself goes on either way.
pub fn request_manifest(who: &Who, app: &deploy::store::AppRow, seed_hash: &str, deployment: Option<&str>) -> Option<J> {
    let text = std::fs::read_to_string(deploy::seed::seed_path(seed_hash).join(deploy::manifest::MANIFEST_FILE)).ok()?;
    let m = deploy::manifest::parse(&text).ok()?;
    if m.box_services.is_empty() {
        return None;
    }
    let mut wants = Vec::new();
    let mut refused = Vec::new();
    for b in &m.box_services {
        match resolve(&b.name, b.host_port, b.app_port, b.proto.as_deref()) {
            Ok(w) => wants.push(w),
            Err(e) => refused.push(json!({ "name": b.name, "code": e.code, "hint": e.hint })),
        }
    }
    let reason = m.box_services.iter().find_map(|b| b.reason.as_deref()).unwrap_or("the Seed's manifest asks for it");
    let mut out = if wants.is_empty() {
        json!({ "status": "refused" })
    } else {
        match ask(who, app, &wants, Some(reason), bs::SOURCE_MANIFEST, deployment) {
            Ok(v) => v,
            Err(e) => json!({ "status": "error", "code": e.code, "hint": e.hint }),
        }
    };
    if !refused.is_empty() {
        out["refused"] = json!(refused);
    }
    Some(out)
}

// ── narrow ───────────────────────────────────────────────────────────

/// POST `/cli/app/box-service/revoke {app, name}`: at once (approved or
/// held). The App's nft rule reloads without it: new connections are
/// refused at once and open ones are cut (nft ruleset v4).
fn revoke(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let name = s(&v, "name").ok_or_else(|| no(dcodes::BAD_REQUEST, 400, "name the service to revoke"))?.trim().to_ascii_lowercase();
    let by = actor(who);
    let Some(row) = db(|c| bs::revoke(c, &app.app_id, &name))? else {
        return Ok(json!({ "ok": true, "revoked": false, "hint": format!("{name} isn't a box service of App {}", app.app_id) }));
    };
    // A held request left asking for nothing is withdrawn.
    if let Some(req) = row.request_id.as_deref() {
        if db(|c| bs::request_is_empty(c, &app.app_id, req))? {
            let _ = db(|c| Ok(k2_core::app_grace::requests::decide(c, req, k2_core::app_grace::requests::STATUS_WITHDRAWN, &by, None, Some("its services were revoked"), deploy::now())));
        }
    }
    let nft = changed(&app.app_id);
    audit(&app.app_id, "box_service_revoked", &by, json!({ "name": row.name, "hostPort": row.host_port, "was": row.status, "nft": nft.as_ref().err() }));
    if let Err(e) = &nft {
        return Err(Refusal {
            code: dcodes::ENGINE_UNAVAILABLE,
            status: 503,
            hint: format!("{name} is revoked, but the App's door didn't reload ({e}); stop the App (k2 app stop {}) to be sure", app.app_id),
            extra: Some(json!({ "revoked": true })),
        });
    }
    Ok(json!({ "ok": true, "revoked": true, "name": row.name, "hostPort": row.host_port, "note": "shut at once: new connections are refused and open ones cut" }))
}

#[cfg(test)]
pub(crate) mod test_seams {
    use std::sync::Mutex;

    /// `Some(None)`: no Sew config; `Some(Some(text))`: this config.
    static TOML: Mutex<Option<Option<String>>> = Mutex::new(None);

    pub fn set_sew_toml(t: Option<Option<String>>) {
        *TOML.lock().unwrap() = t;
    }
    pub(crate) fn sew_toml() -> Option<Option<String>> {
        TOML.lock().unwrap().clone()
    }
}

#[cfg(test)]
#[path = "box_service_routes_tests.rs"]
mod tests;
