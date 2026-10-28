//! Pause / resume another workspace's heartbeat (0.46.6), through the
//! heartbeat dispatch exactly as the TCP arm and the cell socket call it:
//! who may (the owner, an Admin login, a K2 shell tab), who may not (an
//! agent: `other_workspace`, which the card walls teach; a Member login:
//! `admin_only`), errors that name the workspace, the agent's own
//! workspace as before, and the audit line for every cross-workspace act.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use serde_json::Value;

use super::*;
use crate::sidecar_routes::{AgentCaller, Caller};

struct Ws {
    id: String,
    path: String,
    handle: String,
}

/// A registered workspace on disk with one enabled heartbeat per name.
fn workspace(home: &crate::test_support::TempHome, tag: &str, heartbeats: &[&str]) -> Ws {
    let handle = format!("{tag}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let dir = home.path().join(&handle);
    std::fs::create_dir_all(&dir).expect("workspace dir");
    let path = dir.to_string_lossy().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", rusqlite::params![id, handle, path])
        .expect("insert project");
    for hb in heartbeats {
        let rel = format!(".k2/heartbeats/{hb}/WAKEUP.md");
        std::fs::create_dir_all(dir.join(format!(".k2/heartbeats/{hb}"))).unwrap();
        std::fs::write(dir.join(&rel), "Check the queue.\n").unwrap();
        k2_core::db::schema::AgentHeartbeat::insert(&conn, &uuid::Uuid::new_v4().to_string(), &id, hb, "hourly", "{}", &rel, true)
            .expect("insert heartbeat");
    }
    Ws { id, path, handle }
}

/// A heartbeat name no other test uses (the test DB is shared).
fn hb_name() -> String {
    format!("triage-{}", &uuid::Uuid::new_v4().simple().to_string()[..6])
}

fn enabled(ws: &Ws, name: &str) -> bool {
    enabled_of(&ws.id, name).unwrap_or_else(|| panic!("{} has no heartbeat {name}", ws.handle))
}

/// The heartbeat dispatch as the TCP arm runs it for the owner token or a
/// login (`actor`: `owner-token`, `user:<name>`).
fn call_as(actor: Option<&str>, params: &[(&str, &str)]) -> (String, Value) {
    let p: HashMap<String, String> = params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = crate::heartbeat_routes::with_request_actor(actor.map(str::to_string), || {
        crate::misc_routes::dispatch(ROUTE, &p).expect("heartbeat enable is handled")
    });
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

/// What `stamp_principal` leaves for an agent's passport: `project` is its
/// own path. No live session behind it, so it is an agent (fail closed).
fn call_as_agent(me: &Ws, extra: &[(&str, &str)]) -> (String, Value) {
    let mut params = vec![
        ("project", me.path.as_str()),
        ("project_id", me.id.as_str()),
        (crate::caller_workspace::PRINCIPAL_BOUND_KEY, "1"),
    ];
    params.extend_from_slice(extra);
    call_as(None, &params)
}

fn audit(home: &crate::test_support::TempHome) -> String {
    std::fs::read_to_string(home.path().join(".k2").join("auth-audit.jsonl")).unwrap_or_default()
}

fn login(name: &str, role: Role) -> String {
    let user = format!("{name}{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    k2_core::connect_users::add_user(&user, "password123").expect("add_user");
    if role != Role::Member {
        k2_core::connect_users::set_role(&user, role).expect("set_role");
    }
    user
}

/// The actor on the heartbeat's newest `changed` history row.
fn last_change_actor(ws: &Ws, name: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT actor FROM heartbeat_fires WHERE project_id = ?1 AND schedule_name = ?2 AND decision = ?3 ORDER BY rowid DESC LIMIT 1",
        rusqlite::params![ws.id, name, k2_core::heartbeats::DECISION_CHANGED],
        |r| r.get::<_, Option<String>>(0),
    )
    .expect("a changed row")
}

#[test]
fn the_owner_pauses_and_resumes_another_workspace() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let t = hb_name();
    let mgr = workspace(&home, "hbomgr", &[t.as_str()]);
    let comp = workspace(&home, "hbocomp", &[t.as_str()]);

    // By handle and by path; `enabled=false` pauses, `true` resumes.
    let (s, v) = call_as(Some("owner-token"), &[("project", &mgr.path), ("name", t.as_str()), ("enabled", "false"), ("workspace", &comp.handle)]);
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!((v["workspace"].as_str(), v["heartbeat"].as_str(), &v["enabled"]), (Some(comp.handle.as_str()), Some(t.as_str()), &Value::Bool(false)), "{v}");
    assert!(!enabled(&comp, &t), "the named workspace's heartbeat is paused");
    assert!(enabled(&mgr, &t), "the caller's own heartbeat of the same name is untouched");
    assert_eq!(last_change_actor(&comp, &t).as_deref(), Some("owner-token"));

    let (s, v) = call_as(Some("owner-token"), &[("project", &mgr.path), ("name", t.as_str()), ("enabled", "true"), ("workspace", &comp.path)]);
    assert_eq!(s, "200 OK", "{v}");
    assert!(enabled(&comp, &t), "resumed");

    // `<workspace>/<name>` reads as both.
    let ws_name = format!("{}/{t}", comp.handle);
    let (s, v) = call_as(Some("owner-token"), &[("project", &mgr.path), ("name", &ws_name), ("enabled", "false")]);
    assert_eq!(s, "200 OK", "{v}");
    assert!(!enabled(&comp, &t) && enabled(&mgr, &t), "workspace/name paused the named workspace's");

    let a = audit(&home);
    assert!(a.contains("\"event\":\"heartbeat-other\""), "{a}");
    assert!(a.contains(&format!("paused:{}/{t}", comp.handle)) && a.contains(&format!("resumed:{}/{t}", comp.handle)), "{a}");
}

#[test]
fn an_admin_login_and_a_k2_shell_tab_are_people() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let t = hb_name();
    let mgr = workspace(&home, "hbpmgr", &[]);
    let comp = workspace(&home, "hbpcomp", &[t.as_str()]);

    let admin = login("hboadm", Role::Admin);
    let actor = format!("user:{admin}");
    let (s, v) = call_as(Some(&actor), &[("project", &mgr.path), ("name", t.as_str()), ("enabled", "false"), ("workspace", &comp.handle)]);
    assert_eq!(s, "200 OK", "{v}");
    assert!(!enabled(&comp, &t));
    assert_eq!(last_change_actor(&comp, &t).as_deref(), Some(actor.as_str()));
    assert!(audit(&home).contains(&format!("login:{admin}")), "the audit names the login");

    // A person typing in a K2 shell tab of the manager's workspace.
    let shell = Caller::Shell { project_id: mgr.id.clone() };
    let r = handle(Some(&mgr.path), Some(&comp.handle), &t, true, &shell, "test");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(enabled(&comp, &t));
    assert_eq!(last_change_actor(&comp, &t), Some(format!("shell:{}", mgr.handle)));
}

#[test]
fn an_agent_is_refused_with_other_workspace_and_nothing_changes() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let t = hb_name();
    let mgr = workspace(&home, "hbamgr", &[]);
    let comp = workspace(&home, "hbacomp", &[t.as_str()]);

    let ws_name = format!("{}/{t}", comp.handle);
    for extra in [
        vec![("name", t.as_str()), ("enabled", "false"), ("workspace", comp.handle.as_str())],
        vec![("name", t.as_str()), ("enabled", "false"), ("workspace", comp.path.as_str())],
        // `workspace/name`, the spelling the field report tried.
        vec![("name", ws_name.as_str()), ("enabled", "false")],
    ] {
        let (s, v) = call_as_agent(&mgr, &extra);
        assert_eq!(s, "403 Forbidden", "{extra:?}: {v}");
        assert_eq!(v["error"]["code"], OTHER_WORKSPACE, "never no_such_heartbeat: {v}");
        // The walls read these two; the CLI prints the hint.
        assert_eq!(v["error"]["workspace"], comp.handle.as_str(), "{v}");
        assert_eq!(v["error"]["heartbeat"], t.as_str(), "{v}");
        let hint = v["error"]["hint"].as_str().expect("hint");
        assert!(hint.contains(&format!("k2 heartbeat disable {t} --workspace {}", comp.handle)), "{hint}");
        assert!(hint.contains(&mgr.handle), "names the agent's own workspace: {hint}");
    }
    assert!(enabled(&comp, &t), "an agent's refusal changes nothing");
    let (s, v) = call_as_agent(&mgr, &[("name", t.as_str()), ("enabled", "true"), ("workspace", comp.handle.as_str())]);
    assert_eq!((s.as_str(), &v["error"]["code"]), ("403 Forbidden", &Value::from(OTHER_WORKSPACE)), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains(&format!("k2 heartbeat enable {t}")), "{v}");

    let a = audit(&home);
    assert!(a.contains(&format!("refused:other_workspace:{}/{t}", comp.handle)), "{a}");
    assert!(a.contains(&format!("agent:{}", mgr.handle)), "the audit names the agent: {a}");
}

#[test]
fn a_member_login_is_refused() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let t = hb_name();
    let mgr = workspace(&home, "hbmmgr", &[]);
    let comp = workspace(&home, "hbmcomp", &[t.as_str()]);
    let member = login("hbomem", Role::Member);
    let (s, v) = call_as(
        Some(&format!("user:{member}")),
        &[("project", &mgr.path), ("name", t.as_str()), ("enabled", "false"), ("workspace", &comp.handle)],
    );
    assert_eq!(s, "403 Forbidden", "{v}");
    assert_eq!(v["error"]["code"], "admin_only", "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("member"), "{v}");
    assert!(enabled(&comp, &t), "nothing changed");
    assert!(audit(&home).contains(&format!("refused:admin_only:{}/{t}", comp.handle)));
}

#[test]
fn a_missing_heartbeat_names_the_workspace_it_was_looked_for_in() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let t = hb_name();
    let mgr = workspace(&home, "hbnmgr", &[]);
    let comp = workspace(&home, "hbncomp", &[t.as_str()]);

    // In another workspace: the owner and an agent both hear where.
    let (s, v) = call_as(Some("owner-token"), &[("project", &mgr.path), ("name", "nope"), ("enabled", "false"), ("workspace", &comp.handle)]);
    assert_eq!(s, "404 Not Found", "{v}");
    assert_eq!(v["error"], "no_such_heartbeat", "{v}");
    assert_eq!(v["workspace"], comp.handle.as_str(), "{v}");
    let msg = v["message"].as_str().unwrap();
    assert!(msg.contains(&format!("in workspace {}", comp.handle)) && msg.contains("schedule list --workspace"), "{msg}");
    let (s, v) = call_as_agent(&mgr, &[("name", "nope"), ("enabled", "false"), ("workspace", comp.handle.as_str())]);
    assert_eq!((s.as_str(), &v["error"]), ("404 Not Found", &Value::from("no_such_heartbeat")), "{v}");
    assert!(v["message"].as_str().unwrap().contains(&format!("in workspace {}", comp.handle)), "{v}");

    // In the agent's own workspace, when another workspace has one by that
    // name: the answer says which and how (the field report's "ran it from
    // the other folder").
    let (s, v) = call_as_agent(&mgr, &[("name", t.as_str()), ("enabled", "false")]);
    assert_eq!(s, "404 Not Found", "{v}");
    let msg = v["message"].as_str().unwrap();
    assert!(msg.contains(&format!("in workspace {}", mgr.handle)), "{msg}");
    assert!(msg.contains(&format!("k2 heartbeat disable {t} --workspace {}", comp.handle)), "{msg}");

    // An unknown workspace is its own error, never the caller's heartbeat.
    let (s, v) = call_as(Some("owner-token"), &[("project", &mgr.path), ("name", t.as_str()), ("enabled", "false"), ("workspace", "no-such-ws-xyz")]);
    assert_eq!(s, "400 Bad Request", "{v}");
    assert!(v["error"].as_str().unwrap_or("").contains("workspace not found"), "{v}");
    assert!(enabled(&comp, &t));
}

#[test]
fn the_agents_own_workspace_works_as_before() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let t = hb_name();
    let mgr = workspace(&home, "hbsmgr", &[t.as_str()]);

    let (s, v) = call_as_agent(&mgr, &[("name", t.as_str()), ("enabled", "false")]);
    assert_eq!(s, "200 OK", "{v}");
    assert_eq!(v, serde_json::json!({"success": true}), "the answer is unchanged");
    assert!(!enabled(&mgr, &t));
    // Naming its own workspace is the same thing.
    let (s, v) = call_as_agent(&mgr, &[("name", t.as_str()), ("enabled", "true"), ("workspace", mgr.handle.as_str())]);
    assert_eq!(s, "200 OK", "{v}");
    assert!(enabled(&mgr, &t));
    // An unknown name is still a 404 no_such_heartbeat.
    let (s, v) = call_as_agent(&mgr, &[("name", "nope"), ("enabled", "false")]);
    assert_eq!((s.as_str(), &v["error"]), ("404 Not Found", &Value::from("no_such_heartbeat")), "{v}");
    // Own-workspace changes are not cross-workspace: no audit line.
    assert!(!audit(&home).contains("heartbeat-other"), "{}", audit(&home));
}

/// `project=` names the workspace exactly like `workspace=` (0.46.6): the
/// owner may, a Member login may not (on any workspace: a login has none of
/// its own), an agent whose own `project=` claim names another registered
/// workspace is refused with the card's code; a claim of its own workspace,
/// or of a folder that isn't a workspace, is its own toggle as before.
#[test]
fn project_names_the_workspace_like_workspace_does() {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let t = hb_name();
    let mgr = workspace(&home, "hbjmgr", &[t.as_str()]);
    let comp = workspace(&home, "hbjcomp", &[t.as_str()]);

    // The owner: allowed, audited (a login or the owner has no workspace
    // of its own, so every toggle of theirs is another workspace's).
    let (s, v) = call_as(Some("owner-token"), &[("project", &comp.path), ("name", t.as_str()), ("enabled", "false")]);
    assert_eq!(s, "200 OK", "{v}");
    assert!(!enabled(&comp, &t) && enabled(&mgr, &t), "the project= workspace's heartbeat paused, no other");
    assert!(audit(&home).contains(&format!("paused:{}/{t}", comp.handle)), "{}", audit(&home));
    let (s, v) = call_as(Some("owner-token"), &[("project", &comp.path), ("name", t.as_str()), ("enabled", "true")]);
    assert_eq!(s, "200 OK", "{v}");
    assert!(enabled(&comp, &t));

    // An Admin login: allowed.
    let admin = login("hbjadm", Role::Admin);
    let (s, v) = call_as(Some(&format!("user:{admin}")), &[("project", &comp.path), ("name", t.as_str()), ("enabled", "false")]);
    assert_eq!(s, "200 OK", "{v}");
    assert!(!enabled(&comp, &t));

    // A Member login: admin_only on project=, whichever workspace it names.
    let member = login("hbjmem", Role::Member);
    for ws in [&comp.path, &mgr.path] {
        let (s, v) = call_as(Some(&format!("user:{member}")), &[("project", ws), ("name", t.as_str()), ("enabled", "true")]);
        assert_eq!((s.as_str(), &v["error"]["code"]), ("403 Forbidden", &Value::from("admin_only")), "{ws}: {v}");
    }
    assert!(!enabled(&comp, &t) && enabled(&mgr, &t), "a refused Member changes nothing");
    assert!(audit(&home).contains(&format!("refused:admin_only:{}/{t}", mgr.handle)), "{}", audit(&home));

    // An agent whose own project= claim names another workspace: refused
    // like workspace=, with what the card needs.
    let (s, v) = call_as_agent(&mgr, &[(crate::caller_workspace::CLIENT_PROJECT_KEY, comp.path.as_str()), ("name", t.as_str()), ("enabled", "true")]);
    assert_eq!(s, "403 Forbidden", "{v}");
    assert_eq!(v["error"]["code"], OTHER_WORKSPACE, "{v}");
    assert_eq!((v["error"]["workspace"].as_str(), v["error"]["heartbeat"].as_str()), (Some(comp.handle.as_str()), Some(t.as_str())), "{v}");
    assert!(!enabled(&comp, &t), "the agent's refusal changes nothing");
    // Its own workspace, or a folder that isn't a workspace: its own toggle.
    let not_ws = home.path().join("not-a-workspace").to_string_lossy().to_string();
    for claim in [mgr.path.as_str(), not_ws.as_str()] {
        let (s, v) = call_as_agent(&mgr, &[(crate::caller_workspace::CLIENT_PROJECT_KEY, claim), ("name", t.as_str()), ("enabled", "false")]);
        assert_eq!((s.as_str(), &v), ("200 OK", &serde_json::json!({"success": true})), "{claim}: {v}");
        assert!(!enabled(&mgr, &t), "{claim}");
        let (s, _) = call_as_agent(&mgr, &[(crate::caller_workspace::CLIENT_PROJECT_KEY, claim), ("name", t.as_str()), ("enabled", "true")]);
        assert_eq!(s, "200 OK");
    }

    // The claim key is the server's: a client can't send it, and the
    // server keeps the request's own project= under it before the stamp.
    let mut p: HashMap<String, String> = HashMap::new();
    p.insert(crate::caller_workspace::CLIENT_PROJECT_KEY.into(), "/spoofed".into());
    crate::caller_workspace::strip_request_stamps(&mut p);
    assert!(p.is_empty(), "{p:?}");
    p.insert(crate::caller_workspace::CLIENT_PROJECT_KEY.into(), "/spoofed".into());
    p.insert("project".into(), comp.path.clone());
    crate::caller_workspace::keep_client_project(&mut p);
    assert_eq!(p.get(crate::caller_workspace::CLIENT_PROJECT_KEY), Some(&comp.path));
}

#[test]
fn get_is_refused_and_callers_are_classified() {
    // The door guard: a GET to the POST-only route is refused centrally
    // (405 on TCP and the cell socket) before any handler runs.
    let none = HashMap::new();
    assert!(crate::routes::route_policy::get_refused_params(ROUTE, &none), "GET {ROUTE} must be refused");
    // A passport with no live session behind it is an agent (fail closed).
    let mut p = HashMap::new();
    p.insert("project_id".to_string(), "w-gone".to_string());
    p.insert(crate::caller_workspace::PRINCIPAL_BOUND_KEY.to_string(), "1".to_string());
    p.insert(crate::caller_workspace::CELL_SESSION_KEY.to_string(), "no-such-session".to_string());
    assert!(matches!(caller_from_params(&p), Caller::Agent(AgentCaller { ref project_id, .. }) if project_id == "w-gone"));
    // A login actor is a login; nothing at all is the owner (in-daemon).
    let c = crate::heartbeat_routes::with_request_actor(Some("user:pat".into()), || caller_from_params(&HashMap::new()));
    assert_eq!(c, Caller::Login { username: "pat".into() });
    assert_eq!(caller_from_params(&HashMap::new()), Caller::Owner);
}
