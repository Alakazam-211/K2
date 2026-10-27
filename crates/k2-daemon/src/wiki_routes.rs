//! Daemon-side `/cli/wiki/*` routes — workspace knowledge base (brain map).
//!
//! GET: index, note, serve status. POST: seed, serve on/off, public chat off.
//! When serve is enabled, binds a localhost site on `127.0.0.1` only
//! (no daemon token): SPA + `/api/index` + `/api/note`.
//!
//! ## Site lock (W0, 0.46.0)
//!
//! The site answers only when the `Host` header is this listener's own
//! loopback `host:port` or a nested Connect subdomain whose target is this
//! listener (the published wiki). Anything else is refused before routing
//! (DNS-rebinding guard). There is no CORS: no `Access-Control-Allow-*`
//! header is ever sent, and a request whose `Origin` is not the requested
//! host is refused. JSON never carries absolute filesystem paths, and note
//! ids under dot-folders (e.g. `.archive/`) are not served.
//!
//! ## Public chat (disabled — W0, 0.46.0)
//!
//! Agent-backed public chat let an anonymous visitor start this
//! workspace's real agent, unsandboxed, with auto-approve forced. It is off
//! everywhere until it comes back on the K2 runtime:
//! - the site's `/api/chat*` routes answer `410` and never start a session;
//! - `POST /cli/wiki/chat` (on) answers `410 public_chat_unavailable`;
//!   off still works;
//! - serve status always reports `publicChatEnabled:false`;
//! - [`boot_heal_public_chat`] turns any leftover opt-in off at daemon
//!   start, reverts `api_skip_permissions` on those workspaces, revokes the
//!   daemon-held chat keys and writes an auth-audit line per change.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::cli_response::CliResponse;
use k2_core::skin::SkinPass;

/// Error code for every refused public-chat request (CLI exit 3).
pub const PUBLIC_CHAT_UNAVAILABLE: &str = "public_chat_unavailable";
/// Visitor- and owner-facing reason public chat is off.
pub const PUBLIC_CHAT_UNAVAILABLE_HINT: &str =
    "Public chat is coming back with the K2 runtime. It is off on every workspace for now.";

// ── Serve state (process-wide, per workspace) ──────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServeStatus {
    pub enabled: bool,
    pub port: Option<u16>,
    pub url: Option<String>,
    pub workspace_path: String,
    /// Always `false` (W0): public chat is disabled until the runtime.
    pub public_chat_enabled: bool,
    /// Always `false` (W0).
    pub public_chat_ready: bool,
    /// Always `false` (W0): tells clients the toggle is gone, not just off.
    pub public_chat_available: bool,
    /// Kept for older clients; never set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_chat_error: Option<String>,
}

struct ServeEntry {
    port: u16,
    /// `true` = cancel requested.
    cancel: watch::Sender<bool>,
}

fn servers() -> &'static Mutex<HashMap<String, ServeEntry>> {
    static S: OnceLock<Mutex<HashMap<String, ServeEntry>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn workspace_key(workspace: &Path) -> String {
    dunce::canonicalize(&workspace)
        .unwrap_or_else(|_| workspace.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn status_with(key: String, port: Option<u16>) -> ServeStatus {
    ServeStatus {
        enabled: port.is_some(),
        port,
        url: port.map(|p| format!("http://127.0.0.1:{p}")),
        workspace_path: key,
        public_chat_enabled: false,
        public_chat_ready: false,
        public_chat_available: false,
        public_chat_error: None,
    }
}

fn status_for(workspace: &Path) -> ServeStatus {
    let key = workspace_key(workspace);
    let port = {
        let guard = servers().lock().unwrap_or_else(|e| e.into_inner());
        guard.get(&key).map(|e| e.port)
    };
    status_with(key, port)
}

fn stop_server(workspace: &Path) {
    let key = workspace_key(workspace);
    let mut guard = servers().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = guard.remove(&key) {
        let _ = entry.cancel.send(true);
    }
}

fn start_server(workspace: PathBuf, port: u16) -> Result<ServeStatus, String> {
    stop_server(&workspace);

    let handle = tokio::runtime::Handle::try_current()
        .map_err(|_| "wiki serve requires the daemon tokio runtime".to_string())?;

    let (cancel_tx, cancel_rx) = watch::channel(false);
    let ws = workspace.clone();

    let (listener, actual_port) = handle.block_on(async {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|e| format!("bind 127.0.0.1:{port}: {e}"))?;
        let actual = listener
            .local_addr()
            .map_err(|e| format!("local_addr: {e}"))?
            .port();
        Ok::<_, String>((listener, actual))
    })?;

    let key = workspace_key(&workspace);
    {
        let mut guard = servers().lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(
            key.clone(),
            ServeEntry {
                port: actual_port,
                cancel: cancel_tx,
            },
        );
    }

    handle.spawn(async move {
        run_wiki_site(ws, listener, actual_port, cancel_rx).await;
    });

    Ok(status_with(key, Some(actual_port)))
}

// ── Public chat: disabled (W0) + boot heal ─────────────────────────────

/// What [`heal_public_chat_for`] changed on one workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicChatHeal {
    pub path: String,
    /// `api_skip_permissions` was on (1, or NULL = default on) and is now 0.
    pub skip_permissions_reverted: bool,
}

/// Turn public chat off on one `projects` row that has it on, and revert
/// its `api_skip_permissions`.
///
/// **Rule.** Turning chat on always wrote `api_skip_permissions=1`, and
/// nothing records who set it (the product default is also on), so the
/// two cannot be told apart. Rule used: for every workspace whose
/// `wiki_public_chat` is 1, set `wiki_public_chat=0` and, when
/// `api_skip_permissions` reads on (1, or NULL which reads as the default
/// on), set it to 0. Workspaces with chat off are never touched. Each
/// change writes one `wiki-public-chat-heal` line to the auth audit log
/// (`~/.k2/auth-audit.jsonl`, `k2 users audit`). The owner can turn
/// skip-permissions back on with `k2 workspace set`.
///
/// Returns `None` when the row is unknown or chat is already off
/// (idempotent).
pub fn heal_public_chat_for(project_path: &str, ingress: &str) -> Option<PublicChatHeal> {
    let (name, skip_on) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let row = conn.query_row(
            "SELECT name, api_skip_permissions FROM projects \
             WHERE path = ?1 AND wiki_public_chat = 1",
            rusqlite::params![project_path],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<i64>>(1)?)),
        );
        let (name, skip) = match row {
            Ok(v) => v,
            Err(_) => return None,
        };
        let skip_on = skip.map(|v| v != 0).unwrap_or(true);
        let sql = if skip_on {
            "UPDATE projects SET wiki_public_chat = 0, api_skip_permissions = 0 WHERE path = ?1"
        } else {
            "UPDATE projects SET wiki_public_chat = 0 WHERE path = ?1"
        };
        if let Err(e) = conn.execute(sql, rusqlite::params![project_path]) {
            k2_core::log_debug!(
                "[wiki] ERROR could not turn public chat off for {project_path}: {e}"
            );
            return None;
        }
        (name, skip_on)
    };
    let outcome = if skip_on {
        "public_chat_off;api_skip_permissions_reverted"
    } else {
        "public_chat_off;api_skip_permissions_already_off"
    };
    let who = name
        .filter(|n| !n.trim().is_empty())
        .or_else(|| k2_core::wiki_public_chat::primary_slug(project_path))
        .unwrap_or_else(|| "workspace".into());
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "wiki-public-chat-heal",
        &who,
        outcome,
        ingress,
        "-",
        "daemon",
    ));
    k2_core::log_debug!("[wiki] public chat turned off for {project_path} ({outcome})");
    Some(PublicChatHeal {
        path: project_path.to_string(),
        skip_permissions_reverted: skip_on,
    })
}

/// Result of [`boot_heal_public_chat`].
#[derive(Debug, Clone, Default)]
pub struct PublicChatBootHeal {
    pub workspaces: Vec<PublicChatHeal>,
    pub keys_revoked: Vec<String>,
}

/// Idempotent daemon-start heal (W0): every workspace with public chat on
/// is switched off per [`heal_public_chat_for`], and every daemon-held
/// wiki chat key is revoked (an injected agent could have read it). A
/// second run finds nothing to do.
pub fn boot_heal_public_chat() -> PublicChatBootHeal {
    let paths: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut out = Vec::new();
        match conn.prepare("SELECT path FROM projects WHERE wiki_public_chat = 1") {
            Ok(mut stmt) => match stmt.query_map([], |r| r.get::<_, String>(0)) {
                Ok(rows) => out.extend(rows.flatten()),
                Err(e) => k2_core::log_debug!("[wiki] ERROR public chat heal query: {e}"),
            },
            Err(e) => k2_core::log_debug!("[wiki] ERROR public chat heal prepare: {e}"),
        }
        out
    };
    let workspaces = paths
        .iter()
        .filter_map(|p| heal_public_chat_for(p, "boot"))
        .collect();
    let keys_revoked = k2_core::wiki_public_chat::revoke_all_chat_keys();
    if !keys_revoked.is_empty() {
        k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
            "wiki-public-chat-heal",
            "-",
            format!("chat_keys_revoked:{}", keys_revoked.len()),
            "boot",
            "-",
            "daemon",
        ));
    }
    PublicChatBootHeal {
        workspaces,
        keys_revoked,
    }
}

fn public_chat_unavailable_response() -> CliResponse {
    CliResponse {
        status: "410 Gone",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": PUBLIC_CHAT_UNAVAILABLE,
                "hint": PUBLIC_CHAT_UNAVAILABLE_HINT,
            },
        })
        .to_string(),
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

fn need_project_path(params: &HashMap<String, String>) -> Result<PathBuf, CliResponse> {
    // Prefer `project` over `project_path` (same as inbox_routes).
    for key in &["project", "project_path"] {
        if let Some(v) = params.get(*key) {
            if !v.is_empty() {
                return Ok(PathBuf::from(v));
            }
        }
    }
    Err(CliResponse::bad_request(
        "Missing project (or project_path) parameter",
    ))
}

fn str_param(params: &HashMap<String, String>, key: &str) -> String {
    params.get(key).cloned().unwrap_or_default()
}

fn bool_param(params: &HashMap<String, String>, key: &str) -> bool {
    matches!(
        params.get(key).map(|v| v.as_str()),
        Some("1") | Some("true") | Some("on") | Some("yes")
    )
}

// ── GET handlers ───────────────────────────────────────────────────────

/// Skin wiki GET index. Owner/Connect use [`handle_index`].
pub fn handle_index_gated(params: &HashMap<String, String>, skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_index(params, &pass),
        None => handle_index(params),
    }
}

pub fn handle_note_gated(params: &HashMap<String, String>, skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_note(params, &pass),
        None => handle_note(params),
    }
}

/// GET /cli/wiki/index?project=<path>
/// GET /cli/wiki/index?scope=k2 — fleet map (all workspace brains; host registry).
pub fn handle_index(params: &HashMap<String, String>) -> CliResponse {
    let scope = str_param(params, "scope").to_ascii_lowercase();
    if scope == "k2" || scope == "host" || scope == "fleet" {
        return match k2_core::wiki::build_fleet_index() {
            Ok(idx) => CliResponse::ok_json(
                serde_json::to_string(&idx).unwrap_or_else(|_| "{}".to_string()),
            ),
            Err(e) => CliResponse::bad_request(e),
        };
    }
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match k2_core::wiki::build_index(&workspace) {
        Ok(idx) => CliResponse::ok_json(
            serde_json::to_string(&idx).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// GET /cli/wiki/note?project=<path>&id=<wiki-rel-id>
/// Fleet notes: id = `{workspaceId}::{noteId}` (project optional).
pub fn handle_note(params: &HashMap<String, String>) -> CliResponse {
    let id = str_param(params, "id");
    if id.is_empty() {
        return CliResponse::bad_request("Missing id");
    }
    let workspace = need_project_path(params).ok();
    match k2_core::wiki::read_note_fleet_or_local(workspace.as_deref(), &id) {
        Ok(note) => CliResponse::ok_json(
            serde_json::to_string(&note).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

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

/// Handle or uuid only. Folder-basename / abs path / unknown → 403 `skin_room`.
fn skin_resolve_project(pass: &SkinPass, token: &str) -> Result<(String, PathBuf), CliResponse> {
    let token = token.trim();
    if token.is_empty() {
        return Err(crate::skin_routes::skin_room_response());
    }
    let ids = match k2_core::skin::resolve_room_tokens(&[token.to_string()]) {
        Ok(ids) if ids.len() == 1 => ids,
        _ => return Err(crate::skin_routes::skin_room_response()),
    };
    let project_id = ids[0].clone();
    if !pass.has_room(&project_id) {
        return Err(crate::skin_routes::skin_room_response());
    }
    let Some(path) = lookup_project_path(&project_id) else {
        return Err(crate::skin_routes::skin_room_response());
    };
    Ok((project_id, path))
}

fn lookup_project_path(project_id: &str) -> Option<PathBuf> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT path FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .map(PathBuf::from)
}

fn skin_require_cap(pass: &SkinPass, project_id: &str, cap: &str) -> Result<(), CliResponse> {
    if !pass.has_cap_in_room(project_id, cap) {
        return Err(crate::skin_routes::missing_cap_response(cap));
    }
    Ok(())
}

fn skin_wiki_id_forbidden(id: &str) -> bool {
    id.contains(k2_core::wiki::FLEET_ID_SEP)
        || id.contains("__focusgroup__")
        || id.contains("__project__")
        || id.contains("__workspace__")
}

fn need_skin_project(params: &HashMap<String, String>) -> Result<String, CliResponse> {
    params
        .get("project")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| usage_error("missing 'project' (workspace handle or UUID)"))
}

fn handle_skin_index(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let scope = str_param(params, "scope").to_ascii_lowercase();
    if scope == "k2" || scope == "host" || scope == "fleet" {
        return crate::skin_routes::skin_room_response();
    }
    let project = match need_skin_project(params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let (project_id, path) = match skin_resolve_project(pass, &project) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = skin_require_cap(pass, &project_id, crate::skin_routes::WIKI_READ) {
        return e;
    }
    match k2_core::wiki::build_index(&path) {
        Ok(idx) => CliResponse::ok_json(
            serde_json::to_string(&idx).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

fn handle_skin_note(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let project = match need_skin_project(params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let id = str_param(params, "id");
    if id.is_empty() {
        return usage_error("missing 'id' (wiki-rel note id)");
    }
    if skin_wiki_id_forbidden(&id) {
        return crate::skin_routes::skin_room_response();
    }
    let (project_id, path) = match skin_resolve_project(pass, &project) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = skin_require_cap(pass, &project_id, crate::skin_routes::WIKI_READ) {
        return e;
    }
    match k2_core::wiki::read_note(&path, &id) {
        Ok(note) => CliResponse::ok_json(
            serde_json::to_string(&note).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// GET /cli/wiki/serve/status?project=<path>
/// Also exposed as GET /cli/wiki/status.
pub fn handle_serve_status(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let st = status_for(&workspace);
    CliResponse::ok_json(serde_json::to_string(&st).unwrap_or_else(|_| "{}".to_string()))
}

// ── POST handlers ──────────────────────────────────────────────────────

/// POST /cli/wiki/seed?project=<path>
pub fn handle_seed_post(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match k2_core::wiki::seed_wiki(&workspace) {
        Ok(created) => CliResponse::ok_json(
            serde_json::json!({
                "success": true,
                "created": created,
                "wikiRel": k2_core::wiki::WIKI_REL,
            })
            .to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// POST /cli/wiki/serve — query/form: enabled=true|false, port? (0 = ephemeral).
/// Optional: publicChat / chat / wikiPublicChat — `off` turns public chat
/// off, `on` is refused (same semantics as POST /cli/wiki/chat).
pub fn handle_serve_post(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Optional public-chat toggle batched with serve.
    if let Some(resp) = apply_public_chat_param(&workspace, params) {
        return resp;
    }

    // Accept enabled= / action= / path-style via `mode` param.
    let mode = str_param(params, "mode").to_ascii_lowercase();
    let action = str_param(params, "action").to_ascii_lowercase();
    let enabled = if !mode.is_empty() {
        matches!(mode.as_str(), "on" | "start" | "true" | "1")
    } else if !action.is_empty() {
        matches!(action.as_str(), "on" | "start" | "true" | "1")
    } else if params.contains_key("enabled") {
        bool_param(params, "enabled")
    } else {
        return CliResponse::bad_request("Missing enabled (true/false) or mode=on|off");
    };

    if !enabled {
        stop_server(&workspace);
        let st = status_for(&workspace);
        return CliResponse::ok_json(
            serde_json::to_string(&st).unwrap_or_else(|_| "{}".to_string()),
        );
    }

    let port: u16 = str_param(params, "port")
        .parse()
        .unwrap_or(0);

    match start_server(workspace, port) {
        Ok(st) => CliResponse::ok_json(
            serde_json::to_string(&st).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// If params carry publicChat / public_chat / chat: `on` is refused with
/// [`PUBLIC_CHAT_UNAVAILABLE`] (W0); `off` turns it off.
/// Returns `Some(err_response)` on refusal/failure; `None` when absent or OK.
fn apply_public_chat_param(
    workspace: &Path,
    params: &HashMap<String, String>,
) -> Option<CliResponse> {
    let key = [
        "publicChat",
        "public_chat",
        "chat",
        "wikiPublicChat",
        "wiki_public_chat",
    ]
    .iter()
    .find(|k| params.contains_key(**k))
    .copied();
    let Some(k) = key else {
        return None;
    };
    if bool_param(params, k) {
        return Some(public_chat_unavailable_response());
    }
    match set_public_chat_off(workspace) {
        Ok(()) => None,
        Err(e) => Some(CliResponse::bad_request(e)),
    }
}

/// Turn public chat off (the only direction left, W0). When chat was on,
/// applies the same rule as the boot heal ([`heal_public_chat_for`]):
/// `api_skip_permissions` is reverted too and the change is audited.
/// Unregistered paths fail loudly.
fn set_public_chat_off(workspace: &Path) -> Result<(), String> {
    let key = workspace_key(workspace);
    let project_path = resolve_project_path_for_settings(&key);
    if heal_public_chat_for(&project_path, "owner").is_some() {
        return Ok(());
    }
    k2_core::workspace::settings::update_project_setting(&project_path, "wiki_public_chat", "0")
}

/// Map a workspace filesystem path to the `projects.path` string used for
/// settings. Falls back to the given path when unregistered (update will 400).
fn resolve_project_path_for_settings(workspace_path: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Ok(p) = conn.query_row(
        "SELECT path FROM projects WHERE path = ?1 LIMIT 1",
        rusqlite::params![workspace_path],
        |r| r.get::<_, String>(0),
    ) {
        return p;
    }
    let want = dunce::canonicalize(&Path::new(workspace_path))
        .unwrap_or_else(|_| PathBuf::from(workspace_path));
    let mut stmt = match conn.prepare("SELECT path FROM projects") {
        Ok(s) => s,
        Err(_) => return workspace_path.to_string(),
    };
    let rows = match stmt.query_map([], |r| r.get::<_, String>(0)) {
        Ok(r) => r,
        Err(_) => return workspace_path.to_string(),
    };
    for p in rows.flatten() {
        let got = dunce::canonicalize(&Path::new(&p))
            .unwrap_or_else(|_| PathBuf::from(&p));
        if got == want {
            return p;
        }
    }
    workspace_path.to_string()
}

/// POST /cli/wiki/chat — enabled=true|false or mode=on|off.
/// W0 (0.46.0): `on` always answers `410 public_chat_unavailable` and
/// changes nothing; `off` still works (and reverts skip-permissions when
/// chat was on). Response for off is ServeStatus.
pub fn handle_chat_post(params: &HashMap<String, String>) -> CliResponse {
    let mode = str_param(params, "mode").to_ascii_lowercase();
    let action = str_param(params, "action").to_ascii_lowercase();
    let enabled = if !mode.is_empty() {
        matches!(mode.as_str(), "on" | "start" | "true" | "1" | "enable")
    } else if !action.is_empty() {
        matches!(action.as_str(), "on" | "start" | "true" | "1" | "enable")
    } else if params.contains_key("enabled") {
        bool_param(params, "enabled")
    } else if params.contains_key("publicChat") {
        bool_param(params, "publicChat")
    } else {
        return CliResponse::bad_request("Missing enabled (true/false) or mode=on|off");
    };
    if enabled {
        return public_chat_unavailable_response();
    }

    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match set_public_chat_off(&workspace) {
        Ok(()) => {
            let st = status_for(&workspace);
            CliResponse::ok_json(serde_json::to_string(&st).unwrap_or_else(|_| "{}".to_string()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

// ── POST dispatcher ────────────────────────────────────────────────────

pub fn dispatch_post(path: &str, params: &HashMap<String, String>) -> CliResponse {
    match path {
        "/cli/wiki/seed" => handle_seed_post(params),
        "/cli/wiki/serve" => handle_serve_post(params),
        // Convenience aliases
        "/cli/wiki/serve/on" => {
            let mut p = params.clone();
            p.insert("enabled".into(), "true".into());
            handle_serve_post(&p)
        }
        "/cli/wiki/serve/off" => {
            let mut p = params.clone();
            p.insert("enabled".into(), "false".into());
            handle_serve_post(&p)
        }
        "/cli/wiki/chat" => handle_chat_post(params),
        "/cli/wiki/chat/on" => {
            let mut p = params.clone();
            p.insert("enabled".into(), "true".into());
            handle_chat_post(&p)
        }
        "/cli/wiki/chat/off" => {
            let mut p = params.clone();
            p.insert("enabled".into(), "false".into());
            handle_chat_post(&p)
        }
        _ => CliResponse::not_found(),
    }
}

// ── Localhost site (127.0.0.1 only, no token) ──────────────────────────

async fn run_wiki_site(
    workspace: PathBuf,
    listener: TcpListener,
    port: u16,
    mut cancel: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = cancel.changed() => {
                if *cancel.borrow() {
                    break;
                }
            }
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _)) => {
                        let ws = workspace.clone();
                        tokio::spawn(async move {
                            handle_site_conn(ws, port, stream).await;
                        });
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

/// First value of header `name` (case-insensitive) in a raw request head.
fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        if k.trim().eq_ignore_ascii_case(name) {
            Some(v.trim())
        } else {
            None
        }
    })
}

/// `host:port` → (`host`, `Some(port)`); bracketed IPv6 kept with brackets.
fn split_host_port(authority: &str) -> (String, Option<u16>) {
    let a = authority.trim().trim_end_matches('/').to_ascii_lowercase();
    if let Some(rest) = a.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let host = format!("[{}]", &rest[..end]);
            let port = rest[end + 1..].strip_prefix(':').and_then(|p| p.parse().ok());
            return (host, port);
        }
    }
    match a.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => (h.to_string(), p.parse().ok()),
        _ => (a, None),
    }
}

fn is_loopback_name(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1")
}

/// Does a Connect subdomain target (`127.0.0.1:7420`, `localhost:7420`,
/// `http://127.0.0.1:7420/`) point at this wiki listener?
fn target_points_at_port(target: &str, port: u16) -> bool {
    let t = target.trim();
    let t = t
        .strip_prefix("http://")
        .or_else(|| t.strip_prefix("https://"))
        .unwrap_or(t);
    let authority = t.split('/').next().unwrap_or("");
    let (host, p) = split_host_port(authority);
    is_loopback_name(&host) && p == Some(port)
}

/// DNS-rebinding guard. Allowed `Host` values: this listener's loopback
/// `host:port` (`127.0.0.1`, `localhost`, `[::1]`), or a nested Connect
/// subdomain (`<label>.<you>.k2.dev`) whose routing target is this
/// listener — the published wiki. Everything else (an attacker's domain
/// rebound to 127.0.0.1, another port, no Host) is refused.
fn site_host_allowed(
    host: Option<&str>,
    port: u16,
    map: &k2_core::tunnel::subdomains::SubdomainMap,
) -> bool {
    let Some(host) = host.map(str::trim).filter(|h| !h.is_empty()) else {
        return false;
    };
    let (name, p) = split_host_port(host);
    if is_loopback_name(&name) {
        return p == Some(port);
    }
    match map.route_for_host(&name) {
        k2_core::tunnel::subdomains::Route::Internal(target) => {
            target_points_at_port(&target, port)
        }
        _ => false,
    }
}

/// Same-origin only: a request that names an `Origin` must name the host
/// it was sent to. No `Origin` (plain navigation, same-origin GET) is fine.
fn site_origin_allowed(origin: Option<&str>, host: &str) -> bool {
    let Some(origin) = origin.map(str::trim).filter(|o| !o.is_empty()) else {
        return true;
    };
    let rest = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"));
    match rest {
        Some(authority) => authority
            .trim_end_matches('/')
            .eq_ignore_ascii_case(host.trim()),
        None => false, // `null`, odd schemes
    }
}

/// Public site JSON never carries absolute filesystem paths: drop every
/// `workspacePath` key (top level and per node) before it leaves.
fn scrub_site_json(mut v: serde_json::Value) -> String {
    fn scrub(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(map) => {
                map.remove("workspacePath");
                for (_, child) in map.iter_mut() {
                    scrub(child);
                }
            }
            serde_json::Value::Array(items) => {
                for child in items.iter_mut() {
                    scrub(child);
                }
            }
            _ => {}
        }
    }
    scrub(&mut v);
    v.to_string()
}

/// Error text for the public site with the workspace path cut out.
fn site_error(workspace: &Path, e: &str) -> String {
    let mut out = e.to_string();
    for p in [workspace_key(workspace), workspace.to_string_lossy().into_owned()] {
        if !p.is_empty() {
            out = out.replace(&p, "<workspace>");
        }
    }
    out
}

/// Note ids under dot-folders (`.archive/x.md`) or dot-files are hidden
/// from the index, so the site does not serve them either.
fn site_note_id_hidden(id: &str) -> bool {
    id.replace('\\', "/")
        .split('/')
        .any(|seg| seg.starts_with('.'))
}

async fn handle_site_conn(workspace: PathBuf, port: u16, mut stream: tokio::net::TcpStream) {
    let mut buf = vec![0u8; 64 * 1024];
    let n = match stream.read(&mut buf).await {
        Ok(0) | Err(_) => return,
        Ok(n) => n,
    };
    let raw = &buf[..n];
    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4);
    let header_bytes = match header_end {
        Some(end) => &raw[..end],
        None => raw,
    };
    let head = String::from_utf8_lossy(header_bytes).into_owned();
    let first = head.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    let host = header_value(&head, "host").map(str::to_string);
    let map = k2_core::tunnel::subdomains::current();
    if !site_host_allowed(host.as_deref(), port, &map) {
        write_site_response(
            &mut stream,
            "421 Misdirected Request",
            "application/json",
            r#"{"error":"host not allowed"}"#,
        )
        .await;
        return;
    }
    if !site_origin_allowed(header_value(&head, "origin"), host.as_deref().unwrap_or("")) {
        write_site_response(
            &mut stream,
            "403 Forbidden",
            "application/json",
            r#"{"error":"cross-origin requests are not allowed"}"#,
        )
        .await;
        return;
    }

    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.clone(), String::new()),
    };

    // Public chat (W0): gone until the runtime. Never starts a session.
    if path == "/api/chat" || path.starts_with("/api/chat/") {
        if path == "/api/chat/status" && (method == "GET" || method == "HEAD") {
            let body = serde_json::json!({
                "enabled": false,
                "reason": PUBLIC_CHAT_UNAVAILABLE_HINT,
                "code": PUBLIC_CHAT_UNAVAILABLE,
            })
            .to_string();
            write_site_response(&mut stream, "200 OK", "application/json", &body).await;
            return;
        }
        let body = serde_json::json!({
            "error": PUBLIC_CHAT_UNAVAILABLE_HINT,
            "code": PUBLIC_CHAT_UNAVAILABLE,
            "enabled": false,
        })
        .to_string();
        write_site_response(&mut stream, "410 Gone", "application/json", &body).await;
        return;
    }

    if method != "GET" && method != "HEAD" {
        write_site_response(
            &mut stream,
            "405 Method Not Allowed",
            "application/json",
            r#"{"error":"method not allowed"}"#,
        )
        .await;
        return;
    }

    match path.as_str() {
        "/api/index" => {
            let body = match tokio::task::spawn_blocking({
                let ws = workspace.clone();
                move || k2_core::wiki::build_index(&ws)
            })
            .await
            {
                Ok(Ok(idx)) => match serde_json::to_value(&idx) {
                    Ok(v) => scrub_site_json(v),
                    Err(_) => "{}".into(),
                },
                Ok(Err(e)) => {
                    write_site_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        &serde_json::json!({"error": site_error(&workspace, &e)}).to_string(),
                    )
                    .await;
                    return;
                }
                Err(e) => {
                    write_site_response(
                        &mut stream,
                        "500 Internal Server Error",
                        "application/json",
                        &serde_json::json!({"error": format!("join: {e}")}).to_string(),
                    )
                    .await;
                    return;
                }
            };
            write_site_response(&mut stream, "200 OK", "application/json", &body).await;
        }
        "/api/note" => {
            let id = query_param(&query, "id").unwrap_or_default();
            if id.is_empty() {
                write_site_response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    r#"{"error":"Missing id"}"#,
                )
                .await;
                return;
            }
            // Percent-decode minimal id
            let id = percent_decode(id);
            if site_note_id_hidden(&id) {
                write_site_response(
                    &mut stream,
                    "404 Not Found",
                    "application/json",
                    r#"{"error":"note not found"}"#,
                )
                .await;
                return;
            }
            let body = match tokio::task::spawn_blocking({
                let ws = workspace.clone();
                let id = id.clone();
                move || k2_core::wiki::read_note(&ws, &id)
            })
            .await
            {
                Ok(Ok(note)) => match serde_json::to_value(&note) {
                    Ok(v) => scrub_site_json(v),
                    Err(_) => "{}".into(),
                },
                Ok(Err(e)) => {
                    write_site_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        &serde_json::json!({"error": site_error(&workspace, &e)}).to_string(),
                    )
                    .await;
                    return;
                }
                Err(e) => {
                    write_site_response(
                        &mut stream,
                        "500 Internal Server Error",
                        "application/json",
                        &serde_json::json!({"error": format!("join: {e}")}).to_string(),
                    )
                    .await;
                    return;
                }
            };
            write_site_response(&mut stream, "200 OK", "application/json", &body).await;
        }
        "/" | "/index.html" => {
            write_site_response(
                &mut stream,
                "200 OK",
                "text/html; charset=utf-8",
                SITE_HTML,
            )
            .await;
        }
        _ => {
            write_site_response(
                &mut stream,
                "404 Not Found",
                "application/json",
                r#"{"error":"not found"}"#,
            )
            .await;
        }
    }
}

/// One site response. No `Access-Control-*` headers: the site is
/// same-origin only (W0).
fn site_response_bytes(status: &str, ct: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {ct}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         {frame}\
         Connection: close\r\n\r\n{body}",
        body.len(),
        frame = k2_core::frame_policy::CONNECT_HEADER_LINES,
    )
}

async fn write_site_response(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    ct: &str,
    body: &str,
) {
    let resp = site_response_bytes(status, ct, body);
    let _ = stream.write_all(resp.as_bytes()).await;
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(v);
            }
        } else if pair == key {
            return Some("");
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = || {
                let a = (bytes[i + 1] as char).to_digit(16)?;
                let b = (bytes[i + 2] as char).to_digit(16)?;
                Some((a * 16 + b) as u8)
            };
            if let Some(b) = h() {
                out.push(b);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Minimal live SPA: list + simple force graph, no CDN deps. Polls /api/index.
///
/// Stability notes (0.40.47+):
/// - Do **not** rebuild the sim when `generatedAt` changes — that timestamp
///   is regenerated every poll even when the graph is unchanged, which used
///   to re-scatter nodes every 2s.
/// - Merge by node id so positions survive polls; only reheat on structure
///   change. Simulation cools down so layout settles instead of thrashing.
const SITE_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>K2 Wiki</title>
<style>
  /* K2 dark system tokens (mirrors app --color-* defaults) */
  :root {
    color-scheme: dark;
    --bg: #0a0a0a;
    --panel: #141414;
    --elevated: #1e1e1e;
    --fg: #e4e4e7;
    --secondary: #a1a1aa;
    --muted: #71717a;
    --accent: #3b82f6;
    --accent-soft: #93c5fd;
    --border: #2a2a2a;
    --miss: #f87171;
    --node: #e4e4e7;
  }
  * { box-sizing: border-box; }
  html, body { height:100%; }
  body {
    margin:0;
    font: 13px/1.45 ui-monospace, 'SF Mono', Menlo, Monaco, Consolas, monospace;
    background:var(--bg); color:var(--fg);
    height:100vh; display:flex; flex-direction:column; overflow:hidden;
  }
  header {
    display:flex; gap:12px; align-items:center;
    padding:8px 12px; border-bottom:1px solid var(--border);
    background:var(--panel); flex-shrink:0;
  }
  header h1 {
    font-size:11px; margin:0; font-weight:700;
    letter-spacing:0.12em; text-transform:uppercase; color:var(--muted);
    flex-shrink:0;
  }
  header .search-wrap { display:flex; align-items:center; gap:10px; flex:1; min-width:0; max-width:520px; }
  input[type=search] {
    flex:1; min-width:120px; max-width:280px; padding:6px 10px;
    border:1px solid var(--border); background:var(--elevated); color:var(--fg);
    outline:none; font:inherit;
  }
  input[type=search]:focus { border-color:var(--accent); }
  header .meta {
    display:flex; align-items:center; gap:8px; flex-shrink:0;
    color:var(--muted); font-size:11px; white-space:nowrap;
  }
  header .meta .count-tag {
    display:inline-flex; align-items:center; min-width:1.25rem;
    padding:2px 6px; border:1px solid var(--border); background:rgba(255,255,255,.06);
    color:var(--secondary); font-variant-numeric:tabular-nums; font-size:10px; font-weight:600;
  }
  header .meta .live {
    color:var(--muted); font-size:10px; text-transform:uppercase; letter-spacing:0.06em;
  }
  header .meta .live::before {
    content:''; display:inline-block; width:6px; height:6px; border-radius:50%;
    background:#4ade80; margin-right:5px; vertical-align:middle;
  }
  header .spacer { flex:1; }
  header button#toggle-reader {
    flex-shrink:0; display:inline-flex; align-items:center; gap:5px;
    padding:5px 9px; font:10px/1 inherit; font-weight:600;
    color:var(--secondary); background:transparent;
    border:1px solid var(--border); cursor:pointer;
  }
  header button#toggle-reader:hover { color:var(--fg); border-color:var(--muted); }
  header button#toggle-reader.collapsed {
    border-color:rgba(59,130,246,.5); background:rgba(59,130,246,.12); color:var(--fg);
  }
  header button#toggle-reader svg { width:10px; height:10px; transition:transform .15s linear; }
  header button#toggle-reader:not(.collapsed) svg { transform:rotate(180deg); }
  header button#toggle-chat {
    flex-shrink:0; display:none; align-items:center; gap:5px;
    padding:5px 9px; font:10px/1 inherit; font-weight:600;
    color:var(--secondary); background:transparent;
    border:1px solid var(--border); cursor:pointer; margin-right:6px;
  }
  header button#toggle-chat.visible { display:inline-flex; }
  header button#toggle-chat:hover { color:var(--fg); border-color:var(--muted); }
  header button#toggle-chat.active {
    border-color:rgba(59,130,246,.5); background:rgba(59,130,246,.12); color:var(--fg);
  }
  main { flex:1; display:grid; grid-template-columns: 1fr minmax(280px, 0.9fr); min-height:0; min-width:0; }
  main.reader-collapsed { grid-template-columns: 1fr; }
  main.reader-collapsed #reader { display:none; }
  main.reader-collapsed #graph-wrap { border-right:0; }
  main.has-chat { grid-template-columns: 1fr minmax(240px, 0.75fr) minmax(260px, 0.7fr); }
  main.has-chat.reader-collapsed { grid-template-columns: 1fr minmax(260px, 0.7fr); }
  main.has-chat.chat-collapsed { grid-template-columns: 1fr minmax(280px, 0.9fr); }
  main.has-chat.reader-collapsed.chat-collapsed { grid-template-columns: 1fr; }
  main.chat-collapsed #chat { display:none; }
  #graph-wrap { position:relative; min-height:0; min-width:0; height:100%; border-right:1px solid var(--border); overflow:hidden; background:var(--bg); }
  canvas { position:absolute; inset:0; width:100%; height:100%; display:block; cursor:grab; touch-action:none; }
  canvas:active { cursor:grabbing; }
  #reader { overflow:auto; padding:14px 16px; background:var(--panel); min-height:0; }
  #chat {
    display:none; flex-direction:column; min-height:0; min-width:0;
    background:var(--panel); border-left:1px solid var(--border); overflow:hidden;
  }
  main.has-chat:not(.chat-collapsed) #chat { display:flex; }
  #chat .chat-head {
    flex-shrink:0; padding:10px 12px 8px; border-bottom:1px solid var(--border);
    font-size:11px; font-weight:700; letter-spacing:0.1em; text-transform:uppercase; color:var(--muted);
  }
  #chat .chat-log {
    flex:1; overflow:auto; padding:10px 12px; display:flex; flex-direction:column; gap:10px;
  }
  #chat .bubble {
    padding:8px 10px; border:1px solid var(--border); background:var(--elevated);
    font-size:12px; line-height:1.5; color:var(--secondary); white-space:pre-wrap; word-break:break-word;
  }
  #chat .bubble.user { border-color:rgba(59,130,246,.35); color:var(--fg); }
  #chat .bubble.agent { border-color:var(--border); }
  #chat .bubble.agent.final { border-color:rgba(74,222,128,.35); color:var(--fg); }
  #chat .bubble.meta { border-style:dashed; color:var(--muted); font-size:11px; }
  #chat .bubble .role {
    display:block; font-size:9px; font-weight:700; letter-spacing:0.08em;
    text-transform:uppercase; color:var(--muted); margin-bottom:4px;
  }
  #chat .chat-form {
    flex-shrink:0; display:flex; flex-direction:column; gap:8px;
    padding:10px 12px; border-top:1px solid var(--border); background:var(--bg);
  }
  #chat textarea {
    width:100%; min-height:64px; max-height:140px; resize:vertical;
    padding:8px 10px; border:1px solid var(--border); background:var(--elevated);
    color:var(--fg); font:inherit; outline:none;
  }
  #chat textarea:focus { border-color:var(--accent); }
  #chat textarea:disabled { opacity:0.55; }
  #chat .chat-actions { display:flex; align-items:center; gap:8px; }
  #chat button#chat-send {
    flex:1; padding:7px 10px; font:11px/1 inherit; font-weight:600;
    color:#fff; background:var(--accent); border:1px solid transparent; cursor:pointer;
  }
  #chat button#chat-send:hover:not(:disabled) { filter:brightness(1.08); }
  #chat button#chat-send:disabled { opacity:0.5; cursor:default; }
  #chat button#chat-new {
    padding:7px 10px; font:10px/1 inherit; font-weight:600;
    color:var(--secondary); background:transparent; border:1px solid var(--border); cursor:pointer;
  }
  #chat button#chat-new:hover { color:var(--fg); border-color:var(--muted); }
  #chat .chat-status { font-size:10px; color:var(--muted); min-height:1.2em; }
  #reader > h2.title { margin:0 0 8px; font-size:14px; font-weight:600; color:var(--fg); }
  #reader .tags { color:var(--muted); font-size:10px; margin-bottom:12px; text-transform:uppercase; letter-spacing:0.06em; }
  #reader .md { font:12.5px/1.6 ui-monospace, SFMono-Regular, Menlo, monospace; color:var(--secondary); }
  #reader .md > :first-child { margin-top:0; }
  #reader .md h1 { font-size:18px; font-weight:700; color:var(--fg); margin:1.1em 0 0.45em; border-bottom:1px solid var(--border); padding-bottom:0.25em; }
  #reader .md h2 { font-size:15px; font-weight:650; color:var(--fg); margin:1em 0 0.4em; }
  #reader .md h3 { font-size:13px; font-weight:650; color:var(--fg); margin:0.9em 0 0.35em; }
  #reader .md h4, #reader .md h5, #reader .md h6 { font-size:12px; font-weight:600; color:var(--fg); margin:0.8em 0 0.3em; }
  #reader .md p { margin:0.55em 0; }
  #reader .md ul, #reader .md ol { margin:0.45em 0; padding-left:1.35em; }
  #reader .md li { margin:0.2em 0; }
  #reader .md blockquote {
    margin:0.6em 0; padding:0.35em 0 0.35em 0.85em;
    border-left:2px solid var(--border); color:var(--muted);
  }
  #reader .md hr { border:0; border-top:1px solid var(--border); margin:1em 0; }
  #reader .md a { color:var(--accent); text-decoration:underline; text-decoration-color:rgba(59,130,246,.4); }
  #reader .md a:hover { text-decoration-color:var(--accent); }
  #reader .md a.wiki { cursor:pointer; }
  #reader .md code {
    font:0.92em/1.4 ui-monospace, SFMono-Regular, Menlo, monospace;
    background:rgba(255,255,255,.06); border:1px solid var(--border);
    padding:0.1em 0.35em; color:var(--accent-soft);
  }
  #reader .md pre {
    margin:0.7em 0; padding:10px 12px; overflow:auto;
    background:var(--bg); border:1px solid var(--border);
    color:var(--fg); line-height:1.45;
  }
  #reader .md pre code { background:none; border:0; padding:0; color:inherit; font-size:11.5px; }
  #reader .md table { border-collapse:collapse; margin:0.7em 0; width:100%; font-size:11.5px; }
  #reader .md th, #reader .md td { border:1px solid var(--border); padding:5px 8px; text-align:left; }
  #reader .md th { background:rgba(255,255,255,.04); color:var(--fg); font-weight:600; }
  #reader .md strong { color:var(--fg); font-weight:650; }
  #reader .empty { color:var(--muted); font-size:11px; }
  #hint {
    position:absolute; right:10px; bottom:10px; z-index:2;
    font-size:10px; color:var(--muted);
    background:rgba(20,20,20,.85); border:1px solid var(--border);
    padding:4px 8px; pointer-events:none;
  }
  @media (max-width:1100px) {
    main.has-chat:not(.chat-collapsed):not(.reader-collapsed) {
      grid-template-columns:1fr minmax(220px, 0.7fr);
      grid-template-rows:1fr 1fr;
    }
    main.has-chat:not(.chat-collapsed):not(.reader-collapsed) #reader { display:none; }
  }
  @media (max-width:800px) {
    main:not(.reader-collapsed):not(.has-chat) { grid-template-columns:1fr; grid-template-rows:1fr 1fr; }
    main:not(.reader-collapsed):not(.has-chat) #graph-wrap { border-right:0; border-bottom:1px solid var(--border); }
    main.has-chat:not(.chat-collapsed) {
      grid-template-columns:1fr; grid-template-rows:1fr 1fr;
    }
    main.has-chat:not(.chat-collapsed) #reader { display:none; }
    main.has-chat:not(.chat-collapsed) #graph-wrap { border-right:0; border-bottom:1px solid var(--border); }
    main.has-chat:not(.chat-collapsed) #chat { border-left:0; }
  }
</style>
</head>
<body>
<header>
  <h1>K2 Wiki</h1>
  <div class="search-wrap">
    <input type="search" id="q" placeholder="Search notes…" />
    <div class="meta" id="meta">
      <span>Articles</span>
      <span class="count-tag" id="note-count">—</span>
      <span class="live" id="live-pill">live</span>
    </div>
  </div>
  <div class="spacer"></div>
  <button type="button" id="toggle-chat" title="Ask this wiki" aria-pressed="false">
    <span>Ask</span>
  </button>
  <button type="button" id="toggle-reader" title="Collapse article viewer">
    <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.5">
      <path d="M4 2 L8 6 L4 10" stroke-linecap="round" stroke-linejoin="round"/>
    </svg>
    <span id="toggle-reader-label">Hide article</span>
  </button>
</header>
<main id="layout">
  <div id="graph-wrap">
    <canvas id="c"></canvas>
    <div id="hint">scroll = pan · pinch = zoom · drag node</div>
  </div>
  <aside id="reader"><p class="empty">Select a note.</p></aside>
  <aside id="chat" aria-label="Ask this wiki">
    <div class="chat-head">Ask this wiki</div>
    <div class="chat-log" id="chat-log">
      <div class="bubble meta">Ask questions about this workspace knowledge base. Answers come from the workspace agent.</div>
    </div>
    <form class="chat-form" id="chat-form">
      <textarea id="chat-input" rows="3" placeholder="Ask a question…" maxlength="4000"></textarea>
      <div class="chat-actions">
        <button type="submit" id="chat-send">Send</button>
        <button type="button" id="chat-new" title="Start a new conversation">New</button>
      </div>
      <div class="chat-status" id="chat-status"></div>
    </form>
  </aside>
</main>
<script>
(function(){
  const canvas = document.getElementById('c');
  const wrap = document.getElementById('graph-wrap');
  const layout = document.getElementById('layout');
  const ctx = canvas.getContext('2d');
  const reader = document.getElementById('reader');
  const meta = document.getElementById('meta');
  const noteCountEl = document.getElementById('note-count');
  const livePill = document.getElementById('live-pill');
  const q = document.getElementById('q');
  const toggleReaderBtn = document.getElementById('toggle-reader');
  const toggleReaderLabel = document.getElementById('toggle-reader-label');
  let index = { nodes: [], links: [], generatedAt: '', noteCount: 0 };
  let selected = null;
  let readerCollapsed = false;
  let nodes = []; // sim nodes (stable identity by id)
  let links = [];
  let drag = null;
  let lastFp = '';
  /** Force "temperature" — cools to 0 so layout settles instead of thrashing. */
  let alpha = 1;
  let viewW = 600, viewH = 400;
  // Camera: world point (camX, camY) sits at canvas center; camK is zoom.
  let camX = 300, camY = 200, camK = 1;
  // Multi-touch state for pinch + two-finger pan
  const pointers = new Map(); // id -> {x,y} canvas coords
  let pinch0 = null; // {dist, camK, midWx, midWy, camX, camY}

  function fingerprint(idx){
    const ns = (idx.nodes || []).map(n =>
      n.id + '\t' + (n.title||'') + '\t' + (n.exists?1:0) + '\t' + (n.tags||[]).join(',')
    ).sort().join('\n');
    const ls = (idx.links || []).map(l =>
      l.source + '\t' + l.target + '\t' + (l.missing?1:0)
    ).sort().join('\n');
    return (idx.noteCount||0) + '\n' + ns + '\n' + ls;
  }

  function screenToWorld(sx, sy){
    return {
      x: (sx - viewW/2) / camK + camX,
      y: (sy - viewH/2) / camK + camY,
    };
  }

  function canvasCoords(e){
    const rect = canvas.getBoundingClientRect();
    return { x: e.clientX - rect.left, y: e.clientY - rect.top };
  }

  function resize(){
    const r = wrap.getBoundingClientRect();
    const w = Math.max(1, Math.floor(r.width));
    const h = Math.max(1, Math.floor(r.height));
    if (w < 2 || h < 2) return;
    const prevW = viewW, prevH = viewH;
    const first = !nodes.length && viewW === 600 && viewH === 400;
    viewW = w; viewH = h;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const bw = Math.max(1, Math.floor(w * dpr));
    const bh = Math.max(1, Math.floor(h * dpr));
    if (canvas.width !== bw || canvas.height !== bh) {
      canvas.width = bw;
      canvas.height = bh;
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    // First layout: aim camera at sim center.
    if (first || (camX === 300 && camY === 200 && !nodes.length)) {
      camX = w/2; camY = h/2;
    }
    // Proportional reflow of sim positions only (camera stays put).
    if (nodes.length && prevW > 2 && prevH > 2 && (prevW !== w || prevH !== h)) {
      const sx = w / prevW, sy = h / prevH;
      for (const n of nodes) {
        n.x *= sx; n.y *= sy;
        n.vx = 0; n.vy = 0;
      }
      camX *= sx; camY *= sy;
    }
  }
  if (typeof ResizeObserver !== 'undefined') {
    new ResizeObserver(() => resize()).observe(wrap);
  }
  window.addEventListener('resize', resize);
  resize();

  /** Merge index into sim: keep x/y for known ids; place newcomers nearby. */
  function mergeSim(idx){
    const W = viewW, H = viewH;
    const prev = new Map(nodes.map(n => [n.id, n]));
    const map = new Map();
    const next = [];
    const list = idx.nodes || [];
    list.forEach((n, i) => {
      const old = prev.get(n.id);
      let o;
      if (old) {
        o = old;
        o.title = n.title || n.id;
        o.exists = !!n.exists;
        o.tags = n.tags || [];
      } else {
        const angle = (i / Math.max(1, list.length)) * Math.PI * 2;
        const rad = Math.min(W, H) * 0.28;
        o = {
          id: n.id, title: n.title || n.id, exists: !!n.exists,
          tags: n.tags || [],
          x: W/2 + Math.cos(angle)*rad,
          y: H/2 + Math.sin(angle)*rad,
          vx:0, vy:0
        };
      }
      map.set(n.id, o);
      next.push(o);
    });
    nodes = next;
    links = (idx.links || []).map(l => ({
      source: map.get(l.source), target: map.get(l.target), missing: !!l.missing
    })).filter(l => l.source && l.target);
    alpha = Math.max(alpha, 0.55);
  }

  function step(){
    if (alpha < 0.01) return;
    const W = viewW, H = viewH;
    const a = alpha;
    for (let i=0;i<nodes.length;i++){
      for (let j=i+1;j<nodes.length;j++){
        let dx = nodes[j].x - nodes[i].x;
        let dy = nodes[j].y - nodes[i].y;
        let d2 = dx*dx + dy*dy + 0.01;
        let f = (600 * a) / d2;
        let d = Math.sqrt(d2);
        dx/=d; dy/=d;
        nodes[i].vx -= f*dx; nodes[i].vy -= f*dy;
        nodes[j].vx += f*dx; nodes[j].vy += f*dy;
      }
    }
    for (const l of links){
      let dx = l.target.x - l.source.x;
      let dy = l.target.y - l.source.y;
      let d = Math.sqrt(dx*dx+dy*dy) || 1;
      let f = (d - 90) * 0.015 * a;
      dx/=d; dy/=d;
      l.source.vx += f*dx; l.source.vy += f*dy;
      l.target.vx -= f*dx; l.target.vy -= f*dy;
    }
    // Layout gravity toward world origin of the sim (view center at spawn).
    // Do NOT clamp to viewport — camera pans freely over the world.
    for (const n of nodes){
      if (drag === n) continue;
      n.vx += (W/2 - n.x) * 0.0015 * a;
      n.vy += (H/2 - n.y) * 0.0015 * a;
      n.vx *= 0.82; n.vy *= 0.82;
      n.x += n.vx; n.y += n.vy;
    }
    alpha *= 0.985;
    if (alpha < 0.01) alpha = 0;
  }

  // Match in-app WikiGraph label visibility (pixel diameter of the node).
  const NODE_R = 5;
  const NODE_R_SEL = 6.5;
  const LABEL_FADE_START_PX = 20; // labels start fading in
  const LABEL_MIN_PX = 25;        // full opacity
  /** Soft blue for Home when not selected (matches app HOME_SOFT_BLUE). */
  const HOME_SOFT = '#8fa8ff';

  function labelOpacityForDiameter(diameterPx){
    if (diameterPx <= LABEL_FADE_START_PX) return 0;
    if (diameterPx >= LABEL_MIN_PX) return 1;
    const t = (diameterPx - LABEL_FADE_START_PX) / (LABEL_MIN_PX - LABEL_FADE_START_PX);
    return t * t * (3 - 2 * t); // smoothstep
  }

  /** Match app isWikiHomeNode — real Home note, not missing stubs. */
  function isHomeNode(n){
    if (!n || !n.exists) return false;
    const id = String(n.id || '');
    const title = String(n.title || '').trim().toLowerCase();
    const bare = id.includes('::') ? (id.split('::').pop() || id) : id;
    if (bare.toLowerCase() === 'home.md') return true;
    if (title === 'home') return true;
    return false;
  }

  function draw(){
    const W = viewW, H = viewH;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0,0,W,H);
    ctx.save();
    // Camera: world (camX,camY) → canvas center, scaled by camK
    ctx.translate(W/2, H/2);
    ctx.scale(camK, camK);
    ctx.translate(-camX, -camY);

    for (const l of links){
      ctx.beginPath();
      ctx.strokeStyle = l.missing ? 'rgba(248,113,113,.4)' : 'rgba(42,42,42,.95)';
      ctx.lineWidth = 1 / camK;
      ctx.moveTo(l.source.x, l.source.y);
      ctx.lineTo(l.target.x, l.target.y);
      ctx.stroke();
    }
    // On-screen diameter of a default node at current zoom (matches app math:
    // 2 * r_graph * globalScale).
    const diameterPx = 2 * NODE_R * camK;
    const zoomLabelA = labelOpacityForDiameter(diameterPx);
    const qq = (q.value || '').toLowerCase();
    for (const n of nodes){
      if (qq && !n.title.toLowerCase().includes(qq) && !n.id.toLowerCase().includes(qq)
          && !(n.tags||[]).some(t => String(t).toLowerCase().includes(qq))) continue;
      const sel = selected === n.id;
      const home = isHomeNode(n);
      const r = sel ? NODE_R_SEL : NODE_R;
      ctx.beginPath();
      if (!n.exists) {
        ctx.fillStyle = '#f87171';
        ctx.globalAlpha = 0.55;
      } else if (sel) {
        ctx.fillStyle = '#3b82f6';
        ctx.globalAlpha = 1;
      } else if (home) {
        // Deselected Home stays findable as a lighter blue (in-app UX).
        ctx.fillStyle = HOME_SOFT;
        ctx.globalAlpha = 0.9;
      } else {
        ctx.fillStyle = '#e4e4e7';
        ctx.globalAlpha = 0.88;
      }
      ctx.arc(n.x, n.y, r, 0, Math.PI*2);
      ctx.fill();
      ctx.globalAlpha = 1;
      if (sel || home) {
        ctx.strokeStyle = sel ? '#3b82f6' : HOME_SOFT;
        ctx.lineWidth = (sel ? 1.5 : 1.1) / camK;
        ctx.globalAlpha = sel ? 1 : 0.65;
        ctx.beginPath();
        ctx.arc(n.x, n.y, r + 3/camK, 0, Math.PI*2);
        ctx.stroke();
        ctx.globalAlpha = 1;
      }
      // Labels: always on for selection + Home; otherwise fade by zoom.
      const labelA = (sel || home) ? 1 : zoomLabelA;
      if (labelA <= 0.01) continue;
      ctx.fillStyle = sel ? '#93c5fd' : (home ? HOME_SOFT : '#a1a1aa');
      // Keep label roughly constant on-screen (world font = screen_px / camK).
      const fontPx = Math.max(11 / camK, 2.8 / camK);
      ctx.font = fontPx + 'px ui-monospace, Menlo, monospace';
      ctx.globalAlpha = (sel || home ? 1 : 0.9) * labelA * (!n.exists ? 0.55 : 1);
      ctx.fillText(n.title, n.x + r + 4/camK, n.y + 3/camK);
      ctx.globalAlpha = 1;
    }
    ctx.restore();
  }

  function loop(){ step(); draw(); requestAnimationFrame(loop); }
  requestAnimationFrame(loop);

  // ── Pan / pinch (match in-app WikiGraph: wheel=pan, ctrl+wheel=zoom) ──
  wrap.addEventListener('wheel', (e) => {
    e.preventDefault();
    e.stopPropagation();
    const { x: sx, y: sy } = canvasCoords(e);
    if (e.ctrlKey || e.metaKey) {
      // Pinch (browsers synthesize ctrl+wheel)
      const world = screenToWorld(sx, sy);
      const factor = Math.exp(-e.deltaY * 0.01);
      const next = Math.min(8, Math.max(0.15, camK * factor));
      camK = next;
      camX = world.x - (sx - viewW/2) / camK;
      camY = world.y - (sy - viewH/2) / camK;
      return;
    }
    // Two-finger trackpad / mouse wheel → pan in world space
    camX += e.deltaX / camK;
    camY += e.deltaY / camK;
  }, { passive: false });

  function setReaderCollapsed(collapsed){
    readerCollapsed = !!collapsed;
    layout.classList.toggle('reader-collapsed', readerCollapsed);
    toggleReaderBtn.classList.toggle('collapsed', readerCollapsed);
    toggleReaderLabel.textContent = readerCollapsed ? 'Show article' : 'Hide article';
    toggleReaderBtn.title = readerCollapsed ? 'Show article viewer' : 'Collapse article viewer';
    // Let layout settle, then remeasure canvas.
    requestAnimationFrame(() => { resize(); });
  }
  toggleReaderBtn.addEventListener('click', () => setReaderCollapsed(!readerCollapsed));

  function escapeHtml(s){
    return String(s).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  }

  /** Lightweight Markdown → HTML (no CDN). Supports common GFM-ish bits + [[wikilinks]]. */
  function renderMarkdown(src){
    let text = String(src || '').replace(/\r\n/g, '\n');
    // Protect fenced code blocks
    const fences = [];
    text = text.replace(/```([a-zA-Z0-9_-]*)\n([\s\S]*?)```/g, (_, lang, code) => {
      const i = fences.length;
      fences.push({ lang: lang || '', code });
      return `\u0000FENCE${i}\u0000`;
    });
    // Protect inline code
    const inlines = [];
    text = text.replace(/`([^`\n]+)`/g, (_, code) => {
      const i = inlines.length;
      inlines.push(code);
      return `\u0000CODE${i}\u0000`;
    });

    const lines = text.split('\n');
    const out = [];
    let i = 0;
    let inUl = false, inOl = false, inBq = false;

    function closeLists(){
      if (inUl) { out.push('</ul>'); inUl = false; }
      if (inOl) { out.push('</ol>'); inOl = false; }
    }
    function closeBq(){
      if (inBq) { out.push('</blockquote>'); inBq = false; }
    }
    function inlineFmt(s){
      let t = escapeHtml(s);
      // [[wikilink]] or [[title|alias]]
      t = t.replace(/\[\[([^\]|#]+)(?:\|([^\]]+))?\]\]/g, (_, target, label) => {
        const title = target.trim();
        const lab = (label || title).trim();
        return `<a class="wiki" data-title="${escapeHtml(title)}">${escapeHtml(lab)}</a>`;
      });
      // [text](url)
      t = t.replace(/\[([^\]]+)\]\((https?:[^)\s]+)\)/g, (_, lab, url) =>
        `<a href="${escapeHtml(url)}" target="_blank" rel="noreferrer">${lab}</a>`
      );
      // bold / italic (bold first; no lookbehind for older engines)
      t = t.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
      t = t.replace(/__([^_]+)__/g, '<strong>$1</strong>');
      t = t.replace(/\*([^*]+)\*/g, '<em>$1</em>');
      t = t.replace(/_([^_]+)_/g, '<em>$1</em>');
      // restore inline code
      t = t.replace(/\u0000CODE(\d+)\u0000/g, (_, n) =>
        `<code>${escapeHtml(inlines[Number(n)])}</code>`
      );
      return t;
    }

    while (i < lines.length) {
      const line = lines[i];
      // table block
      if (/^\s*\|.+\|\s*$/.test(line) && i + 1 < lines.length && /^\s*\|?\s*:?-{3,}/.test(lines[i+1])) {
        closeLists(); closeBq();
        const rows = [];
        while (i < lines.length && /^\s*\|/.test(lines[i])) {
          rows.push(lines[i]);
          i++;
        }
        // drop separator row
        const bodyRows = rows.filter((r, idx) => idx === 0 || !/^\s*\|?\s*:?-{3,}/.test(r));
        const cells = (r) => r.replace(/^\s*\|/, '').replace(/\|\s*$/, '').split('|').map(c => c.trim());
        if (bodyRows.length) {
          out.push('<table><thead><tr>');
          cells(bodyRows[0]).forEach(c => out.push(`<th>${inlineFmt(c)}</th>`));
          out.push('</tr></thead><tbody>');
          for (let r = 1; r < bodyRows.length; r++) {
            out.push('<tr>');
            cells(bodyRows[r]).forEach(c => out.push(`<td>${inlineFmt(c)}</td>`));
            out.push('</tr>');
          }
          out.push('</tbody></table>');
        }
        continue;
      }
      // fence placeholder alone
      const fenceM = line.match(/^\u0000FENCE(\d+)\u0000$/);
      if (fenceM) {
        closeLists(); closeBq();
        const f = fences[Number(fenceM[1])];
        out.push(`<pre><code>${escapeHtml(f.code.replace(/\n$/, ''))}</code></pre>`);
        i++; continue;
      }
      if (/^\s*---+\s*$/.test(line) || /^\s*\*\*\*+\s*$/.test(line)) {
        closeLists(); closeBq();
        out.push('<hr/>'); i++; continue;
      }
      const hm = line.match(/^(#{1,6})\s+(.+)$/);
      if (hm) {
        closeLists(); closeBq();
        const lvl = hm[1].length;
        out.push(`<h${lvl}>${inlineFmt(hm[2].trim())}</h${lvl}>`);
        i++; continue;
      }
      const bq = line.match(/^\s*>\s?(.*)$/);
      if (bq) {
        closeLists();
        if (!inBq) { out.push('<blockquote>'); inBq = true; }
        out.push(`<p>${inlineFmt(bq[1])}</p>`);
        i++; continue;
      } else {
        closeBq();
      }
      const ul = line.match(/^\s*[-*+]\s+(.+)$/);
      if (ul) {
        closeBq();
        if (inOl) { out.push('</ol>'); inOl = false; }
        if (!inUl) { out.push('<ul>'); inUl = true; }
        out.push(`<li>${inlineFmt(ul[1])}</li>`);
        i++; continue;
      }
      const ol = line.match(/^\s*\d+\.\s+(.+)$/);
      if (ol) {
        closeBq();
        if (inUl) { out.push('</ul>'); inUl = false; }
        if (!inOl) { out.push('<ol>'); inOl = true; }
        out.push(`<li>${inlineFmt(ol[1])}</li>`);
        i++; continue;
      }
      if (/^\s*$/.test(line)) {
        closeLists(); closeBq();
        i++; continue;
      }
      closeLists(); closeBq();
      // paragraph — merge soft-wrapped lines
      let para = line;
      while (i + 1 < lines.length && !/^\s*$/.test(lines[i+1])
        && !/^(#{1,6})\s/.test(lines[i+1])
        && !/^\s*[-*+]\s/.test(lines[i+1])
        && !/^\s*\d+\.\s/.test(lines[i+1])
        && !/^\s*>/.test(lines[i+1])
        && !/^\u0000FENCE/.test(lines[i+1])
        && !/^\s*\|/.test(lines[i+1])
        && !/^\s*---+\s*$/.test(lines[i+1])) {
        i++;
        para += ' ' + lines[i].trim();
      }
      out.push(`<p>${inlineFmt(para)}</p>`);
      i++;
    }
    closeLists(); closeBq();
    // any leftover fence tokens in paragraphs
    let html = out.join('\n');
    html = html.replace(/\u0000FENCE(\d+)\u0000/g, (_, n) => {
      const f = fences[Number(n)];
      return `<pre><code>${escapeHtml(f.code.replace(/\n$/, ''))}</code></pre>`;
    });
    return html;
  }

  function wireWikiLinks(root){
    root.querySelectorAll('a.wiki').forEach(a => {
      a.onclick = (e) => {
        e.preventDefault();
        const title = a.dataset.title;
        const hit = (index.nodes||[]).find(n =>
          (n.title||'').toLowerCase() === title.toLowerCase()
          || (n.id||'').toLowerCase() === title.toLowerCase()
          || (n.aliases||[]).some(x => String(x).toLowerCase() === title.toLowerCase())
        );
        if (hit) selectNote(hit.id);
        else selectNote('missing:' + title);
      };
    });
  }

  async function selectNote(id){
    selected = id;
    // If the reader was collapsed, keep it collapsed — selection still works on the map.
    if (String(id).startsWith('missing:')) {
      reader.innerHTML = `<h2 class="title">${escapeHtml(id.slice(8))}</h2><p class="empty">Note does not exist yet.</p>`;
      return;
    }
    try {
      const r = await fetch('/api/note?id=' + encodeURIComponent(id));
      const j = await r.json();
      if (j.error) {
        reader.innerHTML = `<p class="empty">${escapeHtml(j.error)}</p>`;
        return;
      }
      const tags = (j.tags||[]).map(t => '#'+t).join(' ');
      reader.innerHTML =
        `<h2 class="title">${escapeHtml(j.title||j.id)}</h2>` +
        (tags ? `<div class="tags">${escapeHtml(tags)}</div>` : '') +
        `<div class="md">${renderMarkdown(j.body||'')}</div>`;
      wireWikiLinks(reader);
    } catch (e) {
      reader.innerHTML = `<p class="empty">${escapeHtml(String(e))}</p>`;
    }
  }

  function hitNode(sx, sy){
    const w = screenToWorld(sx, sy);
    let best = null, bd = 14 / camK;
    for (const n of nodes){
      const d = Math.hypot(n.x - w.x, n.y - w.y);
      if (d < bd) { bd = d; best = n; }
    }
    return best;
  }

  canvas.addEventListener('pointerdown', e => {
    const { x: sx, y: sy } = canvasCoords(e);
    pointers.set(e.pointerId, { x: sx, y: sy });
    if (pointers.size === 2) {
      // Start pinch / two-finger pan
      drag = null;
      const pts = [...pointers.values()];
      const dx = pts[1].x - pts[0].x, dy = pts[1].y - pts[0].y;
      const dist = Math.hypot(dx, dy) || 1;
      const mid = { x: (pts[0].x + pts[1].x)/2, y: (pts[0].y + pts[1].y)/2 };
      const world = screenToWorld(mid.x, mid.y);
      pinch0 = { dist, camK, midWx: world.x, midWy: world.y, camX, camY };
      return;
    }
    if (pointers.size === 1) {
      const best = hitNode(sx, sy);
      if (best) {
        drag = best;
        selectNote(best.id);
        canvas.setPointerCapture(e.pointerId);
      }
    }
  });
  canvas.addEventListener('pointermove', e => {
    if (!pointers.has(e.pointerId)) return;
    const { x: sx, y: sy } = canvasCoords(e);
    pointers.set(e.pointerId, { x: sx, y: sy });
    if (pointers.size >= 2 && pinch0) {
      const pts = [...pointers.values()];
      const dx = pts[1].x - pts[0].x, dy = pts[1].y - pts[0].y;
      const dist = Math.hypot(dx, dy) || 1;
      const mid = { x: (pts[0].x + pts[1].x)/2, y: (pts[0].y + pts[1].y)/2 };
      const nextK = Math.min(8, Math.max(0.15, pinch0.camK * (dist / pinch0.dist)));
      camK = nextK;
      // Keep midpoint world point under the fingers; also allow pan via mid movement
      camX = pinch0.midWx - (mid.x - viewW/2) / camK;
      camY = pinch0.midWy - (mid.y - viewH/2) / camK;
      return;
    }
    if (drag && pointers.size === 1) {
      const w = screenToWorld(sx, sy);
      drag.x = w.x; drag.y = w.y;
      drag.vx = drag.vy = 0;
    }
  });
  function endPointer(e){
    pointers.delete(e.pointerId);
    if (pointers.size < 2) pinch0 = null;
    if (pointers.size === 0) drag = null;
  }
  canvas.addEventListener('pointerup', endPointer);
  canvas.addEventListener('pointercancel', endPointer);

  // Search filters which labels/nodes are drawn (see draw()).
  q.addEventListener('input', () => { /* redraw uses q.value live */ });

  async function refresh(){
    try {
      const r = await fetch('/api/index');
      const j = await r.json();
      if (j.error) { if (meta) meta.title = j.error; return; }
      index = j;
      if (noteCountEl) noteCountEl.textContent = String(j.noteCount || 0);
      if (livePill) livePill.style.opacity = '1';
      const fp = fingerprint(j);
      if (fp !== lastFp) {
        lastFp = fp;
        mergeSim(j);
        if (!selected) {
          const first = (j.nodes||[]).find(n => n.exists);
          if (first) selectNote(first.id);
        }
      }
    } catch (e) {
      if (noteCountEl) noteCountEl.textContent = '—';
      if (livePill) { livePill.textContent = 'offline'; livePill.style.opacity = '0.6'; }
      if (meta) meta.title = String(e);
    }
  }
  refresh();
  setInterval(refresh, 3000);

  // ── Public chat (Ask this wiki) — only when /api/chat/status says enabled ──
  // Never receives or stores API keys / owner tokens; same-origin /api/chat* only.
  // Visitor continuity: localStorage keeps host-session id + UI transcript so a
  // return visit rewakes the same terminal (while idle reaper hasn't killed it)
  // without any login. Cleared by "New conversation".
  const toggleChatBtn = document.getElementById('toggle-chat');
  const chatLog = document.getElementById('chat-log');
  const chatForm = document.getElementById('chat-form');
  const chatInput = document.getElementById('chat-input');
  const chatSend = document.getElementById('chat-send');
  const chatNew = document.getElementById('chat-new');
  const chatStatus = document.getElementById('chat-status');
  let chatEnabled = false;
  let chatCollapsed = false;
  let chatSessionId = null;
  let chatSince = 0;
  let chatBusy = false;
  let chatPollTimer = null;
  // In-memory mirror of cached bubbles (role/text/cls) for persist.
  let chatHistory = [];
  const CHAT_CACHE_VERSION = 1;
  const CHAT_HISTORY_MAX = 80;
  // Keyed by origin so each published wiki host keeps its own visitor session.
  const CHAT_CACHE_KEY = 'k2-wiki-chat-v' + CHAT_CACHE_VERSION + ':' + (location.origin || '');

  function chatCacheLoad(){
    try {
      const raw = localStorage.getItem(CHAT_CACHE_KEY);
      if (!raw) return null;
      const o = JSON.parse(raw);
      if (!o || typeof o !== 'object') return null;
      return o;
    } catch (_) { return null; }
  }

  function chatCacheSave(){
    try {
      if (!chatSessionId && chatHistory.length === 0) {
        localStorage.removeItem(CHAT_CACHE_KEY);
        return;
      }
      const payload = {
        v: CHAT_CACHE_VERSION,
        sessionId: chatSessionId || null,
        since: chatSince || 0,
        history: chatHistory.slice(-CHAT_HISTORY_MAX),
        updatedAt: Date.now(),
      };
      localStorage.setItem(CHAT_CACHE_KEY, JSON.stringify(payload));
    } catch (_) { /* private mode / quota — continuity is best-effort */ }
  }

  function chatCacheClear(){
    chatSessionId = null;
    chatSince = 0;
    chatHistory = [];
    try { localStorage.removeItem(CHAT_CACHE_KEY); } catch (_) {}
  }

  function setChatCollapsed(collapsed){
    chatCollapsed = !!collapsed;
    layout.classList.toggle('chat-collapsed', chatCollapsed);
    if (toggleChatBtn) {
      toggleChatBtn.classList.toggle('active', !chatCollapsed && chatEnabled);
      toggleChatBtn.setAttribute('aria-pressed', (!chatCollapsed && chatEnabled) ? 'true' : 'false');
    }
    requestAnimationFrame(() => { resize(); });
  }

  function setChatAvailable(on){
    chatEnabled = !!on;
    layout.classList.toggle('has-chat', chatEnabled);
    if (toggleChatBtn) {
      toggleChatBtn.classList.toggle('visible', chatEnabled);
    }
    if (!chatEnabled) {
      setChatCollapsed(true);
      stopChatPoll();
    } else {
      // Restore visitor session + bubbles once chrome appears (reload / return).
      restoreChatFromCache();
      if (!layout.classList.contains('chat-collapsed') && !chatCollapsed) {
        setChatCollapsed(false);
      }
    }
    requestAnimationFrame(() => { resize(); });
  }

  function setChatBusy(busy, msg){
    chatBusy = !!busy;
    if (chatSend) chatSend.disabled = chatBusy;
    if (chatInput) chatInput.disabled = chatBusy;
    if (chatStatus) chatStatus.textContent = msg || (chatBusy ? 'Thinking…' : '');
  }

  function appendChatBubble(role, text, extraClass, opts){
    if (!chatLog) return;
    const skipPersist = opts && opts.skipPersist;
    const div = document.createElement('div');
    div.className = 'bubble ' + role + (extraClass ? ' ' + extraClass : '');
    const label = role === 'user' ? 'You' : (role === 'agent' ? 'Wiki agent' : '');
    if (label) {
      const r = document.createElement('span');
      r.className = 'role';
      r.textContent = label;
      div.appendChild(r);
    }
    // Prefer markdown-ish for agent finals; plain escape for progress/meta.
    if (role === 'agent' && extraClass === 'final' && typeof renderMarkdown === 'function') {
      const md = document.createElement('div');
      md.className = 'md';
      md.innerHTML = renderMarkdown(text);
      div.appendChild(md);
      if (typeof wireWikiLinks === 'function') wireWikiLinks(div);
    } else {
      const span = document.createElement('span');
      span.textContent = text;
      div.appendChild(span);
    }
    chatLog.appendChild(div);
    chatLog.scrollTop = chatLog.scrollHeight;
    if (!skipPersist) {
      // Skip ephemeral "thinking" progress lines in cache; keep user + finals + errors.
      if (role === 'user' || role === 'meta' || (role === 'agent' && extraClass === 'final')) {
        chatHistory.push({ role: role, text: String(text), cls: extraClass || '' });
        if (chatHistory.length > CHAT_HISTORY_MAX) {
          chatHistory = chatHistory.slice(-CHAT_HISTORY_MAX);
        }
        chatCacheSave();
      }
    }
  }

  function restoreChatFromCache(){
    const o = chatCacheLoad();
    if (!o) return;
    if (o.sessionId && typeof o.sessionId === 'string') chatSessionId = o.sessionId;
    if (typeof o.since === 'number' && o.since >= 0) chatSince = o.since;
    const hist = Array.isArray(o.history) ? o.history : [];
    if (hist.length && chatLog && chatLog.children.length === 0) {
      chatHistory = [];
      hist.forEach(h => {
        if (!h || !h.text) return;
        const role = h.role === 'user' || h.role === 'agent' || h.role === 'meta' ? h.role : 'meta';
        const cls = h.cls ? String(h.cls) : '';
        chatHistory.push({ role: role, text: String(h.text), cls: cls });
        appendChatBubble(role, String(h.text), cls, { skipPersist: true });
      });
      if (chatSessionId && chatStatus) {
        chatStatus.textContent = 'Resuming conversation…';
      }
    } else if (hist.length) {
      chatHistory = hist.filter(h => h && h.text).map(h => ({
        role: h.role === 'user' || h.role === 'agent' || h.role === 'meta' ? h.role : 'meta',
        text: String(h.text),
        cls: h.cls ? String(h.cls) : '',
      }));
    }
  }

  function stopChatPoll(){
    if (chatPollTimer) { clearInterval(chatPollTimer); chatPollTimer = null; }
  }

  // Ingest only NEW agent lines (seq > chatSince). Never re-paint prior turns.
  // `done` = a new line with final:true (not a historical final from since=0).
  function ingestChatMessages(msgs, doneFlag){
    let sawFinal = false;
    (msgs || []).forEach(m => {
      if (!m) return;
      const seq = (typeof m.seq === 'number') ? m.seq : null;
      if (seq !== null && seq <= chatSince) return; // already shown
      const text = m.text ? String(m.text) : '';
      if (!text) {
        if (seq !== null && seq > chatSince) chatSince = seq;
        return;
      }
      const isFinal = !!m.final;
      appendChatBubble('agent', text, isFinal ? 'final' : 'meta');
      if (seq !== null && seq > chatSince) chatSince = seq;
      if (isFinal) sawFinal = true;
    });
    // Server may report done only when a new final is in this slice.
    if (doneFlag && sawFinal) { /* keep */ }
    else if (doneFlag && !(msgs && msgs.length)) {
      // Empty slice + done:true is nonsense for follow-ups; ignore.
    }
    chatCacheSave();
    return sawFinal;
  }

  async function pollChatOnce(){
    if (!chatSessionId) return true;
    try {
      const r = await fetch('/api/chat/messages?session=' + encodeURIComponent(chatSessionId) + '&since=' + chatSince);
      const j = await r.json();
      if (!r.ok) {
        // Dead/expired session → drop cache so next send starts clean.
        if (r.status === 404) {
          chatCacheClear();
          setChatBusy(false, 'Previous session ended — send again to start a new one.');
          return true;
        }
        setChatBusy(false, (j && j.error) ? j.error : ('HTTP ' + r.status));
        return true;
      }
      const done = ingestChatMessages(j.messages, j.done);
      if (typeof j.latest_seq === 'number' && j.latest_seq > chatSince) {
        chatSince = j.latest_seq;
        chatCacheSave();
      }
      if (done) {
        setChatBusy(false, '');
        return true;
      }
      setChatBusy(true, 'Waiting for answer…');
      return false;
    } catch (e) {
      setChatBusy(false, String(e));
      return true;
    }
  }

  function startChatPoll(){
    stopChatPoll();
    let ticks = 0;
    chatPollTimer = setInterval(async () => {
      ticks++;
      const done = await pollChatOnce();
      if (done || ticks > 180) { // ~3 min @ 1s
        stopChatPoll();
        if (!done) setChatBusy(false, 'Timed out waiting for a final answer — try again.');
      }
    }, 1000);
  }

  async function sendChat(text){
    // Snapshot cursor BEFORE the turn so we only wait for *new* finals and
    // tell the gateway not to re-emit prior respond lines.
    const sinceBefore = chatSince || 0;
    const payload = { text: text, since: sinceBefore };
    if (chatSessionId) payload.sessionId = chatSessionId;
    setChatBusy(true, 'Sending…');
    appendChatBubble('user', text);
    try {
      const r = await fetch('/api/chat', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
      });
      const j = await r.json();
      if (!r.ok) {
        // Stale session id (reaped + not resumable) → clear and surface error.
        if (r.status === 404 && chatSessionId) {
          chatCacheClear();
          setChatBusy(false, 'Previous session ended — send again to start a new one.');
          appendChatBubble('meta', 'Previous session ended. Send your message again.');
          return;
        }
        setChatBusy(false, (j && j.error) ? j.error : ('HTTP ' + r.status));
        appendChatBubble('meta', (j && j.error) ? j.error : 'Request failed');
        return;
      }
      if (j.sessionId) chatSessionId = j.sessionId;
      // Do NOT jump chatSince to latest_seq before ingest — that skipped
      // painting and, with a stale drain, marked the turn done on the old final.
      const done = ingestChatMessages(j.messages, j.done);
      chatCacheSave();
      if (done) {
        setChatBusy(false, '');
      } else {
        setChatBusy(true, 'Waiting for answer…');
        startChatPoll();
      }
    } catch (e) {
      setChatBusy(false, String(e));
      appendChatBubble('meta', String(e));
    }
  }

  if (toggleChatBtn) {
    toggleChatBtn.addEventListener('click', () => {
      if (!chatEnabled) return;
      setChatCollapsed(!chatCollapsed);
    });
  }
  if (chatNew) {
    chatNew.addEventListener('click', () => {
      stopChatPoll();
      chatCacheClear();
      setChatBusy(false, 'New conversation');
      if (chatLog) {
        chatLog.innerHTML = '';
        appendChatBubble('meta', 'New conversation. Ask a question about this wiki.');
      }
    });
  }
  if (chatForm) {
    chatForm.addEventListener('submit', (e) => {
      e.preventDefault();
      if (chatBusy || !chatEnabled) return;
      const text = (chatInput && chatInput.value || '').trim();
      if (!text) return;
      if (chatInput) chatInput.value = '';
      sendChat(text);
    });
  }
  // Enter sends (Shift+Enter newline)
  if (chatInput) {
    chatInput.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        if (chatForm) chatForm.requestSubmit();
      }
    });
  }

  async function refreshChatStatus(){
    try {
      const r = await fetch('/api/chat/status');
      const j = await r.json();
      const on = !!(j && j.enabled);
      if (on !== chatEnabled) {
        setChatAvailable(on);
        if (on && chatStatus) chatStatus.textContent = '';
      } else if (!on && chatStatus && j && j.reason) {
        // keep quiet in UI when chrome is hidden
      }
    } catch (_) {
      // offline — hide chat chrome
      if (chatEnabled) setChatAvailable(false);
    }
  }
  // Start collapsed until status enables.
  setChatAvailable(false);
  setChatCollapsed(true);
  refreshChatStatus();
  setInterval(refreshChatStatus, 8000);
})();
</script>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_ws(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2-wiki-routes-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn index_and_seed_handlers() {
        let ws = temp_ws("seed");
        let mut params = HashMap::new();
        params.insert("project".into(), ws.to_string_lossy().into_owned());

        let empty = handle_index(&params);
        assert_eq!(empty.status, "200 OK");
        assert!(empty.body.contains("\"noteCount\":0") || empty.body.contains("\"note_count\":0") || empty.body.contains("noteCount"));

        let seeded = handle_seed_post(&params);
        assert_eq!(seeded.status, "200 OK", "{}", seeded.body);
        assert!(seeded.body.contains("Home.md"));

        let idx = handle_index(&params);
        assert_eq!(idx.status, "200 OK");
        assert!(idx.body.contains("Home.md"));

        let mut note_params = params.clone();
        note_params.insert("id".into(), "Home.md".into());
        let note = handle_note(&note_params);
        assert_eq!(note.status, "200 OK", "{}", note.body);
        assert!(note.body.contains("Knowledge Base") || note.body.contains("body"));

        let st = handle_serve_status(&params);
        assert_eq!(st.status, "200 OK");
        assert!(st.body.contains("\"enabled\":false") || st.body.contains("enabled"));
        // Phase 1 — public chat fields present; default OFF.
        assert!(
            st.body.contains("publicChatEnabled"),
            "status must include publicChatEnabled; got {}",
            st.body
        );
        assert!(
            st.body.contains("\"publicChatEnabled\":false"),
            "public chat must default OFF; got {}",
            st.body
        );
        assert!(
            st.body.contains("\"publicChatReady\":false"),
            "public chat ready must be false when disabled; got {}",
            st.body
        );
        // Never leak a raw API key in status.
        assert!(
            !st.body.contains("k2sk_"),
            "status must never contain raw API key"
        );

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn percent_decode_basic() {
        assert_eq!(percent_decode("Home.md"), "Home.md");
        assert_eq!(percent_decode("Feature%20-%20X.md"), "Feature - X.md");
        assert_eq!(percent_decode("a+b"), "a b");
    }

    // ── W0 (0.46.0): public chat disabled, site locked ─────────────────

    fn insert_project_flags(path: &str, chat: i64, skip: Option<i64>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let name = format!("w0-{}", &id[..8]);
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, wiki_public_chat, api_skip_permissions) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, name, path, chat, skip],
        )
        .expect("insert project row");
        id
    }

    fn flags(path: &str) -> (i64, Option<i64>) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT wiki_public_chat, api_skip_permissions FROM projects WHERE path = ?1",
            rusqlite::params![path],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)),
        )
        .expect("project row")
    }

    fn json(body: &str) -> serde_json::Value {
        serde_json::from_str(body).expect("JSON body")
    }

    fn assert_unavailable(r: &CliResponse, what: &str) {
        assert_eq!(r.status, "410 Gone", "{what}: {}", r.body);
        let v = json(&r.body);
        assert_eq!(v["error"]["code"], PUBLIC_CHAT_UNAVAILABLE, "{what}: {}", r.body);
        assert_eq!(v["error"]["hint"], PUBLIC_CHAT_UNAVAILABLE_HINT, "{what}: {}", r.body);
    }

    /// Status always reports chat off and unavailable.
    #[test]
    fn wiki_public_chat_status_always_off() {
        let ws = temp_ws("chat-status");
        let path = ws.to_string_lossy().into_owned();
        // Even a leftover DB opt-in reports off.
        insert_project_flags(&path, 1, Some(1));
        let st = status_for(&ws);
        assert!(!st.public_chat_enabled);
        assert!(!st.public_chat_ready);
        assert!(!st.public_chat_available);
        assert!(st.public_chat_error.is_none());
        let r = handle_serve_status(&wiki_params(&path, &[]));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v = json(&r.body);
        assert_eq!(v["publicChatEnabled"], false, "{}", r.body);
        assert_eq!(v["publicChatAvailable"], false, "{}", r.body);
        let _ = fs::remove_dir_all(&ws);
    }

    /// `POST /cli/wiki/chat` on (every spelling) and `serve publicChat=on`
    /// answer 410 public_chat_unavailable and change nothing.
    #[test]
    fn wiki_chat_on_route_refused() {
        let ws = temp_ws("chat-on");
        let path = ws.to_string_lossy().into_owned();
        insert_project_flags(&path, 0, Some(0));

        for (route, extra) in [
            ("/cli/wiki/chat", vec![("enabled", "true")]),
            ("/cli/wiki/chat", vec![("mode", "on")]),
            ("/cli/wiki/chat", vec![("action", "enable")]),
            ("/cli/wiki/chat", vec![("publicChat", "1")]),
            ("/cli/wiki/chat/on", vec![]),
            ("/cli/wiki/serve", vec![("enabled", "true"), ("publicChat", "true")]),
            ("/cli/wiki/serve", vec![("enabled", "false"), ("chat", "on")]),
        ] {
            let r = dispatch_post(route, &wiki_params(&path, &extra));
            assert_unavailable(&r, &format!("{route} {extra:?}"));
            assert_eq!(flags(&path), (0, Some(0)), "{route} {extra:?} must change nothing");
        }
        // Refused before any lookup: an unknown project gets the same answer.
        let r = dispatch_post(
            "/cli/wiki/chat",
            &wiki_params("/tmp/no-such-k2-wiki-chat-proj", &[("enabled", "true")]),
        );
        assert_unavailable(&r, "unknown project");

        // Off still works.
        let r = dispatch_post("/cli/wiki/chat/off", &wiki_params(&path, &[]));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(json(&r.body)["publicChatEnabled"], false, "{}", r.body);
        // Off on an unregistered path fails loudly.
        let r = dispatch_post(
            "/cli/wiki/chat",
            &wiki_params("/tmp/no-such-k2-wiki-chat-proj", &[("enabled", "false")]),
        );
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        let _ = fs::remove_dir_all(&ws);
    }

    /// Off on a workspace that still has chat on reverts the
    /// skip-permissions the old toggle wrote (hole 2).
    #[test]
    fn wiki_chat_off_reverts_skip_permissions_when_chat_was_on() {
        let _home = crate::test_support::TempHome::new();
        let ws = temp_ws("chat-off-revert");
        let path = ws.to_string_lossy().into_owned();
        insert_project_flags(&path, 1, Some(1));
        let r = dispatch_post("/cli/wiki/chat", &wiki_params(&path, &[("enabled", "false")]));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(flags(&path), (0, Some(0)));
        let _ = fs::remove_dir_all(&ws);
    }

    /// Boot heal: chat on → off everywhere; skip-permissions reverted when
    /// it read on (1 or NULL); untouched when chat was off; chat keys
    /// revoked; one audit line per change; second run is a no-op.
    #[test]
    fn wiki_boot_heal_turns_chat_off_and_reverts_skip_permissions() {
        let _home = crate::test_support::TempHome::new();
        let a = temp_ws("heal-a");
        let b = temp_ws("heal-b");
        let c = temp_ws("heal-c");
        let d = temp_ws("heal-d");
        let (pa, pb, pc, pd) = (
            a.to_string_lossy().into_owned(),
            b.to_string_lossy().into_owned(),
            c.to_string_lossy().into_owned(),
            d.to_string_lossy().into_owned(),
        );
        insert_project_flags(&pa, 1, Some(1));
        insert_project_flags(&pb, 1, None);
        insert_project_flags(&pc, 1, Some(0));
        insert_project_flags(&pd, 0, Some(1));
        // A leftover daemon-held chat key (minted by the old toggle).
        let meta = k2_core::wiki_public_chat::ensure_chat_key(&pa).expect("mint chat key");
        let raw = k2_core::wiki_public_chat::load_raw_key(&pa).expect("raw key on file");
        assert!(k2_core::api_keys::resolve_api_key(&raw).is_some(), "key live before heal");

        let report = boot_heal_public_chat();
        let find = |p: &str| report.workspaces.iter().find(|h| h.path == p).cloned();
        assert!(find(&pa).expect("A healed").skip_permissions_reverted);
        assert!(find(&pb).expect("B healed").skip_permissions_reverted);
        assert!(!find(&pc).expect("C healed").skip_permissions_reverted);
        assert!(find(&pd).is_none(), "chat-off workspace must not be touched");
        assert_eq!(flags(&pa), (0, Some(0)));
        assert_eq!(flags(&pb), (0, Some(0)));
        assert_eq!(flags(&pc), (0, Some(0)));
        assert_eq!(flags(&pd), (0, Some(1)), "skip-permissions kept when chat was off");

        assert!(report.keys_revoked.contains(&meta.key_id), "{:?}", report.keys_revoked);
        assert!(k2_core::api_keys::resolve_api_key(&raw).is_none(), "key revoked");
        assert!(k2_core::wiki_public_chat::chat_key_meta(&pa).is_none(), "key file removed");

        let audit = fs::read_to_string(k2_core::auth_audit::path()).expect("audit log");
        let heal_lines: Vec<&str> = audit
            .lines()
            .filter(|l| l.contains("\"wiki-public-chat-heal\""))
            .collect();
        assert!(
            heal_lines
                .iter()
                .filter(|l| l.contains("api_skip_permissions_reverted"))
                .count()
                >= 2,
            "{audit}"
        );
        assert!(
            heal_lines.iter().any(|l| l.contains("api_skip_permissions_already_off")),
            "{audit}"
        );
        assert!(heal_lines.iter().any(|l| l.contains("chat_keys_revoked:")), "{audit}");

        let again = boot_heal_public_chat();
        for p in [&pa, &pb, &pc, &pd] {
            assert!(
                !again.workspaces.iter().any(|h| &h.path == p),
                "second heal must be a no-op for {p}"
            );
        }
        assert!(!again.keys_revoked.contains(&meta.key_id));
        assert_eq!(flags(&pd), (0, Some(1)));
        for w in [a, b, c, d] {
            let _ = fs::remove_dir_all(&w);
        }
    }

    /// Host-session spawns no longer treat a leftover chat opt-in as
    /// consent to skip permissions (`policy.rs`), and the wiki module
    /// never reaches host sessions or the chat key.
    #[test]
    fn wiki_public_chat_has_no_spawn_path() {
        let src = include_str!("wiki_routes.rs");
        let prod = &src[..src.find("#[cfg(test)]").expect("test module")];
        for needle in [
            "v1_host_sessions",
            "handle_v1_host_new",
            "load_raw_key",
            "ensure_chat_key",
        ] {
            assert!(!prod.contains(needle), "wiki_routes must not call {needle}");
        }
        let policy = include_str!("v1_host_sessions/policy.rs");
        assert!(
            !policy.contains("get_wiki_public_chat"),
            "host-session policy must not read wiki_public_chat"
        );
    }

    /// `k2 wiki chat on` is refused by the CLI itself (exit 3, never sent —
    /// an older remote daemon would still accept it), human and JSON.
    #[test]
    fn wiki_cli_chat_on_exits_3() {
        let cli = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
        let home = temp_ws("cli-home");
        for args in [
            vec!["wiki", "chat", "on"],
            vec!["wiki", "chat", "enable"],
            vec!["wiki", "chat", "on", "--json"],
        ] {
            let out = std::process::Command::new("bash")
                .arg(&cli)
                .args(&args)
                .env_clear()
                .env("HOME", &home)
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .output()
                .expect("run cli/k2");
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(3), "{args:?}: {stderr}");
            assert!(stderr.contains("K2 runtime"), "{args:?}: {stderr}");
            assert!(out.stdout.is_empty(), "{args:?}");
            if args.contains(&"--json") {
                let v: serde_json::Value =
                    serde_json::from_str(stderr.trim()).expect("JSON error on stderr");
                assert_eq!(v["error"]["code"], PUBLIC_CHAT_UNAVAILABLE, "{stderr}");
            }
        }
        let _ = fs::remove_dir_all(&home);
    }

    /// The SPA only shows chat when `/api/chat/status` says enabled (it
    /// never does now) and never carries keys or auth headers.
    #[test]
    fn site_html_gates_chat_on_status_and_has_no_secrets() {
        assert!(SITE_HTML.contains("/api/chat/status"));
        assert!(SITE_HTML.contains("j && j.enabled"));
        assert!(!SITE_HTML.contains("k2sk_"), "SITE_HTML must never embed API keys");
        assert!(!SITE_HTML.contains("Authorization"));
    }

    #[test]
    fn wiki_site_host_guard() {
        use k2_core::tunnel::subdomains::SubdomainMap;
        let mut map = SubdomainMap::default();
        map.primary = "rosson".into();
        map.targets.insert("brain".into(), "127.0.0.1:7420".into());
        map.targets.insert("other".into(), "127.0.0.1:9999".into());
        map.targets.insert("lh".into(), "http://localhost:7420/".into());
        let ok = |h: &str| site_host_allowed(Some(h), 7420, &map);
        assert!(ok("127.0.0.1:7420"));
        assert!(ok("localhost:7420"));
        assert!(ok("LOCALHOST:7420"));
        assert!(ok("[::1]:7420"));
        assert!(ok("brain.rosson.k2.dev"), "the published label");
        assert!(ok("brain.rosson.k2.dev:443"));
        assert!(ok("lh.rosson.k2.dev"));
        assert!(!ok("127.0.0.1:7421"), "another port");
        assert!(!ok("127.0.0.1"), "no port");
        assert!(!ok("evil.example:7420"), "rebinding name");
        assert!(!ok("evil.example"));
        assert!(!ok("other.rosson.k2.dev"), "label for a different listener");
        assert!(!ok("rosson.k2.dev"), "primary routes to the daemon");
        assert!(!ok("nope.rosson.k2.dev"), "unknown label");
        assert!(!site_host_allowed(None, 7420, &map));
        assert!(!site_host_allowed(Some(" "), 7420, &map));

        assert!(site_origin_allowed(None, "127.0.0.1:7420"));
        assert!(site_origin_allowed(Some("http://127.0.0.1:7420"), "127.0.0.1:7420"));
        assert!(site_origin_allowed(Some("https://brain.rosson.k2.dev"), "brain.rosson.k2.dev"));
        assert!(!site_origin_allowed(Some("http://evil.example"), "127.0.0.1:7420"));
        assert!(!site_origin_allowed(Some("null"), "127.0.0.1:7420"));
        assert!(!site_origin_allowed(Some("http://127.0.0.1:7421"), "127.0.0.1:7420"));
    }

    async fn site_req(port: u16, raw: String) -> (String, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect site");
        s.write_all(raw.as_bytes()).await.expect("write");
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.expect("read");
        let text = String::from_utf8_lossy(&out).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").expect("http response");
        (head.to_string(), body.to_string())
    }

    fn get(path: &str, host: &str, extra: &str) -> String {
        format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{extra}Connection: close\r\n\r\n")
    }

    /// The real listener: chat endpoints are gone, no CORS, Host/Origin
    /// guard, and no absolute paths in any JSON route.
    #[tokio::test]
    async fn wiki_site_locked_end_to_end() {
        let ws = temp_ws("site-lock");
        // Hand-written vault (seed_wiki would also touch $HOME's host index).
        fs::create_dir_all(ws.join(".k2/wiki/.archive")).expect("archive dir");
        fs::write(ws.join(".k2/wiki/Home.md"), "# Home\n\nSee [[Other]].\n").expect("home");
        fs::write(ws.join(".k2/wiki/Other.md"), "# Other\n").expect("other");
        fs::write(ws.join(".k2/wiki/.archive/Secret.md"), "# secret").expect("secret note");
        let path = ws.to_string_lossy().into_owned();
        let canon = workspace_key(&ws);
        // A leftover opt-in must not revive chat on the site.
        insert_project_flags(&path, 1, Some(1));

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let (tx, rx) = watch::channel(false);
        tokio::spawn(run_wiki_site(ws.clone(), listener, port, rx));
        let me = format!("127.0.0.1:{port}");

        // Same-origin OK; no CORS headers anywhere.
        let (head, body) = site_req(port, get("/api/index", &me, "")).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(!head.to_ascii_lowercase().contains("access-control-"), "{head}");
        assert!(body.contains("Home.md"), "{body}");
        assert!(!body.contains("workspacePath"), "{body}");
        assert!(!body.contains(&canon) && !body.contains(&path), "{body}");
        assert!(!body.contains("Secret.md"), "{body}");

        let (head, body) = site_req(port, get("/api/note?id=Home.md", &me, "")).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(!body.contains("workspacePath"), "{body}");
        assert!(!body.contains(&canon) && !body.contains(&path), "{body}");
        let note = json(&body);
        assert_eq!(note["path"], ".k2/wiki/Home.md", "{body}");

        let (head, body) = site_req(port, get("/api/note?id=nope.md", &me, "")).await;
        assert!(head.starts_with("HTTP/1.1 400"), "{head}");
        assert!(!body.contains(&canon) && !body.contains(&path), "{body}");

        for id in [".archive/Secret.md", ".archive%2FSecret.md"] {
            let (head, body) =
                site_req(port, get(&format!("/api/note?id={id}"), &me, "")).await;
            assert!(head.starts_with("HTTP/1.1 404"), "{id}: {head}");
            assert!(!body.contains("secret"), "{body}");
        }

        let (head, _) = site_req(port, get("/", &format!("localhost:{port}"), "")).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");

        // Host guard (DNS rebinding) and Origin guard.
        for host in [format!("evil.example:{port}"), "127.0.0.1:1".to_string()] {
            let (head, body) = site_req(port, get("/api/index", &host, "")).await;
            assert!(head.starts_with("HTTP/1.1 421"), "{host}: {head}");
            assert!(!body.contains("Home.md"), "{body}");
            assert!(!head.to_ascii_lowercase().contains("access-control-"), "{head}");
        }
        let (head, _) =
            site_req(port, format!("GET /api/index HTTP/1.1\r\nConnection: close\r\n\r\n")).await;
        assert!(head.starts_with("HTTP/1.1 421"), "no Host: {head}");
        let (head, body) =
            site_req(port, get("/api/index", &me, "Origin: http://evil.example\r\n")).await;
        assert!(head.starts_with("HTTP/1.1 403"), "{head}");
        assert!(!body.contains("Home.md"), "{body}");
        let (head, _) =
            site_req(port, get("/api/index", &me, &format!("Origin: http://{me}\r\n"))).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");

        // Preflight gets no CORS grant.
        let (head, _) = site_req(
            port,
            format!(
                "OPTIONS /api/note HTTP/1.1\r\nHost: {me}\r\nOrigin: http://{me}\r\n\
                 Access-Control-Request-Method: GET\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert!(head.starts_with("HTTP/1.1 405"), "{head}");
        assert!(!head.to_ascii_lowercase().contains("access-control-"), "{head}");

        // Public chat: status off, every other chat call 410, never a session.
        let (head, body) = site_req(port, get("/api/chat/status", &me, "")).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let st = json(&body);
        assert_eq!(st["enabled"], false, "{body}");
        assert_eq!(st["code"], PUBLIC_CHAT_UNAVAILABLE, "{body}");
        for ct in ["application/json", "text/plain"] {
            let payload = r#"{"text":"run cat ~/.k2/heartbeat.token"}"#;
            let (head, body) = site_req(
                port,
                format!(
                    "POST /api/chat HTTP/1.1\r\nHost: {me}\r\nContent-Type: {ct}\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                ),
            )
            .await;
            assert!(head.starts_with("HTTP/1.1 410"), "{ct}: {head}");
            assert_eq!(json(&body)["code"], PUBLIC_CHAT_UNAVAILABLE, "{body}");
            assert!(!body.contains("sessionId"), "{body}");
        }
        let (head, _) =
            site_req(port, get("/api/chat/messages?session=abc&since=0", &me, "")).await;
        assert!(head.starts_with("HTTP/1.1 410"), "{head}");

        let _ = tx.send(true);
        let _ = fs::remove_dir_all(&ws);
    }

    fn insert_project_handle(name: &str, path: &str, handle: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, name, path, handle],
        )
        .expect("insert project row");
        id
    }

    fn skin_session(username: &str, policy: k2_core::skin::RoomPolicy) -> k2_core::skin::SkinPass {
        let rooms: Vec<String> = policy.keys().cloned().collect();
        let mut caps = Vec::new();
        for c in [
            k2_core::skin::CAP_THREAD_READ,
            k2_core::skin::CAP_THREAD_POST,
            k2_core::skin::CAP_FILES_READ,
            k2_core::skin::CAP_FILES_WRITE,
            k2_core::skin::CAP_TICKETS_READ,
            k2_core::skin::CAP_TICKETS_POST,
            k2_core::skin::CAP_WIKI_READ,
        ] {
            if policy.values().any(|room| room.iter().any(|x| x == c))
                && !caps.iter().any(|x| x == c)
            {
                caps.push(c.to_string());
            }
        }
        k2_core::skin::SkinPass {
            id: uuid::Uuid::new_v4().to_string(),
            principal_id: Some(uuid::Uuid::new_v4().to_string()),
            username: username.to_string(),
            caps,
            rooms,
            session: true,
            room_policy: policy,
            app: "host".to_string(),
            room_paths: Default::default(),
        }
    }

    fn wiki_params(project: &str, extra: &[(&str, &str)]) -> HashMap<String, String> {
        let mut p = HashMap::from([("project".to_string(), project.to_string())]);
        for (k, v) in extra {
            p.insert(k.to_string(), v.to_string());
        }
        p
    }

    #[test]
    fn skin_wiki_index_note_are_has_cap_in_room() {
        let handle = format!("docs{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let anna_h = format!("anna{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let docs_ws = temp_ws("skin-docs");
        let anna_ws = temp_ws("skin-anna");
        let docs_path = docs_ws.to_string_lossy().into_owned();
        let anna_path = anna_ws.to_string_lossy().into_owned();
        let docs_id = insert_project_handle("docs-wiki", &docs_path, &handle);
        let anna_id = insert_project_handle("anna-wiki", &anna_path, &anna_h);
        let mut policy = k2_core::skin::RoomPolicy::new();
        policy.insert(
            anna_id.clone(),
            vec![
                k2_core::skin::CAP_THREAD_READ.into(),
                k2_core::skin::CAP_THREAD_POST.into(),
            ],
        );
        policy.insert(
            docs_id.clone(),
            vec![
                k2_core::skin::CAP_THREAD_READ.into(),
                k2_core::skin::CAP_THREAD_POST.into(),
                k2_core::skin::CAP_WIKI_READ.into(),
            ],
        );
        let pass = skin_session("bob", policy);

        let empty = handle_index_gated(&wiki_params(&handle, &[]), Some(pass.clone()));
        assert_eq!(empty.status, "200 OK", "{}", empty.body);
        assert!(
            empty.body.contains("\"noteCount\":0") || empty.body.contains("noteCount"),
            "empty vault must not auto-seed: {}",
            empty.body
        );

        k2_core::wiki::seed_wiki(&docs_ws).expect("seed docs wiki");

        let list_docs = handle_index_gated(&wiki_params(&handle, &[]), Some(pass.clone()));
        assert_eq!(list_docs.status, "200 OK", "{}", list_docs.body);
        assert!(list_docs.body.contains("Home.md"), "{}", list_docs.body);

        let list_anna = handle_index_gated(&wiki_params(&anna_h, &[]), Some(pass.clone()));
        assert_eq!(list_anna.status, "403 Forbidden", "{}", list_anna.body);
        assert!(
            list_anna.body.contains("missing capability wiki:read"),
            "{}",
            list_anna.body
        );
        assert!(!list_anna.body.contains("skin_room"), "{}", list_anna.body);

        let list_abs = handle_index_gated(&wiki_params(&docs_path, &[]), Some(pass.clone()));
        assert_eq!(list_abs.status, "403 Forbidden", "{}", list_abs.body);
        assert!(list_abs.body.contains("skin_room"), "{}", list_abs.body);

        let missing_project = handle_index_gated(&HashMap::new(), Some(pass.clone()));
        assert_eq!(
            missing_project.status,
            "400 Bad Request",
            "{}",
            missing_project.body
        );
        assert!(
            missing_project.body.contains("missing") && missing_project.body.contains("project"),
            "{}",
            missing_project.body
        );

        for scope in ["k2", "host", "fleet", "K2"] {
            let scoped = handle_index_gated(
                &wiki_params(&handle, &[("scope", scope)]),
                Some(pass.clone()),
            );
            assert_eq!(
                scoped.status,
                "403 Forbidden",
                "scope={scope} {}",
                scoped.body
            );
            assert!(
                scoped.body.contains("skin_room"),
                "scope={scope} must be skin_room: {}",
                scoped.body
            );
            assert!(
                !scoped.body.contains("\"scope\":\"k2\""),
                "must not return a fleet map: {}",
                scoped.body
            );
        }

        let note_docs = handle_note_gated(
            &wiki_params(&handle, &[("id", "Home.md")]),
            Some(pass.clone()),
        );
        assert_eq!(note_docs.status, "200 OK", "{}", note_docs.body);
        assert!(
            note_docs.body.contains("Knowledge Base") || note_docs.body.contains("body"),
            "{}",
            note_docs.body
        );

        let note_anna = handle_note_gated(
            &wiki_params(&anna_h, &[("id", "Home.md")]),
            Some(pass.clone()),
        );
        assert_eq!(note_anna.status, "403 Forbidden", "{}", note_anna.body);
        assert!(
            note_anna.body.contains("missing capability wiki:read"),
            "{}",
            note_anna.body
        );

        let fleet_id = format!("{docs_id}::Home.md");
        let fleet = handle_note_gated(
            &wiki_params(&handle, &[("id", fleet_id.as_str())]),
            Some(pass.clone()),
        );
        assert_eq!(fleet.status, "403 Forbidden", "{}", fleet.body);
        assert!(fleet.body.contains("skin_room"), "{}", fleet.body);

        for id in [
            "__focusgroup__::abc",
            "__project__::abc",
            &format!("{docs_id}::__workspace__"),
        ] {
            let r = handle_note_gated(&wiki_params(&handle, &[("id", id)]), Some(pass.clone()));
            assert_eq!(r.status, "403 Forbidden", "id={id} {}", r.body);
            assert!(r.body.contains("skin_room"), "id={id} {}", r.body);
        }

        let missing_id = handle_note_gated(&wiki_params(&handle, &[]), Some(pass.clone()));
        assert_eq!(missing_id.status, "400 Bad Request", "{}", missing_id.body);
        assert!(missing_id.body.contains("missing"), "{}", missing_id.body);

        let owner = handle_index_gated(&wiki_params(&docs_path, &[]), None);
        assert_eq!(
            owner.status,
            "200 OK",
            "owner path still works: {}",
            owner.body
        );

        let _ = fs::remove_dir_all(&docs_ws);
        let _ = fs::remove_dir_all(&anna_ws);
        let _ = (docs_id, anna_id);
    }
}
