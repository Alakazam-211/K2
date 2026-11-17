//! The mesh read itself: a version's bytes in, its K2SCENE1 and a report out ([`mesh_read`],
//! [`mesh_read_paired`]). Pure: no I/O, clock or environment, so the same bytes give the same
//! output, byte for byte.
//!
//! **The decode** ([`decode`]) is the live view's: the file cap, the decompression, the reader,
//! the version gate, the scene model and the live scene, with every decline the live tier
//! makes. So ids, file order and transforms are exactly what the live view holds.
//!
//! **Keys.** Each object is written with its address, `@obj:N`, and each camera with `@cam:N`,
//! `N` the number of the object's live key `oN`. The keys are the ones the live store holds for
//! the version's objects, given as the carrier's pairing (its keys for `objs`, in the decode's
//! order: [`mesh_read_paired`]); without a pairing, the keys an import of the bytes mints (`o1`,
//! `o2`, ... in file order: [`mesh_read`]). A pairing that doesn't fit the decode (another count
//! of objects, a key that isn't `oN`, a key twice) is refused ([`MeshReadError::Pairing`]): no
//! key is guessed.
//!
//! **The scene drawn** is the file's current scene: the `curscene` its global block
//! (`FileGlobal`) names. When that is missing or names no scene, the first scene in file order,
//! and the report says so ([`MeshReport::scene_from`]).
//!
//! **Objects**, in file order: the mesh objects the scene holds, through its master collection
//! and the collections below it. Each is written with:
//!
//! - its live name as its id, and its key;
//! - its mesh: each `Mesh` data-block written once, in the meshes' file order, and shared by
//!   every object using it; each mesh's content hash in the header;
//! - its world matrix, composed from its stored location, rotation and scale through its parents
//!   and their parent inverses (the live view's placements and the kit's `world_matrix`); the
//!   world matrix a file may cache is never read;
//! - `hidden` ([`super::hidden`]): `self` when its own viewport flag is set, `collection` when a
//!   hidden collection hides it; hidden objects stay in the output;
//! - its modifiers, by the file's own struct names (`SubsurfModifierData` gives `Subsurf`), at
//!   most [`MAX_MODIFIERS`], each at most [`MAX_MODIFIER_BYTES`]; `modifiers_not_applied` when
//!   it has any (the mesh is drawn without them);
//! - `transform_not_evaluated` when animation, a driver or a constraint drives its transform
//!   (the stored numbers are drawn);
//! - a constant base colour, and no `double_sided` (unknown: both sides drawn).
//!
//! **Cameras**, in file order: the camera objects the scene holds, perspective ones only, each
//! with its world matrix composed as an object's and its view worked out from its data and the
//! scene's render size ([`super::camera`]); `active` for the scene's own camera.
//!
//! **Skipped and reported, never placed by guess** ([`Skipped`]): an object linked from a
//! library (`linked_object`), a mesh linked from one (`linked_mesh`), a delta transform that
//! isn't identity (`delta_transform`), a parenting other than to an object
//! (`parent_not_object`) or to something not listed (`parent_unlisted`), a world matrix that
//! can't be composed (`transform_unplaced`) or isn't finite (`transform_not_finite`), a mesh
//! the reader refuses or that has no triangle (the mesh's own word,
//! [`mesh::MeshSkip::word`]); a camera with no camera data the read can use (`camera_no_data`),
//! or one [`super::camera`] refuses (its word: `camera_not_perspective`,
//! `camera_sensor_fit_unknown`...).
//!
//! **Bounds first.** The objects, cameras and meshes are counted against [`MeshReadLimits`];
//! then the read's heap is worked out from the decoded file's sizes and each mesh's counts (from
//! its own struct), and checked against [`MeshReadLimits::max_heap_bytes`], before any mesh
//! array is read or the output allocated. The vertex and triangle counts are checked on what is
//! written, after the loose vertices are dropped, by the writer's own checks.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::Key;
use k2_mpc_base::placement::{world_matrix, Link, Mat4, Placement, Precision};
use k2_mpc_base::session::Pairing;

use super::camera::{self, CameraData, Render};
use super::hidden::{HiddenBy, HiddenWalk};
use super::k2scene::{
    self, Hidden, LeftOut, MeshHead, Scene, SceneCamera, SceneMesh, SceneObject, MAX_MODIFIERS,
    MAX_MODIFIER_BYTES,
};
use super::mesh::{self, MeshCounts};
use super::tri::TriCounts;
use super::{at_most, MeshReadError, MeshReadLimits};
use crate::live::model::{
    self, coll_blocks, coll_children, key_num, LiveColl, LiveObj, LiveScene, Parent,
};
use crate::live::{address_of, place, OBJS, OBJ_PROPS};
use crate::mutant;
use crate::reader::{read_blend, BlendFile, Block, Target};
use crate::scene::{read_scene, Detail, IdBlock, SceneModel, LIST_MAX};
use crate::write::decompress;

/// This read's own revision: a host keys its cache of the output by the version and this.
pub const MESH_READ_FORMAT: u32 = 1;

/// The base colour every object is drawn with (linear RGBA).
pub const BASE_COLOR: [f32; 4] = [0.8, 0.8, 0.8, 1.0];

/// A version, decoded as the live view decodes it.
#[derive(Debug)]
pub struct Decoded {
    /// The file read.
    pub file: BlendFile,
    /// Its decompressed bytes.
    pub raw: Vec<u8>,
    /// Its scene model.
    pub model: SceneModel,
    /// The live scene of it.
    pub scene: LiveScene,
}

/// One object or camera left out, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// Its live name.
    pub name: String,
    /// The reason's fixed word.
    pub reason: &'static str,
}

/// One object written, as the report lists it (no geometry).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Drawn {
    /// Its id (live name).
    pub name: String,
    /// Its address, `@obj:N`.
    pub key: String,
    /// Its mesh's index.
    pub mesh: u32,
    /// Its mesh's vertices.
    pub vertices: u64,
    /// Its mesh's triangles.
    pub triangles: u64,
    /// Its mesh's content hash.
    pub mesh_hash: String,
    /// `self` or `collection` when hidden.
    pub hidden: Option<&'static str>,
    /// Its modifiers' words.
    pub modifiers: Vec<String>,
    /// Its transform is drawn as stored, not evaluated.
    pub transform_not_evaluated: bool,
}

/// One camera written, as the report lists it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DrawnCamera {
    /// Its id (its object's live name).
    pub name: String,
    /// Its address, `@cam:N`.
    pub key: String,
    /// Its vertical field of view, in radians.
    pub yfov: f32,
    /// Width over height.
    pub aspect: f32,
    /// How its sensor fits.
    pub sensor_fit: &'static str,
    /// The scene's own camera.
    pub active: bool,
}

/// What a read did, beside its output (never inside K2SCENE1).
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct MeshReport {
    /// The scene drawn, by name; `None` when the file has no scene.
    pub scene: Option<String>,
    /// How it was chosen: `current` (the file's current scene), `first` (the current one
    /// missing or naming no scene: the first in file order), `none` (no scene).
    pub scene_from: &'static str,
    /// The objects written, in order.
    pub drawn: Vec<Drawn>,
    /// The cameras written, in order.
    pub cameras: Vec<DrawnCamera>,
    /// The mesh objects and cameras of the scene left out, in file order.
    pub skipped: Vec<Skipped>,
    /// Meshes written.
    pub meshes: u64,
    /// Objects written.
    pub objects: u64,
    /// Of those, hidden.
    pub hidden: u64,
    /// Of those, with a transform something drives (drawn as stored).
    pub transform_not_evaluated: u64,
    /// Of those, with more modifiers than written.
    pub modifiers_truncated: u64,
    /// Objects of the scene that instance a collection (not drawn).
    pub collection_instances: u64,
    /// What triangulating counted.
    pub tri: TriCounts,
    /// The heap the read was worked out to need, in bytes (checked against
    /// [`MeshReadLimits::max_heap_bytes`]).
    pub heap: u64,
}

impl MeshReport {
    /// One line for the scene, one per object and per camera written, one per object or camera
    /// left out: for a person or an agent reading text.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!(
            "scene {} ({}): {} meshes, {} objects, {} cameras, {} hidden, {} left out, {} \
             collection instances not drawn",
            self.scene.as_deref().unwrap_or("-"),
            self.scene_from,
            self.meshes,
            self.objects,
            self.cameras.len(),
            self.hidden,
            self.skipped.len(),
            self.collection_instances
        )];
        for d in &self.drawn {
            let mut l = format!(
                "object {}\t{}\tmesh {}\tvertices {}\ttriangles {}",
                d.name, d.key, d.mesh, d.vertices, d.triangles
            );
            if let Some(h) = d.hidden {
                l.push_str(&format!("\thidden {h}"));
            }
            if !d.modifiers.is_empty() {
                l.push_str(&format!(
                    "\tmodifiers {} (not applied)",
                    d.modifiers.join(",")
                ));
            }
            if d.transform_not_evaluated {
                l.push_str("\ttransform not evaluated");
            }
            out.push(l);
        }
        for c in &self.cameras {
            let mut l = format!(
                "camera {}\t{}\tyfov {}\taspect {}\tsensor fit {}",
                c.name, c.key, c.yfov, c.aspect, c.sensor_fit
            );
            if c.active {
                l.push_str("\tactive");
            }
            out.push(l);
        }
        for s in &self.skipped {
            out.push(format!("left out {}\t{}", s.name, s.reason));
        }
        out
    }
}

/// A read's output.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshRead {
    /// The K2SCENE1 bytes.
    pub k2scene: Vec<u8>,
    /// The report.
    pub report: MeshReport,
}

/// Decodes `bytes` as the live view does, under `limits.live`.
pub fn decode(bytes: &[u8], limits: &MeshReadLimits) -> Result<Decoded, MeshReadError> {
    let live = &limits.live;
    at_most("stored bytes", bytes.len() as u64, live.max_file_bytes)?;
    let declined = |d: k2_mpc_base::session::Declined| MeshReadError::Declined { code: d.code };
    let (how, raw) = decompress(bytes, &live.read)
        .map_err(model::read_declined)
        .map_err(declined)?;
    let file = read_blend(&mut &raw[..], &live.read)
        .map_err(model::read_declined)
        .map_err(declined)?;
    if !crate::scene::reads(file.header.version) {
        return Err(MeshReadError::Declined {
            code: "live_version",
        });
    }
    let model = read_scene(&file)
        .map_err(model::read_declined)
        .map_err(declined)?;
    let scene = model::from_model(&file, &raw, &model, how).map_err(declined)?;
    Ok(Decoded {
        file,
        raw,
        model,
        scene,
    })
}

/// The read of `bytes` under `limits`, its objects keyed as an import of the bytes mints:
/// its K2SCENE1 and a report, or a typed refusal.
pub fn mesh_read(bytes: &[u8], limits: &MeshReadLimits) -> Result<MeshRead, MeshReadError> {
    mesh_read_paired(bytes, None, limits)
}

/// The read of `bytes` under `limits`, its objects keyed by `pairing` (the carrier's: the live
/// keys of the version's objects, in the decode's order), or as an import mints them without
/// one. A pairing that doesn't fit the decode is refused.
pub fn mesh_read_paired(
    bytes: &[u8],
    pairing: Option<&Pairing>,
    limits: &MeshReadLimits,
) -> Result<MeshRead, MeshReadError> {
    let d = decode(bytes, limits)?;
    read_decoded_paired(&d, pairing, limits)
}

/// The scene the file's global block names as current, else the first in file order: its
/// index in the model and how it was chosen.
pub fn choose_scene(
    file: &BlendFile,
    raw: &[u8],
    model: &SceneModel,
) -> (Option<usize>, &'static str) {
    let scenes: Vec<usize> = model
        .ids
        .iter()
        .enumerate()
        .filter(|(_, id)| matches!(id.detail, Detail::Scene { .. }))
        .map(|(i, _)| i)
        .collect();
    let Some(&first) = scenes.first() else {
        return (None, "none");
    };
    if mutant::active("SceneChoiceFirst") {
        return (Some(first), "current");
    }
    let current = file.blocks.iter().enumerate().find_map(|(g, b)| {
        if &b.head.code != b"GLOB" || file.sdna.struct_name(b.head.sdna) != Some("FileGlobal") {
            return None;
        }
        let f = file.sdna.field(b.head.sdna, "curscene")?;
        if f.pointer_depth != 1 || f.array_len != 1 {
            return None;
        }
        let body = mesh::block_body(file, raw, g as u32)?;
        let p = file.endian().ptr(body, f.offset as usize)?;
        let Target::Block(j) = file.resolve(p, g as u32) else {
            return None;
        };
        scenes.iter().copied().find(|&s| model.ids[s].block == j)
    });
    match current {
        Some(s) => (Some(s), "current"),
        None => (Some(first), "first"),
    }
}

/// The collections holding live object `o`, by their place in the live scene's collections.
pub fn holders(o: &LiveObj) -> Vec<usize> {
    std::iter::once(o.coll)
        .chain(o.also.iter().copied())
        .collect()
}

/// The walk of the scene drawn's collections ([`HiddenWalk`]), numbered as the live scene's
/// collections, from model scene `s`'s master collection.
pub fn scene_walk(model: &SceneModel, scene: &LiveScene, s: usize) -> Option<HiddenWalk> {
    let master = coll_blocks(model).iter().position(|&b| b == s)?;
    let children = coll_children(model);
    Some(HiddenWalk::new(master, &children, &|c| {
        scene.colls.get(c).is_some_and(|x| x.hidden)
    }))
}

/// The modifiers of object block `ob`, by the file's own struct names (the suffix
/// `ModifierData` taken off), in list order: at most [`MAX_MODIFIERS`], each at most
/// [`MAX_MODIFIER_BYTES`]; and whether there were more.
fn modifiers(file: &BlendFile, raw: &[u8], ob: u32) -> (Vec<String>, bool) {
    let sd = &file.sdna;
    let e = file.endian();
    let mut out = Vec::new();
    let s = file.blocks[ob as usize].head.sdna;
    let Some(f) = sd.field(s, "modifiers") else {
        return (out, false);
    };
    if f.is_pointer() || sd.type_name(f.type_index) != "ListBase" {
        return (out, false);
    }
    let Some(body) = mesh::block_body(file, raw, ob) else {
        return (out, false);
    };
    let (mut p, mut from) = (e.ptr(body, f.offset as usize).unwrap_or(0), ob);
    let mut seen = HashSet::new();
    let mut more = false;
    while p != 0 && seen.len() < LIST_MAX {
        let Target::Block(j) = file.resolve(p, from) else {
            break;
        };
        if !seen.insert(j) {
            break;
        }
        let js = file.blocks[j as usize].head.sdna;
        let Some(name) = sd.struct_name(js) else {
            break;
        };
        if out.len() == MAX_MODIFIERS {
            more = true;
            break;
        }
        let word = name.strip_suffix("ModifierData").filter(|w| !w.is_empty());
        let mut word = word.unwrap_or(name).to_owned();
        if word.len() > MAX_MODIFIER_BYTES {
            let mut cut = MAX_MODIFIER_BYTES;
            while !word.is_char_boundary(cut) {
                cut -= 1;
            }
            word.truncate(cut);
        }
        out.push(word);
        let next = sd
            .field_path(js, "modifier.next")
            .map(|(at, _)| at)
            .or_else(|| sd.field(js, "next").map(|f| f.offset));
        let (Some(at), Some(jb)) = (next, mesh::block_body(file, raw, j)) else {
            break;
        };
        p = e.ptr(jb, at as usize).unwrap_or(0);
        from = j;
    }
    (out, more)
}

/// The world matrix object block `ob` caches, by the names files give it (a test oracle and
/// the planted bug only).
fn cached_world(file: &BlendFile, raw: &[u8], ob: u32) -> Option<Mat4> {
    let s = file.blocks[ob as usize].head.sdna;
    let body = mesh::block_body(file, raw, ob)?;
    let (at, f) = ["obmat", "object_to_world", "runtime.object_to_world"]
        .iter()
        .find_map(|p| file.sdna.field_path(s, p))?;
    if f.is_pointer() || file.sdna.type_name(f.type_index) != "float" || f.array_len != 16 {
        return None;
    }
    let e = file.endian();
    let mut m = [0.0f64; 16];
    for (i, x) in m.iter_mut().enumerate() {
        *x = f64::from(e.f32(body, at as usize + 4 * i)?);
    }
    Some(m)
}

/// Whether object block `ob` instances a collection (`instance_collection`, `dup_group`).
fn instances_collection(file: &BlendFile, raw: &[u8], ob: u32) -> bool {
    let s = file.blocks[ob as usize].head.sdna;
    let Some(f) = file
        .sdna
        .field_any(s, &["instance_collection", "dup_group"])
        .filter(|f| f.pointer_depth == 1 && f.array_len == 1)
    else {
        return false;
    };
    mesh::block_body(file, raw, ob)
        .and_then(|b| file.endian().ptr(b, f.offset as usize))
        .is_some_and(|p| p != 0)
}

/// The drawn scene's render size (`r.xsch`, `r.ysch`, `r.xasp`, `r.yasp`) from scene block `sc`.
fn render_of(file: &BlendFile, raw: &[u8], sc: u32) -> Option<Render> {
    let s = file.blocks.get(sc as usize)?.head.sdna;
    let body = mesh::block_body(file, raw, sc)?;
    let e = file.endian();
    let num = |path: &str, ty: &str| -> Option<f64> {
        let (at, f) = file.sdna.field_path(s, path)?;
        if f.is_pointer() || f.array_len != 1 || file.sdna.type_name(f.type_index) != ty {
            return None;
        }
        match ty {
            "int" => e.i32(body, at as usize).map(f64::from),
            _ => e.f32(body, at as usize).map(f64::from),
        }
    };
    Some(Render {
        xsch: num("r.xsch", "int")?,
        ysch: num("r.ysch", "int")?,
        xasp: num("r.xasp", "float")?,
        yasp: num("r.yasp", "float")?,
    })
}

/// A camera's data from its record's fields (`type`, `lens`, `sensor_x`, `sensor_y`,
/// `sensor_fit`, `clipsta`); `None` when one is missing.
fn camera_data(fields: &[(String, String)]) -> Option<CameraData> {
    let get = |n: &str| fields.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
    let int = |n: &str| get(n)?.parse::<i64>().ok();
    let float = |n: &str| get(n)?.parse::<f32>().ok();
    Some(CameraData {
        kind: int("type")?,
        lens: float("lens")?,
        sensor: [float("sensor_x")?, float("sensor_y")?],
        fit: int("sensor_fit")?,
        clipsta: float("clipsta")?,
    })
}

/// The keys of the decode's objects: `pairing`'s for `objs` when given (checked: as many as the
/// objects, each `oN`, none twice), else the ones an import mints.
fn object_keys(d: &Decoded, pairing: Option<&Pairing>) -> Result<Vec<Key>, MeshReadError> {
    let Some(p) = pairing else {
        return Ok(d.scene.minted_keys().1);
    };
    let keys = p.keys(OBJS).ok_or(MeshReadError::Pairing {
        what: "no keys for the objects",
    })?;
    check_object_keys(keys, d.scene.objs.len())?;
    Ok(keys.to_vec())
}

/// The checks on a pairing's object keys for a decode of `objects` objects: as many keys, each
/// `oN`, none twice.
pub(crate) fn check_object_keys(keys: &[Key], objects: usize) -> Result<(), MeshReadError> {
    let bad = |what| MeshReadError::Pairing { what };
    if keys.len() != objects {
        return Err(bad("the object keys don't fit the objects"));
    }
    let mut seen = HashSet::new();
    for k in keys {
        if key_num(k.as_str(), 'o').is_none() {
            return Err(bad("an object key isn't oN"));
        }
        if !seen.insert(k) {
            return Err(bad("an object key is given twice"));
        }
    }
    Ok(())
}

/// What the read takes of one object from the file, beside what the live store holds of it
/// (its name, key, holders, own flag and transform). Kept per carrier ([`CarrierRead`]), so a
/// live state's K2SCENE1 is written without the file.
#[derive(Debug, Clone)]
pub(crate) struct FileObj {
    /// Linked from a library.
    pub library: bool,
    /// It instances a collection.
    pub instances: bool,
    /// A mesh object's `Mesh` block, or why it has none the read draws (`no_mesh`,
    /// `linked_mesh`).
    pub mesh: Result<u32, &'static str>,
    /// A camera object's data, when its record gives it.
    pub camera: Option<CameraData>,
    /// The scene drawn's own camera.
    pub scene_camera: bool,
    /// A mesh object's modifiers.
    pub modifiers: Vec<String>,
    /// It has more modifiers than those.
    pub more_modifiers: bool,
}

/// What the read takes of each of the decode's objects from the file (`obj_ids` their places
/// in the model; `scene_camera` the scene drawn's camera, by its place in the model).
fn file_objs(d: &Decoded, obj_ids: &[usize], scene_camera: Option<usize>) -> Vec<FileObj> {
    let (file, raw, ids) = (&d.file, &d.raw[..], &d.model.ids);
    let records = d.model.cameras();
    let record_of: HashMap<&str, usize> = records
        .iter()
        .enumerate()
        .map(|(i, c)| (c.object.as_str(), i))
        .collect();
    obj_ids
        .iter()
        .enumerate()
        .map(|(oi, &mi)| {
            let o = &d.scene.objs[oi];
            let id = &ids[mi];
            let is_mesh = o.kind == "mesh";
            let mesh = match &id.detail {
                _ if !is_mesh => Err("no_mesh"),
                Detail::Object { data: Some(di), .. } => {
                    if ids[*di].struct_name != "Mesh" || ids[*di].library.is_some() {
                        Err("linked_mesh")
                    } else {
                        Ok(ids[*di].block)
                    }
                }
                _ => Err("no_mesh"),
            };
            let (modifiers, more_modifiers) = if is_mesh {
                modifiers(file, raw, id.block)
            } else {
                (Vec::new(), false)
            };
            let camera = if o.kind == "camera" {
                let object_key = id.key.clone().unwrap_or_else(|| format!("#{mi}"));
                record_of
                    .get(object_key.as_str())
                    .and_then(|&r| camera_data(&records[r].fields))
            } else {
                None
            };
            FileObj {
                library: id.library.is_some(),
                instances: instances_collection(file, raw, id.block),
                mesh,
                camera,
                scene_camera: scene_camera == Some(mi),
                modifiers,
                more_modifiers,
            }
        })
        .collect()
}

/// One object to write, before its mesh is known to read.
pub(crate) struct Candidate {
    /// Its place among the live scene's objects.
    obj: usize,
    name: String,
    key: String,
    mesh_block: u32,
    transform: [f32; 16],
    hidden: Option<Hidden>,
    modifiers: Vec<String>,
    more_modifiers: bool,
    driven: bool,
}

/// One object as the read takes it: its place among the decode's objects (`oi`), its live name,
/// its key's number, the collections holding it (by their place in the decode's collections)
/// and its own viewport flag; the decode's object (its type, its parent and its placement's
/// facts) and what the read takes of it from the file.
pub(crate) struct ObjIn<'a> {
    pub oi: usize,
    pub name: &'a str,
    pub num: u64,
    pub holders: Vec<usize>,
    pub hidden: bool,
    pub live: &'a LiveObj,
    pub file: &'a FileObj,
}

/// The objects and cameras of the scene drawn, gathered before any mesh is read.
#[derive(Default)]
pub(crate) struct Gathered {
    cands: Vec<Candidate>,
    cams: Vec<SceneCamera>,
    /// Each camera's place among the decode's objects.
    cam_of: Vec<usize>,
    left: Vec<LeftOut>,
    skipped: Vec<Skipped>,
    collection_instances: u64,
}

/// An object's world matrix, or why it has none the read draws: its delta transforms, its
/// parenting, then its placement through its parents' (`places`, by address) by the kit's
/// `world_matrix`; `stale` in its place under a plant.
pub(crate) fn world_of(
    o: &LiveObj,
    address: &str,
    places: &HashMap<String, Option<Placement>>,
    stale: Option<Mat4>,
) -> Result<Mat4, &'static str> {
    if !o.place.deltas_identity {
        return Err("delta_transform");
    }
    match &o.parent {
        Parent::Other(_) => return Err("parent_unlisted"),
        Parent::Obj(_) if o.place.partype != Some(0) => return Err("parent_not_object"),
        _ => {}
    }
    let w = stale.or_else(|| {
        world_matrix(address, |a| match places.get(a) {
            Some(Some(p)) => Link::Placed(p),
            Some(None) => Link::Unplaced,
            None => Link::NotShown,
        })
    });
    let w = w.ok_or("transform_unplaced")?;
    if !w.iter().all(|x| x.is_finite() && (*x as f32).is_finite()) {
        return Err("transform_not_finite");
    }
    Ok(w)
}

/// The mesh objects and cameras of `objs` the scene drawn holds (`walk`), in their order: each
/// placed (`placed`), hidden by the shared rule, its mesh's block named; each camera's view
/// from its world matrix, its data and the render size; what is left out, and why.
pub(crate) fn gather<'a>(
    objs: impl IntoIterator<Item = ObjIn<'a>>,
    walk: Option<&HiddenWalk>,
    render: Option<&Render>,
    placed: &dyn Fn(&ObjIn<'a>) -> Result<Mat4, &'static str>,
) -> Gathered {
    let mut cands: Vec<Candidate> = Vec::new();
    let mut cams: Vec<SceneCamera> = Vec::new();
    let mut cam_of: Vec<usize> = Vec::new();
    let mut left: Vec<LeftOut> = Vec::new();
    let mut skipped: Vec<Skipped> = Vec::new();
    let mut collection_instances = 0u64;
    for x in objs {
        let o = x.live;
        if !walk.is_some_and(|w| w.holds(&x.holders)) {
            continue;
        }
        if x.file.instances {
            collection_instances += 1;
        }
        let is_camera = o.kind == "camera";
        if o.kind != "mesh" && !is_camera {
            continue;
        }
        let (num, name) = (x.num, x.name);
        let mut skip = |reason: &'static str| {
            if is_camera && !mutant::active("LeftOutUnlisted") {
                left.push(LeftOut {
                    name: name.to_owned(),
                    key: (num > 0).then(|| format!("@cam:{num}")),
                    reason: reason.to_owned(),
                });
            }
            skipped.push(Skipped {
                name: name.to_owned(),
                reason,
            })
        };
        if x.file.library {
            skip("linked_object");
            continue;
        }
        if is_camera {
            let w = match placed(&x) {
                Ok(w) => w,
                Err(r) => {
                    skip(r);
                    continue;
                }
            };
            let Some(data) = x.file.camera else {
                skip("camera_no_data");
                continue;
            };
            match camera::camera(&w, &data, render) {
                Ok(l) => {
                    let key = if mutant::active("CameraKeyFromCameraIndex") {
                        // planted: the camera's key from its place among the cameras
                        format!("@cam:{}", cams.len() + 1)
                    } else {
                        format!("@cam:{num}")
                    };
                    cams.push(SceneCamera {
                        id: name.to_owned(),
                        key,
                        position: l.position,
                        target: None,
                        forward: Some(l.forward),
                        up: l.up,
                        yfov: l.yfov,
                        aspect: Some(l.aspect),
                        znear: Some(l.znear),
                        active: Some(x.file.scene_camera),
                        lens_mm: Some(l.lens_mm),
                        sensor_mm: Some(l.sensor_mm),
                        sensor_fit: Some(l.fit),
                    });
                    cam_of.push(x.oi);
                }
                Err(r) => skip(r),
            }
            continue;
        }
        let mesh_block = match x.file.mesh {
            Ok(b) => b,
            Err(r) => {
                skip(r);
                continue;
            }
        };
        let w = match placed(&x) {
            Ok(w) => w,
            Err(r) => {
                skip(r);
                continue;
            }
        };
        let hidden = match walk.and_then(|w| w.of(x.hidden, &x.holders)) {
            Some(HiddenBy::Itself) => Some(Hidden::Itself),
            Some(HiddenBy::Collection(_)) => Some(Hidden::Collection),
            None => None,
        };
        if hidden.is_some() && mutant::active("HiddenDropped") {
            continue;
        }
        cands.push(Candidate {
            obj: x.oi,
            name: name.to_owned(),
            key: format!("@obj:{num}"),
            mesh_block,
            transform: w.map(|x| x as f32),
            hidden,
            modifiers: x.file.modifiers.clone(),
            more_modifiers: x.file.more_modifiers,
            driven: o.place.driven,
        });
    }
    Gathered {
        cands,
        cams,
        cam_of,
        left,
        skipped,
        collection_instances,
    }
}

/// The mesh blocks the candidates use, each once, in file order, with the counts of objects,
/// cameras and meshes checked against `limits`.
fn bounded_blocks(g: &Gathered, limits: &MeshReadLimits) -> Result<BTreeSet<u32>, MeshReadError> {
    at_most("objects", g.cands.len() as u64, limits.max_objects)?;
    at_most("cameras", g.cams.len() as u64, limits.max_cameras)?;
    let blocks: BTreeSet<u32> = g.cands.iter().map(|c| c.mesh_block).collect();
    at_most("meshes", blocks.len() as u64, limits.max_meshes)?;
    Ok(blocks)
}

/// The heap the decoded file takes as it is held: the decompressed bytes at their capacity, the
/// blocks and the bodies kept, the schema, the model and the live scene, a generous figure per
/// element ([`heap_estimate`]'s first part).
fn decoded_heap(d: &Decoded) -> u64 {
    let file = &d.file;
    let n = |x: usize| x as u64;
    let kept: u64 = (0..file.blocks.len())
        .filter_map(|i| file.body(i as u32))
        .map(|b| n(b.len()))
        .sum();
    let sdna: u64 = file
        .blocks
        .iter()
        .filter(|b| &b.head.code == b"DNA1")
        .map(|b| b.head.len)
        .sum();
    n(d.raw.capacity())
        + kept
        + 2 * sdna
        + n(file.blocks.len()) * (n(std::mem::size_of::<Block>()) + 96)
        + n(d.model.ids.len()) * (n(std::mem::size_of::<IdBlock>()) + 2048)
        + n(d.scene.objs.len() + d.scene.colls.len()) * 4096
}

/// The heap a read needs at most, in bytes, worked out before any mesh array or the output is
/// allocated: the decoded file as it is held (`decoded`, [`decoded_heap`]), the objects and
/// cameras gathered, every mesh as written (its vertices as stored: the loose ones are only
/// dropped while reading), the largest mesh's reading scratch (its arrays, its triangles, the
/// compaction's map and renumbered triangles), and the output (the header at twice its largest
/// text, and the data).
fn heap_estimate(
    decoded: u64,
    cands: &[Candidate],
    cams: &[SceneCamera],
    counts: &BTreeMap<u32, MeshCounts>,
) -> u64 {
    let n = |x: usize| x as u64;
    let words = |v: &[String]| v.iter().map(|m| n(m.len()) + 32).sum::<u64>();
    let gathered: u64 = cands
        .iter()
        .map(|c| 512 + n(c.name.len() + c.key.len()) + words(&c.modifiers))
        .sum::<u64>()
        + n(cams.len()) * 512;
    let header = 2
        * (256
            + cands
                .iter()
                .map(|c| 700 + 6 * n(c.name.len() + c.key.len()) + 6 * words(&c.modifiers))
                .sum::<u64>()
            + n(counts.len()) * 200
            + cams
                .iter()
                .map(|c| 900 + 6 * n(c.id.len() + c.key.len()))
                .sum::<u64>());
    let (mut meshes, mut scratch) = (0u64, 0u64);
    for c in counts.values() {
        let (v, f, k, t) = (
            u64::from(c.vertices),
            u64::from(c.faces),
            u64::from(c.corners),
            c.triangles(),
        );
        meshes += 12 * v + 12 * t + 64;
        scratch = scratch.max(16 * v + 4 * k + 8 * f + 24 * t);
    }
    decoded + gathered + meshes + scratch + header + meshes + 16
}

/// The objects written from the candidates, in order, and each one's place among the decode's
/// objects: each with its mesh's index (`mesh_of`: a block's mesh index, or the word its mesh
/// is skipped by; `None` passes the object over), the report's counts and skips kept as it goes.
fn objects_of(
    cands: Vec<Candidate>,
    mesh_of: &mut dyn FnMut(u32) -> Option<Result<u32, &'static str>>,
    report: &mut MeshReport,
) -> (Vec<SceneObject>, Vec<usize>) {
    let mut objects = Vec::new();
    let mut of_obj = Vec::new();
    for c in cands {
        let mesh = match mesh_of(c.mesh_block) {
            Some(Ok(i)) => i,
            Some(Err(word)) => {
                report.skipped.push(Skipped {
                    name: c.name,
                    reason: word,
                });
                continue;
            }
            None => continue,
        };
        report.hidden += u64::from(c.hidden.is_some());
        report.transform_not_evaluated += u64::from(c.driven);
        report.modifiers_truncated += u64::from(c.more_modifiers);
        let key = if mutant::active("KeyFromPosition") {
            // planted: the key from the object's place among the objects written
            format!("@obj:{}", objects.len() + 1)
        } else {
            c.key
        };
        of_obj.push(c.obj);
        objects.push(SceneObject {
            id: c.name,
            key,
            mesh,
            base_color: BASE_COLOR,
            double_sided: None,
            transform: c.transform,
            modifiers_not_applied: !c.modifiers.is_empty(),
            modifiers: c.modifiers,
            transform_not_evaluated: c.driven,
            hidden: c.hidden,
        });
    }
    (objects, of_obj)
}

/// The read of a decoded version under `limits`, its objects keyed as an import mints.
pub fn read_decoded(d: &Decoded, limits: &MeshReadLimits) -> Result<MeshRead, MeshReadError> {
    read_decoded_paired(d, None, limits)
}

/// The read of a decoded version under `limits`, its objects keyed by `pairing` when given.
pub fn read_decoded_paired(
    d: &Decoded,
    pairing: Option<&Pairing>,
    limits: &MeshReadLimits,
) -> Result<MeshRead, MeshReadError> {
    read_inner(d, pairing, limits).map(|(r, _)| r)
}

/// What [`read_inner`] gives beside its output: what a carrier keeps of its read
/// ([`CarrierRead`]).
pub(crate) struct ReadParts {
    /// Each written object's place among the live scene's objects, with its mesh's hash.
    pub by_obj: Vec<(usize, String)>,
    files: Vec<FileObj>,
    render: Option<Render>,
    blocks: HashMap<u32, Result<u32, &'static str>>,
    counts: HashMap<u32, MeshCounts>,
    decoded_heap: u64,
    meshes: Vec<MeshHead>,
    objects: Vec<(usize, SceneObject)>,
    cameras: Vec<(usize, SceneCamera)>,
}

/// [`read_decoded_paired`], and what a carrier keeps of the read ([`ReadParts`]).
pub(crate) fn read_inner(
    d: &Decoded,
    pairing: Option<&Pairing>,
    limits: &MeshReadLimits,
) -> Result<(MeshRead, ReadParts), MeshReadError> {
    let (file, raw, ids) = (&d.file, &d.raw[..], &d.model.ids);
    let mut report = MeshReport::default();
    let obj_ids: Vec<usize> = ids
        .iter()
        .enumerate()
        .filter(|(_, id)| matches!(id.detail, Detail::Object { .. }) && id.struct_name == "Object")
        .map(|(i, _)| i)
        .collect();
    if obj_ids.len() != d.scene.objs.len() {
        return Err(MeshReadError::Invalid {
            what: "the live scene's objects aren't the model's",
        });
    }
    let ok = object_keys(d, pairing)?;
    let (scene_i, from) = choose_scene(file, raw, &d.model);
    report.scene_from = from;
    report.scene = scene_i.map(|s| ids[s].name.clone());
    let walk = scene_i.and_then(|s| scene_walk(&d.model, &d.scene, s));
    let render = scene_i.and_then(|s| render_of(file, raw, ids[s].block));
    let scene_camera = scene_i.and_then(|s| match &ids[s].detail {
        Detail::Scene { camera, .. } => *camera,
        _ => None,
    });
    let files = file_objs(d, &obj_ids, scene_camera);

    // every object's placement, by address, as the live view places it
    let (ck, _) = d.scene.minted_keys();
    let content = d.scene.to_content(&ck, &ok, d.scene.minted_marks());
    let empty = BTreeMap::new();
    let props = content
        .iter()
        .find_map(|(r, x)| match x {
            Neutral::Map(m) if r == OBJ_PROPS => Some(m),
            _ => None,
        })
        .unwrap_or(&empty);
    let mut places: HashMap<String, Option<Placement>> = HashMap::new();
    for (o, k) in d.scene.objs.iter().zip(&ok) {
        let mut p = place::placement(&place::ObjFields::of(props, k), &o.place);
        if let Some(p) = &mut p {
            p.precision = Precision::F32;
        }
        places.insert(address_of(OBJS, k), p);
    }
    let placed = |x: &ObjIn<'_>| -> Result<Mat4, &'static str> {
        let stale = if mutant::active("StoredMatrixUsed") {
            cached_world(file, raw, ids[obj_ids[x.oi]].block)
        } else {
            None
        };
        world_of(x.live, &address_of(OBJS, &ok[x.oi]), &places, stale)
    };
    let objs = d.scene.objs.iter().enumerate().map(|(oi, o)| ObjIn {
        oi,
        name: &o.name,
        num: key_num(ok[oi].as_str(), 'o').unwrap_or(0),
        holders: holders(o),
        hidden: o.hide[0],
        live: o,
        file: &files[oi],
    });
    let g = gather(objs, walk.as_ref(), render.as_ref(), &placed);

    // bounds before any array: objects, cameras, meshes, then the heap from their counts
    let blocks = bounded_blocks(&g, limits)?;
    let mut counts = BTreeMap::new();
    for &b in &blocks {
        if let Ok(c) = mesh::counts(file, raw, b) {
            counts.insert(b, c);
        }
    }
    let decoded = decoded_heap(d);
    report.heap = heap_estimate(decoded, &g.cands, &g.cams, &counts);
    if !mutant::active("HeapUnchecked") {
        at_most("heap", report.heap, limits.max_heap_bytes)?;
    }
    if mutant::active("LimitBeforeCompaction") {
        // planted: the stored vertices counted, the loose ones with them
        let v = counts.values().map(|c| u64::from(c.vertices)).sum();
        at_most("vertices", v, limits.max_vertices)?;
    }
    let Gathered {
        cands,
        cams,
        cam_of,
        left,
        skipped,
        collection_instances,
    } = g;
    report.skipped = skipped;
    report.collection_instances = collection_instances;

    // the meshes, each once, in their file order
    let mut meshes: Vec<SceneMesh> = Vec::new();
    let mut index: HashMap<u32, Result<u32, &'static str>> = HashMap::new();
    for &b in &blocks {
        let got = mesh::read_mesh(file, raw, b, limits.ear_clip_max_corners, &mut report.tri);
        index.insert(
            b,
            got.map(|(_, m)| {
                meshes.push(m);
                (meshes.len() - 1) as u32
            })
            .map_err(|e| e.word()),
        );
    }
    let (mut objects, of_obj) = objects_of(
        cands,
        &mut |b| {
            let got = index.get(&b).copied()?;
            Some(got.map(|i| {
                if mutant::active("InstancesDuplicated") {
                    meshes.push(meshes[i as usize].clone());
                    (meshes.len() - 1) as u32
                } else {
                    i
                }
            }))
        },
        &mut report,
    );
    // a mesh no written object uses (all its objects skipped) isn't written
    let used: BTreeSet<u32> = objects.iter().map(|o| o.mesh).collect();
    let mut renumber: HashMap<u32, u32> = (0..meshes.len() as u32).map(|i| (i, i)).collect();
    if used.len() != meshes.len() {
        renumber.clear();
        let mut kept = Vec::new();
        for (i, m) in meshes.into_iter().enumerate() {
            if used.contains(&(i as u32)) {
                renumber.insert(i as u32, kept.len() as u32);
                kept.push(m);
            }
        }
        meshes = kept;
        for o in &mut objects {
            o.mesh = renumber[&o.mesh];
        }
    }
    report.meshes = meshes.len() as u64;
    report.objects = objects.len() as u64;
    let scene = Scene {
        meshes,
        objects,
        cameras: cams,
        cameras_left_out: left,
    };
    let (k2scene, hashes) = k2scene::write_hashed(&scene, limits)?;
    report.drawn = scene
        .objects
        .iter()
        .map(|o| {
            let m = &scene.meshes[o.mesh as usize];
            Drawn {
                name: o.id.clone(),
                key: o.key.clone(),
                mesh: o.mesh,
                vertices: m.positions.len() as u64,
                triangles: m.triangles.len() as u64,
                mesh_hash: hashes[o.mesh as usize].clone(),
                hidden: o.hidden.map(Hidden::word),
                modifiers: o.modifiers.clone(),
                transform_not_evaluated: o.transform_not_evaluated,
            }
        })
        .collect();
    report.cameras = scene
        .cameras
        .iter()
        .map(|c| DrawnCamera {
            name: c.id.clone(),
            key: c.key.clone(),
            yfov: c.yfov,
            aspect: c.aspect.unwrap_or(0.0),
            sensor_fit: c.sensor_fit.map_or("-", |f| f.word()),
            active: c.active == Some(true),
        })
        .collect();
    let by_obj = of_obj
        .iter()
        .zip(&scene.objects)
        .map(|(&oi, o)| (oi, hashes[o.mesh as usize].clone()))
        .collect();
    // each block read: its mesh's index in the output, or its skip word
    let blocks = index
        .into_iter()
        .filter_map(|(b, got)| match got {
            Ok(i) => renumber.get(&i).map(|&j| (b, Ok(j))),
            Err(w) => Some((b, Err(w))),
        })
        .collect();
    let mesh_heads = scene
        .meshes
        .iter()
        .zip(&hashes)
        .map(|(m, h)| MeshHead {
            vertices: m.positions.len() as u64,
            triangles: m.triangles.len() as u64,
            hash: h.clone(),
        })
        .collect();
    let Scene {
        objects, cameras, ..
    } = scene;
    let parts = ReadParts {
        by_obj,
        files,
        render,
        blocks,
        counts: counts.into_iter().collect(),
        decoded_heap: decoded,
        meshes: mesh_heads,
        objects: of_obj.into_iter().zip(objects).collect(),
        cameras: cam_of.into_iter().zip(cameras).collect(),
    };
    Ok((MeshRead { k2scene, report }, parts))
}

/// What a carrier keeps of its mesh read, to write a live state's K2SCENE1 without reading the
/// file again ([`crate::live::reuse`]): the read's data section, shared and never copied, with
/// its content id; each mesh's counts and hash; each mesh block the read read, with its mesh's
/// index or the word it is skipped by, and its stored counts; the decoded file's heap figure;
/// what the read takes of each object from the file; the render size; the read's objects and
/// cameras by their place among the decode's objects; the decode's objects and collections as
/// the live store holds them. Built once per carrier ([`crate::live::place::CarrierView`]);
/// the whole-file decode isn't kept.
pub struct CarrierRead {
    pub(crate) data: Arc<[u8]>,
    pub(crate) id: String,
    pub(crate) meshes: Vec<MeshHead>,
    pub(crate) blocks: HashMap<u32, Result<u32, &'static str>>,
    pub(crate) counts: HashMap<u32, MeshCounts>,
    pub(crate) decoded_heap: u64,
    pub(crate) files: Vec<FileObj>,
    pub(crate) render: Option<Render>,
    /// The read's objects, in order, each with its place among the decode's objects.
    pub(crate) objects: Vec<(usize, SceneObject)>,
    /// The read's cameras, the same way.
    pub(crate) cameras: Vec<(usize, SceneCamera)>,
    /// The decode's objects, in its order (the order the carrier's pairing keys).
    pub(crate) objs: Vec<LiveObj>,
    /// The decode's collections, the same way.
    pub(crate) colls: Vec<LiveColl>,
}

impl std::fmt::Debug for CarrierRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CarrierRead({} data bytes, {} meshes, {} objects, {} cameras)",
            self.data.len(),
            self.meshes.len(),
            self.objects.len(),
            self.cameras.len()
        )
    }
}

/// The content id of a data section: SHA-256 of its bytes, lowercase hex.
pub fn data_id(data: &[u8]) -> String {
    let mut d = k2_mpc_base::index::Digest::stream();
    if mutant::active("CarrierIdStale") {
        // planted: the id from the data's length, not its bytes
        d.update(&(data.len() as u64).to_le_bytes());
    } else {
        d.update(data);
    }
    let mut s = String::with_capacity(64);
    for b in d.finish().as_bytes() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// The read of a carrier `d` (keyed as an import mints: the data section doesn't depend on the
/// keys) under `limits`, kept for the reuse ([`CarrierRead`]), with each written object's mesh
/// hash by its place among the decode's objects; `None` when the read refuses.
pub(crate) fn carrier_read(
    d: &Decoded,
    limits: &MeshReadLimits,
) -> Option<(CarrierRead, Vec<(usize, String)>)> {
    let (read, parts) = read_inner(d, None, limits).ok()?;
    let out = read.k2scene;
    let hlen = u32::from_le_bytes(out.get(8..12)?.try_into().ok()?);
    let at = usize::try_from(k2scene::align4(k2scene::PREFIX + u64::from(hlen))).ok()?;
    let data: Arc<[u8]> = Arc::from(out.get(at..)?);
    drop(out);
    let id = data_id(&data);
    let ReadParts {
        by_obj,
        files,
        render,
        blocks,
        counts,
        decoded_heap,
        meshes,
        objects,
        cameras,
    } = parts;
    Some((
        CarrierRead {
            data,
            id,
            meshes,
            blocks,
            counts,
            decoded_heap,
            files,
            render,
            objects,
            cameras,
            objs: d.scene.objs.clone(),
            colls: d.scene.colls.clone(),
        },
        by_obj,
    ))
}

/// Why a live state's K2SCENE1 can't be written over its carrier's read.
#[derive(Debug)]
pub(crate) enum RewriteError {
    /// A drawn object's mesh is one the carrier's read didn't read (its address).
    MeshNotHeld(String),
    /// The meshes the output would hold aren't the carrier's.
    MeshesChanged,
    /// A limit, or a writer check.
    Read(MeshReadError),
}

impl From<MeshReadError> for RewriteError {
    fn from(e: MeshReadError) -> Self {
        RewriteError::Read(e)
    }
}

/// What [`rewrite`] writes: the first part of the K2SCENE1 (to be followed by the carrier's data
/// section), and its objects and cameras, each with its place among the decode's objects.
pub(crate) struct Rewritten {
    pub header: Vec<u8>,
    pub objects: Vec<(usize, SceneObject)>,
    pub cameras: Vec<(usize, SceneCamera)>,
}

/// The first part of the K2SCENE1 of gathered objects and cameras `g` over the carrier's read
/// `cr`, through the read's own steps: the same bounds and heap figure (the carrier's decoded
/// file's, with the gathered objects' and the meshes' counts), each object's mesh from the
/// blocks the carrier's read read, the same writer checks and header text. Refused when a drawn
/// object's mesh isn't one the carrier's read read, or the meshes written wouldn't be exactly
/// the carrier's (the data section would differ).
pub(crate) fn rewrite(
    g: Gathered,
    cr: &CarrierRead,
    limits: &MeshReadLimits,
) -> Result<Rewritten, RewriteError> {
    let blocks = bounded_blocks(&g, limits)?;
    if let Some(c) = g
        .cands
        .iter()
        .find(|c| !cr.blocks.contains_key(&c.mesh_block))
    {
        return Err(RewriteError::MeshNotHeld(c.key.clone()));
    }
    let counts: BTreeMap<u32, MeshCounts> = blocks
        .iter()
        .filter_map(|b| cr.counts.get(b).map(|c| (*b, *c)))
        .collect();
    let heap = heap_estimate(cr.decoded_heap, &g.cands, &g.cams, &counts);
    if !mutant::active("HeapUnchecked") {
        at_most("heap", heap, limits.max_heap_bytes)?;
    }
    let Gathered {
        cands,
        cams,
        cam_of,
        left,
        ..
    } = g;
    let mut scratch = MeshReport::default();
    let (objects, of_obj) = objects_of(cands, &mut |b| cr.blocks.get(&b).copied(), &mut scratch);
    let used: BTreeSet<u32> = objects.iter().map(|o| o.mesh).collect();
    if used.len() != cr.meshes.len() {
        return Err(RewriteError::MeshesChanged);
    }
    let header = k2scene::write_header(
        &cr.meshes,
        &objects,
        &cams,
        &left,
        cr.data.len() as u64,
        limits,
    )?;
    Ok(Rewritten {
        header,
        objects: of_obj.into_iter().zip(objects).collect(),
        cameras: cam_of.into_iter().zip(cams).collect(),
    })
}
