//! Federation outbox DRAIN — the retry loop the durable outbox was built for.
//!
//! `handle_send` durably enqueues every sealed envelope before the dial and
//! the CLI tells users a queued message "will deliver when it reconnects" —
//! this module is what makes that promise TRUE (audit
//! `.k2/notes/federation-toggle-topology.md` finding #5: `outbox::list_all`
//! previously had zero production callers, so a message to a briefly
//! unreachable peer was queued forever).
//!
//! ## Triggers (all funnel into the ONE single-flight [`drain_peer`])
//!
//!   1. **Peer proved reachable** — a verified inbound message from a peer
//!      means it is online; `handle_inbound` calls [`note_peer_reachable`]
//!      which resets its backoff and kicks an immediate background drain.
//!      (There is no persistent per-peer connection to hook: federation
//!      dials are stateless HTTPS through the Connect relay, so "reconnect"
//!      is only observable as a successful dial/ingress.)
//!   2. **Periodic sweep** — [`spawn`] runs a low-cost tick (default 30s):
//!      expire stale entries, then drain every peer that has a non-empty
//!      queue AND is past its per-peer exponential backoff (30s doubling to
//!      a 2h cap, ±20% jitter). Peers with empty queues cost one readdir.
//!   3. **Daemon boot** — the sweep's first pass runs right after the
//!      readiness gate opens (main.rs spawns this next to the other reapers),
//!      so messages queued before a restart deliver on the next boot.
//!
//! ## Ordering + outcomes
//!
//! Per peer, envelopes deliver OLDEST-FIRST by the inner signal's creation
//! time; the first transport/5xx failure stops that peer's drain (retry on
//! the next trigger) so later messages can never overtake a stalled earlier
//! one. Terminal outcomes:
//!
//!   - 2xx delivered            → remove from the outbox.
//!   - 2xx `mode:"declined"`    → DEAD-LETTER (recipient consent is off;
//!     re-dialing can never flip an opt-out — mirrors `handle_send`).
//!   - 409 replay/duplicate     → the peer already handled this envelope
//!     (a prior delivery whose response was lost) → remove, count delivered.
//!   - other 4xx                → DEAD-LETTER (the peer understood and
//!     rejected; retrying a reject forever is the audit's anti-pattern).
//!   - transport / 5xx          → keep queued, stop this peer, back off.
//!   - 2xx live-delivery failure (`mode:"live"`, `delivered:false`, e.g. a
//!     wake race) → keep queued, stop this peer, back off (transient).
//!
//! Every envelope is RE-SEALED ([`k2_core::federation::reseal`] — fresh
//! `at`/nonce, SAME `msg_uuid`) just before its dial: a queued envelope
//! older than the receiver's skew window would be dead on arrival, and a
//! previously-dialed envelope's nonce is already burned in the receiver's
//! replay guard (recorded at the ingress gate even when delivery failed),
//! so re-dialing the same bytes would false-409 as a prior delivery.
//! Receive-side idempotency for the at-least-once redelivery this opens is
//! `federation::seen` (durable per-peer msg_uuid dedupe).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;

use k2_core::federation::{self, outbox, PeerStore, PeerTrust};
use k2_core::log_debug;

/// First-retry delay after a failed drain attempt.
const BACKOFF_BASE_SECS: u64 = 30;

/// Backoff ceiling — a peer that stays down is re-tried every ~2h, not
/// hammered every tick, and not forgotten.
const BACKOFF_CAP_SECS: u64 = 2 * 60 * 60;

/// Jitter applied to every backoff delay (±20%) so a fleet of daemons that
/// lost the same peer doesn't re-dial it in lockstep.
const JITTER_FRAC: f64 = 0.2;

/// Sweep cadence. Overridable via `K2_FED_DRAIN_TICK_SECS` for tests/tuning.
const TICK_INTERVAL_SECS: u64 = 30;

fn tick_interval() -> Duration {
    let secs = std::env::var("K2_FED_DRAIN_TICK_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(TICK_INTERVAL_SECS);
    Duration::from_secs(secs)
}

// ─────────────────────────────────────────────────────────────────────
// Pure backoff schedule (unit-tested)
// ─────────────────────────────────────────────────────────────────────

/// Exponential backoff for the Nth consecutive failure (N ≥ 1):
/// 30s, 60s, 120s, … capped at [`BACKOFF_CAP_SECS`]. Pure.
pub fn backoff_delay(consecutive_failures: u32) -> Duration {
    let n = consecutive_failures.max(1) - 1;
    // Saturate the shift well before u64 overflow; the cap dominates anyway.
    let secs = if n >= 32 {
        BACKOFF_CAP_SECS
    } else {
        (BACKOFF_BASE_SECS.saturating_mul(1u64 << n)).min(BACKOFF_CAP_SECS)
    };
    Duration::from_secs(secs)
}

/// Apply ±[`JITTER_FRAC`] jitter to `base` given `r` ∈ [0,1). Pure — the
/// caller supplies the randomness (production uses a clock-derived `r`).
pub fn with_jitter(base: Duration, r: f64) -> Duration {
    let r = r.clamp(0.0, 1.0);
    let factor = 1.0 - JITTER_FRAC + (2.0 * JITTER_FRAC * r);
    Duration::from_secs_f64(base.as_secs_f64() * factor)
}

/// Cheap non-crypto randomness for jitter (subsecond clock noise).
fn jitter_r() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    f64::from(nanos % 1_000_000) / 1_000_000.0
}

// ─────────────────────────────────────────────────────────────────────
// Per-peer state: backoff bookkeeping + single-flight
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct PeerBackoff {
    consecutive_failures: u32,
    next_attempt_at: Instant,
}

/// Backoff state per peer fingerprint. In-memory: a daemon restart resets it
/// (the boot sweep should try every queued peer once anyway).
fn backoff_state() -> &'static Mutex<HashMap<String, PeerBackoff>> {
    static S: std::sync::OnceLock<Mutex<HashMap<String, PeerBackoff>>> =
        std::sync::OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Fingerprints with a drain currently in flight — the single-flight gate.
fn in_flight() -> &'static Mutex<HashSet<String>> {
    static S: std::sync::OnceLock<Mutex<HashSet<String>>> = std::sync::OnceLock::new();
    S.get_or_init(|| Mutex::new(HashSet::new()))
}

/// RAII single-flight claim on a peer's drain. `acquire` returns `None` when
/// another drain of the SAME peer is already running (the caller skips —
/// every trigger funnels here, so triggers can never stack dials).
struct DrainClaim(String);

impl DrainClaim {
    fn acquire(fp: &str) -> Option<Self> {
        let mut set = in_flight().lock().expect("in_flight lock");
        if !set.insert(fp.to_string()) {
            return None;
        }
        Some(Self(fp.to_string()))
    }
}

impl Drop for DrainClaim {
    fn drop(&mut self) {
        let mut set = in_flight().lock().expect("in_flight lock");
        set.remove(&self.0);
    }
}

/// Is `fp` past its backoff window (or never failed)?
fn is_due(fp: &str) -> bool {
    let map = backoff_state().lock().expect("backoff lock");
    map.get(fp)
        .map(|b| Instant::now() >= b.next_attempt_at)
        .unwrap_or(true)
}

/// Record a failed drain attempt: bump the failure count and schedule the
/// next attempt per the jittered exponential schedule.
fn record_failure(fp: &str) {
    let mut map = backoff_state().lock().expect("backoff lock");
    let failures = map.get(fp).map(|b| b.consecutive_failures).unwrap_or(0) + 1;
    let delay = with_jitter(backoff_delay(failures), jitter_r());
    map.insert(
        fp.to_string(),
        PeerBackoff {
            consecutive_failures: failures,
            next_attempt_at: Instant::now() + delay,
        },
    );
    log_debug!(
        "[federation/drain] peer {fp}: attempt failed ({failures} consecutive) — next retry in {}s",
        delay.as_secs()
    );
}

/// Clear a peer's backoff (successful drain, or proof of reachability).
fn reset_backoff(fp: &str) {
    backoff_state().lock().expect("backoff lock").remove(fp);
}

/// A peer just proved it is reachable (e.g. a verified inbound message from
/// it): clear its backoff and kick an immediate background drain of anything
/// queued for it. Cheap no-op when its queue is empty.
pub fn note_peer_reachable(fp: &str) {
    reset_backoff(fp);
    if outbox::list_for_peer(fp).is_empty() {
        return;
    }
    kick(fp);
}

/// Fire-and-forget a background drain of one peer. Callers on blocking
/// workers (route handlers) use this so the drain never extends a request.
/// Single-flight inside [`drain_peer`] keeps concurrent kicks harmless.
pub fn kick(fp: &str) {
    let fp = fp.to_string();
    std::thread::Builder::new()
        .name("fed-drain".into())
        .spawn(move || {
            let outcome = drain_peer(&fp);
            log_debug!("[federation/drain] kick({fp}) → {outcome:?}");
        })
        .map(|_| ())
        .unwrap_or_else(|e| log_debug!("[federation/drain] WARN: spawn kick thread: {e}"));
}

// ─────────────────────────────────────────────────────────────────────
// The drain itself
// ─────────────────────────────────────────────────────────────────────

/// What one [`drain_peer`] pass did (for logs/tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrainOutcome {
    /// Another drain of this peer was already in flight — skipped.
    InFlight,
    /// Nothing queued for this peer.
    Empty,
    /// The peer isn't pinned/Trusted right now — left queued, backed off.
    PeerUnavailable,
    /// Every queued envelope reached a terminal outcome.
    Drained { delivered: usize, dead_lettered: usize },
    /// Stopped at the first transport/5xx/transient failure — the rest stays
    /// queued, in order, for the next trigger.
    Stalled { delivered: usize, dead_lettered: usize, remaining: usize },
    /// Air-gap is on — leftover outbox is not dialed.
    Airgap,
}

/// Drain one peer's queue, oldest-first, stopping at the first retryable
/// failure. Blocking (network I/O) — call from a blocking context.
pub fn drain_peer(fp: &str) -> DrainOutcome {
    let Some(_claim) = DrainClaim::acquire(fp) else {
        return DrainOutcome::InFlight;
    };

    let items = outbox::list_for_peer_ordered(fp);
    if items.is_empty() {
        return DrainOutcome::Empty;
    }

    // Resolve the dial target from the pinned peer. A missing or untrusted
    // peer keeps its queue (trust may come back) but backs off so the sweep
    // doesn't spin on it.
    let peer = PeerStore::load()
        .ok()
        .and_then(|s| s.get(fp).cloned())
        .filter(|p| p.trust == PeerTrust::Trusted);
    let Some(peer) = peer else {
        record_failure(fp);
        log_debug!(
            "[federation/drain] peer {fp}: not pinned/Trusted — {} message(s) stay queued",
            items.len()
        );
        return DrainOutcome::PeerUnavailable;
    };
    // F7: air-gap may dial RFC1918 / Tailscale / explicit http://; leftover
    // *.k2.dev must not SYN. Gate after resolving the URL so LAN drains.
    let base = match crate::federation_routes::gated_peer_base_url(&peer) {
        Ok(b) => b,
        Err(e) => {
            log_debug!(
                "[federation/drain] peer {fp}: not dialing ({e}) — {} message(s) stay queued",
                items.len()
            );
            return DrainOutcome::Airgap;
        }
    };

    let mut delivered = 0usize;
    let mut dead_lettered = 0usize;
    let total = items.len();

    for item in items {
        // Re-seal EVERY envelope before the dial (fresh at/nonce, SAME
        // msg_uuid; overwrite the queued file so the fresh seal survives a
        // restart). Two reasons this is unconditional:
        //   1. Skew — a queued envelope older than the receiver's window
        //      would be dead on arrival at the skew check.
        //   2. Nonce burn — the receiver records the nonce at the INGRESS
        //      GATE, before delivery. A prior attempt that reached the gate
        //      but failed to deliver (2xx-undelivered wake race, or a lost
        //      response) has already burned these bytes' nonce; re-dialing
        //      them yields a false 409 "replayed nonce" that reads as a
        //      prior delivery — silent message loss (caught live by the
        //      two-daemon e2e). A fresh nonce re-enters the gate cleanly;
        //      TRUE dedupe is the receiver's durable msg_uuid seen-store.
        let bytes = match reseal_queued(fp, &item.bytes) {
            Ok(b) => b,
            Err(e) => {
                // Can't re-sign (key unreadable?) — transient; retry later.
                log_debug!("[federation/drain] peer {fp}: reseal {}: {e}", item.msg_uuid);
                record_failure(fp);
                return DrainOutcome::Stalled {
                    delivered,
                    dead_lettered,
                    remaining: total - delivered - dead_lettered,
                };
            }
        };

        match post_inbound_status(&base, &bytes) {
            Err(transport) => {
                // Unreachable — everything from here stays queued, in order.
                log_debug!(
                    "[federation/drain] peer {fp}: unreachable at {} ({transport}) — {} message(s) stay queued",
                    item.msg_uuid,
                    total - delivered - dead_lettered
                );
                record_failure(fp);
                return DrainOutcome::Stalled {
                    delivered,
                    dead_lettered,
                    remaining: total - delivered - dead_lettered,
                };
            }
            Ok((status, body)) => match classify(status, &body) {
                Disposition::Delivered => {
                    outbox::remove(&item.path);
                    delivered += 1;
                    log_debug!(
                        "[federation/drain] peer {fp}: DELIVERED queued message {}",
                        item.msg_uuid
                    );
                }
                Disposition::DeadLetter(reason) => {
                    if let Err(e) = outbox::dead_letter(&item, &reason) {
                        log_debug!("[federation/drain] WARN: dead-letter {}: {e}", item.msg_uuid);
                        outbox::remove(&item.path);
                    }
                    dead_lettered += 1;
                }
                Disposition::Retry(why) => {
                    log_debug!(
                        "[federation/drain] peer {fp}: transient failure on {} ({why}) — retrying later",
                        item.msg_uuid
                    );
                    record_failure(fp);
                    return DrainOutcome::Stalled {
                        delivered,
                        dead_lettered,
                        remaining: total - delivered - dead_lettered,
                    };
                }
            },
        }
    }

    reset_backoff(fp);
    DrainOutcome::Drained { delivered, dead_lettered }
}

/// Re-seal a queued envelope with our key and overwrite the queued file
/// (same msg_uuid ⇒ same path) so the fresh seal is the durable copy.
fn reseal_queued(fp: &str, old_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let key = k2_core::tunnel::tls::load_or_generate_keypair()
        .map_err(|e| format!("load keypair: {e}"))?;
    let new_bytes = federation::reseal(old_bytes, &key).map_err(|e| e.to_string())?;
    outbox::enqueue(fp, &new_bytes)?;
    Ok(new_bytes)
}

/// How one dial result disposes of one queued envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Disposition {
    /// Terminal success — remove from the queue.
    Delivered,
    /// Terminal failure — dead-letter with this reason, move on.
    DeadLetter(String),
    /// Transient — keep queued, stop this peer's drain.
    Retry(String),
}

/// Classify a peer's HTTP response. See the module docs for the table.
fn classify(status: u16, body: &str) -> Disposition {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);

    if (200..300).contains(&status) {
        let delivered = parsed.get("delivered").and_then(|v| v.as_bool());
        let mode = parsed.get("mode").and_then(|v| v.as_str()).unwrap_or("");
        if delivered == Some(false) {
            if mode == "declined" {
                // An explicit consent DECLINE is terminal — re-dialing can
                // never flip the recipient's opt-out (mirrors handle_send).
                let reason = parsed
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("declined_by_peer");
                return Disposition::DeadLetter(format!("peer declined: {reason}"));
            }
            // Reached the peer but the LIVE delivery itself failed (wake
            // race, spawn failure) — transient; the message is still owed.
            let reason = parsed
                .get("result")
                .and_then(|r| r.get("reason"))
                .and_then(|v| v.as_str())
                .unwrap_or("live delivery failed");
            return Disposition::Retry(format!("2xx but undelivered: {reason}"));
        }
        return Disposition::Delivered;
    }

    let err = parsed
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or(body)
        .to_string();

    if status == 409 && (err.contains("replayed nonce") || err.contains("duplicate message")) {
        // The peer already handled this exact envelope — a prior delivery
        // whose response we lost. That IS a delivery.
        return Disposition::Delivered;
    }
    if (400..500).contains(&status) {
        // The peer understood the request and rejected it (bad envelope /
        // trust revoked / unknown workspace / stale skew that somehow dodged
        // the re-seal). Retrying a reject forever is the audit's
        // anti-pattern — dead-letter it, keep the row for inspection.
        return Disposition::DeadLetter(format!("peer rejected: HTTP {status}: {err}"));
    }
    Disposition::Retry(format!("HTTP {status}: {err}"))
}

/// POST a sealed envelope to `<base>/cli/federation/inbound`, returning the
/// HTTP status + body. Unlike [`crate::federation_routes::post_inbound`]
/// (which collapses every non-2xx into `Err`), the drain needs the status
/// AND body to tell a terminal 4xx reject from a retryable 5xx. `Err` here
/// means TRANSPORT failure only (unreachable / timeout).
fn post_inbound_status(base: &str, envelope_bytes: &[u8]) -> Result<(u16, String), String> {
    let url = format!("{base}/cli/federation/inbound");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(envelope_bytes.to_vec())
        .send()
        .map_err(|e| format!("POST {url}: {e}"))?;
    let status = resp.status().as_u16();
    let body = resp.text().unwrap_or_default();
    Ok((status, body))
}

// ─────────────────────────────────────────────────────────────────────
// The sweep task (trigger 2 + the boot pass)
// ─────────────────────────────────────────────────────────────────────

/// One sweep pass: expire over-age entries, then drain every peer that has
/// a non-empty queue and is past its backoff. Blocking. Returns the peers
/// it attempted (for logs/tests).
pub fn sweep_once() -> Vec<String> {
    let expired = outbox::prune_expired();
    if expired > 0 {
        log_debug!("[federation/drain] sweep: expired {expired} over-age queued message(s)");
    }
    let mut attempted = Vec::new();
    for fp in outbox::peers_with_queued() {
        if !is_due(&fp) {
            continue;
        }
        let outcome = drain_peer(&fp);
        log_debug!("[federation/drain] sweep: peer {fp} → {outcome:?}");
        attempted.push(fp);
    }
    attempted
}

/// Spawn the outbox drain sweeper as a background tokio task (mirrors
/// `session_reaper::spawn`'s shape). The first pass runs immediately —
/// that IS the boot trigger for messages queued before a restart. Each
/// tick is a no-op readdir when federation is off or every queue is empty.
pub fn spawn() -> JoinHandle<()> {
    tokio::spawn(async move { run().await })
}

async fn run() {
    let tick = tick_interval();
    log_debug!("[federation/drain] started — tick={}s", tick.as_secs());
    loop {
        if federation::enabled() {
            // Blocking network + disk I/O — keep it off the async reactor.
            let _ = tokio::task::spawn_blocking(sweep_once).await;
        }
        tokio::time::sleep(tick).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Pure backoff schedule ──

    #[test]
    fn backoff_schedule_doubles_from_30s_and_caps() {
        assert_eq!(backoff_delay(1), Duration::from_secs(30));
        assert_eq!(backoff_delay(2), Duration::from_secs(60));
        assert_eq!(backoff_delay(3), Duration::from_secs(120));
        assert_eq!(backoff_delay(8), Duration::from_secs(3840));
        assert_eq!(backoff_delay(9), Duration::from_secs(BACKOFF_CAP_SECS), "must cap");
        assert_eq!(backoff_delay(40), Duration::from_secs(BACKOFF_CAP_SECS), "cap holds far out");
        // Degenerate input (0 failures) behaves like the first failure.
        assert_eq!(backoff_delay(0), Duration::from_secs(30));
    }

    #[test]
    fn jitter_stays_within_20_percent() {
        let base = Duration::from_secs(100);
        assert_eq!(with_jitter(base, 0.0), Duration::from_secs(80));
        assert_eq!(with_jitter(base, 1.0), Duration::from_secs(120));
        let mid = with_jitter(base, 0.5);
        assert!(
            (mid.as_secs_f64() - 100.0).abs() < 0.001,
            "r=0.5 must be the identity, got {mid:?}"
        );
    }

    // ── Single-flight ──

    #[test]
    fn single_flight_claim_blocks_second_acquire_until_drop() {
        let fp = "single-flight-test-fp";
        let first = DrainClaim::acquire(fp).expect("first claim");
        assert!(
            DrainClaim::acquire(fp).is_none(),
            "second concurrent claim must be refused"
        );
        drop(first);
        assert!(
            DrainClaim::acquire(fp).is_some(),
            "claim must be reusable after release"
        );
    }

    #[test]
    fn drain_peer_reports_in_flight_when_claimed() {
        let fp = "in-flight-report-fp";
        let _claim = DrainClaim::acquire(fp).expect("claim");
        assert_eq!(drain_peer(fp), DrainOutcome::InFlight);
    }

    // ── Response classification (the terminal-outcome table) ──

    #[test]
    fn classify_2xx_delivered_is_delivered() {
        assert_eq!(
            classify(200, r#"{"delivered":true,"mode":"live"}"#),
            Disposition::Delivered
        );
        // A bare 200 with no body (older peer) still counts as delivered —
        // the transport succeeded and nothing said otherwise.
        assert_eq!(classify(200, ""), Disposition::Delivered);
    }

    #[test]
    fn classify_decline_is_dead_letter_not_retry() {
        let d = classify(
            200,
            r#"{"delivered":false,"mode":"declined","reason":"remote_instruction_disabled"}"#,
        );
        match d {
            Disposition::DeadLetter(reason) => {
                assert!(reason.contains("remote_instruction_disabled"), "got: {reason}")
            }
            other => panic!("a decline must dead-letter, got {other:?}"),
        }
    }

    #[test]
    fn classify_live_delivery_failure_is_retry_not_dead_letter() {
        let d = classify(
            200,
            r#"{"delivered":false,"mode":"live","result":{"success":false,"reason":"spawn_failed"}}"#,
        );
        assert!(
            matches!(d, Disposition::Retry(_)),
            "a transient live-delivery failure must stay queued, got {d:?}"
        );
    }

    #[test]
    fn classify_replay_409_counts_as_delivered() {
        assert_eq!(
            classify(409, r#"{"error":"replayed nonce"}"#),
            Disposition::Delivered,
            "a replay reject means the peer already handled the envelope"
        );
        assert_eq!(
            classify(409, r#"{"error":"duplicate message (already delivered)"}"#),
            Disposition::Delivered,
            "the durable msg_uuid dedupe reject is also a prior delivery"
        );
    }

    #[test]
    fn classify_other_4xx_dead_letters_and_5xx_retries() {
        assert!(matches!(
            classify(403, r#"{"error":"peer not trusted"}"#),
            Disposition::DeadLetter(_)
        ));
        assert!(matches!(
            classify(409, r#"{"error":"clock skew 9999s exceeds the accepted window"}"#),
            Disposition::DeadLetter(_)
        ));
        assert!(matches!(classify(500, "oops"), Disposition::Retry(_)));
        assert!(matches!(classify(502, ""), Disposition::Retry(_)));
    }

    // ── drain_peer against loopback stubs ──

    use k2_core::awareness::{AgentAddress, AgentSignal, SignalKind, WorkspaceId};
    use k2_core::federation::{FederationPeer, PeerStore};

    /// Pin `fp`'s peer as Trusted under subdomain "peer" and return the
    /// fingerprint. Uses a fresh key per call.
    fn pin_trusted() -> String {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut store = PeerStore::load().unwrap_or_default();
        let fp = store.upsert(FederationPeer::pin("drain-peer", "peer", key.public_key_pem()).unwrap());
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, "inbound");
        store.save().unwrap();
        fp
    }

    /// Seal `text` (from our own key) and enqueue it for `fp`, with the
    /// signal's order key pinned to `at`.
    fn enqueue_msg(fp: &str, text: &str, at: chrono::DateTime<chrono::Utc>) {
        let key = k2_core::tunnel::tls::load_or_generate_keypair().unwrap();
        let mut signal = AgentSignal::new(
            AgentAddress::Agent {
                workspace: WorkspaceId("ws-a".into()),
                name: "alice".into(),
            },
            AgentAddress::Agent {
                workspace: WorkspaceId("ws-b".into()),
                name: "bob".into(),
            },
            SignalKind::Msg { text: text.into() },
        );
        signal.at = at;
        let bytes = federation::seal(&signal, &key, "peer", 8).unwrap();
        outbox::enqueue(fp, &bytes).unwrap();
    }

    /// Multi-request loopback stub: answers every request with `status` +
    /// `body`, streaming each received request BODY out on the channel.
    async fn spawn_stub(
        status: &'static str,
        body: &'static str,
    ) -> (u16, tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.expect("bind stub");
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { break };
                let tx = tx.clone();
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
                let req_body = body_start.map(|bs| acc[bs..].to_vec()).unwrap_or_default();
                let _ = tx.send(req_body);
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = s.write_all(resp.as_bytes()).await;
                let _ = s.flush().await;
            }
        });
        (port, rx)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drain_delivers_in_order_and_clears_the_queue() {
        let _home = crate::test_support::TempHome::new();
        let (port, mut rx) = spawn_stub("200 OK", r#"{"delivered":true,"mode":"live"}"#).await;
        std::env::set_var("K2_FEDERATION_INBOUND_BASE", format!("http://127.0.0.1:{port}"));

        let fp = pin_trusted();
        let base = chrono::Utc::now();
        // Enqueue newest-first to prove the drain re-orders oldest-first.
        enqueue_msg(&fp, "third", base);
        enqueue_msg(&fp, "second", base - chrono::Duration::seconds(60));
        enqueue_msg(&fp, "first", base - chrono::Duration::seconds(120));

        let fp2 = fp.clone();
        let outcome = tokio::task::spawn_blocking(move || drain_peer(&fp2)).await.unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        assert_eq!(outcome, DrainOutcome::Drained { delivered: 3, dead_lettered: 0 });
        assert!(outbox::list_for_peer(&fp).is_empty(), "queue must be cleared");

        // The wire order is the SIGNAL order, oldest first.
        let mut texts = Vec::new();
        while let Ok(body) = rx.try_recv() {
            let env: federation::FederationEnvelope = serde_json::from_slice(&body).unwrap();
            match env.payload.signal.kind {
                SignalKind::Msg { text } => texts.push(text),
                other => panic!("unexpected kind: {other:?}"),
            }
        }
        assert_eq!(texts, vec!["first", "second", "third"], "must deliver oldest-first");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drain_stalls_on_5xx_and_backs_off_keeping_queue_in_order() {
        let _home = crate::test_support::TempHome::new();
        let (port, _rx) = spawn_stub("500 Internal Server Error", r#"{"error":"boom"}"#).await;
        std::env::set_var("K2_FEDERATION_INBOUND_BASE", format!("http://127.0.0.1:{port}"));

        let fp = pin_trusted();
        enqueue_msg(&fp, "queued", chrono::Utc::now());

        let fp2 = fp.clone();
        let outcome = tokio::task::spawn_blocking(move || drain_peer(&fp2)).await.unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        assert_eq!(
            outcome,
            DrainOutcome::Stalled { delivered: 0, dead_lettered: 0, remaining: 1 }
        );
        assert_eq!(outbox::list_for_peer(&fp).len(), 1, "5xx must keep the message queued");
        assert!(!is_due(&fp), "a failed drain must schedule a backoff window");
        assert!(
            outbox::list_dead_for_peer(&fp).is_empty(),
            "5xx must never dead-letter"
        );
        reset_backoff(&fp);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drain_dead_letters_a_decline_instead_of_retrying_forever() {
        let _home = crate::test_support::TempHome::new();
        let (port, _rx) = spawn_stub(
            "200 OK",
            r#"{"delivered":false,"mode":"declined","reason":"remote_instruction_disabled"}"#,
        )
        .await;
        std::env::set_var("K2_FEDERATION_INBOUND_BASE", format!("http://127.0.0.1:{port}"));

        let fp = pin_trusted();
        enqueue_msg(&fp, "unwanted", chrono::Utc::now());

        let fp2 = fp.clone();
        let outcome = tokio::task::spawn_blocking(move || drain_peer(&fp2)).await.unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        assert_eq!(outcome, DrainOutcome::Drained { delivered: 0, dead_lettered: 1 });
        assert!(outbox::list_for_peer(&fp).is_empty(), "declined message must leave the queue");
        let dead = outbox::list_dead_for_peer(&fp);
        assert_eq!(dead.len(), 1, "the decline must be dead-lettered for inspection");
        assert!(dead[0].reason.contains("remote_instruction_disabled"), "got: {}", dead[0].reason);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drain_treats_replay_reject_as_prior_delivery() {
        let _home = crate::test_support::TempHome::new();
        let (port, _rx) = spawn_stub("409 Conflict", r#"{"error":"replayed nonce"}"#).await;
        std::env::set_var("K2_FEDERATION_INBOUND_BASE", format!("http://127.0.0.1:{port}"));

        let fp = pin_trusted();
        enqueue_msg(&fp, "already landed", chrono::Utc::now());

        let fp2 = fp.clone();
        let outcome = tokio::task::spawn_blocking(move || drain_peer(&fp2)).await.unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        assert_eq!(
            outcome,
            DrainOutcome::Drained { delivered: 1, dead_lettered: 0 },
            "replay reject = the peer already has it = delivered"
        );
        assert!(outbox::list_for_peer(&fp).is_empty());
        assert!(outbox::list_dead_for_peer(&fp).is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drain_reseals_every_dial_with_fresh_nonce_preserving_msg_uuid() {
        let _home = crate::test_support::TempHome::new();
        let (port, mut rx) = spawn_stub("200 OK", r#"{"delivered":true,"mode":"live"}"#).await;
        std::env::set_var("K2_FEDERATION_INBOUND_BASE", format!("http://127.0.0.1:{port}"));

        let fp = pin_trusted();
        enqueue_msg(&fp, "reseal me", chrono::Utc::now());
        let original = outbox::list_for_peer(&fp).pop().unwrap();
        let orig_env: federation::FederationEnvelope =
            serde_json::from_slice(&original.bytes).unwrap();

        let fp2 = fp.clone();
        let outcome = tokio::task::spawn_blocking(move || drain_peer(&fp2)).await.unwrap();
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        assert_eq!(outcome, DrainOutcome::Drained { delivered: 1, dead_lettered: 0 });
        let wire = rx.try_recv().expect("stub must receive the resealed envelope");
        let sent: federation::FederationEnvelope = serde_json::from_slice(&wire).unwrap();
        assert_eq!(
            sent.payload.msg_uuid, orig_env.payload.msg_uuid,
            "re-seal must preserve the idempotency key"
        );
        // The nonce MUST rotate on every dial: a prior failed attempt has
        // already burned the old nonce in the receiver's replay guard, and
        // re-dialing it would false-409 as a prior delivery (message loss —
        // the live two-daemon e2e caught exactly this).
        assert_ne!(sent.payload.nonce, orig_env.payload.nonce, "nonce must rotate per dial");
        // And it must verify against OUR key (a real re-sign, not a splice).
        let key = k2_core::tunnel::tls::load_or_generate_keypair().unwrap();
        federation::open(&wire, &key.public_key_pem()).expect("resealed envelope must open");
    }

    #[test]
    fn drain_peer_airgap_does_not_dial_k2_dev() {
        let _home = crate::test_support::TempHome::new();
        let prev_air = std::env::var_os("K2_AIRGAP");
        std::env::set_var("K2_AIRGAP", "1");
        let spy = std::net::TcpListener::bind("127.0.0.1:0").expect("spy bind");
        spy.set_nonblocking(true).expect("nonblocking");
        // No inbound-base override: Connect subdomain would have been
        // https://peer.k2.dev — must refuse without SYN (F7).
        let prev_base = std::env::var_os("K2_FEDERATION_INBOUND_BASE");
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");
        let fp = pin_trusted();
        enqueue_msg(&fp, "leftover outbox", chrono::Utc::now());
        let outcome = drain_peer(&fp);
        assert_eq!(
            outcome,
            DrainOutcome::Airgap,
            "leftover *.k2.dev outbox must not dial when air-gap is on"
        );
        assert!(spy.accept().is_err(), "must not SYN *.k2.dev under air-gap");
        match prev_base {
            Some(p) => std::env::set_var("K2_FEDERATION_INBOUND_BASE", p),
            None => std::env::remove_var("K2_FEDERATION_INBOUND_BASE"),
        }
        match prev_air {
            Some(p) => std::env::set_var("K2_AIRGAP", p),
            None => std::env::remove_var("K2_AIRGAP"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drain_airgap_allows_lan_http_fake() {
        let _home = crate::test_support::TempHome::new();
        let prev_air = std::env::var_os("K2_AIRGAP");
        std::env::set_var("K2_AIRGAP", "1");
        let (port, _rx) = spawn_stub("200 OK", r#"{"delivered":true,"mode":"live"}"#).await;
        std::env::remove_var("K2_FEDERATION_INBOUND_BASE");

        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut store = PeerStore::load().unwrap_or_default();
        let fp = store.upsert(
            FederationPeer::pin("lan", "127.0.0.1", key.public_key_pem())
                .unwrap()
                .with_base_url(format!("http://127.0.0.1:{port}")),
        );
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, "inbound");
        store.save().unwrap();
        enqueue_msg(&fp, "lan leftover", chrono::Utc::now());

        let fp2 = fp.clone();
        let outcome = tokio::task::spawn_blocking(move || drain_peer(&fp2)).await.unwrap();
        match prev_air {
            Some(p) => std::env::set_var("K2_AIRGAP", p),
            None => std::env::remove_var("K2_AIRGAP"),
        }
        assert_eq!(
            outcome,
            DrainOutcome::Drained {
                delivered: 1,
                dead_lettered: 0
            }
        );
        assert!(outbox::list_for_peer(&fp).is_empty());
    }
}
