//! `/cli/publish/*` — daemon-owned published services.
//!
//! GET: list, logs. POST: run/start/stop/rm. Mutating routes are POST-only
//! (GET twins 405). Not under `/cli/tunnel/` (owner-only deny).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::cli::{need_project, opt_param, str_param};
use crate::cli_response::CliResponse;
use crate::publish_runtime::{self, PublishError, RunSpec};

fn resolve_project_id(project: &str) -> Result<String, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT id FROM projects WHERE path = ?1 OR id = ?1",
        rusqlite::params![project],
        |r| r.get(0),
    )
    .map_err(|_| format!("no registered workspace matches {project:?}"))
}

fn resolve_project_path(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT path FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| r.get(0),
    )
    .ok()
}

fn err_to_resp(e: PublishError) -> CliResponse {
    CliResponse {
        status: e.status,
        content_type: "application/json",
        body: serde_json::json!({ "error": e.message }).to_string(),
    }
}

fn boolish(params: &HashMap<String, String>, keys: &[&str]) -> bool {
    for k in keys {
        if let Some(v) = params.get(*k) {
            if matches!(v.as_str(), "1" | "true" | "on" | "yes") {
                return true;
            }
        }
    }
    false
}

fn parse_port(params: &HashMap<String, String>) -> Result<u16, CliResponse> {
    let raw = str_param(params, "port");
    if raw.trim().is_empty() {
        return Err(CliResponse::bad_request("Missing port"));
    }
    raw.parse::<u16>()
        .map_err(|_| CliResponse::bad_request("port must be an integer 1–65535"))
        .and_then(|p| {
            if p == 0 {
                Err(CliResponse::bad_request("Missing port"))
            } else {
                Ok(p)
            }
        })
}

/// GET dispatcher: list + logs + 405 twins for POST-only paths.
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    if !path.starts_with("/cli/publish/") {
        return None;
    }
    Some(match path {
        "/cli/publish/list" => handle_list(params),
        "/cli/publish/logs" => handle_logs(params),
        "/cli/publish/run"
        | "/cli/publish/start"
        | "/cli/publish/stop"
        | "/cli/publish/rm" => CliResponse::method_not_allowed(),
        _ => CliResponse::not_found(),
    })
}

/// POST dispatcher.
pub fn dispatch_post(path: &str, params: &HashMap<String, String>) -> CliResponse {
    match path {
        "/cli/publish/run" => handle_run(params),
        "/cli/publish/start" => handle_start(params),
        "/cli/publish/stop" => handle_stop(params),
        "/cli/publish/rm" => handle_rm(params),
        _ => CliResponse::not_found(),
    }
}

fn handle_list(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    match publish_runtime::list(&project_id) {
        Ok(services) => CliResponse::ok_json(
            serde_json::json!({ "services": services }).to_string(),
        ),
        Err(e) => err_to_resp(e),
    }
}

fn handle_logs(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let name = str_param(params, "name");
    if name.trim().is_empty() {
        return CliResponse::bad_request("Missing name");
    }
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    let mut n: usize = 200;
    if let Some(lines) = opt_param(params, "lines") {
        if let Ok(v) = lines.parse::<usize>() {
            n = v.max(1);
        }
    }
    if let Some(tail) = opt_param(params, "tail") {
        if let Ok(v) = tail.parse::<usize>() {
            n = v.max(1);
        }
    }
    let text = publish_runtime::read_log_tail(&project_id, &name, n);
    CliResponse::ok_json(
        serde_json::json!({
            "name": name,
            "text": text,
        })
        .to_string(),
    )
}

fn handle_run(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    let name = str_param(params, "name");
    if name.trim().is_empty() {
        return CliResponse::bad_request("Missing name");
    }
    let cmd = str_param(params, "cmd");
    let port = match parse_port(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let no_tunnel = boolish(params, &["noTunnel", "no_tunnel"]);
    let cwd = opt_param(params, "cwd")
        .map(PathBuf::from)
        .or_else(|| resolve_project_path(&project_id).map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(&project));
    match publish_runtime::run(RunSpec {
        project_id,
        name,
        cmd,
        cwd,
        port,
        no_tunnel,
        replace_spec: true,
    }) {
        Ok(svc) => CliResponse::ok_json(serde_json::to_string(&svc).unwrap_or_else(|_| "{}".into())),
        Err(e) => err_to_resp(e),
    }
}

fn handle_start(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let name = str_param(params, "name");
    if name.trim().is_empty() {
        return CliResponse::bad_request("Missing name");
    }
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    match publish_runtime::start(&project_id, &name) {
        Ok(svc) => CliResponse::ok_json(serde_json::to_string(&svc).unwrap_or_else(|_| "{}".into())),
        Err(e) => err_to_resp(e),
    }
}

fn handle_stop(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let name = str_param(params, "name");
    if name.trim().is_empty() {
        return CliResponse::bad_request("Missing name");
    }
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    match publish_runtime::stop(&project_id, &name) {
        Ok(svc) => CliResponse::ok_json(serde_json::to_string(&svc).unwrap_or_else(|_| "{}".into())),
        Err(e) => err_to_resp(e),
    }
}

fn handle_rm(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let name = str_param(params, "name");
    if name.trim().is_empty() {
        return CliResponse::bad_request("Missing name");
    }
    let keep = boolish(params, &["keepHostname", "keep_hostname"]);
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    match publish_runtime::rm(&project_id, &name, keep) {
        Ok(()) => CliResponse::ok_json(serde_json::json!({ "ok": true, "name": name }).to_string()),
        Err(e) => err_to_resp(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_chain_405s_mutating_publish_routes() {
        let p = HashMap::new();
        for path in [
            "/cli/publish/run",
            "/cli/publish/start",
            "/cli/publish/stop",
            "/cli/publish/rm",
        ] {
            let resp = dispatch(path, &p).expect("GET twin must exist");
            assert_eq!(resp.status, "405 Method Not Allowed", "path={path}");
        }
    }

    #[test]
    fn get_list_requires_project() {
        let resp = dispatch("/cli/publish/list", &HashMap::new()).unwrap();
        assert_eq!(resp.status, "400 Bad Request");
    }

    #[test]
    fn unknown_publish_path_is_404() {
        let resp = dispatch("/cli/publish/nope", &HashMap::new()).unwrap();
        assert_eq!(resp.status, "404 Not Found");
    }
}
