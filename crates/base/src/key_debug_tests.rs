//! No kit type that holds key material prints it in `Debug`. One row per type with a `Debug`
//! and a key: a blob space's key here, and under feature `native` the engine's two keys and
//! the kit types that carry them. Each key is built from known bytes; every spelling of them
//! that a `Debug` could print is looked for, and then the output is held to the exact layout,
//! so a spelling this file doesn't know still fails the row.

use crate::store::SpaceKey;
use std::fmt::Debug;

/// 32 distinct bytes, all at 0x80 or above: a byte is then always three decimal digits, two
/// hex, three octal and eight binary, so a run of bytes has one spelling in each radix.
const fn known_key(start: u8, step: u8) -> [u8; 32] {
    let mut k = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        k[i] = 0x80 | (start.wrapping_add(step.wrapping_mul(i as u8)) & 0x7f);
        i += 1;
    }
    k
}

const DIGEST: [u8; 32] = known_key(11, 37);
const SECRET: [u8; 32] = known_key(90, 53);

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const BASE64_URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn base64(bytes: &[u8], alphabet: &[u8; 64]) -> String {
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let b = |i: usize| u32::from(c.get(i).copied().unwrap_or(0));
        let n = (b(0) << 16) | (b(1) << 8) | b(2);
        for i in 0..=c.len() {
            out.push(alphabet[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

fn run(bytes: &[u8], spell: impl Fn(u8) -> String) -> String {
    bytes.iter().map(|b| spell(*b)).collect()
}

/// The printed text without case, punctuation, spacing or radix prefixes: a list, a spaced or
/// colon-separated run, `0xd1` and `\xd1` all read as one run of digits.
fn squash(printed: &str) -> String {
    printed
        .to_lowercase()
        .replace("0x", "")
        .replace("\\x", "")
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

/// The spelling of `key` that `printed` holds, if any. Any 4 bytes in a row count, so a
/// printed prefix or fragment is found as well as the whole key.
fn leak(printed: &str, key: &[u8; 32]) -> Option<String> {
    let flat = squash(printed);
    for w in key.windows(4) {
        let spellings = [
            ("decimal", run(w, |b| b.to_string())),
            ("hex", run(w, |b| format!("{b:02x}"))),
            ("octal", run(w, |b| format!("{b:o}"))),
            ("binary", run(w, |b| format!("{b:08b}"))),
        ];
        for (form, needle) in spellings {
            if flat.contains(&needle) {
                return Some(format!("{form} {needle}"));
            }
        }
    }
    for (form, alphabet) in [("base64", BASE64), ("base64url", BASE64_URL)] {
        let encoded = base64(key, alphabet);
        for w in encoded.as_bytes().windows(8) {
            let needle = std::str::from_utf8(w).unwrap();
            if printed.contains(needle) {
                return Some(format!("{form} {needle}"));
            }
        }
    }
    None
}

/// `value`'s `Debug`, plain and pretty: no spelling of any of `keys`, and exactly `layout`.
fn assert_no_key_bytes(value: &dyn Debug, layout: &str, keys: &[&[u8; 32]]) {
    for printed in [format!("{value:?}"), format!("{value:#?}")] {
        for key in keys {
            let found = leak(&printed, key);
            assert!(found.is_none(), "key bytes ({found:?}) in `{printed}`");
        }
        assert_eq!(printed, layout, "a `Debug` of a key holder changed layout");
    }
}

/// The search itself: every spelling it claims is found, whole and in part, and nothing else
/// is. Without this a row could pass because its needles are wrong.
#[test]
fn the_key_byte_search_finds_every_spelling() {
    let k = &DIGEST;
    let part = &k[5..9];
    let hex_list: Vec<String> = k.iter().map(|b| format!("{b:02x}")).collect();
    let plants = [
        ("decimal list", format!("{k:?}")),
        ("pretty decimal list", format!("{k:#?}")),
        ("hex list", format!("{k:x?}")),
        ("upper hex list", format!("{k:X?}")),
        ("pretty hex list", format!("{k:#x?}")),
        ("hex run", hex_list.concat()),
        ("colon hex", hex_list.join(":")),
        ("spaced hex", hex_list.join(" ")),
        ("0x hex", format!("0x{}", hex_list.join(", 0x"))),
        ("escaped bytes", run(k, |b| format!("\\x{b:02x}"))),
        ("octal run", run(k, |b| format!("{b:o}"))),
        ("binary run", run(k, |b| format!("{b:08b}"))),
        ("base64", base64(k, BASE64)),
        ("base64url", base64(k, BASE64_URL)),
        ("a fragment in decimal", format!("{part:?}")),
        ("a fragment in hex", run(part, |b| format!("{b:02x}"))),
    ];
    for (name, printed) in &plants {
        assert!(leak(printed, k).is_some(), "{name} not found in {printed}");
    }
    assert_eq!(leak("SpaceKey { no: 7, key: <secret> }", k), None);
    assert_eq!(leak(&format!("{SECRET:?}"), k), None, "another key's bytes");
}

/// A blob space's key (`store::blob`): the generation number prints, the key does not.
#[test]
fn space_key_debug_prints_no_key_bytes() {
    let k = SpaceKey { no: 7, key: DIGEST };
    assert_no_key_bytes(&k, "SpaceKey { no: 7, key: <secret> }", &[&DIGEST]);
}
