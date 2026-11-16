//! Meshes in the public corpus (`tests/corpus/manifest.tsv`, fetched by `scripts/fetch-corpus.sh`
//! into the folder `BLEND_PUBLIC_CORPUS` names; each row fails when that is unset).
//!
//! The schema survey prints, for every file, the structs a mesh read looks at, as the file's own
//! schema gives them (struct and field names, types, sizes and offsets), and every mesh's counts,
//! pointers and attribute layers: a layer's type number and name, and the block its data is in.
//! Only names and numbers are printed: no geometry.
//!
//! The layouts row reads every mesh of every file: each with polygons reads, from the layout
//! the survey found its version saving (the structs up to 3.06, the attribute layers in 4.03,
//! the attribute store from 5.00), and one without has nothing to draw. A mesh holding two
//! layouts must read alike from both (none of the public files holds two).
//!
//! The scenes row reads every file the live tier reads: the scene drawn is each file's current
//! one, the output passes the strict checker, and every object drawn whose file caches its world
//! matrix (up to the 3.x saves) has a composed transform equal to the cached one within `f32`
//! rounding (the cached matrix is the oracle only: the read never uses it).

use std::collections::BTreeMap;
use std::path::PathBuf;

use k2_mpc_blend::meshread::check::check;
use k2_mpc_blend::meshread::mesh::{self, Layout, MeshSkip};
use k2_mpc_blend::meshread::tri::TriCounts;
use k2_mpc_blend::meshread::{mesh_read, MeshReadError, MeshReadLimits};
use k2_mpc_blend::reader::DATA;
use k2_mpc_blend::write::decompress;
use k2_mpc_blend::{read_blend, read_scene, BlendFile, Detail, ReadLimits, Target};

const MANIFEST: &str = include_str!("corpus/manifest.tsv");

fn names() -> Vec<&'static str> {
    MANIFEST
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| l.split('\t').next().unwrap())
        .collect()
}

fn corpus() -> PathBuf {
    std::env::var_os("BLEND_PUBLIC_CORPUS")
        .map(PathBuf::from)
        .expect("BLEND_PUBLIC_CORPUS: the public corpus's folder (scripts/fetch-corpus.sh)")
}

/// Block `i`'s body, sliced out of the decompressed file.
fn body<'a>(file: &BlendFile, raw: &'a [u8], i: u32) -> &'a [u8] {
    let h = &file.blocks[i as usize].head;
    &raw[h.body_offset as usize..(h.body_offset + h.len) as usize]
}

/// A block's code, struct name, count and length, as text.
fn block_text(file: &BlendFile, i: u32) -> String {
    let h = &file.blocks[i as usize].head;
    let code = String::from_utf8_lossy(&h.code).replace('\0', "");
    let st = if h.sdna == 0 {
        "raw".to_owned()
    } else {
        file.sdna.struct_name(h.sdna).unwrap_or("?").to_owned()
    };
    format!("{code}/{st} count={} len={}", h.count, h.len)
}

/// What a pointer at `at` of `b` (read from block `from`) names, as text.
fn ptr_text(file: &BlendFile, b: &[u8], at: usize, from: u32) -> String {
    let Some(p) = file.endian().ptr(b, at) else {
        return "short".into();
    };
    match file.resolve(p, from) {
        Target::Null => "null".into(),
        Target::Unresolved => "unresolved".into(),
        Target::Ambiguous => "ambiguous".into(),
        Target::Block(j) => block_text(file, j),
    }
}

/// A scalar field's value as text (`?` for another kind).
fn scalar(file: &BlendFile, b: &[u8], at: usize, ty: &str) -> String {
    let e = file.endian();
    let v = match ty {
        "char" | "uchar" | "int8_t" | "uint8_t" => b.get(at).map(|x| x.to_string()),
        "short" => e.i16(b, at).map(|x| x.to_string()),
        "ushort" => e.u16(b, at).map(|x| x.to_string()),
        "int" | "int32_t" => e.i32(b, at).map(|x| x.to_string()),
        "uint" | "uint32_t" => e.u32(b, at).map(|x| x.to_string()),
        "int64_t" => e.i64(b, at).map(|x| x.to_string()),
        "uint64_t" => e.u64(b, at).map(|x| x.to_string()),
        "float" => e.f32(b, at).map(|x| x.to_string()),
        _ => None,
    };
    v.unwrap_or_else(|| "?".into())
}

/// A NUL-ended char array as text.
fn chars(b: &[u8], at: usize, n: usize) -> String {
    let s = &b[at.min(b.len())..(at + n).min(b.len())];
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf8_lossy(&s[..end]).into_owned()
}

/// Every field of struct `s` at `base` of `b`: scalars and char arrays by value, pointers by what
/// they name, embedded structs walked (to `depth`). One `name=value` per field.
fn dump(file: &BlendFile, b: &[u8], base: usize, s: u32, from: u32, depth: u32) -> Vec<String> {
    let mut out = Vec::new();
    let Some(def) = file.sdna.struct_def(s) else {
        return out;
    };
    for f in &def.fields {
        let at = base + f.offset as usize;
        let ty = file.sdna.type_name(f.type_index);
        let v = if f.is_pointer() {
            if f.array_len == 1 && !f.function_pointer {
                ptr_text(file, b, at, from)
            } else {
                format!("{} pointers", f.array_len)
            }
        } else if (ty == "char" || ty == "uchar") && f.array_len > 1 {
            format!("{:?}", chars(b, at, f.array_len as usize))
        } else if f.array_len == 1 {
            match file.sdna.struct_of_type(f.type_index) {
                Some(inner) if depth > 0 => {
                    let mut inner_lines = dump(file, b, at, inner, from, depth - 1);
                    for l in &mut inner_lines {
                        *l = format!("{}.{l}", f.name);
                    }
                    out.extend(inner_lines);
                    continue;
                }
                Some(_) => format!("<{ty}>"),
                None => scalar(file, b, at, ty),
            }
        } else {
            format!("<{ty} x{}>", f.array_len)
        };
        out.push(format!("{}={v}", f.name));
    }
    out
}

/// The struct `name`'s fields as the schema gives them, those `keep` takes.
fn struct_line(file: &BlendFile, name: &str, keep: &dyn Fn(&str) -> bool) -> String {
    let Some(s) = file.sdna.struct_index(name) else {
        return format!("struct {name}: absent");
    };
    let def = file.sdna.struct_def(s).unwrap();
    let fields: Vec<String> = def
        .fields
        .iter()
        .filter(|f| keep(&f.name))
        .map(|f| {
            format!(
                "{} {}@{}",
                file.sdna.type_name(f.type_index),
                f.raw_name,
                f.offset
            )
        })
        .collect();
    format!("struct {name} size={}: {}", def.size, fields.join(", "))
}

/// The layers of the `CustomData` embedded at `at` of mesh block `me`: `type name -> block`.
fn layers(file: &BlendFile, raw: &[u8], me: u32, cd_at: usize) -> Vec<String> {
    let b = body(file, raw, me);
    let sd = &file.sdna;
    let (Some(cd), Some(lay)) = (
        sd.struct_index("CustomData"),
        sd.struct_index("CustomDataLayer"),
    ) else {
        return vec!["no CustomData or CustomDataLayer struct".into()];
    };
    let e = file.endian();
    let (Some(f_layers), Some(f_tot)) = (sd.field(cd, "layers"), sd.field(cd, "totlayer")) else {
        return vec!["CustomData lacks layers or totlayer".into()];
    };
    let tot = e.i32(b, cd_at + f_tot.offset as usize).unwrap_or(-1);
    let mut out = vec![format!("totlayer={tot}")];
    let Some(p) = e.ptr(b, cd_at + f_layers.offset as usize) else {
        return out;
    };
    let Target::Block(lb) = file.resolve(p, me) else {
        out.push(format!("layers -> {:?}", file.resolve(p, me)));
        return out;
    };
    out.push(format!("layers -> {}", block_text(file, lb)));
    let lbody = body(file, raw, lb);
    let size = sd.struct_def(lay).unwrap().size as usize;
    let (Some(ty), Some(name), Some(data)) = (
        sd.field(lay, "type"),
        sd.field(lay, "name"),
        sd.field(lay, "data"),
    ) else {
        out.push("CustomDataLayer lacks type, name or data".into());
        return out;
    };
    for k in 0..(tot.max(0) as usize) {
        let base = k * size;
        if base + size > lbody.len() {
            out.push(format!("layer {k}: past the block"));
            break;
        }
        let t = e.i32(lbody, base + ty.offset as usize).unwrap();
        let n = chars(lbody, base + name.offset as usize, name.array_len as usize);
        let d = ptr_text(file, lbody, base + data.offset as usize, lb);
        out.push(format!("layer {k}: type={t} name={n:?} data -> {d}"));
    }
    out
}

/// The attributes of the `AttributeStorage` embedded at `at` of mesh block `me`: each one's
/// name, numbers and the block its data is in (through an `AttributeArray` or
/// `AttributeSingle` when it points to one).
fn attributes(file: &BlendFile, raw: &[u8], me: u32, at: usize) -> Vec<String> {
    let b = body(file, raw, me);
    let sd = &file.sdna;
    let e = file.endian();
    let (Some(st), Some(attr)) = (
        sd.struct_index("AttributeStorage"),
        sd.struct_index("Attribute"),
    ) else {
        return vec!["no AttributeStorage or Attribute struct".into()];
    };
    let mut out = dump(file, b, at, st, me, 0);
    let (Some(list), Some(num)) = (
        sd.field(st, "dna_attributes"),
        sd.field(st, "dna_attributes_num"),
    ) else {
        out.push("AttributeStorage lacks dna_attributes or dna_attributes_num".into());
        return out;
    };
    let n = e.i32(b, at + num.offset as usize).unwrap_or(-1);
    let Some(p) = e.ptr(b, at + list.offset as usize) else {
        return out;
    };
    let Target::Block(lb) = file.resolve(p, me) else {
        out.push(format!("dna_attributes -> {:?}", file.resolve(p, me)));
        return out;
    };
    let lbody = body(file, raw, lb);
    let size = sd.struct_def(attr).unwrap().size as usize;
    for k in 0..(n.max(0) as usize) {
        let base = k * size;
        if base + size > lbody.len() {
            out.push(format!("attribute {k}: past the block"));
            break;
        }
        let mut name = String::from("?");
        let mut rest = Vec::new();
        for f in &sd.struct_def(attr).unwrap().fields {
            let fat = base + f.offset as usize;
            let ty = sd.type_name(f.type_index);
            if f.name == "name" && f.is_pointer() {
                if let Some(Target::Block(nb)) = e.ptr(lbody, fat).map(|q| file.resolve(q, lb)) {
                    let nbody = body(file, raw, nb);
                    name = chars(nbody, 0, nbody.len());
                }
            } else if f.name == "data" && f.is_pointer() {
                let t = e.ptr(lbody, fat).map(|q| file.resolve(q, lb));
                let text = match t {
                    Some(Target::Block(db)) => {
                        let ds = file.blocks[db as usize].head.sdna;
                        let dsn = sd.struct_name(ds).unwrap_or("raw");
                        let mut s = block_text(file, db);
                        if ds != 0 && (dsn == "AttributeArray" || dsn == "AttributeSingle") {
                            let inner = dump(file, body(file, raw, db), 0, ds, db, 0);
                            s = format!("{s} {{{}}}", inner.join(" "));
                        }
                        s
                    }
                    other => format!("{other:?}"),
                };
                rest.push(format!("data -> {text}"));
            } else if !f.is_pointer() && f.array_len == 1 {
                rest.push(format!("{}={}", f.name, scalar(file, lbody, fat, ty)));
            }
        }
        out.push(format!("attribute {k}: name={name:?} {}", rest.join(" ")));
    }
    out
}

#[test]
#[ignore = "the public corpus: scripts/public-corpus.sh runs it"]
fn mesh_schema_survey() {
    let dir = corpus();
    let mut files = 0;
    let mut meshes_seen = 0;
    for name in names() {
        let bytes = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (_, raw) = decompress(&bytes, &ReadLimits::default()).expect("decompresses");
        let file = read_blend(&mut &raw[..], &ReadLimits::default()).expect("reads");
        files += 1;
        let v = file.header.version_text();
        println!("survey: == {name} version {v}");
        let all = |_: &str| true;
        for s in [
            "Mesh",
            "CustomData",
            "CustomDataLayer",
            "MVert",
            "MPoly",
            "MLoop",
            "Material",
            "Camera",
        ] {
            println!("survey: {v} {}", struct_line(&file, s, &all));
        }
        let render = |n: &str| matches!(n, "xsch" | "ysch" | "xasp" | "yasp" | "size");
        println!("survey: {v} {}", struct_line(&file, "RenderData", &render));
        let scene = |n: &str| matches!(n, "r" | "camera");
        println!("survey: {v} {}", struct_line(&file, "Scene", &scene));
        let object = |n: &str| {
            n.starts_with('d')
                || n.starts_with("par")
                || matches!(n, "modifiers" | "type" | "rotmode" | "data")
        };
        println!("survey: {v} {}", struct_line(&file, "Object", &object));
        let mut attr_structs = Vec::new();
        let mut modifier_structs = Vec::new();
        for i in 0..file.sdna.struct_count() as u32 {
            let n = file.sdna.struct_name(i).unwrap_or("");
            if n.contains("Attribute") || n.contains("attribute") {
                attr_structs.push(n.to_owned());
            }
            if n.ends_with("ModifierData") {
                modifier_structs.push(n.to_owned());
            }
        }
        println!("survey: {v} attribute structs: {}", attr_structs.join(" "));
        for s in &attr_structs {
            println!("survey: {v} {}", struct_line(&file, s, &all));
        }
        println!(
            "survey: {v} {} modifier structs: {}",
            modifier_structs.len(),
            modifier_structs.join(" ")
        );
        // every mesh: its own struct dumped (two in full), and a signature counted for all
        let mut sigs: BTreeMap<String, usize> = BTreeMap::new();
        let me_struct = file.sdna.struct_index("Mesh");
        let mut n_me = 0;
        for (i, blk) in file.blocks.iter().enumerate() {
            if &blk.head.code != b"ME\0\0" {
                continue;
            }
            let i = i as u32;
            let Some(ms) = me_struct else { continue };
            n_me += 1;
            let b = body(&file, &raw, i);
            let lines = dump(&file, b, 0, ms, i, 1);
            if n_me <= 2 {
                for l in &lines {
                    if !l.starts_with("id.") {
                        println!("survey: {v} mesh {n_me}: {l}");
                    }
                }
            }
            // the signature: which pointers are set, and each CustomData's layer names
            let mut sig = Vec::new();
            let def = file.sdna.struct_def(ms).unwrap();
            for f in &def.fields {
                let ty = file.sdna.type_name(f.type_index);
                if ty == "CustomData" && !f.is_pointer() {
                    let ls = layers(&file, &raw, i, f.offset as usize);
                    if n_me <= 2 {
                        for l in &ls {
                            println!("survey: {v} mesh {n_me}: {} {l}", f.name);
                        }
                    }
                    let names: Vec<String> = ls
                        .iter()
                        .filter_map(|l| l.split_once("type=").map(|(_, r)| r.to_owned()))
                        .map(|r| {
                            let (t, rest) = r.split_once(' ').unwrap_or((&r, ""));
                            let n = rest
                                .split_once(" data")
                                .map_or(rest, |(n, _)| n)
                                .trim_start_matches("name=");
                            format!("{t}:{n}")
                        })
                        .collect();
                    sig.push(format!("{}[{}]", f.name, names.join(",")));
                } else if ty == "AttributeStorage" && !f.is_pointer() {
                    let ls = attributes(&file, &raw, i, f.offset as usize);
                    if n_me <= 2 {
                        for l in &ls {
                            println!("survey: {v} mesh {n_me}: {} {l}", f.name);
                        }
                    }
                    let names: Vec<String> = ls
                        .iter()
                        .filter_map(|l| l.split_once(": ").map(|(_, r)| r.to_owned()))
                        .filter(|r| r.starts_with("name="))
                        .map(|r| r.split(" data ->").next().unwrap_or("").to_owned())
                        .collect();
                    sig.push(format!("{}[{}]", f.name, names.join(",")));
                } else if f.is_pointer() && f.array_len == 1 && !f.function_pointer {
                    let t = ptr_text(&file, b, f.offset as usize, i);
                    if t != "null" {
                        let kind = t.split(' ').next().unwrap_or("").to_owned();
                        sig.push(format!("*{}->{kind}", f.name));
                    }
                }
            }
            *sigs.entry(sig.join(" ")).or_default() += 1;
        }
        meshes_seen += n_me;
        println!("survey: {v} meshes={n_me}");
        for (s, n) in &sigs {
            println!("survey: {v} signature x{n}: {s}");
        }
        // values seen: cameras' fields, objects' parent types and deltas, materials' flags
        let mut seen: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
        for (i, blk) in file.blocks.iter().enumerate() {
            let code = &blk.head.code;
            let wanted: &[&str] = match code {
                b"CA\0\0" => &[
                    "type",
                    "sensor_fit",
                    "sensor_x",
                    "sensor_y",
                    "shiftx",
                    "shifty",
                ],
                b"OB\0\0" => &["partype", "type"],
                b"MA\0\0" => &["blend_flag", "blend_method"],
                _ => continue,
            };
            let b = body(&file, &raw, i as u32);
            let s = blk.head.sdna;
            for w in wanted {
                let val = file.sdna.field(s, w).map_or("absent".to_owned(), |f| {
                    scalar(
                        &file,
                        b,
                        f.offset as usize,
                        file.sdna.type_name(f.type_index),
                    )
                });
                let code = String::from_utf8_lossy(&code[..2]).into_owned();
                *seen
                    .entry(format!("{code}.{w}"))
                    .or_default()
                    .entry(val)
                    .or_default() += 1;
            }
            if code == b"OB\0\0" {
                // each delta field: whether it holds its identity value
                for (f, ident) in [
                    ("dloc", &[0.0f32; 3][..]),
                    ("drot", &[0.0; 3][..]),
                    ("dscale", &[1.0; 3][..]),
                    ("dsize", &[1.0; 3][..]),
                    ("dquat", &[1.0, 0.0, 0.0, 0.0][..]),
                    ("drotAngle", &[0.0][..]),
                ] {
                    let state = file.sdna.field(s, f).map_or("absent".to_owned(), |fd| {
                        let e = file.endian();
                        let got: Vec<Option<f32>> = (0..fd.array_len as usize)
                            .map(|k| e.f32(b, fd.offset as usize + 4 * k))
                            .collect();
                        let same = got.len() == ident.len()
                            && got.iter().zip(ident).all(|(g, w)| *g == Some(*w));
                        if same { "identity" } else { "other" }.to_owned()
                    });
                    *seen
                        .entry(format!("OB.{f}"))
                        .or_default()
                        .entry(state)
                        .or_default() += 1;
                }
            }
        }
        for (k, vals) in &seen {
            let vs: Vec<String> = vals.iter().map(|(v, n)| format!("{v} x{n}")).collect();
            println!("survey: {v} values {k}: {}", vs.join(", "));
        }
        let data_blocks = file.blocks.iter().filter(|b| b.head.code == DATA).count();
        println!("survey: {v} data blocks={data_blocks}");
    }
    assert!(files >= 7, "every manifest file surveyed: {files}");
    assert!(meshes_seen > 0, "meshes surveyed: {meshes_seen}");
    println!("survey: done, {files} files, {meshes_seen} meshes");
}

#[test]
#[ignore = "the public corpus: scripts/public-corpus.sh runs it"]
fn every_corpus_mesh_reads_from_its_layout() {
    let dir = corpus();
    let (mut files, mut read, mut empty, mut agreed) = (0, 0, 0, 0);
    for name in names() {
        let bytes = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (_, raw) = decompress(&bytes, &ReadLimits::default()).expect("decompresses");
        let file = read_blend(&mut &raw[..], &ReadLimits::default()).expect("reads");
        let v = file.header.version_text();
        let want = match v.as_str() {
            "2.73" | "3.06" => Layout::Structs,
            "4.03" => Layout::Layers,
            _ => Layout::Store,
        };
        let (mut n, mut ok, mut tris, mut verts, mut kept) = (0, 0, 0, 0u64, 0u64);
        let mut counts = TriCounts::default();
        for (i, b) in file.blocks.iter().enumerate() {
            if &b.head.code != b"ME\0\0" {
                continue;
            }
            let i = i as u32;
            n += 1;
            let c = mesh::counts(&file, &raw, i).unwrap_or_else(|e| panic!("{name}: counts {e:?}"));
            let found = mesh::layouts(&file, &raw, i);
            if found.len() > 1 {
                let g: Vec<_> = found
                    .iter()
                    .map(|&l| {
                        mesh::read_geometry(&file, &raw, i, l)
                            .map(|g| (g.positions, g.corners, g.faces))
                    })
                    .collect();
                assert!(
                    g.windows(2).all(|w| w[0] == w[1]),
                    "{name}: mesh {n} alike from {found:?}"
                );
                agreed += 1;
            }
            match mesh::read_mesh(&file, &raw, i, 4096, &mut counts) {
                Ok((l, m)) => {
                    assert_eq!(l, want, "{name}: mesh {n}'s layout");
                    assert_eq!(
                        m.triangles.len() as u64,
                        c.triangles(),
                        "{name}: mesh {n}: n - 2 each"
                    );
                    ok += 1;
                    tris += m.triangles.len();
                    verts += u64::from(c.vertices);
                    kept += m.positions.len() as u64;
                }
                Err(MeshSkip::NoTriangles) => {
                    assert_eq!(
                        c.faces, 0,
                        "{name}: mesh {n} has polygons but nothing to draw"
                    );
                    empty += 1;
                }
                Err(e) => panic!("{name}: mesh {n} ({} faces) refused: {e:?}", c.faces),
            }
        }
        assert!(n > 0, "{name}: meshes");
        println!(
            "layouts: {name} {v}: {n} meshes, {ok} read from {}, {tris} triangles, {kept} of {verts} vertices kept, {counts:?}",
            want.word()
        );
        files += 1;
        read += ok;
    }
    assert!(files >= 7 && read > 0, "files {files}, meshes read {read}");
    println!("layouts: done, {files} files, {read} meshes read, {empty} with no polygon, {agreed} in two layouts");
}

/// The world matrix object block `b` caches, column-major (the oracle only).
fn cached(file: &BlendFile, raw: &[u8], b: u32) -> Option<[f64; 16]> {
    let s = file.blocks[b as usize].head.sdna;
    let body = body(file, raw, b);
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

#[test]
#[ignore = "the public corpus: scripts/public-corpus.sh runs it"]
fn every_corpus_scene_reads_and_matches_cached_matrices() {
    let dir = corpus();
    let limits = MeshReadLimits::default();
    let (mut read, mut compared, mut matched, mut worst) = (0, 0, 0, 0f64);
    for name in names() {
        let bytes = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let r = match mesh_read(&bytes, &limits) {
            Ok(r) => r,
            Err(MeshReadError::Declined { code }) => {
                println!("scenes: {name}: declined {code}");
                assert_eq!(
                    code, "live_version",
                    "{name}: only an old version is declined"
                );
                continue;
            }
            Err(e) => panic!("{name}: {e}"),
        };
        let c = check(&r.k2scene, &limits).unwrap_or_else(|e| panic!("{name}: the checker: {e}"));
        let rep = &r.report;
        assert_eq!(
            rep.scene_from, "current",
            "{name}: the file names its current scene"
        );
        let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
        for s in &rep.skipped {
            *reasons.entry(s.reason).or_default() += 1;
        }
        println!(
            "scenes: {name}: scene {:?} ({}), {} meshes, {} objects, {} hidden, {} not evaluated, \
             {} collection instances, left out {reasons:?}, {} bytes",
            rep.scene.as_deref().unwrap_or("-"),
            rep.scene_from,
            rep.meshes,
            rep.objects,
            rep.hidden,
            rep.transform_not_evaluated,
            rep.collection_instances,
            r.k2scene.len()
        );
        assert_eq!(c.scene.objects.len() as u64, rep.objects);
        // the oracle: the matrices a 3.x save caches
        let (_, raw) = decompress(&bytes, &ReadLimits::default()).expect("decompresses");
        let file = read_blend(&mut &raw[..], &ReadLimits::default()).expect("reads");
        let model = read_scene(&file).expect("models");
        for o in &c.scene.objects {
            let Some(id) = model.ids.iter().find(|i| {
                matches!(i.detail, Detail::Object { .. })
                    && i.struct_name == "Object"
                    && i.library.is_none()
                    && i.name == o.id
            }) else {
                panic!("{name}: no object {}", o.id);
            };
            let Some(want) = cached(&file, &raw, id.block) else {
                continue;
            };
            compared += 1;
            let mut d: f64 = 0.0;
            let mut scale: f64 = 1.0;
            for (&g, &w) in o.transform.iter().zip(&want) {
                let got = f64::from(g);
                d = d.max((got - w).abs());
                scale = scale.max(got.abs()).max(w.abs());
            }
            if d <= 1e-5 * scale {
                matched += 1;
                worst = worst.max(d / scale);
            } else {
                println!(
                    "scenes: {name}: {} differs from its cached matrix by {d:.3e}",
                    o.id
                );
            }
        }
        read += 1;
    }
    println!(
        "scenes: done, {read} files read; cached matrices compared {compared}, matched {matched}, \
         worst {worst:.3e} of scale"
    );
    assert!(read >= 6, "files read: {read}");
    assert!(compared > 0, "no drawn object caches a matrix");
    assert_eq!(
        matched, compared,
        "every composed transform matches its cached matrix"
    );
}
