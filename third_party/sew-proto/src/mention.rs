//! Mention syntax, shared by Sew, K2 and clients (contracts rev 20; S13 §7.2–7.3).
//!
//! | Token | Syntax | Meaning |
//! |---|---|---|
//! | Member id | `<@ID>` | That member |
//! | Role or group id | `<@&ID>` | A role or user group (`<@&SPACE_ID>` is `@everyone`) |
//! | Handle | `@handle`, `[A-Za-z0-9_-]{2,64}`, lowercased | Reserved word, member, or group |
//! | Here | `@here` | Online channel members at send time |
//! | Channel | `@channel`, `@everyone`, `@all` | Every channel member |
//!
//! Mentions are never parsed inside code (fenced blocks and inline code spans). A backslash
//! before ASCII punctuation makes that character literal, so `\@rosson` and `\<@1>` are text.
//! A handle's `@` must start the body or follow a character that is not ASCII alphanumeric and
//! not one of `_ - . @ / : +`, so e-mail addresses and URLs are not mentions. Resolution to
//! members, roles and groups is the server's job; unresolvable tokens are plain text.

use crate::id::{Id, parse_id_str};

/// Handles that never resolve to a member (S13 gap G9).
pub const RESERVED_HANDLES: &[&str] = &["here", "channel", "everyone", "all"];

/// What a mention token names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MentionTokenKind {
    /// `<@ID>`.
    Member(Id),
    /// `<@&ID>`: a role or a user group.
    RoleOrGroup(Id),
    /// `@handle`, lowercased; never a reserved word.
    Handle(String),
    /// `@here`.
    Here,
    /// `@channel`, `@everyone` or `@all`.
    Channel,
}

/// A mention token and its byte range in the body.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MentionToken {
    /// What it names.
    pub kind: MentionTokenKind,
    /// Byte offset of the first character (`<` or `@`).
    pub start: usize,
    /// Byte offset just past the token.
    pub end: usize,
}

fn is_handle_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

fn run_len(b: &[u8], i: usize, c: u8) -> usize {
    b[i..].iter().take_while(|x| **x == c).count()
}

/// Byte ranges `[start, end)` of fenced code blocks and inline code spans.
pub fn code_regions(body: &str) -> Vec<(usize, usize)> {
    let b = body.as_bytes();
    let mut regions = Vec::new();
    let mut fences: Vec<(usize, usize)> = Vec::new();

    // Fenced blocks, line by line.
    let mut line_start = 0;
    let mut open: Option<(usize, u8, usize)> = None; // (region start, fence char, run length)
    while line_start < b.len() {
        let line_end = b[line_start..]
            .iter()
            .position(|c| *c == b'\n')
            .map_or(b.len(), |p| line_start + p);
        let indent = b[line_start..line_end]
            .iter()
            .take_while(|c| **c == b' ')
            .count();
        let first = line_start + indent;
        match open {
            None => {
                if indent <= 3 && first < line_end && (b[first] == b'`' || b[first] == b'~') {
                    let n = run_len(b, first, b[first]);
                    if n >= 3 {
                        open = Some((line_start, b[first], n));
                    }
                }
            }
            Some((start, ch, n)) => {
                if indent <= 3 && first < line_end && b[first] == ch {
                    let m = run_len(b, first, ch);
                    let rest = &b[first + m..line_end];
                    if m >= n && rest.iter().all(u8::is_ascii_whitespace) {
                        fences.push((start, line_end));
                        open = None;
                    }
                }
            }
        }
        line_start = line_end + 1;
    }
    if let Some((start, _, _)) = open {
        fences.push((start, b.len()));
    }

    // Inline code spans outside fences.
    let in_fence = |i: usize| fences.iter().find(|(s, e)| i >= *s && i < *e).map(|f| f.1);
    let mut i = 0;
    while i < b.len() {
        if let Some(end) = in_fence(i) {
            i = end;
            continue;
        }
        match b[i] {
            b'\\' if i + 1 < b.len() && b[i + 1].is_ascii_punctuation() => i += 2,
            b'`' => {
                let n = run_len(b, i, b'`');
                let mut j = i + n;
                let mut close = None;
                while j < b.len() {
                    if in_fence(j).is_some() {
                        break;
                    }
                    if b[j] == b'`' {
                        let m = run_len(b, j, b'`');
                        if m == n {
                            close = Some(j + m);
                            break;
                        }
                        j += m;
                    } else {
                        j += 1;
                    }
                }
                match close {
                    Some(end) => {
                        regions.push((i, end));
                        i = end;
                    }
                    None => i += n,
                }
            }
            _ => i += 1,
        }
    }

    regions.extend(fences);
    regions.sort_unstable();
    regions
}

/// What `<@…` at a position is.
enum IdToken {
    /// A valid `<@ID>` or `<@&ID>` ending at the offset.
    Valid(MentionTokenKind, usize),
    /// `<@digits>` or `<@&digits>` with a bad id: plain text up to the offset.
    Invalid(usize),
    /// Not id-token shaped; scan on normally.
    NotToken,
}

fn parse_id_token(b: &[u8], i: usize) -> IdToken {
    // b[i..] starts with "<@".
    let mut j = i + 2;
    let role = b.get(j) == Some(&b'&');
    if role {
        j += 1;
    }
    let digits = b[j..].iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || b.get(j + digits) != Some(&b'>') {
        return if role {
            IdToken::Invalid(j)
        } else {
            IdToken::NotToken
        };
    }
    let end = j + digits + 1;
    let id = std::str::from_utf8(&b[j..j + digits])
        .ok()
        .and_then(parse_id_str);
    match id {
        Some(id) if role => IdToken::Valid(MentionTokenKind::RoleOrGroup(id), end),
        Some(id) => IdToken::Valid(MentionTokenKind::Member(id), end),
        None => IdToken::Invalid(end),
    }
}

fn handle_boundary_ok(body: &str, at: usize) -> bool {
    match body[..at].chars().next_back() {
        None => true,
        Some(c) => {
            !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '@' | '/' | ':' | '+'))
        }
    }
}

/// Every mention token in `body`, in order. Pure; never fails.
pub fn parse(body: &str) -> Vec<MentionToken> {
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
            b'<' if b.get(i + 1) == Some(&b'@') => match parse_id_token(b, i) {
                IdToken::Valid(kind, end) => {
                    out.push(MentionToken {
                        kind,
                        start: i,
                        end,
                    });
                    i = end;
                }
                IdToken::Invalid(end) => i = end,
                IdToken::NotToken => i += 1,
            },
            b'@' => {
                let n = b[i + 1..]
                    .iter()
                    .take_while(|c| is_handle_byte(**c))
                    .count();
                if (2..=64).contains(&n) && handle_boundary_ok(body, i) {
                    let end = i + 1 + n;
                    let handle = body[i + 1..end].to_ascii_lowercase();
                    let kind = match handle.as_str() {
                        "here" => MentionTokenKind::Here,
                        "channel" | "everyone" | "all" => MentionTokenKind::Channel,
                        _ => MentionTokenKind::Handle(handle),
                    };
                    out.push(MentionToken {
                        kind,
                        start: i,
                        end,
                    });
                    i = end;
                } else {
                    i += 1 + n;
                }
            }
            _ => i += 1,
        }
    }
    out
}
