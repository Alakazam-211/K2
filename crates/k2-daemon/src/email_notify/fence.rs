//! The fence around notification senders (prd-email-notifications-core-v1
//! §3.1).
//!
//! The structural half is E1: a sender is never a `mail_addresses` row, so
//! every Hosted Mail verb that loads one (read, access grant, set-primary,
//! ACL share, app password, password rotate, forward, alias-onto, import,
//! quota, the Apple profile) answers `not_found` for it, to the owner too.
//! This module is the rest:
//!
//! - **E2** ([`reserved`]): mint, alias add, list create, group create and
//!   catch-all set refuse a sender's address (live, or reserved for
//!   30 days after removal) with `409 reserved`.
//! - **E4** (Rosson R1, [`domain_gate`], [`server_gate`],
//!   [`is_notification`]): on a domain carrying a sender, a mail-manage
//!   agent can't change the send mode, retire or rotate a DKIM key,
//!   redirect DMARC reports, or drop/retry a queued notification, and can't
//!   stop the mail server or set its PTR/HELO name while any hosted sender
//!   exists; the queue list shows it a notification's recipient count,
//!   never the addresses, and its approvals list leaves held App email out.
//!   No agent loosens `agentSend` on a workspace whose App sends email
//!   ([`agent_send_gate`]; security review L4). The owner and Admins keep
//!   every power.
//!
//! Every check here fails closed: an unreadable store answers "fenced".

use rusqlite::Connection;

use crate::cli_response::CliResponse;

fn err_json(status: &'static str, code: &str, hint: String) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string(),
    }
}

/// [`reserved`] on a connection the caller already holds.
pub fn reserved_conn(conn: &Connection, address: &str) -> bool {
    let a = address.trim().to_ascii_lowercase();
    if a.is_empty() {
        return false;
    }
    k2_core::email_notify::is_reserved(conn, &a).unwrap_or(true)
}

/// E2: is `address` a notification sender's (live or still reserved)?
/// An unreadable store answers yes (fail-closed).
pub fn reserved(address: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    reserved_conn(&conn, address)
}

pub fn reserved_hint(address: &str) -> String {
    format!(
        "{} is a notification sender's address (k2 notifications email sender list): it can't also be a mailbox, \
         an alias, a list, a group or a catch-all. A removed sender's address stays reserved for 30 days.",
        address.trim().to_ascii_lowercase()
    )
}

/// `409 reserved`.
pub fn reserved_response(address: &str) -> CliResponse {
    err_json("409 Conflict", "reserved", reserved_hint(address))
}

/// `403 owner_only` for a fenced step.
pub fn owner_only(what: &str) -> CliResponse {
    err_json(
        "403 Forbidden",
        "owner_only",
        format!(
            "Notification mail (K2's own password resets and the Apps' email) depends on this, so only the owner or an \
             Admin can {what} (k2 notifications email sender list shows what it carries). Ask your human."
        ),
    )
}

/// Is `domain` carrying a live notification sender? Fail-closed.
pub fn domain_carries_sender(domain: &str) -> bool {
    let d = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if d.is_empty() {
        return false;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::email_notify::domain_has_sender(&conn, &d).unwrap_or(true)
}

/// May `domain`'s DKIM/autoconfig rows go in a PARENT zone (Email
/// Notifications E11: a sending subdomain without its own zone)?
/// Security review L10: only with the box switch on AND either the
/// domain carries a live sender, or the caller is the owner/Admin (no
/// agent passport). A mail-manage agent adding `x.brand.com` gets the
/// exact zone only, as before 0.46.5. Call it with no db lock held.
pub fn parent_zone_allowed(domain: &str) -> bool {
    if !super::box_enabled() {
        return false;
    }
    if matches!(crate::mail::agent_creds::mail_caller(), crate::mail::agent_creds::MailCaller::Owner) {
        return true;
    }
    // Fail closed the other way from the fence: an unreadable store means
    // the exact zone only.
    let d = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::email_notify::domain_has_sender(&conn, &d).unwrap_or(false)
}

/// Does any live hosted sender exist on this box? Fail-closed.
fn any_hosted_sender() -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match k2_core::email_notify::list_senders(&conn, false) {
        Ok(list) => list.iter().any(|s| s.path == k2_core::email_notify::PATH_HOSTED),
        Err(_) => true,
    }
}

/// E4: the owner and Admins pass; an agent (mail-manage or not) is
/// refused when `domain` carries a sender.
pub fn domain_gate(domain: &str, what: &str) -> Result<(), CliResponse> {
    if !crate::mail::agent_creds::mail_caller().is_agent() {
        return Ok(());
    }
    if domain_carries_sender(domain) {
        return Err(owner_only(what));
    }
    Ok(())
}

/// E4: stopping the mail server while a hosted sender exists.
pub fn server_gate(what: &str) -> Result<(), CliResponse> {
    if !crate::mail::agent_creds::mail_caller().is_agent() {
        return Ok(());
    }
    if any_hosted_sender() {
        return Err(owner_only(what));
    }
    Ok(())
}

/// Does the workspace at `path` own a published App whose email is on?
/// Fail-closed.
pub fn workspace_sends_app_email(path: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COUNT(*) FROM email_app_state e \
         JOIN published_services p ON p.id = e.app_service_id \
         JOIN projects w ON w.id = p.project_id \
         WHERE w.path = ?1 AND e.enabled = 1",
        rusqlite::params![path],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(true)
}

/// How open an `agentSend` value is (`off` < `approval` < `on`); `None`
/// for anything else.
fn send_rank(v: &str) -> Option<u8> {
    match v.trim().to_ascii_lowercase().as_str() {
        "off" => Some(0),
        "approval" => Some(1),
        "on" => Some(2),
        _ => None,
    }
}

/// Security review L4: an agent (a mail-manage IT agent included) can't
/// loosen `agentSend` on a workspace whose App sends email: that setting
/// is the gate on the App agent's sends (E15). Narrowing stays open.
pub fn agent_send_gate(workspace_path: &str, new_value: &str) -> Result<(), CliResponse> {
    if !crate::mail::agent_creds::mail_caller().is_agent() || !workspace_sends_app_email(workspace_path) {
        return Ok(());
    }
    let now = match crate::mail::send::gate_for_path(workspace_path) {
        Ok(crate::mail::send::Gate::Off) => 0,
        Ok(crate::mail::send::Gate::Approval) => 1,
        Ok(crate::mail::send::Gate::On) => 2,
        Err(_) => 0,
    };
    match send_rank(new_value) {
        Some(r) if r <= now => Ok(()),
        _ => Err(owner_only("loosen the email sending setting of a workspace whose App sends email")),
    }
}

/// Is a queued message (by its return path, E9: a notification's is its
/// sender's address) a notification? Fail-closed.
pub fn is_notification(return_path: Option<&str>) -> bool {
    let Some(rp) = return_path.map(|r| r.trim().trim_start_matches('<').trim_end_matches('>').to_ascii_lowercase()) else {
        return false;
    };
    if rp.is_empty() || !rp.contains('@') {
        return false;
    }
    // A VERP-style return path (N6) still names its sender: drop a +tag.
    let base = match rp.split_once('@') {
        Some((local, domain)) => format!("{}@{domain}", local.split('+').next().unwrap_or(local)),
        None => rp.clone(),
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    reserved_conn(&conn, &rp) || reserved_conn(&conn, &base)
}

/// How many recipients a queued message's `recipients` value names
/// (a set object or a list).
pub fn count_recipients(v: &serde_json::Value) -> usize {
    match v {
        serde_json::Value::Object(m) => m.len(),
        serde_json::Value::Array(a) => a.len(),
        _ => 0,
    }
}
