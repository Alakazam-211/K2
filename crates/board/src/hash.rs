//! sha256 values, as the format writes them.

use crate::refusal::{Refusal, RefusalCode};
use sha2::{Digest, Sha256 as Hasher};
use std::fmt;

/// A sha256 hash. `Display` gives `sha256-<64 lower-case hex>`, the form a blob reference uses.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256([u8; 32]);

impl Sha256 {
    /// The hash of these bytes.
    pub fn of(bytes: &[u8]) -> Sha256 {
        let digest = Hasher::digest(bytes);
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        Sha256(out)
    }

    /// The raw hash bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// 64 lower-case hex digits.
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }

    /// Reads 64 lower-case hex digits.
    pub fn from_hex(s: &str) -> Option<Sha256> {
        let mut out = [0u8; 32];
        unhex(s, &mut out)?;
        Some(Sha256(out))
    }

    /// Reads `sha256-<64 lower-case hex>`. Anything else is `field_value`.
    pub fn parse(s: &str) -> Result<Sha256, Refusal> {
        s.strip_prefix("sha256-")
            .and_then(Sha256::from_hex)
            .ok_or_else(|| Refusal::new(RefusalCode::FieldValue, "not sha256-<64 lower hex>"))
    }

    /// The zip entry name of a blob with this hash.
    pub fn blob_name(&self) -> String {
        format!("blobs/{self}")
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256-{}", self.to_hex())
    }
}

impl fmt::Debug for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 15) as usize] as char);
    }
    s
}

pub(crate) fn unhex(s: &str, out: &mut [u8]) -> Option<()> {
    let b = s.as_bytes();
    if b.len() != out.len() * 2 {
        return None;
    }
    fn digit(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        }
    }
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = digit(b[i * 2])? << 4 | digit(b[i * 2 + 1])?;
    }
    Some(())
}
