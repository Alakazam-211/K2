//! `/cli/postal/*` (prd-postal-in-k2-v1 S3, local half; S0's `link-p5`).
//!
//! A workspace's Postal address and its friends, behind the server's Postal
//! switch (`k2_core::postal::enabled`, **off by default**). While the
//! switch is off every route here answers 409 `postal_off`, except
//! `settings` (how the owner turns it on) and `link-p5` (the interim p5
//! path, which works while native Postal is off).
//!
//! ```text
//!   GET  /cli/postal/status?workspace=     address, counts, the network's state
//!   GET  /cli/postal/friends?workspace=    every friend and request (all states)
//!   POST /cli/postal/claim    {workspace, address}           an Owner or Admin
//!   POST /cli/postal/invite   {workspace, to, why}           agent → a postal.invite card;
//!                                                            an Owner or Admin: sent
//!   POST /cli/postal/decide   {workspace, id, decision}      the Owner (Admins if allowed);
//!                                                            K2 presents postal.pair.accept
//!   POST /cli/postal/remove   {workspace, peer}              an Owner or Admin, or the agent itself
//!   POST /cli/postal/block    {workspace, peer}              the same
//!   GET|POST /cli/postal/settings {enabled, adminsMayAccept} read: any login; write: the Owner
//!   POST /cli/postal/link-p5  {workspace}                    a postal.link-p5 card (the Owner's)
//! ```
//!
//! Every friend request, incoming or outgoing, goes through a person
//! (Rosson ruling 5): an agent's invite is a card; an incoming request is a
//! card K2 presents itself ([`crate::permission::actions::postal::present_pair_accept`]);
//! a person's own click in the Postal tab is that person's consent.
//! Tightening (remove, block) needs no card. Nothing here talks to the
//! Postal network yet: an allowed invite waits as `pending_out` for the
//! network half, and incoming requests arrive through
//! [`receive_request`], the seam that half calls.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::postal::store::{self, StoreError};
use k2_core::{cli_name, product_name};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

pub const STATUS: &str = "/cli/postal/status";
pub const FRIENDS: &str = "/cli/postal/friends";
pub const CLAIM: &str = "/cli/postal/claim";
pub const INVITE: &str = "/cli/postal/invite";
pub const DECIDE: &str = "/cli/postal/decide";
pub const REMOVE: &str = "/cli/postal/remove";
pub const BLOCK: &str = "/cli/postal/block";
pub const SETTINGS: &str = "/cli/postal/settings";
pub const LINK_P5: &str = "/cli/postal/link-p5";

pub const PATHS: &[&str] = &[STATUS, FRIENDS, CLAIM, INVITE, DECIDE, REMOVE, BLOCK, SETTINGS, LINK_P5];
const POST_ONLY: &[&str] = &[CLAIM, INVITE, DECIDE, REMOVE, BLOCK, LINK_P5];
/// The routes that answer while the switch is off.
pub const OPEN_WHILE_OFF: &[&str] = &[SETTINGS, LINK_P5];

pub const MAX_BODY: usize = 64 * 1024;

/// The note a request carries (an agent's why), at most this long.
const NOTE_MAX: usize = 300;

pub fn is_route(p: &str) -> bool {
    PATHS.contains(&p)
}

pub fn is_post_only(p: &str) -> bool {
    POST_ONLY.contains(&p)
}

pub fn too_large() -> CliResponse {
    err("413 Payload Too Large", "too_large", "a Postal request body is at most 64 KiB")
}

pub(crate) fn err(status: &'static str, code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint.into() } }).to_string(),
    }
}

fn ok(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

/// The answer every switch-gated route gives while Postal is off.
pub fn off() -> CliResponse {
    CliResponse {
        status: "409 Conflict",
        content_type: "application/json",
        body: json!({
            "ok": false,
            "state": "off",
            "error": {
                "code": "postal_off",
                "hint": format!(
                    "Postal is off on this server: the owner turns it on in Settings → General → Postal, or with {} postal enable",
                    cli_name!()
                ),
            },
        })
        .to_string(),
    }
}

fn store_err(e: StoreError) -> CliResponse {
    let status = match e {
        StoreError::NotFound | StoreError::NoAddress => "404 Not Found",
        StoreError::Db(_) => "500 Internal Server Error",
        _ => "409 Conflict",
    };
    err(status, e.code(), e.hint())
}

/// A workspace a Postal call is about.
#[derive(Debug, Clone)]
pub(crate) struct Ws {
    pub id: String,
    pub path: String,
    pub handle: String,
}

impl Ws {
    fn json(&self) -> Value {
        json!({ "id": self.id, "handle": self.handle })
    }
}

fn ws_of_id(id: &str) -> Option<Ws> {
    let path: String = k2_core::db::read(|conn| {
        conn.query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![id], |r| r.get(0)).ok()
    })?;
    Some(Ws { id: id.to_string(), path, handle: crate::permission::present::workspace_handle(id) })
}

/// A workspace by path, id or handle.
pub(crate) fn ws_of_token(token: &str) -> Option<Ws> {
    let path = crate::workspace_msg::resolve_workspace(token.trim())?;
    let id = k2_core::db::read(|conn| k2_core::workspace::agent_identity::resolve_project_id(conn, &path))?;
    Some(Ws { handle: crate::permission::present::workspace_handle(&id), id, path })
}

/// The workspace named by `workspace` (body or query), else the CLI's
/// `project`. A passport may name only its own (and gets it by default).
fn workspace_for(who: &Who, named: Option<&str>) -> Result<Ws, CliResponse> {
    let named = named.map(str::trim).filter(|s| !s.is_empty());
    if let Some((own, _)) = who.passport() {
        let mine = ws_of_id(own).ok_or_else(|| err("404 Not Found", "not_found", "your workspace isn't registered here"))?;
        if let Some(n) = named {
            if ws_of_token(n).map(|w| w.id) != Some(mine.id.clone()) {
                return Err(err("403 Forbidden", "postal_own_workspace_only", "an agent manages its own workspace's Postal address and friends only"));
            }
        }
        return Ok(mine);
    }
    let n = named.ok_or_else(|| err("400 Bad Request", "usage", "name the workspace: workspace=<name or path>"))?;
    ws_of_token(n).ok_or_else(|| err("404 Not Found", "not_found", format!("no workspace {n} on this server")))
}

fn str_of<'a>(v: &'a Value, params: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).or_else(|| params.get(key).map(String::as_str)).map(str::trim).filter(|s| !s.is_empty())
}

/// The audit actor of a person or agent.
fn actor(who: &Who) -> String {
    who.label()
}

/// Is this a person who can answer for the box at Admin level?
fn admin(who: &Who) -> bool {
    who.is_admin()
}

/// May this person accept a friend request (Rosson's 11-08 answer 2)?
fn may_accept(who: &Who) -> bool {
    who.is_owner() || (who.is_admin() && k2_core::app_settings::load().postal_admins_may_accept)
}

pub fn handle(path: &str, who: Who, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    if is_post_only(path) && !is_post {
        return CliResponse::method_not_allowed();
    }
    if !is_post && path != STATUS && path != FRIENDS && path != SETTINGS {
        return CliResponse::method_not_allowed();
    }
    if is_post && (path == STATUS || path == FRIENDS) {
        return CliResponse::method_not_allowed();
    }
    if matches!(who, Who::Invalid | Who::Wrong) {
        return CliResponse::forbidden();
    }
    if !k2_core::postal::enabled() && !OPEN_WHILE_OFF.contains(&path) {
        return off();
    }
    let v: Value = if is_post && !body.is_empty() {
        match serde_json::from_slice(body) {
            Ok(v) => v,
            Err(_) => return err("400 Bad Request", "usage", "the body is a JSON object"),
        }
    } else {
        Value::Null
    };
    match path {
        SETTINGS => settings(&who, is_post, &v),
        _ => {
            let named = str_of(&v, params, "workspace").or_else(|| params.get("project").map(String::as_str));
            let ws = match workspace_for(&who, named) {
                Ok(w) => w,
                Err(r) => return r,
            };
            match path {
                STATUS => status(&ws),
                FRIENDS => friends(&ws),
                CLAIM => claim(&who, &ws, str_of(&v, params, "address")),
                INVITE => invite(&who, &ws, str_of(&v, params, "to"), str_of(&v, params, "why")),
                DECIDE => decide(&who, &ws, str_of(&v, params, "id"), str_of(&v, params, "decision")),
                REMOVE | BLOCK => tighten(&who, &ws, path == BLOCK, str_of(&v, params, "peer")),
                LINK_P5 => crate::permission::actions::postal::ask_link_p5(&who, &ws),
                _ => CliResponse::not_found(),
            }
        }
    }
}

const PLANE_NOTE: &str = "The Postal network connection comes in a later release: the address and requests are kept here until then.";

fn status(ws: &Ws) -> CliResponse {
    let (handle, counts) = k2_core::db::read(|conn| {
        let h = store::handle_for_workspace(conn, &ws.id);
        let c = h.as_ref().map(|h| store::counts(conn, &h.id)).unwrap_or((0, 0, 0));
        (h, c)
    });
    ok(json!({
        "ok": true,
        "state": "on",
        "workspace": ws.json(),
        "handle": handle,
        "counts": { "accepted": counts.0, "pendingIn": counts.1, "pendingOut": counts.2 },
        "plane": { "connected": false, "note": PLANE_NOTE },
    }))
}

fn friends(ws: &Ws) -> CliResponse {
    let (handle, friends) = k2_core::db::read(|conn| {
        let h = store::handle_for_workspace(conn, &ws.id);
        let f = h.as_ref().map(|h| store::friends(conn, &h.id)).unwrap_or_default();
        (h, f)
    });
    ok(json!({ "ok": true, "state": "on", "workspace": ws.json(), "handle": handle, "friends": friends }))
}

fn people_only(what: &str) -> CliResponse {
    err("403 Forbidden", "postal_people_only", format!("{what} is a person's: an Owner or Admin does it in the workspace's Postal tab"))
}

fn claim(who: &Who, ws: &Ws, address: Option<&str>) -> CliResponse {
    if who.passport().is_some() || !admin(who) {
        return people_only("claiming a Postal address");
    }
    let Some(addr) = address.and_then(k2_core::postal::parse_address) else {
        return err("400 Bad Request", "bad_postal_address", "address must be a Postal address: handle::label.postal.bot");
    };
    let r = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::claim(&conn, &ws.id, &addr, &actor(who))
    };
    match r {
        Ok(h) => ok(json!({ "ok": true, "workspace": ws.json(), "handle": h, "plane": { "connected": false, "note": PLANE_NOTE } })),
        Err(e) => store_err(e),
    }
}

fn handle_of(ws: &Ws) -> Result<store::Handle, CliResponse> {
    k2_core::db::read(|conn| store::handle_for_workspace(conn, &ws.id)).ok_or_else(|| store_err(StoreError::NoAddress))
}

fn invite(who: &Who, ws: &Ws, to: Option<&str>, why: Option<&str>) -> CliResponse {
    let Some(peer) = to.and_then(k2_core::postal::parse_address) else {
        return err("400 Bad Request", "bad_postal_address", "to must be a Postal address: handle::label.postal.bot");
    };
    let handle = match handle_of(ws) {
        Ok(h) => h,
        Err(r) => return r,
    };
    let note: Option<String> = why.map(|w| w.chars().filter(|c| !c.is_control()).take(NOTE_MAX).collect());
    if who.passport().is_some() {
        // An agent asks its person first (Rosson ruling 5): a card, and
        // nothing is recorded until a person allows it.
        let existing = k2_core::db::read(|conn| store::friend_by_peer(conn, &handle.id, &peer.as_string()));
        if let Some(f) = existing {
            let e = match f.status.as_str() {
                "accepted" | "rekey_pending" => Some(StoreError::AlreadyFriends),
                "blocked" => Some(StoreError::Blocked),
                "pending_in" => Some(StoreError::TheyAsked(f.id.clone())),
                "pending_out" => return ok(json!({ "ok": true, "asked": false, "friend": f, "hint": "this request waits already" })),
                _ => None,
            };
            if let Some(e) = e {
                return store_err(e);
            }
        }
        let Some(why) = note.as_deref().filter(|w| !w.trim().is_empty()) else {
            return err("400 Bad Request", "usage", "say why: --why \"what you want to do together\" (your person reads it on the card)");
        };
        return crate::permission::actions::postal::ask_invite(ws, &peer.as_string(), why);
    }
    if !admin(who) {
        return people_only("sending a friend request");
    }
    record_invite(ws, &handle, &peer, note.as_deref(), &actor(who))
}

/// A person's allowed invite (theirs, or a `postal.invite` card's Allow).
pub(crate) fn record_invite(ws: &Ws, handle: &store::Handle, peer: &k2_core::postal::Address, note: Option<&str>, by: &str) -> CliResponse {
    let r = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::record_outgoing(&conn, handle, peer, note, by)
    };
    match r {
        Ok(f) => ok(json!({ "ok": true, "workspace": ws.json(), "friend": f, "plane": { "connected": false, "note": PLANE_NOTE } })),
        Err(e) => store_err(e),
    }
}

/// The card's Allow: the invite route's rules, as the person who clicked.
pub(crate) fn invite_as(who: &Who, ws_id: &str, to: &str, note: Option<&str>) -> CliResponse {
    if !k2_core::postal::enabled() {
        return off();
    }
    if !admin(who) {
        return people_only("sending a friend request");
    }
    let Some(ws) = ws_of_id(ws_id) else { return store_err(StoreError::NotFound) };
    let Some(peer) = k2_core::postal::parse_address(to) else {
        return err("400 Bad Request", "bad_postal_address", "to must be a Postal address: handle::label.postal.bot");
    };
    match handle_of(&ws) {
        Ok(h) => record_invite(&ws, &h, &peer, note, &actor(who)),
        Err(r) => r,
    }
}

fn decide(who: &Who, ws: &Ws, id: Option<&str>, decision: Option<&str>) -> CliResponse {
    if who.passport().is_some() {
        return err(
            "403 Forbidden",
            "postal_people_decide",
            format!("a person accepts or rejects friend requests: {} put a card on your person's Thread for each one", product_name!()),
        );
    }
    let Some(id) = id else { return err("400 Bad Request", "usage", "name the request: id=<the request's id> (k2 postal friends lists them)") };
    let accept = match decision {
        Some("accept") => true,
        Some("reject") => false,
        _ => return err("400 Bad Request", "usage", "decision is accept or reject"),
    };
    decide_as(who, &ws.id, id, accept)
}

/// The decide route's rules for one request of workspace `ws_id`, as `who`
/// (the route, and the `postal.pair.accept` card's Allow and Deny).
pub(crate) fn decide_as(who: &Who, ws_id: &str, id: &str, accept: bool) -> CliResponse {
    if !k2_core::postal::enabled() {
        return off();
    }
    if !may_accept(who) {
        return err(
            "403 Forbidden",
            "postal_owner_decides",
            "the server's owner answers Postal friend requests (the owner may let Admins too, in Settings → General → Postal)",
        );
    }
    let Some(ws) = ws_of_id(ws_id) else { return store_err(StoreError::NotFound) };
    let handle = match handle_of(&ws) {
        Ok(h) => h,
        Err(r) => return r,
    };
    let r = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match store::friend(&conn, id) {
            Some(f) if f.handle_id == handle.id => store::decide(&conn, id, accept, &actor(who)),
            _ => Err(StoreError::NotFound),
        }
    };
    match r {
        Ok(f) => {
            crate::permission::actions::postal::close_pair_cards(&ws.id, &f.id, if accept { "accepted" } else { "rejected" }, who);
            ok(json!({ "ok": true, "workspace": ws.json(), "friend": f }))
        }
        Err(e) => store_err(e),
    }
}

fn tighten(who: &Who, ws: &Ws, block: bool, peer: Option<&str>) -> CliResponse {
    if who.passport().is_none() && !admin(who) {
        return people_only(if block { "blocking an address" } else { "removing a friend" });
    }
    let Some(peer) = peer.and_then(k2_core::postal::parse_address) else {
        return err("400 Bad Request", "bad_postal_address", "peer must be a Postal address: handle::label.postal.bot");
    };
    let handle = match handle_of(ws) {
        Ok(h) => h,
        Err(r) => return r,
    };
    let r = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if block {
            store::block(&conn, &handle, &peer, &actor(who))
        } else {
            store::remove(&conn, &handle.id, &peer.as_string(), &actor(who))
        }
    };
    match r {
        Ok(f) => {
            crate::permission::actions::postal::close_pair_cards(&ws.id, &f.id, if block { "blocked" } else { "removed" }, who);
            ok(json!({ "ok": true, "workspace": ws.json(), "friend": f }))
        }
        Err(e) => store_err(e),
    }
}

fn settings(who: &Who, is_post: bool, v: &Value) -> CliResponse {
    if is_post {
        if who.passport().is_some() || !who.is_owner() {
            return err("403 Forbidden", "owner_only", "the server's owner turns Postal on or off");
        }
        let mut partial = serde_json::Map::new();
        for (wire, key) in [("enabled", "postalEnabled"), ("adminsMayAccept", "postalAdminsMayAccept")] {
            match v.get(wire) {
                None => {}
                Some(Value::Bool(b)) => {
                    partial.insert(key.into(), json!(b));
                }
                Some(_) => return err("400 Bad Request", "usage", format!("{wire} is true or false")),
            }
        }
        if partial.is_empty() {
            return err("400 Bad Request", "usage", "send enabled and/or adminsMayAccept (true or false)");
        }
        match k2_core::app_settings::update(Value::Object(partial)) {
            Ok(merged) => {
                k2_core::postal::set_enabled(merged.postal_enabled);
                crate::hire_routes::audit("postal-settings", &actor(who), &format!("enabled={} adminsMayAccept={}", merged.postal_enabled, merged.postal_admins_may_accept));
            }
            Err(e) => return err("500 Internal Server Error", "internal", e),
        }
    }
    let s = k2_core::app_settings::load();
    ok(json!({ "ok": true, "enabled": k2_core::postal::enabled(), "adminsMayAccept": s.postal_admins_may_accept }))
}

/// An incoming friend request for workspace `ws_id`'s address (the seam
/// the Postal network half calls when the plane reports one). Records it
/// and presents K2's `postal.pair.accept` card; never wakes the agent. A
/// blocked peer or a full queue is refused here, before any card.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn receive_request(
    ws_id: &str,
    peer: &k2_core::postal::Address,
    owner_name: Option<&str>,
    fingerprint: Option<&str>,
    pair_id: Option<&str>,
    note: Option<&str>,
) -> Result<Value, StoreError> {
    let ws = ws_of_id(ws_id).ok_or(StoreError::NotFound)?;
    let rec = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let h = store::handle_for_workspace(&conn, ws_id).ok_or(StoreError::NoAddress)?;
        store::record_incoming(&conn, &h, peer, owner_name, fingerprint, pair_id, note)?
    };
    let f = match rec {
        store::Incoming::New(f) | store::Incoming::Known(f) => f,
    };
    let card = if f.status == "pending_in" {
        crate::permission::actions::postal::present_pair_accept(&ws, &f)
    } else {
        Value::Null
    };
    Ok(json!({ "friend": f, "card": card }))
}

/// `Who` for a card's answerer, so a card's apply runs a route's own rules.
pub(crate) fn who_of(a: &crate::permission::actions::Answerer) -> Who {
    match a {
        crate::permission::actions::Answerer::OwnerToken => Who::Owner,
        crate::permission::actions::Answerer::Login { user, role } => {
            Who::Login { username: user.clone(), role: Role::parse_wire(role).unwrap_or(Role::Member) }
        }
    }
}

/// Tests: hold the server's Postal switch at `on` until the guard drops
/// (one test at a time; the switch is process-wide).
#[cfg(test)]
pub(crate) fn switch_for_test(on: bool) -> SwitchGuard {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = k2_core::postal::enabled();
    k2_core::postal::set_enabled(on);
    SwitchGuard { _g: g, prev }
}

#[cfg(test)]
pub(crate) struct SwitchGuard {
    _g: std::sync::MutexGuard<'static, ()>,
    prev: bool,
}

#[cfg(test)]
impl Drop for SwitchGuard {
    fn drop(&mut self) {
        k2_core::postal::set_enabled(self.prev);
    }
}

#[cfg(test)]
#[path = "postal_routes_tests.rs"]
mod tests;
