//! `/cli/postal/*` (prd-postal-in-k2-v1 S3, local half; S0's `link-p5`).
//!
//! A workspace's Postal address and its friends, behind the server's Postal
//! switch (`k2_core::postal::enabled`, **off by default**). While the
//! switch is off every route here answers `postal_off` (409 to a person,
//! 403 to an agent, whose refusal teaches the `postal.enable` card),
//! except `settings` and `enable` (how the owner turns it on) and
//! `link-p5` (the interim p5 path, which works while native Postal is off).
//!
//! ```text
//!   GET  /cli/postal/status?workspace=     address, counts, the network's state
//!   GET  /cli/postal/friends?workspace=    every friend and request (all states)
//!   POST /cli/postal/claim    {workspace, address, why?}     an Owner or Admin;
//!                                                            agent → a postal.claim card
//!   POST /cli/postal/invite   {workspace, to, why}           agent → a postal.invite card;
//!                                                            an Owner or Admin: recorded
//!   POST /cli/postal/decide   {workspace, id, decision}      the Owner (Admins if allowed);
//!                                                            agent: reject itself, accept →
//!                                                            K2's postal.pair.accept card
//!   POST /cli/postal/remove   {workspace, peer}              an Owner or Admin, or the agent itself
//!   POST /cli/postal/block    {workspace, peer}              the same
//!   POST /cli/postal/enable   {why?}                         the Owner; agent → a postal.enable card
//!   GET|POST /cli/postal/settings {enabled, adminsMayAccept} read: any login; write: the Owner
//!   POST /cli/postal/link-p5  {workspace}                    a postal.link-p5 card (the Owner's)
//! ```
//!
//! Every step an agent takes to befriend another agent goes through a
//! person (Rosson ruling 5; 2026-11-18): the switch, its address, an
//! invite and the p5 link are cards K2 puts up in the same answer
//! (`asked: true`; the CLI's exit 2); an incoming request is a card K2
//! presents itself ([`crate::permission::actions::postal::present_pair_accept`]);
//! a person's own click in the Postal tab is that person's consent.
//! Tightening (remove, block, rejecting a request) needs no card.
//!
//! **Waits on others, never pretended** (each answer says so): nothing
//! here talks to the Postal network. An allowed invite waits as
//! `pending_out` (`sent: false`, `network: "not_available_yet"`) and is
//! queued for the SDK ([`k2_core::postal::gate`]); incoming requests and
//! changed keys arrive only through [`receive_request`] / [`receive_rekey`]
//! ([`K2Gate`], the SDK's `HumanGate`), which the network half calls. That
//! half needs postal-sdk wire v2 (S1, Postal Bot) and the plane's P-1..P-8
//! (K2 Web). `k2 postal send|route|adopt` answer "not available yet" from
//! the CLI (S4, S5, S7).

use std::collections::HashMap;

use k2_core::cli_name;
use k2_core::connect_users::Role;
use k2_core::postal::store::{self, StoreError};
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
pub const ENABLE: &str = "/cli/postal/enable";

pub const PATHS: &[&str] = &[STATUS, FRIENDS, CLAIM, INVITE, DECIDE, REMOVE, BLOCK, SETTINGS, LINK_P5, ENABLE];
const POST_ONLY: &[&str] = &[CLAIM, INVITE, DECIDE, REMOVE, BLOCK, LINK_P5, ENABLE];
/// The routes that answer while the switch is off.
pub const OPEN_WHILE_OFF: &[&str] = &[SETTINGS, LINK_P5, ENABLE];

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

/// The answer every switch-gated route gives an agent while Postal is
/// off: a refusal (403), so the walls map teaches `postal.enable`, and a
/// hint that names the card.
fn off_for(who: &Who) -> CliResponse {
    if who.passport().is_none() {
        return off();
    }
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: json!({
            "ok": false,
            "state": "off",
            "error": {
                "code": "postal_off",
                "hint": format!(
                    "Postal is off on this server, and only the server's owner turns it on: {} postal enable puts that card on your person's Thread",
                    cli_name!()
                ),
            },
        })
        .to_string(),
    }
}

/// The answer a person gets from a switch-gated route while Postal is off.
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
        return off_for(&who);
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
        ENABLE => enable(&who, str_of(&v, params, "why")),
        _ => {
            let named = str_of(&v, params, "workspace").or_else(|| params.get("project").map(String::as_str));
            let ws = match workspace_for(&who, named) {
                Ok(w) => w,
                Err(r) => return r,
            };
            match path {
                STATUS => status(&ws),
                FRIENDS => friends(&ws),
                CLAIM => claim(&who, &ws, str_of(&v, params, "address"), str_of(&v, params, "why")),
                INVITE => invite(&who, &ws, str_of(&v, params, "to"), str_of(&v, params, "why")),
                DECIDE => decide(&who, &ws, str_of(&v, params, "id"), str_of(&v, params, "decision")),
                REMOVE | BLOCK => tighten(&who, &ws, path == BLOCK, str_of(&v, params, "peer")),
                LINK_P5 => crate::permission::actions::postal::ask_link_p5(&who, &ws),
                _ => CliResponse::not_found(),
            }
        }
    }
}

/// Serve one `/cli/postal/*` request on the dispatcher's TCP stream (`buf`
/// is its read buffer; the head is already consumed). `false` when the
/// connection must close (a GET on a POST route, a body over the cap).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve_tcp(
    stream: &mut tokio::net::TcpStream,
    buf: &mut [u8],
    p: &str,
    path: &str,
    query: &str,
    is_post: bool,
    bearer: Option<&str>,
    owner_token: &str,
) -> bool {
    use tokio::io::AsyncReadExt;
    if is_post_only(p) && !crate::routes::http::require_post(stream, buf, is_post).await {
        return false;
    }
    let body_bytes = if is_post {
        match crate::routes::http::read_post_body_capped(stream, buf, MAX_BODY).await {
            Ok(b) => b,
            Err(refused) => {
                let r = too_large();
                crate::routes::http::refuse_body_too_large(stream, &refused, r.status, r.content_type, &r.body).await;
                return false;
            }
        }
    } else {
        let _ = stream.read(buf).await;
        Vec::new()
    };
    let who = crate::compute_routes::who_from_tcp(query, bearer, owner_token);
    let params = crate::routes::http::parse_params(path, query);
    let p_owned = p.to_string();
    let r = tokio::task::spawn_blocking(move || handle(&p_owned, who, is_post, &body_bytes, &params))
        .await
        .unwrap_or_else(|e| CliResponse::internal_error(format!("worker join: {e}")));
    crate::routes::http::send_response(stream, r.status, r.content_type, &r.body).await;
    true
}

const PLANE_NOTE: &str = "The Postal network connection comes in a later release: the address and requests are kept here until then.";

/// What every answer says about the Postal network while the network half
/// waits (postal-sdk wire v2, the plane's P-1..P-8).
fn plane() -> Value {
    json!({ "connected": k2_core::postal::gate::network_available(), "state": k2_core::postal::gate::NOT_YET, "note": PLANE_NOTE })
}

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
        "plane": plane(),
    }))
}

fn friends(ws: &Ws) -> CliResponse {
    let (handle, friends) = k2_core::db::read(|conn| {
        let h = store::handle_for_workspace(conn, &ws.id);
        let f = h.as_ref().map(|h| store::friends(conn, &h.id)).unwrap_or_default();
        (h, f)
    });
    // A request that carries a pairing code: the tab says it can't be
    // accepted yet. The code itself is never in this list (agents read it).
    let friends: Vec<Value> = friends
        .into_iter()
        .map(|f| {
            let waiting = k2_core::postal::gate::code(&f.id).is_some();
            let mut v = serde_json::to_value(&f).unwrap_or(Value::Null);
            if let Some(o) = v.as_object_mut() {
                o.insert("codeWaiting".into(), json!(waiting));
            }
            v
        })
        .collect();
    ok(json!({ "ok": true, "state": "on", "workspace": ws.json(), "handle": handle, "friends": friends, "plane": plane() }))
}

fn people_only(what: &str) -> CliResponse {
    err("403 Forbidden", "postal_people_only", format!("{what} is a person's: an Owner or Admin does it in the workspace's Postal tab"))
}

fn claim(who: &Who, ws: &Ws, address: Option<&str>, why: Option<&str>) -> CliResponse {
    let Some(addr) = address.and_then(k2_core::postal::parse_address) else {
        return err("400 Bad Request", "bad_postal_address", "address must be a Postal address: handle::label.postal.bot");
    };
    if let Some((_, session)) = who.passport() {
        // An agent asks its person (Rosson 2026-11-18): a postal.claim card.
        // What a card can't fix is answered here, before one goes up.
        let a = addr.as_string();
        let (mine, holder) =
            k2_core::db::read(|conn| (store::handle_for_workspace(conn, &ws.id), store::address_holder(conn, &a)));
        match (mine, holder) {
            (Some(h), _) if h.address == a => {
                return ok(json!({ "ok": true, "workspace": ws.json(), "handle": h, "already": true, "plane": plane() }))
            }
            (Some(h), _) => return store_err(StoreError::HasAddress(h.address)),
            (None, Some(other)) if other != ws.id => return store_err(StoreError::AddressTaken),
            _ => {}
        }
        let why = why.map(|w| w.chars().filter(|c| !c.is_control()).take(NOTE_MAX).collect::<String>());
        return crate::permission::actions::postal::ask_claim(ws, session, &a, why.as_deref().filter(|w| !w.trim().is_empty()));
    }
    if !admin(who) {
        return people_only("claiming a Postal address");
    }
    record_claim(who, ws, &addr)
}

fn record_claim(who: &Who, ws: &Ws, addr: &k2_core::postal::Address) -> CliResponse {
    let r = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::claim(&conn, &ws.id, addr, &actor(who))
    };
    match r {
        Ok(h) => ok(json!({ "ok": true, "workspace": ws.json(), "handle": h, "plane": plane() })),
        Err(e) => store_err(e),
    }
}

/// The `postal.claim` card's Allow: the claim route's rules, as the person
/// who clicked.
pub(crate) fn claim_as(who: &Who, ws_id: &str, address: &str) -> CliResponse {
    if !k2_core::postal::enabled() {
        return off();
    }
    if who.passport().is_some() || !admin(who) {
        return people_only("claiming a Postal address");
    }
    let Some(ws) = ws_of_id(ws_id) else { return store_err(StoreError::NotFound) };
    let Some(addr) = k2_core::postal::parse_address(address) else {
        return err("400 Bad Request", "bad_postal_address", "address must be a Postal address: handle::label.postal.bot");
    };
    record_claim(who, &ws, &addr)
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
        Err(_) if who.passport().is_some() => {
            return err(
                "404 Not Found",
                "postal_no_address",
                format!(
                    "this workspace has no Postal address yet: ask for one first with {} postal claim <handle::label.postal.bot> (a card for your person)",
                    cli_name!()
                ),
            )
        }
        Err(r) => return r,
    };
    let note: Option<String> = why.map(|w| w.chars().filter(|c| !c.is_control()).take(NOTE_MAX).collect());
    if let Some((_, session)) = who.passport() {
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
        return crate::permission::actions::postal::ask_invite(ws, session, &peer.as_string(), why);
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
        Ok(f) => {
            // Queued for the SDK; nothing is sent in this release.
            k2_core::postal::gate::record_answer(k2_core::postal::gate::Answer::Invite { friend_id: f.id.clone(), peer: f.peer_address.clone() });
            ok(json!({
                "ok": true,
                "workspace": ws.json(),
                "friend": f,
                "sent": false,
                "network": k2_core::postal::gate::NOT_YET,
                "plane": plane(),
            }))
        }
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
    let Some(id) = id else {
        return err("400 Bad Request", "usage", format!("name the request: id=<the request's id> ({} postal friends lists them)", cli_name!()));
    };
    let accept = match decision {
        Some("accept") => true,
        Some("reject") => false,
        _ => return err("400 Bad Request", "usage", "decision is accept or reject"),
    };
    if who.passport().is_some() {
        let f = match handle_of(ws).and_then(|h| {
            k2_core::db::read(|conn| store::friend(conn, id)).filter(|f| f.handle_id == h.id).ok_or_else(|| store_err(StoreError::NotFound))
        }) {
            Ok(f) => f,
            Err(r) => return r,
        };
        if !matches!(f.status.as_str(), "pending_in" | "rekey_pending") {
            return store_err(StoreError::NotPending(f.status));
        }
        if accept {
            // Only a person accepts: K2's own card for this request, put up
            // again when it expired (Rosson ruling 5).
            return crate::permission::actions::postal::ask_pair_accept(ws, &f);
        }
        // Turning a request down is tightening: the agent's own.
        return record_decision(who, ws, &f.id, false);
    }
    decide_as(who, &ws.id, id, accept)
}

/// The decide route's rules for one request of workspace `ws_id`, as a
/// person `who` (the route, and the `postal.pair.accept` card's Allow and
/// Deny). A request that carries a pairing code can't be accepted in this
/// release: the SDK checks the code the person types from the **other**
/// box (`Handshake::confirm`), and that waits on the network half.
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
    if accept && k2_core::postal::gate::code(id).is_some() {
        return err(
            "409 Conflict",
            k2_core::postal::gate::CODE_NOT_YET,
            "this request carries a pairing code, and checking it needs this server's Postal network connection, which comes in a later release: it can't be accepted yet (Reject or Block still work)",
        );
    }
    record_decision(who, &ws, id, accept)
}

/// Write one decision on a request of `ws`, close its cards, and queue the
/// answer for the SDK.
fn record_decision(who: &Who, ws: &Ws, id: &str, accept: bool) -> CliResponse {
    let handle = match handle_of(ws) {
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
            use k2_core::postal::gate::{self, Answer};
            let (friend_id, pair_id, peer) = (f.id.clone(), f.pair_id.clone(), f.peer_address.clone());
            gate::record_answer(if accept {
                Answer::Accept { friend_id, pair_id, peer }
            } else {
                Answer::Reject { friend_id, pair_id, peer }
            });
            gate::clear_code(&f.id);
            crate::permission::actions::postal::close_pair_cards(&ws.id, &f.id, if accept { "accepted" } else { "rejected" }, who);
            ok(json!({ "ok": true, "workspace": ws.json(), "friend": f, "plane": plane() }))
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
            k2_core::postal::gate::record_answer(k2_core::postal::gate::Answer::Revoke {
                friend_id: f.id.clone(),
                pair_id: f.pair_id.clone(),
                peer: f.peer_address.clone(),
                block,
            });
            k2_core::postal::gate::clear_code(&f.id);
            crate::permission::actions::postal::close_pair_cards(&ws.id, &f.id, if block { "blocked" } else { "removed" }, who);
            ok(json!({ "ok": true, "workspace": ws.json(), "friend": f }))
        }
        Err(e) => store_err(e),
    }
}

/// `k2 postal enable`: the Owner turns Postal on; an agent asks for it on
/// a `postal.enable` card (Rosson 2026-11-18: no Settings hunt).
fn enable(who: &Who, why: Option<&str>) -> CliResponse {
    if let Some((own, session)) = who.passport() {
        if k2_core::postal::enabled() {
            return settings(who, false, &Value::Null);
        }
        let Some(ws) = ws_of_id(own) else { return err("404 Not Found", "not_found", "your workspace isn't registered here") };
        let why = why.map(|w| w.chars().filter(|c| !c.is_control()).take(NOTE_MAX).collect::<String>());
        return crate::permission::actions::postal::ask_enable(&ws, session, why.as_deref().filter(|w| !w.trim().is_empty()));
    }
    enable_as(who)
}

/// The Owner turns Postal on (the enable route, and the `postal.enable`
/// card's Allow).
pub(crate) fn enable_as(who: &Who) -> CliResponse {
    settings(who, true, &json!({ "enabled": true }))
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
/// the Postal network half calls when the plane reports one; see
/// [`K2Gate`]). Records it, keeps this box's pairing code (people only), and
/// presents K2's `postal.pair.accept` card; never wakes the agent. A
/// blocked peer or a full queue is refused here, before any card.
#[cfg_attr(not(test), allow(dead_code))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn receive_request(
    ws_id: &str,
    peer: &k2_core::postal::Address,
    owner_name: Option<&str>,
    fingerprint: Option<&str>,
    pair_id: Option<&str>,
    note: Option<&str>,
    code: Option<&str>,
) -> Result<Value, StoreError> {
    let ws = ws_of_id(ws_id).ok_or(StoreError::NotFound)?;
    let rec = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let h = store::handle_for_workspace(&conn, ws_id).ok_or(StoreError::NoAddress)?;
        store::record_incoming(&conn, &h, peer, owner_name, fingerprint, pair_id, note)?
    };
    let f = match rec {
        store::Incoming::New(f) => {
            if let Some(c) = code {
                k2_core::postal::gate::set_code(&f.id, c);
            }
            f
        }
        store::Incoming::Known(f) => f,
    };
    let card = if f.status == "pending_in" {
        crate::permission::actions::postal::present_pair_accept(&ws, &f)
    } else {
        Value::Null
    };
    Ok(json!({ "friend": f, "card": card }))
}

/// A friend's key changed on the Postal network (the SDK's `rekey` gate):
/// the friend waits as `rekey_pending` and K2 presents the same card, in
/// "trust the new key" words. Never wakes the agent.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn receive_rekey(ws_id: &str, peer: &k2_core::postal::Address) -> Result<Value, StoreError> {
    let ws = ws_of_id(ws_id).ok_or(StoreError::NotFound)?;
    let f = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let h = store::handle_for_workspace(&conn, ws_id).ok_or(StoreError::NoAddress)?;
        store::mark_rekey(&conn, &h.id, &peer.as_string())?
    };
    let card = crate::permission::actions::postal::present_pair_accept(&ws, &f);
    Ok(json!({ "friend": f, "card": card }))
}

/// postal-sdk's `HumanGate`, K2's side: each gate records and presents a
/// card, and never decides. The SDK's pairing (over the plane's pair API,
/// P-1..P-3 and P-7) calls it; nothing does in this release. When S3–S5 wire it,
/// follow the four rules in [`k2_core::postal::gate`] (SeenStore = the
/// `msg_deliveries` unique insert in the storing transaction; verify the
/// `p5-msg-v1` hop proof on live delivery; stop the pull loop when
/// `next_after` doesn't advance; check the box's own key before `open`).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct K2Gate;

impl k2_core::postal::gate::HumanGate for K2Gate {
    fn friend_request(&self, req: &k2_core::postal::gate::FriendRequest) {
        let (Some(local), Some(peer)) = (k2_core::postal::parse_address(&req.local_handle), k2_core::postal::parse_address(&req.peer)) else {
            k2_core::log_debug!("[postal] friend request with a bad address: {} → {}", req.peer, req.local_handle);
            return;
        };
        let Some(ws_id) = k2_core::db::read(|conn| store::address_holder(conn, &local.as_string())) else {
            k2_core::log_debug!("[postal] friend request for an address nobody here holds: {}", req.local_handle);
            return;
        };
        // Both boxes get one (the side that asked too): the request row
        // waits on a person either way. The fingerprint is the SDK's own,
        // from the peer's key; the code is this box's, for people only.
        let fingerprint = req.peer_fingerprint.to_hex();
        if let Err(e) = receive_request(
            &ws_id,
            &peer,
            req.peer_owner_name.as_deref(),
            Some(fingerprint.as_str()),
            Some(req.pair_id.as_str()),
            None,
            Some(req.show_code.as_str()),
        ) {
            k2_core::log_debug!("[postal] friend request from {} refused: {}", req.peer, e.code());
        }
    }

    fn rekey(&self, pending: &k2_core::postal::gate::RekeyPending) {
        let (Some(local), Some(peer)) = (k2_core::postal::parse_address(&pending.local_handle), k2_core::postal::parse_address(&pending.peer))
        else {
            return;
        };
        let Some(ws_id) = k2_core::db::read(|conn| store::address_holder(conn, &local.as_string())) else { return };
        if let Err(e) = receive_rekey(&ws_id, &peer) {
            k2_core::log_debug!("[postal] changed key for {} not recorded: {}", pending.peer, e.code());
        }
    }
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
