//! F · golden samples and the drift test (PRD rev 3.1 §13.4, §17.10).
//!
//! `cargo test -p k2-daemon permission_samples` runs the **real** daemon
//! code — the request rows, `batch::place` / `present_action_card`, the
//! answer route (`POST /cli/thread/answer` through `dispatch_post_as`), the
//! expiry sweep, the cancel route, the switch-off close, the stored view
//! (`present::view_for`) and the per-viewer projection (`view::project`,
//! `Seen::item`, `overlay_ws::wire_json_for`) — and compares what it
//! produces with the files in `crates/k2-daemon/tests/golden/permission_cards/`.
//! Any difference fails the test: the wire changed without the samples.
//!
//! **Updating.** `UPDATE_PERMISSION_SAMPLES=1 cargo test -p k2-daemon --lib
//! permission_samples` (or `scripts/permission-cards/update-samples.sh`)
//! rewrites the folder and its `index.json`. Review the diff: it is the
//! phone contract changing.
//!
//! **Fixed clock and ids.** Each card state is built in a fresh scoped DB
//! with fixed workspace, conversation, session and request ids and a fixed
//! cast of Connect logins in a temp `$HOME` (`rosson` owner, `maya` admin,
//! `dana` member and the asker, `lee` member). Requests are asked "now"
//! (the batch window and the expiry sweep run on the real clock); before a
//! snapshot every time column of the rows is pinned to the fixed clock
//! (asked [`T0`], expires [`EXPIRES`], answered [`ANSWERED`]) and the card's
//! stored view is rebuilt from the rows by the real `refresh_card`. Only
//! the fields the overlay store mints itself are normalized on output: the
//! doc's `id` (a UUID) → a fixed UUID, `created_at` → [`T0`],
//! `choice.expiresAt` → [`EXPIRES`], `choice.answeredAt` → the pinned
//! answer time, and a frame's `rev` (the per-process write counter) → 1.
//!
//! **Actions.** Until lanes E1/E2 register `compute.use`, `apps.approve`,
//! `apps.build` and `apps.core`, this module supplies stand-in specs with
//! the PRD's words. Once a real action is registered its **spec** (words,
//! floor, proof, grant) is used instead; only `check` and `apply` stay the
//! sample's own, so a state like `stale` or `failed` can be produced on
//! purpose. Stand-ins are visible only on the sample run's own thread
//! (`testkit::find`), never to another test.
//!
//! App guests never receive a permission card on any path (K2's rule,
//! 2026-10-21): the `guest__*__hidden.json` files say so, and the test
//! asserts every card sample is an action card (`OverlayDoc::is_action_card`),
//! the one predicate every guest path drops.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use k2_core::connect_users::{self, Role};
use k2_core::db::schema::WorkspaceSession;
use k2_core::overlay::{self, OverlayDoc};
use k2_core::permission::card::Viewer;
use k2_core::permission::catalog::{
    ActionSpec, Area, ConfirmAction, Direction, Expiry, Floor, Grant, ParamKind, ParamSpec, Proof, SettingsRef, Undo,
    WallCoverage, Words,
};
use k2_core::permission::request::{params_hash, PermissionRequest, RequestId, RequestState};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::actions::{self, Answerer, Applied, Check, Ctx, Registered};
use super::present::{self, CardClose};
use super::{answer, batch, expiry, routes, store, view};
use crate::compute_routes::Who;

// ── the fixed clock, ids and cast ────────────────────────────────────

/// When every request was asked (2025-10-21 15:20 UTC).
pub(crate) const T0: i64 = 1_761_060_000;
/// `expiresAt`: 72 hours after [`T0`].
pub(crate) const EXPIRES: i64 = T0 + 259_200;
/// When dana wrote her note.
pub(crate) const NOTE_AT: i64 = T0 + 300;
/// When a person answered (or the daemon closed) a request.
pub(crate) const ANSWERED: i64 = T0 + 1_200;

const BILLING_WS: &str = "4b2f9a10-6c1e-4d7a-9f3e-2a1b0c9d8e71";
const BILLING_CONV: &str = "6f1c0b52-2b1e-4d0e-9c1a-1b2f3e4d5a60";
const SHOP_WS: &str = "8d3e1f20-7a2b-4c9d-8e1f-3b2c1d0e9f82";
const SHOP_CONV: &str = "2c9e4a71-5d3b-4f1a-8b2c-6d7e8f9a0b13";
const SESSION: &str = "sess_9";

/// The doc ids every sample shows (real ids are UUIDs minted by the store).
const CARD_ID: &str = "7f3a2c90-4f1e-4c2a-9b0d-3e5f6a7b8c90";
const CONFIRM_CARD_ID: &str = "9b10ee41-2d3c-4b5a-8e6f-7a8b9c0d1e2f";
const BATCH_CARD_ID: &str = "b4a1c0de-5e6f-4a7b-9c8d-0e1f2a3b4c5d";

/// The single click card's request (`compute.use`, admin floor).
const REQ: &str = "pr_7f3a2c";
/// The same ask on the card whose setting was turned on meanwhile (its own
/// id: the render check is cached per request id for 2 s).
const REQ_DONE: &str = "pr_3d0e5a";
/// The confirm card's row; its card and turns show the held id.
const REQ_CONFIRM: &str = "pr_9b10ee";
const HELD: &str = "ar_22";
/// The batch: two click items and one confirm item.
const REQ_B1: &str = "pr_a1a1a1";
const REQ_B2: &str = "pr_a2a2a2";
const REQ_B3: &str = "pr_a3a3a3";
const HELD_B3: &str = "ar_23";

const ROSSON: &str = "rosson";
const MAYA: &str = "maya";
const DANA: &str = "dana";
const LEE: &str = "lee";
const CAST_PASSWORD: &str = "sample-password-1";

const WHY: &str = "Run the test suite on k2n2-m1; it takes 40 min here";
const NOTE: &str = "I need this for the payroll run";

// ── stand-in actions (the real spec wins once registered) ───────────

thread_local! {
    /// True only on the sample run's own thread.
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    /// What the sample's `check` / `apply` do right now.
    static BEHAVIOUR: Cell<Behaviour> = const { Cell::new(Behaviour::Needed) };
    /// Each world's real conversation id → the fixed one the samples show.
    static CONVS: std::cell::RefCell<Vec<(String, &'static str)>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    /// Still needed; `apply` succeeds.
    Needed,
    /// Already on in Settings (`already_done`).
    AlreadyOn,
    /// The values moved before the click (`stale`).
    Moved,
    /// The route's own entry refuses (`failed`).
    Refused,
}

fn set_behaviour(b: Behaviour) {
    BEHAVIOUR.with(|c| c.set(b));
}

const STANDARD_SETTINGS: SettingsRef = SettingsRef { page: "workspaces/{ws}/agent", path: "Settings → Workspaces → {ws} → Agent" };
const APPROVALS: SettingsRef = SettingsRef { page: "app-approvals", path: "Settings → Apps → Approvals" };

const fn switch_words(setting: &'static str, what: &'static str, consequences: &'static str) -> Words {
    Words { setting, what_changes: what, consequences, scope: "{ws}", duration: "until someone turns it off", retry: "run your command again" }
}

const fn switch(id: &'static str, floor: Floor, words: Words, params: &'static [ParamSpec]) -> ActionSpec {
    ActionSpec {
        id,
        area: Area::Switches,
        params,
        floor,
        direction: Direction::Loosen,
        grant: Grant::Standing,
        proof: Proof::Click,
        expiry: Expiry::STANDARD,
        words,
        undo: Undo::Twin { action: id },
        settings: STANDARD_SETTINGS,
        walls: &[],
        coverage: WallCoverage::None,
        machine_hints: &[],
    }
}

static COMPUTE_USE: ActionSpec = switch(
    "compute.use",
    Floor::Admin,
    switch_words(
        "Allow compute nodes",
        "Let {ws} run jobs on this server's compute nodes",
        "Every active machine; default limits; jobs see only the code {ws} sends.",
    ),
    &[],
);
static APPS_BUILD: ActionSpec = switch(
    "apps.build",
    Floor::Owner,
    switch_words("Build and run Apps", "Let {ws} build and run Apps", "It can build, deploy and restart its own Apps inside the default limits."),
    &[],
);
static APPS_CORE: ActionSpec = switch(
    "apps.core",
    Floor::Owner,
    switch_words("App cores", "Let {ws}'s Apps use the {core} core", "Its Apps may use that core without asking each time."),
    &[ParamSpec { key: "core", kind: ParamKind::Core, required: true, list: false, choices: &[] }],
);
static APPS_APPROVE: ActionSpec = ActionSpec {
    id: "apps.approve",
    area: Area::Held,
    params: &[ParamSpec { key: "request", kind: ParamKind::HeldRequest, required: true, list: false, choices: &[] }],
    floor: Floor::Owner,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::SettingsConfirm { action: ConfirmAction::AppApprove },
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "App Approvals",
        what_changes: "Run {ws}'s held App request {request} on this server",
        consequences: "The App's process runs as asked in {request} until someone stops it.",
        scope: "request {request}",
        duration: "once",
        retry: "run your command again",
    },
    undo: Undo::None,
    settings: APPROVALS,
    walls: &[],
    coverage: WallCoverage::None,
    machine_hints: &[],
};

fn sample_check(_ctx: &Ctx) -> Check {
    match BEHAVIOUR.with(Cell::get) {
        Behaviour::Needed | Behaviour::Refused => Check::Needed,
        Behaviour::AlreadyOn => Check::AlreadyDone,
        Behaviour::Moved => Check::Moved("The compute pool changed after billing asked.".to_string()),
    }
}

fn sample_apply(_ctx: &Ctx, _who: &Answerer, _token: Option<&str>) -> Applied {
    match BEHAVIOUR.with(Cell::get) {
        Behaviour::Refused => Applied::Refused { code: "compute_paused".into(), message: "The compute pool is paused.".into() },
        _ => Applied::Done { message: String::new() },
    }
}

fn sample_apply_confirm(_ctx: &Ctx, _who: &Answerer, token: Option<&str>) -> Applied {
    match token {
        Some(_) => Applied::Done { message: "Approved. The App is running.".into() },
        None => Applied::Refused { code: "vault_confirm_required".into(), message: "confirm first".into() },
    }
}

/// The sample action for `id` while the sample run is active on this
/// thread (see `testkit::find`); None otherwise.
pub(crate) fn stand_in(id: &str) -> Option<Registered> {
    if !ACTIVE.with(Cell::get) {
        return None;
    }
    let (spec, apply): (&'static ActionSpec, fn(&Ctx, &Answerer, Option<&str>) -> Applied) = match id {
        "compute.use" => (&COMPUTE_USE, sample_apply),
        "apps.build" => (&APPS_BUILD, sample_apply),
        "apps.core" => (&APPS_CORE, sample_apply),
        "apps.approve" => (&APPS_APPROVE, sample_apply_confirm),
        _ => return None,
    };
    let spec = actions::find(id).map(|r| r.spec).unwrap_or(spec);
    Some(Registered { spec, check: sample_check, apply })
}

/// Sets [`ACTIVE`] for the run; clears it on drop (also on panic).
struct Active;

impl Active {
    fn on() -> Self {
        ACTIVE.with(|c| c.set(true));
        Active
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        ACTIVE.with(|c| c.set(false));
        set_behaviour(Behaviour::Needed);
    }
}

// ── one card's world ────────────────────────────────────────────────

/// A fresh scoped DB with one workspace and its pinned Chat.
struct World {
    _db: k2_core::db::TestDbGuard,
    ws: &'static str,
    handle: &'static str,
    /// This world's own conversation (the overlay store outlives the
    /// scoped DB, so each card gets a fresh one); shown as `conv_fixed`.
    conv: &'static str,
    conv_fixed: &'static str,
}

fn world(ws: &'static str, handle: &'static str, conv_fixed: &'static str) -> World {
    let db = k2_core::db::scoped_for_test();
    let conv: &'static str = Box::leak(uuid::Uuid::new_v4().to_string().into_boxed_str());
    CONVS.with(|c| c.borrow_mut().push((conv.to_string(), conv_fixed)));
    {
        let shared = k2_core::db::shared();
        let conn = shared.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
            rusqlite::params![ws, handle, format!("/tmp/k2-permission-samples/{handle}")],
        )
        .expect("seed the workspace");
        WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), ws, None, Some(conv), "claude", "system", "running").expect("pin its Chat");
    }
    set_behaviour(Behaviour::Needed);
    World { _db: db, ws, handle, conv, conv_fixed }
}

fn billing() -> World {
    world(BILLING_WS, "billing", BILLING_CONV)
}

fn shopdev() -> World {
    world(SHOP_WS, "shopdev", SHOP_CONV)
}

/// A request row as the request route builds it (fixed id; asked now).
fn row(w: &World, id: &str, action: &str, params: Value, held: Option<&str>, asked_for: Option<&str>) -> PermissionRequest {
    let reg = present::find_action(action).unwrap_or_else(|| panic!("no sample action {action}"));
    let values = params.as_object().cloned().unwrap_or_default();
    let ctx = Ctx { workspace_id: w.ws.to_string(), workspace_handle: present::workspace_handle(w.ws), params: values.clone() };
    let now = overlay::now_secs();
    PermissionRequest {
        id: RequestId::parse(id).unwrap_or_else(|| panic!("{id} is not a request id")),
        workspace_id: w.ws.to_string(),
        session_id: Some(SESSION.to_string()),
        conversation_id: None,
        addr: Some(w.handle.to_string()),
        card_id: None,
        action: action.to_string(),
        params: Value::Object(values.clone()),
        params_hash: params_hash(&values),
        text_hash: present::words_hash(&present::words(reg.spec, &ctx)),
        why: WHY.to_string(),
        state: RequestState::Pending,
        floor: present::floor_str(reg.spec.floor).to_string(),
        held_ref: held.map(str::to_string),
        batch_id: None,
        ask_number: 1,
        asked_for: asked_for.map(str::to_string),
        created_at: now,
        expires_at: now + reg.spec.expiry.default_secs,
        answered_at: None,
        answered_by: None,
        answered_via: None,
        reason: None,
        result_message: None,
        updated_why: None,
        updated_at: None,
    }
}

/// Store the request and put its card on the Thread (the real placement).
fn ask(w: &World, req: &PermissionRequest) -> String {
    store::insert(req).expect("insert the request");
    let placed = batch::place(req, w.conv).expect("place the card");
    placed.card_id
}

fn note(id: &str, by: &str, role: &str) {
    store::put_note(id, by, "user", role, NOTE, NOTE_AT).expect("write the note");
}

/// The answer route, as `login` from `client` (`POST /cli/thread/answer`).
fn answer_as(login: &str, w: &World, card: &str, body: Value) -> (u16, Value) {
    let mut b = json!({ "addr": w.handle, "id": card });
    for (k, v) in body.as_object().expect("an answer body") {
        b[k] = v.clone();
    }
    let r = crate::overlay_routes::dispatch_post_as("/cli/thread/answer", &HashMap::new(), b.to_string().as_bytes(), login);
    let status: u16 = r.status.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("status {}", r.status));
    (status, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("answer body ({e}): {}", r.body)))
}

fn answer_ok(login: &str, w: &World, card: &str, body: Value) -> Value {
    let (s, v) = answer_as(login, w, card, body);
    assert_eq!(s, 200, "{v}");
    v
}

/// Pin every time column on this conversation's requests to the fixed
/// clock and rebuild the card's stored view from the rows.
fn pin(w: &World, card: &str, answered: i64) {
    {
        let shared = k2_core::db::shared();
        let conn = shared.lock();
        conn.execute(
            "UPDATE permission_requests SET created_at = ?1, expires_at = ?2, \
             answered_at = CASE WHEN answered_at IS NULL THEN NULL ELSE ?3 END, \
             updated_at = CASE WHEN updated_at IS NULL THEN NULL ELSE ?3 END \
             WHERE conversation_id = ?4",
            rusqlite::params![T0, EXPIRES, answered, w.conv],
        )
        .expect("pin the clock");
    }
    present::refresh_card(w.conv, card, CardClose::Void).unwrap_or_else(|| panic!("refresh card {card}"));
}

fn doc_of(w: &World, card: &str) -> OverlayDoc {
    overlay::get_thread_doc(w.conv, card).expect("read the card").unwrap_or_else(|| panic!("card {card}"))
}

// ── viewers ──────────────────────────────────────────────────────────

fn login(user: &str, role: &str) -> Viewer {
    Viewer::Login { user: user.to_string(), role: role.to_string() }
}

fn rosson() -> Viewer {
    login(ROSSON, "owner")
}
fn maya() -> Viewer {
    login(MAYA, "admin")
}
fn dana() -> Viewer {
    login(DANA, "member")
}
fn lee() -> Viewer {
    login(LEE, "member")
}
fn agent(w: &World) -> Viewer {
    Viewer::Passport { workspace_id: w.ws.to_string() }
}

// ── output ───────────────────────────────────────────────────────────

/// One sample file: its content and its `index.json` row.
struct Sample {
    view: String,
    state: String,
    form: String,
    transport: &'static str,
    body: Value,
}

#[derive(Default)]
struct Out {
    files: BTreeMap<String, Sample>,
}

impl Out {
    fn put(&mut self, view: &str, state: &str, form: &str, transport: &'static str, body: Value) {
        let name = format!("{view}__{state}__{form}.json");
        let s = Sample { view: view.into(), state: state.into(), form: form.into(), transport, body };
        assert!(self.files.insert(name.clone(), s).is_none(), "two samples named {name}");
    }
}

/// The store-minted fields, fixed (see the module doc).
fn fix_doc(d: &mut Value, card_real: &str, card_fixed: &str, answered: i64) {
    if d["id"] == card_real {
        d["id"] = json!(card_fixed);
    }
    d["created_at"] = json!(T0);
    if let Some(c) = d.get_mut("choice").and_then(Value::as_object_mut) {
        if c.contains_key("expiresAt") {
            c.insert("expiresAt".into(), json!(EXPIRES));
        }
        if c.contains_key("answeredAt") {
            c.insert("answeredAt".into(), json!(answered));
        }
    }
}

fn doc_json(d: &OverlayDoc, card_real: &str, card_fixed: &str, answered: i64) -> Value {
    let mut v = serde_json::to_value(d).expect("a doc serializes");
    fix_doc(&mut v, card_real, card_fixed, answered);
    v
}

/// Replace the real card id anywhere in a response body.
fn fix_ids(v: Value, card_real: &str, card_fixed: &str) -> Value {
    let mut s = v.to_string().replace(card_real, card_fixed);
    CONVS.with(|c| {
        for (real, fixed) in c.borrow().iter() {
            s = s.replace(real.as_str(), fixed);
        }
    });
    serde_json::from_str(&s).expect("still JSON")
}

/// The `cards=2` doc and the fallback doc for one viewer.
#[allow(clippy::too_many_arguments)]
fn both(out: &mut Out, view_name: &str, state: &str, doc: &OverlayDoc, viewer: &Viewer, real: &str, fixed: &str, answered: i64) {
    assert!(doc.is_action_card(), "a permission card is an action card (every guest path drops it)");
    let c2 = view::project(doc, viewer, k2_core::permission::CARDS_CONTRACT);
    out.put(view_name, state, "cards2", "doc", doc_json(&c2, real, fixed, answered));
    fallback(out, view_name, state, doc, viewer, real, fixed, answered);
}

#[allow(clippy::too_many_arguments)]
fn fallback(out: &mut Out, view_name: &str, state: &str, doc: &OverlayDoc, viewer: &Viewer, real: &str, fixed: &str, answered: i64) {
    let fb = view::project(doc, viewer, 0);
    out.put(view_name, state, "fallback", "doc", doc_json(&fb, real, fixed, answered));
}

fn response(out: &mut Out, case: &str, status: u16, body: Value, real: &str, fixed: &str) {
    out.put("answer", case, "response", "http", json!({ "status": status, "body": fix_ids(body, real, fixed) }));
}

fn guest_hidden(out: &mut Out, state: &str) {
    out.put(
        "guest",
        state,
        "hidden",
        "none",
        json!({
            "doc": null,
            "rule": "App guests never receive a permission card on any path: not on GET /cli/thread (page, delta), \
                     POST /cli/thread/catchup, WS /cli/overlay/events, WS /cli/thread/stream or \
                     GET /cli/thread/latest, whatever `cards` they send. The item is left out (a seq gap), \
                     never drawn as text.",
        }),
    );
}

// ── the scenarios ────────────────────────────────────────────────────

/// The single click card (`compute.use`, admin floor) up to `state`.
/// Returns the world, the real card id and the answer time to pin.
fn click_card(out: &mut Out, state: &str) -> (World, String, i64) {
    let w = billing();
    let id = if state.starts_with("already_done") { REQ_DONE } else { REQ };
    let req = row(&w, id, "compute.use", json!({}), None, Some(DANA));
    let card = ask(&w, &req);
    note(id, DANA, "member");
    let mut answered = ANSWERED;
    match state {
        "pending" => {
            // The errors a phone can meet on this card, before anyone answers.
            let (s, v) = answer_as(DANA, &w, &card, json!({ "answer": "Allow", "client": "phone" }));
            response(out, "below-floor", s, v, &card, CARD_ID);
            let (s, v) = answer_as(ROSSON, &w, &card, json!({ "answer": "Maybe", "client": "phone" }));
            response(out, "not-an-option", s, v, &card, CARD_ID);
        }
        "applying" => {
            assert!(store::cas_open_to(id, RequestState::Applying).expect("cas"), "pending → applying");
        }
        "approved" => {
            let v = answer_ok(ROSSON, &w, &card, json!({ "answer": "Allow", "client": "phone" }));
            response(out, "allow", 200, v, &card, CARD_ID);
            let (s, v) = answer_as(MAYA, &w, &card, json!({ "answer": "Allow", "client": "phone" }));
            response(out, "answered-already", s, v, &card, CARD_ID);
        }
        "already_done" => {
            set_behaviour(Behaviour::AlreadyOn);
        }
        "already_done-closed" => {
            set_behaviour(Behaviour::AlreadyOn);
            let v = answer_ok(ROSSON, &w, &card, json!({ "answer": "Close", "client": "phone" }));
            response(out, "close", 200, v, &card, CARD_ID);
        }
        "denied" => {
            let v = answer_ok(ROSSON, &w, &card, json!({ "answer": "Deny", "reason": "Not this week; the minis are busy", "client": "phone" }));
            response(out, "deny", 200, v, &card, CARD_ID);
        }
        "expired" => {
            let n = expiry::sweep_requests(overlay::now_secs() + 73 * 3600);
            assert_eq!(n, 1, "the expiry sweep expires the request");
            answered = EXPIRES;
        }
        "stale" => {
            set_behaviour(Behaviour::Moved);
            let v = answer_ok(ROSSON, &w, &card, json!({ "answer": "Allow", "client": "phone" }));
            response(out, "allow-stale", 200, v, &card, CARD_ID);
        }
        "failed" => {
            set_behaviour(Behaviour::Refused);
            let v = answer_ok(ROSSON, &w, &card, json!({ "answer": "Allow", "client": "phone" }));
            response(out, "allow-failed", 200, v, &card, CARD_ID);
        }
        "cancelled" => {
            let who = Who::Agent { project_id: w.ws.to_string(), session_id: SESSION.to_string() };
            let r = routes::handle_gated(routes::CANCEL, &who, true, json!({ "id": id }).to_string().as_bytes(), &HashMap::new());
            assert_eq!(r.status, "200 OK", "cancel: {}", r.body);
        }
        "closed" => {
            assert_eq!(answer::close_for_switch_off(Some(w.ws)), 1, "the switch-off close");
        }
        other => panic!("no click state {other}"),
    }
    pin(&w, &card, answered);
    (w, card, answered)
}

fn click_samples(out: &mut Out) {
    // Pending: every viewer.
    let (w, card, at) = click_card(out, "pending");
    let d = doc_of(&w, &card);
    both(out, "approver-click", "pending", &d, &rosson(), &card, CARD_ID, at);
    both(out, "approver-admin", "pending", &d, &maya(), &card, CARD_ID, at);
    both(out, "owner-token", "pending", &d, &Viewer::OwnerToken, &card, CARD_ID, at);
    both(out, "asker", "pending", &d, &dana(), &card, CARD_ID, at);
    both(out, "member", "pending", &d, &lee(), &card, CARD_ID, at);
    fallback(out, "agent", "pending", &d, &agent(&w), &card, CARD_ID, at);
    guest_hidden(out, "pending");
    frames(out, &w, &card, "pending");
    drop(w);

    let (w, card, at) = click_card(out, "applying");
    let d = doc_of(&w, &card);
    both(out, "approver-click", "applying", &d, &rosson(), &card, CARD_ID, at);
    both(out, "asker", "applying", &d, &dana(), &card, CARD_ID, at);
    drop(w);

    let (w, card, at) = click_card(out, "approved");
    let d = doc_of(&w, &card);
    both(out, "approver-click", "approved", &d, &rosson(), &card, CARD_ID, at);
    both(out, "approver-admin", "approved", &d, &maya(), &card, CARD_ID, at);
    both(out, "asker", "approved", &d, &dana(), &card, CARD_ID, at);
    both(out, "member", "approved", &d, &lee(), &card, CARD_ID, at);
    fallback(out, "agent", "approved", &d, &agent(&w), &card, CARD_ID, at);
    guest_hidden(out, "approved");
    frames(out, &w, &card, "approved");
    drop(w);

    for state in ["already_done", "already_done-closed", "denied", "expired", "stale", "failed", "cancelled", "closed"] {
        let (w, card, at) = click_card(out, state);
        let d = doc_of(&w, &card);
        both(out, "approver-click", state, &d, &rosson(), &card, CARD_ID, at);
        both(out, "asker", state, &d, &dana(), &card, CARD_ID, at);
        drop(w);
    }
}

/// `GET /cli/thread` item (also the delta's `items`/`changed[]` and
/// catchup) and the `WS /cli/overlay/events` frame, for the approver with
/// `cards=2`.
fn frames(out: &mut Out, w: &World, card: &str, state: &str) {
    let item = overlay::find_thread_item(w.conv, card).expect("read the item").unwrap_or_else(|| panic!("item {card}"));
    let seen = view::Seen::new(rosson(), k2_core::permission::CARDS_CONTRACT);
    let mut it = serde_json::to_value(seen.item(&item).as_ref()).expect("an item serializes");
    it["id"] = json!(CARD_ID);
    it["conversation_id"] = json!(w.conv_fixed);
    fix_doc(&mut it["doc"], card, CARD_ID, ANSWERED);
    out.put("approver-click", state, "item", "get", it);

    let frame = crate::overlay_ws::OverlayFrame {
        collection: "thread".to_string(),
        seq: item.seq,
        id: item.id.clone(),
        doc: Some(item.doc.clone()),
        activity: None,
        conversation_id: Some(w.conv.to_string()),
        rev: 1,
    };
    let wire = crate::overlay_ws::wire_json_for(&frame, None, &seen);
    let mut f: Value = serde_json::from_str(&wire).expect("a frame is JSON");
    f["id"] = json!(CARD_ID);
    fix_doc(&mut f["doc"], card, CARD_ID, ANSWERED);
    out.put("approver-click", state, "ws", "ws", f);
}

fn confirm_samples(out: &mut Out) {
    for state in ["pending", "approved"] {
        let w = shopdev();
        let req = row(&w, REQ_CONFIRM, "apps.approve", json!({ "request": HELD }), Some(HELD), None);
        let card = ask(&w, &req);
        if state == "pending" {
            let (s, v) = answer_as(ROSSON, &w, &card, json!({ "answer": "Allow", "client": "phone" }));
            response(out, "confirm-required", s, v, &card, CONFIRM_CARD_ID);
            let (s, v) = answer_as(MAYA, &w, &card, json!({ "answer": "Allow", "client": "phone" }));
            response(out, "admin-below-owner-floor", s, v, &card, CONFIRM_CARD_ID);
        } else {
            let v = answer_ok(ROSSON, &w, &card, json!({ "answer": "Allow", "confirmToken": "token-from-vault-confirm-finish", "client": "phone" }));
            response(out, "allow-confirmed", 200, v, &card, CONFIRM_CARD_ID);
        }
        pin(&w, &card, ANSWERED);
        let d = doc_of(&w, &card);
        both(out, "approver-confirm", state, &d, &rosson(), &card, CONFIRM_CARD_ID, ANSWERED);
        both(out, "admin-wait-confirm", state, &d, &maya(), &card, CONFIRM_CARD_ID, ANSWERED);
        drop(w);
    }
}

fn batch_samples(out: &mut Out) {
    for state in ["pending", "partial", "done"] {
        let w = billing();
        let r1 = row(&w, REQ_B1, "apps.build", json!({}), None, None);
        let r2 = row(&w, REQ_B2, "apps.core", json!({ "core": "db" }), None, None);
        let r3 = row(&w, REQ_B3, "apps.approve", json!({ "request": HELD_B3 }), Some(HELD_B3), None);
        let card = ask(&w, &r1);
        assert_eq!(ask(&w, &r2), card, "the second ask joins the first card (a batch)");
        assert_eq!(ask(&w, &r3), card, "the third ask joins the batch");
        if state != "pending" {
            let v = answer_ok(ROSSON, &w, &card, json!({ "answer": "Allow all", "client": "phone" }));
            if state == "partial" {
                response(out, "batch-allow-all", 200, v, &card, BATCH_CARD_ID);
            }
        }
        if state == "done" {
            let v = answer_ok(
                ROSSON,
                &w,
                &card,
                json!({ "item": HELD_B3, "answer": "Allow", "confirmToken": "token-from-vault-confirm-finish", "client": "phone" }),
            );
            response(out, "batch-item-confirmed", 200, v, &card, BATCH_CARD_ID);
        }
        pin(&w, &card, ANSWERED);
        let d = doc_of(&w, &card);
        both(out, "batch-approver", state, &d, &rosson(), &card, BATCH_CARD_ID, ANSWERED);
        both(out, "batch-admin", state, &d, &maya(), &card, BATCH_CARD_ID, ANSWERED);
        drop(w);
    }
}

/// The push a just-posted card sends (lane A2's `routing::push_card`, PRD
/// §17.7): rosson has a phone; the captured `PushEvent` rendered by the
/// relay payload builder (`K2Cloud::payload`, the legacy lane's `data`)
/// and the sealed lane's plaintext (`e2e::alert_plaintext`, before
/// sealing and padding). Fixed server stamp and seal context.
fn push_samples(out: &mut Out) {
    let w = billing();
    k2_core::push_devices::upsert_device("dev-rosson-phone", "ios", "apns-token-sample", ROSSON).expect("register rosson's phone");
    let mark = crate::push_routes::test_push_capture::mark();
    let req = row(&w, REQ, "compute.use", json!({}), None, Some(DANA));
    let card = ask(&w, &req);
    let events: Vec<k2_core::push::PushEvent> = crate::push_routes::test_push_capture::since(mark)
        .into_iter()
        .filter(|e| match e {
            k2_core::push::PushEvent::ThreadPost(t) => {
                t.conversation_id == w.conv
                    && matches!(&t.budget, k2_core::push::ThreadBudget::Permission { request_id, .. } if request_id == REQ)
            }
            _ => false,
        })
        .collect();
    assert_eq!(events.len(), 1, "one permission push, to rosson's phone only: {events:?}");
    let event = &events[0];
    let stamp = k2_core::push::PushStamp { subdomain: "rosson".to_string(), instance_id: "inst_01".to_string() };
    let badge_key = "rosson:inst_01";
    let legacy = k2_core::push::K2Cloud::payload(event, &stamp, badge_key);
    out.put("push", "permission", "legacy", "push", fix_ids(legacy, &card, CARD_ID));
    let ctx = k2_core::push::e2e::SealCtx {
        kid: "a1b2c3d4e5f60718".to_string(),
        ctr: 7,
        ts: T0,
        stamp: stamp.clone(),
        badge_key: badge_key.to_string(),
    };
    let sealed = k2_core::push::e2e::alert_plaintext(event, &ctx).expect("a Thread push has a sealed plaintext");
    out.put("push", "permission", "sealed-plaintext", "push", fix_ids(sealed, &card, CARD_ID));
    drop(w);
}

/// Every sample, as file name → pretty JSON (with a trailing newline).
fn generate() -> BTreeMap<String, String> {
    let _home = crate::test_support::TempHome::new();
    for (user, role) in [(ROSSON, Some(Role::Owner)), (MAYA, Some(Role::Admin)), (DANA, None), (LEE, None)] {
        connect_users::add_user(user, CAST_PASSWORD).expect("add a cast login");
        if let Some(r) = role {
            connect_users::set_role(user, r).expect("set the cast role");
        }
    }
    let _active = Active::on();
    let mut out = Out::default();
    click_samples(&mut out);
    confirm_samples(&mut out);
    batch_samples(&mut out);
    push_samples(&mut out);

    let cast = [ROSSON, MAYA, DANA, LEE];
    let mut files = BTreeMap::new();
    let mut index = Vec::new();
    for (name, s) in &out.files {
        let text = format!("{}\n", serde_json::to_string_pretty(&s.body).expect("pretty JSON"));
        for n in names_in(&s.body) {
            assert!(cast.contains(&n.as_str()), "{name}: approvers.names has {n}, outside the sample cast (another test wrote a login into this HOME?)");
        }
        index.push(json!({
            "file": name,
            "view": s.view,
            "state": s.state,
            "form": s.form,
            "transport": s.transport,
            "sha256": hex(&Sha256::digest(text.as_bytes())),
        }));
        files.insert(name.clone(), text);
    }
    let index = json!({
        "contract": "cards=2",
        "view": { "v": k2_core::permission::card::VIEW_VERSION },
        "featureKey": k2_core::permission::FEATURE_KEY,
        "writtenBy": "cargo test -p k2-daemon permission_samples (crates/k2-daemon/src/permission/permission_samples.rs)",
        "clock": { "askedAt": T0, "expiresAt": EXPIRES, "noteAt": NOTE_AT, "answeredAt": ANSWERED },
        "files": index,
    });
    files.insert("index.json".to_string(), format!("{}\n", serde_json::to_string_pretty(&index).expect("pretty JSON")));
    files
}

/// Every `approvers.names` entry in a sample.
fn names_in(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == "approvers" {
                    if let Some(ns) = x.get("names").and_then(Value::as_array) {
                        out.extend(ns.iter().filter_map(Value::as_str).map(str::to_string));
                    }
                }
                out.extend(names_in(x));
            }
        }
        Value::Array(a) => a.iter().for_each(|x| out.extend(names_in(x))),
        _ => {}
    }
    out
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/permission_cards")
}

/// The committed files (name → text), `index.json` included.
fn committed(dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd {
        let p = e.expect("read the golden dir").path();
        if p.extension().and_then(|x| x.to_str()) == Some("json") {
            let name = p.file_name().and_then(|n| n.to_str()).expect("a file name").to_string();
            out.insert(name, std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())));
        }
    }
    out
}

#[test]
fn permission_samples_match_the_golden_files() {
    let produced = generate();
    check_coverage(&produced);
    let dir = golden_dir();
    if std::env::var_os("UPDATE_PERMISSION_SAMPLES").is_some() {
        std::fs::create_dir_all(&dir).expect("create the golden dir");
        for name in committed(&dir).keys() {
            if !produced.contains_key(name) {
                std::fs::remove_file(dir.join(name)).expect("remove a sample that is no longer written");
            }
        }
        for (name, text) in &produced {
            std::fs::write(dir.join(name), text).expect("write a sample");
        }
        return;
    }
    let have = committed(&dir);
    let mut problems = Vec::new();
    for (name, text) in &produced {
        match have.get(name) {
            None => problems.push(format!("missing: {name}")),
            Some(t) if t != text => problems.push(format!("differs: {name}\n--- committed\n{t}--- produced\n{text}")),
            Some(_) => {}
        }
    }
    for name in have.keys() {
        if !produced.contains_key(name) {
            problems.push(format!("no longer written: {name}"));
        }
    }
    assert!(
        problems.is_empty(),
        "the permission-card wire changed without the golden samples ({} problems). If the change is intended, run \
         scripts/permission-cards/update-samples.sh, review the diff (it is the phone contract changing) and tell \
         Companion.\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// PRD §17.10's required set is there, and each file's state is the state
/// its view says.
fn check_coverage(produced: &BTreeMap<String, String>) {
    for (view, state, forms) in [
        ("approver-click", "pending", &["cards2", "fallback", "item", "ws"][..]),
        ("approver-click", "approved", &["cards2", "fallback", "item", "ws"][..]),
        ("approver-confirm", "pending", &["cards2", "fallback"][..]),
        ("approver-confirm", "approved", &["cards2", "fallback"][..]),
        ("asker", "pending", &["cards2", "fallback"][..]),
        ("asker", "approved", &["cards2", "fallback"][..]),
        ("member", "pending", &["cards2", "fallback"][..]),
        ("member", "approved", &["cards2", "fallback"][..]),
        ("approver-admin", "pending", &["cards2", "fallback"][..]),
        ("owner-token", "pending", &["cards2", "fallback"][..]),
        ("approver-click", "already_done", &["cards2", "fallback"][..]),
        ("approver-click", "already_done-closed", &["cards2", "fallback"][..]),
        ("approver-click", "denied", &["cards2", "fallback"][..]),
        ("approver-click", "expired", &["cards2", "fallback"][..]),
        ("approver-click", "stale", &["cards2", "fallback"][..]),
        ("approver-click", "failed", &["cards2", "fallback"][..]),
        ("approver-click", "closed", &["cards2", "fallback"][..]),
        ("approver-click", "cancelled", &["cards2", "fallback"][..]),
        ("batch-approver", "pending", &["cards2", "fallback"][..]),
        ("batch-approver", "done", &["cards2", "fallback"][..]),
        ("guest", "pending", &["hidden"][..]),
        ("guest", "approved", &["hidden"][..]),
        ("agent", "pending", &["fallback"][..]),
        ("push", "permission", &["legacy", "sealed-plaintext"][..]),
    ] {
        for form in forms {
            let name = format!("{view}__{state}__{form}.json");
            assert!(produced.contains_key(&name), "missing sample {name}");
        }
    }
    // The states the files claim are the states the views say.
    let state_of = |name: &str| -> Value {
        let v: Value = serde_json::from_str(&produced[name]).expect("JSON");
        v["choice"]["action"]["view"]["state"].clone()
    };
    for s in ["pending", "applying", "approved", "denied", "expired", "stale", "failed", "closed", "cancelled", "already_done"] {
        assert_eq!(state_of(&format!("approver-click__{s}__cards2.json")), json!(s), "approver-click {s}");
    }
    assert_eq!(state_of("approver-click__already_done-closed__cards2.json"), json!("already_done"));
    assert_eq!(state_of("approver-confirm__approved__cards2.json"), json!("approved"));
    let v: Value = serde_json::from_str(&produced["batch-approver__pending__cards2.json"]).expect("JSON");
    assert_eq!(v["choice"]["action"]["view"]["viewer"]["canAnswerAll"], json!(false), "a confirm item blocks Allow all: {v}");
    let v: Value = serde_json::from_str(&produced["asker__pending__cards2.json"]).expect("JSON");
    assert_eq!(v["choice"]["options"], json!([]), "a wait viewer gets no options: {v}");
    assert_eq!(v["choice"]["action"]["view"]["noteBox"]["mine"], json!(NOTE), "{v}");
}
