//! Route bodies for the tunnel-identity path (0.45.2): `POST
//! /cli/tunnel/config` (a write that changes the subdomain or token),
//! `POST /cli/tunnel/switch`, and `POST /cli/tunnel/release` (release or
//! forget). Each moves every subdomain-bound component through
//! `k2_core::tunnel::identity` and then tells every client
//! (`tunnel_status_changed` + `tunnel_identity_changed`).
//!
//! Blocking work (frpc spawn, cert broker, `block_on` inside the listener
//! ensure hook) runs on the blocking pool: the ensure hook must not run on
//! a runtime worker.

use k2_core::tunnel::identity::{self, IdentityChangeReport, SwitchRequest};
use k2_core::tunnel::unpair::ReleaseMode;
use k2_core::tunnel::{FrpcBinary, TunnelConfigUpdate};

use crate::cli::CliResponse;
use crate::session_events::{emit, SessionEvent};

/// Tell every client the tunnel moved. Always both events: older clients
/// only know `tunnel_status_changed`.
pub fn emit_identity_events(report: &IdentityChangeReport) {
    let _ = emit(SessionEvent::TunnelStatusChanged {
        running: report.status.running,
        public_url: report.status.public_url.clone(),
    });
    let _ = emit(SessionEvent::TunnelIdentityChanged {
        previous_subdomain: report.previous_subdomain.clone(),
        subdomain: report.subdomain.clone(),
        public_url: report
            .subdomain
            .as_deref()
            .map(|s| format!("https://{s}.{}", k2_core::tunnel::config::SUBDOMAIN_HOST)),
        running: report.status.running,
        ok: report.ok,
    });
}

fn to_json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

/// `POST /cli/tunnel/config`: persist the update; when the subdomain,
/// token or E2E flag changed, move the daemon to the new identity (a
/// running tunnel restarts under it). Body = the redacted view, plus
/// `identityChange` (the step report) when the identity changed.
pub async fn handle_config_post(upd: TunnelConfigUpdate, daemon_port: u16) -> CliResponse {
    let result = tokio::task::spawn_blocking(move || {
        identity::apply_config(upd, daemon_port, &FrpcBinary::Auto)
    })
    .await
    .unwrap_or_else(|e| Err(format!("worker join: {e}")));
    match result {
        Ok((view, report)) => {
            let mut v = serde_json::to_value(&view)
                .unwrap_or_else(|_| serde_json::json!({}));
            if let Some(report) = report {
                emit_identity_events(&report);
                if let Some(obj) = v.as_object_mut() {
                    obj.insert(
                        "identityChange".to_string(),
                        serde_json::to_value(&report).unwrap_or(serde_json::Value::Null),
                    );
                }
            }
            CliResponse::ok_json(v.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// `POST /cli/tunnel/switch` (JSON [`SwitchRequest`]): bind another
/// subdomain and move everything to it; a failed step rolls back to the
/// old name. 200 with the step report either way (`ok`, `rolledBack`,
/// `error`); 400 only for bad input (nothing changed).
pub async fn handle_switch(body: &[u8], daemon_port: u16) -> CliResponse {
    let req: SwitchRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let result = tokio::task::spawn_blocking(move || {
        identity::switch_subdomain(req, daemon_port, &FrpcBinary::Auto)
    })
    .await
    .unwrap_or_else(|e| Err(format!("worker join: {e}")));
    match result {
        Ok(report) => {
            k2_core::log_debug!(
                "[tunnel] switch {:?} → {:?}: ok={} rolledBack={} error={:?}",
                report.previous_subdomain,
                report.subdomain,
                report.ok,
                report.rolled_back,
                report.error
            );
            emit_identity_events(&report);
            CliResponse::ok_json(to_json(&report))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// `POST /cli/tunnel/release?confirm=1[&mode=forget]`: release (report
/// upstream only when this device is the holder) or forget locally, then
/// move the daemon off the name (listener, renewer, routes). Body = the
/// release report plus `identityChange`.
pub async fn handle_release(mode: ReleaseMode, daemon_port: u16) -> CliResponse {
    let result = tokio::task::spawn_blocking(move || {
        identity::release(mode, daemon_port, &FrpcBinary::Auto)
    })
    .await
    .unwrap_or_else(|e| Err(format!("worker join: {e}")));
    match result {
        Ok((released, report)) => {
            k2_core::log_debug!(
                "[tunnel] identity RELEASED for subdomain {:?} (mode={mode:?}, \
                 upstream_reported={}, queued={}, local_only={})",
                released.subdomain,
                released.upstream_reported,
                released.queued,
                released.local_only
            );
            emit_identity_events(&report);
            let mut v = serde_json::to_value(&released).unwrap_or_else(|_| serde_json::json!({}));
            if let Some(obj) = v.as_object_mut() {
                obj.insert(
                    "identityChange".to_string(),
                    serde_json::to_value(&report).unwrap_or(serde_json::Value::Null),
                );
            }
            CliResponse::ok_json(v.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// `mode=forget` → [`ReleaseMode::ForgetLocal`]; anything else releases.
pub fn release_mode(params: &std::collections::HashMap<String, String>) -> ReleaseMode {
    match params.get("mode").map(|s| s.trim().to_ascii_lowercase()) {
        Some(m) if m == "forget" || m == "local" => ReleaseMode::ForgetLocal,
        _ => ReleaseMode::Release,
    }
}
