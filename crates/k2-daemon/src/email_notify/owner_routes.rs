//! `/cli/notifications/email/*`: the box's senders, system sender, relays,
//! log and settings (`k2 notifications email …`). Owner and Admins only: an
//! agent passport (and a K2 shell tab's) is `owner_only`, and the path is
//! on the passports' deny list. Steps that widen what leaves the box
//! (adding a sender or a relay, choosing the system sender, a test send)
//! need a fresh owner confirm on top, every caller, the owner token
//! included (agents can read it).
//!
//! ```text
//! GET  /cli/notifications/email                       status: switch, mail path, system sender, senders, relays, settings
//! GET  /cli/notifications/email/senders               ?all=1 (retired too)
//! POST /cli/notifications/email/sender/add            {address, name?, nameLocked?, replyTo?, relay?, confirmToken}   owner + confirm
//! POST /cli/notifications/email/sender/set            {sender, name?, nameLocked?, replyTo?}
//! POST /cli/notifications/email/sender/check          {sender}
//! POST /cli/notifications/email/sender/rm             {sender}
//! GET  /cli/notifications/email/system-sender
//! POST /cli/notifications/email/system-sender/set     {sender, confirmToken}                                        owner + confirm
//! POST /cli/notifications/email/system-sender/clear   {}
//! POST /cli/notifications/email/system-sender/test    {confirmToken}                                                owner + confirm
//! GET  /cli/notifications/email/relays
//! POST /cli/notifications/email/relay/add             {host, port?, tls?, username, password, fromDomain, confirmToken} owner + confirm
//! POST /cli/notifications/email/relay/test            {relay, from, confirmToken}                                   owner + confirm
//! POST /cli/notifications/email/relay/rm              {relay}
//! GET  /cli/notifications/email/log                   ?app=&limit=&status=
//! POST /cli/notifications/email/settings              {logRetentionDays?, ownerEmail?}                              owner
//! ```

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::email_notify as store;
use serde_json::{json, Value as J};

use super::{db_fail, fail, respond, sender_json, Fail, R};
use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

pub const STATUS: &str = "/cli/notifications/email";
pub const SENDERS: &str = "/cli/notifications/email/senders";
pub const SENDER_ADD: &str = "/cli/notifications/email/sender/add";
pub const SENDER_SET: &str = "/cli/notifications/email/sender/set";
pub const SENDER_CHECK: &str = "/cli/notifications/email/sender/check";
pub const SENDER_RM: &str = "/cli/notifications/email/sender/rm";
pub const SYSTEM: &str = "/cli/notifications/email/system-sender";
pub const SYSTEM_SET: &str = "/cli/notifications/email/system-sender/set";
pub const SYSTEM_CLEAR: &str = "/cli/notifications/email/system-sender/clear";
pub const SYSTEM_TEST: &str = "/cli/notifications/email/system-sender/test";
pub const RELAYS: &str = "/cli/notifications/email/relays";
pub const RELAY_ADD: &str = "/cli/notifications/email/relay/add";
pub const RELAY_TEST: &str = "/cli/notifications/email/relay/test";
pub const RELAY_RM: &str = "/cli/notifications/email/relay/rm";
pub const LOG: &str = "/cli/notifications/email/log";
pub const SETTINGS: &str = "/cli/notifications/email/settings";

pub const ROUTES: &[&str] = &[
    STATUS, SENDERS, SENDER_ADD, SENDER_SET, SENDER_CHECK, SENDER_RM, SYSTEM, SYSTEM_SET, SYSTEM_CLEAR, SYSTEM_TEST, RELAYS, RELAY_ADD, RELAY_TEST, RELAY_RM,
    LOG, SETTINGS,
];
pub const POST_ROUTES: &[&str] = &[SENDER_ADD, SENDER_SET, SENDER_CHECK, SENDER_RM, SYSTEM_SET, SYSTEM_CLEAR, SYSTEM_TEST, RELAY_ADD, RELAY_TEST, RELAY_RM, SETTINGS];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

// ── callers ──────────────────────────────────────────────────────────

/// The owner token, or an Owner or Admin login. Never a passport (an
/// agent's, or a K2 shell tab's: agents share that workspace).
fn admin(who: &Who) -> R<()> {
    match who {
        Who::Owner | Who::Login { role: Role::Owner | Role::Admin, .. } => Ok(()),
        Who::Invalid => Err(fail(403, "forbidden", "invalid or missing token")),
        Who::Wrong => Err(fail(403, "wrong_credential", "App passes, API keys and API sessions can't manage Email Notifications")),
        _ => Err(fail(
            403,
            "owner_only",
            "Email Notifications (senders, the system sender, relays) are the owner's and Admins', from a terminal outside K2 \
             or Settings. An agent asks with a permission card.",
        )),
    }
}

/// The owner in person (the owner token or an Owner login).
fn owner(who: &Who) -> R<()> {
    admin(who)?;
    match who {
        Who::Owner | Who::Login { role: Role::Owner, .. } => Ok(()),
        _ => Err(fail(403, "owner_only", "this step is the owner's")),
    }
}

fn actor(who: &Who) -> String {
    crate::apps::routes::actor(who)
}

/// A fresh vault confirm of `action` bound to `secret_id`, from the owner
/// in person. Without one: `403 vault_confirm_required` naming what the
/// CLI must confirm.
fn confirm(who: &Who, v: &J, action: &str, secret_id: &str) -> R<()> {
    owner(who)?;
    let stamp = match who {
        Who::Owner => "owner".to_string(),
        Who::Login { username, .. } => format!("connect:{username}"),
        _ => String::new(),
    };
    let Some(token) = v.get("confirmToken").and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(Fail {
            status: 403,
            code: "vault_confirm_required",
            hint: format!("this needs a fresh owner confirm (Touch ID, your login or the vault password) for {action} {secret_id}; k2 notifications email asks for it from a terminal outside K2"),
            retry_after_ms: None,
        });
    };
    let vault = crate::vault::get().map_err(|_| fail(503, "vault_closed", "the vault isn't open yet"))?;
    vault
        .confirm
        .take(token, action, Some(secret_id), &stamp)
        .map_err(|_| fail(403, "vault_confirm_required", "confirm again: a confirm works once, for this step, within 60 seconds"))?;
    Ok(())
}

/// The `vault_confirm_required` refusal carries the action and the id the
/// CLI binds its confirm to.
fn confirm_needed(f: Fail, action: &str, secret_id: &str) -> CliResponse {
    let mut r = super::fail_response(&f);
    if f.code == "vault_confirm_required" {
        if let Ok(mut v) = serde_json::from_str::<J>(&r.body) {
            v["error"]["action"] = json!(action);
            v["error"]["secretId"] = json!(secret_id);
            r.body = v.to_string();
        }
    }
    r
}

fn body(b: &[u8]) -> R<J> {
    if b.iter().all(|c| c.is_ascii_whitespace()) {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<J>(b) {
        Ok(v @ J::Object(_)) => Ok(v),
        _ => Err(fail(400, "usage", "the body is a JSON object")),
    }
}

fn s<'a>(v: &'a J, k: &str) -> Option<&'a str> {
    v.get(k).and_then(J::as_str).map(str::trim).filter(|x| !x.is_empty())
}

fn str_list(v: &J, k: &str) -> R<Vec<String>> {
    match v.get(k) {
        None | Some(J::Null) => Ok(Vec::new()),
        Some(J::String(x)) => Ok(x.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()),
        Some(J::Array(a)) => a.iter().map(|x| x.as_str().map(str::to_string).ok_or_else(|| fail(400, "usage", format!("{k} is a list of strings")))).collect(),
        Some(_) => Err(fail(400, "usage", format!("{k} is a list of strings"))),
    }
}

// ── handlers ─────────────────────────────────────────────────────────

fn status() -> R<J> {
    let senders = super::senders::list(false)?;
    let relays = super::relay::list()?;
    let (system, settings) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (store::system_sender(&conn).map_err(db_fail)?, store::settings(&conn).map_err(db_fail)?)
    };
    Ok(json!({
        "ok": true,
        "boxSwitch": if super::box_enabled() { "on" } else { "off" },
        "hostedMail": super::hosted_supported(),
        "airgap": super::airgap_blocks(),
        "systemSender": system.as_ref().map(sender_json),
        "senders": senders.iter().map(sender_json).collect::<Vec<_>>(),
        "relays": relays.iter().map(super::relay::relay_json).collect::<Vec<_>>(),
        "settings": settings,
        "agpl": AGPL_NOTE,
    }))
}

/// E23: where Stalwart's source is (printed by status, the study page and
/// `k2 hostmail status`).
pub const AGPL_NOTE: &str = "Hosted mail runs Stalwart 0.16.25 (AGPL-3.0-only), unmodified, from github.com/Alakazam-211/stalwart \
     (tag v0.16.25, which mirrors upstream stalwartlabs/stalwart); source: https://github.com/Alakazam-211/stalwart/tree/v0.16.25";

fn sender_add(who: &Who, v: &J) -> Result<J, CliResponse> {
    let raw = s(v, "address").ok_or_else(|| super::fail_response(&fail(400, "usage", "send {address} (e.g. noreply@notify.brand.com)")))?;
    let (address, _, _) = super::senders::normalize(raw).map_err(|f| super::fail_response(&f))?;
    let args = super::senders::AddArgs {
        address: address.clone(),
        name: s(v, "name").map(str::to_string),
        name_locked: v.get("nameLocked").and_then(J::as_bool).unwrap_or(false),
        reply_to: str_list(v, "replyTo").map_err(|f| super::fail_response(&f))?,
        relay: s(v, "relay").map(str::to_string),
    };
    confirm(who, v, super::CONFIRM_SENDER, &address).map_err(|f| confirm_needed(f, super::CONFIRM_SENDER, &address))?;
    super::senders::add(&args, &actor(who), super::now()).map_err(|f| super::fail_response(&f))
}

/// `sender set`: the From name, its lock and the default Reply-To. Any
/// change needs a fresh owner confirm bound to the sender's address
/// (security review L5: agents can read the owner token, and these decide
/// what K2's resets say and where replies to an App's mail go).
fn sender_set(who: &Who, v: &J) -> Result<J, CliResponse> {
    let r = |f: Fail| super::fail_response(&f);
    admin(who).map_err(r)?;
    let sender = super::senders::find_live(s(v, "sender").ok_or_else(|| r(fail(400, "usage", "send {sender, name?, nameLocked?, replyTo?}")))?).map_err(r)?;
    let name = match s(v, "name") {
        Some(n) if super::compose::encoded_word(n) => return Err(r(fail(400, "usage", "the name can't hold an encoded word (=?…?=)"))),
        Some(n) => crate::mail::send::validate_from_name(n).map_err(|_| r(fail(400, "usage", "the name isn't valid (no @, no control characters, at most 128)")))?,
        None => sender.from_name.clone(),
    };
    let locked = v.get("nameLocked").and_then(J::as_bool).unwrap_or(sender.from_name_locked);
    let reply_to = if v.get("replyTo").is_some() {
        let raw = str_list(v, "replyTo").map_err(r)?;
        let mut l: Vec<String> = Vec::new();
        for a in &raw {
            let a = super::compose::strict_address(a).map_err(|_| r(fail(400, "usage", "replyTo has an address that isn't valid")))?;
            if !l.contains(&a) {
                l.push(a);
            }
        }
        if l.len() > super::compose::MAX_REPLY_TO {
            return Err(r(fail(400, "usage", "at most 3 reply-to addresses")));
        }
        l
    } else {
        sender.reply_to.clone()
    };
    if name != sender.from_name || locked != sender.from_name_locked || reply_to != sender.reply_to {
        confirm(who, v, super::CONFIRM_SENDER, &sender.address).map_err(|f| confirm_needed(f, super::CONFIRM_SENDER, &sender.address))?;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::set_sender_name(&conn, &sender.id, &name, locked, &reply_to, super::now()).map_err(|e| r(db_fail(e)))?;
    let after = store::get_sender(&conn, &sender.id).map_err(|e| r(db_fail(e)))?.ok_or_else(|| r(db_fail("the sender vanished")))?;
    drop(conn);
    super::audit_line("email-sender-set", &actor(who), &format!("sender={}", sender.id));
    Ok(json!({ "ok": true, "sender": sender_json(&after) }))
}

fn system_set(who: &Who, v: &J) -> Result<J, CliResponse> {
    let raw = s(v, "sender").ok_or_else(|| super::fail_response(&fail(400, "usage", "send {sender} (an id or address from k2 notifications email sender list)")))?;
    let sender = super::senders::find_live(raw).map_err(|f| super::fail_response(&f))?;
    confirm(who, v, super::CONFIRM_SYSTEM_SENDER, &sender.address).map_err(|f| confirm_needed(f, super::CONFIRM_SYSTEM_SENDER, &sender.address))?;
    super::senders::set_system(&sender.id, &actor(who), super::now()).map_err(|f| super::fail_response(&f))
}

/// The fixed text of `system-sender test`.
pub const SYSTEM_TEST_SUBJECT: &str = "Test email from K2";

fn system_test(who: &Who, v: &J) -> Result<J, CliResponse> {
    let sys = super::senders::system()
        .map_err(|f| super::fail_response(&f))?
        .ok_or_else(|| super::fail_response(&fail(409, "no_sender", "no system sender yet (k2 notifications email system-sender set <sender>)")))?;
    confirm(who, v, super::CONFIRM_TEST, &sys.address).map_err(|f| confirm_needed(f, super::CONFIRM_TEST, &sys.address))?;
    let owner_email = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::settings(&conn).map_err(|e| super::fail_response(&db_fail(e)))?.owner_email
    };
    let to = owner_email.ok_or_else(|| {
        super::fail_response(&fail(409, "no_owner_email", "set your own address first: k2 notifications email settings --owner-email you@example.com (tests only ever go there)"))
    })?;
    let c = super::send::Composed { subject: SYSTEM_TEST_SUBJECT.to_string(), text: super::relay::TEST_TEXT.to_string(), html: None };
    let id = super::send::send_fixed(&sys, store::ORIGIN_SYSTEM, "test", None, &to, &c, &super::budget::Kind::System).map_err(|f| super::fail_response(&f))?;
    super::audit_line("email-system-sender-test", &actor(who), &format!("log={id}"));
    Ok(json!({
        "ok": true,
        "id": id,
        "to": to,
        "result": "accepted for delivery",
        // E13: what K2 can honestly say. It can't read your inbox, so it
        // never claims SPF/DKIM/DMARC results there.
        "checks": test_checks(&sys),
    }))
}

/// E13: submission accepted, the DNS rows K2 can see, nothing claimed
/// about the receiving inbox.
fn test_checks(s: &store::Sender) -> J {
    let dmarc = match super::dmarc::check(&s.domain, super::now()) {
        Ok(f) => json!({ "ok": true, "at": f.at, "policy": f.policy }),
        Err(f) => json!({ "ok": false, "hint": f.hint }),
    };
    let (dkim, spf) = if s.path == store::PATH_HOSTED {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match crate::mail::domains::load_domain(&conn, &s.domain) {
            Some(row) => {
                let rows = crate::mail::domains::dns_status_of(&row).records;
                let valid = |pred: &dyn Fn(&crate::mail::domains::RecordRow) -> bool| rows.iter().any(|r| pred(r) && r.status == "valid");
                (json!(valid(&|r| r.id.starts_with("dkim:"))), json!(valid(&|r| r.id == "spf")))
            }
            None => (json!(false), json!(false)),
        }
    } else {
        (J::Null, J::Null)
    };
    json!({
        "submission": "accepted",
        "dmarc": dmarc,
        "dkimPublished": dkim,
        "spfPublished": spf,
        "note": if s.path == store::PATH_HOSTED {
            "K2's mail server signs DKIM for this domain; check the headers in your inbox for the receiving server's verdict"
        } else {
            "the relay provider signs; check the headers in your inbox for the receiving server's verdict"
        },
    })
}

fn relay_add(who: &Who, v: &J) -> Result<J, CliResponse> {
    let args = super::relay::AddArgs {
        host: s(v, "host").unwrap_or_default().to_string(),
        port: v.get("port").and_then(J::as_u64).map(|p| p.min(65535) as u16).unwrap_or(0),
        tls: s(v, "tls").unwrap_or_default().to_string(),
        username: s(v, "username").unwrap_or_default().to_string(),
        password: v.get("password").and_then(J::as_str).unwrap_or_default().to_string(),
        from_domain: s(v, "fromDomain").unwrap_or_default().to_string(),
    };
    let checked = super::relay::validate(&args).map_err(|f| super::fail_response(&f))?;
    // L11: the confirm binds the host, the From domain and the login.
    let binding = super::relay_binding(&checked.host, &checked.from_domain, &checked.username);
    confirm(who, v, super::CONFIRM_RELAY, &binding).map_err(|f| confirm_needed(f, super::CONFIRM_RELAY, &binding))?;
    super::relay::add(&checked, &actor(who), super::now()).map_err(|f| super::fail_response(&f))
}

fn relay_test(who: &Who, v: &J) -> Result<J, CliResponse> {
    let relay = s(v, "relay").ok_or_else(|| super::fail_response(&fail(400, "usage", "send {relay, from}")))?.to_string();
    let from = s(v, "from").ok_or_else(|| super::fail_response(&fail(400, "usage", "send {relay, from} (from = a sender on that relay)")))?;
    let sender = super::senders::find_live(from).map_err(|f| super::fail_response(&f))?;
    confirm(who, v, super::CONFIRM_TEST, &sender.address).map_err(|f| confirm_needed(f, super::CONFIRM_TEST, &sender.address))?;
    let mut out = super::relay::test(&relay, &sender.id, &actor(who), super::now()).map_err(|f| super::fail_response(&f))?;
    let after = super::senders::find_live(&sender.id).map_err(|f| super::fail_response(&f))?;
    out["checks"] = test_checks(&after);
    Ok(out)
}

fn log(who: &Who, params: &HashMap<String, String>) -> R<J> {
    admin(who)?;
    let limit = params.get("limit").and_then(|l| l.parse::<usize>().ok()).unwrap_or(50).clamp(1, 500);
    let app = params.get("app").map(|a| a.trim()).filter(|a| !a.is_empty());
    let status = params.get("status").map(|a| a.trim()).filter(|a| !a.is_empty());
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_log(&conn, &store::LogFilter { app, status, tag: None, before: None, limit, without_system: false }).map_err(db_fail)?
    };
    let items: Vec<J> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "sender": r.sender_id,
                "origin": r.origin,
                "app": r.app_service_id,
                "purpose": r.purpose,
                "to": r.to,
                "recipients": r.recipient_count,
                "subject": r.subject,
                "tags": r.tags,
                "status": r.status,
                "error": r.error_code,
                "createdAt": r.created_at,
            })
        })
        .collect();
    Ok(json!({ "ok": true, "items": items }))
}

fn settings(who: &Who, v: &J) -> R<J> {
    owner(who)?;
    let now = super::now();
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Some(d) = v.get("logRetentionDays") {
        let days = d.as_i64().ok_or_else(|| fail(400, "usage", "logRetentionDays is a whole number of days (7–90)"))?;
        store::set_retention(&conn, days, now).map_err(|e| fail(400, "usage", e))?;
    }
    if let Some(e) = v.get("ownerEmail") {
        let addr = match e.as_str().map(str::trim) {
            None | Some("") => None,
            Some(a) => Some(crate::mail::addresses::normalize_address(a).map_err(|_| fail(400, "usage", "ownerEmail isn't a valid address"))?),
        };
        store::set_owner_email(&conn, addr.as_deref(), now).map_err(db_fail)?;
    }
    let st = store::settings(&conn).map_err(db_fail)?;
    drop(conn);
    super::audit_line("email-settings", &actor(who), &format!("retention={} ownerEmail={}", st.log_retention_days, st.owner_email.is_some()));
    Ok(json!({ "ok": true, "settings": st }))
}

fn either(r: Result<J, CliResponse>) -> CliResponse {
    match r {
        Ok(v) => respond(Ok(v)),
        Err(resp) => resp,
    }
}

pub fn handle(path: &str, who: Who, is_post: bool, body_bytes: &[u8], params: &HashMap<String, String>) -> CliResponse {
    if is_post_route(path) && !is_post {
        return CliResponse::method_not_allowed();
    }
    if let Err(f) = admin(&who) {
        return super::fail_response(&f);
    }
    let v = if is_post {
        match body(body_bytes) {
            Ok(v) => v,
            Err(f) => return super::fail_response(&f),
        }
    } else {
        J::Null
    };
    let id_of = |k: &str| s(&v, k).map(str::to_string).ok_or_else(|| fail(400, "usage", format!("send {{{k}}}")));
    match path {
        STATUS => respond(status()),
        SENDERS => respond(super::senders::list(params.get("all").is_some_and(|a| a == "1" || a == "true")).map(|l| json!({ "ok": true, "senders": l.iter().map(sender_json).collect::<Vec<_>>() }))),
        SENDER_ADD => either(sender_add(&who, &v)),
        SENDER_SET => either(sender_set(&who, &v)),
        SENDER_CHECK => respond(id_of("sender").and_then(|id| super::senders::recheck(&id, super::now()))),
        SENDER_RM => respond(id_of("sender").and_then(|id| super::senders::remove(&id, &actor(&who), super::now()))),
        SYSTEM => respond(super::senders::system().map(|s| json!({ "ok": true, "systemSender": s.as_ref().map(sender_json) }))),
        SYSTEM_SET => either(system_set(&who, &v)),
        SYSTEM_CLEAR => respond(super::senders::clear_system(&actor(&who), super::now())),
        SYSTEM_TEST => either(system_test(&who, &v)),
        RELAYS => respond(super::relay::list().map(|l| json!({ "ok": true, "relays": l.iter().map(super::relay::relay_json).collect::<Vec<_>>() }))),
        RELAY_ADD => either(relay_add(&who, &v)),
        RELAY_TEST => either(relay_test(&who, &v)),
        RELAY_RM => respond(id_of("relay").and_then(|id| super::relay::remove(&id, &actor(&who)))),
        LOG => respond(log(&who, params)),
        SETTINGS => respond(settings(&who, &v)),
        _ => super::fail_response(&fail(404, "not_found", "no such Email Notifications route")),
    }
}
