//! `/cli/mail/*` — SERVER-concern handlers: status + preflight (REAL),
//! enable / disable / uninstall (S1, REAL), config, doctor (stubs for
//! S5/S6).
//!
//! Dispatched by the `crate::mail_routes` shim. AUTH/GATING contract
//! for this file's mutations (PRD §10), enforced in the dispatcher's
//! `/cli/mail/` POST arm and re-asserted per-handler as slices land:
//! server enable/disable/uninstall + config = OWNER-OR-ADMIN
//! (`token_is_owner_or_admin`), POST-only (`require_post` +
//! `post_allowed`, house rule feedback_post_only_route_guards).
//!
//! Non-Linux daemons (D3): validation runs first (so the Mac
//! example-page exercises real error text), then every mutation stops
//! at the `mail_supported()` gate with the structured `unsupported`
//! 409 — nothing system-level ever executes off-Linux.

use std::collections::HashMap;

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
    // already running → nothing to do.
    if supervisor::current_status().as_deref() == Some("running") {
        return CliResponse::ok_json(
            serde_json::json!({ "ok": true, "state": "running", "alreadyEnabled": true })
                .to_string(),
        );
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
        if let Err(e) = result {
            k2_core::log_debug!("[mail/supervisor] enable failed: {e}");
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

/// GET `/cli/mail/config` — S5: read the effective send-mode/relay/
/// gating configuration.
pub fn handle_config_get(_params: &HashMap<String, String>) -> CliResponse {
    super::not_built("S5", "GET /cli/mail/config")
}

/// POST `/cli/mail/config/set` — S5: send-mode per domain, relay
/// creds, `mail_agent_send` / `mail_address_cap` per workspace
/// (owner-or-admin; the settings keys + resolvers already exist in
/// k2-core: `workspace::settings::mail_agent_send_for_path` /
/// `mail_address_cap_for_path`).
pub fn handle_config_set(_body: &[u8]) -> CliResponse {
    super::not_built("S5", "POST /cli/mail/config/set")
}

/// GET `/cli/mail/doctor` — S6: latest stored run (+ `?run=1` triggers
/// a fresh one when built; method shape finalized in S6).
pub fn handle_doctor(_params: &HashMap<String, String>) -> CliResponse {
    super::not_built("S6", "GET /cli/mail/doctor")
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
        clean_row();
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

    /// Every remaining stub answers the structured 501 (never a
    /// 404/500) so callers can tell "reserved for a later slice" from
    /// a bad path.
    #[test]
    fn stubs_answer_structured_501() {
        for (resp, slice) in [
            (handle_config_get(&HashMap::new()), "S5"),
            (handle_config_set(b"{}"), "S5"),
            (handle_doctor(&HashMap::new()), "S6"),
        ] {
            assert_eq!(resp.status, "501 Not Implemented");
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["ok"], false);
            assert_eq!(v["error"]["code"], "not_built");
            let hint = v["error"]["hint"].as_str().expect("hint");
            assert!(
                hint.contains(&format!("not built yet — mail slice {slice}")),
                "{hint}"
            );
        }
    }
}
