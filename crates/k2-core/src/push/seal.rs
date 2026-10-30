//! Sealing a push for one phone (prd-e2e-push-v1 rev 2 §3): HPKE
//! (RFC 9180) **PSK mode**, DHKEM(P-256, HKDF-SHA256) / HKDF-SHA256 /
//! AES-256-GCM, fixed-size padded plaintexts.
//!
//! Wire blob (`e2e`, base64url, no padding):
//! `0x01 (format v1) ‖ 0x01 (suite) ‖ enc (65-byte SEC1) ‖ ciphertext+tag`.
//! - AAD = the first 2 bytes.
//! - `info` = `"k2-push-v1\0alert"` or `"k2-push-v1\0nudge"`.
//! - `psk_id` = the ASCII constant `k2-push-psk-v1`.
//! - Plaintext = `u16be(len) ‖ JSON ‖ 0x00…`, padded to 1536 (alert) or
//!   256 (nudge) bytes, so a blob is always 1619 / 339 bytes = 2159 / 452
//!   base64url characters (the relay refuses any other length).
//!
//! **Format 2** (`push-e2e-v2`, 0.46.7): the same suite, `info`, PSK and
//! padding rule, with byte 0 = `0x02` (so the AAD binds the format) and
//! bigger buckets: 2880 (alert) / 1024 (nudge) bytes of plaintext, so a
//! blob is 2963 / 1107 bytes = 3951 / 1476 base64url characters. The
//! alert bucket is the largest 64-byte multiple whose relay APNs body
//! (`{"aps":{…},"kind":"e2e","v":"2","e2e":"<3951>"}`, 4072 bytes) stays
//! under Apple's 4096-byte cap; FCM's `data` is 3963 bytes of its 4096.
//! Only for a phone that registered `e2e.v: 2` and a relay that advertised
//! `s2`; everything else stays format 1.
//!
//! No key id rides in clear: the phone trial-decrypts with its keys. This
//! module is pure: no DB, no network, no logging (never log a blob, a key
//! or a plaintext).

use base64::Engine as _;
use hpke::{
    aead::AesGcm256, kdf::HkdfSha256, kem::DhP256HkdfSha256, Deserializable, Kem as KemTrait, OpModeR, OpModeS,
    PskBundle, Serializable,
};

type Kem = DhP256HkdfSha256;

/// Blob byte 0: the format version.
pub const FORMAT_V1: u8 = 0x01;
/// Blob byte 0 of format 2 (the bigger buckets).
pub const FORMAT_V2: u8 = 0x02;
/// Blob byte 1: the suite (mode_psk / DHKEM P-256 / HKDF-SHA256 / AES-256-GCM).
pub const SUITE_ID: u8 = 0x01;
/// The suite name a phone registers (`e2e.suite`).
pub const SUITE_NAME: &str = "p256-sha256-aes256gcm-psk";
/// The HPKE `psk_id` (constant, so no per-device id rides in clear).
pub const PSK_ID: &[u8] = b"k2-push-psk-v1";
/// A P-256 public key, SEC1 uncompressed.
pub const PUBLIC_KEY_LEN: usize = 65;
/// The phone's PSK.
pub const PSK_LEN: usize = 32;
/// `enc`: the ephemeral public key, SEC1 uncompressed.
pub const ENC_LEN: usize = 65;
/// The AES-GCM tag.
pub const TAG_LEN: usize = 16;
/// Format + suite bytes (also the AAD).
pub const HEADER_LEN: usize = 2;
/// Padded alert plaintext.
pub const ALERT_PAD: usize = 1536;
/// Padded nudge plaintext.
pub const NUDGE_PAD: usize = 256;
/// Format 2's padded alert plaintext (the largest 64-byte multiple that
/// keeps the relay's APNs body under 4096 bytes).
pub const ALERT_PAD_V2: usize = 2880;
/// Format 2's padded nudge plaintext (room for the `stale` hint).
pub const NUDGE_PAD_V2: usize = 1024;
/// Apple's cap on a push payload, and FCM's on a message's `data`.
pub const VENDOR_PAYLOAD_MAX: usize = 4096;
/// The `data.kind` of a sealed alert on the relay wire.
pub const WIRE_KIND_ALERT: &str = "e2e";
/// The `data.kind` of a sealed nudge on the relay wire.
pub const WIRE_KIND_NUDGE: &str = "nudge";
/// `data.v` on the relay wire (a string, like every `data` value).
pub const WIRE_VERSION: &str = "1";

/// base64url without padding (the blob encoding).
pub const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// The two sealed kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealKind {
    /// A visible notification (thread, ticket, project).
    Alert,
    /// A silent badge nudge.
    Nudge,
}

impl SealKind {
    /// The padded plaintext size.
    pub const fn pad_len(self) -> usize {
        match self {
            Self::Alert => ALERT_PAD,
            Self::Nudge => NUDGE_PAD,
        }
    }

    /// The blob size in bytes: 1619 (alert) / 339 (nudge).
    pub const fn blob_len(self) -> usize {
        HEADER_LEN + ENC_LEN + self.pad_len() + TAG_LEN
    }

    /// The base64url length: 2159 (alert) / 452 (nudge).
    pub const fn b64_len(self) -> usize {
        (self.blob_len() * 4).div_ceil(3)
    }

    /// The HPKE `info`, binding the kind (an alert can't be replayed as a nudge).
    pub const fn info(self) -> &'static [u8] {
        match self {
            Self::Alert => b"k2-push-v1\0alert",
            Self::Nudge => b"k2-push-v1\0nudge",
        }
    }

    /// The relay wire `data.kind`.
    pub const fn wire_kind(self) -> &'static str {
        match self {
            Self::Alert => WIRE_KIND_ALERT,
            Self::Nudge => WIRE_KIND_NUDGE,
        }
    }
}

/// The blob format: which buckets and which byte 0. Format 1 is every
/// phone's; format 2 needs the phone's `e2e.v: 2` and the relay's `s2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Format {
    V1,
    V2,
}

impl Format {
    /// Blob byte 0 (also in the AAD).
    pub const fn byte(self) -> u8 {
        match self {
            Self::V1 => FORMAT_V1,
            Self::V2 => FORMAT_V2,
        }
    }

    /// The relay wire `data.v` (a string).
    pub const fn wire_version(self) -> &'static str {
        match self {
            Self::V1 => WIRE_VERSION,
            Self::V2 => WIRE_VERSION_V2,
        }
    }

    /// The number a phone registers (`e2e.v`) and a key row stores.
    pub const fn number(self) -> u32 {
        match self {
            Self::V1 => 1,
            Self::V2 => 2,
        }
    }

    /// The padded plaintext size of `kind` in this format.
    pub const fn pad_len(self, kind: SealKind) -> usize {
        match (self, kind) {
            (Self::V1, k) => k.pad_len(),
            (Self::V2, SealKind::Alert) => ALERT_PAD_V2,
            (Self::V2, SealKind::Nudge) => NUDGE_PAD_V2,
        }
    }

    /// The blob size in bytes.
    pub const fn blob_len(self, kind: SealKind) -> usize {
        HEADER_LEN + ENC_LEN + self.pad_len(kind) + TAG_LEN
    }

    /// The base64url length (what the relay checks).
    pub const fn b64_len(self, kind: SealKind) -> usize {
        (self.blob_len(kind) * 4).div_ceil(3)
    }

    /// The most JSON bytes a bucket holds.
    pub const fn max_json(self, kind: SealKind) -> usize {
        self.pad_len(kind) - 2
    }
}

/// `data.v` of a format-2 sealed push on the relay wire.
pub const WIRE_VERSION_V2: &str = "2";

/// Why a seal or open failed. Never carries key, plaintext or blob bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealError {
    /// The JSON doesn't fit the bucket (`len`, `max`).
    TooLarge { len: usize, max: usize },
    /// A bad public key, PSK or private key.
    BadKey(&'static str),
    /// HPKE failed (encap, decap or AEAD open).
    Crypto(&'static str),
    /// The blob or the padding is malformed.
    Malformed(&'static str),
}

impl std::fmt::Display for SealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { len, max } => write!(f, "sealed plaintext too large ({len} > {max})"),
            Self::BadKey(w) => write!(f, "bad key: {w}"),
            Self::Crypto(w) => write!(f, "hpke: {w}"),
            Self::Malformed(w) => write!(f, "malformed: {w}"),
        }
    }
}

impl std::error::Error for SealError {}

/// The most JSON bytes a bucket holds (`pad - 2` for the length prefix).
pub const fn max_json(kind: SealKind) -> usize {
    kind.pad_len() - 2
}

/// `u16be(len) ‖ json ‖ 0x00…` to exactly `kind.pad_len()` bytes.
pub fn pad(json: &[u8], kind: SealKind) -> Result<Vec<u8>, SealError> {
    pad_in(json, kind, Format::V1)
}

/// [`pad`] to `format`'s bucket.
pub fn pad_in(json: &[u8], kind: SealKind, format: Format) -> Result<Vec<u8>, SealError> {
    let max = format.max_json(kind);
    if json.len() > max {
        return Err(SealError::TooLarge { len: json.len(), max });
    }
    let mut out = Vec::with_capacity(format.pad_len(kind));
    out.extend_from_slice(&(json.len() as u16).to_be_bytes());
    out.extend_from_slice(json);
    out.resize(format.pad_len(kind), 0);
    Ok(out)
}

/// Undo [`pad`]: the length prefix must fit and every byte after the JSON
/// must be `0x00`.
pub fn unpad(pt: &[u8]) -> Result<&[u8], SealError> {
    if pt.len() < 2 {
        return Err(SealError::Malformed("plaintext shorter than its length prefix"));
    }
    let len = u16::from_be_bytes([pt[0], pt[1]]) as usize;
    let end = 2 + len;
    if end > pt.len() {
        return Err(SealError::Malformed("length prefix past the end"));
    }
    if pt[end..].iter().any(|b| *b != 0) {
        return Err(SealError::Malformed("non-zero padding"));
    }
    Ok(&pt[2..end])
}

/// Hex of the first 8 bytes of SHA-256(public key): the `kid`/`keyId`.
pub fn key_id(public_key: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(public_key).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Check a phone public key: 65 bytes, SEC1 uncompressed, on the curve.
pub fn check_public_key(public_key: &[u8]) -> Result<(), SealError> {
    if public_key.len() != PUBLIC_KEY_LEN || public_key[0] != 0x04 {
        return Err(SealError::BadKey("public key must be a 65-byte SEC1 uncompressed P-256 point"));
    }
    p256::PublicKey::from_sec1_bytes(public_key)
        .map(|_| ())
        .map_err(|_| SealError::BadKey("public key is not a point on P-256"))
}

/// Seal one padded plaintext to one phone, with the given RNG for the
/// ephemeral key (the ephemeral is `DeriveKeyPair(32 bytes from rng)`).
/// Production uses [`seal`]; a fixed RNG makes the test vectors
/// reproducible.
pub fn seal_with_rng<R: hpke::rand_core::CryptoRng + hpke::rand_core::RngCore>(
    kind: SealKind,
    public_key: &[u8],
    psk: &[u8],
    json: &[u8],
    rng: &mut R,
) -> Result<Vec<u8>, SealError> {
    seal_with_rng_in(kind, Format::V1, public_key, psk, json, rng)
}

/// [`seal_with_rng`] in `format`.
pub fn seal_with_rng_in<R: hpke::rand_core::CryptoRng + hpke::rand_core::RngCore>(
    kind: SealKind,
    format: Format,
    public_key: &[u8],
    psk: &[u8],
    json: &[u8],
    rng: &mut R,
) -> Result<Vec<u8>, SealError> {
    check_public_key(public_key)?;
    if psk.len() != PSK_LEN {
        return Err(SealError::BadKey("psk must be 32 bytes"));
    }
    let pk = <Kem as KemTrait>::PublicKey::from_bytes(public_key)
        .map_err(|_| SealError::BadKey("public key rejected by hpke"))?;
    let padded = pad_in(json, kind, format)?;
    let header = [format.byte(), SUITE_ID];
    let mode = OpModeS::Psk(PskBundle { psk, psk_id: PSK_ID });
    let (enc, mut ctx) = hpke::setup_sender::<AesGcm256, HkdfSha256, Kem, _>(&mode, &pk, kind.info(), rng)
        .map_err(|_| SealError::Crypto("encap failed"))?;
    let ct = ctx.seal(&padded, &header).map_err(|_| SealError::Crypto("seal failed"))?;
    let mut blob = Vec::with_capacity(format.blob_len(kind));
    blob.extend_from_slice(&header);
    blob.extend_from_slice(&enc.to_bytes());
    blob.extend_from_slice(&ct);
    debug_assert_eq!(blob.len(), format.blob_len(kind));
    if blob.len() != format.blob_len(kind) {
        return Err(SealError::Malformed("blob length drifted"));
    }
    Ok(blob)
}

/// OS randomness for the ephemeral key.
struct OsRng;

impl hpke::rand_core::RngCore for OsRng {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.fill_bytes(&mut b);
        u32::from_le_bytes(b)
    }
    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        // A failing OS RNG must never produce a weak ephemeral key.
        getrandom::getrandom(dest).expect("OS randomness unavailable");
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), hpke::rand_core::Error> {
        getrandom::getrandom(dest).map_err(|e| hpke::rand_core::Error::from(core::num::NonZeroU32::from(e.code())))
    }
}

impl hpke::rand_core::CryptoRng for OsRng {}

/// Seal with a fresh OS-random ephemeral key; returns the base64url blob
/// (exactly [`SealKind::b64_len`] characters).
pub fn seal(kind: SealKind, public_key: &[u8], psk: &[u8], json: &[u8]) -> Result<String, SealError> {
    seal_in(kind, Format::V1, public_key, psk, json)
}

/// [`seal`] in `format` (exactly [`Format::b64_len`] characters).
pub fn seal_in(kind: SealKind, format: Format, public_key: &[u8], psk: &[u8], json: &[u8]) -> Result<String, SealError> {
    let blob = seal_with_rng_in(kind, format, public_key, psk, json, &mut OsRng)?;
    Ok(B64.encode(blob))
}

/// The receiver side (what the phones implement; here for tests, the test
/// vectors and the headless check): decode, check the header and length,
/// open with the private key + PSK, unpad. Returns the JSON bytes. Both
/// formats open: the length names the format, and byte 0 must agree.
pub fn open(kind: SealKind, private_key: &[u8], psk: &[u8], blob_b64: &str) -> Result<Vec<u8>, SealError> {
    open_format(kind, private_key, psk, blob_b64).map(|(_, json)| json)
}

/// [`open`], also saying which format the blob was.
pub fn open_format(kind: SealKind, private_key: &[u8], psk: &[u8], blob_b64: &str) -> Result<(Format, Vec<u8>), SealError> {
    let format = [Format::V1, Format::V2]
        .into_iter()
        .find(|f| f.b64_len(kind) == blob_b64.len())
        .ok_or(SealError::Malformed("wrong blob length"))?;
    let blob = B64.decode(blob_b64).map_err(|_| SealError::Malformed("not base64url"))?;
    if blob.len() != format.blob_len(kind) || blob[0] != format.byte() || blob[1] != SUITE_ID {
        return Err(SealError::Malformed("bad header or length"));
    }
    let sk = <Kem as KemTrait>::PrivateKey::from_bytes(private_key)
        .map_err(|_| SealError::BadKey("private key rejected by hpke"))?;
    let enc = <Kem as KemTrait>::EncappedKey::from_bytes(&blob[HEADER_LEN..HEADER_LEN + ENC_LEN])
        .map_err(|_| SealError::Malformed("bad enc"))?;
    let mode = OpModeR::Psk(PskBundle { psk, psk_id: PSK_ID });
    let mut ctx = hpke::setup_receiver::<AesGcm256, HkdfSha256, Kem>(&mode, &sk, &enc, kind.info())
        .map_err(|_| SealError::Crypto("decap failed"))?;
    let pt = ctx
        .open(&blob[HEADER_LEN + ENC_LEN..], &blob[..HEADER_LEN])
        .map_err(|_| SealError::Crypto("open failed"))?;
    Ok((format, unpad(&pt)?.to_vec()))
}

/// The public key (SEC1 uncompressed) of a P-256 private key (tests,
/// vectors).
pub fn public_key_of(private_key: &[u8]) -> Result<Vec<u8>, SealError> {
    let sk = <Kem as KemTrait>::PrivateKey::from_bytes(private_key)
        .map_err(|_| SealError::BadKey("private key rejected by hpke"))?;
    Ok(<Kem as KemTrait>::sk_to_pk(&sk).to_bytes().to_vec())
}

/// `DeriveKeyPair(ikm)` (RFC 9180 §7.1.3) → (private, public SEC1). The
/// ephemeral key [`seal_with_rng`] uses is `DeriveKeyPair` of the first 32
/// bytes its RNG yields.
pub fn derive_keypair(ikm: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let (sk, pk) = <Kem as KemTrait>::derive_keypair(ikm);
    (sk.to_bytes().to_vec(), pk.to_bytes().to_vec())
}

/// A deterministic "RNG" that yields `ikm` over and over: the ephemeral
/// key becomes `DeriveKeyPair(ikm)`. **Test vectors only**: never for a
/// real push.
pub struct FixedIkm(pub [u8; 32]);

impl hpke::rand_core::RngCore for FixedIkm {
    fn next_u32(&mut self) -> u32 {
        u32::from_le_bytes([self.0[0], self.0[1], self.0[2], self.0[3]])
    }
    fn next_u64(&mut self) -> u64 {
        u64::from_le_bytes(self.0[..8].try_into().expect("8 bytes"))
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for (i, b) in dest.iter_mut().enumerate() {
            *b = self.0[i % 32];
        }
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), hpke::rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl hpke::rand_core::CryptoRng for FixedIkm {}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex")).collect()
    }

    /// RFC 9180 test vector: mode_psk (1), DHKEM(P-256, HKDF-SHA256) 0x0010,
    /// HKDF-SHA256 0x0001, AES-256-GCM 0x0002 (the CFRG test-vectors.json
    /// entry rust-hpke ships, `test-vectors-5f503c5.json`). The sender side
    /// (ephemeral from `ikmE`) produces the vector's `enc` and first
    /// ciphertext; the receiver side opens it.
    #[test]
    fn rfc9180_psk_mode_vector_p256_sha256_aes256gcm() {
        let ikm_e = hex("3f9edbfb0f212a16692104c98023db64197b8c94831cbc0c1e62d752d0a097e6");
        let sk_rm = hex("dd70766222d5a88e72c247bd8ad9c28ea49125ee463a63902cc6db68c34f76a6");
        let pk_rm = hex("04349f377dc7fcbb0d52d09e7caa97f53a1badc59aac6959f74a4f5a965f1015d4eeced4cd89f4b3d06c7a716e741d4a9863d8313843c987b96f756b111080f07c");
        let psk = hex("0247fd33b913760fa1fa51e1892d9f307fbe65eb171e8132c2af18555a738b82");
        let psk_id = hex("456e6e796e20447572696e206172616e204d6f726961");
        let info = hex("4f6465206f6e2061204772656369616e2055726e");
        let enc_want = hex("04a3cd1fd41bb0915973a14325a6c7612b336630e6c2fd3f3ae5a311bfe950d493155f446f3fc4a45d439073e998624fca9490ac7eca4c312271d8720f8e6d7a74");
        let aad = hex("436f756e742d30");
        let pt = hex("4265617574792069732074727574682c20747275746820626561757479");
        let ct_want = hex("1552f6db424acdef53728dbfab35b85266681af9f9c42fa60e30cc858da8eb1fe05437fea881290cdeaad317d0");

        assert_eq!(public_key_of(&sk_rm).expect("pk"), pk_rm);
        let pk = <Kem as KemTrait>::PublicKey::from_bytes(&pk_rm).expect("pk");
        let mut rng = FixedIkm(ikm_e.clone().try_into().expect("32"));
        let mode = OpModeS::Psk(PskBundle { psk: &psk, psk_id: &psk_id });
        let (enc, mut ctx) =
            hpke::setup_sender::<AesGcm256, HkdfSha256, Kem, _>(&mode, &pk, &info, &mut rng).expect("setup");
        assert_eq!(enc.to_bytes().to_vec(), enc_want, "ephemeral from ikmE");
        assert_eq!(ctx.seal(&pt, &aad).expect("seal"), ct_want, "sender ciphertext 0");

        let sk = <Kem as KemTrait>::PrivateKey::from_bytes(&sk_rm).expect("sk");
        let enc = <Kem as KemTrait>::EncappedKey::from_bytes(&enc_want).expect("enc");
        let mode = OpModeR::Psk(PskBundle { psk: &psk, psk_id: &psk_id });
        let mut rctx = hpke::setup_receiver::<AesGcm256, HkdfSha256, Kem>(&mode, &sk, &enc, &info).expect("setup r");
        assert_eq!(rctx.open(&ct_want, &aad).expect("open"), pt);
        // And the wrong PSK fails.
        let bad = [7u8; 32];
        let mode = OpModeR::Psk(PskBundle { psk: &bad, psk_id: &psk_id });
        let mut rctx = hpke::setup_receiver::<AesGcm256, HkdfSha256, Kem>(&mode, &sk, &enc, &info).expect("setup r");
        assert!(rctx.open(&ct_want, &aad).is_err(), "a wrong PSK must not open");
    }

    /// The relay refuses any other length (k2-connect `validate_sealed`):
    /// 2159 base64url chars for an alert, 452 for a nudge.
    #[test]
    fn blob_lengths_are_the_relay_contract() {
        assert_eq!(SealKind::Alert.blob_len(), 1619);
        assert_eq!(SealKind::Nudge.blob_len(), 339);
        assert_eq!(SealKind::Alert.b64_len(), 2159);
        assert_eq!(SealKind::Nudge.b64_len(), 452);
        let (sk, pk) = derive_keypair(&[9u8; 32]);
        let psk = [3u8; 32];
        for kind in [SealKind::Alert, SealKind::Nudge] {
            for json in [&b"{}"[..], &vec![b'x'; max_json(kind)][..]] {
                let b = seal(kind, &pk, &psk, json).expect("seal");
                assert_eq!(b.len(), kind.b64_len(), "{kind:?}");
                assert!(b.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'), "base64url, no padding");
                assert_eq!(open(kind, &sk, &psk, &b).expect("open"), json);
            }
        }
    }

    /// The relay's sealed bodies, byte for byte (k2-connect `apns.rs`
    /// `sealed_apns_body`), at their largest.
    fn relay_apns_body(kind: SealKind, format: Format) -> String {
        let blob = "A".repeat(format.b64_len(kind));
        let aps = match kind {
            SealKind::Alert => serde_json::json!({
                "alert": { "title": "K2", "body": "New message" },
                "sound": "default",
                "mutable-content": 1,
            }),
            SealKind::Nudge => serde_json::json!({ "content-available": 1 }),
        };
        serde_json::json!({ "aps": aps, "kind": kind.wire_kind(), "v": format.wire_version(), "e2e": blob }).to_string()
    }

    /// FCM counts the `data` keys and values against 4096 bytes
    /// (k2-connect `fcm.rs` `sealed_fcm_body`: `{kind, v, e2e}`).
    fn fcm_data_bytes(kind: SealKind, format: Format) -> usize {
        let pairs = [("kind", kind.wire_kind()), ("v", format.wire_version())];
        pairs.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() + "e2e".len() + format.b64_len(kind)
    }

    /// K2 (0.46.7): format 2's buckets are fixed, and the relay's whole
    /// APNs body and FCM data stay under 4096 bytes with base64 and the
    /// envelope. The alert bucket is the LARGEST 64-byte multiple that does
    /// (the next one is over), so most replies arrive whole.
    #[test]
    fn format_2_buckets_fit_the_vendor_caps_and_the_alert_one_is_the_largest() {
        assert_eq!(Format::V2.blob_len(SealKind::Alert), 2963);
        assert_eq!(Format::V2.b64_len(SealKind::Alert), 3951);
        assert_eq!(Format::V2.blob_len(SealKind::Nudge), 1107);
        assert_eq!(Format::V2.b64_len(SealKind::Nudge), 1476);
        // Format 1 is unchanged (the relay's s1 contract).
        assert_eq!(Format::V1.b64_len(SealKind::Alert), SealKind::Alert.b64_len());
        assert_eq!(Format::V1.b64_len(SealKind::Nudge), SealKind::Nudge.b64_len());
        assert_eq!(relay_apns_body(SealKind::Alert, Format::V1).len(), 2280, "today's alert push");
        let apns = relay_apns_body(SealKind::Alert, Format::V2).len();
        assert_eq!(apns, 4072, "the format-2 alert push");
        assert!(apns <= VENDOR_PAYLOAD_MAX);
        assert!(relay_apns_body(SealKind::Nudge, Format::V2).len() <= VENDOR_PAYLOAD_MAX);
        assert_eq!(fcm_data_bytes(SealKind::Alert, Format::V2), 3963);
        assert!(fcm_data_bytes(SealKind::Alert, Format::V2) <= VENDOR_PAYLOAD_MAX);
        // The next 64-byte bucket would not fit Apple's cap.
        let next = ALERT_PAD_V2 + 64;
        let next_b64 = ((HEADER_LEN + ENC_LEN + next + TAG_LEN) * 4).div_ceil(3);
        let over = apns - Format::V2.b64_len(SealKind::Alert) + next_b64;
        assert!(over > VENDOR_PAYLOAD_MAX, "{next} would still fit ({over} bytes): raise the bucket");
        // Real seals are exactly those lengths, open back, and say their format.
        let (sk, pk) = derive_keypair(&[8u8; 32]);
        let psk = [2u8; 32];
        for kind in [SealKind::Alert, SealKind::Nudge] {
            for json in [&b"{}"[..], &vec![b'z'; Format::V2.max_json(kind)][..]] {
                let b = seal_in(kind, Format::V2, &pk, &psk, json).expect("seal v2");
                assert_eq!(b.len(), Format::V2.b64_len(kind));
                let raw = B64.decode(&b).expect("b64");
                assert_eq!(raw[0], FORMAT_V2);
                assert_eq!(open_format(kind, &sk, &psk, &b).expect("open"), (Format::V2, json.to_vec()));
            }
            assert!(matches!(
                seal_in(kind, Format::V2, &pk, &psk, &vec![b'z'; Format::V2.max_json(kind) + 1]),
                Err(SealError::TooLarge { .. })
            ));
        }
        // The format byte is bound by the AAD: a v2 blob relabelled v1 fails.
        let b = seal_in(SealKind::Alert, Format::V2, &pk, &psk, b"{}").expect("seal");
        let mut raw = B64.decode(&b).expect("b64");
        raw[0] = FORMAT_V1;
        assert!(open(SealKind::Alert, &sk, &psk, &B64.encode(&raw)).is_err());
    }

    #[test]
    fn seal_open_round_trip_and_kind_binding() {
        let (sk, pk) = derive_keypair(&[1u8; 32]);
        let psk = [5u8; 32];
        let json = br#"{"v":1,"title":"Cortana","body":"hi \xf0\x9f\x9a\x80"}"#;
        let b = seal(SealKind::Alert, &pk, &psk, json).expect("seal");
        assert_eq!(open(SealKind::Alert, &sk, &psk, &b).expect("open"), json);
        // Two seals of the same plaintext differ (fresh ephemeral).
        assert_ne!(b, seal(SealKind::Alert, &pk, &psk, json).expect("seal 2"));
        // Wrong PSK, wrong key, or flipped kind all fail at the tag.
        assert!(matches!(open(SealKind::Alert, &sk, &[6u8; 32], &b), Err(SealError::Crypto(_))));
        let (sk2, _) = derive_keypair(&[2u8; 32]);
        assert!(matches!(open(SealKind::Alert, &sk2, &psk, &b), Err(SealError::Crypto(_))));
        let n = seal(SealKind::Nudge, &pk, &psk, b"{}").expect("nudge");
        assert!(open(SealKind::Alert, &sk, &psk, &n).is_err(), "a nudge is not an alert");
        // A flipped AAD byte fails too.
        let mut raw = B64.decode(&b).expect("b64");
        raw[1] ^= 0xff;
        let tampered = B64.encode(&raw);
        assert!(open(SealKind::Alert, &sk, &psk, &tampered).is_err());
        raw[1] ^= 0xff;
        let last = raw.len() - 1;
        raw[last] ^= 1;
        assert!(matches!(open(SealKind::Alert, &sk, &psk, &B64.encode(&raw)), Err(SealError::Crypto(_))));
    }

    #[test]
    fn padding_is_fixed_and_strict() {
        for kind in [SealKind::Alert, SealKind::Nudge] {
            let p = pad(b"{}", kind).expect("pad");
            assert_eq!(p.len(), kind.pad_len());
            assert_eq!(&p[..4], &[0, 2, b'{', b'}']);
            assert_eq!(unpad(&p).expect("unpad"), b"{}");
            assert_eq!(pad(&vec![b'a'; max_json(kind)], kind).expect("max").len(), kind.pad_len());
            assert_eq!(
                pad(&vec![b'a'; max_json(kind) + 1], kind),
                Err(SealError::TooLarge { len: max_json(kind) + 1, max: max_json(kind) })
            );
        }
        assert_eq!(ALERT_PAD, 1536);
        assert_eq!(NUDGE_PAD, 256);
        let mut p = pad(b"{}", SealKind::Nudge).expect("pad");
        p[100] = 1;
        assert!(unpad(&p).is_err(), "non-zero padding is refused");
        let mut p = pad(b"{}", SealKind::Nudge).expect("pad");
        p[0] = 0xff;
        assert!(unpad(&p).is_err(), "a length past the end is refused");
    }

    #[test]
    fn bad_keys_are_refused() {
        let (_, pk) = derive_keypair(&[1u8; 32]);
        assert!(check_public_key(&pk).is_ok());
        assert!(check_public_key(&pk[..64]).is_err());
        let mut off = pk.clone();
        off[64] ^= 1;
        assert!(check_public_key(&off).is_err(), "off the curve");
        let mut compressed = pk.clone();
        compressed[0] = 0x02;
        assert!(check_public_key(&compressed).is_err());
        assert!(seal(SealKind::Alert, &pk, &[1u8; 31], b"{}").is_err(), "short psk");
        assert_eq!(key_id(&pk).len(), 16);
    }
}
