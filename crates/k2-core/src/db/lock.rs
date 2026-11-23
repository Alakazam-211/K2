//! The main database's lock, with a wait/hold log.
//!
//! [`DbLock`] is the one `ReentrantMutex<Connection>` every K2 database
//! caller takes (`db::shared().lock()`). It times the outermost hold on each
//! thread: a caller that waited [`WAIT_LOG_AT`] or more for the lock, or held
//! it [`HOLD_LOG_AT`] or more, logs one line naming its call site:
//!
//! ```text
//! [db] waited 340ms held 1210ms at crates/k2-core/src/projects_ops.rs:785
//! ```
//!
//! The site comes from `#[track_caller]`, so it is the line that called
//! `.lock()`. One line per site per [`SITE_QUIET_FOR`]; lines a site would
//! have logged in between are counted on its next line. `K2_DB_LOCK_LOG=0`
//! turns the log off (no timing at all then).
//!
//! The lock is reentrant: only the outermost hold on a thread is timed, so a
//! nested `.lock()` never logs the same hold twice.

use parking_lot::{Mutex, ReentrantMutex, ReentrantMutexGuard};
use rusqlite::Connection;
use std::collections::HashMap;
use std::ops::Deref;
use std::panic::Location;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// A wait for the lock this long or longer is logged.
pub const WAIT_LOG_AT: Duration = Duration::from_millis(100);
/// A hold of the lock this long or longer is logged.
pub const HOLD_LOG_AT: Duration = Duration::from_millis(250);
/// One line per call site in this window.
pub const SITE_QUIET_FOR: Duration = Duration::from_secs(10);

/// The main database's connection behind its reentrant lock.
pub struct DbLock {
    inner: ReentrantMutex<Connection>,
    /// Holds on the owning thread. Changed only by the thread that holds
    /// `inner`, so it is 0 whenever a thread takes the lock from free.
    depth: AtomicU32,
    /// The read-only connections beside this one ([`DbLock::read`]); `None`
    /// for a database that has none (in memory, not WAL, switched off).
    pub(super) readers: parking_lot::RwLock<Option<std::sync::Arc<super::read_pool::ReadPool>>>,
}

/// A held [`DbLock`]. Derefs to the connection.
pub struct DbGuard<'a> {
    guard: ReentrantMutexGuard<'a, Connection>,
    depth: &'a AtomicU32,
    outer: Option<OuterHold>,
}

struct OuterHold {
    site: &'static Location<'static>,
    waited: Duration,
    acquired: Instant,
}

impl DbLock {
    pub fn new(conn: Connection) -> Self {
        Self { inner: ReentrantMutex::new(conn), depth: AtomicU32::new(0), readers: parking_lot::RwLock::new(None) }
    }

    /// Take the lock, waiting as long as it takes.
    #[track_caller]
    pub fn lock(&self) -> DbGuard<'_> {
        let site = Location::caller();
        let t0 = lock_log_enabled().then(Instant::now);
        let guard = self.inner.lock();
        self.wrap(guard, site, t0)
    }

    /// Take the lock if it is free (or this thread holds it).
    #[track_caller]
    pub fn try_lock(&self) -> Option<DbGuard<'_>> {
        let site = Location::caller();
        let t0 = lock_log_enabled().then(Instant::now);
        let guard = self.inner.try_lock()?;
        Some(self.wrap(guard, site, t0))
    }

    /// Take the lock, waiting at most `timeout`.
    #[track_caller]
    pub fn try_lock_for(&self, timeout: Duration) -> Option<DbGuard<'_>> {
        let site = Location::caller();
        let t0 = lock_log_enabled().then(Instant::now);
        let guard = self.inner.try_lock_for(timeout)?;
        Some(self.wrap(guard, site, t0))
    }

    /// Some thread holds the lock.
    pub fn is_locked(&self) -> bool {
        self.inner.is_locked()
    }

    /// This thread holds the lock.
    pub fn is_owned_by_current_thread(&self) -> bool {
        self.inner.is_owned_by_current_thread()
    }

    fn wrap<'a>(
        &'a self,
        guard: ReentrantMutexGuard<'a, Connection>,
        site: &'static Location<'static>,
        t0: Option<Instant>,
    ) -> DbGuard<'a> {
        let prev = self.depth.fetch_add(1, Ordering::Relaxed);
        let outer = match t0 {
            Some(t0) if prev == 0 => {
                let acquired = Instant::now();
                let waited = acquired.saturating_duration_since(t0);
                note_wait(waited);
                Some(OuterHold { site, waited, acquired })
            }
            _ => None,
        };
        DbGuard { guard, depth: &self.depth, outer }
    }
}

impl Deref for DbGuard<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.guard
    }
}

impl Drop for DbGuard<'_> {
    fn drop(&mut self) {
        // Runs before `guard` is released, so the depth stays the owner's.
        self.depth.fetch_sub(1, Ordering::Relaxed);
        if let Some(o) = self.outer.take() {
            let held = o.acquired.elapsed();
            if o.waited >= WAIT_LOG_AT || held >= HOLD_LOG_AT {
                report(o.site, o.waited, held);
            }
        }
    }
}

/// Process-wide totals of the waits for the lock (outermost holds only;
/// none while `K2_DB_LOCK_LOG=0`): what the `[db] waited` lines sample.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WaitStats {
    /// Waits of 1 ms or more.
    pub waits: u64,
    /// Waits of [`WAIT_LOG_AT`] or more (each would log, quiet window aside).
    pub logged: u64,
    /// The time all waits of 1 ms or more took together.
    pub waited: Duration,
}

static WAITS: AtomicU64 = AtomicU64::new(0);
static WAITS_LOGGED: AtomicU64 = AtomicU64::new(0);
static WAITED_MICROS: AtomicU64 = AtomicU64::new(0);

fn note_wait(waited: Duration) {
    if waited >= Duration::from_millis(1) {
        WAITS.fetch_add(1, Ordering::Relaxed);
        WAITED_MICROS.fetch_add(waited.as_micros() as u64, Ordering::Relaxed);
        if waited >= WAIT_LOG_AT {
            WAITS_LOGGED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The lock's wait totals since the process started (every [`DbLock`]).
pub fn wait_stats() -> WaitStats {
    WaitStats {
        waits: WAITS.load(Ordering::Relaxed),
        logged: WAITS_LOGGED.load(Ordering::Relaxed),
        waited: Duration::from_micros(WAITED_MICROS.load(Ordering::Relaxed)),
    }
}

/// `K2_DB_LOCK_LOG=0` (or `false` / `off`) turns the log off. Read once.
pub fn lock_log_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        !matches!(
            std::env::var("K2_DB_LOCK_LOG").map(|v| v.trim().to_ascii_lowercase()).as_deref(),
            Ok("0") | Ok("false") | Ok("off")
        )
    })
}

/// The log line. `skipped`: lines this site would have logged since its
/// last one.
pub fn format_line(waited: Duration, held: Duration, site: &Location<'_>, skipped: u32) -> String {
    let mut line = format!(
        "[db] waited {}ms held {}ms at {}:{}",
        waited.as_millis(),
        held.as_millis(),
        site.file(),
        site.line()
    );
    if skipped > 0 {
        line.push_str(&format!(" (+{skipped} more here in the last {}s)", SITE_QUIET_FOR.as_secs()));
    }
    line
}

fn report(site: &'static Location<'static>, waited: Duration, held: Duration) {
    static LAST: OnceLock<Mutex<HashMap<(&'static str, u32), (Instant, u32)>>> = OnceLock::new();
    let key = (site.file(), site.line());
    let now = Instant::now();
    let skipped = {
        let mut last = LAST.get_or_init(Default::default).lock();
        if let Some((at, skipped)) = last.get_mut(&key) {
            if now.saturating_duration_since(*at) < SITE_QUIET_FOR {
                *skipped += 1;
                return;
            }
        }
        last.insert(key, (now, 0)).map(|(_, s)| s).unwrap_or(0)
    };
    let line = format_line(waited, held, site, skipped);
    #[cfg(any(test, feature = "test-util"))]
    capture::push(&line);
    crate::log_debug!("{line}");
}

/// Test-only: collect the lock log's lines while the guard lives.
#[cfg(any(test, feature = "test-util"))]
pub mod capture {
    use parking_lot::Mutex;

    /// (live captures, lines). Tests run in parallel, so lines are kept
    /// while any capture lives.
    static LINES: Mutex<(usize, Vec<String>)> = Mutex::new((0, Vec::new()));

    pub(super) fn push(line: &str) {
        let mut l = LINES.lock();
        if l.0 > 0 {
            l.1.push(line.to_string());
        }
    }

    /// Start collecting. Lines from every thread are kept, so assert on
    /// the lines that name your own call site.
    pub fn start() -> Capture {
        LINES.lock().0 += 1;
        Capture
    }

    #[must_use = "collecting stops when the guard drops"]
    pub struct Capture;

    impl Capture {
        pub fn lines(&self) -> Vec<String> {
            LINES.lock().1.clone()
        }
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            let mut l = LINES.lock();
            l.0 -= 1;
            if l.0 == 0 {
                l.1.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn mem() -> Arc<DbLock> {
        Arc::new(DbLock::new(Connection::open_in_memory().expect("open in-memory")))
    }

    #[test]
    fn a_long_hold_logs_one_line_naming_the_call_site() {
        let cap = capture::start();
        let db = mem();
        let site_line;
        {
            let db = db.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            let t = std::thread::spawn(move || {
                let line = line!() + 1;
                let g = db.lock();
                tx.send(line).expect("send line");
                std::thread::sleep(Duration::from_millis(300));
                drop(g);
            });
            site_line = rx.recv().expect("site line");
            t.join().expect("holder thread");
        }
        let site = format!("{}:{site_line}", file!());
        let mine: Vec<String> = cap.lines().into_iter().filter(|l| l.ends_with(&site)).collect();
        assert_eq!(mine.len(), 1, "one line for the 300 ms hold at {site}; got {mine:?}");
        assert!(mine[0].starts_with("[db] waited "), "{}", mine[0]);
        let held: u64 = mine[0]
            .split(" held ")
            .nth(1)
            .and_then(|s| s.split("ms").next())
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("no held ms in {}", mine[0]));
        assert!(held >= 300, "held {held} ms");
    }

    #[test]
    fn a_waiter_logs_its_wait_and_a_nested_lock_does_not_log_again() {
        let cap = capture::start();
        let db = mem();
        let holder_ready = Arc::new(std::sync::Barrier::new(2));
        let h = {
            let db = db.clone();
            let ready = holder_ready.clone();
            std::thread::spawn(move || {
                let g = db.lock();
                ready.wait();
                std::thread::sleep(Duration::from_millis(200));
                drop(g);
            })
        };
        holder_ready.wait();
        let wait_line = line!() + 1;
        let outer = db.lock();
        let nested_line = line!() + 1;
        let nested = db.lock();
        drop(nested);
        drop(outer);
        h.join().expect("holder");
        let waits: Vec<String> =
            cap.lines().into_iter().filter(|l| l.ends_with(&format!("{}:{wait_line}", file!()))).collect();
        assert_eq!(waits.len(), 1, "the waiter logs once: {waits:?}");
        let waited: u64 = waits[0]
            .strip_prefix("[db] waited ")
            .and_then(|s| s.split("ms").next())
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("no waited ms in {}", waits[0]));
        assert!(waited >= 100, "waited {waited} ms");
        let nested: Vec<String> =
            cap.lines().into_iter().filter(|l| l.ends_with(&format!("{}:{nested_line}", file!()))).collect();
        assert!(nested.is_empty(), "a nested hold is the outer hold: {nested:?}");
    }

    #[test]
    fn a_short_hold_logs_nothing_and_a_site_logs_once_per_window() {
        let cap = capture::start();
        let db = mem();
        let mut site_line = 0;
        for _ in 0..3 {
            site_line = line!() + 1;
            let g = db.lock();
            std::thread::sleep(Duration::from_millis(260));
            drop(g);
        }
        let quick_line = line!() + 1;
        drop(db.lock());
        let lines = cap.lines();
        let at = |n: u32| -> Vec<String> {
            lines.iter().filter(|l| l.ends_with(&format!("{}:{n}", file!()))).cloned().collect()
        };
        assert_eq!(at(site_line).len(), 1, "three long holds at one site in 10 s log once: {:?}", at(site_line));
        assert!(at(quick_line).is_empty(), "a quick hold logs nothing");
    }

    #[test]
    fn the_durability_line_names_the_journal_mode_and_synchronous_level() {
        let dir = std::env::temp_dir().join(format!("k2-durability-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let conn = crate::db::open_with_resilience(dir.join("k2.db")).expect("open file db");
        let line = crate::db::durability_line(&conn);
        drop(conn);
        std::fs::remove_dir_all(&dir).expect("remove temp dir");
        assert!(line.starts_with("[db] main database: journal_mode=wal synchronous="), "{line}");
        let level = line.rsplit("synchronous=").next().expect("level");
        assert!(
            ["0 (OFF)", "1 (NORMAL)", "2 (FULL)", "3 (EXTRA)"].contains(&level),
            "a known level: {line}"
        );
    }

    #[test]
    fn the_line_counts_skipped_lines() {
        let site = Location::caller();
        let l = format_line(Duration::from_millis(5), Duration::from_millis(1210), site, 3);
        assert!(l.starts_with("[db] waited 5ms held 1210ms at "), "{l}");
        assert!(l.ends_with(" (+3 more here in the last 10s)"), "{l}");
    }
}
