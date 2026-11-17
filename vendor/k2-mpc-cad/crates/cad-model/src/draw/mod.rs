//! The drawing-model read: everything a viewer needs to draw a drawing, with the format's
//! inheritance rules already applied (colour, linetype, lineweight and transparency by layer and
//! by block, text alignment and style, multiline text as paragraphs and runs), in two shapes:
//!
//! | Verb | Data |
//! |---|---|
//! | `draw header --version N` | [`header`]: units, settings, layers, linetypes, text styles, blocks, layouts and their viewports, external references (`cad.drawing.header/1`) |
//! | `draw page --version N --space S [--cursor C]` | [`page`]: one page of a space's entities in draw order, each resolved (`cad.drawing.page/1`) |
//!
//! A format's reader implements [`DrawSource`]: the model it already builds ([`Doc`]), the
//! per-record tables ([`DrawTables`]), and one entity's raw geometry on demand ([`Ent`]).
//! Geometry is never stored on the model: a page's geometry exists only while the page is built.
//! Everything else (resolution, placement, tessellation, paging, the shapes) is here, so every
//! format draws the same way.
//!
//! Coordinates are the world plan's x and y in the drawing's units; angles are radians,
//! counter-clockwise from +x. A block reference is a reference, never an expansion: a block's
//! own entities come from `draw page --space @h:<block>`, in the block's coordinates.

pub mod aci;
pub mod curve;
pub mod emit;
pub mod hatch;
pub mod style;
pub mod text;

pub use emit::{header, page, PageBudget};

use k2_mpc_base::errors::Value;

use crate::error::CadError;
use crate::model::Doc;

/// The significant digits every number of the drawing-model read is written with.
pub const FLOAT_DIGITS: usize = 9;

/// A number as the drawing-model read writes it: rounded to [`FLOAT_DIGITS`] significant
/// digits (written then as the fewest digits that read back as the rounded value), and `-0` as
/// `0`. Part of the `draw header` and `draw page` contract: a platform's maths library may
/// compute a value's last bits otherwise, and rounded, the bytes are the same on every platform
/// (a value within a last bit of half a unit in the ninth digit could still round apart). The
/// rounding is decimal formatting and parsing, no maths library.
pub fn out_float(x: f64) -> f64 {
    if !x.is_finite() || crate::mutant::active("DrawFloatsUnrounded") {
        return x;
    }
    let r = format!("{:.*e}", FLOAT_DIGITS - 1, x)
        .parse::<f64>()
        .unwrap_or(x);
    if r == 0.0 {
        0.0
    } else {
        r
    }
}

/// The drawing's scale for [`snap`]: its computed extents' larger span, or 1 when it has none.
pub fn noise_scale(doc: &Doc) -> f64 {
    match crate::reads::computed_extents(doc).0 {
        Some(b) => {
            let span = (b[2] - b[0]).max(b[3] - b[1]);
            if span.is_finite() && span > 0.0 {
                span
            } else {
                1.0
            }
        }
        None => 1.0,
    }
}

/// Every number in `v` whose magnitude is below `1e-12 × scale` written as `0`: a value that
/// is zero but for a platform's last-bit noise (`-2.22e-16` from a rotation's cosine) is the
/// same on every platform. Part of the `draw header` and `draw page` contract, with
/// [`out_float`]'s rounding: each response is snapped by its drawing's [`noise_scale`].
pub fn snap(v: &mut Value, scale: f64) {
    if crate::mutant::active("DrawNoiseKept") {
        return;
    }
    let eps = 1e-12 * scale;
    match v {
        Value::Float(_) => {
            if v.as_f64().is_some_and(|x| x.abs() < eps && x != 0.0) {
                *v = Value::float(0.0).unwrap_or(Value::Null);
            }
        }
        Value::List(items) => items.iter_mut().for_each(|x| snap(x, scale)),
        Value::Obj(kv) => kv.iter_mut().for_each(|(_, x)| snap(x, scale)),
        _ => {}
    }
}

/// A number of the drawing-model read as a JSON value ([`out_float`]; not finite: `null`).
pub(crate) fn fl(x: f64) -> Value {
    Value::float(out_float(x)).unwrap_or(Value::Null)
}

/// Entities a page holds at most.
pub const PAGE_ENTITIES: usize = 5_000;
/// A page's JSON at most, in bytes (an entity larger than this alone is alone on its page).
pub const PAGE_BYTES: usize = 4 << 20;
/// Points a tessellated path (a spline, an arc in a tilted frame) holds at most.
pub const PATH_POINTS_MAX: usize = 8_192;
/// Pattern line segments one hatch gives at most; over it the hatch is `dense`.
pub const HATCH_SEGMENTS_MAX: usize = 50_000;

/// Why an entity isn't drawn: a closed set of stable words. A page never emits another.
pub const WHYS: [&str; 6] = [
    // a kind this read doesn't model (solids, meshes, tables, proxies, a kind the reader
    // doesn't know)
    "not_modelled",
    // an attribute definition in a block: a template, not drawn in a reference (unless constant)
    "attdef",
    // a spline with a weight at or below zero
    "spline_weights",
    // a path or pattern that can't be drawn within its bound
    "over_cap",
    // a kind this read models, whose values don't read
    "unreadable",
    // a viewport: in the header's layouts, not drawn as an entity
    "viewport",
];

/// The drawing-wide settings the drawing rules use.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// The global linetype scale (`$LTSCALE`).
    pub ltscale: f64,
    /// Paper-space linetype scaling in viewports (`$PSLTSCALE`).
    pub psltscale: bool,
    /// The default lineweight, hundredths of a millimetre (`$LWDEFAULT`).
    pub lwdefault: i64,
    /// Lineweights shown (`$LWDISPLAY`).
    pub lwdisplay: bool,
    /// Solids and hatches filled (`$FILLMODE`).
    pub fillmode: bool,
    /// The point display mode and size (`$PDMODE`, `$PDSIZE`).
    pub pdmode: i64,
    pub pdsize: f64,
    /// Mirrored text stays readable (`$MIRRTEXT` 0).
    pub mirrtext: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            ltscale: 1.0,
            psltscale: true,
            lwdefault: 25,
            lwdisplay: false,
            fillmode: true,
            pdmode: 0,
            pdsize: 0.0,
            mirrtext: false,
        }
    }
}

/// A layer's drawing facts the model doesn't keep: its true colour and its transparency.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayerDraw {
    /// True colour, `0xRRGGBB`.
    pub rgb: Option<u32>,
    /// Transparency as the format codes it (`0x02000000 | alpha`).
    pub alpha: Option<i64>,
}

/// One complex element of a linetype: text or a shape after a dash.
#[derive(Debug, Clone, PartialEq)]
pub struct LtElement {
    /// The dash it follows (from 0).
    pub after: usize,
    /// `text` or `shape`.
    pub kind: &'static str,
    pub text: Option<String>,
    pub shape: Option<i64>,
    /// The text style or shape file it uses.
    pub style: Option<u64>,
    pub scale: f64,
    /// Radians.
    pub rotation: f64,
    pub offset: [f64; 2],
}

/// A linetype.
#[derive(Debug, Clone, PartialEq)]
pub struct Linetype {
    pub handle: Option<u64>,
    pub name: String,
    /// The pattern's length.
    pub length: f64,
    /// Dash lengths: positive a dash, negative a gap, zero a dot.
    pub dashes: Vec<f64>,
    pub complex: Vec<LtElement>,
}

/// A text style.
#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    pub handle: Option<u64>,
    pub name: String,
    /// The font file as written.
    pub file: String,
    pub bigfont: Option<String>,
    /// The font family, when the drawing names one.
    pub family: Option<String>,
    /// A non-zero fixed height overrides each text's own.
    pub fixed_height: f64,
    pub width: f64,
    /// Radians.
    pub oblique: f64,
    pub bold: bool,
    pub italic: bool,
    pub backwards: bool,
    pub upside_down: bool,
    pub vertical: bool,
}

impl TextStyle {
    /// A single-line (shape) font: a `.shx` file, or a file named without an extension.
    pub fn shx(&self) -> bool {
        let f = self.file.trim().to_ascii_lowercase();
        f.ends_with(".shx") || (!f.is_empty() && !f.contains('.'))
    }
}

/// A sheet's paper.
#[derive(Debug, Clone, PartialEq)]
pub struct Paper {
    /// Width and height in millimetres.
    pub w_mm: f64,
    pub h_mm: f64,
    /// Quarter turns (0 to 3).
    pub rotation: i64,
    /// Left, bottom, right, top, in millimetres.
    pub margins_mm: [f64; 4],
}

/// A viewport of a layout onto model space.
#[derive(Debug, Clone, PartialEq)]
pub struct Viewport {
    pub handle: Option<u64>,
    /// Centre and size on the sheet.
    pub centre: [f64; 2],
    pub size: [f64; 2],
    /// The model-space view: its centre and height.
    pub view_centre: [f64; 2],
    pub view_height: f64,
    /// Radians.
    pub rotation: f64,
    pub on: bool,
    /// Layers frozen in this viewport only.
    pub frozen_layers: Vec<u64>,
    /// The entity that clips it, when it is clipped.
    pub clip: Option<u64>,
    /// It looks straight down.
    pub plan: bool,
}

/// What a layout record holds beyond its block record: its tab, paper and viewports.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LayoutInfo {
    pub name: Option<String>,
    pub tab: Option<i64>,
    pub paper: Option<Paper>,
    pub viewports: Vec<Viewport>,
}

/// A space's draw-order table: entity handle and its sort handle.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SortTable {
    pub pairs: Vec<(u64, u64)>,
}

/// An external reference's path and kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XrefPath {
    /// Its block record (an index into [`Doc::blocks`]).
    pub block: usize,
    pub path: String,
    pub overlay: bool,
}

/// A dimension style's arrow facts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DimStyle {
    pub name: String,
    /// The arrow size (`DIMASZ`), before the overall scale.
    pub arrow_size: Option<f64>,
    /// The overall scale (`DIMSCALE`).
    pub scale: Option<f64>,
    /// A leader's arrow block (`DIMLDRBLK`), by its block record's handle.
    pub leader_arrow: Option<u64>,
}

/// An entity's own overrides of its dimension style's arrow facts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DimOverrides {
    pub arrow_size: Option<f64>,
    pub scale: Option<f64>,
    pub leader_arrow: Option<u64>,
}

/// The per-record tables a reader hands over beside its [`Doc`].
#[derive(Debug, Clone, Default)]
pub struct DrawTables {
    pub settings: Settings,
    /// One per [`Doc::layers`] entry, in the same order.
    pub layers: Vec<LayerDraw>,
    pub linetypes: Vec<Linetype>,
    pub text_styles: Vec<TextStyle>,
    /// One per [`Doc::blocks`] entry, in the same order (default for a block definition).
    pub layouts: Vec<LayoutInfo>,
    /// One per [`Doc::blocks`] entry: its draw-order table, when it has one.
    pub sortents: Vec<Option<SortTable>>,
    pub xrefs: Vec<XrefPath>,
    pub dimstyles: Vec<DimStyle>,
    /// Image definitions: handle and file path as written.
    pub image_defs: Vec<(u64, String)>,
}

/// A polyline vertex: its point (its own frame's), bulge and widths.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vert {
    pub p: [f64; 3],
    pub bulge: f64,
    pub w0: f64,
    pub w1: f64,
}

/// A single-line text, an attribute or an attribute definition, as written.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextRaw {
    /// The first and second alignment points (its own frame's).
    pub p10: Option<[f64; 3]>,
    pub p11: Option<[f64; 3]>,
    pub height: f64,
    /// Degrees.
    pub rotation: f64,
    pub width: Option<f64>,
    /// Degrees.
    pub oblique: Option<f64>,
    pub style: String,
    /// Horizontal (0 left, 1 center, 2 right, 3 aligned, 4 middle, 5 fit) and vertical (0
    /// baseline, 1 bottom, 2 middle, 3 top) alignment.
    pub halign: i64,
    pub valign: i64,
    /// Generation flags: 2 backwards, 4 upside down.
    pub generation: i64,
    pub text: String,
    pub n: [f64; 3],
}

/// A multiline text's columns: their type (1 static, 2 dynamic), count, width and gutter,
/// whether their height is automatic and their flow reversed, and each column's height.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Columns {
    pub kind: i64,
    pub count: i64,
    pub width: Option<f64>,
    pub gutter: Option<f64>,
    pub auto_height: bool,
    pub reversed: bool,
    pub heights: Vec<f64>,
}

/// A multiline text as written: its content with every inline code.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MTextRaw {
    pub at: [f64; 3],
    pub height: f64,
    /// The reference width (0: none).
    pub width: f64,
    /// 1 to 9: top left to bottom right.
    pub attach: i64,
    /// The direction (world), when given: it wins over `rotation`.
    pub dir: Option<[f64; 3]>,
    /// Degrees.
    pub rotation: f64,
    pub style: String,
    pub spacing: f64,
    pub spacing_style: i64,
    /// Background fill: flags, colour number, true colour, scale.
    pub background: Option<(i64, Option<i64>, Option<u32>, f64)>,
    /// Column type and count.
    pub columns: Option<Columns>,
    pub content: String,
    pub n: [f64; 3],
}

/// An attribute of a block reference.
#[derive(Debug, Clone, PartialEq)]
pub struct AttribRaw {
    pub handle: Option<u64>,
    pub tag: String,
    pub flags: i64,
    pub text: TextRaw,
    pub layer: String,
}

/// One edge of a hatch boundary loop, in the hatch's frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Edge {
    Line([f64; 2], [f64; 2]),
    /// Centre, radius, start and end angle (degrees), counter-clockwise.
    Arc {
        c: [f64; 2],
        r: f64,
        a0: f64,
        a1: f64,
        ccw: bool,
    },
    /// Centre, major axis end (relative), minor-to-major ratio, start and end angle (degrees).
    Ellipse {
        c: [f64; 2],
        major: [f64; 2],
        ratio: f64,
        a0: f64,
        a1: f64,
        ccw: bool,
    },
    Spline(SplineRaw),
}

/// A polyline loop: its vertices with their bulges, and whether it is closed.
pub type PolyLoop = (Vec<([f64; 2], f64)>, bool);

/// A hatch's gradient fill.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Gradient {
    pub name: String,
    /// Degrees.
    pub angle: f64,
    pub shift: f64,
    pub one_colour: bool,
    /// Each colour: its number and true colour, either absent.
    pub colours: Vec<(Option<i64>, Option<u32>)>,
}

/// A hatch boundary loop: its flags and edges, or a polyline.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopRaw {
    /// 1 external, 2 polyline, 4 derived, 16 outermost.
    pub flags: i64,
    pub edges: Vec<Edge>,
    /// A polyline loop: vertices with bulges, closed.
    pub poly: Option<PolyLoop>,
}

/// A hatch pattern's definition line, in drawing units (scale and angle applied).
#[derive(Debug, Clone, PartialEq)]
pub struct PatLine {
    /// Degrees.
    pub angle: f64,
    pub base: [f64; 2],
    pub offset: [f64; 2],
    pub dashes: Vec<f64>,
}

/// A hatch as written.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HatchRaw {
    pub solid: bool,
    pub pattern: String,
    /// Degrees.
    pub angle: f64,
    pub scale: f64,
    pub double: bool,
    /// 0 odd parity, 1 outermost, 2 ignore islands.
    pub style: i64,
    pub defs: Vec<PatLine>,
    pub loops: Vec<LoopRaw>,
    /// A gradient: its name, angle (degrees), shift, one colour, and its colours (colour number,
    /// true colour).
    pub gradient: Option<Gradient>,
    /// The handles of the boundary's source entities.
    pub assoc: Vec<u64>,
    pub elevation: f64,
    pub n: [f64; 3],
}

/// A spline as written.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SplineRaw {
    pub degree: i64,
    pub flags: i64,
    pub knots: Vec<f64>,
    pub ctrl: Vec<[f64; 3]>,
    pub weights: Vec<f64>,
    pub fit: Vec<[f64; 3]>,
}

/// An entity's raw geometry: the values as the format states them, in its own frame where it
/// has one (`n`, its extrusion). [`emit`] resolves and converts them.
#[derive(Debug, Clone, PartialEq)]
pub enum Geom {
    Line {
        p1: [f64; 3],
        p2: [f64; 3],
    },
    /// An infinite line (`ray: false`) or a ray from `point`.
    XLine {
        point: [f64; 3],
        dir: [f64; 3],
        ray: bool,
    },
    Point {
        p: [f64; 3],
    },
    Circle {
        c: [f64; 3],
        r: f64,
        n: [f64; 3],
    },
    /// Angles in degrees.
    Arc {
        c: [f64; 3],
        r: f64,
        a0: f64,
        a1: f64,
        n: [f64; 3],
    },
    /// Centre and major axis in world coordinates; parameters in radians.
    Ellipse {
        c: [f64; 3],
        major: [f64; 3],
        ratio: f64,
        p0: f64,
        p1: f64,
        n: [f64; 3],
    },
    Poly {
        verts: Vec<Vert>,
        closed: bool,
        const_w: Option<f64>,
        /// The vertices are world points (a 3D polyline).
        world: bool,
        n: [f64; 3],
    },
    Spline(SplineRaw),
    Text(TextRaw),
    MText(MTextRaw),
    Insert {
        block: String,
        at: [f64; 3],
        scale: [f64; 3],
        /// Degrees.
        rotation: f64,
        /// Columns, rows, and their spacing.
        array: Option<(i64, i64, f64, f64)>,
        attribs: Vec<AttribRaw>,
        n: [f64; 3],
    },
    /// Four corners (a SOLID or TRACE in its frame, as the format orders them: 1, 2, 3, 4,
    /// drawn 1, 2, 4, 3).
    Solid {
        pts: [[f64; 3]; 4],
        n: [f64; 3],
    },
    /// Four world corners, in drawing order, and its invisible-edge flags.
    Face {
        pts: [[f64; 3]; 4],
        hidden: i64,
    },
    Hatch(Box<HatchRaw>),
    Dimension {
        block: String,
        kind: i64,
        measured: Option<f64>,
        text: String,
        style: String,
        /// The defining points by code (10, 11, 13, 14, 15, 16).
        defs: Vec<(i32, [f64; 3])>,
        ins: Option<[f64; 3]>,
    },
    Leader {
        verts: Vec<[f64; 3]>,
        spline: bool,
        arrow: bool,
        style: String,
        annotation: Option<u64>,
        /// Its own dimension-style overrides: arrow size, overall scale, arrow block.
        overrides: DimOverrides,
    },
    /// A multileader: its text, its leaders' lines and landings, its arrowhead
    /// ([`crate::leader`]).
    Mleader(Box<crate::leader::Mleader>),
    /// An image or a wipeout: its insertion point, pixel axis vectors, size in pixels, clip
    /// boundary (pixel coordinates) and clip type, and the image definition.
    Image {
        wipeout: bool,
        ins: [f64; 3],
        u: [f64; 3],
        v: [f64; 3],
        size: [f64; 2],
        clip: Vec<[f64; 2]>,
        clip_type: i64,
        def: Option<u64>,
        path: Option<String>,
    },
    /// A viewport: drawn from the header's layouts.
    Viewport,
    /// An attribute definition: a template.
    AttDef {
        text: TextRaw,
        constant: bool,
    },
    /// A kind this read doesn't model.
    NotModelled,
    /// A kind this read models whose values don't read.
    Unreadable,
}

/// One entity as a reader hands it to a page: its raw geometry and the common facts the model
/// doesn't keep.
#[derive(Debug, Clone, PartialEq)]
pub struct Ent {
    pub geom: Geom,
    /// Its own linetype scale (1 when unstated).
    pub lt_scale: f64,
    /// It is invisible (60).
    pub invisible: bool,
    /// A colour-book colour's name.
    pub book: Option<String>,
}

impl Ent {
    pub fn of(geom: Geom) -> Ent {
        Ent {
            geom,
            lt_scale: 1.0,
            invisible: false,
            book: None,
        }
    }
}

/// What a drawing format's reader gives the drawing-model read.
pub trait DrawSource {
    /// The model the index and the other reads use.
    fn doc(&self) -> &Doc;
    /// The per-record tables.
    fn tables(&self) -> &DrawTables;
    /// Entity `i`'s raw geometry ([`Doc::entities`] index), read on demand.
    fn entity(&self, i: usize) -> Result<Ent, CadError>;
}

/// The host's answer for an external reference: whether its path resolves, and to what.
pub trait XrefResolver {
    /// The resolved doc's address, when `path` resolves.
    fn resolve(&self, path: &str) -> Option<String>;
}

/// No resolution: every external reference is unresolved.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoXrefs;

impl XrefResolver for NoXrefs {
    fn resolve(&self, _path: &str) -> Option<String> {
        None
    }
}
