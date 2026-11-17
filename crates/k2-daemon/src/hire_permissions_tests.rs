//! The permissions step of a template hire (prd-agent-templates-v1 §9, T3):
//! reach per hirer, decline, cards now or at the first start, the hire root
//! for agent hirers, and the audit. Real shared test DB and temp hire root.
//! Lib tests never flip the server's cards switch (lane B's rule), so the
//! routes run with cards off and [`step_with`] takes cards on directly.

use std::collections::HashMap;
use std::path::Path;

use k2_core::db::schema::WorkspaceSession;
use k2_core::hire::template::permissions::{StepIn, CARDS_ROW};
use k2_core::hire::HireCaller;
use serde_json::{json, Value};

use super::*;
use crate::compute_routes::Who;
use crate::hire_routes::test_support::ensure_test_root;
use crate::hire_routes::{handle_cli, CLI_HIRE};

fn uniq(prefix: &str) -> String {
    format!("{prefix}-{}", &uuid::Uuid::new_v4().simple().to_string()[..10])
}

fn cli(who: Who, body: Value) -> (u16, Value) {
    let r = handle_cli(CLI_HIRE, who, true, body.to_string().as_bytes(), &HashMap::new(), None);
    let code = r.status.split_whitespace().next().and_then(|n| n.parse().ok()).expect("status");
    (code, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json {e}: {}", r.body)))
}

/// `(target, status)` of every permission row.
fn perms(v: &Value) -> Vec<(String, String)> {
    v["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("no rows: {v}"))
        .iter()
        .filter(|r| r["step"] == "permission")
        .map(|r| (r["target"].as_str().unwrap().to_string(), r["status"].as_str().unwrap().to_string()))
        .collect()
}

fn row<'a>(v: &'a Value, target: &str) -> &'a Value {
    v["rows"].as_array().unwrap().iter().find(|r| r["step"] == "permission" && r["target"] == target).unwrap_or_else(|| panic!("no {target}: {v}"))
}

fn sink() -> Vec<(String, String, String)> {
    crate::hire_routes::AUDIT_SINK.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// `hire_events` rows for one permission with one result.
fn events(action: &str, result: &str) -> i64 {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COUNT(*) FROM hire_events WHERE action = ?1 AND reason = ?2 AND result = ?3",
        rusqlite::params![AUDIT_EVENT, action, result],
        |r| r.get::<_, i64>(0),
    )
    .expect("count")
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

/// The owner gives every suggestion; the dry run says `grant`, the hire
/// says `done` and the switches are on; a re-run changes nothing. A switch
/// that is on by default ("Manage K2 projects") is `skip`. With the
/// server's cards off, "Can ask for permission cards" is `skip`.
#[test]
fn the_owner_grants_every_suggestion_once_and_it_is_audited() {
    let _home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    install();
    let name = uniq("Lead");
    let (st, plan) = cli(Who::Owner, json!({ "template": "lead", "name": name, "dryRun": true }));
    assert_eq!(st, 200, "{plan}");
    let want = ["agents.hire", "agents.manage", "connections.create"];
    for a in want {
        assert_eq!(row(&plan, a)["status"], "grant", "{plan}");
    }
    assert_eq!(row(&plan, "projects.manage")["status"], "grant", "a new folder: nothing is on yet: {plan}");
    assert_eq!(row(&plan, CARDS_ROW)["status"], "skip", "{plan}");
    assert!(row(&plan, CARDS_ROW)["reason"].as_str().unwrap().contains("off on this server"), "{plan}");
    assert!(!sink().iter().any(|(e, _, o)| e == AUDIT_EVENT && o.contains(&name.to_lowercase())), "a dry run audits nothing");

    let (st, out) = cli(Who::Owner, json!({ "template": "lead", "name": name, "launch": false }));
    assert_eq!(st, 200, "{out}");
    let id = out["id"].as_str().unwrap().to_string();
    for a in want {
        assert_eq!(row(&out, a)["status"], "done", "{out}");
        let sw = k2_core::hire::starter_permissions::standing_switch(a).unwrap();
        assert!(crate::first_run_grants::switch_on(&id, sw), "{a} on");
    }
    assert_eq!((row(&out, "projects.manage")["status"].as_str(), row(&out, "projects.manage")["reason"].as_str()), (Some("skip"), Some("already on")), "on by default: {out}");
    assert_eq!(out["permissions"]["granted"].as_array().unwrap().len(), 3, "{out}");
    let handle = out["handle"].as_str().unwrap().to_string();
    let lines: Vec<_> = sink().into_iter().filter(|(e, a, o)| e == AUDIT_EVENT && a == "owner-token" && o.ends_with(&handle)).collect();
    assert_eq!(lines.len(), 3, "one line per grant: {lines:?}");
    assert!(lines.iter().all(|(_, _, o)| o.starts_with("granted ")), "{lines:?}");
    assert!(events("agents.hire", "granted") >= 1, "a hire_events row per decision");
    let lock = std::fs::read_to_string(Path::new(out["path"].as_str().unwrap()).join(".k2/agent/template.lock")).unwrap();
    assert!(lock.contains("granted = [") && lock.contains("\"connections.create\""), "{lock}");

    // Again: already on, nothing changes, nothing more audited.
    let before = sink().len();
    let (_, again) = cli(Who::Owner, json!({ "template": "lead", "name": name }));
    assert_eq!(again["changed"], false, "{again}");
    for a in want {
        assert_eq!((row(&again, a)["status"].as_str(), row(&again, a)["reason"].as_str()), (Some("skip"), Some("already on")), "{again}");
    }
    assert_eq!(sink().iter().skip(before).filter(|(e, _, _)| e == AUDIT_EVENT).count(), 0);
}

/// The person unticks a suggestion: it stays off and is audited as
/// declined. A declined id the template doesn't suggest is a usage error.
#[test]
fn a_declined_suggestion_stays_off() {
    let _home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    install();
    let name = uniq("Dev");
    let (st, e) = cli(Who::Owner, json!({ "template": "developer", "name": name, "declinePermissions": ["compute.usee"], "dryRun": true }));
    assert_eq!((st, e["error"]["code"].as_str()), (400, Some("usage")), "{e}");
    assert!(e["error"]["hint"].as_str().unwrap().contains("it suggests: permission-cards, compute.use"), "{e}");
    let (st, out) = cli(Who::Owner, json!({ "template": "developer", "name": name, "declinePermissions": ["compute.use"], "launch": false }));
    assert_eq!(st, 200, "{out}");
    assert_eq!(row(&out, "compute.use")["status"], "declined", "{out}");
    assert_eq!(out["permissions"]["declined"], json!(["compute.use"]), "{out}");
    let id = out["id"].as_str().unwrap();
    assert!(!crate::first_run_grants::switch_on(id, "compute"), "declined stays off");
    assert!(sink().iter().any(|(e, _, o)| e == AUDIT_EVENT && o.starts_with("declined compute.use")), "{:?}", sink());
    assert!(events("compute.use", "declined") >= 1);
    // Remembered: a re-run without the decline leaves it off.
    let (_, again) = cli(Who::Owner, json!({ "template": "developer", "name": name }));
    assert_eq!(row(&again, "compute.use")["status"], "declined", "{again}");
    assert!(!crate::first_run_grants::switch_on(id, "compute"));
    // Without a template the field is refused.
    let path = ensure_test_root().join(uniq("plain")).to_string_lossy().into_owned();
    let (st, e) = cli(Who::Owner, json!({ "path": path, "declinePermissions": ["compute.use"] }));
    assert_eq!(st, 400, "{e}");
}

/// An agent passes on only what it holds itself; the rest is refused with
/// a hint naming who can turn it on and the command (cards off). It hires
/// only inside the hire root.
#[test]
fn an_agent_passes_on_only_what_it_holds_and_hires_only_under_the_root() {
    let _home = k2_core::test_env::TempHome::new();
    let root = ensure_test_root();
    install();
    let (ws, agent) = hiring_agent("pass");
    k2_core::workspace::settings::set_agents_can_manage_agents(&ws, false).unwrap();
    k2_core::workspace::settings::update_project_setting(
        &crate::sidecar_routes::workspace_by_id(&ws).unwrap().path,
        "agents_can_create_connections",
        "1",
    )
    .unwrap();
    let (st, out) = cli(agent.clone(), json!({ "template": "lead", "name": uniq("Team"), "launch": false }));
    assert_eq!(st, 200, "{out}");
    assert_eq!(row(&out, "agents.hire")["status"], "done", "it holds agents.hire: {out}");
    assert_eq!(row(&out, "connections.create")["status"], "done", "{out}");
    assert_eq!(row(&out, "projects.manage")["status"], "skip", "on by default: {out}");
    assert_eq!(row(&out, CARDS_ROW)["status"], "skip", "cards off on this server: {out}");
    let refused = row(&out, "agents.manage");
    assert_eq!(refused["status"], "refused", "{out}");
    let handle = out["handle"].as_str().unwrap();
    let hint = refused["hint"].as_str().unwrap();
    assert!(hint.contains(&format!("k2 agent access set {handle} agents_manage on")), "{hint}");
    assert!(hint.starts_with("An owner or admin can turn on"), "{hint}");
    assert!(refused["reason"].as_str().unwrap().contains("permission cards are off"), "{out}");
    assert!(out["permissions"]["refused"].as_array().unwrap().iter().any(|r| r["action"] == "agents.manage" && r["code"] == "hirer_lacks"), "{out}");
    let new_id = out["id"].as_str().unwrap();
    assert!(!crate::first_run_grants::switch_on(new_id, "agents_manage"));
    assert!(crate::first_run_grants::switch_on(new_id, "agents_hire"));
    // The agent's grants carry its own agent-access-set line.
    assert!(sink().iter().any(|(e, a, o)| e == "agent-access-set" && a.starts_with("agent:") && o.contains("agents_hire=true")), "{:?}", sink());
    assert!(sink().iter().any(|(e, _, o)| e == AUDIT_EVENT && o.starts_with("refused:hirer_lacks agents.manage")), "{:?}", sink());
    // Outside the hire root: refused before anything is written.
    let outside = k2_core::test_env::stable_temp_dir().join(uniq("outside")).to_string_lossy().into_owned();
    assert!(!Path::new(&outside).starts_with(&root));
    let (st, e) = cli(agent.clone(), json!({ "template": "writer", "path": outside, "dryRun": true }));
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("outside_hire_root")), "{e}");
    let (st, e) = cli(agent, json!({ "path": outside, "name": "Out" }));
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("outside_hire_root")), "{e}");
    assert!(!Path::new(&outside).exists());
}

/// A registered workspace with (or without) a pinned Thread.
fn target(with_thread: bool) -> (String, String) {
    let path = ensure_test_root().join(uniq("tgt")).to_string_lossy().into_owned();
    let (st, v) = cli(Who::Owner, json!({ "template": "general", "path": path, "launch": false }));
    assert_eq!(st, 200, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    if with_thread {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let conv = uuid::Uuid::new_v4().to_string();
        WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").expect("pin");
    }
    (id, v["path"].as_str().unwrap().to_string())
}

fn step_in<'a>(caller: &'a HireCaller, path: &'a str, suggested: &'a [String], cards: bool, dry: bool) -> StepIn<'a> {
    StepIn { caller, path, template_name: "Developer", cards, suggested, declined: &[], asked_before: &[], dry_run: dry }
}

/// Cards on: what the hirer can't give is asked of a person. With a
/// Thread the card is filed now; without one it waits for the first start
/// and is filed once, then.
#[test]
fn with_cards_on_the_rest_goes_to_a_card_now_or_at_the_first_start() {
    let _home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    // It files permission requests: its own database (the pending cap is
    // server-wide).
    let _db = k2_core::db::scoped_for_test();
    let (hirer, _) = hiring_agent("cardh");
    let agent = HireCaller::Agent { workspace_id: hirer };
    let suggested = vec!["compute.use".to_string()];

    // Dry run: `card`, and "Can ask" is refused (the hirer can't ask itself).
    let (_, path) = target(true);
    let plan = step_with(&step_in(&agent, &path, &suggested, true, true), true);
    let st: Vec<(&str, &str)> = plan.rows.iter().map(|r| (r.target.as_str(), r.status)).collect();
    assert_eq!(st, vec![(CARDS_ROW, "refused"), ("compute.use", "card")], "{plan:?}");
    assert!(plan.rows[0].hint.as_deref().unwrap().contains("permission_cards on"), "{plan:?}");

    // With a Thread: filed now.
    let (id, path) = target(true);
    let out = step_with(&step_in(&agent, &path, &suggested, false, false), true);
    assert_eq!(out.rows[0].status, "card", "{out:?}");
    assert_eq!((out.carded.clone(), out.cards_pending.clone()), (vec!["compute.use".to_string()], vec![]));
    let asked = crate::permission::store::for_workspace(&id, 10).unwrap();
    assert!(asked.iter().any(|r| r.action == "compute.use"), "a card for a person: {asked:?}");

    // Never asked twice on a re-apply.
    let again = step_with(
        &StepIn { asked_before: &suggested, ..step_in(&agent, &path, &suggested, false, false) },
        true,
    );
    assert_eq!((again.rows[0].status, again.carded.len()), ("card", 0), "{again:?}");
    assert_eq!(crate::permission::store::for_workspace(&id, 10).unwrap().len(), asked.len());

    // No Thread: pending; the lock carries it; the first start files it once.
    let (id, path) = target(false);
    let out = step_with(&step_in(&agent, &path, &suggested, false, false), true);
    assert_eq!(out.cards_pending, vec!["compute.use".to_string()], "{out:?}");
    let mut l = k2_core::hire::template::lock::read(&path).unwrap().expect("lock");
    l.cards_pending = out.cards_pending.clone();
    k2_core::hire::template::lock::write(&path, &l).unwrap();
    assert!(!file_pending_cards_with(&id, true), "no Thread yet: they wait");
    assert!(!file_pending_cards_with(&id, false), "cards off: they wait");
    assert!(k2_core::hire::template::lock::has_pending_cards(&path));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let conv = uuid::Uuid::new_v4().to_string();
        WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running").expect("pin");
    }
    assert!(!file_pending_cards_with(&id, false), "cards off on the server: they still wait");
    assert!(k2_core::hire::template::lock::has_pending_cards(&path));
    assert!(file_pending_cards_with(&id, true));
    assert!(!k2_core::hire::template::lock::has_pending_cards(&path), "taken once");
    assert!(crate::permission::store::for_workspace(&id, 10).unwrap().iter().any(|r| r.action == "compute.use"));
    assert!(!file_pending_cards_with(&id, true), "nothing left");
    assert!(sink().iter().any(|(e, _, o)| e == AUDIT_EVENT && o.starts_with("card-filed compute.use")), "{:?}", sink());
}

/// People by role and API keys: an Admin gives Admin-floor switches and
/// asks for the owner's; a Member and a key give nothing (cards off: a
/// refusal with a hint; cards on: a card).
#[test]
fn people_give_up_to_their_floor_and_keys_give_nothing() {
    let home = k2_core::test_env::TempHome::new();
    ensure_test_root();
    let admin = format!("adm{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let member = format!("mem{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    for (u, role) in [(&admin, k2_core::connect_users::Role::Admin), (&member, k2_core::connect_users::Role::Member)] {
        k2_core::connect_users::add_user(u, "password123").expect("add_user");
        if role != k2_core::connect_users::Role::Member {
            k2_core::connect_users::set_role(u, role).expect("set_role");
        }
    }
    let _ = &home;
    let suggested = vec!["apps.build".to_string(), "compute.use".to_string()];
    let (_, path) = target(true);
    let by_admin = HireCaller::Login { username: admin };
    let plan = step_with(&step_in(&by_admin, &path, &suggested, true, true), false);
    let st: Vec<(&str, &str)> = plan.rows.iter().map(|r| (r.target.as_str(), r.status)).collect();
    assert_eq!(st, vec![(CARDS_ROW, "skip"), ("apps.build", "refused"), ("compute.use", "grant")], "{plan:?}");
    assert!(plan.rows[1].hint.as_deref().unwrap().starts_with("The server's owner can turn on"), "{plan:?}");
    let plan = step_with(&step_in(&by_admin, &path, &suggested, true, true), true);
    let st: Vec<(&str, &str)> = plan.rows.iter().map(|r| (r.target.as_str(), r.status)).collect();
    assert_eq!(st, vec![(CARDS_ROW, "grant"), ("apps.build", "card"), ("compute.use", "grant")], "cards on: the owner is asked: {plan:?}");

    let by_member = HireCaller::Login { username: member };
    let plan = step_with(&step_in(&by_member, &path, &suggested, false, true), false);
    assert!(plan.rows.iter().all(|r| r.status == "refused"), "{plan:?}");
    let plan = step_with(&step_in(&by_member, &path, &suggested, false, true), true);
    assert!(plan.rows.iter().all(|r| r.status == "card"), "{plan:?}");

    let key = HireCaller::ApiKey { id: "k1".into(), label: None };
    let plan = step_with(&step_in(&key, &path, &suggested, true, true), false);
    let st: Vec<(&str, &str)> = plan.rows.iter().map(|r| (r.target.as_str(), r.status)).collect();
    assert_eq!(st, vec![(CARDS_ROW, "skip"), ("apps.build", "refused"), ("compute.use", "refused")], "cards off: {plan:?}");
    assert!(plan.refused.iter().all(|r| r.code == "api_key"), "{plan:?}");
    let plan = step_with(&step_in(&key, &path, &suggested, true, true), true);
    let st: Vec<(&str, &str)> = plan.rows.iter().map(|r| (r.target.as_str(), r.status)).collect();
    assert_eq!(st, vec![(CARDS_ROW, "refused"), ("apps.build", "card"), ("compute.use", "card")], "cards on: {plan:?}");
}
