//! `/cli/workspace/resources*` — daemon-owned workspace resource list.
//!
//! GET list; POST add/remove. Mutating routes are POST-only (GET twins 405).
//! `workspace` is name | path | UUID via [`crate::workspace_msg::resolve_workspace`].

use std::collections::HashMap;
use std::path::Path;

use k2_core::skin::SkinPass;
use k2_core::workspace_resources::{self, ResourceError};

use crate::cli::str_param;
use crate::cli_response::CliResponse;
use crate::fs_routes::{jail_rel_path, resolve_skin_workspace};
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

pub fn handle_list_gated(params: &HashMap<String, String>, skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_list(params, &pass),
        None => handle_list(params),
    }
}

pub fn handle_add_gated(params: &HashMap<String, String>, skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_add(params, &pass),
        None => handle_add(params),
    }
}

pub fn handle_remove_gated(
    params: &HashMap<String, String>,
    skin: Option<SkinPass>,
) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_remove(params, &pass),
        None => handle_remove(params),
    }
}

fn skin_workspace_param(params: &HashMap<String, String>) -> Result<String, CliResponse> {
    params
        .get("workspace")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CliResponse::bad_request("missing workspace"))
}

fn skin_rel_param(params: &HashMap<String, String>) -> Result<String, CliResponse> {
    params
        .get("path")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CliResponse::bad_request("Missing 'path' parameter"))
}

/// Relative `filePath` under the room root only. A stored row outside that
/// root (a worktree file the owner list still shows) is omitted — never
/// returned absolute.
fn guest_file_path(room_root: &str, stored: &str) -> Option<String> {
    let root = std::fs::canonicalize(room_root).ok()?;
    let stored_path = Path::new(stored);
    let canon = std::fs::canonicalize(stored_path).unwrap_or_else(|_| stored_path.to_path_buf());
    let rel = canon.strip_prefix(&root).ok()?;
    if rel.as_os_str().is_empty() {
        return None;
    }
    let s = rel.to_string_lossy().replace('\\', "/");
    if s.is_empty() || s.split('/').any(|p| p == "..") || Path::new(&s).is_absolute() {
        return None;
    }
    Some(s)
}

fn handle_skin_list(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let ws = match skin_workspace_param(params) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let resolved = match resolve_skin_workspace(pass, &ws) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if !pass.has_cap_in_room(&resolved.project_id, crate::skin_routes::FILES_READ) {
        return crate::skin_routes::missing_cap_response(crate::skin_routes::FILES_READ);
    }
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match workspace_resources::list(&conn, &resolved.project_id) {
            Ok(r) => r,
            Err(e) => return resource_error(e),
        }
    };
    let docs: Vec<serde_json::Value> = rows
        .iter()
        .filter_map(|r| {
            let rel = guest_file_path(&resolved.path, &r.file_path)?;
            Some(serde_json::json!({
                "filePath": rel,
                "fileName": file_name(&r.file_path),
                "addedAt": r.added_at,
                "missing": workspace_resources::file_missing(&r.file_path),
            }))
        })
        .collect();
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "workspaceId": resolved.project_id,
            "docs": docs,
        })
        .to_string(),
    )
}

fn handle_skin_add(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let ws = match skin_workspace_param(params) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let rel = match skin_rel_param(params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let resolved = match resolve_skin_workspace(pass, &ws) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if !pass.has_cap_in_room(&resolved.project_id, crate::skin_routes::FILES_WRITE) {
        return crate::skin_routes::missing_cap_response(crate::skin_routes::FILES_WRITE);
    }
    // Jail first. `confine_path` resolves a relative string against the daemon cwd.
    let jailed = match jail_rel_path(&resolved.path, &rel, false) {
        Ok(p) => p,
        Err(e) => return CliResponse::bad_request(e),
    };
    let abs = jailed.to_string_lossy().to_string();
    let stored = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match workspace_resources::add(&conn, &resolved.project_id, &abs) {
            Ok(s) => s,
            Err(e) => return resource_error(e),
        }
    };
    emit_changed(&resolved.project_id);
    let Some(rel_out) = guest_file_path(&resolved.path, &stored) else {
        return CliResponse::internal_error("stored resource is outside the room root");
    };
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "workspaceId": resolved.project_id,
            "filePath": rel_out,
            "fileName": file_name(&stored),
        })
        .to_string(),
    )
}

fn handle_skin_remove(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let ws = match skin_workspace_param(params) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let rel = match skin_rel_param(params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let resolved = match resolve_skin_workspace(pass, &ws) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if !pass.has_cap_in_room(&resolved.project_id, crate::skin_routes::FILES_WRITE) {
        return crate::skin_routes::missing_cap_response(crate::skin_routes::FILES_WRITE);
    }
    let jailed = match jail_rel_path(&resolved.path, &rel, false) {
        Ok(p) => p,
        Err(e) => return CliResponse::bad_request(e),
    };
    let abs = jailed.to_string_lossy().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(e) = workspace_resources::remove(&conn, &resolved.project_id, &abs) {
            return resource_error(e);
        }
    }
    emit_changed(&resolved.project_id);
    let Some(rel_out) = guest_file_path(&resolved.path, &abs) else {
        return CliResponse::internal_error("removed resource is outside the room root");
    };
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "workspaceId": resolved.project_id,
            "filePath": rel_out,
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
    fn insert_handled(path: &str, name: &str, handle: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, name, path, handle],
        )
        .expect("insert project");
        id
    }

    fn session_pass(project_id: &str, caps: &[&str]) -> SkinPass {
        let mut room_policy = k2_core::skin::RoomPolicy::new();
        room_policy.insert(
            project_id.to_string(),
            caps.iter().map(|c| (*c).to_string()).collect(),
        );
        SkinPass {
            id: uuid::Uuid::new_v4().to_string(),
            principal_id: Some(uuid::Uuid::new_v4().to_string()),
            username: "guest".to_string(),
            caps: caps.iter().map(|c| (*c).to_string()).collect(),
            rooms: vec![project_id.to_string()],
            session: true,
            room_policy,
        }
    }

    #[test]
    fn skin_resources_thread_only_missing_files_caps() {
        let dir = unique_dir("skin-cap");
        let file = dir.join("a.csv");
        touch(&file);
        let canon = dir.canonicalize().expect("canon");
        let handle = format!("rcap{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let name = format!("rcap-name-{handle}");
        let id = insert_handled(&canon.to_string_lossy(), &name, &handle);
        let thread_only = session_pass(
            &id,
            &[
                k2_core::skin::CAP_THREAD_READ,
                k2_core::skin::CAP_THREAD_POST,
            ],
        );
        let list = handle_list_gated(
            &params(&[("workspace", handle.as_str())]),
            Some(thread_only.clone()),
        );
        assert_eq!(list.status, "403 Forbidden", "{}", list.body);
        assert!(
            list.body.contains("missing capability files:read"),
            "{}",
            list.body
        );
        assert!(!list.body.contains("skin_room"), "{}", list.body);
        let add = handle_add_gated(
            &params(&[("workspace", handle.as_str()), ("path", "a.csv")]),
            Some(thread_only),
        );
        assert_eq!(add.status, "403 Forbidden", "{}", add.body);
        assert!(
            add.body.contains("missing capability files:write"),
            "{}",
            add.body
        );
        assert!(!add.body.contains("skin_room"), "{}", add.body);
        assert_eq!(count_rows(&id), 0);
        let write_only = session_pass(&id, &[k2_core::skin::CAP_FILES_WRITE]);
        let list = handle_list_gated(&params(&[("workspace", handle.as_str())]), Some(write_only));
        assert_eq!(list.status, "403 Forbidden", "{}", list.body);
        assert!(
            list.body.contains("missing capability files:read"),
            "files:write does not imply files:read: {}",
            list.body
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn skin_resources_add_outside_jail_writes_no_row() {
        let dir = unique_dir("skin-jail");
        let file = dir.join("a.csv");
        touch(&file);
        let canon = dir.canonicalize().expect("canon");
        let handle = format!("rjail{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let name = format!("rjail-name-{handle}");
        let id = insert_handled(&canon.to_string_lossy(), &name, &handle);
        let pass = session_pass(
            &id,
            &[
                k2_core::skin::CAP_FILES_READ,
                k2_core::skin::CAP_FILES_WRITE,
            ],
        );
        let resp = handle_add_gated(
            &params(&[("workspace", handle.as_str()), ("path", "../outside.csv")]),
            Some(pass.clone()),
        );
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        assert!(
            resp.body.contains("path must not contain '..'"),
            "{}",
            resp.body
        );
        assert_eq!(count_rows(&id), 0);
        let abs = file.to_string_lossy().to_string();
        let resp = handle_add_gated(
            &params(&[("workspace", handle.as_str()), ("path", abs.as_str())]),
            Some(pass),
        );
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        assert!(
            resp.body.contains("path must be relative to the workspace"),
            "{}",
            resp.body
        );
        assert_eq!(count_rows(&id), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn skin_resources_guest_paths_are_relative_and_worktree_is_omitted() {
        let dir = unique_dir("skin-rel");
        let notes = dir.join("notes");
        fs::create_dir_all(&notes).expect("notes");
        let file = notes.join("a.csv");
        touch(&file);
        let canon = dir.canonicalize().expect("canon");
        let wt = unique_dir("skin-wt");
        let wt_file = wt.join("note.csv");
        touch(&wt_file);
        let handle = format!("rrel{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let name = format!("rrel-name-{handle}");
        let id = insert_handled(&canon.to_string_lossy(), &name, &handle);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO workspaces (id, project_id, name, worktree_path) VALUES (?1, ?2, 'wt', ?3)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    id,
                    wt.canonicalize().expect("wt canon").to_string_lossy().to_string()
                ],
            )
            .expect("insert worktree");
        }
        let pass = session_pass(
            &id,
            &[
                k2_core::skin::CAP_FILES_READ,
                k2_core::skin::CAP_FILES_WRITE,
            ],
        );
        let added = handle_add_gated(
            &params(&[("workspace", handle.as_str()), ("path", "notes/a.csv")]),
            Some(pass.clone()),
        );
        assert_eq!(added.status, "200 OK", "{}", added.body);
        let added_json: serde_json::Value = serde_json::from_str(&added.body).expect("json");
        assert_eq!(added_json["filePath"], "notes/a.csv");
        assert_eq!(added_json["fileName"], "a.csv");
        let canon_s = canon.to_string_lossy().to_string();
        assert!(
            !added.body.contains(&canon_s),
            "guest add leaked an absolute path: {}",
            added.body
        );
        let stored: String = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT file_path FROM workspace_resources WHERE workspace_id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .expect("stored row")
        };
        assert!(
            stored.starts_with(&canon_s),
            "row must be the jailed absolute path, got {stored}"
        );
        assert_ne!(stored, "notes/a.csv");
        let owner_wt = handle_add(&params(&[
            ("workspace", name.as_str()),
            ("path", wt_file.to_str().expect("wt path")),
        ]));
        assert_eq!(owner_wt.status, "200 OK", "{}", owner_wt.body);
        assert_eq!(count_rows(&id), 2);
        let owner_list = ok_json(handle_list(&params(&[("workspace", name.as_str())])));
        let owner_docs = owner_list["docs"].as_array().expect("owner docs");
        assert_eq!(owner_docs.len(), 2, "{owner_list}");
        for doc in owner_docs {
            let fp = doc["filePath"].as_str().expect("owner filePath");
            assert!(
                std::path::Path::new(fp).is_absolute(),
                "owner list stays absolute: {fp}"
            );
        }
        let guest = handle_list_gated(&params(&[("workspace", handle.as_str())]), Some(pass));
        assert_eq!(guest.status, "200 OK", "{}", guest.body);
        let guest_json: serde_json::Value = serde_json::from_str(&guest.body).expect("json");
        let docs = guest_json["docs"].as_array().expect("docs");
        assert_eq!(docs.len(), 1, "worktree row must be omitted: {guest_json}");
        assert_eq!(docs[0]["filePath"], "notes/a.csv");
        assert!(
            !guest.body.contains("note.csv"),
            "worktree file leaked to the guest: {}",
            guest.body
        );
        assert!(
            !guest.body.contains(&canon_s),
            "guest list leaked an absolute path: {}",
            guest.body
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&wt);
    }
}
