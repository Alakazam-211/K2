//! One delivery path for automatic wakes of an agent (prd-routines-reminders-
//! and-agent-mail-v1 §4.8, vs-live W38). Sources, reminders and mail all
//! deliver through here, so an agent takes one automatic delivery at a time
//! and every one of them is counted by the same budget
//! ([`k2_core::wake_budget`]). The agent is the key, not the source.
//!
//! Anything else that wants to wake an agent joins by calling [`deliver`].

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use crate::workspace_msg::{self, MsgResponse};

/// One per agent, created the first time it is needed.
static AGENT_LOCKS: LazyLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The lock for `agent` (a workspace id, or whatever token the caller holds
/// consistently). Poisoning is cleared: a panicked delivery must not stop
/// every later wake of that agent.
fn agent_lock(agent: &str) -> Arc<Mutex<()>> {
    let mut locks = AGENT_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    locks.entry(agent.to_string()).or_default().clone()
}

/// Run `f` while this agent takes no other automatic delivery.
pub fn with_agent<R>(agent: &str, f: impl FnOnce() -> R) -> R {
    let lock = agent_lock(agent);
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    f()
}

/// One automatic wake.
#[derive(Debug, Clone, Copy)]
pub struct Wake<'a> {
    /// Workspace token the message path resolves (a path, an id or a name).
    pub workspace: &'a str,
    /// Key for the one-at-a-time lane; the workspace id where the caller has it.
    pub agent: &'a str,
    pub text: &'a str,
    /// The stamp: the agent reads `[from <from>] <text>`.
    pub from: &'a str,
    /// Chatter's `via`.
    pub via: &'a str,
    pub wake_timeout: Duration,
}

/// Deliver one automatic wake, on this agent's lane. A dormant agent is
/// woken: that is what these are for.
pub fn deliver(w: Wake<'_>) -> MsgResponse {
    with_agent(w.agent, || {
        workspace_msg::deliver_live_with_via(w.workspace, w.text, w.from, "", true, w.wake_timeout, w.via)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn one_delivery_per_agent_at_a_time_and_agents_do_not_block_each_other() {
        let started = Arc::new(AtomicBool::new(false));
        let holder = {
            let started = started.clone();
            std::thread::spawn(move || {
                with_agent("agent-a", || {
                    started.store(true, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(300));
                })
            })
        };
        while !started.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let t0 = std::time::Instant::now();
        with_agent("agent-b", || {});
        assert!(t0.elapsed() < Duration::from_millis(200), "another agent's lane is free");
        with_agent("agent-a", || {});
        assert!(t0.elapsed() >= Duration::from_millis(200), "the same agent waits its turn");
        holder.join().expect("holder");
    }
}
