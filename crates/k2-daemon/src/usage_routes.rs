//! Usage v2 routes (prd-usage-v2 §1–§3):
//!
//! - `GET /cli/usage/tokens` — the token ledger report (filters, `since` /
//!   `until`, `tz`, `group_by`, cost).
//! - `GET /cli/usage/turns` — the activity log, newest by time, cursors.
//! - `GET /cli/usage/limits` — every login's subscription limits.
//! - `POST /cli/usage/limits/refresh` — re-check limits (single-flight).
//!
//! **Who (Rosson Q3, 2026-10-16):** the owner token and Owner / Admin
//! logins see everything; Members are refused (route floor `admin` plus
//! this handler). An agent passport (harness session or K2 shell tab, TCP
//! or the cell socket) reads its **own workspace's** tokens and turns and
//! the limits, read only; it may not refresh. App passes and API keys are
//! refused. The TCP dispatcher and the cell socket both build a
//! [`Who`] and call [`handle`].

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::token_usage::{self as ledger, Filter, GroupBy, ReportOpts, TurnCursor};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

pub const TOKENS: &str = "/cli/usage/tokens";
pub const TURNS: &str = "/cli/usage/turns";
pub const LIMITS: &str = "/cli/usage/limits";
pub const LIMITS_REFRESH: &str = "/cli/usage/limits/refresh";

/// Every route this module answers.
pub const ROUTES: &[&str] = &[TOKENS, TURNS, LIMITS, LIMITS_REFRESH];

/// Largest refresh body.
pub const MAX_BODY: usize = 16 * 1024;

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    p == LIMITS_REFRESH
}

fn refuse(status: &'static str, code: &str, error: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "error": error, "code": code }).to_string(),
    }
}

fn forbidden(code: &str, error: &str) -> CliResponse {
    refuse("403 Forbidden", code, error)
}

/// What a caller may see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Owner token, Owner / Admin login.
    Full,
    /// An agent passport: this workspace's ledger rows, limits read only.
    Workspace(String),
}

fn project_path(project_id: &str) -> Option<String> {
    let db = k2_core::db::try_shared()?;
    let conn = db.lock();
    k2_core::db::schema::Project::get(&conn, project_id).ok().map(|p| p.path).filter(|p| !p.is_empty())
}

/// The gate (Q3). `Err` is the refusal to send.
pub fn access(who: &Who, path: &str) -> Result<Scope, CliResponse> {
    match who {
        Who::Owner => Ok(Scope::Full),
        Who::Login { role, .. } if *role >= Role::Admin => Ok(Scope::Full),
        Who::Login { .. } => Err(forbidden("admin_only", "Usage is visible to the server Owner and Admins")),
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => {
            if path == LIMITS_REFRESH {
                return Err(forbidden(
                    "owner_only",
                    "an agent reads limits but can't start a check; run `k2 usage limits --refresh` in a terminal outside K2",
                ));
            }
            match project_path(project_id) {
                Some(p) => Ok(Scope::Workspace(p)),
                None => Err(forbidden("own_workspace_only", "this session's workspace is not known")),
            }
        }
        Who::Wrong => Err(forbidden("wrong_credential", "an app pass or API key can't read usage")),
        Who::Invalid => Err(forbidden("auth_required", "invalid or missing token")),
    }
}

/// Route one usage request (blocking: sqlite, probes).
pub fn handle(path: &str, who: &Who, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    if is_post_route(path) && !is_post {
        return refuse("405 Method Not Allowed", "post_required", "POST required");
    }
    let scope = match access(who, path) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match path {
        TOKENS => handle_tokens(params, &scope),
        TURNS => handle_turns(params, &scope),
        LIMITS => ok(crate::usage_live::limits_doc(chrono::Utc::now(), true)),
        LIMITS_REFRESH => handle_refresh(body, params),
        _ => refuse("404 Not Found", "not_found", "no such usage route"),
    }
}

/// `crate::cli::dispatch` fallback: the token is the `token` param.
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    if !is_route(path) {
        return None;
    }
    let token = params.get("token").map(String::as_str).unwrap_or("");
    let owner = k2_core::hook_config::get_token();
    let who = crate::compute_routes::who_from_tcp(&format!("token={token}"), None, owner);
    Some(handle(path, &who, false, &[], params))
}

fn ok(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

fn json_or_500<T: serde::Serialize>(v: &T) -> CliResponse {
    match serde_json::to_string(v) {
        Ok(body) => CliResponse::ok_json(body),
        Err(e) => CliResponse::internal_error(format!("usage json: {e}")),
    }
}

fn opt<'a>(params: &'a HashMap<String, String>, key: &str) -> Result<Option<&'a str>, CliResponse> {
    match params.get(key) {
        None => Ok(None),
        Some(s) if s.trim().is_empty() => Err(CliResponse::bad_request(format!("{key} must not be empty"))),
        Some(s) => Ok(Some(s.trim())),
    }
}

/// The shared filter params (`workspace`, `outside`, `harness`,
/// `account`, `model`, `since`, `until`), scoped for an agent.
fn filter_from(params: &HashMap<String, String>, scope: &Scope) -> Result<Filter, CliResponse> {
    let mut f = Filter { workspace: opt(params, "workspace")?.map(str::to_string), ..Default::default() };
    f.outside_only = match params.get("outside").map(String::as_str) {
        None => false,
        Some("1") => true,
        Some(_) => return Err(CliResponse::bad_request("outside must be 1")),
    };
    if f.outside_only && f.workspace.is_some() {
        return Err(CliResponse::bad_request("workspace and outside cannot both be set"));
    }
    f.harness = match opt(params, "harness")? {
        None => None,
        Some(h) if matches!(h, "claude" | "codex" | "grok") => Some(h.to_string()),
        Some(_) => return Err(CliResponse::bad_request("harness must be claude, codex, or grok")),
    };
    f.account = opt(params, "account")?.map(str::to_string);
    f.model = opt(params, "model")?.map(str::to_string);
    let now = chrono::Utc::now().timestamp_millis();
    for (key, slot) in [("since", &mut f.since_ms), ("until", &mut f.until_ms)] {
        if let Some(raw) = opt(params, key)? {
            *slot = Some(ledger::parse_time_bound(raw, now).map_err(|e| CliResponse::bad_request(format!("{key}: {e}")))?);
        }
    }
    if let Scope::Workspace(own) = scope {
        let own_c = ledger::canonical_workspace(own);
        if f.outside_only {
            return Err(forbidden("own_workspace_only", "an agent sees its own workspace's usage only"));
        }
        if let Some(asked) = &f.workspace {
            if ledger::canonical_workspace(asked) != own_c {
                return Err(forbidden("own_workspace_only", "an agent sees its own workspace's usage only"));
            }
        }
        f.workspace = Some(own_c);
    }
    Ok(f)
}

pub(crate) fn handle_tokens(params: &HashMap<String, String>, scope: &Scope) -> CliResponse {
    let filter = match filter_from(params, scope) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let mut opts = ReportOpts::default();
    if let Some(tz) = params.get("tz") {
        match ledger::parse_tz(tz) {
            Ok(z) => opts.tz = z,
            Err(e) => return CliResponse::bad_request(e),
        }
    }
    opts.group_by = match params.get("group_by").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        None => None,
        Some(g) => match GroupBy::parse(g) {
            Some(g) => Some(g),
            None => return CliResponse::bad_request("group_by must be workspace, model, login, or harness"),
        },
    };
    opts.cost = params.get("cost").map(String::as_str) != Some("0");
    let known = match known_workspace_paths() {
        Ok(v) => v,
        Err(e) => return CliResponse::internal_error(e),
    };
    match ledger::report_host(&filter, &opts, &known) {
        Ok(report) => json_or_500(&report),
        Err(e) => CliResponse::internal_error(e),
    }
}

pub(crate) fn handle_turns(params: &HashMap<String, String>, scope: &Scope) -> CliResponse {
    let filter = match filter_from(params, scope) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let cursor = |key: &str| -> Result<Option<TurnCursor>, CliResponse> {
        match params.get(key) {
            None => Ok(None),
            Some(s) => TurnCursor::parse(s)
                .map(Some)
                .ok_or_else(|| CliResponse::bad_request(format!("{key} must be a cursor from an earlier page"))),
        }
    };
    let before = match cursor("before") {
        Ok(c) => c,
        Err(r) => return r,
    };
    let after = match cursor("after") {
        Ok(c) => c,
        Err(r) => return r,
    };
    let limit = match params.get("limit") {
        None => ledger::DEFAULT_TURNS_LIMIT,
        Some(s) => match s.parse::<i64>() {
            Ok(n) => n,
            Err(_) => return CliResponse::bad_request("limit must be an integer"),
        },
    };
    let known = match known_workspace_paths() {
        Ok(v) => v,
        Err(e) => return CliResponse::internal_error(e),
    };
    match ledger::turns_host(&filter, &known, before, after, limit) {
        Ok(page) => json_or_500(&page),
        Err(e) => CliResponse::internal_error(e),
    }
}

fn handle_refresh(body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    let parsed: Value = if body.iter().any(|b| !b.is_ascii_whitespace()) {
        match serde_json::from_slice(body) {
            Ok(v) => v,
            Err(_) => {
                // A form body (`tool=claude&accountId=…`).
                let form: HashMap<String, String> = crate::routes::http::parse_form_body(body).into_iter().collect();
                json!(form)
            }
        }
    } else {
        json!({})
    };
    let pick = |k: &str| parsed.get(k).and_then(|v| v.as_str()).or_else(|| params.get(k).map(String::as_str)).map(str::trim);
    let account = pick("accountId").or_else(|| pick("account"));
    match crate::usage_live::refresh(pick("tool"), account) {
        Ok(doc) => ok(doc),
        Err((400, e)) => CliResponse::bad_request(e),
        Err((404, e)) => refuse("404 Not Found", "not_found", &e),
        Err((_, e)) => CliResponse::internal_error(e),
    }
}

fn known_workspace_paths() -> Result<Vec<String>, String> {
    let mut paths = k2_core::projects_ops::projects_list()?
        .into_iter()
        .map(|p| p.path)
        .filter(|p| !ledger::is_sentinel_workspace(p))
        .collect::<Vec<_>>();
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT worktree_path FROM workspaces \
             WHERE worktree_path IS NOT NULL AND worktree_path != ''",
        )
        .map_err(|e| format!("worktree paths: {e}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("worktree paths: {e}"))?;
    for row in rows {
        let path = row.map_err(|e| format!("worktree path: {e}"))?;
        if !ledger::is_sentinel_workspace(&path) {
            paths.push(path);
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn status(r: &CliResponse) -> &str {
        r.status
    }

    fn body(r: &CliResponse) -> Value {
        serde_json::from_str(&r.body).expect("json body")
    }

    fn okv<T>(r: Result<T, CliResponse>, what: &str) -> T {
        match r {
            Ok(v) => v,
            Err(e) => panic!("{what}: {} {}", e.status, e.body),
        }
    }

    fn errv<T>(r: Result<T, CliResponse>, what: &str) -> CliResponse {
        match r {
            Ok(_) => panic!("{what}: expected a refusal"),
            Err(e) => e,
        }
    }

    #[test]
    fn usage_routes_are_not_404_and_refuse_without_a_token() {
        for p in [TOKENS, TURNS, LIMITS] {
            let resp = crate::cli::dispatch(p, &HashMap::new());
            assert_eq!(status(&resp), "403 Forbidden", "{p}");
            assert_eq!(body(&resp)["code"], json!("auth_required"));
        }
    }

    #[test]
    fn roles_owner_and_admin_pass_member_and_wrong_credentials_fail() {
        let login = |role| Who::Login { username: "u".into(), role };
        for p in ROUTES {
            assert_eq!(okv(access(&Who::Owner, p), "owner"), Scope::Full);
            assert_eq!(okv(access(&login(Role::Owner), p), "owner login"), Scope::Full);
            assert_eq!(okv(access(&login(Role::Admin), p), "admin login"), Scope::Full, "Q3: Admins see usage");
            let m = errv(access(&login(Role::Member), p), "member");
            assert_eq!(status(&m), "403 Forbidden");
            assert_eq!(body(&m)["code"], json!("admin_only"));
            assert_eq!(body(&errv(access(&Who::Wrong, p), "wrong"))["code"], json!("wrong_credential"));
            assert_eq!(body(&errv(access(&Who::Invalid, p), "invalid"))["code"], json!("auth_required"));
        }
    }

    #[test]
    fn member_connect_session_is_refused_end_to_end() {
        crate::test_support::with_temp_home(|| {
            k2_core::connect_users::add_user("usage_member", "password123").expect("add member");
            let member = k2_core::connect_users::create_session("usage_member");
            let who = crate::compute_routes::who_from_tcp(&format!("token={member}"), None, "owner-token-xyz");
            let r = handle(TOKENS, &who, false, &[], &HashMap::new());
            assert_eq!(status(&r), "403 Forbidden");
            k2_core::connect_users::add_user("usage_admin", "password123").expect("add admin");
            k2_core::connect_users::set_role("usage_admin", Role::Admin).expect("admin");
            let admin = k2_core::connect_users::create_session("usage_admin");
            let who = crate::compute_routes::who_from_tcp(&format!("token={admin}"), None, "owner-token-xyz");
            assert_eq!(okv(access(&who, TOKENS), "admin passes"), Scope::Full);
        });
    }

    #[test]
    fn an_agent_is_scoped_to_its_own_workspace_and_cannot_refresh() {
        let own = std::env::temp_dir().join(format!("k2-usage-agent-{}", std::process::id()));
        std::fs::create_dir_all(&own).expect("ws");
        let own_s = own.to_string_lossy().into_owned();
        let scope = Scope::Workspace(own_s.clone());
        let mut params = HashMap::new();
        let f = okv(filter_from(&params, &scope), "own");
        assert_eq!(f.workspace.as_deref(), Some(ledger::canonical_workspace(&own_s).as_str()), "forced to its workspace");
        params.insert("workspace".to_string(), "/somewhere/else".to_string());
        let r = errv(filter_from(&params, &scope), "another workspace");
        assert_eq!(body(&r)["code"], json!("own_workspace_only"));
        let mut params = HashMap::new();
        params.insert("outside".to_string(), "1".to_string());
        assert_eq!(body(&errv(filter_from(&params, &scope), "outside"))["code"], json!("own_workspace_only"));
        let agent = Who::Agent { project_id: "p".into(), session_id: "s".into() };
        let r = errv(access(&agent, LIMITS_REFRESH), "agents don't refresh");
        assert_eq!(body(&r)["code"], json!("owner_only"));
        let _ = std::fs::remove_dir_all(&own);
    }

    #[test]
    fn bad_params_are_400() {
        let full = Scope::Full;
        let p = |k: &str, v: &str| HashMap::from([(k.to_string(), v.to_string())]);
        assert_eq!(status(&errv(filter_from(&p("workspace", ""), &full), "empty ws")), "400 Bad Request");
        assert_eq!(status(&errv(filter_from(&p("harness", "gemini"), &full), "harness")), "400 Bad Request");
        assert_eq!(status(&errv(filter_from(&p("since", "soon"), &full), "since")), "400 Bad Request");
        assert_eq!(status(&errv(filter_from(&p("outside", "yes"), &full), "outside")), "400 Bad Request");
        let both = HashMap::from([("outside".to_string(), "1".to_string()), ("workspace".to_string(), "/x".to_string())]);
        assert_eq!(status(&errv(filter_from(&both, &full), "both")), "400 Bad Request");
        assert_eq!(status(&handle_tokens(&p("tz", "Mars/Olympus"), &full)), "400 Bad Request");
        assert_eq!(status(&handle_tokens(&p("group_by", "color"), &full)), "400 Bad Request");
        assert_eq!(status(&handle_turns(&p("before", "x:y"), &full)), "400 Bad Request");
        let f = okv(filter_from(&p("since", "7d"), &full), "7d");
        assert!(f.since_ms.is_some());
    }

    #[test]
    fn refresh_is_post_only() {
        let r = handle(LIMITS_REFRESH, &Who::Owner, false, &[], &HashMap::new());
        assert_eq!(status(&r), "405 Method Not Allowed");
    }
}
