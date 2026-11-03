//! `chat.refresh notes= mode=` (0.46.8, Rosson 2026-11-03): an agent asks to
//! refresh its own pinned chat, and a person allows it, once or from now on.
//! `k2 agent refresh` with "Can refresh its own pinned chat" off is refused
//! with `chat_refresh_needs_person`, which this action's walls teach; that
//! refusal keeps the agent's note and hands the card its hash.
//!
//! - **Who answers:** the route's floor, an Admin (the owner token, an Owner
//!   or an Admin login). Proof: a click.
//! - **On Allow:** the route's own entry ([`crate::chat_refresh::handle`]) as
//!   the person who clicked, once. "Allow, and let it refresh its chat from
//!   now on" then turns the switch on through the Settings route's entry
//!   ([`turn_on_always`]); a person turns it off in Settings.
//! - **Notes:** `notes=` is a short one-line note, or (what the refusal prints)
//!   the sha256 of the note it kept. The card shows the start of it. What a
//!   person allowed is what the next chat reads: a kept note that changed
//!   makes the card stale.
//! - **The asker:** a sidecar has its own chat, so it can't ask ([`gate_ask`]).
//! - **One at a time:** a second ask with a different note is refused while
//!   one is open, or two Allows would restart the chat twice.
//! - **The turn after:** the approved `[permission:]` line goes to the
//!   workspace's address, so it reaches the NEW chat, after the handoff.

use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef, Undo,
    Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use k2_core::permission::label;
use k2_core::permission::request::PermissionRequest;
use k2_core::workspace::canonical_swap::{normalize_harness, read_refresh_note};
use serde_json::{json, Value};

use super::held::{error_of, route_floor, text_param};
use super::switches::applied;
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::chat_refresh::{CARD_ACTION as ACTION, NEEDS_PERSON, ROUTE};
use crate::cli_response::CliResponse;

const SWAP_ROUTE: &str = "/cli/workspace/swap-canonical";
const NOTE_PARAMS: &[(&str, ParamFrom)] = &[("notes", ParamFrom::Response("/error/notes")), ("mode", ParamFrom::Response("/error/mode"))];

pub(crate) static REFRESH: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Misc,
    params: &[
        ParamSpec { key: "notes", kind: ParamKind::Text, required: false, list: false, choices: &[] },
        ParamSpec { key: "mode", kind: ParamKind::Choice, required: false, list: false, choices: &["recent", "full"] },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Refresh pinned chat",
        what_changes: "Start a new {harness} pinned chat for {ws} that carries on from this one",
        consequences: "K2 ends {ws}'s current chat and starts a new one that gets only a summary of it and {ws}'s note. Anything it's doing right now stops, and secrets kept for that session end. Files aren't touched. The old chat stays in Chat history; the Thread, messages and heartbeats on the pinned chat carry on in the new chat. Sidecars keep running. \"From now on\" lets it do this without asking, until someone turns off \"Can refresh its own pinned chat\" in Settings → Workspaces → {ws} → Agent. {note}",
        scope: "{ws}",
        duration: "once, unless you choose from now on",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "workspaces/{ws}/agent", path: "Settings → Workspaces → {ws} → Agent" },
    walls: &[
        Wall { matches: WallMatch::Code { status: 403, code: NEEDS_PERSON }, route_prefix: ROUTE, method: WallMethod::Post, params: NOTE_PARAMS },
        Wall { matches: WallMatch::Code { status: 403, code: NEEDS_PERSON }, route_prefix: SWAP_ROUTE, method: WallMethod::Post, params: NOTE_PARAMS },
    ],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// How much of the note the card shows.
const NOTE_PREVIEW_CHARS: usize = 300;

fn path_of(workspace_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![workspace_id], |r| r.get::<_, String>(0)).ok()
}

/// The note on the card: a kept one by its hash, else the words themselves.
fn note_of(ctx: &Ctx, path: &str) -> Result<Option<String>, String> {
    match text_param(ctx, "notes") {
        None => Ok(None),
        Some(v) if crate::permission::routes::is_content_hash(v) => read_refresh_note(path, v).map(Some),
        Some(v) => Ok(Some(v.to_string())),
    }
}

/// `{harness}` and `{note}` for the words: fixed in the card's text hash, so a
/// chat moved to another harness, or a changed note, makes the card stale.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ACTION {
        return None;
    }
    let mut out = ctx.clone();
    let path = path_of(&ctx.workspace_id).unwrap_or_default();
    out.params.insert("harness".into(), json!(crate::agent_harness::current_harness(&ctx.workspace_id, &path)));
    let note = match note_of(ctx, &path) {
        Ok(Some(body)) => {
            let one_line = body.split_whitespace().collect::<Vec<_>>().join(" ");
            let shown: String = one_line.chars().take(NOTE_PREVIEW_CHARS).collect();
            let more = one_line.chars().count().saturating_sub(NOTE_PREVIEW_CHARS);
            if crate::permission::routes::secret_shaped(&shown) {
                format!("{}'s note for the new chat isn't shown here because it may hold a secret.", ctx.workspace_handle)
            } else if more > 0 {
                format!("{}'s note for the new chat: \"{shown}…\" ({more} more characters).", ctx.workspace_handle)
            } else {
                format!("{}'s note for the new chat: \"{shown}\"", ctx.workspace_handle)
            }
        }
        Ok(None) => "It left no note for the new chat.".to_string(),
        Err(why) => why,
    };
    out.params.insert("note".into(), json!(note));
    Some(out)
}

/// The card shows the note in its words, never the hash that names it.
pub(crate) fn hides_value(action: &str, key: &str) -> bool {
    action == ACTION && key == "notes"
}

/// A sidecar's passport can't ask (it has its own chat), and neither can a
/// second ask with another note while one is open. Same id rule as the route
/// (`crate::chat_refresh`): the passport's, never `K2_SESSION_ID`.
pub(crate) fn gate_ask(action: &str, workspace_id: &str, session_id: &str, params_hash: &str) -> Result<(), CliResponse> {
    if action != ACTION {
        return Ok(());
    }
    let refuse = |status: &'static str, code: &str, hint: String| CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string(),
    };
    if crate::cell_identity::sidecar_thread_addr(workspace_id, session_id).is_some() {
        return Err(refuse(
            "403 Forbidden",
            crate::chat_refresh::MAIN_ONLY,
            "a sidecar can't refresh itself. A new sidecar knows nothing of this chat, so write a handover to a file first, then ask your main session (`k2 msg <workspace>`) to stop you and open a sidecar under a new name with `--brief <file>`. `k2 sidecar new` on the same name resumes this chat.".to_string(),
        ));
    }
    let open = crate::permission::store::open(Some(workspace_id)).map_err(|e| refuse("500 Internal Server Error", "db", e))?;
    if let Some(other) = open.iter().find(|r| r.action == ACTION && r.params_hash != params_hash) {
        return Err(refuse(
            "409 Conflict",
            "chat_refresh_already_asked",
            format!(
                "you already asked to refresh this chat ({}), with a different note. To change it: `k2 permission cancel {}`, run `k2 agent refresh --note …` again, and ask with its new line.",
                other.display_id(),
                other.display_id()
            ),
        ));
    }
    Ok(())
}

fn check(ctx: &Ctx) -> Check {
    let Some(path) = path_of(&ctx.workspace_id) else {
        return Check::Moved(format!("{} is no longer a workspace on this server", ctx.workspace_handle));
    };
    if k2_core::workspace::settings::agents_can_refresh_chat(&ctx.workspace_id) {
        return Check::Moved(format!(
            "\"Can refresh its own pinned chat\" is on for {}: run `k2 agent refresh` yourself",
            ctx.workspace_handle
        ));
    }
    let current = crate::agent_harness::current_harness(&ctx.workspace_id, &path);
    if normalize_harness(&current).is_err() {
        return Check::Moved(format!("{} runs on {current}, a custom program, so there is no chat to refresh", ctx.workspace_handle));
    }
    match note_of(ctx, &path) {
        Ok(_) => Check::Needed,
        Err(why) => Check::Moved(why),
    }
}

/// Through the route's own entry, as the person who clicked.
fn apply(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    if let Err(refused) = route_floor(ROUTE, who) {
        return refused;
    }
    let Some(path) = path_of(&ctx.workspace_id) else {
        return Applied::Refused { code: "bad_params".into(), message: format!("{} is no longer a workspace on this server", ctx.workspace_handle) };
    };
    let notes = match note_of(ctx, &path) {
        Ok(n) => n.unwrap_or_default(),
        Err(why) => return Applied::Refused { code: "bad_params".into(), message: why },
    };
    let caller = match who {
        Answerer::OwnerToken => crate::sidecar_routes::Caller::Owner,
        Answerer::Login { user, .. } => crate::sidecar_routes::Caller::Login { username: user.clone() },
    };
    let body = json!({ "workspace": ctx.workspace_id, "notes": notes, "mode": text_param(ctx, "mode").unwrap_or("recent") });
    let r = crate::chat_refresh::handle(body.to_string().as_bytes(), &caller, "permission-card");
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if !r.status.starts_with("200") {
        let (code, message) = error_of(&v, "chat_refresh_refused");
        return Applied::Refused { code, message };
    }
    let harness = v["harness"].as_str().unwrap_or("its harness");
    let delivered = if v["messageDelivered"] == true {
        "It got the summary."
    } else {
        "The summary didn't reach it: it's in the handoff file."
    };
    Applied::Done { message: format!("The pinned chat started again on {harness}. {delivered} The old chat is in Chat history.") }
}

/// The card offers "from now on" while the switch is off.
pub(crate) fn offers_always(req: &PermissionRequest) -> bool {
    req.action == ACTION && !k2_core::workspace::settings::agents_can_refresh_chat(&req.workspace_id)
}

/// [`offers_always`] for a card's view, which knows the action and the
/// workspace by name only.
pub(crate) fn offers_always_for(action: &str, request_id: &str) -> bool {
    if action != ACTION {
        return false;
    }
    matches!(crate::permission::store::get_by_display_id(request_id), Ok(Some(req)) if offers_always(&req))
}

/// `Allow, and let it refresh its chat from now on`: after the one-off, the
/// switch, through the Settings route's own entry as the person who clicked.
/// `Ok(line)` for the card.
pub(crate) fn turn_on_always(ctx: &Ctx, who: &Answerer) -> Result<String, String> {
    let r = crate::sidecar_routes::access_entry(who, &json!({ "toggle": "chat_refresh", "workspace": ctx.workspace_id, "value": true }));
    if let Applied::Refused { message, .. } = applied(&r, "") {
        return Err(message);
    }
    Ok(format!(
        "\"Can refresh its own pinned chat\" is now on for {}: it refreshes its chat itself next time (turn it off in Settings → Workspaces → {} → Agent).",
        ctx.workspace_handle, ctx.workspace_handle
    ))
}

/// The label of the "from now on" answer this request's card offers.
pub(crate) const ALWAYS_LABEL: &str = label::ALWAYS_CHAT_REFRESH;

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &REFRESH, check, apply }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "chat_refresh_tests.rs"]
mod tests;
