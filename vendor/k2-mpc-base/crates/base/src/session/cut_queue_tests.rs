//! The host-wide cut queue's rows (`cut_queue.rs`), on a fake clock (plain milliseconds) with
//! test keys: one job at a time, idle, forced, the duty gap, order, a refusal, slow reports,
//! one entry per doc, and the settings' checks. Each row that judges an event first asserts the
//! event happened.
//!
//! Plants (`MPC_BETA_PLANT`, `crate::beta_plant`), one per run, each caught at its row's assert:
//! `queue-runs-two`, `idle-ignored`, `duty-ignored`, `forced-waits-for-idle`, `refused-dropped`.

use super::config::{Config, CutDue, CutQueueError, CutQueueSettings};
use super::cut_queue::{CutOutcome, CutQueue, CutWhy};

/// A queue on the Beta's settings: idle 250 ms, duty 10%, slow 250 ms.
fn queue() -> CutQueue<&'static str> {
    CutQueue::new(CutQueueSettings::BETA)
}

/// One job at a time: while the job handed out is unreported, no second one is, even with
/// another doc due and the host idle; once it is reported the next is handed out.
/// `MPC_BETA_PLANT=queue-runs-two` fails at the second call's assert.
#[test]
fn the_queue_hands_out_one_cut_at_a_time() {
    let mut q = queue();
    q.enqueue("a", CutWhy::Cadence, 300, 0);
    q.enqueue("b", CutWhy::Cadence, 200, 0);
    assert_eq!(q.queued(), 2, "event: two docs queued");
    assert_eq!(
        q.next(1_000, 0),
        Some("a"),
        "event: the first cut handed out"
    );
    assert_eq!(
        q.next(1_000, 0),
        None,
        "row: no second cut while the first runs"
    );
    let r = q.ran("a", 0, CutOutcome::Done);
    assert!(r.is_some(), "event: the first cut reported");
    assert_eq!(
        q.next(1_000, 0),
        Some("b"),
        "row: the next cut once the first is reported"
    );
    println!("QUEUE one-at-a-time rows=2");
}

/// The cadence says why, the queue when: a doc the cadence says is forced is queued forced and
/// cut while editing goes on; one due at its cadence, and one past the large-uncut bound, are
/// queued at the cadence and wait for idle.
#[test]
fn a_due_doc_is_queued_with_the_cadences_reason() {
    assert_eq!(CutWhy::of(CutDue::Forced), CutWhy::Forced, "row: forced");
    assert_eq!(
        CutWhy::of(CutDue::Cadence),
        CutWhy::Cadence,
        "row: at the cadence"
    );
    assert_eq!(
        CutWhy::of(CutDue::LargeUncut),
        CutWhy::Cadence,
        "row: a large uncut doc waits for idle"
    );
    let mut q = queue();
    q.enqueue("big", CutWhy::of(CutDue::LargeUncut), 2_100, 0);
    q.enqueue("due", CutWhy::of(CutDue::Cadence), 200, 0);
    q.enqueue("forced", CutWhy::of(CutDue::Forced), 400, 0);
    assert_eq!(q.queued(), 3, "event: three docs queued");
    // a native commit 1 ms ago: not idle
    assert_eq!(
        q.next(1_000, 999),
        Some("forced"),
        "row: the forced doc is cut while not idle"
    );
    assert!(
        q.ran("forced", 0, CutOutcome::Done).is_some(),
        "event: reported"
    );
    assert_eq!(q.next(1_000, 999), None, "row: the others wait for idle");
    assert_eq!(
        q.next(1_249, 999),
        Some("big"),
        "row: once idle, the largest tail first"
    );
    println!("QUEUE cadence-reason rows=6");
}

/// Idle: a doc due at its cadence waits until no native commit for `native_cut_idle_ms` (250).
/// `MPC_BETA_PLANT=idle-ignored` fails at the not-idle assert.
#[test]
fn the_queue_waits_for_idle() {
    let mut q = queue();
    q.enqueue("a", CutWhy::Cadence, 150, 1_000);
    assert_eq!(
        q.entry(&"a"),
        Some((CutWhy::Cadence, 150, 1_000)),
        "event: the doc queued"
    );
    assert_eq!(
        q.next(1_249, 1_000),
        None,
        "row: no cut 249 ms after a native commit"
    );
    assert_eq!(
        q.next(1_250, 1_000),
        Some("a"),
        "row: the cut once idle for 250 ms"
    );
    println!("QUEUE idle rows=2");
}

/// Forced: a doc at twice its cadence (or past its hard trigger) is cut while editing goes on,
/// ahead of a doc due at its cadence.
/// `MPC_BETA_PLANT=forced-waits-for-idle` fails at the forced doc's assert.
#[test]
fn a_forced_cut_runs_while_editing_goes_on() {
    let mut q = queue();
    q.enqueue("cadence", CutWhy::Cadence, 390, 0);
    q.enqueue("forced", CutWhy::Forced, 400, 10);
    assert_eq!(
        q.entry(&"forced"),
        Some((CutWhy::Forced, 400, 10)),
        "event: the forced doc queued"
    );
    // a native commit 1 ms ago: not idle
    assert_eq!(
        q.next(1_000, 999),
        Some("forced"),
        "row: the forced doc is cut while not idle"
    );
    let r = q
        .ran("forced", 40, CutOutcome::Done)
        .expect("event: reported");
    assert_eq!(r.why, CutWhy::Forced, "row: reported as forced");
    println!("QUEUE forced rows=2");
}

/// The duty: after a cut that took 200 ms, at 10% no cut starts for 1,800 ms after its end, a
/// forced doc's included (here while not idle); after the gap the forced doc is first.
/// `MPC_BETA_PLANT=duty-ignored` fails at the in-gap assert.
#[test]
fn the_duty_gap_follows_a_measured_cut() {
    let mut q = queue();
    q.enqueue("a", CutWhy::Cadence, 200, 0);
    assert_eq!(q.next(1_000, 0), Some("a"), "event: a cut handed out");
    let r = q.ran("a", 200, CutOutcome::Done).expect("event: reported");
    assert_eq!(r.ms, 200, "event: its measured 200 ms");
    q.enqueue("b", CutWhy::Cadence, 300, 1_100);
    q.enqueue("f", CutWhy::Forced, 100, 1_150);
    assert_eq!(q.queued(), 2, "event: two more docs queued");
    // the cut ended at 1_200: the gap runs to 3_000
    assert_eq!(
        q.next(2_999, 2_999),
        None,
        "row: no cut inside the gap, not even a forced one"
    );
    assert_eq!(
        q.next(3_000, 0),
        Some("f"),
        "row: after the gap, the forced doc first"
    );
    println!("QUEUE duty rows=2");
}

/// Order: forced first, then the largest tail, then the oldest.
#[test]
fn the_queue_orders_forced_then_largest_tail_then_oldest() {
    let mut q = queue();
    q.enqueue("a", CutWhy::Cadence, 150, 0);
    q.enqueue("b", CutWhy::Cadence, 300, 10);
    q.enqueue("c", CutWhy::Forced, 50, 20);
    q.enqueue("d", CutWhy::Cadence, 300, 5);
    assert_eq!(q.queued(), 4, "event: four docs queued");
    let mut got = Vec::new();
    for _ in 0..4 {
        let k = q.next(1_000, 0).expect("event: a cut handed out");
        got.push(k);
        assert!(
            q.ran(k, 0, CutOutcome::Done).is_some(),
            "event: {k} reported"
        );
    }
    assert_eq!(
        got,
        ["c", "d", "b", "a"],
        "row: forced, then the largest tail, then the oldest"
    );
    assert_eq!(q.queued(), 0, "row: every doc cut once");
    println!("QUEUE order rows=2");
}

/// A refused cut (pending records, another live instance) stays queued and is retried after
/// the duty gap.
/// `MPC_BETA_PLANT=refused-dropped` fails at the still-queued assert.
#[test]
fn a_refused_cut_stays_queued_and_is_retried() {
    let mut q = queue();
    q.enqueue("a", CutWhy::Cadence, 120, 0);
    assert_eq!(q.next(1_000, 0), Some("a"), "event: a cut handed out");
    let r = q
        .ran("a", 100, CutOutcome::Refused)
        .expect("event: reported");
    assert_eq!(r.outcome, CutOutcome::Refused, "event: the cut was refused");
    assert_eq!(
        q.entry(&"a"),
        Some((CutWhy::Cadence, 120, 0)),
        "row: the refused doc stays queued"
    );
    // ended at 1_100, a 900 ms gap
    assert_eq!(q.next(2_000, 0), Some("a"), "row: retried after the gap");
    println!("QUEUE refused rows=2");
}

/// A cut over `native_cut_slow_ms` (250) is reported slow; one at it is not.
#[test]
fn a_slow_cut_is_reported() {
    let mut q = queue();
    q.enqueue("a", CutWhy::Cadence, 200, 0);
    q.enqueue("b", CutWhy::Cadence, 100, 0);
    assert_eq!(q.next(1_000, 0), Some("a"), "event: a cut handed out");
    let r = q.ran("a", 251, CutOutcome::Done).expect("event: reported");
    assert_eq!((r.key, r.ms, r.tail), ("a", 251, 200), "event: its report");
    assert!(r.slow, "row: 251 ms is slow");
    // ended at 1_251; 251 * 9 = 2_259 ms of gap
    assert_eq!(
        q.next(3_510, 0),
        Some("b"),
        "event: the next cut handed out"
    );
    let r = q.ran("b", 250, CutOutcome::Done).expect("event: reported");
    assert_eq!(r.ms, 250, "event: its report");
    assert!(!r.slow, "row: 250 ms is not slow");
    println!("QUEUE slow rows=2");
}

/// One entry per doc: a second enqueue updates the first (the newer tail, the stronger reason,
/// the first queued time kept), never adds a second.
#[test]
fn a_second_enqueue_updates_the_docs_entry() {
    let mut q = queue();
    q.enqueue("a", CutWhy::Cadence, 100, 0);
    assert_eq!(q.queued(), 1, "event: the doc queued");
    q.enqueue("a", CutWhy::Forced, 210, 50);
    assert_eq!(q.queued(), 1, "row: one entry per doc");
    assert_eq!(
        q.entry(&"a"),
        Some((CutWhy::Forced, 210, 0)),
        "row: the newer tail, the stronger reason, the first queued time"
    );
    q.enqueue("a", CutWhy::Unload, 220, 60);
    assert_eq!(
        q.entry(&"a"),
        Some((CutWhy::Forced, 220, 0)),
        "row: a weaker reason never replaces forced"
    );
    q.enqueue("u", CutWhy::Cadence, 10, 70);
    q.enqueue("u", CutWhy::Unload, 10, 80);
    assert_eq!(q.queued(), 2, "row: one entry per doc");
    assert_eq!(
        q.entry(&"u"),
        Some((CutWhy::Unload, 10, 70)),
        "row: unload over cadence"
    );
    println!("QUEUE dedup rows=5");
}

/// The queue's settings: by default idle 250 ms, duty 10%, slow 250 ms, listed among the
/// settings in effect; a host's own are checked by `CutQueueSettings::new`, which refuses an
/// idle window of 0 or past a minute, a duty outside 1..=50% and a slow threshold of 0, and takes
/// the bounds themselves.
#[test]
fn the_cut_queue_settings_are_checked_and_default_to_the_betas() {
    let d = Config::default().native_cut_queue;
    assert_eq!(
        (d.idle_ms(), d.duty_pct(), d.slow_ms()),
        (250, 10, 250),
        "row: the defaults"
    );
    assert_eq!(d, CutQueueSettings::BETA);
    assert_eq!(
        d.duty_gap_ms(200),
        1_800,
        "row: a 200 ms cut, 1,800 ms of gap"
    );
    let listed = Config::default().limits();
    for (name, want) in [
        ("native_cut_idle_ms", 250),
        ("native_cut_duty_pct", 10),
        ("native_cut_slow_ms", 250),
    ] {
        assert!(
            listed.contains(&(name, want)),
            "row: {name} listed among the settings"
        );
    }
    assert_eq!(
        CutQueueSettings::new(250, 10, 250),
        Ok(CutQueueSettings::BETA)
    );
    let most = CutQueueSettings::IDLE_MOST_MS;
    let mut rows = 0;
    for (i, p, s, want) in [
        (
            0,
            10,
            250,
            CutQueueError::IdleOutOfRange { idle_ms: 0, most },
        ),
        (
            most + 1,
            10,
            250,
            CutQueueError::IdleOutOfRange {
                idle_ms: most + 1,
                most,
            },
        ),
        (
            250,
            0,
            250,
            CutQueueError::DutyOutOfRange {
                duty_pct: 0,
                least: 1,
                most: 50,
            },
        ),
        (
            250,
            51,
            250,
            CutQueueError::DutyOutOfRange {
                duty_pct: 51,
                least: 1,
                most: 50,
            },
        ),
        (250, 10, 0, CutQueueError::SlowZero),
    ] {
        assert_eq!(
            CutQueueSettings::new(i, p, s),
            Err(want),
            "row: ({i}, {p}, {s}) is refused"
        );
        rows += 1;
    }
    for (i, p, s) in [(1, 1, 1), (most, 50, u64::MAX)] {
        let edge = CutQueueSettings::new(i, p, s);
        assert!(edge.is_ok(), "row: the bounds themselves: {edge:?}");
        rows += 1;
    }
    println!("QUEUE settings rows={rows}");
}
