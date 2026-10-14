//! Garden v3 data sources [GV42]: a `source:<id>` Daemon grant on a User
//! secret (fetch, narrowed to one origin). Fakes only.

use std::sync::Arc;

use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::hosts::Origin;
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::keystore::KeyStore;
use k2_vault::*;

const SENTINEL: &str = "k2test_SOURCE_7d6c5b4a3928170615f";

fn open(t: &TestDir, ks: &Arc<FakeKeyStore>) -> LocalVault {
    LocalVault::open(t.path().join("vault"), ks.clone() as Arc<dyn KeyStore>, OpenOptions { kdf: KdfCost::Floor })
        .expect("open vault")
}

fn origin(u: &str) -> Origin {
    Origin::parse_url(u).expect("url").1
}

#[test]
fn a_source_grant_resolves_for_that_source_and_origin_only() {
    let t = TestDir::new("source-grant");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let mut r = PutRequest::new(Namespace::User, Scope::server(), "VERCEL_RO", Kind::ApiKey);
    r.hosts = vec!["api.vercel.com".into(), "api.example.com".into()];
    v.put(r, SecretBytes::from_slice(SENTINEL.as_bytes()), &Actor::Owner).expect("put");
    let grant = |v: &LocalVault, id: &str, delivery: Delivery, hosts: Option<Vec<String>>| {
        v.grant(
            GrantRequest {
                selector: Selector::user(Scope::server(), "VERCEL_RO"),
                consumer: Consumer { kind: ConsumerKind::Daemon, id: id.into() },
                delivery,
                env_name: None,
                file_name: None,
                hosts_narrowed: hosts,
                expires_at: None,
                version: None,
            },
            &Actor::System("sources".into()),
        )
    };
    grant(&v, "source:s1", Delivery::Fetch, Some(vec!["https://api.vercel.com:443".into()])).expect("source grant");
    assert_eq!(grant(&v, "source:s2", Delivery::Env, None).err().expect("env refused").code, "bad_consumer");
    assert_eq!(grant(&v, "mail", Delivery::Fetch, None).err().expect("non-source daemon refused").code, "bad_consumer");
    let sel = Selector::user(Scope::server(), "VERCEL_RO");
    let vercel = origin("https://api.vercel.com/v6/deployments");
    let got = v.resolve(&sel, Use::Fetch(&vercel), &Requester::Source { id: "s1".into() }).expect("resolves");
    assert_eq!(got.value.expose(), SENTINEL.as_bytes());
    assert!(v.resolve(&sel, Use::Fetch(&vercel), &Requester::Source { id: "s2".into() }).is_err(), "another source");
    let other = origin("https://api.example.com/x");
    assert!(v.resolve(&sel, Use::Fetch(&other), &Requester::Source { id: "s1".into() }).is_err(), "narrowed to one origin");
    assert!(v.resolve(&sel, Use::Env, &Requester::Source { id: "s1".into() }).is_err(), "fetch only");
    assert_eq!(Requester::Source { id: "s1".into() }.label(), "source:s1");
    let to = v.grants_to(&Consumer { kind: ConsumerKind::Daemon, id: "source:s1".into() }).expect("grants_to");
    assert_eq!(to.len(), 1);
    assert_eq!(to[0].1, "VERCEL_RO");

    // Rolled back: explicit grants (this one included) stop.
    v.checkpoint();
    drop(v);
    let snapshot = std::fs::read(t.path().join("vault/vault.db")).expect("snapshot");
    let v = open(&t, &ks);
    let mut r = PutRequest::new(Namespace::User, Scope::server(), "LATER", Kind::ApiKey);
    r.hosts = vec!["api.vercel.com".into()];
    v.put(r, SecretBytes::from_slice(SENTINEL.as_bytes()), &Actor::Owner).expect("put later");
    v.checkpoint();
    drop(v);
    std::fs::write(t.path().join("vault/vault.db"), &snapshot).expect("restore old");
    let _ = std::fs::remove_file(t.path().join("vault/vault.db-wal"));
    let _ = std::fs::remove_file(t.path().join("vault/vault.db-shm"));
    let v = open(&t, &ks);
    assert!(v.status().rollback);
    assert!(v.resolve(&sel, Use::Fetch(&vercel), &Requester::Source { id: "s1".into() }).is_err(), "no source fetch while rolled back");
}
