//! `/cli/vault/*` routes that USE a secret on the caller's behalf:
//! `fetch` (V4), `env-file` and `import` (V8). Split from
//! `vault_routes.rs` so delivery lands without touching management.
//!
//! `fetch` (§9.2 as amended by VS3–VS6, KV17): the agent names the secret;
//! K2 checks the URL against the secret's sites, puts the value only where
//! the secret's creator said (sealed injection), makes the call through
//! the egress gate, and returns the response with every echo of the value
//! replaced by `[k2v:NAME]`. Request bodies over 64 KiB, and `-o` output,
//! go through files inside the caller's own workspace.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use k2_vault::fetch::MAX_INLINE_BODY;
use k2_vault::hosts::{Origin, OriginPattern};
use k2_vault::{LocalVault, Requester, SecretStore, Use, VaultError};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::vault::actor::VaultCaller;
use crate::vault_routes::{refuse, selector, Sel};

pub const FETCH: &str = "/cli/vault/fetch";
pub const ENV_FILE: &str = "/cli/vault/env-file";
pub const IMPORT: &str = "/cli/vault/import";
/// GET: a published service's env mode and bindings; POST: change them.
pub const SERVICE: &str = "/cli/vault/service";
/// This vault's public carry recipient (clones/migrations, §13.3).
pub const RECIPIENT: &str = "/cli/vault/recipient";
pub const CARRY_EXPORT: &str = "/cli/vault/carry/export";
pub const CARRY_IMPORT: &str = "/cli/vault/carry/import";
/// A ciphertext snapshot of vault.db for a backup (§13.1).
pub const BACKUP: &str = "/cli/vault/backup";
/// Wrap the data key to a backup's age recipient.
pub const BACKUP_RECIPIENT: &str = "/cli/vault/backup/recipient";

/// Routes this module serves.
pub const ROUTES: &[&str] = &[FETCH, ENV_FILE, IMPORT, SERVICE, RECIPIENT, CARRY_EXPORT, CARRY_IMPORT, BACKUP, BACKUP_RECIPIENT];
/// POST-only routes among them (`service` serves both).
pub const POST_ROUTES: &[&str] = &[FETCH, ENV_FILE, IMPORT, CARRY_EXPORT, CARRY_IMPORT, BACKUP, BACKUP_RECIPIENT];

pub fn is_route(path: &str) -> bool {
    ROUTES.contains(&path)
}

pub fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path)
}

pub fn handle(
    vault: &Arc<LocalVault>,
    path: &str,
    c: &VaultCaller,
    body: &[u8],
    params: &HashMap<String, String>,
    _ingress: &str,
) -> CliResponse {
    let r = match path {
        FETCH => fetch(vault, c, body),
        ENV_FILE => env_file(vault, c, body),
        IMPORT => import(vault, c, body),
        SERVICE if body.is_empty() => service_get(vault, c, params),
        SERVICE => service_set(c, body),
        RECIPIENT => recipient(vault, c),
        CARRY_EXPORT => carry_export(vault, c, body),
        CARRY_IMPORT => carry_import(vault, c, body),
        BACKUP => backup(vault, c, body),
        BACKUP_RECIPIENT => backup_recipient(vault, c, body),
        _ => Err(VaultError::not_found("no such vault route")),
    };
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => refuse(&e),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchSecret {
    #[serde(flatten)]
    sel: Sel,
    /// Pick from the secret's sealed `allowedDst` (`header:X`, `query:k`).
    dst: Option<String>,
    fmt: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchBody {
    #[serde(default)]
    secrets: Vec<FetchSecret>,
    /// Shorthand for one secret by name.
    secret: Option<String>,
    method: Option<String>,
    url: Option<String>,
    #[serde(default)]
    headers: Vec<(String, String)>,
    body_b64: Option<String>,
    /// A file inside the caller's workspace (bodies over 64 KiB).
    body_file: Option<String>,
    #[serde(default)]
    max_bytes: usize,
    #[serde(default)]
    timeout_secs: u64,
    #[serde(default)]
    include_headers: bool,
    /// Write the (redacted) body to this workspace file instead.
    out: Option<String>,
}

fn rate() -> &'static crate::vault_fetch::RateLimiter {
    static R: OnceLock<crate::vault_fetch::RateLimiter> = OnceLock::new();
    R.get_or_init(crate::vault_fetch::RateLimiter::for_fetch)
}

/// A path inside the workspace folder (no `..` escape, no symlink escape
/// for the parent). Returns the full path.
pub fn jailed_path(workspace_root: &Path, rel: &str) -> Result<PathBuf, VaultError> {
    let bad = || VaultError::bad_request("path_outside_workspace", "use a path inside your workspace");
    let rel = rel.trim();
    if rel.is_empty() || rel.contains('\0') {
        return Err(bad());
    }
    let p = Path::new(rel);
    let joined = if p.is_absolute() { p.to_path_buf() } else { workspace_root.join(p) };
    let root = std::fs::canonicalize(workspace_root).map_err(|_| bad())?;
    let parent = joined.parent().ok_or_else(bad)?;
    let parent = std::fs::canonicalize(parent).map_err(|_| bad())?;
    if !parent.starts_with(&root) {
        return Err(bad());
    }
    let name = joined.file_name().ok_or_else(bad)?;
    if name == ".." || name == "." {
        return Err(bad());
    }
    Ok(parent.join(name))
}

fn fetch(vault: &Arc<LocalVault>, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    let a = c.actor.agent().ok_or_else(|| {
        VaultError::forbidden("use_fetch_as_agent", "k2 vault fetch runs for an agent session; people call the site themselves")
    })?;
    let b: FetchBody = serde_json::from_slice(body).map_err(|_| VaultError::bad_request("bad_json", "bad request body"))?;
    let key = format!("{}/{}", a.workspace_id, c.session_id.clone().unwrap_or_default());
    rate().check(&key)?;
    let url = b.url.clone().ok_or_else(|| VaultError::bad_request("url_required", "give the URL to call"))?;
    let (_, origin): (url::Url, Origin) = Origin::parse_url(&url)?;
    let mut wanted = b.secrets;
    if let Some(n) = b.secret.as_deref().filter(|n| !n.is_empty()) {
        wanted.push(FetchSecret { sel: Sel { name: Some(n.to_string()), ..Default::default() }, dst: None, fmt: None });
    }
    if wanted.is_empty() {
        return Err(VaultError::bad_request("secret_required", "name the secret: --secret NAME"));
    }
    let who = c.requester()?;
    let mut injections = vec![];
    for w in &wanted {
        let sel = selector(c, &w.sel)?;
        let r = vault.resolve(&sel, Use::Fetch(&origin), &who)?;
        let (dst, fmt) = r.inject.choose(w.dst.as_deref(), w.fmt.as_deref())?;
        let allowed: Vec<OriginPattern> = k2_vault::hosts::parse_stored(&r.meta.hosts);
        injections.push(crate::vault_fetch::Injection { name: r.meta.name.clone(), value: r.value, dst, fmt, allowed, sign: r.inject.sign.clone() });
    }
    let ws_root = crate::vault::workspace_path(&a.workspace_id).map(PathBuf::from);
    let req_body = match (&b.body_b64, &b.body_file) {
        (Some(_), Some(_)) => return Err(VaultError::bad_request("bad_body", "send the body inline or as a file, not both")),
        (Some(s), None) => {
            let v = base64::engine::general_purpose::STANDARD
                .decode(s.trim())
                .map_err(|_| VaultError::bad_request("bad_body", "bodyB64 isn't base64"))?;
            if v.len() > MAX_INLINE_BODY {
                return Err(VaultError::bad_request("body_too_large", "bodies over 64 KiB go as a file (-d @file)"));
            }
            Some(v)
        }
        (None, Some(f)) => {
            let root = ws_root.as_ref().ok_or_else(|| VaultError::not_found("workspace folder not found"))?;
            let p = jailed_path(root, f)?;
            let meta = std::fs::metadata(&p).map_err(|_| VaultError::not_found("body file not found"))?;
            if meta.len() > 10 * 1024 * 1024 {
                return Err(VaultError::bad_request("body_too_large", "request bodies are at most 10 MiB"));
            }
            Some(std::fs::read(&p).map_err(|_| VaultError::not_found("body file not readable"))?)
        }
        (None, None) => None,
    };
    let spec = crate::vault_fetch::FetchSpec {
        method: b.method.clone().unwrap_or_else(|| if req_body.is_some() { "POST".into() } else { "GET".into() }),
        url,
        headers: b.headers.clone(),
        body: req_body,
        max_bytes: b.max_bytes,
        timeout: Duration::from_secs(b.timeout_secs),
        include_headers: b.include_headers,
        purpose: crate::egress::Purpose::VaultFetch,
    };
    let redactor = vault.redactor();
    let allow = crate::vault_fetch::Allowlist::from_injections(&injections);
    let res = crate::vault_fetch::execute(spec, &allow, injections, &redactor)?;
    let headers: Vec<Value> = res.headers.iter().map(|(k, v)| json!([k, v])).collect();
    if let Some(out) = b.out.as_deref() {
        let root = ws_root.as_ref().ok_or_else(|| VaultError::not_found("workspace folder not found"))?;
        let p = jailed_path(root, out)?;
        write_no_follow(&p, &res.body)?;
        let mut v = json!({
            "status": res.status,
            "headers": headers,
            "truncated": res.truncated,
            "redirected": res.redirected,
            "out": p.display().to_string(),
            "bytes": res.body.len(),
        });
        if let Some(c) = res.clock {
            v["clock"] = json!(c);
        }
        return Ok(v);
    }
    let (text, b64) = match String::from_utf8(res.body.clone()) {
        Ok(t) => (Some(t), None),
        Err(_) => (None, Some(base64::engine::general_purpose::STANDARD.encode(&res.body))),
    };
    let mut v = json!({
        "status": res.status,
        "headers": headers,
        "truncated": res.truncated,
        "redirected": res.redirected,
        "body": text,
        "bodyB64": b64,
    });
    // OVH signing: whose clock signed it (server = /1.0/auth/time).
    if let Some(c) = res.clock {
        v["clock"] = json!(c);
    }
    Ok(v)
}

fn write_no_follow(p: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(p)
        .map_err(|e| VaultError::bad_request("out_not_writable", format!("can't write the output file ({})", e.kind())))?;
    f.write_all(bytes).map_err(|e| VaultError::internal(format!("write: {}", e.kind())))
}

// ── env-file (§10.6) ───────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnvFileBody {
    #[serde(default)]
    secrets: Vec<String>,
    ttl_secs: Option<u64>,
    /// Remove a file made earlier.
    done: Option<String>,
}

fn env_files_dir() -> Result<PathBuf, VaultError> {
    let root = crate::vault::spawn::secrets_root()
        .ok_or_else(|| VaultError::new("no_memory_fs", 409, "this machine has no memory-backed folder for secret files"))?;
    let d = root.join("env-files");
    k2_vault::fsutil::ensure_private_dir(&root)?;
    k2_vault::fsutil::ensure_private_dir(&d)?;
    Ok(d)
}

/// A 0600 env file for `docker run --env-file` / `podman run --env-file`,
/// removed after `ttlSecs` (default 60) or on `done`.
fn env_file(vault: &Arc<LocalVault>, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    c.actor.agent().ok_or_else(|| VaultError::forbidden("use_env_file_as_agent", "env-file runs for an agent session"))?;
    let b: EnvFileBody = serde_json::from_slice(body).map_err(|_| VaultError::bad_request("bad_json", "bad request body"))?;
    let dir = env_files_dir()?;
    if let Some(done) = b.done.as_deref() {
        let p = Path::new(done);
        if p.parent() != Some(dir.as_path()) {
            return Err(VaultError::bad_request("not_an_env_file", "that isn't an env file K2 made"));
        }
        let _ = crate::vault::adopt::remove_old_copy(p);
        return Ok(json!({ "ok": true, "removed": done }));
    }
    if b.secrets.is_empty() {
        return Err(VaultError::bad_request("secret_required", "name the secrets: --secret NAME"));
    }
    let who = c.requester()?;
    let mut lines = String::new();
    for n in &b.secrets {
        let sel = selector(c, &Sel { name: Some(n.clone()), ..Default::default() })?;
        let r = vault.resolve(&sel, Use::Env, &who)?;
        k2_vault::delivery::validate_env_file_value(r.value.expose())?;
        let name = r.env_name.clone().unwrap_or_else(|| k2_vault::delivery::default_env_name(&r.meta.name));
        k2_vault::delivery::validate_env_name(&name)?;
        lines.push_str(&name);
        lines.push('=');
        lines.push_str(std::str::from_utf8(r.value.expose()).map_err(|_| VaultError::bad_request("not_text", "env values must be text"))?);
        lines.push('\n');
    }
    let p = dir.join(format!("{}.env", k2_vault::crypto::base62(16)));
    crate::vault::spawn::write_secret_file(&dir, p.file_name().and_then(|f| f.to_str()).unwrap_or("x.env"), lines.as_bytes())
        .map_err(|e| VaultError::internal(format!("env file: {}", e.kind())))?;
    let ttl = b.ttl_secs.unwrap_or(60).clamp(5, 3600);
    let p2 = p.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(ttl));
        let _ = crate::vault::adopt::remove_old_copy(&p2);
    });
    Ok(json!({ "ok": true, "path": p.display().to_string(), "ttlSecs": ttl }))
}

// ── import (§10.5) ─────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportBody {
    path: Option<String>,
    scope: Option<String>,
    workspace: Option<String>,
    delivery: Option<String>,
    #[serde(default)]
    remove_original: bool,
}

/// Move each sensitive line of a `.env` into the vault and write `.env.k2`
/// beside it (references + the non-secret lines). Never deletes the
/// original unless asked (Q11).
fn import(vault: &Arc<LocalVault>, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    let b: ImportBody = serde_json::from_slice(body).map_err(|_| VaultError::bad_request("bad_json", "bad request body"))?;
    let rel = b.path.clone().ok_or_else(|| VaultError::bad_request("path_required", "which .env?"))?;
    let ws_id = match (c.workspace_id(), b.workspace.as_deref()) {
        (Some(own), _) => own.to_string(),
        (None, Some(w)) => crate::vault::workspace_id(w).ok_or_else(|| VaultError::not_found(format!("no workspace named {w}")))?,
        (None, None) => return Err(VaultError::bad_request("workspace_required", "name the workspace the .env belongs to")),
    };
    let root = crate::vault::workspace_path(&ws_id).map(PathBuf::from).ok_or_else(|| VaultError::not_found("workspace folder not found"))?;
    let p = jailed_path(&root, &rel)?;
    let text = std::fs::read_to_string(&p).map_err(|_| VaultError::not_found("that .env isn't readable"))?;
    let scope = crate::vault_routes::scope_for(c, b.scope.as_deref().or(Some("workspace")), Some(&ws_id), None)?;
    let delivery = b.delivery.as_deref().map(crate::vault_routes::request_delivery).transpose()?;
    let mut out_lines = vec!["# Made by k2 vault import: references only, safe to commit.".to_string()];
    let mut moved = vec![];
    let mut kept = vec![];
    for line in text.lines() {
        let t = line.trim();
        let Some((k, v)) = t.trim_start_matches("export ").split_once('=') else {
            if !t.is_empty() {
                out_lines.push(line.to_string());
            }
            continue;
        };
        let k = k.trim();
        let v = v.trim().trim_matches('"').trim_matches('\'');
        // The doctor's name rule, so `k2 vault import` moves every name
        // `k2 vault doctor` tells you to import with it.
        if t.starts_with('#') || v.is_empty() || k2_vault::refs::VaultRef::is_ref(v) || !crate::vault_routes::doctor_flags_name(k) {
            out_lines.push(line.to_string());
            if !t.starts_with('#') && !v.is_empty() {
                kept.push(k.to_string());
            }
            continue;
        }
        let kind = if k == "DATABASE_URL" || k.ends_with("_DSN") { k2_vault::Kind::Dsn } else { k2_vault::Kind::ApiKey };
        let mut req = k2_vault::PutRequest::new(k2_vault::Namespace::User, scope.clone(), k, kind);
        req.default_delivery = Some(delivery.unwrap_or(k2_vault::Delivery::Env));
        req.delivery_ceiling = Some(k2_vault::Delivery::Env);
        req.origin = format!("import:{rel}");
        req.mode = k2_vault::PutMode::Upsert;
        req.idempotent = true;
        let o = vault.put(req, k2_vault::SecretBytes::from_slice(v.as_bytes()), &c.actor)?;
        let r = crate::vault::display_ref(&o.meta).unwrap_or_default();
        out_lines.push(format!("{k}={r}"));
        moved.push(k.to_string());
    }
    let envk2 = p.with_file_name(".env.k2");
    let mut body_out = out_lines.join("\n");
    body_out.push('\n');
    std::fs::write(&envk2, body_out).map_err(|e| VaultError::internal(format!("write .env.k2: {}", e.kind())))?;
    if b.remove_original && !moved.is_empty() {
        crate::vault::adopt::remove_old_copy(&p).map_err(|e| VaultError::internal(format!("remove: {}", e.kind())))?;
    }
    Ok(json!({ "ok": true, "moved": moved, "kept": kept, "envK2": envk2.display().to_string(), "originalRemoved": b.remove_original && !moved.is_empty() }))
}

// ── published service env (§10.5) ─────────────────────────────────

fn service_get(vault: &Arc<LocalVault>, c: &VaultCaller, params: &HashMap<String, String>) -> Result<Value, VaultError> {
    let label = params.get("service").cloned().unwrap_or_default();
    let id = published_service_id(c, &label)?;
    let se = crate::vault::publish_env::get(&id);
    let grants: Vec<Value> = vault
        .grants_to(&k2_vault::Consumer::published_service(id.clone()))?
        .into_iter()
        .map(|(g, name)| json!({ "name": name, "delivery": g.delivery, "envName": g.env_name, "fileName": g.file_name, "grantId": g.id }))
        .collect();
    Ok(json!({ "service": label, "id": id, "mode": se.mode, "envFile": se.env_file, "inheritedNames": se.inherited_names, "secrets": grants }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServiceBody {
    service: Option<String>,
    mode: Option<String>,
    env_file: Option<String>,
}

fn service_set(c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    let b: ServiceBody = serde_json::from_slice(body).map_err(|_| VaultError::bad_request("bad_json", "bad request body"))?;
    let label = b.service.clone().ok_or_else(|| VaultError::bad_request("service_required", "which service?"))?;
    let id = published_service_id(c, &label)?;
    // Inheriting the daemon's whole environment is a widening: people only.
    if b.mode.as_deref() == Some("legacy") && !c.actor.is_admin_human() {
        return Err(VaultError::owner_confirm_required("only the owner can give a service the daemon's environment"));
    }
    if c.actor.is_member() {
        return Err(VaultError::forbidden("vault_admin_required", "changing a service's environment needs an Owner or Admin"));
    }
    let se = crate::vault::publish_env::set(&id, b.mode.as_deref(), b.env_file.clone().map(Some)).map_err(|e| VaultError::bad_request("bad_mode", e))?;
    Ok(json!({ "ok": true, "service": label, "mode": se.mode, "envFile": se.env_file, "restart": "restart the service to apply" }))
}

// ── carries and backups (§13, KV31, VS2, VS20) ─────────────────────

/// Migrations: the owner, an Admin, or an IT agent ("Allow managing
/// server-wide secrets") may run the carry (Rosson on VS2). The recovery
/// key is never involved.
fn may_carry(c: &VaultCaller) -> Result<(), VaultError> {
    match c.actor.agent() {
        Some(a) if a.vault_manage => Ok(()),
        Some(_) => Err(VaultError::owner_confirm_required("moving secrets between boxes needs \"Allow managing server-wide secrets\"")),
        None if c.actor.is_admin_human() => Ok(()),
        None => Err(VaultError::forbidden("vault_admin_required", "this needs an Owner or Admin")),
    }
}

/// H21 (Rosson 2026-10-16): an agent with "Allow managing server-wide
/// secrets" may carry with no person, so every export is made visible,
/// never blocked: the vault's own audit (one row per secret plus a summary,
/// tamper-evident; `k2 vault audit`), the auth audit (Settings → Logs →
/// Access Audit, which highlights an agent's export), and, for an agent,
/// an owner notice on the Tickets board ([`agent_export_notice`]). Names
/// and fingerprints only, never values.
fn record_carry_export(
    vault: &Arc<LocalVault>,
    c: &VaultCaller,
    target_fp: &str,
    target_vault_id: &str,
    workspaces: &[String],
    secrets: &[(String, u32)],
    names: &[String],
) {
    let peer = format!("carry:{target_fp}@{target_vault_id}");
    vault.audit_carry("carry-export", &c.actor, &peer, secrets);
    let who = c.actor.stamp();
    let ws = c.workspace_id().unwrap_or("-");
    crate::vault::auth_audit(
        "vault-carry",
        &who,
        &format!(
            "ok export {} [{}] of workspaces [{}] by {who} (workspace {ws}) to {target_fp} (vault {target_vault_id})",
            names.len(),
            names.join(", "),
            workspaces.join(", ")
        ),
        "-",
    );
    if let Some(a) = c.actor.agent() {
        let label = crate::vault::workspace_label(&a.workspace_id);
        let (title, body) = agent_export_notice(&label, c.session_id.as_deref(), names, workspaces, target_fp, target_vault_id);
        if let Err(e) = crate::feedback_routes::file_owner_notice(&a.workspace_id, &label, &title, &body) {
            // The audit rows above stand; the notice is best effort.
            k2_core::log_debug!("[vault] carry export notice for {label}: {e}");
        }
        k2_core::agent_hooks::emit(
            k2_core::agent_hooks::HookEvent::SyncProjects,
            json!({ "vaultNotice": {
                "kind": "carry-export",
                "by": who,
                "workspace": ws,
                "session": c.session_id,
                "count": names.len(),
                "secrets": names,
                "workspaces": workspaces,
                "targetFingerprint": target_fp,
                "targetVaultId": target_vault_id,
                "at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            } }),
        );
    }
}

/// The owner notice for an agent's carry export: who, which secrets (names
/// only), where to, and what to do if it wasn't expected.
pub(crate) fn agent_export_notice(
    agent: &str,
    session: Option<&str>,
    names: &[String],
    workspaces: &[String],
    target_fp: &str,
    target_vault_id: &str,
) -> (String, String) {
    let n = names.len();
    let title = format!("{agent} exported {n} secret{} to another vault", if n == 1 { "" } else { "s" });
    let list = if names.is_empty() { "- (none)".to_string() } else { names.iter().map(|x| format!("- {x}")).collect::<Vec<_>>().join("\n") };
    let from = if workspaces.is_empty() { "server secrets only".to_string() } else { workspaces.join(", ") };
    let body = format!(
        "Agent {agent}{} ran `k2 vault carry export`. It may, because \"Allow managing server-wide secrets\" is on for it.\n\n\
         Secrets (names only, never values):\n{list}\n\n\
         From: {from}\n\
         To: vault {target_vault_id} (key {target_fp})\n\n\
         It is also in Settings → Logs → Access Audit and in `k2 vault audit`. \
         Didn't expect it? Turn that switch off for {agent} (Settings → Workspaces → {agent} → Agent) and rotate these secrets.",
        session.map(|s| format!(" (session {s})")).unwrap_or_default(),
    );
    (title, body)
}

fn recipient(vault: &Arc<LocalVault>, c: &VaultCaller) -> Result<Value, VaultError> {
    may_carry(c)?;
    let r = vault.carry_recipient()?;
    Ok(json!({ "recipient": r.encode(), "fingerprint": r.fingerprint(), "vaultId": vault.vault_id() }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CarryExportBody {
    target_recipient: Option<String>,
    target_vault_id: Option<String>,
    #[serde(default)]
    workspaces: Vec<String>,
    /// Server-scope secrets to carry, by name (only when ticked, §13.3).
    #[serde(default)]
    server: Vec<String>,
    #[serde(default)]
    own_box_move: bool,
    /// The workspace id on the target box these secrets are for (CL12).
    /// Needs exactly one source workspace.
    target_workspace_id: Option<String>,
}

/// A sealed carry: the age file plus `(scope label, name)` for each secret
/// in it (never values). The label is empty for a server-scope secret.
pub(crate) struct SealedCarry {
    pub file: Vec<u8>,
    pub names: Vec<(String, String)>,
    pub target_fingerprint: String,
    /// Named secrets left out because their kind never travels (CL13).
    pub never: Vec<String>,
    /// (secret id, version) of each sealed secret, for the audit (H21).
    pub ids: Vec<(String, u32)>,
}

/// Seal `user` secrets to a target vault: the workspace-scope ones of every
/// project in `ws_ids` (projects.id → the label shown), plus the
/// server-scope ones named in `server`. Each workspace item records its
/// source id (`scopeId`) so the target can match by id (CL12).
pub(crate) fn seal_carry(
    vault: &Arc<LocalVault>,
    target_recipient: &str,
    target_vault_id: &str,
    ws_ids: &HashMap<String, String>,
    server: &[String],
    own_box_move: bool,
    target_workspace_id: Option<String>,
) -> Result<SealedCarry, VaultError> {
    let target = k2_vault::agefmt::Recipient::parse(target_recipient)?;
    if target_vault_id.trim().is_empty() {
        return Err(VaultError::bad_request("target_required", "the target vault id (k2 vault recipient on the target)"));
    }
    let all = vault.list(&k2_vault::ListFilter::default(), &k2_vault::Actor::System("carry".into()))?;
    let mut items = vec![];
    let mut never = vec![];
    let mut carried: Vec<(String, u32)> = vec![];
    for m in all {
        if m.namespace != k2_vault::Namespace::User {
            continue;
        }
        let (label, scope_id) = match m.scope.kind {
            k2_vault::ScopeKind::Workspace => match ws_ids.get(&m.scope.id) {
                Some(l) => (l.clone(), Some(m.scope.id.clone())),
                None => continue,
            },
            k2_vault::ScopeKind::Server if server.contains(&m.name) => (String::new(), None),
            _ => continue,
        };
        // CL13: K2 logins, App passes and engine credentials never travel.
        if k2_vault::carry::never_carried_kind(m.kind.as_str()) {
            never.push(m.name.clone());
            continue;
        }
        let v = vault.resolve(&k2_vault::Selector::Id(m.id.clone()), Use::System, &Requester::System("carry".into()))?;
        carried.push((m.id.clone(), m.latest_version));
        items.push(k2_vault::carry::CarryItem {
            scope_kind: m.scope.kind.as_str().into(),
            scope_label: label,
            scope_id,
            name: m.name.clone(),
            kind: m.kind.as_str().into(),
            hosts: m.hosts.clone(),
            inject: m.inject.clone(),
            default_delivery: m.default_delivery.as_str().into(),
            delivery_ceiling: m.delivery_ceiling.as_str().into(),
            description: m.description.clone(),
            human_given: m.human_given,
            value_b64: k2_vault::carry::CarryItem::encode_value(v.value.expose()),
            source_scope: None,
            grants: vec![],
        });
    }
    let names: Vec<(String, String)> = items.iter().map(|i| (i.scope_label.clone(), i.name.clone())).collect();
    let file = vault.carry_seal_for(&target, target_vault_id, own_box_move, target_workspace_id, items)?;
    Ok(SealedCarry { file, names, target_fingerprint: target.fingerprint(), never, ids: carried })
}

fn carry_export(vault: &Arc<LocalVault>, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    may_carry(c)?;
    let b: CarryExportBody = serde_json::from_slice(body).map_err(|_| VaultError::bad_request("bad_json", "bad request body"))?;
    let target_id = b.target_vault_id.clone().ok_or_else(|| VaultError::bad_request("target_required", "the target vault id (k2 vault recipient on the target)"))?;
    let mut ws_ids: HashMap<String, String> = HashMap::new();
    for w in &b.workspaces {
        let id = crate::vault::workspace_id(w).ok_or_else(|| VaultError::not_found(format!("no workspace named {w}")))?;
        let label = crate::vault::workspace_label(&id);
        ws_ids.insert(id, label);
    }
    let target_ws = b.target_workspace_id.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    if target_ws.is_some() && ws_ids.len() != 1 {
        return Err(VaultError::bad_request("one_workspace_per_target", "--target-workspace needs exactly one --workspace"));
    }
    let sealed = seal_carry(vault, b.target_recipient.as_deref().unwrap_or(""), &target_id, &ws_ids, &b.server, b.own_box_move, target_ws.clone())?;
    let n = sealed.names.len();
    let names: Vec<String> = sealed.names.iter().map(|(l, name)| format!("{}:{}", if l.is_empty() { "server" } else { l }, name)).collect();
    record_carry_export(vault, c, &sealed.target_fingerprint, &target_id, &b.workspaces, &sealed.ids, &names);
    Ok(json!({
        "ok": true,
        "count": n,
        "secrets": names,
        "carryB64": base64::engine::general_purpose::STANDARD.encode(&sealed.file),
        "targetFingerprint": sealed.target_fingerprint,
        "targetWorkspaceId": target_ws,
        "notCarried": sealed.never,
    }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CarryImportBody {
    carry_b64: Option<String>,
    #[serde(default)]
    apply: bool,
    /// The workspace on THIS box that gets the carry's workspace secrets:
    /// its id, or a name that resolves to exactly the workspace the carry
    /// was made for (CL12).
    workspace: Option<String>,
}

/// Where one item of an opened carry lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ItemTarget {
    Server,
    /// `projects.id` on this box.
    Workspace(String),
    /// Not imported, with the reason people see.
    Skip(String),
}

/// `projects.id` exists on this box (exact id, never a name).
fn project_exists(id: &str) -> bool {
    crate::vault::workspace_path(id).is_some()
}

/// The source workspaces in a carry, keyed by id (or the label for a
/// carry made before 0.46.0, which has no ids).
fn carry_sources(p: &k2_vault::carry::CarryPayload) -> Vec<String> {
    let mut v: Vec<String> = p
        .items
        .iter()
        .filter(|i| i.scope_kind != "server")
        .map(|i| i.scope_id.clone().unwrap_or_else(|| format!("label:{}", i.scope_label)))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Pick the target workspace for each item of a carry, by id only (CL12).
///
/// - `explicit` is an exact `projects.id` here → every workspace item goes
///   there (the carry must hold one source workspace, and a carry bound to
///   another workspace id is refused).
/// - `explicit` is a name → refused unless it resolves to exactly one
///   workspace AND the carry was made for that id.
/// - nothing named → the carry's bound target id, else the item's own
///   source id, when that id exists here. Otherwise skipped; never a name.
pub(crate) fn carry_targets(p: &k2_vault::carry::CarryPayload, explicit: Option<&str>) -> Result<Vec<ItemTarget>, VaultError> {
    let explicit = explicit.map(str::trim).filter(|s| !s.is_empty());
    let sources = carry_sources(p);
    let made_for = |id: &str| -> bool {
        match &p.target_workspace_id {
            Some(t) => t == id,
            None => sources.len() == 1 && p.items.iter().any(|i| i.scope_kind != "server" && i.scope_id.as_deref() == Some(id)),
        }
    };
    let chosen: Option<String> = match explicit {
        None => None,
        Some(tok) if project_exists(tok) => {
            if sources.len() > 1 {
                return Err(VaultError::bad_request(
                    "carry_has_many_workspaces",
                    format!("this carry holds secrets of {} workspaces; import it without --workspace, or carry one workspace at a time", sources.len()),
                ));
            }
            if let Some(t) = &p.target_workspace_id {
                if t != tok {
                    return Err(VaultError::forbidden(
                        "carry_for_another_workspace",
                        format!("this carry was made for workspace {t}, not {tok}"),
                    ));
                }
            }
            Some(tok.to_string())
        }
        Some(tok) => {
            let id = match crate::workspace_msg::resolve_workspace_detailed(tok) {
                crate::workspace_msg::WorkspaceResolve::Found(path) => crate::canonical_session::lookup_project_id(&path),
                crate::workspace_msg::WorkspaceResolve::Ambiguous { .. } => {
                    return Err(VaultError::conflict(
                        "workspace_name_ambiguous",
                        format!("{tok} names more than one workspace here; pass the workspace id (--workspace <id>)"),
                    ))
                }
                _ => None,
            };
            let Some(id) = id else {
                return Err(VaultError::not_found(format!("no workspace {tok} here; pass the workspace id (--workspace <id>)")));
            };
            if !made_for(&id) {
                return Err(VaultError::forbidden(
                    "carry_workspace_by_name",
                    format!(
                        "{tok} is workspace {id}, but this carry wasn't made for that id. Secrets never go to a workspace by name: \
                         pass --workspace {id} if that is the right one, or ask for a carry made with --target-workspace {id}"
                    ),
                ));
            }
            Some(id)
        }
    };
    Ok(p
        .items
        .iter()
        .map(|i| {
            if i.scope_kind == "server" {
                return ItemTarget::Server;
            }
            if let Some(id) = &chosen {
                return ItemTarget::Workspace(id.clone());
            }
            for cand in [p.target_workspace_id.as_ref(), i.scope_id.as_ref()].into_iter().flatten() {
                if project_exists(cand) {
                    return ItemTarget::Workspace(cand.clone());
                }
            }
            ItemTarget::Skip(format!(
                "{}: no workspace with id {} here (secrets never go by name); pass --workspace <id>",
                i.name,
                p.target_workspace_id.as_deref().or(i.scope_id.as_deref()).unwrap_or("(none in this carry)")
            ))
        })
        .collect())
}

/// Write an opened carry's secrets where `targets` says (one per item).
/// Returns `(imported names, skipped "name: why" lines)`; an existing
/// secret is kept, never overwritten.
pub(crate) fn apply_carry(
    vault: &Arc<LocalVault>,
    p: &k2_vault::carry::CarryPayload,
    targets: &[ItemTarget],
) -> Result<(Vec<String>, Vec<String>), VaultError> {
    if targets.len() != p.items.len() {
        return Err(VaultError::internal("carry targets don't match the items"));
    }
    let mut imported = vec![];
    let mut skipped = vec![];
    for (i, t) in p.items.iter().zip(targets) {
        // CL13 on import too: whatever a carry holds, these never land.
        if k2_vault::carry::never_carried_kind(&i.kind) || i.scope_kind == "session" {
            skipped.push(format!("{}: {} never travels; make it again here", i.name, i.kind));
            continue;
        }
        let scope = match t {
            ItemTarget::Server => k2_vault::Scope::server(),
            ItemTarget::Workspace(id) => k2_vault::Scope::workspace(id.clone()),
            ItemTarget::Skip(why) => {
                skipped.push(why.clone());
                continue;
            }
        };
        let mut req = k2_vault::PutRequest::new(k2_vault::Namespace::User, scope, i.name.clone(), k2_vault::Kind::parse(&i.kind)?);
        req.hosts = i.hosts.clone();
        req.inject = i.inject.clone();
        req.default_delivery = Some(k2_vault::Delivery::parse(&i.default_delivery)?);
        req.delivery_ceiling = Some(k2_vault::Delivery::parse(&i.delivery_ceiling)?);
        req.description = i.description.clone();
        req.origin = format!("import:clone:{}", p.source_vault_id);
        req.mode = k2_vault::PutMode::Create;
        // VS20: person-given only survives the owner's own move.
        req.human_given_override = Some(i.human_given && p.own_box_move);
        match vault.put(req, i.value()?, &k2_vault::Actor::System("carry".into())) {
            Ok(_) => imported.push(i.name.clone()),
            Err(e) if e.code == "secret_exists" => skipped.push(format!("{}: already here (kept this box's)", i.name)),
            Err(e) => skipped.push(format!("{}: {}", i.name, e.code)),
        }
    }
    Ok((imported, skipped))
}

fn carry_import(vault: &Arc<LocalVault>, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    may_carry(c)?;
    let b: CarryImportBody = serde_json::from_slice(body).map_err(|_| VaultError::bad_request("bad_json", "bad request body"))?;
    let file = base64::engine::general_purpose::STANDARD
        .decode(b.carry_b64.as_deref().unwrap_or("").trim())
        .map_err(|_| VaultError::bad_request("bad_carry", "carryB64 isn't base64"))?;
    let p = vault.carry_open(&file)?;
    let targets = carry_targets(&p, b.workspace.as_deref())?;
    let source_fp = k2_vault::crypto::hex(&k2_vault::crypto::sha256(p.source_public_key.as_bytes())[..8]);
    let listing: Vec<Value> = p
        .items
        .iter()
        .zip(&targets)
        .map(|(i, t)| {
            let (target_ws, target_label, skip) = match t {
                ItemTarget::Server => (None, None, None),
                ItemTarget::Workspace(id) => (Some(id.clone()), Some(crate::vault::workspace_label(id)), None),
                ItemTarget::Skip(why) => (None, None, Some(why.clone())),
            };
            json!({
                "scope": if i.scope_label.is_empty() { "server".to_string() } else { format!("ws:{}", i.scope_label) },
                "sourceWorkspaceId": i.scope_id,
                "targetWorkspaceId": target_ws,
                "targetWorkspace": target_label,
                "skip": skip,
                "name": i.name,
                "hosts": i.hosts,
                "delivery": i.default_delivery,
            })
        })
        .collect();
    if !b.apply {
        return Ok(json!({
            "ok": true,
            "dryRun": true,
            "sourceVaultId": p.source_vault_id,
            "sourceKeyFingerprint": source_fp,
            "ownBoxMove": p.own_box_move,
            "madeForWorkspaceId": p.target_workspace_id,
            "secrets": listing,
        }));
    }
    let (imported, skipped) = apply_carry(vault, &p, &targets)?;
    let into: Vec<String> = {
        let mut v: Vec<String> = targets.iter().filter_map(|t| if let ItemTarget::Workspace(id) = t { Some(id.clone()) } else { None }).collect();
        v.sort();
        v.dedup();
        v
    };
    crate::vault::auth_audit(
        "vault-carry",
        &c.actor.stamp(),
        &format!("ok import {} from {} into {}", imported.len(), p.source_vault_id, if into.is_empty() { "-".to_string() } else { into.join(",") }),
        "-",
    );
    Ok(json!({ "ok": true, "imported": imported, "skipped": skipped, "workspaceIds": into }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupBody {
    dest_dir: Option<String>,
    recipient: Option<String>,
}

fn backup(vault: &Arc<LocalVault>, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    if c.is_passport() || !c.actor.is_admin_human() {
        return Err(VaultError::forbidden("owner_only", "backups are made by the owner or K2's backup job"));
    }
    let b: BackupBody = serde_json::from_slice(body).map_err(|_| VaultError::bad_request("bad_json", "bad request body"))?;
    let d = PathBuf::from(b.dest_dir.ok_or_else(|| VaultError::bad_request("dest_required", "where should the snapshot go?"))?);
    if !d.is_dir() {
        return Err(VaultError::not_found("that folder doesn't exist"));
    }
    let dest = d.join("vault.db");
    vault.snapshot_to(&dest)?;
    Ok(json!({ "ok": true, "path": dest.display().to_string(), "note": "ciphertext only; restoring it on a new box needs the recovery key or the backup key" }))
}

fn backup_recipient(vault: &Arc<LocalVault>, c: &VaultCaller, body: &[u8]) -> Result<Value, VaultError> {
    if c.is_passport() || !c.actor.is_admin_human() {
        return Err(VaultError::forbidden("owner_only", "the backup key is set by the owner"));
    }
    let b: BackupBody = serde_json::from_slice(body).map_err(|_| VaultError::bad_request("bad_json", "bad request body"))?;
    let r = k2_vault::agefmt::Recipient::parse(b.recipient.as_deref().unwrap_or(""))?;
    vault.add_wrap("backup", &r, &c.actor)?;
    crate::vault::auth_audit("vault-backup-key", &c.actor.stamp(), &format!("ok {}", r.fingerprint()), "-");
    Ok(json!({ "ok": true, "fingerprint": r.fingerprint() }))
}

/// A published service's id from its label (`name`) or id. Agents see
/// only their own workspace's services.
pub fn published_service_id(c: &VaultCaller, label: &str) -> Result<String, VaultError> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT id, project_id FROM published_services WHERE id = ?1 OR name = ?1 ORDER BY (id = ?1) DESC LIMIT 1",
            rusqlite::params![label],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let (id, project) = row.ok_or_else(|| VaultError::not_found(format!("no published service {label}")))?;
    if let Some(ws) = c.workspace_id() {
        if ws != project {
            return Err(VaultError::not_found(format!("no published service {label} in this workspace")));
        }
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End to end through the route: the agent names the secret, the
    /// upstream gets it in the sealed header, the echo comes back
    /// redacted, another host is refused before any connection.
    #[test]
    fn fetch_route_injects_sealed_header_and_redacts_the_echo() {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicUsize, Ordering};

        use k2_vault::confirm::KdfCost;
        use k2_vault::keystore::fake::FakeKeyStore;
        use k2_vault::{Actor, AgentActor, KeyStore, Kind, Namespace, OpenOptions, PutRequest, Scope, SecretBytes};

        const VAL: &str = "k2test_ROUTEFETCH_9a8b7c6d5e4f3a2b1c";
        crate::test_support::with_temp_home(|| {
            let t = k2_vault::fsutil::TestDir::new("fetch-route");
            let ks: Arc<dyn KeyStore> = Arc::new(FakeKeyStore::new());
            let v = Arc::new(LocalVault::open(t.path().join("vault"), ks, OpenOptions { kdf: KdfCost::Floor }).expect("open"));
            let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            let port = l.local_addr().expect("addr").port();
            let conns = Arc::new(AtomicUsize::new(0));
            let c2 = conns.clone();
            std::thread::spawn(move || {
                for s in l.incoming() {
                    let Ok(mut s) = s else { return };
                    c2.fetch_add(1, Ordering::SeqCst);
                    let mut buf = vec![0u8; 8192];
                    let mut got = Vec::new();
                    while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                        let n = s.read(&mut buf).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        got.extend_from_slice(&buf[..n]);
                    }
                    let head = String::from_utf8_lossy(&got).to_string();
                    let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", head.len(), head);
                    let _ = s.write_all(resp.as_bytes());
                }
            });
            let ws = "ws-fetch";
            let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws), "STRIPE_KEY", Kind::ApiKey);
            r.hosts = vec![format!("http://127.0.0.1:{port}")];
            v.put(r, SecretBytes::from_slice(VAL.as_bytes()), &Actor::Owner).expect("put");
            let c = VaultCaller {
                actor: Actor::Agent(AgentActor { workspace_id: ws.into(), session_id: Some("s1".into()), shell: false, vault_manage: false }),
                channel: crate::vault::actor::Channel::Plain,
                session_id: Some("s1".into()),
            };
            let body = json!({ "secret": "STRIPE_KEY", "url": format!("http://127.0.0.1:{port}/v1/charges") }).to_string();
            let r = handle(&v, FETCH, &c, body.as_bytes(), &HashMap::new(), "test");
            assert_eq!(r.status, "200 OK", "{}", r.body);
            assert!(!r.body.contains(VAL), "the value never comes back: {}", r.body);
            let out: Value = serde_json::from_str(&r.body).expect("json");
            let echoed = out["body"].as_str().expect("text body");
            assert!(echoed.to_ascii_lowercase().contains("authorization: [k2v:stripe_key]") || echoed.contains("[k2v:STRIPE_KEY]"), "{echoed}");
            assert_eq!(conns.load(Ordering::SeqCst), 1);
            // Another host: refused before any connection.
            let body = json!({ "secret": "STRIPE_KEY", "url": "http://127.0.0.1:1/x" }).to_string();
            let r = handle(&v, FETCH, &c, body.as_bytes(), &HashMap::new(), "test");
            assert!(r.status.starts_with("403"), "{}", r.body);
            assert!(r.body.contains("host_not_allowed"), "{}", r.body);
            // A person can't use fetch.
            let owner = VaultCaller { actor: Actor::Owner, channel: crate::vault::actor::Channel::Plain, session_id: None };
            let r = handle(&v, FETCH, &owner, body.as_bytes(), &HashMap::new(), "test");
            assert!(r.status.starts_with("403"), "{}", r.body);
        });
    }

    #[test]
    fn jail_keeps_paths_inside_the_workspace() {
        let t = k2_vault::fsutil::TestDir::new("jail");
        let ws = t.path().join("ws");
        std::fs::create_dir_all(ws.join("sub")).expect("dirs");
        jailed_path(&ws, "out.json").expect("inside");
        jailed_path(&ws, "sub/out.json").expect("inside");
        assert!(jailed_path(&ws, "../escape.json").is_err());
        assert!(jailed_path(&ws, "/etc/passwd").is_err());
        std::os::unix::fs::symlink(t.path(), ws.join("link")).expect("symlink");
        assert!(jailed_path(&ws, "link/x").is_err(), "a symlinked folder can't lead outside");
    }
}
