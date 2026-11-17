//! The drawing as a reader hands it over: format-neutral. Every value a format writes as text
//! is kept as its own text (`color` is `"256"`, not a parsed number), so the index compares
//! what the file says; boxes and points are numbers, for finding and counting.

use std::sync::Arc;

use k2_mpc_base::index::Digest;

/// How good an entity's box is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BboxQ {
    /// The geometry's exact extent.
    Exact,
    /// Close, and containing it (text, splines, frames other than the plan's).
    Approx,
    /// The insertion point only (a block reference).
    InsertOnly,
    /// No box.
    None,
}

impl BboxQ {
    pub fn word(self) -> &'static str {
        match self {
            BboxQ::Exact => "exact",
            BboxQ::Approx => "approx",
            BboxQ::InsertOnly => "insert_only",
            BboxQ::None => "none",
        }
    }
}

/// The drawing's units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Units {
    /// `mm`, `in`, `m`…, `unitless`, or `unknown` for a code no table names.
    pub word: &'static str,
    /// The drawing's own unit code, when it states one.
    pub insunits: Option<i64>,
    /// Its measurement system code (0 imperial, 1 metric), when it states one.
    pub measurement: Option<i64>,
    /// Where `word` came from: `insunits`, or `none`.
    pub source: &'static str,
}

/// The unit words, by unit code.
const UNIT_WORDS: [&str; 25] = [
    "unitless", "in", "ft", "mi", "mm", "cm", "m", "km", "uin", "mil", "yd", "angstrom", "nm",
    "um", "dm", "dam", "hm", "gm", "au", "ly", "pc", "us-ft", "us-in", "us-yd", "us-mi",
];

impl Units {
    /// From a unit code and a measurement code, either absent.
    pub fn from_codes(insunits: Option<i64>, measurement: Option<i64>) -> Units {
        match insunits {
            Some(u) => Units {
                word: usize::try_from(u)
                    .ok()
                    .and_then(|i| UNIT_WORDS.get(i))
                    .copied()
                    .unwrap_or("unknown"),
                insunits: Some(u),
                measurement,
                source: "insunits",
            },
            None => Units {
                word: "unitless",
                insunits: None,
                measurement,
                source: "none",
            },
        }
    }
}

/// What a block record is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// Model space.
    Model,
    /// A paper space layout.
    Layout,
    /// A block definition.
    Block,
}

impl BlockKind {
    pub fn word(self) -> &'static str {
        match self {
            BlockKind::Model => "model",
            BlockKind::Layout => "layout",
            BlockKind::Block => "block",
        }
    }
}

/// A layer.
#[derive(Debug, Clone)]
pub struct Layer {
    pub handle: Option<u64>,
    /// Another object in the drawing has the same handle.
    pub duplicate: bool,
    pub name: String,
    /// Its colour number as written; negative: the layer is off.
    pub color: Option<i64>,
    pub frozen: bool,
    pub locked: bool,
    pub plot: bool,
    pub ltype: Option<String>,
    pub lw: Option<String>,
    /// Dependent on an external reference.
    pub xref: bool,
    /// Every fact of its record but its handle.
    pub record: Digest,
    /// Its facts not given as fields above.
    pub content: Digest,
}

impl Layer {
    /// Whether it is on (its colour isn't negative).
    pub fn on(&self) -> bool {
        self.color.is_none_or(|c| c >= 0)
    }
}

/// Model space, a layout, or a block definition.
#[derive(Debug, Clone)]
pub struct BlockRec {
    pub handle: Option<u64>,
    pub duplicate: bool,
    /// The index key a reader gives a space the file names no record for
    /// (`@space:model`); `None` otherwise.
    pub key: Option<String>,
    pub name: String,
    pub kind: BlockKind,
    pub base: Option<[f64; 3]>,
    /// An anonymous block (its name starts with `*`).
    pub anon: bool,
    /// An external reference.
    pub xref: bool,
    /// An external reference's path, as written.
    pub path: Option<String>,
    /// A layout's own name (model space's and each paper layout's), as its layout object names
    /// it.
    pub layout: Option<String>,
    /// An anonymous block that draws one reference of a dynamic block as it is set: the index,
    /// in [`Doc::blocks`], of that dynamic block's record, whose name is the block's real one.
    pub dynamic_of: Option<usize>,
    pub record: Digest,
    pub content: Digest,
}

/// A text many entities share (a kind, a layer name, a property value): one copy per drawing,
/// shared by every entity that has it ([`Interner`]).
pub type Shared = Arc<str>;

/// Gives one [`Shared`] copy of each distinct text, so a drawing's model holds each kind,
/// layer name and property value once, however many entities name it.
#[derive(Debug, Default)]
pub struct Interner(std::collections::HashMap<Box<str>, Shared>);

impl Interner {
    /// The shared copy of `s`.
    pub fn get(&mut self, s: &str) -> Shared {
        if let Some(x) = self.0.get(s) {
            return x.clone();
        }
        let x: Shared = Arc::from(s);
        self.0.insert(s.into(), x.clone());
        x
    }
}

/// An entity's own display properties, each as written: kept apart, since most entities state
/// none of them (they follow their layer).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Props {
    pub color: Option<Shared>,
    pub rgb: Option<Shared>,
    pub ltype: Option<Shared>,
    pub lw: Option<Shared>,
    pub alpha: Option<Shared>,
}

/// One attribute of a block reference: its tag, its value, its own handle and where it sits.
#[derive(Debug, Clone, PartialEq)]
pub struct Attrib {
    pub tag: Shared,
    pub value: Box<str>,
    /// The attribute's own handle (not its block reference's).
    pub handle: Option<u64>,
    pub at: Option<[f64; 2]>,
}

/// What fewer entities have, kept apart: a block reference's attributes and its scale and
/// rotation, a polyline's vertices, a leader's points (its arrow first) and annotation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extra {
    /// A block reference's attributes, in the file's order.
    pub attribs: Vec<Attrib>,
    /// A polyline's vertices in world x and y (a bulged edge read as its chord); a leader's
    /// points, its arrow's first.
    pub verts: Vec<[f64; 2]>,
    /// The polyline is closed.
    pub closed: bool,
    /// A block reference's scale along its block's x and y and its rotation in degrees, when
    /// any differs from 1, 1 and 0.
    pub xform: Option<[f64; 3]>,
    /// A leader's annotation: the handle of the entity that holds its text.
    pub annotation: Option<u64>,
    /// A multileader's leader lines, each from its arrow to its leader's landing (at most
    /// [`crate::leader::LINES_MAX`] lines and [`crate::leader::POINTS_MAX`] points).
    pub lines: Vec<Vec<[f64; 2]>>,
}

/// One entity, its followers (attributes, vertices) inside it.
#[derive(Debug, Clone)]
pub struct Entity {
    pub handle: Option<u64>,
    pub duplicate: bool,
    /// Its kind, upper case (`LINE`).
    pub kind: Shared,
    /// Its owner: an index into [`Doc::blocks`].
    pub owner: Option<usize>,
    /// The layer's name as written.
    pub layer: Shared,
    /// Its colour, true colour, linetype, lineweight and transparency, when it states any.
    pub props: Option<Box<Props>>,
    /// The first 8 bytes of the digest of its geometry and kind-specific data, its followers'
    /// included: enough to tell a change, which is all the index asks of it.
    pub geom: [u8; 8],
    /// Its facts not given as fields: extension data, reactors, comments, unknown codes.
    pub content: Digest,
    /// Every fact but its handle.
    pub record: Digest,
    /// `[x0, y0, x1, y1]` in the drawing's units, world coordinates.
    pub bbox: Option<[f64; 4]>,
    pub bbox_q: BboxQ,
    /// In a frame of its own (an extrusion other than up).
    pub ocs: bool,
    /// The applications whose extension data it carries.
    pub xd: Vec<Shared>,
    /// `1` when the edit verbs can change it, else why not (`ocs`, `assoc`, `hatch`…).
    pub edit: &'static str,
    /// Its followers (attributes, vertices), the end marker not counted.
    pub followers: u32,
    /// Its text: a text's content, or a block reference's attributes as `TAG=VALUE` lines.
    pub text: Option<Box<str>>,
    /// A block reference's block name.
    pub block: Option<Shared>,
    /// Its insertion point (text, block references).
    pub at: Option<[f64; 2]>,
    /// A line runs from its box's low corner to its high one (else from the low-x, high-y
    /// corner down): with the box, its two ends exactly, at no cost.
    pub line_rises: bool,
    /// Its attributes and vertices, when it has any.
    pub extra: Option<Box<Extra>>,
}

/// A drawing, as one version of a file reads.
#[derive(Debug, Clone)]
pub struct Doc {
    /// The file format (`dxf`, `dwg`).
    pub format: &'static str,
    /// The format's version stamp (`AC1027`).
    pub version: String,
    /// The release that stamp names (`R2013`).
    pub release: Option<&'static str>,
    /// How its text is encoded (`utf-8`, a code page).
    pub encoding: String,
    /// How its lines end, for a text format (`lf`, `crlf`, `mixed`).
    pub eol: Option<&'static str>,
    pub units: Units,
    /// `on`, `off` or `partial`.
    pub handles: &'static str,
    /// What the file says about the program that wrote it last, when it says anything.
    pub writer_mark: Option<String>,
    /// The drawing-wide settings, minus those every save changes.
    pub header: Digest,
    /// The extents the file states.
    pub header_extents: Option<([f64; 3], [f64; 3])>,
    /// The highest handle any object owns or any pointer names.
    pub hmax: Option<u64>,
    /// The next handle the file says to give.
    pub handseed: Option<u64>,
    pub layers: Vec<Layer>,
    pub blocks: Vec<BlockRec>,
    pub entities: Vec<Entity>,
    /// The external references' paths, as written.
    pub xrefs: Vec<String>,
}

impl Doc {
    /// The layer named `name` (case-insensitive), the first if two are.
    pub fn layer_named(&self, name: &str) -> Option<usize> {
        self.layers
            .iter()
            .position(|l| l.name.eq_ignore_ascii_case(name))
    }

    /// The block record named `name` (case-insensitive), the first if two are.
    pub fn block_named(&self, name: &str) -> Option<usize> {
        self.blocks
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case(name))
    }

    /// The block record a block reference names, by its name as written.
    pub fn insert_block(&self, e: &Entity) -> Option<usize> {
        e.block.as_deref().and_then(|b| self.block_named(b))
    }

    /// The name a block reference's block goes by: for a reference to the anonymous block that
    /// draws a dynamic block as set, the dynamic block's own name; else the name it names.
    pub fn block_name<'a>(&'a self, e: &'a Entity) -> Option<&'a str> {
        let b = e.block.as_deref()?;
        // only an anonymous name (`*U…`) can stand for a dynamic block
        if !b.starts_with('*') || crate::mutant::active("DynamicAsAnonymous") {
            return Some(b);
        }
        match self.block_named(b).and_then(|i| self.blocks[i].dynamic_of) {
            Some(r) => Some(self.blocks[r].name.as_str()),
            None => Some(b),
        }
    }

    /// Entity `i`'s block's point `p` (in the block's own x and y) where the reference places
    /// it: from the block's base point, scaled, rotated, at the insertion point. `None` for an
    /// entity that isn't a block reference, or one in a frame of its own.
    pub fn place(&self, i: usize, p: [f64; 2]) -> Option<[f64; 2]> {
        let e = &self.entities[i];
        let at = e.at.filter(|_| e.block.is_some() && !e.ocs)?;
        let base = self
            .insert_block(e)
            .and_then(|b| self.blocks[b].base)
            .unwrap_or([0.0; 3]);
        let [sx, sy, rot] = e
            .extra
            .as_ref()
            .and_then(|x| x.xform)
            .unwrap_or([1.0, 1.0, 0.0]);
        let rot = if crate::mutant::active("PlacedUnrotated") {
            0.0
        } else {
            rot
        };
        let (x, y) = ((p[0] - base[0]) * sx, (p[1] - base[1]) * sy);
        let (s, c) = rot.to_radians().sin_cos();
        Some([at[0] + x * c - y * s, at[1] + x * s + y * c])
    }

    /// A block reference's long axis as placed: its block's drawn extent (its texts and
    /// attribute definitions left out) along the block's longer axis (as scaled), at the middle
    /// of the other, from the low end to the high one,
    /// placed ([`Doc::place`]). For a straight run drawn as a block (a conduit scaled and
    /// rotated along it), its two ends. `None` when the block draws nothing with a box.
    pub fn placed_axis(&self, i: usize) -> Option<[[f64; 2]; 2]> {
        let e = &self.entities[i];
        let b = self.insert_block(e)?;
        let mut acc = crate::geom::Acc::new();
        // the run's shape: its lines and curves, its texts and attribute templates left out
        for q in self.block_points(b, &["ATTDEF", "TEXT", "MTEXT"]) {
            acc.add(q[0], q[1]);
        }
        let bx = acc.get()?;
        let [sx, sy, _] = e
            .extra
            .as_ref()
            .and_then(|x| x.xform)
            .unwrap_or([1.0, 1.0, 0.0]);
        let (my, mx) = ((bx[1] + bx[3]) / 2.0, (bx[0] + bx[2]) / 2.0);
        let (a, z) = if (bx[2] - bx[0]) * sx.abs() >= (bx[3] - bx[1]) * sy.abs() {
            ([bx[0], my], [bx[2], my])
        } else {
            ([mx, bx[1]], [mx, bx[3]])
        };
        Some([self.place(i, a)?, self.place(i, z)?])
    }

    /// A block reference's drawn box as placed: the box of its block's geometry (attribute
    /// definitions left out: they are templates, not drawn in a reference), each point placed
    /// ([`Doc::place`]). For a device's symbol, where a cable ending at its edge meets it.
    /// `None` when the block draws nothing with a box.
    pub fn placed_box(&self, i: usize) -> Option<[f64; 4]> {
        let b = self.insert_block(&self.entities[i])?;
        let mut acc = crate::geom::Acc::new();
        for q in self.block_points(b, &["ATTDEF"]) {
            let p = self.place(i, q)?;
            acc.add(p[0], p[1]);
        }
        acc.get()
    }

    /// The outline points of block `b`'s own entities, those of the kinds in `leave_out` left
    /// out (attribute definitions are always left out but under a plant).
    fn block_points(&self, b: usize, leave_out: &[&str]) -> Vec<[f64; 2]> {
        let templates = crate::mutant::active("PlacedCountsTemplates");
        self.entities
            .iter()
            .enumerate()
            .filter(|(_, d)| d.owner == Some(b))
            .filter(|(_, d)| templates || !leave_out.contains(&&*d.kind))
            .flat_map(|(j, _)| self.outline(j).0)
            .collect()
    }

    /// The entity whose handle is `h`, the first if two have it.
    pub fn entity_by_handle(&self, h: u64) -> Option<usize> {
        self.entities.iter().position(|e| e.handle == Some(h))
    }

    /// The kind of the record that owns `e`.
    pub fn owner_kind(&self, e: &Entity) -> Option<BlockKind> {
        e.owner.and_then(|o| self.blocks.get(o)).map(|b| b.kind)
    }

    /// An entity's address: `@h:<HANDLE>`, else `#<N>` (its place among entities, from 1).
    pub fn entity_address(&self, i: usize) -> String {
        match self.entities[i].handle {
            Some(h) => crate::handle::key(h),
            None => format!("#{}", i + 1),
        }
    }

    /// A block record's address: `@h:<HANDLE>`, its reader's key, else its name.
    pub fn block_address(&self, i: usize) -> String {
        let b = &self.blocks[i];
        match (b.handle, &b.key) {
            (Some(h), _) => crate::handle::key(h),
            (None, Some(k)) => k.clone(),
            (None, None) => format!("name:{}", b.name),
        }
    }

    /// Handles owned by two objects: how many distinct handles.
    pub fn duplicate_handles(&self) -> u64 {
        let mut seen = std::collections::BTreeSet::new();
        let dup = self
            .entities
            .iter()
            .filter(|e| e.duplicate)
            .filter_map(|e| e.handle)
            .chain(
                self.layers
                    .iter()
                    .filter(|l| l.duplicate)
                    .filter_map(|l| l.handle),
            )
            .chain(
                self.blocks
                    .iter()
                    .filter(|b| b.duplicate)
                    .filter_map(|b| b.handle),
            );
        for h in dup {
            seen.insert(h);
        }
        seen.len() as u64
    }

    /// Entity `i`'s outline in world x and y: a line's two ends, a polyline's vertices, a
    /// point; anything else, its box's corners (closed). With whether it is closed.
    pub fn outline(&self, i: usize) -> (Vec<[f64; 2]>, bool) {
        let e = &self.entities[i];
        if let Some(x) = e.extra.as_ref().filter(|x| !x.verts.is_empty()) {
            return (x.verts.clone(), x.closed);
        }
        let Some(b) = e.bbox else {
            return (Vec::new(), false);
        };
        match &*e.kind {
            "LINE" => {
                let rises = e.line_rises != crate::mutant::active("LineDiagonalFlipped");
                if rises {
                    (vec![[b[0], b[1]], [b[2], b[3]]], false)
                } else {
                    (vec![[b[0], b[3]], [b[2], b[1]]], false)
                }
            }
            "POINT" => (vec![[b[0], b[1]]], false),
            _ => (
                vec![[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]],
                true,
            ),
        }
    }

    /// Entities with no handle, in a drawing whose objects have them.
    pub fn missing_handles(&self) -> u64 {
        if self.handles == "off" {
            return 0;
        }
        self.entities.iter().filter(|e| e.handle.is_none()).count() as u64
    }
}
