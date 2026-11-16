//! A `Mesh` data-block's positions and polygons, read by the file's own schema, then
//! triangulated ([`super::tri`]) and compacted to the vertices a triangle uses.
//!
//! **Three layouts**, told apart by what the mesh's struct and blocks hold, never by the version
//! number (`PROVENANCE.md`, "Meshes", has what each sample file holds):
//!
//! - **Attribute layers** ([`Layout::Layers`]): the per-vertex layers (`vdata`) hold one named
//!   `position` (three `f32` per vertex), the per-corner layers (`ldata`) one named
//!   `.corner_vert` (an `i32` vertex index per corner), and `poly_offset_indices` gives each
//!   polygon's first corner (`faces + 1` of them, the last the corner count).
//! - **Vertex, polygon and corner structs** ([`Layout::Structs`]): `mvert` (`co`, three `f32`),
//!   `mpoly` (`loopstart`, `totloop`) and `mloop` (`v`), each a block of its struct.
//! - **The attribute store** ([`Layout::Store`]): `attribute_storage` lists `Attribute`s by
//!   name, each pointing at an `AttributeArray` (its `data` and its `size`); `position` and
//!   `.corner_vert` as above, with `poly_offset_indices`.
//!
//! The first layout the mesh holds whole is read, in that order: a file saved with two of them
//! (the attribute layers and the structs) reads by its layers.
//!
//! **Counts first.** A mesh's vertex, polygon and corner counts come from its own struct; each
//! array's block must be a data block of exactly the length its count gives (and, when the
//! block names its struct, of that struct's size per element) before anything is allocated for
//! it. Each refusal is a typed [`MeshSkip`]: a mesh that can't be read is skipped and named,
//! never guessed at.
//!
//! **Loose vertices are dropped.** Only the vertices a triangle uses are kept, in file order,
//! and the triangles' indices renumbered to match; a mesh with no triangle left has nothing to
//! draw ([`MeshSkip::NoTriangles`]).

use crate::meshread::k2scene::SceneMesh;
use crate::meshread::tri::{triangulate_face, TriCounts};
use crate::mutant;
use crate::reader::{BlendFile, Target, DATA};
use crate::sdna::Field;

/// Where a mesh's geometry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Named attribute layers (`vdata`, `ldata`) and polygon offsets.
    Layers,
    /// The vertex, polygon and corner structs (`mvert`, `mpoly`, `mloop`).
    Structs,
    /// The attribute store (`attribute_storage`) and polygon offsets.
    Store,
}

impl Layout {
    /// Every layout, in the order they are tried.
    pub const ALL: [Layout; 3] = [Layout::Layers, Layout::Structs, Layout::Store];

    /// Its word.
    pub fn word(self) -> &'static str {
        match self {
            Self::Layers => "layers",
            Self::Structs => "structs",
            Self::Store => "store",
        }
    }
}

/// Why a mesh is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MeshSkip {
    /// Not a mesh data-block this reader knows, or it holds no layout whole.
    LayoutUnknown,
    /// A count is negative, or the struct has none.
    Counts,
    /// An array's pointer names no single data block.
    ArrayUnresolved(&'static str),
    /// An array's block isn't the length its count gives.
    ArrayLength(&'static str),
    /// A corner names a vertex past the mesh's vertices.
    CornerOutOfRange,
    /// The polygons' corners don't add up: offsets out of order or not ending at the corner
    /// count, or a polygon past the corners.
    Offsets,
    /// A polygon of fewer than 3 corners.
    FaceTooSmall,
    /// A position isn't finite.
    PositionNotFinite,
    /// No triangle to draw (no polygon).
    NoTriangles,
}

impl MeshSkip {
    /// Its word in a read's report.
    pub fn word(self) -> &'static str {
        match self {
            Self::LayoutUnknown => "mesh_layout_unknown",
            Self::Counts => "mesh_invalid:counts",
            Self::ArrayUnresolved(_) => "mesh_invalid:array_unresolved",
            Self::ArrayLength(_) => "mesh_invalid:array_length",
            Self::CornerOutOfRange => "mesh_invalid:corner_out_of_range",
            Self::Offsets => "mesh_invalid:offsets",
            Self::FaceTooSmall => "mesh_invalid:face_too_small",
            Self::PositionNotFinite => "mesh_invalid:position_not_finite",
            Self::NoTriangles => "no_triangles",
        }
    }
}

/// A mesh's counts, from its own struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshCounts {
    /// Vertices.
    pub vertices: u32,
    /// Polygons.
    pub faces: u32,
    /// Corners.
    pub corners: u32,
}

impl MeshCounts {
    /// The triangles the polygons give, `corners - 2 * faces`, when every polygon has at least
    /// 3 corners (each of `n` corners gives `n - 2`).
    pub fn triangles(&self) -> u64 {
        u64::from(self.corners).saturating_sub(2 * u64::from(self.faces))
    }
}

/// A mesh's geometry, as read: positions, corners (vertex indices), and each polygon's
/// `(first corner, corner count)`, in the file's order.
#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    /// The layout it was read from.
    pub layout: Layout,
    /// Every vertex's position.
    pub positions: Vec<[f32; 3]>,
    /// Every corner's vertex.
    pub corners: Vec<u32>,
    /// Every polygon's first corner and corner count.
    pub faces: Vec<[u32; 2]>,
}

/// The schema field `names[..]` of struct `s` (the first it has), if of type `ty` and not an
/// array.
fn scalar_field<'a>(file: &'a BlendFile, s: u32, names: &[&str], ty: &str) -> Option<&'a Field> {
    let f = file.sdna.field_any(s, names)?;
    (!f.is_pointer() && f.array_len == 1 && file.sdna.type_name(f.type_index) == ty).then_some(f)
}

/// A pointer field `names[..]` of struct `s`.
fn pointer_field<'a>(file: &'a BlendFile, s: u32, names: &[&str]) -> Option<&'a Field> {
    let f = file.sdna.field_any(s, names)?;
    (f.pointer_depth == 1 && !f.function_pointer && f.array_len == 1).then_some(f)
}

/// An embedded struct field `names[..]` of struct `s`, of struct type `ty`: its offset and the
/// embedded struct's index.
fn embedded(file: &BlendFile, s: u32, names: &[&str], ty: &str) -> Option<(u32, u32)> {
    let f = file.sdna.field_any(s, names)?;
    if f.is_pointer() || f.array_len != 1 || file.sdna.type_name(f.type_index) != ty {
        return None;
    }
    Some((f.offset, file.sdna.struct_of_type(f.type_index)?))
}

/// Block `i`'s body in the decompressed file.
pub(crate) fn block_body<'a>(file: &BlendFile, raw: &'a [u8], i: u32) -> Option<&'a [u8]> {
    let h = &file.blocks.get(i as usize)?.head;
    let start = usize::try_from(h.body_offset).ok()?;
    let len = usize::try_from(h.len).ok()?;
    raw.get(start..start.checked_add(len)?)
}

/// The mesh block's own struct and body.
fn mesh_struct<'a>(file: &BlendFile, raw: &'a [u8], me: u32) -> Result<(u32, &'a [u8]), MeshSkip> {
    let blk = file
        .blocks
        .get(me as usize)
        .ok_or(MeshSkip::LayoutUnknown)?;
    let s = blk.head.sdna;
    if !blk.is_id() || file.sdna.struct_name(s) != Some("Mesh") {
        return Err(MeshSkip::LayoutUnknown);
    }
    let body = block_body(file, raw, me).ok_or(MeshSkip::LayoutUnknown)?;
    let size = file.sdna.struct_def(s).map_or(0, |d| d.size as usize);
    if body.len() < size {
        return Err(MeshSkip::LayoutUnknown);
    }
    Ok((s, body))
}

/// The counts of mesh block `me`, read from its own struct: no array is looked at.
pub fn counts(file: &BlendFile, raw: &[u8], me: u32) -> Result<MeshCounts, MeshSkip> {
    let (s, body) = mesh_struct(file, raw, me)?;
    let e = file.endian();
    let read = |names: &[&str]| -> Result<u32, MeshSkip> {
        let f = scalar_field(file, s, names, "int").ok_or(MeshSkip::Counts)?;
        let v = e.i32(body, f.offset as usize).ok_or(MeshSkip::Counts)?;
        u32::try_from(v).map_err(|_| MeshSkip::Counts)
    };
    Ok(MeshCounts {
        vertices: read(&["totvert", "verts_num"])?,
        faces: read(&["totpoly", "faces_num"])?,
        corners: read(&["totloop", "corners_num"])?,
    })
}

/// Where a pointer is: the body holding it, its offset there, and that body's block.
#[derive(Clone, Copy)]
struct At<'b> {
    body: &'b [u8],
    at: usize,
    block: u32,
}

/// The pointer at `ptr`, resolved to a data block whose body
/// holds exactly `count` elements of `elem` bytes; with `count == 0`, an empty array whatever
/// the pointer. When the block names its struct, that struct must be `elem` bytes.
fn array<'a>(
    file: &BlendFile,
    raw: &'a [u8],
    ptr: At<'_>,
    what: &'static str,
    elem: usize,
    count: u32,
) -> Result<&'a [u8], MeshSkip> {
    if count == 0 {
        return Ok(&[]);
    }
    let p = file
        .endian()
        .ptr(ptr.body, ptr.at)
        .ok_or(MeshSkip::ArrayUnresolved(what))?;
    let Target::Block(j) = file.resolve(p, ptr.block) else {
        return Err(MeshSkip::ArrayUnresolved(what));
    };
    let head = &file.blocks[j as usize].head;
    if head.code != DATA {
        return Err(MeshSkip::ArrayUnresolved(what));
    }
    if head.sdna != 0 && file.sdna.struct_def(head.sdna).map(|d| d.size as usize) != Some(elem) {
        return Err(MeshSkip::ArrayLength(what));
    }
    let need = (count as u64)
        .checked_mul(elem as u64)
        .ok_or(MeshSkip::ArrayLength(what))?;
    if head.len != need && !mutant::active("CountsTrusted") {
        return Err(MeshSkip::ArrayLength(what));
    }
    let start = usize::try_from(head.body_offset).map_err(|_| MeshSkip::ArrayLength(what))?;
    let need = usize::try_from(need).map_err(|_| MeshSkip::ArrayLength(what))?;
    raw.get(start..start.saturating_add(need))
        .ok_or(MeshSkip::ArrayLength(what))
}

/// `n` positions, three `f32` at `off` of each `stride`-byte element of `b`.
fn positions(file: &BlendFile, b: &[u8], n: u32, stride: usize, off: usize) -> Vec<[f32; 3]> {
    let e = file.endian();
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n as usize {
        let at = i * stride + off;
        let f = |k: usize| e.f32(b, at + 4 * k).unwrap_or(f32::NAN);
        out.push([f(0), f(1), f(2)]);
    }
    out
}

/// `n` `i32`s at `off` of each `stride`-byte element of `b`, as vertex indices below
/// `vertices`.
fn indices(
    file: &BlendFile,
    b: &[u8],
    n: u32,
    stride: usize,
    off: usize,
    vertices: u32,
) -> Result<Vec<u32>, MeshSkip> {
    let e = file.endian();
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n as usize {
        let v = e
            .i32(b, i * stride + off)
            .ok_or(MeshSkip::CornerOutOfRange)?;
        match u32::try_from(v) {
            Ok(v) if v < vertices => out.push(v),
            _ => return Err(MeshSkip::CornerOutOfRange),
        }
    }
    Ok(out)
}

/// Polygons from `faces + 1` offsets into `corners` corners: from 0, in order, each of at least
/// 3 corners, ending at `corners`.
fn faces_from_offsets(
    file: &BlendFile,
    raw: &[u8],
    body: &[u8],
    s: u32,
    me: u32,
    c: MeshCounts,
) -> Result<Vec<[u32; 2]>, MeshSkip> {
    if c.faces == 0 {
        return Ok(Vec::new());
    }
    let f = pointer_field(file, s, &["poly_offset_indices", "face_offset_indices"])
        .ok_or(MeshSkip::LayoutUnknown)?;
    let at = At {
        body,
        at: f.offset as usize,
        block: me,
    };
    let b = array(file, raw, at, "polygon offsets", 4, c.faces + 1)?;
    let e = file.endian();
    let at = |i: usize| e.i32(b, 4 * i).unwrap_or(-1);
    if at(0) != 0 || at(c.faces as usize) != c.corners as i32 {
        return Err(MeshSkip::Offsets);
    }
    let mut out = Vec::with_capacity(c.faces as usize);
    for i in 0..c.faces as usize {
        let (a, z) = (at(i), at(i + 1));
        if z < a {
            return Err(MeshSkip::Offsets);
        }
        if z - a < 3 {
            return Err(MeshSkip::FaceTooSmall);
        }
        out.push([a as u32, (z - a) as u32]);
    }
    Ok(out)
}

/// The data pointer's place (offset in its block's body, the block) of the layer named `name`
/// among the layers of the `CustomData` embedded at `cd_at` of the mesh's body.
fn layer(
    file: &BlendFile,
    raw: &[u8],
    body: &[u8],
    me: u32,
    cd: (u32, u32),
    name: &str,
) -> Option<(usize, u32)> {
    let sd = &file.sdna;
    let e = file.endian();
    let (cd_at, cd_s) = (cd.0 as usize, cd.1);
    let layers = pointer_field(file, cd_s, &["layers"])?;
    let tot = scalar_field(file, cd_s, &["totlayer"], "int")?;
    let lay = sd.struct_index("CustomDataLayer")?;
    let size = sd.struct_def(lay)?.size as usize;
    let lname = sd.field(lay, "name")?;
    let ldata = pointer_field(file, lay, &["data"])?;
    let n = usize::try_from(e.i32(body, cd_at + tot.offset as usize)?).ok()?;
    let Target::Block(lb) = file.resolve(e.ptr(body, cd_at + layers.offset as usize)?, me) else {
        return None;
    };
    let lbody = block_body(file, raw, lb)?;
    if file.blocks[lb as usize].head.code != DATA || n.checked_mul(size)? > lbody.len() {
        return None;
    }
    (0..n).find_map(|k| {
        let base = k * size;
        let nm =
            lbody.get(base + lname.offset as usize..base + (lname.offset + lname.size) as usize)?;
        let end = nm.iter().position(|&c| c == 0).unwrap_or(nm.len());
        (&nm[..end] == name.as_bytes()).then_some((base + ldata.offset as usize, lb))
    })
}

/// The `AttributeArray` place (its `data` pointer's offset in its body, the block, and its
/// `size`) of the attribute named `name` in the `AttributeStorage` embedded in the mesh.
fn stored(
    file: &BlendFile,
    raw: &[u8],
    body: &[u8],
    me: u32,
    st: (u32, u32),
    name: &str,
) -> Option<(usize, u32, i64)> {
    let sd = &file.sdna;
    let e = file.endian();
    let (st_at, st_s) = (st.0 as usize, st.1);
    let list = pointer_field(file, st_s, &["dna_attributes"])?;
    let num = scalar_field(file, st_s, &["dna_attributes_num"], "int")?;
    let attr = sd.struct_index("Attribute")?;
    let size = sd.struct_def(attr)?.size as usize;
    let aname = pointer_field(file, attr, &["name"])?;
    let adata = pointer_field(file, attr, &["data"])?;
    let arr = sd.struct_index("AttributeArray")?;
    let arr_data = pointer_field(file, arr, &["data"])?;
    let arr_size = scalar_field(file, arr, &["size"], "int64_t")?;
    let n = usize::try_from(e.i32(body, st_at + num.offset as usize)?).ok()?;
    let Target::Block(lb) = file.resolve(e.ptr(body, st_at + list.offset as usize)?, me) else {
        return None;
    };
    let lbody = block_body(file, raw, lb)?;
    if file.blocks[lb as usize].head.code != DATA || n.checked_mul(size)? > lbody.len() {
        return None;
    }
    (0..n).find_map(|k| {
        let base = k * size;
        let Target::Block(nb) = file.resolve(e.ptr(lbody, base + aname.offset as usize)?, lb)
        else {
            return None;
        };
        let nm = block_body(file, raw, nb)?;
        let end = nm.iter().position(|&c| c == 0)?;
        if &nm[..end] != name.as_bytes() {
            return None;
        }
        let Target::Block(ab) = file.resolve(e.ptr(lbody, base + adata.offset as usize)?, lb)
        else {
            return None;
        };
        let ah = &file.blocks[ab as usize].head;
        if ah.code != DATA || ah.sdna != arr {
            return None;
        }
        let abody = block_body(file, raw, ab)?;
        let sz = e.i64(abody, arr_size.offset as usize)?;
        Some((arr_data.offset as usize, ab, sz))
    })
}

/// Which layouts mesh block `me` holds whole (its struct has the fields, and the named layers
/// or attributes are there), in the order they are tried.
pub fn layouts(file: &BlendFile, raw: &[u8], me: u32) -> Vec<Layout> {
    let Ok((s, body)) = mesh_struct(file, raw, me) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let vd = embedded(file, s, &["vdata", "vert_data"], "CustomData");
    let ld = embedded(file, s, &["ldata", "corner_data"], "CustomData");
    if let (Some(vd), Some(ld)) = (vd, ld) {
        if layer(file, raw, body, me, vd, "position").is_some()
            && layer(file, raw, body, me, ld, ".corner_vert").is_some()
        {
            out.push(Layout::Layers);
        }
    }
    let e = file.endian();
    let set = |names: &[&str]| {
        pointer_field(file, s, names)
            .and_then(|f| e.ptr(body, f.offset as usize))
            .is_some_and(|p| p != 0)
    };
    if set(&["mvert"]) && set(&["mpoly"]) && set(&["mloop"]) {
        out.push(Layout::Structs);
    }
    if let Some(st) = embedded(file, s, &["attribute_storage"], "AttributeStorage") {
        if stored(file, raw, body, me, st, "position").is_some()
            && stored(file, raw, body, me, st, ".corner_vert").is_some()
        {
            out.push(Layout::Store);
        }
    }
    out
}

/// Reads mesh block `me`'s geometry from `layout`: every array checked against its block's
/// length before it is read, every corner against the vertices, every polygon's corners.
pub fn read_geometry(
    file: &BlendFile,
    raw: &[u8],
    me: u32,
    layout: Layout,
) -> Result<Geometry, MeshSkip> {
    let (s, body) = mesh_struct(file, raw, me)?;
    let c = counts(file, raw, me)?;
    let sd = &file.sdna;
    let (positions, corners, faces) = match layout {
        Layout::Layers => {
            let vd = embedded(file, s, &["vdata", "vert_data"], "CustomData")
                .ok_or(MeshSkip::LayoutUnknown)?;
            let ld = embedded(file, s, &["ldata", "corner_data"], "CustomData")
                .ok_or(MeshSkip::LayoutUnknown)?;
            let (pat, pb) =
                layer(file, raw, body, me, vd, "position").ok_or(MeshSkip::LayoutUnknown)?;
            let (cat, cb) =
                layer(file, raw, body, me, ld, ".corner_vert").ok_or(MeshSkip::LayoutUnknown)?;
            let pbody = block_body(file, raw, pb).ok_or(MeshSkip::LayoutUnknown)?;
            let cbody = block_body(file, raw, cb).ok_or(MeshSkip::LayoutUnknown)?;
            let p = At {
                body: pbody,
                at: pat,
                block: pb,
            };
            let k = At {
                body: cbody,
                at: cat,
                block: cb,
            };
            let p = array(file, raw, p, "positions", 12, c.vertices)?;
            let k = array(file, raw, k, "corners", 4, c.corners)?;
            let faces = faces_from_offsets(file, raw, body, s, me, c)?;
            (
                positions(file, p, c.vertices, 12, 0),
                indices(file, k, c.corners, 4, 0, c.vertices)?,
                faces,
            )
        }
        Layout::Structs => {
            let st = |name: &str| sd.struct_index(name).ok_or(MeshSkip::LayoutUnknown);
            let (mv, mp, ml) = (st("MVert")?, st("MPoly")?, st("MLoop")?);
            let co = sd
                .field(mv, "co")
                .filter(|f| sd.type_name(f.type_index) == "float" && f.array_len == 3)
                .ok_or(MeshSkip::LayoutUnknown)?;
            let ls =
                scalar_field(file, mp, &["loopstart"], "int").ok_or(MeshSkip::LayoutUnknown)?;
            let lt = scalar_field(file, mp, &["totloop"], "int").ok_or(MeshSkip::LayoutUnknown)?;
            let lv = scalar_field(file, ml, &["v"], "int").ok_or(MeshSkip::LayoutUnknown)?;
            let size = |i: u32| sd.struct_def(i).map_or(0, |d| d.size as usize);
            let at = |name: &str| {
                pointer_field(file, s, &[name])
                    .map(|f| At {
                        body,
                        at: f.offset as usize,
                        block: me,
                    })
                    .ok_or(MeshSkip::LayoutUnknown)
            };
            let v = array(file, raw, at("mvert")?, "positions", size(mv), c.vertices)?;
            let p = array(file, raw, at("mpoly")?, "polygons", size(mp), c.faces)?;
            let l = array(file, raw, at("mloop")?, "corners", size(ml), c.corners)?;
            let e = file.endian();
            let mut faces = Vec::with_capacity(c.faces as usize);
            let mut total: u64 = 0;
            for i in 0..c.faces as usize {
                let base = i * size(mp);
                let start = e
                    .i32(p, base + ls.offset as usize)
                    .ok_or(MeshSkip::Offsets)?;
                let n = e
                    .i32(p, base + lt.offset as usize)
                    .ok_or(MeshSkip::Offsets)?;
                if n < 3 {
                    return Err(MeshSkip::FaceTooSmall);
                }
                let (Ok(start), Ok(n)) = (u32::try_from(start), u32::try_from(n)) else {
                    return Err(MeshSkip::Offsets);
                };
                if u64::from(start) + u64::from(n) > u64::from(c.corners) {
                    return Err(MeshSkip::Offsets);
                }
                total += u64::from(n);
                faces.push([start, n]);
            }
            // every corner in exactly one polygon's count: the triangles are corners - 2 * faces
            if total != u64::from(c.corners) {
                return Err(MeshSkip::Offsets);
            }
            (
                positions(file, v, c.vertices, size(mv), co.offset as usize),
                indices(file, l, c.corners, size(ml), lv.offset as usize, c.vertices)?,
                faces,
            )
        }
        Layout::Store => {
            let st = embedded(file, s, &["attribute_storage"], "AttributeStorage")
                .ok_or(MeshSkip::LayoutUnknown)?;
            let (pat, pb, psz) =
                stored(file, raw, body, me, st, "position").ok_or(MeshSkip::LayoutUnknown)?;
            let (cat, cb, csz) =
                stored(file, raw, body, me, st, ".corner_vert").ok_or(MeshSkip::LayoutUnknown)?;
            if psz != i64::from(c.vertices) && !mutant::active("CountsTrusted") {
                return Err(MeshSkip::ArrayLength("positions"));
            }
            if csz != i64::from(c.corners) && !mutant::active("CountsTrusted") {
                return Err(MeshSkip::ArrayLength("corners"));
            }
            let pbody = block_body(file, raw, pb).ok_or(MeshSkip::LayoutUnknown)?;
            let cbody = block_body(file, raw, cb).ok_or(MeshSkip::LayoutUnknown)?;
            let p = At {
                body: pbody,
                at: pat,
                block: pb,
            };
            let k = At {
                body: cbody,
                at: cat,
                block: cb,
            };
            let p = array(file, raw, p, "positions", 12, c.vertices)?;
            let k = array(file, raw, k, "corners", 4, c.corners)?;
            let faces = faces_from_offsets(file, raw, body, s, me, c)?;
            (
                positions(file, p, c.vertices, 12, 0),
                indices(file, k, c.corners, 4, 0, c.vertices)?,
                faces,
            )
        }
    };
    if !positions.iter().all(|p| p.iter().all(|x| x.is_finite())) {
        return Err(MeshSkip::PositionNotFinite);
    }
    Ok(Geometry {
        layout,
        positions,
        corners,
        faces,
    })
}

/// The used vertices of `positions`, in file order, and `triangles` renumbered to them.
pub fn compact(positions: &[[f32; 3]], triangles: Vec<[u32; 3]>) -> SceneMesh {
    if mutant::active("LooseVertexEmitted") {
        return SceneMesh {
            positions: positions.to_vec(),
            triangles,
        };
    }
    let mut map = vec![u32::MAX; positions.len()];
    for t in &triangles {
        for &i in t {
            map[i as usize] = 0;
        }
    }
    let mut kept = Vec::new();
    for (i, m) in map.iter_mut().enumerate() {
        if *m == 0 {
            *m = kept.len() as u32;
            kept.push(positions[i]);
        }
    }
    let triangles = triangles
        .into_iter()
        .map(|t| [map[t[0] as usize], map[t[1] as usize], map[t[2] as usize]])
        .collect();
    SceneMesh {
        positions: kept,
        triangles,
    }
}

/// Mesh block `me` as a triangle mesh: its first whole layout read, its polygons triangulated
/// (`max_corners`: [`super::MeshReadLimits::ear_clip_max_corners`]), its loose vertices
/// dropped. The layout it was read from, and the mesh.
pub fn read_mesh(
    file: &BlendFile,
    raw: &[u8],
    me: u32,
    max_corners: u32,
    counts_out: &mut TriCounts,
) -> Result<(Layout, SceneMesh), MeshSkip> {
    let c = counts(file, raw, me)?;
    if c.faces == 0 {
        return Err(MeshSkip::NoTriangles);
    }
    let layout = *layouts(file, raw, me)
        .first()
        .ok_or(MeshSkip::LayoutUnknown)?;
    let g = read_geometry(file, raw, me, layout)?;
    let mut tris = Vec::with_capacity(c.triangles() as usize);
    for &[start, n] in &g.faces {
        let face = &g.corners[start as usize..(start + n) as usize];
        triangulate_face(&g.positions, face, max_corners, &mut tris, counts_out);
    }
    let mesh = compact(&g.positions, tris);
    if mesh.triangles.is_empty() {
        return Err(MeshSkip::NoTriangles);
    }
    Ok((layout, mesh))
}
