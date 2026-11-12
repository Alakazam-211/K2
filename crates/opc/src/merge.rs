//! The three-way merge of two saves of a package, at the package level: `ours` is the doc's
//! current version, `theirs` what another program saved, `base` what both started from.
//!
//! **Two saves that each add something pick the same next id.** Each save numbers its new part
//! (`image3.png`, `slide7.xml`, `sheet4.xml`) and its new relationship (`rId9`) from what it
//! saw, so two saves of one base that each add one collide on both, though they added
//! different things. A whole-part merge then sees both sides change the relationships part and
//! calls it a conflict. This merge instead reconciles them first:
//!
//! 1. **Parts both sides added under one name, with different content:** ours' is renamed in
//!    its own numbering, past every name any side uses ([`crate::name::mint`]). Its own
//!    relationships part moves with it, every relationship of ours that targets it is
//!    retargeted, and ours' content-type override for it is renamed.
//! 2. **Relationship ids both sides added for different things:** ours' is re-minted as
//!    `rId<n>`, one past the highest `rId` number any side uses, and every reference to it in
//!    ours' source part (each attribute in the relationships namespace, and VML's `o:relid`)
//!    is rewritten in place. If the old id still appears in any other attribute of that part,
//!    the reference can't be told apart, and the merge is a conflict.
//! 3. **Every part is then merged three-way, whole:** one side's change takes that side's
//!    part. Where both sides changed a part:
//!    - `[Content_Types].xml` merges by key: `Default` by extension, `Override` by part name;
//!    - a relationships part merges by id (after step 2, ids name one thing on both sides);
//!    - any other part goes to the pack's [`PartMerge::both_changed`] (a pack merges its own
//!      content parts; the default is a conflict).
//!
//!    A key, id or part both sides changed differently, or that one side changed and the
//!    other removed, is a conflict.
//! 4. **The result must be sound:** it may not hold a problem neither side had (a part with
//!    no content type, a relationship to a missing part, a repeated id, a reference to a
//!    relationship its part doesn't have), and then the pack's own [`PartMerge::validate`].
//! 5. **Byte for byte where it can be:** a merge equal to one side is that side's file;
//!    otherwise it is laid out as theirs (the later save), with ours' new parts after, and
//!    every part a side's merge left as it was is that side's raw record.
//!
//! Theirs keeps its names and ids, and ours yields: theirs is the file on disk that a person's
//! app saved and may still have open. Everything here is a pure function of the inputs.
//!
//! Native ids inside content (a slide's `sldId`, a shape's `cNvPr`, a sheet's `sheetId`) are
//! the format packs' to re-mint, with the same rule, through [`PartMerge`].

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use crate::error::Decline;
use crate::limits::Limits;
use crate::mutant;
use crate::name::{self, CONTENT_TYPES};
use crate::package::{problems, OpcPackage};
use crate::rels::{Identity, Relationships};
use crate::types::{ContentTypes, Kind};
use crate::xml;
use crate::zip::{self, Source};

/// A pack's answer for a part both sides changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Ours' content (with the merge's renames and re-mints).
    Ours,
    /// Theirs' content.
    Theirs,
    /// Merged content.
    Merged(Vec<u8>),
    /// The changes overlap; why.
    Conflict(String),
}

/// What a format pack adds to the package merge.
pub trait PartMerge {
    /// Both sides changed the part `key` (and both still have it). `ours` already has the
    /// merge's renames and re-mints applied. The default: a conflict.
    fn both_changed(
        &self,
        key: &str,
        base: Option<&[u8]>,
        ours: &[u8],
        theirs: &[u8],
    ) -> Resolution {
        let _ = (base, ours, theirs);
        Resolution::Conflict(format!("both sides changed {key}"))
    }

    /// The pack's own soundness checks on the merged parts (by key), with the two sides.
    fn validate(
        &self,
        merged: &BTreeMap<String, &[u8]>,
        ours: &OpcPackage<'_>,
        theirs: &OpcPackage<'_>,
    ) -> Result<(), String> {
        let _ = (merged, ours, theirs);
        Ok(())
    }
}

/// The package merge with no format pack: a part both sides changed is a conflict.
#[derive(Debug, Clone, Copy, Default)]
pub struct Generic;

impl PartMerge for Generic {}

/// What a merge came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merged {
    /// Ours' file, byte for byte.
    Ours,
    /// Theirs' file, byte for byte.
    Theirs,
    /// A new file.
    New(Vec<u8>),
}

/// What the merge renamed and re-minted on ours' side, for a person.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    /// Ours' parts renamed: (old name, new name).
    pub renamed: Vec<(String, String)>,
    /// Ours' relationship ids re-minted: (relationships part, old id, new id).
    pub reminted: Vec<(String, String, String)>,
}

/// What [`merge3_bytes`] came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The merged file, and what was renamed and re-minted.
    Merged(Vec<u8>, Report),
    /// The changes overlap, or the result wouldn't be sound; why.
    Conflict(String),
    /// One of the files is one the package layer doesn't hold.
    Unsupported(Decline),
}

/// Merge three files with a pack's part merge. A pure function of its inputs.
pub fn merge3_bytes(
    base: &[u8],
    ours: &[u8],
    theirs: &[u8],
    pack: &dyn PartMerge,
    limits: &Limits,
) -> Outcome {
    if ours == theirs || base == theirs {
        return Outcome::Merged(ours.to_vec(), Report::default());
    }
    if base == ours {
        return Outcome::Merged(theirs.to_vec(), Report::default());
    }
    let open = |b| OpcPackage::open(b, limits);
    let (b, o, t) = match (open(base), open(ours), open(theirs)) {
        (Ok(b), Ok(o), Ok(t)) => (b, o, t),
        (Err(d), _, _) | (_, Err(d), _) | (_, _, Err(d)) => return Outcome::Unsupported(d),
    };
    match merge3(&b, &o, &t, pack, limits) {
        Ok((Merged::Ours, r)) => Outcome::Merged(ours.to_vec(), r),
        Ok((Merged::Theirs, r)) => Outcome::Merged(theirs.to_vec(), r),
        Ok((Merged::New(bytes), r)) => Outcome::Merged(bytes, r),
        Err(why) => Outcome::Conflict(why),
    }
}

/// One of ours' parts while the merge renames and re-mints.
#[derive(Debug, Clone)]
struct Work {
    name: String,
    content: Vec<u8>,
    /// Its entry's index in ours' zip (a renamed part keeps its old one).
    origin: usize,
}

/// Where a rebuilt element's namespace prefix goes, until [`append_children`] knows the
/// root's. A control character, so no parsed XML value can hold it.
const PREFIX: &str = "\u{1}";

fn three_way<T: PartialEq>(b: Option<T>, o: Option<T>, t: Option<T>) -> Result<Option<T>, ()> {
    if o == t || o == b {
        Ok(t)
    } else if t == b {
        Ok(o)
    } else {
        Err(())
    }
}

fn utf8<'x>(what: &str, b: &'x [u8]) -> Result<&'x str, String> {
    std::str::from_utf8(b).map_err(|_| format!("{what} isn't UTF-8"))
}

/// The highest `rId<n>` number among `ids`.
fn highest_rid<'x>(ids: impl Iterator<Item = &'x str>) -> u64 {
    ids.filter_map(|i| i.strip_prefix("rId"))
        .filter_map(|n| n.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
}

/// Merge three open packages with a pack's part merge. `Err` is a conflict, with why.
pub fn merge3(
    b: &OpcPackage<'_>,
    o: &OpcPackage<'_>,
    t: &OpcPackage<'_>,
    pack: &dyn PartMerge,
    limits: &Limits,
) -> Result<(Merged, Report), String> {
    let mut report = Report::default();
    let mut ours: BTreeMap<String, Work> = o
        .parts
        .iter()
        .map(|(k, p)| {
            (
                k.clone(),
                Work {
                    name: p.name.clone(),
                    content: p.content.clone(),
                    origin: p.index,
                },
            )
        })
        .collect();
    rename_collisions(b, t, &mut ours, &mut report, limits)?;
    remint_collisions(b, t, &mut ours, &mut report, limits)?;

    // every part, three-way
    let keys: BTreeSet<&String> = b
        .parts
        .keys()
        .chain(ours.keys())
        .chain(t.parts.keys())
        .collect();
    let mut merged: BTreeMap<String, (String, Cow<'_, [u8]>)> = BTreeMap::new();
    for &k in &keys {
        let bc = b.content(k);
        let oc = ours.get(k).map(|w| w.content.as_slice());
        let tc = t.content(k);
        let take_ours = || {
            (
                ours[k].name.clone(),
                Cow::Borrowed(ours[k].content.as_slice()),
            )
        };
        let take_theirs = || {
            (
                t.parts[k].name.clone(),
                Cow::Borrowed(t.parts[k].content.as_slice()),
            )
        };
        let pick = if oc == tc || oc == bc {
            tc.map(|_| take_theirs())
        } else if tc == bc {
            oc.map(|_| take_ours())
        } else {
            let (Some(oc), Some(tc)) = (oc, tc) else {
                return Err(format!("one side changed {k}, the other removed it"));
            };
            let resolution = if k.as_str() == CONTENT_TYPES {
                merge_types(bc, oc, tc, limits)
            } else if name::is_rels(k) {
                merge_rels(k, &ours[k].name, bc, oc, tc, limits)
            } else {
                pack.both_changed(k, bc, oc, tc)
            };
            match resolution {
                Resolution::Ours => Some(take_ours()),
                Resolution::Theirs => Some(take_theirs()),
                Resolution::Merged(c) => Some((t.parts[k].name.clone(), Cow::Owned(c))),
                Resolution::Conflict(why) => return Err(why),
            }
        };
        if let Some(p) = pick {
            merged.insert(k.clone(), p);
        }
    }

    // equal to one side: that side's file, byte for byte
    let same = |side: &OpcPackage<'_>| {
        merged.len() == side.parts.len()
            && merged.iter().all(|(k, (n, c))| {
                side.parts
                    .get(k)
                    .is_some_and(|p| &p.name == n && p.content.as_slice() == c.as_ref())
            })
    };
    if same(t) {
        return Ok((Merged::Theirs, report));
    }
    if same(o) {
        return Ok((Merged::Ours, report));
    }

    // the result is sound, or it's a conflict
    let view: BTreeMap<String, (&str, &[u8])> = merged
        .iter()
        .map(|(k, (n, c))| (k.clone(), (n.as_str(), c.as_ref())))
        .collect();
    let mut known = problems(&view_of(o), limits);
    known.extend(problems(&view_of(t), limits));
    if mutant::active("opc_validate_all") {
        known.clear();
    }
    let found = problems(&view, limits);
    if let Some(p) = found.iter().find(|p| !known.contains(*p)) {
        if !mutant::active("opc_no_validate") {
            return Err(format!(
                "the merge would have a problem neither side has: {p}"
            ));
        }
    }
    let contents: BTreeMap<String, &[u8]> =
        view.iter().map(|(k, (_, c))| (k.clone(), *c)).collect();
    pack.validate(&contents, o, t)?;

    // laid out as theirs (the later save), ours' new parts after
    let mut items: Vec<Source<'_, '_>> = Vec::with_capacity(merged.len());
    let mut placed = BTreeSet::new();
    for (i, e) in t.zip.entries().iter().enumerate() {
        let k = name::key(&e.name);
        let Some((n, c)) = merged.get(&k) else {
            continue;
        };
        placed.insert(k.clone());
        let from_t = &t.parts[&k];
        if n == &from_t.name && c.as_ref() == from_t.content.as_slice() {
            items.push(Source::Raw(&t.zip, i));
        } else if let Some(p) = o
            .parts
            .get(&k)
            .filter(|p| &p.name == n && p.content.as_slice() == c.as_ref())
        {
            items.push(Source::Raw(&o.zip, p.index));
        } else if n == &from_t.name {
            items.push(Source::Replaced(&t.zip, i, c.as_ref()));
        } else {
            items.push(Source::New(n, c.as_ref()));
        }
    }
    // ours' parts theirs doesn't have, in ours' order; a renamed one where its old name was
    let mut order: Vec<(usize, &String)> = ours
        .iter()
        .filter(|(k, _)| !placed.contains(*k) && merged.contains_key(*k))
        .map(|(k, w)| (w.origin, k))
        .collect();
    order.sort();
    for (_, k) in order {
        let (n, c) = &merged[k];
        match o.parts.get(k) {
            Some(p) if &p.name == n && p.content.as_slice() == c.as_ref() => {
                items.push(Source::Raw(&o.zip, p.index))
            }
            Some(p) if &p.name == n => items.push(Source::Replaced(&o.zip, p.index, c.as_ref())),
            _ => items.push(Source::New(n, c.as_ref())),
        }
    }
    let bytes = zip::write(&t.zip, &items).map_err(|e| e.to_string())?;
    OpcPackage::open(&bytes, limits).map_err(|e| format!("the merge doesn't open: {e}"))?;
    Ok((Merged::New(bytes), report))
}

fn view_of<'p>(side: &'p OpcPackage<'_>) -> BTreeMap<String, (&'p str, &'p [u8])> {
    side.parts
        .iter()
        .map(|(k, p)| (k.clone(), (p.name.as_str(), p.content.as_slice())))
        .collect()
}

/// Step 1: ours' parts that collide by name with a different part theirs added are renamed.
fn rename_collisions(
    b: &OpcPackage<'_>,
    t: &OpcPackage<'_>,
    ours: &mut BTreeMap<String, Work>,
    report: &mut Report,
    limits: &Limits,
) -> Result<(), String> {
    if mutant::active("opc_no_rename") {
        return Ok(());
    }
    let mut taken: BTreeSet<String> = b
        .parts
        .keys()
        .chain(ours.keys())
        .chain(t.parts.keys())
        .cloned()
        .collect();
    let mut renames: BTreeMap<String, String> = BTreeMap::new();
    for (k, w) in ours.iter() {
        if k.ends_with('/') || k == CONTENT_TYPES || name::is_rels(k) || b.parts.contains_key(k) {
            continue;
        }
        if let Some(tp) = t.parts.get(k) {
            if tp.content != w.content {
                let new = name::mint(&w.name, &taken);
                taken.insert(name::key(&new));
                // the part's own relationships part moves with it
                let new_rels = name::rels_part_for(&new);
                taken.insert(name::key(&new_rels));
                renames.insert(k.clone(), new);
            }
        }
    }
    if renames.is_empty() {
        return Ok(());
    }
    for (old_key, new) in &renames {
        let w = ours.remove(old_key).expect("a part of ours");
        report.renamed.push((w.name.clone(), new.clone()));
        let old_rels = name::key(&name::rels_part_for(&w.name));
        ours.insert(
            name::key(new),
            Work {
                name: new.clone(),
                content: w.content,
                origin: w.origin,
            },
        );
        if let Some(r) = ours.remove(&old_rels) {
            let n = name::rels_part_for(new);
            ours.insert(
                name::key(&n),
                Work {
                    name: n,
                    content: r.content,
                    origin: r.origin,
                },
            );
        }
    }
    // every relationship of ours that targets a renamed part follows it
    let rels_keys: Vec<String> = ours.keys().filter(|k| name::is_rels(k)).cloned().collect();
    for rk in rels_keys {
        let w = &ours[&rk];
        let source = name::source_of_rels(&w.name).unwrap_or_default();
        let parsed = match Relationships::parse(&w.name, &w.content, limits) {
            Ok(p) => p,
            Err(_) => continue, // unchanged; the soundness check sees an unreadable part
        };
        let mut edits = Vec::new();
        for r in &parsed.list {
            if let Some(new) = r.target_key(&source).and_then(|k| renames.get(&k)) {
                let target = name::retarget(&r.target, name::file_name(new));
                edits.push((r.target_range.clone(), xml::escape_attr(&target)));
            }
        }
        if !edits.is_empty() {
            let text = utf8(&w.name, &w.content)?.to_owned();
            let out = xml::splice(&text, edits);
            ours.get_mut(&rk).expect("present").content = out.into_bytes();
        }
    }
    // and ours' content-type overrides for them
    if let Some(w) = ours.get(CONTENT_TYPES) {
        if let Ok(types) = ContentTypes::parse(&w.content, limits) {
            let mut edits = Vec::new();
            for e in types.entries.iter().filter(|e| e.kind == Kind::Override) {
                if let Some(new) = renames.get(&e.key) {
                    edits.push((e.key_range.clone(), xml::escape_attr(&format!("/{new}"))));
                }
            }
            if !edits.is_empty() {
                let text = utf8("the content types", &w.content)?.to_owned();
                let out = xml::splice(&text, edits);
                ours.get_mut(CONTENT_TYPES).expect("present").content = out.into_bytes();
            }
        }
    }
    Ok(())
}

/// Step 2: ours' new relationship ids that theirs used for something else are re-minted, and
/// ours' references to them rewritten.
fn remint_collisions(
    b: &OpcPackage<'_>,
    t: &OpcPackage<'_>,
    ours: &mut BTreeMap<String, Work>,
    report: &mut Report,
    limits: &Limits,
) -> Result<(), String> {
    let rels_keys: Vec<String> = ours.keys().filter(|k| name::is_rels(k)).cloned().collect();
    for rk in rels_keys {
        let Some(tp) = t.parts.get(&rk) else { continue };
        let w = &ours[&rk];
        let source = name::source_of_rels(&w.name).unwrap_or_default();
        let parse = |n: &str, c: &[u8]| Relationships::parse(n, c, limits).ok();
        let (Some(ro), Some(rt)) = (parse(&w.name, &w.content), parse(&tp.name, &tp.content))
        else {
            continue; // the part-level merge decides; an unreadable part can't merge by id
        };
        let rb = b.parts.get(&rk).and_then(|p| parse(&p.name, &p.content));
        let ids = |r: &Relationships| -> BTreeMap<String, Identity> {
            r.list
                .iter()
                .map(|x| (x.id.clone(), x.identity(&source)))
                .collect()
        };
        let (ib, it) = (rb.as_ref().map(ids).unwrap_or_default(), ids(&rt));
        let mut all: BTreeSet<String> = ib.keys().chain(it.keys()).cloned().collect();
        all.extend(ro.list.iter().map(|r| r.id.clone()));
        let mut map: BTreeMap<String, String> = BTreeMap::new();
        for r in &ro.list {
            if ib.contains_key(&r.id) || map.contains_key(&r.id) {
                continue;
            }
            match it.get(&r.id) {
                Some(theirs) if *theirs != r.identity(&source) => {
                    if mutant::active("opc_no_remint") {
                        continue;
                    }
                    let mut n = highest_rid(all.iter().map(String::as_str)) + 1;
                    while all.contains(&format!("rId{n}")) {
                        n += 1;
                    }
                    let new = format!("rId{n}");
                    all.insert(new.clone());
                    map.insert(r.id.clone(), new);
                }
                _ => {}
            }
        }
        if map.is_empty() {
            continue;
        }
        // the ids in ours' relationships part
        let text = utf8(&w.name, &w.content)?;
        let edits: Vec<_> = ro
            .list
            .iter()
            .filter_map(|r| map.get(&r.id).map(|n| (r.id_range.clone(), n.clone())))
            .collect();
        let rels_name = w.name.clone();
        let new_rels = xml::splice(text, edits);
        ours.get_mut(&rk).expect("present").content = new_rels.into_bytes();
        for (old, new) in &map {
            report
                .reminted
                .push((rels_name.clone(), old.clone(), new.clone()));
        }
        // the references in ours' source part
        if source.is_empty() {
            continue;
        }
        let sk = name::key(&source);
        let Some(sw) = ours.get(&sk) else { continue };
        let text = utf8(&sw.name, &sw.content)?;
        let doc = xml::parse_xml(text, limits)
            .map_err(|e| format!("can't rewrite the references in {}: {e}", sw.name))?;
        let mut edits = Vec::new();
        for n in doc.descendants().filter(|n| n.is_element()) {
            for a in n.attributes() {
                let Some(new) = map.get(a.value()) else {
                    continue;
                };
                let is_ref = matches!(a.namespace(), Some(xml::R) | Some(xml::R_STRICT))
                    || (a.namespace() == Some(xml::VML_OFFICE) && a.name() == "relid");
                if !is_ref {
                    return Err(format!(
                        "{} holds {} in an attribute that isn't a relationship reference ({}), so its references can't be rewritten",
                        sw.name,
                        a.value(),
                        a.name()
                    ));
                }
                if !mutant::active("opc_no_ref_rewrite") {
                    edits.push((a.range_value(), new.clone()));
                }
            }
        }
        let out = xml::splice(text, edits);
        ours.get_mut(&sk).expect("present").content = out.into_bytes();
    }
    Ok(())
}

/// `[Content_Types].xml` both sides changed: by key, rendered on theirs' text.
fn merge_types(b: Option<&[u8]>, o: &[u8], t: &[u8], limits: &Limits) -> Resolution {
    let parse = |x: &[u8]| ContentTypes::parse(x, limits);
    let (bt, ot, tt) = match (b.map(parse).transpose(), parse(o), parse(t)) {
        (Ok(bt), Ok(ot), Ok(tt)) => (bt.unwrap_or_default(), ot, tt),
        _ => return Resolution::Conflict("the content types don't parse".to_owned()),
    };
    for x in [&bt, &ot, &tt] {
        if let Some(k) = x.repeated() {
            return Resolution::Conflict(format!("the content types name {k} twice"));
        }
        if x.other_elements > 0 {
            return Resolution::Conflict(
                "the content types hold an element of another kind".to_owned(),
            );
        }
    }
    let map = |x: &ContentTypes| -> BTreeMap<(Kind, String), String> {
        x.entries
            .iter()
            .map(|e| ((e.kind, e.key.clone()), e.value.clone()))
            .collect()
    };
    let (mb, mo, mt) = (map(&bt), map(&ot), map(&tt));
    let keys: BTreeSet<&(Kind, String)> = mb.keys().chain(mo.keys()).chain(mt.keys()).collect();
    let mut result: BTreeMap<(Kind, String), String> = BTreeMap::new();
    for k in keys {
        match three_way(mb.get(k), mo.get(k), mt.get(k)) {
            Ok(Some(v)) => {
                result.insert(k.clone(), v.clone());
            }
            Ok(None) => {}
            Err(()) if mutant::active("opc_types_ours") => {
                if let Some(v) = mo.get(k) {
                    result.insert(k.clone(), v.clone());
                }
            }
            Err(()) => {
                return Resolution::Conflict(format!(
                    "both sides changed the content type of {}",
                    k.1
                ))
            }
        }
    }
    let Ok(text) = std::str::from_utf8(t) else {
        return Resolution::Conflict("the content types aren't UTF-8".to_owned());
    };
    let mut edits = Vec::new();
    for e in &tt.entries {
        match result.get(&(e.kind, e.key.clone())) {
            None => edits.push((e.range.clone(), String::new())),
            Some(v) if *v != e.value => edits.push((e.value_range.clone(), xml::escape_attr(v))),
            Some(_) => {}
        }
    }
    let mut add = String::new();
    for e in &ot.entries {
        let k = (e.kind, e.key.clone());
        if mt.contains_key(&k) {
            continue;
        }
        let Some(v) = result.get(&k) else { continue };
        let (el, attr) = match e.kind {
            Kind::Default => ("Default", "Extension"),
            Kind::Override => ("Override", "PartName"),
        };
        add.push_str(&format!(
            "<{PREFIX}{el} {attr}=\"{}\" ContentType=\"{}\"/>",
            xml::escape_attr(&e.raw_key),
            xml::escape_attr(v)
        ));
    }
    match append_children(text, edits, &add, limits) {
        Ok(s) => Resolution::Merged(s.into_bytes()),
        Err(why) => Resolution::Conflict(why),
    }
}

/// A relationships part both sides changed: by id, rendered on theirs' text.
fn merge_rels(
    key: &str,
    name_: &str,
    b: Option<&[u8]>,
    o: &[u8],
    t: &[u8],
    limits: &Limits,
) -> Resolution {
    let source = name::source_of_rels(name_).unwrap_or_default();
    let parse = |x: &[u8]| Relationships::parse(name_, x, limits);
    let (rb, ro, rt) = match (b.map(parse).transpose(), parse(o), parse(t)) {
        (Ok(rb), Ok(ro), Ok(rt)) => (rb.unwrap_or_default(), ro, rt),
        _ => return Resolution::Conflict(format!("{key} doesn't parse")),
    };
    for x in [&rb, &ro, &rt] {
        if x.by_id().is_none() {
            return Resolution::Conflict(format!("{key} repeats an id"));
        }
        if x.other_elements > 0 {
            return Resolution::Conflict(format!("{key} holds an element of another kind"));
        }
    }
    let map = |x: &Relationships| -> BTreeMap<String, Identity> {
        x.list
            .iter()
            .map(|r| (r.id.clone(), r.identity(&source)))
            .collect()
    };
    let (mb, mo, mt) = (map(&rb), map(&ro), map(&rt));
    let ids: BTreeSet<&String> = mb.keys().chain(mo.keys()).chain(mt.keys()).collect();
    let mut result: BTreeMap<String, &Identity> = BTreeMap::new();
    for id in ids {
        match three_way(mb.get(id), mo.get(id), mt.get(id)) {
            Ok(Some(v)) => {
                result.insert(id.clone(), v);
            }
            Ok(None) => {}
            Err(()) => {
                return Resolution::Conflict(format!(
                    "both sides changed relationship {id} in {key}"
                ))
            }
        }
    }
    if mutant::active("opc_rels_drop_theirs") {
        result.retain(|id, _| mo.contains_key(id) || mb.contains_key(id));
    }
    let Ok(text) = std::str::from_utf8(t) else {
        return Resolution::Conflict(format!("{key} isn't UTF-8"));
    };
    let ours_by_id: BTreeMap<&str, &crate::rels::Relationship> =
        ro.list.iter().map(|r| (r.id.as_str(), r)).collect();
    let element = |r: &crate::rels::Relationship| {
        format!(
            "<{PREFIX}Relationship Id=\"{}\" Type=\"{}\" Target=\"{}\"{}/>",
            xml::escape_attr(&r.id),
            xml::escape_attr(&r.rel_type),
            xml::escape_attr(&r.target),
            if r.external {
                " TargetMode=\"External\""
            } else {
                ""
            }
        )
    };
    let mut edits = Vec::new();
    for r in &rt.list {
        match result.get(&r.id) {
            None => edits.push((r.range.clone(), String::new())),
            Some(v) if **v != r.identity(&source) => {
                // ours changed it: ours' element, in theirs' place
                let ours_r = ours_by_id[r.id.as_str()];
                edits.push((r.range.clone(), element(ours_r)));
            }
            Some(_) => {}
        }
    }
    let mut add = String::new();
    for r in &ro.list {
        if mt.contains_key(&r.id) || !result.contains_key(&r.id) {
            continue;
        }
        add.push_str(&element(r));
    }
    match append_children(text, edits, &add, limits) {
        Ok(s) => Resolution::Merged(s.into_bytes()),
        Err(why) => Resolution::Conflict(why),
    }
}

/// `text` with `edits` spliced and `add` (elements with [`PREFIX`] for the root's prefix) appended
/// as the root's last children.
fn append_children(
    text: &str,
    mut edits: Vec<(std::ops::Range<usize>, String)>,
    add: &str,
    limits: &Limits,
) -> Result<String, String> {
    let doc = xml::parse_xml(text, limits).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let q = xml::qname(text, root);
    let prefix = q
        .rsplit_once(':')
        .map_or(String::new(), |(p, _)| format!("{p}:"));
    // a rebuilt element, in place or appended, takes the root's prefix
    for e in &mut edits {
        e.1 = e.1.replace(PREFIX, &prefix);
    }
    if !add.is_empty() {
        let add = add.replace(PREFIX, &prefix);
        let r = root.range();
        let whole = &text[r.clone()];
        if whole.ends_with("/>") {
            let at = r.end - 2;
            edits.push((at..r.end, format!(">{add}</{q}>")));
        } else {
            let close = whole
                .rfind("</")
                .ok_or_else(|| "the root element has no end tag".to_owned())?;
            let at = r.start + close;
            edits.push((at..at, add));
        }
    }
    let out = xml::splice(text, edits);
    xml::parse_xml(&out, limits).map_err(|e| format!("the merged XML doesn't parse: {e}"))?;
    Ok(out)
}
