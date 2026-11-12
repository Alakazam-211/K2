//! `[Content_Types].xml`: a part's content type is its `Override` (by part name), else its
//! extension's `Default`. Keys are case folded, as OPC compares part names and extensions.

use std::collections::BTreeMap;
use std::ops::Range;

use crate::error::Decline;
use crate::limits::Limits;
use crate::xml;

/// Which kind of entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// `Default`, keyed by extension.
    Default,
    /// `Override`, keyed by part name.
    Override,
}

/// One `Default` or `Override` element, with where it is in the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeEntry {
    pub kind: Kind,
    /// The extension or part name, case folded, without a leading `/`.
    pub key: String,
    /// The extension or part name as written.
    pub raw_key: String,
    /// The content type.
    pub value: String,
    /// The element's bytes.
    pub range: Range<usize>,
    /// The `ContentType` value's bytes.
    pub value_range: Range<usize>,
    /// The key attribute's value bytes.
    pub key_range: Range<usize>,
}

/// A parsed `[Content_Types].xml`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContentTypes {
    /// Defaults by extension (case folded). A repeated extension keeps the last.
    pub defaults: BTreeMap<String, String>,
    /// Overrides by part key (case folded, no leading `/`). A repeated name keeps the last.
    pub overrides: BTreeMap<String, String>,
    /// Every entry in document order, with its byte ranges.
    pub entries: Vec<TypeEntry>,
    /// Elements of the root that are neither `Default` nor `Override`.
    pub other_elements: usize,
}

impl ContentTypes {
    /// Parse the part's bytes (UTF-8 XML under the limits).
    pub fn parse(bytes: &[u8], limits: &Limits) -> Result<ContentTypes, Decline> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| Decline::new("xml_encoding", "the content types aren't UTF-8"))?;
        let doc = xml::parse_xml(text, limits)?;
        let mut out = ContentTypes::default();
        for n in doc.root_element().children().filter(|c| c.is_element()) {
            let (kind, key_attr) = match n.tag_name().name() {
                "Override" => (Kind::Override, "PartName"),
                "Default" => (Kind::Default, "Extension"),
                _ => {
                    out.other_elements += 1;
                    continue;
                }
            };
            let ct = n.attribute_node("ContentType");
            let k = n.attribute_node(key_attr);
            let value = ct.map(|a| a.value().to_owned()).unwrap_or_default();
            let raw_key = k.map(|a| a.value().to_owned()).unwrap_or_default();
            let key = match kind {
                Kind::Override => crate::name::key(&raw_key),
                Kind::Default => raw_key.to_ascii_lowercase(),
            };
            match kind {
                Kind::Override => out.overrides.insert(key.clone(), value.clone()),
                Kind::Default => out.defaults.insert(key.clone(), value.clone()),
            };
            let r = n.range();
            out.entries.push(TypeEntry {
                kind,
                key,
                raw_key,
                value,
                value_range: ct.map_or(r.end..r.end, |a| a.range_value()),
                key_range: k.map_or(r.end..r.end, |a| a.range_value()),
                range: r,
            });
        }
        Ok(out)
    }

    /// A part's content type (`part` is a key): its override, else its extension's default.
    pub fn of(&self, part: &str) -> Option<&str> {
        if let Some(t) = self.overrides.get(part) {
            return Some(t.as_str());
        }
        let ext = part.rsplit('/').next()?.rsplit_once('.')?.1;
        self.defaults.get(ext).map(String::as_str)
    }

    /// Whether any key is written more than once.
    pub fn repeated(&self) -> Option<&str> {
        let mut seen = std::collections::BTreeSet::new();
        self.entries
            .iter()
            .find(|e| !seen.insert((e.kind, e.key.as_str())))
            .map(|e| e.key.as_str())
    }
}
