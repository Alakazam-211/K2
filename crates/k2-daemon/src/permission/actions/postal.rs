//! Postal's permission cards (prd-postal-in-k2-v1 §5.5, S0 and S3; Rosson
//! ruling 5: every friend request, incoming or outgoing, goes through a
//! person's card).
//!
//! - **`postal.invite to= [note=]`** — an agent wants to be friends with an
//!   outside agent. `k2 postal invite <addr> --why "…"` presents it on the
//!   agent's pinned Thread; only Allow records the request (`pending_out`,
//!   for the Postal network half to send). An Owner or Admin answers.
//! - **`postal.pair.accept id=`** — an outside agent asked to be friends
//!   with this workspace. K2 presents it itself when the request arrives
//!   ([`present_pair_accept`]). Allow accepts and pins the key the request
//!   carried; Deny rejects. The Owner answers (Admins when the Owner allows
//!   it: the decide route checks at the click).
//! - **`postal.link-p5`** (S0) — link p5, Postal's own client on this
//!   machine, to the workspace, so Postal mail p5 receives reaches the
//!   agent through the `/v1` door with a key that can do nothing else. The
//!   card says what is off and what turning each on opens (P13, P14, P19,
//!   P20); Allow mints the key, turns those on, and hands the key to p5 on
//!   stdin. The Owner answers.
//!
//! None has a wall: an agent never gets a refusal to ask from; K2 presents
//! each card from the route that needs it. Every `apply` runs the route's
//! own rules as the person who clicked (`postal_routes::*_as`).

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamKind, ParamSpec, Proof, SettingsRef, Undo, WallCoverage, Words,
};
use k2_core::permission::label;
use k2_core::permission::request::{params_hash, PermissionRequest, RequestId, RequestState};
use k2_core::{cli_name, product_name};
use serde_json::{json, Map, Value};

use super::held::{error_of, route_floor, text_param};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use crate::permission::{answer, limits, present, store};
use crate::postal_routes::{self, Ws};

pub(crate) const INVITE: &str = "postal.invite";
pub(crate) const PAIR_ACCEPT: &str = "postal.pair.accept";
pub(crate) const LINK_P5: &str = "postal.link-p5";
/// The session K2's own Postal cards carry.
pub(crate) const K2_SESSION: &str = "k2:postal";

const SETTINGS: SettingsRef = SettingsRef { page: "workspaces/{ws}/postal", path: "Settings → Workspaces → {ws} → Postal" };

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
    walls: &[],
    coverage: WallCoverage::None,
    machine_hints: &[],
};

pub(crate) static PAIR_ACCEPT_SPEC: ActionSpec = ActionSpec {
    id: PAIR_ACCEPT,
    area: Area::Misc,
    params: &[ParamSpec { key: "id", kind: ParamKind::HeldRequest, required: true, list: false, choices: &[] }],
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
    walls: &[],
    coverage: WallCoverage::None,
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
    walls: &[],
    coverage: WallCoverage::None,
    machine_hints: &[],
};

// ── checks ───────────────────────────────────────────────────────────

fn ws_of(ctx: &Ctx) -> Option<Ws> {
    postal_routes::ws_of_token(&ctx.workspace_id)
}

fn check_invite(ctx: &Ctx) -> Check {
    if !k2_core::postal::enabled() {
        return Check::Unavailable { code: "postal_off", why: "Postal is off on this server".into() };
    }
    let Some(peer) = text_param(ctx, "to").and_then(k2_core::postal::parse_address) else {
        return Check::Moved("to= must be a Postal address: handle::label.postal.bot".into());
    };
    let found = k2_core::db::read(|conn| {
        let h = k2_core::postal::store::handle_for_workspace(conn, &ctx.workspace_id)?;
        Some((h.address.clone(), k2_core::postal::store::friend_by_peer(conn, &h.id, &peer.as_string())))
    });
    let Some((own, existing)) = found else {
        return Check::Moved(format!("{} has no Postal address yet: a person claims one in its Postal tab", ctx.workspace_handle));
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
    let id = text_param(ctx, "id").ok_or("name the request: id=<its id> (k2 postal friends)")?;
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

fn check_pair_accept(ctx: &Ctx) -> Check {
    if !k2_core::postal::enabled() {
        return Check::Unavailable { code: "postal_off", why: "Postal is off on this server".into() };
    }
    match pending_friend(ctx) {
        Ok(_) => Check::Needed,
        Err(e) => Check::Moved(e),
    }
}

fn check_link_p5(_ctx: &Ctx) -> Check {
    match p5_program() {
        Some(_) => Check::Needed,
        None => Check::Unavailable { code: "p5_missing", why: P5_MISSING.into() },
    }
}

// ── words ────────────────────────────────────────────────────────────

/// The cards' computed words (`{invite}`, `{accept}`, `{link}`, `{effect}`).
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
                format!(
                    "When {to} accepts, the two agents can message each other: mail from {to} lands in {ws}'s terminal and wakes it. \
                     {ws} or a person can remove or block {to} any time. The request goes out when this server's Postal network connection is on; until then it waits here."
                ),
            );
        }
        PAIR_ACCEPT => {
            let f = pending_friend(ctx).ok();
            let peer = f.as_ref().map(|f| f.peer_address.clone()).unwrap_or_else(|| "an agent on Postal".into());
            let owner = f.as_ref().and_then(|f| f.owner_name.clone()).map(|o| format!(" (its owner: {o})")).unwrap_or_default();
            put("accept", format!("Let {peer}{owner} be friends with {ws} on Postal"));
            let mut effect = format!(
                "{peer} can then message {ws}: its mail lands in {ws}'s terminal with a [postal {peer} · …] label and wakes it. Deny rejects the request. {ws} or a person can remove or block {peer} later."
            );
            if let Some(note) = f.as_ref().and_then(|f| f.note.clone()).filter(|n| !n.trim().is_empty()) {
                effect.push_str(&format!(" Their note: \"{}\".", note.trim()));
            }
            if let Some(fp) = f.as_ref().and_then(|f| f.peer_fingerprint.clone()) {
                effect.push_str(&format!(" Their key: {fp}."));
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
    Applied::Done {
        message: format!(
            "Friend request to {to} recorded for {}. It goes out when this server's Postal network connection is on; {} postal friends shows it.",
            ctx.workspace_handle,
            cli_name!()
        ),
    }
}

fn apply_pair_accept(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    if let Err(refused) = route_floor(postal_routes::DECIDE, who) {
        return refused;
    }
    let Some(id) = text_param(ctx, "id") else {
        return Applied::Refused { code: "bad_params".into(), message: "id= is the request".into() };
    };
    let r = postal_routes::decide_as(&postal_routes::who_of(who), &ctx.workspace_id, id, true);
    if !r.status.starts_with("200") {
        return refused_of(r);
    }
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    let peer = v.pointer("/friend/peer").and_then(Value::as_str).unwrap_or("them");
    Applied::Done { message: format!("{peer} and {} are Postal friends now.", ctx.workspace_handle) }
}

/// Deny on an incoming request rejects it, as the person, through the
/// decide route's rules. Decided elsewhere already: left as it is.
pub(crate) fn on_denied(req: &PermissionRequest, who: &Answerer) {
    if req.action != PAIR_ACCEPT {
        return;
    }
    let ctx = present::ctx_of(req);
    let Some(id) = text_param(&ctx, "id") else { return };
    let r = postal_routes::decide_as(&postal_routes::who_of(who), &ctx.workspace_id, id, false);
    if !r.status.starts_with("200") {
        k2_core::log_debug!("[postal] reject {id} from its card: {} {}", r.status, r.body);
    }
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
    let mut child = std::process::Command::new(p5)
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

/// Put one card up on the workspace's pinned Thread. Answers `{card,
/// request, addr}`, `{card, request, waiting: true}` when one is open
/// already, or `{card: null, why}` when it can't go up.
fn present(ws: &Ws, action: &str, params: Map<String, Value>, why: &str, held_ref: Option<String>) -> Value {
    let Some(reg) = present::find_action(action) else {
        return json!({ "card": null, "why": "not registered" });
    };
    let hash = params_hash(&params);
    if let Some(open) = open_for(&ws.id, action, held_ref.as_deref()).into_iter().find(|r| held_ref.is_some() || r.params_hash == hash) {
        return json!({ "card": open.card_id, "request": open.display_id(), "waiting": true, "addr": open.addr });
    }
    let Some(conversation_id) = crate::overlay_routes::pinned_chat_of(&ws.id) else {
        return json!({ "card": null, "why": "no Thread" });
    };
    let handle = present::workspace_handle(&ws.id);
    let ctx = Ctx { workspace_id: ws.id.clone(), workspace_handle: handle.clone(), params: params.clone() };
    if let Check::Moved(why) | Check::Unavailable { why, .. } = (reg.check)(&ctx) {
        return json!({ "card": null, "why": why });
    }
    let at = k2_core::overlay::now_secs();
    let req = PermissionRequest {
        id: RequestId::new(),
        workspace_id: ws.id.clone(),
        session_id: Some(K2_SESSION.to_string()),
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
    let reqs = std::slice::from_ref(&req);
    let (prompt, options) = present::prompt_and_options(reqs, false);
    let opts: Vec<&str> = options.iter().map(String::as_str).collect();
    let card_action = present::card_action(reqs, false, present::single_dedupe(&req));
    let posted = crate::overlay_routes::present_action_card(
        Some(&conversation_id),
        &ws.id,
        &prompt,
        &opts,
        card_action,
        false,
        req.expires_at,
        crate::overlay_routes::CardAudience::Logins(present::eligible_logins(reg.spec.floor)),
    );
    let (card_id, addr) = match posted {
        Ok(crate::overlay_routes::Presented::Card { id, addr }) => (id, addr),
        other => {
            // Never strand a request without its card.
            if let Err(e) = store::cas_open_to(req.id.as_str(), RequestState::Cancelled) {
                k2_core::log_debug!("[postal] withdraw {}: {e}", req.id);
            }
            return json!({ "card": null, "why": format!("{other:?}") });
        }
    };
    if let Err(e) = store::set_card(req.id.as_str(), &conversation_id, &card_id, None, req.expires_at) {
        k2_core::log_debug!("[postal] set card {card_id}: {e}");
    }
    if let Ok(Some(stored)) = store::get(req.id.as_str()) {
        answer::audit(k2_core::permission::audit_event::REQUEST, "k2", "daemon", &stored);
    }
    json!({ "card": card_id, "request": req.display_id(), "addr": addr })
}

fn asked(v: Value, what: &str) -> CliResponse {
    if v["card"].is_null() {
        let why = v["why"].as_str().unwrap_or("the card couldn't go up").to_string();
        return postal_routes::err("409 Conflict", "card_not_presented", format!("{what}: {why}"));
    }
    CliResponse::ok_json(
        json!({
            "ok": true,
            "asked": true,
            "card": v["card"],
            "request": v["request"],
            "addr": v["addr"],
            "waiting": v["waiting"].as_bool().unwrap_or(false),
            "hint": "a person answers the card on your pinned Thread; the answer comes back to you as a [permission:] turn",
        })
        .to_string(),
    )
}

/// An agent's invite: K2 presents `postal.invite` with the agent's why.
pub(crate) fn ask_invite(ws: &Ws, to: &str, why: &str) -> CliResponse {
    let mut params = Map::new();
    params.insert("to".into(), json!(to));
    params.insert("note".into(), json!(why));
    asked(present(ws, INVITE, params, why, None), "the friend request card")
}

/// `k2 postal link-p5`: K2 presents `postal.link-p5` (the Owner's).
pub(crate) fn ask_link_p5(who: &Who, ws: &Ws) -> CliResponse {
    if p5_program().is_none() {
        return postal_routes::err("409 Conflict", "p5_missing", P5_MISSING);
    }
    let why = match who.passport() {
        Some(_) => format!("{} wants Postal mail that p5 receives on this machine delivered to it.", ws.handle),
        None => format!("{} asked to link p5 to {}.", who.label(), ws.handle),
    };
    asked(present(ws, LINK_P5, Map::new(), &why, None), "the p5 link card")
}

/// K2's card for an incoming request (`held_ref` = the friend row).
pub(crate) fn present_pair_accept(ws: &Ws, f: &k2_core::postal::store::Friend) -> Value {
    let mut params = Map::new();
    params.insert("id".into(), json!(f.id));
    let why = format!("{} asked to be friends on Postal. A person here accepts or rejects it.", f.peer_address);
    present(ws, PAIR_ACCEPT, params, &why, Some(f.id.clone()))
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
        "accepted" => (RequestState::Approved, label::ALLOW),
        "rejected" => (RequestState::Denied, label::DENY),
        _ => {
            answer::close_open(&open, RequestState::Cancelled, &format!("Closed: the request was {how} somewhere else."));
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

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &INVITE_SPEC, check: check_invite, apply: apply_invite },
        Registered { spec: &PAIR_ACCEPT_SPEC, check: check_pair_accept, apply: apply_pair_accept },
        Registered { spec: &LINK_P5_SPEC, check: check_link_p5, apply: apply_link_p5 },
    ]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "postal_tests.rs"]
mod tests;
