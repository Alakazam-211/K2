//! Garden access: one list per Garden of the agents who may edit it
//! (prd-garden-access-v1 rev 5, slice A2: G9–G14).
//!
//! **Everything here runs only with the `gardenAccess` switch on**
//! (`k2_core::zen::garden_access::enabled`, default off). Off, the new
//! routes have no policy row (`route_policy::live_row`) and fall to the
//! generic Zen arm's 404, and `zen_routes` / `zen_agent_routes` take their
//! 0.46.3 paths. The branches sit at the route edges.
//!
//! - **The resolver (G10)**, [`garden_access_may`]: the owner token and an
//!   Owner login edit; a live local passport (any session in a listed
//!   workspace's room: the pinned chat, another harness tab, a sidecar cell,
//!   a plain shell tab) edits when its workspace is listed, else asks; a
//!   passport whose session is gone is refused (`session_gone`).
//! - **The passport gate (G11, G12)**, [`passport_gate`]: replaces
//!   `zen_routes`' passport block. Garden writes: the lock first (423, as
//!   today), then the list; an unlisted agent gets 403
//!   `garden_access_needed` and K2's `garden.access` card. `garden/delete`
//!   is the person's only: 403 `garden_delete_person` and the
//!   `garden.delete` card. Widget writes need the list on **every** Garden
//!   that places the widget (a draft placed nowhere is open). The
//!   computer-wide look (`reset` of `zen.toml` or a theme, `theme/*` with no
//!   Garden) needs the list on at least one Garden. Proposals and the claim
//!   are retired (410 `retired`); `widget/preview` opens.
//! - **Making a Garden (G13)**, [`garden_new`]: any live local passport, no
//!   card needed; `page` and `widgets` land in the same call; 5 a day
//!   (429 `garden_budget`); the maker is recorded (`zen_garden_made`), never
//!   listed, and K2 presents the `garden.access` card in the same step.
//! - **Stamps (G9)**, [`stamp_after_write`]: every daemon widget write
//!   stamps its writer; the person, or an agent on every placing Garden's
//!   list, vouches (overwrites anyone else's stamp).
//! - **History (G14, G29 events)**: one `zen_garden_events` line per act
//!   ([`after_write`], [`record`]); `GET garden/history` and
//!   `widget/history` read them (the person only).
//! - **The list door (G14)**: [`list_change`] adds or removes, for Settings
//!   and the CLI (through A4's `zen_garden_cards::access_entry`, the one
//!   entry the card and its twin share).
//!
//! The cards themselves (words, the asks store, dedupe, `delete_entry`)
//! are slice A4's (`crate::zen_garden_cards`); the `deleted` line and the
//! G8 mirror are slice A1's delete hook.

use std::collections::{BTreeSet, HashMap};

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::garden_data::authors::{self, CallerKind, StampOutcome, StampSource};
use k2_core::garden_data::{DataError, MadeBy, Space};
use k2_core::zen::garden_access::{self as ga, Actor, Change, EventKind, On, Via};
use k2_core::zen::widget_folder::{self, StagedFile};
use k2_core::zen::widget_store::WidgetState;
use k2_core::zen::{ZenError, ZenFile, ZenFiles};

use crate::cli_response::CliResponse;
use crate::zen_garden_cards::{self as cards, GardenAsk};
use crate::sidecar_routes::Caller;

/// `GET` the list (the person) or whether you're on it (a passport);
/// `POST {garden, agent, allow}` changes it (the person).
pub const ACCESS_ROUTE: &str = "/cli/zen/garden/access";
/// `POST {garden, reason?}`: a live local passport asks to edit.
pub const ASK_ROUTE: &str = "/cli/zen/garden/access/ask";
/// `GET ?garden=&before=&limit=&kind=`: a Garden's History (the person).
pub const GARDEN_HISTORY_ROUTE: &str = "/cli/zen/garden/history";
/// `GET ?widget=&before=&limit=&kind=`: a widget's History (the person).
pub const WIDGET_HISTORY_ROUTE: &str = "/cli/zen/widget/history";

/// `GET`: Gardens that live only in history (the person).
pub const DELETED_ROUTE: &str = "/cli/zen/gardens/deleted";
/// `POST {garden}`: put a deleted Garden back (the person).
pub const RESTORE_ROUTE: &str = "/cli/zen/garden/restore";
/// `GET ?file=&name=`: one kept version, read only (the person).
pub const SNAPSHOT_ROUTE: &str = "/cli/zen/snapshot";

/// This module's routes (their own dispatcher arm, ahead of the Zen arm).
pub const ROUTES: &[&str] = &[
    ACCESS_ROUTE,
    ASK_ROUTE,
    GARDEN_HISTORY_ROUTE,
    WIDGET_HISTORY_ROUTE,
    DELETED_ROUTE,
    RESTORE_ROUTE,
    SNAPSHOT_ROUTE,
];

/// POST-only rows (a GET is 405).
pub const POST_ONLY: &[&str] = &[ASK_ROUTE, RESTORE_ROUTE];

/// History lines per page by default, and at most.
pub const HISTORY_PAGE: usize = 50;
pub const HISTORY_MAX: usize = 200;

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

/// Whether the route tables may answer `path` now: every path that isn't
/// this module's, and this module's only with the switch on (off, they
/// answer exactly as a path with no row does at 0.46.3).
pub fn route_live(path: &str) -> bool {
    !is_route(path) || ga::enabled()
}

fn resp(status: &'static str, body: J) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: body.to_string() }
}

fn refuse(status: &'static str, code: &str, message: impl Into<String>) -> CliResponse {
    resp(status, json!({ "ok": false, "error": code, "message": message.into() }))
}

fn files() -> ZenFiles {
    ZenFiles::local()
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn short(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

fn handle_of(project_id: &str) -> String {
    crate::compute_routes::ws_handle(project_id)
}

// ── who ──────────────────────────────────────────────────────────────

/// The History actor for a caller (`None`: it can't act on Gardens).
pub(crate) fn actor_of(caller: &Caller) -> Option<Actor> {
    match caller {
        Caller::Owner => Some(Actor::Owner),
        Caller::Login { username } => Some(Actor::Login(username.clone())),
        Caller::Agent(a) if !a.sandboxed => Some(Actor::Agent(a.project_id.clone())),
        Caller::Shell { project_id } => Some(Actor::Shell(project_id.clone())),
        _ => None,
    }
}

/// A live local passport's workspace, and whether it is a plain shell tab.
/// `Err(code)`: a passport whose session is gone (`session_gone`), or not
/// a local passport at all (`zen_local_only`).
pub(crate) fn passport_workspace(caller: &Caller) -> Result<(String, bool), &'static str> {
    match caller {
        Caller::Agent(a) if a.sandboxed => Err("zen_local_only"),
        Caller::Agent(a) if a.map_key.is_none() => Err("session_gone"),
        Caller::Agent(a) => Ok((a.project_id.clone(), false)),
        Caller::Shell { project_id } => Ok((project_id.clone(), true)),
        _ => Err("zen_local_only"),
    }
}

fn session_gone() -> CliResponse {
    refuse(
        "403 Forbidden",
        "session_gone",
        "This session has ended, so it can't change Gardens. Open a new session in the room and run it again.",
    )
}

fn passport_refusal(code: &str) -> CliResponse {
    if code == "session_gone" {
        session_gone()
    } else {
        crate::zen_routes::local_only()
    }
}

/// What G10 answers for a caller and a Garden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum May {
    /// The person, or a listed agent.
    Edit,
    /// A live local passport that isn't listed: it may ask.
    Ask,
    /// Anyone else (`zen_local_only`, `session_gone`).
    Refused(&'static str),
}

fn is_listed(garden: &str, project_id: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match ga::is_listed(&conn, garden, project_id) {
        Ok(b) => b,
        Err(e) => {
            k2_core::log_debug!("[zen/access] is_listed {garden} {project_id}: {e:?}; failing closed");
            false
        }
    }
}

/// **G10, the one resolver.** May `caller` edit Garden `garden` (an id)?
pub(crate) fn garden_access_may(caller: &Caller, garden: &str) -> May {
    if crate::zen_routes::garden_owner_may(caller, Some(garden)) {
        return May::Edit;
    }
    match passport_workspace(caller) {
        Ok((p, _)) if is_listed(garden, &p) => May::Edit,
        Ok(_) => May::Ask,
        Err(code) => May::Refused(code),
    }
}

/// Is `project_id` on the list of `garden`, or its maker? (Who may ask to
/// delete it, §1.5a.)
pub(crate) fn may_ask_delete(project_id: &str, garden: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let listed = ga::is_listed(&conn, garden, project_id).unwrap_or(false);
    let maker = matches!(ga::made(&conn, garden), Ok(Some(m)) if m.made_by_project == project_id);
    listed || maker
}

/// The Gardens (id, name) whose live page places widget folder `widget`.
pub(crate) fn placing_gardens(f: &ZenFiles, widget: &str) -> Vec<(String, String)> {
    let mut seen = BTreeSet::new();
    f.custom_placements()
        .into_iter()
        .filter(|p| p.widget == widget)
        .filter(|p| seen.insert(p.garden.clone()))
        .map(|p| (p.garden, p.garden_name))
        .collect()
}

/// G15 agent data door: is `project_id` listed on every Garden that places
/// the widget whose `data:` space this is (and is it placed at all)?
pub(crate) fn agent_edits_widget_data(space: &Space, project_id: &str) -> Result<bool, DataError> {
    if !space.is_data() || authors::is_builtin(space.widget()) {
        return Ok(false);
    }
    let f = files();
    if !f.is_set_up() {
        return Ok(false);
    }
    let gardens: Vec<String> = placing_gardens(&f, space.widget()).into_iter().map(|(g, _)| g).collect();
    if gardens.is_empty() {
        return Ok(false);
    }
    let refs: Vec<&str> = gardens.iter().map(String::as_str).collect();
    let db = k2_core::db::shared();
    let conn = db.lock();
    ga::listed_on_all(&conn, &refs, project_id).map_err(|e| DataError::Unavailable(format!("garden access list: {e:?}")))
}

// ── History ──────────────────────────────────────────────────────────

/// Append one History line as `caller` (switch on only; a store error is
/// logged, never fails the write that already happened).
pub(crate) fn record(on: On<'_>, kind: EventKind, caller: &Caller, detail: J) {
    if !ga::enabled() {
        return;
    }
    let Some(actor) = actor_of(caller) else { return };
    record_as(on, kind, &actor, detail);
}

fn record_as(on: On<'_>, kind: EventKind, actor: &Actor, detail: J) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = ga::record_event(&conn, on, kind, actor, &detail, now()) {
        k2_core::log_debug!("[zen/access] history {} {:?}: {e:?}", kind.as_str(), on);
    }
}

/// The History line for a `zen_routes` write that answered `v` (switch on).
/// Writes nothing for a no-op (`changed: false`).
pub(crate) fn after_write(path: &str, caller: &Caller, body: &[u8], v: &J) {
    if !ga::enabled() || v["changed"] == J::Bool(false) {
        return;
    }
    let b: J = serde_json::from_slice(body).unwrap_or(J::Null);
    let garden_id = v["garden"]["id"].as_str().or_else(|| v["garden"].as_str()).map(str::to_string);
    match path {
        "/cli/zen/garden/rename" => {
            if let Some(g) = garden_id.as_deref() {
                record(On::Garden(g), EventKind::Renamed, caller, json!({ "to": v["garden"]["name"] }));
            }
        }
        "/cli/zen/garden/reorder" => {
            if let Some(g) = resolve_id(b["garden"].as_str()) {
                record(On::Garden(&g), EventKind::Reordered, caller, json!({ "to": b["to"] }));
            }
        }
        "/cli/zen/garden/template" => {
            if let Some(g) = garden_id.as_deref() {
                record(On::Garden(g), EventKind::Template, caller, json!({ "template": v["template"], "snapshot": v["snapshot"] }));
            }
        }
        "/cli/zen/reset" => {
            let detail = json!({ "file": v["file"], "restored": v["restored"], "snapshot": v["snapshot"] });
            if let Some(w) = b["widget"].as_str() {
                // G9: the resetter stamps the restored version (a listed
                // agent or the person vouches; anyone else only an unsigned
                // version, which then runs with Always only).
                let stamp = stamp_after_write(&files(), w.trim(), caller, StampSource::Sign);
                let mut detail = detail;
                detail["hash"] = stamp["hash"].clone();
                record(On::Widget(w.trim()), EventKind::Reset, caller, detail);
            } else if let Some(g) = garden_of_file(v["file"].as_str()) {
                record(On::Garden(&g), EventKind::Reset, caller, detail);
            } else {
                record(On::Computer, EventKind::Reset, caller, detail);
            }
        }
        "/cli/zen/theme/set" | "/cli/zen/theme/next" | "/cli/zen/theme/prev" => {
            let detail = json!({ "theme": v["theme"], "scope": v["scope"] });
            match v["garden"].as_str().or_else(|| v["garden"]["id"].as_str()) {
                Some(g) => record(On::Garden(g), EventKind::Theme, caller, detail),
                None => record(On::Computer, EventKind::Theme, caller, detail),
            }
        }
        "/cli/zen/widget/new" => {
            if let Some(w) = b["name"].as_str() {
                record(On::Widget(w.trim()), EventKind::Made, caller, json!({ "from": b["from"] }));
            }
        }
        // A6 (G29): Lock and design updates are the person's (owner token);
        // `changed: false` (already so) wrote nothing above.
        crate::zen_routes::LOCK_ROUTE => {
            let g = v["garden"]["id"].as_str().map(str::to_string).or_else(|| resolve_id(b["garden"].as_str()));
            if let Some(g) = g {
                let kind = if v["locked"] == J::Bool(true) { EventKind::Locked } else { EventKind::Unlocked };
                record(On::Garden(&g), kind, caller, json!({ "widgets": v["widgets"] }));
            }
        }
        "/cli/zen/garden/sync" => {
            let g = v["garden"]["id"].as_str().map(str::to_string).or_else(|| resolve_id(b["garden"].as_str()));
            if let Some(g) = g {
                let detail = json!({ "sync": b["sync"], "part": b["part"], "undo": b["undo"], "keep": b["keep"] });
                record(On::Garden(&g), EventKind::Sync, caller, detail);
            }
        }
        _ => {}
    }
}

fn resolve_id(sel: Option<&str>) -> Option<String> {
    files().find_garden(sel?.trim()).map(|(_, g)| g.id)
}

fn garden_of_file(file: Option<&str>) -> Option<String> {
    match ZenFile::parse(file?) {
        Ok(ZenFile::Garden(id)) => Some(id),
        _ => None,
    }
}

// ── stamps (G9) ──────────────────────────────────────────────────────

/// The maker a caller signs as: the person (owner token or Owner login),
/// or the workspace of a local agent or shell passport.
pub(crate) fn signer_of(caller: &Caller) -> Option<(MadeBy, CallerKind)> {
    match caller {
        Caller::Owner => Some((MadeBy::Owner, CallerKind::Owner)),
        Caller::Login { .. } if crate::zen_routes::garden_owner_may(caller, None) => Some((MadeBy::Owner, CallerKind::Owner)),
        Caller::Agent(a) if !a.sandboxed => Some((MadeBy::Agent { project_id: a.project_id.clone() }, CallerKind::Agent)),
        Caller::Shell { project_id } => Some((MadeBy::Agent { project_id: project_id.clone() }, CallerKind::Shell)),
        _ => None,
    }
}

/// May this signer vouch for widget `widget` (overwrite another stamp)? The
/// person always; an agent only when listed on every Garden that places it
/// (and it is placed somewhere).
fn may_vouch(f: &ZenFiles, widget: &str, maker: &MadeBy) -> bool {
    match maker {
        MadeBy::Owner => true,
        MadeBy::Agent { project_id } => {
            let gardens: Vec<String> = placing_gardens(f, widget).into_iter().map(|(g, _)| g).collect();
            if gardens.is_empty() {
                return false;
            }
            let refs: Vec<&str> = gardens.iter().map(String::as_str).collect();
            let db = k2_core::db::shared();
            let conn = db.lock();
            ga::listed_on_all(&conn, &refs, project_id).unwrap_or(false)
        }
        _ => false,
    }
}

/// Stamp `widget`'s live clean version as `caller` (switch on). A voucher
/// overwrites; anyone else stamps only an unsigned version. Returns `{hash,
/// madeBy, stamped, vouched}`, or null when there's nothing to stamp (a
/// built-in, a folder with errors, a caller that can't sign).
pub(crate) fn stamp_after_write(f: &ZenFiles, widget: &str, caller: &Caller, source: StampSource) -> J {
    if !ga::enabled() || authors::is_builtin(widget) {
        return J::Null;
    }
    let Some((maker, kind)) = signer_of(caller) else { return J::Null };
    let st = f.widget_status(widget);
    if st.state != WidgetState::Ok || st.hash().is_empty() {
        return J::Null;
    }
    let hash = st.hash().to_string();
    let vouch = may_vouch(f, widget, &maker);
    let out = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let t = now();
        let r = if vouch {
            authors::vouch(&conn, widget, &hash, &maker, source, kind, t)
        } else {
            authors::stamp(&conn, widget, &hash, &maker, source, kind, t)
        };
        r.and_then(|o| authors::maker_of(&conn, widget, &hash).map(|m| (o, m)))
    };
    match out {
        Ok((o, now_maker)) => json!({
            "hash": short(&hash),
            "madeBy": crate::garden_data::call::made_by_wire(&now_maker),
            "stamped": o == StampOutcome::Stamped,
            "vouched": vouch,
        }),
        Err(e) => {
            k2_core::log_debug!("[zen/access] stamp {widget}: {e}");
            J::Null
        }
    }
}

// ── refusals and cards ───────────────────────────────────────────────

/// K2's card (A4), as the value of an answer's `card` field.
fn present(kind: GardenAsk, garden_id: &str, project: &str, made: bool, explicit: bool) -> J {
    cards::present(kind, project, garden_id, made, explicit)
}

/// Did `present` put a card out (a new one or one already waiting)?
fn has_card(card: &J) -> bool {
    !card["card"].is_null()
}

/// 403 `garden_access_needed` naming Garden `(id, name)`, with K2's card
/// (the waiting one, a new one on a first refusal, or null).
pub(crate) fn access_needed(caller: &Caller, garden_id: &str, garden_name: &str, path: &str, ingress: &str) -> CliResponse {
    let card = match passport_workspace(caller) {
        Ok((p, _)) => present(GardenAsk::Access, garden_id, &p, false, false),
        Err(_) => J::Null,
    };
    crate::zen_routes::audit_refusal("zen.garden.access_needed", caller, format!("{path}: garden {garden_id}"), ingress);
    let message = if !has_card(&card) {
        format!(
            "You're not on the list of agents who may edit {garden_name}. Ask the person once: k2 zen garden access \"{garden_name}\" ask"
        )
    } else {
        format!(
            "You're not on the list of agents who may edit {garden_name}, so K2 asked the person (a card on their Thread with you). After they Allow, run this again."
        )
    };
    resp(
        "403 Forbidden",
        json!({ "ok": false, "error": "garden_access_needed", "garden": garden_id, "name": garden_name, "card": card, "message": message }),
    )
}

/// 403 `garden_access_needed` with `garden: null`: the computer-wide look
/// needs an agent on at least one Garden's list (Q6).
fn look_needs_a_list(caller: &Caller, path: &str, ingress: &str) -> CliResponse {
    crate::zen_routes::audit_refusal("zen.garden.access_needed", caller, format!("{path}: computer-wide look"), ingress);
    resp(
        "403 Forbidden",
        json!({
            "ok": false,
            "error": "garden_access_needed",
            "garden": J::Null,
            "name": J::Null,
            "card": J::Null,
            "message": "Changing the look of every Garden needs an agent that edits at least one Garden. Ask for access to a Garden first: k2 zen garden access <garden> ask",
        }),
    )
}

/// 403 `garden_delete_person` with K2's `garden.delete` card (§1.5a).
fn delete_person(caller: &Caller, garden_id: &str, garden_name: &str, ingress: &str) -> CliResponse {
    let card = match passport_workspace(caller) {
        Ok((p, _)) => present(GardenAsk::Delete, garden_id, &p, false, true),
        Err(_) => J::Null,
    };
    crate::zen_routes::audit_refusal("zen.garden.delete_person", caller, format!("garden {garden_id}"), ingress);
    let message = if !has_card(&card) {
        format!("Only the person deletes a Garden. Ask them to delete {garden_name} (Settings → Gardens), or wait for their answer to your last ask.")
    } else {
        format!("Only the person deletes a Garden. K2 asked them to delete {garden_name} (a card on their Thread with you).")
    };
    resp(
        "403 Forbidden",
        json!({ "ok": false, "error": "garden_delete_person", "garden": garden_id, "name": garden_name, "card": card, "message": message }),
    )
}

/// 410 `retired`: Keep as mine, the claim and proposals, with the switch on.
pub(crate) fn retired(what: &str) -> CliResponse {
    resp(
        "410 Gone",
        json!({
            "ok": false,
            "error": "retired",
            "message": format!("{what} is retired: Gardens use access lists now. An agent on a Garden's list edits it (k2 zen garden access <garden>); others ask with k2 zen garden access <garden> ask."),
        }),
    )
}

// ── the passport gate (G11, G12) ─────────────────────────────────────

/// Routes a passport may use with no Garden check (they write nothing a
/// Garden runs, or decide for themselves): checks, shots, the dev loop's
/// preview (Always only, §1.7), a draft folder, signing, runtime errors, a
/// proposal list, and a Garden of its own (G13).
const OPEN_TO_PASSPORTS: &[&str] = &[
    "/cli/zen/garden/check",
    "/cli/zen/garden/new",
    "/cli/zen/proposals",
    "/cli/zen/proposals/reject",
    "/cli/zen/proposals/show",
    "/cli/zen/widget/errors/clear",
    "/cli/zen/widget/folder/check",
    "/cli/zen/widget/new",
    "/cli/zen/widget/preview",
    "/cli/zen/widget/shot",
    "/cli/zen/widget/sign",
];

/// Garden writes named by `garden` (or `file` for reset).
const GARDEN_WRITES: &[&str] = &[
    "/cli/zen/garden/put",
    "/cli/zen/garden/rename",
    "/cli/zen/garden/reorder",
    "/cli/zen/garden/template",
];

fn body_json(body: &[u8]) -> J {
    if body.iter().all(u8::is_ascii_whitespace) {
        return json!({});
    }
    serde_json::from_slice(body).unwrap_or(J::Null)
}

/// The Garden (id, name) a selector names; `None` when there's no such
/// Garden (the route's own handler then answers its 404 / 400).
fn garden_of(f: &ZenFiles, sel: &str) -> Option<(String, String)> {
    f.find_garden(sel.trim()).map(|(_, g)| (g.id, g.name))
}

/// 404 `unknown_garden` for selector `sel` (the same body as `zen_routes`).
fn unknown_garden(f: &ZenFiles, sel: &str) -> CliResponse {
    crate::zen_routes::error_response(ZenError::UnknownGarden {
        garden: sel.to_string(),
        known: f.gardens().into_iter().map(|g| g.id).collect(),
    })
}

/// The lock check, then G10, for a write that names Garden `sel`.
fn garden_write(f: &ZenFiles, sel: &str, caller: &Caller, path: &str, ingress: &str) -> Option<CliResponse> {
    if let Err(e) = f.refuse_if_locked(sel) {
        crate::zen_routes::audit_refusal("zen.garden.lock_refused", caller, format!("{path}: garden {sel} is locked"), ingress);
        return Some(crate::zen_routes::error_response(e));
    }
    let (id, name) = garden_of(f, sel)?;
    match garden_access_may(caller, &id) {
        May::Edit => None,
        May::Ask => Some(access_needed(caller, &id, &name, path, ingress)),
        May::Refused(code) => Some(passport_refusal(code)),
    }
}

/// A widget write: the list on every Garden that places `widget` (a draft
/// placed nowhere is open to any local agent). Widget writes aren't
/// lock-checked: a locked Garden runs its frozen copy (§1.8).
fn widget_write(f: &ZenFiles, widget: &str, caller: &Caller, path: &str, ingress: &str) -> Option<CliResponse> {
    for (id, name) in placing_gardens(f, widget.trim()) {
        match garden_access_may(caller, &id) {
            May::Edit => {}
            May::Ask => return Some(access_needed(caller, &id, &name, path, ingress)),
            May::Refused(code) => return Some(passport_refusal(code)),
        }
    }
    None
}

/// The computer-wide look: a passport on at least one Garden's list.
fn look_write(caller: &Caller, path: &str, ingress: &str) -> Option<CliResponse> {
    let (p, _) = match passport_workspace(caller) {
        Ok(w) => w,
        Err(code) => return Some(passport_refusal(code)),
    };
    let listed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ga::listed_anywhere(&conn, &p).unwrap_or(false)
    };
    if listed {
        None
    } else {
        Some(look_needs_a_list(caller, path, ingress))
    }
}

/// **G11/G12 with the switch on**: what `zen_routes::handle` does for a
/// request that isn't the owner token (nor an Owner login on a GardenOwner
/// route). `None` lets the route run.
pub(crate) fn passport_gate(path: &str, caller: &Caller, body: &[u8], ingress: &str) -> Option<CliResponse> {
    if !crate::zen_routes::is_local_passport(caller) {
        return Some(crate::zen_routes::local_only());
    }
    if OPEN_TO_PASSPORTS.contains(&path) {
        return None;
    }
    // Every write below needs a live session (a dead sandboxed session's
    // passport would otherwise count as its workspace, GA15).
    if let Err(code) = passport_workspace(caller) {
        return Some(passport_refusal(code));
    }
    let b = body_json(body);
    let f = files();
    let named = |k: &str| b[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    match path {
        "/cli/zen/proposals/approve" => Some(retired("Approving Garden proposals from an agent")),
        p if GARDEN_WRITES.contains(&p) => match named("garden") {
            Some(sel) => garden_write(&f, &sel, caller, path, ingress),
            None => None, // the handler answers its own 400
        },
        "/cli/zen/garden/delete" => {
            let sel = named("garden")?;
            let (id, name) = match garden_of(&f, &sel) {
                Some(g) => g,
                None => return Some(unknown_garden(&f, &sel)),
            };
            let (p, _) = passport_workspace(caller).ok()?;
            if may_ask_delete(&p, &id) {
                Some(delete_person(caller, &id, &name, ingress))
            } else {
                Some(access_needed(caller, &id, &name, path, ingress))
            }
        }
        "/cli/zen/reset" => {
            if let Some(w) = named("widget") {
                return widget_write(&f, &w, caller, path, ingress);
            }
            let garden = named("garden").or_else(|| garden_of_file(b["file"].as_str()));
            match garden {
                Some(sel) => garden_write(&f, &sel, caller, path, ingress),
                None => look_write(caller, path, ingress),
            }
        }
        "/cli/zen/theme/set" | "/cli/zen/theme/next" | "/cli/zen/theme/prev" => match named("garden") {
            Some(sel) => garden_write(&f, &sel, caller, path, ingress),
            None => look_write(caller, path, ingress),
        },
        "/cli/zen/widget/put" | "/cli/zen/widget/folder/put" => match named("widget") {
            Some(w) => widget_write(&f, &w, caller, path, ingress),
            None => None,
        },
        _ => Some(crate::zen_routes::local_only()),
    }
}

// ── G13: making a Garden ─────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewWidget {
    widget: String,
    files: Vec<StagedFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewGardenBody {
    name: String,
    template: Option<String>,
    #[serde(rename = "seedHome")]
    seed_home: Option<String>,
    at: Option<usize>,
    /// The first page (`gardens/<id>.toml` text), checked like `garden/check`.
    #[serde(default)]
    page: Option<String>,
    /// New widget folders, checked like `widget/folder/check`; names must be new.
    #[serde(default)]
    widgets: Vec<NewWidget>,
}

fn nonempty(s: &Option<String>) -> Option<String> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Undo a half-made Garden: its new widget folders, its list entry and
/// its history folder.
fn roll_back(f: &ZenFiles, garden: Option<&str>, widgets: &[String]) {
    for w in widgets {
        let dir = f.widget_dir(w);
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            k2_core::log_debug!("[zen/access] roll back {}: {e}", dir.display());
        }
        let hist = f.history_root().join("widgets").join(w);
        let _ = std::fs::remove_dir_all(hist);
    }
    if let Some(id) = garden {
        if let Err(e) = f.delete_garden(id) {
            k2_core::log_debug!("[zen/access] roll back garden {id}: {e}");
        }
        let _ = std::fs::remove_dir_all(f.history_root().join(ZenFile::Garden(id.to_string()).label()));
    }
}

/// **G13** (switch on): `POST /cli/zen/garden/new {name, template?, seedHome?,
/// at?, page?, widgets?}`. The person (owner token) or any live local
/// passport, no card needed. The page and widgets land with it or nothing
/// does. A passport's Garden records its maker (never its list), costs one
/// of 5 a day, and K2 presents the `garden.access` card in the same step.
pub(crate) fn garden_new(owner_token: bool, caller: &Caller, body: &[u8], ingress: &str) -> CliResponse {
    let b: NewGardenBody = match serde_json::from_value(body_json(body)) {
        Ok(b) => b,
        Err(e) => {
            return refuse(
                "400 Bad Request",
                "bad_request",
                format!("garden/new takes {{name, template?, seedHome?, at?, page?, widgets?: [{{widget, files}}]}}: {e}"),
            )
        }
    };
    let maker = if owner_token {
        None
    } else {
        match passport_workspace(caller) {
            Ok(w) => Some(w),
            Err(code) => return passport_refusal(code),
        }
    };
    let t = now();
    if let Some((p, _)) = &maker {
        let made = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            ga::made_since(&conn, p, t - 24 * 60 * 60)
        };
        match made {
            Ok(n) if n >= ga::MADE_PER_DAY => {
                crate::zen_routes::audit_refusal("zen.garden.budget", caller, format!("{n} made in 24 h"), ingress);
                return refuse(
                    "429 Too Many Requests",
                    "garden_budget",
                    format!("You've made {} Gardens today; ask the person to make more.", ga::MADE_PER_DAY),
                );
            }
            Ok(_) => {}
            Err(e) => return refuse("503 Service Unavailable", "store_unavailable", format!("the Garden list store: {e:?}")),
        }
    }
    let f = files();
    // Check every widget before anything is written: a new, clean folder.
    let mut staged = Vec::new();
    let mut names = BTreeSet::new();
    for w in &b.widgets {
        let files_in = match widget_folder::staged_files(&w.files) {
            Ok(s) => s,
            Err(e) => return crate::zen_routes::error_response(e),
        };
        let name = match f.check_widget_folder_put(&w.widget, &files_in) {
            Ok(n) => n,
            Err(e) => return crate::zen_routes::error_response(e),
        };
        if !names.insert(name.clone()) {
            return refuse("400 Bad Request", "bad_request", format!("widgets/{name} is named twice"));
        }
        if f.widget_status(&name).exists {
            return crate::zen_routes::error_response(ZenError::WidgetExists(format!(
                "widgets/{name} already exists; a new Garden brings new widgets only (change that one with k2 zen widget put)"
            )));
        }
        staged.push((name, files_in));
    }
    let mut written: Vec<String> = Vec::new();
    for (name, files_in) in &staged {
        if let Err(e) = f.put_widget_folder(name, files_in, false) {
            roll_back(&f, None, &written);
            return crate::zen_routes::error_response(e);
        }
        written.push(name.clone());
    }
    let g = match f.new_garden(&b.name, nonempty(&b.template).as_deref(), nonempty(&b.seed_home).as_deref(), b.at) {
        Ok(g) => g,
        Err(e) => {
            roll_back(&f, None, &written);
            return crate::zen_routes::error_response(e);
        }
    };
    if let Some(page) = b.page.as_deref() {
        if let Err(e) = f.put_garden(&g.id, page, true) {
            roll_back(&f, Some(&g.id), &written);
            return crate::zen_routes::error_response(e);
        }
    }
    let changed = match crate::zen_routes::refresh_and_emit() {
        Ok(c) => c,
        Err(e) => return crate::zen_routes::error_response(e),
    };
    // The maker stamps its new widgets (first signer; they are new).
    let mut stamped = Vec::new();
    for name in &written {
        let st = f.widget_status(name);
        if st.state == WidgetState::Ok && !st.hash().is_empty() {
            if let Some((who, kind)) = signer_of(caller) {
                let db = k2_core::db::shared();
                let conn = db.lock();
                if let Err(e) = authors::stamp(&conn, name, st.hash(), &who, StampSource::New, kind, t) {
                    k2_core::log_debug!("[zen/access] stamp new widget {name}: {e}");
                }
            }
            stamped.push(json!({ "widget": name, "hash": short(st.hash()) }));
        } else {
            stamped.push(json!({ "widget": name, "hash": J::Null, "state": st.state.as_str() }));
        }
    }
    let actor = actor_of(caller).unwrap_or(Actor::Owner);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some((p, _)) = &maker {
            if let Err(e) = ga::record_made(&conn, &g.id, p, t) {
                k2_core::log_debug!("[zen/access] made {}: {e:?}", g.id);
            }
        }
        let detail = json!({ "name": g.name, "template": g.template, "page": b.page.is_some(), "widgets": written });
        if let Err(e) = ga::record_event(&conn, On::Garden(&g.id), EventKind::Made, &actor, &detail, t) {
            k2_core::log_debug!("[zen/access] made event {}: {e:?}", g.id);
        }
    }
    let card = match &maker {
        Some((p, _)) => {
            crate::gardens_manage::audit("zen-garden-new", &caller.audit_user(), &g.id, None, None);
            present(GardenAsk::Access, &g.id, p, true, true)
        }
        None => J::Null,
    };
    let (index, _) = match f.garden(&g.id) {
        Ok(x) => x,
        Err(e) => return crate::zen_routes::error_response(e),
    };
    let file = ZenFile::Garden(g.id.clone());
    let message = match &maker {
        Some(_) => format!(
            "Made {}. To change it after this, you need the person's OK: K2 asked them on your Thread. Until they Allow, its widgets run without their data.",
            g.name
        ),
        None => format!("Made {}.", g.name),
    };
    resp(
        "200 OK",
        json!({
            "ok": true,
            "garden": f.garden_json(index, &g, &f.read_active()),
            "file": file.label(),
            "path": f.path_of(&file).display().to_string(),
            "changed": changed,
            "made": maker.is_some(),
            "widgets": stamped,
            "card": card,
            "message": message,
        }),
    )
}

// ── G9 sign ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignBody {
    widget: String,
    /// The Garden whose History modal the person signed from (an Owner
    /// login's Sign button, G14/G26a). Recorded on the event only.
    #[serde(default)]
    garden: Option<String>,
}

/// `POST /cli/zen/widget/sign {widget, garden?}` with the switch on: the
/// person (owner token or Owner login) or a local passport signs the live
/// clean version. The person, or an agent listed on every placing Garden,
/// vouches (overwrites another signer); anyone else stamps only an unsigned
/// version, which runs with Always only until someone listed signs (GA1).
pub(crate) fn sign(caller: &Caller, body: &[u8], ingress: &str) -> CliResponse {
    let b: SignBody = match serde_json::from_value(body_json(body)) {
        Ok(b) => b,
        Err(e) => return refuse("400 Bad Request", "bad_request", format!("widget/sign takes {{widget, garden?}}: {e}")),
    };
    let widget = b.widget.trim().to_string();
    if authors::is_builtin(&widget) {
        return resp("200 OK", json!({ "ok": true, "widget": widget, "hash": J::Null, "madeBy": { "kind": "k2" }, "stamped": false }));
    }
    let f = files();
    if !f.is_set_up() {
        return crate::zen_routes::error_response(ZenError::NotSetUp);
    }
    if let Err(e) = crate::zen_routes::refresh_and_emit() {
        return crate::zen_routes::error_response(e);
    }
    let st = f.widget_status(&widget);
    if !st.exists {
        return crate::zen_routes::error_response(ZenError::UnknownWidget(format!("no widget folder '{widget}'")));
    }
    if st.state != WidgetState::Ok || st.hash().is_empty() {
        return crate::zen_routes::error_response(ZenError::WidgetBroken(format!(
            "{widget} has errors; fix them (k2 zen validate --widget {widget}) and sign again"
        )));
    }
    if signer_of(caller).is_none() {
        return crate::zen_routes::error_response(ZenError::OwnerOnly("only you or an agent on this computer can sign a widget".into()));
    }
    let out = stamp_after_write(&f, &widget, caller, StampSource::Sign);
    if out.is_null() {
        return refuse("503 Service Unavailable", "store_unavailable", "the sign wasn't saved; try again");
    }
    let hash = st.hash().to_string();
    let stamped = out["stamped"] == J::Bool(true);
    crate::zen_routes::audit_refusal(
        "zen.widget.sign",
        caller,
        format!("{widget} {} source=sign stamped={stamped} vouched={}", short(&hash), out["vouched"]),
        ingress,
    );
    if stamped {
        let garden = b.garden.as_deref().and_then(|g| resolve_id(Some(g)));
        let detail = json!({ "hash": short(&hash), "vouched": out["vouched"] });
        match garden.as_deref() {
            Some(g) => record(On::GardenWidget(g, &widget), EventKind::Signed, caller, detail),
            None => record(On::Widget(&widget), EventKind::Signed, caller, detail),
        }
    }
    resp(
        "200 OK",
        json!({ "ok": true, "widget": widget, "hash": hash, "madeBy": out["madeBy"], "stamped": stamped, "vouched": out["vouched"] }),
    )
}

// ── G14: the list ────────────────────────────────────────────────────

fn not_available() -> CliResponse {
    refuse("409 Conflict", "not_available", "Garden access lists are off on this server (Settings → Gardens).")
}

fn owner_only_list() -> CliResponse {
    crate::zen_routes::error_response(ZenError::OwnerOnly(
        "Only the person changes who may edit a Garden (Settings → Gardens). An agent asks with k2 zen garden access <garden> ask.".into(),
    ))
}

/// **The list door** (Settings and `k2 zen garden access <g> add|remove`
/// through `zen_garden_cards::access_entry`; the card and the twin may use
/// it with their own `via` and card id): the GardenOwner check, then add or
/// remove `agent` (a workspace id or handle) on `garden` (an id or name)
/// through the store, which writes the History line and the Access Audit
/// line.
pub(crate) fn list_change(caller: &Caller, garden: &str, agent: &str, allow: bool, via: Via, card_id: Option<&str>) -> CliResponse {
    if !ga::enabled() {
        return not_available();
    }
    if !crate::zen_routes::garden_owner_may(caller, Some(garden)) {
        return owner_only_list();
    }
    let f = files();
    let Some((id, name)) = garden_of(&f, garden) else {
        return unknown_garden(&f, &garden.to_string());
    };
    let Some(ws) = crate::compute_routes::ws_by_token(agent) else {
        return refuse("404 Not Found", "unknown_agent", format!("no agent '{agent}' on this computer"));
    };
    let Some(actor) = actor_of(caller) else { return owner_only_list() };
    let t = now();
    let changed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut change = Change::new(via, &actor, t);
        if let Some(c) = card_id {
            change = change.card(c);
        }
        if via == Via::Settings {
            change = change.route(ACCESS_ROUTE);
        }
        let r = if allow { ga::add(&conn, &id, &ws.id, &change) } else { ga::remove(&conn, &id, &ws.id, &change) };
        if allow && r.is_ok() {
            if let Err(e) = ga::clear_ask_card(&conn, &id, &ws.id, ga::AskKind::Access) {
                k2_core::log_debug!("[zen/access] clear ask {id} {}: {e:?}", ws.id);
            }
        }
        r
    };
    match changed {
        Ok(changed) => resp(
            "200 OK",
            json!({
                "ok": true,
                "garden": { "id": id, "name": name },
                "agent": { "id": ws.id, "handle": ws.handle },
                "listed": allow,
                "changed": changed,
            }),
        ),
        Err(ga::AccessError::BadRequest(m)) => refuse("400 Bad Request", "bad_request", m),
        Err(e) => refuse("503 Service Unavailable", "store_unavailable", format!("{e:?}")),
    }
}

// ── routes ───────────────────────────────────────────────────────────

fn param<'a>(params: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    params.get(key).map(|s| s.trim()).filter(|s| !s.is_empty())
}

/// An agent as the Settings pane shows it: `{agent, handle, name, color}`.
pub(crate) fn agent_view(project_id: &str) -> J {
    let row: Option<(String, Option<String>)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT path, color FROM projects WHERE id = ?1", rusqlite::params![project_id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .ok()
    };
    let (name, color) = match row {
        Some((path, color)) => (
            k2_core::workspace::display::agent_display_name(&path),
            color.and_then(|c| k2_core::workspace::agent_look::normalize_color(&c).ok()),
        ),
        None => (project_id.to_string(), None),
    };
    json!({ "agent": project_id, "handle": handle_of(project_id), "name": name, "color": color })
}

/// May `caller` read the list and History as the person? The owner token
/// (presented as such) or an Owner login.
fn person(owner: bool, caller: &Caller) -> bool {
    match caller {
        Caller::Owner => owner,
        Caller::Login { .. } => crate::zen_routes::garden_owner_may(caller, None),
        _ => false,
    }
}

/// `GET /cli/zen/garden/access?garden=`.
fn access_get(owner: bool, caller: &Caller, params: &HashMap<String, String>) -> CliResponse {
    let Some(sel) = param(params, "garden") else {
        return refuse("400 Bad Request", "bad_request", "garden/access needs garden=<id or name>");
    };
    let f = files();
    if !f.is_set_up() {
        return crate::zen_routes::error_response(ZenError::NotSetUp);
    }
    let Some((id, name)) = garden_of(&f, sel) else {
        return unknown_garden(&f, &sel.to_string());
    };
    let (rows, made) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (ga::list(&conn, &id), ga::made(&conn, &id))
    };
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return refuse("503 Service Unavailable", "store_unavailable", format!("{e:?}")),
    };
    let made = made.ok().flatten();
    if person(owner, caller) {
        // The Settings pane's shape (a5-asks.md #4).
        let editors: Vec<J> = rows
            .iter()
            .map(|r| {
                let mut v = agent_view(&r.project_id);
                v["addedBy"] = json!(r.added_by);
                v["addedAt"] = json!(r.added_at);
                v["cardId"] = json!(r.card_id);
                v
            })
            .collect();
        let made = made.map(|m| {
            let mut v = agent_view(&m.made_by_project);
            v["at"] = json!(m.made_at);
            v
        });
        return resp(
            "200 OK",
            json!({
                "ok": true,
                "garden": id,
                "name": name,
                "locked": f.is_locked(&id),
                "made": made,
                "editors": editors,
                "sees": "everything",
            }),
        );
    }
    match passport_workspace(caller) {
        Ok((p, _)) => {
            let listed = rows.iter().any(|r| r.project_id == p);
            let maker = made.is_some_and(|m| m.made_by_project == p);
            resp(
                "200 OK",
                json!({ "ok": true, "garden": id, "name": name, "locked": f.is_locked(&id), "listed": listed, "maker": maker }),
            )
        }
        Err(code) => passport_refusal(code),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessBody {
    garden: String,
    agent: String,
    allow: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AskBody {
    garden: String,
    #[serde(default)]
    reason: Option<String>,
}

/// `POST /cli/zen/garden/access/ask {garden, reason?}`: a live local
/// passport asks to edit. Already listed: nothing to ask.
fn ask_post(caller: &Caller, body: &[u8]) -> CliResponse {
    let b: AskBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return refuse("400 Bad Request", "bad_request", format!("garden/access/ask takes {{garden, reason?}}: {e}")),
    };
    let (p, shell) = match passport_workspace(caller) {
        Ok(w) => w,
        Err(code) => return passport_refusal(code),
    };
    let reason = b.reason.as_deref().map(str::trim).filter(|r| !r.is_empty());
    if reason.is_some_and(|r| r.chars().count() > 500 || r.chars().any(|c| c.is_control() && c != '\n')) {
        return refuse("400 Bad Request", "bad_request", "a reason is one paragraph of at most 500 characters");
    }
    let f = files();
    if !f.is_set_up() {
        return crate::zen_routes::error_response(ZenError::NotSetUp);
    }
    let Some((id, name)) = garden_of(&f, &b.garden) else {
        return unknown_garden(&f, &b.garden.clone());
    };
    if is_listed(&id, &p) {
        return resp(
            "200 OK",
            json!({ "ok": true, "garden": { "id": id, "name": name }, "listed": true, "asked": false, "card": J::Null, "message": format!("You already edit {name}.") }),
        );
    }
    let _ = shell;
    // The agent's own words ride the card as its `why` (A4 keeps K2's words
    // when they're too long or secret-shaped).
    let card = cards::present_with_reason(GardenAsk::Access, &p, &id, false, true, reason);
    let message = if !has_card(&card) {
        format!("K2 didn't ask again: the person has a card waiting, or said Not now in the last 24 hours. Check later with k2 zen garden access \"{name}\".")
    } else {
        format!("K2 asked the person to let you edit {name} (a card on their Thread with you).")
    };
    resp(
        "200 OK",
        json!({ "ok": true, "garden": { "id": id, "name": name }, "listed": false, "asked": has_card(&card), "card": card, "message": message }),
    )
}

// ── History routes ───────────────────────────────────────────────────

/// The kinds a `kind=` filter names: kinds, or the groups the modals offer.
fn kind_filter(raw: Option<&str>) -> Result<Option<BTreeSet<&'static str>>, CliResponse> {
    let Some(raw) = raw else { return Ok(None) };
    let mut out = BTreeSet::new();
    for k in raw.split(',').map(str::trim).filter(|k| !k.is_empty()) {
        let group: &[&str] = match k {
            "all" => return Ok(None),
            "changes" => &[
                "made", "changed", "widget_changed", "renamed", "reordered", "theme", "reset", "template", "locked", "unlocked", "sync",
                "deleted", "restored", "disk_edit", "lock_put_back", "signed", "imported",
            ],
            "access" => &["access_added", "access_removed"],
            "asks" => &["access_asked", "access_not_now", "delete_asked", "delete_kept"],
            one => match EventKind::parse(one) {
                Some(kind) => {
                    out.insert(kind.as_str());
                    continue;
                }
                None => {
                    return Err(refuse(
                        "400 Bad Request",
                        "bad_request",
                        format!("unknown kind '{one}': use all, changes, access, asks, or a History kind"),
                    ))
                }
            },
        };
        out.extend(group.iter().copied());
    }
    Ok(Some(out))
}

/// Who an actor label is, for the modals (a5-asks.md #7): `{kind, id?,
/// handle?, name?, color?}`. `name` is plain words ("you", "a terminal in
/// billing's room", "K2", "outside K2").
pub(crate) fn actor_json(label: &str) -> J {
    match Actor::parse(label) {
        Some(Actor::Owner) => json!({ "kind": "owner", "name": "you" }),
        Some(Actor::Login(u)) => json!({ "kind": "login", "id": u, "name": u }),
        Some(Actor::Agent(p)) => {
            let mut v = agent_view(&p);
            v["kind"] = json!("agent");
            v["id"] = json!(p);
            v
        }
        Some(Actor::Shell(p)) => {
            let mut v = agent_view(&p);
            v["kind"] = json!("shell");
            v["id"] = json!(p);
            v["name"] = json!(format!("a terminal in {}'s room", handle_of(&p)));
            v
        }
        Some(Actor::K2) => json!({ "kind": "k2", "name": "K2" }),
        Some(Actor::Disk) => json!({ "kind": "disk", "name": "outside K2" }),
        Some(Actor::Migration) => json!({ "kind": "migration", "name": "K2 (upgrade)" }),
        None => json!({ "kind": "unknown", "name": label }),
    }
}

/// One History item from a `zen_garden_events` line; [`crate::zen_history::merge`]
/// adds the other sources (snapshots, stamps, decided proposals).
fn event_item(e: &ga::Event) -> J {
    json!({
        "id": format!("event:{}", e.id),
        "at": e.at,
        "kind": e.kind.as_str(),
        "source": "event",
        "actor": actor_json(&e.actor),
        "garden": e.garden_id,
        "widget": e.widget,
        "detail": named_detail(&e.detail),
        "snapshot": J::Null,
    })
}

/// A line's detail with the agent it names (`workspace`, a project id:
/// access added/removed, asks, Not now, Keep it) named for the modals:
/// `name` and `handle`, read now (A6). A workspace that's gone keeps only
/// its id, and the modal says "an agent". The stored line is unchanged.
fn named_detail(detail: &J) -> J {
    let mut d = detail.clone();
    let Some(p) = d["workspace"].as_str().map(str::to_string) else { return d };
    if d.get("name").is_some() || !ga::workspace_exists(&k2_core::db::shared().lock(), &p).unwrap_or(false) {
        return d;
    }
    let a = agent_view(&p);
    d["name"] = a["name"].clone();
    d["handle"] = a["handle"].clone();
    d
}

/// Page through `fetch` (newest first) until `limit` lines match `kinds`.
/// Answers the lines and the `next` cursor (the last line's id), or `None`
/// at the end.
fn page_events(
    limit: usize,
    before: Option<i64>,
    kinds: &Option<BTreeSet<&'static str>>,
    fetch: impl Fn(usize, Option<i64>) -> ga::Result<Vec<ga::Event>>,
) -> Result<(Vec<ga::Event>, Option<i64>), CliResponse> {
    let mut out = Vec::new();
    let mut cursor = before;
    loop {
        let batch = fetch(HISTORY_MAX, cursor).map_err(|e| refuse("503 Service Unavailable", "store_unavailable", format!("{e:?}")))?;
        let n = batch.len();
        for e in batch {
            cursor = Some(e.id);
            if kinds.as_ref().is_none_or(|k| k.contains(e.kind.as_str())) {
                out.push(e);
                if out.len() == limit {
                    return Ok((out, cursor));
                }
            }
        }
        if n < HISTORY_MAX {
            return Ok((out, None));
        }
    }
}

/// `limit` (1–200, default 50) and `before` (a `next` cursor: `event:<id>`
/// or the bare id).
fn paging(params: &HashMap<String, String>) -> Result<(usize, Option<i64>), CliResponse> {
    let limit = match param(params, "limit") {
        None => HISTORY_PAGE,
        Some(l) => match l.parse::<usize>() {
            Ok(n) if (1..=HISTORY_MAX).contains(&n) => n,
            _ => return Err(refuse("400 Bad Request", "bad_request", format!("limit is 1 to {HISTORY_MAX}"))),
        },
    };
    let before = match param(params, "before") {
        None => None,
        Some(b) => {
            let id = b.strip_prefix("event:").unwrap_or(b);
            Some(id.parse::<i64>().map_err(|_| refuse("400 Bad Request", "bad_request", "before is a History cursor (the answer's next)"))?)
        }
    };
    Ok((limit, before))
}

fn cursor(next: Option<i64>) -> J {
    next.map_or(J::Null, |id| json!(format!("event:{id}")))
}

/// The live versions placed in `garden` that fail the signer gate there:
/// `[{widget, hash, gardens: [garden]}]` (the modals' "not signed" line).
fn unsigned_in(f: &ZenFiles, garden: &str, widgets: &[String]) -> Vec<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut out = Vec::new();
    for w in widgets {
        let st = f.garden_widget_status(garden, w);
        let hash = st.hash();
        if hash.is_empty() {
            continue;
        }
        let passes = k2_core::garden_data::signer_gate::passes(&conn, garden, w, hash).unwrap_or(false);
        if !passes {
            out.push(json!({ "widget": w, "hash": short(hash), "gardens": [garden] }));
        }
    }
    out
}

/// `GET /cli/zen/garden/history?garden=&before=&limit=&kind=`: the
/// Garden's lines plus the widget lines of the widgets placed in it now.
/// A deleted Garden's History stays readable by its id (read only).
fn garden_history(params: &HashMap<String, String>) -> CliResponse {
    let Some(sel) = param(params, "garden") else {
        return refuse("400 Bad Request", "bad_request", "garden/history needs garden=<id or name>");
    };
    let (limit, before) = match paging(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let kinds = match kind_filter(param(params, "kind")) {
        Ok(k) => k,
        Err(r) => return r,
    };
    let f = files();
    let (id, name, placed) = match garden_of(&f, sel) {
        Some((id, name)) => {
            let placed: Vec<String> = f
                .custom_placements()
                .into_iter()
                .filter(|p| p.garden == id)
                .map(|p| p.widget)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            (id, Some(name), placed)
        }
        None if sel == ga::COMPUTER_WIDE || k2_core::zen::store::valid_garden_id(sel) => (sel.to_string(), None, Vec::new()),
        None => return unknown_garden(&f, sel),
    };
    let refs: Vec<&str> = placed.iter().map(String::as_str).collect();
    let got = page_events(limit, before, &kinds, |n, b| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ga::events_for_garden(&conn, &id, &refs, n, b)
    });
    let unsigned = if name.is_some() { unsigned_in(&f, &id, &placed) } else { Vec::new() };
    let got = got.and_then(|(events, next)| {
        let items = events.iter().map(event_item).collect::<Vec<_>>();
        let scope = crate::zen_history::Scope::Garden { id: &id, placed: &placed };
        crate::zen_history::merge(&f, scope, &events, items, before, next, &kinds)
            .map(|items| (items, next))
            .map_err(|e| refuse("503 Service Unavailable", "store_unavailable", format!("history merge: {e}")))
    });
    match got {
        Ok((items, next)) => resp(
            "200 OK",
            json!({
                "ok": true,
                "garden": id,
                "name": name,
                "widgets": placed,
                "items": items,
                "next": cursor(next),
                "unsigned": unsigned,
            }),
        ),
        Err(r) => r,
    }
}

/// `GET /cli/zen/widget/history?widget=&before=&limit=&kind=`: every line
/// naming the widget.
fn widget_history(params: &HashMap<String, String>) -> CliResponse {
    let Some(widget) = param(params, "widget") else {
        return refuse("400 Bad Request", "bad_request", "widget/history needs widget=<folder name>");
    };
    let (limit, before) = match paging(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let kinds = match kind_filter(param(params, "kind")) {
        Ok(k) => k,
        Err(r) => return r,
    };
    let f = files();
    let placing = placing_gardens(&f, widget);
    let gardens: Vec<J> = placing.iter().map(|(id, name)| json!({ "id": id, "name": name })).collect();
    let failing: Vec<String> = placing
        .iter()
        .filter(|(g, _)| !unsigned_in(&f, g, &[widget.to_string()]).is_empty())
        .map(|(g, _)| g.clone())
        .collect();
    let unsigned = if failing.is_empty() {
        Vec::new()
    } else {
        let hash = f.widget_status(widget).hash().to_string();
        vec![json!({ "widget": widget, "hash": short(&hash), "gardens": failing })]
    };
    let got = page_events(limit, before, &kinds, |n, b| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ga::events_for_widget(&conn, widget, n, b)
    });
    let got = got.and_then(|(events, next)| {
        let items = events.iter().map(event_item).collect::<Vec<_>>();
        crate::zen_history::merge(&f, crate::zen_history::Scope::Widget(widget), &events, items, before, next, &kinds)
            .map(|items| (items, next))
            .map_err(|e| refuse("503 Service Unavailable", "store_unavailable", format!("history merge: {e}")))
    });
    match got {
        Ok((items, next)) => resp(
            "200 OK",
            json!({
                "ok": true,
                "widget": widget,
                "gardens": gardens,
                "items": items,
                "next": cursor(next),
                "unsigned": unsigned,
            }),
        ),
        Err(r) => r,
    }
}

// ── read decorations for the Settings page (switch on) ───────────────

/// `GET /cli/zen/gardens` rows gain `editors` (a count) and `madeBy`
/// (`{agent, handle, name}` or null) so the sidebar needs no N requests.
pub(crate) fn decorate_gardens(v: &mut J) {
    let Some(rows) = v["gardens"].as_array_mut() else { return };
    for row in rows {
        let Some(id) = row["id"].as_str().map(str::to_string) else { continue };
        let (editors, made) = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            (ga::list(&conn, &id).map(|r| r.len()).unwrap_or(0), ga::made(&conn, &id).ok().flatten().map(|m| m.made_by_project))
        };
        row["editors"] = json!(editors);
        row["madeBy"] = match made {
            Some(p) => {
                let mut a = agent_view(&p);
                if let Some(o) = a.as_object_mut() {
                    o.remove("color");
                }
                a
            }
            None => J::Null,
        };
    }
}

/// Newest modification time (Unix seconds) of a widget folder's files.
fn folder_changed_at(f: &ZenFiles, widget: &str) -> Option<i64> {
    std::fs::read_dir(f.widget_dir(widget))
        .ok()?
        .flatten()
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

/// `GET /cli/zen/widgets` rows gain `signedBy` (`{kind, handle?, name?,
/// at}` or null), `changedAt`, and each `placements[]` row `signed` (the
/// signer gate for that Garden).
pub(crate) fn decorate_widgets(f: &ZenFiles, v: &mut J) {
    let Some(rows) = v["widgets"].as_array_mut() else { return };
    for row in rows {
        let name = row["name"].as_str().unwrap_or_default().to_string();
        let hash = row["hash"].as_str().unwrap_or_default().to_string();
        let signed_by = if hash.len() == 64 && !authors::is_builtin(&name) {
            let (maker, at) = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                (authors::maker_of(&conn, &name, &hash).ok(), authors::signed_at(&conn, &name, &hash).ok().flatten())
            };
            match maker {
                Some(MadeBy::Owner) => json!({ "kind": "owner", "at": at }),
                Some(MadeBy::Agent { project_id }) => {
                    let a = agent_view(&project_id);
                    json!({ "kind": "agent", "handle": a["handle"], "name": a["name"], "at": at })
                }
                _ => J::Null,
            }
        } else if authors::is_builtin(&name) {
            json!({ "kind": "k2", "at": J::Null })
        } else {
            J::Null
        };
        row["signedBy"] = signed_by;
        row["changedAt"] = json!(folder_changed_at(f, &name));
        if let Some(ps) = row["placements"].as_array_mut() {
            for p in ps {
                let Some(g) = p["garden"].as_str().map(str::to_string) else { continue };
                let live = f.garden_widget_status(&g, &name).hash().to_string();
                let ok = !live.is_empty() && {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    k2_core::garden_data::signer_gate::passes(&conn, &g, &name, &live).unwrap_or(false)
                };
                p["signed"] = json!(ok);
            }
        }
    }
}

// ── Deleted Gardens, restore, View this version (G25, G26a) ─────────────

fn rfc3339_secs(v: &J) -> J {
    v.as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map_or(J::Null, |t| json!(t.timestamp()))
}

/// `GET /cli/zen/gardens/deleted`: `{ok, gardens: [{id, name, deletedAt
/// (unix s), deletedBy: <actor>|null, snapshot}]}`.
fn deleted_gardens() -> CliResponse {
    let f = files();
    if !f.is_set_up() {
        return crate::zen_routes::error_response(ZenError::NotSetUp);
    }
    let rows: Vec<J> = f
        .deleted_gardens()
        .into_iter()
        .map(|g| {
            let id = g["id"].as_str().unwrap_or_default().to_string();
            let by = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                ga::events_for_garden(&conn, &id, &[], HISTORY_MAX, None)
                    .ok()
                    .and_then(|ev| ev.into_iter().find(|e| e.kind == EventKind::Deleted))
                    .map(|e| actor_json(&e.actor))
            };
            json!({
                "id": id,
                "name": g["name"],
                "deletedAt": rfc3339_secs(&g["deletedAt"]),
                "deletedBy": by,
                "snapshot": g["snapshot"],
            })
        })
        .collect();
    resp("200 OK", json!({ "ok": true, "gardens": rows }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreBody {
    garden: String,
}

/// `POST /cli/zen/garden/restore {garden}` (the person): the Garden comes
/// back from its kept copy, with its list and made rows from `access.json`
/// (G8), and a `restored` History line.
fn restore(caller: &Caller, body: &[u8]) -> CliResponse {
    let b: RestoreBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return refuse("400 Bad Request", "bad_request", format!("garden/restore takes {{garden}}: {e}")),
    };
    let f = files();
    let g = match f.restore_garden(&b.garden) {
        Ok(g) => g,
        Err(e) => return crate::zen_routes::error_response(e),
    };
    let actor = actor_of(caller).unwrap_or(Actor::Owner);
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let t = now();
        let rows = ga::restore_garden(&conn, &f, &g.id, &actor, t);
        if let Err(e) = ga::record_event(&conn, On::Garden(&g.id), EventKind::Restored, &actor, &json!({ "name": g.name }), t) {
            k2_core::log_debug!("[zen/access] restored event {}: {e:?}", g.id);
        }
        rows
    };
    let rows = match rows {
        Ok(n) => n,
        Err(e) => {
            k2_core::log_debug!("[zen/access] restore rows {}: {e:?}", g.id);
            0
        }
    };
    if let Err(e) = crate::zen_routes::refresh_and_emit() {
        return crate::zen_routes::error_response(e);
    }
    resp("200 OK", json!({ "ok": true, "garden": { "id": g.id, "name": g.name }, "editorsRestored": rows }))
}

/// `GET /cli/zen/snapshot?file=&name=`: one kept version, read only.
fn snapshot(params: &HashMap<String, String>) -> CliResponse {
    let (Some(file), Some(name)) = (param(params, "file"), param(params, "name")) else {
        return refuse("400 Bad Request", "bad_request", "snapshot needs file=<gardens/<id>.toml | widgets/<name> | …> and name=<snapshot>");
    };
    match files().snapshot_view(file, name) {
        Ok(v) => resp("200 OK", v),
        Err(e) => crate::zen_routes::error_response(e),
    }
}

/// One request to this module's routes (switch on; the dispatcher's arm
/// runs only then). `owner`: the owner token was presented.
pub fn handle(
    path: &str,
    is_post: bool,
    owner: bool,
    caller: &Caller,
    ingress: &str,
    params: &HashMap<String, String>,
    body: &[u8],
) -> CliResponse {
    if !ga::enabled() {
        return resp("404 Not Found", json!({ "error": "unknown zen route", "path": path }));
    }
    match (path, is_post) {
        (DELETED_ROUTE | RESTORE_ROUTE | SNAPSHOT_ROUTE, _) if !person(owner, caller) => crate::zen_routes::error_response(
            ZenError::OwnerOnly("Only the person sees deleted Gardens, restores them and reads kept versions (Settings → Gardens).".into()),
        ),
        (DELETED_ROUTE, _) => deleted_gardens(),
        (RESTORE_ROUTE, true) => restore(caller, body),
        (RESTORE_ROUTE, false) => refuse("405 Method Not Allowed", "method_not_allowed", "garden/restore is a POST"),
        (SNAPSHOT_ROUTE, _) => snapshot(params),
        (ACCESS_ROUTE, false) => access_get(owner, caller, params),
        (ACCESS_ROUTE, true) => {
            if !person(owner, caller) {
                return owner_only_list();
            }
            let b: AccessBody = match serde_json::from_slice(body) {
                Ok(b) => b,
                Err(e) => return refuse("400 Bad Request", "bad_request", format!("garden/access takes {{garden, agent, allow}}: {e}")),
            };
            crate::zen_garden_cards::access_entry(caller, &b.garden, &b.agent, b.allow, ingress)
        }
        (ASK_ROUTE, true) => ask_post(caller, body),
        (ASK_ROUTE, false) => refuse("405 Method Not Allowed", "method_not_allowed", "garden/access/ask is a POST"),
        (GARDEN_HISTORY_ROUTE | WIDGET_HISTORY_ROUTE, _) if !person(owner, caller) => crate::zen_routes::error_response(
            ZenError::OwnerOnly("Only the person reads a Garden's History (Settings → Gardens → History).".into()),
        ),
        (GARDEN_HISTORY_ROUTE, _) => garden_history(params),
        (WIDGET_HISTORY_ROUTE, _) => widget_history(params),
        _ => resp("404 Not Found", json!({ "error": "unknown zen route", "path": path })),
    }
}

#[cfg(test)]
#[path = "zen_access_routes_tests.rs"]
mod tests;
