//! The agent access list's `can`, the owner's grid and the suggested role
//! templates (never created on their own; caps that exist today).

use super::*;
use crate::skin::{add_principal_for_app, find_role, list_roles, list_grants, set_grant_enabled, ACCEPTED_CAPS};

struct Fx {
    _home: crate::test_env::TempHome,
    _db: crate::db::TestDbGuard,
    app: String,
    other_app: String,
    room: String,
    desk: String,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_env::TempHome::new();
    let db = crate::db::scoped_for_test();
    let (room, desk) = (format!("jobs-{tag}"), format!("desk-{tag}"));
    let (app, other_app) = {
        let h = crate::db::shared();
        let conn = h.lock();
        for r in [&room, &desk] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![r, format!("/tmp/ac-{r}")])
                .unwrap();
        }
        let a = crate::published_services::insert(&conn, &room, "Jobs App", "node s.js", "/tmp", 4300, "local", "running", "cmd", "")
            .unwrap();
        let o = crate::published_services::insert(&conn, &desk, "crm", "node s.js", "/tmp", 4301, "local", "running", "cmd", "").unwrap();
        (a.id, o.id)
    };
    Fx { _home: home, _db: db, app, other_app, room, desk }
}

#[test]
fn can_reads_the_caps() {
    let c = can_of(&["thread:read".into(), "files:read".into(), "tickets:post".into(), "store:write".into()]);
    assert_eq!(c["chat"], true);
    assert_eq!(c["send"], false);
    assert_eq!(c["files"], "read");
    assert_eq!(c["tickets"], "write");
    assert_eq!(c["store"], "write");
    assert_eq!(c["heartbeats"], "none");
    assert_eq!((c["activity"].as_bool(), c["members"].as_bool(), c["wiki"].as_bool(), c["identity"].as_bool()), (Some(false), Some(false), Some(false), Some(false)));
    assert_eq!(can_of(&[])["files"], "none");
}

#[test]
fn templates_use_only_caps_that_exist_and_name_their_gaps() {
    let ids: Vec<&str> = TEMPLATES.iter().map(|t| t.id).collect();
    assert_eq!(ids, ["owner", "manager", "staff", "field", "client", "vendor", "auditor", "app"]);
    for t in TEMPLATES {
        for c in t.caps {
            assert!(ACCEPTED_CAPS.contains(c), "{}: {c} is not a cap K2 has", t.id);
        }
        assert!(!t.description.is_empty() && !t.label.is_empty(), "{}", t.id);
        if let Some(f) = &t.folders {
            assert!(t.caps.contains(&"files:write"), "{}: folders need files", t.id);
            for d in f.read.iter().chain(f.write.iter()) {
                crate::skin::paths::normalize_folder(d).unwrap_or_else(|e| panic!("{}: {e}", t.id));
            }
        }
    }
    // Read-only Thread is today's thread:read without thread:post.
    let auditor = template("auditor").unwrap();
    assert!(auditor.caps.contains(&"thread:read") && !auditor.caps.contains(&"thread:post"));
    // Upload-only doesn't exist yet: the notes say so.
    for id in ["field", "client", "vendor"] {
        assert!(template(id).unwrap().note.contains("Upload-only"), "{id}: {}", template(id).unwrap().note);
    }
    assert!(template("app").unwrap().caps.is_empty());
    assert!(template("OWNER").is_some(), "case-insensitive");
    assert!(template("nope").is_none());
}

#[test]
fn templates_are_never_created_on_their_own() {
    let f = fx("none");
    assert!(list_roles().unwrap().iter().all(|r| r.app_id.as_deref() != Some(f.app.as_str())), "a new App has no roles");
    let _ = (&f.other_app, &f.desk);
}

#[test]
fn a_role_from_a_template_is_an_ordinary_editable_role() {
    let f = fx("tpl");
    let role = create_from_template("field", &f.app, "Jobs App", None, &[f.room.clone()]).unwrap();
    assert_eq!(role.name, "jobs-app-field");
    assert_eq!(role.app_id.as_deref(), Some(f.app.as_str()));
    assert_eq!(role.room_policy[&f.room], template("field").unwrap().caps.iter().map(|c| c.to_string()).collect::<Vec<_>>());
    let scope = &role.room_paths[&f.room];
    assert_eq!(scope.write, ["Inbox"]);
    assert_eq!(scope.read, ["Plans", "Start Here"]);
    assert_eq!(scope.upload_dir.as_deref(), Some("Inbox"));
    // The next one gets a free name; an explicit taken name is refused.
    let again = create_from_template("field", &f.app, "Jobs App", None, &[f.room.clone()]).unwrap();
    assert_eq!(again.name, "jobs-app-field-2");
    assert!(create_from_template("field", &f.app, "Jobs App", Some("jobs-app-field"), &[]).is_err());
    // Owner/viewer are Connect names; the templates never produce them.
    let owner = create_from_template("owner", &f.app, "Jobs App", None, &[f.room.clone(), f.desk.clone()]).unwrap();
    assert_eq!(owner.name, "jobs-app-owner");
    assert_eq!(owner.room_policy.len(), 2);
    // The App service account has no room lines.
    let app_role = create_from_template("app", &f.app, "Jobs App", None, &[f.room.clone()]).unwrap();
    assert!(app_role.room_policy.is_empty());
    // Editable like any role.
    let edited = crate::skin::clear_role_room(&owner.name, &f.desk).unwrap();
    assert_eq!(edited.room_policy.len(), 1);
    assert!(find_role(&edited.id).unwrap().is_some());
    assert!(create_from_template("nope", &f.app, "Jobs App", None, &[]).unwrap_err().contains("unknown role template"));
}

#[test]
fn the_grid_is_what_a_login_gets() {
    let f = fx("grid");
    let client = create_from_template("client", &f.app, "Jobs App", None, &[f.room.clone()]).unwrap();
    let staff = create_from_template("staff", &f.app, "Jobs App", None, &[f.room.clone(), f.desk.clone()]).unwrap();
    add_principal_for_app("cleo", None, &f.app, Some(&client.name), "test").unwrap();
    add_principal_for_app("sam", None, &f.app, Some(&staff.name), "test").unwrap();
    add_principal_for_app("olive", None, &f.other_app, None, "test").unwrap();
    let g = grid(&f.app).unwrap();
    let handles: Vec<&str> = g.agents.iter().map(|a| a.handle.as_str()).collect();
    assert_eq!(handles, [f.desk.as_str(), f.room.as_str()], "every agent anyone here reaches, by handle");
    let names: Vec<&str> = g.people.iter().map(|p| p.username.as_str()).collect();
    assert_eq!(names, ["cleo", "sam"], "only this App's people");
    let cleo = &g.people[0];
    assert_eq!(cleo.role.as_deref(), Some(client.name.as_str()));
    assert_eq!(cleo.cells.len(), 1);
    assert_eq!(cleo.cells[0].handle, f.room);
    assert_eq!(cleo.cells[0].can["files"], "write");
    assert_eq!(cleo.cells[0].folders.as_ref().map(|s| s.read.clone()), Some(vec!["Shared".to_string()]));
    assert_eq!(g.people[1].cells.len(), 2);
    // A disabled grant reaches nothing.
    let gr = list_grants().unwrap().into_iter().find(|x| x.target_id == f.app && x.subject_id == cleo.principal_id).unwrap();
    set_grant_enabled(&gr.id, false).unwrap();
    let g = grid(&f.app).unwrap();
    assert!(!g.people[0].enabled && g.people[0].cells.is_empty());
    assert_eq!(person(&f.app, "SAM").unwrap().map(|p| p.cells.len()), Some(2));
    assert!(person(&f.app, "olive").unwrap().is_none());
}
