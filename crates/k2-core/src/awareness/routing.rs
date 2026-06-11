//! Routing resolver — pure function from `(signal, target_state)`
//! to `DeliveryPlan`.
//!
//! E3 of Phase 3. No I/O here, no side effects. This module answers
//! the question "given this signal and what we know about its
//! target right now, which egress channels should the composer
//! fire?" E4's `egress::deliver` takes the plan and does the
//! actual work.
//!
//! **Egress matrix** (locked by the plan file
//! `~/.claude/plans/happy-hatching-locket.md`):
//!
//! | Sender chose... | Target LIVE | Target OFFLINE |
//! |---|---|---|
//! | `Delivery::Live` | PTY-inject + bus | wake + queue for inject; bus; (no inbox) |
//! | `Delivery::Inbox` | inbox file; bus; (no wake) | inbox file; bus; (no wake) |
//!
//! Bus broadcast always fires (so subscribers like the daemon's
//! `/cli/awareness/subscribe` WS see every signal). activity_feed
//! write also always fires — both are audit-grade, never skipped.

use crate::awareness::{AgentAddress, AgentSignal, Delivery};

/// What the composer should do with this signal. One-shot
/// instruction; composed at call time, not stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryPlan {
    /// The agent-name slot to deliver to. `None` when the signal's
    /// address is `AgentAddress::Broadcast` or `Workspace` — those
    /// paths fan out in the caller (one Plan per target).
    pub target_agent: Option<String>,
    /// Inject signal text directly into the target's running
    /// session via `terminal_manager.write()`. Only set for
    /// `Delivery::Live` + live target.
    pub inject_to_pty: bool,
    /// Trigger `scheduler::wake_agent()` for this target before
    /// injecting. Only set for `Delivery::Live` + offline target.
    /// After wake, signal is queued for on-session-ready delivery.
    pub wake_target: bool,
    /// Write a durable file to the target's inbox. Only set for
    /// `Delivery::Inbox`.
    pub write_to_inbox: bool,
    /// Broadcast on the in-memory bus. ALWAYS true — bus is the
    /// audit subscription surface.
    pub publish_to_bus: bool,
    /// Insert a row into `activity_feed`. ALWAYS true — audit log.
    pub write_activity_feed: bool,
}

/// What the caller knows about the target at plan time. The
/// composer computes this from `session::registry` + `agent_sessions`
/// DB right before calling `resolve`. Kept as a plain enum so the
/// resolver stays pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetState {
    Live,
    Offline,
}

/// Resolve a single signal + target state to a `DeliveryPlan`.
///
/// For `AgentAddress::Agent` the plan targets a specific agent
/// directly. For `AgentAddress::Workspace` and
/// `AgentAddress::Broadcast` the returned plan has
/// `target_agent: None` — those paths require the composer to
/// enumerate members and call `resolve_for_agent` for each.
///
/// The resolver is stateless. Same inputs → same plan. Good for
/// unit tests, for dry-running delivery before committing, and for
/// feeding a UI that previews "what would sending this message
/// actually do?"
pub fn resolve(signal: &AgentSignal, target_state: TargetState) -> DeliveryPlan {
    match &signal.to {
        AgentAddress::Agent { name, .. } => {
            resolve_for_agent(signal, Some(name.clone()), target_state)
        }
        AgentAddress::Workspace { .. } | AgentAddress::Broadcast => {
            // Multi-target — the composer fans out. Plan here just
            // captures the "always" channels; fanout happens upstream.
            DeliveryPlan {
                target_agent: None,
                inject_to_pty: false,
                wake_target: false,
                write_to_inbox: false,
                publish_to_bus: true,
                write_activity_feed: true,
            }
        }
    }
}

/// Build a plan for a specific agent target. Used by `resolve` for
/// `AgentAddress::Agent` and by the composer when fanning out
/// `Workspace` / `Broadcast` across multiple agents.
pub fn resolve_for_agent(
    signal: &AgentSignal,
    target_agent: Option<String>,
    target_state: TargetState,
) -> DeliveryPlan {
    match (signal.delivery, target_state) {
        (Delivery::Live, TargetState::Live) => DeliveryPlan {
            target_agent,
            inject_to_pty: true,
            wake_target: false,
            write_to_inbox: false,
            publish_to_bus: true,
            write_activity_feed: true,
        },
        (Delivery::Live, TargetState::Offline) => DeliveryPlan {
            target_agent,
            inject_to_pty: false,
            wake_target: true,
            write_to_inbox: false,
            publish_to_bus: true,
            write_activity_feed: true,
        },
        (Delivery::Inbox, _) => DeliveryPlan {
            target_agent,
            inject_to_pty: false,
            wake_target: false,
            write_to_inbox: true,
            publish_to_bus: true,
            write_activity_feed: true,
        },
    }
}
