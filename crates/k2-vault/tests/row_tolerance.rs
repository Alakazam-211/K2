//! R2 (PRD vault-gaps): rows written by a newer K2 (a kind, delivery,
//! consumer or inject destination this build doesn't know) are skipped and
//! counted, not fatal. A row that isn't even well-formed stays tamper.

use std::sync::Arc;

use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::keystore::KeyStore;
use k2_vault::*;

/// Made-up enum text standing in for values a newer K2 adds.
const NEWER_KIND: &str = "zz_newer_kind";
const NEWER_DELIVERY: &str = "zz_newer_delivery";
const NEWER_CONSUMER: &str = "zz_newer_consumer";
const NEWER_DST: &str = "zz-newer-dst:a";

fn open(t: &TestDir, ks: &Arc<FakeKeyStore>) -> LocalVault {
    LocalVault::open(t.path().join("vault"), ks.clone() as Arc<dyn KeyStore>, OpenOptions { kdf: KdfCost::Floor }).expect("open vault")
}

fn session(ws: &str) -> Requester {
    Requester::Session { workspace_id: ws.into(), session_id: Some("sess-1".into()) }
}

fn put(v: &LocalVault, name: &str, kind: Kind, delivery: Delivery) -> SecretMeta {
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), name, kind);
    r.default_delivery = Some(delivery);
    r.hosts = vec!["api.example.com".into()];
    v.put(r, SecretBytes::from_slice(format!("k2test-value-{name}-123456").as_bytes()), &Actor::Owner).expect("put").meta
}

fn db(t: &TestDir) -> rusqlite::Connection {
    rusqlite::Connection::open(t.path().join("vault/vault.db")).expect("db")
}

fn names(v: &LocalVault) -> Vec<String> {
    v.list(&ListFilter::default(), &Actor::Owner).expect("list must not fail on newer rows").into_iter().map(|m| m.name).collect()
}

#[test]
fn newer_rows_are_skipped_and_counted_not_fatal() {
    let t = TestDir::new("r2");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    put(&v, "KEEP_ENV", Kind::Password, Delivery::Env);
    let kind_row = put(&v, "NEW_KIND", Kind::Generic, Delivery::Env);
    let del_row = put(&v, "NEW_DELIVERY", Kind::SshKey, Delivery::File);
    let inj_row = put(&v, "NEW_DST", Kind::ApiKey, Delivery::Fetch);
    let shared = put(&v, "SHARED", Kind::Password, Delivery::Env);
    let g = v
        .grant(
            GrantRequest {
                selector: Selector::Id(shared.id.clone()),
                consumer: Consumer::workspace("ws2"),
                delivery: Delivery::Env,
                env_name: None,
                file_name: None,
                hosts_narrowed: None,
                expires_at: None,
                version: None,
            },
            &Actor::Owner,
        )
        .expect("grant");
    drop(v);

    // What a newer K2 would write: values this build has no enum for. They
    // must stay unknown as K2 grows (a real future value like `job` or
    // `agent` becomes known, its forged row then parses and fails the MAC
    // as tamper), so they are made up and checked here.
    for (k, known) in [
        (NEWER_KIND, Kind::parse(NEWER_KIND).is_ok()),
        (NEWER_DELIVERY, Delivery::parse(NEWER_DELIVERY).is_ok()),
        (NEWER_CONSUMER, ConsumerKind::parse(NEWER_CONSUMER).is_ok()),
        (NEWER_DST, k2_vault::inject::Dst::parse(NEWER_DST).is_ok()),
    ] {
        assert!(!known, "{k} must be a value this build doesn't know");
    }
    let c = db(&t);
    c.execute("UPDATE secret SET kind = ?2 WHERE id = ?1", [&kind_row.id, NEWER_KIND]).expect("kind");
    c.execute("UPDATE secret SET default_delivery = ?2, delivery_ceiling = ?2 WHERE id = ?1", [&del_row.id, NEWER_DELIVERY]).expect("delivery");
    let inj = format!("{{\"dst\":\"{NEWER_DST}\",\"fmt\":\"{{}}\"}}");
    c.execute("UPDATE secret SET inject = ?2 WHERE id = ?1", [&inj_row.id, &inj]).expect("inject");
    c.execute("UPDATE grants SET consumer_kind = ?2, consumer_id = 'collector' WHERE id = ?1", [&g.id, NEWER_CONSUMER]).expect("grant");
    drop(c);

    let v = open(&t, &ks);
    // list works and shows only what this build understands.
    let mut got = names(&v);
    got.sort();
    assert_eq!(got, vec!["KEEP_ENV".to_string(), "SHARED".to_string()]);
    // A session's delivery works.
    let items = v.spawn_items(&session("ws1")).expect("spawn items must not fail on newer rows");
    let item_names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(item_names, vec!["KEEP_ENV", "SHARED"]);
    v.proxy_items(&session("ws1")).expect("proxy items must not fail on newer rows");
    // The newer grant is skipped: ws2 gets nothing, and listing grants works.
    assert!(v.spawn_items(&session("ws2")).expect("ws2 items").is_empty());
    assert!(v.grants(&Selector::Id(shared.id.clone()), &Actor::Owner).expect("grants").is_empty());
    // Counted, and not reported as tamper.
    let st = v.status();
    assert_eq!(st.tamper, "ok", "newer rows aren't tamper: {}", st.tamper);
    assert_eq!(st.counts.newer_secrets, 3);
    assert_eq!(st.counts.newer_grants, 1);
    // A direct lookup says why.
    let e = v.meta(&Selector::user(Scope::workspace("ws1"), "NEW_DELIVERY"), &Actor::Owner).expect_err("newer row");
    assert_eq!(e.code, "vault_needs_newer_k2");
    assert_eq!(e.status, 409);
    // A put over the newer row's name refuses rather than overwriting it.
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "NEW_KIND", Kind::Generic);
    r.default_delivery = Some(Delivery::Env);
    let e = v.put(r, SecretBytes::from_slice(b"k2test-overwrite-123456"), &Actor::Owner).expect_err("no overwrite");
    assert_eq!(e.code, "vault_needs_newer_k2");
    // Rekey can't re-MAC a row it can't read: it stops.
    let e = v.rekey(&Actor::Owner, &HumanProof::Confirmed { method: "t".into() }).expect_err("rekey stops");
    assert_eq!(e.code, "vault_needs_newer_k2");
}

#[test]
fn a_malformed_row_is_still_tamper() {
    let t = TestDir::new("r2t");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let m = put(&v, "A", Kind::ApiKey, Delivery::Fetch);
    drop(v);
    let c = db(&t);
    c.execute("UPDATE secret SET inject = 'not json' WHERE id = ?1", [&m.id]).expect("inject");
    drop(c);
    let v = open(&t, &ks);
    let e = v.list(&ListFilter::default(), &Actor::Owner).expect_err("malformed stays fatal");
    assert_eq!(e.code, "vault_tamper");
    assert_eq!(v.status().counts.newer_secrets, 0);
    assert_ne!(v.status().tamper, "ok");
}

#[test]
fn a_newer_optional_inject_field_is_ignored() {
    // R1: an older daemon reads an inject with a field it doesn't know
    // (e.g. `sign`) as the plain inject.
    let t = TestDir::new("r1");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let m = put(&v, "A", Kind::ApiKey, Delivery::Fetch);
    drop(v);
    let c = db(&t);
    c.execute(
        "UPDATE secret SET inject = '{\"dst\":\"header:Authorization\",\"fmt\":\"Bearer {}\",\"futureField\":{\"x\":1}}' WHERE id = ?1",
        [&m.id],
    )
    .expect("inject");
    drop(c);
    let v = open(&t, &ks);
    // Read, not skipped, and not tamper: the MAC check accepts the form
    // this build writes as well as the stored text.
    assert_eq!(names(&v), vec!["A".to_string()]);
    let st = v.status();
    assert_eq!(st.counts.newer_secrets, 0);
    assert_eq!(st.tamper, "ok", "{}", st.tamper);
    let got = v.meta(&Selector::Id(m.id.clone()), &Actor::Owner).expect("meta");
    assert_eq!(got.inject.expect("inject").fmt, "Bearer {}");
}
