//! Custom emoji tokens in message bodies, shared by Sew, K2 and clients (S16 §6.6, gap G10).
//!
//! ```text
//! token := "<" [ "a" ] ":" NAME ":" ID ">"
//! NAME  := [a-z0-9_-]{2,32}
//! ID    := decimal 1..=i64::MAX, no leading zeros
//! ```
//!
//! Tokens are never parsed inside code (fenced blocks and inline code spans, the
//! [`crate::mention::code_regions`] rules). A backslash before ASCII punctuation makes that
//! character literal, so `\<:ok:1>` is text. `NAME` and `a` are hints: the server resolves by
//! `ID` only, and an unknown or unusable id stays plain text.

use crate::id::{Id, parse_id_str};
use crate::mention::code_regions;

/// A custom emoji token and its byte range in the body.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmojiToken {
    /// The emoji id.
    pub id: Id,
    /// The name hint.
    pub name: String,
    /// The `a` (animated) hint.
    pub animated: bool,
    /// Byte offset of `<`.
    pub start: usize,
    /// Byte offset just past `>`.
    pub end: usize,
}

fn is_name_byte(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-'
}

/// Whether `s` is a valid custom emoji name or alias (`[a-z0-9_-]{2,32}`).
pub fn is_name(s: &str) -> bool {
    (2..=32).contains(&s.len()) && s.bytes().all(is_name_byte)
}

/// Parses a token starting at `b[i] == '<'`; the token and its end.
fn token_at(body: &str, i: usize) -> Option<EmojiToken> {
    let b = body.as_bytes();
    let mut j = i + 1;
    let animated = b.get(j) == Some(&b'a');
    if animated {
        j += 1;
    }
    if b.get(j) != Some(&b':') {
        return None;
    }
    j += 1;
    let name_len = b[j..].iter().take_while(|c| is_name_byte(**c)).count();
    if !(2..=32).contains(&name_len) || b.get(j + name_len) != Some(&b':') {
        return None;
    }
    let name = &body[j..j + name_len];
    j += name_len + 1;
    let digits = b[j..].iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || b.get(j + digits) != Some(&b'>') {
        return None;
    }
    let id = parse_id_str(&body[j..j + digits])?;
    Some(EmojiToken {
        id,
        name: name.to_owned(),
        animated,
        start: i,
        end: j + digits + 1,
    })
}

/// Every custom emoji token in `body`, in order. Pure; never fails.
pub fn parse(body: &str) -> Vec<EmojiToken> {
    let b = body.as_bytes();
    let code = code_regions(body);
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if let Some((_, end)) = code.iter().find(|(s, e)| i >= *s && i < *e) {
            i = *end;
            continue;
        }
        match b[i] {
            b'\\' if i + 1 < b.len() && b[i + 1].is_ascii_punctuation() => i += 2,
            b'<' => match token_at(body, i) {
                Some(t) => {
                    i = t.end;
                    out.push(t);
                }
                None => i += 1,
            },
            _ => i += 1,
        }
    }
    out
}

/// The distinct ids of `body`'s tokens, ascending.
pub fn ids(body: &str) -> Vec<Id> {
    let mut v: Vec<Id> = parse(body).into_iter().map(|t| t.id).collect();
    v.sort_unstable();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        let t = parse("hi <:ok:12> <a:party_1:34>");
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].start, t[0].end), (3, 11));
        assert!(t[1].animated);
        assert_eq!(t[1].name, "party_1");
    }
}
