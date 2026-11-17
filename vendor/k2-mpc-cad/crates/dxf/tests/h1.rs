//! The drawings the handle-stability measurements start from: each reads, carries its two
//! labelled entities and a ground-truth serial on every entity, and its spliced companion
//! differs from it only in the moved entity's point values.

mod support;

use k2_mpc_dxf::testgen::{h1, h1_files, H1_NAMES, SERIAL_APP};
use support::*;

#[test]
pub fn h1_drawings_read_and_splice() {
    let mut entities = 0usize;
    for name in H1_NAMES {
        let b = h1(name).expect("a drawing").build();
        let d = draft(&b.bytes);
        let ents: Vec<_> = d
            .nodes
            .iter()
            .filter(|n| n.attrs.contains_key("geom"))
            .collect();
        assert!(ents.len() >= 14, "{name}: {} entities", ents.len());
        for n in &ents {
            assert_eq!(
                attr(n, "xd"),
                SERIAL_APP,
                "{name}: {:?} has no serial",
                n.key
            );
        }
        entities += ents.len();
        let texts: Vec<&str> = ents.iter().filter_map(|n| n.text.as_deref()).collect();
        assert!(
            texts.contains(&"MOVE-ME") && texts.contains(&"DELETE-ME"),
            "{name}"
        );
        let check = run(&b.bytes, "check", &[]);
        for f in list(&check.data, &["findings"]) {
            assert_eq!(string(f, &["severity"]), "info", "{name}: {}", f.to_json());
        }
        // the companion: only MOVE-ME's geometry changed, every other node the same
        let [(_, orig), (_, spliced)] = h1_files(name).expect("files");
        assert_eq!(orig, b.bytes);
        let ds = draft(&spliced);
        let moved = b.addr("move-me");
        assert_eq!(ds.nodes.len(), d.nodes.len(), "{name}");
        for (a, z) in d.nodes.iter().zip(&ds.nodes) {
            assert_eq!(a.key, z.key, "{name}");
            if a.key.as_deref() == Some(moved.as_str()) {
                assert_ne!(
                    attr(a, "geom"),
                    attr(z, "geom"),
                    "{name}: MOVE-ME didn't move"
                );
            } else {
                assert!(a.record == z.record, "{name}: {:?} changed", a.key);
            }
        }
        // and outside its two point lines, the bytes are the same
        let same_head = orig
            .iter()
            .zip(&spliced)
            .take_while(|(x, y)| x == y)
            .count();
        let same_tail = orig
            .iter()
            .rev()
            .zip(spliced.iter().rev())
            .take_while(|(x, y)| x == y)
            .count();
        assert!(
            same_head + same_tail + 40 >= orig.len(),
            "{name}: the splice strays"
        );
    }
    println!(
        "h1_drawings_read_and_splice: {} drawings, {entities} entities",
        H1_NAMES.len()
    );
}
