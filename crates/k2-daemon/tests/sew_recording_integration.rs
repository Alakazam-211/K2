//! `k2 hostchat recording` (0.46.11 K-a) over loopback, headless, with the
//! real switches:
//!
//! - the three writes are POST-only: a GET is 405 before auth;
//! - an agent with Manage Chat / Voice Hosting gets `person_only`, and with
//!   cards on, the exact Ask-with line for `sew.recording-enable`; a person
//!   is never taught a card;
//! - a person on a box whose Sew can't record (no `app_recording`; this
//!   test box has no Sew at all) is refused before anything runs.
//!
//! The machine itself is tested in `src/sew/recording_tests.rs`.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use serde_json::json;

const ENABLE: &str = "/cli/sew/recording/enable";
const DISABLE: &str = "/cli/sew/recording/disable";
const UNINSTALL: &str = "/cli/sew/recording/uninstall";

struct SwitchOff;
impl Drop for SwitchOff {
    fn drop(&mut self) {
        k2_core::permission::gate::set_server_switch(false);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recording_writes_are_post_only_and_an_agent_is_taught_the_card() {
    let env = setup(&[]);
    let _off = SwitchOff;
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;

    // POST-only: a GET is 405, whoever asks.
    for path in [ENABLE, DISABLE, UNINSTALL] {
        let r = get(port, path, OWNER, "").await;
        assert_eq!(r.status, 405, "{path}: {}", r.body);
    }
    // The reads answer a GET.
    let r = get(port, "/cli/sew/recording", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["installed"], false, "{}", r.body);
    assert_eq!(r.json()["ready"]["ok"], false, "{}", r.body);

    // An agent with Manage Sew: person_only, never an install.
    let ws = seed_ws(&env, "recagent");
    k2_core::workspace::settings::set_agents_can_manage_chat(&ws.id, true).expect("Manage Sew");
    let agent = passport(&ws);
    let off = post(port, ENABLE, &agent, json!({})).await;
    assert_eq!((off.status, off.code()), (403, "person_only".to_string()), "{}", off.body);
    assert!(off.json()["error"]["hint"].as_str().unwrap().contains("k2 permission request sew.recording-enable"), "{}", off.body);
    assert!(off.json().get("permission").is_none(), "cards off: today's body: {}", off.body);

    // Cards on, the agent's own switch on: the exact line to ask with.
    k2_core::permission::gate::set_server_switch(true);
    k2_core::workspace::settings::set_agents_can_request_permissions(&ws.id, true).expect("agent switch");
    let r = post(port, ENABLE, &agent, json!({})).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let v = r.json();
    assert_eq!(v["error"]["code"], "person_only", "{v}");
    assert_eq!(v["permission"]["action"], "sew.recording-enable", "{v}");
    assert_eq!(v["permission"]["approvers"], "owner or admin", "{v}");
    assert_eq!(v["permission"]["command"], "k2 permission request sew.recording-enable --why \"<what you need it for>\"", "{v}");

    // A person is never taught a card; with no Sew that records here, the
    // refusal comes before any confirm or helper call.
    let r = post(port, ENABLE, OWNER, json!({})).await;
    assert_eq!(r.status, 409, "{}", r.body);
    // In gate order: a build with no Sew release pinned, a Mac, then the flag.
    let want = if !k2_root_proto::sew::sew_release_pinned() {
        "sew_not_released"
    } else if !cfg!(target_os = "linux") {
        "unsupported"
    } else {
        "sew_feature_missing"
    };
    assert_eq!(r.code(), want, "{}", r.body);
    assert!(r.json().get("permission").is_none(), "{}", r.body);
    drop(env);
}
