//! Apple App Attest at enrolment (prd-signed-approvals-v1 §3-4, SA-7; S0).
//!
//! The phone's K2 app makes its two approver keys (Face ID, passcode) and an
//! App Attest key, and attests over the enrolment client data:
//!
//! `"K2-APPROVER-ENROL-1" 0x00 ‖ challenge (32 bytes from the relying party)
//!  ‖ SPKI(biometric key) ‖ SPKI(passcode key)`
//!
//! The relying party checks, once, at enrolment (the leaf certificate lasts
//! three days, so this is never re-checked later; the result is stored):
//! - the certificate chain to Apple's App Attestation Root CA (pinned), each
//!   certificate valid at that moment;
//! - the leaf's nonce extension (1.2.840.113635.100.8.2) = SHA-256(authData ‖
//!   SHA-256(clientData));
//! - SHA-256 of the leaf's public key = the App Attest key id = credentialId;
//! - authData: rpIdHash = SHA-256(<team id>.<bundle id>), signCount 0, and
//!   the environment from the AAGUID (development or production).
//!
//! It proves a genuine copy of the K2 app on a genuine Apple device made the
//! request; the keys' rules (Face ID only, passcode only) are then trusted
//! because that app's code sets them.

use crate::payload::is_hex_lower;
use crate::Environment;
use aws_lc_rs::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_ASN1, ECDSA_P384_SHA256_ASN1, ECDSA_P384_SHA384_ASN1};
use sha2::{Digest, Sha256};

/// The K2 phone app's App Attest identity: <team id>.<bundle id>.
pub const K2_PHONE_APP_ID: &str = "36B8R93HXV.com.alakazamlabs.k2so.companion";
/// Apple App Attestation Root CA (from apple.com/certificateauthority).
pub const APPLE_APP_ATTEST_ROOT_PEM: &str = include_str!("../fixtures/sa1/apple-app-attest-root.pem");
/// SHA-256 of the root certificate's DER: a test keeps the PEM equal to it.
pub const APPLE_APP_ATTEST_ROOT_SHA256: &str = "1cb9823ba28ba6ad2d33a006941de2ae4f513ef1d4e831b9f7e0fa7b6242c932";
/// The enrolment client data's prefix.
pub const ENROL_PREFIX: &[u8] = b"K2-APPROVER-ENROL-1\0";

const OID_NONCE: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x63, 0x64, 0x08, 0x02];
const OID_ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const OID_ECDSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
const OID_P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const OID_P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];

/// A checked attestation: what the relying party stores with the keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attested {
    pub environment: Environment,
    /// When it was checked (the relying party's clock, seconds since 1970).
    pub verified_at: i64,
    /// The leaf certificate's end (it is not checked again after enrolment).
    pub leaf_not_after: i64,
}

/// A checked enrolment: the two approver keys the attestation covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enrolment {
    pub attested: Attested,
    pub challenge: [u8; 32],
    pub biometric_spki: Vec<u8>,
    pub passcode_spki: Vec<u8>,
    pub biometric_key_id: String,
    pub passcode_key_id: String,
}

fn bad<T>(d: impl Into<String>) -> Result<T, String> {
    Err(d.into())
}

// ── CBOR (the attestation object: maps, text, bytes, arrays, small uints) ──

#[derive(Debug)]
enum Cbor {
    Uint(u64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Cbor>),
    Map(Vec<(Cbor, Cbor)>),
}

fn cbor(b: &[u8], i: &mut usize, depth: usize) -> Result<Cbor, String> {
    if depth > 8 {
        return bad("attestation: CBOR nested too deep");
    }
    let ib = *b.get(*i).ok_or("attestation: CBOR cut short")?;
    *i += 1;
    let (mt, ai) = (ib >> 5, ib & 31);
    let n: u64 = match ai {
        0..=23 => ai as u64,
        24..=27 => {
            let len = 1usize << (ai - 24);
            let raw = b.get(*i..*i + len).ok_or("attestation: CBOR cut short")?;
            *i += len;
            raw.iter().fold(0u64, |a, x| (a << 8) | *x as u64)
        }
        _ => return bad("attestation: indefinite or reserved CBOR lengths are not read"),
    };
    let take = |i: &mut usize, n: u64| -> Result<Vec<u8>, String> {
        let n = usize::try_from(n).map_err(|_| "attestation: CBOR length")?;
        let v = b.get(*i..i.checked_add(n).ok_or("attestation: CBOR length")?).ok_or("attestation: CBOR cut short")?.to_vec();
        *i += n;
        Ok(v)
    };
    match mt {
        0 => Ok(Cbor::Uint(n)),
        2 => Ok(Cbor::Bytes(take(i, n)?)),
        3 => Ok(Cbor::Text(String::from_utf8(take(i, n)?).map_err(|_| "attestation: CBOR text is not UTF-8")?)),
        4 => {
            if n > 64 {
                return bad("attestation: CBOR array too long");
            }
            let mut v = Vec::new();
            for _ in 0..n {
                v.push(cbor(b, i, depth + 1)?);
            }
            Ok(Cbor::Array(v))
        }
        5 => {
            let mut v = Vec::new();
            if n > 64 {
                return bad("attestation: CBOR map too long");
            }
            for _ in 0..n {
                let k = cbor(b, i, depth + 1)?;
                let x = cbor(b, i, depth + 1)?;
                v.push((k, x));
            }
            Ok(Cbor::Map(v))
        }
        _ => bad("attestation: a CBOR type the attestation never holds"),
    }
}

fn map_get<'a>(m: &'a [(Cbor, Cbor)], key: &str) -> Option<&'a Cbor> {
    m.iter().find(|(k, _)| matches!(k, Cbor::Text(t) if t == key)).map(|(_, v)| v)
}

// ── DER / X.509 ──

/// (tag, contents, whole element) at `i`; moves `i` past it.
fn der<'a>(b: &'a [u8], i: &mut usize) -> Result<(u8, &'a [u8], &'a [u8]), String> {
    let start = *i;
    let tag = *b.get(*i).ok_or("certificate: DER cut short")?;
    *i += 1;
    let first = *b.get(*i).ok_or("certificate: DER cut short")?;
    *i += 1;
    let len = if first < 0x80 {
        first as usize
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 3 {
            return bad("certificate: DER length");
        }
        let mut l = 0usize;
        for _ in 0..n {
            l = (l << 8) | *b.get(*i).ok_or("certificate: DER cut short")? as usize;
            *i += 1;
        }
        l
    };
    let body = b.get(*i..*i + len).ok_or("certificate: DER cut short")?;
    *i += len;
    Ok((tag, body, &b[start..*i]))
}

struct Cert<'a> {
    tbs: &'a [u8],
    sig_alg: &'a [u8],
    sig: &'a [u8],
    not_before: i64,
    not_after: i64,
    /// The key's curve OID and its uncompressed point.
    curve: &'a [u8],
    point: &'a [u8],
    extensions: Vec<(&'a [u8], &'a [u8])>,
}

fn der_time(tag: u8, v: &[u8]) -> Result<i64, String> {
    let s = std::str::from_utf8(v).map_err(|_| "certificate: time")?;
    let full = match (tag, s.len()) {
        (0x17, 13) => {
            let yy: i64 = s[0..2].parse().map_err(|_| "certificate: time")?;
            format!("{}{}", if yy < 50 { 2000 + yy } else { 1900 + yy }, &s[2..])
        }
        (0x18, 15) => s.to_string(),
        _ => return bad("certificate: time format"),
    };
    let iso = format!("{}-{}-{}T{}:{}:{}Z", &full[0..4], &full[4..6], &full[6..8], &full[8..10], &full[10..12], &full[12..14]);
    if !full.ends_with('Z') {
        return bad("certificate: time is not UTC");
    }
    let y: i64 = full[0..4].parse().map_err(|_| "certificate: time")?;
    if y < 2000 {
        return Ok(0);
    }
    crate::time::parse_utc(&iso).ok_or_else(|| "certificate: time".to_string())
}

fn parse_cert(b: &[u8]) -> Result<Cert<'_>, String> {
    let mut i = 0;
    let (t, outer, _) = der(b, &mut i)?;
    if t != 0x30 || i != b.len() {
        return bad("certificate: not one DER certificate");
    }
    let mut j = 0;
    let (t, tbs_body, tbs) = der(outer, &mut j)?;
    if t != 0x30 {
        return bad("certificate: no tbsCertificate");
    }
    let (_, alg, _) = der(outer, &mut j)?;
    let (t, sigbits, _) = der(outer, &mut j)?;
    if t != 0x03 || sigbits.first() != Some(&0) {
        return bad("certificate: signature");
    }
    let mut a = 0;
    let (t, sig_alg, _) = der(alg, &mut a)?;
    if t != 0x06 {
        return bad("certificate: signature algorithm");
    }
    // tbsCertificate: [0] version, serial, signature, issuer, validity, subject, spki, [3] extensions
    let mut k = 0;
    let mut items = Vec::new();
    while k < tbs_body.len() {
        items.push(der(tbs_body, &mut k)?);
    }
    let off = if items.first().map(|x| x.0) == Some(0xa0) { 1 } else { 0 };
    if items.len() < off + 6 {
        return bad("certificate: tbsCertificate too short");
    }
    let validity = items[off + 3].1;
    let mut v = 0;
    let (t1, nb, _) = der(validity, &mut v)?;
    let (t2, na, _) = der(validity, &mut v)?;
    let (not_before, not_after) = (der_time(t1, nb)?, der_time(t2, na)?);
    let spki = items[off + 5].1;
    let mut s = 0;
    let (_, algid, _) = der(spki, &mut s)?;
    let (t, bits, _) = der(spki, &mut s)?;
    if t != 0x03 || bits.first() != Some(&0) || bits.get(1) != Some(&0x04) {
        return bad("certificate: public key is not an uncompressed EC point");
    }
    let mut q = 0;
    let _ = der(algid, &mut q)?; // id-ecPublicKey
    let (_, curve, _) = der(algid, &mut q)?;
    let mut extensions = Vec::new();
    if let Some(&(_, ext_wrap, _)) = items[off + 6..].iter().find(|x| x.0 == 0xa3) {
        let mut e = 0;
        let (_, exts, _) = der(ext_wrap, &mut e)?;
        let mut x = 0;
        while x < exts.len() {
            let (_, one, _) = der(exts, &mut x)?;
            let mut y = 0;
            let (_, oid, _) = der(one, &mut y)?;
            let (mut t, mut val, _) = der(one, &mut y)?;
            if t == 0x01 {
                (t, val, _) = der(one, &mut y)?; // critical BOOLEAN
            }
            if t != 0x04 {
                return bad("certificate: extension value");
            }
            extensions.push((oid, val));
        }
    }
    Ok(Cert { tbs, sig_alg, sig: &sigbits[1..], not_before, not_after, curve, point: &bits[1..], extensions })
}

fn signed_by(child: &Cert, parent: &Cert) -> bool {
    let alg: &'static dyn aws_lc_rs::signature::VerificationAlgorithm = match (parent.curve, child.sig_alg) {
        (c, a) if c == OID_P384 && a == OID_ECDSA_SHA384 => &ECDSA_P384_SHA384_ASN1,
        (c, a) if c == OID_P384 && a == OID_ECDSA_SHA256 => &ECDSA_P384_SHA256_ASN1,
        (c, a) if c == OID_P256 && a == OID_ECDSA_SHA256 => &ECDSA_P256_SHA256_ASN1,
        _ => return false,
    };
    UnparsedPublicKey::new(alg, parent.point).verify(child.tbs, child.sig).is_ok()
}

/// Check an App Attest attestation object over `client_data`, for the App
/// Attest key `key_id_b64` (standard base64 of its 32-byte id) of `app_id`,
/// at `now` (seconds since 1970, the relying party's clock).
pub fn verify_app_attest(attestation: &[u8], client_data: &[u8], key_id_b64: &str, app_id: &str, now: i64) -> Result<Attested, String> {
    use base64::Engine;
    let key_id = base64::engine::general_purpose::STANDARD.decode(key_id_b64).map_err(|_| "the App Attest key id is not base64")?;
    if key_id.len() != 32 {
        return bad("the App Attest key id is not 32 bytes");
    }
    let mut i = 0;
    let Cbor::Map(top) = cbor(attestation, &mut i, 0)? else {
        return bad("attestation: not a CBOR map");
    };
    if i != attestation.len() {
        return bad("attestation: bytes after the CBOR object");
    }
    if !matches!(map_get(&top, "fmt"), Some(Cbor::Text(f)) if f == "apple-appattest") {
        return bad("attestation: fmt is not apple-appattest");
    }
    let Some(Cbor::Bytes(auth)) = map_get(&top, "authData") else {
        return bad("attestation: no authData");
    };
    let Some(Cbor::Map(stmt)) = map_get(&top, "attStmt") else {
        return bad("attestation: no attStmt");
    };
    let Some(Cbor::Array(x5c)) = map_get(stmt, "x5c") else {
        return bad("attestation: no x5c");
    };
    let certs: Vec<&[u8]> = x5c
        .iter()
        .map(|c| match c {
            Cbor::Bytes(b) => Ok(b.as_slice()),
            _ => Err("attestation: x5c holds something other than certificates".to_string()),
        })
        .collect::<Result<_, _>>()?;
    if certs.len() != 2 {
        return bad("attestation: x5c must be the leaf and Apple's intermediate");
    }
    let root_der = crate::keys::pem_block_to_der(APPLE_APP_ATTEST_ROOT_PEM, "CERTIFICATE").map_err(|e| format!("the pinned Apple root: {e}"))?;
    let root = parse_cert(&root_der)?;
    let leaf = parse_cert(certs[0])?;
    let inter = parse_cert(certs[1])?;
    if !signed_by(&inter, &root) {
        return bad("attestation: the intermediate is not signed by Apple's App Attestation Root CA");
    }
    if !signed_by(&leaf, &inter) {
        return bad("attestation: the leaf is not signed by the intermediate");
    }
    for (name, c) in [("root", &root), ("intermediate", &inter), ("leaf", &leaf)] {
        if now < c.not_before || now > c.not_after {
            return bad(format!("attestation: the {name} certificate is not valid now (the leaf lasts 3 days: attest again)"));
        }
    }
    if leaf.curve != OID_P256 || leaf.point.len() != 65 {
        return bad("attestation: the leaf key is not P-256");
    }
    // The nonce: SEQUENCE { [1] { OCTET STRING (32) } }
    let ext = leaf.extensions.iter().find(|(o, _)| *o == OID_NONCE).map(|(_, v)| *v).ok_or("attestation: no nonce extension")?;
    let mut p = 0;
    let (_, seq, _) = der(ext, &mut p)?;
    let mut q = 0;
    let (t, tagged, _) = der(seq, &mut q)?;
    let mut r = 0;
    let (t2, nonce, _) = der(tagged, &mut r)?;
    if t != 0xa1 || t2 != 0x04 || nonce.len() != 32 {
        return bad("attestation: the nonce extension is malformed");
    }
    let mut h = Sha256::new();
    h.update(auth);
    h.update(Sha256::digest(client_data));
    if nonce != h.finalize().as_slice() {
        return bad("attestation: the nonce is not SHA-256(authData ‖ SHA-256(clientData)): it was made for other data");
    }
    if Sha256::digest(leaf.point).as_slice() != key_id.as_slice() {
        return bad("attestation: the leaf's key is not the App Attest key named");
    }
    // authData: rpIdHash(32) flags(1) signCount(4) aaguid(16) credIdLen(2) credId
    if auth.len() < 55 {
        return bad("attestation: authData is too short");
    }
    if auth[..32] != *Sha256::digest(app_id.as_bytes()).as_slice() {
        return bad(format!("attestation: rpIdHash is not SHA-256({app_id}): another app made it"));
    }
    if auth[33..37] != [0, 0, 0, 0] {
        return bad("attestation: signCount is not 0 (an attestation is made once)");
    }
    let environment = match &auth[37..53] {
        b"appattestdevelop" => Environment::Development,
        x if x == b"appattest\0\0\0\0\0\0\0" => Environment::Production,
        _ => return bad("attestation: unknown AAGUID"),
    };
    let cred_len = u16::from_be_bytes([auth[53], auth[54]]) as usize;
    if cred_len != 32 || auth.get(55..87) != Some(key_id.as_slice()) {
        return bad("attestation: credentialId is not the App Attest key id");
    }
    Ok(Attested { environment, verified_at: now, leaf_not_after: leaf.not_after })
}

/// Split the enrolment client data: (challenge, SPKI(biometric), SPKI(passcode)).
pub fn parse_enrol_client_data(cd: &[u8]) -> Result<([u8; 32], Vec<u8>, Vec<u8>), String> {
    let Some(rest) = cd.strip_prefix(ENROL_PREFIX) else {
        return bad("enrolment data does not start with K2-APPROVER-ENROL-1 and a zero byte");
    };
    if rest.len() != 32 + 91 + 91 {
        return bad("enrolment data must be the 32-byte challenge and two P-256 public keys");
    }
    let mut challenge = [0u8; 32];
    challenge.copy_from_slice(&rest[..32]);
    let (bio, pass) = (&rest[32..123], &rest[123..]);
    for (name, k) in [("biometric", bio), ("passcode", pass)] {
        match crate::keys::parse_spki(k) {
            Ok(pk) if pk.alg == crate::Alg::Es256 => {}
            _ => return bad(format!("enrolment data: the {name} key is not a P-256 public key")),
        }
    }
    if bio == pass {
        return bad("enrolment data: the two keys are the same key");
    }
    Ok((challenge, bio.to_vec(), pass.to_vec()))
}

/// The whole enrolment check: the client data's form, the relying party's
/// own challenge, and the attestation over it.
pub fn verify_enrolment(attestation: &[u8], client_data: &[u8], key_id_b64: &str, expected_challenge: &[u8; 32], app_id: &str, now: i64) -> Result<Enrolment, String> {
    let (challenge, bio, pass) = parse_enrol_client_data(client_data)?;
    if &challenge != expected_challenge {
        return bad("enrolment data: the challenge is not the one this relying party issued");
    }
    let attested = verify_app_attest(attestation, client_data, key_id_b64, app_id, now)?;
    let (bk, pk) = (crate::keys::key_id(&bio), crate::keys::key_id(&pass));
    debug_assert!(is_hex_lower(&bk, 64) && is_hex_lower(&pk, 64));
    Ok(Enrolment { attested, challenge, biometric_spki: bio, passcode_spki: pass, biometric_key_id: bk, passcode_key_id: pk })
}
