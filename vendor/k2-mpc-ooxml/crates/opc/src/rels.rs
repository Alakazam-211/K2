//! A relationships part (`_rels/*.rels`): the relationships one source part (or the package)
//! has, each an `Id`, a `Type`, a `Target` and an optional `TargetMode="External"`.
//!
//! A relationship's **identity** is its type, its target (resolved to a part key when it is
//! internal) and its mode, never its `Id`: two saves that each add a relationship pick ids on
//! their own, so the same id can name two different things, and the merge
//! ([`crate::merge`]) pairs relationships by identity and re-mints a colliding id.

use std::collections::BTreeMap;
use std::ops::Range;

use crate::error::Decline;
use crate::limits::Limits;
use crate::name;
use crate::xml;

/// One `Relationship` element, with where it is in the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relationship {
    pub id: String,
    pub rel_type: String,
    /// The target as written.
    pub target: String,
    /// `TargetMode="External"`.
    pub external: bool,
    /// The element's bytes.
    pub range: Range<usize>,
    /// The `Id` value's bytes.
    pub id_range: Range<usize>,
    /// The `Target` value's bytes.
    pub target_range: Range<usize>,
}

/// What a relationship is, apart from its id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Identity {
    pub rel_type: String,
    /// The part key it targets when internal and resolvable, else the target as written.
    pub target: String,
    pub external: bool,
}

impl Relationship {
    /// Its identity, from `source` (the source part's name; `""` for the package).
    pub fn identity(&self, source: &str) -> Identity {
        let target = if self.external {
            self.target.clone()
        } else {
            name::resolve(source, &self.target)
                .map(|t| name::key(&t))
                .unwrap_or_else(|| self.target.clone())
        };
        Identity {
            rel_type: self.rel_type.clone(),
            target,
            external: self.external,
        }
    }

    /// The part key it targets, when internal and resolvable.
    pub fn target_key(&self, source: &str) -> Option<String> {
        if self.external {
            None
        } else {
            name::resolve(source, &self.target).map(|t| name::key(&t))
        }
    }
}

/// A parsed relationships part.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Relationships {
    /// In document order.
    pub list: Vec<Relationship>,
    /// Elements of the root that aren't `Relationship`.
    pub other_elements: usize,
}

impl Relationships {
    /// Parse a relationships part's bytes (UTF-8 XML under the limits).
    pub fn parse(part: &str, bytes: &[u8], limits: &Limits) -> Result<Relationships, Decline> {
        let doc = xml::parse_part(part, bytes, limits)?;
        let mut out = Relationships::default();
        for n in doc.root_element().children().filter(|c| c.is_element()) {
            if n.tag_name().name() != "Relationship" {
                out.other_elements += 1;
                continue;
            }
            let r = n.range();
            let id = n.attribute_node("Id");
            let target = n.attribute_node("Target");
            out.list.push(Relationship {
                id: id.map(|a| a.value().to_owned()).unwrap_or_default(),
                rel_type: n.attribute("Type").unwrap_or("").to_owned(),
                target: target.map(|a| a.value().to_owned()).unwrap_or_default(),
                external: n.attribute("TargetMode") == Some("External"),
                id_range: id.map_or(r.end..r.end, |a| a.range_value()),
                target_range: target.map_or(r.end..r.end, |a| a.range_value()),
                range: r,
            });
        }
        Ok(out)
    }

    /// By id; `None` when an id repeats.
    pub fn by_id(&self) -> Option<BTreeMap<&str, &Relationship>> {
        let mut m = BTreeMap::new();
        for r in &self.list {
            if m.insert(r.id.as_str(), r).is_some() {
                return None;
            }
        }
        Some(m)
    }

    /// The first relationship of a type.
    pub fn of_type(&self, rel_type: &str) -> Option<&Relationship> {
        self.list.iter().find(|r| r.rel_type == rel_type)
    }
}
