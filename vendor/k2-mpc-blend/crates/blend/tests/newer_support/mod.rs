//! Files of format versions 5.02 and 5.03: the reader names their flags and values, the mesh
//! read draws them, the writer writes them (a place, a visibility flip, a rename; a driven
//! channel refused, a transform under constraints written with its warning), and the live tier
//! opens them writable (each live edit verb applied, its write-back held). A file past 5.03 is
//! declined, and the writer refuses it. A link placeholder (a data-block linked from a library,
//! saved as its bare `ID` header) is linked, never drawn and never written. The read-only state
//! a live doc takes for a release read but not written ([`row_live_read_only`]) is checked in
//! a gap the test-only `mutants` feature opens (`tests/mutants.rs`): no release read today
//! sits past the ones written. Each row returns `Err("<row>: <step>: ...")`, a `control` step
//! first; shared by `tests/newer.rs` and the planted bugs in `tests/mutants.rs`. Every file is
//! generated here (`k2_mpc_blend::testgen`).
#![allow(dead_code)]

use k2_mpc_base::doc::{Edit as DocEdit, Neutral};
use k2_mpc_base::elem::{MapOp, Value};
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::index::{IndexLimits, INDEX_TEXT_MAX};
use k2_mpc_base::session::{EditOptions, Pack, PackError, SpliceError};
use k2_mpc_blend::live::{errors, BlendLive, COLLS, COLL_PROPS, OBJ_PROPS, SCENE};
use k2_mpc_blend::meshread::{mesh_read, MeshReadError, MeshReadLimits};
use k2_mpc_blend::testgen::{CameraSpec, Driven, MeshLayouts, ObjectSpec, SceneBuilder};
use k2_mpc_blend::verbs::{self, Verb};
use k2_mpc_blend::write::decompress;
use k2_mpc_blend::{
    index_blend, read_blend, read_scene, write, Change, Detail, Edit, ReadLimits, SceneModel,
    WriteError,
};

use crate::live_support::{fx, outliner, run, run_based, stdout, Opts};
use crate::meshread_support::{cube_spec, mesh_obj, read_ok};

/// The versions read, named, drawn, written and opened live.
pub const NEWER: [u16; 2] = [502, 503];
/// The control: a version read and written before them.
pub const CONTROL: u16 = 501;
/// A version past the ones read and written.
pub const PAST: u16 = 504;
/// The code of a write the live tier refuses on a file it reads but doesn't write back.
pub const READ_ONLY: &str = "live_read_only";
/// The releases read and written, as the texts name them.
pub const RELEASES: &str = "3.06 to 5.03";

fn step(ok: bool, row: &str, name: &str, why: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {name}: {}", why()))
    }
}

/// `5.03` for 503.
fn release(v: u16) -> String {
    format!("{}.{:02}", v / 100, v % 100)
}

/// A scene at `version` (the attribute store's meshes): `Cube` (a mesh), `Hid` (the same mesh,
/// disabled in viewports), the camera `Cam`, and `Plant`, a link placeholder of an object from
/// the library `plants`, all in the collection `Room`.
pub fn mesh_scene(version: u16) -> Vec<u8> {
    mesh_scene_with(version, &[], false)
}

/// [`mesh_scene`] with more link placeholders from the library `plants`, each `(type code,
/// name)`, in no collection; with `pot`, the file's own mesh object `Pot` in `Room` too, its
/// data the placeholder `ME:Leaf`.
pub fn mesh_scene_with(version: u16, more: &[(&str, &str)], pot: bool) -> Vec<u8> {
    let mut b = SceneBuilder::meshes(version, MeshLayouts::STORE);
    b.file_global(Some("S"));
    b.library("plants", "//plants.blend");
    b.scene("S", &[], &["Room"], Some("Cam"));
    let room: &[&str] = if pot {
        &["Cube", "Hid", "Cam", "Plant", "Pot"]
    } else {
        &["Cube", "Hid", "Cam", "Plant"]
    };
    b.collection("Room", room, &[]);
    b.object("Cube", &mesh_obj("Box"));
    if pot {
        b.object("Pot", &mesh_obj("Leaf"));
    }
    b.object(
        "Hid",
        &ObjectSpec {
            restrict: 1,
            ..mesh_obj("Box")
        },
    );
    b.object(
        "Cam",
        &ObjectSpec {
            data: Some("CA:Cam".into()),
            ty: 11,
            rotmode: 1,
            ..ObjectSpec::default()
        },
    );
    b.camera_with("Cam", &CameraSpec::default());
    b.link_placeholder("OB", "Plant", "plants");
    for (code, name) in more {
        b.link_placeholder(code, name, "plants");
    }
    b.mesh_with("Box", &cube_spec(), MeshLayouts::STORE);
    b.bytes()
}

fn model_of(bytes: &[u8]) -> Result<SceneModel, String> {
    let l = ReadLimits::default();
    let (_, raw) = decompress(bytes, &l).map_err(|e| e.to_string())?;
    let f = read_blend(&mut &raw[..], &l).map_err(|e| e.to_string())?;
    read_scene(&f).map_err(|e| e.to_string())
}

// ---------- the mesh read ----------

/// The mesh read draws a 5.02 and a 5.03 file as it draws a 5.01 one (the control): both mesh
/// objects with 12 triangles each, `Hid` hidden by its own flag, the camera; a file past 5.03 is
/// declined `live_version`.
pub fn row_mesh_read_newer() -> Result<(), String> {
    const ROW: &str = "row_mesh_read_newer";
    for v in [CONTROL, NEWER[0], NEWER[1]] {
        let at = if v == CONTROL {
            "control".to_owned()
        } else {
            format!("read {}", release(v))
        };
        let (r, c) = read_ok(ROW, &mesh_scene(v)).map_err(|e| format!("{e} ({at})"))?;
        let drawn: Vec<(&str, Option<&str>, u64)> = r
            .report
            .drawn
            .iter()
            .map(|d| (d.name.as_str(), d.hidden, d.triangles))
            .collect();
        step(
            drawn == vec![("Cube", None, 12), ("Hid", Some("self"), 12)],
            ROW,
            &at,
            || format!("drawn {drawn:?}"),
        )?;
        let cams: Vec<&str> = c.scene.cameras.iter().map(|c| c.id.as_str()).collect();
        step(cams == vec!["Cam"], ROW, &at, || {
            format!("cameras {cams:?}")
        })?;
    }
    let past = mesh_read(&mesh_scene(PAST), &MeshReadLimits::default());
    step(
        matches!(
            past,
            Err(MeshReadError::Declined {
                code: "live_version"
            })
        ),
        ROW,
        &format!("past {}", release(PAST)),
        || format!("{:?}", past.map(|r| r.report.drawn.len())),
    )
}

// ---------- the names ----------

/// A scene at `version`: `A`, a mesh object rotating by XYZ Euler angles, disabled in viewports
/// and renders (`restrictflag` 5), in `Off`, a collection disabled in viewports.
fn named_scene(version: u16) -> Vec<u8> {
    let mut b = SceneBuilder::new(k2_mpc_blend::HeaderForm::Large, 8, false, version, false);
    b.scene("Scene", &[], &["Off"], None);
    b.collection_flagged("Off", &["A"], &[], 1);
    b.object(
        "A",
        &ObjectSpec {
            data: Some("ME:A".into()),
            ty: 1,
            rotmode: 1,
            restrict: 5,
            ..ObjectSpec::default()
        },
    );
    b.mesh("A", &[], 3);
    b.bytes()
}

/// A data-block's named values and its `hidden`.
type Named = (Vec<(String, String)>, Option<bool>);

fn named(m: &SceneModel, key: &str) -> Result<Named, String> {
    let id = m.by_key(key).ok_or_else(|| format!("no {key}"))?;
    Ok((id.named.clone(), id.hidden))
}

/// The flags and values named at 5.02 and 5.03 as at 5.01 (the control): the object's type,
/// rotation mode and three `restrictflag` bits, the collection's viewport bit; past 5.03 nothing
/// is named or hidden, the numbers kept.
pub fn row_names_newer() -> Result<(), String> {
    const ROW: &str = "row_names_newer";
    let want_obj: Vec<(String, String)> = [
        ("object_type", "mesh"),
        ("rotation_mode", "xyz"),
        ("hide_viewport", "true"),
        ("hide_select", "false"),
        ("hide_render", "true"),
    ]
    .iter()
    .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
    .collect();
    for v in [CONTROL, NEWER[0], NEWER[1]] {
        let at = if v == CONTROL {
            "control".to_owned()
        } else {
            format!("named {}", release(v))
        };
        let m = model_of(&named_scene(v)).map_err(|e| format!("{ROW}: {at}: {e}"))?;
        let (n, hidden) = named(&m, "@ob:A").map_err(|e| format!("{ROW}: {at}: {e}"))?;
        step(n == want_obj && hidden == Some(true), ROW, &at, || {
            format!("@ob:A named {n:?}, hidden {hidden:?}")
        })?;
        let (n, hidden) = named(&m, "@gr:Off").map_err(|e| format!("{ROW}: {at}: {e}"))?;
        step(
            n.iter().any(|(k, x)| k == "hide_viewport" && x == "true") && hidden == Some(true),
            ROW,
            &at,
            || format!("@gr:Off named {n:?}, hidden {hidden:?}"),
        )?;
    }
    let m = model_of(&named_scene(PAST)).map_err(|e| format!("{ROW}: past: {e}"))?;
    step(
        m.ids
            .iter()
            .all(|i| i.named.is_empty() && i.hidden.is_none()),
        ROW,
        &format!("past {}", release(PAST)),
        || "a value named past the versions read".into(),
    )
}

// ---------- the live tier ----------

/// The cube's live key in `roots`.
fn cube_key(roots: &[(String, Neutral)]) -> Option<String> {
    crate::live_support::obj_named(roots, "Cube")
}

fn set_xform(roots: &mut [(String, Neutral)], key: &str, v: &str) {
    for (r, n) in roots.iter_mut() {
        if let Neutral::Map(m) = n {
            if r == OBJ_PROPS {
                m.insert(format!("{key}\u{1f}xform"), Value::str(v));
            }
        }
    }
}

/// A file of a release the live tier reads but the writer doesn't write opens live read-only.
/// No release read today is one (the writer writes every release read): this row runs in the
/// gap the test-only `mutants` feature opens (`tests/mutants.rs`, the writer's range set back to
/// 5.01), where 5.02 and 5.03 are such releases. There every live edit verb is refused
/// `live_read_only` (its registered entry, exit 2, nothing applied); a person's edit is refused
/// at the call by the kit's edit check with the code (the kit's `pack_refused`), the file left
/// as it was; the pack's hook alone gives the carrier back untouched and refuses a change with
/// the code. A file past 5.03 is declined `live_version`, naming the versions read. The
/// control: the same verbs write a 4.03 file.
pub fn row_live_read_only() -> Result<(), String> {
    const ROW: &str = "row_live_read_only";
    // control: the outliner at 4.03 takes the same verb
    {
        let f = fx("newer-control", |_| {});
        f.write(&outliner(&Opts::default()));
        let s = f.open();
        let a = f.agent(&s);
        let roots = s
            .read(a)
            .map_err(|e| format!("{ROW}: control: {e}"))?
            .content;
        let cube = cube_key(&roots).ok_or_else(|| format!("{ROW}: control: no Cube"))?;
        let addr = format!("@obj:{}", &cube[1..]);
        let o = run_based(&s, a, &["object", "place", &addr, "--loc", "1,2,3"])
            .map_err(|e| format!("{ROW}: control: {e}"))?;
        step(o.exit == 0, ROW, "control", || stdout(&o))?;
    }
    let pack = BlendLive::default();
    for v in NEWER {
        let at = |s: &str| format!("{} {s}", release(v));
        let bytes = outliner(&Opts {
            version: v,
            ..Opts::default()
        });
        let d = pack.decode(&bytes);
        let Ok(d) = d else {
            return Err(format!("{ROW}: {}: declined {:?}", at("opens"), d.err()));
        };
        let version = d.roots.iter().find_map(|(r, n)| match n {
            Neutral::Map(m) if r == SCENE => m.get("version").and_then(Value::as_str),
            _ => None,
        });
        step(
            version == Some(release(v).as_str()),
            ROW,
            &at("opens"),
            || format!("the scene map's version {version:?}"),
        )?;
        let f = fx(&format!("newer-{v}"), |_| {});
        f.write(&bytes);
        let s = f
            .try_open()
            .map_err(|e| format!("{ROW}: {}: {e}", at("opens through the kit")))?;
        let a = f.agent(&s);
        let roots = s
            .read(a)
            .map_err(|e| format!("{ROW}: {}: {e}", at("reads")))?
            .content;
        let cube = cube_key(&roots).ok_or_else(|| format!("{ROW}: {}: no Cube", at("reads")))?;
        let addr = format!("@obj:{}", &cube[1..]);
        let list = run(&s, a, &["scene", "list"]).map_err(|e| format!("{ROW}: {e}"))?;
        step(list.exit == 0, ROW, &at("scene list"), || stdout(&list))?;
        let v0 = s.status().map_err(|e| format!("{ROW}: {e}"))?.version;
        let entry = errors::by_code(READ_ONLY);
        step(entry.is_some(), ROW, &at("registered"), || {
            format!("{READ_ONLY} has no entry")
        })?;
        let entry = entry.expect("checked");
        for argv in [
            vec!["object", "place", addr.as_str(), "--loc", "1,2,3"],
            vec!["object", "visibility", addr.as_str(), "--render", "off"],
            vec![
                "object",
                "rename",
                addr.as_str(),
                "--name",
                "Box",
                "--break-links",
            ],
        ] {
            let o = run_based(&s, a, &argv).map_err(|e| format!("{ROW}: {argv:?}: {e}"))?;
            step(
                o.exit == 2 && stdout(&o).starts_with(&format!("refused: {READ_ONLY}: ")),
                ROW,
                &at("a live verb is refused"),
                || format!("{argv:?}: exit {} {}", o.exit, stdout(&o)),
            )?;
            let mut j = argv.clone();
            j.push("--json");
            let o = run_based(&s, a, &j).map_err(|e| format!("{ROW}: {j:?}: {e}"))?;
            let r = o
                .result()
                .ok_or_else(|| format!("{ROW}: {j:?}: no result"))?;
            let error = r.get("error").unwrap_or(&Json::Null);
            step(
                error.get("id").and_then(Json::as_str) == Some(entry.id) && o.exit == entry.exit,
                ROW,
                &at("refused by its entry"),
                || format!("{j:?}: exit {}, error {}", o.exit, error.to_json()),
            )?;
        }
        step(
            s.status().map_err(|e| format!("{ROW}: {e}"))?.version == v0,
            ROW,
            &at("nothing applied"),
            || "a refused verb applied something".into(),
        )?;
        // a person's edit: refused at the call by the kit's edit check, with the code
        let p = f.person(&s);
        let mut opts = EditOptions::new();
        opts.base = Some(v0);
        let edit = DocEdit::Map {
            root: OBJ_PROPS.to_owned(),
            ops: vec![MapOp::Set {
                key: format!("{cube}\u{1f}xform"),
                value: Value::str("4,5,6,0,0,0,1,1,1"),
            }],
        };
        let taken = s.edit_ops(p, vec![edit], &opts);
        crate::edit_hook_support::refused_at_call(
            ROW,
            &at("a person's edit is refused at the call"),
            &taken,
            READ_ONLY,
            &[format!("format {}", release(v)).as_str()],
        )?;
        let out = s.flush(p);
        step(out.is_ok(), ROW, &at("nothing to write back"), || {
            format!("{out:?}")
        })?;
        step(f.bytes() == bytes, ROW, &at("the file as it was"), || {
            "the file changed".into()
        })?;
        // the hook alone
        let pairing = pack
            .pairing(&d.roots, &d.codec)
            .map_err(|e| format!("{ROW}: {e}"))?;
        let same = pack.encode_from_paired(&bytes, &pairing, &d.roots, &d.codec);
        step(
            same.as_deref() == Ok(bytes.as_slice()),
            ROW,
            &at("untouched is the carrier"),
            || format!("{:?}", same.map(|b| b.len())),
        )?;
        let mut changed = d.roots.clone();
        set_xform(&mut changed, &cube, "4,5,6,0,0,0,1,1,1");
        let hook = pack.encode_from_paired(&bytes, &pairing, &changed, &d.codec);
        let typed = matches!(&hook, Err(SpliceError::Pack(PackError(m))) if m.starts_with(&format!("{READ_ONLY}: ")) && m.contains(&format!("format {}", release(v))));
        step(typed, ROW, &at("the hook refuses a change"), || {
            format!("{:?}", hook.map(|b| b.len()))
        })?;
    }
    let past = pack.decode(&outliner(&Opts {
        version: PAST,
        ..Opts::default()
    }));
    let reads = format!("({} to {})", release(306), release(NEWER[1]));
    step(
        matches!(&past, Err(d) if d.code == "live_version" && d.message.contains(&reads)),
        ROW,
        &format!("past {}", release(PAST)),
        || format!("{:?}", past.err()),
    )
}

/// The live address (`@obj:N`) of the object named `name` in `roots`.
fn obj_addr(roots: &[(String, Neutral)], name: &str) -> Option<String> {
    crate::live_support::obj_named(roots, name).map(|k| format!("@obj:{}", &k[1..]))
}

/// A 5.02 and a 5.03 file open live and are written back, as a 4.03 one is (the control): the
/// scene map's `version` names the release; the pack's hook alone gives the carrier back
/// untouched, byte for byte; an agent's `object place`, `object visibility` and `object rename`
/// apply (exit 0) and their write-back is written, our reader finding the place, the render bit
/// and the new name in the file; a person's edit is written back too; a placement of an object
/// an action drives (`Walker`) or a constraint holds (`Held`) is refused `blend_driven`, exit 2,
/// nothing applied. A file past 5.03 is declined `live_version`, naming the versions read.
pub fn row_live_newer_writable() -> Result<(), String> {
    const ROW: &str = "row_live_newer_writable";
    let pack = BlendLive::default();
    for v in [403, NEWER[0], NEWER[1]] {
        let at = |s: &str| {
            if v == 403 {
                format!("control {s}")
            } else {
                format!("{} {s}", release(v))
            }
        };
        let bytes = outliner(&Opts {
            version: v,
            ..Opts::default()
        });
        let d = pack.decode(&bytes);
        let Ok(d) = d else {
            return Err(format!("{ROW}: {}: declined {:?}", at("opens"), d.err()));
        };
        let version = d.roots.iter().find_map(|(r, n)| match n {
            Neutral::Map(m) if r == SCENE => m.get("version").and_then(Value::as_str),
            _ => None,
        });
        step(
            version == Some(release(v).as_str()),
            ROW,
            &at("opens"),
            || format!("the scene map's version {version:?}"),
        )?;
        let pairing = pack
            .pairing(&d.roots, &d.codec)
            .map_err(|e| format!("{ROW}: {e}"))?;
        let same = pack.encode_from_paired(&bytes, &pairing, &d.roots, &d.codec);
        step(
            same.as_deref() == Ok(bytes.as_slice()),
            ROW,
            &at("untouched is the carrier"),
            || format!("{:?}", same.map(|b| b.len())),
        )?;
        let f = fx(&format!("newer-w-{v}"), |_| {});
        f.write(&bytes);
        let s = f
            .try_open()
            .map_err(|e| format!("{ROW}: {}: {e}", at("opens through the kit")))?;
        let a = f.agent(&s);
        let roots = s
            .read(a)
            .map_err(|e| format!("{ROW}: {}: {e}", at("reads")))?
            .content;
        let cube_live =
            cube_key(&roots).ok_or_else(|| format!("{ROW}: {}: no Cube", at("reads")))?;
        let addr_of = |name: &str| {
            obj_addr(&roots, name).ok_or_else(|| format!("{ROW}: {}: no {name}", at("reads")))
        };
        let (cube, walker, held) = (addr_of("Cube")?, addr_of("Walker")?, addr_of("Held")?);
        for argv in [
            vec!["object", "place", cube.as_str(), "--loc", "1,2,3"],
            vec!["object", "visibility", cube.as_str(), "--render", "off"],
            vec![
                "object",
                "rename",
                cube.as_str(),
                "--name",
                "Box",
                "--break-links",
            ],
        ] {
            let o = run_based(&s, a, &argv).map_err(|e| format!("{ROW}: {argv:?}: {e}"))?;
            step(o.exit == 0, ROW, &at("a live verb applies"), || {
                format!("{argv:?}: exit {} {}", o.exit, stdout(&o))
            })?;
        }
        let v0 = s.status().map_err(|e| format!("{ROW}: {e}"))?.version;
        for addr in [&walker, &held] {
            let argv = ["object", "place", addr.as_str(), "--loc", "1,1,1"];
            let o = run_based(&s, a, &argv).map_err(|e| format!("{ROW}: {argv:?}: {e}"))?;
            step(
                o.exit == 2 && stdout(&o).starts_with("refused: blend_driven: "),
                ROW,
                &at("a driven placement is refused"),
                || format!("{argv:?}: exit {} {}", o.exit, stdout(&o)),
            )?;
        }
        step(
            s.status().map_err(|e| format!("{ROW}: {e}"))?.version == v0,
            ROW,
            &at("nothing applied"),
            || "a refused verb applied something".into(),
        )?;
        let out = s.flush(a);
        step(
            matches!(&out, Ok(o) if o.written),
            ROW,
            &at("the write-back is written"),
            || format!("{out:?}"),
        )?;
        let m = model_of(&f.bytes()).map_err(|e| format!("{ROW}: {}: {e}", at("reads back")))?;
        let box_named = m
            .by_key("@ob:Box")
            .map(|i| (i.fields.clone(), i.named.clone()));
        let held_back = box_named.as_ref().is_some_and(|(fields, named)| {
            fields.iter().any(|(n, x)| n == "loc" && x == "1,2,3")
                && named.iter().any(|(n, x)| n == "hide_render" && x == "true")
        }) && m.by_key("@ob:Cube").is_none();
        step(
            held_back,
            ROW,
            &at("our reader finds the agent's edits"),
            || format!("@ob:Box {box_named:?}"),
        )?;
        // a person's edit, written back too
        let p = f.person(&s);
        let mut opts = EditOptions::new();
        opts.base = Some(s.status().map_err(|e| format!("{ROW}: {e}"))?.version);
        let edit = DocEdit::Map {
            root: OBJ_PROPS.to_owned(),
            ops: vec![MapOp::Set {
                key: format!("{cube_live}\u{1f}xform"),
                value: Value::str("4,5,6,0,0,0,1,1,1"),
            }],
        };
        s.edit_ops(p, vec![edit], &opts)
            .map_err(|e| format!("{ROW}: {}: {e}", at("a person's edit")))?;
        let out = s.flush(p);
        step(
            matches!(&out, Ok(o) if o.written),
            ROW,
            &at("a person's write-back is written"),
            || format!("{out:?}"),
        )?;
        let m = model_of(&f.bytes()).map_err(|e| format!("{ROW}: {}: {e}", at("reads back")))?;
        let loc = m.by_key("@ob:Box").and_then(|i| {
            i.fields
                .iter()
                .find(|(n, _)| n == "loc")
                .map(|(_, x)| x.clone())
        });
        step(
            loc.as_deref() == Some("4,5,6"),
            ROW,
            &at("our reader finds the person's edit"),
            || format!("@ob:Box loc {loc:?}"),
        )?;
    }
    let past = pack.decode(&outliner(&Opts {
        version: PAST,
        ..Opts::default()
    }));
    let reads = format!("({RELEASES})");
    step(
        matches!(&past, Err(d) if d.code == "live_version" && d.message.contains(&reads)),
        ROW,
        &format!("past {}", release(PAST)),
        || format!("{:?}", past.err()),
    )
}

/// A person's edit of a driven channel, made on the live doc directly rather than through a
/// verb (which refuses it, `blend_driven`), at 4.03 (the control) and 5.03: `Walker`'s
/// location z, which an action animates. The kit's edit check (`BlendLive`'s
/// `Pack::check_edit`, the live rules of `live::check_edit`) refuses it at the call: the kit's
/// `pack_refused` (`MPC-E823`, exit 2) with the pack's code `blend_driven`, the object and the
/// channel (`xform`) named. Nothing is applied: the version and the file are as they were, and
/// a whole read and `object show` are Ok, with no render failure.
pub fn row_person_edit_of_driven_channel() -> Result<(), String> {
    const ROW: &str = "row_person_edit_of_driven_channel";
    for v in [403, NEWER[1]] {
        let at = |s: &str| {
            if v == 403 {
                format!("control {s}")
            } else {
                format!("{} {s}", release(v))
            }
        };
        let bytes = outliner(&Opts {
            version: v,
            ..Opts::default()
        });
        let f = fx(&format!("newer-person-{v}"), |_| {});
        f.write(&bytes);
        let s = f
            .try_open()
            .map_err(|e| format!("{ROW}: {}: {e}", at("opens")))?;
        let p = f.person(&s);
        let roots = s
            .read(p)
            .map_err(|e| format!("{ROW}: {}: {e}", at("reads")))?
            .content;
        let walker = crate::live_support::obj_named(&roots, "Walker")
            .ok_or_else(|| format!("{ROW}: {}: no Walker", at("reads")))?;
        let edit_mode = crate::live_support::field(&roots, OBJ_PROPS, &walker, "edit");
        step(
            edit_mode.as_deref() == Some("a"),
            ROW,
            &at("driven"),
            || format!("Walker's edit {edit_mode:?}"),
        )?;
        let xform = crate::live_support::field(&roots, OBJ_PROPS, &walker, "xform")
            .and_then(|x| k2_mpc_blend::live::model::parse_floats::<9>(&x))
            .ok_or_else(|| format!("{ROW}: {}: Walker's xform", at("reads")))?;
        let mut moved = xform;
        moved[2] += 1.0;
        let text = k2_mpc_blend::live::model::floats_text(&moved);
        let v0 = s.status().map_err(|e| format!("{ROW}: {e}"))?.version;
        let mut opts = EditOptions::new();
        opts.base = Some(v0);
        let edit = DocEdit::Map {
            root: OBJ_PROPS.to_owned(),
            ops: vec![MapOp::Set {
                key: format!("{walker}\u{1f}xform"),
                value: Value::str(text),
            }],
        };
        let addr = format!("@obj:{}", &walker[1..]);
        let taken = s.edit_ops(p, vec![edit], &opts);
        crate::edit_hook_support::refused_at_call(
            ROW,
            &at("refused at the call"),
            &taken,
            errors::DRIVEN.name,
            &[
                addr.as_str(),
                "its transform is driven (animation, drivers or a constraint)",
                "(channel xform)",
            ],
        )?;
        crate::edit_hook_support::unmoved(
            ROW,
            &at("nothing applied"),
            &f,
            &s,
            p,
            v0,
            &bytes,
            &roots,
            &addr,
        )?;
    }
    Ok(())
}

// ---------- the writer ----------

/// A scene at `version` for the writer, each object rotating by XYZ Euler angles: `Plain`
/// (nothing drives it), `Driven` (an action animates its location's z) and `Held` (a track-to
/// constraint).
fn writer_scene(version: u16) -> Vec<u8> {
    let mut b = SceneBuilder::constrained(version);
    b.scene("Scene", &["Plain", "Driven", "Held"], &[], None);
    let at = |x: f32, driven: Driven| ObjectSpec {
        rotmode: 1,
        loc: [x, 0.0, 0.0],
        driven,
        ..ObjectSpec::default()
    };
    b.object("Plain", &at(1.0, Driven::None));
    b.object("Driven", &at(2.0, Driven::Action));
    b.object(
        "Held",
        &at(3.0, Driven::Constraints(&["bTrackToConstraint"])),
    );
    b.action_with(&[(1, &[("location", 2)])]);
    b.bytes()
}

/// The writer writes a 5.02 and a 5.03 file as it writes a 5.01 one (the control): a place, a
/// render flip and a rename of `Plain` in one list, read back by our reader with no warning; the
/// file verb `object place`; a change to `Driven`'s animated z refused `blend_driven` (exit 2,
/// the channel named), nothing written; a place of `Held` written with one `blend_constrained`
/// naming its constraint. A 5.04 file is refused, typed (`blend_version`, its message naming the
/// format and the versions written, 3.06 to 5.03); `scene meshes` reads 5.02 and 5.03.
pub fn row_writer_newer() -> Result<(), String> {
    const ROW: &str = "row_writer_newer";
    let l = ReadLimits::default();
    let place = Verb::Place {
        object: "@ob:Plain".into(),
        loc: Some([4.0, 5.0, 6.0]),
        rot: None,
        scale: None,
    };
    for v in [CONTROL, NEWER[0], NEWER[1]] {
        let at = |s: &str| {
            if v == CONTROL {
                format!("control {s}")
            } else {
                format!("{} {s}", release(v))
            }
        };
        let b = writer_scene(v);
        let w = write(
            &b,
            &[
                Edit::new("@ob:Plain", Change::Location([1.0, 2.0, 3.0])),
                Edit::new(
                    "@ob:Plain",
                    Change::Visibility {
                        viewport: None,
                        select: None,
                        render: Some(true),
                    },
                ),
                Edit::new("@ob:Plain", Change::Name("Moved".into())),
            ],
            &l,
        );
        let w = w.map_err(|e| format!("{ROW}: {}: refused: {e}", at("the writer writes")))?;
        let m = model_of(&w.bytes).map_err(|e| format!("{ROW}: {}: {e}", at("reads back")))?;
        let moved = m
            .by_key("@ob:Moved")
            .map(|i| (i.fields.clone(), i.named.clone()));
        step(
            moved.as_ref().is_some_and(|(fields, named)| {
                fields.iter().any(|(n, x)| n == "loc" && x == "1,2,3")
                    && named.iter().any(|(n, x)| n == "hide_render" && x == "true")
                    && named
                        .iter()
                        .any(|(n, x)| n == "hide_viewport" && x == "false")
            }) && m.by_key("@ob:Plain").is_none()
                && w.warnings.is_empty(),
            ROW,
            &at("the writer writes"),
            || format!("@ob:Moved {moved:?}, warnings {:?}", w.warnings),
        )?;
        let o = verbs::apply(&b, &place, &l);
        step(o.is_ok(), ROW, &at("the file verb writes"), || {
            format!("{:?}", o.err().map(|e| e.to_string()))
        })?;
        crate::driven_support::driven_refusal(
            ROW,
            &at("a driven channel"),
            &b,
            &[Edit::new("@ob:Driven", Change::Location([2.0, 0.0, 5.0]))],
            "location[2]",
        )?;
        let w = write(
            &b,
            &[Edit::new("@ob:Held", Change::Location([3.0, 1.0, 0.0]))],
            &l,
        )
        .map_err(|e| format!("{ROW}: {}: refused: {e}", at("under a constraint")))?;
        crate::driven_support::warned(
            ROW,
            &at("under a constraint"),
            &w,
            "@ob:Held",
            &["bTrackToConstraint"],
        )?;
        let m = verbs::meshes(&mesh_scene(v), &MeshReadLimits::default());
        step(m.is_ok(), ROW, &at("scene meshes reads"), || {
            format!("{:?}", m.err())
        })?;
    }
    let at = format!("past {}", release(PAST));
    let b = writer_scene(PAST);
    let e = write(
        &b,
        &[Edit::new("@ob:Plain", Change::Location([1.0; 3]))],
        &l,
    );
    step(
        matches!(e, Err(WriteError::Version { version }) if version == PAST),
        ROW,
        &at,
        || format!("{:?}", e.map(|w| w.bytes.len())),
    )?;
    let e = verbs::apply(&b, &place, &l).err();
    let r = e.as_ref().map(|e| e.report("object place"));
    let code = r
        .as_ref()
        .and_then(|r| r.detail("code"))
        .and_then(Json::as_str);
    let message = r
        .as_ref()
        .map(|r| r.message().to_owned())
        .unwrap_or_default();
    step(
        code == Some("blend_version")
            && message.contains(&format!("format {}", release(PAST)))
            && message.contains(&format!("({RELEASES})")),
        ROW,
        &format!("{at}: the file verb refuses typed"),
        || format!("{code:?}: {message}"),
    )
}

// ---------- a link placeholder ----------

/// A link placeholder `Plant` (an object of the library `plants`, saved as its bare `ID`
/// header), at 5.01 (the control) and 5.03: linked in the model (its library `plants`, no
/// object of the file by its name), never drawn or listed as a camera by the mesh read, no
/// object of the live scene, and never written (the writer, which writes both versions, finds
/// no object `@ob:Plant`).
pub fn row_link_placeholder() -> Result<(), String> {
    const ROW: &str = "row_link_placeholder";
    for v in [CONTROL, NEWER[1]] {
        let at = |s: &str| {
            if v == CONTROL {
                format!("control {s}")
            } else {
                format!("{} {s}", release(v))
            }
        };
        let bytes = mesh_scene(v);
        let m = model_of(&bytes).map_err(|e| format!("{ROW}: {}: {e}", at("model")))?;
        let plant: Vec<&k2_mpc_blend::IdBlock> =
            m.ids.iter().filter(|i| i.name == "Plant").collect();
        let lib = plant
            .first()
            .and_then(|p| p.library)
            .map(|l| m.ids[l].name.as_str());
        step(
            plant.len() == 1 && lib == Some("plants"),
            ROW,
            &at("linked"),
            || format!("{} ids named Plant, library {lib:?}", plant.len()),
        )?;
        step(
            !m.ids
                .iter()
                .any(|i| i.kind == "object" && i.library.is_none() && i.name == "Plant"),
            ROW,
            &at("no object of the file"),
            || "Plant is an object of the file".into(),
        )?;
        let r = mesh_read(&bytes, &MeshReadLimits::default())
            .map_err(|e| format!("{ROW}: {}: {e}", at("mesh read")))?;
        step(
            !r.report.drawn.iter().any(|d| d.name == "Plant")
                && !r.report.cameras.iter().any(|d| d.name == "Plant"),
            ROW,
            &at("never drawn"),
            || "Plant is drawn".into(),
        )?;
        let d = BlendLive::default()
            .decode(&bytes)
            .map_err(|e| format!("{ROW}: {}: declined {e}", at("live")))?;
        step(
            crate::live_support::obj_named(&d.roots, "Plant").is_none(),
            ROW,
            &at("no live object"),
            || "Plant is a live object".into(),
        )?;
        let w = write(
            &bytes,
            &[Edit::new("@ob:Plant", Change::Location([1.0; 3]))],
            &ReadLimits::default(),
        );
        step(
            matches!(w, Err(WriteError::NotFound(_))),
            ROW,
            &at("never written"),
            || format!("{:?}", w.map(|w| w.bytes.len())),
        )?;
    }
    Ok(())
}

/// The link placeholders [`row_placeholder_kind`] adds to [`mesh_scene`], beside `Plant` (`OB`):
/// a mesh (`Leaf`, the data of the file's own object `Pot`), a collection, a scene, and a text (a
/// type code no other data-block of the file has).
pub const MORE_PLACEHOLDERS: &[(&str, &str)] = &[
    ("ME", "Leaf"),
    ("GR", "Shelf"),
    ("SC", "Stage"),
    ("TX", "Notes"),
];

/// A link placeholder takes the kind of its stored name's type code, as the file's own
/// data-blocks of that code have it, and stays linked; at 5.01 (the control) and 5.03, with
/// placeholders of an object (`Plant`, in `Room`), a mesh, a collection and a scene:
/// - the model: each the kind of its code (`object`, `mesh`, `collection`, `scene`), struct
///   `ID`, its library `plants`, its key its block code's (`@li:plants/id:Plant`), and nothing
///   of its kind read (no fields, named values, hidden flag, material slots or detail); the text
///   `Notes`, whose code no other data-block has, stays `id`;
/// - the structure index: each node of its kind, flagged `linked`, struct `ID`, no data; `Plant`
///   under `Room`, which holds it;
/// - `scene list`: `Plant` an object, linked, with no data, parent, transform or flags read, in
///   `Room`; no other placeholder listed;
/// - never drawn or listed as a camera by the mesh read, no object or collection of the live
///   scene, and never written (the writer, which writes both versions, refuses its key: linked);
/// - `Pot`, whose data is `Leaf`: its data a mesh in the index (`data_type`), `scene list` and
///   the live scene (`mesh @li:plants/id:Leaf`), and left out of the mesh read as `linked_mesh`.
pub fn row_placeholder_kind() -> Result<(), String> {
    const ROW: &str = "row_placeholder_kind";
    let want = [
        ("Plant", "object"),
        ("Leaf", "mesh"),
        ("Shelf", "collection"),
        ("Stage", "scene"),
        ("Notes", "id"),
    ];
    for v in [CONTROL, NEWER[1]] {
        let at = |s: &str| {
            if v == CONTROL {
                format!("control {s}")
            } else {
                format!("{} {s}", release(v))
            }
        };
        let bytes = mesh_scene_with(v, MORE_PLACEHOLDERS, true);
        // the model
        let m = model_of(&bytes).map_err(|e| format!("{ROW}: {}: {e}", at("model")))?;
        let plants = m
            .ids
            .iter()
            .position(|i| i.kind == "library" && i.name == "plants");
        for (name, kind) in want {
            let hits: Vec<&k2_mpc_blend::IdBlock> =
                m.ids.iter().filter(|i| i.name == name).collect();
            let seen = || {
                hits.iter()
                    .map(|i| {
                        format!(
                            "kind {} struct {} library {:?} key {:?}",
                            i.kind, i.struct_name, i.library, i.key
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            step(
                plants.is_some()
                    && hits.len() == 1
                    && hits[0].kind == kind
                    && hits[0].struct_name == "ID"
                    && hits[0].library == plants,
                ROW,
                &at(&format!("{name} is kind {kind}, linked")),
                seen,
            )?;
            let id = hits[0];
            let key = format!("@li:plants/id:{name}");
            step(
                id.key.as_deref() == Some(key.as_str()),
                ROW,
                &at(&format!("{name}'s key {key}")),
                seen,
            )?;
            step(
                id.detail == Detail::Other
                    && id.fields.is_empty()
                    && id.named.is_empty()
                    && id.hidden.is_none()
                    && id.materials.is_empty(),
                ROW,
                &at(&format!("{name} holds nothing of its kind")),
                || {
                    format!(
                        "detail {:?} fields {:?} named {:?} hidden {:?} materials {:?}",
                        id.detail, id.fields, id.named, id.hidden, id.materials
                    )
                },
            )?;
        }
        // the structure index
        let limits = IndexLimits::new(100_000, 64, 64 << 20, INDEX_TEXT_MAX);
        let d = index_blend(&mut &bytes[..], &limits)
            .map_err(|e| format!("{ROW}: {}: {e:?}", at("index")))?;
        let node_at = |key: &str| d.nodes.iter().position(|n| n.key.as_deref() == Some(key));
        for (name, kind) in want {
            let key = format!("@li:plants/id:{name}");
            let n = node_at(&key).map(|i| &d.nodes[i]);
            step(
                n.is_some_and(|n| {
                    n.kind == kind
                        && n.flags.contains("linked")
                        && n.attrs.get("struct").map(String::as_str) == Some("ID")
                        && !n.attrs.contains_key("data")
                        && !n.attrs.contains_key("data_type")
                }),
                ROW,
                &at(&format!("{key}'s node is kind {kind}, linked")),
                || format!("{:?}", n.map(|n| (&n.kind, &n.flags, &n.attrs))),
            )?;
        }
        let pot = node_at("@ob:Pot").map(|i| &d.nodes[i]);
        step(
            pot.is_some_and(|n| {
                n.attrs.get("data_type").map(String::as_str) == Some("mesh")
                    && n.attrs.get("data").map(String::as_str) == Some("@li:plants/id:Leaf")
            }),
            ROW,
            &at("Pot's data a mesh in the index"),
            || format!("{:?}", pot.map(|n| &n.attrs)),
        )?;
        let plant_parent = node_at("@li:plants/id:Plant").and_then(|i| d.nodes[i].parent);
        step(
            plant_parent.map(|p| p as usize) == node_at("@gr:Room") && plant_parent.is_some(),
            ROW,
            &at("Plant's node under Room"),
            || format!("parent {plant_parent:?}, Room at {:?}", node_at("@gr:Room")),
        )?;
        // scene list
        let list = verbs::list(&bytes, &ReadLimits::default())
            .map_err(|e| format!("{ROW}: {}: {e}", at("scene list")))?;
        let lines = list.lines();
        let line = "@li:plants/id:Plant object \"Plant\" data - parent - in [@gr:Room] loc - rot - (-) scale - shown linked";
        let plant = list.objects.iter().find(|o| o.name == "Plant");
        step(
            lines.iter().any(|l| l == line)
                && plant.is_some_and(|o| {
                    o.linked
                        && o.data.is_none()
                        && o.parent.is_none()
                        && o.location.is_none()
                        && o.hidden == [None; 3]
                }),
            ROW,
            &at("scene list lists Plant, linked"),
            || lines.join(" | "),
        )?;
        step(
            !lines.iter().any(|l| {
                MORE_PLACEHOLDERS
                    .iter()
                    .any(|(_, n)| l.contains(&format!("\"{n}\"")))
            }),
            ROW,
            &at("scene list lists no other placeholder"),
            || lines.join(" | "),
        )?;
        let pot = list.objects.iter().find(|o| o.name == "Pot");
        step(
            pot.is_some_and(|o| {
                !o.linked && o.data == Some(("@li:plants/id:Leaf".to_owned(), "mesh".to_owned()))
            }),
            ROW,
            &at("scene list: Pot's data a mesh"),
            || format!("{pot:?}"),
        )?;
        // never drawn
        let r = mesh_read(&bytes, &MeshReadLimits::default())
            .map_err(|e| format!("{ROW}: {}: {e}", at("mesh read")))?;
        for (name, _) in want {
            step(
                !r.report.drawn.iter().any(|d| d.name == name)
                    && !r.report.cameras.iter().any(|d| d.name == name),
                ROW,
                &at(&format!("{name} never drawn")),
                || format!("{name} is drawn"),
            )?;
        }
        let pot_left: Vec<&str> = r
            .report
            .skipped
            .iter()
            .filter(|s| s.name == "Pot")
            .map(|s| s.reason)
            .collect();
        step(
            pot_left == ["linked_mesh"] && !r.report.drawn.iter().any(|d| d.name == "Pot"),
            ROW,
            &at("Pot left out of the mesh read: linked_mesh"),
            || format!("{pot_left:?}"),
        )?;
        // no live object or collection
        let live = BlendLive::default()
            .decode(&bytes)
            .map_err(|e| format!("{ROW}: {}: declined {e}", at("live")))?;
        let coll_names: Vec<String> = crate::live_support::list(&live.roots, COLLS)
            .into_iter()
            .filter_map(|(k, _)| crate::live_support::field(&live.roots, COLL_PROPS, &k, "name"))
            .collect();
        for (name, _) in want {
            step(
                crate::live_support::obj_named(&live.roots, name).is_none()
                    && !coll_names.iter().any(|c| c == name),
                ROW,
                &at(&format!("{name} no live object or collection")),
                || format!("collections {coll_names:?}"),
            )?;
        }
        let pot_data = crate::live_support::obj_named(&live.roots, "Pot")
            .and_then(|k| crate::live_support::field(&live.roots, OBJ_PROPS, &k, "data"));
        step(
            pot_data.as_deref() == Some("mesh @li:plants/id:Leaf"),
            ROW,
            &at("Pot's live data a mesh"),
            || format!("{pot_data:?}"),
        )?;
        // never written
        let w = write(
            &bytes,
            &[Edit::new("@li:plants/id:Plant", Change::Location([1.0; 3]))],
            &ReadLimits::default(),
        );
        step(
            matches!(w, Err(WriteError::Linked(_))),
            ROW,
            &at("never written"),
            || format!("{:?}", w.map(|w| w.bytes.len())),
        )?;
    }
    Ok(())
}
