//! Email Notifications (prd-email-notifications-core-v1, 0.46.5): K2's
//! one-way mail (password resets, invites, an App's own `email.send`) from
//! **senders**, never from mailboxes.
//!
//! - A **sender** is a Stalwart account K2 creates with no usable
//!   credential and login permissions off (hosted, Linux), or an address on
//!   an owner-configured SMTP relay (any box, Macs included). Senders live
//!   only in `email_senders` (k2.db 0227), never in `mail_addresses`, so
//!   every Hosted Mail verb answers `not_found` for them (E1).
//! - The daemon submits hosted mail by admin delegation with the service
//!   ApiKey (`accountId` = the sender's account), never as the account (E3).
//! - One **system sender** per box carries K2's own fixed text (owner
//!   resets with no `from`, the profile PRD's codes and notices). App-written
//!   text never rides it (E14).
//! - An App sends from **its own granted sender** after the owner turned it
//!   on with a fresh confirm (`k2 app email on`), holding the cap
//!   `email:send` (in no App core, so a deploy adding it is held once).
//!
//! Module map:
//! - [`senders`]: add / remove / system sender (owner + confirm).
//! - [`fence`]: the hosted-mail side of E2 and E4.
//! - [`engine`]: the Stalwart and relay seams (fakes in tests).
//! - [`compose`]: request parsing and the E19 composition rules, the JMAP
//!   Email object (E8) and the relay's MIME message.
//! - [`dmarc`]: E10.
//! - [`budget`]: E17.
//! - [`send`]: the §5.2 pipeline, `system_send` (E28), held sends (E15),
//!   the scheduled flusher.
//! - [`relay`]: SMTP relays, their vault credentials (E12).
//! - [`sweep`]: the hourly sweeps (E5, E22, E26, idempotency).
//! - [`owner_routes`] (`/cli/notifications/email/*`), [`app_routes`]
//!   (`/cli/app/email/*`), [`link_routes`] (`/cli/skin/email/*` and
//!   `/v1/apps/<id>/email/*`).
//!
//! **AGPL boundary (E23).** Stalwart is reached only over its loopback
//! HTTP/JMAP API through `crate::mail::jmap`; it is never linked, vendored
//! or patched, and no Enterprise key is ever installed.
//!
//! **Logs (E22).** Daemon log lines here carry ids and counts only: never
//! an address, a subject or a body.

pub mod app_routes;
pub mod budget;
pub mod compose;
pub mod dmarc;
pub mod engine;
pub mod fence;
pub mod link_routes;
pub mod owner_routes;
pub mod relay;
pub mod send;
pub mod senders;
pub mod sweep;

#[cfg(test)]
mod tests;

use serde_json::{json, Value as J};

use crate::cli_response::CliResponse;

/// The box switch (Rosson's switch rule): `off`/`0`/`false`/`no` stops
/// every notification send.
pub const SWITCH_ENV: &str = "K2_EMAIL_NOTIFICATIONS";

/// Vault confirm actions (`secretId` noted per action). Every caller,
/// the owner token included, needs one: agents can read the token.
pub const CONFIRM_SENDER: &str = "email-sender"; // the address
pub const CONFIRM_SYSTEM_SENDER: &str = "email-system-sender"; // the sender address
pub const CONFIRM_RELAY: &str = "email-relay"; // the relay host
pub const CONFIRM_TEST: &str = "email-test"; // the sender address
pub const CONFIRM_APP_ON: &str = "app-email-on"; // the App's service id
pub const CONFIRM_APP_ANY: &str = "app-email-any"; // the App's service id

/// Every confirm action this module registers (vault_routes' list).
pub const CONFIRM_ACTIONS: &[&str] = &[CONFIRM_SENDER, CONFIRM_SYSTEM_SENDER, CONFIRM_RELAY, CONFIRM_TEST, CONFIRM_APP_ON, CONFIRM_APP_ANY];

/// Is Email Notifications allowed on this box at all (default yes)?
pub fn box_enabled() -> bool {
    !crate::app_push::switch_off(std::env::var(SWITCH_ENV).ok().as_deref())
}

/// The text on the confirm sheet, built from the daemon's own records,
/// never from a client's label. `None` = not one of these actions.
pub fn confirm_reason(action: &str, secret_id: Option<&str>) -> Option<Result<String, &'static str>> {
    if !CONFIRM_ACTIONS.contains(&action) {
        return None;
    }
    let Some(id) = secret_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return Some(Err("secretId is required for an Email Notifications confirm"));
    };
    let clean = |s: &str| -> String { s.chars().filter(|c| !c.is_control()).take(120).collect() };
    Some(Ok(match action {
        CONFIRM_SENDER => format!("add {} as a notification sender (K2 creates a send-only account no agent can read)", clean(id)),
        CONFIRM_SYSTEM_SENDER => format!("make {} the system sender: K2's own password resets and codes come from it", clean(id)),
        CONFIRM_RELAY => format!("store an SMTP relay login for {} (notifications from this server go through it)", clean(id)),
        CONFIRM_TEST => format!("send one test email from {} to your own address", clean(id)),
        CONFIRM_APP_ON | CONFIRM_APP_ANY => {
            let Some(row) = crate::app_push::published(id) else { return Some(Err("no published App by that service id")) };
            let name: String = row.name.chars().filter(|c| !c.is_control()).take(64).collect();
            let sender = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                pending_sender_label(&conn, id)
            };
            if action == CONFIRM_APP_ANY {
                format!("let App {name} ({}) email ANY address, not only its own users", row.id)
            } else {
                format!("let App {name} ({}) send email{sender}", row.id)
            }
        }
        _ => return None,
    }))
}

/// ` as noreply@brand.com` when the App already holds a sender (the
/// confirm sheet for turning it back on); empty otherwise (the CLI names
/// the sender it asks for).
fn pending_sender_label(conn: &rusqlite::Connection, app: &str) -> String {
    match k2_core::email_notify::app_sender(conn, app) {
        Ok(Some(s)) => format!(" as {}", s.address),
        _ => String::new(),
    }
}

/// Wall-clock seconds.
pub fn now() -> i64 {
    k2_core::deploy::now()
}

// ── refusals ─────────────────────────────────────────────────────────

/// One refusal: HTTP status, a stable code, a hint in words, and an
/// optional `retryAfterMs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fail {
    pub status: u16,
    pub code: &'static str,
    pub hint: String,
    pub retry_after_ms: Option<u64>,
}

pub type R<T> = Result<T, Fail>;

pub fn fail(status: u16, code: &'static str, hint: impl Into<String>) -> Fail {
    Fail { status, code, hint: hint.into(), retry_after_ms: None }
}

pub fn db_fail(e: impl std::fmt::Display) -> Fail {
    fail(500, "failed", format!("the email store refused: {e}"))
}

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        202 => "202 Accepted",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        429 => "429 Too Many Requests",
        502 => "502 Bad Gateway",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

/// The JSON refusal every route here answers.
pub fn fail_response(f: &Fail) -> CliResponse {
    let mut err = json!({ "code": f.code, "hint": f.hint });
    if let Some(ms) = f.retry_after_ms {
        err["retryAfterMs"] = json!(ms);
    }
    CliResponse { status: status_line(f.status), content_type: "application/json", body: json!({ "ok": false, "error": err }).to_string() }
}

/// `200` with the JSON (`202` when it carries `"accepted": true`, which
/// is dropped from the body), or the refusal.
pub fn respond(r: R<J>) -> CliResponse {
    match r {
        Ok(mut v) => {
            let accepted = v.get("accepted").and_then(J::as_bool) == Some(true);
            if let Some(o) = v.as_object_mut() {
                o.remove("accepted");
            }
            let status = if accepted { "202 Accepted" } else { "200 OK" };
            CliResponse { status, content_type: "application/json", body: v.to_string() }
        }
        Err(f) => fail_response(&f),
    }
}

/// Request bodies here are at most this many bytes (1 MiB of message plus
/// the envelope, E19).
pub const MAX_BODY: usize = 1100 * 1024;

pub fn too_large() -> CliResponse {
    fail_response(&fail(413, "message_too_large", "a notification is at most 1 MB (text 256 KB, html 512 KB)"))
}

/// One Access Audit line. Never an address body or a credential.
pub(crate) fn audit_line(event: &str, actor: &str, line: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, actor, line.to_string(), "-", "-", "email-notify"));
}

/// Is a hosted mail path possible on this box (Linux with Stalwart)?
pub fn hosted_supported() -> bool {
    #[cfg(test)]
    {
        engine::test_hosted_supported()
    }
    #[cfg(not(test))]
    {
        crate::mail::supervisor::mail_supported()
    }
}

/// The air-gap check (E21): every notification send refuses while it is on.
pub fn airgap_blocks() -> bool {
    crate::push_routes::airgap_blocks()
}

/// A sender as the owner sees it (never the Stalwart account id).
pub fn sender_json(s: &k2_core::email_notify::Sender) -> J {
    json!({
        "id": s.id,
        "address": s.address,
        "domain": s.domain,
        "path": s.path,
        "relay": s.relay_id,
        "owner": match s.owner_kind.as_deref() {
            Some(k2_core::email_notify::OWNER_SYSTEM) => json!("system"),
            Some(k2_core::email_notify::OWNER_APP) => json!(format!("app:{}", s.app_service_id.as_deref().unwrap_or(""))),
            _ => J::Null,
        },
        "app": s.app(),
        "fromName": s.from_name,
        "fromNameLocked": s.from_name_locked,
        "replyTo": s.reply_to,
        "state": s.state,
        "bounceHints": s.bounce_hints,
        "createdAt": s.created_at,
        "retiredAt": s.retired_at,
    })
}

/// The App's email state as its owner and agent see it.
pub fn app_state_json(app: &str) -> R<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let st = k2_core::email_notify::app_state(&conn, app).map_err(db_fail)?;
    let sender = match st.sender_id.as_deref() {
        Some(id) => k2_core::email_notify::get_sender(&conn, id).map_err(db_fail)?,
        None => k2_core::email_notify::app_sender(&conn, app).map_err(db_fail)?,
    };
    Ok(json!({
        "app": app,
        "on": st.enabled && box_enabled(),
        "enabled": st.enabled,
        "boxSwitch": if box_enabled() { "on" } else { "off" },
        "approved": cap_approved(app),
        "sender": sender.as_ref().map(sender_json),
        "recipientScope": st.recipient_scope,
        "turnedOnAt": st.turned_on_at,
        "turnedOnBy": st.turned_on_by,
        "offReason": st.off_reason,
    }))
}

/// Did the owner approve `email:send` for this App (its runtime App row's
/// approved set)? A `--cmd` service has none: `k2 app email on` (with its
/// confirm) is the owner's approval there.
pub fn cap_approved(service_id: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(Some(app)) = k2_core::deploy::store::get_app_by_service(&conn, service_id) else { return false };
    matches!(crate::apps::grants::recorded(&conn, &app.app_id), Ok(Some(set)) if set.iter().any(|c| c == k2_core::skin::CAP_EMAIL_SEND))
}

/// An App went away (`publish rm`, `k2 app uninstall`): its state goes and
/// its sender returns to the owner's list (not retired: the owner decides).
pub fn forget_app(service_id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = k2_core::email_notify::forget_app(&conn, service_id, now()) {
        k2_core::log_debug!("[email-notify] forget App {service_id}: WARN {e}");
    }
}

/// An App user was removed: their log rows go (E22).
pub fn forget_recipient(address: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match k2_core::email_notify::purge_recipient(&conn, address) {
        Ok(n) if n > 0 => k2_core::log_debug!("[email-notify] purged {n} log row(s) for a removed App user"),
        Ok(_) => {}
        Err(e) => k2_core::log_debug!("[email-notify] WARN purge for a removed App user: {e}"),
    }
}
