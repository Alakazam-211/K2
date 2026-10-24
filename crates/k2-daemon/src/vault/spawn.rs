//! Env and file delivery at spawn (§9.4, KV19, VS10).
//!
//! Every agent spawn path (`spawn.rs`, `v2_spawn.rs`, the legacy
//! terminal create) calls [`materialize`] right after the passport is
//! minted. Only secrets whose delivery is `file` or `env` for this
//! session arrive; fetch/proxy secrets never touch the environment.
//!
//! Files go to a memory-backed directory, never `/tmp` or the home folder.
//! Linux, the same order as `k2_core::runtime_dir` ([`linux_secrets_candidates`]):
//! `$K2_RUNTIME_DIR/secrets`, else the daemon unit's own dir
//! (`RuntimeDirectory=k2` → `$RUNTIME_DIRECTORY/secrets`, `/run/k2/secrets`;
//! systemd-logind never deletes it, akzm 2026-10-24), else
//! `$XDG_RUNTIME_DIR/k2/secrets`, `/run/k2/secrets`,
//! `/dev/shm/k2-<uid>/secrets`. macOS: the per-user `$TMPDIR/k2-secrets`
//! (private, 0700). No memory filesystem → file delivery is skipped
//! (`no_memory_fs`) and status says so.
//! Directories are made 0700 and must not exist yet; files are created
//! `O_CREAT|O_EXCL|O_NOFOLLOW` 0600. Removed when the session ends and
//! swept at boot.

use std::collections::HashMap;
use std::fs::{DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use k2_vault::{Delivery, Requester, SecretStore, Selector, Use};
use parking_lot::Mutex;

pub const SECRETS_DIR_ENV: &str = "K2_SECRETS_DIR";

/// Live session → its secrets dir (removed at session end).
static DIRS: Mutex<Vec<(String, PathBuf)>> = Mutex::new(Vec::new());

/// The memory-backed root for secrets dirs, or `None` (`no_memory_fs`).
pub fn secrets_root() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let uid = unsafe { libc::geteuid() };
        let env = |k: &str| std::env::var_os(k).filter(|x| !x.is_empty()).map(PathBuf::from);
        let candidates = linux_secrets_candidates(
            env(k2_core::runtime_dir::RUNTIME_DIR_ENV).as_deref(),
            k2_core::runtime_dir::systemd_runtime_directory().as_deref(),
            env("XDG_RUNTIME_DIR").as_deref(),
            uid,
            k2_core::runtime_dir::looks_like_test_process(),
        );
        for c in candidates {
            let parent_ok = c.parent().is_some_and(|p| p.is_dir());
            if !parent_ok {
                continue;
            }
            if ensure_private(&c).is_ok() {
                return Some(c.join("secrets"));
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        let t = std::env::var_os("TMPDIR").filter(|x| !x.is_empty())?;
        let dir = PathBuf::from(t).join("k2-secrets");
        Some(dir)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Linux: the runtime dir and the secrets root's dir, when systemd-logind
/// can delete them under the running daemon (no linger; see
/// `k2_core::runtime_dir::logind_risk`). `k2 vault doctor` and the boot
/// WARN read it.
pub fn runtime_logind_risk() -> Option<k2_core::runtime_dir::LogindRisk> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let extra: Vec<PathBuf> = secrets_root().and_then(|r| r.parent().map(Path::to_path_buf)).into_iter().collect();
    k2_core::runtime_dir::logind_risk(&extra)
}

/// Boot: one WARN line when [`runtime_logind_risk`] finds a dir at risk.
pub fn warn_logind_risk_at_boot() {
    if let Some(r) = runtime_logind_risk() {
        k2_core::log_debug!("[runtime] WARN: {}", r.message());
    }
}

/// Pure Linux order for the dir that holds `secrets/`: the override alone,
/// else the unit's `$RUNTIME_DIRECTORY` (never for a test process: an
/// agent inherits it from the daemon unit), then the pre-0.46.4 chain.
pub fn linux_secrets_candidates(
    override_dir: Option<&Path>,
    systemd_dir: Option<&Path>,
    xdg_runtime_dir: Option<&Path>,
    uid: u32,
    is_test: bool,
) -> Vec<PathBuf> {
    if let Some(d) = override_dir {
        return vec![d.to_path_buf()];
    }
    let mut v = Vec::new();
    if let Some(s) = systemd_dir.filter(|_| !is_test) {
        v.push(s.to_path_buf());
    }
    if let Some(x) = xdg_runtime_dir {
        v.push(x.join("k2"));
    }
    v.push(PathBuf::from("/run/k2"));
    v.push(PathBuf::from(format!("/dev/shm/k2-{uid}")));
    v
}

/// Make `dir` 0700 owned by us (create if missing); refuse symlinks and
/// foreign owners.
fn ensure_private(dir: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(m) => {
            let me = unsafe { libc::geteuid() };
            if m.file_type().is_symlink() || !m.is_dir() || m.uid() != me {
                return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "unsafe secrets dir"));
            }
            if m.permissions().mode() & 0o077 != 0 {
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DirBuilder::new().mode(0o700).create(dir),
        Err(e) => Err(e),
    }
}

/// Write one file `O_CREAT|O_EXCL|O_NOFOLLOW` 0600.
pub fn write_secret_file(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    k2_vault::delivery::validate_file_name(name)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.hint))?;
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join(name))?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Make a fresh per-session dir under `root` (must not exist yet).
pub fn make_session_dir(root: &Path, session: &str) -> std::io::Result<PathBuf> {
    ensure_private(root)?;
    let tag: String = session.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
    let dir = root.join(format!("{tag}-{}", k2_vault::crypto::base62(8)));
    DirBuilder::new().mode(0o700).create(&dir)?;
    let m = std::fs::symlink_metadata(&dir)?;
    let me = unsafe { libc::geteuid() };
    if m.file_type().is_symlink() || m.uid() != me || m.permissions().mode() & 0o077 != 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "secrets dir check failed"));
    }
    Ok(dir)
}

/// What a spawn received (names only; for logs and tests).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Materialized {
    pub env_names: Vec<String>,
    pub files: Vec<String>,
    pub dir: Option<PathBuf>,
    pub skipped: Vec<String>,
}

/// Add this session's env/file secrets to `env`. `sandboxed` cells get env
/// delivery only (a host directory isn't visible inside the cell).
pub fn materialize(env: &mut HashMap<String, String>, workspace_id: &str, session_id: &str, sandboxed: bool) -> Materialized {
    let mut out = Materialized::default();
    let ws = workspace_id.trim();
    if ws.is_empty() {
        return out;
    }
    let Ok(vault) = super::get() else { return out };
    // `k2v://` references in env the owner configured (preset env, launch
    // env): resolved here, only for secrets whose ceiling allows env.
    let refs: Vec<(String, String)> = env
        .iter()
        .filter(|(_, v)| k2_vault::refs::VaultRef::is_ref(v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (k, r) in refs {
        match resolve_env_ref(&vault, &r) {
            Ok(v) => {
                env.insert(k.clone(), v);
                out.env_names.push(k);
            }
            Err(e) => {
                env.remove(&k);
                out.skipped.push(format!("{k}: {}", e.code));
            }
        }
    }
    let who = Requester::Session { workspace_id: ws.to_string(), session_id: Some(session_id.to_string()) };
    deliver_items(&vault, env, &who, session_id, sandboxed, &mut out);
    if let Some(d) = &out.dir {
        // The session registers only after its PTY is up (seconds when
        // k2-ptyd launches); the 60 s sweep must not take the dir first.
        crate::session_liveness::note_spawning(session_id);
        DIRS.lock().push((session_id.to_string(), d.clone()));
    }
    out
}

/// Env and file delivery for one `k2 job` run (prd-vault-gaps-v1 item 5):
/// the same code path as a session ([`deliver_items`]), for
/// `Requester::Job`, so a run gets exactly its `job:<name>` grants (Q13).
///
/// The run's dir is NOT in the session list: the 60 s `sweep_dead` (which
/// keys on live v2 sessions) can never remove it mid-run. The job runner
/// owns its lifecycle and calls [`job_ended`] when the run ends; a daemon
/// restart sweeps it at boot like every other dir.
///
/// Item 3 (`agent` delivery) hook: a socket made by [`deliver_items`] for
/// an `agent` item lives inside `out.dir` (or is listed in `out` by the
/// builder who adds it), so a job gets it at start and [`job_ended`]
/// removes it with the dir.
pub fn materialize_job(env: &mut HashMap<String, String>, workspace_id: &str, job_id: &str, job_name: &str, run_sid: &str) -> Materialized {
    let mut out = Materialized::default();
    let Ok(vault) = super::get() else { return out };
    let who = Requester::Job { workspace_id: workspace_id.to_string(), job_id: job_id.to_string(), name: job_name.to_string() };
    deliver_items(&vault, env, &who, run_sid, false, &mut out);
    out
}

/// A job run ended: its secrets dir (files, and any agent socket) goes.
pub fn job_ended(m: &Materialized) {
    if let Some(d) = &m.dir {
        remove_dir(d);
    }
}

/// The shared delivery loop: every env/file item `who` gets, into `env`
/// and a fresh private dir tagged `tag`. Sessions and job runs both come
/// through here.
fn deliver_items(
    vault: &k2_vault::LocalVault,
    env: &mut HashMap<String, String>,
    who: &Requester,
    tag: &str,
    sandboxed: bool,
    out: &mut Materialized,
) {
    let session_id = tag;
    let items = match vault.spawn_items(who) {
        Ok(i) => i,
        Err(e) => {
            k2_core::log_debug!("[vault] spawn items for {}: {}", who.label(), e.code);
            return;
        }
    };
    if items.is_empty() {
        return;
    }
    let mut dir: Option<PathBuf> = None;
    for it in items {
        let use_ = if it.delivery == Delivery::Env { Use::Env } else { Use::File };
        let r = match vault.resolve(&Selector::Id(it.secret_id.clone()), use_, who) {
            Ok(r) => r,
            Err(e) => {
                out.skipped.push(format!("{}: {}", it.name, e.code));
                continue;
            }
        };
        match it.delivery {
            Delivery::Env => {
                let Some(name) = it.env_name.clone() else { continue };
                match std::str::from_utf8(r.value.expose()) {
                    Ok(s) if !s.contains('\0') => {
                        env.insert(name.clone(), s.to_string());
                        out.env_names.push(name);
                    }
                    _ => out.skipped.push(format!("{}: not text, use file delivery", it.name)),
                }
            }
            _ => {
                if sandboxed {
                    out.skipped.push(format!("{}: file delivery isn't available in a sandboxed cell", it.name));
                    continue;
                }
                if dir.is_none() {
                    let Some(root) = secrets_root() else {
                        out.skipped.push(format!("{}: no_memory_fs", it.name));
                        continue;
                    };
                    match make_session_dir(&root, session_id) {
                        Ok(d) => dir = Some(d),
                        Err(e) => {
                            out.skipped.push(format!("{}: secrets dir ({})", it.name, e.kind()));
                            continue;
                        }
                    }
                }
                let d = dir.as_ref().expect("dir set above");
                let fname = it.file_name.clone().unwrap_or_else(|| it.name.clone());
                match write_secret_file(d, &fname, r.value.expose()) {
                    Ok(()) => out.files.push(fname),
                    Err(e) => out.skipped.push(format!("{}: {}", it.name, e.kind())),
                }
            }
        }
    }
    if let Some(d) = &dir {
        env.insert(SECRETS_DIR_ENV.to_string(), d.display().to_string());
        out.dir = Some(d.clone());
    }
    if !out.env_names.is_empty() || !out.files.is_empty() || !out.skipped.is_empty() {
        k2_core::log_debug!(
            "[vault] spawn {session_id}: env {:?} files {:?} skipped {:?}",
            out.env_names,
            out.files,
            out.skipped
        );
    }
}

/// A `k2v://` reference in owner-set env → its value, when the secret's
/// ceiling allows env delivery.
fn resolve_env_ref(vault: &k2_vault::LocalVault, r: &str) -> Result<String, k2_vault::VaultError> {
    let sel = super::selector_for_ref(r)?;
    let m = vault.meta(&sel, &k2_vault::Actor::System("preset-env".into()))?;
    if m.delivery_ceiling.level() < Delivery::Env.level() {
        return Err(k2_vault::VaultError::forbidden("delivery_above_ceiling", "this secret can't be delivered as env"));
    }
    let v = vault.resolve(&Selector::Id(m.id), Use::System, &Requester::System("preset-env".into()))?;
    String::from_utf8(v.value.expose().to_vec())
        .map_err(|_| k2_vault::VaultError::bad_request("not_text", "env values must be text"))
}

/// Session ended: its secrets dir and placeholders go.
pub fn session_ended(session_id: &str) {
    let mine: Vec<PathBuf> = {
        let mut g = DIRS.lock();
        let (gone, keep): (Vec<_>, Vec<_>) = g.drain(..).partition(|(s, _)| s == session_id);
        *g = keep;
        gone.into_iter().map(|(_, d)| d).collect()
    };
    for d in mine {
        remove_dir(&d);
    }
    if let Ok(v) = super::get() {
        let _ = v.placeholders_drop_session(session_id);
    }
}

fn remove_dir(d: &Path) {
    if let Ok(rd) = std::fs::read_dir(d) {
        for e in rd.flatten() {
            let _ = super::adopt::remove_old_copy(&e.path());
        }
    }
    let _ = std::fs::remove_dir(d);
}

/// Boot: remove every secrets dir except those of agent sessions that
/// survived the restart in k2-ptyd (PRD prd-k2-ptyd-v1 P36: B0's running
/// set, each with the `secrets_dir` its spawn recorded). Those are kept
/// and re-registered in `DIRS`, so they go when their session ends.
/// Without k2-ptyd (or with nothing running in it) nothing survives a
/// restart and every dir goes, as before.
pub fn sweep_at_boot() {
    let keep: Vec<(String, PathBuf)> = surviving_dirs();
    sweep_at_boot_keeping(&keep);
}

/// The `(session id, secrets dir)` pairs B0 found running in ptyd.
fn surviving_dirs() -> Vec<(String, PathBuf)> {
    #[cfg(unix)]
    {
        k2_core::terminal::ptyd::adopt::running()
            .into_iter()
            .filter_map(|s| {
                let dir = s.meta.get("secrets_dir")?.as_str()?.to_string();
                Some((s.sid, PathBuf::from(dir)))
            })
            .collect()
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

/// [`sweep_at_boot`] with an explicit keep set. A kept dir must be a
/// direct child of the secrets root named for its session (`<tag>-<8>`,
/// [`make_session_dir`]); anything else in `keep` is ignored.
pub fn sweep_at_boot_keeping(keep: &[(String, PathBuf)]) {
    let Some(root) = secrets_root() else { return };
    sweep_root_keeping(&root, keep);
}

fn sweep_root_keeping(root: &Path, keep: &[(String, PathBuf)]) {
    let root = root.to_path_buf();
    let kept: Vec<(String, PathBuf)> = keep
        .iter()
        .filter(|(sid, dir)| {
            let tag: String = sid.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
            dir.parent() == Some(root.as_path())
                && dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&format!("{tag}-")))
        })
        .cloned()
        .collect();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()) {
                let p = e.path();
                if kept.iter().any(|(_, d)| d == &p) {
                    continue;
                }
                remove_dir(&p);
            }
        }
    }
    let mut g = DIRS.lock();
    for (sid, dir) in kept {
        if dir.is_dir() && !g.iter().any(|(s, d)| s == &sid && d == &dir) {
            g.push((sid, dir));
        }
    }
}

/// Periodic: drop dirs whose session is gone. `live` is
/// [`crate::session_liveness`]'s answer (registered, waiting for adoption,
/// or still being spawned).
pub fn sweep_dead(live: &dyn Fn(&str) -> bool) {
    let dead: Vec<String> = DIRS.lock().iter().filter(|(s, _)| !live(s)).map(|(s, _)| s.clone()).collect();
    for s in dead {
        session_ended(&s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// k2-ptyd P36: boot keeps the secrets dir of a session that survived
    /// in k2-ptyd (and re-registers it so it goes with the session); every
    /// other dir goes; a keep entry outside the root, or not named for its
    /// session, is ignored.
    #[test]
    fn boot_sweep_keeps_only_surviving_sessions_dirs() {
        let root = std::env::temp_dir().join(format!("k2-secrets-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sid = "4f6e9a1c-1111-4222-8333-944455556666";
        let kept = make_session_dir(&root, sid).unwrap();
        write_secret_file(&kept, "token", b"x").unwrap();
        let gone = make_session_dir(&root, "0b4d1f7e-dead-4c66-9d11-3a3e8f6a7c21").unwrap();
        let foreign = make_session_dir(&root, "other-session").unwrap();
        let outside = std::env::temp_dir().join(format!("k2-secrets-outside-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        sweep_root_keeping(
            &root,
            &[(sid.to_string(), kept.clone()), (sid.to_string(), foreign.clone()), (sid.to_string(), outside.clone())],
        );
        assert!(kept.join("token").is_file(), "the survivor's dir and file stay");
        assert!(!gone.exists(), "a dead session's dir goes");
        assert!(!foreign.exists(), "a dir not named for the kept session goes");
        assert!(outside.exists(), "nothing outside the root is touched");
        assert!(DIRS.lock().iter().any(|(s, d)| s == sid && d == &kept), "re-registered for session end");
        session_ended(sid);
        assert!(!kept.exists(), "goes when the session ends");
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }

    /// KV19: every production spawn path that mints a passport also
    /// delivers vault secrets. A new spawn entry point that skips
    /// `materialize` fails here.
    #[test]
    fn every_spawn_path_materializes_vault_secrets() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root.clone()];
        let mut checked = 0;
        let mut missing = vec![];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).expect("read dir") {
                let p = e.expect("entry").path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                    continue;
                }
                let rel = p.strip_prefix(&root).expect("rel").to_string_lossy().to_string();
                if rel == "session_token.rs" {
                    continue;
                }
                let src = std::fs::read_to_string(&p).expect("read");
                // Whole file: only session_token.rs's own tests mint in tests.
                let prod = src.as_str();
                let mints = prod.matches("prepare_agent_spawn_env(").count();
                if mints == 0 {
                    continue;
                }
                checked += 1;
                let delivers = prod.matches("vault::spawn::materialize(").count();
                if delivers < mints {
                    missing.push(format!("{rel}: {mints} passport mint(s), {delivers} materialize call(s)"));
                }
            }
        }
        assert!(checked >= 3, "expected spawn.rs, v2_spawn.rs and the legacy terminal path; saw {checked}");
        assert!(missing.is_empty(), "spawn paths without vault delivery: {missing:?}");
    }

    #[test]
    fn env_and_file_secrets_reach_a_spawn_and_fetch_ones_do_not() {
        use k2_vault::{Actor, Kind, Namespace, PutRequest, Scope, SecretBytes};
        let v = super::super::ensure_test_vault();
        let ws = uuid::Uuid::new_v4().to_string();
        let put = |name: &str, d: Delivery, val: &str| {
            let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws.clone()), name, Kind::Generic);
            r.default_delivery = Some(d);
            r.hosts = vec!["api.example.com".into()];
            v.put(r, SecretBytes::from_slice(val.as_bytes()), &Actor::Owner).expect("put");
        };
        put("AS_ENV", Delivery::Env, "env-value-1");
        put("AS_FILE", Delivery::File, "file-value-2");
        put("AS_FETCH", Delivery::Fetch, "fetch-value-3");
        let mut env = HashMap::new();
        let sid = uuid::Uuid::new_v4().to_string();
        let out = materialize(&mut env, &ws, &sid, false);
        assert_eq!(env.get("AS_ENV").map(String::as_str), Some("env-value-1"));
        assert!(!env.values().any(|x| x.contains("fetch-value-3")), "fetch secrets never reach env");
        assert!(!env.contains_key("AS_FETCH"));
        if let Some(dir) = &out.dir {
            assert_eq!(std::fs::read(dir.join("AS_FILE")).expect("file"), b"file-value-2");
            assert_eq!(env.get(SECRETS_DIR_ENV).map(String::as_str), Some(dir.display().to_string().as_str()));
            session_ended(&sid);
            assert!(!dir.exists(), "removed at session end");
        } else {
            assert!(out.skipped.iter().any(|s| s.contains("no_memory_fs")), "{out:?}");
        }
        // Another workspace gets nothing.
        let mut env2 = HashMap::new();
        let out2 = materialize(&mut env2, &uuid::Uuid::new_v4().to_string(), "s2", false);
        assert!(env2.is_empty() && out2.files.is_empty(), "{out2:?}");
        // A sandboxed cell gets env only.
        let mut env3 = HashMap::new();
        let out3 = materialize(&mut env3, &ws, "s3", true);
        assert!(out3.dir.is_none());
        assert_eq!(env3.get("AS_ENV").map(String::as_str), Some("env-value-1"));
    }

    /// Item 5: a job run gets its `job:` grants (env + file) through the
    /// same delivery path, nothing from the workspace's implicit grants,
    /// and the 60 s sweep (live v2 sessions) never touches its dir.
    #[test]
    fn a_job_run_gets_its_own_grants_and_the_session_sweep_leaves_it_alone() {
        use k2_vault::{Actor, Consumer, GrantRequest, Kind, Namespace, PutRequest, Scope, SecretBytes};
        let v = super::super::ensure_test_vault();
        let ws = uuid::Uuid::new_v4().to_string();
        let job_id = uuid::Uuid::new_v4().to_string();
        let put = |name: &str, d: Delivery, val: &str| {
            let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws.clone()), name, Kind::Generic);
            r.default_delivery = Some(d);
            r.hosts = vec!["api.example.com".into()];
            v.put(r, SecretBytes::from_slice(val.as_bytes()), &Actor::Owner).expect("put");
        };
        put("JOB_ENV", Delivery::Env, "job-env-value-1");
        put("JOB_FILE", Delivery::File, "job-file-value-2");
        put("WS_ONLY", Delivery::Env, "ws-only-value-3");
        let grant = |name: &str, d: Delivery| {
            v.grant(
                GrantRequest {
                    selector: k2_vault::Selector::user(Scope::workspace(ws.clone()), name),
                    consumer: Consumer::job(&ws, &job_id),
                    delivery: d,
                    env_name: None,
                    file_name: None,
                    hosts_narrowed: None,
                    expires_at: None,
                    version: None,
                },
                &Actor::Owner,
            )
            .expect("grant")
        };
        grant("JOB_ENV", Delivery::Env);
        grant("JOB_FILE", Delivery::File);
        let mut env = HashMap::new();
        let run_sid = format!("job-{}", uuid::Uuid::new_v4());
        let out = materialize_job(&mut env, &ws, &job_id, "collect", &run_sid);
        assert_eq!(env.get("JOB_ENV").map(String::as_str), Some("job-env-value-1"));
        assert!(!env.contains_key("WS_ONLY"), "no implicit workspace grant for a job: {env:?}");
        assert!(!env.values().any(|x| x.contains("ws-only-value-3")));
        match &out.dir {
            Some(dir) => {
                assert_eq!(std::fs::read(dir.join("JOB_FILE")).expect("file"), b"job-file-value-2");
                // The session sweep sees no live sessions at all: the job dir stays.
                sweep_dead(&|_| false);
                assert!(dir.join("JOB_FILE").exists(), "the session sweep never removes a job run's dir");
                job_ended(&out);
                assert!(!dir.exists(), "removed at run end");
            }
            None => assert!(out.skipped.iter().any(|s| s.contains("no_memory_fs")), "{out:?}"),
        }
        // Another job of the workspace gets nothing.
        let mut env2 = HashMap::new();
        let out2 = materialize_job(&mut env2, &ws, &uuid::Uuid::new_v4().to_string(), "other", "job-x");
        assert!(env2.is_empty() && out2.dir.is_none(), "{out2:?}");
    }

    #[test]
    fn secrets_root_order_matches_the_runtime_dir() {
        let p = PathBuf::from;
        let xdg = p("/run/user/1001");
        let unit = p("/run/k2");
        assert_eq!(
            linux_secrets_candidates(Some(Path::new("/x/run")), Some(&unit), Some(&xdg), 1001, false),
            vec![p("/x/run")],
            "override wins alone"
        );
        assert_eq!(
            linux_secrets_candidates(None, Some(&unit), Some(&xdg), 1001, false),
            vec![p("/run/k2"), p("/run/user/1001/k2"), p("/run/k2"), p("/dev/shm/k2-1001")],
            "the unit's dir first, then the old chain"
        );
        assert_eq!(
            linux_secrets_candidates(None, Some(Path::new("/run/other")), Some(&xdg), 1001, true),
            vec![p("/run/user/1001/k2"), p("/run/k2"), p("/dev/shm/k2-1001")],
            "a test process never takes the inherited unit dir"
        );
        assert_eq!(
            linux_secrets_candidates(None, None, None, 1001, false),
            vec![p("/run/k2"), p("/dev/shm/k2-1001")],
            "unset: the pre-0.46.4 fallbacks"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn secrets_root_follows_the_env() {
        let t = k2_vault::fsutil::TestDir::new("spawn-root");
        let over = t.path().join("over");
        let _o = k2_core::test_env::EnvVar::set(k2_core::runtime_dir::RUNTIME_DIR_ENV, &over);
        assert_eq!(secrets_root(), Some(over.join("secrets")), "K2_RUNTIME_DIR set");
        assert!(over.is_dir());
        assert_eq!(std::fs::metadata(&over).expect("m").permissions().mode() & 0o777, 0o700);
        drop(_o);
        let _u = k2_core::test_env::EnvVar::remove(k2_core::runtime_dir::RUNTIME_DIR_ENV);
        let unit = t.path().join("unit");
        let xdg = t.path().join("xdg");
        std::fs::create_dir_all(&xdg).expect("mkdir xdg");
        let _x = k2_core::test_env::EnvVar::set("XDG_RUNTIME_DIR", &xdg);
        let _r = k2_core::test_env::EnvVar::set(k2_core::runtime_dir::SYSTEMD_RUNTIME_DIRECTORY_ENV, &unit);
        let got = secrets_root();
        if k2_core::runtime_dir::looks_like_test_process() {
            assert_eq!(got, Some(xdg.join("k2").join("secrets")), "a test process skips the unit dir");
        } else {
            assert_eq!(got, Some(unit.join("secrets")), "RUNTIME_DIRECTORY set");
        }
    }

    #[test]
    fn session_dir_and_files_are_private_and_exclusive() {
        let t = k2_vault::fsutil::TestDir::new("spawn-dir");
        let root = t.path().join("secrets");
        let d = make_session_dir(&root, "sess-1").expect("dir");
        assert_eq!(std::fs::metadata(&d).expect("m").permissions().mode() & 0o777, 0o700);
        write_secret_file(&d, "STRIPE_KEY", b"v").expect("write");
        assert_eq!(std::fs::metadata(d.join("STRIPE_KEY")).expect("m").permissions().mode() & 0o777, 0o600);
        assert!(write_secret_file(&d, "STRIPE_KEY", b"v").is_err(), "no overwrite (O_EXCL)");
        assert!(write_secret_file(&d, "../x", b"v").is_err(), "no traversal");
        // A pre-made symlink in place of a file is refused.
        std::os::unix::fs::symlink(t.path().join("elsewhere"), d.join("LINKED")).expect("symlink");
        assert!(write_secret_file(&d, "LINKED", b"v").is_err());
        assert!(!t.path().join("elsewhere").exists(), "nothing written through the link");
        // A symlinked root is refused.
        let link_root = t.path().join("linkroot");
        std::os::unix::fs::symlink(t.path(), &link_root).expect("symlink");
        assert!(make_session_dir(&link_root, "s").is_err());
    }
}
