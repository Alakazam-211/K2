//! `POST /cli/app/user/import` through the App routes: who may, dry run
//! and apply, the 409 on clashes, the App's service as the grant target.
//! Synthetic users only.

use k2_core::connect_users::Role;
use k2_core::skin;
use serde_json::{json, Value as J};

use super::super::handover_tests::{agent, call, code, fx};
use super::super::routes::Who;
use super::IMPORT;

fn bcrypt_of(pw: &str) -> String {
    skin::import::bcrypt_for_tests(pw)
}

fn lines(users: &[J]) -> String {
    users.iter().map(|u| format!("{u}\n")).collect()
}

fn user(id: &str, email: &str, pw: &str) -> J {
    json!({"id": id, "email": email, "encrypted_password": bcrypt_of(pw), "raw_app_meta_data": {"provider": "email"}, "created_at": "2024-05-01 08:00:00+00"})
}

#[test]
fn the_owner_dry_runs_then_applies_and_the_user_signs_in_with_the_old_password() {
    let f = fx("uimp1");
    let id = "aaaaaaaa-1111-4222-8333-444444444441";
    let body = json!({ "app": f.app, "lines": lines(&[user(id, "pat@example.com", "pat-old-pw")]) });
    let (st, dry) = call(IMPORT, &Who::Owner, body.clone());
    assert_eq!(st, 200, "{dry}");
    assert_eq!((dry["new"].as_u64(), dry["written"].as_bool(), dry["app"].as_str()), (Some(1), Some(false), Some(f.app.as_str())), "{dry}");
    assert!(!dry.to_string().contains("$2a$"), "no hash in an answer");
    assert!(skin::list_principals().unwrap().iter().all(|p| p.id != id), "a dry run writes nothing");

    let mut apply = body;
    apply["apply"] = json!(true);
    let (st, done) = call(IMPORT, &Who::Owner, apply);
    assert_eq!((st, done["written"].as_bool()), (200, Some(true)), "{done}");
    let p = skin::list_principals().unwrap().into_iter().find(|p| p.id == id).expect("kept its id");
    let g = skin::list_grants().unwrap().into_iter().find(|g| g.subject_id == id).expect("an App grant");
    assert_eq!(g.target_id, f.svc, "the grant names the App's published service");
    assert!(matches!(skin::check_and_record_login("pat@example.com", "pat-old-pw"), skin::SkinLoginOutcome::Ok(ref q) if q.id == p.id));
    assert!(skin::import::stored_hash_for_tests(&p.username).unwrap().starts_with("$argon2"), "rehashed at the first sign-in");
}

#[test]
fn an_apply_with_a_clash_is_409_and_writes_nothing() {
    let f = fx("uimp2");
    skin::add_principal_with_email("already", Some("dup@example.com")).unwrap();
    let body = json!({ "app": f.app, "apply": true, "lines": lines(&[
        user("aaaaaaaa-1111-4222-8333-444444444442", "dup@example.com", "x-pw-1"),
        user("aaaaaaaa-1111-4222-8333-444444444443", "ok@example.com", "x-pw-2"),
    ]) + "not json\n" });
    let (st, v) = call(IMPORT, &Who::Owner, body);
    assert_eq!(st, 409, "{v}");
    assert_eq!(code(&v), skin::import::CLASHES);
    assert_eq!(v["clashes"].as_array().map(Vec::len), Some(2), "the bad line and the taken email: {v}");
    assert!(skin::list_principals().unwrap().iter().all(|p| p.email.as_deref() != Some("ok@example.com")), "nothing written");
}

#[test]
fn people_other_than_the_owner_and_agents_without_the_switch_are_refused() {
    let f = fx("uimp3");
    let body = json!({ "app": f.app, "lines": lines(&[user("aaaaaaaa-1111-4222-8333-444444444444", "q@example.com", "q-pw-1")]) });
    let admin = Who::Login { username: "ann".into(), role: Role::Admin };
    let (st, v) = call(IMPORT, &admin, body.clone());
    assert_eq!((st, code(&v)), (403, "forbidden"), "{v}");
    let (st, v) = call(IMPORT, &Who::Wrong, body.clone());
    assert_eq!(st, 403, "{v}");
    let (st, v) = call(IMPORT, &agent(&f.a), body.clone());
    assert_eq!((st, code(&v)), (403, crate::skin_routes::APP_USERS_OFF), "{v}");
    let (st, v) = call(IMPORT, &agent(&f.b), body.clone());
    assert_eq!(st, 403, "another workspace's agent: {v}");

    k2_core::workspace::settings::set_agents_can_manage_app_users(&f.a, true).unwrap();
    let mut apply = body;
    apply["apply"] = json!(true);
    let (st, v) = call(IMPORT, &agent(&f.a), apply);
    assert_eq!((st, v["written"].as_bool()), (200, Some(true)), "with the switch, its own App: {v}");
    assert_eq!(skin::creators::workspace_of("aaaaaaaa-1111-4222-8333-444444444444").unwrap().as_deref(), Some(f.a.as_str()), "stamped as its");

    k2_core::workspace::settings::set_agents_can_manage_app_users(&f.b, true).unwrap();
    let (st, v) = call(IMPORT, &agent(&f.b), json!({ "app": f.app, "lines": "{}\n" }));
    assert_eq!((st, code(&v)), (404, "app_not_found"), "not its App: {v}");
    let (st, v) = call(IMPORT, &Who::Owner, json!({ "app": f.app }));
    assert_eq!(st, 400, "no lines: {v}");
}
