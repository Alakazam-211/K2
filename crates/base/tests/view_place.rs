//! The view index's 3D slot (`docs/design/view-3d-slot-r1.1.md`) on a scene-shaped toy pack
//! kept in a file: groups (containers, each with a location of its own) holding objects
//! (units) placed in each of the slot's forms, relative to a parent, a group or the doc's space,
//! and frames the store never holds that the carrier alone carries (an object's anonymous
//! frame). Public API only.
//!
//! The rows: the shared vectors (`client/test/vectors/placement.json`) are the Rust helper's,
//! with every form, order and degenerate case, and chains; a placement round-trips the wire bit
//! for bit at each precision; an `F32` slot refuses a number that isn't an `f32`; a field change
//! carries `place` and nothing for the children, whose containers the render hint names; a
//! change to another field asks the pack nothing; a unit that comes into view carries its
//! placement; a door through an opening through a wall has a world matrix; a broken chain has
//! none; a placement relative to nothing is in the doc's space; a change held only in the
//! carrier makes the watch take a snapshot, and one an outside save makes through fields
//! doesn't; the doc's space rides the snapshot and changes; the client's golden.
//!
//! Planted faults (`k2_mpc_base::differential::PlaceMutant`) are caught by name.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::differential::{
    set_view_checks, take_placement_asks, with_place_mutant, PlaceMutant,
};
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, KeyedRoot, KeyedState, ListOp, MapOp};
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::placement::{
    mul, world_matrix, Axis, EulerOrder, Handed, Link, Mat4, Placement, Precision, Rotation, Space,
    SpaceSlot, SpaceSrc, Step,
};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    watch_state_json, AuditEvent, Config, ConnId, Declined, EditOptions, Host, KeyedWatch, Pack,
    PackContent, PackError, Participant, ParticipantKind, Reconciled, Rights, Session,
    SessionManager, Stay, View, WatchFrame, WatchOptions, KEYED_WATCH_FEATURE,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::view_index::{
    ContainerFields, FieldClass, Src, UnitFields, ViewIndex, ViewSchema, ViewSource, ViewTouched,
};
use k2_mpc_base::{Key, Value};

const GROUPS: &str = "groups";
const GROUP_F: &str = "group_f";
const OBJS: &str = "objs";
const OBJ_F: &str = "obj_f";
const SCENE: &str = "scene";

const SCHEMA: Schema = Schema {
    name: "scene3d",
    roots: &[
        (GROUPS, Kind::MovableList),
        (GROUP_F, Kind::Map),
        (OBJS, Kind::MovableList),
        (OBJ_F, Kind::Map),
        (SCENE, Kind::Map),
    ],
};

const PLACE_FIELDS: &[&str] = &["xform", "rot", "quat", "axes", "matrix", "parent", "driven"];

const fn view(precision: Precision) -> ViewSchema {
    ViewSchema {
        containers: GROUPS,
        units: OBJS,
        unit_container_field: "group",
        native_unit: "scene-unit",
        container_fields: ContainerFields {
            title: Src::Field("name"),
            hidden: Src::None,
            layout: Src::None,
            parent: Src::None,
        },
        unit_fields: UnitFields {
            kind: Src::ListValue,
            name: Src::Field("name"),
            ..UnitFields::NONE
        },
        field_class: &[
            ("xform", FieldClass::Geometry),
            ("matrix", FieldClass::Geometry),
            ("axes", FieldClass::Geometry),
            ("name", FieldClass::Text),
        ],
        space: Some(SpaceSlot {
            unit_fields: PLACE_FIELDS,
            container_fields: &["place"],
            space: SpaceSrc::Pack { roots: &[SCENE] },
            precision,
        }),
        ..ViewSchema::NONE
    }
}

// ----- the file and the pack -----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    F64,
    F32,
}

struct Scene3(Mode);

fn gkey(n: u32) -> Key {
    Key::new(format!("g{n}")).unwrap()
}

fn okey(n: u32) -> Key {
    Key::new(format!("o{n}")).unwrap()
}

fn prop(k: &Key, f: &str) -> String {
    format!("{k}\u{1f}{f}")
}

fn canon(s: &str) -> Option<u32> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    s.parse().ok()
}

/// The anonymous frames a file holds: `F|<object>|<16 numbers>`, by object number.
fn frames(bytes: &[u8]) -> HashMap<u32, String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('|').collect();
            match f[..] {
                ["F", n, m] => Some((canon(n)?, m.to_owned())),
                _ => None,
            }
        })
        .collect()
}

/// The file: `SCENE3|unit|metres per unit|up`, `G|n|name|place` (`-`: none),
/// `O|group|n|kind|name|k=v;k=v` and `F` lines (the carrier's alone).
fn decode(bytes: &[u8]) -> Result<PackContent, Declined> {
    let bad = |m: String| Declined {
        code: "not_scene3",
        message: m,
    };
    let text = std::str::from_utf8(bytes).map_err(|_| bad("not utf-8".into()))?;
    let mut lines = text.lines();
    let head: Vec<&str> = lines.next().unwrap_or("").split('|').collect();
    let ["SCENE3", unit, mpu, up] = head[..] else {
        return Err(bad("no SCENE3 header".into()));
    };
    let mut scene = BTreeMap::new();
    scene.insert("unit".to_owned(), Value::str(unit));
    scene.insert("mpu".to_owned(), Value::str(mpu));
    scene.insert("up".to_owned(), Value::str(up));
    let (mut groups, mut objs, mut gf, mut of) =
        (Vec::new(), Vec::new(), BTreeMap::new(), BTreeMap::new());
    let num = |s: &str| canon(s).ok_or_else(|| bad(format!("bad number {s:?}")));
    for line in lines {
        let f: Vec<&str> = line.split('|').collect();
        match f[..] {
            ["G", n, name, place] => {
                let k = gkey(num(n)?);
                gf.insert(prop(&k, "name"), Value::str(name));
                if place != "-" {
                    gf.insert(prop(&k, "place"), Value::str(place));
                }
                groups.push((k, Value::str("group")));
            }
            ["O", g, n, kind, name, fields] => {
                let k = okey(num(n)?);
                of.insert(prop(&k, "group"), Value::str(gkey(num(g)?).as_str()));
                of.insert(prop(&k, "name"), Value::str(name));
                for kv in fields.split(';').filter(|s| !s.is_empty()) {
                    let (fk, fv) = kv.split_once('=').ok_or_else(|| bad(kv.into()))?;
                    of.insert(prop(&k, fk), Value::str(fv));
                }
                objs.push((k, Value::str(kind)));
            }
            ["F", ..] => {}
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

/// The content's lines (no `F` line).
fn render(content: &[(String, Neutral)]) -> Result<String, PackError> {
    let bad = |m: &str| PackError(format!("the scene content: {m}"));
    let root = |n: &str| content.iter().find(|(r, _)| r == n).map(|(_, v)| v);
    let map = |n: &str| match root(n) {
        Some(Neutral::Map(m)) => Ok(m.clone()),
        _ => Err(bad(n)),
    };
    let list = |n: &str| match root(n) {
        Some(Neutral::List(l)) => Ok(l.clone()),
        _ => Err(bad(n)),
    };
    let (gf, of, scene) = (map(GROUP_F)?, map(OBJ_F)?, map(SCENE)?);
    let get = |m: &BTreeMap<String, Value>, k: &str| {
        m.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    let mut out = format!(
        "SCENE3|{}|{}|{}\n",
        get(&scene, "unit"),
        get(&scene, "mpu"),
        get(&scene, "up")
    );
    for (k, _) in list(GROUPS)? {
        let place = get(&gf, &prop(&k, "place"));
        out.push_str(&format!(
            "G|{}|{}|{}\n",
            &k.as_str()[1..],
            get(&gf, &prop(&k, "name")),
            if place.is_empty() { "-" } else { &place }
        ));
    }
    for (k, v) in list(OBJS)? {
        let pre = format!("{k}\u{1f}");
        let fields: Vec<String> = of
            .iter()
            .filter_map(|(mk, mv)| {
                let f = mk.strip_prefix(&pre)?;
                (f != "group" && f != "name").then(|| format!("{f}={}", mv.as_str().unwrap_or("")))
            })
            .collect();
        let g = get(&of, &prop(&k, "group"));
        out.push_str(&format!(
            "O|{}|{}|{}|{}|{}\n",
            g.strip_prefix('g').unwrap_or(&g),
            &k.as_str()[1..],
            v.as_str().unwrap_or_default(),
            get(&of, &prop(&k, "name")),
            fields.join(";")
        ));
    }
    Ok(out)
}

/// The units `new` appends to `old`, when it is `old` and then those (one at least).
fn appended<'a>(old: &[(Key, Value)], new: &'a [(Key, Value)]) -> Option<&'a [(Key, Value)]> {
    (new.len() > old.len() && new[..old.len()] == *old).then(|| &new[old.len()..])
}

fn map_root<'a>(state: &'a KeyedState, name: &str) -> Option<&'a BTreeMap<String, Value>> {
    state.roots.iter().find_map(|(r, root)| match root {
        KeyedRoot::Map(m) if r == name => Some(m),
        _ => None,
    })
}

fn floats<const N: usize>(s: &str, f32s: bool) -> Option<[f64; N]> {
    let v: Vec<f64> = s
        .split(',')
        .map(|x| x.trim().parse::<f64>().ok())
        .collect::<Option<_>>()?;
    let v: Vec<f64> = match f32s {
        true => v.into_iter().map(|x| x as f32 as f64).collect(),
        false => v,
    };
    v.try_into().ok()
}

fn dir(s: &str, f32s: bool) -> Option<Option<[f64; 3]>> {
    match s {
        "-" => Some(None),
        _ => floats(s, f32s).map(Some),
    }
}

impl Pack for Scene3 {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[(GROUP_F, GROUPS), (OBJ_F, OBJS)]
    }
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        decode(bytes)
    }
    fn encode(&self, content: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(render(content)?.into_bytes())
    }
    fn wants_carrier(&self) -> bool {
        true
    }
    /// An outside save by key: its maps' entries set or removed, and units appended at a list's
    /// end inserted there; a save that changes a list otherwise is declined.
    fn reconcile(&self, synced: &View, bytes: &[u8], _: &[u8]) -> Result<Reconciled, Declined> {
        let content = decode(bytes)?;
        let empty = BTreeMap::new();
        // a list's units appended at its end come first; any other list change is refused
        let mut edits = Vec::new();
        for (root, n) in &content.roots {
            if let Neutral::List(new) = n {
                if let Some(tail) = appended(synced.list(root).unwrap_or(&[]), new) {
                    edits.push(Edit::List {
                        root: root.clone(),
                        ops: tail
                            .iter()
                            .map(|(k, v)| ListOp::Insert {
                                key: k.clone(),
                                at: Anchor::End,
                                value: v.clone(),
                            })
                            .collect(),
                    });
                }
            }
        }
        for (root, n) in &content.roots {
            match n {
                Neutral::Map(new) => {
                    let old = synced.map(root).unwrap_or(&empty);
                    let mut ops: Vec<MapOp> = new
                        .iter()
                        .filter(|(k, v)| old.get(*k) != Some(*v))
                        .map(|(k, v)| MapOp::Set {
                            key: k.clone(),
                            value: v.clone(),
                        })
                        .collect();
                    ops.extend(
                        old.keys()
                            .filter(|k| !new.contains_key(*k))
                            .map(|k| MapOp::Delete { key: k.clone() }),
                    );
                    if !ops.is_empty() {
                        edits.push(Edit::Map {
                            root: root.clone(),
                            ops,
                        });
                    }
                }
                Neutral::List(new)
                    if synced.list(root).unwrap_or(&[]) != new.as_slice()
                        && appended(synced.list(root).unwrap_or(&[]), new).is_none() =>
                {
                    return Err(Declined {
                        code: "not_here",
                        message: format!("the save changes the list {root}"),
                    });
                }
                _ => {}
            }
        }
        Ok(Reconciled {
            edits,
            ..Reconciled::default()
        })
    }
    fn encode_from(
        &self,
        carrier: &[u8],
        content: &[(String, Neutral)],
        _: &[u8],
    ) -> Result<Vec<u8>, PackError> {
        let held = decode(carrier).map_err(|d| PackError(d.message))?.roots;
        if held.len() == content.len()
            && held
                .iter()
                .all(|(r, n)| content.iter().any(|(r2, n2)| r == r2 && n == n2))
        {
            return Ok(carrier.to_vec());
        }
        let mut out = render(content)?;
        let mut fs: Vec<(u32, String)> = frames(carrier).into_iter().collect();
        fs.sort();
        for (n, m) in fs {
            out.push_str(&format!("F|{n}|{m}\n"));
        }
        Ok(out.into_bytes())
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        Some(match self.0 {
            Mode::F64 => view(Precision::F64),
            Mode::F32 => view(Precision::F32),
        })
    }
    fn view_derive_change(&self, _: ViewSource<'_>, _: &mut ViewIndex, _: &ViewTouched) -> bool {
        true
    }
    fn view_placement(&self, source: ViewSource<'_>, root: &str, key: &Key) -> Option<Placement> {
        let ViewSource::Live(state, carrier) = source else {
            return None;
        };
        let f32s = self.0 == Mode::F32;
        if root == GROUPS {
            let m = map_root(state, GROUP_F)?;
            let loc = floats(m.get(&prop(key, "place"))?.as_str()?, f32s)?;
            return Some(Placement {
                steps: vec![Step::Trs {
                    loc,
                    rot: Rotation::Euler {
                        order: EulerOrder::Xyz,
                        angles: [0.0; 3],
                    },
                    scale: [1.0; 3],
                }],
                ..Placement::default()
            });
        }
        let m = map_root(state, OBJ_F)?;
        let f = |n: &str| m.get(&prop(key, n)).and_then(|v| v.as_str());
        let raw = f("raw") == Some("1");
        let f32s = f32s && !raw;
        let mut steps = Vec::new();
        // the anonymous frame the carrier alone holds
        if let Some(c) = carrier {
            let fr = c.decoded(|b| Some(frames(b)));
            let n = canon(&key.as_str()[1..]);
            if let Some(mt) = fr.and_then(|fr| fr.get(&n?)) {
                steps.push(Step::Matrix(floats(mt, f32s)?));
            }
        }
        if let Some(mt) = f("matrix") {
            steps.push(Step::Matrix(floats(mt, f32s)?));
        } else if let Some(ax) = f("axes") {
            let p: Vec<&str> = ax.split('/').collect();
            let [o, z, x] = p[..] else { return None };
            steps.push(Step::Axes {
                origin: floats(o, f32s)?,
                z: dir(z, f32s)?,
                x: dir(x, f32s)?,
            });
        } else if let Some(xf) = f("xform") {
            let v: [f64; 9] = floats(xf, f32s)?;
            let rot = match f("rot").unwrap_or("XYZ") {
                "quat" => Rotation::Quat(floats(f("quat")?, f32s)?),
                w => Rotation::Euler {
                    order: EulerOrder::parse(w)?,
                    angles: [v[3], v[4], v[5]],
                },
            };
            steps.push(Step::Trs {
                loc: [v[0], v[1], v[2]],
                rot,
                scale: [v[6], v[7], v[8]],
            });
        }
        if steps.is_empty() {
            return None;
        }
        Some(Placement {
            relative_to: f("parent").map(str::to_owned),
            steps,
            approx: (f("driven") == Some("1")).then(|| "driven".to_owned()),
            ..Placement::default()
        })
    }
    fn view_space(&self, source: ViewSource<'_>) -> Option<Space> {
        let ViewSource::Live(state, _) = source else {
            return None;
        };
        let m = map_root(state, SCENE)?;
        let g = |k: &str| m.get(k).and_then(|v| v.as_str());
        Some(Space {
            length_unit: g("unit")?.to_owned(),
            metres_per_unit: g("mpu").and_then(|s| s.parse().ok()),
            up: if g("up") == Some("y") {
                Axis::Y
            } else {
                Axis::Z
            },
            handed: Handed::Right,
        })
    }
    fn address_of(&self, root: &str, key: &Key) -> String {
        match root {
            GROUPS => format!("@g:{}", &key.as_str()[1..]),
            OBJS => format!("@o:{}", &key.as_str()[1..]),
            _ => key.to_string(),
        }
    }
    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        match root {
            GROUPS => Some(gkey(canon(address.strip_prefix("@g:")?)?)),
            OBJS => Some(okey(canon(address.strip_prefix("@o:")?)?)),
            _ => None,
        }
    }
}

// ----- the fixture -----

struct TestHost;

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        100_000
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct Fx {
    dir: std::path::PathBuf,
    path: std::path::PathBuf,
    _mgr: SessionManager,
    s: Arc<Session>,
    ada: ConnId,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str, mode: Mode, text: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-place-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let ws = std::fs::canonicalize(ws).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, Arc::new(TestHost), c);
    let path = ws.join("s.scene3");
    std::fs::write(&path, text).unwrap();
    let key = DocKey {
        space: "ws".into(),
        path: "s.scene3".into(),
    };
    let s = mgr
        .open_file(Arc::new(Scene3(mode)), &key, &path, &ws)
        .unwrap();
    let ada = s
        .join(
            Participant::new("ada", "Ada", "#2f9e8f", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    Fx {
        dir,
        path,
        _mgr: mgr,
        s,
        ada,
    }
}

impl Fx {
    fn version(&self) -> u64 {
        self.s.status().unwrap().version
    }
    fn edit(&self, edits: Vec<Edit>) {
        let mut opts = EditOptions::new();
        opts.base = Some(self.version());
        self.s.edit_ops(self.ada, edits, &opts).unwrap();
    }
    fn set(&self, n: u32, field: &str, value: &str) {
        self.edit(vec![set_op(OBJ_F, &okey(n), field, value)]);
    }
    fn index(&self) -> ViewIndex {
        self.s.view_index(self.ada).unwrap().unwrap()
    }
    fn watch(&self) -> (KeyedWatch, Vec<WatchFrame>) {
        let mut w = self
            .s
            .watch_keyed(self.ada, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
            .unwrap()
            .unwrap();
        let frames = self.s.watch_snapshot(&mut w).unwrap();
        (w, frames)
    }
    /// The file now with `f` applied to its text, as an app's save, taken in.
    fn outside_save(&self, f: impl Fn(&str) -> String) -> String {
        let now = std::fs::read_to_string(&self.path).unwrap();
        std::fs::write(&self.path, f(&now)).unwrap();
        format!("{:?}", self.s.file_changed().unwrap())
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

/// The change frames among `frames`, as JSON.
fn changes(frames: &[WatchFrame]) -> Vec<Json> {
    frames
        .iter()
        .filter(|f| matches!(f, WatchFrame::Change(_)))
        .map(WatchFrame::to_value)
        .collect()
}

/// The unit delta of `key` in a change frame, as JSON.
fn delta<'a>(frame: &'a Json, key: &str) -> Option<&'a Json> {
    let Some(Json::List(us)) = frame.get("units") else {
        return None;
    };
    us.iter()
        .find(|u| u.get("key").and_then(Json::as_str) == Some(key))
}

/// The sample: group 1 at the origin, group 2 at (100, 0, 0); a parent (o2) in group 1 with a
/// child (o3) in group 2; a wall (o10), its opening (o11) and the opening's door (o12); a unit
/// relative to nothing in group 2 (o30); broken chains (o20 through an unplaced o21; o22
/// through nobody); every form; an anonymous frame on o2.
const SAMPLE: &str = "SCENE3|mm|0.001|z
G|1|Ground|0,0,0
G|2|Upper|100,0,0
O|1|1|mesh|Cube|xform=1,2,3,0,0,0,1,1,1
O|1|2|empty|Rig|xform=0,0,1,0.1,0.2,0.3,1,1,1;rot=ZYX
O|2|3|mesh|Arm|xform=5,0,0,0,0,0,2,2,2;parent=@o:2
O|1|4|mesh|Quat|xform=0,1,0,0,0,0,1,1,1;rot=quat;quat=0.9238795,0,0,0.3826834
O|1|10|wall|Wall|axes=4000,0,0/-/0,1,0;parent=@g:1
O|1|11|opening|Opening|axes=500,0,0/-/-;parent=@o:10
O|1|12|door|Door|matrix=1,0,0,0,0,1,0,0,0,0,1,0,0,0,10,1;parent=@o:11
O|1|20|mesh|Broken|xform=1,1,1,0,0,0,1,1,1;parent=@o:21
O|1|21|mesh|Unplaced|
O|1|22|mesh|Orphan|xform=1,1,1,0,0,0,1,1,1;parent=@o:99
O|2|30|mesh|Free|xform=1,2,3,0,0,0,1,1,1
F|2|1,0,0,0,0,1,0,0,0,0,1,0,7,0,0,1
";

// ----- a JSON reader for the vectors (the kit has a writer only) -----

fn parse_json(s: &str) -> Json {
    let b = s.as_bytes();
    let mut i = 0;
    let v = value(b, &mut i);
    skip(b, &mut i);
    assert_eq!(i, b.len(), "trailing bytes");
    v
}

fn skip(b: &[u8], i: &mut usize) {
    while *i < b.len() && b[*i].is_ascii_whitespace() {
        *i += 1;
    }
}

fn value(b: &[u8], i: &mut usize) -> Json {
    skip(b, i);
    match b[*i] {
        b'{' | b'[' => {
            let obj = b[*i] == b'{';
            *i += 1;
            let (mut m, mut l) = (Vec::new(), Vec::new());
            loop {
                skip(b, i);
                if b[*i] == b'}' || b[*i] == b']' {
                    *i += 1;
                    break;
                }
                if obj {
                    let Json::Str(k) = value(b, i) else {
                        unreachable!("a key")
                    };
                    skip(b, i);
                    *i += 1;
                    m.push((k, value(b, i)));
                } else {
                    l.push(value(b, i));
                }
                skip(b, i);
                if b[*i] == b',' {
                    *i += 1;
                }
            }
            if obj {
                Json::Obj(m)
            } else {
                Json::List(l)
            }
        }
        b'"' => {
            *i += 1;
            let start = *i;
            while b[*i] != b'"' {
                *i += 1;
            }
            *i += 1;
            Json::Str(String::from_utf8(b[start..*i - 1].to_vec()).unwrap())
        }
        b'n' => {
            *i += 4;
            Json::Null
        }
        b't' => {
            *i += 4;
            Json::Bool(true)
        }
        b'f' => {
            *i += 5;
            Json::Bool(false)
        }
        _ => {
            let start = *i;
            while *i < b.len() && b"+-0123456789.eE".contains(&b[*i]) {
                *i += 1;
            }
            let t = std::str::from_utf8(&b[start..*i]).unwrap();
            match t.parse::<i64>() {
                Ok(n) if !t.contains(['.', 'e', 'E']) && t != "-0" => Json::from(n),
                _ => Json::float(t.parse::<f64>().unwrap()).unwrap(),
            }
        }
    }
}

// ----- the shared vectors -----

fn trs(loc: [f64; 3], rot: Rotation, scale: [f64; 3]) -> Step {
    Step::Trs { loc, rot, scale }
}

fn pl(relative_to: Option<&str>, steps: Vec<Step>, precision: Precision) -> Placement {
    Placement {
        relative_to: relative_to.map(str::to_owned),
        steps,
        approx: None,
        precision,
    }
}

fn eye16() -> [f64; 16] {
    let mut m = [0.0; 16];
    for i in [0, 5, 10, 15] {
        m[i] = 1.0;
    }
    m
}

fn mat_json(m: Option<Mat4>) -> Json {
    m.map_or(Json::Null, |m| {
        Json::List(m.iter().map(|x| Json::float(*x).unwrap()).collect())
    })
}

/// Every case of the shared vectors: a name, a placement as JSON (some not readable), its
/// precision.
fn vector_cases() -> Vec<(String, Json, Precision)> {
    use Precision::{F32, F64};
    let e = |o: EulerOrder| Rotation::Euler {
        order: o,
        angles: [0.3, -1.1, 2.5],
    };
    let mut out: Vec<(String, Placement)> = Vec::new();
    for o in EulerOrder::ALL {
        out.push((
            format!("trs euler {}", o.as_str()),
            pl(None, vec![trs([1.0, 2.0, 3.0], e(o), [1.0, 2.0, 0.5])], F64),
        ));
    }
    let q = |w: f64, x, y, z| Rotation::Quat([w, x, y, z]);
    out.push((
        "trs quat".into(),
        pl(
            None,
            vec![trs([0.0; 3], q(0.9238795, 0.0, 0.0, 0.3826834), [1.0; 3])],
            F64,
        ),
    ));
    out.push((
        "trs quat unnormalised".into(),
        pl(
            None,
            vec![trs([0.0; 3], q(2.0, 0.0, 2.0, 0.0), [1.0; 3])],
            F64,
        ),
    ));
    out.push((
        "trs quat zero".into(),
        pl(
            None,
            vec![trs([0.0; 3], q(0.0, 0.0, 0.0, 0.0), [1.0; 3])],
            F64,
        ),
    ));
    let aa = |axis: [f64; 3], angle: f64| Rotation::AxisAngle { axis, angle };
    out.push((
        "trs axis angle".into(),
        pl(
            None,
            vec![trs([0.0; 3], aa([0.0, 0.0, 2.0], 1.2), [1.0; 3])],
            F64,
        ),
    ));
    out.push((
        "trs zero axis".into(),
        pl(None, vec![trs([0.0; 3], aa([0.0; 3], 1.2), [1.0; 3])], F64),
    ));
    out.push((
        "trs zero axis zero angle".into(),
        pl(None, vec![trs([0.0; 3], aa([0.0; 3], 0.0), [1.0; 3])], F64),
    ));
    let axes = |z: Option<[f64; 3]>, x: Option<[f64; 3]>| Step::Axes {
        origin: [10.0, 20.0, 30.0],
        z,
        x,
    };
    out.push((
        "axes defaults".into(),
        pl(None, vec![axes(None, None)], F64),
    ));
    out.push((
        "axes z only".into(),
        pl(None, vec![axes(Some([0.0, 1.0, 1.0]), None)], F64),
    ));
    out.push((
        "axes x only".into(),
        pl(None, vec![axes(None, Some([0.0, 1.0, 0.0]))], F64),
    ));
    out.push((
        "axes both, x not at right angles".into(),
        pl(
            None,
            vec![axes(Some([0.0, 0.0, 3.0]), Some([1.0, 1.0, 1.0]))],
            F64,
        ),
    ));
    out.push((
        "axes z along x, x defaulted".into(),
        pl(None, vec![axes(Some([1.0, 0.0, 0.0]), None)], F64),
    ));
    out.push((
        "axes z along -x, x defaulted".into(),
        pl(None, vec![axes(Some([-1.0, 0.0, 0.0]), None)], F64),
    ));
    out.push((
        "axes x along z".into(),
        pl(
            None,
            vec![axes(Some([0.0, 0.0, 1.0]), Some([0.0, 0.0, 2.0]))],
            F64,
        ),
    ));
    out.push((
        "axes zero z".into(),
        pl(None, vec![axes(Some([0.0; 3]), None)], F64),
    ));
    out.push((
        "axes zero x".into(),
        pl(None, vec![axes(None, Some([0.0; 3]))], F64),
    ));
    let mut m = eye16();
    m[12] = 5.0;
    m[1] = 0.5;
    out.push(("matrix".into(), pl(None, vec![Step::Matrix(m)], F64)));
    let mut bad = m;
    bad[3] = 1e-9;
    out.push((
        "matrix last row not 0,0,0,1".into(),
        pl(None, vec![Step::Matrix(bad)], F64),
    ));
    let two = vec![
        Step::Matrix(m),
        trs([1.0, 0.0, 0.0], e(EulerOrder::Zxy), [2.0; 3]),
    ];
    out.push(("two steps".into(), pl(None, two.clone(), F64)));
    let mut five = two.clone();
    five.extend([
        axes(Some([0.0, 1.0, 0.0]), None),
        Step::Matrix(m),
        trs([0.0; 3], aa([1.0, 1.0, 0.0], 0.7), [1.0; 3]),
    ]);
    out.push((
        "five steps (no limit in a helper)".into(),
        pl(None, five, F64),
    ));
    let f = |x: f64| x as f32 as f64;
    out.push((
        "f32 numbers".into(),
        pl(
            None,
            vec![trs(
                [f(0.1), f(0.2), f(0.3)],
                Rotation::Euler {
                    order: EulerOrder::Xyz,
                    angles: [f(0.7), 0.0, f(1.3)],
                },
                [f(1.1); 3],
            )],
            F32,
        ),
    ));
    let mut cases: Vec<(String, Json, Precision)> = out
        .into_iter()
        .map(|(n, p)| (n, p.to_value(), p.precision))
        .collect();
    // what a reader doesn't know: an unknown placement, never a skipped step
    cases.push((
        "unknown step kind".into(),
        parse_json(r#"{"relative_to":null,"steps":[{"matrix":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]},{"lookat":{"at":[0,0,0]}}]}"#),
        F64,
    ));
    cases.push((
        "unknown euler order".into(),
        parse_json(r#"{"relative_to":null,"steps":[{"trs":{"loc":[0,0,0],"rot":{"euler":"XYW","angles":[0,0,0]},"scale":[1,1,1]}}]}"#),
        F64,
    ));
    cases.push((
        "no steps".into(),
        parse_json(r#"{"relative_to":null,"steps":[]}"#),
        F64,
    ));
    cases
}

/// The chains of the shared vectors: a name, the placements by address (`null`: shown, not
/// placed), the address asked.
/// A chain of the shared vectors: its name, the placements by address, the address asked.
type VChain = (String, Vec<(String, Json)>, String);

fn vector_chains() -> Vec<VChain> {
    let t = |x: f64, rel: Option<&str>| {
        pl(
            rel,
            vec![trs(
                [x, 0.0, 0.0],
                Rotation::Euler {
                    order: EulerOrder::Xyz,
                    angles: [0.0, 0.0, 0.4],
                },
                [1.0; 3],
            )],
            Precision::F64,
        )
        .to_value()
    };
    let chain = |n: usize| -> Vec<(String, Json)> {
        (0..n)
            .map(|i| {
                let rel = (i > 0).then(|| format!("@u:{}", i - 1));
                (format!("@u:{i}"), t(i as f64 + 1.0, rel.as_deref()))
            })
            .collect()
    };
    let mut out = Vec::new();
    for n in 1..=5 {
        out.push((format!("a chain of {n}"), chain(n), format!("@u:{}", n - 1)));
    }
    out.push((
        "a missing link (an address not shown)".into(),
        vec![("@u:1".into(), t(1.0, Some("@u:0")))],
        "@u:1".into(),
    ));
    out.push((
        "an unplaced parent".into(),
        vec![
            ("@u:0".into(), Json::Null),
            ("@u:1".into(), t(1.0, Some("@u:0"))),
        ],
        "@u:1".into(),
    ));
    out.push((
        "a cycle".into(),
        vec![
            ("@u:0".into(), t(1.0, Some("@u:1"))),
            ("@u:1".into(), t(1.0, Some("@u:0"))),
        ],
        "@u:1".into(),
    ));
    out.push((
        "an unknown kind on the way".into(),
        vec![
            (
                "@u:0".into(),
                parse_json(r#"{"relative_to":null,"steps":[{"spline":{}}]}"#),
            ),
            ("@u:1".into(), t(1.0, Some("@u:0"))),
        ],
        "@u:1".into(),
    ));
    out.push(("a degenerate parent".into(), vec![("@u:0".into(), parse_json(r#"{"relative_to":null,"steps":[{"trs":{"loc":[0,0,0],"rot":{"quat":[0,0,0,0]},"scale":[1,1,1]}}]}"#)), ("@u:1".into(), t(1.0, Some("@u:0")))], "@u:1".into()));
    out
}

/// The shared vectors as the Rust helper computes them.
fn vectors() -> Json {
    let cases = vector_cases()
        .into_iter()
        .map(|(name, p, prec)| {
            let m = Placement::from_value(&p, prec).and_then(|p| p.matrix());
            Json::obj([
                ("name", Json::from(name)),
                ("precision", Json::from(prec.as_str())),
                ("placement", p),
                ("matrix", mat_json(m)),
            ])
        })
        .collect();
    let chains = vector_chains()
        .into_iter()
        .map(|(name, ps, at)| {
            let held: HashMap<String, Option<Placement>> = ps
                .iter()
                .map(|(a, p)| (a.clone(), Placement::from_value(p, Precision::F64)))
                .collect();
            let w = world_matrix(&at, |a| match held.get(a) {
                Some(Some(p)) => Link::Placed(p),
                Some(None) => Link::Unplaced,
                None => Link::NotShown,
            });
            Json::obj([
                ("name", Json::from(name)),
                ("precision", Json::from("f64")),
                ("placements", Json::Obj(ps)),
                ("address", Json::from(at)),
                ("world", mat_json(w)),
            ])
        })
        .collect();
    Json::obj([
        (
            "about",
            Json::from(
                "k2-mpc-base placement vectors for the JS client: each placement's matrix \
                 (column-major, null when unknown or degenerate) and each chain's world matrix \
                 (null when broken), as the kit's Rust helper computes them. Written by \
                 crates/base/tests/view_place.rs (MPC_BLESS=1).",
            ),
        ),
        ("cases", Json::List(cases)),
        ("chains", Json::List(chains)),
    ])
}

fn golden(name: &str, got: &str) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../client/test")
        .join(name);
    if std::env::var_os("MPC_BLESS").is_some() {
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing (MPC_BLESS=1 writes it)", path.display()));
    assert!(
        want == got,
        "{} differs from what the kit computes now (MPC_BLESS=1 regenerates it)\n--- now:\n{got}",
        path.display()
    );
}

/// The shared vectors are the Rust helper's: every form, order and degenerate case, unknown
/// kinds, and chains, written as the helper computes them (`vectors/placement.json`, which the
/// client's helper reads too). And the forms agree with each other where they must.
#[test]
fn the_shared_vectors_are_the_rust_helper_s() {
    let mut text = vectors().to_json_pretty();
    text.push('\n');
    golden("vectors/placement.json", &text);
    // a quarter turn about Z, three ways
    let quarter = std::f64::consts::FRAC_PI_2;
    let by = |rot: Rotation| {
        pl(None, vec![trs([0.0; 3], rot, [1.0; 3])], Precision::F64)
            .matrix()
            .unwrap()
    };
    let e = by(Rotation::Euler {
        order: EulerOrder::Xyz,
        angles: [0.0, 0.0, quarter],
    });
    let a = by(Rotation::AxisAngle {
        axis: [0.0, 0.0, 1.0],
        angle: quarter,
    });
    let h = std::f64::consts::FRAC_1_SQRT_2;
    let q = by(Rotation::Quat([h, 0.0, 0.0, h]));
    for (x, y) in [(&e, &a), (&e, &q)] {
        assert!(
            x.iter().zip(y.iter()).all(|(u, v)| (u - v).abs() < 1e-15),
            "{x:?} vs {y:?}"
        );
    }
    // x maps to y: column 0 is (0, 1, 0)
    assert!((e[0]).abs() < 1e-15 && (e[1] - 1.0).abs() < 1e-15, "{e:?}");
    // Euler XYZ is Rz · Ry · Rx
    let one = |o, ang: [f64; 3]| {
        by(Rotation::Euler {
            order: o,
            angles: ang,
        })
    };
    let xyz = one(EulerOrder::Xyz, [0.3, -1.1, 2.5]);
    let rx = one(EulerOrder::Xyz, [0.3, 0.0, 0.0]);
    let ry = one(EulerOrder::Xyz, [0.0, -1.1, 0.0]);
    let rz = one(EulerOrder::Xyz, [0.0, 0.0, 2.5]);
    let prod = mul(&mul(&rz, &ry), &rx);
    assert!(xyz
        .iter()
        .zip(prod.iter())
        .all(|(u, v)| (u - v).abs() < 1e-15));
}

// ----- the rows -----

/// A placement of each form round-trips the wire, every number bit for bit, at each precision;
/// an `F32` slot prints the shortest decimal of the `f32` (`0.1`, not its `f64` digits).
fn row_a_placement_round_trips_the_wire() -> Result<(), String> {
    const ROW: &str = "row_a_placement_round_trips_the_wire";
    for (name, j, prec) in vector_cases() {
        let Some(p) = Placement::from_value(&j, prec) else {
            continue;
        };
        let back = Placement::from_value(&parse_json(&p.to_value().to_json()), prec);
        if back.as_ref() != Some(&p) {
            return Err(format!("{ROW}: {name} came back as {back:?}"));
        }
    }
    let f = |x: f64| x as f32 as f64;
    let p = pl(
        None,
        vec![trs(
            [f(0.1), 0.0, 0.0],
            Rotation::Quat([1.0, 0.0, 0.0, 0.0]),
            [1.0; 3],
        )],
        Precision::F32,
    );
    let text = p.to_value().to_json();
    if !text.contains(r#""loc":[0.1,0,0]"#) {
        return Err(format!("{ROW}: an f32 slot printed {text}"));
    }
    let back =
        Placement::from_value(&parse_json(&text), Precision::F32).ok_or("control: unread")?;
    let Step::Trs { loc, .. } = &back.steps[0] else {
        return Err(format!("{ROW}: control: {back:?}"));
    };
    if loc[0] as f32 != 0.1f32 {
        return Err(format!("{ROW}: the f32 read back is {}", loc[0]));
    }
    Ok(())
}

#[test]
fn a_placement_round_trips_the_wire() {
    row_a_placement_round_trips_the_wire().unwrap();
}

#[test]
fn plant_f32_as_f64_digits_is_caught() {
    row_a_placement_round_trips_the_wire().expect("the row passes as built");
    let e = with_place_mutant(
        PlaceMutant::F32AsF64Digits,
        row_a_placement_round_trips_the_wire,
    )
    .expect_err("the row passes with f64 digits");
    assert!(
        e.starts_with("row_a_placement_round_trips_the_wire: an f32 slot printed"),
        "{e}"
    );
}

/// A pack that hands an `F32` slot a number that isn't an `f32` fails a test build, naming the
/// element; one whose numbers are `f32`s is carried, printed short.
#[test]
fn an_f32_slot_refuses_a_number_that_isn_t_an_f32() {
    let f = fx("f32", Mode::F32, SAMPLE);
    let ix = f.index();
    let u = ix
        .unit("@o:1")
        .and_then(|u| u.placement.clone())
        .expect("o1 placed");
    assert_eq!(u.precision, Precision::F32);
    f.set(1, "raw", "1");
    f.set(1, "xform", "0.1,2,3,0,0,0,1,1,1");
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.s.view_index(f.ada)));
    let e = match r {
        Ok(_) => panic!("a number that isn't an f32 went through"),
        Err(p) => p.downcast_ref::<String>().cloned().unwrap_or_default(),
    };
    assert!(
        e.contains("@o:1: the pack's placement can't be carried") && e.contains("isn't an f32"),
        "refused, but not as the kit says: {e}"
    );
}

/// A change to one unit's rotation alone carries `place` and its new placement, and nothing for
/// its child, whose container (another group) the render hint names.
fn row_a_field_change_carries_place_and_hints_the_children() -> Result<(), String> {
    const ROW: &str = "row_a_field_change_carries_place_and_hints_the_children";
    let f = fx("place", Mode::F64, SAMPLE);
    let (mut w, _) = f.watch();
    f.set(2, "xform", "0,0,1,0.1,0.2,0.9,1,1,1");
    let ch = changes(&w.try_frames());
    let c = ch.first().ok_or(format!("{ROW}: control: no frame"))?;
    let d = delta(c, "@o:2").ok_or(format!("{ROW}: control: no delta for @o:2"))?;
    let ops = d.get("ops").map(Json::to_json).unwrap_or_default();
    if !ops.contains("\"place\"") {
        return Err(format!("{ROW}: @o:2's ops {ops}, no place"));
    }
    let after = d
        .get("placement_after")
        .ok_or(format!("{ROW}: no placement_after"))?;
    let want = f
        .index()
        .unit("@o:2")
        .and_then(|u| u.placement.clone())
        .ok_or("control")?;
    if after.to_json() != want.to_value().to_json() {
        return Err(format!("{ROW}: placement_after {}", after.to_json()));
    }
    if delta(c, "@o:3").is_some() {
        return Err(format!("{ROW}: the child @o:3 has a delta of its own"));
    }
    let hint = c.get("containers").map(Json::to_json).unwrap_or_default();
    if !hint.contains("\"@g:2\"") {
        return Err(format!(
            "{ROW}: the render hint {hint} doesn't name the child's @g:2"
        ));
    }
    Ok(())
}

#[test]
fn a_field_change_carries_place_and_hints_the_children() {
    row_a_field_change_carries_place_and_hints_the_children().unwrap();
}

#[test]
fn plant_place_dropped_on_rotation_is_caught() {
    row_a_field_change_carries_place_and_hints_the_children().expect("the row passes as built");
    let e = with_place_mutant(
        PlaceMutant::PlaceDroppedOnRotation,
        row_a_field_change_carries_place_and_hints_the_children,
    )
    .expect_err("the row passes with place dropped");
    assert!(e.contains("@o:2's ops") && e.contains("no place"), "{e}");
}

#[test]
fn plant_no_chain_hint_is_caught() {
    row_a_field_change_carries_place_and_hints_the_children().expect("the row passes as built");
    let e = with_place_mutant(
        PlaceMutant::NoChainHint,
        row_a_field_change_carries_place_and_hints_the_children,
    )
    .expect_err("the row passes with no chain hint");
    assert!(e.contains("doesn't name the child's @g:2"), "{e}");
}

/// A change to a field outside the slot's fields asks the pack for no placement; one to a
/// placement field asks for that unit's alone (counted).
fn row_only_placement_fields_ask_the_pack() -> Result<(), String> {
    const ROW: &str = "row_only_placement_fields_ask_the_pack";
    // the kit's own test-build check builds every index whole (asking for every placement):
    // off, so the counts are a host build's
    struct Back(bool);
    impl Drop for Back {
        fn drop(&mut self) {
            set_view_checks(self.0);
        }
    }
    let _back = Back(set_view_checks(false));
    let f = fx("asks", Mode::F64, SAMPLE);
    f.index();
    take_placement_asks();
    f.set(1, "name", "Renamed");
    f.index();
    let n = take_placement_asks();
    if n != 0 {
        return Err(format!(
            "{ROW}: a name change asked for {n} placements, want 0"
        ));
    }
    f.set(1, "xform", "9,9,9,0,0,0,1,1,1");
    f.index();
    let n = take_placement_asks();
    if n != 1 {
        return Err(format!(
            "{ROW}: a placement field asked for {n} placements, want 1"
        ));
    }
    Ok(())
}

#[test]
fn only_placement_fields_ask_the_pack() {
    row_only_placement_fields_ask_the_pack().unwrap();
}

#[test]
fn plant_ask_every_unit_is_caught() {
    row_only_placement_fields_ask_the_pack().expect("the row passes as built");
    let e = with_place_mutant(
        PlaceMutant::AskEveryUnit,
        row_only_placement_fields_ask_the_pack,
    )
    .expect_err("the row passes asking every unit");
    assert!(
        e.starts_with("row_only_placement_fields_ask_the_pack: a name change asked for"),
        "{e}"
    );
}

/// A unit that comes into view (an insert, a restore) carries its placement.
fn row_a_unit_coming_into_view_carries_its_placement() -> Result<(), String> {
    const ROW: &str = "row_a_unit_coming_into_view_carries_its_placement";
    let f = fx("comes", Mode::F64, SAMPLE);
    let (mut w, _) = f.watch();
    let k = okey(40);
    f.edit(vec![
        Edit::List {
            root: OBJS.into(),
            ops: vec![ListOp::Insert {
                key: k.clone(),
                at: Anchor::End,
                value: Value::str("mesh"),
            }],
        },
        set_op(OBJ_F, &k, "group", "g1"),
        set_op(OBJ_F, &k, "name", "New"),
        set_op(OBJ_F, &k, "xform", "3,3,3,0,0,0,1,1,1"),
    ]);
    let ch = changes(&w.try_frames());
    let added = ch
        .first()
        .and_then(|c| delta(c, "@o:40"))
        .ok_or(format!("{ROW}: control: no add"))?;
    if added
        .get("placement_after")
        .map_or(true, |p| *p == Json::Null)
    {
        return Err(format!("{ROW}: the insert's delta {}", added.to_json()));
    }
    f.edit(vec![Edit::List {
        root: OBJS.into(),
        ops: vec![ListOp::Delete { key: k.clone() }],
    }]);
    f.edit(vec![Edit::List {
        root: OBJS.into(),
        ops: vec![ListOp::Restore { key: k }],
    }]);
    let ch = changes(&w.try_frames());
    let back = ch
        .last()
        .and_then(|c| delta(c, "@o:40"))
        .ok_or(format!("{ROW}: control: no restore"))?;
    if back
        .get("placement_after")
        .map_or(true, |p| *p == Json::Null)
    {
        return Err(format!("{ROW}: the restore's delta {}", back.to_json()));
    }
    Ok(())
}

#[test]
fn a_unit_coming_into_view_carries_its_placement() {
    row_a_unit_coming_into_view_carries_its_placement().unwrap();
}

#[test]
fn plant_no_placement_on_add_is_caught() {
    row_a_unit_coming_into_view_carries_its_placement().expect("the row passes as built");
    let e = with_place_mutant(
        PlaceMutant::NoPlacementOnAdd,
        row_a_unit_coming_into_view_carries_its_placement,
    )
    .expect_err("the row passes with no placement on add");
    assert!(
        e.starts_with("row_a_unit_coming_into_view_carries_its_placement: the insert's delta"),
        "{e}"
    );
}

fn close(a: &Mat4, b: &Mat4) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-9)
}

/// A door placed through its opening through a wall through a group has a world matrix: the
/// product along the chain.
#[test]
fn a_door_through_an_opening_through_a_wall_is_placed() {
    let f = fx("door", Mode::F64, SAMPLE);
    let ix = f.index();
    let p = |a: &str| -> Mat4 {
        let u = ix
            .unit(a)
            .map(|u| u.placement.clone())
            .or_else(|| ix.container(a).map(|c| c.placement.clone()));
        u.flatten()
            .and_then(|p| p.matrix())
            .unwrap_or_else(|| panic!("{a} placed"))
    };
    let want = mul(
        &mul(&mul(&p("@g:1"), &p("@o:10")), &p("@o:11")),
        &p("@o:12"),
    );
    let got = ix
        .world_matrix("@o:12")
        .expect("the door has a world matrix");
    assert!(close(&got, &want), "{got:?} vs {want:?}");
    // the wall's axes: X along (0,1,0), so the opening's 500 along x lands at y 500
    assert!(
        (got[12] - 4000.0).abs() < 1e-9
            && (got[13] - 500.0).abs() < 1e-9
            && (got[14] - 10.0).abs() < 1e-9,
        "{got:?}"
    );
}

/// A broken chain gives no world matrix: through an element with no placement, through an
/// address nobody has; never identity for the missing link.
fn row_a_broken_chain_has_no_world_matrix() -> Result<(), String> {
    const ROW: &str = "row_a_broken_chain_has_no_world_matrix";
    let f = fx("broken", Mode::F64, SAMPLE);
    let ix = f.index();
    for a in ["@o:20", "@o:22"] {
        if let Some(m) = ix.world_matrix(a) {
            return Err(format!("{ROW}: {a} has a world matrix {m:?}"));
        }
    }
    if ix.world_matrix("@o:1").is_none() {
        return Err(format!("{ROW}: control: @o:1 has none"));
    }
    Ok(())
}

#[test]
fn a_broken_chain_has_no_world_matrix() {
    row_a_broken_chain_has_no_world_matrix().unwrap();
}

#[test]
fn plant_missing_link_identity_is_caught() {
    row_a_broken_chain_has_no_world_matrix().expect("the row passes as built");
    let e = with_place_mutant(
        PlaceMutant::MissingLinkIdentity,
        row_a_broken_chain_has_no_world_matrix,
    )
    .expect_err("the row passes with identity for a missing link");
    assert!(
        e.starts_with("row_a_broken_chain_has_no_world_matrix: @o:20 has a world matrix"),
        "{e}"
    );
}

/// A placement relative to nothing is in the doc's space: its group's own placement (here
/// 100 along x) plays no part.
fn row_none_is_the_doc_space() -> Result<(), String> {
    const ROW: &str = "row_none_is_the_doc_space";
    let f = fx("none", Mode::F64, SAMPLE);
    let ix = f.index();
    let m = ix
        .world_matrix("@o:30")
        .ok_or(format!("{ROW}: control: none"))?;
    if (m[12], m[13], m[14]) != (1.0, 2.0, 3.0) {
        return Err(format!("{ROW}: @o:30 lands at {:?}", (m[12], m[13], m[14])));
    }
    Ok(())
}

#[test]
fn none_is_the_doc_space() {
    row_none_is_the_doc_space().unwrap();
}

#[test]
fn plant_none_is_container_frame_is_caught() {
    row_none_is_the_doc_space().expect("the row passes as built");
    let e = with_place_mutant(PlaceMutant::NoneIsContainerFrame, row_none_is_the_doc_space)
        .expect_err("the row passes with None read as the container's frame");
    assert!(
        e.starts_with("row_none_is_the_doc_space: @o:30 lands at (101.0, 2.0, 3.0)"),
        "{e}"
    );
}

/// What `f` unwound with, or `None` when it returns.
fn refusal_of<R>(f: impl FnOnce() -> R) -> Option<String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(_) => None,
        Err(p) => Some(
            p.downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_default(),
        ),
    }
}

/// The sample with the wall placed relative to its own door: @o:10 -> @o:12 -> @o:11 -> @o:10.
fn cycle_sample() -> String {
    SAMPLE.replace(
        "axes=4000,0,0/-/0,1,0;parent=@g:1",
        "axes=4000,0,0/-/0,1,0;parent=@o:12",
    )
}

/// A pack whose placement chain is a cycle fails a test build loudly, naming the elements
/// around it: through a change kept by the index, and on a whole build of a file that holds
/// one. A chain four long through the same elements builds.
fn row_a_cycle_fails_a_test_build() -> Result<(), String> {
    const ROW: &str = "row_a_cycle_fails_a_test_build";
    let named = |e: &str| {
        e.contains("the pack's placement chain is a cycle: @o:10 -> @o:12 -> @o:11 -> @o:10")
    };
    let f = fx("cycle", Mode::F64, SAMPLE);
    if f.index().world_matrix("@o:12").is_none() {
        return Err(format!(
            "{ROW}: control: the door's chain has no world matrix"
        ));
    }
    f.set(10, "parent", "@o:12");
    match refusal_of(|| f.s.view_index(f.ada)) {
        None => {
            return Err(format!(
                "{ROW}: a cycle built and nothing refused it (a change)"
            ))
        }
        Some(e) if !named(&e) => {
            return Err(format!(
                "{ROW}: refused, but not as the kit says (a change): {e}"
            ))
        }
        Some(_) => {}
    }
    let g = fx("cycle-file", Mode::F64, &cycle_sample());
    match refusal_of(|| g.s.view_index(g.ada)) {
        None => Err(format!(
            "{ROW}: a cycle built and nothing refused it (a whole build)"
        )),
        Some(e) if !named(&e) => Err(format!(
            "{ROW}: refused, but not as the kit says (a whole build): {e}"
        )),
        Some(_) => Ok(()),
    }
}

#[test]
fn a_cycle_fails_a_test_build() {
    row_a_cycle_fails_a_test_build().unwrap();
}

#[test]
fn plant_cycle_unchecked_is_caught() {
    row_a_cycle_fails_a_test_build().expect("the row passes as built");
    let e = with_place_mutant(PlaceMutant::CycleUnchecked, row_a_cycle_fails_a_test_build)
        .expect_err("the row passes with no cycle check");
    assert!(
        e.starts_with(
            "row_a_cycle_fails_a_test_build: a cycle built and nothing refused it (a change)"
        ),
        "{e}"
    );
}

/// Unchecked, as in a release build, the index with a cycle is kept and nothing placed
/// through it has a world matrix; what is placed outside it still has one.
#[test]
fn a_cycle_has_no_world_matrix_where_unchecked() {
    let f = fx("cycle-release", Mode::F64, &cycle_sample());
    let ix = with_place_mutant(PlaceMutant::CycleUnchecked, || f.index());
    for a in ["@o:10", "@o:11", "@o:12"] {
        assert!(
            ix.world_matrix(a).is_none(),
            "{a} has a world matrix in a cycle"
        );
    }
    assert!(ix.world_matrix("@o:1").is_some(), "control: @o:1 has none");
}

/// An outside save that changes only what the carrier holds (o2's anonymous frame) makes the
/// watch take a fresh snapshot, whose placement is the new one; one that moves an object by its
/// fields reaches the watch as a frame with `place`, and no snapshot.
#[test]
fn a_carrier_only_change_takes_a_snapshot() {
    let f = fx("carrier", Mode::F64, SAMPLE);
    let (mut w, _) = f.watch();
    // each save changes the file's size, so a check can't take it for the file it had
    let check = f.outside_save(|t| {
        t.replace(
            "O|1|1|mesh|Cube|xform=1,2,3",
            "O|1|1|mesh|Cube|xform=1,2,40",
        )
    });
    let ch = changes(&w.try_frames());
    let d = ch
        .first()
        .and_then(|c| delta(c, "@o:1"))
        .unwrap_or_else(|| {
            panic!(
                "the field save's frame ({check}): {:?}, needs a snapshot {}",
                ch.iter().map(Json::to_json).collect::<Vec<_>>(),
                w.needs_snapshot()
            )
        });
    assert!(
        d.get("ops").unwrap().to_json().contains("\"place\""),
        "{}",
        d.to_json()
    );
    assert!(!w.needs_snapshot(), "a field save took a snapshot");
    let check = f.outside_save(|t| {
        t.replace(
            "F|2|1,0,0,0,0,1,0,0,0,0,1,0,7,0,0,1",
            "F|2|1,0,0,0,0,1,0,0,0,0,1,0,80,0,0,1",
        )
    });
    w.try_frames();
    assert!(
        w.needs_snapshot(),
        "a carrier-only save ({check}) reached no watcher"
    );
    f.s.watch_snapshot(&mut w).unwrap();
    let p = f
        .index()
        .unit("@o:2")
        .and_then(|u| u.placement.clone())
        .unwrap();
    let Step::Matrix(m) = &p.steps[0] else {
        panic!("the frame first: {p:?}")
    };
    assert_eq!(m[12], 80.0);
}

/// An outside save that adds a unit placed by the carrier alone (its anonymous frame): the
/// add's delta carries its placement, built on the save's carrier, and no snapshot is needed.
/// A save that also moves another unit through the carrier alone still takes a snapshot.
fn row_an_outside_save_adding_a_unit_carries_its_placement() -> Result<(), String> {
    const ROW: &str = "row_an_outside_save_adding_a_unit_carries_its_placement";
    let f = fx("save-adds", Mode::F64, SAMPLE);
    let (mut w, _) = f.watch();
    let check =
        f.outside_save(|t| format!("{t}O|1|50|mesh|Anon|\nF|50|1,0,0,0,0,1,0,0,0,0,1,0,9,0,0,1\n"));
    let ch = changes(&w.try_frames());
    let added = ch.iter().find_map(|c| delta(c, "@o:50")).ok_or(format!(
        "{ROW}: control: no add in the save's frames ({check})"
    ))?;
    if added
        .get("placement_after")
        .map_or(true, |p| *p == Json::Null)
    {
        return Err(format!("{ROW}: the save's add delta {}", added.to_json()));
    }
    if w.needs_snapshot() {
        return Err(format!("{ROW}: the save's add took a snapshot"));
    }
    // a save adding a unit and moving another through the carrier alone: a snapshot
    let check = f.outside_save(|t| {
        format!(
            "{}O|1|51|mesh|Anon2|\nF|51|1,0,0,0,0,1,0,0,0,0,1,0,4,0,0,1\n",
            t.replace(
                "F|2|1,0,0,0,0,1,0,0,0,0,1,0,7,0,0,1",
                "F|2|1,0,0,0,0,1,0,0,0,0,1,0,70,0,0,1",
            )
        )
    });
    let ch = changes(&w.try_frames());
    let added = ch.iter().find_map(|c| delta(c, "@o:51"));
    if added.map_or(true, |a| {
        a.get("placement_after").map_or(true, |p| *p == Json::Null)
    }) {
        return Err(format!("{ROW}: the second save's add ({check}): {added:?}"));
    }
    if !w.needs_snapshot() {
        return Err(format!(
            "{ROW}: a save moving o2 through the carrier alone reached no watcher"
        ));
    }
    Ok(())
}

#[test]
fn an_outside_save_adding_a_unit_carries_its_placement() {
    row_an_outside_save_adding_a_unit_carries_its_placement().unwrap();
}

/// The doc's space rides the snapshot's view and a change's `view_space`.
#[test]
fn the_doc_space_rides_the_snapshot_and_changes() {
    let f = fx("space", Mode::F64, SAMPLE);
    let (mut w, frames) = f.watch();
    let head = frames[0].to_value();
    let sp = head
        .get("view")
        .and_then(|v| v.get("space"))
        .map(Json::to_json);
    assert_eq!(
        sp.as_deref(),
        Some(r#"{"length_unit":"mm","metres_per_unit":0.001,"up":"z","handed":"right"}"#)
    );
    let schema = head
        .get("view")
        .and_then(|v| v.get("schema"))
        .and_then(|s| s.get("space"))
        .map(Json::to_json);
    assert!(schema.unwrap_or_default().contains(r#""precision":"f64""#));
    f.edit(vec![Edit::Map {
        root: SCENE.into(),
        ops: vec![MapOp::Set {
            key: "unit".into(),
            value: Value::str("m"),
        }],
    }]);
    let ch = changes(&w.try_frames());
    let vs = ch
        .first()
        .and_then(|c| c.get("view_space"))
        .map(Json::to_json);
    assert!(vs.unwrap_or_default().contains(r#""length_unit":"m""#));
}

/// A 2D pack (no slot) has no placement and no space anywhere: its frames are as before (the
/// keyed watch's goldens, unchanged, say the same).
#[test]
fn a_pack_with_no_slot_carries_no_placement() {
    let schema = ViewSchema {
        space: None,
        ..view(Precision::F64)
    };
    assert!(schema.to_value().get("space").is_none());
}

// ----- the client's golden -----

/// The frames a watcher of the sample gets (the snapshot, a rotation, an insert, a parent's
/// move, the space), the doc's state and every unit's and container's world matrix at the end
/// (`null` for a broken chain), for the JS client (`golden/keyed_frames_placement.json`;
/// `MPC_BLESS=1` rewrites it). A change frame's `client` is written as 1.
#[test]
fn golden_frames_with_placements() {
    let f = fx("golden", Mode::F64, SAMPLE);
    let (mut w, mut frames) = f.watch();
    f.set(2, "xform", "0,0,1,0.1,0.2,0.9,1,1,1");
    let k = okey(40);
    f.edit(vec![
        Edit::List {
            root: OBJS.into(),
            ops: vec![ListOp::Insert {
                key: k.clone(),
                at: Anchor::End,
                value: Value::str("mesh"),
            }],
        },
        set_op(OBJ_F, &k, "group", "g2"),
        set_op(OBJ_F, &k, "name", "New"),
        set_op(OBJ_F, &k, "xform", "3,3,3,0,0,0,1,1,1"),
        set_op(OBJ_F, &k, "parent", "@o:3"),
    ]);
    f.set(10, "axes", "4000,0,0/0,0,1/1,1,0");
    f.edit(vec![set_op(GROUP_F, &gkey(2), "place", "200,0,0")]);
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
    let ix = f.index();
    let world = ix
        .units
        .iter()
        .map(|u| u.key.clone())
        .chain(ix.containers.iter().map(|c| c.key.clone()))
        .map(|a| {
            let m = ix.world_matrix(&a);
            (a, mat_json(m))
        })
        .collect::<Vec<_>>();
    let state = f.s.keyed_state_for_tests().unwrap();
    let mut text = Json::obj([
        (
            "features",
            Json::List(vec![Json::from(KEYED_WATCH_FEATURE)]),
        ),
        ("frames", Json::List(frames)),
        (
            "final_json",
            Json::from(watch_state_json(&Scene3(Mode::F64), &state)),
        ),
        ("final_rev", Json::from(f.version())),
        ("final_world", Json::Obj(world)),
    ])
    .to_json_pretty();
    text.push('\n');
    golden("golden/keyed_frames_placement.json", &text);
}
