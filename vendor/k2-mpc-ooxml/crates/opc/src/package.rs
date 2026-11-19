//! An open package: the zip, every entry's content (held to its CRC), the content types, and
//! a writer that splices only what changed. The format's refusals, each a typed [`Decline`]:
//!
//! | Code | What |
//! |---|---|
//! | `too_large` | the file is over [`Limits::max_file`] |
//! | `encrypted` | an encrypted package (a compound file, not a zip) |
//! | the [`crate::zip`] codes | the zip itself, including `zip_duplicate` (two entries naming one part) |
//! | `zip_layout` | a package that doesn't write back to its own bytes |
//! | `xml_encoding` | an XML part (`.xml`, `.rels`) in UTF-16, or content types that aren't UTF-8 |
//! | `doctype` | a `DOCTYPE` in any XML part, read or carried |
//! | `no_content_types` | no `[Content_Types].xml` |
//! | `xml_invalid`, `too_large` | content types that don't parse, or over the XML limits |
//!
//! A pack adds its own refusals (macros, a template, the wrong main part) with
//! [`OpcPackage::open_with`]'s per-entry check and after opening.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::Decline;
use crate::limits::Limits;
use crate::mutant;
use crate::name::{self, CONTENT_TYPES};
use crate::rels::Relationships;
use crate::types::ContentTypes;
use crate::xml;
use crate::zip::{self, Package, Source, Span};

/// The first bytes of a compound file: an encrypted package, or a legacy binary format.
pub const CFB_MAGIC: &[u8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// One entry's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// Its index in the zip's entries.
    pub index: usize,
    /// Its name as written in the zip.
    pub name: String,
    pub content: Vec<u8>,
}

/// An open package.
#[derive(Debug, Clone)]
pub struct OpcPackage<'a> {
    /// The zip, borrowing the file's bytes.
    pub zip: Package<'a>,
    /// Every entry by key (case folded, no leading `/`): the parts, the relationships parts,
    /// `[Content_Types].xml`, and any folder entries (keys ending in `/`).
    pub parts: BTreeMap<String, Part>,
    /// The parsed content types.
    pub types: ContentTypes,
}

/// What [`OpcPackage::write`] does to one part, by key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// New content for an existing part (content equal to the old keeps the raw record).
    Replace(Vec<u8>),
    /// Leave the part out.
    Remove,
    /// A new part, with its name as it will be written.
    Add { name: String, content: Vec<u8> },
}

impl<'a> OpcPackage<'a> {
    /// Open a package with no format checks of a pack's.
    pub fn open(bytes: &'a [u8], limits: &Limits) -> Result<OpcPackage<'a>, Decline> {
        Self::open_with(bytes, limits, |_, _| Ok(()))
    }

    /// Open a package: every entry is inflated and checked, and the package must write back to
    /// exactly these bytes; if it couldn't, it is declined rather than merged. `check` sees
    /// each entry's name and content in zip order, after this crate's own checks of it.
    pub fn open_with(
        bytes: &'a [u8],
        limits: &Limits,
        mut check: impl FnMut(&str, &[u8]) -> Result<(), Decline>,
    ) -> Result<OpcPackage<'a>, Decline> {
        if bytes.len() > limits.max_file {
            return Err(Decline::new("too_large", "the file is over the size limit"));
        }
        if bytes.starts_with(CFB_MAGIC) {
            return Err(Decline::new(
                "encrypted",
                "an encrypted package (a compound file, not a zip)",
            ));
        }
        let zip = Package::read(bytes, limits)?;
        if zip.rewrite()? != bytes {
            return Err(Decline::new(
                "zip_layout",
                "the package doesn't write back to its own bytes",
            ));
        }
        let mut parts = BTreeMap::new();
        for (index, e) in zip.entries().iter().enumerate() {
            let content = zip.inflate(index)?;
            if xml::is_xml_name(&e.name) {
                if xml::is_utf16(&content) {
                    return Err(Decline::new(
                        "xml_encoding",
                        format!("{:?} is UTF-16", e.name),
                    ));
                }
                if xml::has_doctype(&content) && !mutant::active("doctype_skip") {
                    return Err(Decline::new(
                        "doctype",
                        format!("{:?} has a DOCTYPE", e.name),
                    ));
                }
            }
            check(&e.name, &content)?;
            parts.insert(
                name::key(&e.name),
                Part {
                    index,
                    name: e.name.clone(),
                    content,
                },
            );
        }
        let ct = parts
            .get(CONTENT_TYPES)
            .ok_or_else(|| Decline::new("no_content_types", "the package has no content types"))?;
        let types = ContentTypes::parse(&ct.content, limits)?;
        Ok(OpcPackage { zip, parts, types })
    }

    /// The file's bytes.
    pub fn bytes(&self) -> &'a [u8] {
        self.zip.bytes()
    }

    /// The package written back with every entry raw: identical to what was opened.
    pub fn encode(&self) -> Result<Vec<u8>, Decline> {
        self.zip.rewrite()
    }

    /// A part's content, by key.
    pub fn content(&self, key: &str) -> Option<&[u8]> {
        self.parts.get(key).map(|p| p.content.as_slice())
    }

    /// A part's content type, by key.
    pub fn content_type(&self, key: &str) -> Option<&str> {
        self.types.of(key)
    }

    /// The relationships of `source` (a part's name or key; `""`: the package), if it has a
    /// relationships part.
    pub fn relationships(
        &self,
        source: &str,
        limits: &Limits,
    ) -> Result<Option<Relationships>, Decline> {
        let rels = name::key(&name::rels_part_for(source));
        match self.parts.get(&rels) {
            Some(p) => Relationships::parse(&p.name, &p.content, limits).map(Some),
            None => Ok(None),
        }
    }

    /// The part the package's first relationship of `rel_type` targets, as a key, if it is
    /// in the package.
    pub fn package_target(
        &self,
        rel_type: &str,
        limits: &Limits,
    ) -> Result<Option<String>, Decline> {
        let Some(rels) = self.relationships("", limits)? else {
            return Ok(None);
        };
        Ok(rels
            .of_type(rel_type)
            .and_then(|r| r.target_key(""))
            .filter(|k| self.parts.contains_key(k)))
    }

    /// The package with `changes` spliced in: every other entry is copied raw, in place, so
    /// untouched parts keep their bytes, the zip's order and its timestamps; a replaced part
    /// keeps its headers; new parts go at the end, in key order. Content equal to a part's
    /// old content keeps its raw record. It is [`OpcPackage::write_plan`] applied to the
    /// file's bytes.
    pub fn write(&self, changes: &BTreeMap<String, Change>) -> Result<Vec<u8>, Decline> {
        zip::apply(self.bytes(), &self.write_plan(changes)?)
    }

    /// What [`OpcPackage::write`] gives, as ranges of the file's bytes and what replaces each
    /// (see [`zip::write_plan`]): a removed or replaced entry's record, new entries inserted
    /// before the directory, and the directory and end record replaced by the new ones. In
    /// order, apart, inside the file; every byte outside them is the file's own.
    pub fn write_plan(&self, changes: &BTreeMap<String, Change>) -> Result<Vec<Span>, Decline> {
        zip::write_plan(&self.zip, &self.items(changes)?)
    }

    /// The same package as [`OpcPackage::write`], built record by record with [`zip::write`]
    /// rather than as ranges of the file: the tests hold the two equal.
    pub fn write_by_records(&self, changes: &BTreeMap<String, Change>) -> Result<Vec<u8>, Decline> {
        zip::write(&self.zip, &self.items(changes)?)
    }

    /// What goes in the package for `changes`, in order: each entry raw or replaced, unless
    /// removed, then the new parts in key order.
    fn items<'s>(
        &'s self,
        changes: &'s BTreeMap<String, Change>,
    ) -> Result<Vec<Source<'s, 'a>>, Decline> {
        let mut items: Vec<Source<'s, 'a>> = Vec::with_capacity(self.parts.len() + 4);
        for (i, e) in self.zip.entries().iter().enumerate() {
            let key = name::key(&e.name);
            match changes.get(&key) {
                None => items.push(Source::Raw(&self.zip, i)),
                Some(Change::Remove) => {}
                Some(Change::Replace(c)) => {
                    if self.parts.get(&key).map(|p| &p.content) == Some(c) {
                        items.push(Source::Raw(&self.zip, i));
                    } else {
                        items.push(Source::Replaced(&self.zip, i, c));
                    }
                }
                Some(Change::Add { .. }) => {
                    return Err(Decline::new(
                        "zip_duplicate",
                        format!("{:?} is already in the package", e.name),
                    ))
                }
            }
        }
        for (key, c) in changes {
            match c {
                Change::Add { name: n, content } => {
                    if name::key(n) != *key {
                        return Err(Decline::new(
                            "zip_name",
                            format!("{n:?} is filed under another key"),
                        ));
                    }
                    items.push(Source::New(n, content));
                }
                Change::Replace(_) | Change::Remove if !self.parts.contains_key(key) => {
                    return Err(Decline::new(
                        "zip_corrupt",
                        format!("{key:?} isn't in the package"),
                    ));
                }
                _ => {}
            }
        }
        Ok(items)
    }
}

/// What's wrong with a package (key → name, content), as named problems: a part with no
/// content type, a relationships part that doesn't parse or repeats an id, an internal
/// relationship to a missing part, a reference to a relationship its part doesn't have.
pub fn problems(parts: &BTreeMap<String, (&str, &[u8])>, limits: &Limits) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let types = parts
        .get(CONTENT_TYPES)
        .and_then(|(_, c)| ContentTypes::parse(c, limits).ok());
    match &types {
        None => {
            out.insert("no readable content types".to_owned());
        }
        Some(ty) => {
            for k in parts.keys() {
                if k != CONTENT_TYPES && !k.ends_with('/') && ty.of(k).is_none() {
                    out.insert(format!("no content type for {k}"));
                }
            }
        }
    }
    let mut ids_of: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (k, (n, c)) in parts {
        let Some(source) = name::source_of_rels(n) else {
            continue;
        };
        let Ok(r) = Relationships::parse(n, c, limits) else {
            out.insert(format!("{k} doesn't parse"));
            continue;
        };
        let mut ids = BTreeSet::new();
        for x in &r.list {
            if !ids.insert(x.id.clone()) {
                out.insert(format!("{k} repeats {}", x.id));
            }
            if let Some(target) = x.target_key(&source) {
                if !parts.contains_key(&target) && !mutant::active("opc_no_target_check") {
                    out.insert(format!("{k} {} targets missing {target}", x.id));
                }
            }
        }
        ids_of.insert(name::key(&source), ids);
    }
    for (k, (n, c)) in parts {
        if !xml::is_xml_name(n) || name::is_rels(k) || k == CONTENT_TYPES {
            continue;
        }
        let Ok(text) = std::str::from_utf8(c) else {
            continue;
        };
        let Ok(doc) = xml::parse_xml(text, limits) else {
            continue;
        };
        let empty = BTreeSet::new();
        let ids = ids_of.get(k).unwrap_or(&empty);
        for nd in doc.descendants().filter(|x| x.is_element()) {
            for a in nd.attributes() {
                let is_ref = matches!(a.namespace(), Some(xml::R) | Some(xml::R_STRICT))
                    || (a.namespace() == Some(xml::VML_OFFICE) && a.name() == "relid");
                if is_ref && !a.value().is_empty() && !ids.contains(a.value()) {
                    out.insert(format!("{k} refers to missing relationship {}", a.value()));
                }
            }
        }
    }
    out
}
