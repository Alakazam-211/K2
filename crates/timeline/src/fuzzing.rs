//! Test-only (feature `fuzzing`): the properties each reader must keep on any input, shared by
//! the cargo-fuzz targets (`fuzz/`) and the seed tests. A violated property panics, like a
//! crash: it is a finding.
//!
//! On any bytes a reader either refuses with a typed error or returns a document that:
//!
//! - reads the same twice (determinism);
//! - passes [`Document::check`] but for overlaps, which a file may hold (and for OTIO, which
//!   places items one after another, passes it entirely);
//! - has every key not marked long in the kit's node-key grammar;
//! - round-trips every overlap-free track through its sequential form;
//! - indexes into a draft whose parents come first and whose keys are node keys, the same
//!   twice.

use k2_mpc_base::index::{IndexLimits, NodeKey, INDEX_TEXT_MAX};

use crate::error::{ReadError, ReadLimits};
use crate::model::{place_steps, Document, Finding, KeyOrigin, Track};

/// The bounds the fuzz targets read under: small, so a run stays fast.
pub fn limits() -> ReadLimits {
    ReadLimits::new(1 << 20, 64, 200_000)
}

fn tracks(ts: &[Track]) {
    for t in ts {
        if let Ok(steps) = t.to_steps() {
            let placed = place_steps(&steps, &t.items).expect("placing a track's own steps");
            for (n, start) in placed {
                assert_eq!(
                    start, t.items[n].start,
                    "sequential form round trip on {}",
                    t.key
                );
            }
        }
        for i in &t.items {
            tracks(&i.tracks);
        }
    }
}

/// The properties of one read: `read` is the reader, `strict` whether overlaps are findings.
pub fn properties(
    data: &[u8],
    read: fn(&[u8], &ReadLimits) -> Result<Document, ReadError>,
    strict: bool,
) {
    let limits = limits();
    let Ok(doc) = read(data, &limits) else {
        return;
    };
    let again = read(data, &limits).expect("a second read of the same bytes");
    assert_eq!(doc, again, "reading is deterministic");
    for f in doc.check() {
        if !strict && matches!(f, Finding::Overlap { .. }) {
            continue;
        }
        panic!("a reader produced a model with {f:?}");
    }
    for k in doc.keys() {
        if k.origin() != KeyOrigin::Long {
            assert!(NodeKey::parse(k.as_str()).is_some(), "not a node key: {k}");
        }
    }
    for t in &doc.timelines {
        tracks(&t.tracks);
    }
    // the index of the model: parents before children, keys in the grammar, deterministic
    let il = IndexLimits::new(1_000_000, 256, 1 << 30, INDEX_TEXT_MAX);
    if let Ok(draft) = crate::pack::index_document(&doc, &il) {
        for (i, n) in draft.nodes.iter().enumerate() {
            if let Some(p) = n.parent {
                assert!((p as usize) < i, "index node {i}: its parent comes later");
            }
            if let Some(k) = &n.key {
                assert!(NodeKey::parse(k).is_some(), "index key {k}");
            }
        }
        assert_eq!(
            Some(draft),
            crate::pack::index_document(&again, &il).ok(),
            "indexing is deterministic"
        );
    }
}

/// The OTIO reader's properties on `data`.
pub fn otio(data: &[u8]) {
    properties(data, crate::otio::read_otio, true);
}

/// The FCPXML reader's properties on `data` (connected clips on one lane may overlap).
pub fn fcpxml(data: &[u8]) {
    properties(data, crate::fcpxml::read_fcpxml, false);
}

/// The PRPROJ reader's properties on `data` (as gzip, and as plain XML; a file's clips may
/// overlap).
pub fn prproj(data: &[u8]) {
    properties(data, crate::prproj::read_prproj, false);
}
