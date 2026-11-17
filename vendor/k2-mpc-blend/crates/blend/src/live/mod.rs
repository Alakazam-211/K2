//! The live BLEND pack ([`BlendLive`]): a scene as a live keyed doc on the kit's structured
//! mode, an outliner of its collections and objects with their numbers, so a person can watch an
//! agent's verbs land. Off by default: the host opens a file live only when it chooses to.
//!
//! - [`model`]: the roots, their keys and fields, and the file read into them;
//! - [`check`]: what a live change may be ([`check_edit`]), the one rule of the write-back and
//!   of the kit's check of every live change before it applies ([`Pack::check_edit`]: a
//!   person's edit, a host's, an agent's verb, undo, redo, revert, restore);
//! - [`splice`]: the live content written back into the file by patching only what changed in
//!   place ([`splice::encode_from_paired`]);
//! - [`verbs`]: the agent's verbs on the live store, with bounded reads; each gives its payload
//!   as data for a call's result (`mpc.result/1`, the kit's), and refuses by the pack's
//!   registered entries ([`errors`]);
//! - [`errors`]: the pack's own refusals, registered ([`Pack::errors`]): the live verbs', the
//!   live tier's declines and the file verbs';
//! - [`view`]: the kit's view index of the scene, its addresses, and what the pack derives;
//! - [`place`]: each object's place in 3D on the kit's view slot, from its own fields and what
//!   only the file holds (the parent inverse, how it is parented, an axis and angle).
//! - [`merged`]: the pack's rules on a doc the kit merged field by field from an outside save
//!   ([`Pack::check_merged_view`]).
//! - [`reuse`]: a live state's K2SCENE1 without reading the file again
//!   ([`reuse::live_k2scene`]): a new header over the carrier's mesh data, shared.
//!
//! **Keys are minted** (`c<n>`, `o<n>`): a file's names don't fit the kit's key grammar, and a
//! name is no identity (a rename is a new file key). The file key travels as the `nid` field,
//! which a live rename sets in the same change, so the next outside save pairs the object
//! by name again.
//!
//! **The carrier and its pairing** (the kit's). Everything but the outliner's fields
//! (meshes, materials, modifiers, node trees, animation, images, caches: every other byte) never
//! enters the engine: the file is written back by patching the live changes into the bytes the
//! kit last read from the file or wrote to it, the **carrier**, which the kit keeps with the
//! carrier's **pairing**: the live keys in the order this pack's decode yields the collections and
//! objects. [`BlendLive`] opts in to both ([`Pack::wants_carrier`], [`Pack::wants_pairing`]); the
//! default [`Pack::pairing`] (each list root's keys in list order) is right, because the lists'
//! order is the decode's: live v1 adds, removes and moves nothing, and an outside save is taken
//! in in its own decode order. [`Pack::encode`] refuses: a scene isn't built from the doc alone.
//!
//! **Outside saves** ([`Pack::reconcile`]) are paired without guessing: a collection by its
//! `nid`; an object by its `nid`, else, among the objects left on both sides, exactly one with
//! the same data (or, with no data, the same parent); anything else is a new element, under a
//! key minted past the marks. The kit applies a save only onto a doc equal to the synced state,
//! and keeps both when the doc holds changes the file doesn't or the kit wrote since the last
//! outside save. When the host noted the version the app opened (the kit's `note_opened`), the
//! kit merges the save's changes since that version onto the live doc field by field, and this
//! pack checks the merged doc ([`merged::check_merged`]: one name per object of the file, each
//! file key its name's, values and holders the store's own); a doc that breaks one keeps both,
//! as with no version noted. So an app that reopened what the kit wrote and saved it merges,
//! with the kit's changes kept.

pub mod check;
pub mod errors;
pub mod merged;
pub mod model;
pub mod place;
pub mod reuse;
pub mod splice;
pub mod verbs;
pub mod view;

use std::collections::{BTreeMap, HashMap, HashSet};

use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Applied, Key, ListOp, MapOp, Value};
use k2_mpc_base::placement::{Placement, Space};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    Declined, Pack, PackContent, PackError, PackRefusal, Pairing, Reconciled, SpliceError,
    VerbState, View,
};
use k2_mpc_base::view_index::{ViewIndex, ViewSchema, ViewSource, ViewTouched};

pub use check::{check_edit, EditRefusal, Rule};
pub use model::{
    f, prop, CollKind, EditMode, LiveLimits, LiveScene, COLLS, COLL_PROPS, MAX_FILE_BYTES, OBJS,
    OBJ_PROPS, SCENE,
};
pub use reuse::{live_k2scene, LiveK2Scene, ReuseDeclined};
pub use view::{address_of, key_of};

/// The live schema. Its name is stamped on each stored doc.
pub const SCHEMA: Schema = Schema {
    name: "blend",
    roots: &[
        (COLLS, Kind::MovableList),
        (OBJS, Kind::MovableList),
        (COLL_PROPS, Kind::Map),
        (OBJ_PROPS, Kind::Map),
        (SCENE, Kind::Map),
    ],
};

/// The composite map roots, with their lists.
pub const COMPOSITE: &[(&str, &str)] = &[(COLL_PROPS, COLLS), (OBJ_PROPS, OBJS)];

/// The codec settings a live scene is stored with: a format byte. The splice reads everything
/// else from the carrier.
pub const CODEC: &[u8] = &[1];

/// The live pack for BLEND files. It holds no file bytes: the kit keeps the carrier.
#[derive(Debug, Default)]
pub struct BlendLive {
    /// The live tier's bounds.
    pub limits: LiveLimits,
}

impl BlendLive {
    /// The pack with these bounds.
    pub fn new(limits: LiveLimits) -> Self {
        BlendLive { limits }
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

/// One element's field of a composite map.
fn field<'v>(m: Option<&'v BTreeMap<String, Value>>, k: &Key, fld: &str) -> Option<&'v str> {
    m?.get(&prop(k, fld))?.as_str()
}

/// The highest `<prefix><n>` among a list root's keys, live or deleted.
fn store_max(synced: &View, root: &str, prefix: char) -> u64 {
    let live = synced.list(root).unwrap_or(&[]).iter().map(|(k, _)| k);
    let gone = synced.hidden(root).into_iter().flat_map(|h| h.keys());
    live.chain(gone)
        .filter_map(|k| model::key_num(k.as_str(), prefix))
        .max()
        .unwrap_or(0)
}

/// The keys an outside save's scene `new` takes against `synced` (the module's pairing rules),
/// and the marks after it.
pub fn pair_save(synced: &View, new: &LiveScene) -> (Vec<Key>, Vec<Key>, (u64, u64)) {
    let sm = synced.map(SCENE);
    let mark = |k: &str| -> u64 {
        sm.and_then(|m| m.get(k))
            .and_then(Value::as_str)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    let mut next_c = mark(model::s::MARK_C).max(store_max(synced, COLLS, 'c'));
    let mut next_o = mark(model::s::MARK_O).max(store_max(synced, OBJS, 'o'));
    // collections: by nid; c0 to c0
    let cp = synced.map(COLL_PROPS);
    let old_c: &[(Key, Value)] = synced.list(COLLS).unwrap_or(&[]);
    let mut by_nid: HashMap<&str, Vec<&Key>> = HashMap::new();
    for (k, _) in old_c {
        if let Some(n) = field(cp, k, f::NID).filter(|n| !n.is_empty()) {
            by_nid.entry(n).or_default().push(k);
        }
    }
    let mut new_nids: HashMap<&str, usize> = HashMap::new();
    for c in &new.colls {
        *new_nids.entry(c.nid.as_str()).or_default() += 1;
    }
    let ck: Vec<Key> = new
        .colls
        .iter()
        .map(|c| {
            if c.kind == CollKind::None {
                return Key::new(model::NONE_COLL).expect("c0");
            }
            match by_nid.get(c.nid.as_str()) {
                Some(v) if v.len() == 1 && !c.nid.is_empty() && new_nids[c.nid.as_str()] == 1 => {
                    v[0].clone()
                }
                _ => {
                    next_c += 1;
                    model::coll_key(next_c)
                }
            }
        })
        .collect();
    // objects: by nid, then by data (or, with no data, by parent), exactly one each side
    let op = synced.map(OBJ_PROPS);
    let old_o: &[(Key, Value)] = synced.list(OBJS).unwrap_or(&[]);
    let mut ok: Vec<Option<Key>> = vec![None; new.objs.len()];
    let mut used: HashSet<&Key> = HashSet::new();
    let mut old_by_nid: HashMap<&str, Vec<&Key>> = HashMap::new();
    for (k, _) in old_o {
        if let Some(n) = field(op, k, f::NID).filter(|n| !n.is_empty()) {
            old_by_nid.entry(n).or_default().push(k);
        }
    }
    let mut new_nid_count: HashMap<&str, usize> = HashMap::new();
    for o in &new.objs {
        *new_nid_count.entry(o.nid.as_str()).or_default() += 1;
    }
    for (i, o) in new.objs.iter().enumerate() {
        if o.nid.is_empty() || new_nid_count[o.nid.as_str()] != 1 {
            continue;
        }
        if let Some(v) = old_by_nid.get(o.nid.as_str()) {
            if v.len() == 1 && used.insert(v[0]) {
                ok[i] = Some(v[0].clone());
            }
        }
    }
    // the carried key rule, on what is left
    let old_nid_of = |k: &str| -> Option<&str> {
        let k = Key::new(k).ok()?;
        field(op, &k, f::NID)
    };
    let sig_old = |k: &Key| -> (String, String) {
        let kind = old_o
            .iter()
            .find(|(x, _)| x == k)
            .and_then(|(_, v)| v.as_str())
            .unwrap_or("")
            .to_owned();
        match field(op, k, f::DATA).filter(|d| !d.is_empty()) {
            Some(d) => (kind, format!("data {d}")),
            None => {
                let p = field(op, k, f::PARENT).unwrap_or("");
                let pn = old_nid_of(p).unwrap_or(p);
                (kind, format!("parent {pn}"))
            }
        }
    };
    let sig_new = |i: usize| -> (String, String) {
        let o = &new.objs[i];
        if !o.data.is_empty() {
            return (o.kind.clone(), format!("data {}", o.data));
        }
        let p = match &o.parent {
            model::Parent::None => String::new(),
            model::Parent::Obj(p) => new.objs[*p].nid.clone(),
            model::Parent::Other(s) => s.clone(),
        };
        (o.kind.clone(), format!("parent {p}"))
    };
    let left_old: Vec<&Key> = old_o
        .iter()
        .map(|(k, _)| k)
        .filter(|k| !used.contains(k))
        .collect();
    let left_new: Vec<usize> = (0..new.objs.len()).filter(|&i| ok[i].is_none()).collect();
    let mut old_sigs: HashMap<(String, String), Vec<&Key>> = HashMap::new();
    for k in &left_old {
        old_sigs.entry(sig_old(k)).or_default().push(k);
    }
    let mut new_sigs: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for &i in &left_new {
        new_sigs.entry(sig_new(i)).or_default().push(i);
    }
    for (sig, is) in &new_sigs {
        if let (Some(ks), [i]) = (old_sigs.get(sig), is.as_slice()) {
            if let [k] = ks.as_slice() {
                ok[*i] = Some((*k).clone());
            }
        }
    }
    let ok: Vec<Key> = ok
        .into_iter()
        .map(|k| {
            k.unwrap_or_else(|| {
                next_o += 1;
                model::obj_key(next_o)
            })
        })
        .collect();
    (ck, ok, (next_c, next_o))
}

impl Pack for BlendLive {
    fn schema(&self) -> Schema {
        SCHEMA
    }

    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        let scene = model::read(bytes, &self.limits)?;
        let (ck, ok) = scene.minted_keys();
        Ok(PackContent {
            roots: scene.to_content(&ck, &ok, scene.minted_marks()),
            codec: CODEC.to_vec(),
        })
    }

    /// A scene is written only by patching its carrier ([`Pack::encode_from_paired`]). The kit
    /// calls this only for a doc it holds no carrier for (never synced with its file).
    fn encode(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        let _ = (content, codec);
        Err(PackError(
            "a live scene is written by patching its file's bytes, and none are kept for this \
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

    /// Every live change is checked before it applies ([`Pack::check_edit`]).
    fn checks_edits(&self) -> bool {
        // planted: the kit never asks, so a change the write-back can't write is taken
        !crate::mutant::active("EditCheckOff")
    }

    /// A live change by the live rules ([`check_edit`], the write-back's own): refused at the
    /// call, the kit's `pack_refused` (`MPC-E823`) with the rule's code
    /// ([`EditRefusal::pack_refusal`]), so the doc never holds what its write-back can't write.
    fn check_edit(&self, view: &View, edits: &[Edit]) -> Result<(), PackRefusal> {
        check::check_edit(view, edits).map_err(|r| r.pack_refusal())
    }

    /// The live content patched into `carrier`, its units keyed by `pairing`
    /// ([`splice::encode_from_paired`]). Untouched is the carrier, byte for byte.
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
        if crate::mutant::active("CollParentDropped") {
            // planted: the collections' parents not declared
            return Some(ViewSchema {
                container_fields: k2_mpc_base::view_index::ContainerFields {
                    parent: k2_mpc_base::view_index::Src::None,
                    ..view::VIEW.container_fields
                },
                ..view::VIEW
            });
        }
        Some(view::VIEW)
    }

    fn view_derive(&self, source: ViewSource<'_>, index: &mut ViewIndex) {
        view::derive(source, index, &self.limits);
    }

    /// The units a change touched derive again; the rest stay ([`view::derive_change`]).
    fn view_derive_change(
        &self,
        source: ViewSource<'_>,
        index: &mut ViewIndex,
        touched: &ViewTouched,
    ) -> bool {
        view::derive_change(source, index, touched, &self.limits)
    }

    /// An object's placement ([`place::view_placement`]); collections have none.
    fn view_placement(&self, source: ViewSource<'_>, root: &str, key: &Key) -> Option<Placement> {
        if root != OBJS {
            return None;
        }
        place::view_placement(source, key, &self.limits)
    }

    /// The doc's space ([`place::view_space`]).
    fn view_space(&self, source: ViewSource<'_>) -> Option<Space> {
        place::view_space(source)
    }

    fn address_of(&self, root: &str, key: &Key) -> String {
        view::address_of(root, key)
    }

    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        view::key_of(root, address)
    }

    /// An outside save, paired by key ([`pair_save`]); the marks never go down.
    fn reconcile(
        &self,
        synced: &View,
        bytes: &[u8],
        _codec: &[u8],
    ) -> Result<Reconciled, Declined> {
        let new = model::read(bytes, &self.limits)?;
        let (ck, ok, marks) = pair_save(synced, &new);
        let content = new.to_content(&ck, &ok, marks);
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

    fn errors(&self) -> &'static [k2_mpc_base::errors::ErrorEntry] {
        errors::ERRORS
    }

    /// The verbs' index of the objects ([`verbs::ObjIndex`]): `scene list`, `collection show`,
    /// `object rename` and every verb's lookup of its object read it instead of every object.
    fn verb_state_build(&self, view: &View) -> Option<Box<dyn VerbState>> {
        Some(Box::new(verbs::ObjIndex::build(view)))
    }

    fn verb_state_change(
        &self,
        state: &mut (dyn VerbState + 'static),
        applied: &[Applied],
        view: &View,
    ) -> bool {
        state
            .downcast_mut::<verbs::ObjIndex>()
            .is_some_and(|ix| ix.change(applied, view))
    }

    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        COMPOSITE
    }

    /// A doc the kit merged field by field from an outside save, from the version the app
    /// opened: checked by the pack's rules ([`merged::check_merged`]). The hook sees the merged
    /// doc only; the save becomes the carrier, so the merged doc differs from it only where the
    /// live doc changed since that version.
    fn check_merged_view(&self, view: &View) -> Result<(), Declined> {
        merged::check_merged(view)
    }
}

/// A key as an address: `o12` is `@obj:12`, `c3` is `@coll:3`; a composite map key adds its
/// field (`@obj:12 xform`). Anything else as it is.
pub fn address_of_key(root: &str, key: &str) -> String {
    let (elem, fld) = match key.split_once(k2_mpc_base::table::CELL_SEP) {
        Some((e, x)) => (e, Some(x)),
        None => (key, None),
    };
    let list = match root {
        COLL_PROPS | COLLS => COLLS,
        OBJ_PROPS | OBJS => OBJS,
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
