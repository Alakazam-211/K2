//! Signed address notices (0.45.2) — the daemon half of
//! [`k2_core::federation::address_notice`].
//!
//! - **Receiver** — `POST /cli/federation/address` ([`handle_address_notice`]).
//!   No token: the body is signed by the peer's federation key and verified
//!   against the key already pinned for its fingerprint. On success it moves
//!   ONLY that peer's stored address, writes an auth-audit line, and emits
//!   `federation_peers_changed`.
//! - **Sender** — [`spawn`] runs a sweep every 30 s while federation is on:
//!   compare this box's address (tunnel subdomain + advertised LAN URL) with
//!   the last one announced, issue a notice to every Trusted peer on a
//!   change, then deliver due notices (persisted outbox, backoff, 7-day
//!   limit). [`kick`] runs one pass at once (a finished subdomain switch).
//!   No sends while air-gap is on. A switch never waits on delivery.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use k2_core::federation::address_notice::{self as notice, SendResult};
use k2_core::federation::{self, FederationPeer, PeerStore};
use k2_core::log_debug;

use crate::cli_response::CliResponse;
use crate::session_events::{emit, SessionEvent};

const TICK_SECS: u64 = 30;
const SEND_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

fn tick_interval() -> Duration {
    let secs = std::env::var("K2_FED_ADDRESS_TICK_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(TICK_SECS);
    Duration::from_secs(secs)
}

fn json_status(status: &'static str, body: serde_json::Value) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: body.to_string() }
}

// ─────────────────────────────────────────────────────────────────────
// Receiver
// ─────────────────────────────────────────────────────────────────────

/// The refusal the public route sends. Unknown sender, untrusted sender and
/// a bad signature all read `rejected` (no hint which fingerprints are
/// pinned); the detail goes to the debug log only.
fn refused(e: &notice::NoticeError) -> CliResponse {
    log_debug!("[federation/address] notice refused: {} ({e})", e.code());
    let code = e.public_code();
    let error = if code == "rejected" { "rejected".to_string() } else { e.to_string() };
    json_status(e.status(), serde_json::json!({ "error": error, "code": code }))
}

/// `POST /cli/federation/address`. See the module doc.
pub fn handle_address_notice(body: &[u8]) -> CliResponse {
    // The peer-store transaction lock makes load → apply → save one step, so
    // two notices from the same peer cannot both pass the seq check. A
    // notice that changes nothing writes nothing.
    let local = match notice::LocalIdentity::current() {
        Ok(l) => l,
        Err(e) => {
            return json_status(
                "500 Internal Server Error",
                serde_json::json!({ "error": format!("local identity: {e}"), "code": "store_error" }),
            )
        }
    };
    // Decode and verify the signature on a snapshot BEFORE taking the store
    // lock: an unauthenticated body never waits on, or holds, the lock.
    let snapshot = match PeerStore::load() {
        Ok(s) => s,
        Err(e) => {
            return json_status(
                "500 Internal Server Error",
                serde_json::json!({ "error": format!("peer store: {e}"), "code": "store_error" }),
            )
        }
    };
    let verified = match notice::verify_notice(&snapshot, body, &local, Utc::now()) {
        Ok(v) => v,
        Err(e) => return refused(&e),
    };
    let tx = PeerStore::transact(|store| {
        let applied = notice::apply_verified(store, &verified, &local, Utc::now())?;
        let label = store
            .get(&applied.fingerprint)
            .map(|p| p.label.clone())
            .filter(|l| !l.trim().is_empty())
            .unwrap_or_else(|| applied.fingerprint.chars().take(16).collect());
        let write = applied.wrote;
        Ok::<_, notice::NoticeError>(((applied, label), write))
    });
    let (applied, label) = match tx {
        Err(e) => {
            return json_status(
                "500 Internal Server Error",
                serde_json::json!({ "error": format!("peer store: {e}"), "code": "store_error" }),
            )
        }
        Ok(Err(e)) => return refused(&e),
        Ok(Ok(v)) => v,
    };
    let describe = |a: &notice::PeerAddress| {
        match (a.subdomain.is_empty(), a.base_url.is_empty()) {
            (false, false) => format!("{} {}", a.subdomain, a.base_url),
            (false, true) => a.subdomain.clone(),
            (true, false) => a.base_url.clone(),
            (true, true) => "-".to_string(),
        }
    };
    if !applied.changed.is_empty() {
        log_debug!(
            "[federation/address] peer {} ({label}) moved {} -> {} (seq {})",
            applied.fingerprint,
            describe(&applied.previous),
            describe(&applied.current),
            applied.seq
        );
        k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
            "federation-address",
            &label,
            format!("ok:{}->{}", describe(&applied.previous), describe(&applied.current)),
            "federation",
            "-",
            "api",
        ));
        let _ = emit(SessionEvent::FederationPeersChanged {
            fingerprint: applied.fingerprint.clone(),
            reason: "address_changed".to_string(),
        });
    }
    CliResponse::ok_json(
        serde_json::json!({
            "ok": true,
            "seq": applied.seq,
            "changed": applied.changed,
            "conflicts": applied.conflicts,
        })
        .to_string(),
    )
}

// ─────────────────────────────────────────────────────────────────────
// Sender
// ─────────────────────────────────────────────────────────────────────

/// POST one signed notice to `peer` at the address this server dials it on.
fn send_notice(peer: &FederationPeer, body: &[u8]) -> SendResult {
    let base = match crate::federation_routes::gated_peer_base_url(peer) {
        Ok(b) => b,
        Err(e) => return SendResult::Transport(e),
    };
    let url = format!("{base}{}", notice::ROUTE);
    let client = match reqwest::blocking::Client::builder()
        // Never follow a redirect: a peer must not steer us to another host.
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(SEND_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(e) => return SendResult::Transport(format!("build http client: {e}")),
    };
    match client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body.to_vec())
        .send()
    {
        Ok(resp) => {
            let status = resp.status().as_u16();
            SendResult::Http(status, resp.text().unwrap_or_default())
        }
        Err(e) => SendResult::Transport(format!("POST {url}: {e}")),
    }
}

/// What one [`sweep_once`] did (logs / tests).
#[derive(Debug)]
pub struct SweepReport {
    pub issue: Option<notice::IssueOutcome>,
    pub sent: Vec<(String, notice::DeliveryOutcome)>,
}

static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

struct Flight;
impl Flight {
    fn acquire() -> Option<Self> {
        IN_FLIGHT.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).ok().map(|_| Flight)
    }
}
impl Drop for Flight {
    fn drop(&mut self) {
        IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

/// Observe this box's address, issue a notice on a change, then deliver
/// due notices through `send`. Blocking. `None` when another pass is in
/// flight.
pub fn sweep_once_with<F>(current: &notice::PeerAddress, send: F) -> Result<Option<SweepReport>, String>
where
    F: FnMut(&FederationPeer, &[u8]) -> SendResult,
{
    let Some(_flight) = Flight::acquire() else {
        return Ok(None);
    };
    let store = PeerStore::load()?;
    let now = Utc::now();
    let issue = notice::observe_and_issue(current, &store, now)?;
    if let notice::IssueOutcome::Issued { seq, address, peers } = &issue {
        log_debug!(
            "[federation/address] address is now {:?}; notice seq {seq} queued for {} peer(s)",
            address,
            peers.len()
        );
    }
    let key = federation::identity::load_or_create()?;
    let sent = notice::drain_with(&key, &store, now, send)?;
    for (fp, outcome) in &sent {
        log_debug!("[federation/address] notice to {fp}: {outcome:?}");
    }
    Ok(Some(SweepReport { issue: Some(issue), sent }))
}

/// One production pass (real address, real HTTP).
pub fn sweep_once() -> Result<Option<SweepReport>, String> {
    sweep_once_with(&notice::local_address(), send_notice)
}

/// Run one pass now on a background thread (a finished subdomain switch).
/// No-op while federation is off.
pub fn kick() {
    if !federation::enabled() {
        return;
    }
    let spawned = std::thread::Builder::new().name("fed-address".into()).spawn(|| {
        if let Err(e) = sweep_once() {
            log_debug!("[federation/address] WARN: pass failed: {e}");
        }
    });
    if let Err(e) = spawned {
        log_debug!("[federation/address] WARN: spawn pass thread: {e}");
    }
}

/// The background sweep. The first pass runs at once (boot).
pub fn spawn() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let tick = tick_interval();
        loop {
            if federation::enabled() {
                match tokio::task::spawn_blocking(sweep_once).await {
                    Ok(Err(e)) => log_debug!("[federation/address] WARN: pass failed: {e}"),
                    Err(e) => log_debug!("[federation/address] WARN: pass panicked: {e}"),
                    Ok(Ok(_)) => {}
                }
            }
            tokio::time::sleep(tick).await;
        }
    })
}

/// Per-peer notice status for `GET /cli/federation/peers` (outbound) —
/// `None` when this box never had to tell that peer anything.
pub fn notice_view(rows: &std::collections::BTreeMap<String, notice::NoticeRow>, fp: &str) -> serde_json::Value {
    match rows.get(fp) {
        None => serde_json::Value::Null,
        Some(r) => serde_json::json!({
            "status": r.status,
            "seq": r.seq,
            "target": r.target,
            "issued_at": r.issued_at.to_rfc3339(),
            "attempts": r.attempts,
            "next_attempt_at": r.next_attempt_at.to_rfc3339(),
            "last_error": r.last_error,
            "failed_reason": r.failed_reason,
            "delivered_at": r.delivered_at.map(|t| t.to_rfc3339()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::with_temp_home;
    use k2_core::federation::address_notice::{NoticeStatus, PeerAddress};
    use k2_core::federation::PeerTrust;

    fn keypair() -> rcgen::KeyPair {
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("gen")
    }

    fn fp_of(k: &rcgen::KeyPair) -> String {
        k2_core::federation::peers::fingerprint_of_spki_der(&k.public_key_der())
    }

    fn pin_trusted(store: &mut PeerStore, pem: String, sub: &str) -> String {
        let f = store.upsert(FederationPeer::pin("peer", sub, pem).expect("pin"));
        store.set_trust(&f, PeerTrust::Trusted);
        f
    }

    #[test]
    fn route_applies_a_verified_notice_and_refuses_strangers() {
        with_temp_home(|| {
            let peer = keypair();
            let mut store = PeerStore::default();
            let f = pin_trusted(&mut store, peer.public_key_pem(), "afs");
            store.save().expect("save");
            let n = notice::AddressNotice {
                v: notice::NOTICE_VERSION,
                fingerprint: f.clone(),
                to_fingerprint: federation::local_fingerprint().expect("local fp"),
                old_address: PeerAddress::new("afs", ""),
                new_address: PeerAddress::new("k2n1-m4", ""),
                issued_at: Utc::now().to_rfc3339(),
                seq: 42,
            };
            let body = serde_json::to_vec(&notice::sign(&n, &peer).expect("sign")).expect("enc");

            let r = handle_address_notice(&body);
            assert_eq!(r.status, "200 OK", "{}", r.body);
            let p = PeerStore::load().expect("load").get(&f).cloned().expect("peer");
            assert_eq!(p.subdomain, "k2n1-m4");
            assert_eq!(p.address_seq, 42);
            let audit = k2_core::auth_audit::tail(10).expect("audit");
            assert!(
                audit.iter().any(|l| l["event"] == "federation-address"),
                "an audit line must be written: {audit:?}"
            );

            // Same bytes again: replay.
            let r = handle_address_notice(&body);
            assert_eq!(r.status, "409 Conflict", "{}", r.body);
            assert!(r.body.contains("\"code\":\"replay\""), "{}", r.body);

            // A stranger's notice: 403, nothing changes.
            let stranger = keypair();
            let mut sn = n.clone();
            sn.fingerprint = fp_of(&stranger);
            sn.seq = 99;
            let sbody = serde_json::to_vec(&notice::sign(&sn, &stranger).expect("sign")).expect("enc");
            let unknown = handle_address_notice(&sbody);
            assert_eq!(unknown.status, "403 Forbidden", "{}", unknown.body);
            let after = PeerStore::load().expect("load");
            assert_eq!(after.list().len(), 1);
            assert_eq!(after.get(&f).expect("peer").subdomain, "k2n1-m4");

            // A bad signature (the pinned peer's fingerprint, another key).
            let mut forged = sn.clone();
            forged.fingerprint = f.clone();
            forged.seq = 100;
            let mut fbody: serde_json::Value =
                serde_json::to_value(notice::sign(&sn, &stranger).expect("sign")).expect("val");
            fbody["notice"] = serde_json::to_value(&forged).expect("val");
            let bad_sig = handle_address_notice(fbody.to_string().as_bytes());

            // An untrusted (Pending) sender with a valid signature.
            let pending_key = keypair();
            let mut store = PeerStore::load().expect("load");
            let pf = store.upsert(FederationPeer::pin("pend", "pend", pending_key.public_key_pem()).expect("pin"));
            store.save().expect("save");
            let mut pn = n.clone();
            pn.fingerprint = pf;
            pn.old_address = PeerAddress::new("pend", "");
            pn.seq = 5;
            let pbody = serde_json::to_vec(&notice::sign(&pn, &pending_key).expect("sign")).expect("enc");
            let untrusted = handle_address_notice(&pbody);

            // All three read the same: no hint which fingerprints are pinned.
            for r in [&unknown, &bad_sig, &untrusted] {
                assert_eq!(r.status, "403 Forbidden", "{}", r.body);
                assert_eq!(r.body, r#"{"code":"rejected","error":"rejected"}"#, "{}", r.body);
            }

            // Garbage: 400. Oversized: 400 before any parsing.
            assert_eq!(handle_address_notice(b"{").status, "400 Bad Request");
            let big = vec![b' '; notice::MAX_NOTICE_BYTES + 1];
            assert_eq!(handle_address_notice(&big).status, "400 Bad Request");
        });
    }

    /// Two "daemons" in one process: box A (this HOME) announces; box B's
    /// receiver is the same route handler, reached through a fake transport
    /// that swaps HOME to B's tree for the call. No network.
    #[test]
    fn end_to_end_switch_reaches_the_peer_and_a_too_old_peer_is_recorded() {
        let home_a = k2_core::test_env::TempHome::new();
        let a_key = federation::identity::load_or_create().expect("A identity");
        let a_pem = a_key.public_key_pem();
        let home_a_path = k2_core::paths::home_dir().expect("home a");

        // Box B: a separate HOME with A pinned as Trusted at "afs".
        let home_b = k2_core::test_env::unique_temp_path("fed-address-b");
        std::fs::create_dir_all(home_b.join(".k2")).expect("mk b");
        let b_key;
        {
            let _b = k2_core::test_env::EnvVar::set("HOME", home_b.to_string_lossy().to_string());
            b_key = federation::identity::load_or_create().expect("B identity");
            let mut store = PeerStore::default();
            pin_trusted(&mut store, a_pem.clone(), "afs");
            store.save().expect("save B");
        }
        // Box C: an old peer (answers 404). Only its key is needed on A.
        let c_key = keypair();

        // A pins B and C.
        let mut a_store = PeerStore::default();
        let fb = pin_trusted(&mut a_store, b_key.public_key_pem(), "bbox");
        let fc = pin_trusted(&mut a_store, c_key.public_key_pem(), "cbox");
        a_store.save().expect("save A");

        let deliver = |peer: &FederationPeer, body: &[u8]| -> SendResult {
            if peer.fingerprint == fc {
                return SendResult::Http(404, r#"{"error":"not found"}"#.to_string());
            }
            let _b = k2_core::test_env::EnvVar::set("HOME", home_b.to_string_lossy().to_string());
            let r = handle_address_notice(body);
            let code = r.status.split(' ').next().and_then(|c| c.parse().ok()).expect("status code");
            SendResult::Http(code, r.body)
        };

        // Baseline, then the switch afs -> k2n1-m4.
        let first = sweep_once_with(&PeerAddress::new("afs", ""), deliver).expect("pass").expect("ran");
        assert!(matches!(first.issue, Some(notice::IssueOutcome::Baseline(_))), "{first:?}");
        let second = sweep_once_with(&PeerAddress::new("k2n1-m4", ""), deliver).expect("pass").expect("ran");
        assert_eq!(second.sent.len(), 2, "{second:?}");
        assert_eq!(k2_core::paths::home_dir().expect("home"), home_a_path, "HOME is A again");

        let rows = notice::rows().expect("rows");
        assert_eq!(rows[&fb].status, NoticeStatus::Delivered);
        assert_eq!(rows[&fc].status, NoticeStatus::Failed);
        assert_eq!(rows[&fc].failed_reason.as_deref(), Some("peer_too_old"));

        // B now dials A at the new name; its other fields are untouched.
        {
            let _b = k2_core::test_env::EnvVar::set("HOME", home_b.to_string_lossy().to_string());
            let b_store = PeerStore::load().expect("B store");
            let a_at_b = b_store.get(&fp_of(&a_key)).expect("A pinned at B");
            assert_eq!(a_at_b.subdomain, "k2n1-m4");
            assert_eq!(a_at_b.public_key_pem, a_pem);
            assert_eq!(a_at_b.trust, PeerTrust::Trusted);
        }

        // The peers listing shows the outbound notice state.
        let listed: serde_json::Value =
            serde_json::from_str(&crate::federation_routes::handle_peers().body).expect("peers json");
        let c_row = listed["peers"]
            .as_array()
            .expect("peers")
            .iter()
            .find(|p| p["fingerprint"] == fc.as_str())
            .expect("C listed");
        assert_eq!(c_row["address_notice"]["status"], "failed");
        assert_eq!(c_row["address_notice"]["failed_reason"], "peer_too_old");

        drop(home_a);
        let _ = std::fs::remove_dir_all(&home_b);
    }

    #[test]
    fn a_notice_send_never_follows_a_redirect() {
        with_temp_home(|| {
            use std::io::{Read, Write};
            let _inbound = k2_core::test_env::EnvVar::remove("K2_FEDERATION_INBOUND_BASE");
            let _air = k2_core::test_env::EnvVar::remove("K2_AIRGAP");
            // Where a redirect would lead: must never be dialed.
            let target = std::net::TcpListener::bind("127.0.0.1:0").expect("bind target");
            target.set_nonblocking(true).expect("nonblocking");
            let target_port = target.local_addr().expect("addr").port();
            // The peer answers 302 to that target.
            let peer = std::net::TcpListener::bind("127.0.0.1:0").expect("bind peer");
            let peer_port = peer.local_addr().expect("addr").port();
            let srv = std::thread::spawn(move || {
                let (mut s, _) = peer.accept().expect("accept");
                s.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout");
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{target_port}/cli/federation/address\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                s.write_all(resp.as_bytes()).expect("write");
            });
            let row = FederationPeer::pin("p", "p", keypair().public_key_pem())
                .expect("pin")
                .with_base_url(format!("http://127.0.0.1:{peer_port}"));
            let res = send_notice(&row, b"{}");
            srv.join().expect("peer thread");
            assert!(matches!(res, SendResult::Http(302, _)), "the 302 is returned as is: {res:?}");
            match target.accept() {
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                other => panic!("the redirect target must never be dialed: {other:?}"),
            }
        });
    }

    #[test]
    fn air_gap_pass_sends_nothing() {
        with_temp_home(|| {
            let _air = k2_core::test_env::EnvVar::set("K2_AIRGAP", "1");
            let mut store = PeerStore::default();
            pin_trusted(&mut store, keypair().public_key_pem(), "p");
            store.save().expect("save");
            sweep_once_with(&PeerAddress::new("afs", ""), |_, _| panic!("air-gap")).expect("pass");
            let r = sweep_once_with(&PeerAddress::new("b", ""), |_, _| panic!("air-gap must not send"))
                .expect("pass")
                .expect("ran");
            assert!(r.sent.is_empty());
            assert!(matches!(r.issue, Some(notice::IssueOutcome::Issued { .. })));
        });
    }
}
