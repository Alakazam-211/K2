//! A private, memory-backed runtime directory for files that briefly hold
//! secrets (the rendered frpc config, K2 vault §10.4). Never `/tmp` or the
//! home folder: Linux `$XDG_RUNTIME_DIR/k2`, else `/run/user/<uid>/k2`,
//! else `/dev/shm/k2-<uid>`; macOS the per-user `$TMPDIR/k2-runtime`
//! (private, 0700). `None` when none is usable.
//!
//! [`RUNTIME_DIR_ENV`] (`K2_RUNTIME_DIR`) names the directory outright
//! (in-process tests point it under their temp `$HOME`).
//!
//! **Test belt (2026-10-14 incident).** k2-core is linked WITHOUT
//! `cfg(test)` by integration tests and by every test daemon they spawn.
//! A test daemon with a temp `$HOME` but the inherited `TMPDIR` /
//! `XDG_RUNTIME_DIR` resolved the REAL user runtime dir, so its tunnel
//! release deleted the production `frpc.toml` (and its stop path
//! `pkill`ed the production frpc by config path); the production
//! supervisor then respawned frpc against a missing file for two hours.
//! A process that looks like a test ([`looks_like_test_process`]) is now
//! refused the real user runtime dir, loudly.

use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Explicit runtime-dir override (absolute path; created 0700 if its
/// parent exists).
pub const RUNTIME_DIR_ENV: &str = "K2_RUNTIME_DIR";

/// The env marker every test daemon harness sets (the agent-spawn guard's
/// shim dir; see `terminal::agent_spawn_guard::SHIM_DIR_ENV`).
const TEST_MARKER_ENV: &str = "K2_TEST_AGENT_SHIM_DIR";

fn ensure_private(dir: &Path) -> bool {
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    match std::fs::symlink_metadata(dir) {
        Ok(m) => {
            if m.file_type().is_symlink() || !m.is_dir() || m.uid() != me {
                return false;
            }
            if m.permissions().mode() & 0o077 != 0 {
                return std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).is_ok();
            }
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::DirBuilder::new().mode(0o700).create(dir).is_ok()
        }
        Err(_) => false,
    }
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).filter(|x| !x.is_empty()).map(PathBuf::from)
}

fn candidates() -> Vec<PathBuf> {
    if let Some(d) = env_path(RUNTIME_DIR_ENV) {
        return vec![d];
    }
    let mut candidates: Vec<PathBuf> = vec![];
    #[cfg(target_os = "linux")]
    {
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        if let Some(x) = env_path("XDG_RUNTIME_DIR") {
            candidates.push(x.join("k2"));
        }
        candidates.push(PathBuf::from(format!("/run/user/{uid}/k2")));
        candidates.push(PathBuf::from(format!("/dev/shm/k2-{uid}")));
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(t) = env_path("TMPDIR") {
            candidates.push(t.join("k2-runtime"));
        }
    }
    candidates
}

/// The private runtime dir, or `None` when no candidate is usable.
///
/// Panics (loudly, on purpose) when this process looks like a test and the
/// dir would be the real user runtime dir — see the module doc. A
/// production daemon never carries the test markers.
pub fn private_runtime_dir() -> Option<PathBuf> {
    match resolve_private_runtime_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            panic!("{e}");
        }
    }
}

/// [`private_runtime_dir`] without the panic: `Err` = refused (a test
/// process resolved the real user runtime dir).
pub fn resolve_private_runtime_dir() -> Result<Option<PathBuf>, String> {
    let is_test = looks_like_test_process();
    let real = if is_test { real_runtime_dirs() } else { Vec::new() };
    for c in candidates() {
        if !c.parent().is_some_and(|p| p.is_dir()) {
            continue;
        }
        // Refuse BEFORE `ensure_private`, which would create or chmod the
        // real dir: a refused test must not touch it at all.
        check_not_real_under_test(&c, is_test, &real)?;
        if ensure_private(&c) {
            return Ok(Some(c));
        }
    }
    Ok(None)
}

/// Pure refusal rule: a test process must never use one of `real` (the
/// user's live runtime dirs).
pub fn check_not_real_under_test(dir: &Path, is_test: bool, real: &[PathBuf]) -> Result<(), String> {
    if !is_test {
        return Ok(());
    }
    // Canonical parent + leaf name: the dir itself may not exist yet, and
    // macOS spells the temp dir both /var/folders and /private/var/folders.
    let canon = |p: &Path| match (p.parent(), p.file_name()) {
        (Some(parent), Some(name)) => std::fs::canonicalize(parent)
            .map(|c| c.join(name))
            .unwrap_or_else(|_| p.to_path_buf()),
        _ => p.to_path_buf(),
    };
    let d = canon(dir);
    if real.iter().any(|r| canon(r) == d) {
        return Err(format!(
            "REFUSED: this looks like a test process ({TEST_MARKER_ENV} set, or $HOME is a temp dir) \
             but the K2 private runtime dir resolved to the REAL user runtime dir {}. A test daemon \
             there overwrites and deletes the production frpc.toml and kills the production frpc. \
             Give every test daemon its own TMPDIR and XDG_RUNTIME_DIR under its temp root \
             (k2_core::test_env::DaemonRuntime), or set {RUNTIME_DIR_ENV} for in-process tests \
             (k2_core::test_env::TempHome does).",
            dir.display()
        ));
    }
    Ok(())
}

/// The user's REAL runtime dirs (what the production daemon resolves).
pub fn real_runtime_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    #[cfg(target_os = "linux")]
    {
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        v.push(PathBuf::from(format!("/run/user/{uid}/k2")));
        v.push(PathBuf::from(format!("/dev/shm/k2-{uid}")));
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(t) = darwin_user_temp_dir() {
            v.push(t.join("k2-runtime"));
        }
    }
    v
}

/// macOS per-user temp dir from `confstr(_CS_DARWIN_USER_TEMP_DIR)` — the
/// real one, whatever `$TMPDIR` says.
#[cfg(target_os = "macos")]
pub fn darwin_user_temp_dir() -> Option<PathBuf> {
    use std::ffi::CStr;
    let mut buf = vec![0 as libc::c_char; 1024];
    // SAFETY: buf is valid for buf.len() bytes; confstr NUL-terminates.
    let n = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr(), buf.len()) };
    if n == 0 || n > buf.len() {
        return None;
    }
    // SAFETY: confstr wrote a NUL-terminated string into buf.
    let s = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned();
    (!s.is_empty()).then(|| PathBuf::from(s))
}

/// The home the password database lists for this uid (never `$HOME`).
fn passwd_home() -> Option<PathBuf> {
    use std::ffi::CStr;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let mut size = 4096usize;
    loop {
        let mut buf = vec![0 as libc::c_char; size];
        // SAFETY: zeroed passwd is a valid out-param for getpwuid_r.
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: all pointers are valid for the call.
        let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result) };
        if rc == libc::ERANGE && size < 1 << 20 {
            size *= 2;
            continue;
        }
        if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
            return None;
        }
        // SAFETY: pw_dir is a NUL-terminated string inside buf.
        let dir = unsafe { CStr::from_ptr(pwd.pw_dir) }.to_string_lossy().into_owned();
        return (!dir.is_empty()).then(|| PathBuf::from(dir));
    }
}

/// Temp roots a test `$HOME` lives under (never a production home).
fn temp_roots() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> =
        ["/tmp", "/private/tmp", "/var/tmp", "/dev/shm", "/var/folders", "/private/var/folders"]
            .iter()
            .map(PathBuf::from)
            .collect();
    if let Some(t) = env_path("TMPDIR") {
        v.push(t);
    }
    #[cfg(target_os = "macos")]
    if let Some(t) = darwin_user_temp_dir() {
        v.push(t);
    }
    v
}

/// Pure test-process rule: the harness marker is set, or `$HOME` sits
/// under a temp root and is not the account's own (password-database)
/// home.
pub fn is_test_process(marker_set: bool, home: Option<&Path>, passwd: Option<&Path>, roots: &[PathBuf]) -> bool {
    if marker_set {
        return true;
    }
    let Some(home) = home else { return false };
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let h = canon(home);
    if passwd.is_some_and(|p| canon(p) == h) {
        return false;
    }
    roots.iter().any(|r| h.starts_with(canon(r)) || h.starts_with(r))
}

/// Whether this process looks like a test (see [`is_test_process`]).
pub fn looks_like_test_process() -> bool {
    is_test_process(
        env_path(TEST_MARKER_ENV).is_some(),
        env_path("HOME").as_deref(),
        passwd_home().as_deref(),
        &temp_roots(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_process_is_refused_the_real_runtime_dir() {
        let root = crate::test_env::unique_temp_path("rt-belt");
        let real = root.join("real").join("k2-runtime");
        let mine = root.join("mine").join("k2-runtime");
        std::fs::create_dir_all(&real).expect("mkdir real");
        std::fs::create_dir_all(&mine).expect("mkdir mine");
        let reals = vec![real.clone()];
        let err = check_not_real_under_test(&real, true, &reals).expect_err("must refuse");
        assert!(err.contains("REFUSED"), "{err}");
        assert!(err.contains(&real.display().to_string()), "{err}");
        check_not_real_under_test(&mine, true, &reals).expect("own dir is fine");
        check_not_real_under_test(&real, false, &reals).expect("production uses the real dir");
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn test_process_detection() {
        let roots = vec![PathBuf::from("/tmp"), PathBuf::from("/var/folders")];
        let passwd = Path::new("/Users/someone");
        assert!(is_test_process(true, Some(passwd), Some(passwd), &roots), "marker wins");
        assert!(is_test_process(false, Some(Path::new("/tmp/k2t-abc")), Some(passwd), &roots));
        assert!(is_test_process(
            false,
            Some(Path::new("/var/folders/xx/T/k2-test-home-1")),
            Some(passwd),
            &roots
        ));
        assert!(!is_test_process(false, Some(passwd), Some(passwd), &roots), "the real home");
        assert!(!is_test_process(false, Some(Path::new("/home/k2")), Some(passwd), &roots));
        // A box whose real home IS under /tmp is not a test.
        assert!(!is_test_process(false, Some(Path::new("/tmp/h")), Some(Path::new("/tmp/h")), &roots));
        assert!(!is_test_process(false, None, Some(passwd), &roots));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_real_macos_runtime_dir_comes_from_confstr() {
        let t = darwin_user_temp_dir().expect("confstr user temp dir");
        assert!(t.is_dir(), "{}", t.display());
        assert_eq!(real_runtime_dirs(), vec![t.join("k2-runtime")]);
    }
}
