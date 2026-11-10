//! GET /cli/agents/cli-status: which agent CLIs are installed. Read only.
//!
//! The per-state behaviour (missing / installed / broken / timeout / cache)
//! is pinned in `k2_core::terminal::cli_status` with fake binaries. Here:
//! the policy row (Member GET, never POST), the exact dispatcher arm on
//! the blocking pool, and the wire shape over HTTP. A test build never
//! runs a real agent CLI: a real binary on this host's PATH is reported
//! as found but "not checked" by the agent spawn guard.

use std::collections::HashMap;

use k2_daemon::routes::route_policy::{self, Floor};
use k2_daemon::test_harness;

use super::ensure_cli_route::http;

const OWNER_TOKEN: &str = "owner-token-cli-status-route";

#[test]
fn cli_status_policy_row_is_a_member_get() {
    let row = route_policy::lookup("/cli/agents/cli-status").expect("cli-status has a policy row");
    assert_eq!(row.get, Some(Floor::Member), "Member floor, like ensure-cli");
    assert_eq!(row.post, None, "read only: never on the POST allowlist");
    assert!(!route_policy::post_allowed("/cli/agents/cli-status"));
}

#[test]
fn cli_status_arm_is_exact_and_off_the_async_worker() {
    let src = include_str!("../../src/routes/dispatcher.rs");
    let marker = "\"/cli/agents/cli-status\" if !is_post =>";
    let arm_at = src.find(marker).expect("exact cli-status arm");
    let window = &src[arm_at..arm_at + 900];
    assert!(window.contains("token_ok"), "token gate: {window}");
    assert!(window.contains("spawn_blocking"), "probes spawn processes: {window}");
}

/// Every row has all six keys, one row per launchable harness, in
/// launch-bar order.
fn assert_shape(body: &str) {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or_else(|e| panic!("json: {e}: {body}"));
    assert_eq!(v["ok"], serde_json::json!(true), "{body}");
    assert_eq!(v["cacheSecs"], serde_json::json!(60), "{body}");
    let tools = v["tools"].as_array().unwrap_or_else(|| panic!("tools array: {body}"));
    let programs: Vec<&str> = tools.iter().map(|t| t["program"].as_str().expect("program")).collect();
    assert_eq!(programs, k2_core::terminal::cli_status::launchable_harnesses(), "{body}");
    assert_eq!(
        programs,
        [
            "claude", "codex", "grok", "gemini", "cursor-agent", "pi", "hermes", "opencode", "goose",
            "aider", "ollama", "copilot", "interpreter",
        ],
        "exactly the launchable harnesses, in a stable order: {body}"
    );
    for t in tools {
        let obj = t.as_object().expect("row object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["error", "installed", "launches", "path", "program", "version"], "{t}");
        let installed = t["installed"].as_bool().expect("installed bool");
        assert!(t["launches"].is_boolean(), "{t}");
        if installed && t["program"] == "hermes" {
            // No version flag: never run, so no version and no error.
            assert!(t["path"].is_string() && t["version"].is_null() && t["error"].is_null(), "{t}");
        } else if installed {
            assert!(t["path"].is_string(), "{t}");
            // Test build: a real CLI is found but never run.
            let err = t["error"].as_str().unwrap_or_else(|| panic!("a real CLI must not be probed: {t}"));
            assert!(err.starts_with("not checked:"), "{t}");
            assert!(t["version"].is_null(), "{t}");
            assert_eq!(t["launches"], serde_json::json!(false), "test build: agent launches are refused: {t}");
        } else {
            assert!(t["path"].is_null() && t["version"].is_null() && t["error"].is_null(), "{t}");
            assert_eq!(t["launches"], serde_json::json!(false), "{t}");
        }
    }
}

#[test]
fn cli_status_dispatch_reports_every_harness() {
    let resp = k2_daemon::cli::dispatch("/cli/agents/cli-status", &HashMap::new());
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    assert_shape(&resp.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_status_http_get_post_and_token() {
    let daemon = test_harness::start(OWNER_TOKEN).await;
    let port = daemon.port;
    let (ok, post, anon) = tokio::task::spawn_blocking(move || {
        (
            http(port, "GET", &format!("/cli/agents/cli-status?token={OWNER_TOKEN}")),
            http(port, "POST", &format!("/cli/agents/cli-status?token={OWNER_TOKEN}")),
            http(port, "GET", "/cli/agents/cli-status"),
        )
    })
    .await
    .expect("join");
    assert_eq!(ok.status, 200, "body={}", ok.body);
    assert_shape(&ok.body);
    assert_eq!(post.status, 405, "read only: POST is refused; body={}", post.body);
    assert_eq!(anon.status, 403, "no token: body={}", anon.body);
}
