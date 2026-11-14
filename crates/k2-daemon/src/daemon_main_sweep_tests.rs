//! Boot sweep marks (speed pass P7): a second boot sweeps no workspace, a
//! workspace added between boots is swept once, a new sweep version sweeps
//! them all again. The sweeps run on this thread against a scoped DB and a
//! temp `$HOME`, over empty workspace folders.

use super::*;
use k2_core::test_env::TempHome;

/// Every per-workspace boot sweep, in boot order. Returns the passes they
/// made.
fn boot_sweeps() -> usize {
    let before = boot_sweep_passes();
    run_workspace_unification_sweep();
    run_migrate_k2so_dot_dirs();
    run_workspace_legacy_migrations_sweep();
    run_migrate_legacy_slug_dirs();
    run_repair_cloned_session_paths();
    boot_sweep_passes() - before
}

const SWEEPS: usize = 5;

fn add_workspaces(home: &TempHome, from: usize, n: usize) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    for i in from..from + n {
        let path = home.path().join(format!("ws{i:03}"));
        std::fs::create_dir_all(&path).expect("workspace dir");
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), format!("ws{i:03}"), path.to_string_lossy()],
        )
        .expect("project");
    }
}

#[test]
fn a_second_boot_with_300_workspaces_sweeps_none_of_them() {
    let home = TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    add_workspaces(&home, 0, 300);
    assert_eq!(boot_sweeps(), 300 * SWEEPS, "the first boot sweeps every workspace once per sweep");
    assert_eq!(boot_sweeps(), 0, "the second boot sweeps none");
}

#[test]
fn a_workspace_added_between_boots_is_swept_exactly_once() {
    let home = TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    add_workspaces(&home, 0, 4);
    assert_eq!(boot_sweeps(), 4 * SWEEPS);
    add_workspaces(&home, 4, 1);
    assert_eq!(boot_sweeps(), SWEEPS, "only the new workspace, once per sweep");
    assert_eq!(boot_sweeps(), 0);
}

#[test]
fn a_new_sweep_version_sweeps_every_workspace_again() {
    let home = TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    add_workspaces(&home, 0, 6);
    assert_eq!(boot_sweeps(), 6 * SWEEPS);
    // The legacy sweep's version follows K2's: an older K2 recorded it.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let key = format!("boot_sweep:{SWEEP_WORKSPACE_LEGACY}");
        let mark = k2_core::db::schema::SchedulerMeta::get(&conn, &key).expect("legacy mark");
        let older = mark.replace(&format!("1+{}", env!("CARGO_PKG_VERSION")), "1+0.0.1");
        assert_ne!(older, mark, "the mark names this K2's version: {mark}");
        k2_core::db::schema::SchedulerMeta::set(&conn, &key, &older).expect("set older");
    }
    assert_eq!(boot_sweeps(), 6, "the legacy sweep covers every workspace again; the others none");
}

#[test]
fn a_workspace_whose_folder_is_missing_is_swept_when_it_is_back() {
    let home = TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    add_workspaces(&home, 0, 2);
    let gone = home.path().join("ws001");
    std::fs::remove_dir_all(&gone).expect("remove ws001");
    assert_eq!(boot_sweeps(), 1 * 3 + 2 * 2, "unification, dot dir and legacy skip a missing folder");
    std::fs::create_dir_all(&gone).expect("ws001 back");
    assert_eq!(boot_sweeps(), 3, "the three that skipped it sweep it now");
}

#[test]
fn with_the_marks_off_every_boot_sweeps_every_workspace() {
    let home = TempHome::new();
    let _off = k2_core::test_env::EnvVar::set("K2_BOOT_SWEEP_MARKS", "0");
    let _db = k2_core::db::scoped_for_test();
    add_workspaces(&home, 0, 3);
    assert_eq!(boot_sweeps(), 3 * SWEEPS);
    assert_eq!(boot_sweeps(), 3 * SWEEPS, "as before the marks");
}
