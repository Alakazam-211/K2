//! The live rows, each returning
//! `Err("<row>: ...")` so a planted bug can say which row caught it.

use std::sync::mpsc::Receiver;
use std::sync::Arc;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Applied, MapOpKind, Value};
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::placement::{
    Axis, EulerOrder, Handed, Mat4, Placement, Precision, Rotation, Space, Step,
};
use k2_mpc_base::schema::Schema;
use k2_mpc_base::session::{
    test_support::{carrier_judge, CarrierVerdict},
    ChangeEvent, ConnId, Declined, DiskCheck, Pack, PackContent, PackError, Pairing, Reconciled,
    Session, SessionError, SpliceError, View,
};
use k2_mpc_base::session::{WatchFrame, WatchOptions, KEYED_WATCH_FEATURE};
use k2_mpc_base::view_index::{Unit, ViewIndex, ViewSchema, ViewSource};
use k2_mpc_blend::live::{
    errors, model, splice, watch_line, BlendLive, LiveLimits, COLLS, COLL_PROPS, OBJS, OBJ_PROPS,
};
use k2_mpc_blend::testgen::{self, ObjectSpec, SceneBuilder};
use k2_mpc_blend::verbs::{apply, Verb};
use k2_mpc_blend::write::decompress;
use k2_mpc_blend::{Compression, ReadLimits};

use super::{
    app_save, field, fx, list, model as model_of, model_field, obj_named, outliner, placed_scene,
    rig, run, run_based, scene, stdout, turned_scene, units_scene, Fx, Opts, CHILD_INVERSE,
    UNIT_SCALE,
};

fn need(ok: bool, row: &str, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {}", what()))
    }
}

/// The live doc's roots as `c` reads them (a read renders the file: its failure is the row's).
fn read_content(row: &str, s: &Session, c: ConnId) -> Result<Vec<(String, Neutral)>, String> {
    s.read(c)
        .map(|r| r.content)
        .map_err(|e| format!("{row}: the read failed: {e}"))
}

fn open(tag: &str, bytes: &[u8]) -> (Fx, Arc<Session>) {
    let f = fx(tag, |_| {});
    f.write(bytes);
    let s = f.open();
    (f, s)
}

/// An agent's edit verb with a fresh base; exit 0 required.
fn verb(row: &str, s: &Session, c: ConnId, a: &[&str]) -> Result<String, String> {
    let o = run_based(s, c, a).map_err(|e| format!("{row}: {a:?}: {e}"))?;
    need(o.exit == 0, row, || {
        format!("{a:?} exited {}: {}", o.exit, stdout(&o))
    })?;
    Ok(stdout(&o))
}

/// Write back as `c`; the write must happen.
fn flush(row: &str, s: &Session, c: ConnId, what: &str) -> Result<(), String> {
    let out = s
        .flush(c)
        .map_err(|e| format!("{row}: {what}: the write-back failed: {e}"))?;
    need(out.written, row, || format!("{what}: nothing was written"))
}

fn drain(rx: &Receiver<ChangeEvent>) -> Vec<ChangeEvent> {
    rx.try_iter().collect()
}

fn obj(c: &[(String, Neutral)], name: &str, row: &str) -> Result<String, String> {
    obj_named(c, name).ok_or_else(|| format!("{row}: no live object named {name:?}"))
}

/// `loc` of an object of a file, by the model.
fn loc_in(bytes: &[u8], key: &str) -> Option<String> {
    let (_, m) = model_of(bytes);
    model_field(&m, key, "loc")
}

// ---------- the store ----------

/// The outliner scene opens live; the keyed state lists every collection (scenes, nested
/// collections, `c0` last) and object with every field.
pub fn row_opens_live_with_every_field() -> Result<(), String> {
    const ROW: &str = "row_opens_live_with_every_field";
    let (f, s) = open("store", &scene());
    let a = f.agent(&s);
    let c = read_content(ROW, &s, a)?;
    let colls = list(&c, COLLS);
    let want: Vec<(String, String)> = [
        ("c1", "scene"),
        ("c2", "collection"),
        ("c3", "collection"),
        ("c4", "collection"),
        ("c0", "none"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
    .collect();
    need(colls == want, ROW, || format!("colls {colls:?}"))?;
    let objs = list(&c, OBJS);
    let kinds: Vec<&str> = objs.iter().map(|(_, v)| v.as_str()).collect();
    need(
        kinds
            == [
                "camera", "mesh", "empty", "light", "mesh", "mesh", "empty", "empty", "empty",
                "empty",
            ],
        ROW,
        || format!("objs {objs:?}"),
    )?;
    let g = |k: &str, fld: &str| field(&c, OBJ_PROPS, k, fld);
    let cube = obj(&c, "Cube", ROW)?;
    let checks: Vec<(&str, String, Option<&str>)> = vec![
        (
            "Cube coll",
            g(&cube, "coll").unwrap_or_default(),
            Some("c2"),
        ),
        (
            "Cube also",
            g(&cube, "also").unwrap_or_default(),
            Some("c4"),
        ),
        (
            "Cube nid",
            g(&cube, "nid").unwrap_or_default(),
            Some("@ob:Cube"),
        ),
        (
            "Cube data",
            g(&cube, "data").unwrap_or_default(),
            Some("mesh @me:Cube"),
        ),
        (
            "Cube xform",
            g(&cube, "xform").unwrap_or_default(),
            Some("0,0,0.25,0,0,0,1,1,1"),
        ),
        (
            "Cube rotmode",
            g(&cube, "rotmode").unwrap_or_default(),
            Some("xyz"),
        ),
        ("Cube edit", g(&cube, "edit").unwrap_or_default(), Some("1")),
    ];
    for (what, got, want) in checks {
        need(Some(got.as_str()) == want, ROW, || {
            format!("{what}: {got:?}, want {want:?}")
        })?;
    }
    let child = obj(&c, "Child", ROW)?;
    need(
        g(&child, "parent").as_deref() == Some(cube.as_str()),
        ROW,
        || format!("Child's parent {:?}", g(&child, "parent")),
    )?;
    need(g(&child, "coll").as_deref() == Some("c3"), ROW, || {
        "Child is in Inner".into()
    })?;
    let bits = (
        g(&child, "hidden"),
        g(&child, "noselect"),
        g(&child, "norender"),
    );
    need(
        bits == (Some("1".into()), Some("0".into()), Some("1".into())),
        ROW,
        || format!("Child's visibility {bits:?} (restrictflag 5)"),
    )?;
    for (name, edit) in [
        ("Spin", "t"),
        ("Walker", "a"),
        ("Held", "a"),
        ("Idle", "1"),
        ("Far", "0"),
        ("Loose", "1"),
    ] {
        let k = obj(&c, name, ROW)?;
        need(g(&k, "edit").as_deref() == Some(edit), ROW, || {
            format!("{name}'s edit {:?}, want {edit}", g(&k, "edit"))
        })?;
    }
    let spin = obj(&c, "Spin", ROW)?;
    need(g(&spin, "quat").as_deref() == Some("1,0,0,0"), ROW, || {
        format!("Spin's quat {:?}", g(&spin, "quat"))
    })?;
    let far = obj(&c, "Far", ROW)?;
    need(
        g(&far, "nid").as_deref() == Some("@li:lib/ob:Far")
            && g(&far, "coll").as_deref() == Some("c0"),
        ROW,
        || format!("Far {:?} in {:?}", g(&far, "nid"), g(&far, "coll")),
    )?;
    let inner = field(&c, COLL_PROPS, "c3", "parent");
    need(inner.as_deref() == Some("c2"), ROW, || {
        format!("Inner's parent {inner:?}")
    })?;
    let sl = run(&s, a, &["scene", "list"]).map_err(|e| format!("{ROW}: {e}"))?;
    let out = stdout(&sl);
    need(
        sl.exit == 0
            && out.contains("@coll:2\tcollection\tSet\tin @coll:1\t5 objects\tshown")
            && out.contains("@coll:0\tnone\t\tin -\t2 objects"),
        ROW,
        || format!("scene list: {out}"),
    )
}

/// The view index: collections as containers (`c0` too), objects as units in their first
/// collection with their kind, name, visibility and transform text.
pub fn row_view_index_is_the_outliner() -> Result<(), String> {
    const ROW: &str = "row_view_index_is_the_outliner";
    let (f, s) = open("view", &scene());
    let a = f.agent(&s);
    let ix = s
        .view_index(a)
        .map_err(|e| format!("{ROW}: {e}"))?
        .ok_or_else(|| format!("{ROW}: no view index"))?;
    let cs: Vec<(&str, Option<&str>)> = ix
        .containers
        .iter()
        .map(|c| (c.key.as_str(), c.title.as_deref()))
        .collect();
    need(
        cs == [
            ("@coll:1", Some("Scene")),
            ("@coll:2", Some("Set")),
            ("@coll:3", Some("Inner")),
            ("@coll:4", Some("Props")),
            ("@coll:0", None),
        ],
        ROW,
        || format!("containers {cs:?}"),
    )?;
    let cube = ix
        .units
        .iter()
        .find(|u| u.name == "Cube")
        .ok_or_else(|| format!("{ROW}: no Cube unit"))?;
    need(
        cube.container == "@coll:2" && cube.kind == "mesh",
        ROW,
        || format!("Cube's unit {cube:?}"),
    )?;
    need(
        cube.text.is_none()
            && cube.derived_value("transform") == Some("loc 0,0,0.25 rot 0,0,0 scale 1,1,1")
            && cube.derived_value("data") == Some("mesh @me:Cube"),
        ROW,
        || {
            format!(
                "derived: Cube's text {:?} and derived values {:?}: no text; transform and data derived",
                cube.text, cube.derived
            )
        },
    )?;
    let child = ix.units.iter().find(|u| u.name == "Child");
    need(child.is_some_and(|u| u.hidden), ROW, || {
        "Child's unit is hidden".into()
    })?;
    need(ix.units.len() == 10, ROW, || {
        format!("{} units", ix.units.len())
    })
}

// ---------- the verb, its change, the watcher ----------

/// `object place` makes one change event setting `xform` (one field), a watcher prints it, and
/// each of 100 placements reaches the watcher as exactly one event. No timing here (a debug
/// suite times nothing): `tests/live_scale.rs` reports the latency, release, as a figure to watch.
pub fn row_place_one_event() -> Result<(), String> {
    const ROW: &str = "row_place_one_event";
    let (f, s) = open("place", &scene());
    let watcher = f.person(&s);
    let a = f.agent(&s);
    let rx = s.subscribe(watcher).map_err(|e| format!("{ROW}: {e}"))?;
    let cube = obj(&read_content(ROW, &s, a)?, "Cube", ROW)?;
    let addr = format!("@obj:{}", &cube[1..]);
    drain(&rx);
    verb(ROW, &s, a, &["object", "place", &addr, "--loc", "1,2,3.5"])?;
    let ev = drain(&rx);
    need(ev.len() == 1, ROW, || {
        format!("{} change events, not one", ev.len())
    })?;
    let want_key = format!("{cube}\x1fxform");
    let ok = match &ev[0].applied {
        Applied::Map { root, ops } => {
            root == OBJ_PROPS
                && ops.len() == 1
                && ops[0].kind == MapOpKind::Set
                && ops[0].key == want_key
                && ops[0].after == Some(Value::str("1,2,3.5,0,0,0,1,1,1"))
        }
        _ => false,
    };
    need(ok, ROW, || {
        format!("not one xform set: {:?}", ev[0].applied)
    })?;
    let line = watch_line(&ev[0]);
    println!("the watcher prints: {line}");
    need(
        line.contains(&format!("{addr} xform = 1,2,3.5,0,0,0,1,1,1")),
        ROW,
        || format!("the watch line: {line}"),
    )?;
    for i in 0..100 {
        let v = run(&s, a, &["scene", "list", "--limit", "1"])
            .map_err(|e| format!("{ROW}: {e}"))?
            .version;
        let x = format!("{},0,0", i as f32 * 0.25);
        let all = super::args(&[
            "object",
            "place",
            &addr,
            "--loc",
            &x,
            "--base",
            &v.to_string(),
        ]);
        s.run_tool(a, OBJS, &all, b"")
            .map_err(|e| format!("{ROW}: {e}"))?;
        // one event each, and no more (the wait is a liveness bound, not a latency one)
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| format!("{ROW}: placement {i}: no event: {e}"))?;
        let extra = drain(&rx).len();
        need(extra == 0, ROW, || {
            format!("placement {i}: {} change events, not one", 1 + extra)
        })?;
    }
    Ok(())
}

// ---------- untouched is byte for byte ----------

/// The generated corpus: the outliner scene plain, gzip and in zstd frames; another save of it;
/// the newer header form; a scene of 4 collections of 50 objects.
pub fn generated_corpus() -> Vec<(String, Vec<u8>)> {
    let plain = scene();
    let shifted = outliner(&Opts {
        shift: 0x10_0000,
        ..Opts::default()
    });
    vec![
        ("outliner.blend".into(), plain.clone()),
        ("outliner-gzip.blend".into(), testgen::gzip(&plain)),
        (
            "outliner-zstd.blend".into(),
            testgen::zstd_frames(&plain, 1 << 12),
        ),
        ("outliner-saved-again.blend".into(), shifted),
        ("large-header.blend".into(), super::big_scene(2, 3, true)),
        (
            "four-collections.blend".into(),
            super::big_scene(4, 50, false),
        ),
        ("outliner-placed.blend".into(), placed_scene()),
        ("turned.blend".into(), turned_scene()),
    ]
}

/// The kit's byte-exact row (`carrier_judge`) over the generated corpus: the paired hook,
/// the file opened live, and the same after a restart, plain and compressed.
pub fn row_untouched_writes_back_byte_for_byte() -> Result<(), String> {
    const ROW: &str = "row_untouched_writes_back_byte_for_byte";
    let corpus = generated_corpus();
    let refs: Vec<(&str, &[u8])> = corpus
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    match carrier_judge(Arc::new(BlendLive::default()), &refs) {
        Ok(CarrierVerdict::ByteExact { files }) => {
            println!("carrier_judge ByteExact over {files} generated files");
            need(files == corpus.len(), ROW, || {
                format!("{files} files judged")
            })
        }
        other => Err(format!("{ROW}: {other:?}")),
    }
}

// ---------- a placed object patches only its fields ----------

/// A placed object writes back with only its `loc` bytes changed (plain); in zstd frames the
/// file is stored again whole and decompresses to the same patch.
pub fn row_place_patches_only_its_fields() -> Result<(), String> {
    const ROW: &str = "row_place_patches_only_its_fields";
    let plain = scene();
    let (raw0, m0) = model_of(&plain);
    let span = m0
        .by_key("@ob:Cube")
        .and_then(|id| id.spans.iter().find(|s| s.field == "loc"))
        .cloned()
        .ok_or_else(|| format!("{ROW}: control: no loc span"))?;
    let inside = |i: usize| (span.offset as usize..span.offset as usize + 12).contains(&i);
    for (tag, bytes) in [
        ("plain", plain.clone()),
        ("zstd", testgen::zstd_frames(&plain, 1 << 12)),
    ] {
        let (f, s) = open(&format!("patch-{tag}"), &bytes);
        let a = f.agent(&s);
        let cube = obj(&read_content(ROW, &s, a)?, "Cube", ROW)?;
        verb(
            ROW,
            &s,
            a,
            &[
                "object",
                "place",
                &format!("@obj:{}", &cube[1..]),
                "--loc",
                "7,8,9",
            ],
        )?;
        flush(ROW, &s, a, tag)?;
        let out = f.bytes();
        let (how, raw) =
            decompress(&out, &ReadLimits::default()).map_err(|e| format!("{ROW}: {tag}: {e}"))?;
        let want_how = if tag == "plain" {
            Compression::None
        } else {
            Compression::Zstd
        };
        need(how == want_how, ROW, || format!("{tag}: stored as {how:?}"))?;
        if tag == "plain" {
            need(out.len() == plain.len(), ROW, || {
                format!("{tag}: the length changed")
            })?;
        }
        need(raw.len() == raw0.len(), ROW, || {
            format!("{tag}: the raw length changed")
        })?;
        let outside: Vec<usize> = raw
            .iter()
            .zip(&raw0)
            .enumerate()
            .filter(|(i, (x, y))| x != y && !inside(*i))
            .map(|(i, _)| i)
            .take(5)
            .collect();
        need(outside.is_empty(), ROW, || {
            format!("{tag}: bytes outside Cube's loc changed: {outside:?}")
        })?;
        need(
            loc_in(&out, "@ob:Cube").as_deref() == Some("7,8,9"),
            ROW,
            || format!("{tag}: Cube's loc reads {:?}", loc_in(&out, "@ob:Cube")),
        )?;
    }
    Ok(())
}

// ---------- an app's save adds an object before a live-edited one ----------

/// An app's save adds an object first in the file (so every object after it moves in the
/// decode's order); the live keys of the others stay (paired by `nid`), the new one gets a new
/// key; then a placement of the lamp patches the lamp, by the pairing the kit made at the save.
pub fn row_save_adds_before_then_edit_patches_right_object() -> Result<(), String> {
    const ROW: &str = "row_save_adds_before_then_edit_patches_right_object";
    let (f, s) = open("save-adds", &scene());
    let a = f.agent(&s);
    let before = read_content(ROW, &s, a)?;
    let lamp = obj(&before, "Lamp", ROW)?;
    let cube = obj(&before, "Cube", ROW)?;
    f.advance(1_000);
    let save = outliner(&Opts {
        added_first: true,
        shift: 0x20_0000,
        ..Opts::default()
    });
    app_save(&f.path(), &save);
    let out = s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    need(matches!(out, DiskCheck::Ingested { .. }), ROW, || {
        format!("control: the save is taken in: {out:?}")
    })?;
    let after = read_content(ROW, &s, a)?;
    need(
        obj_named(&after, "Lamp").as_deref() == Some(lamp.as_str())
            && obj_named(&after, "Cube").as_deref() == Some(cube.as_str()),
        ROW,
        || "the lamp and the cube keep their keys".into(),
    )?;
    let added = obj(&after, "Added", ROW)?;
    need(added == "o11", ROW, || format!("Added's key {added}"))?;
    verb(
        ROW,
        &s,
        a,
        &[
            "object",
            "place",
            &format!("@obj:{}", &lamp[1..]),
            "--loc",
            "-5,6,-7",
        ],
    )?;
    flush(ROW, &s, a, "the write after the save")?;
    let file = f.bytes();
    let got = (
        loc_in(&file, "@ob:Lamp"),
        loc_in(&file, "@ob:Added"),
        loc_in(&file, "@ob:Cube"),
    );
    need(
        got == (
            Some("-5,6,-7".into()),
            Some("9,9,9".into()),
            Some("0,0,0.25".into()),
        ),
        ROW,
        || format!("the placement landed on the wrong object: lamp, added, cube {got:?}"),
    )
}

/// The same at the pack's hooks (the kit's path is the row above): an app's
/// save adds an object first; `reconcile`'s pairing keeps every other object's key (by `nid`)
/// and gives the new one a new key; the pairing of that content, as the kit makes it at the
/// save, puts the keys on the save's objects; a placement of the lamp is then spliced into the
/// lamp, and only the lamp.
pub fn row_pack_save_adds_before_then_edit_patches_right_object() -> Result<(), String> {
    const ROW: &str = "row_pack_save_adds_before_then_edit_patches_right_object";
    let pack = BlendLive::default();
    let limits = LiveLimits::default();
    let d0 = pack
        .decode(&scene())
        .map_err(|e| format!("{ROW}: control: {e}"))?;
    let synced = View::of_content(&d0.roots);
    let save = outliner(&Opts {
        added_first: true,
        shift: 0x20_0000,
        ..Opts::default()
    });
    let new = model::read(&save, &limits).map_err(|e| format!("{ROW}: control: {e}"))?;
    let (ck, ok, marks) = k2_mpc_blend::live::pair_save(&synced, &new);
    let names: Vec<(&str, &str)> = new
        .objs
        .iter()
        .zip(&ok)
        .map(|(o, k)| (o.name.as_str(), k.as_str()))
        .collect();
    need(
        names.first() == Some(&("Added", "o11")) && names.contains(&("Lamp", "o4")),
        ROW,
        || format!("the save's keys: {names:?}"),
    )?;
    let mut content = new.to_content(&ck, &ok, marks);
    for (r, root) in &mut content {
        if let (true, Neutral::Map(m)) = (r == OBJ_PROPS, root) {
            m.insert("o4\x1fxform".into(), Value::str("-5,6,-7,0,0,0,1,1,1"));
        }
    }
    let pairing = pack
        .pairing(&content, k2_mpc_blend::live::CODEC)
        .map_err(|e| format!("{ROW}: {e}"))?;
    let out = splice::encode_from_paired(&save, &pairing, &content, &limits)
        .map_err(|e| format!("{ROW}: the lamp's placement isn't spliced: {e}"))?;
    let got = (
        loc_in(&out, "@ob:Lamp"),
        loc_in(&out, "@ob:Added"),
        loc_in(&out, "@ob:Cube"),
    );
    need(
        got == (
            Some("-5,6,-7".into()),
            Some("9,9,9".into()),
            Some("0,0,0.25".into()),
        ),
        ROW,
        || format!("the placement landed on the wrong object: lamp, added, cube {got:?}"),
    )
}

// ---------- outside saves keep both ----------

/// An app's save after a kit write (the app had the file open before it) keeps both: the
/// save becomes the live doc, the kit's version a conflict copy.
pub fn row_save_after_kit_write_keeps_both() -> Result<(), String> {
    const ROW: &str = "row_save_after_kit_write_keeps_both";
    let (f, s) = open("after-write", &scene());
    let a = f.agent(&s);
    let cube = obj(&read_content(ROW, &s, a)?, "Cube", ROW)?;
    verb(
        ROW,
        &s,
        a,
        &[
            "object",
            "place",
            &format!("@obj:{}", &cube[1..]),
            "--loc",
            "1,1,1",
        ],
    )?;
    flush(ROW, &s, a, "the kit's write")?;
    f.advance(1_000);
    app_save(
        &f.path(),
        &outliner(&Opts {
            lamp_loc: [0.0, 0.0, 9.0],
            shift: 0x30_0000,
            ..Opts::default()
        }),
    );
    let out = s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    let DiskCheck::Conflict { copy, .. } = &out else {
        return Err(format!("{ROW}: not kept both: {out:?}"));
    };
    let c = read_content(ROW, &s, a)?;
    need(
        field(&c, OBJ_PROPS, &cube, "xform").as_deref() == Some("0,0,0.25,0,0,0,1,1,1"),
        ROW,
        || "the save is the live doc".into(),
    )?;
    let copy = copy
        .clone()
        .ok_or_else(|| format!("{ROW}: no conflict copy"))?;
    let kept = std::fs::read(&copy).map_err(|e| format!("{ROW}: {e}"))?;
    need(
        loc_in(&kept, "@ob:Cube").as_deref() == Some("1,1,1"),
        ROW,
        || "the copy holds the kit's version".into(),
    )
}

/// An app's save while the doc holds changes the file doesn't (not written yet) keeps both
/// too, and the save becomes the live doc exactly: never merged by key.
pub fn row_save_on_unwritten_changes_keeps_both() -> Result<(), String> {
    const ROW: &str = "row_save_on_unwritten_changes_keeps_both";
    let (f, s) = open("unwritten", &scene());
    let a = f.agent(&s);
    let c0 = read_content(ROW, &s, a)?;
    let cube = obj(&c0, "Cube", ROW)?;
    let lamp = obj(&c0, "Lamp", ROW)?;
    verb(
        ROW,
        &s,
        a,
        &[
            "object",
            "place",
            &format!("@obj:{}", &cube[1..]),
            "--loc",
            "2,2,2",
        ],
    )?;
    f.advance(10);
    let save = outliner(&Opts {
        lamp_loc: [0.0, 0.0, 9.0],
        shift: 0x40_0000,
        ..Opts::default()
    });
    app_save(&f.path(), &save);
    let out = s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    need(matches!(out, DiskCheck::Conflict { .. }), ROW, || {
        format!("not kept both: {out:?}")
    })?;
    let c = read_content(ROW, &s, a)?;
    need(
        field(&c, OBJ_PROPS, &cube, "xform").as_deref() == Some("0,0,0.25,0,0,0,1,1,1")
            && field(&c, OBJ_PROPS, &lamp, "xform").as_deref() == Some("0,0,9,0,0,0,1,1,1"),
        ROW,
        || "the save is the live doc exactly".into(),
    )?;
    need(f.bytes() == save, ROW, || {
        "the file is the save, unwritten over".into()
    })
}

// ---------- floats round-trip bit for bit ----------

/// A scene whose one object's transform holds floats that print awkwardly.
pub fn odd_floats() -> (Vec<u8>, [f32; 9]) {
    let x = [
        -0.0f32,
        f32::from_bits(1),
        f32::MAX,
        0.1,
        -2.5e-8,
        std::f32::consts::PI,
        1.000_000_1,
        1.0 / 3.0,
        1e10,
    ];
    let mut b = SceneBuilder::modern();
    b.scene("Scene", &["Odd"], &[], None);
    b.object(
        "Odd",
        &ObjectSpec {
            loc: [x[0], x[1], x[2]],
            rot: [x[3], x[4], x[5]],
            scale: [x[6], x[7], x[8]],
            rotmode: 1,
            ..ObjectSpec::default()
        },
    );
    (b.bytes(), x)
}

/// `xform` holds every float of the file's transform bit for bit: the store's text parses
/// back to the file's bits (negative zero, a subnormal, the largest float, thirds).
pub fn row_xform_round_trips_bits() -> Result<(), String> {
    const ROW: &str = "row_xform_round_trips_bits";
    let (bytes, x) = odd_floats();
    let scene =
        model::read(&bytes, &LiveLimits::default()).map_err(|d| format!("{ROW}: declined: {d}"))?;
    let (ck, ok) = scene.minted_keys();
    let c = scene.to_content(&ck, &ok, (0, 0));
    let text = field(&c, OBJ_PROPS, "o1", "xform").ok_or_else(|| format!("{ROW}: no xform"))?;
    let back: Vec<u32> = text
        .split(',')
        .map(|v| v.parse::<f32>().map(f32::to_bits).unwrap_or(u32::MAX))
        .collect();
    let want: Vec<u32> = x.iter().map(|v| v.to_bits()).collect();
    need(back == want, ROW, || {
        format!("{text:?} reads back as other bits")
    })
}

// ---------- a rename keeps the key through an app's save ----------

/// `object rename` sets `name` and `nid` in one change; written back, then an app's
/// save of the renamed scene that also duplicates the object (two objects with its mesh, so the
/// data rule can't pair it): the object keeps its key, paired by `nid`.
pub fn row_rename_keeps_key_through_app_save() -> Result<(), String> {
    const ROW: &str = "row_rename_keeps_key_through_app_save";
    let (f, s) = open("rename", &scene());
    let a = f.agent(&s);
    let cube = obj(&read_content(ROW, &s, a)?, "Cube", ROW)?;
    let addr = format!("@obj:{}", &cube[1..]);
    let out = verb(
        ROW,
        &s,
        a,
        &["object", "rename", &addr, "--name", "Box", "--break-links"],
    )?;
    // the keyed state, through a structured read (it renders nothing)
    let shown = run(&s, a, &["object", "show", &addr]).map_err(|e| format!("{ROW}: {e}"))?;
    let shown = stdout(&shown);
    need(shown.contains("\nnid\t@ob:Box\n"), ROW, || {
        format!("the rename didn't set nid in its change ({out}): {shown}")
    })?;
    flush(ROW, &s, a, "the rename's write")?;
    let (_, m) = model_of(&f.bytes());
    need(m.by_key("@ob:Box").is_some(), ROW, || {
        "the file holds Box".into()
    })?;
    f.advance(1_000);
    // the app reopens the written file, then saves it with a duplicate
    let save = outliner(&Opts {
        cube: "Box".into(),
        dup: true,
        shift: 0x50_0000,
        ..Opts::default()
    });
    // first the app's own re-save of what the kit wrote (no kit write since): taken in
    app_save(&f.path(), &save);
    let out = s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    need(
        matches!(out, DiskCheck::Ingested { .. } | DiskCheck::Conflict { .. }),
        ROW,
        || format!("control: the save is taken in: {out:?}"),
    )?;
    let c = read_content(ROW, &s, a)?;
    need(
        obj_named(&c, "Box").as_deref() == Some(cube.as_str()),
        ROW,
        || {
            format!(
                "the renamed object lost its key: Box is {:?}, was {cube}",
                obj_named(&c, "Box")
            )
        },
    )
}

// ---------- what is refused ----------

/// `object place` is refused on a driven transform (an action; a constraint), `--rot` on a
/// quaternion rotation, anything on an object linked from a library; the control with empty
/// animation data places.
pub fn row_driven_and_unwritable_refused() -> Result<(), String> {
    const ROW: &str = "row_driven_and_unwritable_refused";
    let (f, s) = open("refused", &scene());
    let a = f.agent(&s);
    let c = read_content(ROW, &s, a)?;
    let at = |n: &str| -> Result<String, String> { Ok(format!("@obj:{}", &obj(&c, n, ROW)?[1..])) };
    let v0 = s.status().map_err(|e| format!("{ROW}: {e}"))?.version;
    for (name, flags, code) in [
        ("Walker", vec!["--loc", "1,0,0"], "blend_driven"),
        ("Held", vec!["--scale", "2,2,2"], "blend_driven"),
        ("Spin", vec!["--rot", "0,0,1"], "blend_rotation_mode"),
        ("Far", vec!["--loc", "1,0,0"], "blend_not_editable"),
    ] {
        let addr = at(name)?;
        let mut argv = vec!["object", "place", addr.as_str()];
        argv.extend(flags);
        let o = run_based(&s, a, &argv).map_err(|e| format!("{ROW}: {e}"))?;
        need(
            o.exit == 2 && stdout(&o).starts_with(&format!("refused: {code}: ")),
            ROW,
            || format!("{name}: exit {} {}", o.exit, stdout(&o)),
        )?;
    }
    need(
        s.status().map_err(|e| format!("{ROW}: {e}"))?.version == v0,
        ROW,
        || "a refusal applied something".into(),
    )?;
    verb(
        ROW,
        &s,
        a,
        &["object", "place", &at("Spin")?, "--loc", "1,2,3"],
    )?;
    verb(
        ROW,
        &s,
        a,
        &["object", "place", &at("Idle")?, "--loc", "1,2,3"],
    )?;
    verb(
        ROW,
        &s,
        a,
        &["object", "visibility", &at("Walker")?, "--render", "off"],
    )?;
    flush(ROW, &s, a, "the allowed edits")?;
    let (_, m) = model_of(&f.bytes());
    need(
        model_field(&m, "@ob:Spin", "loc").as_deref() == Some("1,2,3")
            && model_field(&m, "@ob:Idle", "loc").as_deref() == Some("1,2,3")
            && model_field(&m, "@ob:Walker", "restrictflag").as_deref() == Some("4"),
        ROW,
        || "the allowed edits are written".into(),
    )
}

// ---------- a visibility bit something animates ----------

/// `object visibility` on an object with any animated visibility bit
/// ([`super::blinking_scene`]: an action's, a driver's, animation the reading can't follow) is
/// refused whole, typed: `blend_driven`, the entry's exit, the bits named as their F-curves name
/// them (the message and `details.channel`), `object show` next; a flip of a bit nothing animates
/// on such an object too, so the live doc and the file never disagree on a bit. Nothing
/// applies: the doc is as it was. Accepted, the call is reported with what the write-back makes
/// of it. An object nothing animates, and one whose transform only is animated (`Mover`, by a
/// driver; the outliner scene's `Walker`, by an action), flip theirs and write them back.
/// Untouched, the file writes back byte for byte (the kit's `carrier_judge`).
pub fn row_live_visibility_refused_on_animated_bits() -> Result<(), String> {
    const ROW: &str = "row_live_visibility_refused_on_animated_bits";
    let bytes = super::blinking_scene();
    match carrier_judge(
        Arc::new(BlendLive::default()),
        &[("blinking.blend", bytes.as_slice())],
    ) {
        Ok(CarrierVerdict::ByteExact { files: 1 }) => {}
        other => return Err(format!("{ROW}: untouched: {other:?}")),
    }
    let (f, s) = open("animated-bits", &bytes);
    let a = f.agent(&s);
    let c0 = read_content(ROW, &s, a)?;
    let at =
        |n: &str| -> Result<String, String> { Ok(format!("@obj:{}", &obj(&c0, n, ROW)?[1..])) };
    let v0 = s.status().map_err(|e| format!("{ROW}: {e}"))?.version;
    let entry = errors::by_code("blend_driven")
        .ok_or_else(|| format!("{ROW}: blend_driven has no entry"))?;
    let blinking = at("Blinking")?;
    let o = run_based(
        &s,
        a,
        &["object", "visibility", &blinking, "--viewport", "off"],
    )
    .map_err(|e| format!("{ROW}: {e}"))?;
    let text = stdout(&o);
    if o.exit == 0 {
        let back = match s.flush(a) {
            Ok(w) => format!("written: {}", w.written),
            Err(e) => format!("refused: {e}"),
        };
        return Err(format!(
            "{ROW}: text: Blinking --viewport off accepted live; the write-back: {back}"
        ));
    }
    need(
        o.exit == entry.exit
            && text.starts_with("refused: blend_driven: ")
            && text.contains("hide_viewport")
            && text.contains(&format!("next: object show {blinking}\n")),
        ROW,
        || format!("text: exit {}: {text}", o.exit),
    )?;
    for (name, flags, bits) in [
        ("Blinking", ["--viewport", "off"], "hide_viewport"),
        // a bit nothing animates, on an object with an animated one: refused too
        ("Blinking", ["--render", "off"], "hide_viewport"),
        ("Ghost", ["--render", "off"], "hide_render"),
        ("Ghost", ["--viewport", "off"], "hide_render"),
        (
            "Lost",
            ["--select", "off"],
            "hide_viewport,hide_select,hide_render",
        ),
    ] {
        let addr = at(name)?;
        let mut argv = vec!["object", "visibility", addr.as_str()];
        argv.extend(flags);
        argv.push("--json");
        let o = run_based(&s, a, &argv).map_err(|e| format!("{ROW}: {argv:?}: {e}"))?;
        if o.exit == 0 {
            let back = match s.flush(a) {
                Ok(w) => format!("written: {}", w.written),
                Err(e) => format!("refused: {e}"),
            };
            return Err(format!(
                "{ROW}: {name} {flags:?}: accepted live; the write-back: {back}"
            ));
        }
        let r = o
            .result()
            .ok_or_else(|| format!("{ROW}: {argv:?}: --json gives no result"))?;
        let error = r.get("error").unwrap_or(&Json::Null);
        let detail = |k: &str| {
            error
                .get("details")
                .and_then(|d| d.get(k))
                .and_then(Json::as_str)
        };
        let next_verb = error
            .get("next_step")
            .and_then(|n| n.get("verb"))
            .and_then(Json::as_str);
        let next = format!("object show {addr}");
        need(
            o.exit == entry.exit
                && o.exit == 2
                && error.get("id").and_then(Json::as_str) == Some(entry.id)
                && detail("code") == Some("blend_driven")
                && detail("address") == Some(addr.as_str())
                && detail("channel") == Some(bits)
                && detail("next") == Some(next.as_str())
                && next_verb == Some("object show")
                && error
                    .get("message")
                    .and_then(Json::as_str)
                    .is_some_and(|m| m.contains(bits)),
            ROW,
            || {
                format!(
                    "{name} {flags:?}: exit {}, error {}; want blend_driven naming {bits}",
                    o.exit,
                    error.to_json()
                )
            },
        )?;
    }
    need(
        s.status().map_err(|e| format!("{ROW}: {e}"))?.version == v0
            && read_content(ROW, &s, a)? == c0,
        ROW,
        || "a refusal changed the live doc".into(),
    )?;
    need(f.bytes() == bytes, ROW, || {
        "the file changed under the refusals".into()
    })?;
    verb(
        ROW,
        &s,
        a,
        &[
            "object",
            "visibility",
            &at("Plain")?,
            "--viewport",
            "off",
            "--render",
            "off",
        ],
    )?;
    verb(
        ROW,
        &s,
        a,
        &["object", "visibility", &at("Mover")?, "--select", "off"],
    )?;
    flush(ROW, &s, a, "the flips nothing animates")?;
    let (_, m) = model_of(&f.bytes());
    let flag = |k: &str| model_field(&m, k, "restrictflag");
    need(
        flag("@ob:Plain").as_deref() == Some("5")
            && flag("@ob:Mover").as_deref() == Some("2")
            && ["@ob:Blinking", "@ob:Ghost", "@ob:Lost"]
                .iter()
                .all(|k| flag(k).as_deref() == Some("0")),
        ROW,
        || {
            format!(
                "restrictflag: Plain {:?}, Mover {:?}, Blinking {:?}, Ghost {:?}, Lost {:?}",
                flag("@ob:Plain"),
                flag("@ob:Mover"),
                flag("@ob:Blinking"),
                flag("@ob:Ghost"),
                flag("@ob:Lost")
            )
        },
    )?;
    let (f2, s2) = open("animated-transform", &scene());
    let a2 = f2.agent(&s2);
    let c2 = read_content(ROW, &s2, a2)?;
    let walker = format!("@obj:{}", &obj(&c2, "Walker", ROW)?[1..]);
    verb(
        ROW,
        &s2,
        a2,
        &["object", "visibility", &walker, "--viewport", "off"],
    )?;
    flush(ROW, &s2, a2, "an action's transform only")?;
    let (_, m2) = model_of(&f2.bytes());
    need(
        model_field(&m2, "@ob:Walker", "restrictflag").as_deref() == Some("1"),
        ROW,
        || {
            format!(
                "Walker's restrictflag {:?}, want 1",
                model_field(&m2, "@ob:Walker", "restrictflag")
            )
        },
    )
}

// ---------- a rename names its loss ----------

/// `object rename` without `--break-links` is refused and prints the loss; a name another
/// object has, an empty one and one longer than the name field are refused; nothing applies.
pub fn row_rename_needs_break_links() -> Result<(), String> {
    const ROW: &str = "row_rename_needs_break_links";
    let (f, s) = open("break-links", &scene());
    let a = f.agent(&s);
    let cube = obj(&read_content(ROW, &s, a)?, "Cube", ROW)?;
    let addr = format!("@obj:{}", &cube[1..]);
    let v0 = s.status().map_err(|e| format!("{ROW}: {e}"))?.version;
    let o = run_based(&s, a, &["object", "rename", &addr, "--name", "Box"])
        .map_err(|e| format!("{ROW}: {e}"))?;
    let out = stdout(&o);
    need(
        o.exit == 2
            && out.starts_with("refused: blend_breaks_links: ")
            && out.contains("@ob:Cube")
            && out.contains("--break-links"),
        ROW,
        || format!("exit {}: {out}", o.exit),
    )?;
    let long = "x".repeat(64);
    for (name, why) in [
        ("Lamp", "another object"),
        ("", "empty"),
        (long.as_str(), "over the file's name field"),
    ] {
        let o = run_based(
            &s,
            a,
            &["object", "rename", &addr, "--name", name, "--break-links"],
        )
        .map_err(|e| format!("{ROW}: {e}"))?;
        need(
            o.exit == 2
                && stdout(&o).starts_with("refused: blend_name: ")
                && stdout(&o).contains(why),
            ROW,
            || format!("rename to {name:?}: exit {} {}", o.exit, stdout(&o)),
        )?;
    }
    need(
        s.status().map_err(|e| format!("{ROW}: {e}"))?.version == v0,
        ROW,
        || "a refused rename applied something".into(),
    )
}

// ---------- the splice's rows ----------

/// A rename chain in one write-back: the cube becomes `Box`, then the lamp takes the name
/// `Cube`. Written to the right objects: the file's first mesh object is `Box`, the light
/// object `Cube`.
pub fn row_rename_chain_writes_each_name() -> Result<(), String> {
    const ROW: &str = "row_rename_chain_writes_each_name";
    let (f, s) = open("chain", &scene());
    let a = f.agent(&s);
    let c = read_content(ROW, &s, a)?;
    let (cube, lamp) = (obj(&c, "Cube", ROW)?, obj(&c, "Lamp", ROW)?);
    verb(
        ROW,
        &s,
        a,
        &[
            "object",
            "rename",
            &format!("@obj:{}", &cube[1..]),
            "--name",
            "Box",
            "--break-links",
        ],
    )?;
    verb(
        ROW,
        &s,
        a,
        &[
            "object",
            "rename",
            &format!("@obj:{}", &lamp[1..]),
            "--name",
            "Cube",
            "--break-links",
        ],
    )?;
    flush(ROW, &s, a, "the renames")?;
    let (_, m) = model_of(&f.bytes());
    let data_of = |k: &str| match m.by_key(k).map(|i| &i.detail) {
        Some(k2_mpc_blend::Detail::Object { data: Some(d), .. }) => m.ids[*d].key.clone(),
        _ => None,
    };
    need(
        data_of("@ob:Box").as_deref() == Some("@me:Cube")
            && data_of("@ob:Cube").as_deref() == Some("@la:Lamp")
            && m.by_key("@ob:Lamp").is_none(),
        ROW,
        || {
            format!(
                "Box has {:?}, Cube has {:?}",
                data_of("@ob:Box"),
                data_of("@ob:Cube")
            )
        },
    )
}

/// After a write and a restart (a new manager and a new pack: nothing in memory survives), a
/// verb is spliced into the stored carrier by the stored pairing, exactly.
pub fn row_restart_then_verb_writes_exactly() -> Result<(), String> {
    const ROW: &str = "row_restart_then_verb_writes_exactly";
    let mut f = fx("restart", |_| {});
    f.write(&scene());
    let s = f.open();
    let a = f.agent(&s);
    let c = read_content(ROW, &s, a)?;
    let (cube, lamp) = (obj(&c, "Cube", ROW)?, obj(&c, "Lamp", ROW)?);
    verb(
        ROW,
        &s,
        a,
        &[
            "object",
            "place",
            &format!("@obj:{}", &cube[1..]),
            "--loc",
            "3,3,3",
        ],
    )?;
    flush(ROW, &s, a, "the write before the restart")?;
    let written = f.bytes();
    drop(s);
    f.restart();
    let s = f.open();
    let a = f.agent(&s);
    verb(
        ROW,
        &s,
        a,
        &[
            "object",
            "place",
            &format!("@obj:{}", &lamp[1..]),
            "--loc",
            "8,8,8",
        ],
    )?;
    flush(ROW, &s, a, "the write after the restart")?;
    let out = f.bytes();
    let want = apply(
        &written,
        &Verb::Place {
            object: "@ob:Lamp".into(),
            loc: Some([8.0, 8.0, 8.0]),
            rot: None,
            scale: None,
        },
        &ReadLimits::default(),
    )
    .map_err(|e| format!("{ROW}: the file verb: {e}"))?;
    need(out == want.bytes, ROW, || {
        "the write after the restart isn't the file verb's patch of the written file".into()
    })
}

/// The differential oracle: live verbs then one write-back equal the file verbs applied to the
/// same file in the same order, byte for byte (plain).
pub fn row_live_verbs_equal_file_verbs() -> Result<(), String> {
    const ROW: &str = "row_live_verbs_equal_file_verbs";
    let start = scene();
    let (f, s) = open("oracle", &start);
    let a = f.agent(&s);
    let c = read_content(ROW, &s, a)?;
    let at = |n: &str| -> Result<String, String> { Ok(format!("@obj:{}", &obj(&c, n, ROW)?[1..])) };
    verb(
        ROW,
        &s,
        a,
        &[
            "object",
            "place",
            &at("Cube")?,
            "--loc",
            "1,2,3",
            "--rot",
            "0.5,0,0",
        ],
    )?;
    verb(
        ROW,
        &s,
        a,
        &[
            "object",
            "visibility",
            &at("Lamp")?,
            "--viewport",
            "off",
            "--select",
            "off",
        ],
    )?;
    verb(
        ROW,
        &s,
        a,
        &[
            "object",
            "rename",
            &at("Child")?,
            "--name",
            "Kid",
            "--break-links",
        ],
    )?;
    verb(
        ROW,
        &s,
        a,
        &["object", "place", &at("Spin")?, "--scale", "2,2,2"],
    )?;
    flush(ROW, &s, a, "the live verbs")?;
    let l = ReadLimits::default();
    let mut file = start;
    for v in [
        Verb::Place {
            object: "@ob:Cube".into(),
            loc: Some([1.0, 2.0, 3.0]),
            rot: Some([0.5, 0.0, 0.0]),
            scale: None,
        },
        Verb::Visibility {
            object: "@ob:Lamp".into(),
            viewport: Some(true),
            select: Some(true),
            render: None,
        },
        Verb::Rename {
            object: "@ob:Child".into(),
            name: "Kid".into(),
        },
        Verb::Place {
            object: "@ob:Spin".into(),
            loc: None,
            rot: None,
            scale: Some([2.0, 2.0, 2.0]),
        },
    ] {
        file = apply(&file, &v, &l)
            .map_err(|e| format!("{ROW}: file verb {v:?}: {e}"))?
            .bytes;
    }
    need(f.bytes() == file, ROW, || {
        "the live write-back differs from the file verbs' result".into()
    })
}

/// The pack's view of a pairing that doesn't fit: the hook refuses `PairingMismatch`, and
/// through the kit (a pack that leaves a key out of its pairings) the write-back is refused
/// typed (`MPC-E521`), the file as it was, the doc keeping its edit.
pub fn row_pairing_mismatch_refused_typed() -> Result<(), String> {
    const ROW: &str = "row_pairing_mismatch_refused_typed";
    let bytes = scene();
    let f = fx("mismatch", |_| {});
    f.write(&bytes);
    let s = f
        .mgr
        .open_file(
            Arc::new(ShortPairing(BlendLive::default())),
            &super::key(),
            &f.path(),
            &f.ws,
        )
        .map_err(|e| format!("{ROW}: open: {e}"))?;
    let a = f.agent(&s);
    // the cube is `@obj:2`; no read of the file's bytes here: that renders it too
    verb(ROW, &s, a, &["object", "place", "@obj:2", "--loc", "4,4,4"])?;
    let v = s.status().map_err(|e| format!("{ROW}: {e}"))?.version;
    let out = s.flush(a);
    need(
        matches!(out, Err(SessionError::PairingMismatch { .. })),
        ROW,
        || format!("the write-back isn't refused typed (MPC-E521): {out:?}"),
    )?;
    need(f.bytes() == bytes, ROW, || "the file changed".into())?;
    need(s.status().map(|x| x.version) == Ok(v), ROW, || {
        "the doc keeps the edit".into()
    })?;
    // the hook alone
    let pack = BlendLive::default();
    let d = pack.decode(&bytes).map_err(|e| format!("{ROW}: {e}"))?;
    let mut p = pack
        .pairing(&d.roots, &d.codec)
        .map_err(|e| format!("{ROW}: {e}"))?;
    for (r, keys) in &mut p.roots {
        if r == OBJS {
            keys.pop();
        }
    }
    let hook = splice::encode_from_paired(&bytes, &p, &d.roots, &LiveLimits::default());
    need(
        matches!(hook, Err(SpliceError::PairingMismatch(_))),
        ROW,
        || format!("the hook doesn't refuse PairingMismatch: {hook:?}"),
    )
}

/// [`BlendLive`] with a pairing that leaves out its last object key (a pack bug).
pub struct ShortPairing(pub BlendLive);

impl Pack for ShortPairing {
    fn schema(&self) -> Schema {
        self.0.schema()
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        self.0.decode(b)
    }
    fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        self.0.encode(c, codec)
    }
    fn wants_carrier(&self) -> bool {
        true
    }
    fn wants_pairing(&self) -> bool {
        true
    }
    fn pairing(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Pairing, PackError> {
        let mut p = self.0.pairing(c, codec)?;
        for (r, keys) in &mut p.roots {
            if r == OBJS {
                keys.pop();
            }
        }
        Ok(p)
    }
    fn encode_from_paired(
        &self,
        carrier: &[u8],
        pairing: &Pairing,
        c: &[(String, Neutral)],
        codec: &[u8],
    ) -> Result<Vec<u8>, SpliceError> {
        self.0.encode_from_paired(carrier, pairing, c, codec)
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        self.0.view_schema()
    }
    fn view_derive(&self, source: ViewSource<'_>, index: &mut ViewIndex) {
        self.0.view_derive(source, index)
    }
    fn view_derive_change(
        &self,
        source: ViewSource<'_>,
        index: &mut ViewIndex,
        touched: &k2_mpc_base::view_index::ViewTouched,
    ) -> bool {
        self.0.view_derive_change(source, index, touched)
    }
    fn view_placement(
        &self,
        source: ViewSource<'_>,
        root: &str,
        key: &k2_mpc_base::elem::Key,
    ) -> Option<Placement> {
        self.0.view_placement(source, root, key)
    }
    fn view_space(&self, source: ViewSource<'_>) -> Option<k2_mpc_base::placement::Space> {
        self.0.view_space(source)
    }
    fn address_of(&self, root: &str, key: &k2_mpc_base::elem::Key) -> String {
        self.0.address_of(root, key)
    }
    fn key_of(&self, root: &str, address: &str) -> Option<k2_mpc_base::elem::Key> {
        self.0.key_of(root, address)
    }
    fn reconcile(&self, synced: &View, bytes: &[u8], codec: &[u8]) -> Result<Reconciled, Declined> {
        self.0.reconcile(synced, bytes, codec)
    }
    fn verbs(&self) -> &'static [k2_mpc_base::tools::PackVerb] {
        self.0.verbs()
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        self.0.composite()
    }
}

// ---------- declines and bounded reads ----------

/// A file that can't be live is declined typed: another version, over the cap, not a BLEND
/// file, a pointer that names nothing on a listed object, a transform that isn't finite.
pub fn row_declines_are_typed() -> Result<(), String> {
    const ROW: &str = "row_declines_are_typed";
    let pack = BlendLive::default();
    let code = |b: &[u8]| pack.decode(b).err().map(|d| d.code);
    let old = {
        let mut b = SceneBuilder::new(k2_mpc_blend::HeaderForm::Classic, 8, false, 280, false);
        b.scene("Scene", &["A"], &[], None);
        b.object("A", &ObjectSpec::default());
        b.bytes()
    };
    let ghost = {
        let mut b = SceneBuilder::modern();
        b.scene("Scene", &["A"], &[], None);
        b.object(
            "A",
            &ObjectSpec {
                parent: Some("Ghost".into()),
                ..ObjectSpec::default()
            },
        );
        b.bytes()
    };
    let nan = {
        let mut b = SceneBuilder::modern();
        b.scene("Scene", &["A"], &[], None);
        b.object(
            "A",
            &ObjectSpec {
                loc: [f32::NAN, 0.0, 0.0],
                ..ObjectSpec::default()
            },
        );
        b.bytes()
    };
    let small = BlendLive::new(LiveLimits {
        max_file_bytes: 1000,
        ..LiveLimits::default()
    });
    for (what, got, want) in [
        ("an older version", code(&old), "live_version"),
        ("a pointer to nothing", code(&ghost), "live_ambiguous"),
        ("a NaN", code(&nan), "live_value"),
        ("not a BLEND file", code(b"hello"), "not_blend"),
        (
            "over the cap",
            small.decode(&scene()).err().map(|d| d.code),
            "live_too_large",
        ),
    ] {
        need(got == Some(want), ROW, || {
            format!("{what}: {got:?}, want {want}")
        })?;
    }
    need(pack.decode(&scene()).is_ok(), ROW, || {
        "control: the scene opens".into()
    })
}

/// Reads are bounded and paged: `collection show` gives at most 200 objects a page and names
/// the next page; `scene list --limit`; `object show` is one object; bad addresses are typed.
pub fn row_reads_are_bounded_and_paged() -> Result<(), String> {
    const ROW: &str = "row_reads_are_bounded_and_paged";
    let (f, s) = open("reads", &super::big_scene(3, 250, false));
    let a = f.agent(&s);
    let o = run(&s, a, &["collection", "show", "@coll:2"]).map_err(|e| format!("{ROW}: {e}"))?;
    let out = stdout(&o);
    let lines = out.lines().filter(|l| l.starts_with("@obj:")).count();
    need(
        o.exit == 0
            && lines == 200
            && out.contains("next: collection show @coll:2 --after @obj:200"),
        ROW,
        || {
            format!(
                "first page: {lines} objects; {}",
                out.lines().last().unwrap_or("")
            )
        },
    )?;
    let o = run(
        &s,
        a,
        &["collection", "show", "@coll:2", "--after", "@obj:200"],
    )
    .map_err(|e| format!("{ROW}: {e}"))?;
    let lines = stdout(&o)
        .lines()
        .filter(|l| l.starts_with("@obj:"))
        .count();
    need(lines == 50 && !stdout(&o).contains("next:"), ROW, || {
        format!("second page: {lines}")
    })?;
    let o = run(&s, a, &["scene", "list", "--limit", "2"]).map_err(|e| format!("{ROW}: {e}"))?;
    need(
        stdout(&o).lines().count() == 3 && stdout(&o).contains("next: scene list --after @coll:2"),
        ROW,
        || format!("scene list --limit 2: {}", stdout(&o)),
    )?;
    let o = run(&s, a, &["object", "show", "@obj:7"]).map_err(|e| format!("{ROW}: {e}"))?;
    need(
        o.exit == 0
            && stdout(&o).contains("nid\t@ob:Obj_0_6")
            && stdout(&o).contains("coll\t@coll:2"),
        ROW,
        || format!("object show: {}", stdout(&o)),
    )?;
    let o = run(&s, a, &["object", "show", "@obj:9999"]).map_err(|e| format!("{ROW}: {e}"))?;
    need(o.exit == 4, ROW, || {
        format!("a missing object: exit {}", o.exit)
    })?;
    let o = run(&s, a, &["collection", "show", "@coll:1", "--limit", "201"]);
    need(
        o.is_err() || o.as_ref().is_ok_and(|o| o.exit == 2),
        ROW,
        || "--limit over 200 is refused".into(),
    )
}

// ---------- placements in 3D ----------
//
// Each step's failure reads `<row>: <step>: <why>`. A row whose plant is in
// `tests/mutants.rs` has a `control` step first that its plant leaves passing (the file opened,
// the objects in view), then its asserted steps.

/// `Err("<row>: <step>: <why>")` unless `ok`.
fn step(ok: bool, row: &str, name: &str, why: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {name}: {}", why()))
    }
}

/// The view index of `s` as `c` reads it.
fn view(row: &str, s: &Session, c: ConnId) -> Result<ViewIndex, String> {
    s.view_index(c)
        .map_err(|e| format!("{row}: setup: {e}"))?
        .ok_or_else(|| format!("{row}: setup: no view index"))
}

/// The address of the live object named `name`.
fn obj_addr(c: &[(String, Neutral)], name: &str, row: &str) -> Result<String, String> {
    let k = obj(c, name, row)?;
    Ok(format!("@obj:{}", &k[1..]))
}

/// The unit at `addr`, in step `at` of `row`.
fn unit_at<'a>(ix: &'a ViewIndex, addr: &str, row: &str, at: &str) -> Result<&'a Unit, String> {
    ix.unit(addr)
        .ok_or_else(|| format!("{row}: {at}: no unit {addr}"))
}

/// The world matrix of `addr`, in step `at` of `row`.
fn world(ix: &ViewIndex, addr: &str, row: &str, at: &str) -> Result<Mat4, String> {
    ix.world_matrix(addr)
        .ok_or_else(|| format!("{row}: {at}: {addr} has no world matrix"))
}

/// One `Trs` step, Euler `XYZ`, from the file's `f32`s.
fn trs_step(loc: [f32; 3], rot: [f32; 3], scale: [f32; 3]) -> Step {
    Step::Trs {
        loc: loc.map(f64::from),
        rot: Rotation::Euler {
            order: EulerOrder::Xyz,
            angles: rot.map(f64::from),
        },
        scale: scale.map(f64::from),
    }
}

/// [`trs_step`] relative to nothing, as the slot carries it.
fn trs(loc: [f32; 3], rot: [f32; 3], scale: [f32; 3]) -> Placement {
    Placement {
        steps: vec![trs_step(loc, rot, scale)],
        precision: Precision::F32,
        ..Placement::default()
    }
}

/// Whether `m` is within 1e-6 of `want`, entry by entry (both column-major).
fn near(m: &Mat4, want: &[f64; 16]) -> bool {
    m.iter().zip(want).all(|(a, b)| (a - b).abs() <= 1e-6)
}

/// A move by `t`, column-major.
fn moved(t: [f64; 3]) -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, t[0], t[1], t[2], 1.0,
    ]
}

/// A keyed watch on `c`, its snapshot taken.
fn keyed_watch(
    row: &str,
    s: &Session,
    c: ConnId,
) -> Result<k2_mpc_base::session::KeyedWatch, String> {
    let mut w = s
        .watch_keyed(c, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
        .map_err(|e| format!("{row}: setup: {e}"))?
        .ok_or_else(|| format!("{row}: setup: no keyed watch"))?;
    s.watch_snapshot(&mut w)
        .map_err(|e| format!("{row}: setup: the snapshot: {e}"))?;
    Ok(w)
}

/// The change frames among `frames`, as JSON.
fn change_frames(frames: &[WatchFrame]) -> Vec<Json> {
    frames
        .iter()
        .filter(|fr| matches!(fr, WatchFrame::Change(_)))
        .map(WatchFrame::to_value)
        .collect()
}

/// The unit delta of `addr` in the first of `frames` that has one.
fn delta_of<'a>(frames: &'a [Json], addr: &str) -> Option<&'a Json> {
    frames.iter().find_map(|fr| match fr.get("units") {
        Some(Json::List(us)) => us
            .iter()
            .find(|u| u.get("key").and_then(Json::as_str) == Some(addr)),
        _ => None,
    })
}

/// The `control` step of the plant rows: every one of `names` is a unit of the view.
fn in_view(
    ix: &ViewIndex,
    c: &[(String, Neutral)],
    names: &[&str],
    row: &str,
) -> Result<(), String> {
    for name in names {
        let at = obj_addr(c, name, row)?;
        step(ix.unit(&at).is_some(), row, "control", || {
            format!("{name} ({at}) isn't in the view")
        })?;
    }
    Ok(())
}

/// The placed outliner: the doc's space from the scene's unit scale; every object placed in
/// the file's own form (Euler, a quaternion, an axis and angle; a child through its parent
/// inverse; a driven transform marked `driven`), and none where the file can't say exactly
/// where it is (a delta transform, a parenting other than to the object itself); no unit text,
/// the transform and the data as derived values.
pub fn row_view_places_every_object() -> Result<(), String> {
    const ROW: &str = "row_view_places_every_object";
    let (f, s) = open("placed", &placed_scene());
    let a = f.agent(&s);
    let c = read_content(ROW, &s, a)?;
    let ix = view(ROW, &s, a)?;
    let want_space = Space {
        length_unit: "scene-unit".into(),
        metres_per_unit: Some(f64::from(UNIT_SCALE)),
        up: Axis::Z,
        handed: Handed::Right,
    };
    step(ix.space.as_ref() == Some(&want_space), ROW, "space", || {
        format!("the doc's space {:?}", ix.space)
    })?;
    let cube = unit_at(&ix, &obj_addr(&c, "Cube", ROW)?, ROW, "euler")?;
    let want = trs([0.0, 0.0, 0.25], [0.0; 3], [1.0; 3]);
    step(cube.placement.as_ref() == Some(&want), ROW, "euler", || {
        format!("Cube's placement {:?}", cube.placement)
    })?;
    step(
        cube.text.is_none()
            && cube.derived_value("transform") == Some("loc 0,0,0.25 rot 0,0,0 scale 1,1,1")
            && cube.derived_value("data") == Some("mesh @me:Cube"),
        ROW,
        "derived",
        || format!("Cube's text {:?}, derived {:?}", cube.text, cube.derived),
    )?;
    let loose = unit_at(&ix, &obj_addr(&c, "Loose", ROW)?, ROW, "derived")?;
    step(
        loose.derived_value("data").is_none() && loose.derived_value("transform").is_some(),
        ROW,
        "derived",
        || format!("an empty's derived values {:?}", loose.derived),
    )?;
    let spin = unit_at(&ix, &obj_addr(&c, "Spin", ROW)?, ROW, "quaternion")?;
    let quat = matches!(
        spin.placement.as_ref().map(|p| p.steps.as_slice()),
        Some([Step::Trs { rot: Rotation::Quat(q), .. }]) if *q == [1.0, 0.0, 0.0, 0.0]
    );
    step(quat, ROW, "quaternion", || {
        format!("Spin's placement {:?}", spin.placement)
    })?;
    let axis_at = obj_addr(&c, "Axis", ROW)?;
    let axis = unit_at(&ix, &axis_at, ROW, "axis-angle")?;
    let aa = matches!(
        axis.placement.as_ref().map(|p| p.steps.as_slice()),
        Some([Step::Trs { rot: Rotation::AxisAngle { axis, angle }, .. }])
            if *axis == [0.0, 0.0, 1.0] && *angle == 0.5
    );
    step(aa, ROW, "axis-angle", || {
        format!("Axis's placement {:?}", axis.placement)
    })?;
    let m = world(&ix, &axis_at, ROW, "axis-angle")?;
    let (sn, cs) = (0.5f64.sin(), 0.5f64.cos());
    let want_m = [
        cs, sn, 0.0, 0.0, -sn, cs, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 2.0, 0.0, 0.0, 1.0,
    ];
    step(near(&m, &want_m), ROW, "axis-angle", || {
        format!("Axis's world matrix {m:?}, want a turn of 0.5 about Z at (2, 0, 0)")
    })?;
    for name in ["Walker", "Held"] {
        let u = unit_at(&ix, &obj_addr(&c, name, ROW)?, ROW, "driven")?;
        step(
            u.placement.as_ref().and_then(|p| p.approx.as_deref()) == Some("driven"),
            ROW,
            "driven",
            || format!("{name}'s placement {:?}", u.placement),
        )?;
    }
    let idle = unit_at(&ix, &obj_addr(&c, "Idle", ROW)?, ROW, "driven")?;
    step(
        idle.placement.as_ref().is_some_and(|p| p.approx.is_none()),
        ROW,
        "driven",
        || format!("Idle's animation data holds nothing: {:?}", idle.placement),
    )?;
    for name in ["Nudged", "Boned"] {
        let at = obj_addr(&c, name, ROW)?;
        let u = unit_at(&ix, &at, ROW, "unplaced")?;
        step(
            u.placement.is_none() && ix.world_matrix(&at).is_none(),
            ROW,
            "unplaced",
            || format!("{name} is placed: {:?}", u.placement),
        )?;
    }
    let placed = ix.units.iter().filter(|u| u.placement.is_some()).count();
    step(placed == 11 && ix.units.len() == 13, ROW, "count", || {
        format!("{placed} of {} units placed, want 11 of 13", ix.units.len())
    })
}

/// A child is placed relative to its parent by the parent inverse the file keeps, then its own
/// transform: its world matrix is the parent's, the inverse, then its own.
pub fn row_parent_inverse_composes() -> Result<(), String> {
    const ROW: &str = "row_parent_inverse_composes";
    let (f, s) = open("inverse", &placed_scene());
    let a = f.agent(&s);
    let c = read_content(ROW, &s, a)?;
    let ix = view(ROW, &s, a)?;
    in_view(&ix, &c, &["Cube", "Child"], ROW)?;
    let cube = obj_addr(&c, "Cube", ROW)?;
    let child = obj_addr(&c, "Child", ROW)?;
    let m = world(&ix, &cube, ROW, "parent-world")?;
    step(
        near(&m, &moved([0.0, 0.0, 0.25])),
        ROW,
        "parent-world",
        || format!("the cube's world matrix {m:?}"),
    )?;
    let p = unit_at(&ix, &child, ROW, "child-steps")?
        .placement
        .clone()
        .ok_or_else(|| format!("{ROW}: child-steps: the child has no placement"))?;
    let want = Placement {
        relative_to: Some(cube.clone()),
        steps: vec![
            Step::Matrix(CHILD_INVERSE.map(f64::from)),
            trs_step([0.0, 1.0, 0.0], [0.0; 3], [1.0; 3]),
        ],
        precision: Precision::F32,
        ..Placement::default()
    };
    step(p == want, ROW, "child-steps", || {
        format!("the child's placement {p:?}, want the parent inverse, then its own")
    })?;
    let m = world(&ix, &child, ROW, "child-world")?;
    step(
        near(&m, &moved([-1.0, 1.0, 0.25])),
        ROW,
        "child-world",
        || format!("the child's world matrix {m:?}"),
    )
}

/// Euler angles turn about the first axis of the mode's word first, each about the parent's
/// fixed axes (`xyz` is Z after Y after X), and each mode is its own word.
pub fn row_euler_order_words() -> Result<(), String> {
    const ROW: &str = "row_euler_order_words";
    let (f, s) = open("euler", &turned_scene());
    let a = f.agent(&s);
    let c = read_content(ROW, &s, a)?;
    let ix = view(ROW, &s, a)?;
    let all = [
        "Z90", "XZ90", "Mode1", "Mode2", "Mode3", "Mode4", "Mode5", "Mode6",
    ];
    in_view(&ix, &c, &all, ROW)?;
    let m = world(&ix, &obj_addr(&c, "Z90", ROW)?, ROW, "one-axis")?;
    let z90 = [
        0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    step(near(&m, &z90), ROW, "one-axis", || {
        format!("Z90's world matrix {m:?}")
    })?;
    let m = world(&ix, &obj_addr(&c, "XZ90", ROW)?, ROW, "xyz-order")?;
    let xz90 = [
        0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    step(near(&m, &xz90), ROW, "xyz-order", || {
        format!("XZ90's world matrix {m:?}, want about X, then about Z")
    })?;
    let angles = [0.1f32, 0.2, 0.3].map(f64::from);
    for (n, word) in ["XYZ", "XZY", "YXZ", "YZX", "ZXY", "ZYX"]
        .into_iter()
        .enumerate()
    {
        let name = format!("Mode{}", n + 1);
        let u = unit_at(&ix, &obj_addr(&c, &name, ROW)?, ROW, "mode-words")?;
        let got = match u.placement.as_ref().map(|p| p.steps.as_slice()) {
            Some(
                [Step::Trs {
                    rot: Rotation::Euler { order, angles },
                    ..
                }],
            ) => Some((order.as_str(), *angles)),
            _ => None,
        };
        step(got == Some((word, angles)), ROW, "mode-words", || {
            format!("{name}'s rotation {got:?}, want {word}")
        })?;
    }
    Ok(())
}

/// An object the file opened with is placed, and one an app's save adds reaches the watcher
/// placed: in the add delta, or in the fresh snapshot the watch is told to take.
pub fn row_placement_arrives_on_add() -> Result<(), String> {
    const ROW: &str = "row_placement_arrives_on_add";
    let (f, s) = open("place-add", &placed_scene());
    let a = f.agent(&s);
    let mut w = keyed_watch(ROW, &s, a)?;
    let c = read_content(ROW, &s, a)?;
    let ix = view(ROW, &s, a)?;
    in_view(&ix, &c, &["Lamp"], ROW)?;
    let lamp = obj_addr(&c, "Lamp", ROW)?;
    let want_lamp = trs([4.0, -1.0, 6.0], [0.0; 3], [1.0; 3]);
    let got_lamp = &unit_at(&ix, &lamp, ROW, "opened-placed")?.placement;
    step(
        got_lamp.as_ref() == Some(&want_lamp),
        ROW,
        "opened-placed",
        || format!("the lamp's placement {got_lamp:?}"),
    )?;
    f.advance(1_000);
    let save = outliner(&Opts {
        added_first: true,
        placed: true,
        shift: 0x20_0000,
        ..Opts::default()
    });
    app_save(&f.path(), &save);
    let out = s.file_changed().map_err(|e| format!("{ROW}: save: {e}"))?;
    step(
        matches!(out, DiskCheck::Ingested { .. }),
        ROW,
        "save",
        || format!("the save isn't taken in: {out:?}"),
    )?;
    let added = obj_addr(&read_content(ROW, &s, a)?, "Added", ROW)?;
    let frames = change_frames(&w.try_frames());
    let d = delta_of(&frames, &added);
    let ops = d
        .and_then(|d| d.get("ops"))
        .map(Json::to_json)
        .unwrap_or_default();
    let got = d.and_then(|d| d.get("placement_after")).map(Json::to_json);
    let want = trs([9.0; 3], [0.0; 3], [1.0; 3]).to_value().to_json();
    step(ops.contains("\"add\""), ROW, "add-delta", || {
        format!("no add of {added}: ops {ops}")
    })?;
    // the add delta carries the placement, and no snapshot is asked for: the frame is built on
    // the save's carrier
    let asked = w.needs_snapshot();
    step(
        got.as_deref() == Some(want.as_str()) && !asked,
        ROW,
        "add-placed",
        || {
            format!(
                "the add of {added}: placement_after {got:?}, want {want}; the watch asks for a \
                 snapshot: {asked}"
            )
        },
    )?;
    step(delta_of(&frames, &lamp).is_none(), ROW, "others", || {
        format!("the lamp has a delta: {:?}", delta_of(&frames, &lamp))
    })
}

/// Objects that move between carriers keep their placements: a host's move of the child to
/// the front of the objects (no new carrier) builds that one unit again, not the whole index,
/// and every object keeps its placement, the child its parent inverse. Control first: the
/// child placed through its parent inverse before the move. The live pack refuses a move at the
/// call (`blend_live_change`), so the row opens the scene with the kit's edit check off
/// ([`super::unchecked::NoEditCheck`]): its subject is the view's derive of the move.
pub fn row_move_keeps_placements() -> Result<(), String> {
    const ROW: &str = "row_move_keeps_placements";
    let (f, s) = super::unchecked::open_unchecked("move", &placed_scene());
    let host = f.person(&s);
    let c = read_content(ROW, &s, host)?;
    let child = obj(&c, "Child", ROW)?;
    let child_at = obj_addr(&c, "Child", ROW)?;
    let before = view(ROW, &s, host)?;
    let placed = matches!(
        unit_at(&before, &child_at, ROW, "control")?.placement.as_ref().map(|p| p.steps.as_slice()),
        Some([Step::Matrix(m), _]) if *m == CHILD_INVERSE.map(f64::from)
    );
    step(placed, ROW, "control", || {
        "the child isn't placed through its parent inverse before the move".into()
    })?;
    let key = k2_mpc_base::elem::Key::new(child.clone()).map_err(|e| format!("{ROW}: {e}"))?;
    let mut opts = k2_mpc_base::session::EditOptions::new();
    opts.base = Some(s.status().map_err(|e| format!("{ROW}: {e}"))?.version);
    let edit = k2_mpc_base::doc::Edit::List {
        root: OBJS.to_owned(),
        ops: vec![k2_mpc_base::elem::ListOp::Move {
            key,
            to: k2_mpc_base::elem::Anchor::Start,
        }],
    };
    // the units built for the read after the move, with the kit's whole-build check off
    let checks = k2_mpc_base::differential::set_view_checks(false);
    k2_mpc_base::differential::take_view_unit_builds();
    let moved = s.edit_ops(host, vec![edit], &opts);
    let after = view(ROW, &s, host);
    let builds = k2_mpc_base::differential::take_view_unit_builds();
    k2_mpc_base::differential::set_view_checks(checks);
    // (the doc isn't read back as content: live v1's write-back refuses a move, so the file
    // doesn't render; the move is checked by the edit's own result)
    moved.map_err(|e| format!("{ROW}: move: the move of {child} wasn't taken: {e}"))?;
    let after = after?;
    for u in &before.units {
        let now = after.unit(&u.key).and_then(|x| x.placement.clone());
        step(now == u.placement, ROW, "placements", || {
            format!(
                "{}: {:?} after the move, {:?} before",
                u.key, now, u.placement
            )
        })?;
    }
    println!(
        "{ROW}: the read after the move built {builds} of {} units",
        after.units.len()
    );
    step(builds == 1, ROW, "builds", || {
        format!("the read after the move built {builds} units, not the moved one")
    })
}

/// `object place` moves its object's placement: one `place` delta with the whole new
/// placement, nothing for its child, whose collection the render hint names.
pub fn row_place_verb_moves_its_placement() -> Result<(), String> {
    const ROW: &str = "row_place_verb_moves_its_placement";
    let (f, s) = open("place-frame", &placed_scene());
    let a = f.agent(&s);
    let c = read_content(ROW, &s, a)?;
    let (cube, child) = (obj_addr(&c, "Cube", ROW)?, obj_addr(&c, "Child", ROW)?);
    let mut w = keyed_watch(ROW, &s, a)?;
    verb(ROW, &s, a, &["object", "place", &cube, "--rot", "0,0,1.5"])?;
    let frames = change_frames(&w.try_frames());
    let d = delta_of(&frames, &cube);
    let ops = d
        .and_then(|d| d.get("ops"))
        .map(Json::to_json)
        .unwrap_or_default();
    let got = d.and_then(|d| d.get("placement_after")).map(Json::to_json);
    let want = trs([0.0, 0.0, 0.25], [0.0, 0.0, 1.5], [1.0; 3])
        .to_value()
        .to_json();
    step(
        ops.contains("\"place\"") && got.as_deref() == Some(want.as_str()),
        ROW,
        "place-delta",
        || format!("the cube's delta: ops {ops}, placement_after {got:?}, want {want}"),
    )?;
    step(delta_of(&frames, &child).is_none(), ROW, "child", || {
        format!("the child has a delta: {:?}", delta_of(&frames, &child))
    })?;
    let hint = frames
        .iter()
        .find_map(|fr| fr.get("containers"))
        .map(Json::to_json)
        .unwrap_or_default();
    step(hint.contains("\"@coll:3\""), ROW, "hint", || {
        format!("the render hint {hint} doesn't name the child's @coll:3")
    })
}

// ---------- the doc's space: metres per unit only from agreeing metric scenes ----------

/// The doc's space's metres per unit of a file of scenes `(unit.system, unit.scale_length)`,
/// opened live, for step `at` of `row`.
fn metres_per_unit(row: &str, at: &str, scenes: &[(u8, f32)]) -> Result<Option<f64>, String> {
    let (f, s) = open("units", &units_scene(scenes));
    let a = f.agent(&s);
    let ix = view(row, &s, a)?;
    let space = ix
        .space
        .ok_or_else(|| format!("{row}: {at}: no doc space for {scenes:?}"))?;
    Ok(space.metres_per_unit)
}

/// Every scene metric (`unit.system` 1) with the same scale: the doc's space gives that scale
/// as metres per unit (two scenes, and the placed outliner's one); scales that disagree give
/// none.
pub fn row_space_metric_scale() -> Result<(), String> {
    const ROW: &str = "row_space_metric_scale";
    let want = Some(f64::from(UNIT_SCALE));
    let got = metres_per_unit(ROW, "agreeing", &[(1, UNIT_SCALE), (1, UNIT_SCALE)])?;
    step(got == want, ROW, "agreeing", || {
        format!("two metric scenes at {UNIT_SCALE}: {got:?}, want {want:?}")
    })?;
    let (f, s) = open("units-outliner", &placed_scene());
    let a = f.agent(&s);
    let got = view(ROW, &s, a)?.space.and_then(|sp| sp.metres_per_unit);
    step(got == want, ROW, "outliner", || {
        format!("the placed outliner: {got:?}, want {want:?}")
    })?;
    let got = metres_per_unit(ROW, "disagreeing", &[(1, UNIT_SCALE), (1, 1.0)])?;
    step(got.is_none(), ROW, "disagreeing", || {
        format!("two metric scenes at {UNIT_SCALE} and 1: {got:?}, want none")
    })
}

/// A scene whose unit system is 0 gives no metres per unit (unknown), never an assumed scale.
/// Control first: a metric scene gives its scale.
pub fn row_space_system_zero_unknown() -> Result<(), String> {
    const ROW: &str = "row_space_system_zero_unknown";
    let want = Some(f64::from(UNIT_SCALE));
    let got = metres_per_unit(ROW, "control", &[(1, UNIT_SCALE)])?;
    step(got == want, ROW, "control", || {
        format!("a metric scene at {UNIT_SCALE}: {got:?}, want {want:?}")
    })?;
    for scale in [1.0, UNIT_SCALE] {
        let got = metres_per_unit(ROW, "system-zero", &[(0, scale)])?;
        step(got.is_none(), ROW, "system-zero", || {
            format!("a scene of unit system 0 at {scale}: {got:?}, want none")
        })?;
    }
    Ok(())
}

/// Scenes of different unit systems, or of a system other than 1, give no metres per unit,
/// even when their scales agree.
pub fn row_space_mixed_systems_unknown() -> Result<(), String> {
    const ROW: &str = "row_space_mixed_systems_unknown";
    for (at, scenes) in [
        ("metric-and-zero", vec![(1u8, 1.0f32), (0, 1.0)]),
        ("metric-and-two", vec![(1, 1.0), (2, 1.0)]),
        ("system-two", vec![(2, 1.0)]),
    ] {
        let got = metres_per_unit(ROW, at, &scenes)?;
        step(got.is_none(), ROW, at, || {
            format!("scenes {scenes:?}: {got:?}, want none")
        })?;
    }
    Ok(())
}

// ---------- a driven object's place deltas are approximate ----------

/// A placed scene of two objects in `Set`: `Held`, driven by a constraint, at `held`, and
/// `Plain`, driven by nothing, at `plain`. `shift` as [`Opts::shift`]: another save of it.
fn driven_pair(held: [f32; 3], plain: [f32; 3], shift: u64) -> Vec<u8> {
    let mut b = SceneBuilder::placed(403);
    b.shift = shift;
    b.ui();
    b.scene("Scene", &[], &["Set"], None);
    b.collection("Set", &["Held", "Plain"], &[]);
    b.object(
        "Held",
        &ObjectSpec {
            rotmode: 1,
            loc: held,
            driven: testgen::Driven::Constraint,
            ..ObjectSpec::default()
        },
    );
    b.object(
        "Plain",
        &ObjectSpec {
            rotmode: 1,
            loc: plain,
            ..ObjectSpec::default()
        },
    );
    b.bytes()
}

/// The `approx` of `addr`'s place delta in `frames`: `Err` when it has no `place` delta.
fn place_approx(frames: &[Json], addr: &str) -> Result<Option<String>, String> {
    let d = delta_of(frames, addr).ok_or_else(|| format!("{addr} has no delta"))?;
    let ops = d.get("ops").map(Json::to_json).unwrap_or_default();
    if !ops.contains("\"place\"") {
        return Err(format!("{addr}'s delta has no place op: {}", d.to_json()));
    }
    let p = d
        .get("placement_after")
        .ok_or_else(|| format!("{addr}'s place delta has no placement_after"))?;
    Ok(p.get("approx").and_then(Json::as_str).map(str::to_owned))
}

/// A driven object's box is approximate from its first move: the first `place` delta of a
/// driven object (moved by an app's save: the verb refuses to place it) carries `approx`
/// `driven` in its `placement_after`, and an undriven one's (in the same save, and moved by
/// `object place`) carries none. An older reader of the frame (the kit's
/// `Placement::from_value` with the field taken out) reads the same placement without it.
pub fn row_driven_first_place_delta_is_approx() -> Result<(), String> {
    const ROW: &str = "row_driven_first_place_delta_is_approx";
    let (f, s) = open("driven-delta", &driven_pair([0.0; 3], [0.0; 3], 0));
    let a = f.agent(&s);
    let c = read_content(ROW, &s, a)?;
    let (held, plain) = (obj_addr(&c, "Held", ROW)?, obj_addr(&c, "Plain", ROW)?);
    let ix = view(ROW, &s, a)?;
    let approx = |ix: &ViewIndex, at: &str| {
        ix.unit(at)
            .and_then(|u| u.placement.as_ref())
            .and_then(|p| p.approx.clone())
    };
    step(
        approx(&ix, &held).as_deref() == Some("driven") && approx(&ix, &plain).is_none(),
        ROW,
        "control",
        || {
            format!(
                "the index: Held {:?}, Plain {:?}",
                approx(&ix, &held),
                approx(&ix, &plain)
            )
        },
    )?;
    let mut w = keyed_watch(ROW, &s, a)?;
    w.try_frames();
    // an app's save moves both: Held's first move
    app_save(
        &f.path(),
        &driven_pair([1.0, 0.0, 0.0], [2.0, 0.0, 0.0], 0x30_0000),
    );
    let out = s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    step(
        matches!(out, DiskCheck::Ingested { .. }),
        ROW,
        "control",
        || format!("the save is taken in: {out:?}"),
    )?;
    let first = change_frames(&w.try_frames());
    let got = place_approx(&first, &held).map_err(|e| format!("{ROW}: driven: {e}"))?;
    step(got.as_deref() == Some("driven"), ROW, "driven", || {
        format!("Held's first place delta: approx {got:?}, want \"driven\"")
    })?;
    let got = place_approx(&first, &plain).map_err(|e| format!("{ROW}: undriven: {e}"))?;
    step(got.is_none(), ROW, "undriven", || {
        format!("Plain's place delta (the save): approx {got:?}, want none")
    })?;
    verb(ROW, &s, a, &["object", "place", &plain, "--loc", "3,0,0"])?;
    let frames = change_frames(&w.try_frames());
    let got = place_approx(&frames, &plain).map_err(|e| format!("{ROW}: undriven: {e}"))?;
    step(got.is_none(), ROW, "undriven", || {
        format!("Plain's place delta (the verb): approx {got:?}, want none")
    })?;
    // an older reader knows no `approx` and passes over it: Held's placement_after read with
    // the field and with it taken out gives the same steps and parent, only the reason differs
    let wire = delta_of(&first, &held)
        .and_then(|d| d.get("placement_after"))
        .cloned()
        .ok_or_else(|| format!("{ROW}: older: no placement_after for Held"))?;
    let mut bare = wire.clone();
    if let Json::Obj(m) = &mut bare {
        m.retain(|(k, _)| k != "approx");
    }
    let (now, older) = (
        Placement::from_value(&wire, Precision::F32),
        Placement::from_value(&bare, Precision::F32),
    );
    step(
        matches!((&now, &older), (Some(n), Some(o))
            if n.steps == o.steps && n.relative_to == o.relative_to
                && n.approx.as_deref() == Some("driven") && o.approx.is_none()),
        ROW,
        "older",
        || format!("{}: read {now:?}, without approx {older:?}", wire.to_json()),
    )
}

// ---------- the collection tree ----------

/// The file's own collection tree, by name (sorted by the caller): each collection's first holder (a scene's master
/// collection or a collection, in file order, as the file's lists hold them), read from the
/// file's data-blocks; a scene, or a collection nothing holds, is at the top level (`None`).
fn file_tree(bytes: &[u8]) -> Vec<(String, Option<String>)> {
    let (_, m) = model_of(bytes);
    let mut holder: Vec<Option<usize>> = vec![None; m.ids.len()];
    for (i, id) in m.ids.iter().enumerate() {
        let held = match &id.detail {
            k2_mpc_blend::Detail::Scene { collections, .. }
            | k2_mpc_blend::Detail::Collection { collections, .. } => collections,
            _ => continue,
        };
        for &c in held {
            if c != i && holder[c].is_none() {
                holder[c] = Some(i);
            }
        }
    }
    m.ids
        .iter()
        .enumerate()
        .filter(|(_, id)| {
            id.library.is_none() && matches!(id.kind.as_str(), "scene" | "collection")
        })
        .map(|(i, id)| (id.name.clone(), holder[i].map(|h| m.ids[h].name.clone())))
        .collect()
}

/// The view's collection tree, by name: each named container and its parent container's name
/// (`None` at the top level). A parent that names no container of the view is `"?<address>"`.
fn view_tree(ix: &ViewIndex) -> Vec<(String, Option<String>)> {
    ix.containers
        .iter()
        .filter_map(|c| {
            let name = c.title.clone()?;
            let parent = c.parent.as_ref().map(|p| {
                ix.container(p)
                    .and_then(|x| x.title.clone())
                    .unwrap_or_else(|| format!("?{p}"))
            });
            Some((name, parent))
        })
        .collect()
}

/// A collection inside a collection gets its parent's address, in the view index and on the
/// snapshot, so an outliner draws the file's own collection tree: the outliner scene (`Inner`
/// inside `Set`, `Set` and `Props` inside the scene) and the rig (`Rig` and `Crowd` inside the
/// scene). A scene's master collection, and `c0`, stay at the top level.
pub fn row_collection_tree_is_the_files() -> Result<(), String> {
    const ROW: &str = "row_collection_tree_is_the_files";
    for (name, bytes) in [("outliner", scene()), ("rig", rig())] {
        let mut want = file_tree(&bytes);
        want.sort();
        let nested = want.iter().filter(|(_, p)| p.is_some()).count();
        step(nested >= 2, ROW, "control", || {
            format!("{name}: the file nests {nested} collections: {want:?}")
        })?;
        let (f, s) = open(&format!("tree-{name}"), &bytes);
        let a = f.agent(&s);
        let ix = view(ROW, &s, a)?;
        let mut got = view_tree(&ix);
        got.sort();
        step(got == want, ROW, "tree", || {
            format!("{name}: the view's tree {got:?}, the file's {want:?}")
        })?;
        let tops: Vec<&str> = ix
            .containers
            .iter()
            .filter(|c| c.parent.is_none())
            .map(|c| c.key.as_str())
            .collect();
        let scenes: Vec<&str> = ix
            .containers
            .iter()
            .filter(|c| c.title.as_deref() == Some("Scene") || c.title.is_none())
            .map(|c| c.key.as_str())
            .collect();
        step(scenes.iter().all(|k| tops.contains(k)), ROW, "top", || {
            format!("{name}: the scene and c0 {scenes:?}, the top level {tops:?}")
        })?;
        step(ix.refusals.is_empty(), ROW, "refusals", || {
            format!("{name}: the view refused {:?}", ix.refusals)
        })?;
        // the snapshot carries each nested container's parent, by address, and none for the rest
        let mut w = s
            .watch_keyed(a, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
            .map_err(|e| format!("{ROW}: setup: {e}"))?
            .ok_or_else(|| format!("{ROW}: setup: no keyed watch"))?;
        let frames = s
            .watch_snapshot(&mut w)
            .map_err(|e| format!("{ROW}: setup: the snapshot: {e}"))?;
        let head = frames
            .first()
            .map(WatchFrame::to_value)
            .ok_or_else(|| format!("{ROW}: setup: no snapshot"))?;
        let Some(Json::List(cs)) = head.get("view").and_then(|v| v.get("containers")) else {
            return Err(format!(
                "{ROW}: setup: {name}: no containers in the snapshot"
            ));
        };
        for c in &ix.containers {
            let on_wire = cs
                .iter()
                .find(|x| x.get("key").and_then(Json::as_str) == Some(c.key.as_str()))
                .and_then(|x| x.get("parent"))
                .map(Json::to_json);
            let want = c.parent.as_ref().map(|p| Json::from(p.as_str()).to_json());
            step(on_wire == want, ROW, "snapshot", || {
                format!(
                    "{name}: {}'s parent on the snapshot {on_wire:?}, want {want:?}",
                    c.key
                )
            })?;
        }
    }
    Ok(())
}

/// A collection moved into another (its `parent` set, as an outliner's drag does) has its new
/// parent in the view index, and the change frame carries it as `parent_after` on the
/// collection's delta: `Inner` from `Set` into `Props`. Control first: `Inner` is in `Set`. The
/// live pack refuses a collection's field at the call (`blend_live_change`), so the row opens
/// the scene with the kit's edit check off ([`super::unchecked::NoEditCheck`]): its subject is
/// the view's derive of the change and its frame.
pub fn row_collection_move_carries_parent_after() -> Result<(), String> {
    const ROW: &str = "row_collection_move_carries_parent_after";
    let (f, s) = super::unchecked::open_unchecked("coll-move", &scene());
    let host = f.person(&s);
    let ix = view(ROW, &s, host)?;
    let by_title = |ix: &ViewIndex, t: &str| {
        ix.containers
            .iter()
            .find(|c| c.title.as_deref() == Some(t))
            .map(|c| (c.key.clone(), c.parent.clone()))
    };
    let (inner, was) = by_title(&ix, "Inner").ok_or_else(|| format!("{ROW}: control: no Inner"))?;
    let (set, _) = by_title(&ix, "Set").ok_or_else(|| format!("{ROW}: control: no Set"))?;
    let (props, _) = by_title(&ix, "Props").ok_or_else(|| format!("{ROW}: control: no Props"))?;
    step(was.as_deref() == Some(set.as_str()), ROW, "control", || {
        format!("Inner ({inner}) is in {was:?}, not Set ({set})")
    })?;
    let mut w = keyed_watch(ROW, &s, host)?;
    let key = |addr: &str| format!("c{}", addr.trim_start_matches("@coll:"));
    let mut opts = k2_mpc_base::session::EditOptions::new();
    opts.base = Some(s.status().map_err(|e| format!("{ROW}: {e}"))?.version);
    let edit = k2_mpc_base::doc::Edit::Map {
        root: COLL_PROPS.to_owned(),
        ops: vec![k2_mpc_base::elem::MapOp::Set {
            key: format!("{}\u{1f}parent", key(&inner)),
            value: Value::str(key(&props)),
        }],
    };
    s.edit_ops(host, vec![edit], &opts)
        .map_err(|e| format!("{ROW}: move: the move of {inner} wasn't taken: {e}"))?;
    let after = view(ROW, &s, host)?;
    let now = by_title(&after, "Inner").and_then(|(_, p)| p);
    step(now.as_deref() == Some(props.as_str()), ROW, "index", || {
        format!("Inner is in {now:?} after the move, not Props ({props})")
    })?;
    let frames = change_frames(&w.try_frames());
    let carried: Vec<(String, String)> = frames
        .iter()
        .filter_map(|fr| match fr.get("containers_changed") {
            Some(Json::List(cs)) => Some(cs.clone()),
            _ => None,
        })
        .flatten()
        .filter_map(|d| {
            let k = d.get("key")?.as_str()?.to_owned();
            Some((k, d.get("parent_after")?.to_json()))
        })
        .collect();
    let want = vec![(inner.clone(), Json::from(props.as_str()).to_json())];
    step(carried == want, ROW, "frame", || {
        format!("the frame's parent_after {carried:?}, want {want:?}")
    })?;
    step(!w.needs_snapshot(), ROW, "watch", || {
        "the watch asks for a snapshot".into()
    })
}

/// Each live refusal is its registered entry: with `--json`, the result's `error` has the
/// entry's id, the code its text prints, and the address the call named; nothing applies. A
/// driven object placed, `--rot` on a quaternion rotation, an object linked from a library, an
/// address that names nothing.
pub fn row_refusal_is_its_registered_entry() -> Result<(), String> {
    const ROW: &str = "row_refusal_is_its_registered_entry";
    let (f, s) = open("registered", &scene());
    let a = f.agent(&s);
    let c = read_content(ROW, &s, a)?;
    let at = |n: &str| -> Result<String, String> { Ok(format!("@obj:{}", &obj(&c, n, ROW)?[1..])) };
    let v0 = s.status().map_err(|e| format!("{ROW}: {e}"))?.version;
    for (addr, flags, code) in [
        (at("Walker")?, vec!["--loc", "1,0,0"], "blend_driven"),
        (at("Spin")?, vec!["--rot", "0,0,1"], "blend_rotation_mode"),
        (at("Far")?, vec!["--loc", "1,0,0"], "blend_not_editable"),
        (
            "@obj:999".to_owned(),
            vec!["--loc", "1,0,0"],
            "blend_not_found",
        ),
    ] {
        let entry = errors::by_code(code).ok_or_else(|| format!("{ROW}: {code} has no entry"))?;
        let mut argv = vec!["object", "place", addr.as_str()];
        argv.extend(flags);
        argv.push("--json");
        let o = run_based(&s, a, &argv).map_err(|e| format!("{ROW}: {e}"))?;
        let r = o
            .result()
            .ok_or_else(|| format!("{ROW}: {argv:?}: --json gives no result"))?;
        let error = r.get("error").unwrap_or(&Json::Null);
        let detail = |k: &str| {
            error
                .get("details")
                .and_then(|d| d.get(k))
                .and_then(Json::as_str)
        };
        need(
            error.get("id").and_then(Json::as_str) == Some(entry.id)
                && detail("code") == Some(code)
                && detail("address") == Some(addr.as_str())
                && o.exit == entry.exit,
            ROW,
            || format!("{code}: exit {}, error {}", o.exit, error.to_json()),
        )?;
    }
    need(
        s.status().map_err(|e| format!("{ROW}: {e}"))?.version == v0,
        ROW,
        || "a refusal applied something".into(),
    )
}
