//! Env and file delivery at spawn (§9.4, KV19, VS10).
//!
//! Every agent spawn path (`spawn.rs`, `v2_spawn.rs`, the legacy
//! terminal create) calls [`materialize`] right after the passport is
//! minted. Only secrets whose delivery is `file` or `env` for this
//! session arrive; fetch/proxy secrets never touch the environment.
//!
//! Files go to a memory-backed directory, never `/tmp` or the home folder:
//! Linux `$XDG_RUNTIME_DIR/k2/secrets`, else `/run/k2/secrets`
//! (`RuntimeDirectory=k2`), else `/dev/shm/k2-<uid>/secrets`; macOS the
//! per-user `$TMPDIR/k2-secrets` (private, 0700). No memory filesystem →
//! file delivery is skipped (`no_memory_fs`) and status says so.
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
        let mut candidates: Vec<PathBuf> = vec![];
        if let Some(x) = std::env::var_os("XDG_RUNTIME_DIR").filter(|x| !x.is_empty()) {
            candidates.push(PathBuf::from(x).join("k2"));
        }
        candidates.push(PathBuf::from("/run/k2"));
        candidates.push(PathBuf::from(format!("/dev/shm/k2-{uid}")));
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
    let items = match vault.spawn_items(&who) {
        Ok(i) => i,
        Err(e) => {
            k2_core::log_debug!("[vault] spawn items for {ws}: {}", e.code);
            return out;
        }
    };
    if items.is_empty() {
        return out;
    }
    let mut dir: Option<PathBuf> = None;
    for it in items {
        let use_ = if it.delivery == Delivery::Env { Use::Env } else { Use::File };
        let r = match vault.resolve(&Selector::Id(it.secret_id.clone()), use_, &who) {
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
        DIRS.lock().push((session_id.to_string(), d.clone()));
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
    out
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

/// Boot: nothing survives a daemon restart (no PTY does).
pub fn sweep_at_boot() {
    if let Some(root) = secrets_root() {
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()) {
                    remove_dir(&e.path());
                }
            }
        }
    }
}

/// Periodic: drop dirs whose session is gone.
pub fn sweep_dead(live: &dyn Fn(&str) -> bool) {
    let dead: Vec<String> = DIRS.lock().iter().filter(|(s, _)| !live(s)).map(|(s, _)| s.clone()).collect();
    for s in dead {
        session_ended(&s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
