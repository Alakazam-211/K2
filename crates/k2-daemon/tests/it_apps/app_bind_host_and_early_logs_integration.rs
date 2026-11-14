//! Where an App listens and what it said before it died, on the process
//! engine (the lane this harness can run; the walled lanes' halves are unit
//! tests in `apps::bind_host_tests` and the engine's `app_logs_tests`).
//!
//! - K2 gives every App `HOST` with `PORT`; an App that binds exactly
//!   `$HOST:$PORT` passes the health gate and answers through its front.
//! - An App that writes to stderr and exits 1 on its first start: the
//!   deploy fails, `k2 app logs` shows that stderr line (from the first
//!   byte, nothing wrapped in a shell), and the App's state names the exit
//!   code and the logs command.

use crate::support::app_fixture;

use app_fixture::*;
use serde_json::json;

/// Stops an App when the test ends, passing or panicking.
struct StopOnDrop(&'static str);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        let app = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::deploy::store::get_app(&conn, self.0).ok().flatten()
        };
        if let Some(app) = app {
            if let Err(f) = k2_daemon::apps::supervisor::stop_app(&app, "test-cleanup") {
                eprintln!("cleanup: stopping {}: {}", self.0, f.message);
            }
        }
        k2_daemon::apps::supervisor::forget(self.0);
    }
}

async fn install(port: u16, app: &str, ws: &Ws) -> u16 {
    let r = post(port, "/cli/app/install", OWNER, json!({"app": app, "workspace": ws.id, "engine": "process"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["frontPort"].as_u64().expect("frontPort") as u16
}

async fn deploy(port: u16, app: &str, folder: &std::path::Path) -> serde_json::Value {
    let r = post(port, "/cli/app/deploy", OWNER, json!({"app": app, "folder": folder, "wait": 60})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["done"], true, "{v}");
    v["deployment"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_that_binds_the_host_it_is_given_goes_live() {
    require_python3();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "bindws");
    let front = install(d.port, "bindapp", &ws).await;
    let _cleanup = StopOnDrop("bindapp");
    let dir = app_folder(&env.path().join("bind"), "bindapp", "1.0.0", "bound");
    // The fixture server, bound to exactly what K2 gave it: a missing HOST
    // is a KeyError at start, so the deploy fails loudly.
    let fixed = SERVER_PY.replace(r#"S(("127.0.0.1", int(os.environ["PORT"])), H)"#, r#"S((os.environ["HOST"], int(os.environ["PORT"])), H)"#);
    assert_ne!(fixed, SERVER_PY, "the fixture's bind line changed: update this test");
    std::fs::write(dir.join("server.py"), fixed).unwrap();
    let dep = deploy(d.port, "bindapp", &dir).await;
    assert_eq!(dep["result"], "live", "{dep}");
    assert_eq!(http_get(front, "/").map(|(s, b)| (s, b.trim().to_string())), Some((200, "bound".to_string())), "the front reaches it");
    let host = http_get(front, "/envval/HOST").expect("the App answers /envval/HOST");
    assert_eq!(host, (200, "127.0.0.1".to_string()), "the process engine runs on the host's network: loopback");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_early_exit_on_the_first_start_keeps_its_stderr_and_exit_code() {
    require_python3();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "earlyws");
    install(d.port, "earlyapp", &ws).await;
    let _cleanup = StopOnDrop("earlyapp");
    let dir = app_folder(&env.path().join("early"), "earlyapp", "1.0.0", "early");
    std::fs::write(
        dir.join("server.py"),
        "import sys\nprint('starting up', flush=True)\nprint('fatal: open secrets/db.json: no such file or directory', file=sys.stderr)\nsys.exit(1)\n",
    )
    .unwrap();
    let dep = deploy(d.port, "earlyapp", &dir).await;
    assert_eq!(dep["result"], "failed", "{dep}");
    let dep_id = dep["id"].as_str().expect("deployment id").to_string();

    // k2 app logs: nothing is live, so the newest deployment, this one.
    let r = get(d.port, "/cli/app/logs", OWNER, "app=earlyapp").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["deployment"], dep_id.as_str(), "{}", r.body);
    let framed = r.json()["framed"].as_str().expect("framed").to_string();
    assert!(framed.contains("fatal: open secrets/db.json: no such file or directory"), "stderr of the failed start: {framed}");
    assert!(framed.contains("starting up"), "stdout too: {framed}");

    // The App's state: failed, the exit code, and where the output is.
    let r = get(d.port, "/cli/app/list", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let apps = r.json()["apps"].as_array().expect("apps").clone();
    let a = apps.iter().find(|a| a["appId"] == "earlyapp").unwrap_or_else(|| panic!("earlyapp listed: {apps:?}"));
    assert_eq!(a["state"], "failed", "{a}");
    let why = a["stateReason"].as_str().unwrap_or_else(|| panic!("a reason: {a}"));
    assert!(why.contains("exited with code 1"), "{why}");
    assert!(why.contains(&format!("k2 app logs earlyapp --deployment {dep_id}")), "{why}");
}
