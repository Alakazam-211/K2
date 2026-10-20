//! 0.46.3 runtime fixes for Sew's Hearth and Meet templates, end to end
//! through the REAL dispatcher on the process engine (a temp `$HOME`, a
//! vault on a file key under it; never the real `~/.k2` or keychain):
//!
//! - App identity: `K2_APP_NAME` = the App id (`k2 app …`, `k2 hostchat
//!   meet bind`), `K2_APP_ID` = its published service id (S24 logins),
//!   `K2_APP_ORIGIN`/`K2_APP_ORIGINS` follow a hostname bound after
//!   install, on the next restart.
//! - The seal key (K-M3): `$K2_SECRETS_DIR/K2_APP_SEAL_KEY`, stable across
//!   deploy, restart and rollback, only for its own App, in no log or
//!   route body; rotate (owner / switch 2, `yes`) changes it at the next
//!   start.
//! - `requires.sew` / `requires.voice`: install and deploy refuse with the
//!   owner's fix when this box's Sew doesn't meet them.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use serde_json::json;

const SEAL: &str = "K2_APP_SEAL_KEY";

async fn install(port: u16, token: &str, app: &str, ws: &Ws) -> u16 {
    // 0.46.3: new Apps default to a container; this harness has no
    // container engine, so the owner installs on the process engine.
    let r = post(port, "/cli/app/install", token, json!({"app": app, "workspace": ws.id, "engine": "process"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["frontPort"].as_u64().unwrap() as u16
}

async fn deploy_folder(port: u16, app: &str, folder: &std::path::Path) -> serde_json::Value {
    let r = post(port, "/cli/app/deploy", OWNER, json!({"app": app, "folder": folder, "wait": 60})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["deployment"]["result"], "live", "{v}");
    v["deployment"].clone()
}

async fn restart(port: u16, app: &str) {
    let r = post(port, "/cli/app/restart", OWNER, json!({"app": app, "wait": 60})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["deployment"]["result"], "live", "{}", r.body);
}

/// `(status, body)` from the App; panics when nothing answers.
fn app_get(front: u16, path: &str) -> (u16, String) {
    http_get(front, path).unwrap_or_else(|| panic!("the App on {front} didn't answer {path}"))
}

fn env_val(front: u16, name: &str) -> Option<String> {
    match app_get(front, &format!("/envval/{name}")) {
        (200, b) => Some(b),
        (404, _) => None,
        other => panic!("/envval/{name}: {other:?}"),
    }
}

fn seal_of(front: u16) -> String {
    let (st, body) = app_get(front, &format!("/secret/{SEAL}"));
    assert_eq!(st, 200, "the App has no $K2_SECRETS_DIR/{SEAL}: {body}");
    assert_eq!(body.len(), 43, "32 bytes, base64url, no padding or newline: {body:?}");
    body
}

fn service_id(app: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::deploy::store::get_app(&conn, app).unwrap().expect("the App row").service_id
}

/// Every file under `dir` that holds `needle` (the temp `$HOME`: daemon
/// logs, App logs, audit, records). The vault database holds ciphertext.
fn files_holding(dir: &std::path::Path, needle: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file() {
                if let Ok(bytes) = std::fs::read(&p) {
                    if bytes.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                        out.push(p.display().to_string());
                    }
                }
            }
        }
    }
    out
}

fn manifest_with_requires(id: &str, requires: &str) -> String {
    manifest(id, "1.0.0").replacen("\"caps\"", &format!("\"requires\":{requires},\"caps\""), 1)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identity_origin_and_seal_key_reach_the_app_and_stay_stable() {
    require_python3();
    let env = setup(&[("K2_APPS_WATCH_SECS", "60")]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    k2_daemon::vault::get().expect("the vault opened under the temp home");
    let ws = seed_ws(&env, "hearthws");
    let front = install(d.port, OWNER, "hearth", &ws).await;
    let v1 = app_folder(&env.path().join("v1"), "hearth", "1.0.0", "v1");
    deploy_folder(d.port, "hearth", &v1).await;
    let svc = service_id("hearth");

    // Identity: the App id and the S24 service id, both present.
    assert_eq!(env_val(front, "K2_APP_NAME").as_deref(), Some("hearth"), "K2_APP_NAME = the App id");
    assert_eq!(env_val(front, "K2_APP_ID").as_deref(), Some(svc.as_str()), "K2_APP_ID = the published service id (S24)");
    assert!(env_val(front, "K2_APP_DEPLOYMENT").is_some_and(|d| !d.is_empty()), "K2_APP_DEPLOYMENT is set");
    // Local-only, no domain: no origin (unset, never empty or "*").
    assert_eq!(env_val(front, "K2_APP_ORIGIN"), None);
    assert_eq!(env_val(front, "K2_APP_ORIGINS"), None);

    // The seal key: made at the first start, a file, never an env var.
    let k1 = seal_of(front);
    assert_eq!(env_val(front, SEAL), None, "the seal key is a file, not a variable");
    let r = get(d.port, "/cli/app/seal", OWNER, "app=hearth").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["seal"]["present"], true, "{}", r.body);
    assert_eq!(r.json()["seal"]["granted"], true, "{}", r.body);
    assert!(!r.body.contains(&k1), "the status never carries the key");

    // Bind a hostname to the App after install: the running process keeps
    // what it started with; the next restart has it, and the key stays.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::upsert_binding(&conn, "example.com", None, false).unwrap();
        k2_core::domains::upsert_name(&conn, "hearth.example.com", "example.com", "app:hearth").unwrap();
    }
    assert_eq!(env_val(front, "K2_APP_ORIGIN"), None, "a running process keeps its start values");
    restart(d.port, "hearth").await;
    assert_eq!(env_val(front, "K2_APP_ORIGIN").as_deref(), Some("https://hearth.example.com"));
    assert_eq!(env_val(front, "K2_APP_ORIGINS").as_deref(), Some("https://hearth.example.com"));
    assert_eq!(seal_of(front), k1, "restart: same key");

    // A new Seed: same key. A rollback: same key.
    let v2 = app_folder(&env.path().join("v2"), "hearth", "2.0.0", "v2");
    deploy_folder(d.port, "hearth", &v2).await;
    assert_eq!(app_get(front, "/").1.trim(), "v2");
    assert_eq!(seal_of(front), k1, "deploy: same key");
    let r = post(d.port, "/cli/app/rollback", OWNER, json!({"app": "hearth", "wait": 60})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(app_get(front, "/").1.trim(), "v1");
    assert_eq!(seal_of(front), k1, "rollback: same key");
    // The hostname moves to another name: the next restart follows it.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::remove_name(&conn, "hearth.example.com").unwrap();
        k2_core::domains::upsert_name(&conn, "chat.example.com", "example.com", "app:hearth").unwrap();
    }
    restart(d.port, "hearth").await;
    assert_eq!(env_val(front, "K2_APP_ORIGIN").as_deref(), Some("https://chat.example.com"));
    assert_eq!(seal_of(front), k1);

    // Another App: its own key; it never sees hearth's.
    let other_front = install(d.port, OWNER, "meet-x", &ws).await;
    let o1 = app_folder(&env.path().join("o1"), "meet-x", "1.0.0", "o1");
    deploy_folder(d.port, "meet-x", &o1).await;
    let ko = seal_of(other_front);
    assert_ne!(ko, k1, "one key per App");
    assert_eq!(env_val(other_front, "K2_APP_NAME").as_deref(), Some("meet-x"));
    assert_eq!(env_val(other_front, "K2_APP_ORIGIN"), None, "hearth's hostname isn't meet-x's");

    // No log, audit, record or route body holds a key.
    for k in [&k1, &ko] {
        let hits = files_holding(env.path(), k);
        assert!(hits.is_empty(), "a seal key leaked into: {hits:?}");
        for (path, q) in [
            ("/cli/app/logs", "app=hearth&tail=500"),
            ("/cli/app/inspect", "app=hearth"),
            ("/cli/app/history", "app=hearth"),
            ("/cli/app/seal", "app=meet-x"),
        ] {
            let r = get(d.port, path, OWNER, q).await;
            assert_eq!(r.status, 200, "{path}: {}", r.body);
            assert!(!r.body.contains(k.as_str()), "{path} carried a seal key");
        }
    }

    // Rotate: only the owner or switch 2, only with yes, and only at the
    // next start.
    allow_manage(&ws, true);
    let agent = passport(&ws);
    let r = post(d.port, "/cli/app/seal/rotate", &agent, json!({"app": "hearth", "yes": true})).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "switch 1 alone can't: {}", r.body);
    let r = post(d.port, "/cli/app/seal/rotate", OWNER, json!({"app": "hearth"})).await;
    assert_eq!((r.status, r.code()), (409, "signout_confirm_required".to_string()), "{}", r.body);
    assert!(r.body.contains("signs EVERYONE out"), "{}", r.body);
    let r = get(d.port, "/cli/app/seal/rotate", OWNER, "app=hearth").await;
    assert_eq!(r.status, 405, "POST only: {}", r.body);
    assert_eq!(seal_of(front), k1, "nothing changed without yes");
    k2_core::workspace::settings::set_agents_can_manage_apps_full(&ws.id, true).unwrap();
    let r = post(d.port, "/cli/app/seal/rotate", &agent, json!({"app": "hearth", "yes": true})).await;
    assert_eq!(r.status, 200, "switch 2 rotates its own App's key: {}", r.body);
    assert_eq!(r.json()["version"], 2, "{}", r.body);
    assert!(r.json()["warning"].as_str().unwrap().contains("signs EVERYONE out"));
    assert_eq!(seal_of(front), k1, "the running process keeps its key until it restarts");
    restart(d.port, "hearth").await;
    let k2 = seal_of(front);
    assert_ne!(k2, k1, "rotate changed it");
    assert_eq!(seal_of(other_front), ko, "another App's key is untouched");
    restart(d.port, "hearth").await;
    assert_eq!(seal_of(front), k2, "and the new one is stable");
    let r = post(d.port, "/cli/app/seal/rotate", OWNER, json!({"app": "hearth", "yes": true})).await;
    assert_eq!(r.status, 200, "the owner rotates too: {}", r.body);
    let audit = get(d.port, "/cli/app/history", OWNER, "app=hearth").await;
    assert_eq!(audit.status, 200);
    assert!(files_holding(env.path(), &k2).is_empty(), "the rotated key leaked");

    // A plain uninstall keeps the key for a reinstall, like the App's data
    // (purge destroys it: apps::seal's unit test).
    let r = post(d.port, "/cli/app/uninstall", OWNER, json!({"app": "meet-x"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let front2 = install(d.port, OWNER, "meet-x", &ws).await;
    deploy_folder(d.port, "meet-x", &o1).await;
    assert_eq!(seal_of(front2), ko, "a plain uninstall keeps the key (like the data)");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requires_sew_and_voice_refuse_install_and_deploy_with_the_fix() {
    require_python3();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "reqws");
    // This box has no Sew (and voice is off).
    let meet = ws.path.join("meetseed");
    app_folder(&meet, "meetapp", "1.0.0", "m1");
    std::fs::write(meet.join("manifest.json"), manifest_with_requires("meetapp", r#"{"sew":">=0.3.0","voice":true}"#)).unwrap();

    // Validate passes: the form is fine and validate never asks the box.
    let r = post(d.port, "/cli/app/validate", OWNER, json!({"folder": meet})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["ok"], true, "{}", r.body);

    // Install with the folder: refused before anything exists.
    let r = post(d.port, "/cli/app/install", OWNER, json!({"app": "meetapp", "workspace": ws.id, "folder": meet})).await;
    assert_eq!((r.status, r.code()), (409, "requires_unmet".to_string()), "{}", r.body);
    let e = &r.json()["error"];
    let hint = e["hint"].as_str().unwrap();
    assert!(hint.contains(">=0.3.0") && hint.contains("voice"), "{hint}");
    let fix: Vec<String> = e["fix"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
    if cfg!(target_os = "linux") {
        assert!(fix.iter().any(|f| f == "k2 hostchat enable --voice"), "{fix:?}");
    } else {
        assert!(e["missing"][0].as_str().unwrap().contains("Linux"), "{e}");
    }
    assert_eq!(e["sew"]["state"], "not-installed", "{e}");
    let r = get(d.port, "/cli/app/inspect", OWNER, "app=meetapp").await;
    assert_eq!(r.status, 404, "no half-installed App: {}", r.body);

    // An App without requires installs; deploying a Seed that requires
    // Sew is refused the same way, and the live version stays.
    let front = install(d.port, OWNER, "meetapp", &ws).await;
    let plain = app_folder(&env.path().join("plain"), "meetapp", "1.0.0", "plain");
    deploy_folder(d.port, "meetapp", &plain).await;
    let r = post(d.port, "/cli/app/deploy", OWNER, json!({"app": "meetapp", "folder": meet, "wait": 30})).await;
    assert_eq!((r.status, r.code()), (409, "requires_unmet".to_string()), "{}", r.body);
    assert!(r.json()["error"]["hint"].as_str().unwrap().contains("K2 doesn't turn Sew or voice on by itself"), "{}", r.body);
    assert_eq!(app_get(front, "/").1.trim(), "plain", "the live version still serves");

    // A malformed range never reaches the box check: validate and the
    // build refuse it.
    std::fs::write(meet.join("manifest.json"), manifest_with_requires("meetapp", r#"{"sew":"newest"}"#)).unwrap();
    let r = post(d.port, "/cli/app/validate", OWNER, json!({"folder": meet})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["ok"], false, "{}", r.body);
    assert!(r.body.contains("requires.sew"), "{}", r.body);
}
