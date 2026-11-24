//! A federated send's brief retries (0.46.18), against a scripted
//! loopback peer: the roster lookup and the message POST each get three
//! tries for a failure that can pass (no answer, 429, 5xx), never for a
//! refusal (4xx), and a send that still fails says so in one line. Every
//! retried POST carries the same msg_uuid, so the receiver keeps one.
//!
//! One temp HOME is this server; the peer is a std-thread socket stub.
//! Never the real home, keychain or daemon. Pauses run 40× faster under
//! test (`federation_retry::real_pause`).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use k2_core::federation::{FederationEnvelope, FederationPeer, PeerStore, PeerTrust};

use super::*;
use crate::test_support::with_temp_home;

/// What the stub answers one request with.
#[derive(Clone)]
enum Answer {
    /// Read the request, close without an answer.
    Drop,
    /// This status and JSON body.
    Says(u16, String),
}

fn says(status: u16, body: &str) -> Answer {
    Answer::Says(status, body.to_string())
}

/// Every request a stub peer saw: its request line and its body.
type Seen = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// A peer that answers its roster GETs and inbound POSTs from two scripts,
/// in order (the last answer repeats). Keeps every request's path line and
/// body.
struct Peer {
    base: String,
    seen: Seen,
}

impl Peer {
    fn count(&self, prefix: &str) -> usize {
        self.seen.lock().unwrap().iter().filter(|(l, _)| l.starts_with(prefix)).count()
    }
    fn roster_hits(&self) -> usize {
        self.count("GET /cli/federation/roster")
    }
    fn posts(&self) -> Vec<FederationEnvelope> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(l, _)| l.starts_with("POST /cli/federation/inbound"))
            .map(|(_, b)| serde_json::from_slice(b).expect("a sealed envelope"))
            .collect()
    }
}

fn scripted_peer(roster: Vec<Answer>, inbound: Vec<Answer>) -> Peer {
    let l = TcpListener::bind(("127.0.0.1", 0)).expect("bind peer stub");
    let port = l.local_addr().unwrap().port();
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        let (mut r_n, mut i_n) = (0usize, 0usize);
        for stream in l.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut acc: Vec<u8> = Vec::new();
            let mut buf = [0u8; 8192];
            let mut head_end: Option<usize> = None;
            let mut clen = 0usize;
            loop {
                let n = match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                acc.extend_from_slice(&buf[..n]);
                if head_end.is_none() {
                    if let Some(pos) = acc.windows(4).position(|w| w == b"\r\n\r\n") {
                        head_end = Some(pos + 4);
                        let head = String::from_utf8_lossy(&acc[..pos]).to_string();
                        clen = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                    }
                }
                if let Some(he) = head_end {
                    if acc.len() >= he + clen {
                        break;
                    }
                }
            }
            let Some(he) = head_end else { continue };
            let line = String::from_utf8_lossy(&acc[..he]).lines().next().unwrap_or("").to_string();
            log.lock().unwrap().push((line.clone(), acc[he..].to_vec()));
            let pick = |script: &Vec<Answer>, n: &mut usize| {
                let a = script[(*n).min(script.len() - 1)].clone();
                *n += 1;
                a
            };
            let answer = if line.starts_with("GET /cli/federation/roster") {
                pick(&roster, &mut r_n)
            } else if line.starts_with("POST /cli/federation/inbound") {
                pick(&inbound, &mut i_n)
            } else {
                says(404, r#"{"error":"not found"}"#)
            };
            match answer {
                Answer::Drop => drop(s),
                Answer::Says(status, body) => {
                    let _ = write!(
                        s,
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = s.flush();
                }
            }
        }
    });
    Peer { base: format!("http://127.0.0.1:{port}"), seen }
}

/// Pin `peer` Trusted (roster + inbound) at its stub's address; its
/// fingerprint.
fn pin(peer: &Peer) -> String {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut store = PeerStore::load().unwrap_or_default();
    let host = peer.base.trim_start_matches("http://").to_string();
    let mut pinned = FederationPeer::pin("teammate", format!("peer-{}", host.replace(':', "-")), key.public_key_pem()).unwrap();
    pinned.base_url = peer.base.clone();
    let fp = store.upsert(pinned);
    store.set_trust(&fp, PeerTrust::Trusted);
    store.grant(&fp, k2_core::federation::CAP_ROSTER);
    store.grant(&fp, "inbound");
    store.save().unwrap();
    fp
}

const ROSTER: &str = r#"{"agents":[{"workspace_id":"ws-b","workspace_name":"Bob","agent":"bob","address":"ws-b::bob","aliases":[]}],"inbound_dedupe":"in_flight"}"#;

fn json(resp: &CliResponse) -> serde_json::Value {
    serde_json::from_str(&resp.body).unwrap_or_else(|e| panic!("json body ({e}): {}", resp.body))
}

fn send_to(fp: &str) -> CliResponse {
    let body = serde_json::json!({ "to": format!("{fp}::ws-b::bob"), "text": "part 2 of 3" });
    handle_send(body.to_string().as_bytes())
}

// ── The roster lookup ────────────────────────────────────────────────

#[test]
fn a_roster_that_fails_twice_then_answers_is_read_on_the_third_try() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let peer = scripted_peer(vec![Answer::Drop, says(429, "{}"), says(200, ROSTER)], vec![]);
        let fp = pin(&peer);
        let resp = handle_peer_roster(&fp);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        assert_eq!(json(&resp)["roster"]["agents"][0]["agent"], "bob", "{}", resp.body);
        assert_eq!(peer.roster_hits(), 3, "a dropped request and a 429 are each tried again, once");
        assert!(
            crate::federation_retry::peer_dedupes(&fp),
            "the roster said the peer drops in-flight repeats"
        );
    });
}

#[test]
fn a_roster_that_never_answers_ends_with_one_line_after_three_tries() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let peer = scripted_peer(vec![says(500, r#"{"error":"db locked"}"#)], vec![]);
        let fp = pin(&peer);
        let resp = handle_peer_roster(&fp);
        assert_eq!(resp.status, "502 Bad Gateway", "{}", resp.body);
        let v = json(&resp);
        assert_eq!(
            v["error"],
            "the roster step failed after 3 attempts (last error: peer rejected roster: HTTP 500). \
             Nothing was delivered; send again.",
        );
        assert_eq!(v["step"], "roster");
        assert_eq!(v["attempts"], 3);
        assert_eq!(peer.roster_hits(), 3, "three tries, never a fourth");

        // A peer that never answers at all reads the same, with the cause.
        let (_held, dead) = crate::test_support::dead_port();
        let gone = Peer { base: format!("http://127.0.0.1:{dead}"), seen: Arc::default() };
        let fp = pin(&gone);
        let resp = handle_peer_roster(&fp);
        assert_eq!(resp.status, "502 Bad Gateway", "{}", resp.body);
        let line = json(&resp)["error"].as_str().unwrap().to_string();
        assert!(line.starts_with("the roster step failed after "), "{line}");
        assert!(line.contains("(last error: GET http://127.0.0.1:"), "{line}");
        assert!(line.ends_with("Nothing was delivered; send again."), "{line}");
        assert!(!line.contains('\n') && !line.contains('\t'), "one line: {line:?}");
    });
}

#[test]
fn a_401_or_403_roster_is_never_retried() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        for status in [401u16, 403] {
            let peer = scripted_peer(vec![says(status, r#"{"error":"roster denied"}"#)], vec![]);
            let fp = pin(&peer);
            let resp = handle_peer_roster(&fp);
            assert_eq!(resp.status, "502 Bad Gateway", "{}", resp.body);
            assert_eq!(
                json(&resp)["error"],
                format!(
                    "the roster step failed (1 attempt, not retried: peer rejected roster: HTTP {status}). \
                     Nothing was delivered."
                )
            );
            assert_eq!(peer.roster_hits(), 1, "HTTP {status} is a refusal: asked once");
        }
    });
}

// ── The message POST ─────────────────────────────────────────────────

#[test]
fn a_peer_that_fails_twice_then_succeeds_gets_the_message_once() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let peer = scripted_peer(
            vec![],
            vec![
                says(503, r#"{"error":"busy"}"#),
                says(503, r#"{"error":"message is being delivered","code":"in_flight"}"#),
                says(200, r#"{"delivered":true,"mode":"live"}"#),
            ],
        );
        let fp = pin(&peer);
        let resp = send_to(&fp);
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let v = json(&resp);
        assert_eq!(v["status"], "sent", "{v}");

        let posts = peer.posts();
        assert_eq!(posts.len(), 3, "two retries, then delivered");
        let uuids: std::collections::HashSet<&str> =
            posts.iter().map(|e| e.payload.msg_uuid.as_str()).collect();
        assert_eq!(uuids.len(), 1, "every try is the same message (one msg_uuid)");
        assert_eq!(v["msg_uuid"], *uuids.iter().next().unwrap());
        let nonces: std::collections::HashSet<&str> =
            posts.iter().map(|e| e.payload.nonce.as_str()).collect();
        assert_eq!(nonces.len(), 3, "each retry is resealed: the receiver burned the last nonce");
        assert!(outbox::list_for_peer(&fp).is_empty(), "delivered: nothing left queued");
    });
}

#[test]
fn a_peer_that_always_fails_leaves_it_queued_with_the_line() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let peer = scripted_peer(vec![], vec![says(503, r#"{"error":"busy"}"#)]);
        let fp = pin(&peer);
        let resp = send_to(&fp);
        let v = json(&resp);
        assert_eq!(v["status"], "queued", "{v}");
        assert_eq!(v["delivered"], false);
        assert_eq!(v["step"], "send");
        assert_eq!(v["attempts"], 3);
        assert_eq!(
            v["reason"],
            "the send step got no delivery after 3 attempts (last error: peer rejected inbound: HTTP 503)"
        );
        assert_eq!(peer.posts().len(), 3);
        assert_eq!(outbox::list_for_peer(&fp).len(), 1, "kept for the outbox drain");
    });
}

#[test]
fn a_refused_post_is_never_retried() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        for status in [401u16, 403, 404] {
            let peer = scripted_peer(vec![], vec![says(status, r#"{"error":"no"}"#)]);
            let fp = pin(&peer);
            crate::federation_retry::set_peer_dedupes(&fp, true);
            let v = json(&send_to(&fp));
            assert_eq!(v["status"], "queued", "{v}");
            assert_eq!(
                v["reason"],
                format!("the send step got no delivery after 1 attempt (last error: peer rejected inbound: HTTP {status})")
            );
            assert_eq!(peer.posts().len(), 1, "HTTP {status} is a refusal: sent once");
        }
    });
}

#[test]
fn a_post_that_may_have_landed_is_resent_only_to_a_peer_that_dedupes_in_flight() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let script = || vec![says(502, r#"{"error":"bad gateway"}"#), says(200, r#"{"delivered":true}"#)];

        // An older peer (its roster never said so): a 502 may hide a
        // delivery still running there, so the outbox drain takes it.
        let old = scripted_peer(vec![], script());
        let fp = pin(&old);
        crate::federation_retry::set_peer_dedupes(&fp, false);
        let v = json(&send_to(&fp));
        assert_eq!(v["status"], "queued", "{v}");
        assert_eq!(old.posts().len(), 1);

        // A peer that drops an in-flight repeat: resent at once.
        let new = scripted_peer(vec![], script());
        let fp = pin(&new);
        crate::federation_retry::set_peer_dedupes(&fp, true);
        let v = json(&send_to(&fp));
        assert_eq!(v["status"], "sent", "{v}");
        assert_eq!(new.posts().len(), 2);
    });
}

#[test]
fn a_duplicate_answer_on_a_retry_is_a_delivery() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let peer = scripted_peer(
            vec![],
            vec![
                says(503, r#"{"error":"message is being delivered","code":"in_flight"}"#),
                says(409, r#"{"error":"duplicate message (already delivered): x"}"#),
            ],
        );
        let fp = pin(&peer);
        let v = json(&send_to(&fp));
        assert_eq!(v["status"], "sent", "the first try landed; the repeat was dropped: {v}");
        assert_eq!(peer.posts().len(), 2);
        assert!(outbox::list_for_peer(&fp).is_empty());
    });
}

// ── A tray package (k2 msg agent::host --inbox-wake a b) ─────────────

fn two_file_spec() -> federation::TraySpec {
    federation::encode_tray_spec(
        Some("Batch"),
        None,
        true,
        &[("a.md".to_string(), b"one".to_vec()), ("b.pdf".to_string(), b"two".to_vec())],
    )
    .expect("tray spec")
}

#[test]
fn a_tray_whose_roster_failed_names_every_part_that_did_not_land() {
    let roster = CliResponse {
        status: "502 Bad Gateway",
        content_type: "application/json",
        body: serde_json::json!({
            "error": "the roster step failed after 3 attempts (last error: x). Nothing was delivered; send again.",
            "step": "roster",
            "attempts": 3,
        })
        .to_string(),
    };
    let resp = tray_not_delivered("bob", "rosson.k2.dev", &two_file_spec(), roster);
    assert_eq!(resp.status, "502 Bad Gateway");
    let v = json(&resp);
    assert_eq!(
        v["error"],
        "not delivered to bob::rosson.k2.dev (2 files: a.md, b.pdf): the roster step failed after 3 attempts \
         (last error: x). Nothing was delivered; send again."
    );
    assert_eq!(v["attempts"], 3, "the other fields stay");

    let queued = CliResponse::ok_json(
        serde_json::json!({ "status": "queued", "reason": "the send step got no delivery after 3 attempts (last error: y)" })
            .to_string(),
    );
    let v = json(&tray_queued_hint("bob", "rosson.k2.dev", &two_file_spec(), queued));
    assert_eq!(
        v["hint"],
        "2 files: a.md, b.pdf for bob::rosson.k2.dev is not delivered yet: the send step got no delivery after \
         3 attempts (last error: y). K2 retries it from this server's outbox (next try in about 30 s); don't send it again."
    );
    let sent = CliResponse::ok_json(r#"{"status":"sent"}"#.to_string());
    assert_eq!(tray_queued_hint("bob", "h", &two_file_spec(), sent).body, r#"{"status":"sent"}"#);
}

#[test]
fn a_tray_send_whose_roster_never_answers_fails_and_names_its_parts() {
    with_temp_home(|| {
        k2_core::db::init_for_tests();
        let _fed = k2_core::test_env::EnvVar::set("K2_FEDERATION", "1");
        let peer = scripted_peer(vec![says(503, "{}")], vec![says(200, r#"{"delivered":true}"#)]);
        let _fp = pin(&peer);
        let host = peer.base.trim_start_matches("http://").to_string();
        let resp = handle_send_user_tray("bob", &host, two_file_spec(), None, None);
        assert_eq!(resp.status, "502 Bad Gateway", "{}", resp.body);
        let line = json(&resp)["error"].as_str().unwrap().to_string();
        assert!(
            line.starts_with(&format!("not delivered to bob::{host} (2 files: a.md, b.pdf): the roster step failed after 3 attempts")),
            "{line}"
        );
        assert_eq!(peer.roster_hits(), 3);
        assert!(peer.posts().is_empty(), "no roster, no send");
    });
}
