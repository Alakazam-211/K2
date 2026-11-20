//! `base64.b64encode(data).decode("ascii")`: the standard alphabet, `=`
//! padding, no line breaks.
//!
//! Several families carry their own copy of this (written before it was
//! shared); new code uses this one.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `base64.b64encode(data).decode("ascii")`.
pub fn b64encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for (k, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if k <= c.len() {
                out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_python_writes_it() {
        for (raw, want) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob\xff", "Zm9vYv8="),
            (b"\xff\x00\xfe\x01", "/wD+AQ=="),
        ] {
            assert_eq!(b64encode(raw), want, "{raw:?}");
        }
    }
}
