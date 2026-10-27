//! SMTP relays (prd-email-notifications-core-v1 §3.4, E12, N3): how a box
//! without hosted mail (every Mac) sends notifications, through the
//! owner's own provider (SES, Postmark, Resend SMTP, Mailgun, Google
//! Workspace relay…). Generic SMTP only (Q7).
//!
//! - The password lives in the vault's system namespace (scope
//!   `engine:email-notify`, name `relay-<id>`), never in a column and
//!   never in the 0600 file store a closed vault falls back to elsewhere:
//!   a closed vault answers `503 vault_closed` and writes nothing.
//! - K2 can't see whether the provider signs DKIM for the domain, so a
//!   relay sender is `needs_test` until one test through the relay (to the
//!   owner's own address) is accepted.
//! - The password never rides an error or a log line.

use serde_json::{json, Value as J};

use k2_core::email_notify::{self as store, Relay};

use super::{db_fail, fail, Fail, R};

/// What `relay add` takes.
#[derive(Clone, Default)]
pub struct AddArgs {
    pub host: String,
    pub port: u16,
    pub tls: String,
    pub username: String,
    pub password: String,
    pub from_domain: String,
}

impl std::fmt::Debug for AddArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AddArgs")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("from_domain", &self.from_domain)
            .finish()
    }
}

fn usage(h: impl Into<String>) -> Fail {
    fail(400, "usage", h)
}

pub fn relay_json(r: &Relay) -> J {
    json!({
        "id": r.id,
        "host": r.host,
        "port": r.port,
        "tls": r.tls,
        "username": r.username,
        "fromDomain": r.from_domain,
        "lastTestAt": r.last_test_at,
        "lastTestOk": r.last_test_ok,
        "createdAt": r.created_at,
    })
}

fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && h.split('.').all(|l| !l.is_empty() && l.len() <= 63 && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// Check what `relay add` was given (no network).
pub fn validate(a: &AddArgs) -> R<AddArgs> {
    let host = a.host.trim().trim_end_matches('.').to_ascii_lowercase();
    if !host_ok(&host) || !host.contains('.') {
        return Err(usage("host is the provider's SMTP host name (e.g. email-smtp.eu-west-1.amazonaws.com)"));
    }
    let tls = match a.tls.trim() {
        "" | "implicit" | "tls" | "ssl" => "implicit",
        "starttls" => "starttls",
        _ => return Err(usage("tls is implicit (port 465) or starttls (port 587)")),
    };
    let port = if a.port == 0 { if tls == "implicit" { 465 } else { 587 } } else { a.port };
    if port == 25 {
        return Err(usage("relays use submission (465 or 587), not port 25"));
    }
    let username = a.username.trim().to_string();
    if username.is_empty() || username.len() > 256 || username.chars().any(char::is_control) {
        return Err(usage("username is the relay login (1–256 characters)"));
    }
    if a.password.is_empty() || a.password.len() > 1024 || a.password.contains(['\r', '\n']) {
        return Err(usage("password is the relay's SMTP password or API key (no line breaks)"));
    }
    let from_domain = k2_core::mail_domain::normalize_mail_domain(&a.from_domain).map_err(|_| usage("fromDomain is the domain this relay may send for"))?;
    Ok(AddArgs { host, port, tls: tls.to_string(), username, password: a.password.clone(), from_domain })
}

/// `relay add`: the vault first (a closed vault writes nothing), then the
/// row. The caller is the owner with a fresh confirm.
pub fn add(a: &AddArgs, actor: &str, now: i64) -> R<J> {
    let a = validate(a)?;
    let creds = super::engine::relay_creds();
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::insert_relay(&conn, &a.host, a.port, &a.tls, &a.username, &a.from_domain, now).map_err(db_fail)?
    };
    if let Err(e) = creds.put(&row.id, a.password.as_bytes()) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = store::delete_relay(&conn, &row.id);
        return Err(e.to_fail());
    }
    super::audit_line("email-relay-add", actor, &format!("relay={} host={} port={}", row.id, row.host, row.port));
    Ok(json!({ "ok": true, "relay": relay_json(&row), "next": format!("k2 notifications email sender add <address on {}> --relay {}", row.from_domain, row.id) }))
}

pub fn list() -> R<Vec<Relay>> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::list_relays(&conn).map_err(db_fail)
}

/// `relay rm`: refused while a live sender uses it.
pub fn remove(id: &str, actor: &str) -> R<J> {
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if store::get_relay(&conn, id).map_err(db_fail)?.is_none() {
            return Err(fail(404, "not_found", format!("no relay {id}")));
        }
        if store::relay_in_use(&conn, id).map_err(db_fail)? {
            return Err(fail(409, "in_use", format!("relay {id} carries a sender; remove the sender first (k2 notifications email sender rm)")));
        }
    }
    super::engine::relay_creds().destroy(id).map_err(|e| e.to_fail())?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::delete_relay(&conn, id).map_err(db_fail)?;
    drop(conn);
    super::audit_line("email-relay-rm", actor, &format!("relay={id}"));
    Ok(json!({ "ok": true, "removed": id }))
}

/// Submit one composed message through `relay`. Errors never carry the
/// password.
pub fn submit(relay: &Relay, from: &str, rcpt: &[String], rfc822: &[u8]) -> R<()> {
    let password = match super::engine::relay_creds().get(&relay.id) {
        Ok(Some(p)) => p,
        Ok(None) => return Err(fail(409, "relay_not_ready", format!("relay {}'s login is missing; add the relay again", relay.id))),
        Err(e) => return Err(e.to_fail()),
    };
    super::engine::relay_transport().submit(relay, &password, from, rcpt, rfc822).map_err(|e| {
        let clean = if password.is_empty() { e } else { e.replace(&password, "<redacted>") };
        fail(502, "failed", format!("the relay didn't take the message: {clean}"))
    })
}

/// `relay test <relay> --from <sender>`: one fixed message from `from`
/// (a sender on this relay) to the owner's own address. On success the
/// relay's test mark and that sender go ready.
pub fn test(relay_id: &str, from: &str, actor: &str, now: i64) -> R<J> {
    if super::airgap_blocks() {
        return Err(fail(503, "airgap", "air-gap is on: nothing leaves this server"));
    }
    let (relay, sender, owner) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let relay = store::get_relay(&conn, relay_id).map_err(db_fail)?.ok_or_else(|| fail(404, "not_found", format!("no relay {relay_id}")))?;
        let sender = match store::find_sender(&conn, from).map_err(db_fail)? {
            Some(s) if s.is_live() && s.relay_id.as_deref() == Some(relay.id.as_str()) => s,
            _ => return Err(usage(format!("--from is a sender on relay {relay_id} (k2 notifications email sender add <address> --relay {relay_id})"))),
        };
        let owner = store::settings(&conn).map_err(db_fail)?.owner_email;
        (relay, sender, owner)
    };
    let Some(owner) = owner else {
        return Err(fail(409, "no_owner_email", "set your own address first: k2 notifications email settings --owner-email you@example.com (tests only ever go there)"));
    };
    let mid = crate::mail::send::new_message_id_for(&sender.address);
    let to = vec![owner.clone()];
    let headers = Default::default();
    let raw = super::compose::rfc822(
        &super::compose::Message {
            from_name: &sender.from_name,
            from: &sender.address,
            to: &to,
            cc: &[],
            subject: "Test email from K2",
            text: Some(TEST_TEXT),
            html: None,
            reply_to: &[],
            headers: &headers,
            message_id: &mid,
        },
        std::time::SystemTime::now(),
    )
    .map_err(|e| fail(500, "failed", e))?;
    // The log row first (no row, no send), like every notification.
    let log_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::insert_log(
            &conn,
            &store::NewLog {
                sender_id: &sender.id,
                origin: store::ORIGIN_SYSTEM,
                app_service_id: None,
                purpose: Some("relay_test"),
                to: &to,
                subject: "",
                tags: &[],
                status: store::ST_QUEUED,
                error_code: None,
                message_id: None,
                send_after: None,
                scope_any: false,
                now,
            },
        )
        .map_err(db_fail)?
    };
    let r = submit(&relay, &sender.address, &to, &raw);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match &r {
            Ok(()) => store::set_log_submitted(&conn, &log_id, store::ST_QUEUED, &mid, now).map(|_| ()),
            Err(f) => store::set_log_status(&conn, &log_id, store::ST_QUEUED, store::ST_FAILED, Some(f.code), now).map(|_| ()),
        }
        .map_err(db_fail)?;
        store::record_relay_test(&conn, &relay.id, r.is_ok(), now).map_err(db_fail)?;
        if r.is_ok() && sender.state == store::STATE_NEEDS_TEST {
            store::set_sender_state(&conn, &sender.id, store::STATE_READY, now).map_err(db_fail)?;
        }
    }
    super::audit_line("email-relay-test", actor, &format!("relay={} sender={} ok={}", relay.id, sender.id, r.is_ok()));
    r?;
    Ok(json!({ "ok": true, "relay": relay.id, "sender": sender.id, "to": owner, "result": "accepted by the relay" }))
}

/// The fixed text of every K2 test email.
pub const TEST_TEXT: &str = "This is a test email from K2. If it reached you, this sender works.\n\nNo reply is needed.";
