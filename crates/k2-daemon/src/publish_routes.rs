//! `/cli/publish/*` — daemon-owned published services.
//!
//! GET: list, logs, leftovers. POST: run/start/stop/rm + subdomain
//! claim/unclaim (self-only 0074 stamp). Mutating routes are POST-only
//! (GET twins 405). Not under `/cli/tunnel/` (owner-only deny).

use std::collections::HashMap;
use std::path::PathBuf;

use k2_core::db::schema::{SelfUnclaim, SubdomainWorkspace};
use k2_core::tunnel::config::SUBDOMAIN_HOST;

use crate::cli::{need_project, opt_param, str_param};
use crate::cli_response::CliResponse;
use crate::label_owner;
use crate::publish_runtime::{self, ProbeTimeout, PublishError, RunSpec};

const FOREIGN_STICKER: &str =
    "label_owned_by_other_workspace: not yours; ask the server owner to transfer it";

fn resolve_project_id(project: &str) -> Result<String, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT id FROM projects WHERE path = ?1 OR id = ?1",
        rusqlite::params![project],
        |r| r.get(0),
    )
    .map_err(|_| format!("no registered workspace matches {project:?}"))
}

fn resolve_project_path(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT path FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| r.get(0),
    )
    .ok()
}

fn err_to_resp(e: PublishError) -> CliResponse {
    let body = match e.body {
        Some(b) => b.to_string(),
        None => serde_json::json!({ "error": e.message }).to_string(),
    };
    CliResponse {
        status: e.status,
        content_type: "application/json",
        body,
    }
}

fn boolish(params: &HashMap<String, String>, keys: &[&str]) -> bool {
    for k in keys {
        if let Some(v) = params.get(*k) {
            if matches!(v.as_str(), "1" | "true" | "on" | "yes") {
                return true;
            }
        }
    }
    false
}

fn parse_port(params: &HashMap<String, String>) -> Result<u16, CliResponse> {
    let raw = str_param(params, "port");
    if raw.trim().is_empty() {
        return Err(CliResponse::bad_request("Missing port"));
    }
    raw.parse::<u16>()
        .map_err(|_| CliResponse::bad_request("port must be an integer 1–65535"))
        .and_then(|p| {
            if p == 0 {
                Err(CliResponse::bad_request("Missing port"))
            } else {
                Ok(p)
            }
        })
}

/// `--probe-timeout` on `/cli/publish/run`: `probeTimeout` is a duration
/// (`90s`, `10m`, `1500ms`, bare seconds) or `default`; `probeTimeoutMs`
/// is whole milliseconds. Absent = keep the stored grace (new rows get the
/// default). Capped at 15 min.
fn parse_probe_timeout_param(params: &HashMap<String, String>) -> Result<ProbeTimeout, CliResponse> {
    if let Some(raw) = opt_param(params, "probeTimeoutMs").or_else(|| opt_param(params, "probe_timeout_ms")) {
        let ms: u64 = raw.trim().parse().map_err(|_| {
            CliResponse::bad_request("probeTimeoutMs must be a whole number of milliseconds (at most 900000 = 15m)")
        })?;
        if ms == 0 || ms > k2_core::published_services::PROBE_TIMEOUT_MAX_MS {
            return Err(CliResponse::bad_request(
                "--probe-timeout must be more than 0 and at most 15m (probeTimeoutMs 1..=900000)",
            ));
        }
        return Ok(ProbeTimeout::Ms(ms));
    }
    if let Some(raw) = opt_param(params, "probeTimeout").or_else(|| opt_param(params, "probe_timeout")) {
        if raw.trim().eq_ignore_ascii_case("default") {
            return Ok(ProbeTimeout::Default);
        }
        return k2_core::published_services::parse_probe_timeout(&raw)
            .map(ProbeTimeout::Ms)
            .map_err(CliResponse::bad_request);
    }
    Ok(ProbeTimeout::Keep)
}

/// The held-request form of a run's `--probe-timeout` (`"<ms>ms"`,
/// `"default"`, or null).
fn probe_timeout_body(p: ProbeTimeout) -> serde_json::Value {
    match p {
        ProbeTimeout::Keep => serde_json::Value::Null,
        ProbeTimeout::Default => serde_json::json!("default"),
        ProbeTimeout::Ms(ms) => serde_json::json!(format!("{ms}ms")),
    }
}

/// GET dispatcher: list + logs + leftovers + 405 twins for POST-only paths.
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    if !path.starts_with("/cli/publish/") {
        return None;
    }
    Some(match path {
        "/cli/publish/list" => handle_list(params),
        "/cli/publish/logs" => handle_logs(params),
        "/cli/publish/leftovers" => handle_leftovers(params),
        "/cli/publish/validate" => handle_validate(params),
        "/cli/publish/client-ip/list" => handle_client_ip_list(),
        "/cli/publish/client-ip"
        | "/cli/publish/run"
        | "/cli/publish/start"
        | "/cli/publish/stop"
        | "/cli/publish/rm"
        | "/cli/publish/subdomain/claim"
        | "/cli/publish/subdomain/unclaim"
        | "/cli/publish/frame" => CliResponse::method_not_allowed(),
        _ => CliResponse::not_found(),
    })
}

thread_local! {
    /// The classified caller of the current POST (set by
    /// [`dispatch_post_as`]); `None` = not classified (unit tests, old
    /// call sites), treated as a human.
    static CALLER: std::cell::RefCell<Option<crate::compute_routes::Who>> = const { std::cell::RefCell::new(None) };
}

/// [`dispatch_post`] with the classified caller, so `run` can apply the
/// App runtime's `--cmd` mapping (prd-k2-app-runtime-v1 §8.2, S13).
pub fn dispatch_post_as(path: &str, params: &HashMap<String, String>, who: crate::compute_routes::Who) -> CliResponse {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            CALLER.with(|c| *c.borrow_mut() = None);
        }
    }
    let _clear = Clear;
    CALLER.with(|c| *c.borrow_mut() = Some(who));
    dispatch_post(path, params)
}

fn caller_agent_ws() -> Option<String> {
    CALLER.with(|c| match c.borrow().as_ref() {
        Some(crate::compute_routes::Who::Agent { project_id, .. }) => Some(project_id.clone()),
        _ => None,
    })
}

/// The release the `--cmd` mapping is enforced from (§8.2: "before
/// 0.46.1: unchanged"). Its own release, not the cut-over's: a NEW agent
/// `--cmd` service is gated from 0.46.1, while existing services keep
/// their environment (no cut-over is set: [`k2_core::app_grace::CUTOVER_RELEASE`]).
pub const PUBLISH_RUN_GATE_FROM: &str = "0.46.1";

/// 0.46.1 (lane 1, K5): the gate holds an agent's open-path run for the
/// owner wherever the box has the open path. The daemon's own
/// environment (the owner's, never an agent's) can change that:
/// `K2_OPEN_APPS_GATE=warn` puts back 0.46.0's warn-only answer
/// everywhere; `=enforce` holds an agent's new `--cmd` run even where the
/// open path isn't ready (it then runs as before once approved).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateMode {
    Warn,
    OpenPath,
    Always,
}

fn gate_mode() -> GateMode {
    gate_mode_by(std::env::var("K2_OPEN_APPS_GATE").ok().as_deref())
}

fn gate_mode_by(v: Option<&str>) -> GateMode {
    match v.map(str::trim) {
        Some("warn") => GateMode::Warn,
        Some("enforce") => GateMode::Always,
        _ => GateMode::OpenPath,
    }
}

/// Does the gate hold this run (given where it would go)? A re-run of an
/// existing legacy service by an agent without "Build and run Apps"
/// (`switch_off`) is held wherever it goes.
fn gate_holds(mode: GateMode, route: &CmdRoute, switch_off: bool) -> bool {
    match mode {
        GateMode::Warn => false,
        GateMode::Always => true,
        GateMode::OpenPath => switch_off || matches!(route, CmdRoute::Open),
    }
}

/// Why an agent's run is gated (`publish_run_gate`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Gate {
    why: String,
    /// A re-run of an existing legacy service by an agent whose switch 1
    /// is off (held on every route, not only the open path).
    switch_off: bool,
}

/// The warn-only answer a gated run carries where the gate doesn't hold
/// (the open path isn't ready on this box, or the owner set
/// `K2_OPEN_APPS_GATE=warn`): the run went through (`held: false`);
/// `message` says so, then what the gate asks for.
fn crossover_notice(name: &str, why: &str) -> serde_json::Value {
    let mut c = why.chars();
    let why_sentence: String = c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
    serde_json::json!({
        "from": PUBLISH_RUN_GATE_FROM,
        "mode": "warn",
        "held": false,
        "wouldHold": true,
        "hint": why,
        "message": format!("{name} ran unwalled, without the open-path gate (this box has no open path, or the owner set the gate to warn). {why_sentence}"),
    })
}

/// §8.2 / §5.11.5, prd-agent-app-permissions-v1 N1 (Rosson 2026-10-20): a
/// **new** `--cmd` service from an agent is an open-path App, and the open
/// path loosens the wall: every new service waits for the owner's
/// in-person confirm (one held request per service; the "…and publish
/// open Apps" standing approval is retired). A re-run or restart of an
/// existing legacy service stays free under switch 1 "Build and run Apps";
/// without it the re-run is held too. `None` = no gate applies (a human,
/// `--skin`, another workspace's call, or a legacy re-run with switch 1).
fn publish_run_gate(project_id: &str, name: &str, skin: bool) -> Option<Gate> {
    let ws = caller_agent_ws()?;
    if skin || ws != project_id {
        return None;
    }
    let legacy = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::get_by_project_name(&conn, project_id, name)
            .ok()
            .flatten()
            .is_some_and(|r| r.cmd != k2_core::published_services::CMD_APP_SENTINEL)
    };
    gate_for(legacy, crate::apps::access::level(project_id))
}

/// The gate for an agent's run: `legacy` = an existing `--cmd` service.
fn gate_for(legacy: bool, level: crate::apps::access::Level) -> Option<Gate> {
    if legacy {
        if level >= crate::apps::access::Level::Build {
            return None;
        }
        return Some(Gate {
            why: "re-running this service needs \"Build and run Apps\" on this workspace (the owner turns it on: \
                  k2 app access <workspace> --build on), or the owner approves this run"
                .into(),
            switch_off: true,
        });
    }
    Some(Gate {
        why: format!(
            "from K2 {PUBLISH_RUN_GATE_FROM} a new --cmd service from an agent is an open-path App, which loosens its wall: \
             each new service waits for the owner's in-person approval (k2 app approve <id> from a terminal outside K2, or \
             Settings → Apps → Approvals)"
        ),
        switch_off: false,
    })
}

/// Run a publish request the owner approved (a held `publish_run`): on
/// the open path when this box has it (K5), else as in 0.46.0.
pub(crate) fn run_approved(body: &serde_json::Value) -> Result<serde_json::Value, crate::apps::routes::Refusal> {
    let g = |k: &str| body.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    if cmd_route(&g("projectId"), &g("name")) == CmdRoute::Open {
        let run = crate::apps::open::Run {
            project_id: g("projectId"),
            name: g("name"),
            cmd: g("cmd"),
            cwd: PathBuf::from(g("cwd")),
            port: body.get("port").and_then(|v| v.as_u64()).and_then(|p| u16::try_from(p).ok()).unwrap_or(0),
            no_tunnel: body.get("noTunnel").and_then(|v| v.as_bool()).unwrap_or(false),
        };
        // The owner approved it in person: the run is theirs.
        return crate::apps::open::publish_run(&crate::compute_routes::Who::Owner, &run);
    }
    let probe_timeout = match body.get("probeTimeout").and_then(|v| v.as_str()) {
        None => ProbeTimeout::Keep,
        Some(v) if v.trim().eq_ignore_ascii_case("default") => ProbeTimeout::Default,
        Some(v) => k2_core::published_services::parse_probe_timeout(v)
            .map(ProbeTimeout::Ms)
            .map_err(|e| crate::apps::routes::no("publish_failed", 409, e))?,
    };
    let spec = RunSpec {
        project_id: g("projectId"),
        name: g("name"),
        cmd: g("cmd"),
        cwd: PathBuf::from(g("cwd")),
        port: body.get("port").and_then(|v| v.as_u64()).unwrap_or(0) as u16,
        no_tunnel: body.get("noTunnel").and_then(|v| v.as_bool()).unwrap_or(false),
        replace_spec: true,
        skin: false,
        skin_root: String::new(),
        cwd_explicit: body.get("cwdExplicit").and_then(|v| v.as_bool()).unwrap_or(false),
        transfer: false,
        probe_timeout,
    };
    publish_runtime::run(spec)
        .map(|svc| serde_json::to_value(svc).unwrap_or(serde_json::Value::Null))
        .map_err(|e| crate::apps::routes::no("publish_failed", 409, e.message))
}

/// POST dispatcher.
pub fn dispatch_post(path: &str, params: &HashMap<String, String>) -> CliResponse {
    match path {
        "/cli/publish/run" => handle_run(params),
        "/cli/publish/start" => handle_start(params),
        "/cli/publish/stop" => handle_stop(params),
        "/cli/publish/rm" => handle_rm(params),
        "/cli/publish/subdomain/claim" => handle_subdomain_claim(params),
        "/cli/publish/subdomain/unclaim" => handle_subdomain_unclaim(params),
        "/cli/publish/frame" => handle_frame(params),
        "/cli/publish/client-ip" => handle_client_ip_set(params),
        _ => CliResponse::not_found(),
    }
}

/// `GET /cli/publish/client-ip/list`: the nested labels that opted in to K2's
/// client-IP rewrite (0.46.3). Every other nested label is a raw splice.
fn handle_client_ip_list() -> CliResponse {
    let labels: Vec<String> = k2_core::tunnel::client_ip_labels::load().into_iter().collect();
    CliResponse::ok_json(serde_json::json!({ "labels": labels }).to_string())
}

/// Does this box know a nested service called `label` (lowercase)? The
/// control plane's routing map (tunnel cache), the 0074 label stamps, the
/// published services and the Apps (their id is their label). The cache
/// alone isn't enough: it is empty until the first refresh and on an
/// air-gapped box.
fn nested_label_known(label: &str) -> bool {
    if k2_core::tunnel::subdomains::current().targets.contains_key(label) {
        return true;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if SubdomainWorkspace::map(&conn).is_ok_and(|m| m.contains_key(label)) {
        return true;
    }
    if k2_core::published_services::list_all(&conn)
        .is_ok_and(|rows| rows.iter().any(|r| r.name.eq_ignore_ascii_case(label)))
    {
        return true;
    }
    k2_core::deploy::store::get_app(&conn, label).is_ok_and(|a| a.is_some())
}

/// `POST /cli/publish/client-ip` `{label, on}`: opt a nested service in or
/// out of the client-IP rewrite. `on` = its HTTP/1.1 requests get
/// `X-K2-Client-IP` (client copies of the forwarding headers dropped;
/// non-HTTP/1.x answered 400). `off` = the raw splice (the default).
/// Applies to the next connection. Apps use their manifest
/// (`host.clientIp`), not this list.
fn handle_client_ip_set(params: &HashMap<String, String>) -> CliResponse {
    let label = str_param(params, "label");
    let on = match opt_param(params, "on").as_deref().map(str::trim) {
        Some("true" | "1" | "on") => true,
        Some("false" | "0" | "off") => false,
        _ => return CliResponse::bad_request("pass on=true|false (k2 publish client-ip <label> on|off)"),
    };
    let norm = match k2_core::tunnel::client_ip_labels::normalize_label(&label) {
        Ok(l) => l,
        Err(e) => return CliResponse::bad_request(e),
    };
    // Smoke r6: an unknown label used to be stored and reported "on".
    // `off` still clears a listed label whose service is gone.
    let listed = k2_core::tunnel::client_ip_labels::load().contains(&norm);
    if !(nested_label_known(&norm) || (!on && listed)) {
        return CliResponse {
            status: "404 Not Found",
            content_type: "application/json",
            body: serde_json::json!({
                "error": format!(
                    "no nested service {norm:?} on this box (k2 publish subdomain list, k2 app list): nothing changed"
                ),
                "code": "label_not_found",
            })
            .to_string(),
        };
    }
    match k2_core::tunnel::client_ip_labels::set(&label, on) {
        Ok(labels) => CliResponse::ok_json(
            serde_json::json!({
                "label": label.trim().to_ascii_lowercase(),
                "clientIp": on,
                "labels": labels.into_iter().collect::<Vec<_>>(),
            })
            .to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

fn forbidden_sticker() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({ "error": FOREIGN_STICKER }).to_string(),
    }
}

/// Same rules as renderer `nestedPublicUrl`: https public_url host prefix,
/// else `https://<label>.<primary>.k2.dev`, else null. Never fabricates.
fn nested_public_url(label: &str, primary: &str, public_url: Option<&str>) -> Option<String> {
    let clean_label = label.trim();
    if clean_label.is_empty() {
        return None;
    }
    let url = public_url.unwrap_or("").trim().trim_end_matches('/');
    if let Some(host) = url.strip_prefix("https://") {
        if !host.is_empty() {
            return Some(format!("https://{clean_label}.{host}"));
        }
    }
    let clean_primary = primary.trim();
    if !clean_primary.is_empty() {
        return Some(format!(
            "https://{clean_label}.{clean_primary}.{SUBDOMAIN_HOST}"
        ));
    }
    None
}

/// GET /cli/publish/leftovers?project= — this workspace's 0074 labels
/// unioned with the in-memory tunnel cache. Never calls Connect /
/// `refresh_once`. Empty cache still returns stickers.
fn handle_leftovers(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    let stored = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match SubdomainWorkspace::list_for_project(&conn, &project_id) {
            Ok(rows) => rows,
            Err(e) => return CliResponse::internal_error(e.to_string()),
        }
    };
    let cache = k2_core::tunnel::subdomains::current();
    let public_url = k2_core::tunnel::tunnel_status().public_url;
    let leftovers: Vec<serde_json::Value> = stored
        .into_iter()
        .map(|(label, stored_target)| {
            let target = match cache.targets.get(&label) {
                Some(t) if !t.is_empty() => t.clone(),
                _ => stored_target,
            };
            let url = nested_public_url(&label, &cache.primary, public_url.as_deref());
            serde_json::json!({
                "label": label,
                "target": target,
                "url": url,
            })
        })
        .collect();
    CliResponse::ok_json(serde_json::json!({ "leftovers": leftovers }).to_string())
}

/// POST /cli/publish/subdomain/claim — self-only 0074 stamp. INSERT if
/// unattributed or already this workspace; a label another workspace
/// holds → 409 `label_owned_by_other_workspace`, no rewrite.
///
/// - `check=1`: decide only, write nothing. The CLI asks this BEFORE any
///   control-plane create/point/rm so a foreign label is never touched.
/// - `transfer=1`: owner/admin only — move the label to this workspace
///   (audited). Agents and members get 403 `transfer_owner_only`.
/// - `check=1&release=1`: the same ownership answer for a remove; skips the
///   `meet`/`chat` reservation (nothing is claimed). `skin` stays refused.
fn handle_subdomain_claim(params: &HashMap<String, String>) -> CliResponse {
    let label = str_param(params, "label");
    if label.trim().is_empty() {
        return CliResponse::bad_request("Missing label");
    }
    // `skin` first so its 400 is unchanged; `meet`/`chat` after the
    // project resolves (an old same-workspace row may re-stamp).
    if k2_core::skin::is_reserved_nested_label(&label)
        && !k2_core::skin::is_grandfathered_reserved_label(&label)
    {
        return CliResponse::bad_request(k2_core::skin::reserved_nested_label_error(&label));
    }
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    // `check=1&release=1` is the ownership question before a REMOVE
    // (`k2 publish subdomain rm`): it writes nothing, so an old `meet` /
    // `chat` label can still be deleted by the workspace that may own it.
    let release_check = boolish(params, &["check"]) && boolish(params, &["release"]);
    if !release_check {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(resp) = label_owner::refuse_reserved_claim(&conn, &label, &project_id) {
            return resp;
        }
    }
    let target = opt_param(params, "target");
    let transfer = boolish(params, &["transfer"]);
    let label_lc = label.trim().to_ascii_lowercase();
    // audit-app-agent-access-0464 fix 3: an agent never claims a hostname
    // that already serves something on this box while no workspace holds
    // it and nothing of its own names it: that is a held `label_transfer`
    // with a card for the owner (`apps::agent_grants`).
    if !release_check && !transfer && agent_claim_needs_owner(&label_lc, &project_id) {
        if boolish(params, &["check"]) {
            return CliResponse {
                status: "409 Conflict",
                content_type: "application/json",
                body: serde_json::json!({
                    "code": "label_needs_owner",
                    "error": format!("label_needs_owner: '{label_lc}' already serves something on this box and no workspace holds it."),
                    "hint": format!("k2 publish subdomain claim {label_lc} files a request for the owner (a card on your Thread)."),
                    "label": label_lc,
                })
                .to_string(),
            };
        }
        let who = crate::apps::routes::Who::Agent {
            project_id: project_id.clone(),
            session_id: String::new(),
        };
        return crate::apps::routes::respond(crate::apps::agent_grants::label_transfer_held(
            &who,
            &project_id,
            None,
            &label_lc,
            "claim",
            None,
            opt_param(params, "reason").as_deref(),
        ));
    }
    if boolish(params, &["check"]) {
        return match label_owner::check(&label, &project_id, transfer) {
            Ok(decision) => CliResponse::ok_json(
                serde_json::json!({
                    "success": true,
                    "check": true,
                    "label": label_lc,
                    "projectId": project_id,
                    "transfer": matches!(decision, label_owner::Decision::Transfer { .. }),
                })
                .to_string(),
            ),
            Err(r) => r.into_response(),
        };
    }
    match label_owner::claim_checked(&label, &project_id, target.as_deref(), transfer) {
        Ok(decision) => {
            crate::session_events::emit_tunnel_subdomains_changed();
            CliResponse::ok_json(
                serde_json::json!({
                    "success": true,
                    "label": label_lc,
                    "projectId": project_id,
                    "transferred": matches!(decision, label_owner::Decision::Transfer { .. }),
                })
                .to_string(),
            )
        }
        Err(r) => r.into_response(),
    }
}

/// An agent's claim the owner decides (audit fix 3): the caller is an
/// agent passport, no workspace holds `label`, the hostname already
/// serves something on this box (the control plane's map), and no
/// published service of `project_id` has that name.
fn agent_claim_needs_owner(label: &str, project_id: &str) -> bool {
    if label_owner::current() != label_owner::Caller::Agent {
        return false;
    }
    if !k2_core::tunnel::subdomains::current().targets.contains_key(label) {
        return false;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let held = k2_core::db::schema::SubdomainWorkspace::owner_of(&conn, label).ok().flatten().is_some();
    let ours = k2_core::published_services::get_by_project_name(&conn, project_id, label).ok().flatten().is_some();
    !held && !ours
}

/// POST /cli/publish/subdomain/unclaim — drop THIS workspace's sticker.
/// Foreign 0074 → 403, no rewrite.
fn handle_subdomain_unclaim(params: &HashMap<String, String>) -> CliResponse {
    let label = str_param(params, "label");
    if label.trim().is_empty() {
        return CliResponse::bad_request("Missing label");
    }
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    let outcome = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match SubdomainWorkspace::self_unclaim(&conn, &label, &project_id) {
            Ok(o) => o,
            Err(e) => return CliResponse::bad_request(e.to_string()),
        }
    };
    match outcome {
        SelfUnclaim::Foreign { .. } => forbidden_sticker(),
        SelfUnclaim::Removed => {
            crate::session_events::emit_tunnel_subdomains_changed();
            CliResponse::ok_json(
                serde_json::json!({
                    "success": true,
                    "label": label.trim().to_ascii_lowercase(),
                    "removed": true,
                })
                .to_string(),
            )
        }
        SelfUnclaim::Absent => CliResponse::ok_json(
            serde_json::json!({
                "success": true,
                "label": label.trim().to_ascii_lowercase(),
                "removed": false,
            })
            .to_string(),
        ),
    }
}

fn handle_list(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    match publish_runtime::list(&project_id) {
        Ok(services) => {
            CliResponse::ok_json(serde_json::json!({ "services": services }).to_string())
        }
        Err(e) => err_to_resp(e),
    }
}

fn handle_logs(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let name = str_param(params, "name");
    if name.trim().is_empty() {
        return CliResponse::bad_request("Missing name");
    }
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    let mut n: usize = 200;
    if let Some(lines) = opt_param(params, "lines") {
        if let Ok(v) = lines.parse::<usize>() {
            n = v.max(1);
        }
    }
    if let Some(tail) = opt_param(params, "tail") {
        if let Ok(v) = tail.parse::<usize>() {
            n = v.max(1);
        }
    }
    let text = publish_runtime::read_log_tail(&project_id, &name, n);
    CliResponse::ok_json(
        serde_json::json!({
            "name": name,
            "text": text,
        })
        .to_string(),
    )
}

/// GET `/cli/publish/validate?name=` or `?dir=` (`k2 app validate`,
/// decision-k2-runtime-v1 stage 0): parse every JS/TS file and inline
/// script of an App's folder and run the leak lints. `name` checks a
/// published App's own folder (its `--skin` dir, else its `--cwd`); `dir`
/// a workspace-relative folder (default: the workspace). Read-only.
/// `status` is `ok` or `failed` (`k2 app validate` exits 0 or 1).
fn handle_validate(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    let Some(ws) = resolve_project_path(&project_id) else {
        return CliResponse::bad_request("workspace has no path");
    };
    let name = str_param(params, "name");
    let dir_param = str_param(params, "dir");
    let (dir, shown) = if !name.trim().is_empty() {
        let row = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::published_services::get_by_project_name(&conn, &project_id, &name)
        };
        let row = match row {
            Ok(Some(r)) => r,
            Ok(None) => {
                return CliResponse {
                    status: "404 Not Found",
                    content_type: "application/json",
                    body: serde_json::json!({
                        "error": "not_found",
                        "message": format!("no published App named '{}' in this workspace", name.trim()),
                        "suggestion": "list them with k2 publish ps, or check a folder with k2 app validate --dir <path>",
                    })
                    .to_string(),
                }
            }
            Err(e) => return CliResponse::internal_error(format!("published_services: {e}")),
        };
        let base = PathBuf::from(&row.cwd);
        let dir = if row.kind == k2_core::published_services::KIND_SKIN && row.skin_root.trim().is_empty() {
            return CliResponse::bad_request(format!(
                "'{}' serves K2's bundled app pages, not code of its own; nothing to validate",
                row.name
            ));
        } else if row.kind == k2_core::published_services::KIND_SKIN {
            base.join(row.skin_root.trim())
        } else {
            base
        };
        (dir, row.name.clone())
    } else {
        let rel = if dir_param.trim().is_empty() { "." } else { dir_param.trim() };
        let checked = if rel == "." {
            Ok(String::new())
        } else {
            publish_runtime::validate_skin_root(std::path::Path::new(&ws), rel)
        };
        match checked {
            Ok(r) => (PathBuf::from(&ws).join(&r), if r.is_empty() { ".".to_string() } else { r }),
            Err(e) => return CliResponse::bad_request(e.replace("skinRoot", "dir")),
        }
    };
    if !dir.is_dir() {
        return CliResponse::bad_request(format!("{} is not a folder", dir.display()));
    }
    let r = k2_core::jscheck::app::check_dir(&dir);
    let ok = r.errors.is_empty();
    CliResponse::ok_json(
        serde_json::json!({
            "ok": ok,
            "status": if ok { "ok" } else { "failed" },
            "target": shown,
            "dir": dir.display().to_string(),
            "files": r.files,
            "errors": r.errors,
            "warnings": r.warnings,
            "skipped": r.skipped,
        })
        .to_string(),
    )
}

fn handle_run(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    let name = str_param(params, "name");
    if name.trim().is_empty() {
        return CliResponse::bad_request("Missing name");
    }
    let cmd = str_param(params, "cmd");
    let skin = boolish(params, &["skin"]);
    let skin_root = opt_param(params, "skinRoot")
        .or_else(|| opt_param(params, "skin_root"))
        .unwrap_or_default();
    let cwd_explicit = opt_param(params, "cwd").is_some();
    if skin && cwd_explicit {
        return CliResponse::bad_request("--cwd and --skin are mutually exclusive");
    }
    if skin && !cmd.trim().is_empty() {
        return CliResponse::bad_request("--cmd and --skin are mutually exclusive");
    }
    let port = match parse_port(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let no_tunnel = boolish(params, &["noTunnel", "no_tunnel"]);
    let probe_timeout = match parse_probe_timeout_param(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let cwd = opt_param(params, "cwd")
        .map(PathBuf::from)
        .or_else(|| resolve_project_path(&project_id).map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(&project));
    // K5 (§8.2): a NEW --cmd label, or a re-run of an open-path folder App,
    // runs on the open path when this box has it; an existing --cmd
    // service is untouched in 0.46.1.
    let route = if skin { CmdRoute::Legacy } else { cmd_route(&project_id, &name) };
    // S13 / §5.11.5: an agent's open-path run needs both switches, or it
    // is held for the owner. Where the open path isn't ready (macOS, an
    // unprepared box) the run stays as in 0.46.0, with the notice.
    let gate = publish_run_gate(&project_id, &name, skin);
    let who = CALLER.with(|c| c.borrow().clone()).unwrap_or(crate::compute_routes::Who::Invalid);
    let hold_run = |reasons: Vec<String>| {
        let body = serde_json::json!({
            "projectId": project_id, "name": name, "cmd": cmd, "cwd": cwd.display().to_string(),
            "port": port, "noTunnel": no_tunnel, "cwdExplicit": cwd_explicit,
            "probeTimeout": probe_timeout_body(probe_timeout),
        });
        let diff = serde_json::json!([format!("new open-path App {name}: {cmd} (port {port})")]);
        crate::apps::routes::respond(crate::apps::grace::hold(
            &who,
            k2_core::app_grace::requests::KIND_PUBLISH_RUN,
            None,
            &name,
            &project_id,
            body,
            diff,
            reasons,
        ))
    };
    // C4 (disk guard, prd-app-resources-disk-guard-v1 §6.1): a NEW --cmd
    // service, or an open-path run that takes a new snapshot, before the
    // gate (an agent without --disk-override gets disk_low instead of a
    // request the owner would approve into a full disk). A re-run of an
    // existing legacy service is never checked.
    let disk = match publish_disk_check(&who, &project_id, &name, &cwd, &route, skin, opt_param(params, "diskOverride").as_deref()) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let disk_extra = match disk {
        DiskStep::Go(x) => x,
        DiskStep::Hold(reasons) => {
            let mut all: Vec<String> = gate.as_ref().map(|g| vec![g.why.clone()]).unwrap_or_default();
            all.extend(reasons);
            return hold_run(all);
        }
    };
    if let Some(g) = gate.as_ref() {
        let why = g.why.as_str();
        if gate_holds(gate_mode(), &route, g.switch_off) {
            let mut reasons = vec![why.to_string()];
            if let Some(o) = disk_extra.get("diskOverride") {
                reasons.push(format!("past the disk guard: {}", o["reason"].as_str().unwrap_or("")));
            }
            return hold_run(reasons);
        }
    }
    if matches!(route, CmdRoute::Open) {
        let run = crate::apps::open::Run { project_id, name, cmd, cwd, port, no_tunnel };
        return crate::apps::routes::respond(crate::apps::open::publish_run(&who, &run).map(|mut v| {
            crate::apps::resources::decorate(&mut v, disk_extra);
            v
        }));
    }
    let not_ready = match &route {
        CmdRoute::NotReady(why) => Some(why.clone()),
        _ => None,
    };
    match publish_runtime::run(RunSpec {
        project_id,
        name,
        cmd,
        cwd,
        port,
        no_tunnel,
        replace_spec: true,
        skin,
        skin_root,
        cwd_explicit,
        transfer: boolish(params, &["transfer"]),
        probe_timeout,
    }) {
        Ok(svc) => {
            let mut v = serde_json::to_value(&svc).unwrap_or(serde_json::Value::Null);
            if let Some(o) = v.as_object_mut() {
                if let Some(g) = gate {
                    // Where the open path isn't ready (or the owner set
                    // K2_OPEN_APPS_GATE=warn) the run goes through, with
                    // the notice of what the gate would do.
                    let name = o.get("name").and_then(|n| n.as_str()).unwrap_or("this service").to_string();
                    o.insert("crossover".into(), crossover_notice(&name, &g.why));
                }
                if let serde_json::Value::Object(m) = disk_extra {
                    o.extend(m);
                }
                if let Some(why) = not_ready {
                    o.insert(
                        "wall".into(),
                        serde_json::json!({
                            "path": "legacy",
                            "label": "Unwalled (the open path isn't ready on this box)",
                            "why": why,
                        }),
                    );
                }
            }
            CliResponse::ok_json(v.to_string())
        }
        Err(e) => err_to_resp(e),
    }
}

/// What the disk guard decided for a `k2 publish run`.
pub(crate) enum DiskStep {
    /// Go ahead; merge these fields into the answer.
    Go(serde_json::Value),
    /// An override that waits for the owner: file the `publish_run` hold
    /// with these reasons.
    Hold(Vec<String>),
}

/// C4: the disk check for a run that creates a service or a snapshot.
pub(crate) fn publish_disk_check(
    who: &crate::compute_routes::Who,
    project_id: &str,
    name: &str,
    cwd: &std::path::Path,
    route: &CmdRoute,
    skin: bool,
    disk_override: Option<&str>,
) -> Result<DiskStep, CliResponse> {
    use crate::apps::resources as res;
    if skin {
        return Ok(DiskStep::Go(serde_json::json!({})));
    }
    let need = match route {
        // An open-path run snapshots the whole folder, stored twice.
        CmdRoute::Open => crate::apps::open::folder_app(project_id, name)
            .and_then(|a| res::seed_size(a.live_seed.as_deref()))
            .unwrap_or(0)
            .saturating_mul(2),
        _ => {
            let exists = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::published_services::get_by_project_name(&conn, project_id, name).ok().flatten().is_some()
            };
            if exists {
                // A re-run of an existing legacy service: never checked.
                return Ok(DiskStep::Go(serde_json::json!({})));
            }
            0
        }
    };
    let paths = vec![cwd.to_path_buf(), k2_core::paths::k2_home()];
    match res::decide(who, "publish run", name, &paths, need, disk_override, None, false) {
        Ok(res::Decision::Go(x)) => Ok(DiskStep::Go(x)),
        Ok(res::Decision::Refuse(r)) => Err(crate::apps::routes::refusal_response(&r)),
        Ok(res::Decision::Hold { finding, reason }) => Ok(DiskStep::Hold(res::hold_reasons(&finding, "publish run", &reason))),
        Err(r) => Err(crate::apps::routes::refusal_response(&r)),
    }
}

/// `k2 publish stop|start|rm` on an open-path folder App: the App's own
/// routes (§8.2), with the caller's rights there. `None` = not one.
fn open_path_verb(verb: &str, project_id: &str, name: &str, keep_hostname: bool) -> Option<CliResponse> {
    let app = crate::apps::open::folder_app(project_id, name)?;
    let who = CALLER.with(|c| c.borrow().clone()).unwrap_or(crate::compute_routes::Who::Invalid);
    Some(crate::apps::routes::respond(crate::apps::open::publish_verb(&who, verb, &app, keep_hostname)))
}

/// Where a `--cmd` run goes (K5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CmdRoute {
    /// An existing `--cmd` service: unchanged in 0.46.1.
    Legacy,
    /// A new label (or an open-path folder App's re-run) on a box with the
    /// open path.
    Open,
    /// A new label where the open path isn't ready: 0.46.0's behaviour.
    NotReady(String),
}

fn cmd_route(project_id: &str, name: &str) -> CmdRoute {
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::get_by_project_name(&conn, project_id, name).ok().flatten()
    };
    let open_app = crate::apps::open::folder_app(project_id, name).is_some();
    route_for(row.map(|r| r.cmd != k2_core::published_services::CMD_APP_SENTINEL), open_app, crate::apps::open::ready())
}

/// The pure part of [`cmd_route`]: `legacy_row` = this workspace has a row
/// under the name (`Some(true)` a `--cmd` service, `Some(false)` an App's).
fn route_for(legacy_row: Option<bool>, open_app: bool, ready: Result<(), String>) -> CmdRoute {
    match (legacy_row, open_app) {
        (Some(true), _) => CmdRoute::Legacy,
        (_, true) => match ready {
            Ok(()) => CmdRoute::Open,
            Err(why) => CmdRoute::NotReady(why),
        },
        // Another kind of App under the name: publish_runtime refuses it
        // (`is_an_app`), as in 0.46.0.
        (Some(false), false) => CmdRoute::Legacy,
        (None, false) => match ready {
            Ok(()) => CmdRoute::Open,
            Err(why) => CmdRoute::NotReady(why),
        },
    }
}

fn handle_start(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let name = str_param(params, "name");
    if name.trim().is_empty() {
        return CliResponse::bad_request("Missing name");
    }
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    if let Some(resp) = open_path_verb("start", &project_id, &name, false) {
        return resp;
    }
    match publish_runtime::start(&project_id, &name) {
        Ok(svc) => {
            CliResponse::ok_json(serde_json::to_string(&svc).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => err_to_resp(e),
    }
}

fn handle_stop(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let name = str_param(params, "name");
    if name.trim().is_empty() {
        return CliResponse::bad_request("Missing name");
    }
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    if let Some(resp) = open_path_verb("stop", &project_id, &name, false) {
        return resp;
    }
    match publish_runtime::stop(&project_id, &name) {
        Ok(svc) => {
            CliResponse::ok_json(serde_json::to_string(&svc).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => err_to_resp(e),
    }
}

fn handle_rm(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let name = str_param(params, "name");
    if name.trim().is_empty() {
        return CliResponse::bad_request("Missing name");
    }
    let keep = boolish(params, &["keepHostname", "keep_hostname"]);
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    if let Some(resp) = open_path_verb("rm", &project_id, &name, keep) {
        return resp;
    }
    match publish_runtime::rm(&project_id, &name, keep) {
        Ok(()) => CliResponse::ok_json(serde_json::json!({ "ok": true, "name": name }).to_string()),
        Err(e) => err_to_resp(e),
    }
}

fn frame_error(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint } })
            .to_string(),
    }
}

/// POST /cli/publish/frame — owner only (FA3). Exactly one of
/// `allow` (space-separated origins, added to the default), `none: true`
/// (refuse every frame) or `reset: true` (back to the default). Restarts
/// the App's helper when it is running so the new headers apply at once.
///
/// The route-policy floor (Owner) already refuses Admin/Member logins with
/// `role_required`. An agent passport (TCP hook or cell socket) reaches
/// here stamped and gets `owner_only`: widening who may frame an App is
/// the owner's call, never an agent's.
fn handle_frame(params: &HashMap<String, String>) -> CliResponse {
    if crate::caller_workspace::request_principal().is_some()
        || params.contains_key(crate::caller_workspace::PRINCIPAL_BOUND_KEY)
    {
        return frame_error(
            "403 Forbidden",
            "owner_only",
            "k2 publish frame is owner-only: it decides which other sites may frame this App. \
             Ask your human to run it from a shell outside K2 (or the owner's terminal).",
        );
    }
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let name = str_param(params, "name");
    if name.trim().is_empty() {
        return CliResponse::bad_request("Missing name");
    }
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    let allow = opt_param(params, "allow");
    let none = boolish(params, &["none"]);
    let reset = boolish(params, &["reset"]);
    let picked = [allow.is_some(), none, reset].iter().filter(|b| **b).count();
    if picked != 1 {
        return frame_error(
            "400 Bad Request",
            "usage",
            "pass exactly one of allow, none or reset (k2 publish frame <name> --allow <origin> | --none | --reset)",
        );
    }
    let change = if none {
        publish_runtime::FrameChange::None
    } else if reset {
        publish_runtime::FrameChange::Reset
    } else {
        let list: Vec<String> = allow
            .unwrap_or_default()
            .split_ascii_whitespace()
            .map(str::to_string)
            .collect();
        if list.is_empty() {
            return frame_error("400 Bad Request", "usage", "--allow needs an origin, e.g. https://partner.example");
        }
        publish_runtime::FrameChange::Allow(list)
    };
    match publish_runtime::set_frame(&project_id, &name, change) {
        Ok((svc, restarted)) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "service": svc,
                "restarted": restarted,
            })
            .to_string(),
        ),
        Err(publish_runtime::FrameSetError::Policy(e)) => {
            frame_error("400 Bad Request", e.code(), &e.to_string())
        }
        Err(publish_runtime::FrameSetError::Publish(e)) => err_to_resp(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0.46.1 (lane 1, K5): the gate holds by default; only the owner's
    /// own daemon environment can put back 0.46.0's warn-only answer.
    #[test]
    fn the_publish_run_gate_holds_unless_the_owner_set_warn() {
        let open = CmdRoute::Open;
        let not = CmdRoute::NotReady("no engine".into());
        assert_eq!(gate_mode_by(None), GateMode::OpenPath, "0.46.1's default");
        assert_eq!(gate_mode_by(Some("")), GateMode::OpenPath);
        assert_eq!(gate_mode_by(Some(" warn ")), GateMode::Warn);
        assert_eq!(gate_mode_by(Some("enforce")), GateMode::Always);
        assert!(gate_holds(GateMode::OpenPath, &open, false), "held where the run would be an open-path App");
        assert!(!gate_holds(GateMode::OpenPath, &not, false), "not held where the box has no open path (macOS, unprepared)");
        assert!(!gate_holds(GateMode::OpenPath, &CmdRoute::Legacy, false));
        assert!(gate_holds(GateMode::OpenPath, &CmdRoute::Legacy, true), "a legacy re-run without switch 1 is held anywhere");
        assert!(!gate_holds(GateMode::Warn, &open, false));
        assert!(!gate_holds(GateMode::Warn, &CmdRoute::Legacy, true), "the owner's warn mode never holds");
        assert!(gate_holds(GateMode::Always, &not, false));
        assert_eq!(PUBLISH_RUN_GATE_FROM, "0.46.1");
        let n = crossover_notice("shop", "from K2 0.46.1 a new --cmd service from an agent is an open-path App: …");
        assert_eq!((n["mode"].as_str(), n["held"].as_bool(), n["wouldHold"].as_bool()), (Some("warn"), Some(false), Some(true)));
        assert!(n["message"].as_str().unwrap().starts_with("shop ran unwalled, without the open-path gate"), "{n}");
    }

    /// N1 (prd-agent-app-permissions-v1, Rosson 2026-10-20): an agent's
    /// new `--cmd` service waits for the person at every switch level (no
    /// standing approval); a re-run of an existing legacy service is free
    /// under switch 1 and gated without it.
    #[test]
    fn a_new_agent_cmd_service_always_waits_and_legacy_reruns_need_switch_1() {
        use crate::apps::access::Level;
        for level in [Level::Off, Level::Build, Level::Full] {
            let g = gate_for(false, level).unwrap_or_else(|| panic!("{level:?}: a new service must be gated"));
            assert!(!g.switch_off, "{level:?}");
            assert!(g.why.contains("in-person approval"), "{}", g.why);
            assert!(!g.why.contains("publish open Apps"), "the retired switch is never offered: {}", g.why);
        }
        assert_eq!(gate_for(true, Level::Build), None, "a legacy re-run is free under switch 1");
        assert_eq!(gate_for(true, Level::Full), None);
        let g = gate_for(true, Level::Off).expect("gated without switch 1");
        assert!(g.switch_off && g.why.contains("Build and run Apps"), "{g:?}");
    }

    /// K5: where a `--cmd` run goes. An existing `--cmd` service stays as
    /// it is in 0.46.1 (NSI's Jobs keeps working untouched); a new label
    /// goes on the open path only where the box has it.
    #[test]
    fn cmd_runs_route_to_the_open_path_only_for_new_labels_on_a_ready_box() {
        let ready = || Ok(());
        let not = || Err("the App runtime is off on this box".to_string());
        assert_eq!(route_for(Some(true), false, ready()), CmdRoute::Legacy, "an existing --cmd service: untouched");
        assert_eq!(route_for(Some(true), false, not()), CmdRoute::Legacy);
        assert_eq!(route_for(None, false, ready()), CmdRoute::Open, "a new label on a ready box");
        assert_eq!(route_for(None, false, not()), CmdRoute::NotReady("the App runtime is off on this box".into()));
        assert_eq!(route_for(Some(false), true, ready()), CmdRoute::Open, "a re-run of an open-path folder App redeploys it");
        assert!(matches!(route_for(Some(false), true, not()), CmdRoute::NotReady(_)));
        assert_eq!(route_for(Some(false), false, ready()), CmdRoute::Legacy, "another kind of App: publish refuses it as before");
    }

    #[test]
    fn get_chain_405s_mutating_publish_routes() {
        let p = HashMap::new();
        for path in [
            "/cli/publish/run",
            "/cli/publish/start",
            "/cli/publish/stop",
            "/cli/publish/rm",
            "/cli/publish/subdomain/claim",
            "/cli/publish/subdomain/unclaim",
            "/cli/publish/frame",
        ] {
            let resp = dispatch(path, &p).expect("GET twin must exist");
            assert_eq!(resp.status, "405 Method Not Allowed", "path={path}");
        }
    }

    /// FA3: an agent passport (stamped params) never changes who may
    /// frame an App, even its own workspace's. Checked before any lookup.
    #[test]
    fn frame_refuses_agent_passports_with_owner_only() {
        let mut p = HashMap::new();
        p.insert(crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string());
        p.insert("name".to_string(), "agents".to_string());
        p.insert("project".to_string(), "/tmp/k2-frame-no-such-ws".to_string());
        p.insert("allow".to_string(), "https://partner.example".to_string());
        let r = dispatch_post("/cli/publish/frame", &p);
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "owner_only", "{}", r.body);
        assert!(v["error"]["hint"].as_str().expect("hint").contains("owner"), "{}", r.body);
    }

    #[test]
    fn get_list_requires_project() {
        let resp = dispatch("/cli/publish/list", &HashMap::new()).unwrap();
        assert_eq!(resp.status, "400 Bad Request");
    }

    /// 0.45.4: the list route says which services still inherit the
    /// daemon's environment (`envMode: legacy`, ends in 0.46.0) and which
    /// run clean, so `k2 publish ps` can warn agents.
    #[test]
    fn list_reports_env_mode_and_the_046_end_for_legacy_services() {
        crate::test_support::with_temp_home(|| {
            let ws = format!("envmode-ws-{}", uuid::Uuid::new_v4().simple());
            let path = format!("/tmp/{ws}");
            seed_ws(&ws, &path, "EnvMode");
            let (old, new) = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let old = k2_core::published_services::insert(&conn, &ws, "oldshop", "true", &path, 41001, "local", "stopped", "cmd", "")
                    .expect("insert legacy");
                let new = k2_core::published_services::insert(&conn, &ws, "newshop", "true", &path, 41002, "local", "stopped", "cmd", "")
                    .expect("insert clean");
                (old.id, new.id)
            };
            crate::vault::publish_env::set(&old, Some("legacy"), None).expect("legacy");
            crate::vault::publish_env::set(&new, Some("clean"), None).expect("clean");

            let mut p = HashMap::new();
            p.insert("project".to_string(), path.clone());
            let resp = dispatch("/cli/publish/list", &p).expect("list route");
            assert_eq!(resp.status, "200 OK", "{}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
            let rows = v["services"].as_array().expect("services[]");
            let row = |n: &str| rows.iter().find(|r| r["name"] == n).unwrap_or_else(|| panic!("{n} missing: {v}")).clone();
            let o = row("oldshop");
            assert_eq!(o["envMode"], "legacy", "{o}");
            assert!(o["envRemovedIn"].is_null(), "no end is set (Rosson 2026-10-26): {o}");
            assert!(o["envDeprecation"].as_str().expect("notice").contains("k2 publish env oldshop clean"), "{o}");
            let n = row("newshop");
            assert_eq!(n["envMode"], "clean", "{n}");
            assert!(n["envRemovedIn"].is_null(), "{n}");
            assert!(n["envDeprecation"].is_null(), "{n}");

            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::published_services::delete(&conn, &ws, "oldshop").expect("cleanup");
            k2_core::published_services::delete(&conn, &ws, "newshop").expect("cleanup");
        });
    }

    #[test]
    fn unknown_publish_path_is_404() {
        let resp = dispatch("/cli/publish/nope", &HashMap::new()).unwrap();
        assert_eq!(resp.status, "404 Not Found");
    }

    /// The subdomain-cache lock, taken AFTER a temp `$HOME` (env lock
    /// first, the module lock second; see `k2_core::test_env::serial`).
    /// The leftovers route reads `tunnel_status()`, which resolves the
    /// private runtime dir; the `TempHome` points `K2_RUNTIME_DIR` under
    /// itself so the test never resolves the REAL `/run/user/<uid>/k2`
    /// (refused by `k2_core::runtime_dir` when this process looks like a
    /// test, which depends on what sibling tests are doing).
    fn cache_lock() -> (std::sync::MutexGuard<'static, ()>, crate::test_support::TempHome) {
        static M: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let home = crate::test_support::TempHome::new();
        (M.lock().unwrap_or_else(|e| e.into_inner()), home)
    }

    fn seed_ws(id: &str, path: &str, name: &str) {
        let _ = k2_core::db::init_for_tests();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, path, name],
        )
        .expect("seed workspace");
    }

    /// `k2 app validate --dir`: parse errors fail it with file:line and a
    /// suggestion; leak lints are warnings; `..` and absolute dirs are refused.
    #[test]
    fn validate_checks_a_workspace_folder_and_refuses_escapes() {
        let pid = std::process::id();
        let ws = std::env::temp_dir().join(format!("k2-validate-ws-{pid}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(ws.join("app/src")).expect("mkdir");
        std::fs::write(ws.join("app/server.js"), "const seen = new Map()\nserver.on('request', (req) => { seen.set(req.url, 1) })\n")
            .expect("write server.js");
        std::fs::write(ws.join("app/src/page.ts"), "export const n: number = \n").expect("write page.ts");
        let ws_path = ws.canonicalize().expect("canon").display().to_string();
        let id = format!("validate-ws-{pid}");
        seed_ws(&id, &ws_path, "Validate");
        let call = |dir: &str| {
            let mut p = HashMap::new();
            p.insert("project".to_string(), ws_path.clone());
            p.insert("dir".to_string(), dir.to_string());
            dispatch("/cli/publish/validate", &p).expect("GET row")
        };
        let r = call("app");
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["status"], "failed", "{v}");
        assert_eq!(v["files"], 2, "{v}");
        assert_eq!(v["errors"][0]["file"], "src/page.ts", "{v}");
        assert_eq!(v["errors"][0]["line"], 2, "unexpected end of input: {v}");
        assert!(v["errors"][0]["suggestion"].as_str().is_some_and(|s| !s.is_empty()), "{v}");
        assert_eq!(v["warnings"][0]["file"], "server.js", "{v}");
        assert_eq!(v["warnings"][0]["line"], 2, "{v}");
        for bad in ["../", "/etc", "app/../.."] {
            let r = call(bad);
            assert_eq!(r.status, "400 Bad Request", "{bad}: {}", r.body);
        }
        let _ = std::fs::remove_dir_all(&ws);
    }

    fn leftovers_json(project: &str) -> serde_json::Value {
        let mut p = HashMap::new();
        p.insert("project".into(), project.into());
        let resp = dispatch("/cli/publish/leftovers", &p).unwrap();
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        serde_json::from_str(&resp.body).expect("leftovers json")
    }

    #[test]
    fn leftovers_get_returns_0074_when_cache_empty_and_is_workspace_scoped() {
        let _g = cache_lock();
        let pid = std::process::id();
        let docs = format!("docs-left-{pid}");
        let sales = format!("sales-left-{pid}");
        let portal = format!("portal-left-{pid}");
        seed_ws(&docs, "/tmp/docs-left", "Documents");
        seed_ws(&sales, "/tmp/sales-left", "Sales");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            SubdomainWorkspace::self_stamp(&conn, &portal, &docs, None).unwrap();
        }
        k2_core::tunnel::subdomains::store(Default::default());

        let docs_body = leftovers_json(&docs);
        let rows = docs_body["leftovers"].as_array().expect("leftovers[]");
        assert!(
            rows.iter()
                .any(|r| r["label"] == portal && r["target"] == ""),
            "Documents leftover must list {portal} with unknown target; body={docs_body}"
        );
        assert!(docs_body["leftovers"][0].get("url").is_some());

        let sales_body = leftovers_json(&sales);
        let sales_rows = sales_body["leftovers"].as_array().expect("leftovers[]");
        assert!(
            !sales_rows.iter().any(|r| r["label"] == portal),
            "Sales must not inherit Documents leftover; body={sales_body}"
        );

        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = SubdomainWorkspace::unclaim(&conn, &portal);
        }
    }

    #[test]
    fn leftovers_get_overlays_cache_target_and_url_when_primary_known() {
        let _g = cache_lock();
        let pid = std::process::id();
        let docs = format!("docs-ovl-{pid}");
        let portal = format!("portal-ovl-{pid}");
        let primary = format!("prim-ovl-{pid}");
        seed_ws(&docs, "/tmp/docs-ovl", "Documents");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            SubdomainWorkspace::self_stamp(&conn, &portal, &docs, None).unwrap();
        }
        let mut targets = std::collections::HashMap::new();
        targets.insert(portal.clone(), "localhost:3000".to_string());
        k2_core::tunnel::subdomains::store(k2_core::tunnel::subdomains::SubdomainMap {
            primary: primary.clone(),
            targets,
        });

        let body = leftovers_json(&docs);
        let row = body["leftovers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["label"] == portal)
            .expect("portal row");
        assert_eq!(row["target"], "localhost:3000");
        let url = row["url"].as_str().expect("url when primary known");
        assert!(
            url.contains(&portal),
            "public URL must include the leftover label; url={url} primary={primary}"
        );
        assert!(
            url.starts_with("https://"),
            "public URL must be https; url={url}"
        );

        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = SubdomainWorkspace::unclaim(&conn, &portal);
        }
        k2_core::tunnel::subdomains::store(Default::default());
    }

    #[test]
    fn leftovers_get_keeps_last_target_when_cache_empty() {
        let _g = cache_lock();
        let pid = std::process::id();
        let docs = format!("docs-lt-{pid}");
        let portal = format!("portal-lt-{pid}");
        seed_ws(&docs, "/tmp/docs-lt", "Documents");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            SubdomainWorkspace::self_stamp(&conn, &portal, &docs, Some("localhost:3000")).unwrap();
        }
        k2_core::tunnel::subdomains::store(Default::default());
        let body = leftovers_json(&docs);
        let row = body["leftovers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["label"] == portal)
            .expect("portal row");
        assert_eq!(row["target"], "localhost:3000");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = SubdomainWorkspace::unclaim(&conn, &portal);
        }
    }

    #[test]
    fn nested_public_url_matches_renderer_rules() {
        assert_eq!(
            nested_public_url("staging", "rosson", Some("https://rosson.k2.dev")),
            Some("https://staging.rosson.k2.dev".into())
        );
        assert_eq!(
            nested_public_url("staging", "rosson", None),
            Some("https://staging.rosson.k2.dev".into())
        );
        assert_eq!(nested_public_url("staging", "", None), None);
        assert_eq!(
            nested_public_url("", "rosson", Some("https://x.k2.dev")),
            None
        );
    }

    fn claim_params(project: &str, label: &str, target: Option<&str>) -> HashMap<String, String> {
        let mut p = HashMap::new();
        p.insert("project".into(), project.into());
        p.insert("label".into(), label.into());
        if let Some(t) = target {
            p.insert("target".into(), t.into());
        }
        p
    }

    #[test]
    fn subdomain_claim_is_self_only_and_persists_target() {
        let pid = std::process::id();
        let docs = format!("docs-claim-{pid}");
        let sales = format!("sales-claim-{pid}");
        let label = format!("claim-{pid}");
        seed_ws(&docs, "/tmp/docs-claim", "Documents");
        seed_ws(&sales, "/tmp/sales-claim", "Sales");

        let resp = dispatch_post(
            "/cli/publish/subdomain/claim",
            &claim_params(&docs, &label, Some("localhost:3000")),
        );
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(body["success"], true);
        assert_eq!(body["projectId"], docs);

        let steal = dispatch_post(
            "/cli/publish/subdomain/claim",
            &claim_params(&sales, &label, Some("localhost:9")),
        );
        assert_eq!(steal.status, "409 Conflict", "body={}", steal.body);
        let sv: serde_json::Value = serde_json::from_str(&steal.body).unwrap();
        assert_eq!(sv["code"], "label_owned_by_other_workspace");
        // N6: the holder's agent by name and address; never its id or path.
        assert_eq!(sv["owner"]["agent"], "Documents", "{}", steal.body);
        assert!(!steal.body.contains("/tmp/docs-claim") && !steal.body.contains(&format!("\"{docs}\"")), "no path or id leak: {}", steal.body);

        // check=1 (CLI preflight) refuses the same way and writes nothing.
        let mut check = claim_params(&sales, &label, None);
        check.insert("check".into(), "1".into());
        let pre = dispatch_post("/cli/publish/subdomain/claim", &check);
        assert_eq!(pre.status, "409 Conflict", "body={}", pre.body);
        let mut own_check = claim_params(&docs, &label, None);
        own_check.insert("check".into(), "1".into());
        let pre = dispatch_post("/cli/publish/subdomain/claim", &own_check);
        assert_eq!(pre.status, "200 OK", "owner re-check; body={}", pre.body);

        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let map = SubdomainWorkspace::map(&conn).unwrap();
            assert_eq!(map.get(&label).map(String::as_str), Some(docs.as_str()));
            let rows = SubdomainWorkspace::list_for_project(&conn, &docs).unwrap();
            assert_eq!(
                rows.iter()
                    .find(|(l, _)| l == &label)
                    .map(|(_, t)| t.as_str()),
                Some("localhost:3000")
            );
            let _ = SubdomainWorkspace::unclaim(&conn, &label);
        }
    }

    /// AR6: the owner/admin sees who holds a label and can move it with
    /// an explicit transfer; the move is audited. The same workspace
    /// re-pointing its own label keeps working.
    #[test]
    fn subdomain_claim_owner_transfer_is_explicit_and_audited() {
        let _home = crate::test_support::TempHome::new();
        let pid = std::process::id();
        let docs = format!("docs-xfer-{pid}");
        let sales = format!("sales-xfer-{pid}");
        let label = format!("xfer-{pid}");
        seed_ws(&docs, "/tmp/docs-xfer", "Documents");
        seed_ws(&sales, "/tmp/sales-xfer", "Sales");
        let resp = dispatch_post(
            "/cli/publish/subdomain/claim",
            &claim_params(&docs, &label, Some("localhost:1")),
        );
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let resp = dispatch_post(
            "/cli/publish/subdomain/claim",
            &claim_params(&docs, &label, Some("localhost:2")),
        );
        assert_eq!(resp.status, "200 OK", "self re-point; body={}", resp.body);

        let mgr = || label_owner::Caller::Manager { actor: "owner-token".into() };
        let resp = label_owner::with_caller(mgr(), || {
            dispatch_post("/cli/publish/subdomain/claim", &claim_params(&sales, &label, None))
        });
        assert_eq!(resp.status, "409 Conflict", "no flag, no move; body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["owner"]["projectId"], docs.as_str(), "owner sees the holder");
        assert_eq!(v["owner"]["name"], "Documents");

        let mut xfer = claim_params(&sales, &label, Some("localhost:3"));
        xfer.insert("transfer".into(), "1".into());
        let resp = label_owner::with_caller(mgr(), || {
            dispatch_post("/cli/publish/subdomain/claim", &xfer)
        });
        assert_eq!(resp.status, "200 OK", "owner transfer; body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["transferred"], true);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            assert_eq!(
                SubdomainWorkspace::owner_of(&conn, &label).unwrap().as_deref(),
                Some(sales.as_str())
            );
            let _ = SubdomainWorkspace::unclaim(&conn, &label);
        }
        let events = k2_core::auth_audit::tail(50).expect("audit tail");
        assert!(
            events.iter().any(|e| e["event"] == "label-transfer"
                && e["outcome"].as_str().is_some_and(|o| o.contains(&label)
                    && o.contains(&format!("from={docs}"))
                    && o.contains(&format!("to={sales}")))),
            "transfer must be audited: {events:?}"
        );
    }

    #[test]
    fn subdomain_unclaim_drops_own_sticker_not_foreign() {
        // leftovers_json → tunnel_status() → private runtime dir: keep it
        // under a temp HOME (K2_RUNTIME_DIR), never the real one.
        let _home = crate::test_support::TempHome::new();
        let pid = std::process::id();
        let docs = format!("docs-unclaim-{pid}");
        let sales = format!("sales-unclaim-{pid}");
        let label = format!("unclaim-{pid}");
        seed_ws(&docs, "/tmp/docs-unclaim", "Documents");
        seed_ws(&sales, "/tmp/sales-unclaim", "Sales");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            SubdomainWorkspace::self_stamp(&conn, &label, &docs, Some("localhost:3000")).unwrap();
        }
        let steal = dispatch_post(
            "/cli/publish/subdomain/unclaim",
            &claim_params(&sales, &label, None),
        );
        assert_eq!(steal.status, "403 Forbidden", "body={}", steal.body);
        let ok = dispatch_post(
            "/cli/publish/subdomain/unclaim",
            &claim_params(&docs, &label, None),
        );
        assert_eq!(ok.status, "200 OK", "body={}", ok.body);
        let body: serde_json::Value = serde_json::from_str(&ok.body).unwrap();
        assert_eq!(body["removed"], true);
        let gone = leftovers_json(&docs);
        assert!(
            !gone["leftovers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["label"] == label),
            "successful unclaim must drop the leftover; body={gone}"
        );
    }

    #[test]
    fn leftovers_get_requires_project() {
        let resp = dispatch("/cli/publish/leftovers", &HashMap::new()).unwrap();
        assert_eq!(resp.status, "400 Bad Request");
    }

    /// `--probe-timeout` on the run route: a duration (or ms) is stored on
    /// the row and comes back in the Service JSON; over 15 min, zero and
    /// junk are 400 before any row; the GET twin stays 405 with it.
    #[test]
    fn run_route_round_trips_probe_timeout_and_enforces_the_cap() {
        let home = crate::test_support::TempHome::new();
        let _probe_ms = k2_core::test_env::EnvVar::set("K2_PUBLISH_PROBE_MS", "3000");
        let pid = std::process::id();
        let ws = format!("grace-route-{pid}");
        let dir = home.path().join("ws-grace-route");
        std::fs::create_dir_all(&dir).unwrap();
        let dir_s = dir.to_string_lossy().to_string();
        seed_ws(&ws, &dir_s, "Grace");
        // Not bind(:0)-then-drop: another test can be handed that port first.
        let port = k2_core::test_env::loopback_test_port();
        let name = format!("graceroute{pid}");
        let params = |extra: &[(&str, &str)]| {
            let mut p = HashMap::new();
            p.insert("project".to_string(), ws.clone());
            p.insert("name".to_string(), name.clone());
            p.insert(
                "cmd".to_string(),
                format!(
                    "python3 -c \"import socket,itertools; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('127.0.0.1',{port})); s.listen(8); [s.accept()[0].close() for _ in itertools.count()]\""
                ),
            );
            p.insert("port".to_string(), port.to_string());
            p.insert("noTunnel".to_string(), "1".to_string());
            for (k, v) in extra {
                p.insert(k.to_string(), v.to_string());
            }
            p
        };

        for (k, v, want) in [
            ("probeTimeout", "16m", "at most 15m"),
            ("probeTimeout", "0s", "--probe-timeout"),
            ("probeTimeout", "soon", "--probe-timeout"),
            ("probeTimeoutMs", "900001", "at most 15m"),
            ("probeTimeoutMs", "0", "at most 15m"),
        ] {
            let r = dispatch_post("/cli/publish/run", &params(&[(k, v)]));
            assert_eq!(r.status, "400 Bad Request", "{k}={v}: {}", r.body);
            assert!(r.body.contains(want), "{k}={v}: {}", r.body);
            let db = k2_core::db::shared();
            let conn = db.lock();
            assert!(
                k2_core::published_services::get_by_project_name(&conn, &ws, &name).unwrap().is_none(),
                "{k}={v} must not write a row"
            );
        }

        let get = dispatch("/cli/publish/run", &params(&[("probeTimeout", "2m")])).expect("GET twin");
        assert_eq!(get.status, "405 Method Not Allowed", "POST-only guard: {}", get.body);

        let r = dispatch_post("/cli/publish/run", &params(&[("probeTimeout", "2m")]));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["probeTimeoutMs"], 120_000, "{v}");
        assert_eq!(v["probeTimeoutEffectiveMs"], 120_000, "{v}");
        assert_eq!(v["status"], "running", "{v}");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let row = k2_core::published_services::get_by_project_name(&conn, &ws, &name).unwrap().expect("row");
            assert_eq!(row.probe_timeout_ms, Some(120_000), "stored on the row");
        }
        let list = dispatch("/cli/publish/list", &{
            let mut p = HashMap::new();
            p.insert("project".to_string(), ws.clone());
            p
        })
        .expect("list");
        let lv: serde_json::Value = serde_json::from_str(&list.body).unwrap();
        let svc = lv["services"]
            .as_array()
            .expect("services")
            .iter()
            .find(|s| s["name"] == name.as_str())
            .expect("listed")
            .clone();
        assert_eq!(svc["probeTimeoutMs"], 120_000, "list JSON carries it: {svc}");
        publish_runtime::stop(&ws, &name).expect("stop");
        std::thread::sleep(std::time::Duration::from_millis(300));

        // probeTimeoutMs wins; then `default` clears it.
        let r = dispatch_post("/cli/publish/run", &params(&[("probeTimeoutMs", "90000")]));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["probeTimeoutMs"], 90_000, "{v}");
        publish_runtime::stop(&ws, &name).expect("stop");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let r = dispatch_post("/cli/publish/run", &params(&[("probeTimeout", "default")]));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert!(v["probeTimeoutMs"].is_null(), "{v}");
        assert_eq!(v["probeTimeoutEffectiveMs"], 3000, "{v}");

        let _ = publish_runtime::rm(&ws, &name, true);
    }

    /// Smoke r6: `k2 publish client-ip <label> on` stored any label and
    /// said "on". An unknown label is refused (404, nothing written); a
    /// published service, a stamped nested label and an App are known;
    /// `off` still clears a listed label whose service is gone.
    #[test]
    fn client_ip_refuses_an_unknown_label_and_writes_nothing() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("INSERT INTO projects (id, path, name) VALUES ('cipws', '/tmp/cipws', 'cipws')", []).unwrap();
            k2_core::published_services::insert(&conn, "cipws", "cipsvc", "node s.js", "/tmp", 4311, "tunnel", "running", "cmd", "").unwrap();
            SubdomainWorkspace::claim(&conn, "cipstage", "cipws").unwrap();
            conn.execute(
                "INSERT INTO apps (app_id, service_id, owner_workspace, state, created_by, created_at, updated_at) \
                 VALUES ('cipapp', 'svc-cipapp', 'cipws', 'running', 'test', 1, 1)",
                [],
            )
            .unwrap();
        }
        let set = |label: &str, on: &str| {
            let p: HashMap<String, String> = [("label".to_string(), label.to_string()), ("on".to_string(), on.to_string())].into();
            let r = dispatch_post("/cli/publish/client-ip", &p);
            let v: serde_json::Value = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body));
            (r.status, v)
        };
        let (st, v) = set("nosuch-zz", "true");
        assert_eq!(st, "404 Not Found", "{v}");
        assert_eq!(v["code"], "label_not_found", "{v}");
        assert!(v["error"].as_str().unwrap().contains("nosuch-zz"), "{v}");
        assert!(k2_core::tunnel::client_ip_labels::load().is_empty(), "nothing stored for an unknown label");
        let (st, v) = set("nosuch-zz", "false");
        assert_eq!(st, "404 Not Found", "off for an unknown, unlisted label: {v}");
        for known in ["cipsvc", "CIPStage", "cipapp"] {
            let (st, v) = set(known, "true");
            assert_eq!(st, "200 OK", "{known}: {v}");
            assert_eq!(v["clientIp"], true, "{v}");
        }
        assert_eq!(
            k2_core::tunnel::client_ip_labels::load().into_iter().collect::<Vec<_>>(),
            vec!["cipapp".to_string(), "cipstage".to_string(), "cipsvc".to_string()]
        );
        // A listed label whose service is gone can still be turned off.
        k2_core::tunnel::client_ip_labels::set("gone-svc", true).unwrap();
        let (st, v) = set("gone-svc", "true");
        assert_eq!(st, "404 Not Found", "not re-confirmed on: {v}");
        let (st, v) = set("gone-svc", "false");
        assert_eq!(st, "200 OK", "{v}");
        assert!(!k2_core::tunnel::client_ip_labels::enabled("gone-svc"));
        let (st, _) = set("bad label", "true");
        assert_eq!(st, "400 Bad Request");
    }
}
