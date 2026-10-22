//! K2's own Garden cards: `garden.access` and `garden.delete`
//! (prd-garden-access-v1 §1.5, §1.5a, G16–G18). **Slice A4 owns this file**
//! (it replaces A2's stub of the same seam).
//!
//! **They are permission cards.** `garden.access garden=` ("Let Billing
//! edit Sales board?") and `garden.delete garden=` ("Billing asks to delete
//! Sales board") are catalog rows (`permission/actions/gardens.rs`). K2
//! presents them itself, Beta or not: [`present`] writes a
//! `permission_requests` row (0218) and posts it on the asking agent's
//! pinned Thread with the person through `overlay_routes::present_action_card`,
//! as one `permission` card. So both carry the standard `cards=2` view
//! (`view.v` 1, kind `permission`, the same states, Allow / Deny, click
//! proof, "Turn off again"), and the phone needs no new UI (Appendix C).
//! Deny is the PRD's "Not now" (access) and "Keep it" (delete):
//! [`on_denied`] stores it in A1's asks table (`ga::record_not_now`), and no
//! new card comes for 24 hours.
//!
//! **One door.** The card's Allow, its `garden.access-off` twin ("Turn off
//! again"), Settings → Gardens and `k2 zen garden access` all change the
//! list through A2's `zen_access_routes::list_change` (the GardenOwner
//! check, then A1's store); [`access_entry`] is that door for A2's route.
//! The delete card deletes through [`delete_entry`]: the GardenOwner check,
//! then today's `garden/delete` as the person (A1's G8 hook included). A
//! card is never wider than Settings (§6.4).
//!
//! **Asks.** At most one waiting card per (Garden, agent, kind): an open
//! request with the same values is the waiting card. The first refusal
//! presents a card; after that a new card comes only on an explicit ask
//! (`garden/new`, `k2 zen garden access <g> ask`, `k2 zen garden delete
//! <g>`), and never within 24 h of a Not now.
//!
//! **Off.** With `gardenAccess` off nothing here runs: [`present`] answers
//! `{card: null}`, and the three catalog rows' checks answer
//! `not_available`.

use std::collections::HashMap;

use k2_core::overlay;
use k2_core::permission::catalog::Floor;
use k2_core::permission::request::{params_hash, PermissionRequest, RequestId, RequestState};
use k2_core::zen::garden_access::{self as ga, Actor, AskKind, EventKind, On, Via};
use serde_json::{json, Map, Value as J};

use crate::cli_response::CliResponse;
use crate::permission::{answer, limits, present, store};
use crate::sidecar_routes::Caller;

/// The catalog actions K2's cards carry.
pub(crate) const ACCESS: &str = "garden.access";
pub(crate) const ACCESS_OFF: &str = "garden.access-off";
pub(crate) const DELETE: &str = "garden.delete";

/// The session K2's own requests carry, so an agent's own Beta asks never
/// join a K2 card as a batch (and K2's cards never join each other).
pub(crate) const K2_SESSION: &str = "k2:garden";

/// The words the checks give while the switch is off.
pub(crate) const OFF_WORDS: &str =
    "Garden access lists are off on this server; a person turns them on in Settings \u{2192} Gardens.";
/// `garden.keep-mine` and `garden.proposal.approve` with the switch on (G18).
pub(crate) const RETIRED_WORDS: &str = "No longer needed: Gardens use access lists now.";

/// `gardenAccess` is on for this server (A1's switch, default off).
pub(crate) fn garden_access_on() -> bool {
    ga::enabled()
}

// ── the seam A2 calls ───────────────────────────────────────────────

/// Which card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GardenAsk {
    Access,
    Delete,
}

impl GardenAsk {
    fn ask(self) -> AskKind {
        match self {
            GardenAsk::Access => AskKind::Access,
            GardenAsk::Delete => AskKind::Delete,
        }
    }

    fn action(self) -> &'static str {
        match self {
            GardenAsk::Access => ACCESS,
            GardenAsk::Delete => DELETE,
        }
    }
}

/// Present K2's card for (`kind`, workspace, Garden). Returns the value of
/// the answer's `card` field: `{"card": "<doc id>"|null, …}`. `made`: right
/// after `garden/new`. `explicit`: `garden/new`, `k2 zen garden access <g>
/// ask` and `k2 zen garden delete <g>` (a refusal is not explicit).
pub(crate) fn present(kind: GardenAsk, workspace_id: &str, garden_id: &str, made: bool, explicit: bool) -> J {
    present_with_reason(kind, workspace_id, garden_id, made, explicit, None)
}

/// [`present`] with the agent's own words (`k2 zen garden access <g> ask
/// --reason …`): shown as the card's `why`, one line, ≤ 200 characters,
/// never secret-shaped (else K2's words).
pub(crate) fn present_with_reason(
    kind: GardenAsk,
    workspace_id: &str,
    garden_id: &str,
    made: bool,
    explicit: bool,
    reason: Option<&str>,
) -> J {
    present_detail(kind, workspace_id, garden_id, made, explicit, reason, RequestId::new())
}

/// [`present`] with the new request's id given (the golden samples use a
/// fixed one). The answers:
/// - `{card, request, addr}`: a new card;
/// - `{card, request, waiting: true}`: one is already waiting;
/// - `{card: null, notNowUntil}`: Not now / Keep it in the last 24 hours;
/// - `{card: null, asked: true}`: a refusal after an earlier ask (no card);
/// - `{card: null, note: true}`: no Thread; an owners-only inbox note;
/// - `{card: null, asked: false}`: the switch is off or the Garden is gone.
pub(crate) fn present_detail(
    kind: GardenAsk,
    ws: &str,
    garden_id: &str,
    made: bool,
    explicit: bool,
    reason: Option<&str>,
    id: RequestId,
) -> J {
    let gid = garden_id.trim();
    let gname = match (garden_access_on(), garden_name(gid)) {
        (true, Some(n)) => n,
        _ => return json!({ "card": null, "asked": false }),
    };
    let mut params = Map::new();
    params.insert("garden".to_string(), J::String(gid.to_string()));
    let hash = params_hash(&params);
    let action = kind.action();
    if let Ok(Some(open)) = store::open_same(ws, action, &hash) {
        return json!({ "card": open.card_id, "request": open.display_id(), "waiting": true });
    }
    let at = now();
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ga::ask_state(&conn, gid, ws, kind.ask()).unwrap_or_else(|e| {
            k2_core::log_debug!("[garden-cards] asks row for {gid}: {e}");
            None
        })
    };
    if let Some(t) = row.as_ref().and_then(|r| r.last_not_now_at).filter(|t| at < t + ga::NOT_NOW_QUIET_SECS) {
        return json!({ "card": null, "notNowUntil": t + ga::NOT_NOW_QUIET_SECS });
    }
    if !explicit && row.is_some() {
        return json!({ "card": null, "asked": true });
    }
    let handle = present::workspace_handle(ws);
    let text = prompt(kind, made, &handle, &gname);
    let Some(conversation_id) = crate::overlay_routes::pinned_chat_of(ws) else {
        return inbox_note(kind, ws, &handle, &gname, &text);
    };
    let Some(reg) = present::find_action(action) else {
        k2_core::log_debug!("[garden-cards] {action} is not registered");
        return json!({ "card": null, "asked": false });
    };
    let req = PermissionRequest {
        id,
        workspace_id: ws.to_string(),
        session_id: Some(K2_SESSION.to_string()),
        conversation_id: None,
        addr: Some(handle.clone()),
        card_id: None,
        action: action.to_string(),
        params: J::Object(params.clone()),
        params_hash: hash.clone(),
        text_hash: present::text_hash_of(
            reg.spec,
            &crate::permission::actions::Ctx { workspace_id: ws.to_string(), workspace_handle: handle.clone(), params },
        ),
        why: reason_ok(reason).unwrap_or_else(|| k2_why(kind, made, &gname)),
        state: RequestState::Pending,
        floor: present::floor_str(reg.spec.floor).to_string(),
        held_ref: None,
        batch_id: None,
        ask_number: limits::ask_number(ws, action, &hash, at),
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
        k2_core::log_debug!("[garden-cards] request for {gid}: {e}");
        return json!({ "card": null, "asked": false });
    }
    let reqs = std::slice::from_ref(&req);
    let card_action = present::card_action(reqs, false, present::single_dedupe(&req));
    let options = [k2_core::permission::label::ALLOW, k2_core::permission::label::DENY];
    let posted = crate::overlay_routes::present_action_card(
        Some(&conversation_id),
        ws,
        &text,
        &options,
        card_action,
        false,
        req.expires_at,
        crate::overlay_routes::CardAudience::Logins(present::eligible_logins(Floor::GardenOwner)),
    );
    let (card_id, addr) = match posted {
        Ok(crate::overlay_routes::Presented::Card { id, addr }) => (id, addr),
        other => {
            // Never strand a request without its card.
            if let Err(e) = store::cas_open_to(req.id.as_str(), RequestState::Cancelled) {
                k2_core::log_debug!("[garden-cards] withdraw {}: {e}", req.id);
            }
            k2_core::log_debug!("[garden-cards] card for {gid}: {other:?}");
            return match other {
                Ok(crate::overlay_routes::Presented::NoThread) => inbox_note(kind, ws, &handle, &gname, &text),
                _ => json!({ "card": null, "asked": false }),
            };
        }
    };
    if let Err(e) = store::set_card(req.id.as_str(), &conversation_id, &card_id, None, req.expires_at) {
        k2_core::log_debug!("[garden-cards] set card {card_id}: {e}");
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let event = match kind {
            GardenAsk::Access => EventKind::AccessAsked,
            GardenAsk::Delete => EventKind::DeleteAsked,
        };
        let detail = json!({ "workspace": ws, "cardId": card_id, "request": req.display_id(), "made": made });
        if let Err(e) = ga::record_ask(&conn, gid, ws, kind.ask(), &card_id, at)
            .and_then(|_| ga::record_event(&conn, On::Garden(gid), event, &Actor::Agent(ws.to_string()), &detail, at).map(|_| ()))
        {
            k2_core::log_debug!("[garden-cards] asks row for {gid}: {e}");
        }
    }
    if let Ok(Some(stored)) = store::get(req.id.as_str()) {
        answer::audit(k2_core::permission::audit_event::REQUEST, "k2", "daemon", &stored);
    }
    json!({ "card": card_id, "request": req.display_id(), "addr": addr })
}

/// **The list door for A2's routes** (`POST /cli/zen/garden/access`, `k2
/// zen garden access <g> add|remove`): A2's `list_change` as Settings
/// (`Via::Settings`). The card's Allow and its twin call `list_change`
/// with `Via::Card` / `Via::Twin` and the card (`permission/actions/gardens.rs`).
pub(crate) fn access_entry(caller: &Caller, garden: &str, agent_project_id: &str, allow: bool, ingress: &str) -> CliResponse {
    let _ = ingress;
    crate::zen_access_routes::list_change(caller, garden, agent_project_id, allow, Via::Settings, None)
}

/// The `garden.delete` card's apply (G16): the Garden's owner deletes it
/// through the Settings route's own `garden/delete`, as the person
/// (today's snapshot into history, A1's G8 mirror and `deleted` line). A
/// locked Garden deletes too (ruling 1). 409 `not_available` while off,
/// 403 `owner_only` below GardenOwner, 404 `unknown_garden` when gone.
pub(crate) fn delete_entry(caller: &Caller, garden: &str, ingress: &str) -> CliResponse {
    let refuse = |status: &'static str, code: &str, message: &str| CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": code, "message": message }).to_string(),
    };
    if !garden_access_on() {
        return refuse("409 Conflict", "not_available", OFF_WORDS);
    }
    if !crate::zen_routes::garden_owner_may(caller, Some(garden)) {
        return refuse("403 Forbidden", "owner_only", "Only you delete a Garden, in Settings \u{2192} Gardens or on its card.");
    }
    if garden_name(garden).is_none() {
        return refuse("404 Not Found", "unknown_garden", "That Garden is gone already.");
    }
    // The caller is the Garden's owner (checked above): the route runs with
    // the owner's rights, and History names who answered.
    let body = json!({ "garden": garden.trim() }).to_string();
    crate::zen_routes::handle("/cli/zen/garden/delete", true, caller, ingress, &HashMap::new(), body.as_bytes())
}

/// Who may ask for a delete (§1.5a): an agent on the Garden's list, or its
/// maker.
pub(crate) fn may_ask_delete(project_id: &str, garden: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    ga::is_listed(&conn, garden, project_id).unwrap_or(false)
        || ga::made(&conn, garden).ok().flatten().is_some_and(|m| m.made_by_project == project_id)
}

// ── words ───────────────────────────────────────────────────────────

/// The card's `why` when the agent gave none: K2's fixed words.
fn k2_why(kind: GardenAsk, made: bool, garden: &str) -> String {
    match (kind, made) {
        (GardenAsk::Access, true) => format!("I made {garden} and want to keep editing it."),
        (GardenAsk::Access, false) => format!("I'd like to edit {garden}."),
        (GardenAsk::Delete, _) => format!("I'd like {garden} deleted."),
    }
}

/// The card's prompt (K2's words, PRD §1.5, §1.5a): what older builds and
/// the inbox show; the `cards=2` view carries the catalog words.
pub(crate) fn prompt(kind: GardenAsk, made: bool, agent: &str, garden: &str) -> String {
    match (kind, made) {
        (GardenAsk::Access, true) => format!("{agent} made {garden} and wants to keep editing it. Allow?"),
        (GardenAsk::Access, false) => {
            format!("Let {agent} edit {garden}? {agent} can change its layout and widgets, and use their data.")
        }
        (GardenAsk::Delete, _) => format!("{agent} asks to delete {garden}. It leaves your Gardens. K2 keeps a copy in history."),
    }
}

/// An agent's own reason, when it gave one: one line, 1–200 characters,
/// never secret-shaped. Anything else is dropped (K2's words stand).
fn reason_ok(r: Option<&str>) -> Option<String> {
    let t = r?.trim();
    let ok = !t.is_empty()
        && t.chars().count() <= k2_core::permission::request::limits::WHY_MAX_CHARS
        && !t.chars().any(char::is_control)
        && !crate::permission::routes::secret_shaped(t);
    ok.then(|| t.to_string())
}

/// No Thread to put a card on: one owners-only inbox note (none while one
/// with the same title is open), pointing at Settings → Gardens.
fn inbox_note(kind: GardenAsk, ws: &str, agent: &str, garden: &str, text: &str) -> J {
    let title = match kind {
        GardenAsk::Delete => format!("{agent} asks to delete {garden}"),
        GardenAsk::Access => format!("Let {agent} edit {garden}?"),
    };
    let open = k2_core::feedback::list_for_project(ws, &k2_core::feedback::ListFilter::Open).unwrap_or_default();
    if !open.iter().any(|i| i.title == title) {
        let how = match kind {
            GardenAsk::Delete => format!("To delete it: Settings \u{2192} Gardens \u{2192} {garden} \u{2192} Delete."),
            GardenAsk::Access => format!("To allow it: Settings \u{2192} Gardens \u{2192} {garden} \u{2192} Add agent \u{2192} {agent}."),
        };
        let body = format!("{text}\n\n{how} To say no, do nothing.");
        if let Err(e) = crate::feedback_routes::file_owners_only_notice(ws, agent, &title, &body) {
            k2_core::log_debug!("[garden-cards] inbox note for {garden}: {e}");
        }
    }
    json!({ "card": null, "note": true })
}

// ── reads for the catalog checks and the words ──────────────────────

/// A Garden's current name by id (None when it is gone).
pub(crate) fn garden_name(id: &str) -> Option<String> {
    k2_core::zen::ZenFiles::local().gardens().into_iter().find(|g| g.id == id.trim()).map(|g| g.name)
}

/// The name a card shows for Garden `id`: its current name, else the name
/// it had when it was deleted (its `.history` record), so an answered
/// delete card keeps saying what it deleted. None: K2 never knew it.
pub(crate) fn shown_name(id: &str) -> Option<String> {
    if let Some(n) = garden_name(id) {
        return Some(n);
    }
    let files = k2_core::zen::ZenFiles::local();
    let rec = ga::mirror_path(&files, id.trim()).with_file_name(k2_core::zen::store::DELETED_RECORD);
    let text = std::fs::read_to_string(rec).ok()?;
    let v: J = serde_json::from_str(&text).ok()?;
    v.get("name").and_then(J::as_str).map(str::to_string)
}

/// The last Garden can't be deleted (409 `last_garden`).
pub(crate) fn is_last_garden() -> bool {
    k2_core::zen::ZenFiles::local().gardens().len() <= 1
}

pub(crate) fn is_listed(garden: &str, project: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    ga::is_listed(&conn, garden, project).unwrap_or_else(|e| {
        k2_core::log_debug!("[garden-cards] is_listed {garden}: {e}");
        false
    })
}

/// The card id the asks table holds for this (Garden, agent, kind).
pub(crate) fn asked_card(garden: &str, project: &str, kind: AskKind) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    ga::ask_state(&conn, garden, project, kind).ok().flatten().and_then(|r| r.card_id)
}

fn now() -> i64 {
    overlay::now_secs()
}

// ── Not now / Keep it ───────────────────────────────────────────────

/// The person denied a Garden card (Not now / Keep it): stored in the asks
/// table with a History line, so no new card comes for 24 hours. Called by
/// the permission core's Deny for every request (a no-op for other
/// actions).
pub(crate) fn on_denied(req: &PermissionRequest) {
    let kind = match req.action.as_str() {
        ACCESS => AskKind::Access,
        DELETE => AskKind::Delete,
        _ => return,
    };
    let Some(garden) = req.params.get("garden").and_then(J::as_str) else { return };
    let at = req.answered_at.unwrap_or_else(now);
    let actor = match req.answered_by.as_deref() {
        Some("owner") | None => Actor::Owner,
        Some(u) => Actor::Login(u.to_string()),
    };
    let event = match kind {
        AskKind::Access => EventKind::AccessNotNow,
        AskKind::Delete => EventKind::DeleteKept,
    };
    let detail = json!({ "workspace": req.workspace_id, "request": req.display_id() });
    let db = k2_core::db::shared();
    let conn = db.lock();
    let r = ga::record_not_now(&conn, garden, &req.workspace_id, kind, at)
        .and_then(|_| ga::record_event(&conn, On::Garden(garden), event, &actor, &detail, at).map(|_| ()));
    if let Err(e) = r {
        k2_core::log_debug!("[garden-cards] not now for {garden}: {e}");
    }
}

// ── the switch-on void (G18) ────────────────────────────────────────

/// Close what the old path left waiting when the switch turns on (G18):
/// every pending Keep-as-mine card (`zen.widget.keep`) is voided, and every
/// open `garden.keep-mine` / `garden.proposal.approve` permission request
/// is withdrawn with "No longer needed: Gardens use access lists now."
/// Headless: A1's switch flip calls it on every off→on. Returns how many
/// cards and requests it closed.
pub(crate) fn void_on_switch_on() -> usize {
    let now = now();
    let mut n = 0;
    // Keep-as-mine cards are posted on each workspace's pinned Chat
    // (`zen_agent_routes::keep_as_mine`), so those are the Threads to walk.
    let projects: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let ids = conn
            .prepare("SELECT id FROM projects")
            .and_then(|mut st| st.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>());
        ids.unwrap_or_else(|e| {
            k2_core::log_debug!("[garden-cards] workspaces: {e}");
            Vec::new()
        })
    };
    let mut waiting = Vec::new();
    for conv in projects.iter().filter_map(|p| crate::overlay_routes::pinned_chat_of(p)) {
        match overlay::waiting_cards(Some(&conv), now) {
            Ok(cards) => waiting.extend(cards),
            Err(e) => k2_core::log_debug!("[garden-cards] waiting cards on {conv}: {e}"),
        }
    }
    for (conv, card) in waiting {
        let keep = overlay::get_doc(&card)
            .ok()
            .flatten()
            .and_then(|d| d.choice.and_then(|c| c.action))
            .is_some_and(|a| a.id == crate::action_cards::KEEP_WIDGET);
        if !keep {
            continue;
        }
        match overlay::void_card(&conv, "", &card, None) {
            Ok(cb) => {
                crate::overlay_routes::publish_action_frame(&cb);
                n += 1;
            }
            Err(e) => k2_core::log_debug!("[garden-cards] void {card}: {e}"),
        }
    }
    match store::open(None) {
        Ok(reqs) => {
            let old: Vec<PermissionRequest> =
                reqs.into_iter().filter(|r| r.action == "garden.keep-mine" || r.action == "garden.proposal.approve").collect();
            n += answer::close_open(&old, RequestState::Cancelled, RETIRED_WORDS).len();
        }
        Err(e) => k2_core::log_debug!("[garden-cards] open requests: {e}"),
    }
    n
}

#[cfg(test)]
#[path = "zen_garden_cards_tests.rs"]
mod tests;
