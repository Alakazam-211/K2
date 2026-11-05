//! Did the agent take a Thread line? (0.46.9)
//!
//! A Thread delivery that reached a live session (the wake + inject
//! returned `success`) can still be lost inside the agent's TUI: Claude
//! Code swallowed typed input while its background-agents view was up,
//! so the line never became a prompt or a queued command. This module
//! watches each delivered Thread line until the agent shows it took it.
//!
//! - **Taken** = a `[thread:<addr>]` stamp seen for the target session
//!   after the delivery began: an owner `UserPromptSubmit` hook that
//!   carries it, or a transcript user turn / `queued_command` that does
//!   (fed from `thread_activity`, the same evidence the working strip
//!   binds on).
//! - **The clock runs only while the agent is idle.** While its lead is
//!   working (or waiting on a person), a line sits in Claude's queue and is
//!   read in at the next step or turn end; that is slow, not lost. The
//!   window ([`take_window`], 20 s by default, `K2_THREAD_TAKE_CHECK_SECS`,
//!   0 = off) counts from the later of the delivery and the last time the
//!   lead was seen busy. A lead busy for longer than [`BUSY_CAP_MS`] stops
//!   the watch: K2 can't tell, so it does nothing.
//! - **Never a double delivery.** Before calling a line not taken, K2
//!   looks for its text on the agent's screen (an input box, a queued
//!   message list). Text on screen = not retried. Only an idle agent with
//!   no stamp and no text on screen is not taken.
//! - **Not taken:** the caller's `on_not_taken` runs once
//!   (`overlay_routes`: type it in once more, then, if the retry is not
//!   taken either, a notice in the Thread for the sender).
//! - A session whose transcript K2 isn't reading yet ([`RowView::can_tell`]
//!   false: a missing stamp would mean nothing) or that is gone is not
//!   watched further. No retry, no notice.
//!
//! One daemon thread runs the watches, only while there are some.

use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

/// The env var that sets the take window in seconds (0 = off).
pub(crate) const ENV_TAKE_SECS: &str = "K2_THREAD_TAKE_CHECK_SECS";

/// The default take window.
pub(crate) const DEFAULT_TAKE_SECS: u64 = 20;

/// A lead busy this long after the delivery: stop watching (can't tell).
pub(crate) const BUSY_CAP_MS: i64 = 10 * 60_000;

/// A stamp this much older than the delivery's start still counts (clock
/// skew between a hook's receipt time and ours).
const STAMP_SLACK_MS: i64 = 1_000;

/// Stamps remembered (all sessions). Old ones fall off the front.
const SEEN_CAP: usize = 512;

/// How often the watcher looks.
const TICK: Duration = Duration::from_millis(500);

/// The take window, or `None` when the check is off.
pub(crate) fn take_window() -> Option<Duration> {
    parse_window(std::env::var(ENV_TAKE_SECS).ok().as_deref())
}

/// [`take_window`] from the env var's value.
fn parse_window(raw: Option<&str>) -> Option<Duration> {
    let secs = match raw {
        Some(v) => match v.trim().parse::<u64>() {
            Ok(n) => n,
            Err(_) => {
                k2_core::log_debug!("[thread-delivery] {ENV_TAKE_SECS}={v:?} is not a number of seconds; using {DEFAULT_TAKE_SECS}");
                DEFAULT_TAKE_SECS
            }
        },
        None => DEFAULT_TAKE_SECS,
    };
    (secs > 0).then(|| Duration::from_secs(secs))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ── Stamps seen ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Stamp {
    session: String,
    addr: String,
    at: i64,
}

static SEEN: LazyLock<Mutex<VecDeque<Stamp>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));

fn seen() -> std::sync::MutexGuard<'static, VecDeque<Stamp>> {
    SEEN.lock().unwrap_or_else(|e| e.into_inner())
}

/// The agent in `session` read in a line stamped `[thread:<addr>]` at
/// `at` (ms). Called by `thread_activity` for hooks and transcripts.
pub(crate) fn note_stamp(session: &str, addr: &str, at: i64) {
    let mut s = seen();
    s.push_back(Stamp { session: session.to_string(), addr: addr.to_string(), at });
    while s.len() > SEEN_CAP {
        s.pop_front();
    }
}

/// A stamp for `addr` (or another address of the same Thread, by
/// `same_thread`) in `session` at or after `since`.
fn stamp_seen(session: &str, addr: &str, since: i64, same_thread: &dyn Fn(&str) -> bool) -> bool {
    let s = seen();
    s.iter()
        .filter(|st| st.session == session && st.at >= since - STAMP_SLACK_MS)
        .any(|st| st.addr == addr || same_thread(&st.addr))
}

// ── The decision ──────────────────────────────────────────────────────

/// The agent's row, as the check reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RowView {
    /// Its lead is working, or waiting on a person.
    pub busy: bool,
    /// K2 reads its turns (hooks or a transcript). Without that a missing
    /// stamp means nothing.
    pub can_tell: bool,
}

/// What a look at one watch decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// The agent read it in.
    Taken,
    /// Not yet: the agent is busy, or the window isn't over.
    Wait,
    /// The window passed with the agent idle, and the text is on its
    /// screen: it may be sitting in the input. Not retried.
    OnScreen,
    /// The window passed with the agent idle, no stamp, nothing on screen.
    NotTaken,
    /// K2 can't tell; stop watching and do nothing.
    GiveUp(&'static str),
}

/// One look. `waited_ms` = since the delivery; `quiet_ms` = since the
/// later of the delivery and the lead last seen busy. `on_screen` is
/// asked only when everything else says not taken.
pub(crate) fn decide(
    stamp: bool,
    row: Option<RowView>,
    waited_ms: i64,
    quiet_ms: i64,
    window_ms: i64,
    on_screen: impl FnOnce() -> bool,
) -> Step {
    if stamp {
        return Step::Taken;
    }
    let Some(row) = row else {
        return Step::GiveUp("session_gone");
    };
    if !row.can_tell {
        return Step::GiveUp("no_turn_evidence");
    }
    if row.busy {
        return if waited_ms >= BUSY_CAP_MS { Step::GiveUp("busy_too_long") } else { Step::Wait };
    }
    if quiet_ms < window_ms {
        return Step::Wait;
    }
    if on_screen() {
        return Step::OnScreen;
    }
    Step::NotTaken
}

/// The text to look for on screen: the first line after the
/// `[thread:<addr>]` stamp, its first 32 characters, whitespace dropped
/// (a TUI wraps and pads). Empty when there's nothing to look for.
pub(crate) fn screen_snippet(payload: &str) -> String {
    let body = match payload.find("[thread:") {
        Some(i) => match payload[i..].find(']') {
            Some(j) => &payload[i + j + 1..],
            None => payload,
        },
        None => payload,
    };
    let line = body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    // A guest's quoted lines start with `> `.
    let line = line.trim_start_matches('>').trim();
    squash(&line.chars().take(32).collect::<String>())
}

fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// `snippet` is in the screen rows (joined, whitespace dropped).
pub(crate) fn rows_show(rows: &[String], snippet: &str) -> bool {
    if snippet.is_empty() {
        return false;
    }
    let joined: String = rows.iter().map(|r| squash(r)).collect();
    joined.contains(snippet)
}

// ── Watches ───────────────────────────────────────────────────────────

/// What the watcher reads about a session (a seam for the tests).
pub(crate) trait Probe {
    fn row(&self, session: &str) -> Option<RowView>;
    fn screen_shows(&self, session: &str, snippet: &str) -> bool;
    /// `other` is another address of `addr`'s Thread.
    fn same_thread(&self, addr: &str, other: &str) -> bool;
}

/// The live probe: the activity store's row, the session's grid.
struct LiveProbe;

impl Probe for LiveProbe {
    fn row(&self, session: &str) -> Option<RowView> {
        crate::activity_store::with_row(session, |row| RowView {
            busy: row.lead.state != k2_core::activity::LeadState::Idle,
            // Only a session whose transcript K2 is reading: that is where
            // both a new prompt and a mid-turn queued command show up. A
            // session with hooks alone would miss a queued command, and
            // one with neither shows nothing at all.
            can_tell: row.had_transcript(),
        })
    }

    fn screen_shows(&self, session: &str, snippet: &str) -> bool {
        crate::activity_transcript::grid_rows(session).is_some_and(|rows| rows_show(&rows, snippet))
    }

    fn same_thread(&self, addr: &str, other: &str) -> bool {
        match (
            crate::overlay_routes::conversation_for_addr(addr),
            crate::overlay_routes::conversation_for_addr(other),
        ) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }
}

/// One delivered line being watched.
pub(crate) struct Watch {
    pub session: String,
    pub addr: String,
    /// When the delivery began (ms).
    pub since: i64,
    pub snippet: String,
    /// Whether this was already the retry (for the log).
    pub retry: bool,
    /// The later of `since` and the lead last seen busy.
    quiet_since: i64,
    on_not_taken: Option<Box<dyn FnOnce() + Send + 'static>>,
}

impl Watch {
    pub(crate) fn new(
        session: &str,
        addr: &str,
        since: i64,
        payload: &str,
        retry: bool,
        on_not_taken: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self {
            session: session.to_string(),
            addr: addr.to_string(),
            since,
            snippet: screen_snippet(payload),
            retry,
            quiet_since: since,
            on_not_taken: Some(Box::new(on_not_taken)),
        }
    }
}

static WATCHES: LazyLock<Mutex<Vec<Watch>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn watches() -> std::sync::MutexGuard<'static, Vec<Watch>> {
    WATCHES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Watch a delivered line. Starts the watcher thread if none runs.
pub(crate) fn watch(w: Watch) {
    let mut ws = watches();
    let start = ws.is_empty();
    ws.push(w);
    drop(ws);
    if start {
        let spawned = std::thread::Builder::new().name("k2-delivery-take".into()).spawn(run_watcher);
        if let Err(e) = spawned {
            k2_core::log_debug!("[thread-delivery] could not start the take check ({e}); not checking");
            watches().clear();
        }
    }
}

fn run_watcher() {
    loop {
        std::thread::sleep(TICK);
        let window = take_window().unwrap_or(Duration::from_secs(DEFAULT_TAKE_SECS));
        let mut ws = watches();
        let fired = tick(&mut ws, now_ms(), window.as_millis() as i64, &LiveProbe);
        let empty = ws.is_empty();
        drop(ws);
        for f in fired {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
                k2_core::log_debug!("[thread-delivery] a not-taken step panicked");
            }
        }
        if empty {
            // Exit only if no watch came in meanwhile. A watch that found
            // the list empty started its own thread; two threads for a
            // moment is harmless (each step fires once, under the lock).
            let ws = watches();
            if ws.is_empty() {
                return;
            }
        }
    }
}

/// One pass over `ws` at `now`: settle what is decided, keep the rest.
/// Returns the `on_not_taken` steps to run (outside the lock).
pub(crate) fn tick(ws: &mut Vec<Watch>, now: i64, window_ms: i64, probe: &dyn Probe) -> Vec<Box<dyn FnOnce() + Send + 'static>> {
    let mut fired = Vec::new();
    ws.retain_mut(|w| {
        let same = |other: &str| probe.same_thread(&w.addr, other);
        let stamp = stamp_seen(&w.session, &w.addr, w.since, &same);
        let row = probe.row(&w.session);
        if row.is_some_and(|r| r.busy) {
            w.quiet_since = now;
        }
        let step = decide(stamp, row, now - w.since, now - w.quiet_since, window_ms, || {
            probe.screen_shows(&w.session, &w.snippet)
        });
        let (addr, session, after) = (&w.addr, &w.session, now - w.since);
        match step {
            Step::Wait => return true,
            Step::Taken => {
                k2_core::log_debug!("[thread-delivery] taken addr={addr} session={session} after_ms={after} retry={}", w.retry);
            }
            Step::OnScreen => {
                k2_core::log_debug!(
                    "[thread-delivery] not read in, but its text is on the agent's screen; not typed again addr={addr} session={session} after_ms={after}"
                );
            }
            Step::GiveUp(why) => {
                k2_core::log_debug!("[thread-delivery] take check stopped ({why}) addr={addr} session={session} after_ms={after}");
            }
            Step::NotTaken => {
                k2_core::log_debug!(
                    "[thread-delivery] not taken addr={addr} session={session} after_ms={after} retry={}",
                    w.retry
                );
                if let Some(f) = w.on_not_taken.take() {
                    fired.push(f);
                }
            }
        }
        false
    });
    fired
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const W: i64 = 20_000;

    #[derive(Default)]
    struct Fake {
        rows: HashMap<String, RowView>,
        screen: HashMap<String, Vec<String>>,
    }

    impl Probe for Fake {
        fn row(&self, session: &str) -> Option<RowView> {
            self.rows.get(session).copied()
        }
        fn screen_shows(&self, session: &str, snippet: &str) -> bool {
            self.screen.get(session).is_some_and(|r| rows_show(r, snippet))
        }
        fn same_thread(&self, _addr: &str, _other: &str) -> bool {
            false
        }
    }

    fn sid(label: &str) -> String {
        format!("{label}-{}", uuid::Uuid::new_v4())
    }

    const IDLE: RowView = RowView { busy: false, can_tell: true };
    const BUSY: RowView = RowView { busy: true, can_tell: true };

    #[test]
    fn decide_reads_a_stamp_as_taken_first() {
        assert_eq!(decide(true, None, 0, 0, W, || panic!("no screen read")), Step::Taken);
        assert_eq!(decide(true, Some(BUSY), 0, 0, W, || panic!("no screen read")), Step::Taken);
    }

    #[test]
    fn decide_waits_while_the_agent_is_busy_then_gives_up() {
        assert_eq!(decide(false, Some(BUSY), 60_000, 0, W, || panic!("no screen read")), Step::Wait);
        assert_eq!(decide(false, Some(BUSY), BUSY_CAP_MS, 0, W, || panic!("no screen read")), Step::GiveUp("busy_too_long"));
    }

    #[test]
    fn decide_waits_out_the_window_while_idle() {
        assert_eq!(decide(false, Some(IDLE), 5_000, 5_000, W, || panic!("no screen read")), Step::Wait);
        assert_eq!(decide(false, Some(IDLE), W, W, W, || false), Step::NotTaken);
    }

    #[test]
    fn decide_never_calls_text_on_screen_not_taken() {
        assert_eq!(decide(false, Some(IDLE), W, W, W, || true), Step::OnScreen);
    }

    #[test]
    fn decide_gives_up_when_it_cannot_tell() {
        let blind = RowView { busy: false, can_tell: false };
        assert_eq!(decide(false, Some(blind), W, W, W, || false), Step::GiveUp("no_turn_evidence"));
        assert_eq!(decide(false, None, W, W, W, || false), Step::GiveUp("session_gone"));
    }

    #[test]
    fn the_snippet_is_the_text_after_the_stamp() {
        assert_eq!(screen_snippet("[thread:sales] run the tests please"), "runthetestsplease");
        assert_eq!(screen_snippet("[thread:sales] > a guest's\n> second"), "aguest's");
        let long = format!("[thread:x] {}", "a".repeat(80));
        assert_eq!(screen_snippet(&long).len(), 32);
        assert_eq!(screen_snippet("[thread:x]   "), "");
    }

    #[test]
    fn rows_show_finds_a_wrapped_snippet() {
        let rows = vec!["│ > run the te".to_string(), "sts please    │".to_string()];
        assert!(rows_show(&rows, &screen_snippet("[thread:s] run the tests please")));
        assert!(!rows_show(&rows, &screen_snippet("[thread:s] deploy it")));
        assert!(!rows_show(&rows, ""), "nothing to look for is never on screen");
    }

    fn counting(n: &Arc<AtomicUsize>) -> impl FnOnce() + Send + 'static {
        let n = n.clone();
        move || {
            n.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn run_fired(f: Vec<Box<dyn FnOnce() + Send + 'static>>) {
        for x in f {
            x();
        }
    }

    #[test]
    fn a_stamp_after_the_delivery_settles_the_watch() {
        let s = sid("taken");
        let mut probe = Fake::default();
        probe.rows.insert(s.clone(), IDLE);
        let n = Arc::new(AtomicUsize::new(0));
        let mut ws = vec![Watch::new(&s, "sales", 1_000, "[thread:sales] hi there", false, counting(&n))];
        note_stamp(&s, "sales", 1_500);
        run_fired(tick(&mut ws, 2_000, W, &probe));
        assert!(ws.is_empty(), "taken: the watch is done");
        assert_eq!(n.load(Ordering::SeqCst), 0, "no retry for a taken line");
    }

    #[test]
    fn an_older_stamp_does_not_count() {
        let s = sid("old");
        let mut probe = Fake::default();
        probe.rows.insert(s.clone(), IDLE);
        let n = Arc::new(AtomicUsize::new(0));
        note_stamp(&s, "sales", 10_000);
        let mut ws = vec![Watch::new(&s, "sales", 20_000, "[thread:sales] hi", false, counting(&n))];
        run_fired(tick(&mut ws, 20_000 + W, W, &probe));
        assert!(ws.is_empty());
        assert_eq!(n.load(Ordering::SeqCst), 1, "a stamp from before the delivery is not this line");
    }

    #[test]
    fn a_stamp_in_another_session_does_not_count() {
        let s = sid("mine");
        let mut probe = Fake::default();
        probe.rows.insert(s.clone(), IDLE);
        let n = Arc::new(AtomicUsize::new(0));
        note_stamp(&sid("other"), "sales", 5_000);
        let mut ws = vec![Watch::new(&s, "sales", 1_000, "[thread:sales] hi", false, counting(&n))];
        run_fired(tick(&mut ws, 1_000 + W, W, &probe));
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_slow_busy_agent_is_not_retried_until_it_has_been_idle_for_the_window() {
        let s = sid("slow");
        let mut probe = Fake::default();
        probe.rows.insert(s.clone(), BUSY);
        let n = Arc::new(AtomicUsize::new(0));
        let mut ws = vec![Watch::new(&s, "sales", 0, "[thread:sales] build it", false, counting(&n))];
        // Busy for a minute: never retried.
        for t in (0..=60_000).step_by(500) {
            run_fired(tick(&mut ws, t, W, &probe));
        }
        assert_eq!(ws.len(), 1, "still watching while the agent works");
        assert_eq!(n.load(Ordering::SeqCst), 0);
        // Idle now; the window runs from here.
        probe.rows.insert(s.clone(), IDLE);
        run_fired(tick(&mut ws, 60_000 + W - 1, W, &probe));
        assert_eq!(n.load(Ordering::SeqCst), 0, "the window counts from the last busy look");
        // It took the queued line at the turn end.
        note_stamp(&s, "sales", 60_000 + 3_000);
        run_fired(tick(&mut ws, 60_000 + W + 500, W, &probe));
        assert!(ws.is_empty());
        assert_eq!(n.load(Ordering::SeqCst), 0, "slow, not lost: no second delivery");
    }

    #[test]
    fn an_idle_agent_that_never_read_it_in_fires_once() {
        let s = sid("lost");
        let mut probe = Fake::default();
        probe.rows.insert(s.clone(), IDLE);
        probe.screen.insert(s.clone(), vec!["Background agents".to_string(), "  1 running".to_string()]);
        let n = Arc::new(AtomicUsize::new(0));
        let mut ws = vec![Watch::new(&s, "sales", 0, "[thread:sales] are you there", false, counting(&n))];
        run_fired(tick(&mut ws, W - 500, W, &probe));
        assert_eq!(n.load(Ordering::SeqCst), 0, "inside the window");
        run_fired(tick(&mut ws, W, W, &probe));
        run_fired(tick(&mut ws, W + 500, W, &probe));
        assert_eq!(n.load(Ordering::SeqCst), 1, "not taken: fired exactly once");
        assert!(ws.is_empty());
    }

    #[test]
    fn text_still_on_screen_is_not_typed_again() {
        let s = sid("onscreen");
        let mut probe = Fake::default();
        probe.rows.insert(s.clone(), IDLE);
        probe.screen.insert(s.clone(), vec!["> are you th".to_string(), "ere".to_string()]);
        let n = Arc::new(AtomicUsize::new(0));
        let mut ws = vec![Watch::new(&s, "sales", 0, "[thread:sales] are you there", false, counting(&n))];
        run_fired(tick(&mut ws, W, W, &probe));
        assert!(ws.is_empty());
        assert_eq!(n.load(Ordering::SeqCst), 0, "its text is on screen: never a second delivery");
    }

    #[test]
    fn the_window_is_20_s_unless_set_and_zero_turns_it_off() {
        assert_eq!(DEFAULT_TAKE_SECS, 20);
        assert_eq!(parse_window(None), Some(Duration::from_secs(DEFAULT_TAKE_SECS)));
        assert_eq!(parse_window(Some("7")), Some(Duration::from_secs(7)));
        assert_eq!(parse_window(Some(" 45 ")), Some(Duration::from_secs(45)));
        assert_eq!(parse_window(Some("0")), None, "0 turns the check off");
        assert_eq!(parse_window(Some("soon")), Some(Duration::from_secs(DEFAULT_TAKE_SECS)), "junk falls back to the default");
    }
}
