//! The K2SCENE1 writer: a scene's triangle meshes, objects and cameras in the layout a host's
//! renderer reads.
//!
//! ```text
//! magic    8 bytes  "K2SCENE1"
//! hlen     u32 LE   the header's length
//! header   hlen     UTF-8 JSON: version, up axis, meshes (counts, content hashes), objects,
//!                   cameras
//! pad      0-3      zero bytes up to a 4-byte boundary
//! data     per mesh, in header order: positions f32 LE x 3 x vertices,
//!          indices u32 LE x 3 x triangles
//! ```
//!
//! **Bounds first.** Every count is checked against [`MeshReadLimits`] before anything
//! proportional to it is allocated: meshes, objects and cameras; vertices and triangles stored
//! and drawn (a mesh counted once per object that uses it); then the header, built into a buffer
//! that may not grow past its cap; then the output's exact length, computed from the counts.
//! Only then is one buffer of exactly that length filled.
//!
//! **What a renderer refuses is never written:** a value that isn't finite, an index past its
//! mesh's vertices, an object naming no mesh, an id over [`MAX_ID_BYTES`], a key that isn't an
//! address (`@obj:N`, `@cam:N`) or is given twice, more than
//! [`MAX_MODIFIERS`] modifier names or one over [`MAX_MODIFIER_BYTES`], a camera without a
//! direction, with a vertical field of view outside (0, pi), a non-positive aspect, lens or
//! sensor, or a second active camera. Each is a typed refusal, [`MeshReadError`].
//!
//! **Deterministic.** The header is written by hand in a fixed field order, never from a map;
//! each float as the shortest text that reads back to the same `f32`; positions and indices as
//! their little-endian bytes. Normals are not written (`"normals": false`): the renderer shades
//! from the triangles.
//!
//! **Each mesh's content hash** (`hash`, [`HASH_HEX`] lowercase hex digits): SHA-256 of its vertex
//! count and its triangle count, each a `u32` little-endian, then its data exactly as written
//! (positions, then indices). Its text has a fixed length, so the header is laid out first with
//! the hashes as zeros, the data written, and each hash then worked out from the output's own
//! bytes and put in its place.

use std::fmt::Write as _;

use k2_mpc_base::index::Digest;

use super::{at_most, MeshReadError, MeshReadLimits};
use crate::mutant;

/// The layout's magic.
pub const MAGIC: &[u8; 8] = b"K2SCENE1";
/// The header's `version`.
pub const VERSION: u32 = 1;
/// The magic and the header's length.
pub const PREFIX: u64 = 12;
/// The longest id (an object's or a camera's), in bytes.
pub const MAX_ID_BYTES: usize = 1024;
/// The most modifier names one object carries.
pub const MAX_MODIFIERS: usize = 256;
/// The longest modifier name, in bytes.
pub const MAX_MODIFIER_BYTES: usize = 128;
/// The length of a mesh's content hash, in hex digits.
pub const HASH_HEX: usize = 64;

/// One triangle mesh, in its own coordinates.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SceneMesh {
    /// Each vertex's position.
    pub positions: Vec<[f32; 3]>,
    /// Each triangle's three vertex indices, counter-clockwise seen from its front.
    pub triangles: Vec<[u32; 3]>,
}

/// One drawn object: a mesh placed in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneObject {
    /// Its name.
    pub id: String,
    /// Its address, `@obj:N`.
    pub key: String,
    /// The index of its mesh in [`Scene::meshes`].
    pub mesh: u32,
    /// Linear RGBA.
    pub base_color: [f32; 4],
    /// `Some(false)`: back faces may be culled; `Some(true)` or `None` (unknown): both sides.
    pub double_sided: Option<bool>,
    /// Object to world, column-major 4x4.
    pub transform: [f32; 16],
    /// Its modifiers' names, in order.
    pub modifiers: Vec<String>,
    /// The mesh is drawn without its modifiers.
    pub modifiers_not_applied: bool,
    /// The transform is the stored one, with animation, drivers and constraints not evaluated.
    pub transform_not_evaluated: bool,
    /// Hidden in the viewport, and why; `None`: shown.
    pub hidden: Option<Hidden>,
}

/// Why an object is hidden in the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hidden {
    /// Its own flag.
    Itself,
    /// Every collection holding it in the scene drawn is hidden.
    Collection,
}

impl Hidden {
    /// Its word in the header.
    pub fn word(self) -> &'static str {
        match self {
            Self::Itself => "self",
            Self::Collection => "collection",
        }
    }
}

/// How a camera's sensor fits the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorFit {
    /// The larger side.
    Auto,
    /// The width.
    Horizontal,
    /// The height.
    Vertical,
}

impl SensorFit {
    /// Its word in the header.
    pub fn word(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }
    }
}

/// A perspective camera, in world space.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneCamera {
    /// Its name.
    pub id: String,
    /// Its address, `@cam:N`.
    pub key: String,
    /// Where it is.
    pub position: [f32; 3],
    /// A point it looks at (this or `forward`).
    pub target: Option<[f32; 3]>,
    /// The direction it looks in (this or `target`).
    pub forward: Option<[f32; 3]>,
    /// Its up direction.
    pub up: [f32; 3],
    /// Its vertical field of view, in radians.
    pub yfov: f32,
    /// Width over height.
    pub aspect: Option<f32>,
    /// The near clipping distance.
    pub znear: Option<f32>,
    /// The scene's active camera (at most one is).
    pub active: Option<bool>,
    /// The focal length, in mm.
    pub lens_mm: Option<f32>,
    /// The sensor's width and height, in mm.
    pub sensor_mm: Option<[f32; 2]>,
    /// How the sensor fits the image.
    pub sensor_fit: Option<SensorFit>,
}

/// A camera of the scene drawn that is left out, and why: listed so it isn't silently missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftOut {
    /// The camera object's name.
    pub name: String,
    /// Its address, `@cam:N`, when it has a live key.
    pub key: Option<String>,
    /// The reason's fixed word (`camera_not_perspective`, `camera_sensor_fit_unknown`...).
    pub reason: String,
}

/// What a K2SCENE1 holds. Its up axis is always +Z.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    /// The meshes, each written once.
    pub meshes: Vec<SceneMesh>,
    /// The objects that draw them.
    pub objects: Vec<SceneObject>,
    /// The cameras.
    pub cameras: Vec<SceneCamera>,
    /// The scene's cameras left out (`cameras_left_out`; not written when there are none).
    pub cameras_left_out: Vec<LeftOut>,
}

/// The data bytes of a mesh of `vertices` and `triangles`, without normals.
pub fn data_len(vertices: u64, triangles: u64) -> u64 {
    vertices * 12 + triangles * 12
}

/// `n` rounded up to a multiple of 4.
pub fn align4(n: u64) -> u64 {
    (n + 3) & !3
}

fn invalid(what: &'static str) -> MeshReadError {
    MeshReadError::Invalid { what }
}

fn finite(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// The header, built under its cap.
struct Header {
    s: String,
    max: u64,
}

impl Header {
    fn capped(&self) -> Result<(), MeshReadError> {
        at_most("header bytes", self.s.len() as u64, self.max).map(|_| ())
    }

    fn raw(&mut self, t: &str) {
        self.s.push_str(t);
    }

    fn string(&mut self, t: &str) {
        self.s.push('"');
        for c in t.chars() {
            match c {
                '"' => self.s.push_str("\\\""),
                '\\' => self.s.push_str("\\\\"),
                c if (c as u32) < 0x20 => {
                    let _ = write!(self.s, "\\u{:04x}", c as u32);
                }
                c => self.s.push(c),
            }
        }
        self.s.push('"');
    }

    fn float(&mut self, x: f32) {
        // finite (checked before): Rust's shortest text that reads back to the same f32
        let _ = write!(self.s, "{x}");
    }

    fn floats(&mut self, v: &[f32]) {
        self.s.push('[');
        for (i, x) in v.iter().enumerate() {
            if i > 0 {
                self.s.push(',');
            }
            self.float(*x);
        }
        self.s.push(']');
    }

    fn key(&mut self, k: &str) {
        self.s.push(',');
        self.string(k);
        self.s.push(':');
    }
}

/// The checks a renderer makes on a camera.
fn check_camera(c: &SceneCamera) -> Result<(), MeshReadError> {
    if c.id.len() > MAX_ID_BYTES {
        return Err(invalid("a camera's id is too long"));
    }
    if !finite(&c.position) || !finite(&c.up) {
        return Err(invalid("a camera's position or up isn't finite"));
    }
    match (&c.target, &c.forward) {
        (None, None) => return Err(invalid("a camera has no target or forward")),
        (Some(t), _) if !finite(t) => return Err(invalid("a camera's target isn't finite")),
        (_, Some(f)) if !finite(f) => return Err(invalid("a camera's forward isn't finite")),
        _ => {}
    }
    if !(c.yfov > 0.0 && c.yfov < std::f32::consts::PI) {
        return Err(invalid("a camera's yfov is outside (0, pi)"));
    }
    let positive = |x: f32| x.is_finite() && x > 0.0;
    if c.aspect.is_some_and(|a| !positive(a)) {
        return Err(invalid("a camera's aspect isn't positive"));
    }
    if c.znear.is_some_and(|z| !z.is_finite()) {
        return Err(invalid("a camera's znear isn't finite"));
    }
    if c.lens_mm.is_some_and(|l| !positive(l)) {
        return Err(invalid("a camera's lens isn't positive"));
    }
    if c.sensor_mm.is_some_and(|s| !s.iter().all(|&x| positive(x))) {
        return Err(invalid("a camera's sensor isn't positive"));
    }
    Ok(())
}

/// The number of an address `<prefix>N` (`N` decimal, no sign, no leading zero), or `None`.
pub fn address_num(address: &str, prefix: &str) -> Option<u64> {
    let rest = address.strip_prefix(prefix)?;
    if rest.is_empty() || (rest.len() > 1 && rest.starts_with('0')) {
        return None;
    }
    if !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

/// Lowercase hex.
fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        let _ = write!(s, "{x:02x}");
    }
    s
}

/// The content hash of a mesh of `vertices` and `triangles` whose data, as written, is `data`
/// (the module's rule).
pub fn mesh_hash(vertices: u32, triangles: u32, data: &[u8]) -> String {
    let mut d = Digest::stream();
    if !mutant::active("MeshHashOmitsCounts") {
        d.update(&vertices.to_le_bytes());
        d.update(&triangles.to_le_bytes());
    }
    d.update(data);
    hex(d.finish().as_bytes())
}

/// One mesh as the header lists it: its counts and its content hash ([`mesh_hash`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshHead {
    /// Its vertices.
    pub vertices: u64,
    /// Its triangles.
    pub triangles: u64,
    /// Its content hash, [`HASH_HEX`] lowercase hex digits.
    pub hash: String,
}

/// `scene` as K2SCENE1, under `limits`; or a typed refusal, with nothing written.
pub fn write(scene: &Scene, limits: &MeshReadLimits) -> Result<Vec<u8>, MeshReadError> {
    write_hashed(scene, limits).map(|(out, _)| out)
}

/// The checks a K2SCENE1 passes before its header is laid out: the counts of meshes
/// (`counts`, each mesh's vertices and triangles), objects and cameras; the vertices and
/// triangles stored and drawn; each object's and camera's own rules.
fn check_parts(
    counts: &[(u64, u64)],
    objects: &[SceneObject],
    cameras: &[SceneCamera],
    limits: &MeshReadLimits,
) -> Result<(), MeshReadError> {
    let n = |v: usize| v as u64;
    at_most("meshes", n(counts.len()), limits.max_meshes)?;
    at_most("objects", n(objects.len()), limits.max_objects)?;
    at_most("cameras", n(cameras.len()), limits.max_cameras)?;
    let (mut verts, mut tris) = (0u64, 0u64);
    for &(v, t) in counts {
        let u = u64::from(u32::MAX);
        verts += at_most("vertices of a mesh", v, u)?;
        tris += at_most("triangles of a mesh", t, u)?;
    }
    at_most("vertices", verts, limits.max_vertices)?;
    at_most("triangles", tris, limits.max_triangles)?;
    let (mut drawn_v, mut drawn_t) = (0u64, 0u64);
    for o in objects {
        let &(v, t) = counts
            .get(o.mesh as usize)
            .ok_or(invalid("an object names no mesh"))?;
        drawn_v += v;
        drawn_t += t;
    }
    at_most("drawn vertices", drawn_v, limits.max_drawn_vertices)?;
    at_most("drawn triangles", drawn_t, limits.max_drawn_triangles)?;
    let mut keys = std::collections::HashSet::new();
    for o in objects {
        if o.id.len() > MAX_ID_BYTES {
            return Err(invalid("an object's id is too long"));
        }
        if address_num(&o.key, "@obj:").is_none() {
            return Err(invalid("an object's key isn't @obj:N"));
        }
        if !keys.insert(o.key.as_str()) {
            return Err(invalid("an object's key is given twice"));
        }
        at_most(
            "modifiers of an object",
            n(o.modifiers.len()),
            MAX_MODIFIERS as u64,
        )?;
        if o.modifiers.iter().any(|m| m.len() > MAX_MODIFIER_BYTES) {
            return Err(invalid("a modifier name is too long"));
        }
        if !finite(&o.transform) || !finite(&o.base_color) {
            return Err(invalid("an object's transform or colour isn't finite"));
        }
    }
    let mut active = false;
    let mut keys = std::collections::HashSet::new();
    for c in cameras {
        check_camera(c)?;
        if address_num(&c.key, "@cam:").is_none() {
            return Err(invalid("a camera's key isn't @cam:N"));
        }
        if !keys.insert(c.key.as_str()) {
            return Err(invalid("a camera's key is given twice"));
        }
        if c.active == Some(true) {
            if active {
                return Err(invalid("a second active camera"));
            }
            active = true;
        }
    }
    Ok(())
}

/// The header's JSON under its cap, and where each mesh's hash starts in it: each mesh of
/// `meshes` (its vertices, its triangles, its hash) with its hash as given, or zeros in its
/// place when it has none yet (filled once the data is written).
fn header_json(
    meshes: &[(u64, u64, Option<&str>)],
    objects: &[SceneObject],
    cameras: &[SceneCamera],
    left: &[LeftOut],
    limits: &MeshReadLimits,
) -> Result<(String, Vec<usize>), MeshReadError> {
    let mut h = Header {
        s: String::new(),
        max: limits.max_header_bytes,
    };
    let mut hash_at: Vec<usize> = Vec::with_capacity(meshes.len());
    let _ = write!(h.s, "{{\"version\":{VERSION},\"up\":\"z\",\"meshes\":[");
    for (i, &(v, t, hash)) in meshes.iter().enumerate() {
        let mut t = t;
        if i == 0 && mutant::active("HeaderCountOff") {
            t += 1;
        }
        let sep = if i > 0 { "," } else { "" };
        let _ = write!(
            h.s,
            "{sep}{{\"vertices\":{v},\"triangles\":{t},\"normals\":false,\"hash\":\""
        );
        hash_at.push(h.s.len());
        match hash {
            Some(x) => h.s.push_str(x),
            // a place for the hash, filled once the data is written
            None => h.s.push_str(&"0".repeat(HASH_HEX)),
        }
        h.raw("\"}");
        h.capped()?;
    }
    h.raw("],\"objects\":[");
    for (i, o) in objects.iter().enumerate() {
        if i > 0 {
            h.raw(",");
        }
        h.raw("{\"id\":");
        h.string(&o.id);
        h.key("key");
        h.string(&o.key);
        let _ = write!(h.s, ",\"mesh\":{}", o.mesh);
        h.key("base_color");
        h.floats(&o.base_color);
        if let Some(d) = o.double_sided {
            h.key("double_sided");
            h.raw(if d { "true" } else { "false" });
        }
        h.key("transform");
        h.floats(&o.transform);
        if !o.modifiers.is_empty() {
            h.key("modifiers");
            h.raw("[");
            for (k, m) in o.modifiers.iter().enumerate() {
                if k > 0 {
                    h.raw(",");
                }
                h.string(m);
            }
            h.raw("]");
        }
        if o.modifiers_not_applied {
            h.key("modifiers_not_applied");
            h.raw("true");
        }
        if o.transform_not_evaluated {
            h.key("transform_not_evaluated");
            h.raw("true");
        }
        if let Some(why) = o.hidden {
            h.key("hidden");
            h.string(why.word());
        }
        h.raw("}");
        h.capped()?;
    }
    h.raw("],\"cameras\":[");
    for (i, c) in cameras.iter().enumerate() {
        if i > 0 {
            h.raw(",");
        }
        h.raw("{\"id\":");
        h.string(&c.id);
        h.key("key");
        h.string(&c.key);
        h.key("position");
        h.floats(&c.position);
        if let Some(t) = c.target {
            h.key("target");
            h.floats(&t);
        }
        if let Some(f) = c.forward {
            h.key("forward");
            h.floats(&f);
        }
        h.key("up");
        h.floats(&c.up);
        h.key("yfov");
        h.float(c.yfov);
        if let Some(a) = c.aspect {
            h.key("aspect");
            h.float(a);
        }
        if let Some(z) = c.znear {
            h.key("znear");
            h.float(z);
        }
        if let Some(a) = c.active {
            h.key("active");
            h.raw(if a { "true" } else { "false" });
        }
        if let Some(l) = c.lens_mm {
            h.key("lens_mm");
            h.float(l);
        }
        if let Some(s) = c.sensor_mm {
            h.key("sensor_mm");
            h.floats(&s);
        }
        if let Some(f) = c.sensor_fit {
            h.key("sensor_fit");
            h.string(f.word());
        }
        h.raw("}");
        h.capped()?;
    }
    h.raw("]");
    if !left.is_empty() {
        h.raw(",\"cameras_left_out\":[");
        for (i, l) in left.iter().enumerate() {
            if l.name.len() > MAX_ID_BYTES || l.reason.len() > MAX_ID_BYTES {
                return Err(invalid("a left-out camera's name or reason is too long"));
            }
            if i > 0 {
                h.raw(",");
            }
            h.raw("{\"name\":");
            h.string(&l.name);
            if let Some(k) = &l.key {
                h.key("key");
                h.string(k);
            }
            h.key("reason");
            h.string(&l.reason);
            h.raw("}");
            h.capped()?;
        }
        h.raw("]");
    }
    h.raw("}");
    h.capped()?;
    Ok((h.s, hash_at))
}

/// The header's length as written, and where the data section starts: the magic, the length
/// and the header, padded with zeros to a 4-byte boundary; the whole output checked against
/// [`MeshReadLimits::max_output_bytes`] with `data` bytes of data after it.
fn layout(header: &str, data: u64, limits: &MeshReadLimits) -> Result<(u32, u64), MeshReadError> {
    let hlen = at_most("header bytes", header.len() as u64, u64::from(u32::MAX))?;
    let off = align4(PREFIX + hlen);
    at_most("output bytes", off + data, limits.max_output_bytes)?;
    Ok((hlen as u32, off))
}

/// [`write`](fn@write), and each mesh's content hash, in the meshes' order.
pub fn write_hashed(
    scene: &Scene,
    limits: &MeshReadLimits,
) -> Result<(Vec<u8>, Vec<String>), MeshReadError> {
    let n = |v: usize| v as u64;
    let counts: Vec<(u64, u64)> = scene
        .meshes
        .iter()
        .map(|m| (n(m.positions.len()), n(m.triangles.len())))
        .collect();
    check_parts(&counts, &scene.objects, &scene.cameras, limits)?;
    for m in &scene.meshes {
        if !m.positions.iter().all(|p| finite(p)) {
            return Err(invalid("a position isn't finite"));
        }
        let nv = n(m.positions.len());
        if m.triangles.iter().flatten().any(|&i| u64::from(i) >= nv) {
            return Err(invalid("an index is past its mesh's vertices"));
        }
    }
    let heads: Vec<(u64, u64, Option<&str>)> = counts.iter().map(|&(v, t)| (v, t, None)).collect();
    let (h, hash_at) = header_json(
        &heads,
        &scene.objects,
        &scene.cameras,
        &scene.cameras_left_out,
        limits,
    )?;
    let data: u64 = counts.iter().map(|&(v, t)| data_len(v, t)).sum();
    let (hlen, off) = layout(&h, data, limits)?;
    let total = off + data;
    let total_usize = usize::try_from(total).map_err(|_| MeshReadError::TooLarge {
        what: "output bytes",
        got: total,
        max: usize::MAX as u64,
    })?;
    let mut out = Vec::with_capacity(total_usize);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&hlen.to_le_bytes());
    out.extend_from_slice(h.as_bytes());
    out.resize(off as usize, 0);
    let mut spans = Vec::with_capacity(scene.meshes.len());
    for m in &scene.meshes {
        let from = out.len();
        for p in &m.positions {
            for x in p {
                out.extend_from_slice(&x.to_le_bytes());
            }
        }
        for t in &m.triangles {
            for i in t {
                out.extend_from_slice(&i.to_le_bytes());
            }
        }
        spans.push(from..out.len());
    }
    debug_assert_eq!(out.len(), total_usize);
    if out.len() != total_usize {
        return Err(invalid("the output's length isn't the one its counts give"));
    }
    // each mesh's hash, from its counts and its data as written, put in its place
    let mut hashes = Vec::with_capacity(scene.meshes.len());
    for ((m, span), at) in scene.meshes.iter().zip(spans).zip(hash_at) {
        let hash = mesh_hash(
            m.positions.len() as u32,
            m.triangles.len() as u32,
            &out[span],
        );
        let at = PREFIX as usize + at;
        out[at..at + HASH_HEX].copy_from_slice(hash.as_bytes());
        hashes.push(hash);
    }
    Ok((out, hashes))
}

/// The first part of a K2SCENE1 whose data section (`data` bytes) is already written: the
/// magic, the header's length, the header and its zero pad, for `meshes` (counts and hashes,
/// in the data's order), `objects`, `cameras` and `left`. The same checks and the same header
/// text as [`write_hashed`], so this part followed by that data section is the K2SCENE1
/// [`write_hashed`] gives for the same scene; or a typed refusal, with nothing written.
pub fn write_header(
    meshes: &[MeshHead],
    objects: &[SceneObject],
    cameras: &[SceneCamera],
    left: &[LeftOut],
    data: u64,
    limits: &MeshReadLimits,
) -> Result<Vec<u8>, MeshReadError> {
    let counts: Vec<(u64, u64)> = meshes.iter().map(|m| (m.vertices, m.triangles)).collect();
    check_parts(&counts, objects, cameras, limits)?;
    if meshes.iter().any(|m| {
        m.hash.len() != HASH_HEX
            || !m
                .hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) {
        return Err(invalid("a mesh's hash isn't 64 lowercase hex digits"));
    }
    let given: u64 = counts.iter().map(|&(v, t)| data_len(v, t)).sum();
    if given != data {
        return Err(invalid(
            "the data section's length isn't the one its meshes' counts give",
        ));
    }
    let heads: Vec<(u64, u64, Option<&str>)> = meshes
        .iter()
        .map(|m| (m.vertices, m.triangles, Some(m.hash.as_str())))
        .collect();
    let (h, _) = header_json(&heads, objects, cameras, left, limits)?;
    let (hlen, off) = layout(&h, data, limits)?;
    let mut out = Vec::with_capacity(off as usize);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&hlen.to_le_bytes());
    out.extend_from_slice(h.as_bytes());
    out.resize(off as usize, 0);
    Ok(out)
}
