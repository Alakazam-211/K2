//! What a file-type pack gives a session: its schema and its file codec.

use std::borrow::Cow;
use std::fmt;

use super::view::View;
use crate::codec::text_file::{self, Eol, FileFormat};
use crate::doc::{Edit, Neutral};
use crate::elem::Key;
use crate::schema::{Kind, Schema};
use crate::table::{self, LineMap, Table};

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

/// What [`Pack::reconcile`] made of a file's new bytes: the edits that make the doc as the
/// file last held it into what the file holds now (one change, applied **by key**), and the
/// file's codec settings now (an outside save can change the dialect).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reconciled {
    pub edits: Vec<Edit>,
    pub codec: Vec<u8>,
    /// A table's rows the file left exact ([`table::Reconciled::exact_rows`], v0.10.0). Any
    /// other kept row merges as one unit: a pack that reports none gets the strict reading, so
    /// it can't leave the row rule off by leaving this out. Ignored for text.
    pub exact_rows: Vec<crate::elem::Key>,
    /// The column mapping was exact ([`table::Reconciled::cols_exact`]): false (the default) is
    /// the strict reading, a remap, which refuses every keyed write from an older base.
    pub cols_exact: bool,
}

/// A pack failed to encode (bad stored codec settings) or returned content that doesn't fit
/// its schema. Always a bug in the pack or a damaged store.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("pack: {0}")]
pub struct PackError(pub String);

/// The element a key of a [composite](Pack::composite) Map root belongs to: its list root and
/// the element's key, which is everything before the key's first
/// [`crate::table::CELL_SEP`]. `None` when `map_root` isn't declared, the key has no separator,
/// or what comes before it isn't a valid [`Key`]. It reads no doc: the element may be live,
/// deleted or unknown.
pub fn elem_of_map_key<'a>(
    composites: &[(&'a str, &'a str)],
    map_root: &str,
    key: &str,
) -> Option<(&'a str, Key)> {
    let (_, list) = composites.iter().find(|(m, _)| *m == map_root)?;
    let (elem, _) = key.split_once(crate::table::CELL_SEP)?;
    Some((*list, Key::new(elem).ok()?))
}

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

    /// Whether `text` may be `root`'s text after a whole-line merge: a format whose
    /// lines depend on each other (JSON's commas and brackets) can refuse a merge that two
    /// valid texts made invalid. The kit asks it of a text doc's merges only, and acts on a
    /// refusal only when both merged texts pass and the merge doesn't:
    /// - an outside save ([`super::Session::file_changed`]): the save is taken whole instead of
    ///   merged. The live text it replaces stays in the "before external edit" version, and
    ///   the removal notes tell its authors what went and which version holds it;
    /// - `write --base`'s clean merge: refused as one conflict over every line either side
    ///   changed ([`crate::text::merge::refused_merge`]); nothing is applied.
    ///
    /// Default: every text passes (the merge is taken as it is).
    fn check_merged(&self, root: &str, text: &str) -> Result<(), Declined> {
        let _ = (root, text);
        Ok(())
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

    /// A block of records in the doc's dialect (a structured verb's stdin: `cells set`'s
    /// pasted block, `row insert`'s values), decoded with the doc's `codec` settings. The kit
    /// never parses a file format itself: a table pack implements this with its codec.
    /// Default: declined `unsupported` (the verb exits 2 with the reason).
    fn decode_block(&self, bytes: &[u8], codec: &[u8]) -> Result<Vec<table::Record>, Declined> {
        let _ = (bytes, codec);
        Err(Declined {
            code: "unsupported",
            message: "this pack reads no block of records from stdin".into(),
        })
    }

    /// `header on|off` on a table (the grid verbs' `header`): the edits that switch `t`'s
    /// header, applied to `t`. **The rule lives in [`table::Table::set_header`]** (the switch
    /// never changes the rendered bytes; a blank only row can't become the header), and the
    /// default delegates to it. A pack overrides this only to decline more for its format
    /// (the verb exits 2 with the reason, nothing applied); it never re-implements the rule.
    fn set_header(
        &self,
        t: &mut table::Table,
        on: bool,
        codec: &[u8],
    ) -> Result<Vec<crate::doc::Edit>, Declined> {
        let _ = codec;
        t.set_header(on).map_err(|e| Declined {
            // the refusal with its own id keeps its name (MPC-E806 at the verb)
            code: match e {
                table::TableError::HeaderNamesUnrendered => "header_names_unrendered",
                table::TableError::HeaderOnlyRowBlank => "header_only_row_blank",
                _ => "refused",
            },
            message: e.to_string(),
        })
    }

    /// How an outside save changed only the file's codec settings (a CSV re-saved with a BOM,
    /// CRLF or another delimiter: the content the same), from `old` to `new`, for the log and
    /// `show` (computed when read, never stored). Default: "codec settings changed".
    fn describe_codec_change(&self, old: &[u8], new: &[u8]) -> String {
        let _ = (old, new);
        "codec settings changed".to_owned()
    }

    /// The pack's own edit verbs (Markdown's `replace-section`), beside the kit's text
    /// tools. [`super::Session::run_tool`] routes a verb no kit tool has by this name, and
    /// [`crate::parity::Registry::for_pack`] registers them for the parity harness. A name
    /// the kit already uses is never routed (the kit's verb wins) and the registry refuses
    /// it. Default: none.
    fn verbs(&self) -> &'static [crate::tools::PackVerb] {
        &[]
    }

    /// The native ID kinds this pack's codec and own verbs read (K-ID-13), as data: each
    /// kind's class, scope and the verbs that accept it. The pack's `docs/IDS.md` is
    /// generated from it ([`crate::ids::markdown`]) and checked against its registry
    /// ([`crate::ids::check`]). List only the pack's own kinds (Markdown's headings, docx's
    /// `w14:paraId`): the kit's (lines, `L:C`, grid keys, `#N`, versions...) are
    /// [`crate::ids::KIT`]. Default: none (a format with no IDs of its own, as plain text).
    fn ids(&self) -> &'static [crate::ids::IdKind] {
        &[]
    }

    /// The MovableList roots whose keys are **tagged** ([`crate::mint`]), each with the key
    /// prefixes it takes: `(root, prefixes)`. An insert arriving from a connection into such a
    /// root must name `<prefix><n>.<tag>` with one of the prefixes and the connection's own
    /// writer tag ([`super::SessionError::KeyNotOwn`], exit 2); [`super::Session::mint_key`]
    /// makes one. Imports, outside saves and keys the list already knows are not checked.
    /// Default: none.
    fn tagged_keys(&self) -> &'static [(&'static str, &'static [&'static str])] {
        &[]
    }

    /// The Map root that holds the retired marks of [`Pack::tagged_keys`]' roots
    /// ([`crate::mint::mark_key`]). A change that drops tagged keys writes the dropper's own
    /// marks there in the same change ([`crate::mint::marks_for_drops`]), and
    /// [`super::Session::mint_key`] reads the max over every dropper's. `None` (the default):
    /// [`super::Session::mint_key`] reads no marks.
    fn mint_marks(&self) -> Option<&'static str> {
        None
    }

    /// The Map roots whose keys belong to list elements, as `(map_root, list_root)` pairs: each
    /// key of `map_root` is an element key of `list_root`, then [`crate::table::CELL_SEP`], then
    /// the rest (`<item>\x1f<field>`). So a write to such a key counts as touching its element
    /// ([`elem_of_map_key`]; a claim on the element covers it). **It fails open:** a key that
    /// maps to no element isn't covered by any claim (one with no separator, or whose start
    /// isn't a valid [`Key`]), so a pack writes only keys of this shape to these roots. A map root
    /// appears once: the first pair naming it is the one read. Default: none.
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }

    /// The list and map roots whose values may name a blob ([`crate::blob::BlobId::parse_ref`]:
    /// a value that is exactly `blob:<64 hex>`). The doc holds a reference on each blob such a
    /// value names, so blob GC keeps it while any record of the doc names it; a connection's
    /// edit naming a blob this doc's space doesn't hold is refused
    /// ([`super::SessionError::BlobRefUnknown`], exit 4). A value in any other root is ordinary
    /// text, whatever it spells. Default: none.
    fn blob_ref_roots(&self) -> &'static [&'static str] {
        &[]
    }

    /// A file has no keys (A8): its new bytes (an outside save, a text edit verb's result,
    /// an agent's `write --base` body), as edits against `synced`, the doc as the file last
    /// held it. A structured doc's session applies them **by key** to the live doc (CSV
    /// seam design §7.7), so changes the file never got are kept. For a table, `synced`'s
    /// key counters are the live doc's, so keys minted here are new to the live doc too.
    ///
    /// Default: [`default_reconcile`] (decode, then a minimal text diff per text root, or
    /// for a [`crate::table`] schema the aligner [`table::reconcile()`]). A file that doesn't
    /// decode is [`Declined`] (it opens the old way until it decodes again).
    fn reconcile(&self, synced: &View, bytes: &[u8], codec: &[u8]) -> Result<Reconciled, Declined> {
        let _ = codec;
        default_reconcile(self, synced, bytes)
    }

    /// Which element each rendered line of `content` belongs to (A8: `L42` addressing, blame
    /// lines, views), or `None` when lines are the text itself (a text pack). Default: for a
    /// [`crate::table`] schema, [`Table::line_map`]; a codec that writes lines of its own in
    /// front (a `sep=;` preamble) overrides it to account for them.
    fn line_map(&self, content: &View, codec: &[u8]) -> Option<LineMap> {
        let _ = codec;
        if table::is_table(&self.schema()) {
            content.table().ok().map(|t| t.line_map())
        } else {
            None
        }
    }
}

/// [`Pack::reconcile`]'s default: decode `bytes`, then
/// - every root text: each text root's edit from `synced`'s text, whole lines first
///   ([`crate::text::lines::edit_hunks`]);
/// - a [`crate::table`] schema: the decoded table's rendering, aligned to `synced`'s table
///   by [`table::reconcile()`] (rows and columns matched by content, keys kept, moves found);
/// - anything else: declined `cannot_reconcile` (the pack overrides it).
pub fn default_reconcile<P: Pack + ?Sized>(
    pack: &P,
    synced: &View,
    bytes: &[u8],
) -> Result<Reconciled, Declined> {
    let content = pack.decode(bytes)?;
    let schema = pack.schema();
    let cant = |why: String| Declined {
        code: "cannot_reconcile",
        message: why,
    };
    if schema.roots.iter().all(|(_, k)| *k == Kind::Text) {
        let mut edits = Vec::new();
        for (root, n) in &content.roots {
            let new = n
                .as_text()
                .ok_or_else(|| cant(format!("root {root:?} isn't text")))?;
            let old = synced.text(root).unwrap_or_default();
            // whole lines first, as every edit given as a new text (v0.9.7.1's one pairing
            // rule): a character-trimmed diff pairs lines that start alike one line off
            let hunks = crate::text::lines::edit_hunks(old, new);
            if !hunks.is_empty() {
                edits.push(Edit::Text {
                    root: root.clone(),
                    hunks,
                });
            }
        }
        return Ok(Reconciled {
            edits,
            codec: content.codec,
            exact_rows: Vec::new(),
            cols_exact: false,
        });
    }
    if !table::is_table(&schema) {
        return Err(cant(format!(
            "pack {:?} has structured roots and no reconcile",
            schema.name
        )));
    }
    let old = synced.table().map_err(|e| cant(e.to_string()))?;
    let new = Table::from_keyed(View::of_content(&content.roots).keyed())
        .map_err(|e| cant(e.to_string()))?;
    let records = new.render();
    let r = table::reconcile(&old, &records).map_err(|e| cant(e.to_string()))?;
    Ok(Reconciled {
        edits: r.edits,
        codec: content.codec,
        exact_rows: r.exact_rows,
        cols_exact: r.cols_exact,
    })
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
