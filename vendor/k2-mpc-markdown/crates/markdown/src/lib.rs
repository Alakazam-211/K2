//! Markdown type pack for the K2 multiplayer kit.
//!
//! - [`SCHEMA`]: one text root, [`ROOT`], holding the file's Markdown source. Plain text
//!   over the source (not a block model), so every dialect, front matter and HTML survives
//!   untouched, and line tools map directly onto it.
//! - [`codec`]: file bytes <-> doc text, byte-exact (BOM and CRLF restored), with the
//!   Markdown limits and decline rules.
//! - Verb map and parity list: `docs/PARITY.md`.

use k2_mpc_base::{Kind, Schema};

pub mod codec;

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
    fn schema_builds_a_doc() {
        let d = k2_mpc_base::Doc::new(super::SCHEMA).unwrap();
        assert_eq!(d.text(super::ROOT).unwrap(), "");
    }
}
