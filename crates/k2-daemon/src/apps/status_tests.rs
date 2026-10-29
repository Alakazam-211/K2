//! `k2 app status` (TC-15) in process: every part present (`schema: 1`),
//! the verdict from fixtures (nothing deployed, a deploy in progress, crash
//! looping, a failed recorded check, a held request), the next lines, and
//! another workspace's agent refused.

use std::collections::HashMap;

use k2_core::deploy::store;
use rusqlite::params;
use serde_json::{json, Value as J};

use super::super::routes::Who;

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    ws_a: String,
    ws_b: String,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    let (ws_a, ws_b) = (format!("st-a-{tag}"), format!("st-b-{tag}"));
    let h = k2_core::db::shared();
    let conn = h.lock();
    for ws in [&ws_a, &ws_b] {
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![ws, format!("/tmp/k2-status-test/{ws}")]).expect("project");
    }
    let svc = k2_core::published_services::insert(
        &conn,
        &ws_a,
        "jobs",
        k2_core::published_services::CMD_APP_SENTINEL,
        "/tmp/k2-status-test/apps/jobs",
        0,
        k2_core::published_services::EXPOSE_LOCAL,
        k2_core::published_services::DESIRED_STOPPED,
        k2_core::published_services::KIND_CMD,
        "",
    )
    .expect("service");
    store::insert_app(
        &conn,
        &store::NewApp {
            app_id: "jobs",
            service_id: &svc.id,
            seed_id: None,
            owner_workspace: &ws_a,
            isolation: "process",
            memory_limit_bytes: Some(256 << 20),
            front_port: 0,
            created_by: "owner",
        },
    )
    .expect("app");
    drop(conn);
    super::super::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    Fx { _db: db, _home: home, ws_a, ws_b }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn status(who: &Who, q: &[(&str, &str)]) -> (u16, J) {
    let mut p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    p.entry("app".into()).or_insert_with(|| "jobs".into());
    let r = super::super::routes::handle(super::ROUTE, who.clone(), b"", &p);
    let n: u16 = r.status.split_whitespace().next().expect("status").parse().expect("number");
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body)))
}

fn next_runs(v: &J) -> Vec<String> {
    v["next"].as_array().expect("next").iter().map(|n| n["run"].as_str().expect("run").to_string()).collect()
}

const PARTS: [&str; 10] = ["deploy", "health", "checks", "errors", "resources", "door", "hosts", "phone", "updates", "next"];

#[test]
fn every_part_is_there_and_a_fresh_app_is_yellow_with_the_deploy_command() {
    let _f = fx("fresh");
    let (st, v) = status(&Who::Owner, &[]);
    assert_eq!(st, 200, "{v}");
    assert_eq!((v["schema"].as_u64(), v["app"].as_str()), (Some(1), Some("jobs")), "{v}");
    for p in PARTS {
        assert!(v.get(p).is_some(), "part {p}: {v}");
    }
    assert_eq!(v["verdict"], "yellow", "{v}");
    assert!(next_runs(&v).contains(&"k2 app deploy jobs <folder>".to_string()), "{v}");
    assert_eq!(v["checks"]["declared"], 0, "{v}");
    assert_eq!(v["phone"]["push"]["keys"], json!({ "apns": false, "fcm": false }), "yes/no only: {v}");
    assert_eq!(v["updates"]["feed"], "off");
}

#[test]
fn another_workspaces_agent_is_refused_and_its_own_reads_it() {
    let f = fx("who");
    let (st, v) = status(&agent(&f.ws_b), &[]);
    assert!(st == 403 || st == 404, "{st}: {v}");
    let code = v["error"]["code"].as_str().expect("code");
    assert!(k2_core::refusals::lookup(code).is_some(), "{code} is in the registry");
    let (st, v) = status(&agent(&f.ws_a), &[]);
    assert_eq!(st, 200, "{v}");
    let (st, v) = status(&Who::Wrong, &[]);
    assert_eq!(st, 403, "an App pass: {v}");
    let (st, _) = status(&Who::Owner, &[("app", "nope")]);
    assert_eq!(st, 404);
}

#[test]
fn a_deploy_in_progress_is_yellow_and_crash_looping_is_red() {
    let _f = fx("dep");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        store::insert_deployment(&conn, &store::NewDeployment { app_id: "jobs", kind: "deploy", from_seed: None, to_seed: Some("s1"), commit_ref: None, actor: "owner", reason: None })
            .expect("deployment");
    }
    let (_, v) = status(&Who::Owner, &[]);
    assert_eq!(v["verdict"], "yellow", "{v}");
    assert_eq!(v["deploy"]["last"]["result"], "pending");
    assert!(next_runs(&v).iter().any(|r| r == "k2 app deployment jobs"), "{v}");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        store::set_state(&conn, "jobs", store::STATE_CRASHLOOPING, Some("exited 6 times")).expect("state");
    }
    let (_, v) = status(&Who::Owner, &[]);
    assert_eq!(v["verdict"], "red", "{v}");
    assert!(next_runs(&v).contains(&"k2 app errors jobs --kind crash".to_string()), "{v}");
}

#[test]
fn a_failed_recorded_check_is_red_and_no_app_output_shows() {
    let _f = fx("chk");
    let seed = "c0ffee".repeat(10);
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let manifest = json!({ "id": "jobs", "host": { "mode": "cmd", "checks": [{ "name": "health", "path": "/api/health", "expect": 200 }] } });
        store::upsert_seed(
            &conn,
            &store::SeedRow {
                hash: seed.clone(),
                seed_id: "jobs".into(),
                version: Some("1.0.0".into()),
                host_mode: "cmd".into(),
                manifest,
                created_with: J::Null,
                report: J::Null,
                signer: "owner".into(),
                origin: "local".into(),
                built_for: None,
                built_from: None,
                source_ref: Some("abc123".into()),
                data_revision: 0,
                size_bytes: 10,
                created_at: 1,
                last_used_at: 1,
            },
        )
        .expect("seed");
        let d = store::insert_deployment(&conn, &store::NewDeployment { app_id: "jobs", kind: "deploy", from_seed: None, to_seed: Some(&seed), commit_ref: Some("abc123"), actor: "owner", reason: None })
            .expect("deployment");
        store::set_result(&conn, &d, store::RESULT_LIVE, None).expect("live");
        store::set_live(&conn, "jobs", Some(&seed), Some(&d)).expect("set live");
        store::set_state(&conn, "jobs", store::STATE_RUNNING, None).expect("running");
        drop(conn);
        let o = super::super::checks::Outcome {
            name: "health".into(),
            path: "/api/health".into(),
            expect: 200,
            as_test_guest: false,
            status: Some(500),
            header: None,
            body: None,
            ok: false,
            why: Some("status"),
            ms: 4,
        };
        super::super::checks::record("jobs", &d, &[o]);
    }
    let (st, v) = status(&Who::Owner, &[("noChecks", "1")]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["checks"]["source"], "recorded", "{v}");
    assert_eq!(v["checks"]["failed"], 1, "{v}");
    assert_eq!(v["verdict"], "red", "{v}");
    assert!(next_runs(&v).contains(&"k2 app status jobs --json".to_string()), "{v}");
    assert_eq!(v["deploy"]["seed"]["version"], "1.0.0");
}

#[test]
fn the_cli_exit_follows_the_verdict() {
    let cli = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../cli/k2")).expect("cli/k2");
    let f = cli.split("\ncmd_app_status() {").nth(1).expect("cmd_app_status");
    let f = &f[..f.find("\n}\n").expect("end")];
    for (verdict, exit) in [("green", "return 0"), ("yellow", "exit 2"), ("red", "exit 1")] {
        assert!(f.contains(&format!("{verdict}) {exit} ;;")), "{verdict} → {exit}");
    }
}
