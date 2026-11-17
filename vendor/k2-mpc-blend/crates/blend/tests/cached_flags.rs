//! The cached fields the writer leaves as they are: a viewport hide written by the writer
//! patches the object's `restrictflag` alone, so the fields a file caches from it (the view
//! layer's `Base.flag`, `flag_from_collection` and `flag_legacy`, and the object's own
//! `base_flag`, where the file's schema has them) keep their stored values. The program
//! refreshes them when it opens the file (`PROVENANCE.md`); our reader, the mesh read and the
//! live view read the hide from `restrictflag`, so each reads the object hidden by itself with
//! those fields stale. On the generated living room (its schema has the view layer's bases, not
//! the object's `base_flag`) and on the program's own save of it at 5.3.0
//! (`tests/builds/5.3.0/`, both).

mod live_support;

use std::path::Path;

use k2_mpc_blend::meshread::{mesh_read, MeshReadLimits};
use k2_mpc_blend::testgen::scenes;
use k2_mpc_blend::write::decompress;
use k2_mpc_blend::{read_blend, read_scene, write, Change, Edit, ReadLimits, SceneModel};

/// The cached fields read here, by struct: each that the file's schema has.
const CACHED: &[(&str, &[&str])] = &[
    ("Base", &["flag", "flag_from_collection", "flag_legacy"]),
    ("Object", &["base_flag"]),
];

/// Each cached field's bytes in `bytes` (decompressed first), in file order: the struct and
/// field, the block, the element, and the bytes.
fn cached(bytes: &[u8]) -> Vec<(String, u32, u64, Vec<u8>)> {
    let l = ReadLimits::default();
    let (_, raw) = decompress(bytes, &l).expect("decompresses");
    let f = read_blend(&mut &raw[..], &l).expect("reads");
    let mut out = Vec::new();
    for (i, b) in f.blocks.iter().enumerate() {
        let Some(name) = f.sdna.struct_name(b.head.sdna) else {
            continue;
        };
        let Some((_, fields)) = CACHED.iter().find(|(s, _)| *s == name) else {
            continue;
        };
        let size = f.sdna.struct_def(b.head.sdna).expect("its struct").size as usize;
        let body = f.body(i as u32).expect("its body");
        for e in 0..b.head.count {
            for field in *fields {
                let Some(fd) = f.sdna.field(b.head.sdna, field) else {
                    continue;
                };
                let at = e as usize * size + fd.offset as usize;
                let v = body
                    .get(at..at + fd.size as usize)
                    .expect("the field inside its block")
                    .to_vec();
                out.push((format!("{name}.{field}"), i as u32, e, v));
            }
        }
    }
    out
}

fn model(bytes: &[u8]) -> SceneModel {
    let l = ReadLimits::default();
    let (_, raw) = decompress(bytes, &l).expect("decompresses");
    let f = read_blend(&mut &raw[..], &l).expect("reads");
    read_scene(&f).expect("models")
}

/// The key our reader gives the object named `name`.
fn key_named(m: &SceneModel, name: &str) -> String {
    m.ids
        .iter()
        .find(|i| i.name == name && i.key.as_deref().is_some_and(|k| k.starts_with("@ob:")))
        .and_then(|i| i.key.clone())
        .unwrap_or_else(|| panic!("our reader finds no object {name:?}"))
}

/// The mesh read's `hidden` of each drawn object, by name.
fn drawn_hidden(bytes: &[u8]) -> Vec<(String, Option<&'static str>)> {
    let r = mesh_read(bytes, &MeshReadLimits::default())
        .unwrap_or_else(|e| panic!("the mesh read refused: {e}"));
    r.report
        .drawn
        .iter()
        .map(|d| (d.name.clone(), d.hidden))
        .collect()
}

/// Each unit's `hidden_by` in a live session's view of `bytes`, by name.
fn view_hidden_by(tag: &str, bytes: &[u8]) -> Vec<(String, Option<String>)> {
    let f = live_support::fx(tag, |_| {});
    f.write(bytes);
    let s = f.open();
    let host = f.person(&s);
    let ix = s
        .view_index(host)
        .unwrap_or_else(|e| panic!("the view: {e}"))
        .expect("a view index");
    ix.units
        .iter()
        .map(|u| {
            (
                u.name.clone(),
                u.derived_value("hidden_by").map(str::to_owned),
            )
        })
        .collect()
}

/// What one file's row counted.
#[derive(Debug, Default)]
struct Counted {
    bases: usize,
    base_flags: usize,
}

/// One file: the first object the mesh read draws shown, hidden in viewports by the writer;
/// every cached field as it was, and the hide read back by our reader, the mesh read and the
/// live view.
fn hide_keeps_cached(tag: &str, input: &[u8]) -> Counted {
    let shown = drawn_hidden(input);
    let (name, _) = shown
        .iter()
        .find(|(_, h)| h.is_none())
        .unwrap_or_else(|| panic!("{tag}: no object drawn shown"))
        .clone();
    let before_view = view_hidden_by(&format!("{tag}-in"), input);
    let was = before_view
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("{tag}: the live view has no {name}"));
    assert_eq!(
        was.1, None,
        "{tag}: {name} hidden in the live view before the hide"
    );
    let m = model(input);
    let key = key_named(&m, &name);
    let w = write(
        input,
        &[Edit::new(
            key.clone(),
            Change::Visibility {
                viewport: Some(true),
                select: None,
                render: None,
            },
        )],
        &ReadLimits::default(),
    )
    .unwrap_or_else(|e| panic!("{tag}: the writer refused the hide: {e}"));
    let out = &w.bytes;
    // the cached fields, stale: each as the input stores it
    let a = cached(input);
    let b = cached(out);
    assert_eq!(a.len(), b.len(), "{tag}: the cached fields found");
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(
            x, y,
            "{tag}: {} of block {} element {} changed",
            x.0, x.1, x.2
        );
    }
    let c = Counted {
        bases: a.iter().filter(|x| x.0 == "Base.flag").count(),
        base_flags: a.iter().filter(|x| x.0 == "Object.base_flag").count(),
    };
    assert!(c.bases > 0, "{tag}: no view layer base read");
    // our reader: the hide in restrictflag
    let mo = model(out);
    let id = mo.by_key(&key).expect("the object after the hide");
    let flag: i64 = id
        .fields
        .iter()
        .chain(&id.named)
        .find(|(n, _)| n == "restrictflag")
        .map(|(_, v)| v.parse().expect("restrictflag a number"))
        .expect("restrictflag");
    assert_eq!(flag & 1, 1, "{tag}: {key}: restrictflag {flag}");
    assert_eq!(id.hidden, Some(true), "{tag}: {key}: our reader's hidden");
    // the mesh read: hidden by itself
    let after = drawn_hidden(out);
    let d = after
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("{tag}: the mesh read no longer draws {name}"));
    assert_eq!(d.1, Some("self"), "{tag}: {name}: the mesh read's hidden");
    // the live view: hidden by itself
    let view = view_hidden_by(&format!("{tag}-out"), out);
    let u = view
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("{tag}: the live view has no {name}"));
    assert_eq!(
        u.1.as_deref(),
        Some("self"),
        "{tag}: {name}: the live view's hidden_by"
    );
    eprintln!(
        "cached flags {tag}: {name} hidden by the writer; {} cached fields as stored ({} view layer bases, {} objects' base_flag); restrictflag {flag}, the mesh read's hidden self, the live view's hidden_by self",
        a.len(),
        c.bases,
        c.base_flags
    );
    c
}

#[test]
fn a_hide_leaves_the_cached_flags_stale_and_reads_hidden() {
    let c = hide_keeps_cached("generated", &scenes::living_room());
    assert_eq!(
        c.base_flags, 0,
        "the generated schema has no object base_flag"
    );
}

#[test]
fn a_hide_of_the_programs_own_save_leaves_both_cached_flags_stale() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("builds")
        .join("5.3.0")
        .join("living_room.blend");
    let input = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let c = hide_keeps_cached("kept-5.3.0", &input);
    assert!(
        c.base_flags > 0,
        "the program's save holds no object base_flag"
    );
}
