//! Test-only (feature `fuzzing`): the properties the reader, the scene model and the index keep
//! on any input, shared by the cargo-fuzz targets (`fuzz/`) and the seed tests. A violated
//! property panics, like a crash: it is a finding.
//!
//! [`read`] (the `blend_read` target): on any bytes the reader either refuses with a typed error
//! or returns a file that reads the same twice, and so does its scene model; the model's every
//! index names one of its data-blocks, every key is a node key in the kit's grammar, every kind
//! a word the kit takes.
//!
//! [`index`] (the `blend_index` target): the pack's index entry point either refuses the same way
//! twice, never with a read failure (the bytes are in memory), or returns the same draft twice,
//! whose parents come first and whose keys are node keys, unique unless the file names two
//! data-blocks alike.

use std::collections::BTreeSet;

use k2_mpc_base::index::{IndexDraft, IndexLimits, IndexRefused, NodeKey, INDEX_TEXT_MAX};

use crate::error::ReadLimits;
use crate::pack::index_blend_with;
use crate::reader::read_blend;
use crate::scene::{read_scene, Detail, SceneModel};

/// The bounds the fuzz targets read under: small, so a run stays fast.
pub fn limits() -> ReadLimits {
    ReadLimits::new(16 << 20, 100_000, 8 << 20, 64 << 10, 1 << 20, 1 << 20)
}

/// The index bounds the fuzz targets index under.
pub fn index_limits() -> IndexLimits {
    IndexLimits::new(50_000, 64, 32 << 20, INDEX_TEXT_MAX)
}

/// The reader's and the index's properties, on `data`: [`read`] and [`index`]
/// ([`write`](fn@crate::write) is its own target).
pub fn properties(data: &[u8]) {
    read(data);
    index(data);
}

/// The writer's properties (the `blend_write` target): for the first objects `data` holds,
/// each kind of edit alone and all at once either refuses, typed, the same way twice, or
/// writes the same file twice, which differs from `data`'s decompressed bytes only inside the
/// fields it says it patched, and reads again.
pub fn write(data: &[u8]) {
    use crate::write::{decompress, write as write_file, Change, Edit};
    let Ok((_, raw)) = decompress(data, &limits()) else {
        return;
    };
    let Ok(file) = read_blend(&mut &raw[..], &limits()) else {
        return;
    };
    let Ok(model) = read_scene(&file) else {
        return;
    };
    let keys: Vec<String> = model
        .ids
        .iter()
        .filter(|i| i.kind == "object")
        .filter_map(|i| i.key.clone())
        .take(3)
        .collect();
    let n = (data.len() % 1000) as f32;
    let mut all = Vec::new();
    for (k, key) in keys.iter().enumerate() {
        for change in [
            Change::Location([n, -n, 0.5]),
            Change::Rotation([0.25, n, -0.5]),
            Change::Scale([1.5, 2.0, n + 1.0]),
            Change::Visibility {
                viewport: Some(data.len().is_multiple_of(2)),
                select: None,
                render: Some(true),
            },
            Change::Name(format!("fz{k}x{}", data.len() % 97)),
        ] {
            let edit = Edit::new(key.clone(), change);
            all.push(edit.clone());
            one(data, &raw, &[edit]);
        }
    }
    if !all.is_empty() {
        one(data, &raw, &all);
    }

    fn one(data: &[u8], raw: &[u8], edits: &[Edit]) {
        let a = write_file(data, edits, &limits());
        let b = write_file(data, edits, &limits());
        match (a, b) {
            (Ok(a), Ok(b)) => {
                assert_eq!(a, b, "the writer is deterministic");
                let (_, out) = decompress(&a.bytes, &limits()).expect("a written file decodes");
                assert_eq!(out.len(), raw.len(), "a written file keeps its length");
                for (i, (x, y)) in raw.iter().zip(&out).enumerate() {
                    if x != y {
                        assert!(
                            a.patched.iter().any(|s| (s.offset as usize
                                ..(s.offset + u64::from(s.len)) as usize)
                                .contains(&i)),
                            "a byte outside the patched fields changed"
                        );
                    }
                }
                let f = read_blend(&mut &out[..], &limits()).expect("a written file reads");
                read_scene(&f).expect("a written file models");
            }
            (Err(_), Err(_)) => {}
            _ => panic!("the writer is deterministic: one run refused, one didn't"),
        }
    }
}

/// The model of `data`, when it reads.
fn model_of(data: &[u8]) -> Option<SceneModel> {
    let file = read_blend(&mut &data[..], &limits()).ok()?;
    read_scene(&file).ok()
}

/// The reader's and the model's properties.
pub fn read(data: &[u8]) {
    let Ok(file) = read_blend(&mut &data[..], &limits()) else {
        return;
    };
    let again = read_blend(&mut &data[..], &limits()).expect("a second read of the same bytes");
    assert_eq!(file.blocks, again.blocks, "reading is deterministic");
    for i in 0..file.blocks.len() as u32 {
        assert_eq!(file.body(i), again.body(i));
        assert_eq!(file.body_digest(i), again.body_digest(i));
    }
    let Ok(model) = read_scene(&file) else {
        return;
    };
    let model2 = read_scene(&again).expect("a second model of the same bytes");
    assert_eq!(model, model2, "the model is deterministic");
    let n = model.ids.len();
    for id in &model.ids {
        if let Some(k) = &id.key {
            assert!(NodeKey::parse(k).is_some(), "a key in the kit's grammar");
        }
        assert!(
            !id.kind.is_empty()
                && id
                    .kind
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
            "a kind the kit takes"
        );
        assert!(id.library.is_none_or(|l| l < n));
        assert!(id.materials.iter().flatten().all(|&m| m < n));
        let refs: Vec<usize> = match &id.detail {
            Detail::Scene {
                objects,
                collections,
                camera,
                world,
            } => objects
                .iter()
                .chain(collections)
                .chain(camera)
                .chain(world)
                .copied()
                .collect(),
            Detail::Collection {
                objects,
                collections,
            } => objects.iter().chain(collections).copied().collect(),
            Detail::Object { data, parent } => data.iter().chain(parent).copied().collect(),
            _ => Vec::new(),
        };
        assert!(
            refs.iter().all(|&r| r < n),
            "every index names a data-block"
        );
    }
}

/// The index entry point's properties.
pub fn index(data: &[u8]) {
    let first = index_blend_with(&mut &data[..], &index_limits(), &limits());
    let second = index_blend_with(&mut &data[..], &index_limits(), &limits());
    match (first, second) {
        (Ok(a), Ok(b)) => {
            assert_eq!(a, b, "the index is deterministic");
            draft(&a, data);
        }
        (Err(IndexRefused::Read(e)), _) | (_, Err(IndexRefused::Read(e))) => {
            panic!("bytes in memory can't fail to read: {e}")
        }
        (Err(_), Err(_)) => {}
        _ => panic!("the index is deterministic: one run refused, one didn't"),
    }
}

fn draft(draft: &IndexDraft, data: &[u8]) {
    let mut keys = BTreeSet::new();
    for (i, node) in draft.nodes.iter().enumerate() {
        if let Some(p) = node.parent {
            assert!((p as usize) < i, "a parent comes first");
        }
        if let Some(k) = &node.key {
            assert!(NodeKey::parse(k).is_some(), "a node key");
            if !keys.insert(k.clone()) {
                let model = model_of(data).expect("an indexed file models");
                assert!(duplicate_ok(&model, k), "keys are unique");
            }
        }
    }
}

/// A key two data-blocks share: only when the file itself names two data-blocks alike (the kit
/// mints keys for both).
fn duplicate_ok(model: &SceneModel, key: &str) -> bool {
    model
        .ids
        .iter()
        .filter(|i| i.key.as_deref() == Some(key))
        .count()
        > 1
}

/// The ear-clipping limit the `blend_triangulate` target runs under: small, so polygons over it
/// are common.
pub const TRIANGULATE_MAX_CORNERS: u32 = 64;

/// `data` read as polygons for [`triangulate`]: a `u16` (little-endian) count of positions (at
/// most 1,024), that many positions (three `f32` each, as far as the bytes go), then polygons:
/// a corner count byte and that many `u16` vertex indices, each taken modulo the positions (0
/// without any). A polygon cut short is left out.
pub fn triangulate_input(data: &[u8]) -> (Vec<[f32; 3]>, Vec<Vec<u32>>) {
    let mut at = 0usize;
    let n = data
        .get(..2)
        .map_or(0, |b| usize::from(u16::from_le_bytes([b[0], b[1]])))
        .min(1024);
    at += 2;
    let mut positions = Vec::with_capacity(n);
    while positions.len() < n {
        let Some(b) = data.get(at..at + 12) else {
            break;
        };
        let f = |k: usize| f32::from_le_bytes(b[4 * k..4 * k + 4].try_into().unwrap());
        positions.push([f(0), f(1), f(2)]);
        at += 12;
    }
    let p = positions.len() as u32;
    let mut faces = Vec::new();
    while let Some(&k) = data.get(at) {
        at += 1;
        let Some(b) = data.get(at..at + 2 * usize::from(k)) else {
            break;
        };
        at += 2 * usize::from(k);
        let face = b
            .chunks_exact(2)
            .map(|c| {
                let i = u32::from(u16::from_le_bytes([c[0], c[1]]));
                if p == 0 {
                    0
                } else {
                    i % p
                }
            })
            .collect();
        faces.push(face);
    }
    (positions, faces)
}

/// Polygons as [`triangulate_input`] reads them back (positions past 1,024 and polygons past
/// 255 corners are cut).
pub fn triangulate_seed(positions: &[[f32; 3]], faces: &[Vec<u32>]) -> Vec<u8> {
    let n = positions.len().min(1024);
    let mut out = (n as u16).to_le_bytes().to_vec();
    for p in &positions[..n] {
        for x in p {
            out.extend(x.to_le_bytes());
        }
    }
    for f in faces {
        let f = &f[..f.len().min(255)];
        out.push(f.len() as u8);
        for &i in f {
            out.extend((i as u16).to_le_bytes());
        }
    }
    out
}

/// The `blend_triangulate` target's seeds, built here: a cube's quads, regular polygons of 3 to
/// 12 corners, the concave set (an L, an arrow, a ten-pointed star, a comb), a polygon crossing
/// itself, corners on one line and on one point, and a 100-corner star over the limit.
pub fn triangulate_seeds() -> Vec<(&'static str, Vec<u8>)> {
    let ring = |n: usize, r: &dyn Fn(usize) -> f64, z: f32| -> Vec<[f32; 3]> {
        (0..n)
            .map(|k| {
                let a = k as f64 * std::f64::consts::TAU / n as f64;
                [(r(k) * a.cos()) as f32, (r(k) * a.sin()) as f32, z]
            })
            .collect()
    };
    let all = |pts: &[[f32; 3]]| vec![(0..pts.len() as u32).collect::<Vec<u32>>()];
    let flat = |pts: &[[f32; 2]]| pts.iter().map(|p| [p[0], p[1], 0.0]).collect::<Vec<_>>();
    let mut out = Vec::new();
    let cube = vec![
        [-1.0, -1.0, -1.0],
        [1.0, -1.0, -1.0],
        [1.0, 1.0, -1.0],
        [-1.0, 1.0, -1.0],
        [-1.0, -1.0, 1.0],
        [1.0, -1.0, 1.0],
        [1.0, 1.0, 1.0],
        [-1.0, 1.0, 1.0],
    ];
    let quads = vec![
        vec![0, 3, 2, 1],
        vec![4, 5, 6, 7],
        vec![0, 1, 5, 4],
        vec![3, 7, 6, 2],
        vec![0, 4, 7, 3],
        vec![1, 2, 6, 5],
    ];
    out.push(("cube", triangulate_seed(&cube, &quads)));
    let mut regular_p = Vec::new();
    let mut regular_f = Vec::new();
    for n in 3..=12 {
        let base = regular_p.len() as u32;
        regular_p.extend(ring(n, &|_| 1.0, 0.5));
        regular_f.push((base..base + n as u32).collect());
    }
    out.push(("regular", triangulate_seed(&regular_p, &regular_f)));
    let l = flat(&[
        [2.0, 0.0],
        [2.0, 1.0],
        [1.0, 1.0],
        [1.0, 2.0],
        [0.0, 2.0],
        [0.0, 0.0],
    ]);
    out.push(("l", triangulate_seed(&l, &all(&l))));
    let arrow = flat(&[[0.0, 0.0], [2.0, 1.0], [0.0, 2.0], [1.0, 1.0]]);
    out.push(("arrow", triangulate_seed(&arrow, &all(&arrow))));
    let star = ring(10, &|k| if k % 2 == 0 { 0.8 } else { 2.0 }, 0.0);
    out.push(("star", triangulate_seed(&star, &all(&star))));
    let comb = flat(&[
        [4.0, 1.0],
        [3.0, 1.0],
        [3.0, 3.0],
        [2.0, 3.0],
        [2.0, 1.0],
        [1.0, 1.0],
        [1.0, 3.0],
        [0.0, 3.0],
        [0.0, 0.0],
        [5.0, 0.0],
        [5.0, 3.0],
        [4.0, 3.0],
    ]);
    out.push(("comb", triangulate_seed(&comb, &all(&comb))));
    let crossing = flat(&[[3.0, 3.0], [0.0, 2.0], [0.0, 3.0], [1.0, 0.0], [2.0, 3.0]]);
    out.push(("crossing", triangulate_seed(&crossing, &all(&crossing))));
    let line = flat(&[[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0]]);
    out.push(("line", triangulate_seed(&line, &all(&line))));
    let point = vec![[1.0f32; 3]; 5];
    out.push(("point", triangulate_seed(&point, &all(&point))));
    let big = ring(100, &|k| if k % 2 == 0 { 1.0 } else { 0.5 }, 0.0);
    out.push(("big", triangulate_seed(&big, &all(&big))));
    out
}

/// The triangulator's properties (the `blend_triangulate` target), on `data` read as polygons
/// ([`triangulate_input`]) under [`TRIANGULATE_MAX_CORNERS`]: each polygon of `n >= 3` corners
/// gives exactly `n - 2` triangles (fewer corners, none), each of its own corners, in at most
/// `n * n` ear tests; and the same triangles and counts twice.
pub fn triangulate(data: &[u8]) {
    use crate::meshread::tri::{triangulate_face, TriCounts};
    let (positions, faces) = triangulate_input(data);
    let run = || {
        let mut out = Vec::new();
        let mut counts = TriCounts::default();
        for face in &faces {
            let (before, tests) = (out.len(), counts.ear_tests);
            triangulate_face(
                &positions,
                face,
                TRIANGULATE_MAX_CORNERS,
                &mut out,
                &mut counts,
            );
            let n = face.len();
            assert_eq!(
                out.len() - before,
                n.saturating_sub(2),
                "n - 2 triangles for {n} corners"
            );
            for t in &out[before..] {
                assert!(
                    t.iter().all(|i| face.contains(i)),
                    "a triangle of its own corners"
                );
            }
            let n = n as u64;
            assert!(counts.ear_tests - tests <= n * n, "at most n * n ear tests");
        }
        (out, counts)
    };
    assert_eq!(run(), run(), "the same triangles twice");
}
