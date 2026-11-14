//! Which workspaces each boot sweep has already covered.
//!
//! The daemon's boot runs several per-workspace sweeps (the 0.37 unification,
//! the `.k2so` → `.k2` cutover, legacy slug folders, Clone-to session paths,
//! the legacy heartbeat and skill migrations) before it answers any route.
//! Each one is idempotent, but each walked every workspace on every boot, so
//! a server with hundreds of workspaces paid for all of them on every
//! restart.
//!
//! Now each sweep keeps one install-wide mark in the main database
//! (`scheduler_meta`, key `boot_sweep:<name>`): the sweep's version and the
//! workspace paths it covered at that version. A boot sweeps only the paths
//! not in the mark (a workspace added or moved since), then records them.
//! A new sweep version (a fix to the sweep, or for a sweep whose work
//! follows K2's own version, a K2 update) covers every workspace again.
//!
//! `K2_BOOT_SWEEP_MARKS=0` turns the marks off: every boot sweeps every
//! workspace, as before.

use rusqlite::Connection;
use std::collections::BTreeSet;

use crate::db::schema::SchedulerMeta;

const KEY_PREFIX: &str = "boot_sweep:";

#[derive(serde::Serialize, serde::Deserialize, Default, Debug, PartialEq, Eq)]
struct Mark {
    version: String,
    paths: BTreeSet<String>,
}

/// Marks are on unless `K2_BOOT_SWEEP_MARKS` is `0`, `false` or `off`.
pub fn marks_enabled() -> bool {
    !matches!(
        std::env::var("K2_BOOT_SWEEP_MARKS").map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Ok("0") | Ok("false") | Ok("off")
    )
}

fn read_mark(conn: &Connection, sweep: &str) -> Mark {
    SchedulerMeta::get(conn, &format!("{KEY_PREFIX}{sweep}"))
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

/// The paths of `paths` that `sweep` has not covered at `version` (all of
/// them when the marks are off, or the version changed).
pub fn pending(conn: &Connection, sweep: &str, version: &str, paths: &[String]) -> Vec<String> {
    if !marks_enabled() {
        return paths.to_vec();
    }
    let mark = read_mark(conn, sweep);
    if mark.version != version {
        return paths.to_vec();
    }
    paths.iter().filter(|p| !mark.paths.contains(*p)).cloned().collect()
}

/// Record that `sweep` covered `swept` at `version`. `registered` is every
/// workspace path there is now: a path no longer registered leaves the mark,
/// so a workspace added again later is swept again.
pub fn record(conn: &Connection, sweep: &str, version: &str, swept: &[String], registered: &[String]) -> Result<(), String> {
    if !marks_enabled() {
        return Ok(());
    }
    let old = read_mark(conn, sweep);
    let registered: BTreeSet<&String> = registered.iter().collect();
    let mut paths: BTreeSet<String> = if old.version == version {
        old.paths.into_iter().filter(|p| registered.contains(p)).collect()
    } else {
        BTreeSet::new()
    };
    paths.extend(swept.iter().filter(|p| registered.contains(p)).cloned());
    let mark = Mark { version: version.to_string(), paths };
    let value = serde_json::to_string(&mark).map_err(|e| e.to_string())?;
    SchedulerMeta::set(conn, &format!("{KEY_PREFIX}{sweep}"), &value).map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("/srv/ws{i:03}")).collect()
    }

    #[test]
    fn a_second_boot_has_nothing_to_sweep() {
        let _db = crate::db::scoped_for_test();
        let db = crate::db::shared();
        let conn = db.lock();
        let all = paths(300);
        assert_eq!(pending(&conn, "unify", "1", &all).len(), 300, "first boot: all of them");
        record(&conn, "unify", "1", &all, &all).expect("record");
        assert!(pending(&conn, "unify", "1", &all).is_empty(), "second boot: none");
    }

    #[test]
    fn a_workspace_added_between_boots_is_swept_once() {
        let _db = crate::db::scoped_for_test();
        let db = crate::db::shared();
        let conn = db.lock();
        let mut all = paths(3);
        record(&conn, "unify", "1", &all, &all).expect("record");
        all.push("/srv/new".into());
        assert_eq!(pending(&conn, "unify", "1", &all), vec!["/srv/new".to_string()]);
        record(&conn, "unify", "1", &["/srv/new".to_string()], &all).expect("record new");
        assert!(pending(&conn, "unify", "1", &all).is_empty());
    }

    #[test]
    fn a_new_sweep_version_covers_every_workspace_again() {
        let _db = crate::db::scoped_for_test();
        let db = crate::db::shared();
        let conn = db.lock();
        let all = paths(5);
        record(&conn, "legacy", "1+0.46.13", &all, &all).expect("record");
        assert_eq!(pending(&conn, "legacy", "1+0.46.14", &all).len(), 5);
        record(&conn, "legacy", "1+0.46.14", &all[..2], &all).expect("record part");
        assert_eq!(pending(&conn, "legacy", "1+0.46.14", &all), all[2..].to_vec(), "the old version's paths don't count");
    }

    #[test]
    fn a_removed_workspace_leaves_the_mark() {
        let _db = crate::db::scoped_for_test();
        let db = crate::db::shared();
        let conn = db.lock();
        let all = paths(3);
        record(&conn, "dotdir", "1", &all, &all).expect("record");
        let fewer = all[..2].to_vec();
        record(&conn, "dotdir", "1", &[], &fewer).expect("record after removal");
        // The same path registered again is a new workspace there.
        assert_eq!(pending(&conn, "dotdir", "1", &all), vec![all[2].clone()]);
    }

    #[test]
    fn sweeps_keep_their_own_marks() {
        let _db = crate::db::scoped_for_test();
        let db = crate::db::shared();
        let conn = db.lock();
        let all = paths(2);
        record(&conn, "a", "1", &all, &all).expect("record a");
        assert_eq!(pending(&conn, "b", "1", &all).len(), 2);
    }
}
