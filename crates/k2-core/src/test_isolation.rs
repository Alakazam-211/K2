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
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a.to_string_lossy().trim_end_matches('/') == b.to_string_lossy().trim_end_matches('/'),
    }
}

/// Panic when this process inherited a variable that reaches a daemon.
#[track_caller]
pub fn assert_no_prod_env() {
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
