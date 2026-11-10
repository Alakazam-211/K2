//! "Local input wins" (S4, vs-live R14): any physical input at the machine
//! holds remote input off for [`LOCAL_HOLD`], rolling. Enforced here, at
//! the moment deskd sees the local event, so queued remote input can't land
//! after the person has started typing. Remote input that arrives during a
//! hold is **dropped, not queued** (queued `text` would type into whatever
//! the person just clicked).
//!
//! Lock-free: the macOS event tap's callback and the X11 XInput2 thread call
//! [`LocalHold::note_local`] on every local event, so it is one atomic store.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// `sim::hold::PERSON_HOLD` (3 s), borrowed as a constant only (R14).
pub const LOCAL_HOLD: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub struct LocalHold {
    epoch: Instant,
    /// ns since `epoch` of the last local event, plus one; 0 = never.
    last: AtomicU64,
}

impl Default for LocalHold {
    fn default() -> Self {
        LocalHold::new(Instant::now())
    }
}

impl LocalHold {
    pub fn new(epoch: Instant) -> Self {
        LocalHold { epoch, last: AtomicU64::new(0) }
    }

    fn ns(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.epoch).as_nanos() as u64 + 1
    }

    /// A local event happened at `at`.
    pub fn note_local_at(&self, at: Instant) {
        self.last.fetch_max(self.ns(at), Ordering::SeqCst);
    }

    pub fn note_local(&self) {
        self.note_local_at(Instant::now());
    }

    /// How long since the last local event (`None`: none seen yet).
    pub fn local_idle_at(&self, now: Instant) -> Option<Duration> {
        match self.last.load(Ordering::SeqCst) {
            0 => None,
            t => Some(Duration::from_nanos(self.ns(now).saturating_sub(t))),
        }
    }

    pub fn held_at(&self, now: Instant) -> bool {
        self.local_idle_at(now).is_some_and(|d| d < LOCAL_HOLD)
    }

    pub fn held(&self) -> bool {
        self.held_at(Instant::now())
    }
}

/// Edge detector for the hold, polled by deskd's watcher thread: says when
/// a hold starts (release held keys, tell the daemon) and when it ends.
#[derive(Debug, Default)]
pub struct HoldEdges {
    was: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Started,
    Ended,
}

impl HoldEdges {
    pub fn poll(&mut self, held: bool) -> Option<Edge> {
        let edge = match (self.was, held) {
            (false, true) => Some(Edge::Started),
            (true, false) => Some(Edge::Ended),
            _ => None,
        };
        self.was = held;
        edge
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_local_input_means_no_hold() {
        let t0 = Instant::now();
        let h = LocalHold::new(t0);
        assert!(!h.held_at(t0));
        assert_eq!(h.local_idle_at(t0 + Duration::from_secs(9)), None);
    }

    #[test]
    fn a_local_event_holds_for_three_seconds_rolling() {
        let t0 = Instant::now();
        let h = LocalHold::new(t0);
        h.note_local_at(t0 + Duration::from_secs(1));
        assert!(h.held_at(t0 + Duration::from_secs(1)));
        assert!(h.held_at(t0 + Duration::from_millis(3_999)));
        assert!(!h.held_at(t0 + Duration::from_millis(4_000)));
        // Rolling: another event pushes it out.
        h.note_local_at(t0 + Duration::from_millis(3_500));
        assert!(h.held_at(t0 + Duration::from_millis(6_000)));
        assert!(!h.held_at(t0 + Duration::from_millis(6_500)));
        assert_eq!(h.local_idle_at(t0 + Duration::from_millis(4_500)), Some(Duration::from_secs(1)));
    }

    #[test]
    fn an_older_event_never_moves_the_hold_back() {
        let t0 = Instant::now();
        let h = LocalHold::new(t0);
        h.note_local_at(t0 + Duration::from_secs(5));
        h.note_local_at(t0 + Duration::from_secs(1));
        assert!(h.held_at(t0 + Duration::from_secs(7)));
    }

    #[test]
    fn edges_fire_once_each_way() {
        let mut e = HoldEdges::default();
        assert_eq!(e.poll(false), None);
        assert_eq!(e.poll(true), Some(Edge::Started));
        assert_eq!(e.poll(true), None);
        assert_eq!(e.poll(false), Some(Edge::Ended));
        assert_eq!(e.poll(false), None);
    }
}
