//! Sew for Apps A9 on the App transport, in process, before Sew is asked:
//! the pass and cap floors, Sew's feature flag, the App's own Space. The
//! mgmt calls themselves are in tests/sew_for_apps_integration.rs (the
//! fake Sew socket).

use k2_core::skin::{self, link as passes};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::*;

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    jobs: String,
    other: String,
    own_ws: String,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let own_ws = uuid::Uuid::new_v4().to_string();
    let acme = format!("acme-{tag}");
    let (jobs, other) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for (id, handle) in [(&own_ws, format!("jobs-{tag}")), (&acme, acme.clone())] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", params![id, handle, format!("/tmp/ccr-{id}")])
                .unwrap();
        }
        let j = k2_core::published_services::insert(&conn, &own_ws, "jobs", "node s.js", "/tmp", 4600, "local", "running", "cmd", "").unwrap();
        let o = k2_core::published_services::insert(&conn, &acme, "other", "node s.js", "/tmp", 4601, "local", "running", "cmd", "").unwrap();
        (j.id, o.id)
    };
    k2_core::tool_hook::allow_target_off_linux_for_tests(true);
    k2_core::tool_hook::set_target(k2_core::tool_hook::CHAT_KIND, true, Some("/tmp/k2-test-nosew.sock"), Some("/tmp/k2-test-nosew.token"), "owner-token")
        .expect("enable chat");
    Fx { _db: db, _home: home, jobs, other, own_ws }
}

fn pass_for(f: &Fx, caps: &[&str]) -> String {
    let caps: Vec<String> = caps.iter().map(|c| c.to_string()).collect();
    passes::mint_remote_pass(&f.jobs, "jobs", &caps, &[f.own_ws.clone()], 3600).unwrap().raw
}

async fn call(raw: &str, post: bool, body: J) -> (u16, J) {
    let pass = crate::apps::gateway::resolve_pass(raw);
    let r = handle(pass, None, post, body.to_string().into_bytes()).await;
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body)))
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

#[tokio::test]
async fn only_the_apps_own_pass_with_chat_channels() {
    let f = fx("floors");
    let body = json!({"slug": "job-1042", "name": "Job 1042"});
    // chat:read is not enough: making channels is its own cap.
    let (st, v) = call(&pass_for(&f, &["chat:read", "chat:post"]), true, body.clone()).await;
    assert_eq!((st, code(&v)), (403, "cap_not_granted"), "{v}");
    assert_eq!(v["error"]["cap"], json!("chat:channels"));
    // A guest's own session pass (even with a password and a login).
    skin::add_principal_for_app("dana", None, &f.jobs, None, "test").unwrap();
    skin::set_principal_password("dana", Some("s3cret-horse-battery")).unwrap();
    let guest = skin::create_app_session_token("dana", &f.jobs).unwrap().raw;
    let (st, v) = call(&guest, true, body.clone()).await;
    assert_eq!((st, code(&v)), (403, "app_pass_required"), "a guest's pass: {v}");
    let (st, v) = call("k2skn_unknownunknownunknownunknown", true, body.clone()).await;
    assert_eq!((st, code(&v)), (401, "pass_invalid"), "{v}");
    let (st, _) = call(&pass_for(&f, &["chat:channels"]), false, body).await;
    assert_eq!(st, 405, "a POST");
    assert!(skin::app_channels::audit_rows(&f.jobs, 5).unwrap().is_empty(), "nothing audited for a refusal");
}

#[tokio::test]
async fn refused_until_sew_reports_app_channel_create() {
    let f = fx("flag");
    let pass = pass_for(&f, &["chat:channels"]);
    k2_core::tool_hook::app_space_bind(&f.jobs, "48831765147549697", "owner-token").unwrap();
    k2_core::tool_hook::note_features_for_tests(&["app_scoped_sessions", "home_client"]).unwrap();
    let (st, v) = call(&pass, true, json!({"slug": "job-1042", "name": "Job 1042"})).await;
    assert_eq!((st, code(&v)), (409, "sew_feature_missing"), "{v}");
    assert_eq!(v["error"]["feature"], json!("app_channel_create"));
    assert!(v["error"]["hint"].as_str().unwrap().contains("Sew 0.5.0"), "{v}");
}

#[tokio::test]
async fn another_apps_space_and_bad_args_never_reach_sew() {
    let f = fx("space");
    let pass = pass_for(&f, &["chat:channels"]);
    k2_core::tool_hook::note_features_for_tests(&["app_channel_create"]).unwrap();
    // No Space of its own yet.
    let (st, v) = call(&pass, true, json!({"slug": "job-1042", "name": "Job 1042"})).await;
    assert_eq!((st, code(&v)), (409, "space_unbound"), "{v}");
    // Another App's Space.
    k2_core::tool_hook::app_space_bind(&f.other, "48831765147549698", "owner-token").unwrap();
    let (st, v) = call(&pass, true, json!({"slug": "job-1042", "name": "Job 1042", "space": "48831765147549698"})).await;
    assert_eq!((st, code(&v)), (409, "app_mismatch"), "{v}");
    assert!(!v.to_string().contains(&f.other), "never names the other App: {v}");
    // Bad args.
    for bad in [json!({"slug": "Job 1042", "name": "x"}), json!({"slug": "job"}), json!({"slug": "job", "name": "x", "members": "dana"})] {
        let (st, v) = call(&pass, true, bad.clone()).await;
        assert_eq!((st, code(&v)), (400, "usage"), "{bad}: {v}");
    }
}
