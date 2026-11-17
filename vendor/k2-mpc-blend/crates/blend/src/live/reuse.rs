//! A still after a live edit without reading the file again ([`live_k2scene`]): the K2SCENE1
//! of a live state, written as two parts, a new header over the carrier's mesh data.
//!
//! **What a live edit can change.** The live store holds each object's name, transform,
//! visibility and collections, and each collection's own flag; no live change makes or changes
//! geometry: a mesh's vertices and triangles are in the carrier (the bytes the kit last read
//! from the file or wrote to it), never in the store. So between two carriers every mesh, and
//! its hash, is fixed; only the header's objects and cameras change.
//!
//! **What the carrier keeps.** The view decodes the carrier once
//! ([`super::place::CarrierView`]) and runs the whole mesh read on it once, for the meshes'
//! hashes; it keeps that read's data section (shared, never copied), each mesh's counts and
//! hash, and what the read takes of each object from the file (its mesh, modifiers, camera
//! data, how it is parented): [`crate::meshread::read::CarrierRead`]. The whole-file decode is
//! not kept.
//!
//! **The call.** [`live_k2scene`] writes the header for the live state through the mesh read's
//! own steps: each object's placement from its own fields and the carrier's facts
//! ([`super::place::placement`]) composed by the kit's `world_matrix`; `hidden` by the shared
//! hidden-by rule ([`crate::meshread::hidden`]) over the store's flags and holders and the
//! carrier's collection tree; ids and keys from the store; each object's mesh by its data in
//! the carrier's read; the same bounds, the same writer checks, the same header text
//! ([`crate::meshread::k2scene::write_header`]). No file is decoded (the carrier's one decode
//! aside, the first time the carrier is read at all). `header ++ data` is byte for byte the
//! mesh read ([`crate::meshread::mesh_read_paired`]) of a version that holds the live state
//! under its keys: the version a write-back of the state produces, for a state the write-back
//! writes.
//!
//! **The carrier id** ([`LiveK2Scene::carrier_id`]): SHA-256 of the data section, lowercase
//! hex ([`crate::meshread::read::data_id`]): equal across calls exactly when the data section
//! is byte-identical, so a host keeps what it built from the data (its buffers) across live
//! edits, write-backs that change no mesh, and anything else that leaves every mesh as it was;
//! a new carrier whose meshes differ gives a new id.
//!
//! **The three lists** ([`LiveK2Scene::changed`], `added`, `deleted`) compare the output's
//! objects and cameras with the carrier's own read's, by key (`@obj:N`, `@cam:N`, the
//! carrier's pairing naming the carrier's): hints for what to draw again; the header is what
//! counts.
//!
//! **Declined, typed** ([`ReuseDeclined`]), every one with the same fallback: the whole mesh
//! read of the version a write-back of the live state produces. The carrier's read refused, or
//! the carrier doesn't read live; no pairing, or one that doesn't fit; a drawn object whose
//! mesh the carrier's read doesn't hold; a drawn object the carrier holds no facts for (added
//! since the carrier, or its data not the carrier's object's: the view doesn't place it either);
//! the scene drawn's master collection deleted; the collections not the carrier's (one added,
//! deleted or moved, a kind or a parent changed); the meshes written not the carrier's (a mesh
//! of the carrier's read that no drawn object uses); a change the header can't take from the store (the objects'
//! order, an object's type, parent, rotation mode or quaternion, its name removed, a collection
//! holding it that isn't listed; a parent or the scene's camera deleted); a limit crossed or a
//! writer check failed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use k2_mpc_base::elem::{Key, KeyedRoot, KeyedState, Value};
use k2_mpc_base::placement::{Placement, Precision};
use k2_mpc_base::session::Pairing;
use k2_mpc_base::view_index::ViewCarrier;

use super::model::{
    f, floats_text, key_num, prop, CollKind, Parent, COLLS, COLL_PROPS, OBJS, OBJ_PROPS,
};
use super::place::{carrier_view, placement, CarrierView, ObjFields};
use super::view::address_of;
use crate::meshread::hidden::HiddenWalk;
use crate::meshread::k2scene::{SceneCamera, SceneObject};
use crate::meshread::read::{
    check_object_keys, decode, gather, rewrite, world_of, ObjIn, RewriteError,
};
use crate::meshread::{MeshReadError, MeshReadLimits};
use crate::mutant;

/// A live state's K2SCENE1, as two parts: `header ++ data` is the whole output.
#[derive(Clone, PartialEq, Eq)]
pub struct LiveK2Scene {
    /// The magic, the header's length, the header and its zero pad (0 to 3 bytes).
    pub header: Vec<u8>,
    /// The carrier's mesh data section, shared (the same allocation on every call over one
    /// carrier, never copied): each mesh's offset in the header's order is relative to it.
    pub data: Arc<[u8]>,
    /// SHA-256 of `data`, lowercase hex: the same exactly when `data` is byte-identical.
    pub carrier_id: String,
    /// The objects and cameras in both this output and the carrier's own read whose output
    /// differs from the carrier's (`@obj:N`, `@cam:N`): objects first, each in output order.
    pub changed: Vec<String>,
    /// The objects and cameras in this output and not in the carrier's read.
    pub added: Vec<String>,
    /// The objects and cameras in the carrier's read and not in this output, in its order.
    pub deleted: Vec<String>,
}

impl LiveK2Scene {
    /// The whole output in one buffer, `header ++ data` (a copy of the data).
    pub fn to_vec(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.header.len() + self.data.len());
        out.extend_from_slice(&self.header);
        out.extend_from_slice(&self.data);
        out
    }
}

impl std::fmt::Debug for LiveK2Scene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveK2Scene")
            .field("header_bytes", &self.header.len())
            .field("data_bytes", &self.data.len())
            .field("carrier_id", &self.carrier_id)
            .field("changed", &self.changed)
            .field("added", &self.added)
            .field("deleted", &self.deleted)
            .finish()
    }
}

/// Why [`live_k2scene`] doesn't write the live state's K2SCENE1. Every one has the same
/// fallback: the whole mesh read ([`crate::meshread::mesh_read_paired`]) of the version a
/// write-back of the live state produces. The text is fixed; keys are addresses.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReuseDeclined {
    /// The carrier's own mesh read refused, or the carrier doesn't read live: there is nothing
    /// to reuse.
    #[error("the carrier has no mesh read to reuse: read the written-back version whole")]
    NoCarrierRead,
    /// The carrier has no pairing, or one that doesn't fit its collections and objects.
    #[error("the carrier's pairing doesn't fit: {what}: read the written-back version")]
    Pairing {
        /// What doesn't fit (fixed text).
        what: &'static str,
    },
    /// A drawn object's mesh isn't one the carrier's read holds (an object added since the
    /// carrier using another mesh, or one moved into the scene drawn whose mesh the read didn't
    /// read).
    #[error("{key}: its mesh isn't the carrier read's: read the written-back version whole")]
    MeshNotHeld {
        /// The object's address.
        key: String,
    },
    /// A drawn object the carrier holds no facts for: added since the carrier, or its data
    /// isn't the carrier's object's (the view doesn't place it either).
    #[error("{key}: the carrier holds no facts for it: read the written-back version whole")]
    NoFacts {
        /// The object's address.
        key: String,
    },
    /// The scene drawn changed: its master collection was deleted.
    #[error("the scene drawn changed: {what}: read the written-back version whole")]
    DrawnScene {
        /// What changed (fixed text).
        what: &'static str,
    },
    /// The collections aren't the carrier's: one added, deleted or moved, its kind or its
    /// parent changed.
    #[error("the collections aren't the carrier's: {what}: read the written-back version")]
    Collections {
        /// What changed (fixed text).
        what: &'static str,
    },
    /// The meshes written wouldn't be the carrier's (a mesh of the carrier's read that no drawn object uses), so
    /// the data section would differ.
    #[error("the meshes written aren't the carrier's: read the written-back version whole")]
    MeshesChanged,
    /// A change the header can't take from the store: the objects' order, an object's type,
    /// parent, rotation mode or quaternion, its name removed, a collection holding it that isn't
    /// listed, a deleted parent or scene camera.
    #[error("{key}: {what}: read the written-back version whole")]
    Unsupported {
        /// The object's address.
        key: String,
        /// What changed (fixed text).
        what: &'static str,
    },
    /// A limit crossed, or a writer check failed: the mesh read's own refusal.
    #[error("{0}: read the written-back version whole")]
    Refused(MeshReadError),
}

impl ReuseDeclined {
    /// The decline's code: `reuse_no_carrier_read`, `reuse_pairing`, `reuse_mesh_not_held`,
    /// `reuse_no_facts`, `reuse_drawn_scene`, `reuse_collections`, `reuse_meshes_changed`,
    /// `reuse_unsupported`; for a refusal, the mesh read's code (`too_large`, `invalid`).
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoCarrierRead => "reuse_no_carrier_read",
            Self::Pairing { .. } => "reuse_pairing",
            Self::MeshNotHeld { .. } => "reuse_mesh_not_held",
            Self::NoFacts { .. } => "reuse_no_facts",
            Self::DrawnScene { .. } => "reuse_drawn_scene",
            Self::Collections { .. } => "reuse_collections",
            Self::MeshesChanged => "reuse_meshes_changed",
            Self::Unsupported { .. } => "reuse_unsupported",
            Self::Refused(e) => e.code(),
        }
    }
}

fn list<'a>(state: &'a KeyedState, name: &str) -> &'a [(Key, Value)] {
    state
        .roots
        .iter()
        .find_map(|(r, root)| match root {
            KeyedRoot::List { live, .. } if r == name => Some(live.as_slice()),
            _ => None,
        })
        .unwrap_or(&[])
}

fn map<'a>(state: &'a KeyedState, name: &str) -> Option<&'a BTreeMap<String, Value>> {
    state.roots.iter().find_map(|(r, root)| match root {
        KeyedRoot::Map(m) if r == name => Some(m),
        _ => None,
    })
}

fn field<'a>(m: Option<&'a BTreeMap<String, Value>>, k: &Key, fld: &str) -> Option<&'a str> {
    m?.get(&prop(k, fld)).and_then(Value::as_str)
}

/// The live state's K2SCENE1 over `carrier` (the kit's, with its pairing), under `limits`:
/// the module's rules. The carrier is decoded once per carrier, by the view's hook or by the
/// first call; no call decodes a file.
pub fn live_k2scene(
    state: &KeyedState,
    carrier: &ViewCarrier,
    limits: &MeshReadLimits,
) -> Result<LiveK2Scene, ReuseDeclined> {
    if mutant::active("ReuseRereads") {
        // planted: the carrier's file decoded again on every call
        let _ = decode(carrier.bytes(), limits);
    }
    let cv = carrier_view(carrier, &limits.live).ok_or(ReuseDeclined::NoCarrierRead)?;
    let pairing = carrier.pairing().ok_or(ReuseDeclined::Pairing {
        what: "the carrier has no pairing",
    })?;
    live_k2scene_of(state, cv, pairing, limits)
}

/// [`live_k2scene`] over a carrier's view `cv` ([`CarrierView::read`] of the carrier's bytes)
/// and its `pairing`, for a host that holds them itself.
pub fn live_k2scene_of(
    state: &KeyedState,
    cv: &CarrierView,
    pairing: &Pairing,
    limits: &MeshReadLimits,
) -> Result<LiveK2Scene, ReuseDeclined> {
    use ReuseDeclined as R;
    let cr = cv.mesh_read.as_ref().ok_or(R::NoCarrierRead)?;
    let ok = pairing.keys(OBJS).ok_or(R::Pairing {
        what: "no keys for the objects",
    })?;
    check_object_keys(ok, cr.objs.len()).map_err(|e| match e {
        MeshReadError::Pairing { what } => R::Pairing { what },
        other => R::Refused(other),
    })?;
    let ck = pairing.keys(COLLS).ok_or(R::Pairing {
        what: "no keys for the collections",
    })?;
    if ck.len() != cr.colls.len() {
        return Err(R::Pairing {
            what: "the collection keys don't fit the collections",
        });
    }
    let addr = |k: &Key| address_of(OBJS, k);

    // the collections: the carrier's, in its order and kinds, with their parents; the scene
    // drawn's master collection among them
    let colls = list(state, COLLS);
    let drawn = cv.scene.drawn.filter(|&m| m < ck.len());
    if let Some(m) = drawn {
        if !colls.iter().any(|(k, _)| *k == ck[m]) {
            return Err(R::DrawnScene {
                what: "its master collection was deleted",
            });
        }
    }
    let same_colls = colls.len() == ck.len()
        && colls
            .iter()
            .zip(ck)
            .zip(&cr.colls)
            .all(|(((k, v), pk), c)| k == pk && v.as_str() == Some(c.kind.as_str()));
    if !same_colls {
        return Err(R::Collections {
            what: "one was added, deleted or moved, or its kind changed",
        });
    }
    let cprops = map(state, COLL_PROPS);
    for (i, c) in cr.colls.iter().enumerate() {
        if c.kind == CollKind::None {
            continue;
        }
        if field(cprops, &ck[i], f::PARENT) != c.parent.map(|p| ck[p].as_str()) {
            return Err(R::Collections {
                what: "a collection's parent changed",
            });
        }
    }
    let walk = drawn.map(|m| {
        HiddenWalk::new(m, &cv.scene.children, &|c| {
            field(cprops, &ck[c], f::HIDDEN) == Some("1")
        })
    });

    // the objects: each carrier object still live, in the carrier's order
    let props = map(state, OBJ_PROPS);
    let empty = BTreeMap::new();
    let props_or = props.unwrap_or(&empty);
    let at: HashMap<&Key, usize> = ok.iter().enumerate().map(|(i, k)| (k, i)).collect();
    let cat: HashMap<&str, usize> = ck
        .iter()
        .enumerate()
        .map(|(i, k)| (k.as_str(), i))
        .collect();
    let holders_of = |k: &Key| -> Result<Vec<usize>, ReuseDeclined> {
        let named = field(props, k, f::COLL).into_iter().chain(
            field(props, k, f::ALSO)
                .into_iter()
                .flat_map(|a| a.split(',')),
        );
        named
            .map(|c| {
                cat.get(c).copied().ok_or_else(|| R::Unsupported {
                    key: addr(k),
                    what: "a collection holding it isn't listed",
                })
            })
            .collect()
    };
    let mut live = vec![false; ok.len()];
    let mut last: Option<usize> = None;
    let mut added: Vec<(&Key, &Value)> = Vec::new();
    for (k, v) in list(state, OBJS) {
        let Some(&i) = at.get(k) else {
            added.push((k, v));
            continue;
        };
        if last.is_some_and(|l| l > i) {
            return Err(R::Unsupported {
                key: addr(k),
                what: "the objects' order changed",
            });
        }
        last = Some(i);
        live[i] = true;
        let o = &cr.objs[i];
        if v.as_str() != Some(o.kind.as_str()) {
            return Err(R::Unsupported {
                key: addr(k),
                what: "its type changed",
            });
        }
        if field(props, k, f::DATA).unwrap_or("") != o.data {
            return Err(R::NoFacts { key: addr(k) });
        }
        let parent = match &o.parent {
            Parent::None => None,
            Parent::Obj(p) => Some(ok[*p].as_str()),
            Parent::Other(s) => Some(s.as_str()),
        };
        if field(props, k, f::PARENT) != parent {
            return Err(R::Unsupported {
                key: addr(k),
                what: "its parent changed",
            });
        }
        let rotmode = Some(o.rotmode.as_str()).filter(|m| !m.is_empty());
        if field(props, k, f::ROTMODE) != rotmode {
            return Err(R::Unsupported {
                key: addr(k),
                what: "its rotation mode changed",
            });
        }
        if field(props, k, f::QUAT).map(str::to_owned) != o.quat.map(|q| floats_text(&q)) {
            return Err(R::Unsupported {
                key: addr(k),
                what: "its quaternion changed",
            });
        }
        if field(props, k, f::NAME).is_none() {
            return Err(R::Unsupported {
                key: addr(k),
                what: "its name was removed",
            });
        }
    }
    for (i, o) in cr.objs.iter().enumerate() {
        if !live[i] {
            if cr.files[i].scene_camera {
                return Err(R::Unsupported {
                    key: addr(&ok[i]),
                    what: "the scene's camera was deleted",
                });
            }
            continue;
        }
        if let Parent::Obj(p) = o.parent {
            if !live[p] {
                return Err(R::Unsupported {
                    key: addr(&ok[i]),
                    what: "its parent was deleted",
                });
            }
        }
    }
    // an object added since the carrier draws only from what the carrier holds of it: nothing
    for &(k, v) in &added {
        let kind = v.as_str().unwrap_or("");
        if kind != "mesh" && kind != "camera" {
            continue;
        }
        let hs = holders_of(k)?;
        if !walk.as_ref().is_some_and(|w| w.holds(&hs)) {
            continue;
        }
        let data = field(props, k, f::DATA).unwrap_or("");
        let held = kind == "mesh"
            && cr.objs.iter().zip(&cr.files).any(|(o, file)| {
                o.data == data
                    && file
                        .mesh
                        .is_ok_and(|b| matches!(cr.blocks.get(&b), Some(Ok(_))))
            });
        return Err(if kind == "mesh" && !held {
            R::MeshNotHeld { key: addr(k) }
        } else {
            R::NoFacts { key: addr(k) }
        });
    }

    // every live object's placement, by address, as the live view places it
    let mut places: HashMap<String, Option<Placement>> = HashMap::new();
    let mut objs = Vec::new();
    for (i, k) in ok.iter().enumerate() {
        if !live[i] {
            continue;
        }
        let o = &cr.objs[i];
        let stale = if mutant::active("ReuseStaleTransform") {
            // planted: the carrier's transform kept, not the store's
            o.xform.map(|x| floats_text(&x))
        } else {
            None
        };
        let mut fields = ObjFields::of(props_or, k);
        if stale.is_some() {
            fields.xform = stale.as_deref();
        }
        let mut p = placement(&fields, &o.place);
        if let Some(p) = &mut p {
            p.precision = Precision::F32;
        }
        places.insert(addr(k), p);
        objs.push(ObjIn {
            oi: i,
            name: field(props, k, f::NAME).unwrap_or(""),
            num: key_num(k.as_str(), 'o').unwrap_or(0),
            holders: holders_of(k)?,
            hidden: field(props, k, f::HIDDEN) == Some("1"),
            live: o,
            file: &cr.files[i],
        });
    }
    let placed = |x: &ObjIn<'_>| world_of(x.live, &addr(&ok[x.oi]), &places, None);
    let g = gather(objs, walk.as_ref(), cr.render.as_ref(), &placed);
    let out = rewrite(g, cr, limits).map_err(|e| match e {
        RewriteError::MeshNotHeld(key) => R::MeshNotHeld { key },
        RewriteError::MeshesChanged => R::MeshesChanged,
        RewriteError::Read(e) => R::Refused(e),
    })?;

    // the lists, against the carrier's own read under the carrier's pairing
    let mut changed = Vec::new();
    let mut added = Vec::new();
    let mut deleted = Vec::new();
    let was_obj: HashMap<usize, &SceneObject> = cr.objects.iter().map(|(i, o)| (*i, o)).collect();
    let was_cam: HashMap<usize, &SceneCamera> = cr.cameras.iter().map(|(i, c)| (*i, c)).collect();
    let all = mutant::active("ReuseChangedAll");
    for (i, o) in &out.objects {
        match was_obj.get(i) {
            // planted: every key listed as changed
            Some(_) if all => changed.push(o.key.clone()),
            Some(w) if same_object(w, o) => {}
            Some(_) => changed.push(o.key.clone()),
            None => added.push(o.key.clone()),
        }
    }
    for (i, c) in &out.cameras {
        match was_cam.get(i) {
            Some(_) if all => changed.push(c.key.clone()),
            Some(w) if same_camera(w, c) => {}
            Some(_) => changed.push(c.key.clone()),
            None => added.push(c.key.clone()),
        }
    }
    let now_obj: HashSet<usize> = out.objects.iter().map(|(i, _)| *i).collect();
    let now_cam: HashSet<usize> = out.cameras.iter().map(|(i, _)| *i).collect();
    let num = |i: usize| key_num(ok[i].as_str(), 'o').unwrap_or(0);
    for (i, _) in &cr.objects {
        if !now_obj.contains(i) {
            deleted.push(format!("@obj:{}", num(*i)));
        }
    }
    for (i, _) in &cr.cameras {
        if !now_cam.contains(i) {
            deleted.push(format!("@cam:{}", num(*i)));
        }
    }
    let data = if mutant::active("ReuseCopiesData") {
        // planted: the data section copied on every call
        Arc::from(&cr.data[..])
    } else {
        Arc::clone(&cr.data)
    };
    Ok(LiveK2Scene {
        header: out.header,
        data,
        carrier_id: cr.id.clone(),
        changed,
        added,
        deleted,
    })
}

/// Whether two objects are written the same, their keys aside.
fn same_object(a: &SceneObject, b: &SceneObject) -> bool {
    let SceneObject {
        id,
        key: _,
        mesh,
        base_color,
        double_sided,
        transform,
        modifiers,
        modifiers_not_applied,
        transform_not_evaluated,
        hidden,
    } = a;
    *id == b.id
        && *mesh == b.mesh
        && base_color.map(f32::to_bits) == b.base_color.map(f32::to_bits)
        && *double_sided == b.double_sided
        && transform.map(f32::to_bits) == b.transform.map(f32::to_bits)
        && *modifiers == b.modifiers
        && *modifiers_not_applied == b.modifiers_not_applied
        && *transform_not_evaluated == b.transform_not_evaluated
        && *hidden == b.hidden
}

/// Whether two cameras are written the same, their keys aside.
fn same_camera(a: &SceneCamera, b: &SceneCamera) -> bool {
    let bits3 = |v: [f32; 3]| v.map(f32::to_bits);
    let opt3 = |v: Option<[f32; 3]>| v.map(bits3);
    let opt = |v: Option<f32>| v.map(f32::to_bits);
    let SceneCamera {
        id,
        key: _,
        position,
        target,
        forward,
        up,
        yfov,
        aspect,
        znear,
        active,
        lens_mm,
        sensor_mm,
        sensor_fit,
    } = a;
    *id == b.id
        && bits3(*position) == bits3(b.position)
        && opt3(*target) == opt3(b.target)
        && opt3(*forward) == opt3(b.forward)
        && bits3(*up) == bits3(b.up)
        && yfov.to_bits() == b.yfov.to_bits()
        && opt(*aspect) == opt(b.aspect)
        && opt(*znear) == opt(b.znear)
        && *active == b.active
        && opt(*lens_mm) == opt(b.lens_mm)
        && sensor_mm.map(|s| s.map(f32::to_bits)) == b.sensor_mm.map(|s| s.map(f32::to_bits))
        && *sensor_fit == b.sensor_fit
}
