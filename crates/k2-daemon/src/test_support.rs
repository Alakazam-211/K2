//! Test-only isolation helpers shared across the crate.
//!
//! `$HOME` (and every other env var) is process-global, so every test that
//! changes it, or depends on it staying put, must serialize on ONE lock.
//! That lock is k2-core's [`k2_core::test_env::lock`]: k2-daemon's unit
//! tests link k2-core with `test-util`, so the daemon's tests and any
//! k2-core test code in the same binary share the exact same mutex
//! (quiet-gate PRD §5.3, 0.45.1). Before that, the daemon had its own HOME
//! lock plus private ones in `update_routes`, `dns/proxy`, `misc_routes`,
//! `watchdog` and `dns/routes`, and the boot_verifier tests set HOME under
//! none of them and never restored it.
//!
//! Use [`TempHome`] (RAII) or [`with_temp_home`] (closure) for a temp
//! `$HOME`, `k2_core::test_env::EnvVar` for any other variable, and
//! [`lock_home`] to hold the lock without changing anything. Never call
//! `std::env::set_var` for HOME/PATH/SHELL directly: a ratchet test fails.

#![cfg(test)]

use std::sync::Mutex;

/// RAII temp `$HOME` holding the shared env lock (see
/// [`k2_core::test_env::TempHome`]): `.k2/` pre-created, prod-isolation
/// checked, previous HOME restored and the dir removed on drop.
pub(crate) type TempHome = k2_core::test_env::TempHome;

/// Synchronous closure form of [`TempHome`].
pub(crate) fn with_temp_home<F: FnOnce()>(f: F) {
    let _home = TempHome::new();
    f();
}

/// Hold the shared env lock WITHOUT changing anything — for tests that
/// set their own (deliberately SHORT) `$HOME` through
/// `k2_core::test_env::EnvVar`, or only need `$HOME` to stay put.
pub(crate) fn lock_home() -> k2_core::test_env::EnvLock {
    k2_core::test_env::lock()
}

/// RATCHET (quiet-gate PRD §5.3): no raw env mutation in k2-daemon `src/`.
/// Tests change env only through `k2_core::test_env` guards (the one env
/// lock, restore on drop). Production sites are listed with their counts;
/// lower a count when you remove one, never raise it.
#[test]
fn no_new_raw_env_mutation_in_k2_daemon() {
    const BASELINE: &[(&str, usize, usize)] = &[
        // Production: the VMM worker pins LD_LIBRARY_PATH while single-threaded.
        ("bin/k2-vmm-worker.rs", 1, 0),
    ];
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let violations = k2_core::test_env::env_mutation_violations(&src, BASELINE);
    assert!(
        violations.is_empty(),
        "raw env mutation over the ratchet:\n{}",
        violations.join("\n")
    );
}

/// No temp names from a `ThreadId` (see
/// `k2_core::test_env::thread_id_name_violations`).
#[test]
fn no_thread_id_temp_names_in_k2_daemon() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let v = k2_core::test_env::thread_id_name_violations(&src);
    assert!(v.is_empty(), "use test_env::unique_temp_path, not a ThreadId:\n{}", v.join("\n"));
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
