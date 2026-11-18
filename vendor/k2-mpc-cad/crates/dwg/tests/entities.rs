//! Entities over hand-made bytes: one object at a time, its bit fields packed in the row (the
//! data, the strings for 2007 and later, the handles), its size fields and CRC-16 around it,
//! in the 2004 layout and in the 2010 layout. Each row checks the values its kind's read gives,
//! and that a read must end exactly at its data's end.

mod support;

use k2_mpc_dwg::entities::{entity, Body, Entity};
use k2_mpc_dwg::frame::{Ver, XItem};
use support::object::{
    b, bb, bd, bd3, bl, bs, dd, dd_same, ent, h, object, rc, rd, tu, tv, Common, Parts, F,
};
use support::{bytes, coded, refused};

fn read(v: Ver, obj: (Vec<u8>, u64)) -> Entity {
    match entity(&obj.0, obj.1, v, &[]) {
        Ok(e) => e,
        Err(e) => panic!("{v:?}: refused: {}", e.summary()),
    }
}

/// A LINE's fields: the z-zero bit set, start x and y raw, end x and y relative.
fn line_fields(p1: [f64; 2], p2: [f64; 2]) -> F {
    [
        b(true),
        rd(p1[0]),
        dd(p2[0]),
        rd(p1[1]),
        dd(p2[1]),
        b(true),
        b(true),
    ]
    .concat()
}

#[test]
pub fn common_data_by_version() {
    for v in [Ver::R2004, Ver::R2010, Ver::R2013, Ver::R2018] {
        let mut c = Common::on(0x10);
        c.owner = Some(0x1F);
        c.reactors = vec![0x40];
        c.xdic = Some(0x41);
        c.ltype = Some(0x16);
        c.lw = 5;
        c.colour = bs(1);
        let (mut d, hs) = c.fields(v);
        d.extend(line_fields([1.0, 2.0], [3.0, 4.0]));
        // extension data: an application (0x12) with a string "Ab" and a short 7
        let mut items: Vec<u8> = Vec::new();
        if v >= Ver::R2010 {
            items.extend([0, 2, 0, b'A', 0, b'b', 0]);
        } else {
            items.extend([0, 2, 0x1E, 0x00, b'A', b'b']);
        }
        items.extend([70, 7, 0]);
        let mut p = Parts::new(0x13, 0x2A);
        p.eed = bs(items.len() as u16);
        p.eed.extend(h(5, 0x12));
        p.eed.extend(bytes(&items));
        p.eed.extend(bs(0));
        p.data = d;
        p.handles = hs;
        let e = read(v, object(v, &p));
        let at = format!("{v:?}");
        assert_eq!(
            (e.kind.as_str(), e.handle),
            ("LINE", 0x2A),
            "{at}: kind and handle"
        );
        assert_eq!(e.common.owner, Some(0x1F), "{at}: the owner");
        assert_eq!(e.common.reactors, vec![0x40], "{at}: the reactors");
        assert_eq!(e.common.xdic, Some(0x41), "{at}: the extension dictionary");
        assert_eq!(e.common.layer, Some(0x10), "{at}: the layer");
        assert_eq!(e.common.ltype, Some(0x16), "{at}: the linetype");
        assert_eq!(
            (e.common.colour.index, e.common.lineweight),
            (1, 5),
            "{at}: colour, weight"
        );
        assert_eq!(e.eed.len(), 1, "{at}: one application's extension data");
        assert_eq!(e.eed[0].app, 0x12, "{at}: its application");
        assert_eq!(
            e.eed[0].items,
            vec![XItem::Str("Ab".into()), XItem::Short(7)],
            "{at}: its items"
        );
        assert_eq!(
            e.body,
            Body::Line {
                p1: [1.0, 2.0, 0.0],
                p2: [3.0, 4.0, 0.0],
                thickness: 0.0,
                n: [0.0, 0.0, 1.0]
            },
            "{at}: the line"
        );
    }
}

#[test]
pub fn a_graphic_skipped() {
    for v in [Ver::R2004, Ver::R2018] {
        let mut c = Common::on(0x10);
        c.graphic = Some(vec![0xAA; 300 % 256]);
        let e = read(
            v,
            ent(
                v,
                0x13,
                0x2B,
                &c,
                line_fields([5.0, 6.0], [7.0, 8.0]),
                vec![],
                vec![],
            ),
        );
        assert_eq!(
            e.common.layer,
            Some(0x10),
            "{v:?}: the layer after a graphic"
        );
        assert!(
            matches!(e.body, Body::Line { p1, .. } if p1 == [5.0, 6.0, 0.0]),
            "{v:?}: the line after a graphic: {:?}",
            e.body
        );
    }
}

#[test]
pub fn a_book_colour() {
    let v = Ver::R2018;
    let mut c = Common::on(0x10);
    // flags 0xC000 (a book colour, with 0x8000 set too): no true colour in the data
    c.colour = coded(0, &[0x05, 0xC0]);
    c.book = Some(0x77);
    let e = read(
        v,
        ent(
            v,
            0x12,
            0x2C,
            &c,
            [bd3([1.0, 1.0, 0.0]), bd(2.0), b(true), b(true)].concat(),
            vec![],
            vec![],
        ),
    );
    assert_eq!(e.common.book, Some(0x77), "the book colour's handle");
    assert_eq!(e.common.layer, Some(0x10), "the layer after it");
    assert!(
        matches!(e.body, Body::Circle { r, .. } if r == 2.0),
        "the circle: {:?}",
        e.body
    );
}

#[test]
pub fn lines_arcs_circles() {
    let v = Ver::R2013;
    let c = Common::on(0x10);
    // a LINE with z values: z-zero bit clear, the end's x and z equal to the start's
    let line = [
        b(false),
        rd(1.5),
        dd_same(),
        rd(2.5),
        dd(9.0),
        rd(3.0),
        dd_same(),
        b(false),
        bd(0.25),
        b(false),
        bd3([0.0, 0.0, -1.0]),
    ]
    .concat();
    let e = read(v, ent(v, 0x13, 0x30, &c, line, vec![], vec![]));
    assert_eq!(
        e.body,
        Body::Line {
            p1: [1.5, 2.5, 3.0],
            p2: [1.5, 9.0, 3.0],
            thickness: 0.25,
            n: [0.0, 0.0, -1.0]
        }
    );
    let arc = [
        bd3([4.0, 5.0, 0.0]),
        bd(2.0),
        b(true),
        b(true),
        bd(0.5),
        bd(2.5),
    ]
    .concat();
    let e = read(v, ent(v, 0x11, 0x31, &c, arc, vec![], vec![]));
    assert_eq!(
        e.body,
        Body::Arc {
            c: [4.0, 5.0, 0.0],
            r: 2.0,
            thickness: 0.0,
            n: [0.0, 0.0, 1.0],
            a0: 0.5,
            a1: 2.5
        }
    );
    let circle = [bd3([-1.0, 0.0, 0.0]), bd(3.0), b(true), b(true)].concat();
    let e = read(v, ent(v, 0x12, 0x32, &c, circle, vec![], vec![]));
    assert_eq!(
        e.body,
        Body::Circle {
            c: [-1.0, 0.0, 0.0],
            r: 3.0,
            thickness: 0.0,
            n: [0.0, 0.0, 1.0]
        }
    );
}

#[test]
pub fn light_polylines() {
    let v = Ver::R2013;
    let c = Common::on(0x10);
    // closed (0x200), elevation (8), bulges (16), widths (32); three points, the second's x and
    // the third's y the previous point's
    let mut f = bs(0x200 | 8 | 16 | 32);
    f.extend(bd(2.0));
    f.extend(bl(3));
    f.extend(bl(3));
    f.extend(bl(3));
    f.extend(rd(0.0));
    f.extend(rd(0.0));
    f.extend(dd_same());
    f.extend(dd(4.0));
    f.extend(dd(6.0));
    f.extend(dd_same());
    f.extend([bd(0.5), bd(0.0), bd(0.0)].concat());
    for _ in 0..3 {
        f.extend(bd(0.0));
        f.extend(bd(1.0));
    }
    let e = read(v, ent(v, 0x4D, 0x33, &c, f, vec![], vec![]));
    let Body::LwPolyline(l) = e.body else {
        panic!("not a light polyline: {:?}", e.body)
    };
    assert!(l.closed(), "closed");
    assert_eq!(l.elevation, 2.0, "the elevation");
    assert_eq!(
        l.points,
        vec![[0.0, 0.0], [0.0, 4.0], [6.0, 4.0]],
        "each default from the point before"
    );
    assert_eq!(l.bulges, vec![0.5, 0.0, 0.0], "the bulges");
    assert_eq!(l.widths, vec![[0.0, 1.0]; 3], "the widths");
}

#[test]
pub fn texts() {
    // 2004: the text inline in code page 1252 (0x80 is the euro sign); the data flags leave
    // out the elevation, the alignment point, the oblique angle and the width factor
    let v = Ver::R2004;
    let c = Common::on(0x10);
    let mut f = rc(0x01 | 0x02 | 0x04 | 0x10);
    f.extend(rd(10.0));
    f.extend(rd(20.0));
    f.extend(b(true));
    f.extend(b(true));
    f.extend(rd(0.5));
    f.extend(rd(2.5));
    f.extend(tv(b"5\x80"));
    f.extend(bs(0));
    f.extend(bs(1));
    f.extend(bs(2));
    let e = read(v, ent(v, 0x01, 0x34, &c, f, vec![], h(5, 0x11)));
    let Body::Text(t) = e.body else {
        panic!("not a text: {:?}", e.body)
    };
    assert_eq!(t.text, "5\u{20AC}", "the text, by code page 1252");
    assert_eq!(
        (t.ins, t.align),
        ([10.0, 20.0], None),
        "the insertion point"
    );
    assert_eq!(
        (t.rotation, t.height, t.width),
        (0.5, 2.5, 1.0),
        "rotation, height, width"
    );
    assert_eq!((t.halign, t.valign), (1, 2), "the alignment");
    assert_eq!(t.style, Some(0x11), "the style");

    // 2018: a multiline text, not annotative, its redundant fields and no columns; the text
    // from the string stream
    let v = Ver::R2018;
    let mut f = [
        bd3([1.0, 2.0, 0.0]),
        bd3([0.0, 0.0, 1.0]),
        bd3([1.0, 0.0, 0.0]),
        bd(30.0),
        bd(0.0),
        bd(2.5),
        bs(5),
        bs(1),
        bd(0.0),
        bd(0.0),
        bs(1),
        bd(1.0),
        b(false),
        bl(0),
    ]
    .concat();
    f.extend(b(true));
    f.extend(
        [
            bs(0),
            b(true),
            bl(1),
            bd3([1.0, 0.0, 0.0]),
            bd3([1.0, 2.0, 0.0]),
        ]
        .concat(),
    );
    f.extend([bd(30.0), bd(0.0), bd(0.0), bd(0.0)].concat());
    f.extend(bs(0));
    // its style, then its redundant fields' application
    let own = [h(5, 0x11), h(5, 0)].concat();
    let e = read(v, ent(v, 0x2C, 0x35, &c, f, tu("Line\\Pnext"), own));
    let Body::MText(m) = e.body else {
        panic!("not a multiline text: {:?}", e.body)
    };
    assert_eq!(m.text, "Line\\Pnext", "the text from the string stream");
    assert_eq!(
        (m.ins, m.rect_width, m.height, m.attach),
        ([1.0, 2.0, 0.0], 30.0, 2.5, 5)
    );
    assert_eq!(m.style, Some(0x11), "the style");
}

#[test]
pub fn mtext_column_heights() {
    // 2018: a multiline text, not annotative, two dynamic columns of manual height: the first
    // column's height not set (stored as minus the text's extent), the second set
    let v = Ver::R2018;
    let c = Common::on(0x10);
    let mut f = [
        bd3([1.0, 2.0, 0.0]),
        bd3([0.0, 0.0, 1.0]),
        bd3([1.0, 0.0, 0.0]),
        bd(30.0),
        bd(0.0),
        bd(2.5),
        bs(1),
        bs(1),
        bd(4.5),
        bd(20.0),
        bs(1),
        bd(1.0),
        b(false),
        bl(0),
    ]
    .concat();
    f.extend(b(true));
    f.extend(
        [
            bs(0),
            b(true),
            bl(1),
            bd3([1.0, 0.0, 0.0]),
            bd3([1.0, 2.0, 0.0]),
        ]
        .concat(),
    );
    f.extend([bd(30.0), bd(0.0), bd(20.0), bd(4.5)].concat());
    f.extend([bs(2), bl(2), bd(12.0), bd(2.0), b(false), b(false)].concat());
    f.extend([bd(-4.5), bd(6.0)].concat());
    let own = [h(5, 0x11), h(5, 0)].concat();
    let e = read(v, ent(v, 0x2C, 0x37, &c, f, tu("A"), own));
    let Body::MText(m) = e.body else {
        panic!("not a multiline text: {:?}", e.body)
    };
    let cols = m.columns.expect("its columns");
    assert_eq!(
        (cols.kind, cols.count, cols.width, cols.gutter),
        (2, 2, 12.0, 2.0),
        "dynamic, two columns, their width and gutter"
    );
    assert_eq!(
        cols.heights,
        vec![0.0, 6.0],
        "a height not set (stored negative) read as 0, a set one as stored"
    );
}

/// An ATTRIB's fields (2018, single line): its text fields, version, type, tag, field length,
/// flags, lock.
fn attrib(value: &str, tag: &str, at: [f64; 2]) -> (F, F) {
    let mut f = rc(0x01 | 0x02 | 0x04 | 0x08 | 0x10 | 0x20 | 0x40 | 0x80);
    f.extend(rd(at[0]));
    f.extend(rd(at[1]));
    f.extend(b(true));
    f.extend(b(true));
    f.extend(rd(1.0));
    f.extend([rc(0), rc(1)].concat());
    f.extend(bs(0));
    f.extend(rc(0));
    f.extend(b(false));
    let mut s = tu(value);
    s.extend(tu(tag));
    (f, s)
}

#[test]
pub fn multi_line_attributes() {
    let v = Ver::R2018;
    let mut a = Common::on(0x10);
    a.owner = Some(0x60);
    // the text fields as a single-line attribute's, then version 0 and type 2 (multi-line)
    let mut f = rc(0x01 | 0x02 | 0x04 | 0x08 | 0x10 | 0x20 | 0x40 | 0x80);
    f.extend([rd(1.0), rd(2.0), b(true), b(true), rd(2.5), rc(0), rc(2)].concat());
    // the embedded multiline text: the data an entity begins with, from its entity mode
    f.extend(
        [
            bb(0),
            bl(0),
            b(true),
            b(false),
            bs(256),
            bd(1.0),
            bb(0),
            bb(0),
            bb(0),
            rc(0),
            b(false),
            b(false),
            b(false),
            bs(0),
            rc(29),
        ]
        .concat(),
    );
    // then a multiline text's fields (not annotative, its redundant fields, no columns)
    f.extend(
        [
            bd3([1.0, 2.0, 0.0]),
            bd3([0.0, 0.0, 1.0]),
            bd3([1.0, 0.0, 0.0]),
            bd(40.0),
            bd(0.0),
            bd(2.5),
            bs(1),
            bs(1),
            bd(5.0),
            bd(20.0),
            bs(1),
            bd(1.0),
            b(false),
            bl(0),
            b(true),
            bs(0),
            b(true),
            bl(1),
            bd3([1.0, 0.0, 0.0]),
            bd3([1.0, 2.0, 0.0]),
            bd(40.0),
            bd(0.0),
            bd(20.0),
            bd(5.0),
            bs(0),
        ]
        .concat(),
    );
    // no annotative data, then the tag, field length, flags and lock
    f.extend([bs(0), bs(0), rc(0), b(false)].concat());
    let s = [tu("Line one"), tu("Line one\\Pline two"), tu("NOTE")].concat();
    let e = read(v, ent(v, 0x02, 0x64, &a, f, s, h(5, 0x11)));
    let Body::Attrib(t) = e.body else {
        panic!("not an attribute: {:?}", e.body)
    };
    assert_eq!(
        (t.tag.as_str(), t.text.text.as_str()),
        ("NOTE", "Line one"),
        "its tag after its embedded text, its single-line text"
    );
    let m = t.multiline.expect("its embedded multiline text");
    assert_eq!(
        (m.text.as_str(), m.rect_width, m.attach),
        ("Line one\\Pline two", 40.0, 1),
        "the embedded multiline text's content, width and attachment"
    );
}

#[test]
pub fn references_with_attributes() {
    let v = Ver::R2018;
    let c = Common::on(0x10);
    // an INSERT: scale 1 (data flags 11), rotation 0, extrusion up, two attributes
    let f = [
        bd3([5.0, 6.0, 0.0]),
        bb(3),
        bd(0.0),
        bd3([0.0, 0.0, 1.0]),
        b(true),
        bl(2),
    ]
    .concat();
    let own = [h(5, 0x50), h(3, 0x61), h(3, 0x62), h(3, 0x63)].concat();
    let e = read(v, ent(v, 0x07, 0x60, &c, f, vec![], own));
    let Body::Insert(i) = e.body else {
        panic!("not a reference: {:?}", e.body)
    };
    assert_eq!(i.block, Some(0x50), "its block");
    assert_eq!(i.attribs, vec![0x61, 0x62], "its attributes, in order");
    assert_eq!(i.seqend, Some(0x63), "its end marker");
    assert_eq!(
        (i.ins, i.scale),
        ([5.0, 6.0, 0.0], [1.0; 3]),
        "where and how large"
    );

    let mut a = Common::on(0x10);
    a.owner = Some(0x60);
    let (f, s) = attrib("42", "NUM", [5.5, 6.5]);
    let e = read(v, ent(v, 0x02, 0x61, &a, f, s, h(5, 0x11)));
    assert_eq!(e.common.owner, Some(0x60), "the attribute's owner");
    let Body::Attrib(t) = e.body else {
        panic!("not an attribute: {:?}", e.body)
    };
    assert_eq!(
        (t.tag.as_str(), t.text.text.as_str()),
        ("NUM", "42"),
        "tag and value"
    );
    assert_eq!(t.text.ins, [5.5, 6.5], "where it sits");
}

#[test]
pub fn data_end_checked() {
    let v = Ver::R2018;
    let c = Common::on(0x10);
    let whole = ent(
        v,
        0x13,
        0x36,
        &c,
        line_fields([1.0, 1.0], [2.0, 2.0]),
        vec![],
        vec![],
    );
    assert!(
        entity(&whole.0, whole.1, v, &[]).is_ok(),
        "the undamaged line reads"
    );
    let mut f = line_fields([1.0, 1.0], [2.0, 2.0]);
    f.extend(b(false));
    let longer = ent(v, 0x13, 0x36, &c, f, vec![], vec![]);
    refused(
        entity(&longer.0, longer.1, v, &[]),
        "DWG-E003",
        "a line whose data runs a bit past its fields",
    );
}

#[test]
pub fn hatches_dimensions_viewports() {
    // a solid hatch: one polyline path (closed, no bulges), one path of a line edge, a source
    let v = Ver::R2010;
    let c = Common::on(0x10);
    let mut f = [bl(0), bl(0), bd(0.0), bd(0.0), bl(0), bd(0.0), bl(0)].concat();
    f.extend(tu_or_tv(v, "")); // the gradient's name (a string: from the string stream here)
    f.extend([bd(0.0), bd3([0.0, 0.0, 1.0])].concat());
    f.extend(b(true));
    f.extend(b(false));
    f.extend(bl(2));
    f.extend(
        [
            bl(2),
            b(false),
            b(true),
            bl(3),
            rd(0.0),
            rd(0.0),
            rd(1.0),
            rd(0.0),
            rd(1.0),
            rd(1.0),
            bl(0),
        ]
        .concat(),
    );
    f.extend(
        [
            bl(1),
            bl(1),
            rc(1),
            rd(0.0),
            rd(0.0),
            rd(2.0),
            rd(0.0),
            bl(1),
        ]
        .concat(),
    );
    f.extend([bs(1), bs(1), bl(0)].concat());
    let e = read(
        v,
        ent(
            v,
            0x4E,
            0x70,
            &c,
            f,
            [tu(""), tu("SOLID")].concat(),
            h(4, 0x71),
        ),
    );
    let Body::Hatch(x) = e.body else {
        panic!("not a hatch: {:?}", e.body)
    };
    assert!(x.solid, "solid");
    assert_eq!(x.name, "SOLID", "its pattern's name");
    assert_eq!(x.paths.len(), 2, "two paths");
    assert_eq!(
        x.paths[0].poly,
        Some((
            vec![([0.0, 0.0], 0.0), ([1.0, 0.0], 0.0), ([1.0, 1.0], 0.0)],
            true
        )),
        "the polyline path"
    );
    assert_eq!(x.sources, vec![0x71], "the boundary's source");

    // a linear dimension (2018): its common dimension data, then 13, 14, 10, two rotations
    let v = Ver::R2018;
    let mut f = [
        rc(0),
        bd3([0.0, 0.0, 1.0]),
        rd(5.0),
        rd(1.0),
        bd(0.0),
        rc(1),
    ]
    .concat();
    f.extend([bd(0.0), bd(0.0), bd(1.0), bd(1.0), bd(1.0), bd(0.0)].concat());
    f.extend([bs(5), bs(1), bd(1.0), bd(10.0)].concat());
    f.extend([b(false), b(false), b(false)].concat());
    f.extend([rd(0.0), rd(0.0)].concat());
    f.extend(
        [
            bd3([0.0, 0.0, 0.0]),
            bd3([10.0, 0.0, 0.0]),
            bd3([10.0, 1.0, 0.0]),
        ]
        .concat(),
    );
    f.extend([bd(0.0), bd(0.0)].concat());
    let e = read(
        v,
        ent(
            v,
            0x15,
            0x72,
            &c,
            f,
            tu(""),
            [h(5, 0x73), h(5, 0x74)].concat(),
        ),
    );
    let Body::Dimension(d) = e.body else {
        panic!("not a dimension: {:?}", e.body)
    };
    assert_eq!(
        (d.style, d.block),
        (Some(0x73), Some(0x74)),
        "its style and block"
    );
    assert_eq!(d.measured, 10.0, "its measurement");
    assert_eq!(d.dxf_type, 0, "rotated, its text where the block puts it");
    assert!(
        d.defs.contains(&(14, [10.0, 0.0, 0.0])),
        "its 14 point: {:?}",
        d.defs
    );

    // a viewport (2004) with two frozen layers and a clip boundary
    let v = Ver::R2004;
    let mut f = [
        bd3([5.0, 5.0, 0.0]),
        bd(10.0),
        bd(8.0),
        bd3([0.0; 3]),
        bd3([0.0, 0.0, 1.0]),
    ]
    .concat();
    f.extend([bd(0.0), bd(20.0), bd(50.0), bd(0.0), bd(0.0), bd(0.0)].concat());
    f.extend(
        [
            rd(3.0),
            rd(4.0),
            rd(0.0),
            rd(0.0),
            rd(1.0),
            rd(1.0),
            rd(1.0),
            rd(1.0),
        ]
        .concat(),
    );
    f.extend(bs(100));
    f.extend([bl(2), bl(0x30000)].concat());
    f.extend(tv(b""));
    f.extend([rc(0), b(false), b(false)].concat());
    f.extend(
        [
            bd3([0.0; 3]),
            bd3([1.0, 0.0, 0.0]),
            bd3([0.0, 1.0, 0.0]),
            bd(0.0),
            bs(0),
            bs(0),
        ]
        .concat(),
    );
    // its frozen layers, its clip boundary, then (2004) its named and base coordinate systems
    let own = [h(4, 0x80), h(4, 0x81), h(4, 0x82), h(5, 0), h(5, 0)].concat();
    // without its base coordinate system, cut short
    let short = [h(4, 0x80), h(4, 0x81), h(4, 0x82), h(5, 0)].concat();
    let cut = ent(v, 0x22, 0x75, &c, f.clone(), vec![], short);
    refused(
        entity(&cut.0, cut.1, v, &[]),
        "DWG-E002",
        "a viewport whose handles stop before its base coordinate system",
    );
    let e = read(v, ent(v, 0x22, 0x75, &c, f, vec![], own));
    let Body::Viewport(p) = e.body else {
        panic!("not a viewport: {:?}", e.body)
    };
    assert_eq!(p.frozen, vec![0x80, 0x81], "its frozen layers");
    assert_eq!(p.clip, Some(0x82), "its clip boundary");
    assert_eq!(
        (p.view_centre, p.view_height),
        ([3.0, 4.0], 20.0),
        "its view"
    );
}

/// A string field: nothing in the data from 2007 (it sits in the string stream).
fn tu_or_tv(v: Ver, s: &str) -> F {
    if v >= Ver::R2010 {
        Vec::new()
    } else {
        tv(s.as_bytes())
    }
}

#[test]
pub fn splines_and_rays() {
    let v = Ver::R2013;
    let c = Common::on(0x10);
    // control points and knots (scenario 1), rational: two weighted control points
    let mut f = [bl(1), bl(0), bl(15), bl(1)].concat();
    f.extend(
        [
            b(true),
            b(false),
            b(false),
            bd(0.0),
            bd(0.0),
            bl(4),
            bl(2),
            b(true),
        ]
        .concat(),
    );
    f.extend([bd(0.0), bd(0.0), bd(1.0), bd(1.0)].concat());
    f.extend([bd3([0.0, 0.0, 0.0]), bd(1.0), bd3([4.0, 2.0, 0.0]), bd(0.5)].concat());
    let e = read(v, ent(v, 0x24, 0x80, &c, f, vec![], vec![]));
    let Body::Spline(s) = e.body else {
        panic!("not a spline: {:?}", e.body)
    };
    assert_eq!((s.degree, s.flags), (1, 4), "degree 1, rational");
    assert_eq!(s.knots, vec![0.0, 0.0, 1.0, 1.0], "its knots");
    assert_eq!(
        s.ctrl,
        vec![[0.0; 3], [4.0, 2.0, 0.0]],
        "its control points"
    );
    assert_eq!(
        s.weights,
        vec![1.0, 0.5],
        "a weight after each control point"
    );
    // fit points (2013: the fit-points and closed flags, a knot parameter other than custom;
    // the stored scenario, 0 here, set aside)
    let mut f = [bl(0), bl(5), bl(0), bl(3)].concat();
    f.extend([bd(0.0), bd3([1.0, 0.0, 0.0]), bd3([0.0, 1.0, 0.0]), bl(3)].concat());
    f.extend([bd3([0.0; 3]), bd3([1.0, 1.0, 0.0]), bd3([2.0, 0.0, 0.0])].concat());
    let e = read(v, ent(v, 0x24, 0x81, &c, f, vec![], vec![]));
    let Body::Spline(s) = e.body else {
        panic!("not a spline: {:?}", e.body)
    };
    assert_eq!(
        (s.degree, s.flags, s.fit.len()),
        (3, 1, 3),
        "closed, three fit points"
    );
    // a ray: a point and a direction
    let f = [bd3([1.0, 2.0, 0.0]), bd3([1.0, 0.0, 0.0])].concat();
    let e = read(v, ent(v, 0x28, 0x82, &c, f, vec![], vec![]));
    assert_eq!(
        e.body,
        Body::XLine {
            point: [1.0, 2.0, 0.0],
            dir: [1.0, 0.0, 0.0],
            ray: true
        },
        "a ray"
    );
}

#[test]
pub fn images_and_wipeouts() {
    use k2_mpc_dwg::classes::Class;
    let class = |number: u16, name: &str| Class {
        number,
        proxy_flags: 0,
        app_name: "ISM".into(),
        cpp_name: "AcDbRasterImage".into(),
        dxf_name: name.into(),
        zombie: false,
        item_class_id: 0x1F2,
        instances: 1,
    };
    let classes = [class(510, "IMAGE"), class(511, "WIPEOUT")];
    for v in [Ver::R2004, Ver::R2018] {
        // an image: a rectangular clip (two corners)
        let mut f = [
            bl(0),
            bd3([10.0, 20.0, 0.0]),
            bd3([0.5, 0.0, 0.0]),
            bd3([0.0, 0.5, 0.0]),
            rd(640.0),
            rd(480.0),
            bs(7),
            b(true),
            rc(50),
            rc(50),
            rc(0),
        ]
        .concat();
        if v >= Ver::R2010 {
            f.extend(b(false));
        }
        f.extend([bs(1), rd(-0.5), rd(-0.5), rd(639.5), rd(479.5)].concat());
        let obj = ent(
            v,
            510,
            0x70,
            &Common::on(0x10),
            f,
            vec![],
            [h(5, 0x71), h(3, 0x72)].concat(),
        );
        let e = entity(&obj.0, obj.1, v, &classes)
            .unwrap_or_else(|e| panic!("{v:?}: refused: {}", e.summary()));
        let Body::Image { data, wipeout } = e.body else {
            panic!("{v:?}: not an image: {:?}", e.body)
        };
        assert!(!wipeout, "{v:?}: an image");
        assert_eq!(
            (data.ins, data.u, data.v, data.size),
            (
                [10.0, 20.0, 0.0],
                [0.5, 0.0, 0.0],
                [0.0, 0.5, 0.0],
                [640.0, 480.0]
            ),
            "{v:?}: where it sits, its pixel axes and size"
        );
        assert_eq!(
            (data.clip_type, data.clip.clone(), data.def),
            (1, vec![[-0.5, -0.5], [639.5, 479.5]], Some(0x71)),
            "{v:?}: a rectangular clip's two corners, its definition"
        );
        // a wipeout: a polygon clip (four points)
        let mut f = [
            bl(0),
            bd3([0.0; 3]),
            bd3([1.0, 0.0, 0.0]),
            bd3([0.0, 1.0, 0.0]),
            rd(1.0),
            rd(1.0),
            bs(7),
            b(true),
            rc(50),
            rc(50),
            rc(0),
        ]
        .concat();
        if v >= Ver::R2010 {
            f.extend(b(false));
        }
        f.extend([bs(2), bl(4)].concat());
        for p in [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]] {
            f.extend([rd(p[0]), rd(p[1])].concat());
        }
        let obj = ent(
            v,
            511,
            0x73,
            &Common::on(0x10),
            f,
            vec![],
            [h(5, 0), h(3, 0)].concat(),
        );
        let e = entity(&obj.0, obj.1, v, &classes)
            .unwrap_or_else(|e| panic!("{v:?}: refused: {}", e.summary()));
        let Body::Image { data, wipeout } = e.body else {
            panic!("{v:?}: not a wipeout: {:?}", e.body)
        };
        assert!(wipeout, "{v:?}: a wipeout");
        assert_eq!(
            (data.clip_type, data.clip.len(), data.def),
            (2, 4, None),
            "{v:?}: a polygon clip, no definition"
        );
    }
}
