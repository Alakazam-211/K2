//! `/v1` hire + wiki notes + context layers (Julie items 2–4).
//!
//! ```text
//! POST /v1/w                         → create/converge a workspace (hire)
//! POST /v1/w/<ws>/wiki/notes         → write one `.k2/wiki/` note
//! GET  /v1/w/<ws>/context            → list layers (parity with /cli/context/layers)
//! POST /v1/w/<ws>/context            → add catalog XOR inline {label, markdown}
//! POST /v1/w/<ws>/context/remove     → {path|catalog|id}
//! POST /v1/w/<ws>/context/regen
//! ```
//!
//! Surface gate is dispatcher `api_enabled()` (same as host-sessions).
//! Capability: [`V1Capability::HostSessions`] via
//! [`crate::v1_host_sessions::require_host_sessions`]. New unregistered
//! paths require owner or a `*` workspace grant (finite lists 403 usage,
//! not a 404 oracle). Existing registered paths use handle and/or
//! basename grants (ungranted → uniform 404).

use std::path::Path;

use serde::Deserialize;

use k2_core::hire::converge::{self, ConvergeAuthority, ConvergeRequest};
use k2_core::hire::limits::HireLimits;
use k2_core::hire::HireCaller;

use crate::cli_response::CliResponse;
use crate::routes::http::V1Principal;
use crate::v1_host_sessions::require_host_sessions;
use crate::v1_sandboxes::{
    decode_and_validate_segment, resolve_authorized_workspace, uniform_ws_404,
};

use k2_core::workspace::context_layers::{self, ContextError};

// ── Shared errors ─────────────────────────────────────────────────────

fn usage(hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status: "400 Bad Request",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": "usage", "hint": hint.into() },
        })
        .to_string(),
    }
}

fn context_err(e: ContextError) -> CliResponse {
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
            "error": { "code": e.code(), "hint": e.hint() },
        })
        .to_string(),
    }
}

fn require_post(is_post: bool) -> Result<(), CliResponse> {
    if is_post {
        Ok(())
    } else {
        Err(CliResponse::method_not_allowed())
    }
}

fn authorize_ws(principal: &V1Principal, ws_raw: &str) -> Result<String, CliResponse> {
    if let Err(resp) = require_host_sessions(principal) {
        return Err(resp);
    }
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return Err(uniform_ws_404());
    };
    resolve_authorized_workspace(principal, &slug)
}

fn authorizes_existing(principal: &V1Principal, handle: Option<&str>, basename: &str) -> bool {
    match principal {
        V1Principal::Owner => true,
        V1Principal::Api(p) => {
            handle
                .map(|h| !h.is_empty() && p.authorizes_workspace(h))
                .unwrap_or(false)
                || (!basename.is_empty() && p.authorizes_workspace(basename))
        }
    }
}

// ── Path + project lookup ─────────────────────────────────────────────

#[cfg(test)]
struct ProjectRow {
    default_agent: Option<String>,
    default_model: Option<String>,
}

#[cfg(test)]
fn lookup_project(path: &str) -> Option<ProjectRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT default_agent, default_model FROM projects WHERE path = ?1",
        rusqlite::params![path],
        |r| {
            Ok(ProjectRow {
                default_agent: r.get(0)?,
                default_model: r.get(1)?,
            })
        },
    )
    .ok()
}

fn basename_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "workspace".into())
}

fn stack_json(path: &str) -> serde_json::Value {
    match context_layers::list_stack(path) {
        Ok(stack) => serde_json::json!({
            "ok": true,
            "pinned": stack.pinned,
            "layers": stack.layers,
            "softWarn": stack.soft_warn,
            "composedBytes": stack.composed_bytes,
        }),
        Err(_) => serde_json::json!({
            "ok": true,
            "pinned": [],
            "layers": [],
            "softWarn": false,
            "composedBytes": 0,
        }),
    }
}

pub(crate) fn emit_hire_watches() {
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::ProjectsChanged {});
    crate::fs_live::resync_watches();
    crate::charter_compose_watch::resync_watches();
}

// ── Bodies ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct WikiNotesBody {
    id: String,
    body: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContextAddBody {
    #[serde(default)]
    catalog: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    markdown: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContextRemoveBody {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    catalog: Option<String>,
    #[serde(default)]
    id: Option<String>,
}

// ── Hire ──────────────────────────────────────────────────────────────

/// `POST /v1/w` (and GET → 405).
pub(crate) fn handle_v1_w(principal: &V1Principal, is_post: bool, body: &[u8]) -> CliResponse {
    if let Err(resp) = require_post(is_post) {
        return resp;
    }
    handle_v1_hire(principal, body)
}

/// What an API key may do on `POST /v1/w` beyond its workspace grant
/// (T19: new-workspace creation sits under the hire scope).
pub(crate) struct KeyHireDoor {
    pub limits: Option<HireLimits>,
    pub legacy: bool,
}

pub(crate) fn key_hire_door(p: &k2_core::api_keys::ApiPrincipal) -> KeyHireDoor {
    let limits = if p.capabilities.hire {
        HireLimits::parse_stored(k2_core::api_keys::hire_limits_for(&p.id).as_deref())
    } else {
        None
    };
    let legacy = k2_core::api_keys::legacy_new_workspace(&p.id) && p.authorizes_workspace("*");
    KeyHireDoor { limits, legacy }
}

/// `POST /v1/w` — create or converge a workspace on the shared hire core.
/// Does not spawn a PTY.
///
/// API keys: an existing workspace needs the key's workspace grant (else the
/// uniform 404); a NEW one needs the hire scope (or the pre-0.45.6 `*`
/// back-compat flag), must sit inside the owner-set hire root, and counts
/// against the key's quotas. `dbAccess` is capped by the key's DB door.
pub(crate) fn handle_v1_hire(principal: &V1Principal, body: &[u8]) -> CliResponse {
    if let Err(resp) = require_host_sessions(principal) {
        return resp;
    }
    let req: ConvergeRequest = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return usage(format!("invalid JSON body: {e}")),
    };
    let expanded = match converge::expand_hire_path(&req.path) {
        Ok(p) => p,
        Err(e) => return crate::hire_routes::hire_error_response(&e),
    };
    let existing = converge::lookup_project_row(&expanded);
    let (caller, auth, quota_key) = match principal {
        V1Principal::Owner => (HireCaller::Owner, ConvergeAuthority::unrestricted(), None),
        V1Principal::Api(p) => {
            if let Some((path, row)) = existing.as_ref() {
                let handle = row.handle.as_deref().map(str::trim).filter(|s| !s.is_empty());
                if !authorizes_existing(principal, handle, &basename_of(path)) {
                    return uniform_ws_404();
                }
            }
            let door = key_hire_door(p);
            let db_door: &'static str = if p.capabilities.db { "write" } else { "off" };
            let max_db = match door.limits.as_ref() {
                Some(l) => l.db_ceiling(p.capabilities.db),
                None => db_door,
            };
            let root = match k2_core::hire::root::base_dir(door.limits.as_ref().map(|l| l.folder.as_str())) {
                Ok(r) => r,
                Err(e) => return crate::hire_routes::hire_error_response(&e),
            };
            let caller = HireCaller::ApiKey {
                id: p.id.clone(),
                label: k2_core::api_keys::key_label(&p.id),
            };
            let auth = ConvergeAuthority {
                new_workspace_root: Some(root),
                may_create: door.limits.is_some() || door.legacy,
                max_db: Some(max_db),
                create_refusal: Some(
                    "hire of a new workspace needs a key with the hire scope (or a pre-0.45.6 * grant) or the owner token"
                        .into(),
                ),
            };
            (caller, auth, door.limits.map(|l| (p.id.clone(), l)))
        }
    };
    if existing.is_none() {
        if let Some((key_id, l)) = quota_key.as_ref() {
            if let Err(e) = crate::hire_routes::check_single_quota(key_id, l) {
                return crate::hire_routes::hire_error_response(&e);
            }
        }
    }
    let out = match converge::converge(&req, &caller, &auth) {
        Ok(o) => o,
        Err(e) => return crate::hire_routes::hire_error_response(&e),
    };
    if out.created && caller.api_key_id().is_some() {
        crate::hire_routes::ledger_single_hire(&caller, &out);
    }
    if out.changed || out.created {
        emit_hire_watches();
    } else {
        crate::charter_compose_watch::resync_watches();
    }
    let mut body = serde_json::json!({
        "ok": true,
        "path": out.path,
        "id": out.id,
        "handle": out.handle,
        "name": out.name,
        "changed": out.changed,
    });
    if !out.wiki_notes.is_empty() {
        body["wiki"] = serde_json::json!({ "notes": out.wiki_notes });
    }
    if req.context.is_some() || req.layers.is_some() {
        body["context"] = stack_json(&out.path);
    }
    CliResponse::ok_json(body.to_string())
}

// ── Wiki notes ────────────────────────────────────────────────────────

pub(crate) fn handle_v1_wiki_notes(
    principal: &V1Principal,
    ws_raw: &str,
    body: &[u8],
) -> CliResponse {
    let ws_path = match authorize_ws(principal, ws_raw) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let parsed: WikiNotesBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return usage(format!("invalid JSON body: {e}")),
    };
    match k2_core::wiki::write_note(Path::new(&ws_path), &parsed.id, &parsed.body) {
        Ok(rel) => CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "id": parsed.id,
                "path": rel,
            })
            .to_string(),
        ),
        Err(e) => usage(e),
    }
}

// ── Context ───────────────────────────────────────────────────────────

pub(crate) fn handle_v1_context_list(principal: &V1Principal, ws_raw: &str) -> CliResponse {
    let ws_path = match authorize_ws(principal, ws_raw) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    match context_layers::list_stack(&ws_path) {
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
        Err(e) => context_err(e),
    }
}

pub(crate) fn handle_v1_context_add(
    principal: &V1Principal,
    ws_raw: &str,
    body: &[u8],
) -> CliResponse {
    let ws_path = match authorize_ws(principal, ws_raw) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let parsed: ContextAddBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return usage(format!("invalid JSON body: {e}")),
    };
    let catalog = parsed
        .catalog
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let label = parsed
        .label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let markdown = parsed.markdown.as_deref();
    let inline = label.is_some() || markdown.map(|s| !s.is_empty()).unwrap_or(false);
    if catalog.is_some() == inline {
        return usage("provide exactly one of catalog or {label, markdown}");
    }
    if let Some(id) = catalog {
        match context_layers::add_layer(&ws_path, None, Some(id), None) {
            Ok(layer) => {
                crate::charter_compose_watch::resync_watches();
                let stack = context_layers::list_stack(&ws_path).ok();
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
            Err(e) => context_err(e),
        }
    } else {
        let label = match label {
            Some(l) => l,
            None => return usage("inline layer requires label"),
        };
        let markdown = markdown.unwrap_or("");
        match k2_core::hire::steps::add_inline_layer(&ws_path, label, markdown) {
            Ok(_) => {
                crate::charter_compose_watch::resync_watches();
                handle_v1_context_list(principal, ws_raw)
            }
            Err(e) => crate::hire_routes::hire_error_response(&e),
        }
    }
}

pub(crate) fn handle_v1_context_remove(
    principal: &V1Principal,
    ws_raw: &str,
    body: &[u8],
) -> CliResponse {
    let ws_path = match authorize_ws(principal, ws_raw) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let parsed: ContextRemoveBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return usage(format!("invalid JSON body: {e}")),
    };
    let id = parsed
        .id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let path = parsed
        .path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let catalog = parsed
        .catalog
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let n = id.is_some() as u8 + path.is_some() as u8 + catalog.is_some() as u8;
    if n != 1 {
        return usage("provide exactly one of path, catalog, or id");
    }
    let key = if let Some(c) = catalog {
        match context_layers::list_catalog()
            .into_iter()
            .find(|e| e.id == c)
        {
            Some(entry) => entry.path,
            None => c.to_string(),
        }
    } else {
        id.or(path).unwrap_or("").to_string()
    };
    match context_layers::remove_layer(&ws_path, &key) {
        Ok(()) => {
            crate::charter_compose_watch::resync_watches();
            CliResponse::ok_json(r#"{"ok":true}"#.to_string())
        }
        Err(e) => context_err(e),
    }
}

pub(crate) fn handle_v1_context_regen(
    principal: &V1Principal,
    ws_raw: &str,
    _body: &[u8],
) -> CliResponse {
    let ws_path = match authorize_ws(principal, ws_raw) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    match context_layers::regen(&ws_path) {
        Ok(()) => {
            crate::charter_compose_watch::resync_watches();
            let stack = context_layers::list_stack(&ws_path).ok();
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "softWarn": stack.as_ref().map(|s| s.soft_warn).unwrap_or(false),
                    "composedBytes": stack.as_ref().map(|s| s.composed_bytes).unwrap_or(0),
                })
                .to_string(),
            )
        }
        Err(e) => context_err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::http::V1Principal;
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn apik(id: &str, grant: Option<&str>) -> V1Principal {
        apik_caps(id, grant, k2_core::api_keys::ApiCapabilities::all())
    }

    fn apik_caps(
        id: &str,
        grant: Option<&str>,
        capabilities: k2_core::api_keys::ApiCapabilities,
    ) -> V1Principal {
        V1Principal::Api(k2_core::api_keys::ApiPrincipal {
            id: id.to_string(),
            anthropic_key: None,
            provider: None,
            base_url: None,
            scope: "owner".to_string(),
            allowed_workspaces: grant.map(str::to_string),
            capabilities,
        })
    }

    fn insert_project(name: &str, path: &str) -> String {
        let id = Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, path],
        )
        .expect("insert project");
        id
    }

    fn unique_ws(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "k2-v1hire-{}-{}-{}",
            label,
            std::process::id(),
            Uuid::new_v4()
        ))
    }

    fn hire_json(path: &str) -> String {
        serde_json::json!({ "path": path }).to_string()
    }

    fn parse_ok(r: &CliResponse) -> serde_json::Value {
        assert_eq!(r.status, "200 OK", "body={}", r.body);
        serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json {}: {}", e, r.body))
    }

    fn project_exists(path: &str) -> bool {
        lookup_project(path).is_some()
    }

    #[test]
    fn get_v1_w_is_405() {
        k2_core::db::init_for_tests();
        let r = handle_v1_w(&V1Principal::Owner, false, b"{}");
        assert_eq!(r.status, "405 Method Not Allowed", "body={}", r.body);
        assert!(
            r.body.contains("POST required"),
            "405 body: {}",
            r.body
        );
    }

    #[test]
    fn owner_hire_seeds_wiki_and_project_row() {
        k2_core::db::init_for_tests();
        let ws = unique_ws("owner-hire");
        let path = ws.to_string_lossy().into_owned();
        let r = handle_v1_hire(&V1Principal::Owner, hire_json(&path).as_bytes());
        let v = parse_ok(&r);
        assert_eq!(v["ok"], true, "body={}", r.body);
        assert_eq!(v["path"], path, "body={}", r.body);
        assert_eq!(v["changed"], true, "body={}", r.body);
        let handle = v["handle"].as_str().expect("handle");
        assert!(!handle.is_empty(), "handle must be set for host-sessions");
        let id = v["id"].as_str().expect("id");
        assert!(!id.is_empty(), "id: {}", r.body);
        assert!(project_exists(&path), "project row missing for {path}");
        assert!(
            ws.join(".k2/wiki/Home.md").is_file(),
            "Home.md must be seeded"
        );
        assert!(
            ws.join(".k2/wiki/_Index.md").is_file(),
            "_Index.md must be seeded"
        );
        let _ = fs::remove_dir_all(&ws);
    }

    /// A DB-backed key (so the 0160 columns are real), resolved.
    fn minted_key(label: &str, grant: &str, caps: k2_core::api_keys::ApiCapabilities, hire: Option<&str>) -> V1Principal {
        let (_id, raw) = k2_core::api_keys::create_api_key_ex(label, None, Some(grant), None, None, Some(caps), hire)
            .expect("mint key");
        V1Principal::Api(k2_core::api_keys::resolve_api_key(&raw).expect("resolve key"))
    }

    fn host_and_hire() -> k2_core::api_keys::ApiCapabilities {
        k2_core::api_keys::ApiCapabilities { hire: true, ..k2_core::api_keys::ApiCapabilities::new_key_default() }
    }

    #[test]
    fn star_grant_without_hire_scope_cannot_create_t19() {
        k2_core::db::init_for_tests();
        let root = crate::hire_routes::test_support::ensure_test_root();
        let path = root.join(format!("t19-{}", Uuid::new_v4().simple())).to_string_lossy().into_owned();
        let key = minted_key("t19-star", "*", k2_core::api_keys::ApiCapabilities::new_key_default(), None);
        let r = handle_v1_hire(&key, hire_json(&path).as_bytes());
        assert_eq!(r.status, "403 Forbidden", "body={}", r.body);
        assert!(r.body.contains("hire scope"), "body={}", r.body);
        assert!(!project_exists(&path), "403 must not register");
        assert!(!Path::new(&path).exists(), "403 must not mint the folder");
    }

    #[test]
    fn legacy_star_key_keeps_creating_but_only_under_the_root() {
        k2_core::db::init_for_tests();
        let root = crate::hire_routes::test_support::ensure_test_root();
        let key = minted_key("legacy-star", r#"["*"]"#, k2_core::api_keys::ApiCapabilities::new_key_default(), None);
        let V1Principal::Api(p) = &key else { unreachable!() };
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("UPDATE api_keys SET legacy_new_workspace = 1 WHERE id = ?1", rusqlite::params![p.id])
                .expect("flag legacy");
        }
        let inside = root.join(format!("legacy-{}", Uuid::new_v4().simple())).to_string_lossy().into_owned();
        let r = handle_v1_hire(&key, hire_json(&inside).as_bytes());
        assert_eq!(r.status, "200 OK", "body={}", r.body);
        assert!(project_exists(&inside));
        // Scout's flow keeps working: the legacy key reaches the workspace it
        // made through its `*` grant (reach rules are for hire-scoped keys).
        let handle = parse_ok(&r)["handle"].as_str().expect("handle").to_string();
        assert!(
            crate::v1_sandboxes::resolve_authorized_workspace_door(
                &key,
                &handle,
                k2_core::hire::limits::ReachDoor::HostSessions
            )
            .is_ok(),
            "legacy key must still host-session the workspace it created"
        );

        let outside = unique_ws("legacy-outside");
        let outside_s = outside.to_string_lossy().into_owned();
        let r = handle_v1_hire(&key, hire_json(&outside_s).as_bytes());
        assert_eq!(r.status, "403 Forbidden", "body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "outside_hire_root", "body={}", r.body);
        assert!(!outside.exists(), "refused path must not be created");
    }

    #[test]
    fn hire_scoped_key_creates_under_root_and_is_ledgered() {
        k2_core::db::init_for_tests();
        let root = crate::hire_routes::test_support::ensure_test_root();
        let limits = k2_core::hire::limits::HireLimits::default().to_json();
        let key = minted_key("hire-v1w", r#"["nothing"]"#, host_and_hire(), Some(&limits));
        let V1Principal::Api(p) = &key else { unreachable!() };
        let path = root.join(format!("v1w-{}", Uuid::new_v4().simple())).to_string_lossy().into_owned();
        let r = handle_v1_hire(&key, hire_json(&path).as_bytes());
        let v = parse_ok(&r);
        let ws_id = v["id"].as_str().expect("id").to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(
            k2_core::hire::ledger::creator_key_of(&conn, &ws_id).as_deref(),
            Some(p.id.as_str()),
            "a key-made workspace is ledgered to the key"
        );
        assert_eq!(k2_core::hire::ledger::live_members(&conn, &p.id), 1);
    }

    #[test]
    fn key_db_access_needs_the_db_door() {
        k2_core::db::init_for_tests();
        let root = crate::hire_routes::test_support::ensure_test_root();
        let limits = k2_core::hire::limits::HireLimits::default().to_json();
        let key = minted_key("hire-v1w-db", "*", host_and_hire(), Some(&limits));
        let path = root.join(format!("v1w-db-{}", Uuid::new_v4().simple())).to_string_lossy().into_owned();
        let body = serde_json::json!({ "path": path, "dbAccess": "write" }).to_string();
        let r = handle_v1_hire(&key, body.as_bytes());
        assert_eq!(r.status, "403 Forbidden", "body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "exceeds_key", "body={}", r.body);
        assert!(!project_exists(&path), "refused before any write");
    }

    #[test]
    fn finite_list_new_path_is_403_usage() {
        k2_core::db::init_for_tests();
        let ws = unique_ws("finite-new");
        let path = ws.to_string_lossy().into_owned();
        let key = apik("k-hire-finite", Some(r#"["other-ws"]"#));
        let r = handle_v1_hire(&key, hire_json(&path).as_bytes());
        assert_eq!(r.status, "403 Forbidden", "body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "usage", "body={}", r.body);
        let hint = v["error"]["hint"].as_str().expect("hint");
        assert!(
            hint.contains('*') || hint.contains("owner"),
            "hint must mention * or owner: {hint}"
        );
        assert!(!ws.exists(), "403 must not mint the directory");
        assert!(!project_exists(&path), "403 must not insert a project row");
    }

    #[test]
    fn missing_host_sessions_cap_is_uniform_404() {
        k2_core::db::init_for_tests();
        let ws = unique_ws("no-cap");
        let path = ws.to_string_lossy().into_owned();
        let key = apik_caps(
            "k-hire-nocap",
            Some("*"),
            k2_core::api_keys::ApiCapabilities {
                host_sessions: false,
                canonical_message: true,
                sandboxes: true,
                db: false,
                hire: false,
            },
        );
        let r = handle_v1_hire(&key, hire_json(&path).as_bytes());
        assert_eq!(r.status, "404 Not Found", "body={}", r.body);
        assert!(
            r.body.contains("no such workspace"),
            "uniform body: {}",
            r.body
        );
        assert!(!project_exists(&path));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn rehire_same_path_converges() {
        k2_core::db::init_for_tests();
        let ws = unique_ws("rehire");
        let path = ws.to_string_lossy().into_owned();
        let first = handle_v1_hire(&V1Principal::Owner, hire_json(&path).as_bytes());
        let v1 = parse_ok(&first);
        assert_eq!(v1["changed"], true);
        let second = handle_v1_hire(&V1Principal::Owner, hire_json(&path).as_bytes());
        let v2 = parse_ok(&second);
        assert_eq!(v2["ok"], true, "body={}", second.body);
        assert_eq!(
            v2["changed"], false,
            "re-hire of the same path must converge, body={}",
            second.body
        );
        assert_eq!(v2["id"], v1["id"]);
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn hire_matcher_accepts_old_copy_and_c2() {
        assert!(k2_core::workspace::lifecycle::is_already_registered(
            "Workspace already registered: /tmp/x"
        ));
        assert!(k2_core::workspace::lifecycle::is_already_registered(
            "workspace already exists Cortana"
        ));
        assert!(!k2_core::workspace::lifecycle::is_already_registered(
            "Failed to add workspace"
        ));
    }

    #[test]
    fn hire_wiki_notes_and_later_post() {
        k2_core::db::init_for_tests();
        let ws = unique_ws("wiki-notes");
        let path = ws.to_string_lossy().into_owned();
        let body = serde_json::json!({
            "path": path,
            "wiki": [{ "id": "X.md", "body": "note-one" }],
        })
        .to_string();
        let r = handle_v1_hire(&V1Principal::Owner, body.as_bytes());
        let v = parse_ok(&r);
        let handle = v["handle"].as_str().expect("handle").to_string();
        let written = fs::read_to_string(ws.join(".k2/wiki/X.md")).expect("X.md");
        assert_eq!(written, "note-one");

        let later = serde_json::json!({ "id": "X.md", "body": "note-two" }).to_string();
        let post = handle_v1_wiki_notes(&V1Principal::Owner, &handle, later.as_bytes());
        let pv = parse_ok(&post);
        assert_eq!(pv["ok"], true);
        assert_eq!(pv["id"], "X.md");
        assert_eq!(pv["path"], ".k2/wiki/X.md");
        let written = fs::read_to_string(ws.join(".k2/wiki/X.md")).expect("X.md after post");
        assert_eq!(written, "note-two");
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn wiki_id_dotdot_and_abs_are_400_usage() {
        k2_core::db::init_for_tests();
        let ws = unique_ws("wiki-jail");
        let path = ws.to_string_lossy().into_owned();
        let r = handle_v1_hire(&V1Principal::Owner, hire_json(&path).as_bytes());
        let v = parse_ok(&r);
        let handle = v["handle"].as_str().expect("handle");
        for id in ["../x.md", "/tmp/x.md", "foo/../../etc.md"] {
            let body = serde_json::json!({ "id": id, "body": "x" }).to_string();
            let post = handle_v1_wiki_notes(&V1Principal::Owner, handle, body.as_bytes());
            assert_eq!(post.status, "400 Bad Request", "id={id} body={}", post.body);
            let ev: serde_json::Value = serde_json::from_str(&post.body).expect("json");
            assert_eq!(ev["error"]["code"], "usage", "id={id} body={}", post.body);
        }
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn hire_context_catalog_and_inline_layers() {
        k2_core::db::init_for_tests();
        let ws = unique_ws("ctx");
        let path = ws.to_string_lossy().into_owned();
        let body = serde_json::json!({
            "path": path,
            "context": ["wiki:hygiene"],
            "layers": [{ "label": "role", "markdown": "# ROLE\n" }],
        })
        .to_string();
        let r = handle_v1_hire(&V1Principal::Owner, body.as_bytes());
        let v = parse_ok(&r);
        let handle = v["handle"].as_str().expect("handle").to_string();
        let layers = v["context"]["layers"]
            .as_array()
            .expect("layers")
            .iter()
            .map(|l| {
                (
                    l["source"].as_str().expect("source").to_string(),
                    l["path"].as_str().expect("path").to_string(),
                )
            })
            .collect::<Vec<_>>();
        assert!(
            layers.iter().any(|(src, _)| src.contains("wiki-hygiene") || src.contains("hygiene")),
            "wiki:hygiene must be stacked: {layers:?}"
        );
        assert!(
            layers.iter().any(|(_, p)| p.contains(".k2/context/role.md")),
            "inline layer file must be stacked: {layers:?}"
        );
        assert!(
            ws.join(".k2/context/role.md").is_file(),
            "inline markdown must land under .k2/context/"
        );

        let listed = handle_v1_context_list(&V1Principal::Owner, &handle);
        let lv = parse_ok(&listed);
        assert_eq!(lv["ok"], true);
        assert!(lv["layers"].as_array().expect("layers").len() >= 2);

        let add = serde_json::json!({ "catalog": "wiki:index" }).to_string();
        let added = handle_v1_context_add(&V1Principal::Owner, &handle, add.as_bytes());
        assert_eq!(added.status, "200 OK", "body={}", added.body);

        let xor = serde_json::json!({
            "catalog": "wiki:home",
            "label": "x",
            "markdown": "y",
        })
        .to_string();
        let bad = handle_v1_context_add(&V1Principal::Owner, &handle, xor.as_bytes());
        assert_eq!(bad.status, "400 Bad Request", "body={}", bad.body);

        let regen = handle_v1_context_regen(&V1Principal::Owner, &handle, b"{}");
        assert_eq!(regen.status, "200 OK", "body={}", regen.body);

        let listed = handle_v1_context_list(&V1Principal::Owner, &handle);
        let lv = parse_ok(&listed);
        let role_id = lv["layers"]
            .as_array()
            .expect("layers")
            .iter()
            .find(|l| l["path"].as_str() == Some(".k2/context/role.md"))
            .and_then(|l| l["id"].as_str())
            .expect("role layer id")
            .to_string();
        let rm = serde_json::json!({ "id": role_id }).to_string();
        let removed = handle_v1_context_remove(&V1Principal::Owner, &handle, rm.as_bytes());
        assert_eq!(removed.status, "200 OK", "body={}", removed.body);
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn existing_ungranted_path_is_uniform_404() {
        k2_core::db::init_for_tests();
        let ws = unique_ws("exist-deny");
        fs::create_dir_all(&ws).expect("mkdir");
        let path = ws.to_string_lossy().into_owned();
        insert_project("v1hire-exist-deny", &path);
        let key = apik("k-hire-exist-deny", Some(r#"["some-other"]"#));
        let r = handle_v1_hire(&key, hire_json(&path).as_bytes());
        assert_eq!(r.status, "404 Not Found", "body={}", r.body);
        assert!(r.body.contains("no such workspace"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn existing_granted_by_basename_converges() {
        k2_core::db::init_for_tests();
        let ws = unique_ws("exist-ok");
        fs::create_dir_all(&ws).expect("mkdir");
        let path = ws.to_string_lossy().into_owned();
        let base = basename_of(&path);
        insert_project(&base, &path);
        let grant = serde_json::json!([base]).to_string();
        let key = apik("k-hire-exist-ok", Some(&grant));
        let r = handle_v1_hire(&key, hire_json(&path).as_bytes());
        assert_eq!(r.status, "200 OK", "body={}", r.body);
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn wiki_and_context_mutations_are_post_only_405() {
        k2_core::db::init_for_tests();
        assert_eq!(
            CliResponse::method_not_allowed().status,
            "405 Method Not Allowed"
        );
        let r = handle_v1_w(&V1Principal::Owner, false, b"");
        assert_eq!(r.status, "405 Method Not Allowed");
        // Capability miss on GET list still 404s (no existence oracle).
        let no_host = apik_caps(
            "k-hire-ctx-cap",
            Some("*"),
            k2_core::api_keys::ApiCapabilities {
                host_sessions: false,
                canonical_message: true,
                sandboxes: true,
                db: false,
                hire: false,
            },
        );
        let listed = handle_v1_context_list(&no_host, "anything");
        assert_eq!(listed.status, "404 Not Found");
        assert!(listed.body.contains("no such workspace"));
        let notes = handle_v1_wiki_notes(&no_host, "anything", b"{}");
        assert_eq!(notes.status, "404 Not Found");
    }

    #[test]
    fn template_xor_persona() {
        k2_core::db::init_for_tests();
        let ws = unique_ws("xor");
        let path = ws.to_string_lossy().into_owned();
        let body = serde_json::json!({
            "path": path,
            "template": "worker",
            "persona": "# x",
        })
        .to_string();
        let r = handle_v1_hire(&V1Principal::Owner, body.as_bytes());
        assert_eq!(r.status, "400 Bad Request", "body={}", r.body);
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn hire_template_writes_persona() {
        k2_core::db::init_for_tests();
        let ws = unique_ws("tmpl");
        let path = ws.to_string_lossy().into_owned();
        let body = serde_json::json!({
            "path": path,
            "template": "worker",
            "name": "Scout",
            "preset": "claude",
            "defaultModel": "opus",
        })
        .to_string();
        let r = handle_v1_hire(&V1Principal::Owner, body.as_bytes());
        let v = parse_ok(&r);
        assert_eq!(v["name"], "Scout");
        let persona = k2_core::workspace::agent_identity::persona_md_in(
            k2_core::workspace::agent_identity::workspace_agent_path(&path),
        );
        let text = fs::read_to_string(&persona).expect("persona");
        assert!(
            text.contains("WORKER agent"),
            "template body missing: {text}"
        );
        let row = lookup_project(&path).expect("row");
        assert_eq!(row.default_agent.as_deref(), Some("claude"));
        assert_eq!(row.default_model.as_deref(), Some("opus"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn hire_db_access_write_persists_and_does_not_create_db() {
        k2_core::db::init_for_tests();
        let ws = unique_ws("db-access");
        let path = ws.to_string_lossy().into_owned();
        assert_eq!(
            k2_core::workspace::settings::db_agent_access_for_path(&path),
            "off"
        );
        let body = serde_json::json!({
            "path": path,
            "dbAccess": "write",
        })
        .to_string();
        let r = handle_v1_hire(&V1Principal::Owner, body.as_bytes());
        let _v = parse_ok(&r);
        assert_eq!(
            k2_core::workspace::settings::db_agent_access_for_path(&path),
            "write"
        );
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sql_databases WHERE project_id = \
                     (SELECT id FROM projects WHERE path = ?1)",
                    rusqlite::params![path],
                    |row| row.get(0),
                )
                .expect("count sql_databases");
            assert_eq!(n, 0, "hire must not mint a database");
        }
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn missing_path_is_400() {
        k2_core::db::init_for_tests();
        let r = handle_v1_hire(&V1Principal::Owner, b"{}");
        assert_eq!(r.status, "400 Bad Request", "body={}", r.body);
    }

    #[test]
    fn relative_path_is_400() {
        k2_core::db::init_for_tests();
        let r = handle_v1_hire(&V1Principal::Owner, br#"{"path":"not/absolute"}"#);
        assert_eq!(r.status, "400 Bad Request", "body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "usage");
    }
}
