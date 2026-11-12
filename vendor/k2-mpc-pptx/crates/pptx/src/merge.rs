//! The three-way merge of two saves of a deck, by shape: `ours` is the doc's current version
//! (where agents' edits land), `theirs` what a person's app saved, `base` what both started
//! from.
//!
//! **The package first** (the shared layer, [`k2_mpc_opc::merge`]): two saves that each add a
//! slide pick the same next part name (`slide5.xml`) and relationship id (`rId9`); ours' are
//! renamed and re-minted, with every reference on ours' side rewritten. Content types and
//! relationships merge by key. Every part one side changed takes that side's part, byte for
//! byte. A part both sides changed comes here ([`PartMerge`]):
//!
//! - **the presentation** (`ppt/presentation.xml`): slides by `sldId`. Slides one side added
//!   are kept; two different slides added under one `sldId` keep theirs' and get ours' a new
//!   one (one past every id either side uses, as the app numbers them), with ours' section
//!   entries rewritten. A slide one side removed and the other left alone is removed. The
//!   order follows the side that reordered; both reordering is a conflict. The rest of the
//!   part merges whole.
//! - **a slide or a notes slide**: shapes by their `cNvPr` id, the shape the unit. A shape one
//!   side changed (moved, resized, retexted, restyled) takes that side's shape whole. Two
//!   different shapes added under one id are both kept, ours' with new ids (and ours'
//!   connectors and animations that point at them rewritten). The z-order follows the side
//!   that reordered. The slide's own settings (timing, transition, background) merge whole.
//! - `docProps/*` and `ppt/viewProps.xml` are metadata and view state: the later save's.
//! - any other part (a master, a layout, a theme, a chart): a conflict.
//!
//! **Never last-writer-wins.** When both sides change one shape, or one deletes what the other
//! changed, the person's stays in the file (theirs: the save of their app, which may still
//! have it open) and the agent's is **held** as a [`ShapeConflict`] to take later. Anything
//! this merge can't place for sure (both sides reordering, both changing a slide's own
//! settings or a slide list entry, a slide part that holds a different slide on two sides, a
//! repeated id, a result that wouldn't open) is a whole-file conflict: the kit keeps both, as
//! a version and a conflict copy.
//!
//! **Byte for byte where it can be:** a part both sides changed is spliced from the sides'
//! own bytes: each shape is the bytes of the side it came from (with only re-minted ids
//! rewritten), and the slide's frame is one side's bytes. Every other part is a side's raw
//! record.
//!
//! **The base.** The kit hands over what the file last held; choosing the version the app
//! opened instead (the kit's planned outside-save bridge) plugs in at
//! [`crate::pack::PptxPack::merge3_since`], where the stale guards stand until then.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::Mutex;

use k2_mpc_base::index::Digest;
use k2_mpc_opc::merge::{self as opc, PartMerge, Report, Resolution};
use k2_mpc_opc::{Change, OpcPackage};
use roxmltree::Document;

use crate::deck::{rels_of, Pptx, REL_LAYOUT, REL_NOTES, SLIDE_ID_MAX};
use crate::mutant;
use crate::slide::{Tree, Unit};
use crate::xmlx::{self, child, is, parse_xml, start_tag_end, A, P, R};
use crate::{Decline, Limits};

/// Why a shape's change is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConflictWhy {
    /// The person and the agent both changed it, differently.
    BothChanged,
    /// The person deleted it; the agent changed it.
    PersonDeleted,
    /// The agent deleted it; the person changed it.
    AgentDeleted,
}

/// One held conflict: a shape, as the base, the person (in the file) and the agent had it.
/// `None` is "not there" (deleted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeConflict {
    /// Stable: the same clash has the same id.
    pub id: String,
    /// `@sld:<sldId>/sp:<id>`, or `@notes:<sldId>/sp:<id>` for a notes slide's shape.
    pub address: String,
    pub why: ConflictWhy,
    pub base: Option<String>,
    pub person: Option<String>,
    pub agent: Option<String>,
}

/// What a merge made.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Merged {
    /// The merged file.
    pub bytes: Vec<u8>,
    /// The package layer's renames and re-minted relationship ids, on ours' side.
    pub report: Report,
    /// Ours' slides given a new `sldId`: (old, new).
    pub slides_reminted: Vec<(u32, u32)>,
    /// Ours' shape ids re-minted: (the slide's address, old, new).
    pub shapes_reminted: Vec<(String, u32, u32)>,
    /// The agent's changes held, the person's kept.
    pub conflicts: Vec<ShapeConflict>,
}

/// What [`merge3`] came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Merged(Merged),
    /// The changes can't be placed for sure; why. The kit keeps both.
    Conflict(String),
    /// One of the files is one this pack doesn't hold.
    Unsupported(Decline),
}

/// Merge two saves of a deck. A pure function of its inputs.
pub fn merge3(base: &[u8], ours: &[u8], theirs: &[u8], limits: &Limits) -> Outcome {
    let plain = |b: &[u8]| {
        Outcome::Merged(Merged {
            bytes: b.to_vec(),
            ..Merged::default()
        })
    };
    if ours == theirs || base == theirs {
        return plain(ours);
    }
    if base == ours {
        return plain(theirs);
    }
    let open = |b| Pptx::open(b, limits);
    let (b, o, t) = match (open(base), open(ours), open(theirs)) {
        (Ok(b), Ok(o), Ok(t)) => (b, o, t),
        (Err(d), _, _) | (_, Err(d), _) | (_, _, Err(d)) => return Outcome::Unsupported(d),
    };
    match merge_open(&b, &o, &t, limits) {
        Ok(m) => Outcome::Merged(m),
        Err(why) => Outcome::Conflict(why),
    }
}

/// The slide (by id) each notes slide part belongs to.
fn notes_owner(p: &Pptx<'_>) -> BTreeMap<String, u32> {
    p.deck
        .slides
        .iter()
        .filter_map(|s| s.notes.clone().map(|n| (n, s.id)))
        .collect()
}

/// Each part a slide or notes slide lives in must hold the same slide on every side that has
/// it, and a slide must stay in its part: an app that renumbered its parts would otherwise
/// pair two different slides by name.
fn same_slides_in_parts(b: &Pptx<'_>, o: &Pptx<'_>, t: &Pptx<'_>) -> Result<(), String> {
    if mutant::active("pptx_no_part_check") {
        return Ok(());
    }
    let bs = b.deck.slide_of_part();
    let bn = notes_owner(b);
    let b_part: BTreeMap<u32, &str> = b
        .deck
        .slides
        .iter()
        .map(|s| (s.id, s.part.as_str()))
        .collect();
    for side in [o, t] {
        for (map_b, map_s, what) in [
            (&bs, side.deck.slide_of_part(), "slide"),
            (&bn, notes_owner(side), "notes slide"),
        ] {
            for (k, id) in map_b {
                if let Some(i) = map_s.get(k) {
                    if i != id {
                        return Err(format!(
                            "{k} is the {what} of slide {id} in the base and of slide {i} in a save (its parts were renumbered)"
                        ));
                    }
                }
            }
        }
        for s in &side.deck.slides {
            if let Some(bp) = b_part.get(&s.id) {
                if *bp != s.part {
                    return Err(format!(
                        "slide {} moved from {bp} to {} in a save (its parts were renumbered)",
                        s.id, s.part
                    ));
                }
            }
        }
    }
    if b.main != o.main || b.main != t.main {
        return Err("the presentation part moved".into());
    }
    Ok(())
}

fn merge_open(b: &Pptx<'_>, o: &Pptx<'_>, t: &Pptx<'_>, limits: &Limits) -> Result<Merged, String> {
    same_slides_in_parts(b, o, t)?;
    let mut scopes = BTreeMap::new();
    for side in [t, o, b] {
        for s in &side.deck.slides {
            scopes.insert(s.part.clone(), format!("@sld:{}", s.id));
            if let Some(n) = &s.notes {
                scopes.insert(n.clone(), format!("@notes:{}", s.id));
            }
        }
    }
    let parts = PptxParts {
        limits: *limits,
        main: b.main.clone(),
        scopes,
        held: RefCell::new(Vec::new()),
        slides_reminted: RefCell::new(Vec::new()),
        shapes_reminted: RefCell::new(Vec::new()),
    };
    let (m, report) = opc::merge3(&b.opc, &o.opc, &t.opc, &parts, limits)?;
    let bytes = match m {
        opc::Merged::Ours => o.opc.bytes().to_vec(),
        opc::Merged::Theirs => t.opc.bytes().to_vec(),
        opc::Merged::New(v) => v,
    };
    if !mutant::active("pptx_no_result_check") {
        let r = Pptx::open(&bytes, limits)
            .map_err(|d| format!("the merge wouldn't open as a deck: {d}"))?;
        check_result(&r, limits)?;
    }
    Ok(Merged {
        bytes,
        report,
        slides_reminted: parts.slides_reminted.into_inner(),
        shapes_reminted: parts.shapes_reminted.into_inner(),
        conflicts: parts.held.into_inner(),
    })
}

/// What a merged deck may not hold though each side's part is sound: a slide with two layouts
/// or two notes slides (both sides gave it one), or a notes slide two slides share.
fn check_result(p: &Pptx<'_>, limits: &Limits) -> Result<(), String> {
    let mut owner: BTreeMap<&str, u32> = BTreeMap::new();
    for s in &p.deck.slides {
        let rels = rels_of(&p.opc, &s.part, limits).map_err(|d| d.to_string())?;
        for (ty, what) in [(REL_LAYOUT, "layouts"), (REL_NOTES, "notes slides")] {
            let n = rels.list.iter().filter(|r| r.rel_type == ty).count();
            if n > 1 {
                return Err(format!("slide {} would have {n} {what}", s.id));
            }
        }
        if let Some(n) = &s.notes {
            if let Some(prev) = owner.insert(n.as_str(), s.id) {
                return Err(format!(
                    "slides {prev} and {} would share a notes slide",
                    s.id
                ));
            }
        }
    }
    Ok(())
}

/// The pack's half of the package merge.
struct PptxParts {
    limits: Limits,
    main: String,
    /// Slide and notes slide parts, by key: their address (`@sld:256`, `@notes:256`).
    scopes: BTreeMap<String, String>,
    held: RefCell<Vec<ShapeConflict>>,
    slides_reminted: RefCell<Vec<(u32, u32)>>,
    shapes_reminted: RefCell<Vec<(String, u32, u32)>>,
}

impl PartMerge for PptxParts {
    fn both_changed(
        &self,
        key: &str,
        base: Option<&[u8]>,
        ours: &[u8],
        theirs: &[u8],
    ) -> Resolution {
        let r = if key == self.main {
            self.merge_presentation(base, ours, theirs)
        } else if let Some(scope) = self.scopes.get(key) {
            self.merge_tree(scope, base, ours, theirs)
        } else if key.starts_with("docprops/") || key.ends_with("/viewprops.xml") {
            Ok(Resolution::Theirs)
        } else {
            Err(format!("both sides changed {key}"))
        };
        r.unwrap_or_else(Resolution::Conflict)
    }
}

/// Which side a piece of the result comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Side {
    Ours,
    Theirs,
}

/// An item's identity across the sides: an id all sides mean the same thing by, or ours' own
/// item that collided with a different item of theirs under one id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Shared(u32),
    OursNew(u32),
}

/// The result's order: the side that reordered the items all sides kept leads, and items only
/// the other side places go after their nearest predecessor on that side.
fn order(
    rb: &[Key],
    seq_o: &[Key],
    seq_t: &[Key],
    keep: &BTreeSet<Key>,
) -> Result<Vec<Key>, String> {
    let in_all: BTreeSet<Key> = {
        let o: BTreeSet<Key> = seq_o.iter().copied().collect();
        let t: BTreeSet<Key> = seq_t.iter().copied().collect();
        rb.iter()
            .copied()
            .filter(|k| o.contains(k) && t.contains(k) && keep.contains(k))
            .collect()
    };
    let f = |s: &[Key]| -> Vec<Key> { s.iter().copied().filter(|k| in_all.contains(k)).collect() };
    let (cb, co, ct) = (f(rb), f(seq_o), f(seq_t));
    let (lead, other) = if co == cb || co == ct || mutant::active("pptx_no_order") {
        (seq_t, seq_o)
    } else if ct == cb {
        (seq_o, seq_t)
    } else {
        return Err("both sides reordered".into());
    };
    let mut out: Vec<Key> = lead.iter().copied().filter(|k| keep.contains(k)).collect();
    let mut placed: BTreeSet<Key> = out.iter().copied().collect();
    let mut prev: Option<Key> = None;
    for k in other {
        if keep.contains(k) && !placed.contains(k) {
            let at = match prev {
                Some(p) => out.iter().position(|x| *x == p).map_or(0, |i| i + 1),
                None => 0,
            };
            out.insert(at, *k);
            placed.insert(*k);
        }
        if placed.contains(k) {
            prev = Some(*k);
        }
    }
    Ok(out)
}

/// One side's part, parsed.
struct Side3<'x> {
    text: &'x str,
    doc: Document<'x>,
}

fn side_text<'x>(what: &str, b: &'x [u8]) -> Result<&'x str, String> {
    std::str::from_utf8(b).map_err(|_| format!("{what} isn't UTF-8"))
}

/// Edits that rewrite re-minted ids inside `range` of a side's document: a shape's own
/// `cNvPr` id, a connector's ends, an animation's target.
fn remint_edits(
    doc: &Document<'_>,
    range: Range<usize>,
    map: &BTreeMap<u32, u32>,
) -> Vec<(Range<usize>, String)> {
    let mut out = Vec::new();
    if map.is_empty() || mutant::active("pptx_no_ref_rewrite") {
        return out;
    }
    for n in doc.descendants().filter(|n| n.is_element()) {
        let r = n.range();
        if r.start < range.start || r.end > range.end {
            continue;
        }
        let local = n.tag_name().name();
        for a in n.attributes() {
            if a.namespace().is_some() {
                continue;
            }
            let hit = (local == "cNvPr" && a.name() == "id")
                || ((is(n, A, "stCxn") || is(n, A, "endCxn")) && a.name() == "id")
                || a.name() == "spid";
            if !hit {
                continue;
            }
            if let Some(new) = a
                .value()
                .trim()
                .parse::<u32>()
                .ok()
                .and_then(|v| map.get(&v))
            {
                out.push((a.range_value(), new.to_string()));
            }
        }
    }
    out
}

/// `text[range]` with `edits` (inside it) spliced.
fn spliced(text: &str, range: Range<usize>, edits: &[(Range<usize>, String)]) -> String {
    let local: Vec<(Range<usize>, String)> = edits
        .iter()
        .filter(|(r, _)| r.start >= range.start && r.end <= range.end)
        .map(|(r, s)| ((r.start - range.start)..(r.end - range.start), s.clone()))
        .collect();
    xmlx::splice(&text[range], local)
}

fn conflict_id(address: &str, parts: [Option<&str>; 3]) -> String {
    let mut seed = format!("{}:{address}", address.len());
    for p in parts {
        match p {
            None => seed.push_str("\n-"),
            Some(s) => seed.push_str(&format!("\n{}:{s}", s.len())),
        }
    }
    xmlx::hex(Digest::of(seed.as_bytes()).as_bytes())[..12].to_owned()
}

impl PptxParts {
    fn parse<'x>(&self, what: &str, b: &'x [u8]) -> Result<Side3<'x>, String> {
        let text = side_text(what, b)?;
        let doc = parse_xml(text, &self.limits).map_err(|e| format!("{what}: {e}"))?;
        Ok(Side3 { text, doc })
    }

    fn hold(
        &self,
        address: String,
        why: ConflictWhy,
        b: Option<&str>,
        t: Option<&str>,
        o: Option<&str>,
    ) {
        let id = conflict_id(&address, [b, t, o]);
        self.held.borrow_mut().push(ShapeConflict {
            id,
            address,
            why,
            base: b.map(str::to_owned),
            person: t.map(str::to_owned),
            agent: o.map(str::to_owned),
        });
    }

    /// A slide or notes slide both sides changed: by shape.
    fn merge_tree(
        &self,
        scope: &str,
        base: Option<&[u8]>,
        ours: &[u8],
        theirs: &[u8],
    ) -> Result<Resolution, String> {
        let base = base.ok_or_else(|| format!("both sides added {scope} differently"))?;
        let (sb, so, st) = (
            self.parse(scope, base)?,
            self.parse(scope, ours)?,
            self.parse(scope, theirs)?,
        );
        let tree = |s: &Side3<'_>| Tree::parse(&s.doc).map_err(|e| format!("{scope}: {e}"));
        let (tb, to, tt) = (tree(&sb)?, tree(&so)?, tree(&st)?);
        for x in [&tb, &to, &tt] {
            if !x.ids_unique() {
                return Err(format!(
                    "{scope}: shape ids repeat, so its shapes can't be paired"
                ));
            }
        }
        let frame = |s: &Side3<'_>, x: &Tree| {
            (
                s.text[..x.units_start].to_owned(),
                s.text[x.units_end..].to_owned(),
            )
        };
        let (fb, fo, ft) = (frame(&sb, &tb), frame(&so, &to), frame(&st, &tt));
        let frame_side = if fo == ft || fo == fb {
            Side::Theirs
        } else if ft == fb {
            Side::Ours
        } else {
            return Err(format!(
                "{scope}: both sides changed the slide's own settings (timing, transition or background)"
            ));
        };
        let el = |s: &Side3<'_>, u: &Unit| s.text[u.range.clone()].to_owned();
        let by_id = |x: &Tree| -> BTreeMap<u32, usize> {
            x.units
                .iter()
                .enumerate()
                .filter_map(|(i, u)| u.id.map(|id| (id, i)))
                .collect()
        };
        let (mb, mo, mt) = (by_id(&tb), by_id(&to), by_id(&tt));
        let ids: BTreeSet<u32> = mb
            .keys()
            .chain(mo.keys())
            .chain(mt.keys())
            .copied()
            .collect();
        let mut source: BTreeMap<Key, (Side, usize)> = BTreeMap::new();
        for id in ids {
            let addr = || format!("{scope}/sp:{id}");
            let get = |m: &BTreeMap<u32, usize>, s: &Side3<'_>, x: &Tree| {
                m.get(&id).map(|i| (*i, el(s, &x.units[*i])))
            };
            let (b_, o_, t_) = (get(&mb, &sb, &tb), get(&mo, &so, &to), get(&mt, &st, &tt));
            let k = Key::Shared(id);
            match (b_, o_, t_) {
                (Some((_, bx)), Some((oi, ox)), Some((ti, tx))) => {
                    if ox == bx || ox == tx {
                        source.insert(k, (Side::Theirs, ti));
                    } else if tx == bx || mutant::active("pptx_both_changed_ours") {
                        source.insert(k, (Side::Ours, oi));
                    } else {
                        source.insert(k, (Side::Theirs, ti));
                        self.hold(
                            addr(),
                            ConflictWhy::BothChanged,
                            Some(bx.as_str()),
                            Some(tx.as_str()),
                            Some(ox.as_str()),
                        );
                    }
                }
                (Some((_, bx)), None, Some((ti, tx))) => {
                    if tx != bx {
                        source.insert(k, (Side::Theirs, ti));
                        self.hold(
                            addr(),
                            ConflictWhy::AgentDeleted,
                            Some(bx.as_str()),
                            Some(tx.as_str()),
                            None,
                        );
                    }
                }
                (Some((_, bx)), Some((_, ox)), None) => {
                    if ox != bx {
                        self.hold(
                            addr(),
                            ConflictWhy::PersonDeleted,
                            Some(bx.as_str()),
                            None,
                            Some(ox.as_str()),
                        );
                    }
                }
                (Some(_), None, None) => {}
                (None, Some((oi, ox)), Some((ti, tx))) => {
                    source.insert(k, (Side::Theirs, ti));
                    if ox != tx {
                        source.insert(Key::OursNew(id), (Side::Ours, oi));
                    }
                }
                (None, Some((oi, _)), None) => {
                    source.insert(k, (Side::Ours, oi));
                }
                (None, None, Some((ti, _))) => {
                    source.insert(k, (Side::Theirs, ti));
                }
                (None, None, None) => {}
            }
        }

        // ours' new ids that theirs (or the base) also uses get new ones
        let base_all = tb.all_ids();
        let theirs_all = tt.all_ids();
        let mut next = tb
            .max_id()
            .max(to.max_id())
            .max(tt.max_id())
            .checked_add(1)
            .ok_or_else(|| format!("{scope}: no shape id is left"))?;
        let mut map: BTreeMap<u32, u32> = BTreeMap::new();
        for (_, (_, i)) in source.iter().filter(|(_, (s, _))| *s == Side::Ours) {
            for id in &to.units[*i].ids {
                if !base_all.contains(id) && theirs_all.contains(id) && !map.contains_key(id) {
                    if mutant::active("pptx_no_remint_shape") {
                        continue;
                    }
                    map.insert(*id, next);
                    next = next
                        .checked_add(1)
                        .ok_or_else(|| format!("{scope}: no shape id is left"))?;
                }
            }
        }
        let edits = remint_edits(&so.doc, 0..so.text.len(), &map);

        // the order: shared ids, and ours' colliding new ones
        let seq_b: Vec<Key> = tb
            .units
            .iter()
            .filter_map(|u| u.id.map(Key::Shared))
            .collect();
        let seq_t: Vec<Key> = tt
            .units
            .iter()
            .filter_map(|u| u.id.map(Key::Shared))
            .collect();
        let seq_o: Vec<Key> = to
            .units
            .iter()
            .filter_map(|u| u.id)
            .map(|id| {
                if source.contains_key(&Key::OursNew(id)) {
                    Key::OursNew(id)
                } else {
                    Key::Shared(id)
                }
            })
            .collect();
        let keep: BTreeSet<Key> = source.keys().copied().collect();
        let seq = order(&seq_b, &seq_o, &seq_t, &keep).map_err(|e| format!("{scope}: {e}"))?;

        // assembled from the sides' own bytes
        let mut out = match frame_side {
            Side::Theirs => ft.0.clone(),
            Side::Ours => spliced(so.text, 0..to.units_start, &edits),
        };
        for k in &seq {
            let (side, i) = source[k];
            let (s, x) = match side {
                Side::Ours => (&so, &to),
                Side::Theirs => (&st, &tt),
            };
            let u = &x.units[i];
            out.push_str(&s.text[u.lead.clone()]);
            match side {
                Side::Ours => out.push_str(&spliced(so.text, u.range.clone(), &edits)),
                Side::Theirs => out.push_str(&s.text[u.range.clone()]),
            }
        }
        match frame_side {
            Side::Theirs => out.push_str(&ft.1),
            Side::Ours => out.push_str(&spliced(so.text, to.units_end..so.text.len(), &edits)),
        }

        // sound: it parses, ids are unique, every animation targets a shape that is there
        let doc = parse_xml(&out, &self.limits)
            .map_err(|e| format!("{scope}: the merge doesn't parse: {e}"))?;
        let merged = Tree::parse(&doc).map_err(|e| format!("{scope}: {e}"))?;
        if !merged.ids_unique() {
            return Err(format!("{scope}: the merge would repeat a shape id"));
        }
        let all = merged.all_ids();
        if let Some(a) = doc.descendants().filter(|n| n.is_element()).find_map(|n| {
            n.attribute("spid")
                .and_then(|v| v.trim().parse::<u32>().ok())
                .filter(|v| !all.contains(v))
        }) {
            return Err(format!(
                "{scope}: an animation would target shape {a}, which the merge doesn't hold"
            ));
        }
        for (old, new) in &map {
            self.shapes_reminted
                .borrow_mut()
                .push((scope.to_owned(), *old, *new));
        }
        Ok(Resolution::Merged(out.into_bytes()))
    }

    /// The presentation part both sides changed: its slide list by `sldId`, the rest whole.
    fn merge_presentation(
        &self,
        base: Option<&[u8]>,
        ours: &[u8],
        theirs: &[u8],
    ) -> Result<Resolution, String> {
        let base = base.ok_or("both sides added the presentation")?;
        let (sb, so, st) = (
            self.parse("the presentation", base)?,
            self.parse("the presentation", ours)?,
            self.parse("the presentation", theirs)?,
        );
        let (lb, lo, lt) = (slide_list(&sb)?, slide_list(&so)?, slide_list(&st)?);
        let frame = |s: &Side3<'_>, l: &List| {
            (
                s.text[..l.units_start].to_owned(),
                s.text[l.units_end..].to_owned(),
            )
        };
        let (fb, fo, ft) = (frame(&sb, &lb), frame(&so, &lo), frame(&st, &lt));
        let frame_side = if fo == ft || fo == fb {
            Side::Theirs
        } else if ft == fb {
            Side::Ours
        } else {
            return Err("both sides changed the presentation's own settings".into());
        };
        let el = |s: &Side3<'_>, i: &Item| s.text[i.range.clone()].to_owned();
        let by_id = |l: &List| -> BTreeMap<u32, usize> {
            l.items.iter().enumerate().map(|(i, x)| (x.id, i)).collect()
        };
        let (mb, mo, mt) = (by_id(&lb), by_id(&lo), by_id(&lt));
        let ids: BTreeSet<u32> = mb
            .keys()
            .chain(mo.keys())
            .chain(mt.keys())
            .copied()
            .collect();
        let mut source: BTreeMap<Key, (Side, usize)> = BTreeMap::new();
        for id in ids {
            let (b_, o_, t_) = (mb.get(&id), mo.get(&id), mt.get(&id));
            let k = Key::Shared(id);
            match (b_, o_, t_) {
                (Some(bi), Some(oi), Some(ti)) => {
                    let (bx, ox, tx) = (
                        el(&sb, &lb.items[*bi]),
                        el(&so, &lo.items[*oi]),
                        el(&st, &lt.items[*ti]),
                    );
                    if ox == bx || ox == tx {
                        source.insert(k, (Side::Theirs, *ti));
                    } else if tx == bx {
                        source.insert(k, (Side::Ours, *oi));
                    } else {
                        return Err(format!("both sides changed slide {id}'s entry"));
                    }
                }
                (Some(bi), None, Some(ti)) => {
                    if el(&sb, &lb.items[*bi]) != el(&st, &lt.items[*ti]) {
                        return Err(format!(
                            "one side removed slide {id}, the other changed its entry"
                        ));
                    }
                }
                (Some(bi), Some(oi), None) => {
                    if el(&sb, &lb.items[*bi]) != el(&so, &lo.items[*oi]) {
                        return Err(format!(
                            "one side removed slide {id}, the other changed its entry"
                        ));
                    }
                }
                (Some(_), None, None) => {}
                (None, Some(oi), Some(ti)) => {
                    let (ox, tx) = (&lo.items[*oi], &lt.items[*ti]);
                    source.insert(k, (Side::Theirs, *ti));
                    if el(&so, ox) != el(&st, tx) {
                        if ox.rid == tx.rid {
                            return Err(format!(
                                "both sides added slide {id} for one part, differently"
                            ));
                        }
                        source.insert(Key::OursNew(id), (Side::Ours, *oi));
                    }
                }
                (None, Some(oi), None) => {
                    source.insert(k, (Side::Ours, *oi));
                }
                (None, None, Some(ti)) => {
                    source.insert(k, (Side::Theirs, *ti));
                }
                (None, None, None) => {}
            }
        }
        // ours' slides that collided get new ids
        let mut next = lb
            .items
            .iter()
            .chain(lo.items.iter())
            .chain(lt.items.iter())
            .map(|i| i.id)
            .max()
            .unwrap_or(255)
            .max(255);
        let mut map: BTreeMap<u32, u32> = BTreeMap::new();
        for k in source.keys() {
            if let Key::OursNew(id) = k {
                if mutant::active("pptx_no_remint_slide") {
                    continue;
                }
                next = next
                    .checked_add(1)
                    .filter(|n| *n <= SLIDE_ID_MAX)
                    .ok_or("no slide id is left")?;
                map.insert(*id, next);
            }
        }
        let mut edits = Vec::new();
        for n in so
            .doc
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "sldId")
        {
            if let Some(a) = n.attribute_node("id") {
                if let Some(new) = a
                    .value()
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .and_then(|v| map.get(&v))
                {
                    edits.push((a.range_value(), new.to_string()));
                }
            }
        }
        let keyed = |l: &List, ours_side: bool| -> Vec<Key> {
            l.items
                .iter()
                .map(|i| {
                    if ours_side && source.contains_key(&Key::OursNew(i.id)) {
                        Key::OursNew(i.id)
                    } else {
                        Key::Shared(i.id)
                    }
                })
                .collect()
        };
        let keep: BTreeSet<Key> = source.keys().copied().collect();
        let (seq_b, seq_o, seq_t) = (keyed(&lb, false), keyed(&lo, true), keyed(&lt, false));
        let seq =
            order(&seq_b, &seq_o, &seq_t, &keep).map_err(|e| format!("the slide list: {e}"))?;

        // a section list names slides: it must still name exactly the result's, in order
        let (pre, post, frame_seq) = match frame_side {
            Side::Theirs => (ft.0.clone(), ft.1.clone(), seq_t.clone()),
            Side::Ours => (
                spliced(so.text, 0..lo.units_start, &edits),
                spliced(so.text, lo.units_end..so.text.len(), &edits),
                seq_o.clone(),
            ),
        };
        if (pre.contains("sectionLst") || post.contains("sectionLst")) && seq != frame_seq {
            return Err("the deck has sections, and both sides changed its slides".into());
        }
        let mut out = pre;
        for k in &seq {
            let (side, i) = source[k];
            let (s, l) = match side {
                Side::Ours => (&so, &lo),
                Side::Theirs => (&st, &lt),
            };
            let it = &l.items[i];
            out.push_str(&s.text[it.lead.clone()]);
            match side {
                Side::Ours => out.push_str(&spliced(so.text, it.range.clone(), &edits)),
                Side::Theirs => out.push_str(&s.text[it.range.clone()]),
            }
        }
        out.push_str(&post);
        let doc = parse_xml(&out, &self.limits)
            .map_err(|e| format!("the merged presentation doesn't parse: {e}"))?;
        let merged = slide_list(&Side3 { text: &out, doc })?;
        let mut ids = BTreeSet::new();
        let mut rids = BTreeSet::new();
        for i in &merged.items {
            if !ids.insert(i.id) || !rids.insert(i.rid.clone()) {
                return Err("the merged slide list would repeat a slide".into());
            }
        }
        for (old, new) in &map {
            self.slides_reminted.borrow_mut().push((*old, *new));
        }
        Ok(Resolution::Merged(out.into_bytes()))
    }
}

/// One `p:sldId`.
struct Item {
    id: u32,
    rid: String,
    range: Range<usize>,
    lead: Range<usize>,
}

/// A presentation's slide list, split like a shape tree.
struct List {
    units_start: usize,
    units_end: usize,
    items: Vec<Item>,
}

fn slide_list(s: &Side3<'_>) -> Result<List, String> {
    let root = s.doc.root_element();
    let l = child(root, P, "sldIdLst").ok_or("a side has no slide list")?;
    let (start, empty) = start_tag_end(s.text, l);
    if empty {
        return Err("a side's slide list is empty".into());
    }
    let mut items = Vec::new();
    let mut prev = start;
    for c in l.children().filter(|c| c.is_element()) {
        if !is(c, P, "sldId") {
            return Err("the slide list holds another element".into());
        }
        let id = c
            .attribute("id")
            .and_then(|v| v.trim().parse().ok())
            .ok_or("a slide id isn't a number")?;
        let rid = c.attribute((R, "id")).unwrap_or("").to_owned();
        let r = c.range();
        items.push(Item {
            id,
            rid,
            lead: prev..r.start,
            range: r.clone(),
        });
        prev = r.end;
    }
    Ok(List {
        units_start: start,
        units_end: prev,
        items,
    })
}

/// Which value a conflict's taker keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Take {
    /// The person's: already in the file; the record goes.
    Person,
    /// The agent's: written into the file.
    Agent,
}

/// Why a conflict couldn't be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TakeError {
    NotFound(String),
    /// The file's shape no longer is the person's, or the agent's ids are taken now: read it
    /// again.
    Stale(String),
    /// The current file can't be read.
    Declined(Decline),
}

impl std::fmt::Display for TakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TakeError::NotFound(id) => write!(f, "no conflict {id}"),
            TakeError::Stale(why) => write!(f, "the deck changed since the conflict: {why}"),
            TakeError::Declined(d) => write!(f, "{d}"),
        }
    }
}

/// The held conflicts of one deck.
#[derive(Debug, Default)]
pub struct Conflicts {
    held: Mutex<Vec<ShapeConflict>>,
}

impl Conflicts {
    /// Hold `new` (a clash already held is held once).
    pub fn hold(&self, new: impl IntoIterator<Item = ShapeConflict>) {
        let mut h = self.held.lock().unwrap_or_else(|e| e.into_inner());
        for c in new {
            if !h.iter().any(|x| x.id == c.id) {
                h.push(c);
            }
        }
    }

    /// The held conflicts, oldest first.
    pub fn list(&self) -> Vec<ShapeConflict> {
        self.held.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Take conflict `id`'s side against the deck as it is now (`current`). The person's
    /// side drops the record and writes nothing (`None`); the agent's writes its shape (or
    /// its delete), and only if the file still holds the person's ([`TakeError::Stale`]
    /// otherwise).
    pub fn take(
        &self,
        id: &str,
        side: Take,
        current: &[u8],
        limits: &Limits,
    ) -> Result<Option<Vec<u8>>, TakeError> {
        let mut h = self.held.lock().unwrap_or_else(|e| e.into_inner());
        let at = h
            .iter()
            .position(|c| c.id == id)
            .ok_or_else(|| TakeError::NotFound(id.to_owned()))?;
        let out = match side {
            Take::Person => None,
            Take::Agent => Some(apply_agent(&h[at], current, limits)?),
        };
        h.remove(at);
        Ok(out)
    }
}

/// `(part key, shape id)` of a conflict's address in a deck.
fn locate(p: &Pptx<'_>, address: &str) -> Option<(String, u32)> {
    let (scope, sp) = address.rsplit_once("/sp:")?;
    let shape: u32 = sp.parse().ok()?;
    if let Some(id) = scope.strip_prefix("@sld:") {
        let s = p.deck.slide(id.parse().ok()?)?;
        return Some((s.part.clone(), shape));
    }
    let id = scope.strip_prefix("@notes:")?;
    let s = p.deck.slide(id.parse().ok()?)?;
    Some((s.notes.clone()?, shape))
}

fn apply_agent(c: &ShapeConflict, current: &[u8], limits: &Limits) -> Result<Vec<u8>, TakeError> {
    let p = Pptx::open(current, limits).map_err(TakeError::Declined)?;
    let stale = |why: &str| TakeError::Stale(why.to_owned());
    let (key, shape) = locate(&p, &c.address).ok_or_else(|| stale("the slide is gone"))?;
    let text = p.text(&key).map_err(TakeError::Declined)?;
    let doc = parse_xml(text, limits).map_err(TakeError::Declined)?;
    let tree = Tree::parse(&doc).map_err(TakeError::Declined)?;
    let now = tree.find(shape);
    let new = match (&c.person, now) {
        (Some(person), Some(i)) if text[tree.units[i].range.clone()] == **person => {
            let u = &tree.units[i];
            match &c.agent {
                Some(a) => xmlx::splice(text, vec![(u.range.clone(), a.clone())]),
                None => xmlx::splice(text, vec![(u.lead.start..u.range.end, String::new())]),
            }
        }
        (None, None) => match &c.agent {
            Some(a) => xmlx::splice(text, vec![(tree.units_end..tree.units_end, a.clone())]),
            None => text.to_owned(),
        },
        _ => return Err(stale("the shape isn't the person's any more")),
    };
    let ndoc = parse_xml(&new, limits).map_err(TakeError::Declined)?;
    let ntree = Tree::parse(&ndoc).map_err(TakeError::Declined)?;
    if !ntree.ids_unique() {
        return Err(stale("the agent's shape ids are taken now"));
    }
    write_parts(&p.opc, [(key, new)]).map_err(TakeError::Declined)
}

/// The package with parts (by key) given new text; every other part raw.
pub fn write_parts(
    opc: &OpcPackage<'_>,
    parts: impl IntoIterator<Item = (String, String)>,
) -> Result<Vec<u8>, Decline> {
    let changes: BTreeMap<String, Change> = parts
        .into_iter()
        .map(|(k, t)| (k, Change::Replace(t.into_bytes())))
        .collect();
    opc.write(&changes)
}
