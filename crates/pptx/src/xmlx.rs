//! The XML helpers this pack shares: namespaces, element lookups, prefixes, escaping. Nothing is
//! ever re-serialised: callers splice byte ranges ([`k2_mpc_opc::xml::splice`]).

use roxmltree::Node;

pub use k2_mpc_opc::xml::{escape_attr, parse_xml, qname, splice, R};

/// PresentationML (transitional).
pub const P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
/// PresentationML in Strict OOXML: declined.
pub const P_STRICT: &str = "http://purl.oclc.org/ooxml/presentationml/main";
/// DrawingML.
pub const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
/// Markup compatibility (`mc:AlternateContent`).
pub const MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";

/// Whether `n` is the element `{ns}local`.
pub fn is(n: Node<'_, '_>, ns: &str, local: &str) -> bool {
    n.is_element() && n.tag_name().name() == local && n.tag_name().namespace() == Some(ns)
}

/// The first child element `{ns}local` of `n`.
pub fn child<'a, 'i>(n: Node<'a, 'i>, ns: &str, local: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| is(*c, ns, local))
}

/// Every child element `{ns}local` of `n`.
pub fn children<'a, 'i: 'a>(
    n: Node<'a, 'i>,
    ns: &'a str,
    local: &'a str,
) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children().filter(move |c| is(*c, ns, local))
}

/// The prefix an element is written with in the source (`p` for `<p:sp>`, `""` for none).
pub fn prefix_of<'x>(text: &'x str, n: Node<'_, 'x>) -> &'x str {
    qname(text, n).rsplit_once(':').map_or("", |(p, _)| p)
}

/// `prefix:local`, or `local` with an empty prefix.
pub fn q(prefix: &str, local: &str) -> String {
    if prefix.is_empty() {
        local.to_owned()
    } else {
        format!("{prefix}:{local}")
    }
}

/// Where an element's start tag ends (just past its `>`), and whether it is empty (`<x/>`).
pub fn start_tag_end(text: &str, n: Node<'_, '_>) -> (usize, bool) {
    let r = n.range();
    let whole = &text[r.clone()];
    // the first `>` outside a quoted attribute value
    let (mut quote, mut i) = (None::<u8>, 0usize);
    let b = whole.as_bytes();
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => quote = Some(c),
            None if c == b'>' => {
                let empty = i > 0 && b[i - 1] == b'/';
                return (r.start + i + 1, empty);
            }
            None => {}
        }
        i += 1;
    }
    (r.end, true)
}

/// Where an element's end tag starts (its content ends there); for an empty element, `None`.
pub fn end_tag_start(text: &str, n: Node<'_, '_>) -> Option<usize> {
    let (_, empty) = start_tag_end(text, n);
    if empty {
        return None;
    }
    let r = n.range();
    text[r.clone()].rfind("</").map(|i| r.start + i)
}

/// Text for an element's content: `&`, `<` and `>` escaped. `None` when it holds a character XML
/// 1.0 can't carry (a control character other than tab, line feed and carriage return).
pub fn escape_text(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => return None,
            c => out.push(c),
        }
    }
    Some(out)
}

/// Lowercase hex.
pub fn hex(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 15) as usize] as char);
    }
    s
}

/// `s` cut to at most `max` bytes, on a character boundary.
pub fn cut(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

/// The plain text of every DrawingML paragraph under `n`: runs and fields as written, a line
/// break as `\n` inside its paragraph, paragraphs joined by `\n`.
pub fn paragraphs_text(n: Node<'_, '_>) -> String {
    let mut out: Vec<String> = Vec::new();
    for p in n.descendants().filter(|d| is(*d, A, "p")) {
        let mut s = String::new();
        for d in p.descendants() {
            if is(d, A, "t") {
                s.push_str(d.text().unwrap_or(""));
            } else if is(d, A, "br") {
                s.push('\n');
            }
        }
        out.push(s);
    }
    out.join("\n")
}
