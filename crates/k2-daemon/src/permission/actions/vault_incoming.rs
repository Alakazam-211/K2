//! `vault.incoming.accept id= [as=] [replace=yes]` (0.46.10, Rosson
//! 2026-11-07: "when a person must act, the agent presents a one-tap card on
//! the normal permission-card path, desktop Thread and phone, never 'go type
//! a command' or 'go find it in Settings'").
//!
//! Another agent sent this workspace a secret (`k2 vault send NAME --to
//! <agent>`, delivery `hd_…`) and it waits for a person here
//! (prd-agent-secret-handoff-v1 §4.5). Until 0.46.10 that person was told to
//! run `k2 vault incoming accept` from their own terminal, find it in
//! Settings → Vault → Incoming, or answer a bespoke `handoff` card the phone
//! never drew.
//!
//! **It is a permission card now.** When a delivery lands pending, K2
//! presents this card itself, Beta or not ([`present_for`], called from
//! `vault_handoff_routes::after_receive`), on the receiving agent's pinned
//! Thread, the way `apps::held_cards` presents a held App request. So it
//! carries the standard `cards=2` view (`permission`, Allow / Deny, click
//! proof), the targeted push and the Tickets item, and the phone draws it
//! with no new UI. The old `handoff` card is no longer posted; the ones
//! already on Threads keep rendering and close as before (`rec.card`).
//!
//! - **Allow** accepts it ([`apply`]): `POST /cli/vault/incoming/decide`'s
//!   own entry (`vault_handoff_routes::handle`), as the person who clicked,
//!   with `as` / `replace` from the stored values. The route's own checks run
//!   again at the click (person only, a wider Replace needs an Owner or
//!   Admin, the name not taken).
//! - **Deny** declines it ([`on_denied`]): the same entry, `decline`.
//! - **A name clash** (the workspace already has a secret of that name): K2's
//!   card asks to accept it under a free name (`as=NAME_2`); a new version
//!   from the same sender asks to replace it (`replace=yes`). The agent may
//!   ask for another name itself: `k2 permission request vault.incoming.accept
//!   id=hd_… as=OTHER --why "…"` (Beta on).
//! - **Who answers:** the owner token, an Owner or an Admin login. The
//!   decide route also lets a Member keep a secret for a workspace; the
//!   card is narrower on purpose (Rosson 2026-11-07: owner or Admin).
//! - **Who asks:** K2 itself, or the receiving agent for its OWN pending
//!   deliveries only. Anything else is refused at request time in the same
//!   words as an id that doesn't exist.
//! - **One door, many cards.** Whatever decides the delivery (this card,
//!   another card for it, Settings, `k2 vault incoming accept|decline`, the
//!   sender's unsend, the expiry tick) closes every open card for it
//!   ([`close_for_delivery`]), so no card is left waiting on a delivery that
//!   is gone.
//! - **Values:** names and the delivery id only, never a value.

use std::collections::HashMap;

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamKind, ParamSpec, Proof, SettingsRef, Undo, WallCoverage,
    Words,
};
use k2_core::permission::label;
use k2_core::permission::request::{params_hash, PermissionRequest, RequestId, RequestState};
use k2_vault::handoff::HandoffRecord;
use k2_vault::{Actor, Scope, SecretStore, Selector};
use serde_json::{json, Map, Value};

use super::held::{error_of, route_floor, text_param};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::permission::{answer, limits, present, store};

/// The card's id.
pub(crate) const ACTION: &str = "vault.incoming.accept";
/// `k2 vault incoming accept|decline` / Settings → Vault → Incoming.
pub(crate) const ROUTE: &str = crate::vault_handoff_routes::DECIDE;
/// The session K2's own handoff cards carry (never batched with an agent's
/// own Beta asks).
pub(crate) const K2_SESSION: &str = "k2:vault-handoff";
/// The ingress the card's apply runs the decide entry with: the decide
/// route sends no "[k2 vault] accepted" knock for it, because the agent's
/// `[permission:]` turn already carries the same words.
pub(crate) const INGRESS: &str = "permission-card";
const REPLACE_CHOICES: [&str; 1] = ["yes"];

pub(crate) static ACCEPT: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Vault,
    params: &[
        ParamSpec { key: "id", kind: ParamKind::HeldRequest, required: true, list: false, choices: &[] },
        ParamSpec { key: "as", kind: ParamKind::Secret, required: false, list: false, choices: &[] },
        ParamSpec { key: "replace", kind: ParamKind::Choice, required: false, list: false, choices: &REPLACE_CHOICES },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    // `{accept}` and `{effect}` are K2's own words from the stored delivery
    // ([`with_computed`]): the secret's name and kind, the sender (agent
    // and server), the note, a name clash, how long the copy is kept, and
    // when the delivery expires. Never a value.
    words: Words {
        setting: "Accept a secret sent to {ws}",
        what_changes: "{accept}",
        consequences: "{effect}",
        scope: "delivery {id} to {ws}",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "vault", path: "Settings → Vault → Incoming" },
    // No wall: an agent never reaches the decide route (it is DENY for
    // passports), so nothing refuses it there to teach from. K2 presents
    // the card itself; the agent may also ask for it (`k2 permission list`).
    walls: &[],
    coverage: WallCoverage::None,
    machine_hints: &[],
};

/// The exact line an agent asks with (Beta on), for its texts.
pub(crate) fn ask_line(delivery_id: &str) -> String {
    format!("k2 permission request {ACTION} id={delivery_id} --why \"<what you need it for>\"")
}

fn id_of(ctx: &Ctx) -> Result<&str, String> {
    text_param(ctx, "id").ok_or_else(|| "name the delivery: id=hd_… (k2 vault incoming)".to_string())
}

fn replace_of(ctx: &Ctx) -> bool {
    text_param(ctx, "replace") == Some("yes")
}

fn not_yours(ctx: &Ctx, id: &str) -> String {
    format!("{} has no pending delivery {id} (k2 vault incoming lists them)", ctx.workspace_handle)
}

/// The delivery `id`, received by the asking workspace (any status).
fn delivery(vault: &k2_vault::LocalVault, ctx: &Ctx, id: &str) -> Option<HandoffRecord> {
    vault.handoff_get(id, "in").ok().filter(|r| r.local_workspace_id == ctx.workspace_id)
}

/// The asking workspace has a secret named `name`.
fn has_secret(vault: &k2_vault::LocalVault, workspace_id: &str, name: &str) -> bool {
    vault
        .meta(&Selector::user(Scope::workspace(workspace_id.to_string()), name), &Actor::System("permission-card".into()))
        .is_ok()
}

/// The sender, in words: `sales (on this server)` or `sales on rosson.k2.dev`.
fn from_words(peer: &str) -> String {
    match peer.split_once("::") {
        Some((agent, host)) => format!("{agent} on {host}"),
        None => format!("{peer} (on this server)"),
    }
}

/// `3 days`, `12 hours`, `45 minutes`.
fn span_words(secs: i64) -> String {
    let s = secs.max(0);
    let (n, unit) = if s % 86_400 == 0 && s >= 86_400 {
        (s / 86_400, "day")
    } else if s % 3600 == 0 && s >= 3600 {
        (s / 3600, "hour")
    } else if s >= 3600 {
        return format!("{:.1} hours", s as f64 / 3600.0);
    } else {
        ((s + 59) / 60, "minute")
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// Still wanted: a pending delivery to the asking workspace, and a name it
/// can land under (a clash needs `as=` or `replace=yes`).
fn check(ctx: &Ctx) -> Check {
    let id = match id_of(ctx) {
        Ok(i) => i,
        Err(e) => return Check::Moved(e),
    };
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return Check::Unavailable { code: "vault_not_open", why: e.hint },
    };
    let Some(rec) = delivery(&vault, ctx, id) else {
        return Check::Moved(not_yours(ctx, id));
    };
    if rec.status != "pending" {
        return Check::Moved(format!("delivery {id} is {} already", rec.status));
    }
    let replace = replace_of(ctx);
    match text_param(ctx, "as") {
        Some(_) if replace => Check::Moved("as= and replace=yes don't go together: a new name, or a new version of the current one".into()),
        Some(name) => {
            if let Err(e) = k2_vault::model::validate_name(name, true) {
                return Check::Moved(e.hint);
            }
            if has_secret(&vault, &ctx.workspace_id, name) {
                return Check::Moved(format!("{} already has {name}: ask with another as=", ctx.workspace_handle));
            }
            Check::Needed
        }
        None => {
            let name = rec.item.landed_name();
            let taken = has_secret(&vault, &ctx.workspace_id, name);
            match (taken, replace) {
                (true, false) => Check::Moved(format!(
                    "{} already has {name}: ask with as=<another name>, or replace=yes for a new version of it",
                    ctx.workspace_handle
                )),
                (false, true) => Check::Moved(format!("{} has no {name} to replace: ask without replace=", ctx.workspace_handle)),
                _ => Check::Needed,
            }
        }
    }
}

/// The card's words from the stored delivery (`{accept}`, `{effect}`).
/// Everything here is fixed while the delivery waits, so the card's text
/// hash holds until the click.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ACTION {
        return None;
    }
    let mut out = ctx.clone();
    let id = text_param(ctx, "id").unwrap_or("");
    let ws = &ctx.workspace_handle;
    // X15: no id is the catalog's own view (`k2 permission list`), not a
    // card: say what the action does. An id K2 can't read stays as it was.
    if id.trim().is_empty() {
        out.params.insert("accept".into(), json!(format!("Accept a secret another agent sent into {ws}'s vault")));
        out.params.insert("effect".into(), json!("Each card names the secret, who sent it and how it lands; nothing is applied until a person allows that card."));
        return Some(out);
    }
    let rec = crate::vault::get().ok().and_then(|v| delivery(&v, ctx, id));
    let Some(rec) = rec else {
        out.params.insert("accept".into(), json!(format!("Accept delivery {id} into {ws}'s vault")));
        out.params.insert("effect".into(), json!(format!("K2 can't read delivery {id} now; nothing is applied until it can.")));
        return Some(out);
    };
    let from = from_words(&rec.peer_addr);
    let lands = rec.item.landed_name().to_string();
    let replace = replace_of(ctx);
    let landing = match text_param(ctx, "as") {
        Some(a) => format!(" as {a}"),
        None if replace => format!(", replacing its current {lands} with this new version"),
        None if lands != rec.name => format!(" as {lands}"),
        None => String::new(),
    };
    let accept = format!("Accept {} ({}) from {from} into {ws}'s vault{landing}", rec.name, rec.kind);
    let (d, c) = rec.item.landing_delivery();
    let sites = if rec.item.hosts.is_empty() { String::new() } else { format!(" for {}", rec.item.hosts.join(", ")) };
    let mut effect = format!("{ws} gets it delivered as {d}{sites}, and may take it up to {c} itself. ");
    if let Some(t) = rec.item.ttl() {
        effect.push_str(&format!("Its copy is kept for {}, counted from when it was sent. ", span_words(t)));
    }
    match rec.reason.as_deref() {
        Some("clash") => effect.push_str(&format!("{ws} already has a secret named {lands}; that one stays as it is. ")),
        Some("resend") => effect.push_str(&format!("{ws} already has {lands} from {from}: this is a new version of it. ")),
        _ => {}
    }
    if !rec.note.trim().is_empty() {
        effect.push_str(&format!("The sender's note: \"{}\". ", rec.note.trim()));
    }
    effect.push_str(&format!("Deny declines it: the copy is deleted and {} is told. ", rec.peer_addr));
    if let Some(x) = rec.expires_at.as_deref() {
        effect.push_str(&format!("Unanswered, the delivery expires at {x}."));
    }
    out.params.insert("accept".into(), json!(accept));
    out.params.insert("effect".into(), json!(effect.trim_end()));
    Some(out)
}

/// The decide route's body for this answer.
fn decide_body(ctx: &Ctx, id: &str, action: &str) -> Value {
    let mut body = json!({ "id": id, "action": action });
    if action == "accept" {
        if let Some(a) = text_param(ctx, "as") {
            body["as"] = json!(a);
        }
        if replace_of(ctx) {
            body["replace"] = json!(true);
        }
    }
    body
}

/// Through `/cli/vault/incoming/decide`'s own entry, as the person who
/// clicked.
fn apply(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let id = match id_of(ctx) {
        Ok(i) => i.to_string(),
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    if let Err(refused) = route_floor(ROUTE, who) {
        return refused;
    }
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return Applied::Refused { code: e.code.into(), message: e.hint },
    };
    let body = decide_body(ctx, &id, "accept");
    let r = crate::vault_handoff_routes::handle(&vault, ROUTE, &super::vault::caller_of(who), body.to_string().as_bytes(), &HashMap::new(), INGRESS);
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if !r.status.starts_with("200") {
        let (code, message) = error_of(&v, "vault_refused");
        return Applied::Refused { code, message };
    }
    let d = &v["delivery"];
    let name = d["landedName"].as_str().or_else(|| d["name"].as_str()).unwrap_or("it");
    let s = &v["secret"];
    let mut msg = format!("{name} is in {}'s vault now", ctx.workspace_handle);
    if let Some(r) = v["ref"].as_str() {
        msg.push_str(&format!(" ({r})"));
    }
    if let Some(dl) = s["delivery"].as_str() {
        msg.push_str(&format!(", delivered as {dl}"));
    }
    if let Some(x) = s["expiresAt"].as_str() {
        msg.push_str(&format!(" until {x}"));
    }
    msg.push('.');
    if let Some(u) = s["use"].as_str() {
        msg.push_str(&format!(" Use: {u}."));
    }
    if let Some(more) = v["more"].as_str() {
        msg.push_str(&format!(" {more}"));
    }
    Applied::Done { message: msg }
}

/// The card's Deny declines the delivery itself, as the person, through the
/// same entry (`decline`). Already decided elsewhere is left as it is.
pub(crate) fn on_denied(req: &PermissionRequest, who: &Answerer) {
    if req.action != ACTION {
        return;
    }
    let ctx = present::ctx_of(req);
    let Ok(id) = id_of(&ctx) else { return };
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => {
            k2_core::log_debug!("[vault-handoff] decline {id} from its card: {}", e.code);
            return;
        }
    };
    let body = decide_body(&ctx, id, "decline");
    let r = crate::vault_handoff_routes::handle(&vault, ROUTE, &super::vault::caller_of(who), body.to_string().as_bytes(), &HashMap::new(), INGRESS);
    if !r.status.starts_with("200") {
        k2_core::log_debug!("[vault-handoff] decline {id} from its card: {} {}", r.status, r.body);
    }
}

// ── K2 presents it ──────────────────────────────────────────────────

/// The open card requests for one delivery (any session: K2's or the
/// agent's own asks), oldest first.
pub(crate) fn open_for(workspace_id: &str, delivery_id: &str) -> Vec<PermissionRequest> {
    store::open(Some(workspace_id))
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.action == ACTION && r.held_ref.as_deref() == Some(delivery_id))
        .collect()
}

/// A free name for a clash: `NAME_2`, `NAME_3`, … (None past 99, or when
/// the vault can't say).
fn free_name(vault: &k2_vault::LocalVault, workspace_id: &str, name: &str) -> Option<String> {
    (2..100).map(|n| format!("{name}_{n}")).find(|c| k2_vault::model::validate_name(c, true).is_ok() && !has_secret(vault, workspace_id, c))
}

/// The values K2's own card asks with: the delivery, and for a clash a free
/// name (another sender's secret of that name) or Replace (a new version
/// from the same sender).
fn k2_params(vault: &k2_vault::LocalVault, rec: &HandoffRecord) -> Map<String, Value> {
    let mut params = Map::new();
    params.insert("id".into(), json!(rec.delivery_id));
    let lands = rec.item.landed_name();
    if has_secret(vault, &rec.local_workspace_id, lands) {
        if rec.reason.as_deref() == Some("resend") {
            params.insert("replace".into(), json!("yes"));
        } else if let Some(n) = free_name(vault, &rec.local_workspace_id, lands) {
            params.insert("as".into(), json!(n));
        }
    }
    params
}

/// The card's "<agent> says:" line: K2's words (the agent didn't write it).
fn k2_why(rec: &HandoffRecord) -> String {
    let w = format!("{} sent me {}. A person here accepts or declines it.", rec.peer_addr, rec.name);
    w.chars().take(k2_core::permission::request::limits::WHY_MAX_CHARS).collect()
}

/// Present K2's card for a pending delivery `rec` (its receiving
/// workspace's pinned Thread). Answers:
/// - `{card, request, addr}`: a new card;
/// - `{card, request, waiting: true}`: one is open for it already;
/// - `{card: null, why}`: not presented (not pending, no Thread, the vault
///   or the card store said no): the caller falls back to the old card.
pub(crate) fn present_for(rec: &HandoffRecord) -> Value {
    if rec.direction != "in" || rec.status != "pending" {
        return json!({ "card": null, "why": "not pending" });
    }
    let ws = rec.local_workspace_id.as_str();
    if let Some(open) = open_for(ws, &rec.delivery_id).into_iter().next() {
        return json!({ "card": open.card_id, "request": open.display_id(), "waiting": true, "addr": open.addr });
    }
    let Some(reg) = present::find_action(ACTION) else {
        return json!({ "card": null, "why": "not registered" });
    };
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return json!({ "card": null, "why": e.hint }),
    };
    let Some(conversation_id) = crate::overlay_routes::pinned_chat_of(ws) else {
        return json!({ "card": null, "why": "no Thread" });
    };
    let handle = present::workspace_handle(ws);
    let params = k2_params(&vault, rec);
    let ctx = Ctx { workspace_id: ws.to_string(), workspace_handle: handle.clone(), params: params.clone() };
    if let Check::Moved(why) | Check::Unavailable { why, .. } = (reg.check)(&ctx) {
        return json!({ "card": null, "why": why });
    }
    let hash = params_hash(&params);
    let at = k2_core::overlay::now_secs();
    let until = rec
        .expires_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp())
        .unwrap_or(at + reg.spec.expiry.default_secs);
    let req = PermissionRequest {
        id: RequestId::new(),
        workspace_id: ws.to_string(),
        session_id: Some(K2_SESSION.to_string()),
        conversation_id: None,
        addr: Some(handle.clone()),
        card_id: None,
        action: ACTION.to_string(),
        params: Value::Object(params),
        params_hash: hash.clone(),
        text_hash: present::text_hash_of(reg.spec, &ctx),
        why: k2_why(rec),
        state: RequestState::Pending,
        floor: present::floor_str(reg.spec.floor).to_string(),
        held_ref: Some(rec.delivery_id.clone()),
        batch_id: None,
        ask_number: limits::ask_number(ws, ACTION, &hash, at),
        asked_for: None,
        created_at: at,
        expires_at: until.clamp(at + 60, at + reg.spec.expiry.max_secs),
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
        ws,
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
                k2_core::log_debug!("[vault-handoff] withdraw {}: {e}", req.id);
            }
            return json!({ "card": null, "why": format!("{other:?}") });
        }
    };
    if let Err(e) = store::set_card(req.id.as_str(), &conversation_id, &card_id, None, req.expires_at) {
        k2_core::log_debug!("[vault-handoff] set card {card_id}: {e}");
    }
    if let Ok(Some(stored)) = store::get(req.id.as_str()) {
        answer::audit(k2_core::permission::audit_event::REQUEST, "k2", "daemon", &stored);
    }
    json!({ "card": card_id, "request": req.display_id(), "addr": addr })
}

/// What the delivery became, for [`close_for_delivery`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Closed {
    /// A person accepted it somewhere else (another card, Settings, the CLI).
    Accepted,
    /// A person declined it somewhere else.
    Declined,
    /// It expired unanswered.
    Expired,
    /// The sender took it back.
    Unsent,
}

/// Close every open card for `delivery_id` (receiving workspace
/// `workspace_id`): the delivery was decided, expired or taken back. A card
/// being answered right now (`applying`) is the one deciding, and is left
/// to its own answer. `by`: the person who decided (`owner` or a login).
pub(crate) fn close_for_delivery(workspace_id: &str, delivery_id: &str, how: Closed, by: Option<&str>) {
    let open = open_for(workspace_id, delivery_id);
    if open.is_empty() {
        return;
    }
    match how {
        Closed::Expired => {
            answer::close_open(&open, RequestState::Expired, "Expired unanswered: the delivery expired.");
        }
        Closed::Unsent => {
            answer::close_open(&open, RequestState::Cancelled, "Taken back by the sender: nothing to accept.");
        }
        Closed::Accepted | Closed::Declined => {
            let (state, lbl, msg) = match how {
                Closed::Accepted => (RequestState::Approved, label::ALLOW, "Accepted somewhere else (another card, Settings or k2 vault incoming)."),
                _ => (RequestState::Denied, label::DENY, "Declined somewhere else (another card, Settings or k2 vault incoming)."),
            };
            let who = by.unwrap_or("owner");
            let via = if who == "owner" { "desktop" } else { "web" };
            let author = if who == "owner" { k2_core::overlay::PostAuthor::owner() } else { k2_core::overlay::PostAuthor::user(who) };
            let now = k2_core::overlay::now_secs();
            for r in &open {
                match store::cas_open_to(r.id.as_str(), state) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(e) => {
                        k2_core::log_debug!("[vault-handoff] close {}: {e}", r.id);
                        continue;
                    }
                }
                let d = store::Decided { state, by: Some(who), via: Some(via), reason: None, message: Some(msg), at: now };
                if let Err(e) = store::decide(r.id.as_str(), &d) {
                    k2_core::log_debug!("[vault-handoff] close {}: {e}", r.id);
                }
                if let (Some(c), Some(k)) = (r.conversation_id.as_deref(), r.card_id.as_deref()) {
                    present::refresh_card(c, k, present::CardClose::Answer { label: lbl, by: Some(author.clone()) });
                }
                if let Ok(Some(done)) = store::get(r.id.as_str()) {
                    answer::audit(k2_core::permission::audit_event::ANSWER, "k2", "daemon", &done);
                }
            }
        }
    }
}

/// The open card for a delivery, for `k2 vault incoming`: `{request, addr,
/// card}` or null.
pub(crate) fn card_json(workspace_id: &str, delivery_id: &str) -> Value {
    match open_for(workspace_id, delivery_id).into_iter().next() {
        Some(r) => json!({ "request": r.display_id(), "addr": r.addr, "card": r.card_id, "expiresAt": r.expires_at }),
        None => Value::Null,
    }
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &ACCEPT, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "vault_incoming_tests.rs"]
mod tests;
