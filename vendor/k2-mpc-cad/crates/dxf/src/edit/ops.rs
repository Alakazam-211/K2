//! The edit verbs themselves: each reads its arguments, asks the rules, and writes its ranges
//! through the [`Writer`], saying what it meant in the [`Intent`] for the self-check.

use std::collections::{BTreeSet, HashMap};

use k2_mpc_cad_model::args::{self, Spec};
use k2_mpc_cad_model::BlockKind;

use super::check::{Intent, Want};
use super::write::Writer;
use super::{encode_text, num, rules, Cx, Out, EDIT_MAX, OWNED_MAX, VERTICES_MAX};
use crate::error::{
    CadError, AMBIGUOUS, ATTRIB_DRIVEN, ATTRIB_MISSING, EDIT_TOO_MANY, HANDLES_OFF,
    IN_BLOCK_DEFINITION, NOT_EDITABLE, NOT_FOUND, REFERENCED, TABLE_ENTRY_MISSING,
    VALUE_NOT_WRITABLE,
};
use crate::structure::{Structure, Unit};
use crate::{codes, handle, mutant};

// ---------------------------------------------------------------------------------------------
// shared pieces
// ---------------------------------------------------------------------------------------------

/// The entity addresses a call names: at least one, at most [`EDIT_MAX`], each once.
fn addresses(sp: &Spec, words: &[String]) -> Result<Vec<String>, CadError> {
    if words.is_empty() {
        return Err(args::usage(
            sp,
            format!("{} takes one or more entity addresses (@h:HANDLE)", sp.verb),
        ));
    }
    if words.len() > EDIT_MAX {
        return Err(CadError::new(
            EDIT_TOO_MANY,
            format!(
                "{} entities, over the {EDIT_MAX} one call may change",
                words.len()
            ),
            "split the call",
        ));
    }
    let mut seen = BTreeSet::new();
    Ok(words
        .iter()
        .filter(|w| seen.insert(w.to_ascii_uppercase()))
        .cloned()
        .collect())
}

/// Pairs of unit `u` that hold its own data: past its `0` pair, outside its `102` groups,
/// before its extension data.
fn data_pairs(s: &Structure<'_>, u: &Unit) -> Vec<usize> {
    let mut out = Vec::new();
    let mut in102 = false;
    for p in u.first + 1..u.end {
        let c = s.tok.pairs[p].code;
        if codes::is_xdata(c) {
            break;
        }
        if c == 102 {
            in102 = s.tok.value(p).trim_ascii().starts_with(b"{");
            continue;
        }
        if !in102 {
            out.push(p);
        }
    }
    out
}

/// The first data pair of `u` with code `code`.
fn first(s: &Structure<'_>, u: &Unit, code: i32) -> Option<usize> {
    data_pairs(s, u)
        .into_iter()
        .find(|&p| s.tok.pairs[p].code == code)
}

fn int_of(s: &Structure<'_>, u: &Unit, code: i32) -> i64 {
    first(s, u, code)
        .and_then(|p| s.text(p).trim().parse::<i64>().ok())
        .unwrap_or(0)
}

/// The handles `u`'s `102 {ACAD_XDICTIONARY` group points at.
fn xdicts(s: &Structure<'_>, u: &Unit) -> Vec<u64> {
    let mut out = Vec::new();
    let mut inside = false;
    for p in u.first + 1..u.end {
        let c = s.tok.pairs[p].code;
        if c == 102 {
            let v = s.tok.value(p).trim_ascii();
            inside = v.eq_ignore_ascii_case(b"{ACAD_XDICTIONARY");
            continue;
        }
        if inside && c == 360 {
            if let Some(h) = handle::parse(s.tok.value(p)) {
                out.push(h);
            }
        }
    }
    out
}

/// A unit's own handle (`5`).
fn own(s: &Structure<'_>, u: &Unit) -> Option<u64> {
    (u.first + 1..u.end)
        .find(|&p| s.tok.pairs[p].code == 5)
        .and_then(|p| handle::parse(s.tok.value(p)))
}

/// A value line as replaced: the old one's spaces around it kept.
fn keep_spaces(old: &[u8], new: &str) -> Vec<u8> {
    let lead = old.iter().take_while(|b| **b == b' ').count();
    let trail = old.iter().rev().take_while(|b| **b == b' ').count();
    if lead == old.len() {
        return new.as_bytes().to_vec();
    }
    let mut v = old[..lead].to_vec();
    v.extend_from_slice(new.as_bytes());
    v.extend_from_slice(&old[old.len() - trail..]);
    v
}

/// An integer value replacing `old`: right-aligned to its width when it was aligned.
fn int_like(old: &[u8], n: i64) -> Vec<u8> {
    let w = old.len();
    if old.first() == Some(&b' ') {
        format!("{n:>w$}").into_bytes()
    } else {
        n.to_string().into_bytes()
    }
}

// ---------------------------------------------------------------------------------------------
// the common group: layer, linetype, colour, true colour
// ---------------------------------------------------------------------------------------------

/// The common group's codes, in the order the format writes them.
const ORDER: [i32; 16] = [
    67, 410, 8, 6, 347, 62, 370, 48, 60, 92, 310, 420, 430, 440, 390, 284,
];

fn rank(c: i32) -> Option<usize> {
    ORDER.iter().position(|&x| x == c)
}

/// The entity head's common group: the run of common-group pairs around its layer (`8`).
fn common(s: &Structure<'_>, head: &Unit) -> Result<(usize, usize), CadError> {
    let data = data_pairs(s, head);
    let Some(k) = data.iter().position(|&p| s.tok.pairs[p].code == 8) else {
        return Err(CadError::new(
            NOT_EDITABLE,
            "the entity names no layer (unknown)",
            "save the drawing again in the app",
        ));
    };
    let (mut a, mut b) = (k, k + 1);
    while a > 0 && rank(s.tok.pairs[data[a - 1]].code).is_some() && data[a - 1] + 1 == data[a] {
        a -= 1;
    }
    while b < data.len() && rank(s.tok.pairs[data[b]].code).is_some() && data[b] == data[b - 1] + 1
    {
        b += 1;
    }
    Ok((data[a], data[b - 1] + 1))
}

/// Code `c` in the common group `g` holds `value`: replaced where it is, else inserted in the
/// group's order.
fn set_code(w: &mut Writer<'_, '_>, s: &Structure<'_>, g: (usize, usize), c: i32, value: Vec<u8>) {
    if let Some(p) = (g.0..g.1).find(|&p| s.tok.pairs[p].code == c) {
        w.set_value(p, &value);
        return;
    }
    let r = rank(c).unwrap_or(ORDER.len());
    let at = if mutant::active("PairOrderIgnored") {
        g.1
    } else {
        (g.0..g.1)
            .find(|&p| rank(s.tok.pairs[p].code).is_some_and(|x| x > r))
            .unwrap_or(g.1)
    };
    w.insert_before(at, &[(c, value)]);
}

fn remove_code(w: &mut Writer<'_, '_>, s: &Structure<'_>, g: (usize, usize), c: i32) {
    if let Some(p) = (g.0..g.1).find(|&p| s.tok.pairs[p].code == c) {
        w.remove(p, p + 1);
    }
}

/// A colour flag: `bylayer` (256), `byblock` (0), or a number from 0 to 256.
fn color_arg(sp: &Spec, v: &str) -> Result<i64, CadError> {
    match v.to_ascii_lowercase().as_str() {
        "bylayer" => Ok(256),
        "byblock" => Ok(0),
        n => n
            .parse::<i64>()
            .ok()
            .filter(|n| (0..=256).contains(n))
            .ok_or_else(|| args::usage(sp, "--color takes 0 to 256, bylayer or byblock")),
    }
}

/// A true colour flag, `#rrggbb`: its value as the format writes it (`0x00RRGGBB` in decimal).
fn rgb_arg(sp: &Spec, v: &str) -> Result<i64, CadError> {
    v.strip_prefix('#')
        .filter(|h| h.len() == 6)
        .and_then(|h| i64::from_str_radix(h, 16).ok())
        .ok_or_else(|| args::usage(sp, "--rgb takes #rrggbb"))
}

/// A linetype flag: `bylayer`, `byblock`, or a linetype of the drawing's LTYPE table (as the
/// table spells it).
fn ltype_arg(s: &Structure<'_>, v: &str) -> Result<String, CadError> {
    match v.to_ascii_lowercase().as_str() {
        "bylayer" => return Ok("ByLayer".into()),
        "byblock" => return Ok("ByBlock".into()),
        _ => {}
    }
    s.table("LTYPE")
        .and_then(|t| {
            t.entries.iter().find_map(|e| {
                first(s, e, 2)
                    .map(|p| s.text(p).into_owned())
                    .filter(|n| n.eq_ignore_ascii_case(v))
            })
        })
        .ok_or_else(|| {
            CadError::new(
                TABLE_ENTRY_MISSING,
                "the drawing has no linetype of that name",
                "`entity props` and `layer list` show the linetypes in use",
            )
        })
}

/// The display flags of `entity set` and `entity add`, read and checked.
struct Props {
    layer: Option<String>,
    color: Option<i64>,
    rgb: Option<i64>,
    ltype: Option<String>,
}

fn props(cx: &Cx<'_, '_>, sp: &Spec, a: &args::Args) -> Result<Props, CadError> {
    let layer = match a.get("--layer") {
        Some(l) => Some(cx.doc.layers[rules::target_layer(cx.doc, l)?].name.clone()),
        None => None,
    };
    Ok(Props {
        layer,
        color: a.get("--color").map(|v| color_arg(sp, v)).transpose()?,
        rgb: a.get("--rgb").map(|v| rgb_arg(sp, v)).transpose()?,
        ltype: a.get("--ltype").map(|v| ltype_arg(cx.s, v)).transpose()?,
    })
}

// ---------------------------------------------------------------------------------------------
// attrib set
// ---------------------------------------------------------------------------------------------

const ATTRIB_SET: Spec = Spec {
    verb: "attrib set",
    valued: &["--tag", "--value", "--nth"],
    bare: &[],
    words: true,
};

/// The tag of attribute unit `u`.
fn tag_of(s: &Structure<'_>, u: &Unit) -> String {
    first(s, u, 2)
        .map(|p| s.text(p).into_owned())
        .unwrap_or_default()
}

/// Whether the attribute's value is a field: its extension dictionary holds `ACAD_FIELD`.
fn field_driven(cx: &Cx<'_, '_>, u: &Unit) -> bool {
    let s = cx.s;
    xdicts(s, u).into_iter().any(|h| {
        cx.objects.get(&h).is_some_and(|&o| {
            let ou = &s.objects[o];
            (ou.first + 1..ou.end)
                .any(|p| s.tok.pairs[p].code == 3 && s.text(p).eq_ignore_ascii_case("ACAD_FIELD"))
        })
    })
}

pub(crate) fn attrib_set(
    cx: &Cx<'_, '_>,
    w: &mut Writer<'_, '_>,
    it: &mut Intent,
    out: &mut Out,
    a: &[&str],
) -> Result<(), CadError> {
    let sp = &ATTRIB_SET;
    let args = sp.parse(a)?;
    let [addr] = args.words.as_slice() else {
        return Err(args::usage(
            sp,
            "attrib set takes one block reference's address",
        ));
    };
    let tag = args
        .get("--tag")
        .ok_or_else(|| args::usage(sp, "attrib set needs --tag"))?;
    let value = args
        .get("--value")
        .ok_or_else(|| args::usage(sp, "attrib set needs --value"))?;
    let nth = match args.get("--nth") {
        Some(n) => Some(
            n.parse::<usize>()
                .ok()
                .filter(|n| *n >= 1)
                .ok_or_else(|| args::usage(sp, "--nth takes 1 or more"))?,
        ),
        None => None,
    };
    let s = cx.s;
    let i = cx.entity(addr)?;
    if &*cx.doc.entities[i].kind != "INSERT" {
        return Err(CadError::new(
            NOT_FOUND,
            format!("{addr}: no block reference has that address"),
            "`entity find --kind INSERT` lists block references",
        ));
    }
    rules::entity(s, cx.doc, i)?;
    let eu = &s.entities[i];
    let tagged: Vec<&Unit> = eu
        .followers
        .iter()
        .filter(|u| u.kind == "ATTRIB" && tag_of(s, u).eq_ignore_ascii_case(tag))
        .collect();
    let missing = || {
        CadError::new(
            ATTRIB_MISSING,
            format!("{addr} has no attribute with that tag"),
            format!("`block show {addr} --attribs` lists its attributes"),
        )
    };
    let k = match (tagged.len(), nth) {
        (0, _) => return Err(missing()),
        (1, None) => 0,
        (n, None) => {
            return Err(CadError::new(
                AMBIGUOUS,
                format!("{n} attributes of {addr} have that tag"),
                "give --nth N (1 is the first, in the file's order)",
            ))
        }
        (n, Some(x)) if x > n => return Err(missing()),
        (_, Some(x)) => x - 1,
    };
    let u = tagged[k];
    if field_driven(cx, u) {
        return Err(CadError::new(
            ATTRIB_DRIVEN,
            format!("the attribute of {addr} holds a field the app computes"),
            "change the field's source in the app",
        ));
    }
    let enc = encode_text(cx, value)?;
    match first(s, u, 1) {
        Some(p) => w.set_value(p, &enc),
        None => match first(s, u, 2) {
            Some(p) => w.insert_before(p, &[(1, enc)]),
            None => return Err(missing()),
        },
    }
    let h = cx.doc.entities[i].handle.unwrap_or(0);
    it.changed.insert(h);
    it.wants.push(Want::Attrib {
        h,
        tag: tag.to_owned(),
        nth: k,
        value: value.to_owned(),
    });
    out.word = "set";
    out.stdout = format!("set attribute {tag} of {addr}");
    out.viewing = Some(handle::key(h));
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// entity set
// ---------------------------------------------------------------------------------------------

const ENTITY_SET: Spec = Spec {
    verb: "entity set",
    valued: &["--layer", "--color", "--rgb", "--ltype"],
    bare: &[],
    words: true,
};

pub(crate) fn entity_set(
    cx: &Cx<'_, '_>,
    w: &mut Writer<'_, '_>,
    it: &mut Intent,
    out: &mut Out,
    a: &[&str],
) -> Result<(), CadError> {
    let sp = &ENTITY_SET;
    let args = sp.parse(a)?;
    let addrs = addresses(sp, &args.words)?;
    let pr = props(cx, sp, &args)?;
    if pr.layer.is_none() && pr.color.is_none() && pr.rgb.is_none() && pr.ltype.is_none() {
        return Err(args::usage(
            sp,
            "entity set needs --layer, --color, --rgb or --ltype",
        ));
    }
    let s = cx.s;
    let mut idx = Vec::new();
    for addr in &addrs {
        let i = cx.entity(addr)?;
        rules::entity(s, cx.doc, i)?;
        idx.push(i);
    }
    for &i in &idx {
        let head = &s.entities[i].head;
        let h = cx.doc.entities[i].handle.unwrap_or(0);
        let g = common(s, head)?;
        let has = |c: i32| (g.0..g.1).find(|&p| s.tok.pairs[p].code == c);
        if let Some(l) = &pr.layer {
            set_code(w, s, g, 8, l.clone().into_bytes());
            it.wants.push(Want::Layer { h, name: l.clone() });
        }
        if let Some(lt) = &pr.ltype {
            let by_layer = lt == "ByLayer";
            match has(6) {
                None if by_layer => it.wants.push(Want::Ltype { h, value: None }),
                _ => {
                    set_code(w, s, g, 6, lt.clone().into_bytes());
                    it.wants.push(Want::Ltype {
                        h,
                        value: Some(lt.clone()),
                    });
                }
            }
        }
        if let Some(c) = pr.color {
            match has(62) {
                None if c == 256 => it.wants.push(Want::Color { h, value: None }),
                Some(p) => {
                    w.set_value(p, &int_like(s.tok.value(p), c));
                    it.wants.push(Want::Color {
                        h,
                        value: Some(c.to_string()),
                    });
                }
                None => {
                    set_code(w, s, g, 62, cx.int(c));
                    it.wants.push(Want::Color {
                        h,
                        value: Some(c.to_string()),
                    });
                }
            }
            if pr.rgb.is_none() && !mutant::active("TrueColourLeft") {
                remove_code(w, s, g, 420);
                remove_code(w, s, g, 430);
                it.wants.push(Want::Rgb { h, value: None });
            }
        }
        if let Some(rgb) = pr.rgb {
            match has(420) {
                Some(p) => w.set_value(p, &int_like(s.tok.value(p), rgb)),
                None => set_code(w, s, g, 420, cx.int(rgb)),
            }
            remove_code(w, s, g, 430);
            it.wants.push(Want::Rgb {
                h,
                value: Some(rgb.to_string()),
            });
        }
        it.changed.insert(h);
    }
    out.word = "changed";
    out.stdout = format!("changed {}", list(&addrs));
    out.viewing = addrs.first().cloned();
    Ok(())
}

fn list(addrs: &[String]) -> String {
    match addrs {
        [one] => one.clone(),
        many => format!("{} entities: {}", many.len(), many.join(", ")),
    }
}

// ---------------------------------------------------------------------------------------------
// entity move
// ---------------------------------------------------------------------------------------------

const ENTITY_MOVE: Spec = Spec {
    verb: "entity move",
    valued: &["--by", "--unit"],
    bare: &[],
    words: true,
};

/// Whether a text, attribute or attribute definition places itself by its second point (`11`):
/// any horizontal alignment but left, or (`73` for a text, `74` for the others) any vertical one.
fn aligned(s: &Structure<'_>, u: &Unit, kind: &str) -> bool {
    let v = if kind == "TEXT" { 73 } else { 74 };
    int_of(s, u, 72) != 0 || int_of(s, u, v) != 0
}

/// The x codes of the points unit `u` (of kind `kind`) holds: its y code is ten more.
fn point_codes(s: &Structure<'_>, u: &Unit, kind: &str) -> Vec<i32> {
    let vectors = mutant::active("VectorMovedAsPoint");
    match kind {
        "LINE" | "SPLINE" => vec![10, 11],
        "POINT" | "CIRCLE" | "ARC" | "INSERT" | "LWPOLYLINE" | "VERTEX" => vec![10],
        "ELLIPSE" | "MTEXT" => {
            if vectors {
                vec![10, 11]
            } else {
                vec![10]
            }
        }
        "TEXT" | "ATTDEF" | "ATTRIB" => {
            if aligned(s, u, kind) {
                vec![10, 11]
            } else {
                vec![10]
            }
        }
        "SOLID" | "TRACE" | "3DFACE" => vec![10, 11, 12, 13],
        _ => Vec::new(),
    }
}

/// A `dx,dy` flag: its two numbers as given.
fn delta(sp: &Spec, v: &str) -> Result<[String; 2], CadError> {
    let parts: Vec<&str> = v.split(',').map(str::trim).collect();
    match parts.as_slice() {
        [x, y] if num::parse(x).is_some() && num::parse(y).is_some() => {
            Ok([(*x).to_owned(), (*y).to_owned()])
        }
        _ => Err(args::usage(sp, "--by takes dx,dy: two numbers")),
    }
}

pub(crate) fn entity_move(
    cx: &Cx<'_, '_>,
    w: &mut Writer<'_, '_>,
    it: &mut Intent,
    out: &mut Out,
    a: &[&str],
) -> Result<(), CadError> {
    let sp = &ENTITY_MOVE;
    let args = sp.parse(a)?;
    let addrs = addresses(sp, &args.words)?;
    let by = delta(
        sp,
        args.get("--by")
            .ok_or_else(|| args::usage(sp, "entity move needs --by dx,dy"))?,
    )?;
    cx.unit(args.get("--unit"), sp.verb)?;
    let zero = |d: &str| num::parse(d) == Some(0.0);
    let s = cx.s;
    let mut idx = Vec::new();
    for addr in &addrs {
        let i = cx.entity(addr)?;
        rules::entity(s, cx.doc, i)?;
        idx.push(i);
    }
    for &i in &idx {
        let eu = &s.entities[i];
        let h = cx.doc.entities[i].handle.unwrap_or(0);
        let mut seen: HashMap<i32, usize> = HashMap::new();
        let leave_followers = mutant::active("AttribsLeftBehind");
        for (k, u) in std::iter::once(&eu.head).chain(&eu.followers).enumerate() {
            let moves = k == 0 || !leave_followers;
            let xs = point_codes(s, u, u.kind.as_ref());
            let data: BTreeSet<usize> = data_pairs(s, u).into_iter().collect();
            for p in u.first..u.end {
                let c = s.tok.pairs[p].code;
                let nth = {
                    let n = seen.entry(c).or_default();
                    *n += 1;
                    *n - 1
                };
                if !moves || !data.contains(&p) {
                    continue;
                }
                let d = if xs.contains(&c) {
                    &by[0]
                } else if xs.contains(&(c - 10)) {
                    &by[1]
                } else {
                    continue;
                };
                if zero(d) {
                    continue;
                }
                let old = s.tok.value(p);
                let tok = String::from_utf8_lossy(old);
                let new = num::add(tok.trim(), d).ok_or_else(|| {
                    CadError::new(
                        VALUE_NOT_WRITABLE,
                        format!("a coordinate of {} isn't a number", handle::key(h)),
                        "save the drawing again in the app",
                    )
                })?;
                let value = keep_spaces(old, &new);
                w.set_value(p, &value);
                it.wants.push(Want::Pair {
                    h,
                    code: c,
                    nth,
                    value,
                });
            }
        }
        it.changed.insert(h);
    }
    out.word = "moved";
    out.stdout = format!("moved {} by {},{}", list(&addrs), by[0], by[1]);
    out.viewing = addrs.first().cloned();
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// entity add
// ---------------------------------------------------------------------------------------------

const ADD_LINE: Spec = Spec {
    verb: "entity add line",
    valued: &[
        "--unit", "--layer", "--color", "--ltype", "--in", "--from", "--to",
    ],
    bare: &[],
    words: true,
};
const ADD_POLYLINE: Spec = Spec {
    verb: "entity add polyline",
    valued: &[
        "--unit", "--layer", "--color", "--ltype", "--in", "--points", "--bulges", "--width",
    ],
    bare: &["--closed"],
    words: true,
};
const ADD_TEXT: Spec = Spec {
    verb: "entity add text",
    valued: &[
        "--unit", "--layer", "--color", "--ltype", "--in", "--at", "--height", "--value",
        "--rotate", "--style", "--align",
    ],
    bare: &[],
    words: true,
};

/// A point flag, `x,y[,z]`: its numbers as written (z 0 when left out).
fn point(sp: &Spec, flag: &str, v: &str) -> Result<[String; 3], CadError> {
    let n: Vec<f64> = v
        .split(',')
        .map(num::parse)
        .collect::<Option<_>>()
        .ok_or_else(|| args::usage(sp, format!("{flag} takes x,y or x,y,z")))?;
    match n.as_slice() {
        [x, y] => Ok([num::fmt_f64(*x), num::fmt_f64(*y), "0.0".into()]),
        [x, y, z] => Ok([num::fmt_f64(*x), num::fmt_f64(*y), num::fmt_f64(*z)]),
        _ => Err(args::usage(sp, format!("{flag} takes x,y or x,y,z"))),
    }
}

fn number(sp: &Spec, flag: &str, v: &str) -> Result<f64, CadError> {
    num::parse(v).ok_or_else(|| args::usage(sp, format!("{flag} takes a number")))
}

/// Where a new entity goes: its owner's handle, the pair it goes before, and whether it is in the
/// active layout's part of the entities section (`67 1`).
fn place(cx: &Cx<'_, '_>, sp: &Spec, at: Option<&str>) -> Result<(u64, usize, bool), CadError> {
    let s = cx.s;
    let doc = cx.doc;
    let at = at.unwrap_or("model");
    let b = if at.eq_ignore_ascii_case("model") {
        doc.blocks.iter().position(|b| b.kind == BlockKind::Model)
    } else if at.eq_ignore_ascii_case("paper") {
        doc.blocks.iter().position(|b| {
            b.kind == BlockKind::Layout && b.name.eq_ignore_ascii_case("*Paper_Space")
        })
    } else {
        let h = handle::from_addr(at)
            .ok_or_else(|| args::usage(sp, "--in takes model, paper or a layout's @h:HANDLE"))?;
        doc.blocks.iter().position(|b| b.handle == Some(h))
    }
    .ok_or_else(|| {
        CadError::new(
            NOT_FOUND,
            format!("{at}: no space of the drawing has that name or address"),
            "`block list` lists the drawing's layouts",
        )
    })?;
    let rec = &doc.blocks[b];
    if rec.kind == BlockKind::Block {
        return Err(CadError::new(
            IN_BLOCK_DEFINITION,
            "that is a block definition: a change there changes every reference",
            "add to model space or a layout",
        ));
    }
    let owner = rec.handle.ok_or_else(|| {
        CadError::new(
            HANDLES_OFF,
            "that space has no record of its own in the drawing",
            "save the drawing again in the app",
        )
    })?;
    let entities_end = || {
        s.sections
            .iter()
            .find(|x| x.name == "ENTITIES" && x.closed)
            .map(|x| x.end - 1)
            .ok_or_else(|| {
                CadError::new(
                    crate::error::USAGE,
                    "the drawing has no entities section to add to",
                    "save the drawing again in the app",
                )
            })
    };
    let active_paper = rec.name.eq_ignore_ascii_case("*Paper_Space");
    if rec.kind == BlockKind::Model {
        return Ok((owner, entities_end()?, false));
    }
    if active_paper {
        return Ok((owner, entities_end()?, true));
    }
    // another layout: its block, before its end
    let def = s
        .blocks
        .iter()
        .find(|d| first(s, &d.head, 2).is_some_and(|p| s.text(p).eq_ignore_ascii_case(&rec.name)))
        .and_then(|d| d.endblk.as_ref())
        .ok_or_else(|| {
            CadError::new(
                NOT_FOUND,
                "that layout has no block of its own to add to",
                "add to model space or the active layout",
            )
        })?;
    Ok((owner, def.first, false))
}

pub(crate) fn entity_add(
    cx: &Cx<'_, '_>,
    w: &mut Writer<'_, '_>,
    it: &mut Intent,
    out: &mut Out,
    a: &[&str],
) -> Result<(), CadError> {
    let kind_word = a
        .iter()
        .find(|x| !x.starts_with("--"))
        .copied()
        .unwrap_or("");
    let sp = match kind_word {
        "line" => &ADD_LINE,
        "polyline" => &ADD_POLYLINE,
        "text" => &ADD_TEXT,
        _ => {
            return Err(CadError::new(
                crate::error::USAGE,
                "entity add takes line, polyline or text first",
                "entity add line --from x,y --to x,y --unit U",
            ))
        }
    };
    let args = sp.parse(a)?;
    if args.words.len() != 1 {
        return Err(args::usage(sp, format!("{} takes no other words", sp.verb)));
    }
    cx.unit(args.get("--unit"), sp.verb)?;
    let s = cx.s;
    let pr = props(cx, sp, &args)?;
    let layer = match pr.layer {
        Some(l) => l,
        None => {
            let current = s
                .var_value("$CLAYER", 8)
                .map(|v| v.into_owned())
                .unwrap_or_else(|| "0".into());
            cx.doc.layers[rules::target_layer(cx.doc, &current)?]
                .name
                .clone()
        }
    };
    let (owner, before, paper) = place(cx, sp, args.get("--in"))?;
    let h = cx.hmax + 1;
    let mut pairs: Vec<(i32, Vec<u8>)> = Vec::new();
    let mut put = |c: i32, v: &[u8]| pairs.push((c, v.to_vec()));
    let (kind, sub) = match kind_word {
        "line" => ("LINE", "AcDbLine"),
        "polyline" => ("LWPOLYLINE", "AcDbPolyline"),
        _ => ("TEXT", "AcDbText"),
    };
    put(0, kind.as_bytes());
    put(5, cx.hex(h).as_bytes());
    put(330, cx.hex(owner).as_bytes());
    put(100, b"AcDbEntity");
    if paper {
        put(67, &cx.int(1));
    }
    put(8, layer.as_bytes());
    if let Some(lt) = &pr.ltype {
        if lt != "ByLayer" {
            put(6, lt.as_bytes());
        }
    }
    if let Some(c) = pr.color {
        if c != 256 {
            put(62, &cx.int(c));
        }
    }
    if let Some(rgb) = pr.rgb {
        put(420, &cx.int(rgb));
    }
    put(100, sub.as_bytes());
    let mut text_value = None;
    match kind {
        "LINE" => {
            let from = point(
                sp,
                "--from",
                args.get("--from")
                    .ok_or_else(|| args::usage(sp, "entity add line needs --from x,y"))?,
            )?;
            let to = point(
                sp,
                "--to",
                args.get("--to")
                    .ok_or_else(|| args::usage(sp, "entity add line needs --to x,y"))?,
            )?;
            for (k, p) in [(10, &from), (11, &to)] {
                put(k, p[0].as_bytes());
                put(k + 10, p[1].as_bytes());
                put(k + 20, p[2].as_bytes());
            }
        }
        "LWPOLYLINE" => {
            let pts: Vec<[String; 3]> = args
                .get("--points")
                .ok_or_else(|| args::usage(sp, "entity add polyline needs --points x,y;x,y;…"))?
                .split(';')
                .map(|p| point(sp, "--points", p))
                .collect::<Result<_, _>>()?;
            if pts.len() < 2 || pts.len() > VERTICES_MAX {
                return Err(args::usage(
                    sp,
                    format!("--points takes 2 to {VERTICES_MAX} points"),
                ));
            }
            if pts.iter().any(|p| p[2] != "0.0") {
                return Err(args::usage(sp, "--points takes x,y points (a 2D polyline)"));
            }
            let bulges: Vec<f64> = match args.get("--bulges") {
                Some(b) => b
                    .split(',')
                    .map(num::parse)
                    .collect::<Option<_>>()
                    .filter(|v: &Vec<f64>| v.len() == pts.len())
                    .ok_or_else(|| args::usage(sp, "--bulges takes one number per point"))?,
                None => vec![0.0; pts.len()],
            };
            put(90, &cx.int(pts.len() as i64));
            put(70, &cx.int(i64::from(args.has("--closed"))));
            if let Some(wd) = args.get("--width") {
                let x = number(sp, "--width", wd)?;
                if x < 0.0 {
                    return Err(args::usage(sp, "--width takes 0 or more"));
                }
                put(43, num::fmt_f64(x).as_bytes());
            }
            for (p, b) in pts.iter().zip(&bulges) {
                put(10, p[0].as_bytes());
                put(20, p[1].as_bytes());
                if *b != 0.0 {
                    put(42, num::fmt_f64(*b).as_bytes());
                }
            }
        }
        _ => {
            let at = point(
                sp,
                "--at",
                args.get("--at")
                    .ok_or_else(|| args::usage(sp, "entity add text needs --at x,y"))?,
            )?;
            let height = number(
                sp,
                "--height",
                args.get("--height")
                    .ok_or_else(|| args::usage(sp, "entity add text needs --height H"))?,
            )?;
            if height <= 0.0 {
                return Err(args::usage(sp, "--height takes a number above 0"));
            }
            let value = args
                .get("--value")
                .ok_or_else(|| args::usage(sp, "entity add text needs --value S"))?;
            let enc = encode_text(cx, value)?;
            let align = match args.get("--align").unwrap_or("left") {
                "left" => 0,
                "center" => 1,
                "right" => 2,
                _ => return Err(args::usage(sp, "--align takes left, center or right")),
            };
            put(10, at[0].as_bytes());
            put(20, at[1].as_bytes());
            put(30, at[2].as_bytes());
            put(40, num::fmt_f64(height).as_bytes());
            put(1, &enc);
            if let Some(r) = args.get("--rotate") {
                let r = number(sp, "--rotate", r)?;
                if r != 0.0 {
                    put(50, num::fmt_f64(r).as_bytes());
                }
            }
            if let Some(st) = args.get("--style") {
                let name = s
                    .table("STYLE")
                    .and_then(|t| {
                        t.entries.iter().find_map(|e| {
                            first(s, e, 2)
                                .map(|p| s.text(p).into_owned())
                                .filter(|n| n.eq_ignore_ascii_case(st))
                        })
                    })
                    .ok_or_else(|| {
                        CadError::new(
                            TABLE_ENTRY_MISSING,
                            "the drawing has no text style of that name",
                            "`draw header` lists the drawing's text styles",
                        )
                    })?;
                put(7, name.as_bytes());
            }
            if align != 0 {
                put(72, &cx.int(align));
                put(11, at[0].as_bytes());
                put(21, at[1].as_bytes());
                put(31, at[2].as_bytes());
            }
            put(100, b"AcDbText");
            text_value = Some(enc);
        }
    }
    w.insert_before(before, &pairs);
    if let Some(enc) = text_value {
        it.wants.push(Want::Pair {
            h,
            code: 1,
            nth: 0,
            value: enc,
        });
    }
    it.wants.push(Want::Layer {
        h,
        name: layer.clone(),
    });
    it.created.insert(h);
    raise_seed(cx, w, it, out, h);
    out.word = "added";
    out.stdout = format!("added {} ({kind})", handle::key(h));
    out.viewing = Some(handle::key(h));
    Ok(())
}

/// `$HANDSEED` raised past `last`, in its own spelling (its width and case kept).
fn raise_seed(cx: &Cx<'_, '_>, w: &mut Writer<'_, '_>, it: &mut Intent, out: &mut Out, last: u64) {
    if mutant::active("HandseedLeft") {
        return;
    }
    let s = cx.s;
    let Some(v) = s.var("$HANDSEED") else {
        return;
    };
    let Some(p) = (v.first + 1..v.end).find(|&p| s.tok.pairs[p].code == 5) else {
        return;
    };
    let old = s.tok.value(p);
    let seed = handle::parse(old).unwrap_or(0);
    let new = seed.max(last + 1);
    if new == seed {
        return;
    }
    let t = String::from_utf8_lossy(old);
    let digits = t.trim();
    let lower = digits.bytes().any(|b| b.is_ascii_lowercase());
    let mut hex = if lower {
        format!("{new:x}")
    } else {
        format!("{new:X}")
    };
    if digits.len() > hex.len() && digits.starts_with('0') {
        hex = format!("{hex:0>width$}", width = digits.len());
    }
    let value = keep_spaces(old, &hex);
    out.handseed = Some(hex);
    it.header.insert("$HANDSEED".into());
    w.set_value(p, &value);
}

// ---------------------------------------------------------------------------------------------
// entity delete
// ---------------------------------------------------------------------------------------------

const ENTITY_DELETE: Spec = Spec {
    verb: "entity delete",
    valued: &[],
    bare: &[],
    words: true,
};

/// What unit a pair is in, for a delete's referrers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Home {
    Object(usize),
    Entry(usize, usize),
    Entity(usize),
    Other,
}

pub(crate) fn entity_delete(
    cx: &Cx<'_, '_>,
    w: &mut Writer<'_, '_>,
    it: &mut Intent,
    out: &mut Out,
    a: &[&str],
) -> Result<(), CadError> {
    let sp = &ENTITY_DELETE;
    let args = sp.parse(a)?;
    let addrs = addresses(sp, &args.words)?;
    let s = cx.s;
    let mut idx = Vec::new();
    for addr in &addrs {
        let i = cx.entity(addr)?;
        rules::entity(s, cx.doc, i)?;
        idx.push(i);
    }
    // what goes: each entity with its followers, and the objects it owns
    let mut gone: BTreeSet<u64> = BTreeSet::new();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut objs: BTreeSet<usize> = BTreeSet::new();
    let owner_of = |o: usize| {
        let u = &s.objects[o];
        (u.first + 1..u.end)
            .find(|&p| s.tok.pairs[p].code == 330)
            .and_then(|p| handle::parse(s.tok.value(p)))
    };
    for &i in &idx {
        let eu = &s.entities[i];
        let h = cx.doc.entities[i].handle.unwrap_or(0);
        let end = if mutant::active("FollowersLeft") {
            eu.head.end
        } else {
            eu.followers.last().map_or(eu.head.end, |f| f.end)
        };
        ranges.push((eu.head.first, end));
        let mut frontier: Vec<(u64, u64)> = Vec::new();
        for u in std::iter::once(&eu.head).chain(&eu.followers) {
            let uh = own(s, u).unwrap_or(h);
            gone.insert(uh);
            for x in xdicts(s, u) {
                frontier.push((uh, x));
            }
        }
        while let Some((from, x)) = frontier.pop() {
            let Some(&o) = cx.objects.get(&x) else {
                continue;
            };
            if owner_of(o) != Some(from) || !objs.insert(o) {
                continue;
            }
            if objs.len() > super::OWNED_MAX {
                return Err(CadError::new(
                    EDIT_TOO_MANY,
                    format!("an entity owns more than {OWNED_MAX} objects"),
                    "delete it in the app",
                ));
            }
            let u = &s.objects[o];
            for p in u.first + 1..u.end {
                if matches!(s.tok.pairs[p].code, 350 | 360) {
                    if let Some(c) = handle::parse(s.tok.value(p)) {
                        frontier.push((x, c));
                    }
                }
            }
        }
        out.removed.push(h);
    }
    let mut owned: BTreeSet<u64> = BTreeSet::new();
    for &o in &objs {
        let u = &s.objects[o];
        ranges.push((u.first, u.end));
        if let Some(oh) = own(s, u) {
            owned.insert(oh);
        }
    }
    ranges.sort_unstable();
    let removed_pair = |p: usize| {
        let k = ranges.partition_point(|r| r.0 <= p);
        k > 0 && p < ranges[k - 1].1
    };
    // every unit, to name a referrer
    let mut homes: Vec<(usize, usize, Home)> = Vec::new();
    for (o, u) in s.objects.iter().enumerate() {
        homes.push((u.first, u.end, Home::Object(o)));
    }
    for (t, tb) in s.tables.iter().enumerate() {
        for (e, u) in tb.entries.iter().enumerate() {
            homes.push((u.first, u.end, Home::Entry(t, e)));
        }
    }
    for (e, eu) in s.entities.iter().enumerate() {
        let end = eu.followers.last().map_or(eu.head.end, |f| f.end);
        homes.push((eu.head.first, end, Home::Entity(e)));
    }
    homes.sort_unstable_by_key(|x| x.0);
    let home_of = |p: usize| {
        let k = homes.partition_point(|x| x.0 <= p);
        if k > 0 && p < homes[k - 1].1 {
            homes[k - 1].2
        } else {
            Home::Other
        }
    };
    let targets: BTreeSet<u64> = gone.union(&owned).copied().collect();
    let mut blkrefs = false;
    let mut cur_home = Home::Other;
    let mut fix: Vec<(usize, usize)> = Vec::new();
    for p in 0..s.tok.pairs.len() {
        let c = s.tok.pairs[p].code;
        let hm = home_of(p);
        if hm != cur_home {
            cur_home = hm;
            blkrefs = false;
        }
        if c == 102 {
            blkrefs = s
                .tok
                .value(p)
                .trim_ascii()
                .eq_ignore_ascii_case(b"{BLKREFS");
            continue;
        }
        if !codes::is_pointer(c) || removed_pair(p) {
            continue;
        }
        let Some(t) = handle::parse(s.tok.value(p)).filter(|t| targets.contains(t)) else {
            continue;
        };
        match hm {
            Home::Object(o) if s.objects[o].kind == "SORTENTSTABLE" && c == 331 => {
                if !mutant::active("SortentsLeft") {
                    let two = s.tok.pairs.get(p + 1).is_some_and(|x| x.code == 5);
                    fix.push((p, if two { p + 2 } else { p + 1 }));
                    if let Some(oh) = own(s, &s.objects[o]) {
                        it.objects.insert(oh);
                    }
                }
            }
            Home::Entry(tb, e) if c == 331 && blkrefs && s.tables[tb].name == "BLOCK_RECORD" => {
                fix.push((p, p + 1));
                if let Some(eh) = own(s, &s.tables[tb].entries[e]) {
                    it.entries.insert(eh);
                }
            }
            _ => {
                let by = match hm {
                    Home::Object(o) => own(s, &s.objects[o]),
                    Home::Entry(tb, e) => own(s, &s.tables[tb].entries[e]),
                    Home::Entity(e) => cx.doc.entities[e].handle,
                    Home::Other => None,
                };
                let by = by.map_or_else(|| "the header".to_owned(), handle::key);
                return Err(CadError::new(
                    REFERENCED,
                    format!("{} is pointed at by {by}", handle::key(t)),
                    format!("`entity props {by}` or the app shows what it is; remove the reference first"),
                ));
            }
        }
    }
    for (a, b) in ranges.iter().copied().chain(fix) {
        w.remove(a, b);
    }
    it.removed.extend(gone);
    it.objects_removed.extend(owned);
    out.word = "deleted";
    out.stdout = format!("deleted {}", list(&addrs));
    out.viewing = None;
    Ok(())
}
