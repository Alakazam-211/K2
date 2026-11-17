//! The test scenes' rows shared by `tests/scenes.rs`, the public-corpus rows in
//! `tests/corpus.rs` and the planted bugs in `tests/mutants.rs`: each returns
//! `Err("<row>: <step>: ...")`.
#![allow(dead_code)]

use k2_mpc_blend::testgen::{scenes, EMBEDDED_ID_FLAG};
use k2_mpc_blend::write::decompress;
use k2_mpc_blend::{read_blend, BlendFile, ReadLimits, Target};

/// One embedded ID of a file: its struct, its `id.flag`, and whether a scene's
/// `master_collection` points at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Embedded {
    /// Its struct's name (`Collection`, `bNodeTree`).
    pub struct_name: String,
    /// Its ID header's `flag`.
    pub flag: i16,
    /// A scene's master collection.
    pub master: bool,
}

/// A file's embedded IDs, as [`embedded_ids`] finds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIds {
    /// Its version (`402`).
    pub version: u16,
    /// Its scenes.
    pub scenes: usize,
    /// Its embedded IDs, in file order.
    pub ids: Vec<Embedded>,
}

/// A file's embedded IDs, read by the file's own schema: every `DATA` block whose struct embeds
/// an `ID` as its field `id` (a scene's master collection, a node tree inside another
/// data-block); and its version and scene count.
pub fn embedded_ids(bytes: &[u8]) -> Result<FileIds, String> {
    let limits = ReadLimits::default();
    let (_, raw) = decompress(bytes, &limits).map_err(|e| format!("decompress: {e}"))?;
    let file = read_blend(&mut &raw[..], &limits).map_err(|e| format!("read: {e}"))?;
    let masters = masters(&file, &raw);
    let e = file.endian();
    let mut out = Vec::new();
    for (i, b) in file.blocks.iter().enumerate() {
        let h = &b.head;
        if &h.code != b"DATA" || h.sdna == 0 {
            continue;
        }
        let Some(f) = file.sdna.field(h.sdna, "id") else {
            continue;
        };
        if f.is_pointer() || file.sdna.type_name(f.type_index) != "ID" {
            continue;
        }
        let Some((at, _)) = file.sdna.field_path(h.sdna, "id.flag") else {
            return Err(format!("block {i}: an ID without a flag"));
        };
        let flag = e
            .i16(&raw, (h.body_offset + u64::from(at)) as usize)
            .ok_or_else(|| format!("block {i}: its flag is past the file"))?;
        out.push(Embedded {
            struct_name: file.sdna.struct_name(h.sdna).unwrap_or("?").to_owned(),
            flag,
            master: masters.contains(&(i as u32)),
        });
    }
    let scenes = file
        .blocks
        .iter()
        .filter(|b| &b.head.code == b"SC\0\0")
        .count();
    Ok(FileIds {
        version: file.header.version,
        scenes,
        ids: out,
    })
}

/// The blocks the scenes' `master_collection` pointers name.
fn masters(file: &BlendFile, raw: &[u8]) -> Vec<u32> {
    let e = file.endian();
    let mut out = Vec::new();
    for (i, b) in file.blocks.iter().enumerate() {
        let h = &b.head;
        if &h.code != b"SC\0\0" {
            continue;
        }
        let Some(f) = file.sdna.field(h.sdna, "master_collection") else {
            continue;
        };
        let p = e
            .ptr(raw, (h.body_offset + u64::from(f.offset)) as usize)
            .unwrap_or(0);
        if let Target::Block(m) = file.resolve(p, i as u32) {
            out.push(m);
        }
    }
    out
}

/// Checks that `ids` (of a file with `scenes` scenes) has one master collection per scene and
/// that every embedded ID carries [`EMBEDDED_ID_FLAG`].
pub fn check_embedded(what: &str, ids: &[Embedded], scenes: usize) -> Result<(), String> {
    let masters = ids.iter().filter(|e| e.master).count();
    if scenes == 0 || masters != scenes {
        return Err(format!(
            "{what}: {masters} master collections for {scenes} scenes"
        ));
    }
    if let Some(bad) = ids.iter().find(|e| e.flag & EMBEDDED_ID_FLAG == 0) {
        return Err(format!(
            "{what}: an embedded {} without the embedded flag (flag {})",
            bad.struct_name, bad.flag
        ));
    }
    Ok(())
}

/// Every generated scene's embedded IDs (each scene's master collection, and any other the
/// generator writes) carry [`EMBEDDED_ID_FLAG`], as the program's own saves do.
pub fn row_embedded_flag() -> Result<(), String> {
    const ROW: &str = "row_embedded_flag";
    let mut n = 0;
    for (name, write) in scenes::ALL {
        let f = embedded_ids(&write()).map_err(|e| format!("{ROW}: {name}: {e}"))?;
        check_embedded(&format!("{ROW}: {name}"), &f.ids, f.scenes)?;
        n += f.ids.len();
    }
    println!(
        "embedded flag: {} files, {n} embedded IDs, each flagged",
        scenes::ALL.len()
    );
    Ok(())
}

/// Block `i`'s body.
fn body<'a>(file: &BlendFile, raw: &'a [u8], i: u32) -> &'a [u8] {
    let h = &file.blocks[i as usize].head;
    &raw[h.body_offset as usize..(h.body_offset + h.len) as usize]
}

/// Block `i`'s struct name.
fn struct_of(file: &BlendFile, i: u32) -> &str {
    file.sdna
        .struct_name(file.blocks[i as usize].head.sdna)
        .unwrap_or("?")
}

/// The block pointer field `path` of block `i` names, if any.
fn ptr_field(file: &BlendFile, raw: &[u8], i: u32, path: &str) -> Option<u32> {
    let s = file.blocks[i as usize].head.sdna;
    let (at, _) = file.sdna.field_path(s, path)?;
    let p = file.endian().ptr(body(file, raw, i), at as usize)?;
    match file.resolve(p, i) {
        Target::Block(b) => Some(b),
        _ => None,
    }
}

/// The element blocks of the list `path` of block `i` (each element's first field its `next`).
fn list(file: &BlendFile, raw: &[u8], i: u32, path: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let s = file.blocks[i as usize].head.sdna;
    let Some((at, _)) = file.sdna.field_path(s, path) else {
        return out;
    };
    let e = file.endian();
    let mut p = e.ptr(body(file, raw, i), at as usize).unwrap_or(0);
    let mut from = i;
    while p != 0 && out.len() < 1 << 16 {
        let Target::Block(x) = file.resolve(p, from) else {
            break;
        };
        out.push(x);
        p = e.ptr(body(file, raw, x), 0).unwrap_or(0);
        from = x;
    }
    out
}

/// A text field of block `i`.
fn text(file: &BlendFile, raw: &[u8], i: u32, path: &str) -> String {
    let s = file.blocks[i as usize].head.sdna;
    let Some((at, f)) = file.sdna.field_path(s, path) else {
        return String::new();
    };
    let b = &body(file, raw, i)[at as usize..(at + f.size) as usize];
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// The collection blocks below collection block `c` (its `children`), in order.
fn children(file: &BlendFile, raw: &[u8], c: u32) -> Vec<u32> {
    list(file, raw, c, "children")
        .into_iter()
        .filter_map(|x| ptr_field(file, raw, x, "collection"))
        .collect()
}

/// Checks that layer collection `lc` mirrors collection `c` and its children, recursively;
/// counts the layer collections.
fn mirror(file: &BlendFile, raw: &[u8], lc: u32, c: u32, n: &mut usize) -> Result<(), String> {
    *n += 1;
    if struct_of(file, lc) != "LayerCollection" {
        return Err(format!("a {} in the layer tree", struct_of(file, lc)));
    }
    if ptr_field(file, raw, lc, "collection") != Some(c) {
        return Err(format!(
            "a layer collection not on {}",
            text(file, raw, c, "id.name")
        ));
    }
    let kids = list(file, raw, lc, "layer_collections");
    let want = children(file, raw, c);
    if kids.len() != want.len() {
        return Err(format!(
            "{}: {} layer collections for {} children",
            text(file, raw, c, "id.name"),
            kids.len(),
            want.len()
        ));
    }
    for (k, w) in kids.into_iter().zip(want) {
        mirror(file, raw, k, w, n)?;
    }
    Ok(())
}

/// The object blocks collection block `c` holds, through its children too, each once.
fn objects_below(file: &BlendFile, raw: &[u8], c: u32, out: &mut Vec<u32>) {
    for x in list(file, raw, c, "gobject") {
        if let Some(o) = ptr_field(file, raw, x, "ob") {
            if !out.contains(&o) {
                out.push(o);
            }
        }
    }
    for k in children(file, raw, c) {
        objects_below(file, raw, k, out);
    }
}

/// What a file's scenes hold beyond the scene graph, as the program's own saves hold it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SavedCounts {
    /// Scenes checked.
    pub scenes: usize,
    /// Layer collections, over every view layer.
    pub layer_collections: usize,
    /// Bases, over every view layer.
    pub bases: usize,
}

/// Checks every scene of `bytes`: its tool settings (a `ToolSettings` block), its render views
/// (`r.views`: `left` and `right`), one view layer (`view_layers`) whose layer collections
/// mirror the master collection and its children, and whose bases are the objects the scene
/// holds, each once.
pub fn saved_structs(bytes: &[u8]) -> Result<SavedCounts, String> {
    let limits = ReadLimits::default();
    let (_, raw) = decompress(bytes, &limits).map_err(|e| format!("decompress: {e}"))?;
    let file = read_blend(&mut &raw[..], &limits).map_err(|e| format!("read: {e}"))?;
    let mut c = SavedCounts::default();
    for i in 0..file.blocks.len() as u32 {
        if &file.blocks[i as usize].head.code != b"SC\0\0" {
            continue;
        }
        c.scenes += 1;
        let scene = text(&file, &raw, i, "id.name");
        let ts = ptr_field(&file, &raw, i, "toolsettings")
            .ok_or_else(|| format!("{scene}: no tool settings"))?;
        if struct_of(&file, ts) != "ToolSettings" {
            return Err(format!(
                "{scene}: tool settings are a {}",
                struct_of(&file, ts)
            ));
        }
        let views: Vec<String> = list(&file, &raw, i, "r.views")
            .into_iter()
            .map(|v| text(&file, &raw, v, "name"))
            .collect();
        if views != ["left", "right"] {
            return Err(format!("{scene}: render views {views:?}"));
        }
        let master = ptr_field(&file, &raw, i, "master_collection")
            .ok_or_else(|| format!("{scene}: no master collection"))?;
        let layers = list(&file, &raw, i, "view_layers");
        if layers.len() != 1 || struct_of(&file, layers[0]) != "ViewLayer" {
            return Err(format!("{scene}: {} view layers", layers.len()));
        }
        let vl = layers[0];
        let roots = list(&file, &raw, vl, "layer_collections");
        if roots.len() != 1 {
            return Err(format!("{scene}: {} root layer collections", roots.len()));
        }
        mirror(&file, &raw, roots[0], master, &mut c.layer_collections)
            .map_err(|e| format!("{scene}: {e}"))?;
        let bases: Vec<u32> = list(&file, &raw, vl, "object_bases")
            .into_iter()
            .map(|b| {
                ptr_field(&file, &raw, b, "object")
                    .ok_or_else(|| format!("{scene}: a base of nothing"))
            })
            .collect::<Result<_, _>>()?;
        let mut want = Vec::new();
        objects_below(&file, &raw, master, &mut want);
        let (mut got, mut held) = (bases.clone(), want.clone());
        got.sort_unstable();
        held.sort_unstable();
        if got != held || bases.len() != want.len() {
            return Err(format!(
                "{scene}: {} bases for {} objects held",
                bases.len(),
                want.len()
            ));
        }
        c.bases += bases.len();
    }
    if c.scenes == 0 {
        return Err("no scene".into());
    }
    Ok(c)
}

/// Every generated scene holds its tool settings, render views, view layer, layer collections
/// and bases ([`saved_structs`]), as every scene of the program's own saves does.
pub fn row_saved_structs() -> Result<(), String> {
    const ROW: &str = "row_saved_structs";
    let mut total = SavedCounts::default();
    for (name, write) in scenes::ALL {
        let c = saved_structs(&write()).map_err(|e| format!("{ROW}: {name}: {e}"))?;
        if c.layer_collections == 0 || c.bases == 0 {
            return Err(format!("{ROW}: {name}: nothing counted: {c:?}"));
        }
        total.scenes += c.scenes;
        total.layer_collections += c.layer_collections;
        total.bases += c.bases;
    }
    println!(
        "saved structs: {} files, {} scenes, {} layer collections, {} bases",
        scenes::ALL.len(),
        total.scenes,
        total.layer_collections,
        total.bases
    );
    Ok(())
}
