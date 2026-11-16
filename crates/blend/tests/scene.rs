//! The scene model on files built here (`k2_mpc_blend::testgen::SceneBuilder`, a schema of ours
//! shaped like the files' own): what each kind reads, in the modern and the older shape and in
//! every header form; keys, their escaping and what a rename does to them; content digests that
//! survive a save that moves everything in memory and change with what a person changed; and
//! refusals.

use k2_mpc_blend::scene::{escape_name, key_of, kind_of};
use k2_mpc_blend::testgen::{FileBuilder, ObjectSpec, SceneBuilder};
use k2_mpc_blend::{read_blend, read_scene, Detail, HeaderForm, ReadError, ReadLimits, SceneModel};

fn spec(data: Option<&str>) -> ObjectSpec {
    ObjectSpec {
        data: data.map(str::to_owned),
        ..ObjectSpec::default()
    }
}

/// A small studio: a scene with a camera object and two collections (one nested, one object in
/// two), objects with data, a parent, material slots; a library and a material linked from it.
fn studio(b: &mut SceneBuilder, cube_loc: [f32; 3]) {
    b.ui();
    b.library("lib", "//assets/lib.blend");
    b.scene("Scene", &["Camera"], &["Set", "Props"], Some("Camera"));
    b.collection("Set", &["Cube", "Light"], &["Inner"]);
    b.collection("Inner", &["Child"], &[]);
    b.collection("Props", &["Cube"], &[]);
    b.object(
        "Camera",
        &ObjectSpec {
            loc: [1.0, 2.0, 3.0],
            ..spec(Some("CA:Cam"))
        },
    );
    b.object(
        "Cube",
        &ObjectSpec {
            loc: cube_loc,
            rot: [0.5, 0.0, 0.0],
            scale: [2.0, 2.0, 2.0],
            materials: vec![Some("Red".into()), None],
            ..spec(Some("ME:Cube"))
        },
    );
    b.object(
        "Child",
        &ObjectSpec {
            parent: Some("Cube".into()),
            restrict: 5,
            ..spec(None)
        },
    );
    b.object("Light", &spec(Some("LA:Lamp")));
    b.camera("Cam", 50.0);
    b.light("Lamp", 10.0);
    b.mesh("Cube", &[Some("Blue".into())], 4);
    b.material("Red", [1.0, 0.0, 0.0], None);
    b.material("Blue", [0.0, 0.0, 1.0], None);
    b.material("Shared", [0.5, 0.5, 0.5], Some("lib"));
    b.world("World");
}

fn model_of(bytes: &[u8]) -> SceneModel {
    let f = read_blend(&mut &bytes[..], &ReadLimits::default()).expect("check: the file reads");
    read_scene(&f).expect("check: the scene reads")
}

fn at(m: &SceneModel, key: &str) -> usize {
    m.ids
        .iter()
        .position(|i| i.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("no data-block {key}"))
}

fn field<'a>(m: &'a SceneModel, key: &str, name: &str) -> Option<&'a str> {
    m.ids[at(m, key)]
        .fields
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// Everything the model says but the digests and file places, keyed, for comparing files.
fn projection(m: &SceneModel) -> Vec<String> {
    let key = |i: usize| m.ids[i].key.clone().unwrap_or_default();
    let keys = |v: &[usize]| v.iter().map(|&i| key(i)).collect::<Vec<_>>().join(" ");
    m.ids
        .iter()
        .map(|i| {
            let detail = match &i.detail {
                Detail::Scene {
                    objects,
                    collections,
                    camera,
                    world,
                } => format!(
                    "scene objects[{}] collections[{}] camera {:?} world {:?}",
                    keys(objects),
                    keys(collections),
                    camera.map(key),
                    world.map(key)
                ),
                Detail::Collection {
                    objects,
                    collections,
                } => format!(
                    "objects[{}] collections[{}]",
                    keys(objects),
                    keys(collections)
                ),
                Detail::Object { data, parent } => {
                    format!("data {:?} parent {:?}", data.map(key), parent.map(key))
                }
                Detail::Library { file } => format!("file {file:?}"),
                _ => "other".into(),
            };
            let mats: Vec<String> = i
                .materials
                .iter()
                .map(|s| s.map(key).unwrap_or_else(|| "-".into()))
                .collect();
            format!(
                "{:?} {} {} {:?} lib {:?} ui {} | {} | {:?} | mats {:?}",
                i.key,
                i.code,
                i.kind,
                i.name,
                i.library.map(key),
                i.ui,
                detail,
                i.fields,
                mats
            )
        })
        .collect()
}

#[test]
fn every_kind_reads_what_it_holds() {
    let mut b = SceneBuilder::modern();
    studio(&mut b, [0.0, 0.0, 0.25]);
    let m = model_of(&b.bytes());
    assert_eq!(m.unresolved, 0);
    let kinds: Vec<(&str, &str)> = m
        .ids
        .iter()
        .map(|i| (i.code.as_str(), i.kind.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("WM", "wmwindowmanager"),
            ("LI", "library"),
            ("SC", "scene"),
            ("GR", "collection"),
            ("GR", "collection"),
            ("GR", "collection"),
            ("OB", "object"),
            ("OB", "object"),
            ("OB", "object"),
            ("OB", "object"),
            ("CA", "camera"),
            ("LA", "light"),
            ("ME", "mesh"),
            ("MA", "material"),
            ("MA", "material"),
            ("MA", "material"),
            ("WO", "world"),
        ]
    );
    assert!(m.ids[0].ui, "the window manager is the program's own");
    assert!(m.ids[1..].iter().all(|i| !i.ui));
    // the scene: its master collection's objects and collections, its camera
    match &m.ids[at(&m, "@sc:Scene")].detail {
        Detail::Scene {
            objects,
            collections,
            camera,
            world,
        } => {
            assert_eq!(objects, &[at(&m, "@ob:Camera")]);
            assert_eq!(collections, &[at(&m, "@gr:Set"), at(&m, "@gr:Props")]);
            assert_eq!(*camera, Some(at(&m, "@ob:Camera")));
            assert_eq!(*world, None);
        }
        other => panic!("{other:?}"),
    }
    match &m.ids[at(&m, "@gr:Set")].detail {
        Detail::Collection {
            objects,
            collections,
        } => {
            assert_eq!(objects, &[at(&m, "@ob:Cube"), at(&m, "@ob:Light")]);
            assert_eq!(collections, &[at(&m, "@gr:Inner")]);
        }
        other => panic!("{other:?}"),
    }
    // objects: data, parent, transform, flags, material slots
    match &m.ids[at(&m, "@ob:Cube")].detail {
        Detail::Object { data, parent } => {
            assert_eq!((*data, *parent), (Some(at(&m, "@me:Cube")), None));
        }
        other => panic!("{other:?}"),
    }
    match &m.ids[at(&m, "@ob:Child")].detail {
        Detail::Object { data, parent } => {
            assert_eq!((*data, *parent), (None, Some(at(&m, "@ob:Cube"))));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(field(&m, "@ob:Cube", "loc"), Some("0,0,0.25"));
    assert_eq!(field(&m, "@ob:Cube", "rot"), Some("0.5,0,0"));
    assert_eq!(field(&m, "@ob:Cube", "size"), Some("2,2,2"));
    assert_eq!(field(&m, "@ob:Cube", "quat"), Some("1,0,0,0"));
    assert_eq!(field(&m, "@ob:Child", "restrictflag"), Some("5"));
    assert_eq!(
        field(&m, "@ob:Cube", "visibility_flag"),
        None,
        "a field the file lacks is left out"
    );
    assert_eq!(
        m.ids[at(&m, "@ob:Cube")].materials,
        vec![Some(at(&m, "@ma:Red")), None]
    );
    assert_eq!(
        m.ids[at(&m, "@me:Cube")].materials,
        vec![Some(at(&m, "@ma:Blue"))]
    );
    // cameras, lights, materials: their own fields, named as the file names them
    assert_eq!(field(&m, "@ca:Cam", "lens"), Some("50"));
    assert_eq!(field(&m, "@ca:Cam", "clipsta"), Some("0.1"));
    assert_eq!(field(&m, "@ca:Cam", "clipend"), Some("100"));
    assert_eq!(field(&m, "@ca:Cam", "sensor_x"), Some("36"));
    assert_eq!(field(&m, "@la:Lamp", "energy"), Some("10"));
    assert_eq!(field(&m, "@la:Lamp", "g"), Some("0.5"));
    assert_eq!(field(&m, "@la:Lamp", "type"), Some("2"));
    assert_eq!(field(&m, "@ma:Blue", "b"), Some("1"));
    // a library, and a material linked from it: scoped by the library
    assert_eq!(
        m.ids[at(&m, "@li:lib")].detail,
        Detail::Library {
            file: Some("lib.blend".into())
        },
        "the file's name, never its folders"
    );
    let shared = at(&m, "@li:lib/ma:Shared");
    assert_eq!(m.ids[shared].library, Some(at(&m, "@li:lib")));
    assert_eq!(m.ids[shared].name, "Shared");
}

#[test]
fn field_spans_point_at_the_fields_in_the_file() {
    let mut b = SceneBuilder::modern();
    studio(&mut b, [0.0, 0.0, 0.25]);
    let bytes = b.bytes();
    let m = model_of(&bytes);
    let cube = &m.ids[at(&m, "@ob:Cube")];
    let span = |name: &str| {
        cube.spans
            .iter()
            .find(|s| s.field == name)
            .unwrap_or_else(|| panic!("no span {name}"))
    };
    let loc = span("loc");
    assert_eq!((loc.len, loc.type_name.as_str()), (12, "float"));
    let o = loc.offset as usize;
    let z = f32::from_le_bytes(bytes[o + 8..o + 12].try_into().unwrap());
    assert_eq!(z, 0.25);
    let name = span("name");
    let o = name.offset as usize;
    assert_eq!(&bytes[o..o + 6], b"OBCube");
    assert_eq!(name.len, 66);
    let r = span("restrictflag");
    assert_eq!((r.len, r.type_name.as_str()), (2, "short"));
}

#[test]
fn every_form_reads_to_the_same_model() {
    let mut base = SceneBuilder::modern();
    studio(&mut base, [0.0, 0.0, 0.25]);
    let want = projection(&model_of(&base.bytes()));
    for (form, ps, big, version) in [
        (HeaderForm::Classic, 4, false, 403),
        (HeaderForm::Classic, 4, true, 403),
        (HeaderForm::Classic, 8, true, 306),
        (HeaderForm::Large, 8, false, 501),
    ] {
        let mut b = SceneBuilder::new(form, ps, big, version, false);
        studio(&mut b, [0.0, 0.0, 0.25]);
        assert_eq!(
            projection(&model_of(&b.bytes())),
            want,
            "{form:?} {ps} {big} {version}"
        );
    }
}

#[test]
fn the_older_shape_reads_to_the_same_scene() {
    let mut b = SceneBuilder::new(HeaderForm::Classic, 8, false, 273, true);
    b.library("lib", "C:\\assets\\lib.blend");
    b.scene("Scene", &["Camera", "Cube"], &[], Some("Camera"));
    b.collection("Set", &["Cube", "Light"], &[]);
    b.object("Camera", &spec(Some("CA:Cam")));
    b.object("Cube", &spec(Some("ME:Cube")));
    b.object("Light", &spec(Some("LA:Lamp")));
    b.camera("Cam", 35.0);
    b.light("Lamp", 1.0);
    b.mesh("Cube", &[], 3);
    let m = model_of(&b.bytes());
    assert_eq!(m.unresolved, 0);
    match &m.ids[at(&m, "@sc:Scene")].detail {
        Detail::Scene {
            objects,
            collections,
            camera,
            ..
        } => {
            assert_eq!(
                objects,
                &[at(&m, "@ob:Camera"), at(&m, "@ob:Cube")],
                "its bases"
            );
            assert!(collections.is_empty());
            assert_eq!(*camera, Some(at(&m, "@ob:Camera")));
        }
        other => panic!("{other:?}"),
    }
    let set = &m.ids[at(&m, "@gr:Set")];
    assert_eq!(
        (set.kind.as_str(), set.struct_name.as_str()),
        ("collection", "Group")
    );
    match &set.detail {
        Detail::Collection { objects, .. } => {
            assert_eq!(objects, &[at(&m, "@ob:Cube"), at(&m, "@ob:Light")])
        }
        other => panic!("{other:?}"),
    }
    let lamp = &m.ids[at(&m, "@la:Lamp")];
    assert_eq!(
        (lamp.kind.as_str(), lamp.struct_name.as_str()),
        ("light", "Lamp")
    );
    assert_eq!(field(&m, "@la:Lamp", "energy"), Some("1"));
    assert_eq!(
        m.ids[at(&m, "@li:lib")].detail,
        Detail::Library {
            file: Some("lib.blend".into())
        },
        "a path with either separator"
    );
}

#[test]
fn a_save_that_moves_everything_digests_the_same() {
    let build = |shift: u64, users: i32, cube_loc: [f32; 3]| {
        let mut b = SceneBuilder::modern();
        b.shift = shift;
        studio(&mut b, cube_loc);
        b.object(
            "Extra",
            &ObjectSpec {
                users,
                ..spec(None)
            },
        );
        b.bytes()
    };
    let first = model_of(&build(0, 1, [0.0, 0.0, 0.25]));
    // the same scene saved again: every address, session number, runtime pointer and user
    // count different
    let again_bytes = build(0x40_0000, 3, [0.0, 0.0, 0.25]);
    assert_ne!(
        again_bytes,
        build(0, 1, [0.0, 0.0, 0.25]),
        "the bytes differ"
    );
    let again = model_of(&again_bytes);
    assert_eq!(projection(&again), projection(&first));
    for (a, b) in first.ids.iter().zip(&again.ids) {
        assert_eq!(a.content, b.content, "{:?}: the same content", a.key);
        assert!(a.content_stable && b.content_stable);
    }
    // a moved cube: its content changes, and nothing else's
    let moved = model_of(&build(0x40_0000, 1, [0.0, 0.0, 9.0]));
    for (a, b) in first.ids.iter().zip(&moved.ids) {
        if a.key.as_deref() == Some("@ob:Cube") {
            assert_ne!(a.content, b.content, "the moved cube");
        } else {
            assert_eq!(a.content, b.content, "{:?}", a.key);
        }
    }
}

#[test]
fn a_change_in_a_data_block_changes_its_owners_digest() {
    let build = |verts: usize| {
        let mut b = SceneBuilder::modern();
        studio(&mut b, [0.0; 3]);
        b.mesh("Other", &[], verts);
        b.bytes()
    };
    let a = model_of(&build(4));
    let b = model_of(&build(5));
    let other_a = &a.ids[at(&a, "@me:Other")];
    let other_b = &b.ids[at(&b, "@me:Other")];
    assert_ne!(
        other_a.content, other_b.content,
        "another vertex is other content"
    );
    assert_eq!(
        a.ids[at(&a, "@me:Cube")].content,
        b.ids[at(&b, "@me:Cube")].content
    );
}

#[test]
fn a_block_too_large_to_keep_marks_a_digest_unstable_when_it_holds_pointers() {
    let mut b = SceneBuilder::modern();
    studio(&mut b, [0.0; 3]);
    let bytes = b.bytes();
    // the list elements (24 bytes, three pointers) aren't kept
    let small = ReadLimits::new(1 << 30, 100_000, 64 << 20, 16, 16 << 20, 16 << 20);
    let f = read_blend(&mut &bytes[..], &small).unwrap();
    let m = read_scene(&f).unwrap();
    assert!(
        !m.ids[at(&m, "@gr:Set")].content_stable,
        "its elements hold pointers"
    );
    // the vertex block (raw floats, 48 bytes) isn't kept either: digested as stored, stable
    assert!(m.ids[at(&m, "@me:Cube")].content_stable);
    assert!(m.ids[at(&m, "@ob:Cube")].content_stable);
}

#[test]
fn keys_escape_what_the_kit_reads_as_syntax() {
    assert_eq!(escape_name(b"Cube"), "Cube");
    assert_eq!(escape_name(b"Cube 001"), "Cube%20001");
    assert_eq!(escape_name(b"a/b%c"), "a%2Fb%25c");
    assert_eq!(escape_name(b"tab\there"), "tab%09here");
    assert_eq!(escape_name(&[b'A', 0xff, b'B']), "A%FFB");
    assert_eq!(escape_name("Würfel·1".as_bytes()), "Würfel·1");
    assert_eq!(
        escape_name("a\u{00a0}b".as_bytes()),
        "a%C2%A0b",
        "no-break space"
    );
    assert_eq!(key_of("OB", b"Cube", None).as_deref(), Some("@ob:Cube"));
    assert_eq!(
        key_of("MA", b"Steel 2", Some("lib%20a.blend")).as_deref(),
        Some("@li:lib%20a.blend/ma:Steel%202")
    );
    assert_eq!(key_of("OB", b"", None), None, "no name, no key");
    assert_eq!(
        key_of("OB", b"x", Some("")),
        None,
        "no library name, no key"
    );
    assert_eq!(
        key_of("OB", &[b' '; 255], None),
        None,
        "over the kit's limit"
    );
    assert!(key_of("OB", &[b'x'; 200], None).is_some());
    // every key parses as a native ID in the kit's grammar
    for name in [
        &b"Cube 001"[..],
        b"a/b%c",
        b"x:y",
        &[0xc3, 0x28],
        b"#3",
        b"@ob:Cube",
    ] {
        let k = key_of("OB", name, Some("lib")).unwrap();
        assert!(
            k2_mpc_base::index::NodeKey::parse(&k).is_some(),
            "{k} is a node key"
        );
    }
}

#[test]
fn flags_and_values_are_named_from_the_pinned_versions_on() {
    let mut b = SceneBuilder::modern();
    studio(&mut b, [0.0; 3]);
    b.object(
        "Odd",
        &ObjectSpec {
            restrict: 1 | 8 | 1024,
            ..spec(None)
        },
    );
    let m = model_of(&b.bytes());
    let named = |key: &str, name: &str| {
        m.ids[at(&m, key)]
            .named
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    };
    let yes = Some("true".to_owned());
    let no = Some("false".to_owned());
    assert_eq!(named("@ob:Child", "hide_viewport"), yes);
    assert_eq!(named("@ob:Child", "hide_render"), yes);
    assert_eq!(named("@ob:Child", "hide_select"), no);
    assert_eq!(named("@ob:Child", "other_bits"), None);
    assert_eq!(m.ids[at(&m, "@ob:Child")].hidden, Some(true));
    assert_eq!(m.ids[at(&m, "@ob:Cube")].hidden, Some(false));
    assert_eq!(m.ids[at(&m, "@gr:Set")].hidden, Some(false));
    assert_eq!(named("@gr:Set", "hide_render"), no);
    // the builder writes type 0 and rotmode 0
    assert_eq!(named("@ob:Cube", "object_type").as_deref(), Some("empty"));
    assert_eq!(
        named("@ob:Cube", "rotation_mode").as_deref(),
        Some("quaternion")
    );
    assert_eq!(named("@la:Lamp", "light_type").as_deref(), Some("spot"));
    assert_eq!(
        named("@ca:Cam", "camera_type").as_deref(),
        Some("perspective")
    );
    // bits no table names stay a number
    assert_eq!(named("@ob:Odd", "other_bits").as_deref(), Some("1032"));
    assert_eq!(m.ids[at(&m, "@ob:Odd")].hidden, Some(true));
    assert_eq!(field(&m, "@ob:Odd", "restrictflag"), Some("1033"));

    // a file older than the pinned versions: numbers only, nothing hidden
    let mut old = SceneBuilder::new(HeaderForm::Classic, 8, false, 305, false);
    studio(&mut old, [0.0; 3]);
    let m = model_of(&old.bytes());
    assert!(m
        .ids
        .iter()
        .all(|i| i.named.is_empty() && i.hidden.is_none()));
    assert_eq!(field(&m, "@ob:Child", "restrictflag"), Some("5"));

    // a file newer than the pinned versions: numbers only, nothing hidden, until saves pin them
    for version in [502, 503] {
        let mut new = SceneBuilder::new(HeaderForm::Classic, 8, false, version, false);
        studio(&mut new, [0.0; 3]);
        let m = model_of(&new.bytes());
        assert!(m.ids.len() > 3, "{version}: the scene read");
        assert!(
            m.ids
                .iter()
                .all(|i| i.named.is_empty() && i.hidden.is_none()),
            "{version}: a value named past the pinned versions"
        );
        assert_eq!(
            field(&m, "@ob:Child", "restrictflag"),
            Some("5"),
            "{version}"
        );
    }
}

#[test]
fn kinds_are_plain_words() {
    assert_eq!(kind_of("Lamp"), "light");
    assert_eq!(kind_of("Light"), "light");
    assert_eq!(kind_of("Group"), "collection");
    assert_eq!(kind_of("bNodeTree"), "node_tree");
    assert_eq!(kind_of("SomethingNew"), "somethingnew");
    assert_eq!(kind_of("ns::Thing"), "ns__thing");
    assert_eq!(kind_of("::"), "id");
}

#[test]
fn a_rename_is_a_new_key_and_the_old_one_is_gone() {
    let build = |name: &str| {
        let mut b = SceneBuilder::modern();
        b.scene("Scene", &[name], &[], None);
        b.object(name, &spec(Some("ME:Mesh")));
        b.mesh("Mesh", &[], 3);
        model_of(&b.bytes())
    };
    let before = build("Cube");
    let after = build("Hero");
    assert!(before.by_key("@ob:Cube").is_some());
    assert!(after.by_key("@ob:Cube").is_none(), "the old key is gone");
    let hero = after.by_key("@ob:Hero").expect("the new key");
    // what stays: the same data
    match &hero.detail {
        Detail::Object { data, .. } => {
            assert_eq!(after.ids[data.unwrap()].key.as_deref(), Some("@me:Mesh"))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn names_of_any_bytes_read() {
    let mut b = SceneBuilder::new(HeaderForm::Large, 8, false, 501, false);
    b.scene("Scene", &[], &[], None);
    b.object_named(&[b'O', 0xff, b' ', b'K'], &spec(None));
    b.object_named(&[b' '; 255], &spec(None));
    let m = model_of(&b.bytes());
    // ids: the scene, then the two objects
    let odd = &m.ids[1];
    assert_eq!(odd.name_bytes, vec![b'O', 0xff, b' ', b'K']);
    assert_eq!(odd.key.as_deref(), Some("@ob:O%FF%20K"));
    assert_eq!(odd.name, "O\u{fffd} K");
    let long = &m.ids[2];
    assert_eq!(long.name_bytes.len(), 255, "5.x names hold 255 bytes");
    assert_eq!(
        long.key, None,
        "over the kit's limit: left for the kit to mint"
    );
}

#[test]
fn a_list_that_loops_stops_where_it_loops() {
    let mut b = SceneBuilder::modern();
    studio(&mut b, [0.0; 3]);
    let first = b.addr("GR:Set/gobject/0");
    let second = b.addr("GR:Set/gobject/1");
    // the second element's next points back at the first
    let blk = b
        .file
        .blocks
        .iter_mut()
        .find(|x| x.old == second)
        .expect("the element");
    blk.body[..8].copy_from_slice(&first.to_le_bytes());
    let m = model_of(&b.bytes());
    match &m.ids[at(&m, "@gr:Set")].detail {
        Detail::Collection { objects, .. } => {
            assert_eq!(objects, &[at(&m, "@ob:Cube"), at(&m, "@ob:Light")])
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_library_pointer_that_names_no_library_is_counted_and_dropped() {
    // found by the fuzz seeds: a library pointer naming another data-block (here an object)
    let mut b = SceneBuilder::modern();
    studio(&mut b, [0.0; 3]);
    let ma = b.addr("MA:Shared");
    let ob = b.addr("OB:Cube");
    let at = b.file.schema.offset("Material", "id") + b.file.schema.offset("ID", "lib");
    let blk = b
        .file
        .blocks
        .iter_mut()
        .find(|x| x.old == ma)
        .expect("the material");
    blk.body[at..at + 8].copy_from_slice(&ob.to_le_bytes());
    let m = model_of(&b.bytes());
    let shared = m.ids.iter().find(|i| i.name == "Shared").unwrap();
    assert_eq!(shared.library, None);
    assert_eq!(shared.key.as_deref(), Some("@ma:Shared"));
    assert_eq!(m.unresolved, 1);
}

#[test]
fn a_pointer_to_nothing_is_counted_never_guessed() {
    let mut b = SceneBuilder::modern();
    b.scene("Scene", &["Lost"], &[], None);
    b.object(
        "Lost",
        &ObjectSpec {
            parent: Some("Ghost".into()),
            ..spec(Some("ME:Nowhere"))
        },
    );
    let m = model_of(&b.bytes());
    match &m.ids[at(&m, "@ob:Lost")].detail {
        Detail::Object { data, parent } => assert_eq!((*data, *parent), (None, None)),
        other => panic!("{other:?}"),
    }
    assert_eq!(m.unresolved, 2);
}

fn scene_err(f: &FileBuilder) -> &'static str {
    let bytes = f.bytes();
    let file = read_blend(&mut &bytes[..], &ReadLimits::default()).expect("check: the file reads");
    match read_scene(&file) {
        Err(ReadError::Malformed { what }) => what,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_data_block_without_an_id_header_is_refused() {
    let mut f = FileBuilder::classic();
    f.schema.add_struct("Weird", &[("int", "x"), ("int", "y")]);
    f.block(b"XX\0\0", "Weird", 0x100, vec![0; 8]);
    assert_eq!(scene_err(&f), "a data-block's struct has no ID header");
    let mut short = SceneBuilder::modern();
    short.scene("Scene", &[], &[], None);
    let ob = short.file.schema.index("Object");
    short.file.raw(b"OB\0\0", ob, 0x200, 1, vec![0; 40]);
    assert_eq!(
        scene_err(&short.file),
        "a data-block is shorter than its struct"
    );
}
