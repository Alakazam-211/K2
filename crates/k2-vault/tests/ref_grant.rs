//! R8 (prd-env-to-vault-v1): a `k2v://` reference a consumer names in its
//! own config resolves only when that consumer may have the secret. A server
//! secret needs a grant to the service (or its workspace), like any shared
//! secret; a workspace grant never starts auto-delivering to a service.
//! Fakes only.

use std::sync::Arc;

use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::keystore::KeyStore;
use k2_vault::*;

const VALUE: &str = "k2test_R8SERVER_9f8e7d6c5b4a39281706";

fn open(t: &TestDir) -> LocalVault {
    let ks = Arc::new(FakeKeyStore::new());
    LocalVault::open(t.path().join("vault"), ks as Arc<dyn KeyStore>, OpenOptions { kdf: KdfCost::Floor }).expect("open vault")
}

fn svc(id: &str, ws: &str) -> Requester {
    Requester::PublishedService { id: id.into(), workspace_id: ws.into() }
}

fn session(ws: &str, sid: &str) -> Requester {
    Requester::Session { workspace_id: ws.into(), session_id: Some(sid.into()) }
}

fn put_server(v: &LocalVault, name: &str, ceiling: Delivery) -> Selector {
    let mut r = PutRequest::new(Namespace::User, Scope::server(), name, Kind::ApiKey);
    r.hosts = vec!["api.example.com".into()];
    r.default_delivery = Some(Delivery::Fetch);
    r.delivery_ceiling = Some(ceiling);
    v.put(r, SecretBytes::from_slice(VALUE.as_bytes()), &Actor::Owner).expect("put server secret");
    Selector::user(Scope::server(), name)
}

fn grant(v: &LocalVault, sel: &Selector, consumer: Consumer, d: Delivery, by: &Actor) -> Result<Grant> {
    v.grant(
        GrantRequest {
            selector: sel.clone(),
            consumer,
            delivery: d,
            env_name: None,
            file_name: None,
            hosts_narrowed: None,
            expires_at: None,
            version: None,
        },
        by,
    )
}

fn deny_code(r: Result<Resolved>) -> String {
    match r {
        Ok(_) => panic!("expected a refusal, got a value"),
        Err(e) => e.code.to_string(),
    }
}

#[test]
fn a_service_without_a_grant_is_refused_a_server_secret_and_the_refusal_is_audited() {
    let t = TestDir::new("r8-refused");
    let v = open(&t);
    let sel = put_server(&v, "SHARED_TOKEN", Delivery::Env);
    assert_eq!(deny_code(v.resolve_ref_env(&sel, &svc("svc-a", "ws-a"))), "not_granted");
    assert!(!v.ref_would_resolve(&sel, &svc("svc-a", "ws-a")).expect("check"), "no grant: no");
    let rows = v.audit(&AuditQuery::default(), &Actor::Owner).expect("audit");
    assert!(
        rows.iter().any(|r| r.action == "resolve_ref" && r.actor == "published_service:svc-a" && r.outcome == "deny:not_granted"),
        "the refusal is in the audit under the service: {rows:?}"
    );
    // A session gets nothing without a grant either.
    assert_eq!(deny_code(v.resolve_ref_env(&sel, &session("ws-a", "s1"))), "not_granted");
}

#[test]
fn a_service_grant_works_and_only_for_that_service() {
    let t = TestDir::new("r8-service-grant");
    let v = open(&t);
    let sel = put_server(&v, "SHARED_TOKEN", Delivery::Env);
    grant(&v, &sel, Consumer::published_service("svc-a"), Delivery::Env, &Actor::Owner).expect("grant");
    let got = v.resolve_ref_env(&sel, &svc("svc-a", "ws-a")).expect("granted service resolves");
    assert_eq!(got.value.expose(), VALUE.as_bytes());
    assert!(v.ref_would_resolve(&sel, &svc("svc-a", "ws-a")).expect("check"));
    // Another service, even in the same workspace, is refused.
    assert_eq!(deny_code(v.resolve_ref_env(&sel, &svc("svc-b", "ws-a"))), "not_granted");
    v.flush_rollups();
    let rows = v.audit(&AuditQuery::default(), &Actor::Owner).expect("audit");
    assert!(rows.iter().any(|r| r.action == "resolve_ref" && r.actor == "published_service:svc-a" && r.outcome == "ok"), "{rows:?}");
}

/// vs-live A3: a service in workspace B can't read a server secret granted
/// only to workspace A; a service in A can; `ws:*` reaches every service.
#[test]
fn a_workspace_grant_reaches_that_workspaces_services_only() {
    let t = TestDir::new("r8-ws-grant");
    let v = open(&t);
    let sel = put_server(&v, "SHARED_TOKEN", Delivery::Env);
    grant(&v, &sel, Consumer::workspace("ws-a"), Delivery::Env, &Actor::Owner).expect("grant ws-a");
    assert!(v.resolve_ref_env(&sel, &svc("svc-a", "ws-a")).is_ok(), "service in A");
    assert!(v.resolve_ref_env(&sel, &session("ws-a", "s1")).is_ok(), "session in A");
    assert_eq!(deny_code(v.resolve_ref_env(&sel, &svc("svc-b", "ws-b"))), "not_granted", "service in B");
    assert_eq!(deny_code(v.resolve_ref_env(&sel, &session("ws-b", "s2"))), "not_granted", "session in B");
    // No widening: a workspace grant is never auto-delivered to a service.
    assert!(
        v.spawn_items(&svc("svc-a", "ws-a")).expect("items").iter().all(|i| i.name != "SHARED_TOKEN"),
        "a ws grant must not start delivering to the workspace's services"
    );
    let sel2 = put_server(&v, "EVERYONE_TOKEN", Delivery::Env);
    grant(&v, &sel2, Consumer::all_workspaces(), Delivery::Env, &Actor::Owner).expect("grant ws:*");
    assert!(v.resolve_ref_env(&sel2, &svc("svc-b", "ws-b")).is_ok(), "ws:* reaches B's service");
}

#[test]
fn a_grant_below_env_or_a_ceiling_below_env_is_refused() {
    let t = TestDir::new("r8-levels");
    let v = open(&t);
    let sel = put_server(&v, "FETCH_ONLY_GRANT", Delivery::Env);
    grant(&v, &sel, Consumer::published_service("svc-a"), Delivery::Fetch, &Actor::Owner).expect("fetch grant");
    assert_eq!(deny_code(v.resolve_ref_env(&sel, &svc("svc-a", "ws-a"))), "not_granted", "fetch grant isn't env");
    let sel2 = put_server(&v, "FETCH_CEILING", Delivery::Fetch);
    grant(&v, &sel2, Consumer::published_service("svc-a"), Delivery::Fetch, &Actor::Owner).expect("grant");
    assert_eq!(deny_code(v.resolve_ref_env(&sel2, &svc("svc-a", "ws-a"))), "delivery_above_ceiling");
}

#[test]
fn own_workspace_secrets_still_resolve_without_a_grant_but_not_another_workspaces() {
    let t = TestDir::new("r8-own");
    let v = open(&t);
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws-a"), "OWN_KEY", Kind::Generic);
    r.default_delivery = Some(Delivery::Env);
    v.put(r, SecretBytes::from_slice(VALUE.as_bytes()), &Actor::Owner).expect("put");
    let sel = Selector::user(Scope::workspace("ws-a"), "OWN_KEY");
    assert!(v.resolve_ref_env(&sel, &svc("svc-a", "ws-a")).is_ok(), "own workspace: implicit");
    assert!(v.resolve_ref_env(&sel, &session("ws-a", "s1")).is_ok(), "own workspace session: implicit");
    assert_eq!(deny_code(v.resolve_ref_env(&sel, &session("ws-b", "s2"))), "not_granted", "other workspace session");
    assert_eq!(deny_code(v.resolve_ref_env(&sel, &svc("svc-b", "ws-b"))), "not_granted", "other workspace service");
}

#[test]
fn k2s_own_system_secrets_never_resolve_through_a_reference_but_still_do_for_k2() {
    let t = TestDir::new("r8-system");
    let v = open(&t);
    let mut r = PutRequest::new(Namespace::System, Scope::engine("postgres"), "PG_ADMIN", Kind::Password);
    r.default_delivery = Some(Delivery::None);
    r.delivery_ceiling = Some(Delivery::Env);
    v.put(r, SecretBytes::from_slice(VALUE.as_bytes()), &Actor::System("db".into())).expect("put system");
    let sel = Selector::system(Scope::engine("postgres"), "PG_ADMIN");
    assert_eq!(deny_code(v.resolve_ref_env(&sel, &svc("svc-a", "ws-a"))), "vault_system_namespace");
    // K2's own engine path is unchanged.
    let got = v.resolve(&sel, Use::System, &Requester::System("db".into())).expect("K2 resolves its own secret");
    assert_eq!(got.value.expose(), VALUE.as_bytes());
    // And a server secret still resolves for K2's own subsystems (owner paths).
    let srv = put_server(&v, "OWNER_PATH", Delivery::Env);
    assert!(v.resolve(&srv, Use::System, &Requester::System("preset-env".into())).is_ok());
}

/// An IT agent ("Allow managing server-wide secrets") can't hand a server
/// secret to its own workspace's service: the service runs its code.
#[test]
fn an_it_agent_cant_grant_a_server_secret_to_its_own_service() {
    let t = TestDir::new("r8-it-agent");
    let v = open(&t);
    let sel = put_server(&v, "FLEET_TOKEN", Delivery::Env);
    let it = Actor::Agent(AgentActor { workspace_id: "ws-it".into(), session_id: Some("s1".into()), shell: false, vault_manage: true });
    let e = grant(&v, &sel, Consumer::published_service("svc-it"), Delivery::Fetch, &it).expect_err("self-grant refused");
    assert_eq!(e.code, "vault_self_grant");
    grant(&v, &sel, Consumer::workspace("ws-other"), Delivery::Fetch, &it).expect("other workspace still fine");
}

/// Review fix 1: an own-workspace secret is reachable through a reference
/// only at its default delivery: a fetch- or proxy-only key (whose ceiling
/// allows env) never becomes raw env, for a service or a session.
#[test]
fn an_own_workspace_reference_is_capped_at_the_default_delivery() {
    let t = TestDir::new("r8-default-cap");
    let v = open(&t);
    for (name, d) in [("FETCH_ONLY", Delivery::Fetch), ("PROXY_ONLY", Delivery::Proxy), ("FILE_DEFAULT", Delivery::File)] {
        let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws-a"), name, Kind::ApiKey);
        r.hosts = vec!["api.example.com".into()];
        r.default_delivery = Some(d);
        r.delivery_ceiling = Some(Delivery::Env);
        v.put(r, SecretBytes::from_slice(VALUE.as_bytes()), &Actor::Owner).expect("put");
        let sel = Selector::user(Scope::workspace("ws-a"), name);
        assert_eq!(deny_code(v.resolve_ref_env(&sel, &svc("svc-a", "ws-a"))), "not_granted", "{name} via a service");
        assert_eq!(deny_code(v.resolve_ref_env(&sel, &session("ws-a", "s1"))), "not_granted", "{name} via a session");
        // An explicit env grant (a person's choice) still works.
        grant(&v, &sel, Consumer::published_service("svc-a"), Delivery::Env, &Actor::Owner).expect("env grant");
        assert!(v.resolve_ref_env(&sel, &svc("svc-a", "ws-a")).is_ok(), "{name} granted as env");
    }
}

/// Review fix 9: a grant that pins a version delivers that version.
#[test]
fn a_pinned_grant_delivers_its_version() {
    let t = TestDir::new("r8-pinned");
    let v = open(&t);
    let sel = put_server(&v, "PINNED_TOKEN", Delivery::Env);
    v.rotate(&sel, SecretBytes::from_slice(b"k2test_R8SERVER_second_version_000"), &Actor::Owner).expect("rotate");
    v.grant(
        GrantRequest {
            selector: sel.clone(),
            consumer: Consumer::published_service("svc-a"),
            delivery: Delivery::Env,
            env_name: None,
            file_name: None,
            hosts_narrowed: None,
            expires_at: None,
            version: Some(1),
        },
        &Actor::Owner,
    )
    .expect("pinned grant");
    let got = v.resolve_ref_env(&sel, &svc("svc-a", "ws-a")).expect("resolves");
    assert_eq!(got.version, 1);
    assert_eq!(got.value.expose(), VALUE.as_bytes(), "the pinned version, not the latest");
    // An unpinned grant (another consumer path) gets the latest.
    grant(&v, &sel, Consumer::workspace("ws-a"), Delivery::Env, &Actor::Owner).expect("unpinned");
    let got = v.resolve_ref_env(&sel, &svc("svc-a", "ws-a")).expect("resolves");
    assert_eq!(got.value.expose(), b"k2test_R8SERVER_second_version_000", "an unpinned grant wins: latest");
}

/// Review fix 4: the done flag is an audit row (MAC'd chain), found by
/// action and actor.
#[test]
fn an_audit_note_is_found_by_action_and_actor() {
    let t = TestDir::new("r8-note");
    let v = open(&t);
    let a = Actor::System("upgrade-r8".into());
    assert!(!v.audit_has("upgrade_r8_done", &a).expect("query"));
    v.audit_note("upgrade_r8_done", &a, "granted 0").expect("note");
    assert!(v.audit_has("upgrade_r8_done", &a).expect("query"));
    assert!(!v.audit_has("upgrade_r8_done", &Actor::System("other".into())).expect("query"), "another actor's row isn't it");
}
