//! XML under the limits, and the few helpers the package layer needs. Nothing is ever
//! re-serialised: callers splice byte ranges ([`roxmltree`] gives every node's and attribute's
//! range in the source text).

use roxmltree::{Document, ParsingOptions};

use crate::error::Decline;
use crate::limits::Limits;

/// The relationships namespace for references from a part's XML (`r:id`, `r:embed`, ...).
pub const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
/// Strict OOXML's relationships namespace.
pub const R_STRICT: &str = "http://purl.oclc.org/ooxml/officeDocument/relationships";
/// The namespace of a `.rels` part's elements.
pub const PKG_RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
/// The namespace of `[Content_Types].xml`'s elements.
pub const PKG_TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
/// The legacy VML office namespace, whose `relid` attribute also names a relationship.
pub const VML_OFFICE: &str = "urn:schemas-microsoft-com:office:office";

/// Whether a part is read as XML by its name (`.xml`, `.rels`).
pub fn is_xml_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".xml") || n.ends_with(".rels")
}

/// Whether the bytes hold a `DOCTYPE` anywhere, in any case.
pub fn has_doctype(content: &[u8]) -> bool {
    content
        .windows(9)
        .any(|w| w.eq_ignore_ascii_case(b"<!doctype"))
}

/// Whether the bytes start with a UTF-16 byte order mark.
pub fn is_utf16(content: &[u8]) -> bool {
    content.starts_with(&[0xFF, 0xFE]) || content.starts_with(&[0xFE, 0xFF])
}

/// Parse XML under the limits: no DTD (a `DOCTYPE` is `doctype`), a node cap, a depth cap.
pub fn parse_xml<'x>(xml: &'x str, limits: &Limits) -> Result<Document<'x>, Decline> {
    #[allow(clippy::field_reassign_with_default)]
    let opt = {
        let mut opt = ParsingOptions::default();
        opt.allow_dtd = false;
        opt.nodes_limit = limits.max_nodes;
        opt
    };
    let doc = Document::parse_with_options(xml, opt).map_err(|e| match e {
        roxmltree::Error::DtdDetected => Decline::new("doctype", "the XML has a DOCTYPE"),
        roxmltree::Error::NodesLimitReached => {
            Decline::new("too_large", "the XML has too many nodes")
        }
        other => Decline::new("xml_invalid", format!("invalid XML: {other}")),
    })?;
    // an iterative walk: nothing recurses before the depth is known
    let root = doc.root();
    let (mut cur, mut depth) = (root, 0usize);
    'walk: loop {
        if let Some(c) = cur.first_child() {
            cur = c;
            depth += 1;
            if depth > limits.max_depth {
                return Err(Decline::new("too_large", "the XML is nested too deep"));
            }
            continue;
        }
        loop {
            if cur == root {
                break 'walk;
            }
            if let Some(s) = cur.next_sibling() {
                cur = s;
                break;
            }
            match cur.parent() {
                Some(p) => {
                    cur = p;
                    depth -= 1;
                }
                None => break 'walk,
            }
        }
    }
    Ok(doc)
}

/// Parse a part's bytes as UTF-8 XML under the limits.
pub fn parse_part<'x>(
    name: &str,
    bytes: &'x [u8],
    limits: &Limits,
) -> Result<Document<'x>, Decline> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Decline::new("xml_encoding", format!("{name:?} isn't UTF-8")))?;
    parse_xml(text, limits)
}

/// An attribute value with `&`, `<`, `>`, `"` and `'` escaped.
pub fn escape_attr(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// The qualified name of an element as written in the source (`Relationships`, `pr:Types`).
pub fn qname<'x>(text: &'x str, node: roxmltree::Node<'_, 'x>) -> &'x str {
    let r = node.range();
    let start = r.start + 1;
    let rest = &text[start..r.end];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(rest.len());
    &rest[..end]
}

/// Splice `edits` (non-overlapping byte ranges and their replacements, in any order) into
/// `text`.
pub fn splice(text: &str, mut edits: Vec<(std::ops::Range<usize>, String)>) -> String {
    edits.sort_by_key(|e| (e.0.start, e.0.end));
    let mut out = String::with_capacity(text.len() + 64);
    let mut at = 0;
    for (r, s) in edits {
        out.push_str(&text[at..r.start]);
        out.push_str(&s);
        at = r.end;
    }
    out.push_str(&text[at..]);
    out
}
