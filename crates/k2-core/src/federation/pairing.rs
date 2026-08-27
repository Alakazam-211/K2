//! Phase-2 pairing: TOFU pair-request → owner SAS-confirm.
//!
//! `prd-cross-server-agent-comms.md` §"Pairing (TOFU + SAS confirm)".
//! Two pure, store-mutating operations the daemon route handlers wrap:
//!
//!   - [`apply_pair_request`] — a peer presents its identity (label,
//!     subdomain, pinned SPKI PEM). We create a **`Pending`** row keyed by
//!     the fingerprint *derived from the presented key* (never caller-named).
//!     This is the UNAUTH path: it can ONLY ever create a `Pending` peer that
//!     **delivers/serves nothing** (Risk H4). Promotion is a separate,
//!     owner-gated step.
//!   - [`apply_pair_confirm`] — the OWNER approves a `Pending` peer by
//!     fingerprint after comparing the short **SAS** code out-of-band. On a
//!     SAS match the peer flips `Pending`→`Trusted` and is granted its
//!     capability set. Fail-closed: a `Blocked` peer is never promoted, an
//!     unknown fingerprint errors, a SAS mismatch errors.
//!
//! The **SAS** ([`sas_code`]) is a 6-digit code derived deterministically
//! from BOTH daemons' fingerprints (order-independent), so each side computes
//! the identical code from its local fingerprint + the peer's — the human
//! compares them (SSH-host-key / Tailscale ergonomics). No CA, no relay.

use sha2::{Digest, Sha256};

use super::dial;
use super::peers::{FederationPeer, PeerStore, PeerTrust};

/// Default capabilities granted to a freshly-confirmed peer when the owner
/// does not specify an explicit set: receive cross-server messages
/// ([`crate::federation::ingress::CAP_INBOUND`]) + answer roster queries.
/// Deliberately does NOT include any live/spawn capability (Risk H2 —
/// cross-server never spawns).
pub const DEFAULT_CONFIRM_CAPS: [&str; 2] = ["inbound", "roster"];

/// A peer's presented identity on the pairing path. The fingerprint is
/// NEVER taken from here — it is re-derived from `public_key_pem` so a
/// caller cannot name a key it does not hold.
///
/// F3: supply a Connect **subdomain** *or* an operator `--url` / `baseUrl`.
/// Empty both is still 400.
#[derive(Debug, Clone)]
pub struct PairRequest {
    pub label: String,
    pub subdomain: String,
    /// Dial base URL (`http://192.168.1.50:38471`). Optional when
    /// `subdomain` is a Connect label.
    pub base_url: String,
    pub public_key_pem: String,
}

/// Result of [`apply_pair_request`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairOutcome {
    /// The fingerprint derived from the presented key (authoritative id).
    pub fingerprint: String,
    /// The 6-digit SAS the owner compares out-of-band before confirming.
    pub sas: String,
    /// The peer's resulting trust state (always `Pending` for a NEW peer;
    /// an existing peer keeps its current state — request never promotes).
    pub trust: PeerTrust,
    /// True iff this request created a new peer row (vs. re-seen existing).
    pub created: bool,
}

/// Derive the 6-digit short-authentication-string from two daemon
/// fingerprints. Order-independent (the inputs are sorted) so both ends
/// compute the same code regardless of who initiated.
pub fn sas_code(fp_a: &str, fp_b: &str) -> String {
    let (lo, hi) = if fp_a <= fp_b {
        (fp_a, fp_b)
    } else {
        (fp_b, fp_a)
    };
    let mut h = Sha256::new();
    h.update(lo.as_bytes());
    h.update(b"::k2-federation-sas::");
    h.update(hi.as_bytes());
    let d = h.finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
    format!("{:06}", n % 1_000_000)
}

/// Constant-time-ish string equality for the SAS compare (length-leaking
/// but value-constant — same posture as the daemon's token compare). Both
/// inputs are short fixed-width codes so length is not secret anyway.
fn sas_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Apply an inbound pair request to `store`. Creates a `Pending` peer when
/// the fingerprint is unknown; for an already-known peer it does NOT change
/// trust (a `Trusted`/`Blocked` peer is untouched; a `Pending` peer's
/// cosmetic label/subdomain are refreshed). NEVER promotes — promotion is
/// [`apply_pair_confirm`] only.
///
/// `local_fp` is THIS daemon's fingerprint (for the SAS). Returns the
/// fingerprint + SAS + resulting trust. The caller persists the store.
pub fn apply_pair_request(
    store: &mut PeerStore,
    req: &PairRequest,
    local_fp: &str,
) -> Result<PairOutcome, String> {
    // F3: Connect subdomain OR operator --url / baseUrl. Empty both → 400.
    // A LAN URL is the routing hint under air-gap (never invent `.k2.dev`).
    let (subdomain, base_url) = normalize_pair_routing(&req.subdomain, &req.base_url)?;
    let fresh =
        FederationPeer::pin(&req.label, &subdomain, &req.public_key_pem)?.with_base_url(base_url);
    let fp = fresh.fingerprint.clone();
    // PR3: never pin this daemon as a remote peer (self-fingerprint).
    if !local_fp.is_empty() && fp == local_fp {
        return Err(
            "refusing to pair with self — presented fingerprint matches this \
             server's local fingerprint"
                .to_string(),
        );
    }
    let sas = sas_code(local_fp, &fp);

    let (trust, created) = match store.get(&fp) {
        Some(existing) => {
            let trust = existing.trust;
            // Only a still-Pending peer gets its cosmetic fields refreshed;
            // never re-pin (and thus reset epoch/caps) a Trusted/Blocked peer.
            if trust == PeerTrust::Pending {
                store.upsert(fresh);
            }
            (trust, false)
        }
        None => {
            store.upsert(fresh); // lands as Pending (FederationPeer::pin default)
            (PeerTrust::Pending, true)
        }
    };

    Ok(PairOutcome {
        fingerprint: fp,
        sas,
        trust,
        created,
    })
}

/// OWNER-gated confirm: promote a `Pending` peer to `Trusted` after the SAS
/// matches, granting it `capabilities` (or [`DEFAULT_CONFIRM_CAPS`] when the
/// slice is empty). Fail-closed:
///   - unknown fingerprint → `Err`
///   - `Blocked` peer → `Err` (never promotable without an explicit unblock)
///   - SAS mismatch → `Err` (no state change)
///
/// `local_fp` is THIS daemon's fingerprint (to recompute the expected SAS).
/// The caller persists the store after a successful confirm.
pub fn apply_pair_confirm(
    store: &mut PeerStore,
    fingerprint: &str,
    presented_sas: &str,
    local_fp: &str,
    capabilities: &[String],
) -> Result<(), String> {
    let peer = store
        .get(fingerprint)
        .ok_or_else(|| format!("no peer pinned with fingerprint {fingerprint}"))?;
    if peer.trust == PeerTrust::Blocked {
        return Err("peer is Blocked — unblock it explicitly before pairing".to_string());
    }

    let expected = sas_code(local_fp, fingerprint);
    if !sas_eq(presented_sas.trim(), &expected) {
        return Err("SAS code mismatch — refusing to confirm (fail-closed)".to_string());
    }

    if !store.set_trust(fingerprint, PeerTrust::Trusted) {
        return Err("peer vanished during confirm".to_string());
    }
    let caps: Vec<String> = if capabilities.is_empty() {
        DEFAULT_CONFIRM_CAPS.iter().map(|s| s.to_string()).collect()
    } else {
        capabilities.to_vec()
    };
    for cap in caps {
        store.grant(fingerprint, cap);
    }
    Ok(())
}

fn empty_pair_err() -> String {
    if crate::airgap::enabled() {
        "can't federate: need a LAN/Tailscale URL (--url / baseUrl, e.g. \
         http://192.168.1.50:38471) — air-gap will not dial <subdomain>.k2.dev"
            .to_string()
    } else {
        "can't federate: peer has no K2 Connect tunnel subdomain \
         (fail-closed — unroutable without <subdomain>.k2.dev). \
         Each side needs a purchased/bound tunnel under Settings → K2 Connect \
         (or k2.dev/dashboard) before Pair — or pass --url / baseUrl for LAN"
            .to_string()
    }
}

/// Collapse a Connect zone label; leave LAN host:port / MagicDNS intact.
fn normalize_connect_label(raw: &str) -> String {
    let mut n = raw.trim().to_lowercase();
    while n.ends_with(".k2.dev.k2.dev") {
        n.truncate(n.len() - ".k2.dev".len());
    }
    n.strip_suffix(".k2.dev").unwrap_or(&n).to_string()
}

fn normalize_pair_routing(subdomain: &str, base_url: &str) -> Result<(String, String), String> {
    let mut url = base_url.trim().to_string();
    let mut sub = subdomain.trim().to_string();
    if url.is_empty() && dial::is_absolute_http_url(&sub) {
        url = sub;
        sub = String::new();
    }
    if url.is_empty() && sub.is_empty() {
        return Err(empty_pair_err());
    }
    if !url.is_empty() {
        let url = dial::normalize_base_url(&url)?;
        dial::assert_lan_http(&url)?;
        if crate::airgap::enabled() {
            dial::assert_may_dial(&url)?;
        }
        if sub.is_empty() {
            sub = dial::host_port(&url);
        } else {
            sub = normalize_connect_label(&sub);
            if sub.is_empty() {
                sub = dial::host_port(&url);
            }
        }
        return Ok((sub, url));
    }
    sub = normalize_connect_label(&sub);
    if sub.is_empty() {
        return Err(empty_pair_err());
    }
    Ok((sub, String::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_peer_pem() -> String {
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .expect("generate peer keypair")
            .public_key_pem()
    }

    #[test]
    fn sas_is_order_independent_and_six_digits() {
        let a = "aaaa";
        let b = "bbbb";
        let s1 = sas_code(a, b);
        let s2 = sas_code(b, a);
        assert_eq!(s1, s2, "SAS must be symmetric in its two fingerprints");
        assert_eq!(s1.len(), 6, "SAS is a fixed 6-digit code");
        assert!(s1.chars().all(|c| c.is_ascii_digit()));
        assert_ne!(
            sas_code("aaaa", "cccc"),
            s1,
            "different peers → different SAS"
        );
    }

    #[test]
    fn pair_request_creates_pending_only() {
        let mut store = PeerStore::default();
        let req = PairRequest {
            label: "rosson@laptop".into(),
            subdomain: "rosson".into(),
            base_url: String::new(),
            public_key_pem: fresh_peer_pem(),
        };
        let out = apply_pair_request(&mut store, &req, "local-fp").expect("request");
        assert!(out.created);
        assert_eq!(out.trust, PeerTrust::Pending);
        let peer = store.get(&out.fingerprint).expect("peer pinned");
        assert_eq!(
            peer.trust,
            PeerTrust::Pending,
            "request creates only Pending"
        );
        assert!(peer.capabilities.is_empty(), "no caps until owner confirm");
        // Re-seeing the same key does not duplicate or promote.
        let out2 = apply_pair_request(&mut store, &req, "local-fp").expect("re-request");
        assert!(!out2.created);
        assert_eq!(out2.fingerprint, out.fingerprint);
        assert_eq!(store.list().len(), 1, "re-request must not duplicate");
    }

    #[test]
    fn pair_request_rejects_empty_or_blank_subdomain() {
        // A peer pinned with a blank subdomain is permanently unroutable —
        // the request must fail closed and NOT create a peer row.
        for blank in ["", "   ", "\t"] {
            let mut store = PeerStore::default();
            let req = PairRequest {
                label: "p".into(),
                subdomain: blank.into(),
                base_url: String::new(),
                public_key_pem: fresh_peer_pem(),
            };
            let err = apply_pair_request(&mut store, &req, "local-fp")
                .expect_err("blank subdomain must be rejected");
            assert!(
                err.contains("subdomain"),
                "error must name the cause; got: {err}"
            );
            assert!(
                store.list().is_empty(),
                "a rejected pair request must NOT pin an (unroutable) peer"
            );
        }
    }

    #[test]
    fn pair_request_trims_subdomain_before_pinning() {
        // A padded-but-non-empty subdomain is accepted and stored TRIMMED so
        // it matches the `<agent>@<host>` routing-hint compare at send time.
        let mut store = PeerStore::default();
        let req = PairRequest {
            label: "p".into(),
            subdomain: "  rpm  ".into(),
            base_url: String::new(),
            public_key_pem: fresh_peer_pem(),
        };
        let out = apply_pair_request(&mut store, &req, "local-fp").expect("padded subdomain ok");
        assert_eq!(
            store.get(&out.fingerprint).unwrap().subdomain,
            "rpm",
            "subdomain must be persisted trimmed"
        );
    }

    #[test]
    fn confirm_promotes_only_on_matching_sas() {
        let mut store = PeerStore::default();
        let req = PairRequest {
            label: "p".into(),
            subdomain: "p".into(),
            base_url: String::new(),
            public_key_pem: fresh_peer_pem(),
        };
        let out = apply_pair_request(&mut store, &req, "local-fp").expect("request");
        let fp = out.fingerprint.clone();

        // Wrong SAS → rejected, peer stays Pending.
        let err = apply_pair_confirm(&mut store, &fp, "000000", "local-fp", &[])
            .expect_err("wrong SAS must fail");
        assert!(err.contains("mismatch"), "got: {err}");
        assert_eq!(store.get(&fp).unwrap().trust, PeerTrust::Pending);

        // Correct SAS → Trusted + default caps.
        let sas = sas_code("local-fp", &fp);
        apply_pair_confirm(&mut store, &fp, &sas, "local-fp", &[]).expect("confirm");
        let peer = store.get(&fp).unwrap();
        assert_eq!(peer.trust, PeerTrust::Trusted);
        assert!(peer.capabilities.contains("inbound"));
        assert!(peer.capabilities.contains("roster"));
    }

    #[test]
    fn confirm_rejects_unknown_and_blocked() {
        let mut store = PeerStore::default();
        // Unknown fingerprint.
        let err = apply_pair_confirm(&mut store, "deadbeef", "000000", "lf", &[])
            .expect_err("unknown must fail");
        assert!(err.contains("no peer"), "got: {err}");

        // Blocked peer is never promoted.
        let req = PairRequest {
            label: "p".into(),
            subdomain: "p".into(),
            base_url: String::new(),
            public_key_pem: fresh_peer_pem(),
        };
        let out = apply_pair_request(&mut store, &req, "lf").unwrap();
        store.set_trust(&out.fingerprint, PeerTrust::Blocked);
        let sas = sas_code("lf", &out.fingerprint);
        let err = apply_pair_confirm(&mut store, &out.fingerprint, &sas, "lf", &[])
            .expect_err("blocked must fail");
        assert!(err.contains("Blocked"), "got: {err}");
        assert_eq!(
            store.get(&out.fingerprint).unwrap().trust,
            PeerTrust::Blocked
        );
    }

    #[test]
    fn pair_request_rejects_self_fingerprint() {
        // PR3: presenting THIS daemon's key as a peer must fail closed.
        let pem = fresh_peer_pem();
        let fp = FederationPeer::pin("self", "self", &pem)
            .unwrap()
            .fingerprint;
        let mut store = PeerStore::default();
        let req = PairRequest {
            label: "myself".into(),
            subdomain: "self".into(),
            base_url: String::new(),
            public_key_pem: pem,
        };
        let err = apply_pair_request(&mut store, &req, &fp)
            .expect_err("self-fingerprint pair must be rejected");
        assert!(
            err.contains("self") || err.contains("local fingerprint"),
            "error must name self/local; got: {err}"
        );
        assert!(
            store.list().is_empty(),
            "rejected self-pair must not pin a peer row"
        );
    }

    #[test]
    fn pair_request_normalizes_double_zone_subdomain_to_bare() {
        let mut store = PeerStore::default();
        let req = PairRequest {
            label: "p".into(),
            subdomain: "rpm.k2.dev.k2.dev".into(),
            base_url: String::new(),
            public_key_pem: fresh_peer_pem(),
        };
        let out = apply_pair_request(&mut store, &req, "local-fp").expect("request");
        assert_eq!(
            store.get(&out.fingerprint).unwrap().subdomain,
            "rpm",
            "double-zone subdomain must collapse to bare routing label"
        );
    }

    #[test]
    fn pair_request_url_pins_lan_base_without_k2_dev() {
        let mut store = PeerStore::default();
        let req = PairRequest {
            label: "lan-box".into(),
            subdomain: String::new(),
            base_url: "http://192.168.1.50:38471".into(),
            public_key_pem: fresh_peer_pem(),
        };
        let out = apply_pair_request(&mut store, &req, "local-fp").expect("lan url");
        let peer = store.get(&out.fingerprint).expect("pinned");
        assert_eq!(peer.base_url, "http://192.168.1.50:38471");
        assert_eq!(peer.subdomain, "192.168.1.50:38471");
        assert!(
            !peer.base_url.contains("k2.dev") && !peer.subdomain.contains("k2.dev"),
            "LAN pair must not store a .k2.dev dial URL"
        );
        let sas = sas_code("local-fp", &out.fingerprint);
        apply_pair_confirm(&mut store, &out.fingerprint, &sas, "local-fp", &[]).expect("confirm");
        assert_eq!(
            store.get(&out.fingerprint).unwrap().trust,
            PeerTrust::Trusted
        );
    }

    #[test]
    fn pair_request_rejects_empty_url_and_empty_subdomain() {
        let mut store = PeerStore::default();
        let req = PairRequest {
            label: "p".into(),
            subdomain: String::new(),
            base_url: String::new(),
            public_key_pem: fresh_peer_pem(),
        };
        let err = apply_pair_request(&mut store, &req, "local-fp")
            .expect_err("empty url+subdomain must 400");
        assert!(
            err.contains("subdomain") || err.contains("baseUrl") || err.contains("url"),
            "got: {err}"
        );
        assert!(store.list().is_empty());
    }

    #[test]
    fn pair_request_https_rfc1918_is_teaching_error() {
        let mut store = PeerStore::default();
        let req = PairRequest {
            label: "p".into(),
            subdomain: String::new(),
            base_url: "https://192.168.1.50:38471".into(),
            public_key_pem: fresh_peer_pem(),
        };
        let err = apply_pair_request(&mut store, &req, "local-fp").expect_err("https lan");
        assert!(err.contains("HTTP"), "got: {err}");
        assert!(store.list().is_empty());
    }

    #[cfg(not(feature = "airgap"))]
    #[test]
    fn pair_request_connect_subdomain_unchanged_when_not_airgap() {
        let mut store = PeerStore::default();
        let req = PairRequest {
            label: "p".into(),
            subdomain: "rosson".into(),
            base_url: String::new(),
            public_key_pem: fresh_peer_pem(),
        };
        let out = apply_pair_request(&mut store, &req, "local-fp").expect("connect pair");
        let peer = store.get(&out.fingerprint).unwrap();
        assert_eq!(peer.subdomain, "rosson");
        assert!(peer.base_url.is_empty());
    }
}
