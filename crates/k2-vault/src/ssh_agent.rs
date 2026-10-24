//! `agent` delivery for `kind=ssh_key` (prd-vault-gaps-v1 item 3): the
//! pure half. Key parsing and checks, signing, and the small subset of the
//! ssh-agent protocol (draft-miller-ssh-agent) the daemon serves on a
//! per-session socket. No I/O here; the socket, peer check, rate limit and
//! lifecycle live in the daemon (`k2-daemon/src/vault/ssh_agent.rs`).
//!
//! Served:
//! - `REQUEST_IDENTITIES` (11) → `IDENTITIES_ANSWER` (12): the granted keys.
//! - `SIGN_REQUEST` (13) → `SIGN_RESPONSE` (14): ssh-ed25519, and RSA with
//!   `rsa-sha2-256` / `rsa-sha2-512` only (no SHA-1 `ssh-rsa`).
//! - `EXTENSION` (27) `session-bind@openssh.com` (OpenSSH ≥ 8.9): the host
//!   key, session id, the host's signature over it and `is_forwarding`.
//!
//! Everything else (add, remove, lock, smartcard, other extensions) is
//! `FAILURE`. A frame over [`MAX_FRAME`], or one that doesn't parse, closes
//! the connection.
//!
//! The private key is parsed per signature from the decrypted value and
//! dropped after (`ssh-key` zeroizes its private parts on drop).

use signature::{Signer, Verifier};
use ssh_key::private::{KeypairData, RsaKeypair};
use ssh_key::{Algorithm, EcdsaCurve, HashAlg, PrivateKey, PublicKey};

use crate::error::{Result, VaultError};

/// Largest frame the agent reads (PRD: > 256 KiB closes).
pub const MAX_FRAME: usize = 256 * 1024;

pub const FAILURE: u8 = 5;
pub const SUCCESS: u8 = 6;
pub const REQUEST_IDENTITIES: u8 = 11;
pub const IDENTITIES_ANSWER: u8 = 12;
pub const SIGN_REQUEST: u8 = 13;
pub const SIGN_RESPONSE: u8 = 14;
pub const EXTENSION: u8 = 27;

/// `SIGN_REQUEST` flags (draft-miller-ssh-agent §5.3).
pub const RSA_SHA2_256: u32 = 2;
pub const RSA_SHA2_512: u32 = 4;

pub const SESSION_BIND: &str = "session-bind@openssh.com";

/// Smallest RSA modulus the agent serves.
pub const RSA_MIN_BITS: usize = 2048;

// ── keys ───────────────────────────────────────────────────────────────

/// A parsed, unencrypted ssh private key of a type the agent serves.
pub struct AgentKey {
    key: PrivateKey,
}

impl std::fmt::Debug for AgentKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentKey").field("algorithm", &self.key.algorithm().as_str()).finish_non_exhaustive()
    }
}

fn bad_value(hint: impl Into<String>) -> VaultError {
    VaultError::bad_request("bad_value", hint)
}

/// A key type (or size) the agent doesn't serve: file delivery still works.
fn not_served(hint: impl Into<String>) -> VaultError {
    VaultError::bad_request("agent_key_type", hint)
}

fn encrypted() -> VaultError {
    VaultError::bad_request(
        "ssh_key_encrypted",
        "this key has a passphrase; agent delivery needs it without one (ssh-keygen -p -f <copy> -N ''), or use file delivery",
    )
}

impl AgentKey {
    /// An OpenSSH private key (`-----BEGIN OPENSSH PRIVATE KEY-----`), or an
    /// RSA key in PEM (PKCS#1 `BEGIN RSA PRIVATE KEY`, PKCS#8 `BEGIN
    /// PRIVATE KEY`). ed25519 and RSA ≥ 2048 bits; no passphrase.
    pub fn parse(value: &[u8]) -> Result<AgentKey> {
        let text = std::str::from_utf8(value).map_err(|_| bad_value("an ssh private key is text (OpenSSH or PEM)"))?;
        let t = text.trim();
        let key = if t.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----") {
            let k = PrivateKey::from_openssh(t).map_err(|_| bad_value("this isn't a readable OpenSSH private key"))?;
            if k.is_encrypted() {
                return Err(encrypted());
            }
            k
        } else if t.starts_with("-----BEGIN ENCRYPTED PRIVATE KEY-----")
            || (t.starts_with("-----BEGIN RSA PRIVATE KEY-----") && t.contains("ENCRYPTED"))
        {
            return Err(encrypted());
        } else if t.starts_with("-----BEGIN RSA PRIVATE KEY-----") {
            use rsa::pkcs1::DecodeRsaPrivateKey;
            let r = rsa::RsaPrivateKey::from_pkcs1_pem(t).map_err(|_| bad_value("this isn't a readable PEM RSA key"))?;
            from_rsa(r)?
        } else if t.starts_with("-----BEGIN PRIVATE KEY-----") {
            use rsa::pkcs8::DecodePrivateKey;
            let r = rsa::RsaPrivateKey::from_pkcs8_pem(t).map_err(|_| {
                not_served("agent delivery reads only RSA keys from PKCS#8 PEM; convert an ed25519 key to OpenSSH format (ssh-keygen -p -f <copy>), or use file delivery")
            })?;
            from_rsa(r)?
        } else if pem_private_key_block(t) {
            // EC, DSA or another PEM private key: fine for file delivery.
            return Err(not_served("agent delivery serves ed25519 and RSA keys; this PEM key type isn't one, so use file delivery"));
        } else {
            return Err(bad_value("agent delivery needs an ssh private key (OpenSSH or PEM)"));
        };
        match key.key_data() {
            KeypairData::Ed25519(_) => {}
            KeypairData::Rsa(r) => {
                let bits = rsa_bits(r)?;
                if bits < RSA_MIN_BITS {
                    return Err(not_served(format!("this RSA key is {bits} bits; agent delivery needs at least {RSA_MIN_BITS} (file delivery still works)")));
                }
            }
            KeypairData::Encrypted(_) => return Err(encrypted()),
            _ => {
                return Err(not_served(format!(
                    "agent delivery serves ed25519 and RSA keys, not {}; use file delivery",
                    key.algorithm().as_str()
                )))
            }
        }
        Ok(AgentKey { key })
    }

    /// The public key blob (the wire form ssh and `IDENTITIES_ANSWER` use).
    pub fn public_blob(&self) -> Vec<u8> {
        self.key.public_key().to_bytes().expect("a parsed key encodes")
    }

    /// The `.pub` line, with `comment`.
    pub fn public_line(&self, comment: &str) -> String {
        let mut p: PublicKey = self.key.public_key().clone();
        p.set_comment(comment);
        p.to_openssh().expect("a parsed key encodes")
    }

    /// `SHA256:…` fingerprint of the public key.
    pub fn fingerprint(&self) -> String {
        self.key.fingerprint(HashAlg::Sha256).to_string()
    }

    pub fn algorithm(&self) -> String {
        self.key.algorithm().as_str().to_string()
    }

    /// Sign `data` per the request `flags`; the result is the signature
    /// blob (`string algorithm, string signature`). RSA needs
    /// `RSA_SHA2_256` or `RSA_SHA2_512` (SHA-1 `ssh-rsa` is refused,
    /// `ssh_rsa_sha1`).
    pub fn sign(&self, data: &[u8], flags: u32) -> Result<Vec<u8>> {
        let (alg, sig): (&str, Vec<u8>) = match self.key.key_data() {
            KeypairData::Ed25519(k) => {
                let s: ssh_key::Signature = k.try_sign(data).map_err(|_| VaultError::internal("ed25519 signing failed"))?;
                ("ssh-ed25519", s.as_bytes().to_vec())
            }
            KeypairData::Rsa(k) => {
                // Blinded private-key operation (`try_sign_with_rng`): the
                // RNG only blinds the exponentiation (RUSTSEC-2023-0071,
                // Q7); the PKCS#1 v1.5 signature itself stays deterministic.
                use rsa::signature::RandomizedSigner;
                let r = to_rsa(k)?;
                let mut rng = ssh_key::rand_core::OsRng;
                if flags & RSA_SHA2_256 != 0 {
                    let sk = rsa::pkcs1v15::SigningKey::<sha2::Sha256>::new(r);
                    let s: rsa::pkcs1v15::Signature =
                        sk.try_sign_with_rng(&mut rng, data).map_err(|_| VaultError::internal("rsa signing failed"))?;
                    ("rsa-sha2-256", rsa::signature::SignatureEncoding::to_vec(&s))
                } else if flags & RSA_SHA2_512 != 0 {
                    let sk = rsa::pkcs1v15::SigningKey::<sha2::Sha512>::new(r);
                    let s: rsa::pkcs1v15::Signature =
                        sk.try_sign_with_rng(&mut rng, data).map_err(|_| VaultError::internal("rsa signing failed"))?;
                    ("rsa-sha2-512", rsa::signature::SignatureEncoding::to_vec(&s))
                } else {
                    return Err(VaultError::bad_request("ssh_rsa_sha1", "ssh-rsa (SHA-1) signatures aren't served; use rsa-sha2-256/512"));
                }
            }
            _ => return Err(VaultError::internal("unsupported key")),
        };
        let mut out = Vec::with_capacity(alg.len() + sig.len() + 8);
        put_string(&mut out, alg.as_bytes());
        put_string(&mut out, &sig);
        Ok(out)
    }
}

fn from_rsa(r: rsa::RsaPrivateKey) -> Result<PrivateKey> {
    let kp = RsaKeypair::try_from(r).map_err(|_| bad_value("this RSA key can't be used"))?;
    PrivateKey::new(KeypairData::Rsa(kp), "").map_err(|_| bad_value("this RSA key can't be used"))
}

/// ssh-key → rsa. Not `rsa::RsaPrivateKey::try_from(&RsaKeypair)`:
/// ssh-key 0.6.7 passes `p` twice as the primes there, so every key fails
/// (or, worse, would be built from the wrong primes). Built here from n, e,
/// d, p, q and validated (p·q = n, d matches e).
fn to_rsa(k: &RsaKeypair) -> Result<rsa::RsaPrivateKey> {
    let bad = || bad_value("this RSA key can't be used");
    let b = |m: &ssh_key::Mpint| m.as_positive_bytes().map(rsa::BigUint::from_bytes_be).ok_or_else(bad);
    let r = rsa::RsaPrivateKey::from_components(b(&k.public.n)?, b(&k.public.e)?, b(&k.private.d)?, vec![b(&k.private.p)?, b(&k.private.q)?])
        .map_err(|_| bad())?;
    r.validate().map_err(|_| bad())?;
    Ok(r)
}

fn rsa_bits(k: &RsaKeypair) -> Result<usize> {
    use rsa::traits::PublicKeyParts;
    Ok(to_rsa(k)?.size() * 8)
}

/// Can this value be served by the agent? (`bad_value`,
/// `ssh_key_encrypted`.)
pub fn check_value(value: &[u8]) -> Result<()> {
    AgentKey::parse(value).map(|_| ())
}

/// Does a value stored as `kind=ssh_key` read as an ssh private key, for
/// any delivery? An OpenSSH key (with or without a passphrase), an RSA PEM
/// key, or a well-formed passphrase PEM block (its BEGIN and END lines on
/// lines of their own). Catches a key pasted into a one-line field, whose
/// line breaks are gone (`bad_value`).
pub fn check_ssh_key_value(value: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(value).map_err(|_| bad_value("an ssh private key is text (OpenSSH or PEM)"))?;
    let t = text.trim();
    if pem_private_key_block(t) || public_of(value, "").is_ok() {
        return Ok(());
    }
    if !t.contains('\n') && t.contains("-----BEGIN ") {
        return Err(bad_value(
            "this key is on one line: its line breaks were lost (a one-line field?); paste it again in full, or use the file picker",
        ));
    }
    Err(bad_value("this doesn't read as an ssh private key (OpenSSH or PEM)"))
}

/// A well-formed PEM private-key block, encrypted or not, of any key type
/// (OpenSSH, RSA, EC, DSA, PKCS#8): a `-----BEGIN … PRIVATE KEY-----`
/// line, base64 body lines (RFC 1421 headers such as `Proc-Type:` and a
/// blank line allowed), and the matching `-----END … PRIVATE KEY-----`.
pub fn pem_private_key_block(text: &str) -> bool {
    let lines: Vec<&str> = text.trim().lines().map(str::trim).collect();
    if lines.len() < 3 {
        return false;
    }
    let (first, last) = (lines[0], lines[lines.len() - 1]);
    let Some(label) = first.strip_prefix("-----BEGIN ").and_then(|l| l.strip_suffix("-----")) else { return false };
    if !label.ends_with("PRIVATE KEY") || last != format!("-----END {label}-----") {
        return false;
    }
    let mut body = 0;
    for l in &lines[1..lines.len() - 1] {
        if l.is_empty() {
            continue;
        }
        if let Some((k, _)) = l.split_once(':') {
            if !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                continue;
            }
        }
        if !l.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=')) {
            return false;
        }
        body += 1;
    }
    body > 0
}

/// The `.pub` line (with `comment`) and `SHA256:` fingerprint of a stored
/// ssh private key: any key the agent serves, and any OpenSSH key at all
/// (a passphrase key's public half is in the clear).
pub fn public_of(value: &[u8], comment: &str) -> Result<(String, String)> {
    if let Ok(k) = AgentKey::parse(value) {
        return Ok((k.public_line(comment), k.fingerprint()));
    }
    let text = std::str::from_utf8(value).map_err(|_| bad_value("not an ssh private key"))?;
    let k = PrivateKey::from_openssh(text.trim()).map_err(|_| bad_value("not an ssh private key K2 can read the public half of"))?;
    let mut p: PublicKey = k.public_key().clone();
    p.set_comment(comment);
    let line = p.to_openssh().map_err(|_| bad_value("this key's public half can't be written"))?;
    Ok((line, p.fingerprint(HashAlg::Sha256).to_string()))
}

/// Verify an ssh signature blob over `data` with a public key blob. Used
/// by tests (and nothing else in production).
pub fn verify_blob(public_blob: &[u8], data: &[u8], sig_blob: &[u8]) -> bool {
    let Ok(pk) = PublicKey::from_bytes(public_blob) else { return false };
    let Some(sig) = parse_sig_blob(sig_blob) else { return false };
    Verifier::verify(pk.key_data(), data, &sig).is_ok()
}

fn parse_sig_blob(b: &[u8]) -> Option<ssh_key::Signature> {
    let mut r = Reader::new(b);
    let alg = std::str::from_utf8(r.string().ok()?).ok()?.to_string();
    let data = r.string().ok()?.to_vec();
    if !r.done() {
        return None;
    }
    ssh_key::Signature::new(Algorithm::new(&alg).ok()?, data).ok()
}

// ── wire ───────────────────────────────────────────────────────────────

/// A frame that doesn't parse: the connection closes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed;

struct Reader<'a> {
    b: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Reader { b }
    }
    fn u8(&mut self) -> std::result::Result<u8, Malformed> {
        let (x, rest) = self.b.split_first().ok_or(Malformed)?;
        self.b = rest;
        Ok(*x)
    }
    fn u32(&mut self) -> std::result::Result<u32, Malformed> {
        if self.b.len() < 4 {
            return Err(Malformed);
        }
        let (x, rest) = self.b.split_at(4);
        self.b = rest;
        Ok(u32::from_be_bytes([x[0], x[1], x[2], x[3]]))
    }
    fn string(&mut self) -> std::result::Result<&'a [u8], Malformed> {
        let n = self.u32()? as usize;
        if self.b.len() < n {
            return Err(Malformed);
        }
        let (x, rest) = self.b.split_at(n);
        self.b = rest;
        Ok(x)
    }
    fn done(&self) -> bool {
        self.b.is_empty()
    }
}

fn put_u32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_be_bytes());
}

fn put_string(out: &mut Vec<u8>, s: &[u8]) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s);
}

/// What a client asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Identities,
    Sign { key_blob: Vec<u8>, data: Vec<u8>, flags: u32 },
    SessionBind(SessionBind),
    /// Another extension, by name (`FAILURE`).
    Extension(String),
    /// Add, remove, lock, smartcard, anything else (`FAILURE`).
    Refused(u8),
}

/// `session-bind@openssh.com`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBind {
    pub host_key: Vec<u8>,
    pub session_id: Vec<u8>,
    pub signature: Vec<u8>,
    pub forwarding: bool,
}

/// What a bind's signature check found. The fingerprint is advisory (audit
/// only): it is the key the client says it bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindCheck {
    /// The host key signed the session id; its `SHA256:` fingerprint.
    Verified(String),
    /// A host key this build can't verify: one that doesn't parse (host
    /// certificates, key types ssh-key doesn't know) or ECDSA P-384/P-521.
    /// The fingerprint is over the raw blob. `is_forwarding` still counts
    /// and the connection keeps serving.
    Unverifiable(String),
    /// A host key type this build verifies, and the signature is wrong
    /// (or doesn't parse): the connection closes.
    Bad,
}

/// `SHA256:<base64, no padding>` of a raw key blob (what `ssh-keygen -l`
/// prints for a parsed key).
fn blob_fingerprint(blob: &[u8]) -> String {
    use base64::Engine as _;
    use sha2::Digest as _;
    format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(sha2::Sha256::digest(blob)))
}

impl SessionBind {
    pub fn check(&self) -> BindCheck {
        let Ok(pk) = PublicKey::from_bytes(&self.host_key) else { return BindCheck::Unverifiable(blob_fingerprint(&self.host_key)) };
        let fp = pk.fingerprint(HashAlg::Sha256).to_string();
        let verifiable = matches!(
            pk.algorithm(),
            Algorithm::Ed25519 | Algorithm::Rsa { .. } | Algorithm::Ecdsa { curve: EcdsaCurve::NistP256 }
        );
        let Some(sig) = parse_sig_blob(&self.signature) else {
            return if verifiable { BindCheck::Bad } else { BindCheck::Unverifiable(fp) };
        };
        if !verifiable || matches!(sig.algorithm(), Algorithm::Rsa { hash: None }) {
            return BindCheck::Unverifiable(fp);
        }
        match Verifier::verify(pk.key_data(), &self.session_id, &sig) {
            Ok(()) => BindCheck::Verified(fp),
            Err(_) => BindCheck::Bad,
        }
    }
}

/// Parse one frame's payload (the bytes after the length).
pub fn parse_request(payload: &[u8]) -> std::result::Result<Request, Malformed> {
    let mut r = Reader::new(payload);
    let t = r.u8()?;
    let req = match t {
        REQUEST_IDENTITIES => Request::Identities,
        SIGN_REQUEST => {
            let key_blob = r.string()?.to_vec();
            let data = r.string()?.to_vec();
            let flags = r.u32()?;
            Request::Sign { key_blob, data, flags }
        }
        EXTENSION => {
            let name = std::str::from_utf8(r.string()?).map_err(|_| Malformed)?.to_string();
            if name != SESSION_BIND {
                // The rest is the extension's own; not read.
                return Ok(Request::Extension(name));
            }
            let host_key = r.string()?.to_vec();
            let session_id = r.string()?.to_vec();
            let signature = r.string()?.to_vec();
            let forwarding = match r.u8()? {
                0 => false,
                1 => true,
                _ => return Err(Malformed),
            };
            Request::SessionBind(SessionBind { host_key, session_id, signature, forwarding })
        }
        other => return Ok(Request::Refused(other)),
    };
    if !r.done() {
        return Err(Malformed);
    }
    Ok(req)
}

/// A frame: `u32 length || payload`.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 4);
    put_u32(&mut out, payload.len() as u32);
    out.extend_from_slice(payload);
    out
}

/// `IDENTITIES_ANSWER` payload: `(public blob, comment)` per key.
pub fn identities_answer(ids: &[(Vec<u8>, String)]) -> Vec<u8> {
    let mut out = vec![IDENTITIES_ANSWER];
    put_u32(&mut out, ids.len() as u32);
    for (blob, comment) in ids {
        put_string(&mut out, blob);
        put_string(&mut out, comment.as_bytes());
    }
    out
}

/// `SIGN_RESPONSE` payload.
pub fn sign_response(sig_blob: &[u8]) -> Vec<u8> {
    let mut out = vec![SIGN_RESPONSE];
    put_string(&mut out, sig_blob);
    out
}

/// Client side, for tests and the daemon's own checks: encode a request.
pub mod client {
    use super::*;

    pub fn identities() -> Vec<u8> {
        vec![REQUEST_IDENTITIES]
    }

    pub fn sign(key_blob: &[u8], data: &[u8], flags: u32) -> Vec<u8> {
        let mut out = vec![SIGN_REQUEST];
        put_string(&mut out, key_blob);
        put_string(&mut out, data);
        put_u32(&mut out, flags);
        out
    }

    pub fn session_bind(b: &SessionBind) -> Vec<u8> {
        let mut out = vec![EXTENSION];
        put_string(&mut out, SESSION_BIND.as_bytes());
        put_string(&mut out, &b.host_key);
        put_string(&mut out, &b.session_id);
        put_string(&mut out, &b.signature);
        out.push(u8::from(b.forwarding));
        out
    }

    /// `IDENTITIES_ANSWER` → `(blob, comment)`s.
    pub fn parse_identities(payload: &[u8]) -> std::result::Result<Vec<(Vec<u8>, String)>, Malformed> {
        let mut r = Reader::new(payload);
        if r.u8()? != IDENTITIES_ANSWER {
            return Err(Malformed);
        }
        let n = r.u32()?;
        let mut out = vec![];
        for _ in 0..n {
            let b = r.string()?.to_vec();
            let c = String::from_utf8(r.string()?.to_vec()).map_err(|_| Malformed)?;
            out.push((b, c));
        }
        if !r.done() {
            return Err(Malformed);
        }
        Ok(out)
    }

    /// `SIGN_RESPONSE` → the signature blob; `None` for `FAILURE`.
    pub fn parse_sign(payload: &[u8]) -> std::result::Result<Option<Vec<u8>>, Malformed> {
        let mut r = Reader::new(payload);
        match r.u8()? {
            FAILURE if r.done() => Ok(None),
            SIGN_RESPONSE => {
                let s = r.string()?.to_vec();
                if !r.done() {
                    return Err(Malformed);
                }
                Ok(Some(s))
            }
            _ => Err(Malformed),
        }
    }

    /// Sign as a host would: the signature blob a session-bind carries.
    pub fn host_sign(host: &AgentKey, session_id: &[u8]) -> Vec<u8> {
        host.sign(session_id, RSA_SHA2_256).expect("host signs")
    }
}

/// Throwaway keys made at test time (no private key is checked in).
#[cfg(any(test, feature = "test-util"))]
pub mod testkeys {
    use std::sync::OnceLock;

    use rsa::pkcs1::EncodeRsaPrivateKey;
    use ssh_key::rand_core::OsRng;
    use ssh_key::{Algorithm, LineEnding, PrivateKey};

    /// A fresh ed25519 key, OpenSSH format.
    pub fn ed25519() -> String {
        let k = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).expect("ed25519 key");
        k.to_openssh(LineEnding::LF).expect("encode").to_string()
    }

    fn rsa(bits: usize) -> rsa::RsaPrivateKey {
        rsa::RsaPrivateKey::new(&mut OsRng, bits).expect("rsa key")
    }

    /// One RSA-2048 key per test binary (generation is slow), OpenSSH format.
    pub fn rsa2048() -> String {
        static K: OnceLock<String> = OnceLock::new();
        K.get_or_init(|| {
            let r = rsa2048_raw();
            let kp = ssh_key::private::RsaKeypair::try_from(r).expect("keypair");
            let k = PrivateKey::new(ssh_key::private::KeypairData::Rsa(kp), "").expect("key");
            k.to_openssh(LineEnding::LF).expect("encode").to_string()
        })
        .clone()
    }

    fn rsa2048_raw() -> rsa::RsaPrivateKey {
        static R: OnceLock<rsa::RsaPrivateKey> = OnceLock::new();
        R.get_or_init(|| rsa(2048)).clone()
    }

    /// The same RSA-2048 key as PEM (PKCS#1, `BEGIN RSA PRIVATE KEY`).
    pub fn rsa2048_pem() -> String {
        rsa2048_raw().to_pkcs1_pem(rsa::pkcs8::LineEnding::LF).expect("pem").to_string()
    }

    /// The same RSA-2048 key as PKCS#8 PEM (`BEGIN PRIVATE KEY`).
    pub fn rsa2048_pkcs8() -> String {
        use rsa::pkcs8::EncodePrivateKey;
        rsa2048_raw().to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).expect("pkcs8").to_string()
    }

    /// A 1024-bit RSA key (refused), PEM.
    pub fn rsa1024_pem() -> String {
        rsa(1024).to_pkcs1_pem(rsa::pkcs8::LineEnding::LF).expect("pem").to_string()
    }

    /// An ECDSA P-256 key (not served; also a host key type binds verify).
    pub fn p256() -> String {
        let k = PrivateKey::random(&mut OsRng, Algorithm::Ecdsa { curve: ssh_key::EcdsaCurve::NistP256 }).expect("p256 key");
        k.to_openssh(LineEnding::LF).expect("encode").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed() -> AgentKey {
        AgentKey::parse(testkeys::ed25519().as_bytes()).expect("test ed25519 key parses")
    }

    #[test]
    fn ed25519_signs_and_verifies() {
        let k = ed();
        assert_eq!(k.algorithm(), "ssh-ed25519");
        let blob = k.public_blob();
        let sig = k.sign(b"hello", 0).expect("sign");
        assert!(verify_blob(&blob, b"hello", &sig), "the signature verifies");
        assert!(!verify_blob(&blob, b"hellO", &sig), "a different message doesn't");
        assert!(k.public_line("DEPLOY_KEY").starts_with("ssh-ed25519 AAAA"));
        assert!(k.public_line("DEPLOY_KEY").ends_with(" DEPLOY_KEY"));
        assert!(k.fingerprint().starts_with("SHA256:"));
    }

    #[test]
    fn rsa_signs_sha2_only_and_reads_pem() {
        let k = AgentKey::parse(testkeys::rsa2048().as_bytes()).expect("openssh rsa");
        assert_eq!(k.algorithm(), "ssh-rsa");
        let blob = k.public_blob();
        for (flags, alg) in [(RSA_SHA2_256, "rsa-sha2-256"), (RSA_SHA2_512, "rsa-sha2-512"), (RSA_SHA2_256 | RSA_SHA2_512, "rsa-sha2-256")] {
            let sig = k.sign(b"data", flags).expect("sign");
            assert!(verify_blob(&blob, b"data", &sig), "{alg} verifies");
            let name_len = u32::from_be_bytes([sig[0], sig[1], sig[2], sig[3]]) as usize;
            assert_eq!(&sig[4..4 + name_len], alg.as_bytes());
        }
        assert_eq!(k.sign(b"data", 0).expect_err("sha1").code, "ssh_rsa_sha1", "ssh-rsa (SHA-1) is refused");
        // The same key from PKCS#1 PEM has the same public blob.
        let pem = AgentKey::parse(testkeys::rsa2048_pem().as_bytes()).expect("pem rsa");
        assert_eq!(pem.public_blob(), blob);
        assert_eq!(check_value(testkeys::rsa1024_pem().as_bytes()).expect_err("1024").code, "agent_key_type");
        assert_eq!(check_value(testkeys::p256().as_bytes()).expect_err("ecdsa").code, "agent_key_type");
    }

    #[test]
    fn values_that_are_not_servable_keys_are_refused() {
        assert_eq!(check_value(b"not a key").expect_err("text").code, "bad_value");
        assert_eq!(check_value(&[0xff, 0xfe]).expect_err("binary").code, "bad_value");
        assert_eq!(
            check_value(b"-----BEGIN ENCRYPTED PRIVATE KEY-----\nAAAA\n-----END ENCRYPTED PRIVATE KEY-----\n").expect_err("enc").code,
            "ssh_key_encrypted"
        );
        assert_eq!(
            check_value(b"-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00\n\nAAAA\n-----END RSA PRIVATE KEY-----\n")
                .expect_err("enc pem")
                .code,
            "ssh_key_encrypted"
        );
        assert_eq!(
            check_value(b"-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----\n").expect_err("junk").code,
            "bad_value"
        );
    }

    #[test]
    fn an_ssh_key_value_must_keep_its_line_breaks() {
        let k = testkeys::ed25519();
        check_ssh_key_value(k.as_bytes()).expect("a real key");
        check_ssh_key_value(testkeys::rsa2048_pem().as_bytes()).expect("PEM RSA");
        check_ssh_key_value(testkeys::p256().as_bytes()).expect("any OpenSSH key, even one agent doesn't serve");
        check_ssh_key_value(b"-----BEGIN ENCRYPTED PRIVATE KEY-----\nAAAA\n-----END ENCRYPTED PRIVATE KEY-----\n").expect("passphrase PEM");
        let flat = k.replace('\n', "");
        let e = check_ssh_key_value(flat.as_bytes()).expect_err("flattened");
        assert_eq!(e.code, "bad_value");
        assert!(e.hint.contains("line breaks"), "{}", e.hint);
        let spaced = k.replace('\n', " ");
        assert!(check_ssh_key_value(spaced.as_bytes()).expect_err("one line").hint.contains("line breaks"));
        let e = check_ssh_key_value(b"k2test-not-a-key").expect_err("text");
        assert_eq!(e.code, "bad_value");
        assert!(!e.hint.contains("line breaks"), "the hint is only for a flattened paste: {}", e.hint);
        // Every well-formed PEM private key block, any type, encrypted or not.
        for label in ["EC PRIVATE KEY", "PRIVATE KEY", "DSA PRIVATE KEY", "RSA PRIVATE KEY", "ENCRYPTED PRIVATE KEY", "OPENSSH PRIVATE KEY"] {
            let pem = format!("-----BEGIN {label}-----\nMHcCAQEEIBbase64bodyLine0123456789+/=\nAAAA\n-----END {label}-----\n");
            check_ssh_key_value(pem.as_bytes()).unwrap_or_else(|e| panic!("{label}: {e:?}"));
        }
        let mismatched = "-----BEGIN EC PRIVATE KEY-----\nAAAA\n-----END RSA PRIVATE KEY-----\n";
        assert!(check_ssh_key_value(mismatched.as_bytes()).is_err(), "BEGIN and END must match");
        let not_b64 = "-----BEGIN EC PRIVATE KEY-----\nnot base64 at all!\n-----END EC PRIVATE KEY-----\n";
        assert!(check_ssh_key_value(not_b64.as_bytes()).is_err());
    }

    #[test]
    fn requests_parse_and_bad_frames_are_malformed() {
        assert_eq!(parse_request(&client::identities()), Ok(Request::Identities));
        let s = client::sign(b"blob", b"data", RSA_SHA2_512);
        assert_eq!(parse_request(&s), Ok(Request::Sign { key_blob: b"blob".to_vec(), data: b"data".to_vec(), flags: 4 }));
        // Add identity, remove all, lock: refused, not malformed.
        for t in [17u8, 19, 22, 25] {
            assert_eq!(parse_request(&[t, 0, 0]), Ok(Request::Refused(t)));
        }
        // Truncated sign request, trailing bytes, empty payload.
        assert_eq!(parse_request(&s[..s.len() - 1]), Err(Malformed));
        let mut extra = s.clone();
        extra.push(0);
        assert_eq!(parse_request(&extra), Err(Malformed));
        assert_eq!(parse_request(&[]), Err(Malformed));
        // A length past the end.
        assert_eq!(parse_request(&[SIGN_REQUEST, 0xff, 0xff, 0xff, 0xff]), Err(Malformed));
        // Other extensions are named.
        let mut ext = vec![EXTENSION];
        put_string(&mut ext, b"query");
        assert_eq!(parse_request(&ext), Ok(Request::Extension("query".into())));
    }

    #[test]
    fn identities_round_trip() {
        let ids = vec![(b"k1".to_vec(), "ONE".to_string()), (b"k2".to_vec(), "TWO".to_string())];
        assert_eq!(client::parse_identities(&identities_answer(&ids)), Ok(ids));
        let sig = sign_response(b"sig");
        assert_eq!(client::parse_sign(&sig), Ok(Some(b"sig".to_vec())));
        assert_eq!(client::parse_sign(&[FAILURE]), Ok(None));
        assert_eq!(frame(b"ab"), vec![0, 0, 0, 2, b'a', b'b']);
    }

    #[test]
    fn session_bind_checks_the_host_signature() {
        let host = ed();
        let sid = b"exchange-hash-0123456789abcdef".to_vec();
        let good = SessionBind { host_key: host.public_blob(), session_id: sid.clone(), signature: client::host_sign(&host, &sid), forwarding: true };
        let parsed = parse_request(&client::session_bind(&good)).expect("parses");
        assert_eq!(parsed, Request::SessionBind(good.clone()));
        match good.check() {
            BindCheck::Verified(fp) => assert_eq!(fp, host.fingerprint()),
            other => panic!("expected verified, got {other:?}"),
        }
        let mut bad = good.clone();
        bad.session_id = b"another-session".to_vec();
        assert_eq!(bad.check(), BindCheck::Bad, "a signature over another session id fails");
        let mut junk = good.clone();
        junk.signature = b"junk".to_vec();
        assert_eq!(junk.check(), BindCheck::Bad);
        // A host key that doesn't parse (a host certificate, an unknown
        // type) can't be verified: unverifiable, never a close.
        let mut nokey = good.clone();
        nokey.host_key = b"junk".to_vec();
        assert!(matches!(nokey.check(), BindCheck::Unverifiable(ref fp) if fp.starts_with("SHA256:")), "{:?}", nokey.check());
        let mut cert = Vec::new();
        put_string(&mut cert, b"ssh-ed25519-cert-v01@openssh.com");
        put_string(&mut cert, &[7u8; 32]);
        put_string(&mut cert, &host.public_blob());
        let certb = SessionBind { host_key: cert, ..good.clone() };
        assert!(matches!(certb.check(), BindCheck::Unverifiable(_)), "a host-cert bind: {:?}", certb.check());
        // A P-256 host key verifies.
        let p256 = PrivateKey::from_openssh(testkeys::p256()).expect("p256");
        let s: ssh_key::Signature = Signer::try_sign(&p256, &sid).expect("p256 sign");
        let mut sigb = Vec::new();
        put_string(&mut sigb, s.algorithm().as_str().as_bytes());
        put_string(&mut sigb, s.as_bytes());
        let pb = SessionBind { host_key: p256.public_key().to_bytes().expect("blob"), session_id: sid.clone(), signature: sigb.clone(), forwarding: false };
        assert!(matches!(pb.check(), BindCheck::Verified(_)), "a P-256 bind: {:?}", pb.check());
        let pbad = SessionBind { session_id: b"other".to_vec(), ..pb };
        assert_eq!(pbad.check(), BindCheck::Bad);
        // An RSA host key signs with rsa-sha2-256.
        let rh = AgentKey::parse(testkeys::rsa2048().as_bytes()).expect("rsa host");
        let rb = SessionBind { host_key: rh.public_blob(), session_id: sid.clone(), signature: client::host_sign(&rh, &sid), forwarding: false };
        assert!(matches!(rb.check(), BindCheck::Verified(_)), "{:?}", rb.check());
        // is_forwarding other than 0/1 is malformed.
        let mut raw = client::session_bind(&SessionBind { host_key: vec![1], session_id: vec![2], signature: vec![3], forwarding: false });
        *raw.last_mut().expect("byte") = 2;
        assert_eq!(parse_request(&raw), Err(Malformed));
    }
}
