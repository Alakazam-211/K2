//! A3 · projection — `project()`, one function at the three doors that serve
//! Thread docs (PRD rev 3.1 §4.2, vs-live C9): `thread_items_view` (every GET
//! page, the delta's `items`/`changed`, catchup), `wire_json_for` (every WS
//! frame, the skin stream, room-stream's `doc_for`) and `thread_preview`.
//!
//! Contract (frozen at P0):
//! - the identity for any doc without a permission action (`choice.action.id`
//!   `permission` / `permission.batch`); keep-as-mine and every other doc
//!   pass through unchanged, so with the switches off nothing moves;
//! - `cards` is 2 only when the request asked for it **and** the viewer is a
//!   person (`Viewer::is_person`); then the person gets the raw choice doc
//!   with `action.view` written for them (`k2_core::permission::card`);
//! - everyone else (old clients, agent passports) gets the §17.6 fallback;
//! - App guests never reach it: every guest path drops action cards first
//!   (and if one ever did, it gets an empty text doc, never the card);
//! - the doc's `id` and `seq` never change;
//! - the current Settings state is shown: `check` runs at render (cached
//!   [`limits::CHECK_CACHE_SECS`] per request id), so a satisfied pending
//!   card is drawn `already_done` with one option, `Close` (§4.2, Rosson Q1).
//!
//! **The stored view.** A1 writes the card's view (`choice.action.view`,
//! a `PermissionView` / `BatchView`) when it posts or rewrites the card; its
//! `viewer` is whatever A1 put there and is always replaced here. Everything
//! per viewer — `viewer`, `mode`, `noteBox`, the options, `undo` (approvers
//! only) and the asker's result line — is computed here, never stored.
//!
//! **The sweep** ([`sweep_already_done`], §6.2): every minute (the card
//! expiry loop, only while the server switch is on) pending requests are
//! re-checked; one that became satisfied moves `pending → already_done`
//! (compare-and-set) and its card is republished, so open clients update
//! within a minute with no hook in any Settings writer. One that stopped
//! being satisfied goes back to `pending`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use k2_core::overlay::doc::{CardAction, ChoiceOption, OverlayDoc, OverlayItem};
use k2_core::permission::card::{BatchState, BatchView, ItemView, Mode, PermissionView, ViewProof, Viewer, ViewerView};
use k2_core::permission::request::{limits, RequestState};
use k2_core::permission::{label, CARDS_CONTRACT, CARD_ACTION_BATCH, CARD_ACTION_PERMISSION};
use serde_json::Value;

use super::actions::{self, Check, Ctx};

// ── who is looking ───────────────────────────────────────────────────

/// One request's (or socket's) viewer and its `cards` opt-in, resolved
/// once and handed to every door.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Seen {
    pub viewer: Viewer,
    /// [`CARDS_CONTRACT`] only for a person who asked; else 0.
    pub cards: u8,
}

impl Seen {
    /// `asked`: the request's `cards` value. Honoured only for people.
    pub fn new(viewer: Viewer, asked: u8) -> Self {
        let cards = if viewer.is_person() && asked >= CARDS_CONTRACT { CARDS_CONTRACT } else { 0 };
        Seen { viewer, cards }
    }

    /// Today's view for this doc: borrowed unless it is a permission card.
    pub fn doc<'a>(&self, doc: &'a OverlayDoc) -> Cow<'a, OverlayDoc> {
        if is_permission_card(doc) {
            Cow::Owned(project(doc, &self.viewer, self.cards))
        } else {
            Cow::Borrowed(doc)
        }
    }

    /// [`Seen::doc`] for a Thread item (same id, same seq).
    pub fn item<'a>(&self, item: &'a OverlayItem) -> Cow<'a, OverlayItem> {
        if is_permission_card(&item.doc) {
            let mut it = item.clone();
            it.doc = project(&item.doc, &self.viewer, self.cards);
            Cow::Owned(it)
        } else {
            Cow::Borrowed(item)
        }
    }
}

/// A Thread doc carrying a permission action (`permission` /
/// `permission.batch`). Keep-as-mine (`zen.widget.keep`) is not one.
pub(crate) fn is_permission_card(doc: &OverlayDoc) -> bool {
    doc.choice
        .as_ref()
        .and_then(|c| c.action.as_ref())
        .is_some_and(|a| a.id == CARD_ACTION_PERMISSION || a.id == CARD_ACTION_BATCH)
}

/// `cards=` on a query (GET, both WS URLs): a whole number, else 0.
pub(crate) fn cards_param(params: &HashMap<String, String>) -> u8 {
    params.get("cards").and_then(|s| s.trim().parse::<u64>().ok()).map(clamp_cards).unwrap_or(0)
}

/// `"cards": 2` in a JSON body (catchup): a whole number, else 0. Anything
/// else is ignored, exactly as an unknown field was before.
pub(crate) fn cards_body(v: Option<&Value>) -> u8 {
    v.and_then(Value::as_u64).map(clamp_cards).unwrap_or(0)
}

fn clamp_cards(n: u64) -> u8 {
    u8::try_from(n).unwrap_or(u8::MAX)
}

/// A login as the dispatcher resolved it (`owner` = the owner token, else
/// a Connect username) with its Connect role. A user that no longer exists
/// reads as a member (the least it could be).
pub(crate) fn viewer_of_login(login: &str) -> Viewer {
    if login == "owner" {
        return Viewer::OwnerToken;
    }
    let role = match k2_core::connect_users::role_for_user(login) {
        Some(k2_core::connect_users::Role::Owner) => "owner",
        Some(k2_core::connect_users::Role::Admin) => "admin",
        _ => "member",
    };
    Viewer::Login { user: login.to_string(), role: role.to_string() }
}

/// The classified caller (catchup, the cell). A plain shell tab's passport
/// is still a passport here: `cards=2` is for people.
pub(crate) fn viewer_of_caller(caller: &crate::sidecar_routes::Caller) -> Viewer {
    use crate::sidecar_routes::Caller;
    match caller {
        Caller::Owner => Viewer::OwnerToken,
        Caller::Login { username } => viewer_of_login(username),
        Caller::Shell { project_id } => Viewer::Passport { workspace_id: project_id.clone() },
        Caller::Agent(a) => Viewer::Passport { workspace_id: a.project_id.clone() },
        Caller::WrongCredential | Caller::Invalid => unknown(),
    }
}

/// Nobody we can name (an in-process call with no binding): never a
/// person, so it gets the waiting text (fail closed).
pub(crate) fn unknown() -> Viewer {
    Viewer::Passport { workspace_id: String::new() }
}

/// The viewer of one HTTP request: an App guest (bound pass), an agent
/// passport (stamped principal or a cell session), the classified caller
/// when there is one, else the login the dispatcher bound
/// (`read_routes::with_read_login`).
pub(crate) fn request_viewer(
    params: &HashMap<String, String>,
    guest: bool,
    caller: Option<&crate::sidecar_routes::Caller>,
) -> Viewer {
    if guest {
        return Viewer::Guest { app: None };
    }
    if let Some(p) = crate::caller_workspace::principal_from_params(params) {
        return Viewer::Passport { workspace_id: p.workspace_uuid };
    }
    if params.contains_key("cell_session_id") {
        return unknown();
    }
    if let Some(c) = caller {
        return viewer_of_caller(c);
    }
    match crate::read_routes::request_login() {
        Some(login) => viewer_of_login(&login),
        None => unknown(),
    }
}

/// The facts about a viewer that the projection uses.
struct Facts {
    person: bool,
    login: String,
    /// `owner`, `admin` or `member` for people; empty otherwise.
    role: String,
    via: &'static str,
}

fn facts(viewer: &Viewer) -> Facts {
    match viewer {
        Viewer::OwnerToken => Facts { person: true, login: "owner".into(), role: "owner".into(), via: "owner-token" },
        Viewer::Login { user, role } => Facts { person: true, login: user.clone(), role: role.clone(), via: "login" },
        Viewer::Passport { .. } | Viewer::Guest { .. } => {
            Facts { person: false, login: String::new(), role: String::new(), via: "" }
        }
    }
}

/// The viewer meets a card floor (`approvers.floor`: `owner` or `admin`;
/// the GardenOwner floor reads `owner`). Re-checked at the tap (A1).
fn meets(floor: &str, f: &Facts) -> bool {
    f.person && (f.role == "owner" || (floor == "admin" && f.role == "admin"))
}

// ── the current Settings state (§4.2, §6.2) ──────────────────────────

fn check_cache() -> &'static Mutex<HashMap<String, (Instant, Check)>> {
    static C: std::sync::OnceLock<Mutex<HashMap<String, (Instant, Check)>>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

const CACHE_TTL: Duration = Duration::from_secs(limits::CHECK_CACHE_SECS);
/// Past this many entries, expired ones are dropped on the next insert.
const CACHE_TRIM_AT: usize = 1024;

/// The stored values a request's `check` runs on. A narrow read of the 0218
/// row (A1's `store` owns the table; this reads three columns). `request_id`
/// is the card's `view.requestId`: the row's `pr_…` id, or the held
/// request's own id (`held_ref`, e.g. `ar_22`; the newest row for it).
fn request_ctx(request_id: &str) -> Option<(String, Ctx)> {
    let db = k2_core::db::try_shared()?;
    let row = {
        let conn = db.lock();
        conn.query_row(
            "SELECT workspace_id, action, params_json FROM permission_requests WHERE id = ?1 OR held_ref = ?1 \
             ORDER BY (id = ?1) DESC, created_at DESC LIMIT 1",
            rusqlite::params![request_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
        )
    };
    let (workspace_id, action, params_json) = match row {
        Ok(r) => r,
        Err(rusqlite::Error::QueryReturnedNoRows) => return None,
        Err(e) => {
            k2_core::log_debug!("[permission/view] request {request_id}: {e}");
            return None;
        }
    };
    let params = match serde_json::from_str::<Value>(&params_json) {
        Ok(Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    };
    let workspace_handle =
        k2_core::workspace_session_handles::workspace_address_name_shared(&workspace_id).unwrap_or_else(|_| workspace_id.clone());
    Some((action, Ctx { workspace_id, workspace_handle, params }))
}

/// Run `check` for one request now (no cache). `None`: no such request or
/// no registered action (an E-lane row not built yet) — the stored state
/// stands.
fn run_check(request_id: &str) -> Option<Check> {
    #[cfg(test)]
    if let Some(c) = test_checks::get(request_id) {
        return Some(c);
    }
    let (action, ctx) = request_ctx(request_id)?;
    let reg = actions::find(&action)?;
    Some((reg.check)(&ctx))
}

/// [`run_check`] behind the 2 s cache (§4.2: many viewers of one card on
/// one socket burst share a check).
fn cached_check(request_id: &str) -> Option<Check> {
    let now = Instant::now();
    {
        let cache = check_cache().lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, c)) = cache.get(request_id) {
            if now.duration_since(*at) < CACHE_TTL {
                return Some(c.clone());
            }
        }
    }
    let c = run_check(request_id)?;
    remember(request_id, c.clone(), now);
    Some(c)
}

fn remember(request_id: &str, c: Check, at: Instant) {
    let mut cache = check_cache().lock().unwrap_or_else(|p| p.into_inner());
    if cache.len() >= CACHE_TRIM_AT {
        cache.retain(|_, (t, _)| at.duration_since(*t) < CACHE_TTL);
    }
    cache.insert(request_id.to_string(), (at, c));
}

/// Where a request stands for drawing: the stored state, with the card's
/// own status winning when it closed without a view rewrite, and an open
/// request re-checked against Settings (cached).
fn current_state(stored: RequestState, request_id: &str, choice_status: &str, answer: Option<&str>) -> RequestState {
    match choice_status {
        "pending" => {}
        // The card closed but the view was not rewritten (only A1 rewrites
        // it; a pending card's view can't be rewritten once closed): never
        // draw buttons on a closed card.
        "expired" if stored.is_open() || stored == RequestState::Applying => return RequestState::Expired,
        "voided" if stored.is_open() || stored == RequestState::Applying => return RequestState::Cancelled,
        "answered" if stored.is_open() || stored == RequestState::Applying => {
            return match answer {
                Some(a) if a == label::CLOSE => RequestState::AlreadyDone,
                Some(a) if a == label::ALLOW || a == label::ALLOW_ALL => RequestState::Approved,
                Some(a) if a == label::DENY || a == label::DENY_ALL => RequestState::Denied,
                _ => stored,
            };
        }
        _ => return stored,
    }
    if !stored.is_open() {
        return stored;
    }
    match cached_check(request_id) {
        Some(Check::AlreadyDone) => RequestState::AlreadyDone,
        Some(Check::Needed) => RequestState::Pending,
        // Values that moved are `stale` only at the click (A1); drawn as stored.
        Some(Check::Moved(_)) | None => stored,
    }
}

// ── project() ────────────────────────────────────────────────────────

/// The doc as `viewer` may see it (§4.2, §17). The identity for every doc
/// without a permission action.
pub(crate) fn project(doc: &OverlayDoc, viewer: &Viewer, cards: u8) -> OverlayDoc {
    if !is_permission_card(doc) {
        return doc.clone();
    }
    if matches!(viewer, Viewer::Guest { .. }) {
        // Never reached: every guest path drops action cards first. If one
        // ever did, it learns nothing (not even the fallback text).
        return as_text(doc, String::new());
    }
    let cards = if viewer.is_person() && cards >= CARDS_CONTRACT { CARDS_CONTRACT } else { 0 };
    let Some(action) = doc.choice.as_ref().and_then(|c| c.action.as_ref()) else {
        return doc.clone();
    };
    let view = action.view.clone().unwrap_or(Value::Null);
    let out = if action.id == CARD_ACTION_BATCH {
        serde_json::from_value::<BatchView>(view).map(|v| project_batch(doc, v, viewer, cards))
    } else {
        serde_json::from_value::<PermissionView>(view).map(|v| project_one(doc, v, viewer, cards))
    };
    match out {
        Ok(d) => d,
        Err(e) => {
            // A card without a readable view (a bug upstream): K2's own
            // prompt as text, never the raw card or its action.
            k2_core::log_debug!("[permission/view] card {} has no readable view: {e}", doc.id);
            let prompt = doc.choice.as_ref().map(|c| c.prompt.clone()).unwrap_or_default();
            as_text(doc, prompt)
        }
    }
}

/// The same doc (id, from, to, time, via) as `kind:"text"` with `body`.
fn as_text(doc: &OverlayDoc, body: String) -> OverlayDoc {
    let mut d = doc.clone();
    d.kind = "text".to_string();
    d.choice = None;
    d.body = Some(body);
    d
}

fn agent_of(item: &ItemView, doc: &OverlayDoc) -> String {
    item.why.as_ref().map(|w| w.by.clone()).filter(|b| !b.is_empty()).unwrap_or_else(|| doc.from.clone())
}

fn floor_word(floor: &str) -> &'static str {
    if floor == "admin" {
        "an admin"
    } else {
        "an owner"
    }
}

/// The waiting text (§4.3, §17.2.3).
fn waiting_line(item: &ItemView, agent: &str, is_asker: bool) -> String {
    let who = floor_word(&item.approvers.floor);
    if is_asker {
        format!("Asked {who} to turn on {} for you. You can keep working with {agent} meanwhile.", item.setting)
    } else {
        format!("{agent} asked {who} to turn on {}.", item.setting)
    }
}

fn result_message(action: &CardAction) -> Option<String> {
    action.result.as_ref().and_then(|r| r.get("message")).and_then(Value::as_str).map(str::to_string)
}

fn answered_by(item: &ItemView, doc: &OverlayDoc) -> String {
    item.answer
        .as_ref()
        .map(|a| a.by.clone())
        .or_else(|| doc.choice.as_ref().and_then(|c| c.answered_by.as_ref()).map(|a| a.login.clone()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "a person".to_string())
}

/// One line for a request in `state` (the §17.2.4 "Draw" column as text):
/// the fallback body of a closed card, and a batch item's line.
fn state_line(item: &ItemView, state: RequestState, agent: &str, doc: &OverlayDoc, result: Option<&str>) -> String {
    let what = format!("{} for {agent}", item.setting);
    match state {
        RequestState::Pending => format!("Waiting: {what}."),
        RequestState::AlreadyDone => format!("Already on: {what}."),
        RequestState::Applying => format!("Applying {what}…"),
        RequestState::Approved => format!("Allowed by {}: {what}.", answered_by(item, doc)),
        RequestState::Denied => {
            let reason = item.answer.as_ref().and_then(|a| a.reason.clone()).filter(|r| !r.trim().is_empty());
            match reason {
                Some(r) => format!("Denied by {}: {what}. Reason: {r}", answered_by(item, doc)),
                None => format!("Denied by {}: {what}.", answered_by(item, doc)),
            }
        }
        RequestState::Expired => format!("Expired unanswered: {what}."),
        RequestState::Stale => format!("Already changed; nothing applied: {what}."),
        RequestState::Failed => match result {
            Some(m) if !m.trim().is_empty() => format!("Couldn't apply {what}: {m}"),
            _ => format!("Couldn't apply {what}. Nothing was applied."),
        },
        RequestState::Cancelled => format!("Withdrawn by {agent}: {}.", item.setting),
        RequestState::Closed => format!("Closed: permission cards were turned off. {what} was not applied."),
    }
}

/// The viewer's own note on this request, if any (`noteBox.mine`).
fn my_note(item: &ItemView, f: &Facts) -> Option<String> {
    if !f.person {
        return None;
    }
    item.notes.iter().find(|n| n.by == f.login).map(|n| n.text.clone())
}

fn labels(ls: &[&str]) -> Vec<ChoiceOption> {
    ls.iter().map(|l| ChoiceOption { label: l.to_string() }).collect()
}

/// The card's exact labels (§17.3), whatever the stored doc holds (the
/// stored copy may have been written for another viewer).
fn answer_labels() -> Vec<ChoiceOption> {
    labels(&[label::ALLOW, label::DENY])
}

fn batch_labels() -> Vec<ChoiceOption> {
    labels(&[label::ALLOW_ALL, label::DENY_ALL])
}

/// Approver options for one request in `state` (§17.2.4): `Allow` /
/// `Deny` (kept on a closed card, as §17.11.2 shows), `Close` when it is
/// already on, none while applying.
fn approver_options(state: RequestState, answer: Option<&str>) -> Vec<ChoiceOption> {
    match state {
        RequestState::AlreadyDone => labels(&[label::CLOSE]),
        RequestState::Applying => Vec::new(),
        _ if answer == Some(label::CLOSE) => labels(&[label::CLOSE]),
        _ => answer_labels(),
    }
}

/// The per-viewer item: state, note box, undo (approvers only).
fn item_for(mut item: ItemView, state: RequestState, can: bool, f: &Facts) -> ItemView {
    item.state = state;
    item.note_box.allowed = f.person && !can && state == RequestState::Pending;
    item.note_box.max = limits::NOTE_MAX_CHARS;
    item.note_box.mine = my_note(&item, f);
    if !can {
        item.undo = None;
    }
    item
}

fn project_one(doc: &OverlayDoc, view: PermissionView, viewer: &Viewer, cards: u8) -> OverlayDoc {
    let f = facts(viewer);
    let choice = doc.choice.as_ref().expect("a permission card is a choice card");
    let action = choice.action.as_ref().expect("a permission card has an action");
    let state = current_state(view.item.state, &view.item.request_id, &choice.status, choice.answer.as_deref());
    let can = meets(&view.item.approvers.floor, &f);
    let agent = agent_of(&view.item, doc);
    let is_asker = f.person && view.item.asked_for.as_deref() == Some(f.login.as_str());

    if cards == CARDS_CONTRACT && f.person {
        let mut d = doc.clone();
        let c = d.choice.as_mut().expect("choice");
        let a = c.action.as_mut().expect("action");
        c.options = if can { approver_options(state, choice.answer.as_deref()) } else { Vec::new() };
        if !can && state == RequestState::Approved {
            // The approver's result names the Settings path; the asker and
            // other people get who allowed it (§17.11.6).
            a.result = Some(serde_json::json!({
                "message": format!("Allowed by {}.", answered_by(&view.item, doc)),
                "outcome": "approved",
            }));
        }
        let pv = PermissionView {
            v: view.v,
            kind: view.kind,
            viewer: ViewerView {
                login: f.login.clone(),
                role: f.role.clone(),
                via: f.via.to_string(),
                mode: if can { Mode::Approve } else { Mode::Wait },
                can_answer: can,
                is_asker,
                can_answer_all: None,
            },
            item: item_for(view.item, state, can, &f),
        };
        a.view = Some(serde_json::to_value(&pv).expect("a permission view serializes"));
        return d;
    }

    // §17.6 fallback: old clients, agent passports.
    let item = &view.item;
    match state {
        RequestState::Pending if can => match item.proof {
            ViewProof::Click => {
                let mut prompt = choice.prompt.clone();
                if let Some(w) = item.why.as_ref().filter(|w| !w.text.trim().is_empty()) {
                    prompt.push_str(&format!("\n{} says: \"{}\"", w.by, w.text));
                }
                for n in &item.notes {
                    prompt.push_str(&format!("\n{} ({}): \"{}\"", n.by, n.role, n.text));
                }
                let mut d = doc.clone();
                let c = d.choice.as_mut().expect("choice");
                c.prompt = prompt;
                c.options = answer_labels();
                c.action = Some(CardAction {
                    id: action.id.clone(),
                    params: action.params.clone(),
                    dedupe: String::new(),
                    result: None,
                    view: None,
                });
                d
            }
            ViewProof::Confirm { .. } => {
                as_text(doc, format!("{} Approve on your desktop, or update K2 Companion.", choice.prompt))
            }
        },
        RequestState::Pending => as_text(doc, waiting_line(item, &agent, is_asker)),
        other => as_text(doc, state_line(item, other, &agent, doc, result_message(action).as_deref())),
    }
}

fn project_batch(doc: &OverlayDoc, view: BatchView, viewer: &Viewer, cards: u8) -> OverlayDoc {
    let f = facts(viewer);
    let choice = doc.choice.as_ref().expect("a permission card is a choice card");
    let action = choice.action.as_ref().expect("a permission card has an action");
    let states: Vec<RequestState> = view
        .items
        .iter()
        .map(|i| current_state(i.state, &i.request_id, &choice.status, choice.answer.as_deref()))
        .collect();
    let open = |s: &RequestState| s.is_open() || *s == RequestState::Applying;
    let batch_state = if choice.status == "pending" && states.iter().any(open) { BatchState::Pending } else { BatchState::Done };
    let can = meets(&view.approvers.floor, &f);
    let pending_items: Vec<(&ItemView, RequestState)> =
        view.items.iter().zip(states.iter().copied()).filter(|(_, s)| *s == RequestState::Pending).collect();
    let can_all = f.person
        && pending_items.iter().all(|(i, _)| meets(&i.approvers.floor, &f) && matches!(i.proof, ViewProof::Click));
    let agent = view.items.first().map(|i| agent_of(i, doc)).unwrap_or_else(|| doc.from.clone());

    if cards == CARDS_CONTRACT && f.person {
        let mut d = doc.clone();
        let c = d.choice.as_mut().expect("choice");
        let a = c.action.as_mut().expect("action");
        // "Allow all" is drawn only when `viewer.canAnswerAll` (§17.3); the
        // labels stay so a client can answer per item meanwhile.
        c.options = if can { batch_labels() } else { Vec::new() };
        let items = view
            .items
            .into_iter()
            .zip(states.iter().copied())
            .map(|(i, s)| {
                let can_i = meets(&i.approvers.floor, &f);
                item_for(i, s, can_i, &f)
            })
            .collect();
        let bv = BatchView {
            v: view.v,
            kind: view.kind,
            state: batch_state,
            viewer: ViewerView {
                login: f.login.clone(),
                role: f.role.clone(),
                via: f.via.to_string(),
                mode: if can { Mode::Approve } else { Mode::Wait },
                can_answer: can,
                is_asker: false,
                can_answer_all: Some(can && can_all),
            },
            items,
            approvers: view.approvers,
            expires_at: view.expires_at,
        };
        a.view = Some(serde_json::to_value(&bv).expect("a batch view serializes"));
        return d;
    }

    // §17.6 fallback.
    let lines: Vec<String> = view
        .items
        .iter()
        .zip(states.iter().copied())
        .map(|(i, s)| match s {
            RequestState::Pending => format!("- {}: waiting", i.setting),
            other => format!("- {}", state_line(i, other, &agent, doc, None)),
        })
        .collect();
    if batch_state == BatchState::Pending && can {
        let any_confirm = pending_items.iter().any(|(i, _)| !matches!(i.proof, ViewProof::Click));
        if !any_confirm && can_all {
            let mut d = doc.clone();
            let c = d.choice.as_mut().expect("choice");
            c.prompt = format!("{}\n{}", choice.prompt, lines.join("\n"));
            c.options = batch_labels();
            c.action = Some(CardAction {
                id: action.id.clone(),
                params: action.params.clone(),
                dedupe: String::new(),
                result: None,
                view: None,
            });
            return d;
        }
        return as_text(doc, format!("{} Approve on your desktop, or update K2 Companion.\n{}", choice.prompt, lines.join("\n")));
    }
    if batch_state == BatchState::Pending {
        let who = floor_word(&view.approvers.floor);
        let n = view.items.len();
        return as_text(doc, format!("{agent} asked {who} for {n} permissions:\n{}", lines.join("\n")));
    }
    as_text(doc, format!("{agent} asked for {} permissions:\n{}", view.items.len(), lines.join("\n")))
}

// ── the sweep's already_done republish (§6.2) ────────────────────────

/// One open request the sweep re-checks.
struct OpenRow {
    id: String,
    /// `view.requestId`: the held id when there is one, else `id`.
    display_id: String,
    state: RequestState,
    conversation_id: String,
    card_id: String,
}

fn open_rows(now: i64) -> Result<Vec<OpenRow>, String> {
    let Some(db) = k2_core::db::try_shared() else { return Ok(Vec::new()) };
    let conn = db.lock();
    let mut st = conn
        .prepare(
            "SELECT id, state, conversation_id, card_id, held_ref FROM permission_requests \
             WHERE state IN ('pending', 'already_done') AND answered_at IS NULL AND expires_at > ?1 \
             AND conversation_id IS NOT NULL AND card_id IS NOT NULL",
        )
        .map_err(|e| format!("open requests: {e}"))?;
    let rows = st
        .query_map(rusqlite::params![now], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|e| format!("open requests: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        let (id, state, conversation_id, card_id, held_ref) = row.map_err(|e| format!("open requests: {e}"))?;
        let Some(state) = RequestState::parse(&state) else { continue };
        let display_id = held_ref.filter(|h| !h.trim().is_empty()).unwrap_or_else(|| id.clone());
        out.push(OpenRow { id, display_id, state, conversation_id, card_id });
    }
    Ok(out)
}

/// `pending ⇄ already_done` as a compare-and-set: true when this call
/// moved it (A1's click CAS may have taken it first).
fn cas_state(id: &str, from: RequestState, to: RequestState) -> Result<bool, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let n = conn
        .execute(
            "UPDATE permission_requests SET state = ?1 WHERE id = ?2 AND state = ?3 AND answered_at IS NULL",
            rusqlite::params![to.as_str(), id, from.as_str()],
        )
        .map_err(|e| format!("request {id}: {e}"))?;
    Ok(n == 1)
}

/// The stored view with `request_id`'s state set (the single view's
/// `state`, or that item's in a batch, plus the batch state).
fn view_with_state(mut view: Value, request_id: &str, state: RequestState) -> Option<Value> {
    if view.get("kind").and_then(Value::as_str) == Some("batch") {
        let items = view.get_mut("items")?.as_array_mut()?;
        let it = items.iter_mut().find(|i| i.get("requestId").and_then(Value::as_str) == Some(request_id))?;
        it["state"] = Value::String(state.as_str().to_string());
        return Some(view);
    }
    if view.get("requestId").and_then(Value::as_str) != Some(request_id) {
        return None;
    }
    view["state"] = Value::String(state.as_str().to_string());
    Some(view)
}

/// Rewrite the pending card's stored state for one request and publish it
/// (each subscriber gets it projected for them).
fn republish(row: &OpenRow, state: RequestState) -> Result<(), String> {
    let Some(doc) = k2_core::overlay::get_thread_doc(&row.conversation_id, &row.card_id)? else {
        return Ok(());
    };
    let Some(view) = doc.choice.as_ref().and_then(|c| c.action.as_ref()).and_then(|a| a.view.clone()) else {
        return Ok(());
    };
    let Some(view) = view_with_state(view, &row.display_id, state) else { return Ok(()) };
    match crate::overlay_routes::update_action_view(&row.conversation_id, &row.card_id, view) {
        Ok(()) => Ok(()),
        // Answered, voided or expired meanwhile: nothing to redraw.
        Err(e) if e == k2_core::overlay::CARD_NOT_PENDING => Ok(()),
        Err(e) => Err(e),
    }
}

/// One sweep (§6.2): re-check every open request; a pending one that is
/// now satisfied moves to `already_done`, an `already_done` one that no
/// longer is goes back to `pending`, and its card is republished. Only
/// while the server switch is on. Returns how many moved.
pub(crate) fn sweep_already_done(now: i64) -> usize {
    if !k2_core::permission::gate::server_on() {
        return 0;
    }
    sweep_open_requests(now)
}

/// [`sweep_already_done`] without the switch check.
fn sweep_open_requests(now: i64) -> usize {
    let rows = match open_rows(now) {
        Ok(r) => r,
        Err(e) => {
            k2_core::log_debug!("[permission/view] already_done sweep: {e}");
            return 0;
        }
    };
    let mut moved = 0;
    for row in rows {
        let Some(c) = run_check(&row.display_id) else { continue };
        remember(&row.display_id, c.clone(), Instant::now());
        let to = match c {
            Check::AlreadyDone => RequestState::AlreadyDone,
            Check::Needed => RequestState::Pending,
            Check::Moved(_) => continue,
        };
        if to == row.state {
            continue;
        }
        match cas_state(&row.id, row.state, to) {
            Ok(true) => {
                moved += 1;
                if let Err(e) = republish(&row, to) {
                    k2_core::log_debug!("[permission/view] republish {}: {e}", row.id);
                }
            }
            Ok(false) => {}
            Err(e) => k2_core::log_debug!("[permission/view] already_done sweep: {e}"),
        }
    }
    moved
}

/// Test-only `check` results by request id (the E-lane actions aren't
/// registered on this branch).
#[cfg(test)]
pub(crate) mod test_checks {
    use super::Check;
    use std::collections::HashMap;
    use std::sync::Mutex;

    static MAP: Mutex<Option<HashMap<String, Check>>> = Mutex::new(None);

    pub(crate) fn set(request_id: &str, c: Check) {
        MAP.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new).insert(request_id.to_string(), c);
        super::check_cache().lock().unwrap_or_else(|p| p.into_inner()).remove(request_id);
    }

    pub(crate) fn get(request_id: &str) -> Option<Check> {
        MAP.lock().unwrap_or_else(|p| p.into_inner()).as_ref().and_then(|m| m.get(request_id).cloned())
    }
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
