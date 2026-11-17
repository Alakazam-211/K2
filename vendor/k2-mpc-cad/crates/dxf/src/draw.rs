//! The drawing-model read's source for DXF ([`DxfDrawing`]): the model the other reads use, the
//! per-record tables (settings, layer colours and transparency, linetypes, text styles, layouts
//! and their viewports, draw-order tables, external references), and one entity's raw geometry,
//! read from its own pairs when a page asks for it. Nothing about an entity's geometry is kept
//! beyond the page being built.

use std::borrow::Cow;

use k2_mpc_cad_model::draw::{
    AttribRaw, Columns, DimOverrides, DimStyle, DrawSource, DrawTables, Edge, Ent, Geom, Gradient,
    HatchRaw, LayerDraw, LayoutInfo, Linetype, LoopRaw, LtElement, MTextRaw, Paper, PatLine,
    Settings, SortTable, SplineRaw, TextRaw, TextStyle, Vert, Viewport, XrefPath,
};
use k2_mpc_cad_model::{BlockKind, Doc};

use crate::error::CadError;
use crate::structure::{EntityUnit, Structure, Unit};
use crate::token::{decode, unescape};
use crate::{convert, handle, structure, token, Limits};

/// A drawing opened for the drawing-model read: its bytes' tokens and structure, its model and
/// its tables. A host keeps one per version while a viewer pages through it, or opens one per
/// call.
pub struct DxfDrawing<'a> {
    s: Structure<'a>,
    doc: Doc,
    tables: DrawTables,
}

impl<'a> DxfDrawing<'a> {
    /// Reads `bytes`.
    pub fn open(bytes: &'a [u8], limits: &Limits) -> Result<Self, CadError> {
        let t = token::tokenize(bytes, limits)?;
        let s = structure::walk(t, limits)?;
        let doc = convert::doc(&s);
        let tables = tables(&s, &doc);
        Ok(DxfDrawing { s, doc, tables })
    }
}

impl DrawSource for DxfDrawing<'_> {
    fn doc(&self) -> &Doc {
        &self.doc
    }

    fn tables(&self) -> &DrawTables {
        &self.tables
    }

    fn entity(&self, i: usize) -> Result<Ent, CadError> {
        Ok(entity(&self.s, &self.s.entities[i]))
    }
}

/// A unit's pairs as `(code, text)`, extension data and `{…}` groups left out. Text is
/// untrimmed for the codes that hold a text's own words (1, 3), trimmed for the rest.
fn pairs<'a>(s: &Structure<'a>, u: &Unit) -> Vec<(i32, Cow<'a, str>)> {
    let mut out = Vec::new();
    let mut in102 = false;
    for i in u.first + 1..u.end {
        let c = s.tok.pairs[i].code;
        if c >= 1000 {
            break;
        }
        if c == 102 {
            in102 = s.tok.value(i).trim_ascii().starts_with(b"{");
            continue;
        }
        if in102 || c == 999 {
            continue;
        }
        let t = if c == 1 || c == 3 || c == 304 {
            decode(s.tok.value(i))
        } else {
            s.text(i)
        };
        out.push((c, t));
    }
    out
}

/// A unit's extension data, by application: `(application, [(code, text)])`.
type XData<'a> = Vec<(String, Vec<(i32, Cow<'a, str>)>)>;

/// A unit's extension data.
fn xdata<'a>(s: &Structure<'a>, u: &Unit) -> XData<'a> {
    let mut out: XData<'a> = Vec::new();
    for i in u.first + 1..u.end {
        let c = s.tok.pairs[i].code;
        if c == 1001 {
            out.push((s.text(i).into_owned(), Vec::new()));
        } else if c >= 1000 {
            if let Some(last) = out.last_mut() {
                last.1.push((c, s.text(i)));
            }
        }
    }
    out
}

fn num(v: &str) -> Option<f64> {
    v.trim().parse::<f64>().ok().filter(|x| x.is_finite())
}

fn int(v: &str) -> Option<i64> {
    let t = v.trim();
    t.parse::<i64>()
        .ok()
        .or_else(|| num(t).filter(|x| x.fract() == 0.0).map(|x| x as i64))
}

/// Typed lookups over a unit's pairs.
struct P<'p, 'a>(&'p [(i32, Cow<'a, str>)]);

impl P<'_, '_> {
    fn s(&self, c: i32) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| *k == c)
            .map(|(_, v)| v.as_ref())
    }
    fn f(&self, c: i32) -> Option<f64> {
        self.s(c).and_then(num)
    }
    fn i(&self, c: i32) -> Option<i64> {
        self.s(c).and_then(int)
    }
    fn pt(&self, c: i32) -> Option<[f64; 3]> {
        Some([self.f(c)?, self.f(c + 10)?, self.f(c + 20).unwrap_or(0.0)])
    }
    fn h(&self, c: i32) -> Option<u64> {
        self.s(c).and_then(|v| handle::parse(v.as_bytes()))
    }
    fn n(&self) -> [f64; 3] {
        self.pt(210).unwrap_or([0.0, 0.0, 1.0])
    }
    /// Every point at `c` in order (`c`, `c + 10`, `c + 20` each).
    fn pts(&self, c: i32) -> Vec<[f64; 3]> {
        let mut out: Vec<[f64; 3]> = Vec::new();
        let mut open = false;
        for (k, v) in self.0 {
            let x = num(v);
            if *k == c {
                out.push([x.unwrap_or(0.0), 0.0, 0.0]);
                open = true;
            } else if open && *k == c + 10 {
                if let Some(l) = out.last_mut() {
                    l[1] = x.unwrap_or(0.0);
                }
            } else if open && *k == c + 20 {
                if let Some(l) = out.last_mut() {
                    l[2] = x.unwrap_or(0.0);
                }
            }
        }
        out
    }
    /// Every value at `c`, as numbers, in order.
    fn all(&self, c: i32) -> Vec<f64> {
        self.0
            .iter()
            .filter(|(k, _)| *k == c)
            .filter_map(|(_, v)| num(v))
            .collect()
    }
}

/// An attribute's own codes: a 2018 multi-line attribute's embedded multiline text (after its
/// `101` marker) has codes of its own meaning (its `41` and `71` aren't the attribute's width
/// factor and generation flags).
fn attrib_main<'p, 'a>(p: &P<'p, 'a>) -> P<'p, 'a> {
    let cut = if k2_mpc_cad_model::mutant::active("AttribEmbeddedCodesAsMain") {
        p.0.len()
    } else {
        p.0.iter().position(|(c, _)| *c == 101).unwrap_or(p.0.len())
    };
    P(&p.0[..cut])
}

fn text_raw(p: &P<'_, '_>, attrib: bool) -> TextRaw {
    let main;
    let p = if attrib {
        main = attrib_main(p);
        &main
    } else {
        p
    };
    // an attribute's text codes: its text subclass's (a 2018 multi-line attribute's own
    // subclass writes a `71` and a `72` of other meaning: its type, not generation flags or
    // alignment)
    let text_end = if attrib && !k2_mpc_cad_model::mutant::active("AttribTypeAsGeneration") {
        p.0.iter()
            .position(|(c, v)| {
                *c == 100 && matches!(v.trim(), "AcDbAttribute" | "AcDbAttributeDefinition")
            })
            .unwrap_or(p.0.len())
    } else {
        p.0.len()
    };
    let tp = P(&p.0[..text_end]);
    TextRaw {
        p10: p.pt(10),
        p11: p.pt(11),
        height: p.f(40).unwrap_or(1.0),
        rotation: p.f(50).unwrap_or(0.0),
        width: p.f(41),
        oblique: p.f(51),
        style: p.s(7).unwrap_or("Standard").to_owned(),
        halign: tp.i(72).unwrap_or(0),
        valign: p.i(if attrib { 74 } else { 73 }).unwrap_or(0),
        generation: tp.i(71).unwrap_or(0),
        text: unescape(p.s(1).unwrap_or("")).into_owned(),
        n: p.n(),
    }
}

/// Entity `eu`'s raw geometry.
pub fn entity(s: &Structure<'_>, eu: &EntityUnit) -> Ent {
    let v = pairs(s, &eu.head);
    let p = P(&v);
    let kind = eu.head.kind.as_ref();
    let geom = geometry(s, eu, kind, &p).unwrap_or(Geom::Unreadable);
    Ent {
        geom,
        lt_scale: p.f(48).unwrap_or(1.0),
        invisible: p.i(60) == Some(1),
        book: p.s(430).map(str::to_owned),
    }
}

fn geometry(s: &Structure<'_>, eu: &EntityUnit, kind: &str, p: &P<'_, '_>) -> Option<Geom> {
    let n = p.n();
    Some(match kind {
        "LINE" => Geom::Line {
            p1: p.pt(10)?,
            p2: p.pt(11)?,
        },
        "XLINE" | "RAY" => Geom::XLine {
            point: p.pt(10)?,
            dir: p.pt(11)?,
            ray: kind == "RAY",
        },
        "POINT" => Geom::Point { p: p.pt(10)? },
        "CIRCLE" => Geom::Circle {
            c: p.pt(10)?,
            r: p.f(40)?,
            n,
        },
        "ARC" => Geom::Arc {
            c: p.pt(10)?,
            r: p.f(40)?,
            a0: p.f(50).unwrap_or(0.0),
            a1: p.f(51).unwrap_or(360.0),
            n,
        },
        "ELLIPSE" => Geom::Ellipse {
            c: p.pt(10)?,
            major: p.pt(11)?,
            ratio: p.f(40).unwrap_or(1.0),
            p0: p.f(41).unwrap_or(0.0),
            p1: p.f(42).unwrap_or(std::f64::consts::TAU),
            n,
        },
        "LWPOLYLINE" => {
            let elev = p.f(38).unwrap_or(0.0);
            let mut verts: Vec<Vert> = Vec::new();
            for (k, v) in p.0 {
                let x = num(v).unwrap_or(0.0);
                match k {
                    10 => verts.push(Vert {
                        p: [x, 0.0, elev],
                        bulge: 0.0,
                        w0: 0.0,
                        w1: 0.0,
                    }),
                    20 => {
                        if let Some(l) = verts.last_mut() {
                            l.p[1] = x;
                        }
                    }
                    40 => {
                        if let Some(l) = verts.last_mut() {
                            l.w0 = x;
                        }
                    }
                    41 => {
                        if let Some(l) = verts.last_mut() {
                            l.w1 = x;
                        }
                    }
                    42 => {
                        if let Some(l) = verts.last_mut() {
                            l.bulge = x;
                        }
                    }
                    _ => {}
                }
            }
            Geom::Poly {
                verts,
                closed: p.i(70).unwrap_or(0) & 1 != 0,
                const_w: p.f(43).filter(|w| *w != 0.0),
                world: false,
                n,
            }
        }
        "POLYLINE" => {
            let flags = p.i(70).unwrap_or(0);
            if flags & (16 | 64) != 0 {
                return Some(Geom::NotModelled);
            }
            let world = flags & 8 != 0;
            let elev = p.pt(10).map_or(0.0, |q| q[2]);
            let (dw0, dw1) = (p.f(40).unwrap_or(0.0), p.f(41).unwrap_or(0.0));
            let mut verts = Vec::new();
            for f in &eu.followers {
                if f.kind != "VERTEX" {
                    continue;
                }
                let fv = pairs(s, f);
                let q = P(&fv);
                let vf = q.i(70).unwrap_or(0);
                if vf & 16 != 0 && !k2_mpc_cad_model::mutant::active("FrameVertexDrawn") {
                    continue;
                }
                let mut at = q.pt(10).unwrap_or([0.0; 3]);
                if !world {
                    at[2] = elev;
                }
                verts.push(Vert {
                    p: at,
                    bulge: q.f(42).unwrap_or(0.0),
                    w0: q.f(40).unwrap_or(dw0),
                    w1: q.f(41).unwrap_or(dw1),
                });
            }
            Geom::Poly {
                verts,
                closed: flags & 1 != 0,
                const_w: None,
                world,
                n,
            }
        }
        "SPLINE" => Geom::Spline(SplineRaw {
            degree: p.i(71).unwrap_or(3),
            flags: p.i(70).unwrap_or(0),
            knots: p.all(40),
            ctrl: p.pts(10),
            weights: p.all(41),
            fit: p.pts(11),
        }),
        "TEXT" => Geom::Text(text_raw(p, false)),
        "ATTDEF" => Geom::AttDef {
            text: text_raw(p, true),
            constant: p.i(70).unwrap_or(0) & 2 != 0,
        },
        "MTEXT" => {
            // a 2018 embedded object follows its `101` marker: its codes have their own meaning
            // there (its `11` is its insertion point, not the text's direction)
            let cut = if k2_mpc_cad_model::mutant::active("EmbeddedCodesAsMain") {
                p.0.len()
            } else {
                p.0.iter().position(|(c, _)| *c == 101).unwrap_or(p.0.len())
            };
            let emb = P(&p.0[cut.min(p.0.len())..]);
            let p = &P(&p.0[..cut]);
            let mut content = String::new();
            for (k, v) in p.0 {
                if *k == 3 || *k == 1 {
                    content.push_str(v);
                }
            }
            Geom::MText(MTextRaw {
                at: p.pt(10)?,
                height: p.f(40).unwrap_or(1.0),
                width: p.f(41).unwrap_or(0.0),
                attach: p.i(71).unwrap_or(1),
                dir: p.pt(11),
                rotation: p.f(50).unwrap_or(0.0),
                style: p.s(7).unwrap_or("Standard").to_owned(),
                spacing: p.f(44).unwrap_or(1.0),
                spacing_style: p.i(73).unwrap_or(1),
                background: p.i(90).map(|f| {
                    (
                        f,
                        p.i(63),
                        p.i(421).and_then(|c| u32::try_from(c).ok()),
                        p.f(45).unwrap_or(1.5),
                    )
                }),
                columns: if emb.0.is_empty() {
                    p.i(75).map(|k| Columns {
                        kind: k,
                        count: p.i(76).unwrap_or(1),
                        width: p.f(48),
                        gutter: p.f(49),
                        auto_height: p.i(79) == Some(1),
                        reversed: p.i(78) == Some(1),
                        heights: p.all(50),
                    })
                } else {
                    emb.i(71).map(|k| Columns {
                        kind: k,
                        count: emb.i(72).unwrap_or(1),
                        width: emb.f(44),
                        gutter: emb.f(45),
                        auto_height: emb.i(73) == Some(1),
                        reversed: emb.i(74) == Some(1),
                        heights: emb.all(46),
                    })
                },
                content,
                n,
            })
        }
        "INSERT" => {
            let cols = p.i(70).unwrap_or(1);
            let rows = p.i(71).unwrap_or(1);
            let attribs = eu
                .followers
                .iter()
                .filter(|f| f.kind == "ATTRIB")
                .map(|f| {
                    let fv = pairs(s, f);
                    let q = P(&fv);
                    AttribRaw {
                        handle: q.h(5),
                        tag: q.s(2).unwrap_or("").to_owned(),
                        flags: q.i(70).unwrap_or(0),
                        text: text_raw(&q, true),
                        layer: q.s(8).unwrap_or("0").to_owned(),
                    }
                })
                .collect();
            Geom::Insert {
                block: p.s(2)?.to_owned(),
                at: p.pt(10).unwrap_or([0.0; 3]),
                scale: [
                    p.f(41).unwrap_or(1.0),
                    p.f(42).unwrap_or(1.0),
                    p.f(43).unwrap_or(1.0),
                ],
                rotation: p.f(50).unwrap_or(0.0),
                array: (cols > 1 || rows > 1)
                    .then(|| (cols, rows, p.f(44).unwrap_or(0.0), p.f(45).unwrap_or(0.0))),
                attribs,
                n,
            }
        }
        "SOLID" | "TRACE" => {
            let a = p.pt(10)?;
            let b = p.pt(11)?;
            let c = p.pt(12)?;
            let d = p.pt(13).unwrap_or(c);
            Geom::Solid {
                pts: [a, b, c, d],
                n,
            }
        }
        "3DFACE" => {
            let a = p.pt(10)?;
            let b = p.pt(11)?;
            let c = p.pt(12)?;
            let d = p.pt(13).unwrap_or(c);
            Geom::Face {
                pts: [a, b, c, d],
                hidden: p.i(70).unwrap_or(0),
            }
        }
        "HATCH" => Geom::Hatch(Box::new(hatch(p)?)),
        "DIMENSION" => Geom::Dimension {
            block: p.s(2)?.to_owned(),
            kind: p.i(70).unwrap_or(0),
            measured: p.f(42),
            text: p.s(1).map(|t| unescape(t).into_owned()).unwrap_or_default(),
            style: p.s(3).unwrap_or("Standard").to_owned(),
            defs: [10, 11, 13, 14, 15, 16]
                .iter()
                .filter_map(|&c| p.pt(c).map(|q| (c, q)))
                .collect(),
            ins: p.pt(12).filter(|q| q[0] != 0.0 || q[1] != 0.0),
        },
        "LEADER" => Geom::Leader {
            verts: p.pts(10),
            spline: p.i(72) == Some(1),
            arrow: p.i(71) != Some(0),
            style: p.s(3).unwrap_or("Standard").to_owned(),
            annotation: p.h(340),
            overrides: dim_overrides(s, &eu.head),
        },
        "IMAGE" | "WIPEOUT" => {
            let clip: Vec<[f64; 2]> = p.pts(14).iter().map(|q| [q[0], q[1]]).collect();
            let size = p.pt(13).map_or([1.0, 1.0], |q| [q[0], q[1]]);
            Geom::Image {
                wipeout: kind == "WIPEOUT",
                ins: p.pt(10)?,
                u: p.pt(11)?,
                v: p.pt(12)?,
                size,
                clip,
                clip_type: p.i(71).unwrap_or(1),
                def: p.h(340),
                path: None,
            }
        }
        "MULTILEADER" | "MLEADER" => Geom::Mleader(Box::new(crate::mleader::of_entity(s, eu)?)),
        "VIEWPORT" => Geom::Viewport,
        _ => Geom::NotModelled,
    })
}

/// An entity's own dimension-style overrides (its `ACAD` extension data's `DSTYLE` group: a
/// `1070` code, then its value): arrow size (41), overall scale (40), leader arrow block (341).
fn dim_overrides(s: &Structure<'_>, u: &Unit) -> DimOverrides {
    let mut o = DimOverrides::default();
    for (app, x) in xdata(s, u) {
        if !app.eq_ignore_ascii_case("ACAD") {
            continue;
        }
        let mut in_style = false;
        let mut code: Option<i64> = None;
        for (c, t) in &x {
            match (c, in_style) {
                (1000, _) => in_style = t.eq_ignore_ascii_case("DSTYLE"),
                (1070, true) if code.is_none() => code = int(t),
                (1040 | 1070 | 1005, true) => match code.take() {
                    Some(41) => o.arrow_size = num(t),
                    Some(40) => o.scale = num(t),
                    Some(341) => o.leader_arrow = handle::parse(t.as_bytes()),
                    _ => {}
                },
                _ => {}
            }
        }
    }
    o
}

/// A hatch's pairs, in order: the boundary loops (each edge by type), the pattern and its
/// definition lines, the gradient.
fn hatch(p: &P<'_, '_>) -> Option<HatchRaw> {
    let v = p.0;
    let mut h = HatchRaw {
        solid: p.i(70).unwrap_or(0) & 1 != 0,
        pattern: p.s(2).unwrap_or("").to_owned(),
        elevation: p.pt(10).map_or(0.0, |q| q[2]),
        n: p.n(),
        scale: 1.0,
        ..HatchRaw::default()
    };
    let mut k = 0usize;
    let get = |k: usize| v.get(k).map(|(c, t)| (*c, t.as_ref()));
    let f = |t: &str| num(t).unwrap_or(0.0);
    // skip to the loop count
    while let Some((c, _)) = get(k) {
        if c == 91 {
            break;
        }
        k += 1;
    }
    let nloops = get(k).and_then(|(_, t)| int(t)).unwrap_or(0).max(0) as usize;
    k += 1;
    for _ in 0..nloops.min(100_000) {
        // 92: the loop's flags
        while let Some((c, _)) = get(k) {
            if c == 92 {
                break;
            }
            k += 1;
        }
        let flags = get(k).and_then(|(_, t)| int(t)).unwrap_or(0);
        k += 1;
        let mut lp = LoopRaw {
            flags,
            edges: Vec::new(),
            poly: None,
        };
        if flags & 2 != 0 {
            // a polyline: 72 has-bulge, 73 closed, 93 count, then 10/20/(42)
            let (mut closed, mut count) = (true, 0usize);
            while let Some((c, t)) = get(k) {
                match c {
                    72 => {}
                    73 => closed = int(t) == Some(1),
                    93 => {
                        count = int(t).unwrap_or(0).max(0) as usize;
                        k += 1;
                        break;
                    }
                    _ => break,
                }
                k += 1;
            }
            let mut verts: Vec<([f64; 2], f64)> = Vec::with_capacity(count.min(1 << 16));
            while verts.len() < count {
                match get(k) {
                    Some((10, t)) => {
                        let x = f(t);
                        let y = get(k + 1).map_or(0.0, |(_, t)| f(t));
                        verts.push(([x, y], 0.0));
                        k += 2;
                    }
                    Some((42, t)) => {
                        if let Some(l) = verts.last_mut() {
                            l.1 = f(t);
                        }
                        k += 1;
                    }
                    _ => break,
                }
            }
            if let Some((42, t)) = get(k) {
                if let Some(l) = verts.last_mut() {
                    l.1 = f(t);
                }
                k += 1;
            }
            lp.poly = Some((verts, closed));
        } else {
            // 93 edge count, then each edge by its type (72)
            while let Some((c, _)) = get(k) {
                if c == 93 {
                    break;
                }
                k += 1;
            }
            let nedges = get(k).and_then(|(_, t)| int(t)).unwrap_or(0).max(0) as usize;
            k += 1;
            for _ in 0..nedges.min(1 << 20) {
                let Some((72, t)) = get(k) else { break };
                let ty = int(t).unwrap_or(0);
                k += 1;
                // the edge's own pairs: up to the next 72, 97 or loop marker; a spline edge's by
                // its counts (its fit points come after a 97 of their own)
                let start = k;
                if ty == 4 {
                    let mut nk = 0usize;
                    let mut nc = 0usize;
                    while let Some((c, t)) = get(k) {
                        match c {
                            94 | 73 | 74 => {}
                            95 => nk = int(t).unwrap_or(0).max(0) as usize,
                            96 => nc = int(t).unwrap_or(0).max(0) as usize,
                            _ => break,
                        }
                        k += 1;
                    }
                    let (mut seen_k, mut seen_c) = (0usize, 0usize);
                    while let Some((c, _)) = get(k) {
                        match c {
                            40 if seen_k < nk => seen_k += 1,
                            10 if seen_c < nc => seen_c += 1,
                            20 | 42 => {}
                            _ => break,
                        }
                        k += 1;
                    }
                    if let Some((97, t)) = get(k) {
                        let nf = int(t).unwrap_or(0).max(0) as usize;
                        if nf > 0 || matches!(get(k + 1), Some((11 | 12 | 13 | 97, _))) {
                            k += 1;
                            while let Some((c, _)) = get(k) {
                                if !matches!(c, 11 | 21 | 12 | 22 | 13 | 23) {
                                    break;
                                }
                                k += 1;
                            }
                        }
                    }
                } else {
                    while let Some((c, _)) = get(k) {
                        if matches!(c, 72 | 97 | 92 | 75) {
                            break;
                        }
                        k += 1;
                    }
                }
                let e = &v[start..k];
                let q = P(e);
                let p2 = |c: i32| [q.f(c).unwrap_or(0.0), q.f(c + 10).unwrap_or(0.0)];
                let edge = match ty {
                    1 => Edge::Line(p2(10), p2(11)),
                    2 => Edge::Arc {
                        c: p2(10),
                        r: q.f(40).unwrap_or(0.0),
                        a0: q.f(50).unwrap_or(0.0),
                        a1: q.f(51).unwrap_or(360.0),
                        ccw: q.i(73) != Some(0),
                    },
                    3 => Edge::Ellipse {
                        c: p2(10),
                        major: p2(11),
                        ratio: q.f(40).unwrap_or(1.0),
                        a0: q.f(50).unwrap_or(0.0),
                        a1: q.f(51).unwrap_or(360.0),
                        ccw: q.i(73) != Some(0),
                    },
                    4 => {
                        let ctrl: Vec<[f64; 3]> = q.pts(10);
                        Edge::Spline(SplineRaw {
                            degree: q.i(94).unwrap_or(3),
                            flags: ((q.i(73).unwrap_or(0) & 1) * 4)
                                | ((q.i(74).unwrap_or(0) & 1) * 2),
                            knots: q.all(40),
                            weights: q.all(42),
                            ctrl,
                            fit: q.pts(11),
                        })
                    }
                    _ => continue,
                };
                lp.edges.push(edge);
            }
        }
        // 97: source boundary objects
        if let Some((97, t)) = get(k) {
            let ns = int(t).unwrap_or(0).max(0) as usize;
            k += 1;
            for _ in 0..ns {
                if let Some((330, t)) = get(k) {
                    if let Some(hh) = handle::parse(t.as_bytes()) {
                        h.assoc.push(hh);
                    }
                    k += 1;
                }
            }
        }
        h.loops.push(lp);
    }
    // the rest: style, pattern, definition lines, gradient
    let mut defs_left = 0usize;
    while let Some((c, t)) = get(k) {
        match c {
            75 => h.style = int(t).unwrap_or(0),
            52 => h.angle = f(t),
            41 => h.scale = f(t),
            77 => h.double = int(t) == Some(1),
            78 => defs_left = int(t).unwrap_or(0).max(0) as usize,
            53 if defs_left > 0 => {
                defs_left -= 1;
                let mut d = PatLine {
                    angle: f(t),
                    base: [0.0; 2],
                    offset: [0.0; 2],
                    dashes: Vec::new(),
                };
                k += 1;
                while let Some((c2, t2)) = get(k) {
                    match c2 {
                        43 => d.base[0] = f(t2),
                        44 => d.base[1] = f(t2),
                        45 => d.offset[0] = f(t2),
                        46 => d.offset[1] = f(t2),
                        79 => {}
                        49 => d.dashes.push(f(t2)),
                        _ => break,
                    }
                    k += 1;
                }
                h.defs.push(d);
                continue;
            }
            450 if int(t) == Some(1) => {
                let mut colours = Vec::new();
                let (mut name, mut angle, mut shift, mut one) = (String::new(), 0.0, 0.0, false);
                k += 1;
                while let Some((c2, t2)) = get(k) {
                    match c2 {
                        452 => one = int(t2) == Some(1),
                        460 => angle = f(t2).to_degrees(),
                        461 => shift = f(t2),
                        470 => name = t2.to_owned(),
                        63 => colours.push((int(t2), None)),
                        421 => {
                            let c = int(t2).and_then(|c| u32::try_from(c).ok());
                            match colours.last_mut() {
                                Some(l) if l.1.is_none() => l.1 = c,
                                _ => colours.push((None, c)),
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                }
                h.gradient = Some(Gradient {
                    name,
                    angle,
                    shift,
                    one_colour: one,
                    colours,
                });
                continue;
            }
            _ => {}
        }
        k += 1;
    }
    Some(h)
}

fn first<'a>(s: &Structure<'a>, u: &Unit, c: i32) -> Option<Cow<'a, str>> {
    (u.first + 1..u.end)
        .find(|&i| s.tok.pairs[i].code == c)
        .map(|i| s.text(i))
}

/// The drawing's text styles: what a text's box and placement read from its style.
pub fn text_styles(s: &Structure<'_>) -> Vec<TextStyle> {
    s.table("STYLE")
        .map(|t| {
            t.entries
                .iter()
                .map(|u| {
                    let v = pairs(s, u);
                    let p = P(&v);
                    let flags = p.i(70).unwrap_or(0);
                    let gen = p.i(71).unwrap_or(0);
                    let xd = xdata(s, u);
                    let acad = xd.iter().find(|(a, _)| a.eq_ignore_ascii_case("ACAD"));
                    let family = acad
                        .and_then(|(_, x)| x.iter().find(|(c, _)| *c == 1000))
                        .map(|(_, t)| t.to_string())
                        .filter(|t| !t.is_empty());
                    let fflags = acad
                        .and_then(|(_, x)| x.iter().find(|(c, _)| *c == 1071))
                        .and_then(|(_, t)| int(t))
                        .unwrap_or(0);
                    TextStyle {
                        handle: p.h(5),
                        name: p.s(2).unwrap_or("").to_owned(),
                        file: p.s(3).unwrap_or("").to_owned(),
                        bigfont: p.s(4).filter(|b| !b.is_empty()).map(str::to_owned),
                        family,
                        fixed_height: p.f(40).unwrap_or(0.0),
                        width: p.f(41).unwrap_or(1.0),
                        oblique: p.f(50).unwrap_or(0.0).to_radians(),
                        bold: fflags & 0x0200_0000 != 0,
                        italic: fflags & 0x0100_0000 != 0,
                        backwards: gen & 2 != 0,
                        upside_down: gen & 4 != 0,
                        vertical: flags & 4 != 0,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The per-record tables of the drawing.
pub fn tables(s: &Structure<'_>, doc: &Doc) -> DrawTables {
    let var_f = |n: &str, c: i32| s.var_value(n, c).and_then(|v| num(&v));
    let var_i = |n: &str, c: i32| s.var_value(n, c).and_then(|v| int(&v));
    let settings = Settings {
        ltscale: var_f("$LTSCALE", 40).unwrap_or(1.0),
        psltscale: var_i("$PSLTSCALE", 70).unwrap_or(1) != 0,
        lwdefault: var_i("$LWDEFAULT", 370).unwrap_or(25),
        lwdisplay: var_i("$LWDISPLAY", 290).unwrap_or(0) != 0,
        fillmode: var_i("$FILLMODE", 70).unwrap_or(1) != 0,
        pdmode: var_i("$PDMODE", 70).unwrap_or(0),
        pdsize: var_f("$PDSIZE", 40).unwrap_or(0.0),
        mirrtext: var_i("$MIRRTEXT", 70).unwrap_or(0) != 0,
    };
    // layers: in the order the model reads them
    let layers = s
        .table("LAYER")
        .map(|t| {
            t.entries
                .iter()
                .map(|u| {
                    let v = pairs(s, u);
                    let p = P(&v);
                    let alpha = xdata(s, u)
                        .iter()
                        .find(|(a, _)| a.eq_ignore_ascii_case("AcCmTransparency"))
                        .and_then(|(_, x)| x.iter().find(|(c, _)| *c == 1071))
                        .and_then(|(_, t)| int(t));
                    LayerDraw {
                        rgb: p.i(420).and_then(|c| u32::try_from(c & 0xFF_FFFF).ok()),
                        alpha,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let linetypes = s
        .table("LTYPE")
        .map(|t| {
            t.entries
                .iter()
                .map(|u| {
                    let v = pairs(s, u);
                    let p = P(&v);
                    let mut dashes = Vec::new();
                    let mut complex: Vec<LtElement> = Vec::new();
                    for (c, t) in &v {
                        match c {
                            49 => dashes.push(num(t).unwrap_or(0.0)),
                            74 => {
                                let fl = int(t).unwrap_or(0);
                                if fl & 6 != 0 && !dashes.is_empty() {
                                    complex.push(LtElement {
                                        after: dashes.len() - 1,
                                        kind: if fl & 2 != 0 { "text" } else { "shape" },
                                        text: None,
                                        shape: None,
                                        style: None,
                                        scale: 1.0,
                                        rotation: 0.0,
                                        offset: [0.0; 2],
                                    });
                                }
                            }
                            75 | 340 | 46 | 50 | 44 | 45 | 9 => {
                                if let Some(e) = complex.last_mut() {
                                    match c {
                                        75 => e.shape = int(t),
                                        340 => e.style = handle::parse(t.as_bytes()),
                                        46 => e.scale = num(t).unwrap_or(1.0),
                                        50 => e.rotation = num(t).unwrap_or(0.0).to_radians(),
                                        44 => e.offset[0] = num(t).unwrap_or(0.0),
                                        45 => e.offset[1] = num(t).unwrap_or(0.0),
                                        _ => e.text = Some(t.to_string()),
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Linetype {
                        handle: p.h(5),
                        name: p.s(2).unwrap_or("").to_owned(),
                        length: p.f(40).unwrap_or(0.0),
                        dashes,
                        complex,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let text_styles = text_styles(s);
    let by_handle = |h: u64| doc.blocks.iter().position(|b| b.handle == Some(h));
    let mut layouts: Vec<LayoutInfo> = vec![LayoutInfo::default(); doc.blocks.len()];
    let mut sortents: Vec<Option<SortTable>> = vec![None; doc.blocks.len()];
    let mut image_defs: Vec<(u64, String)> = Vec::new();
    for u in &s.objects {
        match u.kind.as_ref() {
            "LAYOUT" => {
                let v = pairs(s, u);
                let p = P(&v);
                // the block record: the last owner pointer, in the layout's own part
                let rec = v
                    .iter()
                    .rev()
                    .find(|(c, _)| *c == 330)
                    .and_then(|(_, t)| handle::parse(t.as_bytes()))
                    .and_then(by_handle);
                let Some(b) = rec else { continue };
                // the paper's size and margins are in millimetres, whatever units the plot uses
                let (w, h) = (p.f(44).unwrap_or(0.0), p.f(45).unwrap_or(0.0));
                let info = &mut layouts[b];
                // the layout's own name: the `1` after its `AcDbLayout` marker (the one before
                // names the page setup)
                info.name = v
                    .iter()
                    .skip_while(|(c, t)| !(*c == 100 && t.eq_ignore_ascii_case("AcDbLayout")))
                    .find(|(c, _)| *c == 1)
                    .map(|(_, t)| t.to_string());
                info.tab = p.i(71);
                info.paper = (w > 0.0 && h > 0.0).then(|| Paper {
                    w_mm: w,
                    h_mm: h,
                    rotation: p.i(73).unwrap_or(0),
                    margins_mm: [
                        p.f(40).unwrap_or(0.0),
                        p.f(41).unwrap_or(0.0),
                        p.f(42).unwrap_or(0.0),
                        p.f(43).unwrap_or(0.0),
                    ],
                });
            }
            "IMAGEDEF" => {
                let v = pairs(s, u);
                let p = P(&v);
                if let (Some(h), Some(path)) = (
                    p.h(5)
                        .or_else(|| first(s, u, 5).and_then(|t| handle::parse(t.as_bytes()))),
                    p.s(1),
                ) {
                    image_defs.push((h, path.to_owned()));
                }
            }
            "SORTENTSTABLE" => {
                let v = pairs(s, u);
                let mut owner = None;
                let mut sub = false;
                let mut table = SortTable::default();
                let mut pending: Option<u64> = None;
                for (c, t) in &v {
                    match c {
                        100 if t.eq_ignore_ascii_case("AcDbSortentsTable") => sub = true,
                        330 if sub && owner.is_none() => {
                            owner = handle::parse(t.as_bytes()).and_then(by_handle);
                        }
                        331 if sub => pending = handle::parse(t.as_bytes()),
                        5 if sub => {
                            if let (Some(e), Some(k)) =
                                (pending.take(), handle::parse(t.as_bytes()))
                            {
                                table.pairs.push((e, k));
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(b) = owner {
                    sortents[b] = Some(table);
                }
            }
            _ => {}
        }
    }
    // viewports: each layout's VIEWPORT entities (the sheet's own, id 1, left out)
    for (i, eu) in s.entities.iter().enumerate() {
        if eu.head.kind != "VIEWPORT" {
            continue;
        }
        let Some(b) = doc.entities[i].owner else {
            continue;
        };
        if doc.blocks[b].kind != BlockKind::Layout {
            continue;
        }
        let v = pairs(s, &eu.head);
        let p = P(&v);
        if p.i(69) == Some(1) {
            continue;
        }
        let flags = p.i(90).unwrap_or(0);
        let dir = p.pt(16).unwrap_or([0.0, 0.0, 1.0]);
        let frozen: Vec<u64> = v
            .iter()
            .filter(|(c, _)| *c == 341 || *c == 331)
            .filter_map(|(_, t)| handle::parse(t.as_bytes()))
            .collect();
        layouts[b].viewports.push(Viewport {
            handle: p.h(5).or(doc.entities[i].handle),
            centre: p.pt(10).map_or([0.0; 2], |q| [q[0], q[1]]),
            size: [p.f(40).unwrap_or(0.0), p.f(41).unwrap_or(0.0)],
            view_centre: p.pt(12).map_or([0.0; 2], |q| [q[0], q[1]]),
            view_height: p.f(45).unwrap_or(0.0),
            rotation: p.f(51).unwrap_or(0.0).to_radians(),
            on: p.i(68).unwrap_or(1) != 0,
            frozen_layers: frozen,
            clip: if flags & 0x10000 != 0 { p.h(340) } else { None },
            plan: dir[0].abs() < 1e-12 && dir[1].abs() < 1e-12 && dir[2] > 0.0,
        });
    }
    // external references: each xref block's path and kind
    let mut xrefs = Vec::new();
    for bd in &s.blocks {
        let flags = first(s, &bd.head, 70).and_then(|v| int(&v)).unwrap_or(0);
        if flags & 4 == 0 {
            continue;
        }
        let name = first(s, &bd.head, 2).unwrap_or_default();
        let Some(b) = doc.block_named(&name) else {
            continue;
        };
        xrefs.push(XrefPath {
            block: b,
            path: first(s, &bd.head, 1).unwrap_or_default().into_owned(),
            overlay: flags & 8 != 0,
        });
    }
    let dimstyles = s
        .table("DIMSTYLE")
        .map(|t| {
            t.entries
                .iter()
                .map(|u| {
                    let v = pairs(s, u);
                    let p = P(&v);
                    DimStyle {
                        name: p.s(2).unwrap_or("").to_owned(),
                        arrow_size: p.f(41),
                        scale: p.f(40),
                        leader_arrow: p.h(341),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    DrawTables {
        dimstyles,
        image_defs,
        settings,
        layers,
        linetypes,
        text_styles,
        layouts,
        sortents,
        xrefs,
    }
}
