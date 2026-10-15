//! `/cli/vault/*` — the K2 vault (`prd-k2-vault-v1.md` §15, §22.3, §23.1).
//!
//! Callers (`vault::actor`):
//! - owner token and Owner/Admin logins: everything on `user` secrets;
//!   widening is audited (KV16; VS1 is deferred, so the owner token keeps
//!   owner rights in 0.45.2). Reveal-class actions need a confirm token.
//! - Member logins: their workspaces' secrets, no widening, no reveal.
//! - agent and K2 shell-tab passports: their own workspace (+ grants);
//!   §11.2 limits; never confirm, reveal, recovery or key operations.
//! - app passes / API keys: refused.
//!
//! Values travel only in POST bodies (`valueB64`), never in a URL; a
//! `value` query parameter is refused (`value_on_argv`). No response
//! carries a `system` value. Responses carry a `user` value only for
//! `get` (an `api` grant) and `reveal` (owner confirm).

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use k2_vault::inject::{self, Inject};
use k2_vault::vault::VaultStatus;
use k2_vault::{
    Actor, AuditQuery, Consumer, ConsumerKind, Delivery, GrantRequest, HumanProof, Kind, ListFilter, LocalVault,
    Namespace, PolicyPatch, PutMode, PutRequest, Requester, Scope, ScopeKind, SecretBytes, SecretMeta, SecretStore,
    Selector, Use, VaultError,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::vault::actor::{Channel, VaultCaller};

pub const STATUS: &str = "/cli/vault/status";
pub const LIST: &str = "/cli/vault/list";
pub const SHOW: &str = "/cli/vault/show";
pub const REQUESTS: &str = "/cli/vault/requests";
pub const DOCTOR: &str = "/cli/vault/doctor";
pub const AUDIT: &str = "/cli/vault/audit";
pub const PRESETS: &str = "/cli/vault/presets";

pub const SET: &str = "/cli/vault/set";
pub const ROTATE: &str = "/cli/vault/rotate";
pub const POLICY: &str = "/cli/vault/policy";
pub const GRANT: &str = "/cli/vault/grant";
pub const REVOKE: &str = "/cli/vault/revoke";
pub const DISABLE: &str = "/cli/vault/disable";
pub const ENABLE: &str = "/cli/vault/enable";
pub const DESTROY: &str = "/cli/vault/destroy";
pub const GET: &str = "/cli/vault/get";
pub const CONFIRM_START: &str = "/cli/vault/confirm/start";
pub const CONFIRM_FINISH: &str = "/cli/vault/confirm/finish";
pub const CONFIRM_PASSWORD_SET: &str = "/cli/vault/confirm-password/set";
pub const REVEAL: &str = "/cli/vault/reveal";
pub const RECOVERY_SHOW_ONCE: &str = "/cli/vault/recovery/show-once";
pub const RECOVERY_CONFIRM: &str = "/cli/vault/recovery/confirm";
pub const RECOVERY_ROTATE: &str = "/cli/vault/recovery/rotate";
pub const KEK_ROTATE: &str = "/cli/vault/kek/rotate";
pub const REKEY: &str = "/cli/vault/rekey";
pub const ROLLBACK: &str = "/cli/vault/rollback";
pub const RESTORE: &str = "/cli/vault/restore";

/// Readable routes (passports allowed where marked in `is_agent_verb`).
pub const READ_ROUTES: &[&str] = &[STATUS, LIST, SHOW, REQUESTS, DOCTOR, AUDIT, PRESETS];

/// POST-only routes.
pub const POST_ROUTES: &[&str] = &[
    SET,
    ROTATE,
    POLICY,
    GRANT,
    REVOKE,
    DISABLE,
    ENABLE,
    DESTROY,
    GET,
    CONFIRM_START,
    CONFIRM_FINISH,
    CONFIRM_PASSWORD_SET,
    REVEAL,
    RECOVERY_SHOW_ONCE,
    RECOVERY_CONFIRM,
    RECOVERY_ROTATE,
    KEK_ROTATE,
    REKEY,
    ROLLBACK,
    RESTORE,
];

/// Routes no passport (agent or K2 shell tab) may reach (KV14 DENY).
pub const PASSPORT_DENIED: &[&str] = &[
    CONFIRM_START,
    CONFIRM_FINISH,
    CONFIRM_PASSWORD_SET,
    REVEAL,
    RECOVERY_SHOW_ONCE,
    RECOVERY_CONFIRM,
    RECOVERY_ROTATE,
    KEK_ROTATE,
    REKEY,
    ROLLBACK,
    RESTORE,
];

/// Request bodies are small JSON (fetch bodies above 64 KiB go by file).
pub const MAX_BODY: usize = 96 * 1024;

pub fn is_route(path: &str) -> bool {
    READ_ROUTES.contains(&path) || POST_ROUTES.contains(&path) || crate::vault_use_routes::is_route(path)
}

pub fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path) || crate::vault_use_routes::is_post_route(path)
}

// ── responses ───────────────────────────────────────────────────────

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        422 => "422 Unprocessable Entity",
        429 => "429 Too Many Requests",
        501 => "501 Not Implemented",
        502 => "502 Bad Gateway",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

pub fn refuse(e: &VaultError) -> CliResponse {
    CliResponse {
        status: status_line(e.status),
        content_type: "application/json",
        body: json!({ "error": { "code": e.code, "hint": e.hint } }).to_string(),
    }
}

fn ok(v: Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

pub fn too_large() -> CliResponse {
    refuse(&VaultError::new("body_too_large", 413, "request bodies here are at most 96 KiB; send larger fetch bodies as a file"))
}

fn err(code: &'static str, status: u16, hint: impl Into<String>) -> VaultError {
    VaultError::new(code, status, hint)
}

// ── body shapes ─────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sel {
    pub id: Option<String>,
    pub name: Option<String>,
    /// `workspace` | `server` | `session`
    pub scope: Option<String>,
    pub workspace: Option<String>,
    pub session: Option<String>,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    #[serde(default)]
    pub system: bool,
}

fn parse<T: for<'de> Deserialize<'de> + Default>(body: &[u8]) -> Result<T, VaultError> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| err("bad_json", 400, format!("bad request body: {}", e.classify_str())))
}

trait ClassifyStr {
    fn classify_str(&self) -> &'static str;
}

impl ClassifyStr for serde_json::Error {
    fn classify_str(&self) -> &'static str {
        // Never echo body text (it may hold a value).
        match self.classify() {
            serde_json::error::Category::Io => "io",
            serde_json::error::Category::Syntax => "not valid JSON",
            serde_json::error::Category::Data => "a field has the wrong type",
            serde_json::error::Category::Eof => "truncated JSON",
        }
    }
}

/// The scope a request names, with the caller's defaults.
pub fn scope_for(c: &VaultCaller, scope: Option<&str>, workspace: Option<&str>, session: Option<&str>) -> Result<Scope, VaultError> {
    let ws_id = |w: Option<&str>| -> Result<String, VaultError> {
        match (w, c.workspace_id()) {
            (Some(w), _) if !w.trim().is_empty() => {
                crate::vault::workspace_id(w).ok_or_else(|| VaultError::not_found(format!("no workspace named {w}")))
            }
            (_, Some(own)) => Ok(own.to_string()),
            _ => Err(err("workspace_required", 400, "name the workspace (--scope ws:<name>)")),
        }
    };
    match scope.unwrap_or("") {
        "server" => Ok(Scope::server()),
        "session" => {
            let s = session
                .map(str::to_string)
                .or_else(|| c.session_id.clone())
                .ok_or_else(|| err("session_required", 400, "chat secrets need a session"))?;
            Ok(Scope::session(s))
        }
        "workspace" | "ws" => Ok(Scope::workspace(ws_id(workspace)?)),
        "" => {
            if workspace.is_some() || c.workspace_id().is_some() {
                Ok(Scope::workspace(ws_id(workspace)?))
            } else {
                Ok(Scope::server())
            }
        }
        other => Err(err("bad_scope", 400, format!("unknown scope {other} (workspace, server or session)"))),
    }
}

pub fn selector(c: &VaultCaller, s: &Sel) -> Result<Selector, VaultError> {
    if let Some(id) = s.id.as_deref().filter(|x| !x.is_empty()) {
        return Ok(Selector::Id(id.to_string()));
    }
    if let Some(r) = s.reference.as_deref().filter(|x| !x.is_empty()) {
        return crate::vault::selector_for_ref(r);
    }
    let name = s.name.as_deref().filter(|x| !x.is_empty()).ok_or_else(|| err("name_required", 400, "name the secret"))?;
    let scope = scope_for(c, s.scope.as_deref(), s.workspace.as_deref(), s.session.as_deref())?;
    Ok(Selector::Name {
        namespace: if s.system { Namespace::System } else { Namespace::User },
        scope,
        name: name.to_string(),
    })
}

fn decode_value(b64: Option<&str>) -> Result<SecretBytes, VaultError> {
    let s = b64.ok_or_else(|| err("value_required", 400, "send the value on stdin (k2 vault set NAME < file)"))?;
    let v = base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|_| err("bad_value", 400, "valueB64 isn't base64"))?;
    Ok(SecretBytes::new(v))
}

pub fn meta_json(m: &SecretMeta) -> Value {
    let mut v = serde_json::to_value(m).unwrap_or(Value::Null);
    if let Value::Object(o) = &mut v {
        o.insert("ref".into(), json!(crate::vault::display_ref(m)));
        o.insert("agentCanSee".into(), json!(m.agent_can_see()));
        if m.scope.kind == ScopeKind::Workspace {
            o.insert("workspace".into(), json!(crate::vault::workspace_label(&m.scope.id)));
        }
        let hosts: Vec<Value> = m
            .hosts
            .iter()
            .map(|h| {
                let host = h.split("://").nth(1).and_then(|x| x.rsplit_once(':')).map(|x| x.0).unwrap_or(h);
                let (_a, u, mixed) = k2_vault::hosts::display_host(host.trim_start_matches("*."));
                json!({ "origin": h, "unicode": u, "mixedScript": mixed })
            })
            .collect();
        o.insert("hostsDisplay".into(), Value::Array(hosts));
    }
    v
}

fn status_json(st: &VaultStatus) -> Value {
    let mut v = serde_json::to_value(st).unwrap_or(Value::Null);
    if let Value::Object(o) = &mut v {
        o.insert("honest".into(), json!(honest_line(&st.key_home)));
        o.insert("nonDumpable".into(), json!(crate::vault::non_dumpable()));
        o.insert(
            "adopted".into(),
            json!(crate::vault::adopted()
                .iter()
                .map(|a| json!({ "store": a.store, "moved": a.moved.len(), "kept": a.kept.len(), "errors": a.errors }))
                .collect::<Vec<_>>()),
        );
        o.insert("agentsCanRead".into(), json!(agents_can_read(&st.key_home)));
        o.insert(
            "legacyCopies".into(),
            json!("older copies of secrets K2 kept before 0.45.2 may remain in snapshots and backups made before the move"),
        );
        if st.recovery != "confirmed" {
            o.insert(
                "recoveryHint".into(),
                json!(RECOVERY_HINT),
            );
        }
    }
    v
}

/// Who sets up the recovery key, and where (VS2). People only.
pub const RECOVERY_HINT: &str = "Only you should do this: in the K2 app, Settings → Vault → Set up recovery key, or from your own SSH login or a terminal outside K2: k2 vault recovery show-once. Agents: don't run it; tell your owner.";

/// §18 / VS21 per-mode copy.
pub fn honest_line(key_home: &str) -> &'static str {
    match key_home {
        "keychain" => "Secrets are encrypted and never shown to guests, apps or other servers. A secret bound to a site is only ever sent to that site. Every use is logged. The vault's key is in your Mac's Keychain.",
        "keeper" | "keeper+tpm" => "Secrets are encrypted and never shown to guests, apps or other servers. A secret bound to a site is only ever sent to that site. Every use is logged. The vault's key is held by this server's root helper; agents with admin rights can still reach it.",
        "systemd-creds" => "Secrets are encrypted and never shown to guests, apps or other servers. A secret bound to a site is only ever sent to that site. Every use is logged. The vault's key is a system credential, not a file in the home folder; agents on this server can still read it while K2 runs.",
        _ => "Secrets are encrypted and never shown to guests, apps or other servers. A secret bound to a site is only ever sent to that site. Every use is logged. The vault's key is a file on this server that agents can read, and full-disk copies include it. Secrets stay out of chats, logs and copies K2 makes.",
    }
}

fn agents_can_read(key_home: &str) -> &'static str {
    match key_home {
        "keychain" => "prompt",
        _ => "yes",
    }
}

// ── entry points ────────────────────────────────────────────────────

/// Dispatcher entry (TCP and cell socket).
pub fn handle(path: &str, caller: Result<VaultCaller, VaultError>, body: &[u8], params: &HashMap<String, String>, ingress: &str) -> CliResponse {
    handle_with_peer(path, caller, body, params, ingress, None)
}

/// Vault-socket entry: `not_person` says why the peer isn't a person.
pub fn handle_with_peer(
    path: &str,
    caller: Result<VaultCaller, VaultError>,
    body: &[u8],
    params: &HashMap<String, String>,
    ingress: &str,
    not_person: Option<&'static str>,
) -> CliResponse {
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => {
            if path == STATUS {
                return ok(json!({ "open": false, "error": { "code": e.code, "hint": e.hint } }));
            }
            return refuse(&e);
        }
    };
    handle_on(&vault, path, caller, body, params, ingress, not_person)
}

/// Pure entry over a given vault (tests).
pub fn handle_on(
    vault: &Arc<LocalVault>,
    path: &str,
    caller: Result<VaultCaller, VaultError>,
    body: &[u8],
    params: &HashMap<String, String>,
    ingress: &str,
    not_person: Option<&'static str>,
) -> CliResponse {
    let c = match caller {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    // Values never ride in a URL.
    if params.keys().any(|k| matches!(k.as_str(), "value" | "valueB64" | "secret" | "password")) {
        return refuse(&err("value_on_argv", 400, "values go on stdin, never on the command line or in a URL"));
    }
    if c.is_passport() && PASSPORT_DENIED.contains(&path) {
        return refuse(&err(
            "owner_only",
            403,
            "this is for the owner, from Settings → Vault or a terminal outside K2; agents can't do it",
        ));
    }
    if crate::vault_use_routes::is_route(path) {
        return crate::vault_use_routes::handle(vault, path, &c, body, params, ingress);
    }
    let r = match path {
        STATUS => Ok(status_json(&vault.status())),
        LIST => list(vault, &c, params),
        SHOW => show(vault, &c, params),
        REQUESTS => requests(vault, &c),
        DOCTOR => doctor(vault, &c),
        AUDIT => audit(vault, &c, params),
        PRESETS => Ok(json!({ "presets": inject::PRESETS })),
        SET => set(vault, &c, body),
        ROTATE => rotate(vault, &c, body),
        POLICY => policy(vault, &c, body),
        GRANT => grant(vault, &c, body),
        REVOKE => revoke(vault, &c, body),
        DISABLE | ENABLE | DESTROY => status_change(vault, &c, path, body),
        GET => get_value(vault, &c, body),
        CONFIRM_START => confirm_start(vault, &c, body),
        CONFIRM_FINISH => confirm_finish(vault, &c, body, ingress),
        CONFIRM_PASSWORD_SET => confirm_password_set(vault, &c, body, ingress, not_person),
        REVEAL => reveal(vault, &c, body, ingress),
        RECOVERY_SHOW_ONCE | RECOVERY_ROTATE => recovery_show(vault, &c, path, body, ingress, not_person),
        RECOVERY_CONFIRM => recovery_confirm(vault, &c, body, ingress, not_person),
        KEK_ROTATE | REKEY => key_op(vault, &c, path, body, ingress),
        ROLLBACK => rollback(vault, &c, body, ingress),
        RESTORE => restore(vault, &c, body, ingress, not_person),
        _ => Err(VaultError::not_found("no such vault route")),
    };
    match r {
        Ok(v) => ok(v),
        Err(e) => refuse(&e),
    }
}

// ── reads ───────────────────────────────────────────────────────────

fn list(vault: &LocalVault, c: &VaultCaller, params: &HashMap<String, String>) -> Result<Value, VaultError> {
    let include_system = params.get("system").is_some_and(|v| v == "1" || v == "true");
    let scope = match params.get("scope").map(String::as_str) {
        None | Some("") | Some("all") => None,
        Some(s) => Some(scope_for(c, Some(s), params.get("workspace").map(String::as_str), params.get("session").map(String::as_str))?),
    };
    let f = ListFilter { include_system, scope, include_destroyed: params.get("destroyed").is_some_and(|v| v == "1") };
    let items = vault.list(&f, &c.actor)?;
    Ok(json!({ "secrets": items.iter().map(meta_json).collect::<Vec<_>>() }))
}

fn sel_from_params(params: &HashMap<String, String>) -> Sel {
    Sel {
        id: params.get("id").cloned(),
        name: params.get("name").cloned(),
        scope: params.get("scope").cloned(),
        workspace: params.get("workspace").cloned(),
        session: params.get("session").cloned(),
        reference: params.get("ref").cloned(),
        system: params.get("system").is_some_and(|v| v == "1" || v == "true"),
    }
}

fn show(vault: &LocalVault, c: &VaultCaller, params: &HashMap<String, String>) -> Result<Value, VaultError> {
    let sel = selector(c, &sel_from_params(params))?;
    let m = vault.meta(&sel, &c.actor)?;
    let grants = vault.grants(&sel, &c.actor)?;
    let audit = vault
        .audit(&AuditQuery { secret_id: Some(m.id.clone()), limit: Some(20), ..Default::default() }, &c.actor)
        .unwrap_or_default();
    let grants: Vec<Value> = grants
        .iter()
        .map(|g| {
            let mut v = serde_json::to_value(g).unwrap_or(Value::Null);
            if let Value::Object(o) = &mut v {
                if g.consumer.kind == ConsumerKind::Workspace && g.consumer.id != "*" {
                    o.insert("consumerLabel".into(), json!(format!("ws:{}", crate::vault::workspace_label(&g.consumer.id))));
                } else {
                    o.insert("consumerLabel".into(), json!(g.consumer.label()));
                }
            }
            v
        })
        .collect();
    Ok(json!({ "secret": meta_json(&m), "grants": grants, "audit": audit }))
}

fn requests(vault: &LocalVault, c: &VaultCaller) -> Result<Value, VaultError> {
    let ws = c.workspace_id().map(str::to_string);
    if ws.is_none() && !c.actor.is_admin_human() && !c.actor.is_member() {
        return Err(err("forbidden", 403, "not allowed"));
    }
    let list = vault.requests(ws.as_deref())?;
    Ok(json!({ "requests": list }))
}

fn audit(vault: &LocalVault, c: &VaultCaller, params: &HashMap<String, String>) -> Result<Value, VaultError> {
    let secret_id = if params.contains_key("name") || params.contains_key("id") || params.contains_key("ref") {
        let sel = selector(c, &sel_from_params(params))?;
        Some(vault.meta(&sel, &c.actor)?.id)
    } else {
        None
    };
    if c.actor.is_member() {
        return Err(err("role_required", 403, "the vault audit needs an Admin or Owner"));
    }
    let q = AuditQuery {
        secret_id,
        since: params.get("since").cloned(),
        workspace_id: None,
        limit: params.get("limit").and_then(|l| l.parse().ok()),
    };
    vault.flush_rollups();
    Ok(json!({ "audit": vault.audit(&q, &c.actor)? }))
}

/// `k2 vault doctor`: key home, recovery, tamper, rotations due, and
/// plaintext `.env` files with sensitive names in workspaces.
fn doctor(vault: &LocalVault, c: &VaultCaller) -> Result<Value, VaultError> {
    let st = vault.status();
    let mut findings: Vec<Value> = vec![];
    if st.recovery != "confirmed" {
        let level = if cfg!(target_os = "linux") { "error" } else { "warning" };
        findings.push(json!({
            "level": level,
            "code": "recovery_unconfirmed",
            "message": format!("The vault has no confirmed recovery key. {RECOVERY_HINT}"),
        }));
    }
    if st.tamper != "ok" {
        findings.push(json!({ "level": "error", "code": "vault_tamper", "message": st.tamper }));
    }
    if st.rollback {
        findings.push(json!({ "level": "error", "code": "vault_rollback", "message": "vault.db looks older than expected; the owner must confirm in Settings → Vault" }));
    }
    if st.key_home == "file" {
        findings.push(json!({ "level": "info", "code": "key_is_a_file", "message": honest_line("file") }));
    }
    for m in vault.list(&ListFilter::default(), &c.actor)? {
        if let (Some(d), Ok(u)) = (m.rotate_every_days, chrono::DateTime::parse_from_rfc3339(&m.updated_at)) {
            if chrono::Utc::now().signed_duration_since(u.with_timezone(&chrono::Utc)) > chrono::Duration::days(i64::from(d)) {
                findings.push(json!({ "level": "warning", "code": "rotation_due", "message": format!("{} is due for rotation", m.name), "secret": m.id }));
            }
        }
    }
    for (name, notice) in legacy_env_services(c) {
        findings.push(json!({
            "level": "warning",
            "code": "publish_env_inherited",
            "service": name,
            "message": format!("published service {name}: {notice}"),
        }));
    }
    for f in plaintext_env_files(c) {
        findings.push(json!({
            "level": "warning",
            "code": "plaintext_env",
            "message": format!("{} holds {} in plain text; move it in with: k2 vault import {}", f.0, f.1.join(", "), f.0),
        }));
    }
    Ok(json!({ "ok": findings.iter().all(|f| f["level"] != "error"), "findings": findings }))
}

/// Published services still on the inherited environment (deprecated,
/// ends in 0.46.0), as (name, notice). Scoped to the caller's workspace
/// when it has one.
fn legacy_env_services(c: &VaultCaller) -> Vec<(String, String)> {
    let ids = crate::vault::publish_env::legacy_ids();
    if ids.is_empty() {
        return vec![];
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut st = match conn.prepare("SELECT id, project_id, name, kind FROM published_services") {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?.unwrap_or_default(),
        ))
    });
    let mut out = vec![];
    if let Ok(it) = rows {
        for (id, project_id, name, kind) in it.flatten() {
            if kind == k2_core::published_services::KIND_SKIN || !ids.contains(&id) {
                continue;
            }
            if let Some(ws) = c.workspace_id() {
                if ws != project_id {
                    continue;
                }
            }
            let notice = crate::vault::publish_env::legacy_env_notice(&name);
            out.push((name, notice));
        }
    }
    out.sort();
    out
}

/// Workspace `.env` files (top level) with sensitive names holding values
/// that aren't `k2v://` references.
fn plaintext_env_files(c: &VaultCaller) -> Vec<(String, Vec<String>)> {
    let paths: Vec<String> = match c.workspace_id() {
        Some(ws) => crate::vault::workspace_path(ws).into_iter().collect(),
        None => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let mut st = match conn.prepare("SELECT path FROM projects") {
                Ok(s) => s,
                Err(_) => return vec![],
            };
            let rows = st.query_map([], |r| r.get::<_, String>(0));
            match rows {
                Ok(it) => it.flatten().collect(),
                Err(_) => vec![],
            }
        }
    };
    let mut out = vec![];
    for p in paths {
        let dir = std::path::Path::new(&p);
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !(name == ".env" || name.starts_with(".env.")) || name == ".env.k2" || name.ends_with(".example") || name.ends_with(".sample") {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() || meta.len() > 256 * 1024 {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            let keys: Vec<String> = text
                .lines()
                .filter_map(|l| {
                    let l = l.trim().trim_start_matches("export ");
                    let (k, v) = l.split_once('=')?;
                    let k = k.trim();
                    let v = v.trim().trim_matches('"').trim_matches('\'');
                    (k2_vault::delivery::is_sensitive_name(k) && !v.is_empty() && !k2_vault::refs::VaultRef::is_ref(v))
                        .then(|| k.to_string())
                })
                .collect();
            if !keys.is_empty() {
                out.push((e.path().display().to_string(), keys));
            }
        }
    }
    out
}

// ── writes ──────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InjectBody {
    dst: Option<String>,
    fmt: Option<String>,
    #[serde(default)]
    allowed_dst: Vec<String>,
    #[serde(default)]
    allowed_fmt: Vec<String>,
}

impl InjectBody {
    fn build(&self) -> Result<Option<Inject>, VaultError> {
        if self.dst.is_none() && self.fmt.is_none() {
            return Ok(None);
        }
        let dst = k2_vault::inject::Dst::parse(self.dst.as_deref().unwrap_or("header:Authorization"))?;
        let mut i = Inject {
            dst,
            fmt: self.fmt.clone().unwrap_or_else(|| "{}".into()),
            allowed_dst: vec![],
            allowed_fmt: self.allowed_fmt.clone(),
        };
        for d in &self.allowed_dst {
            i.allowed_dst.push(k2_vault::inject::Dst::parse(d)?);
        }
        i.validate()?;
        Ok(Some(i))
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetBody {
    #[serde(flatten)]
    sel: Sel,
    kind: Option<String>,
    #[serde(default)]
    hosts: Vec<String>,
    inject: Option<InjectBody>,
    preset: Option<String>,
    delivery: Option<String>,
    ceiling: Option<String>,
    description: Option<String>,
    rotate_every_days: Option<u32>,
    ttl_secs: Option<i64>,
    value_b64: Option<String>,
    /// `create` | `rotate` | `upsert` (default upsert)
    mode: Option<String>,
}

fn delivery_opt(s: Option<&str>) -> Result<Option<Delivery>, VaultError> {
    s.map(request_delivery).transpose()
}

/// A delivery named in a request. `proxy` (the loopback LLM proxy,
/// prd-k2-vault-v1 §9.3 / V5) is held back from this release: nothing
/// would deliver it, so it is refused rather than silently doing nothing.
pub fn request_delivery(s: &str) -> Result<Delivery, VaultError> {
    match Delivery::parse(s)? {
        Delivery::Proxy => Err(err(
            "delivery_not_available",
            400,
            "proxy delivery isn't in this release; use fetch (K2 makes the call) or file/env",
        )),
        d => Ok(d),
    }
}

/// Preset hosts + inject for a named provider (VS3: people confirm).
fn apply_preset(preset: Option<&str>, hosts: &mut Vec<String>, inject: &mut Option<Inject>) -> Result<(), VaultError> {
    if let Some(p) = preset.filter(|p| !p.is_empty()) {
        let p = inject::preset(p).ok_or_else(|| err("unknown_preset", 400, format!("no preset {p}; see k2 vault presets")))?;
        if hosts.is_empty() {
            *hosts = p.host_list();
        }
        if inject.is_none() {
            *inject = Some(p.inject());
        }
    }
    Ok(())
}

fn set(vault: &LocalVault, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    let b: SetBody = parse(body)?;
    let value = decode_value(b.value_b64.as_deref())?;
    let name = b.sel.name.clone().ok_or_else(|| err("name_required", 400, "name the secret"))?;
    let scope = scope_for(c, b.sel.scope.as_deref(), b.sel.workspace.as_deref(), b.sel.session.as_deref())?;
    let kind = b.kind.as_deref().map(Kind::parse).transpose()?.unwrap_or(Kind::ApiKey);
    let mut req = PutRequest::new(Namespace::User, scope.clone(), name, kind);
    let mut hosts = b.hosts.clone();
    let mut inj = b.inject.as_ref().map(InjectBody::build).transpose()?.flatten();
    apply_preset(b.preset.as_deref(), &mut hosts, &mut inj)?;
    req.hosts = hosts;
    req.inject = inj;
    req.default_delivery = delivery_opt(b.delivery.as_deref())?;
    req.delivery_ceiling = delivery_opt(b.ceiling.as_deref())?;
    req.description = b.description.clone().unwrap_or_default();
    req.rotate_every_days = b.rotate_every_days;
    req.mode = match b.mode.as_deref() {
        Some("create") => PutMode::Create,
        Some("rotate") => PutMode::Rotate,
        _ => PutMode::Upsert,
    };
    if scope.kind == ScopeKind::Session {
        let ttl = b.ttl_secs.unwrap_or(24 * 3600).clamp(60, 30 * 24 * 3600);
        req.expires_at = Some((chrono::Utc::now() + chrono::Duration::seconds(ttl)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    }
    let out = vault.put(req, value, &c.actor)?;
    if out.owner_notice {
        owner_notice(&c.actor, &out.meta);
    }
    let r = crate::vault::display_ref(&out.meta);
    Ok(json!({
        "ok": true,
        "created": out.created,
        "newVersion": out.new_version,
        "trimmed": out.trimmed,
        "ref": r,
        "secret": meta_json(&out.meta),
        "use": use_hint(&out.meta),
    }))
}

/// One line telling an agent how to use what it just got.
pub fn use_hint(m: &SecretMeta) -> String {
    match m.default_delivery {
        Delivery::Fetch | Delivery::Proxy if !m.hosts.is_empty() => {
            format!("k2 vault fetch --secret {} https://{}/…", m.name, m.hosts[0].split("://").nth(1).and_then(|h| h.rsplit_once(':')).map(|h| h.0).unwrap_or(""))
        }
        Delivery::Fetch | Delivery::Proxy => "add a site first: ask the owner, or k2 vault hosts".into(),
        Delivery::File => format!("$K2_SECRETS_DIR/{} (new sessions; read the file)", m.name),
        Delivery::Env => format!("${} in new sessions", m.name),
        Delivery::Api => format!("k2 vault get {}", m.name),
        Delivery::None => "not delivered".into(),
    }
}

/// VS11: an agent replaced a person's value. The owner sees it, with undo.
fn owner_notice(actor: &Actor, m: &SecretMeta) {
    let who = actor.stamp();
    crate::vault::auth_audit("vault-agent-rotate", &who, &format!("ok {} v{} (undo: k2 vault rollback {} --to {})", m.name, m.latest_version, m.name, m.latest_version.saturating_sub(1)), "-");
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::SyncProjects,
        json!({ "vaultNotice": { "secret": m.id, "name": m.name, "by": who, "version": m.latest_version } }),
    );
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RotateBody {
    #[serde(flatten)]
    sel: Sel,
    value_b64: Option<String>,
}

fn rotate(vault: &LocalVault, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    let b: RotateBody = parse(body)?;
    let value = decode_value(b.value_b64.as_deref())?;
    let sel = selector(c, &b.sel)?;
    let out = vault.rotate(&sel, value, &c.actor)?;
    if out.owner_notice {
        owner_notice(&c.actor, &out.meta);
    }
    Ok(json!({ "ok": true, "version": out.meta.latest_version, "trimmed": out.trimmed, "secret": meta_json(&out.meta) }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolicyBody {
    #[serde(flatten)]
    sel: Sel,
    hosts: Option<Vec<String>>,
    #[serde(default)]
    add_hosts: Vec<String>,
    #[serde(default)]
    remove_hosts: Vec<String>,
    delivery: Option<String>,
    ceiling: Option<String>,
    inject: Option<InjectBody>,
    preset: Option<String>,
    description: Option<String>,
    rotate_every_days: Option<u32>,
}

fn policy(vault: &LocalVault, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    let b: PolicyBody = parse(body)?;
    let sel = selector(c, &b.sel)?;
    let mut inject = b.inject.as_ref().map(InjectBody::build).transpose()?.flatten();
    let mut preset_hosts = vec![];
    apply_preset(b.preset.as_deref(), &mut preset_hosts, &mut inject)?;
    let mut add = b.add_hosts.clone();
    add.extend(preset_hosts);
    let p = PolicyPatch {
        hosts: b.hosts.clone(),
        add_hosts: add,
        remove_hosts: b.remove_hosts.clone(),
        default_delivery: delivery_opt(b.delivery.as_deref())?,
        delivery_ceiling: delivery_opt(b.ceiling.as_deref())?,
        inject,
        description: b.description.clone(),
        rotate_every_days: b.rotate_every_days,
    };
    let m = vault.set_policy(&sel, p, &c.actor)?;
    Ok(json!({ "ok": true, "secret": meta_json(&m) }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantBody {
    #[serde(flatten)]
    sel: Sel,
    /// `ws:<name>` | `ws:*` | `session:<id>` | `service:<label or id>`
    to: Option<String>,
    #[serde(rename = "as")]
    delivery: Option<String>,
    env_name: Option<String>,
    file_name: Option<String>,
    hosts: Option<Vec<String>>,
    expires_at: Option<String>,
    version: Option<u32>,
}

pub fn consumer_for(c: &VaultCaller, to: &str) -> Result<Consumer, VaultError> {
    let (kind, id) = to.split_once(':').ok_or_else(|| err("bad_consumer", 400, "use ws:<name>, ws:*, session:<id> or service:<label>"))?;
    match kind {
        "ws" | "workspace" if id == "*" => Ok(Consumer::all_workspaces()),
        "ws" | "workspace" => {
            let id = crate::vault::workspace_id(id).ok_or_else(|| VaultError::not_found(format!("no workspace named {id}")))?;
            Ok(Consumer::workspace(id))
        }
        "session" => Ok(Consumer::session(id)),
        "service" => {
            let sid = crate::vault_use_routes::published_service_id(c, id)?;
            Ok(Consumer::published_service(sid))
        }
        _ => Err(err("bad_consumer", 400, "use ws:<name>, ws:*, session:<id> or service:<label>")),
    }
}

fn grant(vault: &LocalVault, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    let b: GrantBody = parse(body)?;
    let sel = selector(c, &b.sel)?;
    let to = b.to.as_deref().ok_or_else(|| err("consumer_required", 400, "who gets it? --to ws:<name>"))?;
    let consumer = consumer_for(c, to)?;
    let delivery = match b.delivery.as_deref() {
        Some(d) => request_delivery(d)?,
        None => vault.meta(&sel, &c.actor)?.default_delivery,
    };
    let g = vault.grant(
        GrantRequest {
            selector: sel,
            consumer,
            delivery,
            env_name: b.env_name.clone(),
            file_name: b.file_name.clone(),
            hosts_narrowed: b.hosts.clone(),
            expires_at: b.expires_at.clone(),
            version: b.version,
        },
        &c.actor,
    )?;
    if delivery == Delivery::Api {
        crate::vault::auth_audit("vault-api-grant", &c.actor.stamp(), &format!("ok {}", g.secret_id), "-");
    }
    Ok(json!({ "ok": true, "grant": g }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevokeBody {
    #[serde(flatten)]
    sel: Sel,
    grant_id: Option<String>,
    from: Option<String>,
}

fn revoke(vault: &LocalVault, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    let b: RevokeBody = parse(body)?;
    if let Some(id) = b.grant_id.as_deref() {
        vault.revoke(id, &c.actor)?;
        return Ok(json!({ "ok": true, "revoked": [id] }));
    }
    let sel = selector(c, &b.sel)?;
    let from = b.from.as_deref().ok_or_else(|| err("consumer_required", 400, "--from ws:<name> or a grant id"))?;
    let consumer = consumer_for(c, from)?;
    let mut done = vec![];
    for g in vault.grants(&sel, &c.actor)? {
        if g.consumer == consumer {
            vault.revoke(&g.id, &c.actor)?;
            done.push(g.id);
        }
    }
    if done.is_empty() {
        return Err(VaultError::not_found("no share to remove"));
    }
    Ok(json!({ "ok": true, "revoked": done }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusBody {
    #[serde(flatten)]
    sel: Sel,
    #[serde(default)]
    yes: bool,
}

fn status_change(vault: &LocalVault, c: &VaultCaller, path: &str, body: &[u8]) -> Result<Value, VaultError> {
    let b: StatusBody = parse(body)?;
    let sel = selector(c, &b.sel)?;
    match path {
        DISABLE => vault.disable(&sel, &c.actor)?,
        ENABLE => vault.enable(&sel, &c.actor)?,
        _ => {
            if !b.yes {
                return Err(err("confirm_destroy", 400, "destroying removes every version; add --yes"));
            }
            vault.destroy(&sel, &c.actor)?
        }
    }
    Ok(json!({ "ok": true }))
}

fn get_value(vault: &LocalVault, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    let b: Sel = parse(body)?;
    let a = c.actor.agent().ok_or_else(|| {
        err("use_reveal", 403, "people see a value with reveal (it needs a confirm): k2 vault reveal NAME from a terminal outside K2")
    })?;
    let sel = selector(c, &b)?;
    let who = Requester::Session { workspace_id: a.workspace_id.clone(), session_id: c.session_id.clone() };
    let r = vault.resolve(&sel, Use::Api, &who)?;
    Ok(json!({
        "valueB64": base64::engine::general_purpose::STANDARD.encode(r.value.expose()),
        "version": r.version,
        "note": "this value is now in your transcript; prefer k2 vault fetch",
    }))
}

// ── confirm, reveal, recovery, keys ─────────────────────────────────

fn human_admin(c: &VaultCaller) -> Result<(), VaultError> {
    if c.is_passport() || !c.actor.is_admin_human() {
        return Err(err("owner_only", 403, "this needs the owner or an admin"));
    }
    Ok(())
}

/// Confirm methods this caller can use now.
fn methods(vault: &LocalVault, c: &VaultCaller) -> Vec<&'static str> {
    let mut m = vec![];
    if matches!(c.actor, Actor::Login { .. }) {
        m.push("connect-password");
    }
    if vault.has_confirm_password() {
        m.push("vault-password");
    }
    if cfg!(target_os = "macos") && matches!(c.actor, Actor::Owner) && c.channel != Channel::Login {
        m.push("local-auth");
    }
    m
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfirmBody {
    action: Option<String>,
    secret_id: Option<String>,
    method: Option<String>,
    password: Option<String>,
    /// Shown in the macOS sheet ("reveal STRIPE_KEY").
    label: Option<String>,
}

const CONFIRM_ACTIONS: &[&str] = &[
    "reveal",
    "kek-rotate",
    "rekey",
    "rollback",
    "restore",
    "recovery-setup",
    "recovery-rotate",
    "confirm-password-change",
    "legacy-export",
    // Node admin (prd-k2-node-admin-v1 §7.4): approving a held request;
    // the token is bound to the request id (`secretId`).
    "node-admin",
    // Garden v3 data sources (contract-sources-v1 §4.3): binding a secret
    // to one source (`secretId` = the source id), and one confirm for a
    // batch approval (`secretId` = the batch hash).
    "source-secret",
    "source-batch",
    // Agent hire (prd-agent-hire-templates-v1 HT5/T18): minting an API key
    // that can hire agents, for every caller (owner token included).
    "hire-key-mint",
    // Sew chat passes (S28 §7.4, KM13): an emergency wipe of the chat-pass
    // signing keys needs a person, not the owner token alone.
    "chat-keys-emergency",
];

fn confirm_start(vault: &LocalVault, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    human_admin(c)?;
    let b: ConfirmBody = parse(body)?;
    let action = b.action.as_deref().unwrap_or("");
    if !CONFIRM_ACTIONS.contains(&action) {
        return Err(err("bad_action", 400, "unknown confirm action"));
    }
    let m = methods(vault, c);
    if m.is_empty() {
        return Err(err(
            "no_confirm_method",
            409,
            "set a vault password first (Settings → Vault, or k2 vault confirm-password set from a terminal outside K2)",
        ));
    }
    Ok(json!({ "methods": m, "expiresIn": 60 }))
}

fn confirm_finish(vault: &LocalVault, c: &VaultCaller, body: &[u8], ingress: &str) -> Result<Value, VaultError> {
    human_admin(c)?;
    let b: ConfirmBody = parse(body)?;
    let action = b.action.as_deref().unwrap_or("");
    if !CONFIRM_ACTIONS.contains(&action) {
        return Err(err("bad_action", 400, "unknown confirm action"));
    }
    let stamp = c.actor.stamp();
    let method = b.method.as_deref().unwrap_or("");
    let res: Result<(), VaultError> = match method {
        "vault-password" => vault.check_confirm_password(b.password.as_deref().unwrap_or(""), &stamp),
        "connect-password" => match &c.actor {
            Actor::Login { username, .. } => {
                vault.throttle.check()?;
                match k2_core::connect_users::check_and_record(username, b.password.as_deref().unwrap_or("")) {
                    k2_core::connect_users::LoginOutcome::Ok => {
                        vault.throttle.record(true);
                        Ok(())
                    }
                    k2_core::connect_users::LoginOutcome::LockedOut => Err(err("confirm_locked", 429, "this login is locked for now")),
                    k2_core::connect_users::LoginOutcome::BadCreds => {
                        vault.throttle.record(false);
                        Err(err("confirm_failed", 403, "that isn't your password"))
                    }
                }
            }
            _ => Err(err("bad_method", 400, "connect-password is for a Connect login")),
        },
        "local-auth" => local_auth(&format!(
            "K2 vault: {}{}",
            action.replace('-', " "),
            b.label.as_deref().map(|l| format!(" {}", l.chars().take(64).collect::<String>())).unwrap_or_default()
        )),
        _ => Err(err("bad_method", 400, "method is vault-password, connect-password or local-auth")),
    };
    match res {
        Ok(()) => {
            let t = vault.confirm.mint(action, b.secret_id.as_deref(), &stamp, method);
            crate::vault::auth_audit("vault-confirm", &stamp, &format!("ok {action} via {method}"), ingress);
            Ok(json!({ "token": t, "expiresIn": 60 }))
        }
        Err(e) => {
            crate::vault::auth_audit("vault-confirm", &stamp, &format!("deny {action} via {method}: {}", e.code), ingress);
            Err(e)
        }
    }
}

#[cfg(target_os = "macos")]
fn local_auth(reason: &str) -> Result<(), VaultError> {
    if let Some(f) = test_hooks::local_auth_fake() {
        return f(reason);
    }
    crate::vault::local_auth::confirm(reason).map_err(|h| err("confirm_failed", 403, h))
}

#[cfg(not(target_os = "macos"))]
fn local_auth(reason: &str) -> Result<(), VaultError> {
    if let Some(f) = test_hooks::local_auth_fake() {
        return f(reason);
    }
    Err(err("bad_method", 400, "local-auth is only on a Mac"))
}

/// Test seam: a fake LocalAuthentication (never a real sheet in tests).
pub mod test_hooks {
    use super::VaultError;
    #[cfg(test)]
    use std::sync::Mutex;

    pub type LaFake = fn(&str) -> Result<(), VaultError>;

    #[cfg(test)]
    static FAKE: Mutex<Option<LaFake>> = Mutex::new(None);

    #[cfg(test)]
    pub fn set_local_auth_fake(f: Option<LaFake>) {
        *FAKE.lock().expect("fake lock") = f;
    }

    pub fn local_auth_fake() -> Option<LaFake> {
        #[cfg(test)]
        {
            *FAKE.lock().expect("fake lock")
        }
        #[cfg(not(test))]
        {
            None
        }
    }
}

fn take_proof(vault: &LocalVault, c: &VaultCaller, action: &str, secret: Option<&str>, token: Option<&str>) -> Result<HumanProof, VaultError> {
    let t = token.ok_or_else(|| err("vault_confirm_required", 403, "this needs a confirm first (k2 vault confirm)"))?;
    vault.confirm.take(t, action, secret, &c.actor.stamp())
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevealBody {
    #[serde(flatten)]
    sel: Sel,
    confirm_token: Option<String>,
}

fn reveal(vault: &LocalVault, c: &VaultCaller, body: &[u8], ingress: &str) -> Result<Value, VaultError> {
    human_admin(c)?;
    let b: RevealBody = parse(body)?;
    let sel = selector(c, &b.sel)?;
    let m = vault.meta(&sel, &c.actor)?;
    let stamp = c.actor.stamp();
    let proof = match take_proof(vault, c, "reveal", Some(&m.id), b.confirm_token.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            crate::vault::auth_audit("vault-reveal", &stamp, &format!("deny {}: {}", m.name, e.code), ingress);
            return Err(e);
        }
    };
    let v = vault.reveal(&Selector::Id(m.id.clone()), &c.actor, &proof);
    match v {
        Ok(v) => {
            crate::vault::auth_audit("vault-reveal", &stamp, &format!("ok {}", m.name), ingress);
            Ok(json!({ "valueB64": base64::engine::general_purpose::STANDARD.encode(v.expose()), "version": m.latest_version }))
        }
        Err(e) => {
            crate::vault::auth_audit("vault-reveal", &stamp, &format!("deny {}: {}", m.name, e.code), ingress);
            Err(e)
        }
    }
}

/// VS2: recovery and the confirm password are for people only.
fn require_person(c: &VaultCaller, not_person: Option<&'static str>) -> Result<(), VaultError> {
    if c.is_passport() {
        return Err(err("owner_only", 403, "agents can't do this; tell your owner"));
    }
    if c.channel.is_person() {
        return Ok(());
    }
    let why = not_person.unwrap_or("it came from a K2 token, not from a person");
    Err(err(
        "person_required",
        403,
        format!(
            "only a person can do this ({why}). Run it from your own SSH login or a terminal outside K2, or sign in to K2 Connect. Agents: don't run it; tell your owner."
        ),
    ))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryBody {
    id: Option<String>,
    #[serde(default)]
    words: Vec<String>,
    confirm_token: Option<String>,
}

/// The K2 app: the owner token on its own, no person channel (VS1 is
/// deferred, so the app has no attested socket yet). Not the vault socket:
/// a peer there that failed the person check stays refused.
fn is_app_owner(c: &VaultCaller, not_person: Option<&'static str>) -> bool {
    not_person.is_none() && !c.is_passport() && matches!(c.actor, Actor::Owner) && c.channel == Channel::Plain
}

fn recovery_show(vault: &LocalVault, c: &VaultCaller, path: &str, body: &[u8], ingress: &str, not_person: Option<&'static str>) -> Result<Value, VaultError> {
    let stamp = c.actor.stamp();
    let b: RecoveryBody = parse(body)?;
    let (state, _, _) = vault.recovery_state();
    let confirmed = state == "confirmed";
    let app_confirmed = match require_person(c, not_person) {
        Ok(()) => {
            human_admin(c)?;
            false
        }
        // Settings → Vault (0.45.4): the owner token alone isn't a person,
        // but a person's confirm is: the Mac's Touch ID / login password
        // sheet (LocalAuthentication) or the vault password, which only a
        // person can set. The single-use token for this exact action stands
        // in for the person channel. Agents (passports) never get here.
        Err(e) if is_app_owner(c, not_person) => {
            let action = if confirmed { "recovery-rotate" } else { "recovery-setup" };
            let Some(token) = b.confirm_token.as_deref() else {
                crate::vault::auth_audit("vault-recovery", &stamp, &format!("deny show: {}", e.code), ingress);
                return Err(err(
                    "person_required",
                    403,
                    "only a person can do this. In the K2 app use Settings → Vault → Set up recovery key (it asks you to confirm), or run k2 vault recovery show-once from your own SSH login or a terminal outside K2. Agents: don't run it; tell your owner.",
                ));
            };
            if confirmed && path != RECOVERY_ROTATE {
                return Err(err(
                    "recovery_exists",
                    409,
                    "a recovery key is already confirmed; to replace it run k2 vault recovery rotate (needs a confirm)",
                ));
            }
            if let Err(e) = take_proof(vault, c, action, None, Some(token)) {
                crate::vault::auth_audit("vault-recovery", &stamp, &format!("deny show: {}", e.code), ingress);
                return Err(e);
            }
            true
        }
        Err(e) => {
            crate::vault::auth_audit("vault-recovery", &stamp, &format!("deny show: {}", e.code), ingress);
            return Err(e);
        }
    };
    if confirmed && !app_confirmed {
        // A new key replaces the old one: confirm first (§12, VS2).
        if path != RECOVERY_ROTATE {
            return Err(err(
                "recovery_exists",
                409,
                "a recovery key is already confirmed; to replace it run k2 vault recovery rotate (needs a confirm)",
            ));
        }
        take_proof(vault, c, "recovery-rotate", None, b.confirm_token.as_deref())?;
    }
    let ch = vault.recovery_begin(&c.actor)?;
    crate::vault::auth_audit("vault-recovery", &stamp, "ok shown once", ingress);
    Ok(json!({
        "id": ch.id,
        "phrase": ch.phrase.expose_str().unwrap_or_default(),
        "checkWords": ch.check_words,
        "recipient": ch.recipient,
        "note": "Write these 24 words down and keep them offline. K2 keeps only the public half. Then type back words 3, 11 and 20.",
    }))
}

fn recovery_confirm(vault: &LocalVault, c: &VaultCaller, body: &[u8], ingress: &str, not_person: Option<&'static str>) -> Result<Value, VaultError> {
    let stamp = c.actor.stamp();
    // The app (owner token) may finish what it started: the show-once id
    // is random, held 15 minutes, bound to the same actor, and only handed
    // out after a person check or a confirm; the words were only on screen.
    if let Err(e) = require_person(c, not_person) {
        if !is_app_owner(c, not_person) {
            return Err(e);
        }
    }
    human_admin(c)?;
    let b: RecoveryBody = parse(body)?;
    let id = b.id.as_deref().ok_or_else(|| err("id_required", 400, "which show-once?"))?;
    match vault.recovery_confirm(id, &b.words, &c.actor) {
        Ok(r) => {
            crate::vault::auth_audit("vault-recovery", &stamp, "ok confirmed", ingress);
            Ok(json!({ "ok": true, "recovery": "confirmed", "recipient": r }))
        }
        Err(e) => {
            crate::vault::auth_audit("vault-recovery", &stamp, &format!("deny confirm: {}", e.code), ingress);
            Err(e)
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfirmPasswordBody {
    password: Option<String>,
    confirm_token: Option<String>,
}

fn confirm_password_set(vault: &LocalVault, c: &VaultCaller, body: &[u8], ingress: &str, not_person: Option<&'static str>) -> Result<Value, VaultError> {
    let stamp = c.actor.stamp();
    if let Err(e) = require_person(c, not_person) {
        crate::vault::auth_audit("vault-confirm-password", &stamp, &format!("deny: {}", e.code), ingress);
        return Err(e);
    }
    human_admin(c)?;
    let b: ConfirmPasswordBody = parse(body)?;
    if vault.has_confirm_password() {
        // Changing it: the old password (or a Connect owner login's
        // password) first. Reset otherwise needs the recovery key (VS16).
        take_proof(vault, c, "confirm-password-change", None, b.confirm_token.as_deref())?;
    }
    let pw = b.password.as_deref().ok_or_else(|| err("password_required", 400, "type the new vault password"))?;
    vault.set_confirm_password(pw, &c.actor)?;
    crate::vault::auth_audit("vault-confirm-password", &stamp, "ok set", ingress);
    Ok(json!({ "ok": true, "note": "K2 only asks for your vault password inside the K2 window or your own terminal." }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyOpBody {
    confirm_token: Option<String>,
}

fn key_op(vault: &LocalVault, c: &VaultCaller, path: &str, body: &[u8], ingress: &str) -> Result<Value, VaultError> {
    human_admin(c)?;
    if !matches!(c.actor, Actor::Owner | Actor::Login { role: k2_vault::Role::Owner, .. }) {
        return Err(err("owner_only", 403, "key operations are for the owner"));
    }
    let b: KeyOpBody = parse(body)?;
    let action = if path == KEK_ROTATE { "kek-rotate" } else { "rekey" };
    let stamp = c.actor.stamp();
    let proof = take_proof(vault, c, action, None, b.confirm_token.as_deref())?;
    let r = if path == KEK_ROTATE { vault.kek_rotate(&c.actor, &proof) } else { vault.rekey(&c.actor, &proof) };
    match r {
        Ok(()) => {
            crate::vault::auth_audit("vault-key", &stamp, &format!("ok {action}"), ingress);
            Ok(json!({ "ok": true }))
        }
        Err(e) => {
            crate::vault::auth_audit("vault-key", &stamp, &format!("deny {action}: {}", e.code), ingress);
            Err(e)
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RollbackBody {
    #[serde(flatten)]
    sel: Sel,
    version: Option<u32>,
    confirm_token: Option<String>,
}

fn rollback(vault: &LocalVault, c: &VaultCaller, body: &[u8], ingress: &str) -> Result<Value, VaultError> {
    human_admin(c)?;
    let b: RollbackBody = parse(body)?;
    let sel = selector(c, &b.sel)?;
    let m = vault.meta(&sel, &c.actor)?;
    let v = b.version.ok_or_else(|| err("version_required", 400, "--to <version>"))?;
    take_proof(vault, c, "rollback", Some(&m.id), b.confirm_token.as_deref())?;
    let m2 = vault.rollback_to(&Selector::Id(m.id.clone()), v, &c.actor)?;
    crate::vault::auth_audit("vault-rollback", &c.actor.stamp(), &format!("ok {} to v{v}", m.name), ingress);
    Ok(json!({ "ok": true, "secret": meta_json(&m2) }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoreBody {
    #[serde(default)]
    acknowledge_rollback: bool,
    confirm_token: Option<String>,
    /// Restore from a backup folder holding `vault.db`.
    from_dir: Option<String>,
    /// `recovery` (24 words) or `backup` (`AGE-SECRET-KEY-1…`).
    holder: Option<String>,
    phrase: Option<String>,
    identity: Option<String>,
    #[serde(default)]
    dry_run: bool,
    /// Replace a vault that already holds secrets (needs a confirm).
    #[serde(default)]
    replace: bool,
}

/// Restore on a new box (§13.2): person only (VS2), checked before
/// anything is overwritten.
fn restore_from(vault: &LocalVault, c: &VaultCaller, b: &RestoreBody, ingress: &str, not_person: Option<&'static str>) -> Result<Value, VaultError> {
    require_person(c, not_person)?;
    let holder = b.holder.as_deref().unwrap_or("recovery");
    let identity = match holder {
        "recovery" => k2_vault::recovery::RecoverySeed::from_phrase(b.phrase.as_deref().unwrap_or(""))?.identity(),
        "backup" => k2_vault::agefmt::Identity::parse(b.identity.as_deref().unwrap_or(""))?,
        _ => return Err(err("bad_holder", 400, "holder is recovery or backup")),
    };
    let from = std::path::PathBuf::from(b.from_dir.clone().unwrap_or_default());
    let (vid, n) = LocalVault::check_identity(&from, &identity, holder)?;
    if b.dry_run {
        return Ok(json!({ "ok": true, "dryRun": true, "vaultId": vid, "secrets": n }));
    }
    if vault.user_secret_count() > 0 {
        if !b.replace {
            return Err(err("vault_not_empty", 409, "this box's vault already holds secrets; restoring replaces it (add --replace; it needs a confirm)"));
        }
        take_proof(vault, c, "restore", None, b.confirm_token.as_deref())?;
    }
    let dir = vault.dir().to_path_buf();
    let ks = vault.key_store().clone();
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
    let aside = dir.join(format!("replaced-{stamp}"));
    k2_vault::fsutil::ensure_private_dir(&aside)?;
    for f in ["vault.db", "vault.db-wal", "vault.db-shm"] {
        let p = dir.join(f);
        if p.exists() {
            std::fs::rename(&p, aside.join(f)).map_err(|e| err("restore_failed", 500, format!("move aside: {}", e.kind())))?;
        }
    }
    let put_back = || {
        for f in ["vault.db", "vault.db-wal", "vault.db-shm"] {
            let _ = std::fs::rename(aside.join(f), dir.join(f));
        }
    };
    if let Err(e) = std::fs::copy(from.join("vault.db"), dir.join("vault.db")) {
        put_back();
        return Err(err("restore_failed", 500, format!("copy: {}", e.kind())));
    }
    match LocalVault::adopt_with_identity(&dir, ks, &identity, holder, k2_vault::OpenOptions::default()) {
        Ok(v) => {
            crate::vault::replace(std::sync::Arc::new(v));
            crate::vault::auth_audit("vault-restore", &c.actor.stamp(), &format!("ok from {holder} ({n} secrets)"), ingress);
            Ok(json!({ "ok": true, "vaultId": vid, "secrets": n, "previous": aside.display().to_string() }))
        }
        Err(e) => {
            let _ = std::fs::remove_file(dir.join("vault.db"));
            put_back();
            Err(e)
        }
    }
}

fn restore(vault: &LocalVault, c: &VaultCaller, body: &[u8], ingress: &str, not_person: Option<&'static str>) -> Result<Value, VaultError> {
    human_admin(c)?;
    let b: RestoreBody = parse(body)?;
    if b.from_dir.is_some() {
        return restore_from(vault, c, &b, ingress, not_person);
    }
    if !b.acknowledge_rollback {
        return Err(err(
            "restore_from_backup",
            400,
            "to restore a vault from a backup: k2 vault restore --from <dir> (with the recovery phrase), from a terminal outside K2",
        ));
    }
    let proof = take_proof(vault, c, "restore", None, b.confirm_token.as_deref())?;
    vault.acknowledge_rollback(&c.actor, &proof)?;
    crate::vault::auth_audit("vault-restore", &c.actor.stamp(), "ok rollback acknowledged", ingress);
    Ok(json!({ "ok": true }))
}

#[cfg(test)]
mod tests;
