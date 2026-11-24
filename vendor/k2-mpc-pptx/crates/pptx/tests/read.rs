//! The read rows: slides in presentation order by `sldId`, shapes per slide (id, kind, name,
//! position and size, text), layouts and masters, notes; the structure index; and the round
//! trips (every generated deck, stored and deflated, writes back to its own bytes).

mod common;

use std::collections::BTreeSet;

use common::{deflated, limits, specs};
use k2_mpc_base::index::{Digest, IndexLimits};
use k2_mpc_base::session::BlobPack;
use k2_mpc_pptx::index::draft;
use k2_mpc_pptx::slide::{read_shapes, Tree};
use k2_mpc_pptx::testgen::{self, Shape, Slide, LAYOUT_CONTENT, LAYOUT_TITLE, MASTER_ID, PNG};
use k2_mpc_pptx::xmlx::parse_xml;
use k2_mpc_pptx::{Pptx, PptxPack};

fn ilimits() -> IndexLimits {
    IndexLimits::new(100_000, 64, 64 << 20, 64 << 10)
}

#[test]
fn the_deck_reads_in_presentation_order() {
    let bytes = testgen::deck(&testgen::sample());
    let p = Pptx::open(&bytes, &limits()).expect("opens");
    let ids: Vec<u32> = p.deck.slides.iter().map(|s| s.id).collect();
    assert_eq!(ids, [256, 257, 258, 259]);
    assert_eq!(p.main, "ppt/presentation.xml");
    assert_eq!(p.deck.slides[0].part, "ppt/slides/slide1.xml");
    assert_eq!(p.deck.slides[0].rid, "rId10");
    assert!(p.deck.slides[3].hidden && !p.deck.slides[2].hidden);
    assert_eq!(
        p.deck.slides[0].layout.as_deref(),
        Some("ppt/slidelayouts/slidelayout1.xml")
    );
    assert_eq!(
        p.deck.slides[1].layout.as_deref(),
        Some("ppt/slidelayouts/slidelayout2.xml")
    );
    assert_eq!(
        p.deck.slides[1].notes.as_deref(),
        Some("ppt/notesslides/notesslide2.xml")
    );
    assert_eq!(p.deck.slides[0].notes, None);
    assert_eq!(p.deck.size, Some((12_192_000, 6_858_000)));
    assert!(!p.deck.sections);
    assert_eq!(p.deck.masters.len(), 1);
    let m = &p.deck.masters[0];
    assert_eq!(m.id, Some(MASTER_ID));
    let names: Vec<(Option<u64>, &str, Option<&str>)> = m
        .layouts
        .iter()
        .map(|l| (l.id, l.name.as_str(), l.kind.as_deref()))
        .collect();
    assert_eq!(
        names,
        [
            (Some(LAYOUT_TITLE), "Title Slide", Some("title")),
            (Some(LAYOUT_CONTENT), "Title and Content", Some("obj"))
        ]
    );
    assert_eq!(
        p.deck.notes_master.as_deref(),
        Some("ppt/notesmasters/notesmaster1.xml")
    );
}

#[test]
fn a_reordered_list_reads_in_its_order() {
    let mut spec = testgen::sample();
    spec.slides.swap(0, 2);
    let bytes = testgen::deck(&spec);
    let p = Pptx::open(&bytes, &limits()).expect("opens");
    let ids: Vec<u32> = p.deck.slides.iter().map(|s| s.id).collect();
    assert_eq!(ids, [258, 257, 256, 259]);
}

#[test]
fn shapes_read_with_their_ids_kinds_bounds_and_text() {
    let bytes = testgen::deck(&testgen::sample());
    let p = Pptx::open(&bytes, &limits()).expect("opens");
    let text = p.text(&p.deck.slides[2].part).expect("text");
    let doc = parse_xml(text, &limits()).expect("parses");
    let shapes = read_shapes(&doc);
    let got: Vec<(Option<u32>, &str)> = shapes.iter().map(|s| (s.id, s.kind)).collect();
    assert_eq!(
        got,
        [
            (Some(2), "placeholder"),
            (Some(4), "textbox"),
            (Some(5), "shape"),
            (Some(6), "connector"),
            (Some(7), "picture"),
            (Some(8), "table"),
            (Some(9), "group"),
        ]
    );
    assert_eq!(shapes[0].text.as_deref(), Some("Everything"));
    assert_eq!(
        shapes[0].placeholder.as_ref().unwrap().kind.as_deref(),
        Some("title")
    );
    assert_eq!(shapes[1].name, "TextBox 3");
    assert_eq!(shapes[1].text.as_deref(), Some("Left box"));
    let x = shapes[1].xfrm.expect("xfrm");
    assert_eq!((x.x, x.y, x.cx, x.cy), (838200, 1500000, 3000000, 500000));
    assert_eq!(shapes[2].text, None);
    assert_eq!(shapes[4].descr.as_deref(), Some("A small square"));
    assert_eq!(shapes[4].embed.as_deref(), Some("rId2"));
    assert_eq!(shapes[5].text.as_deref(), Some("Site\nCount\nNorth\n12"));
    let x = shapes[5].xfrm.expect("a table's p:xfrm");
    assert_eq!((x.x, x.cx), (2500000, 4000000));
    let kids: Vec<(Option<u32>, &str)> =
        shapes[6].children.iter().map(|s| (s.id, s.kind)).collect();
    assert_eq!(kids, [(Some(10), "textbox"), (Some(11), "shape")]);
    assert_eq!(shapes[6].xfrm.expect("group xfrm").x, 7500000);

    let tree = Tree::parse(&doc).expect("splits");
    let ids: Vec<Option<u32>> = tree.units.iter().map(|u| u.id).collect();
    assert_eq!(ids, [2, 4, 5, 6, 7, 8, 9].map(Some));
    assert_eq!(tree.units[6].ids, [9, 10, 11]);
    assert_eq!(tree.frame_ids, [1]);
    assert!(tree.ids_unique());
    assert_eq!(tree.max_id(), 11);
    // the split puts the part back together
    let mut back = text[..tree.units_start].to_owned();
    for u in &tree.units {
        back.push_str(&text[u.lead.clone()]);
        back.push_str(&text[u.range.clone()]);
    }
    back.push_str(&text[tree.units_end..]);
    assert_eq!(back, text);
}

#[test]
fn every_tree_splits_back_to_its_bytes() {
    let mut parts = 0;
    for (name, spec) in specs() {
        let bytes = testgen::deck(&spec);
        let p = Pptx::open(&bytes, &limits()).unwrap_or_else(|e| panic!("{name}: {e}"));
        for (k, _) in p.opc.parts.iter().filter(|(k, _)| {
            k.ends_with(".xml")
                && (k.contains("/slides/")
                    || k.contains("notesslides/")
                    || k.contains("slidelayouts/")
                    || k.contains("slidemasters/")
                    || k.contains("notesmasters/"))
        }) {
            let text = p.text(k).expect("text");
            let doc = parse_xml(text, &limits()).expect("parses");
            let tree = Tree::parse(&doc).unwrap_or_else(|e| panic!("{name} {k}: {e}"));
            let mut back = text[..tree.units_start].to_owned();
            for u in &tree.units {
                back.push_str(&text[u.lead.clone()]);
                back.push_str(&text[u.range.clone()]);
            }
            back.push_str(&text[tree.units_end..]);
            assert_eq!(back, text, "{name} {k}");
            parts += 1;
        }
    }
    println!("trees split back: {parts} parts");
    assert!(parts > 50);
}

#[test]
fn round_trips_are_byte_exact() {
    let l = limits();
    let mut n = 0;
    for (name, spec) in specs() {
        let parts = testgen::parts(&spec);
        for (layout, bytes) in [
            ("stored", testgen::stored_zip(&parts)),
            ("deflated", deflated(&parts)),
        ] {
            let p = Pptx::open(&bytes, &l).unwrap_or_else(|e| panic!("{name} {layout}: {e}"));
            assert_eq!(
                p.encode().expect("encode"),
                bytes,
                "{name} {layout}: encode"
            );
            let none = std::collections::BTreeMap::new();
            assert_eq!(
                p.opc.write(&none).expect("write"),
                bytes,
                "{name} {layout}: write"
            );
            // a second reader agrees on every entry
            let theirs = common::entries(&bytes);
            assert_eq!(theirs.len(), p.opc.parts.len(), "{name} {layout}");
            for (en, c) in theirs {
                let k = en.to_ascii_lowercase();
                assert_eq!(p.opc.parts[&k].content, c, "{name} {layout}: {en}");
            }
            n += 1;
        }
    }
    println!("round trips: {n} decks byte-exact");
}

#[test]
fn the_index_holds_slides_shapes_notes_and_layouts() {
    let bytes = testgen::deck(&testgen::sample());
    let p = Pptx::open(&bytes, &limits()).expect("opens");
    let d = draft(&p, &ilimits(), &limits()).expect("indexes");
    assert_eq!(d.source, "pptx-1");
    assert_eq!(d.attrs["slides"], "4");
    assert_eq!(d.attrs["width"], "12192000");
    let keys: Vec<(String, String, Option<u32>)> = d
        .nodes
        .iter()
        .map(|n| (n.key.clone().unwrap_or_default(), n.kind.clone(), n.parent))
        .collect();
    let slide = |id: u32| {
        keys.iter()
            .position(|k| k.0 == format!("@sld:{id}"))
            .expect("slide node") as u32
    };
    // slides in order, each before its shapes; masters and layouts after
    let order: Vec<&str> = keys
        .iter()
        .filter(|k| k.1 == "slide")
        .map(|k| k.0.as_str())
        .collect();
    assert_eq!(order, ["@sld:256", "@sld:257", "@sld:258", "@sld:259"]);
    let s258 = slide(258);
    let on_258: Vec<&str> = keys
        .iter()
        .filter(|k| k.2 == Some(s258))
        .map(|k| k.0.as_str())
        .collect();
    // front to back
    assert_eq!(
        on_258,
        [
            "@sld:258/sp:9",
            "@sld:258/sp:8",
            "@sld:258/sp:7",
            "@sld:258/sp:6",
            "@sld:258/sp:5",
            "@sld:258/sp:4",
            "@sld:258/sp:2"
        ]
    );
    let group = keys.iter().position(|k| k.0 == "@sld:258/sp:9").unwrap() as u32;
    let in_group: Vec<&str> = keys
        .iter()
        .filter(|k| k.2 == Some(group))
        .map(|k| k.0.as_str())
        .collect();
    assert_eq!(in_group, ["@sld:258/sp:11", "@sld:258/sp:10"]);
    let node = |key: &str| {
        d.nodes
            .iter()
            .find(|n| n.key.as_deref() == Some(key))
            .expect(key)
    };
    let tb = node("@sld:258/sp:4");
    assert_eq!(tb.kind, "textbox");
    assert_eq!(tb.text.as_deref(), Some("Left box"));
    let b = tb.bounds.expect("bounds");
    assert_eq!(
        (b.left, b.top, b.right, b.bottom),
        (838200, 1500000, 3838200, 2000000)
    );
    assert_eq!(tb.attrs["z"], "1");
    let pic = node("@sld:258/sp:7");
    assert_eq!(pic.content, Some(Digest::of(PNG)));
    assert_eq!(pic.attrs["descr"], "A small square");
    let title = node("@sld:256");
    assert_eq!(title.name, "Quarterly review");
    assert_eq!(title.weak["creation_id"], "1234567");
    assert_eq!(title.attrs["layout"], format!("@layout:{LAYOUT_TITLE}"));
    assert_eq!(title.attrs["layout_name"], "Title Slide");
    assert!(!node("@sld:259").visible);
    let notes = node("@notes:257");
    assert_eq!(notes.parent, Some(slide(257)));
    assert_eq!(
        notes.text.as_deref(),
        Some("Mention the pilot sites.\nAsk for questions.")
    );
    assert_eq!(node("@sld:257/sp:3").attrs["placeholder"], "obj");
    let m = keys
        .iter()
        .position(|k| k.0 == format!("@master:{MASTER_ID}"))
        .unwrap() as u32;
    let layouts: Vec<&str> = keys
        .iter()
        .filter(|k| k.2 == Some(m))
        .map(|k| k.0.as_str())
        .collect();
    assert_eq!(
        layouts,
        [
            format!("@layout:{LAYOUT_TITLE}"),
            format!("@layout:{LAYOUT_CONTENT}")
        ]
    );
    // parents always come first
    for (i, n) in d.nodes.iter().enumerate() {
        if let Some(p) = n.parent {
            assert!((p as usize) < i);
        }
    }
}

#[test]
fn the_pack_indexes_through_the_kit_interface() {
    let bytes = testgen::deck(&testgen::sample());
    let pack = PptxPack::default();
    let spec = pack.index_spec().expect("an index");
    assert_eq!(
        (spec.pack, spec.format, spec.older_format, spec.own_records),
        ("pptx", 2, Some(1), true),
        "format 2 (the view items) reads format 1's rows; own records declared (the kit stores \
         them under its offset format)"
    );
    let d = pack
        .index(&mut bytes.as_slice(), bytes.len() as u64, &ilimits())
        .expect("indexes");
    assert!(d.nodes.len() > 20);
    // the kit's registry takes the verb names
    k2_mpc_base::parity::Registry::for_blob_pack(&pack).expect("registry");
    // a cap is a refusal, never a panic
    let small = IndexLimits::new(5, 64, 64 << 20, 64 << 10);
    assert!(pack
        .index(&mut bytes.as_slice(), bytes.len() as u64, &small)
        .is_err());
    let text_cap = IndexLimits::new(100_000, 64, 64 << 20, 4);
    let d = pack
        .index(&mut bytes.as_slice(), bytes.len() as u64, &text_cap)
        .expect("indexes");
    assert!(d.nodes.iter().any(|n| n.flags.contains("text_truncated")));
}

#[test]
fn repeated_shape_ids_are_left_to_the_kit_to_mint() {
    let mut s = Slide::titled(256, "Twice", "");
    s.shapes.push(Shape::text_box(5, (0, 0, 10, 10), "one"));
    s.shapes.push(Shape::text_box(5, (0, 0, 10, 10), "two"));
    let bytes = testgen::deck(&testgen::DeckSpec::new(vec![s]));
    let p = Pptx::open(&bytes, &limits()).expect("opens");
    let text = p.text(&p.deck.slides[0].part).unwrap();
    let doc = parse_xml(text, &limits()).unwrap();
    assert!(!Tree::parse(&doc).unwrap().ids_unique());
    let d = draft(&p, &ilimits(), &limits()).expect("indexes");
    let twice: BTreeSet<usize> = d
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.key.as_deref() == Some("@sld:256/sp:5"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(twice.len(), 2);
}

/// A slide's and a group's own record digest (index format 3): unchanged by an edit inside one
/// of their shapes (the record changes), changed by their shapes' order; a shape with no
/// children has none.
#[test]
fn a_container_s_own_record_leaves_out_its_shapes_but_not_their_order() {
    use k2_mpc_pptx::verbs::{apply, Edit};
    let pack = PptxPack::default();
    let read = |b: &[u8]| {
        let d = pack
            .index(&mut &b[..], b.len() as u64, &ilimits())
            .expect("indexes");
        let get = |k: &str| {
            d.nodes
                .iter()
                .find(|n| n.key.as_deref() == Some(k))
                .map(|n| (n.record, n.own))
                .unwrap_or_else(|| panic!("no node {k}"))
        };
        (get("@sld:258"), get("@sld:258/sp:9"), get("@sld:258/sp:4"))
    };
    let base = testgen::deck(&testgen::sample());
    let (slide, group, shape) = read(&base);
    assert!(
        slide.1.is_some() && group.1.is_some() && shape.1.is_none(),
        "a slide and a group have an own digest, a plain shape none"
    );
    let moved = apply(
        &base,
        &Edit::MoveShape {
            slide: 258,
            shape: 4,
            x: Some(100),
            y: None,
            w: None,
            h: None,
        },
        &limits(),
    )
    .expect("moves")
    .bytes;
    let (s2, g2, _) = read(&moved);
    let in_group = apply(
        &base,
        &Edit::SetText {
            slide: 258,
            shape: 10,
            text: "Grouped".into(),
        },
        &limits(),
    )
    .expect("sets the text")
    .bytes;
    let (s3, g3, _) = read(&in_group);
    let mut spec = testgen::sample();
    let mix = spec
        .slides
        .iter_mut()
        .find(|x| x.id == 258)
        .expect("slide 258");
    mix.shapes.swap(4, 5);
    let (s4, g4, _) = read(&testgen::deck(&spec));
    assert_eq!(
        (
            (s2.0 != slide.0, s2.1 == slide.1, g2 == group),
            (
                s3.0 != slide.0,
                s3.1 == slide.1,
                g3.0 != group.0,
                g3.1 == group.1
            ),
            (s4.1 != slide.1, g4.1 == group.1)
        ),
        ((true, true, true), (true, true, true, true), (true, true)),
        "own digests: a shape's move changes the slide's record, not its own; a text edit in the \
         group changes the group's and slide's records, not their own; the shapes' order changes \
         the slide's own"
    );
}
