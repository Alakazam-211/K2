//! `/cli/federation/*` route handlers (Federation V1 functional core).
//!
//! SSOT: `.k2/prds/prd-cross-server-agent-comms.md` (Phases 2/3/4). The whole
//! surface is **dark by default** — the dispatcher 404s every
//! `/cli/federation/*` path unless [`k2_core::federation::enabled`]
//! (`K2_FEDERATION`, default OFF) is true — so a shipped 0.40.x build has zero
//! behavior change. These handlers wrap the k2-core primitives:
//!
//!   - [`handle_pair_request`] — P2 UNAUTH pair-request → creates only a
//!     `Pending` peer (fail-closed; promotion needs the owner SAS confirm).
//!   - [`handle_pair_confirm`] — P2 OWNER-gated SAS confirm → `Trusted` + caps.
//!   - [`handle_inbound`] — P3 the SECURITY CORE: verify → require_peer →
//!     replay/skew/ttl → sanitize → deliver to the local INBOX ONLY.
//!   - [`handle_send`] — P4 seal an [`AgentSignal`], durably enqueue, dial the
//!     peer's E2E listener, POST to its `/cli/federation/inbound`.
//!   - [`handle_roster`] — P5 stub (owner-gated for now; full peer-auth
//!     projection lands in Phase 5).
//!
//! The dispatcher owns method/auth gating (POST-only via `require_post`; owner
//! token via `require_owner` on confirm/send; the inbound route is
//! authenticated by the ENVELOPE itself, not a token — DECISION-2).

use crate::cli_response::CliResponse;

use k2_core::awareness::{AgentAddress, AgentSignal, Delivery, SignalKind, WorkspaceId};
use k2_core::federation::{self, ingress, outbox, pairing, PeerStore};

/// Default hop budget stamped on outbound envelopes (Risk M2).
const OUTBOUND_TTL: u8 = 8;

fn json_err(status: &'static str, msg: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "error": msg.to_string() }).to_string(),
    }
}

// ─────────────────────────────────────────────────────────────────────
// P2 — pairing
// ─────────────────────────────────────────────────────────────────────

/// `POST /cli/federation/pair/request` — UNAUTH body. A peer presents its
/// identity; we create (or re-see) a `Pending` row keyed by the fingerprint
/// derived from the presented key. Returns the fingerprint + SAS the owner
/// compares out-of-band. Creates ONLY `Pending` — never trusts.
pub fn handle_pair_request(body: &[u8]) -> CliResponse {
    #[derive(serde::Deserialize)]
    struct Req {
        label: Option<String>,
        subdomain: Option<String>,
        public_key_pem: String,
    }
    let req: Req = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("bad pair-request body: {e}")),
    };
    let local_fp = match federation::local_fingerprint() {
        Ok(fp) => fp,
        Err(e) => return json_err("500 Internal Server Error", format!("local fingerprint: {e}")),
    };
    let mut store = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => return json_err("500 Internal Server Error", format!("load peer store: {e}")),
    };
    let pair_req = pairing::PairRequest {
        label: req.label.unwrap_or_default(),
        subdomain: req.subdomain.unwrap_or_default(),
        public_key_pem: req.public_key_pem,
    };
    let outcome = match pairing::apply_pair_request(&mut store, &pair_req, &local_fp) {
        Ok(o) => o,
        Err(e) => return CliResponse::bad_request(e),
    };
    if let Err(e) = store.save() {
        return json_err("500 Internal Server Error", format!("save peer store: {e}"));
    }
    CliResponse::ok_json(
        serde_json::json!({
            "fingerprint": outcome.fingerprint,
            "sas": outcome.sas,
            "trust": "pending",
            "created": outcome.created,
            "local_fingerprint": local_fp,
        })
        .to_string(),
    )
}

/// `POST /cli/federation/pair/confirm` — OWNER-gated (dispatcher enforces).
/// The owner approves a `Pending` peer by fingerprint after comparing the
/// SAS. On a SAS match the peer flips `Trusted` and is granted its caps.
pub fn handle_pair_confirm(body: &[u8]) -> CliResponse {
    #[derive(serde::Deserialize)]
    struct Req {
        fingerprint: String,
        sas: String,
        #[serde(default)]
        capabilities: Vec<String>,
    }
    let req: Req = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("bad pair-confirm body: {e}")),
    };
    let local_fp = match federation::local_fingerprint() {
        Ok(fp) => fp,
        Err(e) => return json_err("500 Internal Server Error", format!("local fingerprint: {e}")),
    };
    let mut store = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => return json_err("500 Internal Server Error", format!("load peer store: {e}")),
    };
    if let Err(e) =
        pairing::apply_pair_confirm(&mut store, &req.fingerprint, &req.sas, &local_fp, &req.capabilities)
    {
        // SAS mismatch / blocked / unknown → fail-closed 403.
        return json_err("403 Forbidden", e);
    }
    if let Err(e) = store.save() {
        return json_err("500 Internal Server Error", format!("save peer store: {e}"));
    }
    CliResponse::ok_json(
        serde_json::json!({ "ok": true, "fingerprint": req.fingerprint, "trust": "trusted" })
            .to_string(),
    )
}

// ─────────────────────────────────────────────────────────────────────
// P3 — inbound ingress (the security core)
// ─────────────────────────────────────────────────────────────────────

/// `POST /cli/federation/inbound` — accept a signed envelope. Authenticated by
/// the ENVELOPE (verify against the pinned key + `require_peer`), NOT by any
/// token. Delivers to the local inbox ONLY. Every failure REJECTS.
pub fn handle_inbound(body: &[u8]) -> CliResponse {
    let local_fp = federation::local_fingerprint().unwrap_or_default();
    let store = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => return json_err("500 Internal Server Error", format!("load peer store: {e}")),
    };
    let inbox_root = k2_core::awareness::inbox_root();
    match ingress::ingest(
        body,
        &store,
        ingress::global_nonce_cache(),
        &inbox_root,
        &local_fp,
        ingress::DEFAULT_SKEW_SECS,
    ) {
        Ok(signal) => CliResponse::ok_json(
            serde_json::json!({ "delivered": true, "signal_id": signal.id.to_string() })
                .to_string(),
        ),
        Err(e) => {
            // Fail-closed: map each reject to a status. Auth/authorization
            // failures are 403; malformed/bad-sig are 400; replay/skew/ttl/loop
            // are 409 (the request was well-formed but not acceptable now).
            let status = match e {
                ingress::IngressError::UnknownPeer
                | ingress::IngressError::NotTrusted
                | ingress::IngressError::CapabilityDenied => "403 Forbidden",
                ingress::IngressError::Decode(_) | ingress::IngressError::BadSignature => {
                    "400 Bad Request"
                }
                ingress::IngressError::Replay
                | ingress::IngressError::SkewTooLarge { .. }
                | ingress::IngressError::TtlExpired
                | ingress::IngressError::LoopDetected => "409 Conflict",
            };
            json_err(status, e)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// P4 — outbound send
// ─────────────────────────────────────────────────────────────────────

/// `POST /cli/federation/send` — OWNER-gated (dispatcher enforces). Body:
/// `{"to":"<peer>::<workspace>::<agent>","text":"..."}`. Seals an
/// `AgentSignal`, enqueues it durably, then dials the peer and POSTs to its
/// `/cli/federation/inbound`. On a confirmed delivery the queued copy is
/// removed; otherwise it stays for the retry loop. Blocking (network I/O) —
/// the dispatcher runs it on a blocking worker.
pub fn handle_send(body: &[u8]) -> CliResponse {
    #[derive(serde::Deserialize)]
    struct Req {
        to: String,
        text: String,
    }
    let req: Req = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("bad send body: {e}")),
    };

    // Parse `<peer>::<workspace>::<agent>`.
    let parts: Vec<&str> = req.to.split("::").collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
        return CliResponse::bad_request("address must be <peer>::<workspace>::<agent>");
    }
    let (peer_sel, ws, agent) = (parts[0], parts[1], parts[2]);

    let store = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => return json_err("500 Internal Server Error", format!("load peer store: {e}")),
    };
    // Resolve the peer by fingerprint, label, OR subdomain (identity is the
    // key; label/subdomain are convenience selectors).
    let peer = store.list().iter().find(|p| {
        p.fingerprint == peer_sel || p.label == peer_sel || p.subdomain == peer_sel
    });
    let peer = match peer {
        Some(p) => p.clone(),
        None => return json_err("404 Not Found", format!("no pinned peer matches '{peer_sel}'")),
    };
    if peer.trust != k2_core::federation::PeerTrust::Trusted {
        return json_err(
            "403 Forbidden",
            format!("peer '{peer_sel}' is not Trusted (state={:?})", peer.trust),
        );
    }

    let key = match k2_core::tunnel::tls::load_or_generate_keypair() {
        Ok(k) => k,
        Err(e) => return json_err("500 Internal Server Error", format!("load keypair: {e}")),
    };
    let local_fp = federation::local_fingerprint().unwrap_or_default();

    // `from` is a Workspace address tagged with our fingerprint — informative
    // for the recipient; the recipient forces inbox delivery regardless. The
    // whole signal is signed end-to-end, so `from` is authenticated to the
    // verified peer (no forgeable plaintext `[from]`, Risk C1).
    let signal = AgentSignal {
        from: AgentAddress::Workspace {
            workspace: WorkspaceId(format!("peer:{local_fp}")),
        },
        to: AgentAddress::Agent {
            workspace: WorkspaceId(ws.to_string()),
            name: agent.to_string(),
        },
        delivery: Delivery::Inbox,
        ..AgentSignal::new(
            AgentAddress::Broadcast,
            AgentAddress::Broadcast,
            SignalKind::Msg { text: req.text },
        )
    };

    let bytes = match federation::seal(&signal, &key, &peer.subdomain, OUTBOUND_TTL) {
        Ok(b) => b,
        Err(e) => return json_err("500 Internal Server Error", format!("seal envelope: {e}")),
    };
    let msg_uuid = serde_json::from_slice::<federation::FederationEnvelope>(&bytes)
        .map(|e| e.payload.msg_uuid)
        .unwrap_or_default();

    // Durable enqueue BEFORE the dial (restart-survival; Risk M1).
    let queued_path = match outbox::enqueue(&peer.fingerprint, &bytes) {
        Ok(p) => p,
        Err(e) => return json_err("500 Internal Server Error", format!("enqueue outbox: {e}")),
    };

    // Dial the peer's E2E listener over the Connect tunnel + POST the envelope.
    match post_inbound(&peer_base_url(&peer.subdomain), &bytes) {
        Ok(()) => {
            outbox::remove(&queued_path);
            CliResponse::ok_json(
                serde_json::json!({ "status": "sent", "msg_uuid": msg_uuid, "peer": peer.fingerprint })
                    .to_string(),
            )
        }
        Err(e) => CliResponse::ok_json(
            serde_json::json!({
                "status": "queued",
                "msg_uuid": msg_uuid,
                "peer": peer.fingerprint,
                "hint": format!("delivery deferred to retry: {e}")
            })
            .to_string(),
        ),
    }
}

/// Resolve a peer's inbound base URL. Defaults to its `<subdomain>.k2.dev`
/// HTTPS endpoint (the relay carries only ciphertext; B terminates TLS).
/// `K2_FEDERATION_INBOUND_BASE` overrides it for local/loopback testing.
fn peer_base_url(subdomain: &str) -> String {
    if let Ok(base) = std::env::var("K2_FEDERATION_INBOUND_BASE") {
        if !base.trim().is_empty() {
            return base.trim().trim_end_matches('/').to_string();
        }
    }
    format!("https://{subdomain}.k2.dev")
}

/// POST a sealed envelope to `<base>/cli/federation/inbound`. Blocking
/// (reqwest::blocking). A non-2xx or transport error returns `Err` so the
/// caller leaves the message queued for retry.
pub fn post_inbound(base: &str, envelope_bytes: &[u8]) -> Result<(), String> {
    let url = format!("{base}/cli/federation/inbound");
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(envelope_bytes.to_vec())
        .send()
        .map_err(|e| format!("POST {url}: {e}"))?;
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(format!("peer rejected inbound: HTTP {}", status.as_u16()))
    }
}

// ─────────────────────────────────────────────────────────────────────
// P5 — roster (stub)
// ─────────────────────────────────────────────────────────────────────

/// `GET /cli/federation/roster` — STUB. Phase 5 fills this with a per-peer
/// cached workspace/agent/presence projection gated by `require_peer`. For now
/// it returns an empty projection so the route exists end-to-end.
pub fn handle_roster() -> CliResponse {
    CliResponse::ok_json(
        serde_json::json!({ "workspaces": [], "note": "roster stub — Phase 5 fills this" })
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::with_temp_home;
    use k2_core::federation::{FederationPeer, PeerTrust};

    fn body(v: serde_json::Value) -> Vec<u8> {
        v.to_string().into_bytes()
    }

    #[test]
    fn pair_request_then_owner_confirm_promotes_to_trusted() {
        with_temp_home(|| {
            // A peer presents a fresh key.
            let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let pem = peer_key.public_key_pem();

            let req = handle_pair_request(&body(serde_json::json!({
                "label": "rosson@laptop",
                "subdomain": "rosson",
                "public_key_pem": pem,
            })));
            assert_eq!(req.status, "200 OK", "request body: {}", req.body);
            let v: serde_json::Value = serde_json::from_str(&req.body).unwrap();
            let fp = v["fingerprint"].as_str().unwrap().to_string();
            let sas = v["sas"].as_str().unwrap().to_string();
            assert_eq!(v["trust"], "pending");

            // The store has it Pending (delivers nothing yet).
            let store = PeerStore::load().unwrap();
            assert_eq!(store.get(&fp).unwrap().trust, PeerTrust::Pending);

            // Owner confirms with the matching SAS → Trusted + caps.
            let conf = handle_pair_confirm(&body(serde_json::json!({
                "fingerprint": fp,
                "sas": sas,
            })));
            assert_eq!(conf.status, "200 OK", "confirm body: {}", conf.body);
            let store = PeerStore::load().unwrap();
            let p = store.get(&fp).unwrap();
            assert_eq!(p.trust, PeerTrust::Trusted);
            assert!(p.capabilities.contains("inbound"));
        });
    }

    #[test]
    fn pair_confirm_rejects_wrong_sas() {
        with_temp_home(|| {
            let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let req = handle_pair_request(&body(serde_json::json!({
                "label": "p", "subdomain": "p", "public_key_pem": peer_key.public_key_pem(),
            })));
            let v: serde_json::Value = serde_json::from_str(&req.body).unwrap();
            let fp = v["fingerprint"].as_str().unwrap();
            let conf = handle_pair_confirm(&body(serde_json::json!({
                "fingerprint": fp, "sas": "000000",
            })));
            assert_eq!(conf.status, "403 Forbidden", "wrong SAS must 403");
            // Still Pending.
            let store = PeerStore::load().unwrap();
            assert_eq!(store.get(fp).unwrap().trust, PeerTrust::Pending);
        });
    }

    #[test]
    fn inbound_rejects_unknown_peer() {
        with_temp_home(|| {
            // Seal an envelope from a key nobody pinned.
            let key = k2_core::tunnel::tls::load_or_generate_keypair().unwrap();
            let signal = AgentSignal::new(
                AgentAddress::Broadcast,
                AgentAddress::Agent {
                    workspace: WorkspaceId("ws".into()),
                    name: "bob".into(),
                },
                SignalKind::Msg { text: "hi".into() },
            );
            let bytes = federation::seal(&signal, &key, "peer", 8).unwrap();
            let resp = handle_inbound(&bytes);
            assert_eq!(resp.status, "403 Forbidden", "unknown peer must reject: {}", resp.body);
        });
    }

    #[test]
    fn inbound_delivers_valid_envelope_from_trusted_peer() {
        with_temp_home(|| {
            k2_core::db::init_for_tests();
            // Point the awareness inbox at a temp dir inside HOME.
            let inbox_root = dirs::home_dir().unwrap().join(".k2/awareness/inbox");
            k2_core::awareness::set_inbox_root(inbox_root.clone());

            // Pin a DISTINCT peer key (not the local key, or the loop guard
            // would trip on self-in-trace) as Trusted+inbound, then seal with it.
            let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
            let pem = key.public_key_pem();
            let mut store = PeerStore::default();
            let fp = store.upsert(FederationPeer::pin("peer", "peer", &pem).unwrap());
            store.set_trust(&fp, PeerTrust::Trusted);
            store.grant(&fp, "inbound");
            store.save().unwrap();

            let signal = AgentSignal::new(
                AgentAddress::Broadcast,
                AgentAddress::Agent {
                    workspace: WorkspaceId("ws".into()),
                    name: "bob".into(),
                },
                SignalKind::Msg { text: "hello over the wire".into() },
            );
            let bytes = federation::seal(&signal, &key, "self", 8).unwrap();
            let resp = handle_inbound(&bytes);
            assert_eq!(resp.status, "200 OK", "valid envelope must deliver: {}", resp.body);

            let drained = k2_core::awareness::inbox::drain(&inbox_root, "bob");
            assert_eq!(drained.len(), 1, "one inbox item delivered");
            assert_eq!(drained[0].delivery, Delivery::Inbox);
            k2_core::awareness::ingress::clear_inbox_root_for_tests();
        });
    }

    // ── P4 outbound: seal + durable enqueue + dial + POST over the wire ──

    /// Stand up a loopback TCP stub mimicking a peer's
    /// `/cli/federation/inbound`. It reads one HTTP request, captures the body,
    /// answers 200, and ships the captured body back via a oneshot. Returns the
    /// bound port + the receiver.
    async fn spawn_inbound_stub() -> (u16, tokio::sync::oneshot::Receiver<Vec<u8>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind stub");
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.expect("accept");
            let mut acc: Vec<u8> = Vec::new();
            let mut clen: Option<usize> = None;
            let mut body_start: Option<usize> = None;
            let mut buf = [0u8; 4096];
            loop {
                let n = match s.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                acc.extend_from_slice(&buf[..n]);
                if body_start.is_none() {
                    if let Some(pos) = acc.windows(4).position(|w| w == b"\r\n\r\n") {
                        body_start = Some(pos + 4);
                        let head = String::from_utf8_lossy(&acc[..pos]);
                        clen = head.lines().find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        });
                    }
                }
                if let (Some(bs), Some(cl)) = (body_start, clen) {
                    if acc.len() >= bs + cl {
                        break;
                    }
                }
            }
            let body = body_start.map(|bs| acc[bs..].to_vec()).unwrap_or_default();
            let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
            let _ = s.write_all(resp.as_bytes()).await;
            let _ = s.flush().await;
            let _ = tx.send(body);
        });
        (port, rx)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_seals_enqueues_and_posts_to_peer_inbound() {
        // Hold the crate-wide HOME lock for the whole test (TempHome) so the
        // keypair/store/outbox live in an isolated ~/.k2 and the env override is
        // serialized against other HOME-swapping tests.
        let _home = crate::test_support::TempHome::new();

        let (port, rx) = spawn_inbound_stub().await;
        let base = format!("http://127.0.0.1:{port}");

        // Pin a Trusted peer (subdomain "peer") with a fresh key, and capture
        // OUR local pubkey to verify the sealed envelope the stub receives.
        let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut store = PeerStore::default();
        let fp = store.upsert(
            FederationPeer::pin("teammate", "peer", peer_key.public_key_pem()).unwrap(),
        );
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, "inbound");
        store.save().unwrap();

        let local_key = k2_core::tunnel::tls::load_or_generate_keypair().unwrap();
        let local_pem = local_key.public_key_pem();

        // Run the blocking send on a worker (reqwest::blocking can't run on the
        // async thread). Override the dial target to our loopback stub.
        std::env::set_var("K2_FEDERATION_INBOUND_BASE", &base);
        let send_body = body(serde_json::json!({ "to": "peer::ws-b::bob", "text": "hi over the wire" }));
        let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
            .await
            .unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        assert_eq!(resp.status, "200 OK", "send body: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["status"], "sent", "must report sent on a 200 from the peer");

        // The peer received the EXACT sealed envelope; it opens against our
        // pinned key and carries the addressed signal.
        let received = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await
            .expect("stub must receive within timeout")
            .expect("stub body");
        let signal = federation::open(&received, &local_pem).expect("envelope opens against sender key");
        match signal.to {
            AgentAddress::Agent { name, workspace } => {
                assert_eq!(name, "bob");
                assert_eq!(workspace.0, "ws-b");
            }
            other => panic!("unexpected to-address: {other:?}"),
        }

        // Delivered → outbox drained.
        assert!(
            outbox::list_for_peer(&fp).is_empty(),
            "a confirmed delivery must remove the queued copy"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_to_unreachable_peer_leaves_message_queued() {
        let _home = crate::test_support::TempHome::new();

        let peer_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut store = PeerStore::default();
        let fp = store.upsert(
            FederationPeer::pin("teammate", "peer", peer_key.public_key_pem()).unwrap(),
        );
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, "inbound");
        store.save().unwrap();

        // Point at a dead port (nothing listening) → dial fails.
        std::env::set_var("K2_FEDERATION_INBOUND_BASE", "http://127.0.0.1:1");
        let send_body = body(serde_json::json!({ "to": "peer::ws::bob", "text": "queued please" }));
        let resp = tokio::task::spawn_blocking(move || handle_send(&send_body))
            .await
            .unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        assert_eq!(resp.status, "200 OK");
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["status"], "queued", "an undeliverable send must stay queued");
        assert_eq!(
            outbox::list_for_peer(&fp).len(),
            1,
            "the durable outbox must retain the message for retry"
        );
    }
}
