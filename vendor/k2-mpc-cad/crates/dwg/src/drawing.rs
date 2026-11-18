//! A DWG drawing for the drawing-model read ([`DwgDrawing`]): the format-neutral model
//! ([`Doc`]: layers, block records, entities by handle with their common facts, boxes and
//! digests), the per-record tables ([`DrawTables`]: layer colours and transparency, linetypes,
//! text styles, layouts and their viewports, external references, dimension style names), and
//! one entity's raw geometry, decoded from its object again when a page asks for it.
//!
//! The drawing's settings, units, extents and next handle come from the header section
//! ([`crate::header`]); a header that doesn't read refuses the drawing.

use std::collections::{BTreeMap, HashMap};

use k2_mpc_base::index::{Digest, DigestStream};
use k2_mpc_cad_model::draw::{
    AttDefInfo, AttribRaw, DimOverrides, DimStyle, DrawSource, DrawTables, Edge, Ent, Geom,
    Gradient, HatchRaw, LayerDraw, LayoutInfo, Linetype, LoopRaw, LtElement, MTextRaw, Paper,
    PatLine, SplineRaw, TextRaw, TextStyle, Vert, Viewport, XrefPath,
};
use k2_mpc_cad_model::geom::{self, Acc};
use k2_mpc_cad_model::{
    Attrib, BboxQ, BlockKind, BlockRec, Doc, Entity, Extra, Interner, Layer, Props, Units,
};

use crate::classes::{classes, Class};
use crate::entities::{self, AttData, Body, HatchEdge, TextData};
use crate::error::{BAD_CONTAINER, OVER_BOUND};
use crate::frame::{self, lineweight, Eed, EntCommon, Ver, XItem};
use crate::objects::object_map;
use crate::polylines::{PolylineData, VertexData};
use crate::tables::{self, BlockHeader, LayerRec, LtypeRec, StyleRec};
use crate::{mutant, read_container, read_section, CadError, Limits};

const NEXT: &str = "the file is damaged: open a copy the app that made it saves again";

/// A drawing opened for the drawing-model read: its objects section, its model and tables, and
/// where each entity's object sits. A host keeps one per version while a viewer pages through
/// it, or opens one per call.
pub struct DwgDrawing {
    /// What the read tolerated: data past what the format names, never refused.
    tolerated: Tolerated,
    objects: Vec<u8>,
    ver: Ver,
    classes: Vec<Class>,
    /// Per entity of the model: its object's location.
    at: Vec<u64>,
    loc: HashMap<u64, u64>,
    max_objects: usize,
    /// Colour-book colours, by handle.
    books: HashMap<u64, tables::DbColor>,
    names: Names,
    /// The current layer's handle (`CLAYER`), when the header names one.
    clayer: Option<u64>,
    /// Per block record: its attribute definitions, in its own order.
    attdefs: Vec<Vec<AttDefInfo>>,
    /// Per block record: whether its record, its BLOCK or a settable attribute definition of it
    /// carries the annotative mark.
    annotative: Vec<bool>,
    doc: Doc,
    tables: DrawTables,
}

/// What a read tolerated and counted without refusing the file: data a newer or another writer
/// may add past what the format names, and the table entries and entities whose own read
/// refused, which the drawing leaves out or keeps in part (a host can say so).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tolerated {
    /// The header's string stream: bits past the texts the format names.
    pub header_extra_string_bits: usize,
    /// Objects (layers, dimension styles, entities) whose handle stream holds data past every
    /// handle the format names for their kind.
    pub objects_extra_handles: usize,
    /// Table entries left out of the drawing, their read refused: registered applications,
    /// linetypes, text styles, layers and block records, and dimension styles whose name doesn't
    /// read either.
    pub entries_left_out: usize,
    /// Dimension styles whose settings' read refused, kept by their name alone (no arrowhead
    /// settings).
    pub dimstyles_name_only: usize,
    /// Entities whose kind's read refused, kept with their common data alone (no geometry).
    pub entities_common_only: usize,
    /// Entities left out of the drawing: their kind's read refused, and their common data's too.
    pub entities_left_out: usize,
}

/// A table entry's read, as the drawing keeps it: the entry when it reads; `None` when its
/// object doesn't open (it isn't counted: the table's order already leaves out an entry whose
/// object doesn't open as its kind) or when its read refused, which `left_out` counts.
pub fn count_left_out<T>(read: Option<Result<T, CadError>>, left_out: &mut usize) -> Option<T> {
    match read {
        Some(Ok(v)) => Some(v),
        Some(Err(_)) => {
            if !mutant::active("LeftOutUncounted") {
                *left_out += 1;
            }
            None
        }
        None => None,
    }
}

/// What an entity's read cut: its kind's read refused (`common_only`), and its common data's
/// too (`left_out`).
#[derive(Default)]
struct Cut {
    common_only: bool,
    left_out: bool,
}

/// Names by handle: layers, linetypes, text styles, block records, dimension styles,
/// registered applications.
#[derive(Debug, Default, Clone)]
pub struct Names {
    pub layer: HashMap<u64, String>,
    pub ltype: HashMap<u64, String>,
    pub style: HashMap<u64, String>,
    pub block: HashMap<u64, String>,
    pub dimstyle: HashMap<u64, String>,
    pub app: HashMap<u64, String>,
}

impl Names {
    fn of(m: &HashMap<u64, String>, h: Option<u64>, default: &str) -> String {
        h.and_then(|h| m.get(&h).cloned())
            .unwrap_or_else(|| default.to_owned())
    }
}

/// The release a version stamp names.
fn release(stamp: &str) -> Option<&'static str> {
    Some(match stamp {
        "AC1018" => "R2004",
        "AC1024" => "R2010",
        "AC1027" => "R2013",
        "AC1032" => "R2018",
        _ => return None,
    })
}

fn feed(d: &mut DigestStream, tag: &str, v: &str) {
    d.update(&(tag.len() as u64).to_le_bytes());
    d.update(tag.as_bytes());
    d.update(&(v.len() as u64).to_le_bytes());
    d.update(v.as_bytes());
}

/// A kind's name as DXF files name it: the vertices, polylines and dimensions by their DXF
/// kind, a multiple reference as a reference.
fn dxf_kind(kind: &str) -> &str {
    if kind.starts_with("VERTEX_") {
        "VERTEX"
    } else if kind.starts_with("POLYLINE_") {
        "POLYLINE"
    } else if kind.starts_with("DIMENSION_") {
        "DIMENSION"
    } else if kind == "MINSERT" {
        "INSERT"
    } else {
        kind
    }
}

/// An entity's own linetype, as DXF names it: none for by layer; by block and continuous by
/// the linetype table's own spelling of them.
fn ltype_name(c: &EntCommon, names: &Names) -> Option<String> {
    let table = |want: &str| {
        names
            .ltype
            .values()
            .find(|n| n.eq_ignore_ascii_case(want))
            .cloned()
            .unwrap_or_else(|| want.to_owned())
    };
    match c.lt_flags {
        1 => Some(table("ByBlock")),
        2 => Some(table("Continuous")),
        3 => Some(Names::of(&names.ltype, c.ltype, "Continuous")),
        _ => None,
    }
}

/// A transparency as DXF codes it (`0x02000000 | alpha`, `0x01000000` by block); none by layer.
/// Its kind is the high byte: 1 by block, 2 or 3 a value in the low byte.
fn alpha_code(t: Option<u32>) -> Option<i64> {
    let t = t?;
    match t >> 24 {
        1 => Some(0x0100_0000),
        2 | 3 => Some(0x0200_0000 | i64::from(t & 0xFF)),
        _ => None,
    }
}

/// An entity's colour as DXF states it: its colour number (none by layer) and its true colour
/// (`0xRRGGBB`). A true colour's high byte says which it is: 0xC2 a true colour, 0xC3 a colour
/// number in its low byte, 0xC1 by block, 0xC0 by layer.
fn colour_fields(c: &crate::bits::Colour) -> (Option<i64>, Option<u32>) {
    let by_index = (c.index != 256).then_some(i64::from(c.index));
    match c.rgb {
        Some(v) if v >> 24 == 0xC3 => (Some(i64::from(v & 0xFF)), None),
        Some(v) if v >> 24 == 0xC1 => (Some(0), None),
        Some(v) if v >> 24 == 0xC0 => (None, None),
        Some(v) => (by_index, Some(v & 0x00FF_FFFF)),
        None => (by_index, None),
    }
}

/// Radians as degrees (the drawing model's angles for arcs, texts, references, hatches).
fn rad(x: f64) -> f64 {
    if mutant::active("RadiansKept") {
        return x;
    }
    x.to_degrees()
}

impl DwgDrawing {
    /// Reads `bytes`: the container, the object map, the class list, the tables and every
    /// entity's common data and box. An encrypted drawing is refused.
    pub fn open(bytes: &[u8], limits: &Limits) -> Result<Self, CadError> {
        let c = read_container(bytes, limits)?;
        let ver = Ver::of(&c.version)?;
        let map = read_section(bytes, &c, "AcDb:Handles", limits)?;
        let objects = read_section(bytes, &c, "AcDb:AcDbObjects", limits)?;
        let cls = read_section(bytes, &c, "AcDb:Classes", limits)
            .and_then(|s| classes(&s, &c.version, c.maintenance))?;
        let entries = object_map(&map, limits)?;
        let hv = crate::header::header(
            &read_section(bytes, &c, "AcDb:Header", limits)?,
            ver,
            c.maintenance,
        )?;
        let measurement = read_section(bytes, &c, "AcDb:Template", limits)
            .ok()
            .and_then(|t| crate::header::measurement(&t).ok());
        let mut hd = Digest::stream();
        feed(&mut hd, "header", &hv.stable);
        feed(&mut hd, "measurement", &format!("{measurement:?}"));
        let mut loc: HashMap<u64, u64> = HashMap::with_capacity(entries.len());
        // each object's type, by handle, in the map's order
        let mut by_type: BTreeMap<u16, Vec<u64>> = BTreeMap::new();
        for e in &entries {
            loc.insert(e.handle, e.location);
            if let Ok(o) = frame::open(&objects, e.location, ver) {
                by_type.entry(o.kind).or_default().push(e.handle);
            }
        }
        let hmax = entries.iter().map(|e| e.handle).max();
        let mut d = DwgDrawing {
            tolerated: Tolerated {
                header_extra_string_bits: hv.extra_string_bits,
                ..Tolerated::default()
            },
            objects,
            ver,
            classes: cls,
            at: Vec::new(),
            loc,
            max_objects: limits.max_objects,
            books: HashMap::new(),
            names: Names::default(),
            clayer: Some(hv.clayer).filter(|h| *h != 0),
            attdefs: Vec::new(),
            annotative: Vec::new(),
            doc: Doc {
                format: "dwg",
                version: c.version.clone(),
                release: release(&c.version),
                encoding: if ver.strings() {
                    "utf-16le".into()
                } else {
                    format!(
                        "ansi_{}",
                        if c.code_page == 30 {
                            1252
                        } else {
                            u32::from(c.code_page)
                        }
                    )
                },
                eol: None,
                units: Units::from_codes(Some(i64::from(hv.insunits)), measurement),
                handles: "on",
                writer_mark: None,
                header: hd.finish(),
                header_extents: Some((hv.extmin, hv.extmax)),
                hmax,
                handseed: Some(hv.handseed),
                layers: Vec::new(),
                blocks: Vec::new(),
                entities: Vec::new(),
                xrefs: Vec::new(),
            },
            tables: DrawTables {
                settings: k2_mpc_cad_model::draw::Settings {
                    ltscale: hv.ltscale,
                    psltscale: hv.psltscale,
                    lwdisplay: hv.lwdisplay,
                    fillmode: hv.fillmode,
                    pdmode: i64::from(hv.pdmode),
                    pdsize: hv.pdsize,
                    mirrtext: hv.mirrtext,
                    ..Default::default()
                },
                ..DrawTables::default()
            },
        };
        d.read_tables(&by_type)?;
        Ok(d)
    }

    /// What the read tolerated and counted: data past what the format names, and the entries
    /// and entities left out or kept in part.
    pub fn tolerated(&self) -> Tolerated {
        self.tolerated
    }

    fn obj(&self, h: u64) -> Option<frame::Obj<'_>> {
        let at = *self.loc.get(&h)?;
        frame::open(&self.objects, at, self.ver).ok()
    }

    /// A table's entries in its control object's order; the map's order when the control
    /// object doesn't read.
    fn ordered(
        &self,
        control: u16,
        entry: u16,
        extra: usize,
        by_type: &BTreeMap<u16, Vec<u64>>,
    ) -> Vec<u64> {
        let mut out: Vec<u64> = by_type
            .get(&control)
            .and_then(|v| v.first())
            .and_then(|&h| self.obj(h))
            .and_then(|mut o| tables::control(&mut o, extra).ok())
            .unwrap_or_default();
        out.retain(|h| self.obj(*h).is_some_and(|o| o.kind == entry));
        if !out.is_empty() {
            return out;
        }
        let mut seen: std::collections::HashSet<u64> = out.iter().copied().collect();
        for &h in by_type.get(&entry).map(Vec::as_slice).unwrap_or(&[]) {
            if seen.insert(h) {
                out.push(h);
            }
        }
        out
    }

    fn read_tables(&mut self, by_type: &BTreeMap<u16, Vec<u64>>) -> Result<(), CadError> {
        let empty = Digest::stream().finish();
        // colour-book colours
        let books: Vec<(u64, tables::DbColor)> = by_type
            .values()
            .flatten()
            .filter_map(|&h| {
                let mut o = self.obj(h)?;
                let (k, _) = entities::kind_of(o.kind, &self.classes)?;
                (k == "DBCOLOR")
                    .then(|| tables::dbcolor(&mut o).ok().map(|c| (h, c)))
                    .flatten()
            })
            .collect();
        self.books = books.into_iter().collect();
        // image definitions: their image files' paths (never opened)
        for &h in by_type.values().flatten() {
            let Some(mut o) = self.obj(h) else { continue };
            if entities::kind_of(o.kind, &self.classes).is_some_and(|(k, _)| k == "IMAGEDEF") {
                if let Ok(p) = tables::imagedef(&mut o) {
                    self.tables.image_defs.push((h, p));
                }
            }
        }
        // registered applications, and dimension styles (a leader's arrowhead settings)
        for &h in by_type.get(&0x43).map(Vec::as_slice).unwrap_or(&[]) {
            let r = self.obj(h).map(|mut o| tables::appid(&mut o));
            if let Some(e) = count_left_out(r, &mut self.tolerated.entries_left_out) {
                self.names.app.insert(h, e.name);
            }
        }
        for h in self.ordered(0x44, 0x45, 0, by_type) {
            let read = self.obj(h).map(|mut o| {
                let r = tables::dimstyle(&mut o);
                (tables::extra_handle_bits(&o), r)
            });
            if read.as_ref().is_some_and(|(x, r)| *x > 0 && r.is_ok()) {
                self.tolerated.objects_extra_handles += 1;
            }
            match read.map(|(_, r)| r) {
                Some(Ok(d)) => {
                    self.names.dimstyle.insert(h, d.entry.name.clone());
                    self.tables.dimstyles.push(DimStyle {
                        name: d.entry.name,
                        arrow_size: Some(d.arrow_size),
                        scale: Some(d.scale),
                        leader_arrow: d.leader_arrow,
                    });
                }
                // the object doesn't open: left out, uncounted (as `count_left_out`)
                None => {}
                // its settings refused: its name alone
                Some(Err(_)) => {
                    let r = self.obj(h).map(|mut o| tables::dimstyle_name(&mut o));
                    if let Some(n) = count_left_out(r, &mut self.tolerated.entries_left_out) {
                        self.tolerated.dimstyles_name_only += 1;
                        self.names.dimstyle.insert(h, n.clone());
                        self.tables.dimstyles.push(DimStyle {
                            name: n,
                            ..DimStyle::default()
                        });
                    }
                }
            }
        }
        // linetypes
        let mut lts: Vec<(u64, LtypeRec)> = Vec::new();
        // the control object lists BYLAYER and BYBLOCK after the others; a DXF file's table
        // begins with them (BYBLOCK first)
        let mut lt_order = self.ordered(0x38, 0x39, 2, by_type);
        for name in ["BYLAYER", "BYBLOCK"] {
            let named = |h: &u64| {
                self.obj(*h)
                    .and_then(|mut x| tables::ltype(&mut x).ok())
                    .is_some_and(|l| l.entry.name.eq_ignore_ascii_case(name))
            };
            if let Some(k) = lt_order.iter().position(named) {
                let h = lt_order.remove(k);
                lt_order.insert(0, h);
            }
        }
        for h in lt_order {
            let r = self.obj(h).map(|mut o| tables::ltype(&mut o));
            if let Some(l) = count_left_out(r, &mut self.tolerated.entries_left_out) {
                self.names.ltype.insert(h, l.entry.name.clone());
                lts.push((h, l));
            }
        }
        // text styles
        let mut sts: Vec<(u64, StyleRec, Vec<Eed>)> = Vec::new();
        for h in self.ordered(0x34, 0x35, 0, by_type) {
            let r = self
                .obj(h)
                .map(|mut o| tables::style(&mut o).map(|s| (o.eed.clone(), s)));
            if let Some((eed, s)) = count_left_out(r, &mut self.tolerated.entries_left_out) {
                self.names.style.insert(h, s.entry.name.clone());
                sts.push((h, s, eed));
            }
        }
        // layers
        let mut lys: Vec<(u64, LayerRec, Vec<Eed>)> = Vec::new();
        for h in self.ordered(0x32, 0x33, 0, by_type) {
            let r = self.obj(h).map(|mut o| {
                let l = tables::layer(&mut o);
                (o.eed.clone(), tables::extra_handle_bits(&o), l)
            });
            if r.as_ref().is_some_and(|(_, x, l)| *x > 0 && l.is_ok()) {
                self.tolerated.objects_extra_handles += 1;
            }
            let r = r.map(|(eed, _, l)| l.map(|l| (eed, l)));
            if let Some((eed, l)) = count_left_out(r, &mut self.tolerated.entries_left_out) {
                self.names.layer.insert(h, l.entry.name.clone());
                lys.push((h, l, eed));
            }
        }
        // block records: model space and paper space after the others in the control object
        let mut bhs: Vec<(u64, BlockHeader)> = Vec::new();
        let mut rep: HashMap<u64, u64> = HashMap::new();
        let mut record_eed: HashMap<u64, Vec<Eed>> = HashMap::new();
        for h in self.ordered(0x30, 0x31, 2, by_type) {
            let r = self
                .obj(h)
                .map(|mut o| tables::block_header(&mut o).map(|b| (o.eed.clone(), b)));
            if let Some((eed, b)) = count_left_out(r, &mut self.tolerated.entries_left_out) {
                if let Some(t) = dynamic_target(&eed, &self.names.app) {
                    rep.insert(h, t);
                }
                record_eed.insert(h, eed);
                self.names.block.insert(h, b.entry.name.clone());
                bhs.push((h, b));
            }
        }
        let rank = |b: &BlockHeader| {
            let n = b.entry.name.to_ascii_uppercase();
            if n == "*MODEL_SPACE" {
                0
            } else if n.starts_with("*PAPER_SPACE") {
                1
            } else {
                2
            }
        };
        bhs.sort_by_key(|(_, b)| rank(b));

        // the model's layers
        for (h, l, eed) in &lys {
            let mut r = Digest::stream();
            feed(&mut r, "name", &l.entry.name);
            feed(&mut r, "flags", &l.flags.to_string());
            feed(&mut r, "colour", &format!("{:?}", l.colour));
            feed(&mut r, "ltype", &Names::of(&self.names.ltype, l.ltype, ""));
            let mut c = Digest::stream();
            feed(&mut c, "eed", &format!("{eed:?}"));
            // a true colour's colour number: the palette's nearest (an object colour's own number
            // is written as 0)
            let index = match l.colour.rgb {
                Some(c) if l.colour.index == 0 => nearest_aci(c),
                _ => i64::from(l.colour.index),
            };
            self.doc.layers.push(Layer {
                handle: Some(*h),
                duplicate: false,
                name: l.entry.name.clone(),
                color: Some(if l.off() { -index } else { index }),
                frozen: l.frozen(),
                locked: l.locked(),
                plot: l.plotted(),
                ltype: l.ltype.and_then(|t| self.names.ltype.get(&t).cloned()),
                lw: lineweight(l.lineweight()).map(|w| w.to_string()),
                xref: l.entry.xdep || l.entry.name.contains('|'),
                record: r.finish(),
                content: c.finish(),
            });
            let alpha = self.xdata(eed, "AcCmTransparency").and_then(|x| {
                x.iter().find_map(|i| match i {
                    XItem::Long(v) => Some(i64::from(*v)),
                    _ => None,
                })
            });
            self.tables.layers.push(LayerDraw {
                rgb: l.colour.rgb,
                alpha,
            });
        }
        self.tables.linetypes = lts
            .iter()
            .map(|(h, l)| {
                let mut complex = Vec::new();
                for (k, d) in l.dashes.iter().enumerate() {
                    if d.flags & 6 != 0 {
                        complex.push(LtElement {
                            after: k,
                            kind: if d.flags & 2 != 0 { "text" } else { "shape" },
                            text: d.text.clone(),
                            shape: Some(i64::from(d.shape)),
                            style: d.style,
                            scale: d.scale,
                            rotation: d.rotation,
                            offset: d.offset,
                        });
                    }
                }
                Linetype {
                    handle: Some(*h),
                    name: l.entry.name.clone(),
                    length: l.length,
                    dashes: l.dashes.iter().map(|d| d.length).collect(),
                    complex,
                }
            })
            .collect();
        self.tables.text_styles = sts
            .iter()
            .map(|(h, s, eed)| {
                let acad = self.xdata(eed, "ACAD");
                let family = acad
                    .and_then(|x| {
                        x.iter().find_map(|i| match i {
                            XItem::Str(t) => Some(t.clone()),
                            _ => None,
                        })
                    })
                    .filter(|t| !t.is_empty());
                let fflags = acad
                    .and_then(|x| {
                        x.iter().find_map(|i| match i {
                            XItem::Long(v) => Some(*v),
                            _ => None,
                        })
                    })
                    .unwrap_or(0);
                TextStyle {
                    handle: Some(*h),
                    name: s.entry.name.clone(),
                    file: s.font.clone(),
                    bigfont: Some(s.bigfont.clone()).filter(|b| !b.is_empty()),
                    family,
                    fixed_height: s.fixed_height,
                    width: s.width,
                    oblique: s.oblique,
                    bold: fflags & 0x0200_0000 != 0,
                    italic: fflags & 0x0100_0000 != 0,
                    backwards: s.generation & 2 != 0,
                    upside_down: s.generation & 4 != 0,
                    vertical: s.vertical,
                }
            })
            .collect();

        // the model's block records
        // a name the file repeats (anonymous blocks are written without the number a reader
        // gives them, paper spaces as one name) is numbered, so every record's name is its own
        let finals = unique_names(bhs.iter().map(|(_, b)| b.entry.name.as_str()));
        for ((h, b), name) in bhs.iter().zip(finals) {
            self.names.block.insert(*h, name.clone());
            let n = name.to_ascii_uppercase();
            let kind = if n == "*MODEL_SPACE" {
                BlockKind::Model
            } else if n.starts_with("*PAPER_SPACE") {
                BlockKind::Layout
            } else {
                BlockKind::Block
            };
            let mut r = Digest::stream();
            feed(&mut r, "name", &b.entry.name);
            feed(
                &mut r,
                "flags",
                &format!("{} {} {} {}", b.anonymous, b.has_attdefs, b.xref, b.overlay),
            );
            feed(&mut r, "base", &format!("{:?}", b.base));
            feed(&mut r, "path", &b.xref_path);
            feed(&mut r, "description", &b.description);
            self.doc.blocks.push(BlockRec {
                handle: Some(*h),
                duplicate: false,
                key: None,
                name: name.clone(),
                kind,
                base: Some(b.base),
                anon: kind == BlockKind::Block && name.starts_with('*'),
                xref: b.xref,
                path: Some(b.xref_path.clone()).filter(|p| b.xref && !p.is_empty()),
                layout: None,
                dynamic_of: None,
                record: r.finish(),
                content: empty,
            });
            if b.xref {
                self.tables.xrefs.push(XrefPath {
                    block: self.doc.blocks.len() - 1,
                    path: b.xref_path.clone(),
                    overlay: b.overlay,
                });
                self.doc.xrefs.push(b.xref_path.clone());
            }
        }
        // an anonymous block that draws a dynamic block's reference as set names that block's
        // record in its record's extension data
        for k in 0..self.doc.blocks.len() {
            if !self.doc.blocks[k].anon {
                continue;
            }
            let of = self.doc.blocks[k]
                .handle
                .and_then(|h| rep.get(&h))
                .and_then(|t| self.doc.blocks.iter().position(|b| b.handle == Some(*t)))
                .filter(|&t| {
                    t != k
                        && self.doc.blocks[t].kind == BlockKind::Block
                        && !self.doc.blocks[t].anon
                });
            self.doc.blocks[k].dynamic_of = of;
        }
        if !self.doc.blocks.iter().any(|b| b.kind == BlockKind::Model) {
            self.doc.blocks.push(BlockRec {
                handle: None,
                duplicate: false,
                key: Some("@space:model".into()),
                name: "*Model_Space".into(),
                kind: BlockKind::Model,
                base: None,
                anon: false,
                xref: false,
                path: None,
                layout: None,
                dynamic_of: None,
                record: empty,
                content: empty,
            });
        }
        let nblocks = self.doc.blocks.len();
        // each block record's attribute definitions and annotative mark (its record's, its
        // BLOCK's or a settable attribute definition's extension data), as the DXF read judges
        // them
        let mut attdefs = vec![Vec::new(); nblocks];
        let mut annotative = vec![false; nblocks];
        for (bi, (h, b)) in bhs.iter().enumerate() {
            let mut annot = record_eed
                .get(h)
                .is_some_and(|e| annotative_eed(e, &self.names.app));
            let decode = |x: u64| {
                let at = *self.loc.get(&x)?;
                entities::entity(&self.objects, at, self.ver, &self.classes).ok()
            };
            if let Some(be) = b.block_entity.and_then(decode) {
                annot |= annotative_eed(&be.eed, &self.names.app);
            }
            for &x in &b.entities {
                let Some(e) = decode(x) else { continue };
                if let Some(info) = attdef_info(&e) {
                    if !info.constant {
                        annot |= annotative_eed(&e.eed, &self.names.app);
                    }
                    attdefs[bi].push(info);
                }
            }
            annotative[bi] = annot;
        }
        self.attdefs = attdefs;
        self.annotative = annotative;
        self.tables.layouts = vec![LayoutInfo::default(); nblocks];
        self.tables.sortents = vec![None; nblocks];

        // the entities, block by block in each block's own order
        let mut names = Interner::default();
        for (bi, (_, b)) in bhs.iter().enumerate() {
            for &h in &b.entities {
                let Some(&at) = self.loc.get(&h) else {
                    continue;
                };
                let mut cut = Cut::default();
                let e = self.entity_model(h, at, bi, &mut names, &mut cut);
                if cut.left_out && !mutant::active("LeftOutUncounted") {
                    self.tolerated.entities_left_out += 1;
                }
                if let Some((e, extra)) = e {
                    if cut.common_only {
                        self.tolerated.entities_common_only += 1;
                    }
                    if extra > 0 {
                        self.tolerated.objects_extra_handles += 1;
                    }
                    if self.doc.entities.len() >= self.max_objects {
                        return Err(CadError::new(
                            OVER_BOUND,
                            "more entities than the object limit",
                            "raise the object limit for a file you trust",
                        ));
                    }
                    self.doc.entities.push(e);
                    self.at.push(at);
                }
            }
        }

        // layouts: their paper, name and tab, and their viewports (the sheet's own, the first
        // in the layout's block, left out)
        let layouts: Vec<tables::LayoutRec> = by_type
            .values()
            .flatten()
            .filter_map(|&h| {
                let mut o = self.obj(h)?;
                let (k, _) = entities::kind_of(o.kind, &self.classes)?;
                (k == "LAYOUT")
                    .then(|| tables::layout(&mut o).ok())
                    .flatten()
            })
            .collect();
        for l in layouts {
            let Some(b) = l
                .block
                .and_then(|bh| self.doc.blocks.iter().position(|r| r.handle == Some(bh)))
            else {
                continue;
            };
            self.doc.blocks[b].layout = Some(l.name.clone());
            let info = &mut self.tables.layouts[b];
            info.name = Some(l.name.clone());
            info.tab = Some(i64::from(l.tab));
            info.paper = (l.paper[0] > 0.0 && l.paper[1] > 0.0).then_some(Paper {
                w_mm: l.paper[0],
                h_mm: l.paper[1],
                rotation: i64::from(l.rotation),
                margins_mm: l.margins,
            });
        }
        // draw-order tables
        let sorts: Vec<tables::SortTable> = by_type
            .values()
            .flatten()
            .filter_map(|&h| {
                let mut o = self.obj(h)?;
                let (k, _) = entities::kind_of(o.kind, &self.classes)?;
                (k == "SORTENTSTABLE")
                    .then(|| tables::sortents(&mut o).ok())
                    .flatten()
            })
            .collect();
        for (block, pairs) in sorts {
            let b = block.and_then(|bh| self.doc.blocks.iter().position(|r| r.handle == Some(bh)));
            if let Some(b) = b {
                self.tables.sortents[b] = Some(k2_mpc_cad_model::draw::SortTable { pairs });
            }
        }
        let mut first_seen = vec![false; nblocks];
        for i in 0..self.doc.entities.len() {
            if &*self.doc.entities[i].kind != "VIEWPORT" {
                continue;
            }
            let Some(b) = self.doc.entities[i].owner else {
                continue;
            };
            if self.doc.blocks[b].kind != BlockKind::Layout {
                continue;
            }
            if !first_seen[b] {
                first_seen[b] = true;
                continue;
            }
            let Ok(e) = entities::entity(&self.objects, self.at[i], self.ver, &self.classes) else {
                continue;
            };
            let Body::Viewport(v) = e.body else { continue };
            let plan = v.direction[0].abs() < 1e-12
                && v.direction[1].abs() < 1e-12
                && v.direction[2] > 0.0;
            self.tables.layouts[b].viewports.push(Viewport {
                handle: Some(e.handle),
                centre: [v.centre[0], v.centre[1]],
                size: v.size,
                view_centre: v.view_centre,
                view_height: v.view_height,
                rotation: v.twist,
                on: v.status & 0x20000 != 0,
                frozen_layers: v.frozen,
                clip: if v.status & 0x10000 != 0 {
                    v.clip
                } else {
                    None
                },
                plan,
            });
        }
        Ok(())
    }

    /// The extension data of the application named `app` in `eed`.
    fn xdata<'e>(&self, eed: &'e [Eed], app: &str) -> Option<&'e [XItem]> {
        eed.iter()
            .find(|x| {
                self.names
                    .app
                    .get(&x.app)
                    .is_some_and(|n| n.eq_ignore_ascii_case(app))
            })
            .map(|x| x.items.as_slice())
    }

    /// The model's entity for the object `h` at `at`, in block `owner`; `None` for an object
    /// that isn't an entity (or BLOCK and ENDBLK, which the block record stands for).
    fn entity_model(
        &self,
        h: u64,
        at: u64,
        owner: usize,
        names: &mut Interner,
        cut: &mut Cut,
    ) -> Option<(Entity, usize)> {
        let o = frame::open(&self.objects, at, self.ver).ok()?;
        let (kind, is_entity) = entities::kind_of(o.kind, &self.classes)?;
        if !is_entity || kind == "BLOCK" || kind == "ENDBLK" {
            return None;
        }
        let decoded = entities::entity(&self.objects, at, self.ver, &self.classes);
        // a decode refused: its common data alone, when that reads
        let mut extra_bits = 0;
        let (common, eed, body) = match decoded {
            Ok(e) => {
                extra_bits = e.extra_handle_bits;
                (e.common, e.eed, Some(e.body))
            }
            Err(_) => {
                let mut o = o;
                let eed = o.eed.clone();
                let Ok(c) = frame::entity_common(&mut o) else {
                    cut.left_out = true;
                    return None;
                };
                cut.common_only = true;
                (c, eed, None)
            }
        };
        let dk = dxf_kind(&kind).to_owned();
        let layer = Names::of(&self.names.layer, common.layer, "0");
        let (aci, mut true_colour) = colour_fields(&common.colour);
        // a colour-book colour: its true colour from its colour object
        if let Some(b) = common.book.and_then(|h| self.books.get(&h)) {
            true_colour = b.rgb.or(true_colour);
        }
        let colour = aci.map(|n| n.to_string());
        let rgb = true_colour.map(|c| c.to_string());
        let ltype = ltype_name(&common, &self.names);
        let lw = lineweight(common.lineweight)
            .filter(|w| *w != -1)
            .map(|w| w.to_string());
        let alpha = alpha_code(common.colour.transparency).map(|a| a.to_string());
        let props = (colour.is_some()
            || rgb.is_some()
            || ltype.is_some()
            || lw.is_some()
            || alpha.is_some())
        .then(|| {
            let mut p = |v: &Option<String>| v.as_deref().map(|v| names.get(v));
            Box::new(Props {
                color: p(&colour),
                rgb: p(&rgb),
                ltype: p(&ltype),
                lw: p(&lw),
                alpha: p(&alpha),
            })
        });
        // the attributes, typed
        let mut attribs: Vec<Attrib> = Vec::new();
        if let Some(Body::Insert(i)) = &body {
            for &a in &i.attribs {
                let Some(&aat) = self.loc.get(&a) else {
                    continue;
                };
                if let Ok(ae) = entities::entity(&self.objects, aat, self.ver, &self.classes) {
                    if let Body::Attrib(t) = ae.body {
                        attribs.push(Attrib {
                            tag: names.get(t.tag.trim()),
                            value: t.text.text.clone().into_boxed_str(),
                            handle: Some(a),
                            at: Some(t.text.ins),
                        });
                    }
                }
            }
        }
        let n = body.as_ref().map_or([0.0, 0.0, 1.0], normal);
        let up = geom::is_up(n);
        let (mut bbox, mut bbox_q) = match &body {
            Some(Body::Polyline(p)) => poly_bbox(p, &self.vertices(p).unwrap_or_default(), n, up),
            b => b.as_ref().map_or((None, BboxQ::None), |b| bbox(b, n, up)),
        };
        // a text's box: laid out as the drawing-model read lays it (one computation for both)
        if let Some(b) = body
            .as_ref()
            .filter(|b| matches!(b, Body::Text(_) | Body::AttDef(_) | Body::MText(_)))
        {
            let probe = entities::Entity {
                handle: h,
                kind: kind.clone(),
                type_number: 0,
                eed: Vec::new(),
                common: common.clone(),
                body: b.clone(),
                extra_handle_bits: 0,
            };
            let tb = match geom_of(&probe, &self.names, Vec::new()) {
                Geom::Text(t) | Geom::AttDef { text: t, .. } => Some(
                    k2_mpc_cad_model::draw::text::text_box(&t, &self.tables.text_styles),
                ),
                Geom::MText(m) => Some(k2_mpc_cad_model::draw::text::mtext_box(&m)),
                _ => None,
            };
            if let Some(tb) = tb.filter(|x| x.iter().all(|v| v.is_finite())) {
                bbox = Some(tb);
                bbox_q = BboxQ::Approx;
            }
        }
        let text = match &body {
            Some(Body::Text(t)) => Some(t.text.clone()),
            Some(Body::AttDef(a)) => Some(a.text.text.clone()),
            Some(Body::MText(m)) => Some(m.text.clone()),
            Some(Body::Mleader(m)) => m.content.text.clone(),
            Some(Body::Insert(_)) if !attribs.is_empty() => Some(
                attribs
                    .iter()
                    .map(|a| format!("{}={}", a.tag, a.value))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        };
        let at2 = match &body {
            Some(Body::Text(t)) => Some(t.ins),
            Some(Body::AttDef(a)) => Some(a.text.ins),
            Some(Body::MText(m)) => Some([m.ins[0], m.ins[1]]),
            Some(Body::Insert(i)) => Some([i.ins[0], i.ins[1]]),
            Some(Body::Mleader(m)) => m.content.at,
            _ => None,
        };
        let block = match &body {
            Some(Body::Insert(i)) => i
                .block
                .and_then(|b| self.names.block.get(&b))
                .map(|n| names.get(n)),
            _ => None,
        };
        let verts: Vec<[f64; 2]> = match &body {
            Some(Body::LwPolyline(l)) => l
                .points
                .iter()
                .filter_map(|q| {
                    if up {
                        Some(*q)
                    } else {
                        geom::to_world([q[0], q[1], l.elevation], n).map(|w| [w[0], w[1]])
                    }
                })
                .collect(),
            // a multileader's outline: its first leader's first line, then that leader's landing
            Some(Body::Mleader(m)) => m.content.first_line(),
            // a leader's points, its arrow first, in world x and y
            Some(Body::Leader(l)) => l.points.iter().map(|p| [p[0], p[1]]).collect(),
            // an old polyline's vertices, as the DXF read keeps them (x and y as written; a
            // polyline in a tilted frame taken to world x and y at elevation 0)
            Some(Body::Polyline(p)) => self
                .vertices(p)
                .unwrap_or_default()
                .iter()
                .filter_map(|v| {
                    if up {
                        Some([v.p[0], v.p[1]])
                    } else {
                        geom::to_world([v.p[0], v.p[1], 0.0], n).map(|w| [w[0], w[1]])
                    }
                })
                .collect(),
            _ => Vec::new(),
        };
        let closed = match &body {
            Some(Body::LwPolyline(l)) => l.closed(),
            Some(Body::Polyline(p)) => p.flags & 1 != 0,
            _ => false,
        };
        let followers = match &body {
            Some(Body::Polyline(p)) => p.vertices.len() as u32,
            _ => attribs.len() as u32,
        };
        // a reference's scale along its block's x and y and its rotation (degrees), when any
        // differs from 1, 1 and 0
        let xform = match &body {
            Some(Body::Insert(i)) => {
                Some([i.scale[0], i.scale[1], rad(i.rotation)]).filter(|x| *x != [1.0, 1.0, 0.0])
            }
            _ => None,
        };
        let annotation = match &body {
            Some(Body::Leader(l)) => l.annotation,
            _ => None,
        };
        let extra =
            (!attribs.is_empty() || !verts.is_empty() || xform.is_some() || annotation.is_some())
                .then(|| {
                    Box::new(Extra {
                        attribs: attribs.clone(),
                        verts,
                        closed,
                        xform,
                        annotation,
                        lines: match &body {
                            Some(Body::Mleader(m)) => m.content.lines(),
                            _ => Vec::new(),
                        },
                    })
                });
        let line_rises = match &body {
            Some(Body::Line { p1, p2, .. }) => (p2[0] - p1[0]) * (p2[1] - p1[1]) >= 0.0,
            _ => true,
        };
        let mut g = Digest::stream();
        feed(&mut g, "kind", &dk);
        feed(&mut g, "body", &format!("{body:?}"));
        for a in &attribs {
            feed(&mut g, "attrib", &format!("{} {:?}", a.tag, a.at));
        }
        let geo = g.finish();
        let mut geom8 = [0u8; 8];
        geom8.copy_from_slice(&geo.as_bytes()[..8]);
        let mut c = Digest::stream();
        feed(&mut c, "eed", &format!("{eed:?}"));
        feed(
            &mut c,
            "reactors",
            &format!("{:?} {:?}", common.reactors, common.xdic),
        );
        let content = c.finish();
        let mut r = Digest::stream();
        feed(&mut r, "kind", &dk);
        feed(&mut r, "layer", &layer);
        feed(
            &mut r,
            "props",
            &format!("{colour:?} {rgb:?} {ltype:?} {lw:?} {alpha:?}"),
        );
        feed(&mut r, "geom", &format!("{geom8:?}"));
        feed(&mut r, "text", text.as_deref().unwrap_or(""));
        feed(&mut r, "content", &format!("{:?}", content.as_bytes()));
        let reactors = !common.reactors.is_empty();
        let block_name = block.as_deref();
        let edit = edit_reason(&dk, up, reactors, block_name);
        let xd: Vec<_> = eed
            .iter()
            .filter_map(|x| self.names.app.get(&x.app))
            .map(|n| names.get(n))
            .collect();
        Some((
            Entity {
                handle: Some(h),
                duplicate: false,
                kind: names.get(&dk),
                owner: Some(owner),
                layer: names.get(&layer),
                props,
                geom: geom8,
                content,
                record: r.finish(),
                bbox,
                bbox_q,
                ocs: !up,
                xd,
                edit,
                followers,
                text: text.map(String::into_boxed_str),
                block,
                at: at2,
                line_rises,
                extra,
            },
            extra_bits,
        ))
    }

    /// An old polyline's vertices, in order: `None` when one isn't there or doesn't read as a
    /// vertex.
    fn vertices(&self, p: &PolylineData) -> Option<Vec<VertexData>> {
        let mut out = Vec::with_capacity(p.vertices.len().min(1 << 16));
        for h in &p.vertices {
            let at = *self.loc.get(h)?;
            match entities::entity(&self.objects, at, self.ver, &self.classes)
                .ok()?
                .body
            {
                Body::Vertex(v) => out.push(v),
                _ => return None,
            }
        }
        Some(out)
    }

    /// Entity `i`'s raw geometry.
    fn geometry(&self, i: usize) -> Result<Ent, CadError> {
        let at = *self
            .at
            .get(i)
            .ok_or_else(|| CadError::new(BAD_CONTAINER, "no such entity", NEXT))?;
        let e = match entities::entity(&self.objects, at, self.ver, &self.classes) {
            Ok(e) => e,
            Err(_) => {
                return Ok(Ent::of(Geom::Unreadable));
            }
        };
        let mut attribs = Vec::new();
        if let Body::Insert(ins) = &e.body {
            for &a in &ins.attribs {
                let Some(&aat) = self.loc.get(&a) else {
                    continue;
                };
                let Ok(ae) = entities::entity(&self.objects, aat, self.ver, &self.classes) else {
                    continue;
                };
                if let Body::Attrib(t) = &ae.body {
                    attribs.push(attrib_raw(
                        a,
                        t,
                        Names::of(&self.names.layer, ae.common.layer, "0"),
                        Names::of(&self.names.style, t.text.style, "Standard"),
                    ));
                }
            }
        }
        let geom = match &e.body {
            // an old polyline: its vertices, every one read, or the polyline is unreadable
            Body::Polyline(p) => match self.vertices(p) {
                Some(v) => poly_geom(p, &v),
                None => Geom::Unreadable,
            },
            _ => geom_of(&e, &self.names, attribs),
        };
        Ok(Ent {
            geom,
            lt_scale: e.common.lt_scale,
            invisible: e.common.invisible,
            book: e
                .common
                .book
                .and_then(|h| self.books.get(&h))
                .map(tables::DbColor::dxf_name),
        })
    }
}

/// An old polyline's raw geometry from its vertices, as the DXF read gives it: a face mesh or a
/// mesh not modelled; a spline frame's control points left out; a 2D polyline's vertices at its
/// elevation, in its own frame; a 3D polyline's in world coordinates.
pub fn poly_geom(p: &PolylineData, verts: &[VertexData]) -> Geom {
    if p.flags & (16 | 64) != 0 {
        return Geom::NotModelled;
    }
    let world = p.flags & 8 != 0;
    let verts = verts
        .iter()
        .filter(|v| v.flags & 16 == 0 || mutant::active("FrameVertexKept"))
        .map(|v| {
            let mut at = v.p;
            if !world {
                at[2] = p.elevation;
            }
            Vert {
                p: at,
                bulge: v.bulge,
                w0: v.w0,
                w1: v.w1,
            }
        })
        .collect();
    Geom::Poly {
        verts,
        closed: p.flags & 1 != 0,
        const_w: None,
        world,
        n: p.n,
    }
}

/// An entity's own dimension-style overrides, from its `ACAD` extension data's `DSTYLE` group
/// (a `1070` code, then its value), as the DXF read takes them: arrow size (41), overall scale
/// (40), leader arrow block (341).
pub fn dim_overrides(eed: &[Eed], apps: &HashMap<u64, String>) -> DimOverrides {
    let mut o = DimOverrides::default();
    for x in eed {
        if !apps
            .get(&x.app)
            .is_some_and(|n| n.eq_ignore_ascii_case("ACAD"))
        {
            continue;
        }
        let mut in_style = false;
        let mut code: Option<i64> = None;
        for item in &x.items {
            let value = match item {
                XItem::Str(t) => {
                    in_style = t.eq_ignore_ascii_case("DSTYLE");
                    continue;
                }
                XItem::Short(c) if in_style && code.is_none() => {
                    code = Some(i64::from(*c));
                    continue;
                }
                XItem::Short(v) => (Some(f64::from(*v)), None),
                XItem::Real(_, v) => (Some(*v), None),
                XItem::Handle(1005, h) => (None, Some(*h)),
                _ => continue,
            };
            if !in_style {
                continue;
            }
            let code = if mutant::active("DstyleCodesSwapped") {
                code.take().map(|c| match c {
                    40 => 41,
                    41 => 40,
                    c => c,
                })
            } else {
                code.take()
            };
            match (code, value) {
                (Some(41), (Some(v), _)) => o.arrow_size = Some(v),
                (Some(40), (Some(v), _)) => o.scale = Some(v),
                (Some(341), (_, Some(h))) => o.leader_arrow = Some(h),
                _ => {}
            }
        }
    }
    o
}

/// An entity's raw geometry for the drawing-model read, its handles named by `names`, a
/// reference's attributes given: angles in degrees where the model wants them.
pub fn geom_of(e: &entities::Entity, names: &Names, attribs: Vec<AttribRaw>) -> Geom {
    let style = |h: Option<u64>| Names::of(&names.style, h, "Standard");
    match &e.body {
        Body::Line { p1, p2, .. } => Geom::Line { p1: *p1, p2: *p2 },
        Body::Circle { c, r, n, .. } => Geom::Circle {
            c: *c,
            r: *r,
            n: *n,
        },
        Body::Arc {
            c, r, n, a0, a1, ..
        } => Geom::Arc {
            c: *c,
            r: *r,
            a0: rad(*a0),
            a1: rad(*a1),
            n: *n,
        },
        Body::Point { p, .. } => Geom::Point { p: *p },
        Body::Ellipse {
            c,
            major,
            n,
            ratio,
            p0,
            p1,
        } => Geom::Ellipse {
            c: *c,
            major: *major,
            ratio: *ratio,
            p0: *p0,
            p1: *p1,
            n: *n,
        },
        Body::Solid { pts, n, .. } => Geom::Solid { pts: *pts, n: *n },
        Body::Face { pts, hidden } => Geom::Face {
            pts: *pts,
            hidden: i64::from(*hidden),
        },
        Body::LwPolyline(l) => {
            let verts = l
                .points
                .iter()
                .enumerate()
                .map(|(k, p)| {
                    let w = l.widths.get(k).copied().unwrap_or([0.0; 2]);
                    Vert {
                        p: [p[0], p[1], l.elevation],
                        bulge: l.bulges.get(k).copied().unwrap_or(0.0),
                        w0: w[0],
                        w1: w[1],
                    }
                })
                .collect();
            Geom::Poly {
                verts,
                closed: l.closed(),
                const_w: (l.flags & 4 != 0)
                    .then_some(l.const_width)
                    .filter(|w| *w != 0.0),
                world: false,
                n: l.n,
            }
        }
        Body::Text(t) => Geom::Text(text_raw(t, style(t.style))),
        Body::AttDef(a) => Geom::AttDef {
            text: text_raw(&a.text, style(a.text.style)),
            constant: a.flags & 2 != 0,
        },
        Body::MText(m) => Geom::MText(MTextRaw {
            at: m.ins,
            height: m.height,
            width: m.rect_width,
            attach: i64::from(m.attach),
            dir: Some(m.xdir),
            rotation: 0.0,
            style: style(m.style),
            spacing: m.spacing,
            spacing_style: i64::from(m.spacing_style),
            background: m
                .background
                .map(|(f, s, idx, rgb)| (i64::from(f), Some(i64::from(idx)), rgb, s)),
            columns: m.columns.as_ref().map(|c| k2_mpc_cad_model::draw::Columns {
                kind: i64::from(c.kind),
                count: i64::from(c.count),
                width: Some(c.width),
                gutter: Some(c.gutter),
                auto_height: c.auto_height,
                reversed: c.reversed,
                heights: c.heights.clone(),
            }),
            content: m.text.clone(),
            n: m.n,
        }),
        Body::Insert(ins) => Geom::Insert {
            block: Names::of(&names.block, ins.block, ""),
            at: ins.ins,
            scale: ins.scale,
            rotation: rad(ins.rotation),
            array: ins
                .array
                .filter(|a| a.0 > 1 || a.1 > 1)
                .map(|a| (i64::from(a.0), i64::from(a.1), a.2, a.3)),
            attribs,
            n: ins.n,
        },
        Body::Hatch(h) => Geom::Hatch(Box::new(hatch_raw(h))),
        Body::Dimension(dm) => Geom::Dimension {
            block: Names::of(&names.block, dm.block, ""),
            kind: dm.dxf_type,
            measured: Some(dm.measured),
            text: dm.text.clone(),
            style: Names::of(&names.dimstyle, dm.style, "Standard"),
            defs: dm
                .defs
                .iter()
                .filter(|(c, _)| matches!(c, 10 | 13 | 14 | 15 | 16))
                .copied()
                .chain(std::iter::once((11, dm.text_mid)))
                .collect(),
            ins: dm
                .defs
                .iter()
                .find(|(c, q)| *c == 12 && (q[0] != 0.0 || q[1] != 0.0))
                .map(|(_, q)| *q),
        },
        Body::Viewport(_) => Geom::Viewport,
        Body::Spline(sp) => Geom::Spline(SplineRaw {
            degree: i64::from(sp.degree),
            flags: i64::from(sp.flags),
            knots: sp.knots.clone(),
            ctrl: sp.ctrl.clone(),
            weights: sp.weights.clone(),
            fit: sp.fit.clone(),
        }),
        Body::XLine { point, dir, ray } => Geom::XLine {
            point: *point,
            dir: *dir,
            ray: *ray,
        },
        Body::Mleader(m) => Geom::Mleader(Box::new(m.content.clone())),
        Body::Image { data, wipeout } => Geom::Image {
            wipeout: *wipeout,
            ins: data.ins,
            u: data.u,
            v: data.v,
            size: data.size,
            clip: data.clip.clone(),
            clip_type: i64::from(data.clip_type),
            def: data.def,
            path: None,
        },
        // an old polyline without its vertices (the drawing read gives them, `poly_geom`)
        Body::Polyline(p) => poly_geom(p, &[]),
        Body::Leader(l) => Geom::Leader {
            verts: l.points.clone(),
            spline: l.path_type == 1,
            arrow: l.arrow,
            style: Names::of(&names.dimstyle, l.dimstyle, "Standard"),
            annotation: l.annotation,
            overrides: dim_overrides(&e.eed, &names.app),
        },
        Body::Attrib(_)
        | Body::Block { .. }
        | Body::EndBlk
        | Body::Seqend
        | Body::Vertex(_)
        | Body::Other => Geom::NotModelled,
    }
}

/// The block record a block record's extension data names as the dynamic block it draws: the
/// first entity handle (1005) of its `AcDbBlockRepBTag` application, by `apps` (registered
/// applications by handle).
pub fn dynamic_target(eed: &[Eed], apps: &HashMap<u64, String>) -> Option<u64> {
    eed.iter()
        .filter(|x| {
            apps.get(&x.app)
                .is_some_and(|n| n.eq_ignore_ascii_case("AcDbBlockRepBTag"))
                && !mutant::active("BlockRepTagIgnored")
        })
        .flat_map(|x| x.items.iter())
        .find_map(|i| match i {
            XItem::Handle(1005, h) => Some(*h),
            _ => None,
        })
}

/// The palette's colour number nearest `rgb` (`0xRRGGBB`), the lowest number on a tie.
pub fn nearest_aci(rgb: u32) -> i64 {
    let want = [(rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF].map(|x| x as i64);
    (1..=255)
        .filter_map(|n| k2_mpc_cad_model::draw::aci::rgb(n).map(|c| (n, c)))
        .min_by_key(|(n, c)| {
            let d: i64 = (0..3).map(|k| (i64::from(c[k]) - want[k]).pow(2)).sum();
            (d, *n)
        })
        .map_or(7, |(n, _)| n)
}

/// Each name made unique: the first of a repeated name keeps it; the others are numbered after
/// it, paper spaces from 0 (`*Paper_Space0`), the rest from 1 (`*D1`), skipping names in use.
fn unique_names<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let names: Vec<&str> = names.collect();
    let mut count: HashMap<String, usize> = HashMap::new();
    for n in &names {
        *count.entry(n.to_ascii_uppercase()).or_default() += 1;
    }
    let mut used: std::collections::HashSet<String> =
        names.iter().map(|n| n.to_ascii_uppercase()).collect();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut next: HashMap<String, usize> = HashMap::new();
    names
        .iter()
        .map(|n| {
            let up = n.to_ascii_uppercase();
            let k = seen.entry(up.clone()).or_default();
            *k += 1;
            if count[&up] < 2 || *k == 1 || mutant::active("RepeatedNamesKept") {
                return (*n).to_owned();
            }
            let start = usize::from(!up.starts_with("*PAPER_SPACE"));
            let i = next.entry(up.clone()).or_insert(start);
            loop {
                let cand = format!("{n}{i}");
                *i += 1;
                if used.insert(cand.to_ascii_uppercase()) {
                    return cand;
                }
            }
        })
        .collect()
}

/// For the rows: each block record name made unique, as the drawing names them.
#[doc(hidden)]
pub fn unique_block_names(names: &[&str]) -> Vec<String> {
    unique_names(names.iter().copied())
}

fn text_raw(t: &TextData, style: String) -> TextRaw {
    TextRaw {
        p10: Some([t.ins[0], t.ins[1], t.elevation]),
        p11: t.align.map(|a| [a[0], a[1], t.elevation]),
        height: t.height,
        rotation: rad(t.rotation),
        width: Some(t.width),
        oblique: Some(rad(t.oblique)),
        style,
        halign: i64::from(t.halign),
        valign: i64::from(t.valign),
        generation: i64::from(t.generation),
        text: t.text.clone(),
        n: t.n,
    }
}

fn attrib_raw(h: u64, a: &AttData, layer: String, style: String) -> AttribRaw {
    AttribRaw {
        handle: Some(h),
        tag: a.tag.clone(),
        flags: i64::from(a.flags),
        text: text_raw(&a.text, style),
        layer,
    }
}

fn hatch_raw(h: &entities::HatchData) -> HatchRaw {
    let loops = h
        .paths
        .iter()
        .map(|p| LoopRaw {
            flags: i64::from(p.flags),
            edges: p
                .edges
                .iter()
                .map(|e| match e {
                    HatchEdge::Line(a, b) => Edge::Line(*a, *b),
                    HatchEdge::Arc(c, r, a0, a1, ccw) => Edge::Arc {
                        c: *c,
                        r: *r,
                        a0: rad(*a0),
                        a1: rad(*a1),
                        ccw: *ccw,
                    },
                    HatchEdge::Ellipse(c, m, ratio, a0, a1, ccw) => Edge::Ellipse {
                        c: *c,
                        major: *m,
                        ratio: *ratio,
                        a0: rad(*a0),
                        a1: rad(*a1),
                        ccw: *ccw,
                    },
                    HatchEdge::Spline {
                        degree,
                        knots,
                        ctrl,
                        weights,
                        fit,
                    } => Edge::Spline(SplineRaw {
                        degree: i64::from(*degree),
                        flags: if weights.is_empty() { 0 } else { 4 },
                        knots: knots.clone(),
                        ctrl: ctrl.iter().map(|q| [q[0], q[1], 0.0]).collect(),
                        weights: weights.clone(),
                        fit: fit.iter().map(|q| [q[0], q[1], 0.0]).collect(),
                    }),
                })
                .collect(),
            poly: p.poly.clone(),
        })
        .collect();
    HatchRaw {
        solid: h.solid,
        pattern: h.name.clone(),
        angle: rad(h.angle),
        scale: h.scale,
        double: h.double,
        style: i64::from(h.style),
        defs: h
            .lines
            .iter()
            .map(|l| PatLine {
                angle: rad(l.angle),
                base: l.base,
                offset: l.offset,
                dashes: l.dashes.clone(),
            })
            .collect(),
        loops,
        gradient: h
            .gradient
            .as_ref()
            .map(|(name, angle, shift, one, colours)| Gradient {
                name: name.clone(),
                angle: rad(*angle),
                shift: *shift,
                one_colour: *one,
                colours: colours
                    .iter()
                    .map(|c| (None, Some(c & 0x00FF_FFFF)))
                    .collect(),
            }),
        assoc: h.sources.clone(),
        elevation: h.elevation,
        n: h.n,
    }
}

/// A kind's extrusion.
fn normal(b: &Body) -> [f64; 3] {
    match b {
        Body::Line { n, .. }
        | Body::Circle { n, .. }
        | Body::Arc { n, .. }
        | Body::Point { n, .. }
        | Body::Ellipse { n, .. }
        | Body::Solid { n, .. } => *n,
        Body::LwPolyline(l) => l.n,
        Body::Text(t) => t.n,
        Body::Attrib(a) | Body::AttDef(a) => a.text.n,
        Body::MText(m) => m.n,
        Body::Insert(i) => i.n,
        Body::Hatch(h) => h.n,
        Body::Dimension(d) => d.n,
        Body::Leader(l) => l.n,
        Body::Polyline(p) => p.n,
        _ => [0.0, 0.0, 1.0],
    }
}

/// Why the edit verbs can't change an entity, as the DXF reader says it.
fn edit_reason(kind: &str, up: bool, reactors: bool, block: Option<&str>) -> &'static str {
    let base = match kind {
        "LINE" | "POINT" | "CIRCLE" | "ARC" | "LWPOLYLINE" | "POLYLINE" | "TEXT" | "MTEXT"
        | "INSERT" | "ELLIPSE" | "SPLINE" | "SOLID" | "TRACE" | "3DFACE" | "ATTDEF" => "1",
        "HATCH" => "hatch",
        "DIMENSION" | "ARC_DIMENSION" => "dimension",
        "LEADER" | "MLEADER" | "MULTILEADER" => "leader",
        "VIEWPORT" => "viewport",
        "ACAD_PROXY_ENTITY" => "proxy",
        "3DSOLID" | "REGION" | "BODY" | "SURFACE" => "acis",
        "IMAGE" | "WIPEOUT" | "UNDERLAY" | "PDFUNDERLAY" | "DWFUNDERLAY" | "DGNUNDERLAY" => "image",
        _ => "unknown",
    };
    if base != "1" {
        return base;
    }
    if reactors {
        "assoc"
    } else if !up && kind != "LINE" && kind != "POINT" && kind != "3DFACE" {
        "ocs"
    } else if kind == "INSERT" && block.is_some_and(|b| b.starts_with("*U") || b.starts_with("*u"))
    {
        "dynblock"
    } else {
        "1"
    }
}

/// An entity's box in world x and y, and its quality, by the rules the DXF reader uses.
/// An old polyline's box as the DXF read computes it: every vertex (a face record's at the
/// origin, as written), bulged segments in a 2D polyline's frame, taken to world x and y.
fn poly_bbox(
    p: &PolylineData,
    verts: &[VertexData],
    n: [f64; 3],
    up: bool,
) -> (Option<[f64; 4]>, BboxQ) {
    let closed = p.flags & 1 != 0;
    let three_d = p.flags & (8 | 16 | 64) != 0;
    let mut local = Acc::new();
    for (k, v) in verts.iter().enumerate() {
        let q0 = [v.p[0], v.p[1]];
        let next = match verts.get(k + 1) {
            Some(n1) => Some([n1.p[0], n1.p[1]]),
            None if closed && verts.len() > 1 => Some([verts[0].p[0], verts[0].p[1]]),
            None => None,
        };
        match next {
            Some(q1) if !three_d => geom::bulge_seg(&mut local, q0, q1, v.bulge),
            _ => local.add(q0[0], q0[1]),
        }
    }
    let Some(b) = local.get() else {
        return (None, BboxQ::None);
    };
    let mut acc = Acc::new();
    if up || three_d {
        acc.add_box(b);
        return (acc.get(), BboxQ::Exact);
    }
    for q in [[b[0], b[1]], [b[2], b[1]], [b[0], b[3]], [b[2], b[3]]] {
        if let Some(w) = geom::to_world([q[0], q[1], p.elevation], n) {
            acc.add(w[0], w[1]);
        }
    }
    (acc.get(), BboxQ::Approx)
}

fn bbox(b: &Body, n: [f64; 3], up: bool) -> (Option<[f64; 4]>, BboxQ) {
    let world = |q: [f64; 2], z: f64| -> Option<[f64; 2]> {
        if up {
            Some(q)
        } else {
            geom::to_world([q[0], q[1], z], n).map(|w| [w[0], w[1]])
        }
    };
    let mut acc = Acc::new();
    let q = match b {
        Body::Line { p1, p2, .. } => {
            acc.add(p1[0], p1[1]);
            acc.add(p2[0], p2[1]);
            BboxQ::Exact
        }
        Body::Point { p, .. } => {
            acc.add(p[0], p[1]);
            BboxQ::Exact
        }
        Body::Face { pts, .. } => {
            for p in pts {
                acc.add(p[0], p[1]);
            }
            BboxQ::Exact
        }
        Body::Solid { pts, .. } => {
            for p in pts {
                if let Some(w) = world([p[0], p[1]], p[2]) {
                    acc.add(w[0], w[1]);
                }
            }
            BboxQ::Exact
        }
        Body::Circle { c, r, .. } => {
            if let Some(x) = geom::circle(*c, *r, n) {
                acc.add_box(x);
            }
            BboxQ::Exact
        }
        Body::Arc { c, r, a0, a1, .. } => {
            if up {
                if let Some(x) = geom::arc_deg([c[0], c[1]], *r, rad(*a0), rad(*a1)) {
                    acc.add_box(x);
                }
                BboxQ::Exact
            } else {
                if let Some(x) = geom::circle(*c, *r, n) {
                    acc.add_box(x);
                }
                BboxQ::Approx
            }
        }
        Body::Ellipse {
            c,
            major,
            ratio,
            p0,
            p1,
            ..
        } => {
            if let Some(x) = geom::ellipse([c[0], c[1]], [major[0], major[1]], *ratio) {
                acc.add_box(x);
            }
            let full = p0.abs() < 1e-12 && (p1 - std::f64::consts::TAU).abs() < 1e-9;
            if full {
                BboxQ::Exact
            } else {
                BboxQ::Approx
            }
        }
        Body::LwPolyline(l) => {
            let v = &l.points;
            let closed = l.closed();
            let mut local = Acc::new();
            for (k, q0) in v.iter().enumerate() {
                let bulge = l.bulges.get(k).copied().unwrap_or(0.0);
                let next = match v.get(k + 1) {
                    Some(n1) => Some(*n1),
                    None if closed && v.len() > 1 => Some(v[0]),
                    None => None,
                };
                match next {
                    Some(q1) => geom::bulge_seg(&mut local, *q0, q1, bulge),
                    None => local.add(q0[0], q0[1]),
                }
            }
            let Some(x) = local.get() else {
                return (None, BboxQ::None);
            };
            if up {
                acc.add_box(x);
                BboxQ::Exact
            } else {
                for q in [[x[0], x[1]], [x[2], x[1]], [x[0], x[3]], [x[2], x[3]]] {
                    if let Some(w) = world(q, l.elevation) {
                        acc.add(w[0], w[1]);
                    }
                }
                BboxQ::Approx
            }
        }
        Body::Text(_) | Body::AttDef(_) | Body::MText(_) => {
            let (at, h, t, z, width, align, mtext) = match b {
                Body::Text(t) | Body::AttDef(AttData { text: t, .. }) => (
                    t.ins,
                    t.height,
                    t.text.as_str(),
                    t.elevation,
                    None,
                    t.align,
                    false,
                ),
                Body::MText(m) => (
                    [m.ins[0], m.ins[1]],
                    m.height,
                    m.text.as_str(),
                    m.ins[2],
                    Some(m.rect_width).filter(|w| *w > 0.0),
                    None,
                    true,
                ),
                _ => unreachable!(),
            };
            let h = h.abs();
            let lines = t.split("\\P").count().max(1) as f64;
            let longest = t.split("\\P").map(|l| l.chars().count()).max().unwrap_or(0) as f64;
            let width = width.unwrap_or(0.6 * h * longest);
            let (lo, hi) = if mtext {
                ([at[0], at[1] - h * lines * 1.5], [at[0] + width, at[1] + h])
            } else {
                ([at[0], at[1]], [at[0] + width, at[1] + h])
            };
            for q in [lo, hi] {
                if let Some(w) = world(q, z) {
                    acc.add(w[0], w[1]);
                }
            }
            if let Some(w) = align.and_then(|q| world(q, z)) {
                acc.add(w[0], w[1]);
            }
            BboxQ::Approx
        }
        Body::Insert(i) => {
            if let Some(w) = world([i.ins[0], i.ins[1]], i.ins[2]) {
                acc.add(w[0], w[1]);
            }
            BboxQ::InsertOnly
        }
        // a spline: its control and fit points (the DXF read's rule)
        Body::Spline(sp) => {
            for q in sp.ctrl.iter().chain(&sp.fit) {
                acc.add(q[0], q[1]);
            }
            BboxQ::Approx
        }
        _ => BboxQ::None,
    };
    if mutant::active("DwgBoxDropped") {
        return (None, BboxQ::None);
    }
    match acc.get() {
        Some(x) if q != BboxQ::None => (Some(x), q),
        _ => (None, BboxQ::None),
    }
}

impl DrawSource for DwgDrawing {
    fn doc(&self) -> &Doc {
        &self.doc
    }

    fn tables(&self) -> &DrawTables {
        &self.tables
    }

    fn entity(&self, i: usize) -> Result<Ent, CadError> {
        self.geometry(i)
    }

    fn attdefs(&self, b: usize) -> Option<Vec<AttDefInfo>> {
        self.attdefs.get(b).cloned()
    }

    fn block_marks(&self, b: usize) -> (Option<bool>, Option<bool>) {
        let doc = &self.doc;
        let dynamic = doc.blocks[b].dynamic_of.is_some()
            || doc.blocks.iter().any(|x| x.dynamic_of == Some(b));
        (Some(dynamic), self.annotative.get(b).copied())
    }

    fn current_layer(&self) -> Option<usize> {
        let h = self.clayer?;
        self.doc.layers.iter().position(|l| l.handle == Some(h))
    }
}

/// Whether extension data `eed` carries the annotative mark: its `AcadAnnotative`
/// application's second short not 0 (a mark without it counts), as the DXF read judges it.
pub fn annotative_eed(eed: &[Eed], apps: &HashMap<u64, String>) -> bool {
    if mutant::active("AnnotativeEedIgnored") {
        return false;
    }
    eed.iter()
        .find(|x| {
            apps.get(&x.app)
                .is_some_and(|n| n.eq_ignore_ascii_case("AcadAnnotative"))
        })
        .is_some_and(|x| {
            let shorts: Vec<i16> = x
                .items
                .iter()
                .filter_map(|i| match i {
                    XItem::Short(v) => Some(*v),
                    _ => None,
                })
                .collect();
            shorts.get(1).is_none_or(|v| *v != 0)
        })
}

/// An attribute definition's tag, default value, constant flag and prompt, as a header states
/// it; `None` for any other entity.
pub fn attdef_info(e: &entities::Entity) -> Option<AttDefInfo> {
    let Body::AttDef(a) = &e.body else {
        return None;
    };
    if mutant::active("AttdefsUnread") {
        return None;
    }
    Some(AttDefInfo {
        tag: a.tag.trim().to_owned(),
        default: a.text.text.clone(),
        constant: a.flags & 2 != 0,
        prompt: a.prompt.clone().unwrap_or_default(),
    })
}

/// Runs `draw header` or `draw page` (the words after the verb in `args`) on a DWG file's
/// bytes, as the DXF reader runs them on a DXF file's.
pub fn draw(
    bytes: &[u8],
    verb: &str,
    args: &[&str],
) -> Result<k2_mpc_cad_model::reads::ReadOut, CadError> {
    use k2_mpc_cad_model::draw::{header, page, NoXrefs};
    let open = || DwgDrawing::open(bytes, &Limits::default());
    match verb {
        "draw header" => header(&open()?, args, &NoXrefs),
        "draw page" => page(&open()?, args),
        _ => Err(CadError::new(
            k2_mpc_cad_model::error::USAGE,
            format!("no DWG read verb {verb:?}"),
            "the DWG read verbs: draw header, draw page",
        )),
    }
}
