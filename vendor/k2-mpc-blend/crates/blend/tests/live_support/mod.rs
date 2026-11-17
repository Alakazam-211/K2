//! A live session over a BLEND file (the kit's session manager, a store, a blob store for the
//! carrier and the conflict copies, a host that grants everyone everything), the scenes the live
//! rows use (built here with `k2_mpc_blend::testgen`, never from another program's files), and
//! the rows themselves ([`rows`]), shared by `tests/live.rs` and the planted bugs in
//! `tests/mutants.rs`.
#![allow(dead_code)]

pub mod rows;
pub mod unchecked;
pub mod view_rows;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::Value;
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Host, Participant, ParticipantKind, Rights, Session, SessionError,
    SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{ToolOutput, ToolRunError};
use k2_mpc_blend::live::{BlendLive, OBJS};
use k2_mpc_blend::testgen::{Driven, ObjectSpec, SceneBuilder};
use k2_mpc_blend::write::decompress;
use k2_mpc_blend::{read_blend, read_scene, ReadLimits, SceneModel};

pub struct TestHost {
    pub now: AtomicU64,
    pub events: Mutex<Vec<AuditEvent>>,
}

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, e: &AuditEvent) {
        self.events.lock().unwrap().push(e.clone());
    }
}

pub struct Fx {
    pub dir: PathBuf,
    pub ws: PathBuf,
    pub host: Arc<TestHost>,
    pub store: Arc<DocStore>,
    pub blobs: Arc<BlobStore>,
    pub mgr: SessionManager,
    pub pack: Arc<BlendLive>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

pub const NAME: &str = "scene.blend";

/// A fixture with the kit's default config, as `set` changes it.
pub fn fx(tag: &str, set: impl FnOnce(&mut Config)) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "k2blend-live-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&p);
    let ws = p.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_793_111_400_000),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(p.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            p.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    set(&mut c);
    let mgr = SessionManager::new(store.clone(), host.clone(), c).with_blob_store(blobs.clone());
    Fx {
        dir: p,
        ws,
        host,
        store,
        blobs,
        mgr,
        pack: Arc::new(BlendLive::default()),
    }
}

pub fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: NAME.into(),
    }
}

impl Fx {
    pub fn path(&self) -> PathBuf {
        self.ws.join(NAME)
    }
    pub fn write(&self, bytes: &[u8]) {
        fs::write(self.path(), bytes).unwrap();
    }
    pub fn bytes(&self) -> Vec<u8> {
        fs::read(self.path()).unwrap()
    }
    pub fn try_open(&self) -> Result<Arc<Session>, SessionError> {
        self.mgr
            .open_file(self.pack.clone(), &key(), &self.path(), &self.ws)
    }
    pub fn open(&self) -> Arc<Session> {
        self.try_open().expect("the scene opens live")
    }
    pub fn join(&self, s: &Session, pid: &str, kind: ParticipantKind) -> ConnId {
        s.join(
            Participant::new(pid, pid, "#123456", kind, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
    pub fn person(&self, s: &Session) -> ConnId {
        self.join(s, "ada", ParticipantKind::Person)
    }
    pub fn agent(&self, s: &Session) -> ConnId {
        self.join(s, "atlas", ParticipantKind::Agent)
    }
    /// A restart: a new manager on the same stores and a new pack, the old manager dropped
    /// without a write-back. Drop every session of the old manager first.
    pub fn restart(&mut self) {
        self.mgr = SessionManager::new(
            self.store.clone(),
            self.host.clone(),
            self.mgr.config().clone(),
        )
        .with_blob_store(self.blobs.clone());
        self.pack = Arc::new(BlendLive::default());
    }
    pub fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    /// Bytes under the blob folder.
    pub fn blob_bytes(&self) -> u64 {
        fn walk(p: &Path) -> u64 {
            let Ok(rd) = fs::read_dir(p) else { return 0 };
            rd.flatten()
                .map(|e| {
                    let p = e.path();
                    if p.is_dir() {
                        walk(&p)
                    } else {
                        e.metadata().map_or(0, |m| m.len())
                    }
                })
                .sum()
        }
        walk(&self.dir.join("blobs"))
    }
    /// Bytes of the store file.
    pub fn store_bytes(&self) -> u64 {
        fs::metadata(self.dir.join("docs.redb")).map_or(0, |m| m.len())
    }
}

/// An app's save: written beside the file, then renamed over it.
pub fn app_save(path: &Path, bytes: &[u8]) {
    let tmp = path.with_file_name(".app-save.tmp");
    fs::write(&tmp, bytes).unwrap();
    fs::rename(&tmp, path).unwrap();
}

pub fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_owned()).collect()
}

/// Run a verb as `c`, with no base.
pub fn run(s: &Session, c: ConnId, a: &[&str]) -> Result<ToolOutput, ToolRunError> {
    s.run_tool(c, OBJS, &args(a), b"")
}

/// Run an edit verb as `c`, with the version of a read it just made as its base.
pub fn run_based(s: &Session, c: ConnId, a: &[&str]) -> Result<ToolOutput, ToolRunError> {
    let v = run(s, c, &["scene", "list", "--limit", "1"])?.version;
    let mut all = args(a);
    all.push("--base".into());
    all.push(v.to_string());
    s.run_tool(c, OBJS, &all, b"")
}

pub fn stdout(o: &ToolOutput) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// The live doc's roots, as `c` reads them.
pub fn content(s: &Session, c: ConnId) -> Vec<(String, Neutral)> {
    s.read(c).unwrap().content
}

/// A field of the live store: `root`'s map, key `<elem>\x1f<field>`.
pub fn field(content: &[(String, Neutral)], root: &str, elem: &str, fld: &str) -> Option<String> {
    let k = format!("{elem}\x1f{fld}");
    content.iter().find_map(|(r, n)| match n {
        Neutral::Map(m) if r == root => m.get(&k).and_then(Value::as_str).map(str::to_owned),
        _ => None,
    })
}

/// A list root's `(key, value)`s.
pub fn list(content: &[(String, Neutral)], root: &str) -> Vec<(String, String)> {
    content
        .iter()
        .find_map(|(r, n)| match n {
            Neutral::List(l) if r == root => Some(
                l.iter()
                    .map(|(k, v)| (k.as_str().to_owned(), v.as_str().unwrap_or("").to_owned()))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// The live key of the object named `name`.
pub fn obj_named(content: &[(String, Neutral)], name: &str) -> Option<String> {
    list(content, OBJS)
        .into_iter()
        .map(|(k, _)| k)
        .find(|k| field(content, k2_mpc_blend::live::OBJ_PROPS, k, "name").as_deref() == Some(name))
}

/// The scene model of a file (decompressed first).
pub fn model(bytes: &[u8]) -> (Vec<u8>, SceneModel) {
    let l = ReadLimits::default();
    let (_, raw) = decompress(bytes, &l).expect("decompresses");
    let file = read_blend(&mut &raw[..], &l).expect("reads");
    let m = read_scene(&file).expect("models");
    (raw, m)
}

/// An object's field as the model reads it (`loc`, `size`, `restrictflag`...).
pub fn model_field(m: &SceneModel, key: &str, fld: &str) -> Option<String> {
    m.by_key(key)?
        .fields
        .iter()
        .find(|(n, _)| n == fld)
        .map(|(_, v)| v.clone())
}

/// What may differ between two saves of the outliner scene.
#[derive(Debug, Clone)]
pub struct Opts {
    /// Added to every address: another save of the same scene.
    pub shift: u64,
    /// The cube object's name.
    pub cube: String,
    /// The cube's location.
    pub cube_loc: [f32; 3],
    /// The lamp object's name.
    pub lamp: String,
    /// The lamp's location.
    pub lamp_loc: [f32; 3],
    /// An object `Added` first in the file (an app's save that adds one).
    pub added_first: bool,
    /// A duplicate of the cube, `<cube>.001`, sharing its mesh.
    pub dup: bool,
    /// The schema has the placement fields ([`SceneBuilder::placed`]): the child's parent
    /// inverse is [`CHILD_INVERSE`], the scene's unit scale 0.01, and three more objects in
    /// `Set`, last in the file: `Axis` (an axis and angle), `Nudged` (a delta location) and
    /// `Boned` (parented to the cube other than to the object itself, `partype` 4).
    pub placed: bool,
    /// The file version the scene is written at (4.03 by default).
    pub version: u16,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            shift: 0,
            cube: "Cube".into(),
            cube_loc: [0.0, 0.0, 0.25],
            lamp: "Lamp".into(),
            lamp_loc: [4.0, -1.0, 6.0],
            added_first: false,
            dup: false,
            placed: false,
            version: 403,
        }
    }
}

/// The child's parent inverse in the placed outliner: a move of -1 along X, column-major.
pub const CHILD_INVERSE: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 1.0,
];

/// The placed outliner's unit scale.
pub const UNIT_SCALE: f32 = 0.01;

/// The objects in `Crowd`.
pub const CROWD: usize = 64;

/// `Arm`'s parent inverse, column-major: a quarter turn about Z, then a move of (0.5, -2, 0).
pub const ARM_INVERSE: [f32; 16] = [
    0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.5, -2.0, 0.0, 1.0,
];

/// The rig scene: `Rig` holds `Base` and `Arm` (parented to `Base` through [`ARM_INVERSE`]),
/// `Crowd` holds [`CROWD`] empties on an 8 by 8 grid, every name in `Crowd` as wide as the next.
pub fn rig() -> Vec<u8> {
    let mut b = SceneBuilder::placed(403);
    b.ui();
    let crowd: Vec<String> = (0..CROWD).map(|i| format!("Crowd.{i:02}")).collect();
    let names: Vec<&str> = crowd.iter().map(String::as_str).collect();
    b.scene("Scene", &[], &["Rig", "Crowd"], None);
    b.collection("Rig", &["Base", "Arm"], &[]);
    b.collection("Crowd", &names, &[]);
    let empty = |loc: [f32; 3]| ObjectSpec {
        rotmode: 1,
        loc,
        ..ObjectSpec::default()
    };
    b.object("Base", &empty([0.0, 0.0, 1.0]));
    b.object(
        "Arm",
        &ObjectSpec {
            parent: Some("Base".into()),
            parentinv: ARM_INVERSE,
            ..empty([0.0, 2.0, 0.0])
        },
    );
    for (i, n) in names.iter().enumerate() {
        b.object(n, &empty([(i % 8) as f32, (i / 8) as f32, 0.0]));
    }
    b.bytes()
}

/// The outliner scene with the placement fields ([`Opts::placed`]).
pub fn placed_scene() -> Vec<u8> {
    outliner(&Opts {
        placed: true,
        ..Opts::default()
    })
}

/// `spec` rotating by Euler angles (XYZ), its `type` from its data's code (a mesh, a light, a
/// camera; an empty without data).
fn euler(spec: ObjectSpec) -> ObjectSpec {
    ObjectSpec {
        rotmode: 1,
        ..typed(spec)
    }
}

fn typed(spec: ObjectSpec) -> ObjectSpec {
    let ty = match spec.data.as_deref().map(|d| &d[..2]) {
        Some("ME") => 1,
        Some("LA") => 10,
        Some("CA") => 11,
        _ => 0,
    };
    ObjectSpec { ty, ..spec }
}

/// The outliner scene: a scene holding a camera and two collections, `Set` (the cube, the lamp,
/// a spinner, a walker, an idler and a nested `Inner` with a child and a held object) and `Props`
/// (the cube again); a loose object no collection holds and one linked from a library.
/// - `Cube`: a mesh, Euler, in `Set` and `Props`;
/// - `Child`: an empty parented to the cube, hidden in viewports and renders (`restrictflag` 5);
/// - `Spin`: a mesh rotating by quaternion (`edit` `t`);
/// - `Walker`: animated by an action (`edit` `a`); `Held`: a constraint (`a`);
/// - `Idle`: animation data holding nothing (`edit` `1`, the control);
/// - `Loose`: in no collection (`c0`); `Far`: linked from `lib` (`edit` `0`).
pub fn outliner(o: &Opts) -> Vec<u8> {
    let mut b = if o.placed {
        let mut b = SceneBuilder::placed(o.version);
        b.unit_scale = UNIT_SCALE;
        b
    } else {
        SceneBuilder::animated(o.version)
    };
    b.shift = o.shift;
    let (cube, lamp) = (o.cube.as_str(), o.lamp.as_str());
    let dup = format!("{cube}.001");
    let mut set: Vec<&str> = vec![cube, lamp, "Spin", "Walker", "Idle"];
    if o.dup {
        set.push(&dup);
    }
    if o.added_first {
        set.push("Added");
    }
    if o.placed {
        set.extend(["Axis", "Nudged", "Boned"]);
    }
    b.ui();
    b.library("lib", "//assets/lib.blend");
    b.scene("Scene", &["Camera"], &["Set", "Props"], Some("Camera"));
    b.collection("Set", &set, &["Inner"]);
    b.collection("Inner", &["Child", "Held"], &[]);
    b.collection("Props", &[cube], &[]);
    if o.added_first {
        b.object(
            "Added",
            &euler(ObjectSpec {
                loc: [9.0, 9.0, 9.0],
                ..ObjectSpec::default()
            }),
        );
    }
    b.object(
        "Camera",
        &euler(ObjectSpec {
            data: Some("CA:Cam".into()),
            loc: [1.0, 2.0, 3.0],
            ..ObjectSpec::default()
        }),
    );
    b.object(
        cube,
        &euler(ObjectSpec {
            data: Some("ME:Cube".into()),
            loc: o.cube_loc,
            materials: vec![Some("Red".into())],
            ..ObjectSpec::default()
        }),
    );
    if o.dup {
        b.object(
            &dup,
            &euler(ObjectSpec {
                data: Some("ME:Cube".into()),
                loc: [2.0, 0.0, 0.0],
                ..ObjectSpec::default()
            }),
        );
    }
    b.object(
        "Child",
        &euler(ObjectSpec {
            parent: Some(cube.into()),
            restrict: 5,
            loc: [0.0, 1.0, 0.0],
            parentinv: CHILD_INVERSE,
            ..ObjectSpec::default()
        }),
    );
    b.object(
        lamp,
        &euler(ObjectSpec {
            data: Some("LA:Lamp".into()),
            loc: o.lamp_loc,
            ..ObjectSpec::default()
        }),
    );
    b.object(
        "Spin",
        &typed(ObjectSpec {
            data: Some("ME:Ball".into()),
            rotmode: 0,
            ..ObjectSpec::default()
        }),
    );
    b.object(
        "Walker",
        &euler(ObjectSpec {
            data: Some("ME:Ball".into()),
            driven: Driven::Action,
            ..ObjectSpec::default()
        }),
    );
    b.object(
        "Held",
        &euler(ObjectSpec {
            driven: Driven::Constraint,
            ..ObjectSpec::default()
        }),
    );
    b.object(
        "Idle",
        &euler(ObjectSpec {
            driven: Driven::EmptyAnimData,
            loc: [-3.0, 0.0, 0.0],
            ..ObjectSpec::default()
        }),
    );
    b.object(
        "Loose",
        &euler(ObjectSpec {
            loc: [0.5, 0.5, 0.5],
            ..ObjectSpec::default()
        }),
    );
    b.object(
        "Far",
        &euler(ObjectSpec {
            lib: Some("lib".into()),
            ..ObjectSpec::default()
        }),
    );
    if o.placed {
        b.object(
            "Axis",
            &typed(ObjectSpec {
                rotmode: -1,
                axis_angle: ([0.0, 0.0, 1.0], 0.5),
                loc: [2.0, 0.0, 0.0],
                ..ObjectSpec::default()
            }),
        );
        b.object(
            "Nudged",
            &euler(ObjectSpec {
                delta_loc: [1.0, 0.0, 0.0],
                ..ObjectSpec::default()
            }),
        );
        b.object(
            "Boned",
            &euler(ObjectSpec {
                parent: Some(cube.into()),
                partype: 4,
                ..ObjectSpec::default()
            }),
        );
    }
    b.action();
    b.camera("Cam", 50.0);
    b.light("Lamp", 10.0);
    b.mesh("Cube", &[Some("Blue".into())], 8);
    b.mesh("Ball", &[], 4);
    b.material("Red", [1.0, 0.0, 0.0], None);
    b.material("Blue", [0.0, 0.0, 1.0], None);
    b.world("World");
    b.bytes()
}

/// The outliner scene with its defaults.
pub fn scene() -> Vec<u8> {
    outliner(&Opts::default())
}

/// Objects whose visibility bits are animated, at 4.03: `Blinking` (the action `AC:Walk`, which
/// animates `hide_viewport`), `Ghost` (a driver of `hide_render`) and `Lost` (animation data
/// naming NLA tracks the file doesn't hold: every bit counts as animated); `Mover` (a driver of
/// its location x, its transform only) and `Plain` (nothing animated).
pub fn blinking_scene() -> Vec<u8> {
    let mut b = SceneBuilder::animated(403);
    b.scene(
        "Scene",
        &["Blinking", "Ghost", "Lost", "Mover", "Plain"],
        &[],
        None,
    );
    for (name, x, driven) in [
        ("Blinking", 1.0, Driven::Action),
        ("Ghost", 2.0, Driven::Driver(("hide_render", 0))),
        ("Lost", 3.0, Driven::Dangling),
        ("Mover", 4.0, Driven::Driver(("location", 0))),
        ("Plain", 5.0, Driven::None),
    ] {
        b.object(
            name,
            &euler(ObjectSpec {
                loc: [x, 0.0, 0.0],
                driven,
                ..ObjectSpec::default()
            }),
        );
    }
    b.action_with(&[(1, &[("hide_viewport", 0)])]);
    b.bytes()
}

/// A file with the placement fields of one scene per `(unit.system, unit.scale_length)`, each
/// holding one object.
pub fn units_scene(scenes: &[(u8, f32)]) -> Vec<u8> {
    let mut b = SceneBuilder::placed(403);
    b.ui();
    let names: Vec<(String, String)> = (0..scenes.len())
        .map(|i| (format!("Scene{i}"), format!("Thing{i}")))
        .collect();
    for ((scene, thing), (system, scale)) in names.iter().zip(scenes) {
        b.unit_system = *system;
        b.unit_scale = *scale;
        b.scene(scene, &[thing.as_str()], &[], None);
    }
    for (_, thing) in &names {
        b.object(
            thing,
            &ObjectSpec {
                rotmode: 1,
                ..ObjectSpec::default()
            },
        );
    }
    b.bytes()
}

/// A scene of turned objects with the placement fields, all in the scene's own collection:
/// `Z90`, a quarter turn about Z; `XZ90`, a quarter turn about X and one about Z, Euler `xyz`;
/// `Mode1` to `Mode6`, turned by 0.1, 0.2 and 0.3 in each Euler mode (`rotmode` 1 to 6).
pub fn turned_scene() -> Vec<u8> {
    let mut b = SceneBuilder::placed(403);
    let ms: Vec<String> = (1..=6).map(|n| format!("Mode{n}")).collect();
    let mut names: Vec<&str> = vec!["Z90", "XZ90"];
    names.extend(ms.iter().map(String::as_str));
    b.ui();
    b.scene("Scene", &names, &[], None);
    let q = std::f32::consts::FRAC_PI_2;
    for (name, rot) in [("Z90", [0.0, 0.0, q]), ("XZ90", [q, 0.0, q])] {
        b.object(
            name,
            &ObjectSpec {
                rotmode: 1,
                rot,
                ..ObjectSpec::default()
            },
        );
    }
    for (i, n) in ms.iter().enumerate() {
        b.object(
            n,
            &ObjectSpec {
                rotmode: i as i16 + 1,
                rot: [0.1, 0.2, 0.3],
                ..ObjectSpec::default()
            },
        );
    }
    b.bytes()
}

/// A scene of `colls` collections of `per` objects each (meshes sharing one mesh, Euler, at
/// distinct places), for the measurements; version 5.01 in the newer header when `large`.
pub fn big_scene(colls: usize, per: usize, large: bool) -> Vec<u8> {
    let mut b = if large {
        SceneBuilder::new(k2_mpc_blend::HeaderForm::Large, 8, false, 501, false)
    } else {
        SceneBuilder::modern()
    };
    let names: Vec<Vec<String>> = (0..colls)
        .map(|c| (0..per).map(|i| format!("Obj_{c}_{i}")).collect())
        .collect();
    let cnames: Vec<String> = (0..colls).map(|c| format!("Coll_{c}")).collect();
    let crefs: Vec<&str> = cnames.iter().map(String::as_str).collect();
    b.ui();
    b.scene("Scene", &[], &crefs, None);
    for (c, ns) in names.iter().enumerate() {
        let refs: Vec<&str> = ns.iter().map(String::as_str).collect();
        b.collection(&cnames[c], &refs, &[]);
    }
    for (c, ns) in names.iter().enumerate() {
        for (i, n) in ns.iter().enumerate() {
            b.object(
                n,
                &euler(ObjectSpec {
                    data: Some("ME:Shared".into()),
                    loc: [c as f32, i as f32 * 0.5, 0.125],
                    rot: [0.0, 0.0, 0.1 * i as f32],
                    ..ObjectSpec::default()
                }),
            );
        }
    }
    b.mesh("Shared", &[], 8);
    b.bytes()
}

/// A scene of one object whose mesh carries a data block of `target` bytes that don't compress
/// (a fixed pseudo-random stream), stored as `how` (`plain`, `gzip`, or `zstd` in frames of the
/// writer's size, compressed as the writer compresses): for the storage measurement.
pub fn sized_scene(target: usize, how: &str) -> Vec<u8> {
    let mut b = SceneBuilder::modern();
    b.ui();
    b.scene("Scene", &["Mover"], &[], None);
    b.object(
        "Mover",
        &euler(ObjectSpec {
            data: Some("ME:Heavy".into()),
            ..ObjectSpec::default()
        }),
    );
    b.mesh("Heavy", &[], 1);
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let noise: Vec<u8> = (0..target)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect();
    let at = b.addr("ME:Heavy/noise");
    b.file.raw(b"DATA", 0, at, 1, noise);
    let raw = b.bytes();
    match how {
        "gzip" => k2_mpc_blend::testgen::gzip(&raw),
        "zstd" => {
            let mut out = Vec::new();
            for chunk in raw.chunks(k2_mpc_blend::write::ZSTD_FRAME) {
                out.extend(ruzstd::encoding::compress_to_vec(
                    chunk,
                    ruzstd::encoding::CompressionLevel::Fastest,
                ));
            }
            out
        }
        _ => raw,
    }
}
