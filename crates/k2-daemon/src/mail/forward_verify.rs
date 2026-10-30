//! The confirm link for an outside forward destination
//! (prd-hostmail-external-forwarding-v1 §3.2 state 1; vs-live FW-A16;
//! slice V of prd-email-notifications-part2-v1, purpose `forward`).
//!
//! Nothing is forwarded to an outside address until its owner pressed
//! Confirm on the page the link opens. The store is
//! `k2_core::email_verify` (subject = the forwarding mailbox's address);
//! the pages are `email_notify::verify` (GET renders, only POST confirms,
//! so link scanners can't confirm).
//!
//! **The mail.** K2's fixed text (`verify::compose_verify`): which server,
//! who asked, that it's for copies of the mail arriving at the mailbox,
//! one 24 h confirm link and a stop link. From the box's system sender;
//! a box without one sends it **from the mailbox itself** (Rosson
//! 2026-10-28, vs-live Q8), by delegation, with no stored copy. Never App
//! or agent text.
//!
//! **Hooks.** [`on_confirmed`] turns the mailbox's `pending_verify`
//! forward to that address `active` (and writes the Sieve). [`on_stopped`]
//! (the stop link, any time later) pauses an active forward with reason
//! `revoked`, or removes one still waiting. A revoked destination can only
//! come back through a new `set` and a new confirm.

use std::collections::BTreeMap;

use k2_core::email_verify as vstore;
use k2_core::mail_forward as store;
use serde_json::json;

use crate::email_notify::{self, compose, fail, verify, Fail, R};

const P: &str = "[mail/forward-verify]";

/// How the confirm mail went out.
pub const VIA_SYSTEM: &str = "system";
pub const VIA_MAILBOX: &str = "mailbox";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Started {
    /// The address already confirmed for this mailbox: no mail sent.
    AlreadyVerified(vstore::Verified),
    /// A confirm mail went out (`via`: [`VIA_SYSTEM`] | [`VIA_MAILBOX`]).
    Sent { row: vstore::Verified, via: &'static str },
}

impl Started {
    pub fn verified_row(&self) -> &vstore::Verified {
        match self {
            Started::AlreadyVerified(v) => v,
            Started::Sent { row, .. } => row,
        }
    }
}

fn issue_fail(e: vstore::IssueError) -> Fail {
    match e {
        vstore::IssueError::AlreadyVerified(_) => fail(409, "already_verified", "that address is already confirmed"),
        vstore::IssueError::Limit(code, hint) => {
            let mut f = fail(429, code, hint);
            if code == "rate_limited" {
                f.retry_after_ms = Some(3_600_000);
            }
            f
        }
        vstore::IssueError::Bad(why) => fail(400, "usage", why),
        vstore::IssueError::Db(e) => email_notify::db_fail(e),
    }
}

/// Ask `dest` to confirm it wants copies of `mailbox`'s mail. `account_id`
/// is the mailbox's mail-server account (the fallback sender). `asker`:
/// `owner` | a person key | `agent:<name>`.
pub fn start(mailbox: &str, account_id: Option<&str>, dest: &str, asker: &str, now: i64) -> R<Started> {
    // The address already confirmed for this mailbox: nothing to send.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let live = vstore::live_for(&conn, vstore::PURPOSE_FORWARD, mailbox, now).map_err(email_notify::db_fail)?;
        if let Some(v) = live.into_iter().find(|v| v.is_verified() && v.address == vstore::normalize_address(dest)) {
            return Ok(Started::AlreadyVerified(v));
        }
    }
    if email_notify::airgap_blocks() {
        return Err(fail(409, "air_gap", "air-gap is on: nothing leaves this server, so no outside forward can start"));
    }
    // Refuse before a row exists when the links can't be built.
    verify::link(verify::VERIFY_PATH, "x")?;
    let system = email_notify::send::sender_for(None).ok().flatten();
    if system.is_none() && account_id.is_none() {
        return Err(fail(409, "no_mail_path", "this mailbox has no mail-server account to send the confirm email from"));
    }
    let issued = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        vstore::issue(&conn, vstore::PURPOSE_FORWARD, mailbox, dest, asker, now).map_err(issue_fail)?
    };
    let box_label = crate::app_user_invites::box_label();
    let composed = verify::compose_verify(
        &box_label,
        &verify::who(asker, &box_label),
        &verify::why(vstore::PURPOSE_FORWARD, mailbox),
        &issued.row.address,
        &verify::link(verify::VERIFY_PATH, &issued.token)?,
        &verify::link(verify::STOP_PATH, &issued.stop)?,
    );
    let sent = match &system {
        Some(s) => email_notify::send::system_send_from(s, verify::PURPOSE_MAIL, None, &issued.row.address, &composed).map(|_| VIA_SYSTEM),
        None => send_from_mailbox(mailbox, account_id.unwrap_or_default(), &issued.row.address, &issued.row.id, &composed).map(|_| VIA_MAILBOX),
    };
    match sent {
        Ok(via) => {
            k2_core::log_debug!("{P} confirm mail for row {} sent ({via})", issued.row.id);
            Ok(Started::Sent { row: issued.row, via })
        }
        Err(f) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = vstore::revoke(&conn, &issued.row.id, "send_failed", now);
            Err(f)
        }
    }
}

/// FW-A16: the confirm mail from the mailbox itself (no system sender),
/// by delegation, with no stored copy.
fn send_from_mailbox(mailbox: &str, account_id: &str, to: &str, row_id: &str, c: &email_notify::send::Composed) -> R<()> {
    if account_id.is_empty() {
        return Err(fail(409, "no_mail_path", "this mailbox has no mail-server account to send the confirm email from"));
    }
    let engine = email_notify::engine::mail_engine()?;
    let to_list = vec![to.to_string()];
    let mid = crate::mail::send::new_message_id_for(mailbox);
    let none = BTreeMap::new();
    let m = compose::Message {
        from_name: "K2",
        from: mailbox,
        to: &to_list,
        cc: &[],
        subject: &c.subject,
        text: Some(&c.text),
        html: None,
        reply_to: &[],
        headers: &none,
        message_id: &mid,
    };
    engine
        .submit(&email_notify::engine::HostedMail {
            account_id,
            from: mailbox,
            rcpt: &to_list,
            email: compose::email_json(&m),
            params: compose::envelope_params(row_id),
            destroy_copy: true,
        })
        .map_err(|e| {
            k2_core::log_debug!("{P} confirm mail for row {row_id} refused upstream: {}", email_notify::send::mask_addresses(&e));
            fail(502, "failed", email_notify::send::UPSTREAM_HINT)
        })
}

/// Tell the owner an outside forward just started (no approval asked:
/// Rosson 2026-10-30; this is the record). FW-A23: an agent mailbox's
/// forwarded mail skips agent-send approval and always-BCC, so it says so.
pub fn notice_activated(f: &store::Forward) {
    if !f.is_external() || !f.is_active() {
        return;
    }
    let title = format!("Forwarding to an outside address is on for {}", f.address);
    let who = if f.set_by.starts_with("agent:") {
        format!("the AI agent {}", f.set_by.trim_start_matches("agent:"))
    } else if let Some(p) = f.set_by.strip_prefix("person:") {
        p.to_string()
    } else {
        f.set_by.clone()
    };
    let body = format!(
        "{} now forwards its incoming mail to {} ({}), set by {who} and confirmed by the destination. \
         Forwarded mail leaves this server without agent-send approval or BCC. Spam, bounces and mail \
         that fails DMARC stay in the mailbox. Stop it with: k2 hostmail forward unset {}",
        f.address,
        f.dest,
        if f.keep { "keeps a copy" } else { "no copy kept" },
        f.address
    );
    if let Err(e) = crate::feedback_routes::file_owner_notice(k2_core::db::AUDIT_SENTINEL_IDS[0], "k2-email", &title, &body) {
        k2_core::log_debug!("{P} WARN owner notice for forward {}: {e}", f.id);
    }
}

/// The destination pressed Confirm: its mailbox's waiting forward starts.
pub fn on_confirmed(v: &vstore::Verified) {
    let waiting = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::live_for_verified(&conn, &v.id).unwrap_or_default()
    };
    for f in waiting.into_iter().filter(|f| {
        f.state == store::ST_PENDING_VERIFY && f.is_external() && f.address == v.subject_ref && f.dest == v.address
    }) {
        let change = store::Change { verified_address_id: Some(&v.id), actor: "recipient", ..Default::default() };
        match super::forward_sync::transition_and_sync(&f.id, store::ST_ACTIVE, &change) {
            Ok(row) => {
                k2_core::log_debug!("{P} forward {} confirmed and active", f.id);
                notice_activated(&row);
            }
            Err(e) => k2_core::log_debug!("{P} forward {} confirmed: {}", f.id, email_notify::send::mask_addresses(&e.to_string())),
        }
    }
}

/// The destination pressed Stop: K2 forwards nothing more there.
pub fn on_stopped(v: &vstore::Verified) {
    let live = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::live_for_verified(&conn, &v.id).unwrap_or_default()
    };
    for f in live {
        let r = match f.state.as_str() {
            store::ST_PENDING_VERIFY => super::forward_sync::transition_and_sync(
                &f.id,
                store::ST_REMOVED,
                &store::Change { actor: "recipient", detail: Some(json!({ "why": "stopped_before_confirm" })), ..Default::default() },
            ),
            _ => super::forward_sync::pause(&f.id, store::P_REVOKED, "recipient", None),
        };
        if let Err(e) = r {
            k2_core::log_debug!("{P} forward {} stop: {}", f.id, email_notify::send::mask_addresses(&e.to_string()));
        }
    }
}
