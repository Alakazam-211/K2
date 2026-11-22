//! Postal's permission cards (prd-postal-in-k2-v1 §5.5, S0 and S3; Rosson
//! ruling 5: every friend request, incoming or outgoing, goes through a
//! person's card; Rosson 2026-11-18: every step an agent takes to befriend
//! and talk to another agent is a card for its person, never a Settings
//! hunt).
//!
//! | Card | Who asks | Who answers | Allow does |
//! |---|---|---|---|
//! | **`postal.enable`** | the agent: `k2 postal enable`, or the Ask-with line of any `postal_off` refusal | the Owner | turns the server's Postal switch (Beta, off by default) on |
//! | **`postal.claim address=`** | the agent: `k2 postal claim <address>` | an Owner or Admin | gives the workspace that Postal address (kept here; the network confirms it later) |
//! | **`postal.invite to= [note=]`** | the agent: `k2 postal invite <address> --why "…"` | an Owner or Admin | sends the friend request over the Postal network (kept `pending_out` until this server is connected) |
//! | **`postal.pair.accept id= [pairing=code]`** | K2 itself, on both boxes, when a request's handshake is done (or a friend's key changed); the agent's `k2 postal accept <id>` puts the same card up again | the Owner (Admins when the Owner allows it) | with `pairing=code`, K2's own form on the card ("Type their code") shows this box's code and posts the code from the OTHER box to the decide route (Allow is then already done); Deny (the desktop's Reject) rejects |
//! | **`postal.link-p5`** | the agent (or a person): `k2 postal link-p5` | the Owner | mints a key that can only message this workspace, turns on what it needs, hands it to p5 |
//!
//! Tightening needs no card: an agent removes, blocks or rejects its own
//! friends and requests itself (`postal_routes`).
//!
//! **K2 presents these itself**, Beta or not, the way it presents
//! `vault.incoming.accept`: the route that needs a person puts the card on
//! the agent's pinned Thread in the same answer (`asked: true`, the CLI's
//! exit 2). With permission cards on for the agent, the walls map also
//! teaches the matching `Ask with: k2 permission request postal.…` line,
//! which finds the same open card. Every `apply` runs the route's own rules
//! as the person who clicked (`postal_routes::*_as`).
//!
//! - **Batching (§5.5):** K2 places each card through `batch::place`, so
//!   several requests in five minutes share one card (at most eight). A
//!   request with a pairing code never batches.
//! - **Requests never wake an agent:** the card goes on the Thread only,
//!   and its answer sends the agent no `[permission:]` turn ([`quiet`]).
//! - **Ask limits** (the denial block, the pending and hourly caps) apply to
//!   the agent's own asks (enable, claim, invite, link-p5); the per-handle
//!   cap on incoming requests is the store's (`PENDING_IN_CAP`).
//!
//! The codes never reach the card, the Thread or the agent: the form reads
//! this box's code from `GET /cli/postal/code` as the person, and Allow
//! without a code is refused before anything is decided ([`gate_answer`]).

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef, Undo, Wall,
    WallCoverage, WallMatch, WallMethod, Words,
};
use k2_core::permission::label;
use k2_core::permission::request::{params_hash, PermissionRequest, RequestId, RequestState};
use k2_core::postal::gate;
use k2_core::{cli_name, product_name};
use serde_json::{json, Map, Value};

use super::held::{error_of, route_floor, text_param};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use crate::permission::{answer, batch, limits, present, store};
use crate::postal_routes::{self, Ws};

pub(crate) const INVITE: &str = "postal.invite";
pub(crate) const PAIR_ACCEPT: &str = "postal.pair.accept";
pub(crate) const LINK_P5: &str = "postal.link-p5";
pub(crate) const CLAIM: &str = "postal.claim";
pub(crate) const ENABLE: &str = "postal.enable";
/// The session K2's own Postal cards carry (an incoming request: nobody
/// here asked for it).
pub(crate) const K2_SESSION: &str = "k2:postal";

const SETTINGS: SettingsRef = SettingsRef { page: "workspaces/{ws}/postal", path: "Settings → Workspaces → {ws} → Postal" };

/// A route's answer that put a card up (`{"asked": true, …}`, exit 2).
const ASKED: WallMatch = WallMatch::Held { status: 200, field: "asked", value: "true" };

/// Not sent yet: the words every outgoing step uses while the network half
/// waits (postal-sdk wire v2, the plane's pair API).
const NOT_SENT: &str = concat!("Allow sends the request over the Postal network as soon as this server is connected to it (", cli_name!(), " postal status says whether it is); until then it waits here.");

pub(crate) static INVITE_SPEC: ActionSpec = ActionSpec {
    id: INVITE,
    area: Area::Misc,
    params: &[
        ParamSpec { key: "to", kind: ParamKind::Text, required: true, list: false, choices: &[] },
        ParamSpec { key: "note", kind: ParamKind::Text, required: false, list: false, choices: &[] },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Send a Postal friend request for {ws}",
        what_changes: "{invite}",
        consequences: "{effect}",
        scope: "{ws}'s Postal address",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SETTINGS,
    walls: &[Wall {
        matches: ASKED,
        route_prefix: "/cli/postal/invite",
        method: WallMethod::Post,
        params: &[("to", ParamFrom::Body("/to")), ("note", ParamFrom::Body("/why"))],
    }],
    coverage: WallCoverage::Held,
    form: None,
    machine_hints: &[],
};

pub(crate) static PAIR_ACCEPT_SPEC: ActionSpec = ActionSpec {
    id: PAIR_ACCEPT,
    area: Area::Misc,
    params: &[
        ParamSpec { key: "id", kind: ParamKind::HeldRequest, required: true, list: false, choices: &[] },
        // `code`: the request carries a pairing code (SAS v2): Allow opens
        // K2's own form for the code shown on the other side's screen.
        ParamSpec { key: "pairing", kind: ParamKind::Choice, required: false, list: false, choices: &["code"] },
        // `link`: this server couldn't get its approval token when the
        // request arrived, and the kind of why (the card says so, and that
        // Accept links first). Kept on the card so its words stay fixed; a
        // kind, never free text.
        ParamSpec { key: "link", kind: ParamKind::Choice, required: false, list: false, choices: crate::postal_net::decide_token::KINDS },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Accept a Postal friend for {ws}",
        what_changes: "{accept}",
        consequences: "{effect}",
        scope: "{ws}'s Postal address",
        duration: "until removed",
        retry: "",
    },
    undo: Undo::None,
    settings: SETTINGS,
    walls: &[Wall { matches: ASKED, route_prefix: "/cli/postal/decide", method: WallMethod::Post, params: &[("id", ParamFrom::Body("/id"))] }],
    coverage: WallCoverage::Held,
    form: None,
    machine_hints: &[],
};

pub(crate) static LINK_P5_SPEC: ActionSpec = ActionSpec {
    id: LINK_P5,
    area: Area::Misc,
    params: &[],
    floor: Floor::Owner,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Link p5 to {ws}",
        what_changes: "{link}",
        consequences: "{effect}",
        scope: "{ws}",
        duration: "until the key is revoked",
        retry: "",
    },
    undo: Undo::None,
    settings: SETTINGS,
    walls: &[Wall { matches: ASKED, route_prefix: "/cli/postal/link-p5", method: WallMethod::Post, params: &[] }],
    coverage: WallCoverage::Held,
    form: None,
    machine_hints: &[],
};

pub(crate) static CLAIM_SPEC: ActionSpec = ActionSpec {
    id: CLAIM,
    area: Area::Misc,
    params: &[ParamSpec { key: "address", kind: ParamKind::Text, required: true, list: false, choices: &[] }],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Give {ws} a Postal address",
        what_changes: "{claim}",
        consequences: "{effect}",
        scope: "{ws}",
        duration: "until a person gives it up",
        retry: "",
    },
    undo: Undo::None,
    settings: SETTINGS,
    walls: &[Wall {
        matches: ASKED,
        route_prefix: "/cli/postal/claim",
        method: WallMethod::Post,
        params: &[("address", ParamFrom::Body("/address"))],
    }],
    coverage: WallCoverage::Held,
    form: None,
    machine_hints: &[],
};

pub(crate) static ENABLE_SPEC: ActionSpec = ActionSpec {
    id: ENABLE,
    area: Area::Box,
    params: &[],
    floor: Floor::Owner,
    direction: Direction::Loosen,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Postal (Beta)",
        what_changes: "{enable}",
        consequences: "{effect}",
        scope: "this server",
        duration: "until the owner turns it off",
        retry: "run your command again",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "general", path: "Settings → General → Postal" },
    walls: &[
        Wall { matches: WallMatch::Code { status: 403, code: "postal_off" }, route_prefix: "/cli/postal/", method: WallMethod::Any, params: &[] },
        Wall { matches: ASKED, route_prefix: "/cli/postal/enable", method: WallMethod::Post, params: &[] },
    ],
    coverage: WallCoverage::Code,
    form: None,
    machine_hints: &[],
};

// ── checks ───────────────────────────────────────────────────────────

fn ws_of(ctx: &Ctx) -> Option<Ws> {
    postal_routes::ws_of_token(&ctx.workspace_id)
}

fn off() -> Check {
    Check::Unavailable { code: "postal_off", why: "Postal is off on this server".into() }
}

fn check_invite(ctx: &Ctx) -> Check {
    if !k2_core::postal::enabled() {
        return off();
    }
    let Some(peer) = text_param(ctx, "to").and_then(k2_core::postal::parse_address) else {
        return Check::Moved("to= must be a Postal address: handle::label.postal.bot".into());
    };
    let found = k2_core::db::read(|conn| {
        let h = k2_core::postal::store::handle_for_workspace(conn, &ctx.workspace_id)?;
        Some((h.address.clone(), k2_core::postal::store::friend_by_peer(conn, &h.id, &peer.as_string())))
    });
    let Some((own, existing)) = found else {
        return Check::Moved(format!(
            "{} has no Postal address yet: ask for one first with {} postal claim <handle::label.postal.bot>",
            ctx.workspace_handle,
            cli_name!()
        ));
    };
    if own == peer.as_string() {
        return Check::Moved("that is this workspace's own address".into());
    }
    match existing.map(|f| f.status) {
        Some(s) if s == "accepted" || s == "rekey_pending" || s == "pending_out" => Check::AlreadyDone,
        Some(s) if s == "blocked" => Check::Moved("that address is blocked for this workspace".into()),
        Some(s) if s == "pending_in" => Check::Moved("they asked first: their request waits on its own card".into()),
        _ => Check::Needed,
    }
}

fn pending_friend(ctx: &Ctx) -> Result<k2_core::postal::store::Friend, String> {
    let id = text_param(ctx, "id").ok_or_else(|| format!("name the request: id=<its id> ({} postal friends)", cli_name!()))?;
    let f = k2_core::db::read(|conn| {
        let h = k2_core::postal::store::handle_for_workspace(conn, &ctx.workspace_id)?;
        k2_core::postal::store::friend(conn, id).filter(|f| f.handle_id == h.id)
    })
    .ok_or_else(|| format!("{} has no Postal request {id}", ctx.workspace_handle))?;
    if f.status != "pending_in" && f.status != "rekey_pending" {
        return Err(format!("request {id} is {} already", f.status));
    }
    Ok(f)
}

/// The request's friend row, whatever its state (the card's words stay
/// the same while it moves).
fn friend_of(ctx: &Ctx) -> Option<k2_core::postal::store::Friend> {
    let id = text_param(ctx, "id")?;
    k2_core::db::read(|conn| {
        let h = k2_core::postal::store::handle_for_workspace(conn, &ctx.workspace_id)?;
        k2_core::postal::store::friend(conn, id).filter(|f| f.handle_id == h.id)
    })
}

/// Does this card's request carry a pairing code?
fn wants_code(ctx: &Ctx) -> bool {
    text_param(ctx, "pairing") == Some("code")
}

fn check_pair_accept(ctx: &Ctx) -> Check {
    if !k2_core::postal::enabled() {
        return off();
    }
    match pending_friend(ctx) {
        Ok(_) => Check::Needed,
        Err(e) => match friend_of(ctx) {
            // The person's code went through K2's form on this card: this
            // box signed, and the friend waits on (or has) the other side's.
            Some(f) if f.status == "accepted" || (f.status == "pending_out" && f.decided_by.is_some()) => Check::AlreadyDone,
            _ => Check::Moved(e),
        },
    }
}

/// Allow on a request with a pairing code needs the code typed in K2's
/// form first (it goes straight to the decide route as this person). Refused
/// here, before anything is decided, so the card stays open.
pub(crate) fn gate_answer(req: &PermissionRequest) -> Result<(), CliResponse> {
    if req.action != PAIR_ACCEPT {
        return Ok(());
    }
    let ctx = present::ctx_of(req);
    if wants_code(&ctx) && pending_friend(&ctx).is_ok() {
        return Err(postal_routes::err(
            "409 Conflict",
            "postal_code_needed",
            concat!("type the 6-digit code shown on the other side's screen in ", product_name!(), "'s form on this card first"),
        ));
    }
    Ok(())
}

fn check_link_p5(_ctx: &Ctx) -> Check {
    match p5_program() {
        Some(_) => Check::Needed,
        None => Check::Unavailable { code: "p5_missing", why: P5_MISSING.into() },
    }
}

fn check_claim(ctx: &Ctx) -> Check {
    if !k2_core::postal::enabled() {
        return off();
    }
    let Some(addr) = text_param(ctx, "address").and_then(k2_core::postal::parse_address) else {
        return Check::Moved("address= must be a Postal address: handle::label.postal.bot".into());
    };
    let addr = addr.as_string();
    let (mine, holder) = k2_core::db::read(|conn| {
        (
            k2_core::postal::store::handle_for_workspace(conn, &ctx.workspace_id).map(|h| h.address),
            k2_core::postal::store::address_holder(conn, &addr),
        )
    });
    match (mine, holder) {
        (Some(m), _) if m == addr => Check::AlreadyDone,
        (Some(m), _) => Check::Moved(format!("{} has the Postal address {m} already (one per workspace)", ctx.workspace_handle)),
        (None, Some(ws)) if ws != ctx.workspace_id => Check::Moved("another workspace on this server holds that address".into()),
        _ => Check::Needed,
    }
}

fn check_enable(_ctx: &Ctx) -> Check {
    if k2_core::postal::enabled() {
        Check::AlreadyDone
    } else {
        Check::Needed
    }
}

// ── words ────────────────────────────────────────────────────────────

/// The cards' computed words (`{invite}`, `{accept}`, `{link}`, `{claim}`,
/// `{enable}`, `{effect}`).
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    let ws = ctx.workspace_handle.clone();
    let mut out = ctx.clone();
    let mut put = |k: &str, v: String| {
        out.params.insert(k.into(), json!(v));
    };
    match action {
        INVITE => {
            let to = text_param(ctx, "to").unwrap_or("an agent on Postal").to_string();
            put("invite", format!("Ask {to} to be friends with {ws} on Postal"));
            put(
                "effect",
                format!("Once {to} accepts, the two agents can write to each other. {ws} or a person can remove or block {to} any time. {NOT_SENT}"),
            );
        }
        PAIR_ACCEPT => {
            let f = friend_of(ctx);
            let peer = f.as_ref().map(|f| f.peer_address.clone()).unwrap_or_else(|| "an agent on Postal".into());
            let owner_name = f.as_ref().and_then(|f| f.owner_name.clone()).filter(|o| !o.trim().is_empty());
            let owner = owner_name.as_ref().map(|o| format!(" (its owner: {o})")).unwrap_or_default();
            let rekey = f.as_ref().is_some_and(|f| f.status == "rekey_pending");
            // The headline says what accepting does; the card's own buttons
            // say how (K2's code form, when the request carries a code).
            let what = if rekey {
                format!("Trust the new key of {peer}{owner}, a Postal friend of {ws}")
            } else {
                format!("Let {peer}{owner} be friends with {ws} on Postal")
            };
            put("accept", what);
            let mut effect = if rekey {
                format!("{peer}\u{2019}s key changed on the Postal network, so its mail waits until a person here trusts the new key. Turning it down ends the friendship.")
            } else {
                format!("{peer} can then write to {ws}. Its mail arrives labelled [postal {peer} \u{b7} p_\u{2026}], every line quoted. You can remove or block {peer} later.")
            };
            if wants_code(ctx) {
                // Never the code itself: the agent reads its own Thread.
                let with = owner_name.clone().unwrap_or_else(|| format!("{peer}\u{2019}s person"));
                effect.push_str(&format!(
                    " First you and {with} compare 6-digit codes in {}\u{2019}s own form: they match only if nobody sits between the two servers.",
                    product_name!()
                ));
            }
            if let Some(words) = text_param(ctx, "link").and_then(crate::postal_net::decide_token::kind_words) {
                effect.push_str(&format!(" {}", cant_accept_words(words)));
            }
            if let Some(fp) = f.as_ref().and_then(|f| f.peer_fingerprint.clone()) {
                effect.push_str(&format!(" Their key: {fp}."));
            }
            // The asker's note goes last, in quotes, as they wrote it (a
            // client that shows the note on its own drops this sentence).
            if let Some(note) = f.as_ref().and_then(|f| f.note.clone()).filter(|n| !n.trim().is_empty()) {
                effect.push_str(&format!(" Their note: \u{201c}{}\u{201d}", note.trim()));
            }
            put("effect", effect);
        }
        LINK_P5 => {
            let path = ws_of(ctx).map(|w| w.path).unwrap_or_default();
            put("link", format!("Let p5, Postal's own client on this machine, deliver Postal mail to {ws}"));
            let mut effect = format!(
                "{} makes an API key that can only message {ws}'s main chat and hands it to p5 (never shown). Mail then reaches {ws} under a [postal <sender> · m_…] label, every line quoted.",
                product_name!()
            );
            if !crate::misc_routes::api_enabled() {
                effect.push_str(" The public /v1 API is off on this server: Allow turns it on, which opens the whole /v1 surface (every API key's doors), not only this one.");
            }
            if !path.is_empty() && !k2_core::workspace::settings::remote_instruct_allowed_for_path(&path) {
                effect.push_str(&format!(
                    " Remote instruction is off for {ws}: Allow turns it on, which also lets agents on servers paired with this one message {ws}."
                ));
            }
            put("effect", effect);
        }
        CLAIM => {
            let addr = text_param(ctx, "address")
                .and_then(k2_core::postal::parse_address)
                .map(|a| a.as_string())
                .unwrap_or_else(|| "a Postal address".into());
            put("claim", format!("Give {ws} the Postal address {addr}"));
            put(
                "effect",
                format!(
                    "Agents on other machines can then ask to be friends with {ws} at {addr}; every request still waits on a person's card. One address per workspace: its sidecars and subagents share it. The address's label must be this server's {} Connect name; it goes on the Postal network once this server is connected.",
                    product_name!()
                ),
            );
        }
        ENABLE => {
            put("enable", "Turn on Postal (Beta) on this server, so its agents can have friends on other machines".into());
            put(
                "effect",
                format!(
                    "Every workspace's Postal tab appears, and its agent can ask, on cards like this one, for a Postal address and for friends. Every friend request, in or out, still waits on a person's card. This server talks to the Postal network with its {} Connect account. Off again any time in Settings → General → Postal, or with {} postal disable. Agents on this server aren't isolated from each other yet.",
                    product_name!(),
                    cli_name!()
                ),
            );
        }
        _ => return None,
    }
    Some(out)
}

// ── applies ──────────────────────────────────────────────────────────

fn refused_of(r: CliResponse) -> Applied {
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    let (code, message) = error_of(&v, "postal_refused");
    Applied::Refused { code, message }
}

fn apply_invite(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    if let Err(refused) = route_floor(postal_routes::INVITE, who) {
        return refused;
    }
    let Some(to) = text_param(ctx, "to") else {
        return Applied::Refused { code: "bad_params".into(), message: "to= is the address to ask".into() };
    };
    let r = postal_routes::invite_as(&postal_routes::who_of(who), &ctx.workspace_id, to, text_param(ctx, "note"));
    if !r.status.starts_with("200") {
        return refused_of(r);
    }
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    let message = if let Some(hint) = v["hint"].as_str().filter(|_| v["sent"].as_bool() != Some(true)) {
        // Kept here: this server can't accept the answer yet.
        format!("Friend request to {to} for {}: {hint}.", ctx.workspace_handle)
    } else if v["sent"].as_bool() == Some(true) {
        format!(
            "Friend request to {to} sent for {}. When they answer, a person on each side types the other's 6-digit code on a card. {} postal friends shows it under Requests out.",
            ctx.workspace_handle,
            cli_name!()
        )
    } else {
        format!(
            "Friend request to {to} recorded for {}; it goes out as soon as this server is connected to the Postal network ({} postal status). {} postal friends shows it under Requests out.",
            ctx.workspace_handle,
            cli_name!(),
            cli_name!()
        )
    };
    Applied::Done { message }
}

fn apply_pair_accept(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    if let Err(refused) = route_floor(postal_routes::DECIDE, who) {
        return refused;
    }
    let Some(id) = text_param(ctx, "id") else {
        return Applied::Refused { code: "bad_params".into(), message: "id= is the request".into() };
    };
    let r = postal_routes::decide_as(&postal_routes::who_of(who), &ctx.workspace_id, id, true, None);
    if !r.status.starts_with("200") {
        return refused_of(r);
    }
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    let peer = v.pointer("/friend/peer").and_then(Value::as_str).unwrap_or("them");
    Applied::Done { message: format!("{peer} and {} are Postal friends now.", ctx.workspace_handle) }
}

fn apply_claim(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    if let Err(refused) = route_floor(postal_routes::CLAIM, who) {
        return refused;
    }
    let Some(address) = text_param(ctx, "address") else {
        return Applied::Refused { code: "bad_params".into(), message: "address= is the Postal address".into() };
    };
    let r = postal_routes::claim_as(&postal_routes::who_of(who), &ctx.workspace_id, address);
    if !r.status.starts_with("200") {
        return refused_of(r);
    }
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    let addr = v.pointer("/handle/address").and_then(Value::as_str).unwrap_or(address);
    Applied::Done {
        message: format!(
            "{} has the Postal address {addr}, kept on this server; it is registered with the Postal network once this server is connected. Ask for friends with {} postal invite <address> --why \"…\".",
            ctx.workspace_handle,
            cli_name!()
        ),
    }
}

fn apply_enable(_ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    if let Err(refused) = route_floor(postal_routes::ENABLE, who) {
        return refused;
    }
    let r = postal_routes::enable_as(&postal_routes::who_of(who));
    if !r.status.starts_with("200") {
        return refused_of(r);
    }
    Applied::Done {
        message: format!(
            "Postal is on. Next: {} postal claim <handle::label.postal.bot> asks for this workspace's address.",
            cli_name!()
        ),
    }
}

/// Deny on an incoming request rejects it, as the person, through the
/// decide route's rules. Decided elsewhere already: left as it is.
pub(crate) fn on_denied(req: &PermissionRequest, who: &Answerer) {
    if req.action != PAIR_ACCEPT {
        return;
    }
    let ctx = present::ctx_of(req);
    let Some(id) = text_param(&ctx, "id") else { return };
    let r = postal_routes::decide_as(&postal_routes::who_of(who), &ctx.workspace_id, id, false, None);
    if !r.status.starts_with("200") {
        k2_core::log_debug!("[postal] reject {id} from its card: {} {}", r.status, r.body);
    }
}

/// No `[permission:]` turn for this answer: K2's own card for an incoming
/// request (or a changed key). A request never wakes an agent (§5.5), and
/// neither does a person's answer to it; `k2 postal friends` shows it.
pub(crate) fn quiet(req: &PermissionRequest) -> bool {
    req.action == PAIR_ACCEPT && req.session_id.as_deref() == Some(K2_SESSION)
}

/// Never part of a batch: a request that carries a pairing code (it is
/// read alone, never under "Allow all").
pub(crate) fn never_batched(req: &PermissionRequest) -> bool {
    req.action == PAIR_ACCEPT
        && (req.params.get("pairing").and_then(Value::as_str) == Some("code")
            || req.params.get("id").and_then(Value::as_str).is_some_and(|id| gate::code(id).is_some()))
}

// ── the p5 link (S0) ─────────────────────────────────────────────────

const P5_MISSING: &str = "p5, Postal's own client, isn't installed on this machine (not on the daemon's PATH): install p5, then ask again";

#[cfg(test)]
pub(crate) static P5_FOR_TESTS: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

/// The p5 program on the daemon's PATH.
fn p5_program() -> Option<std::path::PathBuf> {
    #[cfg(test)]
    if let Some(p) = P5_FOR_TESTS.lock().ok().and_then(|g| g.clone()) {
        return Some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("p5")).find(|p| p.is_file())
}

/// Revoke every earlier p5 link key of this workspace (one link at a time).
fn revoke_old_links(label: &str) {
    for k in k2_core::api_keys::list_api_keys().unwrap_or_default() {
        if k.label.as_deref() == Some(label) && k.revoked_at.is_none() {
            if let Err(e) = k2_core::api_keys::revoke_api_key(&k.id) {
                k2_core::log_debug!("[postal] revoke old link key {}: {e}", k.id);
            }
        }
    }
}

/// Hand `key` to `p5 k2 link <slug> --path <dir> --key-stdin` on stdin.
fn run_p5_link(p5: &std::path::Path, slug: &str, dir: &str, key: &str) -> Result<(), String> {
    use std::io::Write;
    let mut child = k2_core::bg_proc::command(p5)
        .args(["k2", "link", slug, "--path", dir, "--key-stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("p5 didn't start: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(key.as_bytes()).map_err(|e| format!("p5 didn't take the key: {e}"))?;
        stdin.write_all(b"\n").map_err(|e| format!("p5 didn't take the key: {e}"))?;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                let mut why = String::new();
                if let Some(mut e) = child.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut why);
                }
                let why: String = why.chars().filter(|c| !c.is_control() || *c == ' ').take(300).collect();
                return Err(format!("p5 k2 link failed ({status}): {}", why.trim()));
            }
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                return Err("p5 k2 link took longer than 20 s".into());
            }
            Err(e) => return Err(format!("p5 k2 link: {e}")),
        }
    }
}

fn apply_link_p5(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    if let Err(refused) = route_floor(postal_routes::LINK_P5, who) {
        return refused;
    }
    if !postal_routes::who_of(who).is_owner() {
        return Applied::Refused { code: "owner_only".into(), message: "the server's owner links p5".into() };
    }
    let Some(ws) = ws_of(ctx) else {
        return Applied::Refused { code: "not_found".into(), message: format!("{} isn't registered here", ctx.workspace_handle) };
    };
    let Some(p5) = p5_program() else {
        return Applied::Refused { code: "p5_missing".into(), message: P5_MISSING.into() };
    };
    let slug = ws.handle.clone();
    let label = k2_core::postal::link_label(&slug);
    revoke_old_links(&label);
    let caps = k2_core::api_keys::ApiCapabilities { canonical_message: true, ..k2_core::api_keys::ApiCapabilities::none() };
    let grant = json!([slug]).to_string();
    let (key_id, raw) = match k2_core::api_keys::create_api_key(&label, None, Some(&grant), None, None, Some(caps)) {
        Ok(k) => k,
        Err(e) => return Applied::Refused { code: "internal".into(), message: e },
    };
    if let Err(e) = run_p5_link(&p5, &slug, &ws.path, &raw) {
        if let Err(r) = k2_core::api_keys::revoke_api_key(&key_id) {
            k2_core::log_debug!("[postal] revoke unused link key {key_id}: {r}");
        }
        return Applied::Refused { code: "p5_link_failed".into(), message: e };
    }
    let mut turned_on = Vec::new();
    if !crate::misc_routes::api_enabled() {
        match k2_core::app_settings::update(json!({ "apiEnabled": true })) {
            Ok(m) => k2_core::app_settings::set_api_enabled(m.api_enabled),
            Err(e) => return Applied::Refused { code: "internal".into(), message: e },
        }
        turned_on.push("the public /v1 API".to_string());
    }
    if !k2_core::workspace::settings::remote_instruct_allowed_for_path(&ws.path) {
        if let Err(e) = k2_core::workspace::settings::update_project_setting(&ws.path, "allow_remote_instruct", "1") {
            return Applied::Refused { code: "internal".into(), message: e.to_string() };
        }
        turned_on.push(format!("remote instruction for {slug}"));
    }
    crate::hire_routes::audit("postal-link-p5", &postal_routes::who_of(who).label(), &format!("ok {slug} key {key_id}"));
    let on = if turned_on.is_empty() { String::new() } else { format!(" Turned on: {}.", turned_on.join(", ")) };
    Applied::Done { message: format!("p5 is linked to {slug}: Postal mail it receives reaches {slug}'s main chat.{on}") }
}

// ── presenting ───────────────────────────────────────────────────────

fn open_for(workspace_id: &str, action: &str, held_ref: Option<&str>) -> Vec<PermissionRequest> {
    store::open(Some(workspace_id))
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.action == action && (held_ref.is_none() || r.held_ref.as_deref() == held_ref))
        .collect()
}

/// Who a card is for: K2's own (an incoming request) or an agent's ask
/// (its session, so the ask limits and batching are that session's).
#[derive(Debug, Clone, Copy)]
enum Asker<'a> {
    Daemon,
    Agent { session: &'a str },
}

/// Put one card up on the workspace's pinned Thread (joining a batch when
/// one fits). Answers `{card, request, addr, batch}`, `{card, request,
/// waiting: true}` when one is open already, `{card: null, why}` when it
/// can't go up, or `{card: null, limited: {status, code, hint}}` when an
/// agent's ask hit a limit.
fn present(ws: &Ws, action: &str, params: Map<String, Value>, why: &str, held_ref: Option<String>, asker: Asker<'_>) -> Value {
    let Some(reg) = present::find_action(action) else {
        return json!({ "card": null, "why": "not registered" });
    };
    let hash = params_hash(&params);
    if let Some(open) = open_for(&ws.id, action, held_ref.as_deref()).into_iter().find(|r| held_ref.is_some() || r.params_hash == hash) {
        return json!({ "card": open.card_id, "request": open.display_id(), "waiting": true, "addr": open.addr });
    }
    let Some(conversation_id) = crate::overlay_routes::pinned_chat_of(&ws.id) else {
        return json!({ "card": null, "why": "this agent has no pinned chat Thread to put the card on" });
    };
    let handle = present::workspace_handle(&ws.id);
    let ctx = Ctx { workspace_id: ws.id.clone(), workspace_handle: handle.clone(), params: params.clone() };
    if let Check::Moved(why) | Check::Unavailable { why, .. } = reg.check_named(&ctx) {
        return json!({ "card": null, "why": why });
    }
    let at = k2_core::overlay::now_secs();
    let session = match asker {
        Asker::Daemon => K2_SESSION.to_string(),
        Asker::Agent { session } => {
            if let Err(l) = limits::check_ask(&ws.id, action, &hash, at) {
                return json!({ "card": null, "limited": { "status": l.status, "code": l.code, "hint": l.hint } });
            }
            session.to_string()
        }
    };
    let req = PermissionRequest {
        id: RequestId::new(),
        workspace_id: ws.id.clone(),
        session_id: Some(session).filter(|s| !s.is_empty()),
        conversation_id: None,
        addr: Some(handle.clone()),
        card_id: None,
        action: action.to_string(),
        params: Value::Object(params),
        params_hash: hash.clone(),
        text_hash: present::text_hash_of(reg.spec, &ctx),
        why: why.chars().take(k2_core::permission::request::limits::WHY_MAX_CHARS).collect(),
        state: RequestState::Pending,
        floor: present::floor_str(reg.spec.floor).to_string(),
        held_ref,
        batch_id: None,
        ask_number: limits::ask_number(&ws.id, action, &hash, at),
        asked_for: None,
        created_at: at,
        expires_at: at + reg.spec.expiry.default_secs,
        answered_at: None,
        answered_by: None,
        answered_via: None,
        reason: None,
        result_message: None,
        updated_why: None,
        updated_at: None,
    };
    if let Err(e) = store::insert(&req) {
        return json!({ "card": null, "why": e });
    }
    let placed = match batch::place(&req, &conversation_id) {
        Ok(p) => p,
        Err(e) => {
            // Never strand a request without its card.
            if let Err(x) = store::cas_open_to(req.id.as_str(), RequestState::Cancelled) {
                k2_core::log_debug!("[postal] withdraw {}: {x}", req.id);
            }
            return json!({ "card": null, "why": e });
        }
    };
    if let Ok(Some(stored)) = store::get(req.id.as_str()) {
        let by = match asker {
            Asker::Daemon => "k2".to_string(),
            Asker::Agent { .. } => format!("agent:{handle}"),
        };
        answer::audit(k2_core::permission::audit_event::REQUEST, &by, if by == "k2" { "daemon" } else { "passport" }, &stored);
    }
    json!({ "card": placed.card_id, "request": req.display_id(), "addr": handle, "batch": placed.batch })
}

/// The route's answer for a card it put up (or found open).
fn asked(v: Value, action: &str, what: &str, hint: &str) -> CliResponse {
    if let Some(l) = v.get("limited").filter(|l| l.is_object()) {
        let status = match l["status"].as_u64() {
            Some(429) => "429 Too Many Requests",
            Some(500) => "500 Internal Server Error",
            _ => "409 Conflict",
        };
        return postal_routes::err(status, l["code"].as_str().unwrap_or("limited"), l["hint"].as_str().unwrap_or("asked too often"));
    }
    if v["card"].is_null() {
        let why = v["why"].as_str().unwrap_or("the card couldn't go up").to_string();
        return postal_routes::err("409 Conflict", "card_not_presented", format!("{what}: {why}"));
    }
    CliResponse::ok_json(
        json!({
            "ok": true,
            "asked": true,
            "action": action,
            "card": v["card"],
            "request": v["request"],
            "addr": v["addr"],
            "waiting": v["waiting"].as_bool().unwrap_or(false),
            "hint": hint,
        })
        .to_string(),
    )
}

const ANSWER_COMES_BACK: &str = "a person answers the card on your pinned Thread; the answer comes back to you as a [permission:] turn";

/// An agent's invite: K2 presents `postal.invite` with the agent's why.
pub(crate) fn ask_invite(ws: &Ws, session: &str, to: &str, why: &str) -> CliResponse {
    let mut params = Map::new();
    params.insert("to".into(), json!(to));
    params.insert("note".into(), json!(why));
    asked(present(ws, INVITE, params, why, None, Asker::Agent { session }), INVITE, "the friend request card", ANSWER_COMES_BACK)
}

/// An agent's claim: K2 presents `postal.claim` (an Owner's or Admin's).
pub(crate) fn ask_claim(ws: &Ws, session: &str, address: &str, why: Option<&str>) -> CliResponse {
    let mut params = Map::new();
    params.insert("address".into(), json!(address));
    let why = why.map(str::to_string).unwrap_or_else(|| format!("{} asks for the Postal address {address}, so agents on other machines can ask to be its friends.", ws.handle));
    asked(present(ws, CLAIM, params, &why, None, Asker::Agent { session }), CLAIM, "the Postal address card", ANSWER_COMES_BACK)
}

/// An agent's `k2 postal enable`: K2 presents `postal.enable` (the Owner's).
pub(crate) fn ask_enable(ws: &Ws, session: &str, why: Option<&str>) -> CliResponse {
    let why = why
        .map(str::to_string)
        .unwrap_or_else(|| format!("{} asks to turn on Postal, so it can have friends (agents on other machines).", ws.handle));
    asked(present(ws, ENABLE, Map::new(), &why, None, Asker::Agent { session }), ENABLE, "the Postal switch card", ANSWER_COMES_BACK)
}

/// `k2 postal link-p5`: K2 presents `postal.link-p5` (the Owner's).
pub(crate) fn ask_link_p5(who: &Who, ws: &Ws) -> CliResponse {
    if p5_program().is_none() {
        return postal_routes::err("409 Conflict", "p5_missing", P5_MISSING);
    }
    let (why, asker) = match who.passport() {
        Some((_, session)) => (format!("{} wants Postal mail that p5 receives on this machine delivered to it.", ws.handle), Asker::Agent { session }),
        None => (format!("{} asked to link p5 to {}.", who.label(), ws.handle), Asker::Daemon),
    };
    asked(present(ws, LINK_P5, Map::new(), &why, None, asker), LINK_P5, "the p5 link card", ANSWER_COMES_BACK)
}

/// The agent's `k2 postal accept <id>`: the same card K2 put up for the
/// request (put up again when it expired).
pub(crate) fn ask_pair_accept(ws: &Ws, f: &k2_core::postal::store::Friend) -> CliResponse {
    asked(
        present_pair_accept(ws, f),
        PAIR_ACCEPT,
        "the friend request card",
        &format!(
            "a person answers the card on your pinned Thread (the Owner, or an Admin when the Owner allows it); the answer doesn't wake you: {} postal friends shows it",
            cli_name!()
        ),
    )
}

/// K2's card for an incoming request or a changed key (`held_ref` = the
/// friend row).
pub(crate) fn present_pair_accept(ws: &Ws, f: &k2_core::postal::store::Friend) -> Value {
    let mut params = Map::new();
    params.insert("id".into(), json!(f.id));
    if gate::code(&f.id).is_some() {
        params.insert("pairing".into(), json!("code"));
    }
    let why = if f.status == "rekey_pending" {
        format!("{}'s key changed on the Postal network. A person here trusts the new key or ends the friendship.", f.peer_address)
    } else {
        format!("{} asked to be friends on Postal. A person here accepts or rejects it.", f.peer_address)
    };
    // A request with a pairing code is confirmed with the owner's token.
    if gate::code(&f.id).is_some() {
        if let Some(why) = cant_accept_now() {
            params.insert("link".into(), json!(why));
        }
    }
    present(ws, PAIR_ACCEPT, params, &why, Some(f.id.clone()), Asker::Daemon)
}

/// The card's sentence when this server couldn't get its approval token as
/// the request arrived: what is missing, why, and the one-click fix (Accept
/// links first; Link now in Settings). Never a Postal website page.
pub(crate) fn cant_accept_words(why: &str) -> String {
    format!(
        "This server can't accept friend requests yet: it doesn't have its Postal approval token ({}). Accept gets it first, with this server's {} Connect account, then accepts; or the owner presses Link now in Settings → General → Postal.",
        why.trim().trim_end_matches('.'),
        product_name!()
    )
}

/// Why (its kind) this server can't accept a request right now, when it can't: with
/// the network connected and no approval token here, it is asked for first
/// (unattended: it waits after a failure, never a storm).
fn cant_accept_now() -> Option<String> {
    use crate::postal_net::decide_token::{self, Why};
    if !k2_core::postal::gate::network_available() {
        return None;
    }
    let net = crate::postal_net::net()?;
    decide_token::ready(&net, Why::Request, "server").err().map(|e| decide_token::kind(&e).to_string())
}

/// Close every open `postal.pair.accept` card for friend row `friend_id`:
/// it was decided, removed or blocked somewhere else. A card being
/// answered right now is the one deciding, and is left to its answer.
pub(crate) fn close_pair_cards(workspace_id: &str, friend_id: &str, how: &str, who: &Who) {
    let open = open_for(workspace_id, PAIR_ACCEPT, Some(friend_id));
    if open.is_empty() {
        return;
    }
    let (state, lbl) = match how {
        "accepted" if who.passport().is_none() => (RequestState::Approved, label::ALLOW),
        "rejected" if who.passport().is_none() => (RequestState::Denied, label::DENY),
        _ => {
            let by = if who.passport().is_some() { format!(" by {} itself", present::workspace_handle(workspace_id)) } else { " somewhere else".into() };
            answer::close_open(&open, RequestState::Cancelled, &format!("Closed: the request was {how}{by}."));
            return;
        }
    };
    let by = match who {
        Who::Login { username, .. } => username.clone(),
        _ => "owner".to_string(),
    };
    let via = if by == "owner" { "desktop" } else { "web" };
    let author = if by == "owner" { k2_core::overlay::PostAuthor::owner() } else { k2_core::overlay::PostAuthor::user(&by) };
    let msg = format!("{} somewhere else (the Postal tab or another card).", if how == "accepted" { "Accepted" } else { "Rejected" });
    let now = k2_core::overlay::now_secs();
    for r in &open {
        match store::cas_open_to(r.id.as_str(), state) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                k2_core::log_debug!("[postal] close {}: {e}", r.id);
                continue;
            }
        }
        let d = store::Decided { state, by: Some(by.as_str()), via: Some(via), reason: None, message: Some(msg.as_str()), at: now };
        if let Err(e) = store::decide(r.id.as_str(), &d) {
            k2_core::log_debug!("[postal] close {}: {e}", r.id);
        }
        if let (Some(c), Some(k)) = (r.conversation_id.as_deref(), r.card_id.as_deref()) {
            present::refresh_card(c, k, present::CardClose::Answer { label: lbl, by: Some(author.clone()) });
        }
        if let Ok(Some(done)) = store::get(r.id.as_str()) {
            answer::audit(k2_core::permission::audit_event::ANSWER, "k2", "daemon", &done);
        }
    }
}

/// Close every open card for friend row `friend_id` because of something
/// the Postal network did (friends now, the other side ended it).
pub(crate) fn close_pair_cards_by_network(workspace_id: &str, friend_id: &str, why: &str) {
    let open = open_for(workspace_id, PAIR_ACCEPT, Some(friend_id));
    if !open.is_empty() {
        answer::close_open(&open, RequestState::Cancelled, &format!("Closed: {why}."));
    }
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &INVITE_SPEC, check: check_invite, apply: apply_invite },
        Registered { spec: &PAIR_ACCEPT_SPEC, check: check_pair_accept, apply: apply_pair_accept },
        Registered { spec: &LINK_P5_SPEC, check: check_link_p5, apply: apply_link_p5 },
        Registered { spec: &CLAIM_SPEC, check: check_claim, apply: apply_claim },
        Registered { spec: &ENABLE_SPEC, check: check_enable, apply: apply_enable },
    ]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "postal_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "postal_async_tests.rs"]
mod async_tests;

#[cfg(test)]
#[path = "postal_link_tests.rs"]
mod link_tests;
