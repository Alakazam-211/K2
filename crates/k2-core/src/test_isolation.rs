//! Prod-isolation guard for tests (K2 test rules, 2026-10-06).
//!
//! A test process run from inside a K2 agent session inherits that
//! session's hook socket, scoped token and pane id. Anything in the test
//! that shells out to `k2` (or a daemon it spawns) then talks to the
//! PRODUCTION daemon. A test whose `$HOME` is the real home reads the
//! production daemon's port and token files the same way. Both are
//! refused here, loudly: a test that can see production must not run.
//!
//! - [`assert_no_prod_env`]: no inherited session variable that reaches a
//!   daemon. Pure tests call this.
//! - [`assert_isolated_from_prod`]: that, plus `$HOME` is not the real
//!   (password-database) home while that home holds a daemon port or
//!   token file. Tests that touch `$HOME`, spawn a daemon, or open
//!   sockets call this after pointing `$HOME` at a temp dir.
//!
//! Run tests with the session variables cleared, for example
//! `env -u K2_HOOK_SOCK -u K2_HOOK_TOKEN … cargo test`, or
//! `for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset $v; done`.

use std::path::{Path, PathBuf};

/// Session variables that carry a way to reach a live daemon: a socket,
/// a token, a port, or the pane/session identity hooks post under.
pub const PROD_REACH_VARS: &[&str] = &[
    "K2_HOOK_SOCK",
    "K2SO_HOOK_SOCK",
    "K2_HOOK_TOKEN",
    "K2SO_HOOK_TOKEN",
    "K2_PANE_ID",
    "K2SO_PANE_ID",
    "K2_TAB_ID",
    "K2SO_TAB_ID",
    "K2_PORT",
    "K2SO_PORT",
    "K2_SESSION_ID",
    "K2_CELL",
    "K2_API_CELL",
    "K2SO_API_CELL",
    "K2_REMOTE_TOKEN",
    "K2_CONNECT_BASE",
    "K2_HOME",
];

/// Daemon port/token files under `~/.k2` a test must never be able to read
/// from the real home.
pub const PROD_FILES: &[&str] = &["daemon.port", "daemon.token", "heartbeat.port", "heartbeat.token"];

/// The inherited prod-reaching variables present in `vars`.
pub fn leaked_vars<'a>(vars: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<String> {
    let mut out: Vec<String> = vars
        .into_iter()
        .filter(|(k, v)| !v.is_empty() && PROD_REACH_VARS.contains(k))
        .map(|(k, _)| k.to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The prod daemon files a test with this `home` could read: non-empty
/// only when `home` is the real home and the files exist there.
pub fn reachable_prod_files(home: &Path, real_home: Option<&Path>) -> Vec<PathBuf> {
    let Some(real) = real_home else { return Vec::new() };
    if !same_dir(home, real) {
        return Vec::new();
    }
    PROD_FILES
        .iter()
        .map(|f| real.join(".k2").join(f))
        .filter(|p| p.exists())
        .collect()
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (dunce::canonicalize(&a), dunce::canonicalize(&b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a.to_string_lossy().trim_end_matches('/') == b.to_string_lossy().trim_end_matches('/'),
    }
}

/// Panic when this process inherited a variable that reaches a daemon.
///
/// Reads the environment under the ONE env lock: another test's
/// `EnvVar::set("K2_CONNECT_BASE", …)` (restored on drop) is not an
/// inherited variable, and without the lock a parallel test saw it
/// (soak 2026-10-25, k2-core shuffle seed 9).
#[track_caller]
pub fn assert_no_prod_env() {
    let _env = crate::test_env::lock();
    let env: Vec<(String, String)> = std::env::vars().collect();
    let leaked = leaked_vars(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    assert!(
        leaked.is_empty(),
        "prod-isolation guard: this test process inherited {leaked:?}, which reach the \
         production daemon. Clear every K2_*/K2SO_* session variable before running tests."
    );
}

/// Panic when this process can reach the production daemon: an inherited
/// session variable, or `$HOME` is the real home that holds the daemon's
/// port/token files.
#[track_caller]
pub fn assert_isolated_from_prod() {
    assert_no_prod_env();
    let home = std::env::var_os("HOME").map(PathBuf::from).expect("prod-isolation guard: HOME is unset");
    let files = reachable_prod_files(&home, real_home().as_deref());
    assert!(
        files.is_empty(),
        "prod-isolation guard: HOME is the real home and the production daemon's {files:?} are \
         readable. Point HOME at a temp dir before this test runs."
    );
}

/// Store files a test redirects with an env knob (`K2_THREADS_REDB`,
/// `K2_DOCS_REDB`, ...) must never be the real home's: a test writing there
/// would change the production daemon's data (docs.redb, MI14).
pub fn store_in_real_home(path: &Path, real_home: Option<&Path>) -> bool {
    let Some(real) = real_home else { return false };
    let k2 = real.join(".k2");
    let dir = path.parent().unwrap_or(path);
    same_dir(dir, &k2) || path.starts_with(&k2)
}

/// Panic when `path` (a store a test redirected) is under the real home's
/// `.k2`.
#[track_caller]
pub fn assert_store_not_in_real_home(path: &Path) {
    assert!(
        !store_in_real_home(path, real_home().as_deref()),
        "prod-isolation guard: store path {path:?} is in the real home's .k2; point it at a temp dir"
    );
}

/// Is `home` a throwaway home a test may write daemon stores under? Not the
/// real home, and inside a temp root: `temp_dirs` (the system temp dir), or
/// the short `/tmp` a socket test uses ([`crate::test_env::short_tmp`]), or
/// a home that holds `TMPDIR` itself (a daemon harness points TMPDIR into
/// its scratch HOME).
pub fn home_is_isolated(home: &Path, real_home: Option<&Path>, temp_dirs: &[PathBuf]) -> bool {
    let canon = canonical_even_if_missing;
    let home = canon(home);
    if real_home.is_some_and(|r| home == canon(r)) {
        return false;
    }
    temp_dirs.iter().any(|t| {
        let t = canon(t);
        home.starts_with(&t) || t.starts_with(&home)
    }) || is_stub_home(&home)
}

/// `canonicalize`, but for a path that may not exist yet: the nearest
/// existing ancestor is resolved and the rest is joined back on. A temp
/// root on macOS is under `/var`, a link to `/private/var`, so a fresh
/// `<temp>/k2-test-home` must resolve through the same link as `<temp>`.
fn canonical_even_if_missing(p: &Path) -> PathBuf {
    let mut base = p;
    let mut rest = Vec::new();
    loop {
        if let Ok(c) = dunce::canonicalize(base) {
            return rest.iter().rev().fold(c, |acc, name| acc.join(name));
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                base = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

/// A test's stub home outside the temp roots ([`crate::test_env::StubHome`],
/// k2-daemon's `stub_home_dir`): `k2-<tag>-home-<32 hex>`.
fn is_stub_home(home: &Path) -> bool {
    let Some(name) = home.file_name().and_then(|n| n.to_str()) else { return false };
    let Some(rest) = name.strip_prefix("k2-") else { return false };
    let Some((_, id)) = rest.rsplit_once("-home-") else { return false };
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The temp roots [`home_is_isolated`] accepts in this process.
fn temp_roots() -> Vec<PathBuf> {
    let mut roots = vec![std::env::temp_dir(), crate::test_env::stable_temp_dir()];
    if cfg!(unix) {
        roots.push(PathBuf::from("/tmp"));
    }
    roots
}

/// Panic when `store` (a file under `~/.k2` that live agents depend on) is
/// about to be read or written in a home that isn't a test's own. 2026-11-07:
/// a `cargo test` with the real HOME overwrote `hook-sessions.json`, and the
/// next daemon refused every surviving agent's passport.
#[track_caller]
pub fn assert_home_isolated_for_store(store: &str) {
    let home = crate::paths::home_dir().unwrap_or_else(|| PathBuf::from("."));
    assert!(
        home_is_isolated(&home, real_home().as_deref(), &temp_roots()),
        "prod-isolation guard: a test build reached ~/.k2/{store} in {}, which is not a temp home. \
         Wrap the test in TempHome / with_temp_home, or give the daemon it spawns a temp HOME.",
        home.display()
    );
}

/// The home the password database lists for this uid (never `$HOME`).
#[cfg(unix)]
pub fn real_home() -> Option<PathBuf> {
    use std::ffi::CStr;
    let uid = unsafe { libc::getuid() };
    let mut size = 4096usize;
    loop {
        let mut buf = vec![0 as libc::c_char; size];
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result) };
        if rc == libc::ERANGE && size < 1 << 20 {
            size *= 2;
            continue;
        }
        if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
            return None;
        }
        let dir = unsafe { CStr::from_ptr(pwd.pw_dir) }.to_string_lossy().into_owned();
        return (!dir.is_empty()).then(|| PathBuf::from(dir));
    }
}

#[cfg(not(unix))]
pub fn real_home() -> Option<PathBuf> {
    // The profile folder the shell reports, never HOME/USERPROFILE.
    dirs::home_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_variables_are_caught_and_test_knobs_are_not() {
        let vars = [
            ("K2_HOOK_SOCK", "/tmp/x.sock"),
            ("K2SO_PANE_ID", "abc"),
            ("K2_TEST_AGENT_SHIM_DIR", "/tmp/shim"),
            ("K2_HOOK_INSTALL", "0"),
            ("K2_PORT", ""),
            ("PATH", "/bin"),
        ];
        assert_eq!(leaked_vars(vars), vec!["K2SO_PANE_ID".to_string(), "K2_HOOK_SOCK".to_string()]);
    }

    #[test]
    fn a_store_home_must_be_a_temp_home_and_not_the_real_one() {
        let temp = std::env::temp_dir();
        let roots = vec![temp.clone()];
        let real = PathBuf::from("/Users/someone");
        assert!(!home_is_isolated(&real, Some(&real), &roots), "the real home");
        assert!(!home_is_isolated(Path::new("/var/lib/elsewhere"), Some(&real), &roots), "not a temp home");
        assert!(home_is_isolated(&temp.join("k2-test-home"), Some(&real), &roots), "under the temp dir");
        assert!(home_is_isolated(&temp.join("h"), None, &[temp.join("h").join("tmp")]), "TMPDIR inside the home");
        assert!(!home_is_isolated(&temp, Some(&temp), &roots), "a real home under the temp dir is still real");
        let stub = PathBuf::from("/var/tmp/k2-v2spawn-home-0123456789abcdef0123456789abcdef");
        assert!(home_is_isolated(&stub, Some(&real), &roots), "a test's stub home");
        assert!(!home_is_isolated(Path::new("/var/tmp/k2-v2spawn-home-oops"), Some(&real), &roots), "not a stub name");
        assert!(!home_is_isolated(Path::new("/home/k2-x-home-0123456789abcdef0123456789abcdef"), Some(Path::new("/home/k2-x-home-0123456789abcdef0123456789abcdef")), &roots), "the real home, whatever its name");
    }

    /// macOS's temp root is `/var/...`, a link to `/private/var/...`. A home
    /// under the root that doesn't exist yet must resolve through the same
    /// link. Built with a real link so Linux checks it too.
    #[cfg(unix)]
    #[test]
    fn a_missing_home_under_a_linked_temp_root_is_isolated() {
        let base = std::env::temp_dir().join(format!("k2-iso-link-{}-{}", std::process::id(), line!()));
        let real_root = base.join("real");
        let linked_root = base.join("linked");
        std::fs::create_dir_all(&real_root).expect("real root");
        std::os::unix::fs::symlink(&real_root, &linked_root).expect("link");
        let real = PathBuf::from("/Users/someone");
        let home = linked_root.join("k2-test-home").join("deeper");
        assert!(!home.exists(), "the home must not exist yet");
        let isolated = home_is_isolated(&home, Some(&real), &[linked_root.clone()]);
        let isolated_by_real_path = home_is_isolated(&real_root.join("k2-test-home"), Some(&real), &[linked_root.clone()]);
        std::fs::remove_dir_all(&base).expect("clean up");
        assert!(isolated, "a missing home under a linked temp root");
        assert!(isolated_by_real_path, "the same home named by its real path");
    }

    #[test]
    fn a_redirected_store_in_the_real_home_is_caught() {
        let real = std::env::temp_dir().join(format!("k2-guard-store-real-{}", std::process::id()));
        let temp = std::env::temp_dir().join(format!("k2-guard-store-temp-{}", std::process::id()));
        std::fs::create_dir_all(real.join(".k2")).expect("mkdir");
        assert!(store_in_real_home(&real.join(".k2/docs.redb"), Some(&real)));
        assert!(store_in_real_home(&real.join(".k2/sub/docs.redb"), Some(&real)));
        assert!(!store_in_real_home(&temp.join("docs.redb"), Some(&real)));
        assert!(!store_in_real_home(&real.join(".k2/docs.redb"), None));
        std::fs::remove_dir_all(&real).expect("cleanup");
    }

    #[test]
    fn prod_files_count_only_under_the_real_home() {
        let real = std::env::temp_dir().join(format!("k2-guard-real-{}", std::process::id()));
        let temp = std::env::temp_dir().join(format!("k2-guard-temp-{}", std::process::id()));
        std::fs::create_dir_all(real.join(".k2")).expect("mkdir");
        std::fs::create_dir_all(temp.join(".k2")).expect("mkdir");
        std::fs::write(real.join(".k2/daemon.port"), "1").expect("write");
        std::fs::write(temp.join(".k2/daemon.port"), "1").expect("write");
        assert_eq!(reachable_prod_files(&real, Some(&real)), vec![real.join(".k2/daemon.port")]);
        assert!(reachable_prod_files(&temp, Some(&real)).is_empty(), "a temp HOME is isolated");
        assert!(reachable_prod_files(&real, None).is_empty());
        std::fs::remove_dir_all(&real).expect("cleanup");
        std::fs::remove_dir_all(&temp).expect("cleanup");
    }
}
