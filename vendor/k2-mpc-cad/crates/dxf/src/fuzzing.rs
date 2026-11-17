//! The properties every read keeps on any input (the test-only `fuzzing` feature), for the fuzz
//! targets and the seed tests: a typed refusal, or a result whose tokens write back to the
//! input and whose model and index are consistent. A violated property panics, like a crash.

use k2_mpc_base::index::IndexLimits;

use crate::{read_doc, token, verbs, Limits};

/// Every read on `data`.
///
/// # Panics
/// When a property fails.
pub fn read(data: &[u8]) {
    let limits = Limits {
        max_bytes: 1 << 24,
        max_line: 1 << 16,
        max_pairs: 1 << 20,
        max_entity_pairs: 1 << 16,
    };
    let Ok(t) = token::tokenize(data, &limits) else {
        return;
    };
    assert_eq!(t.reemit(), data, "the tokens don't write back to the input");
    let Ok(doc) = read_doc(data, &limits) else {
        return;
    };
    for e in &doc.entities {
        if let Some(o) = e.owner {
            assert!(o < doc.blocks.len(), "an owner outside the block records");
        }
        if let Some(b) = e.bbox {
            assert!(b[0] <= b[2] && b[1] <= b[3], "a box upside down");
        }
    }
    let il = IndexLimits::new(100_000, 64, 64 << 20, 64 << 10);
    if let Ok(d) = k2_mpc_cad_model::index::draft(&doc, &il) {
        for (i, n) in d.nodes.iter().enumerate() {
            if let Some(p) = n.parent {
                assert!((p as usize) < i, "a parent after its child");
            }
        }
    }
    for (verb, args) in [
        ("drawing info", &[][..]),
        ("layer list", &[][..]),
        ("block list", &["--anon"][..]),
        ("entity find", &["--in", "all"][..]),
        ("text find", &["--contains", "a"][..]),
        ("header get", &["$ACADVER"][..]),
        ("attrib find", &["--tag", "a"][..]),
        ("entity find", &["--ltype", "continuous"][..]),
        ("check", &["--writeback"][..]),
    ] {
        let _ = verbs::run(data, verb, args);
    }
}
