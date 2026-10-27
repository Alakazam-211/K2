//! File bytes <-> doc text, byte-exact.
//!
//! The doc holds text with `\n` line breaks and no BOM. The file's own style (BOM, CRLF)
//! is remembered in [`FileFormat`] and put back on write, so an untouched file
//! round-trips byte for byte. Files that can't round-trip that way are declined.

use std::fmt;

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// Line-ending style of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eol {
    /// `\n`, or no line breaks at all (new lines are then written as `\n`).
    Lf,
    /// `\r\n` everywhere.
    Crlf,
}

/// What the file looked like on disk, so it can be written back the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileFormat {
    pub bom: bool,
    pub eol: Eol,
}

/// Why a file can't be a live doc. It then opens the old way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decline {
    TooLarge {
        bytes: usize,
        limit: usize,
    },
    NotUtf8,
    /// Contains a NUL byte: treated as binary.
    Binary,
    /// Mixes `\r\n` with bare `\n`, or has a bare `\r`.
    MixedLineEndings,
}

impl Decline {
    /// Stable machine code, for wire frames and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Decline::TooLarge { .. } => "too_large",
            Decline::NotUtf8 => "not_utf8",
            Decline::Binary => "binary",
            Decline::MixedLineEndings => "mixed_line_endings",
        }
    }
}

impl fmt::Display for Decline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Decline::TooLarge { bytes, limit } => write!(
                f,
                "file is {bytes} bytes; live editing supports up to {limit}"
            ),
            Decline::NotUtf8 => write!(f, "file is not valid UTF-8"),
            Decline::Binary => write!(f, "file contains NUL bytes (binary)"),
            Decline::MixedLineEndings => write!(f, "file mixes line-ending styles"),
        }
    }
}

/// Read file bytes into doc text plus the format needed to write them back.
pub fn decode(bytes: &[u8], limit: usize) -> Result<(String, FileFormat), Decline> {
    if bytes.len() > limit {
        return Err(Decline::TooLarge {
            bytes: bytes.len(),
            limit,
        });
    }
    if bytes.contains(&0) {
        return Err(Decline::Binary);
    }
    let (bom, body) = match bytes.strip_prefix(BOM) {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };
    let s = std::str::from_utf8(body).map_err(|_| Decline::NotUtf8)?;
    let eol = detect_eol(s)?;
    let text = match eol {
        Eol::Lf => s.to_owned(),
        Eol::Crlf => s.replace("\r\n", "\n"),
    };
    Ok((text, FileFormat { bom, eol }))
}

/// Render doc text back to file bytes in the file's own format.
///
/// The doc never holds `\r` (see [`normalize_input`]), so `\n` -> `\r\n` is exact.
pub fn encode(text: &str, fmt: FileFormat) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        text.len()
            + 3
            + if fmt.eol == Eol::Crlf {
                text.len() / 32
            } else {
                0
            },
    );
    if fmt.bom {
        out.extend_from_slice(BOM);
    }
    match fmt.eol {
        Eol::Lf => out.extend_from_slice(text.as_bytes()),
        Eol::Crlf => {
            for (i, line) in text.split('\n').enumerate() {
                if i > 0 {
                    out.extend_from_slice(b"\r\n");
                }
                out.extend_from_slice(line.as_bytes());
            }
        }
    }
    out
}

/// Normalise text coming in from an editor, agent or disk before it reaches the doc:
/// `\r\n` and bare `\r` become `\n`. Keeps the doc free of `\r`, which [`encode`] relies on.
pub fn normalize_input(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('\r') {
        return std::borrow::Cow::Borrowed(s);
    }
    std::borrow::Cow::Owned(s.replace("\r\n", "\n").replace('\r', "\n"))
}

fn detect_eol(s: &str) -> Result<Eol, Decline> {
    let b = s.as_bytes();
    let (mut crlf, mut lf) = (0usize, 0usize);
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'\n' => {
                if i > 0 && b[i - 1] == b'\r' {
                    crlf += 1
                } else {
                    lf += 1
                }
            }
            b'\r' if b.get(i + 1) != Some(&b'\n') => return Err(Decline::MixedLineEndings), // bare CR
            _ => {}
        }
    }
    match (crlf, lf) {
        (0, _) => Ok(Eol::Lf),
        (_, 0) => Ok(Eol::Crlf),
        _ => Err(Decline::MixedLineEndings),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const LIMIT: usize = 4 << 20;

    #[test]
    fn plain_lf_round_trips() {
        let b = b"# Title\n\nbody\n";
        let (t, f) = decode(b, LIMIT).unwrap();
        assert_eq!(
            f,
            FileFormat {
                bom: false,
                eol: Eol::Lf
            }
        );
        assert_eq!(encode(&t, f), b);
    }

    #[test]
    fn crlf_and_bom_round_trip_and_doc_has_no_cr() {
        let b = b"\xEF\xBB\xBF# T\r\n\r\nx\r\n";
        let (t, f) = decode(b, LIMIT).unwrap();
        assert_eq!(t, "# T\n\nx\n");
        assert_eq!(
            f,
            FileFormat {
                bom: true,
                eol: Eol::Crlf
            }
        );
        assert_eq!(encode(&t, f), b);
    }

    #[test]
    fn no_final_newline_is_kept() {
        let (t, f) = decode(b"a\r\nb", LIMIT).unwrap();
        assert_eq!(encode(&t, f), b"a\r\nb");
    }

    #[test]
    fn declines() {
        assert_eq!(
            decode(b"a\r\nb\nc", LIMIT).unwrap_err(),
            Decline::MixedLineEndings
        );
        assert_eq!(
            decode(b"a\rb", LIMIT).unwrap_err(),
            Decline::MixedLineEndings
        );
        assert_eq!(decode(b"a\0b", LIMIT).unwrap_err(), Decline::Binary);
        assert_eq!(decode(b"\xff\xfe", LIMIT).unwrap_err(), Decline::NotUtf8);
        assert_eq!(
            decode(&[b'a'; 11], 10).unwrap_err(),
            Decline::TooLarge {
                bytes: 11,
                limit: 10
            }
        );
        assert_eq!(decode(&[b'a'; 10], 10).unwrap().0.len(), 10);
    }

    #[test]
    fn normalize_input_strips_cr() {
        assert_eq!(normalize_input("a\r\nb\rc\n"), "a\nb\nc\n");
        assert!(matches!(
            normalize_input("plain\n"),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    fn line() -> impl Strategy<Value = String> {
        // no \r or \n inside a line; includes BOM-like chars, emoji, CJK, combining marks
        proptest::string::string_regex("[a-zé日本🎉\u{0301}\u{FEFF} #*`\\-\\[\\]]{0,12}").unwrap()
    }

    proptest! {
        #[test]
        fn accepted_files_round_trip_byte_exact(lines in proptest::collection::vec(line(), 0..20),
                                                bom in any::<bool>(), crlf in any::<bool>(), final_nl in any::<bool>()) {
            let sep = if crlf { "\r\n" } else { "\n" };
            let mut s = lines.join(sep);
            if final_nl { s.push_str(sep); }
            let mut bytes = Vec::new();
            if bom { bytes.extend_from_slice(BOM); }
            bytes.extend_from_slice(s.as_bytes());
            let (text, fmt) = decode(&bytes, LIMIT).expect("generated file must be accepted");
            prop_assert!(!text.contains('\r'));
            prop_assert_eq!(encode(&text, fmt), bytes);
        }

        #[test]
        fn decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            if let Ok((text, fmt)) = decode(&bytes, LIMIT) {
                prop_assert_eq!(encode(&text, fmt), bytes);
            }
        }
    }
}
