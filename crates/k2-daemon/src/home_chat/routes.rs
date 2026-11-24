//! `/cli/home-chat/*` (prd-home-people-channels-v1 §14.4; slice S3).
//!
//! Every route takes a person's own login on this server ([`super::people`]):
//! the owner token, agent passports and App sessions get 403. Reads and
//! posts go through the person's own held Sew session, so Sew checks what
//! they may see and do. What mgmt does for them (make, add, remove, rename,
//! archive) K2 checks first against its ledger (HP-19): any channel member
//! adds people, Owner and Admins remove others, the creator or an Owner or
//! Admin renames and deletes, only an Owner or Admin allows agents.
//! Every POST route answers 405 to anything else. Dark (404
//! `home_chat_off`) while the switch is off (the dispatcher answers that).

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};

use k2_core::tool_hook::{self as hook, admin_channels as ledger};

use super::follower::{self, op};
use super::people::{self, Caller};
use super::{deps, now_secs};
use crate::cli_response::CliResponse;
use crate::voice::bff::{sew_refusal, Refusal, SewAnswer};
use crate::voice::routes::refuse;

pub const EVENTS: &str = "/cli/home-chat/events";

/// `GET /cli/home-chat/calls` (0.46.17): the DM calls ringing for the
/// caller now, so a phone woken by a PushKit ring (no webview, no events
/// socket) can answer from CallKit. GET only: a POST answers 405.
pub const CALLS: &str = "/cli/home-chat/calls";

/// `GET /cli/home-chat/recordings?conv=` (0.46.18): a DM's (or a
/// channel's) recordings, newest first, through Sew's client API as the
/// member (Sew 0.11.3 has no mgmt list for DMs: they belong to no Space).
pub const RECORDINGS: &str = "/cli/home-chat/recordings";

/// GET rows.
pub const GET_ROUTES: &[&str] = &[
    CALLS,
    "/cli/home-chat/channels",
    "/cli/home-chat/channels/members",
    "/cli/home-chat/messages",
    "/cli/home-chat/notify",
    "/cli/home-chat/people",
    "/cli/home-chat/push-prefs",
    "/cli/home-chat/reactors",
    RECORDINGS,
    "/cli/home-chat/replies",
    "/cli/home-chat/status",
    "/cli/home-chat/summary",
];

/// POST-only rows (405 on anything else).
pub const POST_ROUTES: &[&str] = &[
    "/cli/home-chat/ack",
    "/cli/home-chat/channels/add",
    "/cli/home-chat/channels/allow-agents",
    "/cli/home-chat/channels/create",
    "/cli/home-chat/channels/delete",
    "/cli/home-chat/channels/leave",
    "/cli/home-chat/channels/remove",
    "/cli/home-chat/channels/rename",
    "/cli/home-chat/dm",
    "/cli/home-chat/notify/set",
    "/cli/home-chat/push-prefs/set",
    "/cli/home-chat/react",
    "/cli/home-chat/send",
    "/cli/home-chat/typing",
    "/cli/home-chat/view",
];

/// Largest request body.
pub const MAX_BODY: usize = 64 * 1024;

pub fn is_route(p: &str) -> bool {
    p == EVENTS || GET_ROUTES.contains(&p) || POST_ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

type R<T> = Result<T, Refusal>;

fn usage(hint: impl Into<String>) -> Refusal {
    Refusal::new(400, "usage", hint)
}

fn internal(e: impl std::fmt::Display) -> Refusal {
    Refusal::new(500, "internal", e.to_string())
}

/// The family is dark while the switch is off.
pub fn dark() -> CliResponse {
    refuse(&Refusal::new(
        404,
        "home_chat_off",
        "people and channels on Home are off on this server (the owner turns them on: k2 settings set homeChat true, or K2_HOME_CHAT=1)",
    ))
}

/// Run one route (blocking). `caller` is the classified request.
pub fn handle(path: &str, caller: R<Caller>, is_post: bool, params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    if is_post_route(path) && !is_post {
        return refuse(&Refusal::new(405, "method_not_allowed", "this route takes POST"));
    }
    if path == CALLS && is_post {
        return refuse(&Refusal::new(405, "method_not_allowed", "this route takes GET"));
    }
    let r = (|| {
        let c = caller?;
        if path == "/cli/home-chat/status" {
            return status_route(&c);
        }
        let (on, reason) = super::status();
        if !on {
            return Err(Refusal::new(503, "chat_unavailable", format!("chat isn't available on this server ({})", reason.unwrap_or("unknown"))));
        }
        route(path, &c, params, body)
    })();
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => refuse(&e),
    }
}

pub(super) fn route(path: &str, c: &Caller, p: &HashMap<String, String>, body: &[u8]) -> R<Value> {
    match path {
        // `me`: the caller's own member id, so a client tells its own
        // messages from others' (Sew names authors by member id).
        "/cli/home-chat/people" => Ok(json!({
            "ok": true,
            "me": {"ext": c.ext, "memberId": people::ready_member(&c.ext), "pictureRev": people::picture_rev(&c.ext)},
            "people": people::list(c).map_err(internal)?,
        })),
        "/cli/home-chat/channels" => channels(c, p),
        "/cli/home-chat/summary" => summary(c, p),
        CALLS => calls(c),
        "/cli/home-chat/messages" => messages(c, p),
        "/cli/home-chat/reactors" => reactors(c, p),
        "/cli/home-chat/replies" => replies(c, p),
        RECORDINGS => recordings(c, p),
        "/cli/home-chat/channels/members" => members(c, p),
        "/cli/home-chat/dm" => dm(c, parse(body)?),
        "/cli/home-chat/send" => send(c, parse(body)?),
        "/cli/home-chat/react" => react(c, parse(body)?),
        "/cli/home-chat/ack" => ack(c, parse(body)?),
        "/cli/home-chat/typing" => typing(c, parse(body)?),
        "/cli/home-chat/view" => view(c, parse(body)?),
        "/cli/home-chat/channels/create" => create(c, parse(body)?),
        "/cli/home-chat/channels/add" => add(c, parse(body)?),
        "/cli/home-chat/channels/remove" => remove(c, parse(body)?),
        "/cli/home-chat/channels/leave" => leave(c, parse(body)?),
        "/cli/home-chat/channels/rename" => rename(c, parse(body)?),
        "/cli/home-chat/channels/delete" => delete(c, parse(body)?),
        "/cli/home-chat/channels/allow-agents" => allow_agents(c, parse(body)?),
        // prd-home-chat-pushes-v1 §7 (HC-14: one path per method).
        "/cli/home-chat/notify" => super::notify::get(c, p),
        "/cli/home-chat/notify/set" => super::notify::set(c, parse(body)?),
        "/cli/home-chat/push-prefs" => super::notify::prefs_get(c),
        "/cli/home-chat/push-prefs/set" => super::notify::prefs_set(c, parse(body)?),
        _ => Err(Refusal::new(404, "not_found", "no such Home chat route")),
    }
}

pub(super) fn parse<T: for<'de> Deserialize<'de>>(body: &[u8]) -> R<T> {
    let b = if body.iter().all(u8::is_ascii_whitespace) { b"{}".as_slice() } else { body };
    serde_json::from_slice(b).map_err(|e| usage(format!("bad request body: {e}")))
}

pub(super) fn param<'a>(p: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    p.get(k).map(|s| s.trim()).filter(|s| !s.is_empty())
}

/// A Sew id from a request (digits only, so it can't steer a path).
pub(super) fn sew_id(raw: &str, what: &str) -> R<String> {
    let t = raw.trim();
    if hook::sew_wire::is_sew_id(t) {
        Ok(t.to_string())
    } else {
        Err(usage(format!("{what} is a Sew id")))
    }
}

fn skin<T>(f: impl FnOnce(&rusqlite::Connection) -> Result<T, String>) -> R<T> {
    k2_core::tool_hook::with_home_chat_db(f).map_err(internal)
}

/// One call to Sew as the person, on their held session; a 401 signs in
/// again once.
fn as_person(c: &Caller, method: &str, path: &str, body: Option<&Value>) -> R<SewAnswer> {
    sew_as(&c.ext, method, path, body)
}

/// One call to Sew as box person `ext` on their held session (DM calls use
/// it too: `voice::dm_call`); a 401 signs in again once.
pub(crate) fn sew_as(ext: &str, method: &str, path: &str, body: Option<&Value>) -> R<SewAnswer> {
    sew_as_keyed(ext, method, path, body, None)
}

/// [`sew_as`], a POST carrying Sew's `Idempotency-Key` when `key` is given
/// (Sew replays the first answer to the same key for 24 h).
fn sew_as_keyed(ext: &str, method: &str, path: &str, body: Option<&Value>, key: Option<&str>) -> R<SewAnswer> {
    let h = follower::hold(ext);
    let d = deps();
    let call = |tok: &str| match (method, body, key) {
        ("GET", _, _) => d.sew.get(Some(tok), path),
        (_, b, Some(k)) => d.sew.post_keyed(Some(tok), path, b.unwrap_or(&json!({})), k),
        (_, b, None) => d.sew.post(Some(tok), path, b.unwrap_or(&json!({}))),
    };
    let tok = h.token()?;
    let a = call(&tok).map_err(super::sessions::unavailable)?;
    if a.status != 401 {
        return Ok(a);
    }
    h.forget_session();
    let tok = h.token()?;
    call(&tok).map_err(super::sessions::unavailable)
}

pub(super) fn ok_or_refuse(a: SewAnswer) -> R<Value> {
    if a.ok() {
        return Ok(a.body);
    }
    // Sew's `422 invalid_state {reason: channel_archived}` is the brief's
    // `409 channel_archived` (a post, a reaction, a read in an archived
    // channel), the same answer K2's own ledger check gives.
    let said = |p: &str| a.body.pointer(p).and_then(Value::as_str) == Some("channel_archived");
    if said("/error/details/reason") || said("/error/code") {
        return Err(archived());
    }
    Err(sew_refusal(&a))
}

fn flags() -> k2_core::tool_hook::features::HomeChatFeatures {
    (deps().flags)()
}

// ── reads ────────────────────────────────────────────────────────────

fn status_route(c: &Caller) -> R<Value> {
    let (on, reason) = super::status();
    let chat = follower::get(&c.ext).map(|h| h.status());
    Ok(json!({"ok": true, "on": on, "reason": reason, "flags": flags(), "chat": chat}))
}

fn k5_space() -> R<String> {
    hook::admin_space_id().map_err(internal)?.ok_or_else(|| Refusal::new(503, "chat_unavailable", "the K5 Space isn't ready yet"))
}

/// The caller's visible K5 text channels (browse: joined or not, HP-27),
/// with what K2's ledger knows of each.
fn channels(c: &Caller, p: &HashMap<String, String>) -> R<Value> {
    let space = k5_space()?;
    let mut path = format!("/api/v1/spaces/{space}/channels?scope=browse&limit=100");
    if let Some(after) = param(p, "after") {
        path.push_str(&format!("&after={}", sew_id(after, "after")?));
    }
    if let Some(q) = param(p, "q") {
        path.push_str(&format!("&q={}", hook::sew_wire::pct_path(q)));
    }
    let body = ok_or_refuse(as_person(c, "GET", &path, None)?)?;
    let list = body.get("items").or_else(|| body.get("channels")).unwrap_or(&body);
    let mut out = Vec::new();
    for ch in list.as_array().into_iter().flatten() {
        let kind = ch["type"].as_str().or_else(|| ch["kind"].as_str()).unwrap_or("text");
        if kind != "text" {
            continue; // voice rooms aren't Home channels
        }
        let id = match &ch["id"] {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => continue,
        };
        let row = skin(|conn| ledger::channel_by_id(conn, &id))?;
        out.push(json!({
            "conv": id, "name": ch["name"], "open": ch["visibility"] == "public",
            "inviteOnly": row.as_ref().map(|r| !r.open).or_else(|| super::state::invite_only_of(&ch["visibility"])),
            "archived": ch["archived"].as_bool().unwrap_or(false) || row.as_ref().is_some_and(|r| r.state == ledger::CH_ARCHIVED),
            "home": row.is_some(), "allowAgents": row.as_ref().is_some_and(|r| r.allow_agents),
        }));
    }
    Ok(json!({"ok": true, "channels": out, "next": body.get("next").cloned().unwrap_or(Value::Null)}))
}

/// [`CALLS`]: the caller's own ringing DM calls, read from their follower's
/// state (the same state `hello`'s `calls` shows; only the ones ringing for
/// them, `[]` when none). `chat` says whether to trust an empty list: right
/// after a restart the follower is `connecting` and knows no ring yet.
fn calls(c: &Caller) -> R<Value> {
    let h = follower::hold(&c.ext);
    let calls = h.state.lock().unwrap_or_else(|e| e.into_inner()).ringing_view();
    let chat = match h.status() {
        "live" => "live",
        "connecting" => "connecting",
        _ => "unavailable",
    };
    Ok(json!({"ok": true, "chat": chat, "calls": calls}))
}

fn summary(c: &Caller, p: &HashMap<String, String>) -> R<Value> {
    let convs: Vec<String> = param(p, "convs").map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
    let h = follower::hold(&c.ext);
    let flag = flags().read_state_unread_count;
    settle_kinds(c, &h, &convs)?;
    let mut rows = h.state.lock().unwrap_or_else(|e| e.into_inner()).summary(&convs, flag);
    // A DM names its other person when K2 kept it (`with`), so a person row
    // on Home finds its badge without opening the DM first.
    for r in rows.iter_mut().filter(|r| r["kind"] == "dm") {
        let conv = r["conv"].as_str().unwrap_or_default().to_string();
        if let Some(other) = skin(|conn| ledger::dm_other(conn, &c.ext, &conv))? {
            r["with"] = json!(other);
        }
    }
    // A Home channel's kind is K2's ledger's (made here): invite-only or open.
    for r in rows.iter_mut().filter(|r| r["kind"] == "channel") {
        let conv = r["conv"].as_str().unwrap_or_default().to_string();
        if let Some(ch) = skin(|conn| ledger::channel_by_id(conn, &conv))? {
            r["inviteOnly"] = json!(!ch.open);
        }
    }
    let chat = match h.status() {
        "live" => "live",
        "connecting" => "connecting",
        _ => "unavailable",
    };
    Ok(json!({"ok": true, "chat": chat, "convs": rows}))
}

/// A DM is a DM in the summary whatever Sew event made its row first: a
/// read state can come before the message (Sew moves the poster's cursor
/// first) and says nothing of the kind. A row that reads as a channel with
/// no Space is checked against K2's DM ledger (also mending a row stored
/// wrong before); one no event has named yet is read from Sew once.
fn settle_kinds(c: &Caller, h: &follower::Held, convs: &[String]) -> R<()> {
    let unsure = h.state.lock().unwrap_or_else(|e| e.into_inner()).kind_unsure(convs);
    for conv in unsure {
        if skin(|conn| ledger::dm_other(conn, &c.ext, &conv))?.is_some() {
            h.state.lock().unwrap_or_else(|e| e.into_inner()).note_dm(&conv);
        }
    }
    let unknown = h.state.lock().unwrap_or_else(|e| e.into_inner()).kind_unknown(convs);
    for conv in unknown {
        if !hook::sew_wire::is_sew_id(&conv) {
            continue;
        }
        // Best effort: unreachable now, asked again on the next summary.
        if let Ok(a) = as_person(c, "GET", &format!("/api/v1/channels/{conv}"), None) {
            let ch = if a.ok() { a.body.get("channel").cloned().unwrap_or(a.body) } else { Value::Null };
            h.state.lock().unwrap_or_else(|e| e.into_inner()).apply_channel(&conv, &ch);
        }
    }
    Ok(())
}

/// A page of messages (newest last), with Sew's `reactions` and
/// `reference` as they are. `since` (a channel seq) reads the channel's
/// changes feed instead; `410 resync_required` answers the newest page with
/// `resync: true` (§14.3).
fn messages(c: &Caller, p: &HashMap<String, String>) -> R<Value> {
    let conv = sew_id(param(p, "conv").ok_or_else(|| usage("name the conversation: conv"))?, "conv")?;
    let limit: u32 = match param(p, "limit") {
        Some(l) => l.parse().ok().filter(|n| (1..=100).contains(n)).ok_or_else(|| usage("limit is 1-100"))?,
        None => 50,
    };
    if let Some(since) = param(p, "since") {
        let since: u64 = since.parse().map_err(|_| usage("since is a channel seq"))?;
        let a = as_person(c, "GET", &format!("/api/v1/channels/{conv}/changes?since={since}&limit={limit}"), None)?;
        if a.status != 410 {
            return Ok(json!({"ok": true, "conv": conv, "changes": ok_or_refuse(a)?}));
        }
        let page = ok_or_refuse(as_person(c, "GET", &format!("/api/v1/channels/{conv}/messages?limit={limit}"), None)?)?;
        return Ok(json!({"ok": true, "conv": conv, "resync": true, "page": page}));
    }
    let mut path = format!("/api/v1/channels/{conv}/messages?limit={limit}");
    if let Some(b) = param(p, "before") {
        path.push_str(&format!("&before={}", sew_id(b, "before")?));
    }
    if let Some(a) = param(p, "after") {
        path.push_str(&format!("&after={}", sew_id(a, "after")?));
    }
    let a = as_person(c, "GET", &path, None)?;
    if a.status == 404 {
        if let Some(gone) = dm_gone(c, &conv)? {
            return Err(gone);
        }
    }
    let page = ok_or_refuse(a)?;
    Ok(json!({"ok": true, "conv": conv, "page": page}))
}

fn reactors(c: &Caller, p: &HashMap<String, String>) -> R<Value> {
    let msg = sew_id(param(p, "msg").ok_or_else(|| usage("name the message: msg"))?, "msg")?;
    let mut path = format!("/api/v1/messages/{msg}/reactors");
    if let Some(e) = param(p, "emoji") {
        path.push_str(&format!("?emoji={}", hook::sew_wire::pct_path(e)));
    }
    Ok(json!({"ok": true, "reactors": ok_or_refuse(as_person(c, "GET", &path, None)?)?}))
}

/// `GET /cli/home-chat/replies?conv=&id=[&after=][&limit=]`: the replies
/// under one message, oldest first, through the person's own session
/// (prd-home-call-recording-v1 §5, HR-10): a recording card's transcript
/// lines, for Show transcript. Sew answers only what this person may read
/// (in a DM, the two people); a line of another conversation is dropped.
fn replies(c: &Caller, p: &HashMap<String, String>) -> R<Value> {
    let conv = sew_id(param(p, "conv").ok_or_else(|| usage("name the conversation: conv"))?, "conv")?;
    let id = sew_id(param(p, "id").ok_or_else(|| usage("name the message: id (the recording card)"))?, "id")?;
    let limit: u32 = match param(p, "limit") {
        Some(l) => l.parse().ok().filter(|n| (1..=100).contains(n)).ok_or_else(|| usage("limit is 1-100"))?,
        None => 100,
    };
    let mut path = format!("/api/v1/messages/{id}/replies?limit={limit}");
    if let Some(a) = param(p, "after") {
        path.push_str(&format!("&after={}", sew_id(a, "after")?));
    }
    let a = as_person(c, "GET", &path, None)?;
    if a.status == 404 {
        if let Some(gone) = dm_gone(c, &conv)? {
            return Err(gone);
        }
    }
    let mut page = ok_or_refuse(a)?;
    let keep = |m: &Value| match &m["channel_id"] {
        Value::String(s) => *s == conv,
        Value::Number(n) => n.to_string() == conv,
        _ => false,
    };
    if let Some(items) = page.get_mut("items").and_then(Value::as_array_mut) {
        items.retain(keep);
    } else if let Some(items) = page.as_array_mut() {
        items.retain(keep);
    }
    Ok(json!({"ok": true, "conv": conv, "id": id, "page": page}))
}

/// `GET /cli/home-chat/recordings?conv=[&before=][&limit=]`: the
/// conversation's recordings, newest first (Sew's `Page<Recording>`:
/// `items`, `has_more`), through the person's own held session, so Sew
/// answers only a member (in a DM, the two people). This is Sew 0.11.3's
/// client list, `GET /api/v1/channels/{id}/recordings`; Sew's mgmt has no
/// list for DMs (fetch by id only), so K2 builds none. A recording of
/// another conversation is dropped. No transcript text rides here: the
/// lines are the card's replies (`/cli/home-chat/replies`).
fn recordings(c: &Caller, p: &HashMap<String, String>) -> R<Value> {
    let conv = sew_id(param(p, "conv").ok_or_else(|| usage("name the conversation: conv"))?, "conv")?;
    let limit: u32 = match param(p, "limit") {
        Some(l) => l.parse().ok().filter(|n| (1..=100).contains(n)).ok_or_else(|| usage("limit is 1-100"))?,
        None => 50,
    };
    let mut path = format!("/api/v1/channels/{conv}/recordings?limit={limit}");
    if let Some(b) = param(p, "before") {
        path.push_str(&format!("&before={}", sew_id(b, "before")?));
    }
    let a = as_person(c, "GET", &path, None)?;
    if a.status == 404 {
        if let Some(gone) = dm_gone(c, &conv)? {
            return Err(gone);
        }
    }
    let mut page = ok_or_refuse(a)?;
    let keep = |r: &Value| match &r["channel_id"] {
        Value::String(s) => *s == conv,
        Value::Number(n) => n.to_string() == conv,
        _ => false,
    };
    if let Some(items) = page.get_mut("items").and_then(Value::as_array_mut) {
        items.retain(keep);
    } else if let Some(items) = page.as_array_mut() {
        items.retain(keep);
    }
    Ok(json!({"ok": true, "conv": conv, "page": page}))
}

/// The ledger row of a Home channel by its Sew id.
fn home_channel(conv: &str) -> R<ledger::ChannelRow> {
    skin(|conn| ledger::channel_by_id(conn, conv))?.ok_or_else(|| Refusal::new(404, "channel_not_found", "no Home channel with that id on this server"))
}

/// May the caller see this channel (a member, or it is open)?
fn sees(c: &Caller, ch: &ledger::ChannelRow) -> R<bool> {
    Ok(ch.open || skin(|conn| ledger::is_member(conn, &ch.slug, &c.ext))?)
}

/// The channel a members read names: `conv` (its Sew id), or `slug` (K2's,
/// from `channels/create`, so the creator can wait for a pending channel's id).
pub(super) fn named_channel(p: &HashMap<String, String>) -> R<ledger::ChannelRow> {
    if let Some(slug) = param(p, "slug") {
        if !k2_core::home_chat::is_home_slug(slug) {
            return Err(usage("slug is a Home channel's slug (c-…)"));
        }
        return skin(|conn| ledger::channel_by_slug(conn, slug))?
            .ok_or_else(|| Refusal::new(404, "channel_not_found", "no Home channel with that id on this server"));
    }
    home_channel(&sew_id(param(p, "conv").ok_or_else(|| usage("name the channel: conv or slug"))?, "conv")?)
}

fn members(c: &Caller, p: &HashMap<String, String>) -> R<Value> {
    let ch = named_channel(p)?;
    if !sees(c, &ch)? {
        return Err(Refusal::new(404, "channel_not_found", "no Home channel with that id on this server"));
    }
    let list = skin(|conn| ledger::members_of(conn, &ch.slug))?;
    Ok(json!({
        "ok": true, "conv": ch.channel_id, "slug": ch.slug, "state": ch.state, "name": ch.desired_name(), "open": ch.open,
        "inviteOnly": !ch.open,
        "creator": ch.creator_ext, "allowAgents": ch.allow_agents,
        "people": list.iter().filter(|m| m.state != ledger::M_PENDING_REMOVE).map(|m| json!({"ext": m.external_id, "state": m.state})).collect::<Vec<_>>(),
        // Agent grants are a later slice (S8): none yet.
        "agents": [],
    }))
}

// ── DMs, posts, reactions, read state ────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DmBody {
    ext: String,
}

/// Open (or make) the DM with `ext`, keyed on Sew member ids (HP-29); the
/// channel id is kept per (caller, target).
fn dm(c: &Caller, b: DmBody) -> R<Value> {
    let target = b.ext.trim().to_string();
    if target == c.ext {
        return Err(usage("a DM is with someone else"));
    }
    people::check_target(&target)?;
    if let Some(conv) = skin(|conn| ledger::dm_channel(conn, &c.ext, &target))? {
        // The kept id is Sew's only while Sew still has it: a rollback or a
        // restore of Sew's data can take it away. Sew answers for it first;
        // a DM it no longer has (404, or 403 once it isn't the caller's) is
        // forgotten and made again below, never handed back.
        let a = as_person(c, "GET", &format!("/api/v1/channels/{}", sew_id(&conv, "the kept DM")?), None)?;
        match a.status {
            s if (200..300).contains(&s) => return Ok(json!({"ok": true, "conv": conv, "created": false})),
            403 | 404 => {
                k2_core::log_debug!("[home-chat] {}: kept DM {conv} with {target} is gone from Sew ({}); making it again", c.ext, a.status);
                skin(|conn| ledger::forget_dm_channel(conn, &conv))?;
            }
            _ => return Err(sew_refusal(&a)),
        }
    }
    let member = people::member_id(&target)?;
    let body = ok_or_refuse(as_person(c, "POST", "/api/v1/dms", Some(&json!({"member_ids": [member]})))?)?;
    let conv = match &body["id"] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Err(super::sessions::unavailable("Sew's DM answer had no channel id")),
    };
    skin(|conn| ledger::set_dm_channel(conn, &c.ext, &target, &conv, now_secs()))?;
    Ok(json!({"ok": true, "conv": conv, "created": true}))
}

/// A DM whose other side was disabled is read-only (HP-13).
fn check_dm_writable(c: &Caller, conv: &str) -> R<()> {
    if let Some(target) = skin(|conn| ledger::dm_target(conn, &c.ext, conv))? {
        people::check_target(&target)?;
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SendBody {
    conv: String,
    text: String,
    nonce: String,
    #[serde(default)]
    reply_to: Option<String>,
}

fn send(c: &Caller, b: SendBody) -> R<Value> {
    let conv = sew_id(&b.conv, "conv")?;
    if b.text.trim().is_empty() {
        return Err(usage("text is empty"));
    }
    if b.nonce.is_empty() || b.nonce.len() > 64 {
        return Err(usage("nonce is 1-64 characters (a retry sends the same one)"));
    }
    check_dm_writable(c, &conv)?;
    check_channel_writable(&conv)?;
    let mut body = json!({"body": b.text, "nonce": b.nonce});
    if let Some(r) = b.reply_to.as_deref() {
        // D8: an inline reply reference, never a Sew sub-thread.
        body["reference"] = json!({"message_id": sew_id(r, "replyTo")?, "kind": "reply"});
    }
    let key = sends::Key { ext: c.ext.clone(), conv: conv.clone(), nonce: b.nonce.clone() };
    let idem = key.idempotency_key();
    let sent = sends::once(key, || {
        let a = sew_as_keyed(&c.ext, "POST", &format!("/api/v1/channels/{conv}/messages"), Some(&body), Some(&idem))?;
        if a.status == 404 {
            if let Some(gone) = dm_gone(c, &conv)? {
                return Err(gone);
            }
        }
        let m = ok_or_refuse(a)?;
        Ok(json!({"ok": true, "message": m}))
    });
    if sent.is_ok() {
        // homes-people-group-v1: a DM someone sends arrives in the other
        // person's People group (once; their own placement wins).
        super::arrivals::dm_sent(&c.ext, &conv);
    }
    sent
}

/// Sew answered 404 for `conv`: when it is a DM of the caller's that K2
/// kept (forgotten now, so opening the DM makes it again) or already forgot
/// after a reset of Sew's data, the answer is 404 `conversation_gone`, not
/// Sew's bare `not_found`.
fn dm_gone(c: &Caller, conv: &str) -> R<Option<Refusal>> {
    let kept = skin(|conn| ledger::dm_target(conn, &c.ext, conv))?.is_some();
    if kept {
        skin(|conn| ledger::forget_dm_channel(conn, conv))?;
    }
    if kept || skin(|conn| ledger::dm_gone(conn, &c.ext, conv))? {
        return Ok(Some(Refusal::new(
            404,
            "conversation_gone",
            "this DM is gone from chat on this server: open it again (POST /cli/home-chat/dm) and resend",
        )));
    }
    Ok(None)
}

/// A Home channel K2's ledger holds as archived (or on its way there) takes
/// no post: 409 `channel_archived`, before Sew is asked.
fn check_channel_writable(conv: &str) -> R<()> {
    if let Some(ch) = skin(|conn| ledger::channel_by_id(conn, conv))? {
        if ch.state == ledger::CH_ARCHIVED || ch.want_archive {
            return Err(archived());
        }
    }
    Ok(())
}

fn archived() -> Refusal {
    Refusal::new(409, "channel_archived", "that channel is archived: it is read-only")
}

/// One post per (sender, conversation, nonce) (§14.4 `send`: "a retry sends
/// the same nonce"). A retry inside [`sends::KEEP_SECS`] (Sew's own
/// idempotency window, 24 h) gets the first post's answer back and posts
/// nothing; one that arrives while the first is still with Sew waits for
/// it. Sew gets the same `Idempotency-Key` for each, so a retry after a
/// daemon restart (this memory is gone) is still one message: Sew replays.
pub(super) mod sends {
    use std::collections::HashMap;
    use std::sync::{Condvar, Mutex, OnceLock};
    use std::time::Duration;

    use serde_json::Value;
    use sha2::{Digest, Sha256};

    use super::R;

    /// Sew keeps an `Idempotency-Key` this long (`IDEMPOTENCY_TTL_MS`).
    pub const KEEP_SECS: i64 = 24 * 3600;
    /// Most answers kept (the oldest go first).
    pub const KEEP_MAX: usize = 4096;
    /// How long a retry waits for the first send still with Sew (Sew's
    /// client timeout is 15 s).
    const WAIT: Duration = Duration::from_secs(20);

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub struct Key {
        pub ext: String,
        pub conv: String,
        pub nonce: String,
    }

    impl Key {
        /// Sew's `Idempotency-Key` (`[A-Za-z0-9_-]{1,64}`): the three
        /// hashed, so any nonce fits and a person's nonce never names another's post.
        pub fn idempotency_key(&self) -> String {
            let mut h = Sha256::new();
            for part in [&self.ext, &self.conv, &self.nonce] {
                h.update(part.as_bytes());
                h.update([0u8]);
            }
            let hex: String = h.finalize().iter().take(20).map(|b| format!("{b:02x}")).collect();
            format!("k2hc-send-{hex}")
        }
    }

    enum Slot {
        InFlight,
        Done { at: i64, answer: Value },
    }

    struct Book {
        map: Mutex<HashMap<Key, Slot>>,
        moved: Condvar,
    }

    fn book() -> &'static Book {
        static B: OnceLock<Book> = OnceLock::new();
        B.get_or_init(|| Book { map: Mutex::new(HashMap::new()), moved: Condvar::new() })
    }

    /// Run `post` once for `key`; a retry gets its answer. A refusal is not
    /// kept: the next try asks Sew again (with the same key).
    pub fn once(key: Key, post: impl FnOnce() -> R<Value>) -> R<Value> {
        let b = book();
        let mut m = b.map.lock().unwrap_or_else(|p| p.into_inner());
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            match m.get(&key) {
                Some(Slot::Done { at, answer }) if *at > super::now_secs() - KEEP_SECS => return Ok(answer.clone()),
                Some(Slot::InFlight) => {
                    let left = deadline.saturating_duration_since(std::time::Instant::now());
                    if left.is_zero() {
                        return Err(crate::voice::bff::Refusal::new(
                            409,
                            "send_in_progress",
                            "the first send with this nonce is still on its way; try again in a moment",
                        ));
                    }
                    m = b.moved.wait_timeout(m, left).unwrap_or_else(|p| p.into_inner()).0;
                }
                _ => break,
            }
        }
        m.insert(key.clone(), Slot::InFlight);
        drop(m);
        // A post that panics never leaves its retries waiting.
        struct Unstick<'a>(&'a Book, Option<Key>);
        impl Drop for Unstick<'_> {
            fn drop(&mut self) {
                if let Some(k) = self.1.take() {
                    self.0.map.lock().unwrap_or_else(|p| p.into_inner()).remove(&k);
                    self.0.moved.notify_all();
                }
            }
        }
        let mut unstick = Unstick(b, Some(key.clone()));
        let r = post();
        unstick.1 = None;
        let mut m = b.map.lock().unwrap_or_else(|p| p.into_inner());
        match &r {
            Ok(answer) => {
                let now = super::now_secs();
                m.retain(|_, s| !matches!(s, Slot::Done { at, .. } if *at <= now - KEEP_SECS));
                if m.len() >= KEEP_MAX {
                    let oldest = m
                        .iter()
                        .filter_map(|(k, s)| match s {
                            Slot::Done { at, .. } => Some((*at, k.clone())),
                            Slot::InFlight => None,
                        })
                        .min_by(|a, b| a.0.cmp(&b.0))
                        .map(|(_, k)| k);
                    if let Some(k) = oldest {
                        m.remove(&k);
                    }
                }
                m.insert(key, Slot::Done { at: now, answer: answer.clone() });
            }
            Err(_) => {
                m.remove(&key);
            }
        }
        drop(m);
        b.moved.notify_all();
        r
    }

    /// Tests: forget every kept answer (a daemon restart).
    #[cfg(test)]
    pub fn forget_for_tests() {
        book().map.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReactBody {
    conv: String,
    msg: String,
    emoji: String,
    on: bool,
}

fn react(c: &Caller, b: ReactBody) -> R<Value> {
    let conv = sew_id(&b.conv, "conv")?;
    let msg = sew_id(&b.msg, "msg")?;
    let e = b.emoji.trim();
    if e.is_empty() || e.chars().count() > 16 || e.chars().any(char::is_control) {
        return Err(usage("emoji is one Unicode emoji"));
    }
    check_dm_writable(c, &conv)?;
    let verb = if b.on { "add" } else { "remove" };
    let m = ok_or_refuse(as_person(c, "POST", &format!("/api/v1/messages/{msg}/reactions/{verb}"), Some(&json!({"emoji": {"kind": "unicode", "value": e}})))?)?;
    Ok(json!({"ok": true, "message": m}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConvBody {
    conv: String,
}

fn ack(c: &Caller, b: ConvBody) -> R<Value> {
    let conv = sew_id(&b.conv, "conv")?;
    ok_or_refuse(as_person(c, "POST", &format!("/api/v1/channels/{conv}/ack"), Some(&json!({})))?)?;
    Ok(json!({"ok": true}))
}

fn typing(c: &Caller, b: ConvBody) -> R<Value> {
    let conv = sew_id(&b.conv, "conv")?;
    if flags().http_typing {
        ok_or_refuse(as_person(c, "POST", &format!("/api/v1/channels/{conv}/typing"), Some(&json!({})))?)?;
        return Ok(json!({"ok": true, "via": "http"}));
    }
    let sent = follower::hold(&c.ext).send_gateway(op::TYPING, json!({"channel_id": conv}));
    Ok(json!({"ok": true, "via": "gateway", "sent": sent}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewBody {
    conv: String,
    open: bool,
}

/// A client opened or closed a conversation: typing is subscribed for the
/// open ones only (≤ 100 per scope); opening an open channel the person
/// hasn't joined joins it (HP-27).
fn view(c: &Caller, b: ViewBody) -> R<Value> {
    let conv = sew_id(&b.conv, "conv")?;
    let mut joined = false;
    if b.open {
        if let Some(ch) = skin(|conn| ledger::channel_by_id(conn, &conv))? {
            if ch.open && ch.state == ledger::CH_ACTIVE && !skin(|conn| ledger::is_member(conn, &ch.slug, &c.ext))? {
                ok_or_refuse(as_person(c, "POST", &format!("/api/v1/channels/{conv}/join"), Some(&json!({})))?)?;
                skin(|conn| ledger::note_joined(conn, &ch.slug, &c.ext, now_secs()))?;
                joined = true;
            }
        }
    }
    let h = follower::hold(&c.ext);
    let scopes: HashMap<String, Vec<String>> = {
        let mut st = h.state.lock().unwrap_or_else(|e| e.into_inner());
        if b.open {
            st.viewing.insert(conv.clone());
        } else {
            st.viewing.remove(&conv);
        }
        let mut by: HashMap<String, Vec<String>> = HashMap::new();
        for v in &st.viewing {
            let scope = st.convs.get(v).and_then(|x| x.space_id.clone()).unwrap_or_else(|| "dms".to_string());
            by.entry(scope).or_default().push(v.clone());
        }
        by
    };
    for (scope, mut ids) in scopes {
        ids.sort();
        ids.truncate(100);
        h.send_gateway(op::SUBSCRIBE, json!({"space_id": scope, "typing": ids}));
    }
    Ok(json!({"ok": true, "joined": joined}))
}

// ── channels over mgmt (K2 checks the caller first, HP-19) ───────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    name: String,
    #[serde(default)]
    open: bool,
    #[serde(default)]
    members: Vec<String>,
}

fn create(c: &Caller, b: CreateBody) -> R<Value> {
    k2_core::home_chat::valid_channel_name(&b.name).map_err(usage)?;
    if b.members.len() >= hook::sew_wire::CHANNEL_MEMBERS_MAX {
        return Err(usage(format!("a new channel takes at most {} people; add the rest after", hook::sew_wire::CHANNEL_MEMBERS_MAX)));
    }
    let members: Vec<String> = b.members.iter().map(|m| m.trim().to_string()).collect();
    for m in &members {
        people::check_target(m)?;
    }
    let row = skin(|conn| ledger::request_create(conn, &b.name, b.open, &c.ext, &members, now_secs()))?;
    hook::kick_reconcile();
    Ok(json!({"ok": true, "slug": row.slug, "conv": row.channel_id, "state": row.state, "name": row.name, "open": row.open}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddBody {
    conv: String,
    ext: Vec<String>,
}

fn live_channel(conv: &str) -> R<ledger::ChannelRow> {
    let ch = home_channel(&sew_id(conv, "conv")?)?;
    if ch.state != ledger::CH_ACTIVE || ch.want_archive {
        return Err(Refusal::new(409, "channel_archived", "that channel is archived: it is read-only"));
    }
    Ok(ch)
}

fn add(c: &Caller, b: AddBody) -> R<Value> {
    let ch = live_channel(&b.conv)?;
    if !skin(|conn| ledger::is_member(conn, &ch.slug, &c.ext))? {
        return Err(Refusal::new(403, "not_a_member", "only people in a channel add others to it"));
    }
    let mut out = Vec::new();
    let mut added = Vec::new();
    for e in &b.ext {
        let e = e.trim();
        let outcome = match people::check_target(e) {
            Ok(()) => skin(|conn| ledger::request_add(conn, &ch.slug, e, now_secs()))?.to_string(),
            Err(r) => format!("refused: {}", r.code),
        };
        if outcome == "added" {
            added.push(e.to_string());
        }
        out.push(json!({"ext": e, "outcome": outcome}));
    }
    hook::kick_reconcile();
    // homes-people-group-v1: the channel arrives in each added person's People group.
    if let Some(id) = ch.channel_id.as_deref() {
        super::arrivals::channel_added(&c.ext, id, ch.desired_name(), &added);
    }
    Ok(json!({"ok": true, "results": out}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveBody {
    conv: String,
    ext: String,
}

fn remove(c: &Caller, b: RemoveBody) -> R<Value> {
    let ch = live_channel(&b.conv)?;
    let target = b.ext.trim();
    if target != c.ext && !c.is_admin() {
        return Err(Refusal::new(403, "admin_required", "only the server's Owner or an Admin removes someone else; anyone may leave"));
    }
    if ch.open {
        return Err(Refusal::new(409, "channel_open", "an open channel is open to everyone on this server: no one is removed from it"));
    }
    let was = skin(|conn| ledger::request_remove(conn, &ch.slug, target, now_secs()))?;
    hook::kick_reconcile();
    Ok(json!({"ok": true, "removed": was}))
}

fn leave(c: &Caller, b: ConvBody) -> R<Value> {
    let ch = home_channel(&sew_id(&b.conv, "conv")?)?;
    let conv = ch.channel_id.clone().unwrap_or_default();
    if ch.open {
        ok_or_refuse(as_person(c, "POST", &format!("/api/v1/channels/{conv}/leave"), Some(&json!({})))?)?;
        skin(|conn| ledger::note_left(conn, &ch.slug, &c.ext))?;
        return Ok(json!({"ok": true, "left": true}));
    }
    // K2 governs the Space: K2's own Leave removes over mgmt (HP-19).
    let was = skin(|conn| ledger::request_remove(conn, &ch.slug, &c.ext, now_secs()))?;
    hook::kick_reconcile();
    Ok(json!({"ok": true, "left": was}))
}

/// The creator, or the server's Owner or an Admin who can see the channel.
fn may_change(c: &Caller, ch: &ledger::ChannelRow) -> R<()> {
    if ch.creator_ext == c.ext || (c.is_admin() && sees(c, ch)?) {
        return Ok(());
    }
    Err(Refusal::new(403, "not_creator", "only the channel's creator, or the server's Owner or an Admin, renames or deletes it"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameBody {
    conv: String,
    name: String,
}

fn rename(c: &Caller, b: RenameBody) -> R<Value> {
    let ch = live_channel(&b.conv)?;
    may_change(c, &ch)?;
    k2_core::home_chat::valid_channel_name(&b.name).map_err(usage)?;
    skin(|conn| ledger::request_rename(conn, &ch.slug, &b.name, now_secs()))?;
    hook::kick_reconcile();
    Ok(json!({"ok": true}))
}

fn delete(c: &Caller, b: ConvBody) -> R<Value> {
    let ch = live_channel(&b.conv)?;
    may_change(c, &ch)?;
    skin(|conn| ledger::request_archive(conn, &ch.slug, now_secs()))?;
    hook::kick_reconcile();
    Ok(json!({"ok": true, "archived": true}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AllowBody {
    conv: String,
    on: bool,
}

fn allow_agents(c: &Caller, b: AllowBody) -> R<Value> {
    let ch = live_channel(&b.conv)?;
    if !c.is_admin() || !sees(c, &ch)? {
        return Err(Refusal::new(403, "admin_required", "only the server's Owner or an Admin allows agents in a channel"));
    }
    skin(|conn| ledger::set_allow_agents(conn, &ch.slug, b.on, now_secs()))?;
    Ok(json!({"ok": true, "allowAgents": b.on}))
}
