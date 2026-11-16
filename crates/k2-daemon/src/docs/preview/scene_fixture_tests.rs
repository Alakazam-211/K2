//! The live-scene preview fixture (`tests/fixtures/scene-preview/` at the
//! repo root): a living room's K2SCENE1, three of its views drawn through
//! the host's own path ([`scene::render_k2scene_with`], [`service::store`])
//! at the thumb2x width, transparent, with their id images, and the JSON the
//! route answers for the listing and for each image ([`route::listing`],
//! [`route::image`]). The client's tests read them to check that an object's
//! box (its listed `bounds` through its transform, projected with the
//! image's `camera`) lines up with the object drawn, and to pick with the id
//! image.
//!
//! [`the_fixture_is_what_the_host_draws_and_answers`] makes every file again
//! in memory and asserts each is byte for byte the one committed; with
//! `K2_REGEN_FIXTURES=1` it writes them instead. The renderer gives the same
//! bytes on every CPU (no libm on its path), and so does this scene: its
//! transforms are composed here with `+ - * /` only ([`sin_cos`]).
//!
//! [`the_fixture_boxes_line_up_with_the_id_images`] reads the committed
//! files alone and does the client's check: every pixel the id image gives
//! an object falls inside that object's projected box.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use k2_mpc_base::blob::BlobStore;
use k2_mpc_base::store::DocKey;
use k2_scenerender::math::{m4_mul, m4_to_f32, M4};
use k2_scenerender::proto::BINARY_NAME;
use k2_scenerender::scene::{decode, encode, Camera, InputLimits, Mesh, Object, Scene, SensorFit, UpAxis};
use k2_scenerender::testgen::boxes;
use serde_json::{json, Value};

use super::service::{self, Keyed, Snapshot};
use super::*;
use crate::test_support::{with_temp_home, Scratch};

/// Set to rewrite the fixture's files instead of checking them.
const ENV_REGEN: &str = "K2_REGEN_FIXTURES";

/// The views drawn, and the stem of each one's files.
const VIEWS: [(&str, &str); 3] = [("@scene", "scene"), ("@cam:Camera", "cam-Camera"), ("@obj:Sofa", "obj-Sofa")];

/// The key the fixture's render hashes are keyed with (a host keys them
/// with the space's own host cache key, which is random).
const HASH_KEY: [u8; 32] = [0x2a; 32];

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/scene-preview")
}

// ── The scene ────────────────────────────────────────────────────────

/// `sin x` and `cos x` with `+ - * /` only, in a fixed order, so the
/// transforms are the same bits on every CPU (the platform's `sin` may
/// differ in its last bit): the angle quartered, the Taylor series of the
/// quarter (terms to x^17 and x^18), then the double-angle formulas twice,
/// as `k2_scenerender::trig` does for its `tan`. |x| <= pi here.
fn sin_cos(x: f64) -> (f64, f64) {
    let y = x * 0.25;
    let y2 = y * y;
    let mut s = y * (1.0 + y2 * (-1.0 / 6.0 + y2 * (1.0 / 120.0 + y2 * (-1.0 / 5_040.0 + y2 * (1.0 / 362_880.0 + y2 * (-1.0 / 39_916_800.0 + y2 * (1.0 / 6_227_020_800.0 + y2 * (-1.0 / 1_307_674_368_000.0 + y2 * (1.0 / 355_687_428_096_000.0)))))))));
    let mut c = 1.0 + y2 * (-0.5 + y2 * (1.0 / 24.0 + y2 * (-1.0 / 720.0 + y2 * (1.0 / 40_320.0 + y2 * (-1.0 / 3_628_800.0 + y2 * (1.0 / 479_001_600.0 + y2 * (-1.0 / 87_178_291_200.0 + y2 * (1.0 / 20_922_789_888_000.0 + y2 * (-1.0 / 6_402_373_705_728_000.0)))))))));
    for _ in 0..2 {
        let (s2, c2) = (2.0 * s * c, c * c - s * s);
        s = s2;
        c = c2;
    }
    (s, c)
}

/// A rotation as the file stores it.
#[derive(Clone, Copy)]
enum Rot {
    /// Euler angles in radians, XYZ order (X applied first: Rz * Ry * Rx).
    Xyz([f64; 3]),
    /// `[w, x, y, z]`.
    Quat([f64; 4]),
}

const NO_ROT: Rot = Rot::Xyz([0.0, 0.0, 0.0]);

/// Location, rotation and scale as one column-major matrix: T * R * S.
fn trs(loc: [f64; 3], rot: Rot, scale: [f64; 3]) -> M4 {
    let r: [[f64; 3]; 3] = match rot {
        Rot::Xyz([a, b, c]) => {
            let ((sa, ca), (sb, cb), (sc, cc)) = (sin_cos(a), sin_cos(b), sin_cos(c));
            // Rows of Rz(c) * Ry(b) * Rx(a).
            [
                [cc * cb, cc * sb * sa - sc * ca, cc * sb * ca + sc * sa],
                [sc * cb, sc * sb * sa + cc * ca, sc * sb * ca - cc * sa],
                [-sb, cb * sa, cb * ca],
            ]
        }
        Rot::Quat([w, x, y, z]) => [
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
            [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x)],
            [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y)],
        ],
    };
    let mut m = [0.0; 16];
    for col in 0..3 {
        for row in 0..3 {
            m[col * 4 + row] = r[row][col] * scale[col];
        }
    }
    m[12] = loc[0];
    m[13] = loc[1];
    m[14] = loc[2];
    m[15] = 1.0;
    m
}

/// One unit of the room's snapshot that the scene needs: a drawn mesh
/// object, or an empty a mesh object is parented to (not drawn).
struct Unit {
    name: &'static str,
    /// The stand-in mesh's name; `None` for an empty.
    mesh: Option<&'static str>,
    /// The parent's name: world = parent's world * parent inverse (identity
    /// in the snapshot) * its own.
    parent: Option<&'static str>,
    loc: [f64; 3],
    rot: Rot,
    scale: [f64; 3],
}

const fn unit(name: &'static str, mesh: Option<&'static str>, parent: Option<&'static str>, loc: [f64; 3], rot: Rot, scale: [f64; 3]) -> Unit {
    Unit { name, mesh, parent, loc, rot, scale }
}

const ONE: [f64; 3] = [1.0, 1.0, 1.0];

/// The living room's units, as its snapshot places them (in its order). The
/// lights and the empties nothing is parented to are left out (nothing to
/// draw), and so is the hidden left wall.
const ROOM: &[Unit] = &[
    unit("Floor", Some("Plane"), None, [0.0, 0.0, 0.0], NO_ROT, [3.0, 2.5, 1.0]),
    unit("Wall.Back", Some("Wall"), None, [0.0, 2.5, 0.6], Rot::Xyz([1.5707964, 0.0, 0.0]), [3.0, 0.6, 1.0]),
    unit("Window", Some("Window"), None, [1.2, 2.46, 0.72], Rot::Xyz([1.5707964, 0.0, 0.0]), [0.7, 0.3, 1.0]),
    unit("Rug", Some("Rug"), None, [0.2, -0.3, 0.005], NO_ROT, [1.6, 1.1, 1.0]),
    unit("Ceiling fan", Some("Fan"), None, [0.0, 0.0, 2.75], Rot::Quat([0.9238795, 0.0, 0.0, 0.38268343]), ONE),
    unit("Table", Some("Table"), None, [0.6, 0.4, 0.0], NO_ROT, ONE),
    unit("Chair.001", Some("Chair"), None, [0.6, -0.35, 0.0], NO_ROT, ONE),
    unit("Chair.002", Some("Chair"), None, [0.6, 1.15, 0.0], Rot::Xyz([0.0, 0.0, 3.1415927]), ONE),
    unit("Chair.003", Some("Chair"), None, [-0.15, 0.4, 0.0], Rot::Xyz([0.0, 0.0, -1.5707964]), ONE),
    unit("Chair.004", Some("Chair"), None, [1.35, 0.4, 0.0], Rot::Xyz([0.0, 0.0, 1.5707964]), ONE),
    unit("Sofa", Some("Sofa"), None, [-1.9, -1.2, 0.0], Rot::Xyz([0.0, 0.0, 1.5707964]), ONE),
    unit("Shelf", Some("Shelf"), None, [2.6, 2.2, 0.0], NO_ROT, ONE),
    unit("Book.001", Some("Book"), Some("Shelf"), [-0.3, 0.0, 1.12], NO_ROT, ONE),
    unit("Book.002", Some("Book"), Some("Shelf"), [-0.05, 0.0, 1.12], Rot::Xyz([0.0, 0.17453292, 0.0]), ONE),
    unit("Floor lamp", Some("Lamp"), None, [-2.55, 1.95, 0.0], NO_ROT, ONE),
    unit("Vase", Some("Vase"), Some("Table"), [0.0, 0.0, 0.76], NO_ROT, ONE),
    unit("Mug", Some("Mug"), None, [2.75, 2.15, 0.62], NO_ROT, ONE),
    unit("Clock", None, None, [-0.6, 2.44, 0.86], Rot::Xyz([1.5707964, 0.0, 0.0]), ONE),
    unit("Clock hand", Some("Hand"), Some("Clock"), [0.0, 0.0, 0.01], Rot::Xyz([0.0, 0.0, 0.5934119]), ONE),
];

/// Stand-in meshes (the snapshot has no geometry): boxes in mesh-local
/// metres, each mesh once, shared by every object that uses it.
fn stand_in(name: &str) -> Vec<([f32; 3], [f32; 3])> {
    let leg = |x: f32, y: f32, half: f32, top: f32| ([x - half, y - half, 0.0], [x + half, y + half, top]);
    match name {
        // Blender's 2 x 2 plane, as a slab under its face.
        "Plane" => vec![([-1.0, -1.0, -0.02], [1.0, 1.0, 0.0])],
        // A plane stood up by its object's rotation: the slab behind its face.
        "Wall" => vec![([-1.0, -1.0, -0.05], [1.0, 1.0, 0.0])],
        "Window" => vec![([-1.0, -1.0, 0.0], [1.0, 1.0, 0.02])],
        "Rug" => vec![([-1.0, -1.0, 0.0], [1.0, 1.0, 0.01])],
        "Fan" => vec![
            ([-0.08, -0.08, -0.1], [0.08, 0.08, 0.05]),
            ([-0.015, -0.015, 0.05], [0.015, 0.015, 0.25]),
            ([-0.65, -0.07, -0.06], [0.65, 0.07, -0.04]),
            ([-0.07, -0.65, -0.06], [0.07, 0.65, -0.04]),
        ],
        "Table" => vec![
            ([-0.5, -0.35, 0.72], [0.5, 0.35, 0.76]),
            leg(-0.45, -0.3, 0.02, 0.72),
            leg(0.45, -0.3, 0.02, 0.72),
            leg(-0.45, 0.3, 0.02, 0.72),
            leg(0.45, 0.3, 0.02, 0.72),
        ],
        // The back on the chair's -Y side.
        "Chair" => vec![
            ([-0.22, -0.22, 0.42], [0.22, 0.22, 0.46]),
            ([-0.22, -0.22, 0.46], [0.22, -0.18, 0.92]),
            leg(-0.19, -0.19, 0.015, 0.42),
            leg(0.19, -0.19, 0.015, 0.42),
            leg(-0.19, 0.19, 0.015, 0.42),
            leg(0.19, 0.19, 0.015, 0.42),
        ],
        // The back on the sofa's +Y side, arms at its ends.
        "Sofa" => vec![
            ([-0.9, -0.4, 0.0], [0.9, 0.4, 0.42]),
            ([-0.9, 0.25, 0.42], [0.9, 0.4, 0.85]),
            ([-0.9, -0.4, 0.42], [-0.75, 0.25, 0.62]),
            ([0.75, -0.4, 0.42], [0.9, 0.25, 0.62]),
        ],
        "Shelf" => vec![
            ([-0.5, -0.15, 0.0], [-0.47, 0.15, 1.12]),
            ([0.47, -0.15, 0.0], [0.5, 0.15, 1.12]),
            ([-0.47, -0.15, 0.0], [0.47, 0.15, 0.03]),
            ([-0.47, -0.15, 0.57], [0.47, 0.15, 0.6]),
            ([-0.47, -0.15, 1.09], [0.47, 0.15, 1.12]),
            ([-0.47, 0.13, 0.03], [0.47, 0.15, 1.09]),
        ],
        "Book" => vec![([-0.02, -0.1, 0.0], [0.02, 0.1, 0.24])],
        "Lamp" => vec![
            ([-0.15, -0.15, 0.0], [0.15, 0.15, 0.03]),
            ([-0.015, -0.015, 0.03], [0.015, 0.015, 1.45]),
            ([-0.2, -0.2, 1.45], [0.2, 0.2, 1.75]),
        ],
        "Vase" => vec![([-0.07, -0.07, 0.0], [0.07, 0.07, 0.28])],
        "Mug" => vec![([-0.04, -0.04, -0.02], [0.04, 0.04, 0.08])],
        "Hand" => vec![([-0.01, 0.0, -0.005], [0.01, 0.15, 0.005])],
        other => panic!("no stand-in for {other}"),
    }
}

/// A camera as the file places it: the camera looks down its local -Z with
/// local +Y up; 50 mm on a 36 x 24 mm sensor, fitted automatically, at
/// 16:9 (yfov = 2 atan(18 / 50 / (16 / 9)) = 0.3996 rad).
fn camera(id: &str, loc: [f64; 3], angles: [f64; 3], active: bool) -> Camera {
    let m = trs(loc, Rot::Xyz(angles), ONE);
    Camera {
        id: id.into(),
        position: [loc[0] as f32, loc[1] as f32, loc[2] as f32],
        target: None,
        forward: Some([-m[8] as f32, -m[9] as f32, -m[10] as f32]),
        up: [m[4] as f32, m[5] as f32, m[6] as f32],
        yfov: 0.3996,
        aspect: Some(16.0 / 9.0),
        znear: Some(0.1),
        active: Some(active),
        lens_mm: Some(50.0),
        sensor_mm: Some([36.0, 24.0]),
        sensor_fit: Some(SensorFit::Auto),
    }
}

/// The living room as K2SCENE1 would carry it: Z up, an object per drawn
/// unit named as the file names it, its world transform composed through
/// its parents, stand-in meshes, the sofa drawn without its subdivision
/// modifier, and the file's two cameras.
fn room() -> Scene {
    let mut world: BTreeMap<&str, M4> = BTreeMap::new();
    let mut mesh_of: Vec<&str> = Vec::new();
    let mut meshes: Vec<Mesh> = Vec::new();
    let mut objects: Vec<Object> = Vec::new();
    for u in ROOM {
        let own = trs(u.loc, u.rot, u.scale);
        let w = match u.parent {
            Some(p) => m4_mul(world.get(p).unwrap_or_else(|| panic!("{} before its parent {p}", u.name)), &own),
            None => own,
        };
        world.insert(u.name, w);
        let Some(mesh) = u.mesh else { continue };
        let index = match mesh_of.iter().position(|m| *m == mesh) {
            Some(i) => i,
            None => {
                mesh_of.push(mesh);
                meshes.push(boxes(&stand_in(mesh)));
                mesh_of.len() - 1
            }
        };
        let sofa = u.name == "Sofa";
        objects.push(Object {
            id: u.name.into(),
            mesh: index as u32,
            base_color: [0.8, 0.8, 0.8, 1.0],
            double_sided: Some(false),
            transform: m4_to_f32(&w),
            hash: None,
            modifiers: if sofa { vec!["SUBSURF".into()] } else { Vec::new() },
            modifiers_not_applied: sofa,
        });
    }
    let cameras = vec![
        camera("Camera", [-2.6, -3.4, 1.35], [1.4137167, 0.0, -0.62831855], true),
        camera("Camera.001", [1.75, -0.75, 1.3], [1.2915436, 0.0, 0.7679449], false),
    ];
    Scene { up: UpAxis::Z, meshes, objects, cameras }
}

// ── Drawing it through the host ──────────────────────────────────────

fn body(r: crate::cli_response::CliResponse) -> Value {
    assert_eq!(r.status, "200 OK", "{}", r.body);
    serde_json::from_str(&r.body).expect("a JSON answer")
}

fn b64(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(v["base64"].as_str().expect("base64")).expect("decodes")
}

fn pretty(v: &Value) -> Vec<u8> {
    let mut s = serde_json::to_string_pretty(v).expect("json");
    s.push('\n');
    s.into_bytes()
}

/// Every file of the fixture, made now: the scene, the route's listing,
/// and per view its still, its id image and the route's answer for the
/// still (its `base64` left out: the still is its own file).
fn make() -> BTreeMap<String, Vec<u8>> {
    let bin = k2_core::test_env::workspace_bin(scene::ENV_BIN, "k2-scenerender", BINARY_NAME);
    let bytes = encode(&room());
    let mut files = BTreeMap::new();
    with_temp_home(|| {
        let s = Scratch::new();
        let docs = Arc::new(k2_mpc_base::store::DocStore::open(&s.path().join("docs.redb")).expect("store"));
        let blobs = Arc::new(BlobStore::open(docs, &s.path().join("blobs"), k2_mpc_base::blob::BlobConfig::default(), Arc::new(|| 1_000)).expect("blobs"));
        let l = scene::list_k2scene(&bytes).expect("a listing");
        let snap = Arc::new(Snapshot {
            key: DocKey { space: "scene-preview".into(), path: "scenes/living-room.blend".into() },
            rev: 1,
            version: None,
            renderer: &scene::SCENE,
            build: scene::SCENE.build(),
            page: l.page,
            containers: l.containers.into_iter().map(|c| Keyed { hash: service::keyed_hash(&HASH_KEY, &c.digest), c }).collect(),
            bytes: Arc::new(bytes.clone()),
        });
        files.insert("living-room.k2scene".to_string(), bytes.clone());
        files.insert("listing.json".to_string(), pretty(&body(route::listing(&blobs, &snap, None, Look::Transparent, Shape::Wide))));
        for (c, stem) in VIEWS {
            let k = snap.container(c).unwrap_or_else(|| panic!("no {c}"));
            let mut img = scene::render_k2scene_with(&bin, &bytes, &k.c, Width::Thumb2x, Look::Transparent, Shape::Wide, Duration::from_secs(60)).unwrap_or_else(|f| panic!("{c}: {f:?}"));
            // Its timings are fixed, so the answers are the same every time.
            img.render_ms = 0;
            service::store(&blobs, &snap, c, &k.hash, Width::Thumb2x, Look::Transparent, Shape::Wide, &img, 0).expect("cached");
            let mut a = body(route::image(&blobs, &snap, c, None, Width::Thumb2x, Look::Transparent, Shape::Wide, false));
            assert_eq!((a["state"].clone(), b64(&a)), (json!("ready"), img.bytes.clone()), "{c}: the route serves the still drawn");
            let ids = b64(&body(route::image(&blobs, &snap, c, None, Width::Thumb2x, Look::Transparent, Shape::Wide, true)));
            assert_eq!(Some(&ids), img.ids.as_ref(), "{c}: and its id image");
            a.as_object_mut().expect("an object").remove("base64");
            files.insert(format!("{stem}.webp"), img.bytes);
            files.insert(format!("{stem}.ids.png"), ids);
            files.insert(format!("{stem}.json"), pretty(&a));
        }
    });
    files
}

#[test]
fn the_fixture_is_what_the_host_draws_and_answers() {
    let fresh = make();
    let dir = fixture_dir();
    if std::env::var_os(ENV_REGEN).is_some_and(|v| !v.is_empty()) {
        std::fs::create_dir_all(&dir).expect("the fixture's folder");
        for (name, bytes) in &fresh {
            std::fs::write(dir.join(name), bytes).expect("write a fixture file");
            eprintln!("[scene-preview] wrote {name} ({} bytes)", bytes.len());
        }
        return;
    }
    let on_disk: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}; run with {ENV_REGEN}=1", dir.display()))
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "README.md")
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(on_disk, fresh.keys().cloned().collect::<Vec<_>>(), "the fixture's files changed; run with {ENV_REGEN}=1");
    for (name, bytes) in &fresh {
        let have = std::fs::read(dir.join(name)).expect("read a fixture file");
        assert!(have == *bytes, "{name} drifted from what the host draws and answers ({} bytes now, {} committed); run with {ENV_REGEN}=1", bytes.len(), have.len());
    }
}

/// The scene's object transforms are composed the same every time, and its
/// parents are followed: a book sits on the shelf, the vase on the table.
#[test]
fn the_rooms_parents_place_their_children() {
    let s = room();
    assert_eq!(encode(&s), encode(&room()));
    let at = |id: &str| s.objects.iter().find(|o| o.id == id).unwrap_or_else(|| panic!("no {id}")).transform;
    let (book, vase, hand) = (at("Book.001"), at("Vase"), at("Clock hand"));
    let near = |m: [f32; 16], at: [f32; 3]| (0..3).all(|i| (m[12 + i] - at[i]).abs() < 1e-5);
    assert!(near(book, [2.3, 2.2, 1.12]), "on the shelf: {book:?}");
    assert!(near(vase, [0.6, 0.4, 0.76]), "on the table: {vase:?}");
    // The hand on the clock's face: the clock stood up by its rotation, the hand 1 cm in front of it.
    assert!(near(hand, [-0.6, 2.43, 0.86]), "{hand:?}");
    let (s90, c90) = sin_cos(std::f64::consts::FRAC_PI_2);
    assert!((s90 - 1.0).abs() < 1e-12 && c90.abs() < 1e-12, "{s90} {c90}");
    assert!(s.objects.iter().all(|o| o.id != "Clock" && o.id != "Wall.Left"), "an empty and a hidden object aren't drawn");
    assert_eq!(s.objects.iter().filter(|o| o.modifiers_not_applied).map(|o| o.id.as_str()).collect::<Vec<_>>(), ["Sofa"]);
}

// ── The client's check, on the committed files ───────────────────────

fn read_json(name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(fixture_dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))).expect("json")
}

fn floats<const N: usize>(v: &Value) -> [f64; N] {
    let a = v.as_array().expect("numbers");
    assert_eq!(a.len(), N);
    std::array::from_fn(|i| a[i].as_f64().expect("a number"))
}

/// Column-major 4x4 times `[x, y, z, 1]`.
fn mul4(m: &[f64; 16], p: [f64; 3]) -> [f64; 4] {
    std::array::from_fn(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r])
}

/// Slack around a projected box, in pixels: a pixel is given an object
/// when one of its samples is covered, anywhere in it.
const SLACK: f64 = 2.0;

/// For each view, each object in its id table: its listed box's eight
/// corners through its transform (from the scene's header) and the image's
/// `view` and `proj`, to pixels (the camera's documented mapping); every
/// pixel the id image gives that object falls inside that box's 2D bounds.
#[test]
fn the_fixture_boxes_line_up_with_the_id_images() {
    let scene = decode(&std::fs::read(fixture_dir().join("living-room.k2scene")).expect("the scene"), &InputLimits::default()).expect("a K2SCENE1");
    let listing = read_json("listing.json");
    let bounds = |id: &str| -> ([f64; 3], [f64; 3]) {
        let key = format!("@obj:{id}");
        let c = listing["containers"].as_array().expect("containers").iter().find(|c| c["key"] == key.as_str()).unwrap_or_else(|| panic!("{key} not listed"));
        (floats(&c["bounds"]["min"]), floats(&c["bounds"]["max"]))
    };
    for (c, stem) in VIEWS {
        let meta = read_json(&format!("{stem}.json"));
        let (w, h) = (meta["size"]["w"].as_u64().expect("w") as usize, meta["size"]["h"].as_u64().expect("h") as usize);
        let (view, proj): ([f64; 16], [f64; 16]) = (floats(&meta["camera"]["view"]), floats(&meta["camera"]["proj"]));
        let near = meta["camera"]["near"].as_f64().expect("near");
        let png = std::fs::read(fixture_dir().join(format!("{stem}.ids.png"))).expect("the id image");
        let (iw, ih, px) = k2_scenerender::encode::ids_from_png(&png).expect("an id image");
        assert_eq!((iw as usize, ih as usize), (w, h), "{c}: at the still's size");
        let mut checked = 0;
        for e in meta["ids"]["objects"].as_array().expect("an id table") {
            let (index, id) = (e["index"].as_u64().expect("index") as usize, e["id"].as_str().expect("id"));
            let o = &scene.objects[index];
            assert_eq!(o.id, id, "{c}: the table names the scene's object");
            let t: [f64; 16] = std::array::from_fn(|i| o.transform[i] as f64);
            let (lo, hi) = bounds(id);
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            let mut in_front = true;
            for k in 0..8 {
                let p = [if k & 1 == 0 { lo[0] } else { hi[0] }, if k & 2 == 0 { lo[1] } else { hi[1] }, if k & 4 == 0 { lo[2] } else { hi[2] }];
                let wp = mul4(&t, p);
                let v = mul4(&view, [wp[0], wp[1], wp[2]]);
                let clip = mul4(&proj, [v[0], v[1], v[2]]);
                if clip[3] < near {
                    in_front = false;
                    break;
                }
                let (nx, ny) = (clip[0] / clip[3], clip[1] / clip[3]);
                let (x, y) = ((nx * 0.5 + 0.5) * w as f64, (0.5 - ny * 0.5) * h as f64);
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
            let mine: Vec<(usize, usize)> = (0..w * h).filter(|&i| px[i] == index as u32 + 1).map(|i| (i % w, i / w)).collect();
            if !in_front || mine.is_empty() {
                // A box partly behind the eye has no 2D bounds; an object hidden behind others has no pixels.
                continue;
            }
            for &(i, j) in &mine {
                let (x, y) = (i as f64 + 0.5, j as f64 + 0.5);
                assert!(
                    x >= x0 - SLACK && x <= x1 + SLACK && y >= y0 - SLACK && y <= y1 + SLACK,
                    "{c}: {id}'s pixel ({i}, {j}) is outside its projected box ({x0:.1}, {y0:.1})-({x1:.1}, {y1:.1})"
                );
            }
            checked += 1;
        }
        let want = match c {
            "@scene" => 12,
            "@cam:Camera" => 6,
            _ => 1,
        };
        assert!(checked >= want, "{c}: only {checked} objects checked");
    }
}
