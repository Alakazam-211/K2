//! Keys a relying party trusts, and how keys are named.
//!
//! - A **key id** is the lowercase hex SHA-256 of the key's DER
//!   SubjectPublicKeyInfo (what `openssl pkey -pubin -outform DER` prints).
//! - Its **fingerprint words** are six words from `fixtures/sa1/wordlist.txt`
//!   (256 words): one per byte, for the key id's first six bytes. A person
//!   compares them between the device's screen and the relying party's
//!   printout when the key is pinned.
//! - A **key set** is a folder: `approvers.tsv` (name, person, check, view
//!   per line, tab-separated, `#` comments) and `<name>.pem` for each name.
//!   The check and view of an approval are read from here, never from the
//!   record (SA-5).

use crate::{Alg, Check, View};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::path::Path;

/// The 256 fingerprint words, one per byte value.
pub const WORDLIST: &str = include_str!("../fixtures/sa1/wordlist.txt");

/// The DER header of a P-256 SubjectPublicKeyInfo (uncompressed point follows).
pub const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42,
    0x00,
];
const RSA_OID: [u8; 11] = [0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];

pub fn key_id(spki_der: &[u8]) -> String {
    Sha256::digest(spki_der).iter().map(|b| format!("{b:02x}")).collect()
}

/// Six fingerprint words for a key id (64 lowercase hex).
pub fn words(key_id: &str) -> Option<String> {
    if !crate::payload::is_hex_lower(key_id, 64) {
        return None;
    }
    let list: Vec<&str> = WORDLIST.lines().collect();
    assert_eq!(list.len(), 256, "the wordlist has 256 words");
    let mut out = Vec::new();
    for i in 0..6 {
        let b = u8::from_str_radix(&key_id[i * 2..i * 2 + 2], 16).ok()?;
        out.push(list[b as usize]);
    }
    Some(out.join(" "))
}

/// The DER inside one `PUBLIC KEY` PEM block.
pub fn pem_to_der(pem: &str) -> Result<Vec<u8>, String> {
    let begin = "-----BEGIN PUBLIC KEY-----";
    let end = "-----END PUBLIC KEY-----";
    let a = pem.find(begin).ok_or("no BEGIN PUBLIC KEY line")?;
    let rest = &pem[a + begin.len()..];
    let z = rest.find(end).ok_or("no END PUBLIC KEY line")?;
    if rest[z + end.len()..].contains("-----BEGIN") {
        return Err("more than one PEM block".into());
    }
    let b64: String = rest[..z].chars().filter(|c| !c.is_ascii_whitespace()).collect();
    base64::engine::general_purpose::STANDARD.decode(b64).map_err(|_| "the PEM body is not base64".to_string())
}

/// A public key in the form the verifier takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey {
    pub alg: Alg,
    pub spki: Vec<u8>,
    /// ES256: the 65-byte uncompressed point. RS256: the PKCS#1 RSAPublicKey DER.
    pub raw: Vec<u8>,
}

fn der_len(b: &[u8], i: &mut usize) -> Option<usize> {
    let first = *b.get(*i)?;
    *i += 1;
    if first < 0x80 {
        return Some(first as usize);
    }
    let n = (first & 0x7f) as usize;
    if n == 0 || n > 3 {
        return None;
    }
    let mut len = 0usize;
    for _ in 0..n {
        len = (len << 8) | *b.get(*i)? as usize;
        *i += 1;
    }
    Some(len)
}

/// (tag, contents) of the DER element at `i`; moves `i` past it.
fn der_elem<'a>(b: &'a [u8], i: &mut usize) -> Option<(u8, &'a [u8])> {
    let tag = *b.get(*i)?;
    *i += 1;
    let len = der_len(b, i)?;
    let body = b.get(*i..*i + len)?;
    *i += len;
    Some((tag, body))
}

/// Read a SubjectPublicKeyInfo: P-256 (ES256) or RSA 2048–8192 (RS256).
pub fn parse_spki(der: &[u8]) -> Result<PublicKey, String> {
    if der.len() == 91 && der[..26] == P256_SPKI_PREFIX && der[26] == 0x04 {
        return Ok(PublicKey { alg: Alg::Es256, spki: der.to_vec(), raw: der[26..].to_vec() });
    }
    let mut i = 0;
    let (t, outer) = der_elem(der, &mut i).ok_or("not DER")?;
    if t != 0x30 || i != der.len() {
        return Err("not a SubjectPublicKeyInfo".into());
    }
    let mut j = 0;
    let (t, alg) = der_elem(outer, &mut j).ok_or("not DER")?;
    if t != 0x30 || !alg.starts_with(&RSA_OID) || (alg.len() != RSA_OID.len() && alg[RSA_OID.len()..] != [0x05, 0x00]) {
        return Err("not a P-256 or RSA public key".into());
    }
    let (t, bits) = der_elem(outer, &mut j).ok_or("not DER")?;
    if t != 0x03 || j != outer.len() || bits.first() != Some(&0) {
        return Err("bad RSA key bits".into());
    }
    let pkcs1 = &bits[1..];
    // RSAPublicKey ::= SEQUENCE { modulus INTEGER, publicExponent INTEGER }
    let mut k = 0;
    let (t, seq) = der_elem(pkcs1, &mut k).ok_or("bad RSA key")?;
    if t != 0x30 || k != pkcs1.len() {
        return Err("bad RSA key".into());
    }
    let mut m = 0;
    let (t, modulus) = der_elem(seq, &mut m).ok_or("bad RSA key")?;
    if t != 0x02 {
        return Err("bad RSA modulus".into());
    }
    let bits_len = modulus.iter().skip_while(|b| **b == 0).count() * 8;
    if !(2048..=8192).contains(&bits_len) {
        return Err(format!("an RSA key must be 2048 to 8192 bits, not about {bits_len}"));
    }
    Ok(PublicKey { alg: Alg::Rs256, spki: der.to_vec(), raw: pkcs1.to_vec() })
}

/// One key the relying party trusts, with what it pinned about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedKey {
    pub name: String,
    pub person: String,
    pub check: Check,
    pub view: View,
    pub key: PublicKey,
    pub key_id: String,
}

/// A key name in a key set: ^[a-z0-9][a-z0-9._-]{0,62}$ (the file is `<name>.pem`).
pub fn name_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 63 && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-'))
}

/// Parse `approvers.tsv` and its PEM files from a folder.
pub fn load_dir(dir: &Path) -> Result<Vec<TrustedKey>, String> {
    let tsv = dir.join("approvers.tsv");
    let text = std::fs::read_to_string(&tsv).map_err(|e| format!("{}: {e}", tsv.display()))?;
    let mut out: Vec<TrustedKey> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() != 4 {
            return Err(format!("approvers.tsv line {}: want name, person, check, view (tab-separated)", n + 1));
        }
        let (name, person) = (cols[0], cols[1]);
        if !name_ok(name) {
            return Err(format!("approvers.tsv line {}: bad name {name:?}", n + 1));
        }
        if person.is_empty() || person.len() > crate::payload::MAX_NAME_BYTES || person.chars().any(|c| crate::json::forbidden(c as u32)) {
            return Err(format!("approvers.tsv line {}: bad person {person:?}", n + 1));
        }
        let check = Check::parse(cols[2]).ok_or_else(|| format!("approvers.tsv line {}: check must be biometric or credential", n + 1))?;
        let view = View::parse(cols[3]).ok_or_else(|| format!("approvers.tsv line {}: view must be device, host or page", n + 1))?;
        let pem_path = dir.join(format!("{name}.pem"));
        let pem = std::fs::read_to_string(&pem_path).map_err(|e| format!("{}: {e}", pem_path.display()))?;
        let key = parse_spki(&pem_to_der(&pem).map_err(|e| format!("{}: {e}", pem_path.display()))?).map_err(|e| format!("{}: {e}", pem_path.display()))?;
        let key_id = key_id(&key.spki);
        if out.iter().any(|k| k.name == name || k.key_id == key_id) {
            return Err(format!("approvers.tsv line {}: {name} (or its key) is listed twice", n + 1));
        }
        out.push(TrustedKey { name: name.into(), person: person.into(), check, view, key, key_id });
    }
    Ok(out)
}
