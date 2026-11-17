//! The live view's derived values from the carrier's scene: each unit's `mesh_hash` and
//! `hidden_by`, each container's `current_scene`. Each row returns `Err("<row>: <step>: ...")`,
//! a `control` step first that its plant leaves passing.

use std::sync::Arc;

use k2_mpc_base::elem::{MapOp, Value};
use k2_mpc_base::session::{ConnId, EditOptions, Session};
use k2_mpc_base::view_index::{Unit, ViewIndex};
use k2_mpc_blend::live::COLL_PROPS;
use k2_mpc_blend::meshread::{mesh_read, MeshReadLimits};
use k2_mpc_blend::testgen::{MeshLayouts, MeshSpec, ObjectSpec, SceneBuilder};

use super::{fx, run_based, stdout, Fx};

fn step(ok: bool, row: &str, name: &str, why: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {name}: {}", why()))
    }
}

fn open(tag: &str, bytes: &[u8]) -> (Fx, Arc<Session>) {
    let f = fx(tag, |_| {});
    f.write(bytes);
    let s = f.open();
    (f, s)
}

fn view(row: &str, s: &Session, c: ConnId) -> Result<ViewIndex, String> {
    s.view_index(c)
        .map_err(|e| format!("{row}: setup: {e}"))?
        .ok_or_else(|| format!("{row}: setup: no view index"))
}

fn named<'a>(ix: &'a ViewIndex, name: &str, row: &str) -> Result<&'a Unit, String> {
    ix.units
        .iter()
        .find(|u| u.name == name)
        .ok_or_else(|| format!("{row}: setup: no unit {name}"))
}

fn container(ix: &ViewIndex, title: &str, row: &str) -> Result<String, String> {
    ix.containers
        .iter()
        .find(|c| c.title.as_deref() == Some(title))
        .map(|c| c.key.clone())
        .ok_or_else(|| format!("{row}: setup: no container {title}"))
}

/// A mesh object of mesh `data`, rotating by XYZ Euler angles.
fn mesh_obj(data: &str) -> ObjectSpec {
    ObjectSpec {
        data: Some(format!("ME:{data}")),
        ty: 1,
        rotmode: 1,
        ..ObjectSpec::default()
    }
}

/// The unit cube's eight corners and six quads.
fn cube() -> MeshSpec {
    let positions = vec![
        [-1.0, -1.0, -1.0],
        [1.0, -1.0, -1.0],
        [1.0, 1.0, -1.0],
        [-1.0, 1.0, -1.0],
        [-1.0, -1.0, 1.0],
        [1.0, -1.0, 1.0],
        [1.0, 1.0, 1.0],
        [-1.0, 1.0, 1.0],
    ];
    let faces = [
        [0, 3, 2, 1],
        [4, 5, 6, 7],
        [0, 1, 5, 4],
        [3, 7, 6, 2],
        [0, 4, 7, 3],
        [1, 2, 6, 5],
    ]
    .iter()
    .map(|f| f.to_vec())
    .collect();
    MeshSpec {
        positions,
        faces,
        ..MeshSpec::default()
    }
}

/// `A` and `B` share the cube, `T` has a triangle of its own, `E` is an empty, `Bad` uses a
/// mesh with a corner past its vertices.
fn hashed_scene() -> Vec<u8> {
    let mut b = SceneBuilder::meshes(403, MeshLayouts::LAYERS);
    b.file_global(Some("S"));
    b.scene("S", &["A", "B", "T", "E", "Bad"], &[], None);
    b.object("A", &mesh_obj("Cube"));
    b.object("B", &mesh_obj("Cube"));
    b.object("T", &mesh_obj("Tri"));
    b.object(
        "E",
        &ObjectSpec {
            rotmode: 1,
            ..ObjectSpec::default()
        },
    );
    b.object("Bad", &mesh_obj("Broken"));
    b.mesh_with("Cube", &cube(), MeshLayouts::LAYERS);
    let mut tri = cube();
    tri.faces.truncate(1);
    b.mesh_with("Tri", &tri, MeshLayouts::LAYERS);
    let mut broken = cube();
    broken.faces[0][0] = 99;
    b.mesh_with("Broken", &broken, MeshLayouts::LAYERS);
    b.bytes()
}

/// Each object's `mesh_hash` is its mesh's hash in the mesh read of the same bytes: shared by
/// the objects sharing a mesh, none for an empty or a mesh the read leaves out; an agent's move
/// leaves it as it was.
pub fn row_view_mesh_hash() -> Result<(), String> {
    const ROW: &str = "row_view_mesh_hash";
    let bytes = hashed_scene();
    let read = mesh_read(&bytes, &MeshReadLimits::default())
        .map_err(|e| format!("{ROW}: setup: the mesh read: {e}"))?;
    let hash_of = |name: &str| {
        read.report
            .drawn
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.mesh_hash.clone())
    };
    let (f, s) = open("mesh-hash", &bytes);
    let a = f.agent(&s);
    let ix = view(ROW, &s, a)?;
    let got = |ix: &ViewIndex, n: &str| -> Result<Option<String>, String> {
        Ok(named(ix, n, ROW)?
            .derived_value("mesh_hash")
            .map(str::to_owned))
    };
    step(
        got(&ix, "T")?.is_some() && hash_of("T").is_some() && hash_of("A").is_some(),
        ROW,
        "control",
        || format!("T: {:?}, the read's {:?}", got(&ix, "T"), hash_of("T")),
    )?;
    let (ha, hb) = (got(&ix, "A")?, got(&ix, "B")?);
    step(
        ha == hash_of("A") && hb == hash_of("A"),
        ROW,
        "shared",
        || format!("A {ha:?}, B {hb:?}, the read's {:?}", hash_of("A")),
    )?;
    step(got(&ix, "T")? == hash_of("T"), ROW, "its own", || {
        format!("T {:?}, the read's {:?}", got(&ix, "T"), hash_of("T"))
    })?;
    step(
        got(&ix, "E")?.is_none() && got(&ix, "Bad")?.is_none(),
        ROW,
        "none",
        || format!("E {:?}, Bad {:?}", got(&ix, "E"), got(&ix, "Bad")),
    )?;
    let addr = named(&ix, "A", ROW)?.key.clone();
    let o = run_based(&s, a, &["object", "place", &addr, "--loc", "4,5,6"])
        .map_err(|e| format!("{ROW}: move: {e}"))?;
    step(o.exit == 0, ROW, "move", || stdout(&o))?;
    let after = view(ROW, &s, a)?;
    step(got(&after, "A")? == ha, ROW, "after a move", || {
        format!("A {:?} after the move, {ha:?} before", got(&after, "A"))
    })
}

/// `S` holds `Vis`, `Own` (its own flag set) and the collections `Off` (hidden: `InOff`,
/// `Both`, and `Inner` holding `Deep`) and `On` (`Both`).
fn hidden_scene() -> Vec<u8> {
    let mut b = SceneBuilder::meshes(403, MeshLayouts::LAYERS);
    b.file_global(Some("S"));
    b.scene("S", &["Vis", "Own"], &["Off", "On"], None);
    b.collection_flagged("Off", &["InOff", "Both"], &["Inner"], 1);
    b.collection("Inner", &["Deep"], &[]);
    b.collection("On", &["Both"], &[]);
    b.object("Vis", &mesh_obj("Cube"));
    b.object(
        "Own",
        &ObjectSpec {
            restrict: 1,
            ..mesh_obj("Cube")
        },
    );
    for n in ["InOff", "Both", "Deep"] {
        b.object(n, &mesh_obj("Cube"));
    }
    b.mesh_with("Cube", &cube(), MeshLayouts::LAYERS);
    b.bytes()
}

fn hidden_by(ix: &ViewIndex, row: &str) -> Result<Vec<(String, Option<String>)>, String> {
    ["Vis", "Own", "InOff", "Both", "Deep"]
        .iter()
        .map(|n| {
            Ok((
                (*n).to_owned(),
                named(ix, n, row)?
                    .derived_value("hidden_by")
                    .map(str::to_owned),
            ))
        })
        .collect()
}

/// `hidden_by` by the rule: `self` for its own flag, the outermost hidden collection for one
/// hidden only through collections, none when one way shows it. A person shows `Off`: every
/// object below it is shown again, those the change didn't touch included. The live pack
/// refuses a collection's field at the call (`blend_live_change`), so the row opens the scene
/// with the kit's edit check off ([`super::unchecked::NoEditCheck`]): its subject is the view's
/// derive of the change.
pub fn row_view_hidden_by() -> Result<(), String> {
    const ROW: &str = "row_view_hidden_by";
    let (f, s) = super::unchecked::open_unchecked("hidden-by", &hidden_scene());
    let host = f.person(&s);
    let ix = view(ROW, &s, host)?;
    let off = container(&ix, "Off", ROW)?;
    let by = Some(format!("collection:{off}"));
    let want = vec![
        ("Vis".to_owned(), None),
        ("Own".to_owned(), Some("self".to_owned())),
        ("InOff".to_owned(), by.clone()),
        ("Both".to_owned(), None),
        ("Deep".to_owned(), by),
    ];
    let got = hidden_by(&ix, ROW)?;
    step(got == want, ROW, "control", || format!("{got:?}"))?;
    let mut opts = EditOptions::new();
    opts.base = Some(s.status().map_err(|e| format!("{ROW}: {e}"))?.version);
    let edit = k2_mpc_base::doc::Edit::Map {
        root: COLL_PROPS.to_owned(),
        ops: vec![MapOp::Set {
            key: format!("c{}\u{1f}hidden", off.trim_start_matches("@coll:")),
            value: Value::str("0"),
        }],
    };
    // the read after the change, with the kit's whole-build check off: the row checks it
    let checks = k2_mpc_base::differential::set_view_checks(false);
    let shown = s.edit_ops(host, vec![edit], &opts);
    let after = view(ROW, &s, host);
    k2_mpc_base::differential::set_view_checks(checks);
    shown.map_err(|e| format!("{ROW}: show: the change wasn't taken: {e}"))?;
    let got = hidden_by(&after?, ROW)?;
    let want = vec![
        ("Vis".to_owned(), None),
        ("Own".to_owned(), Some("self".to_owned())),
        ("InOff".to_owned(), None),
        ("Both".to_owned(), None),
        ("Deep".to_owned(), None),
    ];
    step(got == want, ROW, "after Off is shown", || {
        format!("{got:?}")
    })
}

/// `current_scene` marks the master collection of the scene the mesh read draws (the file's
/// current scene, `B` here, not the first), and no other container; `hidden_by` gives `self`
/// for an object of another scene with its own flag, nothing for one without.
pub fn row_view_current_scene() -> Result<(), String> {
    const ROW: &str = "row_view_current_scene";
    let mut b = SceneBuilder::meshes(403, MeshLayouts::LAYERS);
    b.file_global(Some("B"));
    b.scene("A", &["a", "off"], &[], None);
    b.scene("B", &["b"], &[], None);
    b.object("a", &mesh_obj("Cube"));
    b.object(
        "off",
        &ObjectSpec {
            restrict: 1,
            ..mesh_obj("Cube")
        },
    );
    b.object("b", &mesh_obj("Cube"));
    b.mesh_with("Cube", &cube(), MeshLayouts::LAYERS);
    let (f, s) = open("current-scene", &b.bytes());
    let a = f.agent(&s);
    let ix = view(ROW, &s, a)?;
    let (ca, cb) = (container(&ix, "A", ROW)?, container(&ix, "B", ROW)?);
    step(ix.containers.len() == 2, ROW, "control", || {
        format!("{} containers", ix.containers.len())
    })?;
    let marked: Vec<(String, Option<String>)> = ix
        .containers
        .iter()
        .map(|c| {
            (
                c.key.clone(),
                c.derived_value("current_scene").map(str::to_owned),
            )
        })
        .collect();
    let want = vec![(ca, None), (cb, Some("true".to_owned()))];
    step(marked == want, ROW, "the scene drawn", || {
        format!("{marked:?}")
    })?;
    let of = |n: &str| -> Result<Option<String>, String> {
        Ok(named(&ix, n, ROW)?
            .derived_value("hidden_by")
            .map(str::to_owned))
    };
    step(
        of("a")?.is_none() && of("off")? == Some("self".to_owned()) && of("b")?.is_none(),
        ROW,
        "another scene's objects",
        || format!("a {:?}, off {:?}, b {:?}", of("a"), of("off"), of("b")),
    )
}
