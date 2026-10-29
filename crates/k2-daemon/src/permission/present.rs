//! A1 — The card's words and its stored `view` (PRD rev 3.1 §4, §17.2),
//! the action lookup, who may answer, and posting/refreshing the card.
//!
//! **The stored view.** The card doc carries `choice.action.view`: the
//! `cards=2` view built from the request rows, with a *neutral* `viewer`
//! (`mode:"wait"`, no login). Lane A3's `project()` personalizes it per
//! viewer at the serve doors (viewer, mode, options, `noteBox`, the live
//! `already_done` check). [`view_for`] is the one builder both use, so the
//! stored doc and the projection can't drift.
//!
//! Every state change goes through [`refresh_card`]: it rebuilds the view
//! from the rows and writes it on the same doc id and seq (pending →
//! `update_action_view`; closed → `record_action_outcome`), then publishes
//! the frame.

use k2_core::connect_users::Role;
use k2_core::overlay::{self, CardAction, CardCallback};
use k2_core::permission::card::{
    Approvers, BatchState, BatchView, ItemView, KeyValue, Mode, NoteBox, PermissionView, ViewProof, Viewer, ViewerView,
    Why, AnswerView, UndoView, VIEW_VERSION,
};
use k2_core::permission::catalog::{ActionSpec, Direction, Floor, Grant, NotCardableSpec, ParamKind, Proof, Undo};
use k2_core::permission::request::{limits, PermissionRequest, RequestState};
use k2_core::permission::{label, CARD_ACTION_BATCH, CARD_ACTION_PERMISSION};
use serde_json::{json, Map, Value};

use super::actions::{self, Answerer, Ctx, Registered};
use super::store;

// ── the action lookup ───────────────────────────────────────────────

/// The registered action with this id (E-lanes' registry; under test,
/// also the test kit's actions).
pub(crate) fn find_action(id: &str) -> Option<Registered> {
    #[cfg(test)]
    if let Some(r) = super::testkit::find(id) {
        return Some(r);
    }
    actions::find(id)
}

/// The not-card-able row with this id (§5.4, §9).
pub(crate) fn find_not_cardable(id: &str) -> Option<&'static NotCardableSpec> {
    #[cfg(test)]
    if let Some(n) = super::testkit::not_cardable().into_iter().find(|n| n.id == id) {
        return Some(n);
    }
    actions::not_cardable().into_iter().find(|n| n.id == id)
}

/// Every card-able action (for `k2 permission list`).
pub(crate) fn all_actions() -> Vec<Registered> {
    #[allow(unused_mut)]
    let mut out = actions::all();
    #[cfg(test)]
    out.extend(super::testkit::all());
    out
}

// ── the asking side ─────────────────────────────────────────────────

/// The workspace's address (its agent's name on cards and turns). Falls
/// back to the id for a workspace that is gone.
pub(crate) fn workspace_handle(workspace_id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(workspace_id).unwrap_or_else(|_| workspace_id.to_string())
}

/// The request's `Ctx` (values as stored).
pub(crate) fn ctx_of(req: &PermissionRequest) -> Ctx {
    Ctx {
        workspace_id: req.workspace_id.clone(),
        workspace_handle: workspace_handle(&req.workspace_id),
        params: req.params.as_object().cloned().unwrap_or_default(),
    }
}

// ── words (§5.1; daemon-known names only, never the agent's --why) ──

/// The K2-written words of one request, rendered from the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Words {
    pub setting: String,
    pub what_changes: String,
    pub prompt: String,
    pub consequences: String,
    pub scope: String,
    pub duration: String,
    pub settings_page: String,
    pub settings_path: String,
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(value_text).collect::<Vec<_>>().join(", "),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Fill `{ws}`, `{agent}` and `{<param key>}`.
pub(crate) fn fill(template: &str, ctx: &Ctx) -> String {
    let mut out = template.replace("{ws}", &ctx.workspace_handle).replace("{agent}", &ctx.workspace_handle);
    for (k, v) in &ctx.params {
        out = out.replace(&format!("{{{k}}}"), &value_text(v));
    }
    out
}

fn sentence(s: &str) -> String {
    let t = s.trim();
    if t.ends_with(['.', '!', '?']) {
        t.to_string()
    } else {
        format!("{t}.")
    }
}

/// The words a person reads: [`words_raw`] with every Garden value
/// (`ParamKind::Garden`) shown by the Garden's **current** name (an id
/// whose Garden is gone stays the id), prd-garden-access-v1 §1.5a.
pub(crate) fn words(spec: &ActionSpec, ctx: &Ctx) -> Words {
    let gardens: Vec<&str> = spec.params.iter().filter(|p| p.kind == ParamKind::Garden).map(|p| p.key).collect();
    if gardens.is_empty() {
        return words_raw(spec, ctx);
    }
    let mut shown = ctx.clone();
    for key in gardens {
        if let Some(name) = shown.params.get(key).and_then(Value::as_str).and_then(crate::zen_garden_cards::shown_name) {
            shown.params.insert(key.to_string(), Value::String(name));
        }
    }
    words_raw(spec, &shown)
}

/// The card text hash fixed at request time and compared at the click
/// (C35), over the words with values **as stored** (a Garden's id, not its
/// name): a catalog wording change is `stale`, a Garden rename isn't.
/// Byte-identical to `words_hash(&words(..))` for every action without a
/// Garden value.
pub(crate) fn text_hash_of(spec: &ActionSpec, ctx: &Ctx) -> String {
    words_hash(&words_raw(spec, ctx))
}

/// The words with every value as stored, plus the values K2 computes for
/// an action itself (`zen.sign`'s `{changes}`: the version's manifest
/// changes in K2's words, never the agent's; `zen_sign_cards`).
fn words_raw(spec: &ActionSpec, ctx: &Ctx) -> Words {
    let computed = crate::zen_sign_cards::with_computed(spec.id, ctx).or_else(|| super::actions::sew::with_computed(spec.id, ctx));
    let ctx = computed.as_ref().unwrap_or(ctx);
    let what = fill(spec.words.what_changes, ctx);
    let what_trim = what.trim().trim_end_matches('.').to_string();
    // A setting label may name a value ("Edit {garden}"); none of the
    // switch labels has braces, so theirs are unchanged.
    let setting = fill(spec.words.setting, ctx);
    Words {
        prompt: format!("{} asks: {what_trim} ({setting}).", ctx.workspace_handle),
        setting,
        what_changes: sentence(&what),
        consequences: fill(spec.words.consequences, ctx),
        scope: fill(spec.words.scope, ctx),
        duration: spec.words.duration.to_string(),
        settings_page: fill(spec.settings.page, ctx),
        settings_path: fill(spec.settings.path, ctx),
    }
}

/// The card text hash fixed at request time (C35): a wording change
/// between the ask and the click makes the request `stale`.
pub(crate) fn words_hash(w: &Words) -> String {
    k2_core::permission::request::text_hash(&format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        w.setting, w.what_changes, w.prompt, w.consequences, w.scope, w.duration
    ))
}

// ── who may answer (§6.1; the catalog floor) ────────────────────────

/// The answerer meets the floor. GardenOwner is the owner person of that
/// Garden: the owner token or a login the owner linked, whatever its role
/// (§5.2.1, GP-B1: an unlinked Owner-role login is another person).
pub(crate) fn meets_floor(floor: Floor, who: &Answerer) -> bool {
    match who {
        Answerer::OwnerToken => true,
        Answerer::Login { user, role } => match floor {
            Floor::GardenOwner => crate::zen_routes::login_is_owner_person(user),
            Floor::Owner => role == "owner",
            Floor::Admin => role == "owner" || role == "admin",
        },
    }
}

/// A floor from its stored form (`owner`, `admin`, `garden-owner`).
pub(crate) fn floor_of(stored: &str) -> Floor {
    match stored {
        "admin" => Floor::Admin,
        "garden-owner" => Floor::GardenOwner,
        _ => Floor::Owner,
    }
}

/// The stored form of a floor (`garden-owner` kept apart from `owner`).
pub(crate) fn floor_str(f: Floor) -> &'static str {
    match f {
        Floor::Owner => "owner",
        Floor::Admin => "admin",
        Floor::GardenOwner => "garden-owner",
    }
}

fn role_str(r: Role) -> &'static str {
    match r {
        Role::Owner => "owner",
        Role::Admin => "admin",
        Role::Member => "member",
    }
}

/// An answerer from the Thread route's caller: the owner token, or a
/// Connect login with its role.
pub(crate) fn answerer_for(session_author: &str) -> Answerer {
    if crate::session_author::is_owner_token(session_author) {
        return Answerer::OwnerToken;
    }
    // A reserved-name login (made before 0.46.6) meets no floor.
    let role = if k2_core::connect_users::is_reserved_username(session_author) {
        "none"
    } else {
        k2_core::connect_users::role_for_user(session_author).map(role_str).unwrap_or("member")
    };
    Answerer::Login { user: session_author.to_string(), role: role.to_string() }
}

/// The Connect logins at the floor (enabled accounts): `approvers.names`
/// and the targeted push's audience (§6.7). Never includes the owner
/// token, which is not a login.
pub(crate) fn eligible_logins(floor: Floor) -> Vec<String> {
    let Ok(users) = k2_core::connect_users::list_users() else { return Vec::new() };
    users
        .into_iter()
        .filter(|u| !u.disabled && !k2_core::connect_users::is_reserved_username(&u.username))
        .filter(|u| meets_floor(floor, &Answerer::Login { user: u.username.clone(), role: role_str(u.role).to_string() }))
        .map(|u| u.username)
        .collect()
}

pub(crate) fn approvers(floor: Floor) -> Approvers {
    Approvers { floor: floor.as_str().to_string(), label: floor.label().to_string(), names: eligible_logins(floor) }
}

// ── the view ────────────────────────────────────────────────────────

/// The stored (neutral) viewer: no login, `wait`, can't answer. A3's
/// `project()` replaces it per viewer.
pub(crate) fn neutral_viewer() -> ViewerView {
    ViewerView {
        login: String::new(),
        role: String::new(),
        via: String::new(),
        mode: Mode::Wait,
        can_answer: false,
        is_asker: false,
        can_answer_all: None,
    }
}

/// `view.viewer` for a person (A3 calls this through [`view_for`]).
pub(crate) fn viewer_view(viewer: &Viewer, floor: Floor, asked_for: Option<&str>) -> ViewerView {
    let (login, role, via) = match viewer {
        Viewer::OwnerToken => ("owner".to_string(), "owner".to_string(), "owner-token"),
        Viewer::Login { user, role } => (user.clone(), role.clone(), "login"),
        Viewer::Guest { .. } | Viewer::Passport { .. } => return neutral_viewer(),
    };
    let answerer = match viewer {
        Viewer::OwnerToken => Answerer::OwnerToken,
        _ => Answerer::Login { user: login.clone(), role: role.clone() },
    };
    let can = meets_floor(floor, &answerer);
    ViewerView {
        is_asker: asked_for.is_some_and(|a| a == login),
        login,
        role,
        via: via.to_string(),
        mode: if can { Mode::Approve } else { Mode::Wait },
        can_answer: can,
        can_answer_all: None,
    }
}

fn decision_of(req: &PermissionRequest) -> Option<&'static str> {
    req.answered_at?;
    match req.state {
        RequestState::Approved | RequestState::Failed | RequestState::Stale => Some("allow"),
        RequestState::AlreadyDone => Some("close"),
        RequestState::Denied => Some("deny"),
        _ => None,
    }
}

/// One request's item view (§17.2.1), without a viewer.
pub(crate) fn item_view(req: &PermissionRequest, spec: &ActionSpec) -> ItemView {
    let ctx = ctx_of(req);
    let w = words(spec, &ctx);
    let floor = floor_of(&req.floor);
    let mut values = vec![KeyValue { key: "workspace".into(), value: ctx.workspace_handle.clone() }];
    for p in spec.params {
        if let Some(v) = ctx.params.get(p.key) {
            // A Garden shows by its current name, as in the words.
            let shown = match (p.kind, v.as_str()) {
                (ParamKind::Garden, Some(id)) => crate::zen_garden_cards::shown_name(id).unwrap_or_else(|| id.to_string()),
                _ => value_text(v),
            };
            values.push(KeyValue { key: p.key.to_string(), value: shown });
        }
    }
    let why_text = req.updated_why.clone().unwrap_or_else(|| req.why.clone());
    let answer = decision_of(req).map(|d| AnswerView {
        decision: d.to_string(),
        by: req.answered_by.clone().unwrap_or_default(),
        via: req.answered_via.clone().unwrap_or_default(),
        at: req.answered_at.unwrap_or_default(),
        reason: req.reason.clone(),
    });
    let undo = match (req.state, spec.grant, spec.undo, req.answered_at) {
        // Once turned off again from the card (lane A2), no second undo.
        (RequestState::Approved, Grant::Standing, Undo::Twin { .. }, Some(at)) if !super::undo::is_undone(req) => {
            Some(UndoView { label: label::TURN_OFF_AGAIN.to_string(), until: at + limits::TURN_OFF_AGAIN_SECS })
        }
        _ => None,
    };
    let proof = match spec.proof {
        Proof::Click => ViewProof::Click,
        Proof::SettingsConfirm { action } => ViewProof::Confirm { action, secret_id: req.display_id().to_string() },
    };
    ItemView {
        request_id: req.display_id().to_string(),
        state: req.state,
        action: spec.id.to_string(),
        setting: w.setting,
        what_changes: w.what_changes,
        direction: spec.direction,
        grant: spec.grant,
        scope: w.scope,
        duration: w.duration,
        consequences: w.consequences,
        values,
        why: Some(Why { by: ctx.workspace_handle.clone(), text: why_text }),
        notes: store::notes(req.id.as_str()).unwrap_or_default(),
        asked_for: req.asked_for.clone(),
        askers: None,
        approvers: approvers(floor),
        proof,
        expires_at: req.expires_at,
        settings_page: w.settings_page,
        note_box: NoteBox { allowed: false, max: limits::NOTE_MAX_CHARS, mine: None },
        answer,
        undo,
    }
}

/// The `view` JSON for the requests on one card: a `permission` view for
/// one request, a `batch` view for several. `viewer`: None for the stored
/// (neutral) view; a person for A3's projection.
pub(crate) fn view_for(reqs: &[PermissionRequest], batch: bool, viewer: Option<&Viewer>) -> Option<Value> {
    let first = reqs.first()?;
    if !batch {
        let r = find_action(&first.action)?;
        let item = item_view(first, r.spec);
        let v = match viewer {
            Some(v) => viewer_view(v, floor_of(&first.floor), first.asked_for.as_deref()),
            None => neutral_viewer(),
        };
        let view = PermissionView { v: VIEW_VERSION, kind: "permission".into(), viewer: v, item };
        return serde_json::to_value(view).ok();
    }
    let mut items = Vec::new();
    let mut confirm_pending = false;
    for req in reqs {
        let Some(r) = find_action(&req.action) else { continue };
        if req.is_answerable() && !r.spec.proof.batchable() {
            confirm_pending = true;
        }
        items.push(item_view(req, r.spec));
    }
    let floor = floor_of(&first.floor);
    let any_open = reqs.iter().any(|r| r.is_answerable() || r.state == RequestState::Applying);
    let mut vv = match viewer {
        Some(v) => viewer_view(v, floor, first.asked_for.as_deref()),
        None => neutral_viewer(),
    };
    vv.can_answer_all = Some(vv.can_answer && any_open && !confirm_pending);
    let expires_at = reqs.iter().map(|r| r.expires_at).min().unwrap_or(first.expires_at);
    let view = BatchView {
        v: VIEW_VERSION,
        kind: "batch".into(),
        state: if any_open { BatchState::Pending } else { BatchState::Done },
        viewer: vv,
        items,
        approvers: approvers(floor),
        expires_at,
    };
    serde_json::to_value(view).ok()
}

/// `choice.action.result` for a closed single request.
pub(crate) fn result_for(req: &PermissionRequest) -> Value {
    json!({ "message": req.result_message.clone().unwrap_or_default(), "outcome": req.state.as_str() })
}

/// The card's prompt and options for its requests (single or batch).
pub(crate) fn prompt_and_options(reqs: &[PermissionRequest], batch: bool) -> (String, Vec<String>) {
    if !batch {
        let first = &reqs[0];
        let prompt = find_action(&first.action)
            .map(|r| words(r.spec, &ctx_of(first)).prompt)
            .unwrap_or_else(|| format!("{} asks for a permission.", workspace_handle(&first.workspace_id)));
        let mut options = vec![label::ALLOW.to_string(), label::DENY.to_string()];
        if crate::apps::held_cards::offers_always(first) {
            options.push(label::ALWAYS.to_string());
        }
        return (prompt, options);
    }
    let ws = workspace_handle(&reqs[0].workspace_id);
    (format!("{ws} asks for {} permissions.", reqs.len()), vec![label::ALLOW_ALL.to_string(), label::DENY_ALL.to_string()])
}

/// The card action for its requests.
pub(crate) fn card_action(reqs: &[PermissionRequest], batch: bool, dedupe: String) -> CardAction {
    let (id, params) = if batch {
        (CARD_ACTION_BATCH, json!({ "requests": reqs.iter().map(|r| r.display_id()).collect::<Vec<_>>() }))
    } else {
        (CARD_ACTION_PERMISSION, json!({ "request": reqs[0].display_id() }))
    };
    CardAction { id: id.to_string(), params, dedupe, result: None, view: view_for(reqs, batch, None) }
}

/// A single card's dedupe key: `<action>:<first 8 of the values hash>`.
pub(crate) fn single_dedupe(req: &PermissionRequest) -> String {
    format!("{}:{}", req.action, &req.params_hash[..req.params_hash.len().min(8)])
}

/// Rebuild the stored view of a card from its rows and write it on the
/// same id and seq; publishes the frame. A pending card is closed here
/// when none of its requests is open any more: `answer` labels it
/// (answered) or, with `void`, it is voided (cancelled / closed). Returns
/// the card's callback when it changed.
pub(crate) fn refresh_card(conversation_id: &str, card_id: &str, close: CardClose<'_>) -> Option<CardCallback> {
    let reqs = store::for_card(conversation_id, card_id).ok()?;
    if reqs.is_empty() {
        return None;
    }
    let action = overlay::card_action(conversation_id, card_id).ok().flatten()?;
    let batch = action.id == CARD_ACTION_BATCH;
    let view = view_for(&reqs, batch, None)?;
    let doc = overlay::get_thread_doc(conversation_id, card_id).ok().flatten()?;
    let pending = doc.is_pending_choice();
    let any_open = reqs.iter().any(|r| r.is_answerable() || r.state == RequestState::Applying);
    let result = if batch {
        let done = reqs.iter().filter(|r| !r.is_answerable() && r.state != RequestState::Applying).count();
        json!({ "message": format!("{done} of {} answered.", reqs.len()), "outcome": if any_open { "pending" } else { "done" } })
    } else {
        result_for(&reqs[0])
    };
    let cb = if pending && any_open {
        overlay::update_action_view(conversation_id, card_id, view).ok()?
    } else {
        if pending {
            let project_id = reqs[0].workspace_id.clone();
            let closed = match close {
                CardClose::Answer { label, by } => overlay::close_action_card(conversation_id, card_id, label, by),
                CardClose::Void => overlay::void_card(conversation_id, &project_id, card_id, None),
                CardClose::Expire => overlay::expire_card(conversation_id, card_id),
            };
            if let Err(e) = closed {
                k2_core::log_debug!("[permission] close card {card_id}: {e}");
            }
        }
        overlay::record_action_outcome(conversation_id, card_id, Some(result), view).ok()?
    };
    crate::overlay_routes::publish_action_frame(&cb);
    Some(cb)
}

/// How [`refresh_card`] closes a pending card whose requests are all done.
pub(crate) enum CardClose<'a> {
    /// A person answered (`Allow`, `Deny`, `Close`, `Allow all`…).
    Answer { label: &'a str, by: Option<overlay::PostAuthor> },
    /// The daemon withdrew it (cancel, switch-off close): `voided`.
    Void,
    /// Its time ran out: `expired`.
    Expire,
}

/// The words a refusal or a list shows for a direction.
pub(crate) fn on_word(d: Direction) -> &'static str {
    match d {
        Direction::Loosen => "on",
        Direction::Tighten => "off",
    }
}

/// `{key: value}` of a JSON object (empty for anything else).
pub(crate) fn as_map(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}
