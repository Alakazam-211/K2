//! The per-App coalescing of App state events: the bound on a manual
//! clock, then once through the real bus.

use std::time::{Duration, Instant};

use super::state_events::{self, drain_refreshes, wait_told, Gate, SETTLE, WINDOW};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
fn one_change_is_told_once_after_the_settle() {
    let t0 = Instant::now();
    let mut g = Gate::default();
    assert_eq!(g.note("ws-a", t0), Some(t0 + SETTLE));
    assert_eq!(g.note("ws-a", t0 + ms(10)), None, "the armed flush carries it");
    assert_eq!(g.flush(t0 + SETTLE), vec!["ws-a".to_string()]);
    assert!(g.flush(t0 + SETTLE + ms(1)).is_empty(), "told once");
}

/// A park: the stop, then the row moving to no one (`''`). The workspace
/// it left hears once; the empty owner never does.
#[test]
fn a_park_tells_the_workspace_it_left_and_never_the_empty_owner() {
    let t0 = Instant::now();
    let mut g = Gate::default();
    assert!(g.note("ws-a", t0).is_some());
    assert_eq!(g.note("", t0 + ms(5)), None);
    assert_eq!(g.flush(t0 + SETTLE), vec!["ws-a".to_string()]);
    let mut parked = Gate::default();
    assert_eq!(parked.note("", t0), None, "nothing to tell: nothing armed");
    assert!(parked.flush(t0).is_empty());
}

/// A park holds the gate across its steps: the stop and the move to no
/// one fold into one flush however far apart they land (a slow run used
/// to let the stop's flush go first, telling the workspace twice).
#[test]
fn a_hold_folds_changes_a_settle_apart_into_one_flush() {
    let t0 = Instant::now();
    let mut g = Gate::default();
    g.hold();
    assert_eq!(g.note("ws-a", t0), None, "held: nothing arms");
    let late = t0 + SETTLE + WINDOW;
    assert_eq!(g.note("", late), None, "still held");
    assert_eq!(g.release(late), Some(late + SETTLE), "the release arms the one flush");
    assert_eq!(g.flush(late + SETTLE), vec!["ws-a".to_string()]);
    assert!(g.flush(late + SETTLE + ms(1)).is_empty(), "told once");
}

/// A flush armed before the hold that comes due inside it tells nothing;
/// the release arms it again with everything the hold folded in.
#[test]
fn a_flush_due_inside_a_hold_waits_for_the_release() {
    let t0 = Instant::now();
    let mut g = Gate::default();
    let at = g.note("ws-a", t0).expect("armed before the hold");
    g.hold();
    assert!(g.flush(at).is_empty(), "due inside the hold: nothing told yet");
    assert_eq!(g.note("ws-b", at + ms(10)), None);
    let end = at + ms(500);
    assert_eq!(g.release(end), Some(end + SETTLE));
    assert_eq!(g.flush(end + SETTLE), vec!["ws-a".to_string(), "ws-b".to_string()]);
    // Nothing pending: a release arms nothing.
    g.hold();
    assert_eq!(g.release(end + SETTLE + ms(1)), None);
}

#[test]
fn one_flush_tells_each_workspace_it_touched_once() {
    let t0 = Instant::now();
    let mut g = Gate::default();
    g.note("ws-b", t0);
    g.note("ws-a", t0 + ms(1));
    g.note("ws-b", t0 + ms(2));
    assert_eq!(g.flush(t0 + SETTLE), vec!["ws-a".to_string(), "ws-b".to_string()]);
}

/// A crash loop changing state every 20 ms for 10 s: about one flush per
/// second, never two within a window, and the last change is told.
#[test]
fn a_fast_crash_loop_is_told_at_most_once_a_window() {
    let t0 = Instant::now();
    let mut g = Gate::default();
    let mut armed: Option<Instant> = None;
    let mut flushes: Vec<Instant> = Vec::new();
    let mut last_change = t0;
    let mut t = t0;
    while t < t0 + Duration::from_secs(10) {
        if let Some(at) = armed.filter(|at| *at <= t) {
            assert_eq!(g.flush(at).len(), 1);
            flushes.push(at);
            armed = None;
        }
        if let Some(at) = g.note("ws-a", t) {
            assert!(armed.is_none(), "one flush armed at a time");
            armed = Some(at);
        }
        last_change = t;
        t += ms(20);
    }
    let at = armed.expect("the last change arms a flush");
    assert_eq!(g.flush(at).len(), 1);
    flushes.push(at);
    assert!(at >= last_change, "the last change is told");
    assert!((10..=12).contains(&flushes.len()), "~1 a second over 10 s: {}", flushes.len());
    for w in flushes.windows(2) {
        assert!(w[1] - w[0] >= WINDOW, "two flushes {:?} apart", w[1] - w[0]);
    }
}

/// Changes a window apart are each told, without waiting a whole window.
#[test]
fn changes_a_window_apart_are_each_told_after_the_settle() {
    let t0 = Instant::now();
    let mut g = Gate::default();
    assert_eq!(g.note("ws-a", t0), Some(t0 + SETTLE));
    g.flush(t0 + SETTLE);
    let t1 = t0 + Duration::from_secs(3);
    assert_eq!(g.note("ws-a", t1), Some(t1 + SETTLE));
    // A change right after a flush waits out the window.
    g.flush(t1 + SETTLE);
    let t2 = t1 + SETTLE + ms(10);
    assert_eq!(g.note("ws-a", t2), Some(t1 + SETTLE + WINDOW));
}

/// Through the real bus and threads: 200 changes inside the settle reach
/// the workspace's clients as one `publish_services_changed`, and a later
/// change as one more.
#[test]
fn a_burst_on_the_bus_is_one_event_and_a_later_change_another() {
    let ws = "ws-state-events-burst";
    let app = "state-events-burst";
    let mut rx = crate::session_events::subscribe();
    let start = Instant::now();
    for _ in 0..200 {
        state_events::app_changed_in(app, ws);
    }
    assert!(start.elapsed() < SETTLE, "the burst fits the settle: {:?}", start.elapsed());
    wait_told();
    assert_eq!(drain_refreshes(&mut rx, ws), (1, 0));
    state_events::app_changed_in(app, ws);
    wait_told();
    assert_eq!(drain_refreshes(&mut rx, ws), (1, 0));
}
