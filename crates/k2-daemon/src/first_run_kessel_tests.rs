//! Kessel at first run, and the Chief of Staff (prd-setup-checklist-v1 §10,
//! `kessel-first-run-contract.md`): the four routes on a temp home and a
//! per-thread database. Agent sessions are built as the dispatcher's
//! classifier builds them (`sidecar_routes::Caller`). Fail loudly.

use std::collections::HashMap;

use k2_core::hire::starter_permissions::{standing_switch, CHIEF_OF_STAFF_STARTER_PERMISSIONS, KESSEL_STARTER_PERMISSIONS};
use serde_json::{json, Value};

use super::*;
use crate::sidecar_routes::{AgentCaller, Caller};

struct Env {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
}

/// A temp home and database with `kesselFirstRun` as given, and these tools
/// signed in.
fn env(flag: bool, tools: &[&str]) -> Env {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    crate::vault::ensure_test_vault();
    k2_core::app_settings::update(json!({ "kesselFirstRun": flag })).expect("flag");
    for t in tools {
        sign_in(t);
    }
    TEST_LAUNCHES.lock().clear();
    Env { _db: db, _home: home }
}

fn sign_in(tool: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO llm_accounts (id, tool, label, kind, created_at, state) VALUES (?1, ?2, 'work', 'subscription', 1, 'signed_in')",
        rusqlite::params![format!("acct-{tool}"), tool],
    )
    .expect("sign in");
}

fn login(name: &str, role: k2_core::connect_users::Role) -> Actor {
    k2_core::connect_users::add_user(name, "password123").expect("add");
    if role != k2_core::connect_users::Role::Member {
        k2_core::connect_users::set_role(name, role).expect("role");
    }
    Actor::login(name)
}

fn json_of(r: &CliResponse) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{}: {e}: {}", r.status, r.body))
}

fn code(r: &CliResponse) -> String {
    json_of(r)["error"]["code"].as_str().unwrap_or_else(|| panic!("a code: {} {}", r.status, r.body)).to_string()
}

fn post(path: &str, body: Value, actor: &Actor) -> CliResponse {
    handle(true, path, body.to_string().as_bytes(), actor)
}

fn status_for(actor: &Actor) -> Value {
    let r = handle(false, STATUS_PATH, b"", actor);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    json_of(&r)
}

fn default_agent(id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT default_agent FROM projects WHERE id = ?1", [id], |r| r.get(0)).expect("project")
}

fn agent_mode(id: &str) -> (String, i64) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT agent_mode, agent_enabled FROM projects WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).expect("project")
}

/// Kessel made by the owner token: `(workspace id, path, handle)`.
fn make_kessel() -> (String, String, String) {
    let r = post(KESSEL_PATH, json!({ "tool": "codex" }), &Actor::owner_token());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v = json_of(&r);
    (
        v["workspaceId"].as_str().expect("id").to_string(),
        v["path"].as_str().expect("path").to_string(),
        v["handle"].as_str().expect("handle").to_string(),
    )
}

/// Kessel's pinned chat, as the dispatcher classifies its passport.
fn kessel_session(kessel: &str) -> Caller {
    Caller::Agent(AgentCaller { project_id: kessel.to_string(), map_key: Some(kessel.to_string()), args: Vec::new(), sandboxed: false })
}

fn cos_body() -> Vec<u8> {
    json!({ "name": "Max", "about": "Runs a small bakery", "tool": "codex" }).to_string().into_bytes()
}

fn on(id: &str, action: &str) -> bool {
    crate::first_run_grants::switch_on(id, standing_switch(action).expect("standing"))
}

#[test]
fn every_kessel_route_is_404_with_the_flag_off_and_the_status_carries_nothing_of_kessel() {
    let _e = env(false, &["codex"]);
    for p in [KESSEL_PATH, COS_START_PATH, OFFER_DISMISS_PATH] {
        let r = post(p, json!({}), &Actor::owner_token());
        assert_eq!((r.status, code(&r).as_str()), ("404 Not Found", "kessel_first_run_off"), "{p}");
    }
    let r = handle_chief_of_staff(&cos_body(), &kessel_session("w-any"));
    assert_eq!((r.status, code(&r).as_str()), ("404 Not Found", "kessel_first_run_off"));
    let v = status_for(&Actor::owner_token());
    for k in ["offer", "kessel", "kesselDefaults"] {
        assert!(v.get(k).is_none(), "{k}: {v}");
    }
    let ids: Vec<&str> = v["rows"].as_array().expect("rows").iter().map(|r| r["id"].as_str().expect("id")).collect();
    assert!(ids.contains(&"first_agent") && !ids.contains(&"chief_of_staff"), "{ids:?}");
}

#[test]
fn the_kessel_route_makes_the_seat_workspace_harness_pinned_chat_cards_and_grants() {
    let _e = env(true, &["codex"]);
    let mut rx = crate::session_events::sender().subscribe();
    let v = status_for(&Actor::owner_token());
    assert_eq!(v["offer"], json!("meet_kessel"), "{v}");
    assert_eq!(v["kessel"], Value::Null);
    assert!(v["kesselDefaults"]["path"].as_str().expect("path").ends_with("k2-agents/kessel"), "{v}");

    let r = post(KESSEL_PATH, json!({ "tool": "codex" }), &Actor::owner_token());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let out = json_of(&r);
    let id = out["workspaceId"].as_str().expect("id").to_string();
    let path = out["path"].as_str().expect("path").to_string();
    assert_eq!(out["name"], json!("Kessel"));
    assert!(path.ends_with("k2-agents/kessel"), "{path}");

    // The seat, the harness and pinned chat.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let seat = k2_core::orchestrator::live_kessel(&conn).expect("seat").expect("live");
        assert_eq!((seat.workspace_id.as_str(), seat.name.as_str()), (id.as_str(), "Kessel"));
        assert_eq!(k2_core::orchestrator::load(&conn).expect("row").expect("row").created_by.as_deref(), Some("owner"));
    }
    assert_eq!(default_agent(&id).as_deref(), Some("codex"), "the harness");
    assert_eq!(agent_mode(&id), ("custom".to_string(), 1), "pinned chat on");
    // Its own cards switch, and every Kessel starter switch (the owner holds all).
    assert!(k2_core::workspace::settings::agents_can_request_permissions(&id), "Let this agent ask for permission");
    let granted: Vec<&str> = out["permissions"]["granted"].as_array().expect("granted").iter().map(|g| g.as_str().expect("id")).collect();
    let mut want = KESSEL_STARTER_PERMISSIONS.to_vec();
    want.push("chat.refresh");
    assert_eq!(granted, want, "the list, then the chat_refresh toggle beside it");
    for a in &want {
        assert!(on(&id, a), "{a} on for Kessel");
    }
    // The launch with its first-start hello.
    let launches = TEST_LAUNCHES.lock().clone();
    assert_eq!(launches.len(), 1, "{launches:?}");
    assert!(launches[0].1.as_deref().is_some_and(|h| h.contains("first start")), "{launches:?}");
    // Events: first_run_changed to everyone, and projects_changed.
    let (mut changed, mut projects) = (false, false);
    while let Ok(ev) = rx.try_recv() {
        match ev {
            crate::session_events::SessionEvent::FirstRunChanged { person: None } => changed = true,
            crate::session_events::SessionEvent::ProjectsChanged {} => projects = true,
            _ => {}
        }
    }
    assert!(changed && projects, "first_run_changed {changed}, projects_changed {projects}");

    // The status now names Kessel; the offer is gone; a second call is 409.
    let v = status_for(&Actor::owner_token());
    assert_eq!(v["offer"], Value::Null);
    assert_eq!(v["kessel"]["workspaceId"], json!(id));
    let r = post(KESSEL_PATH, json!({ "tool": "codex" }), &Actor::owner_token());
    assert_eq!((r.status, code(&r).as_str()), ("409 Conflict", "kessel_exists"));
}

#[test]
fn an_admin_creator_leaves_out_organization_manage_and_kessel_asks_an_owner_for_it() {
    let _e = env(true, &["claude"]);
    let julie = login("julie", k2_core::connect_users::Role::Admin);
    // No tool given: the server default (claude), which is signed in.
    let r = post(KESSEL_PATH, json!({}), &julie);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let out = json_of(&r);
    let id = out["workspaceId"].as_str().expect("id").to_string();
    assert_eq!(default_agent(&id).as_deref(), Some("claude"));
    assert!(!on(&id, "organization.manage"), "an Admin can't give an Owner-floor switch");
    assert_eq!(out["permissions"]["left"], json!([{ "action": "organization.manage", "reason": "owner_floor" }]));
    for a in KESSEL_STARTER_PERMISSIONS.iter().filter(|a| **a != "organization.manage") {
        assert!(on(&id, a), "{a} granted by an Admin");
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert_eq!(k2_core::orchestrator::load(&conn).expect("row").expect("row").created_by.as_deref(), Some("login:julie"));
}

#[test]
fn kessel_needs_a_signed_in_tool_and_an_admin_or_better() {
    let _e = env(true, &["codex"]);
    for (body, want) in [
        (json!({ "tool": "grok" }), "tool_not_signed_in"),
        (json!({ "tool": "gemini" }), "usage"),
        (json!({ "tool": 3 }), "usage"),
        (json!({ "name": "x".repeat(MAX_NAME + 1) }), "usage"),
    ] {
        let r = post(KESSEL_PATH, body.clone(), &Actor::owner_token());
        assert_eq!((r.status, code(&r).as_str()), ("400 Bad Request", want), "{body}");
    }
    let anna = login("anna", k2_core::connect_users::Role::Member);
    let r = post(KESSEL_PATH, json!({ "tool": "codex" }), &anna);
    assert_eq!((r.status, code(&r).as_str()), ("403 Forbidden", "role_required"), "a Member");
    let r = post(COS_START_PATH, json!({}), &anna);
    assert_eq!(r.status, "403 Forbidden", "a Member on start");
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert_eq!(k2_core::orchestrator::load(&conn).expect("seat"), None, "nothing was seated");
}

#[test]
fn the_offer_is_dismissed_per_person() {
    let _e = env(true, &["codex"]);
    let julie = login("julie", k2_core::connect_users::Role::Admin);
    let r = post(OFFER_DISMISS_PATH, json!({}), &Actor::owner_token());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(json_of(&r)["offer"], Value::Null, "the owner said not now");
    assert_eq!(status_for(&julie)["offer"], json!("meet_kessel"), "julie didn't");
    let anna = login("anna", k2_core::connect_users::Role::Member);
    assert_eq!(status_for(&anna)["offer"], Value::Null, "a Member never gets it");
}

#[test]
fn start_posts_into_kessels_thread_as_the_caller() {
    let _e = env(true, &["codex"]);
    let julie = login("julie", k2_core::connect_users::Role::Admin);
    let r = post(COS_START_PATH, json!({}), &julie);
    assert_eq!((r.status, code(&r).as_str()), ("409 Conflict", "needs_kessel"));
    let (kessel, _, handle) = make_kessel();
    // Kessel's pinned chat (its launch would pin it).
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let conv = uuid::Uuid::new_v4().to_string();
        k2_core::db::schema::WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &kessel, None, Some(&conv), "codex", "system", "running")
            .expect("pin");
    }
    let r = post(COS_START_PATH, json!({}), &julie);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(json_of(&r)["workspaceId"], json!(kessel));
    let q: HashMap<String, String> = [("addr".to_string(), handle.clone())].into();
    let t: Value = serde_json::from_str(&crate::overlay_routes::dispatch("/cli/thread", &q).expect("thread").body).expect("json");
    let items = t["items"].as_array().unwrap_or_else(|| panic!("items: {t}"));
    let item = items
        .iter()
        .find(|i| i["doc"]["body"].as_str().is_some_and(|b| b.starts_with(COS_START_TEXT)))
        .unwrap_or_else(|| panic!("the start message: {t}"));
    assert_eq!(item["doc"]["author"]["login"], json!("julie"), "posted as the caller: {item}");
}

/// The seat-only route: only Kessel's own pinned chat. Every other caller
/// gets `kessel_session_only`, and nothing is made.
#[test]
fn the_chief_of_staff_route_refuses_everyone_but_kessels_pinned_chat() {
    let _e = env(true, &["codex"]);
    let (kessel, _, _) = make_kessel();
    let other = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path) VALUES ('w-other', 'other', '/tmp/kessel-other')", []).expect("other");
        "w-other".to_string()
    };
    let refused = [
        ("another agent", Caller::Agent(AgentCaller { project_id: other.clone(), map_key: Some(other.clone()), args: vec![], sandboxed: false })),
        ("Kessel's own sidecar", Caller::Agent(AgentCaller { project_id: kessel.clone(), map_key: Some("tab-pg1".into()), args: vec![], sandboxed: false })),
        ("a gone session", Caller::Agent(AgentCaller { project_id: kessel.clone(), map_key: None, args: vec![], sandboxed: false })),
        ("a sandboxed Kessel", Caller::Agent(AgentCaller { project_id: kessel.clone(), map_key: Some(kessel.clone()), args: vec![], sandboxed: true })),
        ("a shell tab", Caller::Shell { project_id: kessel.clone() }),
        ("a login", Caller::Login { username: "julie".into() }),
        ("the owner token", Caller::Owner),
        ("an API key", Caller::WrongCredential),
        ("no credential", Caller::Invalid),
    ];
    for (who, caller) in refused {
        let r = handle_chief_of_staff(&cos_body(), &caller);
        assert_eq!((r.status, code(&r).as_str()), ("403 Forbidden", "kessel_session_only"), "{who}");
    }
    // A person through the person arm, too.
    let r = post(COS_PATH, json!({ "name": "Max", "about": "x", "tool": "codex" }), &Actor::owner_token());
    assert_eq!((r.status, code(&r).as_str()), ("403 Forbidden", "kessel_session_only"), "the owner token's arm");
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert_eq!(k2_core::orchestrator::live_chief_of_staff(&conn).expect("seat"), None, "nothing was made");
}

/// The classifier the dispatcher uses: an API key and an app pass are the
/// wrong credential, the owner token is the owner, a login is a login.
#[test]
fn the_dispatcher_classifies_keys_logins_and_the_owner_token() {
    let _e = env(true, &[]);
    let classify = |q: &str| crate::sidecar_routes::caller_from_tcp(COS_PATH, q, None, "owner-tok");
    assert_eq!(classify("token=owner-tok"), Caller::Owner);
    assert_eq!(classify("token=k2sk_abc123"), Caller::WrongCredential, "an API key");
    assert_eq!(classify("token=k2skn_abc123"), Caller::WrongCredential, "an app pass");
    k2_core::connect_users::add_user("julie", "password123").expect("add");
    let s = k2_core::connect_users::create_session("julie");
    assert_eq!(classify(&format!("token={s}")), Caller::Login { username: "julie".into() });
    assert!(crate::session_token::is_agent_verb(COS_PATH), "a passport reaches the handler");
}

#[test]
fn kessels_pinned_chat_hires_the_chief_of_staff_with_only_what_kessel_holds() {
    let _e = env(true, &["codex"]);
    let (kessel, kessel_path, kessel_handle) = make_kessel();
    // Kessel lost one of its switches since: it can't pass that one on.
    k2_core::workspace::settings::set_agents_can_refresh_chat(&kessel, false).expect("off");
    TEST_LAUNCHES.lock().clear();
    let mut rx = crate::session_events::sender().subscribe();

    let r = handle_chief_of_staff(br#"{"name": "Max", "about": "Runs a small bakery", "tool": "grok"}"#, &kessel_session(&kessel));
    assert_eq!((r.status, code(&r).as_str()), ("400 Bad Request", "tool_not_signed_in"));
    let r = handle_chief_of_staff(br#"{"name": "Max", "tool": "codex"}"#, &kessel_session(&kessel));
    assert_eq!((r.status, code(&r).as_str()), ("400 Bad Request", "usage"), "all three asked first");

    let r = handle_chief_of_staff(&cos_body(), &kessel_session(&kessel));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let out = json_of(&r);
    let cos = out["workspaceId"].as_str().expect("id").to_string();
    let cos_path = out["path"].as_str().expect("path").to_string();
    assert_eq!(out["name"], json!("Max"));
    assert!(cos_path.ends_with("k2-agents/max"), "{cos_path}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(k2_core::orchestrator::live_chief_of_staff(&conn).expect("seat").expect("live").workspace_id, cos, "recorded");
        // Connected to Kessel.
        let rel = k2_core::db::schema::WorkspaceRelation::list_for_source(&conn, &kessel).expect("relations");
        assert!(rel.iter().any(|r| r.target_project_id == cos), "Kessel → Max: {rel:?}");
    }
    assert_eq!(default_agent(&cos).as_deref(), Some("codex"));
    assert_eq!(agent_mode(&cos), ("custom".to_string(), 1), "pinned chat on");
    assert!(k2_core::workspace::settings::agents_can_request_permissions(&cos), "its cards switch");
    assert!(k2_core::workspace::settings::get_agents_can_create_connections(&cos_path), "per-workspace connections on");
    // Only what Kessel holds: chat.refresh stays a card ask.
    assert!(!on(&cos, "chat.refresh"));
    assert_eq!(out["permissions"]["left"], json!([{ "action": "chat.refresh", "reason": "hirer_lacks" }]));
    for a in CHIEF_OF_STAFF_STARTER_PERMISSIONS {
        assert!(on(&cos, a), "{a} passed on");
    }
    assert!(!on(&cos, "organization.manage") && !on(&cos, "gardens.manage"), "nothing beyond its set");
    // The charter: what they do and Kessel's handle, filled in.
    let role = std::fs::read_to_string(k2_core::workspace::agent_identity::persona_md_in(
        k2_core::workspace::agent_identity::workspace_agent_path(&cos_path),
    ))
    .expect("ROLE.md");
    assert!(role.contains("Runs a small bakery") && role.contains(&format!("k2 msg {kessel_handle}")), "{role}");
    // Launched with its hello; events sent.
    let launches = TEST_LAUNCHES.lock().clone();
    assert_eq!(launches.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>(), vec!["max"], "{launches:?}");
    assert!(launches[0].1.is_some());
    let mut events = (false, false);
    while let Ok(ev) = rx.try_recv() {
        match ev {
            crate::session_events::SessionEvent::FirstRunChanged { person: None } => events.0 = true,
            crate::session_events::SessionEvent::ProjectsChanged {} => events.1 = true,
            _ => {}
        }
    }
    assert!(events.0 && events.1, "{events:?}");
    // The row is done; a second hire is 409; Kessel's own folder is never reused.
    let v = status_for(&Actor::owner_token());
    let row = v["rows"].as_array().expect("rows").iter().find(|r| r["id"] == json!("chief_of_staff")).cloned().expect("row");
    assert_eq!(row, json!({ "id": "chief_of_staff", "state": "done", "opens": "kessel_thread_cos" }));
    let r = handle_chief_of_staff(&cos_body(), &kessel_session(&kessel));
    assert_eq!((r.status, code(&r).as_str()), ("409 Conflict", "chief_of_staff_exists"));
    assert!(std::path::Path::new(&kessel_path).is_dir());
}

#[test]
fn a_kessel_folder_already_there_is_409_folder_exists_not_a_crash() {
    let _e = env(true, &["codex"]);
    let root = k2_core::hire::root::hire_root().expect("root");
    // Someone's folder (an empty one that isn't a workspace is adopted).
    std::fs::create_dir_all(root.join("kessel")).expect("someone's folder");
    std::fs::write(root.join("kessel").join("notes.txt"), "mine").expect("someone's file");
    let r = post(KESSEL_PATH, json!({ "tool": "codex" }), &Actor::owner_token());
    assert_eq!((r.status, code(&r).as_str()), ("409 Conflict", "folder_exists"), "{}", r.body);
    assert!(json_of(&r)["error"]["hint"].as_str().expect("hint").contains("another handle"));
    let r = post(KESSEL_PATH, json!({ "tool": "codex", "handle": "kesselb" }), &Actor::owner_token());
    assert_eq!(r.status, "200 OK", "another handle works: {}", r.body);
}

/// Another agent (not Kessel) registered with `handle`, in a folder of its own.
fn other_agent_holding(id: &str, handle: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?3)",
        rusqlite::params![id, format!("/tmp/{id}-elsewhere"), handle],
    )
    .expect("other agent");
}

fn workspaces() -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0)).expect("count")
}

#[test]
fn kessel_taken_by_another_agent_makes_kessel_as_kessel5() {
    let _e = env(true, &["codex"]);
    other_agent_holding("w-fan", "kessel");
    let v = status_for(&Actor::owner_token());
    assert_eq!(v["kesselDefaults"]["handle"], json!("kessel5"), "{v}");
    assert!(v["kesselDefaults"]["path"].as_str().expect("path").ends_with("k2-agents/kessel5"), "{v}");
    assert_eq!(v["kesselDefaults"]["name"], json!("Kessel"));
    // The sheet sends its prefilled handle back: still the default.
    let r = post(KESSEL_PATH, json!({ "tool": "codex", "handle": "kessel5" }), &Actor::owner_token());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let out = json_of(&r);
    assert_eq!(out["name"], json!("Kessel"));
    assert_eq!(out["handle"], json!("kessel5"), "{out}");
    assert!(out["path"].as_str().expect("path").ends_with("k2-agents/kessel5"), "{out}");
}

#[test]
fn kessel_left_out_with_kessel_taken_is_kessel5_too() {
    let _e = env(true, &["codex"]);
    other_agent_holding("w-fan", "kessel");
    let (_, path, handle) = make_kessel();
    assert_eq!(handle, "kessel5");
    assert!(path.ends_with("k2-agents/kessel5"), "{path}");
}

#[test]
fn handle_kessel_with_kessel_taken_is_the_default_so_kessel5() {
    let _e = env(true, &["codex"]);
    other_agent_holding("w-fan", "kessel");
    let r = post(KESSEL_PATH, json!({ "tool": "codex", "handle": "kessel" }), &Actor::owner_token());
    assert_eq!((r.status, json_of(&r)["handle"].clone()), ("200 OK", json!("kessel5")), "{}", r.body);
}

#[test]
fn kessel_and_kessel5_both_taken_is_409_kessel_handle_taken_and_nothing_is_made() {
    let _e = env(true, &["codex"]);
    other_agent_holding("w-fan", "kessel");
    other_agent_holding("w-fan5", "kessel5");
    let before = workspaces();
    for body in [json!({ "tool": "codex" }), json!({ "tool": "codex", "handle": "kessel" })] {
        let r = post(KESSEL_PATH, body, &Actor::owner_token());
        assert_eq!((r.status, code(&r).as_str()), ("409 Conflict", "kessel_handle_taken"), "{}", r.body);
        let hint = json_of(&r)["error"]["hint"].as_str().expect("hint").to_string();
        assert!(hint.contains("kessel and kessel5") && hint.contains("--handle"), "{hint}");
    }
    assert_eq!(workspaces(), before, "nothing made");
    assert!(matches!(seat_kessel(), Ok(None)), "no Kessel seated");
    // A handle the person chose still works.
    let r = post(KESSEL_PATH, json!({ "tool": "codex", "handle": "kay" }), &Actor::owner_token());
    assert_eq!((r.status, json_of(&r)["handle"].clone()), ("200 OK", json!("kay")), "{}", r.body);
}

#[test]
fn an_explicit_taken_handle_is_refused_as_before_with_no_fallback() {
    let _e = env(true, &["codex"]);
    let root = k2_core::hire::root::hire_root().expect("root");
    let ops = root.join("ops").to_string_lossy().into_owned();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES ('w-ops', 'Ops', ?1, 'ops')", [&ops]).expect("ops");
    }
    let before = workspaces();
    let r = post(KESSEL_PATH, json!({ "tool": "codex", "handle": "ops" }), &Actor::owner_token());
    assert_eq!((r.status, code(&r).as_str()), ("409 Conflict", "folder_in_use"), "{}", r.body);
    assert_eq!(workspaces(), before, "no fallback was made");
}

#[test]
fn a_chief_of_staff_named_like_kessels_folder_is_refused() {
    let _e = env(true, &["codex"]);
    let (kessel, _, _) = make_kessel();
    let r = handle_chief_of_staff(br#"{"name": "Kessel", "about": "x", "tool": "codex"}"#, &kessel_session(&kessel));
    assert_eq!((r.status, code(&r).as_str()), ("409 Conflict", "folder_in_use"));
}

#[test]
fn deleting_kessels_workspace_clears_the_seat_and_the_offer_comes_back_and_kessel_is_made_again_in_place() {
    let _e = env(true, &["codex"]);
    let (kessel, kessel_path, _) = make_kessel();
    let v = status_for(&Actor::owner_token());
    let row = v["rows"].as_array().expect("rows").iter().find(|r| r["id"] == json!("chief_of_staff")).cloned().expect("row");
    assert_eq!(row["opens"], json!("kessel_thread_cos"));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM projects WHERE id = ?1", [&kessel]).expect("delete");
    }
    let v = status_for(&Actor::owner_token());
    assert_eq!(v["kessel"], Value::Null, "{v}");
    assert_eq!(v["offer"], json!("meet_kessel"));
    let row = v["rows"].as_array().expect("rows").iter().find(|r| r["id"] == json!("chief_of_staff")).cloned().expect("row");
    assert_eq!(row, json!({ "id": "chief_of_staff", "state": "todo", "opens": "kessel_sheet", "note": "needs_kessel" }));
    // Kessel's old session can't hire any more.
    let r = handle_chief_of_staff(&cos_body(), &kessel_session(&kessel));
    assert_eq!((r.status, code(&r).as_str()), ("403 Forbidden", "kessel_session_only"));
    // Meet Kessel again: its old folder is the `kessel` template's (its lock says
    // so), so Kessel is made again in place, with its hello.
    TEST_LAUNCHES.lock().clear();
    let r = post(KESSEL_PATH, json!({ "tool": "codex" }), &Actor::owner_token());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let again = json_of(&r);
    assert_eq!(again["path"], json!(kessel_path), "the same folder");
    let id = again["workspaceId"].as_str().expect("id").to_string();
    assert_ne!(id, kessel, "a new workspace");
    assert_eq!(status_for(&Actor::owner_token())["kessel"]["workspaceId"], json!(id), "the seat names it");
    let launches = TEST_LAUNCHES.lock().clone();
    assert!(launches.len() == 1 && launches[0].1.is_some(), "{launches:?}");
}

// ── The first chat (0.46.11's picker) ────────────────────────────────

/// An `exec cat` shim for `bin` only: every other agent CLI is missing.
fn installed(home: &crate::test_support::TempHome, bin: &str) -> k2_core::test_env::EnvVar {
    let dir = home.path().join("agent-shims");
    std::fs::create_dir_all(&dir).expect("shim dir");
    k2_core::test_env::write_executable(&dir.join(bin), "#!/bin/sh\nexec cat\n");
    k2_core::test_env::EnvVar::set(k2_core::terminal::agent_spawn_guard::SHIM_DIR_ENV, &dir)
}

/// What the Chat tab sends when it opens an agent's chat: no `harness`.
fn chat_tab_ensure(path: &str) -> (String, Value) {
    let r = crate::pinned_chat::handle_ensure_pinned_chat(json!({ "project": path }).to_string().as_bytes());
    (r.status.to_string(), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn chat_is_live(id: &str) -> bool {
    crate::v2_session_map::lookup_by_agent_name(&crate::canonical_session::canonical_key_for(id)).is_some()
}

fn stop_chat(id: &str) {
    crate::v2_session_map::unregister(&crate::canonical_session::canonical_key_for(id));
}

/// A server that has never had a pinned chat asks before a never-chatted
/// agent's first chat starts on Claude (`409 harness_choice_needed`). Kessel
/// and the Chief of Staff come with a tool already picked, so their routes
/// start the chat on it the way the picker does, and the Chat tab finds it
/// running: nobody is asked again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kessel_made_with_claude_on_a_fresh_server_starts_its_chat_with_no_409_and_so_does_the_chief_of_staff() {
    let e = env(true, &["claude"]);
    let _claude = installed(&e._home, "claude");
    let ever: bool = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT EXISTS(SELECT 1 FROM workspace_sessions)", [], |r| r.get(0)).expect("sessions")
    };
    assert!(!ever, "a fresh server: no pinned chat ever");

    let r = post(KESSEL_PATH, json!({ "tool": "claude" }), &Actor::owner_token());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let out = json_of(&r);
    let (kessel, kessel_path) = (out["workspaceId"].as_str().expect("id").to_string(), out["path"].as_str().expect("path").to_string());
    assert!(chat_is_live(&kessel), "Kessel's chat started on the tool picked in the sheet");
    assert_eq!(crate::pinned_chat_choice::needs_choice(&kessel_path), None, "nothing left to ask");
    let (status, v) = chat_tab_ensure(&kessel_path);
    assert_eq!(status, "200 OK", "the Chat tab finds Kessel's chat, no 409: {v}");
    assert_eq!((v["provider"].as_str(), v["reused"].as_bool()), (Some("claude"), Some(true)), "{v}");

    let r = handle_chief_of_staff(
        &json!({ "name": "Max", "about": "Runs a small bakery", "tool": "claude" }).to_string().into_bytes(),
        &kessel_session(&kessel),
    );
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let out = json_of(&r);
    let (cos, cos_path) = (out["workspaceId"].as_str().expect("id").to_string(), out["path"].as_str().expect("path").to_string());
    assert!(chat_is_live(&cos), "the Chief of Staff's chat started on the tool Kessel asked for");
    let (status, v) = chat_tab_ensure(&cos_path);
    assert_eq!(status, "200 OK", "no 409 for the Chief of Staff either: {v}");
    assert_eq!((v["provider"].as_str(), v["reused"].as_bool()), (Some("claude"), Some(true)), "{v}");

    stop_chat(&cos);
    stop_chat(&kessel);
}

/// A focus group named `name`: its id.
fn focus_group(name: &str) -> String {
    k2_core::db_ops::focus_groups_create(name, None).expect("group").id
}

fn focus_group_id(id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT focus_group_id FROM projects WHERE id = ?1", [id], |r| r.get(0)).expect("project")
}

fn workspaces() -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0)).expect("count")
}

/// The sheet's focus-group picker: Kessel lands in the group it names, and a
/// group that isn't there is refused before anything is made.
#[test]
fn kessel_lands_in_the_chosen_focus_group_and_an_unknown_group_makes_nothing() {
    let _e = env(true, &["codex"]);
    let ops = focus_group("Ops");
    let _home = focus_group("Home");
    let before = workspaces();

    let r = post(KESSEL_PATH, json!({ "tool": "codex", "focusGroup": "no-such-group" }), &Actor::owner_token());
    assert_eq!((r.status, code(&r).as_str()), ("400 Bad Request", "unknown_focus_group"), "{}", r.body);
    assert!(matches!(seat_kessel(), Ok(None)), "no Kessel seated");
    assert_eq!(workspaces(), before, "no workspace made");
    assert!(TEST_LAUNCHES.lock().is_empty(), "nothing launched");
    let r = post(KESSEL_PATH, json!({ "tool": "codex", "focusGroup": 7 }), &Actor::owner_token());
    assert_eq!((r.status, code(&r).as_str()), ("400 Bad Request", "usage"));

    let r = post(KESSEL_PATH, json!({ "tool": "codex", "focusGroup": &ops }), &Actor::owner_token());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let out = json_of(&r);
    let kessel = out["workspaceId"].as_str().expect("id").to_string();
    assert_eq!(out["focusGroupId"], json!(&ops), "{out}");
    assert_eq!(focus_group_id(&kessel).as_deref(), Some(ops.as_str()), "Kessel is in Ops");
}

/// No `focusGroup`: Kessel joins no group (focus groups off, or none picked).
#[test]
fn kessel_without_a_focus_group_joins_none() {
    let _e = env(true, &["codex"]);
    let _ops = focus_group("Ops");
    let (kessel, _, _) = make_kessel();
    assert_eq!(focus_group_id(&kessel), None);
}

/// Kessel hires the Chief of Staff in chat, with no sheet: it joins Kessel's own
/// focus group unless Kessel names another.
#[test]
fn the_chief_of_staff_joins_kessels_focus_group_by_default() {
    let _e = env(true, &["codex"]);
    let ops = focus_group("Ops");
    let r = post(KESSEL_PATH, json!({ "tool": "codex", "focusGroup": &ops }), &Actor::owner_token());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let kessel = json_of(&r)["workspaceId"].as_str().expect("id").to_string();
    let before = workspaces();

    let mut body = json!({ "name": "Max", "about": "Runs a small bakery", "tool": "codex", "focusGroup": "nope" });
    let r = handle_chief_of_staff(body.to_string().as_bytes(), &kessel_session(&kessel));
    assert_eq!((r.status, code(&r).as_str()), ("400 Bad Request", "unknown_focus_group"), "{}", r.body);
    assert_eq!(workspaces(), before, "no workspace made");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(k2_core::orchestrator::live_chief_of_staff(&conn).expect("seat"), None, "nothing seated");
    }

    body.as_object_mut().expect("object").remove("focusGroup");
    let r = handle_chief_of_staff(body.to_string().as_bytes(), &kessel_session(&kessel));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let out = json_of(&r);
    let cos = out["workspaceId"].as_str().expect("id").to_string();
    assert_eq!(out["focusGroupId"], json!(&ops));
    assert_eq!(focus_group_id(&cos).as_deref(), Some(ops.as_str()), "with Kessel, in Ops");
}

#[test]
fn the_chief_of_staff_honours_a_focus_group_kessel_names() {
    let _e = env(true, &["codex"]);
    let ops = focus_group("Ops");
    let home = focus_group("Home");
    let r = post(KESSEL_PATH, json!({ "tool": "codex", "focusGroup": &ops }), &Actor::owner_token());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let kessel = json_of(&r)["workspaceId"].as_str().expect("id").to_string();

    // By name, in any case (the CLI's --focus-group <id|name>).
    let body = json!({ "name": "Max", "about": "Runs a small bakery", "tool": "codex", "focusGroup": "home" });
    let r = handle_chief_of_staff(body.to_string().as_bytes(), &kessel_session(&kessel));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let cos = json_of(&r)["workspaceId"].as_str().expect("id").to_string();
    assert_eq!(focus_group_id(&cos).as_deref(), Some(home.as_str()), "in Home, as Kessel asked");
    assert_eq!(focus_group_id(&kessel).as_deref(), Some(ops.as_str()), "Kessel stays in Ops");
}
