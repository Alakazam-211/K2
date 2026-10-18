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
    assert_eq!(m.default_delivery, Delivery::Fetch, "fetch when there are sites");
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
    let r = v.handoff_receive(incoming("ws-ops", it, "sales", true), SecretBytes::from_slice(b"-----BEGIN KEY----- abc")).expect("file");
    assert_eq!(r.meta.expect("meta").default_delivery, Delivery::File);
}

#[test]
fn wildcard_body_inject_person_given_and_remote_off_always_pend() {
    let t = open("ho-pend");
    let v = &t.v;
    let mut wild = item("WILD");
    wild.hosts = vec!["https://*.stripe.com".into()];
    let mut body = item("BODY");
    body.inject = Some(crate::inject::Inject { dst: crate::inject::Dst::Body, fmt: "{}".into(), allowed_dst: vec![], allowed_fmt: vec![] });
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
