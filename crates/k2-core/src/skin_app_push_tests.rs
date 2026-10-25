//! Push devices: validation, upsert and rotation, one token one guest,
//! limits, the no-oracle reads, cleanup with every way a sign-in ends, the
//! sweep, purge, and the per-App settings.

use super::*;
use crate::skin::app_login::{self, Device};
use crate::skin::{
    add_principal_for_app, create_app_session_token, create_role_for_app, list_grants, remove_principal, revoke_sessions_for_app,
    revoke_token, set_grant_enabled, set_principal_password, with_skin_conn, RoomPolicy,
};
use serde_json::json;

struct Fx {
    _home: crate::test_env::TempHome,
    _db: crate::db::TestDbGuard,
    app: String,
    other_app: String,
    dana: String,
    eli: String,
}

fn principal_id(username: &str) -> String {
    with_skin_conn(|c| {
        c.query_row("SELECT id FROM principals WHERE username = ?1", params![username], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())
    })
    .expect("principal")
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
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![r, format!("/tmp/ap-{r}")])
                .expect("project");
        }
        let a = crate::published_services::insert(&conn, &room, "jobs", "node s.js", "/tmp", 4300, "local", "running", "cmd", "")
            .expect("svc");
        let o = crate::published_services::insert(&conn, &other_room, "crm", "node s.js", "/tmp", 4301, "local", "running", "cmd", "")
            .expect("svc");
        (a.id, o.id)
    };
    let mut policy = RoomPolicy::new();
    policy.insert(room.clone(), vec!["thread:read".into(), "thread:post".into()]);
    create_role_for_app(&format!("r-{tag}"), &policy, Some(&app)).expect("role");
    for u in ["dana", "eli"] {
        add_principal_for_app(u, None, &app, Some(&format!("r-{tag}")), "test").expect("guest");
        set_principal_password(u, Some("s3cret-horse")).expect("password");
    }
    let mut other = RoomPolicy::new();
    other.insert(other_room.clone(), vec!["thread:read".into()]);
    create_role_for_app(&format!("o-{tag}"), &other, Some(&other_app)).expect("role");
    Fx { _home: home, _db: db, app, other_app, dana: principal_id("dana"), eli: principal_id("eli") }
}

const APNS_A: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
const APNS_B: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";
const FCM_A: &str = "dGVzdC10b2tlbi1mb3ItZmNt:APA91bH-fake_token_value_0123456789";

fn reg(device: &str, platform: &str, token: &str) -> Registration {
    parse_registration(&json!({"deviceId": device, "platform": platform, "pushToken": token})).expect("valid registration")
}

fn session(username: &str, app: &str) -> String {
    create_app_session_token(username, app).expect("session").meta.id
}

fn count(app: &str) -> usize {
    devices_for_app(app, None).expect("devices").len()
}

#[test]
fn registration_body_rules_name_the_field_and_never_echo_the_token() {
    let r = reg("iphone-1", "apns", &APNS_A.to_ascii_uppercase());
    assert_eq!(r.push_token, APNS_A, "APNs hex is stored lower case");
    assert_eq!((r.environment.as_str(), r.channel.as_str()), ("production", "app"));
    assert_eq!((r.previews, r.muted_topics.clone()), (None, None), "absent fields keep the stored choice");
    let r = parse_registration(&json!({"deviceId": "p", "platform": "fcm", "pushToken": FCM_A, "previews": null, "mutedTopics": ["jobs", "a.b", "jobs"]}))
        .expect("fcm");
    assert_eq!(r.previews, Some(None));
    assert_eq!(r.muted_topics, Some(vec!["a.b".to_string(), "jobs".to_string()]));

    let secret_token = "zz-not-hex-but-long-enough-0123456789";
    let cases = [
        (json!({"platform": "apns", "pushToken": APNS_A}), "deviceId"),
        (json!({"deviceId": "bad id", "platform": "apns", "pushToken": APNS_A}), "deviceId"),
        (json!({"deviceId": "d", "platform": "web", "pushToken": APNS_A}), "platform"),
        (json!({"deviceId": "d", "platform": "apns", "token": APNS_A}), "pushToken"),
        (json!({"deviceId": "d", "platform": "apns"}), "pushToken"),
        (json!({"deviceId": "d", "platform": "apns", "pushToken": secret_token}), "hex"),
        (json!({"deviceId": "d", "platform": "apns", "pushToken": "abc"}), "16 to 4096"),
        (json!({"deviceId": "d", "platform": "apns", "pushToken": "a".repeat(4098)}), "16 to 4096"),
        (json!({"deviceId": "d", "platform": "fcm", "pushToken": "has space in it 0123456789"}), "FCM"),
        (json!({"deviceId": "d", "platform": "fcm", "pushToken": FCM_A, "environment": "sandbox"}), "production for fcm"),
        (json!({"deviceId": "d", "platform": "apns", "pushToken": APNS_A, "environment": "dev"}), "environment"),
        (json!({"deviceId": "d", "platform": "apns", "pushToken": APNS_A, "channel": "web"}), "channel"),
        (json!({"deviceId": "d", "platform": "apns", "pushToken": APNS_A, "previews": "yes"}), "previews"),
        (json!({"deviceId": "d", "platform": "apns", "pushToken": APNS_A, "mutedTopics": ["Jobs"]}), "topic"),
        (json!({"deviceId": "d", "platform": "apns", "pushToken": APNS_A, "mutedTopics": vec!["t"; 33]}), "at most 32"),
    ];
    for (body, want) in cases {
        let e = parse_registration(&body).expect_err(&format!("refused: {body}"));
        assert!(e.contains(want), "{e:?} should name {want:?} for {body}");
        assert!(!e.contains(secret_token) && !e.contains(APNS_A), "never echoes a token: {e}");
    }
    assert!(format!("{:?}", reg("d", "apns", APNS_A)).contains("<redacted>"));
    assert!(!format!("{:?}", reg("d", "apns", APNS_A)).contains(APNS_A));
}

#[test]
fn register_upserts_keeps_created_at_and_never_serializes_the_token() {
    let f = fx("up");
    let pass = session("dana", &f.app);
    let a = register(&f.app, &f.dana, &pass, &reg("iphone-1", "apns", APNS_A)).expect("register");
    assert!(a.id.starts_with(ID_PREFIX) && a.id.len() == ID_PREFIX.len() + 22, "{}", a.id);
    assert_eq!((a.app.as_str(), a.principal_id.as_str(), a.username.as_str()), (f.app.as_str(), f.dana.as_str(), "dana"));
    assert_eq!(a.session_token_id.as_deref(), Some(pass.as_str()));
    assert_eq!(a.device_login_id, None, "a plain login pass has no device sign-in");
    with_skin_conn(|c| {
        c.execute("UPDATE app_push_devices SET created_at = 100, last_seen_at = 100 WHERE id = ?1", params![a.id]).map_err(|e| e.to_string())
    })
    .expect("age it");
    let b = register(&f.app, &f.dana, &pass, &reg("iphone-1", "apns", APNS_B)).expect("rotate token");
    assert_eq!(b.id, a.id, "same (App, guest, device) is one row");
    assert_eq!(b.created_at, 100, "created_at kept");
    assert!(b.last_seen_at > 100, "last seen bumped");
    assert_eq!(b.push_token, APNS_B);
    assert_eq!(count(&f.app), 1);
    let v = serde_json::to_value(&b).expect("json");
    let s = v.to_string();
    assert!(!s.contains(APNS_B) && !s.contains("pushToken") && !s.contains("sessionTokenId"), "{s}");
    assert!(!format!("{b:?}").contains(APNS_B));
    // Prefs survive a re-register that leaves them out, and change when sent.
    set_prefs(&f.app, &f.dana, "iphone-1", Some(Some(false)), Some(&["jobs".to_string()])).expect("prefs").expect("row");
    let c = register(&f.app, &f.dana, &pass, &reg("iphone-1", "apns", APNS_B)).expect("again");
    assert_eq!((c.previews, c.muted_topics.clone()), (Some(false), vec!["jobs".to_string()]));
    let r = parse_registration(&json!({"deviceId": "iphone-1", "platform": "apns", "pushToken": APNS_B, "previews": null, "mutedTopics": []}))
        .expect("reg");
    let d = register(&f.app, &f.dana, &pass, &r).expect("reset");
    assert_eq!((d.previews, d.muted_topics.len()), (None, 0));
}

#[test]
fn one_token_belongs_to_one_guest_per_app() {
    let f = fx("move");
    let pd = session("dana", &f.app);
    let pe = session("eli", &f.app);
    register(&f.app, &f.dana, &pd, &reg("phone", "apns", APNS_A)).expect("dana");
    // Eli can't take a token dana still holds on a live sign-in (review
    // B-L4): no silent takeover.
    assert_eq!(register(&f.app, &f.eli, &pe, &reg("phone", "apns", APNS_A)).expect_err("held"), RegisterError::TokenInUse);
    assert_eq!(devices_for_app(&f.app, Some(&f.dana)).expect("list").len(), 1, "dana keeps it");
    // Dana's own row moves when her same token comes under a new device id.
    let moved = register(&f.app, &f.dana, &pd, &reg("phone-2", "apns", APNS_A)).expect("dana again");
    assert_eq!(devices_for_app(&f.app, None).expect("list").len(), 1);
    assert_eq!(moved.device_id, "phone-2");
    // The phone signs out of dana (her sign-in ends): eli may take it.
    assert!(revoke_token(&pd).expect("logout"));
    let e = register(&f.app, &f.eli, &pe, &reg("phone", "apns", APNS_A)).expect("eli");
    let all = devices_for_app(&f.app, None).expect("list");
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0].id, e.id);
    assert_eq!(all[0].principal_id, f.eli);
    // The same token on another App is another App's row.
    crate::skin::create_grant(&crate::skin::SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: f.dana.clone(),
        kind: "app".into(),
        target_id: f.other_app.clone(),
        role: Some("o-move".into()),
        scope: None,
        created_by: Some("test".into()),
    })
    .expect("dana on the other App");
    let po = session("dana", &f.other_app);
    register(&f.other_app, &f.dana, &po, &reg("phone", "apns", APNS_A)).expect("other app");
    assert_eq!(count(&f.app), 1);
    assert_eq!(count(&f.other_app), 1);
}

#[test]
fn a_stale_holder_is_pruned_before_a_register_and_targets_skip_expired_passes() {
    let f = fx("stale");
    let pd = session("dana", &f.app);
    let pe = session("eli", &f.app);
    let d = register(&f.app, &f.dana, &pd, &reg("p1", "apns", APNS_A)).expect("dana");
    assert_eq!(targets(&f.app, &[f.dana.clone()]).expect("targets").len(), 1);
    assert!(still_deliverable(&f.app, &d.id, APNS_A).expect("check"));
    assert!(!still_deliverable(&f.app, &d.id, APNS_B).expect("check"), "another token is not this row");
    assert!(!still_deliverable(&f.other_app, &d.id, APNS_A).expect("check"), "another App never");
    // Dana's pass expires with no revoke: targets skip her row at once
    // (review B-L2), and so does the delivery check (B-L1).
    with_skin_conn(|c| c.execute("UPDATE tokens SET expires_at = 1 WHERE id = ?1", params![pd]).map_err(|e| e.to_string())).expect("expire");
    assert!(targets(&f.app, &[f.dana.clone()]).expect("targets").is_empty());
    assert!(!still_deliverable(&f.app, &d.id, APNS_A).expect("check"));
    // Eli's register prunes the stale holder instead of being refused.
    register(&f.app, &f.eli, &pe, &reg("p9", "apns", APNS_A)).expect("eli takes a token whose holder's sign-in ended");
    assert_eq!(devices_for_app(&f.app, Some(&f.dana)).expect("list").len(), 0);
    // A disabled grant fails the delivery check.
    let e = devices_for_app(&f.app, Some(&f.eli)).expect("list").remove(0);
    let g = list_grants().expect("grants").into_iter().find(|g| g.subject_id == f.eli && g.target_id == f.app).expect("grant");
    with_skin_conn(|c| c.execute("UPDATE grants SET enabled = 0 WHERE id = ?1", params![g.id]).map_err(|e| e.to_string())).expect("disable raw");
    assert!(!still_deliverable(&f.app, &e.id, APNS_A).expect("check"), "a disabled grant");
    assert!(targets(&f.app, &[f.eli.clone()]).expect("targets").is_empty());
}

#[test]
fn twenty_devices_per_guest_and_the_app_ceiling() {
    let f = fx("lim");
    let pass = session("dana", &f.app);
    for i in 0..21 {
        let tok = format!("{:064x}", i + 1);
        register(&f.app, &f.dana, &pass, &reg(&format!("d{i}"), "apns", &tok)).expect("register");
        with_skin_conn(|c| {
            c.execute(
                "UPDATE app_push_devices SET last_seen_at = ?1 WHERE device_id = ?2",
                params![1_000 + i as i64, format!("d{i}")],
            )
            .map_err(|e| e.to_string())
        })
        .expect("order");
    }
    let mine = devices_for_app(&f.app, Some(&f.dana)).expect("list");
    assert_eq!(mine.len(), MAX_DEVICES_PER_GUEST);
    assert!(!mine.iter().any(|d| d.device_id == "d0"), "the least recently seen went");
    assert!(mine.iter().any(|d| d.device_id == "d20"), "the new one stays");

    set_max_devices(&f.app, Some(21), "owner").expect("ceiling");
    let pe = session("eli", &f.app);
    register(&f.app, &f.eli, &pe, &reg("e0", "apns", &format!("{:064x}", 900))).expect("21st row");
    let e = register(&f.app, &f.eli, &pe, &reg("e1", "apns", &format!("{:064x}", 901))).expect_err("over the App ceiling");
    assert_eq!(e, RegisterError::AppFull { limit: 21 });
    // A re-register of an existing row is not a new row.
    register(&f.app, &f.eli, &pe, &reg("e0", "apns", &format!("{:064x}", 902))).expect("update ok at the ceiling");
    assert_eq!(count(&f.app), 21);
    assert!(set_max_devices(&f.app, Some(0), "owner").is_err());
    assert_eq!(config(&f.app).expect("cfg").max_devices, Some(21));
}

#[test]
fn reads_and_removes_are_the_guests_own_with_one_answer_for_a_miss() {
    let f = fx("own");
    let pd = session("dana", &f.app);
    let pe = session("eli", &f.app);
    let d = register(&f.app, &f.dana, &pd, &reg("p1", "apns", APNS_A)).expect("dana");
    register(&f.app, &f.eli, &pe, &reg("p2", "apns", APNS_B)).expect("eli");
    assert!(get_own(&f.app, &f.eli, "p1").expect("read").is_none(), "eli can't read dana's device");
    assert!(get_own(&f.other_app, &f.dana, "p1").expect("read").is_none(), "another App can't");
    assert!(set_prefs(&f.app, &f.eli, "p1", Some(Some(false)), None).expect("prefs").is_none());
    assert!(!unregister(&f.app, &f.eli, "p1").expect("miss"), "someone else's row: removed false");
    assert!(!unregister(&f.other_app, &f.dana, "p1").expect("miss"));
    assert!(!unregister(&f.app, &f.dana, "nope").expect("miss"));
    assert_eq!(get_own(&f.app, &f.dana, "p1").expect("read").map(|r| r.id), Some(d.id.clone()));
    assert!(!remove_by_owner(&f.other_app, &d.id).expect("other app id"));
    assert!(unregister(&f.app, &f.dana, "p1").expect("own"));
    assert!(get_own(&f.app, &f.dana, "p1").expect("read").is_none());
    let counts = device_counts(&f.app).expect("counts");
    assert_eq!(counts, vec![(f.eli.clone(), 1, vec!["apns".to_string()])]);
    let e = devices_for_app(&f.app, Some(&f.eli)).expect("list").remove(0);
    assert!(!delete_dead(&f.app, &e.id, APNS_A).expect("wrong token"), "a re-registered token keeps its row");
    assert!(delete_dead(&f.app, &e.id, APNS_B).expect("dead"));
    assert_eq!(count(&f.app), 0);
}

#[test]
fn a_device_ends_with_its_pass_logout_password_grant_and_app() {
    let f = fx("end");
    // Logout (the pass revoked).
    let p = session("dana", &f.app);
    register(&f.app, &f.dana, &p, &reg("p1", "apns", APNS_A)).expect("register");
    assert!(revoke_token(&p).expect("logout"));
    assert_eq!(count(&f.app), 0, "logout removes it");

    // Password change ends every session of the person.
    let p = session("dana", &f.app);
    register(&f.app, &f.dana, &p, &reg("p1", "apns", APNS_A)).expect("register");
    set_principal_password("dana", Some("another-horse-1")).expect("password change");
    assert_eq!(count(&f.app), 0, "password change removes it");

    // The App grant disabled.
    let p = session("dana", &f.app);
    register(&f.app, &f.dana, &p, &reg("p1", "apns", APNS_A)).expect("register");
    let g = list_grants().expect("grants").into_iter().find(|g| g.subject_id == f.dana && g.target_id == f.app).expect("grant");
    set_grant_enabled(&g.id, false).expect("disable");
    assert_eq!(count(&f.app), 0, "grant disabled removes it");
    set_grant_enabled(&g.id, true).expect("enable");

    // The App removed.
    let p = session("eli", &f.app);
    register(&f.app, &f.eli, &p, &reg("p2", "apns", APNS_B)).expect("register");
    revoke_sessions_for_app(&f.app).expect("publish rm");
    assert_eq!(count(&f.app), 0, "App removed removes it");

    // The guest removed: the foreign key takes the rows.
    let p = session("eli", &f.app);
    register(&f.app, &f.eli, &p, &reg("p2", "apns", APNS_B)).expect("register");
    assert!(remove_principal("eli").expect("remove"));
    assert_eq!(count(&f.app), 0, "guest removed removes it");
}

#[test]
fn a_device_sign_in_keeps_its_push_row_through_refreshes_and_takes_it_when_it_ends() {
    let f = fx("dev");
    let phone = Device { id: "iphone-1".into(), name: "Dana's iPhone".into() };
    let d = app_login::login_device("dana", &f.app, &phone).expect("login").expect("stay signed in");
    let row = register(&f.app, &f.dana, &d.session.meta.id, &reg("iphone-1", "apns", APNS_A)).expect("register");
    assert_eq!(row.device_login_id.as_deref(), Some(d.device_login_id.as_str()), "tied to the device sign-in");
    let mut refresh = d.refresh.clone();
    for _ in 0..3 {
        let r = match app_login::refresh(&refresh, Some(&f.app)).expect("refresh") {
            app_login::Refresh::Ok(n) => *n,
            other => panic!("expected a refresh, got {other:?}"),
        };
        refresh = r.refresh;
    }
    assert_eq!(count(&f.app), 1, "rotating refreshes end the old passes but not the push row");
    // The sweep agrees: the chain is live.
    assert_eq!(sweep().expect("sweep"), 0);
    // The guest signs the device out.
    assert!(app_login::revoke_own(&f.dana, &f.app, &d.device_login_id).expect("revoke"));
    assert_eq!(count(&f.app), 0, "device sign-out removes it");

    // Reuse detection ends the chain, and the row with it.
    let d = app_login::login_device("dana", &f.app, &phone).expect("login").expect("stay");
    register(&f.app, &f.dana, &d.session.meta.id, &reg("iphone-1", "apns", APNS_A)).expect("register");
    let first = d.refresh.clone();
    match app_login::refresh(&first, Some(&f.app)).expect("refresh") {
        app_login::Refresh::Ok(_) => {}
        other => panic!("expected a refresh, got {other:?}"),
    }
    match app_login::refresh(&first, Some(&f.app)).expect("reuse") {
        app_login::Refresh::Reused => {}
        other => panic!("expected reuse, got {other:?}"),
    }
    assert_eq!(count(&f.app), 0, "reuse removes it");

    // The owner ends the sign-in.
    let d = app_login::login_device("dana", &f.app, &phone).expect("login").expect("stay");
    register(&f.app, &f.dana, &d.session.meta.id, &reg("iphone-1", "apns", APNS_A)).expect("register");
    assert!(app_login::revoke_by_owner(&d.device_login_id).expect("owner revoke"));
    assert_eq!(count(&f.app), 0, "owner revoke removes it");
}

#[test]
fn the_hourly_sweep_takes_rows_whose_pass_or_sign_in_expired() {
    let f = fx("sw");
    let p = session("dana", &f.app);
    register(&f.app, &f.dana, &p, &reg("p1", "apns", APNS_A)).expect("register");
    let phone = Device { id: "iphone-2".into(), name: "Phone".into() };
    let d = app_login::login_device("eli", &f.app, &phone).expect("login").expect("stay");
    register(&f.app, &f.eli, &d.session.meta.id, &reg("iphone-2", "apns", APNS_B)).expect("register");
    assert_eq!(sweep().expect("sweep"), 0, "both live");
    with_skin_conn(|c| c.execute("UPDATE tokens SET expires_at = 1 WHERE id = ?1", params![p]).map_err(|e| e.to_string())).expect("expire pass");
    assert_eq!(sweep().expect("sweep"), 1, "the plain pass expired");
    with_skin_conn(|c| {
        c.execute("UPDATE device_logins SET expires_at = 1 WHERE id = ?1", params![d.device_login_id]).map_err(|e| e.to_string())
    })
    .expect("expire sign-in");
    assert_eq!(sweep().expect("sweep"), 1, "the device sign-in expired");
    assert_eq!(count(&f.app), 0);
}

#[test]
fn purge_takes_both_tables_and_settings_default_safely() {
    let f = fx("cfg");
    let c = config(&f.app).expect("cfg");
    assert!(c.enabled && !c.owner_on && !c.agent_replies && c.previews, "{c:?}");
    assert_eq!((c.max_devices, c.apns.clone(), c.fcm.clone(), c.cred_error.clone()), (None, None, None, None));
    let c = set_switches(&f.app, Switches { owner_on: Some(true), ..Default::default() }, "owner").expect("on");
    assert!(c.enabled && c.owner_on);
    let c = set_switches(&f.app, Switches { enabled: Some(false), agent_replies: Some(true), ..Default::default() }, "owner").expect("off");
    assert!(!c.enabled && c.owner_on && c.agent_replies && c.previews, "None keeps the other fields");
    assert!(set_switches(HOST_APP_ID, Switches::default(), "owner").is_err(), "never host");
    let meta = ApnsMeta {
        key_id: "ABC123DEFG".into(),
        team_id: "TEAM123456".into(),
        bundle_id: "com.example.app".into(),
        key_fingerprint: "0011223344556677".into(),
        set_at: 42,
        proven_at: None,
    };
    set_cred_error(&f.app, "apns:InvalidProviderToken\n<script>").expect("err");
    assert_eq!(config(&f.app).expect("cfg").cred_error.as_deref(), Some("apns:InvalidProviderTokenscript"), "codes only");
    let c = set_apns_meta(&f.app, Some(&meta), "owner").expect("meta");
    assert_eq!(c.apns, Some(meta));
    assert_eq!(c.cred_error, None, "new keys clear the last error");
    assert_eq!(config(&f.other_app).expect("cfg").apns, None, "per App");
    let fm = FcmMeta {
        project_id: "proj".into(),
        client_email: "sa@proj.iam.gserviceaccount.com".into(),
        key_fingerprint: "aa".into(),
        set_at: 7,
        proven_at: None,
    };
    assert_eq!(set_fcm_meta(&f.app, Some(&fm), "owner").expect("fcm").fcm, Some(fm.clone()));
    // The first accepted send proves a key; new keys make it unproven again.
    assert!(!mark_proven(&f.app, PLATFORM_FCM, "old-key-fpr").expect("stale"), "a send with a replaced key proves nothing");
    assert!(mark_proven(&f.app, PLATFORM_FCM, "aa").expect("prove"));
    assert!(!mark_proven(&f.app, PLATFORM_FCM, "aa").expect("again"), "once");
    assert!(config(&f.app).expect("cfg").fcm.expect("fcm").proven_at.is_some());
    set_fcm_meta(&f.app, Some(&fm), "owner").expect("new key");
    assert_eq!(config(&f.app).expect("cfg").fcm.expect("fcm").proven_at, None, "a new key is unproven");
    assert!(mark_proven(&f.app, "web", "aa").is_err());
    assert_eq!(set_apns_meta(&f.app, None, "owner").expect("clear").apns, None);
    assert!(!mark_proven(&f.app, PLATFORM_APNS, "0011223344556677").expect("no apns key"), "nothing to prove without a key");

    let p = session("dana", &f.app);
    register(&f.app, &f.dana, &p, &reg("p1", "apns", APNS_A)).expect("register");
    assert_eq!(purge_app(&f.app).expect("purge"), 1);
    assert_eq!(count(&f.app), 0);
    let c = config(&f.app).expect("cfg");
    assert!(c.enabled && !c.owner_on && c.fcm.is_none(), "settings back to defaults: {c:?}");
    assert_eq!(purge_app(HOST_APP_ID).expect("host"), 0);
    let e = register(HOST_APP_ID, &f.dana, &p, &reg("p1", "apns", APNS_A)).expect_err("never host");
    assert!(matches!(e, RegisterError::Store(ref m) if m.contains("never the box login")), "{e:?}");
}
