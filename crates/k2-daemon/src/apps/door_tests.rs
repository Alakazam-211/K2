//! The 0.46.1 door, in process (`prd-k2-app-runtime-v1` §5.3–5.6, §5.11;
//! S19, S20, S42, AR10, AR43): suspend/resume, remote passes, quotas,
//! held deploys and first installs, the held `manage` request for agents
//! without the switch (and its replay), appliers for other lanes' kinds,
//! and the agent permissions matrix.

use std::collections::HashMap;
use std::sync::Mutex;

use k2_core::app_grace::requests as reqs;
use k2_core::connect_users::Role;
use k2_core::deploy::store;
use rusqlite::params;
use serde_json::{json, Value as J};

use super::egress_routes::test_seams;
use super::routes::Who;

static MSGS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    ws_a: String,
    ws_b: String,
}

fn ws_path(ws: &str) -> String {
    format!("/tmp/k2-door-test/{ws}")
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    let (ws_a, ws_b) = (format!("ws-a-{tag}"), format!("ws-b-{tag}"));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for ws in [&ws_a, &ws_b] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![ws, ws_path(ws)]).unwrap();
        }
        for (app, ws, iso) in [("jobs", &ws_a, "container"), ("other", &ws_b, "container"), ("plain", &ws_a, "process")] {
            store::insert_app(
                &conn,
                &store::NewApp {
                    app_id: app,
                    service_id: &format!("svc-{app}-{tag}"),
                    seed_id: None,
                    owner_workspace: ws,
                    isolation: iso,
                    memory_limit_bytes: None,
                    front_port: 0,
                    created_by: "owner",
                },
            )
            .unwrap();
        }
        // Seeds: caps are the manifest's; reports are valid.
        for (hash, seed_id, caps) in [
            ("2222222222222222222222222222222222222222222222222222222222222222", "jobs", json!(["thread:read", "files:write"])),
            ("1111111111111111111111111111111111111111111111111111111111111111", "jobs", json!(["thread:read"])),
            ("3333333333333333333333333333333333333333333333333333333333333333", "jobs", json!(["thread:read", "files:write", "store:read"])),
            ("0000000000000000000000000000000000000000000000000000000000000000", "bare", json!([])),
        ] {
            conn.execute(
                "INSERT INTO seeds (hash, seed_id, host_mode, manifest_json, created_with, report_json, signer, size_bytes, created_at, last_used_at) \
                 VALUES (?1, ?2, 'cmd', ?3, '{}', '{\"ok\":true}', 'owner', 1, 1, 1)",
                params![hash, seed_id, json!({ "caps": caps }).to_string()],
            )
            .unwrap();
        }
    }
    MSGS.lock().unwrap().clear();
    super::alerts::set_sink(Some(Box::new(|ws: &str, text: &str| MSGS.lock().unwrap().push((ws.into(), text.into())))));
    Fx { _db: db, _home: home, ws_a, ws_b }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn admin() -> Who {
    Who::Login { username: "ada".into(), role: Role::Admin }
}

fn owner_login() -> Who {
    Who::Login { username: "rosson".into(), role: Role::Owner }
}

fn switch(ws: &str, on: bool) {
    k2_core::workspace::settings::set_agents_can_manage_skin(&ws_path(ws), on).unwrap();
}

/// (HTTP status, JSON body).
fn call(path: &str, who: &Who, body: J) -> (u16, J) {
    let r = super::routes::handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new());
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn get(path: &str, who: &Who, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = super::routes::handle(path, who.clone(), b"", &p);
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

fn svc(app: &str) -> String {
    let h = k2_core::db::shared();
    let conn = h.lock();
    store::get_app(&conn, app).unwrap().unwrap().service_id
}

fn approve(req: &str, who: &Who, stamp: &str) -> J {
    let tok = test_seams::vault_handle().confirm.mint(super::approvals::CONFIRM_ACTION, Some(req), stamp, "local-auth");
    let r = super::approvals::handle("/cli/app/approve", who, json!({ "request": req, "confirmToken": tok }).to_string().as_bytes(), &HashMap::new());
    r.unwrap_or_else(|e| panic!("approve {req}: {} {}", e.code, e.hint))
}

fn recorded(app: &str) -> Option<Vec<String>> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    super::grants::recorded(&conn, app).unwrap()
}

#[test]
fn suspend_revokes_every_pass_in_one_step_and_only_the_owner_resumes() {
    let f = fx("suspend");
    let s = svc("jobs");
    let caps = vec!["thread:read".to_string()];
    let link = k2_core::skin::link::mint_link_pass(&s, "jobs", "gw:38601", &caps, &[f.ws_a.clone()]).unwrap();
    let remote = k2_core::skin::link::mint_remote_pass(&s, "jobs", &caps, &[f.ws_a.clone()], 3600).unwrap();
    assert!(k2_core::skin::resolve_skin_token(&remote.raw).is_some());

    // Another workspace's agent never finds it; its own agent may suspend
    // (it only narrows), without any switch.
    switch(&f.ws_a, false);
    let (st, v) = call("/cli/app/suspend", &agent(&f.ws_b), json!({"app": "jobs"}));
    assert_eq!((st, code(&v)), (404, "app_not_found"), "{v}");
    let (st, v) = call("/cli/app/suspend", &agent(&f.ws_a), json!({"app": "jobs", "reason": "leaking"}));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["passesRevoked"], 2, "link and remote, one step: {v}");
    assert_eq!(v["suspension"]["suspended"], true);
    assert_eq!(v["suspension"]["reason"], "leaking");
    assert!(k2_core::skin::link::resolve_link_pass(&link.raw, "gw:38601", &s).is_none(), "the next link call is 401");
    assert!(k2_core::skin::resolve_skin_token(&remote.raw).is_none(), "the remote pass is dead too");
    // Again: idempotent.
    let (st, v) = call("/cli/app/suspend", &admin(), json!({"app": "jobs"}));
    assert_eq!((st, v["already"].as_bool()), (200, Some(true)), "{v}");

    // Frozen: deploys, starts, rollbacks, the link and remote passes.
    for (path, body) in [
        ("/cli/app/deploy", json!({"app": "jobs", "seed": "1111111111111111111111111111111111111111111111111111111111111111"})),
        ("/cli/app/start", json!({"app": "jobs"})),
        ("/cli/app/rollback", json!({"app": "jobs"})),
    ] {
        let (st, v) = call(path, &Who::Owner, body);
        assert_eq!((st, code(&v)), (409, "app_suspended"), "{path}: {v}");
    }
    let (st, v) = call("/cli/app/remote-pass/rotate", &Who::Owner, json!({"app": "jobs"}));
    assert_eq!((st, code(&v)), (409, "app_suspended"), "{v}");
    let spec = super::gateway::LinkSpec {
        app_id: "jobs".into(),
        service_id: s.clone(),
        workspace_id: f.ws_a.clone(),
        port: 0,
        caps: caps.clone(),
        rooms: vec![f.ws_a.clone()],
    };
    assert!(super::gateway::open(spec, None).unwrap_err().contains("suspended"), "no link for a suspended App");

    // Resume: never an agent or an admin; the owner (token or login).
    for who in [agent(&f.ws_a), admin()] {
        let (st, v) = call("/cli/app/resume", &who, json!({"app": "jobs"}));
        assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    }
    let (st, v) = call("/cli/app/resume", &owner_login(), json!({"app": "jobs"}));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["suspension"]["suspended"], false);
    assert!(!super::suspend::is_suspended("jobs"));
}

#[test]
fn remote_passes_are_the_owners_to_mint_and_anyones_to_revoke() {
    let f = fx("remote");
    let s = svc("jobs");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        super::grants::record(&conn, "jobs", &["thread:read".to_string()]).unwrap();
        conn.execute("UPDATE apps SET live_seed = '2222222222222222222222222222222222222222222222222222222222222222' WHERE app_id = 'jobs'", []).unwrap();
    }
    for who in [agent(&f.ws_a), admin()] {
        let (st, v) = call("/cli/app/remote-pass/rotate", &who, json!({"app": "jobs"}));
        assert_eq!((st, code(&v)), (403, "owner_only"), "{v}");
    }
    let (st, v) = call("/cli/app/remote-pass/rotate", &owner_login(), json!({"app": "jobs", "ttl": "2d"}));
    assert_eq!(st, 200, "{v}");
    // The grants formula: manifest (thread:read, files:write) ∩ approved (thread:read).
    assert_eq!(v["caps"], json!(["thread:read"]));
    assert_eq!(v["withheld"], json!(["files:write"]));
    assert_eq!(v["rooms"], json!([f.ws_a]));
    assert_eq!(v["expiresAt"].as_i64().unwrap() - k2_core::deploy::now(), 2 * 86_400);
    let first = v["pass"].as_str().unwrap().to_string();
    let p = k2_core::skin::resolve_skin_token(&first).expect("works on the main port");
    assert_eq!(k2_core::skin::link::remote_pass_app(&p.id).as_deref(), Some(s.as_str()));
    assert!(k2_core::skin::link::resolve_link_pass(&first, "gw:38601", &s).is_none(), "never on a link");
    // Rotate: a new pass; the old one is revoked unless keepOld.
    let (_, v) = call("/cli/app/remote-pass/rotate", &Who::Owner, json!({"app": "jobs"}));
    assert_eq!(v["revoked"], 1, "{v}");
    let second = v["pass"].as_str().unwrap().to_string();
    assert!(k2_core::skin::resolve_skin_token(&first).is_none());
    let (st, v) = get("/cli/app/remote-pass", &agent(&f.ws_a), &[("app", "jobs")]);
    assert_eq!((st, v["live"].as_u64()), (200, Some(1)), "{v}");
    assert!(!v.to_string().contains(&first) && !v.to_string().contains(second.as_str()), "never the secret: {v}");
    let (st, _) = get("/cli/app/remote-pass", &agent(&f.ws_b), &[("app", "jobs")]);
    assert_eq!(st, 404);
    let (st, v) = call("/cli/app/remote-pass/revoke", &agent(&f.ws_a), json!({"app": "jobs"}));
    assert_eq!((st, v["revoked"].as_u64()), (200, Some(1)), "{v}");
}

#[test]
fn quotas_lower_at_once_and_raises_are_the_owners() {
    let f = fx("quota");
    // Its own App id: the gateway's quota cache is process-wide.
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        store::insert_app(
            &conn,
            &store::NewApp {
                app_id: "qapp",
                service_id: "svc-qapp",
                seed_id: None,
                owner_workspace: &f.ws_a,
                isolation: "container",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "owner",
            },
        )
        .unwrap();
    }
    let (st, v) = get("/cli/app/quota", &agent(&f.ws_a), &[("app", "qapp")]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["quotas"]["rps"], 50);
    assert_eq!(v["quotas"]["bodyBytes"], 8 * 1024 * 1024);
    switch(&f.ws_a, true);
    let (st, v) = call("/cli/app/quota/set", &agent(&f.ws_a), json!({"app": "qapp", "rps": 5, "bodyBytes": "1M"}));
    assert_eq!(st, 200, "{v}");
    assert_eq!((v["quotas"]["rps"].as_u64(), v["quotas"]["bodyBytes"].as_u64()), (Some(5), Some(1024 * 1024)));
    assert_eq!(super::quotas::quotas_for("qapp").rps, 5, "the gateway sees it at once");
    // A raise from the agent is held for the owner.
    let (st, v) = call("/cli/app/quota/set", &agent(&f.ws_a), json!({"app": "qapp", "rps": 500}));
    assert_eq!((st, v["kind"].as_str()), (202, Some("quota")), "{v}");
    let id = v["request"]["id"].as_str().unwrap().to_string();
    assert_eq!(super::quotas::quotas_for("qapp").rps, 5, "held is not live");
    let out = approve(&id, &owner_login(), "connect:rosson");
    assert_eq!(out["status"], "approved", "{out}");
    assert_eq!(super::quotas::quotas_for("qapp").rps, 500);
    // The owner raises directly; nonsense is refused.
    let (st, _) = call("/cli/app/quota/set", &Who::Owner, json!({"app": "qapp", "sockets": 128}));
    assert_eq!(st, 200);
    let (st, v) = call("/cli/app/quota/set", &Who::Owner, json!({"app": "qapp", "rps": 0}));
    assert_eq!((st, code(&v)), (400, "bad_request"), "{v}");
    let (st, v) = call("/cli/app/quota/set", &Who::Owner, json!({"app": "qapp", "reset": true}));
    assert_eq!((st, v["quotas"]["rps"].as_u64()), (200, Some(50)), "{v}");
}

#[test]
fn deploys_adding_caps_and_first_installs_are_held_for_every_caller() {
    let f = fx("deploy");
    // First install of a guarded App, the owner token included (AR14).
    let (st, v) = call("/cli/app/deploy", &Who::Owner, json!({"app": "jobs", "seed": "2222222222222222222222222222222222222222222222222222222222222222"}));
    assert_eq!((st, v["status"].as_str(), v["kind"].as_str()), (202, Some("held"), Some("install")), "{v}");
    assert_eq!(v["diff"]["caps"]["add"], json!(["thread:read", "files:write"]));
    let id = v["request"]["id"].as_str().unwrap().to_string();
    assert_eq!(recorded("jobs"), None, "nothing approved while held");
    // The agent with its switch on is held the same way.
    switch(&f.ws_a, true);
    let (st, v) = call("/cli/app/deploy", &agent(&f.ws_a), json!({"app": "jobs", "seed": "2222222222222222222222222222222222222222222222222222222222222222"}));
    assert_eq!((st, v["kind"].as_str()), (202, Some("install")), "{v}");
    // Only the owner approves; the approved set is recorded.
    let r = super::approvals::handle(
        "/cli/app/approve",
        &agent(&f.ws_a),
        json!({"request": id}).to_string().as_bytes(),
        &HashMap::new(),
    );
    assert_eq!(r.unwrap_err().code, "owner_only");
    let out = approve(&id, &owner_login(), "connect:rosson");
    assert_eq!(out["request"]["decidedBy"], "owner:rosson", "{out}");
    assert_eq!(recorded("jobs"), Some(vec!["files:write".to_string(), "thread:read".to_string()]));

    let check = |app: &str, seed: &str| {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let row = store::get_app(&conn, app).unwrap().unwrap();
        super::grants::check_deploy(&conn, &row, seed).unwrap()
    };
    use super::grants::DeployCheck;
    assert_eq!(check("jobs", "1111111111111111111111111111111111111111111111111111111111111111"), DeployCheck::Fits, "narrowing never waits");
    assert_eq!(check("jobs", "2222222222222222222222222222222222222222222222222222222222222222"), DeployCheck::Fits);
    assert_eq!(
        check("jobs", "3333333333333333333333333333333333333333333333333333333333333333"),
        DeployCheck::Held {
            kind: reqs::KIND_DEPLOY,
            add: vec!["store:read".into()],
            set: vec!["files:write".into(), "thread:read".into(), "store:read".into()]
        }
    );
    let (st, v) = call("/cli/app/deploy", &Who::Owner, json!({"app": "jobs", "seed": "3333333333333333333333333333333333333333333333333333333333333333"}));
    assert_eq!((st, v["kind"].as_str()), (202, Some("deploy")), "{v}");
    assert_eq!(v["diff"]["lines"], json!(["+ cap store:read"]));
    // An App without a pass (the stage-1 process engine) holds nothing.
    assert_eq!(check("plain", "3333333333333333333333333333333333333333333333333333333333333333"), DeployCheck::Fits);
    // A first install asking for nothing, or fitting an earlier approval
    // of the same Seed id, records its set and goes on.
    assert_eq!(check("other", "0000000000000000000000000000000000000000000000000000000000000000"), DeployCheck::FirstFits(vec![]));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("UPDATE apps SET seed_id = 'jobs' WHERE app_id = 'jobs'", []).unwrap();
    }
    assert_eq!(check("other", "1111111111111111111111111111111111111111111111111111111111111111"), DeployCheck::FirstFits(vec!["thread:read".into()]));
    // The link pass: manifest ∩ approved.
    let e = super::grants::narrow_for_link("jobs", &["thread:read".into(), "store:read".into()], &f.ws_a);
    assert_eq!((e.caps, e.withheld), (vec!["thread:read".to_string()], vec!["store:read".to_string()]));
    let _ = &f.ws_b;
}

#[test]
fn an_agent_without_the_switch_gets_a_held_request_the_owner_can_replay() {
    let f = fx("manage");
    switch(&f.ws_a, false);
    let (st, v) = call("/cli/app/stop", &agent(&f.ws_a), json!({"app": "jobs", "reason": "it loops"}));
    assert_eq!((st, v["status"].as_str(), v["kind"].as_str()), (202, Some("held"), Some("manage")), "{v}");
    assert!(v["hint"].as_str().unwrap().contains("k2 app suspend"), "{v}");
    let id = v["request"]["id"].as_str().unwrap().to_string();
    assert_eq!(v["request"]["summary"], "k2 app stop jobs");
    assert_eq!(v["request"]["reason"], "it loops");
    // The agent sees its own request; the other workspace doesn't.
    let mine = super::approvals::handle("/cli/app/approvals", &agent(&f.ws_a), b"", &HashMap::new()).unwrap();
    assert_eq!(mine["requests"].as_array().unwrap().len(), 1);
    assert!(mine["ownerConfirm"]["available"].is_boolean(), "{mine}");
    let theirs = super::approvals::handle("/cli/app/approvals", &agent(&f.ws_b), b"", &HashMap::new()).unwrap();
    assert!(theirs["requests"].as_array().unwrap().is_empty());
    // The owner approves: the stop runs as the agent asked.
    let out = approve(&id, &Who::Owner, "owner");
    assert_eq!(out["status"], "approved", "{out}");
    assert_eq!(out["result"]["applied"], "/cli/app/stop");
    // With the switch on the agent acts at once.
    switch(&f.ws_a, true);
    let (st, v) = call("/cli/app/stop", &agent(&f.ws_a), json!({"app": "jobs"}));
    assert_eq!(st, 200, "{v}");
}

#[test]
fn other_lanes_kinds_apply_through_their_registered_applier() {
    let f = fx("applier");
    let file = |kind: &'static str| {
        let h = k2_core::db::shared();
        let app = {
            let conn = h.lock();
            store::get_app(&conn, "jobs").unwrap().unwrap()
        };
        super::approvals::file(&super::approvals::Filing {
            who: &agent(&f.ws_a),
            kind,
            app: Some(&app),
            label: "jobs",
            workspace: &f.ws_a,
            body: json!({ "memory": "8G" }),
            diff: json!({ "lines": ["memory 8G (past the headroom)"] }),
            summary: format!("{kind} for jobs"),
            reasons: vec!["test".into()],
            reason: None,
        })
        .unwrap()
        .id
    };
    let a = file(reqs::KIND_OVERCOMMIT);
    let out = approve(&a, &Who::Owner, "owner");
    assert_eq!((out["status"].as_str(), out["error"]["code"].as_str()), (Some("failed"), Some("no_applier")), "{out}");
    fn ok(r: &reqs::AppRequest, by: &str) -> super::routes::R<J> {
        Ok(json!({ "applied": r.kind, "by": by }))
    }
    super::approvals::register_applier(reqs::KIND_DB_RAW, ok);
    let b = file(reqs::KIND_DB_RAW);
    let out = approve(&b, &Who::Owner, "owner");
    assert_eq!((out["status"].as_str(), out["result"]["applied"].as_str()), (Some("approved"), Some("db_raw")), "{out}");
}

/// §12.1 agent permissions matrix (the door's routes): for each switch
/// state, each `/cli/app/*` route runs, is held, or is refused.
#[test]
fn agent_permissions_matrix() {
    let f = fx("matrix");
    #[derive(Debug, PartialEq, Clone, Copy)]
    enum O {
        Run,
        Held,
        Refused,
    }
    let outcome = |st: u16, v: &J| -> O {
        if st == 202 && v["status"] == "held" {
            return O::Held;
        }
        match v["error"]["code"].as_str() {
            Some("owner_only" | "apps_manage_off" | "forbidden" | "wrong_credential") => O::Refused,
            _ => O::Run,
        }
    };
    // (route, POST?, body, no switch, manage, manage+open)
    let ghost = json!({"app": "ghost-app"});
    let rows: Vec<(&str, bool, J, O, O, O)> = vec![
        // Reads stay open.
        ("/cli/app/list", false, json!({}), O::Run, O::Run, O::Run),
        ("/cli/app/inspect", false, json!({"app": "jobs"}), O::Run, O::Run, O::Run),
        ("/cli/app/history", false, json!({"app": "jobs"}), O::Run, O::Run, O::Run),
        ("/cli/app/logs", false, json!({"app": "jobs"}), O::Run, O::Run, O::Run),
        ("/cli/app/errors", false, json!({"app": "jobs"}), O::Run, O::Run, O::Run),
        ("/cli/app/audit", false, json!({"app": "jobs"}), O::Run, O::Run, O::Run),
        ("/cli/app/quota", false, json!({"app": "jobs"}), O::Run, O::Run, O::Run),
        ("/cli/app/link", false, json!({"app": "jobs"}), O::Run, O::Run, O::Run),
        ("/cli/app/remote-pass", false, json!({"app": "jobs"}), O::Run, O::Run, O::Run),
        // Changes: held without the switch, run with it.
        ("/cli/app/build", true, json!({}), O::Held, O::Run, O::Run),
        ("/cli/app/install", true, json!({}), O::Held, O::Run, O::Run),
        ("/cli/app/deploy", true, ghost.clone(), O::Held, O::Run, O::Run),
        ("/cli/app/rollback", true, ghost.clone(), O::Held, O::Run, O::Run),
        ("/cli/app/stop", true, ghost.clone(), O::Held, O::Run, O::Run),
        ("/cli/app/start", true, ghost.clone(), O::Held, O::Run, O::Run),
        ("/cli/app/restart", true, ghost.clone(), O::Held, O::Run, O::Run),
        ("/cli/app/config", true, ghost.clone(), O::Held, O::Run, O::Run),
        ("/cli/app/uninstall", true, ghost.clone(), O::Held, O::Run, O::Run),
        ("/cli/app/link/rotate", true, ghost.clone(), O::Held, O::Run, O::Run),
        ("/cli/app/exec", true, ghost.clone(), O::Held, O::Run, O::Run),
        ("/cli/app/quota/set", true, json!({"app": "jobs", "rps": 10}), O::Held, O::Run, O::Run),
        // Narrowing never waits, with or without the switch.
        ("/cli/app/suspend", true, ghost.clone(), O::Run, O::Run, O::Run),
        ("/cli/app/remote-pass/revoke", true, json!({"app": "jobs"}), O::Run, O::Run, O::Run),
        // Human-presence and owner-only: never an agent.
        ("/cli/app/resume", true, json!({"app": "jobs"}), O::Refused, O::Refused, O::Refused),
        ("/cli/app/remote-pass/rotate", true, json!({"app": "jobs"}), O::Refused, O::Refused, O::Refused),
    ];
    let a = agent(&f.ws_a);
    for (state, (manage, open)) in [(0, (false, false)), (1, (true, false)), (2, (true, true))] {
        switch(&f.ws_a, manage);
        {
            let h = k2_core::db::shared();
            let conn = h.lock();
            conn.execute("UPDATE projects SET agents_can_publish_open_apps = ?2 WHERE id = ?1", params![f.ws_a, open as i64]).unwrap();
        }
        for (path, post, body, o0, o1, o2) in &rows {
            let (st, v) = if *post {
                call(path, &a, body.clone())
            } else {
                let q: Vec<(&str, &str)> = body.as_object().unwrap().iter().map(|(k, v)| (k.as_str(), v.as_str().unwrap())).collect();
                get(path, &a, &q)
            };
            let want = [*o0, *o1, *o2][state];
            assert_eq!(outcome(st, &v), want, "switch state {state}: {path} → {st} {v}");
        }
    }
    // Approve is the owner's in person, whatever the switches.
    let r = super::approvals::handle("/cli/app/approve", &a, br#"{"request":"w_x"}"#, &HashMap::new());
    assert_eq!(r.unwrap_err().code, "owner_only");
}
