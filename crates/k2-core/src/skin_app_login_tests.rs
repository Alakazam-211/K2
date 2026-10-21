//! Device sign-in: rotation, reuse detection, revoke (own, owner, with the
//! person's sessions, with the pass), the stay setting and its switch.

use super::*;
use crate::skin::{
    add_principal_for_app, create_role_for_app, list_grants, resolve_skin_token, revoke_token, set_grant_enabled,
    set_principal_password, RoomPolicy,
};

struct Fx {
    _home: crate::test_env::TempHome,
    _db: crate::db::TestDbGuard,
    app: String,
    other_app: String,
    room: String,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_env::TempHome::new();
    let db = crate::db::scoped_for_test();
    let room = format!("room-{tag}");
    let other_room = format!("other-{tag}");
    let (app, other_app) = {
        let h = crate::db::shared();
        let conn = h.lock();
        for r in [&room, &other_room] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![r, format!("/tmp/al-{r}")])
                .unwrap();
        }
        let a = crate::published_services::insert(&conn, &room, "jobs", "node s.js", "/tmp", 4200, "local", "running", "cmd", "").unwrap();
        let o = crate::published_services::insert(&conn, &other_room, "crm", "node s.js", "/tmp", 4201, "local", "running", "cmd", "")
            .unwrap();
        (a.id, o.id)
    };
    let mut policy = RoomPolicy::new();
    policy.insert(room.clone(), vec!["thread:read".into(), "thread:post".into()]);
    create_role_for_app(&format!("r-{tag}"), &policy, Some(&app)).unwrap();
    add_principal_for_app("dana", None, &app, Some(&format!("r-{tag}")), "test").unwrap();
    set_principal_password("dana", Some("s3cret-horse")).unwrap();
    Fx { _home: home, _db: db, app, other_app, room }
}

fn phone() -> Device {
    Device { id: "iphone-1".into(), name: "Dana's iPhone".into() }
}

fn login(f: &Fx) -> DeviceSession {
    login_device("dana", &f.app, &phone()).expect("device login").expect("stay signed in is on by default")
}

fn ok(r: Refresh) -> DeviceSession {
    match r {
        Refresh::Ok(d) => *d,
        other => panic!("expected a refresh, got {other:?}"),
    }
}

#[test]
fn defaults_are_thirty_days_unlisted_and_bounded() {
    let f = fx("dflt");
    let s = settings(&f.app).unwrap();
    assert_eq!((s.listed, s.stay_signed_in_days), (false, DEFAULT_STAY_DAYS));
    assert_eq!(DEFAULT_STAY_DAYS, 30);
    assert_eq!(MAX_STAY_DAYS, 90);
    assert!(set_settings(&f.app, None, Some(91), "owner").is_err(), "at most 90");
    assert!(set_settings(&f.app, None, Some(-1), "owner").is_err());
    let s = set_settings(&f.app, Some(true), Some(90), "owner").unwrap();
    assert_eq!((s.listed, s.stay_signed_in_days), (true, 90));
    let s = set_settings(&f.app, None, Some(0), "owner").unwrap();
    assert_eq!((s.listed, s.stay_signed_in_days), (true, 0), "None keeps listed");
    assert!(!settings(&f.other_app).unwrap().listed, "per App");
}

#[test]
fn device_shapes() {
    assert_eq!(parse_device(None).unwrap(), None);
    assert_eq!(parse_device(Some(&serde_json::Value::Null)).unwrap(), None);
    let d = parse_device(Some(&serde_json::json!({"id": "a.b:c-1_2", "name": "  Phone\u{7}  "}))).unwrap().unwrap();
    assert_eq!((d.id.as_str(), d.name.as_str()), ("a.b:c-1_2", "Phone"));
    let d = parse_device(Some(&serde_json::json!({"id": "x"}))).unwrap().unwrap();
    assert_eq!(d.name, "Device");
    for bad in [serde_json::json!("id"), serde_json::json!({}), serde_json::json!({"id": "has space"}), serde_json::json!({"id": "x".repeat(129)})] {
        assert!(parse_device(Some(&bad)).is_err(), "{bad}");
    }
}

#[test]
fn refresh_rotates_and_a_spent_token_is_refused() {
    let f = fx("rot");
    let first = login(&f);
    assert!(first.refresh.starts_with(REFRESH_PREFIX) && is_refresh_token(&first.refresh));
    assert!(!crate::skin::is_skin_token(&first.refresh), "a refresh token is never a pass");
    let pass1 = resolve_skin_token(&first.session.raw).expect("live pass");
    assert_eq!(pass1.app, f.app);
    assert!(first.pass_expires_at <= first.refresh_expires_at);
    let now = super::now_secs();
    assert!((first.refresh_expires_at - (now + 30 * 86_400)).abs() < 60, "30 days");

    let second = ok(refresh(&first.refresh, Some(&f.app)).unwrap());
    assert_ne!(second.refresh, first.refresh, "rotates");
    assert_ne!(second.session.raw, first.session.raw);
    assert_eq!(second.device_login_id, first.device_login_id, "same device sign-in");
    assert!(resolve_skin_token(&first.session.raw).is_none(), "the old pass ended");
    assert!(resolve_skin_token(&second.session.raw).is_some());
    let devices = devices_for(pass1.principal_id.as_deref().unwrap(), &f.app).unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].session_token_id.as_deref(), Some(resolve_skin_token(&second.session.raw).unwrap().id.as_str()));
    assert_eq!(devices[0].name, "Dana's iPhone");

    // Reuse of a spent token: the whole device sign-in ends (R8).
    assert!(matches!(refresh(&first.refresh, None).unwrap(), Refresh::Reused));
    assert!(resolve_skin_token(&second.session.raw).is_none(), "reuse ends the live pass too");
    assert!(matches!(refresh(&second.refresh, None).unwrap(), Refresh::Invalid), "and its current refresh token");
    assert!(devices_for(pass1.principal_id.as_deref().unwrap(), &f.app).unwrap().is_empty());
}

#[test]
fn bound_to_the_app_and_garbage_is_invalid() {
    let f = fx("bind");
    let d = login(&f);
    assert!(matches!(refresh(&d.refresh, Some(&f.other_app)).unwrap(), Refresh::Invalid), "another App");
    // Not spent by the refusal: the right App still works.
    let _ = ok(refresh(&d.refresh, Some(&f.app)).unwrap());
    for junk in ["", "k2skr_short", "k2skn_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", &format!("{REFRESH_PREFIX}{}", "A".repeat(43))] {
        assert!(matches!(refresh(junk, None).unwrap(), Refresh::Invalid), "{junk}");
    }
}

#[test]
fn revoke_own_owner_logout_and_with_the_persons_sessions() {
    let f = fx("rev");
    let a = login(&f);
    let pid = resolve_skin_token(&a.session.raw).unwrap().principal_id.unwrap();
    // The same device signing in again replaces its sign-in.
    let a2 = login(&f);
    assert!(matches!(refresh(&a.refresh, None).unwrap(), Refresh::Invalid), "replaced");
    let other = login_device("dana", &f.app, &Device { id: "ipad".into(), name: "iPad".into() }).unwrap().unwrap();
    assert_eq!(devices_for(&pid, &f.app).unwrap().len(), 2);

    // Own revoke: only the person's own id on this App.
    assert!(!revoke_own("someone-else", &f.app, &other.device_login_id).unwrap());
    assert!(!revoke_own(&pid, &f.other_app, &other.device_login_id).unwrap());
    assert!(revoke_own(&pid, &f.app, &other.device_login_id).unwrap());
    assert!(resolve_skin_token(&other.session.raw).is_none(), "its pass ends");
    assert!(matches!(refresh(&other.refresh, None).unwrap(), Refresh::Invalid));

    // Logout (the pass revoked) ends the device sign-in.
    let pass = resolve_skin_token(&a2.session.raw).unwrap();
    assert!(revoke_token(&pass.id).unwrap());
    assert!(matches!(refresh(&a2.refresh, None).unwrap(), Refresh::Invalid), "logout ends refresh");

    // Owner revoke.
    let b = login(&f);
    assert!(revoke_by_owner(&b.device_login_id).unwrap());
    assert!(!revoke_by_owner(&b.device_login_id).unwrap(), "already ended");
    assert!(matches!(refresh(&b.refresh, None).unwrap(), Refresh::Invalid));

    // A password change ends every session, device sign-ins included.
    let c = login(&f);
    set_principal_password("dana", Some("another-horse")).unwrap();
    assert!(matches!(refresh(&c.refresh, None).unwrap(), Refresh::Invalid));
    assert!(devices_for(&pid, &f.app).unwrap().is_empty());
}

#[test]
fn a_disabled_grant_ends_the_chain_and_stay_zero_is_todays_login() {
    let f = fx("grant");
    let d = login(&f);
    let g = list_grants().unwrap().into_iter().find(|g| g.target_id == f.app).unwrap();
    set_grant_enabled(&g.id, false).unwrap();
    assert!(matches!(refresh(&d.refresh, None).unwrap(), Refresh::Invalid));
    set_grant_enabled(&g.id, true).unwrap();
    // Stay signed in off: a device login is exactly today's (the caller
    // mints a plain 7-day pass), and live chains end at their next refresh.
    let d = login(&f);
    set_settings(&f.app, None, Some(0), "owner").unwrap();
    assert!(login_device("dana", &f.app, &phone()).unwrap().is_none());
    assert!(matches!(refresh(&d.refresh, None).unwrap(), Refresh::Invalid));
    // A login without a grant is the same refusal as a bad password.
    set_settings(&f.app, None, Some(30), "owner").unwrap();
    assert_eq!(login_device("dana", &f.other_app, &phone()).unwrap_err(), crate::skin::HOST_LOGIN_DENIED);
    let _ = f.room;
}

#[test]
fn a_short_stay_caps_the_pass() {
    let f = fx("short");
    set_settings(&f.app, None, Some(1), "owner").unwrap();
    let d = login(&f);
    let now = super::now_secs();
    assert!((d.refresh_expires_at - (now + 86_400)).abs() < 60);
    assert_eq!(d.pass_expires_at, d.refresh_expires_at, "a pass never outlives its device sign-in");
}
