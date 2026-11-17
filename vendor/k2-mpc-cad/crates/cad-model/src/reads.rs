//! The reads every drawing format shares, each a pure function of one [`Doc`], with typed data
//! whose shape [`ReadOut::spec`] names (`cad.<shape>/<version>`):
//!
//! | Verb | Data |
//! |---|---|
//! | `drawing info` | [`info`]: version, units, extents, counts, handles, what is editable (`cad.drawing/1`) |
//! | `layer list [--unused]` | [`layer_list`]: each layer, its switches, the entities on it (`cad.layer-list/1`) |
//! | `block list [--unused] [--anon]` | [`block_list`]: each block, its inserts direct and nested (`cad.block-list/1`) |
//! | `entity find …` | [`entity_find`]: entities by layer, kind, owner and box, paged (`cad.entity-find/1`) |
//! | `text find --contains S …` | [`text_find`]: text, multiline text and attribute values (`cad.text-find/1`) |
//!
//! Counts are computed here, on each read, never stored on a node. Every read gives the
//! drawing's units.

use std::collections::BTreeMap;

use k2_mpc_base::errors::Value;

use crate::args::{self, parse_box, parse_limit, Spec};
use crate::error::CadError;
use crate::geom::{inside, num, touches};
use crate::model::{BboxQ, BlockKind, Doc, Units};
use crate::mutant;

/// A read's output: its text, its typed data and the data's shape, and the next page's
/// arguments when there is one.
#[derive(Debug, Clone)]
pub struct ReadOut {
    /// One line per row, as the host prints it.
    pub stdout: String,
    pub data: Value,
    /// The data's shape: name and version.
    pub spec: (&'static str, u32),
    /// The arguments of the call that reads the next page.
    pub next: Option<Vec<String>>,
}

/// The largest page a find returns.
pub const PAGE_MAX: usize = 500;
/// A find's page when `--limit` isn't given.
pub const PAGE_DEFAULT: usize = 100;
/// How many expansions the nested insert counts may make before they stop (and say so).
pub const NESTED_VISITS_MAX: u64 = 10_000_000;

fn f(x: f64) -> Value {
    Value::float(x).unwrap_or(Value::Null)
}

fn pt(p: [f64; 2]) -> Value {
    Value::List(vec![f(p[0]), f(p[1])])
}

fn bx(b: [f64; 4]) -> Value {
    Value::List(b.iter().map(|x| f(*x)).collect())
}

fn opt_str(s: &Option<String>) -> Value {
    s.as_ref().map_or(Value::Null, |s| Value::from(s.as_str()))
}

/// The units, as every read gives them.
pub fn units(u: &Units) -> Value {
    Value::obj([
        ("word", Value::from(u.word)),
        ("insunits", u.insunits.map_or(Value::Null, Value::from)),
        (
            "measurement",
            u.measurement.map_or(Value::Null, Value::from),
        ),
        ("source", Value::from(u.source)),
    ])
}

fn units_of(doc: &Doc) -> Value {
    if mutant::active("UnitsDropped") {
        return Value::Null;
    }
    units(&doc.units)
}

/// The entities whose box is computed for the extents: model space's, with a box.
fn computed_extents(doc: &Doc) -> (Option<[f64; 4]>, u64, u64) {
    let mut acc = crate::geom::Acc::new();
    let (mut over, mut of) = (0u64, 0u64);
    for e in &doc.entities {
        if doc.owner_kind(e) != Some(BlockKind::Model) {
            continue;
        }
        of += 1;
        if let (Some(b), q) = (e.bbox, e.bbox_q) {
            if q != BboxQ::None {
                acc.add_box(b);
                over += 1;
            }
        }
    }
    (acc.get(), over, of)
}

fn sentinel(v: f64) -> bool {
    !v.is_finite() || v.abs() >= 1e19
}

/// The extents: the file's, the computed, and whether the file's are stale or the sentinel.
pub fn extents(doc: &Doc) -> Value {
    let (mut computed, over, of) = computed_extents(doc);
    let header = doc.header_extents;
    if mutant::active("ExtentsFromHeader") {
        computed = header.map(|(lo, hi)| [lo[0], lo[1], hi[0], hi[1]]);
    }
    let is_sentinel = header.is_some_and(|(lo, hi)| {
        lo.iter().chain(hi.iter()).any(|v| sentinel(*v)) || lo[0] > hi[0] || lo[1] > hi[1]
    });
    let stale = match (header, computed) {
        (Some(_), _) if is_sentinel => true,
        (Some((lo, hi)), Some(c)) => {
            let tol = 1e-6 * (1.0 + c.iter().fold(0.0f64, |m, v| m.max(v.abs())));
            c[0] < lo[0] - tol || c[1] < lo[1] - tol || c[2] > hi[0] + tol || c[3] > hi[1] + tol
        }
        _ => false,
    };
    let header_v = match header {
        Some((lo, hi)) => Value::obj([
            ("min", Value::List(lo.iter().map(|v| f(*v)).collect())),
            ("max", Value::List(hi.iter().map(|v| f(*v)).collect())),
        ]),
        None => Value::Null,
    };
    let computed_v = match computed {
        Some(c) => Value::obj([
            ("min", pt([c[0], c[1]])),
            ("max", pt([c[2], c[3]])),
            ("over", Value::from(over)),
            ("of", Value::from(of)),
        ]),
        None => Value::Null,
    };
    Value::obj([
        ("header", header_v),
        ("computed", computed_v),
        ("header_stale", Value::from(header.is_some() && stale)),
        ("header_sentinel", Value::from(is_sentinel)),
    ])
}

/// Every count `drawing info` gives.
pub fn counts(doc: &Doc) -> Value {
    let mut by_kind: BTreeMap<&str, u64> = BTreeMap::new();
    let (mut model, mut paper, mut blocks) = (0u64, 0u64, 0u64);
    for e in &doc.entities {
        *by_kind.entry(&*e.kind).or_default() += 1;
        match doc.owner_kind(e) {
            Some(BlockKind::Model) => model += 1,
            Some(BlockKind::Layout) => paper += 1,
            Some(BlockKind::Block) => blocks += 1,
            None => {}
        }
    }
    let user = doc
        .blocks
        .iter()
        .filter(|b| b.kind == BlockKind::Block && !b.anon)
        .count();
    let anon = doc
        .blocks
        .iter()
        .filter(|b| b.kind == BlockKind::Block && b.anon)
        .count();
    Value::obj([
        ("entities", Value::from(doc.entities.len())),
        (
            "by_kind",
            Value::obj(by_kind.into_iter().map(|(k, n)| (k, Value::from(n)))),
        ),
        (
            "by_space",
            Value::obj([
                ("model", Value::from(model)),
                ("paper", Value::from(paper)),
                ("blocks", Value::from(blocks)),
            ]),
        ),
        ("layers", Value::from(doc.layers.len())),
        ("blocks", Value::from(user)),
        ("anon_blocks", Value::from(anon)),
    ])
}

/// Handle health.
pub fn handles(doc: &Doc) -> Value {
    let hx = |h: Option<u64>| h.map_or(Value::Null, |h| Value::from(crate::handle::norm(h)));
    Value::obj([
        ("mode", Value::from(doc.handles)),
        ("hmax", hx(doc.hmax)),
        ("handseed", hx(doc.handseed)),
        ("duplicates", Value::from(doc.duplicate_handles())),
        ("missing", Value::from(doc.missing_handles())),
    ])
}

/// How many entities the edit verbs can change, and why not the others.
pub fn editable(doc: &Doc) -> Value {
    let mut yes = 0u64;
    let mut no: BTreeMap<&str, u64> = BTreeMap::new();
    for e in &doc.entities {
        if e.edit == "1" {
            yes += 1;
        } else {
            *no.entry(e.edit).or_default() += 1;
        }
    }
    Value::obj([
        ("yes", Value::from(yes)),
        (
            "no",
            Value::obj(no.into_iter().map(|(k, n)| (k, Value::from(n)))),
        ),
    ])
}

/// `drawing info`: the shared facts, then `extra` (a format's own, as `encoding` details).
pub fn info(doc: &Doc, extra: Vec<(&'static str, Value)>) -> ReadOut {
    let mut v: Vec<(String, Value)> = vec![
        ("format".into(), Value::from(doc.format)),
        ("acadver".into(), Value::from(doc.version.as_str())),
        (
            "release".into(),
            doc.release.map_or(Value::Null, Value::from),
        ),
        ("encoding".into(), Value::from(doc.encoding.as_str())),
        ("eol".into(), doc.eol.map_or(Value::Null, Value::from)),
        ("units".into(), units_of(doc)),
        ("extents".into(), extents(doc)),
        ("counts".into(), counts(doc)),
        ("handles".into(), handles(doc)),
        ("editable".into(), editable(doc)),
        (
            "writer".into(),
            Value::obj([
                ("mark", opt_str(&doc.writer_mark)),
                ("policy", Value::from("unproven")),
            ]),
        ),
    ];
    v.extend(extra.into_iter().map(|(k, x)| (k.to_owned(), x)));
    let data = Value::Obj(v);
    let stdout = format!(
        "{} {} ({}), {} entities, {} layers, units {}, handles {}\n",
        doc.format,
        doc.version,
        doc.release.unwrap_or("unknown release"),
        doc.entities.len(),
        doc.layers.len(),
        doc.units.word,
        doc.handles
    );
    ReadOut {
        stdout,
        data,
        spec: ("cad.drawing", 1),
        next: None,
    }
}

const LAYER_LIST: Spec = Spec {
    verb: "layer list",
    valued: &[],
    bare: &["--unused"],
    words: false,
};

/// `layer list [--unused]`.
pub fn layer_list(doc: &Doc, a: &[&str]) -> Result<ReadOut, CadError> {
    let args = LAYER_LIST.parse(a)?;
    let mut ents = vec![(0u64, 0u64); doc.layers.len()];
    for e in &doc.entities {
        if let Some(i) = doc.layer_named(&e.layer) {
            ents[i].0 += 1;
            if doc.owner_kind(e) == Some(BlockKind::Model) {
                ents[i].1 += 1;
            }
        }
    }
    let mut rows = Vec::new();
    let mut out = String::new();
    for (i, l) in doc.layers.iter().enumerate() {
        if args.has("--unused") && ents[i].0 > 0 {
            continue;
        }
        let address = l.handle.map(crate::handle::key);
        out.push_str(&format!(
            "{}\t{}\t{}\n",
            address.as_deref().unwrap_or("-"),
            l.name,
            ents[i].0
        ));
        rows.push(Value::obj([
            ("address", address.map_or(Value::Null, Value::from)),
            ("name", Value::from(l.name.as_str())),
            (
                "color",
                l.color.map_or(Value::Null, |c| Value::from(c.abs())),
            ),
            ("on", Value::from(l.on())),
            ("frozen", Value::from(l.frozen)),
            ("locked", Value::from(l.locked)),
            ("plot", Value::from(l.plot)),
            ("ltype", opt_str(&l.ltype)),
            ("lw", opt_str(&l.lw)),
            ("xref", Value::from(l.xref)),
            ("ents", Value::from(ents[i].0)),
            ("ents_model", Value::from(ents[i].1)),
        ]));
    }
    Ok(ReadOut {
        stdout: out,
        data: Value::obj([("units", units_of(doc)), ("layers", Value::List(rows))]),
        spec: ("cad.layer-list", 1),
        next: None,
    })
}

const BLOCK_LIST: Spec = Spec {
    verb: "block list",
    valued: &[],
    bare: &["--unused", "--anon"],
    words: false,
};

/// Each block's insert counts: direct from model space, from paper space, inside blocks, and
/// nested from model space (every placement an expansion of model space makes). The nested
/// counts stop after [`NESTED_VISITS_MAX`] expansions (`capped`).
fn insert_counts(doc: &Doc) -> (Vec<[u64; 4]>, bool) {
    let n = doc.blocks.len();
    let mut c = vec![[0u64; 4]; n];
    // inserts inside each block: (target, how many)
    let mut inside: Vec<BTreeMap<usize, u64>> = vec![BTreeMap::new(); n];
    let mut from_model: BTreeMap<usize, u64> = BTreeMap::new();
    for e in &doc.entities {
        let Some(t) = e.block.as_ref().and_then(|b| doc.block_named(b)) else {
            continue;
        };
        match doc.owner_kind(e) {
            Some(BlockKind::Model) => {
                c[t][0] += 1;
                *from_model.entry(t).or_default() += 1;
            }
            Some(BlockKind::Layout) => c[t][1] += 1,
            Some(BlockKind::Block) => {
                c[t][2] += 1;
                if let Some(o) = e.owner {
                    *inside[o].entry(t).or_default() += 1;
                }
            }
            None => {}
        }
    }
    if mutant::active("NestedCountsDirectOnly") {
        for r in c.iter_mut() {
            r[3] = r[0];
        }
        return (c, false);
    }
    // expand from model space, depth-first, with multiplicities
    let mut visits = 0u64;
    let mut capped = false;
    let mut stack: Vec<(usize, u64, u32)> =
        from_model.into_iter().map(|(t, m)| (t, m, 0)).collect();
    while let Some((t, m, depth)) = stack.pop() {
        visits += 1;
        if visits > NESTED_VISITS_MAX || depth > 64 {
            capped = true;
            break;
        }
        c[t][3] = c[t][3].saturating_add(m);
        for (&u, &k) in &inside[t] {
            stack.push((u, m.saturating_mul(k), depth + 1));
        }
    }
    (c, capped)
}

/// `block list [--unused] [--anon]`: block definitions (not model space or layouts); anonymous
/// ones with `--anon`.
pub fn block_list(doc: &Doc, a: &[&str]) -> Result<ReadOut, CadError> {
    let args = BLOCK_LIST.parse(a)?;
    let (counts, capped) = insert_counts(doc);
    let mut ents = vec![0u64; doc.blocks.len()];
    // references drawn through a dynamic block's anonymous blocks, by the dynamic block
    let mut dynamic = vec![0u64; doc.blocks.len()];
    for e in &doc.entities {
        if let Some(o) = e.owner {
            ents[o] += 1;
        }
        if let Some(r) = doc.insert_block(e).and_then(|t| doc.blocks[t].dynamic_of) {
            dynamic[r] += 1;
        }
    }
    let mut rows = Vec::new();
    let mut out = String::new();
    for (i, b) in doc.blocks.iter().enumerate() {
        if b.kind != BlockKind::Block || (b.anon && !args.has("--anon")) {
            continue;
        }
        let [dm, dp, ib, nm] = counts[i];
        if args.has("--unused") && dm + dp + ib > 0 {
            continue;
        }
        let address = doc.block_address(i);
        out.push_str(&format!("{address}\t{}\t{dm}\t{nm}\n", b.name));
        rows.push(Value::obj([
            ("address", Value::from(address.as_str())),
            ("name", Value::from(b.name.as_str())),
            (
                "base",
                b.base.map_or(Value::Null, |p| {
                    Value::List(p.iter().map(|v| f(*v)).collect())
                }),
            ),
            ("anon", Value::from(b.anon)),
            ("xref", Value::from(b.xref)),
            ("path", opt_str(&b.path)),
            (
                "dynamic_of",
                b.dynamic_of
                    .map_or(Value::Null, |r| Value::from(doc.blocks[r].name.as_str())),
            ),
            (
                "inserts",
                Value::obj([
                    (
                        "direct",
                        Value::obj([("model", Value::from(dm)), ("paper", Value::from(dp))]),
                    ),
                    ("in_blocks", Value::from(ib)),
                    ("nested_model", Value::from(nm)),
                    ("dynamic", Value::from(dynamic[i])),
                ]),
            ),
            ("ents", Value::from(ents[i])),
        ]));
    }
    Ok(ReadOut {
        stdout: out,
        data: Value::obj([
            ("units", units_of(doc)),
            ("blocks", Value::List(rows)),
            ("nested_capped", Value::from(capped)),
        ]),
        spec: ("cad.block-list", 1),
        next: None,
    })
}

/// Where a find looks: model space (the default), paper space, everywhere, or one record.
enum Scope {
    Model,
    Paper,
    All,
    Record(usize),
}

fn scope(doc: &Doc, spec: &Spec, v: Option<&str>, default: Scope) -> Result<Scope, CadError> {
    match v {
        None => Ok(default),
        Some("model") => Ok(Scope::Model),
        Some("paper") => Ok(Scope::Paper),
        Some("all") => Ok(Scope::All),
        Some(a) => (0..doc.blocks.len())
            .find(|&i| doc.block_address(i).eq_ignore_ascii_case(a))
            .map(Scope::Record)
            .ok_or_else(|| {
                CadError::new(
                    crate::error::NOT_FOUND,
                    format!("--in {a}: no model space, layout or block has that address"),
                    format!(
                        "`block list` gives the blocks' addresses; or --in model|paper|all ({})",
                        spec.verb
                    ),
                )
            }),
    }
}

fn in_scope(doc: &Doc, s: &Scope, i: usize) -> bool {
    let e = &doc.entities[i];
    match s {
        Scope::All => true,
        Scope::Model => doc.owner_kind(e) == Some(BlockKind::Model),
        Scope::Paper => doc.owner_kind(e) == Some(BlockKind::Layout),
        Scope::Record(r) => e.owner == Some(*r),
    }
}

fn owner_word(doc: &Doc, i: usize) -> Value {
    match doc.entities[i].owner {
        Some(o) => match doc.blocks[o].kind {
            BlockKind::Model => Value::from("model"),
            _ => Value::from(doc.block_address(o)),
        },
        None => Value::Null,
    }
}

/// The page of `matches` after `after` (an address), at most `limit`; and whether more remain.
fn page(
    doc: &Doc,
    matches: &[usize],
    after: Option<&str>,
    limit: usize,
) -> Result<(Vec<usize>, bool), CadError> {
    let start = match after {
        None => 0,
        Some(a) => {
            let at = matches
                .iter()
                .position(|&i| doc.entity_address(i).eq_ignore_ascii_case(a))
                .ok_or_else(|| {
                    CadError::new(
                        crate::error::NOT_FOUND,
                        format!("--after {a}: that entity isn't among the matches in this version"),
                        "run the find again without --after",
                    )
                })?;
            at + 1
        }
    };
    let rest = &matches[start.min(matches.len())..];
    let take: Vec<usize> = rest.iter().take(limit).copied().collect();
    Ok((take, rest.len() > limit))
}

const ENTITY_FIND: Spec = Spec {
    verb: "entity find",
    valued: &[
        "--layer",
        "--kind",
        "--in",
        "--window",
        "--crossing",
        "--ltype",
        "--inside",
        "--crosses",
        "--after",
        "--limit",
    ],
    bare: &[],
    words: false,
};

/// `entity find [--layer L] [--kind K] [--in model|paper|all|ADDR] [--window x0,y0,x1,y1 |
/// --crossing x0,y0,x1,y1] [--after ADDR] [--limit N]`. A window takes what lies wholly inside;
/// a crossing what touches. With no filter it is refused: a find never returns everything.
pub fn entity_find(doc: &Doc, a: &[&str]) -> Result<ReadOut, CadError> {
    let sp = &ENTITY_FIND;
    let args = sp.parse(a)?;
    if [
        "--layer",
        "--kind",
        "--in",
        "--window",
        "--crossing",
        "--ltype",
        "--inside",
        "--crosses",
    ]
    .iter()
    .all(|f| args.get(f).is_none())
    {
        return Err(args::usage(
            sp,
            "entity find needs a filter: it never lists everything",
        ));
    }
    if args.get("--window").is_some() && args.get("--crossing").is_some() {
        return Err(args::usage(sp, "--window and --crossing: one of them"));
    }
    let window = args
        .get("--window")
        .map(|v| parse_box(sp, "--window", v))
        .transpose()?;
    let crossing = args
        .get("--crossing")
        .map(|v| parse_box(sp, "--crossing", v))
        .transpose()?;
    let limit = parse_limit(sp, args.get("--limit"), PAGE_DEFAULT, PAGE_MAX)?;
    let sc = scope(doc, sp, args.get("--in"), Scope::Model)?;
    let layer = args.get("--layer");
    let kind = args.get("--kind");
    let ltype = args.get("--ltype");
    let inside_of = args
        .get("--inside")
        .map(|a| boundary(doc, sp, a))
        .transpose()?;
    let crosses_of = args
        .get("--crosses")
        .map(|a| boundary(doc, sp, a))
        .transpose()?;
    let window_as_crossing = mutant::active("WindowAsCrossing");
    let matches: Vec<usize> = (0..doc.entities.len())
        .filter(|&i| {
            let e = &doc.entities[i];
            in_scope(doc, &sc, i)
                && layer.is_none_or(|l| e.layer.eq_ignore_ascii_case(l))
                && kind.is_none_or(|k| e.kind.eq_ignore_ascii_case(k))
                && window.is_none_or(|w| {
                    e.bbox.is_some_and(|b| {
                        if window_as_crossing {
                            touches(b, w)
                        } else {
                            inside(b, w)
                        }
                    })
                })
                && crossing.is_none_or(|c| e.bbox.is_some_and(|b| touches(b, c)))
                && ltype.is_none_or(|t| ltype_eff(doc, i).eq_ignore_ascii_case(t))
                && inside_of
                    .as_ref()
                    .is_none_or(|(p, poly)| *p != i && relate(doc, i, poly) == Relation::Inside)
                && crosses_of
                    .as_ref()
                    .is_none_or(|(p, poly)| *p != i && relate(doc, i, poly) == Relation::Crosses)
        })
        .collect();
    let (rows_i, more) = page(doc, &matches, args.get("--after"), limit)?;
    let mut out = String::new();
    let mut rows = Vec::new();
    for &i in &rows_i {
        let e = &doc.entities[i];
        let address = doc.entity_address(i);
        out.push_str(&format!(
            "{address}\t{}\t{}\t{}\n",
            e.kind,
            e.layer,
            e.bbox
                .map(crate::geom::box_text)
                .unwrap_or_else(|| "-".into())
        ));
        rows.push(Value::obj([
            ("address", Value::from(address.as_str())),
            ("kind", Value::from(&*e.kind)),
            ("layer", Value::from(&*e.layer)),
            ("owner", owner_word(doc, i)),
            ("bbox", e.bbox.map_or(Value::Null, bx)),
            ("bbox_q", Value::from(e.bbox_q.word())),
        ]));
    }
    let next = more.then(|| args.with_after(&doc.entity_address(*rows_i.last().expect("a page"))));
    if let Some(n) = &next {
        out.push_str(&format!("next: entity find {}\n", n.join(" ")));
    }
    Ok(ReadOut {
        stdout: out,
        data: Value::obj([
            ("units", units_of(doc)),
            ("rows", Value::List(rows)),
            ("total", Value::from(matches.len())),
        ]),
        spec: ("cad.entity-find", 1),
        next,
    })
}

const TEXT_FIND: Spec = Spec {
    verb: "text find",
    valued: &[
        "--contains",
        "--layer",
        "--in",
        "--near",
        "--within",
        "--after",
        "--limit",
    ],
    bare: &["--case"],
    words: false,
};

/// The longest text a find row gives, in bytes.
pub const TEXT_ROW_MAX: usize = 200;

/// `text find --contains S [--layer L] [--in model|paper|all|ADDR] [--case] [--after ADDR]
/// [--limit N]`: entities whose text holds `S`, case-insensitive unless `--case`; everywhere
/// unless `--in`.
pub fn text_find(doc: &Doc, a: &[&str]) -> Result<ReadOut, CadError> {
    let sp = &TEXT_FIND;
    let args = sp.parse(a)?;
    let needle = match args.get("--contains") {
        Some(s) if !s.is_empty() => s,
        _ => {
            return Err(args::usage(
                sp,
                "text find needs --contains S, a non-empty text",
            ))
        }
    };
    let case = args.has("--case");
    let low = needle.to_lowercase();
    let limit = parse_limit(sp, args.get("--limit"), PAGE_DEFAULT, PAGE_MAX)?;
    let sc = scope(doc, sp, args.get("--in"), Scope::All)?;
    let layer = args.get("--layer");
    let near = match (args.get("--near"), args.get("--within")) {
        (None, None) => None,
        (Some(a), Some(d)) => {
            let d: f64 = d
                .parse()
                .ok()
                .filter(|d: &f64| d.is_finite() && *d >= 0.0)
                .ok_or_else(|| args::usage(sp, "--within takes a distance, a number from 0"))?;
            let i = (0..doc.entities.len())
                .find(|&i| doc.entity_address(i).eq_ignore_ascii_case(a))
                .ok_or_else(|| {
                    CadError::new(
                        crate::error::NOT_FOUND,
                        format!("--near {a}: no entity has that address"),
                        "`entity find` gives entities' addresses",
                    )
                })?;
            Some((doc.outline(i), d))
        }
        _ => return Err(args::usage(sp, "--near and --within go together")),
    };
    let matches: Vec<usize> = (0..doc.entities.len())
        .filter(|&i| {
            let e = &doc.entities[i];
            in_scope(doc, &sc, i)
                && layer.is_none_or(|l| e.layer.eq_ignore_ascii_case(l))
                && near.as_ref().is_none_or(|((pts, closed), d)| {
                    e.at.is_some_and(|p| distance(p, pts, *closed) <= *d)
                })
                && e.text.as_deref().is_some_and(|t| {
                    if case {
                        t.contains(needle)
                    } else {
                        t.to_lowercase().contains(&low)
                    }
                })
        })
        .collect();
    let (rows_i, more) = page(doc, &matches, args.get("--after"), limit)?;
    let mut out = String::new();
    let mut rows = Vec::new();
    for &i in &rows_i {
        let e = &doc.entities[i];
        let address = doc.entity_address(i);
        let t = e.text.as_deref().unwrap_or("");
        let shown = k2_mpc_base::index::cut_text(t, TEXT_ROW_MAX as u32).0;
        out.push_str(&format!(
            "{address}\t{}\t{}\n",
            e.kind,
            shown.replace('\n', " | ")
        ));
        rows.push(Value::obj([
            ("address", Value::from(address.as_str())),
            ("kind", Value::from(&*e.kind)),
            ("layer", Value::from(&*e.layer)),
            ("owner", owner_word(doc, i)),
            ("at", e.at.map_or(Value::Null, pt)),
            ("text", Value::from(shown)),
        ]));
    }
    let next = more.then(|| args.with_after(&doc.entity_address(*rows_i.last().expect("a page"))));
    if let Some(n) = &next {
        out.push_str(&format!("next: text find {}\n", n.join(" ")));
    }
    Ok(ReadOut {
        stdout: out,
        data: Value::obj([
            ("units", units_of(doc)),
            ("rows", Value::List(rows)),
            ("total", Value::from(matches.len())),
        ]),
        spec: ("cad.text-find", 1),
        next,
    })
}

/// A number in a row, as text: the shortest form that reads back the same.
pub fn num_text(x: f64) -> String {
    num(x)
}

/// How an entity's outline lies against a closed boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// Wholly inside (its outline's points inside or on the boundary, no edge crossed).
    Inside,
    /// Touching or crossing the boundary.
    Crosses,
    /// Wholly outside.
    Outside,
}

/// The boundary address `a` names: a closed polyline, as its index and vertices.
fn boundary(doc: &Doc, sp: &Spec, a: &str) -> Result<(usize, Vec<[f64; 2]>), CadError> {
    let i = (0..doc.entities.len())
        .find(|&i| doc.entity_address(i).eq_ignore_ascii_case(a))
        .ok_or_else(|| {
            CadError::new(
                crate::error::NOT_FOUND,
                format!("{a}: no entity has that address"),
                format!("`entity find` gives entities' addresses ({})", sp.verb),
            )
        })?;
    match doc.entities[i].extra.as_ref() {
        Some(x) if x.closed && x.verts.len() >= 3 => {
            if mutant::active("BoxNotPolygon") {
                let b = doc.entities[i].bbox.unwrap_or([0.0; 4]);
                return Ok((
                    i,
                    vec![[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]],
                ));
            }
            Ok((i, x.verts.clone()))
        }
        _ => Err(CadError::new(
            crate::error::NOT_A_BOUNDARY,
            format!("{a} isn't a closed polyline"),
            "name a closed polyline: `entity find --kind LWPOLYLINE` lists them",
        )),
    }
}

/// How entity `i` lies against polygon `poly`.
pub fn relate(doc: &Doc, i: usize, poly: &[[f64; 2]]) -> Relation {
    let (pts, closed) = doc.outline(i);
    relate_points(&pts, closed, poly)
}

/// How an outline (points, closed or not) lies against polygon `poly`.
pub fn relate_points(pts: &[[f64; 2]], closed: bool, poly: &[[f64; 2]]) -> Relation {
    use crate::geom::{edges, in_polygon, segments, segments_meet};
    if pts.is_empty() {
        return Relation::Outside;
    }
    let segs = segments(pts, closed);
    let meets = segs
        .iter()
        .any(|(a, b)| edges(poly).any(|(c, d)| segments_meet(*a, *b, c, d)))
        || (segs.is_empty() && edges(poly).any(|(c, d)| segments_meet(pts[0], pts[0], c, d)));
    let ins = pts.iter().filter(|p| in_polygon(**p, poly)).count();
    if meets && !(ins == pts.len() && !proper_cross(&segs, poly)) {
        Relation::Crosses
    } else if ins == pts.len() {
        Relation::Inside
    } else if ins > 0 {
        Relation::Crosses
    } else {
        Relation::Outside
    }
}

/// Whether a segment passes through the polygon's boundary from one side to the other (not
/// merely touching it from inside).
fn proper_cross(segs: &[([f64; 2], [f64; 2])], poly: &[[f64; 2]]) -> bool {
    use crate::geom::in_polygon;
    segs.iter().any(|(a, b)| {
        // a segment wholly inside keeps its midpoints inside; one that leaves has a point out
        (1..8).any(|k| {
            let t = f64::from(k) / 8.0;
            !in_polygon([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])], poly)
        })
    })
}

/// The shortest distance from `p` to an outline (0 inside a closed one).
fn distance(p: [f64; 2], pts: &[[f64; 2]], closed: bool) -> f64 {
    use crate::geom::{in_polygon, segments, to_segment};
    if mutant::active("NearIgnoresDistance") {
        return 0.0;
    }
    if closed && in_polygon(p, pts) {
        return 0.0;
    }
    let segs = segments(pts, closed);
    if segs.is_empty() {
        return pts.first().map_or(f64::INFINITY, |q| {
            ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()
        });
    }
    segs.iter()
        .map(|(a, b)| to_segment(p, *a, *b))
        .fold(f64::INFINITY, f64::min)
}

/// Entity `i`'s effective linetype: its own, else its layer's (`BYLAYER`), else `CONTINUOUS`;
/// `BYBLOCK` stays for an entity of a block definition (each reference decides it) and is
/// `CONTINUOUS` in model space or a layout.
pub fn ltype_eff(doc: &Doc, i: usize) -> String {
    let e = &doc.entities[i];
    let own = e.props.as_ref().and_then(|p| p.ltype.as_deref());
    let by_layer = || {
        doc.layer_named(&e.layer)
            .and_then(|l| doc.layers[l].ltype.clone())
            .filter(|t| !t.eq_ignore_ascii_case("BYLAYER") && !t.eq_ignore_ascii_case("BYBLOCK"))
            .unwrap_or_else(|| "CONTINUOUS".into())
    };
    match own {
        None => {
            if mutant::active("ByLayerUnresolved") {
                "BYLAYER".into()
            } else {
                by_layer()
            }
        }
        Some(t) if t.eq_ignore_ascii_case("BYLAYER") => {
            if mutant::active("ByLayerUnresolved") {
                "BYLAYER".into()
            } else {
                by_layer()
            }
        }
        Some(t) if t.eq_ignore_ascii_case("BYBLOCK") => {
            if doc.owner_kind(e) == Some(BlockKind::Block) {
                "BYBLOCK".into()
            } else {
                "CONTINUOUS".into()
            }
        }
        Some(t) => t.to_owned(),
    }
}

const ATTRIB_FIND: Spec = Spec {
    verb: "attrib find",
    valued: &["--tag", "--value", "--block", "--in", "--after", "--limit"],
    bare: &[],
    words: false,
};

/// `attrib find [--tag T] [--value V] [--block NAME] [--in model|paper|all|ADDR] [--after
/// ADDR] [--limit N]`: block references' attributes, typed: the tag and the block name match
/// without case, the value exactly. Each row's address is the attribute's own handle. A
/// reference drawn through a dynamic block's anonymous block goes by the dynamic block's name
/// (`block`), the anonymous one beside it (`anon_block`); `--block` matches either.
pub fn attrib_find(doc: &Doc, a: &[&str]) -> Result<ReadOut, CadError> {
    let sp = &ATTRIB_FIND;
    let args = sp.parse(a)?;
    if ["--tag", "--value", "--block"]
        .iter()
        .all(|f| args.get(f).is_none())
    {
        return Err(args::usage(
            sp,
            "attrib find needs --tag, --value or --block",
        ));
    }
    let limit = parse_limit(sp, args.get("--limit"), PAGE_DEFAULT, PAGE_MAX)?;
    let sc = scope(doc, sp, args.get("--in"), Scope::All)?;
    let (tag, value, block) = (args.get("--tag"), args.get("--value"), args.get("--block"));
    let mut hits: Vec<(usize, usize)> = Vec::new();
    for (i, e) in doc.entities.iter().enumerate() {
        let Some(x) = e.extra.as_ref() else { continue };
        if !in_scope(doc, &sc, i)
            || block.is_some_and(|b| {
                !(e.block
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(b))
                    || doc.block_name(e).is_some_and(|n| n.eq_ignore_ascii_case(b)))
            })
        {
            continue;
        }
        for (k, at) in x.attribs.iter().enumerate() {
            if tag.is_none_or(|t| at.tag.eq_ignore_ascii_case(t))
                && value.is_none_or(|v| &*at.value == v)
            {
                hits.push((i, k));
            }
        }
    }
    let addr = |(i, k): (usize, usize)| -> String {
        let at = &doc.entities[i].extra.as_ref().expect("attribs").attribs[k];
        at.handle
            .map(crate::handle::key)
            .unwrap_or_else(|| format!("{}/{}", doc.entity_address(i), k + 1))
    };
    let start = match args.get("--after") {
        None => 0,
        Some(a) => {
            hits.iter()
                .position(|h| addr(*h).eq_ignore_ascii_case(a))
                .ok_or_else(|| {
                    CadError::new(
                        crate::error::NOT_FOUND,
                        format!(
                            "--after {a}: that attribute isn't among the matches in this version"
                        ),
                        "run the find again without --after",
                    )
                })?
                + 1
        }
    };
    let page_hits: Vec<(usize, usize)> = hits.iter().skip(start).take(limit).copied().collect();
    let more = hits.len() > start + page_hits.len();
    let mut out = String::new();
    let mut rows = Vec::new();
    for &(i, k) in &page_hits {
        let e = &doc.entities[i];
        let at = &e.extra.as_ref().expect("attribs").attribs[k];
        let value = if mutant::active("AttribValueAsText") {
            format!("{}={}", at.tag, at.value)
        } else {
            at.value.to_string()
        };
        let a = addr((i, k));
        out.push_str(&format!(
            "{a}\t{}\t{}\t{value}\n",
            doc.entity_address(i),
            at.tag
        ));
        rows.push(Value::obj([
            ("attrib", Value::from(a.as_str())),
            ("insert", Value::from(doc.entity_address(i).as_str())),
            ("block", doc.block_name(e).map_or(Value::Null, Value::from)),
            ("anon_block", anon_block(doc, e)),
            ("tag", Value::from(&*at.tag)),
            ("value", Value::from(value)),
            ("at", at.at.map_or(Value::Null, pt)),
            ("layer", Value::from(&*e.layer)),
            ("owner", owner_word(doc, i)),
        ]));
    }
    let next = more.then(|| args.with_after(&addr(*page_hits.last().expect("a page"))));
    if let Some(n) = &next {
        out.push_str(&format!("next: attrib find {}\n", n.join(" ")));
    }
    Ok(ReadOut {
        stdout: out,
        data: Value::obj([
            ("units", units_of(doc)),
            ("rows", Value::List(rows)),
            ("total", Value::from(hits.len())),
        ]),
        spec: ("cad.attrib-find", 1),
        next,
    })
}

/// The anonymous block a reference names, when it stands for a dynamic block; else null.
fn anon_block(doc: &Doc, e: &crate::model::Entity) -> Value {
    match (e.block.as_deref(), doc.block_name(e)) {
        (Some(w), Some(n)) if w != n => Value::from(w),
        _ => Value::Null,
    }
}

const BLOCK_SHOW: Spec = Spec {
    verb: "block show",
    valued: &[],
    bare: &["--attribs"],
    words: true,
};

/// `block show ADDR [--attribs]`: one block reference: its block, where it is, and with
/// `--attribs` its attributes in the file's order.
pub fn block_show(doc: &Doc, a: &[&str]) -> Result<ReadOut, CadError> {
    let sp = &BLOCK_SHOW;
    let args = sp.parse(a)?;
    let [addr] = args.words.as_slice() else {
        return Err(args::usage(
            sp,
            "block show takes one block reference's address",
        ));
    };
    let i = (0..doc.entities.len())
        .find(|&i| {
            doc.entity_address(i).eq_ignore_ascii_case(addr) && doc.entities[i].block.is_some()
        })
        .ok_or_else(|| {
            CadError::new(
                crate::error::NOT_FOUND,
                format!("{addr}: no block reference has that address"),
                "`entity find --kind INSERT` lists block references",
            )
        })?;
    let e = &doc.entities[i];
    let mut attribs: Vec<Value> = Vec::new();
    let mut out = format!(
        "{}\t{}\n",
        doc.entity_address(i),
        doc.block_name(e).unwrap_or("")
    );
    if args.has("--attribs") {
        let mut list: Vec<&crate::model::Attrib> = e
            .extra
            .as_ref()
            .map(|x| x.attribs.iter().collect())
            .unwrap_or_default();
        if mutant::active("AttribsOutOfOrder") {
            list.reverse();
        }
        for at in list {
            out.push_str(&format!("  {}\t{}\n", at.tag, at.value));
            attribs.push(Value::obj([
                (
                    "attrib",
                    at.handle
                        .map_or(Value::Null, |h| Value::from(crate::handle::key(h))),
                ),
                ("tag", Value::from(&*at.tag)),
                ("value", Value::from(&*at.value)),
                ("at", at.at.map_or(Value::Null, pt)),
            ]));
        }
    }
    Ok(ReadOut {
        stdout: out,
        data: Value::obj([
            ("units", units_of(doc)),
            ("insert", Value::from(doc.entity_address(i).as_str())),
            ("block", doc.block_name(e).map_or(Value::Null, Value::from)),
            ("anon_block", anon_block(doc, e)),
            ("at", e.at.map_or(Value::Null, pt)),
            ("layer", Value::from(&*e.layer)),
            ("owner", owner_word(doc, i)),
            (
                "attribs",
                if args.has("--attribs") {
                    Value::List(attribs)
                } else {
                    Value::Null
                },
            ),
        ]),
        spec: ("cad.block-show", 1),
        next: None,
    })
}

const ENTITY_PROPS: Spec = Spec {
    verb: "entity props",
    valued: &[],
    bare: &[],
    words: true,
};

/// `entity props ADDR…`: each entity's display properties as written, and its effective
/// linetype (resolved through its layer).
pub fn entity_props(doc: &Doc, a: &[&str]) -> Result<ReadOut, CadError> {
    let sp = &ENTITY_PROPS;
    let args = sp.parse(a)?;
    if args.words.is_empty() {
        return Err(args::usage(
            sp,
            "entity props takes one or more entity addresses",
        ));
    }
    let mut rows = Vec::new();
    let mut out = String::new();
    for w in &args.words {
        let i = (0..doc.entities.len())
            .find(|&i| doc.entity_address(i).eq_ignore_ascii_case(w))
            .ok_or_else(|| {
                CadError::new(
                    crate::error::NOT_FOUND,
                    format!("{w}: no entity has that address"),
                    "`entity find` gives entities' addresses",
                )
            })?;
        let e = &doc.entities[i];
        let p = e.props.as_deref();
        let get = |f: fn(&crate::model::Props) -> &Option<crate::model::Shared>| {
            p.and_then(|p| f(p).as_deref())
                .map_or(Value::Null, Value::from)
        };
        let eff = ltype_eff(doc, i);
        out.push_str(&format!(
            "{}\t{}\t{}\t{eff}\n",
            doc.entity_address(i),
            e.kind,
            e.layer
        ));
        rows.push(Value::obj([
            ("address", Value::from(doc.entity_address(i).as_str())),
            ("kind", Value::from(&*e.kind)),
            ("layer", Value::from(&*e.layer)),
            ("color", get(|p| &p.color)),
            ("rgb", get(|p| &p.rgb)),
            ("ltype", get(|p| &p.ltype)),
            ("ltype_eff", Value::from(eff)),
            ("lw", get(|p| &p.lw)),
            ("alpha", get(|p| &p.alpha)),
        ]));
    }
    Ok(ReadOut {
        stdout: out,
        data: Value::obj([("rows", Value::List(rows))]),
        spec: ("cad.entity-props", 1),
        next: None,
    })
}
