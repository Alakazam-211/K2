//! A part holding a shape tree (a slide, a notes slide, a layout, a master), split for splicing.
//!
//! **The shape tree** (`p:cSld/p:spTree`) is the tree's own properties (`p:nvGrpSpPr`,
//! `p:grpSpPr`), then its shapes, then an optional `p:extLst`. Each shape (any child that isn't
//! one of those three) is a [`Unit`]: its element's bytes, and the bytes between it and the
//! previous child (its lead: whitespace or comments). So the part is always
//!
//! ```text
//! text[..units_start] + (lead + element) for each unit + text[units_end..]
//! ```
//!
//! and the part outside the units (`text[..units_start]` and `text[units_end..]`) is the
//! slide's **frame**: the slide's own settings, its background, transition and timing.
//!
//! **A unit's id** is the `id` of the first `cNvPr` inside it: its own non-visual properties
//! (`p:nvSpPr/p:cNvPr` for a shape), or the first choice's for an `mc:AlternateContent`.
//! Ids are unique per slide only; a unit without one is anonymous, and can't be paired.

use std::collections::BTreeSet;
use std::ops::Range;

use roxmltree::{Document, Node};

use crate::xmlx::{self, child, is, A, MC, P, R};
use crate::Decline;

/// One shape in a shape tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// The id of its own `cNvPr`, when it has one.
    pub id: Option<u32>,
    /// The element's bytes.
    pub range: Range<usize>,
    /// The bytes between the previous child and this one.
    pub lead: Range<usize>,
    /// Every `cNvPr` id inside it (its own first), in document order. An
    /// `mc:AlternateContent` repeats its shape's id in each choice.
    pub ids: Vec<u32>,
}

/// A shape tree, split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    /// The `p:spTree` element.
    pub tree: Range<usize>,
    /// Where the units start: the end of the tree's own properties.
    pub units_start: usize,
    /// Where the units end: the end of the last unit, else `units_start`.
    pub units_end: usize,
    pub units: Vec<Unit>,
    /// `cNvPr` ids outside the units (the tree's own).
    pub frame_ids: Vec<u32>,
}

fn bad(why: &str) -> Decline {
    Decline::new("bad_slide", why.to_owned())
}

fn id_of(n: Node<'_, '_>) -> Result<Option<u32>, Decline> {
    match n.attribute("id") {
        None => Ok(None),
        Some(v) => v
            .trim()
            .parse::<u32>()
            .map(Some)
            .map_err(|_| bad("a shape id isn't a number")),
    }
}

/// Every `cNvPr` id under `n` (itself included), in document order.
pub fn ids_under(n: Node<'_, '_>) -> Result<Vec<u32>, Decline> {
    let mut out = Vec::new();
    for d in n.descendants() {
        if d.is_element() && d.tag_name().name() == "cNvPr" {
            if let Some(i) = id_of(d)? {
                out.push(i);
            }
        }
    }
    Ok(out)
}

/// The shape tree element of a part's root (`p:cSld/p:spTree`).
pub fn tree_node<'a, 'i>(root: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    child(root, P, "cSld").and_then(|c| child(c, P, "spTree"))
}

impl Tree {
    /// Split the shape tree of a parsed part (`doc` is `text` parsed).
    pub fn parse(doc: &Document<'_>) -> Result<Tree, Decline> {
        let root = doc.root_element();
        let tree = tree_node(root).ok_or_else(|| bad("a part has no shape tree"))?;
        let mut units = Vec::new();
        let mut frame_ids = Vec::new();
        let mut units_start = None::<usize>;
        let mut prev_end = 0usize;
        let mut seen_ext = false;
        for c in tree.children().filter(|c| c.is_element()) {
            let frame_head = is(c, P, "nvGrpSpPr") || is(c, P, "grpSpPr");
            if frame_head {
                if !units.is_empty() || seen_ext {
                    return Err(bad("the shape tree's properties come after a shape"));
                }
                frame_ids.extend(ids_under(c)?);
                units_start = Some(c.range().end);
                prev_end = c.range().end;
                continue;
            }
            if is(c, P, "extLst") {
                seen_ext = true;
                continue;
            }
            if seen_ext {
                return Err(bad("a shape comes after the shape tree's extensions"));
            }
            if units_start.is_none() {
                return Err(bad("a shape comes before the shape tree's properties"));
            }
            let ids = ids_under(c)?;
            let r = c.range();
            units.push(Unit {
                id: ids.first().copied(),
                lead: prev_end..r.start,
                range: r.clone(),
                ids,
            });
            prev_end = r.end;
        }
        let units_start = units_start.ok_or_else(|| bad("the shape tree has no properties"))?;
        let units_end = units.last().map_or(units_start, |u| u.range.end);
        Ok(Tree {
            tree: tree.range(),
            units_start,
            units_end,
            units,
            frame_ids,
        })
    }

    /// Every `cNvPr` id in the tree (the frame's and the units').
    pub fn all_ids(&self) -> BTreeSet<u32> {
        self.frame_ids
            .iter()
            .chain(self.units.iter().flat_map(|u| u.ids.iter()))
            .copied()
            .collect()
    }

    /// Whether every unit has an id, and no id is in two units or in a unit and the frame.
    pub fn ids_unique(&self) -> bool {
        let mut seen: BTreeSet<u32> = self.frame_ids.iter().copied().collect();
        for u in &self.units {
            if u.id.is_none() {
                return false;
            }
            let own: BTreeSet<u32> = u.ids.iter().copied().collect();
            if own.iter().any(|i| seen.contains(i)) {
                return false;
            }
            seen.extend(own);
        }
        true
    }

    /// The unit with this id, by its index.
    pub fn find(&self, id: u32) -> Option<usize> {
        self.units.iter().position(|u| u.id == Some(id))
    }

    /// The highest `cNvPr` id in the tree (0 for none).
    pub fn max_id(&self) -> u32 {
        self.all_ids().into_iter().max().unwrap_or(0)
    }
}

/// A shape's position and size (`a:xfrm`), in EMU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Xfrm {
    pub x: i64,
    pub y: i64,
    pub cx: i64,
    pub cy: i64,
    /// In 60,000ths of a degree.
    pub rot: i64,
}

/// A placeholder's binding to its layout (`p:ph`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Placeholder {
    /// `type`, when written (absent means an object placeholder).
    pub kind: Option<String>,
    pub idx: Option<String>,
}

/// What a shape is, for reading (the index, the list verb).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeInfo {
    pub id: Option<u32>,
    /// `placeholder`, `textbox`, `shape`, `picture`, `table`, `chart`, `diagram`, `ole`,
    /// `graphic`, `group`, `connector`, `ink` or `other`.
    pub kind: &'static str,
    pub name: String,
    /// Alt text (`descr`).
    pub descr: Option<String>,
    pub hidden: bool,
    pub xfrm: Option<Xfrm>,
    /// The plain text of its paragraphs (a text body's, or a table's cells').
    pub text: Option<String>,
    pub placeholder: Option<Placeholder>,
    /// A picture's image relationship (`a:blip/@r:embed`).
    pub embed: Option<String>,
    /// A group's shapes, back to front.
    pub children: Vec<ShapeInfo>,
    /// The element's bytes.
    pub range: Range<usize>,
}

fn num(n: Node<'_, '_>, a: &str) -> i64 {
    n.attribute(a)
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

/// A transform element's values (`a:xfrm` or `p:xfrm`).
pub fn read_xfrm(x: Node<'_, '_>) -> Xfrm {
    let off = child(x, A, "off");
    let ext = child(x, A, "ext");
    Xfrm {
        x: off.map_or(0, |o| num(o, "x")),
        y: off.map_or(0, |o| num(o, "y")),
        cx: ext.map_or(0, |e| num(e, "cx")),
        cy: ext.map_or(0, |e| num(e, "cy")),
        rot: num(x, "rot"),
    }
}

/// The transform element of a shape, wherever its kind keeps it.
pub fn xfrm_node<'a, 'i>(n: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    if is(n, P, "graphicFrame") {
        return child(n, P, "xfrm");
    }
    if is(n, P, "grpSp") {
        return child(n, P, "grpSpPr").and_then(|s| child(s, A, "xfrm"));
    }
    child(n, P, "spPr").and_then(|s| child(s, A, "xfrm"))
}

/// The element a shape's kind is read from: an `mc:AlternateContent`'s first choice's shape.
pub fn effective<'a, 'i>(n: Node<'a, 'i>) -> Node<'a, 'i> {
    if is(n, MC, "AlternateContent") {
        if let Some(inner) = child(n, MC, "Choice").and_then(|c| c.first_element_child()) {
            return inner;
        }
    }
    n
}

/// The non-visual properties element of a shape (`p:nvSpPr`, `p:nvPicPr`...).
fn nv<'a, 'i>(n: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name().starts_with("nv"))
}

/// Read one shape tree child.
pub fn read_shape(outer: Node<'_, '_>) -> ShapeInfo {
    let n = effective(outer);
    let nvp = nv(n);
    let c_nv_pr = nvp
        .and_then(|v| {
            v.children()
                .find(|c| c.is_element() && c.tag_name().name() == "cNvPr")
        })
        .or_else(|| {
            n.descendants()
                .find(|d| d.is_element() && d.tag_name().name() == "cNvPr")
        });
    let ph = nvp
        .and_then(|v| child(v, P, "nvPr"))
        .and_then(|v| child(v, P, "ph"))
        .map(|p| Placeholder {
            kind: p.attribute("type").map(str::to_owned),
            idx: p.attribute("idx").map(str::to_owned),
        });
    let local = if n.tag_name().namespace() == Some(P) {
        n.tag_name().name()
    } else {
        ""
    };
    let kind: &'static str = match local {
        "sp" => {
            let tx = nvp
                .and_then(|v| child(v, P, "cNvSpPr"))
                .and_then(|c| c.attribute("txBox"))
                .is_some_and(|v| v == "1" || v == "true");
            if ph.is_some() {
                "placeholder"
            } else if tx {
                "textbox"
            } else {
                "shape"
            }
        }
        "pic" => "picture",
        "graphicFrame" => {
            let uri = n
                .descendants()
                .find(|d| is(*d, A, "graphicData"))
                .and_then(|g| g.attribute("uri"))
                .unwrap_or("");
            if uri.ends_with("/drawingml/2006/table") {
                "table"
            } else if uri.ends_with("/drawingml/2006/chart") {
                "chart"
            } else if uri.ends_with("/drawingml/2006/diagram") {
                "diagram"
            } else if uri.ends_with("/presentationml/2006/ole") {
                "ole"
            } else {
                "graphic"
            }
        }
        "grpSp" => "group",
        "cxnSp" => "connector",
        "contentPart" => "ink",
        _ if n.tag_name().name() == "contentPart" => "ink",
        _ => "other",
    };
    let text = match kind {
        "placeholder" | "textbox" | "shape" => child(n, P, "txBody").map(xmlx::paragraphs_text),
        "table" => Some(xmlx::paragraphs_text(n)),
        _ => None,
    };
    let children = if kind == "group" {
        n.children()
            .filter(|c| {
                c.is_element()
                    && !is(*c, P, "nvGrpSpPr")
                    && !is(*c, P, "grpSpPr")
                    && !is(*c, P, "extLst")
            })
            .map(read_shape)
            .collect()
    } else {
        Vec::new()
    };
    let embed = if kind == "picture" {
        child(n, P, "blipFill")
            .and_then(|b| child(b, A, "blip"))
            .and_then(|b| b.attribute((R, "embed")))
            .map(str::to_owned)
    } else {
        None
    };
    ShapeInfo {
        id: c_nv_pr
            .and_then(|c| c.attribute("id"))
            .and_then(|v| v.trim().parse().ok()),
        kind,
        name: c_nv_pr
            .and_then(|c| c.attribute("name"))
            .unwrap_or("")
            .to_owned(),
        descr: c_nv_pr
            .and_then(|c| c.attribute("descr"))
            .map(str::to_owned),
        hidden: c_nv_pr
            .and_then(|c| c.attribute("hidden"))
            .is_some_and(|v| v == "1" || v == "true"),
        xfrm: xfrm_node(n).map(read_xfrm),
        text,
        placeholder: ph,
        embed,
        children,
        range: outer.range(),
    }
}

/// Every shape in a tree, back to front.
pub fn read_shapes(doc: &Document<'_>) -> Vec<ShapeInfo> {
    let Some(tree) = tree_node(doc.root_element()) else {
        return Vec::new();
    };
    tree.children()
        .filter(|c| {
            c.is_element()
                && !is(*c, P, "nvGrpSpPr")
                && !is(*c, P, "grpSpPr")
                && !is(*c, P, "extLst")
        })
        .map(read_shape)
        .collect()
}

/// The text of a part's placeholder of `kind` (`title`, `ctrTitle`, `body`), if it has one.
pub fn placeholder_text(shapes: &[ShapeInfo], kinds: &[&str]) -> Option<String> {
    shapes.iter().find_map(|s| {
        let p = s.placeholder.as_ref()?;
        let k = p.kind.as_deref().unwrap_or("obj");
        if kinds.contains(&k) {
            s.text.clone()
        } else {
            None
        }
    })
}

/// Find the shape element with `cNvPr` id `id` anywhere in the tree (inside groups too); for
/// an `mc:AlternateContent`, the wrapper.
pub fn find_shape<'a, 'i>(doc: &'a Document<'i>, id: u32) -> Option<Node<'a, 'i>> {
    let tree = tree_node(doc.root_element())?;
    fn walk<'a, 'i>(n: Node<'a, 'i>, id: u32) -> Option<Node<'a, 'i>> {
        for c in n.children().filter(|c| c.is_element()) {
            if is(c, P, "nvGrpSpPr") || is(c, P, "grpSpPr") || is(c, P, "extLst") {
                continue;
            }
            let e = effective(c);
            let own = nv(e)
                .and_then(|v| {
                    v.children()
                        .find(|x| x.is_element() && x.tag_name().name() == "cNvPr")
                })
                .and_then(|x| x.attribute("id"))
                .and_then(|v| v.trim().parse::<u32>().ok());
            if own == Some(id) {
                return Some(c);
            }
            if is(e, P, "grpSp") {
                if let Some(f) = walk(e, id) {
                    return Some(f);
                }
            }
        }
        None
    }
    walk(tree, id)
}
