//! What a scene previews, and the raw digest of what each draws: the host's
//! listing (V3's `PreviewRenderer::list`), from the mesh read alone.
//!
//! **Containers:** `@scene` (the auto 3/4 view of every object), one per
//! camera (`@cam:<id>`, through that camera) and one per object (`@obj:<id>`,
//! the auto view of that object alone). An id that is empty, too long for an
//! address or already used gets the index form (`@cam#2`, `@obj#17`).
//!
//! **Digest** (SHA-256, [`crate::sha256`]), over what changes the pixels and
//! nothing else, object and camera ids left out (so a rename doesn't redraw):
//! the up axis; for a camera view the camera's position, target or forward,
//! up, field of view, aspect and near plane; then for each drawn object, in
//! order, its mesh's digest (over the mesh's counts and its data bytes as
//! read), its transform, its base colour, its sidedness and
//! `modifiers_not_applied` (shown with the image). The renderer's build is
//! not in it: the host keys its cache with it.

use crate::sha256::Sha256;
use crate::scene::{self, Camera, Header, InputError, InputLimits, MeshInfo, Object, UpAxis};

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
}

impl Target {
    /// The opaque locator the host keeps (never on the wire).
    pub fn locator(self) -> String {
        match self {
            Target::Scene => "scene".into(),
            Target::Camera(i) => format!("c{i}"),
            Target::Object(i) => format!("o{i}"),
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
        s.strip_prefix('o').and_then(n).map(Target::Object)
    }

    /// The view that draws it.
    pub fn view(self, width: u32) -> crate::proto::View {
        use crate::camera::ViewSpec;
        use crate::proto::View;
        match self {
            Target::Scene => View::new(ViewSpec::Auto, width),
            Target::Camera(i) => View::new(ViewSpec::Camera(i), width),
            Target::Object(i) => View { view: ViewSpec::Auto, objects: Some(vec![i]), width, aspect: None },
        }
    }
}

/// One previewed thing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Its address (`@scene`, `@cam:Camera`, `@obj:Cube`).
    pub key: String,
    /// Its id as the file names it (empty for the scene).
    pub title: String,
    pub target: Target,
    /// The raw digest of what it draws.
    pub digest: [u8; 32],
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

fn view_digest(up: UpAxis, camera: Option<&Camera>, objects: &[&Object], mesh: &[[u8; 32]]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"k2scenerender/view/v1\0");
    h.update(&[match up {
        UpAxis::Y => b'y',
        UpAxis::Z => b'z',
    }]);
    match camera {
        Some(c) => {
            h.update(&[1]);
            f32s(&mut h, &c.position);
            opt3(&mut h, &c.target);
            opt3(&mut h, &c.forward);
            f32s(&mut h, &c.up);
            f32s(&mut h, &[c.yfov]);
            opt1(&mut h, c.aspect);
            opt1(&mut h, c.znear);
        }
        None => h.update(&[0]),
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
    Ok(list_header(&h, &mesh))
}

fn list_header(h: &Header, mesh: &[[u8; 32]]) -> Vec<Item> {
    let all: Vec<&Object> = h.objects.iter().collect();
    let mut items = Vec::with_capacity(1 + h.cameras.len() + h.objects.len());
    items.push(Item { key: "@scene".into(), title: String::new(), target: Target::Scene, digest: view_digest(h.up, None, &all, mesh) });
    let mut used = std::collections::HashSet::new();
    for (i, c) in h.cameras.iter().enumerate() {
        let key = if addressable(&c.id) && used.insert(format!("@cam:{}", c.id)) { format!("@cam:{}", c.id) } else { format!("@cam#{i}") };
        items.push(Item { key, title: c.id.clone(), target: Target::Camera(i as u32), digest: view_digest(h.up, Some(c), &all, mesh) });
    }
    for (i, o) in h.objects.iter().enumerate() {
        let key = if addressable(&o.id) && used.insert(format!("@obj:{}", o.id)) { format!("@obj:{}", o.id) } else { format!("@obj#{i}") };
        items.push(Item { key, title: o.id.clone(), target: Target::Object(i as u32), digest: view_digest(h.up, None, &[o], mesh) });
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
        assert_eq!(keys, ["@scene", "@cam:Camera", "@obj:Cube"]);
        assert_eq!(items.iter().map(|i| i.target).collect::<Vec<_>>(), [Target::Scene, Target::Camera(0), Target::Object(0)]);
        for t in [Target::Scene, Target::Camera(7), Target::Object(12)] {
            assert_eq!(Target::from_locator(&t.locator()), Some(t));
        }
        for bad in ["", "c", "o-1", "c01", "x3", "o1x", "scene2"] {
            assert_eq!(Target::from_locator(bad), None, "{bad}");
        }
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
        // Move one object: the scene, the camera and that object.
        let mut m = a.clone();
        m.objects[2].transform[12] += 0.5;
        assert_eq!(changed(&m), ["@scene", "@cam:Camera", "@obj:Item.002"]);
        // Its colour, its sidedness, its modifiers flag: the same set.
        let mut c = a.clone();
        c.objects[2].base_color[0] = 0.0;
        assert_eq!(changed(&c), ["@scene", "@cam:Camera", "@obj:Item.002"]);
        let mut f = a.clone();
        f.objects[2].modifiers_not_applied = true;
        assert_eq!(changed(&f), ["@scene", "@cam:Camera", "@obj:Item.002"]);
        // A shared mesh's data: every object that uses it (0 and 2 use mesh 0).
        let mut g = a.clone();
        g.meshes[0].positions[0][0] += 0.25;
        assert_eq!(changed(&g), ["@scene", "@cam:Camera", "@obj:Item.000", "@obj:Item.002"]);
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
    fn junk_is_a_typed_refusal() {
        assert!(list(b"not a scene", &InputLimits::default()).is_err());
        let b = encode(&testgen::cube_scene());
        assert!(list(&b[..b.len() - 4], &InputLimits::default()).is_err());
    }
}
