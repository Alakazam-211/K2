//! Key and prefix syntax (contract §3.6, PRD §5.3). C0 → B1.
//!
//! A key is 1–256 bytes of `[A-Za-z0-9._~:@+/-]`, with no leading `/`, no
//! `..` segment and no empty segment. A prefix follows the same rules but
//! may be empty (the whole space) and may end in `/`.

use super::{DataError, MAX_KEY_BYTES};

fn allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || "._~:@+/-".contains(c)
}

/// Check one key.
pub fn validate_key(key: &str) -> Result<(), DataError> {
    if key.is_empty() || key.len() > MAX_KEY_BYTES {
        return Err(DataError::BadKey(format!("a key is 1–{MAX_KEY_BYTES} bytes")));
    }
    check_shape(key, false)
}

/// Check a grant / list / changes prefix (`''` = the whole space).
pub fn validate_prefix(prefix: &str) -> Result<(), DataError> {
    if prefix.is_empty() {
        return Ok(());
    }
    if prefix.len() > MAX_KEY_BYTES {
        return Err(DataError::BadKey(format!("a prefix is at most {MAX_KEY_BYTES} bytes")));
    }
    check_shape(prefix, true)
}

fn check_shape(s: &str, trailing_slash_ok: bool) -> Result<(), DataError> {
    if let Some(c) = s.chars().find(|c| !allowed(*c)) {
        return Err(DataError::BadKey(format!("'{c}' isn't allowed in a key (letters, digits and . _ ~ : @ + / -)")));
    }
    if s.starts_with('/') {
        return Err(DataError::BadKey("a key never starts with /".into()));
    }
    let body = if trailing_slash_ok { s.strip_suffix('/').unwrap_or(s) } else { s };
    for seg in body.split('/') {
        if seg.is_empty() {
            return Err(DataError::BadKey("a key has no empty segment (//, or a trailing /)".into()));
        }
        if seg == ".." {
            return Err(DataError::BadKey("a key has no .. segment".into()));
        }
    }
    Ok(())
}

/// True when `key` is under `prefix` (`''` covers every key). Plain string
/// prefixes; no wildcards (PRD §5.5.3).
pub fn under(prefix: &str, key: &str) -> bool {
    key.starts_with(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_follow_the_contract_syntax() {
        for ok in ["obj/7", "log/41", "project/7/board/2/obj/9", "a.b~c:d@e+f-g_h", &"k".repeat(256)] {
            validate_key(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in ["", "/obj", "obj//7", "obj/", "obj/../x", "..", "obj 7", "obj/ä", &"k".repeat(257)] {
            assert!(validate_key(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn prefixes_may_be_empty_or_end_in_a_slash() {
        for ok in ["", "project/7/", "obj", "board/q4/"] {
            validate_prefix(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in ["/", "/x/", "a//", "../", "a/../"] {
            assert!(validate_prefix(bad).is_err(), "{bad:?} must be refused");
        }
        assert!(under("", "anything"));
        assert!(under("project/7/", "project/7/board/1"));
        assert!(!under("project/7/", "project/70/board/1"));
    }
}
