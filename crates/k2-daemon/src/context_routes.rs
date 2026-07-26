//! Daemon `/cli/context/*` routes — context hamburger (AGENTS.md layer stack).
//!
//! See `.k2/prds/prd-context-hamburger-v1.md` §7.
//!
//! | Method | Route |
//! |--------|-------|
//! | GET    | `/cli/context/layers?project=` |
//! | POST   | `/cli/context/add` |
//! | POST   | `/cli/context/remove` |
//! | POST   | `/cli/context/set-enabled` |
//! | POST   | `/cli/context/move` |
//! | GET    | `/cli/context/show?project=&outline=` |
//! | POST   | `/cli/context/regen` |
//! | GET    | `/cli/context/presets` |
//!
//! Auth: `token_ok` (owner or connect-user session) — same tier as other
//! workspace mutates. POST-only mutations 405 on GET.

use std::collections::HashMap;

use serde::Deserialize;

use crate::cli_response::CliResponse;
use k2_core::workspace::context_layers::{self, ContextError};

// ── Error helpers ─────────────────────────────────────────────────────

fn err_response(e: ContextError) -> CliResponse {
    let status = match &e {
        ContextError::NotFound(_) => "404 Not Found",
        ContextError::DuplicateLayer(_) => "409 Conflict",
        ContextError::Db(_) => "500 Internal Server Error",
        _ => "400 Bad Request",
    };
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": e.code(),
                "hint": e.hint(),
            },
        })
        .to_string(),
    }
}

fn usage(hint: impl Into<String>) -> CliResponse {
    err_response(ContextError::BadUsage(hint.into()))
}

fn resolve_project(token: &str) -> Result<String, CliResponse> {
    if token.trim().is_empty() {
        return Err(usage("missing project (path, name, or id)"));
    }
    crate::workspace_msg::resolve_workspace(token.trim()).ok_or_else(|| {
        err_response(ContextError::NotFound(format!(
            "workspace not registered: {token}"
        )))
    })
}

// ── GET dispatch ──────────────────────────────────────────────────────

/// GET-chain dispatch. Returns `Some` for handled paths (including 405 for
/// POST-only mutations reached via GET). `None` if not a context route.
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    let resp = match path {
        "/cli/context/layers" => handle_layers(params),
        "/cli/context/show" => handle_show(params),
        "/cli/context/presets" => handle_presets(),
        // POST-only mutations hit via GET chain → 405.
        "/cli/context/add"
        | "/cli/context/remove"
        | "/cli/context/set-enabled"
        | "/cli/context/move"
        | "/cli/context/regen" => CliResponse::method_not_allowed(),
        _ => return None,
    };
    Some(resp)
}

/// POST dispatch (exact path match).
pub fn dispatch_post(path: &str, body: &[u8]) -> CliResponse {
    match path {
        "/cli/context/add" => handle_add(body),
        "/cli/context/remove" => handle_remove(body),
        "/cli/context/set-enabled" => handle_set_enabled(body),
        "/cli/context/move" => handle_move(body),
        "/cli/context/regen" => handle_regen(body),
        _ => CliResponse::not_found(),
    }
}

// ── Handlers ──────────────────────────────────────────────────────────

fn handle_layers(params: &HashMap<String, String>) -> CliResponse {
    let project = match params.get("project").or_else(|| params.get("project_path")) {
        Some(p) if !p.is_empty() => p.as_str(),
        _ => return usage("missing project query param"),
    };
    let path = match resolve_project(project) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match context_layers::list_stack(&path) {
        Ok(stack) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "pinned": stack.pinned,
                "layers": stack.layers,
                "softWarn": stack.soft_warn,
                "composedBytes": stack.composed_bytes,
            })
            .to_string(),
        ),
        Err(e) => err_response(e),
    }
}

fn handle_show(params: &HashMap<String, String>) -> CliResponse {
    let project = match params.get("project").or_else(|| params.get("project_path")) {
        Some(p) if !p.is_empty() => p.as_str(),
        _ => return usage("missing project query param"),
    };
    let path = match resolve_project(project) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let outline = matches!(
        params.get("outline").map(|s| s.as_str()),
        Some("1") | Some("true") | Some("on")
    );
    if outline {
        match context_layers::show_outline(&path) {
            Ok(sections) => CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "outline": sections,
                })
                .to_string(),
            ),
            Err(e) => err_response(e),
        }
    } else {
        match context_layers::show_composed(&path) {
            Ok(body) => CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "body": body,
                })
                .to_string(),
            ),
            Err(e) => err_response(e),
        }
    }
}

fn handle_presets() -> CliResponse {
    let presets = context_layers::list_presets();
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "presets": presets,
        })
        .to_string(),
    )
}

#[derive(Deserialize)]
struct AddBody {
    project: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    preset: Option<String>,
    #[serde(default)]
    label: Option<String>,
}

fn handle_add(body: &[u8]) -> CliResponse {
    let parsed: AddBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return usage(format!("invalid JSON body: {e}")),
    };
    let project_path = match resolve_project(&parsed.project) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match context_layers::add_layer(
        &project_path,
        parsed.path.as_deref(),
        parsed.preset.as_deref(),
        parsed.label.as_deref(),
    ) {
        Ok(layer) => {
            let stack = context_layers::list_stack(&project_path).ok();
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "layer": layer,
                    "softWarn": stack.as_ref().map(|s| s.soft_warn).unwrap_or(false),
                    "composedBytes": stack.as_ref().map(|s| s.composed_bytes).unwrap_or(0),
                })
                .to_string(),
            )
        }
        Err(e) => err_response(e),
    }
}

#[derive(Deserialize)]
struct IdBody {
    project: String,
    id: String,
}

fn handle_remove(body: &[u8]) -> CliResponse {
    let parsed: IdBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return usage(format!("invalid JSON body: {e}")),
    };
    if parsed.id.trim().is_empty() {
        return usage("missing id");
    }
    let project_path = match resolve_project(&parsed.project) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match context_layers::remove_layer(&project_path, &parsed.id) {
        Ok(()) => CliResponse::ok_json(r#"{"ok":true}"#.to_string()),
        Err(e) => err_response(e),
    }
}

#[derive(Deserialize)]
struct SetEnabledBody {
    project: String,
    id: String,
    enabled: bool,
}

fn handle_set_enabled(body: &[u8]) -> CliResponse {
    let parsed: SetEnabledBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return usage(format!("invalid JSON body: {e}")),
    };
    if parsed.id.trim().is_empty() {
        return usage("missing id");
    }
    let project_path = match resolve_project(&parsed.project) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match context_layers::set_enabled(&project_path, &parsed.id, parsed.enabled) {
        Ok(layer) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "layer": layer,
            })
            .to_string(),
        ),
        Err(e) => err_response(e),
    }
}

#[derive(Deserialize)]
struct MoveBody {
    project: String,
    id: String,
    #[serde(default)]
    position: Option<i64>,
    #[serde(default)]
    direction: Option<String>,
}

fn handle_move(body: &[u8]) -> CliResponse {
    let parsed: MoveBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return usage(format!("invalid JSON body: {e}")),
    };
    if parsed.id.trim().is_empty() {
        return usage("missing id");
    }
    let project_path = match resolve_project(&parsed.project) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match context_layers::move_layer(
        &project_path,
        &parsed.id,
        parsed.position,
        parsed.direction.as_deref(),
    ) {
        Ok(layer) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "layer": layer,
            })
            .to_string(),
        ),
        Err(e) => err_response(e),
    }
}

#[derive(Deserialize)]
struct ProjectBody {
    project: String,
}

fn handle_regen(body: &[u8]) -> CliResponse {
    let parsed: ProjectBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return usage(format!("invalid JSON body: {e}")),
    };
    let project_path = match resolve_project(&parsed.project) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match context_layers::regen(&project_path) {
        Ok(()) => {
            let stack = context_layers::list_stack(&project_path).ok();
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "softWarn": stack.as_ref().map(|s| s.soft_warn).unwrap_or(false),
                    "composedBytes": stack.as_ref().map(|s| s.composed_bytes).unwrap_or(0),
                })
                .to_string(),
            )
        }
        Err(e) => err_response(e),
    }
}

// ── Unit tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;

    fn unique_root(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2-ctx-routes-{}-{}-{}",
            tag,
            std::process::id(),
            Uuid::new_v4()
        ));
        fs::create_dir_all(p.join(".k2/agent")).unwrap();
        p
    }

    fn register(path: &str) -> String {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, "ctx-route-test", path],
        )
        .unwrap();
        id
    }

    fn cleanup(path: &str, id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM project_context_layers WHERE project_id = ?1",
            rusqlite::params![id],
        );
        let _ = conn.execute(
            "DELETE FROM projects WHERE id = ?1",
            rusqlite::params![id],
        );
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn get_layers_empty_stack() {
        let root = unique_root("layers");
        let path = root.to_str().unwrap().to_string();
        let pid = register(&path);

        let mut params = HashMap::new();
        params.insert("project".into(), path.clone());
        let resp = dispatch("/cli/context/layers", &params).expect("handled");
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["ok"], true);
        assert!(v["layers"].as_array().unwrap().is_empty());
        assert_eq!(v["pinned"].as_array().unwrap().len(), 3);

        cleanup(&path, &pid);
    }

    #[test]
    fn post_add_and_presets() {
        let root = unique_root("add");
        let path = root.to_str().unwrap().to_string();
        let pid = register(&path);
        fs::write(root.join("note.md"), "# Note\n\nbody\n").unwrap();

        let body = serde_json::json!({
            "project": path,
            "path": "note.md",
            "label": "Note"
        })
        .to_string();
        let resp = dispatch_post("/cli/context/add", body.as_bytes());
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["layer"]["path"], "note.md");

        // Duplicate → 409
        let resp = dispatch_post("/cli/context/add", body.as_bytes());
        assert_eq!(resp.status, "409 Conflict");
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["error"]["code"], "duplicate_layer");

        // Presets GET
        let resp = dispatch("/cli/context/presets", &HashMap::new()).unwrap();
        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        let presets = v["presets"].as_array().unwrap();
        assert!(presets.len() >= 4, "wiki + manager + k2 packs");
        assert!(
            presets.iter().any(|p| p["id"] == "manager:pack"),
            "manager:pack preset listed"
        );
        assert!(
            presets.iter().any(|p| p["id"] == "k2:pack"),
            "k2:pack preset listed"
        );

        cleanup(&path, &pid);
    }

    #[test]
    fn pack_preset_materializes_and_system_toggle() {
        let root = unique_root("pack");
        let path = root.to_str().unwrap().to_string();
        let pid = register(&path);

        let body = serde_json::json!({
            "project": path,
            "preset": "manager:pack"
        })
        .to_string();
        let resp = dispatch_post("/cli/context/add", body.as_bytes());
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["layer"]["source"], "preset:manager");
        assert_eq!(v["layer"]["exists"], true);
        assert!(
            root.join(".k2/context/presets/manager.md").is_file(),
            "manager pack file must materialize"
        );

        // System layer off via set-enabled
        let body = serde_json::json!({
            "project": path,
            "id": "pinned:tooling",
            "enabled": false
        })
        .to_string();
        let resp = dispatch_post("/cli/context/set-enabled", body.as_bytes());
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["layer"]["id"], "pinned:tooling");
        assert_eq!(v["layer"]["enabled"], false);

        let mut params = HashMap::new();
        params.insert("project".into(), path.clone());
        let resp = dispatch("/cli/context/layers", &params).unwrap();
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        let pinned = v["pinned"].as_array().unwrap();
        let tooling = pinned
            .iter()
            .find(|p| p["id"] == "pinned:tooling")
            .expect("tooling row");
        assert_eq!(tooling["enabled"], false);

        cleanup(&path, &pid);
    }

    #[test]
    fn post_only_mutations_405_on_get() {
        let resp = dispatch("/cli/context/add", &HashMap::new()).unwrap();
        assert_eq!(resp.status, "405 Method Not Allowed");
        let resp = dispatch("/cli/context/regen", &HashMap::new()).unwrap();
        assert_eq!(resp.status, "405 Method Not Allowed");
    }

    #[test]
    fn path_escape_returns_stable_code() {
        let root = unique_root("escape");
        let path = root.to_str().unwrap().to_string();
        let pid = register(&path);

        let body = serde_json::json!({
            "project": path,
            "path": "../../etc/passwd"
        })
        .to_string();
        let resp = dispatch_post("/cli/context/add", body.as_bytes());
        assert_eq!(resp.status, "400 Bad Request");
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["error"]["code"], "path_escape");

        cleanup(&path, &pid);
    }

    #[test]
    fn unknown_project_404() {
        let mut params = HashMap::new();
        params.insert(
            "project".into(),
            "/tmp/k2-ctx-routes-nope-xyz".into(),
        );
        let resp = dispatch("/cli/context/layers", &params).unwrap();
        assert_eq!(resp.status, "404 Not Found");
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["error"]["code"], "not_found");
    }
}
