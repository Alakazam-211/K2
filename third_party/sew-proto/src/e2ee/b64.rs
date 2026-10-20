//! `B64`: base64url without padding (RFC 4648 §5), the wire form of every MLS blob and key in
//! `sew_proto::e2ee` (prd-sew-50 §9). Hand-written: `sew-proto` takes no codec dependency
//! (rev 26 G19).
//!
//! Decoding is strict: no padding, no characters outside the URL-safe alphabet, no length that
//! is 1 mod 4, and the unused low bits of the last character must be zero (so every byte string
//! has exactly one accepted encoding).

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Why a base64url string was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum B64Error {
    /// A character outside `A–Z a–z 0–9 - _` (padding `=` included).
    BadChar,
    /// The length is 1 mod 4: no byte string encodes to it.
    BadLength,
    /// The unused low bits of the last character are not zero.
    NonCanonical,
}

impl fmt::Display for B64Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            B64Error::BadChar => "invalid base64url character (padding is not allowed)",
            B64Error::BadLength => "invalid base64url length",
            B64Error::NonCanonical => "non-canonical base64url (trailing bits set)",
        })
    }
}

impl std::error::Error for B64Error {}

/// Encodes `bytes` as base64url without padding.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        let chars = chunk.len() + 1;
        for i in 0..chars {
            let idx = (n >> (18 - 6 * i)) & 0x3f;
            out.push(char::from(ALPHABET[idx as usize]));
        }
    }
    out
}

fn value(c: u8) -> Result<u32, B64Error> {
    Ok(u32::from(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'-' => 62,
        b'_' => 63,
        _ => return Err(B64Error::BadChar),
    }))
}

/// Decodes strict base64url without padding.
pub fn decode(s: &str) -> Result<Vec<u8>, B64Error> {
    let raw = s.as_bytes();
    if raw.len() % 4 == 1 {
        return Err(B64Error::BadLength);
    }
    let mut out = Vec::with_capacity(raw.len() / 4 * 3 + 2);
    for chunk in raw.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value(c)? << (18 - 6 * i);
        }
        let bytes = chunk.len() - 1;
        // Bits below the last whole byte must be zero.
        let used_bits = 8 * bytes as u32;
        let unused = 24 - used_bits;
        if chunk.len() < 4 && n & ((1u32 << unused) - 1) != 0 {
            return Err(B64Error::NonCanonical);
        }
        for i in 0..bytes {
            out.push(((n >> (16 - 8 * i)) & 0xff) as u8);
        }
    }
    Ok(out)
}

/// A byte string carried as base64url without padding.
#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub struct B64(pub Vec<u8>);

impl B64 {
    /// Wraps bytes.
    pub fn new(bytes: impl Into<Vec<u8>>) -> B64 {
        B64(bytes.into())
    }

    /// The bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The bytes, owned.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    /// The decoded length.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The wire string.
    pub fn encoded(&self) -> String {
        encode(&self.0)
    }

    /// Parses a wire string.
    pub fn parse(s: &str) -> Result<B64, B64Error> {
        decode(s).map(B64)
    }
}

impl fmt::Debug for B64 {
    /// Sizes only: blobs and keys never reach logs (prd-sew-50 §19.5).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "B64({} bytes)", self.0.len())
    }
}

impl From<Vec<u8>> for B64 {
    fn from(v: Vec<u8>) -> B64 {
        B64(v)
    }
}

impl From<&[u8]> for B64 {
    fn from(v: &[u8]) -> B64 {
        B64(v.to_vec())
    }
}

impl Serialize for B64 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for B64 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<B64, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = B64;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a base64url string without padding")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<B64, E> {
                decode(v).map(B64).map_err(E::custom)
            }
        }
        d.deserialize_str(V)
    }
}

impl JsonSchema for B64 {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "B64".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^[A-Za-z0-9_-]*$",
            "description": "Bytes as base64url without padding (RFC 4648 §5); canonical trailing bits."
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4648_vectors_url_safe_unpadded() {
        let cases: &[(&[u8], &str)] = &[
            (b"", ""),
            (b"f", "Zg"),
            (b"fo", "Zm8"),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg"),
            (b"fooba", "Zm9vYmE"),
            (b"foobar", "Zm9vYmFy"),
            (&[0xfb, 0xff], "-_8"),
        ];
        for (raw, enc) in cases {
            assert_eq!(encode(raw), *enc);
            assert_eq!(decode(enc).as_deref(), Ok(*raw));
        }
    }

    #[test]
    fn strict_decoding() {
        assert_eq!(decode("Zg=="), Err(B64Error::BadChar));
        assert_eq!(decode("Zm9v+A"), Err(B64Error::BadChar));
        assert_eq!(decode("Z"), Err(B64Error::BadLength));
        assert_eq!(decode("Zh"), Err(B64Error::NonCanonical));
        assert_eq!(decode("Zm9"), Err(B64Error::NonCanonical));
    }

    #[test]
    fn every_length_round_trips() {
        for n in 0..70u8 {
            let v: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37).wrapping_add(n)).collect();
            assert_eq!(decode(&encode(&v)), Ok(v));
        }
    }
}
