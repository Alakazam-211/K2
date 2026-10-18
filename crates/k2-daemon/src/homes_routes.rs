//! `/cli/me/homes*` — a person's Homes synced between their devices
//! (homes-sync-v1, 0.46.1; `k2_core::homes_sync` holds the model).
//!
//! | Method | Path | Body / query | Answer |
//! |---|---|---|---|
//! | GET  | `/cli/me/homes` | `?deviceId=&since=<cursor>` | `{cursor, full, changes[], redirects{}, device{deviceId, syncing}, serverId, servers{}}` |
//! | POST | `/cli/me/homes/push` | `{deviceId, changes[]}` | `{cursor, applied, superseded, rejected[{item, reason}]}` |
//! | POST | `/cli/me/homes/devices/register` | `{deviceId, name, platform}` | `{device}` |
//! | GET  | `/cli/me/homes/devices` | `?deviceId=` (marks `current`) | `{devices[{deviceId, name, platform, registeredAt, lastSyncAt, current}]}` |
//! | POST | `/cli/me/homes/devices/remove` | `{deviceId}` | `{removed: true}` |
//!
//! **Who.** A person only: the owner token or a Connect login (Member
//! floor), resolved to the person key (`owner` for the owner token and
//! every login the owner linked, else the login's own name). The person
//! comes from the credential, never the body. Agent passports, app passes
//! (`k2skn_`), API keys and the agent cell socket get 403
//! `homes_person_only`.
//!
//! **Devices.** A device registers before it pushes (409
//! `device_not_registered` otherwise). Once removed, its pull and push
//! answer 410 `device_removed` (and the person's sockets get
//! `homes_device_removed {deviceId}`) until it registers again.
//!
//! **Frames.** A push that changed something emits `homes_changed
//! {cursor}` to that person's `/cli/sessions/events` sockets only.

use std::collections::HashMap;

use k2_core::homes_sync::{self, DeviceState, Removal};
use k2_core::notify_prefs::{People, OWNER};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::notify_routes::Actor;
use crate::session_events::SessionEvent;

pub const HOMES_PATH: &str = "/cli/me/homes";
pub const PUSH_PATH: &str = "/cli/me/homes/push";
pub const DEVICES_PATH: &str = "/cli/me/homes/devices";
pub const REGISTER_PATH: &str = "/cli/me/homes/devices/register";
pub const REMOVE_PATH: &str = "/cli/me/homes/devices/remove";

/// Body cap for the POST routes (a push of [`homes_sync::MAX_CHANGES`]
/// items fits well under it).
pub const MAX_BODY: usize = 1024 * 1024;

/// Is `path` one of the Homes sync routes?
pub fn is_homes_route(path: &str) -> bool {
    matches!(path, HOMES_PATH | PUSH_PATH | DEVICES_PATH | REGISTER_PATH | REMOVE_PATH)
}

/// The POST-only ones (a GET is 405).
pub fn is_post_only(path: &str) -> bool {
    matches!(path, PUSH_PATH | REGISTER_PATH | REMOVE_PATH)
}

fn error(status: &'static str, code: &str, hint: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint.to_string() } }).to_string(),
    }
}

fn ok(v: Value) -> CliResponse {
    CliResponse { status: "200 OK", content_type: "application/json", body: v.to_string() }
}

/// 403 for anything that is not a person (agents, app passes, API keys,
/// the cell socket).
pub fn not_a_person() -> CliResponse {
    error(
        "403 Forbidden",
        "homes_person_only",
        "Homes belong to a person: sign in with your own login (agents, app passes and API keys have no Homes)",
    )
}

/// GET chain / cell socket door: never served there.
pub fn refuse_outside_person_arm(path: &str, is_post: bool) -> Option<CliResponse> {
    if !is_homes_route(path) {
        return None;
    }
    if is_post_only(path) && !is_post {
        return Some(CliResponse::method_not_allowed());
    }
    Some(not_a_person())
}

fn device_removed(device_id: &str) -> CliResponse {
    error(
        "410 Gone",
        "device_removed",
        format!("device {device_id} was removed from Homes sync on this server; register it again to sync"),
    )
}

/// The person key for `actor`, or 403 (a login that no longer exists).
pub fn person_for(actor: &Actor) -> Result<String, CliResponse> {
    if actor.owner_token {
        return Ok(OWNER.to_string());
    }
    if k2_core::connect_users::role_for_user(&actor.login).is_none() {
        return Err(not_a_person());
    }
    let people = People::load().map_err(|e| error("500 Internal Server Error", "store", e))?;
    Ok(people.person_key(&actor.login))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn device_param(params: &HashMap<String, String>) -> Result<Option<String>, CliResponse> {
    match params.get("deviceId").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(d) if homes_sync::valid_device_id(d) => Ok(Some(d.to_string())),
        Some(_) => Err(error("400 Bad Request", "usage", "deviceId: 1-128 chars of A-Z a-z 0-9 . _ : -")),
    }
}

fn body_device(body: &Value) -> Result<String, CliResponse> {
    match body.get("deviceId").and_then(Value::as_str).map(str::trim) {
        Some(d) if homes_sync::valid_device_id(d) => Ok(d.to_string()),
        _ => Err(error("400 Bad Request", "usage", "deviceId is required: 1-128 chars of A-Z a-z 0-9 . _ : -")),
    }
}

/// Handle one `/cli/me/homes*` request for `actor` (the dispatcher's
/// person arm already resolved the credential).
pub fn handle(is_post: bool, path: &str, body: &[u8], actor: &Actor, params: &HashMap<String, String>) -> CliResponse {
    if !is_homes_route(path) {
        return CliResponse::not_found();
    }
    match (is_post, path) {
        (false, PUSH_PATH | REGISTER_PATH | REMOVE_PATH) => return CliResponse::method_not_allowed(),
        (true, HOMES_PATH | DEVICES_PATH) => return CliResponse::method_not_allowed(),
        _ => {}
    }
    let person = match person_for(actor) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let body: Value = if is_post {
        match serde_json::from_slice(body) {
            Ok(v @ Value::Object(_)) => v,
            _ => return error("400 Bad Request", "usage", "body must be a JSON object"),
        }
    } else {
        Value::Null
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    if !homes_sync::tables_exist(&conn) {
        return error("503 Service Unavailable", "not_ready", "Homes sync is not migrated yet");
    }
    let now = now_ms();
    let store = |e: rusqlite::Error| error("500 Internal Server Error", "store", e);
    match path {
        HOMES_PATH => {
            let device = match device_param(params) {
                Ok(Some(d)) => d,
                Ok(None) => return error("400 Bad Request", "usage", "deviceId is required"),
                Err(r) => return r,
            };
            let since = match params.get("since").map(|s| s.trim()).filter(|s| !s.is_empty()) {
                None => None,
                Some(s) => match s.parse::<i64>() {
                    Ok(n) if n >= 0 => Some(n),
                    _ => return error("400 Bad Request", "usage", "since must be a cursor from an earlier answer"),
                },
            };
            let state = match homes_sync::device_state(&conn, &person, &device) {
                Ok(s) => s,
                Err(e) => return store(e),
            };
            if state == DeviceState::Removed {
                return device_removed(&device);
            }
            let p = match homes_sync::pull(&conn, &person, since, Some(&device), now) {
                Ok(p) => p,
                Err(e) => return store(e),
            };
            let redirects: serde_json::Map<String, Value> =
                p.redirects.into_iter().map(|(from, to)| (from, Value::String(to))).collect();
            ok(json!({
                "ok": true,
                "person": person,
                "serverId": crate::boot_status::server_id(),
                "cursor": p.cursor,
                "full": p.full,
                "changes": p.changes.iter().map(homes_sync::Item::to_json).collect::<Vec<_>>(),
                "redirects": redirects,
                "device": { "deviceId": device, "syncing": state == DeviceState::Active },
                "servers": server_map(),
            }))
        }
        PUSH_PATH => {
            let device = match body_device(&body) {
                Ok(d) => d,
                Err(r) => return r,
            };
            let Some(changes) = body.get("changes").and_then(Value::as_array) else {
                return error("400 Bad Request", "usage", "changes must be an array of Home and row items");
            };
            if changes.len() > homes_sync::MAX_CHANGES {
                return error(
                    "413 Payload Too Large",
                    "too_many_changes",
                    format!("at most {} changes per push", homes_sync::MAX_CHANGES),
                );
            }
            match homes_sync::device_state(&conn, &person, &device) {
                Ok(DeviceState::Active) => {}
                Ok(DeviceState::Removed) => return device_removed(&device),
                Ok(DeviceState::Unknown) => {
                    return error(
                        "409 Conflict",
                        "device_not_registered",
                        "register this device first: POST /cli/me/homes/devices/register",
                    )
                }
                Err(e) => return store(e),
            }
            let r = match homes_sync::push(&conn, &person, &device, changes, now) {
                Ok(r) => r,
                Err(e) => return store(e),
            };
            drop(conn);
            if r.changed() {
                let _ = crate::session_events::emit(SessionEvent::HomesChanged { person: person.clone(), cursor: r.cursor });
            }
            ok(json!({
                "ok": true,
                "cursor": r.cursor,
                "applied": r.applied,
                "superseded": r.superseded,
                "rejected": r.rejected.iter().map(|x| json!({ "item": x.item, "reason": x.reason })).collect::<Vec<_>>(),
            }))
        }
        REGISTER_PATH => {
            let device = match body_device(&body) {
                Ok(d) => d,
                Err(r) => return r,
            };
            let Some(name) = body.get("name").and_then(Value::as_str).and_then(homes_sync::clean_device_name) else {
                return error("400 Bad Request", "usage", "name is required (the device's own name, e.g. \"Rosson's iPhone\")");
            };
            let Some(platform) = body.get("platform").and_then(Value::as_str).and_then(homes_sync::clean_platform) else {
                return error("400 Bad Request", "usage", "platform is required: ios, android, macos, windows, linux, web, …");
            };
            match homes_sync::register_device(&conn, &person, &device, &name, &platform, now) {
                Ok(d) => ok(json!({ "ok": true, "device": d.to_json(Some(&device)) })),
                Err(e) => store(e),
            }
        }
        DEVICES_PATH => {
            let current = match device_param(params) {
                Ok(d) => d,
                Err(r) => return r,
            };
            match homes_sync::list_devices(&conn, &person) {
                Ok(list) => ok(json!({
                    "ok": true,
                    "devices": list.iter().map(|d| d.to_json(current.as_deref())).collect::<Vec<_>>(),
                })),
                Err(e) => store(e),
            }
        }
        REMOVE_PATH => {
            let device = match body_device(&body) {
                Ok(d) => d,
                Err(r) => return r,
            };
            match homes_sync::remove_device(&conn, &person, &device, now) {
                Ok(Removal::Unknown) => {
                    error("404 Not Found", "unknown_device", format!("no device {device} syncs Homes here"))
                }
                Ok(Removal::AlreadyRemoved) => ok(json!({ "ok": true, "removed": true, "deviceId": device })),
                Ok(Removal::Removed) => {
                    drop(conn);
                    let _ = crate::session_events::emit(SessionEvent::HomesDeviceRemoved {
                        person: person.clone(),
                        device_id: device.clone(),
                    });
                    ok(json!({ "ok": true, "removed": true, "deviceId": device }))
                }
                Err(e) => store(e),
            }
        }
        _ => CliResponse::not_found(),
    }
}

/// Host key → `serverId` for every server this one can vouch for: itself
/// (its public tunnel host) and its pinned federation peers. Clients use
/// it (with each saved server's own `/boot-status` `serverId`) to move
/// rows from `handle::<hostKey>` to `handle::<serverId>`.
pub fn server_map() -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    if let (Some(id), Some(host)) = (crate::boot_status::server_id(), crate::boot_status::public_host()) {
        out.insert(host, Value::String(id));
    }
    if let Ok(store) = k2_core::federation::peers::PeerStore::load() {
        for p in store.list() {
            if let Some(key) = peer_host_key(&p.subdomain, &p.base_url) {
                out.entry(key)
                    .or_insert_with(|| Value::String(k2_core::federation::identity::server_id_from_fingerprint(&p.fingerprint)));
            }
        }
    }
    out
}

/// A pinned peer's Home host key: a Connect subdomain → `<sub>.k2.dev`; a
/// saved `host:port` stays as is; else the base URL's host (`:port` unless
/// https on 443).
pub fn peer_host_key(subdomain: &str, base_url: &str) -> Option<String> {
    let sub = subdomain.trim().to_ascii_lowercase();
    if !sub.is_empty() {
        if sub.contains(':') || sub.contains('.') {
            return Some(sub);
        }
        return Some(format!("{sub}.{}", k2_core::tunnel::config::SUBDOMAIN_HOST));
    }
    let base = base_url.trim().to_ascii_lowercase();
    let (scheme, rest) = base.split_once("://")?;
    let hostport = rest.split('/').next().filter(|h| !h.is_empty())?;
    match (scheme, hostport.rsplit_once(':')) {
        ("https", Some((h, "443"))) => Some(h.to_string()),
        ("https", None) => Some(hostport.to_string()),
        ("http", None) => Some(format!("{hostport}:80")),
        (_, Some(_)) => Some(hostport.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_shape_and_doors() {
        for p in [HOMES_PATH, PUSH_PATH, DEVICES_PATH, REGISTER_PATH, REMOVE_PATH] {
            assert!(is_homes_route(p), "{p}");
        }
        assert!(!is_homes_route("/cli/me/homesx"));
        // The GET chain / cell socket: POST-only on GET → 405, else 403.
        assert_eq!(refuse_outside_person_arm(PUSH_PATH, false).unwrap().status, "405 Method Not Allowed");
        assert_eq!(refuse_outside_person_arm(PUSH_PATH, true).unwrap().status, "403 Forbidden");
        let r = refuse_outside_person_arm(HOMES_PATH, false).unwrap();
        assert_eq!(r.status, "403 Forbidden");
        assert!(r.body.contains("homes_person_only"), "{}", r.body);
        assert!(refuse_outside_person_arm("/cli/thread", false).is_none());
    }

    #[test]
    fn peer_host_keys() {
        assert_eq!(peer_host_key("rosson", ""), Some("rosson.k2.dev".into()));
        assert_eq!(peer_host_key("192.168.1.50:38471", ""), Some("192.168.1.50:38471".into()));
        assert_eq!(peer_host_key("", "https://box.example.com"), Some("box.example.com".into()));
        assert_eq!(peer_host_key("", "https://box.example.com:443/x"), Some("box.example.com".into()));
        assert_eq!(peer_host_key("", "http://box.lan"), Some("box.lan:80".into()));
        assert_eq!(peer_host_key("", "http://10.0.0.2:38471"), Some("10.0.0.2:38471".into()));
        assert_eq!(peer_host_key("", ""), None);
    }
}
