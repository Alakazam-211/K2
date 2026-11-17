//! The structure index: what a node is, its key, and which of its fields change when one fact
//! of an entity changes. A node's attributes are its own record's facts, never counts over other
//! records, so two people's edits to different entities change different nodes.

mod support;

use k2_mpc_dxf::testgen::{self, Ent, Spec, Ver};
use k2_mpc_dxf::Limits;
use support::*;

fn entity_nodes(d: &k2_mpc_base::index::IndexDraft) -> usize {
    d.nodes
        .iter()
        .filter(|n| {
            !matches!(
                n.kind.as_str(),
                "drawing" | "layer" | "model" | "layout" | "block"
            )
        })
        .count()
}

#[test]
pub fn every_kind_is_a_node_keyed_by_handle() {
    for v in Ver::ALL {
        let b = testgen::sampler(v).build();
        let d = draft(&b.bytes);
        let root = &d.nodes[0];
        assert_eq!(root.key.as_deref(), Some("@doc:dxf"), "{v:?}");
        assert_eq!(root.kind, "drawing");
        assert_eq!(root.name, v.acadver());
        let wall = node(&d, &b.addr("wall"));
        assert_eq!(wall.kind, "line", "{v:?}");
        assert_eq!(wall.name, "LINE", "{v:?}");
        assert_eq!(attr(wall, "layer"), "A-WALL", "{v:?}");
        // 9 model entities, 1 paper, 2 in DOOR
        assert_eq!(entity_nodes(&d), 12, "{v:?}");
        println!(
            "every_kind_is_a_node_keyed_by_handle: {v:?} {} nodes",
            d.nodes.len()
        );
    }
}

#[test]
pub fn handles_normalised() {
    let mut s = testgen::sampler(Ver::R2018);
    s.opts.lower_handles = true;
    s.opts.padded_handles = true;
    let b = s.build();
    let d = draft(&b.bytes);
    let want = format!("@h:{:X}", b.handle("wall"));
    let n = node(&d, &want);
    assert_eq!(attr(n, "layer"), "A-WALL");
    let mut keyed = 0;
    for n in &d.nodes {
        if let Some(k) = n.key.as_deref().and_then(|k| k.strip_prefix("@h:")) {
            assert!(
                !k.starts_with('0') || k == "0",
                "a key keeps its leading zeros: {k}"
            );
            assert_eq!(k, k.to_ascii_uppercase(), "a key keeps lower case: {k}");
            keyed += 1;
        }
    }
    assert!(keyed > 15, "only {keyed} keyed nodes");
    // the same handle spelled twice is a duplicate, flagged on both
    let b2 = Spec::new(Ver::R2018)
        .model(Ent::line([0.0, 0.0], [1.0, 0.0]).handle(0x500))
        .model(Ent::line([2.0, 0.0], [3.0, 0.0]).handle(0x500))
        .build();
    let d2 = draft(&b2.bytes);
    let dup: Vec<_> = d2
        .nodes
        .iter()
        .filter(|n| n.key.as_deref() == Some("@h:500"))
        .collect();
    assert_eq!(dup.len(), 2);
    assert!(dup.iter().all(|n| n.flags.contains("duplicate_handle")));
}

#[test]
pub fn hmax_from_every_object() {
    let mut s = testgen::sampler(Ver::R2010);
    s.opts.handseed = Some(0x11);
    let b = s.build();
    let m = k2_mpc_dxf::read_doc(&b.bytes, &Limits::default()).unwrap();
    // the dictionaries in OBJECTS are above every entity
    assert_eq!(m.hmax, Some(b.hmax), "hmax");
    assert_eq!(m.handseed, Some(0x11), "$HANDSEED as written");
    let info = run(&b.bytes, "drawing info", &[]);
    assert_eq!(
        string(&info.data, &["handles", "hmax"]),
        format!("{:X}", b.hmax)
    );
    assert_eq!(string(&info.data, &["handles", "handseed"]), "11");
}

#[test]
pub fn no_handles_minted() {
    let mut s = testgen::sampler(Ver::R12);
    s.opts.handles = false;
    let b = s.build();
    let d = draft(&b.bytes);
    let ents: Vec<_> = d
        .nodes
        .iter()
        .filter(|n| n.name == "LINE" || n.name == "CIRCLE" || n.name == "TEXT")
        .collect();
    assert!(ents.len() >= 4, "only {} entities", ents.len());
    for n in &ents {
        assert_eq!(
            n.key, None,
            "{} has a key with no handle in the file",
            n.name
        );
    }
    assert_eq!(d.attrs.get("handles").map(String::as_str), Some("off"));
}

#[test]
pub fn followers_in_parent() {
    let b = testgen::sampler(Ver::R2000).build();
    let d = draft(&b.bytes);
    for n in &d.nodes {
        assert!(
            !matches!(n.name.as_str(), "ATTRIB" | "VERTEX" | "SEQEND"),
            "a follower is a node: {} {:?}",
            n.name,
            n.key
        );
    }
    assert_eq!(attr(node(&d, &b.addr("door1")), "followers"), "1");
    assert_eq!(attr(node(&d, &b.addr("poly")), "followers"), "3");
    // the attribute's value is a field of the insert's own (format 4), not its text
    let door = node(&d, &b.addr("door1"));
    assert_eq!(
        (attr(door, "att.no"), door.text.as_deref()),
        ("D-01", None),
        "followers_in_parent: the attribute is the insert's field att.no"
    );
}

/// attribs_are_fields (index format 4): each attribute of a block reference is a field of the
/// reference's node, `att.<tag>`: the tag in lower case, a byte other than a letter, digit or
/// `_` written `-xx`, a repeated tag's second `.2`; no attribute is the reference's text. A
/// reference whose tags can't each be named (a field name over the kit's 64 bytes) keeps them
/// as its text, as before. The kit takes every draft.
#[test]
pub fn attribs_are_fields() {
    let long = "T".repeat(70);
    let b = Spec::new(Ver::R2018)
        .block("DEV", [0.0, 0.0], vec![Ent::circle([0.0, 0.0], 0.5)])
        .model(
            Ent::insert("DEV", [0.0, 0.0])
                .attrib("DEVICE", "CAM-01")
                .attrib("LABEL", "C1")
                .attrib("DEVICE", "CAM-02")
                .attrib("ROOM.NO", "1-01")
                .label("dev"),
        )
        .model(
            Ent::insert("DEV", [5.0, 0.0])
                .attrib(&long, "x")
                .label("long"),
        )
        .build();
    let d = draft(&b.bytes);
    let dev = node(&d, &b.addr("dev"));
    let att: Vec<(&str, &str)> = dev
        .attrs
        .iter()
        .filter(|(k, _)| k.starts_with("att."))
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        att,
        vec![
            ("att.device", "CAM-01"),
            ("att.device.2", "CAM-02"),
            ("att.label", "C1"),
            ("att.room-2eno", "1-01"),
        ],
        "attribs_are_fields: one field per attribute"
    );
    assert_eq!(
        dev.text, None,
        "attribs_are_fields: no attribute is the text"
    );
    let l = node(&d, &b.addr("long"));
    assert!(
        !l.attrs.keys().any(|k| k.starts_with("att.")),
        "attribs_are_fields: a tag too long to name is no field"
    );
    assert_eq!(
        l.text.as_deref(),
        Some(format!("{long}=x").as_str()),
        "attribs_are_fields: a tag too long to name keeps the text"
    );
    let wire = k2_mpc_base::index::index_to_wire(
        &k2_mpc_dxf::DxfPack,
        &mut &b.bytes[..],
        b.bytes.len() as u64,
        &limits(),
    );
    assert!(
        wire.is_ok(),
        "attribs_are_fields: the kit takes the draft: {:?}",
        wire.err()
    );
    println!("attribs_are_fields: 4 fields, 1 kept as text");
}

/// The drawing of the change rows: two layers, a few entities, no paper space (so an added
/// entity shifts no other handle).
fn base() -> Spec {
    Spec::new(Ver::R2013)
        .layer("A-WALL", 1)
        .layer("E-LITE", 3)
        .block("DOOR", [0.0, 0.0], vec![Ent::line([0.0, 0.0], [1.0, 0.0])])
        .model(
            Ent::line([0.0, 0.0], [10.0, 0.0])
                .on("A-WALL")
                .label("wall"),
        )
        .model(Ent::circle([5.0, 5.0], 1.0).on("E-LITE").label("lamp"))
        .model(Ent::insert("DOOR", [3.0, 0.0]).on("A-WALL").label("door"))
}

#[test]
pub fn attrs_are_own_facts() {
    let a = base().build();
    let b = base()
        .model(
            Ent::line([0.0, 5.0], [10.0, 5.0])
                .on("A-WALL")
                .handle(0x900),
        )
        .model(Ent::insert("DOOR", [6.0, 0.0]).on("A-WALL").handle(0x901))
        .build();
    let (da, db) = (draft(&a.bytes), draft(&b.bytes));
    let mut same = 0;
    for na in &da.nodes {
        let Some(k) = na.key.as_deref() else { continue };
        let nb = node(&db, k);
        assert_eq!(na.attrs, nb.attrs, "{k} ({}) changed attributes", na.name);
        assert_eq!(na.name, nb.name, "{k} renamed");
        assert!(
            na.record == nb.record,
            "{k} ({}) changed its record",
            na.name
        );
        assert!(
            na.content == nb.content,
            "{k} ({}) changed its content",
            na.name
        );
        same += 1;
    }
    assert_eq!(db.nodes.len(), da.nodes.len() + 2, "two entities added");
    println!("attrs_are_own_facts: {same} nodes unchanged by two adds");
}

#[test]
pub fn move_is_not_rename() {
    let a = base().build();
    let mut s = base();
    s.model[0] = Ent::line([1.0, 0.0], [11.0, 0.0])
        .on("A-WALL")
        .label("wall");
    let b = s.build();
    let (da, db) = (draft(&a.bytes), draft(&b.bytes));
    let k = a.addr("wall");
    let (na, nb) = (node(&da, &k), node(&db, &k));
    assert_eq!(na.name, nb.name, "a move renamed the entity");
    assert_ne!(
        attr(na, "geom"),
        attr(nb, "geom"),
        "geom unchanged by a move"
    );
    assert_ne!(
        attr(na, "bbox"),
        attr(nb, "bbox"),
        "bbox unchanged by a move"
    );
    assert_eq!(attr(na, "layer"), attr(nb, "layer"));
    assert!(
        na.content == nb.content,
        "a move changed the content digest"
    );
}

#[test]
pub fn layer_change_is_a_field() {
    let a = base().build();
    let mut s = base();
    s.model[0] = Ent::line([0.0, 0.0], [10.0, 0.0])
        .on("E-LITE")
        .label("wall");
    let b = s.build();
    let (da, db) = (draft(&a.bytes), draft(&b.bytes));
    let k = a.addr("wall");
    let (na, nb) = (node(&da, &k), node(&db, &k));
    assert_eq!(attr(nb, "layer"), "E-LITE");
    assert_ne!(attr(na, "layer_key"), attr(nb, "layer_key"));
    assert_eq!(
        attr(na, "geom"),
        attr(nb, "geom"),
        "a layer change changed geom"
    );
    assert!(
        na.content == nb.content,
        "a layer change changed the content digest"
    );
    // a colour change is its own field too
    let mut s = base();
    s.model[1] = Ent::circle([5.0, 5.0], 1.0)
        .on("E-LITE")
        .color(5)
        .label("lamp");
    let c = s.build();
    let dc = draft(&c.bytes);
    let k = a.addr("lamp");
    assert_eq!(attr(node(&dc, &k), "color"), "5");
    assert_eq!(attr(node(&da, &k), "geom"), attr(node(&dc, &k), "geom"));
    assert!(node(&da, &k).content == node(&dc, &k).content);
}

#[test]
pub fn lastsavedby_not_indexed() {
    let mut s = testgen::sampler(Ver::R2018);
    s.opts.lastsavedby = Some("someone-private".into());
    let b = s.build();
    let d = draft(&b.bytes);
    let needle = "someone-private";
    for (k, v) in &d.attrs {
        assert!(!v.contains(needle), "doc attr {k} holds it");
    }
    for n in &d.nodes {
        assert!(!n.name.contains(needle), "a node name holds it");
        assert!(
            !n.text.as_deref().unwrap_or("").contains(needle),
            "a node text holds it"
        );
        for (k, v) in &n.attrs {
            assert!(!v.contains(needle), "attr {k} of {:?} holds it", n.key);
        }
    }
    let info = run(&b.bytes, "drawing info", &[]);
    assert!(
        !info.data.to_json().contains(needle),
        "drawing info holds it"
    );
    assert!(
        !info.stdout.contains(needle),
        "drawing info's text holds it"
    );
    let h = run(&b.bytes, "header get", &["$LASTSAVEDBY"]);
    assert!(
        h.data.to_json().contains(needle),
        "header get by name reads it"
    );
}

#[test]
pub fn units_in_doc_attrs() {
    let b = testgen::sampler(Ver::R2004).build();
    let d = draft(&b.bytes);
    assert_eq!(d.attrs.get("units").map(String::as_str), Some("mm"));
    assert_eq!(d.attrs.get("insunits").map(String::as_str), Some("4"));
    assert_eq!(d.attrs.get("acadver").map(String::as_str), Some("AC1018"));
    assert_eq!(d.attrs.get("eol").map(String::as_str), Some("lf"));
}

#[test]
pub fn bbox_exact_kinds() {
    let b = Spec::new(Ver::R2010)
        // an arc across the 0 degree line
        .model(Ent::arc([0.0, 0.0], 1.0, 315.0, 45.0).label("arc"))
        // a bulged polyline: a half circle below the chord from (0,0) to (10,0)
        .model(Ent::lwpoly(&[([0.0, 0.0], 1.0), ([10.0, 0.0], 0.0)], false).label("bulge"))
        // a circle in a mirrored frame: x is -x
        .model(
            Ent::circle([5.0, 0.0], 2.0)
                .extrusion([0.0, 0.0, -1.0])
                .label("ocs"),
        )
        .model(Ent::line([1.0, 2.0], [-3.0, 4.0]).label("line"))
        .build();
    let d = draft(&b.bytes);
    let h = std::f64::consts::FRAC_1_SQRT_2;
    let bb = |l: &str| bbox(attr(node(&d, &b.addr(l)), "bbox"));
    same_box(bb("arc"), [h, -h, 1.0, h], "arc across 0 degrees");
    same_box(bb("bulge"), [0.0, -5.0, 10.0, 0.0], "bulged polyline");
    same_box(
        bb("ocs"),
        [-7.0, -2.0, -3.0, 2.0],
        "circle in a mirrored frame",
    );
    same_box(bb("line"), [-3.0, 2.0, 1.0, 4.0], "line");
    let arc = node(&d, &b.addr("arc"));
    let filled = k2_mpc_cad_model::index::with_defaults(&arc.name, &arc.attrs);
    assert_eq!(filled.get("bbox_q").map(String::as_str), Some("exact"));
    assert_eq!(attr(node(&d, &b.addr("ocs")), "ocs"), "1");
}

/// Every node of the compact draft against the full one: the same node, and the same
/// attributes once the defaults are filled back.
fn same_reading(name: &str, bytes: &[u8]) -> usize {
    use k2_mpc_cad_model::index::{draft, draft_full, with_defaults};
    let doc = k2_mpc_dxf::read_doc(bytes, &Limits::default()).unwrap();
    let (d, f) = (
        draft(&doc, &limits()).unwrap(),
        draft_full(&doc, &limits()).unwrap(),
    );
    assert_eq!(d.nodes.len(), f.nodes.len(), "{name}");
    let mut left_out = 0;
    for (c, w) in d.nodes.iter().zip(&f.nodes) {
        assert_eq!(
            (&c.key, &c.name, &c.kind),
            (&w.key, &w.name, &w.kind),
            "{name}"
        );
        assert!(
            c.record == w.record && c.content == w.content,
            "{name}: {:?}",
            c.key
        );
        assert_eq!(
            (&c.bounds, &c.text, &c.flags),
            (&w.bounds, &w.text, &w.flags),
            "{name}"
        );
        assert_eq!(
            with_defaults(&c.name, &c.attrs),
            w.attrs,
            "{name}: {:?} reads back otherwise",
            c.key
        );
        left_out += w.attrs.len() - c.attrs.len();
    }
    left_out
}

#[test]
pub fn compact_attrs_read_back_the_same() {
    use k2_mpc_cad_model::index::{draft, draft_full, with_defaults};
    let mut left_out = 0;
    let mut drawings = 0;
    for v in Ver::ALL {
        for (n, s) in [
            ("sampler", testgen::sampler(v)),
            ("nested", testgen::nested_blocks(v)),
        ] {
            left_out += same_reading(&format!("{n}-{v:?}"), &s.build().bytes);
            drawings += 1;
        }
    }
    for name in testgen::H1_NAMES {
        left_out += same_reading(name, &testgen::h1(name).unwrap().build().bytes);
        drawings += 1;
    }
    // a change reads the same: the fields that differ between two versions, node by node
    let a = base().build();
    let mut s = base();
    s.model[0] = Ent::line([1.0, 0.0], [11.0, 0.0])
        .on("E-LITE")
        .label("wall");
    s.model[1] = Ent::circle([5.0, 5.0], 1.0)
        .on("E-LITE")
        .extrusion([0.0, 0.0, -1.0])
        .label("lamp");
    let b = s.build();
    let changed = |full: bool| {
        let read = |bytes: &[u8]| {
            let doc = k2_mpc_dxf::read_doc(bytes, &Limits::default()).unwrap();
            let d = if full {
                draft_full(&doc, &limits()).unwrap()
            } else {
                draft(&doc, &limits()).unwrap()
            };
            d.nodes
                .iter()
                .map(|n| (n.key.clone(), with_defaults(&n.name, &n.attrs)))
                .collect::<Vec<_>>()
        };
        let (x, y) = (read(&a.bytes), read(&b.bytes));
        let mut out = Vec::new();
        for ((k, p), (_, q)) in x.iter().zip(&y) {
            for f in p.keys().chain(q.keys()) {
                if p.get(f) != q.get(f) {
                    out.push((k.clone(), f.clone()));
                }
            }
        }
        out.sort();
        out.dedup();
        out
    };
    let (compact, full) = (changed(false), changed(true));
    assert!(
        compact.iter().any(|(_, f)| f == "edit"),
        "the move into a frame of its own changes edit: {compact:?}"
    );
    assert_eq!(compact, full, "a change reads differently");
    assert!(left_out > 100, "only {left_out} attributes left out");
    println!(
        "compact_attrs_read_back_the_same: {drawings} drawings, {left_out} attributes left out and read back, {} changed fields alike",
        compact.len()
    );
}

#[test]
pub fn r12_drawings_hold_only_r12_kinds() {
    // lightweight polylines and multiline text came after R12: our R12 drawings write the
    // shapes as R12's own kinds (a polyline with vertices, a single-line text)
    let b = testgen::sampler(Ver::R12).build();
    let doc = k2_mpc_dxf::read_doc(&b.bytes, &Limits::default()).unwrap();
    let kinds: Vec<&str> = doc.entities.iter().map(|e| &*e.kind).collect();
    for k in ["LWPOLYLINE", "MTEXT", "ELLIPSE", "SPLINE"] {
        assert!(!kinds.contains(&k), "an R12 drawing holds a {k}");
    }
    assert_eq!(kinds.iter().filter(|k| **k == "POLYLINE").count(), 2);
    assert_eq!(doc.entities.len(), 12);
}

/// A text's box in the index is laid out as the drawing-model read lays it: a multiline text's
/// around its attachment point, a single-line text's around its alignment point.
#[test]
pub fn index_text_box_follows_alignment() {
    let b = testgen::draw_sampler(Ver::R2018).build();
    let d = draft(&b.bytes);
    let near = |a: f64, w: f64, what: &str| {
        assert!(
            (a - w).abs() <= 1e-9 * (1.0 + w.abs()),
            "{what}: {a}, expected {w}"
        );
    };
    for k in 1..=9usize {
        let n = node(&d, &b.addr(&format!("mtext-attach-{k}")));
        let bb = bbox(attr(n, "bbox"));
        let x = 300.0 + 60.0 * k as f64;
        let (col, row) = ((k - 1) % 3, (k - 1) / 3);
        near(
            [bb[0], (bb[0] + bb[2]) / 2.0, bb[2]][col],
            x,
            &format!("index box of attachment {k}: x at the attachment point"),
        );
        near(
            [bb[3], (bb[1] + bb[3]) / 2.0, bb[1]][row],
            100.0,
            &format!("index box of attachment {k}: y at the attachment point"),
        );
    }
    let c = bbox(attr(node(&d, &b.addr("text-centre")), "bbox"));
    near(
        (c[0] + c[2]) / 2.0,
        20.0,
        "a centred text's box: x at its alignment point",
    );
    near(
        (c[1] + c[3]) / 2.0,
        10.0,
        "a middle text's box: y at its alignment point",
    );
    println!("index_text_box_follows_alignment: 9 attachments and a centred text");
}

/// Every node's key, kind and name, before and after a change to what the index computes (a
/// format bump): the kit carries a node across versions by its key, so the keys must not move.
/// Byte for byte against files written at the commit before the change (`GOLDEN_WRITE=1`).
#[test]
pub fn index_keys_survive_format_bump() {
    let drawings = [
        ("sampler-2018", testgen::sampler(Ver::R2018).build().bytes),
        ("sampler-r12", testgen::sampler(Ver::R12).build().bytes),
        (
            "nested-2010",
            testgen::nested_blocks(Ver::R2010).build().bytes,
        ),
        ("draw-2018", testgen::draw_sampler(Ver::R2018).build().bytes),
        (
            "h1-2018",
            testgen::h1("h1-r2018").expect("a drawing").build().bytes,
        ),
    ];
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let write = std::env::var("GOLDEN_WRITE").as_deref() == Ok("1");
    let (mut compared, mut keys) = (0, 0);
    for (name, bytes) in &drawings {
        let d = draft(bytes);
        let mut lines: Vec<String> = d
            .nodes
            .iter()
            .map(|n| format!("{} {} {}", n.key.as_deref().unwrap_or("-"), n.kind, n.name))
            .collect();
        lines.sort();
        keys += lines.len();
        let got = lines.join("\n") + "\n";
        let path = dir.join(format!("index-keys-{name}.txt"));
        if write {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &got).unwrap();
            continue;
        }
        let want = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "no golden {}: write them at the commit before",
                path.display()
            )
        });
        assert!(got == want, "{name}: the index's keys moved");
        compared += 1;
    }
    assert!(write || compared == 5, "only {compared} compared");
    println!("index_keys_survive_format_bump: {compared} drawings, {keys} keys");
}

/// In the index too, a text's box is an estimate: its quality is `approx`, never `exact`.
#[test]
pub fn index_text_boxes_are_approx() {
    let b = testgen::draw_sampler(Ver::R2018).build();
    let d = draft(&b.bytes);
    let mut n = 0;
    for node in d
        .nodes
        .iter()
        .filter(|x| matches!(x.name.as_str(), "TEXT" | "MTEXT" | "ATTDEF"))
    {
        assert_eq!(
            attr(node, "bbox_q"),
            "approx",
            "{:?}: a text box is an estimate",
            node.key
        );
        n += 1;
    }
    assert!(n >= 10, "only {n} texts");
    println!("index_text_boxes_are_approx: {n} texts");
}

/// In the index, a 2018 multiline text's embedded object doesn't turn its box: the box is its
/// plain twin's, 40 to the right.
#[test]
pub fn index_mtext_embedded_object() {
    let b = testgen::mtext_embedded_sampler().build();
    let d = draft(&b.bytes);
    let p = bbox(attr(node(&d, &b.addr("plain")), "bbox"));
    let e = bbox(attr(node(&d, &b.addr("embedded")), "bbox"));
    same_box(
        e,
        [p[0] + 40.0, p[1], p[2] + 40.0, p[3]],
        "the embedded text's index box",
    );
    println!("index_mtext_embedded_object: the twin's box");
}
