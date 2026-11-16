//! XML for the XML formats: UTF-8 only, no document type with its own declarations, a node
//! limit, and a refusal for anything not well-formed.

use crate::error::{ReadError, ReadLimits};

/// `bytes` as UTF-8 text, a document type with declarations refused.
pub(crate) fn text(bytes: &[u8]) -> Result<&str, ReadError> {
    let text = std::str::from_utf8(bytes).map_err(|e| ReadError::Syntax {
        format: "utf-8",
        at: e.valid_up_to() as u64,
        why: "not UTF-8",
    })?;
    refuse_internal_subset(text)?;
    Ok(text)
}

/// Refuses a document type with its own declarations (`<!DOCTYPE x [ … ]>`): entity
/// declarations are how XML expands a small file into a huge one, and these formats have none.
fn refuse_internal_subset(text: &str) -> Result<(), ReadError> {
    if let Some(i) = text.find("<!DOCTYPE") {
        let rest = &text[i..];
        let end = rest.find('>').unwrap_or(rest.len());
        if rest[..end].contains('[') {
            return Err(ReadError::Structure {
                why: "a document type with its own declarations",
            });
        }
    }
    Ok(())
}

/// Parses XML text under the node limit, refusing what isn't well-formed.
pub(crate) fn parse<'a>(
    text: &'a str,
    limits: &ReadLimits,
) -> Result<roxmltree::Document<'a>, ReadError> {
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: u32::try_from(limits.max_items).unwrap_or(u32::MAX),
        // no resolver: an external entity is never fetched or read
        ..roxmltree::ParsingOptions::default()
    };
    roxmltree::Document::parse_with_options(text, opts).map_err(|e| match e {
        roxmltree::Error::NodesLimitReached => ReadError::Limit {
            what: "XML nodes",
            limit: limits.max_items,
        },
        _ => ReadError::Syntax {
            format: "xml",
            at: 0,
            why: "not well-formed XML",
        },
    })
}

/// An element's attribute by its plain name: the one attribute of that name in no namespace.
/// roxmltree's own lookup by a plain name also matches a namespaced attribute with that local
/// name (`x:name` for `name`), whichever comes first; these readers never take one for the
/// other.
pub(crate) trait PlainAttr<'a> {
    /// The value of the attribute named `name` in no namespace, if there is one.
    fn plain(&self, name: &str) -> Option<&'a str>;
}

impl<'a> PlainAttr<'a> for roxmltree::Node<'a, '_> {
    fn plain(&self, name: &str) -> Option<&'a str> {
        if crate::mutant::active("AttrByLocalName") {
            // planted: roxmltree's lookup, which a namespaced attribute can answer
            return self.attribute(name);
        }
        self.attributes()
            .find(|a| a.namespace().is_none() && a.name() == name)
            .map(|a| a.value())
    }
}
