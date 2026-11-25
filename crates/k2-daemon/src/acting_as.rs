//! Who a terminal acts as (0.46.18, App role papercut 1).
//!
//! A K2 terminal carries its workspace's passport, so everything typed there
//! acts as that workspace's agent, even when the owner types it. When an
//! App or role route (`/cli/skin/…`, `/cli/app/…`) refuses a passport as
//! the owner's (`owner_only`) or needs the App-users switch
//! (`app_users_off`), the refusal says so plainly:
//!
//! - `error.actingAs`: `agent:<handle>`;
//! - the hint ends with one sentence: "This terminal acts as agent @h …".
//!
//! Stamped in one place for both doors: TCP (`routes::http::send_response*`,
//! reading the token `routes::pass_gate` noted) and the cell socket (the
//! cell's own passport). The owner token, logins and App passes never get
//! it. By design a terminal never claims the owner: no flag, no "is a human
//! typing" guess, and a shell tab is not the owner on these routes. The
//! owner's word is a card answered in the app, or the owner token outside K2.

use std::borrow::Cow;

use serde_json::Value;

use crate::session_token::HookPrincipal;

/// The refusal codes that get the line.
const CODES: &[&str] = &["owner_only", crate::skin_routes::APP_USERS_OFF];

/// The routes it covers: App users, roles, grants and tokens, and the App
/// runtime's own (`/cli/app/…`).
pub fn covers(path: &str) -> bool {
    let p = path.split('?').next().unwrap_or(path);
    p.starts_with("/cli/skin/") || p.starts_with("/cli/app/")
}

/// The sentence the hint ends with, for agent `handle`.
pub fn sentence(handle: &str) -> String {
    format!(
        "This terminal acts as agent @{handle} (its passport), even when you type here: the owner approves in the {} app, \
         or runs it from a terminal outside {}.",
        k2_core::brand::PRODUCT_NAME,
        k2_core::brand::PRODUCT_NAME
    )
}

/// `body` with `actingAs` and the sentence, for a refusal to `principal`'s
/// passport; `None`: not one of these refusals (or stamped already).
pub fn stamp(status: &str, body: &str, principal: &HookPrincipal) -> Option<String> {
    stamp_as(status, body, || crate::skin_routes::skin_manage_actor_label(principal))
}

/// [`stamp`] with the agent's label (`agent:<handle>`) read only when needed.
fn stamp_as(status: &str, body: &str, label: impl FnOnce() -> String) -> Option<String> {
    if !status.starts_with("403") || !body.trim_start().starts_with('{') {
        return None;
    }
    let mut v: Value = serde_json::from_str(body).ok()?;
    let err = v.get_mut("error")?.as_object_mut()?;
    let code = err.get("code").and_then(Value::as_str)?;
    if !CODES.contains(&code) || err.contains_key("actingAs") {
        return None;
    }
    let label = label();
    let handle = label.strip_prefix("agent:").unwrap_or(&label).to_string();
    let line = sentence(&handle);
    let hint = match err.get("hint").and_then(Value::as_str) {
        Some(h) if !h.trim().is_empty() => format!("{} {line}", h.trim_end()),
        _ => line,
    };
    err.insert("hint".into(), Value::String(hint));
    err.insert("actingAs".into(), Value::String(format!("agent:{handle}")));
    Some(v.to_string())
}

/// TCP: the response about to go out, for the passport the request
/// carried (noted by `routes::pass_gate` on a covered route only).
pub(crate) fn stamp_tcp<'a>(status: &str, body: &'a str) -> Cow<'a, str> {
    if !status.starts_with("403") {
        return Cow::Borrowed(body);
    }
    let stamped = crate::routes::pass_gate::request_hook_token()
        .and_then(|t| crate::session_token::validate_hook(&t))
        .and_then(|v| stamp(status, body, &v.principal));
    match stamped {
        Some(s) => Cow::Owned(s),
        None => Cow::Borrowed(body),
    }
}

/// Cell socket: the verb's answer, for this cell's passport (the cell
/// socket is Unix only).
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn stamp_cell(status: &str, path: &str, body: &str, principal: &HookPrincipal) -> Option<String> {
    if !covers(path) {
        return None;
    }
    stamp(status, body, principal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sales(status: &str, body: &str) -> Option<String> {
        stamp_as(status, body, || "agent:sales".to_string())
    }

    #[test]
    fn an_owner_only_refusal_names_the_agent_this_terminal_acts_as() {
        let body = r#"{"ok":false,"error":{"code":"owner_only","hint":"only the owner changes it"}}"#;
        let out = sales("403 Forbidden", body).expect("stamped");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["actingAs"], "agent:sales", "{v}");
        let hint = v["error"]["hint"].as_str().unwrap();
        assert!(hint.starts_with("only the owner changes it This terminal acts as agent @sales (its passport), even when you type here"), "{hint}");
        assert_eq!(v["error"]["code"], "owner_only", "the code stays");
        // Once only.
        assert!(sales("403 Forbidden", &out).is_none());
    }

    #[test]
    fn app_users_off_gets_it_too_and_nothing_else_does() {
        let off = r#"{"error":{"code":"app_users_off","hint":"needs the switch"}}"#;
        assert!(sales("403 Forbidden", off).is_some());
        for (status, body) in [
            ("403 Forbidden", r#"{"error":{"code":"forbidden","hint":"x"}}"#),
            ("403 Forbidden", r#"{"error":"Invalid or missing auth token"}"#),
            ("200 OK", r#"{"error":{"code":"owner_only","hint":"x"}}"#),
            ("409 Conflict", r#"{"error":{"code":"owner_only","hint":"x"}}"#),
            ("403 Forbidden", "not json"),
        ] {
            assert!(sales(status, body).is_none(), "{status} {body}");
        }
    }

    #[test]
    fn it_covers_app_and_role_routes_only() {
        for p in ["/cli/skin/roles/room", "/cli/skin/users?x=1", "/cli/app/approve"] {
            assert!(covers(p), "{p}");
        }
        for p in ["/cli/tickets", "/cli/skin", "/cli/apps", "/cli/msg"] {
            assert!(!covers(p), "{p}");
        }
    }
}
