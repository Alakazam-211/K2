//! The table objects over hand-made bytes: a control object's entries, a layer, a block record
//! with its entities, a linetype with a text element, a text style, a layout.

mod support;

use k2_mpc_dwg::frame::{open, Ver};
use k2_mpc_dwg::tables;
use support::bytes;
use support::object::{b, bd, bd3, bl, bs, h, object, rc, rd, tu, tv, Parts, F};

/// The object common data: no reactors, no extension dictionary.
fn common(v: Ver) -> F {
    let mut f = [bl(0), b(true)].concat();
    if v >= Ver::R2013 {
        f.extend(b(false));
    }
    f
}

/// A table entry's fields after its name: from 2007 one short (0, as every entry of the public
/// samples has it); before, the 64-flag, the external reference index plus one, the dependent
/// flag.
fn entry_flags(v: Ver) -> F {
    if v >= Ver::R2010 {
        bs(0)
    } else {
        [b(false), bs(0), b(false)].concat()
    }
}

#[test]
pub fn control_entries() {
    let v = Ver::R2018;
    let mut p = Parts::new(0x30, 0x01);
    p.data = [common(v), bl(1)].concat();
    p.handles = [h(4, 0), h(2, 0x20), h(3, 0x1F), h(3, 0x1E)].concat();
    let (buf, at) = object(v, &p);
    let mut o = open(&buf, at, v).expect("opens");
    assert_eq!(
        tables::control(&mut o, 2),
        Ok(vec![0x20, 0x1F, 0x1E]),
        "the entries, then model space and paper space"
    );
}

#[test]
pub fn layers() {
    let v = Ver::R2018;
    let mut p = Parts::new(0x33, 0x10);
    // off (2), lineweight index 7; colour 3, no true colour
    p.data = [
        common(v),
        entry_flags(v),
        bs(2 | (7 << 5)),
        bs(3),
        bl(0),
        rc(0),
    ]
    .concat();
    p.strings = tu("Walls");
    // its owner, external reference block, plot style, material, linetype, visual style
    p.handles = [
        h(4, 0x02),
        h(5, 0),
        h(5, 0x0F),
        h(5, 0),
        h(5, 0x16),
        h(5, 0),
    ]
    .concat();
    let (buf, at) = object(v, &p);
    let mut o = open(&buf, at, v).expect("opens");
    let l = tables::layer(&mut o).expect("reads");
    assert_eq!(l.entry.name, "Walls", "its name");
    assert!(
        l.off() && !l.frozen() && !l.locked(),
        "off, not frozen, not locked"
    );
    assert_eq!(
        (l.lineweight(), l.colour.index),
        (7, 3),
        "its weight and colour"
    );
    assert_eq!(l.ltype, Some(0x16), "its linetype");
}

#[test]
pub fn block_records() {
    let v = Ver::R2018;
    let mut p = Parts::new(0x31, 0x50);
    let mut d = [common(v), entry_flags(v)].concat();
    // not anonymous, attribute definitions, not an external reference; two entities
    d.extend([b(false), b(true), b(false), b(false), b(false), bl(2)].concat());
    d.extend(bd3([1.0, 2.0, 0.0]));
    d.extend([rc(1), rc(0)].concat());
    d.extend(bl(0));
    d.extend([bs(0), b(true), rc(0)].concat());
    p.data = d;
    p.strings = [tu("DOOR"), tu(""), tu("a door")].concat();
    p.handles = [
        h(4, 0x01),
        h(5, 0),
        h(3, 0x51),
        h(3, 0x52),
        h(3, 0x53),
        h(3, 0x54),
        h(4, 0x60),
        h(5, 0),
    ]
    .concat();
    let (buf, at) = object(v, &p);
    let mut o = open(&buf, at, v).expect("opens");
    let r = tables::block_header(&mut o).expect("reads");
    assert_eq!((r.entry.name.as_str(), r.base), ("DOOR", [1.0, 2.0, 0.0]));
    assert!(r.has_attdefs && !r.anonymous && !r.xref, "its flags");
    assert_eq!(r.block_entity, Some(0x51), "its BLOCK");
    assert_eq!(r.entities, vec![0x52, 0x53], "its entities, in order");
    assert_eq!(r.endblk, Some(0x54), "its ENDBLK");
}

/// xref_block_records: an external reference's block record writes no count of entities (it
/// owns none): a 2018 attached reference and a 2004 overlaid one read to their data's end with
/// their flags, base point, path, BLOCK, ENDBLK, insert and layout.
#[test]
pub fn xref_block_records() {
    let v = Ver::R2018;
    let mut p = Parts::new(0x31, 0x70);
    let mut d = [common(v), entry_flags(v)].concat();
    // not anonymous, no attribute definitions, an external reference, not overlaid, loaded; no
    // count of entities
    d.extend([b(false), b(false), b(true), b(false), b(false)].concat());
    d.extend(bd3([10.0, 20.0, 0.0]));
    d.extend([rc(1), rc(0)].concat());
    d.extend(bl(0));
    d.extend([bs(0), b(true), rc(0)].concat());
    p.data = d;
    p.strings = [tu("SITE"), tu(".\\site.dwg"), tu("")].concat();
    // its owner, external reference block, BLOCK, ENDBLK, one insert, its layout
    p.handles = [
        h(4, 0x01),
        h(5, 0),
        h(3, 0x71),
        h(3, 0x72),
        h(4, 0x80),
        h(5, 0),
    ]
    .concat();
    let (buf, at) = object(v, &p);
    let mut o = open(&buf, at, v).expect("opens");
    let r = tables::block_header(&mut o)
        .unwrap_or_else(|e| panic!("xref_block_records: 2018 refused: {}", e.summary()));
    assert_eq!(
        (r.entry.name.as_str(), r.xref_path.as_str(), r.base),
        ("SITE", ".\\site.dwg", [10.0, 20.0, 0.0]),
        "xref_block_records: 2018 its name, path and base point"
    );
    assert!(
        r.xref && !r.overlay && !r.anonymous,
        "xref_block_records: 2018 its flags"
    );
    assert_eq!(
        (r.block_entity, r.endblk, r.entities.len()),
        (Some(0x71), Some(0x72), 0),
        "xref_block_records: 2018 its BLOCK and ENDBLK, no entities"
    );
    assert_eq!(
        tables::extra_handle_bits(&o),
        0,
        "xref_block_records: 2018 its handle stream read to its end"
    );

    let v = Ver::R2004;
    let mut p = Parts::new(0x31, 0x74);
    let mut d = [common(v), tv(b"GRID"), entry_flags(v)].concat();
    // an overlaid external reference (both bits), loaded
    d.extend([b(false), b(false), b(true), b(true), b(false)].concat());
    d.extend(bd3([0.0, -5.0, 2.5]));
    d.extend(tv(b"grid.dwg"));
    d.extend(rc(0));
    d.extend(tv(b""));
    d.extend(bl(0));
    p.data = d;
    // its owner, external reference block, BLOCK, ENDBLK, no inserts, its layout
    p.handles = [h(4, 0x01), h(5, 0), h(3, 0x75), h(3, 0x76), h(5, 0)].concat();
    let (buf, at) = object(v, &p);
    let mut o = open(&buf, at, v).expect("opens");
    let r = tables::block_header(&mut o)
        .unwrap_or_else(|e| panic!("xref_block_records: 2004 refused: {}", e.summary()));
    assert_eq!(
        (r.entry.name.as_str(), r.xref_path.as_str(), r.base),
        ("GRID", "grid.dwg", [0.0, -5.0, 2.5]),
        "xref_block_records: 2004 its name, path and base point"
    );
    assert!(r.xref && r.overlay, "xref_block_records: 2004 overlaid");
    assert_eq!(
        (r.block_entity, r.endblk, r.entities.len()),
        (Some(0x75), Some(0x76), 0),
        "xref_block_records: 2004 its BLOCK and ENDBLK, no entities"
    );
}

#[test]
pub fn linetypes_and_styles() {
    // 2004: a dash, then a text element ("AB" at byte 1 of the text area)
    let v = Ver::R2004;
    let mut p = Parts::new(0x39, 0x16);
    let mut d = [
        common(v),
        tv(b"DASHTEXT"),
        entry_flags(v),
        tv(b"--AB--"),
        bd(1.5),
    ]
    .concat();
    d.extend([rc(b'A'), rc(2)].concat());
    d.extend([bd(1.0), bs(0), rd(0.0), rd(0.0), bd(1.0), bd(0.0), bs(0)].concat());
    d.extend([bd(-0.5), bs(1), rd(0.1), rd(-0.1), bd(0.2), bd(0.0), bs(2)].concat());
    let mut area = vec![0u8; 256];
    area[1..3].copy_from_slice(b"AB");
    d.extend(bytes(&area));
    p.data = d;
    p.handles = [h(4, 0x05), h(5, 0), h(5, 0), h(5, 0x11)].concat();
    let (buf, at) = object(v, &p);
    let mut o = open(&buf, at, v).expect("opens");
    let l = tables::ltype(&mut o).expect("reads");
    assert_eq!((l.entry.name.as_str(), l.length), ("DASHTEXT", 1.5));
    assert_eq!(l.dashes.len(), 2, "two dashes");
    assert_eq!(l.dashes[1].text.as_deref(), Some("AB"), "the text element");
    assert_eq!(l.dashes[1].style, Some(0x11), "its style");

    let v = Ver::R2018;
    let mut p = Parts::new(0x35, 0x11);
    p.data = [
        common(v),
        entry_flags(v),
        b(true),
        b(false),
        bd(0.0),
        bd(0.8),
        bd(0.0),
        rc(2),
        bd(2.5),
    ]
    .concat();
    p.strings = [tu("Notes"), tu("romans.shx"), tu("")].concat();
    p.handles = [h(4, 0x03), h(5, 0)].concat();
    let (buf, at) = object(v, &p);
    let mut o = open(&buf, at, v).expect("opens");
    let s = tables::style(&mut o).expect("reads");
    assert_eq!(
        (s.entry.name.as_str(), s.font.as_str()),
        ("Notes", "romans.shx")
    );
    assert_eq!(
        (s.width, s.generation),
        (0.8, 2),
        "its width and generation"
    );
    assert!(
        s.shape_file && !s.vertical,
        "a shape file, not vertical: the first bit, then the second"
    );
}

#[test]
pub fn layouts() {
    let v = Ver::R2018;
    let mut p = Parts::new(0x52, 0x90);
    let mut d = common(v);
    d.extend(bs(0));
    d.extend([bd(5.0), bd(6.0), bd(7.0), bd(8.0), bd(297.0), bd(210.0)].concat());
    d.extend([bd(0.0), bd(0.0), bs(1), bs(1), bs(0)].concat());
    d.extend([bd(0.0), bd(0.0), bd(0.0), bd(0.0), bd(1.0), bd(1.0)].concat());
    d.extend([bs(0), bd(1.0), bd(0.0), bd(0.0), bs(0), bs(0), bs(0)].concat());
    d.extend([bl(1), bs(0), bd3([0.0; 3])].concat());
    d.extend([rd(0.0), rd(0.0), rd(420.0), rd(297.0)].concat());
    for _ in 0..3 {
        d.extend(bd3([0.0; 3]));
    }
    d.extend([bd(0.0), bs(0), bd3([0.0; 3]), bd3([0.0; 3])].concat());
    d.extend(bl(2));
    p.data = d;
    p.strings = [tu(""), tu(""), tu("A4"), tu(""), tu("Sheet 1")].concat();
    p.handles = [
        h(4, 0x1A),
        h(5, 0),
        h(4, 0),
        h(4, 0x1E),
        h(4, 0),
        h(5, 0),
        h(5, 0),
        h(4, 0x91),
        h(4, 0x92),
    ]
    .concat();
    let (buf, at) = object(v, &p);
    let mut o = open(&buf, at, v).expect("opens");
    let l = tables::layout(&mut o).expect("reads");
    assert_eq!((l.name.as_str(), l.tab), ("Sheet 1", 1), "its name and tab");
    assert_eq!(
        (l.paper, l.margins),
        ([297.0, 210.0], [5.0, 6.0, 7.0, 8.0]),
        "its paper"
    );
    assert_eq!(l.block, Some(0x1E), "its block record");
    assert_eq!(l.viewports, vec![0x91, 0x92], "its viewports");
}

#[test]
pub fn book_colours() {
    let v = Ver::R2018;
    let mut p = Parts::new(0x1F5, 0x77);
    p.data = [common(v), bs(0), bl(0xC2FF_8000), rc(3)].concat();
    p.strings = [tu("RAL 2004"), tu("RAL CLASSIC")].concat();
    p.handles = [h(4, 0x76)].concat();
    let (buf, at) = object(v, &p);
    let mut o = open(&buf, at, v).expect("opens");
    let c = tables::dbcolor(&mut o).expect("reads");
    assert_eq!(c.rgb, Some(0xFF_8000), "its true colour");
    assert_eq!(
        c.dxf_name(),
        "RAL CLASSIC$RAL 2004",
        "its book and its name"
    );
}

#[test]
pub fn dimension_styles() {
    for v in [Ver::R2004, Ver::R2010, Ver::R2018] {
        let new = v >= Ver::R2010;
        let cmc = || [bs(256), bl(0), rc(0)].concat();
        let mut d = common(v);
        let mut s: F = Vec::new();
        if new {
            // the name, DIMPOST, DIMAPOST in the string stream
            s.extend([tu("ISO"), tu(""), tu("")].concat());
        } else {
            d.extend(tv(b"ISO"));
        }
        d.extend(entry_flags(v));
        if !new {
            d.extend([tv(b""), tv(b"")].concat());
        }
        // DIMSCALE 2, DIMASZ 1.25, seven more distances
        d.extend([bd(2.0), bd(1.25)].concat());
        for _ in 0..7 {
            d.extend(bd(0.0));
        }
        if new {
            d.extend([bd(0.0), bd(0.0), bs(0), cmc()].concat());
        }
        for _ in 0..6 {
            d.extend(b(false));
        }
        d.extend([bs(1), bs(8), bs(0)].concat());
        if new {
            d.extend(bs(0));
        }
        for _ in 0..9 {
            d.extend(bd(0.5));
        }
        d.extend([b(false), bs(2), b(true), b(false), b(false), b(false)].concat());
        // the three colours, then eleven shorts
        d.extend([cmc(), cmc(), cmc()].concat());
        for _ in 0..11 {
            d.extend(bs(2));
        }
        d.extend(
            [
                b(false),
                b(false),
                bs(1),
                bs(0),
                bs(0),
                bs(0),
                b(false),
                bs(3),
            ]
            .concat(),
        );
        if new {
            d.extend([b(false), b(false), bd(0.0), bd(0.0)].concat());
            s.extend([tu(""), tu("")].concat());
            // the texts sit in the string stream; their places in the data hold nothing
        }
        d.extend([bs(256), bs(256), b(true)].concat());
        let mut p = Parts::new(0x45, 0x27);
        p.data = d;
        p.strings = s;
        // its owner, external reference block, text style, leader arrowhead, then its three
        // arrowheads and from 2007 its three linetypes
        let rest = if new { 6 } else { 3 };
        let short = [h(4, 0x0A), h(5, 0), h(5, 0x11), h(5, 0x60)].concat();
        p.handles = [short.clone(), vec![(0x50, 8); rest]].concat();
        // one handle short: cut short
        let mut cut = p.clone();
        cut.handles = [short, vec![(0x50, 8); rest - 1]].concat();
        let (buf, at) = object(v, &cut);
        let mut o = open(&buf, at, v).expect("opens");
        support::refused(
            tables::dimstyle(&mut o),
            "DWG-E002",
            &format!("{v:?}: a dimension style one handle short"),
        );
        let (buf, at) = object(v, &p);
        let mut o = open(&buf, at, v).expect("opens");
        let ds =
            tables::dimstyle(&mut o).unwrap_or_else(|e| panic!("{v:?}: refused: {}", e.summary()));
        assert_eq!(ds.entry.name, "ISO", "{v:?}: its name");
        assert_eq!(
            (ds.scale, ds.arrow_size),
            (2.0, 1.25),
            "{v:?}: its overall scale and arrow size"
        );
        assert_eq!(
            ds.leader_arrow,
            Some(0x60),
            "{v:?}: its leader arrowhead block, after its text style"
        );
    }
}

#[test]
pub fn image_definitions() {
    for v in [Ver::R2004, Ver::R2018] {
        let mut p = Parts::new(600, 0x71);
        let mut d = [common(v), bl(0), rd(640.0), rd(480.0)].concat();
        if v >= Ver::R2010 {
            p.strings = tu("images/site.png");
        } else {
            d.extend(tv(b"images/site.png"));
        }
        d.extend([b(true), rc(0), rd(1.0), rd(1.0)].concat());
        p.data = d;
        p.handles = [h(4, 0x0D), h(4, 0x72)].concat();
        let (buf, at) = object(v, &p);
        let mut o = open(&buf, at, v).expect("opens");
        assert_eq!(
            tables::imagedef(&mut o).map_err(|e| e.summary()),
            Ok("images/site.png".to_owned()),
            "{v:?}: its image file's path"
        );
    }
}

/// entry_states: from 2007 the short after an entry's name is read as a short, whatever its
/// value (32 here takes ten bits, not two).
#[test]
pub fn entry_states() {
    let v = Ver::R2018;
    let mut p = Parts::new(0x43, 0x12);
    p.data = [common(v), bs(32), rc(0)].concat();
    p.strings = tu("ACAD");
    p.handles = [h(4, 0x09), h(5, 0)].concat();
    let (buf, at) = object(v, &p);
    let mut o = open(&buf, at, v).expect("opens");
    let e =
        tables::appid(&mut o).unwrap_or_else(|e| panic!("entry_states: refused: {}", e.summary()));
    assert_eq!(
        (e.name.as_str(), e.state, e.xdep),
        ("ACAD", 32, false),
        "entry_states: its name, its short, not dependent"
    );
}

/// entry_state_dependent: from 2007 an entry whose short has bit 256 is dependent on an
/// external reference, whatever its name.
#[test]
pub fn entry_state_dependent() {
    let v = Ver::R2018;
    let mut p = Parts::new(0x43, 0x13);
    p.data = [common(v), bs(256), rc(0)].concat();
    p.strings = tu("ACAD");
    p.handles = [h(4, 0x09), h(5, 0)].concat();
    let (buf, at) = object(v, &p);
    let mut o = open(&buf, at, v).expect("opens");
    let e = tables::appid(&mut o)
        .unwrap_or_else(|e| panic!("entry_state_dependent: refused: {}", e.summary()));
    assert_eq!(
        (e.state, e.xdep),
        (256, true),
        "entry_state_dependent: its short, dependent"
    );
}

/// layer_without_visual_style_refused: a 2018 layer whose handles stop after its linetype is
/// cut short (its visual style is missing).
#[test]
pub fn layer_without_visual_style_refused() {
    let v = Ver::R2018;
    let mut p = Parts::new(0x33, 0x10);
    p.data = [common(v), entry_flags(v), bs(7 << 5), bs(3), bl(0), rc(0)].concat();
    p.strings = tu("Walls");
    p.handles = [h(4, 0x02), h(5, 0), h(5, 0x0F), h(5, 0), h(5, 0x16)].concat();
    let (buf, at) = object(v, &p);
    let mut o = open(&buf, at, v).expect("opens");
    support::refused(
        tables::layer(&mut o),
        "DWG-E002",
        "layer_without_visual_style_refused",
    );
}
