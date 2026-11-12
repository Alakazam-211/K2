//! The pack's part of the rendering-data contract (r1.4, Item 1): a deck's view index, read
//! from the bytes the renderer draws ([`index`]). The kit adds `tier`, `rev` and `version`,
//! keys each render hash with the space's key, and pages units by container; this is what
//! only the pack can work out:
//!
//! - **geometry** (gap 4): each unit's transform box on the page, group children composed by
//!   [`crate::geom::compose`] (the true affine image, flips, `approx_box`), placeholders with
//!   no transform given their layout's box, else their master's (`bbox_inherited`), `hidden`;
//! - **the render hash** (gap 5): [`crate::render::render_digest`] per slide;
//! - **fonts** (gap 6): every font the deck's text uses, theme fonts resolved to their families,
//!   and per slide which; **field kinds** (`slidenum`, `datetime`) per unit and slide, a slide
//!   number's text shown as the slide's position; **`name_is_generic`**;
//! - **layouts** (gap 28): each layout's own name and master; each unit's placeholder type.
//!
//! On a live deck these are read from the written bytes (the carrier with the live changes
//! spliced in, contract Q2), so they are what the renderer sees.

use std::collections::{BTreeMap, BTreeSet};

use k2_mpc_base::index::Digest;
use roxmltree::{Document, Node};

use crate::deck::{rels_of, Pptx, SlideRef};
use crate::geom::{compose, GroupXf, Xf};
use crate::live::model::{flips, table_text, text_edit, TextEdit};
use crate::render;
use crate::slide::{effective, read_shape, read_xfrm, tree_node, xfrm_node};
use crate::xmlx::{child, cut, is, parse_xml, A, P};
use crate::{Decline, Limits};

/// The longest text summary per unit, in bytes.
pub const SUMMARY_MAX: usize = 1024;
/// The longest slide title, in bytes.
pub const TITLE_MAX: usize = 1024;

/// One font the deck's text uses: a family name, never font bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontUse {
    pub family: String,
    /// `latin`, `ea`, `cs` or `sym`.
    pub script: &'static str,
    /// `theme-major`, `theme-minor` or `explicit`.
    pub from: &'static str,
    /// The deck embeds it (`p:embeddedFontLst`). No embedded font is used until the Steward
    /// rules (contract Q9).
    pub embedded: bool,
}

impl FontUse {
    fn order(&self) -> (u8, &str, &str) {
        let rank = match self.from {
            "theme-major" => 0,
            "theme-minor" => 1,
            _ => 2,
        };
        (rank, self.family.as_str(), self.script)
    }
}

/// A layout, by its address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutEntry {
    pub key: String,
    /// Its own name (`p:cSld/@name`).
    pub name: String,
    pub master: Option<String>,
}

/// A transform box in page space, in EMU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bbox {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

/// One slide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    pub key: String,
    /// From 1.
    pub position: u32,
    pub title: String,
    pub hidden: bool,
    pub layout: Option<String>,
    /// The raw digest of its render closure: the kit keys it before it leaves.
    pub render_hash: Digest,
    /// Indexes into [`ViewIndex::fonts`].
    pub fonts: Vec<usize>,
    /// `slidenum`, `datetime`.
    pub has_fields: Vec<&'static str>,
    /// Every unit on it, a group's children included.
    pub unit_count: u32,
}

/// A unit's text, cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitText {
    pub summary: String,
    pub truncated: bool,
}

/// A placeholder: its type (`obj` when none is written) and index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitPh {
    pub ty: String,
    pub idx: Option<String>,
}

/// One shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// `@sld:<sldId>/sp:<cNvPr>`; `None` for a shape with no id (never on a live deck).
    pub key: Option<String>,
    pub container: String,
    /// The group it is in.
    pub parent: Option<String>,
    /// An element of the live store (a top-level shape); a group's children are read from
    /// the file and are never selected or claimed.
    pub live: bool,
    pub kind: &'static str,
    pub name: String,
    pub name_is_generic: bool,
    pub alt: Option<String>,
    /// 0 at the back, within its parent.
    pub z: u32,
    pub bbox: Option<Bbox>,
    pub bbox_inherited: bool,
    /// 60,000ths of a degree, clockwise, about the box centre.
    pub rot: i64,
    pub flip_h: bool,
    pub flip_v: bool,
    pub approx_box: bool,
    pub text: Option<UnitText>,
    pub fields: Vec<&'static str>,
    pub hidden: bool,
    pub placeholder: Option<UnitPh>,
    pub editable_text: bool,
}

/// The pack's view index of one deck.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewIndex {
    /// `p:sldSz` `(cx, cy)`, EMU.
    pub page: (i64, i64),
    pub fonts: Vec<FontUse>,
    pub layouts: Vec<LayoutEntry>,
    /// In presentation order.
    pub containers: Vec<Container>,
    /// By container, back to front, a group's children right after it.
    pub units: Vec<Unit>,
}

/// Whether a shape's name is an app's default: words that each start with a capital letter,
/// a space, then a number (`Title 1`, `TextBox 6`, `Slide Number Placeholder 3`). An empty
/// name is generic too. The view then names the unit by its kind or its text (DM 3).
pub fn name_is_generic(name: &str) -> bool {
    if name.is_empty() {
        return true;
    }
    let Some((words, num)) = name.rsplit_once(' ') else {
        return false;
    };
    let num_ok = !num.is_empty()
        && num.bytes().all(|b| b.is_ascii_digit())
        && !(num.len() > 1 && num.starts_with('0'));
    let words_ok = !words.is_empty()
        && words.split(' ').all(|w| {
            let mut c = w.chars();
            matches!(c.next(), Some(f) if f.is_uppercase()) && c.all(char::is_alphabetic)
        });
    num_ok && words_ok
}

/// The kind of a text field (`a:fld/@type`), as the contract names it.
pub fn field_kind(ty: &str) -> Option<&'static str> {
    if ty == "slidenum" {
        Some("slidenum")
    } else if ty.starts_with("datetime") {
        Some("datetime")
    } else {
        None
    }
}

/// The text body (or table) of a shape whose text and fields count.
fn text_root<'a, 'i>(n: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    if is(n, P, "graphicFrame") {
        return n.descendants().find(|d| is(*d, A, "tbl"));
    }
    child(n, P, "txBody")
}

/// The field kinds in a shape's own text, in contract order.
fn fields_of(n: Node<'_, '_>) -> Vec<&'static str> {
    let mut seen = BTreeSet::new();
    if let Some(t) = text_root(n) {
        for d in t.descendants().filter(|d| is(*d, A, "fld")) {
            if let Some(k) = d.attribute("type").and_then(field_kind) {
                seen.insert(k);
            }
        }
    }
    ["slidenum", "datetime"]
        .into_iter()
        .filter(|k| seen.contains(k))
        .collect()
}

/// A text body's text with each slide-number field shown as `position`.
fn text_at(n: Node<'_, '_>, position: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    for p in n.descendants().filter(|d| is(*d, A, "p")) {
        let mut s = String::new();
        let mut skip_to = 0usize;
        for d in p.descendants() {
            if d.range().start < skip_to {
                continue;
            }
            if is(d, A, "fld") && d.attribute("type") == Some("slidenum") {
                s.push_str(&position.to_string());
                skip_to = d.range().end;
            } else if is(d, A, "t") {
                s.push_str(d.text().unwrap_or(""));
            } else if is(d, A, "br") {
                s.push('\n');
            }
        }
        out.push(s);
    }
    out.join("\n")
}

fn summary(t: String) -> UnitText {
    let truncated = t.len() > SUMMARY_MAX;
    UnitText {
        summary: cut(&t, SUMMARY_MAX).to_owned(),
        truncated,
    }
}

fn xf_of(x: Node<'_, '_>) -> Xf {
    let v = read_xfrm(x);
    let (fh, fv) = flips(x);
    Xf {
        x: v.x,
        y: v.y,
        w: v.cx,
        h: v.cy,
        rot: v.rot,
        fh,
        fv,
    }
}

fn group_xf(g: Node<'_, '_>) -> Option<GroupXf> {
    let x = child(g, P, "grpSpPr").and_then(|s| child(s, A, "xfrm"))?;
    let n = |e: &str, a: &str| {
        child(x, A, e)
            .and_then(|c| c.attribute(a))
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(0)
    };
    Some(GroupXf {
        own: xf_of(x),
        ch_x: n("chOff", "x"),
        ch_y: n("chOff", "y"),
        ch_w: n("chExt", "cx"),
        ch_h: n("chExt", "cy"),
    })
}

/// A placeholder's box in a layout or master: its type class and index.
#[derive(Debug, Clone)]
struct PhBox {
    ty: String,
    idx: Option<String>,
    xf: Option<Xf>,
}

fn ph_boxes(doc: &Document<'_>) -> Vec<PhBox> {
    let Some(tree) = tree_node(doc.root_element()) else {
        return Vec::new();
    };
    tree.children()
        .filter(|c| c.is_element())
        .filter_map(|c| {
            let info = read_shape(c);
            let ph = info.placeholder?;
            Some(PhBox {
                ty: ph.kind.unwrap_or_else(|| "obj".into()),
                idx: ph.idx,
                xf: xfrm_node(effective(c)).map(xf_of),
            })
        })
        .collect()
}

/// The master's placeholder type a slide placeholder's type falls back to.
fn master_class(ty: &str) -> &str {
    match ty {
        "title" | "ctrTitle" => "title",
        "dt" | "ftr" | "sldNum" | "hdr" => ty,
        _ => "body",
    }
}

/// The box a placeholder with no transform takes: its layout's placeholder (by index, else by
/// type), else its master's (by type class).
fn inherited(ty: &str, idx: Option<&str>, layout: &[PhBox], master: &[PhBox]) -> Option<Xf> {
    let by_idx = idx.and_then(|i| layout.iter().find(|b| b.idx.as_deref() == Some(i)));
    let l = by_idx.or_else(|| layout.iter().find(|b| b.ty == ty));
    if let Some(xf) = l.and_then(|b| b.xf) {
        return Some(xf);
    }
    let class = master_class(l.map_or(ty, |b| b.ty.as_str()));
    master.iter().find(|b| b.ty == class).and_then(|b| b.xf)
}

/// What the deck's text falls back to: the theme's fonts, the master's text styles and the
/// presentation's default text style.
#[derive(Debug, Clone, Default)]
struct Fonts {
    major: String,
    minor: String,
    major_ea: String,
    minor_ea: String,
    major_cs: String,
    minor_cs: String,
    /// Title, body and other text's default typeface tokens, from the master.
    title: String,
    body: String,
    /// The presentation's default text style's.
    other: String,
    embedded: BTreeSet<String>,
}

fn latin_of(n: Option<Node<'_, '_>>) -> Option<String> {
    n.and_then(|n| child(n, A, "lvl1pPr"))
        .and_then(|l| child(l, A, "defRPr"))
        .and_then(|d| child(d, A, "latin"))
        .and_then(|l| l.attribute("typeface"))
        .map(str::to_owned)
}

impl Fonts {
    fn resolve(&self, tf: &str, script: &'static str) -> Option<FontUse> {
        let theme = |family: &str, from: &'static str, script: &'static str| {
            (!family.is_empty()).then(|| FontUse {
                family: family.to_owned(),
                script,
                from,
                embedded: self.embedded.contains(family),
            })
        };
        match tf {
            "" => None,
            "+mj-lt" => theme(&self.major, "theme-major", "latin"),
            "+mn-lt" => theme(&self.minor, "theme-minor", "latin"),
            "+mj-ea" => theme(&self.major_ea, "theme-major", "ea"),
            "+mn-ea" => theme(&self.minor_ea, "theme-minor", "ea"),
            "+mj-cs" => theme(&self.major_cs, "theme-major", "cs"),
            "+mn-cs" => theme(&self.minor_cs, "theme-minor", "cs"),
            f => Some(FontUse {
                family: f.to_owned(),
                script,
                from: "explicit",
                embedded: self.embedded.contains(f),
            }),
        }
    }

    /// The fonts one shape's text uses (a group's: its children's).
    fn of_shape(&self, outer: Node<'_, '_>, out: &mut Vec<FontUse>) {
        let n = effective(outer);
        if is(n, P, "grpSp") {
            for c in n
                .children()
                .filter(|c| c.is_element() && !is(*c, P, "nvGrpSpPr") && !is(*c, P, "grpSpPr"))
            {
                self.of_shape(c, out);
            }
            return;
        }
        let info = read_shape(outer);
        let style_ref = child(n, P, "style")
            .and_then(|s| child(s, A, "fontRef"))
            .and_then(|f| f.attribute("idx"))
            .map(|i| if i == "major" { "+mj-lt" } else { "+mn-lt" });
        let default = match (&info.placeholder, style_ref) {
            (_, Some(r)) => r.to_owned(),
            (Some(ph), None) => match ph.kind.as_deref() {
                Some("title" | "ctrTitle") => self.title.clone(),
                _ => self.body.clone(),
            },
            (None, None) => self.other.clone(),
        };
        if let Some(r) = style_ref {
            out.extend(self.resolve(r, "latin"));
        }
        let Some(t) = text_root(n) else { return };
        for run in t
            .descendants()
            .filter(|d| is(*d, A, "r") || is(*d, A, "fld"))
        {
            let rpr = child(run, A, "rPr");
            let tf = |e: &str| {
                rpr.and_then(|r| child(r, A, e))
                    .and_then(|x| x.attribute("typeface"))
                    .map(str::to_owned)
            };
            let latin = tf("latin").unwrap_or_else(|| default.clone());
            out.extend(self.resolve(&latin, "latin"));
            for (e, script) in [("ea", "ea"), ("cs", "cs"), ("sym", "sym")] {
                if let Some(x) = tf(e) {
                    out.extend(self.resolve(&x, script));
                }
            }
        }
    }
}

/// The theme's font scheme: `(major latin, minor latin, major ea, minor ea, major cs, minor cs)`.
fn theme_fonts(doc: &Document<'_>) -> [String; 6] {
    let scheme = doc.descendants().find(|d| is(*d, A, "fontScheme"));
    let get = |which: &str, e: &str| {
        scheme
            .and_then(|s| child(s, A, which))
            .and_then(|m| child(m, A, e))
            .and_then(|l| l.attribute("typeface"))
            .unwrap_or("")
            .to_owned()
    };
    [
        get("majorFont", "latin"),
        get("minorFont", "latin"),
        get("majorFont", "ea"),
        get("minorFont", "ea"),
        get("majorFont", "cs"),
        get("minorFont", "cs"),
    ]
}

/// Per master part: its placeholder boxes and the fonts its slides fall back to.
struct MasterCtx {
    boxes: Vec<PhBox>,
    fonts: Fonts,
}

fn master_ctx(
    p: &Pptx<'_>,
    part: &str,
    base: &Fonts,
    limits: &Limits,
) -> Result<MasterCtx, Decline> {
    let doc = parse_xml(p.text(part)?, limits)?;
    let mut fonts = base.clone();
    let styles = child(doc.root_element(), P, "txStyles");
    fonts.title =
        latin_of(styles.and_then(|s| child(s, P, "titleStyle"))).unwrap_or_else(|| "+mj-lt".into());
    fonts.body =
        latin_of(styles.and_then(|s| child(s, P, "bodyStyle"))).unwrap_or_else(|| "+mn-lt".into());
    let rels = rels_of(&p.opc, part, limits)?;
    let src = p.name_of(part);
    let theme = rels
        .list
        .iter()
        .filter(|r| !r.external && r.rel_type.ends_with("/theme"))
        .find_map(|r| r.target_key(&src))
        .filter(|k| p.opc.parts.contains_key(k));
    if let Some(t) = theme {
        let tdoc = parse_xml(p.text(&t)?, limits)?;
        let [a, b, c, d, e, f] = theme_fonts(&tdoc);
        (
            fonts.major,
            fonts.minor,
            fonts.major_ea,
            fonts.minor_ea,
            fonts.major_cs,
            fonts.minor_cs,
        ) = (a, b, c, d, e, f);
    }
    Ok(MasterCtx {
        boxes: ph_boxes(&doc),
        fonts,
    })
}

struct Walk<'c> {
    sid: u32,
    position: usize,
    text: &'c str,
    layout: &'c [PhBox],
    master: &'c [PhBox],
    units: Vec<Unit>,
}

impl Walk<'_> {
    #[allow(clippy::too_many_arguments)]
    fn shape(
        &mut self,
        outer: Node<'_, '_>,
        z: u32,
        parent: Option<String>,
        groups: &mut Vec<GroupXf>,
        parent_approx: bool,
    ) {
        let info = read_shape(outer);
        let n = effective(outer);
        let top = parent.is_none();
        let key = info.id.map(|id| format!("@sld:{}/sp:{id}", self.sid));
        let own = xfrm_node(n).map(xf_of);
        let (xf, inherited_box) = match (own, &info.placeholder) {
            (Some(x), _) => (Some(x), false),
            (None, Some(ph)) => {
                let ty = ph.kind.as_deref().unwrap_or("obj");
                (
                    inherited(ty, ph.idx.as_deref(), self.layout, self.master),
                    true,
                )
            }
            (None, None) => (None, false),
        };
        let placed = xf.map(|x| compose(x, groups, parent_approx));
        let text = match info.kind {
            "placeholder" | "textbox" | "shape" => {
                child(n, P, "txBody").map(|t| summary(text_at(t, self.position)))
            }
            "table" => Some(summary(table_text(n))),
            _ => None,
        };
        self.units.push(Unit {
            key: key.clone(),
            container: format!("@sld:{}", self.sid),
            parent: parent.clone(),
            live: top,
            kind: info.kind,
            name_is_generic: name_is_generic(&info.name),
            name: info.name.clone(),
            alt: info.descr.clone(),
            z,
            bbox: placed.map(|p| Bbox {
                x: p.x,
                y: p.y,
                w: p.w,
                h: p.h,
            }),
            bbox_inherited: inherited_box && placed.is_some(),
            rot: placed.map_or(0, |p| p.rot),
            flip_h: placed.is_some_and(|p| p.fh),
            flip_v: placed.is_some_and(|p| p.fv),
            approx_box: placed.is_some_and(|p| p.approx),
            text,
            fields: fields_of(n),
            hidden: info.hidden,
            placeholder: info.placeholder.as_ref().map(|p| UnitPh {
                ty: p.kind.clone().unwrap_or_else(|| "obj".into()),
                idx: p.idx.clone(),
            }),
            editable_text: top && text_edit(self.text, outer) == TextEdit::Plain,
        });
        if is(n, P, "grpSp") {
            let g = group_xf(n);
            if let Some(g) = g {
                groups.push(g);
            }
            let approx = placed.is_some_and(|p| p.approx);
            let kids: Vec<Node<'_, '_>> = n
                .children()
                .filter(|c| {
                    c.is_element()
                        && !is(*c, P, "nvGrpSpPr")
                        && !is(*c, P, "grpSpPr")
                        && !is(*c, P, "extLst")
                })
                .collect();
            for (cz, c) in kids.into_iter().enumerate() {
                self.shape(c, cz as u32, key.clone(), groups, approx);
            }
            if g.is_some() {
                groups.pop();
            }
        }
    }
}

/// One slide as read: its fonts, its units, its title, its field kinds, whether it has a table.
type SlideRead = (Vec<FontUse>, Vec<Unit>, String, Vec<&'static str>, bool);

/// The view index of a file.
pub fn index(bytes: &[u8], limits: &Limits) -> Result<ViewIndex, Decline> {
    let p = Pptx::open(bytes, limits)?;
    index_open(&p, limits)
}

/// The view index of an open deck.
pub fn index_open(p: &Pptx<'_>, limits: &Limits) -> Result<ViewIndex, Decline> {
    let deck = &p.deck;
    let pdoc = parse_xml(p.text(&p.main)?, limits)?;
    let proot = pdoc.root_element();
    let mut base = Fonts {
        other: latin_of(child(proot, P, "defaultTextStyle")).unwrap_or_else(|| "+mn-lt".into()),
        ..Fonts::default()
    };
    if let Some(l) = child(proot, P, "embeddedFontLst") {
        for f in l.descendants().filter(|d| is(*d, P, "font")) {
            if let Some(t) = f.attribute("typeface") {
                base.embedded.insert(t.to_owned());
            }
        }
    }
    let mut masters: BTreeMap<String, MasterCtx> = BTreeMap::new();
    let mut layout_master: BTreeMap<String, String> = BTreeMap::new();
    let mut layouts = Vec::new();
    for m in &deck.masters {
        masters.insert(m.part.clone(), master_ctx(p, &m.part, &base, limits)?);
        for l in &m.layouts {
            layout_master.insert(l.part.clone(), m.part.clone());
            if let Some(id) = l.id {
                layouts.push(LayoutEntry {
                    key: format!("@layout:{id}"),
                    name: l.name.clone(),
                    master: m.id.map(|i| format!("@master:{i}")),
                });
            }
        }
    }
    let layout_ids: BTreeMap<&str, u64> = deck
        .layouts()
        .filter_map(|(_, l)| l.id.map(|i| (l.part.as_str(), i)))
        .collect();
    let mut layout_boxes: BTreeMap<String, Vec<PhBox>> = BTreeMap::new();
    let mut per_slide: Vec<SlideRead> = Vec::new();
    for (i, s) in deck.slides.iter().enumerate() {
        let text = p.text(&s.part)?;
        let doc = parse_xml(text, limits)?;
        let layout_part = s.layout.clone().unwrap_or_default();
        if !layout_boxes.contains_key(&layout_part) && p.opc.parts.contains_key(&layout_part) {
            let ldoc = parse_xml(p.text(&layout_part)?, limits)?;
            layout_boxes.insert(layout_part.clone(), ph_boxes(&ldoc));
        }
        let none = Vec::new();
        let lb = layout_boxes.get(&layout_part).unwrap_or(&none);
        let mctx = layout_master.get(&layout_part).and_then(|m| masters.get(m));
        let mb = mctx.map_or(&none, |m| &m.boxes);
        let fonts_ctx = mctx.map_or_else(|| base.clone(), |m| m.fonts.clone());
        let mut w = Walk {
            sid: s.id,
            position: i + 1,
            text,
            layout: lb,
            master: mb,
            units: Vec::new(),
        };
        let mut fonts = Vec::new();
        let mut title = String::new();
        let mut table = false;
        if let Some(tn) = tree_node(doc.root_element()) {
            let tops: Vec<Node<'_, '_>> = tn
                .children()
                .filter(|c| {
                    c.is_element()
                        && !is(*c, P, "nvGrpSpPr")
                        && !is(*c, P, "grpSpPr")
                        && !is(*c, P, "extLst")
                })
                .collect();
            for (z, c) in tops.into_iter().enumerate() {
                fonts_ctx.of_shape(c, &mut fonts);
                let info = read_shape(c);
                if title.is_empty()
                    && info
                        .placeholder
                        .as_ref()
                        .is_some_and(|ph| matches!(ph.kind.as_deref(), Some("title" | "ctrTitle")))
                {
                    title = cut(info.text.as_deref().unwrap_or(""), TITLE_MAX).to_owned();
                }
                w.shape(c, z as u32, None, &mut Vec::new(), false);
            }
            table = doc.descendants().any(|d| {
                is(d, A, "graphicData")
                    && d.attribute("uri")
                        .is_some_and(|u| u.ends_with("/drawingml/2006/table"))
            });
        }
        let mut has = BTreeSet::new();
        for u in &w.units {
            has.extend(u.fields.iter().copied());
        }
        let has_fields: Vec<&'static str> = ["slidenum", "datetime"]
            .into_iter()
            .filter(|k| has.contains(k))
            .collect();
        per_slide.push((fonts, w.units, title, has_fields, table));
    }
    // the deck's fonts: theme major, theme minor, then explicit, by family and script
    let mut all: Vec<FontUse> = Vec::new();
    for (f, ..) in &per_slide {
        for x in f {
            if !all.contains(x) {
                all.push(x.clone());
            }
        }
    }
    all.sort_by(|a, b| a.order().cmp(&b.order()));
    let mut containers = Vec::new();
    let mut units = Vec::new();
    for (i, (s, (f, us, title, has_fields, table))) in deck.slides.iter().zip(per_slide).enumerate()
    {
        let mut idx: Vec<usize> = f
            .iter()
            .filter_map(|x| all.iter().position(|a| a == x))
            .collect();
        idx.sort_unstable();
        idx.dedup();
        let used: Vec<FontUse> = idx.iter().map(|i| all[*i].clone()).collect();
        let hash = render::render_digest(
            p,
            s,
            i + 1,
            has_fields.contains(&"slidenum"),
            table,
            &used,
            limits,
        )?;
        containers.push(Container {
            key: format!("@sld:{}", s.id),
            position: (i + 1) as u32,
            title,
            hidden: s.hidden,
            layout: slide_layout(s, &layout_ids),
            render_hash: hash,
            fonts: idx,
            has_fields,
            unit_count: us.len() as u32,
        });
        units.extend(us);
    }
    Ok(ViewIndex {
        page: deck.size.unwrap_or((0, 0)),
        fonts: all,
        layouts,
        containers,
        units,
    })
}

fn slide_layout(s: &SlideRef, ids: &BTreeMap<&str, u64>) -> Option<String> {
    s.layout
        .as_deref()
        .and_then(|l| ids.get(l))
        .map(|id| format!("@layout:{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_names() {
        for n in [
            "Title 1",
            "TextBox 6",
            "Slide Number Placeholder 3",
            "Right Arrow 7",
            "",
        ] {
            assert!(name_is_generic(n), "{n:?}");
        }
        for n in ["Revenue callout", "Title", "title 1", "Box 01", "Q4 2"] {
            assert!(!name_is_generic(n), "{n:?}");
        }
    }
}
