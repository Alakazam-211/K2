//! What the executor's accept loop does when the process or the machine is
//! out of descriptors or memory (EMFILE, ENFILE, ENOBUFS, ENOMEM).
//!
//! `accept()` fails at once, for as long as the shortage lasts, with the
//! connection still queued, so `poll` says ready again straight away and a
//! loop that logs and retries pegs a core and fills the node's disk just when
//! the machine is already in trouble. Pause and keep the listener; every other
//! error stays the caller's to handle.
//!
//! The same rule as the daemon's `k2_core::accept_backoff`: k2-node never
//! links k2-core (CN26), so it keeps its own small copy.

use std::io;
use std::time::{Duration, Instant};

const FIRST_PAUSE: Duration = Duration::from_millis(50);
const MAX_PAUSE: Duration = Duration::from_secs(1);
/// At most one "accept error" line, and one "accepting again" line, in this long.
const LOG_EVERY: Duration = Duration::from_secs(60);

/// Is `e` a shortage that will pass, rather than a fault in the listener?
pub fn is_resource_exhaustion(e: &io::Error) -> bool {
    e.raw_os_error().is_some_and(|c| [libc::EMFILE, libc::ENFILE, libc::ENOBUFS, libc::ENOMEM].contains(&c))
}

/// One accept loop's pause and log state.
#[derive(Debug)]
pub struct AcceptBackoff {
    label: &'static str,
    next_pause: Duration,
    /// Failed since the last accept that worked.
    failing: bool,
    /// Failed accepts the last "accepting again" line hasn't reported.
    failed: u64,
    failing_since: Option<Instant>,
    /// Failed accepts the last "accept error" line hasn't reported.
    unlogged: u64,
    last_error_line: Option<Instant>,
    last_recovered_line: Option<Instant>,
}

impl AcceptBackoff {
    /// `label` starts every line, as the loop's other log lines do: `admin:`.
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            next_pause: FIRST_PAUSE,
            failing: false,
            failed: 0,
            failing_since: None,
            unlogged: 0,
            last_error_line: None,
            last_recovered_line: None,
        }
    }

    /// A failed accept. `Some(pause)`: a shortage, so sleep that long and go
    /// round again, keeping the listener. `None`: not a shortage, and this
    /// logged nothing, so the loop's own handling stands.
    pub fn on_error(&mut self, e: &io::Error) -> Option<Duration> {
        let (pause, line) = self.note_error(e, Instant::now());
        if let Some(line) = line {
            crate::nlog!("{line}");
        }
        pause
    }

    /// An accept that worked. Costs one branch unless the last one failed.
    pub fn on_accept(&mut self) {
        if self.failing {
            if let Some(line) = self.note_accept(Instant::now()) {
                crate::nlog!("{line}");
            }
        }
    }

    fn note_error(&mut self, e: &io::Error, now: Instant) -> (Option<Duration>, Option<String>) {
        if !is_resource_exhaustion(e) {
            return (None, None);
        }
        let pause = self.next_pause;
        self.next_pause = (pause * 2).min(MAX_PAUSE);
        self.failing = true;
        self.failed += 1;
        self.failing_since.get_or_insert(now);
        if self.last_error_line.is_some_and(|t| now.saturating_duration_since(t) < LOG_EVERY) {
            self.unlogged += 1;
            return (Some(pause), None);
        }
        let line = if self.unlogged > 0 {
            format!("{} accept error: {e} (and {} more in the last {} s)", self.label, self.unlogged, LOG_EVERY.as_secs())
        } else {
            format!("{} accept error: {e} (pausing up to {} s between tries; the listener stays up)", self.label, MAX_PAUSE.as_secs())
        };
        self.unlogged = 0;
        self.last_error_line = Some(now);
        (Some(pause), Some(line))
    }

    fn note_accept(&mut self, now: Instant) -> Option<String> {
        self.next_pause = FIRST_PAUSE;
        self.failing = false;
        if self.last_recovered_line.is_some_and(|t| now.saturating_duration_since(t) < LOG_EVERY) {
            return None;
        }
        let since = self.failing_since.take().unwrap_or(now);
        let line = format!(
            "{} accepting again after {} failed accepts over {} ms",
            self.label,
            self.failed,
            now.saturating_duration_since(since).as_millis()
        );
        self.failed = 0;
        self.last_recovered_line = Some(now);
        Some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emfile() -> io::Error {
        io::Error::from_raw_os_error(libc::EMFILE)
    }

    #[test]
    fn only_a_shortage_counts_and_anything_else_is_left_to_the_loop() {
        let mut b = AcceptBackoff::new("t:");
        for code in [libc::EMFILE, libc::ENFILE, libc::ENOBUFS, libc::ENOMEM] {
            assert!(is_resource_exhaustion(&io::Error::from_raw_os_error(code)), "{code}");
        }
        for code in [libc::EBADF, libc::EINVAL, libc::ECONNABORTED, libc::ENOTSOCK] {
            let e = io::Error::from_raw_os_error(code);
            assert!(!is_resource_exhaustion(&e), "{code}");
            assert_eq!(b.note_error(&e, Instant::now()), (None, None), "{code}");
        }
        assert!(!is_resource_exhaustion(&io::Error::other("no OS code")));
        assert!(!b.failing, "a fault in the listener is no shortage");
    }

    #[test]
    fn the_pause_doubles_to_a_second_and_one_accept_resets_it() {
        let mut b = AcceptBackoff::new("t:");
        let now = Instant::now();
        let pauses: Vec<u128> = (0..8).map(|_| b.note_error(&emfile(), now).0.unwrap().as_millis()).collect();
        assert_eq!(pauses, [50, 100, 200, 400, 800, 1000, 1000, 1000]);
        b.note_accept(now);
        assert_eq!(b.note_error(&emfile(), now).0, Some(Duration::from_millis(50)));
    }

    #[test]
    fn a_long_shortage_logs_its_first_line_then_one_count_a_minute() {
        let mut b = AcceptBackoff::new("admin:");
        let t0 = Instant::now();
        let first = b.note_error(&emfile(), t0).1.expect("the first failure is logged");
        assert!(first.starts_with("admin: accept error: "), "{first}");
        for s in 1..60 {
            assert_eq!(b.note_error(&emfile(), t0 + Duration::from_secs(s)).1, None, "second {s}");
        }
        let count = b.note_error(&emfile(), t0 + LOG_EVERY).1.expect("the next minute's line");
        assert!(count.contains("and 59 more in the last 60 s"), "{count}");
    }

    #[test]
    fn recovery_is_one_line_with_the_count_and_flapping_stays_quiet() {
        let mut b = AcceptBackoff::new("t:");
        let t0 = Instant::now();
        b.note_error(&emfile(), t0);
        b.note_error(&emfile(), t0 + Duration::from_millis(50));
        let line = b.note_accept(t0 + Duration::from_millis(150)).expect("recovered");
        assert_eq!(line, "t: accepting again after 2 failed accepts over 150 ms");

        // Out again and back, over and over: nothing new until the minute is up.
        let t1 = t0 + Duration::from_secs(1);
        for i in 0..50 {
            let t = t1 + Duration::from_millis(i * 100);
            assert_eq!(b.note_error(&emfile(), t).1, None, "try {i}");
            assert_eq!(b.note_accept(t), None, "try {i}");
        }
        let t2 = t0 + LOG_EVERY + Duration::from_secs(1);
        assert!(b.note_error(&emfile(), t2).1.expect("a new minute").contains("and 51 more"));
        let later = b.note_accept(t2).expect("a new minute");
        assert!(later.contains("after 51 failed accepts"), "the quiet ones still count: {later}");
    }
}
