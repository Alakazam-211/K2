//! minisign in Rust: the format Tauri's updater and K2's U1/U2 keys use
//! (PRD §4.5: "minisign-format signing in Rust"; vs-live V28: "minisign via
//! a pure-Rust crate").
//!
//! - **Secret key file** (`minisign -G`, or Tauri's `.key`, which is the same
//!   text base64-encoded once more): `untrusted comment: …` then base64 of
//!   `"Ed" "Sc"|"\0\0" "B2" salt[32] opslimit[u64le] memlimit[u64le]
//!   enc(keynum[8] sk[64] chk[32])`. The 104-byte tail is XORed with
//!   scrypt(password, salt) using libsodium's parameter choice; `chk` is
//!   BLAKE2b-256(`"Ed"` ‖ keynum ‖ sk).
//! - **Signature** (prehashed, `ED`): line 2 is base64(`"ED"` ‖ keynum ‖
//!   Ed25519(sk, BLAKE2b-512(file))); line 4 is base64(Ed25519(sk, that
//!   64-byte signature ‖ trusted comment)).
//!
//! Signing is Ed25519 through `aws-lc-rs` (deterministic, so a signature
//! here is byte-identical to the minisign binary's for the same comments:
//! a test pins that against a fixture the real minisign 0.12 made). Every
//! signature is checked again with the independent `minisign-verify` crate
//! before it leaves the signer.

use std::io::Read;

use blake2::digest::consts::U32;
use blake2::{Blake2b, Blake2b512, Digest};
use k2_node_proto::admin::ApprovalSigner;
use k2_node_proto::crypto::{b64, unb64};
use zeroize::{Zeroize, Zeroizing};

type Blake2b256 = Blake2b<U32>;

const SIG_ALG: &[u8; 2] = b"Ed";
const KDF_SCRYPT: &[u8; 2] = b"Sc";
const KDF_NONE: &[u8; 2] = b"\0\0";
const CHK_ALG: &[u8; 2] = b"B2";
const SK_LEN: usize = 2 + 2 + 2 + 32 + 8 + 8 + 104;

/// minisign's defaults for a new key (`crypto_pwhash_scryptsalsa208sha256`
/// "sensitive": N = 2^20, r = 8, p = 1, 1 GiB).
pub const OPSLIMIT_SENSITIVE: u64 = 33_554_432;
pub const MEMLIMIT_SENSITIVE: u64 = 1_073_741_824;

/// A minisign public key (`RW…` line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey {
    pub keynum: [u8; 8],
    pub pk: [u8; 32],
}

impl PublicKey {
    /// From the base64 line (`RW…`), or a whole `.pub` file.
    pub fn parse(text: &str) -> Result<Self, String> {
        let line = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))
            .ok_or("no minisign public key line")?;
        let raw = unb64(line)?;
        if raw.len() != 42 || &raw[..2] != SIG_ALG {
            return Err("not a minisign public key line".into());
        }
        let mut keynum = [0u8; 8];
        keynum.copy_from_slice(&raw[2..10]);
        let mut pk = [0u8; 32];
        pk.copy_from_slice(&raw[10..]);
        Ok(Self { keynum, pk })
    }

    pub fn line(&self) -> String {
        let mut raw = SIG_ALG.to_vec();
        raw.extend_from_slice(&self.keynum);
        raw.extend_from_slice(&self.pk);
        b64(&raw)
    }

    /// The key id minisign prints (`7835E51A2106C44F`): keynum reversed, hex.
    pub fn key_id(&self) -> String {
        key_id_of(&self.keynum)
    }
}

fn key_id_of(keynum: &[u8; 8]) -> String {
    let mut k = *keynum;
    k.reverse();
    k2_node_proto::crypto::hex(&k).to_ascii_uppercase()
}

/// An unlocked minisign secret key.
pub struct SecretKey {
    keynum: [u8; 8],
    signer: ApprovalSigner,
    pk: [u8; 32],
}

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretKey({})", key_id_of(&self.keynum))
    }
}

/// libsodium `pickparams` (crypto_pwhash/scryptsalsa208sha256): (log2 N, r, p).
pub fn scrypt_params(opslimit: u64, memlimit: u64) -> (u8, u32, u32) {
    let opslimit = opslimit.max(32_768);
    let r: u64 = 8;
    let mut n_log2: u32 = 1;
    if opslimit < memlimit / 32 {
        let max_n = opslimit / (r * 4);
        while n_log2 < 63 {
            if (1u64 << n_log2) > max_n / 2 {
                break;
            }
            n_log2 += 1;
        }
        (n_log2 as u8, r as u32, 1)
    } else {
        let max_n = memlimit / (r * 128);
        while n_log2 < 63 {
            if (1u64 << n_log2) > max_n / 2 {
                break;
            }
            n_log2 += 1;
        }
        let max_rp = ((opslimit / 4) / (1u64 << n_log2)).min(0x3fff_ffff);
        (n_log2 as u8, r as u32, (max_rp / r) as u32)
    }
}

fn kdf_stream(password: &[u8], salt: &[u8], opslimit: u64, memlimit: u64) -> Result<Zeroizing<[u8; 104]>, String> {
    let (log_n, r, p) = scrypt_params(opslimit, memlimit);
    if log_n > 22 || p == 0 || p > 64 {
        // 2^22 × 8 × 128 = 4 GiB: a key file asking for more is refused, not run.
        return Err(format!("the key asks for scrypt N=2^{log_n} p={p}: refused"));
    }
    // `len` is only the PHC string's length (10–64); the output is 104 bytes.
    let params = scrypt::Params::new(log_n, r, p, 32).map_err(|e| format!("scrypt params: {e}"))?;
    let mut out = Zeroizing::new([0u8; 104]);
    scrypt::scrypt(password, salt, &params, &mut *out).map_err(|e| format!("scrypt: {e}"))?;
    Ok(out)
}

fn checksum(keynum: &[u8; 8], sk: &[u8]) -> [u8; 32] {
    let mut h = Blake2b256::new();
    h.update(SIG_ALG);
    h.update(keynum);
    h.update(sk);
    let mut out = [0u8; 32];
    out.copy_from_slice(&h.finalize());
    out
}

/// k2sign's `decode_secret_key`: a minisign key file, or Tauri's base64 of one.
pub fn unwrap_key_file(raw: &[u8]) -> Result<Zeroizing<String>, String> {
    let text = Zeroizing::new(if raw.starts_with(b"untrusted comment:") {
        raw.to_vec()
    } else {
        let squeezed = Zeroizing::new(String::from_utf8_lossy(raw).split_whitespace().collect::<String>());
        unb64(&squeezed).map_err(|_| "not a minisign or Tauri secret key".to_string())?
    });
    if !text.starts_with(b"untrusted comment:") {
        return Err("not a minisign or Tauri secret key".into());
    }
    Ok(Zeroizing::new(String::from_utf8(text.to_vec()).map_err(|_| "the secret key file is not text".to_string())?))
}

/// Whether a key file is password-encrypted (scrypt), not `-W` plain (L9).
pub fn is_encrypted(file: &[u8]) -> Result<bool, String> {
    let text = unwrap_key_file(file)?;
    let line = text.lines().nth(1).ok_or("the secret key file has no key line")?;
    let raw = Zeroizing::new(unb64(line.trim())?);
    if raw.len() != SK_LEN {
        return Err("not a minisign Ed25519 secret key".into());
    }
    Ok(&raw[2..4] == KDF_SCRYPT)
}

impl SecretKey {
    /// Decrypt a key file with its password (empty for an unencrypted key).
    pub fn open(file: &[u8], password: &[u8]) -> Result<Self, String> {
        let text = unwrap_key_file(file)?;
        let line = text.lines().nth(1).ok_or("the secret key file has no key line")?;
        let raw = Zeroizing::new(unb64(line.trim())?);
        if raw.len() != SK_LEN || &raw[..2] != SIG_ALG || &raw[4..6] != CHK_ALG {
            return Err("not a minisign Ed25519 secret key".into());
        }
        let salt = &raw[6..38];
        let ops = u64::from_le_bytes(raw[38..46].try_into().map_err(|_| "opslimit")?);
        let mem = u64::from_le_bytes(raw[46..54].try_into().map_err(|_| "memlimit")?);
        let mut keynum_sk = Zeroizing::new([0u8; 104]);
        keynum_sk.copy_from_slice(&raw[54..]);
        match &raw[2..4] {
            k if k == KDF_SCRYPT => {
                let stream = kdf_stream(password, salt, ops, mem)?;
                for (b, s) in keynum_sk.iter_mut().zip(stream.iter()) {
                    *b ^= s;
                }
            }
            k if k == KDF_NONE => {
                if !password.is_empty() {
                    return Err("the key is not encrypted, but a password was given".into());
                }
            }
            _ => return Err("unknown minisign key derivation".into()),
        }
        let mut keynum = [0u8; 8];
        keynum.copy_from_slice(&keynum_sk[..8]);
        let sk = &keynum_sk[8..72];
        let chk = &keynum_sk[72..104];
        if !k2_node_proto::crypto::ct_eq(&checksum(&keynum, sk), chk) {
            return Err("wrong password for the secret key (or a damaged key)".into());
        }
        let mut seed: [u8; 32] = sk[..32].try_into().map_err(|_| "seed")?;
        let mut pk = [0u8; 32];
        pk.copy_from_slice(&sk[32..64]);
        let signer = ApprovalSigner::from_seed(&seed);
        seed.zeroize();
        let signer = signer?;
        if signer.public().0 != pk {
            return Err("the secret key's public half does not match its seed".into());
        }
        Ok(Self { keynum, signer, pk })
    }

    /// A new key: `(key, key file text, public key)`, encrypted with
    /// `password` and the given scrypt limits.
    pub fn generate(password: &[u8], opslimit: u64, memlimit: u64) -> Result<(Self, String, PublicKey), String> {
        let rnd = Zeroizing::new(k2_node_proto::crypto::random_bytes(8 + 32 + 32)?);
        let mut keynum = [0u8; 8];
        keynum.copy_from_slice(&rnd[..8]);
        let mut seed: [u8; 32] = rnd[8..40].try_into().map_err(|_| "seed")?;
        let salt = &rnd[40..72];
        let signer = ApprovalSigner::from_seed(&seed)?;
        let pk = signer.public().0;
        let mut sk = Zeroizing::new(seed.to_vec());
        seed.zeroize();
        sk.extend_from_slice(&pk);
        let mut keynum_sk = Zeroizing::new(Vec::with_capacity(104));
        keynum_sk.extend_from_slice(&keynum);
        keynum_sk.extend_from_slice(&sk);
        keynum_sk.extend_from_slice(&checksum(&keynum, &sk));
        let stream = kdf_stream(password, salt, opslimit, memlimit)?;
        for (b, s) in keynum_sk.iter_mut().zip(stream.iter()) {
            *b ^= s;
        }
        let mut raw = Vec::with_capacity(SK_LEN);
        raw.extend_from_slice(SIG_ALG);
        raw.extend_from_slice(KDF_SCRYPT);
        raw.extend_from_slice(CHK_ALG);
        raw.extend_from_slice(salt);
        raw.extend_from_slice(&opslimit.to_le_bytes());
        raw.extend_from_slice(&memlimit.to_le_bytes());
        raw.extend_from_slice(&keynum_sk);
        let text = format!("untrusted comment: minisign encrypted secret key\n{}\n", b64(&raw));
        let public = PublicKey { keynum, pk };
        Ok((Self { keynum, signer, pk }, text, public))
    }

    pub fn public(&self) -> PublicKey {
        PublicKey { keynum: self.keynum, pk: self.pk }
    }

    /// Sign a BLAKE2b-512 prehash (from [`prehash`]) into a minisig text.
    pub fn sign_prehashed(&self, hash: &[u8; 64], untrusted: &str, trusted: &str) -> Result<String, String> {
        for (what, c) in [("untrusted", untrusted), ("trusted", trusted)] {
            if c.contains('\n') || c.contains('\r') {
                return Err(format!("the {what} comment has a line break"));
            }
        }
        let sig = self.signer.sign(hash);
        let mut line2 = b"ED".to_vec();
        line2.extend_from_slice(&self.keynum);
        line2.extend_from_slice(&sig);
        let mut global = sig.to_vec();
        global.extend_from_slice(trusted.as_bytes());
        let gsig = self.signer.sign(&global);
        Ok(format!(
            "untrusted comment: {untrusted}\n{}\ntrusted comment: {trusted}\n{}\n",
            b64(&line2),
            b64(&gsig)
        ))
    }

    pub fn sign(&self, data: &[u8], untrusted: &str, trusted: &str) -> Result<String, String> {
        self.sign_prehashed(&prehash(data), untrusted, trusted)
    }
}

/// BLAKE2b-512 of bytes (minisign's prehash).
pub fn prehash(data: &[u8]) -> [u8; 64] {
    let mut out = [0u8; 64];
    out.copy_from_slice(&Blake2b512::digest(data));
    out
}

/// BLAKE2b-512 of a reader, streamed (300 MB app tarballs).
pub fn prehash_reader(mut r: impl Read) -> std::io::Result<[u8; 64]> {
    let mut h = Blake2b512::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(&h.finalize());
    Ok(out)
}

/// `(algorithm, key id)` of a minisig text (k2sign's `sig_algo`).
pub fn sig_algo(text: &str) -> Result<(String, String), String> {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    let second = lines.next().unwrap_or_default();
    if !first.starts_with("untrusted comment:") || text.lines().count() < 4 {
        return Err("not a minisign signature".into());
    }
    let raw = unb64(second)?;
    if raw.len() != 74 {
        return Err("not a minisign signature line".into());
    }
    let mut keynum = [0u8; 8];
    keynum.copy_from_slice(&raw[2..10]);
    Ok((String::from_utf8_lossy(&raw[..2]).into_owned(), key_id_of(&keynum)))
}

/// Verify with the independent `minisign-verify` crate, streaming `r`;
/// returns the trusted comment.
pub fn verify_reader(public_line: &str, r: impl Read, sig_text: &str) -> Result<String, String> {
    let pk = minisign_verify::PublicKey::from_base64(public_line.trim()).map_err(|e| format!("public key: {e}"))?;
    let sig = minisign_verify::Signature::decode(sig_text).map_err(|e| format!("signature: {e}"))?;
    let mut v = pk.verify_stream(&sig).map_err(|e| format!("verify: {e}"))?;
    let mut r = r;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = r.read(&mut buf).map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            break;
        }
        v.update(&buf[..n]);
    }
    v.finalize().map_err(|e| format!("verify: {e}"))?;
    Ok(sig.trusted_comment().to_string())
}

pub fn verify(public_line: &str, data: &[u8], sig_text: &str) -> Result<String, String> {
    verify_reader(public_line, data, sig_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrypt_params_match_libsodium() {
        // minisign's defaults: N = 2^20, r = 8, p = 1.
        assert_eq!(scrypt_params(OPSLIMIT_SENSITIVE, MEMLIMIT_SENSITIVE), (20, 8, 1));
        // The interactive limits libsodium documents: N = 2^14, r = 8, p = 1.
        assert_eq!(scrypt_params(524_288, 16_777_216), (14, 8, 1));
        // opslimit below the floor is raised to 32768.
        assert_eq!(scrypt_params(1, 1 << 30), (10, 8, 1));
    }

    #[test]
    fn generate_sign_verify_round_trip_with_cheap_params() {
        let (k, file, public) = SecretKey::generate(b"pw", 32_768, 1 << 20).unwrap();
        let k2 = SecretKey::open(file.as_bytes(), b"pw").unwrap();
        assert_eq!(k2.public(), public);
        assert_eq!(k.public(), public);
        assert!(SecretKey::open(file.as_bytes(), b"wrong").unwrap_err().contains("wrong password"));
        // Tauri's form: the whole key file base64-encoded once more.
        let tauri = b64(file.as_bytes());
        assert_eq!(SecretKey::open(tauri.as_bytes(), b"pw").unwrap().public(), public);
        let sig = k.sign(b"hello", "signature from tauri secret key", "timestamp:1\tfile:x").unwrap();
        assert_eq!(verify(&public.line(), b"hello", &sig).unwrap(), "timestamp:1\tfile:x");
        assert!(verify(&public.line(), b"hellO", &sig).is_err());
        let (algo, id) = sig_algo(&sig).unwrap();
        assert_eq!((algo.as_str(), id), ("ED", public.key_id()));
        assert_eq!(PublicKey::parse(&public.line()).unwrap(), public);
        assert!(k.sign(b"x", "a", "b\nc").is_err());
    }
}
