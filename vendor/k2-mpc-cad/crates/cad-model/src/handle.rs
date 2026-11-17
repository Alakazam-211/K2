//! Handles: a DXF object's own id, hexadecimal. Normalised: leading zeros dropped, upper case.
//! `1F`, `01F` and `1f` are one handle; the file's own spelling is kept in its bytes.

/// The handle a value holds: 1 to 16 hexadecimal digits, spaces around it ignored. `None` for
/// anything else.
pub fn parse(v: &[u8]) -> Option<u64> {
    let s = std::str::from_utf8(v).ok()?.trim();
    if s.is_empty() || s.len() > 16 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(s, 16).ok()
}

/// The normalised spelling: upper case, no leading zeros (`0` for zero).
pub fn norm(h: u64) -> String {
    format!("{h:X}")
}

/// The address and index key of the object with handle `h`: `@h:<HANDLE>`.
pub fn key(h: u64) -> String {
    if crate::mutant::active("HandleNotNormalised") {
        return format!("@h:{h:06x}");
    }
    format!("@h:{}", norm(h))
}

/// The handle an address names: `@h:<hex>` (any case, leading zeros allowed).
pub fn from_addr(a: &str) -> Option<u64> {
    a.strip_prefix("@h:").and_then(|h| parse(h.as_bytes()))
}
