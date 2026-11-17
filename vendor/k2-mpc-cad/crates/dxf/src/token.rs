//! The tokenizer: a DXF file as a sequence of code/value pairs, each with its exact byte
//! ranges. A file is lines; each pair is a code line (an integer, padding kept) and a value line,
//! each with its own ending (`\n` or `\r\n`, mixed files kept line by line). A byte-order mark,
//! `999` comments and anything after the `0`/`EOF` pair are carried as they are. Writing the
//! tokens back ([`Tokens::reemit`]) gives the input's bytes.
//!
//! It refuses only what isn't pairs (an odd number of lines, a code that isn't an integer),
//! binary DXF, and a file over [`Limits`]; a well-formed but unusual file is read as it is.

use std::borrow::Cow;

use crate::error::{CadError, BINARY, NOT_PAIRS, OVER_BOUND};
use crate::{mutant, Limits};

/// The first bytes of a binary DXF file.
pub const BINARY_SENTINEL: &[u8] = b"AutoCAD Binary DXF";

/// A line's ending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Eol {
    /// The file's last line, unended.
    None,
    Lf,
    CrLf,
}

impl Eol {
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Eol::None => b"",
            Eol::Lf => b"\n",
            Eol::CrLf => b"\r\n",
        }
    }
}

/// One code/value pair, by its byte ranges: 20 bytes, so a drawing's pairs take a few times
/// its size, not tens of times (offsets are 32-bit: [`tokenize`] refuses files of 4 GiB or
/// more).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pair {
    pub code: i32,
    /// Where the code line starts; its text, padding kept, runs to its ending
    /// ([`Pair::code_end`]).
    pub code_at: u32,
    /// The value line's text: `value_at..value_end`.
    pub value_at: u32,
    pub value_end: u32,
    pub code_eol: Eol,
    pub value_eol: Eol,
}

impl Pair {
    /// Where the code line's text ends: its ending comes right before the value line.
    pub fn code_end(&self) -> usize {
        self.value_at as usize - self.code_eol.bytes().len()
    }

    /// Where the code line starts.
    pub fn start(&self) -> usize {
        self.code_at as usize
    }

    /// The value line's text, as a range of the file's bytes.
    pub fn value_range(&self) -> std::ops::Range<usize> {
        self.value_at as usize..self.value_end as usize
    }

    /// Just past the value line's ending.
    pub fn end(&self) -> usize {
        self.value_end as usize + self.value_eol.bytes().len()
    }
}

/// A file's tokens.
#[derive(Debug, Clone)]
pub struct Tokens<'a> {
    pub bytes: &'a [u8],
    /// A UTF-8 byte-order mark comes first.
    pub bom: bool,
    pub pairs: Vec<Pair>,
    /// Where the bytes after the last pair start (the file's length when there are none).
    pub tail_at: usize,
    /// The pairs end with `0`/`EOF`.
    pub eof: bool,
}

/// `v` as text: UTF-8 when it is, else as code page 1252 (the default for files before UTF-8).
pub fn decode(v: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(v) {
        Ok(s) => Cow::Borrowed(s),
        Err(_) => Cow::Owned(v.iter().map(|&b| cp1252(b)).collect()),
    }
}

/// One byte of code page 1252.
fn cp1252(b: u8) -> char {
    const HIGH: [u16; 32] = [
        0x20AC, 0xFFFD, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
        0x2039, 0x0152, 0xFFFD, 0x017D, 0xFFFD, 0xFFFD, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022,
        0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0xFFFD, 0x017E, 0x0178,
    ];
    match b {
        0x80..=0x9F => char::from_u32(u32::from(HIGH[usize::from(b - 0x80)])).unwrap_or('\u{FFFD}'),
        _ => char::from(b),
    }
}

/// Text with its `\\U+XXXX` escapes (a character the file's code page can't hold) written as
/// the characters they name. An escape that names no character is kept as written.
pub fn unescape(t: &str) -> Cow<'_, str> {
    if !t.contains("\\U+") && !t.contains("\\u+") {
        return Cow::Borrowed(t);
    }
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(i) = rest.find("\\U+").or_else(|| rest.find("\\u+")) {
        out.push_str(&rest[..i]);
        let hex = rest
            .get(i + 3..i + 7)
            .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()));
        match hex
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .and_then(char::from_u32)
        {
            Some(c) => {
                out.push(c);
                rest = &rest[i + 7..];
            }
            None => {
                out.push_str(&rest[i..i + 3]);
                rest = &rest[i + 3..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// The code pages whose text this reader decodes (besides UTF-8, from 2007 on).
pub const CODE_PAGES_READ: [&str; 1] = ["ansi_1252"];

impl<'a> Tokens<'a> {
    /// Pair `i`'s value, its bytes as written.
    pub fn value(&self, i: usize) -> &'a [u8] {
        &self.bytes[self.pairs[i].value_range()]
    }

    /// Pair `i`'s value as text, spaces around it trimmed.
    pub fn text(&self, i: usize) -> Cow<'a, str> {
        match decode(self.value(i)) {
            Cow::Borrowed(s) => Cow::Borrowed(s.trim()),
            Cow::Owned(s) => Cow::Owned(s.trim().to_owned()),
        }
    }

    /// Whether pair `i` is `0`/`word` (case as written).
    pub fn is0(&self, i: usize, word: &str) -> bool {
        self.pairs[i].code == 0 && self.value(i).trim_ascii() == word.as_bytes()
    }

    /// The tokens written back: the input's bytes, always.
    pub fn reemit(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.bytes.len());
        if self.bom {
            out.extend_from_slice(&self.bytes[..3]);
        }
        let lf = mutant::active("LineEndingNormalised");
        let unpad = mutant::active("CodePaddingDropped");
        let drop_comments = mutant::active("CommentDropped");
        for p in &self.pairs {
            if drop_comments && p.code == 999 {
                continue;
            }
            if unpad {
                out.extend_from_slice(p.code.to_string().as_bytes());
            } else {
                out.extend_from_slice(&self.bytes[p.start()..p.code_end()]);
            }
            out.extend_from_slice(if lf && p.code_eol != Eol::None {
                Eol::Lf.bytes()
            } else {
                p.code_eol.bytes()
            });
            out.extend_from_slice(&self.bytes[p.value_range()]);
            out.extend_from_slice(if lf && p.value_eol != Eol::None {
                Eol::Lf.bytes()
            } else {
                p.value_eol.bytes()
            });
        }
        if !mutant::active("TailDropped") {
            out.extend_from_slice(&self.bytes[self.tail_at..]);
        }
        out
    }

    /// The file's line endings: `lf`, `crlf`, `mixed`, or `none` (one line, unended).
    pub fn eol(&self) -> &'static str {
        let (mut lf, mut crlf) = (false, false);
        for p in &self.pairs {
            for e in [p.code_eol, p.value_eol] {
                match e {
                    Eol::Lf => lf = true,
                    Eol::CrLf => crlf = true,
                    Eol::None => {}
                }
            }
        }
        match (lf, crlf) {
            (true, true) => "mixed",
            (false, true) => "crlf",
            (true, false) => "lf",
            (false, false) => "none",
        }
    }

    /// How many code lines are padded (a space before the number) and how many aren't, among
    /// codes under 100 (where padding shows).
    pub fn padding(&self) -> (u64, u64) {
        let (mut padded, mut plain) = (0, 0);
        for p in &self.pairs {
            if p.code >= 100 || p.code < 0 {
                continue;
            }
            if self.bytes.get(p.start()) == Some(&b' ') {
                padded += 1;
            } else {
                plain += 1;
            }
        }
        (padded, plain)
    }
}

/// One line: its text's end and its ending, from `at`.
fn line(bytes: &[u8], at: usize) -> (usize, Eol, usize) {
    match bytes[at..].iter().position(|&b| b == b'\n') {
        Some(i) => {
            let nl = at + i;
            if nl > at && bytes[nl - 1] == b'\r' {
                (nl - 1, Eol::CrLf, nl + 1)
            } else {
                (nl, Eol::Lf, nl + 1)
            }
        }
        None => (bytes.len(), Eol::None, bytes.len()),
    }
}

/// The tokens of `bytes`.
pub fn tokenize<'a>(bytes: &'a [u8], limits: &Limits) -> Result<Tokens<'a>, CadError> {
    if bytes.starts_with(BINARY_SENTINEL) {
        return Err(CadError::new(
            BINARY,
            "a binary DXF file",
            "binary DXF isn't read; save the drawing as ASCII DXF",
        ));
    }
    if bytes.len() as u64 > limits.max_bytes || bytes.len() >= u32::MAX as usize {
        return Err(CadError::new(
            OVER_BOUND,
            format!(
                "the file is over the size limit of {} bytes",
                limits.max_bytes
            ),
            "the drawing stays a plain file",
        ));
    }
    let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let mut at = if bom { 3 } else { 0 };
    let mut pairs = Vec::new();
    let mut eof = false;
    let lenient = mutant::active("LenientTokenizer");
    while at < bytes.len() {
        let code_at = at;
        let (code_end, code_eol, next) = line(bytes, at);
        if code_end - code_at > limits.max_line {
            return Err(over_line(limits).at(code_at));
        }
        if code_eol == Eol::None || next >= bytes.len() {
            // a code line with no value line after it
            return Err(CadError::new(
                NOT_PAIRS,
                "an odd number of lines: a group code with no value",
                "`check` reads what it can; the file may be cut short",
            )
            .at(code_at));
        }
        let text = std::str::from_utf8(&bytes[code_at..code_end])
            .ok()
            .map(|s| s.trim_matches(|c| c == ' ' || c == '\t'));
        let code = match text.and_then(|t| t.parse::<i32>().ok()) {
            Some(c) => c,
            None if lenient => {
                at = next;
                continue;
            }
            None => {
                return Err(CadError::new(
                    NOT_PAIRS,
                    "a group code that isn't an integer",
                    "the file isn't DXF pairs from here; `check` names the offset",
                )
                .at(code_at));
            }
        };
        let value_at = next;
        let (value_end, value_eol, after) = line(bytes, value_at);
        if value_end - value_at > limits.max_line {
            return Err(over_line(limits).at(value_at));
        }
        // in range: the file is under 4 GiB (refused above)
        pairs.push(Pair {
            code,
            code_at: code_at as u32,
            value_at: value_at as u32,
            value_end: value_end as u32,
            code_eol,
            value_eol,
        });
        if pairs.len() as u64 > limits.max_pairs {
            return Err(CadError::new(
                OVER_BOUND,
                format!("over the limit of {} pairs", limits.max_pairs),
                "the drawing stays a plain file",
            )
            .at(code_at));
        }
        at = after;
        if code == 0 && bytes[value_at..value_end].trim_ascii() == b"EOF" {
            eof = true;
            break;
        }
    }
    Ok(Tokens {
        bytes,
        bom,
        pairs,
        tail_at: at,
        eof,
    })
}

fn over_line(limits: &Limits) -> CadError {
    CadError::new(
        OVER_BOUND,
        format!("a line over the limit of {} bytes", limits.max_line),
        "the drawing stays a plain file",
    )
}
