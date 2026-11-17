//! What we write is well formed: from 2000 on, every entity we write (by the generator, by our
//! splices, by the merge) carries its subclass markers, in order: `AcDbEntity`, then its kind's
//! own. R12 drawings carry none (it has no subclasses).

use k2_mpc_cad_model::merge::MergeOutcome;
use k2_mpc_dxf::testgen::{self, splice_insert, splice_move, Ver};
use k2_mpc_dxf::{structure, token, Limits};

/// The markers each kind we write carries after `AcDbEntity`, in order: our own table.
fn markers(kind: &str) -> Option<&'static [&'static str]> {
    Some(match kind {
        "LINE" => &["AcDbLine"],
        "POINT" => &["AcDbPoint"],
        "CIRCLE" => &["AcDbCircle"],
        "ARC" => &["AcDbCircle", "AcDbArc"],
        "LWPOLYLINE" => &["AcDbPolyline"],
        "POLYLINE" => &["AcDb2dPolyline"],
        "VERTEX" => &["AcDbVertex", "AcDb2dVertex"],
        "SEQEND" => &[],
        "TEXT" => &["AcDbText", "AcDbText"],
        "ATTRIB" => &["AcDbText", "AcDbAttribute"],
        "MTEXT" => &["AcDbMText"],
        "INSERT" => &["AcDbBlockReference"],
        "ELLIPSE" => &["AcDbEllipse"],
        "SPLINE" => &["AcDbSpline"],
        _ => return None,
    })
}

/// Every entity unit's markers, checked; how many units were.
fn check(name: &str, bytes: &[u8], modern: bool) -> usize {
    let limits = Limits::default();
    let s = structure::walk(token::tokenize(bytes, &limits).unwrap(), &limits).unwrap();
    let mut n = 0;
    for e in &s.entities {
        for u in std::iter::once(&e.head).chain(&e.followers) {
            let got: Vec<String> = (u.first + 1..u.end)
                .filter(|&i| s.tok.pairs[i].code == 100)
                .map(|i| s.text(i).into_owned())
                .collect();
            if !modern {
                assert!(got.is_empty(), "{name}: an R12 {} carries {got:?}", u.kind);
                n += 1;
                continue;
            }
            let own =
                markers(&u.kind).unwrap_or_else(|| panic!("{name}: a {} we don't write", u.kind));
            let mut want = vec!["AcDbEntity".to_owned()];
            want.extend(own.iter().map(|m| (*m).to_owned()));
            assert_eq!(
                got, want,
                "{name}: a {} with its subclass markers out of place",
                u.kind
            );
            n += 1;
        }
    }
    n
}

#[test]
pub fn subclass_markers_in_order() {
    let mut units = 0;
    for v in Ver::ALL {
        let modern = v != Ver::R12;
        for (name, s) in [
            ("sampler", testgen::sampler(v)),
            ("nested", testgen::nested_blocks(v)),
        ] {
            units += check(&format!("{name}-{v:?}"), &s.build().bytes, modern);
        }
    }
    for name in testgen::H1_NAMES {
        let modern = name != "h1-r12-handles";
        for (file, bytes) in testgen::h1_files(name).expect("files") {
            units += check(&file, &bytes, modern);
        }
    }
    // the merge's outputs: ours' changes written into theirs
    let b = testgen::sampler(Ver::R2018).build();
    let ours = splice_insert(
        &splice_move(&b.bytes, b.handle("wall"), 1.0),
        b.handle("wall"),
        8,
        62,
        "3",
    );
    let mut with_added = testgen::sampler(Ver::R2018);
    with_added
        .model
        .push(testgen::Ent::lwpoly(&[([0.0, 0.0], 0.0), ([1.0, 1.0], 0.0)], false).handle(0x900));
    for (case, o) in [("fields", ours), ("added", with_added.build().bytes)] {
        let theirs = splice_move(&b.bytes, b.handle("lamp"), 2.0);
        match k2_mpc_dxf::merge::merge(&b.bytes, &o, &theirs) {
            MergeOutcome::Merged { bytes, .. } => {
                units += check(&format!("merge-{case}"), &bytes, true)
            }
            other => panic!(
                "merge-{case} didn't merge: {}",
                matches!(other, MergeOutcome::Conflict(_))
            ),
        }
    }
    assert!(units > 500, "only {units} units checked");
    println!("subclass_markers_in_order: {units} entity units checked");
}
