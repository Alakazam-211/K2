//! Published services' environment (§10.5, KV27, Q16).
//!
//! - New services start with a **clean** environment: a fixed base
//!   (`PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `LANG`, `LC_*`, `TZ`,
//!   `TMPDIR`, `PORT`, `K2_PUBLISH_*`) plus what the owner granted them:
//!   vault secrets bound to the service (file by default, env when chosen)
//!   and the `k2v://` references in its `.env.k2`.
//! - Services that existed before 0.45.2 keep inheriting the daemon's
//!   environment (`legacy`) until the owner switches them; K2 records the
//!   variable names such a service actually got, so Settings can show
//!   what switching would remove. Never values.
//!
//! The per-service mode lives in `~/.k2/publish-env.json` (0600; names
//! and modes only), so this needs no main-DB migration.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

use k2_vault::{Delivery, Requester, SecretStore, Selector, Use};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceEnv {
    /// `clean` | `legacy`
    pub mode: String,
    /// Workspace-relative `.env.k2` to resolve at start (default `.env.k2`
    /// when present in the service's folder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_file: Option<String>,
    /// Names (never values) a legacy service inherited at its last start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inherited_names: Vec<String>,
    /// 0.46.0 grace: the cut-over behaviour (0.46.4), opted into early (Rosson's
    /// cross-over plan, 2026-10-14). Implies `clean`, plus: `PATH` is the
    /// system folders only, `HOME` is the service's own folder under
    /// `~/.k2/publish-homes/`, and an owner-token call the daemon traces
    /// to this service is refused (Linux).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clean_mode: bool,
    /// R8 review fix 6: the `.env.k2` references the vault refused at the
    /// service's last start (`KEY: code`, names only). Rewritten on every
    /// start (empty clears it); shown in the service's status.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refused_refs: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Modes {
    /// `true` once existing services were marked legacy (first boot).
    #[serde(default)]
    seeded: bool,
    #[serde(default)]
    services: BTreeMap<String, ServiceEnv>,
}

static LOCK: Mutex<()> = Mutex::new(());

fn path() -> PathBuf {
    k2_core::paths::k2_home().join("publish-env.json")
}

fn load() -> Modes {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(m: &Modes) -> Result<(), String> {
    let body = serde_json::to_vec_pretty(m).map_err(|e| e.to_string())?;
    k2_vault::fsutil::write_private_atomic(&path(), &body).map_err(|e| e.hint)
}

/// First boot with the vault: every service that already exists keeps
/// its inherited environment (Q16); later services start clean.
pub fn seed_existing() {
    let _g = LOCK.lock();
    let mut m = load();
    if m.seeded {
        return;
    }
    let db = k2_core::db::shared();
    let ids: Vec<String> = {
        let conn = db.lock();
        let st = conn.prepare("SELECT id FROM published_services");
        match st {
            Ok(mut s) => match s.query_map([], |r| r.get::<_, String>(0)) {
                Ok(i) => i.flatten().collect(),
                Err(_) => vec![],
            },
            Err(_) => vec![],
        }
    };
    for id in ids {
        m.services.entry(id).or_insert_with(|| ServiceEnv { mode: "legacy".into(), ..Default::default() });
    }
    m.seeded = true;
    if let Err(e) = save(&m) {
        k2_core::log_debug!("[vault] publish-env seed: {e}");
    }
}

pub fn get(service_id: &str) -> ServiceEnv {
    let m = load();
    // Until the first-boot pass ran (no vault yet), nothing changes for
    // any service: they all keep inheriting.
    let default_mode = if m.seeded { "clean" } else { "legacy" };
    m.services.get(service_id).cloned().unwrap_or(ServiceEnv { mode: default_mode.into(), ..Default::default() })
}

/// Rosson 2026-10-10: services from before 0.45.4 keep inheriting the
/// daemon's environment, with this notice wherever the service shows
/// (status, `k2 publish list`, the Published drawer, `k2 vault doctor`).
/// New services start clean. The 2026-10-14 cross-over plan moved the end
/// from 0.46.0 to 0.46.1, Rosson moved it to 0.46.2 (2026-10-18) and then to
/// 0.46.4 (2026-10-20: in 0.46.3 each agent moves its service to an App itself): nothing before
/// 0.46.4 removes it.
pub const LEGACY_ENV_REMOVED_IN: &str = k2_core::app_grace::CUTOVER_RELEASE;

/// The deprecation notice for one service (`name` = its publish name).
pub fn legacy_env_notice(name: &str) -> String {
    format!(
        "This service still inherits the K2 daemon's whole environment because it existed \
         before 0.45.4. That ends {ends}: it will then start with a \
         clean environment plus only the secrets granted to it. Switch now with \
         `k2 publish env {name} clean` (grant what it needs with `k2 publish secret add {name} …`), \
         see what would break with `k2 app warnings {name}`, or try the whole \
         {LEGACY_ENV_REMOVED_IN} behaviour first with `k2 app clean-mode {name} on`. \
         Move it to an App {when}: `k2 study app-move`.",
        ends = k2_core::app_grace::cutover_phrase(),
        when = if k2_core::app_grace::cutover_now() { "now" } else { "before then" },
    )
}

/// `true` only for a service explicitly kept on the inherited environment
/// (`legacy`), never for one that merely has no entry yet.
pub fn is_legacy(service_id: &str) -> bool {
    load()
        .services
        .get(service_id)
        .is_some_and(|e| e.mode == "legacy")
}

/// The ids of every service kept on the inherited environment.
pub fn legacy_ids() -> Vec<String> {
    load()
        .services
        .into_iter()
        .filter(|(_, e)| e.mode == "legacy")
        .map(|(id, _)| id)
        .collect()
}

pub fn set(service_id: &str, mode: Option<&str>, env_file: Option<Option<String>>) -> Result<ServiceEnv, String> {
    let _g = LOCK.lock();
    let mut m = load();
    let e = m.services.entry(service_id.to_string()).or_insert_with(|| ServiceEnv { mode: "clean".into(), ..Default::default() });
    if let Some(md) = mode {
        if md != "clean" && md != "legacy" {
            return Err("mode is clean or legacy".into());
        }
        e.mode = md.to_string();
        if md == "clean" {
            e.inherited_names.clear();
        } else {
            // Inheriting again ends the early cut-over behaviour too.
            e.clean_mode = false;
        }
    }
    if let Some(f) = env_file {
        e.env_file = f.filter(|x| !x.trim().is_empty());
    }
    let out = e.clone();
    save(&m)?;
    Ok(out)
}

/// Turn clean mode (the cut-over behaviour, 0.46.4) on or off for one service. On
/// also switches the env mode to `clean`; off keeps `clean` (it never
/// gives the daemon's environment back: that is `set(.., "legacy")`,
/// owner only). Names recorded while the service was legacy are kept, so
/// the warnings can still name them.
pub fn set_clean_mode(service_id: &str, on: bool) -> Result<ServiceEnv, String> {
    let _g = LOCK.lock();
    let mut m = load();
    let e = m.services.entry(service_id.to_string()).or_insert_with(|| ServiceEnv { mode: "clean".into(), ..Default::default() });
    e.clean_mode = on;
    if on {
        e.mode = "clean".into();
    }
    let out = e.clone();
    save(&m)?;
    Ok(out)
}

pub fn is_clean_mode(service_id: &str) -> bool {
    load().services.get(service_id).is_some_and(|e| e.clean_mode)
}

/// The ids of every service in clean mode.
pub fn clean_mode_ids() -> Vec<String> {
    load().services.into_iter().filter(|(_, e)| e.clean_mode).map(|(id, _)| id).collect()
}

/// A clean-mode service's own `HOME` (`~/.k2/publish-homes/<service id>`,
/// 0700): `$HOME/.k2/…` lookups no longer find the daemon's files, as in
/// the 0.46.1 wall.
pub fn service_home(service_id: &str) -> PathBuf {
    let safe: String = service_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    k2_core::paths::k2_home().join("publish-homes").join(safe)
}

fn ensure_service_home(service_id: &str) -> PathBuf {
    let h = service_home(service_id);
    if std::fs::create_dir_all(&h).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&h, std::fs::Permissions::from_mode(0o700));
        }
    }
    h
}

fn record_inherited(service_id: &str, names: Vec<String>) {
    let _g = LOCK.lock();
    let mut m = load();
    if let Some(e) = m.services.get_mut(service_id) {
        if e.inherited_names != names {
            e.inherited_names = names;
            let _ = save(&m);
        }
    }
}

const BASE_EXACT: &[&str] = &["PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "TZ", "TMPDIR"];

fn base_env() -> HashMap<String, String> {
    std::env::vars()
        .filter(|(k, _)| BASE_EXACT.contains(&k.as_str()) || k.starts_with("LC_") || k.starts_with("K2_PUBLISH_"))
        .collect()
}

/// Parse `.env.k2` lines into (key, value-or-ref).
pub fn parse_env_k2(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim().trim_start_matches("export ");
            if l.is_empty() || l.starts_with('#') {
                return None;
            }
            let (k, v) = l.split_once('=')?;
            let v = v.trim().trim_matches('"').trim_matches('\'');
            Some((k.trim().to_string(), v.to_string()))
        })
        .collect()
}

/// S24-9: every published `--cmd` service learns which App it is and where
/// the daemon is, in both env modes. A BFF names its App on
/// `POST /cli/skin/login` with `{"app": $K2_APP_ID}`; `K2_HOST`/`K2_PORT`
/// are the daemon's loopback address (runtime litmus D2).
pub const APP_ID_ENV: &str = "K2_APP_ID";

/// The App identity variables (names and values) for one service.
pub fn app_identity_env(service_id: &str, daemon_port: u16) -> Vec<(&'static str, String)> {
    let mut v = vec![
        (APP_ID_ENV, service_id.to_string()),
        ("K2_HOST", "127.0.0.1".to_string()),
    ];
    if daemon_port != 0 {
        v.push(("K2_PORT", daemon_port.to_string()));
    }
    v
}

/// 0.46.0 grace: the variables every published `--cmd` service gets on
/// top of S24's identity, in every mode: `K2_APP_NAME` (its publish name)
/// and `K2_GATEWAY_URL` (where an SDK's `fromEnv()` reaches K2; the
/// daemon's loopback until the 0.46.1 link replaces it). A BFF that read
/// `~/.k2/heartbeat.port` keeps working from `K2_HOST`/`K2_PORT`.
pub fn service_identity_env(service_id: &str, name: &str, daemon_port: u16) -> Vec<(&'static str, String)> {
    let mut v = app_identity_env(service_id, daemon_port);
    if !name.is_empty() {
        v.push((crate::apps::process_engine::APP_NAME_ENV, name.to_string()));
    }
    if daemon_port != 0 {
        v.push(("K2_GATEWAY_URL", format!("http://127.0.0.1:{daemon_port}")));
    }
    v
}

/// What [`apply_service`] did.
#[derive(Debug, Default)]
pub struct Applied {
    /// File secrets to remove when the service stops.
    pub secrets_dir: Option<PathBuf>,
    /// Clean mode set `PATH` itself: the spawner must not enrich it.
    pub fixed_path: bool,
}

/// Prepare the child's environment. Returns the secrets dir to remove
/// when the service stops (file deliveries).
pub fn apply(cmd: &mut Command, service_id: &str, workspace_id: &str, cwd: &Path, port: u16) -> Option<PathBuf> {
    apply_service(cmd, service_id, "", workspace_id, cwd, port).secrets_dir
}

/// Prepare a published service's environment (`name` = its publish name).
///
/// - `legacy`: the daemon's whole environment (recorded by name), plus the
///   identity block and, since 0.46.0, the secrets granted to it (adding,
///   not removing: 0.46.0 removes nothing).
/// - `clean`: the base, `PORT`, the identity block, the grants.
/// - clean mode: as `clean`, but `PATH` is
///   [`k2_core::app_grace::scan::CLEAN_PATH`] and `HOME` is the service's
///   own folder.
pub fn apply_service(cmd: &mut Command, service_id: &str, name: &str, workspace_id: &str, cwd: &Path, port: u16) -> Applied {
    let se = get(service_id);
    let identity = service_identity_env(service_id, name, crate::publish_runtime::loopback_port());
    // 0.46.1: the service's public https origins (unset when it has none).
    let origins = crate::apps::origins::env_pairs(&crate::apps::origins::for_service(service_id, name));
    if se.mode == "legacy" && !se.clean_mode {
        let mut names: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
        names.sort();
        record_inherited(service_id, names);
        cmd.envs(identity);
        cmd.env_remove(crate::apps::origins::ORIGIN_ENV);
        cmd.env_remove(crate::apps::origins::ORIGINS_ENV);
        cmd.envs(origins);
        let secrets_dir = apply_granted(cmd, service_id, workspace_id, cwd);
        return Applied { secrets_dir, fixed_path: false };
    }
    cmd.env_clear();
    cmd.envs(base_env());
    if se.clean_mode {
        cmd.env("PATH", k2_core::app_grace::scan::CLEAN_PATH);
        cmd.env("HOME", ensure_service_home(service_id));
    }
    cmd.env("PORT", port.to_string());
    cmd.envs(identity);
    cmd.envs(origins);
    let secrets_dir = apply_granted(cmd, service_id, workspace_id, cwd);
    Applied { secrets_dir, fixed_path: se.clean_mode }
}

/// The names a service's process gets from K2 itself in clean mode (and,
/// by name, in the 0.46.1 wall): the base names, `PORT`, the identity
/// block. Grants and `.env.k2` names are added by the caller.
pub fn clean_mode_names() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = BASE_EXACT.to_vec();
    v.extend([
        "PORT",
        APP_ID_ENV,
        "K2_HOST",
        "K2_PORT",
        crate::apps::process_engine::APP_NAME_ENV,
        "K2_GATEWAY_URL",
        crate::apps::origins::ORIGIN_ENV,
        crate::apps::origins::ORIGINS_ENV,
        super::spawn::SECRETS_DIR_ENV,
    ]);
    v
}

/// Does `name` belong to the clean base (exact names, `LC_*`, `K2_PUBLISH_*`)?
pub fn is_clean_base_name(name: &str) -> bool {
    clean_mode_names().contains(&name) || name.starts_with("LC_") || name.starts_with("K2_PUBLISH_")
}

/// What the vault grants one service at start: `.env.k2` values (literals
/// with non-sensitive names, `k2v://` references resolved) and the secrets
/// granted to the service, split by delivery. Values only leave here into
/// a child's env, a memory-backed secrets dir, or (the guarded engine) a
/// Podman secret.
#[derive(Default)]
pub struct Granted {
    pub env: Vec<(String, String)>,
    /// (file name, bytes) for `K2_SECRETS_DIR`.
    pub files: Vec<(String, Vec<u8>)>,
    /// `.env.k2` references the vault refused (`KEY: code`; names only).
    pub refused: Vec<String>,
}

/// Resolve the vault half of a clean start (`.env.k2` in `cwd`, then the
/// service's granted secrets).
pub fn granted(service_id: &str, workspace_id: &str, cwd: &Path) -> Granted {
    let mut out = Granted::default();
    let se = get(service_id);
    let Ok(vault) = super::get() else { return out };
    // `.env.k2`: references resolved, literals passed through.
    let env_file = se.env_file.clone().unwrap_or_else(|| ".env.k2".into());
    if let Ok(p) = crate::vault_use_routes::jailed_path(cwd, &env_file) {
        if let Ok(text) = std::fs::read_to_string(&p) {
            for (k, v) in parse_env_k2(&text) {
                if k2_vault::delivery::validate_env_name(&k).is_err() {
                    k2_core::log_debug!("[vault] publish {service_id}: {k} isn't allowed as an env name");
                    continue;
                }
                if k2_vault::refs::VaultRef::is_ref(&v) {
                    match resolve_ref(&vault, &v, service_id, workspace_id) {
                        Ok(val) => out.env.push((k, val)),
                        Err(e) => {
                            k2_core::log_debug!("WARN [vault] publish {service_id}: {k} not delivered: {} ({})", e.code, e.hint);
                            out.refused.push(format!("{k}: {}", e.code));
                        }
                    }
                } else if !k2_vault::delivery::is_sensitive_name(&k) {
                    out.env.push((k, v));
                } else {
                    k2_core::log_debug!("[vault] publish {service_id}: {k} holds a value in .env.k2; use a k2v:// reference");
                }
            }
        }
    }
    // Secrets granted to this service.
    let who = Requester::PublishedService { id: service_id.to_string(), workspace_id: workspace_id.to_string() };
    let items = vault.spawn_items(&who).unwrap_or_default();
    for it in items {
        // ssh-agent delivery (item 3) is for sessions and job runs only.
        if it.delivery == Delivery::Agent {
            k2_core::log_debug!("[vault] publish {service_id}: {} is agent delivery (sessions only); not delivered", it.name);
            continue;
        }
        let use_ = if it.delivery == Delivery::Env { Use::Env } else { Use::File };
        let Ok(r) = vault.resolve(&Selector::Id(it.secret_id.clone()), use_, &who) else { continue };
        if it.delivery == Delivery::Env {
            if let (Some(n), Ok(s)) = (it.env_name.clone(), std::str::from_utf8(r.value.expose())) {
                out.env.push((n, s.to_string()));
            }
            continue;
        }
        out.files.push((it.file_name.clone().unwrap_or_else(|| it.name.clone()), r.value.expose().to_vec()));
    }
    record_refused(service_id, &out.refused);
    out
}

/// What a service's `DATABASE_URL` is at its next start
/// (prd-app-pg-choice-dualrun-v1 PA1): the same sources a clean start
/// reads (its `.env.k2` references, then its granted env secrets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbUrl {
    /// Neither source names one.
    Absent,
    /// The value the service gets.
    Value(String),
    /// A source names one K2 can't read now (a broken reference, a sealed
    /// vault): the service may not get it either.
    Unreadable,
}

pub const DATABASE_URL: &str = "DATABASE_URL";

/// [`DbUrl`] for one service, read the way [`granted`] reads it.
pub fn database_url(service_id: &str, workspace_id: &str, cwd: &Path) -> DbUrl {
    let g = granted(service_id, workspace_id, cwd);
    if let Some((_, v)) = g.env.iter().rev().find(|(k, _)| k == DATABASE_URL) {
        return DbUrl::Value(v.clone());
    }
    // Named but not resolved: a reference that didn't resolve, or a grant.
    let se = get(service_id);
    let env_file = se.env_file.clone().unwrap_or_else(|| ".env.k2".into());
    let in_file = crate::vault_use_routes::jailed_path(cwd, &env_file)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|t| parse_env_k2(&t).iter().any(|(k, v)| k == DATABASE_URL && k2_vault::refs::VaultRef::is_ref(v)));
    let granted_one = super::get().ok().is_some_and(|vault| {
        let who = Requester::PublishedService { id: service_id.to_string(), workspace_id: workspace_id.to_string() };
        vault.spawn_items(&who).unwrap_or_default().iter().any(|it| it.delivery == Delivery::Env && it.env_name.as_deref() == Some(DATABASE_URL))
    });
    if in_file || granted_one {
        DbUrl::Unreadable
    } else {
        DbUrl::Absent
    }
}

/// R8 review fix 6: keep this start's refused references (names and codes
/// only) where the service's status shows them.
fn record_refused(service_id: &str, refused: &[String]) {
    let _g = LOCK.lock();
    let mut m = load();
    let had = m.services.get(service_id).map(|e| e.refused_refs.clone()).unwrap_or_default();
    if had == refused {
        return;
    }
    let default_mode = if m.seeded { "clean" } else { "legacy" };
    let e = m.services.entry(service_id.to_string()).or_insert_with(|| ServiceEnv { mode: default_mode.into(), ..Default::default() });
    e.refused_refs = refused.to_vec();
    if let Err(e) = save(&m) {
        k2_core::log_debug!("[vault] publish {service_id}: refused list not saved: {e}");
    }
}

/// The references refused at the service's last start (names and codes).
pub fn refused_refs(service_id: &str) -> Vec<String> {
    load().services.get(service_id).map(|e| e.refused_refs.clone()).unwrap_or_default()
}

/// The vault half of a clean start: `.env.k2` references (in `cwd`) and the
/// secrets granted to the service. The App runtime's process engine builds
/// its own clean base and calls this (prd-k2-app-runtime-v1 §4.2 item 9).
/// Returns the secrets dir to remove when the instance stops.
pub fn apply_granted(cmd: &mut Command, service_id: &str, workspace_id: &str, cwd: &Path) -> Option<PathBuf> {
    let g = granted(service_id, workspace_id, cwd);
    for (k, v) in &g.env {
        cmd.env(k, v);
    }
    let mut dir: Option<PathBuf> = None;
    for (name, bytes) in &g.files {
        if dir.is_none() {
            let Some(root) = super::spawn::secrets_root() else {
                k2_core::log_debug!("[vault] publish {service_id}: no memory filesystem for file secrets");
                continue;
            };
            dir = super::spawn::make_session_dir(&root, &format!("svc-{service_id}")).ok();
        }
        if let Some(d) = &dir {
            let _ = super::spawn::write_secret_file(d, name, bytes);
        }
    }
    if let Some(d) = &dir {
        cmd.env(super::spawn::SECRETS_DIR_ENV, d);
    }
    dir
}

/// One `k2v://` reference in a service's `.env.k2` → its value (R8,
/// prd-env-to-vault-v1). The service is the requester, never the daemon:
/// its own workspace's secrets resolve as before; a server secret needs a
/// grant to this service (`k2 publish secret add`, `service:<label>`) or
/// to its workspace (`ws:<name>`, `ws:*`) at env delivery. Another
/// workspace's or a chat's secret is refused as before. Refusals are
/// audited under `published_service:<id>`.
fn resolve_ref(vault: &k2_vault::LocalVault, r: &str, service_id: &str, workspace_id: &str) -> Result<String, k2_vault::VaultError> {
    let sel = super::selector_for_ref(r)?;
    let scope_ok = match &sel {
        Selector::Name { scope, .. } => match scope.kind {
            k2_vault::ScopeKind::Server => true,
            k2_vault::ScopeKind::Workspace => scope.id == workspace_id,
            _ => false,
        },
        _ => false,
    };
    if !scope_ok {
        return Err(k2_vault::VaultError::forbidden("not_this_workspace", "a service can use its own workspace's secrets and server secrets granted to it"));
    }
    let who = Requester::PublishedService { id: service_id.to_string(), workspace_id: workspace_id.to_string() };
    let v = vault.resolve_ref_env(&sel, &who)?;
    String::from_utf8(v.value.expose().to_vec()).map_err(|_| k2_vault::VaultError::bad_request("not_text", "env values must be text"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(cmd: &Command, key: &str) -> Option<String> {
        cmd.get_envs()
            .find(|(k, _)| *k == std::ffi::OsStr::new(key))
            .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    /// S24-9: a `--cmd` service learns its App id and the daemon address in
    /// both env modes (clean and legacy).
    #[test]
    fn app_identity_is_injected_in_both_modes() {
        crate::test_support::with_temp_home(|| {
            let dir = std::env::temp_dir();
            for mode in ["clean", "legacy"] {
                let id = uuid::Uuid::new_v4().to_string();
                set(&id, Some(mode), None).expect("mode");
                let mut cmd = Command::new("true");
                let _ = apply(&mut cmd, &id, "ws-1", &dir, 4321);
                assert_eq!(env_of(&cmd, APP_ID_ENV).as_deref(), Some(id.as_str()), "{mode}");
                assert_eq!(env_of(&cmd, "K2_HOST").as_deref(), Some("127.0.0.1"), "{mode}");
            }
            let v = app_identity_env("svc-1", 5555);
            assert!(v.contains(&("K2_PORT", "5555".to_string())), "{v:?}");
            assert!(
                !app_identity_env("svc-1", 0).iter().any(|(k, _)| *k == "K2_PORT"),
                "no port known → no K2_PORT"
            );
        });
    }

    /// 0.46.0 grace: every service gets K2_APP_NAME and K2_GATEWAY_URL;
    /// clean mode fixes PATH to the system folders and gives the service
    /// its own HOME (so `$HOME/.k2/…` no longer finds the daemon's files),
    /// and the spawner is told not to enrich PATH. Turning it on also
    /// switches a legacy service to clean; legacy turns clean mode off.
    #[test]
    fn clean_mode_fixes_path_and_home_and_names_the_gateway() {
        crate::test_support::with_temp_home(|| {
            let id = uuid::Uuid::new_v4().to_string();
            set(&id, Some("legacy"), None).expect("legacy");
            let dir = std::env::temp_dir();
            let mut cmd = Command::new("true");
            let a = apply_service(&mut cmd, &id, "jobs", "ws-1", &dir, 4321);
            assert!(!a.fixed_path, "legacy keeps today's PATH");
            assert_eq!(env_of(&cmd, "K2_APP_NAME").as_deref(), Some("jobs"));
            let on = set_clean_mode(&id, true).expect("on");
            assert!(on.clean_mode && on.mode == "clean", "{on:?}");
            assert_eq!(clean_mode_ids(), vec![id.clone()]);
            let mut cmd = Command::new("true");
            let a = apply_service(&mut cmd, &id, "jobs", "ws-1", &dir, 4321);
            assert!(a.fixed_path);
            assert_eq!(env_of(&cmd, "PATH").as_deref(), Some(k2_core::app_grace::scan::CLEAN_PATH));
            let home = env_of(&cmd, "HOME").expect("HOME");
            assert_eq!(std::path::PathBuf::from(&home), service_home(&id));
            assert!(std::path::Path::new(&home).is_dir());
            assert!(home.starts_with(&k2_core::paths::k2_home().join("publish-homes").display().to_string()), "{home}");
            assert_eq!(env_of(&cmd, "K2_APP_ID").as_deref(), Some(id.as_str()));
            let gw = service_identity_env(&id, "jobs", 5555);
            assert!(gw.contains(&("K2_GATEWAY_URL", "http://127.0.0.1:5555".to_string())), "{gw:?}");
            assert!(is_clean_base_name("K2_GATEWAY_URL") && is_clean_base_name("LC_ALL") && !is_clean_base_name("DATABASE_URL"));
            let off = set_clean_mode(&id, false).expect("off");
            assert!(!off.clean_mode && off.mode == "clean", "off never gives the inherited env back: {off:?}");
            set_clean_mode(&id, true).expect("on again");
            let legacy = set(&id, Some("legacy"), None).expect("legacy");
            assert!(!legacy.clean_mode, "inheriting again ends clean mode");
        });
    }

    #[test]
    fn env_k2_parsing() {
        let v = parse_env_k2("# c\nexport A=1\nSTRIPE_KEY=\"k2v://ws/sales/STRIPE_KEY\"\n\nB='x y'\n");
        assert_eq!(v, vec![("A".into(), "1".into()), ("STRIPE_KEY".into(), "k2v://ws/sales/STRIPE_KEY".into()), ("B".into(), "x y".into())]);
    }

    /// Rosson 2026-10-10: only a service explicitly kept on the inherited
    /// environment carries the 0.46.0 deprecation notice; switching it to
    /// clean removes the notice, and a new service never has one.
    #[test]
    fn only_legacy_services_carry_the_046_deprecation_notice() {
        crate::test_support::with_temp_home(|| {
            let old = uuid::Uuid::new_v4().to_string();
            let new = uuid::Uuid::new_v4().to_string();
            set(&old, Some("legacy"), None).expect("legacy");
            assert!(is_legacy(&old));
            assert!(!is_legacy(&new), "a service with no entry is not legacy");
            assert_eq!(legacy_ids(), vec![old.clone()]);
            let n = legacy_env_notice("shop");
            assert!(n.contains("0.46.4") && n.contains("k2 publish env shop clean") && n.contains("k2 app clean-mode shop on"), "{n}");
            set(&old, Some("clean"), None).expect("clean");
            assert!(!is_legacy(&old));
            assert!(legacy_ids().is_empty());
        });
    }

    #[test]
    fn clean_services_get_base_env_and_bound_secrets_only() {
        crate::test_support::with_temp_home(|| {
            use k2_vault::{Actor, Consumer, GrantRequest, Kind, Namespace, PutRequest, Scope, SecretBytes};
            let v = crate::vault::ensure_test_vault();
            let ws = uuid::Uuid::new_v4().to_string();
            let svc = uuid::Uuid::new_v4().to_string();
            let t = k2_vault::fsutil::TestDir::new("svc");
            let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws.clone()), "SVC_KEY", Kind::ApiKey);
            r.hosts = vec!["api.example.com".into()];
            v.put(r, SecretBytes::from_slice(b"svc-secret-value-1"), &Actor::Owner).expect("put");
            v.grant(
                GrantRequest {
                    selector: k2_vault::Selector::user(Scope::workspace(ws.clone()), "SVC_KEY"),
                    consumer: Consumer::published_service(svc.clone()),
                    delivery: Delivery::Env,
                    env_name: None,
                    file_name: None,
                    hosts_narrowed: None,
                    expires_at: None,
                    version: None,
                },
                &Actor::Owner,
            )
            .expect("grant");
            std::fs::write(t.path().join(".env.k2"), format!("MODE=prod\nDB_PASSWORD=literal-not-allowed\n")).expect("env.k2");
            set(&svc, Some("clean"), None).expect("clean");
            let leak = k2_core::test_env::EnvVar::set("K2_VAULT_TEST_LEAK", "should-not-pass");
            let mut cmd = Command::new("/usr/bin/env");
            let _ = apply(&mut cmd, &svc, &ws, t.path(), 4321);
            let out = cmd.output().expect("run env");
            drop(leak);
            let envs: HashMap<String, String> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
                .collect();
            assert_eq!(envs.get("SVC_KEY").map(String::as_str), Some("svc-secret-value-1"));
            assert_eq!(envs.get("PORT").map(String::as_str), Some("4321"));
            assert_eq!(envs.get("MODE").map(String::as_str), Some("prod"));
            assert!(!envs.contains_key("DB_PASSWORD"), "a literal under a sensitive name never passes");
            assert!(!envs.contains_key("K2_VAULT_TEST_LEAK"), "the daemon's own env doesn't leak into a clean service");
            // Legacy services inherit (and their names are recorded).
            set(&svc, Some("legacy"), None).expect("legacy");
            let mut cmd2 = Command::new("true");
            let _ = apply(&mut cmd2, &svc, &ws, t.path(), 4321);
            assert!(!get(&svc).inherited_names.is_empty());
        });
    }
}
