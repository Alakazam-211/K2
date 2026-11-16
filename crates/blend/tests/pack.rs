//! The pack's draft on files built here: the tree (scenes, then what holds what, then the rest),
//! keys, attributes, weak IDs and flags; the same draft from every way a file is stored and from
//! another save of the same scene; and each refusal as the kit's.

mod support;

use k2_mpc_base::index::{IndexDraft, IndexLimits, IndexRefused, INDEX_TEXT_MAX};
use k2_mpc_blend::testgen::{gzip, zstd_frames, SceneBuilder};
use k2_mpc_blend::{index_blend, HeaderForm};
use support::{limits, spec, studio, Studio};

fn draft(bytes: &[u8]) -> IndexDraft {
    index_blend(&mut &bytes[..], &limits()).expect("check: the file indexes")
}

/// (key, kind, parent's key) per node, in draft order.
fn outline(d: &IndexDraft) -> Vec<(String, String, Option<String>)> {
    let key = |i: usize| d.nodes[i].key.clone().unwrap_or_default();
    d.nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (key(i), n.kind.clone(), n.parent.map(|p| key(p as usize))))
        .collect()
}

fn node<'a>(d: &'a IndexDraft, key: &str) -> &'a k2_mpc_base::index::DraftNode {
    d.nodes
        .iter()
        .find(|n| n.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("no node {key}"))
}

fn attr<'a>(d: &'a IndexDraft, key: &str, name: &str) -> Option<&'a str> {
    node(d, key).attrs.get(name).map(String::as_str)
}

#[test]
fn scenes_hold_collections_and_objects_and_the_rest_are_roots() {
    let d = draft(&studio(&Studio::default()));
    let s = |x: &str| x.to_owned();
    let want: Vec<(String, String, Option<String>)> = vec![
        (s("@sc:Scene"), s("scene"), None),
        (s("@gr:Set"), s("collection"), Some(s("@sc:Scene"))),
        (s("@gr:Inner"), s("collection"), Some(s("@gr:Set"))),
        (s("@ob:Child"), s("object"), Some(s("@gr:Inner"))),
        (s("@ob:Cube"), s("object"), Some(s("@gr:Set"))),
        (s("@ob:Light"), s("object"), Some(s("@gr:Set"))),
        (s("@gr:Props"), s("collection"), Some(s("@sc:Scene"))),
        (s("@ob:Camera"), s("object"), Some(s("@sc:Scene"))),
        (s("@li:lib"), s("library"), None),
        (s("@ca:Cam"), s("camera"), None),
        (s("@la:Lamp"), s("light"), None),
        (s("@me:Cube"), s("mesh"), None),
        (s("@ma:Red"), s("material"), None),
        (s("@ma:Blue"), s("material"), None),
        (s("@li:lib/ma:Shared"), s("material"), None),
        (s("@wo:World"), s("world"), None),
    ];
    assert_eq!(outline(&d), want, "the window manager is no node");
    assert_eq!(d.source, "blend-4.03");
    for (k, v) in [
        ("version", "4.03"),
        ("header", "classic"),
        ("pointer_size", "8"),
        ("endian", "little"),
        ("compression", "none"),
        ("data_blocks", "17"),
        ("interface_data_blocks", "1"),
        ("unresolved_pointers", "0"),
    ] {
        assert_eq!(d.attrs.get(k).map(String::as_str), Some(v), "{k}");
    }
    // what an object says, by key
    assert_eq!(attr(&d, "@ob:Cube", "struct"), Some("Object"));
    assert_eq!(attr(&d, "@ob:Cube", "data"), Some("@me:Cube"));
    assert_eq!(attr(&d, "@ob:Cube", "data_type"), Some("mesh"));
    assert_eq!(attr(&d, "@ob:Cube", "materials"), Some("@ma:Red,-"));
    assert_eq!(
        attr(&d, "@ob:Cube", "collections"),
        Some("@gr:Set,@gr:Props")
    );
    assert_eq!(attr(&d, "@ob:Cube", "loc"), Some("0,0,0.25"));
    assert_eq!(attr(&d, "@ob:Cube", "parent"), None);
    assert_eq!(attr(&d, "@ob:Child", "parent"), Some("@ob:Cube"));
    assert_eq!(attr(&d, "@ob:Child", "data"), Some("-"));
    assert_eq!(attr(&d, "@ob:Child", "data_type"), Some("empty"));
    assert_eq!(attr(&d, "@ob:Child", "restrictflag"), Some("5"));
    assert_eq!(attr(&d, "@ob:Camera", "collections"), Some("@sc:Scene"));
    assert_eq!(attr(&d, "@ob:Light", "data_type"), Some("light"));
    assert_eq!(attr(&d, "@sc:Scene", "camera"), Some("@ob:Camera"));
    assert_eq!(attr(&d, "@sc:Scene", "objects"), Some("1"));
    assert_eq!(attr(&d, "@sc:Scene", "children"), Some("2"));
    assert_eq!(attr(&d, "@gr:Inner", "collections"), Some("@gr:Set"));
    assert_eq!(attr(&d, "@me:Cube", "materials"), Some("@ma:Blue"));
    assert_eq!(attr(&d, "@ca:Cam", "lens"), Some("50"));
    assert_eq!(attr(&d, "@la:Lamp", "energy"), Some("10"));
    assert_eq!(attr(&d, "@li:lib", "file"), Some("lib.blend"));
    assert_eq!(attr(&d, "@li:lib/ma:Shared", "library"), Some("@li:lib"));
    assert!(node(&d, "@li:lib/ma:Shared").flags.contains("linked"));
    // weak IDs: an object's data
    assert_eq!(
        node(&d, "@ob:Cube").weak.get("data").map(String::as_str),
        Some("@me:Cube")
    );
    assert!(node(&d, "@ob:Child").weak.is_empty());
    // every node carries a content digest; only the child disabled in viewports is hidden
    assert!(d
        .nodes
        .iter()
        .all(|n| n.content.is_some() && n.text.is_none()));
    let hidden: Vec<&str> = d
        .nodes
        .iter()
        .filter(|n| !n.visible)
        .filter_map(|n| n.key.as_deref())
        .collect();
    assert_eq!(
        hidden,
        ["@ob:Child"],
        "restrictflag 5: disabled in viewports and renders"
    );
    assert_eq!(attr(&d, "@ob:Child", "hide_viewport"), Some("true"));
    assert_eq!(attr(&d, "@ob:Child", "hide_render"), Some("true"));
    assert_eq!(attr(&d, "@ob:Child", "hide_select"), Some("false"));
    assert_eq!(attr(&d, "@ob:Cube", "hide_viewport"), Some("false"));
    assert_eq!(attr(&d, "@gr:Set", "hide_viewport"), Some("false"));
    assert_eq!(attr(&d, "@la:Lamp", "light_type"), Some("spot"));
    assert_eq!(attr(&d, "@ca:Cam", "camera_type"), Some("perspective"));
    assert!(d
        .nodes
        .iter()
        .all(|n| n.flags.iter().all(|f| f == "linked")));
}

#[test]
fn every_way_of_storing_a_file_indexes_the_same() {
    let plain = studio(&Studio::default());
    let want = draft(&plain);
    for (how, bytes) in [("gzip", gzip(&plain)), ("zstd", zstd_frames(&plain, 1000))] {
        let mut got = draft(&bytes);
        assert_eq!(got.attrs.remove("compression").as_deref(), Some(how));
        let mut want = want.clone();
        want.attrs.remove("compression");
        assert_eq!(got, want, "{how}");
    }
}

#[test]
fn another_save_of_the_same_scene_indexes_the_same() {
    let first = draft(&studio(&Studio::default()));
    let again = draft(&studio(&Studio {
        shift: 0x77_0000,
        ..Studio::default()
    }));
    assert_eq!(again, first, "every record and content digest the same");
    let moved = draft(&studio(&Studio {
        shift: 0x77_0000,
        cube_loc: [4.0, 0.0, 0.0],
        ..Studio::default()
    }));
    for (a, b) in first.nodes.iter().zip(&moved.nodes) {
        if a.key.as_deref() == Some("@ob:Cube") {
            assert_ne!(a.record, b.record);
            assert_ne!(a.content, b.content);
        } else {
            assert_eq!(a, b, "{:?}", a.key);
        }
    }
}

#[test]
fn the_older_shape_indexes_into_the_same_kind_of_tree() {
    let mut b = SceneBuilder::new(HeaderForm::Classic, 4, true, 273, true);
    b.scene("Scene", &["Cube"], &[], None);
    b.collection("Set", &["Cube", "Loose"], &[]);
    b.object("Cube", &spec(Some("ME:Cube")));
    b.object("Loose", &spec(None));
    b.mesh("Cube", &[], 3);
    let d = draft(&b.bytes());
    let s = |x: &str| x.to_owned();
    assert_eq!(
        outline(&d),
        vec![
            (s("@sc:Scene"), s("scene"), None),
            (s("@ob:Cube"), s("object"), Some(s("@sc:Scene"))),
            (s("@gr:Set"), s("collection"), None),
            (s("@ob:Loose"), s("object"), Some(s("@gr:Set"))),
            (s("@me:Cube"), s("mesh"), None),
        ],
        "a collection no scene holds is a root, holding what nothing placed"
    );
    assert_eq!(
        attr(&d, "@ob:Cube", "collections"),
        Some("@sc:Scene,@gr:Set")
    );
    assert_eq!(d.source, "blend-2.73");
    assert_eq!(d.attrs.get("endian").map(String::as_str), Some("big"));
    assert_eq!(d.attrs.get("pointer_size").map(String::as_str), Some("4"));
}

#[test]
fn a_name_over_the_key_limit_is_left_for_the_kit() {
    let mut b = SceneBuilder::new(HeaderForm::Large, 8, false, 501, false);
    b.scene("Scene", &[], &[], None);
    b.object_named(&[b' '; 255], &spec(None));
    b.object_named(&[b'N', 0xfe], &spec(None));
    let d = draft(&b.bytes());
    assert_eq!(d.nodes[1].key, None);
    assert_eq!(d.nodes[1].name.len(), 255);
    assert_eq!(d.nodes[2].key.as_deref(), Some("@ob:N%FE"));
    assert!(d.nodes[2].flags.contains("name_not_utf8"));
    assert_eq!(d.source, "blend-5.01");
    assert_eq!(d.attrs.get("header").map(String::as_str), Some("large"));
}

#[test]
fn each_refusal_is_the_kits() {
    let l = limits();
    let refuse = |bytes: &[u8], l: &IndexLimits| match index_blend(&mut &bytes[..], l) {
        Err(e) => e,
        Ok(_) => panic!("indexed"),
    };
    assert!(matches!(
        refuse(b"not a scene", &l),
        IndexRefused::Unsupported
    ));
    let mut unknown = studio(&Studio::default());
    unknown[8] = b'?';
    assert!(matches!(refuse(&unknown, &l), IndexRefused::Unsupported));
    let full = studio(&Studio::default());
    match refuse(&full[..full.len() / 2], &l) {
        IndexRefused::Unreadable { why } => assert!(why.contains("block"), "{why}"),
        other => panic!("{other:?}"),
    }
    match refuse(&full, &IndexLimits::new(5, 64, 64 << 20, INDEX_TEXT_MAX)) {
        IndexRefused::TooManyNodes { count, limit } => assert_eq!((count, limit), (16, 5)),
        other => panic!("{other:?}"),
    }
    match refuse(&full, &IndexLimits::new(100, 2, 64 << 20, INDEX_TEXT_MAX)) {
        IndexRefused::TooDeep { depth, limit } => assert_eq!((depth, limit), (3, 2)),
        other => panic!("{other:?}"),
    }
}
