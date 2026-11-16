//! Test-only (feature `fuzzing`): the properties each reader must keep on any input, shared by
//! the cargo-fuzz targets (`fuzz/`) and the seed tests. A violated property panics, like a
//! crash: it is a finding.
//!
//! On any bytes a reader either refuses with a typed error whose offset is inside the input, or
//! returns a model that:
//!
//! - reads the same twice (determinism);
//! - holds only references that resolve (apertures, contours, segments, attribute sets,
//!   transforms, step and repeats, tools);
//! - stays within the limits it was read under;
//! - indexes into a draft whose parents come first and whose keys are node keys, each once, the
//!   same twice.

use std::collections::BTreeSet;

use k2_mpc_base::index::{IndexDraft, IndexLimits, NodeKey, INDEX_TEXT_MAX};

use crate::photoplot::Shape;
use crate::{drill, photoplot, Limits};

/// The bounds the fuzz targets read under: small, so a run stays fast.
pub fn limits() -> Limits {
    Limits {
        max_bytes: 1 << 20,
        max_word_bytes: 64 << 10,
        max_objects: 200_000,
        max_expansion: 1_000_000,
        max_points: 400_000,
        max_apertures: 10_000,
        max_macros: 1_000,
        max_expr_depth: 32,
        max_macro_work: 2_000_000,
        max_block_depth: 8,
        max_attr_sets: 10_000,
    }
}

fn index_limits() -> IndexLimits {
    IndexLimits::new(1_000_000, 64, 1 << 30, INDEX_TEXT_MAX)
}

fn draft_ok(d: &IndexDraft) {
    let mut keys = BTreeSet::new();
    for (i, n) in d.nodes.iter().enumerate() {
        if let Some(p) = n.parent {
            assert!((p as usize) < i, "index node {i}: its parent comes later");
        }
        if let Some(k) = &n.key {
            assert!(NodeKey::parse(k).is_some(), "index key {k}");
            assert!(keys.insert(k.clone()), "index key {k} twice");
        }
    }
}

/// The photoplot reader's properties on `data`.
pub fn photoplot(data: &[u8]) {
    let l = limits();
    let r = photoplot::read(data, &l);
    assert_eq!(r, photoplot::read(data, &l), "reading is deterministic");
    let m = match r {
        Ok(m) => m,
        Err(e) => {
            assert!(e.offset <= data.len(), "an offset past the input: {e}");
            return;
        }
    };
    assert!(m.objects.len() as u64 <= l.max_objects);
    assert!(m.expanded_objects <= l.max_expansion);
    for o in &m.objects {
        if o.aperture != 0 {
            assert!(m.aperture(o.aperture).is_some(), "object with no aperture");
        }
        assert!((o.transform as usize) < m.transforms.len());
        assert!((o.attrs as usize) < m.object_attr_sets.len());
        assert!((o.region_attrs as usize) < m.region_attr_sets.len());
        assert!(o.step_repeat as usize <= m.step_repeats.len());
        if let Shape::Region { contours } = &o.shape {
            assert!(contours.end as usize <= m.contours.len());
        }
    }
    for c in &m.contours {
        assert!(c.segments.end as usize <= m.segments.len());
        // a contour is closed (4.10.1)
        let end = m.segments[c.segments.end as usize - 1].end();
        assert_eq!(end, c.start, "an open contour in the model");
    }
    for a in &m.apertures {
        if let photoplot::Template::Macro(name) = &a.template {
            assert!(m.macros.iter().any(|x| &x.name == name));
        }
    }
    let il = index_limits();
    if let Ok(d) = crate::index_photoplot(&m, &il) {
        draft_ok(&d);
        assert_eq!(
            Some(d),
            crate::index_photoplot(&m, &il).ok(),
            "indexing is deterministic"
        );
    }
}

/// The drill reader's properties on `data`.
pub fn drill(data: &[u8]) {
    let l = limits();
    let r = drill::read(data, &l);
    assert_eq!(r, drill::read(data, &l), "reading is deterministic");
    let m = match r {
        Ok(m) => m,
        Err(e) => {
            assert!(e.offset <= data.len(), "an offset past the input: {e}");
            return;
        }
    };
    for h in &m.hits {
        assert!(m.tool(h.tool).is_some(), "a hit with no tool");
        assert!((h.attrs as usize) < m.hole_attr_sets.len());
    }
    for r in &m.routs {
        assert!(m.tool(r.tool).is_some(), "a rout with no tool");
        assert!(r.segments.end as usize <= m.segments.len());
        assert!((r.attrs as usize) < m.hole_attr_sets.len());
    }
    let il = index_limits();
    if let Ok(d) = crate::index_drill(&m, &il) {
        draft_ok(&d);
        assert_eq!(
            Some(d),
            crate::index_drill(&m, &il).ok(),
            "indexing is deterministic"
        );
    }
}

/// The common-export drill reading's properties on `data`: as the strict reader's, and a file
/// it reads is always reported not conforming.
pub fn drill_common(data: &[u8]) {
    let l = limits();
    let r = drill::common::read(data, &l);
    assert_eq!(r, drill::common::read(data, &l), "reading is deterministic");
    let c = match r {
        Ok(c) => c,
        Err(e) => {
            assert!(e.offset <= data.len(), "an offset past the input: {e}");
            return;
        }
    };
    assert_eq!(
        c.conformance(),
        crate::error::Conformance::NonConforming,
        "a common read reported conforming"
    );
    let m = &c.drill;
    for h in &m.hits {
        assert!(m.tool(h.tool).is_some(), "a hit with no tool");
        assert!((h.attrs as usize) < m.hole_attr_sets.len());
    }
    for r in &m.routs {
        assert!(m.tool(r.tool).is_some(), "a rout with no tool");
        assert!(r.segments.end as usize <= m.segments.len());
    }
    let il = index_limits();
    if let Ok(d) = crate::index_common(&c, None, &il) {
        draft_ok(&d);
        assert_eq!(
            d.attrs.get("conformance").map(String::as_str),
            Some("non-conforming")
        );
        assert_eq!(
            Some(d),
            crate::index_common(&c, None, &il).ok(),
            "indexing is deterministic"
        );
    }
}
