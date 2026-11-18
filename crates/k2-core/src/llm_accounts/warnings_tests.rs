//! The warnings before a Claude sign-in ends: one per step per owner,
//! kept across restarts, starting over after a new sign-in, none for a
//! sign-in whose file doesn't say when it ends.

use rusqlite::Connection;

use super::warnings::{self, Warning};
use super::*;

const T0: i64 = 1_800_000_000;
const DAY: i64 = 86_400;

/// A database file of this test's own, removed on drop.
struct DbFile(std::path::PathBuf);

impl DbFile {
    fn new(tag: &str) -> DbFile {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        DbFile(std::env::temp_dir().join(format!("k2-warnings-{tag}-{}-{n}.db", std::process::id())))
    }
}

impl Drop for DbFile {
    fn drop(&mut self) {
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", self.0.display()));
        }
    }
}

fn db_at(path: &std::path::Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    crate::db::run_migrations(&conn).unwrap();
    conn
}

fn token(conn: &Connection, label: &str, ends_at: Option<i64>) -> Entry {
    let e = create(conn, Tool::Claude, label, None).unwrap();
    update_meta(conn, &e.id, &MetaUpdate { state: Some(state::SIGNED_IN.into()), refreshed_at: Some(T0 - 30 * DAY), sign_in_ends_at: Some(ends_at), ..Default::default() }).unwrap();
    get(conn, &e.id).unwrap().unwrap()
}

fn steps(ws: &[Warning]) -> Vec<(String, String)> {
    ws.iter().map(|w| (w.label.clone(), w.step.clone())).collect()
}

#[test]
fn the_step_due_is_the_most_urgent_one_reached() {
    let end = T0 + 10 * DAY;
    assert_eq!(warnings::step_for(end, T0), None);
    assert_eq!(warnings::step_for(end, end - 7 * DAY), Some("7d"));
    assert_eq!(warnings::step_for(end, end - 2 * DAY), Some("7d"));
    assert_eq!(warnings::step_for(end, end - DAY), Some("1d"));
    assert_eq!(warnings::step_for(end, end - 3 * 3600), Some("1d"));
    assert_eq!(warnings::step_for(end, end - 2 * 3600), Some("2h"));
    assert_eq!(warnings::step_for(end, end - 1), Some("2h"));
    assert_eq!(warnings::step_for(end, end), Some(warnings::ENDED));
}

#[test]
fn each_step_shows_once_per_owner_and_a_dismissal_survives_a_restart() {
    let file = DbFile::new("restart");
    let path = file.0.clone();
    let end = T0 + 10 * DAY;
    let (id, epoch) = {
        let conn = db_at(&path);
        let jeff = token(&conn, "JeffsToken", Some(end));
        let _none = token(&conn, "no-end-known", None);
        assert!(warnings::pending_for(&conn, "owner-token", T0).unwrap().is_empty(), "10 days out: nothing yet");
        let at7 = end - 7 * DAY + 60;
        let ws = warnings::pending_for(&conn, "owner-token", at7).unwrap();
        assert_eq!(steps(&ws), vec![("JeffsToken".into(), "7d".into())], "no warning for a token without an end time");
        assert_eq!(ws[0].ends_at, Some(end));
        warnings::dismiss(&conn, "owner-token", &jeff.id, "7d", ws[0].epoch, at7).unwrap();
        assert!(warnings::pending_for(&conn, "owner-token", at7 + 3600).unwrap().is_empty(), "7d shown once");
        // Another owner still gets it.
        assert_eq!(steps(&warnings::pending_for(&conn, "login:sam", at7).unwrap()), vec![("JeffsToken".into(), "7d".into())]);
        (jeff.id, ws[0].epoch)
    };
    // Restart: the same file, a new connection.
    let conn = db_at(&path);
    assert!(warnings::pending_for(&conn, "owner-token", end - 2 * DAY).unwrap().is_empty(), "7d stays put away after a restart");
    let ws = warnings::pending_for(&conn, "owner-token", end - DAY + 60).unwrap();
    assert_eq!(steps(&ws), vec![("JeffsToken".into(), "1d".into())]);
    warnings::dismiss(&conn, "owner-token", &id, "1d", epoch, end - DAY + 60).unwrap();
    assert!(warnings::pending_for(&conn, "owner-token", end - 3 * 3600).unwrap().is_empty());
    let ws = warnings::pending_for(&conn, "owner-token", end - 3600).unwrap();
    assert_eq!(steps(&ws), vec![("JeffsToken".into(), "2h".into())]);
    warnings::dismiss(&conn, "owner-token", &id, "2h", epoch, end - 3600).unwrap();
    assert!(warnings::pending_for(&conn, "owner-token", end - 60).unwrap().is_empty());
    // Ended: once more, then never again.
    let ws = warnings::pending_for(&conn, "owner-token", end + 60).unwrap();
    assert_eq!(steps(&ws), vec![("JeffsToken".into(), "ended".into())]);
    warnings::dismiss(&conn, "owner-token", &id, "ended", epoch, end + 60).unwrap();
    assert!(warnings::pending_for(&conn, "owner-token", end + DAY).unwrap().is_empty());
    assert!(warnings::dismiss(&conn, "owner-token", &id, "3d", epoch, end).is_err(), "unknown steps are refused");
}

#[test]
fn a_new_sign_in_starts_the_warnings_over() {
    let file = DbFile::new("reset");
    let conn = db_at(&file.0);
    let end = T0 + 10 * DAY;
    let jeff = token(&conn, "JeffsToken", Some(end));
    let at = end - DAY + 60;
    let w = warnings::pending_for(&conn, "owner-token", at).unwrap().remove(0);
    warnings::dismiss(&conn, "owner-token", &jeff.id, &w.step, w.epoch, at).unwrap();
    assert!(warnings::pending_for(&conn, "owner-token", at).unwrap().is_empty());
    // Signed in again: a new end, so the next warning is its own.
    let later = end + 30 * DAY;
    update_meta(&conn, &jeff.id, &MetaUpdate { sign_in_ends_at: Some(Some(later)), ..Default::default() }).unwrap();
    assert!(warnings::pending_for(&conn, "owner-token", at).unwrap().is_empty(), "the new sign-in ends in a month");
    assert_eq!(steps(&warnings::pending_for(&conn, "owner-token", later - DAY + 60).unwrap()), vec![("JeffsToken".into(), "1d".into())]);
    // Marked "sign in again" with no end time on file: ended, keyed by
    // its last sign-in; a later sign-in and end warn again.
    let plain = token(&conn, "plain", None);
    // Marked "sign in again" for another reason: no pop-up.
    update_meta(&conn, &plain.id, &MetaUpdate { state: Some(state::NEEDS_LOGIN.into()), detail: Some(Some("The provider refused this token's saved sign-in twice.".into())), ..Default::default() }).unwrap();
    assert!(warnings::pending_for(&conn, "owner-token", T0).unwrap().iter().all(|w| w.account_id != plain.id));
    update_meta(&conn, &plain.id, &MetaUpdate { detail: Some(Some(super::store::ClaudeEnded::Ended.words())), ..Default::default() }).unwrap();
    let w = warnings::pending_for(&conn, "owner-token", T0).unwrap().into_iter().find(|w| w.account_id == plain.id).unwrap();
    assert_eq!((w.step.as_str(), w.ends_at), ("ended", None));
    warnings::dismiss(&conn, "owner-token", &plain.id, "ended", w.epoch, T0).unwrap();
    assert!(warnings::pending_for(&conn, "owner-token", T0).unwrap().iter().all(|w| w.account_id != plain.id));
    update_meta(&conn, &plain.id, &MetaUpdate { refreshed_at: Some(T0 + DAY), ..Default::default() }).unwrap();
    assert!(warnings::pending_for(&conn, "owner-token", T0 + 2 * DAY).unwrap().iter().any(|w| w.account_id == plain.id && w.step == "ended"));
}
