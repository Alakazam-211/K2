//! `vault.send`'s wall, with the real switches (lib tests never flip the
//! process-wide one): an agent's `k2 vault send NAME --to agent::host` of a
//! secret a person gave, to a paired server that can take a handoff.
//!
//! - cards off, and cards on with the agent's own switch off: `held`, no
//!   `permission` object, and a hint that names Settings and the command a
//!   person runs;
//! - both on: the refusal names the secret, the target and the note, its hint
//!   never sends anyone to a terminal, and it teaches the exact Ask-with line;
//! - a person is never taught a card.
//!
//! The card itself (Allow, "from now on", Deny, the standing grants) is
//! tested in `permission/actions/vault_send_tests.rs`. The other server is
//! a stub that answers the signed vault-recipient request only: the hold
//! comes before anything is sealed or posted.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::io::{Read, Write};

use app_fixture::*;
use k2_core::federation::{self, vault_recipient as vr, FederationPeer, PeerStore, PeerTrust};
use k2_vault::{Actor, Kind, Namespace, PutRequest, Scope, SecretBytes, SecretStore};
use serde_json::{json, Value};

const SEND: &str = "/cli/vault/send";
const VALUE: &[u8] = b"k2test-send-wall-value-not-real-7719";

struct SwitchOff;
impl Drop for SwitchOff {
    fn drop(&mut self) {
        k2_core::permission::gate::set_server_switch(false);
    }
}

fn agent_switch(ws: &Ws, on: bool) {
    k2_core::workspace::settings::set_agents_can_request_permissions(&ws.id, on).expect("agent switch");
}

/// A peer that answers `POST /cli/federation/vault-recipient` with a
/// statement signed by its pinned key (and 404s anything else).
fn spawn_recipient_stub(peer_key: rcgen::KeyPair, our_fp: String, recipient: String) -> u16 {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    let peer_fp = k2_core::federation::peers::fingerprint_of_spki_pem(&peer_key.public_key_pem()).unwrap();
    std::thread::spawn(move || {
        for stream in l.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut acc = Vec::new();
            let mut buf = [0u8; 8192];
            let (mut head_end, mut clen) = (None, 0usize);
            loop {
                let n = match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                acc.extend_from_slice(&buf[..n]);
                if head_end.is_none() {
                    if let Some(p) = acc.windows(4).position(|w| w == b"\r\n\r\n") {
                        head_end = Some(p + 4);
                        let head = String::from_utf8_lossy(&acc[..p]).to_ascii_lowercase();
                        clen = head.lines().find_map(|l| l.strip_prefix("content-length:").and_then(|v| v.trim().parse().ok())).unwrap_or(0);
                    }
                }
                if head_end.is_some_and(|h| acc.len() >= h + clen) {
                    break;
                }
            }
            let line = String::from_utf8_lossy(&acc).lines().next().unwrap_or("").to_string();
            let (status, out) = if line.starts_with("POST /cli/federation/vault-recipient") {
                let st = vr::Statement {
                    v: vr::VERSION,
                    fingerprint: peer_fp.clone(),
                    to_fingerprint: our_fp.clone(),
                    vault_id: "v-stub".into(),
                    recipient: recipient.clone(),
                    features: vec![k2_vault::handoff::FEATURE.into()],
                    issued_at: chrono::Utc::now().to_rfc3339(),
                };
                ("200 OK", serde_json::to_string(&vr::sign(&st, &peer_key).unwrap()).unwrap())
            } else {
                ("404 Not Found", r#"{"error":"not found"}"#.to_string())
            };
            let resp = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len());
            let _ = s.write_all(resp.as_bytes());
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_held_send_teaches_the_card_only_while_both_switches_are_on() {
    let env = setup(&[("K2_FEDERATION", "1")]);
    let _off = SwitchOff;
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "sendagent");
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    let vault = k2_daemon::vault::get().expect("the test vault");
    let mut put = PutRequest::new(Namespace::User, Scope::workspace(ws.id.clone()), "FLEET_DATABASE", Kind::ApiKey);
    put.hosts = vec!["api.example.com".into()];
    vault.put(put, SecretBytes::from_slice(VALUE), &Actor::Owner).expect("a person gives the secret");

    // A paired, Trusted server that can take a handoff.
    let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut store = PeerStore::load().unwrap_or_default();
    let fp = store.upsert(FederationPeer::pin("stubbox", "stubbox", peer_key.public_key_pem()).unwrap());
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, "inbound");
    store.save().unwrap();
    let our_fp = federation::local_fingerprint().expect("this server's federation key");
    let stub = spawn_recipient_stub(peer_key, our_fp, vault.carry_recipient().unwrap().encode());
    let _base = k2_core::test_env::EnvVar::set("K2_FEDERATION_INBOUND_BASE", &format!("http://127.0.0.1:{stub}"));

    let agent = passport(&ws);
    let body = json!({ "name": "FLEET_DATABASE", "to": "argus::stubbox", "note": "for the import" });

    // Off, and on with the agent's own switch off: held, the person's ways
    // in words, and no closing line.
    let check_off = |r: &Resp, why: &str| {
        assert_eq!(r.status, 403, "{why}: {}", r.body);
        let v: Value = r.json();
        assert_eq!(v["error"]["code"], "held", "{why}: {v}");
        assert!(v.get("permission").is_none(), "{why}: {v}");
        let hint = v["error"]["hint"].as_str().unwrap();
        assert!(hint.contains("Ask your person (an Owner or Admin)"), "{why}: {hint}");
        assert!(hint.contains("Settings → Workspaces → sendagent → Permissions → Secrets it may send to other servers"), "{why}: {hint}");
        assert!(hint.contains("k2 vault send FLEET_DATABASE --scope ws:sendagent --to argus::stubbox"), "{why}: {hint}");
    };
    let r = post(port, SEND, &agent, body.clone()).await;
    check_off(&r, "cards off");
    k2_core::permission::gate::set_server_switch(true);
    agent_switch(&ws, false);
    let r = post(port, SEND, &agent, body.clone()).await;
    check_off(&r, "the agent's switch off");

    // Both on: the exact line to ask with, and no terminal in the words.
    agent_switch(&ws, true);
    let r = post(port, SEND, &agent, body.clone()).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let v: Value = r.json();
    assert_eq!(v["error"]["code"], "held", "{v}");
    assert_eq!((v["error"]["secret"].as_str(), v["error"]["to"].as_str()), (Some("FLEET_DATABASE"), Some("argus::stubbox")), "{v}");
    let hint = v["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("Ask them with the line below") && hint.contains("secret=A,B,C"), "{hint}");
    for never in ["terminal", "Terminal", "iTerm", "k2 vault send FLEET"] {
        assert!(!hint.contains(never), "cards on: the hint never sends anyone to a terminal ({never}): {hint}");
    }
    assert_eq!(v["permission"]["action"], "vault.send", "{v}");
    assert_eq!(v["permission"]["approvers"], "owner or admin", "{v}");
    assert_eq!(
        v["permission"]["command"],
        "k2 permission request vault.send note='for the import' secret=FLEET_DATABASE to=argus::stubbox --why \"<what you need it for>\"",
        "{v}"
    );

    // A person is never taught a card.
    let r = post(port, SEND, OWNER, json!({ "name": "FLEET_DATABASE", "scope": "workspace", "workspace": ws.handle, "to": "argus::stubbox" })).await;
    assert!(r.json().get("permission").is_none(), "{}", r.body);
    assert!(!r.body.contains("k2test-send-wall-value"));
    drop(env);
}
