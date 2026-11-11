//! `/cli/first-run/*`: the setup checklist (prd-setup-checklist-v1 C1;
//! the daemon–app contract is `setup-checklist-contract.md`).
//! `k2_core::first_run` holds the rows and the store.
//!
//! | Method | Path | Body | Answer |
//! |---|---|---|---|
//! | GET  | `/cli/first-run/status` | | `{enabled, hidden, finished, rows[{id, state, opens, note?}]}` |
//! | POST | `/cli/first-run/hide` | | the same body |
//! | POST | `/cli/first-run/show` | | the same body (hidden, finished and skips cleared) |
//! | POST | `/cli/first-run/skip` | `{"row": "<id>"}` | the same body |
//! | POST | `/cli/first-run/offer/dismiss` | | the same body ("Not now" on Meet K5, per person) |
//! | POST | `/cli/first-run/k5` | `{tool?, name?, handle?, focusGroup?}` | `{workspaceId, name, path, focusGroupId, …}` |
//! | POST | `/cli/first-run/chief-of-staff/start` | | `{workspaceId}` |
//! | POST | `/cli/first-run/chief-of-staff` | `{name, about, tool, focusGroup?}` | `{workspaceId, name, path, focusGroupId, …}` |
//!
//! **K5 at first run** (prd-setup-checklist-v1 §10, `k5-first-run-contract.md`):
//! the last four answer 404 `k5_first_run_off` while the owner's
//! `k5FirstRun` switch is off. `k5` and `chief-of-staff/start` are a
//! person's at the Admin floor (the owner token, an Owner or an Admin);
//! `chief-of-staff` is the one agent-callable route here, honoured only for
//! **K5's own pinned chat**: its passport's workspace is the seat's and its
//! map key is the canonical one (the same check as `k2 agent refresh`), so
//! another agent, K5's sidecar, a shell tab, a login, an API key and the
//! owner token are refused ([`handle_chief_of_staff`]). The rest of the
//! family stays on the passport denylist.
//!
//! **Who.** A person only: the owner token or a Connect login at the
//! Member floor (`route_policy`), resolved to the person key from the
//! credential, never the body (`homes_routes::resolve_person`: the owner
//! token and the logins the owner linked are `owner`; a reserved-name
//! login is nobody). Agent passports are on the denylist
//! (`session_token`, `/cli/first-run/`), app passes and API keys fail
//! `validate_session`, and a Voice & Chat only login is refused by the
//! Member floor. The GET chain (an agent's cell socket) refuses as a belt.
//!
//! **Focus groups.** `focusGroup` (a group's id, or its name) puts the new
//! agent in that group, the way `k2 agent hire --focus-group` and Add
//! Workspace do (`db_ops::focus_groups_assign_project`). It is checked
//! before anything is made: no such group is 400 `unknown_focus_group` and
//! nothing is made. The Chief of Staff joins K5's group when it's left out.
//!
//! **Frames.** `first_run_changed` to that person's sockets on hide, show,
//! skip, dismiss and finish; to everyone when K5 or the Chief of Staff is
//! made (with `projects_changed`).

use k2_core::first_run::{self, Action, Viewer};
use k2_core::notify_prefs::People;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::homes_routes::PersonRefusal;
use crate::notify_routes::Actor;
use crate::session_events::SessionEvent;

pub const STATUS_PATH: &str = "/cli/first-run/status";
pub const HIDE_PATH: &str = "/cli/first-run/hide";
pub const SHOW_PATH: &str = "/cli/first-run/show";
pub const SKIP_PATH: &str = "/cli/first-run/skip";
pub const OFFER_DISMISS_PATH: &str = "/cli/first-run/offer/dismiss";
pub const K5_PATH: &str = "/cli/first-run/k5";
pub const COS_START_PATH: &str = "/cli/first-run/chief-of-staff/start";
pub const COS_PATH: &str = "/cli/first-run/chief-of-staff";

/// What `chief-of-staff/start` posts into K5's Thread, as the person.
pub const COS_START_TEXT: &str = "Let's hire my Chief of Staff.";

/// The longest name a K5 or Chief of Staff may get.
pub const MAX_NAME: usize = 64;

/// Body cap for the POSTs (`{"row": "<id>"}` at most).
pub const MAX_BODY: usize = 4 * 1024;

pub fn is_route(path: &str) -> bool {
    matches!(
        path,
        STATUS_PATH | HIDE_PATH | SHOW_PATH | SKIP_PATH | OFFER_DISMISS_PATH | K5_PATH | COS_START_PATH | COS_PATH
    )
}

pub fn is_post_only(path: &str) -> bool {
    matches!(path, HIDE_PATH | SHOW_PATH | SKIP_PATH | OFFER_DISMISS_PATH | K5_PATH | COS_START_PATH | COS_PATH)
}

/// The K5 routes, which answer 404 while `k5FirstRun` is off.
pub fn is_k5_route(path: &str) -> bool {
    matches!(path, OFFER_DISMISS_PATH | K5_PATH | COS_START_PATH | COS_PATH)
}

fn error(status: &'static str, code: &str, hint: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint.to_string() } }).to_string(),
    }
}

/// 403 for anything that is not a person.
pub fn not_a_person() -> CliResponse {
    error(
        "403 Forbidden",
        "first_run_person_only",
        "the setup list belongs to a person: sign in with your own login (agents, app passes and API keys have none)",
    )
}

/// GET chain door (`cli::dispatch`): never served there.
pub fn refuse_outside_person_arm(path: &str, is_post: bool) -> Option<CliResponse> {
    if !is_route(path) {
        return None;
    }
    if is_post_only(path) && !is_post {
        return Some(CliResponse::method_not_allowed());
    }
    Some(not_a_person())
}

/// Tell `person`'s sockets (every socket when `None`) to re-read
/// `GET /cli/first-run/status`.
pub fn emit_changed(person: Option<String>) {
    let _ = crate::session_events::emit(SessionEvent::FirstRunChanged { person });
}

fn person_for(actor: &Actor) -> Result<String, CliResponse> {
    crate::homes_routes::resolve_person(actor).map_err(|r| match r {
        PersonRefusal::NoLogin => not_a_person(),
        PersonRefusal::Store(e) => error("500 Internal Server Error", "store", e),
        PersonRefusal::Reserved => error(
            "403 Forbidden",
            "first_run_person_only",
            "this login's name is one K2 uses for the server's owner and its agents, so it has no setup list; the owner can remove it in Settings → Admin Access and add the person under another name",
        ),
    })
}

fn viewer_for(actor: &Actor) -> Result<Viewer, CliResponse> {
    if actor.owner_token {
        return Ok(Viewer::OwnerToken);
    }
    match k2_core::connect_users::role_for_user(&actor.login) {
        // The Member floor already refused it; a belt for any other door.
        Some(k2_core::connect_users::Role::VoiceChat) => {
            Err(error("403 Forbidden", "role_required", "the setup list needs a Member, Admin or Owner login"))
        }
        Some(role) => Ok(Viewer::Login(role)),
        None => Err(not_a_person()),
    }
}

fn action_for(path: &str, body: &[u8]) -> Result<Option<Action>, CliResponse> {
    Ok(match path {
        STATUS_PATH => None,
        HIDE_PATH => Some(Action::Hide),
        SHOW_PATH => Some(Action::Show),
        OFFER_DISMISS_PATH => Some(Action::DismissOffer),
        SKIP_PATH => {
            let v: Value = serde_json::from_slice(body)
                .map_err(|e| error("400 Bad Request", "usage", format!("send {{\"row\": \"<id>\"}}: {e}")))?;
            let row = v
                .get("row")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .ok_or_else(|| error("400 Bad Request", "usage", "send {\"row\": \"<id>\"}"))?;
            Some(Action::Skip(row.to_string()))
        }
        _ => return Err(CliResponse::not_found()),
    })
}

/// Handle one `/cli/first-run/*` request for `actor` (the dispatcher's
/// person arm already resolved the credential).
pub fn handle(is_post: bool, path: &str, body: &[u8], actor: &Actor) -> CliResponse {
    if !is_route(path) {
        return CliResponse::not_found();
    }
    if is_post_only(path) != is_post {
        return CliResponse::method_not_allowed();
    }
    if is_k5_route(path) && !k2_core::app_settings::load().k5_first_run {
        return k5_off();
    }
    if path == COS_PATH {
        // A person (or the owner token) on the seat-only route.
        return k5_session_only("this step is K5's: ask K5 on its Thread to hire your Chief of Staff (the setup list's row starts it)");
    }
    let person = match person_for(actor) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let viewer = match viewer_for(actor) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if path == K5_PATH || path == COS_START_PATH {
        if !matches!(viewer, Viewer::OwnerToken | Viewer::Login(k2_core::connect_users::Role::Admin | k2_core::connect_users::Role::Owner)) {
            return error("403 Forbidden", "role_required", "making K5 needs the owner, an Owner or an Admin login");
        }
        return if path == K5_PATH { handle_k5(body, actor) } else { handle_cos_start(actor) };
    }
    let action = match action_for(path, body) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let people = match People::load() {
        Ok(p) => p,
        Err(e) => return error("500 Internal Server Error", "store", e),
    };
    let settings = k2_core::app_settings::load();
    let now = chrono::Utc::now().timestamp();
    let result = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let facts = match first_run::load_facts(&conn, &people, &settings, &person) {
            Ok(f) => f,
            Err(e) => return error("500 Internal Server Error", "store", e),
        };
        match &action {
            None => first_run::status_on(&conn, &people, &person, viewer, &facts, settings.setup_checklist, now),
            Some(a) => {
                first_run::apply_on(&conn, &people, &person, viewer, &facts, settings.setup_checklist, a, now)
            }
        }
    };
    let status = match result {
        Ok(s) => s,
        Err(first_run::Error::UnknownRow(row)) => {
            return error(
                "400 Bad Request",
                "unknown_row",
                format!("{row:?} is not a row on your setup list (k2 setup shows them)"),
            )
        }
        Err(first_run::Error::Store(e)) => return error("500 Internal Server Error", "store", e),
    };
    if action.is_some() || status.newly_finished {
        emit_changed(Some(person));
    }
    match serde_json::to_string(&status) {
        Ok(body) => CliResponse::ok_json(body),
        Err(e) => error("500 Internal Server Error", "serialize", e),
    }
}

// ── K5 at first run (prd-setup-checklist-v1 §10) ─────────────────────

/// 404 while the owner's `k5FirstRun` switch is off.
pub fn k5_off() -> CliResponse {
    error("404 Not Found", "k5_first_run_off", "K5 at first run is off on this server (the owner turns on k5FirstRun)")
}

/// 403 on the seat-only route for anything but K5's own pinned chat.
fn k5_session_only(hint: &str) -> CliResponse {
    error("403 Forbidden", "k5_session_only", hint)
}

/// One K5 or Chief of Staff made at a time, so the 409s hold.
static MAKE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

fn hire_error(e: &k2_core::hire::HireError) -> CliResponse {
    crate::hire_routes::hire_error_response(e)
}

/// `hire::template::apply`'s refusals, as this route's bodies: the folder
/// is taken (`folder_exists`, with `taken` as the hint), or holds another
/// template's agent (`template_conflict`), both 409; a system template an
/// API key can't hire (`system_template`), 403. Anything else as the hire
/// routes answer it.
fn apply_error(e: &k2_core::hire::HireError, taken: String) -> CliResponse {
    match e.code {
        "folder_exists" => error("409 Conflict", "folder_exists", taken),
        "template_conflict" => error("409 Conflict", "template_conflict", &e.hint),
        "system_template" => error("403 Forbidden", "system_template", &e.hint),
        _ => hire_error(e),
    }
}

fn json_body(body: &[u8]) -> Result<Value, CliResponse> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    let v: Value = serde_json::from_slice(body).map_err(|e| error("400 Bad Request", "usage", format!("send a JSON object: {e}")))?;
    if !v.is_object() {
        return Err(error("400 Bad Request", "usage", "send a JSON object"));
    }
    Ok(v)
}

/// A string field, trimmed; `Err` when it is there but not a string or too long.
fn str_field(v: &Value, key: &str, max: usize) -> Result<Option<String>, CliResponse> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.chars().count() > max {
                return Err(error("400 Bad Request", "usage", format!("{key} is at most {max} characters")));
            }
            Ok((!t.is_empty()).then(|| t.to_string()))
        }
        Some(_) => Err(error("400 Bad Request", "usage", format!("{key} must be a string"))),
    }
}

/// The tool, which must be one of claude, codex or grok and signed in.
/// `None` asked: the server default (`defaultAgent`).
fn signed_in_tool(asked: Option<String>) -> Result<String, CliResponse> {
    let entries = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::llm_accounts::list(&conn).map_err(|e| error("500 Internal Server Error", "store", e.to_string()))?
    };
    let signed = first_run::signed_in_tools(&entries);
    let tool = match asked {
        Some(t) => t.to_ascii_lowercase(),
        None => {
            let d = k2_core::app_settings::load().default_agent.trim().to_ascii_lowercase();
            if signed.contains(&d.as_str()) {
                d
            } else if let Some(first) = signed.first() {
                first.to_string()
            } else {
                d
            }
        }
    };
    if !matches!(tool.as_str(), "claude" | "codex" | "grok") {
        return Err(error("400 Bad Request", "usage", format!("tool must be claude, codex or grok (got {tool:?})")));
    }
    if !signed.contains(&tool.as_str()) {
        return Err(error(
            "400 Bad Request",
            "tool_not_signed_in",
            format!("{tool} isn't signed in on this server: sign in first (Settings → LLM tokens, or k2 llm tokens add {tool} <label>)"),
        ));
    }
    Ok(tool)
}

/// 409 when `path` is already some workspace's folder (a new agent needs
/// its own).
fn folder_free(path: &str) -> Result<(), CliResponse> {
    let expanded = k2_core::hire::converge::expand_hire_path(path).map_err(|e| hire_error(&e))?;
    if let Some((p, row)) = k2_core::hire::converge::lookup_project_row(&expanded) {
        return Err(error(
            "409 Conflict",
            "folder_in_use",
            format!("{p} is already the workspace {}; pick another name or handle", row.handle.unwrap_or(row.name)),
        ));
    }
    Ok(())
}

/// The focus group `asked` names (its id, or its name in any case), as an
/// id; checked before anything is made. 400 `unknown_focus_group` when no
/// group is that, `ambiguous_focus_group` when two share the name.
fn focus_group_for(asked: Option<String>) -> Result<Option<String>, CliResponse> {
    let Some(asked) = asked else { return Ok(None) };
    let groups = k2_core::db_ops::focus_groups_list().map_err(|e| error("500 Internal Server Error", "store", e))?;
    if let Some(g) = groups.iter().find(|g| g.id == asked) {
        return Ok(Some(g.id.clone()));
    }
    let named: Vec<_> = groups.iter().filter(|g| g.name.eq_ignore_ascii_case(&asked)).collect();
    match named.as_slice() {
        [g] => Ok(Some(g.id.clone())),
        [] => Err(error(
            "400 Bad Request",
            "unknown_focus_group",
            format!("no focus group is {asked:?}: nothing was made (Settings → Workspaces lists them)"),
        )),
        _ => Err(error("400 Bad Request", "ambiguous_focus_group", format!("more than one focus group is named {asked:?}: send its id"))),
    }
}

/// K5's focus group, if it's in one: where its Chief of Staff goes by default.
fn focus_group_of(workspace_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::Project::get(&conn, workspace_id).ok().and_then(|p| p.focus_group_id)
}

/// Put the new agent in `group`, as Add Workspace and `k2 agent hire
/// --focus-group` do. The agent stands whether or not this works.
fn join_focus_group(report: &k2_core::hire::template::ApplyReport, group: Option<&str>) -> Option<String> {
    let group = group?;
    match k2_core::db_ops::focus_groups_assign_project(&report.workspace_id, Some(group)) {
        Ok(p) => p.focus_group_id,
        Err(e) => {
            k2_core::log_debug!("[first-run] focus group {group} for {}: {e}", report.handle);
            None
        }
    }
}

fn seat_k5() -> Result<Option<k2_core::orchestrator::Live>, CliResponse> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::orchestrator::live_k5(&conn).map_err(|e| error("500 Internal Server Error", "store", e.to_string()))
}

fn handle_of(id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(id).unwrap_or_else(|_| id.to_string())
}

/// The person K5 works for, by the seat's `created_by` (`owner` or
/// `login:<name>`): their display name, for `{{person}}`.
fn seat_person() -> String {
    let created_by = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::orchestrator::load(&conn).ok().flatten().and_then(|s| s.created_by)
    };
    let login = created_by.as_deref().and_then(|c| c.strip_prefix("login:")).unwrap_or(k2_core::notify_prefs::OWNER);
    let people = People::load().unwrap_or(People { users: Vec::new(), linked: Vec::new() });
    k2_core::person_name::effective(login, &people).display_name
}

/// The launches this test binary asked for: `(handle, hello)`.
#[cfg(test)]
pub(crate) static TEST_LAUNCHES: parking_lot::Mutex<Vec<(String, Option<String>)>> = parking_lot::Mutex::new(Vec::new());

/// Start the new agent's pinned chat on `tool` the way the Chat tab's
/// first-chat picker does: `ensure-pinned-chat` with `harness`. The pick is
/// saved as its default agent and the chat starts on it, so a server that
/// has never had a pinned chat doesn't answer `409 harness_choice_needed`
/// (`pinned_chat_choice`) and the person isn't asked again.
fn start_chat(path: &str, tool: &str) -> Result<(), String> {
    let body = json!({ "project": path, "harness": tool }).to_string();
    let r = crate::pinned_chat::handle_ensure_pinned_chat(body.as_bytes());
    if r.status.starts_with('2') {
        Ok(())
    } else {
        Err(format!("{}: {}", r.status, r.body))
    }
}

/// Start the new agent's pinned chat on the chosen tool ([`start_chat`]),
/// then deliver its first-start `hello` off the request thread
/// (`hire_routes`' launch pattern). The agent stands whether or not this
/// works.
fn launch(path: String, handle: String, tool: &str, hello: Option<String>, from: String, sender: crate::hire_routes::Sender) {
    let started = start_chat(&path, tool);
    if let Err(e) = &started {
        k2_core::log_debug!("[first-run] chat {handle} on {tool}: {e}");
    }
    #[cfg(test)]
    {
        let _ = (path, from, sender, started);
        TEST_LAUNCHES.lock().push((handle, hello));
    }
    #[cfg(not(test))]
    {
        if started.is_err() {
            return;
        }
        let spawned = crate::rt::spawn("first-run-launch", move || {
            if let Some(brief) = hello {
                let (r, _) = crate::hire_routes::deliver_recorded(
                    &handle,
                    &brief,
                    &from,
                    "",
                    true,
                    crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
                    "first-run",
                    &sender,
                );
                if !r.success {
                    k2_core::log_debug!("[first-run] hello {handle}: {}", r.reason.unwrap_or_default());
                }
            }
        });
        if let Err(e) = spawned {
            k2_core::log_debug!("[first-run] launch not started: {e}");
        }
    }
}

fn audit(event: &str, actor: &str, outcome: String) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, actor, outcome, "first-run", "-", "cli"));
}

/// The answer both making routes send.
fn made(
    report: &k2_core::hire::template::ApplyReport,
    focus_group: Option<String>,
    granted: &crate::first_run_grants::Granted,
    cards: &Result<(), String>,
) -> CliResponse {
    let body = json!({
        "ok": true,
        "workspaceId": report.workspace_id,
        "name": report.name,
        "path": report.path,
        "handle": report.handle,
        "focusGroupId": focus_group,
        "permissions": {
            "granted": granted.granted,
            "left": granted.left,
            "cards": cards.is_ok(),
        },
    });
    CliResponse::ok_json(body.to_string())
}

/// `POST /cli/first-run/k5 {tool?, name?, handle?, focusGroup?}`: make K5
/// from the `k5` template, seat it, put it in the focus group, set the
/// harness, pinned chat and cards switch, grant the starter set the creator
/// can give, then launch it with its hello.
fn handle_k5(body: &[u8], actor: &Actor) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (tool, name, handle) = match (str_field(&v, "tool", 32), str_field(&v, "name", MAX_NAME), str_field(&v, "handle", MAX_NAME)) {
        (Ok(t), Ok(n), Ok(h)) => (t, n, h),
        (Err(r), _, _) | (_, Err(r), _) | (_, _, Err(r)) => return r,
    };
    let asked_group = match str_field(&v, "focusGroup", 128) {
        Ok(g) => g,
        Err(r) => return r,
    };
    let _made = MAKE_LOCK.lock();
    match seat_k5() {
        Ok(Some(k5)) => {
            return error("409 Conflict", "k5_exists", format!("K5 is already here: {} ({})", k5.name, handle_of(&k5.workspace_id)))
        }
        Ok(None) => {}
        Err(r) => return r,
    }
    let tool = match signed_in_tool(tool) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let name = name.unwrap_or_else(|| k2_core::orchestrator::DEFAULT_NAME.to_string());
    let handle_seed = handle.unwrap_or_else(|| k2_core::orchestrator::DEFAULT_HANDLE.to_string());
    let handle = match k2_core::workspace_session_handles::slugify_address_token(&handle_seed) {
        Ok(h) => h,
        Err(e) => return error("400 Bad Request", "usage", format!("handle: {e}")),
    };
    let root = match k2_core::hire::root::hire_root() {
        Ok(r) => r,
        Err(e) => return hire_error(&e),
    };
    let path = root.join(&handle).to_string_lossy().into_owned();
    if let Err(r) = folder_free(&path) {
        return r;
    }
    let group = match focus_group_for(asked_group) {
        Ok(g) => g,
        Err(r) => return r,
    };
    let (caller, grantor, created_by, from) = if actor.owner_token {
        (
            k2_core::hire::HireCaller::Owner,
            crate::first_run_grants::Grantor::Person(crate::permission::actions::Answerer::OwnerToken),
            "owner".to_string(),
            k2_core::person_name::sender_identity_now(k2_core::person_name::Sender::OwnerToken),
        )
    } else {
        let role = k2_core::connect_users::role_for_user(&actor.login).map(|r| r.as_wire().to_string()).unwrap_or_default();
        (
            k2_core::hire::HireCaller::Login { username: actor.login.clone() },
            crate::first_run_grants::Grantor::Person(crate::permission::actions::Answerer::Login { user: actor.login.clone(), role }),
            format!("login:{}", actor.login),
            actor.login.clone(),
        )
    };
    let mut req = k2_core::hire::template::ApplyRequest::new("k5");
    req.name = Some(name.clone());
    req.handle = Some(handle);
    req.tool = Some(tool.clone());
    // A folder the `k5` template made before (its lock names `k5`) is
    // K5's again: a K5 whose workspace was deleted is made in place.
    let report = match k2_core::hire::template::apply(&req, &caller) {
        Ok(r) => r,
        // `<hire root>/<handle>` is already there and isn't a K5 the
        // template made: K5's folder is taken, so another handle.
        Err(e) => {
            return apply_error(&e, format!("{path} is already there and isn't K5's: pick another handle (k2 setup k5 --handle <h>)"))
        }
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(e) = k2_core::orchestrator::record_k5(&conn, &report.workspace_id, &name, &created_by, chrono::Utc::now().timestamp()) {
            return error("500 Internal Server Error", "store", e.to_string());
        }
    }
    let target = crate::first_run_grants::Target { id: report.workspace_id.clone(), path: report.path.clone(), handle: report.handle.clone() };
    let group = join_focus_group(&report, group.as_deref());
    after_make(&report);
    let cards = crate::first_run_grants::cards_on(&target, &grantor);
    let granted = crate::first_run_grants::grant_set(&crate::first_run_grants::with_extras(k2_core::hire::starter_permissions::K5_STARTER_PERMISSIONS), &target, &grantor);
    audit("first-run-k5", &created_by, format!("ok {} {tool} granted={}", report.handle, granted.granted.join(",")));
    emit_changed(None);
    crate::v1_hire::emit_hire_watches();
    launch(report.path.clone(), report.handle.clone(), &tool, report.hello.clone(), from, crate::hire_routes::Sender::from_hire_caller(&caller, None));
    made(&report, group, &granted, &cards)
}

/// Pinned chat on, for any new agent the first-run routes make (the
/// template already set the harness: `default_agent`, on create).
fn after_make(report: &k2_core::hire::template::ApplyReport) {
    // Pinned chat on: the workspace is an agent with a pinned Chat
    // (agent_mode custom keeps agent_enabled on).
    if let Err(e) = k2_core::workspace::settings::update_project_setting(&report.path, "agent_mode", "custom") {
        k2_core::log_debug!("[first-run] pinned chat for {}: {e}", report.handle);
    }
}

/// `POST /cli/first-run/chief-of-staff/start`: "Let's hire my Chief of
/// Staff." into K5's Thread as the caller (a compose post, so it reaches
/// K5 like the person typed it), after making sure K5's chat is up.
fn handle_cos_start(actor: &Actor) -> CliResponse {
    let k5 = match seat_k5() {
        Ok(Some(k)) => k,
        Ok(None) => return error("409 Conflict", "needs_k5", "there's no K5 yet: make K5 first (k2 setup k5)"),
        Err(r) => return r,
    };
    #[cfg(not(test))]
    if let Err(e) = crate::canonical_session::ensure_canonical_session(&k5.path) {
        return error("500 Internal Server Error", "k5_not_started", format!("K5's chat didn't start: {e}"));
    }
    let addr = handle_of(&k5.workspace_id);
    let params: std::collections::HashMap<String, String> =
        [("addr", addr.as_str()), ("text", COS_START_TEXT), ("via", "compose")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let author = if actor.owner_token { "owner" } else { actor.login.as_str() };
    let r = crate::overlay_routes::dispatch_post_as("/cli/thread/post", &params, b"", author);
    if !r.status.starts_with('2') {
        return r;
    }
    if let Ok(person) = person_for(actor) {
        emit_changed(Some(person));
    }
    CliResponse::ok_json(json!({ "ok": true, "workspaceId": k5.workspace_id }).to_string())
}

/// `POST /cli/first-run/chief-of-staff {name, about, tool, focusGroup?}`:
/// the one
/// agent-callable route here. **Only K5's own pinned chat** may call it:
/// the passport's workspace is the seat's K5 and its map key is the
/// canonical one (`map_key == project_id`, as `k2 agent refresh` checks;
/// `K2_SESSION_ID` is never trusted). Then: the `chief-of-staff` template,
/// the seat, pinned chat, cards, the starter set K5 holds, the connection
/// to K5, the events and the launch with its hello. It joins `focusGroup`,
/// or K5's own focus group when that's left out.
pub fn handle_chief_of_staff(body: &[u8], caller: &crate::sidecar_routes::Caller) -> CliResponse {
    use crate::sidecar_routes::Caller;
    if !k2_core::app_settings::load().k5_first_run {
        return k5_off();
    }
    let k5 = match seat_k5() {
        Ok(k) => k,
        Err(r) => return r,
    };
    let only = "only K5's own pinned chat can hire the Chief of Staff (k2 setup chief-of-staff); ask K5 on its Thread";
    let k5 = match (caller, k5) {
        (Caller::Agent(a), Some(k5)) if a.project_id == k5.workspace_id => {
            if a.map_key.as_deref() != Some(k5.workspace_id.as_str()) || a.sandboxed {
                return k5_session_only(
                    "a sidecar can't hire the Chief of Staff: ask K5's pinned chat (k2 msg <K5's handle>) to run k2 setup chief-of-staff",
                );
            }
            k5
        }
        (Caller::Agent(_), Some(_)) => return k5_session_only(only),
        (Caller::Agent(_), None) => return k5_session_only("there's no K5 on this server, so no agent can hire the Chief of Staff"),
        (Caller::Shell { .. }, _) => {
            return k5_session_only("a shell tab is a person: ask K5 on its Thread to hire your Chief of Staff")
        }
        (Caller::Owner | Caller::Login { .. }, _) => return k5_session_only(only),
        (Caller::WrongCredential | Caller::Invalid, _) => return k5_session_only(only),
    };
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (name, about, tool) = match (str_field(&v, "name", MAX_NAME), str_field(&v, "about", 500), str_field(&v, "tool", 32)) {
        (Ok(n), Ok(a), Ok(t)) => (n, a, t),
        (Err(r), _, _) | (_, Err(r), _) | (_, _, Err(r)) => return r,
    };
    let (Some(name), Some(about), Some(tool)) = (name, about, tool) else {
        return error("400 Bad Request", "usage", "send {\"name\", \"about\", \"tool\"}: ask your person all three first");
    };
    let asked_group = match str_field(&v, "focusGroup", 128) {
        Ok(g) => g,
        Err(r) => return r,
    };
    let _made = MAKE_LOCK.lock();
    let existing = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match k2_core::orchestrator::live_chief_of_staff(&conn) {
            Ok(c) => c,
            Err(e) => return error("500 Internal Server Error", "store", e.to_string()),
        }
    };
    if let Some(c) = existing {
        return error("409 Conflict", "chief_of_staff_exists", format!("the Chief of Staff is already here: {} ({})", c.name, handle_of(&c.workspace_id)));
    }
    let tool = match signed_in_tool(Some(tool)) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let handle = match k2_core::workspace_session_handles::slugify_address_token(&name) {
        Ok(h) => h,
        Err(e) => return error("400 Bad Request", "usage", format!("name: {e}")),
    };
    let root = match k2_core::hire::root::hire_root() {
        Ok(r) => r,
        Err(e) => return hire_error(&e),
    };
    let path = root.join(&handle).to_string_lossy().into_owned();
    if let Err(r) = folder_free(&path) {
        return r;
    }
    let group = match asked_group {
        Some(g) => match focus_group_for(Some(g)) {
            Ok(g) => g,
            Err(r) => return r,
        },
        None => focus_group_of(&k5.workspace_id),
    };
    let k5_handle = handle_of(&k5.workspace_id);
    let caller = k2_core::hire::HireCaller::Agent { workspace_id: k5.workspace_id.clone() };
    let mut req = k2_core::hire::template::ApplyRequest::new("chief-of-staff");
    req.name = Some(name);
    req.handle = Some(handle);
    req.tool = Some(tool.clone());
    req.params.insert("about".into(), about);
    req.params.insert("k5".into(), k5_handle.clone());
    req.params.insert("person".into(), seat_person());
    let report = match k2_core::hire::template::apply(&req, &caller) {
        Ok(r) => r,
        Err(e) => return apply_error(&e, format!("{path} is already there: ask your person for another name")),
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(e) = k2_core::orchestrator::record_chief_of_staff(&conn, &report.workspace_id) {
            return error("500 Internal Server Error", "store", e.to_string());
        }
    }
    let group = join_focus_group(&report, group.as_deref());
    after_make(&report);
    let target = crate::first_run_grants::Target { id: report.workspace_id.clone(), path: report.path.clone(), handle: report.handle.clone() };
    let grantor = crate::first_run_grants::Grantor::Agent { workspace_id: k5.workspace_id.clone() };
    let cards = crate::first_run_grants::cards_on(&target, &grantor);
    let granted = crate::first_run_grants::grant_set(
        &crate::first_run_grants::with_extras(k2_core::hire::starter_permissions::CHIEF_OF_STAFF_STARTER_PERMISSIONS),
        &target,
        &grantor,
    );
    // K5 and its hire can message each other: the seat route is the
    // authority for this one connection (as a person's add would be).
    let connected = k2_core::connections::connections_for_actor(&k5.path, "add", Some(&report.handle), None, true);
    if let Err(e) = &connected {
        k2_core::log_debug!("[first-run] connect {k5_handle} → {}: {e}", report.handle);
    }
    audit(
        "first-run-chief-of-staff",
        &format!("agent:{k5_handle}"),
        format!("ok {} {tool} granted={} connected={}", report.handle, granted.granted.join(","), connected.is_ok()),
    );
    emit_changed(None);
    crate::v1_hire::emit_hire_watches();
    launch(report.path.clone(), report.handle.clone(), &tool, report.hello.clone(), k5_handle, crate::hire_routes::Sender::from_hire_caller(&caller, None));
    made(&report, group, &granted, &cards)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_shape_and_doors() {
        for p in [STATUS_PATH, HIDE_PATH, SHOW_PATH, SKIP_PATH] {
            assert!(is_route(p), "{p}");
        }
        assert!(!is_route("/cli/first-run/statusx") && !is_route("/cli/first-run/"));
        for p in [HIDE_PATH, SHOW_PATH, SKIP_PATH] {
            assert_eq!(refuse_outside_person_arm(p, false).expect("door").status, "405 Method Not Allowed", "{p}");
            let r = refuse_outside_person_arm(p, true).expect("door");
            assert_eq!(r.status, "403 Forbidden", "{p}");
            assert!(r.body.contains("first_run_person_only"), "{}", r.body);
        }
        let r = refuse_outside_person_arm(STATUS_PATH, false).expect("door");
        assert_eq!(r.status, "403 Forbidden");
        assert!(refuse_outside_person_arm("/cli/thread", false).is_none());
        // The handler's own guards: a GET to a POST route, a POST to status.
        let h = handle(false, HIDE_PATH, b"", &Actor::owner_token());
        assert_eq!(h.status, "405 Method Not Allowed");
        let h = handle(true, STATUS_PATH, b"{}", &Actor::owner_token());
        assert_eq!(h.status, "405 Method Not Allowed");
    }

    #[test]
    fn a_login_that_is_not_a_person_is_refused() {
        let _home = crate::test_support::TempHome::new();
        k2_core::connect_users::test_add_legacy_reserved_user("owner", "password123").expect("user named owner");
        let r = handle(false, STATUS_PATH, b"", &Actor::login("ghost"));
        assert_eq!(r.status, "403 Forbidden");
        assert!(r.body.contains("first_run_person_only"), "{}", r.body);
        let r = handle(false, STATUS_PATH, b"", &Actor::login("owner"));
        assert_eq!(r.status, "403 Forbidden", "a reserved name is nobody: {}", r.body);
    }

    #[test]
    fn skip_needs_a_row() {
        for body in [&b""[..], b"{}", br#"{"row": "  "}"#, br#"{"row": 3}"#] {
            let r = action_for(SKIP_PATH, body).expect_err("usage");
            assert_eq!(r.status, "400 Bad Request");
        }
        let ok = |path: &str, body: &[u8]| match action_for(path, body) {
            Ok(a) => a,
            Err(r) => panic!("{path}: {} {}", r.status, r.body),
        };
        assert_eq!(ok(SKIP_PATH, br#"{"row":"phone"}"#), Some(Action::Skip("phone".into())));
        assert_eq!(ok(STATUS_PATH, b""), None);
    }

    /// End to end through the handler on a temp home: the owner token's
    /// list, a skip, the frame, and a Member's narrower list.
    #[test]
    fn the_owner_token_and_a_member_get_their_own_lists() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        // Git runs (whatever this machine has): no dev_tools row.
        let _git = first_run::dev_tools_for_test(first_run::DevTools::NoRow);
        let mut rx = crate::session_events::sender().subscribe();
        let r = handle(false, STATUS_PATH, b"", &Actor::owner_token());
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["enabled"], json!(false), "the switch is off by default");
        let ids: Vec<&str> = v["rows"].as_array().expect("rows").iter().map(|r| r["id"].as_str().expect("id")).collect();
        assert_eq!(ids, vec!["llm_sign_in", "first_agent", "tunnel", "phone", "own_sign_in"]);

        let r = handle(true, SKIP_PATH, br#"{"row":"tunnel"}"#, &Actor::owner_token());
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["rows"][2], json!({ "id": "tunnel", "state": "skipped", "opens": "tunnel" }));
        let mut saw = false;
        while let Ok(ev) = rx.try_recv() {
            // Other tests in this process may send their own frames.
            if let SessionEvent::FirstRunChanged { person } = ev {
                saw |= person.as_deref() == Some("owner");
            }
        }
        assert!(saw, "skip sends first_run_changed to the owner");

        // show clears the skip: the row is to do again.
        let r = handle(true, SHOW_PATH, b"", &Actor::owner_token());
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["rows"][2], json!({ "id": "tunnel", "state": "todo", "opens": "tunnel" }));
        assert_eq!(v["finished"], json!(false));

        let r = handle(true, SKIP_PATH, br#"{"row":"nope"}"#, &Actor::owner_token());
        assert_eq!(r.status, "400 Bad Request");
        assert!(r.body.contains("unknown_row"), "{}", r.body);

        k2_core::connect_users::add_user("anna", "password123").expect("add");
        let r = handle(false, STATUS_PATH, b"", &Actor::login("anna"));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        let ids: Vec<&str> = v["rows"].as_array().expect("rows").iter().map(|r| r["id"].as_str().expect("id")).collect();
        assert_eq!(ids, vec!["first_agent", "phone"], "a Member's rows");
        let r = handle(true, SKIP_PATH, br#"{"row":"tunnel"}"#, &Actor::login("anna"));
        assert_eq!(r.status, "400 Bad Request", "a Member can't skip a row it doesn't see");

        let r = handle(true, HIDE_PATH, b"", &Actor::login("anna"));
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["hidden"], json!(true));
        let r = handle(false, STATUS_PATH, b"", &Actor::owner_token());
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["hidden"], json!(false), "anna's hide is anna's");
    }

    fn rows_of(r: &CliResponse) -> Vec<Value> {
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        v["rows"].as_array().expect("rows").clone()
    }

    fn dev_row(rows: &[Value]) -> Option<Value> {
        rows.iter().find(|r| r["id"] == json!("dev_tools")).cloned()
    }

    /// The developer-tools step through the route, with git's state
    /// injected (never this machine's): absent while git runs; on a Mac
    /// first, to do, the owner token's only, with `installing` while Apple's
    /// installer is open, then done; on Linux the command, no opener, for
    /// the owner token, Owner and Admin.
    #[test]
    fn the_dev_tools_row_through_the_route() {
        use first_run::DevTools;
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        k2_core::connect_users::add_user("olga", "password123").expect("add");
        k2_core::connect_users::set_role("olga", k2_core::connect_users::Role::Owner).expect("role");
        k2_core::connect_users::add_user("adam", "password123").expect("add");
        k2_core::connect_users::set_role("adam", k2_core::connect_users::Role::Admin).expect("role");
        k2_core::connect_users::add_user("mia", "password123").expect("add");
        let status = |actor: &Actor| rows_of(&handle(false, STATUS_PATH, b"", actor));

        {
            let _git = first_run::dev_tools_for_test(DevTools::NoRow);
            assert_eq!(dev_row(&status(&Actor::owner_token())), None, "git runs: no row");
        }
        {
            let _git = first_run::dev_tools_for_test(DevTools::AppleMissing { installing: false });
            let rows = status(&Actor::owner_token());
            assert_eq!(rows[0], json!({ "id": "dev_tools", "state": "todo", "opens": "dev_tools" }), "first, before llm_sign_in");
            assert_eq!(rows[1]["id"], json!("llm_sign_in"));
            for who in ["olga", "adam", "mia"] {
                assert_eq!(dev_row(&status(&Actor::login(who))), None, "{who}: Apple's installer opens on the server's screen");
            }
            let r = handle(true, SKIP_PATH, br#"{"row":"dev_tools"}"#, &Actor::login("adam"));
            assert_eq!(r.status, "400 Bad Request", "a login can't skip a row it doesn't see");
        }
        {
            let _git = first_run::dev_tools_for_test(DevTools::AppleMissing { installing: true });
            let row = dev_row(&status(&Actor::owner_token())).expect("row");
            assert_eq!(row, json!({ "id": "dev_tools", "state": "todo", "opens": "dev_tools", "note": "installing" }));
        }
        {
            let _git = first_run::dev_tools_for_test(DevTools::AppleDone);
            let row = dev_row(&status(&Actor::owner_token())).expect("row");
            assert_eq!(row, json!({ "id": "dev_tools", "state": "done", "opens": "dev_tools" }));
        }
        {
            let _git = first_run::dev_tools_for_test(DevTools::LinuxMissing { command: "sudo apt install git" });
            let want = json!({ "id": "dev_tools", "state": "todo", "opens": null, "note": "sudo apt install git" });
            assert_eq!(status(&Actor::owner_token())[0], want);
            assert_eq!(dev_row(&status(&Actor::login("olga"))), Some(want.clone()), "an Owner login");
            assert_eq!(dev_row(&status(&Actor::login("adam"))), Some(want), "an Admin login");
            assert_eq!(dev_row(&status(&Actor::login("mia"))), None, "a Member");
            // Skippable like the others.
            let r = handle(true, SKIP_PATH, br#"{"row":"dev_tools"}"#, &Actor::owner_token());
            assert_eq!(dev_row(&rows_of(&r)).expect("row")["state"], json!("skipped"));
        }
    }

    /// An install starting and git starting to run tell every client to
    /// re-read the list (the row turns done without a reload).
    #[test]
    fn git_changes_send_first_run_changed_to_everyone() {
        let mut rx = crate::session_events::sender().subscribe();
        crate::git_tools_routes::setup_list_changed();
        let mut saw = false;
        while let Ok(ev) = rx.try_recv() {
            saw |= matches!(ev, SessionEvent::FirstRunChanged { person: None });
        }
        assert!(saw, "first_run_changed to every socket");
    }
}

#[cfg(test)]
#[path = "first_run_k5_tests.rs"]
mod k5_tests;
