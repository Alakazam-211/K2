//! The structure index of a [`Doc`]: the kit's [`IndexDraft`].
//!
//! | Node | Key | Parent | Name | Attributes |
//! |---|---|---|---|---|
//! | the drawing (`drawing`) | `@doc:<format>` | none | the version stamp | none: the doc attributes hold the file's facts |
//! | a layer (`layer`) | `@h:<handle>` | the drawing | its name | `color`, `on`, `frozen`, `locked`, `plot`, `ltype`, `lw`, `xref` |
//! | model space, a layout, a block (`model`, `layout`, `block`) | `@h:<handle>` | the drawing | its name | `base`, `anon`, `xref` |
//! | an entity (its kind, lower case) | `@h:<handle>` | its owner | its kind, upper case | `layer`, `layer_key`, `block_key`, `color`, `rgb`, `ltype`, `lw`, `alpha`, `geom`, `bbox`, `bbox_q`, `ocs`, `xd`, `edit`, `followers` |
//!
//! **Each attribute is a fact of the node's own record.** No node counts other records (the
//! entities on a layer, a block's inserts), so an edit to one entity changes that entity's node
//! alone, and two edits to different entities never change the same node. An entity's name is
//! its kind alone, so a move is a change of `geom` and `bbox`, never a rename. Its content digest
//! covers only what no attribute states, so a layer or colour change is a change of that one
//! field.
//!
//! **Defaults are left out of what is stored**, so a drawing's index stays small: an entity's
//! `bbox_q` when it is `exact`, and its `edit` when it is `1` (the edit verbs can change it); its
//! kind is its node's name. [`with_defaults`] gives them back, so every read shows the same
//! fields whether or not they were stored ([`draft_full`] stores them all, for comparison).
//!
//! Bounds are the entity's box in thousandths of the drawing's unit (`bounds_scale` 1000), the
//! y axis up (`y_axis`).

use std::collections::BTreeMap;

use k2_mpc_base::index::{
    cut_text, Digest, DraftNode, IndexDraft, IndexLimits, IndexRefused, NodeRect,
    INDEX_ATTR_VALUE_MAX, INDEX_NAME_MAX,
};

use crate::geom::box_text;
use crate::handle;
use crate::model::{BboxQ, Doc};
use crate::mutant;

/// The bounds' scale: thousandths of the drawing's unit.
pub const BOUNDS_SCALE: f64 = 1000.0;

fn cut(s: &str, max: usize) -> String {
    cut_text(s, max as u32).0.to_owned()
}

fn hex16(d: &Digest) -> String {
    d.as_bytes()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn bounds(b: [f64; 4]) -> Option<NodeRect> {
    let lim = 1e15;
    if b.iter()
        .any(|v| !v.is_finite() || v.abs() * BOUNDS_SCALE > lim)
    {
        return None;
    }
    Some(NodeRect {
        left: (b[0] * BOUNDS_SCALE).floor() as i64,
        top: (b[1] * BOUNDS_SCALE).floor() as i64,
        right: (b[2] * BOUNDS_SCALE).ceil() as i64,
        bottom: (b[3] * BOUNDS_SCALE).ceil() as i64,
    })
}

fn yes(b: bool) -> String {
    if b { "1" } else { "0" }.into()
}

/// The index draft of `doc`, within the kit's `limits`.
/// An entity's attributes left out of the index when they hold these values.
pub const ENTITY_DEFAULTS: [(&str, &str); 2] = [("bbox_q", "exact"), ("edit", "1")];

/// An entity node's attributes as every read shows them: the stored ones, the defaults that
/// were left out ([`ENTITY_DEFAULTS`]), and `kind`, its node's name. A node that isn't an entity
/// (no `geom` attribute) is given back as it is.
pub fn with_defaults(
    name: &str,
    attrs: &std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, String> {
    let mut a = attrs.clone();
    if !a.contains_key("geom") {
        return a;
    }
    for (k, v) in ENTITY_DEFAULTS {
        if k == "bbox_q" && mutant::active("DefaultNotFilled") {
            continue;
        }
        a.entry(k.to_owned()).or_insert_with(|| v.to_owned());
    }
    a.entry("kind".to_owned())
        .or_insert_with(|| name.to_owned());
    a
}

/// The index draft of `doc`, within the kit's `limits`, defaults left out.
pub fn draft(doc: &Doc, limits: &IndexLimits) -> Result<IndexDraft, IndexRefused> {
    build(doc, limits, false)
}

/// The index draft of `doc` with every attribute written out, defaults and `kind` included:
/// what [`with_defaults`] gives back from [`draft`]'s nodes.
pub fn draft_full(doc: &Doc, limits: &IndexLimits) -> Result<IndexDraft, IndexRefused> {
    build(doc, limits, true)
}

fn build(doc: &Doc, limits: &IndexLimits, full: bool) -> Result<IndexDraft, IndexRefused> {
    let count = 1 + doc.layers.len() + doc.blocks.len() + doc.entities.len();
    if count as u64 > u64::from(limits.max_nodes) {
        return Err(IndexRefused::TooManyNodes {
            count: count as u64,
            limit: limits.max_nodes,
        });
    }
    let mut d = IndexDraft::new(format!("{}-{}", doc.format, doc.version));
    let a = &mut d.attrs;
    a.insert("format".into(), doc.format.into());
    a.insert("acadver".into(), cut(&doc.version, INDEX_ATTR_VALUE_MAX));
    if let Some(r) = doc.release {
        a.insert("release".into(), r.into());
    }
    a.insert("encoding".into(), cut(&doc.encoding, INDEX_ATTR_VALUE_MAX));
    if let Some(e) = doc.eol {
        a.insert("eol".into(), e.into());
    }
    a.insert("units".into(), doc.units.word.into());
    if let Some(u) = doc.units.insunits {
        a.insert("insunits".into(), u.to_string());
    }
    if let Some(m) = doc.units.measurement {
        a.insert("measurement".into(), m.to_string());
    }
    a.insert("handles".into(), doc.handles.into());
    if let Some(w) = &doc.writer_mark {
        a.insert("writer_mark".into(), cut(w, 120));
    }
    a.insert("y_axis".into(), "up".into());
    a.insert("bounds_scale".into(), "1000".into());

    let mut root = DraftNode::new("drawing", cut(&doc.version, INDEX_NAME_MAX), doc.header);
    root.key = Some(format!("@doc:{}", doc.format));
    d.nodes.push(root);

    // layers, by file order: entity counts never go here (CountOnLayerNode plants one)
    let mut ents_on: BTreeMap<String, usize> = BTreeMap::new();
    if mutant::active("CountOnLayerNode") {
        for e in &doc.entities {
            *ents_on.entry(e.layer.to_ascii_uppercase()).or_default() += 1;
        }
    }
    for l in &doc.layers {
        let mut n = DraftNode::new("layer", cut(&l.name, INDEX_NAME_MAX), l.record);
        n.key = l.handle.map(handle::key);
        n.parent = Some(0);
        n.content = Some(l.content);
        let at = &mut n.attrs;
        if let Some(c) = l.color {
            at.insert("color".into(), c.abs().to_string());
        }
        at.insert("on".into(), yes(l.on()));
        at.insert("frozen".into(), yes(l.frozen));
        at.insert("locked".into(), yes(l.locked));
        at.insert("plot".into(), yes(l.plot));
        if let Some(t) = &l.ltype {
            at.insert("ltype".into(), cut(t, INDEX_ATTR_VALUE_MAX));
        }
        if let Some(w) = &l.lw {
            at.insert("lw".into(), cut(w, INDEX_ATTR_VALUE_MAX));
        }
        at.insert("xref".into(), yes(l.xref));
        if let Some(c) = ents_on.get(&l.name.to_ascii_uppercase()) {
            at.insert("ents".into(), c.to_string());
        }
        n.visible = l.on() && !l.frozen;
        if l.duplicate {
            n.flags.insert("duplicate_handle".into());
        }
        d.nodes.push(n);
    }

    // block records
    let first_block = d.nodes.len();
    for b in &doc.blocks {
        let mut n = DraftNode::new(b.kind.word(), cut(&b.name, INDEX_NAME_MAX), b.record);
        n.key = b.handle.map(handle::key).or_else(|| b.key.clone());
        n.parent = Some(0);
        n.content = Some(b.content);
        if let Some(p) = b.base {
            n.attrs.insert(
                "base".into(),
                format!(
                    "{},{},{}",
                    crate::geom::num(p[0]),
                    crate::geom::num(p[1]),
                    crate::geom::num(p[2])
                ),
            );
        }
        n.attrs.insert("anon".into(), yes(b.anon));
        n.attrs.insert("xref".into(), yes(b.xref));
        if b.duplicate {
            n.flags.insert("duplicate_handle".into());
        }
        d.nodes.push(n);
    }

    // entities, under their owners
    for e in &doc.entities {
        let name = if mutant::active("PointInName") {
            match e.bbox {
                Some(b) => format!(
                    "{} {},{}",
                    e.kind,
                    crate::geom::num(b[0]),
                    crate::geom::num(b[1])
                ),
                None => e.kind.to_string(),
            }
        } else {
            e.kind.to_string()
        };
        let mut n = DraftNode::new(
            cut(&e.kind.to_ascii_lowercase(), 64),
            cut(&name, INDEX_NAME_MAX),
            e.record,
        );
        n.key = e.handle.map(handle::key);
        n.parent = Some(e.owner.map(|o| (first_block + o) as u32).unwrap_or(0));
        n.content = Some(e.content);
        n.bounds = e.bbox.and_then(bounds);
        let at = &mut n.attrs;
        if full {
            at.insert("kind".into(), cut(&e.kind, INDEX_ATTR_VALUE_MAX));
        }
        at.insert("layer".into(), cut(&e.layer, INDEX_ATTR_VALUE_MAX));
        if let Some(h) = doc.layer_named(&e.layer).and_then(|i| doc.layers[i].handle) {
            at.insert("layer_key".into(), handle::key(h));
        }
        if let Some(bn) = &e.block {
            if let Some(i) = doc.block_named(bn) {
                at.insert("block_key".into(), doc.block_address(i));
            }
        }
        if let Some(p) = &e.props {
            for (k, v) in [
                ("color", &p.color),
                ("rgb", &p.rgb),
                ("ltype", &p.ltype),
                ("lw", &p.lw),
                ("alpha", &p.alpha),
            ] {
                if let Some(v) = v {
                    at.insert(k.into(), cut(v, INDEX_ATTR_VALUE_MAX));
                }
            }
        }
        at.insert(
            "geom".into(),
            e.geom.iter().map(|b| format!("{b:02x}")).collect(),
        );
        if let Some(b) = e.bbox {
            at.insert("bbox".into(), box_text(b));
        }
        if full || e.bbox_q != BboxQ::Exact {
            at.insert("bbox_q".into(), e.bbox_q.word().into());
        }
        if e.ocs {
            at.insert("ocs".into(), "1".into());
        }
        if !e.xd.is_empty() {
            at.insert("xd".into(), cut(&e.xd.join(","), INDEX_ATTR_VALUE_MAX));
        }
        if full || e.edit != "1" {
            at.insert("edit".into(), e.edit.into());
        }
        if e.followers > 0 {
            at.insert("followers".into(), e.followers.to_string());
        }
        if let Some(t) = &e.text {
            let (t, cut_) = cut_text(t, limits.max_text_bytes);
            n.text = Some(t.to_owned());
            if cut_ {
                n.flags.insert("text_truncated".into());
            }
        }
        if e.duplicate {
            n.flags.insert("duplicate_handle".into());
        }
        if e.bbox_q == BboxQ::None {
            n.flags.insert("no_box".into());
        }
        n.weak.insert("rec".into(), hex16(&e.record));
        d.nodes.push(n);
    }
    Ok(d)
}
