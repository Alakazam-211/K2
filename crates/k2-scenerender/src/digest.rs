//! What a scene previews, and the raw digest of what each draws: the host's
//! listing (V3's `PreviewRenderer::list`), from the mesh read alone.
//!
//! **Containers**, in this order: `@scene` (the auto 3/4 view of every
//! object), one per camera (`@cam:<id>`, through that camera), one per object
//! (`@obj:<id>`, the auto view of that object alone), then the turntable's
//! eight frames of every object (`@turntable:0` to `@turntable:7`, frame k at
//! azimuth 45 + 45k degrees, [`ViewSpec::Turntable`]). An id that is empty,
//! too long for an address or already used gets the index form (`@cam#2`,
//! `@obj#17`); the turntable's addresses are fixed. So a scene lists
//! 1 + cameras + objects + 8 items.
//!
//! **Digest** (SHA-256, [`crate::sha256`]), over what changes the pixels and
//! nothing else, object and camera ids left out (so a rename doesn't redraw):
//! the up axis; for a camera view the camera's position, target or forward,
//! up, field of view, aspect and near plane; for a turntable frame its
//! number; then for each drawn object, in
//! order, its mesh's digest (over the mesh's counts and its data bytes as
//! read), its transform, its base colour, its sidedness and
//! `modifiers_not_applied` (shown with the image). The renderer's build is
//! not in it: the host keys its cache with it.
//!
//! **Bounds:** each object item also carries its mesh's local box
//! ([`LocalBounds`], mesh space: the client applies the object's transform),
//! read from the positions in the bytes; `approx` when the object's
//! modifiers were not applied (the evaluated mesh may be larger). An empty
//! mesh, or one with a non-finite position (which the renderer refuses), has
//! none. Bounds are not in any digest.
//!
//! **Cameras:** each camera item carries the camera's display values
//! ([`CameraDisplay`]: `active`, `lens_mm`, `sensor_mm`, `sensor_fit`, each
//! left out when the file has none). They draw nothing and are in no digest.

use serde::Serialize;

use crate::camera::{mesh_bounds, ViewSpec, TURNTABLE_FRAMES};
use crate::sha256::Sha256;
use crate::scene::{self, Camera, CameraDisplay, Header, InputError, InputLimits, MeshInfo, Object, UpAxis};

/// Longest id put in an address as is.
pub const MAX_ADDRESS_ID: usize = 128;

/// What a container draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Every object, the auto view.
    Scene,
    /// Every object, through camera i.
    Camera(u32),
    /// Object i alone, the auto view.
    Object(u32),
    /// Every object, turntable frame k (0 to 7).
    Turntable(u8),
}

impl Target {
    /// The opaque locator the host keeps (never on the wire).
    pub fn locator(self) -> String {
        match self {
            Target::Scene => "scene".into(),
            Target::Camera(i) => format!("c{i}"),
            Target::Object(i) => format!("o{i}"),
            Target::Turntable(k) => format!("t{k}"),
        }
    }

    pub fn from_locator(s: &str) -> Option<Target> {
        if s == "scene" {
            return Some(Target::Scene);
        }
        let n = |r: &str| -> Option<u32> { (!r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()) && (r == "0" || !r.starts_with('0'))).then(|| r.parse().ok()).flatten() };
        if let Some(r) = s.strip_prefix('c') {
            return n(r).map(Target::Camera);
        }
        if let Some(r) = s.strip_prefix('t') {
            return n(r).filter(|&k| k < TURNTABLE_FRAMES as u32).map(|k| Target::Turntable(k as u8));
        }
        s.strip_prefix('o').and_then(n).map(Target::Object)
    }

    /// The view that draws it.
    pub fn view(self, width: u32) -> crate::proto::View {
        use crate::proto::View;
        match self {
            Target::Scene => View::new(ViewSpec::Auto, width),
            Target::Camera(i) => View::new(ViewSpec::Camera(i), width),
            Target::Object(i) => View { objects: Some(vec![i]), ..View::new(ViewSpec::Auto, width) },
            Target::Turntable(k) => View::new(ViewSpec::Turntable(k), width),
        }
    }
}

/// An object's mesh box in mesh-local coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct LocalBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// The object is drawn without its modifiers: its evaluated mesh (what
    /// the file shows) may be larger than this base mesh's box.
    pub approx: bool,
}

/// One previewed thing.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// Its address (`@scene`, `@cam:Camera`, `@obj:Cube`, `@turntable:3`).
    pub key: String,
    /// Its id as the file names it (empty for the scene and the turntable).
    pub title: String,
    pub target: Target,
    /// The raw digest of what it draws.
    pub digest: [u8; 32],
    /// An object's mesh box (local; `None` for an empty mesh and for every
    /// other item).
    pub bounds: Option<LocalBounds>,
    /// A camera's display values (`None` for every other item).
    pub camera: Option<CameraDisplay>,
}

fn addressable(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ADDRESS_ID && !id.chars().any(|c| c.is_control())
}

fn f32s(h: &mut Sha256, v: &[f32]) {
    for x in v {
        h.update(&x.to_bits().to_le_bytes());
    }
}

fn opt3(h: &mut Sha256, v: &Option<[f32; 3]>) {
    match v {
        Some(v) => {
            h.update(&[1]);
            f32s(h, v);
        }
        None => h.update(&[0]),
    }
}

fn opt1(h: &mut Sha256, v: Option<f32>) {
    match v {
        Some(v) => {
            h.update(&[1]);
            f32s(h, &[v]);
        }
        None => h.update(&[0]),
    }
}

/// The digest of one mesh: its counts, whether it has normals, and its data bytes.
pub fn mesh_digest(m: &MeshInfo, data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"k2scenerender/mesh/v1\0");
    h.update(&(m.vertices as u64).to_le_bytes());
    h.update(&(m.triangles as u64).to_le_bytes());
    h.update(&[m.normals as u8]);
    h.update(data);
    h.finish()
}

/// What a view looks through.
#[derive(Clone, Copy)]
enum Through<'a> {
    Auto,
    Camera(&'a Camera),
    Turntable(u8),
}

fn view_digest(up: UpAxis, through: Through<'_>, objects: &[&Object], mesh: &[[u8; 32]]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"k2scenerender/view/v1\0");
    h.update(&[match up {
        UpAxis::Y => b'y',
        UpAxis::Z => b'z',
    }]);
    // 0: auto, 1: a camera, 2: a turntable frame (the first two as they always were).
    match through {
        Through::Camera(c) => {
            h.update(&[1]);
            f32s(&mut h, &c.position);
            opt3(&mut h, &c.target);
            opt3(&mut h, &c.forward);
            f32s(&mut h, &c.up);
            f32s(&mut h, &[c.yfov]);
            opt1(&mut h, c.aspect);
            opt1(&mut h, c.znear);
        }
        Through::Turntable(k) => h.update(&[2, k]),
        Through::Auto => h.update(&[0]),
    }
    h.update(&(objects.len() as u64).to_le_bytes());
    for o in objects {
        h.update(&mesh[o.mesh as usize]);
        f32s(&mut h, &o.transform);
        f32s(&mut h, &o.base_color);
        h.update(&[match o.double_sided {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        }]);
        h.update(&[o.modifiers_not_applied as u8]);
    }
    h.finish()
}

/// The containers of a K2SCENE1 held in memory, with their digests. Reads
/// and checks the header ([`scene::layout`]); hashes each mesh's data once,
/// without decoding it.
pub fn list(bytes: &[u8], lim: &InputLimits) -> Result<Vec<Item>, InputError> {
    let (h, ranges) = scene::layout(bytes, lim)?;
    let mesh: Vec<[u8; 32]> = h.meshes.iter().zip(&ranges).map(|(m, r)| mesh_digest(m, &bytes[r.clone()])).collect();
    // A mesh's box once, only for the meshes an object uses.
    let mut boxes: Vec<Option<Option<([f32; 3], [f32; 3])>>> = vec![None; h.meshes.len()];
    for o in &h.objects {
        let mi = o.mesh as usize;
        if boxes[mi].is_none() {
            boxes[mi] = Some(local_box(&h.meshes[mi], &bytes[ranges[mi].clone()]));
        }
    }
    let boxes: Vec<Option<([f32; 3], [f32; 3])>> = boxes.into_iter().map(Option::flatten).collect();
    Ok(list_header(&h, &mesh, &boxes))
}

/// The box of a mesh's positions (the first `12 * vertices` of its data);
/// `None` when it has none or one isn't finite.
fn local_box(m: &MeshInfo, data: &[u8]) -> Option<([f32; 3], [f32; 3])> {
    let n = m.vertices as usize * 12;
    let positions: Vec<[f32; 3]> = data.get(..n)?.as_chunks::<12>().0.iter()
        .map(|c| [f32::from_le_bytes([c[0], c[1], c[2], c[3]]), f32::from_le_bytes([c[4], c[5], c[6], c[7]]), f32::from_le_bytes([c[8], c[9], c[10], c[11]])])
        .collect();
    if !positions.iter().flatten().all(|v| v.is_finite()) {
        return None;
    }
    // f32 in, f32 out: the f64 box holds the f32 values exactly.
    let (lo, hi) = mesh_bounds(&positions)?;
    Some(([lo[0] as f32, lo[1] as f32, lo[2] as f32], [hi[0] as f32, hi[1] as f32, hi[2] as f32]))
}

fn list_header(h: &Header, mesh: &[[u8; 32]], boxes: &[Option<([f32; 3], [f32; 3])>]) -> Vec<Item> {
    let all: Vec<&Object> = h.objects.iter().collect();
    let mut items = Vec::with_capacity(1 + h.cameras.len() + h.objects.len() + TURNTABLE_FRAMES as usize);
    items.push(Item { key: "@scene".into(), title: String::new(), target: Target::Scene, digest: view_digest(h.up, Through::Auto, &all, mesh), bounds: None, camera: None });
    let mut used = std::collections::HashSet::new();
    for (i, c) in h.cameras.iter().enumerate() {
        let key = if addressable(&c.id) && used.insert(format!("@cam:{}", c.id)) { format!("@cam:{}", c.id) } else { format!("@cam#{i}") };
        items.push(Item { key, title: c.id.clone(), target: Target::Camera(i as u32), digest: view_digest(h.up, Through::Camera(c), &all, mesh), bounds: None, camera: Some(c.display()) });
    }
    for (i, o) in h.objects.iter().enumerate() {
        let key = if addressable(&o.id) && used.insert(format!("@obj:{}", o.id)) { format!("@obj:{}", o.id) } else { format!("@obj#{i}") };
        let bounds = boxes[o.mesh as usize].map(|(min, max)| LocalBounds { min, max, approx: o.modifiers_not_applied });
        items.push(Item { key, title: o.id.clone(), target: Target::Object(i as u32), digest: view_digest(h.up, Through::Auto, &[o], mesh), bounds, camera: None });
    }
    for k in 0..TURNTABLE_FRAMES {
        let key = format!("@turntable:{k}");
        used.insert(key.clone());
        items.push(Item { key, title: String::new(), target: Target::Turntable(k), digest: view_digest(h.up, Through::Turntable(k), &all, mesh), bounds: None, camera: None });
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::encode;
    use crate::testgen;

    fn digests(s: &crate::scene::Scene) -> Vec<(String, [u8; 32])> {
        list(&encode(s), &InputLimits::default()).expect("list").into_iter().map(|i| (i.key, i.digest)).collect()
    }

    #[test]
    fn a_scene_lists_itself_its_cameras_and_its_objects() {
        let s = testgen::camera_scene();
        let items = list(&encode(&s), &InputLimits::default()).expect("list");
        let keys: Vec<&str> = items.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, ["@scene", "@cam:Camera", "@obj:Cube", "@turntable:0", "@turntable:1", "@turntable:2", "@turntable:3", "@turntable:4", "@turntable:5", "@turntable:6", "@turntable:7"]);
        let mut targets = vec![Target::Scene, Target::Camera(0), Target::Object(0)];
        targets.extend((0..8).map(Target::Turntable));
        assert_eq!(items.iter().map(|i| i.target).collect::<Vec<_>>(), targets);
        for t in [Target::Scene, Target::Camera(7), Target::Object(12), Target::Turntable(0), Target::Turntable(7)] {
            assert_eq!(Target::from_locator(&t.locator()), Some(t));
        }
        assert_eq!(Target::Turntable(3).locator(), "t3");
        assert_eq!(Target::Turntable(5).view(64), crate::proto::View::new(ViewSpec::Turntable(5), 64));
        for bad in ["", "c", "o-1", "c01", "x3", "o1x", "scene2", "t", "t8", "t03", "t-1", "t300"] {
            assert_eq!(Target::from_locator(bad), None, "{bad}");
        }
        // An object or camera named like a turntable frame keeps its own address.
        let mut n = testgen::camera_scene();
        n.objects[0].id = "@turntable:0".into();
        n.cameras[0].id = "turntable:1".into();
        let keys: Vec<String> = digests(&n).into_iter().map(|x| x.0).collect();
        assert_eq!(keys[..4], ["@scene".to_string(), "@cam:turntable:1".into(), "@obj:@turntable:0".into(), "@turntable:0".into()]);
        assert_eq!(keys.len(), 11);
    }

    #[test]
    fn the_turntables_frames_each_have_their_own_digest() {
        let s = testgen::grid_scene(2, false);
        let d = digests(&s);
        let frames: Vec<[u8; 32]> = d[d.len() - 8..].iter().map(|x| x.1).collect();
        let distinct: std::collections::BTreeSet<[u8; 32]> = frames.iter().copied().collect();
        assert_eq!(distinct.len(), 8, "eight frames, eight digests");
        assert!(!frames.contains(&d[0].1), "and none is the scene's");
        // Over what they draw: every object, so any object's edit redraws them all.
        let mut m = s.clone();
        m.objects[1].transform[13] += 0.5;
        let dm = digests(&m);
        assert!(d[d.len() - 8..].iter().zip(&dm[dm.len() - 8..]).all(|(a, b)| a.1 != b.1));
    }

    #[test]
    fn ids_dont_change_digests_and_duplicates_get_index_addresses() {
        let a = testgen::grid_scene(2, false);
        let mut b = a.clone();
        for (i, o) in b.objects.iter_mut().enumerate() {
            o.id = format!("Renamed{i}");
        }
        let (da, db) = (digests(&a), digests(&b));
        assert_eq!(da.iter().map(|x| x.1).collect::<Vec<_>>(), db.iter().map(|x| x.1).collect::<Vec<_>>(), "a rename redraws nothing");
        let mut c = a.clone();
        c.objects[1].id = c.objects[0].id.clone();
        c.objects[2].id = String::new();
        let keys: Vec<String> = digests(&c).into_iter().map(|x| x.0).collect();
        assert_eq!(keys[1..4], ["@obj:Item.000".to_string(), "@obj#1".into(), "@obj#2".into()]);
        assert_eq!(keys[5..], (0..8).map(|k| format!("@turntable:{k}")).collect::<Vec<_>>());
        // Two objects drawing the same thing in the same place have the same digest.
        let mut d = a.clone();
        d.objects[1] = d.objects[0].clone();
        d.objects[1].id = "Copy".into();
        let dd = digests(&d);
        assert_eq!(dd[1].1, dd[2].1);
    }

    #[test]
    fn an_edit_dirties_only_what_draws_it() {
        let mut a = testgen::grid_scene(2, false);
        a.cameras = testgen::camera_scene().cameras;
        let base = digests(&a);
        let changed = |b: &crate::scene::Scene| -> Vec<String> {
            base.iter().zip(digests(b)).filter(|(x, y)| x.1 != y.1).map(|(x, _)| x.0.clone()).collect()
        };
        let tt: Vec<String> = (0..8).map(|k| format!("@turntable:{k}")).collect();
        let with_tt = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).chain(tt.iter().cloned()).collect() };
        // Move one object: the scene, the camera, that object and the turntable.
        let mut m = a.clone();
        m.objects[2].transform[12] += 0.5;
        assert_eq!(changed(&m), with_tt(&["@scene", "@cam:Camera", "@obj:Item.002"]));
        // Its colour, its sidedness, its modifiers flag: the same set.
        let mut c = a.clone();
        c.objects[2].base_color[0] = 0.0;
        assert_eq!(changed(&c), with_tt(&["@scene", "@cam:Camera", "@obj:Item.002"]));
        let mut f = a.clone();
        f.objects[2].modifiers_not_applied = true;
        assert_eq!(changed(&f), with_tt(&["@scene", "@cam:Camera", "@obj:Item.002"]));
        // A shared mesh's data: every object that uses it (0 and 2 use mesh 0).
        let mut g = a.clone();
        g.meshes[0].positions[0][0] += 0.25;
        assert_eq!(changed(&g), with_tt(&["@scene", "@cam:Camera", "@obj:Item.000", "@obj:Item.002"]));
        // The camera: only its view.
        let mut k = a.clone();
        k.cameras[0].yfov = 0.7;
        assert_eq!(changed(&k), ["@cam:Camera"]);
        // The up axis: every view.
        let mut u = a.clone();
        u.up = UpAxis::Z;
        assert_eq!(changed(&u).len(), base.len());
        // The pack's own hashes and the modifier names change no pixels.
        let mut n = a.clone();
        n.meshes[0].hash = Some("x".into());
        n.objects[0].hash = Some("y".into());
        n.objects[0].modifiers = vec!["MIRROR".into()];
        assert!(changed(&n).is_empty());
    }

    #[test]
    fn each_object_lists_its_meshs_local_box() {
        let mut s = testgen::grid_scene(2, false);
        s.objects[3].modifiers = vec!["SUBSURF".into()];
        s.objects[3].modifiers_not_applied = true;
        // A third mesh with nothing in it, and an object using it.
        s.meshes.push(crate::scene::Mesh { positions: Vec::new(), normals: None, indices: Vec::new(), hash: None });
        s.objects.push(testgen::object("Empty", 2, [1.0; 3], testgen::identity(), None));
        let items = list(&encode(&s), &InputLimits::default()).expect("list");
        let cube = Some(LocalBounds { min: [-0.5; 3], max: [0.5; 3], approx: false });
        let objects: Vec<&Item> = items.iter().filter(|i| matches!(i.target, Target::Object(_))).collect();
        assert_eq!(objects.len(), 5);
        assert_eq!(objects[0].bounds, cube, "mesh-local: the object's transform isn't in it");
        assert_eq!(objects[2].bounds, cube, "a shared mesh: the same box");
        let sphere = objects[1].bounds.expect("the sphere's box");
        assert!(!sphere.approx && sphere.min[1] < 0.0 && sphere.max[1] > 0.0);
        assert_eq!(objects[3].bounds.map(|b| b.approx), Some(true), "modifiers not applied: approximate");
        assert_eq!(objects[4].bounds, None, "an empty mesh has no box");
        assert!(items.iter().filter(|i| !matches!(i.target, Target::Object(_))).all(|i| i.bounds.is_none()));
        // The exact f32 extremes, whatever the transform.
        let mut t = testgen::cube_scene();
        t.meshes[0].positions[0] = [-0.75, 0.1, 0.2];
        t.objects[0].transform = testgen::placed([5.0, 6.0, 7.0], [3.0, 3.0, 3.0]);
        let b = list(&encode(&t), &InputLimits::default()).expect("list")[1].bounds.expect("a box");
        assert_eq!((b.min, b.max), ([-0.75, -0.5, -0.5], [0.5, 0.5, 0.5]));
        // A non-finite position (the renderer refuses the scene): no box, and no panic.
        let mut n = testgen::cube_scene();
        n.meshes[0].positions[2][0] = f32::INFINITY;
        assert_eq!(list(&encode(&n), &InputLimits::default()).expect("list")[1].bounds, None);
    }

    #[test]
    fn camera_items_carry_their_display_values_and_no_digest_sees_them() {
        let a = testgen::camera_scene();
        let mut b = a.clone();
        b.cameras[0].active = Some(true);
        b.cameras[0].lens_mm = Some(35.0);
        b.cameras[0].sensor_mm = Some([36.0, 24.0]);
        b.cameras[0].sensor_fit = Some(crate::scene::SensorFit::Horizontal);
        let (la, lb) = (list(&encode(&a), &InputLimits::default()).expect("a"), list(&encode(&b), &InputLimits::default()).expect("b"));
        assert_eq!(la.iter().map(|i| i.digest).collect::<Vec<_>>(), lb.iter().map(|i| i.digest).collect::<Vec<_>>());
        assert_eq!(lb[1].camera, Some(b.cameras[0].display()));
        assert_eq!(serde_json::to_string(&lb[1].camera).expect("json"), r#"{"active":true,"lens_mm":35.0,"sensor_mm":[36.0,24.0],"sensor_fit":"horizontal"}"#);
        // Absent in the file: absent in the item.
        assert_eq!(serde_json::to_string(&la[1].camera).expect("json"), "{}");
        assert!(la.iter().chain(&lb).filter(|i| !matches!(i.target, Target::Camera(_))).all(|i| i.camera.is_none()));
    }

    #[test]
    fn junk_is_a_typed_refusal() {
        assert!(list(b"not a scene", &InputLimits::default()).is_err());
        let b = encode(&testgen::cube_scene());
        assert!(list(&b[..b.len() - 4], &InputLimits::default()).is_err());
    }
}
