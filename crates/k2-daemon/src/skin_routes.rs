//! Skin roster / passes / front-door (`/cli/skin*`).
//!
//! GET `/cli/skin/users` and GET `/cli/skin-tokens` admit owner-tier
//! (`owner_role_identity`) **or** a workspace-agent scoped hook. Mint and
//! other mutations stay `owner_role_identity` only (valid hooks get teaching
//! `owner_only`). Mutations are POST-only (GET twins 405). Raw `k2skn_…`
//! secret is returned once on create.

use crate::cli_response::CliResponse;
use k2_core::skin::{self, CAP_THREAD_POST, CAP_THREAD_READ};
use k2_core::skin_door;

fn json_body(body: &[u8]) -> Result<serde_json::Value, CliResponse> {
    match serde_json::from_slice(body) {
        Ok(v) => Ok(v),
        Err(_) if body.iter().all(|b| b.is_ascii_whitespace()) => Ok(serde_json::json!({})),
        Err(e) => Err(CliResponse::bad_request(format!("invalid JSON body: {e}"))),
    }
}

fn str_field<'a>(v: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(|x| x.as_str()) {
            let t = s.trim();
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    None
}

fn caps_field(v: &serde_json::Value) -> Option<Vec<String>> {
    v.get("caps")
        .or_else(|| v.get("capabilities"))
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
}

fn apply_field(v: &serde_json::Value) -> bool {
    match v.get("apply").or_else(|| v.get("Apply")) {
        None => true,
        Some(x) if x.is_null() => true,
        Some(x) => x.as_bool().unwrap_or(true),
    }
}

fn ui_port_field(v: &serde_json::Value, current: Option<u16>) -> Option<u16> {
    let raw = v.get("uiPort").or_else(|| v.get("ui_port"));
    match raw {
        None => current,
        Some(x) if x.is_null() => None,
        Some(x) => {
            if let Some(n) = x.as_u64() {
                u16::try_from(n).ok()
            } else if let Some(s) = x.as_str() {
                let t = s.trim();
                if t.is_empty() {
                    None
                } else {
                    t.parse().ok()
                }
            } else {
                current
            }
        }
    }
}

fn status_json() -> CliResponse {
    match skin_door::status() {
        Ok(d) => CliResponse::ok_json(serde_json::to_string(&d).unwrap_or_else(|_| "{}".into())),
        Err(e) => CliResponse::internal_error(e),
    }
}

pub fn handle_users_get() -> CliResponse {
    match skin::list_principals() {
        Ok(users) => CliResponse::ok_json(serde_json::json!({ "users": users }).to_string()),
        Err(e) => CliResponse::internal_error(e),
    }
}

pub fn handle_users_post(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username", "name"]) else {
        return CliResponse::bad_request("missing username");
    };
    match skin::add_principal(username) {
        Ok(p) => {
            k2_core::log_debug!("[skin] actor={actor} added principal {}", p.username);
            CliResponse::ok_json(serde_json::to_string(&p).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_users_remove(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username", "name"]) else {
        return CliResponse::bad_request("missing username");
    };
    match skin::remove_principal(username) {
        Ok(true) => {
            k2_core::log_debug!("[skin] actor={actor} removed principal {username}");
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Ok(false) => CliResponse::bad_request(format!("skin user '{username}' not found")),
        Err(e) => CliResponse::internal_error(e),
    }
}

pub fn handle_tokens_get() -> CliResponse {
    match skin::list_tokens() {
        Ok(tokens) => CliResponse::ok_json(serde_json::json!({ "tokens": tokens }).to_string()),
        Err(e) => CliResponse::internal_error(e),
    }
}

pub fn handle_tokens_post(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username", "name"]) else {
        return CliResponse::bad_request("missing username");
    };
    let caps = caps_field(&v);
    match skin::create_token(username, caps.as_deref()) {
        Ok((meta, raw)) => {
            k2_core::log_debug!(
                "[skin] actor={actor} minted token {} for {} caps={:?}",
                meta.id,
                meta.username,
                meta.caps
            );
            CliResponse::ok_json(
                serde_json::json!({
                    "id": meta.id,
                    "username": meta.username,
                    "prefix": meta.prefix,
                    "caps": meta.caps,
                    "createdAt": meta.created_at,
                    "token": raw,
                })
                .to_string(),
            )
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_tokens_revoke(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id) = str_field(&v, &["id", "tokenId", "token_id"]) else {
        return CliResponse::bad_request("missing id");
    };
    match skin::revoke_token(id) {
        Ok(true) => {
            k2_core::log_debug!("[skin] actor={actor} revoked token {id}");
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Ok(false) => CliResponse::ok_json(r#"{"success":false}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_front_door_get() -> CliResponse {
    status_json()
}

pub fn handle_front_door_post(body: &[u8], actor: &str, daemon_port: u16) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(mode) = str_field(&v, &["mode"]) else {
        return CliResponse::bad_request("missing mode (connect|direct)");
    };
    let url = str_field(&v, &["url"]);
    let hint = str_field(&v, &["hint"]);
    let current = skin::get_front_door().ok().and_then(|d| d.ui_port);
    let ui_port = ui_port_field(&v, current);
    let apply = apply_field(&v);
    match skin::set_front_door(mode, url, hint, ui_port) {
        Ok(d) => {
            k2_core::log_debug!(
                "[skin] actor={actor} front-door mode={} apply={apply}",
                d.mode
            );
            if apply {
                match skin_door::apply(daemon_port) {
                    Ok(st) => CliResponse::ok_json(
                        serde_json::to_string(&st).unwrap_or_else(|_| "{}".into()),
                    ),
                    Err(e) => CliResponse::bad_request(e),
                }
            } else {
                status_json()
            }
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn missing_cap_response(cap: &str) -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({ "error": format!("missing capability {cap}") }).to_string(),
    }
}

pub fn revoked_skin_response() -> CliResponse {
    CliResponse {
        status: "401 Unauthorized",
        content_type: "application/json",
        body: r#"{"error":"invalid or revoked skin token"}"#.to_string(),
    }
}

pub fn skin_terminal_forbidden() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: r#"{"error":"skin tokens cannot use the terminal"}"#.to_string(),
    }
}

/// Host belt: `Host: skin.*` must never be a kingdom door even for
/// `token_ok` / Connect sessions. Teaching JSON, fail loud.
pub fn skin_host_forbidden(path: &str) -> Option<CliResponse> {
    let blocked = path == "/cli/sessions/grid"
        || path == "/cli/sessions/bytes"
        || path.starts_with("/cli/terminal/")
        || path == "/cli/auth/login"
        || path.starts_with("/v1/");
    if !blocked {
        return None;
    }
    Some(CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "error": skin_door::PATH_FILTER_ERROR,
            "hint": "https://skin.<sub>.k2.dev is Thread-only. Grid/PTY stays on the operator <sub>.k2.dev kingdom door.",
        })
        .to_string(),
    })
}

pub const THREAD_READ: &str = CAP_THREAD_READ;
pub const THREAD_POST: &str = CAP_THREAD_POST;

/// Stable teaching response for a valid scoped agent passport on a
/// skin owner surface (mint / remove / revoke / front-door). CLI maps
/// `owner_only` + 403 → exit 3. Missing/garbage credentials stay
/// [`CliResponse::forbidden`].
pub fn owner_only_response() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": "owner_only",
                "hint": "requires owner/admin — ask your human (k2 skin user add/remove, skin-token create/revoke, and front-door apply are owner surfaces; use k2 skin user list / k2 skin-token list to read the roster)",
            },
        })
        .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_only_response_has_stable_code_and_403() {
        let r = owner_only_response();
        assert_eq!(r.status, "403 Forbidden");
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "owner_only");
        let hint = v["error"]["hint"].as_str().unwrap_or("");
        assert!(
            hint.contains("owner") || hint.contains("human"),
            "hint should teach owner/human: {hint}"
        );
        assert!(
            !r.body.contains("Invalid or missing auth token"),
            "must not look like a broken passport: {}",
            r.body
        );
    }
}
