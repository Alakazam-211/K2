//! Plain-text type pack for the K2 multiplayer kit: code, configuration (JSON, YAML, TOML
//! as text), markup (HTML, XML, SVG source) and diagram sources (Mermaid, PlantUML,
//! Graphviz, draw.io XML), one pack for all of them.
//!
//! - [`SCHEMA`]: one text root, [`ROOT`], holding the file's text. Plain text over the
//!   source, so every language, comment and blank line survives, and the kit's line tools
//!   map directly onto it.
//! - [`types`]: the file-type table. Each extension (or well-known name such as `Makefile`)
//!   maps to a [`Family`] and codec [`Settings`]; anything else is decided by the strict
//!   guard ([`types::UNKNOWN`]).
//! - [`codec`]: file bytes <-> doc text, byte-exact. BOM, line endings (LF, CRLF or CR) and
//!   the final newline are kept; nothing is normalised; a file that can't round-trip is
//!   refused with a typed reason, never mangled.
//! - [`TextPack`]: the pack a host opens a text file with, for that file's type
//!   ([`TextPack::for_path`]). [`PACK`] is the pack for files of unknown type.
//! - Verb map and parity list: `docs/PARITY.md`, and the same list as data ([`parity`]).
//!   The kit's text verbs cover every row; this pack adds none.

use std::borrow::Cow;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::session::{Declined, Pack, PackContent, PackError};
use k2_mpc_base::{Kind, Schema};

pub mod codec;
pub mod parity;
pub mod types;

pub use codec::{Controls, Eol, Format, Refusal, Settings};
pub use types::{lookup, Family, FileType, Lookup};

/// Name of the root text. Editor bindings attach to it. (The kit's `TextFilePack::ROOT`.)
pub const ROOT: &str = "source";

/// The data model on the kit's primitives. One schema for every type in the table, so a
/// doc keeps its history when its file is renamed from one text type to another.
pub const SCHEMA: Schema = Schema {
    name: "text",
    roots: &[(ROOT, Kind::Text)],
};

/// The text pack for one file type: what a host passes to `SessionManager::open_file`.
///
/// Every `TextPack` has the same schema and writes files the same way; the type only sets
/// how a file is read in (its size cap, its control-byte guard, and the line-ending style
/// for new line breaks in a file that has none yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPack {
    ty: FileType,
}

/// The pack for a file of unknown type (the strictest guard). Use [`TextPack::for_path`]
/// for a file whose name is known.
pub const PACK: TextPack = TextPack::new(types::UNKNOWN);

impl TextPack {
    pub const fn new(ty: FileType) -> Self {
        Self { ty }
    }

    /// The pack for the file at `path` (only its name counts), or `None` when the table
    /// says it is never text ([`Lookup::NotText`]).
    pub fn for_path(path: &str) -> Option<Self> {
        lookup(path).file_type().map(Self::new)
    }

    pub fn file_type(&self) -> FileType {
        self.ty
    }

    pub fn family(&self) -> Family {
        self.ty.family
    }

    pub fn settings(&self) -> Settings {
        self.ty.settings
    }
}

impl Default for TextPack {
    fn default() -> Self {
        PACK
    }
}

impl Pack for TextPack {
    fn schema(&self) -> Schema {
        SCHEMA
    }

    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        let (text, f) = codec::decode(bytes, &self.ty.settings)?;
        Ok(PackContent {
            roots: vec![(ROOT.to_owned(), Neutral::Text(text))],
            codec: codec::codec_bytes(f),
        })
    }

    fn encode(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        let f = codec::parse_codec(codec)?;
        match content {
            [(root, Neutral::Text(t))] if root == ROOT => codec::encode(t, f),
            _ => Err(PackError(format!("text pack expects one root {ROOT:?}"))),
        }
    }

    fn normalize_input<'a>(&self, _root: &str, text: &'a str) -> Cow<'a, str> {
        codec::normalize_input(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_mpc_base::session::TextFilePack;

    #[test]
    fn the_pack_round_trips_and_refuses() {
        let p = TextPack::for_path("run.bat").unwrap();
        assert_eq!(p.family(), Family::Code);
        let c = p.decode(b"\xEF\xBB\xBF@echo off\r\nexit\r\n").unwrap();
        assert_eq!(
            c.roots,
            vec![("source".into(), Neutral::Text("@echo off\nexit\n".into()))]
        );
        assert_eq!(
            p.encode(&c.roots, &c.codec).unwrap(),
            b"\xEF\xBB\xBF@echo off\r\nexit\r\n"
        );
        let d = p.decode(b"a\r\nb\n").unwrap_err();
        assert_eq!(d.code, "mixed_line_endings");
        assert!(p.encode(&c.roots, &[9]).is_err(), "bad codec is loud");
        assert!(p.encode(&[], &c.codec).is_err());
        assert!(p
            .encode(&[("other".into(), Neutral::Text(String::new()))], &c.codec)
            .is_err());
        assert_eq!(p.normalize_input(ROOT, "a\r\nb\rc"), "a\nb\nc");
        assert!(
            p.verbs().is_empty(),
            "the kit's verbs cover text; no pack verbs"
        );
    }

    #[test]
    fn every_type_shares_one_schema_and_one_writer() {
        let bytes = b"x = 1\r\ny = 2\r\n";
        let mut packs: Vec<TextPack> = types::TABLE.iter().map(|g| TextPack::new(g.ty)).collect();
        packs.push(PACK);
        let c = PACK.decode(bytes).unwrap();
        for p in packs {
            assert_eq!(p.schema(), SCHEMA);
            assert_eq!(p.encode(&c.roots, &c.codec).unwrap(), bytes);
        }
        // the kit's text-file pack: same root, and its stored settings read the same here
        let kit = TextFilePack::new("markdown", 1 << 20);
        assert_eq!(kit.schema().roots, SCHEMA.roots);
        let k = kit.decode(bytes).unwrap();
        assert_eq!(PACK.encode(&k.roots, &k.codec).unwrap(), bytes);
    }

    #[test]
    fn for_path() {
        assert_eq!(TextPack::for_path("a.png"), None);
        assert_eq!(TextPack::for_path("a.xyz"), Some(PACK));
        assert_eq!(TextPack::default(), PACK);
        assert_eq!(
            TextPack::for_path("Cargo.toml").unwrap().family(),
            Family::Config
        );
    }

    #[test]
    fn schema_builds_a_doc() {
        let d = k2_mpc_base::Doc::new(SCHEMA).unwrap();
        assert_eq!(d.text(ROOT).unwrap(), "");
    }
}
