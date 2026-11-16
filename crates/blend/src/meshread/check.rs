//! Test-only (features `testgen`, `fuzzing`): a strict K2SCENE1 reader, written from the
//! layout's description ([`super::k2scene`]) and a renderer's input rules, sharing no code with
//! the writer. [`check`] returns the scene a K2SCENE1 holds, or why a renderer would refuse it.
//!
//! It refuses what a renderer refuses (a count over its limit, an exact length that isn't the
//! one the counts give, non-zero padding, a value that isn't finite, an index past its mesh, an
//! object naming no mesh, an id or a modifier name too long, a camera a renderer can't use, a
//! second active camera) and, stricter than a renderer: JSON that isn't strict RFC 8259 JSON,
//! a duplicate or unknown key, a float whose text reads to other bits through `f64` than as
//! an `f32` directly (a renderer may read it either way), an object or camera without its key
//! (`@obj:N`, `@cam:N`: `N` with no sign or leading zero), a key given twice or a number shared
//! by an object and a camera, and a mesh without its content hash (64 lowercase hex digits) or
//! whose hash isn't SHA-256 of its two counts (`u32` little-endian) and its data as written.
//!
//! [`parse_json`] is its strict JSON reader, for the tests' own JSON files too.

use std::collections::HashSet;

use k2_mpc_base::index::Digest;

use super::k2scene::{
    align4, data_len, Hidden, LeftOut, Scene, SceneCamera, SceneMesh, SceneObject, SensorFit,
    MAGIC, MAX_ID_BYTES, MAX_MODIFIERS, MAX_MODIFIER_BYTES, PREFIX, VERSION,
};
use super::MeshReadLimits;

/// What [`check`] read.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    /// The scene.
    pub scene: Scene,
    /// The up axis's word (`"y"` when the header has none).
    pub up: String,
    /// The header's JSON.
    pub header: String,
    /// Where the data starts.
    pub data_offset: u64,
    /// Meshes that carried normals (read, checked and dropped).
    pub meshes_with_normals: usize,
    /// Each mesh's content hash, as the header gives it (checked against its data).
    pub hashes: Vec<String>,
}

/// A JSON value, its numbers kept as written ([`parse_json`]).
#[derive(Debug, Clone, PartialEq)]
pub enum J {
    /// `null`.
    Null,
    /// `true`, `false`.
    Bool(bool),
    /// A number, its text as written.
    Num(String),
    /// A string.
    Str(String),
    /// A list.
    Arr(Vec<J>),
    /// An object, its keys in order (each once).
    Obj(Vec<(String, J)>),
}

impl J {
    /// The value at key `k` of an object.
    pub fn get(&self, k: &str) -> Option<&J> {
        match self {
            J::Obj(v) => v.iter().find(|(e, _)| e == k).map(|(_, x)| x),
            _ => None,
        }
    }
}

const DEPTH_MAX: u32 = 64;

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while let Some(&c) = self.b.get(self.i) {
            if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn lit(&mut self, t: &str) -> Result<(), String> {
        if self.b[self.i..].starts_with(t.as_bytes()) {
            self.i += t.len();
            Ok(())
        } else {
            Err(format!("JSON: bad literal at {}", self.i))
        }
    }

    fn value(&mut self, depth: u32) -> Result<J, String> {
        if depth > DEPTH_MAX {
            return Err("JSON: nested too deep".into());
        }
        self.ws();
        match self.b.get(self.i) {
            None => Err("JSON: ends early".into()),
            Some(b'n') => self.lit("null").map(|_| J::Null),
            Some(b't') => self.lit("true").map(|_| J::Bool(true)),
            Some(b'f') => self.lit("false").map(|_| J::Bool(false)),
            Some(b'"') => self.string().map(J::Str),
            Some(b'[') => {
                self.i += 1;
                let mut v = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(J::Arr(v));
                }
                loop {
                    v.push(self.value(depth + 1)?);
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(J::Arr(v));
                        }
                        _ => return Err(format!("JSON: bad array at {}", self.i)),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut v: Vec<(String, J)> = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(J::Obj(v));
                }
                loop {
                    self.ws();
                    if self.b.get(self.i) != Some(&b'"') {
                        return Err(format!("JSON: a key expected at {}", self.i));
                    }
                    let k = self.string()?;
                    if v.iter().any(|(e, _)| *e == k) {
                        return Err(format!("JSON: duplicate key {k:?}"));
                    }
                    self.ws();
                    if self.b.get(self.i) != Some(&b':') {
                        return Err(format!("JSON: ':' expected at {}", self.i));
                    }
                    self.i += 1;
                    let x = self.value(depth + 1)?;
                    v.push((k, x));
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(J::Obj(v));
                        }
                        _ => return Err(format!("JSON: bad object at {}", self.i)),
                    }
                }
            }
            Some(c) if *c == b'-' || c.is_ascii_digit() => self.number().map(J::Num),
            Some(_) => Err(format!("JSON: unexpected byte at {}", self.i)),
        }
    }

    fn number(&mut self) -> Result<String, String> {
        let start = self.i;
        let bad = |i: usize| format!("JSON: bad number at {i}");
        let digits = |p: &mut Self| {
            let s = p.i;
            while p.b.get(p.i).is_some_and(u8::is_ascii_digit) {
                p.i += 1;
            }
            p.i - s
        };
        if self.b.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        match self.b.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(c) if c.is_ascii_digit() => {
                digits(self);
            }
            _ => return Err(bad(start)),
        }
        if self.b.get(self.i) == Some(&b'.') {
            self.i += 1;
            if digits(self) == 0 {
                return Err(bad(start));
            }
        }
        if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.b.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if digits(self) == 0 {
                return Err(bad(start));
            }
        }
        Ok(String::from_utf8_lossy(&self.b[start..self.i]).into_owned())
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let s = self
            .b
            .get(self.i..self.i + 4)
            .ok_or("JSON: a \\u escape ends early")?;
        let t = std::str::from_utf8(s).map_err(|_| "JSON: a bad \\u escape")?;
        if !t.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("JSON: a bad \\u escape".into());
        }
        self.i += 4;
        u32::from_str_radix(t, 16).map_err(|_| "JSON: a bad \\u escape".into())
    }

    fn string(&mut self) -> Result<String, String> {
        // at the opening quote; the header was checked to be UTF-8 as a whole
        self.i += 1;
        let mut out = String::new();
        loop {
            let start = self.i;
            while let Some(&c) = self.b.get(self.i) {
                if c == b'"' || c == b'\\' || c < 0x20 {
                    break;
                }
                self.i += 1;
            }
            out.push_str(
                std::str::from_utf8(&self.b[start..self.i]).map_err(|_| "JSON: not UTF-8")?,
            );
            match self.b.get(self.i) {
                None => return Err("JSON: a string has no end".into()),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let e = *self.b.get(self.i).ok_or("JSON: an escape ends early")?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xD800..0xDC00).contains(&hi) {
                                if !self.b[self.i..].starts_with(b"\\u") {
                                    return Err("JSON: a lone surrogate".into());
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err("JSON: a lone surrogate".into());
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else if (0xDC00..0xE000).contains(&hi) {
                                return Err("JSON: a lone surrogate".into());
                            } else {
                                hi
                            };
                            out.push(char::from_u32(cp).ok_or("JSON: a bad code point")?);
                        }
                        _ => return Err("JSON: a bad escape".into()),
                    }
                }
                Some(_) => return Err("JSON: a control character in a string".into()),
            }
        }
    }
}

/// `text` as one strict JSON value (RFC 8259; a key twice in an object is refused).
pub fn parse_json(text: &str) -> Result<J, String> {
    let mut p = Parser {
        b: text.as_bytes(),
        i: 0,
    };
    let v = p.value(0)?;
    p.ws();
    if p.i != p.b.len() {
        return Err("JSON: bytes after the value".into());
    }
    Ok(v)
}

/// An object's fields, refusing a key outside `known`.
fn fields<'a>(j: &'a J, what: &str, known: &[&str]) -> Result<&'a [(String, J)], String> {
    let J::Obj(v) = j else {
        return Err(format!("{what}: not an object"));
    };
    if let Some((k, _)) = v.iter().find(|(k, _)| !known.contains(&k.as_str())) {
        return Err(format!("{what}: unknown key {k:?}"));
    }
    Ok(v)
}

fn get<'a>(f: &'a [(String, J)], k: &str) -> Option<&'a J> {
    f.iter().find(|(e, _)| e == k).map(|(_, v)| v)
}

fn need<'a>(f: &'a [(String, J)], k: &str, what: &str) -> Result<&'a J, String> {
    get(f, k).ok_or_else(|| format!("{what}: no {k}"))
}

fn as_u32(j: &J, what: &str) -> Result<u32, String> {
    match j {
        J::Num(t) if t.bytes().all(|c| c.is_ascii_digit()) => t
            .parse::<u32>()
            .map_err(|_| format!("{what}: not a u32: {t}")),
        _ => Err(format!("{what}: not a whole number")),
    }
}

fn as_f32(j: &J, what: &str) -> Result<f32, String> {
    let J::Num(t) = j else {
        return Err(format!("{what}: not a number"));
    };
    let wide = t
        .parse::<f64>()
        .map_err(|_| format!("{what}: not a number"))? as f32;
    let narrow = t
        .parse::<f32>()
        .map_err(|_| format!("{what}: not a number"))?;
    if wide.to_bits() != narrow.to_bits() {
        return Err(format!("{what}: {t} reads differently through f64"));
    }
    Ok(narrow)
}

fn as_f32s<const N: usize>(j: &J, what: &str) -> Result<[f32; N], String> {
    let J::Arr(v) = j else {
        return Err(format!("{what}: not a list"));
    };
    if v.len() != N {
        return Err(format!("{what}: {} numbers, not {N}", v.len()));
    }
    let mut out = [0f32; N];
    for (o, x) in out.iter_mut().zip(v) {
        *o = as_f32(x, what)?;
    }
    Ok(out)
}

fn as_bool(j: &J, what: &str) -> Result<bool, String> {
    match j {
        J::Bool(b) => Ok(*b),
        _ => Err(format!("{what}: not a bool")),
    }
}

fn as_str<'a>(j: &'a J, what: &str) -> Result<&'a str, String> {
    match j {
        J::Str(s) => Ok(s),
        _ => Err(format!("{what}: not a string")),
    }
}

/// The list at `k`: empty when absent and not `required`.
fn list<'a>(f: &'a [(String, J)], k: &str, required: bool) -> Result<&'a [J], String> {
    match get(f, k) {
        None if !required => Ok(&[]),
        None => Err(format!("header: no {k}")),
        Some(J::Arr(v)) => Ok(v.as_slice()),
        Some(_) => Err(format!("{k}: not a list")),
    }
}

fn over(what: &str, got: u64, max: u64) -> Result<(), String> {
    if got > max {
        Err(format!("too_large: {what} {got}, at most {max}"))
    } else {
        Ok(())
    }
}

fn finite(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// The mesh header: (vertices, triangles, normals, hash).
fn mesh_info(j: &J) -> Result<(u32, u32, bool, String), String> {
    let f = fields(j, "mesh", &["vertices", "triangles", "normals", "hash"])?;
    let v = as_u32(need(f, "vertices", "mesh")?, "mesh vertices")?;
    let t = as_u32(need(f, "triangles", "mesh")?, "mesh triangles")?;
    let n = as_bool(need(f, "normals", "mesh")?, "mesh normals")?;
    let h = as_str(need(f, "hash", "mesh")?, "mesh hash")?;
    if h.len() != 64
        || !h
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(format!(
            "mesh: its hash {h:?} isn't 64 lowercase hex digits"
        ));
    }
    Ok((v, t, n, h.to_owned()))
}

/// The number of an address `<prefix>N`, `N` with no sign and no leading zero.
fn key_number(k: &str, prefix: &str, what: &str) -> Result<u64, String> {
    let n = k
        .strip_prefix(prefix)
        .filter(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
        .filter(|n| n.len() == 1 || !n.starts_with('0'))
        .ok_or_else(|| format!("{what}: its key {k:?} isn't {prefix}N"))?;
    n.parse::<u64>()
        .map_err(|_| format!("{what}: its key {k:?} isn't {prefix}N"))
}

fn object(j: &J) -> Result<SceneObject, String> {
    let w = "object";
    let f = fields(
        j,
        w,
        &[
            "id",
            "key",
            "mesh",
            "base_color",
            "double_sided",
            "transform",
            "hash",
            "modifiers",
            "modifiers_not_applied",
            "transform_not_evaluated",
            "hidden",
        ],
    )?;
    let id = as_str(need(f, "id", w)?, "object id")?.to_owned();
    if id.len() > MAX_ID_BYTES {
        return Err("object: its id is too long".into());
    }
    let key = as_str(need(f, "key", w)?, "object key")?.to_owned();
    key_number(&key, "@obj:", w)?;
    if let Some(h) = get(f, "hash") {
        if as_str(h, "object hash")?.len() > MAX_ID_BYTES {
            return Err("object: its hash is too long".into());
        }
    }
    let double_sided = match get(f, "double_sided") {
        None | Some(J::Null) => None,
        Some(x) => Some(as_bool(x, "double_sided")?),
    };
    let modifiers = match get(f, "modifiers") {
        None => Vec::new(),
        Some(J::Arr(v)) => {
            over(
                "modifiers of an object",
                v.len() as u64,
                MAX_MODIFIERS as u64,
            )?;
            let mut out = Vec::with_capacity(v.len());
            for m in v {
                let m = as_str(m, "modifier")?;
                if m.len() > MAX_MODIFIER_BYTES {
                    return Err("object: a modifier name is too long".into());
                }
                out.push(m.to_owned());
            }
            out
        }
        Some(_) => return Err("modifiers: not a list".into()),
    };
    let hidden = match get(f, "hidden") {
        None => None,
        Some(x) => Some(match as_str(x, "hidden")? {
            "self" => Hidden::Itself,
            "collection" => Hidden::Collection,
            other => return Err(format!("hidden: unknown word {other:?}")),
        }),
    };
    let flag = |k: &str| get(f, k).map_or(Ok(false), |x| as_bool(x, k));
    Ok(SceneObject {
        id,
        key,
        mesh: as_u32(need(f, "mesh", w)?, "object mesh")?,
        base_color: as_f32s(need(f, "base_color", w)?, "base_color")?,
        double_sided,
        transform: as_f32s(need(f, "transform", w)?, "transform")?,
        modifiers,
        modifiers_not_applied: flag("modifiers_not_applied")?,
        transform_not_evaluated: flag("transform_not_evaluated")?,
        hidden,
    })
}

fn left_out(j: &J) -> Result<LeftOut, String> {
    let w = "a camera left out";
    let f = fields(j, w, &["name", "key", "reason"])?;
    let name = as_str(need(f, "name", w)?, "left-out name")?.to_owned();
    let reason = as_str(need(f, "reason", w)?, "left-out reason")?.to_owned();
    if name.len() > MAX_ID_BYTES || reason.is_empty() || reason.len() > MAX_ID_BYTES {
        return Err(format!("{w}: its name or reason"));
    }
    let key = match get(f, "key") {
        None => None,
        Some(k) => {
            let k = as_str(k, "left-out key")?.to_owned();
            key_number(&k, "@cam:", w)?;
            Some(k)
        }
    };
    Ok(LeftOut { name, key, reason })
}

fn camera(j: &J) -> Result<SceneCamera, String> {
    let w = "camera";
    let f = fields(
        j,
        w,
        &[
            "id",
            "key",
            "position",
            "target",
            "forward",
            "up",
            "yfov",
            "aspect",
            "znear",
            "active",
            "lens_mm",
            "sensor_mm",
            "sensor_fit",
        ],
    )?;
    let opt_f32 = |k: &str| get(f, k).map(|x| as_f32(x, k)).transpose();
    let id = match get(f, "id") {
        None => String::new(),
        Some(x) => as_str(x, "camera id")?.to_owned(),
    };
    let key = as_str(need(f, "key", w)?, "camera key")?.to_owned();
    key_number(&key, "@cam:", w)?;
    let sensor_fit = match get(f, "sensor_fit") {
        None => None,
        Some(x) => Some(match as_str(x, "sensor_fit")? {
            "auto" => SensorFit::Auto,
            "horizontal" => SensorFit::Horizontal,
            "vertical" => SensorFit::Vertical,
            other => return Err(format!("sensor_fit: unknown word {other:?}")),
        }),
    };
    Ok(SceneCamera {
        id,
        key,
        position: as_f32s(need(f, "position", w)?, "position")?,
        target: get(f, "target").map(|x| as_f32s(x, "target")).transpose()?,
        forward: get(f, "forward")
            .map(|x| as_f32s(x, "forward"))
            .transpose()?,
        up: as_f32s(need(f, "up", w)?, "up")?,
        yfov: as_f32(need(f, "yfov", w)?, "yfov")?,
        aspect: opt_f32("aspect")?,
        znear: opt_f32("znear")?,
        active: get(f, "active").map(|x| as_bool(x, "active")).transpose()?,
        lens_mm: opt_f32("lens_mm")?,
        sensor_mm: get(f, "sensor_mm")
            .map(|x| as_f32s(x, "sensor_mm"))
            .transpose()?,
        sensor_fit,
    })
}

fn camera_usable(c: &SceneCamera) -> Result<(), &'static str> {
    if !finite(&c.position) || !finite(&c.up) {
        return Err("position or up is not finite");
    }
    match (&c.target, &c.forward) {
        (Some(t), _) if !finite(t) => return Err("target is not finite"),
        (None, Some(f)) if !finite(f) => return Err("forward is not finite"),
        (None, None) => return Err("needs a target or a forward vector"),
        _ => {}
    }
    if !(c.yfov > 0.0 && c.yfov < std::f32::consts::PI) {
        return Err("yfov must be in (0, pi)");
    }
    if c.aspect.is_some_and(|a| !(a.is_finite() && a > 0.0)) {
        return Err("aspect must be positive");
    }
    if c.znear.is_some_and(|z| !z.is_finite()) {
        return Err("znear is not finite");
    }
    Ok(())
}

/// Reads `bytes` as a K2SCENE1 under `limits`, strictly: the scene, or why it is refused.
pub fn check(bytes: &[u8], limits: &MeshReadLimits) -> Result<Checked, String> {
    let len = bytes.len() as u64;
    over("input bytes", len, limits.max_output_bytes)?;
    if len < PREFIX {
        return Err(format!("truncated: need {PREFIX} bytes, have {len}"));
    }
    if &bytes[..8] != MAGIC {
        return Err("not a K2SCENE1 file".into());
    }
    let hlen = u64::from(u32::from_le_bytes(bytes[8..12].try_into().unwrap()));
    over("header bytes", hlen, limits.max_header_bytes)?;
    if PREFIX + hlen > len {
        return Err(format!(
            "truncated: need {} bytes, have {len}",
            PREFIX + hlen
        ));
    }
    let header = std::str::from_utf8(&bytes[12..(12 + hlen) as usize])
        .map_err(|_| "header: not UTF-8")?
        .to_owned();
    let top = parse_json(&header)?;
    let f = fields(
        &top,
        "header",
        &[
            "version",
            "up",
            "meshes",
            "objects",
            "cameras",
            "cameras_left_out",
        ],
    )?;
    let version = as_u32(need(f, "version", "header")?, "version")?;
    if version != VERSION {
        return Err(format!("header version {version}"));
    }
    let up = match get(f, "up") {
        None => "y".to_owned(),
        Some(x) => match as_str(x, "up")? {
            u @ ("y" | "z") => u.to_owned(),
            other => return Err(format!("up: unknown word {other:?}")),
        },
    };
    let (jm, jo, jc) = (
        list(f, "meshes", true)?,
        list(f, "objects", true)?,
        list(f, "cameras", false)?,
    );
    over("meshes", jm.len() as u64, limits.max_meshes)?;
    over("objects", jo.len() as u64, limits.max_objects)?;
    over("cameras", jc.len() as u64, limits.max_cameras)?;
    let infos = jm.iter().map(mesh_info).collect::<Result<Vec<_>, _>>()?;
    let objects = jo.iter().map(object).collect::<Result<Vec<_>, _>>()?;
    let cameras = jc.iter().map(camera).collect::<Result<Vec<_>, _>>()?;
    let jl = list(f, "cameras_left_out", false)?;
    if get(f, "cameras_left_out").is_some() && jl.is_empty() {
        return Err("cameras_left_out: written empty".into());
    }
    over("cameras left out", jl.len() as u64, limits.max_cameras)?;
    let cameras_left_out = jl.iter().map(left_out).collect::<Result<Vec<_>, _>>()?;
    let mut numbers = HashSet::new();
    for (i, o) in objects.iter().enumerate() {
        if !numbers.insert(key_number(&o.key, "@obj:", "object")?) {
            return Err(format!("object {i}: its key {} is given twice", o.key));
        }
    }
    for (i, c) in cameras.iter().enumerate() {
        if !numbers.insert(key_number(&c.key, "@cam:", "camera")?) {
            return Err(format!(
                "camera {i}: its key {} is given twice, or its number is an object's",
                c.key
            ));
        }
    }
    let verts: u64 = infos.iter().map(|m| u64::from(m.0)).sum();
    let tris: u64 = infos.iter().map(|m| u64::from(m.1)).sum();
    over("vertices", verts, limits.max_vertices)?;
    over("triangles", tris, limits.max_triangles)?;
    let (mut dv, mut dt) = (0u64, 0u64);
    for (i, o) in objects.iter().enumerate() {
        let m = infos
            .get(o.mesh as usize)
            .ok_or_else(|| format!("object {i} names mesh {}, which doesn't exist", o.mesh))?;
        dv += u64::from(m.0);
        dt += u64::from(m.1);
    }
    let mut active = false;
    for (i, c) in cameras.iter().enumerate() {
        if c.id.len() > MAX_ID_BYTES {
            return Err(format!("camera {i}: its id is too long"));
        }
        if c.active == Some(true) {
            if active {
                return Err(format!("camera {i}: a second active camera"));
            }
            active = true;
        }
        if c.lens_mm.is_some_and(|l| !(l.is_finite() && l > 0.0)) {
            return Err(format!("camera {i}: lens_mm must be finite and > 0"));
        }
        if c.sensor_mm
            .is_some_and(|s| !s.iter().all(|v| v.is_finite() && *v > 0.0))
        {
            return Err(format!("camera {i}: sensor_mm must be finite and > 0"));
        }
    }
    over("drawn vertices", dv, limits.max_drawn_vertices)?;
    over("drawn triangles", dt, limits.max_drawn_triangles)?;
    let off = align4(PREFIX + hlen);
    let data: u64 = infos
        .iter()
        .map(|&(v, t, n, _)| {
            let normals = if n { u64::from(v) * 12 } else { 0 };
            data_len(u64::from(v), u64::from(t)) + normals
        })
        .sum();
    let need_len = off + data;
    if len < need_len {
        return Err(format!("truncated: need {need_len} bytes, have {len}"));
    }
    if len > need_len {
        return Err(format!("{} bytes after the data", len - need_len));
    }
    if bytes[(PREFIX + hlen) as usize..off as usize]
        .iter()
        .any(|&b| b != 0)
    {
        return Err("the header's padding is not zero".into());
    }
    let mut at = off as usize;
    let f32_at = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let mut meshes = Vec::with_capacity(infos.len());
    let mut with_normals = 0;
    for (mi, &(nv, nt, normals, ref hash)) in infos.iter().enumerate() {
        let from = at;
        let mut positions = Vec::with_capacity(nv as usize);
        for _ in 0..nv {
            let p = [f32_at(at), f32_at(at + 4), f32_at(at + 8)];
            if !finite(&p) {
                return Err(format!("positions of mesh {mi} are not finite"));
            }
            positions.push(p);
            at += 12;
        }
        if normals {
            with_normals += 1;
            for _ in 0..nv {
                if !finite(&[f32_at(at), f32_at(at + 4), f32_at(at + 8)]) {
                    return Err(format!("normals of mesh {mi} are not finite"));
                }
                at += 12;
            }
        }
        let mut triangles = Vec::with_capacity(nt as usize);
        for _ in 0..nt {
            let t = [u32_at(at), u32_at(at + 4), u32_at(at + 8)];
            if let Some(i) = t.iter().find(|&&i| i >= nv) {
                return Err(format!("mesh {mi}: index {i} with {nv} vertices"));
            }
            triangles.push(t);
            at += 12;
        }
        let mut d = Vec::with_capacity(8 + (at - from));
        d.extend_from_slice(&nv.to_le_bytes());
        d.extend_from_slice(&nt.to_le_bytes());
        d.extend_from_slice(&bytes[from..at]);
        let want: String = Digest::of(&d)
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if *hash != want {
            return Err(format!("mesh {mi}: its hash isn't its counts' and data's"));
        }
        meshes.push(SceneMesh {
            positions,
            triangles,
        });
    }
    debug_assert_eq!(at as u64, len);
    for (i, o) in objects.iter().enumerate() {
        if !finite(&o.transform) {
            return Err(format!("transform of object {i} is not finite"));
        }
        if !finite(&o.base_color) {
            return Err(format!("base colour of object {i} is not finite"));
        }
    }
    for (i, c) in cameras.iter().enumerate() {
        camera_usable(c).map_err(|why| format!("camera {i}: {why}"))?;
    }
    Ok(Checked {
        scene: Scene {
            meshes,
            objects,
            cameras,
            cameras_left_out,
        },
        up,
        header,
        data_offset: off,
        meshes_with_normals: with_normals,
        hashes: infos.into_iter().map(|i| i.3).collect(),
    })
}
