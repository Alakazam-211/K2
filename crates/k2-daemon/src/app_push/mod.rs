//! Push notifications for custom Apps (`prd-custom-app-push-v1`, 0.46.4).
//!
//! A person builds their own phone app as the front end of a custom K2 App.
//! Its guests register their phones here; the App, its agent or the owner
//! sends them notifications; **the daemon calls Apple and Google directly
//! with the App owner's own keys**, held in the vault (system namespace,
//! scope `app/<service id>`). K2 Connect's push relay is not on this path.
//!
//! - [`guest_routes`]: the guest's own device (`push.register`,
//!   `push.unregister`, `push.device`, `push.prefs`), a guest session pass
//!   only, on the main port, the browser helper or the App's link.
//! - [`owner_routes`]: `/cli/app/push/*` for the owner, Admins and the
//!   App's agent (`k2 app push …`).
//! - Storage: `k2_core::skin::app_push` (skin.db).
//!
//! **State.** An App's push is on when the owner approved `push:send` for
//! it (a held deploy, once) or ran `k2 app push on`, and the owner hasn't
//! turned it off. `K2_APP_PUSH=off` in the daemon's environment turns every
//! App off (Rosson's switch rule). Keys are separate: an App can be on with
//! no keys yet, so a mobile developer can integrate before they exist.

pub mod budget;
pub mod creds;
pub mod guest_routes;
pub mod owner_routes;
pub mod payload;
pub mod sender;
pub mod sendlog;
pub mod worker;
#[cfg(test)]
pub(crate) mod test_keys;

use k2_core::skin::{self, app_push as store};

/// The box-wide switch: `off` (or `0`/`false`/`no`) stops every App.
pub const SWITCH_ENV: &str = "K2_APP_PUSH";

/// Is App push allowed on this box at all (default yes)?
pub fn box_enabled() -> bool {
    !switch_off(std::env::var(SWITCH_ENV).ok().as_deref())
}

/// Does the switch's value mean off?
pub fn switch_off(v: Option<&str>) -> bool {
    matches!(v.map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("off" | "0" | "false" | "no"))
}

/// Why an App's push is off (`None` = on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Off {
    /// `host` (the box login) never has push (Q12).
    Host,
    /// No published App by that id.
    NoApp,
    /// `K2_APP_PUSH=off`.
    Box,
    /// The owner turned it off.
    Owner,
    /// The owner hasn't approved `push:send` (or run `k2 app push on`).
    NotApproved,
}

impl Off {
    pub fn as_str(self) -> &'static str {
        match self {
            Off::Host => "host",
            Off::NoApp => "no_app",
            Off::Box => "box_off",
            Off::Owner => "owner_off",
            Off::NotApproved => "not_approved",
        }
    }
}

/// The published service row of `app` (a service id), if any.
pub(crate) fn published(app: &str) -> Option<k2_core::published_services::PublishedService> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::get_by_id(&conn, app).ok().flatten()
}

/// Did the owner approve `push:send` for this App (its runtime App row's
/// approved set)? A `--cmd` service has none: its owner runs `k2 app push on`.
pub fn push_send_approved(service_id: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(Some(app)) = k2_core::deploy::store::get_app_by_service(&conn, service_id) else { return false };
    matches!(crate::apps::grants::recorded(&conn, &app.app_id), Ok(Some(set)) if set.iter().any(|c| c == skin::CAP_PUSH_SEND))
}

/// An App's push state: `Ok(())` = on.
pub fn state(app: &str) -> Result<(), Off> {
    let app = app.trim();
    if app.is_empty() || app == skin::HOST_APP_ID {
        return Err(Off::Host);
    }
    if !box_enabled() {
        return Err(Off::Box);
    }
    if published(app).is_none() {
        return Err(Off::NoApp);
    }
    let cfg = store::config(app).map_err(|_| Off::NoApp)?;
    if !cfg.enabled {
        return Err(Off::Owner);
    }
    if cfg.owner_on || push_send_approved(app) {
        Ok(())
    } else {
        Err(Off::NotApproved)
    }
}

/// `k2Who` / register's `who` for one guest of one App (CP9, CP29).
pub fn who(app: &str, principal_id: &str) -> Result<String, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::read_state::app_push_who(&conn, app, principal_id).map_err(|e| e.to_string())
}

/// An App went away (`publish rm`, `k2 app uninstall`): its keys, devices
/// and settings go.
pub fn forget_app(service_id: &str) {
    creds::forget_app(service_id);
    match store::purge_app(service_id) {
        Ok(n) if n > 0 => k2_core::log_debug!("[app-push] {service_id} removed: {n} push device(s) dropped"),
        Ok(_) => {}
        Err(e) => k2_core::log_debug!("[app-push] {service_id} removed: WARN dropping push devices: {e}"),
    }
}

/// The hourly sweep (CP6): devices whose pass or device sign-in ended
/// without a revoke (plain expiry).
pub fn sweep() {
    match store::sweep() {
        Ok(n) if n > 0 => k2_core::log_debug!("[app-push] swept {n} push device(s) whose sign-in ended"),
        Ok(_) => {}
        Err(e) => k2_core::log_debug!("[app-push] WARN sweep: {e}"),
    }
}

/// The App id `app-audit.db` rows use: the runtime App id when the
/// service is a runtime App, else the service id.
pub(crate) fn audit_app_id(service_id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::deploy::store::get_app_by_service(&conn, service_id).ok().flatten().map(|a| a.app_id).unwrap_or_else(|| service_id.to_string())
}

/// One Access Audit line (`auth-audit.jsonl`). Never a key, a token or a
/// notification's text.
pub(crate) fn audit_line(event: &str, actor: &str, line: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, actor, line.to_string(), "-", "-", "app-push"));
}

/// The JSON refusal shape every App push route uses.
pub(crate) fn refusal(status: &'static str, code: &str, hint: &str) -> crate::cli_response::CliResponse {
    refusal_with(status, code, hint, serde_json::Map::new())
}

/// [`refusal`] with extra fields inside `error` (`retryAfterMs`, `limit`).
pub(crate) fn refusal_with(
    status: &'static str,
    code: &str,
    hint: &str,
    extra: serde_json::Map<String, serde_json::Value>,
) -> crate::cli_response::CliResponse {
    let mut err = serde_json::Map::new();
    err.insert("code".into(), serde_json::json!(code));
    err.insert("hint".into(), serde_json::json!(hint));
    err.extend(extra);
    crate::cli_response::CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": err }).to_string(),
    }
}

/// `403 push_off` for a state that is off, in words for the caller.
pub(crate) fn off_response(off: Off) -> crate::cli_response::CliResponse {
    match off {
        Off::Host => refusal("400 Bad Request", "not_for_host", "push is for a published App's own users, never the box login (host)"),
        _ => refusal("403 Forbidden", "push_off", "this App doesn't send notifications (the owner turns them on: k2 app push on <app>)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_box_switch_reads_off_words() {
        for v in ["off", " No ", "0", "FALSE"] {
            assert!(switch_off(Some(v)), "{v}");
        }
        for v in [None, Some("on"), Some(""), Some("1")] {
            assert!(!switch_off(v), "{v:?}");
        }
        assert_eq!(state(skin::HOST_APP_ID), Err(Off::Host));
        assert_eq!(state(""), Err(Off::Host));
    }
}
