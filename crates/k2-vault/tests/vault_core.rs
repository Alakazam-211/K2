//! V1 must-have tests (§19a, §23.7). Fakes only: FakeKeyStore, temp dirs.

use std::sync::Arc;

use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::hosts::Origin;
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::keystore::KeyStore;
use k2_vault::vault::CardAnswer;
use k2_vault::*;

const SENTINEL: &str = "k2test_SENTINEL_9f8e7d6c5b4a39281706";

fn opts() -> OpenOptions {
    OpenOptions { kdf: KdfCost::Floor }
}

fn open(t: &TestDir, ks: &Arc<FakeKeyStore>) -> LocalVault {
    LocalVault::open(t.path().join("vault"), ks.clone() as Arc<dyn KeyStore>, opts()).expect("open vault")
}

fn agent(ws: &str) -> Actor {
    Actor::Agent(AgentActor { workspace_id: ws.into(), session_id: Some("sess-1".into()), shell: false, vault_manage: false })
}

fn put_user(v: &LocalVault, scope: Scope, name: &str, value: &str, hosts: &[&str], actor: &Actor) -> PutOutcome {
    let mut r = PutRequest::new(Namespace::User, scope, name, Kind::ApiKey);
    r.hosts = hosts.iter().map(|s| s.to_string()).collect();
    v.put(r, SecretBytes::from_slice(value.as_bytes()), actor).expect("put")
}

fn origin(u: &str) -> Origin {
    Origin::parse_url(u).expect("url").1
}

fn session(ws: &str) -> Requester {
    Requester::Session { workspace_id: ws.into(), session_id: Some("sess-1".into()) }
}

fn file_bytes(t: &TestDir) -> Vec<u8> {
    let mut all = Vec::new();
    for name in ["vault.db", "vault.db-wal", "vault.db-shm"] {
        if let Ok(b) = std::fs::read(t.path().join("vault").join(name)) {
            all.extend(b);
        }
    }
    all
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn round_trip_every_kind_and_value_is_ciphertext_on_disk() {
    let t = TestDir::new("rt");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    for (i, k) in Kind::ALL.iter().enumerate() {
        let name = format!("K{i}");
        let val = format!("{SENTINEL}-{i}");
        let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), &name, *k);
        r.default_delivery = Some(Delivery::Env);
        v.put(r, SecretBytes::from_slice(val.as_bytes()), &Actor::Owner).unwrap_or_else(|e| panic!("{k}: {e}"));
        let got = v
            .resolve(&Selector::user(Scope::workspace("ws1"), &name), Use::Env, &session("ws1"))
            .unwrap_or_else(|e| panic!("{k}: {e}"));
        assert_eq!(got.value.expose(), val.as_bytes(), "{k}");
    }
    v.checkpoint();
    let disk = file_bytes(&t);
    assert!(!contains(&disk, SENTINEL.as_bytes()), "plaintext must never reach the file");
}

#[test]
fn reopen_keeps_values() {
    let t = TestDir::new("reopen");
    let ks = Arc::new(FakeKeyStore::new());
    {
        let v = open(&t, &ks);
        put_user(&v, Scope::workspace("ws1"), "A", SENTINEL, &["api.stripe.com"], &Actor::Owner);
    }
    let v = open(&t, &ks);
    let r = v
        .resolve(&Selector::user(Scope::workspace("ws1"), "A"), Use::Fetch(&origin("https://api.stripe.com/v1")), &session("ws1"))
        .expect("resolve");
    assert_eq!(r.value.expose(), SENTINEL.as_bytes());
    assert_eq!(v.status().tamper, "ok");
}

#[test]
fn moved_ciphertext_fails_closed() {
    let t = TestDir::new("aad");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let a = put_user(&v, Scope::workspace("ws1"), "A", "value-a-123456", &["api.stripe.com"], &Actor::Owner);
    let b = put_user(&v, Scope::workspace("ws1"), "B", "value-b-123456", &["api.stripe.com"], &Actor::Owner);
    drop(v);
    // Copy A's ciphertext (and its row MAC) into B's row.
    let conn = rusqlite::Connection::open(t.path().join("vault/vault.db")).expect("db");
    conn.execute(
        "UPDATE secret_version SET nonce = (SELECT nonce FROM secret_version WHERE secret_id = ?1),
            ciphertext = (SELECT ciphertext FROM secret_version WHERE secret_id = ?1),
            fingerprint = (SELECT fingerprint FROM secret_version WHERE secret_id = ?1),
            row_mac = (SELECT row_mac FROM secret_version WHERE secret_id = ?1) WHERE secret_id = ?2",
        rusqlite::params![a.meta.id, b.meta.id],
    )
    .expect("swap");
    drop(conn);
    let v = open(&t, &ks);
    let e = v
        .resolve(&Selector::user(Scope::workspace("ws1"), "B"), Use::Fetch(&origin("https://api.stripe.com/")), &session("ws1"))
        .err()
        .expect("must fail");
    assert_eq!(e.code, "vault_tamper");
}

#[test]
fn edited_secret_row_fails_closed() {
    let t = TestDir::new("rowmac");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    put_user(&v, Scope::workspace("ws1"), "A", SENTINEL, &["api.stripe.com"], &Actor::Owner);
    drop(v);
    let conn = rusqlite::Connection::open(t.path().join("vault/vault.db")).expect("db");
    conn.execute("UPDATE secret SET allowed_hosts = '[\"https://evil.example:443\"]'", []).expect("edit");
    drop(conn);
    let v = open(&t, &ks);
    assert!(v.status().tamper.contains("integrity"), "status shows tamper: {}", v.status().tamper);
    let e = v
        .resolve(&Selector::user(Scope::workspace("ws1"), "A"), Use::Fetch(&origin("https://evil.example/")), &session("ws1"))
        .err()
        .expect("refused");
    assert_eq!(e.code, "vault_tamper");
}

#[test]
fn edited_or_deleted_audit_row_breaks_the_chain() {
    let t = TestDir::new("audit");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    put_user(&v, Scope::workspace("ws1"), "A", SENTINEL, &["api.stripe.com"], &Actor::Owner);
    put_user(&v, Scope::workspace("ws1"), "B", SENTINEL, &["api.stripe.com"], &Actor::Owner);
    drop(v);
    let conn = rusqlite::Connection::open(t.path().join("vault/vault.db")).expect("db");
    conn.execute("UPDATE audit SET actor = 'someone-else' WHERE seq = 2", []).expect("edit");
    drop(conn);
    let v = open(&t, &ks);
    assert!(v.status().tamper.contains("changed at row 2"), "{}", v.status().tamper);
    drop(v);

    let t2 = TestDir::new("audit2");
    let ks2 = Arc::new(FakeKeyStore::new());
    let v = open(&t2, &ks2);
    put_user(&v, Scope::workspace("ws1"), "A", SENTINEL, &["api.stripe.com"], &Actor::Owner);
    drop(v);
    let conn = rusqlite::Connection::open(t2.path().join("vault/vault.db")).expect("db");
    let max: i64 = conn.query_row("SELECT MAX(seq) FROM audit", [], |r| r.get(0)).expect("max");
    conn.execute("DELETE FROM audit WHERE seq = ?1", [max]).expect("truncate tail");
    drop(conn);
    let v = open(&t2, &ks2);
    assert!(v.status().tamper != "ok", "a truncated audit tail must show: {}", v.status().tamper);
}

#[test]
fn older_vault_db_is_a_rollback_and_stops_explicit_grants() {
    let t = TestDir::new("rollback");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    put_user(&v, Scope::server(), "SHARED", SENTINEL, &["api.stripe.com"], &Actor::Owner);
    put_user(&v, Scope::workspace("ws1"), "OWN", SENTINEL, &["api.stripe.com"], &Actor::Owner);
    v.grant(
        GrantRequest {
            selector: Selector::user(Scope::server(), "SHARED"),
            consumer: Consumer::workspace("ws1"),
            delivery: Delivery::Fetch,
            env_name: None,
            file_name: None,
            hosts_narrowed: None,
            expires_at: None,
            version: None,
        },
        &Actor::Owner,
    )
    .expect("grant");
    v.checkpoint();
    drop(v);
    let snapshot = std::fs::read(t.path().join("vault/vault.db")).expect("snapshot");
    let v = open(&t, &ks);
    // A destroy after the snapshot …
    v.destroy(&Selector::user(Scope::workspace("ws1"), "OWN"), &Actor::Owner).expect("destroy");
    v.checkpoint();
    drop(v);
    // … then the old file comes back.
    std::fs::write(t.path().join("vault/vault.db"), &snapshot).expect("restore old");
    let _ = std::fs::remove_file(t.path().join("vault/vault.db-wal"));
    let _ = std::fs::remove_file(t.path().join("vault/vault.db-shm"));
    let v = open(&t, &ks);
    let st = v.status();
    assert!(st.rollback, "rollback detected");
    let o = origin("https://api.stripe.com/x");
    let e = v
        .resolve(&Selector::user(Scope::server(), "SHARED"), Use::Fetch(&o), &session("ws1"))
        .err()
        .expect("explicit grant stops");
    assert!(e.status == 403, "{e}");
    // Implicit own-workspace use keeps working (status stays red).
    v.resolve(&Selector::user(Scope::workspace("ws1"), "OWN"), Use::Fetch(&o), &session("ws1")).expect("implicit ok");
    // User mutations wait for the owner.
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "NEW", Kind::ApiKey);
    r.hosts = vec!["api.stripe.com".into()];
    assert_eq!(v.put(r, SecretBytes::from_slice(b"abcdefgh"), &Actor::Owner).err().expect("blocked").code, "vault_rollback");
    // The owner acknowledges with a confirm.
    v.acknowledge_rollback(&Actor::Owner, &HumanProof::Confirmed { method: "test".into() }).expect("ack");
    assert!(!v.status().rollback);
    v.resolve(&Selector::user(Scope::server(), "SHARED"), Use::Fetch(&o), &session("ws1")).expect("grant works again");
}

#[test]
fn crash_between_commit_and_anchor_opens_cleanly() {
    let t = TestDir::new("crash");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    *ks.fail_set_anchor.lock() = true;
    put_user(&v, Scope::workspace("ws1"), "A", SENTINEL, &["api.stripe.com"], &Actor::Owner);
    *ks.fail_set_anchor.lock() = false;
    drop(v);
    let v = open(&t, &ks);
    let st = v.status();
    assert!(!st.rollback, "gen+1 is accepted and repaired");
    assert_eq!(ks.anchor().expect("a").expect("some").gen, st.gen);
}

#[test]
fn another_vault_is_refused() {
    let t = TestDir::new("subst");
    let ks = Arc::new(FakeKeyStore::new());
    drop(open(&t, &ks));
    // A vault made with another box key, dropped in place.
    let other = TestDir::new("subst-other");
    let ks_other = Arc::new(FakeKeyStore::new());
    drop(open(&other, &ks_other));
    std::fs::copy(other.path().join("vault/vault.db"), t.path().join("vault/vault.db")).expect("swap");
    let _ = std::fs::remove_file(t.path().join("vault/vault.db-wal"));
    let e = LocalVault::open(t.path().join("vault"), ks.clone() as Arc<dyn KeyStore>, opts()).err().expect("refused");
    assert_eq!(e.code, "vault_substituted");
    // Same box key, but another vault id than the anchor remembers.
    let t3 = TestDir::new("subst3");
    drop(open(&t3, &ks));
    std::fs::copy(t3.path().join("vault/vault.db"), t.path().join("vault/vault.db")).expect("swap");
    ks.force_anchor(Some(Anchor { vault_id: Some("00000000-0000-4000-8000-000000000000".into()), gen: 1, audit_seq: None, audit_mac: None }));
    let e = LocalVault::open(t.path().join("vault"), ks.clone() as Arc<dyn KeyStore>, opts()).err().expect("refused");
    assert_eq!(e.code, "vault_substituted");
}

#[test]
fn destroy_leaves_no_ciphertext_on_disk() {
    let t = TestDir::new("destroy");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let o = put_user(&v, Scope::workspace("ws1"), "A", SENTINEL, &["api.stripe.com"], &Actor::Owner);
    v.checkpoint();
    let ct: Vec<u8> = {
        let conn = rusqlite::Connection::open(t.path().join("vault/vault.db")).expect("db");
        conn.query_row("SELECT ciphertext FROM secret_version WHERE secret_id = ?1", [&o.meta.id], |r| r.get(0)).expect("ct")
    };
    assert!(contains(&file_bytes(&t), &ct));
    v.destroy(&Selector::user(Scope::workspace("ws1"), "A"), &Actor::Owner).expect("destroy");
    v.checkpoint();
    assert!(!contains(&file_bytes(&t), &ct), "destroyed ciphertext must be gone from vault.db and its WAL");
    let e = v
        .resolve(&Selector::user(Scope::workspace("ws1"), "A"), Use::Fetch(&origin("https://api.stripe.com/")), &session("ws1"))
        .err()
        .expect("gone");
    assert_eq!(e.code, "secret_not_active");
}

#[test]
fn fetch_host_binding_and_implicit_grants() {
    let t = TestDir::new("fetch");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    put_user(&v, Scope::workspace("ws1"), "STRIPE", SENTINEL, &["api.stripe.com"], &Actor::Owner);
    let sel = Selector::user(Scope::workspace("ws1"), "STRIPE");
    let ok = v.resolve(&sel, Use::Fetch(&origin("https://api.stripe.com/v1/charges")), &session("ws1")).expect("ok");
    assert_eq!(ok.inject.dst.render(), "header:Authorization", "Stripe preset");
    for bad in ["https://evil.example/", "http://api.stripe.com/", "https://api.stripe.com:8443/"] {
        let e = v.resolve(&sel, Use::Fetch(&origin(bad)), &session("ws1")).err().expect(bad);
        assert_eq!(e.code, "host_not_allowed", "{bad}");
    }
    // Another workspace can't use it.
    let e = v.resolve(&sel, Use::Fetch(&origin("https://api.stripe.com/")), &session("ws2")).err().expect("other ws");
    assert_eq!(e.status, 403);
    // Fetch-only secrets aren't handed out as env.
    let e = v.resolve(&sel, Use::Env, &session("ws1")).err().expect("env refused");
    assert_eq!(e.code, "delivery_not_granted");
    // Denies are audited, never with the value.
    v.flush_rollups();
    let rows = v.audit(&AuditQuery::default(), &Actor::Owner).expect("audit");
    assert!(rows.iter().any(|r| r.outcome.starts_with("deny:host_not_allowed")));
    assert!(rows.iter().any(|r| r.action == "fetch" && r.outcome == "ok"));
    let all = serde_json::to_string(&rows).expect("json");
    assert!(!all.contains(SENTINEL));
}

#[test]
fn agents_cannot_widen_a_person_given_secret_but_can_tighten() {
    let t = TestDir::new("loosen");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    put_user(&v, Scope::workspace("ws1"), "STRIPE", SENTINEL, &["api.stripe.com"], &Actor::Owner);
    let sel = Selector::user(Scope::workspace("ws1"), "STRIPE");
    let a = agent("ws1");
    let widen = PolicyPatch { add_hosts: vec!["evil.example".into()], ..Default::default() };
    assert_eq!(v.set_policy(&sel, widen.clone(), &a).err().expect("no").code, "vault_owner_confirm_required");
    let raise = PolicyPatch { default_delivery: Some(Delivery::Env), ..Default::default() };
    assert_eq!(v.set_policy(&sel, raise, &a).err().expect("no").code, "vault_owner_confirm_required");
    let inject = PolicyPatch { inject: Some(k2_vault::inject::Inject::header("X-Leak", "{}")), ..Default::default() };
    assert_eq!(v.set_policy(&sel, inject, &a).err().expect("no").code, "vault_owner_confirm_required");
    let api = GrantRequest {
        selector: sel.clone(),
        consumer: Consumer::session("sess-1"),
        delivery: Delivery::Api,
        env_name: None,
        file_name: None,
        hosts_narrowed: None,
        expires_at: None,
        version: None,
    };
    assert!(v.grant(api, &a).is_err(), "no api grant on a person's secret");
    // Tightening is open.
    v.set_policy(&sel, PolicyPatch { description: Some("narrow".into()), ..Default::default() }, &a).expect("describe");
    v.disable(&sel, &a).expect("disable");
    assert_eq!(v.enable(&sel, &a).err().expect("re-enable is widening").code, "vault_owner_confirm_required");
    // The owner may widen (KV16; VS1 deferred) and it's audited.
    v.enable(&sel, &Actor::Owner).expect("owner enables");
    v.set_policy(&sel, widen, &Actor::Owner).expect("owner widens");
    let rows = v.audit(&AuditQuery::default(), &Actor::Owner).expect("audit");
    assert!(rows.iter().any(|r| r.action == "policy" && r.outcome == "ok:widened"));
    // Agents never add wildcards.
    let wc = PolicyPatch { add_hosts: vec!["*.stripe.com".into()], ..Default::default() };
    assert_eq!(v.set_policy(&sel, wc, &a).err().expect("no").code, "wildcard_not_allowed");
}

#[test]
fn agent_owned_secret_can_be_widened_by_its_workspace() {
    let t = TestDir::new("own");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let a = agent("ws1");
    let o = put_user(&v, Scope::workspace("ws1"), "WEBHOOK", "k2test_generated_1234567", &["hooks.example.com"], &a);
    assert!(!o.meta.human_given);
    let sel = Selector::user(Scope::workspace("ws1"), "WEBHOOK");
    v.set_policy(&sel, PolicyPatch { default_delivery: Some(Delivery::Env), ..Default::default() }, &a).expect("own");
    let r = v.resolve(&sel, Use::Env, &session("ws1")).expect("env now");
    assert_eq!(r.env_name.as_deref(), Some("WEBHOOK"));
    // Another workspace's agent can't touch it.
    assert!(v.set_policy(&sel, PolicyPatch { description: Some("x".into()), ..Default::default() }, &agent("ws2")).is_err());
}

#[test]
fn agent_rotate_of_a_person_given_secret_notifies_and_keeps_destination() {
    let t = TestDir::new("vs11");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "DATABASE_URL", Kind::Dsn);
    r.default_delivery = Some(Delivery::Env);
    v.put(r, SecretBytes::from_slice(b"postgres://u:p@db.example.com:5432/app"), &Actor::Owner).expect("put");
    let sel = Selector::user(Scope::workspace("ws1"), "DATABASE_URL");
    let a = agent("ws1");
    let e = v.rotate(&sel, SecretBytes::from_slice(b"postgres://u:p@attacker.example:5432/app"), &a).err().expect("refused");
    assert_eq!(e.code, "vault_owner_confirm_required");
    let ok = v.rotate(&sel, SecretBytes::from_slice(b"postgres://u:NEW@db.example.com:5432/app"), &a).expect("same host ok");
    assert!(ok.owner_notice, "owner gets a notice with undo");
    assert!(ok.meta.human_given);
    // Owner undo: roll back to v1.
    v.rollback_to(&sel, 1, &Actor::Owner).expect("undo");
    let r = v.resolve(&sel, Use::Env, &session("ws1")).expect("resolve");
    assert_eq!(r.value.expose(), b"postgres://u:p@db.example.com:5432/app");
}

#[test]
fn it_agent_shares_server_secrets_with_other_workspaces_only() {
    let t = TestDir::new("it");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let it = Actor::Agent(AgentActor { workspace_id: "it".into(), session_id: Some("s".into()), shell: false, vault_manage: true });
    put_user(&v, Scope::server(), "OPENAI", "k2test-abcdefghijklmnop", &["api.openai.com"], &Actor::Owner);
    let g = |c: Consumer| GrantRequest {
        selector: Selector::user(Scope::server(), "OPENAI"),
        consumer: c,
        delivery: Delivery::Fetch,
        env_name: None,
        file_name: None,
        hosts_narrowed: None,
        expires_at: None,
        version: None,
    };
    v.grant(g(Consumer::workspace("sales")), &it).expect("other workspace");
    assert_eq!(v.grant(g(Consumer::workspace("it")), &it).err().expect("self").code, "vault_self_grant");
    assert_eq!(v.grant(g(Consumer::all_workspaces()), &it).err().expect("all").code, "vault_self_grant");
    v.resolve(&Selector::user(Scope::server(), "OPENAI"), Use::Fetch(&origin("https://api.openai.com/v1/models")), &session("sales"))
        .expect("sales uses it");
    // A plain agent can't create server secrets.
    let mut r = PutRequest::new(Namespace::User, Scope::server(), "X", Kind::ApiKey);
    r.hosts = vec!["api.x.ai".into()];
    assert_eq!(v.put(r, SecretBytes::from_slice(b"k2test-123456789"), &agent("ws1")).err().expect("no").code, "vault_manage_required");
}

#[test]
fn env_and_file_names_are_checked_on_grants() {
    let t = TestDir::new("names");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    put_user(&v, Scope::server(), "K", "value-123456789", &["api.example.com"], &Actor::Owner);
    let mk = |env: Option<&str>, file: Option<&str>, d: Delivery| GrantRequest {
        selector: Selector::user(Scope::server(), "K"),
        consumer: Consumer::workspace("ws1"),
        delivery: d,
        env_name: env.map(String::from),
        file_name: file.map(String::from),
        hosts_narrowed: None,
        expires_at: None,
        version: None,
    };
    assert_eq!(v.grant(mk(Some("NODE_OPTIONS"), None, Delivery::Env), &Actor::Owner).err().expect("no").code, "env_name_not_allowed");
    assert_eq!(v.grant(mk(Some("LD_PRELOAD"), None, Delivery::Env), &Actor::Owner).err().expect("no").code, "env_name_not_allowed");
    assert_eq!(v.grant(mk(None, Some("../x"), Delivery::File), &Actor::Owner).err().expect("no").code, "bad_file_name");
    v.grant(mk(Some("MY_KEY"), None, Delivery::Env), &Actor::Owner).expect("ok");
    let items = v.spawn_items(&session("ws1")).expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].env_name.as_deref(), Some("MY_KEY"));
}

#[test]
fn explicit_grant_env_name_beats_the_implicit_own_workspace_candidate() {
    let t = TestDir::new("shadow");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    // A workspace's own password (default delivery env) with a name that
    // isn't a valid env name, then an explicit env grant to that same
    // workspace naming the variable.
    let r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "mini-k2n1-admin-pw", Kind::Password);
    v.put(r, SecretBytes::from_slice(b"k2test-admin-pw-123456"), &Actor::Owner).expect("put");
    let sel = Selector::user(Scope::workspace("ws1"), "mini-k2n1-admin-pw");
    v.grant(
        GrantRequest {
            selector: sel.clone(),
            consumer: Consumer::workspace("ws1"),
            delivery: Delivery::Env,
            env_name: Some("MINI_K2N1_PW".into()),
            file_name: None,
            hosts_narrowed: None,
            expires_at: None,
            version: None,
        },
        &Actor::Owner,
    )
    .expect("grant");
    let r = v.resolve(&sel, Use::Env, &session("ws1")).expect("resolve");
    assert_eq!(r.env_name.as_deref(), Some("MINI_K2N1_PW"));
    let items = v.spawn_items(&session("ws1")).expect("items");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].env_name.as_deref(), Some("MINI_K2N1_PW"));
}

#[test]
fn unnamed_env_delivery_derives_a_valid_env_name_and_still_denies() {
    let t = TestDir::new("derive");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    for name in ["mini-k2n1-admin-pw", "k2-token"] {
        let r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), name, Kind::Password);
        v.put(r, SecretBytes::from_slice(b"k2test-admin-pw-123456"), &Actor::Owner).expect("put");
    }
    // Own secret, nothing names the variable: derived, not the raw name.
    let r = v.resolve(&Selector::user(Scope::workspace("ws1"), "mini-k2n1-admin-pw"), Use::Env, &session("ws1")).expect("resolve");
    assert_eq!(r.env_name.as_deref(), Some("MINI_K2N1_ADMIN_PW"));
    // A derived name that's denied (K2_*) is never delivered at spawn.
    let r = v.resolve(&Selector::user(Scope::workspace("ws1"), "k2-token"), Use::Env, &session("ws1")).expect("resolve");
    let denied = r.env_name.expect("env name");
    assert_eq!(denied, "K2_TOKEN");
    assert_eq!(delivery::validate_env_name(&denied).expect_err("denied").code, "env_name_not_allowed");
    let items = v.spawn_items(&session("ws1")).expect("items");
    let names: Vec<_> = items.iter().map(|i| (i.name.as_str(), i.env_name.as_deref())).collect();
    assert_eq!(names, vec![("mini-k2n1-admin-pw", Some("MINI_K2N1_ADMIN_PW"))]);
    // An env grant without --env-name stores the derived name.
    let g = v
        .grant(
            GrantRequest {
                selector: Selector::user(Scope::workspace("ws1"), "mini-k2n1-admin-pw"),
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
    assert_eq!(g.env_name.as_deref(), Some("MINI_K2N1_ADMIN_PW"));
}

#[test]
fn reveal_needs_a_confirm_and_never_shows_system_values() {
    let t = TestDir::new("reveal");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    put_user(&v, Scope::workspace("ws1"), "A", SENTINEL, &["api.stripe.com"], &Actor::Owner);
    let sel = Selector::user(Scope::workspace("ws1"), "A");
    assert_eq!(v.reveal(&sel, &Actor::Owner, &HumanProof::None).err().expect("no").code, "vault_confirm_required");
    let tok = v.confirm.mint("reveal", None, "owner", "test");
    let proof = v.confirm.take(&tok, "reveal", None, "owner").expect("proof");
    assert_eq!(v.reveal(&sel, &Actor::Owner, &proof).expect("ok").expose(), SENTINEL.as_bytes());
    assert!(v.reveal(&sel, &agent("ws1"), &proof).is_err());
    let mut sys = PutRequest::new(Namespace::System, Scope::engine("stalwart"), "mailsec_admin_0123", Kind::Password);
    sys.default_delivery = Some(Delivery::None);
    v.put(sys, SecretBytes::from_slice(b"admin-pass-xyz"), &Actor::System("mail".into())).expect("system put");
    let ssel = Selector::system(Scope::engine("stalwart"), "mailsec_admin_0123");
    assert_eq!(v.reveal(&ssel, &Actor::Owner, &proof).err().expect("never").code, "vault_system_namespace");
    // System values: daemon only.
    assert!(v.resolve(&ssel, Use::Api, &session("ws1")).is_err());
    assert_eq!(v.resolve(&ssel, Use::System, &Requester::System("mail".into())).expect("daemon").value.expose(), b"admin-pass-xyz");
    // Owners see system metadata, agents don't.
    let owner_list = v.list(&ListFilter { include_system: true, ..Default::default() }, &Actor::Owner).expect("list");
    assert!(owner_list.iter().any(|m| m.namespace == Namespace::System));
    let agent_list = v.list(&ListFilter { include_system: true, ..Default::default() }, &agent("ws1")).expect("list");
    assert!(agent_list.iter().all(|m| m.namespace == Namespace::User));
}

#[test]
fn confirm_password_lockout() {
    let t = TestDir::new("confirmpw");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    assert_eq!(v.check_confirm_password("whatever-123", "owner").err().expect("none set").code, "no_confirm_password");
    assert!(v.set_confirm_password("correct horse battery", &agent("ws1")).is_err(), "agents never set it");
    v.set_confirm_password("correct horse battery", &Actor::Owner).expect("set");
    v.check_confirm_password("correct horse battery", "owner").expect("ok");
    for _ in 0..5 {
        assert_eq!(v.check_confirm_password("wrong password!", "owner").err().expect("wrong").code, "confirm_failed");
    }
    assert_eq!(v.check_confirm_password("correct horse battery", "owner").err().expect("locked").code, "confirm_locked");
}

#[test]
fn recovery_key_restores_on_a_fresh_box_and_neither_key_fails_first() {
    let t = TestDir::new("recovery");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    put_user(&v, Scope::workspace("ws1"), "A", SENTINEL, &["api.stripe.com"], &Actor::Owner);
    assert_eq!(v.status().recovery, "unconfirmed");
    assert!(v.recovery_begin(&agent("ws1")).is_err(), "agents never see the recovery phrase");
    let ch = v.recovery_begin(&Actor::Owner).expect("begin");
    let phrase = ch.phrase.expose_str().expect("utf8").to_string();
    let words: Vec<&str> = phrase.split(' ').collect();
    assert_eq!(words.len(), 24);
    let bad = vec!["zoo".to_string(), "zoo".into(), "zoo".into()];
    assert!(v.recovery_confirm(&ch.id, &bad, &Actor::Owner).is_err());
    let ch = v.recovery_begin(&Actor::Owner).expect("begin again");
    let phrase = ch.phrase.expose_str().expect("utf8").to_string();
    let words: Vec<String> = phrase.split(' ').map(String::from).collect();
    let back = vec![words[2].clone(), words[10].clone(), words[19].clone()];
    v.recovery_confirm(&ch.id, &back, &Actor::Owner).expect("confirm");
    assert_eq!(v.status().recovery, "confirmed");
    v.checkpoint();
    drop(v);
    // New box: copy vault.db only; a different (empty) key store.
    let new_box = TestDir::new("recovery-new");
    std::fs::create_dir_all(new_box.path().join("vault")).expect("dir");
    std::fs::copy(t.path().join("vault/vault.db"), new_box.path().join("vault/vault.db")).expect("copy");
    let ks2 = Arc::new(FakeKeyStore::new());
    // Without a key, open fails before changing anything.
    let before = std::fs::read(new_box.path().join("vault/vault.db")).expect("read");
    let e = LocalVault::open(new_box.path().join("vault"), ks2.clone() as Arc<dyn KeyStore>, opts()).err().expect("no key");
    assert!(e.code == "vault_key_missing" || e.code == "vault_substituted", "{e}");
    assert_eq!(std::fs::read(new_box.path().join("vault/vault.db")).expect("read"), before, "nothing overwritten");
    // The wrong phrase can't open it either.
    let wrong = k2_vault::recovery::RecoverySeed::generate().identity();
    let ks3 = Arc::new(FakeKeyStore::new());
    assert!(LocalVault::adopt_with_identity(new_box.path().join("vault"), ks3.clone() as Arc<dyn KeyStore>, &wrong, "recovery", opts()).is_err());
    let id = k2_vault::recovery::RecoverySeed::from_phrase(&phrase).expect("phrase").identity();
    let v2 = LocalVault::adopt_with_identity(new_box.path().join("vault"), ks2.clone() as Arc<dyn KeyStore>, &id, "recovery", opts())
        .expect("restore with the recovery key");
    let r = v2
        .resolve(&Selector::user(Scope::workspace("ws1"), "A"), Use::Fetch(&origin("https://api.stripe.com/")), &session("ws1"))
        .expect("values back");
    assert_eq!(r.value.expose(), SENTINEL.as_bytes());
}

#[test]
fn rekey_keeps_values_and_chain() {
    let t = TestDir::new("rekey");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    put_user(&v, Scope::workspace("ws1"), "A", SENTINEL, &["api.stripe.com"], &Actor::Owner);
    assert!(v.rekey(&Actor::Owner, &HumanProof::None).is_err());
    v.rekey(&Actor::Owner, &HumanProof::Confirmed { method: "t".into() }).expect("rekey");
    drop(v);
    let v = open(&t, &ks);
    assert_eq!(v.status().tamper, "ok");
    let r = v
        .resolve(&Selector::user(Scope::workspace("ws1"), "A"), Use::Fetch(&origin("https://api.stripe.com/")), &session("ws1"))
        .expect("resolve");
    assert_eq!(r.value.expose(), SENTINEL.as_bytes());
    v.kek_rotate(&Actor::Owner, &HumanProof::Confirmed { method: "t".into() }).expect("kek rotate");
    drop(v);
    let v = open(&t, &ks);
    assert_eq!(v.status().tamper, "ok");
}

#[test]
fn secret_cards_are_for_people() {
    let t = TestDir::new("cards");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let mk = |id: &str| k2_vault::vault::SecretRequest {
        card_id: id.into(),
        conversation: "conv-1".into(),
        session_id: Some("sess-1".into()),
        workspace_id: "ws1".into(),
        requested_by: "agent:ws1/sess-1".into(),
        name: "STRIPE_KEY".into(),
        kind: Kind::ApiKey,
        proposed_scope: "workspace".into(),
        proposed_hosts: vec!["api.stripe.com".into()],
        proposed_delivery: Delivery::Fetch,
        prompt: "Paste your Stripe restricted key".into(),
        ttl_secs: None,
        status: String::new(),
        answered_by: None,
        secret_id: None,
        created_at: String::new(),
        answered_at: None,
    };
    v.request_create(mk("card-0123456789abcdef")).expect("card");
    let ans = CardAnswer::default();
    let e = v.request_answer("card-0123456789abcdef", SecretBytes::from_slice(SENTINEL.as_bytes()), &ans, &agent("ws1")).err().expect("agent");
    assert_eq!(e.code, "secret_card_human_only");
    let member = Actor::Login { username: "mia".into(), role: Role::Member };
    let srv = CardAnswer { scope: Some("server".into()), ..Default::default() };
    assert!(v.request_answer("card-0123456789abcdef", SecretBytes::from_slice(SENTINEL.as_bytes()), &srv, &member).is_err());
    let out = v.request_answer("card-0123456789abcdef", SecretBytes::from_slice(SENTINEL.as_bytes()), &ans, &member).expect("member answers");
    assert_eq!(out.reference, "k2v://ws/ws1/STRIPE_KEY");
    assert!(out.meta.human_given);
    let e = v.request_answer("card-0123456789abcdef", SecretBytes::from_slice(b"another"), &ans, &Actor::Owner).err().expect("second");
    assert_eq!(e.code, "card_not_pending");
    // Agent proposals can't carry wildcards.
    let mut wc = mk("card-wildcard-0123456");
    wc.proposed_hosts = vec!["*.stripe.com".into()];
    assert_eq!(v.request_create(wc).err().expect("no").code, "wildcard_not_allowed");
    // Mismatched provider needs the second click.
    let mut off = mk("card-mismatch-0123456");
    off.proposed_hosts = vec!["api-stripe.example".into()];
    off.name = "OTHER".into();
    v.request_create(off).expect("card");
    let e = v.request_answer("card-mismatch-0123456", SecretBytes::from_slice(&[k2_vault::inject::preset("stripe").expect("stripe preset").prefixes[0].as_bytes(), b"abcdef123456"].concat()), &ans, &Actor::Owner).err().expect("red");
    assert_eq!(e.code, "card_host_mismatch");
    let yes = CardAnswer { confirmed_warnings: true, ..Default::default() };
    v.request_answer("card-mismatch-0123456", SecretBytes::from_slice(&[k2_vault::inject::preset("stripe").expect("stripe preset").prefixes[0].as_bytes(), b"abcdef123456"].concat()), &yes, &Actor::Owner).expect("confirmed");
    // Guests: chat scope, fetch only, a host required.
    let mut g = mk("card-guest-0123456789");
    g.name = "GUEST_KEY".into();
    v.request_create(g).expect("card");
    let guest = Actor::Guest { username: "gail".into(), workspace_id: "ws1".into() };
    let env = CardAnswer { delivery: Some(Delivery::Env), confirmed_warnings: true, ..Default::default() };
    assert_eq!(v.request_answer("card-guest-0123456789", SecretBytes::from_slice(b"guestvalue123"), &env, &guest).err().expect("no").code, "guest_fetch_only");
    let out = v.request_answer("card-guest-0123456789", SecretBytes::from_slice(b"guestvalue123"), &ans, &guest).expect("guest");
    assert_eq!(out.meta.scope.kind, ScopeKind::Session);
    assert_eq!(out.meta.delivery_ceiling, Delivery::Fetch);
    assert!(out.meta.expires_at.is_some());
    // Values never in the request rows or audit.
    let reqs = serde_json::to_string(&v.requests(None).expect("reqs")).expect("json");
    assert!(!reqs.contains(SENTINEL));
}

#[test]
fn idempotent_put_adds_no_version_on_retry() {
    let t = TestDir::new("idem");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let mk = || {
        let mut r = PutRequest::new(Namespace::System, Scope::engine("postgres"), "dbsec_ws1_agent", Kind::Password);
        r.idempotent = true;
        r.default_delivery = Some(Delivery::None);
        r
    };
    let a = v.put(mk(), SecretBytes::from_slice(b"pw-123456"), &Actor::System("adopt".into())).expect("first");
    let b = v.put(mk(), SecretBytes::from_slice(b"pw-123456"), &Actor::System("adopt".into())).expect("retry");
    assert!(a.created && !b.new_version);
    assert_eq!(b.meta.latest_version, 1);
    let c = v.put(mk(), SecretBytes::from_slice(b"pw-different"), &Actor::System("adopt".into())).expect("change");
    assert_eq!(c.meta.latest_version, 2);
}

#[test]
fn redactor_covers_user_values() {
    let t = TestDir::new("redact");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    put_user(&v, Scope::workspace("ws1"), "A", SENTINEL, &["api.stripe.com"], &Actor::Owner);
    let r = v.redactor();
    assert_eq!(r.redact_str(&format!("x {SENTINEL} y")), "x [k2v:A] y");
}

#[test]
fn placeholders_resolve_by_hash_and_die_with_the_session() {
    let t = TestDir::new("ph");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let o = put_user(&v, Scope::workspace("ws1"), "A", SENTINEL, &["api.anthropic.com"], &Actor::Owner);
    let tok = v.placeholder_mint(&o.meta.id, "ws1", "sess-1", "anthropic", None).expect("mint");
    assert_eq!(
        v.placeholder_lookup(&tok).expect("lookup"),
        k2_vault::Placeholder {
            secret_id: o.meta.id.clone(),
            workspace_id: "ws1".into(),
            session_id: "sess-1".into(),
            provider: "anthropic".into(),
        }
    );
    assert!(!contains(&file_bytes(&t), tok.as_bytes()), "only the hash is stored");
    v.placeholders_drop_session("sess-1").expect("drop");
    assert_eq!(v.placeholder_lookup(&tok).err().expect("gone").code, "placeholder_unknown");
}

#[test]
fn proxy_items_list_only_proxy_deliveries_for_that_session() {
    let t = TestDir::new("proxy-items");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let put = |name: &str, d: Delivery| {
        let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), name, Kind::ApiKey);
        r.hosts = vec!["api.anthropic.com".into()];
        r.default_delivery = Some(d);
        v.put(r, SecretBytes::from_slice(format!("{SENTINEL}-{name}").as_bytes()), &Actor::Owner).expect("put")
    };
    put("ON_PROXY", Delivery::Proxy);
    put("ON_FETCH", Delivery::Fetch);
    put("ON_ENV", Delivery::Env);
    let items = v.proxy_items(&session("ws1")).expect("items");
    assert_eq!(items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), vec!["ON_PROXY"]);
    assert_eq!(items[0].hosts, vec!["https://api.anthropic.com:443".to_string()]);
    assert!(v.proxy_items(&session("ws2")).expect("items").is_empty(), "another workspace gets nothing");

    // A server secret reaches a session only through a grant.
    let mut r = PutRequest::new(Namespace::User, Scope::server(), "SERVER_KEY", Kind::ApiKey);
    r.hosts = vec!["api.openai.com".into()];
    r.default_delivery = Some(Delivery::Proxy);
    let o = v.put(r, SecretBytes::from_slice(b"k2test-server-value-123"), &Actor::Owner).expect("put");
    let who = Requester::Session { workspace_id: "ws2".into(), session_id: Some("sess-9".into()) };
    assert!(v.proxy_items(&who).expect("items").is_empty());
    v.grant(
        GrantRequest {
            selector: Selector::Id(o.meta.id.clone()),
            consumer: Consumer::session("sess-9"),
            delivery: Delivery::Proxy,
            env_name: None,
            file_name: None,
            hosts_narrowed: None,
            expires_at: None,
            version: None,
        },
        &Actor::System("llm-wallet".into()),
    )
    .expect("grant");
    assert_eq!(v.proxy_items(&who).expect("items").iter().map(|i| i.name.clone()).collect::<Vec<_>>(), vec!["SERVER_KEY"]);
    v.resolve(&Selector::Id(o.meta.id.clone()), Use::Proxy(&origin("https://api.openai.com/v1")), &who)
        .expect("granted session resolves on the proxy");

    // Session end drops the grant K2 made; an owner's grant stays.
    v.grant(
        GrantRequest {
            selector: Selector::Id(o.meta.id.clone()),
            consumer: Consumer::session("sess-owner"),
            delivery: Delivery::Proxy,
            env_name: None,
            file_name: None,
            hosts_narrowed: None,
            expires_at: None,
            version: None,
        },
        &Actor::Owner,
    )
    .expect("owner grant");
    let tok = v.placeholder_mint(&o.meta.id, "ws2", "sess-9", "openai", None).expect("mint");
    v.placeholders_drop_session("sess-9").expect("drop");
    assert_eq!(v.placeholder_lookup(&tok).expect_err("gone").code, "placeholder_unknown");
    let e = v
        .resolve(&Selector::Id(o.meta.id.clone()), Use::Proxy(&origin("https://api.openai.com/v1")), &who)
        .err()
        .expect("grant gone with the session");
    assert_eq!(e.code, "not_granted");
    let left = v.grants(&Selector::Id(o.meta.id.clone()), &Actor::Owner).expect("grants");
    assert_eq!(left.iter().map(|g| g.consumer.id.as_str()).collect::<Vec<_>>(), vec!["sess-owner"]);

    // Boot sweep: every placeholder goes.
    let a = v.placeholder_mint(&o.meta.id, "ws2", "sess-a", "openai", None).expect("mint");
    let b = v.placeholder_mint(&o.meta.id, "ws3", "sess-b", "openai", None).expect("mint");
    v.placeholders_drop_all().expect("drop all");
    for tok in [a, b] {
        assert_eq!(v.placeholder_lookup(&tok).expect_err("gone").code, "placeholder_unknown");
    }
    assert!(v.placeholder_mint(&o.meta.id, "", "sess-c", "openai", None).is_err(), "workspace required");
}

#[test]
fn files_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let t = TestDir::new("modes");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    put_user(&v, Scope::workspace("ws1"), "A", SENTINEL, &["api.stripe.com"], &Actor::Owner);
    let dmode = std::fs::metadata(t.path().join("vault")).expect("meta").permissions().mode() & 0o777;
    assert_eq!(dmode, 0o700);
    for f in ["vault.db", "vault.db-wal", "vault.db-shm"] {
        let p = t.path().join("vault").join(f);
        if p.exists() {
            let m = std::fs::metadata(&p).expect("meta").permissions().mode() & 0o777;
            assert_eq!(m & 0o077, 0, "{f} must not be readable by others ({m:o})");
        }
    }
}
