//! App-governed Spaces S1/S2/S6 core: the chat key record, its Sew
//! settings (I7), revoke and its `app_id` session revoke (I8), and the
//! keyed pass gate (no per-person grant, the member ceiling, provision
//! first).

use super::*;
use crate::skin::add_principal_for_app;
use crate::tool_hook::{allow_target_off_linux_for_tests, keyed_chat_gate, set_target, KeyedChatGate, CHAT_KIND};

struct Fx {
    _home: crate::test_env::TempHome,
    _db: crate::db::TestDbGuard,
    app: String,
}

fn fx(tag: &str, flags: &[&str]) -> Fx {
    let home = crate::test_env::TempHome::new();
    let db = crate::db::scoped_for_test();
    allow_target_off_linux_for_tests(true);
    let app = {
        let h = crate::db::shared();
        let conn = h.lock();
        let ws = format!("ck-{tag}");
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![ws, format!("/tmp/ck-{tag}")]).unwrap();
        crate::published_services::insert(&conn, &ws, "jobs", "node s.js", "/tmp", 4310, "local", "running", "cmd", "").unwrap().id
    };
    set_target(CHAT_KIND, true, Some("/tmp/k2-test-sew.sock"), Some("/tmp/k2-test-sew.token"), "owner-token").expect("enable chat");
    crate::tool_hook::note_features_for_tests(flags).unwrap();
    Fx { _home: home, _db: db, app }
}

fn want(caps: &[&str], level: Level, uploads: bool, max: u32) -> Want {
    Want { caps: caps.iter().map(|c| c.to_string()).collect(), max_level: level, uploads, max_members: max }
}

fn outbox(subject: &str) -> Vec<(String, String, String)> {
    with_conn(|conn| {
        let mut st = conn
            .prepare("SELECT op, state, payload FROM tool_outbox WHERE subject_key = ?1 ORDER BY seq")
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map(params![subject], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string());
        rows
    })
    .unwrap()
}

fn settings_body(app: &str) -> serde_json::Value {
    crate::tool_hook::home_settings()
        .unwrap()
        .into_iter()
        .find(|s| s.subject == format!("app:{app}"))
        .unwrap_or_else(|| panic!("no Sew settings row for {app}"))
        .body
}

#[test]
fn a_key_is_stored_with_its_ceiling_and_its_sew_settings() {
    let f = fx("set", &["app_governed_spaces"]);
    assert_eq!(get(&f.app).unwrap(), None);
    assert_eq!(enabled(&f.app).unwrap(), None);
    let (before, k) = set(&f.app, &want(&["voice:join", "chat:read", "chat:read"], Level::Moderator, true, 500), "owner").unwrap();
    assert_eq!(before, None);
    assert!(k.enabled);
    assert_eq!(k.caps, vec!["chat:read".to_string(), "voice:join".to_string()], "canonical order, de-duplicated");
    assert_eq!((k.max_level, k.uploads, k.max_members, k.granted_by.as_str()), (Level::Moderator, true, 500, "owner"));
    assert_eq!(enabled(&f.app).unwrap().as_ref(), Some(&k));
    // I7: the App's settings row holds the ceiling for Sew.
    assert_eq!(settings_body(&f.app), json!({"max_level": "moderator", "uploads": true}));
    // A narrowing replaces it in place.
    let (before, k2) = set(&f.app, &want(&["chat:read"], Level::Member, false, 10), "admin").unwrap();
    assert_eq!(before.as_ref(), Some(&k));
    assert_eq!((k2.caps.clone(), k2.max_level, k2.uploads, k2.max_members), (vec!["chat:read".to_string()], Level::Member, false, 10));
    assert_eq!(settings_body(&f.app), json!({"max_level": "member", "uploads": false}));
    let rows = audit_rows(&f.app, 10).unwrap();
    assert_eq!(rows.iter().filter(|r| r.event == AUDIT_SET).count(), 2, "{rows:?}");
}

#[test]
fn bad_keys_are_refused_and_host_is_no_app() {
    let f = fx("bad", &["app_governed_spaces"]);
    let e = set(&f.app, &want(&[], Level::Member, true, 5), "owner").expect_err("no caps");
    assert!(e.contains("at least one chat cap"), "{e}");
    let e = set(&f.app, &want(&["files:read"], Level::Member, true, 5), "owner").expect_err("not a chat cap");
    assert!(e.contains("'files:read' is not a chat cap"), "{e}");
    let e = set(&f.app, &want(&["chat:read"], Level::Member, true, 0), "owner").expect_err("max 0");
    assert!(e.contains("max is 1..=10000"), "{e}");
    let e = set(&f.app, &want(&["chat:read"], Level::Member, true, MAX_MAX_MEMBERS + 1), "owner").expect_err("max too big");
    assert!(e.contains("max is 1..=10000"), "{e}");
    let e = set(HOST_APP_ID, &want(&["chat:read"], Level::Member, true, 5), "owner").expect_err("host");
    assert!(e.contains("host is the box login"), "{e}");
    assert_eq!(get(&f.app).unwrap(), None, "nothing written");
}

#[test]
fn widening_is_any_more_than_before() {
    let on = ChatKey {
        service_id: "s".into(),
        kind: KIND,
        enabled: true,
        caps: vec!["chat:read".into(), "chat:post".into()],
        max_level: Level::Moderator,
        uploads: false,
        max_members: 100,
        granted_by: "owner".into(),
        granted_at: 1,
        revoked_at: None,
    };
    let same = want(&["chat:read", "chat:post"], Level::Moderator, false, 100);
    assert!(widens(None, &same), "turning on widens");
    assert!(widens(Some(&ChatKey { enabled: false, ..on.clone() }), &same), "turning back on widens");
    assert!(!widens(Some(&on), &same), "the same is no widening");
    assert!(!widens(Some(&on), &want(&["chat:read"], Level::Member, false, 50)), "narrowing");
    assert!(widens(Some(&on), &want(&["chat:read", "voice:join"], Level::Moderator, false, 100)), "a new cap");
    assert!(widens(Some(&on), &want(&["chat:read"], Level::Admin, false, 100)), "a higher level");
    assert!(widens(Some(&on), &want(&["chat:read"], Level::Member, false, 101)), "a higher ceiling");
    assert!(widens(Some(&on), &want(&["chat:read"], Level::Member, true, 100)), "uploads on");
    assert!(Level::Member < Level::Moderator && Level::Moderator < Level::Admin);
    assert_eq!(Level::parse(" Admin "), Some(Level::Admin));
    assert_eq!(Level::parse("owner"), None);
}

#[test]
fn revoke_turns_the_key_off_and_ends_the_apps_sessions_by_app_id() {
    let f = fx("rev", &["app_governed_spaces"]);
    set(&f.app, &want(&["chat:read"], Level::Moderator, true, 5), "owner").unwrap();
    let r = revoke(&f.app, "owner").unwrap();
    assert!(r.before.as_ref().is_some_and(|b| b.enabled));
    assert!(r.sessions_revoke_queued);
    let k = get(&f.app).unwrap().expect("the row stays");
    assert!(!k.enabled && k.revoked_at.is_some());
    assert_eq!(enabled(&f.app).unwrap(), None);
    let rows = outbox(&format!("revoke:app:{}", f.app));
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "sew_session_revoke");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&rows[0].2).unwrap(), json!({"app_id": f.app}));
    // Revoking again keeps the first revoke time.
    let at = k.revoked_at;
    revoke(&f.app, "owner").unwrap();
    assert_eq!(get(&f.app).unwrap().unwrap().revoked_at, at);
    // An App that never had a key: nothing to do, nothing queued.
    let none = revoke("00000000-0000-4000-8000-000000000000", "owner").unwrap();
    assert_eq!(none, Revoked { before: None, sessions_revoke_queued: false });
}

#[test]
fn without_the_flag_no_app_id_revoke_is_sent_and_no_ceiling_goes_to_sew() {
    let f = fx("noflag", &["home_client", "app_origin"]);
    set(&f.app, &want(&["chat:read"], Level::Moderator, true, 5), "owner").unwrap();
    let plan = crate::skin::with_skin_conn(|c| crate::tool_hook::home::plan(c, &Default::default())).unwrap();
    assert!(plan.iter().all(|o| o.subject_key != format!("app:{}", f.app)), "max_level/uploads wait for app_governed_spaces: {plan:?}");
    let r = revoke(&f.app, "owner").unwrap();
    assert!(!r.sessions_revoke_queued);
    assert!(outbox(&format!("revoke:app:{}", f.app)).is_empty());
}

#[test]
fn the_sweep_sends_the_ceiling_once_sew_has_the_flag() {
    let f = fx("plan", &["app_governed_spaces"]);
    set(&f.app, &want(&["chat:read"], Level::Admin, false, 5), "owner").unwrap();
    let plan = crate::skin::with_skin_conn(|c| crate::tool_hook::home::plan(c, &Default::default())).unwrap();
    let op = plan.iter().find(|o| o.subject_key == format!("app:{}", f.app)).expect("an app_settings op");
    assert_eq!(op.payload, json!({"max_level": "admin", "uploads": false}));
}

#[test]
fn removing_the_app_revokes_its_key_and_its_sessions() {
    let f = fx("rm", &["app_governed_spaces"]);
    set(&f.app, &want(&["chat:read"], Level::Moderator, true, 5), "owner").unwrap();
    crate::skin::app_chat::app_removed(&f.app, "system:app-removed").unwrap();
    assert!(!get(&f.app).unwrap().unwrap().enabled);
    assert_eq!(outbox(&format!("revoke:app:{}", f.app)).len(), 1);
    // A keyless App's removal also ends its sessions (I8) on such a Sew.
    let other = {
        let h = crate::db::shared();
        let conn = h.lock();
        crate::published_services::insert(&conn, "ck-rm", "other", "node s.js", "/tmp", 4311, "local", "running", "cmd", "").unwrap().id
    };
    crate::skin::app_chat::app_removed(&other, "system:app-removed").unwrap();
    assert_eq!(outbox(&format!("revoke:app:{other}")).len(), 1);
}

#[test]
fn the_keyed_gate_writes_the_grant_then_waits_for_the_member() {
    let f = fx("gate", &["app_governed_spaces"]);
    let dana = add_principal_for_app("dana", None, &f.app, None, "test").unwrap();
    // No key: the gate never writes a grant, and without one of her own
    // dana has no chat (today's rule).
    assert_eq!(keyed_chat_gate(&dana.id, &f.app, "dana").unwrap(), KeyedChatGate::NoGrant);
    assert!(with_conn(|c| crate::tool_hook::chat::person_grant(c, &dana.id)).unwrap().is_none());

    set(&f.app, &want(&["chat:read"], Level::Moderator, true, 5), "owner").unwrap();
    assert_eq!(keyed_chat_gate(&dana.id, &f.app, "dana").unwrap(), KeyedChatGate::Pending);
    let g = with_conn(|c| crate::tool_hook::chat::person_grant(c, &dana.id)).unwrap().expect("the key's grant");
    assert_eq!((g.enabled, g.scope.as_deref(), g.created_by.as_deref()), (true, Some("member"), Some(marker(&f.app).as_str())));
    assert_eq!(with_conn(|c| members_made(c, &f.app)).unwrap(), 1);
    let subject = format!("k2:principal:{}", dana.id);
    let rows = outbox(&subject);
    assert_eq!(rows.iter().map(|r| (r.0.as_str(), r.1.as_str())).collect::<Vec<_>>(), vec![("provision", "pending")]);
    // Still queued: pending, no second row.
    assert_eq!(keyed_chat_gate(&dana.id, &f.app, "dana").unwrap(), KeyedChatGate::Pending);
    assert_eq!(outbox(&subject).len(), 1);
    // Sew confirmed it: ready.
    with_conn(|c| c.execute("UPDATE tool_outbox SET state = 'done' WHERE subject_key = ?1", params![subject]).map_err(|e| e.to_string())).unwrap();
    assert_eq!(keyed_chat_gate(&dana.id, &f.app, "dana").unwrap(), KeyedChatGate::Ready);
    assert!(principal_has_counting_chat_grant(&dana.id).unwrap());

    // Revoked: the key's grant no longer counts for a pass, and the gate
    // says so.
    revoke(&f.app, "owner").unwrap();
    assert!(!principal_has_counting_chat_grant(&dana.id).unwrap());
    assert_eq!(keyed_chat_gate(&dana.id, &f.app, "dana").unwrap(), KeyedChatGate::NoGrant);
    // An owner-made grant on a keyless App: today's path (NotKeyed).
    let eve = add_principal_for_app("eve", None, &f.app, None, "test").unwrap();
    crate::skin::insert_chat_grant_for_tests(&eve.id).unwrap();
    assert_eq!(keyed_chat_gate(&eve.id, &f.app, "eve").unwrap(), KeyedChatGate::NotKeyed);
}

#[test]
fn the_keyed_gate_keeps_the_member_ceiling_and_an_off_grant_off() {
    let f = fx("cap", &["app_governed_spaces"]);
    let a = add_principal_for_app("ana", None, &f.app, None, "test").unwrap();
    let b = add_principal_for_app("bo", None, &f.app, None, "test").unwrap();
    set(&f.app, &want(&["chat:read"], Level::Member, true, 1), "owner").unwrap();
    assert_eq!(keyed_chat_gate(&a.id, &f.app, "ana").unwrap(), KeyedChatGate::Pending);
    assert_eq!(keyed_chat_gate(&b.id, &f.app, "bo").unwrap(), KeyedChatGate::Limit(1));
    assert!(with_conn(|c| crate::tool_hook::chat::person_grant(c, &b.id)).unwrap().is_none(), "no grant past the ceiling");
    // Someone switched ana's grant off: the key doesn't switch it back on.
    let g = with_conn(|c| crate::tool_hook::chat::person_grant(c, &a.id)).unwrap().unwrap();
    crate::skin::set_grant_enabled_as(&g.id, false, "owner-token").unwrap();
    assert_eq!(keyed_chat_gate(&a.id, &f.app, "ana").unwrap(), KeyedChatGate::Disabled);
    assert!(!with_conn(|c| crate::tool_hook::chat::person_grant(c, &a.id)).unwrap().unwrap().enabled);
    // An owner-made grant counts for a pass whatever the key does.
    crate::skin::insert_chat_grant_for_tests(&b.id).unwrap();
    assert!(principal_has_counting_chat_grant(&b.id).unwrap());
}

#[test]
fn only_this_apps_own_people_are_keyed_eligible() {
    let f = fx("elig", &["app_governed_spaces"]);
    let dana = add_principal_for_app("dana", None, &f.app, None, "test").unwrap();
    assert!(keyed_eligible(&dana.id, &f.app).unwrap());
    // Another App's user, an unknown id: no.
    assert!(!keyed_eligible(&dana.id, "00000000-0000-4000-8000-000000000000").unwrap());
    assert!(!keyed_eligible("00000000-0000-4000-8000-00000000abcd", &f.app).unwrap());
    // A login marked as an agent's: never a person.
    let bot = add_principal_for_app("bot", None, &f.app, None, "test").unwrap();
    with_conn(|c| c.execute("UPDATE principals SET agent_workspace = 'ws-x' WHERE id = ?1", params![bot.id]).map_err(|e| e.to_string())).unwrap();
    assert!(!keyed_eligible(&bot.id, &f.app).unwrap());
    // A person who also holds the box login keeps today's rule.
    let both = add_principal_for_app("both", None, &f.app, None, "test").unwrap();
    with_conn(|c| {
        c.execute(
            "INSERT INTO grants (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by)
             VALUES ('g-host-both', 'principal', ?1, 'app', 'host', NULL, NULL, 1, 1, 'test')",
            params![both.id],
        )
        .map_err(|e| e.to_string())
    })
    .unwrap();
    assert!(!keyed_eligible(&both.id, &f.app).unwrap());
    // A disabled login on this App: no.
    let gone = add_principal_for_app("gone", None, &f.app, None, "test").unwrap();
    with_conn(|c| {
        c.execute("UPDATE grants SET enabled = 0 WHERE subject_id = ?1 AND kind = 'app'", params![gone.id]).map_err(|e| e.to_string())
    })
    .unwrap();
    assert!(!keyed_eligible(&gone.id, &f.app).unwrap());
}
