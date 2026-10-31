//! `/cli/me/stash*`: a person's prompt stash synced between their devices
//! (stash-sync-v1, 0.46.7; `k2_core::stash_sync` holds the model; the phone
//! contract is `.k2/prds/contract-stash-sync-v1.md`).
//!
//! | Method | Path | Body / query | Answer |
//! |---|---|---|---|
//! | GET  | `/cli/me/stash` | `?deviceId=&since=<cursor>` | `{ok, cursor, full, changes[record]}` |
//! | POST | `/cli/me/stash/push` | `{deviceId, changes[record]}` | `{ok, cursor, applied, superseded, rejected[{item, reason}]}` |
//!
//! **Who.** A person only, exactly as Homes sync (`homes_routes`): the owner
//! token or a Connect login, resolved to the person key from the
//! credential, never the body. Agent passports, app passes and API keys get
//! 403 `stash_person_only`; the agent cell socket never reaches a handler
//! (not on the scoped allowlist; the GET chain refuses as a belt).
//!
//! **Devices.** The Homes registry (`/cli/me/homes/devices*`): a device
//! registers there before it pushes (409 `device_not_registered`); a
//! removed one answers 410 `device_removed` here too.
//!
//! **Frames.** A push that changed something emits `stash_changed
//! {cursor}` to that person's sockets only.

use std::collections::HashMap;

use k2_core::homes_sync::{self, DeviceState};
use k2_core::stash_sync;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::homes_routes::PersonRefusal;
use crate::notify_routes::Actor;
use crate::session_events::SessionEvent;

pub const STASH_PATH: &str = "/cli/me/stash";
pub const PUSH_PATH: &str = "/cli/me/stash/push";

/// Body cap for the push (the contract's 4 MiB; split bigger batches).
pub const MAX_BODY: usize = 4 * 1024 * 1024;

pub fn is_stash_route(path: &str) -> bool {
    matches!(path, STASH_PATH | PUSH_PATH)
}

pub fn is_post_only(path: &str) -> bool {
    path == PUSH_PATH
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

/// 403 for anything that is not a person.
pub fn not_a_person() -> CliResponse {
    error(
        "403 Forbidden",
        "stash_person_only",
        "the stash belongs to a person: sign in with your own login (agents, app passes and API keys have no stash)",
    )
}

/// GET chain door (`cli::dispatch`): never served there.
pub fn refuse_outside_person_arm(path: &str, is_post: bool) -> Option<CliResponse> {
    if !is_stash_route(path) {
        return None;
    }
    if is_post_only(path) && !is_post {
        return Some(CliResponse::method_not_allowed());
    }
    Some(not_a_person())
}

fn person_for(actor: &Actor) -> Result<String, CliResponse> {
    crate::homes_routes::resolve_person(actor).map_err(|r| match r {
        PersonRefusal::NoLogin => not_a_person(),
        PersonRefusal::Store(e) => error("500 Internal Server Error", "store", e),
        PersonRefusal::Reserved => error(
            "403 Forbidden",
            "stash_person_only",
            "this login's name is one K2 uses for the server's owner and its agents, so it has no stash; the owner can remove it in Settings → Admin Access and add the person under another name",
        ),
    })
}

fn device_removed(device_id: &str) -> CliResponse {
    error(
        "410 Gone",
        "device_removed",
        format!("device {device_id} was removed from this server's device list; register it again (POST /cli/me/homes/devices/register) to sync"),
    )
}

fn valid_device(d: Option<&str>) -> Option<String> {
    d.map(str::trim).filter(|d| homes_sync::valid_device_id(d)).map(str::to_string)
}

/// Handle one `/cli/me/stash*` request for `actor` (the dispatcher's
/// person arm already resolved the credential).
pub fn handle(is_post: bool, path: &str, body: &[u8], actor: &Actor, params: &HashMap<String, String>) -> CliResponse {
    if !is_stash_route(path) {
        return CliResponse::not_found();
    }
    if !is_post && path == PUSH_PATH {
        return CliResponse::method_not_allowed();
    }
    if is_post && path == STASH_PATH {
        return CliResponse::method_not_allowed();
    }
    let person = match person_for(actor) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    if !stash_sync::tables_exist(&conn) {
        return error("503 Service Unavailable", "not_ready", "stash sync is not migrated yet");
    }
    let now = chrono::Utc::now().timestamp_millis();
    let store = |e: rusqlite::Error| error("500 Internal Server Error", "store", e);
    if path == STASH_PATH {
        let Some(device) = valid_device(params.get("deviceId").map(String::as_str)) else {
            return error("400 Bad Request", "usage", "deviceId is required: 1-128 chars of A-Z a-z 0-9 . _ : -");
        };
        let since = match params.get("since").map(|s| s.trim()).filter(|s| !s.is_empty()) {
            None => None,
            Some(s) => match s.parse::<i64>() {
                Ok(n) if n >= 0 => Some(n),
                _ => return error("400 Bad Request", "usage", "since must be a cursor from an earlier answer"),
            },
        };
        match homes_sync::device_state(&conn, &person, &device) {
            Ok(DeviceState::Removed) => return device_removed(&device),
            Ok(_) => {}
            Err(e) => return store(e),
        }
        let p = match stash_sync::pull(&conn, &person, since, Some(&device), now) {
            Ok(p) => p,
            Err(e) => return store(e),
        };
        return ok(json!({
            "ok": true,
            "cursor": p.cursor,
            "full": p.full,
            "changes": p.changes.iter().map(stash_sync::Record::to_json).collect::<Vec<_>>(),
        }));
    }
    // PUSH_PATH
    let body: Value = match serde_json::from_slice(body) {
        Ok(v @ Value::Object(_)) => v,
        _ => return error("400 Bad Request", "usage", "body must be a JSON object"),
    };
    let Some(device) = valid_device(body.get("deviceId").and_then(Value::as_str)) else {
        return error("400 Bad Request", "usage", "deviceId is required: 1-128 chars of A-Z a-z 0-9 . _ : -");
    };
    let Some(changes) = body.get("changes").and_then(Value::as_array) else {
        return error("400 Bad Request", "usage", "changes must be an array of stash records");
    };
    if changes.len() > stash_sync::MAX_CHANGES {
        return error(
            "413 Payload Too Large",
            "too_many_changes",
            format!("at most {} records per push", stash_sync::MAX_CHANGES),
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
    let r = match stash_sync::push(&conn, &person, &device, changes, now) {
        Ok(r) => r,
        Err(e) => return store(e),
    };
    drop(conn);
    if r.changed() {
        let _ = crate::session_events::emit(SessionEvent::StashChanged { person: person.clone(), cursor: r.cursor });
    }
    ok(json!({
        "ok": true,
        "cursor": r.cursor,
        "applied": r.applied,
        "superseded": r.superseded,
        "rejected": r.rejected.iter().map(|x| json!({ "item": x.item, "reason": x.reason })).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_shape_and_doors() {
        assert!(is_stash_route(STASH_PATH) && is_stash_route(PUSH_PATH));
        assert!(!is_stash_route("/cli/me/stashx") && !is_stash_route("/cli/me/homes"));
        assert_eq!(refuse_outside_person_arm(PUSH_PATH, false).unwrap().status, "405 Method Not Allowed");
        let r = refuse_outside_person_arm(PUSH_PATH, true).unwrap();
        assert_eq!(r.status, "403 Forbidden");
        assert!(r.body.contains("stash_person_only"), "{}", r.body);
        let r = refuse_outside_person_arm(STASH_PATH, false).unwrap();
        assert_eq!(r.status, "403 Forbidden");
        assert!(refuse_outside_person_arm("/cli/thread", false).is_none());
        // The handler's own guard: a GET to the push is 405 before anything.
        let h = handle(false, PUSH_PATH, b"", &Actor::owner_token(), &HashMap::new());
        assert_eq!(h.status, "405 Method Not Allowed");
        let h = handle(true, STASH_PATH, b"{}", &Actor::owner_token(), &HashMap::new());
        assert_eq!(h.status, "405 Method Not Allowed");
    }

    /// A login that doesn't exist (or an agent's name) is never a person.
    #[test]
    fn a_login_that_is_not_a_person_is_refused() {
        let _home = crate::test_support::TempHome::new();
        k2_core::connect_users::test_add_legacy_reserved_user("owner", "password123").expect("user named owner");
        let r = person_for(&Actor::login("ghost")).expect_err("no such login");
        assert!(r.body.contains("stash_person_only"), "{}", r.body);
        let r = person_for(&Actor::login("owner")).expect_err("reserved name");
        assert_eq!(r.status, "403 Forbidden");
        assert!(r.body.contains("stash_person_only"), "{}", r.body);
        assert_eq!(person_for(&Actor::owner_token()).ok().as_deref(), Some("owner"));
    }
}
