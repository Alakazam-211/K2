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
//! Custom domains (0.46.6) ride the same routes plus `check`, `primary`
//! and `external` (`{app, host, external: bool}`: the owner's own web
//! server serves the name, not K2's front; see [`custom_external`]).
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

pub const ROUTES: &[&str] = &[
    "/cli/app/domains",
    "/cli/app/domain/add",
    "/cli/app/domain/rm",
    "/cli/app/domain/check",
    "/cli/app/domain/primary",
    "/cli/app/domain/external",
];
pub const POST_ROUTES: &[&str] = &[
    "/cli/app/domain/add",
    "/cli/app/domain/rm",
    "/cli/app/domain/check",
    "/cli/app/domain/primary",
    "/cli/app/domain/external",
];

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
        "/cli/app/domain/external" => custom_external(who, body),
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
    use super::custom_domain_flow as flow;
    let mut v = super::custom_domains::status_json(d, false);
    v["hostname"] = json!(d.hostname);
    // 0.46.6 (PRD §7): which path this box offers; behind a router or on a
    // desktop, Path B; a web server that isn't K2's on :443. 0.46.8: Path
    // B through K2 Connect (Pro) with its 20-name meter (`setup.pathB`).
    let advice = flow::setup_advice();
    let shown = flow::shown_path(d, &advice);
    v["records"] = J::Array(flow::records_for(d, &advice));
    v["recordsPath"] = json!(shown);
    v["recordsByPath"] = json!({
        "direct": flow::records_direct(d),
        "connect": flow::records_connect(d, &super::origins::box_subdomain()),
    });
    // No records to show (a desktop without Path B): the setup line says
    // what it needs; no DNS step to send anyone on.
    let pending = v["txtPending"].as_str().map(str::to_string);
    v["next"] = if let Some(p) = pending {
        json!(p)
    } else if advice["records"] == "none" && d.state != k2_core::domains::STATE_LIVE {
        json!(format!("No DNS to set yet. Once that's solved: k2 app domain add {} {} prints the records.", d.app, d.hostname))
    } else {
        json!(next_step(d, shown))
    };
    v["setup"] = advice;
    if let Some(h) = own_server_hint(d, k2_core::front443::load().enabled) {
        v["ownServer"] = json!(h);
    }
    v
}

/// The one line for an App behind the owner's own web server: a name
/// waiting for DNS on a box whose K2 front is off never goes live here, so
/// it is no origin until the owner says their server serves it.
fn own_server_hint(d: &k2_core::domains::AppDomain, front_enabled: bool) -> Option<String> {
    use k2_core::domains::{STATE_VERIFYING, STATE_WAITING_DNS};
    (!front_enabled && matches!(d.state.as_str(), STATE_WAITING_DNS | STATE_VERIFYING)).then(|| {
        format!("If your own web server serves this name, run: k2 app domain add {} {} --external", d.app, d.hostname)
    })
}

/// The next step in plain words, per state (PRD §9.1), for the path whose
/// records are shown (`direct` or `connect`).
fn next_step(d: &k2_core::domains::AppDomain, shown: &str) -> String {
    use k2_core::domains::*;
    let connect = shown == PATH_CONNECT;
    let front = if connect || k2_core::front443::load().enabled {
        String::new()
    } else {
        " This box's :443 front is off: path (a) needs it (the owner runs k2 front enable).".to_string()
    };
    if connect && d.state == STATE_ISSUING {
        return "Proven, and K2 Connect routes it here; K2 is getting its certificate through the relay (TLS-ALPN-01). \
                The tunnel restarts once to carry the name (a few seconds' blip)."
            .to_string();
    }
    match d.state.as_str() {
        STATE_LIVE => format!(
            "Served. Make it the App's primary origin with k2 app domain primary {} {} (then k2 app restart {}).",
            d.app, d.hostname, d.app
        ),
        STATE_ISSUING => format!("Proven; K2 is getting its certificate (TLS-ALPN-01 or DNS-01).{front}"),
        STATE_EXTERNAL => format!(
            "Your own web server serves it, not K2: K2 makes no DNS check or certificate for it. It is one of the App's \
             origins from the App's next start (k2 app restart {app}). For K2 to serve it instead: \
             k2 app domain external {app} {host} off",
            app = d.app,
            host = d.hostname
        ),
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

/// POST `/cli/app/domain/add {app, label: app.customer.com, external?}`
/// (custom). `external: true` (`--external`): the owner's own web server
/// serves the name, not K2's front; it is recorded `external` (an origin
/// from the App's next start; no TXT, no check, no certificate). An
/// already-bound name of this App is switched to `external`.
fn custom_add(who: &Who, app: &AppRow, v: &J) -> R<J> {
    use super::custom_domains as cd;
    let host = host_arg(v)?;
    // `k2 publish` visibility: a custom name goes past the site gate.
    if let Some(why) = crate::site_gate::custom_name_refusal(&app.app_id, &host) {
        let msg = why.split_once(": ").map_or(why.as_str(), |(_, m)| m).to_string();
        return Err(no(k2_core::site_visibility::DOOR_UNSUPPORTED, 409, msg));
    }
    let external = b(v, "external");
    let caller = super::routes::agent_ws(who).map(str::to_string);
    let existing = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::get_name(&conn, &host).map_err(db_err)?
    };
    let role = format!("{}{}", k2_core::domains::ROLE_APP_PREFIX, app.app_id);
    if existing.as_ref().is_some_and(|e| e.role == role) {
        let d = app_domain_of(app, &host)?;
        if external && d.state != k2_core::domains::STATE_EXTERNAL {
            return set_external(who, app, &d, true);
        }
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
            format!(
                "this box serves at most {} custom App names on both paths together (at most {} of them through K2's relays)",
                super::custom_domain_flow::MAX_NAMES,
                k2_core::tunnel::custom_names::MAX_PER_BOX
            ),
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
        if external {
            k2_core::domains::set_app_external(&conn, &host, true).map_err(db_err)?;
        }
        k2_core::domains::app_domain(&conn, &host).map_err(db_err)?.ok_or_else(|| db_err("the new row is gone"))?
    };
    let by = actor(who);
    if external {
        // The owner's own server serves it: no TXT, no check, no route.
        labels::audit(&app.app_id, "custom_domain_add", &by, json!({ "host": host, "apex": binding.apex, "external": true }));
        if existing.is_some() {
            crate::front443::rebuild_now();
        }
        super::routes::mirror(&app.app_id);
        return Ok(json!({
            "ok": true,
            "app": app.app_id,
            "custom": custom_json(&d),
            "external": true,
            "note": restart_note(&app.app_id, &host),
        }));
    }
    let d = claim_if_path_b(app, &d)?;
    let planted = plant_for(caller.as_deref(), &d, &binding);
    labels::audit(&app.app_id, "custom_domain_add", &by, json!({ "host": host, "apex": binding.apex, "txt": planted }));
    crate::front443::rebuild_now();
    // The first check now (the re-check loop keeps trying every 10 min).
    let _ = super::custom_domain_flow::check_now(&host);
    let d = app_domain_of(app, &host)?;
    super::routes::mirror(&app.app_id);
    Ok(json!({ "ok": true, "app": app.app_id, "custom": custom_json(&d), "txt": planted }))
}

/// §6: K2 plants the TXT itself on a K2-hosted zone. An agent needs
/// `dns_manage` for that (§9.3); without it the record is shown. `null`
/// when the zone isn't K2-hosted (the person sets it).
fn plant_for(caller_ws: Option<&str>, d: &k2_core::domains::AppDomain, binding: &k2_core::domains::DomainBinding) -> J {
    if !binding.dns_write || binding.is_pending_ns() {
        return J::Null;
    }
    // Path B before a claim succeeded: K2 Connect hasn't given the value.
    if !d.holds_relay_claim() && super::custom_domain_flow::shown_path(d, &super::custom_domain_flow::setup_advice()) == k2_core::domains::PATH_CONNECT {
        return json!({ "planted": false, "why": "K2 Connect hasn't given the TXT value yet" });
    }
    let may = match caller_ws.and_then(super::routes::ws_by_id) {
        Some(ws) => k2_core::workspace::settings::dns_manage_allowed_for_path(&ws.path),
        None => true,
    };
    if !may {
        return json!({ "planted": false, "why": "this agent has no DNS write (dns_manage): set the TXT below, or ask the owner" });
    }
    match super::custom_domain_flow::plant_txt(d, binding) {
        Ok(()) => json!({ "planted": true }),
        Err(e) => json!({ "planted": false, "why": e }),
    }
}

/// Path B is what K2 shows for `d`: claim now, to learn the TXT value K2
/// Connect picked. A claim that doesn't go through isn't an error here: the
/// name stays bound, says why, and K2 tries again.
fn claim_if_path_b(app: &AppRow, d: &k2_core::domains::AppDomain) -> R<k2_core::domains::AppDomain> {
    use super::custom_domain_flow as flow;
    let advice = flow::setup_advice();
    if advice["records"] == "none" || flow::shown_path(d, &advice) != k2_core::domains::PATH_CONNECT {
        return Ok(d.clone());
    }
    if let Err(e) = super::relay_claims::with_api(|api| super::relay_claims::ensure_claim(d, api)) {
        k2_core::log_debug!("[apps/domains] {}: no claim at K2 Connect yet: {e}", d.hostname);
    }
    app_domain_of(app, &d.hostname)
}

/// What an `external` name needs to reach the App: its next start.
fn restart_note(app_id: &str, host: &str) -> String {
    format!(
        "{host} is served by your own web server, not K2. It is one of the App's origins from its next start: \
         k2 app restart {app_id}"
    )
}

/// POST `/cli/app/domain/external {app, host, external: true|false}`
/// (`k2 app domain external <app> <host> on|off`): the owner's own web
/// server serves the name (`on`: `external`, an origin from the App's next
/// start; K2 stops routing, checking and renewing it), or K2's front
/// should (`off`: back to `waiting_dns`, no origin until it is live,
/// CD-A4). Same callers as `add`.
fn custom_external(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let host = host_arg(&v)?;
    let on = v
        .get("external")
        .and_then(J::as_bool)
        .ok_or_else(|| no(codes::BAD_REQUEST, 400, "say on or off: k2 app domain external <app-id> <host> on|off"))?;
    let d = app_domain_of(&app, &host)?;
    set_external(who, &app, &d, on)
}

/// Switch `d` to `external` (`on`) or back into K2's flow (`off`).
fn set_external(who: &Who, app: &AppRow, d: &k2_core::domains::AppDomain, on: bool) -> R<J> {
    let host = d.hostname.clone();
    // `off` on a name already in K2's flow changes nothing either.
    if (d.state == k2_core::domains::STATE_EXTERNAL) == on {
        return Ok(json!({ "ok": true, "already": true, "app": app.app_id, "external": on, "custom": custom_json(d) }));
    }
    let was = d.state.clone();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::set_app_external(&conn, &host, on).map_err(db_err)?;
    }
    labels::audit(&app.app_id, "custom_domain_external", &actor(who), json!({ "host": host, "external": on, "was": was }));
    // A name K2 served (live / issuing) leaves the door; one handed back
    // gets its TXT (on a K2-hosted zone) and its first check now.
    super::custom_domain_flow::serving_changed(&app.app_id);
    let mut planted = J::Null;
    if !on {
        let binding = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::domains::get_binding(&conn, &d.apex).map_err(db_err)?
        };
        if let Some(binding) = binding {
            let fresh = app_domain_of(app, &host)?;
            let fresh = claim_if_path_b(app, &fresh)?;
            planted = plant_for(super::routes::agent_ws(who), &fresh, &binding);
        }
        let _ = super::custom_domain_flow::check_now(&host);
    }
    let d = app_domain_of(app, &host)?;
    super::routes::mirror(&app.app_id);
    let note = if on {
        restart_note(&app.app_id, &host)
    } else {
        format!(
            "K2's front serves {host} once its DNS checks pass and it is live; until then it is not one of the App's \
             origins. Then: k2 app restart {}",
            app.app_id
        )
    };
    Ok(json!({ "ok": true, "app": app.app_id, "external": on, "custom": custom_json(&d), "txt": planted, "note": note }))
}

/// POST `/cli/app/domain/rm {app, label: app.customer.com}` (custom): the
/// binding and its route go now (and K2's TXT on a K2-hosted zone); the
/// certificate files in `~/.k2/certs/<host>/` stay (PRD §8.5).
fn custom_rm(who: &Who, app: &AppRow, v: &J) -> R<J> {
    let host = host_arg(v)?;
    let d = app_domain_of(app, &host)?;
    // Path B (PB-3): release at K2 Connect first (the relay stops routing
    // it), then the tunnel's custom proxy and the local route drop it. A
    // control plane that doesn't answer never blocks the removal: it frees
    // the name after its own re-check fails.
    let relay_note = match super::relay_claims::with_api(|api| super::relay_claims::release(&d, api)) {
        Ok(true) => Some("released at K2 Connect; the tunnel restarts once to drop it (a few seconds' blip)".to_string()),
        Ok(false) => None,
        Err(e) => Some(format!(
            "K2 Connect didn't confirm the release ({e}); it frees the name once its own re-check fails"
        )),
    };
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
    super::custom_domain_flow::serving_changed(&app.app_id);
    // No front443 here: the shared table (and the tunnel's list) still
    // drops the name now.
    super::custom_names::reload();
    labels::audit(&app.app_id, "custom_domain_rm", &actor(who), json!({ "host": host, "relay": relay_note }));
    super::routes::mirror(&app.app_id);
    let mut note = "no longer served; restart the App so its origins drop it".to_string();
    if let Some(r) = &relay_note {
        note = format!("{note}. {r}");
    }
    Ok(json!({ "ok": true, "app": app.app_id, "removed": host, "relay": relay_note, "note": note }))
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
    let before = app_domain_of(&app, &host)?;
    if k2_core::airgap::enabled() {
        return Err(no("airgap", 409, "air-gap is on: K2 makes no DNS checks"));
    }
    let (d, o) = super::custom_domain_flow::check_now(&host).ok_or_else(|| no(NOT_ATTACHED, 404, format!("{host} is gone")))?;
    // This check claimed the name and K2 Connect gave a new TXT value: on a
    // K2-hosted zone K2 sets it, as `add` does. It shows up for the next check.
    if d.verify_token != before.verify_token {
        let binding = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::domains::get_binding(&conn, &d.apex).map_err(db_err)?
        };
        if let Some(b) = binding {
            let _ = plant_for(super::routes::agent_ws(who), &d, &b);
        }
    }
    // A name that changed state is served (or dropped) at once.
    super::custom_names::reload();
    Ok(json!({ "ok": true, "app": app.app_id, "custom": custom_json(&d), "check": { "ok": o.ok, "txt": o.txt_ok, "pointsHere": o.here, "path": o.path, "seen": o.seen, "reason": o.reason } }))
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
    // App-governed Spaces S7 (AG-13b): the primary decides the App's Sew
    // origin; the sweep sends it now, not at the 10-minute one.
    k2_core::tool_hook::kick_reconcile();
    let d = app_domain_of(&app, &d.hostname)?;
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "primary": host,
        "live": d.is_live(),
        // An `external` name (the owner's own server serves it) is an
        // origin already: primary from the next start too.
        "note": if d.is_origin() {
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
        // 0.46.8: Path B on this box (Pro, behind K2 Connect) and the
        // 20-name meter; the 100-name cap counts both paths (PB-A13).
        "pathB": super::custom_domain_flow::setup_advice()["pathB"].clone(),
        "maxCustomNames": super::custom_domain_flow::MAX_NAMES,
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
    if b(&v, "external") {
        return Err(no(
            codes::BAD_REQUEST,
            400,
            "--external is for your own domain (e.g. shop.example.com): a <box>.k2.dev subdomain is always served by K2",
        ));
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

    /// Rosson 2026-11-01: for an agent without "Build and run Apps", every
    /// custom-domain verb (Path B's too) is the one broad refusal,
    /// `apps_manage_off`, which teaches `k2 permission request apps.build`:
    /// never a per-domain or web-server card.
    #[test]
    fn an_agent_without_the_app_switch_gets_the_broad_refusal_on_every_verb() {
        let _db = k2_core::db::scoped_for_test();
        let _home = crate::test_support::TempHome::new();
        setup();
        build_switch("ws-nsi", false);
        let me = agent("ws-nsi");
        for (path, body) in [
            ("/cli/app/domain/add", json!({ "app": "agentnsi", "label": HOST })),
            ("/cli/app/domain/check", json!({ "app": "agentnsi", "host": HOST })),
            ("/cli/app/domain/rm", json!({ "app": "agentnsi", "label": HOST })),
            ("/cli/app/domain/primary", json!({ "app": "agentnsi", "host": HOST })),
            ("/cli/app/domain/external", json!({ "app": "agentnsi", "host": HOST, "external": true })),
        ] {
            let e = post(&me, path, body).unwrap_err();
            assert_eq!((e.code, e.status), (codes::APPS_MANAGE_OFF, 403), "{path}: {}", e.hint);
            assert!(e.hint.contains("Build and run Apps"), "{path}: {}", e.hint);
        }
    }

    /// K2 Web's review: K2 Connect picks the TXT value. On a box where Path B
    /// is what K2 shows, `add` claims first and shows K2 Connect's value, never
    /// the box's; while the claim doesn't go through, the CNAME shows alone and
    /// says why, and nothing is planted.
    #[test]
    fn add_claims_first_and_shows_the_txt_k2_connect_picked() {
        use crate::apps::relay_claims::{self, ClaimAnswer, CpError, RelayApi};
        use std::cell::RefCell;
        use std::rc::Rc;
        const PICKED: &str = "relay-picked-token-0123456789";
        struct Cp {
            down: RefCell<bool>,
            claims: RefCell<u32>,
        }
        impl RelayApi for Cp {
            fn claim(&self, h: &str, _: Option<bool>) -> Result<ClaimAnswer, CpError> {
                *self.claims.borrow_mut() += 1;
                if *self.down.borrow() {
                    return Err(CpError::Retry("K2 Connect's database didn't answer: K2 tries again".into()));
                }
                Ok(ClaimAnswer { hostname: h.into(), status: "pending".into(), verify_token: Some(PICKED.into()), last_error: None })
            }
            fn verify(&self, _: &str) -> Result<ClaimAnswer, CpError> {
                panic!("no verify before the TXT is seen")
            }
            fn release(&self, _: &str) -> Result<(), CpError> {
                Ok(())
            }
            fn list(&self) -> Result<Vec<ClaimAnswer>, CpError> {
                Ok(vec![])
            }
            fn http_redirect(&self, _: Option<&str>, _: bool) -> Result<(), CpError> {
                Ok(())
            }
        }
        let _db = k2_core::db::scoped_for_test();
        let _home = crate::test_support::TempHome::new();
        setup();
        let mut cfg = k2_core::tunnel::config::TunnelConfig::default();
        cfg.subdomain = "box1".into();
        k2_core::tunnel::config::save(&cfg).unwrap();
        // Behind a router, on Pro: Path B is what K2 shows.
        flow::set_test_nat(Some(Some(true)));
        relay_claims::set_test_pro(Some(Ok(true)));
        relay_claims::set_test_ready(Some(Ok(())));
        let cp = Rc::new(Cp { down: RefCell::new(true), claims: RefCell::new(0) });
        relay_claims::set_test_api(Some(cp.clone()));
        let me = agent("ws-nsi");

        // K2 Connect not answering: bound, the CNAME alone, the reason, no TXT.
        let v = post(&me, "/cli/app/domain/add", json!({ "app": "agentnsi", "label": HOST })).unwrap();
        let c = &v["custom"];
        assert_eq!(*cp.claims.borrow(), 1);
        let recs = c["records"].as_array().unwrap();
        assert_eq!(recs.len(), 1, "{c}");
        assert_eq!((recs[0]["type"].as_str(), recs[0]["value"].as_str()), (Some("CNAME"), Some("agentnsi.box1.k2.dev")));
        assert!(c["txt"].is_null(), "the box's token is never shown: {c}");
        let p = c["txtPending"].as_str().unwrap();
        assert!(p.contains("No TXT record to set yet") && p.contains("database didn't answer"), "{p}");
        assert_eq!(c["next"].as_str(), Some(p));

        // It answers: the next list or check shows ITS value.
        *cp.down.borrow_mut() = false;
        flow::set_test_lookup(None);
        let d = app_domain_of(&app_for(&me, Some("agentnsi")).unwrap(), HOST).unwrap();
        relay_claims::with_api(|api| relay_claims::ensure_claim(&d, api)).unwrap();
        let list = handle("/cli/app/domains", &me, b"", &HashMap::from([("app".to_string(), "agentnsi".to_string())])).unwrap();
        let c = &list["custom"][0];
        let recs = c["records"].as_array().unwrap();
        assert_eq!(recs.len(), 2, "{c}");
        assert_eq!(recs[1]["name"].as_str(), Some("_k2-domain.nsi.agentnsi.com"));
        assert_eq!(recs[1]["value"].as_str(), Some("k2-verify=relay-picked-token-0123456789"));
        assert_eq!(c["txt"]["value"].as_str(), Some("k2-verify=relay-picked-token-0123456789"));
        assert!(c.get("txtPending").is_none(), "{c}");

        flow::set_test_nat(None);
        relay_claims::set_test_api(None);
        relay_claims::set_test_pro(None);
        relay_claims::set_test_ready(None);
    }

    /// PB-3: removing a Path B name releases it at K2 Connect first, then the
    /// row goes. A control plane that doesn't answer never blocks the removal
    /// and says so; a name with no claim never calls it.
    #[test]
    fn rm_releases_a_path_b_name_at_the_control_plane_first() {
        use crate::apps::relay_claims::{self, ClaimAnswer, CpError, RelayApi};
        use std::cell::RefCell;
        use std::rc::Rc;
        struct Cp {
            calls: RefCell<Vec<String>>,
            down: bool,
        }
        impl RelayApi for Cp {
            fn claim(&self, _: &str, _: Option<bool>) -> Result<ClaimAnswer, CpError> {
                panic!("no claim on rm")
            }
            fn http_redirect(&self, _: Option<&str>, _: bool) -> Result<(), CpError> {
                panic!("no switch on rm")
            }
            fn verify(&self, _: &str) -> Result<ClaimAnswer, CpError> {
                panic!("no verify on rm")
            }
            fn release(&self, h: &str) -> Result<(), CpError> {
                self.calls.borrow_mut().push(h.to_string());
                if self.down {
                    return Err(CpError::Unreachable("HTTP 502".into()));
                }
                Ok(())
            }
            fn list(&self) -> Result<Vec<ClaimAnswer>, CpError> {
                panic!("no list on rm")
            }
        }
        let _db = k2_core::db::scoped_for_test();
        let _home = crate::test_support::TempHome::new();
        setup();
        let bind = |h: &str, relay: Option<&str>| {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::domains::upsert_name(&conn, h, "agentnsi.com", "app:agentnsi").unwrap();
            k2_core::domains::set_relay_state(&conn, h, relay, None, 1).unwrap();
        };
        let me = agent("ws-nsi");

        bind(HOST, Some(k2_core::domains::RELAY_VERIFIED));
        let cp = Rc::new(Cp { calls: RefCell::new(vec![]), down: false });
        relay_claims::set_test_api(Some(cp.clone()));
        let v = post(&me, "/cli/app/domain/rm", json!({ "app": "agentnsi", "label": HOST })).unwrap();
        assert_eq!(*cp.calls.borrow(), vec![HOST.to_string()]);
        assert!(v["relay"].as_str().unwrap().contains("released at K2 Connect"), "{v}");
        assert!(v["note"].as_str().unwrap().contains("a few seconds' blip"), "{v}");

        bind(HOST, Some(k2_core::domains::RELAY_CLAIMED));
        let cp = Rc::new(Cp { calls: RefCell::new(vec![]), down: true });
        relay_claims::set_test_api(Some(cp.clone()));
        let v = post(&me, "/cli/app/domain/rm", json!({ "app": "agentnsi", "label": HOST })).unwrap();
        assert!(v["relay"].as_str().unwrap().contains("didn't confirm the release"), "{v}");
        assert_eq!(v["removed"], HOST);

        bind(HOST, None);
        let cp = Rc::new(Cp { calls: RefCell::new(vec![]), down: false });
        relay_claims::set_test_api(Some(cp.clone()));
        let v = post(&me, "/cli/app/domain/rm", json!({ "app": "agentnsi", "label": HOST })).unwrap();
        assert!(cp.calls.borrow().is_empty(), "no claim: nothing to release");
        assert!(v["relay"].is_null(), "{v}");
        relay_claims::set_test_api(None);
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

        // Primary: accepted, says it waits for live, and asks for a sweep
        // so Sew's origin follows now (S7, AG-13b).
        let _ = k2_core::tool_hook::take_reconcile_request();
        let v = post(&me, "/cli/app/domain/primary", json!({ "app": "agentnsi", "host": HOST })).unwrap();
        assert_eq!((v["primary"].as_str(), v["live"].as_bool()), (Some(HOST), Some(false)));
        assert!(k2_core::tool_hook::take_reconcile_request(), "primary asked for a sweep");
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
    /// A neutral App on its own apex, for the 0.46.8 tests below.
    fn setup_shop() {
        runtime_app("ws-shop", "shop");
        build_switch("ws-shop", true);
        flow::set_test_box_ips(Some(vec!["203.0.113.10".parse().unwrap()]));
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::upsert_binding(&conn, "shop.example", None, false).unwrap();
    }

    /// 0.46.8: a name that turns live after `k2 app links set` answers the
    /// link files at once (it used to wait for the next deploy); removed,
    /// it stops.
    #[test]
    fn link_file_hosts_follow_a_custom_domain_turning_live_and_going_away() {
        use k2_core::skin::app_pairing as pairing;
        let _db = k2_core::db::scoped_for_test();
        let _home = crate::test_support::TempHome::new();
        setup_shop();
        let me = agent("ws-shop");
        let host = "links.shop.example";
        let service = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::deploy::store::get_app(&conn, "shop").unwrap().expect("the App").service_id
        };
        let input = pairing::LinksInput { ios: vec!["TEAMID1234.com.you.app".into()], android: vec![], paths: vec![], drop: vec![] };
        pairing::set_links(&service, &input, "test", 1).expect("set links");
        crate::app_pairing::refresh_links(&service);
        let hosts = || crate::app_pairing::links::get_files("shop").expect("link files").hosts.clone();
        assert!(!hosts().iter().any(|h| h == host), "not bound yet: {:?}", hosts());

        let v = post(&me, "/cli/app/domain/add", json!({ "app": "shop", "label": host })).unwrap();
        assert!(!hosts().iter().any(|h| h == host), "bound, not live: {:?}", hosts());

        // Proven with a usable certificate: live, and a link host now.
        let pem = crate::domains::acme::test_le_pem(host);
        crate::domains::store::install(host, &pem.chain_pem, &pem.key_pem).unwrap();
        let txt = v["custom"]["records"].as_array().unwrap().iter().find(|r| r["type"] == "TXT").unwrap().clone();
        flow::set_test_lookup(Some(Box::new(Proven(txt["value"].as_str().unwrap().to_string()))));
        let _ = k2_core::tool_hook::take_reconcile_request();
        let v = post(&me, "/cli/app/domain/check", json!({ "app": "shop", "host": host })).unwrap();
        assert_eq!(v["custom"]["state"], "live", "{v}");
        assert!(hosts().iter().any(|h| h == host), "live: a link host at once: {:?}", hosts());
        assert!(k2_core::tool_hook::take_reconcile_request(), "Sew's origins follow now too");

        let _ = post(&me, "/cli/app/domain/rm", json!({ "app": "shop", "label": host })).unwrap();
        assert!(!hosts().iter().any(|h| h == host), "removed: {:?}", hosts());
        flow::set_test_lookup(None);
        crate::app_pairing::links::set_files("shop", None);
    }

    /// 0.46.8: Sew's origin stays the one primary; the list carries every
    /// live host, primary first, and a name that isn't live is in neither.
    #[test]
    fn sews_origin_list_is_every_live_host_primary_first() {
        let _db = k2_core::db::scoped_for_test();
        let _home = crate::test_support::TempHome::new();
        setup_shop();
        let (a, b, waiting) = ("a.shop.example", "b.shop.example", "wait.shop.example");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            for h in [a, b, waiting] {
                k2_core::domains::upsert_name(&conn, h, "shop.example", "app:shop").unwrap();
            }
            for h in [a, b] {
                k2_core::domains::set_app_state(&conn, h, k2_core::domains::STATE_LIVE, None).unwrap();
            }
            k2_core::domains::set_primary(&conn, "shop", b).unwrap();
        }
        let service = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::deploy::store::get_app(&conn, "shop").unwrap().expect("the App").service_id
        };
        let list = crate::sew::home::signalling_origins(&service, "shop");
        assert_eq!(list.first().map(String::as_str), Some("wss://b.shop.example"), "primary first: {list:?}");
        assert!(list.iter().any(|o| o == "wss://a.shop.example"), "{list:?}");
        assert!(!list.iter().any(|o| o.contains("wait.")), "not live, not an origin: {list:?}");
        assert!(list.iter().all(|o| k2_core::tool_hook::home::is_sew_origin(o)), "each one Sew takes: {list:?}");
        assert_eq!(crate::sew::home::signalling_origin(&service, "shop").as_deref(), list.first().map(String::as_str), "the one origin is the list's first");
    }

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

    /// No DNS read may happen: an `external` name is never checked.
    struct NoLookup;
    impl flow::Lookup for NoLookup {
        fn txt(&self, n: &str) -> Result<Vec<String>, String> {
            panic!("an external name was checked (TXT {n})")
        }
        fn addrs(&self, h: &str) -> Result<Vec<IpAddr>, String> {
            panic!("an external name was checked (A {h})")
        }
        fn cname(&self, h: &str) -> Option<String> {
            panic!("an external name was checked (CNAME {h})")
        }
    }

    fn origins_of(app: &str) -> Vec<String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::origin_hostnames_for_app(&conn, app).unwrap()
    }

    /// 0.46.6 parity with 0.46.5 for an App behind the owner's own web
    /// server: `--external` records the name as `external` (an origin, no
    /// TXT, no check, no certificate, no route); without it the name waits
    /// for DNS and is no origin (CD-A4), and says how to opt in when K2's
    /// front is off; `off` hands it back to K2's flow.
    #[test]
    fn external_names_are_origins_and_go_back_to_k2s_flow() {
        let _db = k2_core::db::scoped_for_test();
        let _home = crate::test_support::TempHome::new();
        runtime_app("ws-shop", "shop");
        build_switch("ws-shop", true);
        flow::set_test_box_ips(Some(vec!["203.0.113.10".parse().unwrap()]));
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::domains::upsert_binding(&conn, "example.com", None, false).unwrap();
        }
        let me = agent("ws-shop");
        assert!(!k2_core::front443::load().enabled, "this box has no K2 front");

        // Without --external: waiting_dns, no origin, and the one line.
        flow::set_test_lookup(Some(Box::new(Proven("unrelated".into()))));
        let v = post(&me, "/cli/app/domain/add", json!({ "app": "shop", "label": "shop2.example.com" })).unwrap();
        assert_eq!(v["custom"]["state"], "waiting_dns", "{v}");
        assert_eq!(v["custom"]["notServedYet"], true, "{v}");
        assert_eq!(
            v["custom"]["ownServer"],
            "If your own web server serves this name, run: k2 app domain add shop shop2.example.com --external",
            "{v}"
        );
        assert!(origins_of("shop").is_empty(), "a bound name nothing serves is no origin (CD-A4)");

        // The same name with --external: switched, an origin, never checked.
        flow::set_test_lookup(Some(Box::new(NoLookup)));
        let v = post(&me, "/cli/app/domain/add", json!({ "app": "shop", "label": "shop2.example.com", "external": true })).unwrap();
        assert_eq!((v["custom"]["state"].as_str(), v["external"].as_bool()), (Some("external"), Some(true)), "{v}");
        assert_eq!(v["custom"]["notServedYet"], false, "{v}");
        assert!(v["custom"].get("ownServer").is_none(), "{v}");
        assert!(v["note"].as_str().unwrap().ends_with("k2 app restart shop"), "{v}");
        assert!(v["custom"]["next"].as_str().unwrap().contains("k2 app domain external shop shop2.example.com off"), "{v}");
        assert_eq!(origins_of("shop"), vec!["shop2.example.com".to_string()]);

        // A new name bound --external from the start: no TXT, no check.
        let v = post(&me, "/cli/app/domain/add", json!({ "app": "shop", "label": "www.example.com", "external": true })).unwrap();
        assert_eq!(v["custom"]["state"], "external", "{v}");
        assert!(v.get("txt").is_none(), "{v}");
        assert_eq!(origins_of("shop"), vec!["shop2.example.com".to_string(), "www.example.com".to_string()]);
        let again = post(&me, "/cli/app/domain/add", json!({ "app": "shop", "label": "www.example.com", "external": true })).unwrap();
        assert_eq!(again["already"], true, "{again}");
        let again = post(&me, "/cli/app/domain/external", json!({ "app": "shop", "host": "www.example.com", "external": true })).unwrap();
        assert_eq!(again["already"], true, "{again}");
        assert!(flow::take_test_issues().is_empty(), "no certificate for an external name");
        assert!(crate::apps::tls_door::reload().routed.is_empty(), "the door never routes an external name");

        // Off: back to K2's flow (checked now, no origin until live).
        flow::set_test_lookup(Some(Box::new(Proven("unrelated".into()))));
        let v = post(&me, "/cli/app/domain/external", json!({ "app": "shop", "host": "shop2.example.com", "external": false })).unwrap();
        assert_eq!((v["custom"]["state"].as_str(), v["external"].as_bool()), (Some("waiting_dns"), Some(false)), "{v}");
        assert!(v["custom"]["checkedAt"].is_i64(), "checked at once: {v}");
        assert_eq!(origins_of("shop"), vec!["www.example.com".to_string()]);
        let again = post(&me, "/cli/app/domain/external", json!({ "app": "shop", "host": "shop2.example.com", "external": false })).unwrap();
        assert_eq!(again["already"], true, "{again}");

        // Refusals: no on|off, a k2.dev label, another workspace's agent,
        // a Member login, an agent without "Build and run Apps".
        let e = post(&me, "/cli/app/domain/external", json!({ "app": "shop", "host": "shop2.example.com" })).unwrap_err();
        assert_eq!((e.status, e.code), (400, codes::BAD_REQUEST), "{e:?}");
        let e = post(&me, "/cli/app/domain/add", json!({ "app": "shop", "label": "agents", "external": true })).unwrap_err();
        assert_eq!((e.status, e.code), (400, codes::BAD_REQUEST), "{e:?}");
        runtime_app("ws-other", "crm");
        build_switch("ws-other", true);
        let e = post(&agent("ws-other"), "/cli/app/domain/external", json!({ "app": "shop", "host": "shop2.example.com", "external": true }))
            .unwrap_err();
        assert_eq!(e.code, codes::APP_NOT_FOUND, "{e:?}");
        let member = Who::Login { username: "mo".into(), role: k2_core::connect_users::Role::Member };
        let e = post(&member, "/cli/app/domain/external", json!({ "app": "shop", "host": "shop2.example.com", "external": true }))
            .unwrap_err();
        assert_eq!(e.status, 403, "{e:?}");
        build_switch("ws-shop", false);
        let e = post(&me, "/cli/app/domain/add", json!({ "app": "shop", "label": "shop2.example.com", "external": true })).unwrap_err();
        assert_eq!((e.status, e.code), (403, codes::APPS_MANAGE_OFF), "{e:?}");
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(k2_core::domains::app_domain(&conn, "shop2.example.com").unwrap().unwrap().state, "waiting_dns", "untouched");
        drop(conn);
        flow::set_test_lookup(None);
        flow::set_test_box_ips(None);
    }

    /// The opt-in line shows only where it helps: a name waiting for DNS
    /// on a box without K2's front.
    #[test]
    fn the_own_server_line_is_for_a_waiting_name_without_the_front() {
        let d = |state: &str| k2_core::domains::AppDomain {
            hostname: "shop2.example.com".into(),
            apex: "example.com".into(),
            app: "shop".into(),
            state: state.into(),
            verify_token: "t".into(),
            verified_at: None,
            checked_at: None,
            path: None,
            is_primary: false,
            last_error: None,
            failing_since: None,
            apex_attached: true,
            relay_state: None,
            relay_error: None,
            relay_checked_at: None,
        };
        use k2_core::domains::*;
        for s in [STATE_WAITING_DNS, STATE_VERIFYING] {
            assert_eq!(
                own_server_hint(&d(s), false).as_deref(),
                Some("If your own web server serves this name, run: k2 app domain add shop shop2.example.com --external")
            );
            assert_eq!(own_server_hint(&d(s), true), None, "K2's front is on: path (a) applies");
        }
        for s in [STATE_ISSUING, STATE_LIVE, STATE_FAILING, STATE_EXTERNAL] {
            assert_eq!(own_server_hint(&d(s), false), None, "{s}");
        }
    }
}
