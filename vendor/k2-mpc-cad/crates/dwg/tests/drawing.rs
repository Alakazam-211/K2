//! The drawing-model mapping of decoded entities: angles in degrees, handles named, a light
//! polyline's vertices with their bulges and widths, a dimension's defining points.

use std::collections::HashMap;
use std::f64::consts::PI;

use k2_mpc_cad_model::draw::{DrawSource, Geom};
use k2_mpc_dwg::bits::Colour;
use k2_mpc_dwg::drawing::{
    annotative_eed, attdef_info, count_left_out, dim_overrides, dynamic_target, geom_of, poly_geom,
    unique_block_names, DwgDrawing, Names, Tolerated,
};
use k2_mpc_dwg::entities::{AttData, Body, Entity, InsertData, LwData, TextData};
use k2_mpc_dwg::error::BAD_VALUE;
use k2_mpc_dwg::frame::EntCommon;
use k2_mpc_dwg::frame::{Eed, XItem};
use k2_mpc_dwg::{CadError, Limits};

fn ent(body: Body) -> Entity {
    Entity {
        handle: 0x30,
        kind: String::new(),
        type_number: 0,
        eed: Vec::new(),
        common: EntCommon {
            entmode: 2,
            colour: Colour {
                index: 256,
                rgb: None,
                book: false,
                transparency: None,
            },
            lt_scale: 1.0,
            lt_flags: 0,
            ps_flags: 0,
            mat_flags: 0,
            invisible: false,
            lineweight: 29,
            owner: None,
            reactors: Vec::new(),
            xdic: None,
            book: None,
            layer: Some(0x10),
            ltype: None,
            plotstyle: None,
            material: None,
        },
        body,
        extra_handle_bits: 0,
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
pub fn geometry_for_the_model() {
    let mut names = Names::default();
    names.block.insert(0x50, "DOOR".into());
    let up = [0.0, 0.0, 1.0];
    match geom_of(
        &ent(Body::Arc {
            c: [0.0; 3],
            r: 1.0,
            thickness: 0.0,
            n: up,
            a0: PI / 2.0,
            a1: PI,
        }),
        &names,
        Vec::new(),
    ) {
        Geom::Arc { a0, a1, .. } => {
            assert!(
                near(a0, 90.0) && near(a1, 180.0),
                "an arc's angles in degrees: {a0} {a1}"
            )
        }
        g => panic!("not an arc: {g:?}"),
    }
    let ins = InsertData {
        ins: [1.0, 2.0, 0.0],
        scale: [1.0; 3],
        rotation: PI,
        n: up,
        array: None,
        block: Some(0x50),
        has_attribs: false,
        owned: 0,
        attribs: Vec::new(),
        seqend: None,
    };
    match geom_of(&ent(Body::Insert(ins)), &names, Vec::new()) {
        Geom::Insert {
            block, rotation, ..
        } => {
            assert_eq!(block, "DOOR", "the block by its handle");
            assert!(near(rotation, 180.0), "the rotation in degrees: {rotation}");
        }
        g => panic!("not a reference: {g:?}"),
    }
    let lw = LwData {
        flags: 0x200 | 16 | 32,
        const_width: 0.0,
        elevation: 3.0,
        thickness: 0.0,
        n: up,
        points: vec![[0.0, 0.0], [1.0, 0.0]],
        bulges: vec![0.5, 0.0],
        widths: vec![[0.1, 0.2], [0.0, 0.0]],
    };
    match geom_of(&ent(Body::LwPolyline(lw)), &names, Vec::new()) {
        Geom::Poly { verts, closed, .. } => {
            assert!(closed, "closed");
            assert_eq!(verts[0].p, [0.0, 0.0, 3.0], "a vertex at the elevation");
            assert_eq!((verts[0].bulge, verts[0].w0, verts[0].w1), (0.5, 0.1, 0.2));
        }
        g => panic!("not a polyline: {g:?}"),
    }
}

#[test]
pub fn repeated_block_names_numbered() {
    assert_eq!(
        unique_block_names(&[
            "*Model_Space",
            "*Paper_Space",
            "*Paper_Space",
            "*D",
            "*D",
            "*D1",
            "*D",
            "DOOR"
        ]),
        vec![
            "*Model_Space",
            "*Paper_Space",
            "*Paper_Space0",
            "*D",
            "*D2",
            "*D1",
            "*D3",
            "DOOR"
        ],
        "each name its own; numbers in use skipped"
    );
}

#[test]
pub fn dynamic_block_named() {
    let apps: std::collections::HashMap<u64, String> = [
        (0x12, "ACAD".to_owned()),
        (0x13, "AcDbBlockRepBTag".to_owned()),
    ]
    .into();
    let eed = vec![
        Eed {
            app: 0x12,
            items: vec![XItem::Handle(1005, 0x99)],
        },
        Eed {
            app: 0x13,
            items: vec![XItem::Short(1), XItem::Handle(1005, 0x50)],
        },
    ];
    assert_eq!(
        dynamic_target(&eed, &apps),
        Some(0x50),
        "the dynamic block, by the application's own handle"
    );
    assert_eq!(
        dynamic_target(&eed[..1], &apps),
        None,
        "no tag, no dynamic block"
    );
}

#[test]
pub fn leader_style_overrides() {
    let apps: std::collections::HashMap<u64, String> =
        [(0x12, "ACAD".to_owned()), (0x14, "OTHER".to_owned())].into();
    let eed = vec![
        Eed {
            app: 0x14,
            items: vec![
                XItem::Str("DSTYLE".into()),
                XItem::Short(41),
                XItem::Real(1040, 9.0),
            ],
        },
        Eed {
            app: 0x12,
            items: vec![
                XItem::Str("DSTYLE".into()),
                XItem::Brace(true),
                XItem::Short(40),
                XItem::Real(1040, 2.0),
                XItem::Short(41),
                XItem::Real(1040, 0.5),
                XItem::Short(341),
                XItem::Handle(1005, 0x66),
                XItem::Brace(false),
            ],
        },
    ];
    let o = dim_overrides(&eed, &apps);
    assert_eq!(
        (o.scale, o.arrow_size, o.leader_arrow),
        (Some(2.0), Some(0.5), Some(0x66)),
        "the ACAD application's DSTYLE group: scale 40, arrow size 41, leader arrow 341"
    );
    assert_eq!(
        dim_overrides(&eed[..1], &apps).arrow_size,
        None,
        "another application's group is not the style's"
    );
}

#[test]
pub fn polyline_geometry() {
    use k2_mpc_dwg::polylines::{PolyKind, PolylineData, VertexData};
    let p = PolylineData {
        kind: PolyKind::TwoD,
        flags: 1 | 4,
        curve: 6,
        w0: 0.0,
        w1: 0.0,
        thickness: 0.0,
        elevation: 2.0,
        n: [0.0, 0.0, 1.0],
        owned: 3,
        vertices: vec![0x41, 0x42, 0x43],
        seqend: Some(0x44),
    };
    let vx = |flags: u8, x: f64| VertexData {
        flags,
        p: [x, 1.0, 0.0],
        w0: 0.5,
        w1: 0.5,
        bulge: 0.25,
        face: None,
    };
    // a spline-fit vertex (8) kept, a spline frame's control point (16) left out
    match poly_geom(&p, &[vx(8, 0.0), vx(16, 5.0), vx(8, 9.0)]) {
        Geom::Poly {
            verts,
            closed,
            world,
            ..
        } => {
            assert_eq!(verts.len(), 2, "the frame's control point left out");
            assert_eq!(verts[1].p, [9.0, 1.0, 2.0], "at the polyline's elevation");
            assert!(closed && !world, "closed, in its own frame");
            assert_eq!((verts[0].w0, verts[0].bulge), (0.5, 0.25), "widths, bulge");
        }
        g => panic!("not a polyline: {g:?}"),
    }
    let mesh = PolylineData {
        kind: PolyKind::Mesh,
        flags: 16,
        ..p
    };
    assert_eq!(
        poly_geom(&mesh, &[]),
        Geom::NotModelled,
        "a mesh not modelled"
    );
}

/// block_annotative_mark: the annotative mark in extension data, as the DXF read judges it:
/// the `AcadAnnotative` application's second short not 0 (on), 0 (off), or no such application.
#[test]
pub fn block_annotative_mark() {
    let apps: HashMap<u64, String> = [(0x40, "AcadAnnotative".to_owned()), (0x41, "ACAD".into())]
        .into_iter()
        .collect();
    let mark = |second: i16| {
        vec![Eed {
            app: 0x40,
            items: vec![
                XItem::Str("AnnotativeData".into()),
                XItem::Brace(true),
                XItem::Short(1),
                XItem::Short(second),
                XItem::Brace(false),
            ],
        }]
    };
    assert!(annotative_eed(&mark(1), &apps), "block_annotative_mark: on");
    assert!(
        !annotative_eed(&mark(0), &apps),
        "block_annotative_mark: off"
    );
    let other = vec![Eed {
        app: 0x41,
        items: vec![XItem::Short(1), XItem::Short(1)],
    }];
    assert!(
        !annotative_eed(&other, &apps),
        "block_annotative_mark: another application's data"
    );
}

/// attdef_infos: an attribute definition as a header states it: its tag (trimmed), its default
/// value, constant or not, its prompt; no other kind is one.
#[test]
pub fn attdef_infos() {
    let text = TextData {
        elevation: 0.0,
        ins: [0.0, 1.0],
        align: None,
        n: [0.0, 0.0, 1.0],
        thickness: 0.0,
        oblique: 0.0,
        rotation: 0.0,
        height: 0.25,
        width: 1.0,
        text: "D-00".into(),
        generation: 0,
        halign: 0,
        valign: 0,
        style: None,
    };
    let def = |flags: u8| {
        ent(Body::AttDef(AttData {
            text: text.clone(),
            tag: " NO ".into(),
            flags,
            prompt: Some("Door number".into()),
            multiline: None,
        }))
    };
    let got = attdef_info(&def(0)).expect("attdef_infos: an attribute definition");
    assert_eq!(
        (
            got.tag.as_str(),
            got.default.as_str(),
            got.constant,
            got.prompt.as_str()
        ),
        ("NO", "D-00", false, "Door number"),
        "attdef_infos: its tag, default, constant flag and prompt"
    );
    assert!(
        attdef_info(&def(2))
            .expect("attdef_infos: constant")
            .constant,
        "attdef_infos: flag 2 is constant"
    );
    assert_eq!(
        attdef_info(&ent(Body::EndBlk)),
        None,
        "attdef_infos: another kind"
    );
}

/// left_out_entries_counted: a table entry whose read refused is left out and counted in the
/// read report; one that reads is kept and not counted; one whose object doesn't open is
/// neither kept nor counted (the table's order already leaves it out).
#[test]
pub fn left_out_entries_counted() {
    let mut n = 0;
    let refused: Option<Result<u8, CadError>> = Some(Err(CadError::new(BAD_VALUE, "x", "y")));
    assert_eq!(
        count_left_out(refused, &mut n),
        None,
        "left_out_entries_counted: a refused entry is left out"
    );
    assert_eq!(n, 1, "left_out_entries_counted: and counted");
    assert_eq!(
        count_left_out(Some(Ok(7u8)), &mut n),
        Some(7),
        "left_out_entries_counted: an entry that reads is kept"
    );
    assert_eq!(
        count_left_out::<u8>(None, &mut n),
        None,
        "left_out_entries_counted: an object that doesn't open"
    );
    assert_eq!(n, 1, "left_out_entries_counted: neither counted");
}

/// fixture_read_report: the committed fixture's read report counts nothing left out or kept in
/// part, and nothing past the format, and its drawing has its layers and block records.
#[test]
pub fn fixture_read_report() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/draw_sampler.dwg");
    let bytes =
        std::fs::read(&p).unwrap_or_else(|e| panic!("fixture_read_report: {}: {e}", p.display()));
    let d = DwgDrawing::open(&bytes, &Limits::default())
        .unwrap_or_else(|e| panic!("fixture_read_report: refused: {}", e.summary()));
    assert_eq!(
        d.tolerated(),
        Tolerated::default(),
        "fixture_read_report: nothing left out, kept in part or past the format"
    );
    assert!(
        !d.doc().layers.is_empty() && !d.doc().blocks.is_empty(),
        "fixture_read_report: its layers and block records"
    );
}
