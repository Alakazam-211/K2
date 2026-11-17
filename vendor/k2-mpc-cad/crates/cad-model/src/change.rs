//! What one edit changed, per entity: `cad.change/1`, the shape a viewer reads to outline a
//! change (`address`, `op`, `kind`, `layer`, `owner`, `fields`, `bbox_before`, `bbox_after`,
//! `bbox_q`). For one call the range is the call's own version, which the kit's result carries:
//! `from`, `to`, `by` and `version` are null here, and `versions` empty.

use k2_mpc_base::errors::Value;

use crate::model::{BlockKind, Doc, Entity};

fn f(x: f64) -> Value {
    Value::float(x).unwrap_or(Value::Null)
}

fn bx(b: Option<[f64; 4]>) -> Value {
    b.map_or(Value::Null, |b| {
        Value::List(b.iter().map(|x| f(*x)).collect())
    })
}

/// Where entity `e` is: `model`, a layout's name, or `@h:<block>` inside a block definition.
fn owner(doc: &Doc, e: &Entity) -> Value {
    let Some(o) = e.owner else {
        return Value::Null;
    };
    let b = &doc.blocks[o];
    match b.kind {
        BlockKind::Model => Value::from("model"),
        BlockKind::Layout => Value::from(b.layout.clone().unwrap_or_else(|| b.name.clone())),
        BlockKind::Block => Value::from(doc.block_address(o)),
    }
}

/// The fields of an entity that differ between `a` and `b`, by the shape's words.
fn fields(a: &Entity, b: &Entity) -> Vec<&'static str> {
    let mut out = Vec::new();
    if a.geom != b.geom || a.bbox != b.bbox {
        out.push("geom");
    }
    if a.layer != b.layer {
        out.push("layer");
    }
    let (pa, pb) = (a.props.as_deref(), b.props.as_deref());
    let get = |p: Option<&crate::model::Props>, k: usize| {
        p.and_then(|p| [&p.color, &p.rgb, &p.ltype, &p.lw, &p.alpha][k].clone())
    };
    for (k, w) in ["color", "rgb", "ltype", "lw", "alpha"].iter().enumerate() {
        if get(pa, k) != get(pb, k) {
            out.push(*w);
        }
    }
    if a.text != b.text {
        out.push("text");
    }
    if a.owner != b.owner {
        out.push("owner");
    }
    if a.content != b.content && out.is_empty() {
        out.push("other");
    }
    out
}

fn row(doc: &Doc, e: &Entity, op: &str, flds: Vec<&'static str>, before: Option<&Entity>) -> Value {
    let (b0, b1) = match op {
        "added" => (None, e.bbox),
        "removed" => (e.bbox, None),
        _ => (before.and_then(|x| x.bbox), e.bbox),
    };
    Value::obj([
        (
            "address",
            Value::from(e.handle.map_or_else(String::new, crate::handle::key)),
        ),
        ("op", Value::from(op)),
        ("kind", Value::from(&*e.kind)),
        ("layer", Value::from(&*e.layer)),
        ("owner", owner(doc, e)),
        (
            "fields",
            Value::List(flds.into_iter().map(Value::from).collect()),
        ),
        ("bbox_before", bx(b0)),
        ("bbox_after", bx(b1)),
        ("bbox_q", Value::from(e.bbox_q.word())),
        ("version", Value::Null),
        ("by", Value::Null),
        ("identity", Value::Null),
    ])
}

/// `cad.change/1` for one edit of `before` into `after`: the entities `changed`, `added` and
/// `removed` (by handle), in that order. `format` is the drawing's.
pub fn of_edit(
    before: &Doc,
    after: &Doc,
    changed: &[u64],
    added: &[u64],
    removed: &[u64],
) -> Value {
    let find = |d: &Doc, h: u64| d.entities.iter().find(|e| e.handle == Some(h)).cloned();
    let mut rows = Vec::new();
    for &h in changed {
        if let (Some(a), Some(b)) = (find(before, h), find(after, h)) {
            rows.push(row(after, &b, "changed", fields(&a, &b), Some(&a)));
        }
    }
    for &h in added {
        if let Some(b) = find(after, h) {
            rows.push(row(after, &b, "added", Vec::new(), None));
        }
    }
    for &h in removed {
        if let Some(a) = find(before, h) {
            rows.push(row(before, &a, "removed", Vec::new(), None));
        }
    }
    Value::obj([
        ("schema", Value::from("cad.change/1")),
        ("format", Value::from(after.format)),
        ("from", Value::Null),
        ("to", Value::Null),
        ("units", crate::reads::units(&after.units)),
        ("pairing", Value::from("kept")),
        ("versions", Value::List(Vec::new())),
        ("entities", Value::List(rows)),
        ("entities_more", Value::from(0u64)),
        ("tables", Value::List(Vec::new())),
        ("blocks", Value::List(Vec::new())),
        ("page", Value::Null),
    ])
}
