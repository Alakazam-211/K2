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

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

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

fn usage_forbidden(hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
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

/// Wildcard grant: `allowed_workspaces` is `*` or a JSON list containing `*`.
fn grant_is_star(principal: &V1Principal) -> bool {
    match principal {
        V1Principal::Owner => true,
        V1Principal::Api(p) => p.authorizes_workspace("*"),
    }
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

struct ProjectRow {
    id: String,
    name: String,
    handle: Option<String>,
    default_agent: Option<String>,
    default_model: Option<String>,
}

fn lookup_project(path: &str) -> Option<ProjectRow> {
    lookup_project_row(path).map(|(_, row)| row)
}

/// Match a hire path to a registered `projects.path`, including macOS
/// `/var` ↔ `/private/var` aliasing after canonicalize.
fn lookup_project_row(path: &str) -> Option<(String, ProjectRow)> {
    for candidate in path_lookup_candidates(path) {
        if let Some(row) = lookup_project_exact(&candidate) {
            return Some((candidate, row));
        }
    }
    None
}

fn lookup_project_exact(path: &str) -> Option<ProjectRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT id, name, handle, default_agent, default_model FROM projects WHERE path = ?1",
        rusqlite::params![path],
        |r| {
            Ok(ProjectRow {
                id: r.get(0)?,
                name: r.get(1)?,
                handle: r.get(2)?,
                default_agent: r.get(3)?,
                default_model: r.get(4)?,
            })
        },
    )
    .ok()
}

fn path_lookup_candidates(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<String>, p: String| {
        if !out.iter().any(|e| e == &p) {
            out.push(p);
        }
    };
    push(&mut out, path.to_string());
    if let Ok(c) = Path::new(path).canonicalize() {
        push(&mut out, c.to_string_lossy().into_owned());
    }
    if let Some(rest) = path.strip_prefix("/private") {
        if rest.starts_with('/') {
            push(&mut out, rest.to_string());
        }
    } else if path.starts_with('/') {
        push(&mut out, format!("/private{path}"));
    }
    out
}

fn expand_hire_path(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("missing path".into());
    }
    let expanded = if t == "~" {
        dirs::home_dir().ok_or_else(|| "cannot resolve ~".to_string())?
    } else if let Some(rest) = t.strip_prefix("~/") {
        let home = dirs::home_dir().ok_or_else(|| "cannot resolve ~".to_string())?;
        home.join(rest)
    } else {
        PathBuf::from(t)
    };
    if !expanded.is_absolute() {
        return Err("path must be absolute or ~".into());
    }
    if expanded.exists() {
        if !expanded.is_dir() {
            return Err(format!("{} exists and is not a directory", expanded.display()));
        }
        return expanded
            .canonicalize()
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| format!("canonicalize: {e}"));
    }
    Ok(expanded.to_string_lossy().into_owned())
}

fn basename_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "workspace".into())
}

fn technical_agent_name(path: &str) -> String {
    let s: String = basename_of(path)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if s.is_empty() {
        "agent".into()
    } else {
        s
    }
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

fn emit_hire_watches() {
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::ProjectsChanged {});
    crate::fs_live::resync_watches();
    crate::charter_compose_watch::resync_watches();
}

// ── Hire body ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HireBody {
    path: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    preset: Option<String>,
    #[serde(default)]
    template: Option<String>,
    #[serde(default)]
    persona: Option<String>,
    #[serde(default)]
    wiki: Option<Vec<WikiNoteIn>>,
    #[serde(default)]
    context: Option<Vec<String>>,
    #[serde(default)]
    layers: Option<Vec<InlineLayerIn>>,
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    no_wiki: Option<bool>,
    /// Persist `db_agent_access` (`off`/`read`/`write`). Default stays off.
    /// Does NOT mint a database — hire calls create separately.
    #[serde(default)]
    db_access: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WikiNoteIn {
    id: String,
    body: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InlineLayerIn {
    label: String,
    markdown: String,
}

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

/// `POST /v1/w` — create or converge a workspace. Does not spawn a PTY.
pub(crate) fn handle_v1_hire(principal: &V1Principal, body: &[u8]) -> CliResponse {
    if let Err(resp) = require_host_sessions(principal) {
        return resp;
    }
    let req: HireBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return usage(format!("invalid JSON body: {e}")),
    };
    let expanded = match expand_hire_path(&req.path) {
        Ok(p) => p,
        Err(e) => return usage(e),
    };
    let (path, existing) = match lookup_project_row(&expanded) {
        Some((registered, row)) => (registered, Some(row)),
        None => (expanded, None),
    };
    let template = req
        .template
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let persona = req
        .persona
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if template.is_some() && persona.is_some() {
        return usage("template and persona are mutually exclusive — pass one or the other");
    }
    let template_body = if let Some(t) = template {
        match hire_template_body(t) {
            Some(b) => Some(b),
            None => {
                return usage(format!(
                    "unknown template '{t}' — valid: worker, manager, qa, researcher"
                ));
            }
        }
    } else {
        None
    };

    let basename = basename_of(&path);
    if let Some(row) = existing.as_ref() {
        let handle = row.handle.as_deref().map(str::trim).filter(|s| !s.is_empty());
        if !authorizes_existing(principal, handle, &basename) {
            return uniform_ws_404();
        }
    } else if !grant_is_star(principal) {
        return usage_forbidden(
            "hire of a new workspace needs * grant or owner token",
        );
    }

    let no_wiki = req.no_wiki.unwrap_or(false);
    let mut changed = false;
    let was_registered = existing.is_some();

    if existing.is_none() {
        let seed = !no_wiki;
        let result = if Path::new(&path).exists() {
            k2_core::workspace::lifecycle::open_workspace_ex(&path, seed, true, false)
        } else {
            // create_workspace_ex mkdirs + registers. Call create_dir_all first
            // only when the parent is missing; create_workspace_ex itself
            // refuses a path that already exists.
            if let Some(parent) = Path::new(&path).parent() {
                if !parent.as_os_str().is_empty() && !parent.exists() {
                    if let Err(e) = fs::create_dir_all(parent) {
                        return usage(format!("create parent: {e}"));
                    }
                }
            }
            k2_core::workspace::lifecycle::create_workspace_ex(&path, seed, true, false)
        };
        match result {
            Ok(_) => changed = true,
            Err(e) => {
                if e.contains("already registered") {
                    // Converge.
                } else {
                    return usage(e);
                }
            }
        }
    }

    if !no_wiki {
        match k2_core::wiki::seed_wiki(Path::new(&path)) {
            Ok(created) => {
                if !created.is_empty() {
                    changed = true;
                }
            }
            Err(e) => return usage(format!("seed wiki: {e}")),
        }
    }

    let agent_name = technical_agent_name(&path);
    let persona_dir = k2_core::workspace::agent_identity::workspace_agent_path(&path);
    let persona_existed =
        k2_core::workspace::agent_identity::persona_present_in(&persona_dir);
    if !persona_existed {
        let (role, agent_type, prompt) = if let Some(body) = template_body {
            let at = if template == Some("manager") {
                Some("manager".to_string())
            } else {
                Some("agent-template".to_string())
            };
            (
                template.unwrap_or("agent").to_string(),
                at,
                Some(body.to_string()),
            )
        } else if let Some(p) = persona {
            ("custom".to_string(), None, Some(p.to_string()))
        } else {
            (agent_name.clone(), None, None)
        };
        match k2_core::workspace::agent::create(
            path.clone(),
            agent_name,
            role,
            prompt,
            agent_type,
        ) {
            Ok(_) => changed = true,
            Err(e) => return usage(e),
        }
    }

    if let Some(name) = req.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let current = k2_core::workspace::display::agent_display_name(&path);
        if current != name {
            if let Err(e) = k2_core::workspace::display::set_agent_display_name(&path, name) {
                return usage(e);
            }
            changed = true;
        }
    }

    let row_now = lookup_project(&path);
    if let Some(preset) = req.preset.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let already = row_now
            .as_ref()
            .and_then(|r| r.default_agent.as_deref())
            .unwrap_or("");
        if already != preset {
            if let Err(e) =
                k2_core::workspace::settings::update_project_setting(&path, "default_agent", preset)
            {
                return usage(e);
            }
            changed = true;
        }
    }
    if let Some(model) = req
        .default_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let already = row_now
            .as_ref()
            .and_then(|r| r.default_model.as_deref())
            .unwrap_or("");
        if already != model {
            if let Err(e) =
                k2_core::workspace::settings::update_project_setting(&path, "default_model", model)
            {
                return usage(e);
            }
            changed = true;
        }
    }

    let mut wiki_notes: Vec<serde_json::Value> = Vec::new();
    if let Some(notes) = req.wiki.as_ref() {
        for n in notes {
            match k2_core::wiki::write_note(Path::new(&path), &n.id, &n.body) {
                Ok(rel) => {
                    wiki_notes.push(serde_json::json!({ "id": n.id, "path": rel }));
                    changed = true;
                }
                Err(e) => return usage(e),
            }
        }
    }

    if let Some(catalogs) = req.context.as_ref() {
        for id in catalogs {
            let id = id.trim();
            if id.is_empty() {
                return usage("context catalog id must not be empty");
            }
            match context_layers::add_layer(&path, None, Some(id), None) {
                Ok(_) => changed = true,
                Err(ContextError::DuplicateLayer(_)) => {}
                Err(e) => return context_err(e),
            }
        }
    }
    if let Some(layers) = req.layers.as_ref() {
        for layer in layers {
            match add_inline_layer(&path, &layer.label, &layer.markdown) {
                Ok(did) => {
                    if did {
                        changed = true;
                    }
                }
                Err(resp) => return resp,
            }
        }
    }

    if let Some(access) = req
        .db_access
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !k2_core::workspace::settings::DB_AGENT_ACCESS_MODES.contains(&access) {
            return usage("dbAccess must be 'off', 'read', or 'write'");
        }
        let already = k2_core::workspace::settings::db_agent_access_for_path(&path);
        if already != access {
            if let Err(e) = k2_core::workspace::settings::update_project_setting(
                &path,
                "db_agent_access",
                access,
            ) {
                return usage(e);
            }
            changed = true;
        }
    }

    if changed || !was_registered {
        emit_hire_watches();
    } else {
        crate::charter_compose_watch::resync_watches();
    }

    let row = match lookup_project(&path) {
        Some(r) => r,
        None => return usage("workspace did not register"),
    };
    let handle = row
        .handle
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&basename)
        .to_string();
    let display = {
        let d = k2_core::workspace::display::agent_display_name(&path);
        if d.is_empty() {
            row.name.clone()
        } else {
            d
        }
    };

    let mut body = serde_json::json!({
        "ok": true,
        "path": path,
        "id": row.id,
        "handle": handle,
        "name": display,
        "changed": changed,
    });
    if !wiki_notes.is_empty() {
        body["wiki"] = serde_json::json!({ "notes": wiki_notes });
    }
    if req.context.is_some() || req.layers.is_some() {
        body["context"] = stack_json(&path);
    }
    CliResponse::ok_json(body.to_string())
}

fn sanitize_layer_label(label: &str) -> Result<String, String> {
    let t = label.trim();
    if t.is_empty() {
        return Err("missing layer label".into());
    }
    if !t
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err(
            "layer label must match [a-zA-Z0-9._-]+ (no spaces or other punctuation)".into(),
        );
    }
    Ok(t.to_string())
}

/// Write `.k2/context/<label>.md` then stack it. Returns whether the stack grew.
fn add_inline_layer(project_path: &str, label: &str, markdown: &str) -> Result<bool, CliResponse> {
    let safe = match sanitize_layer_label(label) {
        Ok(s) => s,
        Err(e) => return Err(usage(e)),
    };
    let rel = format!(".k2/context/{safe}.md");
    let dest = Path::new(project_path).join(&rel);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| usage(format!("mkdir context: {e}")))?;
    }
    fs::write(&dest, markdown).map_err(|e| usage(format!("write context layer: {e}")))?;
    match context_layers::add_layer(project_path, Some(&rel), None, Some(&safe)) {
        Ok(_) => Ok(true),
        Err(ContextError::DuplicateLayer(_)) => Ok(false),
        Err(e) => Err(context_err(e)),
    }
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
        match add_inline_layer(&ws_path, label, markdown) {
            Ok(_) => {
                crate::charter_compose_watch::resync_watches();
                handle_v1_context_list(principal, ws_raw)
            }
            Err(resp) => resp,
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

/// CLI `k2 agent hire --template` archetypes (worker|manager|qa|researcher).
fn hire_template_body(name: &str) -> Option<&'static str> {
    match name {
        "worker" => Some(TEMPLATE_WORKER),
        "manager" => Some(TEMPLATE_MANAGER),
        "qa" => Some(TEMPLATE_QA),
        "researcher" => Some(TEMPLATE_RESEARCHER),
        _ => None,
    }
}

const TEMPLATE_WORKER: &str = r#"## Role

You are a WORKER agent: you execute well-scoped tasks end-to-end and
report the outcome. You report to: __MANAGER__ (replace with your
manager agent's name).

## How you work

- Take ONE task at a time; finish it before picking up the next.
- Verify your own work before reporting done — run it, test it, read
  it back. "It should work" is not done.
- Keep a short running work log in your workspace so a re-launched
  session can pick up your context.
- If you are blocked for more than ~15 minutes, say so — don't spin.

## Do NOT

- Do NOT take on work outside the task's scope without asking first.
- Do NOT answer another agent's human-in-the-loop prompt — back off
  and let its owner handle it.
- Do NOT push to shared branches, rotate credentials, or touch
  infrastructure unless the task explicitly says so.
- Do NOT go silent when blocked or uncertain.

## Escalation

When blocked, uncertain, or the task itself looks wrong: STOP and
message your escalation contact: __ESCALATION_CONTACT__ (replace with
a real agent name or your owner on day one).
"#;

const TEMPLATE_MANAGER: &str = r#"## Role

You are a MANAGER agent: you break goals into tasks, delegate them to
connected worker agents, track progress, and report upward. You report
to: __ESCALATION_CONTACT__ (your operator/owner — replace this).

## How you work

- Keep a live task board (a simple markdown file in this workspace):
  who owns what, current status, next checkpoint.
- Delegate OUTCOMES, not keystrokes — let workers own their approach;
  hold them to the result.
- Follow up on anything you delegated that has gone quiet.
- Summarize status upward on request in under 10 lines.

## Do NOT

- Do NOT do the workers' tasks yourself — if you keep doing a job,
  flag that the team needs a hire instead.
- Do NOT reconfigure or retire agents without your owner's instruction.
- Do NOT answer a worker's human-in-the-loop prompt you don't have
  standing over.
- Do NOT let a task sit unassigned without telling your owner.

## Escalation

Anything outside your delegation authority — hiring, retiring,
spending, credentials, production changes — goes to:
__ESCALATION_CONTACT__ (replace on day one).
"#;

const TEMPLATE_QA: &str = r#"## Role

You are a QA agent: you verify other agents' work against its stated
intent and report findings. You report to: __MANAGER__ (replace with
your manager agent's name).

## How you work

- Start from what the change CLAIMS to do; test the claim first, then
  the edges around it.
- Reproduce before you report — every finding comes with exact steps.
- Rate every finding: blocker / bug / nit. Never inflate severity.
- "Passed" means YOU ran it and watched it work — never mark passed
  on faith.

## Do NOT

- Do NOT fix the code yourself — report to the owning agent; you
  verify, they fix.
- Do NOT approve work you only read but did not run.
- Do NOT answer another agent's human-in-the-loop prompt.
- Do NOT soften findings to be agreeable — precise and neutral beats
  polite and vague.

## Escalation

Blockers the owning agent disputes, or anything that looks like data
loss / security exposure: escalate immediately to
__ESCALATION_CONTACT__ (replace on day one).
"#;

const TEMPLATE_RESEARCHER: &str = r#"## Role

You are a RESEARCHER agent: you investigate questions, compare
options, and produce short, sourced write-ups. You report to:
__MANAGER__ (replace with your manager agent's name).

## How you work

- Restate the question AND the decision it feeds before digging.
- Prefer primary sources; note the retrieval date on anything volatile
  (pricing, versions, benchmarks).
- Deliverable shape: a 5–15 line summary up top, details + sources
  below.
- Say "unknown" when the evidence is thin — a marked gap is worth
  more than a confident guess.

## Do NOT

- Do NOT modify code, configs, or infrastructure — you produce
  knowledge, not changes.
- Do NOT present speculation as fact; label confidence explicitly.
- Do NOT pad reports — if it fits in 10 lines, deliver 10 lines.
- Do NOT answer another agent's human-in-the-loop prompt.

## Escalation

Questions you cannot source, or requests that would need real
spending/credentials to answer: escalate to __ESCALATION_CONTACT__
(replace on day one).
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::http::V1Principal;
    use std::fs;
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

    #[test]
    fn star_grant_hires() {
        k2_core::db::init_for_tests();
        let ws = unique_ws("star-hire");
        let path = ws.to_string_lossy().into_owned();
        let key = apik("k-hire-star", Some("*"));
        let r = handle_v1_hire(&key, hire_json(&path).as_bytes());
        let v = parse_ok(&r);
        assert_eq!(v["ok"], true);
        assert!(project_exists(&path));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn star_in_json_list_hires() {
        k2_core::db::init_for_tests();
        let ws = unique_ws("star-list");
        let path = ws.to_string_lossy().into_owned();
        let key = apik("k-hire-star-list", Some(r#"["*"]"#));
        let r = handle_v1_hire(&key, hire_json(&path).as_bytes());
        assert_eq!(r.status, "200 OK", "body={}", r.body);
        let _ = fs::remove_dir_all(&ws);
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
