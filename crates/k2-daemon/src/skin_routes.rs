//! Skin roster / passes / front-door (`/cli/skin*`).
//!
//! GET `/cli/skin/users` and GET `/cli/skin-tokens` admit owner-tier
//! (`owner_role_identity`) **or** a workspace-agent scoped hook. Manage
//! mutations admit owner-tier **or** a scoped hook when that workspace's
//! Agent-tab `agents_can_manage_skin` column is ON. Leftover front-door /
//! Hydra stay owner-only. Mutations are POST-only (GET twins 405). Raw
//! `k2skn_…` secret is returned once on create.

use std::collections::HashSet;

use crate::cli_response::CliResponse;
use k2_core::skin::{
    self, RoomPolicy, CAP_FILES_READ, CAP_FILES_WRITE, CAP_THREAD_POST, CAP_THREAD_READ,
    CAP_TICKETS_POST, CAP_TICKETS_READ,
};
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

fn rooms_field(v: &serde_json::Value) -> Option<Vec<String>> {
    v.get("rooms")
        .or_else(|| v.get("agents"))
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
}

fn apply_tokens_field(v: &serde_json::Value) -> bool {
    match v
        .get("applyTokens")
        .or_else(|| v.get("apply_tokens"))
        .or_else(|| v.get("apply-tokens"))
    {
        None => false,
        Some(x) if x.is_null() => false,
        Some(x) => x.as_bool().unwrap_or(false),
    }
}

fn resolve_rooms_or_400(raw: Option<Vec<String>>) -> Result<Vec<String>, CliResponse> {
    let Some(list) = raw else {
        return Err(CliResponse::bad_request(
            "rooms must include at least one workspace",
        ));
    };
    match skin::resolve_room_tokens(&list) {
        Ok(ids) => Ok(ids),
        Err(e) => Err(CliResponse::bad_request(e)),
    }
}

/// Roles may be Thread-dark (`[]`). Unknown handles still 400.
fn resolve_rooms_allow_empty(raw: Option<Vec<String>>) -> Result<Vec<String>, CliResponse> {
    match skin::resolve_room_tokens(&raw.unwrap_or_default()) {
        Ok(ids) => Ok(ids),
        Err(e) => Err(CliResponse::bad_request(e)),
    }
}

const ROOM_ACCESS_TEACHING: &str = "use roomAccess (per-room functions), not caps+rooms";

fn room_access_value(v: &serde_json::Value) -> Option<&serde_json::Value> {
    v.get("roomAccess").or_else(|| v.get("room_access"))
}

fn parse_room_access(v: &serde_json::Value) -> Result<RoomPolicy, CliResponse> {
    let Some(arr) = v.as_array() else {
        return Err(CliResponse::bad_request("roomAccess must be an array"));
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut policy = RoomPolicy::new();
    for item in arr {
        let Some(handle) = str_field(item, &["handle", "id"]) else {
            return Err(CliResponse::bad_request(
                "roomAccess item missing handle or id",
            ));
        };
        let key = handle.to_ascii_lowercase();
        if !seen.insert(key) {
            return Err(CliResponse::bad_request(format!(
                "duplicate handle {handle:?}"
            )));
        }
        let ids = match skin::resolve_room_tokens(&[handle.to_string()]) {
            Ok(ids) if ids.len() == 1 => ids,
            Ok(_) => {
                return Err(CliResponse::bad_request(format!(
                    "unknown workspace handle {handle:?}"
                )))
            }
            Err(e) => return Err(CliResponse::bad_request(e)),
        };
        let project_id = ids[0].clone();
        if policy.contains_key(&project_id) {
            return Err(CliResponse::bad_request(format!(
                "duplicate handle {handle:?}"
            )));
        }
        let caps = match skin::parse_caps(caps_field(item).as_deref()) {
            Ok(c) => c,
            Err(e) => return Err(CliResponse::bad_request(e)),
        };
        policy.insert(project_id, caps);
    }
    Ok(policy)
}

/// Create: missing both → empty map. Update: missing both → `None` (keep).
fn role_policy_from_body(
    v: &serde_json::Value,
    is_update: bool,
) -> Result<Option<RoomPolicy>, CliResponse> {
    if let Some(ra) = room_access_value(v) {
        return Ok(Some(parse_room_access(ra)?));
    }
    let caps = caps_field(v);
    let rooms = rooms_field(v);
    if caps.is_some() && rooms.is_some() {
        return Err(CliResponse::bad_request(ROOM_ACCESS_TEACHING.to_string()));
    }
    if caps.is_some() && rooms.is_none() {
        return Err(CliResponse::bad_request(
            "caps require rooms or roomAccess (per-room functions)",
        ));
    }
    if let Some(list) = rooms {
        let ids = resolve_rooms_allow_empty(Some(list))?;
        return Ok(Some(skin::thread_only_room_policy(&ids)));
    }
    if is_update {
        Ok(None)
    } else {
        Ok(Some(RoomPolicy::new()))
    }
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
            let password = str_field(&v, &["password"]);
            if password.is_some() {
                match skin::set_principal_password(&p.username, password) {
                    Ok(p) => {
                        return CliResponse::ok_json(
                            serde_json::to_string(&p).unwrap_or_else(|_| "{}".into()),
                        )
                    }
                    Err(e) => return CliResponse::bad_request(e),
                }
            }
            CliResponse::ok_json(serde_json::to_string(&p).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_users_password(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username", "name"]) else {
        return CliResponse::bad_request("missing username");
    };
    let password = v
        .get("password")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match skin::set_principal_password(username, password) {
        Ok(p) => {
            k2_core::log_debug!(
                "[skin] actor={actor} set password for {} has_password={}",
                p.username,
                p.has_password
            );
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
    if v.get("username").is_some() {
        return CliResponse::bad_request("use name (platform label), not username");
    }
    let Some(name) = str_field(&v, &["name"]) else {
        return CliResponse::bad_request("missing name");
    };
    let caps = caps_field(&v);
    let rooms = match rooms_field(&v) {
        None => {
            return CliResponse::bad_request("rooms must include at least one workspace");
        }
        Some(list) if list.is_empty() => {
            return CliResponse::bad_request("rooms must include at least one workspace");
        }
        Some(list) => match resolve_rooms_or_400(Some(list)) {
            Ok(ids) if ids.is_empty() => {
                return CliResponse::bad_request("rooms must include at least one workspace");
            }
            Ok(ids) => ids,
            Err(e) => return e,
        },
    };
    match skin::create_token(name, caps.as_deref(), &rooms) {
        Ok((meta, raw)) => {
            k2_core::log_debug!(
                "[skin] actor={actor} minted platform token {} name={} caps={:?} rooms={:?}",
                meta.id,
                meta.name,
                meta.caps,
                meta.rooms
            );
            CliResponse::ok_json(
                serde_json::json!({
                    "id": meta.id,
                    "name": meta.name,
                    "prefix": meta.prefix,
                    "caps": meta.caps,
                    "rooms": meta.rooms,
                    "roomHandles": meta.room_handles,
                    "createdAt": meta.created_at,
                    "token": raw,
                })
                .to_string(),
            )
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_tokens_rooms(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id) = str_field(&v, &["id", "tokenId", "token_id"]) else {
        return CliResponse::bad_request("missing id");
    };
    let Some(list) = rooms_field(&v) else {
        return CliResponse::bad_request("missing rooms");
    };
    let rooms = match resolve_rooms_or_400(Some(list)) {
        Ok(ids) => ids,
        Err(e) => return e,
    };
    match skin::set_token_rooms(id, &rooms) {
        Ok(meta) => {
            k2_core::log_debug!(
                "[skin] actor={actor} token {} rooms={:?}",
                meta.id,
                meta.rooms
            );
            CliResponse::ok_json(serde_json::to_string(&meta).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_roles_get() -> CliResponse {
    match skin::list_roles() {
        Ok(roles) => CliResponse::ok_json(serde_json::json!({ "roles": roles }).to_string()),
        Err(e) => CliResponse::internal_error(e),
    }
}

pub fn handle_roles_post(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if v.get("username").is_some() {
        return CliResponse::bad_request("use name (role label), not username");
    }
    let Some(name) = str_field(&v, &["name"]) else {
        return CliResponse::bad_request("missing name");
    };
    let policy = match role_policy_from_body(&v, false) {
        Ok(Some(p)) => p,
        Ok(None) => RoomPolicy::new(),
        Err(e) => return e,
    };
    match skin::create_role(name, &policy) {
        Ok(role) => {
            k2_core::log_debug!(
                "[skin] actor={actor} created role {} name={} caps={:?} rooms={:?}",
                role.id,
                role.name,
                role.caps,
                role.rooms
            );
            CliResponse::ok_json(serde_json::to_string(&role).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_roles_update(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id_or_name) = str_field(&v, &["id", "name", "role"]) else {
        return CliResponse::bad_request("missing id or name");
    };
    let policy = match role_policy_from_body(&v, true) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match skin::update_role(id_or_name, policy.as_ref()) {
        Ok(role) => {
            k2_core::log_debug!(
                "[skin] actor={actor} updated role {} caps={:?} rooms={:?}",
                role.name,
                role.caps,
                role.rooms
            );
            CliResponse::ok_json(serde_json::to_string(&role).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_roles_room(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id_or_name) = str_field(&v, &["id", "name", "role"]) else {
        return CliResponse::bad_request("missing id or name");
    };
    let Some(handle) = str_field(&v, &["handle", "id"]) else {
        return CliResponse::bad_request("missing handle or id");
    };
    let clear = match v.get("clear") {
        None => false,
        Some(x) if x.is_null() => false,
        Some(x) => x.as_bool().unwrap_or(false),
    };
    let ids = match skin::resolve_room_tokens(&[handle.to_string()]) {
        Ok(ids) if ids.len() == 1 => ids,
        Ok(_) => return CliResponse::bad_request(format!("unknown workspace handle {handle:?}")),
        Err(e) => return CliResponse::bad_request(e),
    };
    let project_id = &ids[0];
    let result = if clear {
        skin::clear_role_room(id_or_name, project_id)
    } else {
        let caps = caps_field(&v);
        skin::set_role_room(id_or_name, project_id, caps.as_deref())
    };
    match result {
        Ok(role) => {
            k2_core::log_debug!(
                "[skin] actor={actor} role room {} handle={} clear={clear} rooms={:?}",
                role.name,
                handle,
                role.rooms
            );
            CliResponse::ok_json(serde_json::to_string(&role).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_roles_remove(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id_or_name) = str_field(&v, &["id", "name", "role"]) else {
        return CliResponse::bad_request("missing id or name");
    };
    match skin::remove_role(id_or_name) {
        Ok(true) => {
            k2_core::log_debug!("[skin] actor={actor} removed role {id_or_name}");
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Ok(false) => CliResponse::bad_request(format!("unknown skin role '{id_or_name}'")),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_roles_assign(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username"]) else {
        return CliResponse::bad_request("missing username");
    };
    let Some(role) = str_field(&v, &["role", "name", "id"]) else {
        return CliResponse::bad_request("missing role");
    };
    match skin::assign_role(username, role) {
        Ok(p) => {
            k2_core::log_debug!(
                "[skin] actor={actor} assigned role {} to {}",
                p.role_name.as_deref().unwrap_or(role),
                p.username
            );
            CliResponse::ok_json(serde_json::to_string(&p).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_roles_unassign(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username", "name"]) else {
        return CliResponse::bad_request("missing username");
    };
    match skin::unassign_role(username) {
        Ok(p) => {
            k2_core::log_debug!("[skin] actor={actor} unassigned role from {}", p.username);
            CliResponse::ok_json(serde_json::to_string(&p).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_users_rooms(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(username) = str_field(&v, &["username", "name"]) else {
        return CliResponse::bad_request("missing username");
    };
    let Some(list) = rooms_field(&v) else {
        return CliResponse::bad_request("missing rooms");
    };
    let rooms = match resolve_rooms_or_400(Some(list)) {
        Ok(ids) => ids,
        Err(e) => return e,
    };
    let apply = apply_tokens_field(&v);
    match skin::set_principal_default_rooms(username, &rooms, apply) {
        Ok(p) => {
            k2_core::log_debug!(
                "[skin] actor={actor} user {} default_rooms={:?} apply_tokens={apply}",
                p.username,
                p.default_rooms
            );
            CliResponse::ok_json(serde_json::to_string(&p).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_agents_get(pass: &skin::SkinPass) -> CliResponse {
    let agents = skin::live_agents(&pass.rooms);
    CliResponse::ok_json(serde_json::json!({ "agents": agents }).to_string())
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

pub fn handle_hydra_get() -> CliResponse {
    CliResponse::ok_json(crate::skin_hydra::status_json().to_string())
}

pub fn handle_hydra_post(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let enabled = match v.get("enabled").or_else(|| v.get("Enabled")) {
        Some(x) => match x.as_bool() {
            Some(b) => b,
            None => return CliResponse::bad_request("enabled must be true or false"),
        },
        None => return CliResponse::bad_request("missing enabled (true|false)"),
    };
    let apply = apply_field(&v);
    match crate::skin_hydra::apply(enabled, apply) {
        Ok(st) => {
            k2_core::log_debug!(
                "[skin] actor={actor} hydra enabled={enabled} apply={apply} running={}",
                st.get("running").and_then(|x| x.as_bool()).unwrap_or(false)
            );
            CliResponse::ok_json(st.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
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

pub const SKIN_ROOM_JSON: &str =
    r#"{"ok":false,"error":{"code":"skin_room","hint":"this pass cannot use that agent"}}"#;

pub fn skin_room_response() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: SKIN_ROOM_JSON.to_string(),
    }
}

/// Owner/Connect/hook on `GET /cli/skin/agents` — that list is skin-token only.
pub fn skin_agents_forbidden() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": "skin_token",
                "hint": "GET /cli/skin/agents is for a live skin pass",
            },
        })
        .to_string(),
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
            "hint": "Grid/PTY is not available with a skin token. Use Thread (and files if scoped).",
        })
        .to_string(),
    })
}

pub const THREAD_READ: &str = CAP_THREAD_READ;
pub const THREAD_POST: &str = CAP_THREAD_POST;
pub const FILES_READ: &str = CAP_FILES_READ;
pub const FILES_WRITE: &str = CAP_FILES_WRITE;
pub const TICKETS_READ: &str = CAP_TICKETS_READ;
pub const TICKETS_POST: &str = CAP_TICKETS_POST;

const OWNER_ONLY_HINT: &str = "requires owner/admin — ask your human (k2 skin user add/remove/password, k2 skin role create/update/remove, k2 skin user role/unassign, skin-token create/revoke/rooms; use k2 skin user list / k2 skin role list / k2 skin-token list to read the roster). Host the UI with k2 publish, not k2 skin.";

const OWNER_ONLY_MANAGE_HINT: &str = "requires owner/admin — ask your human (k2 skin user add/remove/password, k2 skin role create/update/remove, k2 skin user role/unassign, skin-token create/revoke/rooms; use k2 skin user list / k2 skin role list / k2 skin-token list to read the roster). Host the UI with k2 publish, not k2 skin. To let this workspace's agent manage Skin Access, Settings → Workspaces → (this workspace) → Agent → Allow this agent to manage Skin Access.";

fn owner_only_with_hint(hint: &str) -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": "owner_only",
                "hint": hint,
            },
        })
        .to_string(),
    }
}

/// Stable teaching response for leftover owner surfaces (front-door /
/// Hydra). CLI maps `owner_only` + 403 → exit 3. Missing/garbage
/// credentials stay [`CliResponse::forbidden`].
pub fn owner_only_response() -> CliResponse {
    owner_only_with_hint(OWNER_ONLY_HINT)
}

/// Teaching response for Skin Access **manage** mutations when a valid
/// scoped hook hits a workspace whose Agent-tab toggle is OFF.
pub fn owner_only_manage_response() -> CliResponse {
    owner_only_with_hint(OWNER_ONLY_MANAGE_HINT)
}

/// Actor label for a scoped hook that passed the manage gate.
/// `agent:<handle>` from [`workspace_address_name_shared`]. Never
/// `"owner"` / `"owner-token"`.
pub fn skin_manage_actor_label(principal: &crate::session_token::HookPrincipal) -> String {
    let handle = k2_core::workspace_session_handles::workspace_address_name_shared(
        principal.workspace_uuid.trim(),
    )
    .ok()
    .filter(|s| !s.is_empty())
    .or_else(|| {
        let addr = principal.agent_address.trim();
        if !addr.is_empty() {
            Some(addr.to_string())
        } else {
            None
        }
    })
    .unwrap_or_else(|| "agent".to_string());
    format!("agent:{handle}")
}

/// Attach the non-secret actor string to a 200 JSON object so tests
/// can assert it is not `"owner-token"` when a hook mutated.
pub fn stamp_actor(mut r: CliResponse, actor: &str) -> CliResponse {
    if !r.status.starts_with("200") {
        return r;
    }
    if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&r.body) {
        if v.is_object() {
            v["actor"] = serde_json::json!(actor);
            r.body = v.to_string();
        }
    }
    r
}

fn enable_field(v: &serde_json::Value) -> Option<bool> {
    let raw = v.get("enable")?;
    if let Some(n) = raw.as_i64() {
        return match n {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
    }
    if let Some(s) = raw.as_str() {
        return match s.trim() {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        };
    }
    None
}

/// `POST /cli/agents-manage-skin` `{project, enable: 0|1}` — owner-only
/// writer for `projects.agents_can_manage_skin`. Not `workspace/set`.
pub fn handle_agents_manage_skin(body: &[u8]) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(project) = str_field(&v, &["project"]) else {
        return CliResponse::bad_request("missing project");
    };
    let Some(enable) = enable_field(&v) else {
        return CliResponse::bad_request("enable must be 0 or 1");
    };
    let Some(path) = crate::workspace_msg::resolve_workspace(project) else {
        return crate::workspace_routes::workspace_not_found_response(project);
    };
    match k2_core::workspace::settings::set_agents_can_manage_skin(&path, enable) {
        Ok(()) => {
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::SyncProjects,
                serde_json::Value::Null,
            );
            CliResponse::ok_json(
                serde_json::json!({
                    "success": true,
                    "agentsCanManageSkin": enable,
                })
                .to_string(),
            )
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

const LOGIN_FAILED_JSON: &str = r#"{"error":"invalid username or password"}"#;

fn login_failed_json() -> CliResponse {
    CliResponse {
        status: "401 Unauthorized",
        content_type: "application/json",
        body: LOGIN_FAILED_JSON.to_string(),
    }
}

fn login_failed_html() -> CliResponse {
    CliResponse {
        status: "401 Unauthorized",
        content_type: "text/html; charset=utf-8",
        body: "<!DOCTYPE html><html><body><p>invalid username or password</p></body></html>"
            .to_string(),
    }
}

pub struct SkinLoginReply {
    pub response: CliResponse,
    /// Raw `k2skn_` for Set-Cookie (and JSON `token`). Never the hash.
    pub token: Option<String>,
    /// HTML form success → 302 Location. JSON leaves this `None`.
    pub location: Option<String>,
}

fn is_form_body(content_type: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    ct.contains("application/x-www-form-urlencoded") || ct.contains("multipart/form-data")
}

fn percent_decode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.replace('+', " ").into_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &bytes[i + 1..i + 3];
            if let Ok(s) = std::str::from_utf8(hex) {
                if let Ok(v) = u8::from_str_radix(s, 16) {
                    out.push(v as char);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn parse_form_pairs(body: &[u8]) -> Vec<(String, String)> {
    let s = String::from_utf8_lossy(body);
    s.split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            Some((percent_decode(k.trim()), percent_decode(v.trim())))
        })
        .collect()
}

fn form_field<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Relative path only — block `//host` and scheme URLs.
pub fn safe_next(raw: Option<&str>) -> String {
    let t = raw.unwrap_or("/").trim();
    if t.starts_with('/')
        && !t.starts_with("//")
        && !t.contains("://")
        && !t.contains('\r')
        && !t.contains('\n')
    {
        t.to_string()
    } else {
        "/".to_string()
    }
}

/// Public `POST /cli/skin/login`. Generic 401. Dummy argon2 + lockout live
/// in core (`check_and_record_login`); caller adds the 500 ms 401 delay.
pub fn handle_login(body: &[u8], content_type: &str) -> SkinLoginReply {
    let form = is_form_body(content_type);
    let parsed = if form {
        let pairs = parse_form_pairs(body);
        let username = form_field(&pairs, "username").unwrap_or("").to_string();
        let password = form_field(&pairs, "password").unwrap_or("").to_string();
        let next = form_field(&pairs, "next").map(str::to_string);
        Some((username, password, next))
    } else {
        match serde_json::from_slice::<serde_json::Value>(body) {
            Ok(v) => {
                let username = str_field(&v, &["username", "name"])
                    .unwrap_or("")
                    .to_string();
                let password = v
                    .get("password")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let next = str_field(&v, &["next"]).map(str::to_string);
                Some((username, password, next))
            }
            Err(_) => None,
        }
    };
    let Some((username, password, next)) = parsed else {
        return SkinLoginReply {
            response: if form {
                login_failed_html()
            } else {
                login_failed_json()
            },
            token: None,
            location: None,
        };
    };
    if username.trim().is_empty() || password.is_empty() {
        return SkinLoginReply {
            response: if form {
                login_failed_html()
            } else {
                login_failed_json()
            },
            token: None,
            location: None,
        };
    }
    let principal = match skin::check_and_record_login(&username, &password) {
        skin::SkinLoginOutcome::Ok(p) => p,
        skin::SkinLoginOutcome::BadCreds | skin::SkinLoginOutcome::LockedOut => {
            return SkinLoginReply {
                response: if form {
                    login_failed_html()
                } else {
                    login_failed_json()
                },
                token: None,
                location: None,
            };
        }
    };
    let (meta, raw) = match skin::create_session_token(&principal.username) {
        Ok(v) => v,
        Err(e) => {
            return SkinLoginReply {
                response: CliResponse::internal_error(e),
                token: None,
                location: None,
            };
        }
    };
    if form {
        let location = safe_next(next.as_deref());
        return SkinLoginReply {
            response: CliResponse {
                status: "302 Found",
                content_type: "text/html; charset=utf-8",
                body: "<!DOCTYPE html><html><body>ok</body></html>".to_string(),
            },
            token: Some(raw),
            location: Some(location),
        };
    }
    let body = serde_json::json!({
        "ok": true,
        "token": raw.clone(),
        "username": principal.username,
        "rooms": meta.rooms,
        "roomHandles": meta.room_handles,
        "caps": meta.caps,
        "role": principal.role_name,
        "roomAccess": meta.room_access,
    })
    .to_string();
    debug_assert!(
        !body.contains("password_hash") && !body.contains("passwordHash"),
        "password_hash must never be on the wire"
    );
    SkinLoginReply {
        response: CliResponse::ok_json(body),
        token: Some(raw),
        location: None,
    }
}

/// Session `k2skn_` only. Static partner key → 403, not revoked.
pub fn handle_logout(presented: Option<&str>) -> CliResponse {
    let Some(raw) = presented.map(str::trim).filter(|s| !s.is_empty()) else {
        return CliResponse {
            status: "401 Unauthorized",
            content_type: "application/json",
            body: r#"{"error":"invalid or revoked skin token"}"#.to_string(),
        };
    };
    let Some(pass) = skin::resolve_skin_token(raw) else {
        return CliResponse {
            status: "401 Unauthorized",
            content_type: "application/json",
            body: r#"{"error":"invalid or revoked skin token"}"#.to_string(),
        };
    };
    if !pass.session {
        return CliResponse {
            status: "403 Forbidden",
            content_type: "application/json",
            body: r#"{"error":"static skin tokens cannot be logged out"}"#.to_string(),
        };
    }
    match skin::revoke_token(&pass.id) {
        Ok(_) => CliResponse::ok_json(r#"{"ok":true}"#.to_string()),
        Err(e) => CliResponse::internal_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_next_rejects_open_redirects() {
        assert_eq!(safe_next(Some("https://evil.example")), "/");
        assert_eq!(safe_next(Some("//evil")), "/");
        assert_eq!(safe_next(Some("/app")), "/app");
    }

    #[test]
    fn login_failed_bodies_are_generic() {
        let a = login_failed_json();
        let b = login_failed_json();
        assert_eq!(a.body, b.body);
        assert_eq!(a.status, "401 Unauthorized");
        assert!(!a.body.contains("guest"));
        assert_eq!(a.body, LOGIN_FAILED_JSON);
    }

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
            !hint.contains("Allow this agent to manage Skin Access"),
            "leftover/hydra keep today's hint without the Agent-tab sentence: {hint}"
        );
        assert!(
            !r.body.contains("Invalid or missing auth token"),
            "must not look like a broken passport: {}",
            r.body
        );
    }

    #[test]
    fn owner_only_manage_response_adds_agent_tab_sentence() {
        let r = owner_only_manage_response();
        assert_eq!(r.status, "403 Forbidden");
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "owner_only");
        let hint = v["error"]["hint"].as_str().expect("hint");
        assert!(
            hint.contains("Allow this agent to manage Skin Access"),
            "manage OFF must teach the Agent-tab toggle: {hint}"
        );
        assert!(
            !r.body.contains("gated"),
            "must stay owner_only, not gated: {}",
            r.body
        );
    }

    #[test]
    fn skin_manage_actor_label_is_agent_handle_never_owner_token() {
        k2_core::db::init_for_tests();
        let db = k2_core::db::shared();
        let conn = db.lock();
        let id = uuid::Uuid::new_v4().to_string();
        let handle = format!("sales{}", &id[..8]);
        let path = format!("/tmp/k2-skin-actor-{id}");
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
            rusqlite::params![id, handle, path],
        )
        .expect("seed");
        drop(conn);
        let principal = crate::session_token::HookPrincipal {
            workspace_uuid: id,
            agent_address: "sidecar".to_string(),
        };
        let actor = skin_manage_actor_label(&principal);
        assert_eq!(actor, format!("agent:{handle}"));
        assert_ne!(actor, "owner");
        assert_ne!(actor, "owner-token");
    }

    #[test]
    fn stamp_actor_skips_non_200_and_sets_field_on_ok() {
        let denied = owner_only_manage_response();
        let stamped = stamp_actor(denied, "agent:sales");
        assert!(!stamped.body.contains("agent:sales"), "{}", stamped.body);
        let ok = CliResponse::ok_json(r#"{"username":"bob"}"#.to_string());
        let stamped = stamp_actor(ok, "agent:sales");
        let v: serde_json::Value = serde_json::from_str(&stamped.body).expect("json");
        assert_eq!(v["actor"], "agent:sales");
        assert_eq!(v["username"], "bob");
    }

    #[test]
    fn hydra_get_unsupported_on_non_linux() {
        let _g = crate::skin_hydra::hydra_test_lock();
        k2_core::db::init_for_tests();
        {
            let db = k2_core::db::shared();
            let _ = db.lock().execute("DELETE FROM skin_hydra", []);
        }
        crate::skin_hydra::stop();
        if cfg!(target_os = "linux") {
            return;
        }
        let r = handle_hydra_get();
        assert_eq!(r.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["supported"], false, "{}", r.body);
        assert_eq!(v["enabled"], false, "{}", r.body);
        assert_eq!(v["running"], false, "{}", r.body);
        assert!(
            v["hint"].as_str().unwrap_or("").contains("LINUX"),
            "{}",
            r.body
        );
    }

    #[test]
    fn hydra_post_off_is_not_running() {
        let _g = crate::skin_hydra::hydra_test_lock();
        k2_core::db::init_for_tests();
        {
            let db = k2_core::db::shared();
            let _ = db.lock().execute("DELETE FROM skin_hydra", []);
        }
        let r = handle_hydra_post(br#"{"enabled":false,"apply":true}"#, "owner");
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["enabled"], false, "{}", r.body);
        assert_eq!(v["running"], false, "{}", r.body);
    }

    #[test]
    fn hydra_post_requires_enabled() {
        let r = handle_hydra_post(br#"{}"#, "owner");
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("enabled"), "{}", r.body);
    }
}
