//! The sign-in door's skin.db half: per-App settings, token targets, the
//! dry run and the hash-checked mint.

use super::*;
use crate::skin::{add_principal_for_app, check_and_record_login_hashed, must_change, set_principal_password, set_principal_password_tied, SkinLoginOutcome};

struct Fx {
    _home: crate::test_env::TempHome,
    _db: crate::db::TestDbGuard,
    app: String,
    other: String,
    pid: String,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_env::TempHome::new();
    let db = crate::db::scoped_for_test();
    let room = format!("room-{tag}");
    let (app, other) = {
        let h = crate::db::shared();
        let conn = h.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![room, format!("/tmp/ad-{room}")]).unwrap();
        let a = crate::published_services::insert(&conn, &room, "jobs", "node s.js", "/tmp", 4311, "local", "running", "cmd", "").unwrap().id;
        let b = crate::published_services::insert(&conn, &room, "shop", "node s.js", "/tmp", 4312, "local", "running", "cmd", "").unwrap().id;
        (a, b)
    };
    let p = add_principal_for_app("dana", Some("dana@example.test"), &app, None, "test").unwrap();
    set_principal_password("dana", Some("first-pass-1")).unwrap();
    Fx { _home: home, _db: db, app, other, pid: p.id }
}

#[test]
fn settings_default_on_with_no_origins_and_round_trip() {
    let f = fx("settings");
    let s = settings(&f.app).unwrap();
    assert!(!s.off && s.origins.is_empty(), "{s:?}");
    assert!(set_off(&f.app, true, "owner").unwrap().off);
    assert!(!set_off(&f.app, false, "owner").unwrap().off);
    let s = set_origins(&f.app, &["TAURI://localhost".into(), "tauri://localhost".into(), "http://tauri.localhost".into()], "owner").unwrap();
    assert_eq!(s.origins, vec!["tauri://localhost".to_string(), "http://tauri.localhost".to_string()], "normalised, no repeats");
    assert!(!s.off, "setting origins keeps the switch");
    for bad in ["https://evil.example", "tauri://localhost/x", "localhost", "null"] {
        assert!(set_origins(&f.app, &[bad.to_string()], "owner").is_err(), "{bad}");
    }
    assert_eq!(allowed_origins(&f.app).unwrap(), s.origins);
    assert!(allowed_origins(&f.other).unwrap().is_empty(), "per App");
}

#[test]
fn the_box_switch_starts_off() {
    let _f = fx("box");
    assert!(!box_on().unwrap(), "dark until the owner turns it on (V32)");
    assert!(set_box_on(true, "owner").unwrap());
    assert!(box_on().unwrap());
    assert!(!set_box_on(false, "owner").unwrap());
}

#[test]
fn a_universal_link_path_never_takes_the_door() {
    use crate::skin::app_pairing::valid_link_path;
    for p in ["/.k2/auth", "/.k2/auth/sign-in", "/.K2/AUTH/x", "//.k2//auth/x"] {
        assert!(!valid_link_path(p), "{p}");
    }
    for p in ["/.k2/other", "/messages", "/.k2x/auth"] {
        assert!(valid_link_path(p), "{p}");
    }
}

#[test]
fn token_targets_name_the_person_and_the_app_without_using_the_token() {
    let f = fx("targets");
    let inv = invites::mint(&f.pid, &f.app, invites::PURPOSE_INVITE, 3600, None, false, None, "owner-token").unwrap();
    invites::activate(&inv.id).unwrap().expect("started");
    let t = token_target(&inv.token).unwrap().expect("live invite");
    assert_eq!((t.principal_id.as_str(), t.username.as_str(), t.app.as_str(), t.owner_reset), (f.pid.as_str(), "dana", f.app.as_str(), false));
    // Reading it twice uses nothing.
    assert!(token_target(&inv.token).unwrap().is_some());

    let own = invites::mint(&f.pid, &f.app, invites::PURPOSE_OWNER_RESET, 3600, None, false, None, "owner-token").unwrap();
    invites::activate(&own.id).unwrap().expect("started");
    assert!(token_target(&own.token).unwrap().expect("live").owner_reset, "the owner's reset is marked");

    // A must-change token.
    set_principal_password_tied("dana", Some("temp-pass-11"), None, true).unwrap();
    let started = must_change::start("dana", &f.app).unwrap();
    let t = token_target(&started.token).unwrap().expect("live change token");
    assert_eq!((t.app.as_str(), t.owner_reset), (f.app.as_str(), false));

    for junk in ["", "k2inv_nope", "k2chg_nope", "k2skn_abc", "k2pwr_abc"] {
        assert!(token_target(junk).unwrap().is_none(), "{junk}");
    }
}

#[test]
fn the_dry_run_keeps_nothing_and_refuses_an_app_without_a_grant() {
    let f = fx("dry");
    let before: i64 = with_conn(|c| c.query_row("SELECT COUNT(*) FROM tokens", [], |r| r.get(0)).map_err(|e| e.to_string())).unwrap();
    dry_run("dana", &f.app).expect("dana has a grant on jobs");
    let after: i64 = with_conn(|c| c.query_row("SELECT COUNT(*) FROM tokens", [], |r| r.get(0)).map_err(|e| e.to_string())).unwrap();
    assert_eq!(before, after, "nothing kept");
    assert_eq!(dry_run("dana", &f.other).unwrap_err(), HOST_LOGIN_DENIED, "no grant on shop");
}

#[test]
fn the_mint_needs_the_same_hash_and_must_change_off() {
    let f = fx("mint");
    let (out, hash) = check_and_record_login_hashed("dana", "first-pass-1");
    assert!(matches!(out, SkinLoginOutcome::Ok(_)), "{out:?}");
    let hash = hash.expect("the checked hash");
    assert_eq!(password_hash_of(&f.pid).unwrap().as_deref(), Some(hash.as_str()));
    match mint(&f.pid, &f.app, &hash, None).expect("mint") {
        Minted::Plain(s) => {
            assert!(s.raw.starts_with("k2skn_"));
            assert_eq!(s.app, f.app);
        }
        Minted::Device(_) => panic!("no device asked"),
    }
    // Another App: no grant there.
    assert_eq!(mint(&f.pid, &f.other, &hash, None).unwrap_err(), HOST_LOGIN_DENIED);
    // The password changes: a code made with the old hash mints nothing.
    set_principal_password("dana", Some("second-pass-2")).unwrap();
    assert_eq!(mint(&f.pid, &f.app, &hash, None).unwrap_err(), HOST_LOGIN_DENIED, "V12: the code dies with its password");
    // Must-change on: nothing, even with the current hash.
    set_principal_password_tied("dana", Some("temp-pass-22"), None, true).unwrap();
    let now = password_hash_of(&f.pid).unwrap().expect("hash");
    assert_eq!(mint(&f.pid, &f.app, &now, None).unwrap_err(), HOST_LOGIN_DENIED);
    // A wrong password gives no hash.
    let (out, hash) = check_and_record_login_hashed("dana", "wrong-pass-0");
    assert!(matches!(out, SkinLoginOutcome::BadCreds) && hash.is_none());
}

#[test]
fn a_device_mint_needs_stay_signed_in_on() {
    let f = fx("device");
    let hash = password_hash_of(&f.pid).unwrap().expect("hash");
    let dev = Device { id: "phone-1".into(), name: "Dana's phone".into() };
    app_login::set_settings(&f.app, None, Some(0), "owner").unwrap();
    assert!(matches!(mint(&f.pid, &f.app, &hash, Some(&dev)).unwrap(), Minted::Plain(_)), "stay off: a plain pass");
    app_login::set_settings(&f.app, None, Some(30), "owner").unwrap();
    match mint(&f.pid, &f.app, &hash, Some(&dev)).unwrap() {
        Minted::Device(d) => {
            assert!(d.refresh.starts_with(app_login::REFRESH_PREFIX));
            assert_eq!(d.device.name, "Dana's phone");
        }
        Minted::Plain(_) => panic!("stay on: a device sign-in"),
    }
}
