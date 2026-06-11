//! Per-sender coordination-level budget tracker.
//!
//! G5 of Phase 3.2. The bus + egress composer is the natural
//! choke-point to enforce per-agent emit rate limits, because
//! every cross-agent signal flows through it regardless of whether
//! it originated from a CLI call, an APC escape inside a live PTY,
//! or the daemon itself. This module owns the in-memory counters;
//! `egress::deliver` calls `check_and_increment` at the top of the
//! delivery path and short-circuits with an audit-only `Deny`
//! report when a sender is over budget.
//!
//! **Budget vocabulary** comes from G3's `CoordinationLevel`
//! enum — four levels, each mapping to a concrete per-key emit
//! count (`None=0`, `Minimal=2`, `Moderate=5`, `Chatty=10`). An
//! agent's level lives in its `AGENT.md` frontmatter. CLI-sourced
//! signals (`from.name == "cli"`) bypass budgets entirely — human
//! senders aren't subject to agent coordination limits.
//!
//! **Bypass paths:**
//! - `Priority::Urgent` — emergencies always pass.
//! - `SignalKind::Status` — telemetry ("I'm working on X"), not
//!   coordination. Doesn't count against budget.
//! - Sender name `"cli"` — human-triggered. Unlimited.
//!
//! **Key granularity.** `(workspace_id, from_agent)` — one counter
//! per sender per workspace. An agent that works in two workspaces
//! has separate budgets in each. Counters reset on daemon restart
//! (sessions are ephemeral; the "per-session" scope in the PRD
//! approximates to "per-daemon-lifetime" for today). A future
//! commit can wire per-session reset through `session::registry`
//! if real-world usage demands it.
//!
//! **Denials are audited, not silent.** `egress::deliver` writes
//! an activity_feed row with `event_type='signal:budget-denied'`
//! when this fn returns `Deny`. The full AgentSignal JSON is still
//! stored in `metadata`, so higher-level tooling can reconstruct
//! WHY the signal was dropped + what it would have said.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::workspace::launch_profile::CoordinationLevel;

/// Shared budget counter, keyed by `(workspace_id, from_agent)`.
/// Value = number of signals this sender has emitted so far this
/// daemon run. Incremented on allow, never on deny or bypass —
/// denials don't "use up" budget, so a single over-limit burst
/// is denied once, not compounded on retry.
static COUNTERS: OnceLock<Mutex<HashMap<(String, String), u32>>> = OnceLock::new();

fn counters() -> &'static Mutex<HashMap<(String, String), u32>> {
    COUNTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Outcome of `check_and_increment`. `Allow` means the sender is
/// within budget; the counter has been incremented as part of the
/// call. `Deny` means the sender is over budget; the counter is
/// unchanged and the caller should skip delivery + write an audit
/// row for the denial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetDecision {
    Allow,
    Deny {
        /// Coordination level this sender is configured for.
        level: CoordinationLevel,
        /// Concrete budget (emits/session) for the level.
        budget: u32,
        /// Current count — always `>= budget` here.
        used: u32,
    },
}

impl BudgetDecision {
    /// Convenience: `true` iff this decision is a `Deny`.
    pub fn is_deny(self) -> bool {
        matches!(self, BudgetDecision::Deny { .. })
    }
}

/// Hint that `from_agent` is a human-triggered CLI sender, not an
/// agent with a coordination level. CLI senders bypass budgets
/// regardless of their `from.name` string — except the literal
/// string `"cli"` is the conventional human-sent name (see
/// `cli/k2so::cmd_signal`). Other tokens (e.g. a real agent name
/// that happens to match) still go through budget checks.
pub const CLI_SENDER_NAME: &str = "cli";

/// Inputs for a budget decision. Named struct rather than a pile
/// of bool arguments so call sites at egress are readable.
#[derive(Debug)]
pub struct BudgetCheck<'a> {
    /// `projects.id` (UUID after G0) from the signal's `from.workspace`.
    pub workspace_id: &'a str,
    /// Sender agent name from `signal.from.name`.
    pub from_agent: &'a str,
    /// Sender's coordination level (loaded from AGENT.md by caller).
    pub level: CoordinationLevel,
    /// `true` if `signal.kind` is Status — exempt from budget.
    pub is_status: bool,
    /// `true` if `signal.priority == Urgent` — exempt from budget.
    pub is_urgent: bool,
}

/// Core budget decision + counter increment. Pure enough to test
/// deterministically: the only state is the shared counter, which
/// tests can reset via `reset_for_tests`.
pub fn check_and_increment(check: &BudgetCheck<'_>) -> BudgetDecision {
    // Bypasses: CLI senders + Urgent + Status never count against
    // budget. Ordered by cheapness — name compare is free, priority
    // + kind checks are bool reads.
    if check.from_agent == CLI_SENDER_NAME {
        return BudgetDecision::Allow;
    }
    if check.is_urgent || check.is_status {
        return BudgetDecision::Allow;
    }

    let budget = check.level.budget();
    let key = (
        check.workspace_id.to_string(),
        check.from_agent.to_string(),
    );
    let mut map = counters().lock().unwrap_or_else(|e| e.into_inner());
    let used = *map.get(&key).unwrap_or(&0);
    if used >= budget {
        return BudgetDecision::Deny {
            level: check.level,
            budget,
            used,
        };
    }
    map.insert(key, used + 1);
    BudgetDecision::Allow
}

/// Current emit count for a sender. Test helper — production code
/// has no reason to peek.
#[cfg(any(test, feature = "test-util"))]
pub fn current_count(workspace_id: &str, from_agent: &str) -> u32 {
    let map = counters().lock().unwrap_or_else(|e| e.into_inner());
    *map
        .get(&(workspace_id.to_string(), from_agent.to_string()))
        .unwrap_or(&0)
}

/// Drop every counter. Call at the top of tests that assert on
/// budget state so parallel test runs + reruns-after-panic don't
/// poison the shared map.
#[cfg(any(test, feature = "test-util"))]
pub fn reset_for_tests() {
    counters()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn check(
        workspace: &str,
        agent: &str,
        level: CoordinationLevel,
    ) -> BudgetDecision {
        check_and_increment(&BudgetCheck {
            workspace_id: workspace,
            from_agent: agent,
            level,
            is_status: false,
            is_urgent: false,
        })
    }

    #[test]
    fn allow_within_budget_then_deny_past_it() {
        let _g = lock();
        reset_for_tests();
        // Minimal level = 2 emits.
        assert_eq!(check("ws", "agent-a", CoordinationLevel::Minimal), BudgetDecision::Allow);
        assert_eq!(check("ws", "agent-a", CoordinationLevel::Minimal), BudgetDecision::Allow);
        let deny = check("ws", "agent-a", CoordinationLevel::Minimal);
        assert!(deny.is_deny(), "third emit at Minimal should deny");
        if let BudgetDecision::Deny { budget, used, level } = deny {
            assert_eq!(level, CoordinationLevel::Minimal);
            assert_eq!(budget, 2);
            assert_eq!(used, 2);
        }
    }

    #[test]
    fn denied_emits_dont_increment_counter() {
        let _g = lock();
        reset_for_tests();
        // Blow past the 2-emit Minimal budget intentionally.
        for _ in 0..5 {
            check("ws", "quiet", CoordinationLevel::Minimal);
        }
        // Counter should be exactly 2 (the allowed ones) — denies
        // don't increment, so the sender isn't "further and further
        // behind" after repeated over-budget attempts.
        assert_eq!(current_count("ws", "quiet"), 2);
    }

    #[test]
    fn cli_sender_is_unlimited() {
        let _g = lock();
        reset_for_tests();
        for _ in 0..100 {
            let d = check("ws", CLI_SENDER_NAME, CoordinationLevel::None);
            assert_eq!(d, BudgetDecision::Allow, "cli must never be denied");
        }
        // Bypass path doesn't increment the counter either.
        assert_eq!(current_count("ws", CLI_SENDER_NAME), 0);
    }

    #[test]
    fn urgent_priority_bypasses_budget() {
        let _g = lock();
        reset_for_tests();
        // Exhaust the Moderate budget (5).
        for _ in 0..5 {
            check("ws", "urgent-agent", CoordinationLevel::Moderate);
        }
        // Normal emit is denied.
        assert!(check("ws", "urgent-agent", CoordinationLevel::Moderate).is_deny());
        // But urgent passes.
        let d = check_and_increment(&BudgetCheck {
            workspace_id: "ws",
            from_agent: "urgent-agent",
            level: CoordinationLevel::Moderate,
            is_status: false,
            is_urgent: true,
        });
        assert_eq!(d, BudgetDecision::Allow);
    }

    #[test]
    fn status_kind_bypasses_budget() {
        let _g = lock();
        reset_for_tests();
        // Exhaust the Minimal budget (2) with Msg emits.
        check("ws", "status-agent", CoordinationLevel::Minimal);
        check("ws", "status-agent", CoordinationLevel::Minimal);
        // Status emits keep passing.
        for _ in 0..10 {
            let d = check_and_increment(&BudgetCheck {
                workspace_id: "ws",
                from_agent: "status-agent",
                level: CoordinationLevel::Minimal,
                is_status: true,
                is_urgent: false,
            });
            assert_eq!(d, BudgetDecision::Allow);
        }
    }

    #[test]
    fn none_level_denies_first_emit() {
        let _g = lock();
        reset_for_tests();
        // CoordinationLevel::None = budget 0 → even the first emit
        // is over budget. Guard for "silent agents" that should
        // never speak to the bus.
        let d = check("ws", "silent", CoordinationLevel::None);
        assert!(d.is_deny());
        if let BudgetDecision::Deny { budget, used, .. } = d {
            assert_eq!(budget, 0);
            assert_eq!(used, 0);
        }
    }

    #[test]
    fn workspaces_are_isolated() {
        let _g = lock();
        reset_for_tests();
        // Same agent name, different workspaces. The Minimal=2
        // budget per workspace means `agent-x` can emit 2 in ws-a
        // AND 2 in ws-b before either is denied.
        for _ in 0..2 {
            assert_eq!(
                check("ws-a", "agent-x", CoordinationLevel::Minimal),
                BudgetDecision::Allow
            );
            assert_eq!(
                check("ws-b", "agent-x", CoordinationLevel::Minimal),
                BudgetDecision::Allow
            );
        }
        assert!(check("ws-a", "agent-x", CoordinationLevel::Minimal).is_deny());
        assert!(check("ws-b", "agent-x", CoordinationLevel::Minimal).is_deny());
    }

    #[test]
    fn chatty_level_maps_to_ten_emits() {
        let _g = lock();
        reset_for_tests();
        for _ in 0..10 {
            assert_eq!(
                check("ws", "verbose", CoordinationLevel::Chatty),
                BudgetDecision::Allow
            );
        }
        assert!(check("ws", "verbose", CoordinationLevel::Chatty).is_deny());
    }
}
