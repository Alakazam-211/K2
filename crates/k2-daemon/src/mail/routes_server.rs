//! `/cli/mail/*` — SERVER-concern handlers: status (REAL), enable /
//! disable / uninstall, config, doctor (stubs for S1/S5/S6).
//!
//! Dispatched by the `crate::mail_routes` shim. AUTH/GATING contract
//! for this file's mutations (PRD §10), enforced in the dispatcher's
//! `/cli/mail/` POST arm and re-asserted per-handler as slices land:
//! server enable/disable/uninstall + config = OWNER-OR-ADMIN
//! (`token_is_owner_or_admin`), POST-only (`require_post` +
//! `post_allowed`, house rule feedback_post_only_route_guards).

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::supervisor::{mail_supported, STALWART_PINNED_VERSION};

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
///   "portPlan": <port_plan|null> }
/// ```
///
/// `state` comes from the `mail_server` singleton row; NO row =
/// `"not-installed"` (the 0072 contract). The renderer gates the whole
/// Settings→Email page on `supported` — from the DAEMON's report,
/// never `navigator.platform` (a Mac app driving a remote Linux daemon
/// must see the real page).
pub fn handle_status(_params: &HashMap<String, String>) -> CliResponse {
    let row: Option<(String, Option<String>, Option<String>, Option<String>)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT status, installed_version, hostname, port_plan FROM mail_server WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok()
    };
    let (state, version, hostname, port_plan) = match row {
        Some((status, installed, hostname, plan)) => (status, installed, hostname, plan),
        None => ("not-installed".to_string(), None, None, None),
    };
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "supported": mail_supported(),
            "state": state,
            "version": version,
            "pinnedVersion": STALWART_PINNED_VERSION,
            "hostname": hostname,
            "portPlan": port_plan,
        })
        .to_string(),
    )
}

/// POST `/cli/mail/server/enable` — S1: preflight → install →
/// bootstrap (owner-or-admin; fails closed if the open-relay self-test
/// can't run, pre-mortem #3).
pub fn handle_server_enable(_body: &[u8]) -> CliResponse {
    super::not_built("S1", "POST /cli/mail/server/enable")
}

/// POST `/cli/mail/server/disable` — S1: stop + disable, KEEP data
/// (owner-or-admin; warns loudly that MX now points at a dead port).
pub fn handle_server_disable(_body: &[u8]) -> CliResponse {
    super::not_built("S1", "POST /cli/mail/server/disable")
}

/// POST `/cli/mail/server/uninstall` — S1: disable + optional explicit
/// data purge (owner-or-admin; double-confirm, types the hostname).
pub fn handle_server_uninstall(_body: &[u8]) -> CliResponse {
    super::not_built("S1", "POST /cli/mail/server/uninstall")
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

    /// The one REAL route: empty `mail_server` table → not-installed,
    /// `supported` matches the runtime gate, pinned version reported.
    /// Then an installed row flips state/version/hostname/portPlan.
    ///
    /// `db::shared()` is the process-global in-memory test DB — the
    /// row is cleaned up at the end so sibling tests keep seeing the
    /// empty-table default.
    #[test]
    fn status_reports_not_installed_then_row_state() {
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

        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, installed_version, \
                 hostname, port_plan, installed_at, updated_at) \
                 VALUES (1, 'running', ?1, '0.16.0', 'mail.acme.dev', 'tls-alpn', 100, 100)",
                rusqlite::params![STALWART_PINNED_VERSION],
            )
            .expect("insert mail_server row");
        }
        let resp = handle_status(&HashMap::new());
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["state"], "running");
        assert_eq!(v["version"], "0.16.0");
        assert_eq!(v["hostname"], "mail.acme.dev");
        assert_eq!(v["portPlan"], "tls-alpn");
        assert_eq!(v["supported"], cfg!(target_os = "linux"), "supported is the RUNTIME gate, independent of install state");

        // Clean up the singleton for sibling tests.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("DELETE FROM mail_server WHERE id = 1", [])
                .expect("cleanup");
        }
    }

    /// Every stub answers the structured 501 (never a 404/500) so
    /// callers can tell "reserved for a later slice" from a bad path.
    #[test]
    fn stubs_answer_structured_501() {
        for (resp, slice) in [
            (handle_server_enable(b"{}"), "S1"),
            (handle_server_disable(b"{}"), "S1"),
            (handle_server_uninstall(b"{}"), "S1"),
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
