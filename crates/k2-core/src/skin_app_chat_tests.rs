//! Sew for Apps A2 core rules: own guests only, own grants only, the
//! member ceiling, never stranding a shared person, audited, idempotent.

use super::*;
use crate::skin::{add_principal, add_principal_for_app, list_grants};
use crate::tool_hook::{allow_target_off_linux_for_tests, set_target, CHAT_KIND};

struct Fx {
    _home: crate::test_env::TempHome,
    _db: crate::db::TestDbGuard,
    app: String,
    other: String,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_env::TempHome::new();
    let db = crate::db::scoped_for_test();
    allow_target_off_linux_for_tests(true);
    let (app, other) = {
        let h = crate::db::shared();
        let conn = h.lock();
        for r in [format!("own-{tag}"), format!("oth-{tag}")] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![r, format!("/tmp/gc-{r}")])
                .unwrap();
        }
        let a = crate::published_services::insert(&conn, &format!("own-{tag}"), "jobs", "node s.js", "/tmp", 4300, "local", "running", "cmd", "")
            .unwrap();
        let o = crate::published_services::insert(&conn, &format!("oth-{tag}"), "other", "node s.js", "/tmp", 4301, "local", "running", "cmd", "")
            .unwrap();
        (a.id, o.id)
    };
    set_target(CHAT_KIND, true, Some("/tmp/k2-test-sew.sock"), Some("/tmp/k2-test-sew.token"), "owner-token").expect("enable chat");
    // The safety gate's flag (its own test turns it off).
    crate::tool_hook::note_features_for_tests(&["app_scoped_sessions"]).unwrap();
    Fx { _home: home, _db: db, app, other }
}

fn chat_grant_of(username: &str) -> Option<crate::skin::SkinGrant> {
    let p = crate::skin::list_principals().unwrap().into_iter().find(|p| p.username == username)?;
    list_grants().unwrap().into_iter().find(|g| g.kind == CHAT_KIND && g.subject_id == p.id)
}

fn set_app(f: &Fx, user: &str, scope: Scope) -> Result<GuestChat, Refusal> {
    set(&Caller::App, Some(&f.app), user, scope, &format!("app:{}", f.app), "app")
}

#[test]
fn app_gives_and_takes_its_own_guests_chat_under_the_grant() {
    let f = fx("give");
    add_principal_for_app("dana", None, &f.app, None, "test").unwrap();
    assert_eq!(get_grant(&f.app).unwrap(), None);
    let (before, g) = grant(&f.app, None, "owner-token").unwrap();
    assert!(before.is_none());
    assert_eq!(g.max_members, DEFAULT_MAX_MEMBERS);

    let r = read(&f.app, "dana").unwrap();
    assert_eq!((r.scope.clone(), r.enabled, r.managed_by.clone(), r.changed), (None, false, None, false));
    let out = set_app(&f, "dana", Scope::Member).unwrap();
    assert!(out.changed);
    assert_eq!(out.scope.as_deref(), Some("member"));
    assert_eq!(out.managed_by.as_deref(), Some("app"));
    assert_eq!((out.members, out.max_members), (Some(1), Some(DEFAULT_MAX_MEMBERS)));
    let g = chat_grant_of("dana").expect("a chat grant");
    assert_eq!(g.created_by.as_deref(), Some(app_marker(&f.app).as_str()), "the App's own grant");
    assert_eq!(g.scope.as_deref(), Some("member"));
    // The same scope again writes nothing.
    assert!(!set_app(&f, "dana", Scope::Member).unwrap().changed);
    // member → guest → none.
    assert_eq!(set_app(&f, "dana", Scope::Guest).unwrap().scope.as_deref(), Some("guest"));
    let off = set_app(&f, "dana", Scope::None).unwrap();
    assert!(off.changed && off.scope.is_none() && off.managed_by.is_none());
    assert!(chat_grant_of("dana").is_none());
    assert!(!set_app(&f, "dana", Scope::None).unwrap().changed, "none twice is a no-op");

    // Each change is one audit row, with the scope after it.
    let rows = audit_rows(&f.app, 10).unwrap();
    let changes: Vec<_> = rows.iter().filter(|r| r.event == AUDIT_CHANGE).collect();
    assert_eq!(changes.len(), 3, "{rows:?}");
    assert!(rows.iter().any(|r| r.event == AUDIT_GRANT));
}

#[test]
fn only_this_apps_guests_and_never_an_agent_login_or_a_login() {
    let f = fx("own");
    grant(&f.app, None, "owner-token").unwrap();
    add_principal_for_app("eli", None, &f.other, None, "test").unwrap();
    add_principal("fay").unwrap();
    for u in ["eli", "fay", "nobody", "x"] {
        assert_eq!(set_app(&f, u, Scope::Member), Err(Refusal::NotFound), "{u}");
        assert_eq!(read(&f.app, u), Err(Refusal::NotFound), "{u}");
    }
    assert!(chat_grant_of("eli").is_none() && chat_grant_of("fay").is_none());
    // The host is no App.
    assert_eq!(set(&Caller::App, Some(HOST_APP_ID), "fay", Scope::Member, "x", "app"), Err(Refusal::NotFound));
}

/// App-governed Spaces S0: an App user whose name is also a Connect login
/// (the owner's or an Admin's) is a separate account and gets chat as its
/// own principal. A Connect login itself is still nobody here.
#[test]
fn an_app_user_named_like_a_box_login_gets_chat_as_itself_s0() {
    let f = fx("s0");
    grant(&f.app, None, "owner-token").unwrap();
    crate::connect_users::add_user("rosson", "password-long-1").unwrap();
    crate::connect_users::set_role("rosson", crate::connect_users::Role::Owner).unwrap();
    crate::connect_users::add_user("ada", "password-long-1").unwrap();
    crate::connect_users::set_role("ada", crate::connect_users::Role::Admin).unwrap();
    let r = add_principal_for_app("rosson", None, &f.app, None, "test").unwrap();
    let out = set_app(&f, "rosson", Scope::Member).expect("the App user named like the owner's login gets chat");
    assert!(out.changed);
    assert_eq!(out.principal_id, r.id, "its own principal, never the box login");
    assert_eq!(out.scope.as_deref(), Some("member"));
    let g = chat_grant_of("rosson").expect("a chat grant");
    assert_eq!((g.subject_kind.as_str(), g.subject_id.as_str()), ("principal", r.id.as_str()));
    assert_eq!(read(&f.app, "rosson").unwrap().principal_id, r.id);
    // The owner's CLI reaches the same App user, never the login.
    assert_eq!(owner_read(None, "rosson").unwrap().principal_id, r.id);
    // A name that is only a Connect login (the Admin) is nobody to the App
    // or the owner's command: no grant is ever written for a box user.
    assert_eq!(set_app(&f, "ada", Scope::Member), Err(Refusal::NotFound));
    assert_eq!(set(&Caller::Owner, None, "ada", Scope::Member, "owner-token", "cli"), Err(Refusal::NotFound));
    assert!(list_grants().unwrap().iter().all(|g| g.subject_kind != "connect"), "no Connect chat grant");
}

#[test]
fn an_owner_grant_is_the_owners_and_an_off_grant_stays_off() {
    let f = fx("owner");
    grant(&f.app, None, "owner-token").unwrap();
    add_principal_for_app("gus", None, &f.app, None, "test").unwrap();
    // The owner gives gus chat (no App in the call).
    let o = set(&Caller::Owner, None, "gus", Scope::Member, "owner-token", "cli").unwrap();
    assert!(o.changed);
    assert_eq!(o.app, None);
    let r = read(&f.app, "gus").unwrap();
    assert_eq!((r.scope.as_deref(), r.managed_by.as_deref()), (Some("member"), Some("owner")));
    // The App may ask for what is there, never change or remove it.
    assert!(!set_app(&f, "gus", Scope::Member).unwrap().changed);
    assert_eq!(set_app(&f, "gus", Scope::Guest), Err(Refusal::NotAppGranted));
    assert_eq!(set_app(&f, "gus", Scope::None), Err(Refusal::NotAppGranted));
    // The owner switches it off: the App can't switch it back on.
    let g = chat_grant_of("gus").unwrap();
    crate::skin::set_grant_enabled_as(&g.id, false, "owner-token").unwrap();
    assert_eq!(set_app(&f, "gus", Scope::Member), Err(Refusal::Disabled));
    // The owner can: the same scope turns it back on.
    assert!(set(&Caller::Owner, Some(&f.app), "gus", Scope::Member, "owner-token", "cli").unwrap().changed);
    assert!(chat_grant_of("gus").unwrap().enabled);
}

#[test]
fn the_member_ceiling_counts_only_this_apps_own_grants() {
    let f = fx("cap");
    grant(&f.app, Some(2), "owner-token").unwrap();
    for u in ["ha", "hb", "hc", "hd"] {
        add_principal_for_app(u, None, &f.app, None, "test").unwrap();
    }
    // An owner-given grant doesn't spend the App's ceiling.
    set(&Caller::Owner, None, "hd", Scope::Member, "owner-token", "cli").unwrap();
    set_app(&f, "ha", Scope::Member).unwrap();
    set_app(&f, "hb", Scope::Guest).unwrap();
    assert_eq!(set_app(&f, "hc", Scope::Member), Err(Refusal::Limit { max: 2 }));
    assert!(chat_grant_of("hc").is_none());
    // A change of scope isn't a new member.
    assert!(set_app(&f, "hb", Scope::Member).unwrap().changed);
    // Freeing one makes room.
    set_app(&f, "ha", Scope::None).unwrap();
    assert!(set_app(&f, "hc", Scope::Member).unwrap().changed);
    // Re-giving the grant raises the ceiling.
    grant(&f.app, Some(3), "owner-token").unwrap();
    assert!(set_app(&f, "ha", Scope::Member).unwrap().changed);
    assert!(grant(&f.app, Some(0), "owner-token").is_err());
    assert!(grant(&f.app, Some(MAX_MAX_MEMBERS + 1), "owner-token").is_err());
    assert!(grant(HOST_APP_ID, None, "owner-token").is_err());
}

#[test]
fn none_never_strands_a_person_another_app_shares() {
    let f = fx("shared");
    grant(&f.app, None, "owner-token").unwrap();
    let p = add_principal_for_app("ivy", None, &f.app, None, "test").unwrap();
    set_app(&f, "ivy", Scope::Member).unwrap();
    crate::skin::create_grant(&crate::skin::SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: p.id.clone(),
        kind: "app".into(),
        target_id: f.other.clone(),
        role: None,
        scope: Some("[]".into()),
        created_by: None,
    })
    .unwrap();
    assert_eq!(set_app(&f, "ivy", Scope::None), Err(Refusal::SharedPerson));
    assert!(chat_grant_of("ivy").is_some(), "kept for the other App");
    // The owner may still take it.
    assert!(set(&Caller::Owner, None, "ivy", Scope::None, "owner-token", "cli").unwrap().changed);
}

#[test]
fn an_unusable_username_and_chat_off_are_loud() {
    let f = fx("loud");
    grant(&f.app, None, "owner-token").unwrap();
    add_principal_for_app("all", None, &f.app, None, "test").unwrap();
    match set_app(&f, "all", Scope::Member) {
        Err(Refusal::Unusable(m)) => assert!(m.contains("reserved chat mention"), "{m}"),
        other => panic!("{other:?}"),
    }
    add_principal_for_app("jo", None, &f.app, None, "test").unwrap();
    set_target(CHAT_KIND, false, None, None, "owner-token").unwrap();
    assert_eq!(set_app(&f, "jo", Scope::Member), Err(Refusal::ChatOff));
    assert_eq!(Refusal::ChatOff.status(), 409);
    assert_eq!(Refusal::Limit { max: 1 }.code(), "chat_limit");
    assert_eq!(Scope::parse(" Member "), Some(Scope::Member));
    assert_eq!(Scope::parse("off"), Some(Scope::None));
    assert_eq!(Scope::parse("admin"), None);
}

/// The safety gate (Sew 2026-10-28): without `app_scoped_sessions` an App
/// can't give chat (taking it away still works), the owner can, and every
/// answer naming the App warns that it sees all the person's chat.
#[test]
fn without_app_scoped_sessions_only_the_owner_gives_chat() {
    let f = fx("gate");
    grant(&f.app, None, "owner-token").unwrap();
    add_principal_for_app("lu", None, &f.app, None, "test").unwrap();
    add_principal_for_app("mo", None, &f.app, None, "test").unwrap();
    set_app(&f, "mo", Scope::Member).unwrap();
    crate::tool_hook::note_features_for_tests(&["home_client"]).unwrap();
    assert_eq!(set_app(&f, "lu", Scope::Member), Err(Refusal::NotAppScoped));
    assert_eq!(Refusal::NotAppScoped.code(), "chat_not_app_scoped");
    assert!(chat_grant_of("lu").is_none());
    let r = read(&f.app, "mo").unwrap();
    assert!(!r.app_scoped);
    assert_eq!(r.warning.as_deref(), Some(WARN_UNSCOPED));
    assert!(WARN_UNSCOPED.contains("this App can see all chat this person can"));
    assert!(set_app(&f, "mo", Scope::None).unwrap().changed, "taking chat away is always allowed");
    let o = set(&Caller::Owner, Some(&f.app), "lu", Scope::Member, "owner-token", "cli").unwrap();
    assert!(o.changed && o.warning.is_some());
    let none = set(&Caller::Owner, None, "lu", Scope::Guest, "owner-token", "cli").unwrap();
    assert_eq!(none.warning, None, "no App named: nothing to warn about");
}

#[test]
fn revoking_the_grant_keeps_peoples_chat_but_stops_the_app() {
    let f = fx("revoke");
    grant(&f.app, None, "owner-token").unwrap();
    add_principal_for_app("kim", None, &f.app, None, "test").unwrap();
    set_app(&f, "kim", Scope::Member).unwrap();
    assert!(revoke(&f.app, "owner-token").unwrap().is_some());
    assert!(chat_grant_of("kim").is_some(), "people keep chat");
    assert_eq!(get_grant(&f.app).unwrap(), None);
    assert!(revoke(&f.app, "owner-token").unwrap().is_none(), "twice is a no-op");
}
