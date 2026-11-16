//! The scene's view schema for the kit's view index ([`VIEW`]), its addresses
//! ([`address_of`], [`key_of`]) and what the pack derives on the kit's index ([`derive`](fn@derive)).
//!
//! **An outliner placed in 3D.** Collections (and each scene's master collection, and `c0` for
//! objects in none) are the containers, in file order; objects the units, in their first
//! collection, with their type (the kit's `kind`, the list value), name and visibility.
//!
//! - **The collection tree** ([`k2_mpc_base::view_index::Container::parent`]): a collection's
//!   container names the container of the collection or scene holding it first, read from its
//!   `parent` field (a live key; the kit maps it to `@coll:N` and checks the tree). A scene's
//!   master collection, and `c0`, are at the top level. An outliner draws the file's collection
//!   tree from the index alone.
//! - **Placements** ride the kit's 3D slot ([`super::place::SPACE`]): each object's location,
//!   rotation and scale in its rotation mode, with its parent and the parent inverse
//!   ([`super::place`]). Collections have none.
//! - **Derived values per unit** ([`DERIVED`]): `transform`, the numbers as people read them
//!   (`loc x,y,z rot x,y,z scale x,y,z`); `data`, the object's data (`mesh @me:Cube`; none for
//!   an empty); `mesh_hash`, the content hash its mesh has in the carrier's mesh read (none for
//!   an object without a mesh, or whose mesh that read leaves out), found by the object's data,
//!   so objects sharing a mesh, and one added live using a mesh the carrier holds, carry the
//!   same; `hidden_by`, `self` or `collection:@coll:N` by the hidden-by rule
//!   ([`crate::meshread::hidden`]) in the scene the mesh read draws (none when shown, or not in
//!   it). A unit has no text: an object holds none.
//! - **Derived values per container** ([`CONTAINER_DERIVED`]): `current_scene`, `true` on the
//!   master collection of the scene the mesh read draws (the file's current scene, else the
//!   first), on no other.
//!
//! **What the hidden-by rule reads:** the live store's flags (each object's and collection's
//! `hidden`) and holders (each object's `coll` and `also`); the collections' tree as the
//! carrier holds it, by the carrier's pairing (each collection's children in the file's order,
//! a collection under two collections under both), with a collection the live store moves
//! (its `parent` another than the carrier's) under its new parent alone, last; and the scene the
//! carrier's mesh read draws. Without a carrier or a pairing that fits it, only `self` is
//! given. The carrier is decoded once per carrier ([`super::place::carrier_view`]), the mesh
//! read for the hashes with it, never per derive.
//!
//! **Addresses** are key-derived: `@coll:N` for `cN`, `@obj:N` for `oN`. The blob tier names
//! objects by file key (`@ob:Cube`): the two tiers differ for v1.

use std::collections::{BTreeMap, HashMap, HashSet};

use k2_mpc_base::elem::{Key, KeyedRoot, KeyedState, Value};
use k2_mpc_base::view_index::{
    Container, ContainerFields, FieldClass, Src, Unit, UnitFields, ViewIndex, ViewSchema,
    ViewSource, ViewTouched,
};

use super::model::{
    coll_key, f, key_num, obj_key, parse_floats, LiveLimits, COLLS, COLL_PROPS, OBJS,
};
use super::place::{carrier_view, field_of, obj_props, CarrierView, SPACE};
use crate::meshread::hidden::{HiddenBy, HiddenWalk};
use crate::mutant;

/// The derived value `transform`: an object's numbers as people read them.
pub const TRANSFORM: &str = "transform";

/// The derived value `data`: the object's data, its kind and key (`mesh @me:Cube`).
pub const DATA: &str = "data";

/// The derived value `mesh_hash`: the content hash of the object's mesh in the mesh read.
pub const MESH_HASH: &str = "mesh_hash";

/// The derived value `hidden_by`: `self` or `collection:@coll:N`.
pub const HIDDEN_BY: &str = "hidden_by";

/// The derived value per container `current_scene`: `true` on the drawn scene's master
/// collection.
pub const CURRENT_SCENE: &str = "current_scene";

/// The derived values per unit, in their order.
pub const DERIVED: &[&str] = &[TRANSFORM, DATA, MESH_HASH, HIDDEN_BY];

/// The derived values per container, in their order.
pub const CONTAINER_DERIVED: &[&str] = &[CURRENT_SCENE];

/// The scene's view schema (live tier).
pub const VIEW: ViewSchema = ViewSchema {
    containers: COLLS,
    units: OBJS,
    unit_container_field: f::COLL,
    container_id_field: None,
    native_unit: "scene-unit",
    container_fields: ContainerFields {
        title: Src::Field(f::NAME),
        hidden: Src::Field(f::HIDDEN),
        // the collection or scene holding it first, by live key (the kit maps it to an address)
        parent: Src::Field(f::PARENT),
        ..ContainerFields::NONE
    },
    unit_fields: UnitFields {
        kind: Src::ListValue,
        name: Src::Field(f::NAME),
        hidden: Src::Field(f::HIDDEN),
        ..UnitFields::NONE
    },
    field_class: &[
        (f::XFORM, FieldClass::Geometry),
        (f::QUAT, FieldClass::Geometry),
        (f::HIDDEN, FieldClass::Visibility),
        (f::NOSELECT, FieldClass::Visibility),
        (f::NORENDER, FieldClass::Visibility),
        (f::NAME, FieldClass::Text),
        (f::NID, FieldClass::Meta),
        (f::COLL, FieldClass::Meta),
        (f::ALSO, FieldClass::Meta),
        (f::DATA, FieldClass::Meta),
        (f::PARENT, FieldClass::Meta),
        (f::ROTMODE, FieldClass::Meta),
        (f::EDIT, FieldClass::Meta),
    ],
    unit_derived: DERIVED,
    container_derived: CONTAINER_DERIVED,
    space: Some(SPACE),
    ..ViewSchema::NONE
};

/// The address of element `key` of `root`: `@coll:N`, `@obj:N`; anything else as it is.
pub fn address_of(root: &str, key: &Key) -> String {
    match root {
        COLLS => key_num(key.as_str(), 'c').map(|n| format!("@coll:{n}")),
        OBJS => key_num(key.as_str(), 'o').map(|n| format!("@obj:{n}")),
        _ => None,
    }
    .unwrap_or_else(|| key.as_str().to_owned())
}

/// [`address_of`]'s inverse.
pub fn key_of(root: &str, address: &str) -> Option<Key> {
    let num = |s: &str| key_num(&format!("n{s}"), 'n');
    match root {
        COLLS => Some(coll_key(num(address.strip_prefix("@coll:")?)?)),
        OBJS => Some(obj_key(num(address.strip_prefix("@obj:")?)?)),
        _ => None,
    }
}

/// An `xform` as people read it: `loc x,y,z rot x,y,z scale x,y,z`.
pub fn xform_text(x: &str) -> Option<String> {
    let v = parse_floats::<9>(x)?;
    let t = |i: usize| super::model::floats_text(&v[i..i + 3]);
    Some(format!("loc {} rot {} scale {}", t(0), t(3), t(6)))
}

/// Unit `u`'s derived values from its own fields in `props`: `transform` and `data`.
fn derive_unit(props: Option<&BTreeMap<String, Value>>, u: &mut Unit) {
    let (Some(k), Some(props)) = (key_of(OBJS, &u.key), props) else {
        return;
    };
    if let Some(t) = field_of(props, &k, f::XFORM).and_then(xform_text) {
        u.set_derived(TRANSFORM, t);
    }
    if let Some(d) = field_of(props, &k, f::DATA).filter(|d| !d.is_empty()) {
        u.set_derived(DATA, d);
    }
}

/// Sets derived value `name` to `text`, or takes it away.
fn put(derived: &mut Vec<(String, String)>, name: &str, text: Option<String>) {
    match text {
        Some(t) => match derived.iter_mut().find(|(n, _)| n == name) {
            Some((_, v)) => *v = t,
            None => derived.push((name.to_owned(), t)),
        },
        None => derived.retain(|(n, _)| n != name),
    }
}

/// What the derived values read beside each unit's own fields: the objects' and collections'
/// fields, and the carrier's scene with the collections' walk (the module's rules).
struct Scene<'a> {
    props: Option<&'a BTreeMap<String, Value>>,
    cv: Option<&'a CarrierView>,
    /// The walk, and each collection key's number in it.
    walk: Option<(HiddenWalk, HashMap<&'a str, usize>, &'a [Key])>,
    /// The drawn scene's master collection, by address.
    drawn: Option<String>,
}

fn map_root<'a>(state: &'a KeyedState, name: &str) -> Option<&'a BTreeMap<String, Value>> {
    state.roots.iter().find_map(|(r, root)| match root {
        KeyedRoot::Map(m) if r == name => Some(m),
        _ => None,
    })
}

impl<'a> Scene<'a> {
    fn of(source: ViewSource<'a>, limits: &LiveLimits) -> Option<Scene<'a>> {
        let ViewSource::Live(state, carrier) = source else {
            return None;
        };
        let props = obj_props(state);
        let cv = carrier.and_then(|c| carrier_view(c, limits));
        let keys = carrier
            .and_then(|c| c.pairing())
            .and_then(|p| p.keys(COLLS))
            .filter(|k| cv.is_some_and(|cv| cv.scene.colls == k.len()));
        let (Some(cv), Some(keys)) = (cv, keys) else {
            return Some(Scene {
                props,
                cv,
                walk: None,
                drawn: None,
            });
        };
        let sc = &cv.scene;
        let at: HashMap<&str, usize> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| (k.as_str(), i))
            .collect();
        let cprops = map_root(state, COLL_PROPS);
        let field = |i: usize, fld: &str| cprops.and_then(|m| field_of(m, &keys[i], fld));
        // the carrier's tree, with each collection the live store moves under its new parent
        let mut children = sc.children.clone();
        children.resize(keys.len(), Vec::new());
        for (i, &was) in sc.parents.iter().enumerate().take(keys.len()) {
            let live = field(i, f::PARENT).and_then(|p| at.get(p).copied());
            if let Some(p) = live.filter(|&p| Some(p) != was) {
                for c in &mut children {
                    c.retain(|&k| k != i);
                }
                children[p].push(i);
            }
        }
        let drawn = sc.drawn.filter(|&m| m < keys.len());
        let walk = drawn.map(|m| {
            let w = HiddenWalk::new(m, &children, &|c| field(c, f::HIDDEN) == Some("1"));
            (w, at, keys)
        });
        Some(Scene {
            props,
            cv: Some(cv),
            walk,
            drawn: drawn.map(|m| address_of(COLLS, &keys[m])),
        })
    }

    /// Unit `u`'s `mesh_hash` and `hidden_by`.
    fn unit(&self, u: &mut Unit) {
        let (Some(k), Some(props)) = (key_of(OBJS, &u.key), self.props) else {
            return;
        };
        let hash = self.cv.and_then(|cv| {
            let data = field_of(props, &k, f::DATA).filter(|d| !d.is_empty())?;
            let h = cv.scene.mesh_hashes.get(data)?;
            Some(if mutant::active("MeshHashPerObject") {
                // planted: each object its own hash
                format!("{h}{}", u.key)
            } else {
                h.clone()
            })
        });
        put(&mut u.derived, MESH_HASH, hash);
        self.hidden_by(props, &k, u);
    }

    /// Unit `u` (object `k`)'s `hidden_by`.
    fn hidden_by(&self, props: &BTreeMap<String, Value>, k: &Key, u: &mut Unit) {
        let own = field_of(props, k, f::HIDDEN) == Some("1");
        let by = match &self.walk {
            None => own.then_some(HiddenBy::Itself),
            Some((w, at, _)) => {
                let holders: Vec<usize> = field_of(props, k, f::COLL)
                    .into_iter()
                    .chain(
                        field_of(props, k, f::ALSO)
                            .into_iter()
                            .flat_map(|a| a.split(',')),
                    )
                    .filter_map(|c| at.get(c).copied())
                    .collect();
                w.of(own, &holders)
            }
        };
        let text = match by {
            None => None,
            Some(HiddenBy::Itself) => Some("self".to_owned()),
            Some(HiddenBy::Collection(c)) => self
                .walk
                .as_ref()
                .map(|(_, _, keys)| format!("collection:{}", address_of(COLLS, &keys[c]))),
        };
        put(&mut u.derived, HIDDEN_BY, text);
    }

    /// Container `c`'s `current_scene`.
    fn container(&self, c: &mut Container) {
        let on = self.drawn.as_deref() == Some(c.key.as_str());
        put(&mut c.derived, CURRENT_SCENE, on.then(|| "true".to_owned()));
    }
}

/// What the pack derives on the kit's index: each object's derived values ([`DERIVED`]), from
/// the live store and the carrier's scene, and each container's ([`CONTAINER_DERIVED`]).
pub fn derive(source: ViewSource<'_>, index: &mut ViewIndex, limits: &LiveLimits) {
    let Some(sc) = Scene::of(source, limits) else {
        return;
    };
    for u in &mut index.units {
        derive_unit(sc.props, u);
        sc.unit(u);
    }
    for c in &mut index.containers {
        sc.container(c);
    }
}

/// [`derive`](fn@derive) for the units a change touched: each reads its own fields, and a placement
/// finds its file facts by its key in the carrier's pairing ([`super::place`]), so objects that
/// come, go or move change no other object's. A collection built again (its flag, its parent)
/// or containers that came, went or moved give every unit its `hidden_by` again; every
/// container's `current_scene` is set again.
pub fn derive_change(
    source: ViewSource<'_>,
    index: &mut ViewIndex,
    touched: &ViewTouched,
    limits: &LiveLimits,
) -> bool {
    let Some(sc) = Scene::of(source, limits) else {
        return false;
    };
    let again: HashSet<&str> = touched.units.iter().map(String::as_str).collect();
    let tree = (!touched.containers.is_empty() || touched.order)
        && !mutant::active("HiddenByStaleOnCollection");
    for u in &mut index.units {
        if again.contains(u.key.as_str()) {
            derive_unit(sc.props, u);
            sc.unit(u);
        } else if tree {
            if let (Some(k), Some(props)) = (key_of(OBJS, &u.key), sc.props) {
                sc.hidden_by(props, &k, u);
            }
        }
    }
    for c in &mut index.containers {
        sc.container(c);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_lead_back() {
        for (root, key, addr) in [
            (COLLS, coll_key(0), "@coll:0"),
            (COLLS, coll_key(12), "@coll:12"),
            (OBJS, obj_key(7), "@obj:7"),
        ] {
            assert_eq!(address_of(root, &key), addr);
            assert_eq!(key_of(root, addr), Some(key));
        }
        for bad in ["@obj:07", "@obj:", "@obj:-1", "obj:1", "@coll:1"] {
            assert_eq!(key_of(OBJS, bad), None, "{bad}");
        }
        assert_eq!(key_of(COLLS, "@obj:1"), None);
    }

    #[test]
    fn the_schema_fits() {
        let schema = super::super::SCHEMA;
        assert_eq!(VIEW.check(&schema, super::super::COMPOSITE), Ok(()));
    }
}
