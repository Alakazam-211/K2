//! The live pptx pack ([`PptxLive`]): a deck as a live keyed doc on the kit's structured mode,
//! so a person can watch an agent's verbs land.
//!
//! - [`model`]: the roots, their keys and fields, and the deck read into them;
//! - [`splice`]: the live content written back into the file by splicing only what changed
//!   ([`splice::encode_from`]);
//! - [`verbs`]: the agent's verbs on the live store, with bounded reads.
//!
//! **Keys are the file's own ids** (`s<sldId>`, `s<sldId>-<cNvPr>`), for imported elements and
//! for ones a verb adds alike: a verb fixes the new element's file id inside its own change,
//! under the session lock, from the deck's id marks (B1), so the key is its address and a
//! desktop save that keeps the ids keeps the keys. A file id is unique only when it is minted
//! at one serialisation point, which the kit's Yrs host is.
//!
//! **The carrier.** Opaque parts (masters, layouts, themes, pictures, tables, charts,
//! animations) never enter the engine: the file is written by splicing the live changes into
//! the bytes the kit last read from the file or wrote to it, the **carrier**, which the kit
//! keeps in its store and hands to [`Pack::encode_from`] (the kit's carrier hook K1;
//! [`PptxLive`] opts in with [`Pack::wants_carrier`]). So a deck encodes after a restart from
//! the stored carrier, with no read of the file, and a version's file (`file_at`) is that
//! version spliced into its own carrier. The pack holds no file bytes between calls:
//! [`splice::encode_from`] decodes the carrier it is handed each time. With no carrier (a doc
//! never synced with its file) [`Pack::encode`] refuses: a deck isn't built from the doc alone.

pub mod model;
pub mod splice;
pub mod verbs;

use std::collections::{BTreeMap, HashSet};

use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Key, ListOp, MapOp, Value};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{Declined, Pack, PackContent, PackError, Reconciled, View};

pub use model::{
    f, parse_key, prop, shape_key, slide_key, Geom, LiveDeck, LiveShape, LiveSlide, Ph, TextEdit,
    DECK, SHAPES, SHAPE_PROPS, SLIDES, SLIDE_PROPS, TEXT_MAX,
};

use crate::{Decline, Limits};

/// The live schema. Its name is stamped on each stored doc.
pub const SCHEMA: Schema = Schema {
    name: "pptx",
    roots: &[
        (SLIDES, Kind::MovableList),
        (SHAPES, Kind::MovableList),
        (SLIDE_PROPS, Kind::Map),
        (SHAPE_PROPS, Kind::Map),
        (DECK, Kind::Map),
    ],
};

/// The composite map roots, with their lists.
pub const COMPOSITE: &[(&str, &str)] = &[(SLIDE_PROPS, SLIDES), (SHAPE_PROPS, SHAPES)];

/// The codec settings a live deck is stored with: a format byte. The splice reads everything
/// else from the carrier and ignores the settings (a doc stored before the carrier hook holds
/// the byte and a digest, ignored as well).
pub const CODEC: &[u8] = &[1];

/// The live pack for `.pptx`. It holds no file bytes: the kit keeps the carrier.
#[derive(Debug, Default)]
pub struct PptxLive {
    pub limits: Limits,
}

/// A decline as the kit takes it.
pub fn declined(d: Decline) -> Declined {
    Declined {
        code: d.code,
        message: d.message,
    }
}

impl PptxLive {
    pub fn new(limits: Limits) -> Self {
        PptxLive { limits }
    }
}

/// The edits that make a list `old` (with its deleted elements `hidden`) into `new`, by key:
/// deletes, then inserts and restores (each placed after the element before it in `new`),
/// then the moves that put the order right, then value changes.
pub fn list_edits(
    old: &[(Key, Value)],
    hidden: Option<&BTreeMap<Key, Value>>,
    new: &[(Key, Value)],
) -> Vec<ListOp> {
    let mut ops = Vec::new();
    let new_keys: HashSet<&Key> = new.iter().map(|(k, _)| k).collect();
    let mut order: Vec<Key> = Vec::new();
    for (k, _) in old {
        if new_keys.contains(k) {
            order.push(k.clone());
        } else {
            ops.push(ListOp::Delete { key: k.clone() });
        }
    }
    let old_vals: BTreeMap<&Key, &Value> = old.iter().map(|(k, v)| (k, v)).collect();
    let mut prev: Option<Key> = None;
    for (k, v) in new {
        if !old_vals.contains_key(k) {
            let at = prev.clone().map_or(Anchor::Start, Anchor::After);
            if hidden.is_some_and(|h| h.contains_key(k)) {
                ops.push(ListOp::Restore { key: k.clone() });
                ops.push(ListOp::Move {
                    key: k.clone(),
                    to: at,
                });
                if hidden.and_then(|h| h.get(k)) != Some(v) {
                    ops.push(ListOp::Set {
                        key: k.clone(),
                        value: v.clone(),
                    });
                }
            } else {
                ops.push(ListOp::Insert {
                    key: k.clone(),
                    at,
                    value: v.clone(),
                });
            }
            let pos = match &prev {
                Some(p) => order.iter().position(|x| x == p).map_or(0, |i| i + 1),
                None => 0,
            };
            order.insert(pos, k.clone());
        }
        prev = Some(k.clone());
    }
    // the moves: walk `new`; each element right after the one before it
    for (i, (k, _)) in new.iter().enumerate() {
        let want = if i == 0 { None } else { Some(&new[i - 1].0) };
        let at = order.iter().position(|x| x == k).expect("placed");
        let before = if at == 0 { None } else { Some(&order[at - 1]) };
        if before != want {
            let to = want.cloned().map_or(Anchor::Start, Anchor::After);
            order.remove(at);
            let pos = match want {
                Some(w) => order.iter().position(|x| x == w).map_or(0, |j| j + 1),
                None => 0,
            };
            order.insert(pos, k.clone());
            ops.push(ListOp::Move { key: k.clone(), to });
        }
    }
    for (k, v) in new {
        if let Some(o) = old_vals.get(k) {
            if *o != v {
                ops.push(ListOp::Set {
                    key: k.clone(),
                    value: v.clone(),
                });
            }
        }
    }
    ops
}

/// The edits that make a map `old` into `new`.
pub fn map_edits(old: &BTreeMap<String, Value>, new: &BTreeMap<String, Value>) -> Vec<MapOp> {
    let mut ops = Vec::new();
    for (k, v) in new {
        if old.get(k) != Some(v) {
            ops.push(MapOp::Set {
                key: k.clone(),
                value: v.clone(),
            });
        }
    }
    for k in old.keys().filter(|k| !new.contains_key(*k)) {
        ops.push(MapOp::Delete { key: k.clone() });
    }
    ops
}

/// The highest shape id the store holds for slide `sid`, live or deleted (from the shapes
/// list's keys and the `fid` fields).
pub fn store_max_fid(view: &View, sid: u32) -> u32 {
    let mut m = 0;
    let mut see = |k: &str| {
        if let Some((s, Some(f))) = parse_key(k) {
            if s == sid {
                m = m.max(f);
            }
        }
    };
    for (k, _) in view.list(SHAPES).unwrap_or(&[]) {
        see(k.as_str());
    }
    if let Some(h) = view.hidden(SHAPES) {
        for k in h.keys() {
            see(k.as_str());
        }
    }
    if let Some(props) = view.map(SHAPE_PROPS) {
        let start = format!("s{sid}-");
        for (k, _) in props.range(start.clone()..) {
            if !k.starts_with(&start) {
                break;
            }
            see(k.split(k2_mpc_base::table::CELL_SEP).next().unwrap_or(""));
        }
    }
    m
}

impl Pack for PptxLive {
    fn schema(&self) -> Schema {
        SCHEMA
    }

    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        let deck = model::read(bytes, &self.limits).map_err(declined)?;
        Ok(PackContent {
            roots: deck.to_content(),
            codec: CODEC.to_vec(),
        })
    }

    /// A deck is written only by splicing into its carrier ([`Pack::encode_from`]). The kit
    /// calls this only for a doc it holds no carrier for (never synced with its file), and
    /// the opaque parts can't be built from the doc alone.
    fn encode(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        let _ = (content, codec);
        Err(PackError(
            "a live deck is written by splicing into its file's bytes, and none are kept for \
             this doc: open it from its file"
                .into(),
        ))
    }

    fn wants_carrier(&self) -> bool {
        true
    }

    /// The live content spliced into `carrier`, the bytes the kit last read from the file or
    /// wrote to it ([`splice::encode_from`]). Untouched is the carrier, byte for byte.
    fn encode_from(
        &self,
        carrier: &[u8],
        content: &[(String, Neutral)],
        codec: &[u8],
    ) -> Result<Vec<u8>, PackError> {
        splice::encode_from(carrier, content, codec, &self.limits)
    }

    /// An outside save, by key: slides by `sldId`, shapes by `(sldId, cNvPr)`. The marks
    /// never go down: each is the highest of the save's, the store's and the ids the store
    /// holds.
    fn reconcile(
        &self,
        synced: &View,
        bytes: &[u8],
        _codec: &[u8],
    ) -> Result<Reconciled, Declined> {
        let read = model::read(bytes, &self.limits).map_err(declined)?;
        let mut new = read.clone();
        let old_deck = synced.map(DECK).cloned().unwrap_or_default();
        let old_mark = |k: &str| -> u32 {
            old_deck
                .get(k)
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        };
        let mut store_sld = 0u32;
        let live = synced.list(SLIDES).unwrap_or(&[]).iter().map(|(k, _)| k);
        let gone = synced.hidden(SLIDES).into_iter().flat_map(|h| h.keys());
        for k in live.chain(gone) {
            if let Some((s, None)) = parse_key(k.as_str()) {
                store_sld = store_sld.max(s);
            }
        }
        new.sld_mark = new.sld_mark.max(old_mark(model::SLD_MARK)).max(store_sld);
        let mut sids: Vec<u32> = new.fid_marks.keys().copied().collect();
        for k in old_deck.keys() {
            if let Some(s) = k.strip_prefix("mark/fid/").and_then(|s| s.parse().ok()) {
                sids.push(s);
            }
        }
        for sid in sids {
            let m = new
                .fid_marks
                .get(&sid)
                .copied()
                .unwrap_or(0)
                .max(old_mark(&model::fid_mark_key(sid)))
                .max(store_max_fid(synced, sid));
            new.fid_marks.insert(sid, m);
        }
        let content = new.to_content();
        let mut edits = Vec::new();
        for (root, n) in &content {
            match n {
                Neutral::List(l) => {
                    let ops = list_edits(synced.list(root).unwrap_or(&[]), synced.hidden(root), l);
                    if !ops.is_empty() {
                        edits.push(Edit::List {
                            root: root.clone(),
                            ops,
                        });
                    }
                }
                Neutral::Map(m) => {
                    let empty = BTreeMap::new();
                    let ops = map_edits(synced.map(root).unwrap_or(&empty), m);
                    if !ops.is_empty() {
                        edits.push(Edit::Map {
                            root: root.clone(),
                            ops,
                        });
                    }
                }
                _ => {}
            }
        }
        Ok(Reconciled {
            edits,
            codec: CODEC.to_vec(),
            ..Reconciled::default()
        })
    }

    fn verbs(&self) -> &'static [k2_mpc_base::tools::PackVerb] {
        verbs::VERBS
    }

    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        COMPOSITE
    }
}

/// A key as an address: `s258-5` is `@sld:258/sp:5`, `s258` is `@sld:258`; a composite map
/// key adds its field (`@sld:258/sp:5 geom`). Anything else as it is.
pub fn address_of(key: &str) -> String {
    let (elem, field) = match key.split_once(k2_mpc_base::table::CELL_SEP) {
        Some((e, f)) => (e, Some(f)),
        None => (key, None),
    };
    let a = match parse_key(elem) {
        Some((s, Some(f))) => format!("@sld:{s}/sp:{f}"),
        Some((s, None)) => format!("@sld:{s}"),
        None => elem.to_owned(),
    };
    match field {
        Some(f) => format!("{a} {f}"),
        None => a,
    }
}

/// One change as a terminal watcher prints it: its version, who and through which verb, then
/// each op by address (values cut at 60 bytes, line ends shown as `\n`).
pub fn watch_line(e: &k2_mpc_base::session::ChangeEvent) -> String {
    use k2_mpc_base::elem::{Applied, ListOpKind, MapOpKind};
    let v = |x: &Option<Value>| -> String {
        let s = x.as_ref().and_then(Value::as_str).unwrap_or("");
        crate::xmlx::cut(s, 60).replace('\n', "\\n")
    };
    let ops: Vec<String> = match &e.applied {
        Applied::List { ops, .. } => ops
            .iter()
            .map(|o| {
                let a = address_of(o.key.as_str());
                match o.kind {
                    ListOpKind::Insert => {
                        format!("add {a} at {}", o.index_after.map_or(0, |i| i + 1))
                    }
                    ListOpKind::Move => format!(
                        "move {a} {} to {}",
                        o.index_before.map_or(0, |i| i + 1),
                        o.index_after.map_or(0, |i| i + 1)
                    ),
                    ListOpKind::Delete => format!("delete {a}"),
                    ListOpKind::Restore => format!("restore {a}"),
                    ListOpKind::Set => format!("kind {a} = {}", v(&o.value_after)),
                    _ => format!("drop {a}"),
                }
            })
            .collect(),
        Applied::Map { ops, .. } => ops
            .iter()
            .map(|o| {
                let a = address_of(&o.key);
                match o.kind {
                    MapOpKind::Set => format!("{a} = {}", v(&o.after)),
                    _ => format!("{a} removed"),
                }
            })
            .collect(),
        _ => Vec::new(),
    };
    format!("v{} {} via {}: {}", e.version, e.pid, e.via, ops.join("; "))
}
