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
//! **Two phases**, so the live view shares them: [`read_open`] reads the file into a
//! [`DeckRead`] (each shape as the file holds it: its own transform, the box it would inherit,
//! its text with any slide number left open, its fonts; each slide's render digest), and
//! [`DeckRead::index`] places it: every shape's box composed through its groups, each slide's
//! text at its position. The live tier's derive hook ([`crate::live::view_derive`]) keeps the
//! carrier's [`DeckRead`] and places the same shapes from the live store's boxes ([`place`]),
//! so the blob view and the live view of the same bytes agree by construction.

use std::collections::{BTreeMap, BTreeSet};

use k2_mpc_base::index::Digest;
use roxmltree::{Document, Node};

use crate::deck::{rels_of, Pptx, SlideRef};
use crate::geom::{compose, GroupXf, Placed, Xf};
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

/// The deck's fonts from each slide's (in slide order, each once), theme major first, then
/// theme minor, then explicit, by family and script; and each slide's, as indexes into them.
pub fn deck_fonts(per_slide: &[Vec<FontUse>]) -> (Vec<FontUse>, Vec<Vec<usize>>) {
    let mut all: Vec<FontUse> = Vec::new();
    for f in per_slide {
        for x in f {
            if !all.contains(x) {
                all.push(x.clone());
            }
        }
    }
    all.sort_by(|a, b| a.order().cmp(&b.order()));
    let idx = per_slide
        .iter()
        .map(|f| {
            let mut i: Vec<usize> = f
                .iter()
                .filter_map(|x| all.iter().position(|a| a == x))
                .collect();
            i.sort_unstable();
            i.dedup();
            i
        })
        .collect();
    (all, idx)
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

/// The field kinds in `kinds`, once each, in the contract's order.
pub fn field_order(kinds: &BTreeSet<&'static str>) -> Vec<&'static str> {
    ["slidenum", "datetime"]
        .into_iter()
        .filter(|k| kinds.contains(k))
        .collect()
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
    field_order(&seen)
}

/// A piece of a shape's text as the view shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Text(String),
    /// A slide-number field: the slide's position, wherever the slide is.
    SlideNum,
}

/// A shape's text as read, a slide number left open until the slide's position is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRead(Vec<Piece>);

impl TextRead {
    /// The text with each slide number shown as `position`, cut to [`SUMMARY_MAX`].
    pub fn at(&self, position: usize) -> UnitText {
        let mut s = String::new();
        for p in &self.0 {
            match p {
                Piece::Text(t) => s.push_str(t),
                Piece::SlideNum => s.push_str(&position.to_string()),
            }
        }
        summary(s)
    }
}

/// A text body's text, each slide-number field a [`Piece::SlideNum`].
fn text_pieces(n: Node<'_, '_>) -> TextRead {
    let mut out: Vec<Piece> = Vec::new();
    let text = |out: &mut Vec<Piece>, s: &str| match out.last_mut() {
        Some(Piece::Text(t)) => t.push_str(s),
        _ => out.push(Piece::Text(s.to_owned())),
    };
    for (i, p) in n.descendants().filter(|d| is(*d, A, "p")).enumerate() {
        if i > 0 {
            text(&mut out, "\n");
        }
        let mut skip_to = 0usize;
        for d in p.descendants() {
            if d.range().start < skip_to {
                continue;
            }
            if is(d, A, "fld") && d.attribute("type") == Some("slidenum") {
                out.push(Piece::SlideNum);
                skip_to = d.range().end;
            } else if is(d, A, "t") {
                text(&mut out, d.text().unwrap_or(""));
            } else if is(d, A, "br") {
                text(&mut out, "\n");
            }
        }
    }
    if out.is_empty() {
        out.push(Piece::Text(String::new()));
    }
    TextRead(out)
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

/// One shape's fonts: all its text uses (a group's: its children's), the ones its style alone
/// gives, and the ones a run in its first run's format gives (what `shape text` writes).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShapeFonts {
    pub all: Vec<FontUse>,
    pub style: Vec<FontUse>,
    pub retext: Vec<FontUse>,
}

/// A top-level shape's style fonts and re-texted run fonts, as [`Fonts::collect`] fills them.
type Top<'a> = (&'a mut Vec<FontUse>, &'a mut Vec<FontUse>);

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

    /// The default typeface of text in a shape with no style reference: a title placeholder's,
    /// another placeholder's, or other text's.
    fn default_for(&self, placeholder: Option<Option<&str>>) -> &str {
        match placeholder {
            Some(Some("title" | "ctrTitle")) => &self.title,
            Some(_) => &self.body,
            None => &self.other,
        }
    }

    /// The fonts of one run whose properties are `rpr`, in a shape whose default is `default`.
    fn run(&self, rpr: Option<Node<'_, '_>>, default: &str, out: &mut Vec<FontUse>) {
        let tf = |e: &str| {
            rpr.and_then(|r| child(r, A, e))
                .and_then(|x| x.attribute("typeface"))
                .map(str::to_owned)
        };
        let latin = tf("latin").unwrap_or_else(|| default.to_owned());
        out.extend(self.resolve(&latin, "latin"));
        for (e, script) in [("ea", "ea"), ("cs", "cs"), ("sym", "sym")] {
            if let Some(x) = tf(e) {
                out.extend(self.resolve(&x, script));
            }
        }
    }

    /// The fonts one shape's text uses (a group's: its children's).
    fn of_shape(&self, outer: Node<'_, '_>) -> ShapeFonts {
        let mut out = ShapeFonts::default();
        let (mut style, mut retext) = (Vec::new(), Vec::new());
        self.collect(outer, &mut out.all, Some((&mut style, &mut retext)));
        out.style = style;
        out.retext = retext;
        out
    }

    /// The fonts of `outer`'s text into `all`; for a top-level shape, also its style's and a
    /// re-texted run's into `top`.
    fn collect(&self, outer: Node<'_, '_>, all: &mut Vec<FontUse>, top: Option<Top<'_>>) {
        let n = effective(outer);
        if is(n, P, "grpSp") {
            for c in n
                .children()
                .filter(|c| c.is_element() && !is(*c, P, "nvGrpSpPr") && !is(*c, P, "grpSpPr"))
            {
                self.collect(c, all, None);
            }
            return;
        }
        let info = read_shape(outer);
        let style_ref = child(n, P, "style")
            .and_then(|s| child(s, A, "fontRef"))
            .and_then(|f| f.attribute("idx"))
            .map(|i| if i == "major" { "+mj-lt" } else { "+mn-lt" });
        let default = match style_ref {
            Some(r) => r.to_owned(),
            None => self
                .default_for(info.placeholder.as_ref().map(|p| p.kind.as_deref()))
                .to_owned(),
        };
        let mut style = Vec::new();
        if let Some(r) = style_ref {
            style.extend(self.resolve(r, "latin"));
        }
        all.extend(style.iter().cloned());
        if let Some(t) = text_root(n) {
            for r in t
                .descendants()
                .filter(|d| is(*d, A, "r") || is(*d, A, "fld"))
            {
                self.run(child(r, A, "rPr"), &default, all);
            }
        }
        if let Some((top_style, top_retext)) = top {
            // `shape text` keeps the first paragraph's first run's format, or none
            let first = child(n, P, "txBody")
                .and_then(|tb| child(tb, A, "p"))
                .and_then(|p| child(p, A, "r"))
                .and_then(|r| child(r, A, "rPr"));
            self.run(first, &default, top_retext);
            *top_style = style;
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
#[derive(Debug, Clone)]
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

/// One shape as the file holds it, before it is placed ([`place`]).
#[derive(Debug, Clone)]
pub struct ShapeRead {
    pub id: Option<u32>,
    pub kind: &'static str,
    pub name: String,
    pub descr: Option<String>,
    pub hidden: bool,
    /// Its own transform, in its parent's coordinates.
    pub own: Option<Xf>,
    /// The box a placeholder with no transform takes from its layout, else its master.
    pub inherited: Option<Xf>,
    pub placeholder: Option<UnitPh>,
    pub text: Option<TextRead>,
    pub fields: Vec<&'static str>,
    /// `shape text` sets its text losing nothing (a top-level shape's).
    pub plain: bool,
    /// Whether `shape text` can set its text, and at what cost: what the live store's `edit`
    /// field holds for a top-level shape ([`text_edit`]).
    pub edit: TextEdit,
    /// A group's transform and child space.
    pub group: Option<GroupXf>,
    pub is_group: bool,
    /// A group's shapes, back to front.
    pub children: Vec<ShapeRead>,
    /// A top-level shape's fonts (empty for a group's children: the group's hold theirs).
    pub fonts: ShapeFonts,
}

fn shape_read(
    outer: Node<'_, '_>,
    text: &str,
    layout: &[PhBox],
    master: &[PhBox],
    fonts: Option<&Fonts>,
) -> ShapeRead {
    let info = read_shape(outer);
    let n = effective(outer);
    let own = xfrm_node(n).map(xf_of);
    let inherited_box = match (&own, &info.placeholder) {
        (None, Some(ph)) => inherited(
            ph.kind.as_deref().unwrap_or("obj"),
            ph.idx.as_deref(),
            layout,
            master,
        ),
        _ => None,
    };
    let unit_text = match info.kind {
        "placeholder" | "textbox" | "shape" => child(n, P, "txBody").map(text_pieces),
        "table" => Some(TextRead(vec![Piece::Text(table_text(n))])),
        _ => None,
    };
    let is_group = is(n, P, "grpSp");
    let edit = text_edit(text, outer);
    let children = if is_group {
        n.children()
            .filter(|c| {
                c.is_element()
                    && !is(*c, P, "nvGrpSpPr")
                    && !is(*c, P, "grpSpPr")
                    && !is(*c, P, "extLst")
            })
            .map(|c| shape_read(c, text, layout, master, None))
            .collect()
    } else {
        Vec::new()
    };
    ShapeRead {
        id: info.id,
        kind: info.kind,
        name: info.name.clone(),
        descr: info.descr.clone(),
        hidden: info.hidden,
        own,
        inherited: inherited_box,
        placeholder: info.placeholder.as_ref().map(|p| UnitPh {
            ty: p.kind.clone().unwrap_or_else(|| "obj".into()),
            idx: p.idx.clone(),
        }),
        text: unit_text,
        fields: fields_of(n),
        plain: edit == TextEdit::Plain,
        edit,
        group: if is_group { group_xf(n) } else { None },
        is_group,
        children,
        fonts: fonts.map(|f| f.of_shape(outer)).unwrap_or_default(),
    }
}

/// Where a slide's shapes are placed: its id and its position (for a slide number's text).
#[derive(Debug, Clone, Copy)]
pub struct At {
    pub sid: u32,
    pub position: usize,
}

/// Place shape `r` at `z` under `parent` (`None` at the top of its slide), inside `groups`
/// (outermost first), whose innermost's box is approximate when `parent_approx`: its unit,
/// then its children's, back to front ([`compose`]). `own` replaces its own transform (the
/// live store's box: [`crate::live::view_derive`]); `None` keeps the file's.
#[allow(clippy::too_many_arguments)]
pub fn place(
    at: At,
    r: &ShapeRead,
    z: u32,
    parent: Option<String>,
    groups: &mut Vec<GroupXf>,
    parent_approx: bool,
    own: Option<Xf>,
    out: &mut Vec<Unit>,
) {
    let top = parent.is_none();
    let key = r.id.map(|id| format!("@sld:{}/sp:{id}", at.sid));
    let mine = own.or(r.own);
    let (xf, inherited_box) = match (mine, &r.placeholder) {
        (Some(x), _) => (Some(x), false),
        (None, Some(_)) => (r.inherited, true),
        (None, None) => (None, false),
    };
    let placed: Option<Placed> = xf.map(|x| compose(x, groups, parent_approx));
    out.push(Unit {
        key: key.clone(),
        container: format!("@sld:{}", at.sid),
        parent: parent.clone(),
        live: top,
        kind: r.kind,
        name_is_generic: name_is_generic(&r.name),
        name: r.name.clone(),
        alt: r.descr.clone(),
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
        text: r.text.as_ref().map(|t| t.at(at.position)),
        fields: r.fields.clone(),
        hidden: r.hidden,
        placeholder: r.placeholder.clone(),
        editable_text: top && r.plain,
    });
    if r.is_group {
        let g = r.group.map(|mut g| {
            if let Some(o) = own {
                g.own = o;
            }
            g
        });
        if let Some(g) = g {
            groups.push(g);
        }
        let approx = placed.is_some_and(|p| p.approx);
        for (cz, c) in r.children.iter().enumerate() {
            place(at, c, cz as u32, key.clone(), groups, approx, None, out);
        }
        if g.is_some() {
            groups.pop();
        }
    }
}

/// One slide as read.
#[derive(Debug, Clone)]
pub struct SlideRead {
    pub sid: u32,
    /// Its position in the file, from 1.
    pub position: usize,
    pub title: String,
    pub hidden: bool,
    /// Its layout's address.
    pub layout: Option<String>,
    /// Its shapes, back to front.
    pub shapes: Vec<ShapeRead>,
    /// The fonts its text uses, shape by shape.
    pub fonts: Vec<FontUse>,
    /// The field kinds its shapes' text holds (a group's children's too).
    pub has_fields: Vec<&'static str>,
    /// It holds a table.
    pub table: bool,
    /// The raw digest of its render closure ([`render::render_digest`]), at its position.
    pub digest: Digest,
}

/// A layout's placeholder boxes, its master's, and the fonts its slides' text falls back to.
#[derive(Debug, Clone)]
struct LayoutCtx {
    boxes: Vec<PhBox>,
    master: Vec<PhBox>,
    fonts: Fonts,
}

/// A deck as read ([`read_open`]): every slide's shapes as the file holds them, and what a
/// placement needs; [`DeckRead::index`] places them.
#[derive(Debug, Clone)]
pub struct DeckRead {
    /// `p:sldSz` `(cx, cy)`, EMU.
    pub page: (i64, i64),
    pub layouts: Vec<LayoutEntry>,
    /// In presentation order.
    pub slides: Vec<SlideRead>,
    /// By layout id.
    layout_ctx: BTreeMap<u64, LayoutCtx>,
    /// The deck's own text defaults (a slide with no layout).
    base: Fonts,
}

impl DeckRead {
    /// The slide with this id.
    pub fn slide(&self, sid: u32) -> Option<&SlideRead> {
        self.slides.iter().find(|s| s.sid == sid)
    }

    fn ctx(&self, layout: Option<u64>) -> (Option<&LayoutCtx>, &Fonts) {
        let l = layout.and_then(|id| self.layout_ctx.get(&id));
        (l, l.map_or(&self.base, |l| &l.fonts))
    }

    /// The box a placeholder of type `ty` and index `idx` with no transform takes on a slide
    /// of layout `layout` (a slide the file doesn't hold yet).
    pub fn inherited(&self, layout: Option<u64>, ty: &str, idx: Option<&str>) -> Option<Xf> {
        let (l, _) = self.ctx(layout);
        let l = l?;
        inherited(ty, idx, &l.boxes, &l.master)
    }

    /// The fonts a run with no font of its own uses in a shape with no style reference, on a
    /// slide of layout `layout`: a placeholder's of type `placeholder` (`Some(None)`: an
    /// object placeholder), or a text box's (`None`).
    pub fn default_fonts(
        &self,
        layout: Option<u64>,
        placeholder: Option<Option<&str>>,
    ) -> Vec<FontUse> {
        let (_, f) = self.ctx(layout);
        f.resolve(f.default_for(placeholder), "latin")
            .into_iter()
            .collect()
    }

    /// The view index of the deck as read: every shape placed, each slide's text at its
    /// position.
    pub fn index(&self) -> ViewIndex {
        let per: Vec<Vec<FontUse>> = self.slides.iter().map(|s| s.fonts.clone()).collect();
        let (all, idx) = deck_fonts(&per);
        let mut containers = Vec::new();
        let mut units = Vec::new();
        for (s, idx) in self.slides.iter().zip(idx) {
            let at = At {
                sid: s.sid,
                position: s.position,
            };
            let mut us = Vec::new();
            for (z, r) in s.shapes.iter().enumerate() {
                place(at, r, z as u32, None, &mut Vec::new(), false, None, &mut us);
            }
            containers.push(Container {
                key: format!("@sld:{}", s.sid),
                position: s.position as u32,
                title: s.title.clone(),
                hidden: s.hidden,
                layout: s.layout.clone(),
                render_hash: s.digest,
                fonts: idx,
                has_fields: s.has_fields.clone(),
                unit_count: us.len() as u32,
            });
            units.extend(us);
        }
        ViewIndex {
            page: self.page,
            fonts: all,
            layouts: self.layouts.clone(),
            containers,
            units,
        }
    }
}

/// The view index of a file.
pub fn index(bytes: &[u8], limits: &Limits) -> Result<ViewIndex, Decline> {
    let p = Pptx::open(bytes, limits)?;
    index_open(&p, limits)
}

/// The view index of an open deck.
pub fn index_open(p: &Pptx<'_>, limits: &Limits) -> Result<ViewIndex, Decline> {
    Ok(read_open(p, limits)?.index())
}

/// An open deck read, every shape as the file holds it ([`DeckRead`]).
pub fn read_open(p: &Pptx<'_>, limits: &Limits) -> Result<DeckRead, Decline> {
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
    let none = Vec::new();
    let mut slides = Vec::new();
    let mut per_slide: Vec<Vec<FontUse>> = Vec::new();
    for (i, s) in deck.slides.iter().enumerate() {
        let text = p.text(&s.part)?;
        let doc = parse_xml(text, limits)?;
        let layout_part = s.layout.clone().unwrap_or_default();
        load_layout(p, &layout_part, limits, &mut layout_boxes)?;
        let lb = layout_boxes.get(&layout_part).unwrap_or(&none);
        let mctx = layout_master.get(&layout_part).and_then(|m| masters.get(m));
        let mb = mctx.map_or(&none, |m| &m.boxes);
        let fonts_ctx = mctx.map_or_else(|| base.clone(), |m| m.fonts.clone());
        let mut shapes = Vec::new();
        let mut fonts = Vec::new();
        let mut title = String::new();
        let mut table = false;
        if let Some(tn) = tree_node(doc.root_element()) {
            for c in tn.children().filter(|c| {
                c.is_element()
                    && !is(*c, P, "nvGrpSpPr")
                    && !is(*c, P, "grpSpPr")
                    && !is(*c, P, "extLst")
            }) {
                let r = shape_read(c, text, lb, mb, Some(&fonts_ctx));
                fonts.extend(r.fonts.all.iter().cloned());
                let info = read_shape(c);
                if title.is_empty()
                    && info
                        .placeholder
                        .as_ref()
                        .is_some_and(|ph| matches!(ph.kind.as_deref(), Some("title" | "ctrTitle")))
                {
                    title = cut(info.text.as_deref().unwrap_or(""), TITLE_MAX).to_owned();
                }
                shapes.push(r);
            }
            table = doc.descendants().any(|d| {
                is(d, A, "graphicData")
                    && d.attribute("uri")
                        .is_some_and(|u| u.ends_with("/drawingml/2006/table"))
            });
        }
        let mut has = BTreeSet::new();
        let mut walk: Vec<&ShapeRead> = shapes.iter().collect();
        while let Some(r) = walk.pop() {
            has.extend(r.fields.iter().copied());
            walk.extend(r.children.iter());
        }
        per_slide.push(fonts.clone());
        slides.push(SlideRead {
            sid: s.id,
            position: i + 1,
            title,
            hidden: s.hidden,
            layout: slide_layout(s, &layout_ids),
            shapes,
            fonts,
            has_fields: field_order(&has),
            table,
            digest: Digest::of(&[]),
        });
    }
    // each slide's render digest, over the fonts it uses in the deck's order
    let (all, idx) = deck_fonts(&per_slide);
    for ((sr, s), idx) in slides.iter_mut().zip(&deck.slides).zip(idx) {
        let used: Vec<FontUse> = idx.iter().map(|i| all[*i].clone()).collect();
        sr.digest = render::render_digest(
            p,
            s,
            sr.position,
            sr.has_fields.contains(&"slidenum"),
            sr.table,
            &used,
            limits,
        )?;
    }
    // every layout's boxes and fonts, for a slide the file doesn't hold yet
    let mut layout_ctx = BTreeMap::new();
    for (_, l) in deck.layouts() {
        let Some(id) = l.id else { continue };
        load_layout(p, &l.part, limits, &mut layout_boxes)?;
        let mctx = layout_master.get(&l.part).and_then(|m| masters.get(m));
        layout_ctx.insert(
            id,
            LayoutCtx {
                boxes: layout_boxes.get(&l.part).cloned().unwrap_or_default(),
                master: mctx.map(|m| m.boxes.clone()).unwrap_or_default(),
                fonts: mctx.map_or_else(|| base.clone(), |m| m.fonts.clone()),
            },
        );
    }
    Ok(DeckRead {
        page: deck.size.unwrap_or((0, 0)),
        layouts,
        slides,
        layout_ctx,
        base,
    })
}

/// Layout part `part`'s placeholder boxes into `boxes`, once.
fn load_layout(
    p: &Pptx<'_>,
    part: &str,
    limits: &Limits,
    boxes: &mut BTreeMap<String, Vec<PhBox>>,
) -> Result<(), Decline> {
    if !boxes.contains_key(part) && p.opc.parts.contains_key(part) {
        let ldoc = parse_xml(p.text(part)?, limits)?;
        boxes.insert(part.to_owned(), ph_boxes(&ldoc));
    }
    Ok(())
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
