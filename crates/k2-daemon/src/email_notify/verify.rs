//! Verified addresses (prd-email-notifications-part2-v1 §4 V, vs-live
//! EN-A5, EN-A26): before K2 mails an address on someone's behalf, the
//! address's owner clicks a confirm link. The store is
//! `k2_core::email_verify` (k2.db 0244).
//!
//! ```text
//! GET  /cli/notify/email/address          a person: their notice address (and pending ones)
//! POST /cli/notify/email/address/set      {address}            a person: send a confirm link
//! POST /cli/notify/email/address/remove   {address?, person?}  a person (their own); the owner or an
//!                                                              Admin (`person`: someone else's)
//! GET  /cli/email/verify?t=k2ev_…         PUBLIC: a page with a Confirm button (never confirms)
//! POST /cli/email/verify                  PUBLIC: t= (form or query) confirms, once
//! GET  /cli/email/stop?t=k2es_…           PUBLIC: a page with a Stop button
//! POST /cli/email/stop?t=k2es_…           PUBLIC: stops (RFC 8058 one-click: body ignored)
//! ```
//!
//! - **The mail** is one fixed K2 text from the system sender
//!   (`system_send`, purpose `verify_address`): which server, who asked,
//!   why, one confirm link (24 h) and a stop link. Never App or agent text.
//!   No system sender: `409 no_mail_path`. The links point at the box's
//!   front door (the same rules as the set-password page); none:
//!   `409 no_front_door`.
//! - **Link scanners** (Outlook Safe Links, Gmail prefetch) follow `GET`s,
//!   so a `GET` only renders. Only the `POST` confirms or stops. The pages
//!   load nothing and run no script (CSP `default-src 'none'`), and are
//!   `no-store`, `DENY`, `no-referrer`.
//! - **Limits** (the store): 3 pending per person, 10 confirm mails per box
//!   per hour, so a link can't be used to spam an address.
//! - Agents never reach the person routes: `/cli/notify/*` resolves a
//!   person from the token (owner token or a Connect login) and refuses
//!   passports.
//!
//! A mailbox forward (the forwarding PRD) uses the same store with purpose
//! `forward`; Rosson 2026-10-28: on a box with no system sender its
//! confirm may come from the mailbox itself. That path is
//! `mail::forward_verify`; a confirm or a stop of a `forward` row calls
//! its hooks (after this module lets go of the db).

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::email_verify as store;
use serde_json::{json, Value as J};

use super::{fail, respond, Fail, R};
use crate::cli_response::CliResponse;

pub const ADDRESS_PATH: &str = "/cli/notify/email/address";
pub const ADDRESS_SET_PATH: &str = "/cli/notify/email/address/set";
pub const ADDRESS_REMOVE_PATH: &str = "/cli/notify/email/address/remove";
pub const VERIFY_PATH: &str = "/cli/email/verify";
pub const STOP_PATH: &str = "/cli/email/stop";

/// The log purpose of the confirm mail.
pub const PURPOSE_MAIL: &str = "verify_address";

pub fn is_person_route(p: &str) -> bool {
    p == ADDRESS_PATH || p == ADDRESS_SET_PATH || p == ADDRESS_REMOVE_PATH || p == super::agent_notices::MUTE
}

pub fn is_public_page(p: &str) -> bool {
    p == VERIFY_PATH || p == STOP_PATH
}

/// What an address is for, in the mail and on the page.
pub(crate) fn why(purpose: &str, subject_ref: &str) -> String {
    match purpose {
        store::PURPOSE_FORWARD => format!("copies of the mail that arrives at {subject_ref}"),
        _ => "notices from this server's AI agents (for example \"the nightly build failed\")".to_string(),
    }
}

/// Who asked, in words (never a token or an id). `agent:<name>` is an AI
/// agent on the box (a mailbox forward set by a mail-manage agent).
pub(crate) fn who(person: &str, box_label: &str) -> String {
    if person == k2_core::notify_prefs::OWNER {
        format!("The owner of {box_label}")
    } else if let Some(agent) = person.strip_prefix("agent:") {
        let clean: String = agent.chars().filter(|c| !c.is_control()).take(64).collect();
        format!("The AI agent {clean} on {box_label}")
    } else {
        let clean: String = person.chars().filter(|c| !c.is_control()).take(64).collect();
        format!("{clean}, a person on {box_label},")
    }
}

/// The confirm mail (fixed K2 text).
pub fn compose_verify(box_label: &str, asker: &str, why: &str, address: &str, confirm_url: &str, stop_url: &str) -> super::send::Composed {
    let subject = format!("Confirm this address for K2 on {box_label}");
    let text = format!(
        "{asker} asked K2 to send {why} to this address ({address}).\n\n\
         To allow it, open this link and press Confirm. The link works for 24 hours:\n{confirm_url}\n\n\
         If you didn't expect this, ignore this email: nothing else is sent here unless the address is confirmed.\n\
         To stop K2 from emailing this address for this request, open:\n{stop_url}\n\n\
         K2 on {box_label}\n"
    );
    super::send::Composed { subject: subject.chars().filter(|c| !c.is_control()).take(180).collect(), text, html: None }
}

/// The front-door URL for `path` with `?t=token`.
pub(crate) fn link(path: &str, token: &str) -> R<String> {
    match crate::app_user_invites::k2_front_door_url(path) {
        Ok(Some(u)) => Ok(format!("{u}?t={token}")),
        Ok(None) => Err(fail(409, "no_front_door", "this server has no front door URL for the confirm link (Settings → Apps → front door, or connect the server)")),
        Err(why) => Err(fail(409, "no_front_door", format!("the confirm link can't use this server's front door: {why}"))),
    }
}

fn issue_fail(e: store::IssueError) -> Fail {
    match e {
        store::IssueError::AlreadyVerified(_) => fail(409, "already_verified", "that address is already confirmed"),
        store::IssueError::Limit(code, hint) => {
            let mut f = fail(429, code, hint);
            if code == "rate_limited" {
                f.retry_after_ms = Some(3_600_000);
            }
            f
        }
        store::IssueError::Bad(why) => fail(400, "usage", why),
        store::IssueError::Db(e) => super::db_fail(e),
    }
}

fn row_json(v: &store::Verified) -> J {
    json!({
        "id": v.id,
        "address": v.address,
        "purpose": v.purpose,
        "state": v.state,
        "createdAt": v.created_at,
        "expiresAt": if v.state == store::ST_PENDING { json!(v.expires_at) } else { J::Null },
        "verifiedAt": v.verified_at,
    })
}

/// Ask `address` to confirm it may get mail for (purpose, subject). Sends
/// the confirm mail from the system sender. A failed send leaves no
/// pending row behind.
pub fn request(purpose: &str, subject_ref: &str, address: &str, asker_person: &str, now: i64) -> R<J> {
    let address = super::compose::strict_address(address)?;
    if super::fence::reserved(&address) {
        return Err(fail(409, "sender_address", "that is one of this server's notification senders; give an address a person reads"));
    }
    // Refuse before a row exists when no mail can go out.
    let sender = super::send::sender_for(None)?
        .ok_or_else(|| fail(409, "no_mail_path", "this server has no system sender, so it can't send the confirm email (k2 notifications email system-sender set)"))?;
    link(VERIFY_PATH, "x")?;
    let issued = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::issue(&conn, purpose, subject_ref, &address, asker_person, now).map_err(issue_fail)?
    };
    let box_label = crate::app_user_invites::box_label();
    let composed = compose_verify(
        &box_label,
        &who(asker_person, &box_label),
        &why(purpose, subject_ref),
        &issued.row.address,
        &link(VERIFY_PATH, &issued.token)?,
        &link(STOP_PATH, &issued.stop)?,
    );
    match super::send::system_send_from(&sender, PURPOSE_MAIL, None, &issued.row.address, &composed) {
        Ok(log_id) => {
            super::audit_line("email-verify-sent", asker_person, &format!("row={} purpose={purpose} log={log_id}", issued.row.id));
            Ok(json!({ "ok": true, "sent": true, "address": row_json(&issued.row) }))
        }
        Err(f) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = store::revoke(&conn, &issued.row.id, "send_failed", now);
            Err(f)
        }
    }
}

fn body_json(b: &[u8]) -> R<J> {
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

fn person_view(person: &str, now: i64) -> R<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let live = store::live_for(&conn, store::PURPOSE_PERSON_NOTICE, person, now).map_err(super::db_fail)?;
    let verified = live.iter().find(|v| v.is_verified()).map(row_json);
    let pending: Vec<J> = live.iter().filter(|v| v.state == store::ST_PENDING).map(row_json).collect();
    let muted = k2_core::email_notify::notice_mutes(&conn, person).map_err(super::db_fail)?;
    Ok(json!({ "ok": true, "person": person, "address": verified, "pending": pending, "mutedWorkspaces": muted }))
}

fn remove(person: &str, role: Role, actor_person: &str, v: &J, now: i64) -> R<J> {
    let target = s(v, "person").unwrap_or(person).to_string();
    if target != person && role < Role::Admin {
        return Err(fail(403, "forbidden", "you can remove only your own notice address (the owner or an Admin can remove anyone's)"));
    }
    let only = s(v, "address").map(store::normalize_address);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let live = store::live_for(&conn, store::PURPOSE_PERSON_NOTICE, &target, now).map_err(super::db_fail)?;
    let mut removed = 0;
    for row in live.iter().filter(|r| only.as_deref().is_none_or(|a| a == r.address)) {
        if store::revoke(&conn, &row.id, actor_person, now).map_err(super::db_fail)? {
            removed += 1;
        }
    }
    if removed == 0 {
        return Err(fail(404, "not_found", "no notice address to remove"));
    }
    super::audit_line("email-verify-removed", actor_person, &format!("person={target} rows={removed}"));
    Ok(json!({ "ok": true, "removed": removed }))
}

/// `/cli/notify/email/address*` for a person (the dispatcher resolved
/// `person` from the token; `role` is the login's role, Owner for the
/// owner token).
pub fn person_route(is_post: bool, path: &str, body: &[u8], person: &str, role: Role, params: &HashMap<String, String>) -> CliResponse {
    let now = super::now();
    let post_only = path == ADDRESS_SET_PATH || path == ADDRESS_REMOVE_PATH || path == super::agent_notices::MUTE;
    if post_only && !is_post {
        return CliResponse::method_not_allowed();
    }
    if path == ADDRESS_PATH && is_post {
        return CliResponse::method_not_allowed();
    }
    let r = match path {
        ADDRESS_PATH => {
            let target = params.get("person").map(|p| p.trim()).filter(|p| !p.is_empty());
            match target {
                Some(t) if t != person && role < Role::Admin => {
                    Err(fail(403, "forbidden", "you can read only your own notice address (Admins read anyone's)"))
                }
                Some(t) => person_view(t, now),
                None => person_view(person, now),
            }
        }
        ADDRESS_SET_PATH => body_json(body).and_then(|v| {
            let a = s(&v, "address").ok_or_else(|| fail(400, "usage", "send {address}"))?;
            request(store::PURPOSE_PERSON_NOTICE, person, a, person, now)
        }),
        ADDRESS_REMOVE_PATH => body_json(body).and_then(|v| remove(person, role, person, &v, now)),
        p if p == super::agent_notices::MUTE => body_json(body).and_then(|v| super::agent_notices::mute(person, &v, now)),
        _ => Err(fail(404, "not_found", "no such route")),
    };
    respond(r)
}

// ── the public pages ────────────────────────────────────────────────

/// One public page answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub status: &'static str,
    pub html: String,
}

/// The pages' CSP: nothing loads, no script runs, the form posts only
/// to this origin.
pub const PAGE_CSP: &str =
    "default-src 'none'; style-src 'unsafe-inline'; img-src 'none'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'";

/// The full HTTP response for a page (the daemon and the gateway both
/// send exactly this).
pub fn page_response(p: &Page) -> String {
    format!(
        "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Content-Security-Policy: {PAGE_CSP}\r\nReferrer-Policy: no-referrer\r\nCache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nConnection: close\r\n\r\n{}",
        p.status,
        p.html.len(),
        p.html
    )
}

fn esc(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&#39;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

fn html(title: &str, body: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<meta name="robots" content="noindex">
<title>{t}</title>
<style>
:root {{ color-scheme: light dark; --bg:#f6f6f4; --fg:#1d1d1b; --muted:#6b6b66; --card:#fff; --line:#dcdcd6; --accent:#2f5bd3; }}
@media (prefers-color-scheme: dark) {{ :root {{ --bg:#111214; --fg:#ececea; --muted:#9a9a94; --card:#1b1c1f; --line:#2c2d31; --accent:#7c9cff; }} }}
body {{ margin:0; min-height:100vh; display:grid; place-items:center; padding:16px; background:var(--bg); color:var(--fg);
  font:16px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }}
main {{ width:min(440px, 100%); background:var(--card); border:1px solid var(--line); border-radius:14px; padding:24px; }}
h1 {{ font-size:20px; margin:0 0 8px; }}
p {{ margin:0 0 16px; color:var(--muted); }}
strong {{ color:var(--fg); word-break:break-all; }}
button {{ width:100%; font:600 16px/1 inherit; padding:14px; border-radius:10px; border:1px solid var(--accent);
  background:var(--accent); color:#fff; cursor:pointer; }}
</style>
</head>
<body><main>{body}</main></body>
</html>
"#,
        t = esc(title)
    )
}

fn form(action: &str, token: &str, button: &str) -> String {
    // `token` passed `token_shape_ok` (base64url only); escaped anyway.
    format!(
        r#"<form method="post" action="{}"><input type="hidden" name="t" value="{}"><button type="submit">{}</button></form>"#,
        esc(action),
        esc(token),
        esc(button)
    )
}

fn gone(status: &'static str, what: &str) -> Page {
    Page { status, html: html("Link not valid", &format!("<h1>This link doesn't work any more</h1><p>{}</p>", esc(what))) }
}

/// The token from `?t=`, a form body `t=`, or a JSON body `{"token"}`.
fn token_from(query: &str, body: &[u8]) -> Option<String> {
    let mut found = None;
    for pair in query.split('&').chain(std::str::from_utf8(body).unwrap_or("").split('&')) {
        if let Some(v) = pair.trim().strip_prefix("t=") {
            found = Some(v.trim().to_string());
        }
    }
    if found.is_none() {
        if let Ok(v) = serde_json::from_slice::<J>(body) {
            found = v.get("token").and_then(J::as_str).map(|s| s.trim().to_string());
        }
    }
    found.filter(|t| !t.is_empty())
}

/// `GET /cli/email/verify` and `GET /cli/email/stop`: render only.
pub fn page_get(path: &str, query: &str) -> Page {
    let now = super::now();
    let token = token_from(query, b"").unwrap_or_default();
    let box_label = crate::app_user_invites::box_label();
    let db = k2_core::db::shared();
    let conn = db.lock();
    match path {
        VERIFY_PATH => match store::peek(&conn, &token, now) {
            Ok(Some(v)) => Page {
                status: "200 OK",
                html: html(
                    "Confirm your address",
                    &format!(
                        "<h1>Confirm this address</h1><p>{} asked K2 to send {} to <strong>{}</strong>.</p>\
                         <p>Press Confirm to allow it. You can stop these emails later from a link in each one.</p>{}",
                        esc(&who(&v.requested_by, &box_label)),
                        esc(&why(&v.purpose, &v.subject_ref)),
                        esc(&v.address),
                        form(VERIFY_PATH, &token, "Confirm")
                    ),
                ),
            },
            Ok(None) => gone("404 Not Found", "It was already used, it expired (links work for 24 hours), or it was replaced by a newer one."),
            Err(_) => gone("500 Internal Server Error", "K2 couldn't read its records. Try again later."),
        },
        _ => match store::peek_stop(&conn, &token) {
            Ok(Some(v)) if v.state == store::ST_PENDING || v.state == store::ST_VERIFIED => Page {
                status: "200 OK",
                html: html(
                    "Stop these emails",
                    &format!(
                        "<h1>Stop K2's emails to this address?</h1><p>K2 on {} will stop sending {} to <strong>{}</strong>.</p>{}",
                        esc(&box_label),
                        esc(&why(&v.purpose, &v.subject_ref)),
                        esc(&v.address),
                        form(STOP_PATH, &token, "Stop these emails")
                    ),
                ),
            },
            Ok(Some(v)) => Page {
                status: "200 OK",
                html: html("Already stopped", &format!("<h1>Already stopped</h1><p>K2 sends nothing more to <strong>{}</strong> for this.</p>", esc(&v.address))),
            },
            Ok(None) => gone("404 Not Found", "K2 doesn't know this stop link."),
            Err(_) => gone("500 Internal Server Error", "K2 couldn't read its records. Try again later."),
        },
    }
}

/// `POST /cli/email/verify` and `POST /cli/email/stop`.
pub fn page_post(path: &str, query: &str, body: &[u8]) -> Page {
    let mut hook: Option<(bool, store::Verified)> = None;
    let page = page_post_inner(path, query, body, &mut hook);
    // A mailbox forward follows its address (outside the db lock).
    match hook {
        Some((true, v)) if v.purpose == store::PURPOSE_FORWARD => crate::mail::forward_verify::on_confirmed(&v),
        Some((false, v)) if v.purpose == store::PURPOSE_FORWARD => crate::mail::forward_verify::on_stopped(&v),
        _ => {}
    }
    page
}

fn page_post_inner(path: &str, query: &str, body: &[u8], hook: &mut Option<(bool, store::Verified)>) -> Page {
    let now = super::now();
    let token = token_from(query, body).unwrap_or_default();
    let db = k2_core::db::shared();
    let conn = db.lock();
    match path {
        VERIFY_PATH => match store::confirm(&conn, &token, now) {
            Ok(v) => {
                super::audit_line("email-verify-confirmed", "recipient", &format!("row={} purpose={}", v.id, v.purpose));
                *hook = Some((true, v.clone()));
                Page {
                    status: "200 OK",
                    html: html(
                        "Address confirmed",
                        &format!(
                            "<h1>Confirmed</h1><p><strong>{}</strong> will get {}. Every email has a link to stop them.</p>",
                            esc(&v.address),
                            esc(&why(&v.purpose, &v.subject_ref))
                        ),
                    ),
                }
            }
            Err(store::ConfirmError::Expired) => gone("404 Not Found", "It expired (links work for 24 hours). Ask for a new one."),
            Err(store::ConfirmError::Invalid) => gone("404 Not Found", "It was already used, or it was replaced by a newer one."),
            Err(store::ConfirmError::Db(_)) => gone("500 Internal Server Error", "K2 couldn't save this. Try again later."),
        },
        _ => match store::stop(&conn, &token, now) {
            Ok(Some(v)) => {
                super::audit_line("email-verify-stopped", "recipient", &format!("row={} purpose={}", v.id, v.purpose));
                *hook = Some((false, v.clone()));
                Page {
                    status: "200 OK",
                    html: html("Stopped", &format!("<h1>Stopped</h1><p>K2 sends nothing more to <strong>{}</strong> for this.</p>", esc(&v.address))),
                }
            }
            Ok(None) => gone("404 Not Found", "K2 doesn't know this stop link."),
            Err(_) => gone("500 Internal Server Error", "K2 couldn't save this. Try again later."),
        },
    }
}

/// Hourly: expire due links; forget rows dead for 90 days.
pub fn hourly(now: i64) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = store::expire_due(&conn, now) {
        k2_core::log_debug!("[email-notify] WARN verify expiry: {e}");
    }
    if let Err(e) = store::purge_dead(&conn, now - 90 * 86_400) {
        k2_core::log_debug!("[email-notify] WARN verify purge: {e}");
    }
}
