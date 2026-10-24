//! prd-vault-gaps-v1 item 3: `agent` delivery for ssh keys, the vault side.
//! Kind and value checks, ordering (fetch < agent < file), spawn items,
//! identities and signatures (re-checked on every call), the per-secret
//! agent settings (loosening rule, bounds, rekey), the public key, and
//! old-daemon tolerance (R2/R3).

use std::sync::Arc;

use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::keystore::KeyStore;
use k2_vault::ssh_agent::{testkeys, verify_blob, RSA_SHA2_256};
use k2_vault::vault::AgentPolicyPatch;
use k2_vault::*;

fn open(t: &TestDir, ks: &Arc<FakeKeyStore>) -> LocalVault {
    LocalVault::open(t.path().join("vault"), ks.clone() as Arc<dyn KeyStore>, OpenOptions { kdf: KdfCost::Floor }).expect("open vault")
}

fn vault() -> (TestDir, LocalVault) {
    let t = TestDir::new("agent");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    (t, v)
}

fn session(ws: &str, sid: &str) -> Requester {
    Requester::Session { workspace_id: ws.into(), session_id: Some(sid.into()) }
}

fn agent(ws: &str) -> Actor {
    Actor::Agent(AgentActor { workspace_id: ws.into(), session_id: Some("sess-1".into()), shell: false, vault_manage: false })
}

fn put_key(v: &LocalVault, ws: &str, name: &str, key: &str, d: Delivery, actor: &Actor) -> SecretMeta {
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws), name, Kind::SshKey);
    r.default_delivery = Some(d);
    v.put(r, SecretBytes::from_slice(key.as_bytes()), actor).expect("put ssh key").meta
}

fn grant(v: &LocalVault, id: &str, consumer: Consumer, d: Delivery) -> Result<Grant> {
    v.grant(
        GrantRequest {
            selector: Selector::Id(id.into()),
            consumer,
            delivery: d,
            env_name: None,
            file_name: None,
            hosts_narrowed: None,
            expires_at: None,
            version: None,
        },
        &Actor::Owner,
    )
}

#[test]
fn agent_is_for_ssh_keys_the_agent_can_serve() {
    let (_t, v) = vault();
    // Not an ssh key: refused for agent delivery.
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "TOKEN", Kind::ApiKey);
    r.default_delivery = Some(Delivery::Agent);
    assert_eq!(v.put(r, SecretBytes::from_slice(b"k2test-token-123456"), &Actor::Owner).expect_err("api key").code, "delivery_not_for_kind");
    // An ssh_key whose value isn't a key the agent serves.
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "JUNK", Kind::SshKey);
    r.default_delivery = Some(Delivery::Agent);
    assert_eq!(v.put(r, SecretBytes::from_slice(b"not a key"), &Actor::Owner).expect_err("junk").code, "bad_value");
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "OLD_RSA", Kind::SshKey);
    r.default_delivery = Some(Delivery::Agent);
    assert_eq!(v.put(r, SecretBytes::from_slice(testkeys::rsa1024_pem().as_bytes()), &Actor::Owner).expect_err("1024").code, "bad_value");
    // A file-delivery ssh key may hold anything (unchanged), but can't be
    // switched to agent or granted as agent while its value isn't servable.
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "ANY_FILE", Kind::SshKey);
    r.default_delivery = Some(Delivery::File);
    let m = v.put(r, SecretBytes::from_slice(b"-----BEGIN ENCRYPTED PRIVATE KEY-----\nAAAA\n-----END ENCRYPTED PRIVATE KEY-----\n"), &Actor::Owner).expect("file").meta;
    let e = v
        .set_policy(&Selector::Id(m.id.clone()), PolicyPatch { default_delivery: Some(Delivery::Agent), ..Default::default() }, &Actor::Owner)
        .expect_err("encrypted key → agent");
    assert_eq!(e.code, "ssh_key_encrypted");
    assert_eq!(grant(&v, &m.id, Consumer::workspace("ws2"), Delivery::Agent).expect_err("grant").code, "ssh_key_encrypted");
    // A generic secret can't be granted as agent.
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "GEN", Kind::Generic);
    r.default_delivery = Some(Delivery::Env);
    let g = v.put(r, SecretBytes::from_slice(b"k2test-generic-123456"), &Actor::Owner).expect("generic").meta;
    assert_eq!(grant(&v, &g.id, Consumer::workspace("ws2"), Delivery::Agent).expect_err("generic").code, "delivery_not_for_kind");
    // An agent key can't be rotated to something the agent can't serve.
    let ok = put_key(&v, "ws1", "DEPLOY", &testkeys::ed25519(), Delivery::Agent, &Actor::Owner);
    assert_eq!(ok.default_delivery, Delivery::Agent);
    assert!(!ok.agent_can_see(), "agent delivery never shows the key");
    let e = v.rotate(&Selector::Id(ok.id.clone()), SecretBytes::from_slice(b"not a key"), &Actor::Owner).expect_err("rotate");
    assert_eq!(e.code, "bad_value");
    // SSH_AUTH_SOCK can never be a grant's env name.
    let e = v
        .grant(
            GrantRequest {
                selector: Selector::Id(g.id.clone()),
                consumer: Consumer::workspace("ws2"),
                delivery: Delivery::Env,
                env_name: Some("SSH_AUTH_SOCK".into()),
                file_name: None,
                hosts_narrowed: None,
                expires_at: None,
                version: None,
            },
            &Actor::Owner,
        )
        .expect_err("protected env name");
    assert_eq!(e.code, "env_name_not_allowed");
}

#[test]
fn spawn_items_and_identities_list_only_agent_grants() {
    let (_t, v) = vault();
    let ed = put_key(&v, "ws1", "ED_KEY", &testkeys::ed25519(), Delivery::Agent, &Actor::Owner);
    let rsa = put_key(&v, "ws1", "RSA_KEY", &testkeys::rsa2048(), Delivery::Agent, &Actor::Owner);
    put_key(&v, "ws1", "FILE_KEY", &testkeys::ed25519(), Delivery::File, &Actor::Owner);
    let other = put_key(&v, "ws2", "OTHER_WS_KEY", &testkeys::ed25519(), Delivery::Agent, &Actor::Owner);
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "FETCH_ONLY", Kind::ApiKey);
    r.hosts = vec!["api.example.com".into()];
    v.put(r, SecretBytes::from_slice(b"k2test-fetch-123456"), &Actor::Owner).expect("fetch");

    let items = v.spawn_items(&session("ws1", "s1")).expect("items");
    let got: Vec<(&str, Delivery)> = items.iter().map(|i| (i.name.as_str(), i.delivery)).collect();
    assert_eq!(got, vec![("ED_KEY", Delivery::Agent), ("FILE_KEY", Delivery::File), ("RSA_KEY", Delivery::Agent)]);

    let ids = v.agent_identities(&session("ws1", "s1")).expect("identities");
    let names: Vec<&str> = ids.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, vec!["ED_KEY", "RSA_KEY"], "file, fetch and other workspaces' keys never appear");
    assert!(v.agent_identities(&session("ws3", "s9")).expect("none").is_empty());

    // Signatures verify; the blob must be the secret's current key.
    let ed_id = ids.iter().find(|i| i.secret_id == ed.id).expect("ed");
    let sig = v.agent_sign(&ed.id, &session("ws1", "s1"), &ed_id.public_blob, b"challenge", 0, Some("SHA256:host")).expect("sign");
    assert!(verify_blob(&ed_id.public_blob, b"challenge", &sig));
    let rsa_id = ids.iter().find(|i| i.secret_id == rsa.id).expect("rsa");
    let sig = v.agent_sign(&rsa.id, &session("ws1", "s1"), &rsa_id.public_blob, b"challenge", RSA_SHA2_256, None).expect("rsa sign");
    assert!(verify_blob(&rsa_id.public_blob, b"challenge", &sig));
    assert_eq!(v.agent_sign(&rsa.id, &session("ws1", "s1"), &rsa_id.public_blob, b"c", 0, None).expect_err("sha1").code, "ssh_rsa_sha1");
    // Another workspace's key: refused by the grant check.
    let e = v.agent_sign(&other.id, &session("ws1", "s1"), &ed_id.public_blob, b"c", 0, None).expect_err("not granted");
    assert_eq!(e.code, "delivery_not_granted");

    // A rotation changes the key: the old blob is refused, the list moves on.
    v.rotate(&Selector::Id(ed.id.clone()), SecretBytes::from_slice(testkeys::ed25519().as_bytes()), &Actor::Owner).expect("rotate");
    assert_eq!(v.agent_sign(&ed.id, &session("ws1", "s1"), &ed_id.public_blob, b"c", 0, None).expect_err("old blob").code, "key_changed");
    let ids2 = v.agent_identities(&session("ws1", "s1")).expect("identities");
    assert_ne!(ids2.iter().find(|i| i.secret_id == ed.id).expect("ed").public_blob, ed_id.public_blob);

    // Disable: gone from the list and refused.
    v.disable(&Selector::Id(rsa.id.clone()), &Actor::Owner).expect("disable");
    assert!(v.agent_identities(&session("ws1", "s1")).expect("ids").iter().all(|i| i.secret_id != rsa.id));
    assert_eq!(v.agent_sign(&rsa.id, &session("ws1", "s1"), &rsa_id.public_blob, b"c", RSA_SHA2_256, None).expect_err("disabled").code, "secret_not_active");

    // A grant to another workspace as agent lists it there; file beats agent.
    grant(&v, &other.id, Consumer::workspace("ws1"), Delivery::Agent).expect("grant agent");
    assert!(v.agent_identities(&session("ws1", "s1")).expect("ids").iter().any(|i| i.secret_id == other.id));
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws4"), "BOTH", Kind::SshKey);
    r.default_delivery = Some(Delivery::Agent);
    let both = v.put(r, SecretBytes::from_slice(testkeys::ed25519().as_bytes()), &Actor::Owner).expect("both").meta;
    grant(&v, &both.id, Consumer::workspace("ws4"), Delivery::File).expect("file grant");
    let items = v.spawn_items(&session("ws4", "s4")).expect("items");
    assert_eq!(items.iter().find(|i| i.secret_id == both.id).expect("both").delivery, Delivery::File, "the higher delivery wins");
    assert!(v.agent_identities(&session("ws4", "s4")).expect("ids").is_empty());

    // Every signature is one audit entry (rolled up, exact count).
    v.flush_rollups();
    let rows = v.audit(&AuditQuery { secret_id: Some(ed.id.clone()), ..Default::default() }, &Actor::Owner).expect("audit");
    let ok: u64 = rows.iter().filter(|r| r.action == "sign" && r.outcome == "ok").map(|r| r.count).sum();
    assert_eq!(ok, 1, "{rows:?}");
    assert!(rows.iter().any(|r| r.action == "sign" && r.target_host.as_deref() == Some("ssh:SHA256:host")), "{rows:?}");
    assert!(rows.iter().any(|r| r.action == "sign" && r.outcome == "deny:key_changed"), "{rows:?}");
}

#[test]
fn agent_settings_default_bounds_and_the_loosening_rule() {
    let (_t, v) = vault();
    // A person's key in ws1; the ws1 agent's own key.
    let person = put_key(&v, "ws1", "PERSON_KEY", &testkeys::ed25519(), Delivery::Agent, &Actor::Owner);
    let own = put_key(&v, "ws1", "OWN_KEY", &testkeys::ed25519(), Delivery::Agent, &agent("ws1"));
    let p = v.agent_policy(&person.id).expect("policy");
    assert_eq!((p.burst, p.per_minute, p.allow_forwarded), (60, 240, false), "defaults");

    let sel = Selector::Id(person.id.clone());
    let patch = |b: Option<u32>, m: Option<u32>, f: Option<bool>| AgentPolicyPatch { burst: b, per_minute: m, allow_forwarded: f };
    for (b, m) in [(Some(0), None), (Some(601), None), (None, Some(0)), (None, Some(6001))] {
        assert_eq!(v.set_agent_policy(&sel, patch(b, m, None), &Actor::Owner).expect_err("bounds").code, "bad_limit");
    }
    // An agent may tighten a person's key, never loosen it.
    let (_, p) = v.set_agent_policy(&sel, patch(Some(5), Some(100), None), &agent("ws1")).expect("tighten");
    assert_eq!((p.burst, p.per_minute), (5, 100));
    assert_eq!(v.set_agent_policy(&sel, patch(Some(6), None, None), &agent("ws1")).expect_err("raise").code, "vault_owner_confirm_required");
    assert_eq!(v.set_agent_policy(&sel, patch(None, None, Some(true)), &agent("ws1")).expect_err("forward").code, "vault_owner_confirm_required");
    // Another workspace's agent can't see it at all.
    assert_eq!(v.set_agent_policy(&sel, patch(Some(1), None, None), &agent("ws2")).expect_err("other ws").status, 404);
    // The person may loosen; the agent may loosen its own key.
    let (_, p) = v.set_agent_policy(&sel, patch(Some(600), Some(6000), Some(true)), &Actor::Owner).expect("owner");
    assert_eq!((p.burst, p.per_minute, p.allow_forwarded), (600, 6000, true));
    v.set_agent_policy(&Selector::Id(own.id.clone()), patch(None, None, Some(true)), &agent("ws1")).expect("own key");
    // Not an ssh key.
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "TOKEN", Kind::ApiKey);
    r.hosts = vec!["api.example.com".into()];
    let tok = v.put(r, SecretBytes::from_slice(b"k2test-token-123456"), &Actor::Owner).expect("tok").meta;
    assert_eq!(v.set_agent_policy(&Selector::Id(tok.id), patch(Some(1), None, None), &Actor::Owner).expect_err("kind").code, "agent_policy_not_for_kind");

    // A rekey keeps the settings and reports no tamper.
    v.rekey(&Actor::Owner, &HumanProof::Confirmed { method: "t".into() }).expect("rekey");
    let p = v.agent_policy(&person.id).expect("after rekey");
    assert_eq!((p.burst, p.per_minute, p.allow_forwarded), (600, 6000, true));
    assert_eq!(v.status().tamper, "ok");
}

#[test]
fn a_tampered_agent_setting_fails_closed() {
    let t = TestDir::new("agent-tamper");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let k = put_key(&v, "ws1", "KEY", &testkeys::ed25519(), Delivery::Agent, &Actor::Owner);
    v.set_agent_policy(&Selector::Id(k.id.clone()), AgentPolicyPatch { allow_forwarded: Some(true), ..Default::default() }, &Actor::Owner)
        .expect("allow");
    drop(v);
    let c = rusqlite::Connection::open(t.path().join("vault/vault.db")).expect("db");
    c.execute("UPDATE agent_policy SET burst = 599 WHERE secret_id = ?1", [&k.id]).expect("tamper");
    drop(c);
    let v = open(&t, &ks);
    let p = v.agent_policy(&k.id).expect("policy");
    assert_eq!((p.burst, p.per_minute, p.allow_forwarded), (1, 1, false), "strictest on a bad MAC");
    assert_ne!(v.status().tamper, "ok", "status says so");
}

/// Round 2 (7): a policy row is bound to the audit entry that wrote it.
/// Deleting the row, or putting an older (validly MAC'd) row back to undo
/// a tightening, fails closed.
#[test]
fn deleting_or_replaying_an_agent_setting_fails_closed() {
    let t = TestDir::new("agent-replay");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let k = put_key(&v, "ws1", "KEY", &testkeys::ed25519(), Delivery::Agent, &Actor::Owner);
    let sel = Selector::Id(k.id.clone());
    v.set_agent_policy(&sel, AgentPolicyPatch { allow_forwarded: Some(true), ..Default::default() }, &Actor::Owner).expect("allow");
    let old: (i64, i64, i64, String, String, i64, Vec<u8>) = rusqlite::Connection::open(t.path().join("vault/vault.db"))
        .expect("db")
        .query_row(
            "SELECT burst, per_minute, allow_forwarded, updated_by, updated_at, audit_seq, row_mac FROM agent_policy WHERE secret_id = ?1",
            [&k.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        )
        .expect("row");
    // Tighten: no forwarding, burst 5.
    v.set_agent_policy(&sel, AgentPolicyPatch { burst: Some(5), allow_forwarded: Some(false), ..Default::default() }, &Actor::Owner)
        .expect("tighten");
    let p = v.agent_policy(&k.id).expect("policy");
    assert_eq!((p.burst, p.allow_forwarded), (5, false), "the fresh row is trusted");
    drop(v);
    // Replay the older row (its own MAC is valid).
    let c = rusqlite::Connection::open(t.path().join("vault/vault.db")).expect("db");
    c.execute(
        "UPDATE agent_policy SET burst = ?2, per_minute = ?3, allow_forwarded = ?4, updated_by = ?5, updated_at = ?6, audit_seq = ?7, row_mac = ?8 WHERE secret_id = ?1",
        rusqlite::params![k.id, old.0, old.1, old.2, old.3, old.4, old.5, old.6],
    )
    .expect("replay");
    drop(c);
    let v = open(&t, &ks);
    let p = v.agent_policy(&k.id).expect("policy");
    assert_eq!((p.burst, p.per_minute, p.allow_forwarded), (1, 1, false), "a replayed row isn't the latest: strictest");
    drop(v);
    // Delete it: the audit log says one was set.
    let c = rusqlite::Connection::open(t.path().join("vault/vault.db")).expect("db");
    c.execute("DELETE FROM agent_policy WHERE secret_id = ?1", [&k.id]).expect("delete");
    drop(c);
    let v = open(&t, &ks);
    let p = v.agent_policy(&k.id).expect("policy");
    assert_eq!((p.burst, p.per_minute, p.allow_forwarded), (1, 1, false), "a deleted row is not the defaults: strictest");
    assert_ne!(v.status().tamper, "ok");
    // A person setting it again repairs it.
    let (_, p) = v.set_agent_policy(&sel, AgentPolicyPatch { burst: Some(10), per_minute: Some(100), ..Default::default() }, &Actor::Owner).expect("repair");
    assert_eq!(v.agent_policy(&k.id).expect("policy"), p);
    // A secret with no policy at all keeps the defaults.
    let k2 = put_key(&v, "ws1", "KEY2", &testkeys::ed25519(), Delivery::Agent, &Actor::Owner);
    let d = v.agent_policy(&k2.id).expect("defaults");
    assert_eq!((d.burst, d.per_minute, d.allow_forwarded), (60, 240, false));
}

/// Round 2 (6): an ssh private key never goes into an outbound request.
#[test]
fn an_ssh_key_is_never_fetched() {
    let (_t, v) = vault();
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "FETCHY", Kind::SshKey);
    r.default_delivery = Some(Delivery::Fetch);
    r.hosts = vec!["api.example.com".into()];
    let m = v.put(r, SecretBytes::from_slice(testkeys::ed25519().as_bytes()), &Actor::Owner).expect("put").meta;
    let o = k2_vault::hosts::Origin::parse_url("https://api.example.com/x").expect("origin").1;
    let Err(e) = v.resolve(&Selector::Id(m.id), Use::Fetch(&o), &session("ws1", "s1")) else {
        panic!("an ssh key was resolved for fetch");
    };
    assert_eq!(e.code, "ssh_key_not_sent");
}

/// Round 2 (2): the daemon's socket records are MAC'd; a tampered row is
/// never returned.
#[test]
fn agent_socket_records_are_macd() {
    let t = TestDir::new("agent-sock-rec");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let rec = |tag: &str| k2_vault::vault::AgentSocket {
        tag: tag.into(),
        kind: "session".into(),
        workspace_id: "ws1".into(),
        secrets_dir: format!("/run/k2/secrets/{tag}-abcdefgh"),
        sock_path: format!("/run/k2/secrets/{tag}-abcdefgh/a.sock"),
        created_at: "2026-10-24T00:00:00Z".into(),
        ..Default::default()
    };
    v.agent_socket_put(&rec("s1")).expect("put");
    v.agent_socket_put(&rec("s2")).expect("put");
    assert_eq!(v.agent_socket_get("s1").expect("get"), Some(rec("s1")));
    assert_eq!(v.agent_socket_put(&k2_vault::vault::AgentSocket { kind: "other".into(), ..rec("s3") }).expect_err("kind").code, "bad_agent_socket");
    drop(v);
    let c = rusqlite::Connection::open(t.path().join("vault/vault.db")).expect("db");
    c.execute("UPDATE agent_socket SET workspace_id = 'ws-other' WHERE tag = 's2'", []).expect("tamper");
    drop(c);
    let v = open(&t, &ks);
    assert_eq!(v.agent_socket_get("s2").expect("get"), None, "a tampered record is never used");
    assert_eq!(v.agent_sockets().expect("all"), vec![rec("s1")]);
    // The ptyd binding: once, after the spawn; a fresh put is unbound.
    assert!(v.agent_socket_bind("s1", "boot-a", 100, 1_700_000_000).expect("bind"));
    let b = v.agent_socket_get("s1").expect("get").expect("rec");
    assert_eq!((b.ptyd_boot.as_str(), b.pid, b.started_at), ("boot-a", 100, 1_700_000_000));
    assert!(!v.agent_socket_bind("s1", "boot-a", 101, 1_700_000_000).expect("again"), "bound once");
    assert_eq!(v.agent_socket_get("s1").expect("get").expect("rec").pid, 100);
    assert!(!v.agent_socket_bind("nope", "boot-a", 1, 1).expect("no record"));
    assert_eq!(v.agent_socket_bind("s1", "", 1, 1).expect_err("empty boot").code, "bad_agent_socket");
    v.agent_socket_put(&k2_vault::vault::AgentSocket { ptyd_boot: "forged".into(), pid: 9, started_at: 9, ..rec("s1") }).expect("put");
    assert!(v.agent_socket_get("s1").expect("get").expect("rec").ptyd_boot.is_empty(), "a put never carries a binding");
    v.agent_socket_remove("s1").expect("rm");
    assert!(v.agent_sockets().expect("all").is_empty());
}

/// Round 3 (4): a vault made by the 8939f4853 build has agent_policy
/// without `audit_seq` (and an older agent_socket). It opens; the tables
/// are rebuilt; a policy that had been set reads as missing (strictest)
/// until a person sets it again, an unset one keeps the defaults.
#[test]
fn a_vault_with_the_pre_release_agent_tables_opens() {
    let t = TestDir::new("agent-old-schema");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let k = put_key(&v, "ws1", "OLD", &testkeys::ed25519(), Delivery::Agent, &Actor::Owner);
    let k2 = put_key(&v, "ws1", "NEVER_SET", &testkeys::ed25519(), Delivery::Agent, &Actor::Owner);
    v.set_agent_policy(&Selector::Id(k.id.clone()), AgentPolicyPatch { burst: Some(9), ..Default::default() }, &Actor::Owner).expect("set");
    drop(v);
    let c = rusqlite::Connection::open(t.path().join("vault/vault.db")).expect("db");
    c.execute_batch(
        "DROP TABLE agent_policy;
         CREATE TABLE agent_policy (secret_id TEXT PRIMARY KEY, burst INTEGER NOT NULL, per_minute INTEGER NOT NULL,
            allow_forwarded INTEGER NOT NULL DEFAULT 0, updated_by TEXT NOT NULL, updated_at TEXT NOT NULL, row_mac BLOB NOT NULL);
         DROP TABLE agent_socket;
         CREATE TABLE agent_socket (tag TEXT PRIMARY KEY, kind TEXT NOT NULL, workspace_id TEXT NOT NULL, secrets_dir TEXT NOT NULL,
            sock_path TEXT NOT NULL, created_at TEXT NOT NULL, row_mac BLOB NOT NULL);",
    )
    .expect("old schema");
    c.execute("INSERT INTO agent_policy VALUES (?1, 600, 6000, 1, 'owner', 't', x'00')", [&k.id]).expect("old row");
    c.execute("INSERT INTO agent_socket VALUES ('s1', 'session', 'ws1', '/d', '/d/a.sock', 't', x'00')", []).expect("old sock");
    drop(c);
    let v = open(&t, &ks);
    let p = v.agent_policy(&k.id).expect("policy");
    assert_eq!((p.burst, p.per_minute, p.allow_forwarded), (1, 1, false), "a policy that was set reads as missing: strictest");
    let d = v.agent_policy(&k2.id).expect("defaults");
    assert_eq!((d.burst, d.per_minute, d.allow_forwarded), (60, 240, false), "never set: the defaults");
    assert!(v.agent_sockets().expect("socks").is_empty(), "old socket records are dropped");
    let (_, p) = v.set_agent_policy(&Selector::Id(k.id.clone()), AgentPolicyPatch { burst: Some(9), ..Default::default() }, &Actor::Owner).expect("set again");
    assert_eq!(v.agent_policy(&k.id).expect("policy"), p);
    v.agent_socket_put(&k2_vault::vault::AgentSocket {
        tag: "s2".into(),
        kind: "session".into(),
        workspace_id: "ws1".into(),
        secrets_dir: "/d2".into(),
        sock_path: "/d2/a.sock".into(),
        created_at: "t".into(),
        ..Default::default()
    })
    .expect("new socket record");
}

/// Round 3 (7): deleting the newer audit entry (a tightening) and putting
/// the older, looser row back can't revive it: the log has a gap against
/// its MAC'd header.
#[test]
fn deleting_an_audit_entry_cannot_revive_an_older_policy() {
    let t = TestDir::new("agent-audit-gap");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let k = put_key(&v, "ws1", "KEY", &testkeys::ed25519(), Delivery::Agent, &Actor::Owner);
    let sel = Selector::Id(k.id.clone());
    v.set_agent_policy(&sel, AgentPolicyPatch { allow_forwarded: Some(true), ..Default::default() }, &Actor::Owner).expect("allow");
    let db = t.path().join("vault/vault.db");
    let old: (i64, i64, i64, String, String, i64, Vec<u8>) = rusqlite::Connection::open(&db)
        .expect("db")
        .query_row(
            "SELECT burst, per_minute, allow_forwarded, updated_by, updated_at, audit_seq, row_mac FROM agent_policy WHERE secret_id = ?1",
            [&k.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        )
        .expect("row");
    v.set_agent_policy(&sel, AgentPolicyPatch { allow_forwarded: Some(false), ..Default::default() }, &Actor::Owner).expect("tighten");
    drop(v);
    let c = rusqlite::Connection::open(&db).expect("db");
    let newest: i64 = c
        .query_row("SELECT MAX(seq) FROM audit WHERE secret_id = ?1 AND action = 'policy' AND outcome LIKE 'ok:agent %'", [&k.id], |r| r.get(0))
        .expect("newest");
    c.execute("DELETE FROM audit WHERE seq = ?1", [newest]).expect("delete newest");
    c.execute(
        "UPDATE agent_policy SET burst = ?2, per_minute = ?3, allow_forwarded = ?4, updated_by = ?5, updated_at = ?6, audit_seq = ?7, row_mac = ?8 WHERE secret_id = ?1",
        rusqlite::params![k.id, old.0, old.1, old.2, old.3, old.4, old.5, old.6],
    )
    .expect("replay");
    drop(c);
    let v = open(&t, &ks);
    let p = v.agent_policy(&k.id).expect("policy");
    assert!(!p.allow_forwarded, "the looser row must not come back");
    assert_eq!((p.burst, p.per_minute), (1, 1), "strictest while the log has a gap");
}

/// Round 3 (3): an ssh_key value must read as a key whatever the delivery;
/// one pasted into a one-line field (line breaks gone) is refused.
#[test]
fn a_flattened_ssh_key_is_refused_on_put_and_rotate() {
    let (_t, v) = vault();
    let key = testkeys::ed25519();
    let flat = key.replace('\n', " ");
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "FLAT", Kind::SshKey);
    r.default_delivery = Some(Delivery::File);
    assert_eq!(v.put(r, SecretBytes::from_slice(flat.as_bytes()), &Actor::Owner).expect_err("flat").code, "bad_value");
    let m = put_key(&v, "ws1", "GOOD", &key, Delivery::File, &Actor::Owner);
    let e = v.rotate(&Selector::Id(m.id.clone()), SecretBytes::from_slice(flat.as_bytes()), &Actor::Owner).expect_err("flat rotate");
    assert_eq!(e.code, "bad_value");
    v.rotate(&Selector::Id(m.id), SecretBytes::from_slice(testkeys::ed25519().as_bytes()), &Actor::Owner).expect("a real key rotates");
}

/// Round 2 (14): what a downgrade to 0.46.3 would break.
#[test]
fn downgrade_blockers_count_agent_job_and_signed_rows() {
    let (_t, v) = vault();
    assert!(!v.downgrade_blockers().expect("none").any());
    let k = put_key(&v, "ws1", "AG", &testkeys::ed25519(), Delivery::Agent, &Actor::Owner);
    let f = put_key(&v, "ws1", "FI", &testkeys::ed25519(), Delivery::File, &Actor::Owner);
    grant(&v, &f.id, Consumer::job("ws1", "j1"), Delivery::File).expect("job grant");
    grant(&v, &f.id, Consumer::workspace("ws2"), Delivery::Agent).expect("agent grant");
    let b = v.downgrade_blockers().expect("blockers");
    assert_eq!((b.agent, b.job_grants, b.signed), (2, 1, 0), "{b:?}");
    assert!(b.any());
    let _ = k;
}

#[test]
fn the_public_key_of_an_ssh_key() {
    let (_t, v) = vault();
    let key = testkeys::ed25519();
    let m = put_key(&v, "ws1", "DEPLOY_KEY", &key, Delivery::Agent, &Actor::Owner);
    let (_, line, fp) = v.agent_public_key(&Selector::Id(m.id.clone()), &agent("ws1")).expect("public key");
    assert!(line.starts_with("ssh-ed25519 AAAA") && line.ends_with(" DEPLOY_KEY"), "{line}");
    assert!(fp.starts_with("SHA256:"));
    assert!(!line.contains("PRIVATE"));
    assert_eq!(v.agent_public_key(&Selector::Id(m.id.clone()), &agent("ws2")).expect_err("other ws").status, 404);
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws1"), "TOKEN", Kind::ApiKey);
    r.hosts = vec!["api.example.com".into()];
    let tok = v.put(r, SecretBytes::from_slice(b"k2test-token-123456"), &Actor::Owner).expect("tok").meta;
    assert_eq!(v.agent_public_key(&Selector::Id(tok.id), &Actor::Owner).expect_err("kind").code, "not_an_ssh_key");
}

/// R3, and this build's own R2 skip. Checks that `agent` is appended last
/// (older codes, and so older MACs, unchanged), that the stored text is
/// exactly `agent`, and that this build skips delivery text it doesn't
/// know without failing list/spawn (made-up text written into an agent
/// row). It proves nothing about 0.46.3, which has no R2 and fails every
/// list on such a row; that is why `k2 vault doctor` warns once agent rows
/// exist (downgrade_blockers).
#[test]
fn agent_rows_stay_readable_by_r2_tolerant_daemons() {
    const OLDER_DELIVERIES: &[&str] = &["fetch", "proxy", "file", "env", "api", "none"];
    assert!(!OLDER_DELIVERIES.contains(&Delivery::Agent.as_str()));
    for (i, d) in OLDER_DELIVERIES.iter().enumerate() {
        assert_eq!(Delivery::parse(d).expect("known").code() as usize, i, "older codes unchanged");
    }
    assert_eq!(Delivery::Agent.code() as usize, OLDER_DELIVERIES.len());

    let t = TestDir::new("agent-r2");
    let ks = Arc::new(FakeKeyStore::new());
    let v = open(&t, &ks);
    let keep = put_key(&v, "ws1", "KEEP_FILE", &testkeys::ed25519(), Delivery::File, &Actor::Owner);
    let k = put_key(&v, "ws1", "AGENT_KEY", &testkeys::ed25519(), Delivery::Agent, &Actor::Owner);
    let shared = put_key(&v, "ws1", "SHARED", &testkeys::ed25519(), Delivery::File, &Actor::Owner);
    let g = grant(&v, &shared.id, Consumer::workspace("ws2"), Delivery::Agent).expect("agent grant");
    drop(v);
    let c = rusqlite::Connection::open(t.path().join("vault/vault.db")).expect("db");
    let stored: String = c.query_row("SELECT default_delivery FROM secret WHERE id = ?1", [&k.id], |r| r.get(0)).expect("row");
    assert_eq!(stored, "agent");
    let stored: String = c.query_row("SELECT delivery FROM grants WHERE id = ?1", [&g.id], |r| r.get(0)).expect("grant");
    assert_eq!(stored, "agent");
    // What a daemon that doesn't know `agent` sees: text it can't parse.
    c.execute("UPDATE secret SET default_delivery = 'zz_unknown_delivery' WHERE id = ?1", [&k.id]).expect("secret");
    c.execute("UPDATE grants SET delivery = 'zz_unknown_delivery' WHERE id = ?1", [&g.id]).expect("grant");
    drop(c);
    let v = open(&t, &ks);
    let names: Vec<String> = v.list(&ListFilter::default(), &Actor::Owner).expect("list must not fail").into_iter().map(|m| m.name).collect();
    assert!(names.contains(&"KEEP_FILE".to_string()) && !names.contains(&"AGENT_KEY".to_string()), "{names:?}");
    let items = v.spawn_items(&session("ws1", "s1")).expect("spawn items must not fail");
    assert!(items.iter().any(|i| i.secret_id == keep.id));
    assert!(v.spawn_items(&session("ws2", "s2")).expect("ws2").is_empty());
    assert!(v.agent_identities(&session("ws1", "s1")).expect("identities must not fail").is_empty());
    assert_eq!(v.status().tamper, "ok");
}
