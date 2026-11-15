//! The merge rows: two saves of one deck, by shape. Ours is the doc's version (an agent's
//! edits), theirs a person's app save. Every row says what lands, what is held, and what is a
//! whole-file conflict.

mod common;

use common::{entries, limits};
use k2_mpc_base::session::{BlobMerge, BlobPack};
use k2_mpc_pptx::merge::{merge3, ConflictWhy, Merged, Outcome, Take};
use k2_mpc_pptx::slide::{read_shapes, ShapeInfo};
use k2_mpc_pptx::testgen::{self, DeckSpec, Shape, Slide};
use k2_mpc_pptx::xmlx::parse_xml;
use k2_mpc_pptx::{Pptx, PptxPack, Since};

fn merged(b: &[u8], o: &[u8], t: &[u8]) -> Merged {
    match merge3(b, o, t, &limits()) {
        Outcome::Merged(m) => {
            Pptx::open(&m.bytes, &limits()).expect("the merge opens");
            m
        }
        other => panic!("not merged: {other:?}"),
    }
}

fn conflict(b: &[u8], o: &[u8], t: &[u8]) -> String {
    match merge3(b, o, t, &limits()) {
        Outcome::Conflict(why) => why,
        Outcome::Merged(_) => panic!("merged, not a conflict"),
        Outcome::Unsupported(d) => panic!("unsupported: {d}"),
    }
}

fn find(v: &[ShapeInfo], id: u32) -> Option<ShapeInfo> {
    v.iter().find_map(|s| {
        if s.id == Some(id) {
            Some(s.clone())
        } else {
            find(&s.children, id)
        }
    })
}

/// A shape of a slide, as read back.
fn shape(bytes: &[u8], sld: u32, id: u32) -> Option<ShapeInfo> {
    let p = Pptx::open(bytes, &limits()).expect("opens");
    let s = p.deck.slide(sld)?;
    let t = p.text(&s.part).expect("text");
    let doc = parse_xml(t, &limits()).expect("parses");
    find(&read_shapes(&doc), id)
}

fn shape_ids(bytes: &[u8], sld: u32) -> Vec<u32> {
    let p = Pptx::open(bytes, &limits()).expect("opens");
    let s = p.deck.slide(sld).expect("slide");
    let doc = parse_xml(p.text(&s.part).unwrap(), &limits()).unwrap();
    read_shapes(&doc).iter().filter_map(|s| s.id).collect()
}

fn slide_xml(bytes: &[u8], sld: u32) -> String {
    let p = Pptx::open(bytes, &limits()).expect("opens");
    let s = p.deck.slide(sld).expect("slide");
    p.text(&s.part).unwrap().to_owned()
}

fn text(bytes: &[u8], sld: u32, id: u32) -> Option<String> {
    shape(bytes, sld, id).and_then(|s| s.text)
}

fn notes(bytes: &[u8], sld: u32) -> String {
    let p = Pptx::open(bytes, &limits()).expect("opens");
    let n = p
        .deck
        .slide(sld)
        .and_then(|s| s.notes.clone())
        .expect("notes");
    let doc = parse_xml(p.text(&n).unwrap(), &limits()).unwrap();
    k2_mpc_pptx::slide::placeholder_text(&read_shapes(&doc), &["body"]).unwrap_or_default()
}

fn ids(bytes: &[u8]) -> Vec<u32> {
    Pptx::open(bytes, &limits())
        .expect("opens")
        .deck
        .slides
        .iter()
        .map(|s| s.id)
        .collect()
}

fn base() -> DeckSpec {
    testgen::sample()
}

fn edit_shape(spec: &mut DeckSpec, sld: u32, id: u32, f: impl FnOnce(&mut Shape)) {
    fn walk(v: &mut [Shape], id: u32) -> Option<&mut Shape> {
        for s in v.iter_mut() {
            if s.id() == id {
                return Some(s);
            }
            if let Shape::Group { children, .. } = s {
                if let Some(x) = walk(children, id) {
                    return Some(x);
                }
            }
        }
        None
    }
    let s = spec.slides.iter_mut().find(|s| s.id == sld).expect("slide");
    f(walk(&mut s.shapes, id).expect("shape"));
}

fn set_text(spec: &mut DeckSpec, sld: u32, id: u32, t: &str) {
    edit_shape(spec, sld, id, |s| match s {
        Shape::Title { text, .. } | Shape::Body { text, .. } | Shape::TextBox { text, .. } => {
            *text = t.to_owned()
        }
        _ => panic!("no text"),
    });
}

fn move_to(spec: &mut DeckSpec, sld: u32, id: u32, x: i64) {
    edit_shape(spec, sld, id, |s| match s {
        Shape::TextBox { at, .. }
        | Shape::Rect { at, .. }
        | Shape::Picture { at, .. }
        | Shape::Table { at, .. }
        | Shape::Group { at, .. }
        | Shape::Connector { at, .. } => at.0 = x,
        _ => panic!("no position"),
    });
}

fn remove_shape(spec: &mut DeckSpec, sld: u32, id: u32) {
    let s = spec.slides.iter_mut().find(|s| s.id == sld).expect("slide");
    s.shapes.retain(|x| x.id() != id);
}

fn d(spec: &DeckSpec) -> Vec<u8> {
    testgen::deck(spec)
}

#[test]
fn both_add_a_slide_and_both_land() {
    let b = base();
    let mut o = b.clone();
    o.slides
        .push(Slide::titled(260, "Agent slide", "from the agent"));
    let mut t = b.clone();
    t.slides
        .push(Slide::titled(260, "Person slide", "from the person"));
    let (bb, ob, tb) = (d(&b), d(&o), d(&t));
    let m = merged(&bb, &ob, &tb);
    assert_eq!(ids(&m.bytes), [256, 257, 258, 259, 261, 260]);
    assert_eq!(text(&m.bytes, 260, 2).as_deref(), Some("Person slide"));
    assert_eq!(text(&m.bytes, 261, 2).as_deref(), Some("Agent slide"));
    assert_eq!(m.slides_reminted, [(260, 261)]);
    assert!(
        m.report
            .renamed
            .iter()
            .any(|(a, b)| a == "ppt/slides/slide5.xml" && b == "ppt/slides/slide6.xml"),
        "{:?}",
        m.report
    );
    assert!(m.conflicts.is_empty());
    // the person's slide keeps its part, its id and its bytes
    let p = Pptx::open(&m.bytes, &limits()).unwrap();
    assert_eq!(p.deck.slide(260).unwrap().part, "ppt/slides/slide5.xml");
    assert_eq!(slide_xml(&m.bytes, 260), slide_xml(&tb, 260));
    // through the pack, told the kit wrote nothing since the last outside save, the same;
    // through the kit's call, which can't say so, both are kept
    let pack = PptxPack::default();
    assert_eq!(
        pack.merge3_since(&bb, &ob, &tb, Since::NoKitWrite),
        BlobMerge::Merged(m.bytes.clone())
    );
    assert_eq!(pack.merge3_bytes(&bb, &ob, &tb), BlobMerge::Conflict);
}

#[test]
fn different_shapes_on_one_slide_both_land_and_the_rest_is_untouched() {
    let b = base();
    let mut o = b.clone();
    set_text(&mut o, 258, 4, "Agent text");
    let mut t = b.clone();
    move_to(&mut t, 258, 5, 6_000_000);
    let (bb, ob, tb) = (d(&b), d(&o), d(&t));
    let m = merged(&bb, &ob, &tb);
    assert_eq!(text(&m.bytes, 258, 4).as_deref(), Some("Agent text"));
    assert_eq!(shape(&m.bytes, 258, 5).unwrap().xfrm.unwrap().x, 6_000_000);
    assert!(m.conflicts.is_empty() && m.shapes_reminted.is_empty());
    assert_eq!(shape_ids(&m.bytes, 258), [2, 4, 5, 6, 7, 8, 9]);
    // every other part is theirs' content
    let theirs = entries(&tb);
    let got = entries(&m.bytes);
    assert_eq!(got.len(), theirs.len());
    let mut same = 0;
    for ((gn, gc), (tn, tc)) in got.iter().zip(theirs.iter()) {
        assert_eq!(gn, tn, "laid out as theirs");
        if gn != "ppt/slides/slide3.xml" {
            assert_eq!(gc, tc, "{gn} untouched");
            same += 1;
        }
    }
    assert!(same > 20);
    // the slide part: theirs' frame and shapes, with ours' one shape spliced in
    let x = slide_xml(&m.bytes, 258);
    let ox = slide_xml(&ob, 258);
    let start = ox.find("<p:sp><p:nvSpPr><p:cNvPr id=\"4\"").unwrap();
    let end = start + ox[start..].find("</p:sp>").unwrap() + 7;
    assert!(
        x.contains(&ox[start..end]),
        "ours' shape bytes as they were"
    );
}

#[test]
fn both_edit_one_shape_keeps_the_persons_and_holds_the_agents() {
    let b = base();
    let mut o = b.clone();
    set_text(&mut o, 258, 4, "Agent words");
    let mut t = b.clone();
    set_text(&mut t, 258, 4, "Person words");
    move_to(&mut t, 258, 5, 7_000_000);
    let (bb, ob, tb) = (d(&b), d(&o), d(&t));
    let m = merged(&bb, &ob, &tb);
    assert_eq!(text(&m.bytes, 258, 4).as_deref(), Some("Person words"));
    assert_eq!(m.conflicts.len(), 1);
    let c = &m.conflicts[0];
    assert_eq!(c.address, "@sld:258/sp:4");
    assert_eq!(c.why, ConflictWhy::BothChanged);
    assert!(c.agent.as_deref().unwrap().contains("Agent words"));
    assert!(c.person.as_deref().unwrap().contains("Person words"));
    assert!(c.base.as_deref().unwrap().contains("Left box"));
    assert_eq!(c.id.len(), 12);

    // through the pack, even told the save's base, a clash is a whole-file conflict and
    // nothing is held in memory, until the kit's durable held conflicts land
    let pack = PptxPack::default();
    assert_eq!(
        pack.merge3_since(&bb, &ob, &tb, Since::NoKitWrite),
        BlobMerge::Conflict
    );
    assert!(pack.conflicts.list().is_empty());
    // the held path, kept for then: taking the agent's writes it, only over the person's
    let bytes = m.bytes.clone();
    let hold = |pack: &PptxPack| pack.conflicts.hold(m.conflicts.clone());
    hold(&pack);
    let held = pack.conflicts.list();
    assert_eq!(held.len(), 1);
    // held once, however often the same clash is held
    hold(&pack);
    assert_eq!(pack.conflicts.list().len(), 1);
    let taken = pack
        .conflicts
        .take(&held[0].id, Take::Agent, &bytes, &limits())
        .expect("taken")
        .expect("bytes");
    assert_eq!(text(&taken, 258, 4).as_deref(), Some("Agent words"));
    assert_eq!(shape(&taken, 258, 5).unwrap().xfrm.unwrap().x, 7_000_000);
    assert!(pack.conflicts.list().is_empty());
    // the person's side: nothing to write
    hold(&pack);
    let id = pack.conflicts.list()[0].id.clone();
    // stale: the file moved on since
    let mut t2 = t.clone();
    set_text(&mut t2, 258, 4, "Person again");
    assert!(pack
        .conflicts
        .take(&id, Take::Agent, &d(&t2), &limits())
        .is_err());
    assert_eq!(
        pack.conflicts.take(&id, Take::Person, &bytes, &limits()),
        Ok(None)
    );
    assert!(pack.conflicts.list().is_empty());
}

#[test]
fn a_delete_against_an_edit_keeps_the_persons_side() {
    // the person deleted the shape the agent moved
    let b = base();
    let mut o = b.clone();
    move_to(&mut o, 258, 7, 100);
    let mut t = b.clone();
    remove_shape(&mut t, 258, 7);
    let (bb, ob, tb) = (d(&b), d(&o), d(&t));
    let m = merged(&bb, &ob, &tb);
    assert!(shape(&m.bytes, 258, 7).is_none());
    assert_eq!(m.conflicts.len(), 1);
    assert_eq!(m.conflicts[0].why, ConflictWhy::PersonDeleted);
    assert!(m.conflicts[0].person.is_none());
    let pack = PptxPack::default();
    pack.conflicts.hold(m.conflicts.clone());
    let back = pack
        .conflicts
        .take(&m.conflicts[0].id, Take::Agent, &m.bytes, &limits())
        .unwrap()
        .unwrap();
    assert_eq!(shape(&back, 258, 7).unwrap().xfrm.unwrap().x, 100);

    // the agent deleted the title the person retitled
    let mut o = b.clone();
    remove_shape(&mut o, 257, 2);
    let mut t = b.clone();
    set_text(&mut t, 257, 2, "Better goals");
    let (ob, tb) = (d(&o), d(&t));
    let m = merged(&bb, &ob, &tb);
    assert_eq!(text(&m.bytes, 257, 2).as_deref(), Some("Better goals"));
    assert_eq!(m.conflicts.len(), 1);
    assert_eq!(m.conflicts[0].why, ConflictWhy::AgentDeleted);
    assert!(m.conflicts[0].agent.is_none());

    // a delete on one side, nothing on the other: deleted
    let mut o = b.clone();
    remove_shape(&mut o, 258, 8);
    let mut t = b.clone();
    set_text(&mut t, 258, 2, "Retitled");
    let m = merged(&bb, &d(&o), &d(&t));
    assert!(shape(&m.bytes, 258, 8).is_none());
    assert_eq!(text(&m.bytes, 258, 2).as_deref(), Some("Retitled"));
    assert!(m.conflicts.is_empty());
}

#[test]
fn both_add_a_shape_under_one_id_and_ours_is_reminted() {
    let b = base();
    let mut o = b.clone();
    let s = o.slides.iter_mut().find(|s| s.id == 258).unwrap();
    s.shapes
        .push(Shape::text_box(12, (1, 2, 3, 4), "agent box"));
    s.shapes.push(Shape::Connector {
        id: 13,
        at: (5, 6, 7, 8),
        from: 12,
        to: 2,
    });
    let mut t = b.clone();
    let s = t.slides.iter_mut().find(|s| s.id == 258).unwrap();
    s.shapes
        .push(Shape::text_box(12, (9, 9, 9, 9), "person box"));
    let (bb, ob, tb) = (d(&b), d(&o), d(&t));
    let m = merged(&bb, &ob, &tb);
    assert_eq!(text(&m.bytes, 258, 12).as_deref(), Some("person box"));
    assert_eq!(text(&m.bytes, 258, 14).as_deref(), Some("agent box"));
    assert_eq!(m.shapes_reminted, [("@sld:258".to_owned(), 12, 14)]);
    let x = slide_xml(&m.bytes, 258);
    assert!(
        x.contains("<a:stCxn id=\"14\" idx=\"3\"/>"),
        "ours' connector follows its shape"
    );
    assert_eq!(shape_ids(&m.bytes, 258), [2, 4, 5, 6, 7, 8, 9, 14, 13, 12]);
    // the same shape added on both sides lands once
    let m = merged(&bb, &tb, &tb);
    assert_eq!(m.bytes, tb);
    let mut t2 = t.clone();
    move_to(&mut t2, 258, 4, 1);
    let mut o2 = t.clone();
    set_text(&mut o2, 258, 2, "Ours titled");
    let m = merged(&bb, &d(&o2), &d(&t2));
    assert_eq!(shape_ids(&m.bytes, 258), [2, 4, 5, 6, 7, 8, 9, 12]);
}

#[test]
fn a_reorder_on_one_side_and_an_edit_on_the_other() {
    let b = base();
    let mut o = b.clone();
    o.slides.swap(0, 1);
    let mut t = b.clone();
    set_text(&mut t, 256, 3, "New subtitle");
    let m = merged(&d(&b), &d(&o), &d(&t));
    assert_eq!(ids(&m.bytes), [257, 256, 258, 259]);
    assert_eq!(text(&m.bytes, 256, 3).as_deref(), Some("New subtitle"));
    // the person reorders, the agent adds: the agent's slide after its neighbour
    let mut o = b.clone();
    o.slides.insert(1, Slide::titled(260, "Inserted", "x"));
    let mut t = b.clone();
    t.slides.reverse();
    let m = merged(&d(&b), &d(&o), &d(&t));
    assert_eq!(ids(&m.bytes), [259, 258, 257, 256, 260]);
    // z-order: the agent brings a shape to the front, the person edits another
    let mut o = b.clone();
    let s = o.slides.iter_mut().find(|s| s.id == 258).unwrap();
    let r = s.shapes.remove(2);
    s.shapes.push(r);
    let mut t = b.clone();
    set_text(&mut t, 258, 4, "person");
    let m = merged(&d(&b), &d(&o), &d(&t));
    assert_eq!(shape_ids(&m.bytes, 258), [2, 4, 6, 7, 8, 9, 5]);
    assert_eq!(text(&m.bytes, 258, 4).as_deref(), Some("person"));
}

#[test]
fn whole_file_conflicts_when_nothing_can_be_placed_for_sure() {
    let b = base();
    let bb = d(&b);
    // both reorder slides, differently
    let mut o = b.clone();
    o.slides.swap(0, 1);
    let mut t = b.clone();
    t.slides.swap(2, 3);
    assert!(conflict(&bb, &d(&o), &d(&t)).contains("reordered"));
    // both reorder one slide's shapes, differently
    let mut o = b.clone();
    o.slides[2].shapes.swap(1, 2);
    let mut t = b.clone();
    t.slides[2].shapes.swap(3, 4);
    assert!(conflict(&bb, &d(&o), &d(&t)).contains("reordered"));
    // the person deleted a slide the agent edited
    let mut o = b.clone();
    set_text(&mut o, 259, 2, "agent");
    let mut t = b.clone();
    t.slides.retain(|s| s.id != 259);
    conflict(&bb, &d(&o), &d(&t));
    // both changed one slide's timing
    let mut o = b.clone();
    o.slides[1].animate = Some(2);
    let mut t = b.clone();
    t.slides[1].animate = Some(3);
    assert!(conflict(&bb, &d(&o), &d(&t)).contains("own settings"));
    // a section list, and both sides changed the slides
    let mut sb = b.clone();
    sb.sections = vec![("All".into(), vec![256, 257, 258, 259])];
    let mut so = sb.clone();
    so.slides.push(Slide::titled(260, "a", "b"));
    so.sections[0].1.push(260);
    let mut st = sb.clone();
    st.slides.push(Slide::titled(261, "c", "d"));
    st.sections[0].1.push(261);
    conflict(&d(&sb), &d(&so), &d(&st));
    // a master both changed
    let mut o = b.clone();
    set_text(&mut o, 256, 2, "x");
    let parts_o = testgen::parts(&o);
    let parts_t = testgen::parts(&b);
    let bump = |p: &[(String, Vec<u8>)], v: &str| {
        let m = "ppt/slideMasters/slideMaster1.xml";
        let x = String::from_utf8(p.iter().find(|(n, _)| n == m).unwrap().1.clone()).unwrap();
        testgen::with_part(p, m, x.replace(">Title<", &format!(">{v}<")).into_bytes())
    };
    let o2 = testgen::stored_zip(&bump(&parts_o, "Agent title"));
    let t2 = testgen::stored_zip(&bump(&parts_t, "Person title"));
    assert!(conflict(&bb, &o2, &t2).contains("slidemaster1"));
}

#[test]
fn a_part_that_holds_another_slide_is_a_conflict() {
    // the person's app renumbered: slide1.xml now holds slide 257 and slide2.xml slide 256
    let b = base();
    let parts = testgen::parts(&b);
    let pres = "ppt/presentation.xml";
    let x = String::from_utf8(parts.iter().find(|(n, _)| n == pres).unwrap().1.clone()).unwrap();
    let swapped = x
        .replace("id=\"256\"", "id=\"X\"")
        .replace("id=\"257\"", "id=\"256\"")
        .replace("id=\"X\"", "id=\"257\"");
    let t = testgen::stored_zip(&testgen::with_part(&parts, pres, swapped.into_bytes()));
    let mut o = b.clone();
    set_text(&mut o, 258, 4, "agent");
    assert!(conflict(&d(&b), &d(&o), &t).contains("renumbered"));
}

#[test]
fn notes_merge_by_shape_too() {
    let b = base();
    let mut o = b.clone();
    o.slides[1].notes = Some("Agent notes".into());
    let mut t = b.clone();
    t.slides[1].notes = Some("Person notes".into());
    set_text(&mut t, 257, 2, "Goals, revised");
    let m = merged(&d(&b), &d(&o), &d(&t));
    assert_eq!(notes(&m.bytes, 257), "Person notes");
    assert_eq!(text(&m.bytes, 257, 2).as_deref(), Some("Goals, revised"));
    assert_eq!(m.conflicts.len(), 1);
    assert_eq!(m.conflicts[0].address, "@notes:257/sp:3");
    // notes on one side, a shape on the other
    let mut t = b.clone();
    set_text(&mut t, 257, 2, "Goals, revised");
    let m = merged(&d(&b), &d(&o), &d(&t));
    assert_eq!(notes(&m.bytes, 257), "Agent notes");
    assert!(m.conflicts.is_empty());
}

#[test]
fn an_add_against_a_delete_and_metadata_both_changed() {
    let b = base();
    let mut o = b.clone();
    o.slides.push(Slide::titled(260, "Added", "by the agent"));
    let mut t = b.clone();
    t.slides.retain(|s| s.id != 259);
    let m = merged(&d(&b), &d(&o), &d(&t));
    assert_eq!(ids(&m.bytes), [256, 257, 258, 260]);
    // docProps/app.xml changed on both sides (the slide count): the person's
    assert_eq!(
        common::entry(&m.bytes, "docProps/app.xml"),
        common::entry(&d(&t), "docProps/app.xml")
    );
}

#[test]
fn identities_and_the_stale_buffer_guard() {
    let b = d(&base());
    let mut o = base();
    set_text(&mut o, 256, 2, "agent");
    let o = d(&o);
    let mut t = base();
    set_text(&mut t, 257, 2, "person");
    let t = d(&t);
    assert_eq!(merged(&b, &o, &b).bytes, o);
    assert_eq!(merged(&b, &b, &t).bytes, t);
    assert_eq!(merged(&b, &t, &t).bytes, t);
    // the kit's base equal to ours says nothing about what the app had open: keep both
    let pack = PptxPack::default();
    assert_eq!(pack.merge3_bytes(&b, &b, &t), BlobMerge::Conflict);
    assert!(pack.can_merge());
    // a file this pack doesn't hold: unsupported once the merge is tried; through the kit's
    // call the stale guard answers first, and the kit keeps both either way
    assert_eq!(
        pack.merge3_since(&b, &o, b"not a deck", Since::NoKitWrite),
        BlobMerge::Unsupported
    );
    assert_eq!(
        pack.merge3_bytes(&b, &o, b"not a deck"),
        BlobMerge::Conflict
    );
}

/// An app's save over a version the kit may have written keeps both. The kit wrote `v1` (an
/// agent's change W1) to the file; an agent's W2 is pending (the head is `v2`; the file and
/// the kit's base are `v1`); an app that read the file before `v1` saves its buffer. A merge
/// from `v1` reads that save as the person undoing W1 and drops it with no conflict. The pack
/// can't tell this save from one made on `v1`, so it keeps both.
#[test]
fn a_stale_save_after_a_kit_write_keeps_both() {
    let v0 = base();
    let mut v1 = v0.clone();
    set_text(&mut v1, 256, 2, "W1 by the agent");
    let mut v2 = v1.clone();
    set_text(&mut v2, 258, 4, "W2 by the agent");
    let mut save = v0.clone();
    move_to(&mut save, 258, 5, 6_000_000);
    let (b1, b2, s) = (d(&v1), d(&v2), d(&save));
    // the hazard: the bare merge from what the file last held loses W1, silently
    let m = merged(&b1, &b2, &s);
    assert!(m.conflicts.is_empty());
    assert_ne!(text(&m.bytes, 256, 2).as_deref(), Some("W1 by the agent"));
    assert_eq!(text(&m.bytes, 258, 4).as_deref(), Some("W2 by the agent"));
    // through the kit's call: keep both (the head, with W1 and W2, becomes a version and a
    // conflict copy)
    let pack = PptxPack::default();
    assert_eq!(pack.merge3_bytes(&b1, &b2, &s), BlobMerge::Conflict);
    assert!(pack.conflicts.list().is_empty());
    // the same save made on `v1` is kept both too: the pack can't tell it from the stale one
    let mut fresh = v1.clone();
    move_to(&mut fresh, 258, 5, 6_000_000);
    let f = d(&fresh);
    assert_eq!(pack.merge3_bytes(&b1, &b2, &f), BlobMerge::Conflict);
    // a caller that knows the kit wrote nothing since the last outside save gets the merge:
    // W1, W2 and the person's move all land
    let BlobMerge::Merged(m) = pack.merge3_since(&b1, &b2, &f, Since::NoKitWrite) else {
        panic!("merged")
    };
    assert_eq!(text(&m, 256, 2).as_deref(), Some("W1 by the agent"));
    assert_eq!(text(&m, 258, 4).as_deref(), Some("W2 by the agent"));
    assert_eq!(shape(&m, 258, 5).unwrap().xfrm.unwrap().x, 6_000_000);
    // nothing changed on one side: no history can lose anything, so the kit's call passes
    assert_eq!(
        pack.merge3_bytes(&b1, &b2, &b1),
        BlobMerge::Merged(b2.clone())
    );
    assert_eq!(
        pack.merge3_bytes(&b1, &b2, &b2),
        BlobMerge::Merged(b2.clone())
    );
}

/// Until the kit's durable held conflicts land, a clash (both sides changed one
/// shape, or one deleted what the other changed) is a whole-file conflict through the pack,
/// even when the save's base is known: an agent's side held in pack memory would be lost on a
/// restart, with its clash record and its `conflict take`. The kit keeps both instead.
#[test]
fn a_clash_through_the_pack_is_a_whole_file_conflict() {
    let b = base();
    let bb = d(&b);
    // both changed shape 4
    let mut o = b.clone();
    set_text(&mut o, 258, 4, "Agent words");
    let mut t = b.clone();
    set_text(&mut t, 258, 4, "Person words");
    let both = (d(&o), d(&t));
    // the person deleted the shape the agent moved
    let mut o = b.clone();
    move_to(&mut o, 258, 7, 100);
    let mut t = b.clone();
    remove_shape(&mut t, 258, 7);
    let person_deleted = (d(&o), d(&t));
    // the agent deleted the title the person retitled
    let mut o = b.clone();
    remove_shape(&mut o, 257, 2);
    let mut t = b.clone();
    set_text(&mut t, 257, 2, "Better goals");
    let agent_deleted = (d(&o), d(&t));
    for (what, (ob, tb), why) in [
        ("both changed", both, ConflictWhy::BothChanged),
        ("person deleted", person_deleted, ConflictWhy::PersonDeleted),
        ("agent deleted", agent_deleted, ConflictWhy::AgentDeleted),
    ] {
        // the bare merge holds the agent's side ...
        let m = merged(&bb, &ob, &tb);
        assert_eq!(m.conflicts.len(), 1, "{what}");
        assert_eq!(m.conflicts[0].why, why, "{what}");
        // ... the pack keeps both, holding nothing in memory
        let pack = PptxPack::default();
        assert_eq!(
            pack.merge3_since(&bb, &ob, &tb, Since::NoKitWrite),
            BlobMerge::Conflict,
            "{what}"
        );
        assert!(pack.conflicts.list().is_empty(), "{what}");
        assert_eq!(pack.merge3_bytes(&bb, &ob, &tb), BlobMerge::Conflict);
    }
    // no clash: still merged
    let mut o = b.clone();
    set_text(&mut o, 258, 4, "Agent text");
    let mut t = b.clone();
    move_to(&mut t, 258, 5, 6_000_000);
    let (ob, tb) = (d(&o), d(&t));
    let pack = PptxPack::default();
    assert_eq!(
        pack.merge3_since(&bb, &ob, &tb, Since::NoKitWrite),
        BlobMerge::Merged(merged(&bb, &ob, &tb).bytes)
    );
}
