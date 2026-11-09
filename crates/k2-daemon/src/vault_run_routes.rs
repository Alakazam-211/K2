//! `/cli/vault/run` and `/cli/vault/run-helper`: `k2 vault run`
//! (prd-vault-for-agents-v1 A1).
//!
//! The daemon owns policy, audit and the per-run files; the helper
//! (`vault/run.rs`, `k2-daemon --vault-run`) only builds the child's
//! environment and masks its output. A secret reaches the child at the
//! delivery the caller already has, never wider (V22): env as env, file as
//! a per-run 0600 file plus `<ENV>_FILE`. The answer holds values the caller
//! could already read with `k2 vault env-file` or `k2 vault file`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use k2_vault::{Delivery, LocalVault, Selector, VaultError};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::vault::actor::VaultCaller;
use crate::vault_routes::{not_found_elsewhere, refuse, Sel};
use crate::vault_use_routes::{jailed_path, use_refusal, use_selector};

pub const RUN: &str = "/cli/vault/run";
/// Where this daemon's binary is, for the CLI to exec as the helper.
pub const RUN_HELPER: &str = "/cli/vault/run-helper";

pub const ROUTES: &[&str] = &[RUN, RUN_HELPER];
pub const POST_ROUTES: &[&str] = &[RUN];

/// More than anyone means; the audit and the answer stay small.
const MAX_SECRETS: usize = 64;
const MAX_ENV_FILE: u64 = 256 * 1024;

pub fn is_route(path: &str) -> bool {
    ROUTES.contains(&path)
}

pub fn is_post_route(path: &str) -> bool {
    POST_ROUTES.contains(&path)
}

/// What a run depends on outside the vault, so the tests name it instead of
/// reaching for the daemon's session map, database and memory folder.
struct RunCtx {
    sandboxed: bool,
    workspace_root: Option<PathBuf>,
    secrets_root: Option<PathBuf>,
}

impl RunCtx {
    fn live(c: &VaultCaller) -> RunCtx {
        RunCtx {
            sandboxed: c.session_id.as_deref().is_some_and(session_is_sandboxed),
            workspace_root: c.workspace_id().and_then(crate::vault::workspace_path).map(PathBuf::from),
            secrets_root: crate::vault::spawn::secrets_root(),
        }
    }
}

/// Is this session a microVM? A session that isn't in the map has ended
/// (its cell socket went with it), so it can't be a live sandbox; that is
/// the same reading `caller_from_session` gives a passport whose session
/// is gone.
fn session_is_sandboxed(session_id: &str) -> bool {
    k2_core::session::SessionId::parse(session_id)
        .and_then(|id| crate::v2_session_map::lookup_by_session_id(&id))
        .is_some_and(|s| matches!(s.sandbox, k2_core::terminal::SandboxSpec::Microvm))
}

pub fn handle(vault: &Arc<LocalVault>, path: &str, c: &VaultCaller, body: &[u8]) -> CliResponse {
    let r = match path {
        RUN_HELPER => run_helper(),
        RUN => run(vault, c, body, &RunCtx::live(c)),
        _ => Err(VaultError::not_found("no such vault route")),
    };
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => refuse(&e),
    }
}

/// `std::env::current_exe` on Linux reads `/proc/self/exe`, which ends in
/// " (deleted)" once an update has replaced the binary on disk.
fn installed_exe(p: PathBuf) -> PathBuf {
    match p.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(s) => PathBuf::from(s),
        None => p,
    }
}

fn run_helper() -> Result<Value, VaultError> {
    let exe = std::env::current_exe().map_err(|e| VaultError::internal(format!("run helper: {}", e.kind())))?;
    let exe = installed_exe(exe);
    if !exe.is_file() {
        return Err(VaultError::new("run_helper_missing", 409, "the k2-daemon binary isn't where it started; restart the K2 daemon"));
    }
    Ok(json!({ "helper": exe.display().to_string() }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunSecret {
    name: String,
    env: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunBody {
    #[serde(default)]
    secrets: Vec<RunSecret>,
    env_file: Option<String>,
    scope: Option<String>,
    /// The command's basename, for the audit. Argv is never sent.
    cmd: Option<String>,
    #[serde(default)]
    no_mask: bool,
}

/// One secret the run asked for.
struct Want {
    sel: Selector,
    /// For a by-name "no such secret" that can say where the name does live.
    asked: Sel,
    name: String,
    env: Option<String>,
}

/// `--scope` as typed (`ws:sales`, `workspace`, `server`, `session`,
/// `session:<id>`), the spellings the CLI's other verbs take.
fn sel_named(name: &str, scope: Option<&str>) -> Sel {
    let mut s = Sel { name: Some(name.to_string()), ..Default::default() };
    let Some(raw) = scope.map(str::trim).filter(|x| !x.is_empty()) else { return s };
    if let Some(w) = raw.strip_prefix("ws:").or_else(|| raw.strip_prefix("workspace:")) {
        s.scope = Some("workspace".into());
        s.workspace = Some(w.to_string());
    } else if let Some(id) = raw.strip_prefix("session:") {
        s.scope = Some("session".into());
        s.session = Some(id.to_string());
    } else {
        s.scope = Some(raw.to_string());
    }
    s
}

/// The audit keeps the command's basename, in characters a log can hold.
fn audit_cmd(raw: Option<&str>) -> String {
    let last = raw.unwrap_or("").rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = last.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-')).take(64).collect();
    if clean.is_empty() {
        "command".to_string()
    } else {
        clean
    }
}

fn plain_name_ok(k: &str) -> bool {
    !k.is_empty()
        && k.bytes().next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// An env file's entries as (key, value-or-reference). It must really be
/// inside the workspace: a link inside pointing outside is refused too.
fn read_env_file(root: &Path, rel: &str) -> Result<Vec<(String, String)>, VaultError> {
    let outside = || VaultError::bad_request("path_outside_workspace", "use a path inside your workspace");
    let unreadable = || VaultError::not_found("that env file isn't readable from this workspace");
    let p = jailed_path(root, rel)?;
    let real = dunce::canonicalize(&p).map_err(|_| unreadable())?;
    let real_root = dunce::canonicalize(root).map_err(|_| outside())?;
    if !real.starts_with(&real_root) {
        return Err(outside());
    }
    let meta = std::fs::metadata(&real).map_err(|_| unreadable())?;
    if !meta.is_file() {
        return Err(unreadable());
    }
    if meta.len() > MAX_ENV_FILE {
        return Err(VaultError::bad_request("env_file_too_large", "that env file is over 256 KiB"));
    }
    let text = std::fs::read_to_string(&real).map_err(|_| VaultError::bad_request("not_text", "that env file isn't text"))?;
    Ok(crate::vault::publish_env::parse_env_k2(&text))
}

/// What came back for one secret, between the resolve pass and the write
/// pass: nothing is written until every secret has passed.
struct Got {
    name: String,
    env: String,
    is_env: bool,
    value: k2_vault::SecretBytes,
}

fn b64(v: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(v)
}

fn run(vault: &Arc<LocalVault>, c: &VaultCaller, body: &[u8], ctx: &RunCtx) -> Result<Value, VaultError> {
    c.actor.agent().ok_or_else(|| VaultError::forbidden("use_run_as_agent", "k2 vault run runs for an agent session; people use k2 vault reveal"))?;
    if ctx.sandboxed {
        return Err(VaultError::forbidden("run_not_in_sandbox", "k2 vault run isn't available in a sandboxed session; use k2 vault fetch"));
    }
    let b: RunBody = serde_json::from_slice(body).map_err(|_| VaultError::bad_request("bad_json", "bad request body"))?;
    if b.secrets.is_empty() && b.env_file.is_none() {
        return Err(VaultError::bad_request("secret_required", "name the secrets: --secret NAME or --env-file .env.k2"));
    }
    if b.secrets.len() > MAX_SECRETS {
        return Err(VaultError::bad_request("too_many_secrets", format!("at most {MAX_SECRETS} secrets per run")));
    }

    let mut wants: Vec<Want> = Vec::with_capacity(b.secrets.len());
    let mut plain: Vec<(String, String)> = Vec::new();
    if let Some(rel) = b.env_file.as_deref() {
        let root = ctx.workspace_root.as_deref().ok_or_else(|| VaultError::not_found("workspace folder not found"))?;
        for (i, (k, v)) in read_env_file(root, rel)?.into_iter().enumerate() {
            // Never echo the name: a malformed line may be a pasted value.
            let bad = VaultError::bad_request("bad_env_name", format!("entry {} of the env file (not counting comments and blank lines) has a bad name", i + 1));
            if !k2_vault::refs::VaultRef::is_ref(&v) {
                if !plain_name_ok(&k) {
                    return Err(bad);
                }
                plain.push((k, v));
                continue;
            }
            if k2_vault::delivery::validate_env_name(&k).is_err() {
                return Err(bad);
            }
            let r = k2_vault::refs::VaultRef::parse(&v)?;
            if r.version.is_some() {
                return Err(VaultError::bad_request("run_pinned_ref", format!("{k}: k2 vault run uses the secret's current version; remove the @version")));
            }
            let asked = Sel { reference: Some(v.clone()), ..Default::default() };
            wants.push(Want { sel: use_selector(vault, c, &asked)?, asked, name: r.name, env: Some(k) });
        }
    }
    for s in &b.secrets {
        let name = s.name.trim();
        if name.is_empty() {
            return Err(VaultError::bad_request("secret_required", "--secret needs a name"));
        }
        if let Some(e) = s.env.as_deref() {
            k2_vault::delivery::validate_env_name(e)?;
        }
        let asked = sel_named(name, b.scope.as_deref());
        wants.push(Want { sel: use_selector(vault, c, &asked)?, asked, name: name.to_string(), env: s.env.clone() });
    }

    let who = c.requester()?;
    let consumer = format!("run:{}", c.session_id.as_deref().unwrap_or("unknown"));
    let outcome = format!("ok:run {}{}", audit_cmd(b.cmd.as_deref()), if b.no_mask { " no-mask" } else { "" });
    let mut got: Vec<Got> = Vec::with_capacity(wants.len());
    let mut names_taken: HashSet<String> = HashSet::new();
    for w in wants {
        let r = vault
            .resolve_for_run(&w.sel, &who, &consumer, &outcome)
            .map_err(|e| use_refusal(vault, c, &w.sel, &who, &w.name, e, Delivery::Env))
            .map_err(|e| not_found_elsewhere(vault, c, &w.asked, e))?;
        let is_env = r.delivery.level() >= Delivery::Env.level();
        let base = w.env.or_else(|| r.env_name.clone()).unwrap_or_else(|| k2_vault::delivery::default_env_name(&r.meta.name));
        let env = if is_env { base } else { format!("{base}_FILE") };
        k2_vault::delivery::validate_env_name(&env)?;
        if is_env && r.value.expose().contains(&0) {
            return Err(VaultError::bad_request("value_has_nul", format!("{} holds a NUL byte, which an environment variable can't; deliver it as a file", r.meta.name)));
        }
        if !names_taken.insert(env.clone()) {
            return Err(VaultError::bad_request("duplicate_env_name", format!("two secrets would both set {env}; rename one with --secret NAME=ENV_NAME")));
        }
        got.push(Got { name: r.meta.name.clone(), env, is_env, value: r.value });
    }

    let run_id = k2_vault::crypto::base62(16);
    let mut run_dir: Option<PathBuf> = None;
    let mut items: Vec<Value> = Vec::with_capacity(got.len() + plain.len());
    if got.iter().any(|g| !g.is_env) {
        let root = ctx.secrets_root.clone().ok_or_else(|| VaultError::new("no_memory_fs", 409, "this machine has no memory-backed folder for secret files"))?;
        let runs = root.join("run");
        let dir = runs.join(&run_id);
        k2_vault::fsutil::ensure_private_dir(&root)?;
        k2_vault::fsutil::ensure_private_dir(&runs)?;
        k2_vault::fsutil::ensure_private_dir(&dir)?;
        run_dir = Some(dir);
    }
    for (i, g) in got.iter().enumerate() {
        if g.is_env {
            items.push(json!({ "name": g.name, "env": g.env, "how": "env", "valueB64": b64(g.value.expose()) }));
            continue;
        }
        let dir = run_dir.as_deref().expect("a file item has a run dir");
        let tag: String = g.name.chars().filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')).take(40).collect();
        let file = format!("{i}-{tag}");
        if let Err(e) = crate::vault::spawn::write_secret_file(dir, &file, g.value.expose()) {
            let _ = std::fs::remove_dir_all(dir);
            return Err(VaultError::internal(format!("run file: {}", e.kind())));
        }
        // `maskB64` is only for the output mask; `valueB64` would be env.
        items.push(json!({ "name": g.name, "env": g.env, "how": "file", "path": dir.join(&file).display().to_string(), "maskB64": b64(g.value.expose()) }));
    }
    // A secret beats a plain line of the same name.
    for (k, v) in plain {
        if !names_taken.contains(&k) {
            items.push(json!({ "name": k, "env": k, "how": "plain", "valueB64": b64(v.as_bytes()) }));
        }
    }
    let mut out = json!({ "runId": run_id, "items": items });
    if let Some(d) = run_dir {
        out["runDir"] = json!(d.display().to_string());
    }
    Ok(out)
}

#[cfg(test)]
#[path = "vault_run_routes_tests.rs"]
mod tests;
