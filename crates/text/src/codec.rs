//! File bytes <-> doc text, byte for byte.
//!
//! The doc holds the file's text with `\n` line breaks and no BOM. What the file looked like
//! on disk ([`Format`]: the BOM and the one line-ending style it uses) is stored with the doc
//! and put back on write, so every byte nobody edited is written back exactly as it was.
//! Nothing else is touched: no Unicode normalisation, no trailing-whitespace or final-newline
//! fix, no tab conversion.
//!
//! A file that can't round-trip that way is **refused, never mangled** ([`Refusal`]): over
//! the size cap, not UTF-8 (UTF-16/32 included), binary (a NUL byte, or for unknown types a
//! control byte), or more than one line-ending style in one file.
//!
//! This is the kit's `codec::text_file` plus classic-Mac files (bare `\r` throughout), the
//! per-type settings ([`Settings`]) and refusals that say where the problem is.

use std::fmt;

use k2_mpc_base::session::{Declined, PackError};

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// The one line-ending style of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Eol {
    /// `\n`.
    Lf,
    /// `\r\n`.
    Crlf,
    /// A bare `\r` (classic Mac OS).
    Cr,
}

impl Eol {
    pub fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "\n",
            Eol::Crlf => "\r\n",
            Eol::Cr => "\r",
        }
    }
    /// Stable name: `lf`, `crlf`, `cr`.
    pub fn name(self) -> &'static str {
        match self {
            Eol::Lf => "lf",
            Eol::Crlf => "crlf",
            Eol::Cr => "cr",
        }
    }
}

/// Which C0 control bytes a type accepts. `\0` is always refused (binary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Controls {
    /// Every byte but NUL. For the known text types: a form feed in C, an escape in a log.
    Allow,
    /// Only tab, line feed, vertical tab, form feed, carriage return and escape. For files
    /// whose type is unknown: other C0 controls there mean the file is not text.
    Refuse,
}

/// A file type's codec settings: the parts that vary by type. Everything else is fixed
/// for every type: the BOM, every line ending and the final newline (or its absence) are
/// preserved exactly, and the text is never normalised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Settings {
    /// The largest file (bytes on disk, BOM included) that can be a live doc.
    pub max_bytes: usize,
    /// The style new line breaks get in a file that has none yet (empty, or one line).
    /// A file with line breaks keeps its own style.
    pub new_eol: Eol,
    /// The control-byte guard.
    pub controls: Controls,
}

/// What the file looked like on disk, so it can be written back the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Format {
    pub bom: bool,
    pub eol: Eol,
}

/// Why a file can't be a live doc. It then opens the way the host opens files that aren't
/// live (read-only, or tier 0). The file is never written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    TooLarge {
        bytes: usize,
        limit: usize,
    },
    /// A UTF-16 or UTF-32 byte-order mark.
    WideEncoding {
        encoding: &'static str,
    },
    /// The first invalid UTF-8 sequence starts at this byte offset (from the file's start).
    NotUtf8 {
        offset: usize,
    },
    /// A NUL byte at this offset.
    Nul {
        offset: usize,
    },
    /// A control byte the type's guard refuses ([`Controls::Refuse`]).
    Control {
        byte: u8,
        offset: usize,
    },
    /// More than one line-ending style; the counts of each.
    MixedLineEndings {
        lf: usize,
        crlf: usize,
        cr: usize,
    },
}

impl Refusal {
    /// Stable machine code, the kit's: `too_large`, `not_utf8`, `binary`,
    /// `mixed_line_endings`.
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::TooLarge { .. } => "too_large",
            Refusal::WideEncoding { .. } | Refusal::NotUtf8 { .. } => "not_utf8",
            Refusal::Nul { .. } | Refusal::Control { .. } => "binary",
            Refusal::MixedLineEndings { .. } => "mixed_line_endings",
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::TooLarge { bytes, limit } => write!(
                f,
                "file is {bytes} bytes; live editing supports up to {limit} for this type"
            ),
            Refusal::WideEncoding { encoding } => write!(
                f,
                "file is {encoding} (byte-order mark); live editing supports UTF-8 only"
            ),
            Refusal::NotUtf8 { offset } => write!(
                f,
                "file is not valid UTF-8 (first bad byte at offset {offset}); live editing \
                 supports UTF-8 only"
            ),
            Refusal::Nul { offset } => {
                write!(f, "file contains a NUL byte at offset {offset} (binary)")
            }
            Refusal::Control { byte, offset } => write!(
                f,
                "file contains control byte 0x{byte:02X} at offset {offset}; a file of \
                 unknown type with control bytes is treated as binary"
            ),
            Refusal::MixedLineEndings { lf, crlf, cr } => write!(
                f,
                "file mixes line-ending styles ({crlf} CRLF, {lf} LF, {cr} CR); live editing \
                 needs one style, so it is left untouched"
            ),
        }
    }
}

impl From<Refusal> for Declined {
    fn from(r: Refusal) -> Self {
        Declined {
            code: r.code(),
            message: r.to_string(),
        }
    }
}

/// Read file bytes into doc text plus the format to write them back.
pub fn decode(bytes: &[u8], s: &Settings) -> Result<(String, Format), Refusal> {
    if bytes.len() > s.max_bytes {
        return Err(Refusal::TooLarge {
            bytes: bytes.len(),
            limit: s.max_bytes,
        });
    }
    // UTF-32 LE starts like UTF-16 LE, so it is checked first.
    for (mark, encoding) in [
        (&b"\xFF\xFE\x00\x00"[..], "UTF-32LE"),
        (b"\x00\x00\xFE\xFF", "UTF-32BE"),
        (b"\xFF\xFE", "UTF-16LE"),
        (b"\xFE\xFF", "UTF-16BE"),
    ] {
        if bytes.starts_with(mark) {
            return Err(Refusal::WideEncoding { encoding });
        }
    }
    if let Some(offset) = bytes.iter().position(|&b| b == 0) {
        return Err(Refusal::Nul { offset });
    }
    let (bom, body) = match bytes.strip_prefix(BOM) {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };
    let skip = bytes.len() - body.len();
    let s_text = std::str::from_utf8(body).map_err(|e| Refusal::NotUtf8 {
        offset: skip + e.valid_up_to(),
    })?;
    if s.controls == Controls::Refuse {
        if let Some(i) = body.iter().position(|&b| refused_control(b)) {
            return Err(Refusal::Control {
                byte: body[i],
                offset: skip + i,
            });
        }
    }
    let eol = detect_eol(body)?.unwrap_or(s.new_eol);
    let text = match eol {
        Eol::Lf => s_text.to_owned(),
        Eol::Crlf => s_text.replace("\r\n", "\n"),
        Eol::Cr => s_text.replace('\r', "\n"),
    };
    Ok((text, Format { bom, eol }))
}

/// Render doc text back to file bytes in the file's own format.
///
/// The doc never holds `\r` (the kit passes every inserted string through
/// [`normalize_input`]), so each `\n` is exactly one line break. A doc that holds one
/// anyway is refused loudly rather than written as a file that would no longer decode.
pub fn encode(text: &str, f: Format) -> Result<Vec<u8>, PackError> {
    if let Some(i) = text.find('\r') {
        return Err(PackError(format!(
            "text pack: the doc holds a CR at byte {i}; its line breaks must be \\n only \
             (refusing to write a file that would mix line endings)"
        )));
    }
    let eol = f.eol.as_str().as_bytes();
    let breaks = if eol.len() > 1 {
        text.bytes().filter(|&b| b == b'\n').count()
    } else {
        0
    };
    let mut out = Vec::with_capacity(text.len() + breaks + if f.bom { 3 } else { 0 });
    if f.bom {
        out.extend_from_slice(BOM);
    }
    if f.eol == Eol::Lf {
        out.extend_from_slice(text.as_bytes());
        return Ok(out);
    }
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.extend_from_slice(eol);
        }
        out.extend_from_slice(line.as_bytes());
    }
    Ok(out)
}

/// Text arriving from an editor, an agent or a restore, before it reaches the doc: `\r\n`
/// and bare `\r` become `\n`, which [`encode`] then writes in the file's own style. (The
/// kit's; it keeps the doc free of `\r`.)
pub use k2_mpc_base::codec::text_file::normalize_input;

/// Codec settings as stored with the doc: `[1, bom, eol]`, eol `0` LF, `1` CRLF, `2` CR.
/// The same layout as the kit's `TextFilePack`, plus `2`.
pub fn codec_bytes(f: Format) -> Vec<u8> {
    vec![
        CODEC_V1,
        u8::from(f.bom),
        match f.eol {
            Eol::Lf => 0,
            Eol::Crlf => 1,
            Eol::Cr => 2,
        },
    ]
}

/// The [`Format`] in stored codec settings, or a [`PackError`] (a damaged store).
pub fn parse_codec(codec: &[u8]) -> Result<Format, PackError> {
    match codec {
        [CODEC_V1, bom @ (0 | 1), eol @ (0..=2)] => Ok(Format {
            bom: *bom == 1,
            eol: match eol {
                0 => Eol::Lf,
                1 => Eol::Crlf,
                _ => Eol::Cr,
            },
        }),
        _ => Err(PackError(format!(
            "text pack: bad codec settings {codec:?}"
        ))),
    }
}

const CODEC_V1: u8 = 1;

/// C0 controls other than tab, LF, VT, FF, CR and ESC.
fn refused_control(b: u8) -> bool {
    b < 0x20 && !matches!(b, b'\t' | b'\n' | 0x0B | 0x0C | b'\r' | 0x1B)
}

/// The file's one line-ending style, `None` if it has no line break, or the refusal.
fn detect_eol(b: &[u8]) -> Result<Option<Eol>, Refusal> {
    let (mut lf, mut crlf, mut cr) = (0usize, 0usize, 0usize);
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\r' if b.get(i + 1) == Some(&b'\n') => {
                crlf += 1;
                i += 1;
            }
            b'\r' => cr += 1,
            b'\n' => lf += 1,
            _ => {}
        }
        i += 1;
    }
    match (lf > 0, crlf > 0, cr > 0) {
        (false, false, false) => Ok(None),
        (true, false, false) => Ok(Some(Eol::Lf)),
        (false, true, false) => Ok(Some(Eol::Crlf)),
        (false, false, true) => Ok(Some(Eol::Cr)),
        _ => Err(Refusal::MixedLineEndings { lf, crlf, cr }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const S: Settings = Settings {
        max_bytes: 4 << 20,
        new_eol: Eol::Lf,
        controls: Controls::Allow,
    };
    const STRICT: Settings = Settings {
        controls: Controls::Refuse,
        ..S
    };

    fn round_trip(b: &[u8], s: &Settings) -> (String, Format) {
        let (t, f) = decode(b, s).unwrap_or_else(|r| panic!("{b:?}: refused: {r}"));
        assert!(!t.contains('\r'), "doc text holds a CR: {t:?}");
        assert_eq!(encode(&t, f).unwrap(), b, "round trip of {b:?}");
        (t, f)
    }

    #[test]
    fn each_style_round_trips_and_the_doc_has_no_cr() {
        for (b, eol) in [
            (&b"a\nb\n"[..], Eol::Lf),
            (b"a\r\nb\r\n", Eol::Crlf),
            (b"a\rb\r", Eol::Cr),
            (b"a\r", Eol::Cr),
            (b"\r\n", Eol::Crlf),
        ] {
            let (t, f) = round_trip(b, &S);
            assert_eq!(f.eol, eol, "{b:?}");
            assert_eq!(t.matches('\n').count(), f.eol_count(b), "{b:?}");
        }
    }

    impl Format {
        fn eol_count(&self, b: &[u8]) -> usize {
            let s = std::str::from_utf8(b).unwrap();
            s.matches(self.eol.as_str()).count()
        }
    }

    #[test]
    fn bom_no_final_newline_and_empty_files() {
        let (t, f) = round_trip(b"\xEF\xBB\xBFk = 1\r\nv = 2", &S);
        assert_eq!(
            (t.as_str(), f.bom, f.eol),
            ("k = 1\nv = 2", true, Eol::Crlf)
        );
        let (t, f) = round_trip(b"", &S);
        assert_eq!((t.as_str(), f.bom), ("", false));
        let (t, f) = round_trip(b"\xEF\xBB\xBF", &S);
        assert_eq!((t.as_str(), f.bom), ("", true));
        // a BOM-like character inside the text is text
        round_trip("a\u{FEFF}b\n".as_bytes(), &S);
        // a second BOM right after the first is text too
        let (t, _) = round_trip(b"\xEF\xBB\xBF\xEF\xBB\xBFx", &S);
        assert_eq!(t, "\u{FEFF}x");
    }

    #[test]
    fn a_file_without_line_breaks_takes_the_types_style_for_new_ones() {
        let crlf = Settings {
            new_eol: Eol::Crlf,
            ..S
        };
        let (t, f) = round_trip(b"@echo off", &crlf);
        assert_eq!(f.eol, Eol::Crlf);
        assert_eq!(
            encode(&(t + "\nexit\n"), f).unwrap(),
            b"@echo off\r\nexit\r\n"
        );
        // a file that has breaks keeps its own style whatever the type's default
        assert_eq!(round_trip(b"a\nb", &crlf).1.eol, Eol::Lf);
    }

    #[test]
    fn refusals_say_what_and_where() {
        let r = |b: &[u8], s: &Settings| decode(b, s).unwrap_err();
        assert_eq!(
            r(b"a\r\nb\nc", &S),
            Refusal::MixedLineEndings {
                lf: 1,
                crlf: 1,
                cr: 0
            }
        );
        assert_eq!(
            r(b"a\rb\nc", &S),
            Refusal::MixedLineEndings {
                lf: 1,
                crlf: 0,
                cr: 1
            }
        );
        assert_eq!(
            r(b"a\r\r\n", &S),
            Refusal::MixedLineEndings {
                lf: 0,
                crlf: 1,
                cr: 1
            }
        );
        assert_eq!(r(b"ab\0c", &S), Refusal::Nul { offset: 2 });
        assert_eq!(r(b"\xEF\xBB\xBFab\xE9", &S), Refusal::NotUtf8 { offset: 5 });
        assert_eq!(r(b"ok \xC3", &S), Refusal::NotUtf8 { offset: 3 });
        assert_eq!(
            r(b"\xFF\xFEa\0", &S),
            Refusal::WideEncoding {
                encoding: "UTF-16LE"
            }
        );
        assert_eq!(
            r(b"\xFE\xFF\0a", &S),
            Refusal::WideEncoding {
                encoding: "UTF-16BE"
            }
        );
        assert_eq!(
            r(b"\xFF\xFE\0\0a\0\0\0", &S),
            Refusal::WideEncoding {
                encoding: "UTF-32LE"
            }
        );
        assert_eq!(
            r(b"\0\0\xFE\xFF", &S),
            Refusal::WideEncoding {
                encoding: "UTF-32BE"
            }
        );
        assert_eq!(
            r(&[b'a'; 11], &Settings { max_bytes: 10, ..S }),
            Refusal::TooLarge {
                bytes: 11,
                limit: 10
            }
        );
        assert_eq!(
            decode(&[b'a'; 10], &Settings { max_bytes: 10, ..S })
                .unwrap()
                .0
                .len(),
            10
        );
        // the control guard: unknown types only
        let ctl = b"key\x01value\n";
        assert_eq!(r(ctl, &STRICT), Refusal::Control { byte: 1, offset: 3 });
        round_trip(ctl, &S);
        // tab, VT, FF, ESC pass the strict guard
        round_trip(b"\tx\x0B\x0C\x1B[0m\n", &STRICT);
        // codes are the kit's
        let codes: Vec<&str> = [
            r(b"a\r\nb\n", &S),
            r(b"\0", &S),
            r(b"\xFF", &S),
            r(b"\xFF\xFE", &S),
            r(ctl, &STRICT),
        ]
        .iter()
        .map(Refusal::code)
        .collect();
        assert_eq!(
            codes,
            [
                "mixed_line_endings",
                "binary",
                "not_utf8",
                "not_utf8",
                "binary"
            ]
        );
        let d: Declined = r(b"a\0", &S).into();
        assert_eq!(d.code, "binary");
        assert!(d.message.contains("offset 1"), "{}", d.message);
    }

    #[test]
    fn a_cr_in_the_doc_is_refused_loudly() {
        for eol in [Eol::Lf, Eol::Crlf, Eol::Cr] {
            let e = encode("a\r\nb", Format { bom: false, eol }).unwrap_err();
            assert!(e.0.contains("CR at byte 1"), "{e}");
        }
    }

    #[test]
    fn codec_settings_are_strict() {
        for bom in [false, true] {
            for eol in [Eol::Lf, Eol::Crlf, Eol::Cr] {
                let f = Format { bom, eol };
                assert_eq!(parse_codec(&codec_bytes(f)).unwrap(), f);
            }
        }
        for bad in [
            &[][..],
            &[1, 0],
            &[1, 2, 0],
            &[1, 0, 3],
            &[2, 0, 0],
            &[1, 0, 0, 0],
        ] {
            assert!(parse_codec(bad).is_err(), "{bad:?}");
        }
        // the kit's TextFilePack settings read the same
        assert_eq!(
            parse_codec(&[1, 1, 1]).unwrap(),
            Format {
                bom: true,
                eol: Eol::Crlf
            }
        );
    }

    #[test]
    fn normalize_input_strips_cr() {
        assert_eq!(normalize_input("a\r\nb\rc\n"), "a\nb\nc\n");
    }

    fn line() -> impl Strategy<Value = String> {
        // no \r or \n inside a line: emoji, CJK, combining marks, a BOM-like char, tabs,
        // form feed, escape, RTL and a ZWJ sequence
        proptest::string::string_regex(
            "[a-zé日本🎉\u{0301}\u{FEFF}\u{05D0}\u{200D}\t\u{000C}\u{001B} {}<>\"'#;:=-]{0,16}",
        )
        .unwrap()
    }

    proptest! {
        #[test]
        fn accepted_files_round_trip_byte_exact(lines in proptest::collection::vec(line(), 0..24),
                                                bom in any::<bool>(), eol in 0..3u8,
                                                final_nl in any::<bool>(), strict in any::<bool>()) {
            let sep = ["\n", "\r\n", "\r"][usize::from(eol)];
            let mut s = lines.join(sep);
            if final_nl { s.push_str(sep); }
            let mut bytes = Vec::new();
            if bom { bytes.extend_from_slice(BOM); }
            bytes.extend_from_slice(s.as_bytes());
            let st = if strict { STRICT } else { S };
            let (text, fmt) = decode(&bytes, &st).expect("generated file must be accepted");
            prop_assert!(!text.contains('\r'));
            // text that itself starts with U+FEFF is, as bytes, a BOM: only the bytes must match
            if !s.starts_with('\u{FEFF}') {
                prop_assert_eq!(fmt.bom, bom);
            }
            prop_assert_eq!(encode(&text, fmt).unwrap(), bytes);
        }

        #[test]
        fn decode_never_panics_and_what_it_accepts_round_trips(
            bytes in proptest::collection::vec(any::<u8>(), 0..256), strict in any::<bool>()) {
            let st = if strict { STRICT } else { S };
            if let Ok((text, fmt)) = decode(&bytes, &st) {
                prop_assert_eq!(encode(&text, fmt).unwrap(), bytes);
            }
        }

        #[test]
        fn decode_never_panics_on_line_ending_soup(
            bytes in proptest::collection::vec(prop_oneof![Just(b'\r'), Just(b'\n'), Just(b'a'), Just(0xEFu8), Just(0xBB), Just(0xBF)], 0..64)) {
            if let Ok((text, fmt)) = decode(&bytes, &S) {
                prop_assert_eq!(encode(&text, fmt).unwrap(), bytes);
            }
        }
    }
}
