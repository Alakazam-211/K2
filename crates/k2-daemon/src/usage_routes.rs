//! `GET /cli/usage/tokens` — machine total, or one workspace path.
//! `GET /cli/usage/turns` — the raw per-turn ledger log, newest first,
//! keyset-paged for the Settings → Token usage live log.
//!
//! Arms of `cli::dispatch`. Not in `post_allowed` (a POST is 405 at the
//! dispatcher). The `/cli/` catchall only checks `token_ok`, which would
//! let a Member read every workspace, so these arms gate again with
//! [`crate::routes::http::owner_role_identity`].

use std::collections::HashMap;

use crate::cli_response::CliResponse;

/// Owner token, or a connect user whose role is Owner. Admin, Member,
/// and Viewer fail. Skin tokens are not sessions and are not the owner
/// token, so they fail too.
pub fn usage_allowed(query: &str, owner_token: &str) -> bool {
    crate::routes::http::owner_role_identity(query, owner_token).is_some()
}

pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    if path != "/cli/usage/tokens" && path != "/cli/usage/turns" {
        return None;
    }
    let token = params.get("token").map(String::as_str).unwrap_or("");
    let owner = k2_core::hook_config::get_token();
    if !usage_allowed(&format!("token={token}"), owner) {
        return Some(CliResponse::forbidden());
    }
    if path == "/cli/usage/turns" {
        return Some(handle_turns(params));
    }
    Some(handle_tokens(params))
}

pub(crate) fn handle_tokens(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match params.get("workspace") {
        None => None,
        Some(s) if s.is_empty() => {
            return CliResponse::bad_request("workspace must not be empty");
        }
        Some(s) => Some(s.as_str()),
    };
    // `project` is not the parameter. Ignore it.
    let known = match known_workspace_paths() {
        Ok(v) => v,
        Err(e) => return CliResponse::internal_error(e),
    };
    let harness = match params.get("harness") {
        None => None,
        Some(s) if s.is_empty() => {
            return CliResponse::bad_request("harness must not be empty");
        }
        Some(s) if matches!(s.as_str(), "claude" | "codex" | "grok") => Some(s.as_str()),
        Some(_) => {
            return CliResponse::bad_request("harness must be claude, codex, or grok");
        }
    };
    let outside_only = match params.get("outside") {
        None => false,
        Some(s) if s == "1" => true,
        Some(_) => return CliResponse::bad_request("outside must be 1"),
    };
    if outside_only && workspace.is_some() {
        return CliResponse::bad_request("workspace and outside cannot both be set");
    }
    match k2_core::token_usage::query_host(workspace, harness, outside_only, &known) {
        Ok(report) => match serde_json::to_string(&report) {
            Ok(body) => CliResponse::ok_json(body),
            Err(e) => CliResponse::internal_error(format!("token usage json: {e}")),
        },
        Err(e) => CliResponse::internal_error(e),
    }
}

pub(crate) fn handle_turns(params: &HashMap<String, String>) -> CliResponse {
    // Optional workspace filter. An explicit empty string is a client
    // bug, not "machine-wide" — reject it (parity with handle_tokens).
    // Absent = machine-wide (every workspace), which is the log default.
    let workspace = match params.get("workspace") {
        None => None,
        Some(s) if s.is_empty() => {
            return CliResponse::bad_request("workspace must not be empty");
        }
        Some(s) => Some(s.as_str()),
    };
    let before = match params.get("before") {
        None => None,
        Some(s) => match s.parse::<i64>() {
            Ok(n) => Some(n),
            Err(_) => return CliResponse::bad_request("before must be an integer rowid"),
        },
    };
    let limit = match params.get("limit") {
        None => k2_core::token_usage::DEFAULT_TURNS_LIMIT,
        Some(s) => match s.parse::<i64>() {
            Ok(n) => n,
            Err(_) => return CliResponse::bad_request("limit must be an integer"),
        },
    };
    let known = match known_workspace_paths() {
        Ok(v) => v,
        Err(e) => return CliResponse::internal_error(e),
    };
    match k2_core::token_usage::query_turns_host(workspace, &known, before, limit) {
        Ok(page) => match serde_json::to_string(&page) {
            Ok(body) => CliResponse::ok_json(body),
            Err(e) => CliResponse::internal_error(format!("token turns json: {e}")),
        },
        Err(e) => CliResponse::internal_error(e),
    }
}

fn known_workspace_paths() -> Result<Vec<String>, String> {
    let mut paths = k2_core::projects_ops::projects_list()?
        .into_iter()
        .map(|p| p.path)
        .filter(|p| !k2_core::token_usage::is_sentinel_workspace(p))
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
        if !k2_core::token_usage::is_sentinel_workspace(&path) {
            paths.push(path);
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn get_usage_tokens_is_not_404() {
        let resp = crate::cli::dispatch("/cli/usage/tokens", &HashMap::new());
        assert_ne!(resp.status, "404 Not Found");
        assert_eq!(resp.status, "403 Forbidden");
    }

    #[test]
    fn empty_workspace_query_is_400() {
        let mut params = HashMap::new();
        params.insert("workspace".into(), String::new());
        let resp = handle_tokens(&params);
        assert_eq!(resp.status, "400 Bad Request");
        let body: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        let err = body.get("error").and_then(|v| v.as_str()).expect("error");
        assert_eq!(err, "workspace must not be empty");
    }

    #[test]
    fn member_connect_token_is_forbidden_owner_role_is_not() {
        // The HTTP harness's owner token is not installed in hook_config
        // here, so this calls the gate directly (the same function the
        // route uses) instead of a live listener.
        crate::test_support::with_temp_home(|| {
            k2_core::connect_users::add_user("token_usage_member", "password123")
                .expect("add member");
            let member = k2_core::connect_users::create_session("token_usage_member");
            assert_ne!(member.as_str(), "owner-token-xyz");
            assert!(
                !usage_allowed(&format!("token={member}"), "owner-token-xyz"),
                "a Member session must not read the machine ledger"
            );

            k2_core::connect_users::add_user("token_usage_owner_role", "password123")
                .expect("add owner role");
            k2_core::connect_users::set_role(
                "token_usage_owner_role",
                k2_core::connect_users::Role::Owner,
            )
            .expect("set owner role");
            let owner_session = k2_core::connect_users::create_session("token_usage_owner_role");
            assert!(
                usage_allowed(&format!("token={owner_session}"), "owner-token-xyz"),
                "an Owner-role connect session must pass"
            );

            k2_core::connect_users::add_user("token_usage_admin", "password123")
                .expect("add admin");
            k2_core::connect_users::set_role(
                "token_usage_admin",
                k2_core::connect_users::Role::Admin,
            )
            .expect("set admin");
            let admin = k2_core::connect_users::create_session("token_usage_admin");
            assert!(
                !usage_allowed(&format!("token={admin}"), "owner-token-xyz"),
                "an Admin session must not pass"
            );

            assert!(usage_allowed("token=owner-token-xyz", "owner-token-xyz"));
            assert!(!usage_allowed("token=", "owner-token-xyz"));
            assert!(!usage_allowed("workspace=/tmp", "owner-token-xyz"));
        });
    }
}
