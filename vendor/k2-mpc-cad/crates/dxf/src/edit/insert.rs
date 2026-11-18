//! `block insert`: a new block reference, with an attribute for each of its block's attribute
//! definitions that isn't constant, placed as the reference places its block's geometry (the
//! definition's points from the block's base, scaled, turned, then put at the reference's
//! point; heights scaled; rotations turned), each attribute's value from `--attr TAG=VALUE` or
//! its definition's default. A definition on layer `0` puts its attribute on the reference's
//! layer.
//!
//! Refused, before any plan: a block that is missing, an external reference, a layout,
//! anonymous or annotative (`DXF-E051`), a dynamic block (`DXF-E044`), an `--attr` tag the
//! block doesn't define as settable (`DXF-E047`), a definition whose value is a field
//! (`DXF-E048`) or that is multi-line (`DXF-E049`).

use std::collections::{BTreeMap, HashMap};

use k2_mpc_cad_model::args::{self, Spec};
use k2_mpc_cad_model::{BlockKind, Doc};

use super::check::{Intent, Want};
use super::ops::{data_pairs, field_driven, first, int_of, own, place, props, raise_seed, xdicts};
use super::write::Writer;
use super::xform::turn_vector;
use super::{encode_text, num, rules, Cx, Out};
use crate::error::{
    CadError, ATTRIB_MISSING, ATTRIB_MULTILINE, BLOCK_NOT_INSERTABLE, DYNAMIC_GEOMETRY,
};
use crate::structure::{Structure, Unit};
use crate::{codes, handle, mutant};

const BLOCK_INSERT: Spec = Spec {
    verb: "block insert",
    valued: &[
        "--block", "--at", "--unit", "--rotate", "--scale", "--layer", "--color", "--ltype", "--in",
    ],
    bare: &[],
    words: true,
};

fn not_insertable(why: &str, what: &str) -> CadError {
    CadError::new(
        BLOCK_NOT_INSERTABLE,
        format!("{what} can't be placed by block insert ({why})"),
        "`block list` lists the blocks a reference can name",
    )
}

/// Whether unit `u` carries the annotative mark set (`1001 AcadAnnotative`, then
/// `AnnotativeData` with its version and its flag `1`). A mark of another shape is read as set:
/// it is refused rather than placed wrong.
fn annotative(s: &Structure<'_>, u: &Unit) -> bool {
    if mutant::active("AnnotativeIgnored") {
        return false;
    }
    let mut i = u.first + 1;
    while i < u.end {
        if s.tok.pairs[i].code == 1001 && s.text(i).trim().eq_ignore_ascii_case("AcadAnnotative") {
            if mutant::active("AnnotativeReadLoose") {
                return true;
            }
            let ints: Vec<String> = (i + 1..u.end)
                .take_while(|&p| s.tok.pairs[p].code != 1001)
                .filter(|&p| s.tok.pairs[p].code == 1070)
                .map(|p| s.text(p).trim().to_owned())
                .collect();
            return ints.get(1).is_none_or(|f| f != "0");
        }
        i += 1;
    }
    false
}

/// Whether a block record's extension dictionary marks a dynamic block (`ACAD_ENHANCEDBLOCK`).
fn enhanced(s: &Structure<'_>, objects: &HashMap<u64, usize>, u: &Unit) -> bool {
    xdicts(s, u).into_iter().any(|h| {
        objects.get(&h).is_some_and(|&o| {
            let ou = &s.objects[o];
            (ou.first + 1..ou.end).any(|p| {
                s.tok.pairs[p].code == 3
                    && s.text(p).trim().eq_ignore_ascii_case("ACAD_ENHANCEDBLOCK")
            })
        })
    })
}

/// What a block's own marks say: whether it is dynamic (a dynamic block's definition or one of
/// its drawn forms, or its record's extension dictionary marks it), and whether it is
/// annotative (its record, its `BLOCK` or one of its settable attribute definitions carries
/// the mark). `block insert` refuses by these, and the draw header states them, so the two
/// can't disagree.
pub(crate) struct Marks {
    pub dynamic: bool,
    pub annotative: bool,
}

/// The block record `b` of `doc` (read from `s`, `objects` its object index): its [`Marks`].
pub(crate) fn marks(
    s: &Structure<'_>,
    doc: &Doc,
    objects: &HashMap<u64, usize>,
    b: usize,
) -> Marks {
    let rec = &doc.blocks[b];
    let entry = record_entry(s, rec.handle);
    let dynamic = rec.dynamic_of.is_some()
        || doc.blocks.iter().any(|x| x.dynamic_of == Some(b))
        || entry.is_some_and(|e| enhanced(s, objects, e));
    let def = definition(s, &rec.name);
    let annotative = entry.is_some_and(|e| annotative(s, e))
        || def.is_some_and(|d| {
            annotative(s, &d.head)
                || d.ents.iter().any(|&k| {
                    let u = &s.entities[k].head;
                    u.kind == "ATTDEF" && int_of(s, u, 70) & 2 == 0 && annotative(s, u)
                })
        });
    Marks {
        dynamic,
        annotative,
    }
}

/// The block record table entry with handle `h`.
fn record_entry<'s>(s: &'s Structure<'_>, h: Option<u64>) -> Option<&'s Unit> {
    s.table("BLOCK_RECORD").and_then(|t| {
        t.entries
            .iter()
            .find(|e| own(s, e).is_some() && own(s, e) == h)
    })
}

/// The block definition named `name`.
pub(crate) fn definition<'s>(
    s: &'s Structure<'_>,
    name: &str,
) -> Option<&'s crate::structure::BlockDef> {
    s.blocks
        .iter()
        .find(|d| first(s, &d.head, 2).is_some_and(|p| s.text(p).trim().eq_ignore_ascii_case(name)))
}

/// A point of the block (`x, y` as written) placed: from the block's base, scaled by `scale`,
/// turned by `deg`, then put at `at`. Exact in decimal where it can be.
fn placed(
    x: &str,
    y: &str,
    base: &[String; 2],
    at: &[String; 2],
    scale: &str,
    deg: &str,
    eps: f64,
) -> Option<(String, String)> {
    if mutant::active("AttribsUnplaced") {
        return Some((x.to_owned(), y.to_owned()));
    }
    let dx = num::mul(&num::sub(x, &base[0])?, scale)?;
    let dy = num::mul(&num::sub(y, &base[1])?, scale)?;
    let (rx, ry) = turn_vector(&dx, &dy, deg, eps)?;
    Some((num::add(&at[0], &rx)?, num::add(&at[1], &ry)?))
}

pub(crate) fn block_insert(
    cx: &Cx<'_, '_>,
    w: &mut Writer<'_, '_>,
    it: &mut Intent,
    out: &mut Out,
    a: &[&str],
) -> Result<(), CadError> {
    let sp = &BLOCK_INSERT;
    // `--attr TAG=VALUE` may be given once per tag: taken out before the rest is read
    let mut attr: Vec<(String, String)> = Vec::new();
    let mut rest: Vec<&str> = Vec::new();
    let mut k = 0;
    while k < a.len() {
        if a[k] == "--attr" {
            let v = a
                .get(k + 1)
                .ok_or_else(|| args::usage(sp, "--attr needs TAG=VALUE"))?;
            let (t, v) = v
                .split_once('=')
                .filter(|(t, _)| !t.is_empty())
                .ok_or_else(|| args::usage(sp, "--attr takes TAG=VALUE"))?;
            if attr.iter().any(|(x, _)| x.eq_ignore_ascii_case(t)) {
                return Err(args::usage(sp, format!("--attr {t} given twice")));
            }
            attr.push((t.to_owned(), v.to_owned()));
            k += 2;
        } else {
            rest.push(a[k]);
            k += 1;
        }
    }
    let args = sp.parse(&rest)?;
    if !args.words.is_empty() {
        return Err(args::usage(
            sp,
            "block insert takes no words: name the block with --block",
        ));
    }
    let s = cx.s;
    let doc = cx.doc;
    let name = args
        .get("--block")
        .ok_or_else(|| args::usage(sp, "block insert needs --block NAME|@h:BLOCK"))?;
    let b = if name.starts_with("@h:") || name.starts_with("@H:") {
        handle::from_addr(name).and_then(|h| doc.blocks.iter().position(|r| r.handle == Some(h)))
    } else {
        doc.block_named(name)
    }
    .ok_or_else(|| not_insertable("missing", &format!("the block {name}")))?;
    let rec = &doc.blocks[b];
    let what = format!("the block {}", rec.name);
    if rec.xref {
        return Err(not_insertable("xref", &what));
    }
    if rec.kind != BlockKind::Block {
        return Err(not_insertable("layout", &what));
    }
    if rec.anon {
        return Err(not_insertable("anonymous", &what));
    }
    let m = marks(s, doc, &cx.objects, b);
    let entry = record_entry(s, rec.handle);
    if m.dynamic {
        return Err(CadError::new(
            DYNAMIC_GEOMETRY,
            format!("{what} is a dynamic block: its references are drawn from its parameters"),
            "insert it in the app, which sets its parameters",
        ));
    }
    let def = definition(s, &rec.name).ok_or_else(|| not_insertable("missing", &what))?;
    if m.annotative {
        return Err(not_insertable("annotative", &what));
    }
    // the block's base point, as written
    let base = [10, 20].map(|c| {
        first(s, &def.head, c)
            .map(|p| s.text(p).trim().to_owned())
            .unwrap_or_else(|| "0.0".into())
    });
    // another application's object in the definition: its reference would draw what we can't
    // read
    if def
        .ents
        .iter()
        .any(|&k| s.entities[k].head.kind == "ACAD_PROXY_ENTITY")
        && !mutant::active("ProxyBlockInserted")
    {
        return Err(CadError::new(
            crate::error::PROXY,
            format!("{what} holds another application's object (a proxy)"),
            "insert it in the application that made it",
        ));
    }
    let eps = super::xform::noise_eps(doc);
    // the attribute definitions that make attributes, in the file's order
    let mut defs: Vec<&Unit> = Vec::new();
    for &k in &def.ents {
        let u = &s.entities[k].head;
        if u.kind != "ATTDEF" {
            continue;
        }
        if int_of(s, u, 70) & 2 != 0 && !mutant::active("ConstantAttdefWritten") {
            continue;
        }
        if (u.first..u.end).any(|p| s.tok.pairs[p].code == 101) {
            return Err(CadError::new(
                ATTRIB_MULTILINE,
                format!("{what} has a multi-line attribute"),
                "insert it in the app",
            ));
        }
        if field_driven(cx, u) {
            return Err(CadError::new(
                crate::error::ATTRIB_DRIVEN,
                format!("an attribute of {what} holds a field the app computes"),
                "insert it in the app",
            ));
        }
        defs.push(u);
    }
    let tag = |u: &Unit| {
        first(s, u, 2)
            .map(|p| s.text(p).trim().to_owned())
            .unwrap_or_default()
    };
    for (t, _) in &attr {
        if !defs.iter().any(|u| tag(u).eq_ignore_ascii_case(t)) {
            return Err(CadError::new(
                ATTRIB_MISSING,
                format!("{what} defines no attribute {t} a reference can set"),
                format!(
                    "`block show` of a reference of {} --attribs lists its attributes",
                    rec.name
                ),
            ));
        }
    }
    cx.unit(args.get("--unit"), sp.verb)?;
    let at = {
        let v = args
            .get("--at")
            .ok_or_else(|| args::usage(sp, "block insert needs --at x,y"))?;
        let p: Vec<&str> = v.split(',').map(str::trim).collect();
        match p.as_slice() {
            [x, y] if num::parse(x).is_some() && num::parse(y).is_some() => {
                [(*x).to_owned(), (*y).to_owned()]
            }
            _ => return Err(args::usage(sp, "--at takes x,y: two numbers")),
        }
    };
    let scale = args.get("--scale").unwrap_or("1").trim().to_owned();
    if !num::parse(&scale).is_some_and(|x| x > 0.0) {
        return Err(args::usage(sp, "--scale takes a number above 0"));
    }
    let deg = args.get("--rotate").unwrap_or("0").trim().to_owned();
    let turned = num::turn(&deg).ok_or_else(|| args::usage(sp, "--rotate takes degrees"))?;
    let pr = props(cx, sp, &args)?;
    let layer = match pr.layer {
        Some(l) => l,
        None => {
            let current = s
                .var_value("$CLAYER", 8)
                .map(|v| v.into_owned())
                .unwrap_or_else(|| "0".into());
            doc.layers[rules::target_layer(doc, &current)?].name.clone()
        }
    };
    let (owner, before, paper) = place(cx, sp, args.get("--in"))?;
    let h = cx.hmax + 1;
    let block_name = first(s, &def.head, 2)
        .map(|p| s.tok.value(p).to_vec())
        .unwrap_or_else(|| rec.name.clone().into_bytes());
    let one = num::parse(&scale) == Some(1.0);
    let mut pairs: Vec<(i32, Vec<u8>)> = Vec::new();
    {
        let mut put = |c: i32, v: &[u8]| pairs.push((c, v.to_vec()));
        put(0, b"INSERT");
        put(5, cx.hex(h).as_bytes());
        put(330, cx.hex(owner).as_bytes());
        put(100, b"AcDbEntity");
        if paper {
            put(67, &cx.int(1));
        }
        put(8, layer.as_bytes());
        if let Some(lt) = pr.ltype.as_ref().filter(|l| *l != "ByLayer") {
            put(6, lt.as_bytes());
        }
        if let Some(c) = pr.color.filter(|c| *c != 256) {
            put(62, &cx.int(c));
        }
        if let Some(rgb) = pr.rgb {
            put(420, &cx.int(rgb));
        }
        put(100, b"AcDbBlockReference");
        if !defs.is_empty() {
            put(66, &cx.int(1));
        }
        put(2, &block_name);
        put(10, at[0].as_bytes());
        put(20, at[1].as_bytes());
        put(30, b"0.0");
        if !one {
            for c in [41, 42, 43] {
                put(c, scale.as_bytes());
            }
        }
        if num::parse(&turned) != Some(0.0) {
            put(50, turned.as_bytes());
        }
    }
    // one attribute per definition
    let mut next = h + 1;
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let bad = || {
        CadError::new(
            crate::error::VALUE_NOT_WRITABLE,
            format!("a number of {what}'s attribute definitions can't be placed"),
            "insert it in the app",
        )
    };
    for u in &defs {
        let t = tag(u);
        let value = attr
            .iter()
            .find(|(x, _)| x.eq_ignore_ascii_case(&t))
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| {
                first(s, u, 1)
                    .map(|p| crate::token::unescape(&s.text(p)).into_owned())
                    .unwrap_or_default()
            });
        let enc = encode_text(cx, &value)?;
        let ah = next;
        next += 1;
        it.minted.insert(ah);
        let data = data_pairs(s, u);
        let text = |p: usize| s.text(p).trim().to_owned();
        let mut ap: Vec<(i32, Vec<u8>)> = vec![
            (0, b"ATTRIB".to_vec()),
            (5, cx.hex(ah).into_bytes()),
            (330, cx.hex(h).into_bytes()),
        ];
        let mut k = 0;
        while k < data.len() {
            let p = data[k];
            let c = s.tok.pairs[p].code;
            let v = s.tok.value(p);
            k += 1;
            if codes::is_xdata(c) {
                break;
            }
            match c {
                5 | 330 => {} // its own: the attribute's are written above
                8 => {
                    let l = text(p);
                    let l = if l == "0" { layer.clone() } else { l };
                    ap.push((8, l.into_bytes()));
                }
                100 if text(p).eq_ignore_ascii_case("AcDbAttributeDefinition") => {
                    ap.push((100, b"AcDbAttribute".to_vec()));
                }
                3 => {} // the prompt is the definition's
                1 => {
                    ap.push((1, enc.clone()));
                    if num::parse(&turned) != Some(0.0) && first(s, u, 50).is_none() {
                        ap.push((50, turned.clone().into_bytes()));
                    }
                }
                10 | 11 => {
                    // the y and z that follow
                    let y = data
                        .get(k)
                        .copied()
                        .filter(|&q| s.tok.pairs[q].code == c + 10);
                    let Some(q) = y else {
                        ap.push((c, v.to_vec()));
                        continue;
                    };
                    k += 1;
                    let (nx, ny) = placed(&text(p), &text(q), &base, &at, &scale, &deg, eps)
                        .ok_or_else(bad)?;
                    ap.push((c, nx.into_bytes()));
                    ap.push((c + 10, ny.into_bytes()));
                    if let Some(z) = data
                        .get(k)
                        .copied()
                        .filter(|&r| s.tok.pairs[r].code == c + 20)
                    {
                        k += 1;
                        let nz = if one {
                            text(z)
                        } else {
                            num::mul(&text(z), &scale).ok_or_else(bad)?
                        };
                        ap.push((c + 20, nz.into_bytes()));
                    }
                }
                40 => {
                    let v = if one {
                        text(p)
                    } else {
                        num::mul(&text(p), &scale).ok_or_else(bad)?
                    };
                    ap.push((40, v.into_bytes()));
                }
                50 => {
                    let v = num::add(&text(p), &deg)
                        .and_then(|x| num::turn(&x))
                        .ok_or_else(bad)?;
                    ap.push((50, v.into_bytes()));
                }
                _ => ap.push((c, v.to_vec())),
            }
        }
        let nth = {
            let n = seen.entry(t.to_ascii_uppercase()).or_default();
            *n += 1;
            *n - 1
        };
        it.wants.push(Want::Attrib {
            h,
            tag: t.clone(),
            nth,
            value: value.clone(),
        });
        pairs.extend(ap);
    }
    if !defs.is_empty() {
        let sh = next;
        next += 1;
        it.minted.insert(sh);
        pairs.push((0, b"SEQEND".to_vec()));
        pairs.push((5, cx.hex(sh).into_bytes()));
        pairs.push((330, cx.hex(h).into_bytes()));
        pairs.push((100, b"AcDbEntity".to_vec()));
        pairs.push((8, layer.clone().into_bytes()));
    }
    w.insert_before(before, &pairs);
    // the block record's list of its references, when it keeps one
    if let Some(e) = entry {
        let mut open = None;
        for p in e.first + 1..e.end {
            if s.tok.pairs[p].code == 102 {
                let v = s.text(p);
                let v = v.trim();
                if v.eq_ignore_ascii_case("{BLKREFS") {
                    open = Some(p);
                } else if v == "}" && open.is_some() {
                    if !mutant::active("BlkrefsLeft") {
                        w.insert_before(p, &[(331, cx.hex(h).into_bytes())]);
                        if let Some(eh) = own(s, e) {
                            it.entries.insert(eh);
                        }
                    }
                    break;
                }
            }
        }
    }
    it.wants.push(Want::Layer {
        h,
        name: layer.clone(),
    });
    it.wants.push(Want::Pair {
        h,
        code: 10,
        nth: 0,
        value: at[0].clone().into_bytes(),
    });
    it.created.insert(h);
    raise_seed(cx, w, it, out, next - 1);
    out.word = "inserted";
    out.stdout = format!(
        "inserted {} ({}, {} attribute{})",
        handle::key(h),
        rec.name,
        defs.len(),
        if defs.len() == 1 { "" } else { "s" }
    );
    out.viewing = Some(handle::key(h));
    Ok(())
}
