//! Our own test drawings (the test-only `testgen` feature): a small writer of ASCII DXF, built
//! from a [`Spec`] of layers, blocks and entities, with the byte-level variations the tokenizer
//! must carry exactly (line endings, code padding, comments, a byte-order mark, bytes after the
//! end marker, handle spellings). It writes what these tests need, not every table a drawing
//! may hold. Its header values are neutral: no user, machine or path names.

use std::collections::BTreeMap;

/// A DXF version, by its `$ACADVER` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ver {
    /// AC1009.
    R12,
    /// AC1015.
    R2000,
    /// AC1018.
    R2004,
    /// AC1021.
    R2007,
    /// AC1024.
    R2010,
    /// AC1027.
    R2013,
    /// AC1032.
    R2018,
}

impl Ver {
    /// Every version, oldest first.
    pub const ALL: [Ver; 7] = [
        Ver::R12,
        Ver::R2000,
        Ver::R2004,
        Ver::R2007,
        Ver::R2010,
        Ver::R2013,
        Ver::R2018,
    ];

    /// Its `$ACADVER` value.
    pub fn acadver(self) -> &'static str {
        match self {
            Ver::R12 => "AC1009",
            Ver::R2000 => "AC1015",
            Ver::R2004 => "AC1018",
            Ver::R2007 => "AC1021",
            Ver::R2010 => "AC1024",
            Ver::R2013 => "AC1027",
            Ver::R2018 => "AC1032",
        }
    }

    fn modern(self) -> bool {
        self != Ver::R12
    }
}

/// How lines end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eol {
    Lf,
    CrLf,
    /// `\n` and `\r\n` by turns, line by line.
    Mixed,
}

/// The byte-level shape of the file and its header values.
#[derive(Debug, Clone)]
pub struct Opts {
    pub version: Ver,
    pub eol: Eol,
    /// Codes right-justified to three characters (`  0`), as some writers do.
    pub pad: bool,
    /// A UTF-8 byte-order mark first.
    pub bom: bool,
    /// Bytes after the end marker.
    pub tail: Vec<u8>,
    /// `999` comments: one first, one in the entities.
    pub comments: bool,
    /// Entities carry handles (code 5). Off: an R12 drawing with `$HANDLING` 0.
    pub handles: bool,
    /// Handles in lower case.
    pub lower_handles: bool,
    /// Handles with leading zeros (`00002A`).
    pub padded_handles: bool,
    /// `$INSUNITS`, when set (written for R2000 and later).
    pub insunits: Option<i32>,
    /// `$MEASUREMENT`, when set.
    pub measurement: Option<i32>,
    /// `$EXTMIN` and `$EXTMAX`, when set.
    pub extents: Option<([f64; 2], [f64; 2])>,
    /// `$LASTSAVEDBY`, when set (R2000 and later).
    pub lastsavedby: Option<String>,
    /// `$HANDSEED` as written; `None`: one above the highest handle.
    pub handseed: Option<u64>,
    /// The `0`/`EOF` pair at the end.
    pub eof: bool,
    /// `$DWGCODEPAGE` (R2000 and later).
    pub codepage: String,
}

impl Opts {
    /// LF, unpadded, handles on, millimetres, an end marker; nothing else.
    pub fn new(version: Ver) -> Self {
        Opts {
            version,
            eol: Eol::Lf,
            pad: false,
            bom: false,
            tail: Vec::new(),
            comments: false,
            handles: true,
            lower_handles: false,
            padded_handles: false,
            insunits: Some(4),
            measurement: Some(1),
            extents: None,
            lastsavedby: None,
            handseed: None,
            eof: true,
            codepage: "ANSI_1252".into(),
        }
    }
}

/// An entity's geometry or content.
#[derive(Debug, Clone)]
pub enum Shape {
    Line([f64; 2], [f64; 2]),
    Point([f64; 2]),
    Circle([f64; 2], f64),
    /// Centre, radius, start and end angle (degrees, counter-clockwise).
    Arc([f64; 2], f64, f64, f64),
    /// Vertices with their bulge, closed.
    LwPoly(Vec<([f64; 2], f64)>, bool),
    /// A POLYLINE with VERTEX followers and a SEQEND: vertices with their bulge, closed.
    Poly(Vec<([f64; 2], f64)>, bool),
    /// Insertion point, height, text.
    Text([f64; 2], f64, String),
    MText([f64; 2], f64, String),
    /// An INSERT of a block at a point, with attributes (tag, value) as ATTRIB followers.
    Insert(String, [f64; 2], Vec<(String, String)>),
    /// Any kind, its subclass pairs as given (after the common group).
    Raw(String, Vec<(i32, String)>),
}

/// A multileader's leader, as [`Ent::mleader_branches`] writes it: its lines (each from its
/// arrow) and its landing.
pub type MleaderBranch = (Vec<Vec<[f64; 2]>>, [f64; 2]);

/// One entity, with its common properties.
#[derive(Debug, Clone)]
pub struct Ent {
    pub shape: Shape,
    pub layer: String,
    pub color: Option<i32>,
    pub ltype: Option<String>,
    /// An extrusion other than 0,0,1: the entity is in its own frame.
    pub extrusion: Option<[f64; 3]>,
    /// A block reference's scale along x and y and its rotation in degrees (an INSERT only).
    pub xform: Option<[f64; 3]>,
    /// XDATA pairs (from a `1001` application name on).
    pub xdata: Vec<(i32, String)>,
    /// A label for the test to find its handle by ([`Built::handle`]).
    pub label: Option<String>,
    /// Its handle, forced (to plant a duplicate).
    pub handle: Option<u64>,
    /// Written without a handle, though the drawing has them.
    pub no_handle: bool,
    /// More pairs of the common group (lineweight, true colour, transparency, linetype scale,
    /// invisibility), after the colour.
    pub props: Vec<(i32, String)>,
}

impl Ent {
    fn of(shape: Shape) -> Ent {
        Ent {
            shape,
            layer: "0".into(),
            color: None,
            ltype: None,
            extrusion: None,
            xform: None,
            xdata: Vec::new(),
            label: None,
            handle: None,
            no_handle: false,
            props: Vec::new(),
        }
    }

    pub fn line(a: [f64; 2], b: [f64; 2]) -> Ent {
        Ent::of(Shape::Line(a, b))
    }

    pub fn point(p: [f64; 2]) -> Ent {
        Ent::of(Shape::Point(p))
    }

    pub fn circle(c: [f64; 2], r: f64) -> Ent {
        Ent::of(Shape::Circle(c, r))
    }

    pub fn arc(c: [f64; 2], r: f64, start: f64, end: f64) -> Ent {
        Ent::of(Shape::Arc(c, r, start, end))
    }

    pub fn lwpoly(v: &[([f64; 2], f64)], closed: bool) -> Ent {
        Ent::of(Shape::LwPoly(v.to_vec(), closed))
    }

    pub fn poly(v: &[([f64; 2], f64)], closed: bool) -> Ent {
        Ent::of(Shape::Poly(v.to_vec(), closed))
    }

    pub fn text(p: [f64; 2], h: f64, s: &str) -> Ent {
        Ent::of(Shape::Text(p, h, s.into()))
    }

    pub fn mtext(p: [f64; 2], h: f64, s: &str) -> Ent {
        Ent::of(Shape::MText(p, h, s.into()))
    }

    pub fn insert(block: &str, p: [f64; 2]) -> Ent {
        Ent::of(Shape::Insert(block.into(), p, Vec::new()))
    }

    pub fn raw(kind: &str, pairs: &[(i32, &str)]) -> Ent {
        Ent::of(Shape::Raw(
            kind.into(),
            pairs.iter().map(|(c, v)| (*c, (*v).to_owned())).collect(),
        ))
    }

    /// A leader through `points` (its arrow at the first), pointing at its text by the handle
    /// `annotation` when given (2000 and later: a LEADER entity).
    pub fn leader(points: &[[f64; 2]], annotation: Option<u64>) -> Ent {
        let mut p: Vec<(i32, String)> = vec![(100, "AcDbLeader".into()), (3, "Standard".into())];
        for (c, v) in [(71, "1"), (72, "0"), (73, "0"), (74, "1"), (75, "0")] {
            p.push((c, v.into()));
        }
        p.push((40, "0.5".into()));
        p.push((41, "2.0".into()));
        p.push((76, points.len().to_string()));
        for q in points {
            p.push((10, num(q[0])));
            p.push((20, num(q[1])));
            p.push((30, "0.0".into()));
        }
        if let Some(h) = annotation {
            p.push((340, format!("{h:X}")));
        }
        Ent::of(Shape::Raw("LEADER".into(), p))
    }

    /// A multileader (2000 and later) with one leader: its line through `line` (its arrow at
    /// the first point) to its landing, and its text `text` at `at`.
    pub fn mleader(line: &[[f64; 2]], landing: [f64; 2], at: [f64; 2], text: &str) -> Ent {
        Ent::mleader_branches(&[(vec![line.to_vec()], landing)], at, text)
    }

    /// A multileader (2000 and later) with a leader per branch: its lines (each from its
    /// arrow) and its landing; its text `text` (written as given, inline codes and all) at `at`.
    pub fn mleader_branches(branches: &[MleaderBranch], at: [f64; 2], text: &str) -> Ent {
        let landing = branches.first().map_or(at, |b| b.1);
        let mut p: Vec<(i32, String)> = Vec::new();
        for (c, v) in [
            (100, "AcDbMLeader"),
            (270, "2"),
            (300, "CONTEXT_DATA{"),
            (40, "1.0"),
        ] {
            p.push((c, v.into()));
        }
        let mut nums: Vec<(i32, f64)> = vec![(10, landing[0]), (20, landing[1]), (30, 0.0)];
        nums.extend([(41, 0.5), (140, 0.5), (145, 0.2)]);
        p.extend(nums.drain(..).map(|(c, v)| (c, num(v))));
        for (c, v) in [(174, "1"), (175, "1"), (176, "0"), (177, "0"), (290, "1")] {
            p.push((c, v.into()));
        }
        p.push((304, text.into()));
        nums.extend([
            (11, 0.0),
            (21, 0.0),
            (31, 1.0),
            (12, at[0]),
            (22, at[1]),
            (32, 0.0),
        ]);
        nums.extend([
            (13, 1.0),
            (23, 0.0),
            (33, 0.0),
            (42, 0.0),
            (43, 0.0),
            (44, 0.0),
        ]);
        nums.push((45, 1.0));
        p.extend(nums.drain(..).map(|(c, v)| (c, num(v))));
        for (c, v) in [(170, "1"), (171, "1"), (172, "5"), (296, "0")] {
            p.push((c, v.into()));
        }
        nums.extend([(110, 0.0), (120, 0.0), (130, 0.0), (111, 1.0), (121, 0.0)]);
        nums.extend([(131, 0.0), (112, 0.0), (122, 1.0), (132, 0.0)]);
        p.extend(nums.drain(..).map(|(c, v)| (c, num(v))));
        p.push((297, "0".into()));
        for (k, (lines, landing)) in branches.iter().enumerate() {
            for (c, v) in [(302, "LEADER{"), (290, "1"), (291, "1")] {
                p.push((c, v.into()));
            }
            nums.extend([(10, landing[0]), (20, landing[1]), (30, 0.0)]);
            nums.extend([(11, 1.0), (21, 0.0), (31, 0.0)]);
            p.extend(nums.drain(..).map(|(c, v)| (c, num(v))));
            p.push((90, k.to_string()));
            p.push((40, "0.5".into()));
            for (j, line) in lines.iter().enumerate() {
                p.push((304, "LEADER_LINE{".into()));
                for q in line {
                    p.push((10, num(q[0])));
                    p.push((20, num(q[1])));
                    p.push((30, "0.0".into()));
                }
                p.push((91, j.to_string()));
                p.push((305, "}".into()));
            }
            p.push((271, "0".into()));
            p.push((303, "}".into()));
        }
        for (c, v) in [
            (272, "9"),
            (273, "9"),
            (301, "}"),
            (90, "0"),
            (170, "1"),
            (290, "1"),
            (291, "1"),
            (41, "0.5"),
            (42, "0.25"),
            (172, "2"),
        ] {
            p.push((c, v.into()));
        }
        Ent::of(Shape::Raw("MULTILEADER".into(), p))
    }

    /// On layer `name`.
    pub fn on(mut self, name: &str) -> Ent {
        self.layer = name.into();
        self
    }

    pub fn color(mut self, c: i32) -> Ent {
        self.color = Some(c);
        self
    }

    pub fn ltype(mut self, t: &str) -> Ent {
        self.ltype = Some(t.into());
        self
    }

    pub fn extrusion(mut self, n: [f64; 3]) -> Ent {
        self.extrusion = Some(n);
        self
    }

    /// Scaled by `sx` along the block's x and `sy` along its y, then rotated by `rot` degrees
    /// (an INSERT only).
    pub fn placed(mut self, sx: f64, sy: f64, rot: f64) -> Ent {
        self.xform = Some([sx, sy, rot]);
        self
    }

    /// An attribute (an INSERT only).
    pub fn attrib(mut self, tag: &str, value: &str) -> Ent {
        if let Shape::Insert(_, _, a) = &mut self.shape {
            a.push((tag.into(), value.into()));
        }
        self
    }

    /// XDATA under application `app`, with these pairs after its `1001`.
    pub fn xdata(mut self, app: &str, pairs: &[(i32, &str)]) -> Ent {
        self.xdata.push((1001, app.into()));
        self.xdata
            .extend(pairs.iter().map(|(c, v)| (*c, (*v).to_owned())));
        self
    }

    pub fn label(mut self, l: &str) -> Ent {
        self.label = Some(l.into());
        self
    }

    pub fn handle(mut self, h: u64) -> Ent {
        self.handle = Some(h);
        self
    }

    pub fn no_handle(mut self) -> Ent {
        self.no_handle = true;
        self
    }

    /// A pair of the common group (`370` lineweight, `420` true colour, `440` transparency,
    /// `48` linetype scale, `60` invisibility).
    pub fn prop(mut self, code: i32, v: &str) -> Ent {
        self.props.push((code, v.into()));
        self
    }
}

/// A layer.
#[derive(Debug, Clone)]
pub struct Layer {
    pub name: String,
    /// Negative: the layer is off.
    pub color: i32,
    pub frozen: bool,
    pub locked: bool,
    pub ltype: String,
}

/// A block definition.
#[derive(Debug, Clone)]
pub struct Block {
    pub name: String,
    pub base: [f64; 2],
    pub ents: Vec<Ent>,
    /// Its flags (`1` anonymous, `4` an external reference).
    pub flags: i32,
    /// An external reference's path.
    pub path: String,
    /// An anonymous block that draws a dynamic block's reference as set: that block's name,
    /// named by handle in this block's record's extension data.
    pub rep_of: Option<String>,
}

/// A whole drawing.
#[derive(Debug, Clone)]
pub struct Spec {
    pub opts: Opts,
    pub layers: Vec<Layer>,
    pub blocks: Vec<Block>,
    pub model: Vec<Ent>,
    pub paper: Vec<Ent>,
    /// Header variables added after the generator's own: name and pairs.
    pub header_extra: Vec<(String, Vec<(i32, String)>)>,
    /// Objects added after the generator's own (R2000 and later): kind and pairs after the
    /// handle.
    pub objects_extra: Vec<(String, Vec<(i32, String)>)>,
    /// The tables and objects the drawing-model read reads (all empty: none written).
    pub draw: DrawExtra,
}

/// Tables and objects for the drawing-model read's rows, written only when given.
#[derive(Debug, Clone, Default)]
pub struct DrawExtra {
    /// Linetypes after the three every drawing has: name and dash lengths.
    pub linetypes: Vec<(String, Vec<f64>)>,
    /// Text styles after `Standard`: name, font file, family (in its extension data, from 2000
    /// on).
    pub styles: Vec<(String, String, Option<String>)>,
    /// Pairs added at the end of a layer's entry, by layer name.
    pub layer_pairs: Vec<(String, Vec<(i32, String)>)>,
    /// Model space's draw-order table (2000 on): entity handle and sort handle.
    pub sortents: Vec<(u64, u64)>,
    /// The paper layout's paper size, millimetres.
    pub paper: Option<(f64, f64)>,
    /// Image definitions (2000 on): their handle and file path.
    pub images: Vec<(u64, String)>,
    /// The paper layout's name (2000 on); `Layout1` when none.
    pub layout: Option<String>,
}

/// A generated drawing: its bytes, and each labelled entity's handle.
#[derive(Debug, Clone)]
pub struct Built {
    pub bytes: Vec<u8>,
    pub labels: BTreeMap<String, u64>,
    /// The highest handle written.
    pub hmax: u64,
}

impl Built {
    /// The handle of the entity labelled `l`.
    ///
    /// # Panics
    /// When no entity has that label: a test's own mistake.
    pub fn handle(&self, l: &str) -> u64 {
        *self
            .labels
            .get(l)
            .unwrap_or_else(|| panic!("no entity labelled {l}"))
    }

    /// `@h:<HANDLE>` of the entity labelled `l`.
    pub fn addr(&self, l: &str) -> String {
        format!("@h:{:X}", self.handle(l))
    }
}

impl Spec {
    /// Layer `0` only, no entities.
    pub fn new(version: Ver) -> Spec {
        Spec::with(Opts::new(version))
    }

    pub fn with(opts: Opts) -> Spec {
        Spec {
            opts,
            layers: vec![Layer {
                name: "0".into(),
                color: 7,
                frozen: false,
                locked: false,
                ltype: "CONTINUOUS".into(),
            }],
            blocks: Vec::new(),
            model: Vec::new(),
            paper: Vec::new(),
            header_extra: Vec::new(),
            objects_extra: Vec::new(),
            draw: DrawExtra::default(),
        }
    }

    pub fn layer(mut self, name: &str, color: i32) -> Spec {
        self.layers.push(Layer {
            name: name.into(),
            color,
            frozen: false,
            locked: false,
            ltype: "CONTINUOUS".into(),
        });
        self
    }

    pub fn layer_full(mut self, l: Layer) -> Spec {
        self.layers.push(l);
        self
    }

    pub fn block(mut self, name: &str, base: [f64; 2], ents: Vec<Ent>) -> Spec {
        self.blocks.push(Block {
            name: name.into(),
            base,
            ents,
            flags: 0,
            path: String::new(),
            rep_of: None,
        });
        self
    }

    /// An anonymous block `name` (`*U…`) drawing a reference of dynamic block `of` as set (2000
    /// and later): its record names `of`'s record (`AcDbBlockRepBTag`, `1005`).
    pub fn dynamic(mut self, name: &str, base: [f64; 2], ents: Vec<Ent>, of: &str) -> Spec {
        self.blocks.push(Block {
            name: name.into(),
            base,
            ents,
            flags: 1,
            path: String::new(),
            rep_of: Some(of.into()),
        });
        self
    }

    /// An external reference `name` to the drawing at `path`: a block with no entities.
    pub fn xref(mut self, name: &str, path: &str) -> Spec {
        self.blocks.push(Block {
            name: name.into(),
            base: [0.0, 0.0],
            ents: Vec::new(),
            flags: 4,
            path: path.into(),
            rep_of: None,
        });
        self
    }

    pub fn model(mut self, e: Ent) -> Spec {
        self.model.push(e);
        self
    }

    pub fn paper(mut self, e: Ent) -> Spec {
        self.paper.push(e);
        self
    }

    pub fn header(mut self, name: &str, pairs: &[(i32, &str)]) -> Spec {
        self.header_extra.push((
            name.into(),
            pairs.iter().map(|(c, v)| (*c, (*v).to_owned())).collect(),
        ));
        self
    }

    pub fn object(mut self, kind: &str, pairs: &[(i32, &str)]) -> Spec {
        self.objects_extra.push((
            kind.into(),
            pairs.iter().map(|(c, v)| (*c, (*v).to_owned())).collect(),
        ));
        self
    }

    /// The drawing's bytes.
    pub fn build(&self) -> Built {
        let mut w = Writer::new(&self.opts);
        w.paper = self.draw.paper;
        w.layout = self.draw.layout.clone();
        w.write(self)
    }
}

/// A number as the generator writes it: shortest round-trip text, with a decimal point.
pub fn num(x: f64) -> String {
    let s = format!("{x}");
    if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") {
        s
    } else {
        format!("{s}.0")
    }
}

/// Handles kept for objects other records point at before they are written: the default plot
/// style's placeholder (every layer names it) and the two layouts (their block records name
/// them). Below every handle the writer mints.
const PLACEHOLDER: u64 = 0x0A;
/// The application under which an anonymous block's record names the dynamic block it draws,
/// and the generator's own placeholder for that name's handle until it is known.
const REP_APP: &str = "AcDbBlockRepBTag";
const REP_MARK: &str = "\u{1}rep:";
const LAYOUT_MODEL: u64 = 0x0B;
const LAYOUT_PAPER: u64 = 0x0C;
/// Model space's extension dictionary and its draw-order table, when the drawing has one.
const SORT_DICT: u64 = 0x0D;
const SORT_TABLE: u64 = 0x0E;

struct Writer<'o> {
    o: &'o Opts,
    /// The paper layout's paper size.
    paper: Option<(f64, f64)>,
    /// The paper layout's name.
    layout: Option<String>,
    next: u64,
    labels: BTreeMap<String, u64>,
    hmax: u64,
}

type Pairs = Vec<(i32, String)>;

fn p(out: &mut Pairs, code: i32, v: impl Into<String>) {
    out.push((code, v.into()));
}

fn pt(out: &mut Pairs, base: i32, xy: [f64; 2]) {
    p(out, base, num(xy[0]));
    p(out, base + 10, num(xy[1]));
    p(out, base + 20, "0.0");
}

impl<'o> Writer<'o> {
    fn new(o: &'o Opts) -> Self {
        Writer {
            o,
            paper: None,
            layout: None,
            next: 0x10,
            labels: BTreeMap::new(),
            hmax: 0,
        }
    }

    fn mint(&mut self) -> u64 {
        let h = self.next;
        self.next += 1;
        self.hmax = self.hmax.max(h);
        h
    }

    fn hex(&self, h: u64) -> String {
        let s = if self.o.lower_handles {
            format!("{h:x}")
        } else {
            format!("{h:X}")
        };
        if self.o.padded_handles {
            format!("{s:0>6}")
        } else {
            s
        }
    }

    /// A handle pair, when the drawing has handles; the handle (or 0 when none).
    fn handle(&mut self, out: &mut Pairs) -> u64 {
        if !self.o.handles {
            return 0;
        }
        let h = self.mint();
        let s = self.hex(h);
        p(out, 5, s);
        h
    }

    /// The objects a drawing from 2000 on holds whatever it draws: the root dictionary and its
    /// group, layout, multiline style, plot settings and plot style name dictionaries, the
    /// default plot style's placeholder, the standard multiline style, and the model and paper
    /// layouts.
    fn skeleton_objects(&mut self, out: &mut Pairs, model: u64, paper: u64) {
        let dict_head = |w: &mut Self, out: &mut Pairs, kind: &str, owner: u64| -> u64 {
            p(out, 0, kind);
            let h = w.handle(out);
            if owner != 0 {
                p(out, 102, "{ACAD_REACTORS");
                let o = w.hex(owner);
                p(out, 330, o.clone());
                p(out, 102, "}");
                p(out, 330, o);
            } else {
                p(out, 330, "0");
            }
            p(out, 100, "AcDbDictionary");
            p(out, 281, "1");
            h
        };
        // the handles of the five dictionaries the root names: minted in this order below
        let base = self.next + 1;
        let names = [
            "ACAD_GROUP",
            "ACAD_LAYOUT",
            "ACAD_MLINESTYLE",
            "ACAD_PLOTSETTINGS",
            "ACAD_PLOTSTYLENAME",
        ];
        let root = dict_head(self, out, "DICTIONARY", 0);
        for (i, n) in names.iter().enumerate() {
            p(out, 3, *n);
            let h = self.hex(base + i as u64);
            p(out, 350, h);
        }
        dict_head(self, out, "DICTIONARY", root);
        let layouts = dict_head(self, out, "DICTIONARY", root);
        let paper_name = self.layout.clone().unwrap_or_else(|| "Layout1".into());
        for (n, h) in [("Model", LAYOUT_MODEL), (paper_name.as_str(), LAYOUT_PAPER)] {
            p(out, 3, n);
            let h = self.hex(h);
            p(out, 350, h);
        }
        let mlines = dict_head(self, out, "DICTIONARY", root);
        let mls = self.next + 2;
        p(out, 3, "Standard");
        let m = self.hex(mls);
        p(out, 350, m);
        dict_head(self, out, "DICTIONARY", root);
        let styles = dict_head(self, out, "ACDBDICTIONARYWDFLT", root);
        p(out, 3, "Normal");
        let ph = self.hex(PLACEHOLDER);
        p(out, 350, ph.clone());
        p(out, 100, "AcDbDictionaryWithDefault");
        p(out, 340, ph.clone());
        // the placeholder (its handle reserved: layers point at it)
        p(out, 0, "ACDBPLACEHOLDER");
        p(out, 5, ph);
        p(out, 102, "{ACAD_REACTORS");
        let st = self.hex(styles);
        p(out, 330, st.clone());
        p(out, 102, "}");
        p(out, 330, st);
        p(out, 0, "MLINESTYLE");
        self.handle(out);
        p(out, 102, "{ACAD_REACTORS");
        let ml = self.hex(mlines);
        p(out, 330, ml.clone());
        p(out, 102, "}");
        p(out, 330, ml);
        for (c, v) in [
            (100, "AcDbMlineStyle"),
            (2, "Standard"),
            (70, "0"),
            (3, ""),
            (62, "256"),
            (51, "90.0"),
            (52, "90.0"),
            (71, "2"),
            (49, "0.5"),
            (62, "256"),
            (6, "BYLAYER"),
            (49, "-0.5"),
            (62, "256"),
            (6, "BYLAYER"),
        ] {
            p(out, c, v);
        }
        let size = self.paper;
        let paper_name = self.layout.clone().unwrap_or_else(|| "Layout1".into());
        for (name, h, owner, tab) in [
            ("Model", LAYOUT_MODEL, model, "0"),
            (paper_name.as_str(), LAYOUT_PAPER, paper, "1"),
        ] {
            let (w, ht) = match size {
                Some((w, ht)) if tab == "1" => (num(w), num(ht)),
                _ => ("0.0".to_owned(), "0.0".to_owned()),
            };
            p(out, 0, "LAYOUT");
            let hs = self.hex(h);
            p(out, 5, hs);
            p(out, 102, "{ACAD_REACTORS");
            let l = self.hex(layouts);
            p(out, 330, l.clone());
            p(out, 102, "}");
            p(out, 330, l);
            for (c, v) in [
                (100, "AcDbPlotSettings"),
                (1, ""),
                (2, "none_device"),
                (4, ""),
                (6, ""),
                (40, "0.0"),
                (41, "0.0"),
                (42, "0.0"),
                (43, "0.0"),
                (44, w.as_str()),
                (45, ht.as_str()),
                (46, "0.0"),
                (47, "0.0"),
                (48, "0.0"),
                (49, "0.0"),
                (140, "0.0"),
                (141, "0.0"),
                (142, "1.0"),
                (143, "1.0"),
                (70, "688"),
                (72, "0"),
                (73, "0"),
                (74, "5"),
                (7, ""),
                (75, "16"),
                (147, "1.0"),
                (148, "0.0"),
                (149, "0.0"),
                (100, "AcDbLayout"),
            ] {
                p(out, c, v);
            }
            p(out, 1, name);
            p(out, 70, "1");
            p(out, 71, tab);
            for (c, v) in [
                (10, "0.0"),
                (20, "0.0"),
                (11, "420.0"),
                (21, "297.0"),
                (12, "0.0"),
                (22, "0.0"),
                (32, "0.0"),
                (14, "0.0"),
                (24, "0.0"),
                (34, "0.0"),
                (15, "0.0"),
                (25, "0.0"),
                (35, "0.0"),
                (146, "0.0"),
                (13, "0.0"),
                (23, "0.0"),
                (33, "0.0"),
                (16, "1.0"),
                (26, "0.0"),
                (36, "0.0"),
                (17, "0.0"),
                (27, "1.0"),
                (37, "0.0"),
                (76, "0"),
            ] {
                p(out, c, v);
            }
            let o = self.hex(owner);
            p(out, 330, o);
        }
    }

    fn table_head(&mut self, out: &mut Pairs, name: &str, count: usize) -> u64 {
        p(out, 0, "TABLE");
        p(out, 2, name);
        let h = if self.o.version.modern() {
            let h = self.handle(out);
            p(out, 330, "0");
            p(out, 100, "AcDbSymbolTable");
            h
        } else {
            0
        };
        p(out, 70, count.to_string());
        h
    }

    fn entry_head(&mut self, out: &mut Pairs, kind: &str, owner: u64, sub: &str) {
        p(out, 0, kind);
        if self.o.version.modern() {
            self.handle(out);
            let o = self.hex(owner);
            p(out, 330, o);
            p(out, 100, "AcDbSymbolTableRecord");
            p(out, 100, sub);
        }
    }

    fn write(mut self, s: &Spec) -> Built {
        let modern = self.o.version.modern();
        let mut apps: Vec<String> = vec!["ACAD".into()];
        if modern && s.blocks.iter().any(|b| b.rep_of.is_some()) {
            apps.push(REP_APP.into());
        }
        let every = s
            .model
            .iter()
            .chain(&s.paper)
            .chain(s.blocks.iter().flat_map(|b| &b.ents));
        for e in every {
            for (c, v) in &e.xdata {
                if *c == 1001 && !apps.iter().any(|a| a.eq_ignore_ascii_case(v)) {
                    apps.push(v.clone());
                }
            }
        }
        for (c, v) in s.draw.layer_pairs.iter().flat_map(|(_, p)| p) {
            if *c == 1001 && !apps.iter().any(|a| a.eq_ignore_ascii_case(v)) {
                apps.push(v.clone());
            }
        }

        // TABLES
        let mut tables = Pairs::new();
        p(&mut tables, 0, "SECTION");
        p(&mut tables, 2, "TABLES");
        if modern {
            let vt = self.table_head(&mut tables, "VPORT", 1);
            self.entry_head(&mut tables, "VPORT", vt, "AcDbViewportTableRecord");
            p(&mut tables, 2, "*Active");
            p(&mut tables, 70, "0");
            for (c, v) in [
                (10, "0.0"),
                (20, "0.0"),
                (11, "1.0"),
                (21, "1.0"),
                (12, "0.0"),
                (22, "0.0"),
                (13, "0.0"),
                (23, "0.0"),
                (14, "10.0"),
                (24, "10.0"),
                (15, "10.0"),
                (25, "10.0"),
                (16, "0.0"),
                (26, "0.0"),
                (36, "1.0"),
                (17, "0.0"),
                (27, "0.0"),
                (37, "0.0"),
                (40, "100.0"),
                (41, "1.5"),
                (42, "50.0"),
                (43, "0.0"),
                (44, "0.0"),
                (50, "0.0"),
                (51, "0.0"),
                (71, "0"),
                (72, "100"),
                (73, "1"),
                (74, "3"),
                (75, "0"),
                (76, "0"),
                (77, "0"),
                (78, "0"),
            ] {
                p(&mut tables, c, v);
            }
            p(&mut tables, 0, "ENDTAB");
        }
        let lt = self.table_head(&mut tables, "LTYPE", 3 + s.draw.linetypes.len());
        for n in ["BYBLOCK", "BYLAYER", "CONTINUOUS"] {
            self.entry_head(&mut tables, "LTYPE", lt, "AcDbLinetypeTableRecord");
            p(&mut tables, 2, n);
            p(&mut tables, 70, "0");
            p(&mut tables, 3, "");
            p(&mut tables, 72, "65");
            p(&mut tables, 73, "0");
            p(&mut tables, 40, "0.0");
        }
        for (n, dashes) in &s.draw.linetypes {
            self.entry_head(&mut tables, "LTYPE", lt, "AcDbLinetypeTableRecord");
            p(&mut tables, 2, n.as_str());
            p(&mut tables, 70, "0");
            p(&mut tables, 3, "");
            p(&mut tables, 72, "65");
            p(&mut tables, 73, dashes.len().to_string());
            p(&mut tables, 40, num(dashes.iter().map(|d| d.abs()).sum()));
            for d in dashes {
                p(&mut tables, 49, num(*d));
                if modern {
                    p(&mut tables, 74, "0");
                }
            }
        }
        p(&mut tables, 0, "ENDTAB");
        let lh = self.table_head(&mut tables, "LAYER", s.layers.len());
        for l in &s.layers {
            self.entry_head(&mut tables, "LAYER", lh, "AcDbLayerTableRecord");
            p(&mut tables, 2, l.name.as_str());
            let flags = u8::from(l.frozen) | (u8::from(l.locked) << 2);
            p(&mut tables, 70, flags.to_string());
            p(&mut tables, 62, l.color.to_string());
            p(&mut tables, 6, l.ltype.as_str());
            let own_lw = s
                .draw
                .layer_pairs
                .iter()
                .any(|(n, p)| *n == l.name && p.iter().any(|(c, _)| *c == 370));
            if modern {
                if !own_lw {
                    p(&mut tables, 370, "-3");
                }
                let ph = self.hex(PLACEHOLDER);
                p(&mut tables, 390, ph);
            }
            for (name, pairs) in &s.draw.layer_pairs {
                if *name == l.name {
                    tables.extend(pairs.iter().cloned());
                }
            }
        }
        p(&mut tables, 0, "ENDTAB");
        let sh = self.table_head(&mut tables, "STYLE", 1 + s.draw.styles.len());
        self.entry_head(&mut tables, "STYLE", sh, "AcDbTextStyleTableRecord");
        p(&mut tables, 2, "Standard");
        p(&mut tables, 70, "0");
        p(&mut tables, 40, "0.0");
        p(&mut tables, 41, "1.0");
        p(&mut tables, 50, "0.0");
        p(&mut tables, 71, "0");
        p(&mut tables, 42, "2.5");
        p(&mut tables, 3, "txt");
        p(&mut tables, 4, "");
        for (name, file, family) in &s.draw.styles {
            self.entry_head(&mut tables, "STYLE", sh, "AcDbTextStyleTableRecord");
            p(&mut tables, 2, name.as_str());
            p(&mut tables, 70, "0");
            p(&mut tables, 40, "0.0");
            p(&mut tables, 41, "1.0");
            p(&mut tables, 50, "0.0");
            p(&mut tables, 71, "0");
            p(&mut tables, 42, "2.5");
            p(&mut tables, 3, file.as_str());
            p(&mut tables, 4, "");
            if let (Some(f), true) = (family, modern) {
                p(&mut tables, 1001, "ACAD");
                p(&mut tables, 1000, f.as_str());
                p(&mut tables, 1071, "0");
            }
        }
        p(&mut tables, 0, "ENDTAB");
        if modern {
            for t in ["VIEW", "UCS"] {
                self.table_head(&mut tables, t, 0);
                p(&mut tables, 0, "ENDTAB");
            }
        }
        let ah = self.table_head(&mut tables, "APPID", apps.len());
        for a in &apps {
            self.entry_head(&mut tables, "APPID", ah, "AcDbRegAppTableRecord");
            p(&mut tables, 2, a.as_str());
            p(&mut tables, 70, "0");
        }
        p(&mut tables, 0, "ENDTAB");
        if modern {
            p(&mut tables, 0, "TABLE");
            p(&mut tables, 2, "DIMSTYLE");
            let dh = self.handle(&mut tables);
            p(&mut tables, 330, "0");
            p(&mut tables, 100, "AcDbSymbolTable");
            p(&mut tables, 70, "1");
            p(&mut tables, 100, "AcDbDimStyleTable");
            p(&mut tables, 71, "0");
            p(&mut tables, 0, "DIMSTYLE");
            let h = self.mint();
            let hs = self.hex(h);
            p(&mut tables, 105, hs);
            let o = self.hex(dh);
            p(&mut tables, 330, o);
            p(&mut tables, 100, "AcDbSymbolTableRecord");
            p(&mut tables, 100, "AcDbDimStyleTableRecord");
            p(&mut tables, 2, "Standard");
            p(&mut tables, 70, "0");
            p(&mut tables, 0, "ENDTAB");
        }
        // block records: model, paper, then each block
        let mut records: Vec<(String, u64)> = Vec::new();
        if modern {
            let names: Vec<String> = ["*Model_Space", "*Paper_Space"]
                .iter()
                .map(|n| (*n).to_owned())
                .chain(s.blocks.iter().map(|b| b.name.clone()))
                .collect();
            let bh = self.table_head(&mut tables, "BLOCK_RECORD", names.len());
            for n in names {
                p(&mut tables, 0, "BLOCK_RECORD");
                let h = self.handle(&mut tables);
                if n == "*Model_Space" && !s.draw.sortents.is_empty() {
                    p(&mut tables, 102, "{ACAD_XDICTIONARY");
                    let d = self.hex(SORT_DICT);
                    p(&mut tables, 360, d);
                    p(&mut tables, 102, "}");
                }
                let o = self.hex(bh);
                p(&mut tables, 330, o);
                p(&mut tables, 100, "AcDbSymbolTableRecord");
                p(&mut tables, 100, "AcDbBlockTableRecord");
                p(&mut tables, 2, n.as_str());
                let layout = match n.as_str() {
                    "*Model_Space" => self.hex(LAYOUT_MODEL),
                    "*Paper_Space" => self.hex(LAYOUT_PAPER),
                    _ => "0".into(),
                };
                p(&mut tables, 340, layout);
                p(&mut tables, 70, "0");
                p(&mut tables, 280, "1");
                p(&mut tables, 281, "0");
                if let Some(of) = s
                    .blocks
                    .iter()
                    .find(|b| b.name == n)
                    .and_then(|b| b.rep_of.as_ref())
                {
                    // the record it names may come later: its handle is filled in below
                    p(&mut tables, 1001, REP_APP);
                    p(&mut tables, 1070, "1");
                    p(&mut tables, 1005, format!("{REP_MARK}{of}"));
                }
                records.push((n, h));
            }
            p(&mut tables, 0, "ENDTAB");
            for pair in tables.iter_mut().filter(|pair| pair.0 == 1005) {
                if let Some(of) = pair.1.strip_prefix(REP_MARK) {
                    let h = records.iter().find(|(n, _)| n == of).map_or(0, |(_, h)| *h);
                    pair.1 = self.hex(h);
                }
            }
        }
        p(&mut tables, 0, "ENDSEC");
        let rec = |name: &str| -> u64 {
            records
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, h)| *h)
                .unwrap_or(0)
        };

        // BLOCKS
        let mut blocks = Pairs::new();
        p(&mut blocks, 0, "SECTION");
        p(&mut blocks, 2, "BLOCKS");
        // each definition: its name, base point, entities, flags and path
        type Def = (String, [f64; 2], Vec<Ent>, i32, String);
        let mut defs: Vec<Def> = Vec::new();
        if modern {
            defs.push((
                "*Model_Space".into(),
                [0.0, 0.0],
                Vec::new(),
                0,
                String::new(),
            ));
            defs.push((
                "*Paper_Space".into(),
                [0.0, 0.0],
                Vec::new(),
                0,
                String::new(),
            ));
        }
        defs.extend(s.blocks.iter().map(|b| {
            (
                b.name.clone(),
                b.base,
                b.ents.clone(),
                b.flags,
                b.path.clone(),
            )
        }));
        for (name, base, ents, flags, path) in &defs {
            let owner = rec(name);
            p(&mut blocks, 0, "BLOCK");
            self.handle(&mut blocks);
            if modern {
                let o = self.hex(owner);
                p(&mut blocks, 330, o);
                p(&mut blocks, 100, "AcDbEntity");
            }
            p(&mut blocks, 8, "0");
            if modern {
                p(&mut blocks, 100, "AcDbBlockBegin");
            }
            p(&mut blocks, 2, name.as_str());
            p(&mut blocks, 70, flags.to_string());
            pt(&mut blocks, 10, *base);
            p(&mut blocks, 3, name.as_str());
            p(&mut blocks, 1, path.as_str());
            for e in ents {
                self.entity(&mut blocks, e, owner, false);
            }
            p(&mut blocks, 0, "ENDBLK");
            self.handle(&mut blocks);
            if modern {
                let o = self.hex(owner);
                p(&mut blocks, 330, o);
                p(&mut blocks, 100, "AcDbEntity");
            }
            p(&mut blocks, 8, "0");
            if modern {
                p(&mut blocks, 100, "AcDbBlockEnd");
            }
        }
        p(&mut blocks, 0, "ENDSEC");

        // ENTITIES
        let mut ents = Pairs::new();
        p(&mut ents, 0, "SECTION");
        p(&mut ents, 2, "ENTITIES");
        if self.o.comments {
            p(&mut ents, 999, "entities follow");
        }
        let (ms, ps) = (rec("*Model_Space"), rec("*Paper_Space"));
        for e in &s.model {
            self.entity(&mut ents, e, ms, false);
        }
        for e in &s.paper {
            self.entity(&mut ents, e, ps, true);
        }
        p(&mut ents, 0, "ENDSEC");

        // OBJECTS (R2000 and later)
        let mut objects = Pairs::new();
        if modern {
            p(&mut objects, 0, "SECTION");
            p(&mut objects, 2, "OBJECTS");
            self.skeleton_objects(&mut objects, rec("*Model_Space"), rec("*Paper_Space"));
            if !s.draw.sortents.is_empty() {
                let ms = self.hex(rec("*Model_Space"));
                let (d, t) = (self.hex(SORT_DICT), self.hex(SORT_TABLE));
                p(&mut objects, 0, "DICTIONARY");
                p(&mut objects, 5, d.clone());
                p(&mut objects, 330, ms.clone());
                p(&mut objects, 100, "AcDbDictionary");
                p(&mut objects, 280, "1");
                p(&mut objects, 281, "1");
                p(&mut objects, 3, "ACAD_SORTENTS");
                p(&mut objects, 360, t.clone());
                p(&mut objects, 0, "SORTENTSTABLE");
                p(&mut objects, 5, t);
                p(&mut objects, 102, "{ACAD_REACTORS");
                p(&mut objects, 330, d.clone());
                p(&mut objects, 102, "}");
                p(&mut objects, 330, d);
                p(&mut objects, 100, "AcDbSortentsTable");
                p(&mut objects, 330, ms);
                for (e, k) in &s.draw.sortents {
                    let (e, k) = (self.hex(*e), self.hex(*k));
                    p(&mut objects, 331, e);
                    p(&mut objects, 5, k);
                }
            }
            for (h, path) in &s.draw.images {
                self.hmax = self.hmax.max(*h);
                let hs = self.hex(*h);
                p(&mut objects, 0, "IMAGEDEF");
                p(&mut objects, 5, hs);
                p(&mut objects, 100, "AcDbRasterImageDef");
                p(&mut objects, 90, "0");
                p(&mut objects, 1, path.as_str());
                p(&mut objects, 10, "100.0");
                p(&mut objects, 20, "50.0");
                p(&mut objects, 11, "1.0");
                p(&mut objects, 21, "1.0");
                p(&mut objects, 280, "1");
                p(&mut objects, 281, "0");
            }
            for (kind, pairs) in &s.objects_extra {
                p(&mut objects, 0, kind.as_str());
                self.handle(&mut objects);
                objects.extend(pairs.iter().cloned());
            }
            p(&mut objects, 0, "ENDSEC");
        }

        // HEADER, last, so $HANDSEED knows the highest handle
        let mut header = Pairs::new();
        p(&mut header, 0, "SECTION");
        p(&mut header, 2, "HEADER");
        p(&mut header, 9, "$ACADVER");
        p(&mut header, 1, self.o.version.acadver());
        if modern {
            p(&mut header, 9, "$DWGCODEPAGE");
            p(&mut header, 3, self.o.codepage.as_str());
        }
        if !modern {
            p(&mut header, 9, "$HANDLING");
            p(&mut header, 70, if self.o.handles { "1" } else { "0" });
        }
        if self.o.handles {
            let seed = self.o.handseed.unwrap_or(self.hmax + 1);
            let s = self.hex(seed);
            p(&mut header, 9, "$HANDSEED");
            p(&mut header, 5, s);
        }
        if let Some(u) = self.o.insunits.filter(|_| modern) {
            p(&mut header, 9, "$INSUNITS");
            p(&mut header, 70, u.to_string());
        }
        if let Some(m) = self.o.measurement {
            p(&mut header, 9, "$MEASUREMENT");
            p(&mut header, 70, m.to_string());
        }
        if let Some((lo, hi)) = self.o.extents {
            p(&mut header, 9, "$EXTMIN");
            pt(&mut header, 10, lo);
            p(&mut header, 9, "$EXTMAX");
            pt(&mut header, 10, hi);
        }
        p(&mut header, 9, "$CLAYER");
        p(&mut header, 8, "0");
        if let Some(n) = self.o.lastsavedby.as_ref().filter(|_| modern) {
            p(&mut header, 9, "$LASTSAVEDBY");
            p(&mut header, 1, n.as_str());
        }
        for (name, pairs) in &s.header_extra {
            p(&mut header, 9, name.as_str());
            header.extend(pairs.iter().cloned());
        }
        p(&mut header, 0, "ENDSEC");

        let mut all = Pairs::new();
        if self.o.comments {
            p(&mut all, 999, "generated test drawing");
        }
        all.extend(header);
        if modern {
            all.push((0, "SECTION".into()));
            all.push((2, "CLASSES".into()));
            all.push((0, "ENDSEC".into()));
        }
        all.extend(tables);
        all.extend(blocks);
        all.extend(ents);
        all.extend(objects);
        if self.o.eof {
            all.push((0, "EOF".into()));
        }
        Built {
            bytes: self.serialize(&all),
            labels: self.labels,
            hmax: self.hmax,
        }
    }

    fn serialize(&self, pairs: &Pairs) -> Vec<u8> {
        let mut out = Vec::new();
        if self.o.bom {
            out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        let mut line = 0usize;
        let mut eol = |out: &mut Vec<u8>| {
            let crlf = match self.o.eol {
                Eol::Lf => false,
                Eol::CrLf => true,
                Eol::Mixed => line % 2 == 1,
            };
            out.extend_from_slice(if crlf { b"\r\n" } else { b"\n" });
            line += 1;
        };
        for (c, v) in pairs {
            if self.o.pad {
                out.extend_from_slice(format!("{c:>3}").as_bytes());
            } else {
                out.extend_from_slice(c.to_string().as_bytes());
            }
            eol(&mut out);
            out.extend_from_slice(v.as_bytes());
            eol(&mut out);
        }
        out.extend_from_slice(&self.o.tail);
        out
    }

    /// One entity (and its followers) into `out`, owned by `owner`.
    fn entity(&mut self, out: &mut Pairs, e: &Ent, owner: u64, paper: bool) {
        // R12 has neither lightweight polylines nor multiline text: write them as its own kinds,
        // a polyline with vertices and a single-line text
        if !self.o.version.modern() {
            let own = match &e.shape {
                Shape::LwPoly(v, closed) => Some(Shape::Poly(v.clone(), *closed)),
                Shape::MText(at, h, t) => Some(Shape::Text(*at, *h, t.clone())),
                _ => None,
            };
            if let Some(shape) = own {
                let mut r12 = e.clone();
                r12.shape = shape;
                return self.entity(out, &r12, owner, paper);
            }
        }
        let modern = self.o.version.modern();
        let kind = match &e.shape {
            Shape::Line(..) => "LINE",
            Shape::Point(..) => "POINT",
            Shape::Circle(..) => "CIRCLE",
            Shape::Arc(..) => "ARC",
            Shape::LwPoly(..) => "LWPOLYLINE",
            Shape::Poly(..) => "POLYLINE",
            Shape::Text(..) => "TEXT",
            Shape::MText(..) => "MTEXT",
            Shape::Insert(..) => "INSERT",
            Shape::Raw(k, _) => k.as_str(),
        };
        p(out, 0, kind);
        let h = self.common(out, e, owner, paper);
        if let Some(l) = &e.label {
            self.labels.insert(l.clone(), h);
        }
        let sub = |out: &mut Pairs, s: &str| {
            if modern {
                p(out, 100, s);
            }
        };
        match &e.shape {
            Shape::Line(a, b) => {
                sub(out, "AcDbLine");
                pt(out, 10, *a);
                pt(out, 11, *b);
            }
            Shape::Point(a) => {
                sub(out, "AcDbPoint");
                pt(out, 10, *a);
            }
            Shape::Circle(c, r) => {
                sub(out, "AcDbCircle");
                pt(out, 10, *c);
                p(out, 40, num(*r));
            }
            Shape::Arc(c, r, s0, s1) => {
                sub(out, "AcDbCircle");
                pt(out, 10, *c);
                p(out, 40, num(*r));
                sub(out, "AcDbArc");
                p(out, 50, num(*s0));
                p(out, 51, num(*s1));
            }
            Shape::LwPoly(v, closed) => {
                // the planted bug: a writer that leaves out a kind's subclass marker
                if !crate::mutant::active("SubclassDropped") {
                    sub(out, "AcDbPolyline");
                }
                p(out, 90, v.len().to_string());
                p(out, 70, if *closed { "1" } else { "0" });
                for (xy, bulge) in v {
                    p(out, 10, num(xy[0]));
                    p(out, 20, num(xy[1]));
                    if *bulge != 0.0 {
                        p(out, 42, num(*bulge));
                    }
                }
            }
            Shape::Poly(v, closed) => {
                sub(out, "AcDb2dPolyline");
                p(out, 66, "1");
                pt(out, 10, [0.0, 0.0]);
                p(out, 70, if *closed { "1" } else { "0" });
                self.extrusion(out, e);
                out.extend(e.xdata.iter().cloned());
                for (xy, bulge) in v {
                    p(out, 0, "VERTEX");
                    let ph = if h == 0 { owner } else { h };
                    self.common(out, &Ent::of(Shape::Point(*xy)).on(&e.layer), ph, paper);
                    sub(out, "AcDbVertex");
                    sub(out, "AcDb2dVertex");
                    pt(out, 10, *xy);
                    if *bulge != 0.0 {
                        p(out, 42, num(*bulge));
                    }
                }
                p(out, 0, "SEQEND");
                let ph = if h == 0 { owner } else { h };
                self.common(
                    out,
                    &Ent::of(Shape::Point([0.0, 0.0])).on(&e.layer),
                    ph,
                    paper,
                );
            }
            Shape::Text(at, hgt, s) => {
                sub(out, "AcDbText");
                pt(out, 10, *at);
                p(out, 40, num(*hgt));
                p(out, 1, s.as_str());
                sub(out, "AcDbText");
            }
            Shape::MText(at, hgt, s) => {
                sub(out, "AcDbMText");
                pt(out, 10, *at);
                p(out, 40, num(*hgt));
                p(out, 41, "0.0");
                p(out, 71, "1");
                // long text goes in 250-byte `3` chunks, the rest in `1`
                let b = s.as_str();
                let mut rest = b;
                while rest.len() > 250 {
                    let mut cut = 250;
                    while !rest.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    p(out, 3, &rest[..cut]);
                    rest = &rest[cut..];
                }
                p(out, 1, rest);
            }
            Shape::Insert(block, at, attribs) => {
                sub(out, "AcDbBlockReference");
                if !attribs.is_empty() {
                    p(out, 66, "1");
                }
                p(out, 2, block.as_str());
                pt(out, 10, *at);
                if let Some([sx, sy, rot]) = e.xform {
                    p(out, 41, num(sx));
                    p(out, 42, num(sy));
                    p(out, 43, "1.0");
                    p(out, 50, num(rot));
                }
                self.extrusion(out, e);
                out.extend(e.xdata.iter().cloned());
                if !attribs.is_empty() {
                    let ph = if h == 0 { owner } else { h };
                    for (tag, val) in attribs {
                        p(out, 0, "ATTRIB");
                        self.common(out, &Ent::of(Shape::Point(*at)).on(&e.layer), ph, paper);
                        sub(out, "AcDbText");
                        pt(out, 10, *at);
                        p(out, 40, "2.5");
                        p(out, 1, val.as_str());
                        sub(out, "AcDbAttribute");
                        p(out, 2, tag.as_str());
                        p(out, 70, "0");
                    }
                    p(out, 0, "SEQEND");
                    self.common(out, &Ent::of(Shape::Point(*at)).on(&e.layer), ph, paper);
                }
            }
            Shape::Raw(_, pairs) => {
                out.extend(pairs.iter().cloned());
            }
        }
        if !matches!(e.shape, Shape::Poly(..) | Shape::Insert(..)) {
            self.extrusion(out, e);
            out.extend(e.xdata.iter().cloned());
        }
    }

    fn extrusion(&mut self, out: &mut Pairs, e: &Ent) {
        if let Some(n) = e.extrusion {
            p(out, 210, num(n[0]));
            p(out, 220, num(n[1]));
            p(out, 230, num(n[2]));
        }
    }

    /// The common group: handle, owner, layer, linetype, colour. Returns the handle (0: none).
    fn common(&mut self, out: &mut Pairs, e: &Ent, owner: u64, paper: bool) -> u64 {
        let modern = self.o.version.modern();
        let h = if let Some(h) = e.handle {
            self.hmax = self.hmax.max(h);
            let s = self.hex(h);
            p(out, 5, s);
            h
        } else if e.no_handle {
            0
        } else {
            self.handle(out)
        };
        if modern {
            if owner != 0 {
                let o = self.hex(owner);
                p(out, 330, o);
            }
            p(out, 100, "AcDbEntity");
        }
        if paper {
            p(out, 67, "1");
        }
        p(out, 8, e.layer.as_str());
        if let Some(t) = &e.ltype {
            p(out, 6, t.as_str());
        }
        if let Some(c) = e.color {
            p(out, 62, c.to_string());
        }
        out.extend(e.props.iter().cloned());
        h
    }
}

/// A drawing with every kind the index reads, on a few layers, with a nested block and a
/// paper-space entity: the sampler most rows start from.
pub fn sampler(version: Ver) -> Spec {
    Spec::new(version)
        .layer("A-WALL", 1)
        .layer("E-LITE", 3)
        .layer("ANNO", 7)
        .block(
            "DOOR",
            [0.0, 0.0],
            vec![
                Ent::line([0.0, 0.0], [0.9, 0.0])
                    .on("A-WALL")
                    .label("door-leaf"),
                Ent::arc([0.0, 0.0], 0.9, 0.0, 90.0).on("A-WALL"),
            ],
        )
        .model(
            Ent::line([0.0, 0.0], [10.0, 0.0])
                .on("A-WALL")
                .label("wall"),
        )
        .model(Ent::point([5.0, 5.0]).on("E-LITE"))
        .model(
            Ent::circle([20.0, 20.0], 2.0)
                .on("E-LITE")
                .color(1)
                .label("lamp"),
        )
        .model(Ent::arc([30.0, 0.0], 5.0, 0.0, 180.0).on("A-WALL"))
        .model(
            Ent::lwpoly(
                &[([0.0, 0.0], 0.0), ([10.0, 0.0], 1.0), ([10.0, 10.0], 0.0)],
                false,
            )
            .on("A-WALL")
            .label("lw"),
        )
        .model(
            Ent::poly(
                &[([0.0, 0.0], 0.0), ([4.0, 0.0], 0.0), ([4.0, 3.0], 0.0)],
                true,
            )
            .on("A-WALL")
            .label("poly"),
        )
        .model(
            Ent::text([1.0, 1.0], 2.5, "ROOM 101")
                .on("ANNO")
                .label("room"),
        )
        .model(Ent::mtext([1.0, 5.0], 2.5, "North wall\\Ptwo lines").on("ANNO"))
        .model(
            Ent::insert("DOOR", [10.0, 0.0])
                .on("A-WALL")
                .attrib("NO", "D-01")
                .label("door1"),
        )
        .paper(
            Ent::text([10.0, 10.0], 3.5, "TITLE")
                .on("ANNO")
                .label("title"),
        )
}

/// Block references and a leader as the reference reads resolve them (2018): a dynamic block
/// placed directly and through its anonymous block, an external reference, a straight run
/// placed scaled and turned, and a leader to a multiline text. For the checker's set.
pub fn references_sampler() -> Spec {
    Spec::new(Ver::R2018)
        .block("CAMERA", [0.0, 0.0], vec![Ent::circle([0.0, 0.0], 0.5)])
        .dynamic(
            "*U5",
            [0.0, 0.0],
            vec![Ent::circle([0.0, 0.0], 0.75)],
            "CAMERA",
        )
        .block(
            "RUN",
            [0.0, 0.0],
            vec![
                Ent::line([0.0, -0.1], [1.0, -0.1]),
                Ent::line([0.0, 0.1], [1.0, 0.1]),
            ],
        )
        .xref("BACKDROP", "../refs/backdrop.dxf")
        .model(Ent::insert("CAMERA", [0.0, 0.0]).attrib("DEVICE", "CAM-01"))
        .model(Ent::insert("*U5", [10.0, 0.0]).attrib("DEVICE", "CAM-02"))
        .model(Ent::insert("BACKDROP", [0.0, 0.0]))
        .model(
            Ent::insert("RUN", [10.0, 10.0])
                .placed(20.0, 1.0, 90.0)
                .attrib("SIZE", "1"),
        )
        .model(Ent::mtext([16.0, 16.0], 0.5, "C-301").handle(0x500))
        .model(Ent::leader(&[[5.0, 5.0], [15.0, 15.0]], Some(0x500)))
}

/// A multileader with one leader and its own text (2018). For the checker's set.
pub fn mleader_sampler() -> Spec {
    Spec::new(Ver::R2018).model(Ent::mleader(
        &[[10.0, 5.0], [20.0, 20.0]],
        [28.0, 28.0],
        [30.0, 30.0],
        "C-201",
    ))
}

/// Blocks nested three deep, inserted from model and paper space, with known counts:
/// `A` holds 2 `B`, `B` holds 1 `C`, `C` holds a line, `D` is never inserted. Model space holds
/// 2 `A` and 1 `C`; paper space 1 `B`. Direct inserts: A model 2; B paper 1 and 2 in blocks;
/// C model 1 and 1 in blocks; D none. Nested from model space: A 2, B 4, C 5.
pub fn nested_blocks(version: Ver) -> Spec {
    Spec::new(version)
        .layer("L1", 1)
        .layer("L2", 2)
        .layer("UNUSED", 3)
        .block(
            "C",
            [0.0, 0.0],
            vec![Ent::line([0.0, 0.0], [1.0, 0.0]).on("L2")],
        )
        .block("B", [0.0, 0.0], vec![Ent::insert("C", [0.0, 0.0]).on("L1")])
        .block(
            "A",
            [0.0, 0.0],
            vec![
                Ent::insert("B", [0.0, 0.0]).on("L1"),
                Ent::insert("B", [5.0, 0.0]).on("L1"),
            ],
        )
        .block("D", [0.0, 0.0], vec![Ent::circle([0.0, 0.0], 1.0).on("L2")])
        .model(Ent::insert("A", [0.0, 0.0]).on("L1"))
        .model(Ent::insert("A", [100.0, 0.0]).on("L1"))
        .model(Ent::insert("C", [200.0, 0.0]).on("L2"))
        .model(Ent::line([0.0, 0.0], [1.0, 1.0]).on("L2"))
        .paper(Ent::insert("B", [0.0, 0.0]).on("L1"))
}

/// `n` short lines on four layers in model space, in a 100-wide grid.
pub fn sized(version: Ver, n: usize) -> Spec {
    let layers = ["A-WALL", "E-LITE", "ANNO", "A-DOOR"];
    let mut s = Spec::new(version)
        .layer("A-WALL", 1)
        .layer("E-LITE", 3)
        .layer("ANNO", 7)
        .layer("A-DOOR", 2);
    for i in 0..n {
        let (x, y) = ((i % 100) as f64 * 2.0, (i / 100) as f64 * 2.0);
        s.model
            .push(Ent::line([x, y], [x + 1.0, y]).on(layers[i % layers.len()]));
    }
    s
}

/// `n` entities in model space, half of them text as a drawing's tags, labels and notes are:
/// lines, single-line texts (left and centred), and two-line multiline texts at a centre
/// attachment, by turns, in a 100-wide grid.
pub fn sized_text(version: Ver, n: usize) -> Spec {
    let mut s = Spec::new(version)
        .layer("A-WALL", 1)
        .layer("ANNO", 7)
        .layer("TAGS", 3);
    for i in 0..n {
        let (x, y) = ((i % 100) as f64 * 2.0, (i / 100) as f64 * 2.0);
        let e = match i % 4 {
            0 | 2 => Ent::line([x, y], [x + 1.0, y]).on("A-WALL"),
            1 if i % 8 == 1 => Ent::text([x, y], 0.25, &format!("TAG-{i}")).on("TAGS"),
            1 => {
                let (xs, ys, t) = (num(x), num(y), format!("LABEL {i}"));
                Ent::raw(
                    "TEXT",
                    &[
                        (100, "AcDbText"),
                        (10, xs.as_str()),
                        (20, ys.as_str()),
                        (30, "0.0"),
                        (40, "0.25"),
                        (1, t.as_str()),
                        (72, "1"),
                        (11, xs.as_str()),
                        (21, ys.as_str()),
                        (31, "0.0"),
                        (100, "AcDbText"),
                        (73, "2"),
                    ],
                )
                .on("TAGS")
            }
            _ => Ent::mtext([x, y], 0.2, &format!("Note {i}\\Psecond line")).on("ANNO"),
        };
        s.model.push(e);
    }
    s
}

/// The byte-level variants of a drawing every tokenizer row runs over: each line ending,
/// padded and unpadded codes, comments, a byte-order mark, a tail after the end marker, and
/// handle spellings. Each is `(name, bytes)`.
pub fn byte_variants() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for v in Ver::ALL {
        for eol in [Eol::Lf, Eol::CrLf, Eol::Mixed] {
            for pad in [false, true] {
                let mut s = sampler(v);
                s.opts.eol = eol;
                s.opts.pad = pad;
                s.opts.comments = pad;
                s.opts.bom = eol == Eol::CrLf && v.modern();
                if eol == Eol::Mixed {
                    s.opts.tail = b"\x1a\x00trailing bytes\r\n".to_vec();
                }
                s.opts.lower_handles = eol == Eol::Mixed;
                s.opts.padded_handles = pad && v == Ver::R2018;
                out.push((format!("{}-{eol:?}-pad{pad}", v.acadver()), s.build().bytes));
            }
        }
    }
    out
}

/// The drawings our handle-stability measurements start from, by name.
pub const H1_NAMES: [&str; 6] = [
    "h1-r12-handles",
    "h1-r2000",
    "h1-r2004",
    "h1-r2010",
    "h1-r2018",
    "h1-dense",
];

/// The application name of the ground-truth serial each H1 entity carries in its extension
/// data (`1071`), so a re-saved entity pairs with its original exactly where a writer keeps
/// extension data.
pub const SERIAL_APP: &str = "GTSERIAL";

/// Numbers an entity with its ground-truth serial and places it by its serial, so no two
/// entities share coordinates (they pair by geometry where a writer drops extension data).
fn serial(e: Ent, n: u32) -> Ent {
    let v = n.to_string();
    e.xdata(SERIAL_APP, &[(1071, v.as_str())])
}

/// Where entity `n` sits: a grid cell of its own, offset by a fraction unique to it.
fn at(n: u32) -> [f64; 2] {
    let n = f64::from(n);
    [
        (n % 20.0) * 25.0 + n * 0.0137,
        (n / 20.0).floor() * 25.0 + n * 0.0071,
    ]
}

fn plus(a: [f64; 2], dx: f64, dy: f64) -> [f64; 2] {
    [a[0] + dx, a[1] + dy]
}

/// The H1 drawing `name` ([`H1_NAMES`]), or `None` for another name.
///
/// Each holds, numbered by serial: lines, a point, circles, arcs, a bulged lightweight polyline
/// (from 2000 on), a polyline with vertices, text, multiline text (from 2000 on), an ellipse and
/// a spline (from 2000 on), a circle in a mirrored frame, inserts of a block with attributes and
/// of a block that nests another. **Two entities carry a text label beside them**: the line
/// labelled `MOVE-ME` (serial 7) and the line labelled `DELETE-ME` (serial 3, early in the
/// order, so a writer that renumbers by position shifts every later handle). `h1-dense` holds
/// 10,000 lines besides.
pub fn h1(name: &str) -> Option<Spec> {
    let version = match name {
        "h1-r12-handles" => Ver::R12,
        "h1-r2000" => Ver::R2000,
        "h1-r2004" => Ver::R2004,
        "h1-r2010" => Ver::R2010,
        "h1-r2018" | "h1-dense" => Ver::R2018,
        _ => return None,
    };
    let modern = version != Ver::R12;
    let mut o = Opts::new(version);
    o.eol = Eol::CrLf;
    o.comments = true;
    let mut s = Spec::with(o)
        .layer("WALLS", 1)
        .layer("DOORS", 2)
        .layer("NOTES", 7)
        .layer("EXTRA", 3)
        .block(
            "PANE",
            [0.0, 0.0],
            vec![serial(Ent::line([0.0, 0.0], [1.0, 0.0]).on("EXTRA"), 901)],
        )
        .block(
            "WIN",
            [0.0, 0.0],
            vec![
                serial(Ent::insert("PANE", [0.0, 0.0]).on("EXTRA"), 902),
                serial(Ent::insert("PANE", [0.0, 0.5]).on("EXTRA"), 903),
            ],
        )
        .block(
            "DOOR",
            [0.0, 0.0],
            vec![
                serial(Ent::line([0.0, 0.0], [0.9, 0.0]).on("DOORS"), 904),
                serial(Ent::arc([0.0, 0.0], 0.9, 0.0, 90.0).on("DOORS"), 905),
            ],
        );
    let mut n = 0u32;
    let mut add = |s: &mut Spec, e: Ent| {
        n += 1;
        s.model.push(serial(e, n));
        n
    };
    // serials 1 and 2: plain lines; 3: DELETE-ME; 4: its label
    add(&mut s, Ent::line(at(1), plus(at(1), 10.0, 0.0)).on("WALLS"));
    add(&mut s, Ent::line(at(2), plus(at(2), 0.0, 10.0)).on("WALLS"));
    add(
        &mut s,
        Ent::line(at(3), plus(at(3), 10.0, 5.0))
            .on("WALLS")
            .label("delete-me"),
    );
    add(
        &mut s,
        Ent::text(plus(at(3), 0.0, 6.0), 2.0, "DELETE-ME").on("NOTES"),
    );
    add(&mut s, Ent::point(at(5)).on("EXTRA"));
    add(&mut s, Ent::circle(at(6), 3.0).on("EXTRA"));
    // 7: MOVE-ME; 8: its label
    add(
        &mut s,
        Ent::line(at(7), plus(at(7), 10.0, 0.0))
            .on("WALLS")
            .label("move-me"),
    );
    add(
        &mut s,
        Ent::text(plus(at(7), 0.0, 2.0), 2.0, "MOVE-ME").on("NOTES"),
    );
    add(&mut s, Ent::arc(at(9), 4.0, 30.0, 300.0).on("EXTRA"));
    add(
        &mut s,
        Ent::poly(
            &[
                (at(10), 0.0),
                (plus(at(10), 8.0, 0.0), 0.5),
                (plus(at(10), 8.0, 6.0), 0.0),
            ],
            false,
        )
        .on("WALLS"),
    );
    add(&mut s, Ent::text(at(11), 2.5, "ROOM 1").on("NOTES"));
    add(
        &mut s,
        Ent::circle(at(12), 2.0)
            .extrusion([0.0, 0.0, -1.0])
            .on("EXTRA"),
    );
    add(
        &mut s,
        Ent::insert("DOOR", at(13))
            .on("DOORS")
            .attrib("NO", "D-01")
            .label("door"),
    );
    add(&mut s, Ent::insert("WIN", at(14)).on("EXTRA"));
    if modern {
        add(
            &mut s,
            Ent::lwpoly(
                &[
                    (at(15), 0.0),
                    (plus(at(15), 10.0, 0.0), 1.0),
                    (plus(at(15), 10.0, 8.0), 0.0),
                ],
                true,
            )
            .on("WALLS"),
        );
        add(
            &mut s,
            Ent::mtext(at(16), 2.5, "Line one\\PLine two").on("NOTES"),
        );
        let c = at(17);
        let (cx, cy) = (num(c[0]), num(c[1]));
        add(
            &mut s,
            Ent::raw(
                "ELLIPSE",
                &[
                    (100, "AcDbEllipse"),
                    (10, cx.as_str()),
                    (20, cy.as_str()),
                    (30, "0.0"),
                    (11, "6.0"),
                    (21, "0.0"),
                    (31, "0.0"),
                    (40, "0.5"),
                    (41, "0.0"),
                    (42, "6.283185307179586"),
                ],
            )
            .on("EXTRA"),
        );
        let c = at(18);
        let pts: Vec<String> = (0..4)
            .flat_map(|k| {
                let kf = f64::from(k);
                [num(c[0] + kf * 3.0), num(c[1] + (kf * 1.7).sin() * 2.0)]
            })
            .collect();
        let mut pairs: Vec<(i32, &str)> = vec![
            (100, "AcDbSpline"),
            (210, "0.0"),
            (220, "0.0"),
            (230, "1.0"),
            (70, "8"),
            (71, "3"),
            (72, "8"),
            (73, "4"),
            (74, "0"),
            (40, "0.0"),
            (40, "0.0"),
            (40, "0.0"),
            (40, "0.0"),
            (40, "1.0"),
            (40, "1.0"),
            (40, "1.0"),
            (40, "1.0"),
        ];
        for k in 0..4 {
            pairs.push((10, pts[2 * k].as_str()));
            pairs.push((20, pts[2 * k + 1].as_str()));
            pairs.push((30, "0.0"));
        }
        add(&mut s, Ent::raw("SPLINE", &pairs).on("EXTRA"));
    }
    if name == "h1-dense" {
        for k in 0..10_000u32 {
            let x = f64::from(k % 100) * 3.0 + 1000.0 + f64::from(k) * 0.0003;
            let y = f64::from(k / 100) * 3.0 + 1000.0;
            add(&mut s, Ent::line([x, y], [x + 2.0, y + 0.5]).on("EXTRA"));
        }
    }
    Some(s)
}

/// Our own splice of `bytes`: the entity with handle `h` moved by `dx` along x, by rewriting
/// only the value lines of its points (codes 10 and 11), every other byte as it was. A
/// stand-in for the edit verbs' output, for the "re-save a file our tool edited" measurement.
///
/// # Panics
/// When the bytes don't read, or no entity has that handle.
pub fn splice_move(bytes: &[u8], h: u64, dx: f64) -> Vec<u8> {
    let limits = crate::Limits::default();
    let t = crate::token::tokenize(bytes, &limits).expect("our drawing reads");
    let s = crate::structure::walk(t, &limits).expect("our drawing reads");
    let e = s
        .entities
        .iter()
        .find(|e| {
            (e.head.first + 1..e.head.end)
                .find(|&i| s.tok.pairs[i].code == 5)
                .and_then(|i| crate::handle::parse(s.tok.value(i)))
                == Some(h)
        })
        .expect("the entity to move");
    let mut out = Vec::with_capacity(bytes.len());
    let mut from = 0;
    for i in e.head.first..e.head.end {
        let pr = s.tok.pairs[i];
        if pr.code == 10 || pr.code == 11 {
            let x: f64 = s.tok.text(i).parse().expect("a number");
            out.extend_from_slice(&bytes[from..pr.value_at as usize]);
            out.extend_from_slice(num(x + dx).as_bytes());
            from = pr.value_end as usize;
        }
    }
    out.extend_from_slice(&bytes[from..]);
    out
}

/// The H1 drawing `name` and its spliced companion (MOVE-ME moved by 1 along x), as files:
/// `(file name, bytes)` each.
pub fn h1_files(name: &str) -> Option<[(String, Vec<u8>); 2]> {
    let b = h1(name)?.build();
    let spliced = splice_move(&b.bytes, b.handle("move-me"), 1.0);
    Some([
        (format!("{name}.dxf"), b.bytes),
        (format!("{name}-spliced.dxf"), spliced),
    ])
}

/// The pair range (first, end) of the entity with handle `h` (its followers included), in the
/// structure of `bytes`.
fn entity_span(s: &crate::structure::Structure<'_>, h: u64) -> (usize, usize) {
    let e = s
        .entities
        .iter()
        .find(|e| {
            (e.head.first + 1..e.head.end)
                .find(|&i| s.tok.pairs[i].code == 5)
                .and_then(|i| crate::handle::parse(s.tok.value(i)))
                == Some(h)
        })
        .expect("an entity with that handle");
    (
        e.head.first,
        e.followers.last().map_or(e.head.end, |f| f.end),
    )
}

fn read_for_splice(bytes: &[u8]) -> crate::structure::Structure<'_> {
    let limits = crate::Limits::default();
    let t = crate::token::tokenize(bytes, &limits).expect("our drawing reads");
    crate::structure::walk(t, &limits).expect("our drawing reads")
}

/// Our own splice of `bytes`: the first value of code `code` in the entity with handle `h`
/// set to `value`, every other byte as it was.
///
/// # Panics
/// When the entity or the code isn't there.
pub fn splice_value(bytes: &[u8], h: u64, code: i32, value: &str) -> Vec<u8> {
    let s = read_for_splice(bytes);
    let (first, end) = entity_span(&s, h);
    let i = (first + 1..end)
        .find(|&i| s.tok.pairs[i].code == code)
        .expect("the code in the entity");
    let r = s.tok.pairs[i].value_range();
    [&bytes[..r.start], value.as_bytes(), &bytes[r.end..]].concat()
}

/// Our own splice of `bytes`: a pair `code`/`value` added to the entity with handle `h`, right
/// after its first pair of code `after`, written as that pair is (code padding, line ending).
///
/// # Panics
/// When the entity or the code isn't there.
pub fn splice_insert(bytes: &[u8], h: u64, after: i32, code: i32, value: &str) -> Vec<u8> {
    let s = read_for_splice(bytes);
    let (first, end) = entity_span(&s, h);
    let i = (first + 1..end)
        .find(|&i| s.tok.pairs[i].code == after)
        .expect("the code in the entity");
    let p = s.tok.pairs[i];
    let eol = p.value_eol.bytes();
    let padded = bytes.get(p.start()) == Some(&b' ');
    let code_text = if padded {
        format!("{code:>3}")
    } else {
        code.to_string()
    };
    let pair = [code_text.as_bytes(), eol, value.as_bytes(), eol].concat();
    [&bytes[..p.end()], &pair[..], &bytes[p.end()..]].concat()
}

/// Our own splice of `bytes`: the entity with handle `h` cut, its followers with it.
///
/// # Panics
/// When the entity isn't there.
pub fn splice_delete(bytes: &[u8], h: u64) -> Vec<u8> {
    let s = read_for_splice(bytes);
    let (first, end) = entity_span(&s, h);
    let (lo, hi) = (s.tok.pairs[first].start(), s.tok.pairs[end - 1].end());
    [&bytes[..lo], &bytes[hi..]].concat()
}

/// Pairs from `(code, text)` literals.
fn prs(v: &[(i32, &str)]) -> Vec<(i32, String)> {
    v.iter().map(|(c, t)| (*c, (*t).to_owned())).collect()
}

/// A `Raw` entity of kind `kind` with `pairs`.
fn raw(kind: &str, pairs: &[(i32, &str)]) -> Ent {
    Ent::raw(kind, pairs)
}

/// The edit verbs' sampler (2000 and later): every kind the edit verbs change, on layers of each
/// kind (locked, frozen, an external reference's), block references with attributes (one of
/// them twice, one of a dynamic block's drawn form), an entity in a block definition and in a
/// dynamic block's definition and drawn form, a proxy, a draw-order table naming the lamp, and a
/// paper-space text. Each row finds its entity by label.
pub fn edit_sampler(version: Ver) -> Spec {
    let locked = |name: &str, locked: bool, frozen: bool| Layer {
        name: name.into(),
        color: 2,
        frozen,
        locked,
        ltype: "CONTINUOUS".into(),
    };
    let mut s = Spec::new(version)
        .layer("A-WALL", 1)
        .layer("E-DEV", 3)
        .layer("ANNO", 7)
        .layer_full(locked("LOCKED", true, false))
        .layer_full(locked("FROZEN", false, true))
        .layer("BACK|WALLS", 5)
        .block(
            "DEV",
            [0.0, 0.0],
            vec![Ent::circle([0.0, 0.0], 0.5).on("0").label("in-block")],
        )
        .block(
            "DYN",
            [0.0, 0.0],
            vec![Ent::circle([0.0, 0.0], 0.5).on("0").label("in-dyn-def")],
        )
        .dynamic(
            "*U7",
            [0.0, 0.0],
            vec![Ent::line([0.0, 0.0], [1.0, 0.0]).on("0").label("in-dyn")],
            "DYN",
        )
        .model(
            Ent::line([1.1, 0.0], [10.0, 0.0])
                .on("A-WALL")
                .label("wall"),
        )
        .model(
            Ent::line([0.0, 5.0], [10.0, 5.0])
                .on("A-WALL")
                .color(1)
                .prop(420, "16711680")
                .label("red"),
        )
        .model(
            Ent::circle([20.0, 20.0], 2.0)
                .on("E-DEV")
                .handle(0xA00)
                .label("lamp"),
        )
        .model(
            Ent::arc([30.0, 0.0], 5.0, 0.0, 180.0)
                .on("A-WALL")
                .label("arc"),
        )
        .model(
            Ent::lwpoly(
                &[([0.0, 0.0], 0.0), ([10.0, 0.0], 1.0), ([10.0, 10.0], 0.0)],
                false,
            )
            .on("A-WALL")
            .label("lw"),
        )
        .model(
            Ent::poly(
                &[([0.0, 0.0], 0.0), ([4.0, 0.0], 0.0), ([4.0, 3.0], 0.0)],
                true,
            )
            .on("A-WALL")
            .label("poly"),
        )
        .model(
            Ent::text([1.0, 1.0], 2.5, "ROOM 101")
                .on("ANNO")
                .label("room"),
        )
        .model(
            Ent::mtext([1.0, 5.0], 2.5, "North wall")
                .on("ANNO")
                .label("note"),
        )
        .model(
            Ent::raw(
                "ELLIPSE",
                &[
                    (100, "AcDbEllipse"),
                    (10, "5.0"),
                    (20, "5.0"),
                    (30, "0.0"),
                    (11, "2.0"),
                    (21, "1.0"),
                    (31, "0.0"),
                    (40, "0.5"),
                    (41, "0.0"),
                    (42, "6.283185307179586"),
                ],
            )
            .on("A-WALL")
            .label("ellipse"),
        )
        .model(
            Ent::insert("DEV", [30.0, 0.0])
                .on("E-DEV")
                .attrib("DEVICE", "CAM-01")
                .attrib("LABEL", "C1")
                .label("dev"),
        )
        .model(
            Ent::insert("*U7", [40.0, 0.0])
                .on("E-DEV")
                .attrib("DEVICE", "CAM-02")
                .label("dyn"),
        )
        .model(
            Ent::insert("DEV", [50.0, 0.0])
                .on("E-DEV")
                .attrib("DEVICE", "A")
                .attrib("DEVICE", "B")
                .label("twice"),
        )
        .model(
            Ent::line([0.0, -5.0], [1.0, -5.0])
                .on("LOCKED")
                .label("locked"),
        )
        .model(
            Ent::line([0.0, -6.0], [1.0, -6.0])
                .on("FROZEN")
                .label("frozen"),
        )
        .model(
            Ent::line([0.0, -7.0], [1.0, -7.0])
                .on("BACK|WALLS")
                .label("xrefline"),
        )
        .model(
            Ent::raw(
                "ACAD_PROXY_ENTITY",
                &[(100, "AcDbProxyEntity"), (90, "498"), (91, "0"), (92, "0")],
            )
            .on("0")
            .label("proxy"),
        )
        .paper(
            Ent::text([10.0, 10.0], 3.5, "TITLE")
                .on("ANNO")
                .label("title"),
        );
    s.draw.sortents = vec![(0xA00, 0xB00)];
    s
}

/// Our own splice of `bytes`: `pairs` put right after the handle of the attribute tagged `tag`
/// (the first such) of the block reference with handle `h`, written as that pair is.
///
/// # Panics
/// When the reference or the attribute isn't there.
pub fn splice_attrib(bytes: &[u8], h: u64, tag: &str, pairs: &[(i32, &str)]) -> Vec<u8> {
    let s = read_for_splice(bytes);
    let (first, end) = entity_span(&s, h);
    let mut at = None;
    let mut i = first;
    while i < end {
        if s.tok.is0(i, "ATTRIB") {
            let unit_end = (i + 1..end)
                .find(|&j| s.tok.pairs[j].code == 0)
                .unwrap_or(end);
            let tagged = (i + 1..unit_end)
                .any(|j| s.tok.pairs[j].code == 2 && s.tok.text(j).eq_ignore_ascii_case(tag));
            if tagged {
                at = (i + 1..unit_end).find(|&j| s.tok.pairs[j].code == 5);
                break;
            }
            i = unit_end;
            continue;
        }
        i += 1;
    }
    let p = s.tok.pairs[at.expect("the attribute")];
    [
        &bytes[..p.end()],
        &pairs_like(bytes, &p, pairs)[..],
        &bytes[p.end()..],
    ]
    .concat()
}

/// Our own splice of `bytes`: an object of `pairs` (its `0` pair included) put before the end of
/// the OBJECTS section, written as the section's end marker is.
///
/// # Panics
/// When the drawing has no closed OBJECTS section.
pub fn splice_object(bytes: &[u8], pairs: &[(i32, &str)]) -> Vec<u8> {
    let s = read_for_splice(bytes);
    let end = s
        .sections
        .iter()
        .find(|x| x.name == "OBJECTS" && x.closed)
        .map(|x| x.end - 1)
        .expect("an objects section");
    let p = s.tok.pairs[end];
    let at = p.start();
    [
        &bytes[..at],
        &pairs_like(bytes, &p, pairs)[..],
        &bytes[at..],
    ]
    .concat()
}

/// `pairs` written as pair `p` of `bytes` is (code padding, line ending).
fn pairs_like(bytes: &[u8], p: &crate::token::Pair, pairs: &[(i32, &str)]) -> Vec<u8> {
    let eol = p.value_eol.bytes();
    let padded = bytes.get(p.start()) == Some(&b' ');
    let mut out = Vec::new();
    for (code, value) in pairs {
        let code_text = if padded {
            format!("{code:>3}")
        } else {
            code.to_string()
        };
        out.extend_from_slice(&[code_text.as_bytes(), eol, value.as_bytes(), eol].concat());
    }
    out
}

/// The drawing-model read's sampler (2000 on): every kind it models and one it doesn't, the
/// inheritance cases (by layer, by block, layer `0` in a block, own values), a linetype and text
/// styles of its own, layer true colour, lineweight and transparency, a draw-order table, a
/// nested block, a paper layout with a viewport, and the header settings the rules use. Each row
/// finds its entity by label.
pub fn draw_sampler(version: Ver) -> Spec {
    let mut s = Spec::new(version)
        .layer_full(Layer {
            name: "A-WALL".into(),
            color: 1,
            frozen: false,
            locked: false,
            ltype: "DASHED".into(),
        })
        .layer("E-LITE", 3)
        .layer("ANNO", 7)
        .layer("OFF", -2)
        .layer_full(Layer {
            name: "FROZEN".into(),
            color: 4,
            frozen: true,
            locked: false,
            ltype: "CONTINUOUS".into(),
        })
        .header("$LTSCALE", &[(40, "2.0")])
        .header("$LWDEFAULT", &[(370, "25")])
        .header("$PDMODE", &[(70, "3")])
        .header("$PDSIZE", &[(40, "0.5")]);
    s.draw.linetypes = vec![
        ("DASHED".into(), vec![0.5, -0.25]),
        ("DOT".into(), vec![0.0, -0.2]),
    ];
    s.draw.styles = vec![
        ("SANS".into(), "arial.ttf".into(), Some("Arial".into())),
        ("ROMANS".into(), "romans.shx".into(), None),
    ];
    s.draw.layer_pairs = vec![(
        "A-WALL".into(),
        prs(&[
            (370, "35"),
            (420, "3368601"),
            (1001, "AcCmTransparency"),
            (1071, "33554559"),
        ]),
    )];
    s.draw.paper = Some((420.0, 297.0));
    s.draw.sortents = vec![(0xA00, 0xB00)];
    s.draw.images = vec![(0xC00, "images/site-plan.png".into())];
    // a block: by-layer and by-block entities on layer 0, one on a layer of its own, and an
    // attribute definition
    s = s
        .block(
            "DOOR",
            [0.0, 0.0],
            vec![
                Ent::line([0.0, 0.0], [0.9, 0.0]).label("door-leaf0"),
                Ent::arc([0.0, 0.0], 0.9, 0.0, 90.0)
                    .color(0)
                    .ltype("BYBLOCK")
                    .prop(370, "-2")
                    .label("door-swing"),
                Ent::line([0.0, 0.0], [0.0, 0.9])
                    .on("A-WALL")
                    .label("door-jamb"),
                raw(
                    "ATTDEF",
                    &[
                        (100, "AcDbText"),
                        (10, "0.0"),
                        (20, "1.0"),
                        (30, "0.0"),
                        (40, "0.25"),
                        (1, "D-00"),
                        (100, "AcDbAttributeDefinition"),
                        (3, "Door number"),
                        (2, "NO"),
                        (70, "0"),
                    ],
                )
                .label("door-attdef"),
            ],
        )
        .block(
            "NEST",
            [1.0, 1.0],
            vec![Ent::insert("DOOR", [1.0, 1.0]).label("nest-door")],
        )
        .block(
            "*D1",
            [0.0, 0.0],
            vec![
                Ent::line([0.0, -5.0], [100.0, -5.0]),
                // as drawing programs write a dimension's text: middle-centre attachment, above
                // the line by half the text's height plus the gap (0.625)
                Ent::raw(
                    "MTEXT",
                    &[
                        (100, "AcDbMText"),
                        (10, "50.0"),
                        (20, "-3.125"),
                        (30, "0.0"),
                        (40, "2.5"),
                        (41, "0.0"),
                        (71, "5"),
                        (72, "1"),
                        (1, "100"),
                    ],
                ),
            ],
        );
    let m = |s: Spec, e: Ent| s.model(e);
    // style: by layer, own colour, own true colour, by block in model space, transparency,
    // invisible, off and frozen layers
    s = m(
        s,
        Ent::line([0.0, 0.0], [10.0, 0.0])
            .on("A-WALL")
            .handle(0xA00)
            .label("wall"),
    );
    s = m(
        s,
        Ent::line([0.0, 1.0], [10.0, 1.0])
            .on("A-WALL")
            .color(5)
            .prop(370, "50")
            .handle(0xA01)
            .label("own-aci"),
    );
    s = m(
        s,
        Ent::line([0.0, 2.0], [10.0, 2.0])
            .on("A-WALL")
            .color(1)
            .prop(420, "16744448")
            .label("own-rgb"),
    );
    s = m(
        s,
        Ent::line([0.0, 3.0], [10.0, 3.0])
            .on("E-LITE")
            .color(0)
            .ltype("BYBLOCK")
            .prop(370, "-2")
            .label("byblock-model"),
    );
    s = m(
        s,
        Ent::line([0.0, 4.0], [10.0, 4.0])
            .on("E-LITE")
            .prop(440, "33554508")
            .prop(48, "0.5")
            .label("alpha"),
    );
    s = m(
        s,
        Ent::line([0.0, 5.0], [10.0, 5.0])
            .on("E-LITE")
            .prop(60, "1")
            .label("invisible"),
    );
    s = m(s, Ent::line([0.0, 6.0], [10.0, 6.0]).on("OFF").label("off"));
    s = m(
        s,
        Ent::line([0.0, 7.0], [10.0, 7.0])
            .on("FROZEN")
            .label("frozen"),
    );
    s = m(
        s,
        Ent::line([0.0, 8.0], [10.0, 8.0])
            .on("ANNO")
            .ltype("DOT")
            .label("dot"),
    );
    // lines without end
    s = m(
        s,
        raw(
            "XLINE",
            &[
                (100, "AcDbXline"),
                (10, "1.0"),
                (20, "2.0"),
                (30, "0.0"),
                (11, "0.6"),
                (21, "0.8"),
                (31, "0.0"),
            ],
        )
        .on("E-LITE")
        .label("xline"),
    );
    s = m(
        s,
        raw(
            "RAY",
            &[
                (100, "AcDbRay"),
                (10, "1.0"),
                (20, "2.0"),
                (30, "0.0"),
                (11, "0.0"),
                (21, "1.0"),
                (31, "0.0"),
            ],
        )
        .on("E-LITE")
        .label("ray"),
    );
    // points, circles, arcs, ellipses, in the plan's frame, mirrored and tilted
    s = m(s, Ent::point([5.0, 5.0]).on("E-LITE").label("point"));
    s = m(
        s,
        Ent::circle([20.0, 20.0], 2.0).on("E-LITE").label("circle"),
    );
    s = m(
        s,
        Ent::arc([30.0, 0.0], 5.0, 0.0, 90.0)
            .on("A-WALL")
            .label("arc"),
    );
    s = m(
        s,
        Ent::arc([30.0, 0.0], 5.0, 0.0, 90.0)
            .extrusion([0.0, 0.0, -1.0])
            .on("A-WALL")
            .label("arc-mirrored"),
    );
    s = m(
        s,
        Ent::arc([30.0, 0.0], 5.0, 0.0, 90.0)
            .extrusion([0.6, 0.0, 0.8])
            .on("A-WALL")
            .label("arc-tilted"),
    );
    s = m(
        s,
        raw(
            "ELLIPSE",
            &[
                (100, "AcDbEllipse"),
                (10, "50.0"),
                (20, "0.0"),
                (30, "0.0"),
                (11, "10.0"),
                (21, "0.0"),
                (31, "0.0"),
                (40, "0.5"),
                (41, "0.0"),
                (42, "3.141592653589793"),
            ],
        )
        .on("E-LITE")
        .label("ellipse"),
    );
    s = m(
        s,
        raw(
            "ELLIPSE",
            &[
                (100, "AcDbEllipse"),
                (10, "50.0"),
                (20, "0.0"),
                (30, "0.0"),
                (11, "10.0"),
                (21, "0.0"),
                (31, "0.0"),
                (210, "0.0"),
                (220, "0.0"),
                (230, "-1.0"),
                (40, "0.5"),
                (41, "0.0"),
                (42, "1.5707963267948966"),
            ],
        )
        .on("E-LITE")
        .label("ellipse-mirrored"),
    );
    // polylines: bulges and widths, a 2D polyline, a spline-fit polyline with a frame vertex
    s = m(
        s,
        raw(
            "LWPOLYLINE",
            &[
                (100, "AcDbPolyline"),
                (90, "3"),
                (70, "1"),
                (10, "0.0"),
                (20, "0.0"),
                (40, "0.1"),
                (41, "0.2"),
                (10, "10.0"),
                (20, "0.0"),
                (42, "1.0"),
                (10, "10.0"),
                (20, "10.0"),
            ],
        )
        .on("A-WALL")
        .label("lw"),
    );
    s = m(
        s,
        Ent::poly(
            &[([0.0, 0.0], 0.0), ([4.0, 0.0], 0.5), ([4.0, 3.0], 0.0)],
            true,
        )
        .on("A-WALL")
        .label("poly"),
    );
    s = m(
        s,
        raw(
            "POLYLINE",
            &[
                (100, "AcDb2dPolyline"),
                (66, "1"),
                (10, "0.0"),
                (20, "0.0"),
                (30, "0.0"),
                (70, "4"),
                (0, "VERTEX"),
                (5, "F00"),
                (100, "AcDbEntity"),
                (8, "A-WALL"),
                (100, "AcDbVertex"),
                (100, "AcDb2dVertex"),
                (10, "0.0"),
                (20, "0.0"),
                (30, "0.0"),
                (70, "16"),
                (0, "VERTEX"),
                (5, "F01"),
                (100, "AcDbEntity"),
                (8, "A-WALL"),
                (100, "AcDbVertex"),
                (100, "AcDb2dVertex"),
                (10, "1.0"),
                (20, "0.0"),
                (30, "0.0"),
                (70, "8"),
                (0, "VERTEX"),
                (5, "F02"),
                (100, "AcDbEntity"),
                (8, "A-WALL"),
                (100, "AcDbVertex"),
                (100, "AcDb2dVertex"),
                (10, "5.0"),
                (20, "7.0"),
                (30, "0.0"),
                (70, "16"),
                (0, "VERTEX"),
                (5, "F03"),
                (100, "AcDbEntity"),
                (8, "A-WALL"),
                (100, "AcDbVertex"),
                (100, "AcDb2dVertex"),
                (10, "2.0"),
                (20, "1.0"),
                (30, "0.0"),
                (70, "8"),
                (0, "SEQEND"),
                (5, "F04"),
                (100, "AcDbEntity"),
                (8, "A-WALL"),
            ],
        )
        .on("A-WALL")
        .label("fit-poly"),
    );
    // splines: clamped cubic, rational, fit points only, a zero weight
    s = m(
        s,
        raw(
            "SPLINE",
            &[
                (100, "AcDbSpline"),
                (70, "8"),
                (71, "3"),
                (72, "9"),
                (73, "5"),
                (74, "0"),
                (40, "0.0"),
                (40, "0.0"),
                (40, "0.0"),
                (40, "0.0"),
                (40, "1.0"),
                (40, "2.0"),
                (40, "2.0"),
                (40, "2.0"),
                (40, "2.0"),
                (10, "0.0"),
                (20, "0.0"),
                (30, "0.0"),
                (10, "10.0"),
                (20, "20.0"),
                (30, "0.0"),
                (10, "30.0"),
                (20, "20.0"),
                (30, "0.0"),
                (10, "40.0"),
                (20, "0.0"),
                (30, "0.0"),
                (10, "50.0"),
                (20, "10.0"),
                (30, "0.0"),
            ],
        )
        .on("E-LITE")
        .label("spline"),
    );
    s = m(
        s,
        raw(
            "SPLINE",
            &[
                (100, "AcDbSpline"),
                (70, "12"),
                (71, "2"),
                (72, "6"),
                (73, "3"),
                (74, "0"),
                (40, "0.0"),
                (40, "0.0"),
                (40, "0.0"),
                (40, "1.0"),
                (40, "1.0"),
                (40, "1.0"),
                (41, "1.0"),
                (41, "0.7071067811865476"),
                (41, "1.0"),
                (10, "10.0"),
                (20, "0.0"),
                (30, "0.0"),
                (10, "10.0"),
                (20, "10.0"),
                (30, "0.0"),
                (10, "0.0"),
                (20, "10.0"),
                (30, "0.0"),
            ],
        )
        .on("E-LITE")
        .label("spline-rational"),
    );
    s = m(
        s,
        raw(
            "SPLINE",
            &[
                (100, "AcDbSpline"),
                (70, "8"),
                (71, "3"),
                (72, "0"),
                (73, "0"),
                (74, "3"),
                (11, "0.0"),
                (21, "0.0"),
                (31, "0.0"),
                (11, "5.0"),
                (21, "5.0"),
                (31, "0.0"),
                (11, "10.0"),
                (21, "0.0"),
                (31, "0.0"),
            ],
        )
        .on("E-LITE")
        .label("spline-fit"),
    );
    s = m(
        s,
        raw(
            "SPLINE",
            &[
                (100, "AcDbSpline"),
                (70, "12"),
                (71, "1"),
                (72, "4"),
                (73, "2"),
                (74, "0"),
                (40, "0.0"),
                (40, "0.0"),
                (40, "1.0"),
                (40, "1.0"),
                (41, "1.0"),
                (41, "0.0"),
                (10, "0.0"),
                (20, "0.0"),
                (30, "0.0"),
                (10, "1.0"),
                (20, "0.0"),
                (30, "0.0"),
            ],
        )
        .on("E-LITE")
        .label("spline-zero-weight"),
    );
    // text: left, centred on its second point, fit between two points, % codes, a style
    s = m(
        s,
        Ent::text([1.0, 1.0], 2.5, "ROOM 101")
            .on("ANNO")
            .label("text-left"),
    );
    s = m(
        s,
        raw(
            "TEXT",
            &[
                (100, "AcDbText"),
                (10, "0.0"),
                (20, "0.0"),
                (30, "0.0"),
                (40, "2.0"),
                (1, "CENTRED"),
                (7, "SANS"),
                (72, "1"),
                (11, "20.0"),
                (21, "10.0"),
                (31, "0.0"),
                (100, "AcDbText"),
                (73, "2"),
            ],
        )
        .on("ANNO")
        .label("text-centre"),
    );
    s = m(
        s,
        raw(
            "TEXT",
            &[
                (100, "AcDbText"),
                (10, "0.0"),
                (20, "20.0"),
                (30, "0.0"),
                (40, "2.0"),
                (1, "FIT"),
                (72, "5"),
                (11, "10.0"),
                (21, "30.0"),
                (31, "0.0"),
                (100, "AcDbText"),
            ],
        )
        .on("ANNO")
        .label("text-fit"),
    );
    s = m(
        s,
        raw(
            "TEXT",
            &[
                (100, "AcDbText"),
                (10, "0.0"),
                (20, "40.0"),
                (30, "0.0"),
                (40, "2.0"),
                (1, "%%c50 %%uUNDER%%u 90%%d"),
                (100, "AcDbText"),
            ],
        )
        .on("ANNO")
        .label("text-codes"),
    );
    s = m(
        s,
        raw(
            "MTEXT",
            &[
                (100, "AcDbMText"),
                (10, "0.0"),
                (20, "60.0"),
                (30, "0.0"),
                (40, "2.5"),
                (41, "40.0"),
                (71, "5"),
                (72, "1"),
                (7, "SANS"),
                (
                    1,
                    "{\\fArial|b1|i0;Bold}\\PLine \\H2x;big\\H1x; \\S1/2;\\~\\C1;red",
                ),
            ],
        )
        .on("ANNO")
        .label("mtext"),
    );
    // block references: with an attribute, scaled and rotated, an array
    s = m(
        s,
        Ent::insert("DOOR", [10.0, 0.0])
            .on("A-WALL")
            .attrib("NO", "D-01")
            .label("door1"),
    );
    s = m(
        s,
        raw(
            "INSERT",
            &[
                (100, "AcDbBlockReference"),
                (2, "NEST"),
                (10, "100.0"),
                (20, "0.0"),
                (30, "0.0"),
                (41, "2.0"),
                (42, "2.0"),
                (43, "1.0"),
                (50, "90.0"),
            ],
        )
        .on("A-WALL")
        .label("nest1"),
    );
    s = m(
        s,
        raw(
            "INSERT",
            &[
                (100, "AcDbMInsertBlock"),
                (2, "DOOR"),
                (10, "200.0"),
                (20, "0.0"),
                (30, "0.0"),
                (70, "2"),
                (71, "3"),
                (44, "5.0"),
                (45, "6.0"),
            ],
        )
        .on("A-WALL")
        .label("array"),
    );
    // filled shapes
    s = m(
        s,
        raw(
            "SOLID",
            &[
                (100, "AcDbTrace"),
                (10, "0.0"),
                (20, "-10.0"),
                (30, "0.0"),
                (11, "4.0"),
                (21, "-10.0"),
                (31, "0.0"),
                (12, "0.0"),
                (22, "-6.0"),
                (32, "0.0"),
                (13, "4.0"),
                (23, "-6.0"),
                (33, "0.0"),
            ],
        )
        .on("E-LITE")
        .label("solid"),
    );
    s = m(
        s,
        raw(
            "3DFACE",
            &[
                (100, "AcDbFace"),
                (10, "10.0"),
                (20, "-10.0"),
                (30, "0.0"),
                (11, "14.0"),
                (21, "-10.0"),
                (31, "0.0"),
                (12, "14.0"),
                (22, "-6.0"),
                (32, "0.0"),
                (13, "10.0"),
                (23, "-6.0"),
                (33, "0.0"),
                (70, "2"),
            ],
        )
        .on("E-LITE")
        .label("face"),
    );
    // hatches: solid with a polyline loop; a pattern of horizontal lines in a square with a
    // square island; a pattern too fine to draw
    s = m(
        s,
        raw(
            "HATCH",
            &[
                (100, "AcDbHatch"),
                (10, "0.0"),
                (20, "0.0"),
                (30, "0.0"),
                (210, "0.0"),
                (220, "0.0"),
                (230, "1.0"),
                (2, "SOLID"),
                (70, "1"),
                (71, "0"),
                (91, "1"),
                (92, "3"),
                (72, "0"),
                (73, "1"),
                (93, "4"),
                (10, "0.0"),
                (20, "-20.0"),
                (10, "10.0"),
                (20, "-20.0"),
                (10, "10.0"),
                (20, "-30.0"),
                (10, "0.0"),
                (20, "-30.0"),
                (97, "0"),
                (75, "0"),
                (76, "1"),
                (98, "0"),
            ],
        )
        .on("E-LITE")
        .label("hatch-solid"),
    );
    s = m(
        s,
        raw(
            "HATCH",
            &[
                (100, "AcDbHatch"),
                (10, "0.0"),
                (20, "0.0"),
                (30, "0.0"),
                (210, "0.0"),
                (220, "0.0"),
                (230, "1.0"),
                (2, "LINES"),
                (70, "0"),
                (71, "0"),
                (91, "2"),
                (92, "1"),
                (93, "4"),
                (72, "1"),
                (10, "20.0"),
                (20, "-30.0"),
                (11, "30.0"),
                (21, "-30.0"),
                (72, "1"),
                (10, "30.0"),
                (20, "-30.0"),
                (11, "30.0"),
                (21, "-20.0"),
                (72, "1"),
                (10, "30.0"),
                (20, "-20.0"),
                (11, "20.0"),
                (21, "-20.0"),
                (72, "1"),
                (10, "20.0"),
                (20, "-20.0"),
                (11, "20.0"),
                (21, "-30.0"),
                (97, "0"),
                (92, "16"),
                (93, "4"),
                (72, "1"),
                (10, "23.0"),
                (20, "-27.0"),
                (11, "27.0"),
                (21, "-27.0"),
                (72, "1"),
                (10, "27.0"),
                (20, "-27.0"),
                (11, "27.0"),
                (21, "-23.0"),
                (72, "1"),
                (10, "27.0"),
                (20, "-23.0"),
                (11, "23.0"),
                (21, "-23.0"),
                (72, "1"),
                (10, "23.0"),
                (20, "-23.0"),
                (11, "23.0"),
                (21, "-27.0"),
                (97, "0"),
                (75, "0"),
                (76, "1"),
                (52, "0.0"),
                (41, "1.0"),
                (77, "0"),
                (78, "1"),
                (53, "0.0"),
                (43, "0.0"),
                (44, "0.5"),
                (45, "0.0"),
                (46, "1.0"),
                (79, "2"),
                (49, "0.75"),
                (49, "-0.25"),
                (98, "0"),
            ],
        )
        .on("E-LITE")
        .label("hatch-pattern"),
    );
    s = m(
        s,
        raw(
            "HATCH",
            &[
                (100, "AcDbHatch"),
                (10, "0.0"),
                (20, "0.0"),
                (30, "0.0"),
                (210, "0.0"),
                (220, "0.0"),
                (230, "1.0"),
                (2, "FINE"),
                (70, "0"),
                (71, "0"),
                (91, "1"),
                (92, "3"),
                (72, "0"),
                (73, "1"),
                (93, "4"),
                (10, "0.0"),
                (20, "-100.0"),
                (10, "100.0"),
                (20, "-100.0"),
                (10, "100.0"),
                (20, "-200.0"),
                (10, "0.0"),
                (20, "-200.0"),
                (97, "0"),
                (75, "0"),
                (76, "1"),
                (52, "0.0"),
                (41, "1.0"),
                (77, "0"),
                (78, "1"),
                (53, "0.0"),
                (43, "0.0"),
                (44, "0.0"),
                (45, "0.0"),
                (46, "0.001"),
                (79, "0"),
                (98, "0"),
            ],
        )
        .on("E-LITE")
        .label("hatch-dense"),
    );
    // a dimension (its block holds what it draws), a leader, a wipeout, a solid it doesn't
    // model, and a kind no reader knows
    s = m(
        s,
        raw(
            "DIMENSION",
            &[
                (100, "AcDbDimension"),
                (2, "*D1"),
                (10, "100.0"),
                (20, "-5.0"),
                (30, "0.0"),
                (11, "50.0"),
                (21, "-3.125"),
                (31, "0.0"),
                (70, "32"),
                (1, ""),
                (42, "100.0"),
                (3, "Standard"),
                (100, "AcDbAlignedDimension"),
                (13, "0.0"),
                (23, "0.0"),
                (33, "0.0"),
                (14, "100.0"),
                (24, "0.0"),
                (34, "0.0"),
                (100, "AcDbRotatedDimension"),
            ],
        )
        .on("ANNO")
        .label("dim"),
    );
    s = m(
        s,
        raw(
            "LEADER",
            &[
                (100, "AcDbLeader"),
                (3, "Standard"),
                (71, "1"),
                (72, "0"),
                (73, "3"),
                (76, "3"),
                (10, "0.0"),
                (20, "80.0"),
                (30, "0.0"),
                (10, "5.0"),
                (20, "85.0"),
                (30, "0.0"),
                (10, "10.0"),
                (20, "85.0"),
                (30, "0.0"),
            ],
        )
        .on("ANNO")
        .label("leader"),
    );
    s = m(
        s,
        raw(
            "WIPEOUT",
            &[
                (100, "AcDbWipeout"),
                (90, "0"),
                (10, "60.0"),
                (20, "60.0"),
                (30, "0.0"),
                (11, "10.0"),
                (21, "0.0"),
                (31, "0.0"),
                (12, "0.0"),
                (22, "10.0"),
                (32, "0.0"),
                (13, "1.0"),
                (23, "1.0"),
                (70, "7"),
                (280, "1"),
                (281, "50"),
                (282, "50"),
                (283, "0"),
                (71, "2"),
                (91, "5"),
                (14, "-0.5"),
                (24, "-0.5"),
                (14, "0.5"),
                (24, "-0.5"),
                (14, "0.5"),
                (24, "0.5"),
                (14, "-0.5"),
                (24, "0.5"),
                (14, "-0.5"),
                (24, "-0.5"),
            ],
        )
        .on("ANNO")
        .label("wipeout"),
    );
    s = m(
        s,
        raw(
            "LEADER",
            &[
                (100, "AcDbLeader"),
                (3, "Standard"),
                (71, "1"),
                (72, "0"),
                (73, "3"),
                (76, "2"),
                (10, "20.0"),
                (20, "80.0"),
                (30, "0.0"),
                (10, "25.0"),
                (20, "85.0"),
                (30, "0.0"),
            ],
        )
        .on("ANNO")
        .xdata(
            "ACAD",
            &[
                (1000, "DSTYLE"),
                (1002, "{"),
                (1070, "41"),
                (1040, "2.5"),
                (1070, "40"),
                (1040, "2.0"),
                (1002, "}"),
            ],
        )
        .label("leader-styled"),
    );
    // an image: 100 by 50 pixels of 0.1, clipped to a rectangle of pixels 10..90 by 5..45
    s = m(
        s,
        raw(
            "IMAGE",
            &[
                (100, "AcDbRasterImage"),
                (90, "0"),
                (10, "100.0"),
                (20, "100.0"),
                (30, "0.0"),
                (11, "0.1"),
                (21, "0.0"),
                (31, "0.0"),
                (12, "0.0"),
                (22, "0.1"),
                (32, "0.0"),
                (13, "100.0"),
                (23, "50.0"),
                (340, "C00"),
                (70, "7"),
                (280, "1"),
                (281, "50"),
                (282, "50"),
                (283, "0"),
                (71, "1"),
                (91, "2"),
                (14, "9.5"),
                (24, "4.5"),
                (14, "89.5"),
                (24, "44.5"),
            ],
        )
        .on("ANNO")
        .label("image"),
    );
    // multiline texts at each of the nine attachments, 40 wide, 2 high
    for k in 1..=9 {
        let x = num(300.0 + 60.0 * k as f64);
        let a = k.to_string();
        let label = format!("mtext-attach-{k}");
        s = m(
            s,
            raw(
                "MTEXT",
                &[
                    (100, "AcDbMText"),
                    (10, x.as_str()),
                    (20, "100.0"),
                    (30, "0.0"),
                    (40, "2.0"),
                    (41, "40.0"),
                    (71, a.as_str()),
                    (72, "1"),
                    (1, "Room"),
                ],
            )
            .on("ANNO")
            .label(&label),
        );
    }
    // a multileader: modelled later; drawn: false until then
    s = m(
        s,
        raw(
            "MULTILEADER",
            &[
                (100, "AcDbMLeader"),
                (270, "2"),
                (300, "CONTEXT_DATA{"),
                (40, "1.0"),
                (10, "0.0"),
                (20, "0.0"),
                (30, "0.0"),
                (41, "2.5"),
                (290, "0"),
                (296, "0"),
                (301, "}"),
                (340, "0"),
                (90, "0"),
                (170, "1"),
            ],
        )
        .on("ANNO")
        .label("mleader"),
    );
    s = m(
        s,
        raw("3DSOLID", &[(100, "AcDbModelerGeometry"), (70, "1")])
            .on("E-LITE")
            .label("solid3d"),
    );
    s = m(
        s,
        raw("ACME_GADGET", &[(100, "AcDbEntity")])
            .on("E-LITE")
            .label("unknown"),
    );
    // paper space: a viewport onto model space, and a title
    s.paper(
        raw(
            "VIEWPORT",
            &[
                (100, "AcDbViewport"),
                (10, "210.0"),
                (20, "148.5"),
                (30, "0.0"),
                (40, "300.0"),
                (41, "200.0"),
                (68, "2"),
                (69, "2"),
                (12, "50.0"),
                (22, "0.0"),
                (13, "0.0"),
                (23, "0.0"),
                (14, "10.0"),
                (24, "10.0"),
                (15, "10.0"),
                (25, "10.0"),
                (16, "0.0"),
                (26, "0.0"),
                (36, "1.0"),
                (17, "0.0"),
                (27, "0.0"),
                (37, "0.0"),
                (42, "50.0"),
                (43, "0.0"),
                (44, "0.0"),
                (45, "100.0"),
                (50, "0.0"),
                (51, "0.0"),
                (72, "1000"),
                (90, "32864"),
                (1, ""),
                (281, "0"),
                (71, "1"),
                (74, "0"),
                (110, "0.0"),
                (120, "0.0"),
                (130, "0.0"),
                (111, "1.0"),
                (121, "0.0"),
                (131, "0.0"),
                (112, "0.0"),
                (122, "1.0"),
                (132, "0.0"),
                (79, "0"),
                (146, "0.0"),
            ],
        )
        .on("0")
        .label("viewport"),
    )
    .paper(
        Ent::text([10.0, 10.0], 3.5, "TITLE")
            .on("ANNO")
            .label("title"),
    )
}

/// Two multiline texts with the same main group, 40 apart: one as written before 2018, one with
/// the 2018 embedded object after its `101` marker (the object's own direction `10`, insertion
/// point `11`, extents, and two static columns 30 wide with a gutter of 2.5).
/// A constant 2018 attribute definition twice at one place: plain, and multi-line (its type
/// as a `71` in its own subclass; its embedded multiline text after `101`, with a `41` and a
/// `71` of its own meaning).
pub fn attdef_embedded_sampler() -> Spec {
    let main = |x: &'static str| -> Vec<(i32, &'static str)> {
        vec![
            (100, "AcDbText"),
            (10, x),
            (20, "10.0"),
            (30, "0.0"),
            (40, "2.5"),
            (1, "Value"),
            (100, "AcDbAttributeDefinition"),
            (280, "0"),
            (3, "Prompt"),
            (2, "TAG"),
            (70, "2"),
            (280, "0"),
        ]
    };
    let mut embedded = main("0.0");
    embedded.extend([
        // its type (4: a multi-line definition) and a flag, in its own subclass
        (71, "4"),
        (72, "0"),
        (101, "Embedded Object"),
        (10, "0.0"),
        (20, "10.0"),
        (30, "0.0"),
        (40, "30.0"),
        (41, "12.0"),
        (46, "0.0"),
        (71, "5"),
        (72, "5"),
        (1, "Value\\Pline two"),
    ]);
    Spec::new(Ver::R2018)
        .layer("ANNO", 7)
        .model(Ent::raw("ATTDEF", &main("0.0")).on("ANNO").label("plain"))
        .model(Ent::raw("ATTDEF", &embedded).on("ANNO").label("multiline"))
}

pub fn mtext_embedded_sampler() -> Spec {
    let main = |x: &'static str| -> Vec<(i32, &'static str)> {
        vec![
            (100, "AcDbMText"),
            (10, x),
            (20, "100.0"),
            (30, "0.0"),
            (40, "2.5"),
            (41, "30.0"),
            (46, "0.0"),
            (71, "1"),
            (72, "1"),
            (1, "Column one\\Pcolumn two"),
            (7, "Standard"),
            (73, "1"),
            (44, "1.0"),
        ]
    };
    let mut embedded = main("40.0");
    embedded.extend([
        (101, "Embedded Object"),
        (70, "1"),
        (10, "1.0"),
        (20, "0.0"),
        (30, "0.0"),
        (11, "40.0"),
        (21, "100.0"),
        (31, "0.0"),
        (40, "30.0"),
        (41, "10.0"),
        (42, "62.5"),
        (43, "5.0"),
        (71, "1"),
        (72, "2"),
        (44, "30.0"),
        (45, "2.5"),
        (73, "0"),
        (74, "0"),
        (46, "5.0"),
        (46, "5.0"),
    ]);
    Spec::new(Ver::R2018)
        .layer("ANNO", 7)
        .model(Ent::raw("MTEXT", &main("0.0")).on("ANNO").label("plain"))
        .model(Ent::raw("MTEXT", &embedded).on("ANNO").label("embedded"))
}

/// `n` lightweight polylines of `v` vertices each, in model space: drawings whose pages fill by
/// bytes before entities.
pub fn heavy(version: Ver, n: usize, v: usize) -> Spec {
    let mut s = Spec::new(version).layer("A-WALL", 1);
    for i in 0..n {
        let pts: Vec<([f64; 2], f64)> = (0..v)
            .map(|k| ([k as f64 * 0.5, i as f64 + (k % 7) as f64 * 0.1], 0.0))
            .collect();
        s.model.push(Ent::lwpoly(&pts, false).on("A-WALL"));
    }
    s
}
