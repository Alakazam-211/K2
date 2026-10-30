//! What a file-type pack gives a session: its schema and its file codec.

use std::borrow::Cow;
use std::fmt;

use crate::codec::text_file::{self, Eol, FileFormat};
use crate::doc::Neutral;
use crate::schema::{Kind, Schema};

/// A file decoded into doc content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackContent {
    /// Each schema root's content, in any order; every root exactly once.
    pub roots: Vec<(String, Neutral)>,
    /// The codec's own settings for this file (e.g. BOM and line endings), opaque to the
    /// kit. Stored with the doc and handed back to [`Pack::encode`].
    pub codec: Vec<u8>,
}

/// Why a file can't be a live doc (it then opens the old way).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declined {
    /// Stable machine code, e.g. `too_large`, `not_utf8`, `binary`, `mixed_line_endings`.
    pub code: &'static str,
    /// Human-readable reason.
    pub message: String,
}

impl fmt::Display for Declined {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

/// A pack failed to encode (bad stored codec settings) or returned content that doesn't fit
/// its schema. Always a bug in the pack or a damaged store.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("pack: {0}")]
pub struct PackError(pub String);

/// A file type, as a session uses it. Implemented by each pack (Markdown, CSV...).
pub trait Pack: Send + Sync {
    /// The doc model. Its `name` is stamped on each stored doc, and a stored doc only opens
    /// with the pack of the same name.
    fn schema(&self) -> Schema;

    /// File bytes to doc content, or why this file can't be live.
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined>;

    /// Doc content back to file bytes, in the file's own format (`codec` from
    /// [`Pack::decode`]). An untouched file must round-trip byte for byte.
    fn encode(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError>;

    /// Normalise text an editor or agent is inserting into `root` (e.g. `\r\n` to `\n`).
    /// Applied to every inserted string and to `write` bodies before merging.
    fn normalize_input<'a>(&self, root: &str, text: &'a str) -> Cow<'a, str> {
        let _ = root;
        Cow::Borrowed(text)
    }

    /// How this pack's text splits into tokens for D1 (undo spec v1.10): a fresh change
    /// rewrites the tokens it touches wherever someone else wrote them, so it is never built
    /// from someone else's text. Words by default; a code pack would use
    /// [`crate::text::lines::Tokens::Code`].
    fn tokens(&self) -> crate::text::lines::Tokens {
        crate::text::lines::Tokens::Words
    }

    /// Whether the file's bytes put a byte-order mark (U+FEFF, UTF-8 `EF BB BF`) in front
    /// of `root`'s text, under these codec settings. The doc text never holds it, but `cat`
    /// prints the file's bytes, BOM included; so a `write` or `write --base` body that
    /// starts with U+FEFF has that one U+FEFF taken off when this is true (else text read
    /// with `cat` and written back would gain a second BOM).
    ///
    /// Default: for a pack whose schema is one text root, whether [`Pack::encode`] of that
    /// root, empty, starts with the BOM's bytes. A pack with other roots or another encoding
    /// overrides it (or keeps the default, `false`).
    fn writes_bom(&self, root: &str, codec: &[u8]) -> bool {
        let schema = self.schema();
        match schema.roots {
            [(r, Kind::Text)] if *r == root => self
                .encode(&[(root.to_owned(), Neutral::Text(String::new()))], codec)
                .is_ok_and(|b| b.starts_with(text_file::BOM)),
            _ => false,
        }
    }

    /// The pack's own edit verbs (Markdown's `replace-section`), beside the kit's text
    /// tools. [`super::Session::run_tool`] routes a verb no kit tool has by this name, and
    /// [`crate::parity::Registry::for_pack`] registers them for the parity harness. A name
    /// the kit already uses is never routed (the kit's verb wins) and the registry refuses
    /// it. Default: none.
    fn verbs(&self) -> &'static [crate::tools::PackVerb] {
        &[]
    }
}

/// A pack for a plain-text file type in a few lines: one text root named
/// [`TextFilePack::ROOT`], decoded by [`crate::codec::text_file`] (BOM and CRLF restored on
/// encode; NUL, invalid UTF-8, mixed line endings and files over `limit` declined).
///
/// ```
/// use k2_mpc_base::session::TextFilePack;
/// pub const MARKDOWN: TextFilePack = TextFilePack::new("markdown", 4 << 20);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextFilePack {
    name: &'static str,
    limit: usize,
    tokens: crate::text::lines::Tokens,
}

const CODEC_V1: u8 = 1;

impl TextFilePack {
    /// The single text root.
    pub const ROOT: &'static str = "source";

    /// `name` is the schema name (stamped on stored docs); `limit` the largest file, in bytes.
    pub const fn new(name: &'static str, limit: usize) -> Self {
        Self {
            name,
            limit,
            tokens: crate::text::lines::Tokens::Words,
        }
    }

    /// The same pack with another token rule for D1 (v0.9.7; words by default). The undo
    /// rules' worked cases on single letters use [`crate::text::lines::Tokens::Chars`], under
    /// which D1 is the identity, as the oracle's do.
    pub const fn with_tokens(mut self, tokens: crate::text::lines::Tokens) -> Self {
        self.tokens = tokens;
        self
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    fn codec_bytes(f: FileFormat) -> Vec<u8> {
        vec![
            CODEC_V1,
            u8::from(f.bom),
            match f.eol {
                Eol::Lf => 0,
                Eol::Crlf => 1,
            },
        ]
    }

    fn format(codec: &[u8]) -> Result<FileFormat, PackError> {
        match codec {
            [CODEC_V1, bom @ (0 | 1), eol @ (0 | 1)] => Ok(FileFormat {
                bom: *bom == 1,
                eol: if *eol == 1 { Eol::Crlf } else { Eol::Lf },
            }),
            _ => Err(PackError(format!("bad text-file codec settings {codec:?}"))),
        }
    }
}

impl Pack for TextFilePack {
    fn tokens(&self) -> crate::text::lines::Tokens {
        self.tokens
    }

    fn schema(&self) -> Schema {
        Schema {
            name: self.name,
            roots: &[(Self::ROOT, Kind::Text)],
        }
    }

    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        let (text, f) = text_file::decode(bytes, self.limit).map_err(|d| Declined {
            code: d.code(),
            message: d.to_string(),
        })?;
        Ok(PackContent {
            roots: vec![(Self::ROOT.to_owned(), Neutral::Text(text))],
            codec: Self::codec_bytes(f),
        })
    }

    fn encode(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        let f = Self::format(codec)?;
        match content {
            [(root, Neutral::Text(t))] if root == Self::ROOT => Ok(text_file::encode(t, f)),
            _ => Err(PackError(format!(
                "text-file pack expects one root {:?}",
                Self::ROOT
            ))),
        }
    }

    fn normalize_input<'a>(&self, _root: &str, text: &'a str) -> Cow<'a, str> {
        text_file::normalize_input(text)
    }

    fn writes_bom(&self, root: &str, codec: &[u8]) -> bool {
        root == Self::ROOT && Self::format(codec).is_ok_and(|f| f.bom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: TextFilePack = TextFilePack::new("markdown", 64);

    #[test]
    fn round_trips_and_declines() {
        let b = b"\xEF\xBB\xBF# T\r\nx\r\n";
        let c = P.decode(b).unwrap();
        assert_eq!(
            c.roots,
            vec![("source".into(), Neutral::Text("# T\nx\n".into()))]
        );
        assert_eq!(P.encode(&c.roots, &c.codec).unwrap(), b);
        assert_eq!(P.decode(&[b'a'; 65]).unwrap_err().code, "too_large");
        assert_eq!(
            P.decode(b"a\r\nb\n").unwrap_err().code,
            "mixed_line_endings"
        );
        assert_eq!(P.decode(b"\xff").unwrap_err().code, "not_utf8");
        assert!(P.encode(&c.roots, &[9]).is_err(), "bad codec is loud");
        assert!(P.encode(&[], &c.codec).is_err());
        assert_eq!(P.normalize_input("source", "a\r\nb\rc"), "a\nb\nc");
        assert_eq!(P.schema().roots, &[("source", Kind::Text)]);
    }
}
