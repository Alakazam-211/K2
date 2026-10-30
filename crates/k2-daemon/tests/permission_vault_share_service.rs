//! `vault.share-service`'s wall, with the real switches (lib tests never flip
//! the process-wide one): an agent's `k2 publish secret add` / `k2 vault grant
//! --to service:` on a secret a person gave.
//!
//! - cards off, and cards on with the agent's own switch off: the refusal is
//!   today's, byte for byte, with no extra field and no `permission`;
//! - both on: the refusal names the secret, service and delivery, and teaches
//!   the exact Ask-with line, on the same route both verbs post to;
//! - a share a card can't do (another workspace, a delivery a card doesn't
//!   give) stays "It can't be requested".
//!
//! The card itself is tested in `permission/actions/vault_tests.rs`, and the
//! whole flow headless in `tests/cli/card_vault_share_service_headless.sh`.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use k2_vault::{Actor, Delivery, Kind, Namespace, PutRequest, Scope, SecretBytes, SecretStore};
use serde_json::{json, Value};

const GRANT: &str = "/cli/vault/grant";
const VALUE: &[u8] = b"k2test-wall-value-not-real-5512";

struct SwitchOff;
impl Drop for SwitchOff {
    fn drop(&mut self) {
        k2_core::permission::gate::set_server_switch(false);
    }
}

fn agent_switch(ws: &Ws, on: bool) {
    k2_core::workspace::settings::set_agents_can_request_permissions(&ws.id, on).expect("agent switch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_refusal_teaches_the_card_only_while_both_switches_are_on() {
    let env = setup(&[]);
    let _off = SwitchOff;
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "shareagent");
    let other = seed_ws(&env, "shareother");
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let vault = k2_daemon::vault::get().expect("the test vault");
    let mut put = PutRequest::new(Namespace::User, Scope::workspace(ws.id.clone()), "PREVIEW_PIN", Kind::Generic);
    put.default_delivery = Some(Delivery::File);
    put.delivery_ceiling = Some(Delivery::Env);
    vault.put(put, SecretBytes::from_slice(VALUE), &Actor::Owner).expect("a person gives the secret");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::insert(&conn, &ws.id, "preview", "node s.mjs", &ws.path.to_string_lossy(), 47_401, "local", "stopped", "cmd", "").expect("service");
    }
    let agent = passport(&ws);
    let body = json!({ "name": "PREVIEW_PIN", "to": "service:preview", "as": "file" });
    let today = json!({ "error": { "code": "vault_owner_confirm_required", "hint": format!(
        "a person gave this secret, so an agent can't widen where it goes or who can read it. Settings → Vault can't share with a service, so ask the owner or an Admin to run this in their own terminal (Terminal or iTerm, not a K2 terminal; it asks for Touch ID or the vault password; --scope is there because their default scope is server): k2 publish secret add preview PREVIEW_PIN --scope ws:{} --as file",
        ws.handle
    ) } }).to_string();

    // Off, and on with the agent's own switch off: today's bytes.
    let r = post(port, GRANT, &agent, body.clone()).await;
    assert_eq!((r.status, r.body.as_str()), (403, today.as_str()), "cards off");
    k2_core::permission::gate::set_server_switch(true);
    agent_switch(&ws, false);
    let r = post(port, GRANT, &agent, body.clone()).await;
    assert_eq!((r.status, r.body.as_str()), (403, today.as_str()), "the agent's switch off");

    // Both on: the exact line to ask with.
    agent_switch(&ws, true);
    let r = post(port, GRANT, &agent, body.clone()).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let v: Value = r.json();
    assert_eq!(v["error"]["code"], "vault_owner_confirm_required", "{v}");
    assert_eq!((v["error"]["secret"].as_str(), v["error"]["service"].as_str(), v["error"]["as"].as_str()), (Some("PREVIEW_PIN"), Some("preview"), Some("file")), "{v}");
    let hint = v["error"]["hint"].as_str().unwrap();
    assert!(hint.starts_with("a person gave this secret") && hint.contains("allow it on a permission card"), "{hint}");
    assert_eq!(v["permission"]["action"], "vault.share-service", "{v}");
    assert_eq!(v["permission"]["approvers"], "owner or admin", "{v}");
    assert_eq!(v["permission"]["command"], "k2 permission request vault.share-service as=file secret=PREVIEW_PIN service=preview --why \"<what you need it for>\"", "{v}");

    // The agent's chosen names carry; --env-name alone means env.
    let r = post(port, GRANT, &agent, json!({ "name": "PREVIEW_PIN", "to": "service:preview", "envName": "THE_PIN" })).await;
    assert_eq!(r.json()["permission"]["command"], "k2 permission request vault.share-service as=env env_name=THE_PIN secret=PREVIEW_PIN service=preview --why \"<what you need it for>\"", "{}", r.body);

    // What a card can't do stays "It can't be requested": a delivery that
    // never reaches a service, a share with another workspace.
    let r = post(port, GRANT, &agent, json!({ "name": "PREVIEW_PIN", "to": "service:preview", "as": "fetch" })).await;
    assert_eq!((r.status, r.json()["permission"]["requestable"].clone()), (403, json!(false)), "{}", r.body);
    let r = post(port, GRANT, &agent, json!({ "name": "PREVIEW_PIN", "to": format!("ws:{}", other.handle), "as": "env" })).await;
    assert_eq!((r.status, r.json()["permission"]["requestable"].clone()), (403, json!(false)), "{}", r.body);
    // A person is never taught a card.
    let r = post(port, GRANT, OWNER, json!({ "name": "PREVIEW_PIN", "scope": "workspace", "workspace": ws.handle, "to": "service:preview", "as": "file" })).await;
    assert!(r.json().get("permission").is_none(), "{}", r.body);
    assert!(!r.body.contains("k2test-wall-value"));
    drop(env);
}
