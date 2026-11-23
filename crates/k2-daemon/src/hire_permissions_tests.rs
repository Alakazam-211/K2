//! The permissions step of a template hire (prd-agent-templates-v1 §9, T3):
//! reach per hirer through the real hire doors, declines and cards held by
//! the daemon (0330, never the agent's own lock), cards filed at the first
//! start, new folders only for agents and keys, compute never passed on by
//! an agent, and the audit. Lib tests never flip the process cards switch
//! (lane B's rule): a test sets it for its own thread
//! ([`TEST_SERVER_CARDS`]). Every count is scoped to this test's own ids.

use std::collections::HashMap;
use std::path::Path;

use k2_core::connect_users::Role;
use k2_core::db::schema::WorkspaceSession;
use k2_core::hire::permission_records as records;
use k2_core::hire::template::permissions::{StepIn, CARDS_ROW};
use k2_core::hire::HireCaller;
use serde_json::{json, Value};

use super::*;
use crate::compute_routes::Who;
use crate::hire_routes::test_support::ensure_test_root;
use crate::hire_routes::{handle_cli, CLI_HIRE};
use crate::routes::http::V1Principal;

fn uniq(prefix: &str) -> String {
    format!("{prefix}-{}", &uuid::Uuid::new_v4().simple().to_string()[..10])
}

fn code_of(r: &crate::cli_response::CliResponse) -> u16 {
    r.status.split_whitespace().next().and_then(|n| n.parse().ok()).expect("status")
}

fn cli(who: Who, body: Value) -> (u16, Value) {
    let r = handle_cli(CLI_HIRE, who, true, body.to_string().as_bytes(), &HashMap::new(), None);
    (code_of(&r), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json {e}: {}", r.body)))
}

fn v1w(p: &V1Principal, body: Value) -> (u16, Value) {
    let r = crate::v1_hire::handle_v1_hire(p, body.to_string().as_bytes());
    (code_of(&r), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json {e}: {}", r.body)))
}

fn row<'a>(v: &'a Value, target: &str) -> &'a Value {
    v["rows"].as_array().unwrap().iter().find(|r| r["step"] == "permission" && r["target"] == target).unwrap_or_else(|| panic!("no {target}: {v}"))
}

fn status(v: &Value, target: &str) -> String {
    row(v, target)["status"].as_str().unwrap().to_string()
}

/// Audit lines of this event for one workspace handle (the outcome ends
/// with it, or names it before ` card=`).
fn audit_for(handle: &str) -> Vec<(String, String)> {
    crate::hire_routes::AUDIT_SINK
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter(|(e, _, o)| e == AUDIT_EVENT && (o.ends_with(&format!(" {handle}")) || o.contains(&format!(" {handle} card="))))
        .map(|(_, a, o)| (a.clone(), o.clone()))
        .collect()
}

/// `hire_events` rows for one workspace, permission and result.
fn events(ws: &str, action: &str, result: &str) -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COUNT(*) FROM hire_events WHERE action = ?1 AND reason = ?2 AND result = ?3 AND detail LIKE ?4",
        rusqlite::params![AUDIT_EVENT, action, result, format!("%\"workspaceId\":\"{ws}\"%")],
        |r| r.get::<_, i64>(0),
    )
    .expect("count")
}

fn ws_path(id: &str) -> String {
    crate::sidecar_routes::workspace_by_id(id).expect("workspace").path
}

/// An agent's own workspace with "Let this agent hire agents" on, its
/// passport counted as its main session.
fn hiring_agent(prefix: &str) -> (String, Who) {
    let path = ensure_test_root().join(uniq(prefix)).to_string_lossy().into_owned();
    let (st, v) = cli(Who::Owner, json!({ "path": path, "name": "Hirer" }));
    assert_eq!(st, 200, "{v}");
    let id = v["id"].as_str().expect("id").to_string();
    k2_core::workspace::settings::set_agents_can_hire_agents(&id, true).expect("hire on");
    let sid = format!("main-{id}");
    crate::hire_routes::test_main_session(&sid);
    (id.clone(), Who::Agent { project_id: id, session_id: sid })
}

fn pin_thread(id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), id, None, Some(&conv), "claude", "system", "running").expect("pin");
}

fn login(role: Role) -> Who {
    let user = format!("u{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    k2_core::connect_users::add_user(&user, "password123").expect("add_user");
    if role != Role::Member {
        k2_core::connect_users::set_role(&user, role).expect("set_role");
    }
    Who::Login { username: user, role }
}

fn hire_key(grant: &str) -> V1Principal {
    ensure_test_root();
    let caps = k2_core::api_keys::ApiCapabilities { host_sessions: true, hire: true, ..k2_core::api_keys::ApiCapabilities::none() };
    let l = k2_core::hire::limits::HireLimits::from_request(&json!({}), caps.db).expect("limits");
    let (_id, raw) = k2_core::api_keys::create_api_key_ex(&uniq("hk"), None, Some(grant), None, None, Some(caps), Some(&l.to_json())).expect("mint");
    V1Principal::Api(k2_core::api_keys::resolve_api_key(&raw).expect("resolve"))
}

fn set_cards(on: Option<bool>) {
    TEST_SERVER_CARDS.with(|c| c.set(on));
}

/// The owner gives every suggestion; the dry run says `grant` and audits
/// nothing; the hire says `done`, the switches are on, and each grant is one
/// audit line and one `hire_events` row; a re-run changes nothing. A switch
/// on by default ("Manage K2 projects") is `skip`; with cards off on the
/// server, "Can ask for permission cards" is `skip`.
#[test]
fn the_owner_grants_every_suggestion_once_and_it_is_audited() {
    let _home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    install();
    let name = uniq("Lead");
    let (st, plan) = cli(Who::Owner, json!({ "template": "lead", "name": name, "dryRun": true }));
    assert_eq!(st, 200, "{plan}");
    let handle = plan["handle"].as_str().expect("handle").to_string();
    for a in ["agents.hire", "agents.manage", "connections.create", "projects.manage"] {
        assert_eq!(status(&plan, a), "grant", "a new folder: nothing is on yet: {plan}");
    }
    assert_eq!(status(&plan, CARDS_ROW), "skip", "{plan}");
    assert!(row(&plan, CARDS_ROW)["reason"].as_str().unwrap().contains("off on this server"), "{plan}");
    assert_eq!(audit_for(&handle), vec![], "a dry run audits nothing");

    let (st, out) = cli(Who::Owner, json!({ "template": "lead", "name": name, "launch": false }));
    assert_eq!(st, 200, "{out}");
    assert_eq!(out["handle"].as_str(), Some(handle.as_str()), "the plan named it");
    let id = out["id"].as_str().unwrap().to_string();
    let want = ["agents.hire", "agents.manage", "connections.create"];
    for a in want {
        assert_eq!(status(&out, a), "done", "{out}");
        assert!(crate::first_run_grants::switch_on(&id, k2_core::hire::starter_permissions::standing_switch(a).unwrap()), "{a} on");
        assert_eq!(events(&id, a, "granted"), 1, "{a}: one hire_events row");
    }
    assert_eq!((status(&out, "projects.manage").as_str(), row(&out, "projects.manage")["reason"].as_str()), ("skip", Some("already on")), "{out}");
    assert_eq!(out["permissions"]["granted"], json!(want), "{out}");
    let lines = audit_for(&handle);
    assert_eq!(lines.len(), 3, "one line per grant: {lines:?}");
    assert!(lines.iter().all(|(a, o)| a == "owner-token" && o.starts_with("granted ")), "{lines:?}");

    let (_, again) = cli(Who::Owner, json!({ "template": "lead", "name": name }));
    assert_eq!(again["changed"], false, "{again}");
    for a in want {
        assert_eq!((status(&again, a).as_str(), row(&again, a)["reason"].as_str()), ("skip", Some("already on")), "{again}");
    }
    assert_eq!(audit_for(&handle).len(), 3, "nothing more audited");
}

/// A decline is the daemon's: the switch stays off, a re-run (and an edit
/// of the agent's own lock) leaves it off, and only a person naming it
/// with --allow decides it again. A decline the template doesn't suggest,
/// or one also allowed, is a usage error.
#[test]
fn a_decline_is_held_by_the_daemon_and_undone_only_by_name() {
    let _home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    install();
    let name = uniq("Dev");
    let (st, e) = cli(Who::Owner, json!({ "template": "developer", "name": name, "declinePermissions": ["compute.usee"], "dryRun": true }));
    assert_eq!((st, e["error"]["code"].as_str()), (400, Some("usage")), "{e}");
    assert!(e["error"]["hint"].as_str().unwrap().contains("it suggests: permission-cards, compute.use"), "{e}");
    let (st, e) = cli(
        Who::Owner,
        json!({ "template": "developer", "name": name, "declinePermissions": ["compute.use"], "allowPermissions": ["compute.use"], "dryRun": true }),
    );
    assert_eq!((st, e["error"]["code"].as_str()), (400, Some("usage")), "both: {e}");
    let (st, out) = cli(Who::Owner, json!({ "template": "developer", "name": name, "declinePermissions": ["compute.use"], "launch": false }));
    assert_eq!(st, 200, "{out}");
    assert_eq!(status(&out, "compute.use"), "declined", "{out}");
    let id = out["id"].as_str().unwrap().to_string();
    let path = out["path"].as_str().unwrap().to_string();
    assert!(!crate::first_run_grants::switch_on(&id, "compute"), "declined stays off");
    assert_eq!(events(&id, "compute.use", "declined"), 1);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(records::declined(&conn, &id).unwrap(), vec!["compute.use"], "the daemon holds it");
        assert_eq!(records::decline_rank(&conn, &id, "compute.use").unwrap(), Some(records::RANK_OWNER), "with who declined it");
    }
    // The agent rewrites its own lock: no effect.
    let lock_path = Path::new(&path).join(".k2/agent/template.lock");
    let lock = std::fs::read_to_string(&lock_path).unwrap();
    // Top-level keys (an agent would write them there), ahead of the tables.
    std::fs::write(&lock_path, format!("declined = []\ncards_pending = [\"organization.manage\"]\n{lock}")).unwrap();
    let (_, again) = cli(Who::Owner, json!({ "template": "developer", "name": name }));
    assert_eq!(status(&again, "compute.use"), "declined", "a re-run keeps the decline: {again}");
    assert!(row(&again, "compute.use")["reason"].as_str().unwrap().contains("--allow"), "{again}");
    assert!(!crate::first_run_grants::switch_on(&id, "compute"));
    assert_eq!(events(&id, "compute.use", "declined"), 1, "not declined again");
    // A person names it again: granted.
    let (_, allowed) = cli(Who::Owner, json!({ "template": "developer", "name": name, "allowPermissions": ["compute.use"] }));
    assert_eq!(status(&allowed, "compute.use"), "done", "{allowed}");
    assert!(crate::first_run_grants::switch_on(&id, "compute"));
    assert_eq!(events(&id, "compute.use", "allowed-again"), 1);
    // Without a template the fields are refused.
    let plain = ensure_test_root().join(uniq("plain")).to_string_lossy().into_owned();
    let (st, e) = cli(Who::Owner, json!({ "path": plain, "declinePermissions": ["compute.use"] }));
    assert_eq!((st, e["error"]["code"].as_str()), (400, Some("usage")), "{e}");
    let (st, e) = cli(Who::Owner, json!({ "path": plain, "allowPermissions": ["compute.use"] }));
    assert_eq!((st, e["error"]["code"].as_str()), (400, Some("usage")), "{e}");
}

/// A record write that fails mid-loop names what this hire already turned
/// on or carded (those rows ran through their own routes and were
/// audited); with nothing done yet it says so.
#[test]
fn a_record_failure_names_what_was_already_granted() {
    let none = records_error_text("dev", "disk I/O error", &[], &[]);
    assert!(none.contains("nothing was granted"), "{none}");
    let some = records_error_text("dev", "disk I/O error", &["compute.use".into()], &["vault.manage".into()]);
    assert!(some.contains("turned on compute.use") && some.contains("asked on a card for vault.manage"), "{some}");
    assert!(!some.contains("nothing was granted"), "{some}");
    assert!(some.contains("disk I/O error") && some.contains(" dev "), "{some}");
}

/// `--allow` lifts a held decline only for a caller who could grant the
/// switch itself, with a role at least the decliner's, and reach is
/// checked before the record is touched: a Member's `--allow` and an
/// Admin's on an Owner's decline are refused and the decline stays (a later
/// re-run still leaves it off); the Owner lifts it. An Admin lifts a
/// Member's decline.
#[test]
fn allow_lifts_a_decline_only_for_a_role_at_least_the_decliners() {
    let _home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    install();
    set_cards(Some(false));
    let rank = |id: &str| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        records::decline_rank(&conn, id, "compute.use").unwrap()
    };
    let name = uniq("Dev");
    let (st, out) = cli(Who::Owner, json!({ "template": "developer", "name": name, "declinePermissions": ["compute.use"], "launch": false }));
    assert_eq!(st, 200, "{out}");
    let id = out["id"].as_str().unwrap().to_string();
    assert_eq!(rank(&id), Some(records::RANK_OWNER));

    // A Member can't grant it: refused, the decline stays, nothing on.
    let member = login(Role::Member);
    let (st, v) = cli(member, json!({ "template": "developer", "name": name, "allowPermissions": ["compute.use"] }));
    assert_eq!(st, 200, "the hire itself goes ahead: {v}");
    assert_eq!(status(&v, "compute.use"), "refused", "{v}");
    assert!(row(&v, "compute.use")["hint"].as_str().unwrap().contains("--allow compute.use"), "{v}");
    assert_eq!(rank(&id), Some(records::RANK_OWNER), "the record is unchanged");
    assert!(!crate::first_run_grants::switch_on(&id, "compute"));
    assert_eq!(events(&id, "compute.use", "refused:allow_role"), 1, "{v}");

    // An Admin could grant it, but an Owner declined it: refused.
    let admin = login(Role::Admin);
    let (st, v) = cli(admin.clone(), json!({ "template": "developer", "name": name, "allowPermissions": ["compute.use"] }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(status(&v, "compute.use"), "refused", "{v}");
    assert!(row(&v, "compute.use")["reason"].as_str().unwrap().contains("an owner declined it"), "{v}");
    assert_eq!(rank(&id), Some(records::RANK_OWNER));
    assert!(!crate::first_run_grants::switch_on(&id, "compute"));
    assert_eq!(events(&id, "compute.use", "refused:allow_decliner_outranks"), 1);

    // A later plain re-run by the Owner still leaves it off.
    let (_, v) = cli(Who::Owner, json!({ "template": "developer", "name": name }));
    assert_eq!(status(&v, "compute.use"), "declined", "{v}");
    assert!(!crate::first_run_grants::switch_on(&id, "compute"));

    // The Owner names it: lifted and granted.
    let (_, v) = cli(Who::Owner, json!({ "template": "developer", "name": name, "allowPermissions": ["compute.use"] }));
    assert_eq!(status(&v, "compute.use"), "done", "{v}");
    assert_eq!(rank(&id), None, "lifted");
    assert!(crate::first_run_grants::switch_on(&id, "compute"));

    // A Member's decline: an Admin lifts it.
    let name2 = uniq("Dev");
    let member = login(Role::Member);
    let (st, out) = cli(member, json!({ "template": "developer", "name": name2, "declinePermissions": ["compute.use"], "launch": false }));
    assert_eq!(st, 200, "{out}");
    let id2 = out["id"].as_str().unwrap().to_string();
    assert_eq!(rank(&id2), Some(records::RANK_MEMBER));
    let (_, v) = cli(admin, json!({ "template": "developer", "name": name2, "allowPermissions": ["compute.use"] }));
    assert_eq!(status(&v, "compute.use"), "done", "{v}");
    assert_eq!(rank(&id2), None);
    set_cards(None);
}

/// An agent passes on only what it holds, never an Owner-floor switch it
/// holds (organization.manage), never compute (its machines and limits are
/// its own); the rest is refused with who can turn it on (cards off). It
/// hires only inside the hire root.
#[test]
fn an_agent_passes_on_only_what_it_holds_never_owner_switches_or_compute() {
    let _home = k2_core::test_env::TempHome::new();
    let root = ensure_test_root();
    install();
    let (ws, agent) = hiring_agent("pass");
    k2_core::workspace::settings::set_agents_can_manage_agents(&ws, false).unwrap();
    k2_core::workspace::settings::update_project_setting(&ws_path(&ws), "agents_can_create_connections", "1").unwrap();
    let (st, out) = cli(agent.clone(), json!({ "template": "lead", "name": uniq("Team"), "launch": false }));
    assert_eq!(st, 200, "{out}");
    let new_id = out["id"].as_str().unwrap().to_string();
    let handle = out["handle"].as_str().unwrap().to_string();
    assert_eq!(status(&out, "agents.hire"), "done", "it holds agents.hire: {out}");
    assert_eq!(status(&out, "connections.create"), "done", "{out}");
    assert_eq!(status(&out, "projects.manage"), "skip", "on by default: {out}");
    assert_eq!(status(&out, CARDS_ROW), "skip", "cards off on this server: {out}");
    let refused = row(&out, "agents.manage");
    assert_eq!(refused["status"], "refused", "{out}");
    let hint = refused["hint"].as_str().unwrap();
    assert!(hint.contains(&format!("k2 agent access set {handle} agents_manage on")), "{hint}");
    assert!(hint.starts_with("An owner or admin can turn on"), "{hint}");
    assert!(refused["reason"].as_str().unwrap().contains("permission cards are off"), "{out}");
    assert_eq!(out["permissions"]["refused"], json!([{ "action": "agents.manage", "code": "hirer_lacks", "hint": hint }]), "{out}");
    assert!(!crate::first_run_grants::switch_on(&new_id, "agents_manage"));
    assert!(crate::first_run_grants::switch_on(&new_id, "agents_hire"));
    let lines = audit_for(&handle);
    assert_eq!(
        lines.iter().map(|(_, o)| o.split(' ').take(2).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>(),
        vec!["granted agents.hire", "refused:hirer_lacks agents.manage", "granted connections.create"],
        "{lines:?}"
    );
    let access: Vec<_> = crate::hire_routes::AUDIT_SINK
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter(|(e, _, o)| e == "agent-access-set" && o.starts_with(&format!("ok {handle} ")))
        .cloned()
        .collect();
    assert_eq!(access.len(), 2, "the agent's two grants carry their own lines: {access:?}");

    // Holding organization.manage (Owner floor) or compute doesn't let it pass them on.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::org_chart::store::set_agents_can_manage(&conn, &ws, true).expect("org chart on for the hirer");
    }
    let r = crate::sidecar_routes::access_entry(&Answerer::OwnerToken, &json!({ "toggle": "compute", "workspace": ws, "value": true }));
    assert!(r.status.starts_with('2'), "{}", r.body);
    assert!(crate::first_run_grants::switch_on(&ws, "org_chart_manage") && crate::first_run_grants::switch_on(&ws, "compute"));
    let caller = HireCaller::Agent { workspace_id: ws.clone() };
    let suggested = vec!["organization.manage".to_string(), "compute.use".to_string()];
    let path = ws_path(&new_id);
    let got = step_with(&StepIn { caller: &caller, path: &path, template_name: "Lead", cards: false, suggested: &suggested, declined: &[], allowed: &[], dry_run: false }, false).expect("step");
    let st: Vec<(&str, &str, &str)> = got.refused.iter().map(|r| (r.action.as_str(), r.code.as_str(), "")).collect();
    assert_eq!(st, vec![("organization.manage", "not_passable", ""), ("compute.use", "not_passable", "")], "{got:?}");
    assert!(!crate::first_run_grants::switch_on(&new_id, "org_chart_manage") && !crate::first_run_grants::switch_on(&new_id, "compute"));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(k2_core::compute::store::grants(&conn, None, Some(&new_id)).is_empty(), "no machine granted to the hire");
    }

    // Outside the hire root: refused before anything is written.
    // (Under the test home, as main's own test does: a temp dir behind a
    // symlink, such as macOS /var, is refused as a symlink first.)
    let outside = _home.path().join(uniq("outside")).to_string_lossy().into_owned();
    assert!(!Path::new(&outside).starts_with(&root));
    let (st, e) = cli(agent.clone(), json!({ "template": "writer", "path": outside, "dryRun": true }));
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("outside_hire_root")), "{e}");
    let (st, e) = cli(agent, json!({ "path": outside, "name": "Out" }));
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("outside_hire_root")), "{e}");
    assert!(!Path::new(&outside).exists());
}

/// An agent and an API key make a new workspace only: never its own, a
/// peer's, or a folder with something in it. An agent's hire is main's
/// create-only passport hire (`owner_only`, any folder that exists, an empty
/// one too); a key's template hire is `existing_workspace` (an empty,
/// unregistered folder is new). A key's plain converge of a workspace it was
/// granted stays as it was.
#[test]
fn agents_and_keys_hire_only_into_a_new_folder() {
    let _home = k2_core::test_env::TempHome::new();
    let root = ensure_test_root();
    install();
    let (ws, agent) = hiring_agent("own");
    let peer = root.join(uniq("peer")).to_string_lossy().into_owned();
    let (st, v) = cli(Who::Owner, json!({ "template": "general", "path": peer, "launch": false }));
    assert_eq!(st, 200, "{v}");
    let full = root.join(uniq("full"));
    std::fs::create_dir_all(full.join("src")).unwrap();
    let full = full.to_string_lossy().into_owned();
    for path in [ws_path(&ws), peer.clone(), full.clone()] {
        for dry in [true, false] {
            let (st, e) = cli(agent.clone(), json!({ "template": "writer", "path": path, "dryRun": dry, "launch": false }));
            assert_eq!((st, e["error"]["code"].as_str()), (403, Some("owner_only")), "{path} dry={dry}: {e}");
        }
        let (st, e) = cli(agent.clone(), json!({ "path": path, "name": "Again" }));
        assert_eq!((st, e["error"]["code"].as_str()), (403, Some("owner_only")), "{path} plain: {e}");
    }
    // Its own earlier hire is a person's to re-run.
    let name = uniq("Mine");
    let (st, v) = cli(agent.clone(), json!({ "template": "writer", "name": name, "launch": false }));
    assert_eq!(st, 200, "{v}");
    let (st, e) = cli(agent.clone(), json!({ "template": "writer", "name": name, "launch": false }));
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("owner_only")), "{e}");
    // An agent never adopts a folder that is already there, even an empty one.
    let empty = root.join(uniq("empty"));
    std::fs::create_dir_all(&empty).unwrap();
    let (st, e) = cli(agent, json!({ "template": "writer", "path": empty, "dryRun": true }));
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("owner_only")), "{e}");

    // An API key granted every workspace: a template hire on one is refused.
    let key = hire_key("*");
    let (st, e) = v1w(&key, json!({ "path": peer, "template": "developer" }));
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("existing_workspace")), "{e}");
    let (st, v) = v1w(&key, json!({ "path": peer }));
    assert_eq!(st, 200, "a plain converge of a granted workspace stays: {v}");
}

/// A shell tab in a cell hires as its agent, so it is held to the agent's
/// rule: only inside the hire root, only into a new folder, never its own
/// workspace or a peer's (`owner_only`, main's passport hire). An agent can
/// type into a shell tab it opened.
#[test]
fn a_shell_tab_hires_only_into_a_new_folder_under_the_hire_root() {
    let _home = k2_core::test_env::TempHome::new();
    let root = ensure_test_root();
    install();
    let (ws, _agent) = hiring_agent("shellown");
    let shell = Who::Shell { project_id: ws.clone(), session_id: uniq("tab") };
    let peer = root.join(uniq("peer")).to_string_lossy().into_owned();
    let (st, v) = cli(Who::Owner, json!({ "path": peer, "name": "Peer" }));
    assert_eq!(st, 200, "{v}");
    for path in [ws_path(&ws), peer] {
        let (st, e) = cli(shell.clone(), json!({ "template": "writer", "path": path, "launch": false }));
        assert_eq!((st, e["error"]["code"].as_str()), (403, Some("owner_only")), "{path}: {e}");
        let (st, e) = cli(shell.clone(), json!({ "path": path, "name": "Again" }));
        assert_eq!((st, e["error"]["code"].as_str()), (403, Some("owner_only")), "{path} plain: {e}");
    }
    // Beside the hire root, not inside it.
    let away = root.parent().unwrap().join(uniq("away")).to_string_lossy().into_owned();
    let (st, e) = cli(shell.clone(), json!({ "path": away, "name": "Away" }));
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("outside_hire_root")), "{e}");
    assert!(!Path::new(&away).exists(), "nothing made outside the root");
    let fresh = root.join(uniq("fresh")).to_string_lossy().into_owned();
    let (st, v) = cli(shell, json!({ "path": fresh, "name": "Fresh" }));
    assert_eq!(st, 200, "a new folder under the root is fine: {v}");
}

/// The real hire → the daemon's record → the first start: a card the
/// hirer couldn't give waits (no Thread yet), and is filed on the agent's
/// own Thread once it has one, exactly once, exactly the switches the
/// daemon recorded (an edited lock adds nothing), audited as the hirer and
/// the template. It waits while the new agent can't ask for cards.
#[test]
fn a_hires_card_waits_in_the_daemon_and_is_filed_once_at_the_first_start() {
    let _home = k2_core::test_env::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    ensure_test_root();
    install();
    set_cards(Some(true));
    let (hirer, agent) = hiring_agent("cardh");
    k2_core::workspace::settings::set_agents_can_request_permissions(&hirer, true).expect("hirer's cards");
    let (st, out) = cli(agent, json!({ "template": "developer", "name": uniq("Dev"), "launch": false }));
    assert_eq!(st, 200, "{out}");
    let id = out["id"].as_str().unwrap().to_string();
    let path = out["path"].as_str().unwrap().to_string();
    let handle = out["handle"].as_str().unwrap().to_string();
    assert_eq!(status(&out, CARDS_ROW), "done", "the hirer passes on its own cards switch: {out}");
    assert_eq!(status(&out, "compute.use"), "card", "{out}");
    assert!(row(&out, "compute.use")["reason"].as_str().unwrap().contains("when it first starts"), "{out}");
    assert_eq!(out["permissions"]["cardsPending"], json!(["compute.use"]), "{out}");
    let pending = || {
        let db = k2_core::db::shared();
        let conn = db.lock();
        records::pending_cards(&conn, &id).into_iter().map(|p| (p.action, p.template, p.hirer)).collect::<Vec<_>>()
    };
    assert_eq!(pending(), vec![("compute.use".to_string(), "Developer".to_string(), format!("agent:{hirer}"))]);
    assert_eq!(crate::permission::store::for_workspace(&id, 10).unwrap().len(), 0, "nothing filed yet");

    // The agent edits its own lock: nothing more is filed for it.
    let lock_path = Path::new(&path).join(".k2/agent/template.lock");
    let lock = std::fs::read_to_string(&lock_path).unwrap();
    std::fs::write(&lock_path, format!("cards_pending = [\"organization.manage\", \"vault.manage\"]\n{lock}")).unwrap();
    assert!(k2_core::hire::template::lock::read(&path).unwrap().is_some(), "the edited lock still reads");

    assert!(!file_pending_cards(&id), "no Thread yet: it waits");
    pin_thread(&id);
    k2_core::workspace::settings::set_agents_can_request_permissions(&id, false).unwrap();
    assert!(!file_pending_cards(&id), "the agent can't ask for cards: it waits");
    k2_core::workspace::settings::set_agents_can_request_permissions(&id, true).unwrap();
    set_cards(Some(false));
    assert!(!file_pending_cards(&id), "cards off on the server: it waits");
    set_cards(Some(true));
    assert!(file_pending_cards(&id), "the first start files it");
    let asked: Vec<String> = crate::permission::store::for_workspace(&id, 10).unwrap().into_iter().map(|r| r.action).collect();
    assert_eq!(asked, vec!["compute.use"], "exactly the daemon's record; the lock adds nothing");
    assert!(pending().is_empty());
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(records::state_of(&conn, &id, "compute.use", "card").as_deref(), Some("filed"));
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM hire_events WHERE action = ?1 AND reason = 'compute.use' AND result = 'card-filed' AND actor = ?2 AND detail LIKE ?3",
                rusqlite::params![AUDIT_EVENT, format!("agent:{hirer}"), format!("%\"workspaceId\":\"{id}\"%")],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "the filing is a hire_events row, by the hirer");
    }
    let filed: Vec<_> = audit_for(&handle).into_iter().filter(|(_, o)| o.starts_with("card-filed ")).collect();
    assert_eq!(filed.len(), 1, "{filed:?}");
    assert_eq!(filed[0].0, format!("agent:{hirer}"), "the hirer, not the agent asking");
    assert!(!file_pending_cards(&id), "once");
    assert_eq!(crate::permission::store::for_workspace(&id, 10).unwrap().len(), 1);
    set_cards(None);
}

/// People by role and API keys, through the real doors: an Admin gives
/// Admin-floor switches and asks for the owner's on a card; a Member gives
/// nothing, and its hire can't ask either (the agent gets no cards switch
/// from it); a key gives nothing.
#[test]
fn people_give_up_to_their_floor_and_keys_give_nothing() {
    let _home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    install();
    // Cards off: an Admin's app-owner hire.
    let admin = login(Role::Admin);
    let (st, out) = cli(admin.clone(), json!({ "template": "app-owner", "name": uniq("Apps"), "launch": false }));
    assert_eq!(st, 200, "{out}");
    let id = out["id"].as_str().unwrap().to_string();
    assert_eq!(status(&out, CARDS_ROW), "skip", "{out}");
    assert_eq!(status(&out, "apps.build"), "refused", "the owner's switch: {out}");
    assert!(row(&out, "apps.build")["hint"].as_str().unwrap().starts_with("The server's owner can turn on"), "{out}");
    assert_eq!(status(&out, "compute.use"), "done", "{out}");
    assert!(crate::first_run_grants::switch_on(&id, "compute"));
    assert_eq!(events(&id, "apps.build", "refused:owner_floor"), 1);

    // Cards on: the Admin's hire asks the owner; a Member's can't ask at all.
    set_cards(Some(true));
    let (st, out) = cli(admin, json!({ "template": "app-owner", "name": uniq("Apps"), "launch": false }));
    assert_eq!(st, 200, "{out}");
    let id = out["id"].as_str().unwrap().to_string();
    assert_eq!((status(&out, CARDS_ROW).as_str(), status(&out, "apps.build").as_str()), ("done", "card"), "{out}");
    assert_eq!(events(&id, "apps.build", "carded:owner_floor pending-first-start"), 1);
    let (st, out) = cli(login(Role::Member), json!({ "template": "developer", "name": uniq("Dev"), "launch": false }));
    assert_eq!(st, 200, "{out}");
    assert_eq!((status(&out, CARDS_ROW).as_str(), status(&out, "compute.use").as_str()), ("refused", "refused"), "{out}");
    assert!(row(&out, "compute.use")["reason"].as_str().unwrap().contains("can't ask for permission cards"), "{out}");
    let id = out["id"].as_str().unwrap().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert!(records::pending_cards(&conn, &id).is_empty(), "no card it could never file");
    }

    // An API key through /v1/w: nothing given, nothing asked.
    let key = hire_key("[]");
    let path = ensure_test_root().join(uniq("keyed")).to_string_lossy().into_owned();
    let (st, out) = v1w(&key, json!({ "path": path, "template": "developer" }));
    assert_eq!(st, 200, "{out}");
    let rows = out["template"]["rows"].as_array().cloned().expect("template rows");
    let perms: Vec<(String, String)> = rows
        .iter()
        .filter(|r| r["step"] == "permission")
        .map(|r| (r["target"].as_str().unwrap().to_string(), r["status"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(perms, vec![(CARDS_ROW.to_string(), "refused".to_string()), ("compute.use".to_string(), "refused".to_string())], "{out}");
    set_cards(None);
}

/// The recipe hire doesn't take `declinePermissions` yet: a 400 naming the
/// field and where it works, wherever it is in the request, and nothing is
/// made.
#[test]
fn the_recipe_hire_refuses_decline_permissions_by_name() {
    let _home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    let owner = V1Principal::Owner;
    let handle = uniq("rec");
    for body in [
        json!({ "agent": { "handle": handle, "template": "developer", "declinePermissions": ["compute.use"] } }),
        json!({ "members": [{ "handle": handle, "template": "developer", "declinePermissions": ["compute.use"] }] }),
        json!({ "agent": { "handle": handle, "template": "developer" }, "declinePermissions": ["compute.use"] }),
    ] {
        let r = crate::hire_routes::handle_v1(crate::hire_routes::V1_HIRE, &owner, true, body.to_string().as_bytes(), &HashMap::new(), None);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "unsupported_field", "{v}");
        assert_eq!(v["error"]["hint"], k2_core::hire::apply::DECLINE_UNSUPPORTED, "{v}");
        assert!(v["error"]["hint"].as_str().unwrap().contains("POST /v1/w or POST /cli/agents/hire"), "{v}");
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM projects WHERE handle = ?1", rusqlite::params![handle], |r| r.get(0)).expect("count");
    assert_eq!(n, 0, "nothing made");
}
