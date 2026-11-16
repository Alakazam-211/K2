//! Each object's place in 3D, for the kit's view slot ([`SPACE`]): the numbers the file stores,
//! in the shape it stores them, never computed here.
//!
//! - **An object with no parent**: one `Trs` step, `relative_to` none (the doc's space): its
//!   location, its rotation and its scale from `xform`, the rotation as its mode says
//!   ([`placement`]): an Euler order word (`xyz` is the kit's `XYZ`), the quaternion in
//!   `quat`, or the axis and angle the file holds (read from the carrier: the store has no field
//!   for it).
//! - **An object parented to an object**: `relative_to` is the parent's address, and the steps
//!   are a `Matrix` holding the parent inverse matrix the file keeps for the object (read from
//!   the carrier), then the `Trs` step: the world matrix is the parent's world matrix, then the
//!   parent inverse, then the object's own.
//! - **No placement** (`None`, never a guess): a parent that isn't a listed object, or a
//!   parenting other than to the object itself (`partype` not 0: a bone, vertices), whose world
//!   matrix depends on what the slot doesn't hold; delta transforms that aren't identity; a
//!   rotation mode or a field the file doesn't give; no carrier.
//! - **Driven** (animation, drivers or a constraint: [`super::model::driven`]): the placement
//!   with `approx` `driven`; the stored numbers are the base the app recomputes from.
//!
//! **The own-fields rule.** A placement reads its object's own fields (`xform`, `rotmode`,
//! `quat`, `parent`: the slot's [`SpaceSlot::unit_fields`]) and the carrier. The carrier's
//! objects are in the decode's order, which the carrier's pairing keys (the kit's
//! `ViewCarrier::pairing`): an object's facts are the ones its key names there
//! ([`CarrierView`]), never found by its live position, so objects that come, go or move
//! between carriers keep theirs, and one added since the carrier has none (no placement until
//! the file holds it). Its data is checked too, so a pairing that doesn't fit gives no
//! placement rather than another object's.
//!
//! **The doc's space** ([`view_space`]): `scene-unit`, Z up, right-handed; metres per unit is
//! the unit scale every scene holds (the scene map's `unit_scale`) only when every scene's unit
//! system is metric and the scales agree; anything else (a unit system 0, systems that differ,
//! another system, scales that differ, a scene that holds none) gives none: unknown, never an
//! assumed 1.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use k2_mpc_base::elem::{Key, KeyedRoot, KeyedState, Value};
use k2_mpc_base::placement::{
    Axis, EulerOrder, Handed, Placement, Precision, Rotation, Space, SpaceSlot, SpaceSrc, Step,
};
use k2_mpc_base::session::Pairing;
use k2_mpc_base::view_index::{ViewCarrier, ViewSource};

use super::model::{self, f, parse_floats, prop, s, LiveLimits, PlaceFacts, OBJS, OBJ_PROPS};
use super::view::address_of;
use crate::meshread::read::{choose_scene, decode, read_inner, Decoded};
use crate::meshread::MeshReadLimits;
use crate::mutant;
use crate::scene::ROTATION_MODES;

/// The scene's 3D slot: objects are placed from these fields, collections aren't placed, the
/// numbers are the file's `f32`s, and the doc's space is the pack's, from the scene map.
pub const SPACE: SpaceSlot = SpaceSlot {
    unit_fields: &[f::XFORM, f::ROTMODE, f::QUAT, f::PARENT],
    container_fields: &[],
    space: SpaceSrc::Pack {
        roots: &[model::SCENE],
    },
    precision: Precision::F32,
};

/// The length unit's word.
pub const LENGTH_UNIT: &str = "scene-unit";

/// The reason word of a driven placement.
pub const DRIVEN: &str = "driven";

/// An object's own fields a placement reads, as the store holds them.
#[derive(Debug, Clone, Copy, Default)]
pub struct ObjFields<'a> {
    /// `xform`: `lx,ly,lz,rx,ry,rz,sx,sy,sz`.
    pub xform: Option<&'a str>,
    /// `rotmode`: its word, or its number when the file names none.
    pub rotmode: Option<&'a str>,
    /// `quat`: `w,x,y,z`.
    pub quat: Option<&'a str>,
    /// `parent`: a listed object's live key, or a file key.
    pub parent: Option<&'a str>,
}

impl<'a> ObjFields<'a> {
    /// Element `key`'s fields in the objects' field map `props`.
    pub fn of(props: &'a BTreeMap<String, Value>, key: &Key) -> ObjFields<'a> {
        ObjFields {
            xform: field_of(props, key, f::XFORM),
            rotmode: field_of(props, key, f::ROTMODE),
            quat: field_of(props, key, f::QUAT),
            parent: field_of(props, key, f::PARENT),
        }
    }
}

/// Element `key`'s field `fld` in its composite map `props`.
pub fn field_of<'a>(props: &'a BTreeMap<String, Value>, key: &Key, fld: &str) -> Option<&'a str> {
    props.get(&prop(key, fld)).and_then(Value::as_str)
}

fn f64s<const N: usize>(v: [f32; N]) -> [f64; N] {
    v.map(f64::from)
}

/// The rotation a mode word names: an Euler order (the word in capitals), a quaternion, or an
/// axis and angle. A number is read through the file's table ([`ROTATION_MODES`]); anything
/// else is no rotation the slot knows.
enum Mode {
    Euler(EulerOrder),
    Quat,
    AxisAngle,
}

fn mode_of(word: &str) -> Option<Mode> {
    let word = match word.parse::<i64>() {
        Ok(n) => ROTATION_MODES.iter().find(|(v, _)| *v == n)?.1,
        Err(_) => word,
    };
    match word {
        "quaternion" => Some(Mode::Quat),
        "axis_angle" => Some(Mode::AxisAngle),
        w => {
            let mut upper = w.to_ascii_uppercase();
            if mutant::active("EulerOrderReversed") {
                // planted: the order word read the other way round
                upper = upper.chars().rev().collect();
            }
            EulerOrder::parse(&upper).map(Mode::Euler)
        }
    }
}

/// The placement of an object with own fields `o` and file facts `facts` (the module's
/// rules), or `None`.
pub fn placement(o: &ObjFields<'_>, facts: &PlaceFacts) -> Option<Placement> {
    let x = parse_floats::<9>(o.xform?)?;
    if !facts.deltas_identity {
        return None;
    }
    let rot = match mode_of(o.rotmode?)? {
        Mode::Euler(order) => Rotation::Euler {
            order,
            angles: f64s([x[3], x[4], x[5]]),
        },
        Mode::Quat => Rotation::Quat(f64s(parse_floats::<4>(o.quat?)?)),
        Mode::AxisAngle => {
            let (axis, angle) = facts.axis_angle?;
            if !axis.iter().all(|v| v.is_finite()) || !angle.is_finite() {
                return None;
            }
            Rotation::AxisAngle {
                axis: f64s(axis),
                angle: f64::from(angle),
            }
        }
    };
    let trs = Step::Trs {
        loc: f64s([x[0], x[1], x[2]]),
        rot,
        scale: f64s([x[6], x[7], x[8]]),
    };
    let (relative_to, steps) = match o.parent.filter(|p| !p.is_empty()) {
        None => (None, vec![trs]),
        Some(p) => {
            // a listed object's live key; a file key names a parent that isn't listed
            model::key_num(p, 'o')?;
            let parent = Key::new(p).ok()?;
            if facts.partype != Some(0) {
                return None;
            }
            let inverse = facts.parentinv?;
            if !inverse.iter().all(|v| v.is_finite()) {
                return None;
            }
            let steps = if mutant::active("ParentInverseIgnored") {
                // planted: the parent inverse left out
                vec![trs]
            } else {
                vec![Step::Matrix(f64s(inverse)), trs]
            };
            (Some(address_of(OBJS, &parent)), steps)
        }
    };
    Some(Placement {
        relative_to,
        steps,
        approx: facts.driven.then(|| DRIVEN.to_owned()),
        ..Placement::default()
    })
}

/// One object of the carrier, as the view reads it: its data (to check a pairing fits) and its
/// facts.
#[derive(Debug, Clone, PartialEq)]
struct CarrierObj {
    data: String,
    facts: PlaceFacts,
}

/// What the view reads of the carrier's scene beside the objects' facts: the collections'
/// tree as the file holds it, the scene the mesh read draws, and each mesh's content hash.
#[derive(Debug, Clone, Default)]
pub struct CarrierScene {
    /// Each collection's child collections, by their place in the decode's collections (the
    /// order the carrier's pairing keys), in the order the file lists them.
    pub children: Vec<Vec<usize>>,
    /// Each collection's first holder, the same way.
    pub parents: Vec<Option<usize>>,
    /// How many collections the decode lists (`c0` included).
    pub colls: usize,
    /// The place of the drawn scene's master collection (the mesh read's choice).
    pub drawn: Option<usize>,
    /// Each mesh's content hash as the carrier's mesh read writes it, by the data text of the
    /// objects using it (`mesh @me:Cube`): only meshes the read writes.
    pub mesh_hashes: HashMap<String, String>,
}

impl CarrierScene {
    /// The scene of a decoded carrier `d`; mesh hashes from its mesh read under `limits`
    /// (none when the read refuses).
    fn of(d: &Decoded, limits: &MeshReadLimits) -> CarrierScene {
        let blocks = model::coll_blocks(&d.model);
        let (s, _) = choose_scene(&d.file, &d.raw, &d.model);
        let drawn = if mutant::active("CurrentSceneFirst") {
            // planted: the first scene marked, not the one the mesh read draws
            d.scene
                .colls
                .iter()
                .position(|c| c.kind == model::CollKind::Scene)
        } else {
            s.and_then(|s| blocks.iter().position(|&b| b == s))
        };
        let mut mesh_hashes = HashMap::new();
        if let Ok((_, by_obj)) = read_inner(d, None, limits) {
            for (oi, hash) in by_obj {
                if let Some(o) = d.scene.objs.get(oi) {
                    mesh_hashes.insert(o.data.clone(), hash);
                }
            }
        }
        CarrierScene {
            children: model::coll_children(&d.model),
            parents: d.scene.colls.iter().map(|c| c.parent).collect(),
            colls: d.scene.colls.len(),
            drawn,
            mesh_hashes,
        }
    }
}

/// The carrier as the view reads it, decoded once per carrier
/// ([`ViewCarrier::decoded`]): each object's data and [`PlaceFacts`], in the decode's
/// order (the order the carrier's pairing keys), and each paired key's place in it, worked out
/// once and checked on every use; and its scene ([`CarrierScene`]).
#[derive(Debug)]
pub struct CarrierView {
    objs: Vec<CarrierObj>,
    at: Mutex<HashMap<Key, usize>>,
    /// The carrier's scene.
    pub scene: CarrierScene,
}

impl CarrierView {
    /// The carrier `bytes` read (the mesh read's decode, which is the live tier's, and its read
    /// for the meshes' hashes); `None` when they don't read live.
    pub fn read(bytes: &[u8], limits: &LiveLimits) -> Option<CarrierView> {
        let mesh_limits = MeshReadLimits::with_live(*limits);
        let d = decode(bytes, &mesh_limits).ok()?;
        let scene = CarrierScene::of(&d, &mesh_limits);
        Some(CarrierView {
            objs: d
                .scene
                .objs
                .into_iter()
                .map(|o| CarrierObj {
                    data: o.data,
                    facts: o.place,
                })
                .collect(),
            at: Mutex::new(HashMap::new()),
            scene,
        })
    }

    /// The place of `key` in `keys` (the pairing's, or a list's): the one kept, when `keys`
    /// still holds `key` there, else from `keys` again.
    fn place_of(&self, keys: &[Key], key: &Key) -> Option<usize> {
        let mut at = self.at.lock().ok()?;
        if let Some(&i) = at.get(key) {
            if keys.get(i) == Some(key) {
                return Some(i);
            }
        }
        at.clear();
        at.extend(keys.iter().enumerate().map(|(i, k)| (k.clone(), i)));
        at.get(key).copied()
    }

    /// The facts of the object `key` of `state`: the carrier's object its key names in the
    /// carrier's `pairing`, when the pairing keys as many objects as the carrier holds and that
    /// object has the live object's data. `None` for a key the pairing doesn't name (an object
    /// added since the carrier) and without a pairing.
    pub fn facts(
        &self,
        state: &KeyedState,
        pairing: Option<&Pairing>,
        key: &Key,
    ) -> Option<&PlaceFacts> {
        let i = if mutant::active("PlacementByPosition") {
            // planted: the facts found by the object's live position
            let live: Vec<Key> = live_objs(state).iter().map(|(k, _)| k.clone()).collect();
            if live.len() != self.objs.len() {
                return None;
            }
            self.place_of(&live, key)?
        } else {
            let keys = pairing?.keys(OBJS)?;
            if keys.len() != self.objs.len() {
                return None;
            }
            self.place_of(keys, key)?
        };
        let o = &self.objs[i];
        let data = obj_props(state)
            .and_then(|m| field_of(m, key, f::DATA))
            .unwrap_or("");
        (o.data == data).then_some(&o.facts)
    }
}

fn live_objs(state: &KeyedState) -> &[(Key, Value)] {
    state
        .roots
        .iter()
        .find_map(|(r, root)| match root {
            KeyedRoot::List { live, .. } if r == OBJS => Some(live.as_slice()),
            _ => None,
        })
        .unwrap_or(&[])
}

/// The objects' field map of `state`.
pub fn obj_props(state: &KeyedState) -> Option<&BTreeMap<String, Value>> {
    map_root(state, OBJ_PROPS)
}

fn map_root<'a>(state: &'a KeyedState, name: &str) -> Option<&'a BTreeMap<String, Value>> {
    state.roots.iter().find_map(|(r, root)| match root {
        KeyedRoot::Map(m) if r == name => Some(m),
        _ => None,
    })
}

#[cfg(feature = "mutants")]
thread_local! {
    /// The plant `PlacementDroppedOnAdd`: the first carrier's view, kept for every later one.
    static FIRST: std::cell::RefCell<Option<std::sync::Arc<CarrierView>>> =
        const { std::cell::RefCell::new(None) };
}

/// Object `key`'s placement on the live tier ([`crate::live::BlendLive`]'s
/// `view_placement`): its own fields in `source`'s state and its facts in the carrier.
pub fn view_placement(source: ViewSource<'_>, key: &Key, limits: &LiveLimits) -> Option<Placement> {
    let ViewSource::Live(state, Some(carrier)) = source else {
        return None;
    };
    let cv = carrier_view(carrier, limits)?;
    #[cfg(feature = "mutants")]
    if !mutant::active("PlacementDroppedOnAdd") {
        FIRST.with(|f| f.borrow_mut().take());
    } else {
        // planted: the carrier's view decoded once per pack, not once per carrier
        let first = FIRST.with(|f| {
            f.borrow_mut()
                .get_or_insert_with(|| {
                    std::sync::Arc::new(CarrierView {
                        objs: cv.objs.clone(),
                        at: Mutex::new(HashMap::new()),
                        scene: cv.scene.clone(),
                    })
                })
                .clone()
        });
        let facts = first.facts(state, carrier.pairing(), key)?;
        return placement(&ObjFields::of(obj_props(state)?, key), facts);
    }
    let facts = cv.facts(state, carrier.pairing(), key)?;
    placement(&ObjFields::of(obj_props(state)?, key), facts)
}

/// The carrier's view, decoded once per carrier.
pub fn carrier_view<'c>(carrier: &'c ViewCarrier, limits: &LiveLimits) -> Option<&'c CarrierView> {
    carrier.decoded(|b| CarrierView::read(b, limits))
}

/// The doc's space: `scene-unit`, Z up, right-handed, with the scenes' unit scale when the
/// scene map holds one.
pub fn view_space(source: ViewSource<'_>) -> Option<Space> {
    let ViewSource::Live(state, _) = source else {
        return None;
    };
    let metres_per_unit = map_root(state, model::SCENE)
        .and_then(|m| m.get(s::UNIT_SCALE))
        .and_then(Value::as_str)
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .map(f64::from);
    Some(Space {
        length_unit: LENGTH_UNIT.to_owned(),
        metres_per_unit,
        up: Axis::Z,
        handed: Handed::Right,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> PlaceFacts {
        PlaceFacts {
            partype: Some(0),
            parentinv: Some([
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 1.0,
            ]),
            axis_angle: Some(([0.0, 0.0, 1.0], 0.5)),
            deltas_identity: true,
            driven: false,
        }
    }

    fn fields<'a>(rotmode: &'a str, parent: Option<&'a str>) -> ObjFields<'a> {
        ObjFields {
            xform: Some("1,2,3,0.5,0,0,1,1,2"),
            rotmode: Some(rotmode),
            quat: Some("1,0,0,0"),
            parent,
        }
    }

    #[test]
    fn each_mode_and_parent() {
        let p = placement(&fields("zxy", None), &facts()).expect("placed");
        assert_eq!(p.relative_to, None);
        match &p.steps[..] {
            [Step::Trs {
                rot: Rotation::Euler { order, angles },
                ..
            }] => {
                assert_eq!(*order, EulerOrder::Zxy);
                assert_eq!(*angles, [0.5, 0.0, 0.0]);
            }
            other => panic!("{other:?}"),
        }
        let q = placement(&fields("quaternion", None), &facts()).expect("placed");
        assert!(matches!(
            &q.steps[..],
            [Step::Trs {
                rot: Rotation::Quat(_),
                ..
            }]
        ));
        let a = placement(&fields("axis_angle", None), &facts()).expect("placed");
        assert!(matches!(
            &a.steps[..],
            [Step::Trs {
                rot: Rotation::AxisAngle { .. },
                ..
            }]
        ));
        let c = placement(&fields("1", Some("o7")), &facts()).expect("placed");
        assert_eq!(c.relative_to.as_deref(), Some("@obj:7"));
        assert!(matches!(&c.steps[..], [Step::Matrix(_), Step::Trs { .. }]));
    }

    #[test]
    fn what_gives_none() {
        let f = facts();
        assert!(placement(&fields("xyz", Some("@ob:Rig")), &f).is_none());
        assert!(placement(&fields("7", None), &f).is_none());
        assert!(placement(&fields("sideways", None), &f).is_none());
        let bone = PlaceFacts {
            partype: Some(4),
            ..f
        };
        assert!(placement(&fields("xyz", Some("o2")), &bone).is_none());
        assert!(placement(&fields("xyz", None), &bone).is_some());
        let no_inverse = PlaceFacts {
            parentinv: None,
            ..f
        };
        assert!(placement(&fields("xyz", Some("o2")), &no_inverse).is_none());
        let delta = PlaceFacts {
            deltas_identity: false,
            ..f
        };
        assert!(placement(&fields("xyz", None), &delta).is_none());
        let driven = PlaceFacts { driven: true, ..f };
        let d = placement(&fields("xyz", None), &driven).expect("placed");
        assert_eq!(d.approx.as_deref(), Some(DRIVEN));
    }
}
