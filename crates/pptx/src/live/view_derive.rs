//! What a live deck's view index reads from the carrier (the rendering contract's gaps 4, 5 and
//! 6 on the live tier): the items only the file's bytes hold, derived on the kit's index through
//! the kit's carrier hand-off (`ViewSource::Live(state, Some(carrier))`).
//!
//! - **Read once per carrier:** the carrier is decoded into the pack's own read of the deck
//!   ([`crate::view::DeckRead`]: every shape as the file holds it, each slide's render digest)
//!   and the live model of it ([`LiveDeck`]: what the store held when the file was synced),
//!   through [`ViewCarrier::decoded`]. The kit keeps that until the carrier changes, and the
//!   index it built by (carrier id, rev), so a read of an unchanged deck decodes nothing.
//! - **The same code as the pack's view of a file:** a group's children are placed by
//!   [`crate::view::place`], the function that places every shape of
//!   [`crate::view::index`], so the live view and the view of the same bytes agree by
//!   construction.
//!
//! **The rule: the live store wins, and the carrier supplies only what the store doesn't
//! model.** A field the store holds (a box, text, a name, hidden, the order) is the store's;
//! from the carrier come:
//!
//! - **a group's children** (`live: false`), placed from the group's **live** box with the
//!   carrier's child space (`chOff`, `chExt`) and the children's own transforms (contract Item 1,
//!   steps 1 to 8, the true centre): a group the store moved or resized moves or scales them;
//! - **the box a placeholder inherits** from its layout (else its master), when the store holds
//!   none (`bbox_inherited`); a placeholder the store gave a box has that box;
//! - **field kinds** (`fields`, `has_fields`), and a slide number's text at the slide's live
//!   position, while the store's text is the carrier's (a field's text can't be set live);
//! - **fonts**: a shape's from the carrier while its text is the carrier's; one whose text the
//!   store changed keeps its style's and its first run's format (what `shape text` writes); a
//!   shape the carrier doesn't hold (a text box added live, a new slide's placeholder) takes its
//!   layout's defaults;
//! - **the render hash** (as the rendering contract specifies: over the written bytes): a slide the store didn't change as
//!   drawn (its shapes, hidden and layout; its position too when it shows a slide number) has
//!   the carrier's render digest, which is the written bytes' (the splice leaves its closure
//!   untouched); any other is hashed over the carrier's id and the slide as the store holds it,
//!   so it changes with every change that changes the drawing and becomes the written bytes'
//!   digest once the kit's write makes them the carrier.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use k2_mpc_base::elem::{Applied, KeyedRoot, KeyedState};
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::index::{cut_text, Digest, DigestStream};
use k2_mpc_base::session::WatchExtras;
use k2_mpc_base::view_index::{
    self as kv, unit_value, EditableText, ViewCarrier, ViewChange, ViewIndex, VIEW_TEXT_MAX,
};

use super::model::{self, f, parse_key, Geom, LiveDeck, LiveShape, LiveSlide};
use super::{DECK, SHAPES, SHAPE_PROPS, SLIDES, SLIDE_PROPS};
use crate::geom::{compose, Xf};
use crate::view::{self, At, DeckRead, FontUse, ShapeRead};
use crate::{Limits, Pptx};

/// The carrier as the derive hook reads it, decoded once per carrier.
#[derive(Debug)]
pub struct Carried {
    /// The pack's read of the file: every shape as it holds it.
    pub read: DeckRead,
    /// The live model of the file: what the store held when it was synced.
    pub model: LiveDeck,
}

#[cfg(feature = "testgen")]
thread_local! {
    static DECODES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// This thread's carrier decodes by the view derive since the last call, which resets them
/// (test-only: the `testgen` feature).
#[cfg(feature = "testgen")]
pub fn take_decodes() -> u64 {
    DECODES.with(|c| c.replace(0))
}

/// The carrier `bytes` read for the view: `None` when they don't read as a deck (then nothing
/// is derived from them).
pub fn decode(bytes: &[u8], limits: &Limits) -> Option<Carried> {
    #[cfg(feature = "testgen")]
    DECODES.with(|c| c.set(c.get() + 1));
    let p = Pptx::open(bytes, limits).ok()?;
    Some(Carried {
        read: view::read_open(&p, limits).ok()?,
        model: model::read_open(&p, limits).ok()?,
    })
}

/// The deck as the live store holds it.
fn live_deck(state: &KeyedState) -> Option<LiveDeck> {
    let list = |n: &str| {
        state.roots.iter().find_map(|(r, k)| match k {
            KeyedRoot::List { live, .. } if r == n => Some(live.as_slice()),
            _ => None,
        })
    };
    let map = |n: &str| {
        state.roots.iter().find_map(|(r, k)| match k {
            KeyedRoot::Map(m) if r == n => Some(m),
            _ => None,
        })
    };
    let empty = BTreeMap::new();
    LiveDeck::from_parts(
        list(SLIDES)?,
        list(SHAPES)?,
        map(SLIDE_PROPS).unwrap_or(&empty),
        map(SHAPE_PROPS).unwrap_or(&empty),
        map(DECK).unwrap_or(&empty),
    )
    .ok()
}

fn xf(g: Geom) -> Xf {
    Xf {
        x: g.x,
        y: g.y,
        w: g.w,
        h: g.h,
        rot: g.rot,
        fh: g.fh,
        fv: g.fv,
    }
}

/// `@sld:<sid>/sp:<fid>` as `(sid, fid)`.
fn shape_of(addr: &str) -> Option<(u32, u32)> {
    let (s, f) = addr.strip_prefix("@sld:")?.split_once("/sp:")?;
    Some((s.parse().ok()?, f.parse().ok()?))
}

fn cut(s: &str) -> String {
    cut_text(s, VIEW_TEXT_MAX as u32).0.to_owned()
}

fn kit_font(f: &FontUse) -> kv::FontUse {
    kv::FontUse {
        family: f.family.clone(),
        script: f.script.to_owned(),
        from: f.from.to_owned(),
        embedded: f.embedded,
    }
}

/// A unit the pack placed, as the kit's index holds it (a group's child: never text-editable).
fn kit_unit(u: &view::Unit) -> kv::Unit {
    kv::Unit {
        key: u.key.clone().unwrap_or_default(),
        container: u.container.clone(),
        parent: u.parent.clone(),
        live: u.live,
        kind: u.kind.to_owned(),
        name: cut(&u.name),
        name_is_generic: Some(u.name_is_generic),
        alt: u.alt.as_deref().filter(|a| !a.is_empty()).map(cut),
        z: u.z,
        bbox: u.bbox.map(|b| kv::Bbox {
            x: b.x,
            y: b.y,
            w: b.w,
            h: b.h,
        }),
        bbox_inherited: u.bbox_inherited,
        rot: u.rot,
        flip: kv::Flip {
            h: u.flip_h,
            v: u.flip_v,
        },
        approx_box: u.approx_box,
        text: u.text.as_ref().map(|t| kv::UnitText {
            summary: t.summary.clone(),
            truncated: t.truncated,
        }),
        fields: u.fields.iter().map(|s| (*s).to_owned()).collect(),
        hidden: u.hidden,
        placeholder: u.placeholder.as_ref().map(|p| kv::Placeholder {
            ty: p.ty.clone(),
            idx: p.idx.clone(),
        }),
        editable_text: Some(match u.editable_text {
            true => EditableText::Plain,
            false => EditableText::Never,
        }),
    }
}

/// A top-level unit given the box `x` it inherits (`bbox_inherited`).
fn inherit(u: &mut kv::Unit, x: Xf) {
    let p = compose(x, &[], false);
    u.bbox = Some(kv::Bbox {
        x: p.x,
        y: p.y,
        w: p.w,
        h: p.h,
    });
    u.rot = p.rot;
    u.flip = kv::Flip { h: p.fh, v: p.fv };
    u.approx_box = p.approx;
    u.bbox_inherited = true;
}

/// A shape's fonts: the carrier's while the store's text is the carrier's; the style's and the
/// first run's format's when the store changed the text; the layout's defaults for a shape the
/// carrier doesn't hold.
fn shape_fonts(
    read: &DeckRead,
    s: Option<&ShapeRead>,
    same_text: bool,
    lm: Option<&LiveShape>,
    layout: Option<u64>,
) -> Vec<FontUse> {
    let texted = lm
        .and_then(|m| m.text.as_deref())
        .is_some_and(|t| !t.is_empty());
    match s {
        Some(s) if same_text => s.fonts.all.clone(),
        Some(s) => {
            let mut v = s.fonts.style.clone();
            if texted {
                v.extend(s.fonts.retext.iter().cloned());
            }
            v
        }
        None if texted => read.default_fonts(
            layout,
            lm.and_then(|m| m.ph.as_ref()).map(|p| p.ty.as_deref()),
        ),
        None => Vec::new(),
    }
}

/// The slide as drawn is the same in both: its layout, hidden and shapes (less `refd`, a fact
/// the file derives).
fn drawn_same(a: &LiveSlide, b: &LiveSlide) -> bool {
    let shape = |x: &LiveShape| LiveShape {
        refd: false,
        ..x.clone()
    };
    a.layout == b.layout
        && a.hidden == b.hidden
        && a.shapes.len() == b.shapes.len()
        && a.shapes
            .iter()
            .zip(&b.shapes)
            .all(|(x, y)| shape(x) == shape(y))
}

fn put(d: &mut DigestStream, b: &[u8]) {
    d.update(&(b.len() as u64).to_le_bytes());
    d.update(b);
}

/// The digest of a slide the store changed as drawn: over the carrier's id (every part the
/// slide's closure holds is in it) and the slide as the store holds it, its position when it
/// shows a slide number, and its fonts.
fn live_digest(
    carrier: &[u8; 32],
    s: &LiveSlide,
    slidenum: Option<usize>,
    fonts: &[FontUse],
) -> Digest {
    let mut d = Digest::stream();
    put(&mut d, b"k2-mpc-pptx/live-slide/v1");
    put(&mut d, carrier);
    put(&mut d, s.sid.to_string().as_bytes());
    put(
        &mut d,
        slidenum
            .map(|p| p.to_string())
            .unwrap_or_default()
            .as_bytes(),
    );
    put(&mut d, &[u8::from(s.hidden)]);
    put(
        &mut d,
        s.layout
            .map(|l| l.to_string())
            .unwrap_or_default()
            .as_bytes(),
    );
    for x in &s.shapes {
        put(&mut d, b"shape");
        for (k, v) in x.props(s.sid) {
            if k != f::REFD {
                put(&mut d, k.as_bytes());
                put(&mut d, v.as_str().unwrap_or("").as_bytes());
            }
        }
    }
    for f in fonts {
        put(&mut d, f.family.as_bytes());
        put(&mut d, f.script.as_bytes());
        put(&mut d, f.from.as_bytes());
        put(&mut d, &[u8::from(f.embedded)]);
    }
    d.finish()
}

/// The items only the carrier holds, set on the kit's live `index` (the module's rule: the
/// live store wins). Nothing is set when the carrier or the store doesn't read as a deck.
pub fn derive(state: &KeyedState, carrier: &ViewCarrier, index: &mut ViewIndex, limits: &Limits) {
    let Some(c) = carrier.decoded(|b| decode(b, limits)) else {
        return;
    };
    let Some(live) = live_deck(state) else {
        return;
    };
    let live_slides: HashMap<u32, &LiveSlide> = live.slides.iter().map(|s| (s.sid, s)).collect();
    let model_slides: HashMap<u32, &LiveSlide> =
        c.model.slides.iter().map(|s| (s.sid, s)).collect();
    let position: HashMap<String, usize> = index
        .containers
        .iter()
        .map(|x| (x.key.clone(), x.position as usize))
        .collect();
    let mut fonts: HashMap<String, Vec<FontUse>> = HashMap::new();
    let mut fields: HashMap<String, BTreeSet<&'static str>> = HashMap::new();
    let mut units = Vec::with_capacity(index.units.len());
    for mut u in std::mem::take(&mut index.units) {
        let Some((sid, fid)) = shape_of(&u.key) else {
            units.push(u);
            continue;
        };
        let at = At {
            sid,
            position: position.get(&u.container).copied().unwrap_or(0),
        };
        let ls = live_slides.get(&sid).copied();
        let lm = ls.and_then(|s| s.shapes.iter().find(|x| x.fid == fid));
        let s = c
            .read
            .slide(sid)
            .and_then(|r| r.shapes.iter().find(|x| x.id == Some(fid)));
        let cm = model_slides
            .get(&sid)
            .and_then(|s| s.shapes.iter().find(|x| x.fid == fid));
        let same_text = matches!((cm, lm), (Some(a), Some(b)) if a.text == b.text);
        let layout = ls.and_then(|s| s.layout);
        fonts
            .entry(u.container.clone())
            .or_default()
            .extend(shape_fonts(&c.read, s, same_text, lm, layout));
        let mut kids = Vec::new();
        match s {
            Some(s) => {
                if u.bbox.is_none() && s.own.is_none() {
                    if let Some(x) = s.inherited {
                        inherit(&mut u, x);
                    }
                }
                if same_text {
                    u.fields = s.fields.iter().map(|k| (*k).to_owned()).collect();
                    if s.fields.contains(&"slidenum") {
                        if let Some(t) = &s.text {
                            let t = t.at(at.position);
                            u.text = Some(kv::UnitText {
                                summary: t.summary,
                                truncated: t.truncated,
                            });
                        }
                    }
                }
                if s.is_group {
                    let own = lm.and_then(|m| m.geom).map(xf);
                    let mut out = Vec::new();
                    view::place(at, s, u.z, None, &mut Vec::new(), false, own, &mut out);
                    kids = out.iter().skip(1).map(kit_unit).collect();
                }
            }
            None => {
                // a placeholder the file doesn't hold yet (a slide added live)
                if let (None, Some(ph)) = (u.bbox, lm.and_then(|m| m.ph.as_ref())) {
                    if let Some(x) = c.read.inherited(layout, ph.kind(), ph.idx.as_deref()) {
                        inherit(&mut u, x);
                    }
                }
            }
        }
        let has = fields.entry(u.container.clone()).or_default();
        for k in u
            .fields
            .iter()
            .chain(kids.iter().flat_map(|k| k.fields.iter()))
        {
            if let Some(k) = view::field_kind(k) {
                has.insert(k);
            }
        }
        units.push(u);
        units.extend(kids);
    }
    index.units = units;
    let per: Vec<Vec<FontUse>> = index
        .containers
        .iter()
        .map(|x| fonts.remove(&x.key).unwrap_or_default())
        .collect();
    let (all, idx) = view::deck_fonts(&per);
    index.fonts = all.iter().map(kit_font).collect();
    for (ct, idx) in index.containers.iter_mut().zip(idx) {
        ct.fonts = idx.iter().map(|i| *i as u32).collect();
        let has = fields.remove(&ct.key).unwrap_or_default();
        ct.has_fields = view::field_order(&has)
            .into_iter()
            .map(str::to_owned)
            .collect();
        let Some(sid) = ct
            .key
            .strip_prefix("@sld:")
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Some(ls) = live_slides.get(&sid) else {
            continue;
        };
        let pos = ct.position as usize;
        let slidenum = has.contains("slidenum");
        let carried = c
            .read
            .slide(sid)
            .zip(model_slides.get(&sid))
            .filter(|(r, m)| {
                drawn_same(m, ls) && (!r.has_fields.contains(&"slidenum") || r.position == pos)
            });
        let digest = match carried {
            Some((r, _)) => r.digest,
            None => {
                // the slide's fonts as the deck orders them
                let mut mine: Vec<FontUse> = Vec::new();
                for i in &ct.fonts {
                    mine.push(all[*i as usize].clone());
                }
                live_digest(carrier.id(), ls, slidenum.then_some(pos), &mine)
            }
        };
        ct.render_hash = Some(carrier.render_hash(&digest));
    }
}

/// The slides a change touched, by id: a slide's own ops, its shapes', and their fields'.
fn touched(ops: &[Applied]) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    let mut key = |k: &str| {
        if let Some((sid, _)) = parse_key(k) {
            out.insert(sid);
        }
    };
    for a in ops {
        match a {
            Applied::List { root, ops } if root == SLIDES || root == SHAPES => {
                for o in ops {
                    key(o.key.as_str());
                }
            }
            Applied::Map { root, ops } if root == SLIDE_PROPS || root == SHAPE_PROPS => {
                for o in ops {
                    key(o.key.split('\u{1f}').next().unwrap_or(""));
                }
            }
            _ => {}
        }
    }
    out
}

/// A keyed watch's change frame's `derived`: for each slide the change touched, by address,
/// its derived values after it (title, fonts, field kinds, unit count) and its units that
/// carry anything derived (a group's children, an inherited box, field kinds), each as the
/// view index gives it; `null` for a slide the change deleted. Read from the index after the
/// change, which the kit built through its kept index (the carrier's items included). The
/// render hashes are the kit's: every container whose hash changed.
pub fn watch_extras(ops: &[Applied], view: Option<ViewChange<'_>>) -> WatchExtras {
    let Some(v) = view else {
        return WatchExtras::default();
    };
    let slides = touched(ops);
    if slides.is_empty() {
        return WatchExtras::default();
    }
    let mut out: Vec<(String, Json)> = Vec::new();
    for sid in slides {
        let key = format!("@sld:{sid}");
        let Some(ct) = v.after.containers.iter().find(|c| c.key == key) else {
            out.push((key, Json::Null));
            continue;
        };
        let units: Vec<Json> = v
            .after
            .units_of(&key)
            .unwrap_or(&[])
            .iter()
            .filter(|u| !u.live || u.bbox_inherited || !u.fields.is_empty())
            .map(unit_value)
            .collect();
        out.push((
            key,
            Json::obj([
                ("title", Json::from(ct.title.as_ref())),
                ("fonts", Json::list(ct.fonts.iter().copied())),
                ("has_fields", Json::list(ct.has_fields.iter())),
                ("unit_count", Json::from(ct.unit_count)),
                ("units", Json::List(units)),
            ]),
        ));
    }
    WatchExtras::new().derived(Json::Obj(out))
}
