//! The Thread store's open never sticks on a transient lock (a daemon
//! hand-off where the old process still holds `k2-threads.redb`), and the
//! old daemon can let go of the file before the successor needs it.

use super::*;

const TABLE: TableDefinition<&str, &str> = TableDefinition::new("probe");

fn temp_store() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("k2-threads-open-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir.join("k2-threads.redb")
}

/// A handle that really works: one write, then the read sees it.
fn round_trip(db: &Database) {
    let txn = db.begin_write().expect("begin write");
    {
        let mut t = txn.open_table(TABLE).expect("open table");
        t.insert("k", "v").expect("insert");
    }
    txn.commit().expect("commit");
    let r = db.begin_read().expect("begin read");
    let t = r.open_table(TABLE).expect("open table for read");
    assert_eq!(t.get("k").expect("get").expect("row").value(), "v");
}

fn assert_locked(err: &str, path: &std::path::Path) {
    assert!(err.contains("Database already open"), "expected the redb lock error, got: {err}");
    assert!(err.contains(&path.display().to_string()), "the error names the file: {err}");
    assert!(err.contains("another process holds the file"), "the error says who holds it: {err}");
}

const HOLD_PATH: &str = "K2_TEST_REDB_HOLD_PATH";
const HOLD_READY: &str = "K2_TEST_REDB_HOLD_READY";
const HOLD_RELEASE: &str = "K2_TEST_REDB_HOLD_RELEASE";

/// Not a test: the second process for
/// [`a_second_process_holding_the_lock_fails_then_heals_without_a_restart`]
/// (this test binary run again with only this test, the env set).
#[test]
#[ignore = "helper process for the lock-holder test; run by that test"]
fn hold_lock_child() {
    let (Ok(path), Ok(ready), Ok(release)) =
        (std::env::var(HOLD_PATH), std::env::var(HOLD_READY), std::env::var(HOLD_RELEASE))
    else {
        // Run by hand with --ignored: nothing to hold.
        return;
    };
    let db = Database::create(&path).expect("child: open the store");
    std::fs::write(&ready, "1").expect("child: write ready");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !std::path::Path::new(&release).exists() {
        assert!(Instant::now() < deadline, "child: never told to release");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(db);
}

/// The sewlab bug: another process (the old daemon mid hand-off) holds the
/// redb lock when this one first opens the store. The call fails with the
/// lock error; once the holder lets go, a later call opens it — no restart.
#[test]
fn a_second_process_holding_the_lock_fails_then_heals_without_a_restart() {
    let path = temp_store();
    let dir = path.parent().expect("parent").to_path_buf();
    let ready = dir.join("ready");
    let release = dir.join("release");
    let mut child = std::process::Command::new(std::env::current_exe().expect("test exe"))
        .args([
            "--exact",
            "overlay::store::store_open_tests::hold_lock_child",
            "--ignored",
            "--test-threads=1",
        ])
        .env(HOLD_PATH, &path)
        .env(HOLD_READY, &ready)
        .env(HOLD_RELEASE, &release)
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawn the lock holder");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !ready.exists() {
        if let Some(status) = child.try_wait().expect("try_wait") {
            panic!("lock holder exited before holding the lock: {status}");
        }
        assert!(Instant::now() < deadline, "lock holder never became ready");
        std::thread::sleep(Duration::from_millis(20));
    }

    let slot = StoreSlot::new(Duration::from_millis(100), Duration::ZERO);
    let err = slot.get(|| path.clone()).err().expect("the store is held by another process");
    assert_locked(&err, &path);

    std::fs::write(&release, "1").expect("tell the holder to release");
    let status = child.wait().expect("wait for the holder");
    assert!(status.success(), "lock holder failed: {status}");
    std::thread::sleep(Duration::from_millis(150));

    let db = slot.get(|| path.clone()).expect("opens once the holder is gone, same process");
    round_trip(&db);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A failed open is not kept: inside the retry gap the last error comes
/// back (no new attempt), after it the next call opens the store.
#[test]
fn a_failed_open_is_not_cached() {
    let path = temp_store();
    let holder = Database::create(&path).expect("second handle holds the lock");
    let slot = StoreSlot::new(Duration::from_millis(300), Duration::ZERO);

    let first = slot.get(|| path.clone()).err().expect("held: the open fails");
    assert_locked(&first, &path);
    drop(holder);

    let in_gap = slot.get(|| path.clone()).err().expect("inside the retry gap: no new attempt yet");
    assert_eq!(in_gap, first, "inside the gap the last error comes back as is");

    std::thread::sleep(Duration::from_millis(350));
    let db = slot.get(|| path.clone()).expect("after the gap the store opens");
    round_trip(&db);
    let again = slot.get(|| path.clone()).expect("then it stays open");
    assert!(Arc::ptr_eq(&db, &again), "one handle, kept once open");
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// The bounded wait on first open bridges a short overlap: the holder
/// lets go while the open is waiting, and the open succeeds.
#[test]
fn the_first_open_waits_out_a_short_overlap() {
    let path = temp_store();
    let holder = Database::create(&path).expect("second handle holds the lock");
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        drop(holder);
    });
    let slot = StoreSlot::new(Duration::from_millis(500), Duration::from_secs(5));
    let db = slot.get(|| path.clone()).expect("the open waits for the holder to let go");
    round_trip(&db);
    t.join().expect("holder thread");
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// Hand-off: the old daemon's store holds the file; the successor's first
/// call fails. The old daemon releases at quiesce; the successor's next
/// call opens at once, and the old daemon refuses from then on.
#[test]
fn release_hands_the_file_to_a_successor_and_refuses_after() {
    let path = temp_store();
    let old = StoreSlot::new(Duration::from_millis(500), Duration::ZERO);
    let successor = StoreSlot::new(Duration::ZERO, Duration::ZERO);

    round_trip(&old.get(|| path.clone()).expect("the old daemon's store is open"));
    let err = successor.get(|| path.clone()).err().expect("overlap: the successor can't open yet");
    assert_locked(&err, &path);

    assert!(old.release(Duration::from_secs(2)), "nothing in flight: the file closes at once");
    let db = successor.get(|| path.clone()).expect("the successor opens right after the release");
    round_trip(&db);

    let refused = old.get(|| path.clone()).err().expect("the old daemon never reopens");
    assert_eq!(refused, RELEASED);
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// Release waits for an in-flight transaction's handle; when that outlives
/// the wait it reports so, and the file closes when the handle drops.
#[test]
fn release_waits_for_in_flight_handles() {
    let path = temp_store();
    let old = StoreSlot::new(Duration::from_millis(500), Duration::ZERO);

    let in_flight = old.get(|| path.clone()).expect("open");
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        drop(in_flight);
    });
    assert!(old.release(Duration::from_secs(5)), "release waits for the in-flight handle");
    t.join().expect("in-flight thread");
    let successor = StoreSlot::new(Duration::ZERO, Duration::ZERO);
    drop(successor.get(|| path.clone()).expect("the file is free after the release"));
    drop(successor);

    // Held past the wait: release says so; the file frees when it drops.
    let old2 = StoreSlot::new(Duration::from_millis(500), Duration::ZERO);
    let stuck = old2.get(|| path.clone()).expect("open again");
    assert!(!old2.release(Duration::from_millis(50)), "still in use after the wait");
    let successor2 = StoreSlot::new(Duration::ZERO, Duration::ZERO);
    assert_locked(&successor2.get(|| path.clone()).err().expect("still held"), &path);
    drop(stuck);
    round_trip(&successor2.get(|| path.clone()).expect("free once the last handle drops"));
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}
