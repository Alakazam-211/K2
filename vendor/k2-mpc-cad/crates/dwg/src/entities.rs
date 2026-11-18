//! Entities: each kind's name by its type, and the fields of the kinds this reader decodes
//! (LINE, ARC, CIRCLE, LWPOLYLINE, TEXT, MTEXT, INSERT and MINSERT with their ATTRIBs and
//! SEQEND, ATTDEF, BLOCK and ENDBLK, POINT, ELLIPSE, SOLID, 3DFACE, VIEWPORT, HATCH, the seven
//! DIMENSION kinds, SPLINE, RAY, XLINE, LEADER and MULTILEADER ([`crate::leaders`]), the old
//! polylines and their vertices ([`crate::polylines`]), IMAGE and WIPEOUT). Every other entity
//! is read for its common data only ([`Body::Other`]).
//!
//! A kind's read must end exactly at its data's end ([`Obj::finish`]): a field out of place
//! refuses the entity instead of passing on wrong values.
//!
//! From the specification's chapter on object prescriptions (the entity sections and the type
//! numbers), as facts; our own code and table.

use k2_mpc_cad_model::error::CadError;

use crate::classes::Class;
use crate::error::BAD_VALUE;
use crate::frame::{entity_common, open, Eed, EntCommon, Obj, Ver};
use crate::leaders::{self, LeaderData, MleaderData};
use crate::mutant;
use crate::polylines::{self, PolyKind, PolylineData, VertexData};

const NEXT: &str = "the file is damaged: open a copy the app that made it saves again";

/// A fixed type's name and whether it is an entity, by type number. Our own table, in the
/// names DXF files use (the vertices and polylines by kind, `MINSERT` as itself).
pub fn fixed_kind(t: u16) -> Option<(&'static str, bool)> {
    Some(match t {
        0x01 => ("TEXT", true),
        0x02 => ("ATTRIB", true),
        0x03 => ("ATTDEF", true),
        0x04 => ("BLOCK", true),
        0x05 => ("ENDBLK", true),
        0x06 => ("SEQEND", true),
        0x07 => ("INSERT", true),
        0x08 => ("MINSERT", true),
        0x0A => ("VERTEX_2D", true),
        0x0B => ("VERTEX_3D", true),
        0x0C => ("VERTEX_MESH", true),
        0x0D => ("VERTEX_PFACE", true),
        0x0E => ("VERTEX_PFACE_FACE", true),
        0x0F => ("POLYLINE_2D", true),
        0x10 => ("POLYLINE_3D", true),
        0x11 => ("ARC", true),
        0x12 => ("CIRCLE", true),
        0x13 => ("LINE", true),
        0x14 => ("DIMENSION_ORDINATE", true),
        0x15 => ("DIMENSION_LINEAR", true),
        0x16 => ("DIMENSION_ALIGNED", true),
        0x17 => ("DIMENSION_ANG3PT", true),
        0x18 => ("DIMENSION_ANG2LN", true),
        0x19 => ("DIMENSION_RADIUS", true),
        0x1A => ("DIMENSION_DIAMETER", true),
        0x1B => ("POINT", true),
        0x1C => ("3DFACE", true),
        0x1D => ("POLYLINE_PFACE", true),
        0x1E => ("POLYLINE_MESH", true),
        0x1F => ("SOLID", true),
        0x20 => ("TRACE", true),
        0x21 => ("SHAPE", true),
        0x22 => ("VIEWPORT", true),
        0x23 => ("ELLIPSE", true),
        0x24 => ("SPLINE", true),
        0x25 => ("REGION", true),
        0x26 => ("3DSOLID", true),
        0x27 => ("BODY", true),
        0x28 => ("RAY", true),
        0x29 => ("XLINE", true),
        0x2A => ("DICTIONARY", false),
        0x2B => ("OLEFRAME", true),
        0x2C => ("MTEXT", true),
        0x2D => ("LEADER", true),
        0x2E => ("TOLERANCE", true),
        0x2F => ("MLINE", true),
        0x30 => ("BLOCK_CONTROL", false),
        0x31 => ("BLOCK_HEADER", false),
        0x32 => ("LAYER_CONTROL", false),
        0x33 => ("LAYER", false),
        0x34 => ("STYLE_CONTROL", false),
        0x35 => ("STYLE", false),
        0x38 => ("LTYPE_CONTROL", false),
        0x39 => ("LTYPE", false),
        0x3C => ("VIEW_CONTROL", false),
        0x3D => ("VIEW", false),
        0x3E => ("UCS_CONTROL", false),
        0x3F => ("UCS", false),
        0x40 => ("VPORT_CONTROL", false),
        0x41 => ("VPORT", false),
        0x42 => ("APPID_CONTROL", false),
        0x43 => ("APPID", false),
        0x44 => ("DIMSTYLE_CONTROL", false),
        0x45 => ("DIMSTYLE", false),
        0x46 => ("VX_CONTROL", false),
        0x47 => ("VX_TABLE_RECORD", false),
        0x48 => ("GROUP", false),
        0x49 => ("MLINESTYLE", false),
        0x4A => ("OLE2FRAME", true),
        0x4C => ("LONG_TRANSACTION", false),
        0x4D => ("LWPOLYLINE", true),
        0x4E => ("HATCH", true),
        0x4F => ("XRECORD", false),
        0x50 => ("ACDBPLACEHOLDER", false),
        0x51 => ("VBA_PROJECT", false),
        0x52 => ("LAYOUT", false),
        0x1F2 => ("ACAD_PROXY_ENTITY", true),
        0x1F3 => ("ACAD_PROXY_OBJECT", false),
        _ => return None,
    })
}

/// A type's name and whether it is an entity: the fixed table, else the class list (from 500;
/// an item class id of 0x1F2 is an entity). `None` for a type neither names.
pub fn kind_of(t: u16, classes: &[Class]) -> Option<(String, bool)> {
    if let Some((n, e)) = fixed_kind(t) {
        return Some((n.to_owned(), e));
    }
    let c = classes.iter().find(|c| c.number == t)?;
    Some((c.dxf_name.clone(), c.item_class_id == 0x1F2))
}

/// The text fields TEXT, ATTRIB and ATTDEF share.
#[derive(Debug, Clone, PartialEq)]
pub struct TextData {
    pub elevation: f64,
    pub ins: [f64; 2],
    /// The alignment point, when written.
    pub align: Option<[f64; 2]>,
    pub n: [f64; 3],
    pub thickness: f64,
    /// Radians.
    pub oblique: f64,
    /// Radians.
    pub rotation: f64,
    pub height: f64,
    pub width: f64,
    pub text: String,
    pub generation: u16,
    pub halign: u16,
    pub valign: u16,
    pub style: Option<u64>,
}

/// An attribute's own fields beside its text.
#[derive(Debug, Clone, PartialEq)]
pub struct AttData {
    pub text: TextData,
    pub tag: String,
    pub flags: u8,
    /// An attribute definition's prompt.
    pub prompt: Option<String>,
    /// A 2018 multi-line attribute's multiline text, embedded.
    pub multiline: Option<Box<MTextData>>,
}

/// A multiline text's fields.
#[derive(Debug, Clone, PartialEq)]
pub struct MTextData {
    pub ins: [f64; 3],
    pub n: [f64; 3],
    pub xdir: [f64; 3],
    pub rect_width: f64,
    pub height: f64,
    pub attach: u16,
    pub direction: u16,
    pub text: String,
    pub spacing_style: u16,
    pub spacing: f64,
    /// Background flags, scale, colour index, true colour.
    pub background: Option<(u32, f64, u16, Option<u32>)>,
    /// The 2018 column data: type, count, width, gutter, automatic height, flow reversed, and
    /// each column's height (stored for dynamic columns of manual height only).
    pub columns: Option<MTextColumns>,
    /// 2018: the redundant fields of a text that is not annotative are present, and with them
    /// a handle (its application) after its style.
    pub redundant: bool,
    pub style: Option<u64>,
}

/// A multiline text's 2018 column data.
#[derive(Debug, Clone, PartialEq)]
pub struct MTextColumns {
    pub kind: u16,
    pub count: u32,
    pub width: f64,
    pub gutter: f64,
    pub auto_height: bool,
    pub reversed: bool,
    pub heights: Vec<f64>,
}

/// A block reference's fields.
#[derive(Debug, Clone, PartialEq)]
pub struct InsertData {
    pub ins: [f64; 3],
    pub scale: [f64; 3],
    /// Radians.
    pub rotation: f64,
    pub n: [f64; 3],
    /// Columns, rows, column spacing, row spacing (MINSERT).
    pub array: Option<(u16, u16, f64, f64)>,
    pub block: Option<u64>,
    /// It has attributes (and a SEQEND), and how many objects it owns.
    pub has_attribs: bool,
    pub owned: u32,
    /// Its attributes' handles, in order.
    pub attribs: Vec<u64>,
    pub seqend: Option<u64>,
}

/// A light-weight polyline's fields.
#[derive(Debug, Clone, PartialEq)]
pub struct LwData {
    pub flags: u16,
    pub const_width: f64,
    pub elevation: f64,
    pub thickness: f64,
    pub n: [f64; 3],
    pub points: Vec<[f64; 2]>,
    pub bulges: Vec<f64>,
    pub widths: Vec<[f64; 2]>,
}

impl LwData {
    /// Closed (flag 0x200).
    pub fn closed(&self) -> bool {
        self.flags & 0x200 != 0
    }
}

/// A spline's fields.
#[derive(Debug, Clone, PartialEq)]
pub struct SplineData {
    pub degree: u32,
    /// As DXF codes them: 1 closed, 2 periodic, 4 rational.
    pub flags: u32,
    pub knots: Vec<f64>,
    pub ctrl: Vec<[f64; 3]>,
    pub weights: Vec<f64>,
    pub fit: Vec<[f64; 3]>,
}

/// A viewport's fields.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewportData {
    pub centre: [f64; 3],
    pub size: [f64; 2],
    pub target: [f64; 3],
    pub direction: [f64; 3],
    /// Radians.
    pub twist: f64,
    pub view_height: f64,
    pub view_centre: [f64; 2],
    /// 0x20000: on; 0x10000: clipped by an entity.
    pub status: u32,
    pub frozen_count: u32,
    pub frozen: Vec<u64>,
    pub clip: Option<u64>,
}

/// One edge of a hatch boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum HatchEdge {
    Line([f64; 2], [f64; 2]),
    /// Centre, radius, start and end angles (radians), counter-clockwise.
    Arc([f64; 2], f64, f64, f64, bool),
    /// Centre, major axis end (relative), ratio, start and end angles (radians),
    /// counter-clockwise.
    Ellipse([f64; 2], [f64; 2], f64, f64, f64, bool),
    Spline {
        degree: u32,
        knots: Vec<f64>,
        ctrl: Vec<[f64; 2]>,
        weights: Vec<f64>,
        fit: Vec<[f64; 2]>,
    },
}

/// A polyline path: its points with their bulges, and whether it is closed.
pub type PolyPath = (Vec<([f64; 2], f64)>, bool);

/// One hatch boundary path.
#[derive(Debug, Clone, PartialEq)]
pub struct HatchPath {
    pub flags: u32,
    pub edges: Vec<HatchEdge>,
    /// A polyline path: its points with their bulges, and whether it is closed.
    pub poly: Option<PolyPath>,
    pub sources: u32,
}

/// One pattern definition line.
#[derive(Debug, Clone, PartialEq)]
pub struct HatchLine {
    /// Radians.
    pub angle: f64,
    pub base: [f64; 2],
    pub offset: [f64; 2],
    pub dashes: Vec<f64>,
}

/// A hatch's fields.
#[derive(Debug, Clone, PartialEq)]
pub struct HatchData {
    /// A gradient: its name, angle (radians), shift, one-colour flag, and its colours.
    pub gradient: Option<(String, f64, f64, bool, Vec<u32>)>,
    pub elevation: f64,
    pub n: [f64; 3],
    pub name: String,
    pub solid: bool,
    pub paths: Vec<HatchPath>,
    pub style: u16,
    /// Radians.
    pub angle: f64,
    pub scale: f64,
    pub double: bool,
    pub lines: Vec<HatchLine>,
    pub sources: Vec<u64>,
}

/// An image's or a wipeout's fields: where it sits, its pixel axes and size, its clip
/// boundary (pixel coordinates), and its image definition (an image's).
#[derive(Debug, Clone, PartialEq)]
pub struct ImageData {
    pub ins: [f64; 3],
    pub u: [f64; 3],
    pub v: [f64; 3],
    pub size: [f64; 2],
    pub display: u16,
    pub clipping: bool,
    /// 1 a rectangle (two corners), 2 a polygon.
    pub clip_type: u16,
    pub clip: Vec<[f64; 2]>,
    pub def: Option<u64>,
}

fn image(o: &mut Obj<'_>) -> Result<ImageData, CadError> {
    // its class version
    o.d.bl()?;
    let ins = o.d.bd3()?;
    let u = o.d.bd3()?;
    let v = o.d.bd3()?;
    let size = o.d.rd2()?;
    let display = o.d.bs()?;
    let clipping = o.d.b()?;
    // brightness, contrast, fade
    o.d.rc()?;
    o.d.rc()?;
    o.d.rc()?;
    if o.ver >= Ver::R2010 {
        // clip inside or outside
        o.d.b()?;
    }
    let clip_type = o.d.bs()?;
    let mut clip = Vec::new();
    if clip_type == 1 && !mutant::active("ImageRectAsPolygon") {
        clip.push(o.d.rd2()?);
        clip.push(o.d.rd2()?);
    } else {
        let n = o.d.bl()?;
        for _ in 0..counted(o, n, 128)? {
            clip.push(o.d.rd2()?);
        }
    }
    Ok(ImageData {
        ins,
        u,
        v,
        size,
        display,
        clipping,
        clip_type,
        clip,
        def: None,
    })
}

/// A dimension's fields: the ones every kind has, and its defining points by their DXF codes.
#[derive(Debug, Clone, PartialEq)]
pub struct DimData {
    /// The DXF dimension type (0 rotated, 1 aligned, 2 angular 2-line, 3 diameter, 4 radius,
    /// 5 angular 3-point, 6 ordinate) with its flags (32, 64, 128).
    pub dxf_type: i64,
    pub n: [f64; 3],
    pub text_mid: [f64; 3],
    pub text: String,
    pub measured: f64,
    /// Radians.
    pub text_rotation: f64,
    pub defs: Vec<(i32, [f64; 3])>,
    pub style: Option<u64>,
    pub block: Option<u64>,
}

/// A kind's own fields.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Line {
        p1: [f64; 3],
        p2: [f64; 3],
        thickness: f64,
        n: [f64; 3],
    },
    Circle {
        c: [f64; 3],
        r: f64,
        thickness: f64,
        n: [f64; 3],
    },
    /// Angles in radians.
    Arc {
        c: [f64; 3],
        r: f64,
        thickness: f64,
        n: [f64; 3],
        a0: f64,
        a1: f64,
    },
    Point {
        p: [f64; 3],
        thickness: f64,
        n: [f64; 3],
    },
    Ellipse {
        c: [f64; 3],
        major: [f64; 3],
        n: [f64; 3],
        ratio: f64,
        p0: f64,
        p1: f64,
    },
    /// A SOLID or TRACE: its corners in its frame (the elevation as z).
    Solid {
        pts: [[f64; 3]; 4],
        thickness: f64,
        n: [f64; 3],
    },
    Face {
        pts: [[f64; 3]; 4],
        hidden: u16,
    },
    LwPolyline(LwData),
    Text(TextData),
    Attrib(AttData),
    AttDef(AttData),
    MText(MTextData),
    Insert(InsertData),
    Block {
        name: String,
    },
    EndBlk,
    Seqend,
    Viewport(ViewportData),
    Spline(SplineData),
    /// A RAY (`ray`) or an XLINE: a point and a direction.
    XLine {
        point: [f64; 3],
        dir: [f64; 3],
        ray: bool,
    },
    Hatch(Box<HatchData>),
    Dimension(DimData),
    /// A multileader (`MULTILEADER`), its content in the drawing model's leader type.
    Mleader(Box<MleaderData>),
    Leader(LeaderData),
    /// An image or (`wipeout`) a wipeout.
    Image {
        data: Box<ImageData>,
        wipeout: bool,
    },
    /// An old polyline (2D, 3D, face mesh, mesh) with its vertices' handles.
    Polyline(PolylineData),
    /// A polyline's vertex.
    Vertex(VertexData),
    /// A kind read for its common data only.
    Other,
}

/// One entity: its handle, kind, extension data, common data and its kind's fields.
#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    pub handle: u64,
    pub kind: String,
    pub type_number: u16,
    pub eed: Vec<Eed>,
    pub common: EntCommon,
    pub body: Body,
    /// The bits its handle stream holds past every handle the format names for its kind,
    /// beyond the last byte's padding: data another writer added, tolerated (0 for a kind read
    /// for its common data only).
    pub extra_handle_bits: usize,
}

pub(crate) fn bad(why: impl Into<String>, at: usize) -> CadError {
    CadError::new(BAD_VALUE, why, NEXT).at(at)
}

/// The text fields TEXT, ATTRIB and ATTDEF share (2004 and later), with the style handle.
fn text_data(o: &mut Obj<'_>) -> Result<TextData, CadError> {
    let f = o.d.rc()?;
    let elevation = if f & 0x01 == 0 { o.d.rd()? } else { 0.0 };
    let ins = o.d.rd2()?;
    let align = if f & 0x02 == 0 {
        Some([o.d.dd(ins[0])?, o.d.dd(ins[1])?])
    } else {
        None
    };
    let n = o.d.be()?;
    let thickness = o.d.bt()?;
    let oblique = if f & 0x04 == 0 { o.d.rd()? } else { 0.0 };
    let rotation = if f & 0x08 == 0 { o.d.rd()? } else { 0.0 };
    let height = o.d.rd()?;
    let width = if f & 0x10 == 0 { o.d.rd()? } else { 1.0 };
    let text = o.tv()?;
    let generation = if f & 0x20 == 0 { o.d.bs()? } else { 0 };
    let halign = if f & 0x40 == 0 { o.d.bs()? } else { 0 };
    let valign = if f & 0x80 == 0 { o.d.bs()? } else { 0 };
    Ok(TextData {
        elevation,
        ins,
        align,
        n,
        thickness,
        oblique,
        rotation,
        height,
        width,
        text,
        generation,
        halign,
        valign,
        style: None,
    })
}

/// A 2018 multi-line attribute's embedded multiline text: the data an entity begins with, from
/// its entity mode (no graphic flag; its handles not read here), then a multiline text's fields.
fn embedded_mtext(o: &mut Obj<'_>) -> Result<MTextData, CadError> {
    // entity mode, reactor count, extension dictionary and data flags
    o.d.bb()?;
    o.d.bl()?;
    o.d.b()?;
    o.d.b()?;
    // colour, linetype scale, linetype, plot style and material flags, shadow, visual styles,
    // invisibility, lineweight
    o.d.cmc()?;
    o.d.bd()?;
    for _ in 0..3 {
        o.d.bb()?;
    }
    o.d.rc()?;
    for _ in 0..3 {
        o.d.b()?;
    }
    o.d.bs()?;
    o.d.rc()?;
    mtext(o)
}

/// An attribute's fields after its text (2004 and later), a 2018 multi-line attribute's
/// embedded multiline text with them.
fn att_data(o: &mut Obj<'_>, def: bool) -> Result<AttData, CadError> {
    let text = text_data(o)?;
    if o.ver >= Ver::R2010 {
        o.d.rc()?;
    }
    let mut multiline = None;
    if o.ver >= Ver::R2018 {
        let t = o.d.rc()?;
        if t > 1 && !mutant::active("MultilineEmbeddedSkipped") {
            // the specification gives the embedded fields "starting from the entity mode";
            // the 2018 sample's two multi-line attributes end exactly with them
            multiline = Some(Box::new(embedded_mtext(o)?));
            // its annotative data: a byte count, the bytes, then a short (its application in
            // the handle stream)
            let n = o.d.bs()?;
            if n > 0 {
                if usize::from(n).saturating_mul(8) > o.d.left() {
                    return Err(bad(
                        "more annotative data than the data holds",
                        o.d.pos() / 8,
                    ));
                }
                for _ in 0..n {
                    o.d.rc()?;
                }
                o.d.bs()?;
            }
        }
    }
    let tag = o.tv()?;
    o.d.bs()?;
    let flags = o.d.rc()?;
    if o.ver.strings() {
        o.d.b()?;
    }
    let prompt = if def {
        if o.ver >= Ver::R2010 {
            o.d.rc()?;
        }
        Some(o.tv()?)
    } else {
        None
    };
    Ok(AttData {
        text,
        tag,
        flags,
        prompt,
        multiline,
    })
}

fn mtext(o: &mut Obj<'_>) -> Result<MTextData, CadError> {
    let ins = o.d.bd3()?;
    let n = o.d.bd3()?;
    let xdir = o.d.bd3()?;
    let rect_width = o.d.bd()?;
    if o.ver.strings() {
        o.d.bd()?;
    }
    let height = o.d.bd()?;
    let attach = o.d.bs()?;
    let direction = o.d.bs()?;
    o.d.bd()?;
    o.d.bd()?;
    let text = o.tv()?;
    let spacing_style = o.d.bs()?;
    let spacing = o.d.bd()?;
    o.d.b()?;
    let bg = o.d.bl()?;
    let background = if bg & 0x01 != 0 || (o.ver >= Ver::R2018 && bg & 0x10 != 0) {
        let scale = o.d.bd()?;
        let c = o.cmc()?;
        o.d.bl()?;
        Some((bg, scale, c.index, c.rgb))
    } else {
        None
    };
    let mut columns = None;
    let mut redundant = false;
    // 2018: a flag set when the text is not annotative, then its redundant fields and columns
    if o.ver >= Ver::R2018 && (o.d.b()? != mutant::active("MTextAnnotativeInverted")) {
        redundant = true;
        o.d.bs()?;
        o.d.b()?;
        o.d.bl()?;
        o.d.bd3()?;
        o.d.bd3()?;
        for _ in 0..4 {
            o.d.bd()?;
        }
        let kind = o.d.bs()?;
        if kind != 0 {
            let count = o.d.bl()?;
            let width = o.d.bd()?;
            let gutter = o.d.bd()?;
            let auto = o.d.b()?;
            let reversed = o.d.b()?;
            let mut heights = Vec::new();
            if !auto && kind == 2 {
                if (count as usize).saturating_mul(2) > o.d.left() {
                    return Err(bad(
                        "more column heights than the data holds",
                        o.d.pos() / 8,
                    ));
                }
                for _ in 0..count {
                    // a column whose height isn't set: the 2018 samples store minus the text's
                    // extent there, where the program's own DXF save of the same drawing writes
                    // 0; read as 0
                    let h = o.d.bd()?;
                    heights.push(if h < 0.0 && !mutant::active("MTextColumnHeightRaw") {
                        0.0
                    } else {
                        h
                    });
                }
            }
            columns = Some(MTextColumns {
                kind,
                count,
                width,
                gutter,
                auto_height: auto,
                reversed,
                heights,
            });
        }
    }
    Ok(MTextData {
        ins,
        n,
        xdir,
        rect_width,
        height,
        attach,
        direction,
        text,
        spacing_style,
        spacing,
        background,
        columns,
        redundant,
        style: None,
    })
}

fn insert(o: &mut Obj<'_>, minsert: bool) -> Result<InsertData, CadError> {
    let ins = o.d.bd3()?;
    let scale = match o.d.bb()? {
        3 => [1.0; 3],
        1 => {
            let y = o.d.dd(1.0)?;
            [1.0, y, o.d.dd(1.0)?]
        }
        2 => {
            let x = o.d.rd()?;
            [x; 3]
        }
        _ => {
            let x = o.d.rd()?;
            let y = o.d.dd(x)?;
            [x, y, o.d.dd(x)?]
        }
    };
    let rotation = o.d.bd()?;
    let n = o.d.bd3()?;
    let has_attribs = o.d.b()?;
    let owned = if has_attribs { o.d.bl()? } else { 0 };
    let array = if minsert {
        Some((o.d.bs()?, o.d.bs()?, o.d.bd()?, o.d.bd()?))
    } else {
        None
    };
    Ok(InsertData {
        ins,
        scale,
        rotation,
        n,
        array,
        block: None,
        has_attribs,
        owned,
        attribs: Vec::new(),
        seqend: None,
    })
}

fn lwpolyline(o: &mut Obj<'_>) -> Result<LwData, CadError> {
    let flags = o.d.bs()?;
    let const_width = if flags & 4 != 0 { o.d.bd()? } else { 0.0 };
    let elevation = if flags & 8 != 0 { o.d.bd()? } else { 0.0 };
    let thickness = if flags & 2 != 0 { o.d.bd()? } else { 0.0 };
    let n = if flags & 1 != 0 {
        o.d.bd3()?
    } else {
        [0.0, 0.0, 1.0]
    };
    let count = o.d.bl()? as usize;
    let nb = if flags & 16 != 0 {
        o.d.bl()? as usize
    } else {
        0
    };
    let ni = if o.ver >= Ver::R2010 && flags & 1024 != 0 {
        o.d.bl()? as usize
    } else {
        0
    };
    let nw = if flags & 32 != 0 {
        o.d.bl()? as usize
    } else {
        0
    };
    // each point 4 bits at least, each bulge 2, each id 2, each width 4: a count the data can't
    // hold is refused before anything is allocated
    let need = count
        .saturating_mul(4)
        .saturating_add(nb.saturating_mul(2))
        .saturating_add(ni.saturating_mul(2))
        .saturating_add(nw.saturating_mul(4));
    if need > o.d.left() && !mutant::active("LwCountsTrusted") {
        return Err(bad("more vertices than the data holds", o.d.pos() / 8));
    }
    let mut points = Vec::with_capacity(count);
    for k in 0..count {
        let p = if k == 0 {
            o.d.rd2()?
        } else {
            let prev: [f64; 2] = if mutant::active("LwDdFromFirst") {
                points[0]
            } else {
                points[k - 1]
            };
            [o.d.dd(prev[0])?, o.d.dd(prev[1])?]
        };
        points.push(p);
    }
    let mut bulges = Vec::with_capacity(nb);
    for _ in 0..nb {
        bulges.push(o.d.bd()?);
    }
    for _ in 0..ni {
        o.d.bl()?;
    }
    let mut widths = Vec::with_capacity(nw);
    for _ in 0..nw {
        widths.push(o.d.bd2()?);
    }
    Ok(LwData {
        flags,
        const_width,
        elevation,
        thickness,
        n,
        points,
        bulges,
        widths,
    })
}

fn viewport(o: &mut Obj<'_>) -> Result<ViewportData, CadError> {
    let centre = o.d.bd3()?;
    let size = o.d.bd2()?;
    let target = o.d.bd3()?;
    let direction = o.d.bd3()?;
    let twist = o.d.bd()?;
    let view_height = o.d.bd()?;
    for _ in 0..4 {
        o.d.bd()?;
    }
    let view_centre = o.d.rd2()?;
    for _ in 0..3 {
        o.d.rd2()?;
    }
    o.d.bs()?;
    if o.ver.strings() {
        o.d.bs()?;
    }
    let frozen_count = o.d.bl()?;
    let status = o.d.bl()?;
    o.tv()?;
    o.d.rc()?;
    o.d.b()?;
    o.d.b()?;
    for _ in 0..3 {
        o.d.bd3()?;
    }
    o.d.bd()?;
    o.d.bs()?;
    o.d.bs()?;
    if o.ver.strings() {
        o.d.b()?;
        o.d.rc()?;
        o.d.bd()?;
        o.d.bd()?;
        o.cmc()?;
    }
    Ok(ViewportData {
        centre,
        size,
        target,
        direction,
        twist,
        view_height,
        view_centre,
        status,
        frozen_count,
        frozen: Vec::new(),
        clip: None,
    })
}

/// A count the data can't hold at `bits` each is refused before anything is allocated.
pub(crate) fn counted(o: &Obj<'_>, n: u32, bits: usize) -> Result<usize, CadError> {
    let n = n as usize;
    if n.saturating_mul(bits) > o.d.left() && !mutant::active("HatchCountsTrusted") {
        return Err(bad("a count larger than the data holds", o.d.pos() / 8));
    }
    Ok(n)
}

fn spline(o: &mut Obj<'_>) -> Result<SplineData, CadError> {
    let mut scenario = o.d.bl()?;
    let mut flags = 0u32;
    if o.ver >= Ver::R2013 {
        let f1 = o.d.bl()?;
        let knot_param = o.d.bl()?;
        if f1 & 4 != 0 {
            flags |= 1;
        }
        // from 2013 the scenario follows from these: control points and knots when the knot
        // parameter is custom (15) or there are no fit points (flag 1 clear), else fit points
        if !mutant::active("SplineScenarioStored") {
            scenario = if knot_param == 15 || f1 & 1 == 0 {
                1
            } else {
                2
            };
        }
    }
    let degree = o.d.bl()?;
    let (mut nk, mut nc, mut nf, mut weighted) = (0usize, 0usize, 0usize, false);
    if scenario == 2 {
        o.d.bd()?;
        o.d.bd3()?;
        o.d.bd3()?;
        let n = o.d.bl()?;
        nf = counted(o, n, 6)?;
    } else if scenario == 1 {
        let rational = o.d.b()?;
        let closed = o.d.b()?;
        let periodic = o.d.b()?;
        flags = u32::from(closed) | u32::from(periodic) << 1 | u32::from(rational) << 2;
        o.d.bd()?;
        o.d.bd()?;
        let k = o.d.bl()?;
        nk = counted(o, k, 2)?;
        let c = o.d.bl()?;
        nc = counted(o, c, 6)?;
        weighted = o.d.b()?;
    } else {
        return Err(bad(
            format!("a spline of scenario {scenario}"),
            o.d.pos() / 8,
        ));
    }
    let mut knots = Vec::with_capacity(nk);
    for _ in 0..nk {
        knots.push(o.d.bd()?);
    }
    let mut ctrl = Vec::with_capacity(nc);
    let mut weights = Vec::new();
    for _ in 0..nc {
        ctrl.push(o.d.bd3()?);
        if weighted && !mutant::active("SplineWeightsSkipped") {
            weights.push(o.d.bd()?);
        }
    }
    let mut fit = Vec::with_capacity(nf);
    for _ in 0..nf {
        fit.push(o.d.bd3()?);
    }
    Ok(SplineData {
        degree,
        flags,
        knots,
        ctrl,
        weights,
        fit,
    })
}

fn hatch(o: &mut Obj<'_>) -> Result<HatchData, CadError> {
    let is_gradient = o.d.bl()? != 0;
    o.d.bl()?;
    let g_angle = o.d.bd()?;
    let g_shift = o.d.bd()?;
    let g_one = o.d.bl()? != 0;
    o.d.bd()?;
    let nc = o.d.bl()?;
    let nc = counted(o, nc, 14)?;
    let mut colours = Vec::with_capacity(nc);
    for _ in 0..nc {
        o.d.bd()?;
        o.d.bs()?;
        colours.push(o.d.bl()?);
        o.d.rc()?;
    }
    let g_name = o.tv()?;
    let gradient = is_gradient.then_some((g_name, g_angle, g_shift, g_one, colours));
    let elevation = o.d.bd()?;
    let n = o.d.bd3()?;
    let name = o.tv()?;
    let solid = o.d.b()?;
    o.d.b()?;
    let np = o.d.bl()?;
    let np = counted(o, np, 34)?;
    let mut paths = Vec::with_capacity(np);
    let mut pixel = false;
    for _ in 0..np {
        let flags = o.d.bl()?;
        pixel |= flags & 4 != 0;
        let mut edges = Vec::new();
        let mut poly = None;
        if flags & 2 == 0 {
            let ns = o.d.bl()?;
            let ns = counted(o, ns, 8)?;
            edges.reserve(ns);
            for _ in 0..ns {
                edges.push(match o.d.rc()? {
                    1 => HatchEdge::Line(o.d.rd2()?, o.d.rd2()?),
                    2 => HatchEdge::Arc(o.d.rd2()?, o.d.bd()?, o.d.bd()?, o.d.bd()?, o.d.b()?),
                    3 => HatchEdge::Ellipse(
                        o.d.rd2()?,
                        o.d.rd2()?,
                        o.d.bd()?,
                        o.d.bd()?,
                        o.d.bd()?,
                        o.d.b()?,
                    ),
                    4 => {
                        let degree = o.d.bl()?;
                        let rational = o.d.b()?;
                        o.d.b()?;
                        let nk = o.d.bl()?;
                        let nk = counted(o, nk, 2)?;
                        let nctl = o.d.bl()?;
                        let nctl = counted(o, nctl, 128)?;
                        let mut knots = Vec::with_capacity(nk);
                        for _ in 0..nk {
                            knots.push(o.d.bd()?);
                        }
                        let mut ctrl = Vec::with_capacity(nctl);
                        let mut weights = Vec::new();
                        for _ in 0..nctl {
                            ctrl.push(o.d.rd2()?);
                            if rational {
                                weights.push(o.d.bd()?);
                            }
                        }
                        let mut fit = Vec::new();
                        if o.ver >= Ver::R2010 {
                            let nf = o.d.bl()?;
                            let nf = counted(o, nf, 128)?;
                            fit.reserve(nf);
                            for _ in 0..nf {
                                fit.push(o.d.rd2()?);
                            }
                            o.d.rd2()?;
                            o.d.rd2()?;
                        }
                        HatchEdge::Spline {
                            degree,
                            knots,
                            ctrl,
                            weights,
                            fit,
                        }
                    }
                    t => return Err(bad(format!("a hatch edge of type {t}"), o.d.pos() / 8)),
                });
            }
        } else {
            let bulges = o.d.b()?;
            let closed = o.d.b()?;
            let nv = o.d.bl()?;
            let nv = counted(o, nv, 128)?;
            let mut pts = Vec::with_capacity(nv);
            for _ in 0..nv {
                let p = o.d.rd2()?;
                let b = if bulges { o.d.bd()? } else { 0.0 };
                pts.push((p, b));
            }
            poly = Some((pts, closed));
        }
        let sources = o.d.bl()?;
        paths.push(HatchPath {
            flags,
            edges,
            poly,
            sources,
        });
    }
    let style = o.d.bs()?;
    o.d.bs()?;
    let (mut angle, mut scale, mut double) = (0.0, 1.0, false);
    let mut lines = Vec::new();
    if !solid {
        angle = o.d.bd()?;
        scale = o.d.bd()?;
        double = o.d.b()?;
        let nl = u32::from(o.d.bs()?);
        let nl = counted(o, nl, 10)?;
        lines.reserve(nl);
        for _ in 0..nl {
            let angle = o.d.bd()?;
            let base = o.d.bd2()?;
            let offset = o.d.bd2()?;
            let nd = u32::from(o.d.bs()?);
            let nd = counted(o, nd, 2)?;
            let mut dashes = Vec::with_capacity(nd);
            for _ in 0..nd {
                dashes.push(o.d.bd()?);
            }
            lines.push(HatchLine {
                angle,
                base,
                offset,
                dashes,
            });
        }
    }
    if pixel {
        o.d.bd()?;
    }
    let seeds = o.d.bl()?;
    let seeds = counted(o, seeds, 128)?;
    for _ in 0..seeds {
        o.d.rd2()?;
    }
    Ok(HatchData {
        gradient,
        elevation,
        n,
        name,
        solid,
        paths,
        style,
        angle,
        scale,
        double,
        lines,
        sources: Vec::new(),
    })
}

/// A defining point (`3BD`) with its DXF code.
fn p3(o: &mut Obj<'_>, c: i32, defs: &mut Vec<(i32, [f64; 3])>) -> Result<(), CadError> {
    defs.push((c, o.d.bd3()?));
    Ok(())
}

fn dimension(o: &mut Obj<'_>, kind: &str) -> Result<DimData, CadError> {
    if o.ver >= Ver::R2010 {
        o.d.rc()?;
    }
    let n = o.d.bd3()?;
    let mid = o.d.rd2()?;
    let elevation = o.d.bd()?;
    let flags1 = o.d.rc()?;
    let text = o.tv()?;
    let text_rotation = o.d.bd()?;
    o.d.bd()?;
    o.d.bd3()?;
    o.d.bd()?;
    o.d.bs()?;
    o.d.bs()?;
    o.d.bd()?;
    let measured = o.d.bd()?;
    if o.ver.strings() {
        o.d.b()?;
        o.d.b()?;
        o.d.b()?;
    }
    let p12 = o.d.rd2()?;
    let mut defs = vec![(12, [p12[0], p12[1], elevation])];
    let (ty, mut flags2) = match kind {
        "DIMENSION_ORDINATE" => {
            for c in [10, 13, 14] {
                p3(o, c, &mut defs)?;
            }
            (6, if o.d.rc()? & 1 != 0 { 64 } else { 0 })
        }
        "DIMENSION_LINEAR" | "DIMENSION_ALIGNED" => {
            for c in [13, 14, 10] {
                p3(o, c, &mut defs)?;
            }
            o.d.bd()?;
            if kind == "DIMENSION_LINEAR" {
                let r = o.d.bd()?;
                defs.push((50, [r, 0.0, 0.0]));
                (0, 0)
            } else {
                (1, 0)
            }
        }
        "DIMENSION_ANG3PT" => {
            for c in [10, 13, 14, 15] {
                p3(o, c, &mut defs)?;
            }
            (5, 0)
        }
        "DIMENSION_ANG2LN" => {
            let p = o.d.rd2()?;
            defs.push((16, [p[0], p[1], elevation]));
            for c in [13, 14, 15, 10] {
                p3(o, c, &mut defs)?;
            }
            (2, 0)
        }
        "DIMENSION_RADIUS" | "DIMENSION_DIAMETER" => {
            let order = if kind == "DIMENSION_RADIUS" {
                [10, 15]
            } else {
                [15, 10]
            };
            for c in order {
                p3(o, c, &mut defs)?;
            }
            o.d.bd()?;
            (if kind == "DIMENSION_RADIUS" { 4 } else { 3 }, 0)
        }
        _ => return Err(bad("a dimension of no known kind", o.d.pos() / 8)),
    };
    if flags1 & 1 == 0 {
        flags2 |= 128;
    }
    if flags1 & 2 != 0 {
        flags2 |= 32;
    }
    Ok(DimData {
        dxf_type: ty | flags2,
        n,
        text_mid: [mid[0], mid[1], elevation],
        text,
        measured,
        text_rotation,
        defs,
        style: None,
        block: None,
    })
}

/// The kind's fields of entity `o`, from its first field to its data's end, then its own
/// handles.
fn body(o: &mut Obj<'_>, kind: &str) -> Result<Body, CadError> {
    Ok(match kind {
        "LINE" => {
            let z0 = o.d.b()?;
            let x0 = o.d.rd()?;
            let x1 = o.d.dd(x0)?;
            let y0 = o.d.rd()?;
            let y1 = o.d.dd(y0)?;
            let (z0, z1) = if z0 {
                (0.0, 0.0)
            } else {
                let z = o.d.rd()?;
                (z, o.d.dd(z)?)
            };
            let thickness = o.d.bt()?;
            let n = o.d.be()?;
            Body::Line {
                p1: [x0, y0, z0],
                p2: [x1, y1, z1],
                thickness,
                n,
            }
        }
        "CIRCLE" | "ARC" => {
            let c = o.d.bd3()?;
            let r = o.d.bd()?;
            let thickness = o.d.bt()?;
            let n = o.d.be()?;
            if kind == "CIRCLE" {
                Body::Circle { c, r, thickness, n }
            } else {
                let a0 = o.d.bd()?;
                let a1 = o.d.bd()?;
                Body::Arc {
                    c,
                    r,
                    thickness,
                    n,
                    a0,
                    a1,
                }
            }
        }
        "POINT" => {
            let p = o.d.bd3()?;
            let thickness = o.d.bt()?;
            let n = o.d.be()?;
            o.d.bd()?;
            Body::Point { p, thickness, n }
        }
        "ELLIPSE" => Body::Ellipse {
            c: o.d.bd3()?,
            major: o.d.bd3()?,
            n: o.d.bd3()?,
            ratio: o.d.bd()?,
            p0: o.d.bd()?,
            p1: o.d.bd()?,
        },
        "SOLID" | "TRACE" => {
            let thickness = o.d.bt()?;
            let z = o.d.bd()?;
            let mut pts = [[0.0, 0.0, z]; 4];
            for p in &mut pts {
                let q = o.d.rd2()?;
                *p = [q[0], q[1], z];
            }
            let n = o.d.be()?;
            Body::Solid { pts, thickness, n }
        }
        "3DFACE" => {
            let no_flags = o.d.b()?;
            let z_zero = o.d.b()?;
            let x = o.d.rd()?;
            let y = o.d.rd()?;
            let z = if z_zero { 0.0 } else { o.d.rd()? };
            let mut pts = [[x, y, z]; 4];
            for k in 1..4 {
                let prev = pts[k - 1];
                pts[k] = [o.d.dd(prev[0])?, o.d.dd(prev[1])?, o.d.dd(prev[2])?];
            }
            let hidden = if no_flags { 0 } else { o.d.bs()? };
            Body::Face { pts, hidden }
        }
        "LWPOLYLINE" => Body::LwPolyline(lwpolyline(o)?),
        "TEXT" => Body::Text(text_data(o)?),
        "ATTRIB" => Body::Attrib(att_data(o, false)?),
        "ATTDEF" => Body::AttDef(att_data(o, true)?),
        "MTEXT" => Body::MText(mtext(o)?),
        "INSERT" | "MINSERT" => Body::Insert(insert(o, kind == "MINSERT")?),
        "BLOCK" => Body::Block { name: o.tv()? },
        "ENDBLK" => Body::EndBlk,
        "SEQEND" => Body::Seqend,
        "VIEWPORT" => Body::Viewport(viewport(o)?),
        "SPLINE" => Body::Spline(spline(o)?),
        "RAY" | "XLINE" => Body::XLine {
            point: o.d.bd3()?,
            dir: o.d.bd3()?,
            ray: kind == "RAY",
        },
        "HATCH" => Body::Hatch(Box::new(hatch(o)?)),
        k if k.starts_with("DIMENSION_") => Body::Dimension(dimension(o, k)?),
        "MULTILEADER" => Body::Mleader(Box::new(leaders::mleader(o)?)),
        "LEADER" => Body::Leader(leaders::leader(o)?),
        "IMAGE" | "WIPEOUT" => Body::Image {
            data: Box::new(image(o)?),
            wipeout: kind == "WIPEOUT",
        },
        "POLYLINE_2D" => Body::Polyline(polylines::polyline(o, PolyKind::TwoD)?),
        "POLYLINE_3D" => Body::Polyline(polylines::polyline(o, PolyKind::ThreeD)?),
        "POLYLINE_PFACE" => Body::Polyline(polylines::polyline(o, PolyKind::FaceMesh)?),
        "POLYLINE_MESH" => Body::Polyline(polylines::polyline(o, PolyKind::Mesh)?),
        "VERTEX_2D" => Body::Vertex(polylines::vertex(o, true, false)?),
        "VERTEX_3D" | "VERTEX_MESH" | "VERTEX_PFACE" => {
            Body::Vertex(polylines::vertex(o, false, false)?)
        }
        "VERTEX_PFACE_FACE" => Body::Vertex(polylines::vertex(o, false, true)?),
        _ => return Ok(Body::Other),
    })
}

/// The kind's own handles, after the common ones.
fn body_handles(o: &mut Obj<'_>, b: &mut Body) -> Result<(), CadError> {
    match b {
        Body::Text(t) => t.style = o.h()?,
        Body::Attrib(a) | Body::AttDef(a) => a.text.style = o.h()?,
        Body::MText(m) => {
            m.style = o.h()?;
            // its application, when its redundant fields are there (unused here)
            if m.redundant && !mutant::active("EntityTrailingHandlesSkipped") {
                o.h()?;
            }
        }
        Body::Viewport(v) => {
            let n = if mutant::active("ViewportFrozenUncounted") {
                0
            } else {
                v.frozen_count as usize
            };
            if n.saturating_mul(8) > o.hs.left() {
                return Err(bad(
                    "more frozen layers than the handle stream holds",
                    o.hs.pos() / 8,
                ));
            }
            for _ in 0..n {
                if let Some(h) = o.h()? {
                    v.frozen.push(h);
                }
            }
            v.clip = o.h()?;
            // its named and base coordinate systems, and from 2007 its background, visual
            // style, shade plot and sun (unused here): a viewport without them is cut short
            if !mutant::active("EntityTrailingHandlesSkipped") {
                for _ in 0..if o.ver.strings() { 6 } else { 2 } {
                    o.h()?;
                }
            }
        }
        Body::Hatch(h) => {
            let n: usize = h.paths.iter().map(|p| p.sources as usize).sum();
            if n.saturating_mul(8) > o.hs.left() {
                return Err(bad(
                    "more boundary sources than the handle stream holds",
                    o.hs.pos() / 8,
                ));
            }
            for _ in 0..n {
                if let Some(x) = o.h()? {
                    h.sources.push(x);
                }
            }
        }
        Body::Dimension(d) => {
            d.style = o.h()?;
            d.block = o.h()?;
        }
        Body::Mleader(m) => leaders::mleader_handles(o, m)?,
        Body::Leader(l) => leaders::leader_handles(o, l)?,
        Body::Polyline(p) => polylines::polyline_handles(o, p)?,
        // its image definition, then its definition's reactor
        Body::Image { data, .. } => {
            data.def = o.h()?;
            if !mutant::active("EntityTrailingHandlesSkipped") {
                o.h()?;
            }
        }
        Body::Insert(i) => {
            i.block = o.h()?;
            if i.has_attribs {
                let owned = if mutant::active("InsertOwnedIgnored") {
                    0
                } else {
                    i.owned as usize
                };
                if owned.saturating_mul(8) > o.hs.left() {
                    return Err(bad(
                        "more attributes than the handle stream holds",
                        o.hs.pos() / 8,
                    ));
                }
                i.attribs.reserve(owned);
                for _ in 0..owned {
                    if let Some(h) = o.h()? {
                        i.attribs.push(h);
                    }
                }
                i.seqend = o.h()?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// The entity at `location` in `objects`, for a file of `ver`, its kind named by the fixed table
/// or `classes`: its common data always, its kind's fields for the kinds this reader decodes.
/// A decoded kind whose read doesn't end at its data's end is refused.
pub fn entity(
    objects: &[u8],
    location: u64,
    ver: Ver,
    classes: &[Class],
) -> Result<Entity, CadError> {
    let mut o = open(objects, location, ver)?;
    let (kind, is_entity) = kind_of(o.kind, classes)
        .ok_or_else(|| bad(format!("an object of type {} no table names", o.kind), 0))?;
    if !is_entity {
        return Err(bad(
            format!("an object of type {} is not an entity", o.kind),
            0,
        ));
    }
    let common = entity_common(&mut o)?;
    let mut b = body(&mut o, &kind)?;
    if b != Body::Other {
        o.finish()?;
    }
    body_handles(&mut o, &mut b)?;
    let extra_handle_bits = if b == Body::Other {
        0
    } else {
        crate::tables::extra_handle_bits(&o)
    };
    Ok(Entity {
        handle: o.handle,
        kind,
        type_number: o.kind,
        eed: o.eed,
        common,
        body: b,
        extra_handle_bits,
    })
}

/// For the sample checks: entity `location`'s kind and how many bits its kind's read left
/// before its data's end (0 for a read that ends there; `None` for a kind read for its common
/// data only).
pub fn entity_gap(
    objects: &[u8],
    location: u64,
    ver: Ver,
    classes: &[Class],
) -> Result<(String, Option<i64>), CadError> {
    let mut o = open(objects, location, ver)?;
    let (kind, _) = kind_of(o.kind, classes)
        .ok_or_else(|| bad(format!("an object of type {} no table names", o.kind), 0))?;
    entity_common(&mut o)?;
    let b = body(&mut o, &kind)?;
    Ok((kind, (b != Body::Other).then(|| o.gap())))
}

/// For the strict check (test tooling): entity `location`'s kind, the bits its kind's read left
/// before its data's end (`None` for a kind read for its common data only), and its handle
/// stream after its handles were read: the bits left and, when they fit a `u64`, their value
/// (what follows the last handle is the byte's padding: fewer than 8 bits, all zero). The read
/// refuses on none of these; the check reports them.
pub fn entity_ends(
    objects: &[u8],
    location: u64,
    ver: Ver,
    classes: &[Class],
) -> Result<(String, Option<i64>, usize, Option<u64>), CadError> {
    let mut o = open(objects, location, ver)?;
    let (kind, is_entity) = kind_of(o.kind, classes)
        .ok_or_else(|| bad(format!("an object of type {} no table names", o.kind), 0))?;
    if !is_entity {
        return Err(bad(
            format!("an object of type {} is not an entity", o.kind),
            0,
        ));
    }
    entity_common(&mut o)?;
    let mut b = body(&mut o, &kind)?;
    let gap = (b != Body::Other).then(|| o.gap());
    body_handles(&mut o, &mut b)?;
    let left = o.hs.left();
    let rest = if left <= 64 {
        Some(o.hs.bits(left as u32)?)
    } else {
        None
    };
    Ok((kind, gap, left, rest))
}
