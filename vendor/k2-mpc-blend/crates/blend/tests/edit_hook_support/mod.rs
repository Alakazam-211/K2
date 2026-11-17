//! The kit's edit check on the live pack (`BlendLive`'s `Pack::check_edit`, the live rules of
//! `live::check_edit`): a live change the write-back can't write is refused at the call,
//! whoever makes it (a person's edit, an undo, a restore), typed: the kit's `pack_refused`
//! (`MPC-E823`, exit 2) carrying the pack's code (`details.code`, a registered entry) and why
//! (`details.reason`); nothing applied, the version and the file as they were, and a whole read
//! and `object show` Ok with no render failure. A change the rules take commits and is written
//! back as the pack's hook writes it. Rows shared by `tests/edit_hook.rs`, `tests/newer.rs`
//! (the person's edit of a driven channel, `newer_support`) and the planted bugs in
//! `tests/mutants.rs`, each returning `Err("<row>: <step>: ...")`, a control step first.
#![allow(dead_code)]

use std::fmt::Debug;

use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Key, ListOp, MapOp, Value};
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::session::{
    ConnId, DiskCheck, EditOptions, Pack, PackRefusal, Session, SessionError, UndoOutcome,
};
use k2_mpc_base::store::{VersionId, VersionReason};
use k2_mpc_blend::live::model::{floats_text, parse_floats};
use k2_mpc_blend::live::{errors, BlendLive, COLL_PROPS, OBJS, OBJ_PROPS};
use k2_mpc_blend::scene::key_of;
use k2_mpc_blend::testgen::{Driven, ObjectSpec, SceneBuilder};

use crate::live_support::{
    self, app_save, blinking_scene, fx, model, model_field, outliner, run, run_based, stdout, Fx,
    Opts,
};

type Row = Result<(), String>;
type Roots = Vec<(String, Neutral)>;

fn step(ok: bool, row: &str, name: &str, why: impl FnOnce() -> String) -> Row {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {name}: {}", why()))
    }
}

/// `got` is the edit check's refusal at the call: `PackRefused` with the pack's `code` (a
/// registered entry), its report `MPC-E823` `pack_refused`, exit 2, `details.code` the code and
/// `details.reason` the message, which names each of `names`.
pub fn refused_at_call<T: Debug>(
    row: &str,
    at: &str,
    got: &Result<T, SessionError>,
    code: &str,
    names: &[&str],
) -> Row {
    let e = match got {
        Ok(v) => return Err(format!("{row}: {at}: taken, want refused {code}: {v:?}")),
        Err(e) => e,
    };
    let SessionError::PackRefused(PackRefusal {
        code: got_code,
        message,
        ..
    }) = e
    else {
        return Err(format!(
            "{row}: {at}: not the pack's refusal at the call: {e:?}"
        ));
    };
    step(
        *got_code == code && errors::by_code(got_code).is_some(),
        row,
        at,
        || format!("the code {got_code}, want {code} (a registered entry)"),
    )?;
    let missing: Vec<&&str> = names.iter().filter(|n| !message.contains(**n)).collect();
    step(missing.is_empty(), row, at, || {
        format!("the message {message:?} doesn't name {missing:?}")
    })?;
    let rep = e.report();
    step(
        rep.id() == "MPC-E823" && rep.entry().name == "pack_refused" && rep.exit() == 2,
        row,
        at,
        || {
            format!(
                "the report {} {} exit {}, want MPC-E823 pack_refused exit 2",
                rep.id(),
                rep.entry().name,
                rep.exit()
            )
        },
    )?;
    let detail = |k: &str| rep.detail(k).and_then(Json::as_str).map(str::to_owned);
    step(
        detail("code").as_deref() == Some(code) && detail("reason").as_deref() == Some(message),
        row,
        at,
        || format!("the details {:?}", rep.details()),
    )
}

/// Nothing moved after a refusal: the version is `v0`, a flush is Ok and the file is `bytes`,
/// a whole read is Ok with no render failure and the content `before`, and `object show` of
/// `show` is Ok (exit 0).
#[allow(clippy::too_many_arguments)]
pub fn unmoved(
    row: &str,
    at: &str,
    f: &Fx,
    s: &Session,
    c: ConnId,
    v0: u64,
    bytes: &[u8],
    before: &[(String, Neutral)],
    show: &str,
) -> Row {
    let now = s.status().map_err(|e| format!("{row}: {at}: {e}"))?.version;
    step(now == v0, row, at, || {
        format!("the version moved: {v0} -> {now}")
    })?;
    let out = s.flush(c);
    step(out.is_ok(), row, at, || format!("the flush: {out:?}"))?;
    step(f.bytes() == bytes, row, at, || "the file changed".into())?;
    let r = s
        .read(c)
        .map_err(|e| format!("{row}: {at}: a whole read fails: {e}"))?;
    step(r.render_failed.is_none(), row, at, || {
        format!("the whole read is flagged: {:?}", r.render_failed)
    })?;
    step(r.content == before, row, at, || {
        "the read's content isn't the doc before the call".into()
    })?;
    let o = run(s, c, &["object", "show", show]);
    step(matches!(&o, Ok(o) if o.exit == 0), row, at, || {
        format!(
            "object show {show}: {:?}",
            o.as_ref().map(|o| (o.exit, stdout(o)))
        )
    })
}

/// The object named `name`: its key and its address.
fn obj(row: &str, roots: &[(String, Neutral)], name: &str) -> Result<(String, String), String> {
    let k = live_support::obj_named(roots, name).ok_or_else(|| format!("{row}: no {name}"))?;
    let addr = format!("@obj:{}", &k[1..]);
    Ok((k, addr))
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

fn props(ops: Vec<MapOp>) -> Edit {
    Edit::Map {
        root: OBJ_PROPS.to_owned(),
        ops,
    }
}

/// `k`'s `xform` with `by` added to component `i`.
fn moved(roots: &[(String, Neutral)], k: &str, i: usize, by: f32) -> Option<String> {
    let mut x = parse_floats::<9>(&field(roots, k, "xform")?)?;
    x[i] += by;
    Some(floats_text(&x))
}

/// `edits` made by `c` on a base of the version now.
fn edit(s: &Session, c: ConnId, edits: Vec<Edit>) -> Result<(), SessionError> {
    let mut opts = EditOptions::new();
    opts.base = Some(s.status()?.version);
    s.edit_ops(c, edits, &opts).map(|_| ())
}

/// `bytes` opened live, a person joined, and the doc as the person reads it.
fn open(
    row: &str,
    tag: &str,
    bytes: &[u8],
) -> Result<(Fx, std::sync::Arc<Session>, ConnId, Roots), String> {
    let f = fx(tag, |_| {});
    f.write(bytes);
    let s = f.try_open().map_err(|e| format!("{row}: opens: {e}"))?;
    let p = f.person(&s);
    let roots = s.read(p).map_err(|e| format!("{row}: reads: {e}"))?.content;
    Ok((f, s, p, roots))
}

/// Each kind of change the live rules refuse, made by a person directly on the live doc, is
/// refused at the call with its code, nothing moving: on the outliner, `Held`'s transform (a
/// constraint holds it: `blend_driven`, channel `xform`), `Spin`'s rotation (it rotates by
/// quaternion: `blend_rotation_mode`), `Far`'s visibility (linked: `blend_not_editable`), an
/// object inserted in the list, a collection's field, and `Cube`'s `data` (a field live v1
/// doesn't write), the last three `blend_live_change`; on the blinking scene, `Blinking`'s
/// render bit (its viewport bit is animated: `blend_driven`, the bit named). Control: each
/// object's `edit` is the one the rule reads.
pub fn row_hook_refuses_each_rule_at_the_call() -> Row {
    const ROW: &str = "row_hook_refuses_each_rule_at_the_call";
    let bytes = outliner(&Opts::default());
    let (f, s, p, roots) = open(ROW, "hook-rules", &bytes)?;
    let o = |n: &str| obj(ROW, &roots, n);
    let ((held, held_at), (spin, spin_at), (far, far_at), (cube, cube_at)) =
        (o("Held")?, o("Spin")?, o("Far")?, o("Cube")?);
    for (k, want) in [(&held, "a"), (&spin, "t"), (&far, "0"), (&cube, "1")] {
        let e = field(&roots, k, "edit");
        step(e.as_deref() == Some(want), ROW, "control", || {
            format!("{k}'s edit {e:?}, want {want}")
        })?;
    }
    let coll = roots
        .iter()
        .find_map(|(r, n)| match n {
            Neutral::Map(m) if r == COLL_PROPS => {
                m.keys().find(|k| k.ends_with("\u{1f}name")).cloned()
            }
            _ => None,
        })
        .ok_or_else(|| format!("{ROW}: control: no collection name"))?;
    let xform = |k: &str, i: usize| moved(&roots, k, i, 1.0).unwrap_or_default();
    let v0 = s.status().map_err(|e| format!("{ROW}: {e}"))?.version;
    let cases: Vec<(&str, Edit, &str, Vec<&str>, &str)> = vec![
        (
            "a constrained transform",
            props(vec![set(&held, "xform", &xform(&held, 6))]),
            errors::DRIVEN.name,
            vec![
                held_at.as_str(),
                "its transform is driven",
                "(channel xform)",
            ],
            held_at.as_str(),
        ),
        (
            "a quaternion rotation",
            props(vec![set(&spin, "xform", &xform(&spin, 3))]),
            errors::ROTATION_MODE.name,
            vec![spin_at.as_str()],
            spin_at.as_str(),
        ),
        (
            "a linked object",
            props(vec![set(&far, "hidden", "1")]),
            errors::NOT_EDITABLE.name,
            vec![far_at.as_str()],
            far_at.as_str(),
        ),
        (
            "a list insert",
            Edit::List {
                root: OBJS.to_owned(),
                ops: vec![ListOp::Insert {
                    key: Key::new("o999").map_err(|e| format!("{ROW}: {e:?}"))?,
                    at: Anchor::End,
                    value: Value::str("empty"),
                }],
            },
            errors::LIVE_CHANGE.name,
            vec![
                "an insert of @obj:999",
                "live v1 adds, removes and moves none",
            ],
            cube_at.as_str(),
        ),
        (
            "a collection's field",
            Edit::Map {
                root: COLL_PROPS.to_owned(),
                ops: vec![MapOp::Set {
                    key: coll.clone(),
                    value: Value::str("Elsewhere"),
                }],
            },
            errors::LIVE_CHANGE.name,
            vec!["a collection's field"],
            cube_at.as_str(),
        ),
        (
            "a field live v1 doesn't write",
            props(vec![set(&cube, "data", "mesh ME:Other")]),
            errors::LIVE_CHANGE.name,
            vec![cube_at.as_str(), "its field data"],
            cube_at.as_str(),
        ),
    ];
    for (at, e, code, names, show) in cases {
        let got = edit(&s, p, vec![e]);
        refused_at_call(ROW, at, &got, code, &names)?;
        unmoved(ROW, at, &f, &s, p, v0, &bytes, &roots, show)?;
    }
    // an animated visibility bit
    let bytes = blinking_scene();
    let (f, s, p, roots) = open(ROW, "hook-blinking", &bytes)?;
    let (blinking, at) = obj(ROW, &roots, "Blinking")?;
    let anim = field(&roots, &blinking, "visanim");
    step(
        anim.as_deref() == Some("hide_viewport"),
        ROW,
        "control: Blinking",
        || format!("Blinking's visanim {anim:?}"),
    )?;
    let v0 = s.status().map_err(|e| format!("{ROW}: {e}"))?.version;
    let got = edit(&s, p, vec![props(vec![set(&blinking, "norender", "1")])]);
    refused_at_call(
        ROW,
        "an animated visibility bit",
        &got,
        errors::DRIVEN.name,
        &[at.as_str(), "hide_viewport"],
    )?;
    unmoved(
        ROW,
        "an animated visibility bit",
        &f,
        &s,
        p,
        v0,
        &bytes,
        &roots,
        &at,
    )
}

/// A scene of two objects at 4.03: `Walker` (an action animates its location z), at `z`, and
/// `Plain` (nothing drives it).
fn walker_scene(z: f32) -> Vec<u8> {
    let mut b = SceneBuilder::animated(403);
    b.scene("Scene", &["Walker", "Plain"], &[], None);
    for (name, loc, driven) in [
        ("Walker", [0.0, 0.0, z], Driven::Action),
        ("Plain", [1.0, 0.0, 0.0], Driven::None),
    ] {
        b.object(
            name,
            &ObjectSpec {
                loc,
                rotmode: 1,
                driven,
                ..ObjectSpec::default()
            },
        );
    }
    b.action();
    b.bytes()
}

/// `save` written by an app over the doc's file and taken in by the session.
fn take_save(row: &str, at: &str, f: &Fx, s: &Session, save: &[u8]) -> Row {
    f.advance(1_000);
    app_save(&f.path(), save);
    let out = s.file_changed().map_err(|e| format!("{row}: {at}: {e}"))?;
    step(matches!(out, DiskCheck::Ingested { .. }), row, at, || {
        format!("the save isn't taken in: {out:?}")
    })
}

/// The id of the doc's import version.
fn import_version(row: &str, s: &Session, c: ConnId) -> Result<VersionId, String> {
    s.versions(c)
        .map_err(|e| format!("{row}: versions: {e}"))?
        .into_iter()
        .find(|v| v.reason == VersionReason::Import)
        .map(|v| v.id)
        .ok_or_else(|| format!("{row}: no import version"))
}

/// The history can't put back what the live rules refuse either: each is refused at the call,
/// typed, nothing moving.
/// - **An undo**: a person renames `Cube` to `Box` (taken), an agent renames `Lamp` to `Cube`
///   (`--break-links`, taken); the person's undo would name the cube `Cube` again, another
///   object's name: `blend_name`.
/// - **A restore past an app's save that moved a driven channel**: the app re-saves with
///   `Walker`'s animated z at 2 (taken in); restoring the import would write z back,
///   `blend_driven`, channel `xform`.
/// - **A restore past an app's save that added an object**: the app's save adds `Added`
///   (taken in); restoring the import would remove it from the list: `blend_live_change`.
pub fn row_hook_refuses_undo_and_restore() -> Row {
    const ROW: &str = "row_hook_refuses_undo_and_restore";
    // the undo
    {
        let (f, s, p, roots) = open(ROW, "hook-undo", &outliner(&Opts::default()))?;
        let a = f.agent(&s);
        let ((cube, cube_at), (_, lamp_at)) =
            (obj(ROW, &roots, "Cube")?, obj(ROW, &roots, "Lamp")?);
        let nid = key_of("OB", b"Box", None).unwrap_or_default();
        let renamed = edit(
            &s,
            p,
            vec![props(vec![
                set(&cube, "name", "Box"),
                set(&cube, "nid", &nid),
            ])],
        );
        step(renamed.is_ok(), ROW, "control: the person's rename", || {
            format!("{renamed:?}")
        })?;
        let argv = [
            "object",
            "rename",
            lamp_at.as_str(),
            "--name",
            "Cube",
            "--break-links",
        ];
        let o = run_based(&s, a, &argv).map_err(|e| format!("{ROW}: {argv:?}: {e}"))?;
        step(o.exit == 0, ROW, "control: the agent's rename", || {
            format!("exit {} {}", o.exit, stdout(&o))
        })?;
        let flushed = s.flush(p);
        step(
            matches!(&flushed, Ok(x) if x.written),
            ROW,
            "control: written",
            || format!("{flushed:?}"),
        )?;
        let (v0, bytes) = (
            s.status().map_err(|e| format!("{ROW}: {e}"))?.version,
            f.bytes(),
        );
        let before = s.read(p).map_err(|e| format!("{ROW}: {e}"))?.content;
        let got = s.undo(p, 1);
        refused_at_call(
            ROW,
            "the undo",
            &got,
            errors::NAME.name,
            &[cube_at.as_str(), "another object of the file has it"],
        )?;
        unmoved(ROW, "the undo", &f, &s, p, v0, &bytes, &before, &cube_at)?;
    }
    // a restore past a driven channel's save
    {
        let (f, s, p, roots) = open(ROW, "hook-restore-driven", &walker_scene(0.0))?;
        let (walker, at) = obj(ROW, &roots, "Walker")?;
        let e = field(&roots, &walker, "edit");
        step(
            e.as_deref() == Some("a"),
            ROW,
            "control: Walker driven",
            || format!("Walker's edit {e:?}"),
        )?;
        let save = walker_scene(2.0);
        take_save(ROW, "control: the app's save", &f, &s, &save)?;
        let import = import_version(ROW, &s, p)?;
        let v0 = s.status().map_err(|e| format!("{ROW}: {e}"))?.version;
        let before = s.read(p).map_err(|e| format!("{ROW}: {e}"))?.content;
        let z = field(&before, &walker, "xform").and_then(|x| parse_floats::<9>(&x));
        step(
            z.is_some_and(|x| x[2] == 2.0),
            ROW,
            "control: the save's z",
            || format!("Walker's xform {z:?}"),
        )?;
        let got = s.restore(p, import);
        refused_at_call(
            ROW,
            "the restore of a driven channel",
            &got,
            errors::DRIVEN.name,
            &[at.as_str(), "(channel xform)"],
        )?;
        unmoved(
            ROW,
            "the restore of a driven channel",
            &f,
            &s,
            p,
            v0,
            &save,
            &before,
            &at,
        )?;
    }
    // a restore past an added object's save
    {
        let (f, s, p, _) = open(ROW, "hook-restore-list", &outliner(&Opts::default()))?;
        let save = outliner(&Opts {
            added_first: true,
            shift: 0x20_0000,
            ..Opts::default()
        });
        take_save(ROW, "control: the app's save", &f, &s, &save)?;
        let import = import_version(ROW, &s, p)?;
        let v0 = s.status().map_err(|e| format!("{ROW}: {e}"))?.version;
        let before = s.read(p).map_err(|e| format!("{ROW}: {e}"))?.content;
        let (_, added_at) = obj(ROW, &before, "Added")?;
        let got = s.restore(p, import);
        refused_at_call(
            ROW,
            "the restore of a list",
            &got,
            errors::LIVE_CHANGE.name,
            &[
                "the live `objs` differ",
                "live v1 adds, removes and moves none",
            ],
        )?;
        unmoved(
            ROW,
            "the restore of a list",
            &f,
            &s,
            p,
            v0,
            &save,
            &before,
            &added_at,
        )?;
    }
    Ok(())
}

/// The doc's roots with `ops` set in the objects' map.
fn with_sets(roots: &[(String, Neutral)], ops: &[MapOp]) -> Roots {
    let mut out = roots.to_vec();
    for (r, n) in &mut out {
        let Neutral::Map(m) = n else { continue };
        if r != OBJ_PROPS {
            continue;
        }
        for op in ops {
            if let MapOp::Set { key, value } = op {
                m.insert(key.clone(), value.clone());
            }
        }
    }
    out
}

/// What the live rules take commits and is written back as the pack's hook writes it: a
/// person's one edit placing `Cube` and renaming `Walker` (a driven object's name is written)
/// commits, and its write-back is byte for byte the hook's on the file with the edit applied
/// (`encode_from_paired`), our reader finding `Cube` at 4,5,6 and `@ob:Strider`; the person's
/// undo of it commits too, and its write-back is the hook's on the file written with the doc
/// as the file first held it, our reader finding `Cube` back at 0,0,0.25 and `@ob:Walker`.
pub fn row_hook_takes_what_live_writes() -> Row {
    const ROW: &str = "row_hook_takes_what_live_writes";
    let bytes = outliner(&Opts::default());
    let (f, s, p, roots) = open(ROW, "hook-takes", &bytes)?;
    let ((cube, _), (walker, _)) = (obj(ROW, &roots, "Cube")?, obj(ROW, &roots, "Walker")?);
    let x = field(&roots, &cube, "xform")
        .and_then(|t| parse_floats::<9>(&t))
        .ok_or_else(|| format!("{ROW}: control: Cube's xform"))?;
    let mut placed = x;
    placed[..3].copy_from_slice(&[4.0, 5.0, 6.0]);
    let nid = key_of("OB", b"Strider", None).unwrap_or_default();
    let ops = vec![
        set(&cube, "xform", &floats_text(&placed)),
        set(&walker, "name", "Strider"),
        set(&walker, "nid", &nid),
    ];
    let pack = BlendLive::default();
    let d = pack
        .decode(&bytes)
        .map_err(|e| format!("{ROW}: control: decode: {e:?}"))?;
    let pairing = pack
        .pairing(&d.roots, &d.codec)
        .map_err(|e| format!("{ROW}: control: {e}"))?;
    let want = pack
        .encode_from_paired(&bytes, &pairing, &with_sets(&d.roots, &ops), &d.codec)
        .map_err(|e| format!("{ROW}: control: the hook: {e:?}"))?;
    let v0 = s.status().map_err(|e| format!("{ROW}: {e}"))?.version;
    let got = edit(&s, p, vec![props(ops)]);
    step(got.is_ok(), ROW, "the edit commits", || format!("{got:?}"))?;
    let v1 = s.status().map_err(|e| format!("{ROW}: {e}"))?.version;
    step(v1 > v0, ROW, "the edit commits", || {
        format!("the version {v0} -> {v1}")
    })?;
    let out = s.flush(p);
    step(matches!(&out, Ok(o) if o.written), ROW, "written", || {
        format!("{out:?}")
    })?;
    let written = f.bytes();
    step(written == want, ROW, "the hook's bytes", || {
        format!("{} bytes written, the hook's {}", written.len(), want.len())
    })?;
    let (_, m) = model(&written);
    let loc = model_field(&m, "@ob:Cube", "loc");
    let strider = model_field(&m, "@ob:Strider", "loc");
    step(
        loc.as_deref() == Some("4,5,6") && strider.is_some(),
        ROW,
        "our reader",
        || format!("@ob:Cube loc {loc:?}, @ob:Strider loc {strider:?}"),
    )?;
    // the undo, taken and written back: the doc as the file first held it
    let undone = s.undo(p, 1);
    step(
        matches!(&undone, Ok(UndoOutcome::Applied { steps: 1, .. })),
        ROW,
        "the undo commits",
        || format!("{undone:?}"),
    )?;
    let want = pack
        .encode_from_paired(&written, &pairing, &d.roots, &d.codec)
        .map_err(|e| format!("{ROW}: the undo: the hook: {e:?}"))?;
    let out = s.flush(p);
    step(
        matches!(&out, Ok(o) if o.written),
        ROW,
        "the undo written",
        || format!("{out:?}"),
    )?;
    let back = f.bytes();
    step(back == want, ROW, "the undo: the hook's bytes", || {
        format!("{} bytes written, the hook's {}", back.len(), want.len())
    })?;
    let (_, m) = model(&back);
    let loc = model_field(&m, "@ob:Cube", "loc");
    let walker_back = model_field(&m, "@ob:Walker", "loc");
    step(
        loc.as_deref() == Some("0,0,0.25")
            && walker_back.is_some()
            && model_field(&m, "@ob:Strider", "loc").is_none(),
        ROW,
        "the undo: our reader",
        || format!("@ob:Cube loc {loc:?}, @ob:Walker loc {walker_back:?}"),
    )
}
