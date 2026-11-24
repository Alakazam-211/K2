//! "Manage K2 projects" (0162, 0.45.6): the per-agent switch, the
//! authenticated POST entry and its audit lines. A child of
//! `project_group_routes` (moved out unchanged for the line ratchet).

use super::*;
use crate::sidecar_routes::{AgentCaller, Caller};

fn insert_workspace(label: &str) -> (String, String, String) {
    let id = uuid::Uuid::new_v4().to_string();
    let name = format!("pgs-ws-{label}-{id}");
    let path = format!("/tmp/pgs-ws-{label}-{}-{id}", std::process::id());
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
        rusqlite::params![id, name, path],
    )
    .expect("insert project row");
    (id, name, path)
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
    assert!(hint.contains("Let this agent manage K2 projects"), "clear message: {hint}");
    assert!(hint.contains("still work"), "says reads + chat still work: {hint}");
}

/// Audit lines whose outcome names `project` (unique per test).
fn audit_lines(project: &str) -> Vec<serde_json::Value> {
    k2_core::auth_audit::tail(1000)
        .expect("audit tail")
        .into_iter()
        .filter(|e| e["outcome"].as_str().is_some_and(|o| o.contains(project)))
        .collect()
}

fn member_ids(group_id: &str) -> Vec<String> {
    project_groups::list_members(group_id)
        .expect("members")
        .into_iter()
        .map(|m| m.workspace_id)
        .collect()
}

#[test]
fn switch_defaults_on_and_an_agent_can_add_a_member_audited() {
    // The audit file lives under $HOME: hold the shared env lock (and a
    // home of our own) so a parallel TempHome test can't move it mid-test.
    let _home = crate::test_support::TempHome::new();
    let (a_id, _, _) = insert_workspace("on-agent");
    let (b_id, b_name, _) = insert_workspace("on-target");
    assert!(
        k2_core::workspace::settings::agents_can_manage_projects(&a_id),
        "0162 default is ON"
    );
    let name = format!("pgs-on-{}", uuid::Uuid::new_v4());
    let me = agent(&a_id);
    let created = ok(post("/cli/project-group/create", serde_json::json!({ "name": name }), &me));
    let gid = created["id"].as_str().expect("id").to_string();
    let added = ok(post(
        "/cli/project-group/add-member",
        serde_json::json!({ "group": name, "workspace": b_name }),
        &me,
    ));
    assert_eq!(added["workspaceId"], b_id.as_str());
    assert_eq!(member_ids(&gid), vec![b_id.clone()]);

    let lines = audit_lines(&name);
    let who = me.audit_user();
    assert!(who.starts_with("agent:"), "{who}");
    let create = lines
        .iter()
        .find(|e| e["event"] == "project-create")
        .unwrap_or_else(|| panic!("no project-create line: {lines:?}"));
    assert_eq!(create["user"], who.as_str(), "{create}");
    assert!(create["outcome"].as_str().unwrap().starts_with("ok "), "{create}");
    assert!(create["outcome"].as_str().unwrap().contains(&format!("id={gid}")), "{create}");
    assert!(create["ts"].as_str().is_some_and(|t| !t.is_empty()), "when: {create}");
    let add = lines
        .iter()
        .find(|e| e["event"] == "project-add-member")
        .unwrap_or_else(|| panic!("no project-add-member line: {lines:?}"));
    assert_eq!(add["user"], who.as_str(), "{add}");
    let outcome = add["outcome"].as_str().unwrap();
    assert!(outcome.starts_with("ok "), "{add}");
    assert!(outcome.contains(&format!("project={name:?}")), "{add}");
    assert!(outcome.contains("workspace="), "{add}");

    // Owner remove + delete are audited as the owner. (B is the PoC,
    // so remove a second member.)
    let (_, c_name, _) = insert_workspace("on-second");
    ok(post(
        "/cli/project-group/add-member",
        serde_json::json!({ "group": name, "workspace": c_name }),
        &me,
    ));
    ok(post(
        "/cli/project-group/remove-member",
        serde_json::json!({ "group": name, "workspace": c_name }),
        &Caller::Owner,
    ));
    assert_eq!(member_ids(&gid), vec![b_id.clone()]);
    ok(post("/cli/project-group/delete", serde_json::json!({ "group": name }), &Caller::Owner));
    let lines = audit_lines(&name);
    let rm = lines
        .iter()
        .find(|e| e["event"] == "project-remove-member")
        .unwrap_or_else(|| panic!("no project-remove-member line: {lines:?}"));
    assert_eq!(rm["user"], "owner", "{rm}");
    let del = lines
        .iter()
        .find(|e| e["event"] == "project-delete")
        .unwrap_or_else(|| panic!("no project-delete line: {lines:?}"));
    assert_eq!(del["user"], "owner", "{del}");
    assert!(
        del["outcome"].as_str().unwrap().contains(&format!("project={name:?}")),
        "delete names the project it removed: {del}"
    );
}

#[test]
fn switch_off_refuses_agent_mutations_but_not_owner_reads_or_chat() {
    // The audit file lives under $HOME: hold the shared env lock (and a
    // home of our own) so a parallel TempHome test can't move it mid-test.
    let _home = crate::test_support::TempHome::new();
    let (a_id, a_name, _) = insert_workspace("off-agent");
    let (b_id, b_name, _) = insert_workspace("off-target");
    let name = format!("pgs-off-{}", uuid::Uuid::new_v4());
    let owner = Caller::Owner;
    let created = ok(post("/cli/project-group/create", serde_json::json!({ "name": name }), &owner));
    let gid = created["id"].as_str().expect("id").to_string();
    // The agent's workspace is a member (and so the PoC).
    ok(post(
        "/cli/project-group/add-member",
        serde_json::json!({ "group": gid, "workspace": a_name }),
        &owner,
    ));

    k2_core::workspace::settings::set_agents_can_manage_projects(&a_id, false)
        .expect("switch off");
    assert!(!k2_core::workspace::settings::agents_can_manage_projects(&a_id));
    let me = agent(&a_id);

    // Every mutation except msg is refused.
    refused_off(&post(
        "/cli/project-group/create",
        serde_json::json!({ "name": format!("{name}-new") }),
        &me,
    ));
    refused_off(&post(
        "/cli/project-group/add-member",
        serde_json::json!({ "group": gid, "workspace": b_name }),
        &me,
    ));
    refused_off(&post(
        "/cli/project-group/remove-member",
        serde_json::json!({ "group": gid, "workspace": a_name }),
        &me,
    ));
    refused_off(&post(
        "/cli/project-group/rename",
        serde_json::json!({ "group": gid, "name": format!("{name}-renamed") }),
        &me,
    ));
    refused_off(&post("/cli/project-group/delete", serde_json::json!({ "group": gid }), &me));
    refused_off(&post(
        "/cli/project-group/pin",
        serde_json::json!({ "group": gid, "pinned": true }),
        &me,
    ));
    refused_off(&post(
        "/cli/project-group/set-poc",
        serde_json::json!({ "group": gid, "workspace": a_name }),
        &me,
    ));
    refused_off(&post(
        "/cli/project-group/set-color",
        serde_json::json!({ "group": gid, "color": "#112233" }),
        &me,
    ));
    // Nothing changed.
    assert_eq!(member_ids(&gid), vec![a_id.clone()]);
    assert!(project_groups::resolve_group(&format!("{name}-new")).is_err());
    assert_eq!(project_groups::get_group_by_id(&gid).expect("still there").name, name);

    // The refusal is audited as the agent.
    let lines = audit_lines(&name);
    let refused = lines
        .iter()
        .find(|e| {
            e["event"] == "project-add-member"
                && e["outcome"]
                    .as_str()
                    .is_some_and(|o| o.starts_with("refused:agent_permission_off"))
        })
        .unwrap_or_else(|| panic!("no refused add-member line: {lines:?}"));
    assert_eq!(refused["user"], me.audit_user().as_str(), "{refused}");

    // Reads still work (the read routes take no caller; passports
    // reach them via is_agent_verb, asserted below).
    let shown = dispatch(
        "/cli/project-group/show",
        &[("group".to_string(), gid.clone())].into_iter().collect(),
    )
    .expect("show claimed");
    assert_eq!(shown.status, "200 OK", "{}", shown.body);
    assert!(crate::session_token::is_agent_verb("/cli/project-group/show"));
    // Posting in its own project chat still works.
    ok(post(
        "/cli/project-group/msg",
        serde_json::json!({ "group": gid, "body": "still here", "author": a_name }),
        &me,
    ));
    // A person in a K2 shell tab of that workspace is not the agent.
    ok(post(
        "/cli/project-group/pin",
        serde_json::json!({ "group": gid, "pinned": false }),
        &Caller::Shell { project_id: a_id.clone() },
    ));
    // The owner still can.
    let added = ok(post(
        "/cli/project-group/add-member",
        serde_json::json!({ "group": gid, "workspace": b_name }),
        &owner,
    ));
    assert_eq!(added["workspaceId"], b_id.as_str());

    // Back on: the agent may mutate again.
    k2_core::workspace::settings::set_agents_can_manage_projects(&a_id, true).expect("on");
    ok(post(
        "/cli/project-group/remove-member",
        serde_json::json!({ "group": gid, "workspace": b_name }),
        &me,
    ));
}

/// 0.46.2 (Rosson 2026-10-18): an agent reads and posts only in the
/// chats of projects its workspace is a member of, and posts as its own
/// workspace. The PoC is a member like any other. Owner, Connect login
/// and shell tab are unchanged; API sessions are refused.
#[test]
fn project_chat_is_for_member_agents_only() {
    // Create/add/remove/delete write audit lines under $HOME: a home of our own.
    let _home = crate::test_support::TempHome::new();
    let (poc_id, poc_name, _) = insert_workspace("chat-poc");
    let (mem_id, mem_name, _) = insert_workspace("chat-member");
    let (out_id, out_name, _) = insert_workspace("chat-outsider");
    let name = format!("pgs-chat-{}", uuid::Uuid::new_v4());
    let owner = Caller::Owner;
    let gid = ok(post("/cli/project-group/create", serde_json::json!({ "name": name }), &owner))["id"]
        .as_str()
        .expect("id")
        .to_string();
    for ws in [&poc_name, &mem_name] {
        ok(post("/cli/project-group/add-member", serde_json::json!({ "group": gid, "workspace": ws }), &owner));
    }
    assert_eq!(project_groups::get_poc(&gid).expect("poc").as_deref(), Some(poc_id.as_str()), "first member is the PoC");
    project_groups::post_message(&gid, "owner", "secret-line-1").expect("seed");

    let read_params = |extra: &[(&str, &str)]| -> HashMap<String, String> {
        let mut p: HashMap<String, String> = [("group".to_string(), gid.clone())].into_iter().collect();
        for (k, v) in extra {
            p.insert(k.to_string(), v.to_string());
        }
        p
    };
    let reads: [&[(&str, &str)]; 3] = [&[], &[("after_seq", "0")], &[("before_seq", "100")]];

    // Readers the gate lets through, on every read form.
    let login = Caller::Login { username: "gina".to_string() };
    let shell = Caller::Shell { project_id: out_id.clone() };
    for (who, c) in [("owner", &owner), ("login", &login), ("shell", &shell), ("poc agent", &agent(&poc_id)), ("member agent", &agent(&mem_id))] {
        for extra in reads {
            assert!(chat_read_refusal(MESSAGES_PATH, &read_params(extra), c).is_none(), "{who} {extra:?} reads");
        }
    }

    // The outsider agent: the same 403 `not_a_member` on every read
    // form, with no message text and no member names.
    let outsider = agent(&out_id);
    let mut bodies = Vec::new();
    for extra in reads {
        let r = chat_read_refusal(MESSAGES_PATH, &read_params(extra), &outsider)
            .unwrap_or_else(|| panic!("outsider {extra:?} must be refused"));
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("JSON");
        assert_eq!((v["ok"].as_bool(), v["error"]["code"].as_str()), (Some(false), Some("not_a_member")), "{v}");
        for leak in ["secret-line", poc_name.as_str(), mem_name.as_str()] {
            assert!(!r.body.contains(leak), "refusal leaks {leak}: {}", r.body);
        }
        bodies.push(r.body);
    }
    assert!(bodies.windows(2).all(|w| w[0] == w[1]), "one refusal for every read form: {bodies:?}");
    // Only the chat read is gated; an unknown project is the read's own 404.
    assert!(chat_read_refusal("/cli/project-group/show", &read_params(&[]), &outsider).is_none());
    let mut unknown = read_params(&[]);
    unknown.insert("group".to_string(), "no-such-project-zz".to_string());
    assert!(chat_read_refusal(MESSAGES_PATH, &unknown, &outsider).is_none());
    // An API session's passport is refused with the read arm's 403.
    let r = chat_read_refusal(MESSAGES_PATH, &read_params(&[]), &Caller::WrongCredential).expect("api refused");
    assert_eq!((r.status, r.body.as_str()), ("403 Forbidden", CliResponse::forbidden().body.as_str()));

    // Posting. A member agent posts as itself even when the body names
    // the owner or another member.
    let posted = ok(post(
        "/cli/project-group/msg",
        serde_json::json!({ "group": gid, "body": "member line", "author": "owner" }),
        &agent(&mem_id),
    ));
    assert_eq!((posted["author"].as_str(), posted["authorKind"].as_str()), (Some(mem_name.as_str()), Some("agent")), "{posted}");
    let posted = ok(post(
        "/cli/project-group/msg",
        serde_json::json!({ "group": gid, "body": "as the poc?", "author": poc_name }),
        &agent(&mem_id),
    ));
    assert_eq!(posted["author"], mem_name.as_str(), "no posting as another member: {posted}");
    let posted = ok(post("/cli/project-group/msg", serde_json::json!({ "group": gid, "body": "no author" }), &agent(&mem_id)));
    assert_eq!(posted["author"], mem_name.as_str(), "{posted}");
    // The PoC posts too (and never self-injects).
    let posted = ok(post(
        "/cli/project-group/msg",
        serde_json::json!({ "group": gid, "body": "poc line", "author": poc_name }),
        &agent(&poc_id),
    ));
    assert_eq!((posted["author"].as_str(), posted["deliveryReason"].as_str()), (Some(poc_name.as_str()), Some("author-is-poc")), "{posted}");

    // The outsider is refused whatever author it names, and nothing is stored.
    let before = project_groups::list_messages(&gid, None, Some(500)).expect("list").messages.len();
    for author in [None, Some("owner"), Some(poc_name.as_str()), Some(out_name.as_str())] {
        let mut body = serde_json::json!({ "group": gid, "body": "let me in" });
        if let Some(a) = author {
            body["author"] = serde_json::json!(a);
        }
        let r = post("/cli/project-group/msg", body, &outsider);
        assert_eq!(r.status, "403 Forbidden", "author {author:?}: {}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("JSON");
        assert_eq!(v["error"]["code"], "not_a_member", "{v}");
        assert_eq!(r.body, bodies[0], "the read's refusal");
    }
    assert_eq!(project_groups::list_messages(&gid, None, Some(500)).expect("list").messages.len(), before, "refused posts never store");

    // People are unchanged: the owner, a login and a shell tab post as before.
    assert_eq!(ok(post("/cli/project-group/msg", serde_json::json!({ "group": gid, "body": "o" }), &owner))["author"], "owner");
    assert_eq!(ok(post("/cli/project-group/msg", serde_json::json!({ "group": gid, "body": "g" }), &login))["author"], "gina");
    ok(post("/cli/project-group/msg", serde_json::json!({ "group": gid, "body": "s" }), &shell));
    // App passes / API sessions stay refused.
    let r = post("/cli/project-group/msg", serde_json::json!({ "group": gid, "body": "x" }), &Caller::WrongCredential);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);

    // Removed from the project → refused from then on.
    ok(post("/cli/project-group/remove-member", serde_json::json!({ "group": gid, "workspace": mem_name }), &owner));
    assert!(chat_read_refusal(MESSAGES_PATH, &read_params(&[]), &agent(&mem_id)).is_some(), "an ex-member is refused");
    let r = post("/cli/project-group/msg", serde_json::json!({ "group": gid, "body": "still?" }), &agent(&mem_id));
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    ok(post("/cli/project-group/delete", serde_json::json!({ "group": gid }), &owner));
}

#[test]
fn wrong_credentials_are_refused_and_non_admin_logins_cannot_change_appearance() {
    let name = format!("pgs-cred-{}", uuid::Uuid::new_v4());
    for c in [Caller::WrongCredential, Caller::Invalid] {
        let r = post("/cli/project-group/create", serde_json::json!({ "name": name }), &c);
        assert_eq!(r.status, "403 Forbidden", "{c:?}: {}", r.body);
        let r = post(
            "/cli/project-group/msg",
            serde_json::json!({ "group": name, "body": "x" }),
            &c,
        );
        assert_eq!(r.status, "403 Forbidden", "{c:?}: {}", r.body);
    }
    assert!(project_groups::resolve_group(&name).is_err());
    // A login that does not exist is not an admin.
    let created = ok(post(
        "/cli/project-group/create",
        serde_json::json!({ "name": name }),
        &Caller::Owner,
    ));
    let r = post(
        "/cli/project-group/set-color",
        serde_json::json!({ "group": created["id"], "color": "#112233" }),
        &Caller::Login { username: format!("nobody-{}", uuid::Uuid::new_v4()) },
    );
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert!(r.body.contains("owner or an admin"), "{}", r.body);
}

/// Deleting a project (and its chat), Rosson 2026-10-18: a Member
/// login is refused and the project survives; an agent without 'Manage
/// K2 projects' is refused; an agent with it, an admin login, a person
/// in a shell tab and the owner token may delete. Renaming stays open
/// to a Member.
#[test]
fn members_cannot_delete_a_project_but_admins_and_managing_agents_can() {
    let _home = crate::test_support::TempHome::new();
    use k2_core::connect_users::{add_user, role_for_user, set_role, Role};
    let member = format!("pgdm{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let admin = format!("pgda{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    add_user(&member, "password123").expect("member");
    add_user(&admin, "password123").expect("admin");
    set_role(&admin, Role::Admin).expect("make admin");
    assert_eq!(role_for_user(&member), Some(Role::Member));
    let (a_id, _, _) = insert_workspace("del-agent");
    assert!(k2_core::workspace::settings::agents_can_manage_projects(&a_id));

    let mut gids = Vec::new();
    for i in 0..4 {
        let name = format!("pgs-del-{i}-{}", uuid::Uuid::new_v4());
        let c = ok(post("/cli/project-group/create", serde_json::json!({ "name": name }), &Caller::Owner));
        gids.push(c["id"].as_str().expect("id").to_string());
    }

    let as_member = Caller::Login { username: member.clone() };
    let r = post("/cli/project-group/delete", serde_json::json!({ "group": gids[0] }), &as_member);
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert!(r.body.contains("owner, an admin, or an agent with 'Let this agent manage K2 projects' on"), "{}", r.body);
    // An agent without the switch is refused (agent_permission_off).
    let (off_id, _, _) = insert_workspace("del-agent-off");
    k2_core::workspace::settings::set_agents_can_manage_projects(&off_id, false).expect("switch off");
    let r = post("/cli/project-group/delete", serde_json::json!({ "group": gids[0] }), &agent(&off_id));
    assert_eq!(r.status, "403 Forbidden", "agent with the switch off: {}", r.body);
    assert!(r.body.contains("agent_permission_off"), "{}", r.body);
    assert!(project_groups::get_group_by_id(&gids[0]).is_some(), "refusals delete nothing");
    // An agent with the switch on may delete.
    ok(post("/cli/project-group/delete", serde_json::json!({ "group": gids[3] }), &agent(&a_id)));
    // A Member may still rename.
    ok(post(
        "/cli/project-group/rename",
        serde_json::json!({ "group": gids[0], "name": format!("pgs-del-renamed-{}", uuid::Uuid::new_v4()) }),
        &as_member,
    ));

    ok(post("/cli/project-group/delete", serde_json::json!({ "group": gids[0] }), &Caller::Login { username: admin }));
    ok(post("/cli/project-group/delete", serde_json::json!({ "group": gids[1] }), &Caller::Shell { project_id: a_id.clone() }));
    ok(post("/cli/project-group/delete", serde_json::json!({ "group": gids[2] }), &Caller::Owner));
    for gid in &gids {
        assert!(project_groups::get_group_by_id(gid).is_none(), "{gid} deleted");
    }
    // The route policy floor agrees with the arm.
    for path in [
        "/cli/project-group/delete",
        "/cli/project-group/dashboard/add-pane",
        "/cli/project-group/dashboard/remove-pane",
    ] {
        assert_eq!(
            crate::routes::route_policy::lookup(path).and_then(|r| r.post),
            Some(crate::routes::route_policy::Floor::Admin),
            "{path}"
        );
    }
}

/// prd-projects-threads-v1 §4.2: a project's default view changes what
/// everyone sees, so among logins only the owner or an admin may set it
/// (a Member gets 403 forbidden and nothing changes). An agent follows
/// its 'Manage K2 projects' switch: on, it may; off, 403
/// agent_permission_off. The owner token and a person in a shell tab
/// may. The route policy floor (Admin) and the switch agree with the arm.
#[test]
fn only_admins_and_managing_agents_set_a_projects_default_view() {
    let _home = crate::test_support::TempHome::new();
    use k2_core::connect_users::{add_user, role_for_user, set_role, Role};
    let member = format!("pgvm{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let admin = format!("pgva{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    add_user(&member, "password123").expect("member");
    add_user(&admin, "password123").expect("admin");
    set_role(&admin, Role::Admin).expect("make admin");
    assert_eq!(role_for_user(&member), Some(Role::Member));
    let (on_id, _, _) = insert_workspace("view-agent-on");
    assert!(k2_core::workspace::settings::agents_can_manage_projects(&on_id), "switch defaults on");
    let (off_id, _, _) = insert_workspace("view-agent-off");
    k2_core::workspace::settings::set_agents_can_manage_projects(&off_id, false).expect("switch off");

    let name = format!("pgs-view-{}", uuid::Uuid::new_v4());
    let created = ok(post("/cli/project-group/create", serde_json::json!({ "name": name }), &Caller::Owner));
    let gid = created["id"].as_str().expect("id").to_string();
    let set = |view: &str, caller: &Caller| {
        post(SET_DEFAULT_VIEW_PATH, serde_json::json!({ "group": gid, "view": view }), caller)
    };
    let stored = || project_groups::get_group_by_id(&gid).expect("group").default_view;

    // A Member login: forbidden, with a clear hint; nothing changes.
    let r = set("terminal", &Caller::Login { username: member.clone() });
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("JSON body");
    assert_eq!(v["error"]["code"], "forbidden", "{v}");
    let hint = v["error"]["hint"].as_str().expect("hint");
    assert!(hint.contains("owner, an admin, or an agent with 'Let this agent manage K2 projects' on"), "{hint}");
    assert!(hint.contains("view a project opens its agents in"), "{hint}");
    // A login that does not exist is not an admin either.
    let r = set("terminal", &Caller::Login { username: format!("nobody-{}", uuid::Uuid::new_v4()) });
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    // An agent with the switch off: agent_permission_off.
    refused_off(&set("terminal", &agent(&off_id)));
    // App passes and bad tokens: refused.
    for c in [Caller::WrongCredential, Caller::Invalid] {
        assert_eq!(set("terminal", &c).status, "403 Forbidden", "{c:?}");
    }
    assert_eq!(stored(), "thread", "refusals change nothing");

    // An admin login, an agent with the switch on, a shell tab, the owner.
    assert_eq!(ok(set("terminal", &Caller::Login { username: admin.clone() }))["defaultView"], "terminal");
    assert_eq!(ok(set("chatter", &agent(&on_id)))["defaultView"], "chatter");
    assert_eq!(ok(set("terminal", &Caller::Shell { project_id: on_id.clone() }))["defaultView"], "terminal");
    assert_eq!(ok(set("thread", &Caller::Owner))["defaultView"], "thread");
    assert_eq!(stored(), "thread");

    // A bad view from an allowed caller is still usage.
    let r = set("split", &Caller::Login { username: admin });
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);

    // The refusal is audited as the Member.
    let lines = audit_lines(&name);
    assert!(
        lines.iter().any(|e| e["event"] == "project-set-default-view"
            && e["user"] == Caller::Login { username: member.clone() }.audit_user().as_str()
            && e["outcome"].as_str().is_some_and(|o| o.starts_with("refused:forbidden"))),
        "no refused set-default-view line for the Member: {lines:?}"
    );

    // The wiring agrees: Admin floor, behind the switch, owner-or-admin.
    assert_eq!(
        crate::routes::route_policy::lookup(SET_DEFAULT_VIEW_PATH).and_then(|r| r.post),
        Some(crate::routes::route_policy::Floor::Admin)
    );
    assert!(crate::routes::route_policy::post_allowed(SET_DEFAULT_VIEW_PATH));
    assert!(is_gated_mutation(SET_DEFAULT_VIEW_PATH));
    assert!(is_admin_only(SET_DEFAULT_VIEW_PATH));
}

/// Two clients add a pane to the same dashboard at the same time:
/// both panes land (the old whole-layout save lost one). Remove takes
/// one pane off, the rest stay; non-members and non-HTML files are
/// refused; a Member login is refused; repeats are no-ops.
#[test]
fn dashboard_add_and_remove_pane_merge_concurrent_clients() {
    let _home = crate::test_support::TempHome::new();
    let (poc_id, poc_name, _) = insert_workspace("pane-poc");
    let mut members = Vec::new();
    let name = format!("pgs-pane-{}", uuid::Uuid::new_v4());
    let created = ok(post("/cli/project-group/create", serde_json::json!({ "name": name }), &Caller::Owner));
    let gid = created["id"].as_str().expect("id").to_string();
    ok(post("/cli/project-group/add-member", serde_json::json!({ "group": gid, "workspace": poc_name }), &Caller::Owner));
    for i in 0..6 {
        let (id, wname, _) = insert_workspace(&format!("pane-{i}"));
        ok(post("/cli/project-group/add-member", serde_json::json!({ "group": gid, "workspace": wname }), &Caller::Owner));
        members.push((id, wname));
    }
    let dash = project_groups::list_dashboards(&gid).expect("dashboards").remove(0);
    let start = dash.revision;

    // Six clients at once, three adding agents and three adding files.
    let handles: Vec<_> = members
        .iter()
        .enumerate()
        .map(|(i, (id, _))| {
            let pane = if i % 2 == 0 {
                serde_json::json!({ "kind": "terminal", "workspaceId": id })
            } else {
                serde_json::json!({ "kind": "htmlDoc", "workspaceId": id, "filePath": format!("/tmp/doc-{i}.html") })
            };
            let body = serde_json::json!({ "group": gid, "dashboardId": dash.id, "pane": pane });
            std::thread::spawn(move || post("/cli/project-group/dashboard/add-pane", body, &Caller::Owner))
        })
        .collect();
    for h in handles {
        let v = ok(h.join().expect("thread"));
        assert_eq!(v["changed"], true, "{v}");
    }
    let after = project_groups::get_dashboard(&dash.id).expect("dashboard");
    assert_eq!(after.revision, start + 6, "{}", after.layout_json);
    for (i, (id, _)) in members.iter().enumerate() {
        assert!(after.layout_json.contains(id.as_str()), "pane {i} lost: {}", after.layout_json);
    }
    assert!(after.layout_json.contains(&poc_id), "the PoC seed was kept: {}", after.layout_json);

    // Repeat add is a no-op; no revision bump.
    let again = ok(post(
        "/cli/project-group/dashboard/add-pane",
        serde_json::json!({ "group": gid, "dashboardId": dash.id, "pane": { "kind": "terminal", "workspace": members[0].1 } }),
        &Caller::Owner,
    ));
    assert_eq!(again["changed"], false, "{again}");
    assert_eq!(again["revision"], after.revision);

    // Remove one terminal pane; everything else stays.
    let removed = ok(post(
        "/cli/project-group/dashboard/remove-pane",
        serde_json::json!({ "group": gid, "dashboardId": dash.id, "pane": { "kind": "terminal", "workspaceId": members[0].0 } }),
        &Caller::Owner,
    ));
    assert_eq!(removed["changed"], true, "{removed}");
    let layout = removed["layoutJson"].as_str().expect("layoutJson");
    assert!(!layout.contains(members[0].0.as_str()), "{layout}");
    for (id, _) in &members[1..] {
        assert!(layout.contains(id.as_str()), "{layout}");
    }
    let gone = ok(post(
        "/cli/project-group/dashboard/remove-pane",
        serde_json::json!({ "group": gid, "dashboardId": dash.id, "pane": { "kind": "terminal", "workspaceId": members[0].0 } }),
        &Caller::Owner,
    ));
    assert_eq!(gone["changed"], false, "{gone}");

    // Refusals: a non-member, a non-HTML file, a Member login.
    let (_, outsider, _) = insert_workspace("pane-outsider");
    let r = post(
        "/cli/project-group/dashboard/add-pane",
        serde_json::json!({ "group": gid, "dashboardId": dash.id, "pane": { "kind": "terminal", "workspace": outsider } }),
        &Caller::Owner,
    );
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert!(r.body.contains("not_a_member"), "{}", r.body);
    let r = post(
        "/cli/project-group/dashboard/add-pane",
        serde_json::json!({ "group": gid, "dashboardId": dash.id, "pane": { "kind": "htmlDoc", "workspaceId": members[1].0, "filePath": "/tmp/notes.md" } }),
        &Caller::Owner,
    );
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    let member = format!("pgpm{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    k2_core::connect_users::add_user(&member, "password123").expect("member");
    let r = post(
        "/cli/project-group/dashboard/add-pane",
        serde_json::json!({ "group": gid, "dashboardId": dash.id, "pane": { "kind": "terminal", "workspaceId": members[0].0 } }),
        &Caller::Login { username: member },
    );
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    let r = post(
        "/cli/project-group/dashboard/add-pane",
        serde_json::json!({ "group": gid, "dashboardId": "nope", "pane": { "kind": "terminal", "workspaceId": members[0].0 } }),
        &Caller::Owner,
    );
    assert_eq!(r.status, "404 Not Found", "{}", r.body);
}

/// The switch is written only by the owner/admin route; an agent can
/// never flip it (its own or another's), and that route is not an
/// agent verb, while the project routes are.
#[test]
fn an_agent_cannot_flip_the_switch_but_the_owner_can() {
    let (a_id, _, a_path) = insert_workspace("flip");
    k2_core::workspace::settings::set_agents_can_manage_projects(&a_id, false).expect("off");
    let on = serde_json::json!({ "workspace": a_path, "toggle": "projects_manage", "value": true })
        .to_string();
    let params = HashMap::new();
    for c in [agent(&a_id), Caller::WrongCredential] {
        let r = crate::sidecar_routes::handle(
            "/cli/agent-access/set",
            c.clone(),
            on.as_bytes(),
            &params,
            "test",
        );
        assert_eq!(r.status, "403 Forbidden", "{c:?}: {}", r.body);
    }
    assert!(
        !k2_core::workspace::settings::agents_can_manage_projects(&a_id),
        "a refused flip changes nothing"
    );
    assert!(!crate::session_token::is_agent_verb("/cli/agent-access/set"));
    assert!(crate::session_token::is_agent_verb("/cli/project-group/add-member"));
    assert!(crate::session_token::is_agent_verb("/cli/project-group/list"));

    let r = crate::sidecar_routes::handle(
        "/cli/agent-access/set",
        Caller::Owner,
        on.as_bytes(),
        &params,
        "test",
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!(v["agentsCanManageProjects"], true, "{v}");
    assert!(k2_core::workspace::settings::agents_can_manage_projects(&a_id));
    let off = serde_json::json!({ "workspace": a_path, "toggle": "projects_manage", "value": false })
        .to_string();
    let r = crate::sidecar_routes::handle(
        "/cli/agent-access/set",
        Caller::Owner,
        off.as_bytes(),
        &params,
        "test",
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(!k2_core::workspace::settings::agents_can_manage_projects(&a_id));
}

/// Every gated mutation is POST-only (GET → 405 on the read chain and
/// in the route policy), and `msg` is the only POST the switch leaves
/// open.
#[test]
fn gated_mutations_are_post_only() {
    let params = HashMap::new();
    let posts = [
        "/cli/project-group/create",
        "/cli/project-group/rename",
        "/cli/project-group/delete",
        "/cli/project-group/pin",
        "/cli/project-group/sort",
        "/cli/project-group/add-member",
        "/cli/project-group/remove-member",
        "/cli/project-group/set-poc",
        "/cli/project-group/set-icon",
        "/cli/project-group/set-color",
        "/cli/project-group/set-default-view",
        "/cli/project-group/dashboard/save-layout",
        "/cli/project-group/dashboard/rename",
        "/cli/project-group/dashboard/create",
        "/cli/project-group/dashboard/delete",
        "/cli/project-group/dashboard/reorder",
        "/cli/project-group/dashboard/add-pane",
        "/cli/project-group/dashboard/remove-pane",
    ];
    for route in posts {
        assert!(is_gated_mutation(route), "{route} must be behind the switch");
        let resp = dispatch(route, &params).expect("route claimed by GET chain");
        assert_eq!(resp.status, "405 Method Not Allowed", "route={route}");
        assert!(
            crate::routes::route_policy::get_refused_params(route, &params),
            "route policy refuses GET on {route}"
        );
    }
    assert!(!is_gated_mutation("/cli/project-group/msg"));
    for read in [
        "/cli/project-group/list",
        "/cli/project-group/show",
        "/cli/project-group/messages",
    ] {
        assert!(
            !crate::routes::route_policy::get_refused_params(read, &params),
            "{read} is a read"
        );
    }
}
