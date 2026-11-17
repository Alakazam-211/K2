//! The old polylines (`POLYLINE_2D`, `POLYLINE_3D`, `POLYLINE_PFACE`, `POLYLINE_MESH`) and their
//! vertices: a polyline's own fields and its vertices' handles (each vertex an object it owns,
//! then its SEQEND), and each vertex's fields.
//!
//! A polyline's flags are given as DXF has them (its `70`): a 3D polyline's closed and splined
//! bits are mapped onto the DXF bits (8 for 3D, 1 closed, 4 splined), a face mesh is 64, a
//! mesh 16 with its own flags.
//!
//! From the specification's sections on the polyline and vertex entities, as facts; our own
//! code.

use k2_mpc_cad_model::error::CadError;

use crate::entities::bad;
use crate::frame::{Obj, Ver};
use crate::mutant;

/// A polyline's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolyKind {
    TwoD,
    ThreeD,
    FaceMesh,
    Mesh,
}

/// A polyline's fields and its vertices' handles.
#[derive(Debug, Clone, PartialEq)]
pub struct PolylineData {
    pub kind: PolyKind,
    /// Its flags as DXF has them (`70`).
    pub flags: u16,
    /// Its curve and smooth surface type (`75`).
    pub curve: u16,
    /// Default start and end widths (2D).
    pub w0: f64,
    pub w1: f64,
    pub thickness: f64,
    /// Its elevation (2D).
    pub elevation: f64,
    pub n: [f64; 3],
    /// How many objects it owns (its vertices).
    pub owned: u32,
    /// Its vertices' handles, in order, and its SEQEND's.
    pub vertices: Vec<u64>,
    pub seqend: Option<u64>,
}

/// A vertex's fields.
#[derive(Debug, Clone, PartialEq)]
pub struct VertexData {
    /// Its flags as DXF has them (`70`).
    pub flags: u8,
    pub p: [f64; 3],
    /// Start and end width, bulge (a 2D vertex's; 0 otherwise).
    pub w0: f64,
    pub w1: f64,
    pub bulge: f64,
    /// A face record's vertex indices (a face mesh's face vertex).
    pub face: Option<[i16; 4]>,
}

/// A polyline's fields, from its first field to its data's end (its handles after, by
/// [`polyline_handles`]).
pub(crate) fn polyline(o: &mut Obj<'_>, kind: PolyKind) -> Result<PolylineData, CadError> {
    let mut p = PolylineData {
        kind,
        flags: 0,
        curve: 0,
        w0: 0.0,
        w1: 0.0,
        thickness: 0.0,
        elevation: 0.0,
        n: [0.0, 0.0, 1.0],
        owned: 0,
        vertices: Vec::new(),
        seqend: None,
    };
    match kind {
        PolyKind::TwoD => {
            p.flags = o.d.bs()?;
            p.curve = o.d.bs()?;
            p.w0 = o.d.bd()?;
            p.w1 = o.d.bd()?;
            p.thickness = o.d.bt()?;
            p.elevation = o.d.bd()?;
            p.n = o.d.be()?;
        }
        PolyKind::ThreeD => {
            let splined = o.d.rc()?;
            let closed = o.d.rc()?;
            p.flags = 8;
            if closed & 1 != 0 {
                p.flags |= 1;
            }
            if splined & 3 != 0 {
                p.flags |= 4;
                p.curve = if splined & 1 != 0 { 5 } else { 6 };
            }
        }
        PolyKind::FaceMesh => {
            // its vertex and face counts
            o.d.bs()?;
            o.d.bs()?;
            p.flags = 64;
        }
        PolyKind::Mesh => {
            p.flags = o.d.bs()? | 16;
            p.curve = o.d.bs()?;
            // its M and N vertex counts and densities
            for _ in 0..4 {
                o.d.bs()?;
            }
        }
    }
    p.owned = o.d.bl()?;
    Ok(p)
}

/// A polyline's handles after the entity's common ones: each vertex it owns, then its SEQEND.
pub(crate) fn polyline_handles(o: &mut Obj<'_>, p: &mut PolylineData) -> Result<(), CadError> {
    let owned = if mutant::active("PolylineOwnedIgnored") {
        0
    } else {
        p.owned as usize
    };
    if owned.saturating_mul(8) > o.hs.left() {
        return Err(bad(
            "more vertices than the handle stream holds",
            o.hs.pos() / 8,
        ));
    }
    for _ in 0..owned {
        if let Some(h) = o.h()? {
            p.vertices.push(h);
        }
    }
    p.seqend = o.h()?;
    Ok(())
}

/// A vertex's fields (`two_d`: a 2D polyline's vertex; `face`: a face mesh's face record).
pub(crate) fn vertex(o: &mut Obj<'_>, two_d: bool, face: bool) -> Result<VertexData, CadError> {
    if face {
        let mut idx = [0i16; 4];
        for i in &mut idx {
            *i = o.d.bs()? as i16;
        }
        return Ok(VertexData {
            flags: 128,
            p: [0.0; 3],
            w0: 0.0,
            w1: 0.0,
            bulge: 0.0,
            face: Some(idx),
        });
    }
    let flags = o.d.rc()?;
    let p = o.d.bd3()?;
    let (mut w0, mut w1, mut bulge) = (0.0, 0.0, 0.0);
    if two_d {
        w0 = o.d.bd()?;
        // a negative start width stands for equal start and end widths, the end width not
        // written
        if w0 < 0.0 && !mutant::active("VertexWidthSignKept") {
            w0 = -w0;
            w1 = w0;
        } else {
            w1 = o.d.bd()?;
        }
        bulge = o.d.bd()?;
        if o.ver >= Ver::R2010 {
            // its vertex id
            o.d.bl()?;
        }
        // its tangent direction
        o.d.bd()?;
    }
    Ok(VertexData {
        flags,
        p,
        w0,
        w1,
        bulge,
        face: None,
    })
}
