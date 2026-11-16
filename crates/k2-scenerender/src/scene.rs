//! K2SCENE1: what the BLEND pack's host-only mesh read hands the host, and
//! the only thing the worker reads. The contract (K2 Render's lab README,
//! "Proposed contract", with K2 Cores' two object fields):
//!
//! ```text
//! magic    8 bytes  "K2SCENE1"
//! hlen     u32 LE   header length
//! header   hlen     UTF-8 JSON (`Header`): up axis, meshes (counts + hash), objects, cameras
//! pad      0-3      zero bytes up to a 4-byte boundary
//! data     per mesh, in header order: positions f32 LE x 3 x vertices,
//!          normals f32 LE x 3 x vertices (only when "normals"), indices u32 LE x 3 x triangles
//! ```
//!
//! ```json
//! {"version": 1, "up": "z",
//!  "meshes":  [{"vertices": 8, "triangles": 12, "normals": true, "hash": "…"}],
//!  "objects": [{"id": "Cube", "key": "@obj:12", "mesh": 0, "base_color": [0.8, 0.8, 0.8, 1.0],
//!               "double_sided": null, "transform": [16 floats, column-major], "hash": "…",
//!               "modifiers": ["SUBSURF", "MIRROR"], "modifiers_not_applied": true,
//!               "hidden": "collection", "transform_not_evaluated": true}],
//!  "cameras": [{"id": "Camera", "key": "@cam:3", "position": [7.4, -6.5, 5.3], "target": [0, 0, 0],
//!               "up": [0, 0, 1], "yfov": 0.69, "aspect": 1.7778, "znear": 0.1,
//!               "active": true, "lens_mm": 50, "sensor_mm": [36, 24], "sensor_fit": "auto"}],
//!  "cameras_left_out": [{"name": "Ortho", "key": "@cam:7", "reason": "orthographic"}]}
//! ```
//!
//! - **Meshes** (K2 Cores' v1 read): BASE meshes, triangulated by the pack,
//!   in mesh-local coordinates; counter-clockwise is the front face; a mesh
//!   may be shared by several objects (instancing is kept).
//! - **Objects:** `transform` is object-to-world, affine, column-major, as
//!   stored (no animation evaluated); a negative determinant mirrors (the
//!   winding flips). `base_color` is linear RGBA, clamped to [0, 1], alpha
//!   ignored. `double_sided`: `false` lets back faces be culled; `true` or
//!   `null` draws both sides. `modifiers` names the object's modifier types
//!   and `modifiers_not_applied` says the mesh is drawn without them: the
//!   response passes it through so the view can say "shown without modifiers".
//!   Two optional fields, absent from a header that predates them:
//!   `hidden` says the object is hidden in the file, `"self"` (hidden
//!   itself) or `"collection"` (in a hidden collection); absent, it is
//!   shown. A hidden object stays in the file with its mesh (the client has
//!   its geometry and its box) but the scene, camera and turntable views
//!   leave it out: not drawn, not fitted, not classed, not in the id image;
//!   every object keeps its index all the same. A view that names it in its
//!   `objects` draws it (an object's own view). `transform_not_evaluated`
//!   (`true`; absent, `false`) says the object's placement is driven
//!   (drivers, constraints) and it is drawn at its base location, rotation
//!   and scale, as stored: it changes no pixel, the response passes it
//!   through so the view can say "shown at base values", and it is in the
//!   digest of every view that draws the object (only when `true`), so that
//!   flag is never stale on a cached image.
//! - **Cameras** (optional): `target` or `forward`, `up`, `yfov` in radians,
//!   `aspect` width / height; the camera's name is its `id`. Four optional
//!   display fields, as the file has them, absent from a v1 header
//!   that predates them: `active` (the file's active camera; at most one
//!   camera has `true`), `lens_mm` (focal length) and `sensor_mm` (`[w, h]`),
//!   each finite and > 0, and `sensor_fit` (`"auto"`, `"horizontal"` or
//!   `"vertical"`). They are shown with the camera, never drawn: `yfov` is
//!   the only field that sets the view, whatever the lens, sensor or fit
//!   say, and none of them is in a digest ([`crate::digest`]).
//! - **Keys** (optional, on objects and cameras): the live element address
//!   the BLEND reader gives the object or camera, `"@obj:N"` or `"@cam:N"`
//!   (N its live key), which stays the same across edits and renames. The
//!   listing addresses the object's or camera's view by it
//!   ([`crate::digest`]). Checked with the header: a key must be a native
//!   address of one part in the multiplayer kit's grammar ([`crate::addr`])
//!   of kind `obj` on an object and `cam` on a camera, and unique among the
//!   objects (the cameras); else the whole input is refused
//!   (`invalid_input`). Uniqueness is per list: a camera object may be
//!   `@obj:N` among the objects and `@cam:N` among the cameras. A key draws
//!   nothing and is in no digest: with or without one, every digest is the
//!   same.
//! - **Cameras left out** (optional, top level; [`CamerasLeftOut`]): the
//!   cameras the producer couldn't express here, each `{name, key?,
//!   reason}`: `name` as the file has it, `key` its live address when the
//!   reader gives one, `reason` a short word or phrase saying why. It is
//!   informational and never refuses the input: an entry that isn't an
//!   object with a string `name` and `reason` (each at most
//!   [`MAX_LEFT_OUT_BYTES`]) and, when it has a `key`, a native address of
//!   one part of kind `cam` (checked as a camera's key is) is dropped and
//!   counted (`dropped`); of the rest, the first [`InputLimits::max_cameras`]
//!   are kept in order and the others counted (`more`), so a dropped entry
//!   never takes a slot. A value that isn't a list is read as absent, with
//!   `dropped` 1. Nothing draws them and they are in no digest: the host's
//!   listing passes them through, so the view can say which cameras it
//!   doesn't show. Absent (or empty), the bytes are as before the field
//!   existed. The header's own limits (its size first) apply as to any field.
//!
//! **Bounds first.** The worker reads the 12-byte prefix and the header before
//! any mesh data, and checks every count against [`InputLimits`] before it
//! allocates for it: the header length, the number of meshes, objects,
//! cameras and modifiers (counted while the JSON is parsed, so an oversized
//! list stops at its cap), the stored and the drawn (instancing expanded)
//! vertices and triangles, and the exact size the counts imply against the
//! input's (truncated input and trailing bytes are both refused). Then the
//! data, mesh by mesh, straight into its arrays: every index against its
//! mesh's vertex count, every float finite. A breach is a typed refusal
//! ([`InputError::code`]: `too_large` or `invalid_input`), never a crash.

use std::cell::Cell;
use std::fmt;
use std::io::Read;
use std::marker::PhantomData;

use serde::{de, Deserialize, Deserializer, Serialize};

pub const MAGIC: &[u8; 8] = b"K2SCENE1";
pub const VERSION: u32 = 1;
/// Magic and header length.
pub const PREFIX: u64 = 12;
/// Longest id (object, camera) or hash, in bytes.
pub const MAX_ID_BYTES: usize = 1024;
/// Most modifier names on one object, and the longest one in bytes.
pub const MAX_MODIFIERS: usize = 256;
pub const MAX_MODIFIER_BYTES: usize = 128;
/// The longest string of a camera left out (its name, key or reason), in bytes.
pub const MAX_LEFT_OUT_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpAxis {
    /// +Y up, front faces +Z (glTF's convention).
    #[default]
    Y,
    /// +Z up, front faces -Y (Blender's convention).
    Z,
}

/// How a camera's sensor fits the image (as the file has it; display only).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorFit {
    Auto,
    Horizontal,
    Vertical,
}

/// How an object is hidden in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hidden {
    /// The object itself is hidden.
    #[serde(rename = "self")]
    Itself,
    /// The object is in a hidden collection.
    Collection,
}

impl Hidden {
    /// Its word on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Hidden::Itself => "self",
            Hidden::Collection => "collection",
        }
    }
}

/// A camera's display values: shown with it, never drawn, in no digest.
/// Each is left out of the JSON when absent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct CameraDisplay {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lens_mm: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sensor_mm: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sensor_fit: Option<SensorFit>,
}

/// A perspective camera in world space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    #[serde(default)]
    pub id: String,
    /// Its live element address (`@cam:N`), when the reader gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub position: [f32; 3],
    /// A point looked at; or give `forward`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward: Option<[f32; 3]>,
    pub up: [f32; 3],
    /// Vertical field of view, radians.
    pub yfov: f32,
    /// Width / height; the image takes it when the request gives none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub znear: Option<f32>,
    /// Display only (see [`CameraDisplay`]): the file's active camera.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    /// Display only: focal length, mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens_mm: Option<f32>,
    /// Display only: sensor size `[w, h]`, mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensor_mm: Option<[f32; 2]>,
    /// Display only: how the sensor fits the image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensor_fit: Option<SensorFit>,
}

impl Camera {
    /// Its display values.
    pub fn display(&self) -> CameraDisplay {
        CameraDisplay { active: self.active, lens_mm: self.lens_mm, sensor_mm: self.sensor_mm, sensor_fit: self.sensor_fit }
    }
}

/// A camera the producer couldn't express in K2SCENE1 (see the module
/// docs): listed with the scene as sent, never drawn, in no digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraLeftOut {
    /// Its name as the file has it.
    pub name: String,
    /// Its live element address (`@cam:N`), when the reader gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Why it was left out, in a few words.
    pub reason: String,
}

/// The header's `cameras_left_out` as read: the entries kept, in order, and
/// how many were past the cap (`more`) or dropped as malformed (`dropped`).
/// Never refuses the input (see the module docs). Written back as the list
/// alone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CamerasLeftOut {
    pub list: Vec<CameraLeftOut>,
    /// Good entries past the cap ([`InputLimits::max_cameras`]), not kept.
    pub more: u64,
    /// Entries dropped as malformed (1 when the value isn't a list).
    pub dropped: u64,
}

impl CamerasLeftOut {
    /// Nothing kept and nothing counted.
    pub fn is_empty(&self) -> bool {
        self.list.is_empty() && self.more == 0 && self.dropped == 0
    }

    fn not_a_list() -> Self {
        CamerasLeftOut { dropped: 1, ..Default::default() }
    }

    fn no_list(&self) -> bool {
        self.list.is_empty()
    }
}

impl Serialize for CamerasLeftOut {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.list.serialize(s)
    }
}

/// One entry of `cameras_left_out`, or `None` to drop it.
fn left_out_entry(v: serde_json::Value) -> Option<CameraLeftOut> {
    use serde_json::Value as J;
    let J::Object(mut o) = v else { return None };
    let text = |x: Option<J>| match x {
        Some(J::String(s)) if s.len() <= MAX_LEFT_OUT_BYTES => Some(s),
        _ => None,
    };
    let name = text(o.remove("name"))?;
    let reason = text(o.remove("reason"))?;
    let key = match o.remove("key") {
        None | Some(J::Null) => None,
        Some(J::String(k)) if k.len() <= MAX_LEFT_OUT_BYTES && crate::addr::single(&k).is_some_and(|(kind, _)| kind == "cam") => Some(k),
        Some(_) => return None,
    };
    Some(CameraLeftOut { name, key, reason })
}

impl<'de> Deserialize<'de> for CamerasLeftOut {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> de::Visitor<'de> for V {
            type Value = CamerasLeftOut;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a list of cameras left out")
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<CamerasLeftOut, A::Error> {
                let cap = CAPS.with(|c| c.get()[LIST_CAMERAS_LEFT_OUT]);
                let mut out = CamerasLeftOut::default();
                // One entry in memory at a time; the header's size bounds each.
                while let Some(v) = seq.next_element::<serde_json::Value>()? {
                    match left_out_entry(v) {
                        None => out.dropped += 1,
                        Some(e) if (out.list.len() as u64) < cap => out.list.push(e),
                        Some(_) => out.more += 1,
                    }
                }
                Ok(out)
            }
            fn visit_map<A: de::MapAccess<'de>>(self, mut m: A) -> Result<CamerasLeftOut, A::Error> {
                while m.next_entry::<de::IgnoredAny, de::IgnoredAny>()?.is_some() {}
                Ok(CamerasLeftOut::not_a_list())
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<CamerasLeftOut, E> {
                Ok(CamerasLeftOut::not_a_list())
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<CamerasLeftOut, E> {
                Ok(CamerasLeftOut::not_a_list())
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<CamerasLeftOut, E> {
                Ok(CamerasLeftOut::not_a_list())
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<CamerasLeftOut, E> {
                Ok(CamerasLeftOut::not_a_list())
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<CamerasLeftOut, E> {
                Ok(CamerasLeftOut::not_a_list())
            }
            fn visit_unit<E: de::Error>(self) -> Result<CamerasLeftOut, E> {
                Ok(CamerasLeftOut::not_a_list())
            }
            fn visit_none<E: de::Error>(self) -> Result<CamerasLeftOut, E> {
                Ok(CamerasLeftOut::not_a_list())
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshInfo {
    pub vertices: u32,
    pub triangles: u32,
    pub normals: bool,
    /// The pack's hash of the geometry (informational: the render digest
    /// hashes the data itself, `crate::digest`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

/// One drawable: a mesh with a material and a placement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: String,
    /// Its live element address (`@obj:N`), when the reader gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub mesh: u32,
    /// Linear RGBA base colour (alpha is ignored: everything is drawn opaque).
    pub base_color: [f32; 4],
    /// `Some(false)`: back faces may be culled. `None` (unknown) or `Some(true)`: both sides drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub double_sided: Option<bool>,
    /// Object-to-world, column-major 4x4 (affine).
    pub transform: [f32; 16],
    /// The pack's hash of the object, if it has one (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// The object's modifier types, by name (K2 Cores).
    #[serde(default, skip_serializing_if = "Vec::is_empty", deserialize_with = "de_modifiers")]
    pub modifiers: Vec<String>,
    /// The mesh is the base mesh, drawn without its modifiers (K2 Cores' v1 read).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub modifiers_not_applied: bool,
    /// Hidden in the file: the scene, camera and turntable views leave it
    /// out; a view naming it draws it. `None`: shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<Hidden>,
    /// Its placement is driven and not evaluated: drawn at its base
    /// transform (no pixel changes; in the digest of the views drawing it
    /// when `true`, for the flag they carry).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub transform_not_evaluated: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub version: u32,
    #[serde(default)]
    pub up: UpAxis,
    #[serde(deserialize_with = "de_meshes")]
    pub meshes: Vec<MeshInfo>,
    #[serde(deserialize_with = "de_objects")]
    pub objects: Vec<Object>,
    #[serde(default, deserialize_with = "de_cameras")]
    pub cameras: Vec<Camera>,
    /// The cameras the producer couldn't express (absent: none). Never
    /// refuses the input ([`CamerasLeftOut`]).
    #[serde(default, skip_serializing_if = "CamerasLeftOut::no_list")]
    pub cameras_left_out: CamerasLeftOut,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub indices: Vec<[u32; 3]>,
    pub hash: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub up: UpAxis,
    pub meshes: Vec<Mesh>,
    pub objects: Vec<Object>,
    pub cameras: Vec<Camera>,
    /// Passed through from the header; nothing reads it to draw.
    pub cameras_left_out: CamerasLeftOut,
}

/// What a reader accepts. "Drawn" counts expand instancing (a mesh used by
/// n objects counts n times). The worker's come from the host on argv.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputLimits {
    pub max_input_bytes: u64,
    pub max_header_bytes: u32,
    pub max_meshes: u32,
    pub max_objects: u32,
    pub max_cameras: u32,
    pub max_vertices: u64,
    pub max_triangles: u64,
    pub max_drawn_vertices: u64,
    pub max_drawn_triangles: u64,
}

impl Default for InputLimits {
    /// K2's defaults for thumbnails and stills alike (K2 Render's lab result).
    fn default() -> Self {
        InputLimits {
            max_input_bytes: 256 << 20,
            max_header_bytes: 32 << 20,
            max_meshes: 100_000,
            max_objects: 100_000,
            max_cameras: 1_000,
            max_vertices: 8_000_000,
            max_triangles: 8_000_000,
            max_drawn_vertices: 8_000_000,
            max_drawn_triangles: 8_000_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum InputError {
    TooLarge { what: &'static str, got: u64, max: u64 },
    /// A list in the header longer than its cap (counted while parsing).
    TooMany { what: &'static str, max: u64 },
    BadMagic,
    UnsupportedVersion(u32),
    Truncated { need: u64, have: u64 },
    TrailingBytes { extra: u64 },
    BadPadding,
    Header(String),
    BadMeshRef { object: u32, mesh: u32 },
    IndexOutOfRange { mesh: u32, index: u32, vertices: u32 },
    NotFinite { what: &'static str, at: u32 },
    BadCamera { camera: u32, why: &'static str },
    BadField { what: &'static str, at: u32, why: &'static str },
}

impl InputError {
    /// The wire code: `too_large` for a limit, `invalid_input` for everything else.
    pub fn code(&self) -> &'static str {
        match self {
            InputError::TooLarge { .. } | InputError::TooMany { .. } => "too_large",
            _ => "invalid_input",
        }
    }
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InputError::TooLarge { what, got, max } => write!(f, "{what}: {got}, at most {max}"),
            InputError::TooMany { what, max } => write!(f, "{what}: more than {max}"),
            InputError::BadMagic => write!(f, "not a K2SCENE1 file"),
            InputError::UnsupportedVersion(v) => write!(f, "header version {v} (this reader: {VERSION})"),
            InputError::Truncated { need, have } => write!(f, "truncated: need {need} bytes, have {have}"),
            InputError::TrailingBytes { extra } => write!(f, "{extra} bytes after the data"),
            InputError::BadPadding => write!(f, "the header's padding is not zero"),
            InputError::Header(e) => write!(f, "header: {e}"),
            InputError::BadMeshRef { object, mesh } => write!(f, "object {object} names mesh {mesh}, which doesn't exist"),
            InputError::IndexOutOfRange { mesh, index, vertices } => write!(f, "mesh {mesh}: index {index} with {vertices} vertices"),
            InputError::NotFinite { what, at } => write!(f, "{what} {at} is not finite"),
            InputError::BadCamera { camera, why } => write!(f, "camera {camera}: {why}"),
            InputError::BadField { what, at, why } => write!(f, "{what} {at}: {why}"),
        }
    }
}

impl std::error::Error for InputError {}

fn too_large(what: &'static str, got: u64, max: u64) -> Result<(), InputError> {
    if got > max {
        Err(InputError::TooLarge { what, got, max })
    } else {
        Ok(())
    }
}

// ── Lists capped while the header is parsed ─────────────────────────────

const LIST_MESHES: usize = 0;
const LIST_OBJECTS: usize = 1;
const LIST_CAMERAS: usize = 2;
const LIST_MODIFIERS: usize = 3;
/// Not refused past its cap: the cameras left out keep that many and count the rest.
const LIST_CAMERAS_LEFT_OUT: usize = 4;
const LIST_NAMES: [&str; 5] = ["meshes", "objects", "cameras", "modifiers of an object", "cameras left out"];

thread_local! {
    /// The caps of the header being parsed on this thread (unbounded outside [`parse_header`]).
    static CAPS: Cell<[u64; 5]> = const { Cell::new([u64::MAX; 5]) };
    /// The first list that passed its cap.
    static OVER: Cell<Option<usize>> = const { Cell::new(None) };
}

fn capped<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D, which: usize) -> Result<Vec<T>, D::Error> {
    struct V<T>(usize, PhantomData<T>);
    impl<'de, T: Deserialize<'de>> de::Visitor<'de> for V<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a list")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
            let cap = CAPS.with(|c| c.get()[self.0]);
            let mut v = Vec::new();
            while let Some(x) = seq.next_element()? {
                if v.len() as u64 >= cap {
                    OVER.with(|o| {
                        if o.get().is_none() {
                            o.set(Some(self.0));
                        }
                    });
                    return Err(de::Error::custom(format!("{}: more than {cap}", LIST_NAMES[self.0])));
                }
                v.push(x);
            }
            Ok(v)
        }
    }
    d.deserialize_seq(V(which, PhantomData))
}

fn de_meshes<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<MeshInfo>, D::Error> {
    capped(d, LIST_MESHES)
}

fn de_objects<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Object>, D::Error> {
    capped(d, LIST_OBJECTS)
}

fn de_cameras<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Camera>, D::Error> {
    capped(d, LIST_CAMERAS)
}

fn de_modifiers<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    capped(d, LIST_MODIFIERS)
}


/// Parse a header's JSON with its lists capped by `lim`.
pub fn parse_header(json: &[u8], lim: &InputLimits) -> Result<Header, InputError> {
    let caps = [lim.max_meshes as u64, lim.max_objects as u64, lim.max_cameras as u64, MAX_MODIFIERS as u64, lim.max_cameras as u64];
    CAPS.with(|c| c.set(caps));
    OVER.with(|o| o.set(None));
    let parsed = serde_json::from_slice::<Header>(json);
    CAPS.with(|c| c.set([u64::MAX; 5]));
    let over = OVER.with(|o| o.take());
    match (parsed, over) {
        (_, Some(which)) => Err(InputError::TooMany { what: LIST_NAMES[which], max: caps[which] }),
        (Err(e), None) => Err(InputError::Header(e.to_string())),
        (Ok(h), None) => Ok(h),
    }
}

fn align4(n: u64) -> u64 {
    (n + 3) & !3
}

/// The data bytes of one mesh.
pub fn data_len(m: &MeshInfo) -> u64 {
    let v = m.vertices as u64 * 12;
    let n = if m.normals { v } else { 0 };
    v + n + m.triangles as u64 * 12
}

/// Counts drawn by `objects` (instancing expanded): (vertices, triangles).
pub fn drawn_counts(meshes: &[MeshInfo], objects: &[Object]) -> (u64, u64) {
    let mut v = 0u64;
    let mut t = 0u64;
    for o in objects {
        if let Some(m) = meshes.get(o.mesh as usize) {
            v += m.vertices as u64;
            t += m.triangles as u64;
        }
    }
    (v, t)
}

/// The checks on a parsed header that need no mesh data. Returns the data's
/// offset (after the padding) and its length.
pub fn check_header(h: &Header, hlen: u64, input_len: u64, lim: &InputLimits) -> Result<(u64, u64), InputError> {
    if h.version != VERSION {
        return Err(InputError::UnsupportedVersion(h.version));
    }
    too_large("meshes", h.meshes.len() as u64, lim.max_meshes as u64)?;
    too_large("objects", h.objects.len() as u64, lim.max_objects as u64)?;
    too_large("cameras", h.cameras.len() as u64, lim.max_cameras as u64)?;
    let verts: u64 = h.meshes.iter().map(|m| m.vertices as u64).sum();
    let tris: u64 = h.meshes.iter().map(|m| m.triangles as u64).sum();
    too_large("vertices", verts, lim.max_vertices)?;
    too_large("triangles", tris, lim.max_triangles)?;
    for (i, m) in h.meshes.iter().enumerate() {
        if m.hash.as_ref().is_some_and(|s| s.len() > MAX_ID_BYTES) {
            return Err(InputError::BadField { what: "mesh", at: i as u32, why: "its hash is too long" });
        }
    }
    for (i, o) in h.objects.iter().enumerate() {
        let at = i as u32;
        if o.mesh as usize >= h.meshes.len() {
            return Err(InputError::BadMeshRef { object: at, mesh: o.mesh });
        }
        if o.id.len() > MAX_ID_BYTES || o.hash.as_ref().is_some_and(|s| s.len() > MAX_ID_BYTES) {
            return Err(InputError::BadField { what: "object", at, why: "its id or hash is too long" });
        }
        if o.modifiers.iter().any(|m| m.len() > MAX_MODIFIER_BYTES) {
            return Err(InputError::BadField { what: "object", at, why: "a modifier name is too long" });
        }
    }
    check_keys("object", "obj", h.objects.iter().map(|o| o.key.as_deref()))?;
    check_keys("camera", "cam", h.cameras.iter().map(|c| c.key.as_deref()))?;
    let mut active = false;
    for (i, c) in h.cameras.iter().enumerate() {
        if c.id.len() > MAX_ID_BYTES {
            return Err(InputError::BadField { what: "camera", at: i as u32, why: "its id is too long" });
        }
        // The display fields: checked here, so the listing refuses what the renderer refuses.
        let bad = |why: &'static str| Err(InputError::BadCamera { camera: i as u32, why });
        if c.active == Some(true) {
            if active {
                return bad("a second active camera (at most one is active)");
            }
            active = true;
        }
        if c.lens_mm.is_some_and(|l| !(l.is_finite() && l > 0.0)) {
            return bad("lens_mm must be finite and > 0");
        }
        if c.sensor_mm.is_some_and(|s| !s.iter().all(|v| v.is_finite() && *v > 0.0)) {
            return bad("sensor_mm must be finite and > 0");
        }
    }
    let (dv, dt) = drawn_counts(&h.meshes, &h.objects);
    too_large("drawn vertices", dv, lim.max_drawn_vertices)?;
    too_large("drawn triangles", dt, lim.max_drawn_triangles)?;
    let off = align4(PREFIX + hlen);
    let data: u64 = h.meshes.iter().map(data_len).sum();
    let need = off + data;
    if input_len < need {
        return Err(InputError::Truncated { need, have: input_len });
    }
    if input_len > need {
        return Err(InputError::TrailingBytes { extra: input_len - need });
    }
    Ok((off, data))
}

/// Reads a K2SCENE1 of a known length from a stream (stdin, or a slice).
pub struct Decoder<R> {
    r: R,
    pos: u64,
    len: u64,
    hlen: u64,
    lim: InputLimits,
    buf: Vec<u8>,
}

/// Bytes read at a time from the data (a multiple of 12).
const CHUNK: usize = 12 * 5461;

impl<R: Read> Decoder<R> {
    /// `input_len`: the input's exact length (the request's `input_len`).
    pub fn new(r: R, input_len: u64, lim: InputLimits) -> Decoder<R> {
        Decoder { r, pos: 0, len: input_len, hlen: 0, lim, buf: Vec::new() }
    }

    /// The header's JSON length (after [`Decoder::header`]).
    pub fn header_bytes(&self) -> u64 {
        self.hlen
    }

    fn exact(&mut self, out: &mut [u8]) -> Result<(), InputError> {
        let end = self.pos + out.len() as u64;
        if end > self.len {
            return Err(InputError::Truncated { need: end, have: self.len });
        }
        self.r.read_exact(out).map_err(|_| InputError::Truncated { need: self.len, have: self.pos })?;
        self.pos = end;
        Ok(())
    }

    /// The prefix and the header, every bound checked; no mesh data read.
    pub fn header(&mut self) -> Result<Header, InputError> {
        too_large("input bytes", self.len, self.lim.max_input_bytes)?;
        if self.len < PREFIX {
            return Err(InputError::Truncated { need: PREFIX, have: self.len });
        }
        let mut pre = [0u8; PREFIX as usize];
        self.exact(&mut pre)?;
        if &pre[..8] != MAGIC {
            return Err(InputError::BadMagic);
        }
        let hlen = u32::from_le_bytes([pre[8], pre[9], pre[10], pre[11]]) as u64;
        too_large("header bytes", hlen, self.lim.max_header_bytes as u64)?;
        self.hlen = hlen;
        if PREFIX + hlen > self.len {
            return Err(InputError::Truncated { need: PREFIX + hlen, have: self.len });
        }
        let mut json = vec![0u8; hlen as usize];
        self.exact(&mut json)?;
        let h = parse_header(&json, &self.lim)?;
        drop(json);
        let (off, _) = check_header(&h, hlen, self.len, &self.lim)?;
        let mut pad = [0u8; 3];
        let n = (off - self.pos) as usize;
        self.exact(&mut pad[..n])?;
        if pad[..n].iter().any(|&b| b != 0) {
            return Err(InputError::BadPadding);
        }
        Ok(h)
    }

    fn vec3s(&mut self, n: usize, what: &'static str, mesh: u32) -> Result<Vec<[f32; 3]>, InputError> {
        let mut out = Vec::with_capacity(n);
        let mut left = n * 12;
        if self.buf.len() < CHUNK {
            self.buf = vec![0u8; CHUNK];
        }
        let mut buf = std::mem::take(&mut self.buf);
        while left > 0 {
            let k = left.min(CHUNK);
            self.exact(&mut buf[..k])?;
            for c in buf[..k].as_chunks::<12>().0 {
                let v = [
                    f32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                    f32::from_le_bytes([c[4], c[5], c[6], c[7]]),
                    f32::from_le_bytes([c[8], c[9], c[10], c[11]]),
                ];
                if !(v[0].is_finite() && v[1].is_finite() && v[2].is_finite()) {
                    return Err(InputError::NotFinite { what, at: mesh });
                }
                out.push(v);
            }
            left -= k;
        }
        self.buf = buf;
        Ok(out)
    }

    fn triangles(&mut self, n: usize, mesh: u32, vertices: u32) -> Result<Vec<[u32; 3]>, InputError> {
        let mut out = Vec::with_capacity(n);
        let mut left = n * 12;
        if self.buf.len() < CHUNK {
            self.buf = vec![0u8; CHUNK];
        }
        let mut buf = std::mem::take(&mut self.buf);
        while left > 0 {
            let k = left.min(CHUNK);
            self.exact(&mut buf[..k])?;
            for c in buf[..k].as_chunks::<12>().0 {
                let t = [
                    u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                    u32::from_le_bytes([c[4], c[5], c[6], c[7]]),
                    u32::from_le_bytes([c[8], c[9], c[10], c[11]]),
                ];
                if let Some(&index) = t.iter().find(|&&i| i >= vertices) {
                    return Err(InputError::IndexOutOfRange { mesh, index, vertices });
                }
                out.push(t);
            }
            left -= k;
        }
        self.buf = buf;
        Ok(out)
    }

    /// The mesh data, after [`Decoder::header`], and the objects' and cameras' checks.
    pub fn data(mut self, h: Header) -> Result<Scene, InputError> {
        let mut meshes = Vec::with_capacity(h.meshes.len());
        for (mi, m) in h.meshes.iter().enumerate() {
            let mi = mi as u32;
            let nv = m.vertices as usize;
            let positions = self.vec3s(nv, "positions of mesh", mi)?;
            let normals = if m.normals { Some(self.vec3s(nv, "normals of mesh", mi)?) } else { None };
            let indices = self.triangles(m.triangles as usize, mi, m.vertices)?;
            meshes.push(Mesh { positions, normals, indices, hash: m.hash.clone() });
        }
        debug_assert_eq!(self.pos, self.len);
        let mut objects = h.objects;
        for (i, o) in objects.iter_mut().enumerate() {
            if !o.transform.iter().all(|v| v.is_finite()) {
                return Err(InputError::NotFinite { what: "transform of object", at: i as u32 });
            }
            if !o.base_color.iter().all(|v| v.is_finite()) {
                return Err(InputError::NotFinite { what: "base colour of object", at: i as u32 });
            }
            for c in o.base_color.iter_mut() {
                *c = c.clamp(0.0, 1.0);
            }
        }
        for (i, c) in h.cameras.iter().enumerate() {
            check_camera(c).map_err(|why| InputError::BadCamera { camera: i as u32, why })?;
        }
        Ok(Scene { up: h.up, meshes, objects, cameras: h.cameras, cameras_left_out: h.cameras_left_out })
    }
}

/// Decode and check a whole K2SCENE1 held in memory.
pub fn decode(bytes: &[u8], lim: &InputLimits) -> Result<Scene, InputError> {
    let mut d = Decoder::new(bytes, bytes.len() as u64, *lim);
    let h = d.header()?;
    d.data(h)
}

/// The prefix and header of a K2SCENE1 held in memory, checked, and where
/// each mesh's data lies (`[start, end)` in `bytes`). For listing without
/// decoding the data (the daemon's digests).
pub fn layout(bytes: &[u8], lim: &InputLimits) -> Result<(Header, Vec<std::ops::Range<usize>>), InputError> {
    let mut d = Decoder::new(bytes, bytes.len() as u64, *lim);
    let h = d.header()?;
    let mut at = d.pos as usize;
    let mut ranges = Vec::with_capacity(h.meshes.len());
    for m in &h.meshes {
        let n = data_len(m) as usize;
        ranges.push(at..at + n);
        at += n;
    }
    Ok((h, ranges))
}

/// The keys of one list (objects or cameras): each a native address of one
/// part of kind `kind` ([`crate::addr`]), at most [`MAX_ID_BYTES`] long, and
/// none twice.
fn check_keys<'a>(what: &'static str, kind: &str, keys: impl Iterator<Item = Option<&'a str>>) -> Result<(), InputError> {
    let mut seen = std::collections::HashSet::new();
    for (i, k) in keys.enumerate() {
        let Some(k) = k else { continue };
        let at = i as u32;
        let why = if what == "object" { "its key must be a native address @obj:<id> (no spaces)" } else { "its key must be a native address @cam:<id> (no spaces)" };
        if k.len() > MAX_ID_BYTES || crate::addr::single(k).is_none_or(|(kd, _)| kd != kind) {
            return Err(InputError::BadField { what, at, why });
        }
        if !seen.insert(k) {
            return Err(InputError::BadField { what, at, why: if what == "object" { "its key is another object's" } else { "its key is another camera's" } });
        }
    }
    Ok(())
}

fn check_camera(c: &Camera) -> Result<(), &'static str> {
    let fin = |v: &[f32]| v.iter().all(|x| x.is_finite());
    if !fin(&c.position) || !fin(&c.up) {
        return Err("position or up is not finite");
    }
    match (&c.target, &c.forward) {
        (Some(t), _) if !fin(t) => return Err("target is not finite"),
        (None, Some(f)) if !fin(f) => return Err("forward is not finite"),
        (None, None) => return Err("needs a target or a forward vector"),
        _ => {}
    }
    if !(c.yfov > 0.0 && c.yfov < std::f32::consts::PI) {
        return Err("yfov must be in (0, pi)");
    }
    if let Some(a) = c.aspect {
        if !(a.is_finite() && a > 0.0) {
            return Err("aspect must be positive");
        }
    }
    if let Some(z) = c.znear {
        if !z.is_finite() {
            return Err("znear is not finite");
        }
    }
    Ok(())
}

pub fn header_of(scene: &Scene) -> Header {
    Header {
        version: VERSION,
        up: scene.up,
        meshes: scene
            .meshes
            .iter()
            .map(|m| MeshInfo {
                vertices: m.positions.len() as u32,
                triangles: m.indices.len() as u32,
                normals: m.normals.is_some(),
                hash: m.hash.clone(),
            })
            .collect(),
        objects: scene.objects.clone(),
        cameras: scene.cameras.clone(),
        cameras_left_out: scene.cameras_left_out.clone(),
    }
}

/// Encode a scene as K2SCENE1 (tests, fuzz seeds; the pack writes its own).
pub fn encode(scene: &Scene) -> Vec<u8> {
    let json = serde_json::to_vec(&header_of(scene)).unwrap_or_default();
    let data: usize = scene
        .meshes
        .iter()
        .map(|m| m.positions.len() * 12 * (if m.normals.is_some() { 2 } else { 1 }) + m.indices.len() * 12)
        .sum();
    let mut out = Vec::with_capacity(align4(PREFIX + json.len() as u64) as usize + data);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    for m in &scene.meshes {
        for v in m.positions.iter().flatten() {
            out.extend_from_slice(&v.to_le_bytes());
        }
        if let Some(n) = &m.normals {
            for v in n.iter().flatten() {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        for i in m.indices.iter().flatten() {
            out.extend_from_slice(&i.to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testgen;

    fn with_header(json: &str, data: &[u8]) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(json.as_bytes());
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn round_trip_with_the_modifier_fields() {
        let mut s = testgen::cube_scene();
        s.objects[0].modifiers = vec!["SUBSURF".into(), "MIRROR".into()];
        s.objects[0].modifiers_not_applied = true;
        let b = encode(&s);
        assert_eq!(decode(&b, &InputLimits::default()).expect("decode"), s);
        let text = String::from_utf8_lossy(&b[12..]).into_owned();
        assert!(text.contains(r#""modifiers":["SUBSURF","MIRROR"],"modifiers_not_applied":true"#), "{text}");
        // Absent in the JSON: none, and applied.
        let plain = testgen::cube_scene();
        let d = decode(&encode(&plain), &InputLimits::default()).expect("decode");
        assert!(d.objects[0].modifiers.is_empty() && !d.objects[0].modifiers_not_applied);
    }

    #[test]
    fn hidden_and_transform_not_evaluated_round_trip_and_are_absent_by_default() {
        let mut s = testgen::grid_scene(2, false);
        s.objects[1].hidden = Some(Hidden::Itself);
        s.objects[2].hidden = Some(Hidden::Collection);
        s.objects[3].transform_not_evaluated = true;
        let b = encode(&s);
        assert_eq!(decode(&b, &InputLimits::default()).expect("decode"), s);
        let text = String::from_utf8_lossy(&b[12..]).into_owned();
        assert!(text.contains(r#""hidden":"self""#) && text.contains(r#""hidden":"collection""#), "{text}");
        assert!(text.contains(r#""transform_not_evaluated":true"#), "{text}");
        assert_eq!((Hidden::Itself.as_str(), Hidden::Collection.as_str()), ("self", "collection"));
        // Absent: shown and evaluated, and the JSON is what it was.
        let plain = testgen::grid_scene(2, false);
        let text = String::from_utf8_lossy(&encode(&plain)[12..]).into_owned();
        assert!(!text.contains("hidden") && !text.contains("transform_not_evaluated"), "{text}");
        let d = decode(&encode(&plain), &InputLimits::default()).expect("decode");
        assert!(d.objects.iter().all(|o| o.hidden.is_none() && !o.transform_not_evaluated));
    }

    /// A reader ignores an object field it doesn't know, so a field added
    /// later is safe for it.
    #[test]
    fn an_unknown_object_field_is_ignored() {
        let obj = |extra: &str| format!(r#"{{"id":"a","mesh":0,"base_color":[1,1,1,1],"transform":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]{extra}}}"#);
        let j = |extra: &str| format!(r#"{{"version":1,"meshes":[{{"vertices":0,"triangles":0,"normals":false}}],"objects":[{}]}}"#, obj(extra));
        let lim = InputLimits::default();
        let plain = decode(&with_header(&j(""), &[]), &lim).expect("plain");
        let future = decode(&with_header(&j(r#","future_field":1,"another":{"x":[1,2]}"#), &[]), &lim).expect("an unknown field");
        assert_eq!(future, plain);
        assert!(crate::digest::list(&with_header(&j(r#","future_field":1"#), &[]), &lim).is_ok());
        // `null` for the hidden state is shown.
        assert_eq!(decode(&with_header(&j(r#","hidden":null"#), &[]), &lim).expect("null").objects[0].hidden, None);
    }

    #[test]
    fn a_hidden_value_other_than_self_or_collection_is_invalid_input() {
        let lim = InputLimits::default();
        let j = |extra: &str| format!(r#"{{"version":1,"meshes":[{{"vertices":0,"triangles":0,"normals":false}}],"objects":[{{"id":"a","mesh":0,"base_color":[1,1,1,1],"transform":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]{extra}}}]}}"#);
        for (extra, want) in [(r#","hidden":"self""#, Some(Hidden::Itself)), (r#","hidden":"collection""#, Some(Hidden::Collection))] {
            assert_eq!(decode(&with_header(&j(extra), &[]), &lim).expect(extra).objects[0].hidden, want);
        }
        for extra in [r#","hidden":"Self""#, r#","hidden":"view_layer""#, r#","hidden":"""#, r#","hidden":true"#, r#","hidden":1"#, r#","transform_not_evaluated":"yes""#] {
            let b = with_header(&j(extra), &[]);
            let e = decode(&b, &lim).unwrap_err();
            assert!(matches!(e, InputError::Header(_)), "{extra}: {e}");
            assert_eq!(e.code(), "invalid_input", "{extra}");
            // The listing refuses it too (it reads the header only).
            assert!(crate::digest::list(&b, &lim).is_err(), "{extra}");
        }
    }

    #[test]
    fn camera_display_fields_round_trip_and_a_v1_header_reads_without_them() {
        let mut s = testgen::camera_scene();
        s.cameras[0].active = Some(true);
        s.cameras[0].lens_mm = Some(50.0);
        s.cameras[0].sensor_mm = Some([36.0, 24.0]);
        s.cameras[0].sensor_fit = Some(SensorFit::Auto);
        let b = encode(&s);
        assert_eq!(decode(&b, &InputLimits::default()).expect("decode"), s);
        let text = String::from_utf8_lossy(&b[12..]).into_owned();
        assert!(text.contains(r#""active":true,"lens_mm":50.0,"sensor_mm":[36.0,24.0],"sensor_fit":"auto""#), "{text}");
        // Absent: none of them, and the JSON is what it was.
        let plain = testgen::camera_scene();
        let text = String::from_utf8_lossy(&encode(&plain)[12..]).into_owned();
        assert!(!text.contains("active") && !text.contains("lens_mm") && !text.contains("sensor"), "{text}");
        let j = r#"{"version":1,"meshes":[],"objects":[],"cameras":[{"id":"C","position":[0,0,5],"target":[0,0,0],"up":[0,1,0],"yfov":0.8}]}"#;
        let c = &decode(&with_header(j, &[]), &InputLimits::default()).expect("v1").cameras[0];
        assert_eq!(c.display(), CameraDisplay::default());
        for (fit, want) in [("auto", SensorFit::Auto), ("horizontal", SensorFit::Horizontal), ("vertical", SensorFit::Vertical)] {
            let j = format!(r#"{{"version":1,"meshes":[],"objects":[],"cameras":[{{"position":[0,0,5],"target":[0,0,0],"up":[0,1,0],"yfov":0.8,"sensor_fit":"{fit}"}}]}}"#);
            assert_eq!(decode(&with_header(&j, &[]), &InputLimits::default()).expect("fit").cameras[0].sensor_fit, Some(want));
        }
    }

    #[test]
    fn camera_display_fields_are_checked_typed() {
        let lim = InputLimits::default();
        let cam = |extra: &str| format!(r#"{{"position":[0,0,5],"target":[0,0,0],"up":[0,1,0],"yfov":0.8{extra}}}"#);
        let scene = |cams: &[String]| with_header(&format!(r#"{{"version":1,"meshes":[],"objects":[],"cameras":[{}]}}"#, cams.join(",")), &[]);
        // One active camera, others false or absent: fine. Two: refused.
        assert!(decode(&scene(&[cam(r#","active":false"#), cam(r#","active":true"#), cam("")]), &lim).is_ok());
        let e = decode(&scene(&[cam(r#","active":true"#), cam(r#","active":false"#), cam(r#","active":true"#)]), &lim).unwrap_err();
        assert!(matches!(e, InputError::BadCamera { camera: 2, .. }), "{e}");
        assert_eq!(e.code(), "invalid_input");
        for extra in [r#","lens_mm":0"#, r#","lens_mm":-50"#, r#","lens_mm":1e39"#, r#","sensor_mm":[36,0]"#, r#","sensor_mm":[-1,24]"#, r#","sensor_mm":[1e39,24]"#] {
            let e = decode(&scene(&[cam(extra)]), &lim).unwrap_err();
            assert!(matches!(e, InputError::BadCamera { camera: 0, .. }), "{extra}: {e}");
        }
        for extra in [r#","sensor_fit":"diagonal""#, r#","sensor_mm":[36]"#, r#","active":1"#] {
            assert_eq!(decode(&scene(&[cam(extra)]), &lim).unwrap_err().code(), "invalid_input", "{extra}");
        }
        // The listing refuses them too (it reads the header only).
        assert!(crate::digest::list(&scene(&[cam(r#","active":true"#), cam(r#","active":true"#)]), &lim).is_err());
    }

    #[test]
    fn refusals_are_typed() {
        let s = testgen::cube_scene();
        let b = encode(&s);
        let lim = InputLimits::default();
        assert!(matches!(decode(&b[..b.len() - 1], &lim), Err(InputError::Truncated { .. })));
        assert_eq!(decode(&b[..b.len() - 1], &lim).unwrap_err().code(), "invalid_input");
        let mut extra = b.clone();
        extra.push(0);
        assert!(matches!(decode(&extra, &lim), Err(InputError::TrailingBytes { extra: 1 })));
        let mut bad = b.clone();
        bad[0] = b'X';
        assert_eq!(decode(&bad, &lim), Err(InputError::BadMagic));
        let mut oob = b.clone();
        let n = oob.len();
        oob[n - 4..].copy_from_slice(&99u32.to_le_bytes());
        assert!(matches!(decode(&oob, &lim), Err(InputError::IndexOutOfRange { index: 99, .. })));
        let tight = InputLimits { max_drawn_triangles: 11, ..lim };
        let e = decode(&b, &tight).unwrap_err();
        assert_eq!(e.code(), "too_large");
        assert!(matches!(e, InputError::TooLarge { what: "drawn triangles", got: 12, max: 11 }));
        let mut nan = s.clone();
        nan.meshes[0].positions[3][1] = f32::NAN;
        assert!(matches!(decode(&encode(&nan), &lim), Err(InputError::NotFinite { .. })));
        let mut inf = s.clone();
        inf.objects[0].transform[13] = f32::INFINITY;
        // serde_json writes a non-finite float as null: a header error.
        assert_eq!(decode(&encode(&inf), &lim).unwrap_err().code(), "invalid_input");
        assert_eq!(decode(b"", &lim), Err(InputError::Truncated { need: 12, have: 0 }));
        assert_eq!(decode(b"garbage that is long enough", &lim), Err(InputError::BadMagic));
    }

    #[test]
    fn oversized_counts_are_refused_before_any_data_is_read() {
        let lim = InputLimits::default();
        // A header claiming 4G triangles with no data behind it: too large, not truncated.
        let j = r#"{"version":1,"meshes":[{"vertices":3,"triangles":4294967295,"normals":false}],"objects":[{"id":"a","mesh":0,"base_color":[1,1,1,1],"transform":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]}]}"#;
        let e = decode(&with_header(j, &[]), &lim).unwrap_err();
        assert!(matches!(e, InputError::TooLarge { what: "triangles", .. }), "{e}");
        // Instancing expanded: 9 objects of a 1M-triangle mesh is 9M drawn.
        let objs: Vec<String> = (0..9)
            .map(|i| format!(r#"{{"id":"o{i}","mesh":0,"base_color":[1,1,1,1],"transform":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]}}"#))
            .collect();
        let j = format!(r#"{{"version":1,"meshes":[{{"vertices":3,"triangles":1000000,"normals":false}}],"objects":[{}]}}"#, objs.join(","));
        let e = decode(&with_header(&j, &[]), &lim).unwrap_err();
        assert!(matches!(e, InputError::TooLarge { what: "drawn triangles", got: 9_000_000, .. }), "{e}");
        // A list past its cap stops while parsing.
        let tight = InputLimits { max_cameras: 2, ..lim };
        let cam = r#"{"position":[0,0,5],"target":[0,0,0],"up":[0,1,0],"yfov":0.8}"#;
        let j = format!(r#"{{"version":1,"meshes":[],"objects":[],"cameras":[{cam},{cam},{cam}]}}"#);
        assert_eq!(decode(&with_header(&j, &[]), &tight), Err(InputError::TooMany { what: "cameras", max: 2 }));
        let mods: Vec<String> = (0..300).map(|i| format!("\"M{i}\"")).collect();
        let j = format!(
            r#"{{"version":1,"meshes":[{{"vertices":0,"triangles":0,"normals":false}}],"objects":[{{"id":"a","mesh":0,"base_color":[1,1,1,1],"transform":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],"modifiers":[{}]}}]}}"#,
            mods.join(",")
        );
        let e = decode(&with_header(&j, &[]), &lim).unwrap_err();
        assert_eq!(e, InputError::TooMany { what: "modifiers of an object", max: MAX_MODIFIERS as u64 });
        // The header length itself.
        let mut big = MAGIC.to_vec();
        big.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(decode(&big, &lim), Err(InputError::TooLarge { what: "header bytes", .. })));
        // The input length.
        let small = InputLimits { max_input_bytes: 8, ..lim };
        assert!(matches!(decode(&encode(&testgen::cube_scene()), &small), Err(InputError::TooLarge { what: "input bytes", .. })));
    }

    #[test]
    fn hostile_headers_are_invalid_input() {
        let lim = InputLimits::default();
        for j in [
            "",
            "{",
            "[]",
            r#"{"version":2,"meshes":[],"objects":[]}"#,
            r#"{"version":1,"meshes":[],"objects":[{"id":"a","mesh":0,"base_color":[1,1,1,1],"transform":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]}]}"#,
            r#"{"version":1,"meshes":[],"objects":[],"cameras":[{"position":[0,0,5],"up":[0,1,0],"yfov":0.8}]}"#,
            r#"{"version":1,"meshes":[],"objects":[],"cameras":[{"position":[0,0,5],"target":[0,0,0],"up":[0,1,0],"yfov":4}]}"#,
            r#"{"version":1,"version":1,"meshes":[],"objects":[]}"#,
            r#"{"version":1,"meshes":[{"vertices":-1,"triangles":0,"normals":false}],"objects":[]}"#,
        ] {
            let e = decode(&with_header(j, &[]), &lim).unwrap_err();
            assert_eq!(e.code(), "invalid_input", "{j}: {e}");
        }
        // Non-zero padding.
        let mut b = encode(&testgen::cube_scene());
        let hlen = u32::from_le_bytes([b[8], b[9], b[10], b[11]]) as usize;
        if !(12 + hlen).is_multiple_of(4) {
            b[12 + hlen] = 1;
            assert_eq!(decode(&b, &lim), Err(InputError::BadPadding));
        }
        // An id past the cap.
        let mut s = testgen::cube_scene();
        s.objects[0].id = "x".repeat(MAX_ID_BYTES + 1);
        assert!(matches!(decode(&encode(&s), &lim), Err(InputError::BadField { .. })));
    }

    #[test]
    fn the_layout_points_at_each_meshs_data() {
        let s = testgen::grid_scene(3, false);
        let b = encode(&s);
        let (h, ranges) = layout(&b, &InputLimits::default()).expect("layout");
        assert_eq!(h.meshes.len(), ranges.len());
        assert_eq!(ranges.last().map(|r| r.end), Some(b.len()));
        for (m, r) in h.meshes.iter().zip(&ranges) {
            assert_eq!(r.len() as u64, data_len(m));
        }
    }
}
