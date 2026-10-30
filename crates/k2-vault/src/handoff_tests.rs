//! prd-agent-secret-handoff-v1 §14, k2-vault unit tests 1–7. Temp vaults
//! and the fake key store only.

use std::sync::Arc;

use super::*;
use crate::confirm::KdfCost;
use crate::fsutil::TestDir;
use crate::keystore::fake::FakeKeyStore;
use crate::model::Scope;
use crate::policy::{Actor, AgentActor, Role};
use crate::store::{PolicyPatch, PutRequest, Requester, SecretStore, Selector, Use};
use crate::{KeyStore, OpenOptions};

const SENTINEL: &str = "k2test_HANDOFF_SENTINEL_9f8e7d6c5b4a3210";
const ME_FP: &str = "fp-receiver";
const PEER_FP: &str = "fp-sender";

struct V {
    _d: TestDir,
    v: Arc<LocalVault>,
}

fn open(tag: &str) -> V {
    let d = TestDir::new(tag);
    let ks: Arc<dyn KeyStore> = Arc::new(FakeKeyStore::new());
    let v = Arc::new(LocalVault::open(d.path().join("vault"), ks, OpenOptions { kdf: KdfCost::Floor }).expect("open"));
    V { _d: d, v }
}

fn agent(ws: &str) -> Actor {
    Actor::Agent(AgentActor { workspace_id: ws.into(), session_id: Some("s1".into()), shell: false, vault_manage: false })
}

fn put(v: &LocalVault, ws: &str, name: &str, value: &str, actor: &Actor) -> SecretMeta {
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws), name, Kind::ApiKey);
    r.hosts = vec!["api.stripe.com".into()];
    v.put(r, SecretBytes::from_slice(value.as_bytes()), actor).expect("put").meta
}

fn item(name: &str) -> HandoffItem {
    HandoffItem {
        name: name.into(),
        as_name: None,
        kind: "api_key".into(),
        hosts: vec!["https://api.stripe.com".into()],
        inject: None,
        description: "for refunds".into(),
        delivery: "api".into(),
        ceiling: "api".into(),
        human_given: false,
        ttl_secs: None,
    }
}

fn incoming(ws: &str, it: HandoffItem, peer: &str, auto: bool) -> Incoming {
    Incoming {
        delivery_id: new_delivery_id(),
        workspace_id: ws.into(),
        peer_addr: peer.into(),
        peer_fp: None,
        sender_actor: "agent:ws-sales/s9".into(),
        source_vault_id: None,
        item: it,
        note: "for the refund job".into(),
        reply_to: None,
        auto_land: auto,
        remote_off: false,
        sender_label: peer.into(),
    }
}

fn value(v: &LocalVault, ws: &str, name: &str) -> Vec<u8> {
    v.resolve(&Selector::user(Scope::workspace(ws), name), Use::System, &Requester::System("t".into()))
        .expect("resolve")
        .value
        .expose()
        .to_vec()
}

// ── 1. seal / open ──────────────────────────────────────────────────

fn sealed_between(a: &LocalVault, b: &LocalVault, tweak: impl FnOnce(&mut HandoffPayload)) -> (Vec<u8>, String) {
    let id = new_delivery_id();
    let from = Party { fingerprint: PEER_FP.into(), host: "rosson.k2.dev".into(), workspace_id: "ws-sales".into(), handle: "sales".into(), actor: "agent:ws-sales/s".into(), vault_id: String::new() };
    let to = Party { fingerprint: ME_FP.into(), host: "luzz.k2.dev".into(), workspace_id: "ws-ops".into(), handle: "ops".into(), actor: String::new(), vault_id: b.vault_id().into() };
    let mut p = HandoffPayload::new(&id, from, to, item("STRIPE_KEY"), &SecretBytes::from_slice(SENTINEL.as_bytes()), "n".into(), None);
    tweak(&mut p);
    (a.handoff_seal(&b.carry_recipient().expect("recipient"), &mut p).expect("seal"), id)
}

fn expect<'a>(id: &'a str) -> OpenExpect<'a> {
    OpenExpect { local_fingerprint: ME_FP, peer_fingerprint: PEER_FP, delivery_id: id }
}

#[test]
fn seal_open_round_trip_binds_vault_server_sender_and_delivery() {
    let (a, b, c) = (open("ho-a"), open("ho-b"), open("ho-c"));
    let (file, id) = sealed_between(&a.v, &b.v, |_| {});
    assert!(!file.windows(SENTINEL.len()).any(|w| w == SENTINEL.as_bytes()), "sealed bytes hold no plaintext value");
    let p = b.v.handoff_open(&file, expect(&id)).expect("opens for the receiver");
    assert_eq!(p.value().expect("value").expose(), SENTINEL.as_bytes());
    assert_eq!(p.from.vault_id, a.v.vault_id(), "the seal stamps the source vault");
    assert_eq!(p.item.name, "STRIPE_KEY");

    // Another vault can't open it at all (not its age key).
    assert!(c.v.handoff_open(&file, expect(&id)).is_err());
    // Sealed for another vault id (to the right key).
    let (f2, id2) = sealed_between(&a.v, &b.v, |p| p.to.vault_id = "another-vault".into());
    assert_eq!(b.v.handoff_open(&f2, expect(&id2)).expect_err("vault id").code, "carry_not_for_this_vault");
    // Made for another server's fingerprint.
    let e = b.v.handoff_open(&file, OpenExpect { local_fingerprint: "fp-other", ..expect(&id) }).expect_err("server");
    assert_eq!(e.code, "handoff_not_for_this_server");
    // The sealed sender isn't the peer the envelope authenticated (H5).
    let e = b.v.handoff_open(&file, OpenExpect { peer_fingerprint: "fp-mallory", ..expect(&id) }).expect_err("sender");
    assert_eq!(e.code, "handoff_sender_mismatch");
    // The outer delivery id must match.
    let e = b.v.handoff_open(&file, OpenExpect { delivery_id: "hd_00000000000000000000000000000000", ..expect(&id) }).expect_err("id");
    assert_eq!(e.code, "bad_handoff");
    // Expired in transit.
    let (f3, id3) = sealed_between(&a.v, &b.v, |p| {
        p.transport_expires_at = (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
    });
    assert_eq!(b.v.handoff_open(&f3, expect(&id3)).expect_err("expired").code, "handoff_expired");
}

#[test]
fn open_refuses_a_bad_inner_signature_and_unknown_formats() {
    let (a, b) = (open("ho-sig-a"), open("ho-sig-b"));
    let rcp = b.v.carry_recipient().expect("recipient");
    let id = new_delivery_id();
    let from = Party { fingerprint: PEER_FP.into(), workspace_id: "w".into(), vault_id: a.v.vault_id().into(), ..Default::default() };
    let to = Party { fingerprint: ME_FP.into(), workspace_id: "w2".into(), vault_id: b.v.vault_id().into(), ..Default::default() };
    let mut p = HandoffPayload::new(&id, from, to, item("K"), &SecretBytes::from_slice(SENTINEL.as_bytes()), String::new(), None);
    p.source_public_key = crate::crypto::hex(&a.v.carry_public_key().map(|h| crate::crypto::unhex(&h).expect("hex")).expect("pk"));
    let body = serde_json::to_string(&p).expect("json");
    // A signature over different bytes.
    let wrong = a.v.engine_sign(crate::carry::ENGINE, crate::carry::SIGNING, b"something else").expect("sign");
    let s = Sealed { payload: body.clone(), sig: crate::crypto::hex(&wrong) };
    let file = agefmt::encrypt(&[rcp], &serde_json::to_vec(&s).expect("json"));
    assert_eq!(b.v.handoff_open(&file, expect(&id)).expect_err("bad sig").code, "vault_tamper");
    // A handoff-shaped payload with another format string.
    let mut q: serde_json::Value = serde_json::from_str(&body).expect("json");
    q["format"] = serde_json::json!("k2-vault-carry/1");
    let qs = q.to_string();
    let sig = a.v.engine_sign(crate::carry::ENGINE, crate::carry::SIGNING, qs.as_bytes()).expect("sign");
    let file = agefmt::encrypt(&[rcp], &serde_json::to_vec(&Sealed { payload: qs, sig: crate::crypto::hex(&sig) }).expect("json"));
    assert_eq!(b.v.handoff_open(&file, expect(&id)).expect_err("format").code, "bad_handoff");
    // H6: a carry never opens as a handoff, and a handoff never as a carry.
    let carry = a.v.carry_seal(&rcp, b.v.vault_id(), false, vec![]).expect("carry");
    assert_eq!(b.v.handoff_open(&carry, expect(&id)).expect_err("carry as handoff").code, "bad_handoff");
    let (ho, _) = sealed_between(&a.v, &b.v, |_| {});
    assert_eq!(b.v.carry_open(&ho).expect_err("handoff as carry").code, "bad_carry");
}

// ── 2. never sendable ───────────────────────────────────────────────

#[test]
fn never_sendable_kinds_scopes_and_value_shapes() {
    let t = open("ho-never");
    let v = &t.v;
    let ok = put(v, "ws-a", "OK_KEY", SENTINEL, &agent("ws-a"));
    check_sendable(&ok).expect("a workspace api key is sendable");
    for kind in [Kind::LoginSlot, Kind::AppPass, Kind::EngineCred] {
        let mut m = ok.clone();
        m.kind = kind;
        assert_eq!(check_sendable(&m).expect_err("kind").code, "never_sendable", "{kind}");
    }
    let mut server = ok.clone();
    server.scope = Scope::server();
    let e = check_sendable(&server).expect_err("server scope");
    assert_eq!(e.code, "send_not_allowed");
    assert!(e.hint.contains("k2 vault grant"), "{}", e.hint);
    let mut session = ok.clone();
    session.scope = Scope::session("s1");
    assert_eq!(check_sendable(&session).expect_err("session").code, "never_sendable");
    let mut sys = ok.clone();
    sys.namespace = Namespace::System;
    assert_eq!(check_sendable(&sys).expect_err("system").code, "never_sendable");
    let mut off = ok.clone();
    off.status = SecretStatus::Disabled;
    assert_eq!(check_sendable(&off).expect_err("disabled").code, "secret_not_active");
    for p in ["k2sk_abc123", "k2skn_xyz", "  k2rs_grant", "k2v_ph_123"] {
        assert_eq!(check_value(p.as_bytes()).expect_err(p).code, "never_sendable", "{p}");
    }
    assert_eq!(check_value(&vec![b'a'; MAX_VALUE_BYTES + 1]).expect_err("big").code, "value_too_large");
    check_value(SENTINEL.as_bytes()).expect("a plain key is fine");
}

#[test]
fn notes_with_the_value_in_any_encoding_are_refused() {
    let v = SENTINEL.as_bytes();
    assert_eq!(check_note("for the refund job", NOTE_MAX, v).expect("plain"), "for the refund job");
    for bad in [
        format!("here: {SENTINEL}"),
        format!("b64 {}", STANDARD.encode(SENTINEL)),
        format!("hex {}", crate::crypto::hex(SENTINEL.as_bytes())),
    ] {
        assert_eq!(check_note(&bad, MSG_NOTE_MAX, v).expect_err(&bad).code, "note_has_value");
    }
    assert_eq!(check_note(&"x".repeat(201), NOTE_MAX, v).expect_err("long").code, "note_too_long");
    check_note(&"x".repeat(2000), MSG_NOTE_MAX, v).expect("msg notes are longer");
}

// ── 3. landing rules ────────────────────────────────────────────────

#[test]
fn pending_by_default_and_allow_listed_lands_never_api() {
    let t = open("ho-land");
    let v = &t.v;
    // Not allow-listed: pending, value held encrypted, nothing in the vault.
    let r = v.handoff_receive(incoming("ws-ops", item("STRIPE_KEY"), "sales", false), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("pend");
    assert_eq!(r.record.status, "pending");
    assert_eq!(r.record.reason.as_deref(), Some("not_allowed"));
    assert!(r.meta.is_none());
    assert!(v.handoff_has_value(&r.record.delivery_id));
    assert!(v.meta(&Selector::user(Scope::workspace("ws-ops"), "STRIPE_KEY"), &Actor::Owner).is_err(), "nothing landed");

    // Allow-listed: lands straight in, workspace scope, never api (H12).
    let r = v.handoff_receive(incoming("ws-ops", item("OTHER_KEY"), "sales", true), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("land");
    assert_eq!(r.record.status, "landed");
    let m = r.meta.expect("meta");
    assert_eq!(m.scope, Scope::workspace("ws-ops"));
    assert_eq!(m.default_delivery, Delivery::Env, "the sender's own delivery (api), capped at env");
    assert_eq!(m.delivery_ceiling, Delivery::Env, "the ceiling is at most env, never api");
    assert!(!m.human_given, "H10");
    assert_eq!(m.created_by, "system:handoff");
    assert!(m.origin.starts_with("handoff:hd_"));
    assert!(m.description.starts_with("From sales"), "{}", m.description);
    assert_eq!(value(v, "ws-ops", "OTHER_KEY"), SENTINEL.as_bytes());
    // An agent `get` is refused (no api delivery) …
    let got = v.resolve(&Selector::user(Scope::workspace("ws-ops"), "OTHER_KEY"), Use::Api, &Requester::Session { workspace_id: "ws-ops".into(), session_id: None });
    let Err(e) = got else { panic!("an agent got an api value of a handed-off copy") };
    assert_eq!(e.code, "delivery_not_granted");
    // … and the receiving agent can't widen it (H12, test 4).
    let e = v
        .set_policy(&Selector::Id(m.id.clone()), PolicyPatch { default_delivery: Some(Delivery::Api), delivery_ceiling: Some(Delivery::Api), ..Default::default() }, &agent("ws-ops"))
        .expect_err("agent widen");
    assert!(matches!(e.code, "vault_owner_confirm_required" | "vault_not_yours"), "{}", e.code);

    // No sites: file delivery.
    let mut it = item("SSH_DEPLOY");
    it.hosts.clear();
    it.kind = "ssh_key".into();
    let key = crate::ssh_agent::testkeys::ed25519();
    let r = v.handoff_receive(incoming("ws-ops", it, "sales", true), SecretBytes::from_slice(key.as_bytes())).expect("file");
    assert_eq!(r.meta.expect("meta").default_delivery, Delivery::File);

    // Item 3: an ssh key the sender served only through the agent lands as
    // agent (never readable), and must be a key the agent can serve.
    let mut it = item("SSH_AGENT_ONLY");
    it.hosts.clear();
    it.kind = "ssh_key".into();
    it.delivery = "agent".into();
    it.ceiling = "env".into();
    assert_eq!(it.landing_delivery(), (Delivery::Agent, Delivery::Env));
    let key = crate::ssh_agent::testkeys::ed25519();
    let r = v.handoff_receive(incoming("ws-ops", it, "sales", true), SecretBytes::from_slice(key.as_bytes())).expect("agent");
    let m = r.meta.expect("meta");
    assert_eq!(m.default_delivery, Delivery::Agent);
    assert!(!m.agent_can_see());
    // An agent delivery on any other kind isn't kept.
    let mut it = item("NOT_SSH");
    it.hosts.clear();
    it.delivery = "agent".into();
    assert_eq!(it.landing_delivery().0, Delivery::File);
}

fn ops_session() -> Requester {
    Requester::Session { workspace_id: "ws-ops".into(), session_id: Some("s-ops".into()) }
}

/// Bug 1 (2026-10-28): a cross-server send of each kind lands in a form
/// the receiving agent can use. Each item is sealed on A and opened on B
/// (the real `k2-vault-handoff/1` seal), landed, and then used the way
/// the agent would: fetch to its site, a file (`k2 vault file`), env
/// (`k2 vault env-file`), or the ssh agent. The incident: an agent-made
/// api_key with fetch delivery and no sites landed `file (at most env)`
/// and nothing could read it; it now lands `file`, readable as a file.
#[test]
fn a_cross_server_send_of_each_kind_lands_with_a_usable_delivery() {
    let (a, b) = (open("ho-x-a"), open("ho-x-b"));
    let ssh = crate::ssh_agent::testkeys::ed25519();
    let stripe = vec!["https://api.stripe.com".to_string()];
    // (name, kind, sender delivery, sender ceiling, sites, value, lands as)
    let cases: Vec<(&str, &str, &str, &str, Vec<String>, String, Delivery)> = vec![
        ("ENROLL_CODE", "api_key", "fetch", "env", vec![], "ABCDE-FGHJK.0123456789abcdef".into(), Delivery::File),
        ("STRIPE_KEY", "api_key", "fetch", "env", stripe.clone(), "sk_test_k2test_handoff_1".into(), Delivery::Fetch),
        ("GH_TOKEN", "oauth_token", "fetch", "env", vec![], "gho_k2test_handoff_2".into(), Delivery::File),
        ("BLOB", "generic", "file", "env", vec![], "k2test-generic-3".into(), Delivery::File),
        ("SIGNER", "signing_key", "fetch", "env", vec![], "k2test-signing-4".into(), Delivery::File),
        ("HMAC", "hmac_key", "fetch", "env", stripe.clone(), "k2test-hmac-5".into(), Delivery::Fetch),
        ("DB_PASSWORD", "password", "env", "env", vec![], "k2test-password-6".into(), Delivery::Env),
        ("DATABASE_URL", "dsn", "env", "env", vec![], "postgres://u:k2test7@db.example.com/x".into(), Delivery::Env),
        ("HOOK_URL", "url", "env", "env", vec![], "https://hooks.example.com/k2test8".into(), Delivery::Env),
        ("API_RAW", "api_key", "api", "api", vec![], "k2test-api-raw-9".into(), Delivery::Env),
        ("DEPLOY_KEY", "ssh_key", "file", "env", vec![], ssh.clone(), Delivery::File),
        ("TLS_KEY", "tls_key", "file", "env", vec![], "-----BEGIN PRIVATE KEY-----\nk2test10\n-----END PRIVATE KEY-----\n".into(), Delivery::File),
        ("SIGN_ONLY", "ssh_key", "agent", "agent", vec![], ssh.clone(), Delivery::Agent),
    ];
    for (name, kind, d, c, hosts, val, want) in cases {
        let it = HandoffItem {
            name: name.into(),
            as_name: None,
            kind: kind.into(),
            hosts: hosts.clone(),
            inject: None,
            description: String::new(),
            delivery: d.into(),
            ceiling: c.into(),
            human_given: false,
            ttl_secs: None,
        };
        let (file, id) = sealed_between(&a.v, &b.v, |p| {
            p.item = it.clone();
            p.value_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, val.as_bytes());
        });
        let p = b.v.handoff_open(&file, expect(&id)).unwrap_or_else(|e| panic!("{name}: opens: {e}"));
        let mut inc = incoming("ws-ops", p.item.clone(), "sales::rosson.k2.dev", true);
        inc.delivery_id = id.clone();
        let r = b.v.handoff_receive(inc, p.value().expect("value")).unwrap_or_else(|e| panic!("{name}: lands: {e}"));
        let m = r.meta.unwrap_or_else(|| panic!("{name}: landed {:?}", r.record.status));
        assert_eq!(m.default_delivery, want, "{name} ({kind}, sent as {d}) lands as {want}");
        assert_ne!(m.delivery_ceiling, Delivery::Api, "{name}: never api");
        assert!(m.default_delivery.level() <= m.delivery_ceiling.level(), "{name}: delivery within its ceiling");
        let sel = Selector::user(Scope::workspace("ws-ops"), name);
        // Used the way the agent would.
        match want {
            Delivery::Fetch => {
                let o = crate::hosts::Origin::parse_url("https://api.stripe.com/v1/x").expect("origin").1;
                let got = b.v.resolve(&sel, Use::Fetch(&o), &ops_session()).unwrap_or_else(|e| panic!("{name}: fetch: {e}"));
                assert_eq!(got.value.expose(), val.as_bytes());
            }
            Delivery::File | Delivery::Env => {
                let got = b.v.resolve(&sel, Use::File, &ops_session()).unwrap_or_else(|e| panic!("{name}: file (k2 vault file): {e}"));
                assert_eq!(got.value.expose().trim_ascii(), val.trim().as_bytes(), "{name}");
                if want == Delivery::Env {
                    b.v.resolve(&sel, Use::Env, &ops_session()).unwrap_or_else(|e| panic!("{name}: env (k2 vault env-file): {e}"));
                }
            }
            Delivery::Agent => {
                assert!(!m.agent_can_see(), "{name}: an agent-only key stays unreadable");
                let e = b.v.resolve(&sel, Use::File, &ops_session()).err().expect("not as a file");
                assert_eq!(e.code, "delivery_not_granted", "{name}");
            }
            other => panic!("{name}: unexpected {other}"),
        }
        // Never api, whatever the sender had.
        assert_eq!(b.v.resolve(&sel, Use::Api, &ops_session()).err().expect("never api").code, "delivery_not_granted", "{name}");
    }
}

/// Bug 2: `k2 vault incoming accept <id> --replace` takes the INCOMING
/// delivery (capped as any landing is), not the existing record's.
/// Widening the current one needs what `k2 vault delivery` needs: an
/// Owner or Admin. A refused replace changes nothing.
#[test]
fn replace_takes_the_incoming_delivery_and_widening_needs_an_owner_or_admin() {
    let t = open("ho-replace");
    let v = &t.v;
    // ops already has ENROLL_CODE: fetch only, no sites (unusable).
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws-ops"), "ENROLL_CODE", Kind::ApiKey);
    r.default_delivery = Some(Delivery::Fetch);
    r.delivery_ceiling = Some(Delivery::Fetch);
    v.put(r, SecretBytes::from_slice(b"k2test-old-code-1"), &agent("ws-ops")).expect("put");
    let sel = Selector::user(Scope::workspace("ws-ops"), "ENROLL_CODE");
    // sales sends ENROLL_CODE as env: a clash, pending.
    let mut it = item("ENROLL_CODE");
    it.hosts.clear();
    it.delivery = "env".into();
    it.ceiling = "env".into();
    let p = v.handoff_receive(incoming("ws-ops", it, "sales::rosson.k2.dev", true), SecretBytes::from_slice(b"k2test-new-code-2")).expect("pend");
    assert_eq!(p.record.reason.as_deref(), Some("clash"));
    let id = p.record.delivery_id.clone();
    let replace = Decision { action: "accept".into(), as_name: None, replace: true };

    // A Member: replacing would widen fetch → env. Refused; nothing moved.
    let member = Actor::Login { username: "mo".into(), role: Role::Member };
    let e = v.handoff_decide(&id, &replace, &member).expect_err("a Member can't widen");
    assert!(matches!(e.code, "vault_admin_required" | "vault_not_yours"), "{}", e.code);
    assert!(e.hint.contains("would change its delivery from fetch (at most fetch) to env (at most env)"), "{}", e.hint);
    let m = v.meta(&sel, &Actor::Owner).expect("meta");
    assert_eq!((m.default_delivery, m.delivery_ceiling, m.latest_version), (Delivery::Fetch, Delivery::Fetch, 1), "unchanged");
    assert_eq!(value(v, "ws-ops", "ENROLL_CODE"), b"k2test-old-code-1");
    assert_eq!(v.handoff_get(&id, "in").expect("row").status, "pending", "still waiting");

    // The owner replaces: the new value AND the incoming delivery.
    let d = v.handoff_decide(&id, &replace, &Actor::Owner).expect("owner replaces");
    let m = d.meta.expect("meta");
    assert_eq!((m.default_delivery, m.delivery_ceiling, m.latest_version), (Delivery::Env, Delivery::Env, 2));
    assert_eq!(value(v, "ws-ops", "ENROLL_CODE"), b"k2test-new-code-2");
    v.resolve(&sel, Use::Env, &ops_session()).expect("the agent can use it as env now");

    // A narrowing replace needs no widen right: a Member may do it.
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws-ops"), "WIDE_ONE", Kind::Password);
    r.default_delivery = Some(Delivery::Env);
    v.put(r, SecretBytes::from_slice(b"k2test-wide-3"), &Actor::Owner).expect("put");
    let mut it = item("WIDE_ONE");
    it.kind = "password".into();
    it.hosts.clear();
    it.delivery = "file".into();
    it.ceiling = "file".into();
    let p = v.handoff_receive(incoming("ws-ops", it, "billing", true), SecretBytes::from_slice(b"k2test-narrow-4")).expect("pend");
    let d = v.handoff_decide(&p.record.delivery_id, &replace, &member).expect("a narrowing replace");
    let m = d.meta.expect("meta");
    assert_eq!((m.default_delivery, m.delivery_ceiling), (Delivery::File, Delivery::File), "the incoming (narrower) delivery applies");
}

/// Bug 3: the sender's `--ttl` reaches the receiver: a pending copy (and
/// its card) never outlives it, the landed copy expires `--ttl` after it
/// was sent (not after the click), and a same-sender re-send carries its
/// own `--ttl` but leaves the copy's delivery alone (only Replace, by a
/// person, takes the incoming delivery).
#[test]
fn the_senders_ttl_is_carried_to_the_pending_card_and_the_copy() {
    let t = open("ho-ttl");
    let v = &t.v;
    let at = |s: &Option<String>| chrono::DateTime::parse_from_rfc3339(s.as_deref().expect("a time")).expect("rfc3339").timestamp();
    let mut it = item("SHORT_LIVED");
    it.ttl_secs = Some(3600);
    let p = v.handoff_receive(incoming("ws-ops", it, "sales::rosson.k2.dev", false), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("pend");
    let created = at(&Some(p.record.created_at.clone()));
    assert_eq!(at(&p.record.expires_at) - created, 3600, "the card says 1 hour, not 7 days");
    let d = v.handoff_decide(&p.record.delivery_id, &Decision { action: "accept".into(), ..Default::default() }, &Actor::Owner).expect("accept");
    let m = d.meta.expect("meta");
    assert!((at(&m.expires_at) - (created + 3600)).abs() <= 1, "the copy ends 1 h after it was sent: {:?} vs {created}", m.expires_at);
    assert_eq!(d.record.expires_at, m.expires_at);

    // No --ttl: pending waits 7 days, the copy never expires.
    let p = v.handoff_receive(incoming("ws-ops", item("FOREVER"), "sales::rosson.k2.dev", false), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("pend");
    assert_eq!(at(&p.record.expires_at) - at(&Some(p.record.created_at.clone())), PENDING_DAYS * 86400);
    let d = v.handoff_decide(&p.record.delivery_id, &Decision { action: "accept".into(), ..Default::default() }, &Actor::Owner).expect("accept");
    assert_eq!(d.meta.expect("meta").expires_at, None);
    // A --ttl longer than the wait: pending 7 days, the copy keeps 30.
    let mut it = item("LONG_LIVED");
    it.ttl_secs = Some(30 * 86400);
    let p = v.handoff_receive(incoming("ws-ops", it, "sales::rosson.k2.dev", false), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("pend");
    assert_eq!(at(&p.record.expires_at) - at(&Some(p.record.created_at.clone())), PENDING_DAYS * 86400);

    // Same sender, allow-listed: v1 lands (env); the owner narrows it to
    // fetch; a re-send with --ttl takes the --ttl but keeps the owner's
    // fetch (a re-send never reverts a person's choice, nor widens).
    let l1 = v.handoff_receive(incoming("ws-ops", item("ROT_KEY"), "sales", true), SecretBytes::from_slice(b"k2test-rot-1")).expect("v1");
    let m1 = l1.meta.expect("meta");
    assert_eq!((m1.default_delivery, m1.expires_at.clone()), (Delivery::Env, None));
    v.set_policy(
        &Selector::Id(m1.id.clone()),
        PolicyPatch { default_delivery: Some(Delivery::Fetch), delivery_ceiling: Some(Delivery::Fetch), ..Default::default() },
        &Actor::Owner,
    )
    .expect("the owner narrows it");
    let mut again = item("ROT_KEY");
    again.ttl_secs = Some(7200);
    let l2 = v.handoff_receive(incoming("ws-ops", again, "sales", true), SecretBytes::from_slice(b"k2test-rot-2")).expect("v2");
    let m2 = l2.meta.expect("meta");
    assert_eq!((m2.latest_version, m2.default_delivery, m2.delivery_ceiling), (2, Delivery::Fetch, Delivery::Fetch), "the owner's choice stays");
    assert!((at(&m2.expires_at) - at(&Some(l2.record.created_at.clone())) - 7200).abs() <= 1, "{:?}", m2.expires_at);
    assert_eq!(l2.record.expires_at, m2.expires_at, "the row shows the copy's expiry");
    assert_eq!(value(v, "ws-ops", "ROT_KEY"), b"k2test-rot-2");
}

#[test]
fn landing_delivery_rule_table() {
    let mk = |kind: &str, d: &str, c: &str, sites: bool| HandoffItem {
        name: "X".into(),
        as_name: None,
        kind: kind.into(),
        hosts: if sites { vec!["https://api.stripe.com".into()] } else { vec![] },
        inject: None,
        description: String::new(),
        delivery: d.into(),
        ceiling: c.into(),
        human_given: false,
        ttl_secs: None,
    };
    use Delivery as D;
    for (kind, d, c, sites, want) in [
        ("api_key", "fetch", "env", true, (D::Fetch, D::Env)),
        ("api_key", "fetch", "env", false, (D::File, D::Env)),
        ("api_key", "fetch", "fetch", false, (D::Fetch, D::Fetch)),
        ("generic", "file", "env", false, (D::File, D::Env)),
        ("password", "env", "env", false, (D::Env, D::Env)),
        ("password", "env", "file", false, (D::File, D::File)),
        ("api_key", "api", "api", true, (D::Env, D::Env)),
        ("ssh_key", "agent", "env", false, (D::Agent, D::Env)),
        ("ssh_key", "env", "env", false, (D::File, D::Env)),
        ("tls_key", "fetch", "env", true, (D::File, D::Env)),
    ] {
        assert_eq!(mk(kind, d, c, sites).landing_delivery(), want, "{kind} {d}/{c} sites={sites}");
    }
    assert!(use_hint(D::File, "X").contains("k2 vault file --secret X"));
    assert!(use_hint(D::Fetch, "X").contains("k2 vault fetch --secret X"));
    assert!(use_hint(D::Env, "X").contains("k2 vault env-file --secret X"));
}

#[test]
fn wildcard_body_inject_person_given_and_remote_off_always_pend() {
    let t = open("ho-pend");
    let v = &t.v;
    let mut wild = item("WILD");
    wild.hosts = vec!["https://*.stripe.com".into()];
    let mut body = item("BODY");
    body.inject = Some(crate::inject::Inject { dst: crate::inject::Dst::Body, fmt: "{}".into(), allowed_dst: vec![], allowed_fmt: vec![], sign: None });
    let mut person = item("PERSON");
    person.human_given = true;
    for (it, reason) in [(wild, "needs_person"), (body, "needs_person"), (person, "person_given")] {
        let r = v.handoff_receive(incoming("ws-ops", it, "sales", true), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("pend");
        assert_eq!(r.record.status, "pending");
        assert_eq!(r.record.reason.as_deref(), Some(reason));
    }
    let mut inc = incoming("ws-ops", item("REMOTE"), "sales::rosson.k2.dev", true);
    inc.remote_off = true;
    let r = v.handoff_receive(inc, SecretBytes::from_slice(SENTINEL.as_bytes())).expect("pend");
    assert_eq!(r.record.reason.as_deref(), Some("remote_instruction_off"));

    // A person's accept lands the wildcard item as that person (H11).
    let wild_id = v.handoffs(Some("ws-ops"), Some("in")).expect("list").into_iter().find(|r| r.name == "WILD").expect("wild").delivery_id;
    let admin = Actor::Login { username: "lee".into(), role: Role::Admin };
    let d = v.handoff_decide(&wild_id, &Decision { action: "accept".into(), ..Default::default() }, &admin).expect("accept");
    assert_eq!(d.record.status, "landed");
    assert_eq!(d.record.decided_by.as_deref(), Some("connect:lee"));
    let m = d.meta.expect("meta");
    assert!(!m.human_given, "a person accepting doesn't make it person-given (H10)");
    assert!(!v.handoff_has_value(&wild_id), "the pending value row is gone");
}

#[test]
fn clash_pends_and_a_same_sender_resend_is_a_new_version() {
    let t = open("ho-clash");
    let v = &t.v;
    // The receiver already has its own STRIPE_KEY: a send never overwrites it.
    put(v, "ws-ops", "STRIPE_KEY", "the-receivers-own-key-1", &agent("ws-ops"));
    let r = v.handoff_receive(incoming("ws-ops", item("STRIPE_KEY"), "sales", true), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("clash");
    assert_eq!(r.record.status, "pending");
    assert_eq!(r.record.reason.as_deref(), Some("clash"));
    assert_eq!(value(v, "ws-ops", "STRIPE_KEY"), b"the-receivers-own-key-1", "never overwritten");
    // Accepting needs a new name or an explicit replace.
    let owner = Actor::Owner;
    let e = v.handoff_decide(&r.record.delivery_id, &Decision { action: "accept".into(), ..Default::default() }, &owner).expect_err("taken");
    assert_eq!(e.code, "name_taken");
    let d = v
        .handoff_decide(&r.record.delivery_id, &Decision { action: "accept".into(), as_name: Some("STRIPE_KEY_SALES".into()), replace: false }, &owner)
        .expect("accept as");
    assert_eq!(d.record.landed_name.as_deref(), Some("STRIPE_KEY_SALES"));
    assert_eq!(value(v, "ws-ops", "STRIPE_KEY_SALES"), SENTINEL.as_bytes());

    // Same sender, same secret, allow-listed: v1 then v2 (rotation).
    let first = v.handoff_receive(incoming("ws-ops", item("DB_URL_KEY"), "sales", true), SecretBytes::from_slice(b"first-value-123")).expect("v1");
    assert_eq!(first.meta.as_ref().expect("m").latest_version, 1);
    let again = v.handoff_receive(incoming("ws-ops", item("DB_URL_KEY"), "sales", true), SecretBytes::from_slice(b"second-value-456")).expect("v2");
    assert_eq!(again.record.status, "landed");
    assert_eq!(again.record.reason.as_deref(), Some("new_version"));
    assert_eq!(again.meta.as_ref().expect("m").latest_version, 2);
    assert_eq!(value(v, "ws-ops", "DB_URL_KEY"), b"second-value-456");
    // A different sender with the same name: a clash, pending.
    let other = v.handoff_receive(incoming("ws-ops", item("DB_URL_KEY"), "billing", true), SecretBytes::from_slice(b"third-value-789")).expect("other");
    assert_eq!(other.record.reason.as_deref(), Some("clash"));
    // Same sender but not allow-listed any more: pending re-send.
    let resend = v.handoff_receive(incoming("ws-ops", item("DB_URL_KEY"), "sales", false), SecretBytes::from_slice(b"fourth-value-000")).expect("resend");
    assert_eq!(resend.record.reason.as_deref(), Some("resend"));
    assert_eq!(value(v, "ws-ops", "DB_URL_KEY"), b"second-value-456");
}

#[test]
fn pending_caps_per_sender_and_per_workspace() {
    let t = open("ho-caps");
    let v = &t.v;
    for i in 0..PENDING_PER_SENDER {
        v.handoff_receive(incoming("ws-ops", item(&format!("K{i}")), "sales", false), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("pend");
    }
    let e = v.handoff_receive(incoming("ws-ops", item("K9"), "sales", false), SecretBytes::from_slice(SENTINEL.as_bytes())).expect_err("cap");
    assert_eq!((e.code, e.status), ("too_many_pending", 429));
    v.handoff_receive(incoming("ws-ops", item("K9"), "billing", false), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("another sender still fits");
}

// ── 4. people decide ────────────────────────────────────────────────

#[test]
fn passports_guests_and_system_cannot_decide_or_edit_accept_from() {
    let t = open("ho-who");
    let v = &t.v;
    let r = v.handoff_receive(incoming("ws-ops", item("K"), "sales", false), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("pend");
    let id = r.record.delivery_id;
    let shell = Actor::Agent(AgentActor { workspace_id: "ws-ops".into(), session_id: None, shell: true, vault_manage: true });
    for a in [agent("ws-ops"), shell, Actor::Guest { username: "g".into(), workspace_id: "ws-ops".into() }, Actor::System("x".into())] {
        let e = v.handoff_decide(&id, &Decision { action: "accept".into(), ..Default::default() }, &a).expect_err("not a person");
        assert_eq!(e.code, "handoff_person_only");
        assert_eq!(v.accept_from_add("ws-ops", "ws:ws-sales", &a).expect_err("allow-list").code, "handoff_person_only");
    }
    assert_eq!(v.handoff_get(&id, "in").expect("row").status, "pending");
    let d = v.handoff_decide(&id, &Decision { action: "decline".into(), ..Default::default() }, &Actor::Owner).expect("decline");
    assert_eq!(d.record.status, "declined");
    assert!(!v.handoff_has_value(&id));
    assert_eq!(v.handoff_decide(&id, &Decision { action: "accept".into(), ..Default::default() }, &Actor::Owner).expect_err("twice").code, "not_pending");

    // People edit the list; entries are normalised; no wildcard.
    let a = v.accept_from_add("ws-ops", "Sales::Rosson.K2.dev", &Actor::Owner).expect("add");
    assert_eq!(a.sender, "sales::rosson.k2.dev");
    assert!(v.accepts_from("ws-ops", &["sales::rosson.k2.dev".into()]));
    assert!(!v.accepts_from("ws-other", &["sales::rosson.k2.dev".into()]));
    assert_eq!(v.accept_from_add("ws-ops", "*", &Actor::Owner).expect_err("star").code, "no_wildcard");
    assert!(v.accept_from_remove("ws-ops", "sales@rosson.k2.dev", &Actor::Owner).expect("remove"));
    assert!(!v.accepts_from("ws-ops", &["sales::rosson.k2.dev".into()]));
}

// ── 5. durable replay ───────────────────────────────────────────────

#[test]
fn the_same_delivery_id_twice_is_a_replay_with_no_new_version() {
    let t = open("ho-replay");
    let v = &t.v;
    let inc = incoming("ws-ops", item("K"), "sales", true);
    v.handoff_receive(inc.clone(), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("first");
    let e = v.handoff_receive(inc, SecretBytes::from_slice(b"another-value-xyz")).expect_err("replay");
    assert_eq!((e.code, e.status), ("replay", 409));
    let m = v.meta(&Selector::user(Scope::workspace("ws-ops"), "K"), &Actor::Owner).expect("meta");
    assert_eq!(m.latest_version, 1);
    assert_eq!(value(v, "ws-ops", "K"), SENTINEL.as_bytes());
}

// ── 6. tick ─────────────────────────────────────────────────────────

#[test]
fn tick_expires_pending_values_and_destroys_expired_landed_copies() {
    let t = open("ho-tick");
    let v = &t.v;
    let p = v.handoff_receive(incoming("ws-ops", item("PEND"), "sales", false), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("pend");
    let mut it = item("TTL_KEY");
    it.ttl_secs = Some(TTL_MIN_SECS);
    let l = v.handoff_receive(incoming("ws-ops", it, "sales", true), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("land");
    let m = l.meta.expect("meta");
    assert!(m.expires_at.is_some(), "--ttl sets the landed copy's expiry");
    assert!(v.handoff_tick().expect("tick").is_empty(), "nothing due yet");

    v.handoff_backdate_for_tests(&p.record.delivery_id, "in").expect("backdate");
    v.expire_secret_for_tests(&m.id).expect("expire copy");
    let expired = v.handoff_tick().expect("tick");
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].status, "expired");
    assert!(!v.handoff_has_value(&p.record.delivery_id), "the value row is deleted");
    let gone = v.meta(&Selector::Id(m.id.clone()), &Actor::Owner).expect("meta");
    assert_eq!(gone.status, SecretStatus::Destroyed, "H13: an expired handoff copy is destroyed");
    let e = v.handoff_decide(&p.record.delivery_id, &Decision { action: "accept".into(), ..Default::default() }, &Actor::Owner).expect_err("expired");
    assert_eq!(e.code, "not_pending");
}

// ── 7. schema ───────────────────────────────────────────────────────

#[test]
fn tables_are_created_on_open_and_user_version_stays_one() {
    let d = TestDir::new("ho-schema");
    let ks: Arc<dyn KeyStore> = Arc::new(FakeKeyStore::new());
    let dir = d.path().join("vault");
    {
        let v = LocalVault::open(&dir, ks.clone(), OpenOptions { kdf: KdfCost::Floor }).expect("open");
        v.accept_from_add("ws-ops", "ws:ws-sales", &Actor::Owner).expect("row");
    }
    let conn = rusqlite::Connection::open(dir.join("vault.db")).expect("sqlite");
    let ver: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).expect("version");
    assert_eq!(ver, 1, "H22: no bump, so a 0.46.0 daemon still opens this vault");
    for t in ["handoff", "handoff_value", "handoff_accept", "peer_vault"] {
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1", [t], |r| r.get(0)).expect("q");
        assert_eq!(n, 1, "{t}");
    }
    // 0.46.0's migrate is the v1 batch only: it runs nothing on a v1 file
    // and never touches the extra tables. Prove the file still passes the
    // version gate and reopens with the rows intact.
    assert!(ver <= crate::db::SCHEMA_VERSION);
    drop(conn);
    let v = LocalVault::open(&dir, ks, OpenOptions { kdf: KdfCost::Floor }).expect("reopen");
    assert!(v.accepts_from("ws-ops", &["ws:ws-sales".into()]));
}

#[test]
fn peer_vault_cache_reports_a_changed_recipient() {
    let (t, other) = (open("ho-peer"), open("ho-peer-2"));
    let r1 = t.v.carry_recipient().expect("r").encode();
    let r2 = other.v.carry_recipient().expect("r").encode();
    let p = PeerVault { peer_fp: "fp1".into(), vault_id: "v1".into(), recipient: r1, features: vec![FEATURE.into()], fetched_at: "2026-10-17T00:00:00Z".into() };
    assert!(t.v.peer_vault_put(&p).expect("put").is_none());
    assert_eq!(t.v.peer_vault("fp1").expect("cached"), p);
    let p2 = PeerVault { recipient: r2, vault_id: "v2".into(), ..p.clone() };
    assert_eq!(t.v.peer_vault_put(&p2).expect("put").expect("changed"), p);
    t.v.peer_vault_forget("fp1").expect("forget");
    assert!(t.v.peer_vault("fp1").is_none());
}

// ── H5: asks and unsend ─────────────────────────────────────────────

fn ask_row(ws: &str, peer: &str, name: &str) -> HandoffRecord {
    HandoffRecord {
        delivery_id: new_ask_id(),
        direction: "ask_out".into(),
        status: String::new(),
        local_workspace_id: ws.into(),
        peer_addr: peer.into(),
        peer_fp: None,
        sender_actor: "agent:ws-ops/s1".into(),
        source_vault_id: None,
        name: name.into(),
        landed_name: None,
        kind: "api_key".into(),
        note: String::new(),
        reply_to: None,
        item: item(name),
        secret_id: None,
        version: None,
        reason: None,
        card: None,
        created_at: String::new(),
        decided_at: None,
        decided_by: None,
        expires_at: None,
    }
}

#[test]
fn an_open_ask_matches_only_its_peer_and_name_and_closes() {
    let t = open("ho-ask");
    let v = &t.v;
    let a = v.handoff_record_ask(ask_row("ws-ops", "sales", "STRIPE_KEY")).expect("ask");
    assert!(valid_ask_id(&a.delivery_id));
    assert_eq!(a.status, "open");
    assert!(v.handoff_ask_matches(&a.delivery_id, "ws-ops", "sales", "STRIPE_KEY"));
    assert!(!v.handoff_ask_matches(&a.delivery_id, "ws-ops", "billing", "STRIPE_KEY"), "another sender");
    assert!(!v.handoff_ask_matches(&a.delivery_id, "ws-ops", "sales", "OTHER"), "another name");
    assert!(!v.handoff_ask_matches(&a.delivery_id, "ws-x", "sales", "STRIPE_KEY"), "another workspace");
    assert!(v.handoff_close_ask(&a.delivery_id, "ask_out", "answered").expect("close"));
    assert!(!v.handoff_ask_matches(&a.delivery_id, "ws-ops", "sales", "STRIPE_KEY"), "answered once");
    // Expiry closes an ask.
    let b = v.handoff_record_ask(ask_row("ws-ops", "sales", "K2")).expect("ask");
    v.handoff_backdate_for_tests(&b.delivery_id, "ask_out").expect("backdate");
    assert!(!v.handoff_ask_matches(&b.delivery_id, "ws-ops", "sales", "K2"), "past 7 days");
    v.handoff_tick().expect("tick");
    assert_eq!(v.handoff_get(&b.delivery_id, "ask_out").expect("row").status, "expired");
}

#[test]
fn unsend_disables_the_copy_it_wrote_and_leaves_a_replacement_alone() {
    let t = open("ho-unsend");
    let v = &t.v;
    let mut inc = incoming("ws-ops", item("K_UNSEND"), "sales::rosson.k2.dev", true);
    inc.peer_fp = Some("fp-sales".into());
    let r = v.handoff_receive(inc, SecretBytes::from_slice(SENTINEL.as_bytes())).expect("land");
    let id = r.record.delivery_id.clone();
    // Another server can't take it back.
    assert_eq!(v.handoff_unsend(&id, Some("fp-mallory"), "x").expect_err("peer").code, "not_yours");
    let (row, disabled) = v.handoff_unsend(&id, Some("fp-sales"), "handoff:sales::rosson.k2.dev").expect("unsend");
    assert!(disabled);
    assert_eq!(row.status, "unsent");
    let m = v.meta(&Selector::user(Scope::workspace("ws-ops"), "K_UNSEND"), &Actor::Owner).expect("meta");
    assert_eq!(m.status, SecretStatus::Disabled);
    // A copy the owner replaced since is left alone.
    let r = v.handoff_receive(incoming("ws-ops", item("K_KEEP"), "sales", true), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("land");
    v.rotate(&Selector::user(Scope::workspace("ws-ops"), "K_KEEP"), SecretBytes::from_slice(b"owners-own-new-value"), &Actor::Owner).expect("rotate");
    let (row, disabled) = v.handoff_unsend(&r.record.delivery_id, None, "agent:ws-sales/s").expect("unsend");
    assert!(!disabled);
    assert_eq!(row.reason.as_deref(), Some("unsent_kept_replaced"));
    assert_eq!(value(v, "ws-ops", "K_KEEP"), b"owners-own-new-value");
    // A pending copy is dropped with its value.
    let p = v.handoff_receive(incoming("ws-ops", item("K_PEND"), "sales", false), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("pend");
    let (row, _) = v.handoff_unsend(&p.record.delivery_id, None, "agent:ws-sales/s").expect("unsend");
    assert_eq!(row.status, "unsent");
    assert!(!v.handoff_has_value(&p.record.delivery_id));
    assert_eq!(v.handoff_unsend(&p.record.delivery_id, None, "x").expect_err("twice").code, "not_unsendable");
}

// ── rekey covers every MAC'd table ──────────────────────────────────

/// A rekey verifies and re-MACs every MAC'd table (secrets, versions,
/// grants, cards, placements, handoffs, pending handoff values (re-sealed),
/// accept-from, peer vaults, agent settings and agent socket records), so
/// everything still reads after it; a tampered grant stops the rekey
/// instead of coming out valid under the new key.
#[test]
fn rekey_keeps_every_table_and_stops_on_a_tampered_row() {
    let t = open("ho-rekey");
    let v = &t.v;
    let owner = Actor::Owner;
    let proof = crate::policy::HumanProof::Confirmed { method: "t".into() };
    // A secret with two versions, a grant, a card, a placement.
    let a = put(v, "ws-ops", "STRIPE_KEY", "k2test-rekey-value-1", &owner);
    v.rotate(&Selector::Id(a.id.clone()), SecretBytes::from_slice(b"k2test-rekey-value-2"), &owner).expect("rotate");
    v.grant(
        crate::store::GrantRequest {
            selector: Selector::Id(a.id.clone()),
            consumer: crate::model::Consumer::workspace("ws-sales"),
            delivery: Delivery::Fetch,
            env_name: None,
            file_name: None,
            hosts_narrowed: None,
            expires_at: None,
            version: None,
        },
        &owner,
    )
    .expect("grant");
    v.request_create(crate::vault::SecretRequest {
        card_id: "card-rekey-0123456789".into(),
        conversation: "conv-1".into(),
        session_id: Some("sess-1".into()),
        workspace_id: "ws-ops".into(),
        requested_by: "agent:ws-ops/sess-1".into(),
        name: "CARD_KEY".into(),
        kind: Kind::ApiKey,
        proposed_scope: "workspace".into(),
        proposed_hosts: vec!["api.stripe.com".into()],
        proposed_delivery: Delivery::Fetch,
        prompt: "Paste it".into(),
        ttl_secs: None,
        status: String::new(),
        answered_by: None,
        secret_id: None,
        created_at: String::new(),
        answered_at: None,
        inject: None,
    })
    .expect("card");
    v.placement_add(
        crate::vault::NewPlacement {
            secret_id: a.id.clone(),
            target: crate::vault::PlacementTarget::Manual,
            unit: None,
            cred_name: None,
            where_text: Some("box1:/etc/x".into()),
            note: String::new(),
            restart: false,
        },
        "owner",
    )
    .expect("placement");
    // A pending handoff (row + sealed value), an accept-from row, a peer vault.
    let pend = v.handoff_receive(incoming("ws-ops", item("PENDING_KEY"), "sales", false), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("pend");
    assert_eq!(pend.record.status, "pending");
    v.accept_from_add("ws-ops", "Sales::Rosson.K2.dev", &owner).expect("accept-from");
    let other = open("ho-rekey-peer");
    let peer = PeerVault {
        peer_fp: "fp-rekey".into(),
        vault_id: "v-peer".into(),
        recipient: other.v.carry_recipient().expect("r").encode(),
        features: vec![FEATURE.into(), FEATURE_AGENT.into()],
        fetched_at: "2026-10-24T00:00:00Z".into(),
    };
    v.peer_vault_put(&peer).expect("peer");
    // Agent settings and an agent socket record.
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws-ops"), "DEPLOY", Kind::SshKey);
    r.default_delivery = Some(Delivery::Agent);
    let k = v.put(r, SecretBytes::from_slice(crate::ssh_agent::testkeys::ed25519().as_bytes()), &owner).expect("ssh").meta;
    v.set_agent_policy(&Selector::Id(k.id.clone()), crate::vault::AgentPolicyPatch { burst: Some(7), ..Default::default() }, &owner).expect("policy");
    let sock = crate::vault::AgentSocket {
        tag: "sess-rekey".into(),
        kind: "session".into(),
        workspace_id: "ws-ops".into(),
        secrets_dir: "/run/k2/secrets/sess-rekey-abcdefgh".into(),
        sock_path: "/run/k2/secrets/sess-rekey-abcdefgh/a.sock".into(),
        created_at: "2026-10-24T00:00:00Z".into(),
        ..Default::default()
    };
    v.agent_socket_put(&sock).expect("socket record");

    v.rekey(&owner, &proof).expect("rekey");

    assert_eq!(v.status().tamper, "ok", "nothing reads as tampered after a rekey");
    assert_eq!(value(v, "ws-ops", "STRIPE_KEY"), b"k2test-rekey-value-2");
    assert_eq!(v.grants(&Selector::Id(a.id.clone()), &owner).expect("grants").len(), 1);
    assert!(v.requests(Some("ws-ops")).expect("cards").iter().any(|c| c.card_id == "card-rekey-0123456789"));
    assert_eq!(v.placements(Some(&a.id)).expect("placements").rows.len(), 1);
    assert_eq!(v.accept_from(Some("ws-ops")).expect("accept").len(), 1);
    assert_eq!(v.peer_vault("fp-rekey").expect("peer survives"), peer);
    assert_eq!(v.agent_policy(&k.id).expect("policy").burst, 7);
    assert_eq!(v.agent_socket_get("sess-rekey").expect("rec"), Some(sock));
    assert_eq!(v.status().tamper, "ok");
    // The pending value was re-sealed under the new data key: accepting
    // it lands the original value.
    v.handoff_decide(&pend.record.delivery_id, &Decision { action: "accept".into(), ..Default::default() }, &owner).expect("accept");
    assert_eq!(value(v, "ws-ops", "PENDING_KEY"), SENTINEL.as_bytes());

    // A tampered grant stops the next rekey (and nothing changes).
    let db = t._d.path().join("vault/vault.db");
    let c = rusqlite::Connection::open(&db).expect("db");
    let n = c.execute("UPDATE grants SET granted_by = 'someone-else'", []).expect("tamper");
    assert!(n >= 1);
    drop(c);
    let e = v.rekey(&owner, &proof).expect_err("a tampered grant stops the rekey");
    assert_eq!(e.code, "vault_tamper", "{e:?}");
    assert_eq!(value(v, "ws-ops", "STRIPE_KEY"), b"k2test-rekey-value-2", "the vault still opens under the old key");
}

/// One row in every MAC'd table (the rekey tests).
fn fill_every_table(v: &LocalVault, other: &LocalVault) {
    let owner = Actor::Owner;
    let a = put(v, "ws-ops", "STRIPE_KEY", "k2test-rekey-value-1", &owner);
    v.rotate(&Selector::Id(a.id.clone()), SecretBytes::from_slice(b"k2test-rekey-value-2"), &owner).expect("rotate");
    v.grant(
        crate::store::GrantRequest {
            selector: Selector::Id(a.id.clone()),
            consumer: crate::model::Consumer::workspace("ws-sales"),
            delivery: Delivery::Fetch,
            env_name: None,
            file_name: None,
            hosts_narrowed: None,
            expires_at: None,
            version: None,
        },
        &owner,
    )
    .expect("grant");
    v.request_create(crate::vault::SecretRequest {
        card_id: "card-rekey-0123456789".into(),
        conversation: "conv-1".into(),
        session_id: Some("sess-1".into()),
        workspace_id: "ws-ops".into(),
        requested_by: "agent:ws-ops/sess-1".into(),
        name: "CARD_KEY".into(),
        kind: Kind::ApiKey,
        proposed_scope: "workspace".into(),
        proposed_hosts: vec!["api.stripe.com".into()],
        proposed_delivery: Delivery::Fetch,
        prompt: "Paste it".into(),
        ttl_secs: None,
        status: String::new(),
        answered_by: None,
        secret_id: None,
        created_at: String::new(),
        answered_at: None,
        inject: None,
    })
    .expect("card");
    v.placement_add(
        crate::vault::NewPlacement {
            secret_id: a.id.clone(),
            target: crate::vault::PlacementTarget::Manual,
            unit: None,
            cred_name: None,
            where_text: Some("box1:/etc/x".into()),
            note: String::new(),
            restart: false,
        },
        "owner",
    )
    .expect("placement");
    v.handoff_receive(incoming("ws-ops", item("PENDING_KEY"), "sales", false), SecretBytes::from_slice(SENTINEL.as_bytes())).expect("pend");
    v.accept_from_add("ws-ops", "Sales::Rosson.K2.dev", &owner).expect("accept-from");
    v.peer_vault_put(&PeerVault {
        peer_fp: "fp-rekey".into(),
        vault_id: "v-peer".into(),
        recipient: other.carry_recipient().expect("r").encode(),
        features: vec![FEATURE.into()],
        fetched_at: "2026-10-24T00:00:00Z".into(),
    })
    .expect("peer");
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws-ops"), "DEPLOY", Kind::SshKey);
    r.default_delivery = Some(Delivery::Agent);
    let k = v.put(r, SecretBytes::from_slice(crate::ssh_agent::testkeys::ed25519().as_bytes()), &owner).expect("ssh").meta;
    v.set_agent_policy(&Selector::Id(k.id.clone()), crate::vault::AgentPolicyPatch { burst: Some(7), ..Default::default() }, &owner).expect("policy");
    v.agent_socket_put(&crate::vault::AgentSocket {
        tag: "sess-rekey".into(),
        kind: "session".into(),
        workspace_id: "ws-ops".into(),
        secrets_dir: "/run/k2/secrets/sess-rekey-abcdefgh".into(),
        sock_path: "/run/k2/secrets/sess-rekey-abcdefgh/a.sock".into(),
        created_at: "2026-10-24T00:00:00Z".into(),
        ..Default::default()
    })
    .expect("socket record");
}

/// Round 3 (8): one tampered row in any MAC'd table stops the rekey
/// (vault_tamper), table by table.
#[test]
fn rekey_stops_on_a_tampered_row_in_every_macd_table() {
    let other = open("ho-rekey-loop-peer");
    for (table, sql) in [
        ("secret", "UPDATE secret SET description = 'tampered' WHERE name = 'STRIPE_KEY'"),
        ("secret_version", "UPDATE secret_version SET created_by = 'tampered' WHERE version = 1"),
        ("grants", "UPDATE grants SET granted_by = 'tampered'"),
        ("secret_request", "UPDATE secret_request SET prompt = 'tampered'"),
        ("placement", "UPDATE placement SET note = 'tampered'"),
        ("handoff", "UPDATE handoff SET note = 'tampered'"),
        ("handoff_value", "UPDATE handoff_value SET ciphertext = x'00'"),
        ("handoff_accept", "UPDATE handoff_accept SET added_by = 'tampered'"),
        ("peer_vault", "UPDATE peer_vault SET fetched_at = 'tampered'"),
        ("agent_policy", "UPDATE agent_policy SET updated_by = 'tampered'"),
        ("agent_socket", "UPDATE agent_socket SET workspace_id = 'tampered'"),
    ] {
        let t = open(&format!("ho-rekey-{table}"));
        fill_every_table(&t.v, &other.v);
        let c = rusqlite::Connection::open(t._d.path().join("vault/vault.db")).expect("db");
        let n = c.execute(sql, []).unwrap_or_else(|e| panic!("{table}: {e}"));
        assert!(n >= 1, "{table}: no row tampered");
        drop(c);
        let e = t.v.rekey(&Actor::Owner, &crate::policy::HumanProof::Confirmed { method: "t".into() }).expect_err(table);
        assert_eq!(e.code, "vault_tamper", "{table}: {e:?}");
    }
}

// ── accepting is consent (Rosson 2026-10-30) ────────────────────────

/// The incident's secret: a short-lived pairing code for a CLI, sent as
/// the sender's default api_key (fetch, no sites, ceiling env).
fn cli_code(name: &str) -> HandoffItem {
    let mut it = item(name);
    it.hosts.clear();
    it.delivery = "fetch".into();
    it.ceiling = "env".into();
    it
}

fn grant_to(v: &LocalVault, ws: &str, name: &str, to: crate::model::Consumer, d: Delivery, actor: &Actor) -> Result<crate::store::Grant> {
    v.grant(
        crate::store::GrantRequest {
            selector: Selector::user(Scope::workspace(ws), name),
            consumer: to,
            delivery: d,
            env_name: None,
            file_name: None,
            hosts_narrowed: None,
            expires_at: None,
            version: None,
        },
        actor,
    )
}

fn accept(v: &LocalVault, id: &str, actor: &Actor) -> SecretMeta {
    v.handoff_decide(id, &Decision { action: "accept".into(), as_name: None, replace: false }, actor).expect("accept").meta.expect("landed")
}

const CODE: &[u8] = b"K2TEST-PAIR.CODE-0123456789";

/// A: after a person accepts, the receiving workspace's agents take it at
/// any delivery up to the ceiling the card showed, with no owner confirm,
/// and nothing else moves.
#[test]
fn accepting_is_consent_the_receiving_workspace_takes_it_up_to_the_ceiling() {
    let t = open("ho-consent");
    let v = &t.v;
    let ws = crate::model::Consumer::workspace;
    let p = v.handoff_receive(incoming("ws-ops", cli_code("PAIR_CODE"), "sew::sewlab.k2.dev", false), SecretBytes::from_slice(CODE)).expect("pend");
    assert_eq!(p.record.status, "pending");
    let id = p.record.delivery_id.clone();
    // Pending gives nothing: there is no secret to take.
    assert_eq!(grant_to(v, "ws-ops", "PAIR_CODE", ws("ws-ops"), Delivery::Env, &agent("ws-ops")).expect_err("pending").code, "not_found");
    // The card showed: delivered as file, at most env.
    assert_eq!(p.record.item.landing_delivery(), (Delivery::File, Delivery::Env));

    let m = accept(v, &id, &Actor::Owner);
    assert_eq!((m.default_delivery, m.delivery_ceiling), (Delivery::File, Delivery::Env));
    assert!(m.inject.is_none(), "a code with no sites gets no made-up header template: {:?}", m.inject);
    let sel = Selector::user(Scope::workspace("ws-ops"), "PAIR_CODE");
    assert_eq!(v.resolve(&sel, Use::File, &ops_session()).expect("file as accepted").value.expose(), CODE);
    assert_eq!(v.resolve(&sel, Use::Env, &ops_session()).err().expect("not env yet").code, "delivery_not_granted");

    // The receiving agent takes env (the incident's refusal).
    let g = grant_to(v, "ws-ops", "PAIR_CODE", ws("ws-ops"), Delivery::Env, &agent("ws-ops")).expect("accepting is consent: env for its own workspace");
    assert_eq!(g.delivery, Delivery::Env);
    assert_eq!(v.resolve(&sel, Use::Env, &ops_session()).expect("env now").value.expose(), CODE);
    // `k2 vault delivery PAIR_CODE env` too: only the delivery widens.
    let m2 = v.set_policy(&sel, PolicyPatch { default_delivery: Some(Delivery::Env), ..Default::default() }, &agent("ws-ops")).expect("raise to the ceiling");
    assert_eq!((m2.default_delivery, m2.delivery_ceiling), (Delivery::Env, Delivery::Env));

    // Above the ceiling: refused.
    assert_eq!(grant_to(v, "ws-ops", "PAIR_CODE", ws("ws-ops"), Delivery::Api, &agent("ws-ops")).expect_err("api").code, "delivery_above_ceiling");
    for (what, patch) in [
        ("raise the ceiling", PolicyPatch { delivery_ceiling: Some(Delivery::Api), ..Default::default() }),
        ("api delivery", PolicyPatch { default_delivery: Some(Delivery::Api), delivery_ceiling: Some(Delivery::Api), ..Default::default() }),
        ("add a site", PolicyPatch { add_hosts: vec!["api.example.com".into()], ..Default::default() }),
        ("an injection", PolicyPatch { inject: Some(crate::inject::Inject::default()), ..Default::default() }),
    ] {
        assert_eq!(v.set_policy(&sel, patch, &agent("ws-ops")).expect_err(what).code, "vault_owner_confirm_required", "{what}");
    }
    // Another workspace on the same server: as before.
    assert_eq!(grant_to(v, "ws-ops", "PAIR_CODE", ws("ws-sales"), Delivery::File, &agent("ws-ops")).expect_err("ws-sales").code, "vault_owner_confirm_required");
    assert_eq!(
        grant_to(v, "ws-ops", "PAIR_CODE", crate::model::Consumer::all_workspaces(), Delivery::File, &agent("ws-ops")).expect_err("ws:*").code,
        "vault_owner_confirm_required"
    );
    assert_eq!(
        grant_to(v, "ws-ops", "PAIR_CODE", crate::model::Consumer::compute("ws-ops", "k2n7"), Delivery::Env, &agent("ws-ops")).expect_err("compute").code,
        "vault_owner_confirm_required"
    );
    // Another workspace's agent can't touch it at all.
    assert_eq!(grant_to(v, "ws-ops", "PAIR_CODE", ws("ws-sales"), Delivery::File, &agent("ws-sales")).expect_err("not theirs").code, "vault_not_yours");
    assert_eq!(
        v.resolve(&sel, Use::File, &Requester::Session { workspace_id: "ws-sales".into(), session_id: Some("s".into()) }).err().expect("other ws").code,
        "delivery_not_granted"
    );
}

/// A: the card's ceiling is the cap, even when someone raises the copy's
/// own ceiling later; a Member's accept is a person's consent too.
#[test]
fn the_accepted_ceiling_caps_it_even_after_the_owner_raises_the_copy() {
    let t = open("ho-consent-cap");
    let v = &t.v;
    let mut it = cli_code("FILE_ONLY");
    it.ceiling = "file".into();
    let p = v.handoff_receive(incoming("ws-ops", it, "sales", false), SecretBytes::from_slice(CODE)).expect("pend");
    let member = Actor::Login { username: "mo".into(), role: Role::Member };
    let m = accept(v, &p.record.delivery_id, &member);
    assert_eq!((m.default_delivery, m.delivery_ceiling), (Delivery::File, Delivery::File));
    let sel = Selector::user(Scope::workspace("ws-ops"), "FILE_ONLY");
    grant_to(v, "ws-ops", "FILE_ONLY", crate::model::Consumer::session("s1"), Delivery::File, &agent("ws-ops")).expect("its own session, as file");
    assert_eq!(
        grant_to(v, "ws-ops", "FILE_ONLY", crate::model::Consumer::workspace("ws-ops"), Delivery::Env, &agent("ws-ops")).expect_err("env").code,
        "delivery_above_ceiling"
    );
    // The owner raises the copy's ceiling: the agent still gets no more
    // than the card showed without the owner.
    v.set_policy(&sel, PolicyPatch { delivery_ceiling: Some(Delivery::Env), ..Default::default() }, &Actor::Owner).expect("owner");
    assert_eq!(
        grant_to(v, "ws-ops", "FILE_ONLY", crate::model::Consumer::workspace("ws-ops"), Delivery::Env, &agent("ws-ops")).expect_err("above the card").code,
        "vault_owner_confirm_required"
    );
    assert_eq!(
        v.set_policy(&sel, PolicyPatch { default_delivery: Some(Delivery::Env), ..Default::default() }, &agent("ws-ops")).expect_err("above the card").code,
        "vault_owner_confirm_required"
    );
    assert_eq!(v.handoff_accepted_ceiling(&v.meta(&sel, &Actor::Owner).expect("meta")), Some(Delivery::File));
}

/// A: no person, no consent. A copy that landed by accept-from (no one
/// decided it), a declined or pending one, a secret the workspace made
/// itself or a person gave, a Replace over one, and a forged decider all
/// behave exactly as before.
#[test]
fn without_a_persons_accept_nothing_changes() {
    let t = open("ho-consent-none");
    let v = &t.v;
    let own = crate::model::Consumer::workspace("ws-ops");
    // Auto-landed (accept-from): the receiving agent still can't widen it.
    let l = v.handoff_receive(incoming("ws-ops", cli_code("AUTO_CODE"), "sales", true), SecretBytes::from_slice(CODE)).expect("land");
    assert_eq!(l.record.status, "landed");
    assert_eq!(grant_to(v, "ws-ops", "AUTO_CODE", own.clone(), Delivery::Env, &agent("ws-ops")).expect_err("no person").code, "vault_owner_confirm_required");
    assert_eq!(v.handoff_accepted_ceiling(&l.meta.clone().expect("meta")), None);
    // Declined: nothing landed.
    let d = v.handoff_receive(incoming("ws-ops", cli_code("NO_CODE"), "sales", false), SecretBytes::from_slice(CODE)).expect("pend");
    v.handoff_decide(&d.record.delivery_id, &Decision { action: "decline".into(), as_name: None, replace: false }, &Actor::Owner).expect("decline");
    assert_eq!(grant_to(v, "ws-ops", "NO_CODE", own.clone(), Delivery::File, &agent("ws-ops")).expect_err("declined").code, "not_found");
    // A person gave it: as before.
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws-ops"), "GIVEN", Kind::ApiKey);
    r.default_delivery = Some(Delivery::File);
    v.put(r, SecretBytes::from_slice(b"k2test-given-1"), &Actor::Owner).expect("put");
    assert_eq!(grant_to(v, "ws-ops", "GIVEN", own.clone(), Delivery::Env, &agent("ws-ops")).expect_err("person-given").code, "vault_owner_confirm_required");
    // The workspace made it itself: as before (its own to widen).
    let mut r = PutRequest::new(Namespace::User, Scope::workspace("ws-ops"), "MINE", Kind::ApiKey);
    r.default_delivery = Some(Delivery::File);
    v.put(r, SecretBytes::from_slice(b"k2test-mine-2"), &agent("ws-ops")).expect("put");
    grant_to(v, "ws-ops", "MINE", own.clone(), Delivery::Env, &agent("ws-ops")).expect("its own");
    // Replace over the person's secret: the secret keeps its origin, so
    // the accept hands the agent nothing beyond what it had.
    let mut it = cli_code("GIVEN");
    it.delivery = "file".into();
    let p = v.handoff_receive(incoming("ws-ops", it, "sales", false), SecretBytes::from_slice(CODE)).expect("pend");
    v.handoff_decide(&p.record.delivery_id, &Decision { action: "accept".into(), as_name: None, replace: true }, &Actor::Owner).expect("replace");
    assert_eq!(grant_to(v, "ws-ops", "GIVEN", own.clone(), Delivery::Env, &agent("ws-ops")).expect_err("replace").code, "vault_owner_confirm_required");
    // A forged decider on the auto-landed row breaks its MAC: no consent.
    let c = rusqlite::Connection::open(t._d.path().join("vault/vault.db")).expect("db");
    let n = c
        .execute(
            "UPDATE handoff SET decided_by = 'owner', decided_at = '2026-10-30T00:00:00Z' WHERE delivery_id = ?1 AND direction = 'in'",
            [&l.record.delivery_id],
        )
        .expect("tamper");
    assert_eq!(n, 1);
    drop(c);
    assert_eq!(grant_to(v, "ws-ops", "AUTO_CODE", own, Delivery::Env, &agent("ws-ops")).expect_err("forged").code, "vault_owner_confirm_required");
}

/// A and B: a sender claiming more than the card can show widens nothing.
/// An `api` delivery and ceiling land (and are shown) as env at most; the
/// accepted copy can't be taken above env, nor its ceiling raised.
#[test]
fn a_sender_claiming_api_gets_env_at_most_after_an_accept() {
    let t = open("ho-consent-claim");
    let v = &t.v;
    let mut it = cli_code("LOUD");
    it.delivery = "api".into();
    it.ceiling = "api".into();
    assert_eq!(it.landing_delivery(), (Delivery::Env, Delivery::Env), "what the card shows");
    let p = v.handoff_receive(incoming("ws-ops", it, "sales::rosson.k2.dev", false), SecretBytes::from_slice(CODE)).expect("pend");
    let m = accept(v, &p.record.delivery_id, &Actor::Owner);
    assert_eq!((m.default_delivery, m.delivery_ceiling), (Delivery::Env, Delivery::Env));
    assert_eq!(v.handoff_accepted_ceiling(&m), Some(Delivery::Env));
    let own = crate::model::Consumer::workspace("ws-ops");
    assert_eq!(grant_to(v, "ws-ops", "LOUD", own, Delivery::Api, &agent("ws-ops")).expect_err("api").code, "delivery_above_ceiling");
    let sel = Selector::user(Scope::workspace("ws-ops"), "LOUD");
    assert_eq!(
        v.set_policy(&sel, PolicyPatch { default_delivery: Some(Delivery::Api), delivery_ceiling: Some(Delivery::Api), ..Default::default() }, &agent("ws-ops"))
            .expect_err("api")
            .code,
        "vault_owner_confirm_required"
    );
    assert_eq!(v.resolve(&sel, Use::Api, &ops_session()).err().expect("never api").code, "delivery_not_granted");
}

// ── the sender picks the kind and delivery (B) ──────────────────────

#[test]
fn the_sender_says_what_it_is_within_its_own_ceiling() {
    use Delivery as D;
    // An HTTP key with sites, sent as a CLI token: no sites, no header.
    let mut it = item("STRIPE_KEY");
    it.inject = Some(crate::inject::Inject::default());
    it.delivery = "fetch".into();
    it.ceiling = "env".into();
    it.shape_for(Some("generic"), Some("env")).expect("env for a CLI");
    assert_eq!((it.kind.as_str(), it.delivery.as_str(), it.ceiling.as_str()), ("generic", "env", "env"));
    assert!(it.hosts.is_empty() && it.inject.is_none(), "{it:?}");
    assert_eq!(it.landing_delivery(), (D::Env, D::Env));
    // The incident's code, sent as file: lands as file, no header.
    let mut it = cli_code("PAIR_CODE");
    it.shape_for(None, Some("file")).expect("file");
    assert_eq!(it.landing_delivery(), (D::File, D::Env));
    // Nothing said: unchanged (today's behaviour).
    let before = item("SAME");
    let mut same = before.clone();
    same.shape_for(None, None).expect("no-op");
    same.shape_for(Some(""), Some("  ")).expect("blank is nothing");
    assert_eq!(same, before);
    // Fetch keeps its sites, and needs some.
    let mut it = item("HTTP");
    it.ceiling = "env".into();
    it.shape_for(None, Some("fetch")).expect("fetch");
    assert_eq!(it.hosts, vec!["https://api.stripe.com".to_string()]);
    assert_eq!(cli_code("NOSITE").shape_for(None, Some("fetch")).expect_err("no sites").code, "fetch_needs_sites");
    // Never above the sender's own ceiling.
    let mut low = cli_code("LOW");
    low.ceiling = "file".into();
    assert_eq!(low.clone().shape_for(None, Some("env")).expect_err("env above file").code, "delivery_above_ceiling");
    low.shape_for(None, Some("file")).expect("file is within");
    // Only fetch, file and env.
    for d in ["api", "agent", "proxy", "none"] {
        assert_eq!(cli_code("X").shape_for(None, Some(d)).expect_err(d).code, "bad_delivery", "{d}");
    }
    // Keys and signers keep their kind; plain values relabel.
    let mut ssh = cli_code("DEPLOY");
    ssh.kind = "ssh_key".into();
    assert_eq!(ssh.shape_for(Some("generic"), None).expect_err("ssh").code, "kind_fixed");
    assert_eq!(cli_code("X").shape_for(Some("ssh_key"), None).expect_err("to ssh").code, "kind_fixed");
    assert_eq!(cli_code("X").shape_for(Some("login_slot"), None).expect_err("never").code, "kind_fixed");
    let mut pw = cli_code("PW");
    pw.shape_for(Some("password"), None).expect("relabel");
    assert_eq!(pw.kind, "password");
}

#[test]
fn a_shaped_send_lands_with_no_sites_and_no_header_template() {
    let t = open("ho-shape-land");
    let v = &t.v;
    let mut it = item("GH_CLI");
    it.inject = Some(crate::inject::Inject::default());
    it.ceiling = "env".into();
    it.shape_for(Some("oauth_token"), Some("env")).expect("shape");
    let r = v.handoff_receive(incoming("ws-ops", it, "sales", true), SecretBytes::from_slice(b"gho_k2test_shape_1")).expect("land");
    let m = r.meta.expect("meta");
    assert_eq!((m.kind, m.default_delivery, m.delivery_ceiling), (Kind::OauthToken, Delivery::Env, Delivery::Env));
    assert!(m.hosts.is_empty() && m.inject.is_none(), "{m:?}");
    v.resolve(&Selector::user(Scope::workspace("ws-ops"), "GH_CLI"), Use::Env, &ops_session()).expect("env");
    // Today's default item (an old sender): keeps its header template.
    let r = v.handoff_receive(incoming("ws-ops", item("OLD_STYLE"), "sales", true), SecretBytes::from_slice(b"k2test-old-style")).expect("land");
    assert!(r.meta.expect("meta").inject.is_some(), "an HTTP key with sites keeps its injection");
}

// ── say how to use it (C) ───────────────────────────────────────────

#[test]
fn use_hints_follow_the_delivery_the_copy_has() {
    use Delivery as D;
    let f = use_hint_for(D::File, "PAIR_CODE", false);
    assert!(f.starts_with("file: $K2_SECRETS_DIR/PAIR_CODE (run k2 vault sync"), "{f}");
    assert!(!f.contains("k2 vault fetch"), "{f}");
    let e = use_hint_for(D::Env, "pair-code", false);
    assert!(e.starts_with("env: $PAIR_CODE in new sessions"), "{e}");
    assert!(e.contains("k2 vault env-file --secret pair-code"), "{e}");
    let n = use_hint_for(D::Fetch, "PAIR_CODE", false);
    assert!(!n.contains("k2 vault fetch"), "fetch only for an HTTP key with sites: {n}");
    assert!(use_hint_for(D::Fetch, "STRIPE_KEY", true).starts_with("k2 vault fetch --secret STRIPE_KEY"));
}
