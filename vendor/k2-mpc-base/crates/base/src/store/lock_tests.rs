//! One writer per store: the store library takes an exclusive OS lock on the store file when a
//! [`DocStore`] opens it and holds it for the store's life; the OS drops it when the process
//! exits, however it ends, and no lock file is left behind. A second opener, in this process or
//! another, is refused typed: [`StoreError::Locked`] (`MPC-E505` `store_locked`, "Another process
//! has this document open"), nothing read or written. [`DocStore::open_with_retry`] waits for the
//! holder instead, up to a deadline.
//!
//! These rows are also the guard that the store library still locks its file: a version that
//! dropped the lock would fail the second-process row.
//!
//! The second process is this test binary run again, holding the store
//! ([`hold_the_store_for_the_parent`]).
//!
//! Plant: `MPC_STORE_PLANT=store-lock-untyped` (the library's refusal passed through untyped, as
//! before) fails at the second opener's assert.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::*;
use crate::errors::Refusal as _;

/// The line the holder prints once it holds the store.
const HELD: &str = "mpc-store-lock: held";

struct TmpDir(PathBuf);

impl TmpDir {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "k2mb-lock-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }

    fn db(&self) -> PathBuf {
        self.0.join("docs.redb")
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Assert `r` is the typed refusal of a store another opener holds.
fn assert_locked(r: Result<DocStore, StoreError>, what: &str) {
    match r {
        Err(StoreError::Locked) => {}
        Err(e) => panic!("row: {what}: refused untyped: {e:?}"),
        Ok(_) => panic!("row: {what}: a second opener got the store"),
    }
}

/// The second process; killed when dropped, so a failing row leaves no holder behind.
struct Holder(Child);

impl Holder {
    /// Kill it (an abrupt end, as a killed process has) and reap it.
    fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        self.kill();
    }
}

/// This test binary run again as [`hold_the_store_for_the_parent`], holding the store at `db`
/// for `hold_ms` (0: until killed, at most a minute); returns once it holds it.
fn holder(db: &Path, hold_ms: u64) -> Holder {
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .args([
            "--exact",
            "store::lock_tests::hold_the_store_for_the_parent",
            "--ignored",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env("MPC_STORE_HOLD", db)
        .env("MPC_STORE_HOLD_MS", hold_ms.to_string())
        .env_remove("MPC_STORE_PLANT")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let out = child.stdout.take().unwrap();
    let mut held = false;
    for line in BufReader::new(out).lines() {
        let line = line.unwrap();
        // the harness may print the test's name on the same line first
        if line.trim_end().ends_with(HELD) {
            held = true;
            break;
        }
    }
    assert!(held, "control: the second process holds the store");
    Holder(child)
}

/// The second process's body, run only by [`holder`] (`MPC_STORE_HOLD` names the store): open
/// the store, say so, and hold it for `MPC_STORE_HOLD_MS` before dropping it and exiting.
#[test]
#[ignore = "the second process of the store-lock rows: run by name: crates/base/src/store/lock_tests.rs"]
fn hold_the_store_for_the_parent() {
    let Ok(path) = std::env::var("MPC_STORE_HOLD") else {
        return;
    };
    let ms: u64 = std::env::var("MPC_STORE_HOLD_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let store = DocStore::open(&path).unwrap();
    println!("{HELD}");
    std::thread::sleep(Duration::from_millis(if ms == 0 { 60_000 } else { ms }));
    drop(store);
}

/// Two opens of one store in one process: the second is refused typed, `MPC-E505`, exit 6,
/// with the wording "Another process has this document open", and the first keeps working.
/// Dropped, the store opens again.
/// `MPC_STORE_PLANT=store-lock-untyped` fails at the second open's assert.
#[test]
fn a_second_open_in_one_process_is_refused_typed() {
    let t = TmpDir::new("one-process");
    let first = DocStore::open(t.db()).unwrap();
    let r = DocStore::open(t.db());
    let e = match &r {
        Err(e) => e.clone(),
        Ok(_) => StoreError::Corrupt("opened"),
    };
    assert_locked(r, "a second open in this process");
    let rep = e.report();
    assert_eq!(rep.id(), "MPC-E505", "row: store_locked");
    assert_eq!(rep.exit(), 6, "row: exit 6");
    assert!(
        rep.message()
            .starts_with("Another process has this document open"),
        "row: the wording: {}",
        rep.message()
    );
    assert!(
        first.find(&key()).unwrap().is_none(),
        "row: the first open still reads"
    );
    drop(first);
    let again = DocStore::open(t.db());
    assert!(again.is_ok(), "row: released on drop: {:?}", again.err());
}

/// Another process holding the store: this one's open is refused typed. Once that process is
/// killed (an abrupt end), the store opens here: the OS dropped its lock, and nothing
/// it left behind stops the next opener.
#[test]
fn a_second_process_is_refused_and_a_killed_holder_frees_the_store() {
    let t = TmpDir::new("two-process");
    drop(DocStore::open(t.db()).unwrap());
    let mut child = holder(&t.db(), 0);
    assert_locked(
        DocStore::open(t.db()),
        "an open while another process holds the store",
    );
    child.kill();
    let after = DocStore::open(t.db());
    assert!(
        after.is_ok(),
        "row: after the holder is killed the store opens: {:?}",
        after.err()
    );
    let files: Vec<_> = std::fs::read_dir(&t.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        files,
        vec!["docs.redb".to_owned()],
        "row: no lock file left behind"
    );
}

/// The wait option: an open with a wait succeeds once the holder exits (well before the
/// deadline), and is refused typed when the holder outlasts the deadline.
#[test]
fn an_open_with_a_wait_takes_the_store_when_the_holder_exits() {
    let t = TmpDir::new("wait");
    drop(DocStore::open(t.db()).unwrap());
    let mut child = holder(&t.db(), 400);
    let start = Instant::now();
    let r = DocStore::open_with_retry(t.db(), Duration::from_secs(30));
    assert!(r.is_ok(), "row: the wait takes the store: {:?}", r.err());
    assert!(
        start.elapsed() < Duration::from_secs(30),
        "row: it took the store when the holder exited"
    );
    drop(r);
    let _ = child.0.wait();
    let mut child = holder(&t.db(), 0);
    assert_locked(
        DocStore::open_with_retry(t.db(), Duration::from_millis(200)),
        "a wait past its deadline",
    );
    child.kill();
}

fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    }
}
