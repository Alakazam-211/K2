//! Compute secret-env (protocol 4, prd-compute-secret-env-v1 §6): how a
//! vault secret travels from the controller to the node that runs the job.
//!
//! The plan carries env **names** only (`Plan::secret_env`). Right before
//! spawn the node makes a fresh X25519 key pair and sends `secret_need`
//! with the public half; the controller seals `{"env":{NAME:value}}` to it
//! and answers `secret_env`:
//!
//! - `shared = X25519(e_c, e_n)`, both halves fresh and single-use;
//! - `info = signed_bytes("k2-compute-secret-v1", [controller_fp, node_fp,
//!   job_id, generation, attempt, plan_digest, epk_n, epk_c])`;
//! - `key = HKDF-SHA256(salt = "k2-compute-secret-v1", ikm = shared, info)`;
//! - `ct = AES-256-GCM(key, nonce = 0^12, aad = info, payload)`.
//!
//! A zero nonce is safe because every key is used exactly once (both
//! private halves are fresh; aws-lc-rs consumes the node's on agreement and
//! the controller's is dropped after one seal). The signed envelope
//! authenticates both public halves, so the relay (TLS ends there with E2E
//! off) can neither read the value nor swap a key. A captured `secret_env`
//! can't be opened later: the node's private half is gone.

use std::collections::BTreeMap;

use aws_lc_rs::{aead, agreement, hkdf, rand};

use crate::crypto;

/// Domain, HKDF salt and AAD prefix.
pub const DOMAIN: &str = "k2-compute-secret-v1";
/// At most this many secrets per job (SE3).
pub const MAX_SECRETS: usize = 16;
/// Largest value (text, no NUL) (SE3).
pub const MAX_VALUE_BYTES: usize = 8 * 1024;
/// Largest sealed payload (`ct`, raw bytes) (SE3).
pub const MAX_SEALED_BYTES: usize = 48 * 1024;
/// How long the node waits for the controller's answer (SE18).
pub const NEED_TIMEOUT_SECS: u64 = 60;
/// What the node writes on the job's stdin pipe (Linux wall, SE21): this
/// line, then the canonical payload JSON.
pub const PIPE_MAGIC: &[u8] = b"K2SE1\n";
/// Most the `job-exec` shim reads from stdin.
pub const MAX_PIPE_BYTES: usize = 64 * 1024;
/// The offer feature (owner and `trust own` sessions, protocol ≥ 4).
pub const FEATURE_SECRET_ENV: &str = "secret-env";

/// Refusal and reason codes (SE11, SE17, SE20, SE24, SE26).
pub mod codes {
    pub const NOT_GRANTED: &str = "secret_not_granted";
    pub const NOT_ACTIVE: &str = "secret_not_active";
    pub const ATTEMPT_STALE: &str = "secret_attempt_stale";
    pub const ALREADY_RUNNING: &str = "secret_already_running";
    pub const TOO_LARGE: &str = "secret_too_large";
    pub const VAULT_LOCKED: &str = "secret_vault_locked";
    /// Link dropped or the 60 s wait ran out (infrastructure).
    pub const UNAVAILABLE: &str = "secret_unavailable";
    pub const PAYLOAD_MISMATCH: &str = "secret_payload_mismatch";
    pub const NEEDS_WALL: &str = "secret_needs_wall";
    pub const UNTRUSTED_SERVER: &str = "secret_untrusted_server";
    pub const NEEDS_UPDATE: &str = "secret_needs_update";
    pub const LINUX_ONLY: &str = "secret_env_linux_only";
    /// A job of another workspace runs under the same uid (SE26): wait.
    pub const ISOLATION_WAIT: &str = "secret_isolation_wait";
    /// The controller belt redacted what the node should have (SE24).
    pub const REDACTION_MISS: &str = "secret_redaction_miss";
    /// The node doesn't offer `secret-env` right now (placement).
    pub const FEATURE_MISSING: &str = "secret_env_unavailable";
}

/// Why a secret env name is refused on the node (SE20): the shared job
/// env rule ([`crate::env::env_name_refusal`]) plus the loader names a
/// secret must never set. The controller also applies the vault's longer
/// deny list.
pub fn name_refusal(name: &str) -> Option<&'static str> {
    if let Some(c) = crate::env::env_name_refusal(name) {
        return Some(c);
    }
    let up = name.to_ascii_uppercase();
    if up.starts_with("LD_") || up.starts_with("DYLD_") || matches!(up.as_str(), "PATH" | "HOME" | "SHELL" | "IFS" | "BASH_ENV" | "ENV") {
        return Some("env_loader_reserved");
    }
    None
}

/// What a seal binds (besides both public halves).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub controller_fp: String,
    pub node_fp: String,
    pub job_id: String,
    pub generation: u32,
    pub attempt: u32,
    pub plan_digest: String,
}

impl Binding {
    fn info(&self, epk_n: &[u8], epk_c: &[u8]) -> Vec<u8> {
        let generation = self.generation.to_string();
        let attempt = self.attempt.to_string();
        crypto::signed_bytes(
            DOMAIN,
            &[
                self.controller_fp.as_bytes(),
                self.node_fp.as_bytes(),
                self.job_id.as_bytes(),
                generation.as_bytes(),
                attempt.as_bytes(),
                self.plan_digest.as_bytes(),
                epk_n,
                epk_c,
            ],
        )
    }
}

/// Overwrite `buf` with zeros (best effort: copies made elsewhere stay).
pub fn wipe(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: a valid, aligned, exclusive reference.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// Opened secret env: `(NAME, value)` in name order. Its strings are
/// zeroed on drop; `Debug` never prints a value.
#[derive(Default)]
pub struct Plain(pub Vec<(String, String)>);

impl Plain {
    pub fn names(&self) -> Vec<String> {
        self.0.iter().map(|(k, _)| k.clone()).collect()
    }
}

impl std::fmt::Debug for Plain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Plain({:?})", self.names())
    }
}

impl Drop for Plain {
    fn drop(&mut self) {
        for (_, v) in &mut self.0 {
            // SAFETY: zero bytes keep the String valid UTF-8.
            wipe(unsafe { v.as_bytes_mut() });
        }
    }
}

fn key_for(shared: &[u8], info: &[u8]) -> Result<aead::LessSafeKey, ()> {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, DOMAIN.as_bytes()).extract(shared);
    let mut k = [0u8; 32];
    prk.expand(&[info], hkdf::HKDF_SHA256).map_err(|_| ())?.fill(&mut k).map_err(|_| ())?;
    let key = aead::UnboundKey::new(&aead::AES_256_GCM, &k).map_err(|_| ());
    wipe(&mut k);
    Ok(aead::LessSafeKey::new(key?))
}

/// The canonical payload `{"env":{NAME:value}}` after the caps (SE3).
/// `Err(code)`: [`codes::TOO_LARGE`].
pub fn payload(env: &[(String, Vec<u8>)]) -> Result<Vec<u8>, &'static str> {
    if env.is_empty() || env.len() > MAX_SECRETS {
        return Err(codes::TOO_LARGE);
    }
    let mut m = BTreeMap::new();
    for (k, v) in env {
        if v.len() > MAX_VALUE_BYTES || v.contains(&0) {
            return Err(codes::TOO_LARGE);
        }
        let s = std::str::from_utf8(v).map_err(|_| codes::TOO_LARGE)?;
        m.insert(k.as_str(), s);
    }
    // Straight to bytes (no `serde_json::Value` copies of the values), into
    // a buffer sized for the cap so it never reallocates under the cap. A
    // one-field struct of a sorted map is already canonical JSON.
    #[derive(serde::Serialize)]
    struct P<'a> {
        env: BTreeMap<&'a str, &'a str>,
    }
    let mut out = Vec::with_capacity(MAX_SEALED_BYTES);
    let w = serde_json::to_writer(&mut out, &P { env: m });
    if w.is_err() || out.len() + 16 > MAX_SEALED_BYTES {
        wipe(&mut out);
        return Err(codes::TOO_LARGE);
    }
    Ok(out)
}

/// Parse an opened payload (the node, and the `job-exec` shim's pipe).
pub fn parse_payload(bytes: &[u8]) -> Result<Plain, String> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        env: BTreeMap<String, String>,
    }
    let p: P = serde_json::from_slice(bytes).map_err(|_| "the secret payload doesn't parse".to_string())?;
    if p.env.is_empty() || p.env.len() > MAX_SECRETS {
        return Err("the secret payload has no or too many entries".into());
    }
    let plain = Plain(p.env.into_iter().collect());
    for (k, v) in &plain.0 {
        if let Some(c) = name_refusal(k) {
            return Err(format!("secret env name {k}: {c}"));
        }
        if v.len() > MAX_VALUE_BYTES || v.contains('\0') {
            return Err(format!("secret env {k}: value too long or not text"));
        }
    }
    Ok(plain)
}

/// The controller's side: seal `env` to the node's `epk_n` (base64).
/// Returns `(epk_c, ct)`, both base64. `env` values are wiped by the caller.
pub fn seal(env: &[(String, Vec<u8>)], epk_n_b64: &str, b: &Binding) -> Result<(String, String), &'static str> {
    let epk_n = crypto::unb64(epk_n_b64).map_err(|_| codes::ATTEMPT_STALE)?;
    if epk_n.len() != 32 {
        return Err(codes::ATTEMPT_STALE);
    }
    let mut body = payload(env)?;
    let rng = rand::SystemRandom::new();
    let r = (|| -> Result<(String, String), ()> {
        let e_c = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng).map_err(|_| ())?;
        let epk_c = e_c.compute_public_key().map_err(|_| ())?.as_ref().to_vec();
        let info = b.info(&epk_n, &epk_c);
        let key = agreement::agree_ephemeral(e_c, &agreement::UnparsedPublicKey::new(&agreement::X25519, &epk_n), (), |shared| key_for(shared, &info))?;
        let mut in_out = body.clone();
        key.seal_in_place_append_tag(aead::Nonce::assume_unique_for_key([0u8; 12]), aead::Aad::from(&info), &mut in_out).map_err(|_| ())?;
        let ct = crypto::b64(&in_out);
        Ok((crypto::b64(&epk_c), ct))
    })();
    wipe(&mut body);
    r.map_err(|_| codes::ATTEMPT_STALE)
}

/// The node's one-time key for one `secret_need`.
pub struct NodeKey {
    key: agreement::EphemeralPrivateKey,
    public: Vec<u8>,
}

impl std::fmt::Debug for NodeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NodeKey({})", crypto::b64(&self.public))
    }
}

impl NodeKey {
    pub fn generate() -> Result<Self, String> {
        let rng = rand::SystemRandom::new();
        let key = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng).map_err(|_| "X25519 key generation failed".to_string())?;
        let public = key.compute_public_key().map_err(|_| "X25519 public key failed".to_string())?.as_ref().to_vec();
        Ok(Self { key, public })
    }

    /// `epk` for the `secret_need` frame.
    pub fn public_b64(&self) -> String {
        crypto::b64(&self.public)
    }

    /// Open the controller's answer. Consumes the key (single use).
    pub fn open(self, epk_c_b64: &str, ct_b64: &str, b: &Binding) -> Result<Plain, String> {
        let bad = || "the sealed secret env doesn't open (wrong key or binding, or tampered)".to_string();
        let epk_c = crypto::unb64(epk_c_b64).map_err(|_| bad())?;
        let mut ct = crypto::unb64(ct_b64).map_err(|_| bad())?;
        if epk_c.len() != 32 || ct.len() < 17 || ct.len() > MAX_SEALED_BYTES {
            return Err(bad());
        }
        let info = b.info(&self.public, &epk_c);
        let key = agreement::agree_ephemeral(self.key, &agreement::UnparsedPublicKey::new(&agreement::X25519, &epk_c), (), |shared| key_for(shared, &info)).map_err(|_| bad())?;
        let r = match key.open_in_place(aead::Nonce::assume_unique_for_key([0u8; 12]), aead::Aad::from(&info), &mut ct) {
            Ok(plain) => parse_payload(plain),
            Err(_) => Err(bad()),
        };
        wipe(&mut ct);
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: &str = "k2test-secret-SEAL-0123456789abcdef";
    const V2: &str = "k2test-secret-SEAL-second-value";

    fn bind() -> Binding {
        Binding {
            controller_fp: "c".repeat(64),
            node_fp: "n".repeat(64),
            job_id: "j1".into(),
            generation: 2,
            attempt: 1,
            plan_digest: "sha256:abc".into(),
        }
    }

    fn env() -> Vec<(String, Vec<u8>)> {
        vec![("B_TOKEN".into(), V2.as_bytes().to_vec()), ("A_KEY".into(), V1.as_bytes().to_vec())]
    }

    #[test]
    fn seal_open_round_trip() {
        let n = NodeKey::generate().unwrap();
        let (epk_c, ct) = seal(&env(), &n.public_b64(), &bind()).unwrap();
        assert!(!ct.contains(V1) && !crypto::unb64(&ct).unwrap().windows(V1.len()).any(|w| w == V1.as_bytes()), "ct carries the value");
        let p = n.open(&epk_c, &ct, &bind()).unwrap();
        assert_eq!(p.0, vec![("A_KEY".to_string(), V1.to_string()), ("B_TOKEN".to_string(), V2.to_string())]);
        assert_eq!(format!("{p:?}"), "Plain([\"A_KEY\", \"B_TOKEN\"])", "Debug names only");
    }

    #[test]
    fn every_binding_field_and_any_tamper_fails() {
        let mutations: Vec<Box<dyn Fn(&mut Binding)>> = vec![
            Box::new(|b| b.controller_fp.push('x')),
            Box::new(|b| b.node_fp.push('x')),
            Box::new(|b| b.job_id = "j2".into()),
            Box::new(|b| b.generation += 1),
            Box::new(|b| b.attempt += 1),
            Box::new(|b| b.plan_digest.push('x')),
        ];
        for (i, m) in mutations.iter().enumerate() {
            let n = NodeKey::generate().unwrap();
            let (epk_c, ct) = seal(&env(), &n.public_b64(), &bind()).unwrap();
            let mut other = bind();
            m(&mut other);
            assert!(n.open(&epk_c, &ct, &other).is_err(), "mutation {i} still opened");
        }
        // Tampered ciphertext, a swapped controller key, another node key.
        let n = NodeKey::generate().unwrap();
        let (epk_c, ct) = seal(&env(), &n.public_b64(), &bind()).unwrap();
        let mut raw = crypto::unb64(&ct).unwrap();
        raw[3] ^= 1;
        assert!(n.open(&epk_c, &crypto::b64(&raw), &bind()).is_err());
        let n = NodeKey::generate().unwrap();
        let (_, ct) = seal(&env(), &n.public_b64(), &bind()).unwrap();
        let swapped = NodeKey::generate().unwrap().public_b64();
        assert!(n.open(&swapped, &ct, &bind()).is_err());
        let n = NodeKey::generate().unwrap();
        let (epk_c, ct) = seal(&env(), &n.public_b64(), &bind()).unwrap();
        assert!(NodeKey::generate().unwrap().open(&epk_c, &ct, &bind()).is_err());
        assert!(n.open(&epk_c, &ct, &bind()).is_ok());
    }

    #[test]
    fn caps_and_names() {
        let n = NodeKey::generate().unwrap();
        assert_eq!(seal(&[], &n.public_b64(), &bind()), Err(codes::TOO_LARGE), "zero secrets");
        let many: Vec<_> = (0..17).map(|i| (format!("S{i}"), V1.as_bytes().to_vec())).collect();
        assert_eq!(seal(&many, &n.public_b64(), &bind()), Err(codes::TOO_LARGE));
        let big = vec![("BIG".to_string(), vec![b'a'; MAX_VALUE_BYTES + 1])];
        assert_eq!(seal(&big, &n.public_b64(), &bind()), Err(codes::TOO_LARGE));
        let nul = vec![("NUL".to_string(), b"a\0b".to_vec())];
        assert_eq!(seal(&nul, &n.public_b64(), &bind()), Err(codes::TOO_LARGE));
        // 8 × 8 KiB fits the per-value cap but not the 48 KiB seal.
        let wide: Vec<_> = (0..8).map(|i| (format!("W{i}"), vec![b'a'; MAX_VALUE_BYTES])).collect();
        assert_eq!(seal(&wide, &n.public_b64(), &bind()), Err(codes::TOO_LARGE));
        let five: Vec<_> = (0..5).map(|i| (format!("W{i}"), vec![b'a'; MAX_VALUE_BYTES])).collect();
        assert!(seal(&five, &n.public_b64(), &bind()).is_ok());
        assert!(seal(&env(), "short", &bind()).is_err());
        for bad in ["K2_TOKEN", "PATH", "LD_PRELOAD", "DYLD_X", "HOME", "1X", "a-b"] {
            assert!(name_refusal(bad).is_some(), "{bad}");
        }
        assert!(name_refusal("BUILD_GMAIL_CLIENT_SECRET").is_none());
        assert!(parse_payload(br#"{"env":{"K2_TOKEN":"x"}}"#).is_err());
        assert!(parse_payload(br#"{"env":{}}"#).is_err());
        assert!(parse_payload(br#"{"env":{"A":"x"},"more":1}"#).is_err());
    }
}
