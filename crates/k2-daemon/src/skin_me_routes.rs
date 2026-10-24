//! `GET /cli/skin/me`: who a pass is (room-scope R6, RS9; the read-only
//! half of runtime S25's `me`, pulled into 0.46.1).
//!
//! Any live `k2skn_` pass (a guest's login, an App's own link pass, an
//! owner-minted platform pass) reads its own view; nothing else does
//! (403 `skin_token`, like `skin/agents`). The answer is the catalog's
//! `me.get` row (`AppMe`):
//!
//! ```text
//! { ok, kind: guest|app|token, username, app, principalId,
//!   rooms: [{room, handle, caps, folders?}], features: [...], catalogVersion }
//! ```
//!
//! - `kind`: `guest` for a login session; `app` for this App's pass on its
//!   own gateway link; `token` for a platform pass.
//! - `rooms`: only rooms whose agent still exists, each with the caps the
//!   pass holds there (a session's per-room policy; a platform or link
//!   pass's flat caps in each of its rooms). It never lists a room the
//!   pass doesn't hold, so it is not an oracle.
//! - `agents` (app-mobile-access-v1, 0.46.3): the agent access list, one
//!   row per room `rooms` lists, with the agent's identity (displayName,
//!   color, image), caps, `can` (what the caps allow) and folders. A room
//!   not on it is invisible to this pass (every door answers `skin_room`).
//!   `serverId`, `publicHost` and `instanceId` key rows across servers.
//!   0.46.4 (agent-unread-tickets-v1, [`agent_counts`]): `unread`,
//!   `waiting`, `muted` with `thread:read` in the room and
//!   `ticketsWaiting` with `tickets:read` there.
//! - `features`: this server's `/boot-status` keys
//!   (`boot_status::features()`), so an App can tell what the server has
//!   without the owner-only `/boot-status`.
//!
//! The write half (invites, profile) is S25, 0.46.2.

use std::collections::BTreeMap;

use k2_core::skin::SkinPass;
use serde_json::json;

use crate::cli_response::CliResponse;

/// The route.
pub const ROUTE: &str = "/cli/skin/me";

/// `guest`, `app` or `token` for `pass` (`on_link` = the request came in on
/// an App's gateway link).
pub fn kind_of(pass: &SkinPass, on_link: bool) -> &'static str {
    if pass.session {
        "guest"
    } else if on_link {
        "app"
    } else {
        "token"
    }
}

/// Per-agent counts for an App pass (agent-unread-tickets-v1, 0.46.4), by
/// room (project id), over `rooms` the pass holds:
/// - with `thread:read` in the room: `unread`, `waiting`, `muted` on the
///   room's pinned Chat for the pass's own reader
///   (`read_routes::pass_reader`: an App guest is the person
///   `guest:<principalId>`, an App's own pass its `app:<serviceId>`
///   bookmark), the same numbers `thread.unread` gives that room (a room
///   whose Chat has no Thread yet: 0, 0, false);
/// - with `tickets:read` in the room: `ticketsWaiting`, the room's tickets
///   that are `waiting` or `needs_discussion` (the tickets an App may list
///   there, counted).
///
/// A room the pass doesn't hold, or a cap it lacks there, gets no field,
/// so a count never tells the pass about a room or a surface it can't
/// read. `Err` on a store failure.
pub fn agent_counts(pass: &SkinPass, rooms: &[String]) -> Result<BTreeMap<String, serde_json::Map<String, serde_json::Value>>, String> {
    use k2_core::read_state::{self, ReadError, KIND_THREAD};
    let held: Vec<&String> = rooms.iter().filter(|r| pass.has_room(r)).collect();
    let thread_rooms: Vec<&String> =
        held.iter().copied().filter(|r| pass.has_cap_in_room(r, k2_core::skin::CAP_THREAD_READ)).collect();
    let ticket_rooms: Vec<&String> =
        held.iter().copied().filter(|r| pass.has_cap_in_room(r, k2_core::skin::CAP_TICKETS_READ)).collect();
    let mut out: BTreeMap<String, serde_json::Map<String, serde_json::Value>> = BTreeMap::new();
    if thread_rooms.is_empty() && ticket_rooms.is_empty() {
        return Ok(out);
    }
    // Each lookup takes the DB lock itself: before ours.
    let chats: Vec<(&String, Option<String>)> =
        thread_rooms.iter().map(|r| (*r, crate::overlay_routes::pinned_chat_of(r))).collect();
    let person = crate::read_routes::pass_person(pass);
    let now = read_state::now_secs();
    let db = k2_core::db::shared();
    let conn = db.lock();
    if read_state::tables_exist(&conn) {
        for (room, chat) in chats {
            let (unread, waiting, muted) = match chat {
                None => (0, 0, false),
                Some(conv) => match read_state::item(&conn, &person, KIND_THREAD, &conv, now) {
                    Ok(i) => (i.unread, i.waiting, i.muted),
                    // A pinned Chat with no Thread yet: nothing unread.
                    Err(ReadError::NotFound(_)) => (0, 0, false),
                    Err(e) => return Err(e.to_string()),
                },
            };
            let row = out.entry(room.clone()).or_default();
            row.insert("unread".into(), json!(unread));
            row.insert("waiting".into(), json!(waiting));
            row.insert("muted".into(), json!(muted));
        }
    }
    if !ticket_rooms.is_empty() {
        let tickets = k2_core::feedback::count_waiting_by_project_with(&conn, &[])?;
        for room in ticket_rooms {
            let n = tickets.get(room.as_str()).map_or(0, |w| w.waiting);
            out.entry(room.clone()).or_default().insert("ticketsWaiting".into(), json!(n));
        }
    }
    Ok(out)
}

/// [`agent_counts`] merged into agent rows keyed by `key` (the room's
/// project id field). A store failure leaves the counts out (logged): the
/// fields are optional, and an App's sign-in must not fail on a badge.
pub fn add_agent_counts(pass: &SkinPass, rows: &mut [serde_json::Value], key: &str) {
    let rooms: Vec<String> = rows.iter().filter_map(|r| r.get(key).and_then(|v| v.as_str()).map(str::to_string)).collect();
    let counts = match agent_counts(pass, &rooms) {
        Ok(c) => c,
        Err(e) => {
            k2_core::log_debug!("[skin] agent counts: {e}");
            return;
        }
    };
    for row in rows.iter_mut() {
        let Some(room) = row.get(key).and_then(|v| v.as_str()).map(str::to_string) else { continue };
        if let (Some(c), Some(obj)) = (counts.get(&room), row.as_object_mut()) {
            for (k, v) in c {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
}

/// The view (pure over the pass; the handles come from the live rooms).
pub fn view(pass: &SkinPass, on_link: bool) -> serde_json::Value {
    let room_ids: Vec<String> = if pass.session { pass.room_policy.keys().cloned().collect() } else { pass.rooms.clone() };
    let live = k2_core::skin::live_agents(&room_ids);
    let mut rooms: Vec<serde_json::Value> = Vec::with_capacity(live.len());
    let mut agents: Vec<serde_json::Value> = Vec::with_capacity(live.len());
    for a in &live {
        let caps: Vec<String> = if pass.session {
            pass.room_policy.get(&a.project_id).cloned().unwrap_or_default()
        } else {
            pass.caps.clone()
        };
        let mut row = json!({ "room": a.project_id, "handle": a.handle, "caps": caps });
        // The agent access list (app-mobile-access-v1): identity, caps,
        // what they allow and folders in one row per reachable agent.
        let mut agent = json!({
            "room": a.project_id,
            "handle": a.handle,
            "displayName": a.display_name,
            "color": a.color,
            "image": a.image,
            "caps": caps,
            "can": k2_core::skin::access::can_of(&caps),
        });
        // R3: a login's room line folders (absent = the whole room).
        if let Ok(scope) = pass.path_scope(&a.project_id) {
            if !scope.is_empty() {
                row["folders"] = serde_json::to_value(&scope).unwrap_or_default();
                agent["folders"] = row["folders"].clone();
            }
        }
        rooms.push(row);
        agents.push(agent);
    }
    // agent-unread-tickets-v1: unread / waiting / muted and ticketsWaiting
    // where the caps allow them.
    add_agent_counts(pass, &mut agents, "room");
    json!({
        "ok": true,
        "kind": kind_of(pass, on_link),
        "username": pass.username,
        "app": pass.app,
        "principalId": pass.principal_id,
        "rooms": rooms,
        "agents": agents,
        "serverId": crate::boot_status::server_id(),
        "publicHost": crate::boot_status::public_host(),
        "instanceId": crate::boot_status::instance_id(),
        "features": crate::boot_status::features(),
        "catalogVersion": k2_core::contract::catalog().catalog_version,
    })
}

/// `GET /cli/skin/me` for a resolved pass.
pub fn handle(pass: &SkinPass, on_link: bool) -> CliResponse {
    CliResponse::ok_json(view(pass, on_link).to_string())
}

/// A credential that isn't a skin pass.
pub fn forbidden() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: json!({
            "ok": false,
            "error": { "code": "skin_token", "hint": "GET /cli/skin/me is for a live app or guest pass (k2skn_)" },
        })
        .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(session: bool) -> SkinPass {
        SkinPass {
            id: "t1".into(),
            principal_id: session.then(|| "p1".to_string()),
            username: "dana".into(),
            caps: vec!["thread:read".into()],
            rooms: vec![],
            session,
            room_policy: Default::default(),
            app: if session { "host".into() } else { String::new() },
            room_paths: Default::default(),
        }
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of(&pass(true), false), "guest");
        assert_eq!(kind_of(&pass(true), true), "guest", "a guest session stays a guest on a link");
        assert_eq!(kind_of(&pass(false), true), "app");
        assert_eq!(kind_of(&pass(false), false), "token");
    }

    #[test]
    fn the_route_is_an_app_row_on_both_doors() {
        let v = k2_core::contract::catalog().verb("me.get").expect("me.get row");
        assert_eq!(v.bindings.http.as_ref().map(|h| h.route.as_str()), Some(ROUTE));
        assert!(k2_core::app_surface::link_allows_http("GET", ROUTE));
        assert!(k2_core::app_surface::helper_allows_http("GET", ROUTE));
    }
}
