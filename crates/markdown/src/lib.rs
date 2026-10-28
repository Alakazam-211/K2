//! Markdown type pack for the K2 multiplayer kit.
//!
//! - [`SCHEMA`]: one text root, [`ROOT`], holding the file's Markdown source. Plain text
//!   over the source (not a block model), so every dialect, front matter and HTML survives
//!   untouched, and line tools map directly onto it.
//! - [`codec`]: file bytes <-> doc text, byte-exact (BOM and CRLF restored), with the
//!   Markdown limits and decline rules.
//! - [`PACK`]: the pack a host opens Markdown docs with: the schema, the codec, and
//!   Markdown's own verb, `replace-section` ([`section`]).
//! - Verb map and parity list: `docs/PARITY.md`, and the same list as data ([`parity`]).

use std::borrow::Cow;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::session::{Declined, Pack, PackContent, PackError, TextFilePack};
use k2_mpc_base::tools::PackVerb;
use k2_mpc_base::{Kind, Schema};

pub mod codec;
pub mod parity;
pub mod section;

/// Name of the root text. Editor bindings attach to it.
pub const ROOT: &str = "source";

/// The Markdown data model on the kit's primitives.
pub const SCHEMA: Schema = Schema {
    name: "markdown",
    roots: &[(ROOT, Kind::Text)],
};

/// Largest file that can be a live doc (bytes on disk). Larger files open the old way.
pub const MAX_FILE_BYTES: usize = 4 << 20;

/// Extensions that are live Markdown docs (lowercase, no dot). `.mdx` is not: it is JSX.
pub const EXTENSIONS: &[&str] = &["md", "markdown"];

/// The kit's plain-text pack under Markdown's name, root and limit.
const TEXT: TextFilePack = TextFilePack::new("markdown", MAX_FILE_BYTES);

/// The Markdown pack: what a host passes to `SessionManager::open_file`. The kit's
/// plain-text codec (BOM and line endings restored) under [`SCHEMA`], plus Markdown's own
/// verb, `replace-section`, which `Session::run_tool` runs like the kit's edit verbs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MarkdownPack;

/// The Markdown pack.
pub const PACK: MarkdownPack = MarkdownPack;

/// Markdown's own verbs, beside the kit's text tools.
pub const VERBS: &[PackVerb] = &[section::REPLACE_SECTION];

impl Pack for MarkdownPack {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        TEXT.decode(bytes)
    }
    fn encode(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        TEXT.encode(content, codec)
    }
    fn normalize_input<'a>(&self, root: &str, text: &'a str) -> Cow<'a, str> {
        TEXT.normalize_input(root, text)
    }
    fn verbs(&self) -> &'static [PackVerb] {
        VERBS
    }
}

/// True when `path` has a live-Markdown extension.
pub fn is_markdown_path(path: &str) -> bool {
    path.rsplit_once('.')
        .map(|(stem, ext)| {
            !stem.is_empty()
                && !stem.ends_with('/')
                && EXTENSIONS.iter().any(|e| e.eq_ignore_ascii_case(ext))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    #[test]
    fn extensions() {
        for p in ["a.md", "dir/b.MD", "c.markdown", ".k2/wiki/Home.md"] {
            assert!(super::is_markdown_path(p), "{p}");
        }
        for p in ["a.mdx", "md", "a.txt", "dir/.md", ".md"] {
            assert!(!super::is_markdown_path(p), "{p}");
        }
    }

    #[test]
    fn the_pack_is_the_text_pack_plus_its_verb() {
        use k2_mpc_base::session::Pack;
        let p = super::PACK;
        assert_eq!(p.schema(), super::SCHEMA);
        assert_eq!(p.schema(), super::TEXT.schema());
        let c = p.decode(b"\xEF\xBB\xBF# T\r\n").unwrap();
        assert_eq!(
            p.encode(&c.roots, &c.codec).unwrap(),
            b"\xEF\xBB\xBF# T\r\n"
        );
        let names: Vec<&str> = p.verbs().iter().map(|v| v.name()).collect();
        assert_eq!(names, ["replace-section"]);
    }

    #[test]
    fn schema_builds_a_doc() {
        let d = k2_mpc_base::Doc::new(super::SCHEMA).unwrap();
        assert_eq!(d.text(super::ROOT).unwrap(), "");
    }
}
