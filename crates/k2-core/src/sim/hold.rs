//! The control hold: when a person and the agent both drive the device,
//! the person wins (`prd-mobile-dev-v1` §5). Lane-blind and pure (no
//! clock reads): the daemon passes `now`.
//!
//! - Every person input holds the device for [`PERSON_HOLD`] (rolling).
//! - An agent input inside a hold waits up to [`AGENT_WAIT`], then fails
//!   with exit 2 `held: a person is using the device`.
//! - **Take over** holds the device until **Hand back**, or until
//!   [`TAKEOVER_IDLE`] with no input from the person who took it.
//! - After a hand-back (or a take-over that timed out) the agent's next
//!   input is refused once with "run `k2 sim tree` first" unless it read
//!   the tree since.
//! - Reads (tree, screenshot, logs) never wait (Q5).

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub const PERSON_HOLD: Duration = Duration::from_secs(3);
pub const AGENT_WAIT: Duration = Duration::from_secs(2);
pub const TAKEOVER_IDLE: Duration = Duration::from_secs(10 * 60);

/// Who holds the device right now (what clients draw).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum HoldView {
    Free,
    /// A person is tapping; free again in `ms`.
    Person { by: String, ms: u64 },
    /// Taken over until Hand back (or idle timeout in `idleMs`).
    Takeover {
        by: String,
        #[serde(rename = "idleMs")]
        idle_ms: u64,
    },
}

/// Why the agent may not act right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentBlocked {
    /// A person holds the device; retry after this long (`None` = until
    /// Hand back).
    Held { by: String, retry_after: Option<Duration> },
    /// Control just came back; read the screen first.
    ReadTreeFirst,
}

impl AgentBlocked {
    pub fn message(&self) -> String {
        match self {
            AgentBlocked::Held { by, retry_after: None } => {
                format!("held: {by} has taken over the device. Wait for Hand back (`k2 sim wait`); you can still read `k2 sim tree`.")
            }
            AgentBlocked::Held { .. } => {
                "held: a person is using the device. Try again in a moment (`k2 sim wait`).".to_string()
            }
            AgentBlocked::ReadTreeFirst => {
                "the person handed the device back and the screen may have changed. Run `k2 sim tree` before acting.".to_string()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Takeover {
    by: String,
    last_input: Instant,
}

/// Per-sim-session hold state.
#[derive(Debug, Clone, Default)]
pub struct ControlHold {
    person: Option<(String, Instant)>,
    takeover: Option<Takeover>,
    /// Set on hand-back / takeover expiry; cleared when the agent reads the
    /// tree.
    agent_must_read: bool,
}

/// What [`ControlHold::tick`] noticed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoldEvent {
    TakeoverExpired { by: String },
}

impl ControlHold {
    pub fn new() -> ControlHold {
        ControlHold::default()
    }

    /// A person's input reached the device.
    pub fn person_input(&mut self, who: &str, now: Instant) {
        self.person = Some((who.to_string(), now + PERSON_HOLD));
        if let Some(t) = self.takeover.as_mut() {
            if t.by == who {
                t.last_input = now;
            }
        }
    }

    /// Take over (idempotent for the same person; a second person takes it
    /// from the first, as taps run in arrival order).
    pub fn take_over(&mut self, who: &str, now: Instant) {
        self.takeover = Some(Takeover { by: who.to_string(), last_input: now });
    }

    /// Hand back. Returns true when there was a take-over to end. Anyone
    /// who can drive the device may hand back (the tab shows who holds it).
    pub fn hand_back(&mut self, _who: &str, _now: Instant) -> bool {
        if self.takeover.take().is_some() {
            self.agent_must_read = true;
            true
        } else {
            false
        }
    }

    /// Expire an idle take-over. Call on each sweep and before checks.
    pub fn tick(&mut self, now: Instant) -> Option<HoldEvent> {
        if let Some(t) = &self.takeover {
            if now.saturating_duration_since(t.last_input) >= TAKEOVER_IDLE {
                let by = t.by.clone();
                self.takeover = None;
                self.agent_must_read = true;
                return Some(HoldEvent::TakeoverExpired { by });
            }
        }
        if let Some((_, until)) = &self.person {
            if now >= *until {
                self.person = None;
            }
        }
        None
    }

    /// The agent read the tree: it may act again after a hand-back.
    pub fn agent_read_tree(&mut self) {
        self.agent_must_read = false;
    }

    /// May the agent send input now?
    pub fn agent_check(&mut self, now: Instant) -> Result<(), AgentBlocked> {
        self.tick(now);
        if let Some(t) = &self.takeover {
            return Err(AgentBlocked::Held { by: t.by.clone(), retry_after: None });
        }
        if let Some((by, until)) = &self.person {
            if now < *until {
                return Err(AgentBlocked::Held { by: by.clone(), retry_after: Some(*until - now) });
            }
        }
        if self.agent_must_read {
            return Err(AgentBlocked::ReadTreeFirst);
        }
        Ok(())
    }

    pub fn view(&self, now: Instant) -> HoldView {
        if let Some(t) = &self.takeover {
            let idle = TAKEOVER_IDLE.saturating_sub(now.saturating_duration_since(t.last_input));
            return HoldView::Takeover { by: t.by.clone(), idle_ms: idle.as_millis() as u64 };
        }
        if let Some((by, until)) = &self.person {
            if now < *until {
                return HoldView::Person { by: by.clone(), ms: (*until - now).as_millis() as u64 };
            }
        }
        HoldView::Free
    }

    pub fn taken_over_by(&self) -> Option<&str> {
        self.takeover.as_ref().map(|t| t.by.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn person_tap_holds_for_three_seconds_rolling() {
        let t0 = Instant::now();
        let mut h = ControlHold::new();
        assert_eq!(h.agent_check(t0), Ok(()));
        h.person_input("rosson", t0);
        match h.agent_check(t0 + Duration::from_millis(500)) {
            Err(AgentBlocked::Held { retry_after: Some(d), .. }) => assert_eq!(d, Duration::from_millis(2500)),
            other => panic!("expected held, got {other:?}"),
        }
        h.person_input("rosson", t0 + Duration::from_secs(2));
        assert!(h.agent_check(t0 + Duration::from_millis(4500)).is_err(), "rolling window");
        assert_eq!(h.agent_check(t0 + Duration::from_secs(5)), Ok(()));
        assert_eq!(h.view(t0 + Duration::from_secs(6)), HoldView::Free);
    }

    #[test]
    fn takeover_holds_until_hand_back_then_agent_must_read_tree() {
        let t0 = Instant::now();
        let mut h = ControlHold::new();
        h.take_over("rosson", t0);
        let later = t0 + Duration::from_secs(60);
        assert_eq!(h.agent_check(later), Err(AgentBlocked::Held { by: "rosson".into(), retry_after: None }));
        assert!(h.hand_back("rosson", later));
        assert_eq!(h.agent_check(later), Err(AgentBlocked::ReadTreeFirst));
        h.agent_read_tree();
        assert_eq!(h.agent_check(later), Ok(()));
        assert!(!h.hand_back("rosson", later), "nothing to hand back");
    }

    #[test]
    fn takeover_expires_after_ten_idle_minutes() {
        let t0 = Instant::now();
        let mut h = ControlHold::new();
        h.take_over("rosson", t0);
        h.person_input("rosson", t0 + Duration::from_secs(300));
        assert_eq!(h.tick(t0 + Duration::from_secs(600)), None, "input refreshed the idle clock");
        assert_eq!(h.tick(t0 + Duration::from_secs(900)), Some(HoldEvent::TakeoverExpired { by: "rosson".into() }));
        assert_eq!(h.agent_check(t0 + Duration::from_secs(901)), Err(AgentBlocked::ReadTreeFirst));
    }
}
