//! The DSSE envelope and the node key's ECDSA P-256 signature over its
//! pre-authentication encoding (PAE).

use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;
use serde::{Deserialize, Serialize};

use crate::statement::Statement;
use crate::util::{is_fp, sha256_hex, unb64};

/// The only payload type a node signs.
pub const PAYLOAD_TYPE: &str = "application/vnd.in-toto+json";
/// Largest statement payload, in bytes.
pub const MAX_PAYLOAD_BYTES: usize = 384 * 1024;

/// A DSSE envelope: exactly `{payloadType, payload, signatures: [{keyid, sig}]}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    /// Must be [`PAYLOAD_TYPE`].
    #[serde(rename = "payloadType")]
    pub payload_type: String,
    /// The statement's canonical JSON, standard base64 with padding.
    pub payload: String,
    /// Exactly one.
    pub signatures: Vec<EnvelopeSignature>,
}

/// One DSSE signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeSignature {
    /// The node key's fingerprint: SHA-256 of its SubjectPublicKeyInfo DER, 64 hex.
    pub keyid: String,
    /// ECDSA P-256 SHA-256, ASN.1 DER, standard base64.
    pub sig: String,
}

/// DSSE's pre-authentication encoding:
/// `"DSSEv1" SP len(type) SP type SP len(payload) SP payload`.
pub fn pae(payload_type: &str, payload: &[u8]) -> Vec<u8> {
    let head = format!("DSSEv1 {} {payload_type} {} ", payload_type.len(), payload.len());
    let mut out = Vec::with_capacity(head.len() + payload.len());
    out.extend_from_slice(head.as_bytes());
    out.extend_from_slice(payload);
    out
}

/// The fixed DER header of a P-256 SubjectPublicKeyInfo: SEQUENCE {
/// SEQUENCE { id-ecPublicKey, prime256v1 }, BIT STRING (0 unused bits) }.
const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07,
    0x03, 0x42, 0x00,
];

/// A P-256 public key and its fingerprint.
#[derive(Debug, Clone)]
pub struct NodeKey {
    key: VerifyingKey,
    spki_der: Vec<u8>,
}

impl NodeKey {
    /// From SubjectPublicKeyInfo DER. Refuses anything but a P-256 key
    /// with an uncompressed point (91 bytes), the one form whose
    /// fingerprint names a node.
    pub fn from_spki_der(der: &[u8]) -> Result<Self, String> {
        if der.len() != 91 || der[..26] != P256_SPKI_PREFIX || der[26] != 0x04 {
            return Err("not a P-256 SubjectPublicKeyInfo with an uncompressed point".into());
        }
        let key = VerifyingKey::from_public_key_der(der).map_err(|e| format!("not a P-256 public key: {e}"))?;
        Ok(Self { key, spki_der: der.to_vec() })
    }

    /// From a PEM `PUBLIC KEY` block.
    pub fn from_pem(pem: &str) -> Result<Self, String> {
        let begin = "-----BEGIN PUBLIC KEY-----";
        let end = "-----END PUBLIC KEY-----";
        let start = pem.find(begin).ok_or("no PUBLIC KEY block")? + begin.len();
        let stop = pem[start..].find(end).ok_or("unterminated PUBLIC KEY block")? + start;
        let body: String = pem[start..stop].chars().filter(|c| !c.is_whitespace()).collect();
        Self::from_spki_der(&unb64(&body).map_err(|e| format!("PUBLIC KEY block: {e}"))?)
    }

    /// SHA-256 of the SPKI DER, 64 hex: the `keyid` and the roster's `node_fp`.
    pub fn fingerprint(&self) -> String {
        sha256_hex(&self.spki_der)
    }

    /// The SPKI DER.
    pub fn spki_der(&self) -> &[u8] {
        &self.spki_der
    }

    /// Verify a base64 DER signature over `msg`.
    pub fn verify(&self, msg: &[u8], sig_b64: &str) -> bool {
        let Ok(der) = unb64(sig_b64) else {
            return false;
        };
        let Ok(sig) = Signature::from_der(&der) else {
            return false;
        };
        self.key.verify(msg, &sig).is_ok()
    }
}

impl Envelope {
    /// Parse one `.intoto.jsonl` line (or a whole file holding one envelope).
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text.trim()).map_err(|e| format!("not a DSSE envelope: {e}"))
    }

    /// The envelope's shape: the payload type, exactly one signature, a
    /// 64-hex key id.
    pub fn check_shape(&self) -> Result<&EnvelopeSignature, String> {
        if self.payload_type != PAYLOAD_TYPE {
            return Err(format!("payloadType is {:?}, not {PAYLOAD_TYPE}", self.payload_type));
        }
        let [sig] = self.signatures.as_slice() else {
            return Err(format!("the envelope carries {} signatures; a node signs exactly one", self.signatures.len()));
        };
        if !is_fp(&sig.keyid) {
            return Err("the signature's keyid is not 64 lowercase hex".into());
        }
        Ok(sig)
    }

    /// The signer's key id. Trust it only after [`Envelope::open`].
    pub fn keyid(&self) -> Option<&str> {
        match self.signatures.as_slice() {
            [s] => Some(&s.keyid),
            _ => None,
        }
    }

    /// Verify with `key`, then parse the statement. Refuses another payload
    /// type, not exactly one signature, a keyid that isn't `key`'s
    /// fingerprint, a bad signature, a payload over [`MAX_PAYLOAD_BYTES`]
    /// or holding a zero byte, unknown fields, a payload that is not its
    /// canonical form, and a statement that breaks [`Statement::check`].
    pub fn open(&self, key: &NodeKey) -> Result<Statement, String> {
        let sig = self.check_shape()?;
        if sig.keyid != key.fingerprint() {
            return Err("the signature's keyid is not this node key's fingerprint".into());
        }
        if self.payload.len() > MAX_PAYLOAD_BYTES.div_ceil(3) * 4 {
            return Err(format!("the payload is over {MAX_PAYLOAD_BYTES} bytes"));
        }
        let body = unb64(&self.payload).map_err(|e| format!("payload: {e}"))?;
        if !key.verify(&pae(PAYLOAD_TYPE, &body), &sig.sig) {
            return Err("the signature does not verify with that node key".into());
        }
        parse_payload(&body)
    }

    /// The `.intoto.jsonl` line: the envelope on one line, then `\n`.
    pub fn line(&self) -> Result<String, String> {
        Ok(format!("{}\n", serde_json::to_string(self).map_err(|e| e.to_string())?))
    }
}

/// Parse a payload: no zero byte, at most [`MAX_PAYLOAD_BYTES`], unknown
/// fields refused, its own canonical form, the shape rules.
pub fn parse_payload(body: &[u8]) -> Result<Statement, String> {
    if body.contains(&0) {
        return Err("the payload holds a zero byte".into());
    }
    if body.len() > MAX_PAYLOAD_BYTES {
        return Err(format!("the payload is over {MAX_PAYLOAD_BYTES} bytes"));
    }
    let st: Statement = crate::canonical::parse_exact("statement", body)?;
    st.check().map_err(|e| format!("statement: {e}"))?;
    Ok(st)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pae_layout_is_the_dsse_spec() {
        assert_eq!(pae("a/b", b"{}"), b"DSSEv1 3 a/b 2 {}".to_vec());
        assert_eq!(pae("", b""), b"DSSEv1 0  0 ".to_vec());
    }

    #[test]
    fn a_non_p256_key_is_refused() {
        assert!(NodeKey::from_spki_der(&[0x30, 0x03, 0x02, 0x01, 0x00]).is_err());
        assert!(NodeKey::from_pem("-----BEGIN PUBLIC KEY-----\n-----END PUBLIC KEY-----\n").is_err());
    }

    #[test]
    fn envelope_shape_rules() {
        let sig = EnvelopeSignature { keyid: "a".repeat(64), sig: "AAAA".into() };
        let mut env = Envelope { payload_type: PAYLOAD_TYPE.into(), payload: String::new(), signatures: vec![sig.clone()] };
        assert!(env.check_shape().is_ok());
        env.signatures.push(sig.clone());
        assert!(env.check_shape().unwrap_err().contains("2 signatures"));
        env.signatures = vec![EnvelopeSignature { keyid: "A".repeat(64), sig: "AAAA".into() }];
        assert!(env.check_shape().is_err());
        env.signatures = vec![sig];
        env.payload_type = "application/json".into();
        assert!(env.check_shape().is_err());
        assert!(Envelope::from_json(r#"{"payloadType":"x","payload":"","signatures":[],"extra":1}"#).is_err());
    }
}
