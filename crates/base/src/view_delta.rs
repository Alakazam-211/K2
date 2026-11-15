//! View deltas: what one change did to a live doc's view, per
//! container and per unit, so a watcher keeps its view index from the change frames alone and
//! never runs pack code.
//!
//! The kit works them out from the change's ops and the pack's view schema
//! ([`crate::session::Pack::view_schema`]), generically: nothing here knows a file type.
//!
//! - **[`ContainerDelta`]**, one per op on the containers' list (`add`, `delete`, `restore`,
//!   `reorder`, a value `props`), with its 1-based positions before and after, in apply order;
//!   then one per container whose fields changed (`hide_show` when its visibility did, with
//!   `hidden_after`; else `props`), naming the fields. A container's fields set by the change
//!   that adds it fold into its `add`. A container that shifted only because another came or
//!   went isn't listed: the order says where it is.
//! - **[`UnitDelta`]**, one per unit whose place in the view changed. A unit is *shown* when it
//!   is live and its container field names a live container. One that comes into view gets
//!   `add` (inserted by this change) or `restore`; one that leaves it, `delete`; this includes a
//!   unit that comes or goes because its container did. A shown unit that stays gets an op per
//!   kind of change: `reorder` (moved in the units' list), `props` (a new list value, or its
//!   container changed), and by each changed field's class: the geometry field compared part by
//!   part (`move_resize` for `x`, `y`, `w`, `h`; `rotate`; `flip`), `text`, `style`,
//!   `hide_show` (with `hidden_after`) and `props` for the rest. `fields` names the fields that
//!   changed (on an add or a restore, every field the change set), never their values.
//! - **Geometry** travels as the field's own string (`x,y,w,h,rot,fh,fv` for a deck), before and
//!   after: `None` when the geometry wasn't touched, before an add, after a delete, and when the
//!   field is empty (a box inherited from a layout).
//! - **`container`** on a unit delta is its container's address (before a delete, after
//!   anything else): where a view puts it, with no lookup of the store's keys.
//! - Ops with no effect, and writes to deleted elements, show nothing and give nothing.
//!
//! A change frame also carries the **render hint** ([`ViewDeltas::hint`]): every container a
//! delta names, then the containers whose render hash the pack says changed.
//!
//! Several changes committed together (an undo of a group) are worked out as a run
//! ([`view_deltas_run`]): the state after the last is the only state there is, so each earlier
//! change reads the doc through the later changes' values before them.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};

use crate::elem::{Applied, Effect, Key, KeyedRoot, KeyedState, ListApplied, ListOpKind, Value};
use crate::errors::Value as Json;
use crate::placement::{Placement, Step};
use crate::session::{elem_of_map_key, Pack};
use crate::table::CELL_SEP;
use crate::view_index::{flag, parse_geom, FieldClass, Src, ViewIndex, ViewSchema};

/// What a change did to one unit, in the contract's words (`UnitDelta.ops`). Listed in this
/// order, each once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum UnitOp {
    Add,
    Delete,
    Restore,
    Reorder,
    MoveResize,
    Rotate,
    Flip,
    /// Its placement in 3D changed (the 3D slot; with `placement_after`).
    Place,
    Text,
    Style,
    HideShow,
    Props,
    /// A grid cell's value changed (a field of class [`FieldClass::Cell`]).
    Cell,
}

impl UnitOp {
    pub fn as_str(self) -> &'static str {
        match self {
            UnitOp::Add => "add",
            UnitOp::Delete => "delete",
            UnitOp::Restore => "restore",
            UnitOp::Reorder => "reorder",
            UnitOp::MoveResize => "move_resize",
            UnitOp::Rotate => "rotate",
            UnitOp::Flip => "flip",
            UnitOp::Place => "place",
            UnitOp::Text => "text",
            UnitOp::Style => "style",
            UnitOp::HideShow => "hide_show",
            UnitOp::Props => "props",
            UnitOp::Cell => "cell",
        }
    }
}

/// What a change did to one container (`ContainerDelta.op`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ContainerOp {
    Add,
    Delete,
    Restore,
    Reorder,
    HideShow,
    Props,
}

impl ContainerOp {
    pub fn as_str(self) -> &'static str {
        match self {
            ContainerOp::Add => "add",
            ContainerOp::Delete => "delete",
            ContainerOp::Restore => "restore",
            ContainerOp::Reorder => "reorder",
            ContainerOp::HideShow => "hide_show",
            ContainerOp::Props => "props",
        }
    }
}

/// One container's change (contract Item 2, `ContainerDelta`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ContainerDelta {
    /// Its address.
    pub key: String,
    pub op: ContainerOp,
    /// From 1, in the containers' live order just before the op (`None`: it wasn't shown, or
    /// the op doesn't move it).
    pub position_before: Option<u32>,
    /// From 1, just after the op.
    pub position_after: Option<u32>,
    /// With `hide_show`: whether it is hidden now. `None` otherwise.
    pub hidden_after: Option<bool>,
    /// With `hide_show` and `props`: the fields that changed, by name. Empty otherwise.
    pub fields: Vec<String>,
    /// For a schema whose 3D slot has container fields: its placement after the change, on
    /// `add` and `restore` always and otherwise when it changed (`Some(None)`: none now).
    pub placement_after: Option<Option<Placement>>,
    /// For a doc whose containers nest ([`crate::view_index::Container::parent`]): its parent's
    /// address after the change, on `add` and `restore` always and otherwise when it changed
    /// (`Some(None)`: at the top level now). A container whose parent changed with no delta of
    /// its own gets a `props` delta naming no field, with this ([`parent_deltas`]).
    pub parent_after: Option<Option<String>>,
}

impl ContainerDelta {
    /// As the change frame carries it: `placement_after` and `parent_after` only when it
    /// carries them.
    pub fn to_value(&self) -> Json {
        let v = Json::obj([
            ("key", Json::from(&self.key)),
            ("op", Json::from(self.op.as_str())),
            ("position_before", Json::from(self.position_before)),
            ("position_after", Json::from(self.position_after)),
            ("hidden_after", Json::from(self.hidden_after)),
            ("fields", Json::list(self.fields.iter())),
        ])
        .with_placement(&self.placement_after);
        match (v, &self.parent_after) {
            (Json::Obj(mut m), Some(p)) => {
                m.push(("parent_after".into(), Json::from(p.as_ref())));
                Json::Obj(m)
            }
            (v, _) => v,
        }
    }
}

/// One unit's change (contract Item 2, `UnitDelta`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnitDelta {
    /// Its address.
    pub key: String,
    /// Its container's address: before a `delete`, after anything else. `None` only when the
    /// container field names no container the doc has.
    pub container: Option<String>,
    /// In [`UnitOp`]'s order, each once; never empty.
    pub ops: Vec<UnitOp>,
    /// The geometry field's value before, as written (`None`: see the module's notes).
    pub geom_before: Option<String>,
    /// And after.
    pub geom_after: Option<String>,
    /// The fields that changed, in the order the change first wrote them.
    pub fields: Vec<String>,
    /// With `hide_show`: whether it is hidden now. `None` otherwise.
    pub hidden_after: Option<bool>,
    /// Its placeholder's type (`title`, `body`...), from the schema's placeholder field, so a
    /// view words the change without a lookup; `None` when it has none.
    pub placeholder_type: Option<String>,
    /// For a schema with a 3D slot: its placement after the change, on `add` and `restore`
    /// always and with the op `place` when it changed (`Some(None)`: `null`, none now).
    pub placement_after: Option<Option<Placement>>,
}

impl UnitDelta {
    /// As the change frame carries it.
    pub fn to_value(&self) -> Json {
        Json::obj([
            ("key", Json::from(&self.key)),
            ("container", Json::from(self.container.as_ref())),
            ("ops", Json::list(self.ops.iter().map(|o| o.as_str()))),
            ("geom_before", Json::from(self.geom_before.as_ref())),
            ("geom_after", Json::from(self.geom_after.as_ref())),
            ("fields", Json::list(self.fields.iter())),
            ("hidden_after", Json::from(self.hidden_after)),
            (
                "placeholder_type",
                Json::from(self.placeholder_type.as_ref()),
            ),
        ])
        .with_placement(&self.placement_after)
    }
}

/// `placement_after` on a delta's JSON, when it carries one.
trait WithPlacement {
    fn with_placement(self, p: &Option<Option<Placement>>) -> Json;
}

impl WithPlacement for Json {
    fn with_placement(self, p: &Option<Option<Placement>>) -> Json {
        match (self, p) {
            (Json::Obj(mut m), Some(p)) => {
                m.push((
                    "placement_after".into(),
                    p.as_ref().map_or(Json::Null, Placement::to_value),
                ));
                Json::Obj(m)
            }
            (v, _) => v,
        }
    }
}

/// One change's deltas.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ViewDeltas {
    /// In apply order: the containers' list ops first, then their fields.
    pub containers: Vec<ContainerDelta>,
    /// In the order the change first touched them; units that came or went with their container
    /// after them.
    pub units: Vec<UnitDelta>,
    /// For a 3D slot: the containers of everything placed through an element whose placement
    /// the change moved (the render hint names them after the deltas' own).
    pub placed_through: Vec<String>,
}

impl ViewDeltas {
    /// The render hint (contract Item 2, `containers`): every container a delta names (a
    /// container delta's own, a unit delta's container), then each of `render_hashes`' (the
    /// containers whose render hash changed), each once, in that order.
    pub fn hint<'a>(&self, render_hashes: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut put = |a: &str| {
            if seen.insert(a.to_owned()) {
                out.push(a.to_owned());
            }
        };
        for c in &self.containers {
            put(&c.key);
        }
        for u in &self.units {
            if let Some(c) = &u.container {
                put(c);
            }
        }
        for c in &self.placed_through {
            put(c);
        }
        for h in render_hashes {
            put(h);
        }
        out
    }
}

/// One change's deltas: `applied` its ops (every root's, in apply order), `after` the doc's
/// keyed state right after it. `schema` must fit the doc
/// ([`ViewSchema::check`]); one that doesn't gives what it can.
pub fn view_deltas<P: Pack + ?Sized>(
    pack: &P,
    schema: &ViewSchema,
    applied: &[Applied],
    after: &KeyedState,
) -> ViewDeltas {
    view_deltas_run(pack, schema, &[applied], after)
        .pop()
        .unwrap_or_default()
}

/// The deltas of a run of changes committed one after another, `after` the doc's keyed state
/// after the last of them: one [`ViewDeltas`] per change, in order. Each change reads the doc
/// as it was right after it: `after`, with the later changes' values before them.
pub fn view_deltas_run<P: Pack + ?Sized>(
    pack: &P,
    schema: &ViewSchema,
    changes: &[&[Applied]],
    after: &KeyedState,
) -> Vec<ViewDeltas> {
    let base = Base::new(pack, schema, after);
    let mut later = Overlay::default();
    let mut out = Vec::with_capacity(changes.len());
    for applied in changes.iter().rev() {
        let this = Overlay::of(applied);
        let deltas = {
            let a = At {
                base: &base,
                layers: [None, Some(&later)],
            };
            let b = At {
                base: &base,
                layers: [Some(&this), Some(&later)],
            };
            one(&base, applied, &b, &a)
        };
        later.under(this);
        out.push(deltas);
    }
    out.reverse();
    out
}

// ---------- reading the doc at one point of the run ----------

/// One element of a list as a point of the run sees it: `None`, not in the doc; else whether it
/// is live and its value.
type ElemAt = Option<(bool, Value)>;

/// What a change's ops say the doc held just before them: each key's first `before`.
#[derive(Default)]
struct Overlay {
    maps: HashMap<(String, String), Option<Value>>,
    lists: HashMap<(String, Key), ElemAt>,
}

impl Overlay {
    fn of(applied: &[Applied]) -> Overlay {
        let mut o = Overlay::default();
        for a in applied {
            match a {
                Applied::Text { .. } => {}
                Applied::List { root, ops } => {
                    for op in ops {
                        if matches!(op.effect, Effect::None { .. }) {
                            continue;
                        }
                        let before = op
                            .value_before
                            .clone()
                            .map(|v| (op.index_before.is_some(), v));
                        o.lists
                            .entry((root.clone(), op.key.clone()))
                            .or_insert(before);
                    }
                }
                Applied::Map { root, ops } => {
                    for op in ops {
                        if matches!(op.effect, Effect::None { .. }) {
                            continue;
                        }
                        o.maps
                            .entry((root.clone(), op.key.clone()))
                            .or_insert_with(|| op.before.clone());
                    }
                }
            }
        }
        o
    }

    /// `earlier`'s values replace this one's (it is the change before).
    fn under(&mut self, earlier: Overlay) {
        self.maps.extend(earlier.maps);
        self.lists.extend(earlier.lists);
    }
}

/// The doc after the run, indexed lazily, and the schema's roots.
struct Base<'a, P: Pack + ?Sized> {
    pack: &'a P,
    vs: &'a ViewSchema,
    state: &'a KeyedState,
    /// The composite maps of the containers and of the units.
    cmap: Option<&'static str>,
    umap: Option<&'static str>,
    live: [OnceCell<HashMap<&'a Key, &'a Value>>; 2],
    /// The live containers by their id (the first in order wins).
    ids: OnceCell<HashMap<String, Key>>,
}

const CONTAINERS: usize = 0;
const UNITS: usize = 1;

impl<'a, P: Pack + ?Sized> Base<'a, P> {
    fn new(pack: &'a P, vs: &'a ViewSchema, state: &'a KeyedState) -> Self {
        let map_of = |list: &str| {
            pack.composite()
                .iter()
                .find(|(_, l)| *l == list)
                .map(|(m, _)| *m)
        };
        Base {
            pack,
            vs,
            state,
            cmap: map_of(vs.containers),
            umap: map_of(vs.units),
            live: [OnceCell::new(), OnceCell::new()],
            ids: OnceCell::new(),
        }
    }

    fn root(&self, name: &str) -> Option<&'a KeyedRoot> {
        self.state
            .roots
            .iter()
            .find(|(r, _)| r == name)
            .map(|(_, r)| r)
    }

    fn list_name(&self, which: usize) -> &'static str {
        if which == CONTAINERS {
            self.vs.containers
        } else {
            self.vs.units
        }
    }

    fn live_index(&self, which: usize) -> &HashMap<&'a Key, &'a Value> {
        self.live[which].get_or_init(|| match self.root(self.list_name(which)) {
            Some(KeyedRoot::List { live, .. }) => live.iter().map(|(k, v)| (k, v)).collect(),
            _ => HashMap::new(),
        })
    }

    fn elem(&self, which: usize, key: &Key) -> ElemAt {
        if let Some(v) = self.live_index(which).get(key) {
            return Some((true, (*v).clone()));
        }
        match self.root(self.list_name(which)) {
            Some(KeyedRoot::List { hidden, .. }) => hidden.get(key).map(|v| (false, v.clone())),
            _ => None,
        }
    }

    fn map(&self, root: &str, key: &str) -> Option<Value> {
        match self.root(root) {
            Some(KeyedRoot::Map(m)) => m.get(key).cloned(),
            _ => None,
        }
    }

    /// Every container key the doc after the run has, live ones first in order.
    fn container_keys(&self) -> Vec<&'a Key> {
        match self.root(self.vs.containers) {
            Some(KeyedRoot::List { live, hidden, .. }) => {
                live.iter().map(|(k, _)| k).chain(hidden.keys()).collect()
            }
            _ => Vec::new(),
        }
    }

    fn ids(&self, id_field: &str) -> &HashMap<String, Key> {
        self.ids.get_or_init(|| {
            let mut m = HashMap::new();
            if let (Some(KeyedRoot::List { live, .. }), Some(cmap)) =
                (self.root(self.vs.containers), self.cmap)
            {
                for (k, _) in live {
                    let id = self.map(cmap, &field_key(k, id_field));
                    if let Some(Value::Str(id)) = id {
                        m.entry(id).or_insert_with(|| k.clone());
                    }
                }
            }
            m
        })
    }

    /// Every unit key the doc after the run has (live ones in order, then deleted ones).
    fn unit_keys(&self) -> Vec<&'a Key> {
        match self.root(self.vs.units) {
            Some(KeyedRoot::List { live, hidden, .. }) => {
                live.iter().map(|(k, _)| k).chain(hidden.keys()).collect()
            }
            _ => Vec::new(),
        }
    }
}

fn field_key(key: &Key, field: &str) -> String {
    format!("{}{CELL_SEP}{field}", key.as_str())
}

/// The doc at one point of a run: the base under up to two overlays, the first on top.
struct At<'a, 'b, P: Pack + ?Sized> {
    base: &'b Base<'a, P>,
    layers: [Option<&'b Overlay>; 2],
}

impl<P: Pack + ?Sized> At<'_, '_, P> {
    fn map(&self, root: &str, key: &str) -> Option<Value> {
        let k = (root.to_owned(), key.to_owned());
        for o in self.layers.iter().flatten() {
            if let Some(v) = o.maps.get(&k) {
                return v.clone();
            }
        }
        self.base.map(root, key)
    }

    fn elem(&self, which: usize, key: &Key) -> ElemAt {
        let k = (self.base.list_name(which).to_owned(), key.clone());
        for o in self.layers.iter().flatten() {
            if let Some(v) = o.lists.get(&k) {
                return v.clone();
            }
        }
        self.base.elem(which, key)
    }

    fn live(&self, which: usize, key: &Key) -> bool {
        matches!(self.elem(which, key), Some((true, _)))
    }

    /// Element `key`'s field of list `which`, as a string.
    fn field(&self, which: usize, key: &Key, field: &str) -> Option<String> {
        let map = if which == CONTAINERS {
            self.base.cmap
        } else {
            self.base.umap
        }?;
        match self.map(map, &field_key(key, field)) {
            Some(Value::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// What `src` reads of element `key` (a contract word for [`Src::Values`]), as the view
    /// index reads it.
    fn read(&self, which: usize, key: &Key, src: Src) -> Option<String> {
        match src {
            Src::None | Src::Derived(_) => None,
            Src::ListValue => match self.elem(which, key) {
                Some((_, Value::Str(s))) => Some(s),
                _ => None,
            },
            Src::Field(f) => self.field(which, key, f),
            Src::Values { field, values } => {
                let raw = self.field(which, key, field)?;
                values
                    .iter()
                    .find(|(r, _)| *r == raw)
                    .map(|(_, w)| (*w).to_owned())
            }
        }
    }

    /// The live container whose id is `id` (its key, or its `container_id_field`).
    fn container_of_id(&self, id: &str) -> Option<Key> {
        let vs = self.base.vs;
        let Some(f) = vs.container_id_field else {
            let k = Key::new(id).ok()?;
            return self.live(CONTAINERS, &k).then_some(k);
        };
        // the containers an overlay names (their id or liveness may differ from the base's)
        let mut named: Vec<Key> = Vec::new();
        for o in self.layers.iter().flatten() {
            for (r, k) in o.lists.keys() {
                if r == vs.containers {
                    named.push(k.clone());
                }
            }
            if let Some(cmap) = self.base.cmap {
                for (r, mk) in o.maps.keys() {
                    if r != cmap {
                        continue;
                    }
                    if let Some((_, k)) = elem_of_map_key(self.base.pack.composite(), r, mk) {
                        if mk.ends_with(&format!("{CELL_SEP}{f}")) {
                            named.push(k);
                        }
                    }
                }
            }
        }
        if !named.is_empty() {
            // in the base's order, so the first live one wins as the index's does
            let order: HashMap<&Key, usize> = self
                .base
                .container_keys()
                .into_iter()
                .enumerate()
                .map(|(i, k)| (k, i))
                .collect();
            named.sort_by_key(|k| order.get(k).copied().unwrap_or(usize::MAX));
            named.dedup();
            for k in &named {
                if self.live(CONTAINERS, k) && self.field(CONTAINERS, k, f).as_deref() == Some(id) {
                    return Some(k.clone());
                }
            }
        }
        let k = self.base.ids(f).get(id)?;
        if named.contains(k) {
            return None;
        }
        Some(k.clone())
    }

    /// The live container unit `key` is in at this point, if it is shown: live, its container
    /// field naming a live container.
    fn shown_in(&self, key: &Key) -> Option<Key> {
        if !self.live(UNITS, key) {
            return None;
        }
        self.unit_container(key)
    }

    /// The live container unit `key`'s container field names (whether or not it is live).
    fn unit_container(&self, key: &Key) -> Option<Key> {
        let id = self.field(UNITS, key, self.base.vs.unit_container_field)?;
        self.container_of_id(&id)
    }

    fn address(&self, which: usize, key: &Key) -> String {
        self.base.pack.address_of(self.base.list_name(which), key)
    }
}

// ---------- one change ----------

/// What one change touched of one element: its list ops and its fields, in order.
#[derive(Default)]
struct Touched<'c> {
    list: Vec<&'c ListApplied>,
    fields: Vec<String>,
}

/// The touched elements of list `which`, in first-touch order.
#[derive(Default)]
struct Touches<'c> {
    order: Vec<Key>,
    by: HashMap<Key, Touched<'c>>,
}

impl<'c> Touches<'c> {
    fn get(&mut self, k: &Key) -> &mut Touched<'c> {
        if !self.by.contains_key(k) {
            self.order.push(k.clone());
        }
        self.by.entry(k.clone()).or_default()
    }
}

// ----- the 3D slot's deltas -----

macro_rules! place_plant {
    ($name:ident, $variant:ident) => {
        fn $name() -> bool {
            plant_on!(
                crate::differential::place_mutant()
                    == Some(crate::differential::PlaceMutant::$variant)
            )
        }
    };
}

place_plant!(planted_place_dropped_on_rotation, PlaceDroppedOnRotation);
place_plant!(planted_no_placement_on_add, NoPlacementOnAdd);
place_plant!(planted_no_chain_hint, NoChainHint);

/// A placement's parent and translations only (the planted fault
/// [`crate::differential::PlaceMutant::PlaceDroppedOnRotation`] compares these alone).
fn locs(p: &Option<Placement>) -> Option<(Option<String>, Vec<u64>)> {
    let p = p.as_ref()?;
    let mut out = Vec::new();
    for s in &p.steps {
        let t: &[f64] = match s {
            Step::Trs { loc, .. } => loc,
            Step::Axes { origin, .. } => origin,
            Step::Matrix(m) => &m[12..15],
        };
        out.extend(t.iter().map(|x| x.to_bits()));
    }
    Some((p.relative_to.clone(), out))
}

fn same_place(a: &Option<Placement>, b: &Option<Placement>) -> bool {
    if planted_place_dropped_on_rotation() {
        return locs(a) == locs(b);
    }
    a == b
}

/// For a schema with a 3D slot, the placements of one change's deltas `d`, from the view index
/// before it and after it: every unit (and container, for a slot with container fields) that
/// comes into view (`add`, `restore`) carries `placement_after`; one that stays and whose
/// placement differs, number by number, gets `place` (a container: its `placement_after`); the
/// render hint learns the containers of everything placed through a moved element. Returns the
/// addresses whose placement the frame carries.
pub(crate) fn place_deltas(
    vs: &ViewSchema,
    d: &mut ViewDeltas,
    before: Option<&ViewIndex>,
    after: &ViewIndex,
) -> Vec<String> {
    let Some(slot) = vs.space else {
        return Vec::new();
    };
    let (mut named, mut moved) = (Vec::new(), Vec::new());
    for u in &mut d.units {
        if u.ops.contains(&UnitOp::Delete) {
            continue;
        }
        let now = after.unit(&u.key).and_then(|x| x.placement.clone());
        if u.ops
            .iter()
            .any(|o| matches!(o, UnitOp::Add | UnitOp::Restore))
        {
            if !planted_no_placement_on_add() {
                u.placement_after = Some(now);
                named.push(u.key.clone());
            }
            continue;
        }
        let was = before
            .and_then(|b| b.unit(&u.key))
            .and_then(|x| x.placement.clone());
        if !same_place(&was, &now) {
            u.ops.push(UnitOp::Place);
            u.ops.sort();
            u.ops.dedup();
            u.placement_after = Some(now);
            named.push(u.key.clone());
            moved.push(u.key.clone());
        }
    }
    if !slot.container_fields.is_empty() {
        for c in &mut d.containers {
            let now = after.container(&c.key).and_then(|x| x.placement.clone());
            match c.op {
                ContainerOp::Delete => {}
                ContainerOp::Add | ContainerOp::Restore => {
                    if !planted_no_placement_on_add() {
                        c.placement_after = Some(now);
                        named.push(c.key.clone());
                    }
                }
                _ => {
                    let was = before
                        .and_then(|b| b.container(&c.key))
                        .and_then(|x| x.placement.clone());
                    if !same_place(&was, &now) {
                        c.placement_after = Some(now);
                        named.push(c.key.clone());
                        moved.push(c.key.clone());
                    }
                }
            }
        }
    }
    if !moved.is_empty() && !planted_no_chain_hint() {
        d.placed_through = placed_through(after, &moved);
    }
    named
}

/// For a doc whose containers nest (the schema declares [`ContainerFields::parent`], or a
/// container of the index before or after has a parent), the parents of one change's deltas
/// `d`, from the view index before it and after it, so a watcher keeps the containers' tree
/// from the frames alone (it never maps a store's key to an address):
///
/// - a container that comes into view (`add`, `restore`) carries `parent_after`;
/// - one with another delta (not a `delete`) carries it when its parent differs;
/// - on the last change of a flush (`last`), every other container in both indexes whose parent
///   differs gets a `props` delta naming no field, with its `parent_after` (a parent the pack
///   derives from another root, one refused since): the frames then give the index's tree.
///
/// With no index before, every container counts as changed.
///
/// [`ContainerFields::parent`]: crate::view_index::ContainerFields::parent
pub(crate) fn parent_deltas(
    vs: &ViewSchema,
    d: &mut ViewDeltas,
    before: Option<&ViewIndex>,
    after: &ViewIndex,
    last: bool,
) {
    let has = |ix: &ViewIndex| ix.containers.iter().any(|c| c.parent.is_some());
    let nests = vs.container_fields.parent != Src::None || has(after) || before.is_some_and(has);
    if !nests || planted_frame_misses_parent() {
        return;
    }
    let was: Option<HashMap<&str, Option<&String>>> = before.map(|b| {
        b.containers
            .iter()
            .map(|c| (c.key.as_str(), c.parent.as_ref()))
            .collect()
    });
    let now: HashMap<&str, Option<&String>> = after
        .containers
        .iter()
        .map(|c| (c.key.as_str(), c.parent.as_ref()))
        .collect();
    // whether `key`'s parent differs from the index before (`true` without one)
    let differs = |key: &str, p: Option<&String>| match &was {
        Some(w) => w.get(key).map_or(true, |x| *x != p),
        None => true,
    };
    let mut carried: HashSet<String> = HashSet::new();
    for c in &mut d.containers {
        let Some(&p) = now.get(c.key.as_str()) else {
            continue;
        };
        let carry = match c.op {
            ContainerOp::Delete => false,
            ContainerOp::Add | ContainerOp::Restore => true,
            _ => differs(c.key.as_str(), p),
        };
        if carry {
            c.parent_after = Some(p.cloned());
            carried.insert(c.key.clone());
        }
    }
    if !last {
        return;
    }
    for c in &after.containers {
        if carried.contains(&c.key) {
            continue;
        }
        // one that came during the flush had its `add` (or `restore`), which carried it
        let came = was
            .as_ref()
            .is_some_and(|w| !w.contains_key(c.key.as_str()));
        if came || !differs(c.key.as_str(), c.parent.as_ref()) {
            continue;
        }
        d.containers.push(ContainerDelta {
            key: c.key.clone(),
            op: ContainerOp::Props,
            position_before: None,
            position_after: None,
            hidden_after: None,
            fields: Vec::new(),
            placement_after: None,
            parent_after: Some(c.parent.clone()),
        });
    }
}

/// The planted fault [`crate::differential::ViewMutant::FrameMissesParent`].
fn planted_frame_misses_parent() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::FrameMissesParent)
    )
}

/// The containers of everything placed, directly or through others, relative to one of
/// `moved` (a container placed through one counts as itself), in the index's order.
fn placed_through(ix: &ViewIndex, moved: &[String]) -> Vec<String> {
    let mut kids: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
    for u in &ix.units {
        if let Some(r) = u.placement.as_ref().and_then(|p| p.relative_to.as_deref()) {
            kids.entry(r).or_default().push((&u.key, &u.container));
        }
    }
    for c in &ix.containers {
        if let Some(r) = c.placement.as_ref().and_then(|p| p.relative_to.as_deref()) {
            kids.entry(r).or_default().push((&c.key, &c.key));
        }
    }
    let mut seen: HashSet<&str> = moved.iter().map(String::as_str).collect();
    let mut todo: Vec<&str> = moved.iter().map(String::as_str).collect();
    let mut hit: HashSet<&str> = HashSet::new();
    while let Some(a) = todo.pop() {
        for (k, c) in kids.get(a).into_iter().flatten() {
            hit.insert(c);
            if seen.insert(k) {
                todo.push(k);
            }
        }
    }
    ix.containers
        .iter()
        .filter(|c| hit.contains(c.key.as_str()))
        .map(|c| c.key.clone())
        .collect()
}

/// The planted fault [`crate::differential::WatchMutant::DeltaGeomStale`].
fn planted_delta_geom_stale() -> bool {
    plant_on!(
        crate::differential::watch_mutant()
            == Some(crate::differential::WatchMutant::DeltaGeomStale)
    )
}

fn class_of(vs: &ViewSchema, hidden: Src, field: &str) -> FieldClass {
    if vs.geometry.is_some_and(|g| g.field == field) {
        return FieldClass::Geometry;
    }
    if let Some(c) = vs.class_of_field(field) {
        return c;
    }
    let hides = match hidden {
        Src::Field(f) => f == field,
        Src::Values { field: f, .. } => f == field,
        _ => false,
    };
    if hides {
        FieldClass::Visibility
    } else {
        FieldClass::Meta
    }
}

fn nonempty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.is_empty())
}

/// Unit `k`'s placeholder type at `at` (the part of `<type>:<idx>` before the colon).
fn placeholder_type<P: Pack + ?Sized>(at: &At<'_, '_, P>, k: &Key) -> Option<String> {
    let v = at.read(UNITS, k, at.base.vs.unit_fields.placeholder)?;
    let ty = v.split_once(':').map_or(v.as_str(), |(t, _)| t);
    (!ty.is_empty()).then(|| ty.to_owned())
}

/// Unit `k`'s geometry value at `at`, when it has one.
fn geom_at<P: Pack + ?Sized>(at: &At<'_, '_, P>, k: &Key) -> Option<String> {
    let g = at.base.vs.geometry?;
    nonempty(at.field(UNITS, k, g.field))
}

fn one<P: Pack + ?Sized>(
    base: &Base<'_, P>,
    applied: &[Applied],
    before: &At<'_, '_, P>,
    after: &At<'_, '_, P>,
) -> ViewDeltas {
    let vs = base.vs;
    let composites = base.pack.composite();
    let mut cs = Touches::default();
    let mut us = Touches::default();
    for a in applied {
        match a {
            Applied::Text { .. } => {}
            Applied::List { root, ops } => {
                let t = if root == vs.containers {
                    &mut cs
                } else if root == vs.units {
                    &mut us
                } else {
                    continue;
                };
                for op in ops {
                    if matches!(op.effect, Effect::Visible) {
                        t.get(&op.key).list.push(op);
                    }
                }
            }
            Applied::Map { root, ops } => {
                let t = if Some(root.as_str()) == base.cmap {
                    &mut cs
                } else if Some(root.as_str()) == base.umap {
                    &mut us
                } else {
                    continue;
                };
                for op in ops {
                    if matches!(op.effect, Effect::None { .. }) {
                        continue;
                    }
                    let Some((_, k)) = elem_of_map_key(composites, root, &op.key) else {
                        continue;
                    };
                    let field = op.key[k.as_str().len() + CELL_SEP.len_utf8()..].to_owned();
                    let e = t.get(&k);
                    if !e.fields.contains(&field) {
                        e.fields.push(field);
                    }
                }
            }
        }
    }
    let mut out = ViewDeltas::default();

    // the containers: each list op in apply order, then each one's fields
    let mut came: HashSet<Key> = HashSet::new();
    let mut moved: HashSet<Key> = HashSet::new();
    let pos = |i: Option<usize>| i.and_then(|i| u32::try_from(i + 1).ok());
    for a in applied {
        let Applied::List { root, ops } = a else {
            continue;
        };
        if root != vs.containers {
            continue;
        }
        for op in ops.iter().filter(|op| matches!(op.effect, Effect::Visible)) {
            let kind = match op.kind {
                ListOpKind::Insert => ContainerOp::Add,
                ListOpKind::Delete => ContainerOp::Delete,
                ListOpKind::Restore => ContainerOp::Restore,
                ListOpKind::Move => ContainerOp::Reorder,
                ListOpKind::Set => ContainerOp::Props,
                _ => continue,
            };
            if matches!(kind, ContainerOp::Add | ContainerOp::Restore) {
                came.insert(op.key.clone());
            }
            if !matches!(kind, ContainerOp::Reorder | ContainerOp::Props) {
                moved.insert(op.key.clone());
            }
            out.containers.push(ContainerDelta {
                key: after.address(CONTAINERS, &op.key),
                op: kind,
                position_before: pos(op.index_before),
                position_after: pos(op.index_after),
                hidden_after: None,
                fields: Vec::new(),
                placement_after: None,
                parent_after: None,
            });
        }
    }
    let chidden = vs.container_fields.hidden;
    for k in &cs.order {
        let t = &cs.by[k];
        if t.fields.is_empty() || !after.live(CONTAINERS, k) {
            continue;
        }
        if came.contains(k) && !before.live(CONTAINERS, k) {
            continue;
        }
        let changed: Vec<String> = t
            .fields
            .iter()
            .filter(|f| before.field(CONTAINERS, k, f) != after.field(CONTAINERS, k, f))
            .cloned()
            .collect();
        if changed.is_empty() {
            continue;
        }
        if let Some(f) = vs.container_id_field {
            if changed.iter().any(|c| c == f) {
                moved.insert(k.clone());
            }
        }
        let hides = changed
            .iter()
            .any(|f| class_of(vs, chidden, f) == FieldClass::Visibility);
        out.containers.push(ContainerDelta {
            key: after.address(CONTAINERS, k),
            op: if hides {
                ContainerOp::HideShow
            } else {
                ContainerOp::Props
            },
            position_before: None,
            position_after: None,
            hidden_after: hides.then(|| flag(after.read(CONTAINERS, k, chidden).as_deref())),
            fields: changed,
            placement_after: None,
            parent_after: None,
        });
    }

    // the units: those the change touched, then those that came or went with their container
    let mut keys: Vec<Key> = us.order.clone();
    if !moved.is_empty() {
        let seen: HashSet<&Key> = us.order.iter().collect();
        for k in base.unit_keys() {
            if seen.contains(k) {
                continue;
            }
            let was = before.shown_in(k);
            let now = after.shown_in(k);
            let hits = |c: &Option<Key>| c.as_ref().is_some_and(|c| moved.contains(c));
            if was != now && (hits(&was) || hits(&now)) {
                keys.push(k.clone());
            }
        }
    }
    let uf = vs.unit_fields;
    let none = Touched::default();
    for k in &keys {
        let t = us.by.get(k).unwrap_or(&none);
        let was = before.shown_in(k);
        let now = after.shown_in(k);
        let changed = |f: &String| before.field(UNITS, k, f) != after.field(UNITS, k, f);
        let d = match (&was, &now) {
            (None, None) => continue,
            (Some(c), None) => UnitDelta {
                key: after.address(UNITS, k),
                container: Some(before.address(CONTAINERS, c)),
                ops: vec![UnitOp::Delete],
                geom_before: geom_at(before, k),
                geom_after: None,
                fields: Vec::new(),
                hidden_after: None,
                placeholder_type: placeholder_type(before, k),
                placement_after: None,
            },
            (None, Some(c)) => {
                let inserted = t
                    .list
                    .first()
                    .is_some_and(|op| op.kind == ListOpKind::Insert);
                UnitDelta {
                    key: after.address(UNITS, k),
                    container: Some(after.address(CONTAINERS, c)),
                    ops: vec![if inserted {
                        UnitOp::Add
                    } else {
                        UnitOp::Restore
                    }],
                    geom_before: None,
                    geom_after: geom_at(after, k),
                    fields: t.fields.iter().filter(|f| changed(f)).cloned().collect(),
                    hidden_after: None,
                    placeholder_type: placeholder_type(after, k),
                    placement_after: None,
                }
            }
            (Some(cb), Some(ca)) => {
                let mut ops: Vec<UnitOp> = Vec::new();
                for op in &t.list {
                    ops.push(match op.kind {
                        ListOpKind::Set => UnitOp::Props,
                        _ => UnitOp::Reorder,
                    });
                }
                let mut fields = Vec::new();
                let (mut gb, mut ga) = (None, None);
                for f in t.fields.iter().filter(|f| changed(f)) {
                    match class_of(vs, uf.hidden, f) {
                        FieldClass::Geometry => {
                            // a geometry field that isn't the 2D slot's (a scene's transform,
                            // with no slot at all): it moved, with no box before or after
                            let Some(g) = vs.geometry.filter(|g| g.field == f.as_str()) else {
                                ops.push(UnitOp::MoveResize);
                                fields.push(f.clone());
                                continue;
                            };
                            let (b, a) = (
                                before.field(UNITS, k, f).unwrap_or_default(),
                                after.field(UNITS, k, f).unwrap_or_default(),
                            );
                            let mut gops = Vec::new();
                            match (parse_geom(&b, g.parts), parse_geom(&a, g.parts)) {
                                (Some((bb, br, bf)), Some((ab, ar, af))) => {
                                    if bb != ab {
                                        gops.push(UnitOp::MoveResize);
                                    }
                                    if br != ar {
                                        gops.push(UnitOp::Rotate);
                                    }
                                    if bf != af {
                                        gops.push(UnitOp::Flip);
                                    }
                                }
                                (None, None) => {}
                                _ => gops.push(UnitOp::MoveResize),
                            }
                            if gops.is_empty() {
                                continue;
                            }
                            ops.extend(gops);
                            gb = nonempty(Some(b));
                            ga = nonempty(Some(a));
                        }
                        FieldClass::Text => ops.push(UnitOp::Text),
                        FieldClass::Style => ops.push(UnitOp::Style),
                        FieldClass::Visibility => ops.push(UnitOp::HideShow),
                        FieldClass::Meta => ops.push(UnitOp::Props),
                        FieldClass::Cell => ops.push(UnitOp::Cell),
                    }
                    fields.push(f.clone());
                }
                if cb != ca {
                    ops.push(UnitOp::Props);
                }
                if ops.is_empty() {
                    continue;
                }
                ops.sort();
                ops.dedup();
                let hides = ops.contains(&UnitOp::HideShow);
                UnitDelta {
                    key: after.address(UNITS, k),
                    container: Some(after.address(CONTAINERS, ca)),
                    ops,
                    geom_before: gb,
                    geom_after: ga,
                    fields,
                    hidden_after: hides.then(|| flag(after.read(UNITS, k, uf.hidden).as_deref())),
                    placeholder_type: placeholder_type(after, k),
                    placement_after: None,
                }
            }
        };
        out.units.push(d);
    }
    if planted_delta_geom_stale() {
        for u in &mut out.units {
            if u.geom_before.is_some() && u.geom_after.is_some() {
                u.geom_after = u.geom_before.clone();
            }
        }
    }
    out
}
