//! The deck as the live store holds it ([`LiveDeck`]), read from a file ([`read`]), written
//! as the kit's roots ([`LiveDeck::to_content`]) and read back from them
//! ([`LiveDeck::from_content`]).
//!
//! **Roots**:
//!
//! | Root | Kind | Holds |
//! |---|---|---|
//! | `slides` | MovableList | one element per slide, key `s<sldId>`, value `slide`: existence and order |
//! | `shapes` | MovableList | one element per top-level shape, key `s<sldId>-<cNvPr>`, value its kind: existence and z-order (a slide's z-order is the subsequence of its shapes, back to front) |
//! | `slide_props` | Map, composite with `slides` | `<slide key>\x1f<field>` |
//! | `shape_props` | Map, composite with `shapes` | `<shape key>\x1f<field>` |
//! | `deck` | Map | the page, layout facts and the id marks; written by import and outside saves only |
//!
//! **Shape fields:** `slide` (its slide's `sldId`), `fid` (its `cNvPr` id), `name`, `descr`
//! (alt text, when it has one), `geom` (**one field**: `x,y,w,h,rot,fh,fv`, EMU and 60,000ths
//! of a degree, flips `0`/`1`; empty when the shape takes its box from its layout), `text`
//! (when it has a text body: one paragraph per line; a table's cells tab-separated, its rows
//! one per line), `ph` (`type:idx`, a placeholder only), `edit` (`1` plainly editable, `fmt`
//! editable only with the loss of its formatting named, `0` never), `hidden` (`0`/`1`),
//! `refd` (`1` when an animation or connector names it) and `mc` (`1`: an alternate-content
//! shape, edited whole only).
//!
//! **Slide fields:** `sid`, `layout` (`@layout:<id>`, empty when its layout has no id),
//! `hidden`, `notes` (when it has a notes slide with a notes placeholder), `nonotes` (`1`: a
//! notes slide without one, so `notes set` is refused), `linked` (`1`: another slide links to
//! it, so `slide delete` is refused).
//!
//! **Deck fields:** `cx`, `cy`, `sections`, `notes_master`, the id marks and each layout's
//! facts. The marks: `mark/sld` is the highest `sldId` the file showed, and
//! `mark/fid/<sldId>` the highest `cNvPr` id anywhere in that slide's part (**the whole shape
//! tree**, a group's children and the tree's own id included). A live verb mints a new id one
//! past the higher of the mark and every id the store holds for that slide, live or deleted,
//! so it never takes an id a group child holds. Import and outside saves write the marks, never
//! lower them.

use std::collections::{BTreeMap, BTreeSet};

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Key, Value};
use k2_mpc_base::table::CELL_SEP;
use roxmltree::{Document, Node};

use crate::deck::{part_text, rels_of, Pptx, REL_SLIDE};
use crate::slide::{self, effective, read_shape, tree_node, Tree};
use crate::xmlx::{child, is, parse_xml, A, MC, P};
use crate::{Decline, Limits};

/// The roots.
pub const SLIDES: &str = "slides";
pub const SHAPES: &str = "shapes";
pub const SLIDE_PROPS: &str = "slide_props";
pub const SHAPE_PROPS: &str = "shape_props";
pub const DECK: &str = "deck";

/// The longest `text` or `notes` value, in bytes.
pub const TEXT_MAX: usize = 32 * 1024;

/// Field names.
pub mod f {
    pub const SLIDE: &str = "slide";
    pub const FID: &str = "fid";
    pub const NAME: &str = "name";
    pub const DESCR: &str = "descr";
    pub const GEOM: &str = "geom";
    pub const TEXT: &str = "text";
    pub const PH: &str = "ph";
    pub const EDIT: &str = "edit";
    pub const HIDDEN: &str = "hidden";
    pub const REFD: &str = "refd";
    pub const MC: &str = "mc";
    pub const SID: &str = "sid";
    pub const LAYOUT: &str = "layout";
    pub const NOTES: &str = "notes";
    pub const NO_NOTES: &str = "nonotes";
    pub const LINKED: &str = "linked";
}

/// The key of slide `sid`.
pub fn slide_key(sid: u32) -> Key {
    Key::new(format!("s{sid}")).expect("s<u32> is a list key")
}

/// The key of shape `fid` on slide `sid`.
pub fn shape_key(sid: u32, fid: u32) -> Key {
    Key::new(format!("s{sid}-{fid}")).expect("s<u32>-<u32> is a list key")
}

/// `(sid, Some(fid))` of a shape key, `(sid, None)` of a slide key.
pub fn parse_key(k: &str) -> Option<(u32, Option<u32>)> {
    let rest = k.strip_prefix('s')?;
    let num = |s: &str| -> Option<u32> {
        if s.is_empty()
            || (s.len() > 1 && s.starts_with('0'))
            || !s.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        s.parse().ok()
    };
    match rest.split_once('-') {
        Some((a, b)) => Some((num(a)?, Some(num(b)?))),
        None => Some((num(rest)?, None)),
    }
}

/// A composite map key: `<elem>\x1f<field>`.
pub fn prop(elem: &Key, field: &str) -> String {
    format!("{}{CELL_SEP}{field}", elem.as_str())
}

/// A deck map key for slide `sid`'s id mark.
pub fn fid_mark_key(sid: u32) -> String {
    format!("mark/fid/{sid}")
}

/// The deck map key of the slide id mark.
pub const SLD_MARK: &str = "mark/sld";

/// A shape's transform as one value: offset, extent, rotation and flips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Geom {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    /// 60,000ths of a degree.
    pub rot: i64,
    pub fh: bool,
    pub fv: bool,
}

impl Geom {
    pub fn encode(&self) -> String {
        format!(
            "{},{},{},{},{},{},{}",
            self.x,
            self.y,
            self.w,
            self.h,
            self.rot,
            u8::from(self.fh),
            u8::from(self.fv)
        )
    }

    pub fn parse(s: &str) -> Option<Geom> {
        let v: Vec<&str> = s.split(',').collect();
        if v.len() != 7 {
            return None;
        }
        let n = |i: usize| v[i].parse::<i64>().ok();
        let b = |i: usize| match v[i] {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        };
        Some(Geom {
            x: n(0)?,
            y: n(1)?,
            w: n(2)?,
            h: n(3)?,
            rot: n(4)?,
            fh: b(5)?,
            fv: b(6)?,
        })
    }
}

/// Whether a shape's text can be set live, and at what cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEdit {
    /// One paragraph format and one run format (or none): `shape text` loses nothing.
    Plain,
    /// More than one paragraph or run format, or a line break inside a paragraph: `shape
    /// text` would keep only the first paragraph's and run's formatting, so it is refused
    /// unless the agent names the loss (`--drop-formatting`).
    Formatted,
    /// Not a text shape this pack can splice (a picture, table, group, chart, alternate
    /// content) or text holding a field (a slide number, a date): refused.
    Never,
}

impl TextEdit {
    pub fn as_str(self) -> &'static str {
        match self {
            TextEdit::Plain => "1",
            TextEdit::Formatted => "fmt",
            TextEdit::Never => "0",
        }
    }

    pub fn parse(s: &str) -> Option<TextEdit> {
        match s {
            "1" => Some(TextEdit::Plain),
            "fmt" => Some(TextEdit::Formatted),
            "0" => Some(TextEdit::Never),
            _ => None,
        }
    }
}

/// A placeholder's binding: `type` and `idx`, as written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ph {
    pub ty: Option<String>,
    pub idx: Option<String>,
}

impl Ph {
    pub fn encode(&self) -> String {
        format!(
            "{}:{}",
            self.ty.as_deref().unwrap_or(""),
            self.idx.as_deref().unwrap_or("")
        )
    }

    pub fn parse(s: &str) -> Option<Ph> {
        let (t, i) = s.split_once(':')?;
        let o = |v: &str| (!v.is_empty()).then(|| v.to_owned());
        Some(Ph {
            ty: o(t),
            idx: o(i),
        })
    }

    /// The type, `obj` when none is written (`ST_PlaceholderType`'s default).
    pub fn kind(&self) -> &str {
        self.ty.as_deref().unwrap_or("obj")
    }
}

/// One top-level shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveShape {
    pub fid: u32,
    pub kind: String,
    pub name: String,
    pub descr: Option<String>,
    pub geom: Option<Geom>,
    pub text: Option<String>,
    pub ph: Option<Ph>,
    pub edit: TextEdit,
    pub hidden: bool,
    pub refd: bool,
    /// It is an `mc:AlternateContent`: edited whole only.
    pub mc: bool,
}

/// One slide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSlide {
    pub sid: u32,
    pub layout: Option<u64>,
    pub hidden: bool,
    pub notes: Option<String>,
    pub notes_fixed: bool,
    pub linked: bool,
    /// Back to front.
    pub shapes: Vec<LiveShape>,
}

/// A placeholder a layout gives a new slide (`slide add`: not date, footer, number or
/// header), as the file verb copies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhFacts {
    pub ty: Option<String>,
    pub orient: Option<String>,
    pub sz: Option<String>,
    pub idx: Option<String>,
    pub name: String,
    /// It has a text body (the new slide's placeholder gets an empty one).
    pub body: bool,
}

/// A layout with an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutFacts {
    pub id: u64,
    pub name: String,
    pub master: Option<u64>,
    pub placeholders: Vec<PhFacts>,
}

/// The deck, as the live store holds it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LiveDeck {
    pub cx: i64,
    pub cy: i64,
    pub sections: bool,
    pub notes_master: bool,
    /// The highest `sldId` the file showed (255 for none).
    pub sld_mark: u32,
    /// Per slide, the highest `cNvPr` id in its whole part.
    pub fid_marks: BTreeMap<u32, u32>,
    pub layouts: Vec<LayoutFacts>,
    pub slides: Vec<LiveSlide>,
}

fn decline(code: &'static str, m: impl Into<String>) -> Decline {
    Decline::new(code, m)
}

/// The attributes that carry no formatting (proofing, language, editor state).
const NOT_FORMATTING: &[&str] = &[
    "lang", "altLang", "dirty", "err", "smtClean", "smtId", "noProof", "bmk",
];

/// An element's formatting as a comparable string: its attributes (less [`NOT_FORMATTING`]),
/// sorted, then its children's bytes as written.
fn canon(text: &str, n: Option<Node<'_, '_>>) -> String {
    let Some(n) = n else {
        return String::new();
    };
    let mut attrs: Vec<String> = n
        .attributes()
        .filter(|a| a.namespace().is_some() || !NOT_FORMATTING.contains(&a.name()))
        .map(|a| format!("{}={}", a.name(), a.value()))
        .collect();
    attrs.sort();
    let mut out = attrs.join(" ");
    for c in n.children().filter(|c| c.is_element()) {
        out.push('|');
        out.push_str(&text[c.range()]);
    }
    out
}

/// Whether `shape text` can set this shape's text, and at what cost ([`TextEdit`]).
pub fn text_edit(text: &str, outer: Node<'_, '_>) -> TextEdit {
    if is(outer, MC, "AlternateContent") || !is(outer, P, "sp") {
        return TextEdit::Never;
    }
    let Some(tb) = child(outer, P, "txBody") else {
        return TextEdit::Plain;
    };
    if tb.descendants().any(|d| is(d, A, "fld")) {
        return TextEdit::Never;
    }
    let mut ppr = BTreeSet::new();
    let mut rpr = BTreeSet::new();
    for p in tb.children().filter(|c| is(*c, A, "p")) {
        ppr.insert(canon(text, child(p, A, "pPr")));
        for c in p.children().filter(|c| c.is_element()) {
            if is(c, A, "pPr") || is(c, A, "endParaRPr") {
                continue;
            }
            if is(c, A, "r") {
                rpr.insert(canon(text, child(c, A, "rPr")));
            } else {
                // a line break, or anything else inside a paragraph (math, a field...)
                return TextEdit::Formatted;
            }
        }
    }
    if ppr.len() > 1 || rpr.len() > 1 {
        TextEdit::Formatted
    } else {
        TextEdit::Plain
    }
}

/// A table's text: each row's cells tab-separated (a cell's paragraphs joined by spaces),
/// rows one per line.
pub fn table_text(n: Node<'_, '_>) -> String {
    let mut rows = Vec::new();
    for tr in n.descendants().filter(|d| is(*d, A, "tr")) {
        let cells: Vec<String> = tr
            .children()
            .filter(|c| is(*c, A, "tc"))
            .map(|tc| {
                tc.descendants()
                    .filter(|d| is(*d, A, "p"))
                    .map(|p| {
                        let mut s = String::new();
                        for d in p.descendants() {
                            if is(d, A, "t") {
                                s.push_str(d.text().unwrap_or(""));
                            }
                        }
                        s
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        rows.push(cells.join("\t"));
    }
    rows.join("\n")
}

/// The flips of a transform element.
pub fn flips(x: Node<'_, '_>) -> (bool, bool) {
    let b = |a: &str| x.attribute(a).is_some_and(|v| v == "1" || v == "true");
    (b("flipH"), b("flipV"))
}

/// A shape's [`Geom`], when it has a transform.
pub fn geom_of(n: Node<'_, '_>) -> Option<Geom> {
    let x = slide::xfrm_node(effective(n))?;
    let v = slide::read_xfrm(x);
    let (fh, fv) = flips(x);
    Some(Geom {
        x: v.x,
        y: v.y,
        w: v.cx,
        h: v.cy,
        rot: v.rot,
        fh,
        fv,
    })
}

/// The placeholders `slide add` copies from a layout part (as the file verb does).
pub fn layout_placeholders(ldoc: &Document<'_>) -> Vec<PhFacts> {
    let mut out = Vec::new();
    let Some(tree) = tree_node(ldoc.root_element()) else {
        return out;
    };
    for c in tree.children().filter(|c| is(*c, P, "sp")) {
        let Some(ph) = child(c, P, "nvSpPr")
            .and_then(|n| child(n, P, "nvPr"))
            .and_then(|n| child(n, P, "ph"))
        else {
            continue;
        };
        let ty = ph.attribute("type");
        if matches!(ty, Some("dt" | "ftr" | "sldNum" | "hdr")) {
            continue;
        }
        let a = |k: &str| ph.attribute(k).map(str::to_owned);
        out.push(PhFacts {
            ty: a("type"),
            orient: a("orient"),
            sz: a("sz"),
            idx: a("idx"),
            name: child(c, P, "nvSpPr")
                .and_then(|n| child(n, P, "cNvPr"))
                .and_then(|n| n.attribute("name"))
                .unwrap_or("Placeholder")
                .to_owned(),
            body: child(c, P, "txBody").is_some(),
        });
    }
    out
}

fn too_long(what: &str, len: usize) -> Decline {
    decline(
        "live_text_too_large",
        format!("{what} holds {len} bytes of text, over the live limit of {TEXT_MAX}"),
    )
}

/// Read one shape tree child into a [`LiveShape`].
pub fn live_shape(text: &str, doc: &Document<'_>, outer: Node<'_, '_>, fid: u32) -> LiveShape {
    let info = read_shape(outer);
    let inner = effective(outer);
    let shape_text = if info.kind == "table" {
        Some(table_text(inner))
    } else {
        info.text.clone()
    };
    LiveShape {
        fid,
        kind: info.kind.to_owned(),
        name: info.name,
        descr: info.descr,
        geom: geom_of(outer),
        text: shape_text,
        ph: info.placeholder.map(|p| Ph {
            ty: p.kind,
            idx: p.idx,
        }),
        edit: text_edit(text, outer),
        hidden: info.hidden,
        refd: crate::verbs::referenced(doc, fid),
        mc: is(outer, MC, "AlternateContent"),
    }
}

/// Read a file into the live store's model, or decline it (it then stays at tier 0.5).
///
/// Declines beyond the deck's own ([`Pptx::open`]): `live_ids` (a shape on a slide without
/// a `cNvPr` id, or an id twice in one slide's tree) and `live_text_too_large` (a shape's
/// text or a slide's notes over [`TEXT_MAX`]). The kit declines a deck over its keyed limit
/// itself (`live_too_large`, `MPC-E415`).
pub fn read(bytes: &[u8], limits: &Limits) -> Result<LiveDeck, Decline> {
    let p = Pptx::open(bytes, limits)?;
    read_open(&p, limits)
}

/// [`read`] of an open deck.
pub fn read_open(p: &Pptx<'_>, limits: &Limits) -> Result<LiveDeck, Decline> {
    let deck = &p.deck;
    let (cx, cy) = deck.size.unwrap_or((0, 0));
    let mut out = LiveDeck {
        cx,
        cy,
        sections: deck.sections,
        notes_master: deck.notes_master.is_some(),
        sld_mark: deck.slides.iter().map(|s| s.id).max().unwrap_or(255),
        ..LiveDeck::default()
    };
    let layout_ids: BTreeMap<&str, u64> = deck
        .layouts()
        .filter_map(|(_, l)| l.id.map(|id| (l.part.as_str(), id)))
        .collect();
    for (m, l) in deck.layouts() {
        let Some(id) = l.id else { continue };
        let ldoc = parse_xml(p.text(&l.part)?, limits)?;
        out.layouts.push(LayoutFacts {
            id,
            name: l.name.clone(),
            master: m.id,
            placeholders: layout_placeholders(&ldoc),
        });
    }
    // which slides another slide links to (`slide delete` would leave its link dangling)
    let by_part = deck.slide_of_part();
    let mut linked = BTreeSet::new();
    for s in &deck.slides {
        let rels = rels_of(&p.opc, &s.part, limits)?;
        let source = p.name_of(&s.part);
        for r in rels
            .list
            .iter()
            .filter(|r| r.rel_type == REL_SLIDE && !r.external)
        {
            if let Some(t) = r.target_key(&source) {
                if let Some(id) = by_part.get(&t) {
                    if *id != s.id {
                        linked.insert(*id);
                    }
                }
            }
        }
    }
    for s in &deck.slides {
        let text = p.text(&s.part)?;
        let doc = parse_xml(text, limits)?;
        let tree = Tree::parse(&doc)?;
        if !tree.ids_unique() {
            return Err(decline(
                "live_ids",
                format!(
                    "@sld:{}: a shape has no id, or two shapes share one: the deck stays at tier 0.5",
                    s.id
                ),
            ));
        }
        out.fid_marks.insert(s.id, tree.max_id());
        let tn = tree_node(doc.root_element()).expect("the tree parsed");
        let units: Vec<Node<'_, '_>> = tn
            .children()
            .filter(|c| {
                c.is_element()
                    && !is(*c, P, "nvGrpSpPr")
                    && !is(*c, P, "grpSpPr")
                    && !is(*c, P, "extLst")
            })
            .collect();
        let mut shapes = Vec::with_capacity(units.len());
        for (u, n) in tree.units.iter().zip(units) {
            let fid = u.id.expect("ids_unique: every unit has an id");
            let sh = live_shape(text, &doc, n, fid);
            if let Some(t) = &sh.text {
                if t.len() > TEXT_MAX {
                    return Err(too_long(&format!("@sld:{}/sp:{fid}", s.id), t.len()));
                }
            }
            shapes.push(sh);
        }
        let (notes, notes_fixed) = match &s.notes {
            None => (None, false),
            Some(nk) => {
                let ndoc = parse_xml(part_text(&p.opc, nk)?, limits)?;
                let nshapes = slide::read_shapes(&ndoc);
                let body = nshapes.iter().find(|x| {
                    x.placeholder
                        .as_ref()
                        .is_some_and(|ph| ph.kind.as_deref() == Some("body"))
                });
                match body {
                    Some(b) if b.id.is_some() => (Some(b.text.clone().unwrap_or_default()), false),
                    _ => (None, true),
                }
            }
        };
        if let Some(n) = &notes {
            if n.len() > TEXT_MAX {
                return Err(too_long(&format!("@notes:{}", s.id), n.len()));
            }
        }
        out.slides.push(LiveSlide {
            sid: s.id,
            layout: s.layout.as_deref().and_then(|l| layout_ids.get(l)).copied(),
            hidden: s.hidden,
            notes,
            notes_fixed,
            linked: linked.contains(&s.id),
            shapes,
        });
    }
    Ok(out)
}

fn b01(b: bool) -> Value {
    Value::str(if b { "1" } else { "0" })
}

const PH_SEP: char = '\u{1e}';

fn encode_phs(v: &[PhFacts]) -> String {
    v.iter()
        .map(|p| {
            [
                p.ty.as_deref().unwrap_or(""),
                p.orient.as_deref().unwrap_or(""),
                p.sz.as_deref().unwrap_or(""),
                p.idx.as_deref().unwrap_or(""),
                if p.body { "1" } else { "0" },
                p.name.as_str(),
            ]
            .join("\u{1f}")
        })
        .collect::<Vec<_>>()
        .join(&PH_SEP.to_string())
}

fn parse_phs(s: &str) -> Option<Vec<PhFacts>> {
    if s.is_empty() {
        return Some(Vec::new());
    }
    s.split(PH_SEP)
        .map(|one| {
            let v: Vec<&str> = one.splitn(6, '\u{1f}').collect();
            if v.len() != 6 {
                return None;
            }
            let o = |x: &str| (!x.is_empty()).then(|| x.to_owned());
            Some(PhFacts {
                ty: o(v[0]),
                orient: o(v[1]),
                sz: o(v[2]),
                idx: o(v[3]),
                body: v[4] == "1",
                name: v[5].to_owned(),
            })
        })
        .collect()
}

/// The layout address of a layout id.
pub fn layout_addr(id: u64) -> String {
    format!("@layout:{id}")
}

impl LiveShape {
    /// Its fields, as `shape_props` holds them.
    pub fn props(&self, sid: u32) -> Vec<(&'static str, Value)> {
        let mut v = vec![
            (f::SLIDE, Value::str(sid.to_string())),
            (f::FID, Value::str(self.fid.to_string())),
            (f::NAME, Value::str(self.name.clone())),
            (
                f::GEOM,
                Value::str(self.geom.map(|g| g.encode()).unwrap_or_default()),
            ),
            (f::EDIT, Value::str(self.edit.as_str())),
            (f::HIDDEN, b01(self.hidden)),
        ];
        if let Some(d) = &self.descr {
            v.push((f::DESCR, Value::str(d.clone())));
        }
        if let Some(t) = &self.text {
            v.push((f::TEXT, Value::str(t.clone())));
        }
        if let Some(p) = &self.ph {
            v.push((f::PH, Value::str(p.encode())));
        }
        if self.refd {
            v.push((f::REFD, Value::str("1")));
        }
        if self.mc {
            v.push((f::MC, Value::str("1")));
        }
        v
    }
}

impl LiveSlide {
    /// Its fields, as `slide_props` holds them.
    pub fn props(&self) -> Vec<(&'static str, Value)> {
        let mut v = vec![
            (f::SID, Value::str(self.sid.to_string())),
            (
                f::LAYOUT,
                Value::str(self.layout.map(layout_addr).unwrap_or_default()),
            ),
            (f::HIDDEN, b01(self.hidden)),
        ];
        if let Some(n) = &self.notes {
            v.push((f::NOTES, Value::str(n.clone())));
        }
        if self.notes_fixed {
            v.push((f::NO_NOTES, Value::str("1")));
        }
        if self.linked {
            v.push((f::LINKED, Value::str("1")));
        }
        v
    }
}

impl LiveDeck {
    /// The deck map: the page, the facts and the marks.
    pub fn deck_map(&self) -> BTreeMap<String, Value> {
        let mut m = BTreeMap::new();
        m.insert("cx".to_owned(), Value::str(self.cx.to_string()));
        m.insert("cy".to_owned(), Value::str(self.cy.to_string()));
        m.insert("sections".to_owned(), b01(self.sections));
        m.insert("notes_master".to_owned(), b01(self.notes_master));
        m.insert(SLD_MARK.to_owned(), Value::str(self.sld_mark.to_string()));
        for (sid, mark) in &self.fid_marks {
            m.insert(fid_mark_key(*sid), Value::str(mark.to_string()));
        }
        for l in &self.layouts {
            let k = |x: &str| format!("layout/{}/{x}", l.id);
            m.insert(k("name"), Value::str(l.name.clone()));
            m.insert(
                k("master"),
                Value::str(l.master.map(|i| format!("@master:{i}")).unwrap_or_default()),
            );
            m.insert(k("phs"), Value::str(encode_phs(&l.placeholders)));
        }
        m
    }

    /// The kit's roots, in schema order.
    pub fn to_content(&self) -> Vec<(String, Neutral)> {
        let mut slides = Vec::with_capacity(self.slides.len());
        let mut shapes = Vec::new();
        let mut sp = BTreeMap::new();
        let mut hp = BTreeMap::new();
        for s in &self.slides {
            let sk = slide_key(s.sid);
            for (fld, v) in s.props() {
                sp.insert(prop(&sk, fld), v);
            }
            slides.push((sk, Value::str("slide")));
            for x in &s.shapes {
                let k = shape_key(s.sid, x.fid);
                for (fld, v) in x.props(s.sid) {
                    hp.insert(prop(&k, fld), v);
                }
                shapes.push((k, Value::str(x.kind.clone())));
            }
        }
        vec![
            (SLIDES.to_owned(), Neutral::List(slides)),
            (SHAPES.to_owned(), Neutral::List(shapes)),
            (SLIDE_PROPS.to_owned(), Neutral::Map(sp)),
            (SHAPE_PROPS.to_owned(), Neutral::Map(hp)),
            (DECK.to_owned(), Neutral::Map(self.deck_map())),
        ]
    }

    /// The model of the kit's roots (`content` as [`LiveDeck::to_content`] or a session
    /// gives it), or why it isn't one.
    pub fn from_content(content: &[(String, Neutral)]) -> Result<LiveDeck, String> {
        let root = |name: &str| content.iter().find(|(r, _)| r == name).map(|(_, n)| n);
        let list = |name: &str| match root(name) {
            Some(Neutral::List(l)) => Ok(l.as_slice()),
            _ => Err(format!("no list root {name}")),
        };
        let empty = BTreeMap::new();
        let map = |name: &str| match root(name) {
            Some(Neutral::Map(m)) => Ok(m),
            None => Ok(&empty),
            _ => Err(format!("{name} isn't a map")),
        };
        let deck = map(DECK)?;
        let sp = map(SLIDE_PROPS)?;
        let hp = map(SHAPE_PROPS)?;
        Self::from_parts(list(SLIDES)?, list(SHAPES)?, sp, hp, deck)
    }

    /// [`LiveDeck::from_content`] from the roots themselves.
    pub fn from_parts(
        slides: &[(Key, Value)],
        shapes: &[(Key, Value)],
        sp: &BTreeMap<String, Value>,
        hp: &BTreeMap<String, Value>,
        deck: &BTreeMap<String, Value>,
    ) -> Result<LiveDeck, String> {
        let get = |m: &BTreeMap<String, Value>, k: &Key, fld: &str| -> Option<String> {
            m.get(&prop(k, fld))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        };
        let dv = |k: &str| deck.get(k).and_then(|v| v.as_str()).unwrap_or("");
        let num = |s: &str| s.parse::<i64>().unwrap_or(0);
        let mut out = LiveDeck {
            cx: num(dv("cx")),
            cy: num(dv("cy")),
            sections: dv("sections") == "1",
            notes_master: dv("notes_master") == "1",
            sld_mark: dv(SLD_MARK).parse().unwrap_or(255),
            ..LiveDeck::default()
        };
        let mut layout_ids: BTreeSet<u64> = BTreeSet::new();
        for (k, v) in deck {
            if let Some(rest) = k.strip_prefix("mark/fid/") {
                if let (Ok(sid), Some(m)) = (rest.parse::<u32>(), v.as_str()) {
                    out.fid_marks.insert(sid, m.parse().unwrap_or(0));
                }
            } else if let Some(rest) = k.strip_prefix("layout/") {
                if let Some((id, _)) = rest.split_once('/') {
                    if let Ok(id) = id.parse() {
                        layout_ids.insert(id);
                    }
                }
            }
        }
        for id in layout_ids {
            let g = |x: &str| dv(&format!("layout/{id}/{x}")).to_owned();
            out.layouts.push(LayoutFacts {
                id,
                name: g("name"),
                master: g("master")
                    .strip_prefix("@master:")
                    .and_then(|m| m.parse().ok()),
                placeholders: parse_phs(&g("phs"))
                    .ok_or_else(|| format!("layout {id}: bad placeholders"))?,
            });
        }
        let mut index: BTreeMap<u32, usize> = BTreeMap::new();
        for (k, _) in slides {
            let (sid, None) = parse_key(k.as_str()).ok_or_else(|| format!("bad slide key {k}"))?
            else {
                return Err(format!("bad slide key {k}"));
            };
            let layout = get(sp, k, f::LAYOUT).unwrap_or_default();
            index.insert(sid, out.slides.len());
            out.slides.push(LiveSlide {
                sid,
                layout: layout.strip_prefix("@layout:").and_then(|l| l.parse().ok()),
                hidden: get(sp, k, f::HIDDEN).as_deref() == Some("1"),
                notes: get(sp, k, f::NOTES),
                notes_fixed: get(sp, k, f::NO_NOTES).as_deref() == Some("1"),
                linked: get(sp, k, f::LINKED).as_deref() == Some("1"),
                shapes: Vec::new(),
            });
        }
        for (k, v) in shapes {
            let (sid, Some(fid)) =
                parse_key(k.as_str()).ok_or_else(|| format!("bad shape key {k}"))?
            else {
                return Err(format!("bad shape key {k}"));
            };
            // a shape of a deleted slide isn't in the deck
            let Some(&si) = index.get(&sid) else { continue };
            let geom = get(hp, k, f::GEOM).unwrap_or_default();
            out.slides[si].shapes.push(LiveShape {
                fid,
                kind: v.as_str().unwrap_or("").to_owned(),
                name: get(hp, k, f::NAME).unwrap_or_default(),
                descr: get(hp, k, f::DESCR),
                geom: if geom.is_empty() {
                    None
                } else {
                    Some(Geom::parse(&geom).ok_or_else(|| format!("{k}: bad geom {geom:?}"))?)
                },
                text: get(hp, k, f::TEXT),
                ph: get(hp, k, f::PH).and_then(|p| Ph::parse(&p)),
                edit: get(hp, k, f::EDIT)
                    .and_then(|e| TextEdit::parse(&e))
                    .unwrap_or(TextEdit::Never),
                hidden: get(hp, k, f::HIDDEN).as_deref() == Some("1"),
                refd: get(hp, k, f::REFD).as_deref() == Some("1"),
                mc: get(hp, k, f::MC).as_deref() == Some("1"),
            });
        }
        Ok(out)
    }

    /// The slides and shapes with their fields: what the file must read back as (the marks
    /// and layout facts are the file's, not the live doc's).
    pub fn body(&self) -> &[LiveSlide] {
        &self.slides
    }

    /// The layout with this id.
    pub fn layout(&self, id: u64) -> Option<&LayoutFacts> {
        self.layouts.iter().find(|l| l.id == id)
    }

    /// The elements plus map entries the store holds for this deck (the kit's keyed count,
    /// before any deletes).
    pub fn entries(&self) -> usize {
        let c = self.to_content();
        c.iter()
            .map(|(_, n)| match n {
                Neutral::List(l) => l.len(),
                Neutral::Map(m) => m.len(),
                _ => 0,
            })
            .sum()
    }
}
