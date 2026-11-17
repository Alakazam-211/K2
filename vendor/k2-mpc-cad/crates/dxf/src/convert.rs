//! A DXF file's structure as the format-neutral [`Doc`]: layers from the LAYER table, block
//! records from the BLOCK_RECORD table and the BLOCKS section, entities with their facts as
//! separate fields, and the drawing's own facts from the header.
//!
//! **An entity's digests** (each over code and value bytes, as written):
//! - `record`: every pair but its handle (and its followers' handles and owner pointers);
//! - `geom`: its kind-specific pairs (the subclass groups, extrusion, unknown codes) and its
//!   followers' geometry;
//! - `content`: what no field states: subclass markers, `102` groups (reactors, extension
//!   dictionaries), extension data, comments, its space flag and layout name, its followers'
//!   own properties.
//!
//! Its layer, colour, true colour, linetype, lineweight and transparency are fields of their
//! own (in `record`, never in `geom` or `content`), and so is its text.

use std::collections::BTreeMap;

use k2_mpc_base::index::{Digest, DigestStream};
use k2_mpc_cad_model::draw::TextStyle;
use k2_mpc_cad_model::geom::{self, Acc};
use k2_mpc_cad_model::{
    Attrib, BboxQ, BlockKind, BlockRec, Doc, Entity, Extra, Interner, Layer, Props, Units,
};

use crate::codes;
use crate::structure::{EntityUnit, Place, Structure, Unit};
use crate::{handle, mutant};

/// Header variables every save may change: left out of the drawing node's digest.
const VOLATILE: [&str; 10] = [
    "$HANDSEED",
    "$TDUPDATE",
    "$TDUUPDATE",
    "$TDINDWG",
    "$TDUSRTIMER",
    "$EXTMIN",
    "$EXTMAX",
    "$LASTSAVEDBY",
    "$VERSIONGUID",
    "$TDCREATE",
];

/// The release a version stamp names.
pub fn release(acadver: &str) -> Option<&'static str> {
    Some(match acadver {
        "AC1009" => "R12",
        "AC1012" => "R13",
        "AC1014" => "R14",
        "AC1015" => "R2000",
        "AC1018" => "R2004",
        "AC1021" => "R2007",
        "AC1024" => "R2010",
        "AC1027" => "R2013",
        "AC1032" => "R2018",
        _ => return None,
    })
}

fn feed(d: &mut DigestStream, code: i32, v: &[u8]) {
    d.update(&code.to_le_bytes());
    d.update(&(v.len() as u64).to_le_bytes());
    d.update(v);
}

fn num(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|x| x.is_finite())
}

/// The first value of code `code` in `u` (outside `102` groups and extension data).
fn first(s: &Structure<'_>, u: &Unit, code: i32) -> Option<String> {
    let mut in102 = false;
    for i in u.first + 1..u.end {
        let c = s.tok.pairs[i].code;
        if c >= 1000 {
            return None;
        }
        if c == 102 {
            in102 = s.tok.value(i).trim_ascii().starts_with(b"{");
            continue;
        }
        if !in102 && c == code {
            return Some(s.text(i).into_owned());
        }
    }
    None
}

/// The first value of code `code` in `u`'s extension data under application `app`.
fn xdata_value(s: &Structure<'_>, u: &Unit, app: &str, code: i32) -> Option<String> {
    let mut under = false;
    for i in u.first + 1..u.end {
        let c = s.tok.pairs[i].code;
        if c == 1001 {
            under = s.text(i).trim().eq_ignore_ascii_case(app);
        } else if under && c == code {
            return Some(s.text(i).into_owned());
        }
    }
    None
}

/// The digests of a table entry or block unit: record (all but the handle) and content (all
/// but the handle and the codes `fields` names).
fn unit_digests(s: &Structure<'_>, units: &[&Unit], fields: &[i32]) -> (Digest, Digest) {
    let (mut rec, mut con) = (Digest::stream(), Digest::stream());
    for u in units {
        for i in u.first..u.end {
            let c = s.tok.pairs[i].code;
            if c == 5 || c == 105 {
                continue;
            }
            let v = s.tok.value(i);
            feed(&mut rec, c, v);
            if !fields.contains(&c) && c != 0 {
                feed(&mut con, c, v);
            }
        }
    }
    (rec.finish(), con.finish())
}

/// Every handle the file's objects own (codes 5 and 105, outside the header), with counts.
fn owned(s: &Structure<'_>) -> BTreeMap<u64, u32> {
    let header = s
        .sections
        .iter()
        .find(|x| x.name == "HEADER")
        .map(|x| (x.first, x.end));
    let mut m = BTreeMap::new();
    for (i, p) in s.tok.pairs.iter().enumerate() {
        if header.is_some_and(|(a, b)| (a..b).contains(&i)) {
            continue;
        }
        if p.code == 5 || p.code == 105 {
            if let Some(h) = handle::parse(s.tok.value(i)) {
                *m.entry(h).or_insert(0) += 1;
            }
        }
    }
    m
}

/// The highest handle any object owns or any pointer names (the header aside).
fn hmax(s: &Structure<'_>) -> Option<u64> {
    let header = s
        .sections
        .iter()
        .find(|x| x.name == "HEADER")
        .map(|x| (x.first, x.end));
    let mut best: Option<u64> = None;
    for (i, p) in s.tok.pairs.iter().enumerate() {
        if header.is_some_and(|(a, b)| (a..b).contains(&i)) {
            continue;
        }
        if p.code == 5 || p.code == 105 || codes::is_pointer(p.code) {
            if let Some(h) = handle::parse(s.tok.value(i)) {
                best = Some(best.map_or(h, |b| b.max(h)));
            }
        }
    }
    best
}

/// The drawing.
pub fn doc(s: &Structure<'_>) -> Doc {
    let version = s
        .var_value("$ACADVER", 1)
        .map(|v| v.into_owned())
        .unwrap_or_default();
    let modern = version.as_str() >= "AC1015";
    let owned = owned(s);
    let dup = |h: Option<u64>| h.is_some_and(|h| owned.get(&h).copied().unwrap_or(0) > 1);

    // layers
    let mut layers = Vec::new();
    if let Some(t) = s.table("LAYER") {
        for u in &t.entries {
            let h = first(s, u, 5).and_then(|v| handle::parse(v.as_bytes()));
            let name = first(s, u, 2).unwrap_or_default();
            let flags = first(s, u, 70)
                .and_then(|v| v.trim().parse::<i64>().ok())
                .unwrap_or(0);
            let (record, content) = unit_digests(s, &[u], &[2, 62, 70, 290, 6, 370, 330, 100]);
            layers.push(Layer {
                handle: h,
                duplicate: dup(h),
                color: first(s, u, 62).and_then(|v| v.trim().parse::<i64>().ok()),
                frozen: flags & 1 != 0,
                locked: flags & 4 != 0,
                plot: first(s, u, 290).is_none_or(|v| v.trim() != "0"),
                ltype: first(s, u, 6),
                lw: first(s, u, 370),
                xref: flags & 16 != 0 || name.contains('|'),
                name,
                record,
                content,
            });
        }
    }

    // block records and their definitions
    let mut blocks: Vec<BlockRec> = Vec::new();
    let mut def_rec: Vec<Option<usize>> = vec![None; s.blocks.len()];
    let records: Vec<&Unit> = s
        .table("BLOCK_RECORD")
        .map(|t| t.entries.iter().collect())
        .unwrap_or_default();
    let kind_of = |name: &str| {
        let up = name.to_ascii_uppercase();
        if up == "*MODEL_SPACE" {
            BlockKind::Model
        } else if up.starts_with("*PAPER_SPACE") {
            BlockKind::Layout
        } else {
            BlockKind::Block
        }
    };
    const BLOCK_FIELDS: [i32; 10] = [2, 3, 10, 20, 30, 70, 330, 100, 8, 1];
    for r in &records {
        let h = first(s, r, 5).and_then(|v| handle::parse(v.as_bytes()));
        let name = first(s, r, 2).unwrap_or_default();
        // its definition: the BLOCK owned by it, else the one of the same name
        let def = (0..s.blocks.len())
            .find(|&b| {
                h.is_some()
                    && first(s, &s.blocks[b].head, 330).and_then(|v| handle::parse(v.as_bytes()))
                        == h
            })
            .or_else(|| {
                (0..s.blocks.len()).find(|&b| {
                    def_rec[b].is_none()
                        && first(s, &s.blocks[b].head, 2)
                            .is_some_and(|n| n.eq_ignore_ascii_case(&name))
                })
            });
        let mut units: Vec<&Unit> = vec![*r];
        if let Some(d) = def {
            def_rec[d] = Some(blocks.len());
            units.push(&s.blocks[d].head);
        }
        blocks.push(block_rec(
            s,
            h,
            &name,
            def,
            &units,
            &BLOCK_FIELDS,
            kind_of(&name),
            dup(h),
        ));
    }
    for (d, b) in s.blocks.iter().enumerate() {
        if def_rec[d].is_some() {
            continue;
        }
        let head = &b.head;
        let h = first(s, head, 5).and_then(|v| handle::parse(v.as_bytes()));
        let name = first(s, head, 2).unwrap_or_default();
        def_rec[d] = Some(blocks.len());
        blocks.push(block_rec(
            s,
            h,
            &name,
            Some(d),
            &[head],
            &BLOCK_FIELDS,
            kind_of(&name),
            dup(h),
        ));
    }
    let empty = Digest::stream().finish();
    let synthetic = |name: &str, key: &str, kind| BlockRec {
        handle: None,
        duplicate: false,
        key: Some(key.into()),
        name: name.into(),
        kind,
        base: None,
        anon: false,
        xref: false,
        path: None,
        layout: None,
        dynamic_of: None,
        record: empty,
        content: empty,
    };
    if !blocks.iter().any(|b| b.kind == BlockKind::Model) {
        blocks.push(synthetic("*Model_Space", "@space:model", BlockKind::Model));
    }
    let any_paper = s.entities.iter().any(|e| {
        e.place == Place::Entities && first(s, &e.head, 67).is_some_and(|v| v.trim() == "1")
    });
    if any_paper && !blocks.iter().any(|b| b.kind == BlockKind::Layout) {
        blocks.push(synthetic("*Paper_Space", "@space:paper", BlockKind::Layout));
    }
    let model = blocks.iter().position(|b| b.kind == BlockKind::Model);
    let paper = blocks.iter().position(|b| b.kind == BlockKind::Layout);
    let by_handle: BTreeMap<u64, usize> = blocks
        .iter()
        .enumerate()
        .filter_map(|(i, b)| b.handle.map(|h| (h, i)))
        .collect();
    let xref_blocks: Vec<bool> = blocks.iter().map(|b| b.xref).collect();
    // each layout's own name (the `1` after its `AcDbLayout` marker; the one before names its
    // page setup) on the block record it points to (its last owner pointer)
    for u in s.objects.iter().filter(|u| u.kind == "LAYOUT") {
        let (mut ours, mut name, mut rec) = (false, None, None);
        for i in u.first + 1..u.end {
            let c = s.tok.pairs[i].code;
            if c >= 1000 {
                break;
            }
            match c {
                100 => ours |= s.text(i).trim().eq_ignore_ascii_case("AcDbLayout"),
                1 if ours && name.is_none() => name = Some(s.text(i).into_owned()),
                330 => rec = handle::parse(s.tok.value(i)),
                _ => {}
            }
        }
        if let (Some(n), Some(&b)) = (name, rec.and_then(|h| by_handle.get(&h))) {
            blocks[b].layout = Some(n);
        }
    }
    // an anonymous block that draws a dynamic block's reference as set names that block's
    // record in its record's extension data (`AcDbBlockRepBTag`, a `1005` handle)
    for (k, r) in records.iter().enumerate() {
        if !blocks[k].anon {
            continue;
        }
        let of = xdata_value(s, r, "AcDbBlockRepBTag", 1005)
            .and_then(|v| handle::parse(v.as_bytes()))
            .and_then(|h| by_handle.get(&h).copied())
            .filter(|&t| t != k && blocks[t].kind == BlockKind::Block && !blocks[t].anon);
        blocks[k].dynamic_of = of;
    }

    // entities
    let styles = crate::draw::text_styles(s);
    let mut entities = Vec::with_capacity(s.entities.len());
    let mut names = Interner::default();
    for (i, eu) in s.entities.iter().enumerate() {
        let owner = first(s, &eu.head, 330)
            .and_then(|v| handle::parse(v.as_bytes()))
            .and_then(|h| by_handle.get(&h).copied())
            .or(match eu.place {
                Place::Block(b) => def_rec[b],
                Place::Entities => {
                    if first(s, &eu.head, 67).is_some_and(|v| v.trim() == "1") {
                        paper
                    } else {
                        model
                    }
                }
            });
        let mut e = entity(s, eu, owner, i, &mut names, &styles);
        e.duplicate = dup(e.handle);
        if owner.is_some_and(|o| xref_blocks[o]) && e.edit == "1" {
            e.edit = "xref";
        }
        entities.push(e);
    }

    let missing = entities.iter().filter(|e| e.handle.is_none()).count();
    let handles = if !modern
        && s.var_value("$HANDLING", 70)
            .is_some_and(|v| v.trim() == "0")
    {
        "off"
    } else if entities.is_empty() || missing == 0 {
        "on"
    } else if missing == entities.len() {
        "off"
    } else {
        "partial"
    };

    // the header's digest, its volatile variables aside
    let mut hd = Digest::stream();
    for v in &s.header {
        if VOLATILE.iter().any(|x| x.eq_ignore_ascii_case(&v.name)) {
            continue;
        }
        for i in v.first..v.end {
            feed(&mut hd, s.tok.pairs[i].code, s.tok.value(i));
        }
    }
    let point = |name: &str| -> Option<[f64; 3]> {
        Some([
            num(&s.var_value(name, 10)?)?,
            num(&s.var_value(name, 20)?)?,
            s.var_value(name, 30).and_then(|v| num(&v)).unwrap_or(0.0),
        ])
    };
    let header_extents = point("$EXTMIN").zip(point("$EXTMAX"));
    let handseed = s
        .var_value("$HANDSEED", 5)
        .and_then(|v| handle::parse(v.as_bytes()));
    let mut hm = hmax(s);
    if mutant::active("HmaxFromSeed") {
        hm = handseed.map(|h| h.saturating_sub(1));
    }
    let mut writer_mark = s
        .comments
        .first()
        .map(|&i| k2_mpc_base::index::cut_text(&s.text(i), 120).0.to_owned());
    if mutant::active("HeaderCopiedWhole") {
        let all: Vec<String> = s
            .header
            .iter()
            .flat_map(|v| (v.first + 1..v.end).map(|i| s.text(i).into_owned()))
            .collect();
        writer_mark = Some(all.join(" "));
    }
    let encoding = if version.as_str() >= "AC1021" {
        "utf-8".to_owned()
    } else {
        s.var_value("$DWGCODEPAGE", 3)
            .map(|v| v.to_ascii_lowercase())
            .unwrap_or_else(|| "ansi_1252".into())
    };
    let int = |name: &str| {
        s.var_value(name, 70)
            .and_then(|v| v.trim().parse::<i64>().ok())
    };
    let xrefs = s
        .blocks
        .iter()
        .enumerate()
        .filter(|(d, _)| def_rec[*d].is_some_and(|r| blocks[r].xref))
        .filter_map(|(_, b)| first(s, &b.head, 1))
        .collect();
    Doc {
        format: "dxf",
        release: release(&version),
        version,
        encoding,
        eol: Some(s.tok.eol()),
        units: Units::from_codes(int("$INSUNITS"), int("$MEASUREMENT")),
        handles,
        writer_mark,
        header: hd.finish(),
        header_extents,
        hmax: hm,
        handseed,
        layers,
        blocks,
        entities,
        xrefs,
    }
}

#[allow(clippy::too_many_arguments)]
fn block_rec(
    s: &Structure<'_>,
    h: Option<u64>,
    name: &str,
    def: Option<usize>,
    units: &[&Unit],
    fields: &[i32],
    kind: BlockKind,
    duplicate: bool,
) -> BlockRec {
    let (record, content) = unit_digests(s, units, fields);
    let head = def.map(|d| &s.blocks[d].head);
    let flags = head
        .and_then(|u| first(s, u, 70))
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(0);
    let base = head.and_then(|u| {
        Some([
            num(&first(s, u, 10)?)?,
            num(&first(s, u, 20)?)?,
            first(s, u, 30).and_then(|v| num(&v)).unwrap_or(0.0),
        ])
    });
    BlockRec {
        handle: h,
        duplicate,
        key: None,
        name: name.to_owned(),
        kind,
        base,
        anon: kind == BlockKind::Block && name.starts_with('*'),
        xref: flags & 4 != 0,
        path: head
            .filter(|_| flags & 4 != 0)
            .and_then(|u| first(s, u, 1))
            .filter(|p| !p.is_empty()),
        layout: None,
        dynamic_of: None,
        record,
        content,
    }
}

/// What an entity's pairs hold, gathered in one pass.
#[derive(Default)]
struct Gathered {
    handle: Option<u64>,
    /// A block reference's attributes, typed.
    typed: Vec<Attrib>,
    layer: Option<String>,
    color: Option<String>,
    rgb: Option<String>,
    ltype: Option<String>,
    lw: Option<String>,
    alpha: Option<String>,
    xd: Vec<String>,
    reactors: bool,
    /// The first value of each geometry code, as a number.
    nums: BTreeMap<i32, f64>,
    /// Vertices (`10`, `20`) with their bulge (`42`), in order: LWPOLYLINE's own, or the
    /// followers' VERTEXes.
    verts: Vec<([f64; 2], f64)>,
    /// Control and fit points of a spline.
    points: Vec<[f64; 2]>,
    text: Vec<String>,
    attribs: Vec<String>,
    block: Option<String>,
    /// A text's style name (`7`), for its box.
    style: Option<String>,
    /// A multiline text's chunks as written (its inline codes uninterpreted), for its box.
    raw: Vec<String>,
}

fn entity(
    s: &Structure<'_>,
    eu: &EntityUnit,
    owner: Option<usize>,
    index: usize,
    names: &mut Interner,
    styles: &[TextStyle],
) -> Entity {
    let kind = eu.head.kind.to_string();
    let (mut rec, mut geo, mut con) = (Digest::stream(), Digest::stream(), Digest::stream());
    let mut g = Gathered::default();
    let text_codes: &[i32] = match kind.as_str() {
        "TEXT" | "ATTDEF" => &[1],
        "MTEXT" => &[1, 3],
        _ => &[],
    };
    let layer_in_content = mutant::active("LayerInContentHash");
    // a multileader's content (`crate::mleader`): the model keeps its text, where it sits, its
    // first leader's first line with its landing as its outline, and every line
    let ml = matches!(kind.as_str(), "MULTILEADER" | "MLEADER");
    let mut mlr = crate::mleader::Reader::default();
    let mut annotation: Option<u64> = None;
    // the head
    let (mut in102, mut xdata) = (false, false);
    let mut owner_seen = false;
    let mut vx: Option<f64> = None;
    let mut embedded = false;
    for i in eu.head.first..eu.head.end {
        let c = s.tok.pairs[i].code;
        let v = s.tok.value(i);
        if i == eu.head.first {
            feed(&mut rec, c, v);
            feed(&mut geo, c, v);
            continue;
        }
        if c == 5 && g.handle.is_none() && !in102 && !xdata {
            g.handle = handle::parse(v);
            if g.handle.is_some() {
                continue;
            }
        }
        feed(&mut rec, c, v);
        if xdata || codes::is_xdata(c) {
            xdata = true;
            if c == 1001 {
                g.xd.push(s.text(i).into_owned());
            }
            feed(&mut con, c, v);
            continue;
        }
        if c == 102 {
            let t = v.trim_ascii();
            in102 = t.starts_with(b"{");
            if t.eq_ignore_ascii_case(b"{ACAD_REACTORS") {
                g.reactors = true;
            }
            feed(&mut con, c, v);
            continue;
        }
        if in102 || c == 999 || c == 100 || c == 67 || c == 410 {
            feed(&mut con, c, v);
            continue;
        }
        if c == 330 && !owner_seen {
            owner_seen = true;
            continue;
        }
        let field = match c {
            8 => Some(&mut g.layer),
            62 => Some(&mut g.color),
            420 => Some(&mut g.rgb),
            6 => Some(&mut g.ltype),
            370 => Some(&mut g.lw),
            440 => Some(&mut g.alpha),
            _ => None,
        };
        if let Some(f) = field {
            if f.is_none() {
                *f = Some(s.text(i).into_owned());
                if c == 8 && layer_in_content {
                    feed(&mut con, c, v);
                }
                continue;
            }
        }
        if text_codes.contains(&c) {
            let t = crate::token::decode(v);
            if kind == "MTEXT" {
                g.raw.push(t.clone().into_owned());
            }
            g.text.push(crate::token::unescape(&t).into_owned());
            continue;
        }
        if c == 7 && g.style.is_none() {
            g.style = Some(s.text(i).into_owned());
        }
        if c == 2 && kind == "INSERT" && g.block.is_none() {
            g.block = Some(s.text(i).into_owned());
        }
        feed(&mut geo, c, v);
        // a multiline text's 2018 embedded object (after `101`): its codes mean other things
        // there (its `11` is its insertion point), so none of them is taken as the text's own
        if c == 101 && kind == "MTEXT" && !mutant::active("EmbeddedCodesAsMain") {
            embedded = true;
        }
        if embedded {
            continue;
        }
        let n = std::str::from_utf8(v).ok().and_then(num);
        if ml {
            mlr.pair(c, v);
        }
        match (kind.as_str(), c) {
            ("LEADER", 10) => vx = n,
            ("LEADER", 20) => {
                if let (Some(x), Some(y)) = (vx.take(), n) {
                    g.verts.push(([x, y], 0.0));
                }
            }
            ("LEADER", 340) => annotation = handle::parse(v),
            ("LWPOLYLINE", 10) => vx = n,
            ("LWPOLYLINE", 20) => {
                if let (Some(x), Some(y)) = (vx.take(), n) {
                    g.verts.push(([x, y], 0.0));
                }
            }
            ("LWPOLYLINE", 42) => {
                if let (Some(last), Some(b)) = (g.verts.last_mut(), n) {
                    last.1 = b;
                }
            }
            ("SPLINE", 10 | 11) => vx = n,
            ("SPLINE", 20 | 21) => {
                if let (Some(x), Some(y)) = (vx.take(), n) {
                    g.points.push([x, y]);
                }
            }
            _ => {
                if let Some(n) = n {
                    g.nums.entry(c).or_insert(n);
                }
            }
        }
    }
    // the followers
    let mut followers = 0u32;
    for f in &eu.followers {
        if f.kind != "SEQEND" {
            followers += 1;
        }
        let (mut in102, mut xdata, mut owner_seen) = (false, false, false);
        let (mut tag, mut val) = (None, None);
        let (mut fx, mut fy, mut fb) = (None, None, 0.0);
        let mut fh: Option<u64> = None;
        for i in f.first..f.end {
            let c = s.tok.pairs[i].code;
            let v = s.tok.value(i);
            if c == 5 && i > f.first {
                if fh.is_none() {
                    fh = handle::parse(v);
                }
                continue;
            }
            if c == 330 && !owner_seen && !in102 && !xdata {
                owner_seen = true;
                continue;
            }
            feed(&mut rec, c, v);
            if i == f.first {
                feed(&mut geo, c, v);
                continue;
            }
            if xdata || codes::is_xdata(c) {
                xdata = true;
                feed(&mut con, c, v);
                continue;
            }
            if c == 102 {
                in102 = v.trim_ascii().starts_with(b"{");
                feed(&mut con, c, v);
                continue;
            }
            if in102 || matches!(c, 999 | 100 | 67 | 410 | 8 | 62 | 420 | 6 | 370 | 440) {
                feed(&mut con, c, v);
                continue;
            }
            if f.kind == "ATTRIB" && c == 1 {
                val = Some(crate::token::unescape(&crate::token::decode(v)).into_owned());
                continue;
            }
            if f.kind == "ATTRIB" && c == 2 {
                tag = Some(crate::token::decode(v).trim().to_owned());
                continue;
            }
            feed(&mut geo, c, v);
            if f.kind == "VERTEX" || f.kind == "ATTRIB" {
                let n = std::str::from_utf8(v).ok().and_then(num);
                match c {
                    10 => fx = n,
                    20 => fy = n,
                    42 => fb = n.unwrap_or(0.0),
                    _ => {}
                }
            }
        }
        if f.kind == "VERTEX" {
            if let (Some(x), Some(y)) = (fx, fy) {
                g.verts.push(([x, y], fb));
            }
        }
        if f.kind == "ATTRIB" {
            let (tag, val) = (tag.unwrap_or_default(), val.unwrap_or_default());
            g.attribs.push(format!("{tag}={val}"));
            g.typed.push(Attrib {
                tag: names.get(&tag),
                value: val.into_boxed_str(),
                handle: fh,
                at: fx.zip(fy).map(|(x, y)| [x, y]),
            });
        }
    }

    let n = [
        g.nums.get(&210).copied().unwrap_or(0.0),
        g.nums.get(&220).copied().unwrap_or(0.0),
        g.nums.get(&230).copied().unwrap_or(1.0),
    ];
    let up = geom::is_up(n);
    let (mut bbox, mut bbox_q) = bbox(&kind, &g, n, up);
    // a text's box: laid out as the drawing-model read lays it (one computation for both)
    if matches!(kind.as_str(), "TEXT" | "ATTDEF" | "MTEXT") {
        if let Some(b) = text_bbox(&kind, &g, n, styles) {
            bbox = Some(b);
            // an estimate: characters at 0.6 of the height, lines at their pitch
            bbox_q = if mutant::active("TextBoxExact") {
                BboxQ::Exact
            } else {
                BboxQ::Approx
            };
        }
    }
    let mlr = ml.then(|| mlr.finish());
    let text = match kind.as_str() {
        // the `3` chunks come first, the `1` last, as written
        "MTEXT" => Some(g.text.concat()),
        "TEXT" | "ATTDEF" => g.text.first().cloned(),
        "INSERT" if !g.attribs.is_empty() => Some(g.attribs.join("\n")),
        _ if ml => mlr.as_ref().and_then(|m| m.text.clone()),
        _ => None,
    };
    let at = match (g.nums.get(&10), g.nums.get(&20)) {
        _ if ml => mlr.as_ref().and_then(|m| m.at),
        (Some(&x), Some(&y)) if matches!(kind.as_str(), "TEXT" | "MTEXT" | "INSERT" | "ATTDEF") => {
            Some([x, y])
        }
        _ => None,
    };
    let mut handle = g.handle;
    if handle.is_none() && mutant::active("KeyFromPosition") {
        handle = Some(0xF00000 + index as u64);
    }
    let edit = edit_reason(&kind, up, g.reactors, handle.is_none(), g.block.as_deref());
    let props = if g.color.is_some()
        || g.rgb.is_some()
        || g.ltype.is_some()
        || g.lw.is_some()
        || g.alpha.is_some()
    {
        let mut p = |v: &Option<String>| v.as_deref().map(|v| names.get(v));
        Some(Box::new(Props {
            color: p(&g.color),
            rgb: p(&g.rgb),
            ltype: p(&g.ltype),
            lw: p(&g.lw),
            alpha: p(&g.alpha),
        }))
    } else {
        None
    };
    // what fewer entities have: typed attributes, a block reference's scale and rotation, a
    // polyline's vertices in world x and y, a leader's points (its arrow first; in world x and y
    // as written) and annotation
    let xform = if kind == "INSERT" {
        let get = |c: i32, d: f64| g.nums.get(&c).copied().unwrap_or(d);
        Some([get(41, 1.0), get(42, 1.0), get(50, 0.0)]).filter(|x| *x != [1.0, 1.0, 0.0])
    } else {
        None
    };
    let mut leader: Vec<[f64; 2]> = if kind == "LEADER" {
        g.verts.iter().map(|(q, _)| *q).collect()
    } else if let Some(m) = &mlr {
        m.first_line()
    } else {
        Vec::new()
    };
    if mutant::active("LeaderPointsReversed") {
        leader.reverse();
    }
    let verts: Vec<[f64; 2]> = if !leader.is_empty() {
        leader
    } else if matches!(kind.as_str(), "LWPOLYLINE" | "POLYLINE") {
        let elev = g.nums.get(&38).copied().unwrap_or(0.0);
        g.verts
            .iter()
            .filter_map(|(q, _)| {
                if up {
                    Some(*q)
                } else {
                    geom::to_world([q[0], q[1], elev], n).map(|w| [w[0], w[1]])
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let closed = matches!(kind.as_str(), "LWPOLYLINE" | "POLYLINE")
        && g.nums.get(&70).is_some_and(|f| (*f as i64) & 1 != 0);
    let extra = if g.typed.is_empty() && verts.is_empty() && xform.is_none() && annotation.is_none()
    {
        None
    } else {
        Some(Box::new(Extra {
            attribs: std::mem::take(&mut g.typed),
            verts,
            closed,
            xform,
            annotation,
            lines: mlr.as_ref().map(|m| m.lines()).unwrap_or_default(),
        }))
    };
    let line_rises = match (
        g.nums.get(&10),
        g.nums.get(&20),
        g.nums.get(&11),
        g.nums.get(&21),
    ) {
        (Some(x0), Some(y0), Some(x1), Some(y1)) => (x1 - x0) * (y1 - y0) >= 0.0,
        _ => true,
    };
    let mut geom = [0u8; 8];
    geom.copy_from_slice(&geo.finish().as_bytes()[..8]);
    Entity {
        handle,
        duplicate: false,
        kind: names.get(&kind),
        owner,
        layer: names.get(g.layer.as_deref().unwrap_or("0")),
        props,
        geom,
        content: con.finish(),
        record: rec.finish(),
        bbox,
        bbox_q,
        ocs: !up,
        xd: g.xd.iter().map(|x| names.get(x)).collect(),
        edit,
        followers,
        line_rises,
        extra,
        text: text.map(String::into_boxed_str),
        block: g.block.as_deref().map(|b| names.get(b)),
        at,
    }
}

fn edit_reason(
    kind: &str,
    up: bool,
    reactors: bool,
    no_handle: bool,
    block: Option<&str>,
) -> &'static str {
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
    if no_handle {
        "minted"
    } else if reactors {
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

/// A text's box around its alignment or attachment point ([`k2_mpc_cad_model::draw::text`]),
/// when its values read.
fn text_bbox(kind: &str, g: &Gathered, n: [f64; 3], styles: &[TextStyle]) -> Option<[f64; 4]> {
    use k2_mpc_cad_model::draw::text::{mtext_box, text_box};
    use k2_mpc_cad_model::draw::{MTextRaw, TextRaw};
    // the values this entity's one pass gathered, as the drawing-model read takes them
    let get = |c: i32| g.nums.get(&c).copied();
    let pt = |c: i32| Some([get(c)?, get(c + 10)?, get(c + 20).unwrap_or(0.0)]);
    let int = |c: i32| get(c).map(|x| x as i64);
    let style = g.style.clone().unwrap_or_else(|| "Standard".into());
    let b = match kind {
        "TEXT" | "ATTDEF" => {
            let t = TextRaw {
                p10: Some(pt(10)?),
                p11: pt(11),
                height: get(40).unwrap_or(1.0),
                rotation: get(50).unwrap_or(0.0),
                width: get(41),
                oblique: get(51),
                style,
                halign: int(72).unwrap_or(0),
                valign: int(if kind == "ATTDEF" { 74 } else { 73 }).unwrap_or(0),
                generation: int(71).unwrap_or(0),
                text: g.text.first().cloned().unwrap_or_default(),
                n,
            };
            text_box(&t, styles)
        }
        "MTEXT" => {
            let m = MTextRaw {
                at: pt(10)?,
                height: get(40).unwrap_or(1.0),
                width: get(41).unwrap_or(0.0),
                attach: int(71).unwrap_or(1),
                dir: pt(11),
                rotation: get(50).unwrap_or(0.0),
                style,
                spacing: get(44).unwrap_or(1.0),
                spacing_style: int(73).unwrap_or(1),
                background: None,
                columns: None,
                content: g.raw.concat(),
                n,
            };
            mtext_box(&m)
        }
        _ => return None,
    };
    b.iter().all(|v| v.is_finite()).then_some(b)
}

/// An entity's box in world x and y, and its quality.
fn bbox(kind: &str, g: &Gathered, n: [f64; 3], up: bool) -> (Option<[f64; 4]>, BboxQ) {
    let get = |c: i32| g.nums.get(&c).copied();
    let p = |cx: i32| Some([get(cx)?, get(cx + 10)?]);
    let z = |cx: i32| get(cx + 20).unwrap_or(0.0);
    // a point of the entity's own frame, in world x and y
    let world = |q: [f64; 2], zz: f64| -> Option<[f64; 2]> {
        if up {
            Some(q)
        } else {
            let w = geom::to_world([q[0], q[1], zz], n)?;
            Some([w[0], w[1]])
        }
    };
    let mut acc = Acc::new();
    let q = match kind {
        "LINE" => {
            for c in [10, 11] {
                if let Some(q) = p(c) {
                    acc.add(q[0], q[1]);
                }
            }
            BboxQ::Exact
        }
        "POINT" => {
            if let Some(q) = p(10) {
                acc.add(q[0], q[1]);
            }
            BboxQ::Exact
        }
        "3DFACE" => {
            for c in [10, 11, 12, 13] {
                if let Some(q) = p(c) {
                    acc.add(q[0], q[1]);
                }
            }
            BboxQ::Exact
        }
        "SOLID" | "TRACE" => {
            for c in [10, 11, 12, 13] {
                if let Some(w) = p(c).and_then(|q| world(q, z(c))) {
                    acc.add(w[0], w[1]);
                }
            }
            BboxQ::Exact
        }
        "CIRCLE" => {
            if let (Some(c), Some(r)) = (p(10), get(40)) {
                if let Some(b) = geom::circle([c[0], c[1], z(10)], r, n) {
                    acc.add_box(b);
                }
            }
            BboxQ::Exact
        }
        "ARC" => {
            let (Some(c), Some(r)) = (p(10), get(40)) else {
                return (None, BboxQ::None);
            };
            let (a0, a1) = (get(50).unwrap_or(0.0), get(51).unwrap_or(360.0));
            if up {
                if let Some(b) = geom::arc_deg(c, r, a0, a1) {
                    acc.add_box(b);
                }
                BboxQ::Exact
            } else {
                if let Some(b) = geom::circle([c[0], c[1], z(10)], r, n) {
                    acc.add_box(b);
                }
                BboxQ::Approx
            }
        }
        "ELLIPSE" => {
            let (Some(c), Some(m)) = (p(10), p(11)) else {
                return (None, BboxQ::None);
            };
            if let Some(b) = geom::ellipse(c, m, get(40).unwrap_or(1.0)) {
                acc.add_box(b);
            }
            let full = get(41).unwrap_or(0.0).abs() < 1e-12
                && (get(42).unwrap_or(std::f64::consts::TAU) - std::f64::consts::TAU).abs() < 1e-9;
            if full {
                BboxQ::Exact
            } else {
                BboxQ::Approx
            }
        }
        "LWPOLYLINE" | "POLYLINE" => {
            let flags = get(70).unwrap_or(0.0) as i64;
            let closed = flags & 1 != 0;
            let three_d = kind == "POLYLINE" && flags & (8 | 16 | 64) != 0;
            let v = &g.verts;
            let mut local = Acc::new();
            for (k, (q0, b)) in v.iter().enumerate() {
                let next = match v.get(k + 1) {
                    Some(n1) => Some(n1.0),
                    None if closed && v.len() > 1 => Some(v[0].0),
                    None => None,
                };
                match next {
                    Some(q1) if !three_d => geom::bulge_seg(&mut local, *q0, q1, *b),
                    _ => local.add(q0[0], q0[1]),
                }
            }
            let Some(b) = local.get() else {
                return (None, BboxQ::None);
            };
            if up || three_d {
                acc.add_box(b);
                BboxQ::Exact
            } else {
                let elev = get(38).or(get(30)).unwrap_or(0.0);
                for q in [[b[0], b[1]], [b[2], b[1]], [b[0], b[3]], [b[2], b[3]]] {
                    if let Some(w) = world(q, elev) {
                        acc.add(w[0], w[1]);
                    }
                }
                BboxQ::Approx
            }
        }
        "TEXT" | "ATTDEF" | "MTEXT" => {
            let Some(at) = p(10) else {
                return (None, BboxQ::None);
            };
            let h = get(40).unwrap_or(1.0).abs();
            let t: String = g.text.concat();
            let lines = t.split("\\P").count().max(1) as f64;
            let longest = t.split("\\P").map(|l| l.chars().count()).max().unwrap_or(0) as f64;
            let width = get(41)
                .filter(|w| *w > 0.0 && kind == "MTEXT")
                .unwrap_or(0.6 * h * longest);
            let (lo, hi) = if kind == "MTEXT" {
                ([at[0], at[1] - h * lines * 1.5], [at[0] + width, at[1] + h])
            } else {
                ([at[0], at[1]], [at[0] + width, at[1] + h])
            };
            for q in [lo, hi] {
                if let Some(w) = world(q, z(10)) {
                    acc.add(w[0], w[1]);
                }
            }
            if let Some(w) = p(11).and_then(|q| world(q, z(11))) {
                if kind != "MTEXT" {
                    acc.add(w[0], w[1]);
                }
            }
            BboxQ::Approx
        }
        "INSERT" => {
            if let Some(w) = p(10).and_then(|q| world(q, z(10))) {
                acc.add(w[0], w[1]);
            }
            BboxQ::InsertOnly
        }
        "SPLINE" => {
            for q in &g.points {
                acc.add(q[0], q[1]);
            }
            BboxQ::Approx
        }
        _ => BboxQ::None,
    };
    match acc.get() {
        Some(b) if q != BboxQ::None => (Some(b), q),
        _ => (None, BboxQ::None),
    }
}
