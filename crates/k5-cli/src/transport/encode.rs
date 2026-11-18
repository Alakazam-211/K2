//! Query string escaping that writes the same bytes as Python 3.
//!
//! The shell CLI escapes every value it puts in a query string with
//! `urlencode` in `cli/src/cli-tools-policy.sh`:
//!
//! ```text
//! urlencode() {
//!     python3 -c "import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=''))" "$1" 2>/dev/null || echo "$1"
//! }
//! ```
//!
//! [`quote`] is `urllib.parse.quote(s, safe='')`: the text is encoded as
//! UTF-8, ASCII letters, digits and `_ . - ~` stay as they are, and every
//! other byte becomes `%XX` with upper-case hex. So `/`, space, `+`, `&`,
//! `=`, `%`, `?`, `#`, controls, DEL and all non-ASCII are escaped, and a
//! space is `%20`, never `+`.
//!
//! What the shell wrapper adds around Python, and what is kept here:
//!
//! - The value is an argv element, so it cannot hold a NUL. [`quote`] takes
//!   any `&str` and escapes a NUL as `%00`, as Python's `quote` does.
//! - `print` adds a newline and `$(...)` strips it again. The escaped text
//!   never ends in a newline (a newline is `%0A`), so the result is the same
//!   as Python's return value and nothing more is needed here.
//! - When Python fails, the wrapper falls back to `echo "$1"`, the raw value.
//!   That is NOT reproduced. Python cannot fail on a valid UTF-8 argument,
//!   and a `&str` is always valid UTF-8. An argv element with bytes that are
//!   not UTF-8 does make Python fail (it reads them as lone surrogates, which
//!   UTF-8 cannot encode), and the shell then puts the raw bytes in the query
//!   string. [`quote_bytes`] escapes such bytes instead, as Python's `quote`
//!   does when it is given `bytes`.

/// Upper-case hex digits, as Python writes them.
const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// `urllib.parse.quote(s, safe='')`: `s` as UTF-8 with every byte escaped
/// except ASCII letters, digits and `_ . - ~`.
pub fn quote(s: &str) -> String {
    quote_bytes(s.as_bytes())
}

/// `urllib.parse.quote(b, safe='')` for a `bytes` value: the same rule as
/// [`quote`], for bytes that need not be UTF-8 (an argv element on Unix can
/// be any bytes but NUL).
pub fn quote_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &byte in bytes {
        if is_unreserved(byte) {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    out
}

/// `urllib.parse.urlencode(pairs)` (its `quote_plus`, `safe=''`): each
/// key and value as [`quote`] escapes it, but a space is `+`; `key=value`
/// joined by `&`.
pub fn urlencode_plus(pairs: &[(&str, &str)]) -> String {
    let plus = |s: &str| s.split(' ').map(quote).collect::<Vec<_>>().join("+");
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", plus(k), plus(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// The bytes Python's `quote` never escapes (RFC 3986 unreserved).
fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'~')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Python 3's `urllib.parse.urlencode` returned for each.
    #[test]
    fn urlencode_is_python_s() {
        assert_eq!(
            urlencode_plus(&[("id", "a b/~*é"), ("brief", "1"), ("token", "t.x")]),
            "id=a+b%2F~%2A%C3%A9&brief=1&token=t.x"
        );
        assert_eq!(
            urlencode_plus(&[("id", "  +&=%"), ("token", "")]),
            "id=++%2B%26%3D%25&token="
        );
        assert_eq!(
            urlencode_plus(&[("id", "日本 x\n")]),
            "id=%E6%97%A5%E6%9C%AC+x%0A"
        );
    }
}
