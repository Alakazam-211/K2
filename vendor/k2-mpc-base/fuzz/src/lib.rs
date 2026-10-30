//! Shared parts of the kit's fuzz targets: structured text, and the oracles. Every target
//! asserts properties: a panic is a finding, and so is a violated property (an assert).

use arbitrary::{Arbitrary, Unstructured};

pub mod keepboth;
pub mod merge;
pub mod outside;
pub mod writebase;

/// A piece of text: mostly from a small vocabulary, so text repeats (the hard case for
/// diffs and element identity), with newlines, multi-byte characters and the empty string;
/// sometimes arbitrary UTF-8.
#[derive(Debug, Clone, Copy, Arbitrary)]
pub enum Tok {
    A,
    B,
    Ab,
    Nl,
    AbNl,
    Line3,
    Space,
    Kanji,
    Emoji,
    Accent,
}

impl Tok {
    pub fn as_str(self) -> &'static str {
        match self {
            Tok::A => "a",
            Tok::B => "b",
            Tok::Ab => "ab",
            Tok::Nl => "\n",
            Tok::AbNl => "ab\n",
            Tok::Line3 => "line 3\n",
            Tok::Space => " ",
            Tok::Kanji => "日",
            Tok::Emoji => "🎉",
            // a combining sequence: two chars, one grapheme
            Tok::Accent => "e\u{301}",
        }
    }
}

/// At most this many tokens (or bytes of raw text) per piece: the merge is quadratic in the
/// worst case, and short inputs find the bugs.
pub const MAX_TOKS: usize = 48;

#[derive(Debug, Clone)]
pub enum Text {
    Toks(Vec<Tok>),
    /// Arbitrary UTF-8 a text file can hold as it is: no NUL (binary, declined), no `\r`
    /// (line endings are normalised) and no U+FEFF (a leading one is the BOM).
    Raw(String),
}

impl<'a> Arbitrary<'a> for Text {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        if u.ratio(1, 8)? {
            let s: String = u.arbitrary()?;
            let s: String = s
                .chars()
                .filter(|&c| !matches!(c, '\0' | '\r' | '\u{feff}'))
                .take(MAX_TOKS)
                .collect();
            return Ok(Text::Raw(s));
        }
        let n = u.int_in_range(0..=MAX_TOKS)?;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(u.arbitrary()?);
        }
        Ok(Text::Toks(v))
    }
}

impl Text {
    pub fn render(&self) -> String {
        match self {
            Text::Toks(v) => v.iter().map(|t| t.as_str()).collect(),
            Text::Raw(s) => s.clone(),
        }
    }
}

/// The char boundary of `t` nearest below fraction `p` of its length (`p` of `u16::MAX + 1`).
pub fn at(t: &str, p: u16) -> usize {
    let mut i = (p as usize * t.len()) / (u16::MAX as usize + 1);
    while !t.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// `[lo, hi)` of `t` from two fractions, on char boundaries.
pub fn span(t: &str, p: u16, q: u16) -> (usize, usize) {
    let (a, b) = (at(t, p), at(t, q));
    (a.min(b), a.max(b))
}

/// `t` with `[lo, hi)` replaced by `ins`.
pub fn splice(t: &str, (lo, hi): (usize, usize), ins: &str) -> String {
    format!("{}{ins}{}", &t[..lo], &t[hi..])
}
