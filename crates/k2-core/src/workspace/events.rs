//! Channel event queue — cross-workspace / background-push inbox for
//! agents running on the adaptive-heartbeat channel model.
//!
//! When a new work item lands in an agent's inbox while that agent
//! isn't actively connected (e.g. via `k2so work send`, or any other
//! inbox writer), K2SO drops a [`ChannelEvent`] into a per-agent
//! queue here. The agent's next wake (via `k2so checkin`
//! or the `/cli/events` drain route) reads them all in one shot,
//! empties the queue, and acts on them.
//!
//! Queue capacity per agent is bounded by [`MAX_EVENTS_PER_QUEUE`] so
//! a runaway producer can't OOM the process. Oldest events evict when
//! the cap is hit. That's the trade-off: this is a "last-N messages"
//! model, not a durable queue.
//!
//! Serialization format (`type` / `message` / `priority` / `timestamp`)
//! matches what the CLI's `k2so events` command consumes and what
//! agents see in their wake context.

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

use parking_lot::Mutex;

pub const MAX_EVENTS_PER_QUEUE: usize = 100;

#[derive(Clone, Debug, serde::Serialize)]
pub struct ChannelEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub message: String,
    pub priority: String,
    pub timestamp: String,
}

static EVENT_QUEUES: OnceLock<Mutex<HashMap<String, VecDeque<ChannelEvent>>>> = OnceLock::new();

fn event_queues() -> &'static Mutex<HashMap<String, VecDeque<ChannelEvent>>> {
    EVENT_QUEUES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Push an event onto a specific agent's queue. The key is
/// `"<project_path>:<agent_name>"` so the same agent template running
/// in multiple workspaces gets separate queues.
pub fn push_agent_event(
    project_path: &str,
    agent_name: &str,
    event_type: &str,
    message: &str,
    priority: &str,
) {
    let key = format!("{}:{}", project_path, agent_name);
    let event = ChannelEvent {
        event_type: event_type.to_string(),
        message: message.to_string(),
        priority: priority.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    let mut queues = event_queues().lock();
    let queue = queues.entry(key).or_insert_with(VecDeque::new);
    queue.push_back(event);
    while queue.len() > MAX_EVENTS_PER_QUEUE {
        queue.pop_front();
    }
}

/// Drain every queued event for an agent and clear its queue in one
/// atomic operation. Returns the events oldest-first so the caller
/// can act on them in arrival order.
pub fn drain_agent_events(project_path: &str, agent_name: &str) -> Vec<ChannelEvent> {
    let key = format!("{}:{}", project_path, agent_name);
    let mut queues = event_queues().lock();
    queues
        .remove(&key)
        .map(|q| q.into_iter().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_then_drain_roundtrips() {
        let p = format!("/tmp/test-{}", uuid::Uuid::new_v4());
        push_agent_event(&p, "alice", "msg", "hello", "normal");
        push_agent_event(&p, "alice", "msg", "world", "high");
        let drained = drain_agent_events(&p, "alice");
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].message, "hello");
        assert_eq!(drained[1].message, "world");
        // Second drain should be empty.
        assert!(drain_agent_events(&p, "alice").is_empty());
    }

    #[test]
    fn bounded_queue_evicts_oldest() {
        let p = format!("/tmp/evict-{}", uuid::Uuid::new_v4());
        for i in 0..(MAX_EVENTS_PER_QUEUE + 10) {
            push_agent_event(&p, "bob", "msg", &format!("msg{}", i), "normal");
        }
        let drained = drain_agent_events(&p, "bob");
        assert_eq!(drained.len(), MAX_EVENTS_PER_QUEUE);
        // Oldest msg0..msg9 should have been evicted.
        assert_eq!(drained[0].message, "msg10");
    }

    #[test]
    fn separate_agents_have_separate_queues() {
        let p = format!("/tmp/iso-{}", uuid::Uuid::new_v4());
        push_agent_event(&p, "a", "msg", "for-a", "normal");
        push_agent_event(&p, "b", "msg", "for-b", "normal");
        let drained_a = drain_agent_events(&p, "a");
        let drained_b = drain_agent_events(&p, "b");
        assert_eq!(drained_a.len(), 1);
        assert_eq!(drained_a[0].message, "for-a");
        assert_eq!(drained_b.len(), 1);
        assert_eq!(drained_b[0].message, "for-b");
    }
}
