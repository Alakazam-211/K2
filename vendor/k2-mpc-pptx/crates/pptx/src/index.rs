//! The structure index: one version of a deck read into the kit's nodes ([`IndexDraft`]).
//!
//! **Nodes and keys** (the kit's address grammar, `@kind:id/kind:id`):
//!
//! | Kind | Key | Parent |
//! |---|---|---|
//! | `slide` | `@sld:<sldId>` | none, in presentation order |
//! | a shape (`placeholder`, `textbox`, `shape`, `picture`, `table`, `chart`, `diagram`, `ole`, `graphic`, `group`, `connector`, `ink`, `other`) | `@sld:<sldId>/sp:<cNvPr id>` | its slide, or its group |
//! | `notes` | `@notes:<sldId>` | its slide |
//! | `master` | `@master:<sldMasterId>` | none, after the slides |
//! | `layout` | `@layout:<sldLayoutId>` | its master |
//!
//! Every key is the file's own id: a slide's `sldId` survives reordering, and a shape's
//! `cNvPr` id is unique on its slide. A shape without an id, or with one another shape on the
//! slide also has, is handed to the kit without a key or with the repeated one, and the kit
//! mints its key. Shapes are listed front to back (the kit's "top to bottom"); `z` is the
//! position in the shape tree (0 at the back).
//!
//! **What a node holds:** its name (a slide's: its title's text), bounds from its transform in
//! EMU, its plain text (cut to the kit's limit, flagged `text_truncated`), a placeholder's type
//! and index, alt text; the record digest is the shape's XML, and a picture's content digest
//! its image's bytes. A slide's `creation_id` is a weak id. Nothing a node holds is styling.
//!
//! **A container's own record** (the kit's row format 3, declared with
//! `IndexSpec::with_own_records`, stored under the kit's offset format): a slide's record
//! digest is its whole XML and a group's its whole `p:grpSp`, children included, so each also
//! carries its own record's digest ([`DraftNode::own`]): that XML with each child shape's
//! element replaced by a stand-in naming the child (its id, else its place). An edit to one
//! shape then leaves its slide's own record as it was, and the kit's overlap checks (a revert,
//! `--base`) compare the shapes themselves; a change of the slide's own (the shapes' order, an
//! added or removed shape, its background, transition or timing) changes it.
//!
//! **The view items** (the kit's row format 2, from [`FORMAT`] 2): what the view index of a
//! version draws that the fields above don't say, worked out here, where the file is parsed
//! (a host's index worker), by the same code the live tier's derive hook uses
//! ([`crate::view::read_open`] and [`crate::view::place`], through
//! [`crate::live::view_derive`]), so the blob tier's view and the live one agree by
//! construction. The deck's fonts; per slide its render digest (keyed by the kit), its fonts
//! and field kinds; per shape its box on the page (a group's children composed, a placeholder's
//! inherited box), field kinds, the text a view shows (a table's rows tab-separated, a slide
//! number at the slide's position) and whether `shape text` can set it. A shape with no id, or
//! with one another shape on the slide also has, gets none (it shows fewer details, never
//! another shape's), and so does a deck the view can't read.
// The kit's view structs are built with `..X::default()` (k2-mpc-base CHANGELOG): the kit
// adds fields to them, and the update is what keeps a new one from breaking the build.
#![allow(clippy::needless_update)]

use std::collections::HashMap;
use std::io::Read;

use k2_mpc_base::index::{
    cut_text, Digest, DraftNode, IndexDraft, IndexLimits, IndexRefused, IndexVerbs, NodeRect,
    NodeView, ViewGeom, INDEX_ATTR_VALUE_MAX, INDEX_FONTS_MAX, INDEX_NAME_MAX,
};
use k2_mpc_base::view_index as kv;

use crate::deck::{rels_of, target_of, Pptx};
use crate::live::model::TextEdit;
use crate::slide::{self, placeholder_text, ShapeInfo};
use crate::view;
use crate::xmlx::{cut, parse_xml};
use crate::{Decline, Limits};

/// The pack's name in the index's key.
pub const PACK: &str = "pptx";
/// The reader's output format: bump when what a file reads to changes, and the kit recomputes.
/// 2: the view items (the kit's row format 2). The own record digests (the kit's row format 3)
/// need no bump: the spec declares them (`IndexSpec::with_own_records`) and the kit stores them
/// under its offset format, where no older kit reads.
pub const FORMAT: u32 = 2;
/// The format before the view items, whose rows (the same nodes) format 2 reads with the items
/// absent until a version is indexed again ([`k2_mpc_base::index::IndexSpec::with_older_format`]).
pub const OLDER_FORMAT: u32 = 1;
/// The index verbs' names.
pub const VERBS: IndexVerbs = IndexVerbs::new(
    "shape list",
    "shape show",
    "shape diff",
    "shape blame",
    None,
);

fn unreadable(why: &str) -> IndexRefused {
    IndexRefused::Unreadable {
        why: why.to_owned(),
    }
}

/// A refusal for the kit: a file that isn't a deck is `Unsupported`; one this pack declines
/// is `Unreadable` with the decline's code only (no file content).
pub fn refusal(d: &Decline) -> IndexRefused {
    match d.code {
        "not_pptx" | "no_presentation" | "template" => IndexRefused::Unsupported,
        c => IndexRefused::Unreadable {
            why: format!("declined: {c}"),
        },
    }
}

/// Read the file `src` holds into a draft, under `limits` and the package layer's `pl`.
pub fn index_pptx(
    src: &mut dyn Read,
    size: u64,
    limits: &IndexLimits,
    pl: &Limits,
) -> Result<IndexDraft, IndexRefused> {
    let cap = pl.max_file as u64;
    if size > cap {
        return Err(unreadable("the file is over the size limit"));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    src.take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(IndexRefused::Read)?;
    if bytes.len() as u64 > cap {
        return Err(unreadable("the file is over the size limit"));
    }
    let p = Pptx::open(&bytes, pl).map_err(|d| refusal(&d))?;
    draft(&p, limits, pl)
}

struct Builder<'l> {
    draft: IndexDraft,
    limits: &'l IndexLimits,
}

impl Builder<'_> {
    fn push(&mut self, node: DraftNode, depth: u32) -> Result<u32, IndexRefused> {
        if self.draft.nodes.len() as u64 >= u64::from(self.limits.max_nodes) {
            return Err(IndexRefused::TooManyNodes {
                count: self.draft.nodes.len() as u64 + 1,
                limit: self.limits.max_nodes,
            });
        }
        if depth > u32::from(self.limits.max_depth) {
            return Err(IndexRefused::TooDeep {
                depth,
                limit: self.limits.max_depth,
            });
        }
        self.draft.nodes.push(node);
        Ok(self.draft.nodes.len() as u32 - 1)
    }

    fn text(&self, n: &mut DraftNode, text: &str) {
        let (t, cut_) = cut_text(text, self.limits.max_text_bytes);
        n.text = Some(t.to_owned());
        if cut_ {
            n.flags.insert("text_truncated".into());
        }
    }
}

fn attr(n: &mut DraftNode, k: &str, v: &str) {
    n.attrs
        .insert(k.to_owned(), cut(v, INDEX_ATTR_VALUE_MAX).to_owned());
}

fn rect(x: &slide::Xfrm) -> NodeRect {
    NodeRect {
        left: x.x,
        top: x.y,
        right: x.x.saturating_add(x.cx),
        bottom: x.y.saturating_add(x.cy),
    }
}

/// The draft of an open deck.
pub fn draft(p: &Pptx<'_>, limits: &IndexLimits, pl: &Limits) -> Result<IndexDraft, IndexRefused> {
    let mut b = Builder {
        draft: IndexDraft::new("pptx-1"),
        limits,
    };
    let deck = &p.deck;
    b.draft
        .attrs
        .insert("slides".into(), deck.slides.len().to_string());
    if let Some((cx, cy)) = deck.size {
        b.draft.attrs.insert("width".into(), cx.to_string());
        b.draft.attrs.insert("height".into(), cy.to_string());
    }
    if deck.sections {
        b.draft.attrs.insert("sections".into(), "yes".into());
    }
    let layout_ids: std::collections::BTreeMap<&str, (Option<u64>, &str)> = deck
        .layouts()
        .map(|(_, l)| (l.part.as_str(), (l.id, l.name.as_str())))
        .collect();
    let decline = |d: Decline| refusal(&d);

    for (pos, s) in deck.slides.iter().enumerate() {
        let text = p.text(&s.part).map_err(decline)?;
        let doc = parse_xml(text, pl).map_err(decline)?;
        let shapes = slide::read_shapes(&doc);
        let title = placeholder_text(&shapes, &["title", "ctrTitle"]).unwrap_or_default();
        let mut n = DraftNode::new(
            "slide",
            cut(&title, INDEX_NAME_MAX).to_owned(),
            Digest::of(text.as_bytes()),
        );
        n.key = Some(format!("@sld:{}", s.id));
        n.own = own_digest(text, 0..text.len(), &shapes);
        n.visible = !s.hidden;
        attr(&mut n, "position", &(pos + 1).to_string());
        attr(&mut n, "part", &s.name);
        if let Some((id, name)) = s.layout.as_deref().and_then(|l| layout_ids.get(l)) {
            if let Some(id) = id {
                attr(&mut n, "layout", &format!("@layout:{id}"));
            }
            attr(&mut n, "layout_name", name);
        }
        if let Some(c) = doc
            .root_element()
            .descendants()
            .find(|d| d.is_element() && d.tag_name().name() == "creationId")
            .and_then(|c| c.attribute("val"))
        {
            n.weak.insert("creation_id".into(), cut(c, 64).to_owned());
        }
        let slide_ix = b.push(n, 0)?;
        let rels = rels_of(&p.opc, &s.part, pl).map_err(decline)?;
        let image = |rid: &str| {
            target_of(&p.opc, &rels, &s.part, rid)
                .and_then(|(k, _)| p.opc.content(&k).map(Digest::of))
        };
        for (z, sh) in shapes.iter().enumerate().rev() {
            add_shape(&mut b, s.id, sh, z, slide_ix, 1, text, &image)?;
        }
        if let Some(nk) = &s.notes {
            let nt = p.text(nk).map_err(decline)?;
            let ndoc = parse_xml(nt, pl).map_err(decline)?;
            let nshapes = slide::read_shapes(&ndoc);
            let body = placeholder_text(&nshapes, &["body"]).unwrap_or_default();
            let mut n = DraftNode::new("notes", "", Digest::of(nt.as_bytes()));
            n.key = Some(format!("@notes:{}", s.id));
            n.parent = Some(slide_ix);
            b.text(&mut n, &body);
            attr(&mut n, "part", &p.name_of(nk));
            b.push(n, 1)?;
        }
    }

    for m in &deck.masters {
        let text = p.text(&m.part).map_err(decline)?;
        let mut n = DraftNode::new("master", "", Digest::of(text.as_bytes()));
        n.key = m.id.map(|id| format!("@master:{id}"));
        attr(&mut n, "part", &p.name_of(&m.part));
        let mix = b.push(n, 0)?;
        for l in &m.layouts {
            let lt = p.text(&l.part).map_err(decline)?;
            let mut n = DraftNode::new(
                "layout",
                cut(&l.name, INDEX_NAME_MAX).to_owned(),
                Digest::of(lt.as_bytes()),
            );
            n.key = l.id.map(|id| format!("@layout:{id}"));
            n.parent = Some(mix);
            attr(&mut n, "part", &p.name_of(&l.part));
            if let Some(k) = &l.kind {
                attr(&mut n, "type", k);
            }
            b.push(n, 1)?;
        }
    }
    let mut d = b.draft;
    add_view_items(p, &mut d, limits, pl);
    Ok(d)
}

/// The kit's word for a shape's text edit.
fn editable(e: TextEdit) -> kv::EditableText {
    match e {
        TextEdit::Plain => kv::EditableText::Plain,
        TextEdit::Formatted => kv::EditableText::Formatted,
        TextEdit::Never => kv::EditableText::Never,
    }
}

/// A placed shape's view items: its box, field kinds, text (cut to the index's text limit) and
/// whether `shape text` can set it (`edit`, a top-level shape's; a group's child never).
fn unit_view(u: &view::Unit, edit: Option<TextEdit>, limits: &IndexLimits) -> NodeView {
    let mut v = NodeView::new();
    v.geom = u.bbox.map(|b| ViewGeom {
        bbox: kv::Bbox {
            x: b.x,
            y: b.y,
            w: b.w,
            h: b.h,
            ..kv::Bbox::default()
        },
        rot: u.rot,
        flip: kv::Flip {
            h: u.flip_h,
            v: u.flip_v,
            ..kv::Flip::default()
        },
        inherited: u.bbox_inherited,
        approx: u.approx_box,
    });
    v.editable_text = match u.live {
        true => edit.map(editable),
        false => Some(kv::EditableText::Never),
    };
    v.fields = u.fields.iter().map(|f| (*f).to_owned()).collect();
    if let Some(t) = &u.text {
        let (kept, was_cut) = cut_text(&t.summary, limits.max_text_bytes);
        v.text = Some(kept.to_owned());
        v.text_truncated = t.truncated || was_cut;
    }
    v
}

/// The view items of an open deck on its draft's nodes, by key, from the pack's view of it
/// (the module's **view items**). Nothing is added when the view can't read the deck or its
/// fonts don't fit the kit's limits; a key two nodes (or two shapes) share gets none.
fn add_view_items(p: &Pptx<'_>, d: &mut IndexDraft, limits: &IndexLimits, pl: &Limits) {
    let Ok(read) = view::read_open(p, pl) else {
        return;
    };
    let vi = read.index();
    if vi.fonts.len() > INDEX_FONTS_MAX
        || vi
            .fonts
            .iter()
            .any(|f| f.family.is_empty() || f.family.len() > INDEX_NAME_MAX)
    {
        return;
    }
    let mut edit: HashMap<String, TextEdit> = HashMap::new();
    for s in &read.slides {
        for r in &s.shapes {
            if let Some(id) = r.id {
                edit.insert(format!("@sld:{}/sp:{id}", s.sid), r.edit);
            }
        }
    }
    // each key once: a shared one names no single shape
    let mut units: HashMap<&str, Option<&view::Unit>> = HashMap::new();
    for u in &vi.units {
        if let Some(k) = u.key.as_deref() {
            units.entry(k).and_modify(|e| *e = None).or_insert(Some(u));
        }
    }
    let mut slides: HashMap<String, Option<&view::Container>> = HashMap::new();
    for c in &vi.containers {
        slides
            .entry(c.key.clone())
            .and_modify(|e| *e = None)
            .or_insert(Some(c));
    }
    let mut seen: HashMap<String, u32> = HashMap::new();
    for n in &d.nodes {
        if let Some(k) = &n.key {
            *seen.entry(k.clone()).or_default() += 1;
        }
    }
    for n in &mut d.nodes {
        let Some(k) = n.key.as_deref() else { continue };
        if seen.get(k) != Some(&1) {
            continue;
        }
        if n.kind == "slide" {
            if let Some(Some(c)) = slides.get(k) {
                let mut v = NodeView::new();
                v.fonts = c.fonts.iter().map(|i| *i as u32).collect();
                v.fields = c.has_fields.iter().map(|f| (*f).to_owned()).collect();
                n.view = Some(v);
                n.render = Some(c.render_hash);
            }
        } else if let Some(Some(u)) = units.get(k) {
            n.view = Some(unit_view(u, edit.get(k).copied(), limits));
        }
    }
    d.fonts = vi
        .fonts
        .iter()
        .map(|f| kv::FontUse {
            family: f.family.clone(),
            script: f.script.to_owned(),
            from: f.from.to_owned(),
            embedded: f.embedded,
            ..kv::FontUse::default()
        })
        .collect();
}

/// A container's own record ([`DraftNode::own`]): `span` of `text` (a slide's XML, a group's
/// element) with each of its child shapes' elements replaced by a stand-in naming the child (its
/// id, else its place among them), in document order. `None` when a child's element doesn't lie
/// inside `span`, one after another: the kit then counts any change to the record as the
/// container's own.
fn own_digest(text: &str, span: std::ops::Range<usize>, kids: &[ShapeInfo]) -> Option<Digest> {
    let mut ks: Vec<(usize, &ShapeInfo)> = kids.iter().enumerate().collect();
    ks.sort_by_key(|(_, k)| k.range.start);
    let mut d = Digest::stream();
    let mut at = span.start;
    for (place, k) in ks {
        let r = &k.range;
        if r.start < at || r.end > span.end || r.start > r.end {
            return None;
        }
        d.update(text.get(at..r.start)?.as_bytes());
        let stand_in = match k.id {
            Some(id) => format!("\u{0}sp:{id}\u{0}"),
            None => format!("\u{0}#{place}\u{0}"),
        };
        d.update(stand_in.as_bytes());
        at = r.end;
    }
    d.update(text.get(at..span.end)?.as_bytes());
    Some(d.finish())
}

#[allow(clippy::too_many_arguments)]
fn add_shape(
    b: &mut Builder<'_>,
    sld: u32,
    sh: &ShapeInfo,
    z: usize,
    parent: u32,
    depth: u32,
    text: &str,
    image: &dyn Fn(&str) -> Option<Digest>,
) -> Result<(), IndexRefused> {
    let xml = &text[sh.range.clone()];
    let mut n = DraftNode::new(
        sh.kind,
        cut(&sh.name, INDEX_NAME_MAX).to_owned(),
        Digest::of(xml.as_bytes()),
    );
    n.key = sh.id.map(|id| format!("@sld:{sld}/sp:{id}"));
    if !sh.children.is_empty() {
        n.own = own_digest(text, sh.range.clone(), &sh.children);
    }
    n.parent = Some(parent);
    n.visible = !sh.hidden;
    n.bounds = sh.xfrm.as_ref().map(rect);
    if let Some(t) = &sh.text {
        b.text(&mut n, t);
    }
    attr(&mut n, "z", &z.to_string());
    if let Some(x) = &sh.xfrm {
        if x.rot != 0 {
            attr(&mut n, "rot", &x.rot.to_string());
        }
    }
    if let Some(ph) = &sh.placeholder {
        attr(&mut n, "placeholder", ph.kind.as_deref().unwrap_or("obj"));
        if let Some(i) = &ph.idx {
            attr(&mut n, "idx", i);
        }
    }
    if let Some(d) = &sh.descr {
        attr(&mut n, "descr", d);
    }
    if let Some(rid) = &sh.embed {
        n.content = image(rid);
    }
    let me = b.push(n, depth)?;
    for (cz, c) in sh.children.iter().enumerate().rev() {
        add_shape(b, sld, c, cz, me, depth + 1, text, image)?;
    }
    Ok(())
}
