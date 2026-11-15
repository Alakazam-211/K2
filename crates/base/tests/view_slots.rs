//! The view index's per-unit derived values and its keeping through changes, on a scene-shaped
//! toy pack: groups (containers) holding objects (units), each object's children nested under it
//! by the pack, and two derived values per unit (`transform`, `label`) the pack works out from
//! the object's fields and the scene's label prefix. Public API only.
//!
//! The rows:
//! - the derived values ride the snapshot's units pages and the host's reads by unit and by
//!   container, as the schema declares them (an undeclared name dropped, a long one cut); a
//!   change frame carries the values a change set or removed, and only those;
//! - the schema's names are checked; a pack that declares none encodes as before;
//! - the index kept through changes is a whole build's, after every one of a seeded run of
//!   changes of every kind, read after every change or after several;
//! - the counts: a whole build of 2,500 units builds 2,500, a change to one unit builds one
//!   (the measurement prints both, with the same job's times);
//! - a pack that doesn't derive by change is asked once and built whole; a pack whose
//!   incremental derive is wrong is refused loudly in test builds.
//!
//! Planted faults (`k2_mpc_base::differential::ViewMutant`) are caught by name: each
//! `plant_*_is_caught` row needs its check to pass as built and to fail under the fault where
//! it says.

use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::differential::{
    set_view_checks, take_view_unit_builds, with_view_mutant, ViewMutant,
};
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, KeyedRoot, KeyedState, ListOp, MapOp};
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::grid::GridSlot;
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    watch_state_json, AuditEvent, Config, ConnId, Declined, EditOptions, Host, Pack, PackContent,
    PackError, Participant, ParticipantKind, Rights, Session, SessionManager, Stay, WatchFrame,
    WatchOptions, KEYED_WATCH_FEATURE,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::view_index::{
    unit_value, ContainerFields, Derived, FieldClass, PageSpec, Src, Unit, UnitFields, ViewIndex,
    ViewSchema, ViewSource, ViewTouched, UNIT_DERIVED_MAX, VIEW_TEXT_MAX,
};
use k2_mpc_base::{Key, Value};

const GROUPS: &str = "groups";
const GROUP_F: &str = "group_f";
const OBJS: &str = "objs";
const OBJ_F: &str = "obj_f";
const SCENE: &str = "scene";

const SCHEMA: Schema = Schema {
    name: "scene-lite",
    roots: &[
        (GROUPS, Kind::MovableList),
        (GROUP_F, Kind::Map),
        (OBJS, Kind::MovableList),
        (OBJ_F, Kind::Map),
        (SCENE, Kind::Map),
    ],
};

/// The toy's view schema: groups hold objects; an object's `group` field holds its group's key.
const VIEW: ViewSchema = ViewSchema {
    containers: GROUPS,
    units: OBJS,
    unit_container_field: "group",
    container_id_field: None,
    native_unit: "scene-unit",
    page: Some(PageSpec {
        root: SCENE,
        w: "w",
        h: "h",
    }),
    container_fields: ContainerFields {
        title: Src::Field("name"),
        hidden: Src::Field("hidden"),
        layout: Src::None,
        parent: Src::None,
    },
    unit_fields: UnitFields {
        kind: Src::ListValue,
        name: Src::Field("name"),
        hidden: Src::Field("hidden"),
        ..UnitFields::NONE
    },
    field_class: &[
        ("xform", FieldClass::Geometry),
        ("name", FieldClass::Text),
        ("hidden", FieldClass::Visibility),
        ("group", FieldClass::Meta),
        ("kids", FieldClass::Meta),
        ("nolabel", FieldClass::Meta),
    ],
    derived: Derived {
        containers: &[],
        units: &[],
    },
    unit_derived: &["transform", "label"],
    ..ViewSchema::NONE
};

// ----- the pack -----

/// How the toy derives by change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Derives again what the change touched (and every label when the prefix changed).
    ByChange,
    /// Doesn't derive by change: the kit builds the index whole.
    Whole,
    /// Says it derived by change, but derives nothing: a pack bug.
    Wrong,
}

struct ScenePack(Mode);

thread_local! {
    /// Top-level units the pack derived on this thread.
    static DERIVED: Cell<u64> = const { Cell::new(0) };
    /// Times the kit asked the pack to derive by change on this thread.
    static ASKED: Cell<u64> = const { Cell::new(0) };
}

fn take_derived() -> u64 {
    DERIVED.with(|c| c.replace(0))
}

fn take_asked() -> u64 {
    ASKED.with(|c| c.replace(0))
}

fn group_key(n: u32) -> Key {
    Key::new(format!("g{n}")).unwrap()
}

fn obj_key(n: u32) -> Key {
    Key::new(format!("o{n}")).unwrap()
}

fn prop(k: &Key, f: &str) -> String {
    format!("{k}\u{1f}{f}")
}

/// A number with no sign and no leading zero.
fn canon_num(s: &str) -> Option<u32> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    s.parse().ok()
}

fn map_root<'a>(state: &'a KeyedState, name: &str) -> Option<&'a BTreeMap<String, Value>> {
    state.roots.iter().find_map(|(r, root)| match root {
        KeyedRoot::Map(m) if r == name => Some(m),
        _ => None,
    })
}

/// One top-level unit's derived values and the nested units under it (its `kids`): one
/// unit derived. `transform` from `xform`; `label` the prefix and the name unless `nolabel`;
/// a name the schema doesn't declare, which the kit drops.
fn derive_unit(props: Option<&BTreeMap<String, Value>>, prefix: &str, u: &mut Unit) -> Vec<Unit> {
    DERIVED.with(|c| c.set(c.get() + 1));
    let Some(k) = ScenePack(Mode::Whole).key_of(OBJS, &u.key) else {
        return Vec::new();
    };
    let f = |name: &str| {
        props
            .and_then(|m| m.get(&prop(&k, name)))
            .and_then(|v| v.as_str())
    };
    if let Some(x) = f("xform") {
        u.set_derived("transform", format!("xform {x}"));
    }
    if f("nolabel") != Some("1") {
        u.set_derived("label", format!("{prefix}{}", u.name));
    }
    u.set_derived("secret", "not declared");
    let mut kids = Vec::new();
    for (i, name) in f("kids")
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .enumerate()
    {
        let mut kid = Unit {
            key: format!("{}/kid:{name}", u.key),
            container: u.container.clone(),
            parent: Some(u.key.clone()),
            live: false,
            kind: "kid".into(),
            name: name.into(),
            z: i as u32,
            ..Unit::default()
        };
        kid.set_derived("label", format!("{prefix}{name}"));
        kids.push(kid);
    }
    kids
}

fn prefix_of(state: &KeyedState) -> String {
    map_root(state, SCENE)
        .and_then(|m| m.get("prefix"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned()
}

/// The whole derive: every top-level unit's values and nested units.
fn derive_whole(state: &KeyedState, index: &mut ViewIndex) {
    let props = map_root(state, OBJ_F);
    let prefix = prefix_of(state);
    let mut out = Vec::with_capacity(index.units.len());
    for mut u in std::mem::take(&mut index.units) {
        if u.parent.is_some() {
            continue;
        }
        let kids = derive_unit(props, &prefix, &mut u);
        out.push(u);
        out.extend(kids);
    }
    index.units = out;
}

/// The derive by change: the touched units again, every unit when the scene map (the label
/// prefix) changed; every other unit, and its nested units, as they are.
fn derive_change(state: &KeyedState, index: &mut ViewIndex, touched: &ViewTouched) {
    let props = map_root(state, OBJ_F);
    let prefix = prefix_of(state);
    let all = touched.roots.iter().any(|r| r == SCENE);
    let set: HashSet<&str> = touched.units.iter().map(String::as_str).collect();
    let mut out = Vec::with_capacity(index.units.len());
    for mut u in std::mem::take(&mut index.units) {
        if u.parent.is_some() {
            if !all {
                out.push(u);
            }
            continue;
        }
        if all || set.contains(u.key.as_str()) {
            let kids = derive_unit(props, &prefix, &mut u);
            out.push(u);
            out.extend(kids);
        } else {
            out.push(u);
        }
    }
    index.units = out;
}

/// The file: `SCENE|w|h|prefix`, then `G|n|name|hidden` and
/// `O|group|n|kind|name|xform|hidden|kids|nolabel` lines (`-` for none).
fn decode(bytes: &[u8]) -> Result<PackContent, Declined> {
    let bad = |m: String| Declined {
        code: "not_scene",
        message: m,
    };
    let text = std::str::from_utf8(bytes).map_err(|_| bad("not utf-8".into()))?;
    let mut lines = text.lines();
    let head: Vec<&str> = lines.next().unwrap_or("").split('|').collect();
    let ["SCENE", w, h, prefix] = head[..] else {
        return Err(bad("no SCENE header".into()));
    };
    let mut scene = BTreeMap::new();
    scene.insert("w".to_owned(), Value::str(w));
    scene.insert("h".to_owned(), Value::str(h));
    scene.insert("prefix".to_owned(), Value::str(prefix));
    let (mut groups, mut objs) = (Vec::new(), Vec::new());
    let (mut gf, mut of) = (BTreeMap::new(), BTreeMap::new());
    let num = |s: &str| canon_num(s).ok_or_else(|| bad(format!("bad number {s:?}")));
    for line in lines {
        let f: Vec<&str> = line.split('|').collect();
        match f[..] {
            ["G", n, name, hidden] => {
                let k = group_key(num(n)?);
                gf.insert(prop(&k, "name"), Value::str(name));
                gf.insert(prop(&k, "hidden"), Value::str(hidden));
                groups.push((k, Value::str("group")));
            }
            ["O", g, n, kind, name, xform, hidden, kids, nolabel] => {
                let k = obj_key(num(n)?);
                of.insert(prop(&k, "group"), Value::str(group_key(num(g)?).as_str()));
                of.insert(prop(&k, "name"), Value::str(name));
                of.insert(prop(&k, "hidden"), Value::str(hidden));
                if xform != "-" {
                    of.insert(prop(&k, "xform"), Value::str(xform));
                }
                if kids != "-" {
                    of.insert(prop(&k, "kids"), Value::str(kids));
                }
                if nolabel == "1" {
                    of.insert(prop(&k, "nolabel"), Value::str("1"));
                }
                objs.push((k, Value::str(kind)));
            }
            _ => return Err(bad(format!("bad line {line:?}"))),
        }
    }
    Ok(PackContent {
        roots: vec![
            (GROUPS.into(), Neutral::List(groups)),
            (GROUP_F.into(), Neutral::Map(gf)),
            (OBJS.into(), Neutral::List(objs)),
            (OBJ_F.into(), Neutral::Map(of)),
            (SCENE.into(), Neutral::Map(scene)),
        ],
        codec: Vec::new(),
    })
}

impl Pack for ScenePack {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[(GROUP_F, GROUPS), (OBJ_F, OBJS)]
    }
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        decode(bytes)
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(Vec::new())
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        Some(VIEW)
    }
    fn view_derive(&self, source: ViewSource<'_>, index: &mut ViewIndex) {
        if let ViewSource::Live(state, _) = source {
            derive_whole(state, index);
        }
    }
    fn view_derive_change(
        &self,
        source: ViewSource<'_>,
        index: &mut ViewIndex,
        touched: &ViewTouched,
    ) -> bool {
        ASKED.with(|c| c.set(c.get() + 1));
        match self.0 {
            Mode::Whole => false,
            Mode::Wrong => true,
            Mode::ByChange => {
                if let ViewSource::Live(state, _) = source {
                    derive_change(state, index, touched);
                }
                true
            }
        }
    }
    fn address_of(&self, root: &str, key: &Key) -> String {
        match root {
            GROUPS => key.as_str().strip_prefix('g').map(|n| format!("@grp:{n}")),
            OBJS => key.as_str().strip_prefix('o').map(|n| format!("@obj:{n}")),
            _ => None,
        }
        .unwrap_or_else(|| key.to_string())
    }
    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        match root {
            GROUPS => Some(group_key(canon_num(address.strip_prefix("@grp:")?)?)),
            OBJS => Some(obj_key(canon_num(address.strip_prefix("@obj:")?)?)),
            _ => None,
        }
    }
}

// ----- the fixture -----

struct TestHost {
    now: AtomicU64,
}

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct TmpDir(std::path::PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    _dir: TmpDir,
    _mgr: SessionManager,
    s: Arc<Session>,
    ada: ConnId,
}

fn fx(tag: &str, mode: Mode, bytes: &[u8]) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-slots-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(100_000),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host, c);
    let key = DocKey {
        space: "ws".into(),
        path: format!("{tag}.scene"),
    };
    let s = mgr
        .open(Arc::new(ScenePack(mode)), &key, Some(bytes))
        .unwrap();
    let ada = s
        .join(
            Participant::new("ada", "Ada", "#2f9e8f", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    Fx {
        _dir: TmpDir(dir),
        _mgr: mgr,
        s,
        ada,
    }
}

impl Fx {
    fn version(&self) -> u64 {
        self.s.status().unwrap().version
    }

    /// One change of `edits`.
    fn edit(&self, edits: Vec<Edit>) {
        let mut opts = EditOptions::new();
        opts.base = Some(self.version());
        self.s.edit_ops(self.ada, edits, &opts).unwrap();
    }

    fn set(&self, root: &str, key: &Key, field: &str, value: &str) {
        self.edit(vec![set_op(root, key, field, value)]);
    }

    fn index(&self) -> Result<ViewIndex, String> {
        self.s
            .view_index(self.ada)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no view index".to_owned())
    }

    /// The index a whole build gives for the doc now.
    fn whole(&self) -> ViewIndex {
        let state = self.s.keyed_state_for_tests().unwrap();
        ViewIndex::live(&ScenePack(Mode::Whole), &state, self.version()).unwrap()
    }
}

fn set_op(root: &str, key: &Key, field: &str, value: &str) -> Edit {
    Edit::Map {
        root: root.into(),
        ops: vec![MapOp::Set {
            key: prop(key, field),
            value: Value::str(value),
        }],
    }
}

/// A text longer than the view keeps.
fn long_xform() -> String {
    "9".repeat(VIEW_TEXT_MAX + 10)
}

/// The sample scene: three groups (the third hidden and empty), four objects (one with two
/// children, one with no transform and no label, one with a transform past the cut).
fn sample() -> Vec<u8> {
    format!(
        "SCENE|1920|1080|>\n\
         G|1|Props|0\n\
         G|2|Lights|0\n\
         G|3|Empty|1\n\
         O|1|1|mesh|Cube|0,0,0|0|-|0\n\
         O|1|2|empty|Rig|1,2,3|0|arm,leg|0\n\
         O|2|3|light|Sun|-|1|-|1\n\
         O|1|4|mesh|Long|{}|0|-|0\n",
        long_xform()
    )
    .into_bytes()
}

/// A scene of `groups` groups of `per` objects each, every one with a transform.
fn big(groups: u32, per: u32) -> Vec<u8> {
    let mut s = String::from("SCENE|1920|1080|>\n");
    for g in 1..=groups {
        s.push_str(&format!("G|{g}|Group {g}|0\n"));
    }
    for g in 1..=groups {
        for i in 1..=per {
            let n = (g - 1) * per + i;
            s.push_str(&format!("O|{g}|{n}|mesh|Object {n}|{n},0,0|0|-|0\n"));
        }
    }
    s.into_bytes()
}

/// Every unit's derived values as JSON (`{name: text}`), by address, in the index's order, from
/// a snapshot's units pages; `None` for a unit whose page entry has no `derived`.
fn page_derived(frames: &[WatchFrame]) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for f in frames {
        let v = f.to_value();
        let Some(Json::List(units)) = v.get("units").and_then(|u| u.get("units")) else {
            continue;
        };
        for u in units {
            let key = u
                .get("key")
                .and_then(Json::as_str)
                .unwrap_or("?")
                .to_owned();
            out.push((key, u.get("derived").map(Json::to_json)));
        }
    }
    out
}

/// The change frames among `frames`, as JSON.
fn changes(frames: &[WatchFrame]) -> Vec<Json> {
    frames
        .iter()
        .filter(|f| matches!(f, WatchFrame::Change(_)))
        .map(WatchFrame::to_value)
        .collect()
}

// ----- the rows: the derived values on the wire and in the reads -----

/// The derived values ride each unit of the snapshot's pages, as the schema declares them (in
/// its order, an undeclared name dropped, a long text cut, a unit with none has no `derived`),
/// the schema names them, and the host reads them by unit (a nested one too) and by container.
fn row_unit_derived_rides_the_snapshot_and_the_reads() -> Result<(), String> {
    const ROW: &str = "row_unit_derived_rides_the_snapshot_and_the_reads";
    let f = fx("pages", Mode::ByChange, &sample());
    let mut w =
        f.s.watch_keyed(f.ada, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
            .map_err(|e| e.to_string())?
            .ok_or("control: no watch")?;
    let frames = f.s.watch_snapshot(&mut w).map_err(|e| e.to_string())?;
    let head = frames[0].to_value();
    let names = head
        .get("view")
        .and_then(|v| v.get("schema"))
        .and_then(|s| s.get("unit_derived"))
        .map(Json::to_json);
    if names.as_deref() != Some(r#"["transform","label"]"#) {
        return Err(format!(
            "{ROW}: the snapshot's schema names unit_derived {names:?}"
        ));
    }
    let long = format!("xform {}", long_xform());
    let cut = &long[..VIEW_TEXT_MAX];
    let want: Vec<(String, Option<String>)> = vec![
        (
            "@obj:1".into(),
            Some(r#"{"transform":"xform 0,0,0","label":">Cube"}"#.into()),
        ),
        (
            "@obj:2".into(),
            Some(r#"{"transform":"xform 1,2,3","label":">Rig"}"#.into()),
        ),
        ("@obj:2/kid:arm".into(), Some(r#"{"label":">arm"}"#.into())),
        ("@obj:2/kid:leg".into(), Some(r#"{"label":">leg"}"#.into())),
        (
            "@obj:4".into(),
            Some(format!(r#"{{"transform":"{cut}","label":">Long"}}"#)),
        ),
        ("@obj:3".into(), None),
    ];
    let got = page_derived(&frames);
    if got != want {
        return Err(format!(
            "{ROW}: the units pages' derived values {got:?}, want {want:?}"
        ));
    }
    let u =
        f.s.view_unit(f.ada, "@obj:2")
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{ROW}: no unit @obj:2 by address"))?;
    let want2 = vec![
        ("transform".to_owned(), "xform 1,2,3".to_owned()),
        ("label".to_owned(), ">Rig".to_owned()),
    ];
    if u.derived != want2 {
        return Err(format!(
            "{ROW}: @obj:2 read by address has derived {:?}, want {want2:?}",
            u.derived
        ));
    }
    let kid =
        f.s.view_unit(f.ada, "@obj:2/kid:leg")
            .map_err(|e| e.to_string())?;
    if kid.as_ref().and_then(|k| k.derived_value("label")) != Some(">leg") {
        return Err(format!("{ROW}: the nested unit read by address: {kid:?}"));
    }
    if f.s
        .view_unit(f.ada, "@obj:9")
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err(format!(
            "{ROW}: a unit nobody has, read by address, isn't None"
        ));
    }
    let by_c =
        f.s.view_derived(f.ada, "@grp:1")
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{ROW}: no derived values for @grp:1"))?;
    let keys: Vec<&str> = by_c.iter().map(|(u, _)| u.as_str()).collect();
    if keys
        != [
            "@obj:1",
            "@obj:2",
            "@obj:2/kid:arm",
            "@obj:2/kid:leg",
            "@obj:4",
        ]
    {
        return Err(format!("{ROW}: @grp:1's derived values are of {keys:?}"));
    }
    if f.s
        .view_derived(f.ada, "@grp:3")
        .map_err(|e| e.to_string())?
        != Some(Vec::new())
    {
        return Err(format!(
            "{ROW}: an empty group's derived values aren't empty"
        ));
    }
    if f.s
        .view_derived(f.ada, "@grp:9")
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err(format!("{ROW}: a group nobody has isn't None"));
    }
    let c =
        f.s.view_container(f.ada, "@grp:2")
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{ROW}: no container @grp:2 by address"))?;
    if (c.title.as_deref(), c.unit_count) != (Some("Lights"), 1) {
        return Err(format!("{ROW}: @grp:2 read by address: {c:?}"));
    }
    Ok(())
}

#[test]
fn unit_derived_rides_the_snapshot_and_the_reads() {
    row_unit_derived_rides_the_snapshot_and_the_reads().unwrap();
}

/// Each change frame's `derived.units` carries exactly the values the change set (the text) or
/// removed (`null`), by unit address: a field, a value gone, a new unit's, none for a change
/// that moved no value, every label when the prefix changed.
fn row_a_change_frame_carries_the_units_derived_values() -> Result<(), String> {
    const ROW: &str = "row_a_change_frame_carries_the_units_derived_values";
    let f = fx("frames", Mode::ByChange, &sample());
    let mut w =
        f.s.watch_keyed(f.ada, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
            .map_err(|e| e.to_string())?
            .ok_or("control: no watch")?;
    f.s.watch_snapshot(&mut w).map_err(|e| e.to_string())?;
    let derived = |w: &mut k2_mpc_base::session::KeyedWatch| -> Result<String, String> {
        let ch = changes(&w.try_frames());
        if ch.len() != 1 {
            return Err(format!(
                "{ROW}: control: {} change frames, want 1",
                ch.len()
            ));
        }
        Ok(ch[0].get("derived").map(Json::to_json).unwrap_or_default())
    };
    f.set(OBJ_F, &obj_key(1), "xform", "5,5,5");
    let got = derived(&mut w)?;
    if got != r#"{"units":{"@obj:1":{"transform":"xform 5,5,5"}}}"# {
        return Err(format!(
            "{ROW}: after @obj:1's xform, the frame's derived {got}"
        ));
    }
    f.set(OBJ_F, &obj_key(1), "nolabel", "1");
    let got = derived(&mut w)?;
    if got != r#"{"units":{"@obj:1":{"label":null}}}"# {
        return Err(format!(
            "{ROW}: after @obj:1's label went, the frame's derived {got}"
        ));
    }
    let o5 = obj_key(5);
    f.edit(vec![
        Edit::List {
            root: OBJS.into(),
            ops: vec![ListOp::Insert {
                key: o5.clone(),
                at: Anchor::End,
                value: Value::str("mesh"),
            }],
        },
        set_op(OBJ_F, &o5, "group", "g2"),
        set_op(OBJ_F, &o5, "name", "Lamp"),
        set_op(OBJ_F, &o5, "hidden", "0"),
    ]);
    let got = derived(&mut w)?;
    if got != r#"{"units":{"@obj:5":{"label":">Lamp"}}}"# {
        return Err(format!(
            "{ROW}: after @obj:5's insert, the frame's derived {got}"
        ));
    }
    f.edit(vec![Edit::List {
        root: OBJS.into(),
        ops: vec![ListOp::Delete { key: obj_key(3) }],
    }]);
    let got = derived(&mut w)?;
    if got != "null" {
        return Err(format!(
            "{ROW}: after deleting @obj:3 (no values), the frame's derived {got}"
        ));
    }
    f.edit(vec![Edit::Map {
        root: SCENE.into(),
        ops: vec![MapOp::Set {
            key: "prefix".into(),
            value: Value::str("#"),
        }],
    }]);
    let got = derived(&mut w)?;
    let want = r##"{"units":{"@obj:2":{"label":"#Rig"},"@obj:2/kid:arm":{"label":"#arm"},"@obj:2/kid:leg":{"label":"#leg"},"@obj:4":{"label":"#Long"},"@obj:5":{"label":"#Lamp"}}}"##;
    if got != want {
        return Err(format!(
            "{ROW}: after the prefix, the frame's derived {got}, want {want}"
        ));
    }
    Ok(())
}

#[test]
fn a_change_frame_carries_the_units_derived_values() {
    row_a_change_frame_carries_the_units_derived_values().unwrap();
}

/// A field the pack classes `geometry` with no 2D geometry slot (a scene's transform) changes:
/// the unit's delta is a `move_resize` naming the field, with no box before or after, and the
/// change frame goes out (the kit read the 2D slot's parts for any geometry field, so with no
/// slot it built no frame).
fn row_a_transform_with_no_2d_slot_is_a_move() -> Result<(), String> {
    const ROW: &str = "row_a_transform_with_no_2d_slot_is_a_move";
    let f = fx("transform", Mode::ByChange, &sample());
    let mut w =
        f.s.watch_keyed(f.ada, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
            .map_err(|e| e.to_string())?
            .ok_or("control: no watch")?;
    f.s.watch_snapshot(&mut w).map_err(|e| e.to_string())?;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        f.set(OBJ_F, &obj_key(2), "xform", "9,9,9");
    }));
    if let Err(p) = r {
        let m = p
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default();
        return Err(format!("{ROW}: the change built no frame: {m}"));
    }
    let ch = changes(&w.try_frames());
    let units = ch
        .first()
        .and_then(|c| c.get("units"))
        .map(Json::to_json)
        .unwrap_or_default();
    let want = r#"[{"key":"@obj:2","container":"@grp:1","ops":["move_resize"],"geom_before":null,"geom_after":null,"fields":["xform"],"hidden_after":null,"placeholder_type":null}]"#;
    if units != want {
        return Err(format!("{ROW}: the frame's units {units}, want {want}"));
    }
    Ok(())
}

#[test]
fn a_transform_with_no_2d_slot_is_a_move() {
    row_a_transform_with_no_2d_slot_is_a_move().unwrap();
}

#[test]
fn plant_frame_misses_unit_derived_is_caught() {
    row_a_change_frame_carries_the_units_derived_values().expect("the row passes as built");
    let e = with_view_mutant(
        ViewMutant::FrameMissesUnitDerived,
        row_a_change_frame_carries_the_units_derived_values,
    )
    .expect_err("the row passes with no unit derived values on the frame");
    assert!(
        e.starts_with(
            "row_a_change_frame_carries_the_units_derived_values: after @obj:1's xform, the \
             frame's derived null"
        ),
        "caught, but not where the row says: {e}"
    );
}

/// The schema's names are checked (count, grammar, twice, a grid's cell derived name), and a
/// schema or a unit with none encodes as before.
#[test]
fn the_schema_checks_its_names_and_none_encodes_as_before() {
    let composites: &[(&str, &str)] = &[(GROUP_F, GROUPS), (OBJ_F, OBJS)];
    assert_eq!(VIEW.check(&SCHEMA, composites), Ok(()));
    let bad = |names: &'static [&'static str]| {
        ViewSchema {
            unit_derived: names,
            ..VIEW
        }
        .check(&SCHEMA, composites)
    };
    let nine: &'static [&'static str] = &["a", "b", "c", "d", "e", "f", "g", "h", "i"];
    assert_eq!(nine.len(), UNIT_DERIVED_MAX + 1);
    let e = bad(nine).expect_err("nine names");
    assert!(e.contains("at most 8"), "{e}");
    for n in [&[""][..], &["a b"], &["é"], &["x\u{1f}y"]] {
        let e = bad(n).expect_err("a bad name");
        assert!(e.contains("isn't 1 to 64 bytes"), "{n:?}: {e}");
    }
    let long: &'static str = Box::leak("n".repeat(65).into_boxed_str());
    let names: &'static [&'static str] = Box::leak(vec![long].into_boxed_slice());
    assert!(bad(names).is_err(), "a 65-byte name");
    let e = bad(&["label", "label"]).expect_err("twice");
    assert!(e.contains("declared twice"), "{e}");
    let e = ViewSchema {
        grid: Some(GridSlot {
            cell_derived: &["label"],
            ..GridSlot::NONE
        }),
        ..VIEW
    }
    .check(&SCHEMA, composites)
    .expect_err("a cell derived name");
    assert!(e.contains("also a cell derived value"), "{e}");
    // none declared: as before
    let plain = ViewSchema {
        unit_derived: &[],
        ..VIEW
    };
    assert!(plain.to_value().get("unit_derived").is_none());
    assert!(VIEW.to_value().get("unit_derived").is_some());
    let u = Unit {
        key: "@obj:1".into(),
        ..Unit::default()
    };
    assert!(unit_value(&u).get("derived").is_none());
}

// ----- the rows: the index kept through changes -----

/// `f` with the kit's own test-build checks off on this thread (so the row's own check is the
/// one that speaks, and the counts are a host build's), then back as they were.
fn checks_off<T>(f: impl FnOnce() -> T) -> T {
    struct Back(bool);
    impl Drop for Back {
        fn drop(&mut self) {
            set_view_checks(self.0);
        }
    }
    let _back = Back(set_view_checks(false));
    f()
}

/// A seeded generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn step(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.step() % n.max(1)
    }
}

/// The doc's live and deleted elements of a list root.
fn list_keys(state: &KeyedState, root: &str) -> (Vec<Key>, Vec<Key>) {
    state
        .roots
        .iter()
        .find_map(|(r, x)| match x {
            KeyedRoot::List { live, hidden, .. } if r == root => Some((
                live.iter().map(|(k, _)| k.clone()).collect(),
                hidden.keys().cloned().collect(),
            )),
            _ => None,
        })
        .unwrap_or_default()
}

/// One random change of `f`'s doc, and what it was.
fn random_change(f: &Fx, rng: &mut Rng, next: &mut u32, next_g: &mut u32) -> String {
    let state = f.s.keyed_state_for_tests().unwrap();
    let (objs, gone) = list_keys(&state, OBJS);
    let (groups, gone_g) = list_keys(&state, GROUPS);
    let pick = |rng: &mut Rng, v: &[Key]| v[rng.below(v.len() as u64) as usize].clone();
    let group = |rng: &mut Rng| -> String {
        // now and then a group that isn't there: the object then shows in none
        if groups.is_empty() || rng.below(12) == 0 {
            "g99".to_owned()
        } else {
            pick(rng, &groups).as_str().to_owned()
        }
    };
    let what = rng.below(15);
    let (edits, said): (Vec<Edit>, String) = match what {
        0 | 1 if !objs.is_empty() => {
            let k = pick(rng, &objs);
            let x = format!("{},{},{}", rng.below(9), rng.below(9), rng.below(9));
            (
                vec![set_op(OBJ_F, &k, "xform", &x)],
                format!("xform of {k}"),
            )
        }
        2 if !objs.is_empty() => {
            let k = pick(rng, &objs);
            let n = format!("Name {}", rng.below(100));
            (vec![set_op(OBJ_F, &k, "name", &n)], format!("name of {k}"))
        }
        3 if !objs.is_empty() => {
            let k = pick(rng, &objs);
            let v = ["0", "1"][rng.below(2) as usize];
            (
                vec![set_op(OBJ_F, &k, "nolabel", v)],
                format!("nolabel {v} on {k}"),
            )
        }
        4 if !objs.is_empty() => {
            let k = pick(rng, &objs);
            let kids = ["", "a", "a,b", "c,d,e"][rng.below(4) as usize];
            (
                vec![set_op(OBJ_F, &k, "kids", kids)],
                format!("kids {kids:?} of {k}"),
            )
        }
        5 if !objs.is_empty() => {
            let k = pick(rng, &objs);
            let g = group(rng);
            (vec![set_op(OBJ_F, &k, "group", &g)], format!("{k} to {g}"))
        }
        6 | 7 => {
            *next += 1;
            let k = obj_key(*next);
            let at = match objs.is_empty() || rng.below(3) == 0 {
                true => Anchor::End,
                false => Anchor::After(pick(rng, &objs)),
            };
            let g = group(rng);
            let mut e = vec![Edit::List {
                root: OBJS.into(),
                ops: vec![ListOp::Insert {
                    key: k.clone(),
                    at,
                    value: Value::str("mesh"),
                }],
            }];
            e.push(set_op(OBJ_F, &k, "group", &g));
            e.push(set_op(OBJ_F, &k, "name", &format!("New {next}")));
            if rng.below(2) == 0 {
                e.push(set_op(OBJ_F, &k, "kids", "x,y"));
            }
            (e, format!("insert {k} in {g}"))
        }
        8 if !objs.is_empty() => {
            let k = pick(rng, &objs);
            (
                vec![Edit::List {
                    root: OBJS.into(),
                    ops: vec![ListOp::Delete { key: k.clone() }],
                }],
                format!("delete {k}"),
            )
        }
        9 if !gone.is_empty() => {
            let k = pick(rng, &gone);
            (
                vec![Edit::List {
                    root: OBJS.into(),
                    ops: vec![ListOp::Restore { key: k.clone() }],
                }],
                format!("restore {k}"),
            )
        }
        10 if objs.len() > 1 => {
            let k = pick(rng, &objs);
            let to = match rng.below(2) {
                0 => Anchor::Start,
                _ => Anchor::After(pick(rng, &objs)),
            };
            if matches!(&to, Anchor::After(a) if *a == k) {
                (vec![set_op(OBJ_F, &k, "hidden", "1")], format!("hide {k}"))
            } else {
                (
                    vec![Edit::List {
                        root: OBJS.into(),
                        ops: vec![ListOp::Move { key: k.clone(), to }],
                    }],
                    format!("move {k}"),
                )
            }
        }
        11 if !groups.is_empty() => {
            let g = pick(rng, &groups);
            match rng.below(2) {
                0 => (
                    vec![set_op(GROUP_F, &g, "name", &format!("G {}", rng.below(50)))],
                    format!("rename {g}"),
                ),
                _ => (
                    vec![set_op(GROUP_F, &g, "hidden", "1")],
                    format!("hide {g}"),
                ),
            }
        }
        12 => match (rng.below(3), groups.is_empty(), gone_g.is_empty()) {
            (0, false, _) => {
                let g = pick(rng, &groups);
                (
                    vec![Edit::List {
                        root: GROUPS.into(),
                        ops: vec![ListOp::Delete { key: g.clone() }],
                    }],
                    format!("delete group {g}"),
                )
            }
            (1, _, false) => {
                let g = pick(rng, &gone_g);
                (
                    vec![Edit::List {
                        root: GROUPS.into(),
                        ops: vec![ListOp::Restore { key: g.clone() }],
                    }],
                    format!("restore group {g}"),
                )
            }
            _ => {
                *next_g += 1;
                let g = group_key(*next_g);
                (
                    vec![
                        Edit::List {
                            root: GROUPS.into(),
                            ops: vec![ListOp::Insert {
                                key: g.clone(),
                                at: Anchor::Start,
                                value: Value::str("group"),
                            }],
                        },
                        set_op(GROUP_F, &g, "name", "Added"),
                    ],
                    format!("insert group {g}"),
                )
            }
        },
        13 => (
            vec![Edit::Map {
                root: SCENE.into(),
                ops: vec![MapOp::Set {
                    key: ["prefix", "w"][rng.below(2) as usize].into(),
                    value: Value::str(format!("{}", rng.below(40))),
                }],
            }],
            "the scene map".to_owned(),
        ),
        _ if objs.len() > 1 => {
            // two units in one change: a field of one, the group of another
            let a = pick(rng, &objs);
            let b = pick(rng, &objs);
            let g = group(rng);
            (
                vec![
                    set_op(OBJ_F, &a, "xform", "7,7,7"),
                    set_op(OBJ_F, &b, "group", &g),
                ],
                format!("xform of {a} and {b} to {g}"),
            )
        }
        _ => {
            *next += 1;
            let k = obj_key(*next);
            (
                vec![
                    Edit::List {
                        root: OBJS.into(),
                        ops: vec![ListOp::Insert {
                            key: k.clone(),
                            at: Anchor::Start,
                            value: Value::str("empty"),
                        }],
                    },
                    set_op(OBJ_F, &k, "group", &group(rng)),
                ],
                format!("insert {k} first"),
            )
        }
    };
    f.edit(edits);
    said
}

/// The first place two indexes differ, in words.
fn first_difference(got: &ViewIndex, want: &ViewIndex) -> String {
    if got.units.len() != want.units.len() {
        let g: Vec<&str> = got.units.iter().map(|u| u.key.as_str()).collect();
        let w: Vec<&str> = want.units.iter().map(|u| u.key.as_str()).collect();
        return format!("units kept {g:?}, whole {w:?}");
    }
    for (a, b) in got.units.iter().zip(&want.units) {
        if a != b {
            return format!("unit {}: kept {a:?}, whole {b:?}", a.key);
        }
    }
    for (a, b) in got.containers.iter().zip(&want.containers) {
        if a != b {
            return format!("container {}: kept {a:?}, whole {b:?}", a.key);
        }
    }
    format!(
        "containers kept {:?}, whole {:?}; page kept {:?}, whole {:?}",
        got.containers.len(),
        want.containers.len(),
        got.page,
        want.page
    )
}

/// After every one of a seeded run of changes of every kind (fields, children, a group move,
/// inserts, deletes, restores, moves, groups added, deleted and renamed, the scene map, two
/// units in one change), read now after one change, now after several, the index the kit
/// kept is the one a whole build gives. The kit's own check is off here, so the row's is the
/// one that speaks.
fn row_the_kept_index_is_a_whole_build() -> Result<(), String> {
    const ROW: &str = "row_the_kept_index_is_a_whole_build";
    checks_off(|| {
        for seed in [1u64, 2, 3] {
            let f = fx("kept", Mode::ByChange, &big(3, 12));
            let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ seed);
            let (mut next, mut next_g) = (36u32, 3u32);
            f.index()?;
            // units the kit built for the reads, and what whole builds would have built
            let (mut built, mut whole) = (0u64, 0u64);
            for n in 1..=150 {
                let what = random_change(&f, &mut rng, &mut next, &mut next_g);
                if rng.below(3) == 0 && n < 150 {
                    continue;
                }
                take_view_unit_builds();
                let got = f.index()?;
                built += take_view_unit_builds();
                let want = f.whole();
                whole += take_view_unit_builds();
                if got != want {
                    return Err(format!(
                        "{ROW}: seed {seed}, after change {n} ({what}): {}",
                        first_difference(&got, &want)
                    ));
                }
            }
            if built * 4 > whole {
                return Err(format!(
                    "{ROW}: seed {seed}: the reads built {built} units, whole builds {whole}: \
                     the index wasn't kept through the changes"
                ));
            }
        }
        Ok(())
    })
}

#[test]
fn the_kept_index_is_a_whole_build() {
    row_the_kept_index_is_a_whole_build().unwrap();
}

#[test]
fn plant_update_skips_touched_is_caught() {
    row_the_kept_index_is_a_whole_build().expect("the row passes as built");
    let e = with_view_mutant(
        ViewMutant::UpdateSkipsTouched,
        row_the_kept_index_is_a_whole_build,
    )
    .expect_err("the row passes with a touched unit kept as it was");
    assert!(
        e.starts_with("row_the_kept_index_is_a_whole_build: seed 1, after change "),
        "caught, but not where the row says: {e}"
    );
}

/// The counts of keeping the index of a 2,500-unit doc (10 groups of 250) through a change:
/// a whole build builds and derives every unit; a change to one unit (a field, an insert, a
/// move to another group) builds and derives that one; renaming a group builds none; the
/// scene's prefix (a root the pack's labels read) builds none and the pack derives every unit
/// again, its own choice. Prints the counts and, from the same job, the times.
fn row_one_unit_changed_builds_one_unit() -> Result<(), String> {
    const ROW: &str = "row_one_unit_changed_builds_one_unit";
    const N: u64 = 2_500;
    checks_off(|| {
        let f = fx("counts", Mode::ByChange, &big(10, 250));
        take_view_unit_builds();
        take_derived();
        let t = std::time::Instant::now();
        f.index()?;
        let first_t = t.elapsed();
        let first = (take_view_unit_builds(), take_derived());
        if first != (N, N) {
            return Err(format!(
                "{ROW}: control: a whole build built and derived {first:?}, want ({N}, {N})"
            ));
        }
        f.set(OBJ_F, &obj_key(1234), "xform", "1,2,3");
        let t = std::time::Instant::now();
        f.index()?;
        let one_t = t.elapsed();
        let one = (take_view_unit_builds(), take_derived());
        if one != (1, 1) {
            return Err(format!(
                "{ROW}: one unit's field: built and derived {one:?}, want (1, 1)"
            ));
        }
        let k = obj_key(9001);
        f.edit(vec![
            Edit::List {
                root: OBJS.into(),
                ops: vec![ListOp::Insert {
                    key: k.clone(),
                    at: Anchor::After(obj_key(10)),
                    value: Value::str("mesh"),
                }],
            },
            set_op(OBJ_F, &k, "group", "g1"),
            set_op(OBJ_F, &k, "name", "Added"),
        ]);
        f.index()?;
        let insert = (take_view_unit_builds(), take_derived());
        f.set(OBJ_F, &obj_key(20), "group", "g7");
        f.index()?;
        let moved = (take_view_unit_builds(), take_derived());
        f.set(GROUP_F, &group_key(3), "name", "Renamed");
        f.index()?;
        let renamed = (take_view_unit_builds(), take_derived());
        f.edit(vec![Edit::Map {
            root: SCENE.into(),
            ops: vec![MapOp::Set {
                key: "prefix".into(),
                value: Value::str("#"),
            }],
        }]);
        f.index()?;
        let prefix = (take_view_unit_builds(), take_derived());
        println!(
            "view index counts ({N} units): whole build {} built, {} derived; one unit's field \
             {} built, {} derived; an insert {} built, {} derived; a move to another group {} \
             built, {} derived; a group renamed {} built, {} derived; the label prefix {} built, \
             {} derived",
            first.0,
            first.1,
            one.0,
            one.1,
            insert.0,
            insert.1,
            moved.0,
            moved.1,
            renamed.0,
            renamed.1,
            prefix.0,
            prefix.1
        );
        println!(
            "view index time (same job, debug build): whole build {} us, one unit's field {} us",
            first_t.as_micros(),
            one_t.as_micros()
        );
        let want = [(1, 1), (1, 1), (0, 0), (0, N + 1)];
        let got = [insert, moved, renamed, prefix];
        if got != want {
            return Err(format!(
                "{ROW}: insert, move, rename, prefix built and derived {got:?}, want {want:?}"
            ));
        }
        let got = f.index()?;
        if got != f.whole() {
            return Err(format!(
                "{ROW}: control: the kept index isn't a whole build: {}",
                first_difference(&got, &f.whole())
            ));
        }
        Ok(())
    })
}

#[test]
fn one_unit_changed_builds_one_unit() {
    row_one_unit_changed_builds_one_unit().unwrap();
}

#[test]
fn plant_update_rebuilds_all_is_caught() {
    row_one_unit_changed_builds_one_unit().expect("the row passes as built");
    let e = with_view_mutant(
        ViewMutant::UpdateRebuildsAll,
        row_one_unit_changed_builds_one_unit,
    )
    .expect_err("the row passes with every unit built again");
    assert!(
        e.starts_with(
            "row_one_unit_changed_builds_one_unit: one unit's field: built and derived (2500, 2500)"
        ),
        "caught, but not where the row says: {e}"
    );
}

/// A pack that doesn't derive by change is asked once a session, and its index is built whole
/// on every change after that: right, at a whole build's cost.
#[test]
fn a_pack_that_doesnt_derive_by_change_is_asked_once() {
    let (built, asked) = checks_off(|| {
        let f = fx("whole", Mode::Whole, &big(2, 10));
        f.index().unwrap();
        take_view_unit_builds();
        take_asked();
        let mut built = 0;
        for x in ["1,1,1", "2,2,2", "3,3,3"] {
            f.set(OBJ_F, &obj_key(3), "xform", x);
            let got = f.index().unwrap();
            built += take_view_unit_builds();
            assert_eq!(got, f.whole(), "built whole after {x}");
            take_view_unit_builds();
        }
        (built, take_asked())
    });
    assert_eq!(
        (built, asked),
        (61, 1),
        "three changes of a 20-unit doc: each built whole (20 each, and the one unit the first change touched, built before the pack said no), the pack asked once"
    );
}

/// In a test build, a pack whose derive by change leaves a touched unit without what a whole
/// build derives is refused loudly, naming the unit.
#[test]
fn a_wrong_derive_by_change_is_refused_loudly() {
    assert!(k2_mpc_base::differential::view_checks(), "checks on");
    let f = fx("wrong", Mode::Wrong, &sample());
    f.index().unwrap();
    f.set(OBJ_F, &obj_key(1), "xform", "4,4,4");
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.s.view_index(f.ada)));
    let e = match r {
        Ok(ok) => panic!(
            "a wrong derive by change went through: {:?}",
            ok.map(|_| ())
        ),
        Err(p) => p
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default(),
    };
    assert!(
        e.contains("the view index kept through a change at rev")
            && e.contains("isn't a whole build's")
            && e.contains("unit @obj:1"),
        "refused, but not as the kit says: {e}"
    );
}

// ----- the client's golden -----

/// The frames a watcher of the sample gets (the snapshot, then a transform, a label gone, an
/// insert, a delete, the prefix and a restore), with the doc's state and every unit's derived
/// values at the end, for the JS client (`client/test/golden/keyed_frames_unit_derived.json`;
/// `MPC_BLESS=1` rewrites it). The change frames' `client` (a random id per connection) is
/// written as 1.
#[test]
fn golden_frames_with_unit_derived() {
    let f = fx("golden", Mode::ByChange, &sample());
    let mut w =
        f.s.watch_keyed(f.ada, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
            .unwrap()
            .unwrap();
    let mut frames = f.s.watch_snapshot(&mut w).unwrap();
    f.set(OBJ_F, &obj_key(1), "xform", "5,5,5");
    f.set(OBJ_F, &obj_key(1), "nolabel", "1");
    let o5 = obj_key(5);
    f.edit(vec![
        Edit::List {
            root: OBJS.into(),
            ops: vec![ListOp::Insert {
                key: o5.clone(),
                at: Anchor::End,
                value: Value::str("mesh"),
            }],
        },
        set_op(OBJ_F, &o5, "group", "g2"),
        set_op(OBJ_F, &o5, "name", "Lamp"),
    ]);
    f.edit(vec![Edit::List {
        root: OBJS.into(),
        ops: vec![ListOp::Delete { key: obj_key(4) }],
    }]);
    f.edit(vec![Edit::Map {
        root: SCENE.into(),
        ops: vec![MapOp::Set {
            key: "prefix".into(),
            value: Value::str("#"),
        }],
    }]);
    f.edit(vec![Edit::List {
        root: OBJS.into(),
        ops: vec![ListOp::Restore { key: obj_key(4) }],
    }]);
    frames.extend(w.try_frames());
    assert!(!w.needs_snapshot());
    let frames: Vec<Json> = frames
        .iter()
        .map(|fr| match fr.to_value() {
            Json::Obj(m) if matches!(fr, WatchFrame::Change(_)) => Json::Obj(
                m.into_iter()
                    .map(|(k, v)| match k.as_str() {
                        "client" => (k, Json::from(1u64)),
                        _ => (k, v),
                    })
                    .collect(),
            ),
            v => v,
        })
        .collect();
    let ix = f.index().unwrap();
    let derived = Json::obj(ix.units.iter().filter(|u| !u.derived.is_empty()).map(|u| {
        (
            u.key.as_str(),
            Json::obj(u.derived.iter().map(|(n, t)| (n.as_str(), Json::from(t)))),
        )
    }));
    let state = f.s.keyed_state_for_tests().unwrap();
    let final_json = watch_state_json(&ScenePack(Mode::ByChange), &state);
    let mut got = Json::obj([
        (
            "features",
            Json::List(vec![Json::from(KEYED_WATCH_FEATURE)]),
        ),
        ("frames", Json::List(frames)),
        ("final_json", Json::from(final_json)),
        ("final_rev", Json::from(f.version())),
        ("final_derived", derived),
    ])
    .to_json_pretty();
    got.push('\n');
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../client/test/golden/keyed_frames_unit_derived.json");
    if std::env::var_os("MPC_BLESS").is_some() {
        std::fs::write(&path, &got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing (MPC_BLESS=1 writes it)", path.display()));
    assert!(
        want == got,
        "{} differs from what the kit encodes now (MPC_BLESS=1 regenerates it)\n--- now:\n{got}",
        path.display()
    );
}
