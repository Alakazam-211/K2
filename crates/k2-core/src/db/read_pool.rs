//! Read-only connections beside the main database's one writer.
//!
//! The main database has one connection behind one lock ([`DbLock`]): every
//! read waits behind every write, however slow the writer is. In WAL mode a
//! reader on its own connection never waits for a writer and sees the last
//! committed state, so the hot GET routes read through [`DbLock::read`] (or
//! [`super::read`]) on a small pool of read-only connections instead.
//!
//! - **Never half-written state.** Each `read` runs in one read transaction
//!   (`BEGIN … COMMIT`): every statement in it sees the same committed
//!   snapshot, and nothing a writer has not committed.
//! - **Writes stay on the writer.** The connections are opened read-only and
//!   with `query_only = ON`, so a write through one is an error, never a
//!   second writer. A `read` is for pure reads: a caller that writes based on
//!   what it read takes the main lock for both (vs-live SP-1).
//! - **Read your own writes.** A thread that holds the main lock (an open
//!   write transaction) reads on the main connection, so it sees its own
//!   uncommitted rows. A `read` inside a `read` reuses the same connection
//!   (same snapshot; never a second pool slot, so it cannot deadlock).
//! - **Waits for any free reader**, never one picked on arrival.
//! - **Falls back to the main connection** when there is no pool: an
//!   in-memory database, one not in WAL mode, a reader that failed to open,
//!   or `K2_DB_READ_POOL=0`.

use super::DbLock;
use parking_lot::{Condvar, Mutex};
use rusqlite::{Connection, OpenFlags};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Read-only connections the main database gets.
pub const READERS: usize = 4;

pub struct ReadPool {
    path: PathBuf,
    idle: Mutex<Vec<Connection>>,
    freed: Condvar,
    size: usize,
}

impl ReadPool {
    /// Open `size` read-only connections to `path`. Any failure opens none.
    pub fn open(path: &Path, size: usize) -> Result<Self, String> {
        let mut conns = Vec::with_capacity(size);
        for _ in 0..size {
            conns.push(open_reader(path).map_err(|e| format!("open a reader on {}: {e}", path.display()))?);
        }
        Ok(Self { path: path.to_path_buf(), idle: Mutex::new(conns), freed: Condvar::new(), size })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// Readers not in use right now.
    pub fn idle(&self) -> usize {
        self.idle.lock().len()
    }

    fn checkout(&self) -> Connection {
        let mut idle = self.idle.lock();
        loop {
            if let Some(c) = idle.pop() {
                return c;
            }
            self.freed.wait(&mut idle);
        }
    }

    fn checkin(&self, conn: Connection) {
        self.idle.lock().push(conn);
        self.freed.notify_one();
    }
}

fn open_reader(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.busy_timeout(std::time::Duration::from_millis(500))?;
    conn.execute_batch(
        "PRAGMA query_only = ON;\n\
         PRAGMA cache_size = -8000;\n\
         PRAGMA mmap_size = 67108864;\n\
         PRAGMA temp_store = MEMORY;",
    )?;
    // The file must be in WAL mode, or a reader would block the writer.
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(rusqlite::Error::InvalidParameterName(format!("journal_mode is {mode}, not wal")));
    }
    Ok(conn)
}

thread_local! {
    /// The pool connection this thread is reading on, and whose lock it
    /// belongs to: a nested `read` reuses it.
    static IN_READ: Cell<Option<(usize, *const Connection)>> = const { Cell::new(None) };
}

/// Where a `read` ran (counted per thread, for tests).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReadCounts {
    /// On a pool connection.
    pub pool: u64,
    /// On the main connection because this thread held its lock.
    pub held_lock: u64,
    /// On the main connection because there is no pool.
    pub no_pool: u64,
    /// Nested in another `read` (its connection).
    pub nested: u64,
}

#[cfg(any(test, feature = "test-util"))]
thread_local! {
    static COUNTS: Cell<ReadCounts> = const { Cell::new(ReadCounts { pool: 0, held_lock: 0, no_pool: 0, nested: 0 }) };
}

/// Test-only: this thread's `read` counts so far.
#[cfg(any(test, feature = "test-util"))]
pub fn read_counts_for_test() -> ReadCounts {
    COUNTS.with(|c| c.get())
}

#[allow(unused_variables)]
fn count(f: fn(&mut ReadCounts)) {
    #[cfg(any(test, feature = "test-util"))]
    COUNTS.with(|c| {
        let mut v = c.get();
        f(&mut v);
        c.set(v);
    });
}

/// A pool connection on loan to one `read`. Dropping it (also on a panic)
/// ends the read transaction, restores the thread's nested-read slot and
/// hands the connection back.
struct Lease {
    pool: Arc<ReadPool>,
    conn: Option<Connection>,
    prev: Option<(usize, *const Connection)>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        IN_READ.with(|s| s.set(self.prev));
        if let Some(conn) = self.conn.take() {
            if !conn.is_autocommit() {
                if let Err(e) = conn.execute_batch("ROLLBACK") {
                    // A reader that can't end its transaction would pin an
                    // old snapshot: drop it rather than hand it back.
                    crate::log_debug!("[db] a reader could not end its read ({e}); it is closed");
                    return;
                }
            }
            self.pool.checkin(conn);
        }
    }
}

impl DbLock {
    /// Open the read pool on `path` (the file this connection has open).
    /// Replaces a pool already there.
    pub fn open_readers(&self, path: &Path, size: usize) -> Result<(), String> {
        let pool = ReadPool::open(path, size)?;
        *self.readers.write() = Some(Arc::new(pool));
        Ok(())
    }

    /// Close the read pool (before the database file is replaced). Reads
    /// use the main connection until [`Self::open_readers`]. A read already
    /// running finishes on its connection, which is then closed.
    pub fn close_readers(&self) {
        *self.readers.write() = None;
    }

    /// The pool's size, `None` when reads use the main connection.
    pub fn readers(&self) -> Option<usize> {
        self.readers.read().as_ref().map(|p| p.size())
    }

    /// Run a pure read on a read-only connection (see the module docs).
    /// The closure sees one committed snapshot.
    #[track_caller]
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        if self.is_owned_by_current_thread() {
            count(|c| c.held_lock += 1);
            let g = self.lock();
            return f(&g);
        }
        let me = self as *const DbLock as usize;
        let prev = IN_READ.with(|s| s.get());
        if let Some((owner, conn)) = prev {
            if owner == me {
                count(|c| c.nested += 1);
                // SAFETY: the outer `read` on this thread holds that
                // connection (its `Lease`) until it returns, after this.
                return f(unsafe { &*conn });
            }
        }
        let pool = self.readers.read().clone();
        let Some(pool) = pool else {
            count(|c| c.no_pool += 1);
            let g = self.lock();
            return f(&g);
        };
        count(|c| c.pool += 1);
        let conn = pool.checkout();
        let lease = Lease { pool, conn: Some(conn), prev };
        let conn = lease.conn.as_ref().expect("leased connection");
        if let Err(e) = conn.execute_batch("BEGIN") {
            crate::log_debug!("[db] a reader could not start its read ({e}); its statements run one by one");
        }
        IN_READ.with(|s| s.set(Some((me, conn as *const Connection))));
        let out = f(conn);
        if !conn.is_autocommit() {
            if let Err(e) = conn.execute_batch("COMMIT") {
                crate::log_debug!("[db] a reader could not end its read ({e})");
            }
        }
        drop(lease);
        out
    }
}

/// Open the main database's read pool unless something rules it out; logs
/// what it did. Called once the migrations have run.
pub fn start_for_main(handle: &DbLock, path: &Path) {
    if matches!(
        std::env::var("K2_DB_READ_POOL").map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Ok("0") | Ok("false") | Ok("off")
    ) {
        crate::log_debug!("[db] read pool off (K2_DB_READ_POOL=0): reads use the main connection");
        return;
    }
    let mode: String = handle
        .lock()
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap_or_else(|_| "?".to_string());
    if !mode.eq_ignore_ascii_case("wal") {
        crate::log_debug!("[db] read pool off (journal_mode={mode}, not wal): reads use the main connection");
        return;
    }
    match handle.open_readers(path, READERS) {
        Ok(()) => crate::log_debug!("[db] read pool: {READERS} read-only connections"),
        Err(e) => crate::log_debug!("[db] read pool off ({e}): reads use the main connection"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    struct TempDb {
        dir: PathBuf,
        path: PathBuf,
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn temp_db() -> TempDb {
        let dir = std::env::temp_dir().join(format!("k2-read-pool-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("k2.db");
        TempDb { dir, path }
    }

    /// A WAL file DB with one table, behind a `DbLock` with readers.
    fn main_db(t: &TempDb, readers: bool) -> Arc<DbLock> {
        let conn = crate::db::open_with_resilience(&t.path).expect("open");
        conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL);").expect("schema");
        let db = Arc::new(DbLock::new(conn));
        if readers {
            db.open_readers(&t.path, READERS).expect("readers");
        }
        db
    }

    fn rows(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0)).expect("count")
    }

    #[test]
    fn a_read_does_not_wait_for_a_slow_writer() {
        let t = temp_db();
        let db = main_db(&t, true);
        db.lock().execute("INSERT INTO t (v) VALUES ('a')", []).expect("seed");
        let held = Arc::new(std::sync::Barrier::new(2));
        let w = {
            let (db, held) = (db.clone(), held.clone());
            std::thread::spawn(move || {
                let g = db.lock();
                g.execute_batch("BEGIN IMMEDIATE; INSERT INTO t (v) VALUES ('uncommitted');").expect("write");
                held.wait();
                std::thread::sleep(Duration::from_millis(1500));
                g.execute_batch("COMMIT").expect("commit");
            })
        };
        held.wait();
        let t0 = Instant::now();
        let n = db.read(rows);
        let took = t0.elapsed();
        assert_eq!(n, 1, "the reader sees the committed row only, never the open write");
        assert!(took < Duration::from_millis(200), "the read waited {took:?} behind the writer");
        assert_eq!(read_counts_for_test().pool, 1);
        w.join().expect("writer");
        assert_eq!(db.read(rows), 2, "after the commit the next read sees it");
    }

    #[test]
    fn inside_a_write_transaction_a_read_sees_the_uncommitted_row() {
        let t = temp_db();
        let db = main_db(&t, true);
        let before = read_counts_for_test();
        let g = db.lock();
        g.execute_batch("BEGIN; INSERT INTO t (v) VALUES ('mine');").expect("write");
        let n = db.read(rows);
        g.execute_batch("ROLLBACK").expect("rollback");
        drop(g);
        assert_eq!(n, 1, "the holder reads its own uncommitted row");
        let after = read_counts_for_test();
        assert_eq!(after.held_lock, before.held_lock + 1, "the read took the main connection");
        assert_eq!(after.pool, before.pool, "not a pool connection");
    }

    #[test]
    fn a_read_runs_in_one_snapshot_and_a_nested_read_shares_it() {
        let t = temp_db();
        let db = main_db(&t, true);
        db.lock().execute("INSERT INTO t (v) VALUES ('a')", []).expect("seed");
        let (first, second, nested) = db.read(|c| {
            let first = rows(c);
            // A write commits between the two statements of this read.
            let db2 = db.clone();
            std::thread::spawn(move || db2.lock().execute("INSERT INTO t (v) VALUES ('b')", []).expect("write"))
                .join()
                .expect("writer");
            let second = rows(c);
            let nested = db.read(rows);
            (first, second, nested)
        });
        assert_eq!((first, second, nested), (1, 1, 1), "one snapshot for the whole read");
        assert!(read_counts_for_test().nested >= 1);
        assert_eq!(db.read(rows), 2);
        assert_eq!(db.readers(), Some(READERS));
        let pool = db.readers.read().clone().expect("pool");
        assert_eq!(pool.idle(), READERS, "every reader is back");
    }

    #[test]
    fn a_reader_refuses_writes() {
        let t = temp_db();
        let db = main_db(&t, true);
        let err = db.read(|c| c.execute("INSERT INTO t (v) VALUES ('x')", [])).expect_err("a reader can't write");
        assert!(err.to_string().to_ascii_lowercase().contains("readonly") || err.to_string().contains("read-only"), "{err}");
        assert_eq!(db.read(rows), 0);
    }

    #[test]
    fn readers_wait_for_any_free_one_and_all_come_back() {
        let t = temp_db();
        let db = main_db(&t, true);
        let threads: Vec<_> = (0..READERS * 3)
            .map(|_| {
                let db = db.clone();
                std::thread::spawn(move || {
                    db.read(|c| {
                        std::thread::sleep(Duration::from_millis(20));
                        rows(c)
                    })
                })
            })
            .collect();
        for th in threads {
            assert_eq!(th.join().expect("reader thread"), 0);
        }
        let pool = db.readers.read().clone().expect("pool");
        assert_eq!(pool.idle(), READERS);
    }

    #[test]
    fn without_a_pool_a_read_uses_the_main_connection() {
        let t = temp_db();
        let db = main_db(&t, false);
        let before = read_counts_for_test();
        assert_eq!(db.read(rows), 0);
        assert_eq!(read_counts_for_test().no_pool, before.no_pool + 1);
        assert_eq!(db.readers(), None);
    }

    #[test]
    fn closing_and_reopening_readers_follows_the_new_file() {
        let a = temp_db();
        let b = temp_db();
        let db = main_db(&a, true);
        let other = crate::db::open_with_resilience(&b.path).expect("open b");
        other
            .execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL); INSERT INTO t (v) VALUES ('b1'), ('b2');")
            .expect("b rows");
        assert_eq!(db.read(rows), 0);
        db.close_readers();
        assert_eq!(db.readers(), None);
        db.open_readers(&b.path, 2).expect("readers on b");
        assert_eq!(db.read(rows), 2, "the reopened readers read the new file");
        drop(other);
    }

    #[test]
    fn a_panicking_read_hands_its_connection_back() {
        let t = temp_db();
        let db = main_db(&t, true);
        let db2 = db.clone();
        let r = std::thread::spawn(move || db2.read(|_| panic!("boom"))).join();
        assert!(r.is_err(), "the read panicked");
        let pool = db.readers.read().clone().expect("pool");
        assert_eq!(pool.idle(), READERS, "the connection came back");
        assert_eq!(db.read(rows), 0);
    }

    #[test]
    fn the_main_database_commits_with_synchronous_normal_only_in_wal_mode() {
        let t = temp_db();
        let conn = crate::db::open_with_resilience(&t.path).expect("open");
        crate::db::set_main_durability(&conn);
        let sync: i64 = conn.query_row("PRAGMA synchronous", [], |r| r.get(0)).expect("synchronous");
        let limit: i64 = conn.query_row("PRAGMA journal_size_limit", [], |r| r.get(0)).expect("limit");
        assert_eq!(sync, 1, "NORMAL in WAL mode");
        assert_eq!(limit, 134_217_728);

        let mem = Connection::open_in_memory().expect("mem");
        let before: i64 = mem.query_row("PRAGMA synchronous", [], |r| r.get(0)).expect("synchronous");
        crate::db::set_main_durability(&mem);
        let after: i64 = mem.query_row("PRAGMA synchronous", [], |r| r.get(0)).expect("synchronous");
        assert_eq!(after, before, "not WAL: unchanged");
    }

    #[test]
    fn an_in_memory_database_gets_no_pool() {
        let db = DbLock::new(Connection::open_in_memory().expect("mem"));
        let err = db.open_readers(Path::new(":memory:"), 1).expect_err("no readers for memory");
        assert!(!err.is_empty());
        assert_eq!(db.readers(), None);
    }
}
