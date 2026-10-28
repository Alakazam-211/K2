//! Sew for Apps A9 core rules: the args, the gates, the App's own Space,
//! who of the listed users goes in (and why not), the ledger after Sew's
//! members/add answer, and how the create answer maps back.

use super::*;
use crate::skin::{add_principal_for_app, create_grant, SkinGrantWrite};
use crate::tool_hook::sew_wire::{SpaceMemberRef, SpaceNotAdded};
use crate::tool_hook::{allow_target_off_linux_for_tests, set_target};
use rusqlite::params;
use serde_json::json;

const SPACE: &str = "48831765147549697";
const OTHER_SPACE: &str = "48831765147549698";

struct Fx {
    _home: crate::test_env::TempHome,
    _db: crate::db::TestDbGuard,
    app: String,
    other: String,
}

fn fx(tag: &str, flags: &[&str]) -> Fx {
    let home = crate::test_env::TempHome::new();
    let db = crate::db::scoped_for_test();
    allow_target_off_linux_for_tests(true);
    let (app, other) = {
        let h = crate::db::shared();
        let conn = h.lock();
        for r in [format!("own-{tag}"), format!("oth-{tag}")] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![r, format!("/tmp/cc-{r}")])
                .unwrap();
        }
        let a = crate::published_services::insert(&conn, &format!("own-{tag}"), "jobs", "node s.js", "/tmp", 4310, "local", "running", "cmd", "")
            .unwrap();
        let o = crate::published_services::insert(&conn, &format!("oth-{tag}"), "other", "node s.js", "/tmp", 4311, "local", "running", "cmd", "")
            .unwrap();
        (a.id, o.id)
    };
    set_target(CHAT_KIND, true, Some("/tmp/k2-test-sew.sock"), Some("/tmp/k2-test-sew.token"), "owner-token").expect("enable chat");
    crate::tool_hook::note_features_for_tests(flags).unwrap();
    Fx { _home: home, _db: db, app, other }
}

fn chat(principal_id: &str, scope: &str) {
    create_grant(&SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: principal_id.into(),
        kind: "chat".into(),
        target_id: String::new(),
        role: None,
        scope: Some(scope.into()),
        created_by: Some("owner-token".into()),
    })
    .expect("chat grant");
}

fn bind(app: &str, space: &str) {
    crate::tool_hook::app_space_bind(app, space, "owner-token").expect("bind");
}

fn ask(v: serde_json::Value) -> Ask {
    Ask::parse(&v).unwrap_or_else(|e| panic!("{v}: {e:?}"))
}

fn pext(id: &str) -> String {
    format!("k2:principal:{id}")
}

fn names(prep: &Prep, which: &str) -> Vec<String> {
    let v = match which {
        "add" => prep.space_add().map(|r| r.external_ids.iter().map(|e| e.to_string()).collect()).unwrap_or_default(),
        _ => prep.channel_create().members,
    };
    v.into_iter().map(|e| prep.names.get(&e).cloned().unwrap_or_else(|| panic!("unnamed {e}"))).collect()
}

#[test]
fn args_are_checked() {
    let a = ask(json!({"slug": "job-1042", "name": "Job 1042", "private": true, "members": ["Dana", "eli", "dana"]}));
    assert_eq!((a.slug.as_str(), a.name.as_str(), a.private, a.space.as_deref()), ("job-1042", "Job 1042", true, None));
    assert_eq!(a.members, ["Dana", "eli"], "case-insensitive de-dup, order kept");
    assert!(!ask(json!({"slug": "a", "name": "A"})).private, "public by default");
    for bad in [
        json!({"name": "x"}),
        json!({"slug": "Job", "name": "x"}),
        json!({"slug": "-job", "name": "x"}),
        json!({"slug": "job_1", "name": "x"}),
        json!({"slug": "job"}),
        json!({"slug": "job", "name": ""}),
        json!({"slug": "job", "name": "x".repeat(101)}),
        json!({"slug": "job", "name": "x", "private": "yes"}),
        json!({"slug": "job", "name": "x", "members": "dana"}),
        json!({"slug": "job", "name": "x", "members": [1]}),
        json!({"slug": "job", "name": "x", "members": (0..101).map(|i| format!("u{i}")).collect::<Vec<_>>()}),
        json!({"slug": "job", "name": "x", "space": "a b"}),
        json!({"slug": "job", "name": "x", "kind": "voice"}),
        json!(["job"]),
    ] {
        let e = Ask::parse(&bad).expect_err(&bad.to_string());
        assert_eq!(e.code(), "usage", "{bad}: {e:?}");
    }
}

#[test]
fn refused_without_sews_flag_or_chat() {
    let f = fx("flag", &["app_scoped_sessions"]);
    bind(&f.app, SPACE);
    let e = prepare(&f.app, ask(json!({"slug": "job", "name": "Job"}))).unwrap_err();
    assert_eq!((e.status(), e.code()), (409, "sew_feature_missing"), "{e:?}");
    assert!(e.hint().contains("Sew 0.5.0"), "{}", e.hint());

    crate::tool_hook::note_features_for_tests(&["app_channel_create"]).unwrap();
    set_target(CHAT_KIND, false, Some("/tmp/k2-test-sew.sock"), Some("/tmp/k2-test-sew.token"), "owner-token").unwrap();
    let e = prepare(&f.app, ask(json!({"slug": "job", "name": "Job"}))).unwrap_err();
    assert_eq!((e.status(), e.code()), (409, "chat_unavailable"), "{e:?}");
}

#[test]
fn only_the_apps_own_space() {
    let f = fx("space", &["app_channel_create"]);
    let a = ask(json!({"slug": "job", "name": "Job"}));
    let e = prepare(&f.app, a.clone()).unwrap_err();
    assert_eq!((e.status(), e.code()), (409, "space_unbound"), "no Space yet: {e:?}");

    bind(&f.other, OTHER_SPACE);
    let e = prepare(&f.app, ask(json!({"slug": "job", "name": "Job", "space": OTHER_SPACE}))).unwrap_err();
    assert_eq!((e.status(), e.code()), (409, "app_mismatch"), "another App's Space: {e:?}");
    assert!(!e.hint().contains(&f.other), "never names the other App: {}", e.hint());
    let e = prepare(&f.app, ask(json!({"slug": "job", "name": "Job", "space": "999"}))).unwrap_err();
    assert_eq!(e.code(), "space_unbound", "{e:?}");

    bind(&f.app, SPACE);
    let p = prepare(&f.app, a.clone()).unwrap();
    assert_eq!(p.space_id, SPACE, "its only Space by default");
    assert!(p.space_add().is_none());
    assert_eq!(p.channel_create(), sew_wire::ChannelCreateRequest {
        space_id: SPACE.into(),
        app_id: f.app.clone(),
        slug: "job".into(),
        name: "Job".into(),
        private: false,
        members: vec![],
    });
    bind(&f.app, "48831765147549699");
    let e = prepare(&f.app, a).unwrap_err();
    assert_eq!(e.code(), "usage", "two Spaces: name one: {e:?}");
    let p = prepare(&f.app, ask(json!({"slug": "job", "name": "Job", "space": "48831765147549699"}))).unwrap();
    assert_eq!(p.space_id, "48831765147549699");
}

#[test]
fn listed_users_sort_into_add_in_space_skipped_and_not_added() {
    let f = fx("sort", &["app_channel_create"]);
    let dana = add_principal_for_app("dana", None, &f.app, None, "test").unwrap();
    let eli = add_principal_for_app("eli", None, &f.app, None, "test").unwrap();
    let fay = add_principal_for_app("fay", None, &f.app, None, "test").unwrap();
    // gus: this App's user without chat.
    add_principal_for_app("gus", None, &f.app, None, "test").unwrap();
    let hal = add_principal_for_app("hal", None, &f.app, None, "test").unwrap();
    let ivy = add_principal_for_app("ivy", None, &f.other, None, "test").unwrap();
    chat(&dana.id, "member");
    chat(&eli.id, "member");
    chat(&fay.id, "guest");
    chat(&hal.id, "member");
    chat(&ivy.id, "member");
    bind(&f.app, SPACE);
    // eli is in the Space already; hal is banned there.
    crate::skin::with_skin_conn(|conn| {
        let desired = app_space::build_desired(conn)?;
        let w = |id: &str| desired.iter().find(|w| w.ext.to_string() == pext(id) && w.space_id == SPACE).cloned().unwrap();
        app_space::note_direct_add(conn, &w(&eli.id), true, 1)?;
        app_space::note_direct_add(conn, &w(&hal.id), false, 1)
    })
    .unwrap();

    let p = prepare(&f.app, ask(json!({"slug": "job-1", "name": "Job 1", "private": true,
        "members": ["eli", "dana", "fay", "gus", "hal", "ivy", "nobody", "DANA"]})))
    .unwrap();
    assert_eq!(names(&p, "add"), ["dana"], "only the one not yet in the Space");
    assert_eq!(names(&p, "create"), ["eli"], "before the add, only who's in");
    assert_eq!(p.skipped, ["hal"], "banned: never sent");
    let not: Vec<(&str, &str)> = p.not_added.iter().map(|n| (n.username.as_str(), n.reason.as_str())).collect();
    assert_eq!(not, [("fay", "guest"), ("gus", "no_chat"), ("ivy", "not_found"), ("nobody", "not_found")]);
    assert!(!format!("{p:?}").contains(&ivy.id), "another App's user leaves no trace");
}

#[test]
fn space_add_answer_moves_the_ledger_and_maps_back() {
    let f = fx("after", &["app_channel_create"]);
    let mut ids = Vec::new();
    for u in ["ann", "bea", "cal", "dee"] {
        let p = add_principal_for_app(u, None, &f.app, None, "test").unwrap();
        chat(&p.id, "member");
        ids.push(p.id);
    }
    bind(&f.app, SPACE);
    let mut p = prepare(&f.app, ask(json!({"slug": "crew", "name": "Crew", "members": ["dee", "cal", "bea", "ann"]}))).unwrap();
    assert_eq!(names(&p, "add"), ["dee", "cal", "bea", "ann"]);
    let ext = |i: usize| ExternalId::parse(&pext(&ids[i])).unwrap();
    let resp = SpaceMembersAddResponse {
        added: vec![SpaceMemberRef { external_id: ext(0), member_id: "11".into() }],
        already: vec![SpaceMemberRef { external_id: ext(3), member_id: "14".into() }],
        skipped: vec!["12".into()],
        not_added: vec![SpaceNotAdded { external_id: ext(2), reason: "not_provisioned".into() }],
    };
    after_space_add(&mut p, &resp).unwrap();
    assert_eq!(names(&p, "create"), ["dee", "ann"], "the App's order");
    assert_eq!(p.skipped, ["bea"]);
    assert_eq!(p.not_added, [NotAdded { username: "cal".into(), reason: "not_provisioned".into() }]);
    let state = |i: usize| crate::skin::with_skin_conn(|c| app_space::ledger_state(c, SPACE, &pext(&ids[i]))).unwrap();
    assert_eq!(state(0), Some((app_space::ADDED.to_string(), false)));
    assert_eq!(state(3), Some((app_space::ADDED.to_string(), false)));
    assert_eq!(state(1), Some((app_space::HELD.to_string(), true)), "banned: held for good");
    assert_eq!(state(2), Some((app_space::PENDING_ADD.to_string(), false)), "left to the outbox");
    // The next reconcile neither re-adds the banned one nor queues the added.
    let s = crate::skin::with_skin_conn(|c| app_space::reconcile(c, "test", 2)).unwrap();
    assert_eq!(s.adds, 0, "{s:?}");

    // A second call: the in-Space users aren't sent again, the banned one is skipped.
    let p2 = prepare(&f.app, ask(json!({"slug": "crew", "name": "Crew", "members": ["ann", "bea", "dee"]}))).unwrap();
    assert!(p2.space_add().is_none());
    assert_eq!(p2.skipped, ["bea"]);
}

#[test]
fn finish_maps_sews_answer_and_audits() {
    let f = fx("finish", &["app_channel_create"]);
    let mut ids = Vec::new();
    for u in ["ann", "bea", "cal"] {
        let p = add_principal_for_app(u, None, &f.app, None, "test").unwrap();
        chat(&p.id, "member");
        ids.push(p.id);
    }
    bind(&f.app, SPACE);
    crate::skin::with_skin_conn(|conn| {
        for w in app_space::build_desired(conn)? {
            app_space::note_direct_add(conn, &w, true, 1)?;
        }
        Ok(())
    })
    .unwrap();
    let p = prepare(&f.app, ask(json!({"slug": "crew", "name": "Crew", "private": true, "members": ["cal", "bea", "ann", "zed"]}))).unwrap();
    assert_eq!(names(&p, "create"), ["cal", "bea", "ann"]);
    let ext = |i: usize| ExternalId::parse(&pext(&ids[i])).unwrap();
    let resp = ChannelCreateResponse {
        channel_id: "301".into(),
        space_id: SPACE.into(),
        slug: "crew".into(),
        name: "Old crew".into(),
        private: false,
        created: false,
        added: vec![SpaceMemberRef { external_id: ext(0), member_id: "11".into() }],
        already: vec![SpaceMemberRef { external_id: ext(2), member_id: "13".into() }],
        skipped: vec!["12".into()],
        not_added: vec![],
    };
    let out = finish(p, resp, "app:jobs", "app").unwrap();
    assert_eq!(
        serde_json::to_value(&out).unwrap(),
        json!({
            "ok": true, "app": f.app, "spaceId": SPACE, "channelId": "301", "slug": "crew",
            "name": "Old crew", "private": false, "created": false,
            "added": ["cal", "ann"],
            "notAdded": [{"username": "zed", "reason": "not_found"}],
            "skipped": ["bea"],
        }),
        "the current name and visibility; Sew's skipped member ids map back to usernames"
    );
    let rows = audit_rows(&f.app, 5).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].actor, "app:jobs");
    assert_eq!(
        (rows[0].payload["slug"].clone(), rows[0].payload["created"].clone(), rows[0].payload["added"].clone()),
        (json!("crew"), json!(false), json!(2))
    );
}

#[test]
fn chat_channels_is_the_apps_own_cap_held_once() {
    assert!(crate::skin::APP_ONLY_CAPS.contains(&CAP));
    assert!(!crate::skin::is_chat_cap(CAP), "never on a person's chat pass");
    assert!(crate::skin::parse_caps(Some(&[CAP.to_string()])).is_err(), "never a role cap");
    assert_eq!(crate::workspace::settings::app_core_of_cap(CAP), None, "the Sew core doesn't pre-approve it");
    assert_eq!(crate::skin::link::link_caps(&[CAP.to_string()]).0, vec![CAP.to_string()], "an App's pass may carry it");
}
