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
//! F3 injection (PRD §4.3, §7 decision 1) + the comment-thread model:
//! every HUMAN-authored message (an answer OR a comment) ALWAYS
//! best-effort injects into the ASKING session — sandbox rows target
//! their live cell, canonical/sessionless rows go through the
//! workspace-agent `deliver_live` path with wake=true (a dormant
//! canonical agent is woken). Injection runs AFTER the store + emit: a
//! delivery failure never fails the store; the outcome rides the
//! response as `delivered`/`deliveryReason`. A human's FIRST comment
//! on a `waiting` question/approval doubles as the ANSWER behind the
//! scenes (set_answer → status `answered` → `FeedbackAnswered`), so
//! `k2 feedback ask --wait` unblocks — `fyi` NEVER auto-answers (the
//! frozen contract: fyi sits until dismissed/resolved). Agent-authored
//! comments (`k2 feedback comment` passes the agent's name as
//! `author`) store ONLY — no injection back into their own session,
//! no auto-answer. Resolve / dismiss / reopen never inject.

use std::collections::HashMap;

use crate::cli::{need_project, opt_param, str_param};
use crate::cli_response::CliResponse;
use k2_core::feedback::{self, ListFilter, PrefixError};

/// Feedback-domain GET dispatch. Returns `Some(resp)` for a handled
/// path, `None` if the path isn't a feedback-domain route.
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    let resp = match path {
        // ── Reads ───────────────────────────────────────────────────
        // GET /cli/feedback/list?project=<path>[&all=1][&status=<s>]
        // Default shows open items (waiting + answered + needs_discussion), newest first.
        "/cli/feedback/list" => match need_project(params) {
            Ok(p) => {
                let Some(project_id) = resolve_project_id(&p) else {
                    return Some(project_not_registered(&p));
                };
                let filter = match opt_param(params, "status") {
                    Some(s) => ListFilter::Status(s),
                    None if crate::cli::bool_param(params, "all") => ListFilter::All,
                    None => ListFilter::Open,
                };
                match feedback::list_for_project(&project_id, &filter) {
                    Ok(items) => CliResponse::ok_json(
                        serde_json::json!({ "ok": true, "items": items }).to_string(),
                    ),
                    Err(e) => usage_error(e),
                }
            }
            Err(r) => r,
        },

        // GET /cli/feedback/show?id=<id-or-prefix>
        // One item + its full thread. `id` accepts a short unique
        // prefix (ambiguity → `ambiguous_id`, candidates in the hint).
        "/cli/feedback/show" => {
            let id = str_param(params, "id");
            if id.is_empty() {
                return Some(usage_error("missing id (a feedback id or unique prefix)"));
            }
            let full_id = match feedback::resolve_id_prefix(&id) {
                Ok(f) => f,
                Err(e) => return Some(prefix_error_response(&id, e)),
            };
            match feedback::get_with_comments(&full_id) {
                Some((item, comments)) => CliResponse::ok_json(show_json(&item, &comments)),
                None => prefix_error_response(&id, PrefixError::NotFound),
            }
        }

        // ── POST-only mutations reached via the GET chain → 405 ─────
        // (feedback_post_only_route_guards house rule.)
        "/cli/feedback/create" | "/cli/feedback/comment" | "/cli/feedback/answer"
        | "/cli/feedback/resolve" | "/cli/feedback/assign" => CliResponse::method_not_allowed(),

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
pub fn dispatch_post(path: &str, body: &[u8]) -> CliResponse {
    dispatch_post_as(path, body, "owner")
}

pub fn dispatch_post_as(path: &str, body: &[u8], session_author: &str) -> CliResponse {
    match path {
        "/cli/feedback/create" => handle_create(body),
        "/cli/feedback/comment" => handle_comment_as(body, session_author),
        "/cli/feedback/answer" => handle_answer(body),
        "/cli/feedback/resolve" => handle_resolve(body),
        "/cli/feedback/assign" => handle_assign(body),
        _ => CliResponse::not_found(),
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
        PrefixError::NotFound => (
            "not_found",
            format!("no feedback item matches '{given}'"),
        ),
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

/// `projects.name` + `projects.path` for a project id (for the
/// `workspace` field in responses + the `projectPath` event field).
fn project_name_path(project_id: &str) -> (Option<String>, Option<String>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT name, path FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
    )
    .unwrap_or((None, None))
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

/// The `show` wire shape (mockup contract): the item's fields flat at
/// the top level + `workspace` (project name) + `comments` (each with
/// the mockup's `at` alias alongside `createdAt`).
fn show_json(item: &feedback::FeedbackItem, comments: &[feedback::FeedbackComment]) -> String {
    let (name, path) = project_name_path(&item.project_id);
    let mut v = serde_json::to_value(item).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(map) = v.as_object_mut() {
        map.insert("ok".to_string(), serde_json::json!(true));
        map.insert("workspace".to_string(), serde_json::json!(name));
        map.insert("projectPath".to_string(), serde_json::json!(path));
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
    }
    v.to_string()
}

// ── F3 — human message → asking-session injection ─────────────────────

/// Where a human-authored message (answer OR comment) gets injected
/// (F3, PRD §7 decision 1: human messages ALWAYS inject;
/// agent-authored comments and resolve/dismiss/reopen never do).
#[derive(Debug, Clone, PartialEq, Eq)]
enum InjectionTarget {
    /// `session_kind == "sandbox"`: deliver to that LIVE cell only.
    /// A gone cell is a graceful skip — never a wake/spawn (the agent
    /// reads the thread via `k2 feedback show` on its next run).
    SandboxSession(String),
    /// Canonical or null session: workspace-agent addressing (the
    /// `k2 msg` path), wake=true — a dormant canonical agent is woken
    /// and the message delivered on wake.
    WorkspaceAgent,
}

/// Pure injection-target classifier, unit-tested without touching the
/// DB or any live session.
fn injection_target(session_id: Option<&str>, session_kind: Option<&str>) -> InjectionTarget {
    match (session_id, session_kind) {
        (Some(sid), Some("sandbox")) => InjectionTarget::SandboxSession(sid.to_string()),
        _ => InjectionTarget::WorkspaceAgent,
    }
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
    match injection_target(item.session_id.as_deref(), item.session_kind.as_deref()) {
        InjectionTarget::SandboxSession(sid) => {
            // Live-cell-only: check liveness first so a torn-down cell
            // reports a clear `session_gone` instead of `pty_died`.
            let live = k2_core::session::SessionId::parse(&sid)
                .and_then(|s| crate::session_lookup::lookup_by_session_id(&s));
            if live.is_none() {
                return (false, Some("session_gone".to_string()), None);
            }
            let resp = crate::workspace_msg::send_message_to_session(&sid, from, &payload);
            (resp.success, resp.reason, resp.target_session_id)
        }
        InjectionTarget::WorkspaceAgent => {
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
    }
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
}

/// Handler for `POST /cli/feedback/create`. Validates, inserts, and
/// fires `FeedbackCreated` on the `/events` broadcast. Response is the
/// mockup's ask `--json` shape (`ok/id/title/kind/priority/status/
/// options/workspace/sessionId`) plus the full item fields.
pub fn handle_create(body: &[u8]) -> CliResponse {
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
    let Some(path) = crate::workspace_msg::resolve_workspace(&b.project) else {
        return crate::workspace_routes::workspace_not_found_response(&b.project);
    };
    let Some(project_id) = resolve_project_id(&path) else {
        return project_not_registered(&path);
    };
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
    // canonical|sandbox, drop anything else (fail-open to NULL — a bad
    // kind hint must never fail the ask).
    let session_id = b.session_id.filter(|s| !s.trim().is_empty());
    let session_kind = match (&session_id, b.session_kind.as_deref()) {
        (Some(_), Some(k @ ("canonical" | "sandbox"))) => Some(k.to_string()),
        _ => None,
    };

    let mut item = match feedback::create(feedback::NewFeedback {
        project_id,
        session_id,
        session_kind,
        agent_name,
        kind: b.kind.unwrap_or_default(),
        title: b.title,
        body: b.body,
        options: b.options,
        priority: b.priority.unwrap_or(0),
    }) {
        Ok(item) => item,
        Err(e) => return usage_error(e),
    };

    // Optional assignees at create — set before push so mobile targeting
    // and the create response include them. Snapshots only (no FK).
    if let Some(names) = b.assignees {
        if !names.is_empty() {
            match feedback::set_assignees(&item.id, &names) {
                Ok(updated) => item = updated,
                Err(e) => return usage_error(e),
            }
        }
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
    crate::push_routes::notify_feedback_created(
        &item.agent_name,
        &item.id,
        &item.assignees,
    );

    let (name, _) = project_name_path(&item.project_id);
    let mut v = serde_json::to_value(&item).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(map) = v.as_object_mut() {
        map.insert("ok".to_string(), serde_json::json!(true));
        map.insert("workspace".to_string(), serde_json::json!(name));
    }
    CliResponse::ok_json(v.to_string())
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
///   - on a `waiting` question/approval, the comment IS the answer:
///     `set_answer` → status `answered` → `FeedbackAnswered` emit —
///     `ask --wait` unblocks and prints it. `fyi` NEVER auto-answers.
///   - ALWAYS best-effort injects into the asking session via the
///     shared F3 machinery ([`deliver_to_asker`], wake=true) AFTER
///     the store + emit; a delivery failure never fails the store.
///     The outcome rides the response (`delivered`/`deliveryReason`/
///     `deliveredSessionId`, plus `answered` for the auto-answer).
/// - AGENT-authored: store only (thread bump), no injection back into
///   its own session, no auto-answer, no delivery fields.
pub fn handle_comment(body: &[u8]) -> CliResponse {
    handle_comment_as(body, "owner")
}

pub fn handle_comment_as(body: &[u8], session_author: &str) -> CliResponse {
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

    // Agent comment — store only, current shape (no delivery fields).
    if !is_human {
        return match feedback::add_comment(&full_id, author, &b.body) {
            Ok(c) => {
                emit_commented(&full_id, &c.author);
                // Push assignees (or all devices if unassigned) on thread
                // activity — not just create.
                if let Some(item) = feedback::get_item(&full_id) {
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

    // Human comment. A first comment on a waiting question/approval
    // doubles as the ANSWER (frozen contract: --wait prints it); fyi
    // never auto-answers, it sits until dismissed/resolved.
    let Some(before) = feedback::get_item(&full_id) else {
        return prefix_error_response(&b.id, PrefixError::NotFound);
    };
    let answers = before.status == "waiting"
        && matches!(before.kind.as_str(), "question" | "approval");

    let (item, comment) = if answers {
        match feedback::set_answer(&full_id, author, &b.body) {
            Ok(pair) => pair,
            Err(e) => return usage_error(e),
        }
    } else {
        match feedback::add_comment(&full_id, author, &b.body) {
            Ok(c) => match feedback::get_item(&full_id) {
                Some(item) => (item, c),
                None => return usage_error("feedback row vanished after comment"),
            },
            Err(e) => return usage_error(e),
        }
    };

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
    // feedback:commented rides every stored comment (see
    // [`emit_commented`]) — also before the injection, so an open
    // thread panel refreshes without waiting on a slow wake.
    emit_commented(&item.id, &comment.author);
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
pub fn handle_answer(body: &[u8]) -> CliResponse {
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
    let full_id = match feedback::resolve_id_prefix(&b.id) {
        Ok(f) => f,
        Err(e) => return prefix_error_response(&b.id, e),
    };
    let author = b.author.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let (item, comment) = match feedback::set_answer(&full_id, author.unwrap_or("owner"), &b.answer)
    {
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

    // F3 — the answer ALWAYS injects into the asking session (PRD §7
    // decision 1), best-effort AFTER the store + emit: a delivery
    // failure never fails the answer. An unnamed answerer is framed
    // with the owner's display name (same server-side resolution as
    // the composer, D3), so the line reads natively in-session.
    let from = author
        .map(String::from)
        .unwrap_or_else(crate::workspace_msg::resolve_owner_from);
    let (delivered, delivery_reason, delivered_session) =
        deliver_to_asker(&item, &from, item.answer.as_deref().unwrap_or_default());

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
/// board's REOPEN (the per-card status dropdown); a manual `answered`
/// is REJECTED loudly — an answered status with a null answer would
/// break the `ask --wait` contract, so `answered` is only reachable
/// through an actual reply (answer route / first human comment).
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ResolveBody {
    id: String,
    status: Option<String>,
}

/// Handler for `POST /cli/feedback/resolve`.
pub fn handle_resolve(body: &[u8]) -> CliResponse {
    let b: ResolveBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.id.is_empty() {
        return usage_error("missing 'id' (a feedback id or unique prefix)");
    }
    let status = b.status.unwrap_or_else(|| "resolved".to_string());
    if !matches!(
        status.as_str(),
        "resolved" | "dismissed" | "waiting" | "planned" | "needs_discussion"
    ) {
        return usage_error(format!(
            "invalid status '{status}' — resolve accepts: resolved, dismissed, planned, needs_discussion, waiting (reopen)"
        ));
    }
    let full_id = match feedback::resolve_id_prefix(&b.id) {
        Ok(f) => f,
        Err(e) => return prefix_error_response(&b.id, e),
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

/// Handler for `POST /cli/feedback/assign`.
pub fn handle_assign(body: &[u8]) -> CliResponse {
    let b: AssignBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return usage_error(format!("invalid JSON body: {e}")),
    };
    if b.id.is_empty() {
        return usage_error("missing 'id' (a ticket id or unique prefix)");
    }
    let full_id = match feedback::resolve_id_prefix(&b.id) {
        Ok(f) => f,
        Err(e) => return prefix_error_response(&b.id, e),
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
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "id": item.id,
                    "assignees": item.assignees,
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

    fn create_via_route(path: &str, title: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut body = serde_json::json!({ "project": path, "title": title });
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
        let resp = dispatch("/cli/feedback/list", &list_params(path, extra))
            .expect("list route claimed");
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
        assert_eq!(item.status, "waiting", "agent comment must not change status");
        assert_eq!(item.comment_count, 2);

        // answer: stores comment + answer + answered_at + status.
        let resp = handle_answer(
            serde_json::json!({ "id": id, "answer": "Yes" }).to_string().as_bytes(),
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
        assert!(!list_ids(&path, &[]).contains(&id), "default hides resolved");
        assert!(list_ids(&path, &[("all", "1")]).contains(&id));
        assert!(list_ids(&path, &[("status", "resolved")]).contains(&id));

        // reopen (per-card status dropdown): waiting rides the resolve
        // route; the answer survives so --wait semantics stay coherent.
        let resp = handle_resolve(
            serde_json::json!({ "id": id, "status": "waiting" }).to_string().as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "reopen failed: {}", resp.body);
        let r: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(r["status"], "waiting");
        assert!(list_ids(&path, &[]).contains(&id), "reopened item lists by default");

        // A manual `answered` is rejected loudly — only an actual reply
        // may answer (a null-answer answered would break --wait).
        let resp = handle_resolve(
            serde_json::json!({ "id": id, "status": "answered" }).to_string().as_bytes(),
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
            serde_json::json!({ "id": "fbamb-o", "status": "dismissed" }).to_string().as_bytes(),
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
            handle_comment(serde_json::json!({ "id": "zzzz", "body": "x" }).to_string().as_bytes()),
            handle_answer(serde_json::json!({ "id": "zzzz", "answer": "x" }).to_string().as_bytes()),
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
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "create failed: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        let assignees = v["assignees"].as_array().expect("assignees array");
        let names: Vec<&str> = assignees.iter().filter_map(|x| x.as_str()).collect();
        assert_eq!(names, vec!["julie", "owner"], "deduped + sorted snapshots: {names:?}");
        let id = v["id"].as_str().expect("id");
        let item = k2_core::feedback::get_item(id).expect("item");
        assert_eq!(item.assignees, vec!["julie".to_string(), "owner".to_string()]);
    }

    /// Validation misses answer 400 with the stable `usage` code and
    /// the mockup's exact hints.
    #[test]
    fn feedback_create_validation() {
        let (name, path) = unique("validate");
        insert_project(&name, &path);

        // No title.
        let resp = handle_create(serde_json::json!({ "project": path }).to_string().as_bytes());
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

    /// F3 — the injection decision matrix (pure classifier): sandbox
    /// rows target their live cell; canonical and sessionless rows fall
    /// back to workspace-agent addressing; a sandbox kind WITHOUT an id
    /// can't address a cell, so it falls back too.
    #[test]
    fn feedback_injection_target_matrix() {
        assert_eq!(
            injection_target(Some("cell-1"), Some("sandbox")),
            InjectionTarget::SandboxSession("cell-1".to_string())
        );
        assert_eq!(
            injection_target(Some("conv-1"), Some("canonical")),
            InjectionTarget::WorkspaceAgent
        );
        assert_eq!(injection_target(None, None), InjectionTarget::WorkspaceAgent);
        assert_eq!(injection_target(Some("conv-1"), None), InjectionTarget::WorkspaceAgent);
        assert_eq!(injection_target(None, Some("sandbox")), InjectionTarget::WorkspaceAgent);

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
                serde_json::json!({ "id": id, "answer": text }).to_string().as_bytes(),
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
        assert_eq!(item.status, "answered", "failed delivery must not lose the answer");
        assert_eq!(item.answer.as_deref(), Some("navy"));

        // (b) Sandbox row whose cell is gone → graceful session_gone
        // skip (no wake, no spawn) — and the answer stores.
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
        assert_eq!(a["deliveryReason"], "session_gone");
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

    /// The comment injection/answer matrix ("it's just a comment
    /// thread; human comments land in the terminal session"):
    ///
    /// | author | item state           | injects | answers |
    /// |--------|----------------------|---------|---------|
    /// | human  | waiting question     | yes     | yes     |
    /// | human  | waiting approval     | yes     | yes     |
    /// | human  | waiting fyi          | yes     | NEVER   |
    /// | human  | answered (follow-up) | yes     | no (answer unchanged) |
    /// | agent  | anything             | no      | no      |
    ///
    /// Delivery is best-effort against a dead project (sandbox cell
    /// gone → session_gone), so `delivered:false` here — the point is
    /// the delivery ATTEMPT is reported and the store never fails.
    #[test]
    fn feedback_comment_matrix_human_injects_and_first_comment_answers() {
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

        // Human first comment on a WAITING question → answers +
        // unblocks --wait (status answered, answer denormalized,
        // delivery attempted with the shared [feedback:] framing).
        let created = create_via_route(&path, "Which color?", sandbox_session());
        let id = created["id"].as_str().expect("id").to_string();
        let c = comment(serde_json::json!({ "id": id, "body": "navy" }));
        assert_eq!(c["author"], "owner", "author defaults to owner (human)");
        assert_eq!(c["answered"], true);
        assert_eq!(c["status"], "answered");
        assert_eq!(c["delivered"], false, "dead cell → attempted, not delivered");
        assert_eq!(c["deliveryReason"], "session_gone");
        assert!(c["commentId"].is_string());
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "answered", "--wait unblocks on this");
        assert_eq!(item.answer.as_deref(), Some("navy"));
        assert_eq!(item.comment_count, 2);

        // Human FOLLOW-UP comment on the now-answered item → injects,
        // but never re-answers (the accepted answer is untouched).
        let c = comment(serde_json::json!({ "id": id, "body": "also check contrast" }));
        assert_eq!(c["answered"], false);
        assert_eq!(c["status"], "answered");
        assert_eq!(c["deliveryReason"], "session_gone", "still injects");
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.answer.as_deref(), Some("navy"), "answer unchanged");
        assert_eq!(item.comment_count, 3);

        // Human comment on a WAITING approval → answers too.
        let created = create_via_route(
            &path,
            "Ship it?",
            serde_json::json!({ "kind": "approval" }),
        );
        let id = created["id"].as_str().expect("id").to_string();
        let c = comment(serde_json::json!({ "id": id, "body": "Ship it" }));
        assert_eq!(c["answered"], true);
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.answer.as_deref(), Some("Ship it"));

        // Human comment on a WAITING fyi → injects but NEVER answers
        // (frozen contract: fyi sits until dismissed/resolved).
        let created = create_via_route(
            &path,
            "Heads up",
            serde_json::json!({ "kind": "fyi", "sessionId": uuid::Uuid::new_v4().to_string(), "sessionKind": "sandbox" }),
        );
        let id = created["id"].as_str().expect("id").to_string();
        let c = comment(serde_json::json!({ "id": id, "body": "noted, thanks" }));
        assert_eq!(c["answered"], false);
        assert_eq!(c["status"], "waiting");
        assert_eq!(c["deliveryReason"], "session_gone", "fyi comment still injects");
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "waiting", "fyi never auto-answers");
        assert!(item.answer.is_none());

        // AGENT comment (author = its name, as the CLI always sends)
        // → store only: no injection, no auto-answer, no delivery
        // fields in the response.
        let created = create_via_route(&path, "agent self-note", sandbox_session());
        let id = created["id"].as_str().expect("id").to_string();
        let c = comment(serde_json::json!({ "id": id, "body": "still thinking", "author": "scout" }));
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
                serde_json::json!({ "id": id, "status": status }).to_string().as_bytes(),
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
        assert!(item.answer.is_none(), "reopen path never fabricates an answer");

        // Manual answered / garbage statuses are loud usage errors.
        for bad in ["answered", "closed", ""] {
            let resp = handle_resolve(
                serde_json::json!({ "id": id, "status": bad }).to_string().as_bytes(),
            );
            assert_eq!(resp.status, "400 Bad Request", "status '{bad}' body={}", resp.body);
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
            serde_json::json!({ "id": id, "body": "navy" })
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

        // Human FIRST comment on the waiting question → answers, so
        // feedback:answered AND feedback:commented — still never
        // feedback:created (comments must not notify).
        let mark = event_mark();
        let resp = handle_comment(
            serde_json::json!({ "id": id, "body": "8080" }).to_string().as_bytes(),
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

        // Human FOLLOW-UP on the answered item → commented only.
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
            vec!["feedback:commented"],
            "follow-up must not re-answer or notify: {events:?}"
        );
        assert_eq!(events[0].1["author"], "owner");
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
            serde_json::json!({ "id": id, "answer": "Ship it" }).to_string().as_bytes(),
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
        let resp = dispatch("/cli/feedback/list", &list_params(&path, &[("status", "bogus")]))
            .expect("claimed");
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);

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
            serde_json::json!({ "id": id, "body": "8080" }).to_string().as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "comment failed: {}", resp.body);
        let resp = handle_resolve(
            serde_json::json!({ "id": id }).to_string().as_bytes(),
        );
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
}
