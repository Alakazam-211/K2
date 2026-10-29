//! Must-change-password at next sign-in (core): step 7, the flag's set,
//! not-set and cleared rules at the store, the login outcome, and the
//! change token (single use, 15 minutes, nothing else takes it).

use super::*;
use crate::skin::{
    add_principal_for_app, check_and_record_login, consume_password_reset, create_app_session_token, invites,
    mint_password_reset, resolve_skin_token, set_principal_password, set_principal_password_staying,
    set_principal_password_tied, SkinLoginOutcome,
};

struct Fx {
    _home: crate::test_env::TempHome,
    _db: crate::db::TestDbGuard,
    app: String,
    pid: String,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_env::TempHome::new();
    let db = crate::db::scoped_for_test();
    let room = format!("room-{tag}");
    let app = {
        let h = crate::db::shared();
        let conn = h.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![room, format!("/tmp/mc-{room}")])
            .unwrap();
        crate::published_services::insert(&conn, &room, "jobs", "node s.js", "/tmp", 4310, "local", "running", "cmd", "").unwrap().id
    };
    let p = add_principal_for_app("dana", Some("dana@example.test"), &app, None, "test").unwrap();
    Fx { _home: home, _db: db, app, pid: p.id }
}

fn flag(pid: &str) -> bool {
    is_flagged(pid).expect("flag read")
}

fn live_change_tokens(pid: &str) -> i64 {
    with_conn(|c| {
        c.query_row("SELECT COUNT(*) FROM password_change_tokens WHERE principal_id = ?1 AND used_at IS NULL", params![pid], |r| r.get(0))
            .map_err(|e| e.to_string())
    })
    .unwrap()
}

fn session_rows(pid: &str) -> i64 {
    with_conn(|c| {
        c.query_row("SELECT COUNT(*) FROM tokens WHERE principal_id = ?1 AND session = 1", params![pid], |r| r.get(0))
            .map_err(|e| e.to_string())
    })
    .unwrap()
}

#[test]
fn step7_adds_the_flag_and_the_token_table_and_a_v6_file_upgrades() {
    let f = fx("s7");
    assert_eq!(crate::skin::SKIN_SCHEMA_VERSION, 7);
    assert!(!flag(&f.pid), "a new person starts with the flag off");
    // An existing guest whose password the owner set before the upgrade
    // (0.46.5 had no flag): never locked out by the upgrade.
    set_principal_password("dana", Some("set-before-upgrade-1")).unwrap();
    let before = create_app_session_token("dana", &f.app).expect("a session before the upgrade").raw;
    // Wind the file back to what 0.46.5 wrote: version 6, no column, no table.
    let path = crate::skin::store_path();
    *crate::skin::db().lock() = None;
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("DROP TABLE password_change_tokens; ALTER TABLE principals DROP COLUMN password_must_change; PRAGMA user_version = 6;")
            .unwrap();
    }
    let _ = std::fs::remove_file(path.with_file_name("skin.db.pre-7.bak"));
    assert!(crate::skin::principal_exists("dana").unwrap(), "the v6 file opens and keeps its rows");
    let v: i64 = with_conn(|c| c.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(|e| e.to_string())).unwrap();
    assert_eq!(v, 7);
    assert!(path.with_file_name("skin.db.pre-7.bak").exists(), "backed up before step 7");
    assert!(!flag(&f.pid), "an upgraded person has the flag off");
    assert_eq!(live_change_tokens(&f.pid), 0, "the table exists");
    assert!(resolve_skin_token(&before).is_some(), "a session from before the upgrade still works");
    match check_and_record_login("dana", "set-before-upgrade-1") {
        SkinLoginOutcome::Ok(p) => assert_eq!(p.id, f.pid),
        other => panic!("an existing guest signs in normally after the upgrade, got {other:?}"),
    }
    create_app_session_token("dana", &f.app).expect("and gets a session");
}

#[test]
fn the_flag_is_set_only_on_request_and_never_for_a_cleared_password() {
    let f = fx("set");
    set_principal_password_tied("dana", Some("temp-pass-1"), None, true).unwrap();
    assert!(flag(&f.pid), "someone else chose it: on");
    set_principal_password_tied("dana", Some("temp-pass-2"), None, false).unwrap();
    assert!(!flag(&f.pid), "false clears it");
    set_principal_password_tied("dana", Some("temp-pass-3"), None, true).unwrap();
    set_principal_password_tied("dana", None, None, true).unwrap();
    assert!(!flag(&f.pid), "a cleared password never sets it");
    set_principal_password_tied("dana", Some("temp-pass-4"), None, true).unwrap();
    set_principal_password("dana", Some("plain-set-5")).unwrap();
    assert!(!flag(&f.pid), "set_principal_password (no tie) passes false");
}

#[test]
fn every_password_the_person_sets_clears_the_flag() {
    let f = fx("clear");
    // password.change with stay.
    set_principal_password_tied("dana", Some("temp-pass-1"), None, true).unwrap();
    assert!(flag(&f.pid));
    set_principal_password_staying("dana", "my-own-pw-1", "no-such-token").unwrap();
    assert!(!flag(&f.pid), "password.change {{stay}} clears it");
    // An owner reset link (k2inv_).
    set_principal_password_tied("dana", Some("temp-pass-2"), None, true).unwrap();
    let m = invites::mint(&f.pid, &f.app, invites::PURPOSE_OWNER_RESET, 3600, None, false, None, "owner-token").unwrap();
    invites::activate(&m.id).unwrap().expect("started");
    assert_eq!(invites::consume(&m.token, "my-own-pw-2").unwrap(), invites::Consume::Ok);
    assert!(!flag(&f.pid), "a used set-password link clears it");
    // An invite link.
    set_principal_password_tied("dana", Some("temp-pass-3"), None, true).unwrap();
    let m = invites::mint(&f.pid, &f.app, invites::PURPOSE_INVITE, 3600, None, false, None, "owner-token").unwrap();
    invites::activate(&m.id).unwrap().expect("started");
    assert_eq!(invites::consume(&m.token, "my-own-pw-3").unwrap(), invites::Consume::Ok);
    assert!(!flag(&f.pid), "a used invite clears it");
    // The older reset store (forgot by email).
    set_principal_password_tied("dana", Some("temp-pass-4"), None, true).unwrap();
    let r = mint_password_reset("dana").unwrap().expect("password + email");
    assert_eq!(consume_password_reset(&r.token, "my-own-pw-4").unwrap(), crate::skin::SkinResetConsume::Ok);
    assert!(!flag(&f.pid), "a used reset token clears it");
}

#[test]
fn a_flagged_login_gets_must_change_never_ok_and_a_wrong_password_is_still_bad() {
    let f = fx("login");
    set_principal_password_tied("dana", Some("temp-pass-1"), None, true).unwrap();
    match check_and_record_login("dana", "temp-pass-1") {
        SkinLoginOutcome::MustChange(p) => assert_eq!(p.id, f.pid),
        other => panic!("expected MustChange, got {other:?}"),
    }
    match check_and_record_login("dana@example.test", "temp-pass-1") {
        SkinLoginOutcome::MustChange(p) => assert_eq!(p.id, f.pid, "by email too"),
        other => panic!("expected MustChange by email, got {other:?}"),
    }
    assert!(matches!(check_and_record_login("dana", "wrong-guess"), SkinLoginOutcome::BadCreds));
    assert_eq!(session_rows(&f.pid), 0, "no login minted a session");
}

#[test]
fn start_mints_one_change_token_and_no_session_and_refuses_a_login_that_would_be_refused() {
    let f = fx("start");
    set_principal_password_tied("dana", Some("temp-pass-1"), None, true).unwrap();
    let a = start("dana", &f.app).expect("dana holds the App's grant");
    assert!(a.token.starts_with(CHANGE_PREFIX) && a.token.len() == CHANGE_PREFIX.len() + 64, "{}", a.token);
    let now = crate::skin::now_secs();
    assert!(a.expires_at > now && a.expires_at <= now + CHANGE_TTL_SECS, "15 minutes");
    assert_eq!(session_rows(&f.pid), 0, "the dry run kept no session");
    let b = start("dana", &f.app).expect("again");
    assert_ne!(a.token, b.token);
    assert_eq!(live_change_tokens(&f.pid), 1, "a new token voids the older one");
    assert_eq!(consume(&a.token, "my-own-pw-1").unwrap(), ChangeConsume::BadToken, "the older token is gone");
    // No grant for this App (or no such App): the generic refusal.
    assert_eq!(start("dana", "svc-nobody").unwrap_err(), crate::skin::HOST_LOGIN_DENIED);
    assert_eq!(start("ghost", &f.app).unwrap_err(), crate::skin::HOST_LOGIN_DENIED);
}

#[test]
fn the_change_token_sets_a_new_password_once_and_the_person_signs_in_normally() {
    let f = fx("use");
    set_principal_password_tied("dana", Some("temp-pass-1"), None, true).unwrap();
    let s = start("dana", &f.app).unwrap();
    assert_eq!(consume(&s.token, "temp-pass-1").unwrap(), ChangeConsume::SamePassword, "not the temporary one");
    assert_eq!(consume(&s.token, "  temp-pass-1  ").unwrap(), ChangeConsume::SamePassword, "trimmed like every password");
    assert!(flag(&f.pid), "a refused change changes nothing");
    assert_eq!(consume(&s.token, "my-own-pw-1").unwrap(), ChangeConsume::Ok, "the token still works after SamePassword");
    assert!(!flag(&f.pid), "cleared");
    assert_eq!(consume(&s.token, "my-own-pw-2").unwrap(), ChangeConsume::BadToken, "single use");
    assert!(matches!(check_and_record_login("dana", "temp-pass-1"), SkinLoginOutcome::BadCreds), "the temporary one is gone");
    assert!(matches!(check_and_record_login("dana", "my-own-pw-1"), SkinLoginOutcome::Ok(_)), "a normal sign-in");
    create_app_session_token("dana", &f.app).expect("a session mints again");
}

#[test]
fn an_expired_or_voided_change_token_is_refused() {
    let f = fx("exp");
    set_principal_password_tied("dana", Some("temp-pass-1"), None, true).unwrap();
    let s = start("dana", &f.app).unwrap();
    with_conn(|c| {
        c.execute("UPDATE password_change_tokens SET expires_at = ?2 WHERE principal_id = ?1", params![f.pid, crate::skin::now_secs() - 1])
            .map_err(|e| e.to_string())
    })
    .unwrap();
    assert_eq!(consume(&s.token, "my-own-pw-1").unwrap(), ChangeConsume::BadToken, "expired");
    // Any password write voids unused tokens (the owner set another temporary one).
    let s = start("dana", &f.app).unwrap();
    set_principal_password_tied("dana", Some("temp-pass-2"), None, true).unwrap();
    assert_eq!(live_change_tokens(&f.pid), 0);
    assert_eq!(consume(&s.token, "my-own-pw-1").unwrap(), ChangeConsume::BadToken, "voided by the new temporary password");
    // A token minted while flagged, then the flag cleared some other way.
    let s = start("dana", &f.app).unwrap();
    with_conn(|c| c.execute("UPDATE principals SET password_must_change = 0 WHERE id = ?1", params![f.pid]).map_err(|e| e.to_string())).unwrap();
    assert_eq!(consume(&s.token, "my-own-pw-1").unwrap(), ChangeConsume::BadToken, "flag off: nothing to change");
}

#[test]
fn the_change_token_works_nowhere_else() {
    let f = fx("else");
    set_principal_password_tied("dana", Some("temp-pass-1"), None, true).unwrap();
    let s = start("dana", &f.app).unwrap();
    assert!(resolve_skin_token(&s.token).is_none(), "not a pass");
    assert_eq!(invites::consume(&s.token, "my-own-pw-1").unwrap(), invites::Consume::BadToken, "not a set-password link");
    assert_eq!(consume_password_reset(&s.token, "my-own-pw-1").unwrap(), crate::skin::SkinResetConsume::BadToken, "not a reset token");
    assert!(matches!(check_and_record_login("dana", &s.token), SkinLoginOutcome::BadCreds), "not a password");
    // And a set-password link is not a change token.
    let m = invites::mint(&f.pid, &f.app, invites::PURPOSE_OWNER_RESET, 3600, None, false, None, "owner-token").unwrap();
    invites::activate(&m.id).unwrap().expect("started");
    assert_eq!(consume(&m.token, "my-own-pw-1").unwrap(), ChangeConsume::BadToken);
    assert!(flag(&f.pid), "none of those changed anything");
    assert_eq!(consume(&s.token, "my-own-pw-1").unwrap(), ChangeConsume::Ok, "it still works where it belongs");
}
