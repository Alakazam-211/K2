//! The writer on animated channels: rows shared by `tests/write_driven.rs` and the planted bugs
//! in `tests/mutants.rs`, each returning `Err("<row>, <step>: ...")`. A change to a channel an
//! action (directly, or through an NLA strip) or a driver animates is refused, typed
//! (`blend_driven`, exit 2, the channel named, `object show` next), by the writer, the file
//! verbs and the live tier alike; a channel nothing animates is written and reads back. A
//! visibility bit an action or a driver animates is a channel too (`hide_viewport`), refused
//! when a change flips it. A transform written to an object with constraints is written, with a
//! warning naming the constraints' kinds (`blend_constrained`). Every file is generated here
//! (`k2_mpc_blend::testgen`).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::Value;
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::session::Pack;
use k2_mpc_blend::anim::{drives, Drives, VISIBILITY_PATHS};
use k2_mpc_blend::live::model::{self, EditMode, LiveLimits, LiveScene};
use k2_mpc_blend::live::{prop, BlendLive, OBJS, OBJ_PROPS};
use k2_mpc_blend::testgen::{scenes, Driven, MeshLayouts, ObjectSpec, SceneBuilder};
use k2_mpc_blend::verbs::{apply, Verb, VerbError};
use k2_mpc_blend::write::decompress;
use k2_mpc_blend::{
    read_blend, read_scene, write, BlendFile, Change, Edit, ReadLimits, SceneModel, WriteError,
    WriteWarning, Written,
};

pub type Row = Result<(), String>;

/// `Err("<row>, <step>: <what>")` unless `ok`.
pub fn step(row: &str, step: &str, ok: bool, what: impl std::fmt::Display) -> Row {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}, {step}: {what}"))
    }
}

fn limits() -> ReadLimits {
    ReadLimits::default()
}

/// The scene model of `bytes`.
pub fn model_of(bytes: &[u8]) -> Result<SceneModel, String> {
    let (_, raw) = decompress(bytes, &limits()).map_err(|e| e.to_string())?;
    let f = read_blend(&mut &raw[..], &limits()).map_err(|e| e.to_string())?;
    read_scene(&f).map_err(|e| e.to_string())
}

/// Field `name` of the object `key` in `bytes`, as the scene model prints it (`-3,2,0`).
pub fn field_of(bytes: &[u8], key: &str, name: &str) -> Result<String, String> {
    let m = model_of(bytes)?;
    let id = m.by_key(key).ok_or_else(|| format!("no {key}"))?;
    id.fields
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.clone())
        .ok_or_else(|| format!("{key} has no {name}"))
}

/// The writer's refusal of `edits` as a file verb reports it, checked as `blend_driven`: its
/// code, exit 2, a message naming `channel`, and `object show` as the next step. `Err` when
/// it is written, or refused otherwise.
pub fn driven_refusal(row: &str, at: &str, bytes: &[u8], edits: &[Edit], channel: &str) -> Row {
    let e = match write(bytes, edits, &limits()) {
        Ok(_) => {
            return Err(format!(
                "{row}, {at}: written, want refused blend_driven naming {channel}"
            ))
        }
        Err(e) => VerbError::from(e),
    };
    refusal_is_driven(row, at, &e, channel)
}

/// `e` is `blend_driven`: exit 2, its message naming `channel`, its report the entry's with
/// `object show` next.
pub fn refusal_is_driven(row: &str, at: &str, e: &VerbError, channel: &str) -> Row {
    let r = e.report("object place");
    step(
        row,
        at,
        e.code() == Some("blend_driven")
            && e.exit() == 2
            && r.exit() == 2
            && e.to_string().contains(channel)
            && r.message().contains(channel)
            && r.detail("channel").and_then(Json::as_str).is_some_and(|c| c.starts_with(channel))
            && e.entry().and_then(|x| x.step).map(|s| s.verb) == Some("object show"),
        format_args!(
            "refused {:?}, exit {}: {e}; want blend_driven, exit 2, naming {channel}, next object show",
            e.code(),
            e.exit()
        ),
    )
}

/// `edits` written, and the object's `field` reading back as `want`.
pub fn written(
    row: &str,
    at: &str,
    bytes: &[u8],
    edits: &[Edit],
    key: &str,
    field: &str,
    want: &str,
) -> Row {
    let w = write(bytes, edits, &limits()).map_err(|e| format!("{row}, {at}: refused: {e}"))?;
    let got = field_of(&w.bytes, key, field).map_err(|e| format!("{row}, {at}: {e}"))?;
    step(
        row,
        at,
        got == want,
        format_args!("{key} {field} reads back {got}, want {want}"),
    )
}

fn loc(key: &str, v: [f32; 3]) -> Edit {
    Edit::new(key, Change::Location(v))
}

fn rot(key: &str, v: [f32; 3]) -> Edit {
    Edit::new(key, Change::Rotation(v))
}

fn scale(key: &str, v: [f32; 3]) -> Edit {
    Edit::new(key, Change::Scale(v))
}

/// The visibility bits of `key`: `Some(true)` hides (sets the bit), `None` leaves it.
fn vis(key: &str, viewport: Option<bool>, select: Option<bool>, render: Option<bool>) -> Edit {
    Edit::new(
        key,
        Change::Visibility {
            viewport,
            select,
            render,
        },
    )
}

/// The live scene of `bytes`, and the object named `name`'s `edit`.
pub fn live_edit(bytes: &[u8], name: &str) -> Result<EditMode, String> {
    let s: LiveScene =
        model::read(bytes, &LiveLimits::default()).map_err(|d| format!("declined: {d}"))?;
    s.objs
        .iter()
        .find(|o| o.name == name)
        .map(|o| o.edit)
        .ok_or_else(|| format!("no live object {name}"))
}

/// An object placed at `loc`, rotating by Euler XYZ, driven by `driven`.
fn euler(loc: [f32; 3], driven: Driven) -> ObjectSpec {
    ObjectSpec {
        rotmode: 1,
        loc,
        driven,
        ..ObjectSpec::default()
    }
}

/// Objects driven channel by channel, at 4.03: `Steered` (a driver of its Euler rotation's y),
/// `Tracked` (an NLA strip of `AC:Walk`, which animates scale x), `Lost` (animation data naming
/// NLA tracks the file doesn't hold), `Idle` (animation data holding nothing) and `Plain`.
pub fn channels() -> Vec<u8> {
    let mut b = SceneBuilder::animated(403);
    b.scene(
        "Scene",
        &["Steered", "Tracked", "Lost", "Idle", "Plain"],
        &[],
        None,
    );
    b.object(
        "Steered",
        &euler([1.0, 0.0, 0.0], Driven::Driver(("rotation_euler", 1))),
    );
    b.object("Tracked", &euler([2.0, 0.0, 0.0], Driven::Nla));
    b.object("Lost", &euler([3.0, 0.0, 0.0], Driven::Dangling));
    b.object("Idle", &euler([4.0, 0.0, 0.0], Driven::EmptyAnimData));
    b.object("Plain", &euler([5.0, 0.0, 0.0], Driven::None));
    b.action_with(&[(1, &[("scale", 0)])]);
    b.bytes()
}

/// One action of three slots at 5.01, its F-curves in channel bags alone (its `curves` list
/// empty): slot 1 animates location z, slot 2 location x, slot 3 a non-transform channel
/// (`color` red). `Slotted` names slot 1, `Other` slot 2, `Tinted` slot 3.
pub fn slotted() -> Vec<u8> {
    let mut b = SceneBuilder::meshes(501, MeshLayouts::LAYERS);
    b.curves_in_list = false;
    b.scene("Scene", &["Slotted", "Other", "Tinted"], &[], None);
    for (name, slot, x) in [("Slotted", 1, 1.0), ("Other", 2, 2.0), ("Tinted", 3, 3.0)] {
        b.object(
            name,
            &ObjectSpec {
                slot,
                ..euler([x, 0.0, 0.0], Driven::Action)
            },
        );
    }
    b.action_with(&[
        (1, &[("location", 2)]),
        (2, &[("location", 0)]),
        (3, &[("color", 0)]),
    ]);
    b.bytes()
}

/// An object `Tinted` whose action animates a non-transform channel only (`color` red), at
/// 4.03, beside `Plain`.
pub fn tinted() -> Vec<u8> {
    let mut b = SceneBuilder::animated(403);
    b.scene("Scene", &["Tinted", "Plain"], &[], None);
    b.object("Tinted", &euler([1.0, 0.0, 0.0], Driven::Action));
    b.object("Plain", &euler([2.0, 0.0, 0.0], Driven::None));
    b.action_with(&[(1, &[("color", 0)])]);
    b.bytes()
}

const DRIVEN: &str = "@ob:Driven";

/// `driven_quat.blend`: `Driven` stands at (-3, 0, 0), its location's z animated by an action.
/// A patch writing z is refused, typed, with nothing written; so is one list with an undriven
/// change before it (applied whole or refused).
pub fn row_driven_z_refused() -> Row {
    const ROW: &str = "row_driven_z_refused";
    let b = scenes::driven_quat();
    driven_refusal(
        ROW,
        "z written",
        &b,
        &[loc(DRIVEN, [-3.0, 2.0, 1.0])],
        "location[2]",
    )?;
    driven_refusal(
        ROW,
        "z alone",
        &b,
        &[loc(DRIVEN, [-3.0, 0.0, 1.0])],
        "location[2]",
    )?;
    driven_refusal(
        ROW,
        "after an undriven edit",
        &b,
        &[
            loc("@ob:Lamp,%20B:%202", [3.0, 1.0, 0.0]),
            loc(DRIVEN, [-3.0, 2.0, 1.0]),
        ],
        "location[2]",
    )
}

/// `driven_quat.blend`: a patch that writes `Driven`'s x and y, keeping z as stored, is written
/// and reads back; so are its rotation and scale (nothing animates them), and the other
/// objects of the file.
pub fn row_undriven_written() -> Row {
    const ROW: &str = "row_undriven_written";
    let b = scenes::driven_quat();
    written(
        ROW,
        "x and y",
        &b,
        &[loc(DRIVEN, [-3.0, 2.0, 0.0])],
        DRIVEN,
        "loc",
        "-3,2,0",
    )?;
    written(
        ROW,
        "x and y, z restated",
        &b,
        &[loc(DRIVEN, [4.0, 5.0, 0.0])],
        DRIVEN,
        "loc",
        "4,5,0",
    )?;
    written(
        ROW,
        "rotation",
        &b,
        &[rot(DRIVEN, [0.0, 0.0, 0.5])],
        DRIVEN,
        "rot",
        "0,0,0.5",
    )?;
    written(
        ROW,
        "scale",
        &b,
        &[scale(DRIVEN, [2.0, 2.0, 2.0])],
        DRIVEN,
        "size",
        "2,2,2",
    )?;
    written(
        ROW,
        "another object",
        &b,
        &[loc("@ob:Lamp,%20B:%202", [3.0, 1.0, 0.5])],
        "@ob:Lamp,%20B:%202",
        "loc",
        "3,1,0.5",
    )?;
    written(
        ROW,
        "a quaternion object",
        &b,
        &[loc("@ob:Turned", [1.0, 2.0, 0.5])],
        "@ob:Turned",
        "loc",
        "1,2,0.5",
    )
}

/// A driver of `Steered`'s Euler rotation y: a rotation writing y is refused; one writing x
/// alone, and its location, are written.
pub fn row_driver_drives_its_channel() -> Row {
    const ROW: &str = "row_driver_drives_its_channel";
    let b = channels();
    driven_refusal(
        ROW,
        "rotation y",
        &b,
        &[rot("@ob:Steered", [0.0, 0.5, 0.0])],
        "rotation_euler[1]",
    )?;
    written(
        ROW,
        "rotation x",
        &b,
        &[rot("@ob:Steered", [0.5, 0.0, 0.0])],
        "@ob:Steered",
        "rot",
        "0.5,0,0",
    )?;
    written(
        ROW,
        "location",
        &b,
        &[loc("@ob:Steered", [1.0, 2.0, 3.0])],
        "@ob:Steered",
        "loc",
        "1,2,3",
    )
}

/// An NLA strip of `AC:Walk` (scale x animated) on `Tracked`: a scale writing x is refused;
/// one writing y alone, and its location, are written.
pub fn row_nla_strip_drives_its_channel() -> Row {
    const ROW: &str = "row_nla_strip_drives_its_channel";
    let b = channels();
    driven_refusal(
        ROW,
        "scale x",
        &b,
        &[scale("@ob:Tracked", [2.0, 1.0, 1.0])],
        "scale[0]",
    )?;
    written(
        ROW,
        "scale y",
        &b,
        &[scale("@ob:Tracked", [1.0, 2.0, 1.0])],
        "@ob:Tracked",
        "size",
        "1,2,1",
    )?;
    written(
        ROW,
        "location",
        &b,
        &[loc("@ob:Tracked", [2.0, 2.0, 2.0])],
        "@ob:Tracked",
        "loc",
        "2,2,2",
    )
}

/// Animation data the reader can't follow (`Lost`: NLA tracks the file doesn't hold): every
/// transform change is refused, typed, and so is a visibility bit flipped (the reading can't
/// tell it isn't animated); a visibility change that keeps every bit as stored is written.
/// Animation data holding nothing (`Idle`) and none (`Plain`): written.
pub fn row_unfollowed_animation_drives_all() -> Row {
    const ROW: &str = "row_unfollowed_animation_drives_all";
    let b = channels();
    for (at, e, ch) in [
        ("location", loc("@ob:Lost", [3.0, 0.0, 1.0]), "location"),
        (
            "rotation",
            rot("@ob:Lost", [0.1, 0.0, 0.0]),
            "rotation_euler",
        ),
        ("scale", scale("@ob:Lost", [1.0, 1.0, 2.0]), "scale"),
    ] {
        driven_refusal(ROW, at, &b, &[e], ch)?;
    }
    driven_refusal(
        ROW,
        "visibility flipped",
        &b,
        &[vis("@ob:Lost", Some(true), None, None)],
        "hide_viewport",
    )?;
    written(
        ROW,
        "visibility kept",
        &b,
        &[vis("@ob:Lost", Some(false), Some(false), Some(false))],
        "@ob:Lost",
        "restrictflag",
        "0",
    )?;
    for key in ["@ob:Idle", "@ob:Plain"] {
        written(
            ROW,
            "undriven",
            &b,
            &[loc(key, [9.0, 9.0, 9.0])],
            key,
            "loc",
            "9,9,9",
        )?;
    }
    Ok(())
}

/// One action, three slots, its F-curves in channel bags alone (5.01): each object's own slot
/// drives it. `Slotted` (slot 1): z refused, x written; `Other` (slot 2): x refused, z written;
/// `Tinted` (slot 3, a non-transform channel): its location written.
pub fn row_each_slot_drives_its_own() -> Row {
    const ROW: &str = "row_each_slot_drives_its_own";
    let b = slotted();
    driven_refusal(
        ROW,
        "slot 1 z",
        &b,
        &[loc("@ob:Slotted", [1.0, 0.0, 1.0])],
        "location[2]",
    )?;
    driven_refusal(
        ROW,
        "slot 2 x",
        &b,
        &[loc("@ob:Other", [5.0, 0.0, 0.0])],
        "location[0]",
    )?;
    written(
        ROW,
        "slot 1 x",
        &b,
        &[loc("@ob:Slotted", [7.0, 0.0, 0.0])],
        "@ob:Slotted",
        "loc",
        "7,0,0",
    )?;
    written(
        ROW,
        "slot 2 z",
        &b,
        &[loc("@ob:Other", [2.0, 0.0, 7.0])],
        "@ob:Other",
        "loc",
        "2,0,7",
    )?;
    written(
        ROW,
        "slot 3",
        &b,
        &[loc("@ob:Tinted", [3.0, 3.0, 3.0])],
        "@ob:Tinted",
        "loc",
        "3,3,3",
    )
}

/// The file verbs (`object place`, [`apply`]) on `driven_quat.blend`: `--loc -3,2,1` refused
/// `blend_driven`, its report naming the channel; `--loc -3,2,0` written.
pub fn row_file_verb_refuses() -> Row {
    const ROW: &str = "row_file_verb_refuses";
    let b = scenes::driven_quat();
    let place = |v: [f32; 3]| Verb::Place {
        object: DRIVEN.into(),
        loc: Some(v),
        rot: None,
        scale: None,
    };
    match apply(&b, &place([-3.0, 2.0, 1.0]), &limits()) {
        Ok(_) => {
            return Err(format!(
                "{ROW}, z: written, want refused blend_driven naming location[2]"
            ))
        }
        Err(e) => refusal_is_driven(ROW, "z", &e, "location[2]")?,
    }
    let w = apply(&b, &place([-3.0, 2.0, 0.0]), &limits())
        .map_err(|e| format!("{ROW}, x and y: refused: {e}"))?;
    let got = field_of(&w.bytes, DRIVEN, "loc").map_err(|e| format!("{ROW}, x and y: {e}"))?;
    step(
        ROW,
        "x and y",
        got == "-3,2,0",
        format_args!("Driven's loc reads back {got}"),
    )
}

/// An action that animates no transform channel (`Tinted`: `color` red) leaves the transform
/// placeable live (`edit` `1`, as `Plain`'s) and written; the live tier marks `a` only where a
/// transform channel is driven (`driven_quat.blend`'s `Driven`).
pub fn row_live_follows_the_channels() -> Row {
    const ROW: &str = "row_live_follows_the_channels";
    let t = tinted();
    for name in ["Tinted", "Plain"] {
        let e = live_edit(&t, name).map_err(|e| format!("{ROW}, {name}: {e}"))?;
        step(
            ROW,
            "no transform channel",
            e == EditMode::Full,
            format_args!("{name}'s edit {}, want 1", e.as_str()),
        )?;
    }
    written(
        ROW,
        "no transform channel written",
        &t,
        &[loc("@ob:Tinted", [1.0, 2.0, 3.0])],
        "@ob:Tinted",
        "loc",
        "1,2,3",
    )?;
    let e = live_edit(&scenes::driven_quat(), "Driven").map_err(|e| format!("{ROW}: {e}"))?;
    step(
        ROW,
        "a transform channel",
        e == EditMode::Driven,
        format_args!("Driven's edit {}, want a", e.as_str()),
    )
}

/// The writer's three transform properties: the change that writes one, and the property's
/// path as an F-curve names it.
const PROPS: [(&str, usize); 3] = [("location", 0), ("rotation_euler", 1), ("scale", 2)];

fn change_of(prop: usize, v: [f32; 3]) -> Change {
    match prop {
        0 => Change::Location(v),
        1 => Change::Rotation(v),
        _ => Change::Scale(v),
    }
}

/// The file, its model, and the shared reading of each object's channels by key.
fn read_all(bytes: &[u8]) -> Result<(BlendFile, SceneModel), String> {
    let (_, raw) = decompress(bytes, &limits()).map_err(|e| e.to_string())?;
    let f = read_blend(&mut &raw[..], &limits()).map_err(|e| e.to_string())?;
    let m = read_scene(&f).map_err(|e| e.to_string())?;
    Ok((f, m))
}

/// **One rule.** On every generated file but the large one, and on the files above: the live
/// tier marks `edit` `a` exactly where the shared reading ([`drives`]) says something drives the
/// transform; on each object the live tier places (`1`, `t`), the writer writes a change to each
/// component of its location and scale (and of its rotation at `1`); and each component the
/// reading says is driven, the writer refuses (`blend_driven`, or the rotation mode first) on an
/// object the live tier marks `a`. The counts printed: objects, writes, refusals.
pub fn row_live_and_writer_agree() -> Row {
    const ROW: &str = "row_live_and_writer_agree";
    let mut files: Vec<(&str, Vec<u8>)> = scenes::ALL
        .iter()
        .filter(|(n, _)| *n != "large.blend")
        .map(|(n, w)| (*n, w()))
        .collect();
    files.extend([
        ("channels", channels()),
        ("slotted", slotted()),
        ("tinted", tinted()),
    ]);
    let (mut objects, mut writes, mut refusals) = (0usize, 0usize, 0usize);
    for (name, bytes) in &files {
        let at = |o: &str| format!("{name} {o}");
        let live = model::read(bytes, &LiveLimits::default())
            .map_err(|d| format!("{ROW}, {name}: declined: {d}"))?;
        let (f, m) = read_all(bytes).map_err(|e| format!("{ROW}, {name}: {e}"))?;
        for o in &live.objs {
            if o.edit == EditMode::Never {
                continue;
            }
            objects += 1;
            let id = m
                .by_key(&o.nid)
                .ok_or_else(|| format!("{ROW}, {}: no such key", at(&o.name)))?;
            let d: Drives = drives(&f, id.block);
            step(
                ROW,
                &at(&o.name),
                d.transform() == (o.edit == EditMode::Driven),
                format_args!("live edit {}, the reading {d:?}", o.edit.as_str()),
            )?;
            let xform = o
                .xform
                .ok_or_else(|| format!("{ROW}, {}: no transform", at(&o.name)))?;
            for (path, prop) in PROPS {
                for i in 0..3 {
                    let mut v: [f32; 3] = xform[3 * prop..3 * prop + 3].try_into().expect("3");
                    v[i] += 1.0;
                    let e = Edit::new(o.nid.clone(), change_of(prop, v));
                    let got = write(bytes, &[e], &limits());
                    let what = format!("{} {path}[{i}]", at(&o.name));
                    if d.driven(path, i).is_some() {
                        refusals += 1;
                        let ok = matches!(got, Err(WriteError::Driven { .. }))
                            || (prop == 1 && matches!(got, Err(WriteError::RotationMode { .. })));
                        step(
                            ROW,
                            &what,
                            ok && o.edit == EditMode::Driven,
                            format_args!(
                                "driven: the writer {:?}, live edit {}",
                                got.as_ref().map(|_| "wrote").map_err(|e| e.to_string()),
                                o.edit.as_str()
                            ),
                        )?;
                    } else if o.edit == EditMode::Full
                        || (o.edit == EditMode::NoRotation && prop != 1)
                    {
                        writes += 1;
                        step(
                            ROW,
                            &what,
                            got.is_ok(),
                            format_args!(
                                "placed live, refused by the writer: {:?}",
                                got.err().map(|e| e.to_string())
                            ),
                        )?;
                    }
                }
            }
        }
    }
    step(
        ROW,
        "counts",
        objects >= 40 && writes >= 200 && refusals >= 10,
        format_args!("{objects} objects, {writes} writes, {refusals} refusals"),
    )?;
    println!("one rule: {objects} objects, {writes} writes, {refusals} refusals");
    Ok(())
}

/// The program's own saves (`tests/builds/<release>/`, the manifest's sha256s checked by
/// `tests/builds.rs`).
fn kept(release: &str, file: &str) -> Result<Vec<u8>, String> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("builds")
        .join(release)
        .join(file);
    std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Each object's channels as the shared reading gives them, by name: `path[index]` words.
fn channels_by_name(f: &BlendFile, m: &SceneModel) -> BTreeMap<String, (Vec<String>, bool)> {
    m.ids
        .iter()
        .filter(|i| i.code == "OB" && i.library.is_none())
        .map(|i| {
            let d = drives(f, i.block);
            let ch = d
                .channels
                .keys()
                .map(|(p, x)| format!("{p}[{x}]"))
                .collect();
            (i.name.clone(), (ch, d.unknown.is_some() || d.constraint))
        })
        .collect()
}

/// `raw` with the pointer or count `field` of the action `AC` block zeroed (a list's `first`
/// and `last`; an array's count), read again.
fn cut(raw: &[u8], fields: &[&str]) -> Result<BlendFile, String> {
    let f = read_blend(&mut &raw[..], &limits()).map_err(|e| e.to_string())?;
    let mut out = raw.to_vec();
    let acs: Vec<_> = f
        .blocks
        .iter()
        .filter(|b| &b.head.code[..2] == b"AC")
        .collect();
    if acs.is_empty() {
        return Err("no action".into());
    }
    for b in acs {
        for name in fields {
            let (at, fd) = f
                .sdna
                .field_path(b.head.sdna, name)
                .ok_or_else(|| format!("the action has no {name}"))?;
            let start = b.head.body_offset as usize + at as usize;
            let len = if fd.is_pointer() {
                usize::from(f.header.pointer_size)
            } else {
                fd.size as usize
            };
            out[start..start + len].fill(0);
        }
    }
    read_blend(&mut &out[..], &limits()).map_err(|e| e.to_string())
}

/// The program's saves of `driven_quat.blend` and `living_room.blend` at 5.2.4 and 5.3.0: the
/// shared reading finds `Driven`'s `location[2]` and the clock hand's `rotation_euler[2]`, each
/// by an action, and nothing on any other object; the same through the action's channel bags
/// alone (its `curves` list cut), through the list alone (the bags' count cut), and nothing
/// with both cut. The live tier marks the two driven (`a`), and no other object.
pub fn row_program_saves_read_per_channel() -> Row {
    const ROW: &str = "row_program_saves_read_per_channel";
    let mut checked = 0;
    for r in ["5.2.4", "5.3.0"] {
        for (file, who, want) in [
            ("driven_quat.blend", "Driven", "location[2]"),
            ("living_room.blend", "Clock hand", "rotation_euler[2]"),
        ] {
            let at = format!("{r} {file}");
            let bytes = kept(r, file).map_err(|e| format!("{ROW}, {at}: {e}"))?;
            let (_, raw) =
                decompress(&bytes, &limits()).map_err(|e| format!("{ROW}, {at}: {e}"))?;
            let (f, m) = read_all(&bytes).map_err(|e| format!("{ROW}, {at}: {e}"))?;
            let all = channels_by_name(&f, &m);
            for (name, (ch, other)) in &all {
                let ok = if name == who {
                    ch == &[want.to_owned()] && !other
                } else {
                    ch.is_empty() && !other
                };
                step(ROW, &at, ok, format_args!("{name}: {ch:?}, other {other}"))?;
                checked += 1;
            }
            let by = |f: &BlendFile| -> Result<Vec<String>, String> {
                let id = m.ids.iter().find(|i| i.name == who && i.code == "OB");
                let id = id.ok_or_else(|| format!("no {who}"))?;
                let d = drives(f, id.block);
                if d.unknown.is_some() {
                    return Err(format!("{who}: {:?}", d.unknown));
                }
                Ok(d.channels
                    .iter()
                    .map(|((p, x), b)| format!("{p}[{x}] by {b}"))
                    .collect())
            };
            let one = vec![format!("{want} by an action")];
            for (how, fields, expect) in [
                (
                    "bags alone",
                    &["curves.first", "curves.last"][..],
                    one.clone(),
                ),
                (
                    "list alone",
                    &["strip_keyframe_data_array_num"][..],
                    one.clone(),
                ),
                (
                    "neither",
                    &[
                        "curves.first",
                        "curves.last",
                        "strip_keyframe_data_array_num",
                    ][..],
                    Vec::new(),
                ),
            ] {
                let cf = cut(&raw, fields).map_err(|e| format!("{ROW}, {at} {how}: {e}"))?;
                let got = by(&cf).map_err(|e| format!("{ROW}, {at} {how}: {e}"))?;
                step(
                    ROW,
                    &format!("{at} {how}"),
                    got == expect,
                    format_args!("{who}: {got:?}, want {expect:?}"),
                )?;
            }
            let live = model::read(&bytes, &LiveLimits::default())
                .map_err(|d| format!("{ROW}, {at}: declined: {d}"))?;
            for o in &live.objs {
                if o.edit == EditMode::Never {
                    continue;
                }
                step(
                    ROW,
                    &format!("{at} live"),
                    (o.edit == EditMode::Driven) == (o.name == who),
                    format_args!("{}: edit {}", o.name, o.edit.as_str()),
                )?;
            }
        }
    }
    step(
        ROW,
        "counts",
        checked >= 40,
        format_args!("{checked} objects"),
    )?;
    println!("the program's saves: {checked} objects read per channel");
    Ok(())
}

// ---------- a visibility bit an action or a driver animates ----------

/// Objects whose visibility bits are animated, at 4.03: `Blinking` (the action `AC:Walk`, which
/// animates `hide_viewport`), `Strip` (an NLA strip of `AC:Walk`, its viewport bit stored set),
/// `Ghost` (a driver of `hide_render`) and `Plain`.
pub fn blinking() -> Vec<u8> {
    let mut b = SceneBuilder::animated(403);
    b.scene("Scene", &["Blinking", "Strip", "Ghost", "Plain"], &[], None);
    b.object("Blinking", &euler([1.0, 0.0, 0.0], Driven::Action));
    b.object(
        "Strip",
        &ObjectSpec {
            restrict: 1,
            ..euler([2.0, 0.0, 0.0], Driven::Nla)
        },
    );
    b.object(
        "Ghost",
        &euler([3.0, 0.0, 0.0], Driven::Driver(("hide_render", 0))),
    );
    b.object("Plain", &euler([4.0, 0.0, 0.0], Driven::None));
    b.action_with(&[(1, &[("hide_viewport", 0)])]);
    b.bytes()
}

/// One action of two slots at 5.01, its F-curves in channel bags alone: slot 1 animates
/// `hide_select`, slot 2 location x. `Picky` names slot 1, `Mover` slot 2.
pub fn blinking_slots() -> Vec<u8> {
    let mut b = SceneBuilder::meshes(501, MeshLayouts::LAYERS);
    b.curves_in_list = false;
    b.scene("Scene", &["Picky", "Mover"], &[], None);
    for (name, slot, x) in [("Picky", 1, 1.0), ("Mover", 2, 2.0)] {
        b.object(
            name,
            &ObjectSpec {
                slot,
                ..euler([x, 0.0, 0.0], Driven::Action)
            },
        );
    }
    b.action_with(&[(1, &[("hide_select", 0)]), (2, &[("location", 0)])]);
    b.bytes()
}

/// A visibility bit an action, an NLA strip or a driver animates is refused when a change flips
/// it (`blend_driven`, the bit named as its F-curve names it: `hide_viewport`), through the
/// writer and the file verb `object visibility`; a bit kept as stored, and the bits nothing
/// animates, are written beside it. An object whose visibility only is animated places live
/// (`edit` `1`); the live tier's `object visibility` on it is refused
/// (`tests/live.rs`, `live_visibility_refused_on_animated_bits`).
pub fn row_driven_bit_refused() -> Row {
    const ROW: &str = "row_driven_bit_refused";
    let b = blinking();
    driven_refusal(
        ROW,
        "an action's bit",
        &b,
        &[vis("@ob:Blinking", Some(true), None, None)],
        "hide_viewport",
    )?;
    driven_refusal(
        ROW,
        "an NLA strip's bit, cleared",
        &b,
        &[vis("@ob:Strip", Some(false), None, None)],
        "hide_viewport",
    )?;
    driven_refusal(
        ROW,
        "a driver's bit",
        &b,
        &[vis("@ob:Ghost", None, None, Some(true))],
        "hide_render",
    )?;
    driven_refusal(
        ROW,
        "after an undriven edit",
        &b,
        &[
            vis("@ob:Plain", Some(true), None, None),
            vis("@ob:Blinking", Some(true), Some(true), None),
        ],
        "hide_viewport",
    )?;
    written(
        ROW,
        "another bit",
        &b,
        &[vis("@ob:Blinking", None, None, Some(true))],
        "@ob:Blinking",
        "restrictflag",
        "4",
    )?;
    written(
        ROW,
        "the animated bit kept, another flipped",
        &b,
        &[vis("@ob:Blinking", Some(false), Some(true), None)],
        "@ob:Blinking",
        "restrictflag",
        "2",
    )?;
    written(
        ROW,
        "an NLA strip's bit kept set",
        &b,
        &[vis("@ob:Strip", Some(true), None, Some(true))],
        "@ob:Strip",
        "restrictflag",
        "5",
    )?;
    written(
        ROW,
        "a driver's other bit",
        &b,
        &[vis("@ob:Ghost", Some(true), None, None)],
        "@ob:Ghost",
        "restrictflag",
        "1",
    )?;
    written(
        ROW,
        "nothing animated",
        &b,
        &[vis("@ob:Plain", Some(true), Some(true), Some(true))],
        "@ob:Plain",
        "restrictflag",
        "7",
    )?;
    written(
        ROW,
        "a transform beside an animated bit",
        &b,
        &[loc("@ob:Blinking", [5.0, 5.0, 5.0])],
        "@ob:Blinking",
        "loc",
        "5,5,5",
    )?;
    let hide = Verb::Visibility {
        object: "@ob:Blinking".into(),
        viewport: Some(true),
        select: None,
        render: None,
    };
    match apply(&b, &hide, &limits()) {
        Ok(_) => {
            return Err(format!(
                "{ROW}, the file verb: written, want refused blend_driven naming hide_viewport"
            ))
        }
        Err(e) => refusal_is_driven(ROW, "the file verb", &e, "hide_viewport")?,
    }
    let s = blinking_slots();
    driven_refusal(
        ROW,
        "a slot's bit",
        &s,
        &[vis("@ob:Picky", None, Some(true), None)],
        "hide_select",
    )?;
    written(
        ROW,
        "another slot's object",
        &s,
        &[vis("@ob:Mover", None, Some(true), None)],
        "@ob:Mover",
        "restrictflag",
        "2",
    )?;
    for name in ["Blinking", "Strip", "Ghost"] {
        let e = live_edit(&b, name).map_err(|e| format!("{ROW}, live {name}: {e}"))?;
        step(
            ROW,
            "live",
            e == EditMode::Full,
            format_args!("{name}'s edit {}, want 1", e.as_str()),
        )?;
    }
    Ok(())
}

/// The object field `field` of each live object of `bytes` as the pack decodes it, in the
/// objects' order (the live scene's).
fn live_field(bytes: &[u8], field: &str) -> Result<Vec<Option<String>>, String> {
    let c = BlendLive::default()
        .decode(bytes)
        .map_err(|d| format!("declined: {d}"))?
        .roots;
    let root = |name: &str| c.iter().find(|(r, _)| r == name).map(|(_, n)| n);
    let (Some(Neutral::List(objs)), Some(Neutral::Map(props))) = (root(OBJS), root(OBJ_PROPS))
    else {
        return Err("no objects".into());
    };
    Ok(objs
        .iter()
        .map(|(k, _)| {
            props
                .get(&prop(k, field))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect())
}

/// **One rule for the bits.** On every generated file but the large one, and on the files
/// above: the live store marks an object's animated visibility bits (`visanim`: the bits as
/// their F-curves name them, in the order `hide_viewport`, `hide_select`, `hide_render`,
/// comma-separated; no field when none is) exactly where the writer refuses a flip of that bit
/// alone (`blend_driven`), animation the reading can't follow marking all three. The live
/// `object visibility` refuses an object it marks. The counts printed: objects, bits marked.
pub fn row_live_marks_the_animated_bits() -> Row {
    const ROW: &str = "row_live_marks_the_animated_bits";
    let mut files: Vec<(&str, Vec<u8>)> = scenes::ALL
        .iter()
        .filter(|(n, _)| *n != "large.blend")
        .map(|(n, w)| (*n, w()))
        .collect();
    files.extend([
        ("channels", channels()),
        ("slotted", slotted()),
        ("tinted", tinted()),
        ("blinking", blinking()),
        ("blinking_slots", blinking_slots()),
    ]);
    let (mut objects, mut marked, mut all_three) = (0usize, 0usize, 0usize);
    for (name, bytes) in &files {
        let live = model::read(bytes, &LiveLimits::default())
            .map_err(|d| format!("{ROW}, {name}: declined: {d}"))?;
        let stored = live_field(bytes, "visanim").map_err(|e| format!("{ROW}, {name}: {e}"))?;
        for (o, got) in live.objs.iter().zip(&stored) {
            if o.edit == EditMode::Never {
                continue;
            }
            objects += 1;
            let mut refused = Vec::new();
            for (i, (path, _)) in VISIBILITY_PATHS.iter().enumerate() {
                let mut flip = [None; 3];
                flip[i] = Some(!o.hide[i]);
                match write(bytes, &[vis(&o.nid, flip[0], flip[1], flip[2])], &limits()) {
                    Ok(_) => {}
                    Err(WriteError::Driven { .. }) => refused.push(*path),
                    Err(e) => {
                        return Err(format!(
                            "{ROW}, {name} {} {path}: refused otherwise: {e}",
                            o.name
                        ))
                    }
                }
            }
            let want = (!refused.is_empty()).then(|| refused.join(","));
            step(
                ROW,
                &format!("{name} {}", o.name),
                got.as_deref() == want.as_deref(),
                format_args!("visanim {got:?}, the writer refuses {want:?}"),
            )?;
            marked += refused.len();
            all_three += usize::from(refused.len() == 3);
        }
    }
    step(
        ROW,
        "counts",
        objects >= 40 && marked >= 7 && all_three >= 1,
        format_args!("{objects} objects, {marked} bits marked, {all_three} with all three"),
    )?;
    println!("one rule for the bits: {objects} objects, {marked} bits marked, {all_three} with all three");
    Ok(())
}

// ---------- a transform written to an object with constraints ----------

/// Objects with constraints, at 4.03, in a schema that gives each constraint its data
/// (`SceneBuilder::constrained`): `Held` (a track-to, a child-of and a track-to again),
/// `Bare` (one constraint with no data) and `Plain` (none).
pub fn constrained() -> Vec<u8> {
    let mut b = SceneBuilder::constrained(403);
    b.scene("Scene", &["Held", "Bare", "Plain"], &[], None);
    b.object(
        "Held",
        &euler(
            [1.0, 0.0, 0.0],
            Driven::Constraints(&[
                "bTrackToConstraint",
                "bChildOfConstraint",
                "bTrackToConstraint",
            ]),
        ),
    );
    b.object("Bare", &euler([2.0, 0.0, 0.0], Driven::Constraints(&[""])));
    b.object("Plain", &euler([3.0, 0.0, 0.0], Driven::None));
    b.bytes()
}

/// An object with a constraint in a schema whose constraints have no data field (`Pinned`).
pub fn pinned() -> Vec<u8> {
    let mut b = SceneBuilder::animated(403);
    b.scene("Scene", &["Pinned"], &[], None);
    b.object("Pinned", &euler([1.0, 0.0, 0.0], Driven::Constraint));
    b.bytes()
}

/// `w`'s warnings are one `blend_constrained` for `key`, naming `kinds` in order, in its text
/// and its `note: ` line.
pub fn warned(row: &str, at: &str, w: &Written, key: &str, kinds: &[&str]) -> Row {
    let ok = match w.warnings.as_slice() {
        [one @ WriteWarning::Constrained { object, kinds: k }] => {
            let text = one.to_string();
            let note = one.note();
            object == key
                && one.code() == "blend_constrained"
                && one.object() == key
                && k.iter().map(String::as_str).eq(kinds.iter().copied())
                && kinds.iter().all(|x| text.contains(x))
                && note.starts_with("note: ")
                && note.ends_with("(blend_constrained)")
        }
        _ => false,
    };
    step(
        row,
        at,
        ok,
        format_args!(
            "warnings {:?}, want one blend_constrained for {key} naming {kinds:?}",
            w.warnings
        ),
    )
}

/// `edits` written, the object `key`'s `field` reading back as `want`, with one
/// `blend_constrained` naming `kinds` (none when `kinds` is `None`).
#[allow(clippy::too_many_arguments)]
fn written_warned(
    row: &str,
    at: &str,
    bytes: &[u8],
    edits: &[Edit],
    key: &str,
    field: &str,
    want: &str,
    kinds: Option<&[&str]>,
) -> Row {
    let w = write(bytes, edits, &limits()).map_err(|e| format!("{row}, {at}: refused: {e}"))?;
    let got = field_of(&w.bytes, key, field).map_err(|e| format!("{row}, {at}: {e}"))?;
    step(
        row,
        at,
        got == want,
        format_args!("{key} {field} reads back {got}, want {want}"),
    )?;
    match kinds {
        Some(k) => warned(row, at, &w, key, k),
        None => step(
            row,
            at,
            w.warnings.is_empty(),
            format_args!("warnings {:?}, want none", w.warnings),
        ),
    }
}

const TRACK_CHILD: &[&str] = &["bTrackToConstraint", "bChildOfConstraint"];

/// A transform written to an object with constraints is written (its stored values hold) and
/// comes with one warning, `blend_constrained`, naming each kind of constraint once by the
/// file's schema (`bTrackToConstraint`), through the writer and the file verb `object place`;
/// a constraint the reading can't name is `unknown`. Its visibility and name, and every write
/// to an object without constraints, come with none.
pub fn row_constrained_written_with_warning() -> Row {
    const ROW: &str = "row_constrained_written_with_warning";
    let b = constrained();
    let held = "@ob:Held";
    written_warned(
        ROW,
        "location",
        &b,
        &[loc(held, [1.0, 2.0, 3.0])],
        held,
        "loc",
        "1,2,3",
        Some(TRACK_CHILD),
    )?;
    written_warned(
        ROW,
        "rotation",
        &b,
        &[rot(held, [0.0, 0.0, 0.5])],
        held,
        "rot",
        "0,0,0.5",
        Some(TRACK_CHILD),
    )?;
    written_warned(
        ROW,
        "scale",
        &b,
        &[scale(held, [2.0, 2.0, 2.0])],
        held,
        "size",
        "2,2,2",
        Some(TRACK_CHILD),
    )?;
    written_warned(
        ROW,
        "all three and another object, one warning",
        &b,
        &[
            loc(held, [1.0, 2.0, 3.0]),
            rot(held, [0.0, 0.0, 0.5]),
            scale(held, [2.0, 2.0, 2.0]),
            loc("@ob:Plain", [9.0, 9.0, 9.0]),
        ],
        held,
        "loc",
        "1,2,3",
        Some(TRACK_CHILD),
    )?;
    written_warned(
        ROW,
        "visibility",
        &b,
        &[vis(held, Some(true), None, None)],
        held,
        "restrictflag",
        "1",
        None,
    )?;
    written_warned(
        ROW,
        "rename",
        &b,
        &[Edit::new(held, Change::Name("Holder".into()))],
        "@ob:Holder",
        "loc",
        "1,0,0",
        None,
    )?;
    written_warned(
        ROW,
        "no constraint",
        &b,
        &[loc("@ob:Plain", [9.0, 9.0, 9.0])],
        "@ob:Plain",
        "loc",
        "9,9,9",
        None,
    )?;
    written_warned(
        ROW,
        "a constraint with no data",
        &b,
        &[loc("@ob:Bare", [2.0, 2.0, 2.0])],
        "@ob:Bare",
        "loc",
        "2,2,2",
        Some(&["unknown"]),
    )?;
    written_warned(
        ROW,
        "a schema without constraint data",
        &pinned(),
        &[loc("@ob:Pinned", [1.0, 1.0, 1.0])],
        "@ob:Pinned",
        "loc",
        "1,1,1",
        Some(&["unknown"]),
    )?;
    let place = Verb::Place {
        object: held.into(),
        loc: Some([1.0, 2.0, 3.0]),
        rot: None,
        scale: Some([2.0; 3]),
    };
    let w = apply(&b, &place, &limits()).map_err(|e| format!("{ROW}, the file verb: {e}"))?;
    warned(ROW, "the file verb", &w, held, TRACK_CHILD)?;
    let got = field_of(&w.bytes, held, "loc").map_err(|e| format!("{ROW}, the file verb: {e}"))?;
    step(
        ROW,
        "the file verb",
        got == "1,2,3",
        format_args!("Held's loc reads back {got}"),
    )
}
