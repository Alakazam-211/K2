//! `/cli/people/picture*` — profile pictures for K2's own people (0.46.8,
//! Rosson 2026-11-04): the owner and every Connect login on this server.
//!
//! | Method | Path | Who | Acts on |
//! |---|---|---|---|
//! | GET  | `/cli/people/picture` | any login, the owner token, an agent passport | everyone with a picture: `{person, logins, imageHash}` |
//! | GET  | `/cli/people/picture?person=<login>` | same | one person's picture (`imageUrl`, a JPEG data URL) |
//! | POST | `/cli/people/picture` `{image, person?}` | a person | set or replace |
//! | POST | `/cli/people/picture/clear` `{person?}` | a person | remove |
//!
//! - **Per server** (Rosson 2026-11-04): this server's own picture for the
//!   person. Nothing syncs from K2 Connect or other servers.
//! - **Who may change whose** is the person names' rule
//!   (`notify_routes::people_name_write`, Admin Access): your own always;
//!   the owner person (the owner token or an owner-linked login) only by
//!   an Owner; anyone else by an Admin+ who `can_act_on` their role.
//! - **Agents read only.** A passport's GET works (Thread icons); a POST is
//!   a named 403 `person_only`.
//! - **One image pipeline**: the App user profile's
//!   ([`k2_core::workspace::agent_look::normalize_profile_image`]); only K2's
//!   own JPEG is stored and served. The write budget is that profile's too
//!   (20 per person per 10 min).
//! - Every change emits the app-level `person_picture_changed`.

use std::collections::{BTreeMap, HashMap};

use k2_core::connect_users::Role;
use k2_core::notify_prefs::{People, OWNER};
use k2_core::person_picture;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::notify_routes::{actor_person, Actor};

pub const PICTURE_PATH: &str = "/cli/people/picture";
pub const PICTURE_CLEAR_PATH: &str = "/cli/people/picture/clear";
/// A POST body is at most this many bytes (a 2 MiB picture as base64 in
/// JSON fits).
pub const MAX_BODY: usize = 3 * 1024 * 1024;

/// Is `path` one of this module's routes?
pub fn is_route(path: &str) -> bool {
    path == PICTURE_PATH || path == PICTURE_CLEAR_PATH
}

/// Who is asking, as the dispatcher resolved it from the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// The owner token or a live Connect login.
    Person(Actor),
    /// An agent passport (reads only).
    Agent,
}

fn error(status: &'static str, code: &str, hint: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint.to_string() } }).to_string(),
    }
}

fn usage(hint: impl std::fmt::Display) -> CliResponse {
    error("400 Bad Request", "usage", hint)
}

fn store(e: impl std::fmt::Display) -> CliResponse {
    error("500 Internal Server Error", "store", e)
}

/// The refusal for a passport on a write.
pub fn person_only() -> CliResponse {
    error(
        "403 Forbidden",
        "person_only",
        "a person's picture is changed only by that person, or by the owner or an Admin in Settings > Admin Access; an agent can read it (GET /cli/people/picture)",
    )
}

/// The refusal for a request with no person or agent credential.
pub fn not_signed_in() -> CliResponse {
    error("403 Forbidden", "forbidden", "invalid or missing token")
}

/// Handle one request (the dispatcher arm; also the tests' door).
pub fn handle(is_post: bool, path: &str, body: &[u8], caller: &Caller, params: &HashMap<String, String>) -> CliResponse {
    if !is_post {
        // `/clear` is POST-only (`route_policy::get_refused` answers it
        // first on the wire; this is the handler's own guard).
        if path == PICTURE_CLEAR_PATH {
            return CliResponse::method_not_allowed();
        }
        return read(params);
    }
    let Caller::Person(actor) = caller else { return person_only() };
    write(path, body, actor)
}

/// Username → account id for every Connect login (no store write).
fn account_ids() -> Result<BTreeMap<String, String>, String> {
    Ok(k2_core::connect_users::chat_member_facts()?.into_iter().map(|(id, username, _)| (username, id)).collect())
}

fn read(params: &HashMap<String, String>) -> CliResponse {
    let people = match People::load() {
        Ok(p) => p,
        Err(e) => return store(e),
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    match params.get("person").map(|p| p.trim()).filter(|p| !p.is_empty()) {
        None => {
            let ids = match account_ids() {
                Ok(i) => i,
                Err(e) => return store(e),
            };
            match person_picture::list(&conn, &people, &ids) {
                Ok(rows) => CliResponse::ok_json(json!({ "ok": true, "people": rows }).to_string()),
                Err(e) => store(e),
            }
        }
        Some(raw) => {
            let login = match known_login(&people, raw) {
                Ok(l) => l,
                Err(r) => return r,
            };
            let person = people.person_key(&login);
            let key = match person_picture::storage_key(&person) {
                Ok(Some(k)) => k,
                Ok(None) => return unknown_person(&login),
                Err(e) => return store(e),
            };
            match person_picture::get(&conn, &key) {
                Ok(got) => CliResponse::ok_json(
                    json!({
                        "ok": true,
                        "person": person,
                        "imageHash": got.as_ref().map(|g| g.hash.clone()),
                        "imageUrl": got.as_ref().map(|g| g.data_url()),
                    })
                    .to_string(),
                ),
                Err(e) => store(e),
            }
        }
    }
}

fn unknown_person(login: &str) -> CliResponse {
    error("404 Not Found", "unknown_person", format!("'{login}' is not a person on this server (owner, or a Connect username)"))
}

/// `raw` as a login this server knows (`owner` or a Connect username).
fn known_login(people: &People, raw: &str) -> Result<String, CliResponse> {
    let login = raw.trim().to_ascii_lowercase();
    if login == OWNER {
        return Ok(login);
    }
    match people.user(&login) {
        Some(u) => Ok(u.username.clone()),
        None => Err(unknown_person(&login)),
    }
}

fn parse_body(body: &[u8]) -> Result<Value, CliResponse> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(json!({}));
    }
    serde_json::from_slice::<Value>(body).ok().filter(Value::is_object).ok_or_else(|| usage("the body must be a JSON object"))
}

/// The person whose picture `actor` (holding `role`) may change, from the
/// body's `person` (absent: the caller). The names' rule exactly.
fn target(people: &People, actor: &Actor, role: Role, v: &Value) -> Result<String, CliResponse> {
    let Some(me) = actor_person(people, actor) else {
        return Err(error(
            "403 Forbidden",
            "reserved_login",
            "this login's name is one K2 uses for the server's owner and its agents, so it can't have a profile; the owner can remove it in Settings > Admin Access and add the person under another name",
        ));
    };
    let login = match v.get("person") {
        None | Some(Value::Null) => return Ok(me),
        Some(Value::String(s)) if !s.trim().is_empty() => known_login(people, s)?,
        _ => return Err(usage("person must be a Connect username or \"owner\"")),
    };
    let person = people.person_key(&login);
    if person == me {
        return Ok(person);
    }
    if person == OWNER {
        if role < Role::Owner {
            return Err(error("403 Forbidden", "role_required", "only the owner can change the owner's picture"));
        }
        return Ok(person);
    }
    let Some(user) = people.user(&person) else { return Err(unknown_person(&person)) };
    if role < Role::Admin || !k2_core::connect_users::can_act_on(role, user.role) {
        return Err(error(
            "403 Forbidden",
            "role_required",
            "you can change only your own picture (an Admin can change a Member's or an Admin's; only the owner can change an Owner's)",
        ));
    }
    Ok(person)
}

fn write(path: &str, body: &[u8], actor: &Actor) -> CliResponse {
    let v = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let people = match People::load() {
        Ok(p) => p,
        Err(e) => return store(e),
    };
    let role = if actor.owner_token {
        Role::Owner
    } else {
        match k2_core::connect_users::role_for_user(&actor.login) {
            Some(r) => r,
            None => return not_signed_in(),
        }
    };
    let person = match target(&people, actor, role, &v) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let key = match person_picture::storage_key(&person) {
        Ok(Some(k)) => k,
        Ok(None) => return unknown_person(&person),
        Err(e) => return store(e),
    };
    let clearing = path == PICTURE_CLEAR_PATH;
    // Check the picture before spending a write.
    let jpeg = if clearing {
        None
    } else {
        let Some(raw) = v.get("image").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) else {
            return usage("send {\"image\": \"data:image/png|jpeg|webp;base64,…\"} (POST /cli/people/picture/clear removes it)");
        };
        use k2_core::workspace::agent_look::{normalize_profile_image, LookError};
        match normalize_profile_image(raw) {
            Ok(b) => Some(b),
            Err(e) => {
                let status = match e {
                    LookError::TypeRefused(_) => "415 Unsupported Media Type",
                    LookError::TooLarge(_) => "413 Payload Too Large",
                    _ => "400 Bad Request",
                };
                let code = match e {
                    LookError::NotDataUrl => "usage",
                    _ => e.code(),
                };
                return error(status, code, e);
            }
        }
    };
    if let Err(retry) = k2_core::skin::profile::take_write(&format!("k2-person:{key}")) {
        return CliResponse {
            status: "429 Too Many Requests",
            content_type: "application/json",
            body: json!({ "ok": false, "error": { "code": "rate_limited", "hint": "too many picture changes lately: wait, then try again", "retryAfterSecs": retry } })
                .to_string(),
        };
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let hash = match &jpeg {
        Some(bytes) => match person_picture::set(&conn, &key, bytes, &actor.login) {
            Ok(h) => Some(h),
            Err(e) => return store(e),
        },
        None => match person_picture::clear(&conn, &key) {
            Ok(_) => None,
            Err(e) => return store(e),
        },
    };
    drop(conn);
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "people.picture.set",
        &actor.login,
        format!("person={person} picture={}", if clearing { "removed" } else { "set" }),
        "-",
        "-",
        "cli",
    ));
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::PersonPictureChanged {
        person: person.clone(),
        image_hash: hash.clone(),
    });
    let image_url = jpeg.as_ref().map(|b| {
        person_picture::Stored { jpeg: b.clone(), hash: hash.clone().unwrap_or_default(), set_at: 0 }.data_url()
    });
    CliResponse::ok_json(json!({ "ok": true, "person": person, "imageHash": hash, "imageUrl": image_url }).to_string())
}

#[cfg(test)]
#[path = "people_picture_routes_tests.rs"]
mod tests;
