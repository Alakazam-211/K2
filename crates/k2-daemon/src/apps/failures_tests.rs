//! App failure wakes and history (0.46.3): each wake fires once, is rate
//! limited, goes to the App's own workspace and no other; every failure is
//! recorded; `k2 app errors` filters it and refuses another workspace's
//! agent.
//!
//! The alerts sink is process-global and other test modules replace it, so
//! these tests assert on what [`crash`] / [`deploy_failed`] /
//! [`build_failed`] return (the workspace and text they handed to
//! `alerts::notify`), and install a sink only so no test wakes a real agent.

use std::collections::HashMap;

use k2_core::app_audit as hist;
use k2_core::deploy::store;
use rusqlite::params;
use serde_json::Value as J;

use super::super::routes::Who;
use super::*;

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    ws_a: String,
    ws_b: String,
}

/// Two workspaces; `app` belongs to A, `<app>-b` to B. App ids are unique
/// per test: the "said" state is process-wide.
fn fx(app: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    let (ws_a, ws_b) = (format!("ws-a-{app}"), format!("ws-b-{app}"));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for ws in [&ws_a, &ws_b] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![ws, format!("/tmp/k2-failures-test/{ws}")])
                .unwrap();
        }
        for (id, ws) in [(app.to_string(), &ws_a), (format!("{app}-b"), &ws_b)] {
            store::insert_app(
                &conn,
                &store::NewApp {
                    app_id: &id,
                    service_id: &format!("svc-{id}"),
                    seed_id: None,
                    owner_workspace: ws,
                    isolation: "process",
                    memory_limit_bytes: None,
                    front_port: 0,
                    created_by: "owner",
                },
            )
            .unwrap();
        }
    }
    super::super::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    Fx { _db: db, _home: home, ws_a, ws_b }
}

fn history(app: &str) -> Vec<hist::Failure> {
    let c = hist::open(&hist::db_path()).unwrap();
    hist::list_failures(&c, &hist::FailureQuery { app_id: app, limit: 100, ..Default::default() }).unwrap().into_iter().map(|r| r.failure).collect()
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn get(path: &str, who: &Who, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = super::super::routes::handle(path, who.clone(), b"", &p);
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn crash_of<'a>(app: &'a str, dep: &'a str, n: u32, restart_in: Option<i64>) -> Crash<'a> {
    Crash { app_id: app, deployment: dep, exited: true, exit_code: Some(1), signal: None, restart_count: n, restart_in_secs: restart_in, what: None }
}

#[test]
fn the_first_crash_wakes_the_owning_workspace_once_per_window() {
    let f = fx("crashwake");
    let app = "crashwake";
    let said = crash(&crash_of(app, "d-0000000000a1", 1, Some(0))).expect("the first crash wakes");
    assert_eq!(said.workspace, f.ws_a, "the App's own workspace");
    assert_ne!(said.workspace, f.ws_b);
    for want in ["[k2 apps] App crashwake crashed", "exited with code 1", "d-0000000000a1", "restarts it now", "restart 1 of 6", "k2 app logs crashwake --deployment d-0000000000a1 --tail 20", "k2 app errors crashwake --kind crash"] {
        assert!(said.text.contains(want), "{want:?} missing from {}", said.text);
    }

    // More crashes in the window: recorded, no wake (they fold into the
    // crash-loop alert), including the breaker trip itself.
    let mut sig = crash_of(app, "d-0000000000a2", 2, Some(5));
    sig.exit_code = None;
    sig.signal = Some(9);
    assert_eq!(crash(&sig), None, "a second crash within the hour doesn't wake");
    assert_eq!(crash(&crash_of(app, "d-0000000000a3", 6, None)), None, "the breaker's own alert speaks");
    // Another App's crash is its own window, and goes to its own workspace.
    let other = crash(&crash_of("crashwake-b", "d-0000000000b1", 1, Some(0))).expect("another App's first crash wakes");
    assert_eq!(other.workspace, f.ws_b);

    let h = history(app);
    assert_eq!(h.len(), 3, "every crash is recorded: {h:?}");
    assert_eq!(h.iter().map(|x| x.woke).collect::<Vec<_>>(), vec![false, false, true], "newest first; only the first woke");
    assert!(h.iter().all(|x| x.kind == hist::FAILURE_CRASH && !x.rolled_back));
    assert_eq!((h[1].exit_code, h[1].signal, h[1].restart_count), (None, Some(9), 2));
    assert!(h[1].reason.contains("SIGKILL") && h[1].reason.contains("restart in 5s"), "{}", h[1].reason);
    assert!(h[0].reason.contains("crash-looping"), "{}", h[0].reason);
    assert_eq!(h[2].deployment.as_deref(), Some("d-0000000000a1"));
    assert!(history("crashwake-b").iter().all(|x| x.app_id == "crashwake-b"));

    // The owner restarts it after the loop: the next crash wakes again.
    super::super::supervisor::reset_backoff(app);
    let again = crash(&crash_of(app, "d-0000000000a4", 1, Some(0))).expect("after a reset the next crash wakes");
    assert_eq!(again.workspace, f.ws_a);
}

#[test]
fn a_health_check_crash_and_a_failed_restart_say_so() {
    let _f = fx("crashwhat");
    let mut c = crash_of("crashwhat", "d-0000000000c1", 1, Some(30));
    c.exited = false;
    c.exit_code = None;
    let said = crash(&c).unwrap();
    assert!(said.text.contains("stopped answering its health checks") && said.text.contains("restarts it in 30s"), "{}", said.text);
    let restart = Crash { what: Some("failed the health gate when K2 restarted it"), ..crash_of("crashwhat", "d-0000000000c2", 2, Some(5)) };
    assert_eq!(crash(&restart), None);
    let h = history("crashwhat");
    assert!(h[0].reason.starts_with("failed the health gate when K2 restarted it"), "{}", h[0].reason);
}

#[test]
fn a_failed_health_gate_wakes_once_per_five_minutes_and_is_recorded() {
    let f = fx("gatewake");
    let app = "gatewake";
    let d = DeployFail {
        app_id: app,
        deployment: "d-0000000000d1",
        deploy_kind: "deploy",
        actor: "owner",
        code: k2_core::deploy::codes::HEALTH_FAILED,
        reason: "GET /healthz never answered 2xx within 120s (last probe: HTTP 500)",
        exit_code: None,
        signal: None,
    };
    let said = deploy_failed(&d).expect("a failed gate wakes");
    assert_eq!(said.workspace, f.ws_a);
    for want in ["deploy d-0000000000d1 by owner failed its health gate", "GET /healthz never answered 2xx within 120s", "nothing was rolled back", "nothing is live now", "k2 app errors gatewake --kind health"] {
        assert!(said.text.contains(want), "{want:?} missing from {}", said.text);
    }
    // A second failed gate within 5 minutes: recorded, no wake.
    assert_eq!(deploy_failed(&DeployFail { deployment: "d-0000000000d2", ..d.clone() }), None);
    // A deploy that couldn't start is its own kind.
    let start = DeployFail { deployment: "d-0000000000d3", code: k2_core::deploy::codes::ENGINE_UNAVAILABLE, reason: "no free port", exit_code: Some(3), ..d.clone() };
    let s2 = deploy_failed(&start).expect("a deploy that couldn't start wakes");
    assert!(s2.text.contains("couldn't start") && s2.text.contains("exited with code 3"), "{}", s2.text);

    let h = history(app);
    assert_eq!(h.len(), 3);
    assert_eq!((h[0].kind.as_str(), h[0].woke, h[0].exit_code), (hist::FAILURE_DEPLOY, true, Some(3)));
    assert_eq!((h[1].kind.as_str(), h[1].woke), (hist::FAILURE_HEALTH, false));
    assert_eq!((h[2].kind.as_str(), h[2].woke, h[2].rolled_back), (hist::FAILURE_HEALTH, true, false));
    assert!(h[2].reason.starts_with("health_failed: GET /healthz"), "{}", h[2].reason);
}

#[test]
fn a_rollback_in_the_watch_window_is_recorded_as_rolled_back() {
    let _f = fx("rbrec");
    rolled_back("rbrec", "d-0000000000e1", false, None, None);
    rolled_back("rbrec", "d-0000000000e2", true, None, Some(11));
    let h = history("rbrec");
    assert_eq!(h.len(), 2);
    assert!(h.iter().all(|x| x.rolled_back));
    assert_eq!((h[0].kind.as_str(), h[0].signal), (hist::FAILURE_CRASH, Some(11)));
    assert!(h[0].reason.contains("SIGSEGV"), "{}", h[0].reason);
    assert_eq!(h[1].kind, hist::FAILURE_HEALTH);
}

#[test]
fn a_failed_build_wakes_with_its_step_and_log_and_no_output() {
    let f = fx("buildwake");
    let app = "buildwake";
    let log = super::super::wall_build::save_build_log(app, Some("npm ERR! missing script: build\nAPI_KEY=sk-should-stay-in-the-log\n")).unwrap();
    assert!(super::super::wall_build::is_build_log_id(&log), "{log}");
    let b = BuildFail { app_id: app, actor: "agent:buildwake", code: "build_failed", reason: "the build exited 1 (its output is below)", step: Some("npm run build"), log: Some(&log) };
    let said = build_failed(&b).expect("a failed build wakes");
    assert_eq!(said.workspace, f.ws_a);
    for want in ["a build for a deploy by agent:buildwake failed", "build_failed at `npm run build`", &format!("k2 app logs buildwake --deployment {log}"), "k2 app errors buildwake --kind build", "Nothing was deployed"] {
        assert!(said.text.contains(want), "{want:?} missing from {}", said.text);
    }
    assert!(!said.text.contains("npm ERR") && !said.text.contains("sk-should"), "a wake never carries build output: {}", said.text);
    assert_eq!(build_failed(&BuildFail { log: None, ..b.clone() }), None, "rate-limited");
    let h = history(app);
    assert_eq!(h.len(), 2);
    assert_eq!((h[1].kind.as_str(), h[1].deployment.as_deref(), h[1].woke), (hist::FAILURE_BUILD, Some(log.as_str()), true));
    assert!(!h[1].reason.contains("sk-should"));

    // The build log: the owning agent reads it (framed); another
    // workspace's agent can't see the App.
    let (st, v) = get("/cli/app/logs", &agent(&f.ws_a), &[("app", app), ("deployment", &log)]);
    assert_eq!(st, 200, "{v}");
    let framed = v["framed"].as_str().unwrap();
    assert!(framed.contains("kind=\"build\"") && framed.contains("npm ERR! missing script"), "{framed}");
    let (st, v) = get("/cli/app/logs", &agent(&f.ws_b), &[("app", app), ("deployment", &log)]);
    assert_eq!((st, v["error"]["code"].as_str()), (404, Some("app_not_found")), "{v}");
    let (st, _) = get("/cli/app/logs", &agent(&f.ws_a), &[("app", app), ("deployment", "b-000000000000")]);
    assert_eq!(st, 404, "an unknown build log");
    // The reserved build slot (k2 app build without an App) keeps none.
    assert_eq!(super::super::wall_build::save_build_log(super::super::wall_build::BUILD_SLOT, Some("x")), None);
    assert_eq!(super::super::wall_build::save_build_log(app, Some("  \n")), None);
}

#[test]
fn errors_lists_the_history_filters_it_and_refuses_another_workspace() {
    let f = fx("errroute");
    let app = "errroute";
    crash(&crash_of(app, "d-0000000000f1", 1, Some(0)));
    deploy_failed(&DeployFail {
        app_id: app,
        deployment: "d-0000000000f2",
        deploy_kind: "deploy",
        actor: "owner",
        code: k2_core::deploy::codes::HEALTH_FAILED,
        reason: "GET / never answered 2xx within 5s",
        ..Default::default()
    });
    build_failed(&BuildFail { app_id: app, actor: "owner", code: "lockfile_required", reason: "no lockfile", ..Default::default() });
    crash(&crash_of("errroute-b", "d-0000000000f9", 1, Some(0)));
    // An old failure, outside an hour.
    {
        let c = hist::open(&hist::db_path()).unwrap();
        let old = hist::Failure {
            at_ms: (k2_core::deploy::now() - 7200) * 1000,
            app_id: app.into(),
            kind: hist::FAILURE_CRASH.into(),
            reason: "old".into(),
            ..Default::default()
        };
        hist::record_failure(&c, &old).unwrap();
    }
    let me = agent(&f.ws_a);

    let (st, v) = get("/cli/app/errors", &me, &[("app", app)]);
    assert_eq!(st, 200, "{v}");
    let kinds: Vec<&str> = v["failures"].as_array().unwrap().iter().map(|x| x["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, vec!["build", "health", "crash"], "newest first, the last hour by default, only this App");
    assert_eq!(v["failures"][2]["exitCode"], 1);
    assert_eq!(v["failures"][2]["deployment"], "d-0000000000f1");
    assert_eq!(v["failures"][2]["restartCount"], 1);
    assert_eq!(v["failures"][1]["rolledBack"], false);
    assert!(v["events"].is_array() && v["framed"].is_string(), "the deploy events and log lines stay: {v}");
    assert_eq!(v["sinceSecs"], 3600);

    let (_, v) = get("/cli/app/errors", &me, &[("app", app), ("kind", "crash"), ("since", "3h")]);
    let reasons: Vec<&str> = v["failures"].as_array().unwrap().iter().map(|x| x["reason"].as_str().unwrap()).collect();
    assert_eq!(reasons.len(), 2, "{v}");
    assert_eq!(reasons[1], "old");
    assert_eq!(v["kind"], "crash");
    assert_eq!(v["sinceSecs"], 10800);
    let (_, v) = get("/cli/app/errors", &me, &[("app", app), ("kind", "build"), ("since", "600")]);
    assert_eq!(v["failures"].as_array().unwrap().len(), 1);
    assert!(v["failures"][0]["reason"].as_str().unwrap().starts_with("lockfile_required"), "{v}");

    let (st, v) = get("/cli/app/errors", &me, &[("app", app), ("kind", "oops")]);
    assert_eq!((st, v["error"]["code"].as_str()), (400, Some("bad_request")), "{v}");
    let (st, _) = get("/cli/app/errors", &me, &[("app", app), ("since", "soon")]);
    assert_eq!(st, 400);

    // Another workspace's agent can't see the App, and its own App's
    // history holds only its own failures.
    let (st, v) = get("/cli/app/errors", &agent(&f.ws_b), &[("app", app)]);
    assert_eq!((st, v["error"]["code"].as_str()), (404, Some("app_not_found")), "{v}");
    let (st, v) = get("/cli/app/errors", &agent(&f.ws_b), &[("app", "errroute-b")]);
    assert_eq!(st, 200, "{v}");
    let theirs = v["failures"].as_array().unwrap();
    assert_eq!(theirs.len(), 1);
    assert_eq!(theirs[0]["appId"], "errroute-b");
    // The owner sees any App.
    let (st, v) = get("/cli/app/errors", &Who::Owner, &[("app", app), ("kind", "health")]);
    assert_eq!((st, v["failures"].as_array().unwrap().len()), (200, 1));
}

#[test]
fn errors_since_takes_seconds_or_a_duration() {
    use super::super::routes::errors_since_secs;
    assert_eq!(errors_since_secs(None).unwrap(), 3600);
    assert_eq!(errors_since_secs(Some("")).unwrap(), 3600);
    assert_eq!(errors_since_secs(Some("300")).unwrap(), 300);
    assert_eq!(errors_since_secs(Some("30m")).unwrap(), 1800);
    assert_eq!(errors_since_secs(Some("2d")).unwrap(), 172_800);
    assert_eq!(errors_since_secs(Some("45s")).unwrap(), 45);
    assert_eq!(errors_since_secs(Some("0")).unwrap(), 1);
    assert!(errors_since_secs(Some("1w")).is_err());
    assert!(errors_since_secs(Some("-5m")).is_err());
}
