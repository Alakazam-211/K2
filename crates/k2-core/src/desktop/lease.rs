//! The control lease: one controller at a time, the others watch; local
//! input wins (S4).
//!
//! `sim::hold` models one person against an agent (vs-live R14), so this
//! is its own small state machine; it borrows only the sim's timings:
//! [`PERSON_HOLD`] (3 s: physical input at the machine pauses remote
//! input) and [`TAKEOVER_IDLE`] (10 min: an idle controller drops to
//! watching, S8). Input that arrives during a local hold is dropped, never
//! queued (queued text would land in whatever the person just clicked);
//! deskd enforces the hold at the moment it sees local input, and this
//! copy tells viewers (`control {localHold: true}`).

use std::time::{Duration, Instant};

pub use crate::sim::hold::{PERSON_HOLD, TAKEOVER_IDLE};

/// A viewer's id in the registry ([`super::viewers`]).
pub type ViewerId = String;

/// Why a take was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TakeRefused {
    /// Someone else drives (their display name).
    HeldBy(String),
}

/// What changed (for the audit line and the `control` message).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseChange {
    Taken { viewer: ViewerId },
    HandedBack { viewer: ViewerId },
    /// The controller sat idle for [`TAKEOVER_IDLE`].
    TimedOut { viewer: ViewerId },
    /// The controller's socket closed.
    Dropped { viewer: ViewerId },
}

#[derive(Debug, Clone)]
struct Holder {
    viewer: ViewerId,
    name: String,
    last_input: Instant,
}

/// One desktop's lease.
#[derive(Debug, Clone, Default)]
pub struct Lease {
    holder: Option<Holder>,
    local_until: Option<Instant>,
}

impl Lease {
    pub fn new() -> Self {
        Self::default()
    }

    /// The controller's name, if any.
    pub fn controller(&self) -> Option<&str> {
        self.holder.as_ref().map(|h| h.name.as_str())
    }

    pub fn holds(&self, viewer: &str) -> bool {
        self.holder.as_ref().is_some_and(|h| h.viewer == viewer)
    }

    /// Take control. Free, already ours, or the holder idle past
    /// [`TAKEOVER_IDLE`]: taken. Else refused with who drives.
    pub fn take(&mut self, viewer: &str, name: &str, now: Instant) -> Result<Option<LeaseChange>, TakeRefused> {
        if let Some(h) = &self.holder {
            if h.viewer == viewer {
                return Ok(None);
            }
            if now.saturating_duration_since(h.last_input) < TAKEOVER_IDLE {
                return Err(TakeRefused::HeldBy(h.name.clone()));
            }
        }
        self.holder = Some(Holder { viewer: viewer.to_string(), name: name.to_string(), last_input: now });
        Ok(Some(LeaseChange::Taken { viewer: viewer.to_string() }))
    }

    pub fn hand_back(&mut self, viewer: &str) -> Option<LeaseChange> {
        if self.holds(viewer) {
            self.holder = None;
            Some(LeaseChange::HandedBack { viewer: viewer.to_string() })
        } else {
            None
        }
    }

    /// The viewer's socket closed.
    pub fn drop_viewer(&mut self, viewer: &str) -> Option<LeaseChange> {
        if self.holds(viewer) {
            self.holder = None;
            Some(LeaseChange::Dropped { viewer: viewer.to_string() })
        } else {
            None
        }
    }

    /// Physical input at the machine: remote input pauses for
    /// [`PERSON_HOLD`]. Returns true when a hold starts (not extends).
    pub fn local_input(&mut self, now: Instant) -> bool {
        let started = !self.local_hold(now);
        self.local_until = Some(now + PERSON_HOLD);
        started
    }

    pub fn local_hold(&self, now: Instant) -> bool {
        self.local_until.is_some_and(|t| now < t)
    }

    /// May `viewer` inject this input now? Only the controller, and never
    /// during a local hold (the input is dropped, not queued). An accepted
    /// input keeps the lease fresh.
    pub fn admit_input(&mut self, viewer: &str, now: Instant) -> bool {
        if !self.holds(viewer) || self.local_hold(now) {
            return false;
        }
        if let Some(h) = self.holder.as_mut() {
            h.last_input = now;
        }
        true
    }

    /// The 10-minute idle rule (call on the stream's tick).
    pub fn tick(&mut self, now: Instant) -> Option<LeaseChange> {
        let h = self.holder.as_ref()?;
        if now.saturating_duration_since(h.last_input) >= TAKEOVER_IDLE {
            let viewer = h.viewer.clone();
            self.holder = None;
            return Some(LeaseChange::TimedOut { viewer });
        }
        None
    }

    /// Time left in a local hold (for the tab's readout).
    pub fn hold_left(&self, now: Instant) -> Duration {
        self.local_until.map(|t| t.saturating_duration_since(now)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_controller_at_a_time() {
        let t = Instant::now();
        let mut l = Lease::new();
        assert_eq!(l.take("v1", "rosson", t), Ok(Some(LeaseChange::Taken { viewer: "v1".into() })));
        assert_eq!(l.take("v1", "rosson", t), Ok(None), "already ours");
        assert_eq!(l.take("v2", "ann", t), Err(TakeRefused::HeldBy("rosson".into())));
        assert!(l.admit_input("v1", t));
        assert!(!l.admit_input("v2", t), "a watcher never injects");
        assert_eq!(l.hand_back("v2"), None, "not v2's to hand back");
        assert_eq!(l.hand_back("v1"), Some(LeaseChange::HandedBack { viewer: "v1".into() }));
        assert_eq!(l.controller(), None);
        assert!(l.take("v2", "ann", t).is_ok());
        assert_eq!(l.drop_viewer("v2"), Some(LeaseChange::Dropped { viewer: "v2".into() }));
    }

    #[test]
    fn local_input_wins_for_three_seconds_and_input_is_dropped_not_queued() {
        let t = Instant::now();
        let mut l = Lease::new();
        l.take("v1", "rosson", t).unwrap();
        assert!(l.local_input(t), "a hold starts");
        assert!(!l.local_input(t + Duration::from_secs(1)), "extends, doesn't restart");
        assert!(!l.admit_input("v1", t + Duration::from_millis(3500)), "inside the extended hold");
        assert!(l.admit_input("v1", t + Duration::from_secs(4) + Duration::from_millis(1)), "after it");
        assert!(l.holds("v1"), "a local hold pauses, it doesn't take the lease");
        assert_eq!(PERSON_HOLD, Duration::from_secs(3));
    }

    #[test]
    fn an_idle_controller_drops_to_watching_after_ten_minutes() {
        let t = Instant::now();
        let mut l = Lease::new();
        l.take("v1", "rosson", t).unwrap();
        assert_eq!(l.tick(t + TAKEOVER_IDLE - Duration::from_secs(1)), None);
        assert!(l.take("v2", "ann", t + TAKEOVER_IDLE - Duration::from_secs(1)).is_err());
        assert_eq!(l.tick(t + TAKEOVER_IDLE), Some(LeaseChange::TimedOut { viewer: "v1".into() }));
        assert_eq!(l.controller(), None);
        // A take after the holder's idle window succeeds even without a tick.
        let mut l = Lease::new();
        l.take("v1", "rosson", t).unwrap();
        assert!(l.take("v2", "ann", t + TAKEOVER_IDLE).is_ok());
        assert_eq!(TAKEOVER_IDLE, Duration::from_secs(600));
    }
}
