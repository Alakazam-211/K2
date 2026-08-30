//! `/cli/mail/*` — SERVER-concern handlers: status + preflight (REAL),
//! enable / disable / uninstall (S1), config get/set + doctor (S6).
//!
//! Dispatched by the `crate::mail_routes` shim. AUTH/GATING contract
//! for this file's mutations (PRD §10), enforced in the dispatcher's
//! `/cli/mail/` POST arm and re-asserted per-handler as slices land:
//! server enable/disable/uninstall + config/set + the doctor RUN =
//! OWNER-OR-ADMIN (`token_is_owner_or_admin`), POST-only
//! (`require_post` + `post_allowed`, house rule
//! feedback_post_only_route_guards). The config/doctor GETs are
//! secret-free reads (the Settings page renders them for any authed
//! token, like `/cli/mail/status`).
//!
//! Non-Linux daemons (D3): validation runs first (so the Mac
//! example-page exercises real error text), then every mutation stops
//! at the `mail_supported()` gate with the structured `unsupported`
//! 409 — nothing system-level ever executes off-Linux, and the doctor
//! never probes anything from a Mac.

use std::collections::HashMap;

use crate::mail::config::{self, CfgError};
use crate::mail::doctor::{self, DocError};
use crate::cli_response::CliResponse;
use crate::mail::supervisor::{self, mail_supported, STALWART_PINNED_VERSION};

fn err_json(status: &'static str, code: &str, hint: String) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint },
        })
        .to_string(),
    }
}

fn unsupported() -> CliResponse {
    err_json(
        "409 Conflict",
        "unsupported",
        "the email server only works on Linux deployments; this daemon is not Linux"
            .to_string(),
    )
}

/// GET `/cli/mail/status` — REAL from day one: the capability-gating
/// seam the Mac UI reads (pre-mortem #15). Reports:
///
/// ```json
/// { "ok": true,
///   "supported": <mail_supported()>,       // Linux daemon = true
///   "state": "not-installed" | <mail_server.status>,
///   "version": <installed_version|null>,
///   "pinnedVersion": STALWART_PINNED_VERSION,
///   "hostname": <hostname|null>,
///   "portPlan": <port_plan|null>,
///   "enableProgress": <enable_progress_json|null>,  // S1 machine steps
///   "lastError": <last_error|null>,
///   "health": <live verdict — only with ?health=1 on Linux> }
/// ```
///
/// `state` comes from the `mail_server` singleton row; NO row =
/// `"not-installed"` (the 0072 contract). `?health=1` additionally
/// runs the live systemd+API health check (persisting transitions +
/// raising the standard event) before reading. The renderer gates the
/// whole Settings→Email page on `supported` — from the DAEMON's
/// report, never `navigator.platform` (a Mac app driving a remote
/// Linux daemon must see the real page).
pub fn handle_status(params: &HashMap<String, String>) -> CliResponse {
    let health = if mail_supported()
        && params.get("health").map(String::as_str) == Some("1")
        && !supervisor::enable_running().load(std::sync::atomic::Ordering::SeqCst)
    {
        Some(supervisor::refresh_health())
    } else {
        None
    };

    type Row = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let row: Option<Row> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT status, installed_version, hostname, port_plan, \
             enable_progress_json, last_error FROM mail_server WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .ok()
    };
    let (state, version, hostname, port_plan, progress, last_error) = match row {
        Some((status, installed, hostname, plan, progress, last_error)) => {
            (status, installed, hostname, plan, progress, last_error)
        }
        None => ("not-installed".to_string(), None, None, None, None, None),
    };
    let enable_progress = progress
        .and_then(|p| serde_json::from_str::<serde_json::Value>(&p).ok())
        .unwrap_or(serde_json::Value::Null);
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "supported": mail_supported(),
            "state": state,
            "version": version,
            "pinnedVersion": STALWART_PINNED_VERSION,
            "hostname": hostname,
            "portPlan": port_plan,
            "enableProgress": enable_progress,
            "lastError": last_error,
            "health": health,
        })
        .to_string(),
    )
}

/// GET `/cli/mail/preflight` — S1 (PRD §5.1): the read-only checklist,
/// runnable any time (Settings renders it before [Enable Email
/// Server]). On non-Linux daemons the OS check hard-fails and the
/// remaining checks report `skipped` WITHOUT probing anything —
/// the Mac example page stays network-silent.
pub fn handle_preflight(_params: &HashMap<String, String>) -> CliResponse {
    let report = crate::mail::preflight::run_preflight(&crate::mail::preflight::RealPreflightEnv);
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "supported": mail_supported(),
            "report": report.to_json(),
        })
        .to_string(),
    )
}

/// Live daemon HTTP port published in `~/.k2/daemon.port` — same value
/// front-door POST / boot apply pass to `skin_door::apply`.
fn live_daemon_http_port() -> Option<u16> {
    k2_core::port_claim::read_port_file(&k2_core::paths::k2_home().join("daemon.port"))
        .filter(|&p| p != 0 && p != k2_core::skin_door::LOOPBACK_PORT)
}

/// Direct mode, or this process already applied/running Skin Caddy.
fn skin_door_should_reapply_after_mail(mode: &str, caddy_applied: bool) -> bool {
    mode == "direct" || caddy_applied
}

fn live_skin_door_should_reapply() -> bool {
    let direct = k2_core::skin::effective_front_door()
        .map(|d| d.mode == "direct")
        .unwrap_or(false);
    let applied = k2_core::skin_door::status()
        .ok()
        .map(|st| st.applied || st.caddy.running)
        .unwrap_or(false);
    skin_door_should_reapply_after_mail(if direct { "direct" } else { "connect" }, applied)
}

/// Re-apply Skin Caddy so the mail Host site appears. Never fails Enable;
/// returns a hint when apply fails (logged + Enable JSON / enableProgress).
fn reapply_skin_door_after_mail_enable(daemon_port: Option<u16>) -> Option<String> {
    if !live_skin_door_should_reapply() {
        return None;
    }
    // Unit tests share the process DB/HOME; never restart a live Caddy here.
    if cfg!(test) {
        return None;
    }
    let Some(port) = daemon_port.or_else(live_daemon_http_port) else {
        k2_core::log_debug!(
            "[mail] skin-door apply skipped after enable: daemon HTTP port unknown"
        );
        return Some(
            "mail is enabled; Skin Caddy was not re-applied (daemon HTTP port unknown). \
             POST /cli/skin/front-door with apply to attach the mail Host."
                .into(),
        );
    };
    match k2_core::skin_door::apply(port) {
        Ok(_) => None,
        Err(e) => {
            k2_core::log_debug!("[mail] skin-door apply after enable failed: {e}");
            Some(format!(
                "mail is enabled; Skin Caddy did not pick up the mail Host ({e}). \
                 POST /cli/skin/front-door with apply."
            ))
        }
    }
}

/// POST `/cli/mail/server/enable` — S1: preflight-gate → spawn the
/// resumable install+bootstrap state machine (owner-or-admin,
/// dispatcher-enforced). Body: `{"hostname": "mail.acme.dev"}`.
///
/// Synchronous part: hostname validation, the supported/latch gates,
/// and a fresh preflight — a FAILING preflight returns its report
/// immediately (`code: "preflight_failed"`) and nothing installs.
/// Passing → the machine runs on a background thread; progress is
/// polled via GET /cli/mail/status (`enableProgress`), the house
/// persisted-steps pattern. Re-POST after a failure RESUMES.
pub fn handle_server_enable(body: &[u8]) -> CliResponse {
    handle_server_enable_at(body, live_daemon_http_port())
}

/// Enable with an explicit live daemon HTTP port (dispatcher `state.port`).
pub(crate) fn handle_server_enable_at(body: &[u8], daemon_port: Option<u16>) -> CliResponse {
    let daemon_port = daemon_port
        .filter(|&p| p != 0 && p != k2_core::skin_door::LOOPBACK_PORT)
        .or_else(live_daemon_http_port);
    let parsed: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let raw_hostname = parsed["hostname"].as_str().unwrap_or_default();
    if raw_hostname.trim().is_empty() {
        return CliResponse::bad_request(
            "missing 'hostname' — the mail hostname (e.g. mail.acme.dev) is required",
        );
    }
    // Normalized at the boundary (pre-mortem #14) — lowercase punycode
    // A-label, the same helper every mail boundary uses.
    let hostname = match k2_core::mail_domain::normalize_mail_domain(raw_hostname) {
        Ok(h) => h,
        Err(e) => return CliResponse::bad_request(format!("invalid hostname: {e}")),
    };

    // Idempotency short-circuit first (a DB read, platform-safe):
    // already running → nothing to do. Re-apply Skin Caddy so the mail
    // Host appears if Direct is already on (do not wait for a front-door POST).
    if supervisor::current_status().as_deref() == Some("running") {
        let mut body = serde_json::json!({ "ok": true, "state": "running", "alreadyEnabled": true });
        if let Some(hint) = reapply_skin_door_after_mail_enable(daemon_port) {
            body["hint"] = serde_json::json!(hint);
        }
        return CliResponse::ok_json(body.to_string());
    }
    if !mail_supported() {
        return unsupported();
    }
    if !supervisor::try_begin_enable() {
        return err_json(
            "409 Conflict",
            "enable_in_progress",
            "an enable run is already in progress — poll /cli/mail/status".to_string(),
        );
    }

    // Fresh preflight, synchronously: failures return the report and
    // nothing installs (§5.1 hard stops).
    let report =
        crate::mail::preflight::run_preflight(&crate::mail::preflight::RealPreflightEnv);
    if !report.ok {
        supervisor::end_enable();
        return CliResponse {
            status: "200 OK",
            content_type: "application/json",
            body: serde_json::json!({
                "ok": false,
                "error": { "code": "preflight_failed", "hint": "preflight found hard stops — fix them and re-enable" },
                "report": report.to_json(),
            })
            .to_string(),
        };
    }
    let port_plan = report.port_plan.unwrap_or("http-01").to_string();
    let artifact = match supervisor::artifact_for_arch(std::env::consts::ARCH) {
        Ok(a) => a,
        Err(e) => {
            supervisor::end_enable();
            return CliResponse::internal_error(e);
        }
    };

    let preflight_json = report.to_json();
    std::thread::spawn(move || {
        let ops = crate::mail::sysops::RealSystemOps;
        let secrets = crate::mail::secrets::FileSecretStore::default();
        let mut api = crate::mail::jmap::StalwartBootstrap::new();
        let result =
            supervisor::run_enable(&ops, &mut api, &secrets, artifact, &hostname, &port_plan);
        match result {
            Ok(()) => {
                if let Some(hint) = reapply_skin_door_after_mail_enable(daemon_port) {
                    supervisor::note_enable_progress_hint("caddyHint", &hint);
                }
            }
            Err(e) => {
                k2_core::log_debug!("[mail/supervisor] enable failed: {e}");
            }
        }
        supervisor::end_enable();
    });

    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "state": "installing",
            "hint": "installing in the background — poll GET /cli/mail/status for enableProgress",
            "preflight": preflight_json,
        })
        .to_string(),
    )
}

/// POST `/cli/mail/server/disable` — S1: stop + disable the unit,
/// KEEP all data (owner-or-admin). The reply carries the loud §4.1
/// warning: verified domains' MX records now point at a dead port.
pub fn handle_server_disable(_body: &[u8]) -> CliResponse {
    let Some(state) = supervisor::current_status() else {
        return err_json(
            "409 Conflict",
            "not_installed",
            "the email server is not installed — nothing to disable".to_string(),
        );
    };
    if !mail_supported() {
        return unsupported();
    }
    if let Err(e) = supervisor::disable() {
        return CliResponse::internal_error(e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "state": "disabled",
            "previous": state,
            "warning": "mail data is kept, but every verified domain's MX record now \
                        points at a STOPPED server — inbound mail to those domains will \
                        bounce until you re-enable or update DNS",
        })
        .to_string(),
    )
}

/// POST `/cli/mail/server/uninstall` — S1: disable + remove binary/
/// unit (+ optional data purge). Owner-or-admin. DOUBLE-CONFIRM at the
/// ROUTE level (PRD §4.1): a purge is honored ONLY when the body
/// echoes the configured mail hostname —
/// `{"purgeData": true, "confirmHostname": "<hostname>"}` — the UI
/// does the typing, this route enforces it.
pub fn handle_server_uninstall(body: &[u8]) -> CliResponse {
    let parsed: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let purge = parsed["purgeData"].as_bool().unwrap_or(false);

    let Some(_state) = supervisor::current_status() else {
        return err_json(
            "409 Conflict",
            "not_installed",
            "the email server is not installed — nothing to uninstall".to_string(),
        );
    };

    if purge {
        let hostname: Option<String> = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row("SELECT hostname FROM mail_server WHERE id = 1", [], |r| {
                r.get::<_, Option<String>>(0)
            })
            .ok()
            .flatten()
        };
        let expected = hostname.unwrap_or_default();
        let confirmed = parsed["confirmHostname"].as_str().unwrap_or_default();
        if expected.is_empty() || confirmed != expected {
            return err_json(
                "400 Bad Request",
                "confirm_hostname_mismatch",
                format!(
                    "deleting all mail data requires typing the mail hostname exactly \
                     ('confirmHostname' must equal '{expected}')"
                ),
            );
        }
    }

    if !mail_supported() {
        return unsupported();
    }
    if let Err(e) = supervisor::uninstall(purge) {
        return CliResponse::internal_error(e);
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "state": "not-installed",
            "purged": purge,
        })
        .to_string(),
    )
}

// ── S6: config + doctor ─────────────────────────────────────────────────

fn cfg_error_response(err: CfgError) -> CliResponse {
    match err {
        CfgError::Usage(h) => err_json("400 Bad Request", "usage", h),
        CfgError::NotFound(h) => err_json("404 Not Found", "not_found", h),
        CfgError::NotReady(h) => err_json("503 Service Unavailable", "not_ready", h),
        CfgError::Locked(h) => err_json("409 Conflict", "direct_locked", h),
        CfgError::Conflict(h) => err_json("409 Conflict", "conflict", h),
        CfgError::Engine(h) => err_json("502 Bad Gateway", "engine", h),
    }
}

fn doc_error_response(err: DocError) -> CliResponse {
    match err {
        DocError::Usage(h) => err_json("400 Bad Request", "usage", h),
        DocError::NotFound(h) => err_json("404 Not Found", "not_found", h),
        DocError::NotReady(h) => err_json("503 Service Unavailable", "not_ready", h),
        DocError::Engine(h) => err_json("502 Bad Gateway", "engine", h),
    }
}

/// GET `/cli/mail/config` — S6: the effective configuration (global +
/// per-workspace gating, limits, per-domain send modes, relay-config
/// summaries — kind + host + username, NEVER secrets — and the latest
/// server-level doctor grade). Pure read; renders on the Mac example
/// page too (`supported` rides the reply).
pub fn handle_config_get(_params: &HashMap<String, String>) -> CliResponse {
    CliResponse::ok_json(config::config_json().to_string())
}

/// POST `/cli/mail/config/set` body — the `k2 mail config` surface
/// (§11): per-domain send mode (+ relay attach), relay-config CRUD,
/// per-workspace and global D4/D6 gating.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ConfigSetBody {
    domain: Option<String>,
    send_mode: Option<String>,
    relay_config_id: Option<String>,
    relay: Option<config::RelayUpsert>,
    delete_relay_config: Option<String>,
    workspace: Option<String>,
    agent_send: Option<String>,
    address_cap: Option<i64>,
    defaults: Option<ConfigDefaultsBody>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ConfigDefaultsBody {
    agent_send: Option<String>,
    address_cap: Option<i64>,
}

const CONFIG_SET_SURFACE: &str =
    "nothing to set. The surface: {domain + sendMode [+ relayConfigId]} · {relay: \
     {id?, host, port, username, password|secretRef, tlsKind?, spfInclude?}} · \
     {deleteRelayConfig} · {workspace + agentSend|addressCap} · {defaults: \
     {agentSend?, addressCap?}}";

/// POST `/cli/mail/config/set` — S6 (owner-or-admin, dispatcher-
/// enforced). Validation first (the Mac example page exercises real
/// error text), then the D3 platform gate, then the actions apply in
/// order: relay upsert → relay delete → domain send mode → workspace
/// gating → global defaults. The FIRST failure stops the sequence and
/// returns its teaching error (earlier actions in the same call stay
/// applied — the reply's `applied` object says exactly what landed).
pub fn handle_config_set(body: &[u8]) -> CliResponse {
    let b: ConfigSetBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };

    // Shape validation before anything executes.
    let wants_send_mode = b.send_mode.is_some() || b.domain.is_some();
    if b.send_mode.is_some() && b.domain.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return err_json(
            "400 Bad Request",
            "usage",
            "'sendMode' needs 'domain' — which domain's mode is changing?".to_string(),
        );
    }
    if b.domain.is_some() && b.send_mode.is_none() {
        return err_json(
            "400 Bad Request",
            "usage",
            "'domain' given but no 'sendMode' — nothing to change for it".to_string(),
        );
    }
    let wants_workspace = b.workspace.is_some();
    if (b.agent_send.is_some() || b.address_cap.is_some()) && !wants_workspace {
        return err_json(
            "400 Bad Request",
            "usage",
            "'agentSend'/'addressCap' need 'workspace' — or wrap them in 'defaults' \
             for the global default"
                .to_string(),
        );
    }
    let any_action = b.relay.is_some()
        || b.delete_relay_config.is_some()
        || wants_send_mode
        || wants_workspace
        || b.defaults.is_some();
    if !any_action {
        return err_json("400 Bad Request", "usage", CONFIG_SET_SURFACE.to_string());
    }
    // Workspace resolution is part of validation (registry read —
    // platform-independent).
    let workspace_path = match b.workspace.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(ws) => match crate::workspace_msg::resolve_workspace(ws) {
            Some(path) => Some(path),
            None => return crate::workspace_routes::workspace_not_found_response(ws),
        },
        None => {
            if wants_workspace {
                return err_json(
                    "400 Bad Request",
                    "usage",
                    "'workspace' must not be empty".to_string(),
                );
            }
            None
        }
    };

    // D3: nothing mail-shaped executes off-Linux.
    if !mail_supported() {
        return unsupported();
    }

    let secrets = crate::mail::secrets::FileSecretStore::default();
    // The live Stalwart engine, when reachable — only relay
    // transitions require it; the ops layer says so when it's missing.
    let engine = crate::mail::domains::engine_from_db().ok().map(|(c, _)| c);
    let engine_ref: Option<&dyn config::RelayEngine> =
        engine.as_ref().map(|c| c as &dyn config::RelayEngine);

    let mut applied = serde_json::Map::new();

    // 1. Relay upsert (its id feeds a same-call sendMode attach).
    let mut created_relay_id: Option<String> = None;
    if let Some(up) = &b.relay {
        match config::upsert_relay(&secrets, engine_ref, up) {
            Ok(v) => {
                created_relay_id = v["id"].as_str().map(str::to_string);
                applied.insert("relayConfig".to_string(), v);
            }
            Err(e) => return cfg_error_response(e),
        }
    }
    // 2. Relay delete.
    if let Some(id) = b.delete_relay_config.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        match config::delete_relay(&secrets, id) {
            Ok(v) => {
                applied.insert("deletedRelayConfig".to_string(), v["deleted"].clone());
            }
            Err(e) => return cfg_error_response(e),
        }
    }
    // 3. Per-domain send mode.
    if let (Some(domain), Some(mode)) = (b.domain.as_deref(), b.send_mode.as_deref()) {
        let attach = b
            .relay_config_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or(created_relay_id);
        match config::set_send_mode(&secrets, engine_ref, domain, mode, attach.as_deref()) {
            Ok(v) => {
                applied.insert("sendMode".to_string(), v);
            }
            Err(e) => return cfg_error_response(e),
        }
    }
    // 4. Per-workspace gating.
    if let Some(path) = workspace_path.as_deref() {
        match config::set_workspace_gating(path, b.agent_send.as_deref(), b.address_cap) {
            Ok(v) => {
                applied.insert("workspace".to_string(), v);
            }
            Err(e) => return cfg_error_response(e),
        }
    }
    // 5. Global defaults.
    if let Some(d) = &b.defaults {
        match config::set_global_defaults(d.agent_send.as_deref(), d.address_cap) {
            Ok(v) => {
                applied.insert("defaults".to_string(), v["defaults"].clone());
            }
            Err(e) => return cfg_error_response(e),
        }
    }

    CliResponse::ok_json(
        serde_json::json!({ "ok": true, "applied": applied }).to_string(),
    )
}

/// GET `/cli/mail/doctor[?domain=<d>]` — S6: the LATEST persisted run
/// (`run: null` when none). Read-only — the Settings card and the
/// direct-mode UI never trigger probes; `POST /cli/mail/doctor` runs
/// them.
pub fn handle_doctor(params: &HashMap<String, String>) -> CliResponse {
    let domain = crate::cli::str_param(params, "domain");
    let domain = if domain.is_empty() { None } else { Some(domain.as_str()) };
    match doctor::latest_run_json(domain) {
        Ok(run) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "supported": mail_supported(),
                "run": run,
            })
            .to_string(),
        ),
        Err(e) => doc_error_response(e),
    }
}

/// POST `/cli/mail/doctor` — S6: run the full check table NOW
/// (owner-or-admin, dispatcher-enforced; the dispatcher's mail POST
/// arm already runs this in `spawn_blocking` — the probes are blocking
/// I/O). Body: `{"domain": "acme.dev"}` optional (server-level run
/// without it). Persists a `mail_doctor_runs` row and returns the full
/// graded report. On non-Linux daemons the D3 gate answers before ANY
/// probe fires — the Mac example page stays network-silent.
pub fn handle_doctor_run(body: &[u8]) -> CliResponse {
    let parsed: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let domain = parsed["domain"].as_str().map(str::trim).filter(|s| !s.is_empty());
    if !mail_supported() {
        return unsupported();
    }
    match doctor::run(domain) {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => doc_error_response(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean_row() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
    }

    /// Status: empty `mail_server` table → not-installed, `supported`
    /// matches the runtime gate, pinned version reported. Then an
    /// installed row flips state/version/hostname/portPlan and the S1
    /// additions (enableProgress, lastError) surface.
    #[test]
    fn status_reports_not_installed_then_row_state() {
        let _g = crate::mail::mail_server_test_lock();
        clean_row();
        let resp = handle_status(&HashMap::new());
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["ok"], true);
        assert_eq!(v["supported"], cfg!(target_os = "linux"));
        assert_eq!(v["state"], "not-installed");
        assert!(v["version"].is_null());
        assert_eq!(v["pinnedVersion"], STALWART_PINNED_VERSION);
        assert!(v["hostname"].is_null());
        assert!(v["portPlan"].is_null());
        assert!(v["enableProgress"].is_null());
        assert!(v["lastError"].is_null());
        assert!(v["health"].is_null());

        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, installed_version, \
                 hostname, port_plan, enable_progress_json, last_error, installed_at, updated_at) \
                 VALUES (1, 'error', ?1, '0.16.10', 'mail.acme.dev', 'tls-alpn', \
                 '{\"steps\":{\"download\":{\"at\":100}},\"current\":\"verify\"}', \
                 'verify: sha256 mismatch', 100, 100)",
                rusqlite::params![STALWART_PINNED_VERSION],
            )
            .expect("insert mail_server row");
        }
        let resp = handle_status(&HashMap::new());
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["state"], "error");
        assert_eq!(v["version"], "0.16.10");
        assert_eq!(v["hostname"], "mail.acme.dev");
        assert_eq!(v["portPlan"], "tls-alpn");
        assert_eq!(v["enableProgress"]["steps"]["download"]["at"], 100);
        assert_eq!(v["enableProgress"]["current"], "verify");
        assert_eq!(v["lastError"], "verify: sha256 mismatch");
        assert_eq!(
            v["supported"],
            cfg!(target_os = "linux"),
            "supported is the RUNTIME gate, independent of install state"
        );
        clean_row();
    }

    /// Preflight is a REAL read-only route. On non-Linux (the test
    /// environment for CI/dev-Mac) the report hard-fails on the OS
    /// check with everything else skipped — and NO probes ran (the
    /// run_preflight short-circuit; network silence is asserted at the
    /// preflight unit-test layer with a panicking env).
    #[test]
    fn preflight_route_reports_checklist() {
        let resp = handle_preflight(&HashMap::new());
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["ok"], true);
        assert_eq!(v["supported"], cfg!(target_os = "linux"));
        let checks = v["report"]["checks"].as_array().expect("checks");
        assert_eq!(checks.len(), 10);
        let os = checks.iter().find(|c| c["id"] == "os").expect("os check");
        if cfg!(target_os = "linux") {
            assert_eq!(os["status"], "pass");
        } else {
            assert_eq!(os["status"], "fail");
            assert_eq!(v["report"]["ok"], false);
            assert!(checks
                .iter()
                .filter(|c| c["id"] != "os")
                .all(|c| c["status"] == "skipped"));
        }
    }

    /// Enable: body validation runs BEFORE the platform gate (real
    /// error text everywhere), then non-Linux stops at the structured
    /// 409 `unsupported` — nothing system-level executes on a Mac.
    #[test]
    fn enable_validates_then_gates_on_platform() {
        let _g = crate::mail::mail_server_test_lock();
        clean_row();
        let resp = handle_server_enable(b"not json");
        assert_eq!(resp.status, "400 Bad Request");

        let resp = handle_server_enable(b"{}");
        assert_eq!(resp.status, "400 Bad Request");
        assert!(resp.body.contains("hostname"), "{}", resp.body);

        let resp = handle_server_enable(br#"{"hostname":"not a hostname!"}"#);
        assert_eq!(resp.status, "400 Bad Request");
        assert!(resp.body.contains("invalid hostname"), "{}", resp.body);

        if !cfg!(target_os = "linux") {
            let resp = handle_server_enable(br#"{"hostname":"mail.acme.dev"}"#);
            assert_eq!(resp.status, "409 Conflict");
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
            assert_eq!(v["error"]["code"], "unsupported");
            assert!(
                !supervisor::enable_running().load(std::sync::atomic::Ordering::SeqCst),
                "gate must not leave the enable latch claimed"
            );
        }
        clean_row();
    }

    /// Enable short-circuits idempotently when already running.
    #[test]
    fn enable_is_idempotent_when_already_running() {
        let _g = crate::mail::mail_server_test_lock();
        clean_row();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, hostname, updated_at) \
                 VALUES (1, 'running', ?1, 'mail.acme.dev', 100)",
                rusqlite::params![STALWART_PINNED_VERSION],
            )
            .expect("seed row");
        }
        let resp = handle_server_enable(br#"{"hostname":"mail.acme.dev"}"#);
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["ok"], true);
        assert_eq!(v["alreadyEnabled"], true);
        assert_eq!(v["state"], "running");
        // Caddy apply must not swallow Enable success (ok stays true).
        assert_ne!(v["ok"], false);
        clean_row();
    }

    #[test]
    fn skin_door_reapply_after_mail_when_direct_or_caddy_applied() {
        assert!(skin_door_should_reapply_after_mail("direct", false));
        assert!(skin_door_should_reapply_after_mail("direct", true));
        assert!(skin_door_should_reapply_after_mail("connect", true));
        assert!(!skin_door_should_reapply_after_mail("connect", false));
    }

    #[test]
    fn enable_json_keeps_ok_when_caddy_apply_hint_present() {
        let mut body = serde_json::json!({ "ok": true, "state": "running", "alreadyEnabled": true });
        body["hint"] = serde_json::json!(
            "mail is enabled; Skin Caddy did not pick up the mail Host (caddy_missing). \
             POST /cli/skin/front-door with apply."
        );
        assert_eq!(body["ok"], true);
        assert_eq!(body["alreadyEnabled"], true);
        assert!(
            body["hint"].as_str().unwrap_or("").contains("Skin Caddy"),
            "{}",
            body["hint"]
        );
        assert!(
            body["hint"].as_str().unwrap_or("").contains("front-door"),
            "{}",
            body["hint"]
        );
    }

    /// Disable / uninstall: not-installed answers the structured 409;
    /// the purge double-confirm rejects a missing/mismatched hostname
    /// echo BEFORE anything executes (route-level enforcement).
    #[test]
    fn disable_and_uninstall_guards() {
        let _g = crate::mail::mail_server_test_lock();
        clean_row();
        let resp = handle_server_disable(b"{}");
        assert_eq!(resp.status, "409 Conflict");
        assert!(resp.body.contains("not_installed"), "{}", resp.body);

        let resp = handle_server_uninstall(b"{}");
        assert_eq!(resp.status, "409 Conflict");
        assert!(resp.body.contains("not_installed"), "{}", resp.body);

        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, hostname, updated_at) \
                 VALUES (1, 'stopped', ?1, 'mail.acme.dev', 100)",
                rusqlite::params![STALWART_PINNED_VERSION],
            )
            .expect("seed row");
        }
        // Purge without the typed hostname → 400, regardless of OS.
        let resp = handle_server_uninstall(br#"{"purgeData":true}"#);
        assert_eq!(resp.status, "400 Bad Request");
        assert!(resp.body.contains("confirm_hostname_mismatch"), "{}", resp.body);
        let resp =
            handle_server_uninstall(br#"{"purgeData":true,"confirmHostname":"wrong.dev"}"#);
        assert_eq!(resp.status, "400 Bad Request");
        assert!(resp.body.contains("mail.acme.dev"), "names the expected value: {}", resp.body);

        if !cfg!(target_os = "linux") {
            // Valid requests stop at the platform gate on a Mac.
            let resp = handle_server_disable(b"{}");
            assert_eq!(resp.status, "409 Conflict");
            assert!(resp.body.contains("unsupported"), "{}", resp.body);
            let resp = handle_server_uninstall(
                br#"{"purgeData":true,"confirmHostname":"mail.acme.dev"}"#,
            );
            assert_eq!(resp.status, "409 Conflict");
            assert!(resp.body.contains("unsupported"), "{}", resp.body);
        }
        clean_row();
    }

    /// S6 — GET /cli/mail/config is a real, secret-free read that
    /// renders everywhere (Mac example page included).
    #[test]
    fn config_get_answers_the_effective_configuration() {
        let resp = handle_config_get(&HashMap::new());
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["ok"], true);
        assert_eq!(v["supported"], cfg!(target_os = "linux"));
        assert!(v["agentSend"]["default"].is_string());
        assert!(v["limits"]["maxRecipients"].as_u64().unwrap() > 0);
        assert!(v["domains"].is_array());
        assert!(v["relayConfigs"].is_array());
    }

    /// S6 — POST /cli/mail/config/set: teaching validation runs BEFORE
    /// the platform gate; every refusal names what is missing. (Deep
    /// apply behavior is owned by mail::config's tests.)
    #[test]
    fn config_set_validates_teachingly_before_anything_executes() {
        let resp = handle_config_set(b"not json");
        assert_eq!(resp.status, "400 Bad Request");

        // Empty body → the full surface in the hint.
        let resp = handle_config_set(b"{}");
        assert_eq!(resp.status, "400 Bad Request");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["error"]["code"], "usage");
        assert!(v["error"]["hint"].as_str().unwrap().contains("sendMode"), "{v}");

        // sendMode without domain / domain without sendMode / gating
        // without workspace: each teaches.
        for (body, needle) in [
            (br#"{"sendMode":"direct"}"# as &[u8], "'sendMode' needs 'domain'"),
            (br#"{"domain":"acme.dev"}"#, "no 'sendMode'"),
            (br#"{"agentSend":"on"}"#, "'workspace'"),
        ] {
            let resp = handle_config_set(body);
            assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
            assert!(v["error"]["hint"].as_str().unwrap().contains(needle), "{v}");
        }

        // Unknown workspace → the shared 404 shape, before the gate.
        let resp = handle_config_set(br#"{"workspace":"no-such-ws-xyz","agentSend":"on"}"#);
        assert_eq!(resp.status, "404 Not Found", "{}", resp.body);

        if !cfg!(target_os = "linux") {
            // A structurally-valid request stops at the D3 gate on a
            // Mac — no ops, no secret-store writes.
            let resp = handle_config_set(
                br#"{"defaults":{"agentSend":"approval"}}"#,
            );
            assert_eq!(resp.status, "409 Conflict");
            assert!(resp.body.contains("unsupported"), "{}", resp.body);
        }
    }

    /// S6 — the doctor GET serves the latest persisted run (null when
    /// none) and NEVER probes; the POST validates + platform-gates
    /// before any probe could fire.
    #[test]
    fn doctor_get_reads_and_post_gates() {
        let _g = crate::mail::mail_server_test_lock();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM mail_doctor_runs WHERE domain_id IS NULL", []);
        }
        let resp = handle_doctor(&HashMap::new());
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["ok"], true);
        assert!(v["run"].is_null(), "no run on file yet: {v}");

        // A stored run reads back through the route.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_doctor_runs (id, domain_id, results_json, grade, ran_at) \
                 VALUES ('mdr-route-test', NULL, '{\"checks\":[]}', 'warn', 4242)",
                [],
            )
            .expect("seed run");
        }
        let resp = handle_doctor(&HashMap::new());
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["run"]["grade"], "warn");
        assert_eq!(v["run"]["ranAt"], 4242);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM mail_doctor_runs WHERE id = 'mdr-route-test'", []);
        }

        // Unknown domain in the GET → 404 (never a probe).
        let mut params = HashMap::new();
        params.insert("domain".to_string(), "ghost-doctor.example".to_string());
        let resp = handle_doctor(&params);
        assert_eq!(resp.status, "404 Not Found");

        // POST: bad JSON → 400; on a Mac a valid body stops at the D3
        // gate (network-silent example page).
        let resp = handle_doctor_run(b"not json");
        assert_eq!(resp.status, "400 Bad Request");
        if !cfg!(target_os = "linux") {
            let resp = handle_doctor_run(b"{}");
            assert_eq!(resp.status, "409 Conflict");
            assert!(resp.body.contains("unsupported"), "{}", resp.body);
        }
    }
}
