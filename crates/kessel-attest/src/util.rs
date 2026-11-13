//! Small shared pieces: base64, hex, SHA-256, the time form statements use,
//! and the byte layout of the owner's and the node's domain signatures.

use base64::Engine;
use sha2::{Digest, Sha256};

/// Standard base64 with padding.
pub fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Decode standard base64 with padding. Refuses any other form (no
/// whitespace, no URL alphabet, no missing padding), so one value has one
/// spelling.
pub fn unb64(s: &str) -> Result<Vec<u8>, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(s.as_bytes())
        .map_err(|e| format!("base64: {e}"))?;
    if b64(&bytes) != s {
        return Err("base64: not standard padded base64".into());
    }
    Ok(bytes)
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// SHA-256 as 64 lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// SHA-256 of a file, streamed, as 64 lowercase hex.
pub fn sha256_file(path: &std::path::Path) -> Result<String, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// 64 lowercase hex (a SHA-256 or a key fingerprint).
pub fn is_fp(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// 40 lowercase hex (a git commit or tree).
pub fn is_git_hex(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `sha256:<64 hex>`, how signed documents name each other.
pub fn is_digest(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(is_fp)
}

/// `sha256:<hex>` of a body's bytes.
pub fn digest(body: &str) -> String {
    format!("sha256:{}", sha256_hex(body.as_bytes()))
}

/// The bytes a domain-separated signature covers: `domain`, a zero byte,
/// then each part as a u32 big-endian length and its bytes. The owner's
/// roster and the node's self-statement are signed this way.
pub fn signed_bytes(domain: &str, parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(domain.len() + 1 + parts.iter().map(|p| p.len() + 4).sum::<usize>());
    out.extend_from_slice(domain.as_bytes());
    out.push(0);
    for p in parts {
        out.extend_from_slice(&(p.len() as u32).to_be_bytes());
        out.extend_from_slice(p);
    }
    out
}

/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`. Years 0001–9999.
pub fn rfc3339(secs: i64) -> Result<String, String> {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    if !(1..=9999).contains(&y) {
        return Err(format!("time {secs} is outside years 1-9999"));
    }
    Ok(format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60))
}

/// The inverse of [`rfc3339`]: exactly `YYYY-MM-DDTHH:MM:SSZ`, a real date.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r).filter(|t| t.bytes().all(|c| c.is_ascii_digit()))?.parse().ok() };
    let (y, m, d, h, mi, sec) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    if y == 0 || !(1..=12).contains(&m) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    // Days from civil (Howard Hinnant).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec;
    // 2027-02-31 would roll over into March: refuse it.
    (rfc3339(secs).ok()? == s).then_some(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_round_trips_and_refuses_other_forms() {
        for t in [0, 1_800_000_000, 951_782_400 /* 2000-02-29 */, -86_400] {
            assert_eq!(parse_rfc3339(&rfc3339(t).unwrap()), Some(t));
        }
        assert_eq!(rfc3339(1_800_000_000).unwrap(), "2027-01-15T08:00:00Z");
        for bad in ["2027-01-15T08:00:00", "2027-01-15 08:00:00Z", "2027-02-31T00:00:00Z", "2027-01-15T24:00:00Z", "0000-01-01T00:00:00Z", "2027-1-15T08:00:00Z", ""] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }

    #[test]
    fn base64_has_one_spelling() {
        assert_eq!(unb64("YWJj").unwrap(), b"abc");
        assert!(unb64("YWI").is_err(), "missing padding");
        assert!(unb64(" YWJj").is_err(), "whitespace");
        assert!(unb64("YW-j").is_err(), "URL alphabet");
    }

    #[test]
    fn signed_bytes_layout_is_pinned() {
        assert_eq!(signed_bytes("x", &[b"ab", b""]), vec![b'x', 0, 0, 0, 0, 2, b'a', b'b', 0, 0, 0, 0]);
    }
}
