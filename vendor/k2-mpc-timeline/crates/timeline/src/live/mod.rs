//! The live OTIO pack ([`TimelineLive`]): a timeline as a live keyed doc on the kit's
//! structured mode, its tracks as lanes and its items as bars, so a person can watch an agent's
//! verbs land. Off by default: a host opens a file live only when it chooses to.
//!
//! - [`model`]: the roots, their keys and fields, and the file read into them;
//! - [`ticks`]: the store's time unit;
//! - [`splice`]: the live content written back into the file, only what changed rewritten
//!   ([`splice::encode_from_paired`]);
//! - [`verbs`]: the agent's verbs, with bounded reads; each gives its payload as data for a
//!   call's result (`mpc.result/1`, the kit's), and refuses by the pack's registered entries
//!   ([`errors`]);
//! - [`view`]: the kit's view index of the timeline, its addresses, each track's derived label
//!   and kind, and each clip's derived timecodes;
//! - [`index`]: the verbs' track index, kept by the kit beside the live doc through every
//!   change.
//!
//! **Keys are minted** (`t<n>`, `i<n>`, `k<n>`, `m<n>`): OTIO has no object ids. The kit keeps
//! the **carrier** (the bytes it last read from the file or wrote to it) with its **pairing**
//! (the live keys in the order this pack's read of the carrier yields the units), and the
//! write-back splices the live content into it. [`TimelineLive`] opts in to both. Its
//! [`Pack::pairing`] works out the read's order from the content (tracks in order, each track's
//! items by start), since the items list's order means nothing. [`Pack::encode`] refuses: a
//! timeline isn't built from the doc alone.
//!
//! **Outside saves** ([`Pack::reconcile`]) are paired without guessing, in order:
//! 1. **our stamp** in an item's metadata (`"mpc": {"key", "doc"}`, which the write-back puts on
//!    every item it touches): only when its doc id is this doc's and exactly one item of the
//!    save claims the key; a stamp copied with a clip, or from another doc, pairs nothing;
//! 2. the item's derived identity (`nid`: its media's name and source in), when exactly one
//!    item on each side has it;
//! 3. otherwise a new element, under a key minted past the marks.
//!
//! **Tracks pair by their name and kind** ([`pair_tracks`]), so a track keeps its key (and
//! with it whatever names the track by its address: a playhead's track, a claim, a selection)
//! when a save adds, deletes or moves tracks around it:
//! 1. a track whose name and kind (`video`, `audio`, `other`) are those of exactly one track in
//!    the doc and one in the save pairs with it, wherever either sits in the stack;
//! 2. where the name can't decide (no name, or two tracks of that name and kind in either
//!    side), the tracks of that name and kind pair in their order, bottom first: their label
//!    counted among them, so the first unnamed audio track of the doc is the save's first;
//! 3. any other track of the save is new, under a new key, and a track of the doc paired with
//!    none goes. A name never pairs across kinds, nor with another name: a track renamed in the
//!    save (moved or not) is a new track, as a name that changed can't be told from a track
//!    deleted and another added, and a wrong pairing would move a claim or a playhead to
//!    another track.
//!
//! Media pair by a digest of their location, markers by owner, name and time. The kit applies a save only onto a doc equal to the synced state, and keeps
//! both when the doc holds changes the file doesn't or the kit wrote since the last outside save.

pub mod errors;
pub mod index;
pub mod model;
pub mod splice;
pub mod ticks;
pub mod verbs;
pub mod view;

use std::collections::{BTreeMap, HashMap, HashSet};

use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Key, KeyedState, ListOp, MapOp, Value};
use k2_mpc_base::grid::PageDerived;
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    Declined, Pack, PackContent, PackError, Pairing, Reconciled, SpliceError, VerbState, View,
};
use k2_mpc_base::view_index::{ViewIndex, ViewSchema, ViewSource, ViewTouched};

pub use model::{
    f, prop, t, LiveLimits, LiveTimeline, Store, ITEMS, ITEM_PROPS, MARKERS, MARKER_PROPS, MEDIA,
    MEDIA_PROPS, TIMELINE, TRACKS, TRACK_PROPS,
};
pub use view::{address_of, key_of};

/// The live schema. Its name is stamped on each stored doc.
pub const SCHEMA: Schema = Schema {
    name: "timeline-otio",
    roots: &[
        (TRACKS, Kind::MovableList),
        (ITEMS, Kind::MovableList),
        (MARKERS, Kind::MovableList),
        (MEDIA, Kind::MovableList),
        (TRACK_PROPS, Kind::Map),
        (ITEM_PROPS, Kind::Map),
        (MARKER_PROPS, Kind::Map),
        (MEDIA_PROPS, Kind::Map),
        (TIMELINE, Kind::Map),
    ],
};

/// The composite map roots, with their lists.
pub const COMPOSITE: &[(&str, &str)] = &[
    (TRACK_PROPS, TRACKS),
    (ITEM_PROPS, ITEMS),
    (MARKER_PROPS, MARKERS),
    (MEDIA_PROPS, MEDIA),
];

/// The codec settings a live timeline is stored with: a format byte (OTIO). The splice reads
/// everything else from the carrier.
pub const CODEC: &[u8] = &[1];

/// The live pack for OTIO files. It holds no file bytes: the kit keeps the carrier.
#[derive(Debug, Default)]
pub struct TimelineLive {
    /// The live tier's bounds.
    pub limits: LiveLimits,
}

impl TimelineLive {
    /// The pack with these bounds.
    pub fn new(limits: LiveLimits) -> Self {
        TimelineLive { limits }
    }
}

/// The edits that make a list `old` (with its deleted elements `hidden`) into `new`, by key:
/// deletes, then inserts and restores (each after the element before it in `new`), then the
/// moves that put the order right, then value changes.
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
    for (i, (k, _)) in new.iter().enumerate() {
        let want = if i == 0 { None } else { Some(&new[i - 1].0) };
        let Some(at) = order.iter().position(|x| x == k) else {
            continue;
        };
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

/// The highest `<prefix><n>` among a list root's keys, live or deleted.
pub fn store_max(view: &View, root: &str, prefix: char) -> u64 {
    let live = view.list(root).unwrap_or(&[]).iter().map(|(k, _)| k);
    let gone = view.hidden(root).into_iter().flat_map(|h| h.keys());
    live.chain(gone)
        .filter_map(|k| model::key_num(k.as_str(), prefix))
        .max()
        .unwrap_or(0)
}

/// The next key for `prefix` in `root`: one past both the mark and every key the list ever
/// held.
pub fn next_mark(view: &View, root: &str, prefix: char, mark: &str) -> u64 {
    let m = view
        .map(TIMELINE)
        .and_then(|m| m.get(mark))
        .and_then(Value::as_str)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if crate::mutant::active("MintBelowMark") {
        // planted: the mark ignored, one past the live keys only
        return view
            .list(root)
            .unwrap_or(&[])
            .iter()
            .filter_map(|(k, _)| model::key_num(k.as_str(), prefix))
            .max()
            .unwrap_or(0)
            + 1;
    }
    m.max(store_max(view, root, prefix)) + 1
}

/// The doc's track (in `s`) each track of an outside save's timeline `new` pairs with, in the
/// save's order (`None`: a new track), by the module's rule: within each name and kind (`video`,
/// `audio`, `other`; no name is a name of its own), the doc's tracks and the save's pair in their
/// order, bottom first. A name and kind held by one track on each side pairs those two wherever
/// they sit; where the name can't decide (none, or held by several tracks on either side), the
/// order among them is the fallback (their label, counted among the tracks of that name and
/// kind). Tracks of the doc left over go; those of the save left over are new.
pub fn pair_tracks(s: &Store<'_>, new: &LiveTimeline) -> Vec<Option<Key>> {
    // planted: a name matched whatever its kind
    let kinded = !crate::mutant::active("TrackNameWithoutKind");
    let mut doc: HashMap<(&str, &str), std::collections::VecDeque<&Key>> = HashMap::new();
    for (k, v) in s.tracks {
        let kind = if kinded { v.as_str().unwrap_or("") } else { "" };
        doc.entry((s.track(k, f::NAME).unwrap_or(""), kind))
            .or_default()
            .push_back(k);
    }
    new.tracks
        .iter()
        .map(|t| {
            let kind = if kinded { t.kind } else { "" };
            doc.get_mut(&(t.name.as_str(), kind))
                .and_then(std::collections::VecDeque::pop_front)
                .cloned()
        })
        .collect()
}

/// The keys an outside save's timeline `new` takes against `synced` (the module's pairing
/// rules), and the marks after it.
pub fn pair_save(synced: &View, new: &LiveTimeline) -> (model::Keys, model::Marks) {
    let s = Store::of_view(synced);
    let doc = s.tl(t::DOC).unwrap_or("");
    let mut next = [
        next_mark(synced, TRACKS, 't', t::MARK_T) - 1,
        next_mark(synced, ITEMS, 'i', t::MARK_I) - 1,
        next_mark(synced, MARKERS, 'k', t::MARK_K) - 1,
        next_mark(synced, MEDIA, 'm', t::MARK_M) - 1,
    ];
    let mut mint = |slot: usize, c: char| {
        next[slot] += 1;
        model::key(c, next[slot])
    };
    // by a field, exactly one on each side
    fn unique<'a>(
        side: impl Iterator<Item = (&'a Key, &'a str)>,
    ) -> HashMap<&'a str, Option<&'a Key>> {
        let mut m: HashMap<&str, Option<&Key>> = HashMap::new();
        for (k, v) in side {
            m.entry(v).and_modify(|e| *e = None).or_insert(Some(k));
        }
        m
    }
    // tracks: by name and kind, then by order where the name can't decide
    let tracks: Vec<Key> = if crate::mutant::active("TracksByLabel") {
        // planted: by label alone
        let old_t = unique(
            s.tracks
                .iter()
                .filter_map(|(k, _)| Some((k, s.track(k, f::NID)?))),
        );
        new.tracks
            .iter()
            .map(|t| match old_t.get(t.label.as_str()) {
                Some(Some(k)) => (*k).clone(),
                _ => mint(0, 't'),
            })
            .collect()
    } else {
        pair_tracks(&s, new)
            .into_iter()
            .map(|k| k.unwrap_or_else(|| mint(0, 't')))
            .collect()
    };
    // media: by their digest
    let old_m = unique(
        s.media
            .iter()
            .filter_map(|(k, _)| Some((k, s.media_field(k, f::NID)?))),
    );
    let media: Vec<Key> = new
        .media
        .iter()
        .map(|m| match old_m.get(m.nid.as_str()) {
            Some(Some(k)) => (*k).clone(),
            _ => mint(3, 'm'),
        })
        .collect();
    // items: our stamp, then the derived identity
    let live: HashSet<&str> = s.items.iter().map(|(k, _)| k.as_str()).collect();
    let mut claims: HashMap<&str, usize> = HashMap::new();
    for it in &new.items {
        if let Some((k, _)) = &it.stamp {
            *claims.entry(k.as_str()).or_default() += 1;
        }
    }
    let mut got: Vec<Option<Key>> = vec![None; new.items.len()];
    let mut used: HashSet<Key> = HashSet::new();
    let first_wins = crate::mutant::active("StampFirstWins");
    for (i, it) in new.items.iter().enumerate() {
        let Some((k, d)) = &it.stamp else { continue };
        let ours = first_wins || d == doc;
        let once = first_wins || claims[k.as_str()] == 1;
        if ours && once && live.contains(k.as_str()) {
            if let Ok(key) = Key::new(k) {
                if used.insert(key.clone()) {
                    got[i] = Some(key);
                }
            }
        }
    }
    let old_ids: Vec<(&Key, String)> = s
        .items
        .iter()
        .filter(|(k, _)| !used.contains(k))
        .filter_map(|(k, _)| Some((k, s.item_identity(k)?)))
        .collect();
    let old_i = unique(old_ids.iter().map(|(k, v)| (*k, v.as_str())));
    let nids: Vec<String> = (0..new.items.len()).map(|i| new.nid(i)).collect();
    let mut new_i: HashMap<&str, usize> = HashMap::new();
    for (i, n) in nids.iter().enumerate() {
        if got[i].is_none() {
            *new_i.entry(n.as_str()).or_default() += 1;
        }
    }
    let id_first_wins = crate::mutant::active("IdentityFirstWins");
    let items: Vec<Key> = got
        .into_iter()
        .enumerate()
        .map(|(i, g)| {
            if id_first_wins && g.is_none() {
                // planted: an ambiguous identity pairs with the first unused item that has it
                if let Some((k, _)) = old_ids
                    .iter()
                    .find(|(k, v)| *v == nids[i] && !used.contains(*k))
                {
                    used.insert((*k).clone());
                    return (*k).clone();
                }
            }
            g.unwrap_or_else(|| match old_i.get(nids[i].as_str()) {
                Some(Some(k)) if new_i[nids[i].as_str()] == 1 && !used.contains(*k) => {
                    used.insert((*k).clone());
                    (*k).clone()
                }
                _ => mint(1, 'i'),
            })
        })
        .collect();
    // markers: by owner, name and time
    let mut keys = model::Keys {
        tracks,
        items,
        markers: Vec::new(),
        media,
    };
    let old_kids: Vec<(&Key, String)> = s
        .markers
        .iter()
        .filter_map(|(k, _)| Some((k, s.marker_identity(k)?)))
        .collect();
    let old_k = unique(old_kids.iter().map(|(k, v)| (*k, v.as_str())));
    let knids: Vec<String> = (0..new.markers.len())
        .map(|m| new.marker_nid(m, &keys))
        .collect();
    let mut new_k: HashMap<&str, usize> = HashMap::new();
    for n in &knids {
        *new_k.entry(n.as_str()).or_default() += 1;
    }
    keys.markers = knids
        .iter()
        .map(|n| match old_k.get(n.as_str()) {
            Some(Some(k)) if new_k[n.as_str()] == 1 => (*k).clone(),
            _ => mint(2, 'k'),
        })
        .collect();
    (keys, next)
}

impl Pack for TimelineLive {
    fn schema(&self) -> Schema {
        SCHEMA
    }

    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        let lt = model::read(bytes, &self.limits)?;
        let (keys, marks) = lt.minted();
        Ok(PackContent {
            roots: lt.to_content(&keys, marks, &lt.doc_id),
            codec: CODEC.to_vec(),
        })
    }

    /// A timeline is written only by splicing its carrier ([`Pack::encode_from_paired`]). The
    /// kit calls this only for a doc it holds no carrier for (never synced with its file).
    fn encode(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        let _ = (content, codec);
        Err(PackError(
            "a live timeline is written by splicing its file's bytes, and none are kept for this \
             doc: open it from its file"
                .into(),
        ))
    }

    fn wants_carrier(&self) -> bool {
        true
    }

    fn wants_pairing(&self) -> bool {
        true
    }

    /// Tracks in order, each track's items in time order, markers by owner, media by first use
    /// ([`splice::pairing_of`]).
    fn pairing(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Pairing, PackError> {
        let _ = codec;
        Ok(splice::pairing_of(content))
    }

    fn encode_from_paired(
        &self,
        carrier: &[u8],
        pairing: &Pairing,
        content: &[(String, Neutral)],
        codec: &[u8],
    ) -> Result<Vec<u8>, SpliceError> {
        let _ = codec;
        splice::encode_from_paired(carrier, pairing, content, &self.limits)
    }

    fn view_schema(&self) -> Option<ViewSchema> {
        if crate::mutant::active("TimeUnitDropped") {
            // planted: the playhead's unit not declared
            return Some(ViewSchema {
                time_unit: None,
                ..view::VIEW
            });
        }
        if crate::mutant::active("SchemaExtraKey") {
            // planted: another derived name served beside the time unit and `tc`
            return Some(ViewSchema {
                unit_derived: &[view::TC, "playhead"],
                ..view::VIEW
            });
        }
        Some(view::VIEW)
    }

    fn page_derived(&self, state: &KeyedState, units: &[Key]) -> Vec<PageDerived> {
        view::page_derived(state, units)
    }

    /// Each track's label and kind, and each unit's start timecode ([`view::derive`]).
    fn view_derive(&self, source: ViewSource<'_>, index: &mut ViewIndex) {
        view::derive(source, index);
    }

    /// Every track's label and kind derive again when tracks came, went or moved or one was
    /// touched; the units a change touched derive again, every unit when it wrote the `timeline`
    /// map ([`view::derive_change`]).
    fn view_derive_change(
        &self,
        source: ViewSource<'_>,
        index: &mut ViewIndex,
        touched: &ViewTouched,
    ) -> bool {
        view::derive_change(source, index, touched)
    }

    fn address_of(&self, root: &str, key: &Key) -> String {
        view::address_of(root, key)
    }

    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        view::key_of(root, address)
    }

    /// A marker's container is its owner's track; a media's, none.
    fn container_of(&self, root: &str, key: &Key) -> Option<(String, Key)> {
        match root {
            TRACKS => Some((TRACKS.to_owned(), key.clone())),
            _ => None,
        }
    }

    /// An outside save, paired by key ([`pair_save`]); the marks never go down and the doc id
    /// stays.
    fn reconcile(
        &self,
        synced: &View,
        bytes: &[u8],
        _codec: &[u8],
    ) -> Result<Reconciled, Declined> {
        let new = model::read(bytes, &self.limits)?;
        let (keys, marks) = pair_save(synced, &new);
        let doc = synced
            .map(TIMELINE)
            .and_then(|m| m.get(t::DOC))
            .and_then(Value::as_str)
            .unwrap_or(&new.doc_id)
            .to_owned();
        let content = new.to_content(&keys, marks, &doc);
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

    /// The verbs' track index ([`index::TrackIndex`]), built from the live view.
    fn verb_state_build(&self, view: &View) -> Option<Box<dyn VerbState>> {
        Some(Box::new(index::TrackIndex::build(view)))
    }

    /// The track index kept through one change, from its ops.
    fn verb_state_change(
        &self,
        state: &mut (dyn VerbState + 'static),
        applied: &[k2_mpc_base::elem::Applied],
        view: &View,
    ) -> bool {
        let Some(ix) = state.downcast_mut::<index::TrackIndex>() else {
            return false;
        };
        if crate::mutant::active("IndexKeptStale") {
            // planted: kept, it says, and left as it was
            return true;
        }
        ix.change(applied, view);
        true
    }

    fn verbs(&self) -> &'static [k2_mpc_base::tools::PackVerb] {
        verbs::VERBS
    }

    fn errors(&self) -> &'static [k2_mpc_base::errors::ErrorEntry] {
        errors::ERRORS
    }

    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        COMPOSITE
    }
}

/// A key as an address: `i12` is `@item:12`; a composite map key adds its field
/// (`@item:12 geom`). Anything else as it is.
pub fn address_of_key(root: &str, key: &str) -> String {
    let (elem, fld) = match key.split_once(k2_mpc_base::table::CELL_SEP) {
        Some((e, x)) => (e, Some(x)),
        None => (key, None),
    };
    let list = match root {
        TRACK_PROPS => TRACKS,
        ITEM_PROPS => ITEMS,
        MARKER_PROPS => MARKERS,
        MEDIA_PROPS => MEDIA,
        _ => root,
    };
    let a = Key::new(elem).map_or_else(|_| elem.to_owned(), |k| view::address_of(list, &k));
    match fld {
        Some(x) => format!("{a} {x}"),
        None => a,
    }
}

/// One change as a terminal watcher prints it: its version, who and through which verb, then
/// each op by address (values cut at 80 bytes).
pub fn watch_line(e: &k2_mpc_base::session::ChangeEvent) -> String {
    use k2_mpc_base::elem::{Applied, ListOpKind, MapOpKind};
    let v = |x: &Option<Value>| -> String {
        let s = x.as_ref().and_then(Value::as_str).unwrap_or("");
        let (kept, _) = k2_mpc_base::index::cut_text(s, 80);
        kept.replace('\n', "\\n")
    };
    let ops: Vec<String> = match &e.applied {
        Applied::List { root, ops } => ops
            .iter()
            .map(|o| {
                let a = address_of_key(root, o.key.as_str());
                match o.kind {
                    ListOpKind::Insert => format!("add {a}"),
                    ListOpKind::Move => format!("move {a}"),
                    ListOpKind::Delete => format!("delete {a}"),
                    ListOpKind::Restore => format!("restore {a}"),
                    ListOpKind::Set => format!("kind {a} = {}", v(&o.value_after)),
                    _ => format!("drop {a}"),
                }
            })
            .collect(),
        Applied::Map { root, ops } => ops
            .iter()
            .map(|o| {
                let a = address_of_key(root, &o.key);
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
