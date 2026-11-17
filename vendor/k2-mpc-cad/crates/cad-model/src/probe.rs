//! A drawing's structure in counts and names, for setting up a project's reading of its
//! drawings: its layers, blocks (a dynamic block's references under its own name), attribute
//! tags, linetypes, boundary candidates (closed polylines), title-block candidates, and the
//! **shapes** of attribute values; and, over a set ([`probe_set`]), the attribute tags whose
//! values pair up across the set (flag candidates, by structure).
//!
//! **No value ever reaches the output**: an attribute's value, a text's content, a path. A value
//! is only classified (does it look like an identifier: letters, a hyphen, digits; how many of
//! each), and only the classification's counts are printed. Names (layers, blocks, tags,
//! linetypes) are printed: they are the drawing's conventions, which the setup needs.

use std::collections::BTreeMap;

use crate::geom::in_polygon;
use crate::model::{BlockKind, Doc};
use crate::{mutant, reads};

/// An identifier-like value's shape, `letters-digits` (`3-2` for `CAM-01`), or `None`.
fn id_shape(v: &str) -> Option<String> {
    let (l, d) = v.trim().split_once('-')?;
    let ok = !l.is_empty()
        && !d.is_empty()
        && l.bytes().all(|b| b.is_ascii_uppercase())
        && d.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| format!("{}-{}", l.len(), d.len()))
}

/// A tag's values: how many, how many have an identifier's shape, and how many of each shape.
type TagCount = (u64, u64, BTreeMap<String, u64>);

/// The structure of sheet `name`, one fact per line (module docs).
pub fn probe(name: &str, doc: &Doc) -> String {
    // what's drawn: in model space or a layout, not inside a block's definition
    let drawn = |e: &crate::model::Entity| doc.owner_kind(e) != Some(BlockKind::Block);
    let mut out = format!(
        "sheet {name} version {} units {} handles {} drawn {} layers {} blocks {}\n",
        doc.version,
        doc.units.word,
        doc.handles,
        doc.entities.iter().filter(|e| drawn(e)).count(),
        doc.layers.len(),
        doc.blocks
            .iter()
            .filter(|b| b.kind == BlockKind::Block)
            .count()
    );
    // layers: what's drawn on them, their linetype
    let mut on: BTreeMap<String, (u64, BTreeMap<String, u64>)> = BTreeMap::new();
    for e in doc.entities.iter().filter(|e| drawn(e)) {
        let r = on.entry(e.layer.to_ascii_uppercase()).or_default();
        r.0 += 1;
        *r.1.entry(e.kind.to_string()).or_default() += 1;
    }
    for l in &doc.layers {
        let (n, kinds) = on.remove(&l.name.to_ascii_uppercase()).unwrap_or_default();
        let kinds: Vec<String> = kinds.iter().map(|(k, c)| format!("{k}:{c}")).collect();
        out.push_str(&format!(
            "layer {} ents {n} ltype {} kinds {}\n",
            l.name,
            l.ltype.as_deref().unwrap_or("-"),
            if kinds.is_empty() {
                "-".into()
            } else {
                kinds.join(",")
            }
        ));
    }
    for (name, (n, _)) in &on {
        out.push_str(&format!("layer-undefined {name} ents {n}\n"));
    }
    // blocks: references (by the block's real name: a dynamic block's for a reference drawn
    // through one of its anonymous blocks), whether they carry attributes, their tags in the
    // order first seen, and how many are drawn through an anonymous block
    let mut refs: BTreeMap<String, (u64, u64, Vec<String>, u64)> = BTreeMap::new();
    for e in &doc.entities {
        let Some(b) = doc.block_name(e) else {
            continue;
        };
        let r = refs.entry(b.to_ascii_uppercase()).or_default();
        r.0 += 1;
        r.3 += u64::from(e.block.as_deref() != Some(b));
        if let Some(x) = e.extra.as_ref().filter(|x| !x.attribs.is_empty()) {
            r.1 += 1;
            for a in &x.attribs {
                if !r.2.iter().any(|t| t.eq_ignore_ascii_case(&a.tag)) {
                    r.2.push(a.tag.to_string());
                }
            }
        }
    }
    // anonymous blocks (dynamic blocks' as set, dimensions', hatches') in counts only
    let (mut anon, mut dynamic, mut unlinked) = (0u64, 0u64, 0u64);
    for b in doc.blocks.iter().filter(|b| b.kind == BlockKind::Block) {
        if b.anon {
            anon += 1;
            dynamic += u64::from(b.dynamic_of.is_some());
            unlinked += u64::from(
                b.dynamic_of.is_none() && refs.contains_key(&b.name.to_ascii_uppercase()),
            );
            continue;
        }
        let (n, with, tags, dy) = refs
            .get(&b.name.to_ascii_uppercase())
            .cloned()
            .unwrap_or_default();
        out.push_str(&format!(
            "block {} refs {n} with-attribs {with} tags {}{}\n",
            b.name,
            if tags.is_empty() {
                "-".into()
            } else {
                tags.join(",")
            },
            if dy > 0 {
                format!(" dynamic {dy}")
            } else {
                String::new()
            }
        ));
    }
    out.push_str(&format!(
        "anonymous-blocks {anon} dynamic {dynamic} referenced-unlinked {unlinked}\n"
    ));
    // boundary candidates: closed polylines, by layer and effective linetype
    let mut bounds: BTreeMap<(String, String), (u64, u64)> = BTreeMap::new();
    for (i, e) in doc.entities.iter().enumerate() {
        let Some(x) = e.extra.as_ref().filter(|x| x.closed && x.verts.len() >= 3) else {
            continue;
        };
        let inside = doc
            .entities
            .iter()
            .filter(|t| {
                t.text.is_some()
                    && t.block.is_none()
                    && t.at.is_some_and(|p| in_polygon(p, &x.verts))
            })
            .count() as u64;
        let r = bounds
            .entry((e.layer.to_string(), reads::ltype_eff(doc, i)))
            .or_default();
        r.0 += 1;
        r.1 += u64::from(inside > 0);
    }
    for ((layer, lt), (n, named)) in &bounds {
        out.push_str(&format!(
            "boundary-candidate layer {layer} ltype {lt} closed {n} with-text-inside {named}\n"
        ));
    }
    // title-block candidates: references in a layout, the most attributes first
    let mut titles: Vec<(usize, String, Vec<String>)> = doc
        .entities
        .iter()
        .filter(|e| doc.owner_kind(e) == Some(BlockKind::Layout))
        .filter_map(|e| {
            let x = e.extra.as_ref()?;
            Some((
                x.attribs.len(),
                doc.block_name(e)?.to_owned(),
                x.attribs.iter().map(|a| a.tag.to_string()).collect(),
            ))
        })
        .filter(|t| t.0 > 0)
        .collect();
    titles.sort_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)));
    titles.dedup_by(|a, b| a.1 == b.1);
    titles.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (n, block, tags) in titles.iter().take(3) {
        out.push_str(&format!(
            "title-candidate block {block} attribs {n} tags {}\n",
            tags.join(",")
        ));
    }
    // tags: how many values, how many identifier-shaped, by shape
    let mut tags: BTreeMap<String, TagCount> = BTreeMap::new();
    for e in &doc.entities {
        let Some(x) = e.extra.as_ref() else { continue };
        for a in &x.attribs {
            let r = tags.entry(a.tag.to_ascii_uppercase()).or_default();
            r.0 += 1;
            if let Some(s) = id_shape(&a.value) {
                r.1 += 1;
                *r.2.entry(s).or_default() += 1;
            }
        }
    }
    for (tag, (n, ids, shapes)) in &tags {
        let shapes: Vec<String> = shapes.iter().map(|(s, c)| format!("{s}:{c}")).collect();
        out.push_str(&format!(
            "tag {tag} values {n} id-shaped {ids} shapes {}\n",
            if shapes.is_empty() {
                "-".into()
            } else {
                shapes.join(",")
            }
        ));
    }
    if mutant::active("ProbeValueLeaks") {
        // the planted bug: example values beside the counts
        for a in doc
            .entities
            .iter()
            .filter_map(|e| e.extra.as_ref())
            .flat_map(|x| x.attribs.iter())
        {
            out.push_str(&format!("tag-example {} {}\n", a.tag, a.value));
        }
    }
    out
}

/// Over a set of drawings (each a name and its model): the attribute tags whose values pair up
/// across the set, the structure an off-sheet flag has whatever its blocks or its values' shape.
/// For each tag on references drawn in model space or a layout: how many distinct values, how
/// many occur exactly twice (`paired`), once (`single`) or more often (`more`), how many of
/// those have an identifier's shape, and the blocks (by real name) that carry it. A tag is a
/// candidate when some values pair and at least half of them do: a view's or a quantity's
/// numbers repeat on every sheet, a device's identifier stays apart. No value is printed.
pub fn probe_set(docs: &[(&str, &Doc)]) -> String {
    // tag -> value -> occurrences; tag -> blocks
    let mut seen: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    let mut blocks: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (_, doc) in docs {
        for e in &doc.entities {
            if doc.owner_kind(e) == Some(BlockKind::Block) {
                continue;
            }
            let (Some(b), Some(x)) = (doc.block_name(e), e.extra.as_ref()) else {
                continue;
            };
            for a in &x.attribs {
                let v = a.value.trim();
                if v.is_empty() {
                    continue;
                }
                let tag = a.tag.to_ascii_uppercase();
                *seen
                    .entry(tag.clone())
                    .or_default()
                    .entry(v.to_owned())
                    .or_default() += 1;
                let bl = blocks.entry(tag).or_default();
                if !bl.iter().any(|x| x.eq_ignore_ascii_case(b)) {
                    bl.push(b.to_ascii_uppercase());
                }
            }
        }
    }
    let mut out = format!("set sheets {}\n", docs.len());
    let repeats = mutant::active("PairingCountsRepeats");
    for (tag, values) in &seen {
        let (mut paired, mut single, mut more, mut ids) = (0u64, 0u64, 0u64, 0u64);
        for (v, n) in values {
            match n {
                1 => single += 1,
                2 => paired += 1,
                _ if repeats => paired += 1,
                _ => more += 1,
            }
            ids += u64::from(id_shape(v).is_some());
        }
        if paired == 0 || paired * 2 < values.len() as u64 {
            continue;
        }
        let mut bl = blocks.get(tag).cloned().unwrap_or_default();
        bl.sort();
        out.push_str(&format!(
            "pairing-candidate tag {tag} blocks {} values {} paired {paired} single {single} more {more} id-shaped {ids}\n",
            bl.join(","),
            values.len()
        ));
    }
    out
}
