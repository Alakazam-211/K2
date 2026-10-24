//! Cross-server secret handoff (prd-agent-secret-handoff-v1 §5.2, §14),
//! on the existing federation harness: one temp HOME is this server; the
//! other server is a fresh pinned key (receiver tests) or a loopback peer
//! stub with its own temp vault (sender tests). Never the real home,
//! keychain or daemon.

use std::collections::HashMap;
use std::sync::Arc;

use k2_core::awareness::{AgentAddress, AgentSignal, SignalKind, WorkspaceId};
use k2_core::federation::{self, vault_recipient as vr, FederationPeer, PeerStore, PeerTrust};
use k2_vault::confirm::KdfCost;
use k2_vault::fsutil::TestDir;
use k2_vault::handoff::{HandoffItem, HandoffPayload, Party};
use k2_vault::keystore::fake::FakeKeyStore;
use k2_vault::{Actor, KeyStore, Kind, LocalVault, Namespace, OpenOptions, PutRequest, Requester, Scope, SecretBytes, SecretStore, Selector, Use};
use serde_json::{json, Value};

use super::*;
use crate::test_support::with_temp_home;
use crate::vault::actor::{Channel, VaultCaller};

fn sentinel() -> String {
    format!("k2test_FED_HANDOFF_{}", uuid::Uuid::new_v4().simple())
}

fn other_vault(tag: &str) -> (TestDir, Arc<LocalVault>) {
    let d = TestDir::new(tag);
    let ks: Arc<dyn KeyStore> = Arc::new(FakeKeyStore::new());
    let v = Arc::new(LocalVault::open(d.path().join("vault"), ks, OpenOptions { kdf: KdfCost::Floor }).expect("open"));
    (d, v)
}

/// A receiving workspace this server lists to peers (persona `bob`,
/// "agents may create connections" on), with a pinned Chat for the card.
fn receiving_workspace() -> (String, String) {
    let path = std::env::temp_dir().join(format!("k2-fedho-{}", uuid::Uuid::new_v4().simple())).to_string_lossy().into_owned();
    let persona = k2_core::workspace::agent_identity::workspace_agent_md_path(&path);
    std::fs::create_dir_all(persona.parent().unwrap()).unwrap();
    std::fs::write(&persona, "---\nname: bob\n---\n# bob\n").unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, agents_can_create_connections) VALUES (?1, ?2, ?3, 1)",
        rusqlite::params![id, format!("bob-{}", &id[..8]), path],
    )
    .unwrap();
    k2_core::db::schema::WorkspaceSession::upsert(&conn, &uuid::Uuid::new_v4().to_string(), &id, None, Some(&uuid::Uuid::new_v4().to_string()), "claude", "owner", "idle")
        .unwrap();
    (id, path)
}

/// Pin a distinct peer key as Trusted + inbound; returns it and its row.
fn pin_peer(subdomain: &str) -> (rcgen::KeyPair, FederationPeer) {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut store = PeerStore::load().unwrap_or_default();
    let fp = store.upsert(FederationPeer::pin("peer", subdomain, key.public_key_pem()).unwrap());
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();
    let row = store.get(&fp).unwrap().clone();
    (key, row)
}

fn item(name: &str) -> HandoffItem {
    HandoffItem {
        name: name.into(),
        as_name: None,
        kind: "api_key".into(),
        hosts: vec!["https://api.stripe.com".into()],
        inject: None,
        description: String::new(),
        delivery: "fetch".into(),
        ceiling: "env".into(),
        human_given: false,
        ttl_secs: None,
    }
}

/// What a peer (`sender` vault, `peer_key` federation key) posts to our
/// `/cli/federation/vault-handoff`.
#[allow(clippy::too_many_arguments)]
fn sealed_envelope(
    sender: &LocalVault,
    receiver: &LocalVault,
    peer_key: &rcgen::KeyPair,
    peer_fp: &str,
    to_ws: &str,
    delivery_id: &str,
    it: HandoffItem,
    value: &str,
    tweak: impl FnOnce(&mut HandoffPayload),
) -> Vec<u8> {
    let me = federation::local_fingerprint().unwrap();
    let from = Party { fingerprint: peer_fp.into(), host: "peer.k2.dev".into(), workspace_id: "src-ws".into(), handle: "cortana".into(), actor: "agent:src-ws/s1".into(), vault_id: String::new() };
    let to = Party { fingerprint: me, host: String::new(), workspace_id: to_ws.into(), handle: "bob".into(), actor: String::new(), vault_id: receiver.vault_id().into() };
    let mut p = HandoffPayload::new(delivery_id, from, to, it, &SecretBytes::from_slice(value.as_bytes()), "for the refund job".into(), None);
    tweak(&mut p);
    let sealed = sender.handoff_seal(&receiver.carry_recipient().unwrap(), &mut p).expect("seal");
    let signal = AgentSignal::new(
        AgentAddress::Agent { workspace: WorkspaceId("src-ws".into()), name: "cortana".into() },
        AgentAddress::Agent { workspace: WorkspaceId(to_ws.into()), name: "bob".into() },
        SignalKind::Custom {
            kind: vr::HANDOFF_KIND.into(),
            payload: json!({ "deliveryId": delivery_id, "toWorkspaceId": to_ws, "sealedB64": base64::engine::general_purpose::STANDARD.encode(&sealed) }),
        },
    );
    federation::seal(&signal, peer_key, "peer", 8).unwrap()
}

fn body(r: &CliResponse) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|_| panic!("json: {}", r.body))
}

fn code_of(r: &CliResponse) -> String {
    let v = body(r);
    v["code"].as_str().or_else(|| v["error"]["code"].as_str()).unwrap_or("").to_string()
}

// ── receiver ────────────────────────────────────────────────────────

#[test]
fn receiver_pends_without_opt_in_and_lands_only_with_accept_from_and_a_connection() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let v = crate::vault::ensure_test_vault();
        let (_d, sender) = other_vault("fed-sender");
        let (ws, path) = receiving_workspace();
        let (key, peer) = pin_peer("peer");
        let peer_addr = crate::federation_routes::federated_from_label("cortana", &crate::federation_routes::peer_host(&peer));
        let secret = sentinel();

        // Opt-in off (R4): pending, no knock.
        let id1 = k2_vault::handoff::new_delivery_id();
        let r = handle_vault_handoff(&sealed_envelope(&sender, &v, &key, &peer.fingerprint, &ws, &id1, item("K_OFF"), &secret, |_| {}));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(body(&r)["status"], "pending");
        assert_eq!(body(&r)["reason"], "remote_instruction_off");
        assert!(!r.body.contains(&secret));
        let rec = v.handoff_get(&id1, "in").expect("row");
        assert_eq!(rec.peer_addr, peer_addr);
        assert_eq!(rec.peer_fp.as_deref(), Some(peer.fingerprint.as_str()));
        assert!(rec.card.is_some(), "the owner still gets a card");

        // Opt-in on, not listed: pending.
        k2_core::workspace::settings::update_project_setting(&path, "allow_remote_instruct", "1").expect("opt in");
        let id2 = k2_vault::handoff::new_delivery_id();
        let r = handle_vault_handoff(&sealed_envelope(&sender, &v, &key, &peer.fingerprint, &ws, &id2, item("K_LIST"), &secret, |_| {}));
        assert_eq!(body(&r)["reason"], "not_allowed", "{}", r.body);

        // Listed but no connection row on the receiving side (H16): pending.
        v.accept_from_add(&ws, &peer_addr, &Actor::Owner).expect("allow");
        let id3 = k2_vault::handoff::new_delivery_id();
        let r = handle_vault_handoff(&sealed_envelope(&sender, &v, &key, &peer.fingerprint, &ws, &id3, item("K_CONN"), &secret, |_| {}));
        assert_eq!(body(&r)["reason"], "not_allowed", "H16: accept-from alone isn't enough: {}", r.body);

        // Listed + connected: lands, with the value intact.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let (agent, host) = peer_addr.split_once("::").unwrap();
            k2_core::db::schema::WorkspaceRemoteConnection::create(&conn, &uuid::Uuid::new_v4().to_string(), &ws, &peer_addr, host, agent, Some(&peer.fingerprint)).unwrap();
        }
        let id4 = k2_vault::handoff::new_delivery_id();
        let bytes = sealed_envelope(&sender, &v, &key, &peer.fingerprint, &ws, &id4, item("K_LAND"), &secret, |_| {});
        assert!(!String::from_utf8_lossy(&bytes).contains(&secret), "the wire carries sealed bytes only");
        let r = handle_vault_handoff(&bytes);
        assert_eq!(body(&r)["status"], "landed", "{}", r.body);
        let got = v.resolve(&Selector::user(Scope::workspace(&ws), "K_LAND"), Use::System, &Requester::System("t".into())).expect("landed");
        assert_eq!(got.value.expose(), secret.as_bytes());
        assert!(got.meta.description.contains("cortana on"), "{}", got.meta.description);

        // Durable replay (H9): the same delivery id in a fresh envelope.
        let again = sealed_envelope(&sender, &v, &key, &peer.fingerprint, &ws, &id4, item("K_LAND"), "another-value-123", |_| {});
        let r = handle_vault_handoff(&again);
        assert_eq!((r.status, code_of(&r).as_str()), ("409 Conflict", "replay"));
        // The same bytes again: the envelope nonce catches it first.
        let r = handle_vault_handoff(&bytes);
        assert_eq!(r.status, "409 Conflict");
        assert_eq!(v.meta(&Selector::user(Scope::workspace(&ws), "K_LAND"), &Actor::Owner).unwrap().latest_version, 1);

        // Every receive is in the Settings logs data source.
        let lines: Vec<String> = k2_core::auth_audit::tail(1000)
            .unwrap()
            .into_iter()
            .filter(|e| e["event"] == crate::vault_handoff_routes::AUDIT_EVENT)
            .map(|e| e["outcome"].as_str().unwrap_or("").to_string())
            .collect();
        assert!(lines.iter().any(|l| l.starts_with("ok receive K_OFF") && l.contains(&id1) && l.ends_with("pending")), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("ok receive K_LAND") && l.ends_with("landed")), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("deny receive") && l.ends_with("replay")), "{lines:?}");
        let all: String = k2_core::auth_audit::tail(1000).unwrap().iter().map(|e| e.to_string()).collect();
        assert!(!all.contains(&secret), "no value in the auth audit");
    });
}

#[test]
fn receiver_refuses_a_sealed_sender_that_isnt_the_signing_peer_and_bad_kinds() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let v = crate::vault::ensure_test_vault();
        let (_d, sender) = other_vault("fed-sender-2");
        let (ws, _path) = receiving_workspace();
        let (key, peer) = pin_peer("peer");
        // H5: the payload claims another server.
        let id = k2_vault::handoff::new_delivery_id();
        let r = handle_vault_handoff(&sealed_envelope(&sender, &v, &key, &peer.fingerprint, &ws, &id, item("K"), &sentinel(), |p| {
            p.from.fingerprint = "fp-someone-else".into();
        }));
        assert_eq!((r.status, code_of(&r).as_str()), ("403 Forbidden", "handoff_sender_mismatch"));
        // A workspace this server doesn't list.
        let id = k2_vault::handoff::new_delivery_id();
        let r = handle_vault_handoff(&sealed_envelope(&sender, &v, &key, &peer.fingerprint, "no-such-ws", &id, item("K"), &sentinel(), |_| {}));
        assert_eq!(r.status, "404 Not Found", "{}", r.body);
        // A plain message posted here is refused.
        let msg = AgentSignal::new(
            AgentAddress::Agent { workspace: WorkspaceId("src".into()), name: "cortana".into() },
            AgentAddress::Agent { workspace: WorkspaceId(ws.clone()), name: "bob".into() },
            SignalKind::Msg { text: "hi".into() },
        );
        let r = handle_vault_handoff(&federation::seal(&msg, &key, "peer", 8).unwrap());
        assert_eq!((r.status, code_of(&r).as_str()), ("400 Bad Request", "wrong_kind"));
        // An unpinned key gets nothing.
        let stranger = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let fp = k2_core::federation::peers::fingerprint_of_spki_pem(&stranger.public_key_pem()).unwrap();
        let id = k2_vault::handoff::new_delivery_id();
        let r = handle_vault_handoff(&sealed_envelope(&sender, &v, &stranger, &fp, &ws, &id, item("K"), &sentinel(), |_| {}));
        assert_eq!(r.status, "403 Forbidden");
        assert!(v.handoff_get(&id, "in").is_err(), "nothing recorded");
    });
}

#[test]
fn vault_recipient_answers_a_statement_bound_to_the_asking_peer() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let v = crate::vault::ensure_test_vault();
        let (key, peer) = pin_peer("peer");
        let req = AgentSignal::new(
            AgentAddress::Agent { workspace: WorkspaceId("x".into()), name: "vault".into() },
            AgentAddress::Agent { workspace: WorkspaceId("y".into()), name: "vault".into() },
            SignalKind::Custom { kind: vr::REQUEST_KIND.into(), payload: json!({}) },
        );
        let r = handle_vault_recipient(&federation::seal(&req, &key, "peer", 8).unwrap());
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let signed: vr::SignedStatement = serde_json::from_str(&r.body).unwrap();
        let me = federation::local_fingerprint().unwrap();
        let my_pem = federation::identity::load_or_create().unwrap().public_key_pem();
        // The peer checks it against the key it pinned for us.
        vr::verify(&signed, &my_pem, &me, &peer.fingerprint, k2_vault::handoff::FEATURE, chrono::Utc::now()).expect("verifies");
        assert_eq!(signed.statement.recipient, v.carry_recipient().unwrap().encode());
        assert_eq!(signed.statement.vault_id, v.vault_id());
        // An unpinned server gets 403.
        let stranger = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let r = handle_vault_recipient(&federation::seal(&req, &stranger, "peer", 8).unwrap());
        assert_eq!(r.status, "403 Forbidden");
    });
}

// ── sender (a loopback peer stub) ───────────────────────────────────

/// What the stub peer saw.
#[derive(Default)]
struct Seen {
    recipient_hits: usize,
    handoff_bodies: Vec<Vec<u8>>,
    inbound_hits: usize,
    opened: Vec<(String, String, Vec<u8>)>,
}

/// A peer that answers the roster (agent `bob` in `ws-b`), and either 404s
/// the vault routes (an old K2) or answers them for real with its own vault
/// (`receiver`) and federation key.
fn spawn_stub(
    old: bool,
    peer_key: Arc<rcgen::KeyPair>,
    receiver: Arc<LocalVault>,
    our_fp: String,
) -> (u16, Arc<std::sync::Mutex<Seen>>) {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = Arc::new(std::sync::Mutex::new(Seen::default()));
    let s2 = seen.clone();
    let peer_fp = k2_core::federation::peers::fingerprint_of_spki_pem(&peer_key.public_key_pem()).unwrap();
    std::thread::spawn(move || {
        for stream in l.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut acc = Vec::new();
            let mut buf = [0u8; 8192];
            let (mut he, mut clen) = (None, 0usize);
            loop {
                let n = match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                acc.extend_from_slice(&buf[..n]);
                if he.is_none() {
                    if let Some(p) = acc.windows(4).position(|w| w == b"\r\n\r\n") {
                        he = Some(p + 4);
                        let head = String::from_utf8_lossy(&acc[..p]).to_ascii_lowercase();
                        clen = head.lines().find_map(|l| l.strip_prefix("content-length:").and_then(|v| v.trim().parse().ok())).unwrap_or(0);
                    }
                }
                if let Some(h) = he {
                    if acc.len() >= h + clen {
                        break;
                    }
                }
            }
            let Some(h) = he else { continue };
            let line = String::from_utf8_lossy(&acc[..h]).lines().next().unwrap_or("").to_string();
            let body = acc[h..].to_vec();
            let (status, out) = if line.starts_with("GET /cli/federation/roster") {
                ("200 OK", json!({ "agents": [{ "workspace_id": "ws-b", "workspace_name": "bob", "agent": "bob", "address": "ws-b::bob", "aliases": [] }] }).to_string())
            } else if line.starts_with("POST /cli/federation/inbound") {
                s2.lock().unwrap().inbound_hits += 1;
                ("200 OK", "{}".to_string())
            } else if old {
                ("404 Not Found", r#"{"error":"not found"}"#.to_string())
            } else if line.starts_with("POST /cli/federation/vault-recipient") {
                s2.lock().unwrap().recipient_hits += 1;
                let st = vr::Statement {
                    v: 1,
                    fingerprint: peer_fp.clone(),
                    to_fingerprint: our_fp.clone(),
                    vault_id: receiver.vault_id().into(),
                    recipient: receiver.carry_recipient().unwrap().encode(),
                    features: vec![k2_vault::handoff::FEATURE.into()],
                    issued_at: chrono::Utc::now().to_rfc3339(),
                };
                ("200 OK", serde_json::to_string(&vr::sign(&st, &peer_key).unwrap()).unwrap())
            } else if line.starts_with("POST /cli/federation/vault-handoff") {
                // Open it for real, as the receiving vault would.
                let env: federation::FederationEnvelope = serde_json::from_slice(&body).unwrap();
                let (id, sealed) = match &env.payload.signal.kind {
                    SignalKind::Custom { payload, .. } => (
                        payload["deliveryId"].as_str().unwrap().to_string(),
                        base64::engine::general_purpose::STANDARD.decode(payload["sealedB64"].as_str().unwrap()).unwrap(),
                    ),
                    _ => panic!("not a handoff kind"),
                };
                let p = receiver
                    .handoff_open(&sealed, k2_vault::handoff::OpenExpect { local_fingerprint: &peer_fp, peer_fingerprint: &our_fp, delivery_id: &id })
                    .expect("the peer opens what we sealed");
                let mut g = s2.lock().unwrap();
                g.handoff_bodies.push(body.clone());
                g.opened.push((id.clone(), p.item.name.clone(), p.value().unwrap().expose().to_vec()));
                ("200 OK", json!({ "status": "pending", "deliveryId": id, "name": p.item.name, "landedName": p.item.name, "reason": "not_allowed" }).to_string())
            } else {
                ("404 Not Found", r#"{"error":"not found"}"#.to_string())
            };
            let resp = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len());
            let _ = s.write_all(resp.as_bytes());
        }
    });
    (port, seen)
}

fn sales_with_secret(v: &LocalVault, name: &str, value: &str, by: &Actor) -> (String, String) {
    let id = uuid::Uuid::new_v4().to_string();
    let label = format!("sales-{}", &id[..8]);
    let path = std::env::temp_dir().join(format!("k2-fedho-{label}")).to_string_lossy().into_owned();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)", rusqlite::params![id, label, path]).unwrap();
    }
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(&id), name, Kind::ApiKey);
    r.hosts = vec!["api.stripe.com".into()];
    v.put(r, SecretBytes::from_slice(value.as_bytes()), by).unwrap();
    (id, label)
}

fn send(v: &Arc<LocalVault>, c: &VaultCaller, b: Value) -> (u16, Value, String) {
    let r = crate::vault_handoff_routes::handle(v, crate::vault_handoff_routes::SEND, c, b.to_string().as_bytes(), &HashMap::new(), "test");
    (r.status[..3].parse().unwrap(), serde_json::from_str(&r.body).unwrap_or(Value::Null), r.body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_server_send_seals_to_the_peer_vault_and_never_sends_plaintext() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let (_d, receiver) = other_vault("fed-stub-recv");
    let peer_key = Arc::new(rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap());
    let mut store = PeerStore::load().unwrap_or_default();
    let fp = store.upsert(FederationPeer::pin("luzz", "luzz", peer_key.public_key_pem()).unwrap());
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();
    let our_fp = federation::local_fingerprint().unwrap();
    let (port, seen) = spawn_stub(false, peer_key.clone(), receiver.clone(), our_fp);
    let _base = k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", &format!("http://127.0.0.1:{port}"));
    let _fed = k2_core::test_env::EnvVar::set("K2_FEDERATION", "1");
    let secret = sentinel();
    let (_ws, label) = sales_with_secret(&v, "STRIPE_KEY", &secret, &Actor::System("seed".into()));
    let owner = VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None };
    let v2 = v.clone();
    let (st, j, raw) = tokio::task::spawn_blocking(move || {
        send(&v2, &owner, json!({ "name": "STRIPE_KEY", "scope": "workspace", "workspace": label, "to": "bob::luzz", "note": "for refunds" }))
    })
    .await
    .unwrap();
    assert_eq!(st, 200, "{raw}");
    assert_eq!(j["status"], "pending");
    assert_eq!(j["cross"], true);
    assert!(j["who"].as_str().unwrap().contains("must accept it"), "{raw}");
    let g = seen.lock().unwrap();
    assert_eq!(g.recipient_hits, 1, "learned the peer vault once");
    assert_eq!(g.inbound_hits, 0, "H4: never posted to /inbound");
    assert_eq!(g.handoff_bodies.len(), 1);
    assert!(!String::from_utf8_lossy(&g.handoff_bodies[0]).contains(&secret), "only sealed bytes on the wire");
    assert_eq!(g.opened[0].1, "STRIPE_KEY");
    assert_eq!(g.opened[0].2, secret.as_bytes(), "the peer's vault opens the right value");
    let id = j["deliveryId"].as_str().unwrap();
    let out = v.handoff_get(id, "out").expect("out row");
    assert_eq!(out.status, "pending");
    assert!(out.peer_addr.starts_with("bob::"), "{}", out.peer_addr);
    assert!(v.peer_vault(&fp).is_some(), "the peer vault is cached");
    let all: String = k2_core::auth_audit::tail(1000).unwrap().iter().map(|e| e.to_string()).collect();
    assert!(all.contains(&format!("ok send STRIPE_KEY to {} {id} pending", out.peer_addr)), "send is in the Settings logs");
    assert!(!all.contains(&secret));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_old_peer_gets_peer_too_old_and_nothing_is_sent() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let (_d, receiver) = other_vault("fed-stub-old");
    let peer_key = Arc::new(rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap());
    let mut store = PeerStore::load().unwrap_or_default();
    let fp = store.upsert(FederationPeer::pin("oldbox", "oldbox", peer_key.public_key_pem()).unwrap());
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();
    let (port, seen) = spawn_stub(true, peer_key, receiver, federation::local_fingerprint().unwrap());
    let _base = k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", &format!("http://127.0.0.1:{port}"));
    let _fed = k2_core::test_env::EnvVar::set("K2_FEDERATION", "1");
    let secret = sentinel();
    let (_ws, label) = sales_with_secret(&v, "OLD_KEY", &secret, &Actor::System("seed".into()));
    let owner = VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None };
    let v2 = v.clone();
    let (st, j, raw) = tokio::task::spawn_blocking(move || {
        send(&v2, &owner, json!({ "name": "OLD_KEY", "scope": "workspace", "workspace": label, "to": "bob::oldbox" }))
    })
    .await
    .unwrap();
    assert_eq!((st, j["error"]["code"].as_str().unwrap_or("")), (422, "peer_too_old"), "{raw}");
    let g = seen.lock().unwrap();
    assert_eq!(g.inbound_hits, 0, "no plaintext fallback to /inbound");
    assert!(g.handoff_bodies.is_empty(), "nothing sent");
    let all: String = k2_core::auth_audit::tail(1000).unwrap().iter().map(|e| e.to_string()).collect();
    assert!(all.contains("deny send OLD_KEY to bob::oldbox: peer_too_old"), "the refusal is logged");
    assert!(!all.contains(&secret));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn person_given_cross_server_needs_a_person_and_a_confirm_and_agents_need_a_connection() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let (_d, receiver) = other_vault("fed-stub-held");
    let peer_key = Arc::new(rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap());
    let mut store = PeerStore::load().unwrap_or_default();
    let fp = store.upsert(FederationPeer::pin("heldbox", "heldbox", peer_key.public_key_pem()).unwrap());
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();
    let (port, seen) = spawn_stub(false, peer_key, receiver, federation::local_fingerprint().unwrap());
    let _base = k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", &format!("http://127.0.0.1:{port}"));
    let _fed = k2_core::test_env::EnvVar::set("K2_FEDERATION", "1");
    // A person gave sales this key.
    let (ws, label) = sales_with_secret(&v, "GIVEN", &sentinel(), &Actor::Owner);
    let agent = VaultCaller {
        actor: Actor::Agent(k2_vault::AgentActor { workspace_id: ws.clone(), session_id: Some("s".into()), shell: false, vault_manage: false }),
        channel: Channel::Plain,
        session_id: Some("s".into()),
    };
    let owner = VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None };
    let (v2, a2, o2, l2) = (v.clone(), agent.clone(), owner.clone(), label.clone());
    let results = tokio::task::spawn_blocking(move || {
        let held = send(&v2, &a2, json!({ "name": "GIVEN", "to": "bob::heldbox" }));
        let confirm = send(&v2, &o2, json!({ "name": "GIVEN", "scope": "workspace", "workspace": l2, "to": "bob::heldbox" }));
        (held, confirm)
    })
    .await
    .unwrap();
    let ((st, j, _), (st2, j2, raw2)) = results;
    assert_eq!((st, j["error"]["code"].as_str().unwrap_or("")), (403, "held"), "R3: an agent can't send a person's secret to another server");
    assert_eq!((st2, j2["error"]["code"].as_str().unwrap_or("")), (403, "confirm_required"), "{raw2}");
    let secret_id = j2["secretId"].as_str().unwrap().to_string();
    // With a vault confirm token bound to that secret, the person can.
    let token = v.confirm.mint(crate::vault_routes::HANDOFF_CONFIRM_ACTION, Some(&secret_id), "owner", "vault-password");
    let (v3, o3, l3) = (v.clone(), owner.clone(), label.clone());
    let (st, j, raw) = tokio::task::spawn_blocking(move || {
        send(&v3, &o3, json!({ "name": "GIVEN", "scope": "workspace", "workspace": l3, "to": "bob::heldbox", "confirmToken": token }))
    })
    .await
    .unwrap();
    assert_eq!(st, 200, "{raw}");
    assert_eq!(j["status"], "pending");
    // An agent's own (not person-given) secret, but no connection to bob.
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(&ws), "MINE", Kind::ApiKey);
    r.hosts = vec!["api.stripe.com".into()];
    v.put(r, SecretBytes::from_slice(sentinel().as_bytes()), &agent.actor).unwrap();
    let (v4, a4) = (v.clone(), agent.clone());
    let (st, j, raw) = tokio::task::spawn_blocking(move || send(&v4, &a4, json!({ "name": "MINE", "to": "bob::heldbox" }))).await.unwrap();
    assert_eq!((st, j["error"]["code"].as_str().unwrap_or("")), (403, "not_connected"), "{raw}");
    assert_eq!(seen.lock().unwrap().handoff_bodies.len(), 1, "only the confirmed send left this server");
}

// ── H5: a signed revoke ─────────────────────────────────────────────

fn revoke_envelope(peer_key: &rcgen::KeyPair, to_ws: &str, delivery_id: &str) -> Vec<u8> {
    let signal = AgentSignal::new(
        AgentAddress::Agent { workspace: WorkspaceId("src-ws".into()), name: "cortana".into() },
        AgentAddress::Agent { workspace: WorkspaceId(to_ws.into()), name: "vault".into() },
        SignalKind::Custom { kind: REVOKE_KIND.into(), payload: json!({ "deliveryId": delivery_id }) },
    );
    federation::seal(&signal, peer_key, "peer", 8).unwrap()
}

#[test]
fn only_the_sending_peer_can_unsend_and_it_disables_the_copy() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let v = crate::vault::ensure_test_vault();
        let (_d, sender) = other_vault("fed-revoke");
        let (ws, path) = receiving_workspace();
        k2_core::workspace::settings::update_project_setting(&path, "allow_remote_instruct", "1").expect("opt in");
        let (key, peer) = pin_peer("peer");
        let (other_key, _) = pin_peer("peer2");
        let id = k2_vault::handoff::new_delivery_id();
        let r = handle_vault_handoff(&sealed_envelope(&sender, &v, &key, &peer.fingerprint, &ws, &id, item("K_REVOKE"), &sentinel(), |_| {}));
        assert_eq!(body(&r)["status"], "pending", "{}", r.body);
        // Another pinned server can't take it back.
        let r = handle_vault_handoff(&revoke_envelope(&other_key, &ws, &id));
        assert_eq!((r.status, code_of(&r).as_str()), ("403 Forbidden", "not_yours"));
        assert!(v.handoff_has_value(&id), "still pending");
        // The sender can.
        let r = handle_vault_handoff(&revoke_envelope(&key, &ws, &id));
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(body(&r)["status"], "unsent");
        assert!(!v.handoff_has_value(&id), "the pending value is gone");
        let rec = v.handoff_get(&id, "in").unwrap();
        assert_eq!(rec.status, "unsent");
        let card = rec.card.expect("card");
        let doc = k2_core::overlay::get_doc(card.rsplit_once('/').unwrap().1).unwrap().unwrap();
        assert_eq!(doc.handoff.unwrap().status, "unsent", "the card closes");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cross_server_ask_travels_as_a_message_and_its_reply_lands_straight_in() {
    let _home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let v = crate::vault::ensure_test_vault();
    let (_d, peer_vault) = other_vault("fed-ask-peer");
    let peer_key = Arc::new(rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap());
    let mut store = PeerStore::load().unwrap_or_default();
    let fp = store.upsert(FederationPeer::pin("askbox", "askbox", peer_key.public_key_pem()).unwrap());
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.grant(&fp, "roster");
    store.save().unwrap();
    let peer = store.get(&fp).unwrap().clone();
    let (port, seen) = spawn_stub(false, peer_key.clone(), peer_vault.clone(), federation::local_fingerprint().unwrap());
    let _base = k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", &format!("http://127.0.0.1:{port}"));
    let _fed = k2_core::test_env::EnvVar::set("K2_FEDERATION", "1");
    // The asking workspace (listed to peers, opted in) and its connection to bob there.
    let (ws, path) = receiving_workspace();
    k2_core::workspace::settings::update_project_setting(&path, "allow_remote_instruct", "1").unwrap();
    let host = crate::federation_routes::peer_host(&peer);
    let bob = format!("bob::{host}");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::WorkspaceRemoteConnection::create(&conn, &uuid::Uuid::new_v4().to_string(), &ws, &bob, &host, "bob", Some(&fp)).unwrap();
    }
    let owner = VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None };
    let (v2, ws2) = (v.clone(), ws.clone());
    let r = tokio::task::spawn_blocking(move || {
        crate::vault_handoff_routes::handle(
            &v2,
            crate::vault_handoff_routes::ASK,
            &owner,
            json!({ "to": "bob::askbox", "name": "K_ASKED", "workspace": ws2, "note": "for the deploy" }).to_string().as_bytes(),
            &HashMap::new(),
            "test",
        )
    })
    .await
    .unwrap();
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let j = body(&r);
    let ask_id = j["askId"].as_str().unwrap().to_string();
    assert_eq!(j["to"], bob.as_str());
    assert_eq!(seen.lock().unwrap().inbound_hits, 1, "the ask is a plain federated message");
    assert!(seen.lock().unwrap().handoff_bodies.is_empty(), "no secret moved");
    assert_eq!(v.handoff_get(&ask_id, "ask_out").unwrap().peer_addr, bob);

    // bob's server answers with --reply: a sealed handoff whose sender is bob.
    let secret = sentinel();
    let id = k2_vault::handoff::new_delivery_id();
    let me = federation::local_fingerprint().unwrap();
    let from = Party { fingerprint: fp.clone(), host: String::new(), workspace_id: "ws-b".into(), handle: "bob".into(), actor: "agent:ws-b/s".into(), vault_id: String::new() };
    let to = Party { fingerprint: me, host: String::new(), workspace_id: ws.clone(), handle: "bob".into(), actor: String::new(), vault_id: v.vault_id().into() };
    let mut p = HandoffPayload::new(&id, from, to, item("K_ASKED"), &SecretBytes::from_slice(secret.as_bytes()), String::new(), Some(ask_id.clone()));
    let sealed = peer_vault.handoff_seal(&v.carry_recipient().unwrap(), &mut p).unwrap();
    let signal = AgentSignal::new(
        AgentAddress::Agent { workspace: WorkspaceId("ws-b".into()), name: "bob".into() },
        AgentAddress::Agent { workspace: WorkspaceId(ws.clone()), name: "bob".into() },
        SignalKind::Custom {
            kind: vr::HANDOFF_KIND.into(),
            payload: json!({ "deliveryId": id, "toWorkspaceId": ws, "sealedB64": base64::engine::general_purpose::STANDARD.encode(&sealed) }),
        },
    );
    let bytes = federation::seal(&signal, &peer_key, "askbox", 8).unwrap();
    let r = tokio::task::spawn_blocking(move || handle_vault_handoff(&bytes)).await.unwrap();
    assert_eq!(body(&r)["status"], "landed", "a reply to our own ask lands straight in: {}", r.body);
    assert_eq!(v.handoff_get(&ask_id, "ask_out").unwrap().status, "answered");
    let got = v.resolve(&Selector::user(Scope::workspace(&ws), "K_ASKED"), Use::System, &Requester::System("t".into())).unwrap();
    assert_eq!(got.value.expose(), secret.as_bytes());
}

/// Items 2 and 3: what an older peer (no `sign-v1` / `agent-v1` in its
/// vault statement) is refused, before anything is sent.
#[test]
fn an_old_peer_is_refused_signed_and_agent_items_by_feature() {
    let item = |kind: &str, delivery: &str, ceiling: &str, signed: bool| HandoffItem {
        name: "K".into(),
        as_name: None,
        kind: kind.into(),
        hosts: vec![],
        inject: signed.then(|| k2_vault::inject::Inject { sign: Some(k2_vault::inject::Sign::Ovh), ..Default::default() }),
        description: String::new(),
        delivery: delivery.into(),
        ceiling: ceiling.into(),
        human_given: false,
        ttl_secs: None,
    };
    let old = vec![k2_vault::handoff::FEATURE.to_string()];
    let new = vec![
        k2_vault::handoff::FEATURE.to_string(),
        k2_vault::handoff::FEATURE_SIGN.to_string(),
        k2_vault::handoff::FEATURE_AGENT.to_string(),
    ];
    let agent = item("ssh_key", "agent", "env", false);
    assert_eq!(peer_missing_feature(&agent, &old, "old.example").expect("refused").code, "peer_too_old");
    assert!(peer_missing_feature(&agent, &new, "new.example").is_none());
    let ceiling_only = item("ssh_key", "file", "agent", false);
    assert_eq!(peer_missing_feature(&ceiling_only, &old, "old.example").expect("refused").code, "peer_too_old");
    let signed = item("generic", "fetch", "fetch", true);
    assert_eq!(peer_missing_feature(&signed, &old, "old.example").expect("refused").code, "peer_too_old_for_signed");
    assert!(peer_missing_feature(&signed, &new, "new.example").is_none());
    assert!(peer_missing_feature(&item("ssh_key", "file", "env", false), &old, "old.example").is_none(), "a plain file key goes anywhere");
}
