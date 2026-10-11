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
/// daemon's environment in 0.45.x, with this notice wherever the service
/// shows (status, `k2 publish list`, the Published drawer, `k2 vault
/// doctor`). New services start clean.
pub const LEGACY_ENV_REMOVED_IN: &str = "0.46.0";

/// The deprecation notice for one service (`name` = its publish name).
pub fn legacy_env_notice(name: &str) -> String {
    format!(
        "This service still inherits the K2 daemon's whole environment because it existed \
         before 0.45.4. That ends in K2 {LEGACY_ENV_REMOVED_IN}: it will then start with a \
         clean environment plus only the secrets granted to it. Switch now with \
         `k2 publish env {name} clean` (grant what it needs with `k2 publish secret add {name} …`)."
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
        }
    }
    if let Some(f) = env_file {
        e.env_file = f.filter(|x| !x.trim().is_empty());
    }
    let out = e.clone();
    save(&m)?;
    Ok(out)
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

/// Prepare the child's environment. Returns the secrets dir to remove
/// when the service stops (file deliveries).
pub fn apply(cmd: &mut Command, service_id: &str, workspace_id: &str, cwd: &Path, port: u16) -> Option<PathBuf> {
    let se = get(service_id);
    let identity = app_identity_env(service_id, crate::publish_runtime::loopback_port());
    if se.mode == "legacy" {
        let mut names: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
        names.sort();
        record_inherited(service_id, names);
        cmd.envs(identity);
        return None;
    }
    cmd.env_clear();
    cmd.envs(base_env());
    cmd.env("PORT", port.to_string());
    cmd.envs(identity);
    apply_granted(cmd, service_id, workspace_id, cwd)
}

/// The vault half of a clean start: `.env.k2` references (in `cwd`) and the
/// secrets granted to the service. The App runtime's process engine builds
/// its own clean base and calls this (prd-k2-app-runtime-v1 §4.2 item 9).
/// Returns the secrets dir to remove when the instance stops.
pub fn apply_granted(cmd: &mut Command, service_id: &str, workspace_id: &str, cwd: &Path) -> Option<PathBuf> {
    let se = get(service_id);
    let Ok(vault) = super::get() else { return None };
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
                    match resolve_ref(&vault, &v, workspace_id) {
                        Ok(val) => {
                            cmd.env(&k, val);
                        }
                        Err(e) => k2_core::log_debug!("[vault] publish {service_id}: {k}: {}", e.code),
                    }
                } else if !k2_vault::delivery::is_sensitive_name(&k) {
                    cmd.env(&k, v);
                } else {
                    k2_core::log_debug!("[vault] publish {service_id}: {k} holds a value in .env.k2; use a k2v:// reference");
                }
            }
        }
    }
    // Secrets granted to this service.
    let who = Requester::PublishedService { id: service_id.to_string(), workspace_id: workspace_id.to_string() };
    let items = vault.spawn_items(&who).unwrap_or_default();
    let mut dir: Option<PathBuf> = None;
    for it in items {
        let use_ = if it.delivery == Delivery::Env { Use::Env } else { Use::File };
        let Ok(r) = vault.resolve(&Selector::Id(it.secret_id.clone()), use_, &who) else { continue };
        if it.delivery == Delivery::Env {
            if let (Some(n), Ok(s)) = (it.env_name.clone(), std::str::from_utf8(r.value.expose())) {
                cmd.env(n, s);
            }
            continue;
        }
        if dir.is_none() {
            let Some(root) = super::spawn::secrets_root() else {
                k2_core::log_debug!("[vault] publish {service_id}: no memory filesystem for file secrets");
                continue;
            };
            dir = super::spawn::make_session_dir(&root, &format!("svc-{service_id}")).ok();
        }
        if let Some(d) = &dir {
            let _ = super::spawn::write_secret_file(d, it.file_name.as_deref().unwrap_or(&it.name), r.value.expose());
        }
    }
    if let Some(d) = &dir {
        cmd.env(super::spawn::SECRETS_DIR_ENV, d);
    }
    dir
}

fn resolve_ref(vault: &k2_vault::LocalVault, r: &str, workspace_id: &str) -> Result<String, k2_vault::VaultError> {
    let sel = super::selector_for_ref(r)?;
    let m = vault.meta(&sel, &k2_vault::Actor::System("publish".into()))?;
    // A service runs the owner's code for its own workspace: its own
    // workspace's secrets and server secrets, at most env.
    let ok_scope = match m.scope.kind {
        k2_vault::ScopeKind::Server => true,
        k2_vault::ScopeKind::Workspace => m.scope.id == workspace_id,
        _ => false,
    };
    if !ok_scope {
        return Err(k2_vault::VaultError::forbidden("not_this_workspace", "a service can use its own workspace's secrets and server secrets"));
    }
    if m.delivery_ceiling.level() < Delivery::Env.level() {
        return Err(k2_vault::VaultError::forbidden("delivery_above_ceiling", "this secret can't be delivered as env"));
    }
    let v = vault.resolve(&Selector::Id(m.id), Use::System, &Requester::System("publish".into()))?;
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
            assert!(n.contains("0.46.0") && n.contains("k2 publish env shop clean"), "{n}");
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
