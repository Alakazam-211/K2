//! Stage B home switch. Every test builds its own `~/.k2` look-alike in a
//! scratch dir; nothing touches the real home.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

fn scratch() -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    // Not temp_dir(): it reads TMPDIR with no lock, and this test may run
    // while another test has TMPDIR on a folder it is about to delete.
    let d = crate::test_env::stable_temp_dir().join(format!(
        "k2-home-switch-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).expect("scratch dir");
    d
}

fn opts() -> SwitchOpts {
    SwitchOpts { defer: None, by: "test".into() }
}

/// A migrated K2 database with `n` live workspaces.
fn make_db(path: &Path, n: usize) {
    crate::db::bootstrap_test_db_at(path).expect("bootstrap");
    let conn = Connection::open(path).expect("open");
    for i in 0..n {
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![format!("p{i}"), format!("ws-{i}"), format!("/tmp/ws-{i}")],
        )
        .expect("insert project");
    }
}

// Windows: only the unix rename tests count the moved rows.
#[cfg(unix)]
fn live_count(path: &Path) -> i64 {
    let conn = Connection::open(path).expect("open for count");
    conn.query_row(
        "SELECT count(*) FROM projects WHERE id NOT IN ('_orphan', '_broadcast')",
        [],
        |r| r.get(0),
    )
    .expect("count projects")
}

// Windows: only the unix rename tests check the moved file.
#[cfg(unix)]
fn integrity(path: &Path) -> String {
    let conn = Connection::open(path).expect("open for integrity");
    conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).expect("integrity_check")
}

fn read(p: &Path) -> Vec<u8> {
    std::fs::read(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn quarantined(dir: &Path) -> Vec<String> {
    let q = dir.join(QUARANTINE_DIR);
    let mut v: Vec<String> = match std::fs::read_dir(&q) {
        Ok(rd) => rd.map(|e| e.expect("entry").file_name().to_string_lossy().to_string()).collect(),
        Err(_) => Vec::new(),
    };
    v.sort();
    v
}

// Windows: no switch runs there, so no marker is written.
#[cfg(unix)]
fn marker(dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&read(&dir.join(MARKER))).expect("marker is JSON")
}

#[cfg(unix)]
fn assert_link(dir: &Path) {
    let old = dir.join(OLD_NAME);
    let m = std::fs::symlink_metadata(&old).expect("k2so.db exists");
    assert!(m.file_type().is_symlink(), "k2so.db must be a symlink after the switch");
    assert_eq!(
        std::fs::read_link(&old).expect("readlink"),
        PathBuf::from(NEW_NAME),
        "the compat link is relative, to k2.db"
    );
}

/// A legacy home whose last daemon died with rows still in the WAL: the
/// rename folds them into `k2.db`, leaves no side files under either name,
/// and every row survives.
#[cfg(unix)]
#[test]
fn legacy_only_renames_and_keeps_wal_rows() {
    let src = scratch();
    let dir = scratch();
    let live = src.join(OLD_NAME);
    make_db(&live, 3);
    let conn = Connection::open(&live).expect("open live");
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;")
        .expect("wal");
    for i in 3..7 {
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![format!("p{i}"), format!("ws-{i}"), format!("/tmp/ws-{i}")],
        )
        .expect("insert into wal");
    }
    // A crash image: main file + WAL copied while the writer is open.
    let wal = crate::private_home::sibling(&live, "-wal");
    assert!(std::fs::metadata(&wal).expect("wal exists").len() > 0, "rows must sit in the WAL");
    std::fs::copy(&live, dir.join(OLD_NAME)).expect("copy db");
    std::fs::copy(&wal, crate::private_home::sibling(&dir.join(OLD_NAME), "-wal")).expect("copy wal");
    drop(conn);

    let rep = switch_home_db(&dir, &opts());
    assert_eq!(rep.outcome, Outcome::Renamed, "{rep:?}");
    assert_eq!(rep.path, dir.join(NEW_NAME));
    assert!(rep.quarantined.is_empty(), "{rep:?}");
    assert_link(&dir);
    for s in SIDECARS {
        assert!(
            std::fs::symlink_metadata(crate::private_home::sibling(&dir.join(OLD_NAME), s)).is_err(),
            "no k2so.db{s} may remain"
        );
        assert!(
            std::fs::symlink_metadata(crate::private_home::sibling(&dir.join(NEW_NAME), s)).is_err(),
            "the switch itself leaves no k2.db{s}"
        );
    }
    assert_eq!(live_count(&dir.join(NEW_NAME)), 7, "the WAL rows were folded in");
    assert_eq!(integrity(&dir.join(NEW_NAME)), "ok");
    let m = marker(&dir);
    assert_eq!(m["outcome"], "renamed");
    assert_eq!(m["link"], "k2so.db -> k2.db");

    // Second boot: a no-op (same file, marker untouched).
    let before = read(&dir.join(MARKER));
    let rep2 = switch_home_db(&dir, &opts());
    assert_eq!(rep2.outcome, Outcome::AlreadyDone, "{rep2:?}");
    assert_eq!(rep2.path, dir.join(NEW_NAME));
    assert_eq!(read(&dir.join(MARKER)), before, "a second start must not rewrite the marker");
    assert!(quarantined(&dir).is_empty());

    // Downgrade: the 0.46.5 rule (resolve) picks k2.db, never the link.
    assert_eq!(crate::db::resolve_home_db_path(&dir), dir.join(NEW_NAME));
}

/// The Stage A stub `k2.db` (sentinels only) beside a live `k2so.db`
/// (z3mbp, z3mbpZ, Baden): the stub goes to quarantine with its bytes
/// intact, then the live file is renamed.
#[cfg(unix)]
#[test]
fn stub_k2_db_is_quarantined_then_legacy_renamed() {
    let dir = scratch();
    make_db(&dir.join(OLD_NAME), 5);
    make_db(&dir.join(NEW_NAME), 0);
    let stub_bytes = read(&dir.join(NEW_NAME));
    let live_bytes = read(&dir.join(OLD_NAME));

    let rep = switch_home_db(&dir, &opts());
    assert_eq!(rep.outcome, Outcome::Renamed, "{rep:?}");
    assert_eq!(rep.quarantined.len(), 1, "{rep:?}");
    assert_eq!(read(&rep.quarantined[0]), stub_bytes, "the stub is moved, never changed");
    let qname = rep.quarantined[0].file_name().unwrap().to_string_lossy().to_string();
    assert!(qname.ends_with("-k2.db"), "DB-A7: <stamp>-k2.db, got {qname}");
    assert!(
        crate::private_home::is_db_file_name(&qname),
        "DB-A7: the permission sweep must see a quarantined database: {qname}"
    );
    assert_eq!(live_count(&dir.join(NEW_NAME)), 5);
    assert_eq!(read(&dir.join(NEW_NAME)).len(), live_bytes.len());
    assert_link(&dir);
    assert_eq!(marker(&dir)["quarantined"].as_array().expect("array").len(), 1);
}

/// A zero-byte `k2.db` (`touch`) beside a live `k2so.db` counts as a stub.
#[cfg(unix)]
#[test]
fn empty_k2_db_file_is_quarantined() {
    let dir = scratch();
    make_db(&dir.join(OLD_NAME), 2);
    std::fs::write(dir.join(NEW_NAME), b"").expect("touch");
    let rep = switch_home_db(&dir, &opts());
    assert_eq!(rep.outcome, Outcome::Renamed, "{rep:?}");
    assert_eq!(quarantined(&dir).len(), 1);
    assert_eq!(live_count(&dir.join(NEW_NAME)), 2);
}

/// `k2.db` is the live file and `k2so.db` is the stale one: keep `k2.db`,
/// quarantine `k2so.db`, put the link in its place.
#[cfg(unix)]
#[test]
fn live_k2_db_keeps_and_stale_k2so_is_quarantined() {
    let dir = scratch();
    make_db(&dir.join(OLD_NAME), 0);
    make_db(&dir.join(NEW_NAME), 4);
    let stale = read(&dir.join(OLD_NAME));
    let rep = switch_home_db(&dir, &opts());
    assert_eq!(rep.outcome, Outcome::KeptK2Db, "{rep:?}");
    assert_eq!(rep.path, dir.join(NEW_NAME));
    assert_eq!(rep.quarantined.len(), 1);
    assert_eq!(read(&rep.quarantined[0]), stale);
    assert_link(&dir);
    assert_eq!(live_count(&dir.join(NEW_NAME)), 4);
}

/// Both files hold workspaces: refuse. Nothing moves, today's pick stays,
/// the note explains, and the owner notice is due exactly once.
// Windows defers before these checks (see windows_defers_every_case_and_moves_nothing).
#[cfg(unix)]
#[test]
fn both_live_refuses_and_moves_nothing() {
    let dir = scratch();
    make_db(&dir.join(OLD_NAME), 3);
    make_db(&dir.join(NEW_NAME), 2);
    let (old_b, new_b) = (read(&dir.join(OLD_NAME)), read(&dir.join(NEW_NAME)));

    let rep = switch_home_db(&dir, &opts());
    assert_eq!(rep.outcome, Outcome::Blocked { k2_projects: 2, k2so_projects: 3 }, "{rep:?}");
    assert_eq!(rep.path, dir.join(OLD_NAME), "today's pick: more workspaces");
    assert_eq!(read(&dir.join(OLD_NAME)), old_b);
    assert_eq!(read(&dir.join(NEW_NAME)), new_b);
    assert!(quarantined(&dir).is_empty());
    assert!(!dir.join(MARKER).exists(), "no marker on a refusal");
    let note: serde_json::Value =
        serde_json::from_slice(&read(&dir.join(BLOCKED_NOTE))).expect("blocked note");
    assert_eq!(note["k2sodbWorkspaces"], 3);
    assert_eq!(note["k2dbWorkspaces"], 2);
    assert!(note["message"].as_str().expect("message").contains("did not rename or move"));

    assert!(blocked_notice_due(&dir));
    mark_blocked_notice_filed(&dir);
    assert!(!blocked_notice_due(&dir));
    // A second boot keeps the filed flag (no second notice).
    let rep2 = switch_home_db(&dir, &opts());
    assert!(matches!(rep2.outcome, Outcome::Blocked { .. }));
    assert!(!blocked_notice_due(&dir), "the notice is filed once per refusal");
}

/// Another connection holds `k2so.db` open (a draining predecessor): the
/// switch changes nothing and uses the file as it is.
// Windows defers before these checks (see windows_defers_every_case_and_moves_nothing).
#[cfg(unix)]
#[test]
fn in_use_defers_and_moves_nothing() {
    let dir = scratch();
    make_db(&dir.join(OLD_NAME), 2);
    let other = Connection::open(dir.join(OLD_NAME)).expect("other");
    other.execute_batch("PRAGMA journal_mode = WAL;").expect("wal");
    let n: i64 = other.query_row("SELECT count(*) FROM projects", [], |r| r.get(0)).expect("read");
    assert!(n >= 2);

    let rep = switch_home_db(&dir, &opts());
    match &rep.outcome {
        Outcome::Deferred { reason } => assert!(reason.contains("in use"), "{reason}"),
        o => panic!("expected Deferred, got {o:?}"),
    }
    assert_eq!(rep.path, dir.join(OLD_NAME));
    assert!(std::fs::symlink_metadata(dir.join(NEW_NAME)).is_err(), "k2.db must not appear");
    assert!(!dir.join(MARKER).exists());
    drop(other);

    // Once the other connection is gone, the next boot renames.
    let rep2 = switch_home_db(&dir, &opts());
    assert_eq!(rep2.outcome, Outcome::Renamed, "{rep2:?}");
    assert_eq!(live_count(&dir.join(NEW_NAME)), 2);
}

/// The hand-off successor passes a defer reason: nothing is touched.
#[test]
fn defer_option_changes_nothing() {
    let dir = scratch();
    make_db(&dir.join(OLD_NAME), 1);
    make_db(&dir.join(NEW_NAME), 0);
    let o = SwitchOpts { defer: Some("hand-off".into()), by: "test".into() };
    let rep = switch_home_db(&dir, &o);
    assert_eq!(rep.outcome, Outcome::Deferred { reason: "hand-off".into() });
    assert_eq!(rep.path, dir.join(OLD_NAME), "today's pick");
    assert!(dir.join(NEW_NAME).is_file() && dir.join(OLD_NAME).is_file());
    assert!(quarantined(&dir).is_empty());
}

#[cfg(unix)]
#[test]
fn fresh_install_is_k2_db_with_link() {
    let dir = scratch();
    let rep = switch_home_db(&dir, &opts());
    assert_eq!(rep.outcome, Outcome::Fresh);
    assert_eq!(rep.path, dir.join(NEW_NAME));
    assert_link(&dir);
    make_db(&rep.path, 0);
    let rep2 = switch_home_db(&dir, &opts());
    assert_eq!(rep2.outcome, Outcome::AlreadyDone, "{rep2:?}");
    assert_eq!(marker(&dir)["outcome"], "fresh", "the second boot keeps the first marker");
}

/// Crash after the rename, before the link: the next boot adds the link
/// and the marker. Crash after the link: only the marker is added.
#[cfg(unix)]
#[test]
fn crash_states_finish_on_next_boot() {
    let dir = scratch();
    make_db(&dir.join(NEW_NAME), 3);
    let rep = switch_home_db(&dir, &opts());
    assert_eq!(rep.outcome, Outcome::AlreadyDone, "{rep:?}");
    assert_link(&dir);
    assert!(dir.join(MARKER).exists());

    let dir = scratch();
    make_db(&dir.join(NEW_NAME), 3);
    std::os::unix::fs::symlink(NEW_NAME, dir.join(OLD_NAME)).expect("link");
    let rep = switch_home_db(&dir, &opts());
    assert_eq!(rep.outcome, Outcome::AlreadyDone);
    assert!(dir.join(MARKER).exists());
    assert_eq!(live_count(&dir.join(NEW_NAME)), 3);
}

/// A stray `k2.db-wal` with no `k2.db` beside it would be read as the
/// renamed file's WAL; it goes to quarantine first.
#[cfg(unix)]
#[test]
fn stray_k2_db_wal_is_quarantined_before_rename() {
    let dir = scratch();
    make_db(&dir.join(OLD_NAME), 2);
    std::fs::write(crate::private_home::sibling(&dir.join(NEW_NAME), "-wal"), b"not ours")
        .expect("stray wal");
    let rep = switch_home_db(&dir, &opts());
    assert_eq!(rep.outcome, Outcome::Renamed, "{rep:?}");
    assert_eq!(rep.quarantined.len(), 1, "{rep:?}");
    assert_eq!(read(&rep.quarantined[0]), b"not ours");
    assert_eq!(integrity(&dir.join(NEW_NAME)), "ok");
    assert_eq!(live_count(&dir.join(NEW_NAME)), 2);
}

/// The compat link is safe for outside readers and writers: SQLite resolves
/// the symlink before naming `-wal` / `-shm`, so a connection opened as
/// `k2so.db` shares `k2.db`'s WAL (sees rows that exist only in the WAL)
/// and never creates `k2so.db-wal`. If this ever fails, the link must go.
#[cfg(unix)]
#[test]
fn symlink_reader_shares_wal() {
    let dir = scratch();
    let new = dir.join(NEW_NAME);
    make_db(&new, 0);
    std::os::unix::fs::symlink(NEW_NAME, dir.join(OLD_NAME)).expect("link");

    let writer = Connection::open(&new).expect("writer");
    writer
        .execute_batch("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;")
        .expect("wal");
    writer
        .execute("INSERT INTO projects (id, name, path) VALUES ('w1', 'w1', '/tmp/w1')", [])
        .expect("insert");
    assert!(
        std::fs::metadata(crate::private_home::sibling(&new, "-wal")).expect("wal").len() > 0,
        "the row must be in the WAL only"
    );

    let reader = Connection::open_with_flags(dir.join(OLD_NAME), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("reader via the link");
    let file: String = reader
        .query_row("SELECT file FROM pragma_database_list WHERE name = 'main'", [], |r| r.get(0))
        .expect("database_list");
    assert!(file.ends_with("/k2.db"), "SQLite must resolve the link: {file}");
    let seen: i64 = reader
        .query_row("SELECT count(*) FROM projects WHERE id = 'w1'", [], |r| r.get(0))
        .expect("reader count");
    assert_eq!(seen, 1, "the reader must see the WAL-only row");

    // A writer through the link (an old daemon) lands in the same WAL.
    let old_writer = Connection::open(dir.join(OLD_NAME)).expect("writer via the link");
    old_writer
        .execute("INSERT INTO projects (id, name, path) VALUES ('w2', 'w2', '/tmp/w2')", [])
        .expect("insert via link");
    let seen: i64 = writer
        .query_row("SELECT count(*) FROM projects WHERE id = 'w2'", [], |r| r.get(0))
        .expect("count");
    assert_eq!(seen, 1, "the k2.db writer sees the link writer's row");
    for s in ["-wal", "-shm"] {
        assert!(
            std::fs::symlink_metadata(crate::private_home::sibling(&dir.join(OLD_NAME), s)).is_err(),
            "k2so.db{s} must never be created"
        );
    }
}

// ── prd-k2-db-rename-v1 amendments ───────────────────────────────────

#[cfg(unix)]
fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap_or_else(|e| panic!("stat {}: {e}", p.display())).permissions().mode() & 0o777
}

/// DB-A1: the rename keeps the database's inode (a hard link, then the link
/// swapped in over the old name), and no tmp link is left behind.
#[cfg(unix)]
#[test]
fn rename_keeps_the_inode_and_swaps_the_link_in_atomically() {
    use std::os::unix::fs::MetadataExt;
    let dir = scratch();
    make_db(&dir.join(OLD_NAME), 2);
    let ino = std::fs::metadata(dir.join(OLD_NAME)).expect("stat").ino();
    let rep = switch_home_db(&dir, &opts());
    assert_eq!(rep.outcome, Outcome::Renamed, "{rep:?}");
    assert!(!rep.notes.iter().any(|n| n.contains("no hard links")), "unix uses the hard link: {rep:?}");
    assert_eq!(std::fs::metadata(dir.join(NEW_NAME)).expect("stat").ino(), ino, "same database inode");
    assert_eq!(std::fs::metadata(dir.join(NEW_NAME)).expect("stat").nlink(), 1, "no second name left behind");
    assert_link(&dir);
    assert!(std::fs::symlink_metadata(dir.join(LINK_TMP)).is_err(), "the tmp link is gone");
    assert_eq!(live_count(&dir.join(NEW_NAME)), 2);
}

/// DB-A1: a tmp link a crash left behind is replaced, not tripped over.
#[cfg(unix)]
#[test]
fn a_stale_tmp_link_from_a_crash_is_replaced() {
    let dir = scratch();
    make_db(&dir.join(OLD_NAME), 1);
    std::os::unix::fs::symlink("somewhere-else", dir.join(LINK_TMP)).expect("stale tmp link");
    let rep = switch_home_db(&dir, &opts());
    assert_eq!(rep.outcome, Outcome::Renamed, "{rep:?}");
    assert_link(&dir);
    assert!(std::fs::symlink_metadata(dir.join(LINK_TMP)).is_err());
}

/// A crash after the hard link leaves both names on one inode. A downgraded
/// daemon may then have written through the old name (its WAL sits under
/// `k2so.db-wal`): the next boot folds that WAL in, then swaps the link in.
/// Nothing is lost and nothing is treated as "two live files".
#[cfg(unix)]
#[test]
fn both_names_on_one_inode_are_finished_and_an_old_name_wal_is_folded_in() {
    let dir = scratch();
    let old = dir.join(OLD_NAME);
    make_db(&old, 2);
    std::fs::hard_link(&old, dir.join(NEW_NAME)).expect("hard link (crash after_rename)");
    {
        // A 0.46.5 daemon opens it as k2so.db and writes 3 more workspaces;
        // it is killed before any checkpoint.
        let c = Connection::open(&old).expect("open via the old name");
        c.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
            .expect("no checkpoint on close");
        c.execute_batch("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;").expect("wal");
        for i in 2..5 {
            c.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![format!("p{i}"), format!("ws-{i}"), format!("/tmp/ws-{i}")],
            )
            .expect("insert");
        }
    }
    assert!(len_of(&crate::private_home::sibling(&old, "-wal")) > 0, "rows must sit in k2so.db-wal");

    let rep = switch_home_db(&dir, &opts());
    assert_eq!(rep.outcome, Outcome::AlreadyDone, "{rep:?}");
    assert_link(&dir);
    assert_eq!(live_count(&dir.join(NEW_NAME)), 5, "the old-name WAL rows were folded in");
    assert_eq!(integrity(&dir.join(NEW_NAME)), "ok");
    for s in ["-wal", "-shm", "-journal"] {
        assert!(
            std::fs::symlink_metadata(crate::private_home::sibling(&old, s)).is_err(),
            "no k2so.db{s} left beside the link"
        );
    }
    assert!(dir.join(MARKER).exists());
}

/// DB-A2: a real `k2so.db` that is nobody's database (a reader minted it)
/// beside the real `k2.db` is quarantined and replaced by the link in the
/// same pass, not left for a later boot. A live one is never touched.
#[cfg(unix)]
#[test]
fn an_empty_k2so_db_beside_k2_db_is_healed_in_the_same_pass() {
    let dir = scratch();
    make_db(&dir.join(NEW_NAME), 3);
    std::fs::write(dir.join(OLD_NAME), b"").expect("a reader's empty file");
    let mut rep = SwitchReport::new(dir.join(NEW_NAME));
    ensure_link(&dir, &mut rep);
    assert_eq!(rep.quarantined.len(), 1, "{rep:?}");
    assert_link(&dir);

    let dir = scratch();
    make_db(&dir.join(NEW_NAME), 3);
    make_db(&dir.join(OLD_NAME), 1);
    let before = read(&dir.join(OLD_NAME));
    let mut rep = SwitchReport::new(dir.join(NEW_NAME));
    ensure_link(&dir, &mut rep);
    assert!(rep.quarantined.is_empty(), "{rep:?}");
    assert_eq!(read(&dir.join(OLD_NAME)), before);
    assert!(rep.notes.iter().any(|n| n.contains("left alone")), "{rep:?}");
}

/// DB-A3: stray `k2so.db-wal` / `-shm` / an empty `-journal` beside the
/// link go to quarantine.
#[cfg(unix)]
#[test]
fn stray_old_name_side_files_beside_the_link_are_quarantined() {
    let dir = scratch();
    make_db(&dir.join(NEW_NAME), 1);
    std::os::unix::fs::symlink(NEW_NAME, dir.join(OLD_NAME)).expect("link");
    let old = dir.join(OLD_NAME);
    std::fs::write(crate::private_home::sibling(&old, "-wal"), b"wal").expect("wal");
    std::fs::write(crate::private_home::sibling(&old, "-shm"), b"shm").expect("shm");
    std::fs::write(crate::private_home::sibling(&old, "-journal"), b"").expect("empty journal");
    let rep = switch_home_db(&dir, &opts());
    assert_eq!(rep.outcome, Outcome::AlreadyDone, "{rep:?}");
    assert_eq!(rep.quarantined.len(), 3, "{rep:?}");
    for s in ["-wal", "-shm", "-journal"] {
        assert!(std::fs::symlink_metadata(crate::private_home::sibling(&old, s)).is_err(), "k2so.db{s} moved");
    }
    assert_eq!(quarantined(&dir).len(), 3);
}

/// DB-A15: a non-empty `k2so.db-journal` beside the link means an outside
/// writer died mid-write under the old name: refuse, move nothing, say so.
/// `init_home_database` turns this into an error (the daemon won't open).
#[cfg(unix)]
#[test]
fn a_hot_old_name_journal_refuses_and_moves_nothing() {
    let dir = scratch();
    make_db(&dir.join(NEW_NAME), 2);
    std::os::unix::fs::symlink(NEW_NAME, dir.join(OLD_NAME)).expect("link");
    let j = crate::private_home::sibling(&dir.join(OLD_NAME), "-journal");
    std::fs::write(&j, b"unfinished pages").expect("hot journal");
    let before = read(&dir.join(NEW_NAME));
    let rep = switch_home_db(&dir, &opts());
    match &rep.outcome {
        Outcome::HotJournal { journal, bytes } => {
            assert_eq!(journal, &j);
            assert_eq!(*bytes, 16);
        }
        o => panic!("expected HotJournal, got {o:?}"),
    }
    assert_eq!(read(&j), b"unfinished pages", "never moved or changed");
    assert_eq!(read(&dir.join(NEW_NAME)), before);
    assert!(quarantined(&dir).is_empty());
    let note: serde_json::Value = serde_json::from_slice(&read(&dir.join(BLOCKED_NOTE))).expect("note");
    assert_eq!(note["kind"], "hot-journal");
    assert!(note["message"].as_str().expect("message").contains("don't delete or move"));
    assert!(!blocked_notice_due(&dir), "the two-live-files notice is not this one");
}

/// DB-A15 beats a hold or a defer: neither may open k2.db over an
/// unfinished write under the old name.
#[cfg(unix)]
#[test]
fn a_hold_or_defer_never_masks_a_hot_old_name_journal() {
    let dir = scratch();
    make_db(&dir.join(NEW_NAME), 1);
    std::os::unix::fs::symlink(NEW_NAME, dir.join(OLD_NAME)).expect("link");
    std::fs::write(crate::private_home::sibling(&dir.join(OLD_NAME), "-journal"), b"x").expect("hot journal");
    std::fs::write(dir.join(HOLD_FILE), b"").expect("hold");
    assert!(matches!(switch_home_db(&dir, &opts()).outcome, Outcome::HotJournal { .. }), "hold");
    let o = SwitchOpts { defer: Some("hand-off".into()), by: "test".into() };
    assert!(matches!(switch_home_db(&dir, &o).outcome, Outcome::HotJournal { .. }), "defer");
}

/// DB-A5: the owner's hold file defers the switch: nothing moves.
// Windows defers before these checks (see windows_defers_every_case_and_moves_nothing).
#[cfg(unix)]
#[test]
fn the_hold_file_defers_and_moves_nothing() {
    let dir = scratch();
    make_db(&dir.join(OLD_NAME), 1);
    make_db(&dir.join(NEW_NAME), 0);
    std::fs::write(dir.join(HOLD_FILE), b"").expect("hold");
    let rep = switch_home_db(&dir, &opts());
    match &rep.outcome {
        Outcome::Deferred { reason } => assert!(reason.contains("held by the owner"), "{reason}"),
        o => panic!("expected Deferred, got {o:?}"),
    }
    assert_eq!(rep.path, dir.join(OLD_NAME), "today's pick");
    assert!(dir.join(NEW_NAME).is_file() && dir.join(OLD_NAME).is_file());
    assert!(quarantined(&dir).is_empty());
    assert!(!dir.join(MARKER).exists());
    let s = status_json_for(&dir, &rep);
    assert_eq!(s["outcome"], "deferred");
    assert_eq!(s["file"], "k2so.db");
    assert!(s["reason"].as_str().expect("reason").contains("held"));
}

/// DB-A6: the owner keeps k2.db out of two live files: the next boot moves
/// k2so.db aside (bytes intact), links, records the choice and clears it.
#[cfg(unix)]
#[test]
fn keep_k2_db_choice_quarantines_a_live_k2so_db() {
    let dir = scratch();
    make_db(&dir.join(OLD_NAME), 3);
    make_db(&dir.join(NEW_NAME), 2);
    let old_bytes = read(&dir.join(OLD_NAME));
    assert!(matches!(switch_home_db(&dir, &opts()).outcome, Outcome::Blocked { .. }));
    let r = write_keep_choice(&dir, "k2.db", "owner").expect("choice");
    assert_eq!(r["moveAside"], "k2so.db");
    let rep = switch_home_db(&dir, &opts());
    assert_eq!(rep.outcome, Outcome::KeptK2Db, "{rep:?}");
    assert_eq!(rep.quarantined.len(), 1, "{rep:?}");
    assert_eq!(read(&rep.quarantined[0]), old_bytes, "moved, never changed");
    assert_link(&dir);
    assert_eq!(live_count(&dir.join(NEW_NAME)), 2);
    assert!(!dir.join(CHOICE_FILE).exists(), "the choice is used up");
    assert!(!dir.join(BLOCKED_NOTE).exists(), "the refusal is cleared");
    assert_eq!(marker(&dir)["choice"]["keep"], "k2.db");
}

/// DB-A6: keeping k2so.db moves the live k2.db aside, then renames.
#[cfg(unix)]
#[test]
fn keep_k2so_db_choice_quarantines_k2_db_then_renames() {
    let dir = scratch();
    make_db(&dir.join(OLD_NAME), 3);
    make_db(&dir.join(NEW_NAME), 4);
    write_keep_choice(&dir, "k2so.db", "owner").expect("choice");
    let rep = switch_home_db(&dir, &opts());
    assert_eq!(rep.outcome, Outcome::Renamed, "{rep:?}");
    assert_eq!(live_count(&dir.join(NEW_NAME)), 3, "k2.db is now the kept k2so.db");
    assert_eq!(rep.quarantined.len(), 1);
    assert_eq!(live_count(&rep.quarantined[0]), 4, "the old k2.db is kept aside, whole");
    assert_link(&dir);
    assert_eq!(marker(&dir)["choice"]["keep"], "k2so.db");
}

/// DB-A6: a choice pinned to other files (one was replaced since) is
/// ignored and removed; the switch refuses as before.
#[cfg(unix)]
#[test]
fn a_stale_keep_choice_is_ignored() {
    let dir = scratch();
    make_db(&dir.join(OLD_NAME), 3);
    make_db(&dir.join(NEW_NAME), 2);
    write_keep_choice(&dir, "k2.db", "owner").expect("choice");
    std::fs::rename(dir.join(NEW_NAME), dir.join("k2.db.moved-by-hand")).expect("move k2.db away");
    make_db(&dir.join(NEW_NAME), 2);
    let rep = switch_home_db(&dir, &opts());
    assert!(matches!(rep.outcome, Outcome::Blocked { .. }), "{rep:?}");
    assert!(rep.notes.iter().any(|n| n.contains("ignored")), "{rep:?}");
    assert!(!dir.join(CHOICE_FILE).exists());
    assert!(quarantined(&dir).is_empty());
}

/// DB-A6: nothing to choose, or a wrong name: refused, nothing written.
#[test]
fn keep_choice_refuses_without_two_files_or_with_a_wrong_name() {
    let dir = scratch();
    make_db(&dir.join(OLD_NAME), 1);
    let e = write_keep_choice(&dir, "k2.db", "owner").expect_err("one file only");
    assert!(e.contains("nothing to choose"), "{e}");
    make_db(&dir.join(NEW_NAME), 1);
    let e = write_keep_choice(&dir, "k2-other.db", "owner").expect_err("bad name");
    assert!(e.contains("--keep must be"), "{e}");
    assert!(!dir.join(CHOICE_FILE).exists());
}

/// DB-A7: quarantined files are 0600 and the quarantine dir is 0700.
#[cfg(unix)]
#[test]
fn quarantined_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch();
    make_db(&dir.join(OLD_NAME), 1);
    make_db(&dir.join(NEW_NAME), 0);
    std::fs::set_permissions(dir.join(NEW_NAME), std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let rep = switch_home_db(&dir, &opts());
    assert_eq!(rep.outcome, Outcome::Renamed, "{rep:?}");
    assert_eq!(mode(&rep.quarantined[0]), 0o600);
    assert_eq!(mode(&dir.join(QUARANTINE_DIR)), 0o700);
}

/// DB-A4: the boot-status `homeDb` object after a rename.
#[cfg(unix)]
#[test]
fn home_db_status_after_a_rename() {
    let dir = scratch();
    make_db(&dir.join(OLD_NAME), 1);
    let rep = switch_home_db(&dir, &opts());
    let s = status_json_for(&dir, &rep);
    assert_eq!(s["file"], "k2.db");
    assert_eq!(s["outcome"], "renamed");
    assert!(s["reason"].is_null());
    assert!(s["markerAt"].as_str().is_some_and(|a| a.starts_with("20")), "{s}");
    assert_eq!(s.as_object().expect("object").len(), 4, "{s}");
}

/// Windows keeps today's file in 0.46.6: every case (two live files, an
/// open connection, the owner's hold, a plain legacy file) defers with
/// that reason and moves nothing.
#[cfg(windows)]
#[test]
fn windows_defers_every_case_and_moves_nothing() {
    let both = scratch();
    make_db(&both.join(OLD_NAME), 3);
    make_db(&both.join(NEW_NAME), 2);
    let in_use = scratch();
    make_db(&in_use.join(OLD_NAME), 2);
    let other = Connection::open(in_use.join(OLD_NAME)).expect("other");
    other.execute_batch("PRAGMA journal_mode = WAL;").expect("wal");
    let held = scratch();
    make_db(&held.join(OLD_NAME), 1);
    make_db(&held.join(NEW_NAME), 0);
    std::fs::write(held.join(HOLD_FILE), b"").expect("hold");
    let legacy = scratch();
    make_db(&legacy.join(OLD_NAME), 1);

    for (case, dir) in [("both live", &both), ("in use", &in_use), ("held", &held), ("legacy only", &legacy)] {
        let had_new = dir.join(NEW_NAME).exists();
        let old_b = read(&dir.join(OLD_NAME));
        let new_b = had_new.then(|| read(&dir.join(NEW_NAME)));
        let rep = switch_home_db(dir, &opts());
        assert_eq!(
            rep.outcome,
            Outcome::Deferred { reason: "Windows keeps today's file in 0.46.6".into() },
            "{case}: {rep:?}"
        );
        assert_eq!(rep.path, dir.join(OLD_NAME), "{case}: today's pick");
        assert_eq!(read(&dir.join(OLD_NAME)), old_b, "{case}: k2so.db untouched");
        assert_eq!(dir.join(NEW_NAME).exists(), had_new, "{case}: k2.db neither made nor removed");
        if let Some(b) = new_b {
            assert_eq!(read(&dir.join(NEW_NAME)), b, "{case}: k2.db untouched");
        }
        assert!(quarantined(dir).is_empty(), "{case}: nothing quarantined");
        assert!(!dir.join(MARKER).exists(), "{case}: no marker");
        assert!(!dir.join(BLOCKED_NOTE).exists(), "{case}: no blocked note");
        let s = status_json_for(dir, &rep);
        assert_eq!(s["outcome"], "deferred", "{case}: {s}");
        assert_eq!(s["file"], "k2so.db", "{case}: {s}");
    }
    drop(other);
}
