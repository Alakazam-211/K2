//! App user import: given ids, bcrypt at sign-in and the rehash, reruns,
//! clashes (prd-app-supabase-compat-v1 §9.2 and §12's import tests).
//! Synthetic users only.

use super::*;
use crate::skin::{self, SkinLoginOutcome};
use serde_json::json;

const APP: &str = "svc-import-test";

fn bcrypt_of(password: &str, v: bcrypt::Version) -> String {
    bcrypt::hash_with_result(password, 4).expect("bcrypt").format_for_version(v)
}

fn opts(apply: bool) -> Options<'static> {
    Options { app: APP, role: None, apply, agent_workspace: None, actor: "owner", reserved: &[] }
}

fn rows(lines: &[J]) -> (Vec<ImportRow>, Vec<Clash>) {
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    parse_jsonl(&text, now_secs())
}

fn supa_user(id: &str, email: &str, hash: &str) -> J {
    // The columns a Supabase `auth.users` export carries (more than K2 reads).
    json!({
        "instance_id": "00000000-0000-0000-0000-000000000000",
        "id": id, "aud": "authenticated", "role": "authenticated",
        "email": email, "encrypted_password": hash,
        "email_confirmed_at": "2024-03-01 10:00:00.123456+00",
        "raw_app_meta_data": {"provider": "email", "providers": ["email"], "org": "acme"},
        "raw_user_meta_data": {"full_name": "Ada Example"},
        "created_at": "2024-03-01T10:00:00.123456+00:00",
        "banned_until": null,
    })
}

fn login(name: &str, pw: &str) -> SkinLoginOutcome {
    skin::check_and_record_login(name, pw)
}

#[test]
fn bcrypt_shapes_are_recognised_and_nothing_else_verifies() {
    let a = bcrypt_of("pw-one", bcrypt::Version::TwoA);
    let b = bcrypt_of("pw-one", bcrypt::Version::TwoB);
    let y = bcrypt_of("pw-one", bcrypt::Version::TwoY);
    for h in [&a, &b, &y] {
        assert!(is_bcrypt(h), "{h}");
        assert!(verify_legacy("pw-one", h), "{h} verifies");
        assert!(!verify_legacy("pw-two", h), "{h} refuses a wrong password");
    }
    assert!(a.starts_with("$2a$") && b.starts_with("$2b$") && y.starts_with("$2y$"));
    assert!(!is_bcrypt(&a.replacen("$04$", "$31$", 1)), "a cost that would stall every sign-in is refused");
    assert!(is_bcrypt(&a.replacen("$04$", "$12$", 1)));
    assert!(!is_bcrypt("$2x$04$abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123"));
    assert!(!verify_legacy("pw-one", "plain-text"));
    assert!(!verify_legacy("pw-one", "$argon2id$v=19$m=19456,t=2,p=1$AAAA$AAAA"));
}

#[test]
fn lines_parse_from_a_supabase_export_and_bad_ones_are_named() {
    let now = 1_800_000_000;
    let h = bcrypt_of("x", bcrypt::Version::TwoA);
    let ok = parse_row(1, &supa_user("A1B2C3D4-0000-4000-8000-000000000001", "Ada@Example.com", &h), now).expect("parses");
    assert_eq!(ok.id, "a1b2c3d4-0000-4000-8000-000000000001", "the id kept, in UUID form");
    assert_eq!(ok.email, "ada@example.com");
    assert_eq!(ok.password_hash.as_deref(), Some(h.as_str()));
    assert_eq!(ok.created_at, Some(1_709_287_200));
    assert!(ok.app_metadata.as_deref().is_some_and(|m| m.contains("\"org\":\"acme\"")));
    assert!(!ok.banned);
    let short = parse_row(2, &json!({"id": "a1b2c3d4-0000-4000-8000-000000000002", "email": "b@example.com",
        "password_hash": h, "app_metadata": {"k": 1}, "username": "Bee", "banned_until": "2999-01-01 00:00:00+00"}), now)
        .expect("short names parse");
    assert_eq!(short.username.as_deref(), Some("bee"));
    assert!(short.banned, "a ban that runs into the future");
    let past = parse_row(3, &json!({"id": "a1b2c3d4-0000-4000-8000-000000000003", "email": "c@example.com", "banned_until": "2001-01-01T00:00:00Z"}), now).unwrap();
    assert!(!past.banned && past.password_hash.is_none());

    let reason = |v: J| parse_row(9, &v, now).expect_err("refused").reason;
    assert!(reason(json!({"id": "nope", "email": "d@example.com"})).contains("UUID"));
    assert!(reason(json!({"id": "a1b2c3d4-0000-4000-8000-000000000004"})).contains("no email"));
    assert!(reason(json!({"id": "a1b2c3d4-0000-4000-8000-000000000004", "email": "x@example.com", "encrypted_password": "md5abc"})).contains("bcrypt"));
    assert!(reason(json!({"id": "a1b2c3d4-0000-4000-8000-000000000004", "email": "x@example.com", "app_metadata": [1]})).contains("JSON object"));
    let big = "x".repeat(METADATA_MAX);
    assert!(reason(json!({"id": "a1b2c3d4-0000-4000-8000-000000000004", "email": "x@example.com", "user_metadata": {"b": big}})).contains("at most"));
    assert!(reason(json!({"id": "a1b2c3d4-0000-4000-8000-000000000004", "email": "x@example.com", "created_at": "yesterday"})).contains("timestamp"));
    assert!(reason(json!({"id": "a1b2c3d4-0000-4000-8000-000000000004", "email": "x@example.com", "created_at": "2024-01-01 ünï"})).contains("timestamp"));
    let (good, bad) = parse_jsonl("{\"id\":\"a1b2c3d4-0000-4000-8000-000000000005\",\"email\":\"e@example.com\"}\n\nnot json\n", now);
    assert_eq!((good.len(), bad.len(), bad[0].line), (1, 1, 3));
}

#[test]
fn usernames_from_email_fit_the_login_rules() {
    assert_eq!(username_from_email("Ada.Lovelace+x@example.com"), "ada-lovelace-x");
    assert_eq!(username_from_email("a@example.com"), "user-a");
    assert_eq!(username_from_email("..@example.com"), "user");
    let long = format!("{}@example.com", "z".repeat(90));
    let u = username_from_email(&long);
    assert!(normalize_username(&format!("{u}-99")).is_ok(), "room for a suffix: {u}");
}

#[test]
fn an_imported_bcrypt_user_signs_in_keeps_its_id_and_is_rehashed() {
    let _home = crate::test_env::TempHome::new();
    let id = "5f0c9c1e-1d2b-4a3c-9e8f-0123456789ab";
    let hash = bcrypt_of("old-supabase-pw", bcrypt::Version::TwoA);
    let (r, bad) = rows(&[supa_user(id, "ada@example.com", &hash)]);
    let dry = import(&r, &bad, opts(false)).expect("dry run");
    assert_eq!((dry["new"].as_u64(), dry["written"].as_bool()), (Some(1), Some(false)), "{dry}");
    assert!(skin::list_principals().unwrap().is_empty(), "a dry run writes nothing");

    let done = import(&r, &bad, opts(true)).expect("apply");
    assert_eq!(done["written"], true, "{done}");
    assert_eq!(done["derivedUsernames"][0]["username"], "ada", "{done}");
    let p = skin::list_principals().unwrap().into_iter().find(|p| p.email.as_deref() == Some("ada@example.com")).expect("imported");
    assert_eq!(p.id, id, "the given id is the principal id");
    assert_eq!(p.created_at, 1_709_287_200, "created_at carried");
    let (app_meta, user_meta) = metadata_of(id).unwrap();
    assert!(app_meta.is_some_and(|m| m.contains("acme")) && user_meta.is_some_and(|m| m.contains("Ada Example")));
    assert!(is_bcrypt(&stored_hash_for_tests("ada").unwrap()), "stored as given until the first sign-in");

    assert!(matches!(login("ada@example.com", "wrong-pw"), SkinLoginOutcome::BadCreds));
    assert!(is_bcrypt(&stored_hash_for_tests("ada").unwrap()), "a failed sign-in rehashes nothing");
    skin::clear_login_lockout("ada").unwrap();

    let (outcome, kept) = skin::check_and_record_login_hashed("ada@example.com", "old-supabase-pw");
    match outcome {
        SkinLoginOutcome::Ok(p) => assert_eq!(p.id, id, "signed in as the imported id"),
        other => panic!("an imported user who chose their password signs straight in: {other:?}"),
    }
    let now_stored = stored_hash_for_tests("ada").unwrap();
    assert!(now_stored.starts_with("$argon2"), "rehashed to Argon2: {now_stored}");
    assert_eq!(kept.as_deref(), Some(now_stored.as_str()), "the door keeps the hash the login now stands on");
    assert!(matches!(login("ada", "old-supabase-pw"), SkinLoginOutcome::Ok(_)), "and the same password still works, by username too");
    assert!(matches!(login("ada", "wrong-pw"), SkinLoginOutcome::BadCreds));
}

#[test]
fn a_rerun_updates_changed_rows_keeps_a_k2_password_and_adds_new_ones() {
    let _home = crate::test_env::TempHome::new();
    let (a, b) = ("11111111-2222-4333-8444-555555555551", "11111111-2222-4333-8444-555555555552");
    let first = bcrypt_of("first-pw", bcrypt::Version::TwoB);
    let (r, bad) = rows(&[supa_user(a, "a@example.com", &first)]);
    import(&r, &bad, opts(true)).expect("first import");
    assert!(matches!(login("a@example.com", "first-pw"), SkinLoginOutcome::Ok(_)), "signs in, rehashed");

    // At cut-over: the same person (their K2 password now newer), one changed
    // metadata, one new person.
    let mut changed = supa_user(a, "a@example.com", &first);
    changed["raw_user_meta_data"] = json!({"full_name": "Ada Renamed"});
    let second = bcrypt_of("bee-pw", bcrypt::Version::TwoY);
    let (r, bad) = rows(&[changed, supa_user(b, "bee@example.com", &second)]);
    let rep = import(&r, &bad, opts(true)).expect("rerun");
    assert_eq!((rep["updated"].as_u64(), rep["new"].as_u64(), rep["keptK2Password"].as_u64()), (Some(1), Some(1), Some(1)), "{rep}");
    assert!(metadata_of(a).unwrap().1.unwrap().contains("Ada Renamed"));
    assert!(stored_hash_for_tests("user-a").unwrap().starts_with("$argon2"), "the K2 password wins over the old bcrypt");
    assert!(matches!(login("a@example.com", "first-pw"), SkinLoginOutcome::Ok(_)));
    assert!(matches!(login("bee@example.com", "bee-pw"), SkinLoginOutcome::Ok(_)), "a $2y$ hash signs in");

    let (r, bad) = rows(&[supa_user(b, "bee@example.com", &second)]);
    let again = import(&r, &bad, opts(true)).expect("third run");
    assert_eq!((again["unchanged"].as_u64(), again["updated"].as_u64()), (Some(1), Some(0)), "{again}");
}

#[test]
fn clashes_stop_an_apply_and_write_nothing() {
    let _home = crate::test_env::TempHome::new();
    skin::add_principal_with_email("someone", Some("taken@example.com")).unwrap();
    let h = bcrypt_of("pw", bcrypt::Version::TwoA);
    let (r, bad) = rows(&[
        supa_user("22222222-2222-4333-8444-555555555551", "taken@example.com", &h),
        supa_user("22222222-2222-4333-8444-555555555552", "fresh@example.com", &h),
        supa_user("22222222-2222-4333-8444-555555555552", "dupe-id@example.com", &h),
        json!({"id": "22222222-2222-4333-8444-555555555553", "email": "u@example.com", "username": "someone"}),
    ]);
    let dry = import(&r, &bad, opts(false)).expect("a dry run lists clashes");
    let reasons: Vec<String> = dry["clashes"].as_array().unwrap().iter().map(|c| c["reason"].as_str().unwrap().to_string()).collect();
    assert_eq!(reasons.len(), 3, "{dry}");
    assert!(reasons[0].starts_with("email_taken") && reasons[1].contains("same id") && reasons[2].starts_with("username_taken"), "{reasons:?}");
    let refused = import(&r, &bad, opts(true)).expect_err("apply refuses");
    assert_eq!(refused["error"]["code"], CLASHES);
    assert_eq!(refused["written"], false);
    assert_eq!(skin::list_principals().unwrap().len(), 1, "nothing written");
}

#[test]
fn derived_names_dedupe_bans_turn_the_grant_off_and_agents_stay_in_their_workspace() {
    let _home = crate::test_env::TempHome::new();
    let mut banned = json!({"id": "33333333-2222-4333-8444-555555555552", "email": "sam@two.example.com", "banned_until": "2999-01-01T00:00:00Z"});
    banned["encrypted_password"] = json!(bcrypt_of("pw", bcrypt::Version::TwoA));
    let (r, bad) = rows(&[json!({"id": "33333333-2222-4333-8444-555555555551", "email": "sam@one.example.com"}), banned]);
    let rep = import(&r, &bad, opts(true)).expect("import");
    assert_eq!(rep["derivedUsernames"][1]["username"], "sam-2", "{rep}");
    assert_eq!((rep["passwordless"].as_u64(), rep["disabled"].as_u64()), (Some(1), Some(1)));
    let grants = skin::list_grants().unwrap();
    let g = |sub: &str| grants.iter().find(|g| g.subject_id == sub && g.target_id == APP).expect("an App grant").enabled;
    assert!(g("33333333-2222-4333-8444-555555555551"));
    assert!(!g("33333333-2222-4333-8444-555555555552"), "banned: the grant is off");

    // An agent may import new logins (stamped and tied to it) but not touch
    // the owner's.
    let agent = Options { agent_workspace: Some("ws-agent"), actor: "agent:bot", reserved: &[], ..opts(true) };
    let (r, bad) = rows(&[json!({"id": "33333333-2222-4333-8444-555555555551", "email": "sam@one.example.com"})]);
    let refused = import(&r, &bad, agent).expect_err("the owner's login");
    assert!(refused["clashes"][0]["reason"].as_str().unwrap().starts_with("outside_workspace"), "{refused}");
    let reserved = vec!["boss".to_string()];
    let agent = Options { reserved: &reserved, ..agent };
    let (r, bad) = rows(&[
        json!({"id": "33333333-2222-4333-8444-555555555553", "email": "new@example.com"}),
        json!({"id": "33333333-2222-4333-8444-555555555554", "email": "boss@example.com"}),
    ]);
    let rep = import(&r, &bad, agent).expect("an agent's new logins");
    assert_eq!(rep["derivedUsernames"][1]["username"], "boss-2", "a reserved name is never given: {rep}");
    assert_eq!(skin::creators::workspace_of("33333333-2222-4333-8444-555555555553").unwrap().as_deref(), Some("ws-agent"));
    let rerun = import(&r, &bad, agent).expect("and may rerun them");
    assert_eq!(rerun["unchanged"].as_u64(), Some(2), "{rerun}");
}
