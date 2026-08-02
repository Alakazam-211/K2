//! P4-H4 — per-principal + global concurrent-microVM cap (DoS bound for the
//! PUBLIC `POST /v1/sandboxes` API).
//!
//! A daemon-process-global live-cell counter so no single authenticated
//! [`crate::routes::http::V1Principal`] can hold more than `per_principal_cap`
//! live sandbox cells at once, and a `global_cap` bounds the TOTAL across all
//! principals. At either limit the spawn door returns **429** with a
//! machine-readable `code` (`concurrent-cell-cap` per-principal /
//! `cell-capacity` global) and provisions NOTHING (zero side effects).
//!
//! ## Invariants
//! - **Reached ONLY via `/v1/sandboxes`** (itself behind `K2_SANDBOX_API`,
//!   default OFF). Normal v2/spawn + cockpit sessions NEVER acquire and NEVER
//!   release → exact default-OFF parity, zero behavior change.
//! - **Never leaks.** The acquire happens at the door (before any provisioning);
//!   the release happens in the SINGLE authoritative teardown point
//!   ([`crate::v2_spawn::spawn_child_exit_observer`]'s `ChildExit` arm, which
//!   also revokes the scoped token + rmdirs the cgroup + removes the ephemeral
//!   dir). That arm fires on clean exit AND on crash/OOM/kill-9, so a counted
//!   slot is always returned. The early-failure spawn-door paths (resolve or
//!   spawn failed before a session/observer existed) release explicitly.
//! - **Saturating release.** A decrement never underflows below 0, so a stray /
//!   double release can never corrupt the count into a huge `usize`.
//!
//! ## Pure core, thin global shell
//! All the logic lives on [`QuotaState`] (a plain struct: per-principal map +
//! total). It is fully unit-testable on Mac with no I/O. The process-global
//! [`try_acquire`] / [`release`] free functions are a thin lock-then-delegate
//! shell over a `LazyLock<Mutex<QuotaState>>`. Cap VALUES are resolved by the
//! spawn door (owner vs api-key vs global) and passed in, so cap POLICY stays at
//! the door and the counter stays pure.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// A principal's stable identity key for the live-cell map. This is
/// [`crate::routes::http::V1Principal::display_id`]: the sentinel `"owner"` for
/// the owner token, else the API key's `api_keys.id`.
pub type PrincipalKey = String;

/// Why an acquire was REFUSED. Maps 1:1 to a 429 `code` at the door.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaError {
    /// This principal already holds `per_principal_cap` live cells → 429
    /// `{"code":"concurrent-cell-cap"}`.
    PerPrincipalCap,
    /// This workspace already holds `workspace_cap` live cells → 429
    /// `{"code":"workspace-cell-cap"}`.
    WorkspaceCap,
    /// The daemon already holds `global_cap` live cells across ALL principals →
    /// 429 `{"code":"cell-capacity"}`.
    GlobalCap,
    /// Historical / reserved 429 `{"code":"spawn-queue-timeout"}`.
    /// Pre-0.40.78 the queue-wait deadline always returned this and dropped the
    /// concrete cap (pilot F6). Acquire now returns the blocking cap code after
    /// wait; this variant stays for stable `code()` mapping + integrator docs
    /// + unit tests (never constructed on the live path under default queue).
    #[allow(dead_code)]
    QueueTimeout,
}

impl QuotaError {
    /// The machine-readable `code` field for the 429 JSON envelope.
    pub fn code(&self) -> &'static str {
        match self {
            QuotaError::PerPrincipalCap => "concurrent-cell-cap",
            QuotaError::WorkspaceCap => "workspace-cell-cap",
            QuotaError::GlobalCap => "cell-capacity",
            QuotaError::QueueTimeout => "spawn-queue-timeout",
        }
    }

    /// A human-readable, non-secret error message for the 429 `error` field.
    pub fn message(&self) -> &'static str {
        match self {
            QuotaError::PerPrincipalCap => {
                "concurrent sandbox cell limit reached for this principal"
            }
            QuotaError::WorkspaceCap => {
                "concurrent host-session limit reached for this workspace"
            }
            QuotaError::GlobalCap => "sandbox cell capacity reached for this daemon",
            QuotaError::QueueTimeout => {
                "timed out waiting in the spawn queue for a free cell slot"
            }
        }
    }
}

/// The PURE counter state: per-principal + per-workspace + global total.
#[derive(Debug, Default)]
pub struct QuotaState {
    per_principal: HashMap<PrincipalKey, usize>,
    per_workspace: HashMap<String, usize>,
    total: usize,
}

impl QuotaState {
    /// Construct an empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Live-cell count currently held by `key` (0 if none).
    pub fn count_for(&self, key: &str) -> usize {
        self.per_principal.get(key).copied().unwrap_or(0)
    }

    #[cfg(test)]
    pub fn workspace_count(&self, ws: &str) -> usize {
        self.per_workspace.get(ws).copied().unwrap_or(0)
    }

    /// Total live cells across all principals (asserted by unit tests only).
    #[cfg(test)]
    pub fn total(&self) -> usize {
        self.total
    }

    /// Atomically check principal + optional workspace + global caps.
    /// Order: principal → workspace → global (most specific first).
    ///
    /// On `Err` NOTHING is mutated — the caller has acquired no slot and must
    /// NOT release.
    ///
    /// Used by unit tests; process free functions call [`Self::try_acquire_ws`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn try_acquire(
        &mut self,
        key: &str,
        per_principal_cap: usize,
        global_cap: usize,
    ) -> Result<(), QuotaError> {
        self.try_acquire_ws(key, None, per_principal_cap, usize::MAX, global_cap)
    }

    pub fn try_acquire_ws(
        &mut self,
        key: &str,
        workspace: Option<&str>,
        per_principal_cap: usize,
        workspace_cap: usize,
        global_cap: usize,
    ) -> Result<(), QuotaError> {
        let current = self.count_for(key);
        if current >= per_principal_cap {
            return Err(QuotaError::PerPrincipalCap);
        }
        if let Some(ws) = workspace {
            let wc = self.per_workspace.get(ws).copied().unwrap_or(0);
            if wc >= workspace_cap {
                return Err(QuotaError::WorkspaceCap);
            }
        }
        if self.total >= global_cap {
            return Err(QuotaError::GlobalCap);
        }
        *self.per_principal.entry(key.to_string()).or_insert(0) += 1;
        if let Some(ws) = workspace {
            *self.per_workspace.entry(ws.to_string()).or_insert(0) += 1;
        }
        self.total += 1;
        Ok(())
    }

    /// Return one slot held by `key` (+ optional workspace). SATURATING.
    /// Used by unit tests; process free functions call [`Self::release_ws`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn release(&mut self, key: &str) {
        self.release_ws(key, None);
    }

    pub fn release_ws(&mut self, key: &str, workspace: Option<&str>) {
        if let Some(slot) = self.per_principal.get_mut(key) {
            if *slot <= 1 {
                self.per_principal.remove(key);
            } else {
                *slot -= 1;
            }
            self.total = self.total.saturating_sub(1);
        }
        if let Some(ws) = workspace {
            if let Some(slot) = self.per_workspace.get_mut(ws) {
                if *slot <= 1 {
                    self.per_workspace.remove(ws);
                } else {
                    *slot -= 1;
                }
            }
        }
    }
}

/// The process-global live-cell counter. One per daemon process.
static QUOTA: LazyLock<Mutex<QuotaState>> = LazyLock::new(|| Mutex::new(QuotaState::new()));

/// Default per-principal cap for an API-key principal (env
/// `K2_SANDBOX_PRINCIPAL_CELL_CAP`). Multi-tab integrators (Scout = one key ×
/// five spaces). Stock 5 was a one-interview cliff.
const DEFAULT_PRINCIPAL_CELL_CAP: usize = 64;

/// Default per-principal cap for the OWNER principal (env
/// `K2_SANDBOX_OWNER_CELL_CAP`).
const DEFAULT_OWNER_CELL_CAP: usize = 256;

/// Default GLOBAL ceiling across all principals (env `K2_SANDBOX_MAX_CELLS`).
/// Product default **512** live cells/daemon (stock 64 was a prod cliff).
const DEFAULT_GLOBAL_CELL_CAP: usize = 512;

/// Max live API cells in one workspace (env `K2_SANDBOX_WORKSPACE_CELL_CAP`).
const DEFAULT_WORKSPACE_CELL_CAP: usize = 15;

/// Max seconds a spawn waits in the K2 queue when at cap
/// (env `K2_SANDBOX_QUEUE_WAIT_SECS`). 0 = no wait (immediate 429).
const DEFAULT_QUEUE_WAIT_SECS: u64 = 30;

/// Poll interval while waiting in the spawn queue.
const QUEUE_POLL_MS: u64 = 50;

/// Read a positive-usize cap from an env var, falling back to `default` when the
/// var is absent, empty, unparsable, or zero (a zero cap would brick the API
/// entirely — treat it as "unset" and use the default).
fn env_cap(var: &str, default: usize) -> usize {
    match std::env::var(var) {
        Ok(v) => v.trim().parse::<usize>().ok().filter(|n| *n > 0).unwrap_or(default),
        Err(_) => default,
    }
}

fn queue_wait() -> Duration {
    let secs = match std::env::var("K2_SANDBOX_QUEUE_WAIT_SECS") {
        Ok(v) => v
            .trim()
            .parse::<u64>()
            .ok()
            .unwrap_or(DEFAULT_QUEUE_WAIT_SECS),
        Err(_) => DEFAULT_QUEUE_WAIT_SECS,
    };
    Duration::from_secs(secs)
}

/// The per-principal cap to apply to `principal_key`. The owner sentinel gets the
/// high own-use cap; every API-key principal gets the standard cap. Both
/// overridable via env.
pub fn per_principal_cap_for(principal_key: &str) -> usize {
    if principal_key == "owner" {
        env_cap("K2_SANDBOX_OWNER_CELL_CAP", DEFAULT_OWNER_CELL_CAP)
    } else {
        env_cap("K2_SANDBOX_PRINCIPAL_CELL_CAP", DEFAULT_PRINCIPAL_CELL_CAP)
    }
}

/// The global cell ceiling for THIS daemon. Overridable via `K2_SANDBOX_MAX_CELLS`.
pub fn global_cap() -> usize {
    env_cap("K2_SANDBOX_MAX_CELLS", DEFAULT_GLOBAL_CELL_CAP)
}

pub fn workspace_cap() -> usize {
    env_cap("K2_SANDBOX_WORKSPACE_CELL_CAP", DEFAULT_WORKSPACE_CELL_CAP)
}

/// PROCESS-GLOBAL acquire (no workspace axis). Back-compat for sandbox family.
pub fn try_acquire(principal_key: &str) -> Result<(), QuotaError> {
    try_acquire_in_workspace(principal_key, None)
}

/// Acquire with optional per-workspace ceiling (host-sessions pass `Some(ws_path)`).
/// When at capacity, **waits** up to `K2_SANDBOX_QUEUE_WAIT_SECS` (default 30s)
/// polling for a free slot (S8 K2 spawn queue).
///
/// On deadline (still full): returns the **last concrete cap** that refused
/// (`workspace-cell-cap` / `concurrent-cell-cap` / `cell-capacity`) — not a
/// uniform `spawn-queue-timeout`. Pre-0.40.78 always collapsed to
/// `spawn-queue-timeout` after the wait, so sustained-loop load never surfaced
/// the workspace-15 code (pilot F6). Set `K2_SANDBOX_QUEUE_WAIT_SECS=0` for
/// immediate refuse with the same codes (no wait).
///
/// `spawn-queue-timeout` remains a stable code string for integrators; it is
/// returned only when the wait path cannot recover a concrete cap (should not
/// happen on the normal acquire loop).
pub fn try_acquire_in_workspace(
    principal_key: &str,
    workspace: Option<&str>,
) -> Result<(), QuotaError> {
    let per_principal = per_principal_cap_for(principal_key);
    let global = global_cap();
    let ws_cap = workspace_cap();
    let max_wait = queue_wait();
    let deadline = Instant::now() + max_wait;
    loop {
        match {
            let mut state = QUOTA.lock().expect("sandbox quota mutex poisoned");
            state.try_acquire_ws(principal_key, workspace, per_principal, ws_cap, global)
        } {
            Ok(()) => return Ok(()),
            Err(err) => {
                if max_wait.is_zero() {
                    // Immediate refuse — surface the concrete cap that blocked us.
                    return Err(err);
                }
                if Instant::now() >= deadline {
                    // F6: keep the blocking cap code after the wait so Scout
                    // sees workspace-cell-cap (etc.), not only spawn-queue-timeout.
                    return Err(err);
                }
                std::thread::sleep(Duration::from_millis(QUEUE_POLL_MS));
            }
        }
    }
}

/// PROCESS-GLOBAL release (principal only). Prefer [`release_in_workspace`] when
/// the acquire used a workspace key.
pub fn release(principal_key: &str) {
    release_in_workspace(principal_key, None);
}

pub fn release_in_workspace(principal_key: &str, workspace: Option<&str>) {
    let mut state = QUOTA.lock().expect("sandbox quota mutex poisoned");
    state.release_ws(principal_key, workspace);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// N acquires for one principal succeed; the (N+1)th hits the per-principal
    /// cap with the `concurrent-cell-cap` code.
    #[test]
    fn n_acquires_ok_then_n_plus_one_hits_per_principal_cap() {
        let mut s = QuotaState::new();
        let cap = 5;
        let global = 1000; // make the global non-binding
        for i in 0..cap {
            assert!(
                s.try_acquire("P", cap, global).is_ok(),
                "acquire {i} within cap must succeed",
            );
        }
        assert_eq!(s.count_for("P"), cap);
        let err = s.try_acquire("P", cap, global).expect_err("N+1 must be refused");
        assert_eq!(err, QuotaError::PerPrincipalCap);
        assert_eq!(err.code(), "concurrent-cell-cap");
        // The refused acquire mutated NOTHING.
        assert_eq!(s.count_for("P"), cap, "a refused acquire must not increment");
        assert_eq!(s.total(), cap);
    }

    /// After hitting the cap, a release frees exactly one slot so the next
    /// acquire (N+1 -> N -> N+1) succeeds again.
    #[test]
    fn release_frees_a_slot() {
        let mut s = QuotaState::new();
        let cap = 3;
        for _ in 0..cap {
            s.try_acquire("P", cap, 1000).unwrap();
        }
        assert!(s.try_acquire("P", cap, 1000).is_err(), "at cap");
        s.release("P");
        assert_eq!(s.count_for("P"), cap - 1, "release frees one");
        assert!(s.try_acquire("P", cap, 1000).is_ok(), "freed slot is reusable");
        assert_eq!(s.count_for("P"), cap);
    }

    /// The global cap binds INDEPENDENTLY of the per-principal cap: many
    /// principals each well under their own cap can still exhaust the box, and
    /// the refusal carries the `cell-capacity` code.
    #[test]
    fn global_cap_is_independent_of_per_principal_cap() {
        let mut s = QuotaState::new();
        let per_principal = 100; // non-binding per-principal
        let global = 4;
        // Four DISTINCT principals each take one slot — none near its own cap.
        for p in ["a", "b", "c", "d"] {
            assert!(s.try_acquire(p, per_principal, global).is_ok());
        }
        assert_eq!(s.total(), global);
        // A fifth principal, also nowhere near its per-principal cap, is refused
        // on the GLOBAL ceiling.
        let err = s.try_acquire("e", per_principal, global).expect_err("global full");
        assert_eq!(err, QuotaError::GlobalCap);
        assert_eq!(err.code(), "cell-capacity");
        // Freeing one global slot re-admits.
        s.release("a");
        assert_eq!(s.total(), global - 1);
        assert!(s.try_acquire("e", per_principal, global).is_ok());
    }

    /// Per-principal cap is checked BEFORE the global cap: a principal at its own
    /// limit gets the specific per-principal code even if the box also happens to
    /// be globally full.
    #[test]
    fn per_principal_cap_takes_precedence_over_global() {
        let mut s = QuotaState::new();
        // P holds 2 of its cap-2; total == global cap 2 as well.
        s.try_acquire("P", 2, 2).unwrap();
        s.try_acquire("P", 2, 2).unwrap();
        let err = s.try_acquire("P", 2, 2).unwrap_err();
        assert_eq!(err, QuotaError::PerPrincipalCap, "per-principal reported first");
    }

    /// Release saturates at 0: extra releases never underflow the principal
    /// count or the global total into a huge usize.
    #[test]
    fn release_saturates_at_zero() {
        let mut s = QuotaState::new();
        s.try_acquire("P", 5, 5).unwrap();
        s.release("P");
        assert_eq!(s.count_for("P"), 0);
        assert_eq!(s.total(), 0);
        // Extra releases are harmless no-ops, never underflow.
        s.release("P");
        s.release("P");
        s.release("never-acquired");
        assert_eq!(s.count_for("P"), 0);
        assert_eq!(s.total(), 0, "total must never underflow");
    }

    /// Two principals are independent: one exhausting its cap does not affect the
    /// other, and their counts/total stay consistent.
    #[test]
    fn two_principals_are_independent() {
        let mut s = QuotaState::new();
        let cap = 2;
        let global = 100;
        s.try_acquire("A", cap, global).unwrap();
        s.try_acquire("A", cap, global).unwrap();
        assert!(s.try_acquire("A", cap, global).is_err(), "A at cap");
        // B is untouched by A being full.
        assert!(s.try_acquire("B", cap, global).is_ok(), "B independent of A");
        assert_eq!(s.count_for("A"), 2);
        assert_eq!(s.count_for("B"), 1);
        assert_eq!(s.total(), 3);
        // Releasing A does not touch B.
        s.release("A");
        assert_eq!(s.count_for("A"), 1);
        assert_eq!(s.count_for("B"), 1);
    }

    /// A double-spawn-then-both-exit ends at exactly 0 (the leak-proofing
    /// invariant the child-exit observer relies on).
    #[test]
    fn double_acquire_then_both_release_ends_at_zero() {
        let mut s = QuotaState::new();
        s.try_acquire("P", 5, 5).unwrap();
        s.try_acquire("P", 5, 5).unwrap();
        assert_eq!(s.count_for("P"), 2);
        s.release("P");
        s.release("P");
        assert_eq!(s.count_for("P"), 0, "both exits must drain to 0");
        assert_eq!(s.total(), 0);
        // The map entry is pruned at 0 (no unbounded growth with retired keys).
        assert!(!s.per_principal.contains_key("P"));
    }

    /// The OWNER principal gets a much higher cap than an API-key principal (the
    /// own-use exemption), and both honor env overrides.
    #[test]
    fn owner_gets_high_cap_api_gets_standard_cap() {
        // Defaults: owner >> api.
        assert!(
            per_principal_cap_for("owner") > per_principal_cap_for("some-api-key-id"),
            "owner own-use cap must dominate the api-key cap",
        );
        assert_eq!(per_principal_cap_for("owner"), DEFAULT_OWNER_CELL_CAP);
        assert_eq!(per_principal_cap_for("api-key-123"), DEFAULT_PRINCIPAL_CELL_CAP);
    }

    /// `env_cap` falls back on absent / empty / unparsable / zero, and honors a
    /// valid positive override.
    #[test]
    fn env_cap_parsing_and_fallbacks() {
        // A var name unlikely to be set in the test env.
        let var = "K2_SANDBOX_TEST_CAP_UNSET_XYZ";
        std::env::remove_var(var);
        assert_eq!(env_cap(var, 7), 7, "absent → default");
        // Note: we set/remove a dedicated var to avoid cross-test pollution.
        std::env::set_var(var, "0");
        assert_eq!(env_cap(var, 7), 7, "zero is treated as unset (would brick)");
        std::env::set_var(var, "not-a-number");
        assert_eq!(env_cap(var, 7), 7, "unparsable → default");
        std::env::set_var(var, "  12 ");
        assert_eq!(env_cap(var, 7), 12, "valid positive override (trimmed)");
        std::env::remove_var(var);
    }

    /// Per-workspace ceiling via `try_acquire_ws`: N ok, N+1 → `WorkspaceCap`
    /// with machine code `workspace-cell-cap`. Refused acquire mutates nothing.
    #[test]
    fn workspace_cap_hits_workspace_cell_cap() {
        let mut s = QuotaState::new();
        let per_principal = 100; // non-binding
        let ws_cap = 3;
        let global = 1000; // non-binding
        let ws = "/tmp/ws-a";
        for i in 0..ws_cap {
            assert!(
                s.try_acquire_ws("P", Some(ws), per_principal, ws_cap, global)
                    .is_ok(),
                "workspace acquire {i} within cap must succeed",
            );
        }
        assert_eq!(s.workspace_count(ws), ws_cap);
        assert_eq!(s.count_for("P"), ws_cap);
        assert_eq!(s.total(), ws_cap);

        let err = s
            .try_acquire_ws("P", Some(ws), per_principal, ws_cap, global)
            .expect_err("N+1 workspace must be refused");
        assert_eq!(err, QuotaError::WorkspaceCap);
        assert_eq!(err.code(), "workspace-cell-cap");
        assert_eq!(
            s.workspace_count(ws),
            ws_cap,
            "refused workspace acquire must not increment"
        );
        assert_eq!(s.total(), ws_cap);

        // Independent workspace still admits.
        assert!(
            s.try_acquire_ws("P", Some("/tmp/ws-b"), per_principal, ws_cap, global)
                .is_ok(),
            "other workspace is independent of ws-a cap",
        );
        assert_eq!(s.workspace_count(ws), ws_cap);
        assert_eq!(s.workspace_count("/tmp/ws-b"), 1);

        // Release frees one workspace slot so the original admits again.
        s.release_ws("P", Some(ws));
        assert_eq!(s.workspace_count(ws), ws_cap - 1);
        assert!(
            s.try_acquire_ws("P", Some(ws), per_principal, ws_cap, global)
                .is_ok(),
            "freed workspace slot is reusable",
        );
        assert_eq!(s.workspace_count(ws), ws_cap);
    }

    /// Check order: principal → workspace → global (most specific first).
    #[test]
    fn workspace_cap_checked_before_global() {
        let mut s = QuotaState::new();
        // Workspace full at 1; global would also be full if we got there.
        s.try_acquire_ws("P", Some("ws"), 10, 1, 1).unwrap();
        let err = s
            .try_acquire_ws("P", Some("ws"), 10, 1, 1)
            .expect_err("must refuse");
        assert_eq!(
            err,
            QuotaError::WorkspaceCap,
            "workspace reported before global when both would bind"
        );
        assert_eq!(err.code(), "workspace-cell-cap");
    }

    /// Frozen 429 `code` strings for the integrator envelope (0.40.76).
    /// QueueTimeout is only produced by the process-global wait loop; pure
    /// `QuotaState` cannot emit it, but the code mapping must stay stable.
    #[test]
    fn quota_error_codes_match_contract() {
        assert_eq!(QuotaError::PerPrincipalCap.code(), "concurrent-cell-cap");
        assert_eq!(QuotaError::WorkspaceCap.code(), "workspace-cell-cap");
        assert_eq!(QuotaError::GlobalCap.code(), "cell-capacity");
        assert_eq!(QuotaError::QueueTimeout.code(), "spawn-queue-timeout");

        // Messages are non-empty and non-secret (no paths/ids).
        for e in [
            QuotaError::PerPrincipalCap,
            QuotaError::WorkspaceCap,
            QuotaError::GlobalCap,
            QuotaError::QueueTimeout,
        ] {
            assert!(!e.message().is_empty(), "{e:?} message empty");
            assert!(
                !e.message().contains('/'),
                "{e:?} message must not leak paths: {}",
                e.message()
            );
        }
    }

}
