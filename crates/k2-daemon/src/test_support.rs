//! Test-only isolation helpers shared across the crate.
//!
//! Several test modules sandbox `$HOME` to a fresh tempdir so each test gets
//! an isolated `~/.k2` (the `connect-users.json` store, the tunnel cert, the
//! update-job staging dir, …). Because `$HOME` is a PROCESS-GLOBAL env var,
//! any two HOME-swapping tests that run concurrently would stomp each other —
//! one test deleting its tempdir mid-write under another, or one reading a
//! cert another just minted for a different name.
//!
//! The fix is that EVERY test which mutates `$HOME` must serialize on the SAME
//! lock — [`home_lock`] — the single crate-wide source of truth for "I am
//! currently mutating `$HOME`". Acquire it via [`TempHome`] (async/RAII) or
//! [`with_temp_home`] (synchronous closure); never swap `$HOME` by hand.

#![cfg(test)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// The one crate-wide lock guarding `$HOME` mutation in tests. All
/// HOME-swapping tests, in any module, must serialize on this exact lock.
fn home_lock() -> &'static Mutex<()> {
    static HOME_LOCK: Mutex<()> = Mutex::new(());
    &HOME_LOCK
}

/// RAII guard: holds the crate-wide [`home_lock`], points `$HOME` at a fresh
/// tempdir (with `.k2/` pre-created so cert/keypair writers can `rename` a
/// staged file into place), and on drop restores the prior `$HOME` and removes
/// the tempdir.
///
/// Use this for async tests — the lock is held across `.await` points, which
/// is sound under the default current-thread `#[tokio::test]` runtime. See
/// [`with_temp_home`] for the synchronous closure form.
pub(crate) struct TempHome {
    // Field order matters: dropped top-to-bottom, so `$HOME` is restored and
    // the tempdir removed (in `Drop`) before the lock is released.
    prev: Option<OsString>,
    home: PathBuf,
    _guard: MutexGuard<'static, ()>,
}

impl TempHome {
    pub(crate) fn new() -> Self {
        let guard = home_lock().lock().unwrap_or_else(|e| e.into_inner());
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let home = std::env::temp_dir()
            .join(format!("k2-test-home-{}-{nanos}", std::process::id()));
        // Pre-create `.k2/` so writers that stage a tmp file and `rename` it
        // into `~/.k2/…` find the parent dir present.
        std::fs::create_dir_all(home.join(".k2")).expect("create temp HOME/.k2");
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);
        Self {
            prev,
            home,
            _guard: guard,
        }
    }

    /// The sandboxed `$HOME` directory.
    #[allow(dead_code)]
    pub(crate) fn path(&self) -> &Path {
        &self.home
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

/// Synchronous closure form of [`TempHome`]: locks the crate-wide
/// [`home_lock`], swaps `$HOME` to a fresh sandbox for the duration of `f`,
/// then restores it.
pub(crate) fn with_temp_home<F: FnOnce()>(f: F) {
    let _home = TempHome::new();
    f();
}

/// Hold the crate-wide [`home_lock`] WITHOUT swapping `$HOME` — for tests
/// that must hand-roll their own (deliberately SHORT) `$HOME` because a
/// Unix-socket path is capped at `SUN_LEN` (~104 bytes) and [`TempHome`]'s
/// tempdir path blows past it (see `cell_uds`'s tests). Such tests still
/// mutate the process-global `$HOME`, so they MUST serialize with every
/// other HOME mutator on this same lock.
pub(crate) fn lock_home() -> MutexGuard<'static, ()> {
    home_lock().lock().unwrap_or_else(|e| e.into_inner())
}

// ── Shared agent-hooks capture sink ───────────────────────────────────
//
// The agent-hooks sink slot (`k2_core::agent_hooks::set_sink`) is
// PROCESS-GLOBAL and last-writer-wins — so every event-asserting test
// module in the k2-daemon binary must share ONE capture sink (a module
// installing its own would silently steal emissions from the others).
// Tests run in parallel and all emissions land in the shared buffer;
// each assertion filters by its own (unique) entity id, so cross-test
// traffic is invisible. Used by `feedback_routes` and
// `project_group_routes` tests.

use std::sync::OnceLock;

static CAPTURED_EVENTS: OnceLock<Mutex<Vec<(String, serde_json::Value)>>> = OnceLock::new();

fn captured_events() -> &'static Mutex<Vec<(String, serde_json::Value)>> {
    CAPTURED_EVENTS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Install the crate-wide capture sink (idempotent — installs once per
/// process; every later call is a no-op so the single-sink invariant
/// holds).
pub(crate) fn install_capture_sink() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        struct Capture;
        impl k2_core::agent_hooks::AgentHookEventSink for Capture {
            fn emit(&self, event: k2_core::agent_hooks::HookEvent, payload: serde_json::Value) {
                captured_events()
                    .lock()
                    .expect("capture sink lock")
                    .push((event.event_name().to_string(), payload));
            }
        }
        k2_core::agent_hooks::set_sink(Box::new(Capture));
    });
}

/// Snapshot the capture-buffer length BEFORE a mutation…
pub(crate) fn event_mark() -> usize {
    captured_events().lock().expect("capture sink lock").len()
}

/// …then collect every `(wire_name, payload)` emitted since. Callers
/// filter by their own unique entity id (`payload["id"]`,
/// `payload["groupId"]`, …) so parallel tests never see each other.
pub(crate) fn events_since(mark: usize) -> Vec<(String, serde_json::Value)> {
    captured_events().lock().expect("capture sink lock")[mark..].to_vec()
}
