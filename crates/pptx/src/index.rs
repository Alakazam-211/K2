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

use std::io::Read;

use k2_mpc_base::index::{
    cut_text, Digest, DraftNode, IndexDraft, IndexLimits, IndexRefused, IndexVerbs, NodeRect,
    INDEX_ATTR_VALUE_MAX, INDEX_NAME_MAX,
};

use crate::deck::{rels_of, target_of, Pptx};
use crate::slide::{self, placeholder_text, ShapeInfo};
use crate::xmlx::{cut, parse_xml};
use crate::{Decline, Limits};

/// The pack's name in the index's key.
pub const PACK: &str = "pptx";
/// The reader's output format: bump when what a file reads to changes, and the kit recomputes.
pub const FORMAT: u32 = 1;
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
    Ok(b.draft)
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
