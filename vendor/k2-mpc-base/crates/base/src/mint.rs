//! Tagged-key minting: list keys that two writers can never mint alike.
//!
//! A shared counter (`<prefix><n>`, as the table's `r<n>`/`c<n>`) is a facet every insert
//! writes, so concurrent inserts conflict on it. A **tagged key** carries its writer:
//! `<prefix><n>.<tag>`, where
//! - `prefix` is 1 to [`MAX_PREFIX_LEN`] bytes of `[a-z]`, chosen by the pack;
//! - `n` is a decimal number, written without leading zeros;
//! - `tag` is the writer's [`WriterTag`]: the lower-case base 36 of its [`ClientId`].
//!
//! A `ClientId` has exactly one writer (ARCHITECTURE rule 6), so two writers never mint one
//! key, and a writer's next `n` depends only on the keys it can see ([`next_for`]): no written
//! facet is shared. The whole key must fit a list [`Key`] (24 bytes): a 3-byte prefix and the
//! longest tag (11 bytes, for [`crate::doc::MAX_KIT_CLIENT`]) leave 9 digits for `n`; [`TaggedKey::to_key`]
//! refuses a key past that ([`MintError::TooLong`]).
//!
//! **Retired marks.** A drop forgets its key, so the dropper records the highest number it
//! dropped per prefix and tag under its **own** mark key ([`mark_key`], one per dropper, in a
//! pack's marks Map), and [`retired_mark`] reads the max over every dropper's: monotone under
//! any concurrency. A mint only reads marks. Marks are written by the keyed drop (slice 6a);
//! [`marks_for_drops`] gives the writes.
//!
//! [`KeyShape`] is the looser grammar underneath: `<prefix><n>` or `<prefix><n>.<anything>`.
//! The table counts its row and column keys with it, tagged or not.

use std::fmt;

use crate::doc::{ClientId, MAX_CLIENT_ID};
use crate::elem::{Key, MapOp, Value, MAX_KEY_LEN};

/// Longest key prefix, in bytes.
pub const MAX_PREFIX_LEN: usize = 3;

/// Longest [`WriterTag`], in bytes: base 36 of [`crate::doc::MAX_KIT_CLIENT`] (`2gosa7pa2gu`).
pub const MAX_TAG_LEN: usize = 11;

/// Why a key can't be minted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MintError {
    /// The prefix isn't 1 to [`MAX_PREFIX_LEN`] bytes of `[a-z]`.
    #[error("key prefix {0:?} is not 1 to 3 bytes of [a-z]")]
    BadPrefix(String),
    /// `<prefix><n>.<tag>` is longer than a list key may be.
    #[error("key {prefix}{n}.{tag} is {len} bytes; the limit is {max}")]
    TooLong {
        prefix: String,
        n: u64,
        tag: WriterTag,
        len: usize,
        max: usize,
    },
    /// Every `n` up to `u64::MAX` is taken for this prefix and tag.
    #[error("no number is left for key prefix {prefix} and tag {tag}")]
    Exhausted { prefix: String, tag: WriterTag },
}

/// A writer's mark in the keys it mints: its [`ClientId`], written as lower-case base 36.
/// Only IDs a doc accepts as a writer have one (not 0, not the session's own, not past
/// [`MAX_CLIENT_ID`], never the native engine's import agent [`crate::doc::IMPORT_CLIENT`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WriterTag(ClientId);

impl WriterTag {
    /// The tag of `client`, if a doc accepts it as a writer.
    pub fn of(client: ClientId) -> Option<Self> {
        crate::doc::check_client(client).ok().map(|()| Self(client))
    }

    /// The tag spelled `s`: canonical base 36 only (`[0-9a-z]`, no leading zero), of an ID
    /// [`WriterTag::of`] accepts. Exactly the inverse of `Display`.
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.is_empty() || b.len() > MAX_TAG_LEN || b[0] == b'0' {
            return None;
        }
        let mut v: u64 = 0;
        for &c in b {
            let d = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'z' => c - b'a' + 10,
                _ => return None,
            };
            v = v.checked_mul(36)?.checked_add(u64::from(d))?;
        }
        if v > MAX_CLIENT_ID {
            return None;
        }
        Self::of(v)
    }

    /// The writer's client ID.
    pub fn client(self) -> ClientId {
        self.0
    }

    /// The tag's length in bytes (1 to [`MAX_TAG_LEN`]).
    pub fn len(self) -> usize {
        let mut v = self.0;
        let mut n = 1;
        while v >= 36 {
            v /= 36;
            n += 1;
        }
        n
    }

    /// Never: a tag has at least one byte (for clippy's `len_without_is_empty`).
    pub fn is_empty(self) -> bool {
        false
    }
}

impl fmt::Display for WriterTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut buf = [0u8; 13];
        let mut i = buf.len();
        let mut v = self.0;
        loop {
            i -= 1;
            buf[i] = DIGITS[(v % 36) as usize];
            v /= 36;
            if v == 0 {
                break;
            }
        }
        f.write_str(std::str::from_utf8(&buf[i..]).expect("base 36 is ASCII"))
    }
}

/// The loose shape of a numbered key: `<prefix><n>` or `<prefix><n>.<rest>`, with `prefix`
/// 1 to [`MAX_PREFIX_LEN`] bytes of `[a-z]` (the whole run of letters the key starts with),
/// `n` one or more decimal digits (leading zeros allowed, at most `u64::MAX`), and `rest`
/// anything a key may hold, empty included. The table counts its keys with it
/// ([`TaggedKey::parse`] is the strict form).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyShape<'a> {
    pub prefix: &'a str,
    pub n: u64,
    /// After the first `.`; `None` when there is no `.`.
    pub rest: Option<&'a str>,
    /// `n` as written.
    pub digits: &'a str,
}

impl<'a> KeyShape<'a> {
    pub fn parse(key: &'a Key) -> Option<Self> {
        let s = key.as_str();
        let letters = s.bytes().take_while(u8::is_ascii_lowercase).count();
        if letters == 0 || letters > MAX_PREFIX_LEN {
            return None;
        }
        let (prefix, tail) = s.split_at(letters);
        let (digits, rest) = match tail.split_once('.') {
            Some((d, r)) => (d, Some(r)),
            None => (tail, None),
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n = digits.parse::<u64>().ok()?;
        Some(Self {
            prefix,
            n,
            rest,
            digits,
        })
    }
}

/// A minted key: `<prefix><n>.<tag>` (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaggedKey<'a> {
    prefix: &'a str,
    n: u64,
    tag: WriterTag,
}

impl<'a> TaggedKey<'a> {
    /// The key `<prefix><n>.<tag>`; [`MintError::BadPrefix`] unless `prefix` is 1 to
    /// [`MAX_PREFIX_LEN`] bytes of `[a-z]`. Its length is checked by [`TaggedKey::to_key`].
    pub fn new(prefix: &'a str, n: u64, tag: WriterTag) -> Result<Self, MintError> {
        if !good_prefix(prefix) {
            return Err(MintError::BadPrefix(prefix.to_owned()));
        }
        Ok(Self { prefix, n, tag })
    }

    /// `key` read as a tagged key, in its canonical spelling only: no leading zero in `n`
    /// (`0` itself is canonical) and a [`WriterTag::parse`] tag, so
    /// `parse(k)?.to_key() == Ok(k)` for every key it accepts.
    pub fn parse(key: &'a Key) -> Option<Self> {
        let s = KeyShape::parse(key)?;
        if s.digits.len() > 1 && s.digits.starts_with('0') {
            return None;
        }
        let tag = WriterTag::parse(s.rest?)?;
        Some(Self {
            prefix: s.prefix,
            n: s.n,
            tag,
        })
    }

    pub fn prefix(&self) -> &'a str {
        self.prefix
    }

    pub fn n(&self) -> u64 {
        self.n
    }

    pub fn tag(&self) -> WriterTag {
        self.tag
    }

    /// The key, or [`MintError::TooLong`] past [`MAX_KEY_LEN`] bytes.
    pub fn to_key(&self) -> Result<Key, MintError> {
        let s = format!("{}{}.{}", self.prefix, self.n, self.tag);
        if s.len() > MAX_KEY_LEN {
            return Err(MintError::TooLong {
                prefix: self.prefix.to_owned(),
                n: self.n,
                tag: self.tag,
                len: s.len(),
                max: MAX_KEY_LEN,
            });
        }
        Ok(Key::new(s).expect("a tagged key is 1 to 24 bytes of [0-9a-z.]"))
    }
}

fn good_prefix(p: &str) -> bool {
    (1..=MAX_PREFIX_LEN).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_lowercase())
}

/// The next `n` for `prefix` and `tag`: one past the highest among `places` (every key the
/// list knows, **live or deleted**) whose [`TaggedKey::parse`] has that prefix and tag, and
/// the `retired` high-water mark (the highest `n` of that prefix and tag a drop forgot; 0 for
/// none). At least 1. [`MintError::Exhausted`] when the highest is `u64::MAX`.
pub fn next_for<'k>(
    places: impl IntoIterator<Item = &'k Key>,
    retired: u64,
    prefix: &str,
    tag: WriterTag,
) -> Result<u64, MintError> {
    if !good_prefix(prefix) {
        return Err(MintError::BadPrefix(prefix.to_owned()));
    }
    let retired = if planted_ignore_retired() { 0 } else { retired };
    let high = places
        .into_iter()
        .filter_map(TaggedKey::parse)
        .filter(|t| t.prefix == prefix && t.tag == tag)
        .map(|t| t.n)
        .fold(retired, u64::max);
    high.checked_add(1).ok_or_else(|| MintError::Exhausted {
        prefix: prefix.to_owned(),
        tag,
    })
}

/// The start of every retired-mark key in a pack's marks Map ([`mark_key`]).
pub const MARK_PREFIX: &str = "mint/";

/// One dropper's retired-mark key for `prefix` and `tag` in list `root`:
/// `mint/<root>/<prefix>.<tag>/<dropper>`. Each dropper writes only its own key, so concurrent
/// drops never overwrite each other's marks, and the mark read is the max over every
/// dropper's key ([`retired_mark`]): monotone under any concurrency. (One shared key would be
/// last-writer-wins: of two concurrent drops the lower mark could win, and a later mint would
/// reuse a dropped key.)
pub fn mark_key(root: &str, prefix: &str, tag: WriterTag, dropper: WriterTag) -> String {
    if planted_single_lww_mark() {
        return format!("{MARK_PREFIX}{root}/{prefix}.{tag}");
    }
    format!("{MARK_PREFIX}{root}/{prefix}.{tag}/{dropper}")
}

/// The retired mark for `prefix` and `tag` in list `root`: the highest number any dropper
/// wrote under its own [`mark_key`] in `marks` (a pack's marks Map); 0 for none. A value that
/// isn't a decimal number is ignored.
pub fn retired_mark<'m>(
    marks: impl IntoIterator<Item = (&'m str, &'m Value)>,
    root: &str,
    prefix: &str,
    tag: WriterTag,
) -> u64 {
    let base = format!("{MARK_PREFIX}{root}/{prefix}.{tag}");
    marks
        .into_iter()
        .filter(|(k, _)| {
            k.strip_prefix(base.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
        .filter_map(|(_, v)| decimal(v.as_str()?))
        .max()
        .unwrap_or(0)
}

/// A mark's value: one or more ASCII digits, at most `u64::MAX` (no sign, no space).
fn decimal(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// The mark writes that go with a change dropping `dropped` from list `root`, made by
/// `dropper`: for each prefix and tag among the dropped keys that are tagged
/// ([`TaggedKey::parse`]), `dropper`'s own [`mark_key`] set to the highest number dropped,
/// when that is above what its key already holds in `marks`. Untagged keys write nothing.
/// Put them in the same change as the drops.
pub fn marks_for_drops<'k, 'm>(
    root: &str,
    dropped: impl IntoIterator<Item = &'k Key>,
    dropper: WriterTag,
    marks: impl IntoIterator<Item = (&'m str, &'m Value)>,
) -> Vec<MapOp> {
    let mut high: std::collections::BTreeMap<(String, WriterTag), u64> = Default::default();
    for t in dropped.into_iter().filter_map(TaggedKey::parse) {
        let e = high.entry((t.prefix().to_owned(), t.tag())).or_default();
        *e = (*e).max(t.n());
    }
    let marks: std::collections::BTreeMap<&str, &Value> = marks.into_iter().collect();
    high.into_iter()
        .filter_map(|((prefix, tag), n)| {
            let key = mark_key(root, &prefix, tag, dropper);
            let held = marks
                .get(key.as_str())
                .and_then(|v| decimal(v.as_str()?))
                .unwrap_or(0);
            (n > held).then(|| MapOp::Set {
                key,
                value: Value::str(n.to_string()),
            })
        })
        .collect()
}

/// The planted fault [`crate::differential::MintMutant::SingleLwwMark`]: [`mark_key`] is one
/// key per prefix and tag, shared by every dropper (last writer wins).
#[cfg(any(test, feature = "differential"))]
fn planted_single_lww_mark() -> bool {
    crate::differential::mint_mutant() == Some(crate::differential::MintMutant::SingleLwwMark)
}

#[cfg(not(any(test, feature = "differential")))]
fn planted_single_lww_mark() -> bool {
    false
}

/// The planted fault [`crate::differential::MintMutant::IgnoreRetired`]: [`next_for`] ignores
/// the retired mark. Without the `differential` feature there is no hook: `false`.
#[cfg(any(test, feature = "differential"))]
fn planted_ignore_retired() -> bool {
    crate::differential::mint_mutant() == Some(crate::differential::MintMutant::IgnoreRetired)
}

#[cfg(not(any(test, feature = "differential")))]
fn planted_ignore_retired() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::MAX_KIT_CLIENT;

    fn key(s: &str) -> Key {
        Key::new(s).unwrap()
    }

    fn tag(c: ClientId) -> WriterTag {
        WriterTag::of(c).unwrap()
    }

    #[test]
    fn writer_tag_spellings() {
        assert_eq!(tag(2).to_string(), "2");
        assert_eq!(tag(35).to_string(), "z");
        assert_eq!(tag(36).to_string(), "10");
        assert_eq!(tag(MAX_KIT_CLIENT).to_string(), "2gosa7pa2gu");
        assert_eq!(tag(MAX_KIT_CLIENT).len(), MAX_TAG_LEN);
        for c in [2, 9, 10, 35, 36, 1295, 1296, 46_655, 46_656, MAX_KIT_CLIENT] {
            let t = tag(c);
            assert_eq!(t.len(), t.to_string().len(), "len of {c}");
            assert_eq!(
                WriterTag::parse(&t.to_string()),
                Some(t),
                "round trip of {c}"
            );
            assert_eq!(t.client(), c);
        }
    }

    #[test]
    fn writer_tag_only_for_writers() {
        assert_eq!(WriterTag::of(0), None);
        assert_eq!(
            WriterTag::of(1),
            None,
            "the session's own client writes no keys"
        );
        assert_eq!(WriterTag::of(MAX_CLIENT_ID), None, "the import agent's");
        assert_eq!(WriterTag::of(MAX_CLIENT_ID + 1), None);
        assert_eq!(WriterTag::of(u64::MAX), None);
    }

    #[test]
    fn writer_tag_parse_rejects_non_canonical() {
        for s in [
            "",
            "0",
            "1",
            "02",
            "A",
            "2-",
            "2.",
            "_2",
            "2gosa7pa2gv", // MAX_CLIENT_ID, the import agent's
            "2gosa7pa2gw", // MAX_CLIENT_ID + 1
            "zzzzzzzzzzz",
            "100000000000", // 12 digits
        ] {
            assert_eq!(WriterTag::parse(s), None, "{s:?}");
        }
    }

    #[test]
    fn tagged_key_round_trips() {
        for (p, n, c) in [
            ("r", 1, 2),
            ("c", 0, 36),
            ("it", 42, 123_456),
            ("abc", 999_999_999, MAX_KIT_CLIENT),
            ("l", u64::MAX, 2),
        ] {
            let t = TaggedKey::new(p, n, tag(c)).unwrap();
            let k = t.to_key().unwrap();
            assert_eq!(k.as_str(), format!("{p}{n}.{}", tag(c)));
            let back = TaggedKey::parse(&k).unwrap();
            assert_eq!(back, t);
            assert_eq!((back.prefix(), back.n(), back.tag()), (p, n, tag(c)));
        }
    }

    #[test]
    fn max_length_boundary() {
        let t = tag(MAX_KIT_CLIENT);
        // 3 + 9 + 1 + 11 = 24: fits exactly.
        let k = TaggedKey::new("abc", 999_999_999, t)
            .unwrap()
            .to_key()
            .unwrap();
        assert_eq!(k.as_str().len(), MAX_KEY_LEN);
        // One digit more is 25 bytes.
        let e = TaggedKey::new("abc", 1_000_000_000, t)
            .unwrap()
            .to_key()
            .unwrap_err();
        assert_eq!(
            e,
            MintError::TooLong {
                prefix: "abc".into(),
                n: 1_000_000_000,
                tag: t,
                len: 25,
                max: MAX_KEY_LEN
            }
        );
        // The shortest tag and prefix take every u64: 1 + 20 + 1 + 1 = 23.
        assert!(TaggedKey::new("a", u64::MAX, tag(2))
            .unwrap()
            .to_key()
            .is_ok());
    }

    #[test]
    fn bad_prefixes_refused() {
        for p in ["", "abcd", "R", "a1", "a.", "é"] {
            assert_eq!(
                TaggedKey::new(p, 1, tag(2)),
                Err(MintError::BadPrefix(p.to_owned())),
                "{p:?}"
            );
            assert_eq!(
                next_for([], 0, p, tag(2)),
                Err(MintError::BadPrefix(p.to_owned()))
            );
        }
    }

    #[test]
    fn parse_rejects_malformed() {
        for s in [
            "r1",                      // no tag
            "r1.",                     // empty tag
            "r01.2",                   // leading zero
            "r.2",                     // no number
            "1.2",                     // no prefix
            "abcd1.2",                 // prefix too long
            "r1.1",                    // the session's client
            "r1.0",                    // not a client
            "r1.02",                   // tag with a leading zero
            "r1.2.3",                  // two dots
            "r1x.2",                   // letters after the number
            "r1_2",                    // not a dot
            "r1.2-",                   // tag byte outside base 36
            "r1.2gosa7pa2gv",          // the import agent's tag
            "r1.2gosa7pa2gw",          // tag past MAX_CLIENT_ID
            "r18446744073709551616.2", // n past u64::MAX
        ] {
            assert_eq!(TaggedKey::parse(&key(s)), None, "{s:?}");
        }
        assert!(TaggedKey::parse(&key("r0.2")).is_some(), "0 is canonical");
    }

    #[test]
    fn key_shape_is_loose() {
        let s = |k: &str| {
            let k = key(k);
            KeyShape::parse(&k).map(|s| (s.prefix.to_owned(), s.n, s.rest.map(str::to_owned)))
        };
        assert_eq!(s("r5"), Some(("r".into(), 5, None)));
        assert_eq!(s("r05"), Some(("r".into(), 5, None)));
        assert_eq!(s("r5."), Some(("r".into(), 5, Some(String::new()))));
        assert_eq!(s("r5.a.b"), Some(("r".into(), 5, Some("a.b".into()))));
        assert_eq!(s("abc7.x"), Some(("abc".into(), 7, Some("x".into()))));
        for bad in [
            "r",
            "5",
            "r5x",
            "r5-1",
            "abcd5",
            "r.5",
            "r18446744073709551616",
        ] {
            assert_eq!(s(bad), None, "{bad:?}");
        }
    }

    /// The table's count before it moved onto [`KeyShape`] (`table::next_free` at 8b9b1418),
    /// kept here to show the move changed nothing.
    fn old_table_count(k: &Key, prefix: char) -> Option<u64> {
        let s = k.as_str().strip_prefix(prefix)?;
        let n = s.split('.').next()?;
        n.parse::<u64>().ok()
    }

    #[test]
    fn key_shape_counts_as_the_table_did() {
        let mut keys: Vec<String> = [
            "r",
            "r5",
            "r05",
            "r5.",
            "r5.a.b",
            "r5x",
            "r5-1",
            "r5_1",
            "rr5",
            "rc5",
            "c5",
            "c5.2",
            "r.5",
            "5",
            "r18446744073709551615",
            "r18446744073709551616.x",
            "r0",
            "r00.0",
            "abcd5",
            "rabc5",
            "r9.zz",
            "c12.r1",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        // Every key of up to 3 bytes over a small alphabet.
        let alpha = ['r', 'c', '1', '0', '.', 'x', '-'];
        for a in alpha {
            keys.push(a.to_string());
            for b in alpha {
                keys.push(format!("{a}{b}"));
                for c in alpha {
                    keys.push(format!("{a}{b}{c}"));
                }
            }
        }
        for s in &keys {
            let k = key(s);
            for p in ['r', 'c', 'x'] {
                let new = KeyShape::parse(&k)
                    .filter(|sh| sh.prefix.chars().eq([p]))
                    .map(|sh| sh.n);
                assert_eq!(new, old_table_count(&k, p), "{s:?} with prefix {p}");
            }
        }
    }

    #[test]
    fn next_for_counts_live_deleted_and_retired() {
        let me = tag(77);
        let other = tag(78);
        let places = [
            key(&format!("i3.{me}")),
            key(&format!("i9.{me}")), // deleted in the list, still a place
            key(&format!("i50.{other}")),
            key(&format!("l70.{me}")),
            key("i100"),
            key(&format!("i0200.{me}")), // not canonical: not ours to count
        ];
        assert_eq!(next_for(&places, 0, "i", me), Ok(10));
        assert_eq!(next_for(&places, 4, "i", me), Ok(10));
        assert_eq!(next_for(&places, 30, "i", me), Ok(31));
        assert_eq!(next_for(&places, 0, "i", other), Ok(51));
        assert_eq!(next_for(&places, 0, "l", me), Ok(71));
        assert_eq!(next_for(&places, 0, "x", me), Ok(1));
        assert_eq!(next_for([], 0, "i", me), Ok(1));
    }

    #[test]
    fn next_for_exhausted() {
        let me = tag(2);
        let k = TaggedKey::new("a", u64::MAX, me).unwrap().to_key().unwrap();
        assert_eq!(
            next_for([&k], 0, "a", me),
            Err(MintError::Exhausted {
                prefix: "a".into(),
                tag: me
            })
        );
        assert!(matches!(
            next_for([], u64::MAX, "a", me),
            Err(MintError::Exhausted { .. })
        ));
    }

    #[test]
    fn marks_are_per_dropper_and_read_as_the_max() {
        let (w, d1, d2) = (tag(500), tag(600), tag(700));
        let k = |n: u64| TaggedKey::new("i", n, w).unwrap().to_key().unwrap();
        let none: [(&str, &Value); 0] = [];
        let a = marks_for_drops("items", &[k(3), k(7), key("i9")], d1, none);
        assert_eq!(
            a,
            vec![MapOp::Set {
                key: format!("mint/items/i.{w}/{d1}"),
                value: Value::str("7"),
            }]
        );
        let b = marks_for_drops("items", &[k(2), k(3)], d2, none);
        let mut marks = std::collections::BTreeMap::new();
        for op in a.iter().chain(&b) {
            if let MapOp::Set { key, value } = op {
                marks.insert(key.clone(), value.clone());
            }
        }
        marks.insert("mint/items/i.zz/x".into(), Value::str("99")); // another tag
        marks.insert(format!("mint/itemsx/i.{w}/{d1}"), Value::str("99")); // another root
        marks.insert(format!("mint/items/i.{w}0/{d1}"), Value::str("99")); // a longer tag
        marks.insert(format!("mint/items/i.{w}/{d2}x"), Value::str("junk"));
        let it = || marks.iter().map(|(k, v)| (k.as_str(), v));
        assert_eq!(retired_mark(it(), "items", "i", w), 7);
        assert_eq!(retired_mark(it(), "items", "c", w), 0);
        // a dropper whose own key already holds as much writes nothing
        assert!(marks_for_drops("items", &[k(5)], d1, it()).is_empty());
        assert_eq!(marks_for_drops("items", &[k(9)], d1, it()).len(), 1);
        // the mark feeds next_for
        assert_eq!(
            next_for([], retired_mark(it(), "items", "i", w), "i", w),
            Ok(8)
        );
    }

    /// Two writers minting from one list never mint the same key, whatever each has seen.
    #[test]
    fn writers_never_collide() {
        let (a, b) = (tag(1000), tag(1001));
        let mut seen: Vec<Key> = Vec::new();
        let mut all = std::collections::HashSet::new();
        for round in 0..50u64 {
            // Each writer sees only part of the list (concurrency): every third key.
            for (w, phase) in [(a, 0u64), (b, 1u64)] {
                let view: Vec<&Key> = seen
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| (*i as u64 + phase + round) % 3 != 0)
                    .map(|(_, k)| k)
                    .collect();
                // A writer always sees its own keys.
                let own: Vec<&Key> = seen
                    .iter()
                    .filter(|k| TaggedKey::parse(k).is_some_and(|t| t.tag() == w))
                    .collect();
                let n = next_for(view.into_iter().chain(own), 0, "i", w).unwrap();
                let k = TaggedKey::new("i", n, w).unwrap().to_key().unwrap();
                assert!(all.insert(k.clone()), "{k} minted twice");
                seen.push(k);
            }
        }
    }
}
