//! `/cli/app/push/*`: the owner's, Admins' and the App agent's push
//! controls (prd-custom-app-push-v1 CP36, `k2 app push …`). Not
//! App-facing: an App pass, an API key or an API session is refused.
//!
//! ```text
//! GET  /cli/app/push/devices        ?app=&guest=   Admin: platform, environment, channel, last seen; never tokens
//! POST /cli/app/push/devices/remove {app, id}      Admin
//! ```

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::skin::app_push as store;
use serde_json::{json, Value as J};

use crate::apps::grace::{target, Target};
use crate::apps::routes::{actor, json_body, need_credential, no, respond, s, Refusal, Who, R};
use crate::cli_response::CliResponse;

pub const DEVICES: &str = "/cli/app/push/devices";
pub const DEVICES_REMOVE: &str = "/cli/app/push/devices/remove";

pub const ROUTES: &[&str] = &[DEVICES, DEVICES_REMOVE];
pub const POST_ROUTES: &[&str] = &[DEVICES_REMOVE];
/// Request bodies here are at most this many bytes (keys included).
pub const MAX_BODY: usize = 64 * 1024;

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

pub fn too_large() -> CliResponse {
    respond(Err(no("too_large", 413, format!("request bodies here are at most {} KiB", MAX_BODY / 1024))))
}

// ── callers ──────────────────────────────────────────────────────────

/// The owner token, an Owner or Admin login, or a human in a plain K2
/// shell tab.
pub(crate) fn admin_human(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Shell { .. } | Who::Login { role: Role::Owner | Role::Admin, .. })
}

/// The owner in person (can pass a vault confirm).
pub(crate) fn owner_in_person(who: &Who) -> bool {
    matches!(who, Who::Owner | Who::Login { role: Role::Owner, .. })
}

fn admin_only(_t: &Target) -> Refusal {
    no("admin_only", 403, "push devices are the owner's and Admins' to see (an agent reads counts with k2 app push status)")
}

/// The App a call names, checked against the caller (a passport sees only
/// its own workspace's Apps; host is never a push App).
pub(crate) fn push_target(who: &Who, raw: Option<&str>) -> R<Target> {
    need_credential(who)?;
    if raw.map(str::trim) == Some(k2_core::skin::HOST_APP_ID) {
        return Err(no("not_for_host", 400, "push is for a published App's own users, never the box login (host)"));
    }
    target(who, raw)
}

fn device_json(d: &store::PushDevice) -> J {
    json!({
        "id": d.id,
        "username": d.username,
        "deviceId": d.device_id,
        "platform": d.platform,
        "environment": d.environment,
        "channel": d.channel,
        "previews": d.previews,
        "mutedTopics": d.muted_topics,
        "createdAt": d.created_at,
        "lastSeenAt": d.last_seen_at,
    })
}

fn principal_of(username: &str) -> R<Option<String>> {
    let u = k2_core::skin::normalize_username(username).map_err(|e| no("usage", 400, e))?;
    let all = k2_core::skin::list_principals().map_err(|e| no("store", 500, e))?;
    Ok(all.into_iter().find(|p| p.username == u).map(|p| p.id))
}

/// GET `/cli/app/push/devices?app=&guest=`.
fn devices(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let t = push_target(who, params.get("app").map(String::as_str))?;
    if !admin_human(who) {
        return Err(admin_only(&t));
    }
    let list = match params.get("guest").or_else(|| params.get("user")).map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(u) => match principal_of(u)? {
            Some(pid) => store::devices_for_app(&t.service_id, Some(&pid)).map_err(|e| no("store", 500, e))?,
            None => Vec::new(),
        },
        None => store::devices_for_app(&t.service_id, None).map_err(|e| no("store", 500, e))?,
    };
    let devices: Vec<J> = list.iter().map(device_json).collect();
    Ok(json!({ "ok": true, "app": t.service_id, "name": t.name, "count": devices.len(), "devices": devices }))
}

/// POST `/cli/app/push/devices/remove {app, id}`.
fn devices_remove(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let t = push_target(who, s(&v, "app"))?;
    if !admin_human(who) {
        return Err(admin_only(&t));
    }
    let id = s(&v, "id").ok_or_else(|| no("usage", 400, "send {app, id} (an apd_… id from k2 app push devices)"))?;
    let removed = store::remove_by_owner(&t.service_id, id).map_err(|e| no("store", 500, e))?;
    if !removed {
        return Err(no("not_found", 404, "no push device of this App by that id"));
    }
    super::audit_line("app-push-device-remove", &actor(who), &format!("app={} device={id}", t.service_id));
    Ok(json!({ "ok": true, "removed": true }))
}

pub fn handle(path: &str, who: Who, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    if is_post_route(path) {
        if let Err(e) = crate::apps::access::member_read_only(&who) {
            return respond(Err(e));
        }
    }
    let r = match path {
        DEVICES => devices(&who, params),
        DEVICES_REMOVE => devices_remove(&who, body),
        _ => Err(no("not_found", 404, "no such App push route")),
    };
    respond(r)
}

#[cfg(test)]
#[path = "owner_routes_tests.rs"]
mod tests;
