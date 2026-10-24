//! `k2 job` (prd-vault-gaps-v1 item 5): a `job:<ws>/<job>` grant reaches
//! that job's runs only, and a run gets nothing from its workspace's
//! implicit grants (Q13). Agents grant their own workspace's secrets to
//! their own jobs at or below the default delivery (Q14). Fakes only.

use std::sync::Arc;

use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::hosts::Origin;
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::keystore::KeyStore;
use k2_vault::*;

const VALUE: &str = "k2test_JOBGRANT_1a2b3c4d5e6f7a8b9c";

fn open(t: &TestDir) -> LocalVault {
    let ks = Arc::new(FakeKeyStore::new());
    LocalVault::open(t.path().join("vault"), ks as Arc<dyn KeyStore>, OpenOptions { kdf: KdfCost::Floor }).expect("open vault")
}

fn agent(ws: &str, manage: bool) -> Actor {
    Actor::Agent(AgentActor { workspace_id: ws.into(), session_id: Some("s1".into()), shell: false, vault_manage: manage })
}

fn job(ws: &str, id: &str) -> Requester {
    Requester::Job { workspace_id: ws.into(), job_id: id.into(), name: "collect".into() }
}

fn grant(v: &LocalVault, sel: Selector, consumer: Consumer, d: Delivery, env: Option<&str>, by: &Actor) -> Result<Grant> {
    v.grant(
        GrantRequest {
            selector: sel,
            consumer,
            delivery: d,
            env_name: env.map(str::to_string),
            file_name: None,
            hosts_narrowed: None,
            expires_at: None,
            version: None,
        },
        by,
    )
}

#[test]
fn a_job_gets_only_its_own_grants() {
    let t = TestDir::new("job-grant");
    let v = open(&t);
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws-ops"), "OVH_KEY", Kind::ApiKey);
    r.hosts = vec!["eu.api.ovh.com".into()];
    r.default_delivery = Some(Delivery::Env);
    v.put(r, SecretBytes::from_slice(VALUE.as_bytes()), &Actor::Owner).expect("put");
    let sel = Selector::user(Scope::workspace("ws-ops"), "OVH_KEY");
    let ovh = Origin::parse_url("https://eu.api.ovh.com/1.0/me").expect("url").1;

    // The workspace's own sessions get it implicitly; its job does not.
    assert!(v.resolve(&sel, Use::Env, &Requester::Session { workspace_id: "ws-ops".into(), session_id: None }).is_ok());
    assert_eq!(v.resolve(&sel, Use::Env, &job("ws-ops", "j1")).err().expect("no implicit grant").code, "delivery_not_granted");
    assert_eq!(v.resolve(&sel, Use::Fetch(&ovh), &job("ws-ops", "j1")).err().expect("no implicit grant").code, "not_granted");
    assert!(v.spawn_items(&job("ws-ops", "j1")).expect("items").is_empty(), "no implicit spawn items for a job");

    // An explicit workspace grant still isn't a job grant.
    grant(&v, sel.clone(), Consumer::workspace("ws-ops"), Delivery::Env, None, &Actor::Owner).expect("ws grant");
    assert!(v.resolve(&sel, Use::Env, &job("ws-ops", "j1")).is_err(), "a workspace grant alone isn't enough");

    // The agent grants it to its own job (env = the default: allowed).
    grant(&v, sel.clone(), Consumer::job("ws-ops", "j1"), Delivery::Env, Some("OVH_APP_KEY"), &agent("ws-ops", false))
        .expect("agent grants to its own job");
    let got = v.resolve(&sel, Use::Env, &job("ws-ops", "j1")).expect("job resolves");
    assert_eq!(got.value.expose(), VALUE.as_bytes());
    assert_eq!(got.env_name.as_deref(), Some("OVH_APP_KEY"));
    let items = v.spawn_items(&job("ws-ops", "j1")).expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].env_name.as_deref(), Some("OVH_APP_KEY"));
    // Another job of the same workspace, or a same-id job elsewhere, gets nothing.
    assert!(v.resolve(&sel, Use::Env, &job("ws-ops", "j2")).is_err());
    assert!(v.resolve(&sel, Use::Env, &job("ws-other", "j1")).is_err());
    assert!(v.spawn_items(&job("ws-ops", "j2")).expect("items").is_empty());
    // The job's audit label names the job.
    assert_eq!(job("ws-ops", "j1").label(), "job:ws-ops/collect");
    let to = v.grants_to(&Consumer::job("ws-ops", "j1")).expect("grants_to");
    assert_eq!(to.len(), 1);
    assert_eq!(to[0].1, "OVH_KEY");

    // Above the default delivery (api) is a person's call.
    assert!(grant(&v, sel.clone(), Consumer::job("ws-ops", "j1"), Delivery::Api, None, &agent("ws-ops", false)).is_err());
    // Another workspace's agent can't grant to this job (nor to its own job).
    assert!(grant(&v, sel.clone(), Consumer::job("ws-ops", "j1"), Delivery::Fetch, None, &agent("ws-x", false)).is_err());
    assert!(grant(&v, sel.clone(), Consumer::job("ws-x", "j9"), Delivery::Fetch, None, &agent("ws-x", false)).is_err());
    // A malformed job consumer is refused.
    let bad = Consumer { kind: ConsumerKind::Job, id: "no-slash".into() };
    assert_eq!(grant(&v, sel, bad, Delivery::Fetch, None, &Actor::Owner).err().expect("bad id").code, "bad_consumer");
}

#[test]
fn an_it_agent_cannot_grant_a_server_secret_to_its_own_job() {
    let t = TestDir::new("job-grant-it");
    let v = open(&t);
    let mut r = PutRequest::new(Namespace::User, Scope::server(), "FLEET_KEY", Kind::ApiKey);
    r.hosts = vec!["api.example.com".into()];
    v.put(r, SecretBytes::from_slice(VALUE.as_bytes()), &Actor::Owner).expect("put");
    let sel = Selector::user(Scope::server(), "FLEET_KEY");
    let e = grant(&v, sel.clone(), Consumer::job("ws-it", "j1"), Delivery::Fetch, None, &agent("ws-it", true)).expect_err("self");
    assert_eq!(e.code, "vault_self_grant");
    // Another workspace's job: allowed for an IT agent.
    grant(&v, sel, Consumer::job("ws-ops", "j1"), Delivery::Fetch, None, &agent("ws-it", true)).expect("other workspace's job");
}
