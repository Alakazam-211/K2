//! A live change's rules (`k2_mpc_blend::live::check_edit`): rows shared by
//! `tests/check_edit.rs` and the planted bugs in `tests/mutants.rs`, each returning
//! `Err("<row>: <step>: ...")`. Each rule has its row, with a control that the same kind of edit
//! on an object the rule doesn't hold is taken; the last row checks the verdict against the
//! write-back's (`splice::encode_from_paired`) on generated files, one candidate edit at a time.
#![allow(dead_code)]

use std::collections::BTreeMap;

use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Key, ListOp, MapOp, Value};
use k2_mpc_base::session::{Pack, PackError, SpliceError, View};
use k2_mpc_blend::live::model::{floats_text, parse_floats};
use k2_mpc_blend::live::{
    check_edit, errors, BlendLive, EditRefusal, Rule, COLLS, COLL_PROPS, OBJS, OBJ_PROPS, SCENE,
};
use k2_mpc_blend::scene::key_of;
use k2_mpc_blend::testgen::scenes;
use k2_mpc_blend::verbs::{self, Verb};
use k2_mpc_blend::ReadLimits;

use crate::driven_support;
use crate::live_support::{self, outliner, Opts};

type Row = Result<(), String>;
type Roots = Vec<(String, Neutral)>;

fn step(ok: bool, row: &str, name: &str, why: impl FnOnce() -> String) -> Row {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {name}: {}", why()))
    }
}

/// A file's live content, as the pack decodes it.
fn decoded(row: &str, bytes: &[u8]) -> Result<Roots, String> {
    BlendLive::default()
        .decode(bytes)
        .map(|d| d.roots)
        .map_err(|d| format!("{row}: decode: {}: {}", d.code, d.message))
}

/// The key of the object named `name`.
fn obj(row: &str, roots: &[(String, Neutral)], name: &str) -> Result<String, String> {
    live_support::obj_named(roots, name).ok_or_else(|| format!("{row}: no object {name}"))
}

fn field(roots: &[(String, Neutral)], k: &str, fld: &str) -> Option<String> {
    live_support::field(roots, OBJ_PROPS, k, fld)
}

fn set(k: &str, fld: &str, v: &str) -> MapOp {
    MapOp::Set {
        key: format!("{k}\u{1f}{fld}"),
        value: Value::str(v),
    }
}

fn del(k: &str, fld: &str) -> MapOp {
    MapOp::Delete {
        key: format!("{k}\u{1f}{fld}"),
    }
}

fn props(ops: Vec<MapOp>) -> Edit {
    Edit::Map {
        root: OBJ_PROPS.to_owned(),
        ops,
    }
}

fn list(root: &str, ops: Vec<ListOp>) -> Edit {
    Edit::List {
        root: root.to_owned(),
        ops,
    }
}

fn key(k: &str) -> Key {
    Key::new(k).expect("a list key")
}

/// `k`'s `xform` with component `i` changed by `by`, as the store writes it.
fn moved(roots: &[(String, Neutral)], k: &str, i: usize, by: f32) -> Option<String> {
    moved_by(roots, k, &[(i, by)])
}

/// `k`'s `xform` with each component `i` changed by its `by`.
fn moved_by(roots: &[(String, Neutral)], k: &str, by: &[(usize, f32)]) -> Option<String> {
    let mut x = parse_floats::<9>(&field(roots, k, "xform")?)?;
    for (i, v) in by {
        x[*i] += v;
    }
    Some(floats_text(&x))
}

/// `@obj:N` of `k` (`oN`).
fn addr(k: &str) -> String {
    format!("@obj:{}", &k[1..])
}

/// The new name's file key.
fn nid(name: &str) -> String {
    key_of("OB", name.as_bytes(), None).unwrap_or_default()
}

fn taken(row: &str, at: &str, view: &View, edits: &[Edit]) -> Row {
    let v = check_edit(view, edits);
    step(v.is_ok(), row, at, || {
        format!("refused, want taken: {:?}", v.err())
    })
}

/// `edits` refused by `rule`, its code `code` and the text starting with it, about `address`
/// when one is given.
fn refused(
    row: &str,
    at: &str,
    view: &View,
    edits: &[Edit],
    rule: Rule,
    address: Option<&str>,
) -> Result<EditRefusal, String> {
    let r = match check_edit(view, edits) {
        Ok(()) => return Err(format!("{row}: {at}: taken, want refused {rule:?}")),
        Err(r) => r,
    };
    let code_first = match rule.code() {
        Some(c) => r.code() == Some(c) && r.text().starts_with(&format!("{c}: ")),
        None => r.code().is_none() && r.text() == r.message,
    };
    step(
        r.rule == rule && code_first && (address.is_none() || r.address.as_deref() == address),
        row,
        at,
        || format!("{r:?} ({}), want {rule:?} about {address:?}", r.text()),
    )?;
    Ok(r)
}

/// The outliner scene's content and view: `Cube` (`edit` `1`), `Spin` (`t`: a quaternion),
/// `Walker` (`a`: an action), `Held` (`a`: a constraint), `Far` (`0`: linked), checked.
fn outliner_view(row: &str) -> Result<(Roots, View), String> {
    let roots = decoded(row, &outliner(&Opts::default()))?;
    for (name, want) in [
        ("Cube", "1"),
        ("Spin", "t"),
        ("Walker", "a"),
        ("Held", "a"),
        ("Far", "0"),
    ] {
        let k = obj(row, &roots, name)?;
        let edit = field(&roots, &k, "edit");
        step(edit.as_deref() == Some(want), row, "control", || {
            format!("{name}'s edit {edit:?}, want {want}")
        })?;
    }
    let view = View::of_content(&roots);
    Ok((roots, view))
}

/// What the live verbs write is taken: `Cube` placed whole, its visibility flipped and renamed
/// with its `nid`; `Spin`'s location and scale (its rotation kept); `Walker`'s and `Held`'s
/// visibility and rename; an `xform` written with the same floats; nothing; and an edit of the
/// scene map, which isn't written back.
pub fn row_check_edit_takes_what_live_writes() -> Row {
    const ROW: &str = "row_check_edit_takes_what_live_writes";
    let (roots, view) = outliner_view(ROW)?;
    let o = |n: &str| obj(ROW, &roots, n);
    let (cube, spin, walker, held) = (o("Cube")?, o("Spin")?, o("Walker")?, o("Held")?);
    let mut all = parse_floats::<9>(&field(&roots, &cube, "xform").unwrap_or_default())
        .ok_or_else(|| format!("{ROW}: Cube's xform"))?;
    for v in &mut all {
        *v += 0.25;
    }
    let cases: Vec<(&str, Vec<Edit>)> = vec![
        ("nothing", Vec::new()),
        (
            "Cube placed",
            vec![props(vec![set(&cube, "xform", &floats_text(&all))])],
        ),
        (
            "Cube's visibility",
            vec![props(vec![
                set(&cube, "hidden", "1"),
                set(&cube, "norender", "1"),
            ])],
        ),
        (
            "Cube renamed",
            vec![props(vec![
                set(&cube, "name", "Box"),
                set(&cube, "nid", &nid("Box")),
            ])],
        ),
        (
            "Spin's location and scale",
            vec![props(vec![set(
                &spin,
                "xform",
                &moved_by(&roots, &spin, &[(0, 1.5), (6, 1.0)])
                    .ok_or_else(|| format!("{ROW}: Spin's xform"))?,
            )])],
        ),
        (
            "Walker's visibility",
            vec![props(vec![set(&walker, "noselect", "1")])],
        ),
        (
            "Walker renamed",
            vec![props(vec![
                set(&walker, "name", "Runner"),
                set(&walker, "nid", &nid("Runner")),
            ])],
        ),
        (
            "Held's visibility and rename",
            vec![props(vec![
                set(&held, "hidden", "1"),
                set(&held, "name", "Kept"),
                set(&held, "nid", &nid("Kept")),
            ])],
        ),
        (
            "Cube's xform with the same floats",
            vec![props(vec![set(
                &cube,
                "xform",
                &field(&roots, &cube, "xform").unwrap_or_default(),
            )])],
        ),
        (
            "the scene map",
            vec![Edit::Map {
                root: SCENE.to_owned(),
                ops: vec![MapOp::Set {
                    key: "mark/o".into(),
                    value: Value::str("99"),
                }],
            }],
        ),
    ];
    for (at, edits) in &cases {
        taken(ROW, at, &view, edits)?;
    }
    println!("{ROW}: {} edits taken", cases.len());
    Ok(())
}

/// Any op on a list root is refused, `Rule::List`, no code: an object inserted, deleted, moved,
/// retyped; a collection deleted. Control: an edit of the list with no op is taken.
pub fn row_check_edit_refuses_list_ops() -> Row {
    const ROW: &str = "row_check_edit_refuses_list_ops";
    let (roots, view) = outliner_view(ROW)?;
    let cube = key(&obj(ROW, &roots, "Cube")?);
    taken(ROW, "control", &view, &[list(OBJS, Vec::new())])?;
    let first_coll = live_support::list(&roots, COLLS)
        .first()
        .map(|(k, _)| key(k))
        .ok_or_else(|| format!("{ROW}: no collection"))?;
    for (at, e) in [
        (
            "insert",
            list(
                OBJS,
                vec![ListOp::Insert {
                    key: key("o999"),
                    at: Anchor::End,
                    value: Value::str("empty"),
                }],
            ),
        ),
        (
            "delete",
            list(OBJS, vec![ListOp::Delete { key: cube.clone() }]),
        ),
        (
            "move",
            list(
                OBJS,
                vec![ListOp::Move {
                    key: cube.clone(),
                    to: Anchor::Start,
                }],
            ),
        ),
        (
            "retype",
            list(
                OBJS,
                vec![ListOp::Set {
                    key: cube.clone(),
                    value: Value::str("camera"),
                }],
            ),
        ),
        (
            "a collection deleted",
            list(COLLS, vec![ListOp::Delete { key: first_coll }]),
        ),
    ] {
        refused(ROW, at, &view, &[e], Rule::List, None)?;
    }
    Ok(())
}

/// A collection's field is refused, `Rule::Collection`, no code; control: one set to the value
/// it holds is taken.
pub fn row_check_edit_refuses_collection_fields() -> Row {
    const ROW: &str = "row_check_edit_refuses_collection_fields";
    let (roots, view) = outliner_view(ROW)?;
    let (k, v) = roots
        .iter()
        .find_map(|(r, n)| match n {
            Neutral::Map(m) if r == COLL_PROPS => m
                .iter()
                .find(|(k, _)| k.ends_with("\u{1f}name"))
                .map(|(k, v)| (k.clone(), v.clone())),
            _ => None,
        })
        .ok_or_else(|| format!("{ROW}: no collection name"))?;
    let coll = |value: Value| Edit::Map {
        root: COLL_PROPS.to_owned(),
        ops: vec![MapOp::Set {
            key: k.clone(),
            value,
        }],
    };
    taken(ROW, "control", &view, &[coll(v)])?;
    refused(
        ROW,
        "renamed",
        &view,
        &[coll(Value::str("Elsewhere"))],
        Rule::Collection,
        None,
    )?;
    Ok(())
}

/// A transform change of an object whose transform is driven (`edit` `a`) is refused,
/// `blend_driven`, the object and the channel (`xform`) named: `Walker` (an action), its z, its
/// x alone (nothing animates x: the live tier holds one `edit` per object) and its rotation;
/// `Held` (a constraint), its scale. Control: the same changes to `Cube` are taken.
pub fn row_check_edit_refuses_driven_xform() -> Row {
    const ROW: &str = "row_check_edit_refuses_driven_xform";
    let (roots, view) = outliner_view(ROW)?;
    let o = |n: &str| obj(ROW, &roots, n);
    let (cube, walker, held) = (o("Cube")?, o("Walker")?, o("Held")?);
    let x =
        |k: &str, i: usize| moved(&roots, k, i, 1.0).ok_or_else(|| format!("{ROW}: {k}'s xform"));
    for (at, i) in [("z", 2), ("x", 0), ("rotation", 3)] {
        taken(
            ROW,
            &format!("control: Cube's {at}"),
            &view,
            &[props(vec![set(&cube, "xform", &x(&cube, i)?)])],
        )?;
        let r = refused(
            ROW,
            &format!("Walker's {at}"),
            &view,
            &[props(vec![set(&walker, "xform", &x(&walker, i)?)])],
            Rule::Driven,
            Some(addr(&walker).as_str()),
        )?;
        step(r.channel.as_deref() == Some("xform"), ROW, at, || {
            format!("channel {:?}", r.channel)
        })?;
    }
    refused(
        ROW,
        "Held's scale",
        &view,
        &[props(vec![set(&held, "xform", &x(&held, 6)?)])],
        Rule::Driven,
        Some(addr(&held).as_str()),
    )?;
    Ok(())
}

/// A rotation change of an object that rotates by quaternion (`edit` `t`, `Spin`) is refused,
/// `blend_rotation_mode`; its location is taken.
pub fn row_check_edit_refuses_rotation_on_quaternion() -> Row {
    const ROW: &str = "row_check_edit_refuses_rotation_on_quaternion";
    let (roots, view) = outliner_view(ROW)?;
    let spin = obj(ROW, &roots, "Spin")?;
    let x = |i: usize| moved(&roots, &spin, i, 1.0).ok_or_else(|| format!("{ROW}: Spin's xform"));
    taken(
        ROW,
        "control: its location",
        &view,
        &[props(vec![set(&spin, "xform", &x(0)?)])],
    )?;
    refused(
        ROW,
        "its rotation",
        &view,
        &[props(vec![set(&spin, "xform", &x(4)?)])],
        Rule::RotationMode,
        Some(addr(&spin).as_str()),
    )?;
    Ok(())
}

/// Any change to an object nothing writes live (`edit` `0`: `Far`, linked from a library) is
/// refused, `blend_not_editable`: its visibility, its name. Control: `Cube`'s visibility.
pub fn row_check_edit_refuses_never() -> Row {
    const ROW: &str = "row_check_edit_refuses_never";
    let (roots, view) = outliner_view(ROW)?;
    let (cube, far) = (obj(ROW, &roots, "Cube")?, obj(ROW, &roots, "Far")?);
    taken(
        ROW,
        "control",
        &view,
        &[props(vec![set(&cube, "hidden", "1")])],
    )?;
    for (at, ops) in [
        ("visibility", vec![set(&far, "hidden", "1")]),
        (
            "rename",
            vec![set(&far, "name", "Near"), set(&far, "nid", &nid("Near"))],
        ),
    ] {
        refused(
            ROW,
            at,
            &view,
            &[props(ops)],
            Rule::NotEditable,
            Some(addr(&far).as_str()),
        )?;
    }
    Ok(())
}

/// A visibility change of an object with an animated bit (`visanim`) is refused,
/// `blend_driven`, the animated bits named, whichever bit it changes: `Blinking` (an action
/// animates `hide_viewport`) its render bit, `Ghost` (a driver of `hide_render`) its viewport
/// bit, `Lost` (animation data the reading can't follow: all three). Control: `Plain`'s and
/// `Mover`'s (its location driven, no bit) are taken, and `Blinking`'s placement.
pub fn row_check_edit_refuses_animated_visibility() -> Row {
    const ROW: &str = "row_check_edit_refuses_animated_visibility";
    let roots = decoded(ROW, &live_support::blinking_scene())?;
    let view = View::of_content(&roots);
    let o = |n: &str| obj(ROW, &roots, n);
    for (name, fld) in [("Plain", "hidden"), ("Mover", "norender")] {
        taken(
            ROW,
            &format!("control: {name}"),
            &view,
            &[props(vec![set(&o(name)?, fld, "1")])],
        )?;
    }
    let blinking = o("Blinking")?;
    taken(
        ROW,
        "control: Blinking placed",
        &view,
        &[props(vec![set(
            &blinking,
            "xform",
            &moved(&roots, &blinking, 0, 1.0).unwrap_or_default(),
        )])],
    )?;
    for (name, fld, bits) in [
        ("Blinking", "norender", "hide_viewport"),
        ("Ghost", "hidden", "hide_render"),
        ("Lost", "noselect", "hide_viewport,hide_select,hide_render"),
    ] {
        let k = o(name)?;
        let r = refused(
            ROW,
            name,
            &view,
            &[props(vec![set(&k, fld, "1")])],
            Rule::Driven,
            Some(addr(&k).as_str()),
        )?;
        step(
            r.channel.as_deref() == Some(bits) && r.message.contains(bits),
            ROW,
            name,
            || {
                format!(
                    "channel {:?}, message {}, want {bits}",
                    r.channel, r.message
                )
            },
        )?;
    }
    Ok(())
}

/// A name goes with its `nid`: a `nid` alone, or a name whose `nid` isn't its file key,
/// `Rule::NameKey`; a name the file can't hold is `blend_name`: empty, with a NUL, past the
/// name field, another object's (`Lamp`); one a linked object holds (`Far`) is taken. Renames
/// are judged in the list's order, as the writer applies them: `Cube` to `Lamp` while `Lamp`
/// is renamed in the same edit is taken only when `Lamp` comes first.
pub fn row_check_edit_name_rules() -> Row {
    const ROW: &str = "row_check_edit_name_rules";
    let (roots, view) = outliner_view(ROW)?;
    let o = |n: &str| obj(ROW, &roots, n);
    let (cube, lamp) = (o("Cube")?, o("Lamp")?);
    let rename = |k: &str, name: &str| vec![set(k, "name", name), set(k, "nid", &nid(name))];
    taken(ROW, "control", &view, &[props(rename(&cube, "Box"))])?;
    taken(
        ROW,
        "a linked object's name",
        &view,
        &[props(rename(&cube, "Far"))],
    )?;
    refused(
        ROW,
        "nid alone",
        &view,
        &[props(vec![set(&cube, "nid", "@ob:Elsewhere")])],
        Rule::NameKey,
        Some(addr(&cube).as_str()),
    )?;
    refused(
        ROW,
        "name alone",
        &view,
        &[props(vec![set(&cube, "name", "Box")])],
        Rule::NameKey,
        Some(addr(&cube).as_str()),
    )?;
    let max: usize = roots
        .iter()
        .find_map(|(r, n)| match n {
            Neutral::Map(m) if r == SCENE => m.get("name_max")?.as_str()?.parse().ok(),
            _ => None,
        })
        .ok_or_else(|| format!("{ROW}: no name_max"))?;
    let long = "x".repeat(max + 1);
    for (at, name) in [
        ("empty", ""),
        ("a NUL", "Bo\0x"),
        ("past the name field", long.as_str()),
        ("Lamp's", "Lamp"),
    ] {
        refused(
            ROW,
            at,
            &view,
            &[props(vec![
                set(&cube, "name", name),
                set(&cube, "nid", "@ob:x"),
            ])],
            Rule::Name,
            Some(addr(&cube).as_str()),
        )?;
    }
    let pos = |k: &str| {
        live_support::list(&roots, OBJS)
            .iter()
            .position(|(x, _)| x == k)
    };
    let mut ops = rename(&lamp, "Zed");
    ops.extend(rename(&cube, "Lamp"));
    let lamp_first = pos(&lamp) < pos(&cube);
    let v = check_edit(&view, &[props(ops)]);
    step(
        v.is_ok() == lamp_first && v.as_ref().err().is_none_or(|r| r.rule == Rule::Name),
        ROW,
        "a swap",
        || format!("Lamp first {lamp_first}: {v:?}"),
    )
}

/// Any other field is refused (`Rule::Field`): `data`, `coll`, `parent`, `edit`, `visanim`,
/// `rotmode`; a value the field can't hold is `Rule::Value`: an `xform` removed or of two
/// numbers, a bit `2`, a name removed. Control: `Cube`'s `xform` taken.
pub fn row_check_edit_refuses_other_fields() -> Row {
    const ROW: &str = "row_check_edit_refuses_other_fields";
    let (roots, view) = outliner_view(ROW)?;
    let cube = obj(ROW, &roots, "Cube")?;
    taken(
        ROW,
        "control",
        &view,
        &[props(vec![set(
            &cube,
            "xform",
            &moved(&roots, &cube, 0, 1.0).unwrap_or_default(),
        )])],
    )?;
    for (fld, v) in [
        ("data", "mesh @me:Other"),
        ("coll", "c1"),
        ("parent", "o1"),
        ("edit", "a"),
        ("visanim", "hide_render"),
        ("rotmode", "quaternion"),
    ] {
        let r = refused(
            ROW,
            fld,
            &view,
            &[props(vec![set(&cube, fld, v)])],
            Rule::Field,
            Some(addr(&cube).as_str()),
        )?;
        step(r.message.contains(fld), ROW, fld, || r.message.clone())?;
    }
    for (at, op) in [
        ("xform removed", del(&cube, "xform")),
        ("xform of two numbers", set(&cube, "xform", "1,2")),
        ("a bit 2", set(&cube, "hidden", "2")),
        ("name removed", del(&cube, "name")),
    ] {
        refused(
            ROW,
            at,
            &view,
            &[props(vec![op])],
            Rule::Value,
            Some(addr(&cube).as_str()),
        )?;
    }
    Ok(())
}

/// The first rule broken refuses, whatever the edits' order: the lists before the collections
/// before the objects, and the objects in the list's order; a doc read but not written back
/// (its scene map's version past the writer's) refuses any change, `live_read_only`, before
/// anything else, and takes no change. An edit of a root the live schema doesn't have, of a
/// list root by a map's ops, or of text, is `Rule::Root`.
pub fn row_check_edit_first_rule() -> Row {
    const ROW: &str = "row_check_edit_first_rule";
    let (roots, view) = outliner_view(ROW)?;
    let o = |n: &str| obj(ROW, &roots, n);
    let (cube, walker) = (o("Cube")?, o("Walker")?);
    let field_op = props(vec![set(&cube, "data", "mesh @me:Other")]);
    let walker_op = props(vec![set(
        &walker,
        "xform",
        &moved(&roots, &walker, 0, 1.0).unwrap_or_default(),
    )]);
    let delete = list(OBJS, vec![ListOp::Delete { key: key(&cube) }]);
    let coll = Edit::Map {
        root: COLL_PROPS.to_owned(),
        ops: vec![MapOp::Set {
            key: "c1\u{1f}hidden".into(),
            value: Value::str("7"),
        }],
    };
    for edits in [
        vec![field_op.clone(), delete.clone()],
        vec![delete.clone(), field_op.clone()],
    ] {
        refused(ROW, "the list first", &view, &edits, Rule::List, None)?;
    }
    for edits in [
        vec![field_op.clone(), coll.clone()],
        vec![coll.clone(), field_op.clone()],
    ] {
        refused(
            ROW,
            "the collections next",
            &view,
            &edits,
            Rule::Collection,
            None,
        )?;
    }
    let pos = |k: &str| {
        live_support::list(&roots, OBJS)
            .iter()
            .position(|(x, _)| x == k)
    };
    let want = if pos(&cube) < pos(&walker) {
        Rule::Field
    } else {
        Rule::Driven
    };
    for edits in [
        vec![field_op.clone(), walker_op.clone()],
        vec![walker_op.clone(), field_op.clone()],
    ] {
        refused(
            ROW,
            "the objects in the list's order",
            &view,
            &edits,
            want,
            None,
        )?;
    }
    // read-only: the scene map's version past the writer's
    let mut ro = roots.clone();
    for (r, n) in &mut ro {
        if let (true, Neutral::Map(m)) = (r == SCENE, n) {
            m.insert("version".into(), Value::str("9.99"));
        }
    }
    let ro_view = View::of_content(&ro);
    taken(ROW, "read-only: nothing", &ro_view, &[])?;
    for edits in [
        vec![props(vec![set(&cube, "hidden", "1")])],
        vec![delete.clone()],
    ] {
        let r = refused(ROW, "read-only", &ro_view, &edits, Rule::ReadOnly, None)?;
        step(r.message.contains("9.99"), ROW, "read-only", || {
            r.message.clone()
        })?;
    }
    for (at, e) in [
        (
            "a root the schema doesn't have",
            Edit::Map {
                root: "nope".into(),
                ops: vec![MapOp::Set {
                    key: "k".into(),
                    value: Value::str("v"),
                }],
            },
        ),
        (
            "a list root by a map's ops",
            Edit::Map {
                root: OBJS.into(),
                ops: vec![MapOp::Delete { key: cube.clone() }],
            },
        ),
        (
            "text",
            Edit::Text {
                root: OBJS.into(),
                hunks: Vec::new(),
            },
        ),
    ] {
        refused(ROW, at, &view, &[e], Rule::Root, None)?;
    }
    Ok(())
}

// ---------- the file path keeps the writer's per-channel rule ----------

/// The file verb `object place` (`verbs::apply`, the writer used directly, not the live tier) on
/// a transform under constraints (`Held`: a track-to, a child-of and a track-to again) writes
/// it, with one `blend_constrained` naming the kinds once each; live, the same placement is
/// refused per object (`check_edit`, `blend_driven`). Control: `Plain` (no constraint) is
/// written with no warning and taken live.
pub fn row_file_verb_writes_constrained_live_refuses() -> Row {
    const ROW: &str = "row_file_verb_writes_constrained_live_refuses";
    let bytes = driven_support::constrained();
    let place = |object: &str| Verb::Place {
        object: object.into(),
        loc: Some([1.0, 2.0, 3.0]),
        rot: None,
        scale: None,
    };
    let plain = verbs::apply(&bytes, &place("@ob:Plain"), &ReadLimits::default())
        .map_err(|e| format!("{ROW}: control: refused: {e}"))?;
    step(plain.warnings.is_empty(), ROW, "control", || {
        format!("warnings {:?}", plain.warnings)
    })?;
    let w = verbs::apply(&bytes, &place("@ob:Held"), &ReadLimits::default())
        .map_err(|e| format!("{ROW}: the file verb: refused: {e}"))?;
    driven_support::warned(
        ROW,
        "the file verb",
        &w,
        "@ob:Held",
        &["bTrackToConstraint", "bChildOfConstraint"],
    )?;
    let got = driven_support::field_of(&w.bytes, "@ob:Held", "loc")
        .map_err(|e| format!("{ROW}: the file verb: {e}"))?;
    step(got == "1,2,3", ROW, "the file verb", || {
        format!("Held's loc reads back {got}")
    })?;
    let roots = decoded(ROW, &bytes)?;
    let view = View::of_content(&roots);
    let (held, plain) = (obj(ROW, &roots, "Held")?, obj(ROW, &roots, "Plain")?);
    // the location the file verb wrote, as the store writes it
    let x = |k: &str| {
        let mut x = field(&roots, k, "xform")
            .and_then(|t| parse_floats::<9>(&t))
            .ok_or_else(|| format!("{ROW}: {k}'s xform"))?;
        x[..3].copy_from_slice(&[1.0, 2.0, 3.0]);
        Ok::<String, String>(floats_text(&x))
    };
    taken(
        ROW,
        "control live",
        &view,
        &[props(vec![set(&plain, "xform", &x(&plain)?)])],
    )?;
    refused(
        ROW,
        "live",
        &view,
        &[props(vec![set(&held, "xform", &x(&held)?)])],
        Rule::Driven,
        Some(addr(&held).as_str()),
    )?;
    Ok(())
}

/// The file verb `object place` on `driven_quat.blend`'s `Driven` (an action animates its
/// location z): x and y written with z kept as stored (`0`), read back; z changed refused,
/// `blend_driven`, the channel named (`location[2]`). Live, a change of x alone is refused per
/// object (`check_edit`, `blend_driven`).
pub fn row_file_verb_writes_undriven_component_live_refuses() -> Row {
    const ROW: &str = "row_file_verb_writes_undriven_component_live_refuses";
    let bytes = scenes::driven_quat();
    let place = |v: [f32; 3]| Verb::Place {
        object: "@ob:Driven".into(),
        loc: Some(v),
        rot: None,
        scale: None,
    };
    let w = verbs::apply(&bytes, &place([-3.0, 2.0, 0.0]), &ReadLimits::default())
        .map_err(|e| format!("{ROW}: x and y: refused: {e}"))?;
    let got = driven_support::field_of(&w.bytes, "@ob:Driven", "loc")
        .map_err(|e| format!("{ROW}: x and y: {e}"))?;
    step(got == "-3,2,0", ROW, "x and y", || {
        format!("Driven's loc reads back {got}")
    })?;
    match verbs::apply(&bytes, &place([-3.0, 2.0, 1.0]), &ReadLimits::default()) {
        Ok(_) => return Err(format!("{ROW}: z: written, want refused blend_driven")),
        Err(e) => driven_support::refusal_is_driven(ROW, "z", &e, "location[2]")?,
    }
    let roots = decoded(ROW, &bytes)?;
    let view = View::of_content(&roots);
    let driven = obj(ROW, &roots, "Driven")?;
    refused(
        ROW,
        "live: x alone",
        &view,
        &[props(vec![set(
            &driven,
            "xform",
            &moved(&roots, &driven, 0, 1.0).ok_or_else(|| format!("{ROW}: Driven's xform"))?,
        )])],
        Rule::Driven,
        Some(addr(&driven).as_str()),
    )?;
    Ok(())
}

// ---------- check_edit and the write-back agree ----------

/// `content` with `edits` applied as the kit applies them (a list's anchors by key).
fn apply(content: &[(String, Neutral)], edits: &[Edit]) -> Roots {
    let mut c = content.to_vec();
    for e in edits {
        let Some((_, n)) = c.iter_mut().find(|(r, _)| r == e.root()) else {
            continue;
        };
        match (e, n) {
            (Edit::Map { ops, .. }, Neutral::Map(m)) => {
                for op in ops {
                    match op {
                        MapOp::Set { key, value } => {
                            m.insert(key.clone(), value.clone());
                        }
                        MapOp::Delete { key } => {
                            m.remove(key);
                        }
                        _ => {}
                    }
                }
            }
            (Edit::List { ops, .. }, Neutral::List(l)) => {
                for op in ops {
                    let pos = |l: &[(Key, Value)], at: &Anchor| {
                        let of = |k: &Key| l.iter().position(|(x, _)| x == k);
                        if matches!(at, Anchor::Start) {
                            0
                        } else if let Anchor::After(k) = at {
                            of(k).map_or(l.len(), |i| i + 1)
                        } else if let Anchor::Before(k) = at {
                            of(k).unwrap_or(0)
                        } else {
                            l.len()
                        }
                    };
                    match op {
                        ListOp::Insert { key, at, value } => {
                            let i = pos(l, at);
                            l.insert(i, (key.clone(), value.clone()));
                        }
                        ListOp::Delete { key } => l.retain(|(k, _)| k != key),
                        ListOp::Move { key, to } => {
                            if let Some(i) = l.iter().position(|(k, _)| k == key) {
                                let x = l.remove(i);
                                let j = pos(l, to);
                                l.insert(j, x);
                            }
                        }
                        ListOp::Set { key, value } => {
                            for (k, v) in l.iter_mut() {
                                if k == key {
                                    *v = value.clone();
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    c
}

/// A verdict: taken, or refused with its registered code (`None`: a rule with no code).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    Taken,
    Refused(Option<String>),
}

/// The write-back's verdict on `applied`: the code its `PackError` text starts with, among the
/// pack's registered entries.
fn write_back(
    pack: &BlendLive,
    bytes: &[u8],
    roots: &[(String, Neutral)],
    codec: &[u8],
    applied: &[(String, Neutral)],
) -> Result<(Verdict, String), String> {
    let pairing = pack.pairing(roots, codec).map_err(|e| e.to_string())?;
    match pack.encode_from_paired(bytes, &pairing, applied, codec) {
        Ok(_) => Ok((Verdict::Taken, String::new())),
        Err(SpliceError::Pack(PackError(m))) => {
            let code = errors::ERRORS
                .iter()
                .map(|e| e.name)
                .find(|c| m.starts_with(&format!("{c}: ")));
            Ok((Verdict::Refused(code.map(str::to_owned)), m))
        }
        Err(other) => Err(format!("the write-back failed otherwise: {other:?}")),
    }
}

/// The candidate edits on one file: per object (at most 12), its location, rotation and scale
/// moved, its `xform` with the same floats and removed, two bits flipped and one set to `2`, a
/// rename with its `nid`, to another object's name, empty, a name alone, a `nid` alone, its
/// `data` and its `edit` changed; then a collection's field, each kind of list op, two objects
/// placed in one edit, and two renames in one edit, the second to the first's old name.
fn candidates(roots: &[(String, Neutral)]) -> Vec<(String, Vec<Edit>)> {
    let objs: Vec<String> = live_support::list(roots, OBJS)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    let name = |k: &str| field(roots, k, "name").unwrap_or_default();
    let mut out: Vec<(String, Vec<Edit>)> = Vec::new();
    for (i, k) in objs.iter().take(12).enumerate() {
        let mut one = |at: &str, ops: Vec<MapOp>| out.push((format!("{k} {at}"), vec![props(ops)]));
        if let Some(x) = field(roots, k, "xform") {
            for (at, c, by) in [("loc", 0, 1.0), ("rot", 3, 0.5), ("scale", 6, 1.0)] {
                if let Some(m) = moved(roots, k, c, by) {
                    one(at, vec![set(k, "xform", &m)]);
                }
            }
            one("xform same", vec![set(k, "xform", &x)]);
            one("xform removed", vec![del(k, "xform")]);
        }
        let flip = |fld: &str| {
            if field(roots, k, fld).as_deref() == Some("1") {
                "0"
            } else {
                "1"
            }
        };
        one("hidden flip", vec![set(k, "hidden", flip("hidden"))]);
        one("render flip", vec![set(k, "norender", flip("norender"))]);
        one("hidden 2", vec![set(k, "hidden", "2")]);
        let new = format!("Rn{i}");
        one(
            "rename",
            vec![set(k, "name", &new), set(k, "nid", &nid(&new))],
        );
        if let Some(other) = objs.iter().find(|o| *o != k && !name(o).is_empty()) {
            let n = name(other);
            one(
                "rename taken",
                vec![set(k, "name", &n), set(k, "nid", &nid(&n))],
            );
        }
        one(
            "rename empty",
            vec![set(k, "name", ""), set(k, "nid", "@ob:")],
        );
        one("name alone", vec![set(k, "name", &new)]);
        one("nid alone", vec![set(k, "nid", "@ob:Elsewhere")]);
        one("data", vec![set(k, "data", "mesh @me:Other")]);
        let edit = if field(roots, k, "edit").as_deref() == Some("1") {
            "a"
        } else {
            "1"
        };
        one("edit", vec![set(k, "edit", edit)]);
    }
    if let Some((ck, v)) = roots.iter().find_map(|(r, n)| match n {
        Neutral::Map(m) if r == COLL_PROPS => m
            .iter()
            .next()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_owned())),
        _ => None,
    }) {
        out.push((
            "a collection's field".into(),
            vec![Edit::Map {
                root: COLL_PROPS.to_owned(),
                ops: vec![MapOp::Set {
                    key: ck,
                    value: Value::str(format!("{v}x")),
                }],
            }],
        ));
    }
    if let (Some(first), Some(last)) = (objs.first(), objs.last()) {
        let (f, l) = (key(first), key(last));
        // each op changes the list: a move of the last object to the start only when it isn't
        // the first, a retype to another kind
        let kind = live_support::list(roots, OBJS)
            .first()
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let retype = if kind == "light" { "camera" } else { "light" };
        if first != last {
            out.push((
                "list move".into(),
                vec![list(
                    OBJS,
                    vec![ListOp::Move {
                        key: l.clone(),
                        to: Anchor::Start,
                    }],
                )],
            ));
        }
        for (at, e) in [
            (
                "delete",
                list(OBJS, vec![ListOp::Delete { key: f.clone() }]),
            ),
            (
                "insert",
                list(
                    OBJS,
                    vec![ListOp::Insert {
                        key: key("o999"),
                        at: Anchor::End,
                        value: Value::str("empty"),
                    }],
                ),
            ),
            (
                "retype",
                list(
                    OBJS,
                    vec![ListOp::Set {
                        key: f.clone(),
                        value: Value::str(retype),
                    }],
                ),
            ),
        ] {
            out.push((format!("list {at}"), vec![e]));
        }
        if first != last {
            let mut ops = Vec::new();
            for k in [first, last] {
                if let Some(m) = moved(roots, k, 0, 2.0) {
                    ops.push(set(k, "xform", &m));
                }
            }
            out.push(("two placed".into(), vec![props(ops)]));
            let (nf, nl) = (name(first), name(last));
            let mut ops = vec![set(first, "name", "Swap"), set(first, "nid", &nid("Swap"))];
            ops.extend([set(last, "name", &nf), set(last, "nid", &nid(&nf))]);
            out.push(("two renamed, last to first's".into(), vec![props(ops)]));
            let mut ops = vec![set(last, "name", "Swap"), set(last, "nid", &nid("Swap"))];
            ops.extend([set(first, "name", &nl), set(first, "nid", &nid(&nl))]);
            out.push(("two renamed, first to last's".into(), vec![props(ops)]));
        }
    }
    if let Some((ck, _)) = live_support::list(roots, COLLS).first() {
        out.push((
            "collection delete".into(),
            vec![list(COLLS, vec![ListOp::Delete { key: key(ck) }])],
        ));
    }
    out
}

/// **One rule.** On every generated file but the large one (`testgen::scenes::ALL`), and on the
/// live rows' scenes (the outliner at 4.03, placed, at 5.02 and 5.03; the scenes with animated
/// bits, channels, a constraint), each candidate edit ([`candidates`]) gets the same verdict
/// from `check_edit` on the decoded doc's view as from the write-back of the doc with the edit
/// applied (`encode_from_paired`, the kit's hook): taken by both, or refused by both with the
/// same registered code (or none). The counts printed: files, edits, taken, refused by code;
/// each code the rules give is reached.
pub fn row_check_edit_agrees_with_write_back() -> Row {
    const ROW: &str = "row_check_edit_agrees_with_write_back";
    let pack = BlendLive::default();
    let mut files: Vec<(String, Vec<u8>)> = scenes::ALL
        .iter()
        .filter(|(n, _)| *n != "large.blend")
        .map(|(n, w)| ((*n).to_owned(), w()))
        .collect();
    for (n, o) in [
        ("outliner", Opts::default()),
        (
            "outliner placed",
            Opts {
                placed: true,
                ..Opts::default()
            },
        ),
        (
            "outliner 5.02",
            Opts {
                version: 502,
                ..Opts::default()
            },
        ),
        (
            "outliner 5.03",
            Opts {
                version: 503,
                ..Opts::default()
            },
        ),
    ] {
        files.push((n.to_owned(), outliner(&o)));
    }
    files.extend([
        ("blinking scene".to_owned(), live_support::blinking_scene()),
        ("blinking".to_owned(), driven_support::blinking()),
        ("channels".to_owned(), driven_support::channels()),
        ("tinted".to_owned(), driven_support::tinted()),
        ("constrained".to_owned(), driven_support::constrained()),
    ]);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut edits_n = 0usize;
    for (file, bytes) in &files {
        let d = pack
            .decode(bytes)
            .map_err(|d| format!("{ROW}: {file}: decode: {}: {}", d.code, d.message))?;
        let view = View::of_content(&d.roots);
        for (at, edits) in candidates(&d.roots) {
            let ours = match check_edit(&view, &edits) {
                Ok(()) => Verdict::Taken,
                Err(r) => Verdict::Refused(r.code().map(str::to_owned)),
            };
            let applied = apply(&d.roots, &edits);
            let (theirs, text) = write_back(&pack, bytes, &d.roots, &d.codec, &applied)
                .map_err(|e| format!("{ROW}: {file}: {at}: {e}"))?;
            step(ours == theirs, ROW, &format!("{file}: {at}"), || {
                format!(
                    "check_edit {ours:?} ({:?}), the write-back {theirs:?} ({text})",
                    check_edit(&view, &edits).err().map(|r| r.text())
                )
            })?;
            edits_n += 1;
            let label = match &ours {
                Verdict::Taken => "taken".to_owned(),
                Verdict::Refused(c) => format!("refused {}", c.as_deref().unwrap_or("(no code)")),
            };
            *counts.entry(label).or_default() += 1;
        }
    }
    let shown: Vec<String> = counts.iter().map(|(k, n)| format!("{k} {n}")).collect();
    println!(
        "{ROW}: {} files, {edits_n} edits: {}",
        files.len(),
        shown.join(", ")
    );
    for want in [
        "taken",
        "refused blend_driven",
        "refused blend_rotation_mode",
        "refused blend_not_editable",
        "refused blend_name",
        "refused (no code)",
    ] {
        step(
            counts.get(want).copied().unwrap_or(0) > 0,
            ROW,
            "counts",
            || format!("none {want}: {shown:?}"),
        )?;
    }
    Ok(())
}
