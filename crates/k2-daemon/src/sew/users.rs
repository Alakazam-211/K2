//! Sew's own users on a **standalone** install (`k2 hostchat users …`,
//! 0.46.0 demo; Sew docs `mgmt.md` "Standalone local users"). The daemon
//! calls Sew's mgmt socket as itself (its uid is in `[mgmt] allow_uids`);
//! no root helper verb is involved. A K2-identity install answers
//! `403 provider_is_k2` (users come from K2 grants there, S27).
//!
//! Setup links (`setup_link.url`, a one-time password token in the URL
//! fragment, 24 h) are returned to the caller only: never logged, never in
//! an event, an audit line, an error or a progress record.
//!
//! - owner: `POST /mgmt/v1/local/setup-link {username, owner: true,
//!   display_name}` (the first install owner; `409 owner_exists` after)
//! - invite: `POST /mgmt/v1/local/users {username, display_name,
//!   install_role, email?}`
//! - reset: `POST /mgmt/v1/local/setup-link {username}` (voids older links)
//! - list: `GET /mgmt/v1/principals` (paged)
//! - revoke: `POST /mgmt/v1/principals/deprovision {external_id:
//!   "local:<u>"}`; reactivate: `POST /mgmt/v1/local/users/reactivate`
//! - role: `POST /mgmt/v1/local/users/role {username, install_role}`

use serde_json::{json, Value};

use super::ops::MgmtError;
use super::supervisor::{self as sup, no, Env, Fail, R};

/// Sew's local handle rule: `[a-z0-9_-]{2,64}`.
pub fn username_ok(u: &str) -> bool {
    (2..=64).contains(&u.len()) && u.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// A handle from a name or an email: the email's local part, lowercased,
/// anything outside `[a-z0-9_-]` turned into `-`, runs of `-` collapsed,
/// `-`/`_` trimmed at the ends. `None` when nothing usable is left.
pub fn handle_from(input: &str) -> Option<String> {
    let base = input.trim().split('@').next().unwrap_or("");
    let mut out = String::new();
    for c in base.chars().flat_map(char::to_lowercase) {
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' { c } else { '-' };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    let out = out.trim_matches(|c| c == '-' || c == '_').chars().take(64).collect::<String>();
    let out = out.trim_end_matches(['-', '_']).to_string();
    username_ok(&out).then_some(out)
}

/// A plausible email (one `@`, a dot in the domain, no spaces).
pub fn email_ok(e: &str) -> bool {
    let e = e.trim();
    let mut parts = e.split('@');
    let (Some(l), Some(d), None) = (parts.next(), parts.next(), parts.next()) else { return false };
    !l.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.') && e.len() <= 254 && !e.chars().any(char::is_whitespace)
}

fn display_ok(d: &str) -> bool {
    let n = d.chars().count();
    (1..=100).contains(&n) && !d.chars().any(char::is_control)
}

/// The user arguments as Sew wants them: `(username, email?)`.
pub fn resolve(input: &str, email: Option<&str>) -> R<(String, Option<String>)> {
    let input = input.trim();
    let email = match email.map(str::trim).filter(|e| !e.is_empty()) {
        Some(e) if email_ok(e) => Some(e.to_ascii_lowercase()),
        Some(_) => return Err(no("bad_request", 400, "--email is not an email address")),
        None if input.contains('@') => {
            if !email_ok(input) {
                return Err(no("bad_request", 400, format!("'{input}' is not an email address")));
            }
            Some(input.to_ascii_lowercase())
        }
        None => None,
    };
    let username = handle_from(input).ok_or_else(|| {
        no("bad_request", 400, format!("'{input}' gives no Sew username: use 2–64 of a-z 0-9 _ -"))
    })?;
    Ok((username, email))
}

/// Sew's error code from a mgmt error body (`{"error":{"code"|"reason"}}`
/// or `{"code"}`).
fn sew_code(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    for p in ["/error/reason", "/error/code", "/reason", "/code", "/error"] {
        if let Some(s) = v.pointer(p).and_then(Value::as_str) {
            return s.to_string();
        }
    }
    String::new()
}

fn mgmt_fail(e: MgmtError) -> Fail {
    match e {
        MgmtError::Status(status, body) => {
            let code = sew_code(&body);
            let (k2code, http, hint): (&'static str, u16, String) = match code.as_str() {
                "provider_is_k2" => (
                    "provider_is_k2",
                    409,
                    "this Sew install uses K2 identity: its users come from K2 (chat grants), not Sew passwords. \
                     A standalone install is a fresh install: k2 hostchat enable --identity standalone"
                        .into(),
                ),
                "owner_exists" => ("owner_exists", 409, "this Sew install already has an owner: invite people with k2 hostchat users invite".into()),
                "username_taken" => ("username_taken", 409, "that username is taken: pick another (--username)".into()),
                "user_deactivated" => ("user_deactivated", 409, "that user is deactivated: k2 hostchat users reactivate <name>".into()),
                "is_owner" => ("is_owner", 409, "the install owner cannot be revoked".into()),
                "owner_role_fixed" | "guest_role_fixed" => ("role_fixed", 409, format!("Sew refused the role change ({code})")),
                "not_a_local_person" => ("not_a_local_person", 409, "that member is not a local person".into()),
                _ if status == 404 => ("no_such_user", 404, "no such Sew user".into()),
                _ if status == 400 || status == 422 => ("sew_refused", 400, format!("Sew refused it ({})", if code.is_empty() { "validation" } else { &code })),
                _ => ("sew_refused", 502, format!("Sew answered {status} ({code})")),
            };
            let mut f = no(k2code, http, hint);
            f.extra = Some(json!({"sewStatus": status, "sewCode": code}));
            f
        }
        MgmtError::Unreachable(e) => no("sew_unreachable", 503, format!("Sew's mgmt socket is unreachable: {e}")),
        MgmtError::Unauthorized => no("sew_unreachable", 503, "Sew refused K2's mgmt token"),
    }
}

fn need_running(env: &Env) -> R<()> {
    if !env.ops.linux() {
        return Err(no("unsupported", 409, "Sew runs on Linux boxes only"));
    }
    let r = super::store::load().ok_or_else(|| no("not_installed", 404, "Sew is not installed on this box (k2 hostchat enable)"))?;
    if !matches!(r.status.as_str(), "running" | "degraded") {
        return Err(no("not_running", 409, format!("Sew is '{}': users need a running server", r.status)));
    }
    Ok(())
}

fn post(env: &Env, path: &str, body: &Value) -> R<Value> {
    sup::mgmt(env, "POST", path, Some(body)).map_err(mgmt_fail)
}

/// What a `LocalUserResponse` returns to the caller (the setup link
/// included, for the caller's eyes only).
fn user_out(v: &Value) -> Value {
    json!({
        "ok": true,
        "memberId": v.get("member_id"),
        "username": v.get("username"),
        "installRole": v.get("install_role"),
        "deactivated": v.get("deactivated"),
        "created": v.get("created"),
        "reactivated": v.get("reactivated"),
        "changed": v.get("changed"),
        "setupLink": v.get("setup_link").filter(|l| !l.is_null()).map(|l| json!({"url": l.get("url"), "expiresAt": l.get("expires_at")})),
    })
}

/// The first install owner and their setup link.
pub fn owner(env: &Env, name: &str, display: Option<&str>) -> R<Value> {
    need_running(env)?;
    let (username, _) = resolve(name, None)?;
    let display = display.map(str::trim).filter(|d| !d.is_empty()).unwrap_or(name.trim());
    if !display_ok(display) {
        return Err(no("bad_request", 400, "the display name is 1–100 characters"));
    }
    let v = post(env, "/mgmt/v1/local/setup-link", &json!({"username": username, "owner": true, "display_name": display}))?;
    Ok(user_out(&v))
}

/// Invite a local user (member, admin or guest) with a setup link.
pub fn invite(env: &Env, name: &str, display: Option<&str>, role: &str, email: Option<&str>) -> R<Value> {
    need_running(env)?;
    if !matches!(role, "member" | "admin" | "guest") {
        return Err(no("bad_request", 400, "role is member, admin or guest"));
    }
    let (username, email) = resolve(name, email)?;
    let display = display.map(str::trim).filter(|d| !d.is_empty()).map(str::to_string).unwrap_or_else(|| {
        let n = name.trim();
        n.split('@').next().unwrap_or(n).to_string()
    });
    if !display_ok(&display) {
        return Err(no("bad_request", 400, "the display name is 1–100 characters"));
    }
    let mut body = json!({"username": username, "display_name": display, "install_role": role});
    if let Some(e) = email {
        body["email"] = json!(e);
    }
    let v = post(env, "/mgmt/v1/local/users", &body)?;
    Ok(user_out(&v))
}

/// A fresh setup link (password reset; older links stop working).
pub fn reset(env: &Env, name: &str) -> R<Value> {
    need_running(env)?;
    let (username, _) = resolve(name, None)?;
    let v = post(env, "/mgmt/v1/local/setup-link", &json!({"username": username}))?;
    Ok(user_out(&v))
}

pub fn revoke(env: &Env, name: &str) -> R<Value> {
    need_running(env)?;
    let (username, _) = resolve(name, None)?;
    let v = post(env, "/mgmt/v1/principals/deprovision", &json!({"external_id": format!("local:{username}")}))?;
    Ok(json!({"ok": true, "username": username, "memberId": v.get("member_id"), "sessionsRevoked": v.get("sessions_revoked")}))
}

pub fn reactivate(env: &Env, name: &str) -> R<Value> {
    need_running(env)?;
    let (username, _) = resolve(name, None)?;
    let v = post(env, "/mgmt/v1/local/users/reactivate", &json!({"username": username}))?;
    Ok(user_out(&v))
}

pub fn role(env: &Env, name: &str, role: &str) -> R<Value> {
    need_running(env)?;
    if !matches!(role, "admin" | "member") {
        return Err(no("bad_request", 400, "role is admin or member (the owner moves with an owner transfer)"));
    }
    let (username, _) = resolve(name, None)?;
    let v = post(env, "/mgmt/v1/local/users/role", &json!({"username": username, "install_role": role}))?;
    Ok(user_out(&v))
}

/// Every member (all pages; deactivated ones included).
pub fn list(env: &Env) -> R<Value> {
    need_running(env)?;
    let mut items = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..100 {
        let path = match &after {
            Some(a) => format!("/mgmt/v1/principals?limit=500&after={a}"),
            None => "/mgmt/v1/principals?limit=500".to_string(),
        };
        let page = sup::mgmt(env, "GET", &path, None).map_err(mgmt_fail)?;
        let batch = page.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
        after = batch.last().and_then(|i| i.get("member_id")).and_then(|m| m.as_str().map(str::to_string).or_else(|| Some(m.to_string())));
        for i in &batch {
            items.push(json!({
                "memberId": i.get("member_id"),
                "externalId": i.get("external_id"),
                "username": i.get("username"),
                "displayName": i.get("display_name"),
                "email": i.get("email"),
                "kind": i.get("kind"),
                "installRole": i.get("install_role"),
                "deactivated": i.get("deactivated"),
            }));
        }
        if page.get("has_more").and_then(Value::as_bool) != Some(true) || batch.is_empty() {
            return Ok(json!({"ok": true, "users": items}));
        }
    }
    Err(no("sew_refused", 502, "Sew's member list did not end after 100 pages"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_from_names_and_emails() {
        assert_eq!(handle_from("bob").as_deref(), Some("bob"));
        assert_eq!(handle_from("Rosson Long").as_deref(), Some("rosson-long"));
        assert_eq!(handle_from("ana.maria@acme.dev").as_deref(), Some("ana-maria"));
        assert_eq!(handle_from("  __xy__  ").as_deref(), Some("xy"));
        assert_eq!(handle_from("  __x__  "), None, "one character is not a Sew handle");
        assert_eq!(handle_from("Zoë!!").as_deref(), Some("zo"));
        assert_eq!(handle_from("a").as_deref(), None);
        assert_eq!(handle_from("@acme.dev"), None);
        assert_eq!(handle_from(&"x".repeat(80)).map(|h| h.len()), Some(64));
        assert!(username_ok("bob_2-x") && !username_ok("Bob") && !username_ok("b") && !username_ok("bob x"));
        let (u, e) = resolve("Carol@Acme.dev", None).unwrap();
        assert_eq!((u.as_str(), e.as_deref()), ("carol", Some("carol@acme.dev")));
        let (u, e) = resolve("carol", Some("c@acme.dev")).unwrap();
        assert_eq!((u.as_str(), e.as_deref()), ("carol", Some("c@acme.dev")));
        assert_eq!(resolve("carol", Some("not-an-email")).unwrap_err().code, "bad_request");
        assert_eq!(resolve("x@y", None).unwrap_err().code, "bad_request");
    }

    #[test]
    fn sew_refusals_map_to_k2_codes() {
        let f = mgmt_fail(MgmtError::Status(403, r#"{"error":{"code":"forbidden","reason":"provider_is_k2"}}"#.into()));
        assert_eq!((f.code, f.status), ("provider_is_k2", 409));
        let f = mgmt_fail(MgmtError::Status(409, r#"{"error":{"code":"conflict","reason":"username_taken"}}"#.into()));
        assert_eq!(f.code, "username_taken");
        let f = mgmt_fail(MgmtError::Status(409, r#"{"error":{"code":"owner_exists"}}"#.into()));
        assert_eq!(f.code, "owner_exists");
        let f = mgmt_fail(MgmtError::Status(404, r#"{"error":{"code":"not_found"}}"#.into()));
        assert_eq!((f.code, f.status), ("no_such_user", 404));
        let f = mgmt_fail(MgmtError::Unreachable("refused".into()));
        assert_eq!((f.code, f.status), ("sew_unreachable", 503));
    }
}
