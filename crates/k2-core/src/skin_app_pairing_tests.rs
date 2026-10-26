//! App phone pairing, core (S1): codes (one-time, bound, capped, digest
//! only), the redeem's re-checks, the paired device sign-in, push linking,
//! test guests and ties, and the owner config's validation (S3's core half).

use super::*;
use crate::skin::{
    self, add_principal_for_app, app_login, app_push, create_app_session_token, create_role_for_app, list_grants, resolve_skin_token,
    revoke_sessions_for_app, revoke_token, set_grant_enabled, set_principal_password, RoomPolicy,
};

struct Fx {
    _home: crate::test_env::TempHome,
    _db: crate::db::TestDbGuard,
    app: String,
    other_app: String,
    room: String,
    other_room: String,
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
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![r, format!("/tmp/ap-{r}")]).unwrap();
        }
        let a = crate::published_services::insert(&conn, &room, "jobs", "node s.js", "/tmp", 4200, "local", "running", "cmd", "").unwrap();
        let o = crate::published_services::insert(&conn, &other_room, "crm", "node s.js", "/tmp", 4201, "local", "running", "cmd", "").unwrap();
        (a.id, o.id)
    };
    let mut policy = RoomPolicy::new();
    policy.insert(room.clone(), vec!["thread:read".into(), "thread:post".into()]);
    policy.insert(other_room.clone(), vec!["thread:read".into()]);
    create_role_for_app(&format!("r-{tag}"), &policy, Some(&app)).unwrap();
    add_principal_for_app("dana", None, &app, Some(&format!("r-{tag}")), "test").unwrap();
    set_principal_password("dana", Some("s3cret-horse")).unwrap();
    add_principal_for_app("eli", None, &other_app, None, "test").unwrap();
    turn_on(&app);
    turn_on(&other_app);
    Fx { _home: home, _db: db, app, other_app, room, other_room }
}

fn turn_on(app: &str) {
    let input = ConfigInput { ios: vec!["ABCDE12345.com.you.app".into()], ..Default::default() };
    set_config(app, &input, "owner").expect("pairing on");
}

/// A fresh web session pass of `user` on `app`: (token id, principal id).
fn web(user: &str, app: &str) -> (String, String) {
    let s = create_app_session_token(user, app).expect("web session");
    let p = resolve_skin_token(&s.raw).expect("live pass");
    (s.meta.id, p.principal_id.expect("a login session"))
}

fn start(f: &Fx) -> (Minted, String, String) {
    let (pass, pid) = web("dana", &f.app);
    reset_starts_for_test(&f.app, &pid);
    let m = mint(&f.app, &pid, &pass, "jobs.box.k2.dev").expect("mint");
    (m, pass, pid)
}

fn phone(id: &str) -> app_login::Device {
    app_login::Device { id: id.into(), name: "Dana's iPhone".into() }
}

fn redeem(code: &str, app: &str, device: &app_login::Device) -> Result<Paired, Refused> {
    let p = take(code, app)?;
    let ws = skin::ties::app_workspace(app);
    let r = login_device_paired(&p, device, ws.as_deref()).expect("store");
    match &r {
        Paired::Ok { session, .. } => finish_paired(&p.pairing_id, &session.device.name, &session.device_login_id),
        Paired::Refused(_) => finish_failed(&p.pairing_id),
    }
    Ok(r)
}

fn paired(r: Result<Paired, Refused>) -> (app_login::DeviceSession, String) {
    match r {
        Ok(Paired::Ok { session, principal_id }) => (*session, principal_id),
        other => panic!("expected a pairing, got {other:?}"),
    }
}

fn refused(r: Result<Paired, Refused>) -> &'static str {
    match r {
        Ok(Paired::Refused(why)) => why,
        other => panic!("expected a refused re-check, got {other:?}"),
    }
}

#[test]
fn a_code_pairs_once_into_a_device_sign_in_whose_refresh_works() {
    let f = fx("rt");
    let (m, pass, pid) = start(&f);
    assert!(m.code.starts_with("K2A-") && m.code.len() == 27, "{}", m.code);
    assert!(is_pairing_id(&m.pairing_id), "{}", m.pairing_id);
    let st = status(&f.app, &pid, &m.pairing_id).expect("own status");
    assert_eq!(st.state, State::Pending);
    assert!(st.expires_in_secs.is_some_and(|s| s > 0 && s <= CODE_TTL_SECS), "{st:?}");
    // A check names the person and never consumes.
    for _ in 0..3 {
        let c = check(&m.code, &f.app).expect("valid");
        assert_eq!((c.principal_id.as_str(), c.pairing_id.as_str()), (pid.as_str(), m.pairing_id.as_str()));
    }
    let (d, who) = paired(redeem(&m.code, &f.app, &phone("iphone-1")));
    assert_eq!(who, pid);
    assert_eq!(d.session.app, f.app);
    assert!(app_login::is_refresh_token(&d.refresh));
    let pass_now = resolve_skin_token(&d.session.raw).expect("the paired pass works");
    assert_eq!(pass_now.principal_id.as_deref(), Some(pid.as_str()));
    assert_eq!(pass_now.app, f.app);
    // One-time.
    assert_eq!(take(&m.code, &f.app).err(), Some(Refused::Unknown));
    assert_eq!(check(&m.code, &f.app).err(), Some(Refused::Unknown));
    // The status names the device; the person's devices say how it started.
    match status(&f.app, &pid, &m.pairing_id).expect("status").state {
        State::Paired { device_name, device_login_id, .. } => {
            assert_eq!(device_name, "Dana's iPhone");
            assert_eq!(device_login_id, d.device_login_id);
        }
        other => panic!("{other:?}"),
    }
    let devices = app_login::devices_for(&pid, &f.app).expect("devices");
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].via, app_login::VIA_PAIRING);
    let rows = pairings_for_app(&f.app, 10).expect("log");
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].host.as_str(), rows[0].username.as_str(), rows[0].live), ("jobs.box.k2.dev", "dana", true));
    assert_eq!(rows[0].pairing_id, m.pairing_id);
    assert_eq!(counts(&f.app, 0).expect("counts"), (1, 1));
    // A password device login is told apart.
    let pw = app_login::login_device("dana", &f.app, &phone("ipad-2")).expect("login").expect("on");
    let devices = app_login::devices_for(&pid, &f.app).expect("devices");
    let ipad = devices.iter().find(|x| x.id == pw.device_login_id).expect("ipad");
    assert_eq!(ipad.via, app_login::VIA_PASSWORD);
    // The refresh works like any device sign-in's.
    match app_login::refresh(&d.refresh, Some(&f.app)).expect("refresh") {
        app_login::Refresh::Ok(n) => assert_eq!(n.device_login_id, d.device_login_id),
        other => panic!("{other:?}"),
    }
    // The web pass is untouched by the pairing.
    assert!(session_info(&pass).expect("info").is_some());
}

#[test]
fn two_redeems_of_one_code_at_once_pair_exactly_once() {
    let f = fx("race");
    let (m, _, _) = start(&f);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|i| {
            let (c, a, b) = (m.code.clone(), f.app.clone(), barrier.clone());
            std::thread::spawn(move || {
                b.wait();
                matches!(redeem(&c, &a, &phone(&format!("race{i}"))), Ok(Paired::Ok { .. }))
            })
        })
        .collect();
    let wins = handles.into_iter().map(|h| h.join().expect("join")).filter(|ok| *ok).count();
    assert_eq!(wins, 1);
}

#[test]
fn expired_cancelled_unknown_malformed_and_another_kind_all_refuse() {
    let f = fx("bad");
    let (m, _, pid) = start(&f);
    expire_for_test(&m.pairing_id);
    assert_eq!(check(&m.code, &f.app).err(), Some(Refused::Expired));
    assert_eq!(take(&m.code, &f.app).err(), Some(Refused::Expired));
    assert_eq!(status(&f.app, &pid, &m.pairing_id).expect("status").state, State::Expired);
    let (m2, _, _) = start(&f);
    assert!(cancel(&f.app, &pid, &m2.pairing_id));
    assert!(!cancel(&f.app, &pid, &m2.pairing_id), "a second cancel is a miss");
    assert_eq!(take(&m2.code, &f.app).err(), Some(Refused::Unknown));
    assert_eq!(status(&f.app, &pid, &m2.pairing_id).expect("status").state, State::Cancelled);
    for raw in ["", "K2A-", "garbage", "K2A-00000-00000-00000-0000U", "K2A-00000-00000-00000-00000"] {
        assert_eq!(take(raw, &f.app).err(), Some(Refused::Unknown), "{raw}");
    }
    // A Companion code is not an App code, whatever its shape, and an App
    // code is not a Companion code.
    crate::connect_users::add_user("cpanion", "password123").expect("add");
    let k2p = crate::connect_device_pairing::mint("cpanion", "owner-token").expect("companion mint");
    assert_eq!(take(&k2p.code, &f.app).err(), Some(Refused::Unknown));
    let (m3, _, _) = start(&f);
    assert_eq!(crate::connect_device_pairing::redeem(&m3.code, "d", "D").expect("store").err(), Some(crate::connect_device_pairing::Refused::Unknown));
    assert!(crate::connect_device_pairing::redeem(&k2p.code, "d", "D").expect("store").is_ok(), "the Companion code still works there");
    assert!(matches!(redeem(&m3.code, &f.app, &phone("p3")), Ok(Paired::Ok { .. })), "and the App code still works here");
}

#[test]
fn a_code_is_its_apps_only_and_another_apps_redeem_consumes_it_without_a_pass() {
    let f = fx("xapp");
    let (m, _, pid) = start(&f);
    assert_eq!(check(&m.code, &f.other_app).err(), Some(Refused::WrongApp));
    assert!(check(&m.code, &f.app).is_ok(), "a check never consumes");
    assert_eq!(take(&m.code, &f.other_app).err(), Some(Refused::WrongApp));
    assert_eq!(take(&m.code, &f.app).err(), Some(Refused::Unknown), "the attempt consumed it");
    assert!(app_login::devices_for(&pid, &f.app).expect("devices").is_empty());
    assert!(app_login::devices_for(&pid, &f.other_app).expect("devices").is_empty());
    // A pairingId of App A answers like an unknown one on App B, and for
    // another person.
    let (m2, _, _) = start(&f);
    assert!(status(&f.app, &pid, &m2.pairing_id).is_some());
    assert!(status(&f.other_app, &pid, &m2.pairing_id).is_none());
    let (_, eli) = web("eli", &f.other_app);
    assert!(status(&f.app, &eli, &m2.pairing_id).is_none());
    assert!(!cancel(&f.app, &eli, &m2.pairing_id), "someone else's cancel is a miss");
    assert!(check(&m2.code, &f.app).is_ok());
}

#[test]
fn the_code_dies_with_its_web_pass_and_the_persons_password_or_grant() {
    let f = fx("dies");
    // Logout (the pass revoked).
    let (m, pass, _) = start(&f);
    assert!(revoke_token(&pass).expect("revoke"));
    assert_eq!(refused(redeem(&m.code, &f.app, &phone("a"))), "web_pass_ended");
    // A password change ends the web pass and every device sign-in.
    let (m, _, pid) = start(&f);
    let (old, _) = paired(redeem(&start(&f).0.code, &f.app, &phone("old")));
    set_principal_password("dana", Some("another-horse-9")).expect("set");
    assert_eq!(refused(redeem(&m.code, &f.app, &phone("b"))), "web_pass_ended");
    assert!(resolve_skin_token(&old.session.raw).is_none(), "the paired phone ended with the password change");
    assert!(app_login::devices_for(&pid, &f.app).expect("devices").is_empty());
    // The grant disabled.
    let (m, _, _) = start(&f);
    let g = list_grants().expect("grants").into_iter().find(|g| g.target_id == f.app).expect("grant");
    set_grant_enabled(&g.id, false).expect("disable");
    assert_eq!(refused(redeem(&m.code, &f.app, &phone("c"))), "web_pass_ended");
    set_grant_enabled(&g.id, true).expect("enable");
    // The App removed.
    let (m, _, _) = start(&f);
    revoke_sessions_for_app(&f.app).expect("app removed");
    assert_eq!(refused(redeem(&m.code, &f.app, &phone("d"))), "web_pass_ended");
}

#[test]
fn an_owner_claim_ends_codes_and_paired_phones() {
    let f = fx("claim");
    let (m, _, pid) = start(&f);
    let (d, _) = paired(redeem(&start(&f).0.code, &f.app, &phone("p")));
    skin::ties::set(&pid, skin::ties::TieWrite { workspace_id: &f.room, set_by: "agent:x", reason: "password" }).expect("tie");
    skin::ties::owner_claim(&pid, "owner").expect("claim");
    assert_eq!(refused(redeem(&m.code, &f.app, &phone("q"))), "web_pass_ended");
    assert!(resolve_skin_token(&d.session.raw).is_none());
}

#[test]
fn pairing_off_stay_signed_in_off_and_a_config_change_refuse() {
    let f = fx("off");
    // Off at redeem.
    let (m, _, _) = start(&f);
    set_off_raw(&f.app);
    assert_eq!(refused(redeem(&m.code, &f.app, &phone("a"))), "pairing_off");
    turn_on(&f.app);
    // set_off kills outstanding codes at once (and a set too).
    let (m, _, pid) = start(&f);
    assert!(set_off(&f.app, "owner").expect("off"));
    assert_eq!(take(&m.code, &f.app).err(), Some(Refused::Unknown));
    assert_eq!(status(&f.app, &pid, &m.pairing_id).expect("status").state, State::Cancelled);
    turn_on(&f.app);
    let (m, _, _) = start(&f);
    turn_on(&f.app);
    assert_eq!(take(&m.code, &f.app).err(), Some(Refused::Unknown), "a config change kills codes");
    // Stay signed in 0.
    let (m, _, _) = start(&f);
    app_login::set_settings(&f.app, None, Some(0), "owner").expect("stay 0");
    assert_eq!(refused(redeem(&m.code, &f.app, &phone("b"))), "device_signin_off");
}

/// Turn pairing off in the table without killing codes (the race a redeem
/// re-checks).
fn set_off_raw(app: &str) {
    with_conn(|conn| {
        conn.execute("UPDATE app_pairing_config SET enabled = 0 WHERE app_id = ?1", params![app]).map_err(|e| e.to_string())?;
        Ok(())
    })
    .expect("raw off");
}

#[test]
fn codes_are_capped_per_person_and_per_app_and_starts_per_hour() {
    let f = fx("caps");
    let (pass, pid) = web("dana", &f.app);
    reset_starts_for_test(&f.app, &pid);
    let minted: Vec<Minted> = (0..MAX_PENDING_PER_PERSON + 1).map(|_| mint(&f.app, &pid, &pass, "h.k2.dev").expect("mint")).collect();
    assert_eq!(take(&minted[0].code, &f.app).err(), Some(Refused::Unknown), "the oldest went when a 4th was made");
    assert_eq!(status(&f.app, &pid, &minted[0].pairing_id).expect("status").state, State::Cancelled);
    assert!(check(&minted[MAX_PENDING_PER_PERSON].code, &f.app).is_ok());
    // Starts per hour.
    let more = STARTS_PER_HOUR - (MAX_PENDING_PER_PERSON + 1);
    for _ in 0..more {
        mint(&f.app, &pid, &pass, "h.k2.dev").expect("mint");
    }
    match mint(&f.app, &pid, &pass, "h.k2.dev") {
        Err(MintError::RateLimited { retry_after_ms }) => assert!(retry_after_ms > 0 && retry_after_ms <= 3_600_000),
        other => panic!("expected rate_limited, got {other:?}"),
    }
    // Per App: past MAX_PENDING_PER_APP the App's oldest goes.
    let app = format!("cap-app-{}", uuid::Uuid::new_v4());
    let first = mint(&app, "p-0", "t", "h").expect("mint");
    for i in 1..=MAX_PENDING_PER_APP {
        mint(&app, &format!("p-{i}"), "t", "h").expect("mint");
    }
    assert_eq!(outstanding_for(&app), MAX_PENDING_PER_APP);
    assert_eq!(check(&first.code, &app).err(), Some(Refused::Unknown), "the App's oldest went");
    assert_eq!(drop_app(&app), MAX_PENDING_PER_APP);
}

#[test]
fn the_store_holds_digests_never_codes() {
    let f = fx("dig");
    let (m, _, _) = start(&f);
    let body = normalize(&m.code).expect("normalizes");
    let keys = code_keys();
    assert!(keys.iter().all(|k| k.len() == 64 && k.bytes().all(|b| b.is_ascii_hexdigit())), "{keys:?}");
    assert!(keys.iter().all(|k| !k.contains(&body) && !k.contains(&m.code)));
    assert!(keys.contains(&digest(&body)));
}

#[test]
fn normalize_folds_prefix_case_separators_and_lookalikes() {
    let body = "0123456789ABCDEFGHJK";
    assert_eq!(normalize(body).as_deref(), Some(body));
    assert_eq!(normalize("k2a-01234-56789-abcde-fghjk").as_deref(), Some(body));
    assert_eq!(normalize("K2A 0i234 56789 ABCDE FGHJK").as_deref(), Some(body));
    assert_eq!(normalize("K2P-01234-56789-ABCDE-FGHJK"), None, "a Companion prefix is not stripped here");
    assert_eq!(normalize("0123456789ABCDEFGHJ"), None);
    assert_eq!(normalize(&display(body)).as_deref(), Some(body));
}

#[test]
fn the_same_device_replaces_its_sign_in_and_the_device_cap_holds() {
    let f = fx("cap20");
    let (first, _) = paired(redeem(&start(&f).0.code, &f.app, &phone("same")));
    let (second, pid) = paired(redeem(&start(&f).0.code, &f.app, &phone("same")));
    assert!(resolve_skin_token(&first.session.raw).is_none(), "replaced");
    assert!(resolve_skin_token(&second.session.raw).is_some());
    for i in 0..app_login::MAX_LIVE_DEVICES {
        let p = skin::app_pairing::Pending {
            app: f.app.clone(),
            principal_id: pid.clone(),
            minted_from: web("dana", &f.app).0,
            host: "h".into(),
            pairing_id: format!("{ID_PREFIX}t{i:021}"),
            created_at: now_secs(),
            expires_at: now_secs() + 60,
            seq: 0,
        };
        let ws = skin::ties::app_workspace(&f.app);
        assert!(matches!(login_device_paired(&p, &phone(&format!("dev{i}")), ws.as_deref()).expect("store"), Paired::Ok { .. }));
    }
    assert_eq!(app_login::devices_for(&pid, &f.app).expect("devices").len() as i64, app_login::MAX_LIVE_DEVICES);
    let ended_at_the_cap: i64 = with_conn(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM device_logins WHERE principal_id = ?1 AND app_id = ?2 AND revoked_reason = ?3",
            params![pid, f.app, app_login::REASON_LIMIT],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())
    })
    .expect("count");
    assert_eq!(ended_at_the_cap, 1, "the 21st sign-in ended the least recently used one");
}

#[test]
fn a_tied_persons_paired_pass_is_held_to_the_tie() {
    let f = fx("tie");
    let (_, pid) = web("dana", &f.app);
    skin::ties::set(&pid, skin::ties::TieWrite { workspace_id: &f.room, set_by: "agent:x", reason: "password" }).expect("tie");
    let (d, _) = paired(redeem(&start(&f).0.code, &f.app, &phone("t")));
    let p = resolve_skin_token(&d.session.raw).expect("live");
    assert!(p.has_room(&f.room), "{:?}", p.rooms);
    assert!(!p.has_room(&f.other_room), "the tie's ceiling drops the other room: {:?}", p.rooms);
}

#[test]
fn a_test_guest_pairs_only_inside_its_apps_link() {
    let f = fx("tg");
    add_principal_for_app("probe", None, &f.app, None, "test").expect("guest");
    let pid = skin::list_principals().expect("list").into_iter().find(|p| p.username == "probe").expect("probe").id;
    skin::test_guests::record(&pid, &f.app, &f.room, "agent:x", now_secs() + 3600).expect("test guest");
    let web_pass = skin::test_guests::with_link_login(Some(&f.app), || create_app_session_token("probe", &f.app)).expect("link session");
    let mk = || {
        reset_starts_for_test(&f.app, &pid);
        mint(&f.app, &pid, &web_pass.meta.id, "h").expect("mint")
    };
    // The main port: the mint is refused like a missing grant.
    let m = mk();
    assert_eq!(refused(redeem(&m.code, &f.app, &phone("tg1"))), "grant");
    assert_eq!(take(&m.code, &f.app).err(), Some(Refused::Unknown), "consumed");
    // Through the App's own link.
    let m = mk();
    let r = skin::test_guests::with_link_login(Some(&f.app), || redeem(&m.code, &f.app, &phone("tg2")));
    let (d, _) = paired(r);
    assert!(resolve_skin_token(&d.session.raw).is_some());
}

#[test]
fn push_in_the_same_step_links_to_the_paired_sign_in_and_ends_with_it() {
    let f = fx("push");
    let (d, pid) = paired(redeem(&start(&f).0.code, &f.app, &phone("ph")));
    let reg = app_push::parse_registration(&serde_json::json!({
        "deviceId": "ph", "platform": "apns", "pushToken": "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90"
    }))
    .expect("reg");
    let row = app_push::register(&f.app, &pid, &d.session.meta.id, &reg).expect("register");
    assert_eq!(row.device_login_id.as_deref(), Some(d.device_login_id.as_str()));
    assert!(app_login::revoke_own(&pid, &f.app, &d.device_login_id).expect("revoke"));
    assert!(app_push::devices_for_app(&f.app, None).expect("list").is_empty(), "the push row ended with the sign-in");
}

#[test]
fn session_info_reads_a_live_session_only() {
    let f = fx("info");
    let (pass, pid) = web("dana", &f.app);
    let i = session_info(&pass).expect("info").expect("live");
    assert_eq!((i.principal_id.as_str(), i.username.as_str(), i.app.as_str()), (pid.as_str(), "dana", f.app.as_str()));
    assert!(now_secs() - i.created_at < 60);
    revoke_token(&pass).expect("revoke");
    assert!(session_info(&pass).expect("info").is_none());
    assert!(session_info("nope").expect("info").is_none());
}

// ── the owner config (§5.1) ──────────────────────────────────────────

const FPR: &str = "AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89";

fn bad(v: serde_json::Value, field: &str) {
    let e = parse_input(&v).expect_err(&format!("{v} must be refused"));
    assert!(e.starts_with(field), "{v}: the error names {field}: {e}");
}

#[test]
fn every_field_is_checked_and_the_error_names_it() {
    use serde_json::json;
    let ok = parse_input(&json!({
        "app": "x",
        "ios": ["ABCDE12345.com.you.app", "ABCDE12345.com.you.app"],
        "android": [{"package": "com.you.app", "sha256": [FPR.to_ascii_lowercase()]}],
        "hosts": ["Jobs.Box.K2.dev."],
        "nativeOrigins": ["tauri://localhost", "Capacitor://localhost", "https://localhost", "http://localhost:8080"],
        "linkPaths": ["/jobs/*"],
        "store": {"ios": "https://apps.apple.com/app/id123", "android": "https://play.google.com/store/apps/details?id=com.you.app"},
    }))
    .expect("valid");
    assert_eq!(ok.ios, ["ABCDE12345.com.you.app"], "deduplicated");
    assert_eq!(ok.android[0].sha256, [FPR], "uppercased");
    assert_eq!(ok.hosts, Some(vec!["jobs.box.k2.dev".to_string()]));
    assert_eq!(ok.native_origins[1], "capacitor://localhost");
    bad(json!({"ios": ["abcde12345.com.you.app"]}), "ios");
    bad(json!({"ios": ["ABCDE1234.com.you.app"]}), "ios");
    bad(json!({"ios": ["ABCDE12345.app"]}), "ios");
    bad(json!({"ios": ["ABCDE12345.com..app"]}), "ios");
    bad(json!({"ios": "ABCDE12345.com.you.app"}), "ios");
    bad(json!({"ios": (0..11).map(|i| format!("ABCDE12345.com.you.a{i}")).collect::<Vec<_>>()}), "ios");
    bad(json!({"android": [{"package": "app", "sha256": [FPR]}]}), "android");
    bad(json!({"android": [{"package": "1com.you", "sha256": [FPR]}]}), "android");
    bad(json!({"android": [{"package": "com.you.app", "sha256": []}]}), "android");
    bad(json!({"android": [{"package": "com.you.app", "sha256": ["AB:CD"]}]}), "android.sha256");
    bad(json!({"android": [{"package": "com.you.app", "sha256": [FPR.replace(':', "")]}]}), "android.sha256");
    bad(json!({"android": [{"package": "com.you.app", "sha256": vec![FPR; 6]}]}), "android.sha256");
    bad(json!({"android": [{"package": "com.you.app", "sha256": [FPR], "extra": 1}]}), "android");
    bad(json!({}), "ios and/or android");
    bad(json!({"ios": [], "android": []}), "ios and/or android");
    let base = || json!({"ios": ["ABCDE12345.com.you.app"]});
    let with = |k: &str, v: serde_json::Value| {
        let mut b = base();
        b[k] = v;
        b
    };
    bad(with("hosts", json!(["no_dots"])), "hosts");
    bad(with("hosts", json!([])), "hosts");
    bad(with("nativeOrigins", json!(["https://evil.example"])), "nativeOrigins");
    bad(with("nativeOrigins", json!(["tauri://localhost/path"])), "nativeOrigins");
    bad(with("nativeOrigins", json!(["localhost"])), "nativeOrigins");
    bad(with("nativeOrigins", json!(["a://b", "c://d", "e://f", "g://h", "i://j", "k://l"])), "nativeOrigins");
    bad(with("linkPaths", json!(["/.well-known/apple-app-site-association"])), "linkPaths");
    bad(with("linkPaths", json!(["/.WELL-KNOWN/x"])), "linkPaths");
    bad(with("linkPaths", json!(["jobs/*"])), "linkPaths");
    bad(with("linkPaths", json!(["/jobs?x"])), "linkPaths");
    bad(with("store", json!({"ios": "https://evil.example/app"})), "store.ios");
    bad(with("store", json!({"android": "https://play.google.com.evil.example/store/apps/details?id=x"})), "store.android");
    bad(with("store", json!({"android": "https://play.google.com/store/apps/details?id=\"><script>"})), "store.android");
    bad(with("store", json!({"web": "https://x"})), "store");
    bad(with("app2", json!(1)), "unknown field");
}

#[test]
fn set_off_and_purge_round_trip() {
    let f = fx("cfg");
    let c = config(&f.app).expect("read").expect("set");
    assert!(c.is_on());
    assert_eq!(c.updated_by.as_deref(), Some("owner"));
    assert!(set_off(&f.app, "agent:jobs").expect("off"));
    let c = config(&f.app).expect("read").expect("kept");
    assert!(!c.is_on() && !c.enabled);
    assert_eq!(c.ios, ["ABCDE12345.com.you.app"], "off keeps the ids");
    assert!(!set_off(&f.app, "owner").expect("off again"), "already off");
    assert!(all_configs().expect("all").iter().any(|c| c.app == f.app));
    assert_eq!(purge_app(&f.app).expect("purge"), 1);
    assert!(config(&f.app).expect("read").is_none());
    assert!(set_config(HOST_APP_ID, &ConfigInput { ios: vec!["ABCDE12345.com.you.app".into()], ..Default::default() }, "owner").is_err());
}
