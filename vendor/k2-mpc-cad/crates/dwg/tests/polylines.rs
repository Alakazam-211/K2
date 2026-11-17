//! The old polylines and their vertices over hand-made bytes: one object at a time, its bit
//! fields packed in the row, in the 2004 and the 2010 layouts. Each row checks the fields its
//! read gives, and that its read ends exactly at its data's end.

mod support;

use k2_mpc_dwg::entities::{entity, Body, Entity};
use k2_mpc_dwg::frame::Ver;
use k2_mpc_dwg::polylines::{PolyKind, PolylineData, VertexData};
use support::object::{b, bd, bd3, bl, bs, ent, h, rc, Common, F};

fn read(v: Ver, ty: u16, handle: u64, data: F, own: F) -> Entity {
    let obj = ent(v, ty, handle, &Common::on(0x10), data, Vec::new(), own);
    entity(&obj.0, obj.1, v, &[]).unwrap_or_else(|e| panic!("{v:?}: refused: {}", e.summary()))
}

fn poly(e: Entity) -> PolylineData {
    match e.body {
        Body::Polyline(p) => p,
        other => panic!("not a polyline: {other:?}"),
    }
}

fn vert(e: Entity) -> VertexData {
    match e.body {
        Body::Vertex(v) => v,
        other => panic!("not a vertex: {other:?}"),
    }
}

#[test]
pub fn polylines_2d_and_their_vertices() {
    for v in [Ver::R2004, Ver::R2010, Ver::R2018] {
        // closed, default widths, its thickness 0, elevation 3, up, owning two vertices
        let d = [
            bs(1),
            bs(0),
            bd(0.5),
            bd(0.75),
            b(true),
            bd(3.0),
            b(true),
            bl(2),
        ]
        .concat();
        let own = [h(3, 0x41), h(3, 0x42), h(3, 0x43)].concat();
        let p = poly(read(v, 0x0F, 0x40, d, own));
        assert_eq!(
            (p.kind, p.flags, p.w0, p.w1, p.elevation, p.n),
            (PolyKind::TwoD, 1, 0.5, 0.75, 3.0, [0.0, 0.0, 1.0]),
            "{v:?}: a closed 2D polyline, its widths, elevation and frame"
        );
        assert_eq!(
            (p.vertices, p.seqend),
            (vec![0x41, 0x42], Some(0x43)),
            "{v:?}: its vertices, then its SEQEND"
        );
        // a vertex with equal widths (written once, negative), a bulge
        let mut d = [rc(0), bd3([1.0, 2.0, 0.0]), bd(-0.25), bd(0.5)].concat();
        if v >= Ver::R2010 {
            d.extend(bl(7));
        }
        d.extend(bd(0.0));
        let x = vert(read(v, 0x0A, 0x41, d, Vec::new()));
        assert_eq!(
            (x.p, x.w0, x.w1, x.bulge),
            ([1.0, 2.0, 0.0], 0.25, 0.25, 0.5),
            "{v:?}: a negative start width stands for both widths"
        );
        // a vertex with its own two widths
        let mut d = [rc(1), bd3([3.0, 2.0, 0.0]), bd(0.1), bd(0.2), bd(0.0)].concat();
        if v >= Ver::R2010 {
            d.extend(bl(8));
        }
        d.extend(bd(0.0));
        let x = vert(read(v, 0x0A, 0x42, d, Vec::new()));
        assert_eq!(
            (x.flags, x.w0, x.w1, x.bulge),
            (1, 0.1, 0.2, 0.0),
            "{v:?}: its flags and its two widths"
        );
    }
}

#[test]
pub fn polylines_3d_and_face_meshes() {
    let v = Ver::R2018;
    // splined (quadratic), closed, owning one vertex
    let p = poly(read(
        v,
        0x10,
        0x50,
        [rc(1), rc(1), bl(1)].concat(),
        [h(3, 0x51), h(3, 0x52)].concat(),
    ));
    assert_eq!(
        (p.kind, p.flags, p.curve, p.vertices.clone(), p.seqend),
        (PolyKind::ThreeD, 8 | 4 | 1, 5, vec![0x51], Some(0x52)),
        "3D (8), splined (4) as a quadratic (75 is 5), closed (1)"
    );
    let x = vert(read(
        v,
        0x0B,
        0x51,
        [rc(32), bd3([1.0, 2.0, 3.0])].concat(),
        Vec::new(),
    ));
    assert_eq!((x.flags, x.p), (32, [1.0, 2.0, 3.0]), "a 3D vertex");
    // a face mesh: its counts, then two vertices and a face record it owns
    let p = poly(read(
        v,
        0x1D,
        0x60,
        [bs(2), bs(1), bl(3)].concat(),
        [h(3, 0x61), h(3, 0x62), h(3, 0x63), h(3, 0x64)].concat(),
    ));
    assert_eq!(
        (p.kind, p.flags, p.vertices.len()),
        (PolyKind::FaceMesh, 64, 3),
        "a face mesh (64), three owned"
    );
    let x = vert(read(
        v,
        0x0E,
        0x63,
        [bs(1), bs(2), bs(3), bs(0)].concat(),
        Vec::new(),
    ));
    assert_eq!(x.face, Some([1, 2, 3, 0]), "a face record's vertex indices");
}
