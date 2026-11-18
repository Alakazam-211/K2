//! The two shapes: `cad.drawing.header/1` ([`header`]) and `cad.drawing.page/1` ([`page`]).

use std::collections::HashMap;

use k2_mpc_base::errors::Value;

use super::fl;
use k2_mpc_base::index::Digest;

use super::curve::{self, arc_points, insert_matrix, map_box, Mat, NoPath};
use super::style;
use super::text::{self, plan};
use super::{
    hatch, DrawSource, DrawTables, Ent, Geom, XrefResolver, PAGE_BYTES, PAGE_ENTITIES, WHYS,
};
use crate::args::{self, Spec};
use crate::error::{CadError, NOT_FOUND, STALE_CURSOR};
use crate::geom::{self, Acc};
use crate::model::{BboxQ, BlockKind, Doc};
use crate::mutant;
use crate::reads::{self, ReadOut};

/// A page's bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageBudget {
    pub entities: usize,
    pub bytes: usize,
}

impl Default for PageBudget {
    fn default() -> Self {
        PageBudget {
            entities: PAGE_ENTITIES,
            bytes: PAGE_BYTES,
        }
    }
}

fn pt(p: [f64; 2]) -> Value {
    Value::List(vec![fl(p[0]), fl(p[1])])
}

fn bx(b: [f64; 4]) -> Value {
    Value::List(b.iter().map(|x| fl(*x)).collect())
}

fn path(p: &[[f64; 2]]) -> Value {
    Value::List(p.iter().map(|q| pt(*q)).collect())
}

fn hkey(h: Option<u64>) -> Value {
    h.map_or(Value::Null, |h| Value::from(crate::handle::key(h)))
}

fn hex(d: &Digest) -> String {
    d.as_bytes()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

const HEADER: Spec = Spec {
    verb: "draw header",
    valued: &["--version"],
    bare: &[],
    words: false,
};

const PAGE: Spec = Spec {
    verb: "draw page",
    valued: &["--version", "--space", "--cursor"],
    bare: &[],
    words: false,
};

fn version(spec: &Spec, v: Option<&str>) -> Result<u64, CadError> {
    let v = v.ok_or_else(|| args::usage(spec, "--version N is needed: the doc version read"))?;
    v.trim()
        .parse::<u64>()
        .map_err(|_| args::usage(spec, "--version takes a whole number"))
}

/// The block record `--space` names: `model`, or an address.
fn space_of(doc: &Doc, s: &str) -> Result<usize, CadError> {
    let found = if s.eq_ignore_ascii_case("model") {
        doc.blocks.iter().position(|b| b.kind == BlockKind::Model)
    } else {
        (0..doc.blocks.len()).find(|&i| doc.block_address(i).eq_ignore_ascii_case(s))
    };
    found.ok_or_else(|| {
        CadError::new(
            NOT_FOUND,
            format!("{s}: no space or block has that address"),
            "`draw header` lists the layouts and blocks",
        )
    })
}

/// The address a space is named by in the shapes: `model`, else its block address.
fn space_name(doc: &Doc, b: usize) -> String {
    if doc.blocks[b].kind == BlockKind::Model {
        "model".into()
    } else {
        doc.block_address(b)
    }
}

/// The entities of space `b`, in draw order: by sort handle when the space has a draw-order
/// table (an entity it doesn't list sorts by its own handle), else in file order.
pub fn draw_order(doc: &Doc, t: &DrawTables, b: usize) -> Vec<usize> {
    draw_order_of(doc, t, b, members(doc).swap_remove(b))
}

fn draw_order_of(doc: &Doc, t: &DrawTables, b: usize, mut ids: Vec<usize>) -> Vec<usize> {
    if let Some(Some(st)) = t.sortents.get(b) {
        if !mutant::active("SortentsIgnored")
            && ids.iter().all(|&i| doc.entities[i].handle.is_some())
        {
            let map: HashMap<u64, u64> = st.pairs.iter().copied().collect();
            ids.sort_by_key(|&i| {
                let h = doc.entities[i].handle.unwrap_or(0);
                map.get(&h).copied().unwrap_or(h)
            });
        }
    } else if mutant::active("HandleOrderWithoutTable") {
        // the planted bug: handle order where the space has no draw-order table
        ids.sort_by_key(|&i| doc.entities[i].handle.unwrap_or(0));
    }
    ids
}

/// Each block record's entities (indexes into the model's entities), in file order: built once
/// per call, so no lookup walks every entity.
fn members(doc: &Doc) -> Vec<Vec<usize>> {
    let mut m = vec![Vec::new(); doc.blocks.len()];
    for (i, e) in doc.entities.iter().enumerate() {
        if let Some(v) = e.owner.and_then(|o| m.get_mut(o)) {
            v.push(i);
        }
    }
    m
}

/// Block boxes, computed once each (nested references through their matrices), and each
/// block's entities.
struct Boxes {
    memo: HashMap<usize, Option<([f64; 4], bool)>>,
    members: Vec<Vec<usize>>,
}

impl Boxes {
    fn new(doc: &Doc) -> Self {
        Boxes {
            memo: HashMap::new(),
            members: members(doc),
        }
    }

    fn of(&self, b: usize) -> Vec<usize> {
        self.members.get(b).cloned().unwrap_or_default()
    }

    /// Block `b`'s box in its own coordinates, and whether it is approximate.
    fn get(&mut self, src: &dyn DrawSource, b: usize, depth: u32) -> Option<([f64; 4], bool)> {
        if let Some(v) = self.memo.get(&b) {
            return *v;
        }
        if depth > 32 {
            return None;
        }
        // a block that reaches itself has no box (and doesn't loop)
        self.memo.insert(b, None);
        let doc = src.doc();
        let mut acc = Acc::new();
        let mut approx = false;
        for i in self.of(b) {
            let e = &doc.entities[i];
            if &*e.kind == "INSERT" {
                if let Some((bb, _)) = self.insert_box(src, i, depth + 1) {
                    acc.add_box(bb);
                    approx = true;
                }
                continue;
            }
            if let (Some(bb), q) = (e.bbox, e.bbox_q) {
                if q != BboxQ::None {
                    acc.add_box(bb);
                    approx |= q != BboxQ::Exact;
                }
            }
        }
        let v = acc.get().map(|bb| (bb, approx));
        self.memo.insert(b, v);
        v
    }

    /// Entity `i`'s (a block reference's) box: its block's box through its matrix.
    fn insert_box(
        &mut self,
        src: &dyn DrawSource,
        i: usize,
        depth: u32,
    ) -> Option<([f64; 4], Mat)> {
        let doc = src.doc();
        let Ok(ent) = src.entity(i) else {
            return None;
        };
        let Geom::Insert {
            block,
            at,
            scale,
            rotation,
            n,
            ..
        } = &ent.geom
        else {
            return None;
        };
        let b = doc.block_named(block)?;
        let base = doc.blocks[b].base.map_or([0.0, 0.0], |p| [p[0], p[1]]);
        let m = insert_matrix(base, *at, *scale, *rotation, *n);
        let (bb, _) = self.get(src, b, depth)?;
        Some((map_box(m, bb), m))
    }
}

/// `draw header --version N`: `cad.drawing.header/1`.
pub fn header(
    src: &dyn DrawSource,
    a: &[&str],
    xr: &dyn XrefResolver,
) -> Result<ReadOut, CadError> {
    let args = HEADER.parse(a)?;
    let ver = version(&HEADER, args.get("--version"))?;
    let doc = src.doc();
    let t = src.tables();
    let s = &t.settings;
    let settings = Value::obj([
        ("ltscale", fl(s.ltscale)),
        ("psltscale", Value::from(s.psltscale)),
        ("lwdefault_mm", fl(s.lwdefault.max(0) as f64 / 100.0)),
        ("lwdisplay", Value::from(s.lwdisplay)),
        ("fillmode", Value::from(s.fillmode)),
        ("pdmode", Value::from(s.pdmode)),
        ("pdsize", fl(s.pdsize)),
        ("mirrtext", Value::from(s.mirrtext)),
    ]);
    let layers: Vec<Value> = doc
        .layers
        .iter()
        .enumerate()
        .map(|(k, l)| {
            let ld = t.layers.get(k).cloned().unwrap_or_default();
            let n = l
                .color
                .map(i64::abs)
                .filter(|n| (1..=255).contains(n))
                .unwrap_or(7);
            let rgb = ld
                .rgb
                .map(super::aci::from_u32)
                .unwrap_or_else(|| super::aci::rgb(n).unwrap_or([255; 3]));
            let lw =
                l.lw.as_deref()
                    .and_then(|v| v.trim().parse::<i64>().ok())
                    .filter(|w| *w >= 0)
                    .unwrap_or(s.lwdefault.max(0));
            let alpha = ld
                .alpha
                .filter(|c| c & 0x0200_0000 != 0)
                .map_or(1.0, |c| (c & 0xFF) as f64 / 255.0);
            Value::obj([
                ("handle", hkey(l.handle)),
                ("name", Value::from(l.name.as_str())),
                (
                    "rgb",
                    Value::List(rgb.iter().map(|x| Value::from(u64::from(*x))).collect()),
                ),
                ("aci", Value::from(n)),
                ("fg", Value::from(ld.rgb.is_none() && n == 7)),
                ("alpha", fl(alpha)),
                (
                    "ltype",
                    Value::from(l.ltype.as_deref().unwrap_or("CONTINUOUS")),
                ),
                ("lw_mm", fl(lw as f64 / 100.0)),
                ("on", Value::from(l.on())),
                ("frozen", Value::from(l.frozen)),
                ("locked", Value::from(l.locked)),
                ("plot", Value::from(l.plot)),
            ])
        })
        .collect();
    let linetypes: Vec<Value> = t
        .linetypes
        .iter()
        .map(|lt| {
            Value::obj([
                ("handle", hkey(lt.handle)),
                ("name", Value::from(lt.name.as_str())),
                ("length", fl(lt.length)),
                (
                    "dashes",
                    Value::List(lt.dashes.iter().map(|x| fl(*x)).collect()),
                ),
                (
                    "complex",
                    Value::List(
                        lt.complex
                            .iter()
                            .map(|c| {
                                Value::obj([
                                    ("after", Value::from(c.after)),
                                    ("kind", Value::from(c.kind)),
                                    ("text", c.text.as_deref().map_or(Value::Null, Value::from)),
                                    ("shape", c.shape.map_or(Value::Null, Value::from)),
                                    ("style", hkey(c.style)),
                                    ("scale", fl(c.scale)),
                                    ("rotation", fl(c.rotation)),
                                    ("offset", pt(c.offset)),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ])
        })
        .collect();
    let styles: Vec<Value> = t
        .text_styles
        .iter()
        .map(|st| {
            let family = st.family.clone().unwrap_or_else(|| {
                let f = st.file.rsplit(['/', '\\']).next().unwrap_or("");
                f.split('.').next().unwrap_or("").to_owned()
            });
            Value::obj([
                ("handle", hkey(st.handle)),
                ("name", Value::from(st.name.as_str())),
                ("family", Value::from(family)),
                ("file", Value::from(st.file.as_str())),
                (
                    "bigfont",
                    st.bigfont.as_deref().map_or(Value::Null, Value::from),
                ),
                ("shx", Value::from(st.shx())),
                ("fixed_height", fl(st.fixed_height)),
                ("width", fl(st.width)),
                ("oblique", fl(st.oblique)),
                ("bold", Value::from(st.bold)),
                ("italic", Value::from(st.italic)),
                ("backwards", Value::from(st.backwards)),
                ("upside_down", Value::from(st.upside_down)),
                ("vertical", Value::from(st.vertical)),
            ])
        })
        .collect();
    let mut boxes = Boxes::new(doc);
    let counts: Vec<u64> = boxes.members.iter().map(|m| m.len() as u64).collect();
    let mut blocks = Vec::new();
    for (b, rec) in doc.blocks.iter().enumerate() {
        if rec.kind != BlockKind::Block {
            continue;
        }
        let mut d = Digest::stream();
        d.update(rec.record.as_bytes());
        for &i in &boxes.members[b] {
            d.update(doc.entities[i].record.as_bytes());
        }
        let bbox = boxes.get(src, b, 0);
        let xref = t.xrefs.iter().find(|x| x.block == b);
        blocks.push(Value::obj([
            ("handle", Value::from(doc.block_address(b))),
            ("name", Value::from(rec.name.as_str())),
            ("base", rec.base.map_or(Value::Null, |p| pt([p[0], p[1]]))),
            ("entities", Value::from(counts[b])),
            ("bbox", bbox.map_or(Value::Null, |(bb, _)| bx(bb))),
            (
                "bbox_q",
                Value::from(match bbox {
                    Some((_, true)) => "approx",
                    Some((_, false)) => "exact",
                    None => "none",
                }),
            ),
            ("digest", Value::from(hex(&d.finish()))),
            ("anon", Value::from(rec.anon)),
            ("xref", Value::from(rec.xref || xref.is_some())),
            (
                "dynamic",
                src.block_marks(b).0.map_or(Value::Null, Value::from),
            ),
            (
                "annotative",
                src.block_marks(b).1.map_or(Value::Null, Value::from),
            ),
            (
                "attdefs",
                src.attdefs(b).map_or(Value::Null, |a| {
                    Value::List(
                        a.iter()
                            .map(|d| {
                                Value::obj([
                                    ("tag", Value::from(d.tag.as_str())),
                                    ("default", Value::from(d.default.as_str())),
                                    ("constant", Value::from(d.constant)),
                                    ("prompt", Value::from(d.prompt.as_str())),
                                ])
                            })
                            .collect(),
                    )
                }),
            ),
        ]));
    }
    let mut layouts = Vec::new();
    for (b, rec) in doc.blocks.iter().enumerate() {
        if rec.kind == BlockKind::Block {
            continue;
        }
        let info = t.layouts.get(b).cloned().unwrap_or_default();
        let paper = info.paper.as_ref().map_or(Value::Null, |p| {
            Value::obj([
                ("w_mm", fl(p.w_mm)),
                ("h_mm", fl(p.h_mm)),
                ("rotation", Value::from(p.rotation)),
                ("margins_mm", bx(p.margins_mm)),
            ])
        });
        let vps: Vec<Value> = info
            .viewports
            .iter()
            .map(|v| {
                Value::obj([
                    ("handle", hkey(v.handle)),
                    ("centre", pt(v.centre)),
                    ("size", pt(v.size)),
                    ("view_centre", pt(v.view_centre)),
                    ("view_height", fl(v.view_height)),
                    (
                        "scale",
                        if v.view_height > 0.0 {
                            fl(v.size[1] / v.view_height)
                        } else {
                            Value::Null
                        },
                    ),
                    ("rotation", fl(v.rotation)),
                    ("on", Value::from(v.on)),
                    (
                        "frozen_layers",
                        Value::List(v.frozen_layers.iter().map(|h| hkey(Some(*h))).collect()),
                    ),
                    ("clip", hkey(v.clip)),
                    ("plan", Value::from(v.plan)),
                ])
            })
            .collect();
        let sorted = t.sortents.get(b).is_some_and(Option::is_some);
        layouts.push(Value::obj([
            ("handle", Value::from(doc.block_address(b))),
            (
                "name",
                Value::from(info.name.clone().unwrap_or_else(|| rec.name.clone())),
            ),
            (
                "kind",
                Value::from(if rec.kind == BlockKind::Model {
                    "model"
                } else {
                    "paper"
                }),
            ),
            ("space", Value::from(space_name(doc, b))),
            ("entities", Value::from(counts[b])),
            ("tab", info.tab.map_or(Value::Null, Value::from)),
            (
                "draw_order",
                Value::from(if sorted { "sortents" } else { "file" }),
            ),
            ("paper", paper),
            ("viewports", Value::List(vps)),
        ]));
    }
    let xrefs: Vec<Value> = t
        .xrefs
        .iter()
        .map(|x| {
            let doc_ref = xr.resolve(&x.path);
            Value::obj([
                ("block", Value::from(doc.block_address(x.block))),
                ("name", Value::from(doc.blocks[x.block].name.as_str())),
                ("path", Value::from(x.path.as_str())),
                ("overlay", Value::from(x.overlay)),
                ("resolved", Value::from(doc_ref.is_some())),
                ("doc", doc_ref.map_or(Value::Null, Value::from)),
            ])
        })
        .collect();
    let stated = doc
        .header_extents
        .map_or(Value::Null, |(lo, hi)| bx([lo[0], lo[1], hi[0], hi[1]]));
    let mut acc = Acc::new();
    for e in &doc.entities {
        if doc.owner_kind(e) == Some(BlockKind::Model) && e.bbox_q != BboxQ::None {
            if let Some(b) = e.bbox {
                acc.add_box(b);
            }
        }
    }
    let data = Value::obj([
        ("schema", Value::from("cad.drawing.header/1")),
        ("format", Value::from(doc.format)),
        ("version", Value::from(ver)),
        ("acadver", Value::from(doc.version.as_str())),
        ("units", reads::units(&doc.units)),
        (
            "extents",
            Value::obj([
                ("stated", stated),
                ("computed", acc.get().map_or(Value::Null, bx)),
            ]),
        ),
        ("settings", settings),
        ("layers", Value::List(layers)),
        ("linetypes", Value::List(linetypes)),
        ("text_styles", Value::List(styles)),
        ("blocks", Value::List(blocks)),
        // the layer adds and inserts go on when none is named, as the layers list names it
        (
            "current_layer",
            src.current_layer()
                .and_then(|l| doc.layers[l].handle)
                .map_or(Value::Null, |h| Value::from(crate::handle::key(h))),
        ),
        // model space's own block record, by the address a hold on it has; null when the
        // file gives it no handle
        (
            "model_space_block",
            doc.blocks
                .iter()
                .find(|b| b.kind == BlockKind::Model)
                .and_then(|b| b.handle)
                .map_or(Value::Null, |h| Value::from(crate::handle::key(h))),
        ),
        ("layouts", Value::List(layouts)),
        ("xrefs", Value::List(xrefs)),
        (
            "whys",
            Value::List(WHYS.iter().map(|w| Value::from(*w)).collect()),
        ),
        (
            "budget",
            Value::obj([
                ("entities", Value::from(PAGE_ENTITIES)),
                ("bytes", Value::from(PAGE_BYTES)),
            ]),
        ),
    ]);
    let stdout = format!(
        "{} {} version {}: {} layers, {} blocks, {} layouts, units {}\n",
        doc.format,
        doc.version,
        ver,
        doc.layers.len(),
        doc.blocks
            .iter()
            .filter(|b| b.kind == BlockKind::Block)
            .count(),
        doc.blocks
            .iter()
            .filter(|b| b.kind != BlockKind::Block)
            .count(),
        doc.units.word
    );
    let mut data = data;
    super::snap(&mut data, super::noise_scale(doc));
    Ok(ReadOut {
        stdout,
        data,
        spec: ("cad.drawing.header", 1),
        next: None,
    })
}

/// `draw page --version N --space S [--cursor C]`: `cad.drawing.page/1`.
pub fn page(src: &dyn DrawSource, a: &[&str]) -> Result<ReadOut, CadError> {
    page_with(src, a, PageBudget::default())
}

fn cursor_error(why: &str) -> CadError {
    CadError::new(
        STALE_CURSOR,
        format!("the cursor {why}"),
        "read the space's first page at the version you want (no --cursor), or apply cad.change/1 from the cursor's version",
    )
}

/// [`page`] with the bounds `budget`.
pub fn page_with(
    src: &dyn DrawSource,
    a: &[&str],
    budget: PageBudget,
) -> Result<ReadOut, CadError> {
    let args = PAGE.parse(a)?;
    let ver = version(&PAGE, args.get("--version"))?;
    let doc = src.doc();
    let t = src.tables();
    let space_arg = args
        .get("--space")
        .ok_or_else(|| args::usage(&PAGE, "--space model|@h:<layout or block> is needed"))?;
    let space = space_of(doc, space_arg)?;
    let sname = space_name(doc, space);
    let start = match args.get("--cursor") {
        None => 0usize,
        Some(c) => {
            let rest = c
                .strip_prefix('v')
                .ok_or_else(|| cursor_error("doesn't read"))?;
            let (v, rest) = rest
                .split_once(':')
                .ok_or_else(|| cursor_error("doesn't read"))?;
            let (z, sp) = rest
                .split_once(':')
                .ok_or_else(|| cursor_error("doesn't read"))?;
            let v: u64 = v.parse().map_err(|_| cursor_error("doesn't read"))?;
            let z: usize = z.parse().map_err(|_| cursor_error("doesn't read"))?;
            if v != ver && !mutant::active("StaleCursorServed") {
                return Err(cursor_error(&format!(
                    "is from version {v}, not version {ver}"
                )));
            }
            if !sp.eq_ignore_ascii_case(&sname) {
                return Err(cursor_error("is from another space"));
            }
            z + 1
        }
    };
    let mut boxes = Boxes::new(doc);
    let order = draw_order_of(doc, t, space, boxes.of(space));
    let mut rows: Vec<Value> = Vec::new();
    let mut bytes = 0usize;
    let mut z = start;
    let mut oversize = false;
    let mut not_drawn = 0u64;
    let mut out = String::new();
    while z < order.len() && rows.len() < budget.entities {
        let i = order[z];
        let (v, drawn) = entity_value(src, i, z, &mut boxes);
        let n = v.to_json().len() + 1;
        if !rows.is_empty() && bytes + n > budget.bytes && !mutant::active("PageOverBudget") {
            break;
        }
        if rows.is_empty() && n > budget.bytes {
            oversize = true;
        }
        if !drawn {
            not_drawn += 1;
        }
        out.push_str(&format!(
            "{z}\t{}\t{}\t{}\n",
            doc.entity_address(i),
            doc.entities[i].kind,
            doc.entities[i].layer
        ));
        bytes += n;
        rows.push(v);
        z += 1;
        if oversize {
            break;
        }
    }
    let last = z.checked_sub(1).filter(|l| *l >= start);
    let next_args: Option<Vec<String>> = (z < order.len()).then(|| {
        vec![
            "--version".into(),
            ver.to_string(),
            "--space".into(),
            sname.clone(),
            "--cursor".into(),
            format!("v{ver}:{}:{sname}", last.unwrap_or(start)),
        ]
    });
    let data = Value::obj([
        ("schema", Value::from("cad.drawing.page/1")),
        ("format", Value::from(doc.format)),
        ("version", Value::from(ver)),
        ("units", reads::units(&doc.units)),
        ("space", Value::from(sname.as_str())),
        ("first", Value::from(start == 0)),
        ("z_first", Value::from(start)),
        ("z_last", last.map_or(Value::Null, Value::from)),
        ("of", Value::from(order.len())),
        ("not_drawn", Value::from(not_drawn)),
        ("oversize", Value::from(oversize)),
        (
            "budget",
            Value::obj([
                ("entities", Value::from(budget.entities)),
                ("bytes", Value::from(budget.bytes)),
            ]),
        ),
        ("entities", Value::List(rows)),
        (
            "next",
            next_args.as_ref().map_or(Value::Null, |n| {
                let mut v = vec![Value::from("draw"), Value::from("page")];
                v.extend(n.iter().map(|s| Value::from(s.as_str())));
                Value::List(v)
            }),
        ),
    ]);
    let mut data = data;
    super::snap(&mut data, super::noise_scale(src.doc()));
    Ok(ReadOut {
        stdout: out,
        data,
        spec: ("cad.drawing.page", 1),
        next: next_args,
    })
}

/// One entity's row, and whether it is drawn.
fn entity_value(src: &dyn DrawSource, i: usize, z: usize, boxes: &mut Boxes) -> (Value, bool) {
    let doc = src.doc();
    let t = src.tables();
    let e = &doc.entities[i];
    let ent = src.entity(i).unwrap_or_else(|_| Ent::of(Geom::Unreadable));
    let st = style::resolve(doc, t, i, &ent);
    let vis = style::visible(doc, i, &ent);
    let owner = match e.owner {
        Some(o) if doc.blocks[o].kind == BlockKind::Model => Value::from("model"),
        Some(o) => Value::from(doc.block_address(o)),
        None => Value::Null,
    };
    let g = geom_value(src, &ent, boxes);
    let (mut bbox, mut bq) = (e.bbox, e.bbox_q.word());
    if matches!(ent.geom, Geom::Insert { .. }) {
        if let Some((bb, _)) = boxes.insert_box(src, i, 0) {
            bbox = Some(bb);
            bq = "approx";
        }
    }
    if bbox.is_none() {
        if let Some(bb) = drawn_box(src, &ent, boxes) {
            bbox = Some(bb);
            bq = "approx";
        }
    }
    let mut v: Vec<(&str, Value)> = vec![
        ("h", Value::from(doc.entity_address(i))),
        ("kind", Value::from(&*e.kind)),
        ("layer", Value::from(&*e.layer)),
        ("owner", owner),
        ("z", Value::from(z)),
        ("style", st.value()),
        ("visible", style::visible_value(&vis)),
        ("edit", src.edit(i).map_or(Value::Null, Value::from)),
    ];
    let drawn = g.is_ok();
    v.push(("drawn", Value::from(drawn)));
    if let Err(why) = &g {
        v.push(("why", Value::from(*why)));
    }
    v.push(("bbox", bbox.map_or(Value::Null, bx)));
    v.push((
        "bbox_q",
        Value::from(if bbox.is_some() { bq } else { "none" }),
    ));
    if let Ok(g) = g {
        v.push(("geom", g));
    }
    (Value::obj(v), drawn)
}

/// A box, containing the drawn geometry, for the kinds the model gives none: a hatch's loops,
/// a leader's path, an image's or wipeout's outline, a dimension's block.
fn drawn_box(src: &dyn DrawSource, ent: &Ent, boxes: &mut Boxes) -> Option<[f64; 4]> {
    let mut acc = Acc::new();
    match &ent.geom {
        Geom::Hatch(h) => {
            let tol = hatch::hatch_tol(h);
            for l in &h.loops {
                for p in hatch::loop_polygon(l, tol) {
                    let q = plan([p[0], p[1], h.elevation], h.n);
                    acc.add(q[0] - tol, q[1] - tol);
                    acc.add(q[0] + tol, q[1] + tol);
                }
            }
        }
        Geom::Leader { verts, .. } => {
            for p in verts {
                acc.add(p[0], p[1]);
            }
        }
        Geom::Mleader(m) => {
            for p in m.lines().iter().flatten().chain(m.at.iter()) {
                acc.add(p[0], p[1]);
            }
        }
        Geom::Image {
            ins, u, v, size, ..
        } => {
            for (x, y) in [
                (0.0, 0.0),
                (size[0], 0.0),
                (0.0, size[1]),
                (size[0], size[1]),
            ] {
                acc.add(ins[0] + u[0] * x + v[0] * y, ins[1] + u[1] * x + v[1] * y);
            }
        }
        Geom::Dimension { block, ins, .. } => {
            let doc = src.doc();
            let b = doc.block_named(block)?;
            let (bb, _) = boxes.get(src, b, 0)?;
            let d = ins.map_or([0.0, 0.0], |p| [p[0], p[1]]);
            acc.add_box([bb[0] + d[0], bb[1] + d[1], bb[2] + d[0], bb[3] + d[1]]);
        }
        _ => return None,
    }
    acc.get()
}

/// The world plan's points of an arc in frame `n` (angles in radians, counter-clockwise).
fn arc_path(c: [f64; 3], r: f64, a0: f64, sw: f64, n: [f64; 3], tol: f64) -> Vec<[f64; 2]> {
    arc_points([c[0], c[1]], r, a0, sw, tol)
        .into_iter()
        .map(|p| plan([p[0], p[1], c[2]], n))
        .collect()
}

/// Whether frame `n` looks straight down (the plan mirrored).
fn is_down(n: [f64; 3]) -> bool {
    n[0].abs() < 1e-12 && n[1].abs() < 1e-12 && n[2] < 0.0
}

fn ccw_sweep(a0: f64, a1: f64) -> f64 {
    let mut s = (a1 - a0).rem_euclid(std::f64::consts::TAU);
    if s == 0.0 {
        s = std::f64::consts::TAU;
    }
    s
}

/// One entity's `geom`, or why it isn't drawn.
fn geom_value(src: &dyn DrawSource, ent: &Ent, boxes: &mut Boxes) -> Result<Value, &'static str> {
    let doc = src.doc();
    let t = src.tables();
    let xy = |p: [f64; 3]| pt([p[0], p[1]]);
    Ok(match &ent.geom {
        Geom::Line { p1, p2 } => Value::obj([("p1", xy(*p1)), ("p2", xy(*p2))]),
        Geom::XLine { point, dir, ray } => {
            let l = dir[0].hypot(dir[1]);
            let d = if l > 0.0 {
                [dir[0] / l, dir[1] / l]
            } else {
                [1.0, 0.0]
            };
            Value::obj([
                ("point", xy(*point)),
                ("dir", pt(d)),
                ("ray", Value::from(*ray)),
            ])
        }
        Geom::Point { p } => Value::obj([
            ("p", xy(*p)),
            ("pdmode", Value::from(t.settings.pdmode)),
            ("pdsize", fl(t.settings.pdsize)),
        ]),
        Geom::Circle { c, r, n } => {
            if geom::is_up(*n) || is_down(*n) {
                Value::obj([("centre", pt(plan(*c, *n))), ("r", fl(r.abs()))])
            } else {
                let tol = (1e-4 * r.abs()).max(1e-9);
                Value::obj([
                    (
                        "path",
                        path(&arc_path(*c, *r, 0.0, std::f64::consts::TAU, *n, tol)),
                    ),
                    ("ocs", Value::from("tilted")),
                ])
            }
        }
        Geom::Arc { c, r, a0, a1, n } => {
            let (s, e) = (a0.to_radians(), a1.to_radians());
            if geom::is_up(*n) {
                Value::obj([
                    ("centre", pt([c[0], c[1]])),
                    ("r", fl(r.abs())),
                    ("a0", fl(s)),
                    ("a1", fl(e)),
                    ("ccw", Value::from(true)),
                ])
            } else if is_down(*n) {
                let pi = std::f64::consts::PI;
                Value::obj([
                    ("centre", pt(plan(*c, *n))),
                    ("r", fl(r.abs())),
                    ("a0", fl(pi - e)),
                    ("a1", fl(pi - s)),
                    ("ccw", Value::from(true)),
                ])
            } else {
                let tol = (1e-4 * r.abs()).max(1e-9);
                Value::obj([
                    ("path", path(&arc_path(*c, *r, s, ccw_sweep(s, e), *n, tol))),
                    ("ocs", Value::from("tilted")),
                ])
            }
        }
        Geom::Ellipse {
            c,
            major,
            ratio,
            p0,
            p1,
            n,
        } => {
            if geom::is_up(*n) {
                Value::obj([
                    ("centre", xy(*c)),
                    ("major", xy(*major)),
                    ("ratio", fl(*ratio)),
                    ("p0", fl(*p0)),
                    ("p1", fl(*p1)),
                ])
            } else if is_down(*n) {
                Value::obj([
                    ("centre", xy(*c)),
                    ("major", xy(*major)),
                    ("ratio", fl(*ratio)),
                    ("p0", fl(-p1)),
                    ("p1", fl(-p0)),
                ])
            } else {
                let nl = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-300);
                let u = [n[0] / nl, n[1] / nl, n[2] / nl];
                let minor = [
                    (u[1] * major[2] - u[2] * major[1]) * ratio,
                    (u[2] * major[0] - u[0] * major[2]) * ratio,
                    (u[0] * major[1] - u[1] * major[0]) * ratio,
                ];
                let r = (major[0].powi(2) + major[1].powi(2) + major[2].powi(2)).sqrt();
                let tol = (1e-4 * r).max(1e-9);
                let pts: Vec<[f64; 2]> = arc_points([0.0, 0.0], r, *p0, ccw_sweep(*p0, *p1), tol)
                    .into_iter()
                    .map(|q| {
                        let a = q[1].atan2(q[0]);
                        [
                            c[0] + major[0] * a.cos() + minor[0] * a.sin(),
                            c[1] + major[1] * a.cos() + minor[1] * a.sin(),
                        ]
                    })
                    .collect();
                Value::obj([("path", path(&pts)), ("ocs", Value::from("tilted"))])
            }
        }
        Geom::Poly {
            verts,
            closed,
            const_w,
            world,
            n,
        } => {
            if *world || geom::is_up(*n) || is_down(*n) {
                let flip = !*world && is_down(*n);
                let vs: Vec<Value> = verts
                    .iter()
                    .map(|v| {
                        let p = if *world {
                            [v.p[0], v.p[1]]
                        } else {
                            plan(v.p, *n)
                        };
                        let b = if *world {
                            0.0
                        } else if flip {
                            -v.bulge
                        } else {
                            v.bulge
                        };
                        let b = if mutant::active("BulgeDropped") {
                            0.0
                        } else {
                            b
                        };
                        Value::obj([
                            ("p", pt(p)),
                            ("bulge", fl(b)),
                            ("w0", fl(v.w0)),
                            ("w1", fl(v.w1)),
                        ])
                    })
                    .collect();
                Value::obj([
                    ("verts", Value::List(vs)),
                    ("closed", Value::from(*closed)),
                    ("const_w", const_w.map_or(Value::Null, fl)),
                ])
            } else {
                let mut acc = Acc::new();
                for v in verts {
                    acc.add(v.p[0], v.p[1]);
                }
                let tol = acc.get().map_or(1e-9, |b| {
                    (1e-4 * (b[2] - b[0]).hypot(b[3] - b[1])).max(1e-9)
                });
                let mut pts = Vec::new();
                for (k, v) in verts.iter().enumerate() {
                    pts.push(plan(v.p, *n));
                    let next = match verts.get(k + 1) {
                        Some(w) => Some(w),
                        None if *closed => verts.first(),
                        None => None,
                    };
                    if let (Some(w), true) = (next, v.bulge != 0.0) {
                        let lp = hatch_bulge(v.p, w.p, v.bulge, tol);
                        pts.extend(lp.into_iter().map(|q| plan([q[0], q[1], v.p[2]], *n)));
                    }
                }
                Value::obj([
                    ("path", path(&pts)),
                    ("closed", Value::from(*closed)),
                    ("ocs", Value::from("tilted")),
                ])
            }
        }
        Geom::Spline(s) => {
            let p2 = |v: &[[f64; 3]]| Value::List(v.iter().map(|p| pt([p[0], p[1]])).collect());
            let mut fields: Vec<(&str, Value)> = vec![
                ("degree", Value::from(s.degree)),
                ("closed", Value::from(s.flags & 1 != 0)),
                ("periodic", Value::from(s.flags & 2 != 0)),
                ("rational", Value::from(s.flags & 4 != 0)),
                (
                    "knots",
                    Value::List(s.knots.iter().map(|k| fl(*k)).collect()),
                ),
                ("ctrl", p2(&s.ctrl)),
                (
                    "weights",
                    Value::List(s.weights.iter().map(|w| fl(*w)).collect()),
                ),
                ("fit", p2(&s.fit)),
            ];
            if s.ctrl.is_empty() && s.fit.len() >= 2 {
                let pts: Vec<[f64; 2]> = s.fit.iter().map(|p| [p[0], p[1]]).collect();
                fields.extend([
                    ("path", path(&pts)),
                    ("tol", Value::Null),
                    ("tol_achieved", Value::Null),
                    ("interpolated", Value::from(false)),
                ]);
            } else {
                match curve::spline_path(s) {
                    Ok(p) => fields.extend([
                        ("path", path(&p.points)),
                        ("tol", fl(p.tol)),
                        ("tol_achieved", fl(p.achieved)),
                        ("interpolated", Value::from(true)),
                    ]),
                    Err(NoPath::Weights) => return Err("spline_weights"),
                    Err(NoPath::OverCap) => return Err("over_cap"),
                    Err(NoPath::Unreadable) => return Err("unreadable"),
                }
            }
            Value::obj(fields)
        }
        Geom::Text(tr) => text::place(tr, &t.text_styles),
        Geom::AttDef { text: tr, constant } => {
            if *constant {
                text::place(tr, &t.text_styles)
            } else {
                return Err("attdef");
            }
        }
        Geom::MText(m) => text::mtext(m),
        Geom::Insert {
            block,
            at,
            scale,
            rotation,
            array,
            attribs,
            n,
        } => {
            let b = doc.block_named(block);
            let base = b
                .and_then(|b| doc.blocks[b].base)
                .map_or([0.0, 0.0], |p| [p[0], p[1]]);
            let m = insert_matrix(base, *at, *scale, *rotation, *n);
            let atts: Vec<Value> = attribs
                .iter()
                .map(|a| {
                    let placed = text::place(&a.text, &t.text_styles);
                    let plain = placed.get("plain").cloned().unwrap_or(Value::Null);
                    Value::obj([
                        ("h", hkey(a.handle)),
                        ("tag", Value::from(a.tag.as_str())),
                        ("value", plain),
                        ("visible", Value::from(a.flags & 1 == 0)),
                        ("constant", Value::from(a.flags & 2 != 0)),
                        ("layer", Value::from(a.layer.as_str())),
                        ("text", placed),
                    ])
                })
                .collect();
            let mut fields: Vec<(&str, Value)> = vec![
                (
                    "block",
                    b.map_or(Value::Null, |b| Value::from(doc.block_address(b))),
                ),
                ("name", Value::from(block.as_str())),
                ("m", Value::List(m.iter().map(|x| fl(*x)).collect())),
                (
                    "array",
                    array.map_or(Value::Null, |(c, r, dx, dy)| {
                        Value::obj([
                            ("cols", Value::from(c)),
                            ("rows", Value::from(r)),
                            ("dx", fl(dx)),
                            ("dy", fl(dy)),
                        ])
                    }),
                ),
                (
                    "xref",
                    Value::from(b.is_some_and(|b| {
                        doc.blocks[b].xref || t.xrefs.iter().any(|x| x.block == b)
                    })),
                ),
                ("attribs", Value::List(atts)),
            ];
            // the planted bug: the block's entities copied in (an expansion, not a reference)
            if let (Some(b), true) = (b, mutant::active("InsertExpanded")) {
                let inner: Vec<Value> = boxes
                    .of(b)
                    .into_iter()
                    .map(|k| entity_value(src, k, 0, boxes).0)
                    .collect();
                fields.push(("entities", Value::List(inner)));
            }
            Value::obj(fields)
        }
        Geom::Solid { pts, n } => {
            let order = if mutant::active("SolidCornerOrder") {
                [0, 1, 2, 3]
            } else {
                [0, 1, 3, 2]
            };
            Value::obj([
                (
                    "quad",
                    Value::List(order.iter().map(|&k| pt(plan(pts[k], *n))).collect()),
                ),
                ("filled", Value::from(t.settings.fillmode)),
            ])
        }
        Geom::Face { pts, hidden } => Value::obj([
            ("quad", Value::List(pts.iter().map(|p| xy(*p)).collect())),
            ("hidden_edges", Value::from(*hidden)),
        ]),
        Geom::Hatch(h) => hatch::value(h),
        Geom::Dimension {
            block,
            kind,
            measured,
            text: over,
            style: ds,
            defs,
            ins,
        } => {
            let b = doc.block_named(block);
            let shown: String = b
                .map(|b| {
                    boxes
                        .of(b)
                        .into_iter()
                        .filter(|&k| matches!(&*doc.entities[k].kind, "MTEXT" | "TEXT"))
                        .filter_map(|k| src.entity(k).ok())
                        .map(|en| match en.geom {
                            Geom::MText(m) => text::parse_mtext(&m.content, m.height)
                                .paras
                                .iter()
                                .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
                                .collect::<Vec<_>>()
                                .join("\n"),
                            Geom::Text(t) => text::single_line_runs(&t.text)
                                .iter()
                                .map(|r| r.text.as_str())
                                .collect(),
                            _ => String::new(),
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            let m: Mat = match ins {
                Some(p) => [1.0, 0.0, 0.0, 1.0, p[0], p[1]],
                None => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            };
            Value::obj([
                (
                    "block",
                    b.map_or(Value::Null, |b| Value::from(doc.block_address(b))),
                ),
                ("name", Value::from(block.as_str())),
                ("m", Value::List(m.iter().map(|x| fl(*x)).collect())),
                ("type", Value::from(kind & 7)),
                ("flags", Value::from(*kind)),
                ("measured", measured.map_or(Value::Null, fl)),
                (
                    "text",
                    Value::obj([
                        (
                            "override",
                            if over.is_empty() || over == "<>" {
                                Value::Null
                            } else {
                                Value::from(over.as_str())
                            },
                        ),
                        ("shown", Value::from(shown)),
                    ]),
                ),
                (
                    "defs",
                    Value::obj(defs.iter().map(|(c, p)| (c.to_string(), xy(*p)))),
                ),
                ("style", Value::from(ds.as_str())),
                ("assoc", Value::Null),
            ])
        }
        Geom::Leader {
            verts,
            spline,
            arrow,
            style: ds,
            annotation,
            overrides,
        } => {
            let st = t.dimstyles.iter().find(|d| d.name.eq_ignore_ascii_case(ds));
            let size = overrides
                .arrow_size
                .or(st.and_then(|d| d.arrow_size))
                .unwrap_or(0.18);
            let scale = overrides
                .scale
                .or(st.and_then(|d| d.scale))
                .filter(|s| *s > 0.0)
                .unwrap_or(1.0);
            let kind = overrides
                .leader_arrow
                .or(st.and_then(|d| d.leader_arrow))
                .and_then(|h| doc.blocks.iter().find(|b| b.handle == Some(h)))
                .map_or("closed_filled".to_owned(), |b| b.name.clone());
            let arrow_v = Value::obj([
                ("shown", Value::from(*arrow)),
                ("kind", Value::from(kind)),
                ("size", fl(size * scale)),
            ]);
            Value::obj([
                ("path", Value::List(verts.iter().map(|p| xy(*p)).collect())),
                ("spline", Value::from(*spline)),
                ("arrow", arrow_v),
                ("style", Value::from(ds.as_str())),
                ("annotation", hkey(*annotation)),
            ])
        }
        Geom::Image {
            wipeout,
            ins,
            u,
            v,
            size,
            clip,
            clip_type,
            def,
            path: file,
        } => {
            let at = |x: f64, y: f64| -> [f64; 2] {
                [ins[0] + u[0] * x + v[0] * y, ins[1] + u[1] * x + v[1] * y]
            };
            let corners = [
                at(0.0, 0.0),
                at(size[0], 0.0),
                at(size[0], size[1]),
                at(0.0, size[1]),
            ];
            // the clip boundary's origin is the image's top-left pixel centre, y down
            let clip_pt = |c: [f64; 2]| at(c[0] + 0.5, size[1] - (c[1] + 0.5));
            let mut cl: Vec<[f64; 2]> = if *clip_type == 1 && clip.len() == 2 {
                let (a, b) = (clip[0], clip[1]);
                vec![
                    clip_pt(a),
                    clip_pt([b[0], a[1]]),
                    clip_pt(b),
                    clip_pt([a[0], b[1]]),
                ]
            } else {
                clip.iter().map(|c| clip_pt(*c)).collect()
            };
            if cl.is_empty() {
                cl = corners.to_vec();
            }
            let cl = ring(cl);
            if *wipeout {
                Value::obj([("mask", path(&cl)), ("closed", Value::from(true))])
            } else {
                Value::obj([
                    ("corners", path(&corners)),
                    ("clip", path(&cl)),
                    ("closed", Value::from(true)),
                    (
                        "image",
                        Value::obj([
                            ("def", hkey(*def)),
                            (
                                "path",
                                file.clone()
                                    .or_else(|| {
                                        def.and_then(|d| {
                                            t.image_defs
                                                .iter()
                                                .find(|(h, _)| *h == d)
                                                .map(|(_, p)| p.clone())
                                        })
                                    })
                                    .map_or(Value::Null, Value::from),
                            ),
                            ("resolved", Value::Null),
                        ]),
                    ),
                ])
            }
        }
        Geom::Mleader(m) => {
            let kind = m
                .arrow_block
                .and_then(|h| doc.blocks.iter().find(|b| b.handle == Some(h)))
                .map_or("closed_filled".to_owned(), |b| b.name.clone());
            let opt_f = |x: Option<f64>| x.map_or(Value::Null, fl);
            Value::obj([
                (
                    "leaders",
                    Value::List(
                        m.leaders
                            .iter()
                            .map(|b| {
                                Value::obj([
                                    ("landing", b.landing.map_or(Value::Null, pt)),
                                    (
                                        "lines",
                                        Value::List(b.lines.iter().map(|l| path(l)).collect()),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("dropped", Value::from(m.dropped)),
                (
                    "arrow",
                    Value::obj([
                        ("shown", Value::from(true)),
                        ("kind", Value::from(kind)),
                        ("size", opt_f(m.arrow_size)),
                    ]),
                ),
                ("text", m.text.as_deref().map_or(Value::Null, Value::from)),
                ("plain", m.plain().map_or(Value::Null, Value::from)),
                ("at", m.at.map_or(Value::Null, pt)),
                ("attachment", m.attachment.map_or(Value::Null, Value::from)),
                ("height", opt_f(m.height)),
            ])
        }
        Geom::Viewport => return Err("viewport"),
        Geom::NotModelled => {
            return Err(if mutant::active("WhyOutsideSet") {
                "unknown_kind"
            } else {
                "not_modelled"
            })
        }
        Geom::Unreadable => return Err("unreadable"),
    })
}

/// The points along a bulged segment in a frame's plane (its end included).
fn hatch_bulge(p0: [f64; 3], p1: [f64; 3], b: f64, tol: f64) -> Vec<[f64; 2]> {
    let l = super::LoopRaw {
        flags: 0,
        edges: Vec::new(),
        poly: Some((vec![([p0[0], p0[1]], b), ([p1[0], p1[1]], 0.0)], false)),
    };
    let mut pts = hatch::loop_polygon(&l, tol);
    if !pts.is_empty() {
        pts.remove(0);
    }
    pts
}

/// A closed ring's points with its first point not repeated at the end: every closed outline
/// (mask, clip, boundary path) comes this way, with `closed: true`.
pub fn ring(mut pts: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    while pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    pts
}

/// Whether `v` is one of the stable words ([`WHYS`]).
pub fn why_known(v: &str) -> bool {
    WHYS.contains(&v)
}
