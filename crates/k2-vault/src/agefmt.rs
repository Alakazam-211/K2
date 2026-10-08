//! The age v1 file format (C2SP `age.md`), X25519 recipients only.
//!
//! Used for the offline holders of the vault data key (recovery key,
//! backup recipient, clone target) and for clone carries, so stock `rage`
//! / `age` can open an export. Written here instead of linking the `age`
//! crate (KV3: its i18n stack is ~20 crates); the tests check both
//! directions against `age` itself.

use base64::engine::general_purpose::STANDARD_NO_PAD;
use base64::Engine as _;
use bech32::{FromBase32, ToBase32, Variant};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

use crate::crypto::{hmac, hmac_verify, random_bytes};
use crate::error::{Result, VaultError};
use crate::secret::{Key32, SecretBytes};

const INTRO: &str = "age-encryption.org/v1";
const X25519_LABEL: &[u8] = b"age-encryption.org/v1/X25519";
const CHUNK: usize = 64 * 1024;
const RECIPIENT_HRP: &str = "age";
const IDENTITY_HRP: &str = "age-secret-key-";

fn hkdf_raw(salt: &[u8], info: &[u8], ikm: &[u8]) -> Key32 {
    let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut out = [0u8; 32];
    hk.expand(info, &mut out).expect("32-byte HKDF output");
    Key32::from_array(out)
}

/// An age X25519 recipient (public key).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Recipient(pub [u8; 32]);

impl Recipient {
    pub fn encode(&self) -> String {
        bech32::encode(RECIPIENT_HRP, self.0.to_base32(), Variant::Bech32).expect("valid hrp")
    }

    pub fn parse(s: &str) -> Result<Recipient> {
        let (hrp, data, variant) = bech32::decode(s.trim())
            .map_err(|_| VaultError::bad_request("bad_recipient", "not an age recipient (age1…)"))?;
        if hrp != RECIPIENT_HRP || variant != Variant::Bech32 {
            return Err(VaultError::bad_request("bad_recipient", "not an age X25519 recipient"));
        }
        let bytes = Vec::<u8>::from_base32(&data)
            .map_err(|_| VaultError::bad_request("bad_recipient", "bad recipient payload"))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| VaultError::bad_request("bad_recipient", "recipient must be 32 bytes"))?;
        Ok(Recipient(arr))
    }

    /// Short fingerprint for humans comparing out of band (VS20).
    pub fn fingerprint(&self) -> String {
        let h = crate::crypto::sha256(&self.0);
        crate::crypto::hex(&h[..8])
    }
}

/// An age X25519 identity (secret scalar), in locked memory.
pub struct Identity(Key32);

impl Identity {
    pub fn generate() -> Identity {
        Identity(Key32::random())
    }

    pub fn from_scalar(k: Key32) -> Identity {
        Identity(k)
    }

    pub fn scalar(&self) -> &Key32 {
        &self.0
    }

    pub fn recipient(&self) -> Recipient {
        let sk = StaticSecret::from(*self.0.as_bytes());
        Recipient(PublicKey::from(&sk).to_bytes())
    }

    /// `AGE-SECRET-KEY-1…` (what `rage -i` reads).
    pub fn encode(&self) -> SecretBytes {
        let s = bech32::encode(IDENTITY_HRP, self.0.as_bytes().to_base32(), Variant::Bech32)
            .expect("valid hrp");
        SecretBytes::from_string(s.to_uppercase())
    }

    pub fn parse(s: &str) -> Result<Identity> {
        let lower = s.trim().to_lowercase();
        let (hrp, data, variant) = bech32::decode(&lower)
            .map_err(|_| VaultError::bad_request("bad_identity", "not an age identity"))?;
        if hrp != IDENTITY_HRP || variant != Variant::Bech32 {
            return Err(VaultError::bad_request("bad_identity", "not an age X25519 identity"));
        }
        let mut bytes = Vec::<u8>::from_base32(&data)
            .map_err(|_| VaultError::bad_request("bad_identity", "bad identity payload"))?;
        if bytes.len() != 32 {
            bytes.zeroize();
            return Err(VaultError::bad_request("bad_identity", "identity must be 32 bytes"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        bytes.zeroize();
        Ok(Identity(Key32::from_array(arr)))
    }
}

fn wrap_line_64(s: &str, out: &mut String) {
    // Body lines are 64 columns; the last line is always shorter than 64
    // (an empty line when the body is a multiple of 64 columns).
    let bytes = s.as_bytes();
    let mut i = 0;
    loop {
        let end = (i + 64).min(bytes.len());
        out.push_str(&s[i..end]);
        out.push('\n');
        if end - i < 64 {
            break;
        }
        i = end;
    }
}

fn stream_nonce(counter: u64, last: bool) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[3..11].copy_from_slice(&counter.to_be_bytes());
    n[11] = u8::from(last);
    n
}

/// Encrypt `plaintext` to one or more X25519 recipients.
pub fn encrypt(recipients: &[Recipient], plaintext: &[u8]) -> Vec<u8> {
    assert!(!recipients.is_empty(), "age encrypt needs a recipient");
    let mut file_key = random_bytes::<16>();
    let mut header = String::new();
    header.push_str(INTRO);
    header.push('\n');
    for r in recipients {
        let eph = StaticSecret::from(random_bytes::<32>());
        let share = PublicKey::from(&eph);
        let shared = eph.diffie_hellman(&PublicKey::from(r.0));
        let mut salt = Vec::with_capacity(64);
        salt.extend_from_slice(share.as_bytes());
        salt.extend_from_slice(&r.0);
        let wrap_key = hkdf_raw(&salt, X25519_LABEL, shared.as_bytes());
        let body = ChaCha20Poly1305::new(wrap_key.as_bytes().into())
            .encrypt(Nonce::from_slice(&[0u8; 12]), file_key.as_slice())
            .expect("encrypt file key");
        header.push_str("-> X25519 ");
        header.push_str(&STANDARD_NO_PAD.encode(share.as_bytes()));
        header.push('\n');
        wrap_line_64(&STANDARD_NO_PAD.encode(&body), &mut header);
    }
    header.push_str("---");
    let mac_key = hkdf_raw(&[], b"header", &file_key);
    let mac = hmac(&mac_key, header.as_bytes());
    header.push(' ');
    header.push_str(&STANDARD_NO_PAD.encode(mac));
    header.push('\n');

    let nonce = random_bytes::<16>();
    let payload_key = hkdf_raw(&nonce, b"payload", &file_key);
    file_key.zeroize();
    let cipher = ChaCha20Poly1305::new(payload_key.as_bytes().into());
    let mut out = header.into_bytes();
    out.extend_from_slice(&nonce);
    let chunks: Vec<&[u8]> = if plaintext.is_empty() {
        vec![&[][..]]
    } else {
        plaintext.chunks(CHUNK).collect()
    };
    let n = chunks.len();
    for (i, c) in chunks.into_iter().enumerate() {
        let ct = cipher
            .encrypt(Nonce::from_slice(&stream_nonce(i as u64, i + 1 == n)), c)
            .expect("encrypt chunk");
        out.extend_from_slice(&ct);
    }
    out
}

fn bad(hint: &str) -> VaultError {
    VaultError::bad_request("bad_age_file", hint.to_string())
}

/// Decrypt an age file with an X25519 identity.
pub fn decrypt(identity: &Identity, file: &[u8]) -> Result<SecretBytes> {
    // Split the header (text) from the payload (binary) at "---" line end.
    let mut pos = 0usize;
    let mut lines: Vec<&str> = Vec::new();
    let mac_line;
    loop {
        let nl = file[pos..].iter().position(|&b| b == b'\n').ok_or_else(|| bad("truncated header"))?;
        let line = std::str::from_utf8(&file[pos..pos + nl]).map_err(|_| bad("header is not text"))?;
        pos += nl + 1;
        if line.starts_with("---") {
            mac_line = line;
            break;
        }
        lines.push(line);
        if lines.len() > 4096 {
            return Err(bad("header too long"));
        }
    }
    if lines.first() != Some(&INTRO) {
        return Err(bad("not an age v1 file"));
    }
    let mac_b64 = mac_line.strip_prefix("--- ").ok_or_else(|| bad("bad MAC line"))?;
    let mac = STANDARD_NO_PAD.decode(mac_b64).map_err(|_| bad("bad MAC encoding"))?;
    let header_text_len = pos - (mac_line.len() + 1) + 3; // through "---"
    let header_bytes = &file[..header_text_len];

    // Walk stanzas.
    let my_recipient = identity.recipient();
    let sk = StaticSecret::from(*identity.scalar().as_bytes());
    let mut file_key: Option<[u8; 16]> = None;
    let mut i = 1;
    while i < lines.len() {
        let l = lines[i];
        let args = l.strip_prefix("-> ").ok_or_else(|| bad("bad stanza line"))?;
        let parts: Vec<&str> = args.split(' ').collect();
        i += 1;
        let mut body_b64 = String::new();
        loop {
            let bl = *lines.get(i).ok_or_else(|| bad("truncated stanza"))?;
            i += 1;
            body_b64.push_str(bl);
            if bl.len() < 64 {
                break;
            }
        }
        if file_key.is_some() || parts.first() != Some(&"X25519") || parts.len() != 2 {
            continue;
        }
        let share: [u8; 32] = STANDARD_NO_PAD
            .decode(parts[1])
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| bad("bad X25519 share"))?;
        let body = STANDARD_NO_PAD.decode(&body_b64).map_err(|_| bad("bad stanza body"))?;
        if body.len() != 32 {
            return Err(bad("bad X25519 body length"));
        }
        let shared = sk.diffie_hellman(&PublicKey::from(share));
        if shared.as_bytes().iter().all(|&b| b == 0) {
            return Err(bad("degenerate X25519 share"));
        }
        let mut salt = Vec::with_capacity(64);
        salt.extend_from_slice(&share);
        salt.extend_from_slice(&my_recipient.0);
        let wrap_key = hkdf_raw(&salt, X25519_LABEL, shared.as_bytes());
        if let Ok(mut fk) = ChaCha20Poly1305::new(wrap_key.as_bytes().into())
            .decrypt(Nonce::from_slice(&[0u8; 12]), body.as_slice())
        {
            if fk.len() == 16 {
                let mut arr = [0u8; 16];
                arr.copy_from_slice(&fk);
                file_key = Some(arr);
            }
            fk.zeroize();
        }
    }
    let mut file_key = file_key.ok_or_else(|| {
        VaultError::forbidden("age_no_matching_identity", "this key can't open that file")
    })?;
    let mac_key = hkdf_raw(&[], b"header", &file_key);
    if !hmac_verify(&mac_key, header_bytes, &mac) {
        file_key.zeroize();
        return Err(VaultError::tamper("age header MAC mismatch"));
    }
    let payload = &file[pos..];
    if payload.len() < 16 + 16 {
        file_key.zeroize();
        return Err(bad("truncated payload"));
    }
    let payload_key = hkdf_raw(&payload[..16], b"payload", &file_key);
    file_key.zeroize();
    let cipher = ChaCha20Poly1305::new(payload_key.as_bytes().into());
    let body = &payload[16..];
    let enc_chunk = CHUNK + 16;
    let n = body.len().div_ceil(enc_chunk);
    let mut out = Vec::with_capacity(body.len());
    for (idx, c) in body.chunks(enc_chunk).enumerate() {
        let last = idx + 1 == n;
        let pt = cipher
            .decrypt(Nonce::from_slice(&stream_nonce(idx as u64, last)), c)
            .map_err(|_| VaultError::tamper("age payload failed authentication"))?;
        if last && pt.is_empty() && idx > 0 {
            return Err(bad("empty final chunk"));
        }
        out.extend_from_slice(&pt);
    }
    Ok(SecretBytes::new(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::str::FromStr;

    #[test]
    fn round_trip_small_and_multi_chunk() {
        let id = Identity::generate();
        let r = id.recipient();
        for len in [0usize, 1, 32, CHUNK, CHUNK + 1, 2 * CHUNK + 7] {
            let pt: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let f = encrypt(&[r], &pt);
            assert_eq!(decrypt(&id, &f).expect("decrypts").expose(), pt.as_slice(), "len {len}");
        }
    }

    #[test]
    fn wrong_identity_and_tamper_fail() {
        let id = Identity::generate();
        let other = Identity::generate();
        let f = encrypt(&[id.recipient()], b"vdk-bytes");
        assert_eq!(decrypt(&other, &f).err().expect("wrong key").code, "age_no_matching_identity");
        let mut t = f.clone();
        let last = t.len() - 1;
        t[last] ^= 1;
        assert_eq!(decrypt(&id, &t).err().expect("tampered payload").code, "vault_tamper");
    }

    #[test]
    fn multiple_recipients_each_open() {
        let a = Identity::generate();
        let b = Identity::generate();
        let f = encrypt(&[a.recipient(), b.recipient()], b"x");
        assert_eq!(decrypt(&a, &f).expect("a").expose(), b"x");
        assert_eq!(decrypt(&b, &f).expect("b").expose(), b"x");
    }

    #[test]
    fn identity_and_recipient_encodings_round_trip() {
        let id = Identity::generate();
        let enc = id.encode();
        let s = enc.expose_str().expect("utf8").to_string();
        assert!(s.starts_with("AGE-SECRET-KEY-1"));
        let back = Identity::parse(&s).expect("parses");
        assert_eq!(back.recipient(), id.recipient());
        let r = id.recipient().encode();
        assert!(r.starts_with("age1"));
        assert_eq!(Recipient::parse(&r).expect("parses"), id.recipient());
        assert!(Recipient::parse("age1qqqq").is_err());
    }

    #[test]
    fn interop_our_file_opens_with_the_age_crate() {
        let id = Identity::generate();
        let f = encrypt(&[id.recipient()], b"interop payload");
        let s = id.encode();
        let age_id = age::x25519::Identity::from_str(s.expose_str().expect("utf8")).expect("age parses our identity");
        assert_eq!(age_id.to_public().to_string(), id.recipient().encode());
        let dec = age::Decryptor::new(&f[..]).expect("age reads our header");
        let mut r = dec.decrypt(std::iter::once(&age_id as &dyn age::Identity)).expect("age decrypts");
        let mut out = Vec::new();
        r.read_to_end(&mut out).expect("read");
        assert_eq!(out, b"interop payload");
    }

    #[test]
    fn interop_age_file_opens_with_ours() {
        let id = Identity::generate();
        let age_recipient = age::x25519::Recipient::from_str(&id.recipient().encode()).expect("age parses ours");
        let enc = age::Encryptor::with_recipients(std::iter::once(&age_recipient as &dyn age::Recipient))
            .expect("encryptor");
        let mut buf = Vec::new();
        let mut w = enc.wrap_output(&mut buf).expect("wrap");
        let big: Vec<u8> = (0..(CHUNK * 2 + 3)).map(|i| (i % 7) as u8).collect();
        w.write_all(&big).expect("write");
        w.finish().expect("finish");
        assert_eq!(decrypt(&id, &buf).expect("we open age's file").expose(), big.as_slice());
    }
}
