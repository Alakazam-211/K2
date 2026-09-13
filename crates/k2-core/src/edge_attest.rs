//! K2 edge attestation — Ed25519 proof that a request was proxied by a
//! first-party K2 edge (the `*.app.k2.dev` Worker or the k2.dev dashboard).
//!
//! PRD `prd-connect-login-edge-only-v1.md` §3. Pure verification: parse the
//! `X-K2-Edge-Sig` header, rebuild the signed message from the bytes the
//! daemon actually received, and check it against the trusted key set.
//!
//! ## Header
//!
//! ```text
//! X-K2-Edge-Sig: v1;kid=<key id>;ts=<unix seconds>;nonce=<16-32 hex>;ip=<client ip or ->;sig=<base64 std, 64 bytes>
//! ```
//!
//! ## Signed message (exact bytes, `\n`-joined, no trailing newline)
//!
//! ```text
//! k2-edge-v1
//! <METHOD>
//! <path without query>
//! <sub>          # tunnel subdomain label the edge is proxying for, lowercase
//! <ts>
//! <nonce>
//! <ip>
//! <sha256 hex of request body, lowercase; sha256("") for empty>
//! ```
//!
//! ## Checks (A1–A6)
//!
//! - **A1** `kid` in the trusted set = [`BAKED_KEYS`] plus the optional
//!   `~/.k2/edge-keys.json` override (`{"keys":[{"kid":"…","pub":"<b64 raw 32>"}]}`;
//!   same kid overrides, new kid extends; malformed → log + ignore the file).
//! - **A2** `|now − ts| ≤ 120 s`.
//! - **A3** Nonce unseen (bounded in-memory set, evicted by ts).
//! - **A4** `<sub>` equals this daemon's configured tunnel subdomain
//!   (compared lowercase) — a signature minted for another customer's host
//!   never verifies here.
//! - **A5** Body hash recomputed from the received bytes.
//! - **A6** Signature valid. Any failure is a REJECT; the caller renders the
//!   same response as "no header" (no oracle). The reason is only audited.
//!
//! **A7** is the caller's contract: this header is consulted ONLY on tunnel
//! ingress and ONLY for the gated paths. It never grants anything else.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use aws_lc_rs::signature::{UnparsedPublicKey, ED25519};
use base64::Engine;
use sha2::{Digest, Sha256};

/// The attestation header name (case-insensitive on the wire).
pub const HEADER_NAME: &str = "X-K2-Edge-Sig";

/// Fixed first line of the signed message.
pub const MESSAGE_PREFIX: &str = "k2-edge-v1";

/// A2 — maximum |now − ts| accepted, in seconds.
pub const MAX_SKEW_SECS: i64 = 120;

/// A3 — bounded nonce memory (≥ 10k per PRD).
pub const NONCE_CAPACITY: usize = 16_384;

/// Baked first-party PUBLIC keys (safe to commit). Rotation = add the new
/// kid here (release), switch the edge, remove the old kid next release.
pub const BAKED_KEYS: &[(&str, &str)] = &[
    (
        "k2-edge-app-2026-09",
        "fVxnKKXUpYB+h+9WIjLGPZ2yXngDYs04rUak/wQIQxk=",
    ),
    (
        "k2-edge-dash-2026-09",
        "FYeIcpLWpX6Wdc5NquR99Syc6hShaYsppFF9hMiaKVs=",
    ),
];

/// A verified attestation — what the gate hands to the audit record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attestation {
    pub kid: String,
    /// Client IP as asserted by the edge (`-` when the edge had none).
    pub ip: String,
    pub ts: i64,
    pub nonce: String,
}

/// Why verification failed. Every variant is a reject; `reason()` is the
/// audit token (`bad_attest:<reason>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttestError {
    /// No `X-K2-Edge-Sig` header at all (audited as `blocked_ingress`).
    Missing,
    Malformed(&'static str),
    UnknownKid,
    Stale,
    ReplayedNonce,
    WrongSub,
    BadSignature,
    /// This daemon has no tunnel subdomain configured — nothing can be
    /// attested for it.
    NoSubdomain,
}

impl AttestError {
    pub fn reason(&self) -> &'static str {
        match self {
            AttestError::Missing => "missing",
            AttestError::Malformed(_) => "malformed",
            AttestError::UnknownKid => "unknown_kid",
            AttestError::Stale => "stale",
            AttestError::ReplayedNonce => "replay",
            AttestError::WrongSub => "wrong_sub",
            AttestError::BadSignature => "bad_sig",
            AttestError::NoSubdomain => "no_subdomain",
        }
    }
}

impl std::fmt::Display for AttestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttestError::Malformed(why) => write!(f, "malformed: {why}"),
            other => f.write_str(other.reason()),
        }
    }
}

/// Parsed (not yet verified) header fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHeader {
    pub kid: String,
    pub ts: i64,
    pub nonce: String,
    pub ip: String,
    pub sig: Vec<u8>,
}

/// Lowercase hex SHA-256 of `body` (`sha256("")` for empty).
pub fn body_sha256_hex(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Rebuild the exact signed message bytes (§3). `path` must already be the
/// path WITHOUT its query string; `sub` must already be lowercase.
pub fn signed_message(
    method: &str,
    path: &str,
    sub: &str,
    ts: i64,
    nonce: &str,
    ip: &str,
    body: &[u8],
) -> Vec<u8> {
    format!(
        "{MESSAGE_PREFIX}\n{method}\n{path}\n{sub}\n{ts}\n{nonce}\n{ip}\n{}",
        body_sha256_hex(body)
    )
    .into_bytes()
}

/// Parse the header value. Strict: `v1` first, every field present exactly
/// once, nonce 16–32 lowercase/uppercase hex, sig 64 raw bytes.
pub fn parse_header(value: &str) -> Result<ParsedHeader, AttestError> {
    let value = value.trim();
    if !value.is_ascii() {
        return Err(AttestError::Malformed("non-ascii"));
    }
    let mut parts = value.split(';');
    if parts.next().map(str::trim) != Some("v1") {
        return Err(AttestError::Malformed("version"));
    }
    let mut kid: Option<String> = None;
    let mut ts: Option<i64> = None;
    let mut nonce: Option<String> = None;
    let mut ip: Option<String> = None;
    let mut sig: Option<Vec<u8>> = None;
    for part in parts {
        let part = part.trim();
        let Some((k, v)) = part.split_once('=') else {
            return Err(AttestError::Malformed("field"));
        };
        let v = v.trim();
        match k.trim() {
            "kid" => {
                if kid.is_some() || v.is_empty() || v.len() > 64 {
                    return Err(AttestError::Malformed("kid"));
                }
                if !v
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
                {
                    return Err(AttestError::Malformed("kid"));
                }
                kid = Some(v.to_string());
            }
            "ts" => {
                if ts.is_some() {
                    return Err(AttestError::Malformed("ts"));
                }
                ts = Some(v.parse::<i64>().map_err(|_| AttestError::Malformed("ts"))?);
            }
            "nonce" => {
                if nonce.is_some()
                    || v.len() < 16
                    || v.len() > 32
                    || !v.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err(AttestError::Malformed("nonce"));
                }
                nonce = Some(v.to_string());
            }
            "ip" => {
                if ip.is_some() || v.is_empty() || v.len() > 64 || v.contains('\n') {
                    return Err(AttestError::Malformed("ip"));
                }
                ip = Some(v.to_string());
            }
            "sig" => {
                if sig.is_some() {
                    return Err(AttestError::Malformed("sig"));
                }
                let raw = base64::engine::general_purpose::STANDARD
                    .decode(v)
                    .map_err(|_| AttestError::Malformed("sig"))?;
                if raw.len() != 64 {
                    return Err(AttestError::Malformed("sig"));
                }
                sig = Some(raw);
            }
            _ => return Err(AttestError::Malformed("unknown-field")),
        }
    }
    Ok(ParsedHeader {
        kid: kid.ok_or(AttestError::Malformed("kid"))?,
        ts: ts.ok_or(AttestError::Malformed("ts"))?,
        nonce: nonce.ok_or(AttestError::Malformed("nonce"))?,
        ip: ip.ok_or(AttestError::Malformed("ip"))?,
        sig: sig.ok_or(AttestError::Malformed("sig"))?,
    })
}

// ─────────────────────────────────────────────────────────────────────
// Trusted key set (A1)
// ─────────────────────────────────────────────────────────────────────

/// `~/.k2/edge-keys.json` — optional operator override / extension.
pub fn override_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
        .join("edge-keys.json")
}

fn decode_pub(b64: &str) -> Option<[u8; 32]> {
    let raw = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    raw.try_into().ok()
}

/// The trusted `kid → raw 32-byte public key` map: baked keys, then the
/// override file layered on top (same kid replaces, new kid extends). A
/// malformed file is logged and IGNORED — the baked set always stands.
pub fn trusted_keys() -> HashMap<String, [u8; 32]> {
    let mut keys: HashMap<String, [u8; 32]> = HashMap::new();
    for (kid, b64) in BAKED_KEYS {
        match decode_pub(b64) {
            Some(k) => {
                keys.insert((*kid).to_string(), k);
            }
            None => crate::log_debug!("[edge_attest] BUG: baked key {kid} does not decode"),
        }
    }
    let path = override_path();
    if !path.exists() {
        return keys;
    }
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(e) => {
            crate::log_debug!("[edge_attest] WARN read {}: {e} — using baked keys only", path.display());
            return keys;
        }
    };
    let parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            crate::log_debug!("[edge_attest] WARN malformed {}: {e} — ignoring file", path.display());
            return keys;
        }
    };
    let Some(list) = parsed.get("keys").and_then(|k| k.as_array()) else {
        crate::log_debug!("[edge_attest] WARN {} has no \"keys\" array — ignoring file", path.display());
        return keys;
    };
    for entry in list {
        let kid = entry.get("kid").and_then(|k| k.as_str()).unwrap_or("").trim();
        let pub_b64 = entry.get("pub").and_then(|k| k.as_str()).unwrap_or("").trim();
        if kid.is_empty() {
            crate::log_debug!("[edge_attest] WARN {}: entry without kid — skipped", path.display());
            continue;
        }
        match decode_pub(pub_b64) {
            Some(k) => {
                keys.insert(kid.to_string(), k);
            }
            None => crate::log_debug!(
                "[edge_attest] WARN {}: kid {kid} pub is not base64 of 32 bytes — skipped",
                path.display()
            ),
        }
    }
    keys
}

// ─────────────────────────────────────────────────────────────────────
// Nonce memory (A3)
// ─────────────────────────────────────────────────────────────────────

struct NonceSet {
    seen: HashMap<String, i64>,
    order: VecDeque<String>,
}

fn nonces() -> &'static Mutex<NonceSet> {
    static NONCES: OnceLock<Mutex<NonceSet>> = OnceLock::new();
    NONCES.get_or_init(|| {
        Mutex::new(NonceSet {
            seen: HashMap::new(),
            order: VecDeque::new(),
        })
    })
}

/// Atomically record `nonce` (stamped `ts`). Returns `false` when it was
/// already present within the window (replay). Evicts entries older than
/// the skew window (by ts) and caps the set at [`NONCE_CAPACITY`].
fn record_nonce(nonce: &str, ts: i64, now: i64) -> bool {
    let mut g = nonces().lock().unwrap_or_else(|p| p.into_inner());
    // Evict by ts: anything outside the accept window can never be
    // replayed successfully (A2 rejects it first), so it need not be kept.
    let cutoff = now - MAX_SKEW_SECS - 1;
    while let Some(front) = g.order.front() {
        let stale = g.seen.get(front).map(|t| *t < cutoff).unwrap_or(true);
        if !stale {
            break;
        }
        let k = g.order.pop_front().expect("front exists");
        g.seen.remove(&k);
    }
    while g.order.len() >= NONCE_CAPACITY {
        if let Some(k) = g.order.pop_front() {
            g.seen.remove(&k);
        }
    }
    if g.seen.contains_key(nonce) {
        return false;
    }
    g.seen.insert(nonce.to_string(), ts);
    g.order.push_back(nonce.to_string());
    true
}

// ─────────────────────────────────────────────────────────────────────
// Verify
// ─────────────────────────────────────────────────────────────────────

/// Current unix seconds.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Verify `header` (the raw `X-K2-Edge-Sig` value, or `None` when absent)
/// for a request `method path` whose received body is `body`, against this
/// daemon's tunnel subdomain `expected_sub` (any case; compared lowercase)
/// at time `now` (unix seconds). Every failure is a reject.
pub fn verify(
    header: Option<&str>,
    method: &str,
    path: &str,
    body: &[u8],
    expected_sub: &str,
    now: i64,
) -> Result<Attestation, AttestError> {
    let header = header.ok_or(AttestError::Missing)?;
    let parsed = parse_header(header)?;
    // A1
    let keys = trusted_keys();
    let pub_key = keys.get(&parsed.kid).ok_or(AttestError::UnknownKid)?;
    // A2
    if (now - parsed.ts).abs() > MAX_SKEW_SECS {
        return Err(AttestError::Stale);
    }
    // A4 — a daemon with no subdomain cannot be behind the edge.
    let sub = expected_sub.trim().to_ascii_lowercase();
    if sub.is_empty() {
        return Err(AttestError::NoSubdomain);
    }
    // A5 + A6 — path without query (caller strips), body hash recomputed.
    let path = path.split_once('?').map(|(p, _)| p).unwrap_or(path);
    let msg = signed_message(method, path, &sub, parsed.ts, &parsed.nonce, &parsed.ip, body);
    if UnparsedPublicKey::new(&ED25519, pub_key.as_slice())
        .verify(&msg, &parsed.sig)
        .is_err()
    {
        // The signature covers `sub`, so an envelope minted for another
        // customer's host fails HERE. The header carries no sub of its own
        // (v1 derives it from the daemon), so without a hint the honest
        // reason is `bad_sig`; `verify_with_host_hint` refines it to
        // `wrong_sub` when the caller knows the label the edge signed for.
        return Err(AttestError::BadSignature);
    }
    // A3 — only after the signature verified, so unauthenticated garbage
    // cannot pollute the nonce memory. Atomic check-and-insert.
    if !record_nonce(&parsed.nonce, parsed.ts, now) {
        return Err(AttestError::ReplayedNonce);
    }
    Ok(Attestation {
        kid: parsed.kid,
        ip: parsed.ip,
        ts: parsed.ts,
        nonce: parsed.nonce,
    })
}

/// Like [`verify`] but also classifies a signature that WOULD verify for
/// `signed_sub` (the label the edge actually signed for, when the caller
/// knows it — e.g. from the request's Host) as [`AttestError::WrongSub`]
/// instead of `bad_sig`. Purely an audit-quality refinement; the response
/// is identical. When `signed_sub` is `None` this is exactly [`verify`].
pub fn verify_with_host_hint(
    header: Option<&str>,
    method: &str,
    path: &str,
    body: &[u8],
    expected_sub: &str,
    signed_sub_hint: Option<&str>,
    now: i64,
) -> Result<Attestation, AttestError> {
    match verify(header, method, path, body, expected_sub, now) {
        Err(AttestError::BadSignature) => {
            if let (Some(hint), Some(h)) = (signed_sub_hint, header) {
                let hint = hint.trim().to_ascii_lowercase();
                if !hint.is_empty() && hint != expected_sub.trim().to_ascii_lowercase() {
                    if let Ok(parsed) = parse_header(h) {
                        let keys = trusted_keys();
                        if let Some(pub_key) = keys.get(&parsed.kid) {
                            let path = path.split_once('?').map(|(p, _)| p).unwrap_or(path);
                            let msg = signed_message(
                                method,
                                path,
                                &hint,
                                parsed.ts,
                                &parsed.nonce,
                                &parsed.ip,
                                body,
                            );
                            if UnparsedPublicKey::new(&ED25519, pub_key.as_slice())
                                .verify(&msg, &parsed.sig)
                                .is_ok()
                            {
                                return Err(AttestError::WrongSub);
                            }
                        }
                    }
                }
            }
            Err(AttestError::BadSignature)
        }
        other => other,
    }
}

// ─────────────────────────────────────────────────────────────────────
// Signer — an Ed25519 edge signer (ephemeral keys for tests; the same
// message builder a Rust edge would use). Never holds a baked private key.
// ─────────────────────────────────────────────────────────────────────

/// An Ed25519 signer that mints `X-K2-Edge-Sig` headers for a `kid`.
/// Tests generate an EPHEMERAL keypair and trust it through the
/// `~/.k2/edge-keys.json` override — the real private halves never enter
/// the repo.
pub struct EdgeSigner {
    kid: String,
    key: aws_lc_rs::signature::Ed25519KeyPair,
}

impl EdgeSigner {
    /// Generate a fresh keypair for `kid`.
    pub fn generate(kid: &str) -> Result<Self, String> {
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let doc = aws_lc_rs::signature::Ed25519KeyPair::generate_pkcs8(&rng)
            .map_err(|e| format!("generate ed25519 keypair: {e}"))?;
        let key = aws_lc_rs::signature::Ed25519KeyPair::from_pkcs8(doc.as_ref())
            .map_err(|e| format!("parse generated keypair: {e}"))?;
        Ok(Self {
            kid: kid.to_string(),
            key,
        })
    }

    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// Raw 32-byte public key, base64 (standard) — the `pub` field of the
    /// override file.
    pub fn public_key_b64(&self) -> String {
        use aws_lc_rs::signature::KeyPair;
        base64::engine::general_purpose::STANDARD.encode(self.key.public_key().as_ref())
    }

    /// The `~/.k2/edge-keys.json` document that trusts this signer.
    pub fn trust_file_json(&self) -> String {
        serde_json::json!({
            "keys": [ { "kid": self.kid, "pub": self.public_key_b64() } ]
        })
        .to_string()
    }

    /// Write [`Self::trust_file_json`] to [`override_path`] (0600).
    pub fn install_trust_file(&self) -> Result<(), String> {
        let path = override_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
        }
        std::fs::write(&path, self.trust_file_json())
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("chmod {}: {e}", path.display()))?;
        }
        Ok(())
    }

    /// Sign the §3 message and render the full header VALUE.
    pub fn header(
        &self,
        method: &str,
        path: &str,
        sub: &str,
        ts: i64,
        nonce: &str,
        ip: &str,
        body: &[u8],
    ) -> String {
        let path = path.split_once('?').map(|(p, _)| p).unwrap_or(path);
        let msg = signed_message(method, path, &sub.to_ascii_lowercase(), ts, nonce, ip, body);
        let sig = self.key.sign(&msg);
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.as_ref());
        format!("v1;kid={};ts={ts};nonce={nonce};ip={ip};sig={sig_b64}", self.kid)
    }
}

/// A fresh random 32-hex nonce.
pub fn random_nonce() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUB: &str = "rosson";

    fn now() -> i64 {
        now_unix()
    }

    /// Cross-check vector shared with the edge Worker implementer: the
    /// message + body hash are fixed regardless of key.
    #[test]
    fn cross_check_vector_message_and_body_hash() {
        let body = br#"{"username":"a","password":"b"}"#;
        assert_eq!(
            body_sha256_hex(body),
            "270bf84dcb14ea407cf5c7d0fc096be15a5ac43afff03775642c33f4ee944718"
        );
        assert_eq!(
            body_sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let msg = signed_message(
            "POST",
            "/cli/auth/login",
            "rosson",
            1757700000,
            "00112233445566778899aabbccddeeff",
            "203.0.113.9",
            body,
        );
        let expected = "k2-edge-v1\nPOST\n/cli/auth/login\nrosson\n1757700000\n00112233445566778899aabbccddeeff\n203.0.113.9\n270bf84dcb14ea407cf5c7d0fc096be15a5ac43afff03775642c33f4ee944718";
        assert_eq!(std::str::from_utf8(&msg).unwrap(), expected);
        assert!(!msg.ends_with(b"\n"), "no trailing newline");
        // Header shape.
        let s = EdgeSigner::generate("k2-edge-test").unwrap();
        let h = s.header(
            "POST",
            "/cli/auth/login?x=1",
            "rosson",
            1757700000,
            "00112233445566778899aabbccddeeff",
            "203.0.113.9",
            body,
        );
        assert!(
            h.starts_with("v1;kid=k2-edge-test;ts=1757700000;nonce=00112233445566778899aabbccddeeff;ip=203.0.113.9;sig="),
            "{h}"
        );
        let parsed = parse_header(&h).unwrap();
        assert_eq!(parsed.sig.len(), 64);
        // The header signed the path WITHOUT the query string: verify
        // against the bare path with the signer's key.
        use aws_lc_rs::signature::KeyPair;
        UnparsedPublicKey::new(&ED25519, s.key.public_key().as_ref())
            .verify(&msg, &parsed.sig)
            .expect("signature over query-stripped path");
    }

    #[test]
    fn baked_keys_decode_to_32_bytes() {
        for (kid, b64) in BAKED_KEYS {
            assert!(decode_pub(b64).is_some(), "baked key {kid} must be 32 raw bytes");
        }
        crate::tunnel::test_support::with_temp_home(|| {
            let keys = trusted_keys();
            assert_eq!(keys.len(), BAKED_KEYS.len());
            assert!(keys.contains_key("k2-edge-app-2026-09"));
            assert!(keys.contains_key("k2-edge-dash-2026-09"));
        });
    }

    #[test]
    fn parse_header_rejects_bad_shapes() {
        let good = EdgeSigner::generate("k").unwrap().header(
            "POST",
            "/cli/auth/login",
            SUB,
            1,
            "00112233445566778899aabbccddeeff",
            "-",
            b"",
        );
        assert!(parse_header(&good).is_ok());
        let cases = [
            "",
            "v2;kid=k;ts=1;nonce=00112233445566778899aabbccddeeff;ip=-;sig=AAAA",
            "v1;ts=1;nonce=00112233445566778899aabbccddeeff;ip=-;sig=AAAA",
            "v1;kid=k;ts=abc;nonce=00112233445566778899aabbccddeeff;ip=-;sig=AAAA",
            "v1;kid=k;ts=1;nonce=zz;ip=-;sig=AAAA",
            "v1;kid=k;ts=1;nonce=00112233445566778899aabbccddeeff;ip=-;sig=AAAA",
            "v1;kid=k;ts=1;nonce=00112233445566778899aabbccddeeff;ip=-;sig=AAAA;extra=1",
            "v1;kid=k;kid=k;ts=1;nonce=00112233445566778899aabbccddeeff;ip=-;sig=AAAA",
        ];
        for c in cases {
            assert!(
                matches!(parse_header(c), Err(AttestError::Malformed(_))),
                "must reject {c:?}"
            );
        }
    }

    #[test]
    fn verify_matrix_with_ephemeral_key() {
        crate::tunnel::test_support::with_temp_home(|| {
            let signer = EdgeSigner::generate("k2-edge-test").unwrap();
            signer.install_trust_file().unwrap();
            let body = br#"{"username":"a","password":"b"}"#;
            let t = now();

            // Good.
            let h = signer.header("POST", "/cli/auth/login", SUB, t, &random_nonce(), "203.0.113.9", body);
            let att = verify(Some(&h), "POST", "/cli/auth/login", body, "Rosson", t).expect("good sig");
            assert_eq!(att.kid, "k2-edge-test");
            assert_eq!(att.ip, "203.0.113.9");

            // Replay of the same header → replay.
            assert_eq!(
                verify(Some(&h), "POST", "/cli/auth/login", body, SUB, t),
                Err(AttestError::ReplayedNonce)
            );

            // Missing header.
            assert_eq!(
                verify(None, "POST", "/cli/auth/login", body, SUB, t),
                Err(AttestError::Missing)
            );

            // Stale (121 s old) and future (121 s ahead).
            let h = signer.header("POST", "/cli/auth/login", SUB, t - 121, &random_nonce(), "-", body);
            assert_eq!(verify(Some(&h), "POST", "/cli/auth/login", body, SUB, t), Err(AttestError::Stale));
            let h = signer.header("POST", "/cli/auth/login", SUB, t + 121, &random_nonce(), "-", body);
            assert_eq!(verify(Some(&h), "POST", "/cli/auth/login", body, SUB, t), Err(AttestError::Stale));
            // Exactly 120 s is still accepted.
            let h = signer.header("POST", "/cli/auth/login", SUB, t - 120, &random_nonce(), "-", body);
            assert!(verify(Some(&h), "POST", "/cli/auth/login", body, SUB, t).is_ok());

            // Wrong sub: signed for another customer's host.
            let h = signer.header("POST", "/cli/auth/login", "julie", t, &random_nonce(), "-", body);
            assert_eq!(
                verify(Some(&h), "POST", "/cli/auth/login", body, SUB, t),
                Err(AttestError::BadSignature)
            );
            assert_eq!(
                verify_with_host_hint(Some(&h), "POST", "/cli/auth/login", body, SUB, Some("julie"), t),
                Err(AttestError::WrongSub)
            );

            // Unknown kid.
            let other = EdgeSigner::generate("k2-edge-unknown").unwrap();
            let h = other.header("POST", "/cli/auth/login", SUB, t, &random_nonce(), "-", body);
            assert_eq!(
                verify(Some(&h), "POST", "/cli/auth/login", body, SUB, t),
                Err(AttestError::UnknownKid)
            );

            // Tampered body.
            let h = signer.header("POST", "/cli/auth/login", SUB, t, &random_nonce(), "-", body);
            assert_eq!(
                verify(Some(&h), "POST", "/cli/auth/login", b"{}", SUB, t),
                Err(AttestError::BadSignature)
            );

            // Wrong path / method.
            let h = signer.header("POST", "/cli/auth/login", SUB, t, &random_nonce(), "-", body);
            assert_eq!(
                verify(Some(&h), "POST", "/cli/auth/logout", body, SUB, t),
                Err(AttestError::BadSignature)
            );
            assert_eq!(
                verify(Some(&h), "GET", "/cli/auth/login", body, SUB, t),
                Err(AttestError::BadSignature)
            );

            // Query string is ignored on the daemon side too.
            let h = signer.header("POST", "/cli/auth/login", SUB, t, &random_nonce(), "-", body);
            assert!(verify(Some(&h), "POST", "/cli/auth/login?token=x", body, SUB, t).is_ok());

            // No subdomain configured → nothing can be attested.
            let h = signer.header("POST", "/cli/auth/login", "", t, &random_nonce(), "-", body);
            assert_eq!(
                verify(Some(&h), "POST", "/cli/auth/login", body, "", t),
                Err(AttestError::NoSubdomain)
            );
        });
    }

    #[test]
    fn override_file_extends_and_replaces_but_malformed_is_ignored() {
        crate::tunnel::test_support::with_temp_home(|| {
            // Malformed file → baked set intact.
            let path = override_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "{not json").unwrap();
            let keys = trusted_keys();
            assert_eq!(keys.len(), BAKED_KEYS.len(), "malformed override must be ignored");

            // Replace a baked kid + add a new one; a bad entry is skipped.
            let s = EdgeSigner::generate("k2-edge-app-2026-09").unwrap();
            let n = EdgeSigner::generate("k2-edge-new").unwrap();
            let doc = serde_json::json!({"keys": [
                {"kid": "k2-edge-app-2026-09", "pub": s.public_key_b64()},
                {"kid": "k2-edge-new", "pub": n.public_key_b64()},
                {"kid": "broken", "pub": "AAAA"},
                {"pub": n.public_key_b64()},
            ]});
            std::fs::write(&path, doc.to_string()).unwrap();
            let keys = trusted_keys();
            assert_eq!(keys.len(), BAKED_KEYS.len() + 1);
            assert!(!keys.contains_key("broken"));
            let replaced = keys.get("k2-edge-app-2026-09").unwrap();
            assert_eq!(
                base64::engine::general_purpose::STANDARD.encode(replaced),
                s.public_key_b64()
            );
            // The replaced kid now verifies with the override key, and the
            // baked key for that kid no longer does.
            let t = now();
            let h = s.header("POST", "/cli/auth/login", SUB, t, &random_nonce(), "-", b"");
            assert!(verify(Some(&h), "POST", "/cli/auth/login", b"", SUB, t).is_ok());
        });
    }

    #[test]
    fn nonce_memory_evicts_by_ts_and_caps() {
        // Distinct nonces in-window all record; an old-ts nonce is evicted
        // once a newer `now` moves the window past it.
        let base = 5_000_000_000i64;
        let n1 = format!("{:032x}", 0xabc_u128);
        assert!(record_nonce(&n1, base, base));
        assert!(!record_nonce(&n1, base, base), "second record is a replay");
        // Advance `now` past the window: n1 gets evicted, can record again.
        assert!(record_nonce(&n1, base + 400, base + 400));
        // Capacity cap: insert many and ensure the set never exceeds it.
        for i in 0..(NONCE_CAPACITY + 10) {
            let n = format!("{:032x}", 0x1000_0000_u128 + i as u128);
            assert!(record_nonce(&n, base + 400, base + 400));
        }
        let g = nonces().lock().unwrap();
        assert!(g.order.len() <= NONCE_CAPACITY);
        assert_eq!(g.order.len(), g.seen.len());
    }
}
