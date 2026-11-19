//! The verb rows: each agent verb does its one thing, splices only what it changes (every other
//! part byte-exact), keeps ids, and refuses what it can't do safely.

mod common;

use std::collections::BTreeSet;

use common::{entries, limits};
use k2_mpc_base::index::IndexLimits;
use k2_mpc_pptx::blob_verbs::VERB_SET;
use k2_mpc_pptx::index::draft;
use k2_mpc_pptx::slide::{read_shapes, ShapeInfo};
use k2_mpc_pptx::testgen::{self, DeckSpec, LAYOUT_CONTENT, LAYOUT_TITLE};
use k2_mpc_pptx::verbs::{
    apply, list, parse_address, registry, render, Edit, LayoutSel, Place, VerbError, ROWS, VERBS,
};
use k2_mpc_pptx::xmlx::parse_xml;
use k2_mpc_pptx::Pptx;

fn sample() -> Vec<u8> {
    testgen::deck(&testgen::sample())
}

fn run(bytes: &[u8], e: Edit) -> (Vec<u8>, Option<String>) {
    let a = apply(bytes, &e, &limits()).unwrap_or_else(|err| panic!("{e:?}: {err}"));
    let p = Pptx::open(&a.bytes, &limits()).expect("the result opens");
    draft(
        &p,
        &IndexLimits::new(100_000, 64, 64 << 20, 64 << 10),
        &limits(),
    )
    .expect("indexes");
    (a.bytes, a.address)
}

fn err(bytes: &[u8], e: Edit) -> VerbError {
    match apply(bytes, &e, &limits()) {
        Err(x) => x,
        Ok(_) => panic!("{e:?} applied"),
    }
}

/// The names of the entries whose content changed, and those added or removed.
fn changed(a: &[u8], b: &[u8]) -> BTreeSet<String> {
    let (ea, eb) = (entries(a), entries(b));
    let mut out = BTreeSet::new();
    for (n, c) in &eb {
        match ea.iter().find(|(m, _)| m == n) {
            Some((_, d)) if d == c => {}
            _ => {
                out.insert(n.clone());
            }
        }
    }
    for (n, _) in &ea {
        if !eb.iter().any(|(m, _)| m == n) {
            out.insert(format!("-{n}"));
        }
    }
    out
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn ids(bytes: &[u8]) -> Vec<u32> {
    Pptx::open(bytes, &limits())
        .unwrap()
        .deck
        .slides
        .iter()
        .map(|s| s.id)
        .collect()
}

fn shapes(bytes: &[u8], sld: u32) -> Vec<ShapeInfo> {
    let p = Pptx::open(bytes, &limits()).unwrap();
    let s = p.deck.slide(sld).unwrap();
    read_shapes(&parse_xml(p.text(&s.part).unwrap(), &limits()).unwrap())
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

fn shape(bytes: &[u8], sld: u32, id: u32) -> Option<ShapeInfo> {
    find(&shapes(bytes, sld), id)
}

fn slide_xml(bytes: &[u8], sld: u32) -> String {
    let p = Pptx::open(bytes, &limits()).unwrap();
    p.text(&p.deck.slide(sld).unwrap().part).unwrap().to_owned()
}

#[test]
fn the_verbs_are_registry_data() {
    // the kit's registry for a deck that isn't live: every deck verb on the blob path, and not
    // the clash verbs (they stay file verbs until the kit holds conflicts on tier 0)
    let r = registry().expect("the kit takes them");
    let missing: Vec<&str> = VERB_SET
        .iter()
        .map(|v| v.name)
        .filter(|n| r.get(n).is_none())
        .collect();
    let clash: Vec<&str> = ["conflicts list", "conflict take"]
        .into_iter()
        .filter(|n| r.get(n).is_some())
        .collect();
    assert_eq!(
        (missing, clash),
        (Vec::<&str>::new(), Vec::<&str>::new()),
        "the_verbs_are_registry_data: registry() is the kit's blob registry for the deck"
    );
    for v in VERBS {
        assert!(v.serves.iter().all(|s| ROWS.contains(s)), "{}", v.name);
    }
    for row in ROWS {
        assert!(
            VERBS.iter().any(|v| v.serves.contains(row)),
            "{row} has a verb"
        );
    }
    assert_eq!(parse_address("@sld:256"), Ok((256, None)));
    assert_eq!(parse_address("@sld:256/sp:4"), Ok((256, Some(4))));
    for bad in ["sld:256", "@sld:x", "@sp:4", "@sld:256/sp:4/sp:5", "#2"] {
        assert!(parse_address(bad).is_err(), "{bad}");
    }
}

#[test]
fn slide_list_reads_slides_and_shapes() {
    let l = list(&sample(), &limits()).unwrap();
    let got: Vec<(&str, usize, Option<&str>, &str, bool)> = l
        .iter()
        .map(|s| {
            (
                s.address.as_str(),
                s.position,
                s.layout.as_deref(),
                s.title.as_str(),
                s.hidden,
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (
                "@sld:256",
                1,
                Some("Title Slide"),
                "Quarterly review",
                false
            ),
            ("@sld:257", 2, Some("Title and Content"), "Goals", false),
            (
                "@sld:258",
                3,
                Some("Title and Content"),
                "Everything",
                false
            ),
            ("@sld:259", 4, Some("Title and Content"), "Backup", true),
        ]
    );
    assert_eq!(
        l[1].notes.as_deref(),
        Some("Mention the pilot sites.\nAsk for questions.")
    );
    assert_eq!(l[2].shapes.len(), 9);
    let g = &l[2].shapes[7];
    assert_eq!(
        (g.address.as_deref(), g.kind, g.depth),
        (Some("@sld:258/sp:10"), "textbox", 1)
    );
    let text = render(&l, true);
    assert!(text.contains("3\t@sld:258\tTitle and Content\tEverything\n"));
    assert!(text.contains("  @sld:258/sp:4\ttextbox\tTextBox 3\tLeft box\n"));
    assert!(text.contains("\t(hidden)\n"));
}

#[test]
fn slide_add_makes_a_slide_from_its_layout() {
    let b = sample();
    let (o, addr) = run(
        &b,
        Edit::AddSlide {
            layout: LayoutSel::Name("Title and Content".into()),
            place: None,
        },
    );
    assert_eq!(addr.as_deref(), Some("@sld:260"));
    assert_eq!(ids(&o), [256, 257, 258, 259, 260]);
    let s = shapes(&o, 260);
    let got: Vec<(Option<u32>, &str, Option<&str>)> = s
        .iter()
        .map(|x| {
            (
                x.id,
                x.kind,
                x.placeholder.as_ref().and_then(|p| p.kind.as_deref()),
            )
        })
        .collect();
    // the layout's title and content, not its date, footer or slide number
    assert_eq!(
        got,
        [
            (Some(2), "placeholder", Some("title")),
            (Some(3), "placeholder", None)
        ]
    );
    assert_eq!(
        changed(&b, &o),
        set(&[
            "[Content_Types].xml",
            "ppt/presentation.xml",
            "ppt/_rels/presentation.xml.rels",
            "ppt/slides/slide5.xml",
            "ppt/slides/_rels/slide5.xml.rels",
        ])
    );
    let p = Pptx::open(&o, &limits()).unwrap();
    assert_eq!(
        p.deck.slide(260).unwrap().layout.as_deref(),
        Some("ppt/slidelayouts/slidelayout2.xml")
    );
    // the presentation is spliced: one entry more, every other byte the same
    let pres = |x: &[u8]| common::text(x, "ppt/presentation.xml");
    assert_eq!(
        pres(&o),
        pres(&b).replace(
            "<p:sldId id=\"259\" r:id=\"rId13\"/>",
            "<p:sldId id=\"259\" r:id=\"rId13\"/><p:sldId id=\"260\" r:id=\"rId14\"/>"
        )
    );
    // by id, first, after
    let (o, _) = run(
        &b,
        Edit::AddSlide {
            layout: LayoutSel::Id(LAYOUT_TITLE),
            place: Some(Place::First),
        },
    );
    assert_eq!(ids(&o), [260, 256, 257, 258, 259]);
    assert_eq!(
        shapes(&o, 260)[0]
            .placeholder
            .as_ref()
            .unwrap()
            .kind
            .as_deref(),
        Some("ctrTitle")
    );
    let (o, _) = run(
        &b,
        Edit::AddSlide {
            layout: LayoutSel::Id(LAYOUT_CONTENT),
            place: Some(Place::After(256)),
        },
    );
    assert_eq!(ids(&o), [256, 260, 257, 258, 259]);
    // twice: the next id and the next part
    let (o2, a2) = run(
        &o,
        Edit::AddSlide {
            layout: LayoutSel::Id(LAYOUT_CONTENT),
            place: None,
        },
    );
    assert_eq!(a2.as_deref(), Some("@sld:261"));
    assert!(Pptx::open(&o2, &limits())
        .unwrap()
        .deck
        .slide(261)
        .unwrap()
        .part
        .ends_with("slide6.xml"));
    // refusals
    assert!(matches!(
        err(
            &b,
            Edit::AddSlide {
                layout: LayoutSel::Name("Nope".into()),
                place: None
            }
        ),
        VerbError::NotFound(_)
    ));
    assert_eq!(
        err(
            &b,
            Edit::AddSlide {
                layout: LayoutSel::Id(LAYOUT_TITLE),
                place: Some(Place::After(999))
            }
        )
        .exit(),
        4
    );
    let parts = testgen::parts(&testgen::sample());
    let l1 = "ppt/slideLayouts/slideLayout1.xml";
    let x = String::from_utf8(parts.iter().find(|(n, _)| n == l1).unwrap().1.clone()).unwrap();
    let same = testgen::stored_zip(&testgen::with_part(
        &parts,
        l1,
        x.replace("name=\"Title Slide\"", "name=\"Title and Content\"")
            .into_bytes(),
    ));
    let e = err(
        &same,
        Edit::AddSlide {
            layout: LayoutSel::Name("Title and Content".into()),
            place: None,
        },
    );
    assert!(
        matches!(e, VerbError::Usage(_)) && e.to_string().contains("@layout:"),
        "{e}"
    );
}

#[test]
fn slide_add_keeps_sections_whole() {
    let mut spec = testgen::sample();
    spec.sections = vec![
        ("Opening".into(), vec![256, 257]),
        ("Detail".into(), vec![258, 259]),
    ];
    let b = testgen::deck(&spec);
    let (o, _) = run(
        &b,
        Edit::AddSlide {
            layout: LayoutSel::Id(LAYOUT_CONTENT),
            place: Some(Place::After(257)),
        },
    );
    let pres = common::text(&o, "ppt/presentation.xml");
    assert!(pres.contains("<p14:sldId id=\"257\"/><p14:sldId id=\"260\"/></p14:sldIdLst></p14:section><p14:section name=\"Detail\""));
    let (o, _) = run(
        &b,
        Edit::AddSlide {
            layout: LayoutSel::Id(LAYOUT_CONTENT),
            place: None,
        },
    );
    assert!(common::text(&o, "ppt/presentation.xml")
        .contains("<p14:sldId id=\"259\"/><p14:sldId id=\"260\"/>"));
    // moving between sections is refused
    assert!(matches!(
        err(
            &b,
            Edit::MoveSlide {
                slide: 259,
                place: Place::First
            }
        ),
        VerbError::Refused(_)
    ));
}

#[test]
fn slide_move_reorders_the_list_only() {
    let b = sample();
    let (o, _) = run(
        &b,
        Edit::MoveSlide {
            slide: 259,
            place: Place::After(256),
        },
    );
    assert_eq!(ids(&o), [256, 259, 257, 258]);
    assert_eq!(changed(&b, &o), set(&["ppt/presentation.xml"]));
    let (o, _) = run(
        &b,
        Edit::MoveSlide {
            slide: 258,
            place: Place::First,
        },
    );
    assert_eq!(ids(&o), [258, 256, 257, 259]);
    let (o, _) = run(
        &b,
        Edit::MoveSlide {
            slide: 256,
            place: Place::After(259),
        },
    );
    assert_eq!(ids(&o), [257, 258, 259, 256]);
    // already there: the same bytes
    assert_eq!(
        run(
            &b,
            Edit::MoveSlide {
                slide: 257,
                place: Place::After(256)
            }
        )
        .0,
        b
    );
    assert!(matches!(
        err(
            &b,
            Edit::MoveSlide {
                slide: 257,
                place: Place::After(257)
            }
        ),
        VerbError::Usage(_)
    ));
    assert_eq!(
        err(
            &b,
            Edit::MoveSlide {
                slide: 300,
                place: Place::First
            }
        )
        .exit(),
        4
    );
    // a pretty-printed list stays readable
    let mut spec = testgen::sample();
    spec.pretty = true;
    let pb = testgen::deck(&spec);
    let (po, _) = run(
        &pb,
        Edit::MoveSlide {
            slide: 259,
            place: Place::First,
        },
    );
    assert_eq!(ids(&po), [259, 256, 257, 258]);
}

#[test]
fn shape_add_puts_a_text_box_in_front() {
    let b = sample();
    let (o, addr) = run(
        &b,
        Edit::AddTextBox {
            slide: 257,
            at: (100, 200, 3000, 4000),
            text: "One & <two>\nthree".into(),
        },
    );
    assert_eq!(addr.as_deref(), Some("@sld:257/sp:4"));
    let s = shapes(&o, 257);
    let last = s.last().unwrap();
    assert_eq!(
        (last.id, last.kind, last.name.as_str()),
        (Some(4), "textbox", "TextBox 3")
    );
    assert_eq!(last.text.as_deref(), Some("One & <two>\nthree"));
    let x = last.xfrm.unwrap();
    assert_eq!((x.x, x.y, x.cx, x.cy), (100, 200, 3000, 4000));
    assert_eq!(changed(&b, &o), set(&["ppt/slides/slide2.xml"]));
    // the slide's other bytes are as they were
    let (bx, ox) = (slide_xml(&b, 257), slide_xml(&o, 257));
    let cut = bx.find("</p:spTree>").unwrap();
    assert!(ox.starts_with(&bx[..cut]) && ox.ends_with(&bx[cut..]));
    // the next id is past every id on the slide, groups included
    let (o, a) = run(
        &b,
        Edit::AddTextBox {
            slide: 258,
            at: (0, 0, 1, 1),
            text: String::new(),
        },
    );
    assert_eq!(a.as_deref(), Some("@sld:258/sp:12"));
    assert_eq!(shape(&o, 258, 12).unwrap().text.as_deref(), Some(""));
    assert!(matches!(
        err(
            &b,
            Edit::AddTextBox {
                slide: 257,
                at: (0, 0, -1, 1),
                text: "x".into()
            }
        ),
        VerbError::Usage(_)
    ));
    assert!(matches!(
        err(
            &b,
            Edit::AddTextBox {
                slide: 257,
                at: (0, 0, 1, 1),
                text: "bell\u{7}".into()
            }
        ),
        VerbError::Usage(_)
    ));
}

#[test]
fn shape_text_replaces_the_body_and_keeps_its_formatting() {
    let b = sample();
    let (o, _) = run(
        &b,
        Edit::SetText {
            slide: 257,
            shape: 3,
            text: "First\n\nThird".into(),
        },
    );
    assert_eq!(
        shape(&o, 257, 3).unwrap().text.as_deref(),
        Some("First\n\nThird")
    );
    let x = slide_xml(&o, 257);
    assert!(x.contains("<a:r><a:rPr lang=\"en-US\" dirty=\"0\"/><a:t>First</a:t></a:r>"));
    assert!(x.contains("<p:txBody><a:bodyPr/><a:lstStyle/><a:p>"));
    assert_eq!(changed(&b, &o), set(&["ppt/slides/slide2.xml"]));
    // only that shape's paragraphs changed
    let bx = slide_xml(&b, 257);
    let title_end = bx.find("</p:sp>").unwrap();
    assert_eq!(&x[..title_end], &bx[..title_end]);
    // a shape in a group; a shape with no text body gets one
    let (o, _) = run(
        &b,
        Edit::SetText {
            slide: 258,
            shape: 10,
            text: "grouped".into(),
        },
    );
    assert_eq!(shape(&o, 258, 10).unwrap().text.as_deref(), Some("grouped"));
    let (o, _) = run(
        &b,
        Edit::SetText {
            slide: 258,
            shape: 5,
            text: "now with text".into(),
        },
    );
    assert_eq!(
        shape(&o, 258, 5).unwrap().text.as_deref(),
        Some("now with text")
    );
    // not text shapes
    for id in [7, 8, 9, 6] {
        assert!(
            matches!(
                err(
                    &b,
                    Edit::SetText {
                        slide: 258,
                        shape: id,
                        text: "x".into()
                    }
                ),
                VerbError::Refused(_)
            ),
            "{id}"
        );
    }
    assert_eq!(
        err(
            &b,
            Edit::SetText {
                slide: 258,
                shape: 99,
                text: "x".into()
            }
        )
        .exit(),
        4
    );
}

#[test]
fn shape_move_edits_only_the_transform() {
    let b = sample();
    let (o, _) = run(
        &b,
        Edit::MoveShape {
            slide: 258,
            shape: 4,
            x: Some(1),
            y: None,
            w: Some(2),
            h: None,
        },
    );
    let x = shape(&o, 258, 4).unwrap().xfrm.unwrap();
    assert_eq!((x.x, x.y, x.cx, x.cy), (1, 1500000, 2, 500000));
    assert_eq!(
        slide_xml(&o, 258).len(),
        slide_xml(&b, 258).len() - "838200".len() + 1 - "3000000".len() + 1
    );
    // a table's frame, a group, a shape in a group
    for id in [8, 9, 10, 7, 6] {
        let (o, _) = run(
            &b,
            Edit::MoveShape {
                slide: 258,
                shape: id,
                x: None,
                y: Some(42),
                w: None,
                h: None,
            },
        );
        assert_eq!(shape(&o, 258, id).unwrap().xfrm.unwrap().y, 42, "{id}");
    }
    // a placeholder placed by its layout: all four, or nothing
    assert!(matches!(
        err(
            &b,
            Edit::MoveShape {
                slide: 257,
                shape: 2,
                x: Some(1),
                y: None,
                w: None,
                h: None
            }
        ),
        VerbError::Usage(_)
    ));
    let (o, _) = run(
        &b,
        Edit::MoveShape {
            slide: 257,
            shape: 2,
            x: Some(1),
            y: Some(2),
            w: Some(3),
            h: Some(4),
        },
    );
    let x = shape(&o, 257, 2).unwrap().xfrm.unwrap();
    assert_eq!((x.x, x.y, x.cx, x.cy), (1, 2, 3, 4));
    assert!(matches!(
        err(
            &b,
            Edit::MoveShape {
                slide: 258,
                shape: 4,
                x: None,
                y: None,
                w: None,
                h: None
            }
        ),
        VerbError::Usage(_)
    ));
}

#[test]
fn shape_delete_removes_one_shape_and_refuses_what_points_at_it() {
    let b = sample();
    let (o, _) = run(
        &b,
        Edit::DeleteShape {
            slide: 258,
            shape: 8,
        },
    );
    assert!(shape(&o, 258, 8).is_none());
    assert_eq!(shapes(&o, 258).len(), 6);
    assert_eq!(changed(&b, &o), set(&["ppt/slides/slide3.xml"]));
    // animated (4), connected (5), inside a group (10), missing
    for id in [4, 5, 10] {
        assert!(
            matches!(
                err(
                    &b,
                    Edit::DeleteShape {
                        slide: 258,
                        shape: id
                    }
                ),
                VerbError::Refused(_)
            ),
            "{id}"
        );
    }
    assert_eq!(
        err(
            &b,
            Edit::DeleteShape {
                slide: 258,
                shape: 99
            }
        )
        .exit(),
        4
    );
    // the connector itself can go, and then its ends
    let (o, _) = run(
        &b,
        Edit::DeleteShape {
            slide: 258,
            shape: 6,
        },
    );
    let (o, _) = run(
        &o,
        Edit::DeleteShape {
            slide: 258,
            shape: 5,
        },
    );
    assert!(shape(&o, 258, 5).is_none());
}

#[test]
fn notes_set_edits_or_makes_the_notes_slide() {
    let b = sample();
    let (o, _) = run(
        &b,
        Edit::SetNotes {
            slide: 257,
            text: "Fresh notes".into(),
        },
    );
    let l = list(&o, &limits()).unwrap();
    assert_eq!(l[1].notes.as_deref(), Some("Fresh notes"));
    assert_eq!(changed(&b, &o), set(&["ppt/notesSlides/notesSlide2.xml"]));
    // slide 256 has none: made from the notes master
    let (o, _) = run(
        &b,
        Edit::SetNotes {
            slide: 256,
            text: "New\nnotes".into(),
        },
    );
    let l = list(&o, &limits()).unwrap();
    assert_eq!(l[0].notes.as_deref(), Some("New\nnotes"));
    assert_eq!(
        changed(&b, &o),
        set(&[
            "[Content_Types].xml",
            "ppt/slides/_rels/slide1.xml.rels",
            "ppt/notesSlides/notesSlide1.xml",
            "ppt/notesSlides/_rels/notesSlide1.xml.rels",
        ])
    );
    // and again, now that it has one
    let (o2, _) = run(
        &o,
        Edit::SetNotes {
            slide: 256,
            text: "Again".into(),
        },
    );
    assert_eq!(
        list(&o2, &limits()).unwrap()[0].notes.as_deref(),
        Some("Again")
    );
    // no notes master: refused
    let mut spec: DeckSpec = testgen::sample();
    spec.notes_master = false;
    let nm = testgen::deck(&spec);
    assert!(matches!(
        err(
            &nm,
            Edit::SetNotes {
                slide: 256,
                text: "x".into()
            }
        ),
        VerbError::Refused(_)
    ));
}

#[test]
fn verbs_never_touch_other_parts_in_any_layout() {
    // every verb on every fixture (stored and deflated): only the parts it names change
    let mut n = 0;
    for (name, spec) in common::specs() {
        let parts = testgen::parts(&spec);
        for bytes in [testgen::stored_zip(&parts), common::deflated(&parts)] {
            let Some(first) = spec.slides.first() else {
                continue;
            };
            let s = first.id;
            let mut edits = vec![
                Edit::AddSlide {
                    layout: LayoutSel::Id(LAYOUT_CONTENT),
                    place: None,
                },
                Edit::AddTextBox {
                    slide: s,
                    at: (1, 2, 3, 4),
                    text: "t".into(),
                },
                Edit::SetText {
                    slide: s,
                    shape: 2,
                    text: "t".into(),
                },
            ];
            if spec.notes_master {
                edits.push(Edit::SetNotes {
                    slide: s,
                    text: "n".into(),
                });
            }
            for e in edits {
                let a =
                    apply(&bytes, &e, &limits()).unwrap_or_else(|x| panic!("{name} {e:?}: {x}"));
                let p = Pptx::open(&a.bytes, &limits()).unwrap();
                let q = Pptx::open(&bytes, &limits()).unwrap();
                // a part the verb didn't change keeps its raw record: same content, same place
                let ch = changed(&bytes, &a.bytes);
                for (k, part) in &q.opc.parts {
                    if !ch.contains(&part.name) {
                        assert_eq!(p.opc.parts[k].content, part.content, "{name} {e:?} {k}");
                        assert_eq!(p.opc.parts[k].index, part.index, "{name} {e:?} {k}");
                    }
                }
                n += 1;
            }
        }
    }
    println!("verbs: {n} edits, untouched parts byte-exact");
}
