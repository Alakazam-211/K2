//! The test scenes (`testgen::scenes`), each read back through the live tier and the mesh read
//! with what it must hold counted (never a pass on nothing read). The living room is compared
//! with its layout: its live view (`tests/data/living_room_live.txt`), and the keys, meshes and
//! world matrices of its drawn objects and its cameras (`tests/data/living_room_world.k2scene`,
//! a K2SCENE1 header, decoded here). Every file is checked free of paths, user and machine
//! text.

mod live_support;

use std::collections::BTreeSet;

use k2_mpc_blend::live::model::{self, floats_text, CollKind, LiveLimits, LiveScene, Parent};
use k2_mpc_blend::meshread::check::{check, Checked};
use k2_mpc_blend::meshread::read::{decode, Decoded};
use k2_mpc_blend::meshread::{mesh_read, MeshRead, MeshReadLimits};
use k2_mpc_blend::testgen::scenes;
use k2_mpc_blend::write::decompress;
use k2_mpc_blend::ReadLimits;

fn live(bytes: &[u8]) -> LiveScene {
    model::read(bytes, &LiveLimits::default())
        .unwrap_or_else(|d| panic!("the live read declined: {}: {}", d.code, d.message))
}

fn meshes(bytes: &[u8]) -> (MeshRead, Checked) {
    let limits = MeshReadLimits::default();
    let r = mesh_read(bytes, &limits).unwrap_or_else(|e| panic!("the mesh read refused: {e}"));
    let c = check(&r.k2scene, &limits).unwrap_or_else(|e| panic!("the checker refused: {e}"));
    (r, c)
}

/// Each unit's `hidden_by` in a live session's view of `bytes`, and the key of the container
/// titled `Props`.
fn view_hidden_by(tag: &str, bytes: &[u8]) -> (Vec<(String, Option<String>)>, String) {
    let f = live_support::fx(tag, |_| {});
    f.write(bytes);
    let s = f.open();
    let host = f.person(&s);
    let ix = s
        .view_index(host)
        .unwrap_or_else(|e| panic!("the view: {e}"))
        .expect("a view index");
    let props = ix
        .containers
        .iter()
        .find(|c| c.title.as_deref() == Some("Props"))
        .map(|c| c.key.clone())
        .expect("a container titled Props");
    let units = ix
        .units
        .iter()
        .map(|u| {
            (
                u.name.clone(),
                u.derived_value("hidden_by").map(str::to_owned),
            )
        })
        .collect();
    (units, props)
}

fn decoded(bytes: &[u8]) -> Decoded {
    decode(bytes, &MeshReadLimits::default()).unwrap_or_else(|e| panic!("the decode refused: {e}"))
}

/// One object of the live view, as `tests/data/living_room_live.txt` writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ObjRow {
    key: String,
    ty: String,
    name: String,
    /// `coll` and `also` together.
    held_by: BTreeSet<String>,
    parent: String,
    xform: String,
    rotmode: String,
    quat: String,
    hidden: String,
    norender: String,
    edit: String,
}

/// One collection of the live view.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CollRow {
    key: String,
    kind: String,
    name: String,
    parent: String,
    hidden: String,
}

/// The live view's facts, collections (but `c0`) and objects, under the keys an import mints.
fn view(s: &LiveScene) -> (Vec<(String, String)>, Vec<CollRow>, Vec<ObjRow>) {
    let (ck, ok) = s.minted_keys();
    let facts = s
        .facts
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    let colls = s
        .colls
        .iter()
        .zip(&ck)
        .filter(|(c, _)| c.kind != CollKind::None)
        .map(|(c, k)| CollRow {
            key: k.as_str().to_owned(),
            kind: c.kind.as_str().to_owned(),
            name: c.name.clone(),
            parent: c
                .parent
                .map_or_else(|| "-".to_owned(), |p| ck[p].as_str().to_owned()),
            hidden: u8::from(c.hidden).to_string(),
        })
        .collect();
    let objs = s
        .objs
        .iter()
        .zip(&ok)
        .map(|(o, k)| {
            let mut held_by: BTreeSet<String> =
                o.also.iter().map(|&c| ck[c].as_str().to_owned()).collect();
            held_by.insert(ck[o.coll].as_str().to_owned());
            ObjRow {
                key: k.as_str().to_owned(),
                ty: o.kind.clone(),
                name: o.name.clone(),
                held_by,
                parent: match &o.parent {
                    Parent::None => "-".to_owned(),
                    Parent::Obj(p) => ok[*p].as_str().to_owned(),
                    Parent::Other(s) => s.clone(),
                },
                xform: o.xform.map_or_else(|| "-".to_owned(), |x| floats_text(&x)),
                rotmode: o.rotmode.clone(),
                quat: o.quat.map_or_else(|| "-".to_owned(), |q| floats_text(&q)),
                hidden: u8::from(o.hide[0]).to_string(),
                norender: u8::from(o.hide[2]).to_string(),
                edit: o.edit.as_str().to_owned(),
            }
        })
        .collect();
    (facts, colls, objs)
}

/// `tests/data/living_room_live.txt`: its facts, collections and objects.
fn layout_view() -> (Vec<(String, String)>, Vec<CollRow>, Vec<ObjRow>) {
    let text = include_str!("data/living_room_live.txt");
    let (mut facts, mut colls, mut objs) = (Vec::new(), Vec::new(), Vec::new());
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        match f[0] {
            "fact" => facts.push((f[1].to_owned(), f[2].to_owned())),
            "coll" => colls.push(CollRow {
                key: f[1].into(),
                kind: f[2].into(),
                name: f[3].into(),
                parent: f[4].into(),
                hidden: f[5].into(),
            }),
            "obj" => {
                let mut held_by = BTreeSet::from([f[4].to_owned()]);
                if f[5] != "-" {
                    held_by.extend(f[5].split(',').map(str::to_owned));
                }
                objs.push(ObjRow {
                    key: f[1].into(),
                    ty: f[2].into(),
                    name: f[3].into(),
                    held_by,
                    parent: f[6].into(),
                    xform: f[7].into(),
                    rotmode: f[8].into(),
                    quat: f[9].into(),
                    hidden: f[10].into(),
                    norender: f[11].into(),
                    edit: f[12].into(),
                });
            }
            other => panic!("tests/data/living_room_live.txt: an unknown row {other}"),
        }
    }
    (facts, colls, objs)
}

/// One object of `tests/data/living_room_world.k2scene`: its id, key, mesh index and world
/// matrix.
struct WorldRow {
    id: String,
    key: String,
    mesh: u32,
    transform: [f32; 16],
}

/// One camera of `tests/data/living_room_world.k2scene`.
struct WorldCam {
    id: String,
    key: String,
    yfov: f32,
    aspect: f32,
    active: bool,
}

/// The text of field `name` in a JSON object written on one line: a string without its quotes,
/// an array with its brackets, or a number or word.
fn json_field<'a>(line: &'a str, name: &str) -> &'a str {
    let pat = format!("\"{name}\":");
    let at = line
        .find(&pat)
        .unwrap_or_else(|| panic!("no {name} in {line}"))
        + pat.len();
    let rest = &line[at..];
    if let Some(s) = rest.strip_prefix('"') {
        return &s[..s.find('"').expect("the string's end")];
    }
    if rest.starts_with('[') {
        return &rest[..=rest.find(']').expect("the array's end")];
    }
    &rest[..rest.find([',', '}']).expect("the value's end")]
}

/// The K2SCENE1 header of `tests/data/living_room_world.k2scene`: the magic, the header's
/// length (u32, little-endian) and its JSON, one object or camera per line.
fn layout_world() -> (Vec<WorldRow>, Vec<WorldCam>) {
    let bytes = include_bytes!("data/living_room_world.k2scene");
    assert_eq!(&bytes[..8], b"K2SCENE1", "the world file's magic");
    let len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    assert_eq!(len, bytes.len() - 12, "the world file's header length");
    let json = std::str::from_utf8(&bytes[12..]).expect("the header is UTF-8");
    let (mut objects, mut cameras) = (Vec::new(), Vec::new());
    let mut in_cameras = false;
    for line in json.lines() {
        if line.contains("\"cameras\":[") {
            in_cameras = true;
            continue;
        }
        if !line.starts_with("{\"id\":") {
            continue;
        }
        let (id, key) = (json_field(line, "id"), json_field(line, "key"));
        if in_cameras {
            cameras.push(WorldCam {
                id: id.to_owned(),
                key: key.to_owned(),
                yfov: json_field(line, "yfov").parse().expect("a number"),
                aspect: json_field(line, "aspect").parse().expect("a number"),
                active: json_field(line, "active") == "true",
            });
            continue;
        }
        let t: Vec<f32> = json_field(line, "transform")
            .trim_matches(['[', ']'])
            .split(',')
            .map(|x| x.parse().expect("a number"))
            .collect();
        objects.push(WorldRow {
            id: id.to_owned(),
            key: key.to_owned(),
            mesh: json_field(line, "mesh").parse().expect("a mesh index"),
            transform: t.try_into().expect("16 numbers"),
        });
    }
    (objects, cameras)
}

/// Within f32 rounding of `want`.
fn close(got: f32, want: f32) -> bool {
    (got - want).abs() <= 1e-5 * want.abs().max(1.0)
}

#[test]
fn living_room_live_view_is_the_layouts() {
    // the layout is stored zstd; the plain file is the same but for its storage
    let zstd = live(&scenes::living_room_zstd());
    let s = live(&scenes::living_room());
    let (facts, colls, objs) = view(&s);
    let (zfacts, zcolls, zobjs) = view(&zstd);
    let (want_facts, want_colls, want_objs) = layout_view();
    assert_eq!(want_facts.len(), 7, "the layout's facts");
    for (k, v) in &want_facts {
        let got = zfacts.iter().find(|(f, _)| f == k).map(|(_, v)| v.as_str());
        assert_eq!(got, Some(v.as_str()), "fact {k}");
        let plain = if k == "storage" { "none" } else { v.as_str() };
        let got = facts.iter().find(|(f, _)| f == k).map(|(_, v)| v.as_str());
        assert_eq!(got, Some(plain), "fact {k}, stored plain");
    }
    assert_eq!((&zcolls, &zobjs), (&colls, &objs), "zstd and plain");
    assert_eq!(colls, want_colls, "the collections");
    assert_eq!(objs.len(), 26, "the objects");
    for (got, want) in objs.iter().zip(&want_objs) {
        assert_eq!(got, want, "object {}", want.key);
    }
    assert_eq!(objs, want_objs, "every object");
    // the measure guide is in no collection: c0, last
    let none = s.colls.last().expect("a collection");
    assert_eq!(
        none.kind,
        CollKind::None,
        "c0 for the object no collection holds"
    );
    let edits: BTreeSet<&str> = objs.iter().map(|o| o.edit.as_str()).collect();
    assert_eq!(
        edits,
        BTreeSet::from(["0", "1", "a", "t"]),
        "every edit mode"
    );
    println!(
        "living room live: {} facts, {} collections, {} objects compared",
        want_facts.len(),
        colls.len(),
        objs.len()
    );
}

#[test]
fn living_room_meshes_are_the_layouts() {
    let (r, c) = meshes(&scenes::living_room());
    let (rz, _) = meshes(&scenes::living_room_zstd());
    assert_eq!(rz.k2scene, r.k2scene, "zstd and plain draw the same");
    let (world, world_cams) = layout_world();
    assert_eq!(world.len(), 18, "the layout's drawn objects");
    assert_eq!(r.report.scene.as_deref(), Some("Scene"));
    assert_eq!(r.report.scene_from, "current");
    assert_eq!(r.report.meshes, 14, "meshes");
    assert_eq!(c.scene.meshes.len(), 14, "meshes written");
    // the 18 the layout draws, and the hidden left wall
    assert_eq!(r.report.objects, 19, "objects");
    assert_eq!(c.scene.objects.len(), 19, "objects written");
    assert!(r.report.skipped.is_empty(), "{:?}", r.report.skipped);
    let mut ids: BTreeSet<&str> = world.iter().map(|w| w.id.as_str()).collect();
    ids.insert("Wall.Left");
    let got: BTreeSet<&str> = c.scene.objects.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(got, ids, "the objects drawn");
    let hidden: Vec<(&str, &str)> = r
        .report
        .drawn
        .iter()
        .filter_map(|d| d.hidden.map(|h| (d.name.as_str(), h)))
        .collect();
    assert_eq!(hidden, vec![("Wall.Left", "self")], "hidden");
    let wall = r
        .report
        .drawn
        .iter()
        .find(|d| d.name == "Wall.Left")
        .unwrap();
    assert_eq!(wall.key, "@obj:6", "the hidden wall's key");
    // shared meshes hash alike: the four chairs, the two books, the two walls
    let hash = |n: &str| {
        r.report
            .drawn
            .iter()
            .find(|d| d.name == n)
            .map(|d| d.mesh_hash.clone())
            .unwrap_or_else(|| panic!("{n} is drawn"))
    };
    for group in [
        &["Chair.001", "Chair.002", "Chair.003", "Chair.004"][..],
        &["Book.001", "Book.002"],
        &["Wall.Back", "Wall.Left"],
    ] {
        let hashes: BTreeSet<String> = group.iter().map(|n| hash(n)).collect();
        assert_eq!(hashes.len(), 1, "{group:?} share one mesh hash: {hashes:?}");
        assert!(!hash(group[0]).is_empty());
    }
    let distinct: BTreeSet<&str> = r
        .report
        .drawn
        .iter()
        .map(|d| d.mesh_hash.as_str())
        .collect();
    assert_eq!(distinct.len(), 14, "one hash per mesh");
    // the cameras: the scene's active, both 16:9
    assert_eq!(world_cams.len(), 2, "the layout's cameras");
    assert_eq!(c.scene.cameras.len(), 2, "cameras written");
    assert!(c.scene.cameras_left_out.is_empty(), "no camera left out");
    assert_eq!(r.report.cameras.len(), 2, "cameras reported");
    for w in &world_cams {
        let cam = c
            .scene
            .cameras
            .iter()
            .find(|k| k.key == w.key)
            .unwrap_or_else(|| panic!("camera {} is written", w.key));
        assert_eq!(cam.id, w.id, "{}'s id", w.key);
        assert!(
            (cam.yfov - w.yfov).abs() <= 1e-4,
            "{}'s vertical field of view {} against {}",
            w.key,
            cam.yfov,
            w.yfov
        );
        let aspect = cam.aspect.expect("an aspect");
        assert!((aspect - 16.0 / 9.0).abs() <= 1e-6, "{}: {aspect}", w.key);
        assert!(close(aspect, w.aspect), "{}: {aspect}", w.key);
        assert_eq!(cam.active, Some(w.active), "{} active", w.key);
        let rc = r.report.cameras.iter().find(|k| k.key == w.key).unwrap();
        assert_eq!((rc.active, rc.sensor_fit), (w.active, "auto"), "{}", w.key);
    }
    assert_eq!(r.report.hidden, 1);
    let mut compared = 0;
    for w in &world {
        let o = c
            .scene
            .objects
            .iter()
            .find(|o| o.id == w.id)
            .unwrap_or_else(|| panic!("{} is drawn", w.id));
        assert_eq!(o.key, w.key, "{}'s key", w.id);
        assert_eq!(o.mesh, w.mesh, "{}'s mesh", w.id);
        for (i, (g, e)) in o.transform.iter().zip(&w.transform).enumerate() {
            assert!(
                close(*g, *e),
                "{}'s world matrix [{i}]: {g} against {e}: {:?}",
                w.id,
                o.transform
            );
            compared += 1;
        }
    }
    assert_eq!(compared, 18 * 16);
    let sofa = r.report.drawn.iter().find(|d| d.name == "Sofa").unwrap();
    assert_eq!(
        sofa.modifiers,
        vec!["Subsurf".to_owned()],
        "the sofa's modifier"
    );
    let hand = r
        .report
        .drawn
        .iter()
        .find(|d| d.name == "Clock hand")
        .unwrap();
    assert!(hand.transform_not_evaluated, "the clock's hand is driven");
    assert_eq!(r.report.transform_not_evaluated, 1);
    println!(
        "living room meshes: {} objects, {} meshes, {} cameras, {} distinct mesh hashes, {} \
         world numbers compared",
        r.report.objects,
        r.report.meshes,
        c.scene.cameras.len(),
        distinct.len(),
        compared
    );
}

#[test]
fn living_room_cameras_render_and_library() {
    let d = decoded(&scenes::living_room());
    let cams = d.model.cameras();
    assert_eq!(cams.len(), 2, "cameras");
    let names: Vec<&str> = cams.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Camera", "Camera.001"]);
    assert_eq!(cams[0].active_in, ["@sc:Scene"], "the scene's camera");
    assert!(cams[1].active_in.is_empty());
    for cam in &cams {
        let field = |n: &str| {
            cam.fields
                .iter()
                .find(|(k, _)| k == n)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(field("lens"), Some("50"), "{} lens", cam.name);
        assert_eq!(field("sensor_x"), Some("36"), "{} sensor width", cam.name);
        assert_eq!(field("sensor_y"), Some("24"), "{} sensor height", cam.name);
        assert_eq!(field("clipsta"), Some("0.1"), "{} clip start", cam.name);
        assert_eq!(field("camera_type"), Some("perspective"), "{}", cam.name);
    }
    let file = &d.file;
    let e = file.endian();
    let at = |key: &str, path: &str| {
        let id = d.model.by_key(key).unwrap_or_else(|| panic!("{key}"));
        let s = file.blocks[id.block as usize].head.sdna;
        let (off, _) = file
            .sdna
            .field_path(s, path)
            .unwrap_or_else(|| panic!("{key} has {path}"));
        (file.body(id.block).expect("a kept body"), off as usize)
    };
    let (b, o) = at("@sc:Scene", "r.xsch");
    assert_eq!(e.i32(b, o), Some(1920), "render width");
    let (b, o) = at("@sc:Scene", "r.ysch");
    assert_eq!(e.i32(b, o), Some(1080), "render height");
    let (b, o) = at("@sc:Scene", "r.xasp");
    assert_eq!(e.f32(b, o), Some(1.0), "pixel aspect x");
    let (b, o) = at("@sc:Scene", "r.yasp");
    assert_eq!(e.f32(b, o), Some(1.0), "pixel aspect y");
    for cam in ["@ca:Camera", "@ca:Camera.001"] {
        let (b, o) = at(cam, "sensor_fit");
        assert_eq!(b[o], 0, "{cam}: the sensor fitted automatically");
    }
    let (b, o) = at("@li:plants.blend", "filepath");
    assert!(
        b[o..].starts_with(b"//plants.blend\0"),
        "the library's path is relative"
    );
    let plant = d
        .model
        .by_key("@li:plants.blend/ob:Plant")
        .expect("the linked plant");
    assert!(plant.library.is_some());
    println!("living room: {} cameras, render 1920x1080", cams.len());
}

#[test]
fn living_room_hidden_by_self_and_by_collection() {
    let bytes = scenes::living_room_hidden();
    let s = live(&bytes);
    let (_, colls, objs) = view(&s);
    let props = colls.iter().find(|c| c.name == "Props").unwrap();
    assert_eq!(props.hidden, "1", "Props hidden");
    assert_eq!(
        colls.iter().filter(|c| c.hidden == "1").count(),
        1,
        "only Props"
    );
    let wall = objs.iter().find(|o| o.name == "Wall.Left").unwrap();
    assert_eq!(wall.hidden, "1");
    assert_eq!(objs.len(), 26);
    let (r, c) = meshes(&bytes);
    assert_eq!(c.scene.objects.len(), 19, "objects written");
    let hidden: Vec<(&str, &str)> = r
        .report
        .drawn
        .iter()
        .filter_map(|d| d.hidden.map(|h| (d.name.as_str(), h)))
        .collect();
    assert_eq!(
        hidden,
        vec![
            ("Wall.Left", "self"),
            ("Vase", "collection"),
            ("Mug", "collection"),
            ("Clock hand", "collection"),
        ]
    );
    assert_eq!(r.report.hidden, 4);
    assert_eq!(r.report.meshes, 14);
    // the live view's hidden_by: the wall by itself, Props's objects by Props
    let (units, props) = view_hidden_by("scenes-hidden", &bytes);
    assert_eq!(props, "@coll:4", "Props's key");
    let by = format!("collection:{props}");
    let in_props = ["Vase", "Mug", "Clock", "Clock hand", "Plant"];
    assert_eq!(units.len(), 26, "every object a unit");
    let mut marked = 0;
    for (name, h) in &units {
        let want = if name == "Wall.Left" {
            Some("self")
        } else if in_props.contains(&name.as_str()) {
            Some(by.as_str())
        } else {
            None
        };
        assert_eq!(h.as_deref(), want, "{name}'s hidden_by");
        marked += usize::from(h.is_some());
    }
    assert_eq!(marked, 1 + in_props.len());
    let (plain, _) = view_hidden_by("scenes-shown", &scenes::living_room());
    let shown: Vec<(&str, &str)> = plain
        .iter()
        .filter_map(|(n, h)| h.as_deref().map(|h| (n.as_str(), h)))
        .collect();
    assert_eq!(shown, [("Wall.Left", "self")], "the room shown");
    println!(
        "living room hidden: {} objects, {} hidden; the view: {} units, {} hidden by",
        r.report.objects,
        r.report.hidden,
        units.len(),
        marked
    );
}

#[test]
fn three_cameras_read_back() {
    let bytes = scenes::three_cameras();
    let s = live(&bytes);
    let kinds: Vec<&str> = s.objs.iter().map(|o| o.kind.as_str()).collect();
    assert_eq!(kinds, ["camera", "camera", "camera", "camera", "mesh"]);
    let d = decoded(&bytes);
    let cams = d.model.cameras();
    assert_eq!(cams.len(), 4, "cameras");
    let got: Vec<(&str, Option<&str>, Option<&str>, usize)> = cams
        .iter()
        .map(|c| {
            let f = |n: &str| {
                c.fields
                    .iter()
                    .find(|(k, _)| k == n)
                    .map(|(_, v)| v.as_str())
            };
            (
                c.name.as_str(),
                f("lens"),
                f("camera_type"),
                c.active_in.len(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("Camera.Wide", Some("24"), Some("perspective"), 0),
            ("Camera.Normal", Some("50"), Some("perspective"), 1),
            ("Camera.Long", Some("85"), Some("perspective"), 0),
            ("Camera.Top", Some("50"), Some("orthographic"), 0),
        ]
    );
    let (r, c) = meshes(&bytes);
    assert_eq!((r.report.objects, r.report.meshes), (1, 1));
    assert_eq!(c.scene.objects[0].id, "Box");
    // the mesh read writes the perspective cameras and names the orthographic one left out
    let written: Vec<(&str, bool)> = r
        .report
        .cameras
        .iter()
        .map(|k| (k.name.as_str(), k.active))
        .collect();
    assert_eq!(
        written,
        [
            ("Camera.Wide", false),
            ("Camera.Normal", true),
            ("Camera.Long", false),
        ]
    );
    assert_eq!(c.scene.cameras.len(), 3, "cameras written");
    let left: Vec<(&str, &str)> = r
        .report
        .skipped
        .iter()
        .map(|s| (s.name.as_str(), s.reason))
        .collect();
    assert_eq!(left, [("Camera.Top", "camera_not_perspective")]);
    let listed: Vec<(&str, Option<&str>, &str)> = c
        .scene
        .cameras_left_out
        .iter()
        .map(|l| (l.name.as_str(), l.key.as_deref(), l.reason.as_str()))
        .collect();
    assert_eq!(
        listed,
        [("Camera.Top", Some("@cam:4"), "camera_not_perspective")],
        "the K2SCENE1 lists the camera left out"
    );
    println!(
        "three cameras: {} cameras, {} written, {} mesh",
        cams.len(),
        written.len(),
        r.report.meshes
    );
}

#[test]
fn driven_quat_read_back() {
    let bytes = scenes::driven_quat();
    let s = live(&bytes);
    let (_, _, objs) = view(&s);
    let got: Vec<(&str, &str, &str)> = objs
        .iter()
        .map(|o| (o.name.as_str(), o.edit.as_str(), o.rotmode.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("Camera", "1", "xyz"),
            ("Driven", "a", "xyz"),
            ("Turned", "t", "quaternion"),
            ("Lamp, B: 2", "1", "xyz"),
        ]
    );
    assert_eq!(objs[2].quat, "0.70710677,0,0,0.70710677");
    let (r, c) = meshes(&bytes);
    assert_eq!((r.report.objects, r.report.meshes), (3, 1));
    let flags: Vec<(&str, bool)> = r
        .report
        .drawn
        .iter()
        .map(|d| (d.name.as_str(), d.transform_not_evaluated))
        .collect();
    assert_eq!(
        flags,
        [("Driven", true), ("Turned", false), ("Lamp, B: 2", false)]
    );
    // a quarter turn about Z: X goes to Y, Y to -X
    let t = c.scene.objects[1].transform;
    let want = [0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    for (i, (g, w)) in t.iter().zip(want).enumerate() {
        assert!((g - w).abs() < 1e-6, "Turned [{i}]: {t:?}");
    }
    // a name with a comma and a colon reads back as it is
    let lamp = &s.objs[3];
    assert_eq!(
        (lamp.name.as_str(), lamp.nid.as_str()),
        ("Lamp, B: 2", "@ob:Lamp,%20B:%202")
    );
    let drawn = &c.scene.objects[2];
    assert_eq!(
        (drawn.id.as_str(), drawn.key.as_str()),
        ("Lamp, B: 2", "@obj:4")
    );
    println!("driven and turned: {} objects", r.report.objects);
}

#[test]
fn instanced_shares_one_mesh() {
    let bytes = scenes::instanced();
    let s = live(&bytes);
    let shared = s
        .objs
        .iter()
        .filter(|o| o.data == "mesh @me:Shared")
        .count();
    assert_eq!(shared, 4, "objects on the shared mesh");
    // a collection name with a comma and a colon reads back as it is, holding `Own`
    let names: Vec<&str> = s.colls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Scene", "Copies", "Props, A: B"]);
    let own = s.objs.iter().find(|o| o.name == "Own").expect("Own");
    assert_eq!(s.colls[own.coll].name, "Props, A: B");
    assert_eq!(s.colls[own.coll].nid, "@gr:Props,%20A:%20B");
    let (r, c) = meshes(&bytes);
    assert_eq!((r.report.objects, r.report.meshes), (5, 2));
    let got: Vec<(&str, u32)> = c
        .scene
        .objects
        .iter()
        .map(|o| (o.id.as_str(), o.mesh))
        .collect();
    assert_eq!(
        got,
        [
            ("Copy.001", 0),
            ("Copy.002", 0),
            ("Copy.003", 0),
            ("Copy.004", 0),
            ("Own", 1),
        ]
    );
    println!(
        "instanced: {} objects, {} meshes",
        r.report.objects, r.report.meshes
    );
}

#[test]
fn large_reads_back_whole() {
    let bytes = scenes::large();
    let s = live(&bytes);
    let tops = scenes::LARGE_TOP;
    let colls = tops * (1 + scenes::LARGE_CHILDREN);
    assert_eq!(colls, 100);
    // the scene, then every collection; no c0
    assert_eq!(s.colls.len(), 1 + colls, "collections");
    assert!(s.colls.iter().all(|c| c.kind != CollKind::None));
    let under_scene = s.colls.iter().filter(|c| c.parent == Some(0)).count();
    assert_eq!(under_scene, tops, "top collections under the scene");
    let nested = s
        .colls
        .iter()
        .filter(|c| c.parent.is_some_and(|p| p != 0))
        .count();
    assert_eq!(nested, tops * scenes::LARGE_CHILDREN, "child collections");
    let items = colls * scenes::LARGE_PER_COLLECTION;
    assert_eq!(items, 2000);
    assert_eq!(s.objs.len(), items + 1, "the objects and the camera");
    let (r, c) = meshes(&bytes);
    assert_eq!(r.report.objects as usize, items, "objects drawn");
    assert_eq!(c.scene.objects.len(), items);
    assert_eq!(r.report.meshes as usize, scenes::LARGE_MESHES, "meshes");
    assert!(r.report.skipped.is_empty());
    println!(
        "large: {} collections, {} objects, {} meshes",
        s.colls.len(),
        r.report.objects,
        r.report.meshes
    );
}

#[test]
fn empty_scene_has_no_mesh() {
    let bytes = scenes::empty_scene();
    let s = live(&bytes);
    let kinds: Vec<&str> = s.objs.iter().map(|o| o.kind.as_str()).collect();
    assert_eq!(kinds, ["camera", "empty"]);
    let d = decoded(&bytes);
    assert_eq!(d.model.cameras().len(), 1, "cameras");
    let (r, c) = meshes(&bytes);
    assert_eq!(r.report.scene.as_deref(), Some("Scene"));
    assert_eq!((r.report.objects, r.report.meshes), (0, 0));
    assert!(c.scene.objects.is_empty() && c.scene.meshes.is_empty());
    println!("empty scene: {} objects, 0 meshes", s.objs.len());
}

/// Text that names a place on one machine, or a user.
const NOT_NEUTRAL: &[&[u8]] = &[
    b"/Users/",
    b"/home/",
    b"/root/",
    b"/tmp/",
    b"/var/",
    b"/private/",
    b"/Volumes/",
    b"/mnt/",
    b"C:\\",
    b"C:/",
    b"\\Users\\",
];

#[test]
fn scenes_hold_no_path_user_or_machine_text() {
    let names: Vec<&str> = scenes::ALL.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        [
            "living_room.blend",
            "living_room_zstd.blend",
            "living_room_hidden.blend",
            "three_cameras.blend",
            "driven_quat.blend",
            "instanced.blend",
            "large.blend",
            "empty_scene.blend",
        ]
    );
    let mut scanned = 0usize;
    for (name, write) in scenes::ALL {
        let stored = write();
        let (_, raw) =
            decompress(&stored, &ReadLimits::default()).unwrap_or_else(|e| panic!("{name}: {e}"));
        for bytes in [&stored, &raw] {
            assert!(!bytes.is_empty(), "{name}");
            for p in NOT_NEUTRAL {
                let found = bytes.windows(p.len()).position(|w| w == *p);
                assert_eq!(found, None, "{name} holds {:?}", String::from_utf8_lossy(p));
            }
            scanned += bytes.len();
        }
    }
    println!("{} files, {scanned} bytes scanned", scenes::ALL.len());
}

#[test]
fn scenes_are_the_same_bytes_every_time() {
    for (name, write) in scenes::ALL {
        let (a, b) = (write(), write());
        assert!(!a.is_empty(), "{name}");
        assert!(a == b, "{name} differs between two writes");
    }
    println!("{} files written twice, the same", scenes::ALL.len());
}
