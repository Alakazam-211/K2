//! The mutants (`--features mutants`): each plants one bug in the merge or the pack, and the
//! leg written for it must fail under it and pass without it. A mutant no leg catches fails the
//! run, so the merge rows are shown to be able to fail.
#![cfg(feature = "mutants")]

mod common;

use common::{blob_rows, hash_rows, limits, live_rows, splice_rows, view_rows};
use k2_mpc_base::session::{BlobMerge, BlobPack};
use k2_mpc_pptx::merge::{merge3, Outcome};
use k2_mpc_pptx::mutant;
use k2_mpc_pptx::testgen::{self, DeckSpec, Shape, Slide};
use k2_mpc_pptx::verbs::{apply, Edit};
use k2_mpc_pptx::{Pptx, PptxPack, Since};

type Leg = fn() -> Result<(), String>;

fn d(s: &DeckSpec) -> Vec<u8> {
    testgen::deck(s)
}

fn edit(b: &[u8], e: Edit) -> Vec<u8> {
    apply(b, &e, &limits()).expect("applies").bytes
}

fn merged(b: &[u8], o: &[u8], t: &[u8]) -> Result<k2_mpc_pptx::merge::Merged, String> {
    match merge3(b, o, t, &limits()) {
        Outcome::Merged(m) => Ok(m),
        other => Err(format!("{other:?}").chars().take(200).collect()),
    }
}

fn slide_text(bytes: &[u8], sld: u32) -> Result<String, String> {
    let p = Pptx::open(bytes, &limits()).map_err(|e| e.to_string())?;
    let s = p.deck.slide(sld).ok_or("no slide")?;
    Ok(p.text(&s.part).map_err(|e| e.to_string())?.to_owned())
}

/// Two saves that both change shape 4: the agent's is held, the person's in the file.
fn leg_held() -> Result<(), String> {
    let b = d(&testgen::sample());
    let o = edit(
        &b,
        Edit::SetText {
            slide: 258,
            shape: 4,
            text: "agent".into(),
        },
    );
    let t = edit(
        &b,
        Edit::SetText {
            slide: 258,
            shape: 4,
            text: "person".into(),
        },
    );
    let m = merged(&b, &o, &t)?;
    if m.conflicts.len() != 1 || !slide_text(&m.bytes, 258)?.contains(">person<") {
        return Err("the person's isn't kept, or the agent's isn't held".into());
    }
    Ok(())
}

/// Both add a slide: both land, ours re-minted.
fn leg_slides() -> Result<(), String> {
    let b = testgen::sample();
    let mut o = b.clone();
    o.slides.push(Slide::titled(260, "agent", "a"));
    let mut t = b.clone();
    t.slides.push(Slide::titled(260, "person", "p"));
    let m = merged(&d(&b), &d(&o), &d(&t))?;
    let ids: Vec<u32> = Pptx::open(&m.bytes, &limits())
        .map_err(|e| e.to_string())?
        .deck
        .slides
        .iter()
        .map(|s| s.id)
        .collect();
    if ids != [256, 257, 258, 259, 261, 260] {
        return Err(format!("slides {ids:?}"));
    }
    Ok(())
}

/// Both add shape 12, ours with a connector to it: ours' re-minted, the connector following.
fn leg_shapes() -> Result<(), String> {
    let b = testgen::sample();
    let mut o = b.clone();
    o.slides[2]
        .shapes
        .push(Shape::text_box(12, (1, 1, 1, 1), "agent"));
    o.slides[2].shapes.push(Shape::Connector {
        id: 13,
        at: (1, 1, 1, 1),
        from: 12,
        to: 2,
    });
    let mut t = b.clone();
    t.slides[2]
        .shapes
        .push(Shape::text_box(12, (2, 2, 2, 2), "person"));
    let m = merged(&d(&b), &d(&o), &d(&t))?;
    let x = slide_text(&m.bytes, 258)?;
    if !x.contains("<a:stCxn id=\"14\"") || !x.contains("id=\"14\" name=\"TextBox 11\"") {
        return Err("ours' shape or its connector wasn't re-minted".into());
    }
    Ok(())
}

/// The agent reorders slides, the person adds one (both change the slide list): the agent's
/// order stands.
fn leg_order() -> Result<(), String> {
    let b = d(&testgen::sample());
    let o = edit(
        &b,
        Edit::MoveSlide {
            slide: 259,
            place: k2_mpc_pptx::verbs::Place::First,
        },
    );
    let t = edit(
        &b,
        Edit::AddSlide {
            layout: k2_mpc_pptx::verbs::LayoutSel::Name("Title Slide".into()),
            place: None,
        },
    );
    let m = merged(&b, &o, &t)?;
    let ids: Vec<u32> = Pptx::open(&m.bytes, &limits())
        .map_err(|e| e.to_string())?
        .deck
        .slides
        .iter()
        .map(|s| s.id)
        .collect();
    if ids.first() != Some(&259) {
        return Err(format!("order {ids:?}"));
    }
    Ok(())
}

/// A save whose parts were renumbered is a conflict.
fn leg_renumbered() -> Result<(), String> {
    let b = testgen::sample();
    let parts = testgen::parts(&b);
    let pres = "ppt/presentation.xml";
    let x = String::from_utf8(parts.iter().find(|(n, _)| n == pres).unwrap().1.clone()).unwrap();
    let swapped = x
        .replace("id=\"256\"", "id=\"X\"")
        .replace("id=\"257\"", "id=\"256\"")
        .replace("id=\"X\"", "id=\"257\"");
    let t = testgen::stored_zip(&testgen::with_part(&parts, pres, swapped.into_bytes()));
    let o = edit(
        &d(&b),
        Edit::SetText {
            slide: 258,
            shape: 4,
            text: "a".into(),
        },
    );
    match merge3(&d(&b), &o, &t, &limits()) {
        Outcome::Conflict(_) => Ok(()),
        _ => Err("a renumbered save merged".into()),
    }
}

/// Both sides make notes for one slide: a conflict, not two notes slides.
fn leg_two_notes() -> Result<(), String> {
    let b = d(&testgen::sample());
    let o = edit(
        &b,
        Edit::SetNotes {
            slide: 256,
            text: "agent".into(),
        },
    );
    let t = edit(
        &b,
        Edit::SetNotes {
            slide: 256,
            text: "person".into(),
        },
    );
    match merge3(&b, &o, &t, &limits()) {
        Outcome::Conflict(_) => Ok(()),
        _ => Err("two notes slides for one slide merged".into()),
    }
}

/// The kit's base equal to ours: kept as a conflict.
fn leg_stale() -> Result<(), String> {
    let b = d(&testgen::sample());
    let t = edit(
        &b,
        Edit::SetText {
            slide: 257,
            shape: 2,
            text: "x".into(),
        },
    );
    match PptxPack::default().merge3_bytes(&b, &b, &t) {
        BlobMerge::Conflict => Ok(()),
        other => Err(format!("{other:?}").chars().take(60).collect()),
    }
}

/// The kit wrote W1, W2 is pending, and an app saves a buffer from before W1: through the
/// kit's call (which can't say whether the kit wrote since), kept as a conflict.
fn leg_kit_wrote() -> Result<(), String> {
    let v0 = d(&testgen::sample());
    let set = |b: &[u8], slide, shape, text: &str| {
        edit(
            b,
            Edit::SetText {
                slide,
                shape,
                text: text.into(),
            },
        )
    };
    let v1 = set(&v0, 256, 2, "W1");
    let v2 = set(&v1, 258, 4, "W2");
    let save = set(&v0, 257, 2, "person");
    match PptxPack::default().merge3_bytes(&v1, &v2, &save) {
        BlobMerge::Conflict => Ok(()),
        other => Err(format!("{other:?}").chars().take(60).collect()),
    }
}

/// Both change shape 4, the base known: a whole-file conflict, nothing held in memory, until
/// the kit holds conflicts durably.
fn leg_clash_kept_both() -> Result<(), String> {
    let b = d(&testgen::sample());
    let set = |text: &str| {
        edit(
            &b,
            Edit::SetText {
                slide: 258,
                shape: 4,
                text: text.into(),
            },
        )
    };
    let (o, t) = (set("agent"), set("person"));
    let pack = PptxPack::default();
    match pack.merge3_since(&b, &o, &t, Since::NoKitWrite) {
        BlobMerge::Conflict if pack.conflicts.list().is_empty() => Ok(()),
        other => Err(format!("{other:?}").chars().take(60).collect()),
    }
}

const ROWS: &[(&str, Leg)] = &[
    ("pptx_both_changed_ours", leg_held),
    ("pptx_no_remint_slide", leg_slides),
    ("pptx_no_remint_shape", leg_shapes),
    ("pptx_no_ref_rewrite", leg_shapes),
    ("pptx_no_order", leg_order),
    ("pptx_no_part_check", leg_renumbered),
    ("pptx_no_result_check", leg_two_notes),
    ("pack_no_stale_guard", leg_stale),
    ("pack_no_kit_write_guard", leg_kit_wrote),
    ("pack_holds_clashes", leg_clash_kept_both),
    // the live pack: the geometry and render-hash plants, and the
    // live store's, each caught by the row named
    ("GroupRotNotComposed", view_rows::row_index_rev40),
    ("FlipRotAdded", view_rows::row_case_d2),
    ("ApproxNotInherited", view_rows::row_case_d3_counter),
    ("HashMissesSlideNumber", hash_rows::row_turn),
    ("HashMissesSlideNumber", hash_rows::row_v1_slide_added_first),
    ("HashMissesTableStyle", hash_rows::row_v2_table_style),
    ("HashMissesLayout", hash_rows::row_v3_one_layout),
    (
        "HashMissesNothingButOverreaches",
        hash_rows::row_v3_one_layout,
    ),
    (
        "HashMissesNothingButOverreaches",
        hash_rows::row_v6_linked_slide,
    ),
    (
        "EncodeReserialises",
        splice_rows::row_untouched_byte_for_byte,
    ),
    ("EncodeReserialises", splice_rows::row_each_change_spliced),
    ("EncodeReserialises", splice_rows::row_carrier_judge),
    (
        "SpliceReopensPerStep",
        splice_rows::row_one_pass_per_encode_from,
    ),
    ("FidReused", live_rows::row_differential_oracle),
    ("FidReused", live_rows::row_new_shape_after_a_group),
    ("ReadUnbounded", live_rows::row_reads_bounded_and_paged),
    ("ZOrderGlobal", live_rows::row_each_verb),
    // the verbs on the kit's tier-0 path
    (
        "pptx_declares_too_little",
        blob_rows::file_verb_equals_blob_verb,
    ),
];

#[test]
fn every_mutant_is_caught() {
    let mut missed = Vec::new();
    for (name, leg) in ROWS {
        if let Err(e) = leg() {
            panic!("{name}: the leg fails without the mutant: {e}");
        }
        match mutant::with(name, leg) {
            Err(why) => println!("mutant {name}: caught ({why})"),
            Ok(()) => {
                println!("mutant {name}: NOT CAUGHT");
                missed.push(*name);
            }
        }
    }
    println!(
        "{} rows, every mutant caught: {}",
        ROWS.len(),
        missed.is_empty()
    );
    assert!(missed.is_empty(), "not caught: {missed:?}");
}
