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

/// 0.46.8: an App sets its own cap. Nobody is signed out when it changes;
/// at the next new sign-in the least recently used devices end until it
/// fits. Other Apps and the other settings are untouched.
#[test]
fn an_apps_device_cap_ends_the_least_recently_used_at_the_next_sign_in() {
    let f = fx("maxd");
    assert_eq!(settings(&f.app).unwrap().max_devices, MAX_LIVE_DEVICES, "the default is the most");
    for bad in [0, -1, MAX_LIVE_DEVICES + 1] {
        let e = set_max_devices(&f.app, Some(bad), "owner").unwrap_err();
        assert!(e.contains("1 to 20"), "says the range: {e}");
    }
    assert_eq!(settings(&f.app).unwrap().max_devices, MAX_LIVE_DEVICES, "a refused cap changes nothing");

    let sign_in = |id: &str| login_device("dana", &f.app, &Device { id: id.into(), name: id.into() }).unwrap().unwrap();
    // Each older than the next, so which one is least recently used is plain.
    let age = |id: &str, secs: i64| {
        with_conn(|c| {
            c.execute("UPDATE device_logins SET last_used_at = last_used_at - ?1 WHERE device_id = ?2", params![secs, id])
                .map_err(|e| e.to_string())
        })
        .unwrap();
    };
    let a = sign_in("a");
    age("a", 300);
    let b = sign_in("b");
    age("b", 200);
    let c = sign_in("c");
    age("c", 100);
    let pid = resolve_skin_token(&a.session.raw).unwrap().principal_id.unwrap();
    let live = || {
        let mut ids: Vec<String> = devices_for(&pid, &f.app).unwrap().into_iter().map(|d| d.device_id).collect();
        ids.sort();
        ids
    };
    assert_eq!(live(), ["a", "b", "c"]);

    assert_eq!(set_max_devices(&f.app, Some(2), "owner").unwrap().max_devices, 2);
    assert_eq!(live(), ["a", "b", "c"], "nobody is signed out when the cap changes");
    assert!(resolve_skin_token(&a.session.raw).is_some());

    // A device already signed in replaces itself: one more sign-in on c ends
    // only as many others as the cap needs (a, the oldest).
    let c2 = sign_in("c");
    assert_eq!(live(), ["b", "c"], "down to the cap, oldest first");
    assert!(resolve_skin_token(&a.session.raw).is_none(), "a ended");
    assert!(resolve_skin_token(&b.session.raw).is_some(), "b kept");
    assert!(resolve_skin_token(&c.session.raw).is_none(), "c's earlier sign-in was replaced");
    assert!(resolve_skin_token(&c2.session.raw).is_some());
    age("c", 100);

    // A new device ends the least recently used one.
    let d = sign_in("d");
    assert_eq!(live(), ["c", "d"]);
    assert!(resolve_skin_token(&b.session.raw).is_none(), "b ended");
    assert!(resolve_skin_token(&d.session.raw).is_some());
    let ended: i64 = with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM device_logins WHERE app_id = ?1 AND revoked_reason = ?2", params![f.app, REASON_LIMIT], |r| r.get(0))
            .map_err(|e| e.to_string())
    })
    .unwrap();
    assert_eq!(ended, 2, "a and b, both with reason device_limit");

    // A cap of 1: one device at a time, the newest.
    set_max_devices(&f.app, Some(1), "owner").unwrap();
    sign_in("e");
    assert_eq!(live(), ["e"]);

    // Per App, and back to the default.
    assert_eq!(settings(&f.other_app).unwrap().max_devices, MAX_LIVE_DEVICES, "per App");
    assert_eq!(set_max_devices(&f.app, None, "owner").unwrap().max_devices, MAX_LIVE_DEVICES);
    // Each setter keeps the other's fields.
    let s = set_settings(&f.app, Some(true), Some(9), "owner").unwrap();
    assert_eq!((s.listed, s.stay_signed_in_days, s.max_devices), (true, 9, MAX_LIVE_DEVICES));
    set_max_devices(&f.app, Some(3), "owner").unwrap();
    let s = set_settings(&f.app, None, Some(10), "owner").unwrap();
    assert_eq!((s.listed, s.stay_signed_in_days, s.max_devices), (true, 10, 3));
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
