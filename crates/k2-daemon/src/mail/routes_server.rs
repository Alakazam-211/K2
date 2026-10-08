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

/// The structured stop when the daemon user cannot `sudo -n` the mail
/// helper: 409 `mail_helper_missing`, hint = the exact root command.
pub(crate) fn helper_unavailable_response(
    state: crate::mail::helper::HelperState,
) -> Option<CliResponse> {
    let hint = crate::mail::helper::unavailable_message(state)?;
    Some(CliResponse {
        status: "409 Conflict",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": "mail_helper_missing", "hint": hint },
            "helper": state.as_str(),
        })
        .to_string(),
    })
}

/// `helper` / `helperFix` keys for status and enable bodies.
pub(crate) fn helper_status_fields(
    state: crate::mail::helper::HelperState,
    body: &mut serde_json::Value,
) {
    body["helper"] = serde_json::json!(state.as_str());
    if let Some(fix) = crate::mail::helper::unavailable_message(state) {
        body["helperFix"] = serde_json::json!(fix);
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
///   "lastError": <last_error | "acme: <reason>" when the cert is not
///                 issued and the last ACME task failed | null>,
///   "cert": {host, names (served SANs), state, …, acme: {mode,
///            orderNames, locked, lastTask}, note?},
///   "outbound": {ipStrategy, ipStrategySetBy, dane[], daneSummary,
///                daneAutoOff, dnssec, pendingReload?},
///   "helper": "installed" | "missing" | "not allowed by sudoers",  // Linux only
///   "helperFix": <root install command — only when helper != installed>,
///   "upgradeAvailable": <installed version present and != pinnedVersion>,
///   "upgrade": <last `k2 hostmail upgrade` record | null>,
///   "backup": <S8 B1 mode + churn observer — mail::backup::status_json>,
///   "health": <live verdict — only present with ?health=1 on Linux> }
/// ```
///
/// `state` comes from the `mail_server` singleton row; NO row =
/// `"not-installed"` (the 0072 contract). `?health=1` additionally
/// runs the live systemd+API health check (persisting transitions +
/// raising the standard event) before reading. Without `?health=1`
/// the `health` key is omitted (never `"health": null`). The renderer gates the
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
    let installed = row.is_some();
    let (sqlite_state, version, hostname, port_plan, progress, sqlite_last_error) = match row {
        Some((status, installed, hostname, plan, progress, last_error)) => {
            (status, installed, hostname, plan, progress, last_error)
        }
        None => ("not-installed".to_string(), None, None, None, None, None),
    };
    // Linux (and tests with a systemd seam): SQLite is a cache. `state`
    // is never `running` unless the unit is active (H1); `consistent` is
    // false when the row and systemd disagree (H2).
    let (state, consistent, last_error) = match supervisor::systemd_ground_truth() {
        Some(unit) => {
            let rec = supervisor::reconcile_reported_status(
                &sqlite_state,
                sqlite_last_error.as_deref(),
                &unit,
            );
            (rec.state, rec.consistent, rec.last_error)
        }
        None => (sqlite_state, true, sqlite_last_error),
    };
    let enable_progress = progress
        .and_then(|p| serde_json::from_str::<serde_json::Value>(&p).ok())
        .unwrap_or(serde_json::Value::Null);
    let mut body = serde_json::json!({
        // Envelope: the status READ succeeded. Never the systemd verdict
        // — `k2`'s generic caller treats ok=false as a malformed reply.
        "ok": true,
        // Does the SQLite row agree with `systemctl is-active stalwart`?
        "consistent": consistent,
        "supported": mail_supported(),
        "state": state,
        "version": version,
        "pinnedVersion": STALWART_PINNED_VERSION,
        "hostname": hostname,
        "portPlan": port_plan,
        "enableProgress": enable_progress,
        "lastError": last_error,
        "cert": supervisor::tls_cert_status(hostname.as_deref()),
        // CAL44: the extra mail-family names (autoconfig, autodiscover,
        // mta-sts, ua-auto-config per hosted domain) — last DNS answer +
        // per-name cert state from the box store. No network here.
        "extraNames": crate::mail::cert_names::status_json(installed),
        // Calendars S2 (CAL26): {enabled, files, policy, reachable, url,
        // reason}. reachable = tls-alpn OR the Caddy mail Host site.
        "calendar": crate::mail::dav::status_block(
            installed,
            hostname.as_deref(),
            port_plan.as_deref(),
        ),
        // Calendars S1 (CAL15/CAL56): installed != pinned, never semver.
        "upgradeAvailable": crate::mail::upgrade::upgrade_available(version.as_deref()),
        // The last `k2 hostmail upgrade` record (null = never run). A
        // `running` record no thread owns reads `interrupted`.
        "upgrade": crate::mail::upgrade::status_upgrade_json(
            supervisor::upgrade_running().load(std::sync::atomic::Ordering::SeqCst),
        ),
    });
    // L14: omit `health` unless `?health=1` actually ran the probe.
    // Never emit `"health": null` — clients treat a missing key as
    // "not probed", not a failed probe.
    if let Some(health) = health {
        body["health"] = health;
    }
    // K2-issued certificate renewal (domains::renew): the mail host's
    // renewal state + the background renewer's last scan. Local reads.
    body["certRenewal"] = crate::domains::renew::mail_status_json(hostname.as_deref());
    // S8 B1: the box's backup choice + churn observer (local reads only).
    body["backup"] = crate::mail::backup::status_json(installed);
    // Who owns the mail certificate (mail::cert_owner — the one K2-issuer
    // definition): k2 | stalwart-acme | unknown. Local reads only.
    if let Some(cert) = body.get_mut("cert").and_then(|c| c.as_object_mut()) {
        let owner = match hostname.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
            Some(h) => crate::mail::cert_owner::mail_cert_owner(h),
            None => crate::mail::cert_owner::CertOwner::Unknown,
        };
        cert.insert("owner".into(), serde_json::json!(owner.as_str()));
    }
    // 0.45.1 field fixes: `outbound` (mx route, DANE, who set them, the
    // doctor's DNSSEC verdict), `cert.acme` (mode, real order names,
    // locked, last ACME task) + the next-renewal note, and the `lastError`
    // fallback to the last failed ACME task. Registry reads bounded to
    // 2 s and cached 60 s (Settings polls this).
    if installed {
        crate::mail::defaults::fill_status(&mut body, hostname.as_deref());
    }
    // Linux only: the root door enable / cert restart / boot reconcile
    // need. Cached 30 s (each probe is a `sudo -n -l`).
    if mail_supported() {
        helper_status_fields(supervisor::mail_helper_state_cached(), &mut body);
    }
    CliResponse::ok_json(body.to_string())
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
#[cfg(test)]
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

    // Idempotency short-circuit only when systemd says the unit is
    // active. A SQLite `running` row with an inactive unit is a stale
    // cache — alreadyEnabled is illegal then (H3).
    if supervisor::is_already_enabled() {
        supervisor::stamp_enable_noop();
        let mut body = serde_json::json!({
            "ok": true,
            "state": "running",
            "alreadyEnabled": true,
            "hint": "already enabled (no-op)",
        });
        if let Some(p) = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT enable_progress_json FROM mail_server WHERE id = 1",
                [],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        } {
            body["enableProgress"] = p;
        }
        if let Some(hint) = reapply_skin_door_after_mail_enable(daemon_port) {
            body["hint"] = serde_json::json!(hint);
        }
        return CliResponse::ok_json(body.to_string());
    }
    if !mail_supported() {
        return unsupported();
    }
    // Before the latch, preflight, row, or download: without the root
    // helper every install step fails (the "Permission denied (os error
    // 13)" on /usr/local/bin/stalwart was a pre-helper daemon writing it
    // as k2). Stop with the one root command instead.
    let helper_state = supervisor::mail_helper_state();
    if let Some(resp) = helper_unavailable_response(helper_state) {
        return resp;
    }
    if let Some(resp) = upgrade_running_response() {
        return resp;
    }
    if !supervisor::try_begin_enable() {
        return err_json(
            "409 Conflict",
            "enable_in_progress",
            "an enable run is already in progress — poll /cli/mail/status".to_string(),
        );
    }

    // Fresh preflight, synchronously: failures return the report and
    // nothing installs (§5.1 hard stops). Resume after a partial
    // enable (config.json, ports already ours) skips this — :25 in
    // use is our Stalwart, not a foreign MTA (iascm fcc320d9).
    let (port_plan, preflight_json) = if supervisor::is_enable_resume() {
        let plan = supervisor::stored_port_plan().unwrap_or_else(|| "tls-alpn".to_string());
        (plan, serde_json::json!({ "ok": true, "resumed": true }))
    } else {
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
        let plan = report.port_plan.unwrap_or("http-01").to_string();
        (plan, report.to_json())
    };
    let artifact = match supervisor::artifact_for_arch(std::env::consts::ARCH) {
        Ok(a) => a,
        Err(e) => {
            supervisor::end_enable();
            return CliResponse::internal_error(e);
        }
    };

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
                // 0.45.1: cache the box IPv4 (the mail host's A row) and
                // run the field-fix reconcile once the latch is released
                // (it pins the spam-rules URL, waits for the first rules
                // download, then repairs; mx route; DANE).
                let _ = crate::mail::cert_names::refresh_public_ipv4();
                crate::mail::defaults::spawn_after("enable");
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
            "helper": helper_state.as_str(),
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
    if let Some(resp) = upgrade_running_response() {
        return resp;
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

/// POST `/cli/mail/server/rotate-admin` — L12: rotate leftover bootstrap
/// principal `admin` and the provisioned admin password. For boxes
/// already past enable (noir ticket 4bdbb533). Does not wipe the store,
/// SIGTERM, or re-enable. enable no-op is not rotate; disable is not
/// a wipe.
pub fn handle_server_rotate_admin(_body: &[u8]) -> CliResponse {
    if !mail_supported() {
        return unsupported();
    }
    match supervisor::rotate_leftover_admins_live() {
        Ok(report) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "recoveryPrincipal": report.recovery_principal,
                "provisionedAdmin": report.provisioned_admin,
                "hint": "rotated leftover recovery principal `admin` when present; provisioned adminUsername is skipped if that Stalwart account does not exist — store kept",
            })
            .to_string(),
        ),
        Err(e) => {
            let code = if e.contains("not installed")
                || e.contains("not Linux")
                || e.contains("start it")
                || e.contains("secret ref missing")
                || e.contains("before rotate-admin")
                || e.contains("no api_url")
                || e.contains("no API key")
            {
                "not_ready"
            } else {
                "engine"
            };
            let status = if code == "not_ready" {
                "409 Conflict"
            } else {
                "502 Bad Gateway"
            };
            err_json(status, code, e)
        }
    }
}

/// POST `/cli/mail/cert/renew` — L7 / C14: thin alias of `/cli/certs/renew`
/// for the attached mail hostname. Never disable+enable.
pub fn handle_cert_renew(_body: &[u8]) -> CliResponse {
    let hostname = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT hostname FROM mail_server WHERE id = 1", [], |r| {
            r.get::<_, Option<String>>(0)
        })
        .ok()
        .flatten()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    };
    let Some(hostname) = hostname else {
        return err_json(
            "409 Conflict",
            "not_ready",
            "the mail server is not installed — enable it before cert renew".to_string(),
        );
    };
    // One issuer: custom-domain ACME when the mail hostname is attached.
    let attached = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::get_name(&conn, &hostname)
            .ok()
            .flatten()
            .is_some()
    };
    if attached {
        // The custom-domain issuer plants the cert and loads it with
        // ReloadTlsCertificates (no mail helper needed); only if that
        // fails does it restart Stalwart (the helper, else `sudo -n
        // systemctl restart stalwart`), and no open door is a loud error.
        // A K2 plant also switches Stalwart ACME to Manual (cert_owner).
        let mut params = std::collections::HashMap::new();
        params.insert("hostname".into(), hostname);
        return crate::domains::routes::handle_renew(&params);
    }
    let client = match crate::mail::domains::engine_from_db() {
        Ok((c, _)) => c,
        Err(e) => {
            return err_json(
                "409 Conflict",
                "not_ready",
                format!("cannot retry ACME while the mail server is down: {e}"),
            )
        }
    };
    match client.renew_acme_for_mail_hostname(&hostname) {
        Ok(task_id) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "hostname": hostname,
                "taskId": task_id,
                "hint": format!("ACME renewal queued for {hostname}"),
            })
            .to_string(),
        ),
        Err(e) => err_json("502 Bad Gateway", "engine", e),
    }
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
    if let Some(resp) = upgrade_running_response() {
        return resp;
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

/// 409 `upgrade_in_progress` while `k2 hostmail upgrade` runs (it has
/// Stalwart stopped for the snapshot) — disable / uninstall / enable.
pub(crate) fn upgrade_running_response() -> Option<CliResponse> {
    if !supervisor::upgrade_running().load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    Some(err_json(
        "409 Conflict",
        "upgrade_in_progress",
        supervisor::UPGRADE_RUNNING_HINT.to_string(),
    ))
}

/// The parsed POST `/cli/mail/server/upgrade` body. Unknown keys and
/// non-boolean flags are a 400 (a typo like `dry_run` must never start a
/// real upgrade).
pub(crate) fn parse_upgrade_body(body: &[u8]) -> Result<crate::mail::upgrade::UpgradeRequest, String> {
    let trimmed = std::str::from_utf8(body).map(str::trim).unwrap_or("");
    let parsed: serde_json::Value = if trimmed.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_slice(body).map_err(|e| format!("invalid JSON body: {e}"))?
    };
    let Some(obj) = parsed.as_object() else {
        return Err("body must be a JSON object".into());
    };
    let mut req = crate::mail::upgrade::UpgradeRequest::default();
    for (k, v) in obj {
        let flag = v
            .as_bool()
            .ok_or_else(|| format!("'{k}' must be true or false"))?;
        match k.as_str() {
            "dryRun" => req.dry_run = flag,
            "acknowledgeFailed" => req.acknowledge_failed = flag,
            other => {
                return Err(format!(
                    "unknown field '{other}' — the body is {{\"dryRun\": bool}} or \
                     {{\"acknowledgeFailed\": true}}"
                ))
            }
        }
    }
    if req.dry_run && req.acknowledge_failed {
        return Err("dryRun and acknowledgeFailed are separate calls".into());
    }
    Ok(req)
}

/// POST `/cli/mail/server/upgrade` — calendars S1 (CAL7/CAL14/CAL15):
/// the explicit Stalwart upgrade to the pin, owner/admin only
/// (`is_owner_level_mutation` + exact agent DENY + ROUTES Admin).
/// Body `{}` (run), `{"dryRun":true}` (plan only, never an effect) or
/// `{"acknowledgeFailed":true}` (clear a failed-rollback block). A run
/// preflights synchronously, then continues in the background; poll GET
/// `/cli/mail/status` → `upgrade`. Never called by boot, a daemon update
/// or enable.
pub fn handle_server_upgrade(body: &[u8]) -> CliResponse {
    let req = match parse_upgrade_body(body) {
        Ok(r) => r,
        Err(e) => return err_json("400 Bad Request", "usage", e),
    };
    if !mail_supported() {
        return unsupported();
    }
    use crate::mail::upgrade::UpgradeStart;
    match supervisor::upgrade(req) {
        UpgradeStart::Noop(v)
        | UpgradeStart::DryRun(v)
        | UpgradeStart::Started(v)
        | UpgradeStart::Acknowledged(v) => CliResponse::ok_json(v.to_string()),
        UpgradeStart::Refused { code, body, .. } => CliResponse {
            status: if code == "spawn_failed" {
                "500 Internal Server Error"
            } else {
                "409 Conflict"
            },
            content_type: "application/json",
            body: body.to_string(),
        },
    }
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
        // no_run is 409 (not 404): the domain is hosted; the missing
        // resource is a persisted run. 404 stays not_found for unknown
        // domains. Hint names POST /cli/mail/doctor.
        DocError::NoRun(h) => err_json("409 Conflict", "no_run", h),
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
    quota_bytes: Option<i64>,
    quota_messages: Option<i64>,
    /// The owner's per-workspace "always BCC" policy: a comma-separated
    /// string or a list; `""` / `[]` clears it. OWNER/ADMIN ONLY — never
    /// a scoped agent passport, mail-manage included.
    always_bcc: Option<serde_json::Value>,
    defaults: Option<ConfigDefaultsBody>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ConfigDefaultsBody {
    agent_send: Option<String>,
    address_cap: Option<i64>,
    quota_bytes: Option<i64>,
    quota_messages: Option<i64>,
}

const CONFIG_SET_SURFACE: &str =
    "nothing to set. The surface: {domain + sendMode [+ relayConfigId]} · {relay: \
     {id?, host, port, username, password|secretRef, tlsKind?, spfInclude?}} · \
     {deleteRelayConfig} · {workspace + agentSend|addressCap|quotaBytes|quotaMessages|\
     alwaysBcc} · {defaults: {agentSend?, addressCap?, quotaBytes?, quotaMessages?}}";

/// Parse the `alwaysBcc` value: `"a@x, b@y"` (comma-separated; `""`
/// clears) or `["a@x", "b@y"]` (`[]` clears). Anything else is usage.
fn parse_always_bcc(v: &serde_json::Value) -> Result<Vec<String>, CliResponse> {
    let bad = || {
        err_json(
            "400 Bad Request",
            "usage",
            "'alwaysBcc' must be a comma-separated address string or a list of addresses \
             ('' or [] clears the policy)"
                .to_string(),
        )
    };
    let items: Vec<String> = match v {
        serde_json::Value::String(s) => s.split(',').map(str::to_string).collect(),
        serde_json::Value::Array(a) => a
            .iter()
            .map(|i| i.as_str().map(str::to_string).ok_or_else(bad))
            .collect::<Result<_, _>>()?,
        _ => return Err(bad()),
    };
    Ok(items
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

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
    if (b.agent_send.is_some()
        || b.address_cap.is_some()
        || b.quota_bytes.is_some()
        || b.quota_messages.is_some())
        && !wants_workspace
    {
        return err_json(
            "400 Bad Request",
            "usage",
            "'agentSend'/'addressCap'/'quotaBytes'/'quotaMessages' need 'workspace' — \
             or wrap them in 'defaults' for the global default"
                .to_string(),
        );
    }
    // The always-BCC policy is per-workspace only (no global default).
    let always_bcc: Option<Vec<String>> = match b.always_bcc.as_ref() {
        None => None,
        Some(v) => match parse_always_bcc(v) {
            Ok(list) => Some(list),
            Err(resp) => return resp,
        },
    };
    if always_bcc.is_some() && !wants_workspace {
        return err_json(
            "400 Bad Request",
            "usage",
            "'alwaysBcc' needs 'workspace' — the policy is set per workspace".to_string(),
        );
    }
    let wants_gating = b.agent_send.is_some()
        || b.address_cap.is_some()
        || b.quota_bytes.is_some()
        || b.quota_messages.is_some();
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

    // 0.45.0 (see mail::agent_creds): an IT agent (mail-manage) is as
    // capable as the owner here — sendMode (domain), addressCap/quota and
    // agent mail POLICY (agentSend, always-BCC) for any workspace — EXCEPT
    // agentSend/always-BCC on its OWN workspace (`owner_only`: an agent
    // can't loosen the rules on its own sends). Global defaults and relay
    // config stay owner-or-admin: a default can loosen its own workspace.
    let caller = crate::mail::agent_creds::mail_caller();
    if caller.is_agent() {
        if let crate::mail::agent_creds::MailCaller::Agent { .. } = caller {
            return crate::mail::agent_creds::refuse_needs_mail_manage("change mail config");
        }
        if b.defaults.is_some() || b.relay.is_some() || b.delete_relay_config.is_some() {
            return err_json(
                "403 Forbidden",
                "owner_only",
                "global defaults and relay config stay owner-or-admin — mail-manage may set \
                 sendMode (domain) and agentSend/alwaysBcc/addressCap/quota (workspace). Ask \
                 your human."
                    .to_string(),
            );
        }
        if always_bcc.is_some() || b.agent_send.is_some() {
            if let Some(ref path) = workspace_path {
                if let Err(resp) = crate::mail::agent_creds::policy_gate(&caller, path) {
                    return resp;
                }
            }
        }
    }

    // D3: nothing mail-shaped executes off-Linux — except a call that
    // ONLY sets the always-BCC policy: linked/BYO sends run on every
    // platform and the policy rides them too (a pure DB write).
    let only_always_bcc = always_bcc.is_some()
        && !wants_gating
        && b.relay.is_none()
        && b.delete_relay_config.is_none()
        && !wants_send_mode
        && b.defaults.is_none();
    if !mail_supported() && !only_always_bcc {
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
    // 4a. The owner's always-BCC policy (owner/admin — checked above).
    if let (Some(path), Some(list)) = (workspace_path.as_deref(), always_bcc.as_deref()) {
        match config::set_workspace_always_bcc(path, list) {
            Ok(v) => {
                applied.insert("alwaysBcc".to_string(), v);
            }
            Err(e) => return cfg_error_response(e),
        }
    }
    // 4. Per-workspace gating (a bare `workspace` with nothing else still
    // answers the "nothing to set" teaching error).
    if let Some(path) = workspace_path.as_deref().filter(|_| wants_gating || always_bcc.is_none()) {
        match config::set_workspace_gating(
            path,
            b.agent_send.as_deref(),
            b.address_cap,
            b.quota_bytes,
            b.quota_messages,
        ) {
            Ok(v) => {
                applied.insert("workspace".to_string(), v);
            }
            Err(e) => return cfg_error_response(e),
        }
    }
    // 5. Global defaults.
    if let Some(d) = &b.defaults {
        match config::set_global_defaults(
            d.agent_send.as_deref(),
            d.address_cap,
            d.quota_bytes,
            d.quota_messages,
        ) {
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

/// GET `/cli/mail/doctor[?domain=<d>]` — S6: the LATEST persisted run.
/// Bare GET (no domain): `run: null` when this box has never POSTed.
/// `?domain=` on a hosted domain with no per-domain row is `no_run`
/// (409), not `ok:true` + `run:null`. Read-only — GET is not a run;
/// `POST /cli/mail/doctor` runs probes.
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
        assert!(
            v.get("health").is_none(),
            "health omitted unless ?health=1, got {v}"
        );
        assert_eq!(v["upgradeAvailable"], false, "nothing installed: {v}");
        assert!(v["upgrade"].is_null(), "{v}");

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
        // CAL15: the pin moved to 0.16.20, so this box has an upgrade.
        assert_eq!(v["pinnedVersion"], "0.16.20");
        assert_eq!(v["upgradeAvailable"], true, "{v}");
        assert!(v["upgrade"].is_null(), "never upgraded: {v}");
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
        // On the pin: no upgrade available; the last run's record shows.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "UPDATE mail_server SET installed_version = ?1, \
                 upgrade_progress_json = '{\"state\":\"succeeded\",\"from\":\"0.16.10\"}' \
                 WHERE id = 1",
                rusqlite::params![STALWART_PINNED_VERSION],
            )
            .expect("update row");
        }
        let v: serde_json::Value =
            serde_json::from_str(&handle_status(&HashMap::new()).body).expect("json");
        assert_eq!(v["upgradeAvailable"], false, "{v}");
        assert_eq!(v["upgrade"]["state"], "succeeded", "{v}");
        clean_row();
    }

    /// Calendars S1: the upgrade body is strict (a typo never starts a
    /// real upgrade), then the platform gate; deeper behavior is the
    /// fake-engine suite in `mail::upgrade`.
    #[test]
    fn upgrade_route_validates_the_body_then_gates_on_platform() {
        for bad in [
            &br#"{"dry_run":true}"#[..],
            br#"{"dryRun":"yes"}"#,
            br#"{"dryRun":true,"acknowledgeFailed":true}"#,
            br#"[]"#,
            br#"not json"#,
        ] {
            let r = handle_server_upgrade(bad);
            assert_eq!(r.status, "400 Bad Request", "{}: {}", String::from_utf8_lossy(bad), r.body);
            assert!(r.body.contains("usage"), "{}", r.body);
        }
        assert_eq!(
            parse_upgrade_body(b"").expect("empty = run"),
            crate::mail::upgrade::UpgradeRequest::default()
        );
        assert!(parse_upgrade_body(br#"{"dryRun":true}"#).expect("dry").dry_run);
        assert!(
            parse_upgrade_body(br#"{"acknowledgeFailed":true}"#)
                .expect("ack")
                .acknowledge_failed
        );
        if !cfg!(target_os = "linux") {
            for body in [&b"{}"[..], br#"{"dryRun":true}"#] {
                let r = handle_server_upgrade(body);
                assert_eq!(r.status, "409 Conflict", "{}", r.body);
                assert!(r.body.contains("unsupported"), "{}", r.body);
            }
        }
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
        assert_eq!(checks.len(), 11);
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

    #[test]
    fn helper_missing_response_names_the_fix_and_status_fields() {
        use crate::mail::helper::{install_command, HelperState};
        assert!(helper_unavailable_response(HelperState::Installed).is_none());
        for (state, word) in [
            (HelperState::Missing, "missing"),
            (HelperState::NotAllowed, "not allowed by sudoers"),
        ] {
            let resp = helper_unavailable_response(state).expect("stop");
            assert_eq!(resp.status, "409 Conflict");
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
            assert_eq!(v["ok"], false);
            assert_eq!(v["error"]["code"], "mail_helper_missing");
            assert_eq!(v["helper"], word);
            let hint = v["error"]["hint"].as_str().expect("hint");
            assert!(hint.contains("run as root: "), "{hint}");
            assert!(hint.ends_with(&install_command()), "{hint}");

            let mut body = serde_json::json!({ "ok": true });
            helper_status_fields(state, &mut body);
            assert_eq!(body["helper"], word);
            assert_eq!(body["helperFix"].as_str(), Some(hint));
        }
        let mut body = serde_json::json!({ "ok": true });
        helper_status_fields(HelperState::Installed, &mut body);
        assert_eq!(body["helper"], "installed");
        assert!(body.get("helperFix").is_none(), "{body}");

        // The probe seam routes use: default installed, injectable.
        assert_eq!(supervisor::mail_helper_state(), HelperState::Installed);
        supervisor::set_test_helper_state(Some(HelperState::NotAllowed));
        assert_eq!(supervisor::mail_helper_state(), HelperState::NotAllowed);
        assert_eq!(
            supervisor::mail_helper_state_cached(),
            HelperState::NotAllowed
        );
        supervisor::set_test_helper_state(None);
        assert_eq!(supervisor::mail_helper_state(), HelperState::Installed);
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

    /// Enable short-circuits idempotently only when systemd says active (H3).
    #[test]
    fn enable_is_idempotent_when_already_running() {
        let _g = crate::mail::mail_server_test_lock();
        let _unit = supervisor::with_test_unit_state("active");
        supervisor::set_test_store_ready(Some(true));
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
        // The restart mark lives in the row's enable_progress_json, so
        // it must be written AFTER the row exists (an UPDATE on no row
        // is a silent no-op). fcc320d9 made `restart` part of
        // is_already_enabled().
        supervisor::mark_step_for_test("restart");
        let resp = handle_server_enable(br#"{"hostname":"mail.acme.dev"}"#);
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["ok"], true);
        assert_eq!(v["alreadyEnabled"], true);
        assert_eq!(v["state"], "running");
        assert!(
            v["enableProgress"]["lastNoopAt"].as_i64().unwrap_or(0) > 0,
            "C29 alreadyEnabled stamps lastNoopAt: {}",
            v
        );
        // Caddy apply must not swallow Enable success (ok stays true).
        assert_ne!(v["ok"], false);
        supervisor::set_test_store_ready(None);
        clean_row();
    }

    /// L2: a healthy alreadyEnabled install is NOT the hostname
    /// retarget path — even when the POST names a different host.
    #[test]
    fn enable_already_enabled_does_not_retarget_hostname() {
        let _g = crate::mail::mail_server_test_lock();
        let _unit = supervisor::with_test_unit_state("active");
        supervisor::set_test_store_ready(Some(true));
        clean_row();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, hostname, updated_at) \
                 VALUES (1, 'running', ?1, 'mail.old.dev', 100)",
                rusqlite::params![STALWART_PINNED_VERSION],
            )
            .expect("seed row");
        }
        // After the row exists — see enable_is_idempotent_when_already_running.
        supervisor::mark_step_for_test("restart");
        let resp = handle_server_enable(br#"{"hostname":"mail.new.dev"}"#);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["alreadyEnabled"], true, "{}", resp.body);
        let still: String = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row("SELECT hostname FROM mail_server WHERE id = 1", [], |r| {
                r.get(0)
            })
            .expect("hostname")
        };
        assert_eq!(still, "mail.old.dev", "alreadyEnabled must not rewrite sqlite hostname");
        supervisor::set_test_store_ready(None);
        clean_row();
    }

    #[test]
    fn cert_renew_is_not_ready_without_a_mail_hostname() {
        let _g = crate::mail::mail_server_test_lock();
        clean_row();
        let resp = handle_cert_renew(b"{}");
        assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "not_ready");
        assert!(
            !resp.body.contains("alreadyEnabled"),
            "renew must not go through enable: {}",
            resp.body
        );
        clean_row();
    }

    #[test]
    fn rotate_admin_is_not_ready_without_install_and_is_not_enable() {
        let _g = crate::mail::mail_server_test_lock();
        clean_row();
        let resp = handle_server_rotate_admin(b"{}");
        assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["ok"], false);
        let code = v["error"]["code"].as_str().unwrap_or("");
        assert!(
            code == "not_ready" || code == "unsupported",
            "expected not_ready or unsupported, got {code}: {}",
            resp.body
        );
        assert!(
            !resp.body.contains("alreadyEnabled"),
            "rotate-admin is not enable no-op: {}",
            resp.body
        );
        clean_row();
    }

    #[test]
    fn enable_is_not_already_enabled_when_unit_inactive() {
        let _g = crate::mail::mail_server_test_lock();
        let _unit = supervisor::with_test_unit_state("inactive");
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
        assert!(
            !supervisor::is_already_enabled(),
            "H3: SQLite running + inactive unit is not alreadyEnabled"
        );
        // Do not POST enable on Linux here — past the short-circuit the
        // route would run real preflight. Non-Linux stops at unsupported.
        if !cfg!(target_os = "linux") {
            let resp = handle_server_enable(br#"{"hostname":"mail.acme.dev"}"#);
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
            assert_ne!(v.get("alreadyEnabled"), Some(&serde_json::json!(true)), "{}", resp.body);
            assert_eq!(resp.status, "409 Conflict");
            assert_eq!(v["error"]["code"], "unsupported");
        }
        clean_row();
    }

    #[test]
    fn status_is_not_running_when_unit_inactive() {
        let _g = crate::mail::mail_server_test_lock();
        let _unit = supervisor::with_test_unit_state("inactive");
        clean_row();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, hostname, last_error, updated_at) \
                 VALUES (1, 'running', ?1, 'mail.acme.dev', \
                 'systemd reports the stalwart unit is ''inactive''', 100)",
                rusqlite::params![STALWART_PINNED_VERSION],
            )
            .expect("seed row");
        }
        let resp = handle_status(&HashMap::new());
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["state"], "stopped", "H1: state is not running unless unit active: {v}");
        assert_eq!(v["ok"], true, "envelope ok: the status read succeeded: {v}");
        assert_eq!(v["consistent"], false, "H2: running row + inactive unit disagree: {v}");
        assert_eq!(
            v["lastError"],
            "systemd reports the stalwart unit is 'inactive'",
            "{v}"
        );
        assert!(v.get("cert").is_some(), "C24 status shows cert without --health: {v}");
        clean_row();
    }

    /// `cert.owner` is the one K2-issuer definition: stalwart-acme while
    /// the mail host is not attached, unknown once attached but not
    /// planted, k2 with a K2-planted Let's Encrypt cert on record.
    #[test]
    fn status_cert_owner_follows_the_k2_issuer_definition() {
        let _g = crate::mail::mail_server_test_lock();
        let _home = crate::test_support::TempHome::new();
        let _unit = supervisor::with_test_unit_state("active");
        clean_row();
        let host = "mail.owner-status.test";
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, hostname, port_plan, updated_at) \
                 VALUES (1, 'running', ?1, ?2, 'tls-alpn', 100)",
                rusqlite::params![STALWART_PINNED_VERSION, host],
            )
            .expect("seed row");
        }
        let owner = || {
            let v: serde_json::Value =
                serde_json::from_str(&handle_status(&HashMap::new()).body).expect("json");
            v["cert"]["owner"].as_str().expect("cert.owner").to_string()
        };
        assert_eq!(owner(), "stalwart-acme");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::domains::upsert_binding(&conn, "owner-status.test", None, false).unwrap();
            k2_core::domains::upsert_name(&conn, host, "owner-status.test", k2_core::domains::ROLE_MAIL)
                .unwrap();
        }
        assert_eq!(owner(), "unknown");
        let pem = crate::domains::acme::test_le_pem(host);
        crate::domains::store::install(host, &pem.chain_pem, &pem.key_pem).expect("install");
        crate::domains::renew::update(|f| {
            f.names.entry(host.into()).or_default().stalwart_cert_id = Some("c1".into())
        })
        .expect("renewal.json");
        assert_eq!(owner(), "k2");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = k2_core::domains::remove_binding(&conn, "owner-status.test");
        }
        clean_row();
    }

    /// 0.45.1 A2: no supervisor error + cert `missing` + a Failed
    /// AcmeRenewal → `lastError` = `acme: <reason>` (computed at read time,
    /// the health loop can't wipe it), `cert.acme` carries the real order
    /// names; a healthy cert → `lastError` null and `names` = served SANs;
    /// a wider valid cert under a lock → the next-renewal note (HF9).
    #[test]
    fn status_last_error_falls_back_to_the_failed_acme_task() {
        use crate::mail::defaults::{clear_test_status_reads, set_test_status_reads, AcmeTask};
        use crate::mail::jmap::CertManagement;
        let _g = crate::mail::mail_server_test_lock();
        let _clean = crate::mail::MailServerRowCleanup;
        let _home = crate::test_support::TempHome::new();
        let _unit = supervisor::with_test_unit_state("active");
        clean_row();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, installed_version, hostname, \
                 port_plan, updated_at) VALUES (1, 'running', ?1, '0.16.10', 'mail.example.com', \
                 'tls-alpn', 100)",
                rusqlite::params![STALWART_PINNED_VERSION],
            )
            .expect("seed row");
        }
        let failed = AcmeTask {
            id: "t1".into(),
            domain_id: "dom-1".into(),
            state: "Failed".into(),
            reason: Some("urn:ietf:params:acme:error:connection: refused".into()),
            at: Some("2026-10-07T20:00:00Z".into()),
            due: None,
        };
        let locked = CertManagement::Automatic {
            acme_provider_id: "p".into(),
            subject_alternative_names: vec!["mail.example.com".into()],
        };
        set_test_status_reads(
            Ok(vec![serde_json::json!({ "id": "r", "name": "mx", "@type": "Mx", "ipLookupStrategy": "v4ThenV6" })]),
            Ok(vec![]),
            Ok(Some(("dom-1".into(), "example.com".into(), locked))),
            Ok(vec![failed]),
        );
        let status = || -> serde_json::Value {
            serde_json::from_str(&handle_status(&HashMap::new()).body).expect("json")
        };
        let v = status();
        assert_eq!(v["cert"]["state"], "missing", "{v}");
        assert_eq!(v["cert"]["names"], serde_json::json!([]), "{v}");
        assert_eq!(v["cert"]["host"], "mail.example.com", "{v}");
        assert!(
            v["lastError"].as_str().expect("lastError").starts_with("acme: urn:ietf:params:acme:error:connection"),
            "{v}"
        );
        assert_eq!(v["cert"]["acme"]["orderNames"], serde_json::json!(["mail.example.com"]), "{v}");
        assert_eq!(v["cert"]["acme"]["lastTask"]["state"], "Failed", "{v}");
        assert_eq!(v["outbound"]["ipStrategy"], "v4ThenV6", "{v}");
        {
            let _cert = supervisor::with_test_issued_cert(&["mail.example.com", "mta-sts.example.com"]);
            let v = status();
            assert_eq!(v["cert"]["state"], "issued", "{v}");
            assert!(v["lastError"].is_null(), "healthy cert → null: {v}");
            assert_eq!(v["cert"]["names"], serde_json::json!(["mail.example.com", "mta-sts.example.com"]));
            assert!(
                v["cert"]["note"].as_str().expect("HF9 note").contains("covers mail.example.com only"),
                "{v}"
            );
        }
        clear_test_status_reads();
        clean_row();
    }

    /// Envelope `ok` is always true on a successful read; the systemd
    /// verdict rides `consistent` (fb449bc1 regression: the CLI died on ok=false).
    #[test]
    fn status_envelope_ok_is_true_and_verdict_rides_consistent() {
        let _g = crate::mail::mail_server_test_lock();
        let seed = |status: &str| {
            clean_row();
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, hostname, updated_at) \
                 VALUES (1, ?1, ?2, 'mail.acme.dev', 100)",
                rusqlite::params![status, STALWART_PINNED_VERSION],
            )
            .expect("seed row");
        };
        let read = || -> serde_json::Value {
            let resp = handle_status(&HashMap::new());
            assert_eq!(resp.status, "200 OK", "{}", resp.body);
            serde_json::from_str(&resp.body).expect("json")
        };

        {
            let _unit = supervisor::with_test_unit_state("inactive");
            seed("disabled");
            let v = read();
            assert_eq!(v["ok"], true, "{v}");
            assert_eq!(v["consistent"], true, "disabled + inactive agrees: {v}");
            assert_eq!(v["state"], "disabled", "{v}");
            assert_eq!(v["lastError"], serde_json::Value::Null, "{v}");
        }
        {
            let _unit = supervisor::with_test_unit_state("active");
            seed("disabled");
            let v = read();
            assert_eq!(v["ok"], true, "{v}");
            assert_eq!(v["consistent"], false, "disabled + active disagrees: {v}");
            assert_eq!(
                v["lastError"],
                "systemd reports the stalwart unit is 'active' while hostmail is disabled",
                "{v}"
            );

            seed("running");
            let v = read();
            assert_eq!(v["ok"], true, "{v}");
            assert_eq!(v["consistent"], true, "{v}");
            assert_eq!(v["state"], "running", "{v}");
        }
        {
            let _unit = supervisor::with_test_unit_state("failed");
            seed("running");
            let v = read();
            assert_eq!(v["ok"], true, "{v}");
            assert_eq!(v["consistent"], false, "{v}");
            assert_eq!(v["state"], "error", "{v}");
        }
        clean_row();
        {
            // No row at all → not-installed, consistent.
            let _unit = supervisor::with_test_unit_state("inactive");
            let v = read();
            assert_eq!(v["ok"], true, "{v}");
            assert_eq!(v["consistent"], true, "{v}");
            assert_eq!(v["state"], "not-installed", "{v}");
        }
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
            (br#"{"quotaBytes":0}"#, "'workspace'"),
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

    /// The always-BCC policy is the OWNER's oversight control: the owner
    /// (no scoped passport) sets and clears it and `k2 hostmail config`
    /// shows it; the workspace's own agent — even with mail-manage on —
    /// can neither set nor clear it. Works on every platform (linked
    /// sends run everywhere), so it is not stopped by the D3 gate.
    #[test]
    fn always_bcc_owner_sets_and_clears_agent_and_mail_manage_cannot() {
        let id = uuid::Uuid::new_v4().to_string();
        let short = &id[..12];
        let name = format!("abcc-{short}");
        let path = format!("/tmp/mail-always-bcc-{}-{short}", std::process::id());
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, mail_manage_enabled) \
                 VALUES (?1, ?2, ?3, 1)",
                rusqlite::params![id, name, path],
            )
            .expect("insert project");
        }
        let body = |v: serde_json::Value| serde_json::to_vec(&v).unwrap();
        let policy = || k2_core::workspace::settings::mail_always_bcc_for_path(&path).unwrap();
        let agent = crate::session_token::HookPrincipal {
            workspace_uuid: id.clone(),
            agent_address: "agent".to_string(),
        };

        // Agent (mail-manage enabled, its OWN workspace) cannot set it.
        let resp = crate::caller_workspace::with_request_principal(Some(agent.clone()), || {
            handle_config_set(&body(serde_json::json!({
                "workspace": path, "alwaysBcc": "agent-pick@evil.example"
            })))
        });
        assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
        assert!(resp.body.contains("owner_only"), "{}", resp.body);
        assert!(
            resp.body.contains("can't loosen mail rules on its own sends"),
            "{}",
            resp.body
        );
        assert!(policy().is_empty(), "agent write must not land");

        // Owner sets it (comma-separated string; normalized).
        let resp = handle_config_set(&body(serde_json::json!({
            "workspace": path, "alwaysBcc": "Owner@Shop.example, audit@shop.example"
        })));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(
            v["applied"]["alwaysBcc"]["alwaysBcc"],
            serde_json::json!(["owner@shop.example", "audit@shop.example"]),
            "{v}"
        );
        assert_eq!(policy().len(), 2);

        // Visible in the owner's config read.
        let cfg = config::config_json();
        let ours = cfg["workspaceOverrides"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["project"] == path.as_str())
            .expect("override row listed");
        assert_eq!(ours["alwaysBcc"].as_array().map(|a| a.len()), Some(2), "{ours}");

        // Agent cannot clear it either (string or list form).
        for clear in [serde_json::json!(""), serde_json::json!([])] {
            let resp = crate::caller_workspace::with_request_principal(Some(agent.clone()), || {
                handle_config_set(&body(serde_json::json!({
                    "workspace": path, "alwaysBcc": clear
                })))
            });
            assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
            assert_eq!(policy().len(), 2, "agent clear must not land");
        }

        // Validation: bad address, too many, missing workspace.
        let resp = handle_config_set(&body(serde_json::json!({
            "workspace": path, "alwaysBcc": "not-an-address"
        })));
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        let ten: Vec<String> = (0..10).map(|i| format!("o{i}@shop.example")).collect();
        let resp = handle_config_set(&body(serde_json::json!({
            "workspace": path, "alwaysBcc": ten
        })));
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        assert!(resp.body.contains("at most 9"), "{}", resp.body);
        let resp = handle_config_set(&body(serde_json::json!({ "alwaysBcc": "a@b.example" })));
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        assert!(resp.body.contains("'alwaysBcc' needs 'workspace'"), "{}", resp.body);
        assert_eq!(policy().len(), 2, "rejected writes change nothing");

        // Owner clears it.
        let resp = handle_config_set(&body(serde_json::json!({
            "workspace": path, "alwaysBcc": ""
        })));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        assert!(policy().is_empty());

        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id]);
        }
    }

    /// 0.45.0 agent mail POLICY (agentSend, always-BCC): an IT agent
    /// (mail-manage) may change it for OTHER workspaces, like the owner,
    /// but never for its own (`owner_only` — it can't loosen the rules on
    /// its own sends); an agent without mail-manage never; the owner always.
    #[test]
    fn agent_mail_policy_it_agent_for_others_only_owner_always() {
        let mk = |tag: &str, mm: i64| {
            let id = uuid::Uuid::new_v4().to_string();
            let path = format!("/tmp/mail-policy-{tag}-{}-{}", std::process::id(), &id[..12]);
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, mail_manage_enabled, mail_agent_send) \
                 VALUES (?1, ?2, ?3, ?4, 'approval')",
                rusqlite::params![id, format!("pol-{tag}-{}", &id[..12]), path, mm],
            )
            .expect("insert project");
            (id, path)
        };
        let (it_id, it_path) = mk("it", 1);
        let (other_id, other_path) = mk("other", 0);
        let (plain_id, _) = mk("plain", 0);
        let stored = |id: &str| {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT mail_agent_send FROM projects WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get::<_, Option<String>>(0),
            )
            .expect("row")
        };
        let bcc =
            |path: &str| k2_core::workspace::settings::mail_always_bcc_for_path(path).unwrap();
        let body = |v: serde_json::Value| serde_json::to_vec(&v).unwrap();
        let as_agent = |id: &str, b: serde_json::Value| {
            let p = crate::session_token::HookPrincipal {
                workspace_uuid: id.to_string(),
                agent_address: "agent".to_string(),
            };
            crate::caller_workspace::with_request_principal(Some(p), || {
                handle_config_set(&body(b))
            })
        };
        let code = |r: &CliResponse| {
            serde_json::from_str::<serde_json::Value>(&r.body).unwrap()["error"]["code"]
                .as_str()
                .unwrap_or("")
                .to_string()
        };

        // IT agent, OWN workspace: refused, alone or bundled.
        for b in [
            serde_json::json!({ "workspace": it_path, "agentSend": "on" }),
            serde_json::json!({ "workspace": it_path, "agentSend": "on", "addressCap": 3 }),
            serde_json::json!({ "workspace": it_path, "alwaysBcc": "" }),
        ] {
            let r = as_agent(&it_id, b.clone());
            assert_eq!(r.status, "403 Forbidden", "{b}: {}", r.body);
            assert_eq!(code(&r), "owner_only", "{}", r.body);
            assert!(
                r.body.contains("can't loosen mail rules on its own sends"),
                "{}",
                r.body
            );
        }
        assert_eq!(stored(&it_id).as_deref(), Some("approval"), "own write must not land");

        // Agent WITHOUT mail-manage: refused for any workspace.
        for b in [
            serde_json::json!({ "workspace": other_path, "agentSend": "on" }),
            serde_json::json!({ "workspace": other_path, "alwaysBcc": "x@shop.example" }),
        ] {
            let r = as_agent(&plain_id, b.clone());
            assert_eq!(r.status, "403 Forbidden", "{b}: {}", r.body);
            assert_eq!(code(&r), "agent_send_path_only", "{}", r.body);
        }
        assert!(bcc(&other_path).is_empty());

        // IT agent, OTHER workspace: always-BCC lands (a DB write, every
        // platform); agentSend passes the gate (applied on Linux; the D3
        // off-Linux gate answers elsewhere — never a refusal).
        let r = as_agent(
            &it_id,
            serde_json::json!({ "workspace": other_path, "alwaysBcc": "audit@shop.example" }),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(bcc(&other_path), vec!["audit@shop.example".to_string()]);
        let r = as_agent(
            &it_id,
            serde_json::json!({ "workspace": other_path, "agentSend": "on" }),
        );
        assert_ne!(r.status, "403 Forbidden", "{}", r.body);
        if super::mail_supported() {
            assert_eq!(r.status, "200 OK", "{}", r.body);
            assert_eq!(stored(&other_id).as_deref(), Some("on"));
        } else {
            assert_eq!(r.status, "409 Conflict", "{}", r.body);
        }

        // Owner: any workspace, the IT agent's own included.
        let r = handle_config_set(&body(
            serde_json::json!({ "workspace": it_path, "alwaysBcc": "owner@shop.example" }),
        ));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(bcc(&it_path), vec!["owner@shop.example".to_string()]);
        let r = handle_config_set(&body(
            serde_json::json!({ "workspace": it_path, "agentSend": "on" }),
        ));
        assert_ne!(r.status, "403 Forbidden", "{}", r.body);

        let db = k2_core::db::shared();
        let conn = db.lock();
        for id in [&it_id, &other_id, &plain_id] {
            conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id])
                .expect("cleanup project");
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

        // Hosted domain, no per-domain run → no_run 409 (not ok:true + run:null).
        let hosted = format!("norun-route-{}.example", uuid::Uuid::new_v4().simple());
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_domains (id, domain, status, created_at) VALUES (?1, ?2, 'verified', 100)",
                rusqlite::params![format!("dom-{hosted}"), hosted],
            )
            .expect("seed hosted domain");
        }
        let mut params = HashMap::new();
        params.insert("domain".to_string(), hosted.clone());
        let resp = handle_doctor(&params);
        assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["ok"], false, "{v}");
        assert_eq!(v["error"]["code"].as_str().expect("code"), "no_run", "{v}");
        assert!(
            v["error"]["hint"]
                .as_str()
                .expect("hint")
                .contains("POST /cli/mail/doctor"),
            "{v}"
        );
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = conn.execute(
                "DELETE FROM mail_domains WHERE domain = ?1",
                rusqlite::params![hosted],
            );
        }

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
