//! `/cli/workspace/resources*` — daemon-owned workspace resource list.
//!
//! GET list; POST add/remove. Mutating routes are POST-only (GET twins 405).
//! `workspace` is name | path | UUID via [`crate::workspace_msg::resolve_workspace`].

use std::collections::HashMap;
use std::path::Path;

use k2_core::workspace_resources::{self, ResourceError};

use crate::cli::str_param;
use crate::cli_response::CliResponse;
use crate::workspace_routes::workspace_not_found_response;

fn error_response(status: &'static str, code: &str, hint: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint.to_string() },
        })
        .to_string(),
    }
}

fn resource_error(e: ResourceError) -> CliResponse {
    match &e {
        ResourceError::NotFound => error_response("404 Not Found", e.code(), e.to_string()),
        ResourceError::Db(_) => CliResponse::internal_error(e.to_string()),
        ResourceError::PathEscape(_) | ResourceError::NotAFile(_) => {
            error_response("400 Bad Request", e.code(), e.to_string())
        }
    }
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.to_string())
}

fn emit_changed(workspace_id: &str) {
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::WorkspaceResourcesChanged {
        workspace_id: workspace_id.to_string(),
    });
}

fn resolve_project_id(token: &str) -> Result<(String, String), CliResponse> {
    let Some(path) = crate::workspace_msg::resolve_workspace(token) else {
        return Err(workspace_not_found_response(token));
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Some(id) = k2_core::workspace::agent_identity::resolve_project_id(&conn, &path) else {
        return Err(workspace_not_found_response(token));
    };
    Ok((id, path))
}

fn need_workspace(params: &HashMap<String, String>) -> Result<String, CliResponse> {
    for key in ["workspace", "project", "project_path"] {
        let v = str_param(params, key);
        if !v.is_empty() {
            return Ok(v);
        }
    }
    Err(error_response(
        "400 Bad Request",
        "usage",
        "missing workspace (name | path | UUID)",
    ))
}

fn need_path(params: &HashMap<String, String>) -> Result<String, CliResponse> {
    let v = str_param(params, "path");
    if v.is_empty() {
        return Err(error_response(
            "400 Bad Request",
            "usage",
            "missing path (absolute file path)",
        ));
    }
    Ok(v)
}

pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    let resp = match path {
        "/cli/workspace/resources" => handle_list(params),
        "/cli/workspace/resources/add" | "/cli/workspace/resources/remove" => {
            CliResponse::method_not_allowed()
        }
        _ => return None,
    };
    Some(resp)
}

pub fn dispatch_post(path: &str, params: &HashMap<String, String>) -> CliResponse {
    match path {
        "/cli/workspace/resources/add" => handle_add(params),
        "/cli/workspace/resources/remove" => handle_remove(params),
        _ => CliResponse::not_found(),
    }
}

fn handle_list(params: &HashMap<String, String>) -> CliResponse {
    let token = match need_workspace(params) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let (workspace_id, _) = match resolve_project_id(&token) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match workspace_resources::list(&conn, &workspace_id) {
            Ok(r) => r,
            Err(e) => return resource_error(e),
        }
    };
    let docs: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "filePath": r.file_path,
                "fileName": file_name(&r.file_path),
                "addedAt": r.added_at,
                "missing": workspace_resources::file_missing(&r.file_path),
            })
        })
        .collect();
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "workspaceId": workspace_id,
            "docs": docs,
        })
        .to_string(),
    )
}

fn handle_add(params: &HashMap<String, String>) -> CliResponse {
    let token = match need_workspace(params) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let path = match need_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let (workspace_id, _) = match resolve_project_id(&token) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let stored = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match workspace_resources::add(&conn, &workspace_id, &path) {
            Ok(s) => s,
            Err(e) => return resource_error(e),
        }
    };
    emit_changed(&workspace_id);
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "workspaceId": workspace_id,
            "filePath": stored,
            "fileName": file_name(&stored),
        })
        .to_string(),
    )
}

fn handle_remove(params: &HashMap<String, String>) -> CliResponse {
    let token = match need_workspace(params) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let path = match need_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let (workspace_id, _) = match resolve_project_id(&token) {
        Ok(v) => v,
        Err(r) => return r,
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(e) = workspace_resources::remove(&conn, &workspace_id, &path) {
            return resource_error(e);
        }
    }
    emit_changed(&workspace_id);
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "workspaceId": workspace_id,
            "filePath": path,
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn unique_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2-wsres-route-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn touch(path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        let mut f = fs::File::create(path).expect("create file");
        f.write_all(b"x").ok();
    }

    fn insert_workspace(label: &str, path: &str) -> (String, String) {
        let id = uuid::Uuid::new_v4().to_string();
        let name = format!("wsres-{label}-{id}");
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, path],
        )
        .expect("insert project");
        (id, name)
    }

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn ok_json(resp: CliResponse) -> serde_json::Value {
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        serde_json::from_str(&resp.body).expect("valid JSON")
    }

    fn count_rows(workspace_id: &str) -> i64 {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM workspace_resources WHERE workspace_id = ?1",
            rusqlite::params![workspace_id],
            |r| r.get(0),
        )
        .expect("count")
    }

    #[test]
    fn add_twice_one_row() {
        let dir = unique_dir("dup");
        let file = dir.join("a.csv");
        touch(&file);
        let (id, name) = insert_workspace("dup", dir.to_str().unwrap());
        let p = params(&[
            ("workspace", name.as_str()),
            ("path", file.to_str().unwrap()),
        ]);
        let _ = ok_json(dispatch_post("/cli/workspace/resources/add", &p));
        let _ = ok_json(dispatch_post("/cli/workspace/resources/add", &p));
        assert_eq!(count_rows(&id), 1);
        let listed = ok_json(
            dispatch(
                "/cli/workspace/resources",
                &params(&[("workspace", id.as_str())]),
            )
            .expect("list claimed"),
        );
        assert_eq!(listed["docs"].as_array().expect("docs").len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_outside_tree_is_400() {
        let dir = unique_dir("in");
        let sibling = dir.parent().unwrap().join(format!(
            "k2-wsres-route-escape-{}",
            uuid::Uuid::new_v4()
        ));
        touch(&sibling);
        let via = dir.join("..").join(sibling.file_name().unwrap());
        let (id, name) = insert_workspace("esc", dir.to_str().unwrap());
        let resp = dispatch_post(
            "/cli/workspace/resources/add",
            &params(&[
                ("workspace", name.as_str()),
                ("path", via.to_str().unwrap()),
            ]),
        );
        assert_eq!(resp.status, "400 Bad Request", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["error"]["code"], "path_escape");
        assert_eq!(count_rows(&id), 0);
        let _ = fs::remove_file(&sibling);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn worktree_file_not_under_projects_path_is_200() {
        let main = unique_dir("main");
        let wt = unique_dir("wt");
        let file = wt.join("note.csv");
        touch(&file);
        let (id, name) = insert_workspace("wt", main.to_str().unwrap());
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO workspaces (id, project_id, name, worktree_path) VALUES (?1, ?2, 'wt', ?3)",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), id, wt.to_str().unwrap()],
            )
            .expect("insert worktree");
        }
        let resp = dispatch_post(
            "/cli/workspace/resources/add",
            &params(&[
                ("workspace", name.as_str()),
                ("path", file.to_str().unwrap()),
            ]),
        );
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        assert_eq!(count_rows(&id), 1);
        let _ = fs::remove_dir_all(&main);
        let _ = fs::remove_dir_all(&wt);
    }

    #[test]
    fn remove_missing_is_404_with_code() {
        let dir = unique_dir("rm");
        let (_id, name) = insert_workspace("rm", dir.to_str().unwrap());
        let resp = dispatch_post(
            "/cli/workspace/resources/remove",
            &params(&[
                ("workspace", name.as_str()),
                ("path", "/no/such/resource.txt"),
            ]),
        );
        assert_eq!(resp.status, "404 Not Found", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["error"]["code"], "not_found");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mutating_routes_405_on_get() {
        let p = HashMap::new();
        for route in [
            "/cli/workspace/resources/add",
            "/cli/workspace/resources/remove",
        ] {
            let resp = dispatch(route, &p).expect("claimed");
            assert_eq!(resp.status, "405 Method Not Allowed", "route={route}");
        }
        assert!(dispatch("/cli/workspace/resources/nope", &p).is_none());
        let resp = dispatch_post("/cli/workspace/resources/unknown", &p);
        assert_eq!(resp.status, "404 Not Found");
    }
}
