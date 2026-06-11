//! Daemon-side handlers for `/cli/settings/{get,update,reset}` —
//! Phase 2 Unit 7a.
//!
//! Owns the read/write surface for `~/.k2so/settings.json` so the
//! Tauri thin client (and any K2SO Connect / Mobile Companion future
//! client) talks to a single writer instead of racing with an
//! in-process Tauri copy. The actual JSON shape + tmp+rename writer
//! lives in `k2_core::app_settings`; these handlers translate
//! between HTTP and the typed `AppSettings`.
//!
//! # F3 close
//!
//! Pre-Unit-7a, `commands/settings.rs::settings_update` invalidated
//! companion sessions in Tauri's in-process `STATE` — which is empty
//! after Unit 1 moved the companion runtime to the daemon. The call
//! was a no-op and rotated credentials left live tokens valid until
//! their TTL expired.
//!
//! `app_settings::update()` now runs the comparison + invalidation
//! itself, in the same process as the live companion `STATE`. The
//! handler below just hands `partial` off to that single critical
//! section.

use crate::cli_response::CliResponse;

/// Handler for `GET /cli/settings/get`.
///
/// Token check happens in `main.rs` before this is called. Returns
/// the full `AppSettings` JSON serialized via serde so the renderer
/// (or `k2so` CLI) sees the same shape Tauri's `settings_get` used to
/// produce.
pub fn handle_settings_get() -> CliResponse {
    let settings = k2_core::app_settings::load();
    match serde_json::to_string(&settings) {
        Ok(body) => CliResponse::ok_json(body),
        Err(e) => CliResponse::bad_request(format!("serialize settings: {e}")),
    }
}

/// Handler for `POST /cli/settings/update`.
///
/// Body: a JSON object of partial settings to deep-merge into the
/// current state. Accepts the camelCase shape the renderer already
/// sends to Tauri's `settings_update` so a single payload format
/// covers the proxy path AND any future direct-daemon callers.
///
/// On success returns the full post-merge `AppSettings`. F3-close
/// runs inside `app_settings::update()` — when companion-affecting
/// fields differ from disk, every live companion session is
/// invalidated before this handler returns.
pub fn handle_settings_update(body: &[u8]) -> CliResponse {
    let partial: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if !partial.is_object() {
        return CliResponse::bad_request(
            "expected JSON object at top level (got non-object)".to_string(),
        );
    }
    match k2_core::app_settings::update(partial) {
        Ok(merged) => match serde_json::to_string(&merged) {
            Ok(body) => CliResponse::ok_json(body),
            Err(e) => CliResponse::bad_request(format!("serialize merged: {e}")),
        },
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `POST /cli/settings/reset`.
///
/// Restores `AppSettings::default()`, clears the macOS-Keychain
/// companion password hash, and invalidates every live companion
/// session — matching the pre-Unit-7a Tauri `settings_reset` body
/// exactly. POST (not GET) because the call is destructive and
/// shouldn't be reachable via a browser-cached idempotent fetch.
pub fn handle_settings_reset() -> CliResponse {
    match k2_core::app_settings::reset() {
        Ok(defaults) => match serde_json::to_string(&defaults) {
            Ok(body) => CliResponse::ok_json(body),
            Err(e) => CliResponse::bad_request(format!("serialize defaults: {e}")),
        },
        Err(e) => CliResponse::bad_request(e),
    }
}
