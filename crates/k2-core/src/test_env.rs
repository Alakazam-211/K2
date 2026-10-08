//! The ONE lock for process-environment mutation in tests (quiet-gate
//! PRD §5.3, 0.45.1).
//!
//! `$HOME`, `$PATH`, `$SHELL` and the `K2_*` / `K2SO_*` knobs are process
//! globals. Cargo runs a test binary's tests on many threads, so two tests
//! that change the same variable under DIFFERENT locks (or none) stomp each
//! other: one test's store lands under another test's (then deleted) temp
//! home. Before 0.45.1 k2-core had four separate HOME/PATH locks and
//! k2-daemon three more, which is exactly how the `skin::`, `sql::`,
//! `federation_routes::` and `subscription_usage::` flakes happened.
//!
//! Rules (also in `docs/testing.md`):
//!
//! - Every test that sets or removes an env var takes [`lock`] first, or
//!   uses a guard here that takes it ([`EnvVar`], [`TempHome`]).
//! - Every test that READS `$HOME` (directly, or through `dirs::home_dir()`
//!   / a store under `~/.k2`) and needs it stable holds [`lock`] (a
//!   [`TempHome`] does) for the whole time it depends on that value.
//! - Never call `std::env::set_var` / `remove_var` for `HOME`, `PATH` or
//!   `SHELL` outside this module: a source-walk ratchet test fails on it.
//! - k2-daemon re-uses this exact lock (through `test-util`), so both
//!   crates' code in one test binary serialize on the same mutex.
//!
//! The lock is RE-ENTRANT on the same thread, so a test holding it may
//! call a helper that takes it again. It does not poison: a panicking
//! test releases it and the next test runs normally. Guards restore the
//! previous value on drop (LIFO), including on panic.
//!
//! Async tests: the guard is `!Send`, which is fine for `#[tokio::test]`
//! (the test body runs on the test thread). Code inside spawned tasks that
//! reads `$HOME` sees the test's value only while the guard is alive.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use parking_lot::{ReentrantMutex, ReentrantMutexGuard};

static ENV_LOCK: ReentrantMutex<()> = parking_lot::const_reentrant_mutex(());

/// Guard for the crate-wide test env lock.
pub type EnvLock = ReentrantMutexGuard<'static, ()>;

/// Take the crate-wide (and, through `test-util`, cross-crate) env lock.
pub fn lock() -> EnvLock {
    ENV_LOCK.lock()
}

/// Set or remove one env var for the life of the guard, then restore the
/// previous value. Holds [`lock`] the whole time.
#[must_use = "the variable is restored when the guard drops"]
pub struct EnvVar {
    key: OsString,
    prev: Option<OsString>,
    _lock: EnvLock,
}

impl EnvVar {
    /// Set `key` to `value` until the guard drops.
    pub fn set(key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        let lock = lock();
        let key = key.as_ref().to_os_string();
        let prev = std::env::var_os(&key);
        std::env::set_var(&key, value);
        Self { key, prev, _lock: lock }
    }

    /// Remove `key` until the guard drops.
    pub fn remove(key: impl AsRef<OsStr>) -> Self {
        let lock = lock();
        let key = key.as_ref().to_os_string();
        let prev = std::env::var_os(&key);
        std::env::remove_var(&key);
        Self { key, prev, _lock: lock }
    }
}

impl Drop for EnvVar {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => std::env::set_var(&self.key, v),
            None => std::env::remove_var(&self.key),
        }
    }
}

/// A fresh, unique temp dir path (not created). Uses a uuid, never the pid,
/// a `ThreadId` or a timestamp: those repeat across the several test
/// binaries of one `cargo test` run and across runs.
pub fn unique_temp_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("k2-{tag}-{}", uuid::Uuid::new_v4().simple()))
}

/// RAII temp `$HOME`: holds [`lock`], points `$HOME` at a fresh temp dir
/// (with `.k2/` pre-created so writers that stage a tmp file and rename it
/// into `~/.k2/…` find the parent), checks the test cannot reach the
/// production daemon, and on drop restores the previous `$HOME` and removes
/// the dir.
#[must_use = "HOME is restored when the guard drops"]
pub struct TempHome {
    prev: Option<OsString>,
    home: PathBuf,
    _lock: EnvLock,
}

impl TempHome {
    /// A temp home under the system temp dir.
    pub fn new() -> Self {
        Self::at(unique_temp_path("test-home"))
    }

    /// A temp home with a SHORT path (`/tmp/k2t-<8 hex>`), for tests that
    /// bind Unix sockets under `$HOME`: a socket path must fit `SUN_LEN`
    /// (~104 bytes on macOS, 108 on Linux), and the macOS per-user temp dir
    /// alone is ~50 bytes.
    #[cfg(unix)]
    pub fn short() -> Self {
        let id = uuid::Uuid::new_v4().simple().to_string();
        Self::at(PathBuf::from(format!("/tmp/k2t-{}", &id[..8])))
    }

    fn at(home: PathBuf) -> Self {
        let lock = lock();
        std::fs::create_dir_all(home.join(".k2")).expect("create temp HOME/.k2");
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);
        crate::test_isolation::assert_isolated_from_prod();
        Self { prev, home, _lock: lock }
    }

    /// The temp `$HOME`.
    pub fn path(&self) -> &Path {
        &self.home
    }
}

impl Default for TempHome {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// Run `f` with a fresh temp `$HOME` (see [`TempHome`]).
pub fn with_temp_home<R>(f: impl FnOnce(&Path) -> R) -> R {
    let home = TempHome::new();
    f(home.path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_guard_restores_previous_value_lifo() {
        const KEY: &str = "K2_TEST_ENV_GUARD_PROBE";
        let _l = lock();
        assert_eq!(std::env::var_os(KEY), None);
        {
            let _a = EnvVar::set(KEY, "a");
            assert_eq!(std::env::var(KEY).unwrap(), "a");
            {
                let _b = EnvVar::set(KEY, "b");
                assert_eq!(std::env::var(KEY).unwrap(), "b");
                {
                    let _c = EnvVar::remove(KEY);
                    assert_eq!(std::env::var_os(KEY), None);
                }
                assert_eq!(std::env::var(KEY).unwrap(), "b");
            }
            assert_eq!(std::env::var(KEY).unwrap(), "a");
        }
        assert_eq!(std::env::var_os(KEY), None);
    }

    #[test]
    fn temp_home_is_unique_restored_and_removed() {
        let _l = lock();
        let before = std::env::var_os("HOME");
        let dir;
        {
            let a = TempHome::new();
            dir = a.path().to_path_buf();
            assert_eq!(std::env::var_os("HOME").as_deref(), Some(dir.as_os_str()));
            assert!(dir.join(".k2").is_dir());
            // Re-entrant: a nested guard on the same thread does not deadlock.
            let b = TempHome::new();
            assert_ne!(a.path(), b.path());
            drop(b);
            assert_eq!(std::env::var_os("HOME").as_deref(), Some(dir.as_os_str()));
        }
        assert_eq!(std::env::var_os("HOME"), before);
        assert!(!dir.exists(), "temp home removed on drop");
    }

    #[test]
    fn lock_survives_a_panicking_holder() {
        let t = std::thread::spawn(|| {
            let _g = lock();
            panic!("holder panics");
        });
        assert!(t.join().is_err());
        // Not poisoned: the next taker proceeds.
        let _g = lock();
    }
}
