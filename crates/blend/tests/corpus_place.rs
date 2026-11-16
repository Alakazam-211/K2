//! The pack's placements against the world matrices the public corpus's files cache (with
//! `BLEND_PUBLIC_CORPUS` naming the folder `scripts/fetch-corpus.sh` fills; it fails when that is
//! unset). The app caches each object's world matrix in the files it saves up to the 3.x versions
//! (`obmat`); files from 4.x on keep none, so they can't be checked here. The app is only a referee
//! through the numbers its saved files hold.
//!
//! - Every object with a cached matrix is placed by the pack, and its world matrix through its
//!   chain (the kit's `world_matrix`) equals the cached one within `f32` rounding: at most
//!   1e-5 of the larger matrix's largest entry (and at least of 1).
//! - A file that opens live is read through the kit (its view index): every unit's placement is
//!   the one the pack's placement code gives from the file's model. A file the live tier
//!   declines by version is read by that same code from the same model.
//! - Each scene's unit settings are printed beside the store's `unit_scale` (read as metres per
//!   scene unit whatever the unit system: a hypothesis until a saved file confirms it).
//! - The design's two strict rules are counted: a parent inverse whose last row isn't exactly
//!   `0, 0, 0, 1`, and an axis of length 0 with an angle of 0.

mod live_support;

use std::collections::HashMap;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::placement::{world_matrix, Link, Mat4, Placement, Precision};
use k2_mpc_base::session::Pack;
use k2_mpc_blend::live::{address_of, model, place, BlendLive, LiveLimits, OBJS, OBJ_PROPS};
use k2_mpc_blend::write::decompress;
use k2_mpc_blend::{read_blend, read_scene, BlendFile, Detail};

/// The world matrix `block`'s object caches, column-major, by the names files give it.
fn cached(file: &BlendFile, block: u32) -> Option<Mat4> {
    let s = file.blocks[block as usize].head.sdna;
    let body = file.body(block)?;
    let (at, f) = ["obmat", "object_to_world", "runtime.object_to_world"]
        .iter()
        .find_map(|p| file.sdna.field_path(s, p))?;
    if f.is_pointer() || file.sdna.type_name(f.type_index) != "float" || f.array_len != 16 {
        return None;
    }
    let e = file.endian();
    let mut m = [0.0f64; 16];
    for (i, x) in m.iter_mut().enumerate() {
        *x = f64::from(e.f32(body, at as usize + 4 * i)?);
    }
    Some(m)
}

/// `(the largest difference, the scale)`: the scale is the largest entry of either, at least 1.
/// A scene's field at `path` as its raw number (`-` when the schema has none, `?` when it
/// isn't a number this reads).
fn unit_field(file: &BlendFile, block: u32, path: &str) -> String {
    let s = file.blocks[block as usize].head.sdna;
    let (Some(body), Some((at, f))) = (file.body(block), file.sdna.field_path(s, path)) else {
        return "-".into();
    };
    let (e, at) = (file.endian(), at as usize);
    let v = match (file.sdna.type_name(f.type_index), f.array_len) {
        ("float", 1) => e.f32(body, at).map(|v| v.to_string()),
        ("char" | "uchar" | "uint8_t", 1) => body.get(at).map(|b| b.to_string()),
        ("short", 1) => e.i16(body, at).map(|v| v.to_string()),
        ("int", 1) => e.i32(body, at).map(|v| v.to_string()),
        _ => None,
    };
    v.unwrap_or_else(|| "?".into())
}

fn diff(a: &Mat4, b: &Mat4) -> (f64, f64) {
    let mut d: f64 = 0.0;
    let mut s: f64 = 1.0;
    for i in 0..16 {
        d = d.max((a[i] - b[i]).abs());
        s = s.max(a[i].abs()).max(b[i].abs());
    }
    (if d.is_nan() { f64::INFINITY } else { d }, s)
}

#[test]
#[ignore = "the public corpus: scripts/public-corpus.sh runs it"]
fn public_corpus_placements_match_cached_world_matrices() {
    let dir = std::env::var_os("BLEND_PUBLIC_CORPUS")
        .map(std::path::PathBuf::from)
        .expect("BLEND_PUBLIC_CORPUS: the public corpus's folder (scripts/fetch-corpus.sh)");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("the corpus folder")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".blend"))
        .collect();
    names.sort();
    assert!(names.len() >= 7, "the corpus holds {} files", names.len());
    let limits = LiveLimits::default();
    let (mut cached_n, mut compared, mut matched, mut worst) = (0usize, 0usize, 0usize, 0f64);
    let (mut inverses, mut rule1) = (0usize, 0usize);
    let (mut aa_objects, mut rule2, mut rule2_any) = (0usize, 0usize, 0usize);
    let (mut live_files, mut live_units) = (0usize, 0usize);
    let mut declined: Vec<String> = Vec::new();
    for n in &names {
        let bytes = std::fs::read(dir.join(n)).expect("a corpus file");
        let (how, raw) = decompress(&bytes, &limits.read).expect("decompresses");
        let file = read_blend(&mut &raw[..], &limits.read).expect("reads");
        let m = read_scene(&file).expect("models");
        let scene = match model::from_model(&file, &raw, &m, how) {
            Ok(s) => s,
            Err(d) => {
                println!("corpus placements: {n}: the model declined: {}", d.code);
                declined.push(n.clone());
                continue;
            }
        };
        // each scene's unit settings as the file holds them, beside what the store's
        // `unit_scale` reads (a hypothesis: metres per scene unit whatever the unit system)
        let fact = scene
            .facts
            .iter()
            .find(|(k, _)| *k == model::s::UNIT_SCALE)
            .map(|(_, v)| v.as_str());
        for id in m
            .ids
            .iter()
            .filter(|i| matches!(i.detail, Detail::Scene { .. }))
        {
            println!(
                "corpus units: {n}: scene {:?}: unit.system {}, unit.scale_length {}, \
                 unit.length_unit {}; the store's unit_scale {fact:?}",
                id.name,
                unit_field(&file, id.block, "unit.system"),
                unit_field(&file, id.block, "unit.scale_length"),
                unit_field(&file, id.block, "unit.length_unit"),
            );
        }
        let (ck, ok) = scene.minted_keys();
        let content = scene.to_content(&ck, &ok, scene.minted_marks());
        let props = content
            .iter()
            .find_map(|(r, x)| match x {
                Neutral::Map(m) if r == OBJ_PROPS => Some(m),
                _ => None,
            })
            .expect("the objects' fields");
        // the pack's placements from the file's model, by address
        let mut places: HashMap<String, Option<Placement>> = HashMap::new();
        for (o, k) in scene.objs.iter().zip(&ok) {
            let mut p = place::placement(&place::ObjFields::of(props, k), &o.place);
            if let Some(p) = &mut p {
                p.precision = Precision::F32;
            }
            places.insert(address_of(OBJS, k), p);
            if let Some(pi) = o.place.parentinv {
                inverses += 1;
                if !(pi[3] == 0.0 && pi[7] == 0.0 && pi[11] == 0.0 && pi[15] == 1.0) {
                    rule1 += 1;
                }
            }
            let zero = o
                .place
                .axis_angle
                .is_some_and(|(a, t)| a == [0.0; 3] && t == 0.0);
            rule2_any += usize::from(zero);
            if o.rotmode == "axis_angle" || o.rotmode == "-1" {
                aa_objects += 1;
                rule2 += usize::from(zero);
            }
        }
        // through the kit, for a file that opens live
        if BlendLive::default().decode(&bytes).is_ok() {
            let f = live_support::fx("corpus-place", |_| {});
            f.write(&bytes);
            let s = f.open();
            let a = f.agent(&s);
            let ix = s.view_index(a).unwrap().expect("a view index");
            for u in &ix.units {
                let want = places.get(&u.key).expect("a unit the model has");
                assert_eq!(&u.placement, want, "{n}: {} through the kit", u.key);
                live_units += 1;
            }
            live_files += 1;
        }
        // the cached world matrices
        let obj_ids = m
            .ids
            .iter()
            .filter(|id| matches!(id.detail, Detail::Object { .. }) && id.struct_name == "Object");
        for (id, k) in obj_ids.zip(&ok) {
            let Some(want) = cached(&file, id.block) else {
                continue;
            };
            cached_n += 1;
            let addr = address_of(OBJS, k);
            let w = world_matrix(&addr, |a| match places.get(a) {
                Some(Some(p)) => Link::Placed(p),
                Some(None) => Link::Unplaced,
                None => Link::NotShown,
            });
            let Some(w) = w else {
                println!(
                    "corpus placements: {n}: {addr} ({}) has no world matrix",
                    id.name
                );
                continue;
            };
            compared += 1;
            let (d, scale) = diff(&w, &want);
            if d <= 1e-5 * scale {
                matched += 1;
                worst = worst.max(d / scale);
            } else {
                println!(
                    "corpus placements: {n}: {addr} ({}) differs by {d:.3e} (scale {scale:.3e})",
                    id.name
                );
            }
        }
    }
    println!(
        "corpus placements: cached world matrices {cached_n}, compared {compared}, matched \
         {matched}, worst matched {worst:.3e} of scale"
    );
    println!(
        "corpus placements: through the kit, {live_units} units in {live_files} files equal the \
         pack's placements from the model"
    );
    println!(
        "corpus placements: strict rule 1, a parent inverse whose last row isn't exactly 0,0,0,1: \
         {rule1} of {inverses}"
    );
    println!(
        "corpus placements: strict rule 2, a zero axis with a zero angle: {rule2} of {aa_objects} \
         axis-angle objects; {rule2_any} over every object's stored axis and angle"
    );
    assert!(declined.is_empty(), "the model declined {declined:?}");
    assert!(cached_n > 0, "no object caches its world matrix");
    assert_eq!(
        compared, cached_n,
        "every object with a cached matrix is placed"
    );
    assert_eq!(
        matched, compared,
        "every placed object matches its cached matrix"
    );
    assert!(live_files > 0, "no corpus file opens live");
    assert_eq!((rule1, rule2), (0, 0), "a strict rule fired on the corpus");
}

/// The unit probes (`tests/corpus/unit-probes.tsv`, fetched by checksum into the folder
/// `BLEND_UNIT_PROBES` names; it fails when that is unset): the classroom and the pavilion, each a
/// file whose scenes mix unit systems 0 and 1 at a scale of 1. Each scene's unit settings are
/// printed, and the store holds no `unit_scale` for either: their doc's space gives no metres per
/// unit.
#[test]
#[ignore = "the public corpus: scripts/public-corpus.sh runs it"]
fn unit_probes_mixed_systems_give_no_scale() {
    let dir = std::env::var_os("BLEND_UNIT_PROBES")
        .map(std::path::PathBuf::from)
        .expect("BLEND_UNIT_PROBES: the unit probes' folder (scripts/fetch-corpus.sh)");
    let limits = LiveLimits::default();
    let mut seen = 0;
    for n in ["classroom.blend", "pavillon_barcelone_v1.2.blend"] {
        let bytes = std::fs::read(dir.join(n)).expect("a probe file");
        let (how, raw) = decompress(&bytes, &limits.read).expect("decompresses");
        let file = read_blend(&mut &raw[..], &limits.read).expect("reads");
        let m = read_scene(&file).expect("models");
        let scene = model::from_model(&file, &raw, &m, how).expect("the model reads it");
        let fact = scene
            .facts
            .iter()
            .find(|(k, _)| *k == model::s::UNIT_SCALE)
            .map(|(_, v)| v.clone());
        let mut systems = Vec::new();
        for id in m
            .ids
            .iter()
            .filter(|i| matches!(i.detail, Detail::Scene { .. }))
        {
            let system = unit_field(&file, id.block, "unit.system");
            println!(
                "unit probes: {n}: scene {:?}: unit.system {system}, unit.scale_length {}",
                id.name,
                unit_field(&file, id.block, "unit.scale_length"),
            );
            systems.push(system);
        }
        println!("unit probes: {n}: the store's unit_scale {fact:?}");
        assert!(
            systems.iter().any(|s| s == "0") && systems.iter().any(|s| s == "1"),
            "{n}: its scenes don't mix systems 0 and 1: {systems:?}"
        );
        assert_eq!(fact, None, "{n}: mixed unit systems give no unit_scale");
        seen += 1;
    }
    assert_eq!(seen, 2);
}
