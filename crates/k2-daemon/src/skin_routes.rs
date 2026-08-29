//! Owner-tier Skin roster / passes / front-door (`/cli/skin*`).
//!
//! Mint is `owner_role_identity` only. Mutations are POST-only (GET twins
//! 405). Raw `k2skn_…` secret is returned once on create.

use crate::cli_response::CliResponse;
use k2_core::skin::{self, CAP_THREAD_POST, CAP_THREAD_READ};

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
    match skin::effective_front_door() {
        Ok(d) => CliResponse::ok_json(serde_json::to_string(&d).unwrap_or_else(|_| "{}".into())),
        Err(e) => CliResponse::internal_error(e),
    }
}

pub fn handle_front_door_post(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(mode) = str_field(&v, &["mode"]) else {
        return CliResponse::bad_request("missing mode (connect|direct)");
    };
    let url = str_field(&v, &["url"]);
    let hint = str_field(&v, &["hint"]);
    match skin::set_front_door(mode, url, hint) {
        Ok(mut d) => {
            if d.mode == "connect" && d.url.as_deref().map(str::trim).unwrap_or("").is_empty() {
                d.url = k2_core::tunnel::config::load()
                    .ok()
                    .and_then(|c| c.public_url())
                    .and_then(|u| skin::skin_url_from_public(&u));
            }
            k2_core::log_debug!("[skin] actor={actor} front-door mode={}", d.mode);
            CliResponse::ok_json(serde_json::to_string(&d).unwrap_or_else(|_| "{}".into()))
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

pub const THREAD_READ: &str = CAP_THREAD_READ;
pub const THREAD_POST: &str = CAP_THREAD_POST;
