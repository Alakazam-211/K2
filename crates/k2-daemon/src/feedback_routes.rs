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
//! Events: `create` fires `HookEvent::FeedbackCreated`
//! (`{id, projectPath, title, kind, priority, agentName}`) and
//! `answer` fires `HookEvent::FeedbackAnswered` (`{id, projectPath}`)
//! on the existing `/events` WireEvent broadcast.
//!
//! F1 boundary: `answer` ONLY stores (thread comment + denormalized
//! answer + status). The deliver-into-session injection
//! (`deliver_live`, PRD §4.3 answer flow) is F3, not here.

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
        // Default shows open items (waiting + answered), newest first.
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
        | "/cli/feedback/resolve" => CliResponse::method_not_allowed(),

        _ => return None,
    };
    Some(resp)
}

/// Dispatch a `/cli/feedback/*` POST body to its handler. Exact-match
/// paths; unknown paths 404 (mirrors `dispatch_unit6_post`).
pub fn dispatch_post(path: &str, body: &[u8]) -> CliResponse {
    match path {
        "/cli/feedback/create" => handle_create(body),
        "/cli/feedback/comment" => handle_comment(body),
        "/cli/feedback/answer" => handle_answer(body),
        "/cli/feedback/resolve" => handle_resolve(body),
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

    let item = match feedback::create(feedback::NewFeedback {
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

    let (name, _) = project_name_path(&item.project_id);
    let mut v = serde_json::to_value(&item).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(map) = v.as_object_mut() {
        map.insert("ok".to_string(), serde_json::json!(true));
        map.insert("workspace".to_string(), serde_json::json!(name));
    }
    CliResponse::ok_json(v.to_string())
}

/// `POST /cli/feedback/comment` body. `author` defaults to `owner`
/// (the renderer's thread panel); agents pass their own name via the
/// CLI. Comments bump the thread ONLY — no notification machinery
/// fires in F1 (mockup: "only NEW items fire notifications").
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct CommentBody {
    id: String,
    body: String,
    author: Option<String>,
}

/// Handler for `POST /cli/feedback/comment`.
pub fn handle_comment(body: &[u8]) -> CliResponse {
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
    let author = b.author.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("owner");
    match feedback::add_comment(&full_id, author, &b.body) {
        Ok(c) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "id": full_id,
                "commentId": c.id,
                "author": c.author,
            })
            .to_string(),
        ),
        Err(e) => usage_error(e),
    }
}

/// `POST /cli/feedback/answer` body. F1 ONLY STORES: thread comment +
/// denormalized `answer` + `answered_at` + status `answered`, then
/// fires `FeedbackAnswered`. The deliver-into-session injection is F3.
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
    let author = b.author.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("owner");
    let item = match feedback::set_answer(&full_id, author, &b.answer) {
        Ok(item) => item,
        Err(e) => return usage_error(e),
    };

    // FeedbackAnswered on the /events broadcast ({id, projectPath}).
    let (_, path) = project_name_path(&item.project_id);
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::FeedbackAnswered,
        serde_json::json!({
            "id": item.id,
            "projectPath": path,
        }),
    );

    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "id": item.id,
            "status": item.status,
            "answer": item.answer,
            "answeredAt": item.answered_at,
        })
        .to_string(),
    )
}

/// `POST /cli/feedback/resolve` body. `status` defaults to `resolved`;
/// `dismissed` rides the same route (one mutation, two terminal
/// states) — matching the mockup surface, where the CLI only exposes
/// `resolve` and dismiss is the human's board action.
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
    if !matches!(status.as_str(), "resolved" | "dismissed") {
        return usage_error(format!(
            "invalid status '{status}' — resolve accepts: resolved, dismissed"
        ));
    }
    let full_id = match feedback::resolve_id_prefix(&b.id) {
        Ok(f) => f,
        Err(e) => return prefix_error_response(&b.id, e),
    };
    match feedback::set_status(&full_id, &status) {
        Ok(item) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "id": item.id,
                "status": item.status,
            })
            .to_string(),
        ),
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

        // comment bumps the thread ONLY (status stays waiting).
        let resp = handle_comment(
            serde_json::json!({ "id": id, "body": "hold until CI passes" }).to_string().as_bytes(),
        );
        assert_eq!(resp.status, "200 OK", "comment failed: {}", resp.body);
        let c: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(c["author"], "owner", "author defaults to owner");
        let item = k2_core::feedback::get_item(&id).expect("item");
        assert_eq!(item.status, "waiting", "comment must not change status");
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
}
