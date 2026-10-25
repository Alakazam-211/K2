//! Compute secret-env (prd-compute-secret-env-v1 SE10, SE13–SE15): a
//! `compute:<ws>/<node>` grant reaches that workspace's jobs on that node;
//! `compute:<ws>/*` only nodes this server owns; never an implicit
//! workspace grant; env delivery only; people-only grants; one audit row
//! per release, written at once. Fake secrets only.

use std::sync::Arc;

use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::keystore::KeyStore;
use k2_vault::*;

const VALUE: &str = "k2test-secret-COMPUTE-9f8e7d6c5b4a3210";

fn open(t: &TestDir) -> LocalVault {
    let ks = Arc::new(FakeKeyStore::new());
    LocalVault::open(t.path().join("vault"), ks as Arc<dyn KeyStore>, OpenOptions { kdf: KdfCost::Floor }).expect("open vault")
}

fn agent(ws: &str, manage: bool) -> Actor {
    Actor::Agent(AgentActor { workspace_id: ws.into(), session_id: Some("s1".into()), shell: false, vault_manage: manage })
}

fn compute(ws: &str, node: &str, gen: u32, owned: bool) -> Requester {
    Requester::Compute { workspace_id: ws.into(), node_id: node.into(), job_id: "job-1".into(), generation: gen, owned_node: owned }
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

fn put_ws_secret(v: &LocalVault) -> (Selector, String) {
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws-rel"), "GMAIL_CLIENT_SECRET", Kind::ApiKey);
    r.default_delivery = Some(Delivery::Fetch);
    r.delivery_ceiling = Some(Delivery::Env);
    r.hosts = vec!["oauth2.googleapis.com".into()];
    let m = v.put(r, SecretBytes::from_slice(VALUE.as_bytes()), &Actor::Owner).expect("put").meta;
    (Selector::user(Scope::workspace("ws-rel"), "GMAIL_CLIENT_SECRET"), m.id)
}

fn compute_rows(v: &LocalVault, secret_id: &str) -> Vec<AuditRow> {
    v.audit(&AuditQuery { secret_id: Some(secret_id.into()), ..Default::default() }, &Actor::Owner)
        .expect("audit")
        .into_iter()
        .filter(|r| r.actor.starts_with("compute:"))
        .collect()
}

#[test]
fn a_named_node_grant_reaches_that_node_only() {
    let t = TestDir::new("compute-grant");
    let v = open(&t);
    let (sel, id) = put_ws_secret(&v);

    // No grant: the workspace's implicit right never reaches a node.
    let e = v.resolve(&sel, Use::Env, &compute("ws-rel", "node-a", 1, true)).err().expect("no implicit grant");
    assert_eq!(e.code, "delivery_not_granted");
    // An explicit workspace grant isn't a compute grant either.
    grant(&v, sel.clone(), Consumer::workspace("ws-rel"), Delivery::Env, None, &Actor::Owner).expect("ws grant");
    assert!(v.resolve(&sel, Use::Env, &compute("ws-rel", "node-a", 1, true)).is_err());

    grant(&v, sel.clone(), Consumer::compute("ws-rel", "node-a"), Delivery::Env, Some("BUILD_GMAIL_CLIENT_SECRET"), &Actor::Owner).expect("owner grants");
    let got = v.resolve(&sel, Use::Env, &compute("ws-rel", "node-a", 1, false)).expect("named node resolves, owned or not");
    assert_eq!(got.value.expose(), VALUE.as_bytes());
    assert_eq!(got.env_name.as_deref(), Some("BUILD_GMAIL_CLIENT_SECRET"));
    // Another node, another workspace on that node: nothing.
    assert!(v.resolve(&sel, Use::Env, &compute("ws-rel", "node-b", 1, true)).is_err());
    assert!(v.resolve(&sel, Use::Env, &compute("ws-x", "node-a", 1, true)).is_err());
    // Only env: a compute requester never gets api.
    assert!(v.resolve(&sel, Use::Api, &compute("ws-rel", "node-a", 1, true)).is_err());

    // One audit row per release, at once (no rollup flush in between),
    // naming the node, the job and the attempt; never the value.
    let rows = compute_rows(&v, &id);
    let ok: Vec<_> = rows.iter().filter(|r| r.outcome == "ok").collect();
    assert_eq!(ok.len(), 1, "{rows:?}");
    assert_eq!(ok[0].actor, "compute:ws-rel/node-a");
    assert_eq!(ok[0].consumer.as_deref(), Some("compute:ws-rel/node-a job job-1 gen 1"));
    assert_eq!(ok[0].count, 1);
    // A retry is a new generation: a second row of its own.
    v.resolve(&sel, Use::Env, &compute("ws-rel", "node-a", 2, false)).expect("retry");
    let ok2 = compute_rows(&v, &id).into_iter().filter(|r| r.outcome == "ok").count();
    assert_eq!(ok2, 2);
    // Denials are audited too.
    assert!(compute_rows(&v, &id).iter().any(|r| r.outcome.starts_with("deny:")), "denials are audited");
    for r in compute_rows(&v, &id) {
        let all = format!("{r:?}");
        assert!(!all.contains(VALUE), "audit row carries the value: {all}");
    }
    // Revoked: the next attempt is refused.
    let g = v.grants_to(&Consumer::compute("ws-rel", "node-a")).expect("grants_to");
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].1, "GMAIL_CLIENT_SECRET");
    v.revoke(&g[0].0.id, &Actor::Owner).expect("revoke");
    assert!(v.resolve(&sel, Use::Env, &compute("ws-rel", "node-a", 3, false)).is_err());
}

#[test]
fn star_covers_only_nodes_this_server_owns() {
    let t = TestDir::new("compute-grant-star");
    let v = open(&t);
    let (sel, _) = put_ws_secret(&v);
    grant(&v, sel.clone(), Consumer::compute("ws-rel", "*"), Delivery::Env, None, &Actor::Owner).expect("owner grants *");
    let got = v.resolve(&sel, Use::Env, &compute("ws-rel", "node-own", 1, true)).expect("owned node");
    assert_eq!(got.env_name.as_deref(), Some("GMAIL_CLIENT_SECRET"), "the default env name");
    let e = v.resolve(&sel, Use::Env, &compute("ws-rel", "node-tenant", 1, false)).err().expect("a tenant node needs a named grant");
    assert_eq!(e.code, "delivery_not_granted");
    assert!(v.resolve(&sel, Use::Env, &compute("ws-x", "node-own", 1, true)).is_err());
}

#[test]
fn compute_grants_are_env_only_user_only_and_people_only() {
    let t = TestDir::new("compute-grant-rules");
    let v = open(&t);
    let (sel, _) = put_ws_secret(&v);
    for d in [Delivery::Fetch, Delivery::File, Delivery::Api] {
        let e = grant(&v, sel.clone(), Consumer::compute("ws-rel", "node-a"), d, None, &Actor::Owner).expect_err("not env");
        assert!(e.code == "bad_delivery" || e.code == "delivery_above_ceiling", "{d:?}: {}", e.code);
    }
    let bad = Consumer { kind: ConsumerKind::Compute, id: "no-slash".into() };
    assert_eq!(grant(&v, sel.clone(), bad, Delivery::Env, None, &Actor::Owner).expect_err("bad id").code, "bad_consumer");
    // The env name passes the vault's deny list.
    assert!(grant(&v, sel.clone(), Consumer::compute("ws-rel", "node-a"), Delivery::Env, Some("LD_PRELOAD"), &Actor::Owner).is_err());
    // Agents: refused, the workspace's own agent and an IT agent alike (SV7).
    let e = grant(&v, sel.clone(), Consumer::compute("ws-rel", "node-a"), Delivery::Env, None, &agent("ws-rel", false)).expect_err("own agent");
    assert_eq!(e.code, "vault_owner_confirm_required");
    let mut r = PutRequest::new(Namespace::User, Scope::server(), "FLEET_KEY", Kind::ApiKey);
    r.hosts = vec!["api.example.com".into()];
    r.delivery_ceiling = Some(Delivery::Env);
    v.put(r, SecretBytes::from_slice(VALUE.as_bytes()), &Actor::Owner).expect("put server secret");
    let srv = Selector::user(Scope::server(), "FLEET_KEY");
    let e = grant(&v, srv.clone(), Consumer::compute("ws-sales", "*"), Delivery::Env, None, &agent("ws-it", true)).expect_err("IT agent");
    assert_eq!(e.code, "vault_owner_confirm_required");
    assert!(v.grants_to(&Consumer::compute("ws-sales", "*")).expect("grants").is_empty());
    // A system-namespace secret never goes to compute.
    let mut s = PutRequest::new(Namespace::System, Scope::engine("stalwart"), "SYS_KEY", Kind::ApiKey);
    s.delivery_ceiling = Some(Delivery::Env);
    v.put(s, SecretBytes::from_slice(VALUE.as_bytes()), &Actor::System("mail".into())).expect("system put");
    let e = grant(&v, Selector::system(Scope::engine("stalwart"), "SYS_KEY"), Consumer::compute("ws-rel", "*"), Delivery::Env, None, &Actor::System("mail".into()))
        .expect_err("system secret");
    assert_eq!(e.code, "bad_consumer");
}

#[test]
fn the_compute_kind_is_known_and_last() {
    // R3: new consumer kinds go last; an older build (0.46.3) doesn't know
    // `compute` and skips such rows (R2, row_tolerance.rs proves the skip
    // with a made-up kind).
    assert_eq!(ConsumerKind::parse("compute").expect("known"), ConsumerKind::Compute);
    assert_eq!(ConsumerKind::Compute.code() as usize, ConsumerKind::ALL.len() - 1);
    assert_eq!(compute("w", "n", 1, true).label(), "compute:w/n");
}
