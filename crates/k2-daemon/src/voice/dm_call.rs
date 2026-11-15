//! 1-to-1 calls in a DM (prd-home-people-channels-v1 §11a; slice S11;
//! HP-38 to HP-40, HP-42). Sew has the server side since 0.9.2; this is
//! K2's door to it.
//!
//! ```text
//! POST /cli/voice/join {dm, join?, callAnswer?}   start (rings) or answer → callId + the join answer
//! POST /cli/voice/call/decline {dm, callId}        decline a call that rings for you
//! POST /cli/voice/call/ring {dm, callId, memberIds?}  ring again (Sew allows 3 per person)
//! POST /cli/voice/call/cancel {dm, callId}         the caller's Cancel while it rings
//! POST /cli/voice/call/leave {callId?}             leave (today's leave)
//! ```
//!
//! **Who.** A person's own login on this server (a desktop login or a
//! paired phone), by join's rule (`routes::person_of`): the owner token,
//! passports (agents), App passes and API keys get join's refusals. Agents
//! never start or join a DM call; Sew refuses act-as on `call/join` too.
//! Voice & Chat only logins call like any person (Sew sees a person; DMs
//! ignore Space roles).
//!
//! **Whose session.** Each Sew call goes on the person's held Home chat
//! session (`home_chat::routes::sew_as`). The call K2 records for the join
//! is this login session's, so `session/leave` and the call's own routes
//! find it as they find a room's.
//!
//! **Cancel (Sew 0.10.0 A).** Only when Sew reports `call_cancel`. Without
//! it the caller's Cancel is a leave (`POST /api/v1/me/voice/leave`), and the
//! callee sees a missed call.
//!
//! **The join answer (HP-40).** Sew answers `CallJoinResponse {call,
//! created, voice, recording}`. A client whose call client takes that as is
//! (Sew 0.10.0 B) says so with `callAnswer: true` and gets it unchanged;
//! otherwise K2 unwraps `.voice` to the top level (the shape the call client
//! takes for a room) and puts `call` beside it. The room is the DM's
//! channel id.
//!
//! **Gates.** Home chat usable here; the Pro plan, like voice rooms, while
//! [`DM_CALLS_NEED_PRO`] says so; the voice lane's join hold, as rooms have.

use std::sync::Arc;

use serde_json::{json, Value};

use k2_core::voice_rooms::Hosting as Tier;

use super::bff::{self, Person, Refusal};

/// DM calls ride the voice rooms' Pro gate (`host_not_pro`) while this is
/// true; false makes them free on every Linux server.
pub const DM_CALLS_NEED_PRO: bool = true;

/// What the DM call routes decide from, besides the request.
#[derive(Clone)]
pub struct Ctx {
    /// `home_chat::status()`: usable here, or why not.
    pub chat: (bool, Option<&'static str>),
    /// The plan tier (`voice::pro_tier`).
    pub tier: Tier,
    /// Sew reports `call_cancel`.
    pub call_cancel: bool,
    /// The voice BFF's deps (the lane hold; the leave's login session).
    pub bff: Arc<bff::Deps>,
}

impl Ctx {
    pub fn live() -> Ctx {
        Ctx {
            chat: crate::home_chat::status(),
            tier: super::pro_tier().0,
            call_cancel: (crate::home_chat::deps().flags)().call_cancel,
            bff: bff::deps(),
        }
    }
}

type R<T> = Result<T, Refusal>;

fn usage(hint: &str) -> Refusal {
    Refusal::new(400, "usage", hint)
}

/// A Sew id from the body: 1–19 digits (it can't steer a path).
fn id_field(b: &Value, k: &str, what: &str) -> R<String> {
    let raw = match b.get(k) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    if super::kit_door::dec(&raw) {
        Ok(raw)
    } else {
        Err(usage(what))
    }
}

/// Home chat usable, then the Pro gate.
fn gate(ctx: &Ctx) -> R<()> {
    if !ctx.chat.0 {
        return Err(Refusal::new(
            503,
            "chat_unavailable",
            format!("calls aren't available on this server ({})", ctx.chat.1.unwrap_or("unknown")),
        ));
    }
    if DM_CALLS_NEED_PRO && ctx.tier != Tier::Ok {
        return Err(Refusal::new(403, "host_not_pro", "this server's calls are paused (they need the Pro plan)"));
    }
    Ok(())
}

/// Is `conv` a DM of `ext`? K2's ledger (the DMs K2 opened for them) or the
/// person's followed state (a DM someone else opened with them).
fn in_dm(ext: &str, conv: &str) -> bool {
    if let Some(h) = crate::home_chat::follower::get(ext) {
        if h.state.lock().unwrap_or_else(|p| p.into_inner()).convs.get(conv).is_some_and(|c| c.dm) {
            return true;
        }
    }
    k2_core::tool_hook::with_home_chat_db(|c| k2_core::tool_hook::admin_channels::dm_target(c, ext, conv)).ok().flatten().is_some()
}

/// The DM, checked: the caller is in it, and its other side may still be
/// reached (a disabled login's DM is read-only, HP-13).
fn the_dm(p: &Person, b: &Value) -> R<String> {
    let dm = id_field(b, "dm", "name the DM: dm is its conversation id")?;
    let ext = p.ext.to_string();
    if !in_dm(&ext, &dm) {
        return Err(Refusal::new(404, "dm_not_found", "no DM with that id for this login"));
    }
    if let Ok(Some(target)) = k2_core::tool_hook::with_home_chat_db(|c| k2_core::tool_hook::admin_channels::dm_target(c, &ext, &dm)) {
        crate::home_chat::people::check_target(&target)?;
    }
    Ok(dm)
}

fn sew(p: &Person, path: &str, body: &Value) -> R<Value> {
    let a = crate::home_chat::routes::sew_as(&p.ext.to_string(), "POST", path, Some(body))?;
    if a.ok() {
        Ok(a.body)
    } else {
        Err(bff::sew_refusal(&a))
    }
}

/// The join answer the client's call client takes (HP-40): Sew's
/// `CallJoinResponse` as is when `call_answer`, else its `voice` at the top
/// level with `call` beside it.
pub fn join_answer(sew: &Value, call_answer: bool) -> Value {
    if call_answer {
        return sew.clone();
    }
    let mut v = sew.get("voice").cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
    v["call"] = sew.get("call").cloned().unwrap_or(Value::Null);
    v
}

/// `POST /cli/voice/join {dm, join?, callAnswer?}`. `join` is Sew's call
/// join body (`self_mute`, `self_deaf`, `self_video`, `e2ee`,
/// `recording_notice`); K2 always asks for the call credential. Without
/// `join.ring` it rings unless a call already rings for this person in the
/// DM (then this answers it).
pub fn join(p: &Person, b: &Value, ctx: &Ctx) -> R<Value> {
    gate(ctx)?;
    let dm = the_dm(p, b)?;
    let mut body = b.get("join").cloned().unwrap_or_else(|| json!({}));
    if !body.is_object() {
        return Err(usage("join is Sew's call join body, a JSON object ({} for defaults)"));
    }
    let call_answer = match b.get("callAnswer") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(x)) => *x,
        Some(_) => return Err(usage("callAnswer is true or false")),
    };
    let ringing_here = crate::home_chat::follower::get(&p.ext.to_string())
        .is_some_and(|h| h.state.lock().unwrap_or_else(|e| e.into_inner()).incoming_ring(&dm).is_some());
    if body.get("ring").is_none() {
        body["ring"] = json!(!ringing_here);
    }
    body["call_credential"] = json!(true);
    let held_ms = match super::lane::gate_join(ctx.bff.lane_hold) {
        super::lane::Gate::Go { held_ms } => held_ms,
        super::lane::Gate::Refuse { state, held_ms } => {
            return Err(Refusal::new(503, "voice_unavailable", format!("voice on this server is unavailable ({}): its network lane is down", state.reason_text()))
                .details(json!({ "reason": k2_core::tool_hook::sew_wire::reasons::VOICE_UNAVAILABLE, "lane": state, "heldMs": held_ms }))
                .retry(bff::RETRY_SECS));
        }
    };
    let a = sew(p, &bff::paths::call_join(&dm), &body)?;
    let room = format!("dm:{dm}");
    let call_id = bff::note_call(p, &dm, &room);
    let voice = a.get("voice").unwrap_or(&Value::Null);
    let relay_only = voice.pointer("/media/relay_only").and_then(Value::as_bool).unwrap_or(false);
    Ok(json!({
        "ok": true,
        "callId": call_id,
        "dm": dm,
        "room": { "id": room, "kind": "dm", "channelId": dm },
        "call": a.get("call").cloned().unwrap_or(Value::Null),
        "created": a.get("created").cloned().unwrap_or(Value::Null),
        "relayOnly": relay_only,
        "heldMs": held_ms,
        "join": join_answer(&a, call_answer),
    }))
}

/// Sew's call id from the body.
fn call_id(b: &Value) -> R<String> {
    id_field(b, "callId", "name the call: callId is Sew's call id")
}

/// `POST /cli/voice/call/decline {dm, callId}`.
pub fn decline(p: &Person, b: &Value, ctx: &Ctx) -> R<Value> {
    gate(ctx)?;
    let dm = the_dm(p, b)?;
    let id = call_id(b)?;
    sew(p, &bff::paths::call_decline(&dm), &json!({ "call_id": id }))?;
    Ok(json!({ "ok": true, "declined": true, "callId": id }))
}

/// `POST /cli/voice/call/ring {dm, callId, memberIds?}`: ring again.
pub fn ring(p: &Person, b: &Value, ctx: &Ctx) -> R<Value> {
    gate(ctx)?;
    let dm = the_dm(p, b)?;
    let id = call_id(b)?;
    let mut body = json!({ "call_id": id });
    match b.get("memberIds") {
        None | Some(Value::Null) => {}
        Some(Value::Array(ids)) if ids.iter().all(|x| x.as_str().is_some_and(super::kit_door::dec)) => {
            body["member_ids"] = json!(ids);
        }
        Some(_) => return Err(usage("memberIds is a list of Sew member ids")),
    }
    let sew_body = sew(p, &bff::paths::call_ring(&dm), &body)?;
    Ok(json!({ "ok": true, "rung": true, "callId": id, "sew": sew_body }))
}

/// `POST /cli/voice/call/cancel {dm, callId}`: Sew's cancel when it has
/// `call_cancel` (the callee sees no missed call); otherwise a leave, and
/// the callee sees a missed call.
pub fn cancel(p: &Person, b: &Value, ctx: &Ctx) -> R<Value> {
    gate(ctx)?;
    let dm = the_dm(p, b)?;
    let id = call_id(b)?;
    if ctx.call_cancel {
        sew(p, &bff::paths::call_cancel(&dm), &json!({ "call_id": id }))?;
        return Ok(json!({ "ok": true, "cancelled": true, "via": "cancel", "callId": id }));
    }
    let mut v = bff::leave(p, None, &ctx.bff)?;
    v["cancelled"] = json!(false);
    v["via"] = json!("leave");
    v["callId"] = json!(id);
    v["hint"] = json!("this server's chat can't cancel a ring yet: you left the call, and they see a missed call");
    Ok(v)
}

/// `POST /cli/voice/call/leave {callId?}`: today's leave.
pub fn leave(p: &Person, b: &Value, ctx: &Ctx) -> R<Value> {
    let call = b.get("callId").and_then(Value::as_str);
    bff::leave(p, call, &ctx.bff)
}

#[cfg(test)]
#[path = "dm_call_tests.rs"]
mod tests;
