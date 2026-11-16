//! What a scene previews, and the raw digest of what each draws: the host's
//! listing (V3's `PreviewRenderer::list`), from the mesh read alone.
//!
//! **Containers**, in this order: the scene view (the auto 3/4 view of every
//! object), one per camera (through that camera), one per object (the auto
//! view of that object alone), then the turntable's eight frames of every
//! object (frame k at azimuth 45 + 45k degrees, [`ViewSpec::Turntable`]). So
//! a scene lists 1 + cameras + objects + 8 items.
//!
//! **Addresses** ([`addresses`]): every one is a native address in the
//! multiplayer kit's grammar ([`crate::addr`]), so a still's address is also
//! a presence view address.
//! - the scene view: `@scene:main`; a turntable frame: `@turntable:<k>`;
//! - an object's view: the object's `key` when the read gives one (its live
//!   address, `@obj:N`); else its name, `@obj:<enc(id)>` ([`crate::addr::enc`]);
//!   else, when the name is empty, encodes longer than [`MAX_ADDRESS_ID`]
//!   bytes, is another unkeyed object's too, or is spelled like another
//!   object's key, its index in this read, `@obj-n:<i>` (from 0);
//! - a camera's view: the same with `cam` (`@cam:N`, `@cam:<enc(id)>`,
//!   `@cam-n:<i>`).
//!
//! An object hidden in the file is addressed the same way as a shown one.
//!
//! **Which survive an edit:** a key form names the same object across
//! versions, renames included; a name form across every edit but a rename
//! of that object (renamed, it is listed under the new name). The index form
//! (`@obj-n`, `@cam-n`) is **bound to one version**: it means "the i-th
//! object in this read", so adding or removing an object before it, or a
//! rename that makes its name unique again, moves it. Adding, removing or
//! editing other objects never changes a key form, nor a name form unless
//! another unkeyed object takes the same name (both then get the index form).
//!
//! **Digest** (SHA-256, [`crate::sha256`]), over what changes the pixels and
//! nothing else, object and camera ids left out (so a rename doesn't redraw):
//! the up axis; for a camera view the camera's position, target or forward,
//! up, field of view, aspect and near plane; for a turntable frame its
//! number; then for each drawn object, in
//! order, its mesh's digest (over the mesh's counts and its data bytes as
//! read), its transform, its base colour, its sidedness and
//! `modifiers_not_applied` (shown with the image). The scene, a camera and a
//! turntable frame draw every object not hidden in the file: an object
//! hidden in it is left out of the objects above and, only when one is,
//! the indices of the hidden ones follow them (they place the drawn objects'
//! values in the id image). So showing or hiding an object redraws those
//! views, editing a hidden one redraws none of them, and a scene with no
//! hidden object has the digests it had before the field. An object's own
//! view draws it hidden or not: whether it is hidden is not in that view's
//! digest. `transform_not_evaluated` draws nothing differently but rides
//! with the image (`base_values`), so it is in the digest of every view that
//! draws the object, its own included: only when some drawn object has it,
//! the positions of those objects among the drawn ones follow (after the
//! hidden indices), so a scene without it keeps its digests and a cached
//! image's flag can't go stale. The renderer's build is not in it: the host
//! keys its cache with it.
//!
//! **Bounds:** each object item also carries its mesh's local box
//! ([`LocalBounds`], mesh space: the client applies the object's transform),
//! read from the positions in the bytes; `approx` when the object's
//! modifiers were not applied (the evaluated mesh may be larger). An empty
//! mesh, or one with a non-finite position (which the renderer refuses), has
//! none. Bounds are not in any digest. Each object item also says whether the
//! object is hidden in the file ([`Item::hidden`]: it is listed all the same,
//! with its box) and whether its placement is not evaluated
//! ([`Item::not_evaluated`]: the box is placed by the base transform).
//!
//! **Cameras:** each camera item carries the camera's display values
//! ([`CameraDisplay`]: `active`, `lens_mm`, `sensor_mm`, `sensor_fit`, each
//! left out when the file has none). They draw nothing and are in no digest.
//! The cameras the producer left out (`cameras_left_out`, [`listed`]) come
//! with the listing as read (kept, with counts of those past the cap and
//! those dropped as malformed; never a refusal), and are in no digest
//! either.

use serde::Serialize;

use crate::addr;
use crate::camera::{mesh_bounds, ViewSpec, TURNTABLE_FRAMES};
use crate::sha256::Sha256;
use crate::scene::{self, Camera, CameraDisplay, CamerasLeftOut, Header, Hidden, InputError, InputLimits, MeshInfo, Object, UpAxis};

/// Longest encoded name put in an address (bytes); a longer one gets the
/// index form.
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
    /// Its address (`@scene:main`, `@cam:3`, `@obj:Cube`, `@obj-n:4`,
    /// `@turntable:3`; see the module docs).
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
    /// An object hidden in the file, and how (`None` for a shown object and
    /// for every other item).
    pub hidden: Option<Hidden>,
    /// An object whose placement is driven and not evaluated: drawn, and its
    /// box placed, at its base transform (`false` for every other item).
    pub not_evaluated: bool,
    /// A camera or object the read keys: `key` is its own key from the read
    /// (its live key), not its name or index form (`false` for every other
    /// item). Never in a digest.
    pub keyed: bool,
}

/// The addresses of one list's views (`kind` `obj` or `cam`), from each
/// one's `(key, id)` in the read's order (see the module docs): its key,
/// else its encoded name when that is unique and no one's key, else its
/// index (version-bound).
pub fn addresses<'a>(kind: &str, list: impl IntoIterator<Item = (Option<&'a str>, &'a str)>) -> Vec<String> {
    let list: Vec<(Option<&str>, &str)> = list.into_iter().collect();
    let keys: std::collections::HashSet<&str> = list.iter().filter_map(|(k, _)| *k).collect();
    let named: Vec<Option<String>> = list
        .iter()
        .map(|(k, id)| {
            let e = addr::enc(id);
            (k.is_none() && !e.is_empty() && e.len() <= MAX_ADDRESS_ID).then(|| format!("@{kind}:{e}"))
        })
        .collect();
    let mut count = std::collections::HashMap::new();
    for a in named.iter().flatten() {
        *count.entry(a.as_str()).or_insert(0u32) += 1;
    }
    list.iter()
        .zip(&named)
        .enumerate()
        .map(|(i, ((key, _), name))| match (key, name) {
            (Some(k), _) => k.to_string(),
            (None, Some(a)) if count[a.as_str()] == 1 && !keys.contains(a.as_str()) && addr::is_native(a) => a.clone(),
            _ => format!("@{kind}-n:{i}"),
        })
        .collect()
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

/// `objects` are the drawn ones, in order; `hidden` the indices of the
/// scene's objects a scene, camera or turntable view leaves out as hidden in
/// the file (empty for an object's view, and for a scene with none, which
/// then hashes as it did before the field). The drawn objects whose
/// placement is not evaluated follow, by their position in `objects`, only
/// when there are any.
fn view_digest(up: UpAxis, through: Through<'_>, objects: &[&Object], hidden: &[u32], mesh: &[[u8; 32]]) -> [u8; 32] {
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
    // After the fixed-size records counted above, so it can't read as one of them.
    if !hidden.is_empty() {
        h.update(b"hidden\0");
        h.update(&(hidden.len() as u64).to_le_bytes());
        for i in hidden {
            h.update(&i.to_le_bytes());
        }
    }
    let driven: Vec<u32> = objects.iter().enumerate().filter(|(_, o)| o.transform_not_evaluated).map(|(k, _)| k as u32).collect();
    if !driven.is_empty() {
        h.update(b"not_evaluated\0");
        h.update(&(driven.len() as u64).to_le_bytes());
        for k in &driven {
            h.update(&k.to_le_bytes());
        }
    }
    h.finish()
}

/// The containers of a K2SCENE1 held in memory, with their digests. Reads
/// and checks the header ([`scene::layout`]); hashes each mesh's data once,
/// without decoding it.
pub fn list(bytes: &[u8], lim: &InputLimits) -> Result<Vec<Item>, InputError> {
    listed(bytes, lim).map(|l| l.items)
}

/// A scene's listing: its containers ([`list`]) and what the header says
/// beside them.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub items: Vec<Item>,
    /// The cameras the producer couldn't express, as read from the header
    /// (kept, past the cap, dropped; empty when it has none). In no digest.
    pub cameras_left_out: CamerasLeftOut,
}

/// [`list`], with the cameras left out.
pub fn listed(bytes: &[u8], lim: &InputLimits) -> Result<Listed, InputError> {
    let (mut h, ranges) = scene::layout(bytes, lim)?;
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
    let cameras_left_out = std::mem::take(&mut h.cameras_left_out);
    Ok(Listed { items: list_header(&h, &mesh, &boxes), cameras_left_out })
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
    // The scene, the cameras and the turntable draw every object not hidden in the file.
    let all: Vec<&Object> = h.objects.iter().filter(|o| o.hidden.is_none()).collect();
    let hidden: Vec<u32> = h.objects.iter().enumerate().filter(|(_, o)| o.hidden.is_some()).map(|(i, _)| i as u32).collect();
    let mut items = Vec::with_capacity(1 + h.cameras.len() + h.objects.len() + TURNTABLE_FRAMES as usize);
    items.push(Item { key: addr::SCENE.into(), title: String::new(), target: Target::Scene, digest: view_digest(h.up, Through::Auto, &all, &hidden, mesh), bounds: None, camera: None, hidden: None, not_evaluated: false, keyed: false });
    // Every object gets its address from its key or its name, hidden in the file or not.
    let cams = addresses("cam", h.cameras.iter().map(|c| (c.key.as_deref(), c.id.as_str())));
    let objs = addresses("obj", h.objects.iter().map(|o| (o.key.as_deref(), o.id.as_str())));
    for ((i, c), key) in h.cameras.iter().enumerate().zip(cams) {
        items.push(Item { key, title: c.id.clone(), target: Target::Camera(i as u32), digest: view_digest(h.up, Through::Camera(c), &all, &hidden, mesh), bounds: None, camera: Some(c.display()), hidden: None, not_evaluated: false, keyed: c.key.is_some() });
    }
    for ((i, o), key) in h.objects.iter().enumerate().zip(objs) {
        let bounds = boxes[o.mesh as usize].map(|(min, max)| LocalBounds { min, max, approx: o.modifiers_not_applied });
        items.push(Item { key, title: o.id.clone(), target: Target::Object(i as u32), digest: view_digest(h.up, Through::Auto, &[o], &[], mesh), bounds, camera: None, hidden: o.hidden, not_evaluated: o.transform_not_evaluated, keyed: o.key.is_some() });
    }
    for k in 0..TURNTABLE_FRAMES {
        let key = format!("@turntable:{k}");
        items.push(Item { key, title: String::new(), target: Target::Turntable(k), digest: view_digest(h.up, Through::Turntable(k), &all, &hidden, mesh), bounds: None, camera: None, hidden: None, not_evaluated: false, keyed: false });
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
        assert_eq!(keys, ["@scene:main", "@cam:Camera", "@obj:Cube", "@turntable:0", "@turntable:1", "@turntable:2", "@turntable:3", "@turntable:4", "@turntable:5", "@turntable:6", "@turntable:7"]);
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
        assert_eq!(keys[..4], ["@scene:main".to_string(), "@cam:turntable:1".into(), "@obj:@turntable:0".into(), "@turntable:0".into()]);
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
        assert_eq!(keys[1..5], ["@obj-n:0".to_string(), "@obj-n:1".into(), "@obj-n:2".into(), "@obj:Item.003".into()], "a duplicated name: each by its index; an empty one too");
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
        assert_eq!(changed(&m), with_tt(&["@scene:main", "@cam:Camera", "@obj:Item.002"]));
        // Its colour, its sidedness, its modifiers flag: the same set.
        let mut c = a.clone();
        c.objects[2].base_color[0] = 0.0;
        assert_eq!(changed(&c), with_tt(&["@scene:main", "@cam:Camera", "@obj:Item.002"]));
        let mut f = a.clone();
        f.objects[2].modifiers_not_applied = true;
        assert_eq!(changed(&f), with_tt(&["@scene:main", "@cam:Camera", "@obj:Item.002"]));
        // A shared mesh's data: every object that uses it (0 and 2 use mesh 0).
        let mut g = a.clone();
        g.meshes[0].positions[0][0] += 0.25;
        assert_eq!(changed(&g), with_tt(&["@scene:main", "@cam:Camera", "@obj:Item.000", "@obj:Item.002"]));
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

    /// Hiding an object redraws the scene, the cameras and the turntable,
    /// never its own view or another object's; editing a hidden object
    /// redraws only its own view; a scene without the field digests as it did.
    #[test]
    fn hiding_an_object_dirties_the_views_that_leave_it_out() {
        use crate::scene::Hidden;
        let mut a = testgen::grid_scene(2, false);
        a.cameras = testgen::camera_scene().cameras;
        let base = digests(&a);
        let changed = |b: &crate::scene::Scene| -> Vec<String> { base.iter().zip(digests(b)).filter(|(x, y)| x.1 != y.1).map(|(x, _)| x.0.clone()).collect() };
        let mut tt: Vec<String> = vec!["@scene:main".into(), "@cam:Camera".into()];
        tt.extend((0..8).map(|k| format!("@turntable:{k}")));
        let mut h = a.clone();
        h.objects[1].hidden = Some(Hidden::Itself);
        assert_eq!(changed(&h), tt, "not @obj:Item.001");
        // Self or collection: the same pixels, the same digests.
        let mut c = a.clone();
        c.objects[1].hidden = Some(Hidden::Collection);
        assert_eq!(digests(&c), digests(&h));
        // Hidden is not deleted (the others keep their indices in the id image).
        let mut d = a.clone();
        d.objects.remove(1);
        assert_ne!(digests(&d)[0].1, digests(&h)[0].1);
        // Two hidden differ from one.
        let mut two = h.clone();
        two.objects[3].hidden = Some(Hidden::Itself);
        assert_ne!(digests(&two)[0].1, digests(&h)[0].1);
        // Editing a hidden object: only its own view.
        let mut m = h.clone();
        m.objects[1].transform[12] += 0.5;
        m.objects[1].base_color[0] = 0.0;
        let hd = digests(&h);
        let moved: Vec<String> = hd.iter().zip(digests(&m)).filter(|(x, y)| x.1 != y.1).map(|(x, _)| x.0.clone()).collect();
        assert_eq!(moved, ["@obj:Item.001"]);
        // The items say what they are; a hidden object keeps its box.
        let mut hn = h.clone();
        hn.objects[1].transform_not_evaluated = true;
        let items = list(&encode(&hn), &InputLimits::default()).expect("list");
        let obj = items.iter().find(|i| i.key == "@obj:Item.001").expect("listed");
        assert_eq!((obj.hidden, obj.not_evaluated), (Some(Hidden::Itself), true));
        assert!(obj.bounds.is_some_and(|b| !b.approx));
        assert!(items.iter().filter(|i| i.key != "@obj:Item.001").all(|i| i.hidden.is_none() && !i.not_evaluated));
        // A hidden object is addressed like a shown one: by its key, else its name, else its index.
        let keys = |s: &crate::scene::Scene| -> Vec<String> { list(&encode(s), &InputLimits::default()).expect("list").into_iter().map(|i| i.key).collect() };
        let mut k = a.clone();
        k.objects[0].key = Some("@obj:7".into());
        k.objects[2].id = k.objects[3].id.clone();
        let mut kh = k.clone();
        for o in kh.objects.iter_mut() {
            o.hidden = Some(Hidden::Collection);
        }
        assert_eq!(keys(&kh), keys(&k));
        assert_eq!(keys(&kh)[2..6], ["@obj:7".to_string(), "@obj:Item.001".into(), "@obj-n:2".into(), "@obj-n:3".into()]);
    }

    /// `transform_not_evaluated` is in the digest of every view that draws
    /// the object, its own included, so an image's `base_values` can't go
    /// stale; `false` hashes as absent, and a hidden object's flag dirties
    /// only its own view.
    #[test]
    fn not_evaluated_dirties_the_views_that_draw_it_and_false_is_absent() {
        let mut a = testgen::grid_scene(2, false);
        a.cameras = testgen::camera_scene().cameras;
        let base = digests(&a);
        let changed = |b: &crate::scene::Scene| -> Vec<String> { base.iter().zip(digests(b)).filter(|(x, y)| x.1 != y.1).map(|(x, _)| x.0.clone()).collect() };
        let with_tt = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).chain((0..8).map(|k| format!("@turntable:{k}"))).collect() };
        // `false` written out is the same bytes as absent: today's digests.
        let mut f = a.clone();
        f.objects[2].transform_not_evaluated = false;
        assert_eq!(digests(&f), base);
        let plain = encode(&f);
        let hlen = u32::from_le_bytes([plain[8], plain[9], plain[10], plain[11]]) as usize;
        let header = String::from_utf8(plain[12..12 + hlen].to_vec()).expect("a JSON header");
        assert!(!header.contains("transform_not_evaluated"));
        let j = header.replacen(r#""mesh":0,"#, r#""transform_not_evaluated":false,"mesh":0,"#, 1);
        assert!(j.contains(r#""transform_not_evaluated":false"#));
        let mut written = plain[..8].to_vec();
        written.extend_from_slice(&(j.len() as u32).to_le_bytes());
        written.extend_from_slice(j.as_bytes());
        while !written.len().is_multiple_of(4) {
            written.push(0);
        }
        written.extend_from_slice(&plain[(12 + hlen + 3) & !3..]);
        let listed: Vec<(String, [u8; 32])> = list(&written, &InputLimits::default()).expect("written false").into_iter().map(|i| (i.key, i.digest)).collect();
        assert_eq!(listed, base, "an explicit false");
        // True: the scene, the camera, its own view and the turntable; no other object.
        let mut t = a.clone();
        t.objects[2].transform_not_evaluated = true;
        assert_eq!(changed(&t), with_tt(&["@scene:main", "@cam:Camera", "@obj:Item.002"]));
        // Another object's flag is another digest.
        let mut u = a.clone();
        u.objects[3].transform_not_evaluated = true;
        assert_ne!(digests(&u)[0].1, digests(&t)[0].1);
        // Hidden: only its own view draws it, so only its own view changes.
        let mut h = a.clone();
        h.objects[2].hidden = Some(crate::scene::Hidden::Itself);
        let hd = digests(&h);
        let mut hn = h.clone();
        hn.objects[2].transform_not_evaluated = true;
        let moved: Vec<String> = hd.iter().zip(digests(&hn)).filter(|(x, y)| x.1 != y.1).map(|(x, _)| x.0.clone()).collect();
        assert_eq!(moved, ["@obj:Item.002"]);
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

    /// Names with spaces, `%`, `/`, `:`, tabs and non-ASCII, a duplicate,
    /// and keyed objects and cameras (a camera object keyed among both).
    fn addressed_scene() -> crate::scene::Scene {
        let mut s = testgen::grid_scene(2, false);
        s.cameras = testgen::camera_scene().cameras;
        let mut cam2 = s.cameras[0].clone();
        cam2.id = "Cam A/B".into();
        s.cameras.push(cam2);
        s.cameras[0].key = Some("@cam:3".into());
        let names = ["Café Tisch", "ナイフ", "50% off", "a/b", "x:y	z", "Café Tisch", "Lamp", "Cube"];
        let mut more = s.objects.clone();
        more.extend(s.objects.clone());
        s.objects = more;
        for (o, n) in s.objects.iter_mut().zip(names) {
            o.id = n.into();
        }
        s.objects[6].key = Some("@obj:3".into());
        s.objects[7].key = Some("@obj:17".into());
        s
    }

    fn keys_of(s: &crate::scene::Scene) -> Vec<String> {
        list(&encode(s), &InputLimits::default()).expect("list").into_iter().map(|i| i.key).collect()
    }

    #[test]
    fn every_address_is_native_whatever_the_names() {
        let s = addressed_scene();
        let keys = keys_of(&s);
        assert_eq!(
            keys[..11],
            [
                "@scene:main",
                "@cam:3",
                "@cam:Cam%20A%2FB",
                "@obj-n:0",
                "@obj:ナイフ",
                "@obj:50%25%20off",
                "@obj:a%2Fb",
                "@obj:x:y%09z",
                "@obj-n:5",
                "@obj:3",
                "@obj:17",
            ]
        );
        for k in &keys {
            assert!(addr::is_native(k), "{k}");
        }
        // Only an address that is the read's own key says so.
        let keyed: Vec<String> = list(&encode(&s), &InputLimits::default()).expect("list").into_iter().filter(|i| i.keyed).map(|i| i.key).collect();
        assert_eq!(keyed, ["@cam:3", "@obj:3", "@obj:17"]);
        // A name form decodes back to the name exactly.
        for (k, o) in keys[3..11].iter().zip(&s.objects) {
            if let Some(e) = k.strip_prefix("@obj:").filter(|_| o.key.is_none()) {
                assert_eq!(addr::dec(e).as_deref(), Some(o.id.as_str()), "{k}");
            }
        }
        // An unkeyed name spelled like another object's key isn't taken: the index form.
        let mut t = s.clone();
        t.objects[1].id = "3".into();
        assert_eq!(keys_of(&t)[4], "@obj-n:1");
        // Empty, or too long once encoded: the index form.
        let mut u = s.clone();
        u.objects[1].id = String::new();
        u.objects[2].id = "%".repeat(MAX_ADDRESS_ID / 3 + 1);
        assert_eq!(keys_of(&u)[4..6], ["@obj-n:1".to_string(), "@obj-n:2".into()]);
    }

    #[test]
    fn a_rename_keeps_a_key_and_changes_a_name() {
        let s = addressed_scene();
        let before = keys_of(&s);
        let mut r = s.clone();
        r.objects[6].id = "Floor lamp".into(); // keyed
        r.objects[4].id = "Lösung 2".into(); // by name
        r.cameras[0].id = "Renamed".into(); // keyed
        let after = keys_of(&r);
        assert_eq!(after[9], before[9], "keyed: the same address");
        assert_eq!(after[1], before[1]);
        assert_eq!((before[7].as_str(), after[7].as_str()), ("@obj:x:y%09z", "@obj:Lösung%202"), "by name: the new name, encoded");
        let changed: Vec<usize> = (0..before.len()).filter(|&i| before[i] != after[i]).collect();
        assert_eq!(changed, [7]);
    }

    /// Adding or removing other objects leaves a key form and a name form
    /// alone; an index form is bound to its read and moves.
    #[test]
    fn other_objects_coming_and_going_move_only_index_forms() {
        let s = addressed_scene();
        let addr_of = |sc: &crate::scene::Scene, id: &str, nth: usize| -> String {
            let objs = addresses("obj", sc.objects.iter().map(|o| (o.key.as_deref(), o.id.as_str())));
            sc.objects.iter().zip(objs).filter(|(o, _)| o.id == id).nth(nth).map(|(_, a)| a).expect("there")
        };
        let watched = [("ナイフ", 0), ("Lamp", 0), ("Cube", 0)];
        let base: Vec<String> = watched.iter().map(|(id, n)| addr_of(&s, id, *n)).collect();
        assert_eq!(base, ["@obj:ナイフ", "@obj:3", "@obj:17"]);
        // One removed before them, one added at the start, one added at the end.
        let mut removed = s.clone();
        removed.objects.remove(0);
        let mut added = s.clone();
        added.objects.insert(0, testgen::object("New one", 0, [1.0; 3], testgen::identity(), None));
        added.objects.push(testgen::object("Another", 1, [1.0; 3], testgen::identity(), None));
        for v in [&removed, &added] {
            assert_eq!(watched.iter().map(|(id, n)| addr_of(v, id, *n)).collect::<Vec<_>>(), base);
        }
        // The duplicate's second copy: @obj-n:5 in this read, @obj-n:4 once an object before it goes, @obj-n:6 once one comes.
        assert_eq!(addr_of(&s, "Café Tisch", 1), "@obj-n:5");
        let mut dropped = s.clone();
        dropped.objects.remove(1);
        assert_eq!(addr_of(&dropped, "Café Tisch", 1), "@obj-n:4");
        assert_eq!(addr_of(&added, "Café Tisch", 1), "@obj-n:6");
        // And with its twin gone, the name form again.
        let mut single = s.clone();
        single.objects.remove(0);
        assert_eq!(addr_of(&single, "Café Tisch", 0), "@obj:Café%20Tisch");
    }

    /// The reader checks keys: native, of the list's kind, once per list.
    #[test]
    fn keys_are_checked_and_change_no_digest() {
        let s = addressed_scene();
        let mut plain = s.clone();
        for o in &mut plain.objects {
            o.key = None;
        }
        for c in &mut plain.cameras {
            c.key = None;
        }
        let digests_of = |sc: &crate::scene::Scene| list(&encode(sc), &InputLimits::default()).expect("list").into_iter().map(|i| i.digest).collect::<Vec<_>>();
        assert_eq!(digests_of(&s), digests_of(&plain), "a key is in no digest");
        // Absent, the bytes are as before keys existed (no field written).
        assert!(!String::from_utf8_lossy(&encode(&plain)).contains("\"key\""));
        // A camera object keyed among the objects and among the cameras: allowed.
        let mut both = s.clone();
        both.objects[0].key = Some("@obj:3x".into());
        both.cameras[1].key = Some("@cam:3x".into());
        assert!(list(&encode(&both), &InputLimits::default()).is_ok());
        let refused = |sc: &crate::scene::Scene| {
            let e = crate::scene::decode(&encode(sc), &InputLimits::default()).expect_err("refused");
            assert_eq!(e.code(), "invalid_input", "{e}");
            assert_eq!(list(&encode(sc), &InputLimits::default()).expect_err("listing refused too").code(), "invalid_input");
            e.to_string()
        };
        for bad in ["@cam:5", "obj:5", "@obj:a b", "@obj:", "@k:5", "@obj:1/sp:2", "@Obj:5", "", " @obj:5"] {
            let mut b = s.clone();
            b.objects[0].key = Some(bad.into());
            assert!(refused(&b).contains("@obj:<id>"), "{bad:?}");
        }
        for bad in ["@obj:5", "@cam:\t", "cam:5"] {
            let mut b = s.clone();
            b.cameras[1].key = Some(bad.into());
            assert!(refused(&b).contains("@cam:<id>"), "{bad:?}");
        }
        let mut dup = s.clone();
        dup.objects[0].key = Some("@obj:17".into());
        assert!(refused(&dup).contains("another object's"));
        let mut dupc = s.clone();
        dupc.cameras[1].key = Some("@cam:3".into());
        assert!(refused(&dupc).contains("another camera's"));
        // A good one is read back as given.
        let back = crate::scene::decode(&encode(&s), &InputLimits::default()).expect("read");
        assert_eq!((back.objects[6].key.as_deref(), back.cameras[0].key.as_deref()), (Some("@obj:3"), Some("@cam:3")));
    }

    /// `bytes` with its header's `cameras_left_out` set to `v` as raw JSON
    /// (any shape), the data moved to the new padding.
    fn with_left_out(bytes: &[u8], v: serde_json::Value) -> Vec<u8> {
        let hlen = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
        let mut h: serde_json::Value = serde_json::from_slice(&bytes[12..12 + hlen]).expect("a header");
        h["cameras_left_out"] = v;
        let data = &bytes[(12 + hlen + 3) & !3..];
        let json = serde_json::to_vec(&h).expect("json");
        let mut out = crate::scene::MAGIC.to_vec();
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(&json);
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn cameras_left_out_are_listed_as_sent_never_refuse_and_are_in_no_digest() {
        use crate::scene::{decode, CameraLeftOut, MAX_LEFT_OUT_BYTES};
        use serde_json::json;
        let lim = InputLimits::default();
        let s = addressed_scene();
        let plain = encode(&s);
        let digests = |b: &[u8]| list(b, &lim).expect("listed").into_iter().map(|i| (i.key, i.digest)).collect::<Vec<_>>();
        let left_out = |b: &[u8]| {
            let l = listed(b, &lim).expect("never refused");
            assert_eq!(decode(b, &lim).expect("never refused").cameras_left_out, l.cameras_left_out, "the renderer reads the same");
            assert_eq!(digests(b), digests(&plain), "in no digest");
            l.cameras_left_out
        };
        let counts = |l: &CamerasLeftOut| (l.list.len(), l.more, l.dropped);
        // Absent: none, and no field written.
        assert!(!String::from_utf8_lossy(&plain).contains("cameras_left_out"));
        assert!(left_out(&plain).is_empty());
        // Empty: as absent.
        assert!(left_out(&with_left_out(&plain, json!([]))).is_empty());
        // One with a key and one without: read back as sent.
        let two = vec![
            CameraLeftOut { name: "Ortho top".into(), key: Some("@cam:7".into()), reason: "orthographic".into() },
            CameraLeftOut { name: "Pano".into(), key: None, reason: "panoramic".into() },
        ];
        let mut t = s.clone();
        t.cameras_left_out = CamerasLeftOut { list: two.clone(), ..Default::default() };
        let with = encode(&t);
        assert_eq!(left_out(&with), CamerasLeftOut { list: two.clone(), more: 0, dropped: 0 });
        let sent = with_left_out(&plain, json!([{ "name": "Ortho top", "key": "@cam:7", "reason": "orthographic" }, { "name": "Pano", "reason": "panoramic", "key": null }]));
        assert_eq!(left_out(&sent).list, two, "a null key is no key");
        // Each string up to the cap is kept.
        let at_cap = "x".repeat(MAX_LEFT_OUT_BYTES);
        let key_at_cap = format!("@cam:{}", "7".repeat(MAX_LEFT_OUT_BYTES - 5));
        assert_eq!(counts(&left_out(&with_left_out(&plain, json!([{ "name": at_cap.as_str(), "key": key_at_cap, "reason": at_cap.as_str() }])))), (1, 0, 0));
        // A bad entry is dropped and counted; the good ones around it kept in order.
        let good = |n: &str| json!({ "name": n, "reason": "r" });
        let over = "x".repeat(MAX_LEFT_OUT_BYTES + 1);
        let bad_keys = ["@obj:7", "cam:7", "@cam:", "@cam:a b", "@cam:1/x:2", "@k:7", "@Cam:7", ""];
        let mut bad: Vec<serde_json::Value> = bad_keys.iter().map(|k| json!({ "name": "A", "key": k, "reason": "r" })).collect();
        bad.extend([
            json!({ "name": "A", "key": format!("@cam:{}", "7".repeat(MAX_LEFT_OUT_BYTES - 4)), "reason": "r" }),
            json!({ "name": over.as_str(), "reason": "r" }),
            json!({ "name": "A", "reason": over.as_str() }),
            json!({ "name": "A" }),
            json!({ "reason": "r" }),
            json!({ "name": 1, "reason": "r" }),
            json!({ "name": "A", "reason": ["r"] }),
            json!({ "name": "A", "key": 7, "reason": "r" }),
            json!("Ortho"),
            json!(null),
            json!(["A", "r"]),
            json!(3),
        ]);
        for b in &bad {
            let l = left_out(&with_left_out(&plain, json!([good("before"), b, good("after")])));
            assert_eq!(l.list.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["before", "after"], "{b}");
            assert_eq!((l.more, l.dropped), (0, 1), "{b}");
        }
        assert_eq!(counts(&left_out(&with_left_out(&plain, serde_json::Value::Array(bad.clone())))), (0, 0, bad.len() as u64));
        // Not a list: as absent, dropped 1.
        for b in [json!(null), json!({}), json!({ "name": "A", "reason": "r" }), json!("Ortho"), json!(7), json!(true)] {
            assert_eq!(counts(&left_out(&with_left_out(&plain, b.clone()))), (0, 0, 1), "{b}");
        }
        // Past the cap: the first ones kept in order, the rest counted; a
        // dropped entry takes no slot.
        let small = InputLimits { max_cameras: 2, ..InputLimits::default() };
        let many = with_left_out(&plain, json!([good("A"), json!({ "name": "bad" }), good("B"), good("C"), json!(5), good("D")]));
        let l = listed(&many, &small).expect("never refused").cameras_left_out;
        assert_eq!((l.list.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), l.more, l.dropped), (vec!["A", "B"], 2, 2));
        assert_eq!(decode(&many, &small).expect("never refused").cameras_left_out, l);
        let cap = lim.max_cameras as usize;
        let thousands = with_left_out(&plain, serde_json::Value::Array((0..cap + 3).map(|i| good(&format!("C{i}"))).collect()));
        let l = left_out(&thousands);
        assert_eq!(counts(&l), (cap, 3, 0));
        assert_eq!((l.list[0].name.as_str(), l.list[cap - 1].name.clone()), ("C0", format!("C{}", cap - 1)));
    }

    #[test]
    fn junk_is_a_typed_refusal() {
        assert!(list(b"not a scene", &InputLimits::default()).is_err());
        let b = encode(&testgen::cube_scene());
        assert!(list(&b[..b.len() - 4], &InputLimits::default()).is_err());
    }
}
