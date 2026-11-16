//! Native addresses: how a scene's previews, its objects and its cameras are
//! named on the wire, in the multiplayer kit's own grammar, so the address a
//! still is listed under is also a presence view address (what Follow and an
//! agent's still set).
//!
//! **The rule** mirrors the kit's address parser (`addr::parse`, its native
//! ID form): `@` then one or more `/`-separated parts, each `<kind>:<id>`
//! split at the part's first `:`; the kind matches `[a-z][a-z0-9_-]*` and
//! isn't `k` (`@k:` is the kit's minted-key form); the id is non-empty with
//! no whitespace. This crate doesn't link the kit: [`is_native`] is the
//! rule, and its tests are the kit's own cases.
//!
//! **Ids in addresses** ([`enc`], [`dec`]): a file's object or camera name
//! goes in an address percent-encoded, `%XX` (uppercase hex) of each UTF-8
//! byte of `%`, `/`, every Unicode whitespace character and every control
//! character; everything else, non-ASCII letters included, stays as it is.
//! So any non-empty name makes a native id, and [`dec`] gives it back
//! exactly.

/// The scene view's address (the auto view of everything).
pub const SCENE: &str = "@scene:main";

/// Whether `k` is a native kind: `[a-z][a-z0-9_-]*`, and not `k`.
pub fn kind_ok(k: &str) -> bool {
    let mut b = k.bytes();
    k != "k" && b.next().is_some_and(|c| c.is_ascii_lowercase()) && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

/// The parts of `s` as `(kind, id)` when it is a native address under the
/// kit's rule (see the module docs), else `None`.
pub fn native_parts(s: &str) -> Option<Vec<(&str, &str)>> {
    let rest = s.strip_prefix('@')?;
    rest.split('/')
        .map(|part| {
            let (k, id) = part.split_once(':')?;
            (kind_ok(k) && !id.is_empty() && !id.chars().any(char::is_whitespace)).then_some((k, id))
        })
        .collect()
}

/// Whether `s` parses as a native address under the kit's rule.
pub fn is_native(s: &str) -> bool {
    native_parts(s).is_some()
}

/// `s`'s kind and id when it is a native address of one part.
pub fn single(s: &str) -> Option<(&str, &str)> {
    match native_parts(s)?.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

fn escaped(c: char) -> bool {
    c == '%' || c == '/' || c.is_whitespace() || c.is_control()
}

/// A name as an address's id: `%`, `/`, whitespace and control characters
/// as `%XX` of their UTF-8 bytes (uppercase hex), the rest as it is.
pub fn enc(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for c in id.chars() {
        if escaped(c) {
            let mut b = [0u8; 4];
            for byte in c.encode_utf8(&mut b).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// [`enc`] reversed: every `%XX` back to its byte. `None` when a `%` isn't
/// followed by two hex digits or the bytes aren't UTF-8.
pub fn dec(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            if !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kit parser's own cases for its native form: what it reads as a
    /// native ID, and what it refuses or reads as another form.
    #[test]
    fn the_kits_native_rule() {
        assert_eq!(native_parts("@sld:256/sp:4"), Some(vec![("sld", "256"), ("sp", "4")]));
        assert_eq!(native_parts("@lyid:42"), Some(vec![("lyid", "42")]));
        for s in ["@obj:1", "@cam:26", "@obj-n:0", "@scene:main", "@turntable:7", "@obj:a:b", "@obj:Caf\u{e9}", "@a_b-c9:x"] {
            assert!(is_native(s), "{s}");
        }
        // Malformed in the kit, or another form there (a key, a position, a word).
        for s in ["", "@k:", "@:1", "@Sld:1", "@k:1/x:2", "@sld:", "@k:r17", "key:c3", "#3", "L12", "name:city", "city", "@timestamp", "@9a:1", "@-a:1", "@obj:a b", "@obj:a\tb", "@obj:a\u{3000}b", "@obj:x/", "@/obj:x", "obj:1", "@obj"] {
            assert!(!is_native(s), "{s:?}");
        }
        assert_eq!(single("@obj:12"), Some(("obj", "12")));
        assert_eq!(single("@sld:256/sp:4"), None);
        assert!(!kind_ok("k") && kind_ok("ka") && !kind_ok("") && !kind_ok("A"));
    }

    #[test]
    fn names_round_trip_and_always_make_a_native_id() {
        let names = ["Cube", "Café Tisch", "ナイフ", "50% off", "a/b", "x:y", "tab\there", "line\nbreak", " lead", "trail ", "nbsp\u{a0}x", "ideographic\u{3000}space", "%41", "100%", "@obj:7", "😀/😀", "\u{7f}del", "Item.000"];
        for n in names {
            let e = enc(n);
            assert_eq!(dec(&e).as_deref(), Some(n), "{n:?} -> {e:?}");
            assert!(is_native(&format!("@obj:{e}")), "{n:?} -> {e:?}");
        }
        assert_eq!(enc("Café Tisch"), "Café%20Tisch");
        assert_eq!(enc("ナイフ"), "ナイフ");
        assert_eq!(enc("50% off"), "50%25%20off");
        assert_eq!(enc("a/b"), "a%2Fb");
        assert_eq!(enc("tab\there"), "tab%09here");
        assert_eq!(enc("nbsp\u{a0}x"), "nbsp%C2%A0x");
        assert_eq!(enc(""), "");
        for bad in ["%", "%4", "%zz", "%C3", "a%2"] {
            assert_eq!(dec(bad), None, "{bad:?}");
        }
    }
}
