//! 0.46.4 App-move polish (Agent NSI's list after moving a live Next.js BFF
//! from a `--cmd` service onto the App runtime), end to end through the
//! REAL dispatcher:
//!
//! - #4 warnings: the App's own environment (`K2_PASS_FILE`, `K2_APP_DATA`)
//!   is expected, never a break; App ids: a bare id works on the owner
//!   verbs when it is unique on the box, an ambiguous one lists the
//!   candidates, and `<workspace>/<app id>` works on the runtime verbs.
//!
//! ISOLATION: a temp `$HOME`, the in-memory test DB, alerts to a sink. No
//! real agent CLI, no real `~/.k2`, no running daemon.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::sync::{Arc, Mutex};

use app_fixture::*;
use serde_json::{json, Value};

type Seen = Arc<Mutex<Vec<(String, String)>>>;

fn quiet_alerts() -> Seen {
    let seen: Seen = Arc::default();
    let s = seen.clone();
    k2_daemon::apps::alerts::set_sink(Some(Box::new(move |ws: &str, text: &str| {
        s.lock().unwrap().push((ws.to_string(), text.to_string()));
    })));
    seen
}

/// A published service row (never started): `cmd` for a `--cmd` service,
/// the App sentinel for an App's own row.
fn service(ws: &Ws, name: &str, cmd: &str, folder: &std::path::Path, port: i64) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::insert(&conn, &ws.id, name, cmd, &folder.to_string_lossy(), port, "local", "stopped", "cmd", "")
        .expect("insert service")
        .id
}

/// A runtime App row (never started) with its own service row.
fn app_row(ws: &Ws, app: &str) -> String {
    let svc = service(ws, app, k2_core::published_services::CMD_APP_SENTINEL, &ws.path, 0);
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::deploy::store::insert_app(
        &conn,
        &k2_core::deploy::store::NewApp {
            app_id: app,
            service_id: &svc,
            seed_id: None,
            owner_workspace: &ws.id,
            isolation: "process",
            memory_limit_bytes: None,
            front_port: 0,
            created_by: "owner",
        },
    )
    .expect("insert app");
    svc
}

fn codes(v: &Value) -> Vec<(String, String)> {
    v["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("no warnings: {v}"))
        .iter()
        .map(|w| (w["code"].as_str().unwrap().to_string(), w["severity"].as_str().unwrap().to_string()))
        .collect()
}

/// #4: App-mode code reading the App's own env is expected, not breaking.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn warnings_mark_the_apps_own_env_expected_not_breaking() {
    let env = setup(&[]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "agent-nsi");
    let code = ws.path.join("bff");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::write(
        code.join("server.mjs"),
        "const pass = readFileSync(process.env.K2_PASS_FILE);\nconst data = process.env.K2_APP_DATA;\nconst tok = process.env.K2_OWNER_TOKEN;\n",
    )
    .unwrap();
    let svc = service(&ws, "agents", "node server.mjs", &code, 47_101);
    k2_daemon::vault::publish_env::set(&svc, Some("clean"), None).unwrap();

    let r = get(d.port, "/cli/app/warnings", OWNER, "app=agents").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let got = codes(&v);
    let expected: Vec<&(String, String)> = got.iter().filter(|(c, _)| c == "app_env").collect();
    assert_eq!(expected.len(), 2, "K2_PASS_FILE and K2_APP_DATA each once: {got:?}");
    assert!(expected.iter().all(|(_, s)| s == "expected"), "{got:?}");
    let msgs: Vec<String> = v["warnings"].as_array().unwrap().iter().map(|w| w["message"].as_str().unwrap().to_string()).collect();
    for n in ["K2_PASS_FILE", "K2_APP_DATA"] {
        let m = msgs.iter().find(|m| m.contains(n)).unwrap_or_else(|| panic!("{n} not named: {msgs:?}"));
        assert!(m.contains("Expected inside an App"), "{m}");
        assert!(!got.iter().any(|(c, s)| c == "k2_internal_env" && s == "breaks" && m.contains(n)), "{n} is not a break");
    }
    // The owner token read still breaks; only it counts.
    assert!(got.contains(&("k2_internal_env".to_string(), "breaks".to_string())), "{got:?}");
    assert_eq!(v["breaks"], 1, "only K2_OWNER_TOKEN breaks: {v}");
    assert_eq!(v["readyForCutover"], false);
    k2_daemon::apps::alerts::set_sink(None);
}

/// #4: a bare App id works on the owner verbs when it is unique; two
/// services with one name are refused naming both; `<workspace>/<app id>`
/// works on the runtime verbs and must match the App's workspace.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bare_app_ids_resolve_when_unique_and_ambiguous_ones_list_candidates() {
    let env = setup(&[]);
    let _seen = quiet_alerts();
    let d = k2_daemon::test_harness::start(OWNER).await;
    let nsi = seed_ws(&env, "agent-nsi");
    let other = seed_ws(&env, "other");
    let svc = app_row(&nsi, "agentnsi");

    // The owner's copy-host (dry run) by the bare App id: the App's service.
    let r = post(d.port, "/cli/skin/grants/copy-host", OWNER, json!({"app": "agentnsi", "dryRun": true})).await;
    assert_eq!(r.status, 200, "bare App id: {}", r.body);
    assert_eq!(r.json()["app"], svc.as_str(), "{}", r.body);
    // The long form still works and names the same App.
    let r = post(d.port, "/cli/skin/grants/copy-host", OWNER, json!({"app": "agent-nsi/agentnsi", "dryRun": true})).await;
    assert_eq!((r.status, r.json()["app"].clone()), (200, json!(svc)), "{}", r.body);
    // A bare publish name unique on the box resolves too.
    let jobs = service(&other, "jobsbff", "node s.js", &other.path, 47_102);
    let r = post(d.port, "/cli/skin/grants/copy-host", OWNER, json!({"app": "jobsbff", "dryRun": true})).await;
    assert_eq!((r.status, r.json()["app"].clone()), (200, json!(jobs)), "{}", r.body);
    // The same name in two workspaces: refused, both candidates named.
    service(&nsi, "jobsbff", "node s.js", &nsi.path, 47_103);
    let r = post(d.port, "/cli/skin/grants/copy-host", OWNER, json!({"app": "jobsbff", "dryRun": true})).await;
    assert_eq!(r.status, 400, "{}", r.body);
    for want in ["ambiguous", "agent-nsi/jobsbff", "other/jobsbff"] {
        assert!(r.body.contains(want), "{want} missing: {}", r.body);
    }
    // Unknown stays a plain refusal.
    let r = post(d.port, "/cli/skin/grants/copy-host", OWNER, json!({"app": "nosuch", "dryRun": true})).await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.body.contains("unknown app"), "{}", r.body);

    // Runtime verbs: `<workspace>/<app id>` names the App …
    let r = get(d.port, "/cli/app/inspect", OWNER, "app=agent-nsi%2Fagentnsi").await;
    assert_eq!(r.status, 200, "{}", r.body);
    // … but only in its own workspace.
    let r = get(d.port, "/cli/app/inspect", OWNER, "app=other%2Fagentnsi").await;
    assert_eq!((r.status, r.code()), (404, "app_not_found".to_string()), "{}", r.body);
    let r = get(d.port, "/cli/app/inspect", OWNER, "app=agentnsi").await;
    assert_eq!(r.status, 200, "{}", r.body);
    k2_daemon::apps::alerts::set_sink(None);
}
