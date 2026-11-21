//! K2 presents an agent's held App request as a card on the agent's
//! Thread (audit-app-agent-access-0464 §4 "one approval system"), Beta or
//! not: the `apps.approve` permission card (`permission/actions/held.rs`),
//! posted by K2 itself the way `zen_garden_cards` posts its Garden cards.
//! An owner or admin answers it on the Thread, the phone or Settings → App
//! Approvals; every door approves through `/cli/app/approve` with the same
//! fresh confirm (the owner's, or an Admin's own login password since
//! 0.46.13), so a card is never wider than Settings.
//!
//! Which requests: every agent's held App request (0.46.17, the owner's
//! standing rule: a person who must change something gets a one-tap card).
//! The App grants an agent can't apply itself (`super::agent_grants`) raise
//! it where they are filed; every other hold raises it at the one chokepoint
//! for held answers (`approvals::enrich_held`), Beta switch on or off. With
//! an agent's permission cards on, the Beta's own ask
//! (`k2 permission request apps.approve …`) is the card instead.
//!
//! **Who it goes to** (0.46.17). The agent may name the people: `--assign
//! <login>[,<login>…]` on the command that was held (`?assign=` on the
//! route, [`assign_param`]): each must be an owner or admin login on this
//! server, or `owner`, else the command is refused before anything runs
//! (`unknown_approver`, `approver_not_eligible`, `bad_approvers`). Without
//! it, the owner's default (`permissionApprovers`) or every owner and admin.
//! It narrows who is asked, never who may answer
//! (`crate::permission::approvers`).
//!
//! **One answer closes every door** (0.46.17). Deny on the card denies the
//! held request through `/cli/app/deny`, as the person ([`on_denied`]). A
//! request decided in Settings, on the CLI, by expiry or withdrawn by its
//! agent closes its open cards ([`close_decided`]), so no card stays
//! answerable for a request that no longer waits.
//!
//! **"Allow, and let it manage its Apps' users from now on"**
//! (`label::ALWAYS`). A card whose request "Let this agent manage its
//! Apps' users" would have covered (a test guest; the guests-rooms grant
//! in the agent's own room) also offers it: the approval, then that
//! switch through the Settings route, as the person ([`offers_always`],
//! [`turn_on_always`]). The card states what the switch covers
//! ([`SWITCH_COVERS`]). A plain Allow gives a usable test guest: the agent
//! sets its password once (`test_guests::password`).

use std::cell::RefCell;
use std::collections::HashMap;

use k2_core::app_grace::requests as reqs;
use k2_core::permission::request::{params_hash, PermissionRequest, RequestId, RequestState};
use serde_json::{json, Map, Value as J};

use super::routes::{no, Refusal};
use crate::permission::actions::{Answerer, Applied, Ctx};
use crate::permission::{answer, approvers, limits, present, store};

/// What "Let this agent manage its Apps' users" covers, in one line: the
/// card says it next to the option that turns it on, the switch's own
/// catalog entry (`apps.manage-users`) says it, and the desktop says it
/// word for word (`APP_USERS_SWITCH_COVERS` in the renderer; a test below
/// fails when they differ).
pub(crate) const SWITCH_COVERS: &str = "On its own Apps only: add and remove App users and test guests, set their passwords, \
     email and names, unlock them, give them roles and its own room, invite people, give its Apps the guests-rooms grant \
     in its own room, and mint App keys that manage its Apps' users. The users it makes are tied to it. Never a box login \
     or another workspace's guests.";

/// The catalog action K2's App cards carry.
pub(crate) const ACTION: &str = "apps.approve";
/// The session K2's own App cards carry (never batched with an agent's
/// own Beta asks).
pub(crate) const K2_SESSION: &str = "k2:apps";
/// The permission audit's actor for what the daemon itself does, and the
/// decider of a held request decided by no one (stored, read back by us).
const DAEMON_ACTOR: &str = "k2";

/// Present `r`'s card on the requesting agent's pinned Thread, for the
/// call's `--assign` ([`assigned`]) when it named people. Returns the
/// answer's `card` value: `{card, request, addr, routedTo}`, the same with
/// `waiting` when one waits already, or `{card: null}` (no Thread: Settings
/// → App Approvals still lists it).
pub(crate) fn present_for(r: &reqs::AppRequest) -> J {
    let Some(ws) = r.requester_workspace.as_deref() else { return json!({ "card": null }) };
    let mut params = Map::new();
    params.insert("request".to_string(), J::String(r.id.clone()));
    let hash = params_hash(&params);
    if let Ok(Some(open)) = store::open_same(ws, ACTION, &hash) {
        return json!({ "card": open.card_id, "request": open.display_id(), "waiting": true, "addr": open.addr, "routedTo": routed_to(&open) });
    }
    let Some(reg) = present::find_action(ACTION) else {
        k2_core::log_debug!("[app-cards] {ACTION} is not registered");
        return json!({ "card": null });
    };
    let handle = present::workspace_handle(ws);
    // The Thread of the session that asked (a sidecar's own, 0.46.6 client
    // papercut 12), else the pinned Chat.
    let own = r
        .requester_session
        .as_deref()
        .and_then(|s| crate::cell_identity::sidecar_thread_addr(ws, s))
        .and_then(|a| crate::overlay_routes::conversation_for_addr(&a).map(|c| (c, a)));
    let (conversation_id, thread_addr) = match own {
        Some(x) => x,
        None => match crate::overlay_routes::pinned_chat_of(ws) {
            Some(c) => (c, handle.clone()),
            None => return json!({ "card": null, "where": "Settings → App Approvals" }),
        },
    };
    let at = k2_core::overlay::now_secs();
    let ctx = Ctx { workspace_id: ws.to_string(), workspace_handle: handle.clone(), params: params.clone() };
    let req = PermissionRequest {
        id: RequestId::new(),
        workspace_id: ws.to_string(),
        session_id: Some(K2_SESSION.to_string()),
        conversation_id: None,
        addr: Some(thread_addr),
        card_id: None,
        action: ACTION.to_string(),
        params: J::Object(params),
        params_hash: hash.clone(),
        text_hash: present::text_hash_of(reg.spec, &ctx),
        why: why_of(r),
        state: RequestState::Pending,
        floor: present::floor_str(reg.spec.floor).to_string(),
        held_ref: Some(r.id.clone()),
        batch_id: None,
        ask_number: limits::ask_number(ws, ACTION, &hash, at),
        asked_for: None,
        created_at: at,
        expires_at: r.expires_at.min(at + reg.spec.expiry.max_secs),
        answered_at: None,
        answered_by: None,
        answered_via: None,
        reason: None,
        result_message: None,
        updated_why: None,
        updated_at: None,
    };
    if let Err(e) = store::insert(&req) {
        k2_core::log_debug!("[app-cards] request for {}: {e}", r.id);
        return json!({ "card": null });
    }
    // Who it asks: the agent's --assign (0.46.17), else the owner's default
    // (0.46.16), else everyone at the floor.
    if let Some(names) = assigned() {
        if let Err(e) = approvers::set(req.id.as_str(), &names) {
            k2_core::log_debug!("[app-cards] assign for {}: {e}", r.id);
            let _ = store::cas_open_to(req.id.as_str(), RequestState::Failed);
            return json!({ "card": null, "where": "Settings → App Approvals" });
        }
    }
    let audience = approvers::audience_for_new(&req);
    let (prompt, options) = present::prompt_and_options(std::slice::from_ref(&req), false);
    let opts: Vec<&str> = options.iter().map(String::as_str).collect();
    let card_action = present::card_action(std::slice::from_ref(&req), false, present::single_dedupe(&req));
    // A copy-host request's summary is a count (the agent's notes repeat
    // it); the owner's card names who, from the request's first line.
    let detail = match r.body.get("route").and_then(J::as_str) {
        Some(super::agent_grants::COPY_HOST_APPLY) => r.diff.pointer("/lines/0").and_then(J::as_str).unwrap_or(&r.summary),
        _ => r.summary.as_str(),
    };
    let mut text = format!("{prompt}\n{detail}");
    if options.iter().any(|o| o == k2_core::permission::label::ALWAYS) {
        text.push_str(&format!(
            "\n\"{}\" also turns on \"Let this agent manage its Apps' users\" for {handle}: {SWITCH_COVERS}",
            k2_core::permission::label::ALWAYS
        ));
    }
    let posted = crate::overlay_routes::present_action_card(
        Some(&conversation_id),
        ws,
        &text,
        &opts,
        card_action,
        false,
        req.expires_at,
        // The catalog's floor: an Owner or Admin login (0.46.13).
        crate::overlay_routes::CardAudience::Logins(audience),
    );
    let (card_id, addr) = match posted {
        Ok(crate::overlay_routes::Presented::Card { id, addr }) => (id, addr),
        other => {
            if let Err(e) = store::cas_open_to(req.id.as_str(), RequestState::Cancelled) {
                k2_core::log_debug!("[app-cards] withdraw {}: {e}", req.id);
            }
            k2_core::log_debug!("[app-cards] card for {}: {other:?}", r.id);
            return json!({ "card": null, "where": "Settings → App Approvals" });
        }
    };
    if let Err(e) = store::set_card(req.id.as_str(), &conversation_id, &card_id, None, req.expires_at) {
        k2_core::log_debug!("[app-cards] set card {card_id}: {e}");
    }
    if let Ok(Some(stored)) = store::get(req.id.as_str()) {
        answer::audit(k2_core::permission::audit_event::REQUEST, DAEMON_ACTOR, "daemon", &stored);
    }
    let routed = store::get(req.id.as_str()).ok().flatten().map(|q| routed_to(&q)).unwrap_or(J::Null);
    json!({ "card": card_id, "request": r.id, "addr": addr, "routedTo": routed })
}

// ── who the card asks (0.46.17) ─────────────────────────────────────

thread_local! {
    /// The `--assign` of the call being served (set by [`with_assign`]).
    static ASSIGN: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// The query key the CLI sends `--assign` as.
pub(crate) const ASSIGN_PARAM: &str = "assign";

/// `?assign=<login>[,<login>…]` on a call that may be held, checked before
/// the call runs: each name an owner or admin login on this server (one
/// who may answer `apps.approve`), or `owner`. Absent or blank: `None`.
pub(crate) fn assign_param(params: &HashMap<String, String>) -> Result<Option<Vec<String>>, Refusal> {
    let Some(raw) = params.get(ASSIGN_PARAM) else { return Ok(None) };
    let floor = present::find_action(ACTION).map(|r| r.spec.floor).unwrap_or(k2_core::permission::catalog::Floor::Admin);
    let refused = |r: approvers::Refused| no(r.code, 400, assign_words(&r.message));
    match approvers::parse(Some(&J::String(raw.clone()))).map_err(refused)? {
        None => Ok(None),
        Some(names) => approvers::check_named(&names, floor).map(Some).map_err(refused),
    }
}

/// The approvers module's words, for `--assign` (they speak of `--to`).
fn assign_words(m: &str) -> String {
    m.replace("--to ada,owner", "--assign ada,owner")
        .replace("--to takes", "--assign takes")
        .replace("--to names", "--assign names")
        .replace(" (a dry run lists who may answer: --dry-run)", &format!(" (the logins: {} connections list --users)", k2_core::brand::CLI_NAME))
}

/// Run `f` with `names` as the call's `--assign` (put back after, even on
/// a panic), so the card a hold raises inside it goes to them.
pub(crate) fn with_assign<T>(names: Option<Vec<String>>, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Vec<String>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let prior = self.0.take();
            ASSIGN.with(|a| *a.borrow_mut() = prior);
        }
    }
    let prior = ASSIGN.with(|a| a.replace(names));
    let _restore = Restore(prior);
    f()
}

/// The `--assign` of the call being served, if any.
pub(crate) fn assigned() -> Option<Vec<String>> {
    ASSIGN.with(|a| a.borrow().clone())
}

/// `routedTo` of one `apps.approve` request: who it asks.
fn routed_to(req: &PermissionRequest) -> J {
    approvers::routed_json(present::floor_of(&req.floor), approvers::of(req.id.as_str()).as_deref())
}

/// The newest `apps.approve` request for held request `id` (the card, open
/// or answered), as `k2 app approvals` shows it: `{id, state, card,
/// thread, routedTo, narrowed}` (`narrowed`: it asks named people, the
/// agent's `--assign` or the owner's default, not every owner and admin).
pub(crate) fn card_json(id: &str) -> Option<J> {
    let q = store::get_by_display_id(id).ok().flatten().filter(|q| q.action == ACTION)?;
    Some(json!({
        "id": q.display_id(),
        "state": q.state.as_str(),
        "card": q.card_id,
        "thread": q.addr,
        "routedTo": routed_to(&q),
        "narrowed": approvers::of(q.id.as_str()).is_some(),
    }))
}

// ── one answer closes every door (0.46.17) ──────────────────────────

/// The card's Deny denies the held request itself, through `/cli/app/deny`
/// as the person (the route's own checks: an owner or admin denies). A
/// request decided meanwhile is left as it is.
pub(crate) fn on_denied(req: &PermissionRequest, who: &Answerer, why: Option<&str>) {
    if req.action != ACTION {
        return;
    }
    let Some(id) = req.params.get("request").and_then(J::as_str) else { return };
    let mut body = json!({ "request": id });
    if let Some(w) = why.filter(|w| !w.trim().is_empty()) {
        body["reason"] = json!(w);
    }
    let caller = crate::permission::actions::held::who_of(who);
    if let Err(e) = super::approvals::handle("/cli/app/deny", &caller, body.to_string().as_bytes(), &HashMap::new()) {
        k2_core::log_debug!("[app-cards] deny of {id} from its card: {} {}", e.code, e.hint);
    }
}

/// Held request `r` was decided outside its card (Settings, the CLI, its
/// seven days ran out, its agent withdrew it): each open `apps.approve`
/// request for it closes as `r` did, and its card is redrawn. A request a
/// click is applying is never touched (the compare-and-set). Returns how
/// many closed.
pub(crate) fn close_decided(r: &reqs::AppRequest) -> usize {
    let Some(ws) = r.requester_workspace.as_deref() else { return 0 };
    let open: Vec<PermissionRequest> = match store::open(Some(ws)) {
        Ok(o) => o.into_iter().filter(|q| q.action == ACTION && q.params.get("request").and_then(J::as_str) == Some(r.id.as_str())).collect(),
        Err(e) => {
            k2_core::log_debug!("[app-cards] open cards of {}: {e}", r.id);
            return 0;
        }
    };
    if open.is_empty() {
        return 0;
    }
    // `decided_by` as `routes::actor` writes it: `owner`, `owner:<login>`
    // or `user:<login>`; the card names the person.
    let by = match r.decided_by.as_deref() {
        Some(d) => d.strip_prefix("owner:").or_else(|| d.strip_prefix("user:")).unwrap_or(d).to_string(),
        None => DAEMON_ACTOR.to_string(),
    };
    let (state, message) = match r.status.as_str() {
        reqs::STATUS_APPROVED => (RequestState::Approved, format!("Approved in App Approvals by {by}. It applies now.")),
        reqs::STATUS_FAILED => (RequestState::Failed, format!("Approved in App Approvals by {by}, but applying it failed.")),
        reqs::STATUS_DENIED => (RequestState::Denied, format!("Denied in App Approvals by {by}.")),
        reqs::STATUS_WITHDRAWN => (RequestState::Cancelled, "Withdrawn by the agent.".to_string()),
        reqs::STATUS_EXPIRED => (RequestState::Expired, "Expired unanswered (7 days).".to_string()),
        _ => return 0,
    };
    let now = k2_core::overlay::now_secs();
    let mut closed = 0;
    for q in &open {
        match store::cas_open_to(q.id.as_str(), state) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                k2_core::log_debug!("[app-cards] close {}: {e}", q.id);
                continue;
            }
        }
        let decided = store::Decided {
            state,
            by: Some(by.as_str()),
            via: Some("app-approvals"),
            reason: r.decision_reason.as_deref(),
            message: Some(&message),
            at: now,
        };
        if let Err(e) = store::decide(q.id.as_str(), &decided) {
            k2_core::log_debug!("[app-cards] close {}: {e}", q.id);
        }
        if let (Some(c), Some(k)) = (q.conversation_id.as_deref(), q.card_id.as_deref()) {
            present::refresh_card(c, k, if state == RequestState::Expired { present::CardClose::Expire } else { present::CardClose::Void });
        }
        if let Ok(Some(done)) = store::get(q.id.as_str()) {
            answer::audit(k2_core::permission::audit_event::ANSWER, DAEMON_ACTOR, "daemon", &done);
        }
        closed += 1;
    }
    closed
}

/// Cards raised before 0.46.13 were stored for the owner alone (floor
/// `owner`); now an Owner or Admin login approves (Rosson 2026-11-15). Each
/// open `apps.approve` request still at `owner` takes the catalog's floor
/// and the hash of today's words (the consequences line changed with it),
/// and its card's view is rewritten in place: the same card, same id, now
/// answerable by an Admin. Nothing new is posted, so no card is doubled.
/// Run once per daemon start (the card sweep's first pass); idempotent.
/// Returns how many requests moved.
pub(crate) fn open_cards_to_admins() -> usize {
    let Some(reg) = present::find_action(ACTION) else { return 0 };
    let to = present::floor_str(reg.spec.floor);
    if to == "owner" {
        return 0;
    }
    let open = match store::open(None) {
        Ok(o) => o,
        Err(e) => {
            k2_core::log_debug!("[app-cards] open requests: {e}");
            return 0;
        }
    };
    let mut moved = 0;
    for req in open.iter().filter(|r| r.action == ACTION && r.floor == "owner") {
        let hash = present::text_hash_of(reg.spec, &present::ctx_of(req));
        match store::lower_floor(req.id.as_str(), "owner", to, &hash) {
            Ok(true) => moved += 1,
            Ok(false) => continue,
            Err(e) => {
                k2_core::log_debug!("[app-cards] floor of {}: {e}", req.id);
                continue;
            }
        }
        if let (Some(conv), Some(card)) = (req.conversation_id.as_deref(), req.card_id.as_deref()) {
            present::refresh_card(conv, card, present::CardClose::Void);
        }
    }
    if moved > 0 {
        k2_core::log_debug!("[app-cards] {moved} open App card(s) now answerable by an owner or admin");
    }
    moved
}

/// The card's "<agent> says:" line: the agent's own reason when it gave
/// one (one line, ≤ 200 characters, never secret-shaped), else K2's.
fn why_of(r: &reqs::AppRequest) -> String {
    let own = r
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty() && w.chars().count() <= 200 && !w.contains(['\n', '\r']))
        .filter(|w| !crate::permission::routes::secret_shaped(w));
    match own {
        Some(w) => w.to_string(),
        None => format!("K2 held this for you: {}", r.reasons.first().map(String::as_str).unwrap_or("an owner step")),
    }
}

// ── Always allow ────────────────────────────────────────────────────

/// The held App request behind an `apps.approve` permission request.
fn held_of(req: &PermissionRequest) -> Option<reqs::AppRequest> {
    if req.action != ACTION {
        return None;
    }
    let id = req.params.get("request").and_then(J::as_str)?;
    let h = k2_core::db::shared();
    let conn = h.lock();
    reqs::get(&conn, id).ok().flatten()
}

/// Would "Let this agent manage its Apps' users" have let the requesting agent do
/// this itself? A test guest of its own App, a room line on a role of its own
/// App, or the guests-rooms grant in its own room only. Never copy-host or a label transfer (owner steps).
fn switch_covers(r: &reqs::AppRequest) -> bool {
    let Some(ws) = r.requester_workspace.as_deref() else { return false };
    if !r.requested_by.starts_with("agent:") || r.kind != reqs::KIND_MANAGE {
        return false;
    }
    match r.body.get("route").and_then(J::as_str) {
        Some(super::agent_grants::TEST_GUEST_ADD | super::role_room_hold::APPLY_ROUTE) => true,
        Some(super::agent_grants::GUEST_ROOMS_APPLY) => r
            .body
            .pointer("/body/rooms")
            .and_then(J::as_array)
            .is_some_and(|rooms| rooms.iter().all(|x| x.as_str() == Some(ws))),
        _ => false,
    }
}

/// Does this card offer "Always allow"? A single `apps.approve` card whose
/// held request the switch covers, while the switch is still off.
pub(crate) fn offers_always(req: &PermissionRequest) -> bool {
    held_of(req).is_some_and(|r| switch_covers(&r) && !k2_core::workspace::settings::agents_can_manage_app_users(&req.workspace_id))
}

/// [`offers_always`] for a card's view: the request by its display id.
pub(crate) fn offers_always_for(action: &str, request_id: &str) -> bool {
    if action != ACTION {
        return false;
    }
    match store::get_by_display_id(request_id) {
        Ok(Some(req)) => offers_always(&req),
        _ => false,
    }
}

/// "Always allow": after the approval, turn on "Let this agent manage its
/// Apps' users" for the asking workspace through the Settings route's own
/// entry, as the person (the owner both ways). `Ok(line)` for the card.
pub(crate) fn turn_on_always(ctx: &Ctx, who: &Answerer) -> Result<String, String> {
    let ws = ctx.workspace_id.as_str();
    if k2_core::workspace::settings::agents_can_manage_app_users(ws) {
        return Ok(format!("Let this agent manage its Apps' users was already on for {}.", ctx.workspace_handle));
    }
    let r = crate::sidecar_routes::access_entry(who, &json!({ "toggle": "app_users", "workspace": ws, "value": true }));
    if let Applied::Refused { message, .. } = crate::permission::actions::switches::applied(&r, "") {
        return Err(message);
    }
    Ok(format!(
        "Let this agent manage its Apps' users is now on for {}: it manages its own Apps' users itself next time \
         (turn it off in Settings → Workspaces / Agents → {} → Permissions → Apps).",
        ctx.workspace_handle, ctx.workspace_handle
    ))
}

#[cfg(test)]
mod covers_tests {
    /// The desktop's copy of [`super::SWITCH_COVERS`] is the same words.
    #[test]
    fn the_renderer_says_what_the_switch_covers_word_for_word() {
        let ts = include_str!("../../../../src/renderer/components/SessionView/permissionCardView.ts");
        let want = format!("export const APP_USERS_SWITCH_COVERS =\n  \"{}\"", super::SWITCH_COVERS);
        assert!(ts.contains(&want), "permissionCardView.ts must say, word for word:\n{want}");
    }
}
