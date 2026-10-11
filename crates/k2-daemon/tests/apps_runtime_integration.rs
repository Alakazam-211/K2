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
