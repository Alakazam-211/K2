//! Static Apps, headless (prd-tauri-core-0467-v1 TC-1, TC-3, TC-5): the
//! real dispatcher in-process on a temp `$HOME`, no client, no App engine
//! (static Apps need none, so this runs on Linux and macOS alike).
//!
//! A Vite-like `dist/` built and deployed with the manifest kept outside
//! it (`--manifest`), served by K2 itself through the App's front: SPA
//! fallback, a second deploy, the instant rollback, stop, list/inspect
//! wording, the conflict and jail refusals, and the Apps switch.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use serde_json::json;
use std::path::{Path, PathBuf};

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

fn static_manifest(id: &str, version: &str) -> String {
    json!({
        "schema": 1, "kind": "app", "id": id, "name": id, "version": version,
        "host": { "mode": "static", "spa": true, "checks": [{ "name": "home", "path": "/", "expect": 200 }] }
    })
    .to_string()
}

/// A repo with `k2/web.manifest.json` and a built `dist/` whose page says
/// `tag`. Returns (dist, manifest file).
fn site(repo: &Path, id: &str, tag: &str) -> (PathBuf, PathBuf) {
    let dist = repo.join("dist");
    std::fs::create_dir_all(dist.join("assets")).unwrap();
    std::fs::write(dist.join("index.html"), format!("<!doctype html><title>{tag}</title>{tag}")).unwrap();
    std::fs::write(dist.join("assets/index-BX4f1a2c.js"), "console.log(1)").unwrap();
    let m = repo.join("k2/web.manifest.json");
    std::fs::create_dir_all(m.parent().unwrap()).unwrap();
    std::fs::write(&m, static_manifest(id, "1.0.0")).unwrap();
    (dist, m)
}

fn page(front: u16, path: &str) -> String {
    http_get(front, path).map(|(s, b)| format!("{s} {}", b.trim())).unwrap_or_else(|| "no answer".into())
}

#[tokio::test]
async fn static_app_builds_with_an_outside_manifest_deploys_rolls_back_and_stops() {
    let env = setup(&[("K2_APPS_WATCH_SECS", "60")]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let ws = seed_ws(&env, "stws");
    let (dist, mf) = site(&ws.path.join("site"), "docs", "v1");

    // TC-3: build dist/ with the manifest kept in the repo.
    let r = post(d.port, "/cli/app/build", OWNER, json!({"folder": dist, "manifest": mf})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let hash1 = v["seed"]["hash"].as_str().unwrap().to_string();
    assert_eq!(v["seed"]["hostMode"], "static");
    assert!(!dist.join("manifest.json").exists(), "the folder is never written");
    let warnings = v["seed"]["report"]["warnings"].as_array().cloned().unwrap_or_default();
    assert!(!warnings.iter().any(|w| w["code"] == "static_not_on_runtime"), "{warnings:?}");
    assert!(warnings.iter().any(|w| w["code"] == "needs_k2_0467"), "{warnings:?}");
    // The Seed hash covers the written manifest (TA-7).
    let mf2 = ws.path.join("site/k2/other.manifest.json");
    std::fs::write(&mf2, static_manifest("docs", "1.0.1")).unwrap();
    let r = post(d.port, "/cli/app/build", OWNER, json!({"folder": dist, "manifest": mf2})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_ne!(r.json()["seed"]["hash"].as_str().unwrap(), hash1, "another manifest, another Seed");
    // The folder's own manifest.json: refused when it differs, fine when equal.
    std::fs::write(dist.join("manifest.json"), r#"{"name":"My PWA","icons":[]}"#).unwrap();
    let r = post(d.port, "/cli/app/build", OWNER, json!({"folder": dist, "manifest": mf})).await;
    assert_eq!(r.status, 409, "{}", r.body);
    assert_eq!(r.code(), "manifest_conflict");
    std::fs::copy(&mf, dist.join("manifest.json")).unwrap();
    let r = post(d.port, "/cli/app/build", OWNER, json!({"folder": dist, "manifest": mf})).await;
    assert_eq!(r.json()["seed"]["hash"].as_str().unwrap(), hash1, "the same bytes, the same Seed");
    std::fs::remove_file(dist.join("manifest.json")).unwrap();
    // A broken outside manifest names its file and place.
    let bad = ws.path.join("site/k2/bad.manifest.json");
    std::fs::write(&bad, "{\"schema\": 1,").unwrap();
    let r = post(d.port, "/cli/app/build", OWNER, json!({"folder": dist, "manifest": bad})).await;
    assert_eq!(r.status, 422, "{}", r.body);
    assert_eq!(r.code(), "manifest_invalid");
    assert_eq!(r.json()["error"]["report"]["errors"][0]["code"], "manifest_invalid");
    // validate takes the same flag.
    let r = post(d.port, "/cli/app/validate", OWNER, json!({"folder": dist, "manifest": mf})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["ok"], true, "{}", r.body);

    // An agent's manifest outside its workspace is refused (the folder jail).
    let outside = env.path().join("elsewhere.manifest.json");
    std::fs::write(&outside, static_manifest("docs", "1.0.0")).unwrap();
    allow_manage(&ws, true);
    let pass = passport(&ws);
    let r = post(d.port, "/cli/app/build", &pass, json!({"folder": dist, "manifest": outside})).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.code(), "folder_outside_workspace");

    // TC-1/TC-5: install, deploy dist/ with the outside manifest; K2 serves it.
    let r = post(d.port, "/cli/app/install", OWNER, json!({"app": "docs", "workspace": ws.id})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let front = r.json()["frontPort"].as_u64().unwrap() as u16;
    let _cleanup = StopOnDrop("docs");
    let r = post(d.port, "/cli/app/deploy", OWNER, json!({"app": "docs", "folder": dist, "manifest": mf, "wait": 60})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["deployment"]["result"], "live", "{}", r.body);
    assert!(page(front, "/").starts_with("200 <!doctype html><title>v1</title>"), "{}", page(front, "/"));
    assert!(page(front, "/settings/profile").starts_with("200 <!doctype html><title>v1"), "the SPA fallback");
    assert!(page(front, "/missing.png").starts_with("404"), "{}", page(front, "/missing.png"));

    // list and inspect: static, never a weaker wall.
    let r = get(d.port, "/cli/app/list", OWNER, "").await;
    let apps = r.json()["apps"].as_array().cloned().unwrap();
    let row = apps.iter().find(|a| a["appId"] == "docs").expect("listed");
    assert_eq!(row["mode"], "static");
    assert_eq!(row["isolationShown"], "none (static: no App code)");
    let r = get(d.port, "/cli/app/inspect", OWNER, "app=docs").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["runtime"]["engine"], "static");

    // A second deploy, then the instant rollback to the warm first one.
    std::fs::write(dist.join("index.html"), "<!doctype html><title>v2</title>v2").unwrap();
    let r = post(d.port, "/cli/app/deploy", OWNER, json!({"app": "docs", "folder": dist, "manifest": mf, "wait": 60})).await;
    assert_eq!(r.json()["deployment"]["result"], "live", "{}", r.body);
    assert!(page(front, "/").contains("<title>v2</title>"), "{}", page(front, "/"));
    let r = post(d.port, "/cli/app/rollback", OWNER, json!({"app": "docs", "wait": 60})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["deployment"]["result"], "live", "{}", r.body);
    assert!(page(front, "/").contains("<title>v1</title>"), "{}", page(front, "/"));

    // Stop: K2's own 503.
    let r = post(d.port, "/cli/app/stop", OWNER, json!({"app": "docs"})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(page(front, "/").starts_with("503"), "{}", page(front, "/"));

    // The Apps switch still stops every runtime route (apps_disabled).
    {
        let _off = k2_core::test_env::EnvVar::remove("K2_APPS");
        k2_core::deploy::set_enabled(false);
        let r = post(d.port, "/cli/app/deploy", OWNER, json!({"app": "docs", "folder": dist, "manifest": mf})).await;
        assert_eq!(r.status, 404, "{}", r.body);
        assert_eq!(r.code(), "apps_disabled");
        k2_core::deploy::set_enabled(true);
    }
}
