//! The Garden bridge's person rows (garden-person-rows-v1, K2 0.46.3;
//! research `research-app-mobile-surface-parity-v1` groups B and I;
//! Rosson 2026-10-20: "the bridge in the Gardens would get all of those as
//! well", no Projects on the bridge).
//!
//! One catalog row, two transports (room-scope §0.2): the App rows
//! `thread.catchup`, `thread.unread`, `thread.mute`, `tickets.list`,
//! `tickets.show`, `tickets.create`, `tickets.comment`, `tickets.resolve`,
//! `activity.snapshot`, `files.list`, `files.read` and `files.readBinary`
//! also answer a Garden widget through `POST /cli/zen/call`, with the same
//! arguments and the same answer the App's server SDK hands its caller
//! (the row's `responseNames` applied and the closed response schema kept,
//! [`project`]): never a host path, `conversation_id`, `rev` or session id.
//!
//! **Who.** A Garden is the person on their own computer (owner token
//! only, `call::handle`). The person's rights:
//! - **Thread rows** read and change the person's OWN state: their
//!   catch-up (as the person reads Threads in K2), their unread counts and
//!   `badgeKey` (the same key as on their desktop and phone), their mutes
//!   (every one of their sockets and phones hears the change). Threads
//!   only: project chat is left out of the counts and refused.
//! - **Tickets, activity and files** act in any room on this computer for
//!   a widget the person (or K2) made; a widget an agent made acts in its
//!   maker's own room only (creator rights, as `guests.*`), and an
//!   unsigned one is `author_unknown`. A room outside that is `not_bound`;
//!   a ticket outside it is `not_found` (no oracle). Reads answer the
//!   App's room-relative projection; ticket writes are the person's own
//!   (comments by `owner`, a new ticket through the person door: no agent
//!   brief rules).
//!
//! Every row needs its cap in the widget's manifest (`cap_not_granted`),
//! spends the placement's read or write budget, and a paused placement
//! can't write. Every write leaves an audit line.

use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Map, Value as J};

use k2_core::contract::{catalog, Effect, VerbRow};
use k2_core::garden_data::{authors, MadeBy};
use k2_core::skin::{RoomPolicy, SkinPass};

use super::budget::{self, BudgetKey, Spend};
use super::call::{resolve, CallBody, Placed};
use crate::cli_response::CliResponse;

/// The rows this file answers, in catalog order.
pub const VERBS: [&str; 12] = [
    "thread.catchup",
    "thread.unread",
    "thread.mute",
    "files.list",
    "files.read",
    "files.readBinary",
    "tickets.list",
    "tickets.show",
    "tickets.create",
    "tickets.comment",
    "tickets.resolve",
    "activity.snapshot",
];

pub fn is_verb(verb: &str) -> bool {
    VERBS.contains(&verb)
}

/// The person a Garden is (the owner token, this computer).
const PERSON: &str = "owner";

fn status_line(code: u16) -> &'static str {
    match code {
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        415 => "415 Unsupported Media Type",
        423 => "423 Locked",
        429 => "429 Too Many Requests",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

fn no(status: u16, code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status: status_line(status),
        content_type: "application/json",
        body: json!({ "error": { "code": code, "hint": hint.into() } }).to_string(),
    }
}

/// A handler's refusal in the widget door's envelope (`{"error": {code,
/// hint, …}}`), whatever shape the route used (`{ok:false, error:{…}}`,
/// `{error:"text"}`).
fn passthrough(r: CliResponse) -> CliResponse {
    let code: u16 = r.status.split(' ').next().and_then(|c| c.parse().ok()).unwrap_or(500);
    let v: J = serde_json::from_str(&r.body).unwrap_or(J::Null);
    let mut err = match v.get("error") {
        Some(J::Object(o)) => o.clone(),
        Some(J::String(s)) => {
            let mut m = Map::new();
            m.insert("code".into(), json!(if code == 404 { "not_found" } else { "failed" }));
            m.insert("hint".into(), json!(s));
            m
        }
        _ => {
            let mut m = Map::new();
            m.insert("code".into(), json!("failed"));
            m.insert("hint".into(), json!(r.body));
            m
        }
    };
    if !err.contains_key("hint") {
        let hint = err.get("code").and_then(J::as_str).unwrap_or("failed").to_string();
        err.insert("hint".into(), json!(hint));
    }
    CliResponse { status: status_line(code), content_type: "application/json", body: json!({ "error": err }).to_string() }
}

/// The answer the App's server SDK hands its caller (`sdk/server/src/
/// wire.ts` `project`): only the schema's properties, at every depth, read
/// under their wire names (`responseNames`: `relPath` ← `path`).
pub fn project(schema: &J, value: &J, names: &BTreeMap<String, String>) -> J {
    if value.is_null() {
        return J::Null;
    }
    let types: Vec<&str> = match schema.get("type") {
        Some(J::String(t)) => vec![t.as_str()],
        Some(J::Array(a)) => a.iter().filter_map(J::as_str).collect(),
        _ => Vec::new(),
    };
    if types.contains(&"json") {
        return value.clone();
    }
    match value {
        J::Array(items) => match schema.get("items") {
            Some(s) => J::Array(items.iter().map(|x| project(s, x, names)).collect()),
            None => value.clone(),
        },
        J::Object(o) => {
            if let Some(J::Object(props)) = schema.get("properties") {
                let mut out = Map::new();
                for (name, sub) in props {
                    let wire = names.get(name).map(String::as_str).unwrap_or(name);
                    if let Some(x) = o.get(wire) {
                        out.insert(name.clone(), project(sub, x, names));
                    }
                }
                return J::Object(out);
            }
            match schema.get("additionalProperties") {
                Some(map @ J::Object(_)) => J::Object(o.iter().map(|(k, x)| (k.clone(), project(map, x, names))).collect()),
                _ => value.clone(),
            }
        }
        _ => value.clone(),
    }
}

/// The row's one `args` object, checked against its request schema
/// (unknown keys and missing required ones are `bad_request`, as the
/// frame's own guard says them).
fn args_of(row: &VerbRow, args: &[J]) -> Result<Map<String, J>, CliResponse> {
    let schema = row.request.as_array().and_then(|a| a.first());
    let given = match args.first() {
        None | Some(J::Null) => Map::new(),
        Some(J::Object(o)) => o.clone(),
        Some(_) => return Err(no(400, "bad_request", format!("{} takes one argument, an object", row.verb))),
    };
    if args.len() > 1 {
        return Err(no(400, "bad_request", format!("{} takes one argument, an object", row.verb)));
    }
    let Some(schema) = schema else {
        if !given.is_empty() {
            return Err(no(400, "bad_request", format!("{} takes no arguments", row.verb)));
        }
        return Ok(given);
    };
    let props = schema.get("properties").and_then(J::as_object);
    for k in given.keys() {
        if !props.is_some_and(|p| p.contains_key(k)) {
            return Err(no(400, "bad_request", format!("{}: unknown argument '{k}'", row.verb)));
        }
    }
    for r in schema.get("required").and_then(J::as_array).into_iter().flatten().filter_map(J::as_str) {
        if given.get(r).is_none_or(J::is_null) {
            return Err(no(400, "bad_request", format!("{} needs {r}", row.verb)));
        }
    }
    Ok(given)
}

/// Arguments under their wire names (the row's `params`).
fn wire_args(row: &VerbRow, args: &Map<String, J>) -> Map<String, J> {
    let params = row.bindings.http.as_ref().map(|h| h.params.clone()).unwrap_or_default();
    args.iter().map(|(k, v)| (params.get(k).cloned().unwrap_or_else(|| k.clone()), v.clone())).collect()
}

/// A GET's query parameters (strings, as a URL carries them).
fn query_of(wire: &Map<String, J>) -> HashMap<String, String> {
    wire.iter()
        .map(|(k, v)| {
            let s = match v {
                J::String(s) => s.clone(),
                J::Bool(b) => if *b { "1".into() } else { "0".into() },
                other => other.to_string(),
            };
            (k.clone(), s)
        })
        .collect()
}

/// Where a tickets / activity / files row may act for this widget.
enum RoomReach {
    /// The person's own (or K2's) widget: any room on this computer.
    Any,
    /// An agent-made widget: its maker's room only.
    Maker(String),
}

fn room_reach(w: &Placed) -> Result<RoomReach, CliResponse> {
    if k2_core::zen::garden_access::enabled() {
        // Garden access (G15, §1.6): a version that passed the signer gate
        // (checked in `run`) acts as the person in any room; maker-based
        // reach is retired.
        return Ok(RoomReach::Any);
    }
    let maker = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        authors::maker_of(&conn, &w.widget, &w.hash).map_err(|e| crate::garden_data::response(&e))?
    };
    match maker {
        MadeBy::Owner | MadeBy::K2 => Ok(RoomReach::Any),
        MadeBy::Agent { project_id } => Ok(RoomReach::Maker(project_id)),
        MadeBy::Unclaimed => Err(no(
            403,
            "author_unknown",
            format!("nobody signed this version of {0}. Run `k2 zen validate --widget {0}`, or click It's mine", w.widget),
        )),
    }
}

/// A room the widget may act in: `(project id, handle)`. Anything else is
/// `not_bound`, the same answer as a room that doesn't exist.
fn room_in_reach(reach: &RoomReach, token: &str) -> Result<(String, String), CliResponse> {
    let not_bound = || no(403, "not_bound", format!("'{token}' isn't an agent this widget reaches on this computer"));
    let ws = crate::fs_routes::resolve_owner_workspace(token).map_err(|_| not_bound())?;
    if let RoomReach::Maker(p) = reach {
        if *p != ws.project_id {
            return Err(not_bound());
        }
    }
    Ok((ws.project_id, ws.handle))
}

/// A ticket the widget may act on: its room, or `not_found` (missing and
/// out of reach answer the same).
fn ticket_room(reach: &RoomReach, id: &str) -> Result<String, CliResponse> {
    let missing = || no(404, "not_found", format!("no ticket '{id}' this widget reaches"));
    let full = k2_core::feedback::resolve_id_prefix(id).map_err(|_| missing())?;
    let item = k2_core::feedback::get_item(&full).ok_or_else(missing)?;
    if let RoomReach::Maker(p) = reach {
        if *p != item.project_id {
            return Err(missing());
        }
    }
    Ok(item.project_id)
}

/// The person's rights in one room, as the App handlers read a pass: one
/// room line with `cap`, the whole room (no folder scope). Never minted,
/// never stored, never on the wire: it only selects the App's
/// room-relative projection (no host paths) for the person's own read.
fn room_view(project_id: &str, cap: &str) -> SkinPass {
    let mut room_policy = RoomPolicy::new();
    room_policy.insert(project_id.to_string(), vec![cap.to_string()]);
    SkinPass {
        id: "garden-person".to_string(),
        principal_id: None,
        username: PERSON.to_string(),
        caps: vec![cap.to_string()],
        rooms: vec![project_id.to_string()],
        session: true,
        room_policy,
        app: String::new(),
        room_paths: Default::default(),
    }
}

/// One person row through the widget door (owner token and Origin already
/// checked by `call::handle`).
pub fn handle(b: &CallBody) -> CliResponse {
    match run(b) {
        Ok(v) => crate::garden_data::ok(json!({ "ok": true, "result": v })),
        Err(r) => r,
    }
}

fn run(b: &CallBody) -> Result<J, CliResponse> {
    let row = catalog().verb(&b.verb).ok_or_else(|| no(400, "unknown_verb", format!("no verb {}", b.verb)))?;
    let w = resolve(b).map_err(|e| crate::garden_data::response(&e))?;
    if let Some(cap) = row.cap.as_deref() {
        crate::garden_data::call::check_cap(&w, &b.verb, cap).map_err(|e| crate::garden_data::response(&e))?;
    }
    // Garden access (switch on, G15): a person row acts as the person, so
    // only a version that passes the signer gate makes one.
    if !w.signed {
        return Err(crate::garden_data::response(&crate::garden_data::call::needs_editor(&w)));
    }
    let write = row.effect == Effect::Write;
    if write && crate::zen_routes::placement_pause(&w.garden, &w.placement).is_some() {
        return Err(no(423, "paused", "this widget is paused: too many writes. The person can Resume it on the widget"));
    }
    let key = BudgetKey::Placement { garden: w.garden.clone(), placement: w.placement.clone() };
    budget::spend(&key, if write { Spend::Write } else { Spend::Read }).map_err(|e| crate::garden_data::response(&e))?;
    let args = args_of(row, &b.args)?;
    let wire = wire_args(row, &args);
    let r = answer(row, &w, &args, &wire)?;
    if r.status != "200 OK" {
        return Err(passthrough(r));
    }
    let v: J = serde_json::from_str(&r.body).map_err(|e| no(500, "failed", format!("{}: {e}", b.verb)))?;
    if write {
        crate::garden_data::audit::line("zen.person.write", &format!("garden:{}", w.widget), &format!("verb={} placement={}/{}", b.verb, w.garden, w.placement));
    }
    let names = row.bindings.http.as_ref().map(|h| h.response_names.clone()).unwrap_or_default();
    Ok(project(&row.response, &v, &names))
}

fn str_arg<'a>(args: &'a Map<String, J>, k: &str) -> &'a str {
    args.get(k).and_then(J::as_str).map(str::trim).unwrap_or("")
}

fn answer(row: &VerbRow, w: &Placed, args: &Map<String, J>, wire: &Map<String, J>) -> Result<CliResponse, CliResponse> {
    let body = J::Object(wire.clone()).to_string();
    Ok(match row.verb.as_str() {
        // ── the person's own Thread state ───────────────────────────────
        "thread.unread" => match crate::read_routes::garden_unread(PERSON) {
            Ok(v) => CliResponse::ok_json(v.to_string()),
            Err(e) => no(500, "failed", e.to_string()),
        },
        "thread.mute" => crate::read_routes::garden_thread_mute(PERSON, wire),
        "thread.catchup" => {
            let flight = format!("garden:{}/{}", w.garden, w.placement);
            crate::overlay_routes::catchup::handle(
                &HashMap::new(),
                body.as_bytes(),
                Some(&crate::compute_routes::Who::Owner),
                Some(&flight),
                Some(&crate::sidecar_routes::Caller::Owner),
            )
        }
        // ── rooms on this computer ──────────────────────────────────────
        "activity.snapshot" | "files.list" | "files.read" | "files.readBinary" | "tickets.list" | "tickets.create" => {
            let reach = room_reach(w)?;
            let token_key = if row.verb.starts_with("tickets.") { "project" } else { "workspace" };
            let (room, handle) = room_in_reach(&reach, str_arg(args, token_key))?;
            let mut query = query_of(wire);
            query.insert(token_key.to_string(), handle.clone());
            let cap = row.cap.as_deref().unwrap_or_default();
            match row.verb.as_str() {
                "activity.snapshot" => crate::activity_routes::handle_snapshot_gated(&query, Some(room_view(&room, cap))),
                "files.list" => crate::fs_routes::handle_read_dir_gated(&query, Some(room_view(&room, cap))),
                "files.read" => crate::fs_routes::handle_read_file_gated(&query, Some(room_view(&room, cap))),
                "files.readBinary" => crate::fs_routes::handle_read_binary_gated(&query, Some(room_view(&room, cap))),
                "tickets.list" => crate::feedback_routes::handle_list_gated(&query, Some(room_view(&room, cap))),
                _ => {
                    // A person files it (no agent brief rules), in the room
                    // the reach check resolved.
                    let mut b = wire.clone();
                    b.insert("project".into(), json!(handle));
                    crate::feedback_routes::dispatch_post_as_gated(
                        "/cli/feedback/create",
                        J::Object(b).to_string().as_bytes(),
                        PERSON,
                        None,
                        crate::feedback_routes::CreateDoor::Connect,
                    )
                }
            }
        }
        "tickets.show" | "tickets.comment" | "tickets.resolve" => {
            let reach = room_reach(w)?;
            let room = ticket_room(&reach, str_arg(args, "id"))?;
            match row.verb.as_str() {
                "tickets.show" => {
                    crate::feedback_routes::handle_show_gated(&query_of(wire), Some(room_view(&room, row.cap.as_deref().unwrap_or_default())))
                }
                path => crate::feedback_routes::dispatch_post_as_gated(
                    if path == "tickets.comment" { "/cli/feedback/comment" } else { "/cli/feedback/resolve" },
                    body.as_bytes(),
                    PERSON,
                    None,
                    crate::feedback_routes::CreateDoor::Owner,
                ),
            }
        }
        other => no(400, "unknown_verb", format!("{other} isn't a Garden person row")),
    })
}

#[cfg(test)]
#[path = "person_rows_tests.rs"]
mod tests;
