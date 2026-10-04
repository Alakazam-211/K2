//! Daemon-side `/cli/feedback/*` route handlers — Feedback F1
//! (prd-agent-feedback-notifications §4.3).
//!
//! Reads (`list`, `show`) are GET query-string routes dispatched
//! through `crate::cli::dispatch`'s domain chain; mutations
//! (`create`, `comment`, `answer`, `resolve`) are JSON-bodied POSTs
//! dispatched by an isolated arm in `routes::dispatcher` (token_ok
//! auth — owner AND connect-user sessions both pass, like
//! `/cli/chat/*`; connect users see + answer feedback too, PRD §4.3).
//!
//! Error contract (mirrors `workspace_not_found_response`): addressing
//! misses answer `{"ok":false,"error":{"code","hint"}}` with a stable
//! code — `not_found` / `ambiguous_id` (candidates listed in the hint)
//! → the CLI exits 4; validation misses use code `usage` → exit 2.
//!
//! Events on the existing `/events` WireEvent broadcast: `create`
//! fires `HookEvent::FeedbackCreated` (`{id, projectPath, title, kind,
//! priority, agentName}`); a recorded answer — the `answer` route OR a
//! human's first comment on a waiting question/approval — fires
//! `HookEvent::FeedbackAnswered` (`{id, projectPath}`); `resolve`
//! fires `HookEvent::FeedbackStatusChanged` (`{id, projectPath,
//! status}`) for resolve / dismiss / reopen-to-waiting; every STORED
//! comment — the `comment` route (agent- and human-authored) and the
//! `answer` route (a recorded answer also creates a thread entry) —
//! additionally fires `HookEvent::FeedbackCommented` (`{id,
//! projectPath, author}`), an internal refresh signal that never
//! drives the desktop notification (only `FeedbackCreated` notifies).
//!
//! F3 injection (prd-ticket-answer-wakes-canonical D2/D5): every
//! HUMAN-authored message (an answer OR a comment) ALWAYS best-effort
//! injects via workspace-agent `deliver_live` + wake=true — the same
//! engine as `k2 msg <workspace>`. v1 ignores the stamp for inject
//! (sidecar / API / true-sandbox do not get a live-only no-wake arm).
//! Injection runs AFTER the store + emit: a delivery failure never
//! fails the store; the outcome rides the response as
//! `delivered`/`deliveryReason`. A human comment with `optionPick:
//! true` (they picked one of the ticket's options) is the ANSWER
//! (set_answer → status `answered` → `FeedbackAnswered`), so
//! `k2 tickets ask --wait` unblocks. A FREE-TEXT human comment moves
//! the ticket to `needs_discussion` (`FeedbackStatusChanged`); the
//! agent replies, settles it, and marks it answered
//! (`resolve` with `status: "answered"` + `answer`) or resolved. Agent-authored
//! comments (`k2 tickets comment` passes the agent's name as
//! `author`) store ONLY — no injection back into their own session,
//! no auto-answer. Resolve / dismiss / reopen never inject.

use std::collections::HashMap;

use crate::cli::{need_project, opt_param, str_param};
use crate::cli_response::CliResponse;
use k2_core::feedback::{self, ListFilter, PrefixError};
use k2_core::skin::SkinPass;

/// Feedback-domain GET dispatch. Returns `Some(resp)` for a handled
/// path, `None` if the path isn't a feedback-domain route.
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    let resp = match path {
        // ── Reads ───────────────────────────────────────────────────
        // GET /cli/feedback/waiting-count — host-wide Tickets badge.
        // Counts only waiting tickets on a registered workspace (not an
        // audit sentinel): the "Waiting on you" rows the page can show.
        "/cli/feedback/waiting-count" => match feedback::count_waiting() {
            Ok(count) => {
                CliResponse::ok_json(serde_json::json!({ "ok": true, "count": count }).to_string())
            }
            Err(e) => usage_error(e),
        },

        // GET /cli/feedback/list?project=<path>[&all=1][&status=<s>]
        // ONE workspace. `all=1` = every STATUS in that workspace, never
        // every workspace: `project` is required, and `all=1` without it
        // is a 400 that points to `list-all`. Default shows open items
        // (waiting + answered + needs_discussion), newest first.
        "/cli/feedback/list" => handle_list(params),

        // GET /cli/feedback/list-all[?all=1][&status=<s>] — every ticket
        // on the host, including ones whose workspace was removed
        // (`linked: false`, `projectName`/`projectPath` null). Same status
        // rules as `list`. Not an agent verb; apps get 404.
        "/cli/feedback/list-all" => handle_list_all(params),

        // GET /cli/feedback/show?id=<id-or-prefix>
        // One item + its full thread. `id` accepts a short unique
        // prefix (ambiguity → `ambiguous_id`, candidates in the hint).
        "/cli/feedback/show" => handle_show(params),

        // ── POST-only mutations reached via the GET chain → 405 ─────
        // (feedback_post_only_route_guards house rule.)
        "/cli/feedback/create"
        | "/cli/feedback/comment"
        | "/cli/feedback/answer"
        | "/cli/feedback/resolve"
        | "/cli/feedback/assign" => CliResponse::method_not_allowed(),

        _ => return None,
    };
    Some(resp)
}

/// Dispatch a `/cli/feedback/*` POST body to its handler. Exact-match
/// paths; unknown paths 404 (mirrors `dispatch_unit6_post`).
///
/// `session_author` is the daemon-resolved actor from the request
/// token (`"owner"` or a Connect username). Human comments that omit
/// `author` store and inject as that identity (D3) so a Connect user
/// is not framed as the host owner.
#[cfg(test)]
pub fn dispatch_post(path: &str, body: &[u8]) -> CliResponse {
    dispatch_post_as(path, body, "owner")
}

/// Glob-arm / test entry. `create` never reaches the glob arm (the exact
/// arm claims it and decides the door, H29); if it ever did, the
/// strictest door (`Owner`, brief policy applies) is the safe default.
pub fn dispatch_post_as(path: &str, body: &[u8], session_author: &str) -> CliResponse {
    dispatch_post_as_gated(path, body, session_author, None, CreateDoor::Owner)
}

/// Which door a `create` came through (prd-ticket-html-brief-v1 H1/H29).
/// Decided by the dispatcher from the authenticating token, never from
/// self-declared body fields (`sessionId`/`sessionKind`/`agentName`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateDoor {
    /// App guest pass (`k2skn_`): a person. Brief optional.
    App,
    /// Connect-user session: a person. Brief optional.
    Connect,
    /// The owner token: `k2 tickets ask` from an agent (in-cell agents
    /// carry the owner token for this verb) or a human at the box
    /// (Rosson default 1). The brief policy applies.
    Owner,
}

pub fn dispatch_post_as_gated(
    path: &str,
    body: &[u8],
    session_author: &str,
    skin: Option<SkinPass>,
    door: CreateDoor,
) -> CliResponse {
    match path {
        "/cli/feedback/create" => handle_create_gated(body, skin.as_ref(), door),
        "/cli/feedback/comment" => handle_comment_gated(body, session_author, skin.as_ref()),
        "/cli/feedback/answer" => handle_answer_gated(body, session_author, skin.as_ref()),
        "/cli/feedback/resolve" => handle_resolve_gated(body, skin.as_ref()),
        "/cli/feedback/assign" => handle_assign_gated(body, skin.as_ref()),
        _ => CliResponse::not_found(),
    }
}

/// Skin tickets GET list. Owner/Connect use [`dispatch`].
pub fn handle_list_gated(params: &HashMap<String, String>, skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_list(params, &pass),
        None => handle_list(params),
    }
}

pub fn handle_show_gated(params: &HashMap<String, String>, skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_show(params, &pass),
        None => handle_show(params),
    }
}

// ── Shared response helpers ───────────────────────────────────────────

/// Stable usage-error shape (code `usage` → CLI exit 2).
fn usage_error(hint: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status: "400 Bad Request",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": "usage", "hint": hint.to_string() },
        })
        .to_string(),
    }
}

/// Stable addressing-miss shapes for id-prefix resolution (both → CLI
/// exit 4; the ambiguous hint lists the matching candidates).
fn prefix_error_response(given: &str, err: PrefixError) -> CliResponse {
    let (code, hint) = match err {
        PrefixError::NotFound => ("not_found", format!("no feedback item matches '{given}'")),
        PrefixError::Ambiguous(candidates) => (
            "ambiguous_id",
            format!(
                "'{given}' matches {} items — use a longer prefix: {}",
                candidates.len(),
                candidates.join(", "),
            ),
        ),
    };
    CliResponse {
        status: "404 Not Found",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint },
        })
        .to_string(),
    }
}

fn project_not_registered(path: &str) -> CliResponse {
    CliResponse {
        status: "404 Not Found",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": "not_found",
                "hint": format!("workspace not registered: {path}"),
            },
        })
        .to_string(),
    }
}

fn resolve_project_id(path: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::workspace::agent_identity::resolve_project_id(&conn, path)
}

/// Hint for `list?all=1` with no `project` (Appa A2): say where the
/// host-wide read is instead of a bare "missing project".
pub const LIST_ALL_NEEDS_PROJECT: &str = "all=1 requires project=; use /cli/feedback/list-all";

fn handle_list(params: &HashMap<String, String>) -> CliResponse {
    match need_project(params) {
        Ok(p) => {
            let Some(project_id) = resolve_project_id(&p) else {
                return project_not_registered(&p);
            };
            list_items(&project_id, params)
        }
        Err(_) if crate::cli::bool_param(params, "all") => usage_error(LIST_ALL_NEEDS_PROJECT),
        Err(r) => r,
    }
}

fn list_filter(params: &HashMap<String, String>) -> ListFilter {
    match opt_param(params, "status") {
        Some(s) => ListFilter::Status(s),
        None if crate::cli::bool_param(params, "all") => ListFilter::All,
        None => ListFilter::Open,
    }
}

fn handle_list_all(params: &HashMap<String, String>) -> CliResponse {
    match feedback::list_host(&list_filter(params)) {
        Ok(items) => {
            CliResponse::ok_json(serde_json::json!({ "ok": true, "items": items }).to_string())
        }
        Err(e) => usage_error(e),
    }
}

fn handle_show(params: &HashMap<String, String>) -> CliResponse {
    let id = str_param(params, "id");
    if id.is_empty() {
        return usage_error("missing id (a feedback id or unique prefix)");
    }
    let full_id = match feedback::resolve_id_prefix(&id) {
        Ok(f) => f,
        Err(e) => return prefix_error_response(&id, e),
    };
    match feedback::get_with_comments(&full_id) {
        Some((item, comments)) => {
            CliResponse::ok_json(show_json(&item, &comments, wants_brief(params)))
        }
        None => prefix_error_response(&id, PrefixError::NotFound),
    }
}

/// `show?brief=1` (H17): add the stored brief (`{html, text, bytes,
/// sha256, sanitizer, createdAt}`, or null). Plain `show` never carries
/// the HTML, so the 300 ms event refetch stays small (H39).
fn wants_brief(params: &HashMap<String, String>) -> bool {
    crate::cli::bool_param(params, "brief")
}

fn list_items(project_id: &str, params: &HashMap<String, String>) -> CliResponse {
    match feedback::list_for_project(project_id, &list_filter(params)) {
        Ok(items) => {
            CliResponse::ok_json(serde_json::json!({ "ok": true, "items": items }).to_string())
        }
        Err(e) => usage_error(e),
    }
}

/// Handle or uuid only. Folder-basename / abs path / unknown → 403 `skin_room`.
fn skin_resolve_project_id(pass: &SkinPass, token: &str) -> Result<String, CliResponse> {
    let token = token.trim();
    if token.is_empty() {
        return Err(crate::skin_routes::skin_room_response());
    }
    let ids = match k2_core::skin::resolve_room_tokens(&[token.to_string()]) {
        Ok(ids) if ids.len() == 1 => ids,
        _ => return Err(crate::skin_routes::skin_room_response()),
    };
    let project_id = ids[0].clone();
    if !pass.has_room(&project_id) {
        return Err(crate::skin_routes::skin_room_response());
    }
    Ok(project_id)
}

fn skin_require_cap(pass: &SkinPass, project_id: &str, cap: &str) -> Result<(), CliResponse> {
    if !pass.has_cap_in_room(project_id, cap) {
        return Err(crate::skin_routes::missing_cap_response(cap));
    }
    Ok(())
}

/// Load by id. Missing / other room → 403 `skin_room` (no 404 oracle).
fn skin_load_item(
    pass: &SkinPass,
    id: &str,
    cap: &str,
) -> Result<feedback::FeedbackItem, CliResponse> {
    let full_id = match feedback::resolve_id_prefix(id) {
        Ok(f) => f,
        Err(_) => return Err(crate::skin_routes::skin_room_response()),
    };
    let Some(item) = feedback::get_item(&full_id) else {
        return Err(crate::skin_routes::skin_room_response());
    };
    if !pass.has_room(&item.project_id) {
        return Err(crate::skin_routes::skin_room_response());
    }
    skin_require_cap(pass, &item.project_id, cap)?;
    Ok(item)
}

fn handle_skin_list(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let project = params
        .get("project")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let Some(project) = project else {
        return usage_error("missing 'project' (workspace handle or UUID)");
    };
    let project_id = match skin_resolve_project_id(pass, &project) {
        Ok(id) => id,
        Err(e) => return e,
    };
    if let Err(e) = skin_require_cap(pass, &project_id, crate::skin_routes::TICKETS_READ) {
        return e;
    }
    guest_projection(list_items(&project_id, params))
}

fn handle_skin_show(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let id = str_param(params, "id");
    if id.is_empty() {
        return usage_error("missing id (a feedback id or unique prefix)");
    }
    let item = match skin_load_item(pass, &id, crate::skin_routes::TICKETS_READ) {
        Ok(item) => item,
        Err(e) => return e,
    };
    match feedback::get_with_comments(&item.id) {
        Some((item, comments)) => guest_projection(CliResponse::ok_json(show_json(
            &item,
            &comments,
            wants_brief(params),
        ))),
        None => crate::skin_routes::skin_room_response(),
    }
}

/// `projects.name` + `projects.path` for a project id (for the
/// `workspace` field in responses + the `projectPath` event field).
fn project_name_path(project_id: &str) -> (Option<String>, Option<String>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT name, path FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
            ))
        },
    )
    .unwrap_or((None, None))
}

/// That project's pinned-chat conversation id (`workspace_sessions.session_id`).
/// Used by the Agent tab's D6 check so a poison `sandbox` stamp on the
/// canonical id still wakes via `ensure-pinned-chat`.
fn canonical_session_id(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::WorkspaceSession::get(&conn, project_id)
        .ok()
        .flatten()
        .and_then(|r| r.session_id)
        .filter(|s| !s.is_empty())
}

/// `feedback:commented` on the /events broadcast (`{id, projectPath,
/// author}`) — fired whenever a comment is STORED on a thread: the
/// comment route (agent- and human-authored alike) and the answer
/// route (a recorded answer also creates a thread entry). INTERNAL
/// refresh signal only — the renderer refetches the open thread and
/// bumps list comment counts; it must never drive the desktop
/// notification (frozen contract: only NEW items notify, via
/// `FeedbackCreated`).
fn emit_commented(feedback_id: &str, author: &str) {
    let path = feedback::get_item(feedback_id).and_then(|i| project_name_path(&i.project_id).1);
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::FeedbackCommented,
        serde_json::json!({
            "id": feedback_id,
            "projectPath": path,
            "author": author,
        }),
    );
}

// ── prd-app-tickets-websocket-v1 — one `ticket_changed` per mutation ──

/// `change` values on [`crate::session_events::SessionEvent::TicketChanged`]
/// (and the app frame). One stored mutation emits exactly one.
pub const CHANGE_CREATED: &str = "created";
pub const CHANGE_STATUS: &str = "status_changed";
pub const CHANGE_ASSIGNED: &str = "assigned";
pub const CHANGE_ANSWERED: &str = "answered";
pub const CHANGE_COMMENTED: &str = "commented";
pub const CHANGE_RESOLVED: &str = "resolved";
pub const CHANGE_DISMISSED: &str = "dismissed";

/// `via` on `answered`: a person picked one of the ticket's options.
pub const VIA_OPTION_PICK: &str = "option_pick";
/// `via` on `answered`: the `/cli/feedback/answer` route.
pub const VIA_ANSWER: &str = "answer";
/// `via` on `answered`: the agent settled a discussion (`resolve --answered`).
pub const VIA_SETTLED: &str = "settled";
/// `via` on `commented`: a person's free-text reply (opens a discussion).
pub const VIA_FREE_TEXT: &str = "free_text";
/// `via` on `commented`: an agent-authored comment.
pub const VIA_AGENT: &str = "agent";

/// The `change` a `resolve` to `status` reports.
pub(crate) fn resolve_change(status: &str) -> &'static str {
    match status {
        "resolved" => CHANGE_RESOLVED,
        "dismissed" => CHANGE_DISMISSED,
        _ => CHANGE_STATUS,
    }
}

/// Emit the per-ticket live event after a stored mutation. Ids and
/// metadata only (no title, body, names or brief). Best-effort: no
/// subscriber is not an error.
fn emit_ticket_changed(item: &feedback::FeedbackItem, change: &str, via: Option<&str>) {
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::TicketChanged {
        project_id: item.project_id.clone(),
        id: item.id.clone(),
        change: change.to_string(),
        status: item.status.clone(),
        via: via.map(str::to_string),
        has_brief: item.has_brief,
    });
}

/// Keys an app guest never gets on a ticket (prd-app-tickets-websocket-v1
/// D6): absolute paths and session ids are host internals. The room is
/// `projectId` / `workspace`.
pub const GUEST_HIDDEN_KEYS: [&str; 4] =
    ["projectPath", "canonicalSessionId", "sessionId", "sessionKind"];

fn strip_guest_keys(v: &mut serde_json::Value) {
    if let Some(map) = v.as_object_mut() {
        for k in GUEST_HIDDEN_KEYS {
            map.remove(k);
        }
    }
}

/// A `list` / `show` body re-shaped for an app guest.
fn guest_projection(resp: CliResponse) -> CliResponse {
    if resp.status != "200 OK" {
        return resp;
    }
    let mut v: serde_json::Value = match serde_json::from_str(&resp.body) {
        Ok(v) => v,
        Err(e) => return CliResponse::internal_error(format!("ticket projection: {e}")),
    };
    strip_guest_keys(&mut v);
    if let Some(items) = v.get_mut("items").and_then(|i| i.as_array_mut()) {
        for item in items {
            strip_guest_keys(item);
        }
    }
    CliResponse::ok_json(v.to_string())
}

/// The `show` wire shape (mockup contract): the item's fields flat at
/// the top level + `workspace` (project name) + `comments` (each with
/// the mockup's `at` alias alongside `createdAt`).
///
/// `with_brief` adds `brief` (the stored brief only: no path or session
/// fields ride inside it, H40).
fn show_json(
    item: &feedback::FeedbackItem,
    comments: &[feedback::FeedbackComment],
    with_brief: bool,
) -> String {
    let (name, path) = project_name_path(&item.project_id);
    let mut v = serde_json::to_value(item).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(map) = v.as_object_mut() {
        map.insert("ok".to_string(), serde_json::json!(true));
        map.insert("workspace".to_string(), serde_json::json!(name));
        map.insert("projectPath".to_string(), serde_json::json!(path));
        map.insert(
            "canonicalSessionId".to_string(),
            serde_json::json!(canonical_session_id(&item.project_id)),
        );
        map.insert(
            "comments".to_string(),
            serde_json::json!(comments
                .iter()
                .map(|c| serde_json::json!({
                    "author": c.author,
                    "body": c.body,
                    "at": c.created_at,
                }))
                .collect::<Vec<_>>()),
        );
        if with_brief {
            map.insert(
                "brief".to_string(),
                serde_json::json!(k2_core::feedback_brief::get(&item.id)),
            );
        }
    }
    v.to_string()
}

// ── F3 — human message → asking-session injection ─────────────────────

/// Where a human-authored message (answer OR comment) gets injected
/// (F3, PRD §7 decision 1: human messages ALWAYS inject;
/// agent-authored comments and resolve/dismiss/reopen never do).
#[derive(Debug, Clone, PartialEq, Eq)]
enum InjectionTarget {
    /// Reserved: return-to-asker-cell (`k2 msg ws/handle` / live-only
    /// sandbox) is out of v1. Kept so the classifier stays an enum.
    #[allow(dead_code)]
    SandboxSession(String),
    /// Workspace-agent addressing (the `k2 msg <workspace>` path),
    /// wake=true — a dormant canonical agent is woken and the message
    /// delivered on wake. v1 always takes this arm (D5).
    WorkspaceAgent,
}

/// Pure injection-target classifier, unit-tested without touching the
/// DB or any live session. v1 always returns [`InjectionTarget::WorkspaceAgent`].
fn injection_target(_session_id: Option<&str>, _session_kind: Option<&str>) -> InjectionTarget {
    InjectionTarget::WorkspaceAgent
}

/// The injected body (PRD §4.3): `[feedback:<short-id>] <body>` —
/// shared by the answer route AND human comments so both read the
/// same in-session. The `[from <sender>]` attribution prefix is added
/// by the shared msg framing, so the line reads like any other
/// `k2 msg` delivery; the short id is a resolvable prefix
/// (`k2 feedback show <short-id>`).
fn feedback_payload(feedback_id: &str, body: &str) -> String {
    let short: String = feedback_id.chars().take(8).collect();
    format!("[feedback:{short}] {body}")
}

/// Best-effort delivery of a just-stored human message (answer or
/// comment) into the asking session. The caller has already stored it
/// (and emitted any event) — this only reports the outcome:
/// `(delivered, reason, target_session_id)`.
fn deliver_to_asker(
    item: &feedback::FeedbackItem,
    from: &str,
    body: &str,
) -> (bool, Option<String>, Option<String>) {
    let payload = feedback_payload(&item.id, body);
    // D5: v1 always WorkspaceAgent. Classifier is kept + called so the
    // unit matrix stays honest; the SandboxSession arm is never taken.
    let _ = injection_target(item.session_id.as_deref(), item.session_kind.as_deref());
    let (_, path) = project_name_path(&item.project_id);
    let Some(path) = path else {
        return (false, Some("workspace_not_found".to_string()), None);
    };
    let resp = crate::workspace_msg::deliver_live(
        &path,
        &payload,
        from,
        "",
        true,
        crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
    );
    (resp.success, resp.reason, resp.target_session_id)
}

// ── POST handlers ─────────────────────────────────────────────────────

/// `POST /cli/feedback/create` body. `project` accepts a workspace
/// name, absolute path, or project UUID (resolved via
/// [`crate::workspace_msg::resolve_workspace`], same as
/// `/cli/workspace/set`). `agentName` defaults to the workspace's
/// agent display name. Session fields are optional — an ask filed
/// outside any known session must still succeed.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct CreateBody {
    project: String,
    session_id: Option<String>,
    session_kind: Option<String>,
    agent_name: Option<String>,
    kind: Option<String>,
    title: String,
    body: Option<String>,
    options: Option<Vec<String>>,
    priority: Option<i64>,
    /// Optional username snapshots to assign at create time (`owner` or
    /// connect-user names). Applied after insert so push targeting and
    /// the create response both see them. Empty/omitted = unassigned.
    assignees: Option<Vec<String>>,
    /// Ticket brief (prd-ticket-html-brief-v1): raw HTML. Cleaned and
    /// capped here; required on the owner door per the brief policy.
    brief_html: Option<String>,
}

/// Handler for `POST /cli/feedback/create`. Validates, inserts, and
/// fires `FeedbackCreated` on the `/events` broadcast. Response is the
/// mockup's ask `--json` shape (`ok/id/title/kind/priority/status/
/// options/workspace/sessionId`) plus the full item fields.
#[cfg(test)]
pub fn handle_create(body: &[u8]) -> CliResponse {
    handle_create_gated(body, None, CreateDoor::Owner)
}

/// 400 with a `brief_*` code (H3). The CLI maps every `brief_*` code to
/// exit 2 and prints the hint.
fn brief_error(code: &str, hint: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status: "400 Bad Request",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint.to_string() },
        })
        .to_string(),
    }
}

/// 413 for a `create` body over [`k2_core::feedback_brief::MAX_CREATE_BODY_BYTES`],
/// answered by the dispatcher before the body is read (H30).
pub fn create_body_too_large(declared: Option<usize>) -> CliResponse {
    let size = match declared {
        Some(n) => format!("{n} bytes"),
        None => "a body with no Content-Length".to_string(),
    };
    CliResponse {
        status: "413 Payload Too Large",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": "brief_too_large",
                "hint": format!(
                    "the request is {size}; a ticket (brief included) must fit in {} bytes. \
                     The brief cap is {} bytes. See `k2 study ticket-brief`.",
                    k2_core::feedback_brief::MAX_CREATE_BODY_BYTES,
                    k2_core::feedback_brief::MAX_BRIEF_BYTES,
                ),
            },
        })
        .to_string(),
    }
}

fn handle_create_gated(body: &[u8], skin: Option<&SkinPass>, door: CreateDoor) -> CliResponse {
    let b: CreateBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.project.is_empty() {
        return usage_error("missing 'project' (workspace name | path | UUID)");
    }
    if b.title.trim().is_empty() {
        return usage_error("ask requires a <title>");
    }
    if let Some(p) = b.priority {
        if !(1..=5).contains(&p) {
            return usage_error("--priority must be 1-5");
        }
    }
    if let Some(k) = b.kind.as_deref() {
        if !feedback::KINDS.contains(&k) {
            return usage_error(format!(
                "invalid kind '{k}' — valid: question, approval, fyi"
            ));
        }
    }
    let (project_id, path) = match resolve_create_project(&b.project, skin) {
        Ok(pair) => pair,
        Err(e) => return e,
    };

    // Ticket brief (H1/H11/H25/H27). Clean BEFORE the DB lock (H33c).
    // An empty string counts as absent. The policy only binds the owner
    // door; people (app guests, Connect users) may attach one.
    let kind_for_policy = b.kind.clone().unwrap_or_else(|| "question".to_string());
    let mut warnings: Vec<k2_core::feedback_brief::BriefWarning> = Vec::new();
    let brief = match b.brief_html.as_deref().filter(|h| !h.trim().is_empty()) {
        Some(raw) => match k2_core::feedback_brief::clean(raw) {
            Ok(clean) => {
                warnings.extend(clean.warnings());
                Some(clean)
            }
            Err(e) => return brief_error(e.code(), e.hint()),
        },
        None => {
            if door == CreateDoor::Owner
                && k2_core::feedback_brief::brief_required(&kind_for_policy)
            {
                match k2_core::feedback_brief::brief_policy() {
                    k2_core::feedback_brief::BriefPolicy::Require => {
                        return brief_error(
                            "brief_required",
                            k2_core::feedback_brief::BRIEF_REQUIRED_HINT,
                        );
                    }
                    k2_core::feedback_brief::BriefPolicy::Warn => {
                        warnings.push(k2_core::feedback_brief::missing_warning());
                    }
                }
            }
            None
        }
    };

    // Assignee policy (0.43.2, `feedback_brief::ASSIGNEE_POLICY`). Same
    // door and `fyi` exemption as the brief. A name that is not a user on
    // this server warns on every door; only the agent door under Require
    // is refused. Checked BEFORE the insert so a refusal stores nothing.
    let assignees = k2_core::feedback_brief::clean_assignees(b.assignees.as_deref().unwrap_or(&[]));
    let assignee_policy_applies =
        door == CreateDoor::Owner && k2_core::feedback_brief::assignee_required(&kind_for_policy);
    let unknown = unknown_assignees_here(&assignees);
    let require = k2_core::feedback_brief::assignee_policy()
        == k2_core::feedback_brief::AssigneePolicy::Require;
    if assignee_policy_applies && require {
        if assignees.is_empty() {
            return brief_error(
                "assignee_required",
                format!(
                    "{} {}",
                    k2_core::feedback_brief::ASSIGNEE_REQUIRED_MESSAGE,
                    k2_core::feedback_brief::ASSIGNEE_REQUIRED_HINT
                ),
            );
        }
        if !unknown.is_empty() {
            return brief_error(
                "assignee_unknown",
                k2_core::feedback_brief::assignee_unknown_hint(&unknown),
            );
        }
    }
    if !unknown.is_empty() {
        warnings.push(k2_core::feedback_brief::assignee_unknown_warning(&unknown));
    }

    // Asker attribution: explicit agentName wins; otherwise the
    // workspace's agent display name (always returns a string).
    let agent_name = b
        .agent_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| k2_core::workspace::display::agent_display_name(&path));
    // session_kind only means something next to a session_id; accept
    // canonical|sandbox|sidecar|api, drop anything else (fail-open to
    // NULL — a bad kind hint must never fail the ask). D4: do not
    // fail-open *known* kinds to NULL.
    let session_id = b.session_id.filter(|s| !s.trim().is_empty());
    let session_kind = match (&session_id, b.session_kind.as_deref()) {
        (Some(_), Some(k @ ("canonical" | "sandbox" | "sidecar" | "api"))) => Some(k.to_string()),
        _ => None,
    };

    let mut item = match feedback::create_with_brief(
        feedback::NewFeedback {
            project_id,
            session_id,
            session_kind,
            agent_name,
            kind: b.kind.unwrap_or_default(),
            title: b.title,
            body: b.body,
            options: b.options,
            priority: b.priority.unwrap_or(0),
        },
        brief,
    ) {
        Ok(item) => item,
        Err(e) => return usage_error(e),
    };

    // Optional assignees at create — set before push so mobile targeting
    // and the create response include them. Snapshots only (no FK).
    if !assignees.is_empty() {
        match feedback::set_assignees(&item.id, &assignees) {
            Ok(updated) => item = updated,
            Err(e) => return usage_error(e),
        }
    } else if assignee_policy_applies {
        // Warn policy (Require returned above): filed, but say so, with
        // the id so the fix is one copy-paste.
        warnings.push(k2_core::feedback_brief::assignee_missing_warning(Some(&item.id)));
    }

    // FeedbackCreated on the existing /events broadcast (frozen
    // contract: {id, projectPath, title, kind, priority, agentName}).
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::FeedbackCreated,
        serde_json::json!({
            "id": item.id,
            "projectPath": path,
            "title": item.title,
            "kind": item.kind,
            "priority": item.priority,
            "agentName": item.agent_name,
        }),
    );
    // Companion C4 — mobile push, next to the emit. ONLY new items
    // push (the frozen only-created-notifies contract); the event is
    // content-free (agent name + id, NEVER the ask's title/body —
    // §4.5) and dormant/fire-and-forget inside push_routes. Assignees
    // narrow the fan-out when present.
    crate::push_routes::notify_feedback_created(&item.agent_name, &item.id, &item.assignees);
    emit_ticket_changed(&item, CHANGE_CREATED, None);

    let (name, _) = project_name_path(&item.project_id);
    let mut v = serde_json::to_value(&item).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(map) = v.as_object_mut() {
        map.insert("ok".to_string(), serde_json::json!(true));
        map.insert("workspace".to_string(), serde_json::json!(name));
        // H17/H25: `hasBrief`/`briefBytes` ride on the item; warnings are
        // always an array so the CLI (H28 skew check) can rely on it.
        map.insert("warnings".to_string(), serde_json::json!(warnings));
    }
    CliResponse::ok_json(v.to_string())
}

/// The names in `assignees` that are not users on this server: the host
/// owner (the literal `owner` or the owner display name) or a stored
/// Connect user, the same people `k2 connections list --users` prints.
/// If the user store can't be read, nothing is reported (a broken store
/// must not make every assignee look unknown).
fn unknown_assignees_here(assignees: &[String]) -> Vec<String> {
    if assignees.is_empty() {
        return Vec::new();
    }
    let owner = crate::workspace_msg::resolve_owner_from();
    match k2_core::connect_users::list_people_for_agents(&owner) {
        Ok(rows) => {
            let known: Vec<String> = rows.into_iter().map(|r| r.username).collect();
            k2_core::feedback_brief::unknown_assignees(assignees, &known)
        }
        Err(e) => {
            k2_core::log_debug!("[feedback] assignee check skipped: people list failed: {e}");
            Vec::new()
        }
    }
}

fn resolve_create_project(
    project: &str,
    skin: Option<&SkinPass>,
) -> Result<(String, String), CliResponse> {
    if let Some(pass) = skin {
        let project_id = skin_resolve_project_id(pass, project)?;
        skin_require_cap(pass, &project_id, crate::skin_routes::TICKETS_POST)?;
        let Some(path) = project_name_path(&project_id)
            .1
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
        else {
            return Err(crate::skin_routes::skin_room_response());
        };
        return Ok((project_id, path));
    }
    let Some(path) = crate::workspace_msg::resolve_workspace(project) else {
        return Err(crate::workspace_routes::workspace_not_found_response(
            project,
        ));
    };
    let Some(project_id) = resolve_project_id(&path) else {
        return Err(project_not_registered(&path));
    };
    Ok((project_id, path))
}

/// `POST /cli/feedback/comment` body. `author` defaults to `owner`
/// (the renderer's thread panel posts author-less); agents pass their
/// own name via the CLI — that's how human and agent comments are told
/// apart (see [`handle_comment`]).
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct CommentBody {
    id: String,
    body: String,
    author: Option<String>,
    /// The person picked one of the ticket's options (a structured
    /// `--options` button or a brief `k2-options` item). An option pick
    /// ANSWERS the ticket; free text (false / absent) opens a discussion.
    /// Ignored on agent-authored comments.
    option_pick: bool,
}

/// The status a HUMAN comment moves a ticket to. An option pick is the
/// answer (`answered`); free text means the person wants to talk it over
/// (`needs_discussion`). The agent settles a discussion later with
/// `k2 tickets resolve <id> --answered "<outcome>"` or plain `resolve`.
pub(crate) fn human_comment_status(option_pick: bool) -> &'static str {
    if option_pick {
        "answered"
    } else {
        "needs_discussion"
    }
}

/// Handler for `POST /cli/feedback/comment`.
///
/// It's just a comment thread — but HUMAN comments land in the
/// terminal session (the locked direction that retired the renderer's
/// Answer-vs-Comment split):
///
/// - HUMAN-authored (`author` absent or `owner` — the renderer/API
///   default; `k2 feedback comment` always self-identifies with the
///   agent's name, so an agent never matches):
///   - `optionPick: true` (the person picked one of the ticket's
///     options) IS the answer: `set_answer` → status `answered` →
///     `FeedbackAnswered` emit — `ask --wait` unblocks and prints it.
///   - free text (`optionPick` false/absent) stores the comment and
///     moves the ticket to `needs_discussion` (`FeedbackStatusChanged`
///     when it changed). The agent replies, settles it, then marks it
///     answered or resolved (`k2 tickets resolve <id> [--answered]`).
///   - ALWAYS best-effort injects into the asking session via the
///     shared F3 machinery ([`deliver_to_asker`], wake=true) AFTER
///     the store + emit; a delivery failure never fails the store.
///     The outcome rides the response (`delivered`/`deliveryReason`/
///     `deliveredSessionId`, plus `answered` for the auto-answer).
/// - AGENT-authored: store only (thread bump), no injection back into
///   its own session, no auto-answer, no delivery fields.
#[cfg(test)]
pub fn handle_comment(body: &[u8]) -> CliResponse {
    handle_comment_as(body, "owner")
}

#[cfg(test)]
pub fn handle_comment_as(body: &[u8], session_author: &str) -> CliResponse {
    handle_comment_gated(body, session_author, None)
}

fn handle_comment_gated(body: &[u8], session_author: &str, skin: Option<&SkinPass>) -> CliResponse {
    let b: CommentBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.id.is_empty() {
        return usage_error("missing 'id' (a feedback id or unique prefix)");
    }
    if b.body.trim().is_empty() {
        return usage_error("comment requires non-empty <text>");
    }
    let (full_id, author, is_human) = if let Some(pass) = skin {
        let item = match skin_load_item(pass, &b.id, crate::skin_routes::TICKETS_POST) {
            Ok(item) => item,
            Err(e) => return e,
        };
        // Skin: session_author = pass.username. Never body `author`, never "owner".
        (item.id, pass.username.clone(), true)
    } else {
        let full_id = match feedback::resolve_id_prefix(&b.id) {
            Ok(f) => f,
            Err(e) => return prefix_error_response(&b.id, e),
        };
        // Body `author` is the agent-CLI path. UI human posts omit it —
        // use the token identity so a Connect user is stored/injected as
        // themselves, not `resolve_owner_from()`.
        let author_given = b.author.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let session = session_author.trim();
        let author = author_given.unwrap_or(if session.is_empty() { "owner" } else { session });
        let is_human = author_given.is_none() || author == "owner";
        (full_id, author.to_string(), is_human)
    };
    let author = author.as_str();

    // Agent comment — store only, current shape (no delivery fields).
    if !is_human {
        return match feedback::add_comment(&full_id, author, &b.body) {
            Ok(c) => {
                emit_commented(&full_id, &c.author);
                // Push assignees (or all devices if unassigned) on thread
                // activity — not just create.
                if let Some(item) = feedback::get_item(&full_id) {
                    emit_ticket_changed(&item, CHANGE_COMMENTED, Some(VIA_AGENT));
                    crate::push_routes::notify_feedback_commented(&full_id, &item.assignees);
                }
                CliResponse::ok_json(
                    serde_json::json!({
                        "ok": true,
                        "id": full_id,
                        "commentId": c.id,
                        "author": c.author,
                    })
                    .to_string(),
                )
            }
            Err(e) => usage_error(e),
        };
    }

    // Human comment. An OPTION PICK is the answer (status answered,
    // --wait prints it). FREE TEXT opens a discussion: the comment is
    // stored and the ticket moves to needs_discussion until the agent
    // settles it (`k2 tickets resolve <id> [--answered "<outcome>"]`).
    let Some(before) = feedback::get_item(&full_id) else {
        return prefix_error_response(&b.id, PrefixError::NotFound);
    };
    let answers = b.option_pick;
    let target_status = human_comment_status(b.option_pick);

    let (item, comment) = if answers {
        match feedback::set_answer(&full_id, author, &b.body) {
            Ok(pair) => pair,
            Err(e) => return usage_error(e),
        }
    } else {
        let c = match feedback::add_comment(&full_id, author, &b.body) {
            Ok(c) => c,
            Err(e) => return usage_error(e),
        };
        let item = if before.status == target_status {
            match feedback::get_item(&full_id) {
                Some(item) => item,
                None => return usage_error("feedback row vanished after comment"),
            }
        } else {
            match feedback::set_status(&full_id, target_status) {
                Ok(item) => item,
                Err(e) => return usage_error(e),
            }
        };
        (item, c)
    };
    let status_changed = !answers && before.status != item.status;

    // FeedbackAnswered BEFORE the injection so `ask --wait` pollers
    // unblock even if delivery is slow (a wake can take seconds).
    let (_, path) = project_name_path(&item.project_id);
    if answers {
        k2_core::agent_hooks::emit(
            k2_core::agent_hooks::HookEvent::FeedbackAnswered,
            serde_json::json!({
                "id": item.id,
                "projectPath": path,
            }),
        );
    }
    // Free text moved the ticket to needs_discussion: every window's
    // list + waiting-count badge refresh on the status bus.
    if status_changed {
        k2_core::agent_hooks::emit(
            k2_core::agent_hooks::HookEvent::FeedbackStatusChanged,
            serde_json::json!({
                "id": item.id,
                "projectPath": path,
                "status": item.status,
            }),
        );
    }
    // feedback:commented rides every stored comment (see
    // [`emit_commented`]) — also before the injection, so an open
    // thread panel refreshes without waiting on a slow wake.
    emit_commented(&item.id, &comment.author);
    // One app frame for this reply: an option pick is the answer; free
    // text is a comment whose `status` says needs_discussion.
    if answers {
        emit_ticket_changed(&item, CHANGE_ANSWERED, Some(VIA_OPTION_PICK));
    } else {
        emit_ticket_changed(&item, CHANGE_COMMENTED, Some(VIA_FREE_TEXT));
    }
    crate::push_routes::notify_feedback_commented(&item.id, &item.assignees);

    // Shared F3 delivery. Owner token → server display name. Connect
    // user → their username. Matches project-group/msg framing.
    let from = if author == "owner" {
        crate::workspace_msg::resolve_owner_from()
    } else {
        author.to_string()
    };
    let (delivered, delivery_reason, delivered_session) =
        deliver_to_asker(&item, &from, &comment.body);

    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "id": item.id,
            "commentId": comment.id,
            "author": comment.author,
            "answered": answers,
            "status": item.status,
            "delivered": delivered,
            "deliveryReason": delivery_reason,
            "deliveredSessionId": delivered_session,
        })
        .to_string(),
    )
}

/// `POST /cli/feedback/answer` body. Stores (thread comment +
/// denormalized `answer` + `answered_at` + status `answered`), fires
/// `FeedbackAnswered`, then best-effort injects the answer into the
/// asking session (F3 — see [`deliver_to_asker`]). Kept for API
/// compat — the renderer's thread panel now posts plain comments
/// (a human's first comment on a waiting ask answers it).
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct AnswerBody {
    id: String,
    answer: String,
    author: Option<String>,
}

/// Handler for `POST /cli/feedback/answer`.
#[cfg(test)]
pub fn handle_answer(body: &[u8]) -> CliResponse {
    handle_answer_gated(body, "owner", None)
}

fn handle_answer_gated(body: &[u8], session_author: &str, skin: Option<&SkinPass>) -> CliResponse {
    let b: AnswerBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.id.is_empty() {
        return usage_error("missing 'id' (a feedback id or unique prefix)");
    }
    if b.answer.trim().is_empty() {
        return usage_error("answer requires non-empty text");
    }
    let (full_id, author_owned, from_owned) = if let Some(pass) = skin {
        let item = match skin_load_item(pass, &b.id, crate::skin_routes::TICKETS_POST) {
            Ok(item) => item,
            Err(e) => return e,
        };
        let name = pass.username.clone();
        (item.id, name.clone(), name)
    } else {
        let full_id = match feedback::resolve_id_prefix(&b.id) {
            Ok(f) => f,
            Err(e) => return prefix_error_response(&b.id, e),
        };
        let author = b.author.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let session = session_author.trim();
        let stored = author
            .unwrap_or(if session.is_empty() { "owner" } else { session })
            .to_string();
        let from = author
            .map(String::from)
            .unwrap_or_else(crate::workspace_msg::resolve_owner_from);
        (full_id, stored, from)
    };
    let (item, comment) = match feedback::set_answer(&full_id, &author_owned, &b.answer) {
        Ok(pair) => pair,
        Err(e) => return usage_error(e),
    };

    // FeedbackAnswered on the /events broadcast ({id, projectPath}).
    // Emitted BEFORE the injection so `ask --wait` pollers unblock even
    // if delivery is slow (a wake can take seconds).
    let (_, path) = project_name_path(&item.project_id);
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::FeedbackAnswered,
        serde_json::json!({
            "id": item.id,
            "projectPath": path,
        }),
    );
    // set_answer also stored a thread entry, so feedback:commented
    // fires too (see [`emit_commented`]) — an open thread panel picks
    // up the answer without a reselect.
    emit_commented(&item.id, &comment.author);
    emit_ticket_changed(&item, CHANGE_ANSWERED, Some(VIA_ANSWER));

    // F3 — the answer ALWAYS injects into the asking session (PRD §7
    // decision 1), best-effort AFTER the store + emit: a delivery
    // failure never fails the answer. An unnamed answerer is framed
    // with the owner's display name (same server-side resolution as
    // the composer, D3), so the line reads natively in-session.
    let (delivered, delivery_reason, delivered_session) = deliver_to_asker(
        &item,
        &from_owned,
        item.answer.as_deref().unwrap_or_default(),
    );

    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "id": item.id,
            "status": item.status,
            "answer": item.answer,
            "answeredAt": item.answered_at,
            "delivered": delivered,
            "deliveryReason": delivery_reason,
            "deliveredSessionId": delivered_session,
        })
        .to_string(),
    )
}

/// `POST /cli/feedback/resolve` body. `status` defaults to `resolved`;
/// `dismissed` rides the same route (one mutation, two terminal
/// states) — matching the mockup surface, where the CLI only exposes
/// `resolve` and dismiss is the human's board action. `waiting` is the
/// board's REOPEN (the per-card status dropdown). `answered` is the
/// AGENT settling a discussion (`k2 tickets resolve <id> --answered
/// "<outcome>"`): it needs a non-empty `answer` (an answered status with a
/// null answer would break the `ask --wait` contract), stores it as the
/// ticket's answer + a thread entry under `author`, and never injects.
/// A bare `answered` (no `answer`) stays a loud usage error, and apps
/// (skin passes) cannot use it: a person answers by picking an option.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ResolveBody {
    id: String,
    status: Option<String>,
    /// Required with `status: "answered"`: what was agreed.
    answer: Option<String>,
    /// Who settled it (the agent's name from the CLI). Defaults to owner.
    author: Option<String>,
}

/// Handler for `POST /cli/feedback/resolve`.
#[cfg(test)]
pub fn handle_resolve(body: &[u8]) -> CliResponse {
    handle_resolve_gated(body, None)
}

fn handle_resolve_gated(body: &[u8], skin: Option<&SkinPass>) -> CliResponse {
    let b: ResolveBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.id.is_empty() {
        return usage_error("missing 'id' (a feedback id or unique prefix)");
    }
    let status = b.status.clone().unwrap_or_else(|| "resolved".to_string());
    if status == "answered" {
        return settle_answered(&b, skin);
    }
    if !matches!(
        status.as_str(),
        "resolved" | "dismissed" | "waiting" | "planned" | "needs_discussion"
    ) {
        return usage_error(format!(
            "invalid status '{status}' — resolve accepts: resolved, dismissed, planned, needs_discussion, waiting (reopen), or answered with an answer (k2 tickets resolve <id> --answered \"<outcome>\")"
        ));
    }
    let full_id = if let Some(pass) = skin {
        match skin_load_item(pass, &b.id, crate::skin_routes::TICKETS_POST) {
            Ok(item) => item.id,
            Err(e) => return e,
        }
    } else {
        match feedback::resolve_id_prefix(&b.id) {
            Ok(f) => f,
            Err(e) => return prefix_error_response(&b.id, e),
        }
    };
    match feedback::set_status(&full_id, &status) {
        Ok(item) => {
            // FeedbackStatusChanged on the /events broadcast so every
            // window's list + waiting-count badge refresh live (the
            // answer flow has its own FeedbackAnswered). Never injects.
            let (_, path) = project_name_path(&item.project_id);
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::FeedbackStatusChanged,
                serde_json::json!({
                    "id": item.id,
                    "projectPath": path,
                    "status": item.status,
                }),
            );
            emit_ticket_changed(&item, resolve_change(&item.status), None);
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "id": item.id,
                    "status": item.status,
                })
                .to_string(),
            )
        }
        Err(e) => usage_error(e),
    }
}

/// `resolve` with `status: "answered"`: the agent settles a discussion
/// (usually out of `needs_discussion`) and records what was agreed. Stores
/// the answer + a thread entry, fires `FeedbackAnswered` +
/// `FeedbackCommented` (so `ask --wait` and open boards see it), and never
/// injects — the agent is the one writing it.
fn settle_answered(b: &ResolveBody, skin: Option<&SkinPass>) -> CliResponse {
    if skin.is_some() {
        return usage_error(
            "apps cannot set answered — a person answers by picking one of the ticket's options",
        );
    }
    let answer = b.answer.as_deref().map(str::trim).unwrap_or_default();
    if answer.is_empty() {
        return usage_error(
            "answered needs the agreed outcome — k2 tickets resolve <id> --answered \"<outcome>\"",
        );
    }
    let full_id = match feedback::resolve_id_prefix(&b.id) {
        Ok(f) => f,
        Err(e) => return prefix_error_response(&b.id, e),
    };
    let author = b
        .author
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("owner");
    let (item, comment) = match feedback::set_answer(&full_id, author, answer) {
        Ok(pair) => pair,
        Err(e) => return usage_error(e),
    };
    let (_, path) = project_name_path(&item.project_id);
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::FeedbackAnswered,
        serde_json::json!({
            "id": item.id,
            "projectPath": path,
        }),
    );
    emit_commented(&item.id, &comment.author);
    emit_ticket_changed(&item, CHANGE_ANSWERED, Some(VIA_SETTLED));
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "id": item.id,
            "status": item.status,
            "answer": item.answer,
        })
        .to_string(),
    )
}

/// `POST /cli/feedback/assign` — replace the assignee set with
/// username snapshots. Empty list clears assignees (push fans out to
/// all devices again).
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct AssignBody {
    id: String,
    /// Usernames to assign (`owner` or connect-user names). Snapshots.
    usernames: Vec<String>,
}

/// Handler for `POST /cli/feedback/assign` (owner / Connect).
#[cfg(test)]
pub fn handle_assign(body: &[u8]) -> CliResponse {
    handle_assign_gated(body, None)
}

/// `assign` for every door. An app pass (prd-app-tickets-websocket-v1)
/// needs `tickets:post` in the ticket's room; another room's id or an
/// unknown id is 403 `skin_room` (no 404 oracle). Same names, same
/// `assignee_unknown` warning as the owner.
fn handle_assign_gated(body: &[u8], skin: Option<&SkinPass>) -> CliResponse {
    let b: AssignBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.id.is_empty() {
        return usage_error("missing 'id' (a ticket id or unique prefix)");
    }
    let full_id = if let Some(pass) = skin {
        match skin_load_item(pass, &b.id, crate::skin_routes::TICKETS_POST) {
            Ok(item) => item.id,
            Err(e) => return e,
        }
    } else {
        match feedback::resolve_id_prefix(&b.id) {
            Ok(f) => f,
            Err(e) => return prefix_error_response(&b.id, e),
        }
    };
    // Same check as `create`: a name that is not a user on this server
    // still assigns (snapshots), with an `assignee_unknown` warning.
    let unknown = unknown_assignees_here(&k2_core::feedback_brief::clean_assignees(&b.usernames));
    let warnings: Vec<k2_core::feedback_brief::BriefWarning> = if unknown.is_empty() {
        Vec::new()
    } else {
        vec![k2_core::feedback_brief::assignee_unknown_warning(&unknown)]
    };
    match feedback::set_assignees(&full_id, &b.usernames) {
        Ok(item) => {
            let (_, path) = project_name_path(&item.project_id);
            // Reuse status-changed bus so open boards refresh assignees.
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::FeedbackStatusChanged,
                serde_json::json!({
                    "id": item.id,
                    "projectPath": path,
                    "status": item.status,
                    "assignees": item.assignees,
                }),
            );
            emit_ticket_changed(&item, CHANGE_ASSIGNED, None);
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "id": item.id,
                    "assignees": item.assignees,
                    "warnings": warnings,
                })
                .to_string(),
            )
        }
        Err(e) => usage_error(e),
    }
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — Feedback F1 routes
// ──────────────────────────────────────────────────────────────────────
//
// Mirrors workspace_routes' test module: `db::shared()` auto-inits the
// PROCESS-GLOBAL in-memory DB, shared across every test in the binary —
// each test inserts its own project row with a UNIQUE name/path.

#[cfg(test)]
mod tests {
    use super::*;

    // ── Event-capture sink ────────────────────────────────────────────
    //
    // The agent-hooks sink slot is process-global and last-writer-wins,
    // so the ONE capture sink for the whole k2-daemon test binary lives
    // in `crate::test_support` (shared with project_group_routes'
    // tests). Assertions filter by their own (unique) feedback id, so
    // cross-test traffic is invisible.

    use crate::test_support::{event_mark, install_capture_sink};

    /// Collect the events emitted since `mark`, for one feedback id
    /// (in emission order).
    fn events_since(mark: usize, id: &str) -> Vec<(String, serde_json::Value)> {
        crate::test_support::events_since(mark)
            .into_iter()
            .filter(|(_, p)| p["id"] == id)
            .collect()
    }

    fn unique(label: &str) -> (String, String) {
        let id = uuid::Uuid::new_v4();
        (
            format!("fb-routes-{label}-{id}"),
            format!("/tmp/fb-routes-{label}-{}-{id}", std::process::id()),
        )
    }

    fn insert_project(name: &str, path: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, path],
        )
        .expect("insert project row");
        id
    }

    /// A minimal brief that passes every check with no warnings.
    pub(super) const TEST_BRIEF: &str =
        "<h2>Problem</h2><p>Test ask.</p><section class=\"k2-need\"><p>A yes or no.</p></section>";

    /// Owner-door create. Attaches [`TEST_BRIEF`] unless `extra` sets
    /// `briefHtml` itself (H36), so the suite keeps passing when the
    /// brief policy flips to Require.
    fn create_via_route(path: &str, title: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut body = serde_json::json!({ "project": path, "title": title, "briefHtml": TEST_BRIEF });
        if let (Some(dst), Some(src)) = (body.as_object_mut(), extra.as_object()) {
            for (k, v) in src {
                dst.insert(k.clone(), v.clone());
            }
        }
        let resp = handle_create(body.to_string().as_bytes());
        assert_eq!(resp.status, "200 OK", "create failed: {}", resp.body);
        serde_json::from_str(&resp.body).expect("valid create JSON")
    }

    fn list_params(path: &str, extra: &[(&str, &str)]) -> HashMap<String, String> {
        let mut p: HashMap<String, String> =
            HashMap::from([("project".to_string(), path.to_string())]);
        for (k, v) in extra {
            p.insert(k.to_string(), v.to_string());
        }
        p
    }

    fn list_ids(path: &str, extra: &[(&str, &str)]) -> Vec<String> {
        let resp =
            dispatch("/cli/feedback/list", &list_params(path, extra)).expect("list route claimed");
        assert_eq!(resp.status, "200 OK", "list failed: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid list JSON");
        v["items"]
            .as_array()
            .expect("items array")
            .iter()
            .map(|i| i["id"].as_str().expect("id").to_string())
            .collect()
    }

    /// Full lifecycle through the route layer: create (with options +
    /// priority + session) → list shows waiting → show (with thread) →
    /// comment → answer → resolve → default list hides it, all shows it.
    #[test]
    fn feedback_route_lifecycle() {
        let (name, path) = unique("lifecycle");
        insert_project(&name, &path);

        let created = create_via_route(
            &path,
            "Deploy to prod now?",
            serde_json::json!({
                "kind": "approval",
                "priority": 1,
                "options": ["Yes", "No", "Wait for me"],
                "body": "release 0.41 staged",
                "sessionId": "sess-abc",
                "sessionKind": "sandbox",
                "agentName": "scout",
            }),
        );
        assert_eq!(created["ok"], true);
        assert_eq!(created["status"], "waiting");
        assert_eq!(created["kind"], "approval");
        assert_eq!(created["priority"], 1);
        assert_eq!(created["workspace"], name.as_str());
        assert_eq!(created["sessionId"], "sess-abc");
        assert_eq!(created["sessionKind"], "sandbox");
        assert_eq!(
            created["options"],
            serde_json::json!(["Yes", "No", "Wait for me"])
        );
        let id = created["id"].as_str().expect("id").to_string();

        // list (default) shows the waiting item.
        assert!(list_ids(&path, &[]).contains(&id), "waiting item listed");

        // show: full thread, seeded with the ask.
        let resp = dispatch(
            "/cli/feedback/show",
            &HashMap::from([("id".to_string(), id.clone())]),
        )
        .expect("show claimed");
        assert_eq!(resp.status, "200 OK", "show failed: {}", resp.body);
        let shown: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(shown["workspace"], name.as_str());
        assert_eq!(shown["projectPath"], path.as_str());
        let comments = shown["comments"].as_array().expect("comments");
        assert_eq!(comments.len(), 1, "seeded thread: {comments:?}");
        assert_eq!(comments[0]["author"], "scout");
        assert!(comments[0]["at"].is_i64(), "mockup `at` alias present");

        // AGENT comment bumps the thread ONLY (status stays waiting —
        // only a HUMAN comment on a waiting ask answers it).
        let resp = handle_comment(
            serde_json::json!({ "id": id, "body": "hold until CI passes", "author": "scout" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "comment failed: {}", resp.body);
        let c: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(c["author"], "scout");
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(
            item.status, "waiting",
            "agent comment must not change status"
        );
        assert_eq!(item.comment_count, 2);

        // answer: stores comment + answer + answered_at + status.
        let resp = handle_answer(
            serde_json::json!({ "id": id, "answer": "Yes" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "answer failed: {}", resp.body);
        let a: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(a["status"], "answered");
        assert_eq!(a["answer"], "Yes");
        assert!(a["answeredAt"].is_i64());
        // answered items still list by default.
        assert!(list_ids(&path, &[]).contains(&id));

        // resolve: default list hides it; --all / --status show it.
        let resp = handle_resolve(serde_json::json!({ "id": id }).to_string().as_bytes());
        assert_eq!(resp.status, "200 OK", "resolve failed: {}", resp.body);
        assert!(
            !list_ids(&path, &[]).contains(&id),
            "default hides resolved"
        );
        assert!(list_ids(&path, &[("all", "1")]).contains(&id));
        assert!(list_ids(&path, &[("status", "resolved")]).contains(&id));

        // reopen (per-card status dropdown): waiting rides the resolve
        // route; the answer survives so --wait semantics stay coherent.
        let resp = handle_resolve(
            serde_json::json!({ "id": id, "status": "waiting" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "reopen failed: {}", resp.body);
        let r: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(r["status"], "waiting");
        assert!(
            list_ids(&path, &[]).contains(&id),
            "reopened item lists by default"
        );

        // A manual `answered` is rejected loudly — only an actual reply
        // may answer (a null-answer answered would break --wait).
        let resp = handle_resolve(
            serde_json::json!({ "id": id, "status": "answered" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "usage");
    }

    /// A short UNIQUE prefix resolves on show/comment/answer/resolve; an
    /// AMBIGUOUS prefix answers 404 `ambiguous_id` with every candidate
    /// in the hint; an unknown prefix answers 404 `not_found`.
    #[test]
    fn feedback_prefix_resolution_and_ambiguity() {
        let (name, path) = unique("prefix");
        insert_project(&name, &path);

        // Force a shared prefix with direct inserts.
        let project_id = resolve_project_id(&path).expect("registered");
        let db = k2_core::db::shared();
        {
            let conn = db.lock();
            for suffix in ["one", "two"] {
                conn.execute(
                    "INSERT INTO feedback (id, project_id, agent_name, kind, title, priority, status, created_at, updated_at) \
                     VALUES (?1, ?2, 'scout', 'question', 't', 3, 'waiting', 0, 0)",
                    rusqlite::params![format!("fbamb-{suffix}"), project_id],
                )
                .expect("insert");
            }
        }

        // Ambiguous prefix → 404 + ambiguous_id + candidates in hint.
        let resp = dispatch(
            "/cli/feedback/show",
            &HashMap::from([("id".to_string(), "fbamb-".to_string())]),
        )
        .expect("show claimed");
        assert_eq!(resp.status, "404 Not Found", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "ambiguous_id");
        let hint = v["error"]["hint"].as_str().expect("hint");
        assert!(
            hint.contains("fbamb-one") && hint.contains("fbamb-two"),
            "candidates must be in the hint: {hint}"
        );

        // Unique prefix resolves (via a mutation route too).
        let resp = handle_resolve(
            serde_json::json!({ "id": "fbamb-o", "status": "dismissed" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["id"], "fbamb-one");
        assert_eq!(v["status"], "dismissed");

        // Unknown → not_found on every id-taking route.
        for resp in [
            dispatch(
                "/cli/feedback/show",
                &HashMap::from([("id".to_string(), "zzzz".to_string())]),
            )
            .expect("claimed"),
            handle_comment(
                serde_json::json!({ "id": "zzzz", "body": "x" })
                    .to_string()
                    .as_bytes(),
            ),
            handle_answer(
                serde_json::json!({ "id": "zzzz", "answer": "x" })
                    .to_string()
                    .as_bytes(),
            ),
            handle_resolve(serde_json::json!({ "id": "zzzz" }).to_string().as_bytes()),
        ] {
            assert_eq!(resp.status, "404 Not Found", "body={}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "not_found", "body={}", resp.body);
        }
    }

    /// Create may stamp assignees so mobile push + the board people
    /// filter see them immediately (agent `ask --assign`).
    #[test]
    fn feedback_create_with_assignees() {
        let (name, path) = unique("create-assign");
        insert_project(&name, &path);
        let resp = handle_create(
            serde_json::json!({
                "project": path,
                "title": "Need a human",
                "assignees": ["owner", "julie", "owner", "  "],
                "briefHtml": TEST_BRIEF,
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "create failed: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        let assignees = v["assignees"].as_array().expect("assignees array");
        let names: Vec<&str> = assignees.iter().filter_map(|x| x.as_str()).collect();
        assert_eq!(
            names,
            vec!["julie", "owner"],
            "deduped + sorted snapshots: {names:?}"
        );
        let id = v["id"].as_str().expect("id");
        let item = k2_core::feedback::get_item(id).expect("item");
        assert_eq!(
            item.assignees,
            vec!["julie".to_string(), "owner".to_string()]
        );
    }

    /// Validation misses answer 400 with the stable `usage` code and
    /// the mockup's exact hints.
    #[test]
    fn feedback_create_validation() {
        let (name, path) = unique("validate");
        insert_project(&name, &path);

        // No title.
        let resp = handle_create(
            serde_json::json!({ "project": path })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "usage");
        assert_eq!(v["error"]["hint"], "ask requires a <title>");

        // Bad priority.
        let resp = handle_create(
            serde_json::json!({ "project": path, "title": "x", "priority": 9 })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request");
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["hint"], "--priority must be 1-5");

        // Bad kind.
        let resp = handle_create(
            serde_json::json!({ "project": path, "title": "x", "kind": "bug" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);

        // Unknown workspace → the shared not_found shape.
        let resp = handle_create(
            serde_json::json!({ "project": "no-such-ws-anywhere", "title": "x" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "404 Not Found", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "not_found");

        // Null session must not fail the ask; agentName defaults.
        let created = create_via_route(&path, "sessionless ask", serde_json::json!({}));
        assert_eq!(created["ok"], true);
        assert!(created["sessionId"].is_null());
        assert!(
            created["agentName"].as_str().is_some_and(|s| !s.is_empty()),
            "agentName must default: {created}"
        );

        // D4: known kinds persist; unknown/empty fail-open to NULL.
        let mut extra_ids: Vec<String> = vec![created["id"].as_str().expect("id").to_string()];
        for kind in ["canonical", "sandbox", "sidecar", "api"] {
            let title = format!("kind {kind}");
            let created = create_via_route(
                &path,
                &title,
                serde_json::json!({
                    "sessionId": format!("sess-{kind}"),
                    "sessionKind": kind,
                }),
            );
            assert_eq!(
                created["sessionKind"], kind,
                "known kind {kind} must persist: {created}"
            );
            assert_eq!(created["sessionId"], format!("sess-{kind}"));
            extra_ids.push(created["id"].as_str().expect("id").to_string());
        }
        let created = create_via_route(
            &path,
            "unknown kind",
            serde_json::json!({
                "sessionId": "sess-unknown",
                "sessionKind": "host",
            }),
        );
        assert!(
            created["sessionKind"].is_null(),
            "unknown kind must fail-open to NULL: {created}"
        );
        assert_eq!(created["sessionId"], "sess-unknown");
        extra_ids.push(created["id"].as_str().expect("id").to_string());
        for id in extra_ids {
            let resp = handle_resolve(
                serde_json::json!({ "id": id, "status": "dismissed" })
                    .to_string()
                    .as_bytes(),
            );
            assert_eq!(resp.status, "200 OK", "dismiss extra {id}: {}", resp.body);
        }
    }

    /// GET on the POST-only mutations answers an explicit 405 through
    /// the read dispatch chain (feedback_post_only_route_guards), and
    /// the POST dispatcher 404s unknown paths.
    #[test]
    fn feedback_mutations_405_on_get_and_post_404_unknown() {
        let params = HashMap::new();
        for route in [
            "/cli/feedback/create",
            "/cli/feedback/comment",
            "/cli/feedback/answer",
            "/cli/feedback/resolve",
        ] {
            let resp = dispatch(route, &params).expect("route claimed by GET chain");
            assert_eq!(resp.status, "405 Method Not Allowed", "route={route}");
            assert!(resp.body.contains("POST required"), "body={}", resp.body);
        }
        let resp = dispatch_post("/cli/feedback/unknown", b"{}");
        assert_eq!(resp.status, "404 Not Found");
    }

    /// F3 — v1 injection decision matrix (D5): every stamp, including
    /// sandbox + a live-looking id, is WorkspaceAgent. Return-to-asker
    /// cell is out of this slice.
    #[test]
    fn feedback_injection_target_matrix() {
        assert_eq!(
            injection_target(Some("cell-1"), Some("sandbox")),
            InjectionTarget::WorkspaceAgent
        );
        assert_eq!(
            injection_target(Some("conv-1"), Some("canonical")),
            InjectionTarget::WorkspaceAgent
        );
        assert_eq!(
            injection_target(Some("side-1"), Some("sidecar")),
            InjectionTarget::WorkspaceAgent
        );
        assert_eq!(
            injection_target(Some("api-1"), Some("api")),
            InjectionTarget::WorkspaceAgent
        );
        assert_eq!(
            injection_target(None, None),
            InjectionTarget::WorkspaceAgent
        );
        assert_eq!(
            injection_target(Some("conv-1"), None),
            InjectionTarget::WorkspaceAgent
        );
        assert_eq!(
            injection_target(None, Some("sandbox")),
            InjectionTarget::WorkspaceAgent
        );

        // Injected body: PRD §4.3 `[feedback:<short-id>] <body>` —
        // shared by answers AND human comments; the short id is a
        // resolvable 8-char prefix.
        assert_eq!(
            feedback_payload("7b3f1a2c-9d10-4e6f-8a2b-000000000000", "Go"),
            "[feedback:7b3f1a2c] Go"
        );
        assert_eq!(feedback_payload("ab", "x"), "[feedback:ab] x");
    }

    /// F3 — injection is best-effort: a delivery failure NEVER fails
    /// the answer. Covers all three target shapes against a project
    /// with no agent and no live sessions, asserting the answer stores
    /// + status flips + the response reports the delivery outcome
    /// alongside the pre-F3 fields.
    #[test]
    fn feedback_answer_injection_best_effort_still_stores() {
        let (name, path) = unique("inject");
        insert_project(&name, &path);

        let answer = |id: &str, text: &str| -> serde_json::Value {
            let resp = handle_answer(
                serde_json::json!({ "id": id, "answer": text })
                    .to_string()
                    .as_bytes(),
            );
            assert_eq!(resp.status, "200 OK", "answer failed: {}", resp.body);
            serde_json::from_str(&resp.body).expect("valid answer JSON")
        };

        // (a) Sessionless ask → workspace fallback. The test project
        // has no agent (agent_enabled=0, no saved session), so the
        // wake path classifies no_agent_mode — and the answer stores.
        let created = create_via_route(&path, "null-session ask", serde_json::json!({}));
        let id = created["id"].as_str().expect("id").to_string();
        let a = answer(&id, "navy");
        assert_eq!(a["ok"], true);
        assert_eq!(a["status"], "answered");
        assert_eq!(a["answer"], "navy");
        assert!(a["answeredAt"].is_i64());
        assert_eq!(a["delivered"], false);
        assert_eq!(a["deliveryReason"], "no_agent_mode");
        assert!(a["deliveredSessionId"].is_null());
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(
            item.status, "answered",
            "failed delivery must not lose the answer"
        );
        assert_eq!(item.answer.as_deref(), Some("navy"));

        // (b) Random-UUID sandbox stamp on a project with no agent →
        // WorkspaceAgent `deliver_live` (D5), which classifies
        // `no_agent_mode`. Do NOT invert this into a canonical-id
        // fixture — that is the poison-stamp test below. Not
        // `session_gone` (that was the live-PTY-only sandbox arm).
        let created = create_via_route(
            &path,
            "sandbox ask",
            serde_json::json!({
                "sessionId": uuid::Uuid::new_v4().to_string(),
                "sessionKind": "sandbox",
            }),
        );
        let id = created["id"].as_str().expect("id").to_string();
        let a = answer(&id, "Go");
        assert_eq!(a["delivered"], false);
        assert_eq!(a["deliveryReason"], "no_agent_mode");
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "answered");
        assert_eq!(item.answer.as_deref(), Some("Go"));

        // (c) Canonical row → routed by WORKSPACE identity (not the
        // conversation id), so the dead-project agent classifies
        // no_agent_mode, not session_gone.
        let created = create_via_route(
            &path,
            "canonical ask",
            serde_json::json!({
                "sessionId": uuid::Uuid::new_v4().to_string(),
                "sessionKind": "canonical",
            }),
        );
        let id = created["id"].as_str().expect("id").to_string();
        let a = answer(&id, "Hold");
        assert_eq!(a["delivered"], false);
        assert_eq!(a["deliveryReason"], "no_agent_mode");
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "answered");
    }

    /// Poison stamp: `session_kind=sandbox` whose id IS this project's
    /// `workspace_sessions.session_id` (AFSROW-style). v1 injects via
    /// WorkspaceAgent, so the answer is NOT `session_gone` (the old
    /// live-PTY sandbox arm looked up the conversation UUID as a PTY
    /// id and missed).
    #[test]
    fn feedback_poison_sandbox_stamp_on_canonical_id_is_not_session_gone() {
        let (name, path) = unique("poison-stamp");
        std::fs::create_dir_all(&path).expect("workspace cwd");
        let project_id = insert_project(&name, &path);
        let canonical_id = uuid::Uuid::new_v4().to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
                 VALUES (?1, ?2, ?3, 'claude', 'user', 'sleeping', unixepoch())",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), project_id, canonical_id],
            )
            .expect("insert workspace_sessions");
        }

        let created = create_via_route(
            &path,
            "poison sandbox on canonical id",
            serde_json::json!({
                "sessionId": canonical_id,
                "sessionKind": "sandbox",
            }),
        );
        assert_eq!(created["sessionKind"], "sandbox");
        assert_eq!(created["sessionId"], canonical_id.as_str());
        let id = created["id"].as_str().expect("id").to_string();

        let shown = dispatch(
            "/cli/feedback/show",
            &HashMap::from([("id".to_string(), id.clone())]),
        )
        .expect("show claimed");
        assert_eq!(shown.status, "200 OK", "show failed: {}", shown.body);
        let v: serde_json::Value = serde_json::from_str(&shown.body).expect("valid show JSON");
        assert_eq!(
            v["canonicalSessionId"].as_str(),
            Some(canonical_id.as_str()),
            "show must surface workspace_sessions.session_id for D6: {v}"
        );
        assert_eq!(
            v["sessionId"].as_str(),
            Some(canonical_id.as_str()),
            "poison row still stamps the pinned conversation id: {v}"
        );

        // Headless seam: don't spawn a real agent. Saved session makes
        // this wakeable; we only need "not session_gone". v2 spawn needs
        // a tokio reactor even for `/bin/cat`.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let _enter = rt.enter();
        let prior = std::env::var("K2SO_WAKE_HEADLESS_TEST_COMMAND").ok();
        std::env::set_var("K2SO_WAKE_HEADLESS_TEST_COMMAND", "/bin/cat");
        let resp = handle_answer(
            serde_json::json!({ "id": id, "answer": "wake it" })
                .to_string()
                .as_bytes(),
        );
        if let Some(s) = crate::v2_session_map::unregister(&project_id) {
            s.kill();
        }
        match prior {
            Some(v) => std::env::set_var("K2SO_WAKE_HEADLESS_TEST_COMMAND", v),
            None => std::env::remove_var("K2SO_WAKE_HEADLESS_TEST_COMMAND"),
        }
        let _ = std::fs::remove_dir_all(&path);

        assert_eq!(resp.status, "200 OK", "answer failed: {}", resp.body);
        let a: serde_json::Value = serde_json::from_str(&resp.body).expect("valid answer JSON");
        assert_eq!(a["status"], "answered");
        assert_ne!(
            a["deliveryReason"].as_str(),
            Some("session_gone"),
            "poison stamp must not take the live-PTY sandbox arm: {a}"
        );
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "answered");
        assert_eq!(item.answer.as_deref(), Some("wake it"));
    }

    /// The comment injection/answer matrix ("it's just a comment
    /// thread; human comments land in the terminal session"):
    ///
    /// | author | comment     | item state | injects | → status          |
    /// |--------|-------------|------------|---------|-------------------|
    /// | human  | option pick | waiting    | yes     | answered (+answer)|
    /// | human  | free text   | waiting    | yes     | needs_discussion  |
    /// | human  | free text   | answered   | yes     | needs_discussion (answer kept) |
    /// | human  | free text   | waiting fyi| yes     | needs_discussion  |
    /// | agent  | anything    | anything   | no      | unchanged         |
    ///
    /// Delivery is best-effort against a dead project (no agent →
    /// no_agent_mode via WorkspaceAgent), so `delivered:false` here —
    /// the point is the delivery ATTEMPT is reported and the store
    /// never fails.
    #[test]
    fn feedback_comment_matrix_option_pick_answers_free_text_discusses() {
        let (name, path) = unique("comment-matrix");
        insert_project(&name, &path);
        let sandbox_session = || {
            serde_json::json!({
                "sessionId": uuid::Uuid::new_v4().to_string(),
                "sessionKind": "sandbox",
            })
        };
        let comment = |body: serde_json::Value| -> serde_json::Value {
            let resp = handle_comment(body.to_string().as_bytes());
            assert_eq!(resp.status, "200 OK", "comment failed: {}", resp.body);
            serde_json::from_str(&resp.body).expect("valid comment JSON")
        };

        // OPTION PICK on a WAITING question → answers + unblocks --wait
        // (status answered, answer denormalized, delivery attempted with
        // the shared [feedback:] framing).
        let created = create_via_route(&path, "Which color?", sandbox_session());
        let id = created["id"].as_str().expect("id").to_string();
        let c = comment(serde_json::json!({ "id": id, "body": "navy", "optionPick": true }));
        assert_eq!(c["author"], "owner", "author defaults to owner (human)");
        assert_eq!(c["answered"], true);
        assert_eq!(c["status"], "answered");
        assert_eq!(
            c["delivered"], false,
            "dead project → attempted, not delivered"
        );
        assert_eq!(c["deliveryReason"], "no_agent_mode");
        assert!(c["commentId"].is_string());
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "answered", "--wait unblocks on this");
        assert_eq!(item.answer.as_deref(), Some("navy"));
        assert_eq!(item.comment_count, 2);

        // FREE TEXT on the answered item → injects, opens a discussion,
        // and never overwrites the accepted answer.
        let c = comment(serde_json::json!({ "id": id, "body": "also check contrast" }));
        assert_eq!(c["answered"], false);
        assert_eq!(c["status"], "needs_discussion");
        assert_eq!(c["deliveryReason"], "no_agent_mode", "still injects");
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "needs_discussion");
        assert_eq!(item.answer.as_deref(), Some("navy"), "answer unchanged");
        assert_eq!(item.comment_count, 3);

        // FREE TEXT on a WAITING approval → needs_discussion, NOT
        // answered (it used to auto-answer; that is the bug Rosson saw:
        // needs_discussion never happened).
        let created =
            create_via_route(&path, "Ship it?", serde_json::json!({ "kind": "approval" }));
        let id = created["id"].as_str().expect("id").to_string();
        let c = comment(serde_json::json!({ "id": id, "body": "why not Friday?" }));
        assert_eq!(c["answered"], false);
        assert_eq!(c["status"], "needs_discussion");
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "needs_discussion");
        assert!(item.answer.is_none(), "free text never records an answer");

        // `optionPick: false` is the same as absent: free text.
        let created = create_via_route(&path, "Port?", serde_json::json!({}));
        let id = created["id"].as_str().expect("id").to_string();
        let c = comment(serde_json::json!({ "id": id, "body": "8080?", "optionPick": false }));
        assert_eq!(c["status"], "needs_discussion");

        // FREE TEXT on a WAITING fyi → needs_discussion too, and still
        // injects.
        let created = create_via_route(
            &path,
            "Heads up",
            serde_json::json!({ "kind": "fyi", "sessionId": uuid::Uuid::new_v4().to_string(), "sessionKind": "sandbox" }),
        );
        let id = created["id"].as_str().expect("id").to_string();
        let c = comment(serde_json::json!({ "id": id, "body": "noted, but why?" }));
        assert_eq!(c["answered"], false);
        assert_eq!(c["status"], "needs_discussion");
        assert_eq!(
            c["deliveryReason"], "no_agent_mode",
            "fyi comment still injects"
        );
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "needs_discussion");
        assert!(item.answer.is_none());

        // AGENT comment (author = its name, as the CLI always sends)
        // → store only: no injection, no auto-answer, no delivery
        // fields in the response.
        let created = create_via_route(&path, "agent self-note", sandbox_session());
        let id = created["id"].as_str().expect("id").to_string();
        let c =
            comment(serde_json::json!({ "id": id, "body": "still thinking", "author": "scout" }));
        assert_eq!(c["author"], "scout");
        assert!(
            c.get("delivered").is_none()
                && c.get("deliveryReason").is_none()
                && c.get("answered").is_none(),
            "agent comments must not report delivery: {c}"
        );
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "waiting", "agent comment must not answer");
        assert_eq!(item.comment_count, 2);

        // An agent cannot answer its own ticket with `optionPick`.
        let c = comment(serde_json::json!({
            "id": id, "body": "Go", "author": "scout", "optionPick": true
        }));
        assert!(c.get("answered").is_none(), "agent pick is a plain note: {c}");
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "waiting", "agent optionPick is ignored");
        assert!(item.answer.is_none());
    }

    /// The agent settles a discussion: free text put the ticket in
    /// needs_discussion; the agent replies (status stays), then marks it
    /// answered with the agreed outcome (`k2 tickets resolve <id>
    /// --answered "<outcome>"` → resolve route `status: answered` +
    /// `answer`) or resolved. Settling never injects, fires answered +
    /// commented (so --wait and open boards see it), and a bare
    /// `answered` without an outcome stays a loud usage error.
    #[test]
    fn feedback_agent_settles_discussion_to_answered_or_resolved() {
        install_capture_sink();
        let (name, path) = unique("settle");
        insert_project(&name, &path);
        let created = create_via_route(&path, "Which DB?", serde_json::json!({}));
        let id = created["id"].as_str().expect("id").to_string();

        // Human free text → needs_discussion (+ status-changed event).
        let mark = event_mark();
        let resp = handle_comment(
            serde_json::json!({ "id": id, "body": "what about SQLite?" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "comment failed: {}", resp.body);
        let events = events_since(mark, &id);
        assert_eq!(
            events.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["feedback:status-changed", "feedback:commented"],
            "free text discusses: {events:?}"
        );
        assert_eq!(events[0].1["status"], "needs_discussion");

        // Agent reply keeps it in discussion.
        let resp = handle_comment(
            serde_json::json!({ "id": id, "body": "SQLite fits; Postgres later", "author": "scout" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "agent reply failed: {}", resp.body);
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "needs_discussion", "agent reply does not settle");

        // Bare answered (no outcome) → usage error, status untouched.
        let resp = handle_resolve(
            serde_json::json!({ "id": id, "status": "answered", "answer": "  " })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["error"]["code"], "usage");
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "needs_discussion");

        // Agent settles → answered with the outcome as the answer.
        let mark = event_mark();
        let resp = handle_resolve(
            serde_json::json!({
                "id": id,
                "status": "answered",
                "answer": "SQLite now, Postgres later",
                "author": "scout",
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "settle failed: {}", resp.body);
        let r: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(r["status"], "answered");
        assert_eq!(r["answer"], "SQLite now, Postgres later");
        assert!(
            r.get("delivered").is_none() && r.get("deliveryReason").is_none(),
            "settling never injects: {r}"
        );
        let events = events_since(mark, &id);
        assert_eq!(
            events.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["feedback:answered", "feedback:commented"],
            "settle answers + stores a thread entry: {events:?}"
        );
        assert_eq!(events[1].1["author"], "scout");
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "answered");
        assert_eq!(item.answer.as_deref(), Some("SQLite now, Postgres later"));

        // A second discussion settles to resolved instead.
        let resp = handle_comment(
            serde_json::json!({ "id": id, "body": "one more thing" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "comment failed: {}", resp.body);
        assert_eq!(
            k2_core::feedback::get_item(&id).expect("item").status,
            "needs_discussion"
        );
        let resp = handle_resolve(serde_json::json!({ "id": id }).to_string().as_bytes());
        assert_eq!(resp.status, "200 OK", "resolve failed: {}", resp.body);
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "resolved");
    }

    /// Resolve / dismiss / reopen never touch the delivery path.
    /// Manual statuses the route accepts: resolved, dismissed, planned,
    /// needs_discussion, waiting (reopen). A manual `answered` is a
    /// loud usage error (answers go through comment / set_answer).
    #[test]
    fn feedback_resolve_reopen_and_never_injects() {
        let (name, path) = unique("resolve-reopen");
        insert_project(&name, &path);
        let created = create_via_route(
            &path,
            "resolve target",
            serde_json::json!({
                "sessionId": uuid::Uuid::new_v4().to_string(),
                "sessionKind": "sandbox",
            }),
        );
        let id = created["id"].as_str().expect("id").to_string();

        // dismiss → reopen → resolve, none reporting delivery.
        for status in ["dismissed", "waiting", "resolved"] {
            let resp = handle_resolve(
                serde_json::json!({ "id": id, "status": status })
                    .to_string()
                    .as_bytes(),
            );
            assert_eq!(resp.status, "200 OK", "{status} failed: {}", resp.body);
            let r: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(r["status"], status);
            assert!(
                r.get("delivered").is_none() && r.get("deliveryReason").is_none(),
                "resolve must not report delivery: {r}"
            );
        }
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "resolved");
        assert!(
            item.answer.is_none(),
            "reopen path never fabricates an answer"
        );

        // Manual answered / garbage statuses are loud usage errors.
        for bad in ["answered", "closed", ""] {
            let resp = handle_resolve(
                serde_json::json!({ "id": id, "status": bad })
                    .to_string()
                    .as_bytes(),
            );
            assert_eq!(
                resp.status, "400 Bad Request",
                "status '{bad}' body={}",
                resp.body
            );
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
            assert_eq!(v["error"]["code"], "usage", "status '{bad}'");
        }
    }

    #[test]
    fn feedback_comment_session_author_is_connect_user() {
        let (name, path) = unique("comment-session-author");
        insert_project(&name, &path);
        let created = create_via_route(
            &path,
            "Which color?",
            serde_json::json!({
                "sessionId": uuid::Uuid::new_v4().to_string(),
                "sessionKind": "sandbox",
            }),
        );
        let id = created["id"].as_str().expect("id").to_string();
        let resp = handle_comment_as(
            serde_json::json!({ "id": id, "body": "navy", "optionPick": true })
                .to_string()
                .as_bytes(),
            "alice",
        );
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let c: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(c["author"], "alice");
        assert_eq!(c["answered"], true);
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.answer.as_deref(), Some("navy"));
    }

    /// `feedback:commented` fires for every STORED comment on the
    /// comment route — agent- AND human-authored — with the frozen
    /// `{id, projectPath, author}` payload, and NEVER re-fires
    /// `feedback:created` (the only event the renderer's desktop
    /// notification listens to — that seam is the event NAME).
    #[test]
    fn feedback_comment_routes_emit_commented_event() {
        install_capture_sink();
        let (name, path) = unique("commented-event");
        insert_project(&name, &path);
        let created = create_via_route(&path, "Which port?", serde_json::json!({}));
        let id = created["id"].as_str().expect("id").to_string();

        // Agent comment → exactly ONE new event for this id:
        // feedback:commented, author = the agent's name.
        let mark = event_mark();
        let resp = handle_comment(
            serde_json::json!({ "id": id, "body": "leaning 8080", "author": "scout" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "comment failed: {}", resp.body);
        let events = events_since(mark, &id);
        assert_eq!(
            events.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["feedback:commented"],
            "agent comment must emit commented and nothing else: {events:?}"
        );
        assert_eq!(events[0].1["id"], id.as_str());
        assert_eq!(events[0].1["projectPath"], path.as_str());
        assert_eq!(events[0].1["author"], "scout");

        // Human OPTION PICK on the waiting question → answers, so
        // feedback:answered AND feedback:commented — still never
        // feedback:created (comments must not notify).
        let mark = event_mark();
        let resp = handle_comment(
            serde_json::json!({ "id": id, "body": "8080", "optionPick": true })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "comment failed: {}", resp.body);
        let events = events_since(mark, &id);
        assert_eq!(
            events.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["feedback:answered", "feedback:commented"],
            "human first comment answers + comments: {events:?}"
        );
        assert_eq!(events[1].1["projectPath"], path.as_str());
        assert_eq!(events[1].1["author"], "owner");

        // Human free-text FOLLOW-UP on the answered item → discussion:
        // status-changed + commented, never re-answered or notified.
        let mark = event_mark();
        let resp = handle_comment(
            serde_json::json!({ "id": id, "body": "and 8081 for metrics" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "comment failed: {}", resp.body);
        let events = events_since(mark, &id);
        assert_eq!(
            events.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["feedback:status-changed", "feedback:commented"],
            "follow-up must not re-answer or notify: {events:?}"
        );
        assert_eq!(events[1].1["author"], "owner");

        // A second free-text message while already in discussion →
        // commented only (no status change to announce).
        let mark = event_mark();
        let resp = handle_comment(
            serde_json::json!({ "id": id, "body": "or 9090" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "comment failed: {}", resp.body);
        let events = events_since(mark, &id);
        assert_eq!(
            events.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["feedback:commented"],
            "no status change → commented only: {events:?}"
        );
    }

    /// The answer route also creates a thread entry, so it fires
    /// `feedback:commented` right after its `feedback:answered` — and
    /// never `feedback:created`.
    #[test]
    fn feedback_answer_route_emits_commented_event() {
        install_capture_sink();
        let (name, path) = unique("answer-commented");
        insert_project(&name, &path);
        let created = create_via_route(&path, "Ship the fix?", serde_json::json!({}));
        let id = created["id"].as_str().expect("id").to_string();

        let mark = event_mark();
        let resp = handle_answer(
            serde_json::json!({ "id": id, "answer": "Ship it" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "answer failed: {}", resp.body);
        let events = events_since(mark, &id);
        assert_eq!(
            events.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["feedback:answered", "feedback:commented"],
            "answer stores a thread entry too: {events:?}"
        );
        assert_eq!(events[1].1["id"], id.as_str());
        assert_eq!(events[1].1["projectPath"], path.as_str());
        assert_eq!(events[1].1["author"], "owner");
    }

    /// Status filter validation + fyi kind flows through list.
    #[test]
    fn feedback_list_filters_and_fyi() {
        let (name, path) = unique("filters");
        insert_project(&name, &path);

        let fyi = create_via_route(
            &path,
            "Heads up: rate limit is close",
            serde_json::json!({ "kind": "fyi", "priority": 4 }),
        );
        assert_eq!(fyi["kind"], "fyi");
        assert_eq!(fyi["status"], "waiting");
        let id = fyi["id"].as_str().expect("id").to_string();
        assert!(list_ids(&path, &[("status", "waiting")]).contains(&id));

        // Invalid status filter fails loudly.
        let resp = dispatch(
            "/cli/feedback/list",
            &list_params(&path, &[("status", "bogus")]),
        )
        .expect("claimed");
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);

        // The count is host-wide and other tests in this process create
        // and resolve tickets in parallel, so a bare before/after read
        // could move by more or less than our one ticket. Hold the shared
        // DB's reentrant lock across read → create → read: the route
        // calls re-enter it on this thread, other tests' writes wait, and
        // the delta is exactly this test's own fixture.
        let (before_n, another_id, after_n) = {
            let db = k2_core::db::shared();
            let _hold = db.lock();
            let read_count = || {
                let resp = dispatch("/cli/feedback/waiting-count", &HashMap::new())
                    .expect("waiting-count claimed");
                assert_eq!(resp.status, "200 OK", "body={}", resp.body);
                serde_json::from_str::<serde_json::Value>(&resp.body).expect("json")["count"]
                    .as_i64()
                    .expect("count")
            };
            let before_n = read_count();
            let another = create_via_route(&path, "another waiting", serde_json::json!({}));
            let another_id = another["id"].as_str().expect("id").to_string();
            (before_n, another_id, read_count())
        };
        assert_eq!(
            after_n,
            before_n + 1,
            "one new waiting ticket adds exactly one to the waiting count"
        );
        let waiting = list_ids(&path, &[("status", "waiting")]);
        assert!(
            waiting.contains(&id) && waiting.contains(&another_id),
            "both of this test's tickets are waiting: {waiting:?}"
        );

        // Missing project param → 400; unregistered project → 404.
        let resp = dispatch("/cli/feedback/list", &HashMap::new()).expect("claimed");
        assert_eq!(resp.status, "400 Bad Request");
        let resp = dispatch(
            "/cli/feedback/list",
            &list_params("/tmp/never-registered-anywhere", &[]),
        )
        .expect("claimed");
        assert_eq!(resp.status, "404 Not Found", "body={}", resp.body);
    }

    fn list_all_items(extra: &[(&str, &str)]) -> Vec<serde_json::Value> {
        let params: HashMap<String, String> =
            extra.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let resp = dispatch("/cli/feedback/list-all", &params).expect("list-all claimed");
        assert_eq!(resp.status, "200 OK", "list-all failed: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid list-all JSON");
        assert_eq!(v["ok"], serde_json::json!(true), "{}", resp.body);
        v["items"].as_array().expect("items array").clone()
    }

    /// prd-tickets-badge-orphans T3 (shared DB → membership by id, never a
    /// row count): a ticket whose workspace was removed shows up in
    /// `list-all` unlinked, Dismiss by id works on it, and then the
    /// default `list-all` no longer returns it.
    #[test]
    fn feedback_list_all_returns_unlinked_tickets_and_dismiss_clears_them() {
        let (name, path) = unique("list-all");
        let project_id = insert_project(&name, &path);
        let created = create_via_route(&path, "Deploy?", serde_json::json!({}));
        let id = created["id"].as_str().expect("id").to_string();

        // While the workspace is registered, the row is linked.
        let row = list_all_items(&[])
            .into_iter()
            .find(|r| r["id"] == id.as_str())
            .expect("list-all contains the linked ticket");
        assert_eq!(row["linked"], serde_json::json!(true), "{row}");
        assert_eq!(row["projectName"], serde_json::json!(name), "{row}");
        assert_eq!(row["projectPath"], serde_json::json!(path), "{row}");

        // Remove the workspace's project row: the ticket is now unlinked.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "DELETE FROM projects WHERE id = ?1",
                rusqlite::params![project_id],
            )
            .expect("delete project row");
        }
        let row = list_all_items(&[])
            .into_iter()
            .find(|r| r["id"] == id.as_str())
            .expect("list-all contains the unlinked ticket");
        assert_eq!(row["linked"], serde_json::json!(false), "{row}");
        assert_eq!(row["projectName"], serde_json::Value::Null, "{row}");
        assert_eq!(row["projectPath"], serde_json::Value::Null, "{row}");
        assert_eq!(row["title"], serde_json::json!("Deploy?"), "{row}");
        assert_eq!(row["agentName"], created["agentName"], "{row}");
        assert_eq!(row["createdAt"], created["createdAt"], "{row}");
        assert_eq!(row["status"], serde_json::json!("waiting"), "{row}");

        // Dismiss by id (the Unlinked card's action).
        let body = serde_json::json!({ "id": id, "status": "dismissed" }).to_string();
        let resp = dispatch_post("/cli/feedback/resolve", body.as_bytes());
        assert_eq!(resp.status, "200 OK", "dismiss failed: {}", resp.body);

        assert!(
            !list_all_items(&[]).iter().any(|r| r["id"] == id.as_str()),
            "default list-all (open) must no longer contain the dismissed ticket"
        );
        let closed = list_all_items(&[("all", "1")])
            .into_iter()
            .find(|r| r["id"] == id.as_str())
            .expect("list-all all=1 still contains the dismissed ticket");
        assert_eq!(closed["status"], serde_json::json!("dismissed"), "{closed}");

        // Status filter rules are the same as `list`.
        let resp = dispatch(
            "/cli/feedback/list-all",
            &HashMap::from([("status".to_string(), "bogus".to_string())]),
        )
        .expect("claimed");
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);
    }

    /// Appa A2: `list?all=1` without `project` is a 400 that names
    /// `list-all`, never a quiet host-wide or empty answer.
    #[test]
    fn feedback_list_all_flag_without_project_is_400_pointing_to_list_all() {
        let resp = dispatch(
            "/cli/feedback/list",
            &HashMap::from([("all".to_string(), "1".to_string())]),
        )
        .expect("claimed");
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["ok"], serde_json::json!(false), "{}", resp.body);
        assert_eq!(v["error"]["code"], serde_json::json!("usage"), "{}", resp.body);
        assert_eq!(
            v["error"]["hint"],
            serde_json::json!("all=1 requires project=; use /cli/feedback/list-all"),
            "{}",
            resp.body
        );
        // Without all=1 the missing-project error is unchanged.
        let resp = dispatch("/cli/feedback/list", &HashMap::new()).expect("claimed");
        assert_eq!(resp.status, "400 Bad Request");
        assert!(
            resp.body.contains("Missing project (or project_path) parameter"),
            "{}",
            resp.body
        );
    }

    /// Companion C4: CREATE pushes (content-free); comments also push
    /// thread activity; resolve does not.
    #[test]
    fn feedback_create_pushes_mobile_and_other_mutations_do_not() {
        use k2_core::push::PushEvent;
        install_capture_sink();
        let (name, path) = unique("push-trigger");
        insert_project(&name, &path);

        let mark = crate::push_routes::test_push_capture::mark();
        let created = create_via_route(
            &path,
            "Which port should the tunnel bind?",
            serde_json::json!({ "agentName": "scout" }),
        );
        let id = created["id"].as_str().expect("id").to_string();
        let pushes: Vec<_> = crate::push_routes::test_push_capture::since(mark)
            .into_iter()
            .filter(|e| {
                matches!(e, PushEvent::FeedbackCreated { feedback_id, .. } if feedback_id == &id)
            })
            .collect();
        assert_eq!(pushes.len(), 1, "create pushes exactly once: {pushes:?}");
        let e = &pushes[0];
        assert_eq!(e.title(), "K2");
        assert_eq!(e.body(), "scout needs you on a ticket");
        assert!(
            !e.body().contains("tunnel") && !e.body().contains("port"),
            "the ask's text must never ride the push (§4.5): {}",
            e.body()
        );
        assert_eq!(
            e.data(),
            serde_json::json!({ "kind": "ticket", "feedbackId": id })
        );

        // Comments push FeedbackCommented; resolve must not push create.
        let mark = crate::push_routes::test_push_capture::mark();
        let resp = handle_comment(
            serde_json::json!({ "id": id, "body": "leaning 8080", "author": "scout" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "comment failed: {}", resp.body);
        let resp = handle_comment(
            serde_json::json!({ "id": id, "body": "8080" })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "comment failed: {}", resp.body);
        let resp = handle_resolve(serde_json::json!({ "id": id }).to_string().as_bytes());
        assert_eq!(resp.status, "200 OK", "resolve failed: {}", resp.body);
        let since = crate::push_routes::test_push_capture::since(mark);
        let creates: Vec<_> = since
            .iter()
            .filter(|e| {
                matches!(e, PushEvent::FeedbackCreated { feedback_id, .. } if feedback_id == &id)
            })
            .collect();
        assert!(
            creates.is_empty(),
            "resolve must not re-push FeedbackCreated: {creates:?}"
        );
        let comments: Vec<_> = since
            .iter()
            .filter(|e| {
                matches!(e, PushEvent::FeedbackCommented { feedback_id, .. } if feedback_id == &id)
            })
            .collect();
        assert_eq!(
            comments.len(),
            2,
            "each comment should push once: {comments:?}"
        );
    }

    fn insert_project_handle(name: &str, path: &str, handle: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, name, path, handle],
        )
        .expect("insert project row");
        id
    }

    fn skin_session(username: &str, policy: k2_core::skin::RoomPolicy) -> k2_core::skin::SkinPass {
        let rooms: Vec<String> = policy.keys().cloned().collect();
        let mut caps = Vec::new();
        for c in [
            k2_core::skin::CAP_THREAD_READ,
            k2_core::skin::CAP_THREAD_POST,
            k2_core::skin::CAP_FILES_READ,
            k2_core::skin::CAP_FILES_WRITE,
            k2_core::skin::CAP_TICKETS_READ,
            k2_core::skin::CAP_TICKETS_POST,
        ] {
            if policy.values().any(|room| room.iter().any(|x| x == c))
                && !caps.iter().any(|x| x == c)
            {
                caps.push(c.to_string());
            }
        }
        k2_core::skin::SkinPass {
            id: uuid::Uuid::new_v4().to_string(),
            principal_id: Some(uuid::Uuid::new_v4().to_string()),
            username: username.to_string(),
            caps,
            rooms,
            session: true,
            room_policy: policy,
        }
    }

    #[test]
    fn skin_tickets_list_create_show_are_has_cap_in_room() {
        let handle = format!("docs{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let anna_h = format!("anna{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (docs_name, docs_path) = unique("skin-docs");
        let (anna_name, anna_path) = unique("skin-anna");
        let docs_id = insert_project_handle(&docs_name, &docs_path, &handle);
        let anna_id = insert_project_handle(&anna_name, &anna_path, &anna_h);
        let mut policy = k2_core::skin::RoomPolicy::new();
        policy.insert(
            anna_id.clone(),
            vec![
                k2_core::skin::CAP_THREAD_READ.into(),
                k2_core::skin::CAP_THREAD_POST.into(),
            ],
        );
        policy.insert(
            docs_id.clone(),
            vec![
                k2_core::skin::CAP_THREAD_READ.into(),
                k2_core::skin::CAP_THREAD_POST.into(),
                k2_core::skin::CAP_TICKETS_READ.into(),
                k2_core::skin::CAP_TICKETS_POST.into(),
            ],
        );
        let pass = skin_session("bob", policy);

        let list_docs = handle_list_gated(&list_params(&handle, &[]), Some(pass.clone()));
        assert_eq!(list_docs.status, "200 OK", "{}", list_docs.body);

        let list_anna = handle_list_gated(&list_params(&anna_h, &[]), Some(pass.clone()));
        assert_eq!(list_anna.status, "403 Forbidden", "{}", list_anna.body);
        assert!(
            list_anna.body.contains("missing capability tickets:read"),
            "{}",
            list_anna.body
        );
        assert!(!list_anna.body.contains("skin_room"), "{}", list_anna.body);

        let list_abs = handle_list_gated(&list_params(&docs_path, &[]), Some(pass.clone()));
        assert_eq!(list_abs.status, "403 Forbidden", "{}", list_abs.body);
        assert!(list_abs.body.contains("skin_room"), "{}", list_abs.body);

        let created = super::handle_create_gated(
            serde_json::json!({ "project": handle, "title": "Docs ask" })
                .to_string()
                .as_bytes(),
            Some(&pass),
            CreateDoor::App,
        );
        assert_eq!(created.status, "200 OK", "{}", created.body);
        let docs_ticket = serde_json::from_str::<serde_json::Value>(&created.body).expect("json")
            ["id"]
            .as_str()
            .expect("id")
            .to_string();

        let anna_item = feedback::create(feedback::NewFeedback {
            project_id: anna_id.clone(),
            session_id: None,
            session_kind: None,
            agent_name: "anna".into(),
            kind: "question".into(),
            title: "Anna ask".into(),
            body: None,
            options: None,
            priority: 3,
        })
        .expect("anna item");

        let show_anna = handle_show_gated(
            &HashMap::from([("id".into(), anna_item.id.clone())]),
            Some(pass.clone()),
        );
        assert_eq!(show_anna.status, "403 Forbidden", "{}", show_anna.body);
        assert!(
            show_anna.body.contains("missing capability tickets:read"),
            "{}",
            show_anna.body
        );

        let show_docs = handle_show_gated(
            &HashMap::from([("id".into(), docs_ticket.clone())]),
            Some(pass.clone()),
        );
        assert_eq!(show_docs.status, "200 OK", "{}", show_docs.body);

        let show_missing = handle_show_gated(
            &HashMap::from([("id".into(), "00000000-0000-0000-0000-000000000000".into())]),
            Some(pass.clone()),
        );
        assert_eq!(
            show_missing.status, "403 Forbidden",
            "{}",
            show_missing.body
        );
        assert!(
            show_missing.body.contains("skin_room"),
            "unknown id must not 404: {}",
            show_missing.body
        );

        let comment = super::handle_comment_gated(
            serde_json::json!({
                "id": docs_ticket,
                "body": "ship it",
                "author": "owner"
            })
            .to_string()
            .as_bytes(),
            "bob",
            Some(&pass),
        );
        assert_eq!(comment.status, "200 OK", "{}", comment.body);
        let cv: serde_json::Value = serde_json::from_str(&comment.body).expect("json");
        assert_eq!(cv["author"], "bob", "{}", comment.body);

        let create_anna = super::handle_create_gated(
            serde_json::json!({ "project": anna_h, "title": "nope" })
                .to_string()
                .as_bytes(),
            Some(&pass),
            CreateDoor::App,
        );
        assert_eq!(create_anna.status, "403 Forbidden", "{}", create_anna.body);
        assert!(
            create_anna.body.contains("missing capability tickets:post"),
            "{}",
            create_anna.body
        );
    }

    // ── Ticket HTML brief (prd-ticket-html-brief-v1 T4/T5) ─────────────

    use k2_core::feedback_brief::{with_policy_override, BriefPolicy};

    /// Assigns `owner` (always a user on this server) unless `extra` sets
    /// `assignees` itself, so the brief tests see only brief warnings.
    fn create_at(path: &str, door: CreateDoor, extra: serde_json::Value) -> CliResponse {
        let mut body =
            serde_json::json!({ "project": path, "title": "Brief door", "assignees": ["owner"] });
        if let (Some(dst), Some(src)) = (body.as_object_mut(), extra.as_object()) {
            for (k, v) in src {
                dst.insert(k.clone(), v.clone());
            }
        }
        handle_create_gated(body.to_string().as_bytes(), None, door)
    }

    fn parse(resp: &CliResponse) -> serde_json::Value {
        serde_json::from_str(&resp.body).expect("valid JSON")
    }

    fn warning_codes(v: &serde_json::Value) -> Vec<String> {
        v["warnings"]
            .as_array()
            .expect("warnings is always an array")
            .iter()
            .map(|w| w["code"].as_str().expect("warning code").to_string())
            .collect()
    }

    /// T4, Require column: only the owner door must attach a brief.
    #[test]
    fn brief_door_matrix_require() {
        let (name, path) = unique("brief-require");
        insert_project(&name, &path);
        with_policy_override(BriefPolicy::Require, || {
            let r = create_at(&path, CreateDoor::Owner, serde_json::json!({}));
            assert_eq!(r.status, "400 Bad Request", "{}", r.body);
            let v = parse(&r);
            assert_eq!(v["error"]["code"], "brief_required", "{}", r.body);
            assert!(
                v["error"]["hint"].as_str().expect("hint").contains("k2 tickets template"),
                "{}",
                r.body
            );
            // An empty string is the same as no brief.
            let r = create_at(&path, CreateDoor::Owner, serde_json::json!({ "briefHtml": "  " }));
            assert_eq!(parse(&r)["error"]["code"], "brief_required", "{}", r.body);

            for door in [CreateDoor::App, CreateDoor::Connect] {
                let r = create_at(&path, door, serde_json::json!({}));
                assert_eq!(r.status, "200 OK", "{door:?}: {}", r.body);
                let v = parse(&r);
                assert_eq!(v["hasBrief"], false, "{door:?}: {}", r.body);
                assert!(warning_codes(&v).is_empty(), "{door:?}: {}", r.body);
            }

            let r = create_at(&path, CreateDoor::Owner, serde_json::json!({ "briefHtml": TEST_BRIEF }));
            assert_eq!(r.status, "200 OK", "{}", r.body);
            let v = parse(&r);
            assert_eq!(v["hasBrief"], true, "{}", r.body);
            assert_eq!(v["briefBytes"], TEST_BRIEF.len() as i64, "{}", r.body);
            assert!(warning_codes(&v).is_empty(), "{}", r.body);

            // Rosson default 2: fyi is exempt even in Require.
            let r = create_at(&path, CreateDoor::Owner, serde_json::json!({ "kind": "fyi" }));
            assert_eq!(r.status, "200 OK", "{}", r.body);
            assert!(warning_codes(&parse(&r)).is_empty(), "{}", r.body);
        });
    }

    /// T4, Warn column (the 0.43.2 default): the owner door still files,
    /// and the response says a brief is missing.
    #[test]
    fn brief_door_matrix_warn() {
        let (name, path) = unique("brief-warn");
        insert_project(&name, &path);
        with_policy_override(BriefPolicy::Warn, || {
            let r = create_at(&path, CreateDoor::Owner, serde_json::json!({ "kind": "approval" }));
            assert_eq!(r.status, "200 OK", "{}", r.body);
            let v = parse(&r);
            assert_eq!(v["hasBrief"], false, "{}", r.body);
            assert!(v["briefBytes"].is_null(), "{}", r.body);
            assert_eq!(warning_codes(&v), vec!["brief_missing"], "{}", r.body);
            let hint = v["warnings"][0]["hint"].as_str().expect("hint");
            assert!(hint.contains("only warns"), "the response says it is a grace period: {hint}");
            assert!(feedback::get_item(v["id"].as_str().expect("id")).is_some());

            let r = create_at(&path, CreateDoor::Owner, serde_json::json!({ "kind": "fyi" }));
            assert!(warning_codes(&parse(&r)).is_empty(), "fyi is exempt: {}", r.body);
            let r = create_at(&path, CreateDoor::Connect, serde_json::json!({}));
            assert!(warning_codes(&parse(&r)).is_empty(), "people never warn: {}", r.body);
        });
    }

    // ── Assignee policy (0.43.2) ─────────────────────────────────────
    //
    // Only `owner` and uuid names here: the in-file tests read the real
    // HOME's user store (read-only), so a Connect user is proved in
    // tests/ticket_brief_integration.rs under a temp HOME instead.

    use k2_core::feedback_brief::{with_assignee_policy_override, AssigneePolicy};

    /// [`create_at`] with [`TEST_BRIEF`], so only assignee warnings show.
    fn create_assign(path: &str, door: CreateDoor, extra: serde_json::Value) -> CliResponse {
        let mut body = serde_json::json!({ "briefHtml": TEST_BRIEF });
        if let (Some(dst), Some(src)) = (body.as_object_mut(), extra.as_object()) {
            for (k, v) in src {
                dst.insert(k.clone(), v.clone());
            }
        }
        create_at(path, door, body)
    }

    fn no_one_by_this_name() -> String {
        format!("nobody-{}", uuid::Uuid::new_v4().simple())
    }

    /// Warn column (the 0.43.2 default) on the agent door.
    #[test]
    fn assignee_door_matrix_warn() {
        let (name, path) = unique("assignee-warn");
        insert_project(&name, &path);
        with_assignee_policy_override(AssigneePolicy::Warn, || {
            // No assignee: filed, unassigned, with assignee_required.
            for extra in [
                serde_json::json!({ "assignees": [] }),
                serde_json::json!({ "assignees": null }),
            ] {
                let r = create_assign(&path, CreateDoor::Owner, extra);
                assert_eq!(r.status, "200 OK", "{}", r.body);
                let v = parse(&r);
                assert_eq!(warning_codes(&v), vec!["assignee_required"], "{}", r.body);
                let id = v["id"].as_str().expect("id");
                let hint = v["warnings"][0]["hint"].as_str().expect("hint");
                assert!(
                    hint.starts_with(k2_core::feedback_brief::ASSIGNEE_REQUIRED_MESSAGE),
                    "{hint}"
                );
                assert!(hint.contains("required in a future update"), "{hint}");
                assert!(hint.contains(&format!("k2 tickets assign {}", &id[..8])), "{hint}");
                assert!(hint.contains("k2 connections list --users"), "{hint}");
                assert_eq!(v["assignees"], serde_json::json!([]), "{}", r.body);
                assert!(feedback::get_item(id).is_some(), "Warn still files the ticket");
            }

            // Assigned to a user here: no warning, stored.
            let r = create_assign(&path, CreateDoor::Owner, serde_json::json!({ "assignees": ["owner"] }));
            let v = parse(&r);
            assert!(warning_codes(&v).is_empty(), "{}", r.body);
            assert_eq!(v["assignees"], serde_json::json!(["owner"]), "{}", r.body);

            // Not a user here: filed and stored (snapshot), assignee_unknown.
            let ghost = no_one_by_this_name();
            let r = create_assign(
                &path,
                CreateDoor::Owner,
                serde_json::json!({ "assignees": ["owner", ghost, "  "] }),
            );
            assert_eq!(r.status, "200 OK", "{}", r.body);
            let v = parse(&r);
            assert_eq!(warning_codes(&v), vec!["assignee_unknown"], "{}", r.body);
            let hint = v["warnings"][0]["hint"].as_str().expect("hint");
            assert!(hint.starts_with(&format!("`{ghost}` is not a user on this server")), "{hint}");
            assert!(!hint.contains("`owner`"), "owner is a user here: {hint}");
            assert_eq!(v["assignees"], serde_json::json!([ghost, "owner"]), "{}", r.body);

            // fyi is exempt from assignee_required…
            let r = create_assign(
                &path,
                CreateDoor::Owner,
                serde_json::json!({ "kind": "fyi", "assignees": [] }),
            );
            assert_eq!(r.status, "200 OK", "{}", r.body);
            assert!(warning_codes(&parse(&r)).is_empty(), "fyi is exempt: {}", r.body);
            // …but a name that is nobody here still warns.
            let r = create_assign(
                &path,
                CreateDoor::Owner,
                serde_json::json!({ "kind": "fyi", "assignees": [no_one_by_this_name()] }),
            );
            assert_eq!(warning_codes(&parse(&r)), vec!["assignee_unknown"], "{}", r.body);

            // People never need an assignee.
            for door in [CreateDoor::App, CreateDoor::Connect] {
                let r = create_assign(&path, door, serde_json::json!({ "assignees": [] }));
                assert_eq!(r.status, "200 OK", "{door:?}: {}", r.body);
                assert!(warning_codes(&parse(&r)).is_empty(), "{door:?}: {}", r.body);
            }
        });
    }

    /// Require column: the one-line flip refuses on the agent door and
    /// stores nothing; people and fyi still file.
    #[test]
    fn assignee_door_matrix_require() {
        let (name, path) = unique("assignee-require");
        let pid = insert_project(&name, &path);
        with_assignee_policy_override(AssigneePolicy::Require, || {
            let r = create_assign(&path, CreateDoor::Owner, serde_json::json!({ "assignees": [] }));
            assert_eq!(r.status, "400 Bad Request", "{}", r.body);
            let v = parse(&r);
            assert_eq!(v["error"]["code"], "assignee_required", "{}", r.body);
            assert!(
                v["error"]["hint"].as_str().expect("hint").contains("--assign <user>"),
                "{}",
                r.body
            );
            let r = create_assign(
                &path,
                CreateDoor::Owner,
                serde_json::json!({ "assignees": [no_one_by_this_name()] }),
            );
            assert_eq!(r.status, "400 Bad Request", "{}", r.body);
            assert_eq!(parse(&r)["error"]["code"], "assignee_unknown", "{}", r.body);
            let stored = feedback::list_for_project(&pid, &feedback::ListFilter::All).expect("list");
            assert!(stored.is_empty(), "a refusal stores no ticket: {stored:?}");

            let r = create_assign(&path, CreateDoor::Owner, serde_json::json!({ "assignees": ["owner"] }));
            assert_eq!(r.status, "200 OK", "{}", r.body);
            assert!(warning_codes(&parse(&r)).is_empty(), "{}", r.body);
            let r = create_assign(
                &path,
                CreateDoor::Owner,
                serde_json::json!({ "kind": "fyi", "assignees": [] }),
            );
            assert_eq!(r.status, "200 OK", "fyi is exempt: {}", r.body);
            // A person naming nobody here is told, not refused.
            let r = create_assign(
                &path,
                CreateDoor::Connect,
                serde_json::json!({ "assignees": [no_one_by_this_name()] }),
            );
            assert_eq!(r.status, "200 OK", "{}", r.body);
            assert_eq!(warning_codes(&parse(&r)), vec!["assignee_unknown"], "{}", r.body);
        });
    }

    /// 0.43.2 ships Warn; the flip is one line in feedback_brief.rs.
    #[test]
    fn assignee_policy_ships_warn_in_0_43_2() {
        assert_eq!(k2_core::feedback_brief::ASSIGNEE_POLICY, AssigneePolicy::Warn);
        assert_eq!(k2_core::feedback_brief::assignee_policy(), AssigneePolicy::Warn);
        assert!(k2_core::feedback_brief::assignee_required("question"));
        assert!(k2_core::feedback_brief::assignee_required("approval"));
        assert!(!k2_core::feedback_brief::assignee_required("fyi"));
    }

    /// `assign` keeps assigning snapshots, and warns on a name that is
    /// not a user here (POST-only: the GET chain 405s).
    #[test]
    fn assign_warns_on_unknown_user() {
        let (name, path) = unique("assign-unknown");
        insert_project(&name, &path);
        let created =
            create_via_route(&path, "Assign me", serde_json::json!({ "assignees": ["owner"] }));
        let id = created["id"].as_str().expect("id").to_string();
        let ghost = no_one_by_this_name();
        let r = handle_assign(
            serde_json::json!({ "id": id, "usernames": ["owner", ghost] })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v = parse(&r);
        assert_eq!(v["assignees"], serde_json::json!([ghost, "owner"]), "{}", r.body);
        assert_eq!(warning_codes(&v), vec!["assignee_unknown"], "{}", r.body);
        let r = handle_assign(
            serde_json::json!({ "id": id, "usernames": ["owner"] }).to_string().as_bytes(),
        );
        assert!(warning_codes(&parse(&r)).is_empty(), "{}", r.body);
        let get = dispatch("/cli/feedback/assign", &HashMap::new()).expect("assign claimed on GET");
        assert_eq!(get.status, "405 Method Not Allowed", "{}", get.body);
    }

    /// The shipped constant is Warn for 0.43.2. 0.43.3 flips it (S8);
    /// this assertion flips with it.
    #[test]
    fn brief_policy_ships_warn_in_0_43_2() {
        assert_eq!(k2_core::feedback_brief::BRIEF_POLICY, BriefPolicy::Warn);
        assert_eq!(k2_core::feedback_brief::brief_policy(), BriefPolicy::Warn);
    }

    /// Bad briefs are refused with stable `brief_*` codes on every door,
    /// and nothing is stored.
    #[test]
    fn brief_refusals_have_stable_codes() {
        let (name, path) = unique("brief-refuse");
        let pid = insert_project(&name, &path);
        let big = format!("<p>{}</p>", "x".repeat(k2_core::feedback_brief::MAX_BRIEF_BYTES));
        for (brief, code) in [
            (big.as_str(), "brief_too_large"),
            ("<div> <br> </div><script>x()</script>", "brief_empty"),
        ] {
            for door in [CreateDoor::Owner, CreateDoor::App, CreateDoor::Connect] {
                let r = create_at(&path, door, serde_json::json!({ "briefHtml": brief }));
                assert_eq!(r.status, "400 Bad Request", "{door:?} {code}: {}", &r.body[..200.min(r.body.len())]);
                assert_eq!(parse(&r)["error"]["code"], code, "{door:?}");
            }
        }
        let n = feedback::list_for_project(&pid, &feedback::ListFilter::All).expect("list");
        assert!(n.is_empty(), "a refused brief stores no ticket: {n:?}");
    }

    /// Warnings: removed markup and a missing `.k2-need` section.
    #[test]
    fn brief_warnings_sanitized_and_no_need() {
        let (name, path) = unique("brief-warnings");
        insert_project(&name, &path);
        let r = create_at(
            &path,
            CreateDoor::Owner,
            serde_json::json!({ "briefHtml": "<p onclick=\"x()\">Problem</p>" }),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(
            warning_codes(&parse(&r)),
            vec!["brief_sanitized", "brief_no_need"],
            "{}",
            r.body
        );
    }

    /// T5: plain `show` reports the brief but never carries it;
    /// `show?brief=1` returns the cleaned HTML and the text copy; list
    /// items carry `hasBrief`/`briefBytes` and never the HTML.
    #[test]
    fn brief_show_and_list_wire() {
        let (name, path) = unique("brief-wire");
        let pid = insert_project(&name, &path);
        let raw = "<h2>Problem</h2><p>Deploy is <b>blocked</b>.<script>evil()</script></p>\
                   <section class=\"k2-need\"><p>Pick one.</p></section>";
        let created = create_via_route(&path, "Wire", serde_json::json!({ "briefHtml": raw }));
        let id = created["id"].as_str().expect("id").to_string();
        let clean = "<h2>Problem</h2><p>Deploy is <b>blocked</b>.</p>\
                     <section class=\"k2-need\"><p>Pick one.</p></section>";

        let plain = handle_show(&HashMap::from([("id".to_string(), id.clone())]));
        assert_eq!(plain.status, "200 OK", "{}", plain.body);
        let pv = parse(&plain);
        assert_eq!(pv["hasBrief"], true, "{}", plain.body);
        assert_eq!(pv["briefBytes"], clean.len() as i64, "{}", plain.body);
        assert!(pv.get("brief").is_none(), "plain show has no brief key: {}", plain.body);
        assert!(!plain.body.contains("k2-need"), "{}", plain.body);

        let full = handle_show(&HashMap::from([
            ("id".to_string(), id.clone()),
            ("brief".to_string(), "1".to_string()),
        ]));
        assert_eq!(full.status, "200 OK", "{}", full.body);
        let fv = parse(&full);
        assert_eq!(fv["brief"]["html"], clean, "{}", full.body);
        assert_eq!(fv["brief"]["text"], "Problem\nDeploy is blocked.\nPick one.", "{}", full.body);
        assert_eq!(fv["brief"]["bytes"], clean.len() as i64);
        assert_eq!(fv["brief"]["sanitizer"], "k2-brief-v1");
        assert_eq!(fv["brief"]["sha256"].as_str().expect("sha").len(), 64);
        assert!(fv["brief"]["createdAt"].as_i64().expect("createdAt") > 0);

        // A ticket without a brief: `brief: null` on brief=1.
        let bare = create_at(&path, CreateDoor::Connect, serde_json::json!({}));
        let bare_id = parse(&bare)["id"].as_str().expect("id").to_string();
        let bv = parse(&handle_show(&HashMap::from([
            ("id".to_string(), bare_id),
            ("brief".to_string(), "1".to_string()),
        ])));
        assert!(bv["brief"].is_null(), "{bv}");
        assert_eq!(bv["hasBrief"], false);

        let list = list_items(&pid, &list_params(&path, &[("all", "1")]));
        assert_eq!(list.status, "200 OK", "{}", list.body);
        assert!(!list.body.contains("k2-need"), "list never carries HTML: {}", list.body);
        let lv = parse(&list);
        let row = lv["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|i| i["id"] == id.as_str())
            .expect("the brief ticket is listed")
            .clone();
        assert_eq!(row["hasBrief"], true, "{row}");
        assert_eq!(row["briefBytes"], clean.len() as i64, "{row}");

        let host = handle_list_all(&HashMap::from([("all".to_string(), "1".to_string())]));
        assert!(!host.body.contains("k2-need"), "list-all never carries HTML");
        let hv = parse(&host);
        let hrow = hv["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|i| i["id"] == id.as_str())
            .expect("list-all has it")
            .clone();
        assert_eq!(hrow["hasBrief"], true, "{hrow}");
        assert_eq!(hrow["linked"], true, "{hrow}");
    }

    /// H29: the glob/test entry treats `create` as the owner door, so a
    /// path that skipped the exact arm is never more permissive.
    #[test]
    fn dispatch_post_as_create_is_the_owner_door() {
        let (name, path) = unique("brief-glob");
        insert_project(&name, &path);
        with_policy_override(BriefPolicy::Require, || {
            let r = dispatch_post_as(
                "/cli/feedback/create",
                serde_json::json!({ "project": path, "title": "glob" }).to_string().as_bytes(),
                "julie",
            );
            assert_eq!(parse(&r)["error"]["code"], "brief_required", "{}", r.body);
        });
    }

    /// H30: the 413 the dispatcher sends for an oversize create body.
    #[test]
    fn create_body_too_large_is_413_brief_too_large() {
        let r = create_body_too_large(Some(3 * 1024 * 1024));
        assert_eq!(r.status, "413 Payload Too Large");
        let v = parse(&r);
        assert_eq!(v["error"]["code"], "brief_too_large");
        assert!(v["error"]["hint"].as_str().expect("hint").contains("3145728 bytes"));
    }

    // ── prd-app-tickets-websocket-v1 — ticket_changed + app parity ──────

    /// The `TicketChanged` events for `id` since `rx` subscribed, as
    /// `(change, via, status, hasBrief)`. A lagged receiver fails loudly.
    fn ticket_events(
        rx: &mut tokio::sync::broadcast::Receiver<crate::session_events::SessionEvent>,
        id: &str,
        project_id: &str,
    ) -> Vec<(String, Option<String>, String, bool)> {
        let mut out = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(crate::session_events::SessionEvent::TicketChanged {
                    project_id: pid,
                    id: tid,
                    change,
                    status,
                    via,
                    has_brief,
                }) if tid == id => {
                    assert_eq!(pid, project_id, "ticket_changed carries the ticket's room");
                    out.push((change, via, status, has_brief));
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(e) => panic!("session-events receiver: {e:?}"),
            }
        }
        out
    }

    fn tickets_pass(username: &str, project_id: &str, caps: &[&str]) -> k2_core::skin::SkinPass {
        let mut policy = k2_core::skin::RoomPolicy::new();
        policy.insert(project_id.to_string(), caps.iter().map(|c| (*c).to_string()).collect());
        skin_session(username, policy)
    }

    /// T2/T3: every mutation emits exactly one `ticket_changed` with the
    /// right change / via / status, on every door. Option pick vs free text
    /// over the app door; app assign + status set; apps still cannot set
    /// `answered`.
    #[test]
    fn ticket_changed_fires_once_per_mutation_with_shape() {
        let handle = format!("tkt{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (name, path) = unique("ticket-events");
        let pid = insert_project_handle(&name, &path, &handle);
        let pass = tickets_pass(
            "bob",
            &pid,
            &[k2_core::skin::CAP_TICKETS_READ, k2_core::skin::CAP_TICKETS_POST],
        );
        let mut rx = crate::session_events::subscribe();
        let app = |path: &str, body: serde_json::Value| {
            dispatch_post_as_gated(
                path,
                body.to_string().as_bytes(),
                "bob",
                Some(pass.clone()),
                CreateDoor::App,
            )
        };

        // create (app, with a brief and options)
        let r = app(
            "/cli/feedback/create",
            serde_json::json!({
                "project": handle, "title": "Pick a colour",
                "options": ["Navy", "Teal"], "briefHtml": TEST_BRIEF,
            }),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let id = parse(&r)["id"].as_str().expect("id").to_string();
        assert_eq!(
            ticket_events(&mut rx, &id, &pid),
            vec![("created".into(), None, "waiting".into(), true)]
        );

        // free text → one commented/free_text frame, status needs_discussion
        let r = app("/cli/feedback/comment", serde_json::json!({"id": id, "body": "why navy?"}));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(parse(&r)["answered"], false, "{}", r.body);
        assert_eq!(parse(&r)["status"], "needs_discussion", "{}", r.body);
        assert_eq!(
            ticket_events(&mut rx, &id, &pid),
            vec![("commented".into(), Some("free_text".into()), "needs_discussion".into(), true)]
        );

        // option pick → one answered/option_pick frame
        let r = app(
            "/cli/feedback/comment",
            serde_json::json!({"id": id, "body": "Navy", "optionPick": true}),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(parse(&r)["answered"], true, "{}", r.body);
        assert_eq!(parse(&r)["author"], "bob", "{}", r.body);
        let item = feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "answered");
        assert_eq!(item.answer.as_deref(), Some("Navy"));
        assert_eq!(
            ticket_events(&mut rx, &id, &pid),
            vec![("answered".into(), Some("option_pick".into()), "answered".into(), true)]
        );

        // app status set from the allowed set
        let r = app("/cli/feedback/resolve", serde_json::json!({"id": id, "status": "planned"}));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(
            ticket_events(&mut rx, &id, &pid),
            vec![("status_changed".into(), None, "planned".into(), true)]
        );
        let r = app(
            "/cli/feedback/resolve",
            serde_json::json!({"id": id, "status": "needs_discussion"}),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(
            ticket_events(&mut rx, &id, &pid),
            vec![("status_changed".into(), None, "needs_discussion".into(), true)]
        );
        // apps never set answered, and an unknown status is refused
        for bad in [
            serde_json::json!({"id": id, "status": "answered", "answer": "x"}),
            serde_json::json!({"id": id, "status": "shipped"}),
        ] {
            let r = app("/cli/feedback/resolve", bad);
            assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        }
        assert!(ticket_events(&mut rx, &id, &pid).is_empty(), "a refusal emits nothing");

        // app assign: stored, same unknown-name warning as the owner
        let r = app(
            "/cli/feedback/assign",
            serde_json::json!({"id": id, "usernames": ["owner", "ghost-user-zz"]}),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v = parse(&r);
        assert_eq!(v["assignees"], serde_json::json!(["ghost-user-zz", "owner"]), "{v}");
        let warnings = v["warnings"].as_array().expect("warnings array");
        assert_eq!(warnings.len(), 1, "{v}");
        assert_eq!(warnings[0]["code"], "assignee_unknown", "{v}");
        assert!(warnings[0].to_string().contains("ghost-user-zz"), "{v}");
        assert_eq!(
            ticket_events(&mut rx, &id, &pid),
            vec![("assigned".into(), None, "needs_discussion".into(), true)]
        );

        // owner doors: answer route, agent comment, agent settle
        let r = dispatch_post_as(
            "/cli/feedback/answer",
            serde_json::json!({"id": id, "answer": "Teal after all"}).to_string().as_bytes(),
            "owner",
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(
            ticket_events(&mut rx, &id, &pid),
            vec![("answered".into(), Some("answer".into()), "answered".into(), true)]
        );
        let r = dispatch_post_as(
            "/cli/feedback/comment",
            serde_json::json!({"id": id, "body": "noted", "author": "scout"}).to_string().as_bytes(),
            "owner",
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(
            ticket_events(&mut rx, &id, &pid),
            vec![("commented".into(), Some("agent".into()), "answered".into(), true)]
        );
        let r = dispatch_post_as(
            "/cli/feedback/resolve",
            serde_json::json!({"id": id, "status": "answered", "answer": "Teal", "author": "scout"})
                .to_string()
                .as_bytes(),
            "owner",
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(
            ticket_events(&mut rx, &id, &pid),
            vec![("answered".into(), Some("settled".into()), "answered".into(), true)]
        );

        // dismissed / resolved are their own changes
        let r = app("/cli/feedback/resolve", serde_json::json!({"id": id, "status": "dismissed"}));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let r = app("/cli/feedback/resolve", serde_json::json!({"id": id}));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(
            ticket_events(&mut rx, &id, &pid),
            vec![
                ("dismissed".into(), None, "dismissed".into(), true),
                ("resolved".into(), None, "resolved".into(), true),
            ]
        );

        // A ticket without a brief says so.
        let r = app("/cli/feedback/create", serde_json::json!({"project": handle, "title": "plain"}));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let plain = parse(&r)["id"].as_str().expect("id").to_string();
        assert_eq!(
            ticket_events(&mut rx, &plain, &pid),
            vec![("created".into(), None, "waiting".into(), false)]
        );
    }

    /// T1/T5: app assign is room-jailed and cap-gated; a guest without
    /// tickets:read gets 403 on reads; guest reads carry no path or
    /// session ids; the brief is the stored, cleaned one.
    #[test]
    fn app_assign_and_reads_are_room_jailed_and_projected() {
        let h_a = format!("ra{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let h_b = format!("rb{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let (name_a, path_a) = unique("jail-a");
        let (name_b, path_b) = unique("jail-b");
        let a = insert_project_handle(&name_a, &path_a, &h_a);
        let _b = insert_project_handle(&name_b, &path_b, &h_b);
        let in_a = create_via_route(&path_a, "A ask", serde_json::json!({
            "briefHtml": "<p>Hi<script>alert(1)</script></p>",
            "sessionId": "sess-secret", "sessionKind": "canonical",
        }))["id"]
            .as_str()
            .expect("id")
            .to_string();
        let in_b = create_via_route(&path_b, "B ask", serde_json::json!({}))["id"]
            .as_str()
            .expect("id")
            .to_string();

        let reader = tickets_pass("rita", &a, &[k2_core::skin::CAP_TICKETS_READ]);
        let writer = tickets_pass(
            "wes",
            &a,
            &[k2_core::skin::CAP_TICKETS_READ, k2_core::skin::CAP_TICKETS_POST],
        );
        let thread_only = tickets_pass("tom", &a, &[k2_core::skin::CAP_THREAD_READ]);
        let assign = |pass: &k2_core::skin::SkinPass, id: &str| {
            dispatch_post_as_gated(
                "/cli/feedback/assign",
                serde_json::json!({"id": id, "usernames": ["owner"]}).to_string().as_bytes(),
                &pass.username,
                Some(pass.clone()),
                CreateDoor::App,
            )
        };
        let r = assign(&reader, &in_a);
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert!(r.body.contains("missing capability tickets:post"), "{}", r.body);
        let r = assign(&writer, &in_b);
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert!(r.body.contains("skin_room"), "other room is skin_room: {}", r.body);
        let r = assign(&writer, "00000000-0000-0000-0000-000000000000");
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert!(r.body.contains("skin_room"), "no 404 oracle: {}", r.body);
        assert!(feedback::get_item(&in_b).expect("b").assignees.is_empty(), "B untouched");

        // No tickets:read → 403 on list and show.
        let r = handle_list_gated(&list_params(&h_a, &[]), Some(thread_only.clone()));
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert!(r.body.contains("missing capability tickets:read"), "{}", r.body);
        let r = handle_show_gated(&HashMap::from([("id".into(), in_a.clone())]), Some(thread_only));
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);

        // tickets:read: brief (cleaned) + assignee, no host internals.
        let r = assign(&writer, &in_a);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let r = handle_show_gated(
            &HashMap::from([("id".into(), in_a.clone()), ("brief".into(), "1".into())]),
            Some(reader.clone()),
        );
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v = parse(&r);
        assert_eq!(v["assignees"], serde_json::json!(["owner"]), "{v}");
        assert_eq!(v["brief"]["html"], "<p>Hi</p>", "{v}");
        assert_eq!(v["hasBrief"], true, "{v}");
        for k in GUEST_HIDDEN_KEYS {
            assert!(v.get(k).is_none(), "guest show must not carry {k}: {v}");
        }
        assert!(!r.body.contains(&path_a), "no path: {}", r.body);
        assert!(!r.body.contains("sess-secret"), "no session id: {}", r.body);
        let r = handle_list_gated(&list_params(&h_a, &[]), Some(reader));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v = parse(&r);
        let items = v["items"].as_array().expect("items");
        assert_eq!(items.len(), 1, "room A only: {v}");
        for k in GUEST_HIDDEN_KEYS {
            assert!(items[0].get(k).is_none(), "guest list must not carry {k}: {v}");
        }
        // The owner door is unchanged.
        let r = handle_show_gated(&HashMap::from([("id".into(), in_a.clone())]), None);
        let v = parse(&r);
        assert_eq!(v["projectPath"], path_a.as_str(), "{v}");
        assert_eq!(v["sessionId"], "sess-secret", "{v}");
    }
}
