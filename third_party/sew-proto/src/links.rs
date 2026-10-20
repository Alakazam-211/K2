//! Bare links in message bodies, shared by Sew and clients so both highlight the same links
//! (S16 §10.1).
//!
//! - Code (fenced blocks and inline code spans, [`crate::mention::code_regions`]) is skipped.
//! - A link starts at `http://` or `https://` (ASCII case-insensitive); the character before
//!   it must not be ASCII alphanumeric or `\`.
//! - It ends at the first whitespace, `<`, `>`, `"`, backtick or control character.
//! - Trailing `. , : ; ! ? ' * _ ~` are trimmed, repeatedly; a trailing `)` or `]` is trimmed
//!   while the span holds more closers than openers of that kind.
//! - Spans longer than 2,048 bytes are dropped.
//! - `<https://…>` (angle brackets right around the span) is `suppressed`: not previewed.
//! - A span right after `](` (a labelled markdown link target) is `labelled`: not previewed.
//!
//! The server previews the first three distinct unsuppressed, unlabelled spans that pass its
//! URL rules.

use crate::limits::PREVIEW_URL_MAX_BYTES;
use crate::mention::code_regions;

/// A bare link and its byte range in the body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UrlSpan {
    /// Byte offset of the first character.
    pub start: usize,
    /// Byte offset just past the last character.
    pub end: usize,
    /// Written as `<url>`: never previewed.
    pub suppressed: bool,
    /// The target of a labelled markdown link `[label](url)`: never previewed.
    pub labelled: bool,
}

impl UrlSpan {
    /// The span's text in `body`.
    pub fn text<'a>(&self, body: &'a str) -> &'a str {
        &body[self.start..self.end]
    }

    /// Whether the server may preview it.
    pub fn previewable(&self) -> bool {
        !self.suppressed && !self.labelled
    }
}

fn starts_scheme(b: &[u8], i: usize) -> Option<usize> {
    for scheme in [&b"https://"[..], &b"http://"[..]] {
        if b.len() >= i + scheme.len() && b[i..i + scheme.len()].eq_ignore_ascii_case(scheme) {
            return Some(scheme.len());
        }
    }
    None
}

fn is_stop(c: char) -> bool {
    c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | '`')
}

fn count(s: &str, c: char) -> usize {
    s.chars().filter(|x| *x == c).count()
}

/// Trims trailing punctuation and unbalanced closers from `s`; returns the new length.
fn trim_end(s: &str) -> usize {
    let mut end = s.len();
    loop {
        let cur = &s[..end];
        let Some(last) = cur.chars().next_back() else {
            return end;
        };
        let trim = match last {
            '.' | ',' | ':' | ';' | '!' | '?' | '\'' | '*' | '_' | '~' => true,
            ')' => count(cur, ')') > count(cur, '('),
            ']' => count(cur, ']') > count(cur, '['),
            _ => false,
        };
        if !trim {
            return end;
        }
        end -= last.len_utf8();
    }
}

/// Every bare link in `body`, in order. Pure; never fails.
pub fn extract(body: &str) -> Vec<UrlSpan> {
    let b = body.as_bytes();
    let code = code_regions(body);
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if let Some((_, end)) = code.iter().find(|(s, e)| i >= *s && i < *e) {
            i = *end;
            continue;
        }
        let Some(scheme_len) = starts_scheme(b, i) else {
            // Step one character (links start at ASCII, so byte steps inside a multi-byte
            // character never match a scheme).
            i += 1;
            continue;
        };
        let prev_ok = match body[..i].chars().next_back() {
            None => true,
            Some(c) => !(c.is_ascii_alphanumeric() || c == '\\'),
        };
        if !prev_ok {
            i += scheme_len;
            continue;
        }
        let rest = &body[i..];
        let raw_len = rest.find(is_stop).unwrap_or(rest.len());
        let len = trim_end(&rest[..raw_len]);
        let end = i + len;
        if len <= scheme_len {
            i += raw_len.max(scheme_len);
            continue;
        }
        if len <= PREVIEW_URL_MAX_BYTES {
            let suppressed = i > 0 && b[i - 1] == b'<' && b.get(end) == Some(&b'>');
            let labelled = i >= 2 && &b[i - 2..i] == b"](";
            out.push(UrlSpan {
                start: i,
                end,
                suppressed,
                labelled,
            });
        }
        i += raw_len.max(1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(body: &str) -> Vec<&str> {
        extract(body).iter().map(|s| s.text(body)).collect()
    }

    #[test]
    fn basics() {
        assert_eq!(
            texts("(see http://a.example/x_(y)).").as_slice(),
            &["http://a.example/x_(y)"]
        );
        assert_eq!(texts("xhttp://a.example").len(), 0);
        let s = extract("<https://a.example>");
        assert!(s[0].suppressed);
        let s = extract("[l](https://a.example)");
        assert!(s[0].labelled);
    }
}
