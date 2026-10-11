//! App runtime stage 1 — headless contract tests (`prd-k2-app-runtime-v1`
//! §12.1): the real dispatcher in-process on a temp `$HOME`, no client.
//!
//! S6: the preview flag (404 when off); POST-only (405 on GET); `build`
//! stores a stable Seed and reuses it; `validate` reports errors and a
//! tampered Seed is refused; passports are scoped to their workspace's
//! folders and need "Allow this agent to manage Apps" to build.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use serde_json::json;

#[tokio::test]
async fn s6_build_validate_flag_scope_and_tamper() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "jimws");
    let folder = app_folder(&ws.path.join("jim-app"), "jim", "1.0.0", "v1");

    // Preview off → every runtime route 404s.
    {
        let _off = k2_core::test_env::EnvVar::remove("K2_APPS");
        k2_core::deploy::set_enabled(false);
        let r = post(d.port, "/cli/app/build", OWNER, json!({"folder": folder})).await;
        assert_eq!(r.status, 404, "{}", r.body);
        assert_eq!(r.code(), "apps_disabled");
    }

    // POST-only.
    let r = get(d.port, "/cli/app/build", OWNER, "folder=x").await;
    assert_eq!(r.status, 405, "{}", r.body);

    // Owner build: a Seed with its report; an unchanged rebuild reuses it.
    let r = post(d.port, "/cli/app/build", OWNER, json!({"folder": folder})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let hash = v["seed"]["hash"].as_str().unwrap().to_string();
    assert_eq!(hash.len(), 64);
    assert_eq!(v["seed"]["seedId"], "jim");
    assert_eq!(v["seed"]["signer"], "owner");
    assert_eq!(v["seed"]["report"]["ok"], true);
    assert_eq!(v["reused"], false);
    let r = post(d.port, "/cli/app/build", OWNER, json!({"folder": folder})).await;
    assert_eq!(r.json()["seed"]["hash"], hash.as_str());
    assert_eq!(r.json()["reused"], true);

    // Validate by Seed prefix.
    let r = post(d.port, "/cli/app/validate", OWNER, json!({"seed": &hash[..12]})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["ok"], true);

    // A broken manifest: validate reports, build refuses with the report.
    let bad = ws.path.join("bad-app");
    app_folder(&bad, "jim", "1.0.0", "v1");
    std::fs::write(bad.join("manifest.json"), manifest("jim", "1.0.0").replace("\"caps\"", "\"capz\"")).unwrap();
    let r = post(d.port, "/cli/app/validate", OWNER, json!({"folder": bad})).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["ok"], false);
    assert_eq!(r.json()["report"]["errors"][0]["code"], "manifest_invalid");
    let r = post(d.port, "/cli/app/build", OWNER, json!({"folder": bad})).await;
    assert_eq!(r.status, 422, "{}", r.body);
    assert_eq!(r.code(), "seed_invalid");
    assert_eq!(r.json()["error"]["report"]["ok"], false);

    // Passports: validate is open to the workspace's agent; build needs
    // the switch; folders outside the workspace are refused.
    let pass = passport(&ws);
    let r = post(d.port, "/cli/app/validate", &pass, json!({"folder": folder})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    allow_manage(&ws, false);
    let r = post(d.port, "/cli/app/build", &pass, json!({"folder": folder})).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.code(), "apps_manage_off");
    allow_manage(&ws, true);
    let r = post(d.port, "/cli/app/build", &pass, json!({"folder": folder})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let other = seed_ws(&env, "otherws");
    let outside = app_folder(&other.path.join("x"), "x", "1.0.0", "x");
    let r = post(d.port, "/cli/app/build", &pass, json!({"folder": outside})).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.code(), "folder_outside_workspace");

    // Tamper with the stored Seed: refused on validate (and on deploy).
    let stored = k2_core::deploy::seed::seed_path(&hash).join("tag.txt");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stored, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    std::fs::write(&stored, "evil").unwrap();
    let r = post(d.port, "/cli/app/validate", OWNER, json!({"seed": hash})).await;
    assert_eq!(r.status, 409, "{}", r.body);
    assert_eq!(r.code(), "seed_corrupt");
}

// ── S7: the deploy pipeline ──────────────────────────────────────────

async fn install(port: u16, token: &str, app: &str, ws: &Ws) -> u16 {
    let r = post(port, "/cli/app/install", token, json!({"app": app, "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["frontPort"].as_u64().unwrap() as u16
}

async fn deploy_folder(port: u16, token: &str, app: &str, folder: &std::path::Path) -> serde_json::Value {
    let r = post(port, "/cli/app/deploy", token, json!({"app": app, "folder": folder, "wait": 60})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["done"], true, "{v}");
    v["deployment"].clone()
}

fn body_of(front: u16, path: &str) -> String {
    http_get(front, path).map(|(s, b)| format!("{s} {}", b.trim())).unwrap_or_else(|| "no answer".into())
}

/// Requests through the front port in a loop until stopped; returns the
/// failures (anything that isn't a 200 from some version of the App).
fn hammer(front: u16) -> (std::sync::Arc<std::sync::atomic::AtomicBool>, std::thread::JoinHandle<(usize, Vec<String>)>) {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s2 = stop.clone();
    let h = std::thread::spawn(move || {
        let mut ok = 0usize;
        let mut bad = Vec::new();
        while !s2.load(std::sync::atomic::Ordering::SeqCst) {
            match http_get(front, "/") {
                Some((200, _)) => ok += 1,
                other => bad.push(format!("{other:?}")),
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        (ok, bad)
    });
    (stop, h)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s7_deploy_switch_rollback_stop_start_with_no_dropped_requests() {
    require_python3();
    let env = setup(&[("K2_APPS_WATCH_SECS", "60"), ("K2_HOOK_TOKEN", "daemon-secret-must-not-leak")]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "s7ws");
    let front = install(d.port, OWNER, "s7app", &ws).await;
    assert!(body_of(front, "/").starts_with("503"), "installed, nothing deployed: {}", body_of(front, "/"));

    // First deploy: health gate, switch, live.
    let v1 = app_folder(&env.path().join("v1"), "s7app", "1.0.0", "v1");
    let d1 = deploy_folder(d.port, OWNER, "s7app", &v1).await;
    assert_eq!(d1["result"], "live", "{d1}");
    assert_eq!(d1["actor"], "owner");
    assert_eq!(body_of(front, "/"), "200 v1");

    // The App's environment is clean (S1 applied): no daemon K2_* values.
    let envs = body_of(front, "/env");
    assert!(envs.contains("K2_APP_ID") && envs.contains("PORT"), "{envs}");
    assert!(!envs.contains("K2_HOOK_TOKEN"), "daemon env leaked into the App: {envs}");

    // Second deploy under load: no dropped requests across the switch.
    let (stop, h) = hammer(front);
    let v2 = app_folder(&env.path().join("v2"), "s7app", "2.0.0", "v2");
    let d2 = deploy_folder(d.port, OWNER, "s7app", &v2).await;
    assert_eq!(d2["result"], "live");
    assert_eq!(body_of(front, "/"), "200 v2");

    // Rollback: the previous instance is still warm → instant.
    let r = post(d.port, "/cli/app/rollback", OWNER, json!({"app": "s7app"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["instant"], true);
    assert_eq!(r.json()["deployment"]["kind"], "rollback");
    assert_eq!(body_of(front, "/"), "200 v1");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let (ok, bad) = h.join().unwrap();
    assert!(ok > 5, "the load loop ran ({ok})");
    assert!(bad.is_empty(), "requests dropped across deploy + rollback: {bad:?}");

    // L3 status: who, commit, state.
    let r = get(d.port, "/cli/app/deployment", OWNER, &format!("app=s7app&id={}", d2["id"].as_str().unwrap())).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let st = r.json();
    assert_eq!(st["who"], "owner");
    assert_eq!(st["commit"], d2["toSeed"], "not a git checkout: the commit is the tree hash");
    assert_eq!(st["state"], "rolled_back");

    // A failing health gate keeps the old version live.
    let sick = app_folder(&env.path().join("sick"), "s7app", "3.0.0", "sick-v3");
    let d3 = deploy_folder(d.port, OWNER, "s7app", &sick).await;
    assert_eq!(d3["result"], "failed", "{d3}");
    assert!(d3["error"].as_str().unwrap().starts_with("health_failed"), "{d3}");
    assert_eq!(body_of(front, "/"), "200 v1");

    // Data revisions: a rollback across an applied migration is refused.
    let v4 = app_folder(&env.path().join("v4"), "s7app", "4.0.0", "v4");
    std::fs::write(
        v4.join("manifest.json"),
        manifest("s7app", "4.0.0").replacen("\"caps\"", "\"dataRevision\":2,\"caps\"", 1),
    )
    .unwrap();
    assert_eq!(deploy_folder(d.port, OWNER, "s7app", &v4).await["result"], "live");
    let r = post(d.port, "/cli/app/rollback", OWNER, json!({"app": "s7app", "to": d1["toSeed"]})).await;
    assert_eq!(r.status, 409, "{}", r.body);
    assert_eq!(r.code(), "rollback_crosses_migration");

    // Stop: 503 at the front; start: live again.
    let r = post(d.port, "/cli/app/stop", OWNER, json!({"app": "s7app"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(body_of(front, "/").starts_with("503"));
    let r = post(d.port, "/cli/app/start", OWNER, json!({"app": "s7app", "wait": 60})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["deployment"]["result"], "live");
    assert_eq!(body_of(front, "/"), "200 v4");

    // Publish verbs refuse an App row; publish status shows it running.
    let r = post(d.port, "/cli/publish/stop", OWNER, json!({"name": "s7app", "project": ws.path})).await;
    assert!(r.body.contains("is_an_app"), "{}", r.body);

    let r = post(d.port, "/cli/app/stop", OWNER, json!({"app": "s7app"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s7_watch_window_auto_rollback_then_crash_restart_and_stall_alert() {
    require_python3();
    let env = setup(&[("K2_APPS_WATCH_SECS", "60"), ("K2_APPS_LIVE_PROBE_SECS", "1")]);
    let alerts: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>> = Default::default();
    let a2 = alerts.clone();
    k2_daemon::apps::alerts::set_sink(Some(Box::new(move |ws, text| a2.lock().unwrap().push((ws.to_string(), text.to_string())))));
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "s7bws");
    let front = install(d.port, OWNER, "s7b", &ws).await;
    let v1 = app_folder(&env.path().join("v1"), "s7b", "1.0.0", "v1");
    deploy_folder(d.port, OWNER, "s7b", &v1).await;
    let v2 = app_folder(&env.path().join("v2"), "s7b", "2.0.0", "v2");
    let d2 = deploy_folder(d.port, OWNER, "s7b", &v2).await;
    assert_eq!(body_of(front, "/"), "200 v2");

    // v2 crashes inside its watch window → back to the warm v1.
    let v2_port = d2["port"].as_u64().unwrap() as u16;
    let _ = http_get(v2_port, "/crash");
    wait_until("auto-rollback to v1", 15, || body_of(front, "/") == "200 v1").await;
    let r = get(d.port, "/cli/app/deployment", OWNER, &format!("app=s7b&id={}", d2["id"].as_str().unwrap())).await;
    assert_eq!(r.json()["state"], "rolled_back");
    wait_until("the agent is told", 5, || alerts.lock().unwrap().iter().any(|(w, t)| w == &ws.id && t.contains("rolled it back"))).await;

    // After the watch window: a crash restarts the live Seed at once.
    let r = post(d.port, "/cli/app/stop", OWNER, json!({"app": "s7b"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    {
        let _short = k2_core::test_env::EnvVar::set("K2_APPS_WATCH_SECS", "1");
        let r = post(d.port, "/cli/app/start", OWNER, json!({"app": "s7b", "wait": 60})).await;
        let live_port = r.json()["deployment"]["port"].as_u64().unwrap() as u16;
        tokio::time::sleep(std::time::Duration::from_millis(1800)).await; // the window passes
        let _ = http_get(live_port, "/crash");
        wait_until("crash restart", 20, || {
            let b = k2_daemon::apps::supervisor::live_port("s7b");
            b.is_some_and(|p| p != live_port) && body_of(front, "/") == "200 v1"
        })
        .await;
    }

    // A deploy that never comes up: stalled, and the agent hears (L3).
    {
        let _stall = k2_core::test_env::EnvVar::set("K2_APPS_STALL_SECS", "1");
        let never = env.path().join("never");
        app_folder(&never, "s7b", "9.0.0", "never");
        std::fs::write(
            never.join("manifest.json"),
            manifest("s7b", "9.0.0").replace(r#"["python3","server.py"]"#, r#"["sleep","30"]"#),
        )
        .unwrap();
        let r = post(d.port, "/cli/app/deploy", OWNER, json!({"app": "s7b", "folder": never})).await;
        assert_eq!(r.status, 200, "{}", r.body);
        let id = r.json()["deployment"]["id"].as_str().unwrap().to_string();
        wait_until("stalled", 15, || alerts.lock().unwrap().iter().any(|(_, t)| t.contains(&id) && t.contains("stalled"))).await;
        let r = get(d.port, "/cli/app/deployment", OWNER, &format!("app=s7b&id={id}")).await;
        assert_eq!(r.json()["state"], "stalled", "{}", r.body);
        assert_eq!(body_of(front, "/"), "200 v1", "the live version kept serving");
        // Let the stuck pipeline end before stopping.
        wait_until("pipeline ends", 40, || {
            k2_daemon::apps::supervisor::runtime_json("s7b")["busy"] == false
        })
        .await;
    }
    let r = post(d.port, "/cli/app/stop", OWNER, json!({"app": "s7b"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    k2_daemon::apps::alerts::set_sink(None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s7_passport_rules_and_memory_setting() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let mine = seed_ws(&env, "s7mine");
    let theirs = seed_ws(&env, "s7theirs");
    install(d.port, OWNER, "s7theirs-app", &theirs).await;
    let pass = passport(&mine);

    // Without the switch: refused; with it: own workspace only.
    allow_manage(&mine, false);
    let r = post(d.port, "/cli/app/install", &pass, json!({"app": "s7mine-app"})).await;
    assert_eq!(r.code(), "apps_manage_off", "{}", r.body);
    allow_manage(&mine, true);
    let r = post(d.port, "/cli/app/install", &pass, json!({"app": "s7mine-app"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["app"]["ownerWorkspace"], mine.id);
    assert_eq!(r.json()["app"]["memoryLimitBytes"], serde_json::Value::Null, "no per-App memory limit by default");
    let r = post(d.port, "/cli/app/install", &pass, json!({"app": "x2", "workspace": theirs.id})).await;
    assert_eq!(r.code(), "forbidden");
    for path in ["/cli/app/stop", "/cli/app/start", "/cli/app/rollback", "/cli/app/deploy", "/cli/app/config"] {
        let r = post(d.port, path, &pass, json!({"app": "s7theirs-app", "seed": "x", "memory": "1G"})).await;
        assert_eq!(r.status, 404, "{path}: {}", r.body);
        assert_eq!(r.code(), "app_not_found", "{path}");
    }
    let r = get(d.port, "/cli/app/deployment", &pass, "app=s7theirs-app").await;
    assert_eq!(r.code(), "app_not_found");

    // Box-wide ids: reserved words, taken ids.
    let r = post(d.port, "/cli/app/install", OWNER, json!({"app": "deploy", "workspace": mine.id})).await;
    assert_eq!(r.code(), "app_id_reserved");
    let r = post(d.port, "/cli/app/install", OWNER, json!({"app": "s7theirs-app", "workspace": mine.id})).await;
    assert_eq!(r.code(), "app_exists");

    // The memory setting: set and clear (the user decides; none = no limit).
    let r = post(d.port, "/cli/app/config", &pass, json!({"app": "s7mine-app", "memory": "8G"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["app"]["memoryLimitBytes"], 8i64 << 30);
    let r = post(d.port, "/cli/app/config", &pass, json!({"app": "s7mine-app", "memory": "none"})).await;
    assert_eq!(r.json()["app"]["memoryLimitBytes"], serde_json::Value::Null);

    // Every POST route answers 405 on GET.
    for p in k2_daemon::apps::routes::POST_ROUTES {
        let r = get(d.port, p, OWNER, "app=x").await;
        assert_eq!(r.status, 405, "{p}: {}", r.body);
    }
}
