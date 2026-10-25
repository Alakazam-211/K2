//! RS-A15 core rules: own guests only, the ceiling, never above the role,
//! idempotent, audited before/after, rate-limited per App.

use super::*;
use crate::skin::{add_principal, add_principal_for_app, create_role_for_app, RoomPolicy};

struct Fx {
    _home: crate::test_env::TempHome,
    _db: crate::db::TestDbGuard,
    app: String,
    other_app: String,
    own_room: String,
    room_b: String,
    room_c: String,
    tag: String,
}

fn project(conn: &rusqlite::Connection, id: &str) {
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![id, format!("/tmp/gr-{id}")]).unwrap();
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_env::TempHome::new();
    let db = crate::db::scoped_for_test();
    let (own_room, room_b, room_c) = (format!("own-{tag}"), format!("roomb-{tag}"), format!("roomc-{tag}"));
    let (app, other_app) = {
        let h = crate::db::shared();
        let conn = h.lock();
        for r in [&own_room, &room_b, &room_c] {
            project(&conn, r);
        }
        let a = crate::published_services::insert(&conn, &own_room, "jobs", "node s.js", "/tmp", 4100, "local", "running", "cmd", "")
            .unwrap();
        let o = crate::published_services::insert(&conn, &room_b, "other", "node s.js", "/tmp", 4101, "local", "running", "cmd", "")
            .unwrap();
        (a.id, o.id)
    };
    Fx { _home: home, _db: db, app, other_app, own_room, room_b, room_c, tag: tag.to_string() }
}

fn v(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| x.to_string()).collect()
}

fn sorted(mut x: Vec<String>) -> Vec<String> {
    x.sort();
    x
}

#[test]
fn own_guest_set_and_read_within_the_grant() {
    let f = fx("own");
    add_principal_for_app("dana", None, &f.app, None, "test").unwrap();
    assert_eq!(app_ceiling(&f.app).unwrap(), None, "no grant, no ceiling");
    grant(&f.app, &[f.room_b.clone()], "owner").unwrap();
    let ceiling = Ceiling::Rooms(app_ceiling(&f.app).unwrap().expect("granted"));
    assert_eq!(sorted(match &ceiling {
        Ceiling::Rooms(r) => r.clone(),
        _ => unreachable!(),
    }), sorted(vec![f.room_b.clone(), f.own_room.clone()]), "the grant's rooms plus the App's own");

    let out = set(&f.app, "dana", &[f.room_b.clone(), f.own_room.clone()], &ceiling, "app:jobs", "app").unwrap();
    assert!(out.view.changed);
    assert!(out.before.is_empty());
    assert_eq!(out.view.rooms, vec![f.room_b.clone(), f.own_room.clone()]);
    assert_eq!(out.view.role, None);
    let r = read(&f.app, "DANA").unwrap();
    assert_eq!(sorted(r.rooms.clone()), sorted(vec![f.room_b.clone(), f.own_room.clone()]));
    assert!(!r.changed);
    assert_eq!(r.username, "dana");
    // The live grant is what login reads: Thread-only caps in those rooms.
    let g = crate::skin::list_grants().unwrap().into_iter().find(|g| g.target_id == f.app).unwrap();
    assert_eq!(sorted(crate::skin::parse_rooms_json(g.scope.as_deref())), sorted(vec![f.room_b.clone(), f.own_room.clone()]));
    // Handles work as well as ids; [] takes every room away.
    let out = set(&f.app, "dana", &[], &ceiling, "app:jobs", "app").unwrap();
    assert!(out.view.changed && out.view.rooms.is_empty());
}

#[test]
fn another_apps_guest_a_connect_user_and_nobody_are_the_same_not_found() {
    let f = fx("nf");
    add_principal_for_app("erin", None, &f.other_app, None, "test").unwrap();
    add_principal("boxy").unwrap(); // a principal with no grant on this App
    grant(&f.app, &[], "owner").unwrap();
    let c = Ceiling::Rooms(app_ceiling(&f.app).unwrap().unwrap());
    for who in ["erin", "boxy", "ghost", "", "../x"] {
        assert_eq!(read(&f.app, who).unwrap_err(), Refusal::NotFound, "{who}");
        assert_eq!(set(&f.app, who, &[f.own_room.clone()], &c, "app", "app").unwrap_err(), Refusal::NotFound, "{who}");
    }
    // Even with rooms outside the ceiling, a non-guest is not_found first:
    // the App learns nothing about names that aren't its guests.
    assert_eq!(set(&f.app, "erin", &[f.room_c.clone()], &c, "app", "app").unwrap_err(), Refusal::NotFound);
    // A Connect login sharing a guest's name is never the App's guest.
    add_principal_for_app("ownr", None, &f.app, None, "test").unwrap();
    crate::connect_users::add_user("ownr", "correct horse battery 9").unwrap();
    assert_eq!(read(&f.app, "ownr").unwrap_err(), Refusal::NotFound);
    assert_eq!(set(&f.app, "ownr", &[], &c, "app", "app").unwrap_err(), Refusal::NotFound);
    // host is the box login, never an App.
    assert_eq!(read("host", "dana").unwrap_err(), Refusal::NotFound);
    assert!(grant("host", &[], "owner").is_err());
}

#[test]
fn rooms_outside_the_ceiling_and_unknown_handles_are_refused_alike() {
    let f = fx("out");
    add_principal_for_app("dana", None, &f.app, None, "test").unwrap();
    grant(&f.app, &[f.room_b.clone()], "owner").unwrap();
    let c = Ceiling::Rooms(app_ceiling(&f.app).unwrap().unwrap());
    let e = set(&f.app, "dana", &[f.room_b.clone(), f.room_c.clone()], &c, "app", "app").unwrap_err();
    assert_eq!(e, Refusal::OutsideApp(vec![f.room_c.clone()]));
    assert_eq!((e.status(), e.code()), (403, "outside_app_rooms"));
    let e = set(&f.app, "dana", &["no-such-room".to_string()], &c, "app", "app").unwrap_err();
    assert_eq!(e, Refusal::OutsideApp(v(&["no-such-room"])));
    assert!(read(&f.app, "dana").unwrap().rooms.is_empty(), "nothing written on a refusal");
    // The owner's rights: any room on the box.
    let out = set(&f.app, "dana", &[f.room_c.clone()], &Ceiling::AnyRoom, "owner", "cli").unwrap();
    assert_eq!(out.view.rooms, vec![f.room_c.clone()]);
}

#[test]
fn never_above_the_guests_role() {
    let f = fx("role");
    let mut policy = RoomPolicy::new();
    policy.insert(f.own_room.clone(), v(&["thread:read", "files:read"]));
    policy.insert(f.room_b.clone(), v(&["thread:read"]));
    let role = create_role_for_app(&format!("crew{}", f.tag), &policy, Some(&f.app)).unwrap();
    add_principal_for_app("rio", None, &f.app, Some(&role.id), "test").unwrap();
    let c = Ceiling::AnyRoom;
    let r = read(&f.app, "rio").unwrap();
    assert_eq!(r.role.as_deref(), Some(role.name.as_str()));
    assert_eq!(sorted(r.rooms), sorted(vec![f.own_room.clone(), f.room_b.clone()]));
    // A room the role doesn't cover would add caps beyond the role.
    let e = set(&f.app, "rio", &[f.own_room.clone(), f.room_c.clone()], &c, "owner", "cli").unwrap_err();
    assert_eq!(e, Refusal::AboveRole(vec![f.room_c.clone()]));
    assert_eq!((e.status(), e.code()), (403, "above_role"));
    // A narrower set needs a narrower role.
    let e = set(&f.app, "rio", &[f.own_room.clone()], &c, "owner", "cli").unwrap_err();
    assert_eq!(e, Refusal::HasRole(role.name.clone()));
    assert_eq!(e.status(), 409);
    // The role's own rooms, in any order: a no-op.
    let out = set(&f.app, "rio", &[f.room_b.clone(), f.own_room.clone()], &c, "owner", "cli").unwrap();
    assert!(!out.view.changed);
}

#[test]
fn the_same_set_is_a_no_op_and_only_changes_are_audited() {
    let f = fx("idem");
    add_principal_for_app("dana", None, &f.app, None, "test").unwrap();
    grant(&f.app, &[f.room_b.clone()], "owner").unwrap();
    let c = Ceiling::Rooms(app_ceiling(&f.app).unwrap().unwrap());
    set(&f.app, "dana", &[f.own_room.clone()], &c, "app:jobs", "app").unwrap();
    let again = set(&f.app, "dana", &[f.own_room.clone()], &c, "app:jobs", "app").unwrap();
    assert!(!again.view.changed, "same rooms: nothing written");
    let swap = set(&f.app, "dana", &[f.room_b.clone()], &c, "garden:board", "garden").unwrap();
    assert!(swap.view.changed);
    assert_eq!(swap.before, vec![f.own_room.clone()]);

    let rows = audit_rows(&f.app, 10).unwrap();
    let changes: Vec<_> = rows.iter().filter(|r| r.event == AUDIT_CHANGE).collect();
    assert_eq!(changes.len(), 2, "two changes, the no-op left no row: {rows:?}");
    // Newest first: the swap, then the first set.
    assert_eq!(changes[0].actor, "garden:board");
    assert_eq!(changes[0].payload["before"], json!([f.own_room]));
    assert_eq!(changes[0].payload["after"], json!([f.room_b]));
    assert_eq!(changes[0].payload["username"], json!("dana"));
    assert_eq!(changes[0].payload["via"], json!("garden"));
    assert_eq!(changes[1].payload["before"], json!([]));
    assert_eq!(changes[1].payload["after"], json!([f.own_room]));
    assert!(rows.iter().any(|r| r.event == AUDIT_GRANT && r.payload["after"] == json!([f.room_b])), "the grant is audited");
    revoke(&f.app, "owner").unwrap();
    assert_eq!(get_grant(&f.app).unwrap(), None);
    assert!(audit_rows(&f.app, 10).unwrap().iter().any(|r| r.event == AUDIT_REVOKE));
    let h = crate::db::shared();
    let conn = h.lock();
    crate::deploy::audit::verify(&conn).unwrap().expect("the chain holds");
}

#[test]
fn sixty_calls_a_minute_per_app() {
    let f = fx("rate");
    add_principal_for_app("dana", None, &f.app, None, "test").unwrap();
    for i in 0..RATE_PER_MIN {
        read(&f.app, "dana").unwrap_or_else(|e| panic!("call {i}: {e:?}"));
    }
    match read(&f.app, "dana").unwrap_err() {
        Refusal::RateLimited { retry_after_ms } => assert!(retry_after_ms > 0 && retry_after_ms <= 60_000),
        e => panic!("want rate_limited, got {e:?}"),
    }
    assert!(matches!(set(&f.app, "dana", &[], &Ceiling::AnyRoom, "o", "cli"), Err(Refusal::RateLimited { .. })), "writes share it");
    // Per App: another App's budget is its own.
    add_principal_for_app("erin", None, &f.other_app, None, "test").unwrap();
    read(&f.other_app, "erin").unwrap();
    // Review B4: the owner never spends (or is stopped by) the App's budget,
    // and app.rooms has its own.
    for i in 0..(RATE_PER_MIN + 5) {
        with_owner_budget(|| read(&f.app, "dana")).unwrap_or_else(|e| panic!("owner call {i}: {e:?}"));
    }
    assert!(matches!(read(&f.app, "dana"), Err(Refusal::RateLimited { .. })), "the App is still limited");
    rate_take(&app_rooms_rate_key(&f.app)).expect("app.rooms has its own bucket");
}

#[test]
fn a_regrant_replaces_rooms_and_reports_before() {
    let f = fx("regrant");
    let (b, a) = grant(&f.app, &[f.room_b.clone()], "owner").unwrap();
    assert!(b.is_none());
    assert_eq!(a.rooms, vec![f.room_b.clone()]);
    let (b, a) = grant(&f.app, &[f.room_c.clone()], "owner:rosson").unwrap();
    assert_eq!(b.unwrap().rooms, vec![f.room_b.clone()]);
    assert_eq!((a.rooms, a.granted_by.as_str(), a.kind.as_str()), (vec![f.room_c.clone()], "owner:rosson", GRANT_KIND));
}

// ── app.rooms / guests.list (prd-app-caps-0464-v1 AC1/AC2) ───────────

#[test]
fn app_rooms_reads_without_the_grant_and_its_ceiling_is_the_enforced_one() {
    let f = fx("approoms");
    let none = app_rooms(&f.app).unwrap();
    assert!(!none.granted, "{none:?}");
    assert_eq!((none.granted_at, none.grant_rooms.len(), none.ceiling.len()), (None, 0, 0), "no grant: an empty ceiling (V1)");
    let own = none.own_room.clone().expect("the own room resolves");
    assert_eq!((own.room.as_str(), own.handle.as_str()), (f.own_room.as_str(), f.own_room.as_str()));
    grant(&f.app, &[f.room_b.clone(), "gone-room".to_string()], "owner").unwrap();
    let r = app_rooms(&f.app).unwrap();
    assert!(r.granted && r.granted_at.is_some_and(|t| t > 0), "{r:?}");
    assert_eq!(r.grant_rooms.iter().map(|x| x.room.clone()).collect::<Vec<_>>(), vec![f.room_b.clone()], "a gone room is left out");
    assert_eq!(sorted(r.ceiling.clone()), sorted(vec![f.room_b.clone(), f.own_room.clone()]));
    // The ceiling set() enforces is app_ceiling's (live rooms the same).
    let enforced = app_ceiling(&f.app).unwrap().expect("granted");
    for room in &r.ceiling {
        assert!(enforced.contains(room), "{room}");
    }
    let wire = serde_json::to_value(&r).unwrap();
    let mut keys: Vec<&str> = wire.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["app", "ceiling", "grantRooms", "granted", "grantedAt", "ok", "ownRoom"]);
    let mut own_keys: Vec<&str> = wire["ownRoom"].as_object().unwrap().keys().map(String::as_str).collect();
    own_keys.sort_unstable();
    assert_eq!(own_keys, ["displayName", "handle", "room"]);
}

#[test]
fn guests_list_pages_by_username_and_lists_only_this_apps_guests() {
    let f = fx("glist");
    for i in 0..250 {
        add_principal_for_app(&format!("g{i:03}-{}", f.tag), None, &f.app, None, "test").unwrap();
    }
    add_principal_for_app("erin", None, &f.other_app, None, "test").unwrap();
    add_principal("boxy").unwrap();
    // A Connect login sharing a guest's name is never listed.
    add_principal_for_app("ownr", None, &f.app, None, "test").unwrap();
    crate::connect_users::add_user("ownr", "correct horse battery 9").unwrap();
    // A marked agent login is listed with agent: true.
    add_principal_for_app("deskbot", None, &f.app, None, "test").unwrap();
    crate::skin::set_principal_agent("deskbot", Some(&f.own_room)).unwrap();

    let mut seen: Vec<String> = Vec::new();
    let mut cursor = String::new();
    let mut pages = 0;
    loop {
        let p = list(&f.app, &cursor, Some(100)).unwrap();
        pages += 1;
        assert!(p.guests.len() <= 100);
        seen.extend(p.guests.iter().map(|g| g.username.clone()));
        match p.next_cursor {
            Some(c) => cursor = c,
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen.len(), 251, "250 guests + deskbot; never erin, boxy or the Connect login");
    let mut want = seen.clone();
    want.sort();
    assert_eq!(seen, want, "stable order by username");
    assert!(!seen.iter().any(|u| u == "erin" || u == "boxy" || u == "ownr"));
    let all = list(&f.app, "", Some(500)).unwrap();
    assert_eq!(all.guests.len(), 251);
    assert_eq!(all.next_cursor, None);
    let bot = all.guests.iter().find(|g| g.username == "deskbot").expect("deskbot listed");
    assert!(bot.agent);
    assert!(all.guests.iter().filter(|g| g.username != "deskbot").all(|g| !g.agent));
    assert!(all.guests.iter().all(|g| g.enabled && g.role.is_none()));
    // Bad limits are usage; host is never an App.
    assert!(matches!(list(&f.app, "", Some(0)), Err(Refusal::Usage(_))));
    assert!(matches!(list(&f.app, "", Some(501)), Err(Refusal::Usage(_))));
    assert_eq!(list("host", "", None).unwrap_err(), Refusal::NotFound);
}

#[test]
fn a_guests_list_page_is_one_call_against_the_rate() {
    let f = fx("glrate");
    add_principal_for_app("dana", None, &f.app, None, "test").unwrap();
    for i in 0..RATE_PER_MIN {
        list(&f.app, "", None).unwrap_or_else(|e| panic!("page {i}: {e:?}"));
    }
    assert!(matches!(list(&f.app, "", None), Err(Refusal::RateLimited { .. })));
}
