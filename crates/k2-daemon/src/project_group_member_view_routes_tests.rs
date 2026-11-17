//! Tests for `project_group_member_view_routes` (moved with it from
//! `project_group_routes.rs`, unchanged).

use super::*;
use crate::project_group_routes::{dispatch, dispatch_post, dispatch_post_as, is_admin_only, is_gated_mutation};
use crate::sidecar_routes::{AgentCaller, Caller};
use crate::test_support::{event_mark, install_capture_sink};

fn events_for_group(mark: usize, group_id: &str) -> Vec<(String, serde_json::Value)> {
    crate::test_support::events_since(mark)
        .into_iter()
        .filter(|(_, p)| p["groupId"] == group_id)
        .collect()
}

fn gname(label: &str) -> String {
    format!("pgr-{label}-{}", uuid::Uuid::new_v4())
}

fn insert_workspace(label: &str) -> (String, String, String) {
    let id = uuid::Uuid::new_v4().to_string();
    let name = format!("pgr-ws-{label}-{id}");
    let path = format!("/tmp/pgr-ws-{label}-{}-{id}", std::process::id());
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
        rusqlite::params![id, name, path],
    )
    .expect("insert project row");
    (id, name, path)
}

fn post_json(path: &str, body: serde_json::Value) -> CliResponse {
    dispatch_post(path, body.to_string().as_bytes(), "owner")
}

fn ok_json(resp: CliResponse) -> serde_json::Value {
    assert_eq!(resp.status, "200 OK", "body={}", resp.body);
    serde_json::from_str(&resp.body).expect("valid JSON body")
}

fn create_group_via_route(name: &str) -> serde_json::Value {
    ok_json(post_json("/cli/project-group/create", serde_json::json!({ "name": name })))
}

fn show(selector: &str) -> CliResponse {
    let params = [("group".to_string(), selector.to_string())].into_iter().collect();
    dispatch("/cli/project-group/show", &params).expect("show route claimed")
}

fn agent(project_id: &str) -> Caller {
    Caller::Agent(AgentCaller {
        project_id: project_id.to_string(),
        map_key: Some(project_id.to_string()),
        args: Vec::new(),
        sandboxed: false,
    })
}

fn post(path: &str, body: serde_json::Value, caller: &Caller) -> CliResponse {
    dispatch_post_as(path, body.to_string().as_bytes(), caller, "test")
}

fn ok(resp: CliResponse) -> serde_json::Value {
    assert_eq!(resp.status, "200 OK", "body={}", resp.body);
    serde_json::from_str(&resp.body).expect("JSON body")
}

fn refused_off(resp: &CliResponse) {
    assert_eq!(resp.status, "403 Forbidden", "body={}", resp.body);
    let v: serde_json::Value = serde_json::from_str(&resp.body).expect("JSON body");
    assert_eq!(v["error"]["code"], "agent_permission_off", "{v}");
    let hint = v["error"]["hint"].as_str().expect("hint");
    assert!(hint.contains("Manage K2 projects"), "clear message: {hint}");
    assert!(hint.contains("still work"), "says reads + chat still work: {hint}");
}

fn audit_lines(project: &str) -> Vec<serde_json::Value> {
    k2_core::auth_audit::tail(1000)
        .expect("audit tail")
        .into_iter()
        .filter(|e| e["outcome"].as_str().is_some_and(|o| o.contains(project)))
        .collect()
}

/// The route is POST-only (GET → 405 on the read chain and in the route
/// policy), behind the switch, and an unknown project-group POST is
/// still 404 (these two asserts sat in the main file's route lists).
#[test]
fn member_view_route_is_post_only_and_unknown_posts_stay_404() {
    let params = std::collections::HashMap::new();
    assert!(is_gated_mutation(MEMBER_VIEW_PATH));
    let resp = dispatch(MEMBER_VIEW_PATH, &params).expect("route claimed by GET chain");
    assert_eq!(resp.status, "405 Method Not Allowed");
    assert!(resp.body.contains("POST required"), "body={}", resp.body);
    assert!(crate::routes::route_policy::get_refused_params(MEMBER_VIEW_PATH, &params));
    assert_eq!(dispatch_post("/cli/project-group/unknown", b"{}", "owner").status, "404 Not Found");
    assert!(dispatch("/cli/project-group/unknown", &params).is_none());
}

/// `show` answers, per member, what the nav used to ask once per agent
/// (`sessions/lookup-by-agent`): `sessionAlive` and `sessionId`, keyed by
/// the member's workspace id exactly like that lookup.
#[test]
fn project_group_show_members_carry_session_liveness() {
    let name = gname("liveness");
    let gid = create_group_via_route(&name)["id"].as_str().expect("id").to_string();
    let (live_id, _, _) = insert_workspace("live");
    let (idle_id, _, _) = insert_workspace("idle");
    for ws in [&live_id, &idle_id] {
        ok_json(post_json(
            "/cli/project-group/add-member",
            serde_json::json!({ "group": gid, "workspace": ws }),
        ));
    }
    let member = |v: &serde_json::Value, id: &str| {
        v["members"]
            .as_array()
            .expect("members")
            .iter()
            .find(|m| m["workspaceId"] == id)
            .unwrap_or_else(|| panic!("{id} in {v}"))
            .clone()
    };
    let before = ok_json(show(&gid));
    for id in [&live_id, &idle_id] {
        assert_eq!(member(&before, id)["sessionAlive"], false, "{before}");
        assert!(member(&before, id)["sessionId"].is_null(), "{before}");
    }

    // A live session registered under the member's workspace id (the
    // key `lookup-by-agent` and the canonical spawn use).
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio");
    let _enter = rt.enter();
    // A program that stays up on every OS (Windows has no `cat`).
    let (program, args) = k2_core::test_env::sleep_command(600);
    let session = k2_core::test_env::spawn_plain_pty(k2_core::terminal::DaemonPtyConfig {
        program: Some(program),
        args,
        cwd: Some(std::env::temp_dir()),
        ..Default::default()
    })
    .expect("spawn a long-lived program");
    crate::v2_session_map::register(live_id.clone(), session.clone());

    let after = ok_json(show(&gid));
    let live = member(&after, &live_id);
    assert_eq!(live["sessionAlive"], true, "{after}");
    assert_eq!(live["sessionId"], session.session_id.to_string().as_str(), "{after}");
    let idle = member(&after, &idle_id);
    assert_eq!(idle["sessionAlive"], false, "{after}");
    assert!(idle["sessionId"].is_null(), "{after}");
    // The same answer the per-agent lookup gives.
    let lookup = crate::session_lookup::lookup_any(&live_id).expect("live");
    assert_eq!(live["sessionId"], lookup.session_id().to_string().as_str());

    crate::v2_session_map::unregister(&live_id);
    session.kill();
    let gone = ok_json(show(&gid));
    assert_eq!(member(&gone, &live_id)["sessionAlive"], false, "{gone}");
    assert!(member(&gone, &live_id)["sessionId"].is_null(), "{gone}");
}

/// 0302: each member has its own view. `show` gives every member
/// `defaultView` (the view its tile opens in: its own, else the
/// project's) and `ownDefaultView` (its own, or null when it follows
/// the project). `set-member-default-view` sets one (`view` one of the
/// three) or clears it (`view: null`), emits members-changed, and is a
/// loud usage / not_a_member / not_found on anything else.
#[test]
fn project_group_member_default_view_route() {
    install_capture_sink();
    let name = gname("member-view");
    let gid = create_group_via_route(&name)["id"].as_str().expect("id").to_string();
    let (a_id, a_name, _) = insert_workspace("mv-a");
    let (b_id, _, b_path) = insert_workspace("mv-b");
    let (out_id, _, _) = insert_workspace("mv-out");
    for ws in [&a_id, &b_id] {
        ok_json(post_json(
            "/cli/project-group/add-member",
            serde_json::json!({ "group": gid, "workspace": ws }),
        ));
    }
    let views = |id: &str| {
        let v = ok_json(show(&gid));
        let m = v["members"]
            .as_array()
            .expect("members")
            .iter()
            .find(|m| m["workspaceId"] == id)
            .unwrap_or_else(|| panic!("{id} in {v}"))
            .clone();
        (m["defaultView"].clone(), m["ownDefaultView"].clone())
    };
    let s = |x: &str| serde_json::json!(x);
    let null = serde_json::Value::Null;
    assert_eq!(views(&a_id), (s("thread"), null.clone()), "members start on the project's view");

    // Set one member (addressed by name, then by path); the other follows the project.
    let mark = event_mark();
    let set = ok_json(post_json(
        MEMBER_VIEW_PATH,
        serde_json::json!({ "group": name, "workspace": a_name, "view": "terminal" }),
    ));
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(set["groupId"], gid.as_str(), "{set}");
    assert_eq!(set["workspaceId"], a_id.as_str(), "{set}");
    assert_eq!(set["defaultView"], "terminal", "{set}");
    assert_eq!(set["ownDefaultView"], "terminal", "{set}");
    assert_eq!(
        events_for_group(mark, &gid).iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        vec!["project-group:members-changed"]
    );
    assert_eq!(views(&a_id), (s("terminal"), s("terminal")));
    assert_eq!(views(&b_id), (s("thread"), null.clone()));
    ok_json(post_json(
        MEMBER_VIEW_PATH,
        serde_json::json!({ "group": gid, "workspace": b_path, "view": "chatter" }),
    ));
    assert_eq!(views(&b_id), (s("chatter"), s("chatter")));

    // The project's view moves only the members that follow it.
    ok_json(post_json(
        "/cli/project-group/set-default-view",
        serde_json::json!({ "group": gid, "view": "chatter" }),
    ));
    assert_eq!(views(&a_id), (s("terminal"), s("terminal")), "its own view stays");
    let cleared = ok_json(post_json(
        MEMBER_VIEW_PATH,
        serde_json::json!({ "group": gid, "workspace": a_id, "view": null }),
    ));
    assert_eq!(cleared["defaultView"], "chatter", "{cleared}");
    assert!(cleared["ownDefaultView"].is_null(), "{cleared}");
    assert_eq!(views(&a_id), (s("chatter"), null.clone()), "cleared: follows the project again");

    // Usage (400): bad or missing view, missing group or workspace, bad JSON.
    for body in [
        serde_json::json!({ "group": gid, "workspace": a_id, "view": "split" }),
        serde_json::json!({ "group": gid, "workspace": a_id, "view": "heartbeats" }),
        serde_json::json!({ "group": gid, "workspace": a_id, "view": "Thread" }),
        serde_json::json!({ "group": gid, "workspace": a_id, "view": "" }),
        serde_json::json!({ "group": gid, "workspace": a_id, "view": 3 }),
        serde_json::json!({ "group": gid, "workspace": a_id }),
        serde_json::json!({ "workspace": a_id, "view": "thread" }),
        serde_json::json!({ "group": gid, "view": "thread" }),
    ] {
        let resp = post_json(MEMBER_VIEW_PATH, body.clone());
        assert_eq!(resp.status, "400 Bad Request", "body={body} resp={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
        assert_eq!(v["error"]["code"], "usage", "body={body}: {v}");
    }
    let resp = post_json(
        MEMBER_VIEW_PATH,
        serde_json::json!({ "group": gid, "workspace": a_id, "view": "split" }),
    );
    let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
    assert_eq!(v["error"]["hint"], "view must be terminal, thread or chatter, got 'split'", "{v}");
    let resp = post_json(
        MEMBER_VIEW_PATH,
        serde_json::json!({ "group": gid, "workspace": a_id }),
    );
    let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
    assert!(
        v["error"]["hint"].as_str().expect("hint").contains("null"),
        "a missing view says how to clear: {v}"
    );
    assert_eq!(views(&a_id), (s("chatter"), null.clone()), "refusals change nothing");

    // Not a member: 403 not_a_member. Unknown project: 404 not_found.
    let resp = post_json(
        MEMBER_VIEW_PATH,
        serde_json::json!({ "group": gid, "workspace": out_id, "view": "thread" }),
    );
    assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
    let v: serde_json::Value = serde_json::from_str(&resp.body).expect("valid JSON");
    assert_eq!(v["error"]["code"], "not_a_member", "{v}");
    let resp = post_json(
        MEMBER_VIEW_PATH,
        serde_json::json!({ "group": format!("zzz-no-such-{}", uuid::Uuid::new_v4()), "workspace": a_id, "view": "thread" }),
    );
    assert_eq!(resp.status, "404 Not Found", "{}", resp.body);
}

/// 0302: one agent's own view changes what everyone's tile for it opens
/// in, so it follows the project view's rule: among logins the owner or
/// an admin (a Member gets 403 forbidden, nothing changes); an agent
/// follows its 'Manage K2 projects' switch; the owner token and a shell
/// tab may. Wiring: Admin floor, behind the switch, owner-or-admin.
#[test]
fn only_admins_and_managing_agents_set_a_members_view() {
    let _home = crate::test_support::TempHome::new();
    use k2_core::connect_users::{add_user, set_role, Role};
    let member = format!("pgmm{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let admin = format!("pgma{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    add_user(&member, "password123").expect("member");
    add_user(&admin, "password123").expect("admin");
    set_role(&admin, Role::Admin).expect("make admin");
    let (on_id, _, _) = insert_workspace("mview-agent-on");
    let (off_id, _, _) = insert_workspace("mview-agent-off");
    k2_core::workspace::settings::set_agents_can_manage_projects(&off_id, false).expect("switch off");

    let name = format!("pgs-mview-{}", uuid::Uuid::new_v4());
    let created = ok(post("/cli/project-group/create", serde_json::json!({ "name": name }), &Caller::Owner));
    let gid = created["id"].as_str().expect("id").to_string();
    ok(post(
        "/cli/project-group/add-member",
        serde_json::json!({ "group": gid, "workspace": on_id }),
        &Caller::Owner,
    ));
    let set = |view: Option<&str>, caller: &Caller| {
        post(MEMBER_VIEW_PATH, serde_json::json!({ "group": gid, "workspace": on_id, "view": view }), caller)
    };
    let stored = || project_groups::member_default_views(&gid).expect("views").get(&on_id).cloned();

    let r = set(Some("terminal"), &Caller::Login { username: member.clone() });
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("JSON body");
    assert_eq!(v["error"]["code"], "forbidden", "{v}");
    let hint = v["error"]["hint"].as_str().expect("hint");
    assert!(hint.contains("owner, an admin, or an agent with 'Manage K2 projects'"), "{hint}");
    assert!(hint.contains("view an agent opens in"), "{hint}");
    refused_off(&set(Some("terminal"), &agent(&off_id)));
    for c in [Caller::WrongCredential, Caller::Invalid] {
        assert_eq!(set(Some("terminal"), &c).status, "403 Forbidden", "{c:?}");
    }
    assert_eq!(stored(), None, "refusals change nothing");

    assert_eq!(ok(set(Some("terminal"), &Caller::Login { username: admin }))["ownDefaultView"], "terminal");
    assert_eq!(ok(set(Some("chatter"), &agent(&on_id)))["ownDefaultView"], "chatter");
    assert_eq!(ok(set(Some("thread"), &Caller::Shell { project_id: on_id.clone() }))["ownDefaultView"], "thread");
    assert!(ok(set(None, &Caller::Owner))["ownDefaultView"].is_null());
    assert_eq!(stored(), None);

    // The refusal is audited as the Member.
    let lines = audit_lines(&name);
    assert!(
        lines.iter().any(|e| e["event"] == "project-set-member-default-view"
            && e["user"] == Caller::Login { username: member.clone() }.audit_user().as_str()
            && e["outcome"].as_str().is_some_and(|o| o.starts_with("refused:forbidden"))),
        "no refused set-member-default-view line for the Member: {lines:?}"
    );

    assert_eq!(
        crate::routes::route_policy::lookup(MEMBER_VIEW_PATH).and_then(|r| r.post),
        Some(crate::routes::route_policy::Floor::Admin)
    );
    assert!(crate::routes::route_policy::post_allowed(MEMBER_VIEW_PATH));
    assert!(is_gated_mutation(MEMBER_VIEW_PATH));
    assert!(is_admin_only(MEMBER_VIEW_PATH));
}
