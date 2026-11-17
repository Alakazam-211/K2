//! The three-way merge by unit and field: a person's app saving the file (theirs) while agents
//! moved the doc on (ours), from the version both started from (the base). Each row builds the
//! expected file independently (ours' splice applied straight to theirs' bytes) and compares
//! the merge with it byte for byte.

use k2_mpc_base::session::{BlobMerge, BlobPack};
use k2_mpc_cad_model::merge::{MergeConflict, MergeOutcome};
use k2_mpc_dxf::testgen::{
    self, splice_delete, splice_insert, splice_move, splice_value, Ent, Ver,
};
use k2_mpc_dxf::{DxfPack, Limits};

fn merged(base: &[u8], ours: &[u8], theirs: &[u8]) -> Vec<u8> {
    match k2_mpc_dxf::merge::merge(base, ours, theirs) {
        MergeOutcome::Merged { bytes, .. } => bytes,
        MergeOutcome::Conflict(c) => panic!("a conflict: {c:?}"),
        MergeOutcome::Unsupported(why) => panic!("unsupported: {why}"),
    }
}

fn conflicts(base: &[u8], ours: &[u8], theirs: &[u8]) -> Vec<MergeConflict> {
    match k2_mpc_dxf::merge::merge(base, ours, theirs) {
        MergeOutcome::Conflict(c) => c,
        MergeOutcome::Merged { .. } => panic!("merged where a conflict was due"),
        MergeOutcome::Unsupported(why) => panic!("unsupported: {why}"),
    }
}

#[test]
pub fn merge_disjoint_entities() {
    let b = testgen::sampler(Ver::R2018).build();
    // two agents' changes (ours): the wall moved, the room renamed; the person's (theirs): the
    // lamp moved
    let ours = splice_value(
        &splice_move(&b.bytes, b.handle("wall"), 1.0),
        b.handle("room"),
        1,
        "ROOM 102",
    );
    let theirs = splice_move(&b.bytes, b.handle("lamp"), 2.0);
    let want = splice_value(
        &splice_move(&theirs, b.handle("wall"), 1.0),
        b.handle("room"),
        1,
        "ROOM 102",
    );
    let got = merged(&b.bytes, &ours, &theirs);
    assert!(
        got == want,
        "the merge isn't theirs plus ours' changes, byte for byte"
    );
    println!(
        "merge_disjoint_entities: {} bytes, as theirs plus ours' two changes",
        got.len()
    );
}

#[test]
pub fn merge_same_entity_different_fields() {
    let b = testgen::sampler(Ver::R2018).build();
    // the wall: ours moved it, theirs gave it a colour
    let ours = splice_move(&b.bytes, b.handle("wall"), 1.0);
    let theirs = splice_insert(&b.bytes, b.handle("wall"), 8, 62, "5");
    let want = splice_move(&theirs, b.handle("wall"), 1.0);
    assert!(
        merged(&b.bytes, &ours, &theirs) == want,
        "geometry and colour of one entity"
    );
    // the lamp: ours changed its layer, theirs moved it
    let ours = splice_value(&b.bytes, b.handle("lamp"), 8, "A-WALL");
    let theirs = splice_move(&b.bytes, b.handle("lamp"), 3.0);
    let want = splice_value(&theirs, b.handle("lamp"), 8, "A-WALL");
    assert!(
        merged(&b.bytes, &ours, &theirs) == want,
        "layer and geometry of one entity"
    );
}

#[test]
pub fn merge_same_field_conflicts() {
    let b = testgen::sampler(Ver::R2018).build();
    let ours = splice_move(&b.bytes, b.handle("wall"), 1.0);
    let theirs = splice_move(&b.bytes, b.handle("wall"), 2.0);
    let c = conflicts(&b.bytes, &ours, &theirs);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(
        (c[0].address.as_str(), c[0].field.as_str()),
        (b.addr("wall").as_str(), "geom")
    );
    assert!(
        c[0].ours.contains("10=1") && c[0].theirs.contains("10=2"),
        "{c:?}"
    );
    // the same field, other values: the layer
    let ours = splice_value(&b.bytes, b.handle("lamp"), 8, "A-WALL");
    let theirs = splice_value(&b.bytes, b.handle("lamp"), 8, "ANNO");
    let c = conflicts(&b.bytes, &ours, &theirs);
    assert_eq!(
        (
            c[0].field.as_str(),
            c[0].ours.as_str(),
            c[0].theirs.as_str()
        ),
        ("layer", "8=A-WALL", "8=ANNO")
    );
    // deleted on one side, changed on the other
    let ours = splice_delete(&b.bytes, b.handle("wall"));
    let theirs = splice_move(&b.bytes, b.handle("wall"), 2.0);
    let c = conflicts(&b.bytes, &ours, &theirs);
    assert_eq!(
        (c[0].field.as_str(), c[0].ours.as_str()),
        ("whole", "deleted")
    );
    // the same change on both sides is no conflict
    let both = splice_move(&b.bytes, b.handle("wall"), 1.0);
    assert!(merged(&b.bytes, &both, &both) == both);
}

#[test]
pub fn merge_frame_never_guessed() {
    let mut s = testgen::sampler(Ver::R2018);
    s.opts.comments = true;
    let b = s.build();
    let edit_frame = |bytes: &[u8]| -> Vec<u8> {
        let at = bytes
            .windows(22)
            .position(|w| w == b"generated test drawing")
            .expect("the comment");
        [&bytes[..at], b"edited test drawing", &bytes[at + 22..]].concat()
    };
    // ours changed the frame: never guessed
    let ours = splice_move(&edit_frame(&b.bytes), b.handle("wall"), 1.0);
    let theirs = splice_move(&b.bytes, b.handle("lamp"), 2.0);
    match k2_mpc_dxf::merge::merge(&b.bytes, &ours, &theirs) {
        MergeOutcome::Unsupported(why) => assert!(why.contains("outside the units"), "{why}"),
        other => panic!(
            "ours' change to the frame merged: {}",
            matches!(other, MergeOutcome::Merged { .. })
        ),
    }
    // theirs changed it: theirs is the carrier, so it stays
    let ours = splice_move(&b.bytes, b.handle("wall"), 1.0);
    let theirs = edit_frame(&b.bytes);
    assert!(merged(&b.bytes, &ours, &theirs) == splice_move(&theirs, b.handle("wall"), 1.0));
}

#[test]
pub fn merge_adds_and_deletes() {
    let b = testgen::sampler(Ver::R2018).build();
    let mut with_added = testgen::sampler(Ver::R2018);
    with_added.model.push(
        Ent::line([50.0, 50.0], [60.0, 50.0])
            .on("E-LITE")
            .handle(0x900),
    );
    let ours = splice_delete(&with_added.build().bytes, b.handle("lamp"));
    let theirs = splice_move(&b.bytes, b.handle("wall"), 2.0);
    let got = merged(&b.bytes, &ours, &theirs);
    let doc = k2_mpc_dxf::read_doc(&got, &Limits::default()).unwrap();
    let has = |h: u64| doc.entities.iter().any(|e| e.handle == Some(h));
    assert!(has(0x900), "ours' added line is missing");
    assert!(!has(b.handle("lamp")), "ours' deleted lamp is still there");
    let wall = doc
        .entities
        .iter()
        .find(|e| e.handle == Some(b.handle("wall")))
        .unwrap();
    assert_eq!(wall.bbox.map(|x| x[0]), Some(2.0), "theirs' move is lost");
    assert_eq!(
        doc.handseed,
        Some(0x901),
        "$HANDSEED isn't above ours' new handle"
    );
    let base_doc = k2_mpc_dxf::read_doc(&b.bytes, &Limits::default()).unwrap();
    assert_eq!(
        doc.entities.len(),
        base_doc.entities.len(),
        "one added, one deleted"
    );
}

#[test]
pub fn merge_keeps_theirs_style() {
    let b = testgen::sampler(Ver::R2018).build();
    let mut t = testgen::sampler(Ver::R2018);
    t.opts.eol = testgen::Eol::CrLf;
    t.opts.pad = true;
    // theirs: the person's app wrote the file in its own style, and moved the lamp
    let theirs = splice_move(&t.build().bytes, b.handle("lamp"), 2.0);
    // ours: the wall moved and given a colour (a pair added)
    let ours = splice_insert(
        &splice_move(&b.bytes, b.handle("wall"), 1.0),
        b.handle("wall"),
        8,
        62,
        "3",
    );
    let want = splice_insert(
        &splice_move(&theirs, b.handle("wall"), 1.0),
        b.handle("wall"),
        8,
        62,
        "3",
    );
    assert!(
        merged(&b.bytes, &ours, &theirs) == want,
        "ours' changes aren't in theirs' style"
    );
}

#[test]
pub fn merge_without_handles_unsupported() {
    let mut s = testgen::sampler(Ver::R12);
    s.opts.handles = false;
    let b = s.build().bytes;
    match k2_mpc_dxf::merge::merge(&b, &b, &b) {
        MergeOutcome::Unsupported(why) => assert!(why.contains("without a handle"), "{why}"),
        other => panic!(
            "merged without handles: {}",
            matches!(other, MergeOutcome::Merged { .. })
        ),
    }
}

#[test]
pub fn merge_through_the_pack() {
    let b = testgen::sampler(Ver::R2018).build();
    assert!(DxfPack.can_merge());
    let ours = splice_move(&b.bytes, b.handle("wall"), 1.0);
    let theirs = splice_move(&b.bytes, b.handle("lamp"), 2.0);
    let want = splice_move(&theirs, b.handle("wall"), 1.0);
    assert!(DxfPack.merge3_bytes(&b.bytes, &ours, &theirs) == BlobMerge::Merged(want));
    let theirs = splice_move(&b.bytes, b.handle("wall"), 2.0);
    assert!(DxfPack.merge3_bytes(&b.bytes, &ours, &theirs) == BlobMerge::Conflict);
}
