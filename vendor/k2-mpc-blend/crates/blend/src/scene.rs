//! The scene model: every data-block of a file ([`IdBlock`]) with its key, kind and name; for
//! scenes and collections what they hold; for objects their data, parent, transform and
//! material slots; for cameras, lights and materials a few of their own fields. Everything
//! else a data-block holds is carried as a content digest.
//!
//! **Fields by name, from the file.** Every value is read through the file's own schema: a
//! field is looked up by the name the file gives it, and an attribute carries the field's name
//! as the file writes it (`loc`, `rot`, `quat`, `size`, `restrictflag`, `lens`, `energy`). A
//! field a file doesn't have is left out, never filled in. Flags and enumerations are carried
//! as the numbers the file holds, always.
//!
//! **Named values** ([`IdBlock::named`]), for files from [`NAMES_FROM`] to [`NAMES_TO`]: the bits
//! and values learnt by saving scenes with known states and diffing the saved fields
//! (`PROVENANCE.md`), identical in every version pinned. An object's `restrictflag` bits 1, 2 and 4
//! are `hide_viewport`, `hide_select` and `hide_render`; a collection's `flag` bits 1, 2 and 8 are
//! the same three; an object's `type` is `object_type` and its `rotmode` `rotation_mode`; a light's
//! `type` is `light_type`, a camera's `camera_type` ([`OBJECT_TYPES`] and the other tables). A
//! value or bit not in a table stays a number only (`restrictflag`'s other bits are listed as
//! `other_bits`). [`IdBlock::hidden`] is `hide_viewport`: an object or collection disabled in
//! viewports. What a view layer hides (its own per-layer states) is not read. Files older than
//! [`NAMES_FROM`] or newer than [`NAMES_TO`] keep their numbers only, and nothing is hidden: no
//! release's saves pinned their values.
//!
//! **Kinds.** A data-block's kind is its struct's name in plain words (`Object` is `object`,
//! `Lamp` and `Light` are `light`, `Group` and `Collection` are `collection`); a struct this
//! reader has no word for is its name in lower case. An object's `data_type` is its data's
//! kind, or `empty` without data. A data-block whose struct is the bare `ID` (a placeholder for
//! one linked from another file) takes the kind its code has elsewhere in the file.
//!
//! **Keys.** A data-block's name is unique among the data-blocks of its type in one file (or
//! one library), so its key is its two-letter type code and its name: `@ob:Cube`; one linked
//! from a library is scoped by the library: `@li:lib.blend/ob:Cube`. A name's whitespace,
//! control characters, `%`, `/` and any bytes that aren't UTF-8 are written `%XX`. A key over
//! the kit's length limit is left out (the kit mints one). **A key is only as stable as the
//! name:** a rename is a new key. The old key is gone from the next version; an anchor on it
//! is orphaned, with the renamed data-block offered as a candidate when it keeps a weak ID
//! (an object's data) in common. A file holds no ID that survives a rename.
//!
//! **What holds what.** A scene holds its master collection's objects and child collections
//! (or, in files from before collections, its bases' objects); a collection holds the objects
//! and collections its lists name. Lists are walked by their `next` pointers, each element read
//! through its own struct; a list that loops stops where it loops.
//!
//! **Content digests.** A data-block's content digest covers its own struct and every `DATA`
//! block after it, each with its code, struct and count, and with every pointer replaced by
//! what it names: a data-block by its type code, name and library; a block of the same
//! data-block by its place among them; anything else by its kind. So a save that moves
//! everything in memory, but changes nothing, digests the same. Left out as the save's own
//! state: the `ID` header's list links, user count, tags, session numbers, cached hashes and
//! runtime pointers, any field the schema calls runtime, and padding. A block too large to keep
//! is digested as stored; when its struct holds pointers that digest changes with every save,
//! and the data-block is marked (`content_stable` is false).
//!
//! **For later writes.** Each fixed-size field the model reads (a name, a transform, a flag)
//! keeps its place in the decompressed file ([`FieldSpan`]).

use std::collections::{BTreeSet, HashMap, HashSet};

use k2_mpc_base::index::{Digest, DigestStream, INDEX_KEY_MAX};

use crate::error::ReadError;
use crate::reader::{BlendFile, Target, DATA};
use crate::sdna::{Field, EMBED_DEPTH_MAX};

/// Most elements one list walk follows.
pub const LIST_MAX: usize = 1 << 20;
/// Most material slots read from one array.
pub const SLOTS_MAX: usize = 1 << 15;
/// The structs of data-blocks that are the program's own interface (windows, workspaces,
/// screens): they get no node.
pub const UI_STRUCTS: &[&str] = &["wmWindowManager", "WorkSpace", "bScreen"];

/// The oldest file version ([`crate::Header::version`]) whose flags and values are named: the
/// oldest release whose saves pinned them (`PROVENANCE.md`).
pub const NAMES_FROM: u16 = 306;
/// The newest file version whose flags and values are named: the newest release whose saves
/// pinned them (`PROVENANCE.md`).
pub const NAMES_TO: u16 = 501;
/// An object's `restrictflag` bits.
pub const OBJECT_HIDE_BITS: &[(i64, &str)] =
    &[(1, "hide_viewport"), (2, "hide_select"), (4, "hide_render")];
/// A collection's `flag` bits.
pub const COLLECTION_HIDE_BITS: &[(i64, &str)] =
    &[(1, "hide_viewport"), (2, "hide_select"), (8, "hide_render")];
/// An object's `type` values.
pub const OBJECT_TYPES: &[(i64, &str)] = &[
    (0, "empty"),
    (1, "mesh"),
    (2, "curve"),
    (3, "surface"),
    (4, "text"),
    (5, "metaball"),
    (10, "light"),
    (11, "camera"),
    (12, "speaker"),
    (13, "light_probe"),
    (22, "lattice"),
    (25, "armature"),
    (26, "legacy_grease_pencil"),
    (27, "curves"),
    (28, "point_cloud"),
    (29, "volume"),
    (30, "grease_pencil"),
];
/// An object's `rotmode` values.
pub const ROTATION_MODES: &[(i64, &str)] = &[
    (-1, "axis_angle"),
    (0, "quaternion"),
    (1, "xyz"),
    (2, "xzy"),
    (3, "yxz"),
    (4, "yzx"),
    (5, "zxy"),
    (6, "zyx"),
];
/// A light's `type` values.
pub const LIGHT_TYPES: &[(i64, &str)] = &[(0, "point"), (1, "sun"), (2, "spot"), (4, "area")];
/// A camera's `type` values.
pub const CAMERA_TYPES: &[(i64, &str)] =
    &[(0, "perspective"), (1, "orthographic"), (2, "panoramic")];

/// What a data-block's numbers mean, for a file of `version`: `(named values, hidden)`.
fn name_values(
    kind: &str,
    fields: &[(String, String)],
    version: u16,
) -> (Vec<(String, String)>, Option<bool>) {
    let mut named = Vec::new();
    let past = version > NAMES_TO && !crate::mutant::active("NamesPastPinned");
    if version < NAMES_FROM || past {
        return (named, None);
    }
    let num = |f: &str| -> Option<i64> {
        fields
            .iter()
            .find(|(n, _)| n == f)
            .and_then(|(_, v)| v.parse().ok())
    };
    let table = |field: &str, out: &str, t: &[(i64, &str)], named: &mut Vec<(String, String)>| {
        if let Some(v) = num(field) {
            if let Some((_, name)) = t.iter().find(|(k, _)| *k == v) {
                named.push((out.to_owned(), (*name).to_owned()));
            }
        }
    };
    let bits =
        |field: &str, t: &[(i64, &str)], named: &mut Vec<(String, String)>| -> Option<bool> {
            let v = num(field)?;
            let mut known = 0;
            for (bit, name) in t {
                named.push(((*name).to_owned(), (v & bit != 0).to_string()));
                known |= bit;
            }
            if v & !known != 0 {
                named.push(("other_bits".to_owned(), (v & !known).to_string()));
            }
            Some(v & t[0].0 != 0)
        };
    let hidden = match kind {
        "object" => {
            table("type", "object_type", OBJECT_TYPES, &mut named);
            table("rotmode", "rotation_mode", ROTATION_MODES, &mut named);
            bits("restrictflag", OBJECT_HIDE_BITS, &mut named)
        }
        "collection" => bits("flag", COLLECTION_HIDE_BITS, &mut named),
        "light" => {
            table("type", "light_type", LIGHT_TYPES, &mut named);
            None
        }
        "camera" => {
            table("type", "camera_type", CAMERA_TYPES, &mut named);
            None
        }
        _ => None,
    };
    (named, hidden)
}

/// Where a fixed-size field is in the decompressed file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FieldSpan {
    /// The field's name as the file writes it (`loc`, `name`).
    pub field: String,
    /// Its type's name (`float`, `char`).
    pub type_name: String,
    /// From the start of the decompressed file.
    pub offset: u64,
    /// In bytes.
    pub len: u32,
}

/// One data-block of the file.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct IdBlock {
    /// Its block's index in [`BlendFile::blocks`].
    pub block: u32,
    /// The two-letter type code (`OB`).
    pub code: String,
    /// Its struct's name (`Object`).
    pub struct_name: String,
    /// The plain word for it ([`kind_of`]).
    pub kind: String,
    /// Its name without the type code, as text (bytes that aren't UTF-8 replaced).
    pub name: String,
    /// The name's bytes, as stored.
    pub name_bytes: Vec<u8>,
    /// The library it is linked from: an index into [`SceneModel::ids`].
    pub library: Option<usize>,
    /// `@ob:Cube`; `None` when over the kit's length limit.
    pub key: Option<String>,
    /// One of [`UI_STRUCTS`]: no node.
    pub ui: bool,
    /// What the model reads for its kind.
    pub detail: Detail,
    /// Its fields, as `(name, value)`, named as the file writes them.
    pub fields: Vec<(String, String)>,
    /// What some of those fields' numbers mean, as `(name, value)` (see the module): empty for
    /// a file outside [`NAMES_FROM`] to [`NAMES_TO`].
    pub named: Vec<(String, String)>,
    /// Disabled in viewports (an object's or a collection's own flag); `None` when the file is
    /// outside [`NAMES_FROM`] to [`NAMES_TO`] or the field is missing.
    pub hidden: Option<bool>,
    /// Material slots (an object's own, or its data's), as indexes into
    /// [`SceneModel::ids`]; `None` for an empty slot or one that names nothing.
    pub materials: Vec<Option<usize>>,
    /// The content digest (see the module).
    pub content: Digest,
    /// `false` when a block too large to keep held pointers.
    pub content_stable: bool,
    /// Where the fixed-size fields read are: the name, a transform, flags.
    pub spans: Vec<FieldSpan>,
    /// The pointers read for this data-block (its library, data, parent, material slots, the
    /// lists it holds) that named nothing the model could tell apart: its share of
    /// [`SceneModel::unresolved`].
    pub unresolved: u32,
}

/// What a data-block holds, for the kinds the model reads.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Detail {
    /// A scene.
    Scene {
        /// The master collection's objects (or the bases' objects), in list order.
        objects: Vec<usize>,
        /// The master collection's child collections.
        collections: Vec<usize>,
        /// Its camera object.
        camera: Option<usize>,
        /// Its world.
        world: Option<usize>,
    },
    /// A collection.
    Collection {
        /// Its objects, in list order.
        objects: Vec<usize>,
        /// Its child collections.
        collections: Vec<usize>,
    },
    /// An object.
    Object {
        /// Its data (a mesh, camera, light...).
        data: Option<usize>,
        /// Its parent object.
        parent: Option<usize>,
    },
    /// A library.
    Library {
        /// The library file's name, without its folders.
        file: Option<String>,
    },
    /// Anything else.
    Other,
}

/// A file's data-blocks.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SceneModel {
    /// Every data-block, in file order.
    pub ids: Vec<IdBlock>,
    /// Pointers the model followed that named nothing it could tell apart (ambiguous) or no
    /// block (unresolved): counted, never guessed.
    pub unresolved: u32,
}

impl SceneModel {
    /// The data-block with this key.
    pub fn by_key(&self, key: &str) -> Option<&IdBlock> {
        self.ids.iter().find(|i| i.key.as_deref() == Some(key))
    }

    /// Every camera of the file ([`SceneCamera`]): each object whose data is a camera, in file
    /// order, the order `scene list` lists the objects in.
    pub fn cameras(&self) -> Vec<SceneCamera> {
        let key = |i: usize| self.ids[i].key.clone().unwrap_or_else(|| format!("#{i}"));
        let mut out: Vec<SceneCamera> = Vec::new();
        for (i, id) in self.ids.iter().enumerate() {
            let Detail::Object { data: Some(d), .. } = &id.detail else {
                continue;
            };
            let (d, cam) = (*d, &self.ids[*d]);
            if cam.kind != "camera" {
                continue;
            }
            let active_in = self
                .ids
                .iter()
                .enumerate()
                .filter(
                    |(_, s)| matches!(&s.detail, Detail::Scene { camera: Some(c), .. } if *c == i),
                )
                .map(|(s, _)| key(s))
                .collect();
            out.push(SceneCamera {
                object: key(i),
                name: if crate::mutant::active("CameraNameDropped") {
                    cam.name.clone()
                } else {
                    id.name.clone()
                },
                data: key(d),
                active_in,
                fields: cam.fields.iter().chain(&cam.named).cloned().collect(),
                linked: id.library.is_some(),
            });
        }
        if crate::mutant::active("CameraNamesSwapped") && out.len() > 1 {
            let (first, rest) = out.split_at_mut(1);
            if let Some(last) = rest.last_mut() {
                std::mem::swap(&mut first[0].name, &mut last.name);
            }
        }
        out
    }
}

/// One camera, as the scene read lists it for a host: an object whose data is a camera, with
/// the object's name (what a person calls the camera) and the camera fields the model reads.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SceneCamera {
    /// The object's key (`@ob:Name`), or `#N` (its place in [`SceneModel::ids`]) when it has
    /// none.
    pub object: String,
    /// The object's name as the file stores it, as text (bytes that aren't UTF-8 replaced).
    pub name: String,
    /// Its camera data's key (`@ca:Name`), or `#N`.
    pub data: String,
    /// The scenes whose active camera it is, by key, in file order.
    pub active_in: Vec<String>,
    /// The camera data's fields and named values, as [`IdBlock::fields`] and
    /// [`IdBlock::named`] hold them (`type`, `lens`, `ortho_scale`, `clipsta`, `clipend`,
    /// `sensor_x`, `sensor_y`, `sensor_fit`, `camera_type`), each only when the file has it.
    pub fields: Vec<(String, String)>,
    /// The object is linked from a library.
    pub linked: bool,
}

/// The plain word for a data-block struct.
pub fn kind_of(struct_name: &str) -> String {
    let k = match struct_name {
        "Scene" => "scene",
        "Collection" | "Group" => "collection",
        "Object" => "object",
        "Material" => "material",
        "Camera" => "camera",
        "Light" | "Lamp" => "light",
        "Library" => "library",
        "Mesh" => "mesh",
        "World" => "world",
        "bNodeTree" => "node_tree",
        "bAction" => "action",
        "Image" => "image",
        "Text" => "text",
        "Curve" => "curve",
        "Curves" => "curves",
        "bArmature" => "armature",
        "Key" => "shape_key",
        "Tex" => "texture",
        "Brush" => "brush",
        "Palette" => "palette",
        "FreestyleLineStyle" => "line_style",
        "ParticleSettings" => "particles",
        "MovieClip" => "movie_clip",
        "VFont" => "font",
        "bSound" => "sound",
        "Lattice" => "lattice",
        "MetaBall" => "metaball",
        "LightProbe" => "light_probe",
        "PointCloud" => "point_cloud",
        "Volume" => "volume",
        "GreasePencil" | "bGPdata" => "grease_pencil",
        "CacheFile" => "cache_file",
        "PaintCurve" => "paint_curve",
        "Speaker" => "speaker",
        "Mask" => "mask",
        other => {
            let mut s: String = other
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '_'
                    }
                })
                .collect();
            s.truncate(48);
            let t = s.trim_matches('_');
            return if t.is_empty() { "id".into() } else { t.into() };
        }
    };
    k.to_owned()
}

/// `name` as a key's ID part: whitespace, control characters, `%`, `/` and bytes that aren't
/// UTF-8 as `%XX`.
pub fn escape_name(name: &[u8]) -> String {
    let mut out = String::with_capacity(name.len());
    let pct = |out: &mut String, bytes: &[u8]| {
        for b in bytes {
            out.push_str(&format!("%{b:02X}"));
        }
    };
    for chunk in name.utf8_chunks() {
        for c in chunk.valid().chars() {
            if c.is_whitespace() || c.is_control() || c == '%' || c == '/' {
                let mut buf = [0u8; 4];
                pct(&mut out, c.encode_utf8(&mut buf).as_bytes());
            } else {
                out.push(c);
            }
        }
        pct(&mut out, chunk.invalid());
    }
    out
}

/// The key of a data-block with type code `code` and name `name`, in library `lib` (its own
/// escaped name). `None` when over the kit's limit, or the name or the library's name is empty.
pub fn key_of(code: &str, name: &[u8], lib: Option<&str>) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let seg = format!("{}:{}", code.to_ascii_lowercase(), escape_name(name));
    let key = match lib {
        Some("") => return None,
        Some(l) => format!("@li:{l}/{seg}"),
        None => format!("@{seg}"),
    };
    (key.len() <= INDEX_KEY_MAX).then_some(key)
}

/// A pointer field's place: its offset in a body and how deep it points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotKind {
    Ptr { depth: u8 },
    Func,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Slot {
    offset: u32,
    len: u32,
    kind: SlotKind,
}

/// The fields of the `ID` header kept in a content digest; the rest is the save's own state.
const ID_KEPT: &[&str] = &[
    "name",
    "lib",
    "flag",
    "properties",
    "system_properties",
    "override_library",
    "asset_data",
];

/// Where each pointer and each skipped field of a struct is, embedded structs flattened.
struct Layouts<'a> {
    file: &'a BlendFile,
    cache: HashMap<u32, Vec<Slot>>,
}

impl<'a> Layouts<'a> {
    fn new(file: &'a BlendFile) -> Self {
        Self {
            file,
            cache: HashMap::new(),
        }
    }

    fn of(&mut self, s: u32) -> &[Slot] {
        if !self.cache.contains_key(&s) {
            let mut out = Vec::new();
            self.walk(s, 0, 0, &mut out);
            out.sort_by_key(|x| x.offset);
            self.cache.insert(s, out);
        }
        &self.cache[&s]
    }

    fn walk(&self, s: u32, base: u32, depth: usize, out: &mut Vec<Slot>) {
        let sdna = &self.file.sdna;
        let Some(def) = sdna.struct_def(s) else {
            return;
        };
        let is_id = sdna.struct_name(s) == Some("ID");
        let ps = u32::from(sdna.pointer_size());
        for f in &def.fields {
            let at = base + f.offset;
            if skipped(f, sdna.type_name(f.type_index))
                || (is_id && !ID_KEPT.contains(&f.name.as_str()))
            {
                out.push(Slot {
                    offset: at,
                    len: f.size,
                    kind: SlotKind::Skip,
                });
                continue;
            }
            if f.is_pointer() {
                for i in 0..f.array_len {
                    out.push(Slot {
                        offset: at + i * ps,
                        len: ps,
                        kind: if f.function_pointer {
                            SlotKind::Func
                        } else {
                            SlotKind::Ptr {
                                depth: f.pointer_depth,
                            }
                        },
                    });
                }
                continue;
            }
            if let Some(child) = sdna.struct_of_type(f.type_index) {
                if depth < EMBED_DEPTH_MAX {
                    let size = sdna.struct_def(child).map_or(0, |d| d.size);
                    for i in 0..f.array_len {
                        self.walk(child, at + i * size, depth + 1, out);
                    }
                }
            }
        }
    }
}

/// A field left out of content digests: runtime state, or padding.
fn skipped(f: &Field, type_name: &str) -> bool {
    f.name.starts_with("_pad")
        || f.name.to_ascii_lowercase().contains("runtime")
        || type_name.contains("Runtime")
}

/// Reads a file's data-blocks into the model.
pub fn read_scene(file: &BlendFile) -> Result<SceneModel, ReadError> {
    Model::new(file).run()
}

struct Model<'a> {
    file: &'a BlendFile,
    /// block index -> id index
    of_block: HashMap<u32, usize>,
    ids: Vec<IdBlock>,
    unresolved: u32,
}

/// Where an ID's name and library pointer are in struct `s`.
struct IdPaths {
    name_at: u32,
    name_len: u32,
    lib_at: u32,
}

/// The paths to an ID's name and library pointer in struct `s`: `id.name` and `id.lib`, or
/// `name` and `lib` when the struct is the bare `ID`.
fn id_paths(file: &BlendFile, s: u32) -> Option<IdPaths> {
    let sdna = &file.sdna;
    let bare = sdna.struct_name(s) == Some("ID");
    let (n, l) = if bare {
        ("name", "lib")
    } else {
        // the first field must be the embedded ID
        let first = sdna.struct_def(s)?.fields.first()?;
        if first.name != "id" || sdna.type_name(first.type_index) != "ID" || first.is_pointer() {
            return None;
        }
        ("id.name", "id.lib")
    };
    let (name_at, nf) = sdna.field_path(s, n)?;
    let (lib_at, lf) = sdna.field_path(s, l)?;
    if nf.is_pointer() || !lf.is_pointer() || lf.pointer_depth != 1 {
        return None;
    }
    Some(IdPaths {
        name_at,
        name_len: nf.size,
        lib_at,
    })
}

fn cstr(b: &[u8]) -> &[u8] {
    &b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]
}

/// Numbers of a field as text: `1,2.5,3`. `None` for a type this reader doesn't read as
/// numbers, or a field over 16 elements.
fn number_text(file: &BlendFile, body: &[u8], at: usize, f: &Field) -> Option<String> {
    let e = file.endian();
    let t = file.sdna.type_name(f.type_index);
    let elem: usize = match t {
        "float" | "int" | "int32_t" | "uint" | "uint32_t" => 4,
        "double" | "int64_t" | "uint64_t" => 8,
        "short" | "ushort" | "int16_t" | "uint16_t" => 2,
        "char" | "uchar" | "int8_t" | "uint8_t" | "bool" => 1,
        _ => return None,
    };
    let n = f.array_len as usize;
    if f.is_pointer() || n == 0 || n > 16 || elem * n != f.size as usize {
        return None;
    }
    let mut parts = Vec::with_capacity(n);
    for i in 0..n {
        let p = at + i * elem;
        let s = match t {
            "float" => e.f32(body, p)?.to_string(),
            "double" => e.f64(body, p)?.to_string(),
            "int" | "int32_t" => e.i32(body, p)?.to_string(),
            "uint" | "uint32_t" => e.u32(body, p)?.to_string(),
            "int64_t" => e.i64(body, p)?.to_string(),
            "uint64_t" => e.u64(body, p)?.to_string(),
            "short" | "int16_t" => e.i16(body, p)?.to_string(),
            "ushort" | "uint16_t" => e.u16(body, p)?.to_string(),
            "int8_t" => (*body.get(p)? as i8).to_string(),
            _ => body.get(p)?.to_string(),
        };
        parts.push(s);
    }
    Some(parts.join(","))
}

/// The fields each kind reads, by the names files give them; the first name a file has wins.
fn fields_for(kind: &str) -> &'static [&'static [&'static str]] {
    match kind {
        "object" => &[
            &["type"],
            &["partype"],
            &["loc"],
            &["rot"],
            &["quat"],
            &["rotmode"],
            &["scale", "size"],
            &["restrictflag"],
            &["visibility_flag"],
        ],
        "camera" => &[
            &["type"],
            &["lens"],
            &["ortho_scale"],
            &["clipsta"],
            &["clipend"],
            &["sensor_x"],
            &["sensor_y"],
            &["sensor_fit"],
        ],
        "light" => &[
            &["type"],
            &["r"],
            &["g"],
            &["b"],
            &["energy"],
            &["energy_new"],
        ],
        "material" => &[&["r"], &["g"], &["b"]],
        "collection" => &[&["flag"], &["color_tag"]],
        _ => &[],
    }
}

impl<'a> Model<'a> {
    fn new(file: &'a BlendFile) -> Self {
        Self {
            file,
            of_block: HashMap::new(),
            ids: Vec::new(),
            unresolved: 0,
        }
    }

    fn run(mut self) -> Result<SceneModel, ReadError> {
        let file = self.file;
        // the codes' structs, for placeholders of linked data-blocks
        let mut code_struct: HashMap<[u8; 2], String> = HashMap::new();
        for b in &file.blocks {
            if b.is_id() {
                if let Some(n) = file.sdna.struct_name(b.head.sdna) {
                    if n != "ID" {
                        code_struct
                            .entry([b.head.code[0], b.head.code[1]])
                            .or_insert_with(|| n.to_owned());
                    }
                }
            }
        }
        // pass 1: every data-block, its name and library pointer
        let mut libs: Vec<Option<u64>> = Vec::new();
        for (i, b) in file.blocks.iter().enumerate() {
            if !b.is_id() {
                continue;
            }
            let i = i as u32;
            let s = b.head.sdna;
            let struct_name = file
                .sdna
                .struct_name(s)
                .ok_or(ReadError::malformed(
                    "a data-block's struct is out of range",
                ))?
                .to_owned();
            let body = file
                .body(i)
                .ok_or(ReadError::malformed("a data-block's struct wasn't kept"))?;
            let def_size = file.sdna.struct_def(s).map_or(0, |d| d.size) as u64;
            if (body.len() as u64) < def_size {
                return Err(ReadError::malformed(
                    "a data-block is shorter than its struct",
                ));
            }
            let Some(IdPaths {
                name_at,
                name_len,
                lib_at,
            }) = id_paths(file, s)
            else {
                return Err(ReadError::malformed(
                    "a data-block's struct has no ID header",
                ));
            };
            let raw_name = cstr(&body[name_at as usize..(name_at + name_len) as usize]);
            let code = String::from_utf8_lossy(&b.head.code[..2]).into_owned();
            // the stored name starts with the type code
            let name_bytes = raw_name.get(2..).unwrap_or(&[]).to_vec();
            let kind = if struct_name == "ID" {
                code_struct
                    .get(&[b.head.code[0], b.head.code[1]])
                    .map_or_else(|| "id".to_owned(), |n| kind_of(n.as_str()))
            } else {
                kind_of(&struct_name)
            };
            let lib_ptr = file.endian().ptr(body, lib_at as usize);
            libs.push(lib_ptr);
            let ui = UI_STRUCTS.contains(&struct_name.as_str());
            self.of_block.insert(i, self.ids.len());
            self.ids.push(IdBlock {
                block: i,
                code,
                struct_name,
                kind,
                name: String::from_utf8_lossy(&name_bytes).into_owned(),
                name_bytes,
                library: None,
                key: None,
                ui,
                detail: Detail::Other,
                fields: Vec::new(),
                named: Vec::new(),
                hidden: None,
                materials: Vec::new(),
                content: Digest::of(&[]),
                content_stable: true,
                spans: vec![FieldSpan {
                    field: "name".into(),
                    type_name: "char".into(),
                    offset: b.head.body_offset + u64::from(name_at),
                    len: name_len,
                }],
                unresolved: 0,
            });
        }
        // libraries and keys
        for (n, lib) in libs.iter().enumerate() {
            let block = self.ids[n].block;
            // a library pointer must name a library (not itself): anything else is counted
            // with the pointers that name nothing
            let before = self.unresolved;
            self.ids[n].library = match lib.and_then(|p| self.id_at(p, block)) {
                Some(l) if l != n && self.ids[l].kind == "library" => Some(l),
                Some(_) => {
                    self.unresolved += 1;
                    None
                }
                None => None,
            };
            self.ids[n].unresolved += self.unresolved - before;
        }
        let keys: Vec<Option<String>> = self
            .ids
            .iter()
            .map(|id| {
                let lib = id.library.map(|l| escape_name(&self.ids[l].name_bytes));
                key_of(&id.code, &id.name_bytes, lib.as_deref())
            })
            .collect();
        for (id, key) in self.ids.iter_mut().zip(keys) {
            id.key = key;
        }
        // pass 2: what each kind holds
        for n in 0..self.ids.len() {
            let before = self.unresolved;
            self.detail(n);
            self.ids[n].unresolved += self.unresolved - before;
        }
        // pass 3: content digests
        let mut layouts = Layouts::new(file);
        let digests: Vec<(Digest, bool)> = (0..self.ids.len())
            .map(|n| self.content(n, &mut layouts))
            .collect();
        for (id, (d, stable)) in self.ids.iter_mut().zip(digests) {
            id.content = d;
            id.content_stable = stable;
        }
        Ok(SceneModel {
            ids: self.ids,
            unresolved: self.unresolved,
        })
    }

    /// The data-block a pointer names, read from block `from`.
    fn id_at(&mut self, ptr: u64, from: u32) -> Option<usize> {
        match self.file.resolve(ptr, from) {
            Target::Null => None,
            Target::Block(b) => self.of_block.get(&b).copied(),
            Target::Unresolved | Target::Ambiguous => {
                self.unresolved += 1;
                None
            }
        }
    }

    /// A pointer field `name` of block `block` (struct `s`), at `base` in its body.
    fn ptr_field(&self, block: u32, base: usize, s: u32, name: &str) -> Option<u64> {
        let f = self.file.sdna.field(s, name)?;
        if f.pointer_depth == 0 || f.function_pointer {
            return None;
        }
        let body = self.file.body(block)?;
        self.file.endian().ptr(body, base + f.offset as usize)
    }

    fn id_field(&mut self, block: u32, s: u32, name: &str) -> Option<usize> {
        let p = self.ptr_field(block, 0, s, name)?;
        self.id_at(p, block)
    }

    /// The data-blocks a list field (`gobject`, `children`, `base`) of block `block` names: each
    /// element's pointer to a data-block (`ob`, `object`, `collection`, else its first pointer
    /// that names one).
    fn list(&mut self, block: u32, s: u32, name: &str) -> Vec<usize> {
        let file = self.file;
        let Some((at, f)) = file.sdna.field_path(s, &format!("{name}.first")) else {
            return Vec::new();
        };
        if f.pointer_depth != 1 {
            return Vec::new();
        }
        let Some(first) = file
            .body(block)
            .and_then(|b| file.endian().ptr(b, at as usize))
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut ptr = first;
        let mut from = block;
        while out.len() < LIST_MAX {
            let t = match file.resolve(ptr, from) {
                Target::Null => break,
                Target::Block(t) => t,
                _ => {
                    self.unresolved += 1;
                    break;
                }
            };
            if !seen.insert(t) {
                break;
            }
            let es = file.blocks[t as usize].head.sdna;
            if es == 0 || file.body(t).is_none() {
                break;
            }
            let target = ["ob", "object", "collection"]
                .iter()
                .find_map(|n| self.ptr_field(t, 0, es, n))
                .or_else(|| self.first_id_ptr(t, es));
            if let Some(p) = target {
                if let Some(id) = self.id_at(p, t) {
                    out.push(id);
                }
            }
            match self.ptr_field(t, 0, es, "next") {
                Some(n) => {
                    ptr = n;
                    from = t;
                }
                None => break,
            }
        }
        out
    }

    /// An element's first single pointer (not `next` or `prev`) that names a data-block.
    fn first_id_ptr(&self, block: u32, s: u32) -> Option<u64> {
        let file = self.file;
        let body = file.body(block)?;
        let def = file.sdna.struct_def(s)?;
        def.fields
            .iter()
            .filter(|f| f.pointer_depth == 1 && !f.function_pointer && f.name != "next" && f.name != "prev")
            .filter_map(|f| file.endian().ptr(body, f.offset as usize))
            .find(|&p| matches!(file.resolve(p, block), Target::Block(b) if file.blocks[b as usize].is_id()))
    }

    /// The material slots of an array field `mat` (pointers to pointers) with `totcol` slots.
    fn slots(&mut self, block: u32, s: u32) -> Vec<Option<usize>> {
        let file = self.file;
        let Some(f) = file.sdna.field(s, "mat") else {
            return Vec::new();
        };
        if f.pointer_depth != 2 || f.function_pointer {
            return Vec::new();
        }
        let Some(body) = file.body(block) else {
            return Vec::new();
        };
        let e = file.endian();
        let Some(arr) = e.ptr(body, f.offset as usize) else {
            return Vec::new();
        };
        let n = file
            .sdna
            .field(s, "totcol")
            .and_then(|t| number_text(file, body, t.offset as usize, t))
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or(0)
            .clamp(0, SLOTS_MAX as i64) as usize;
        if n == 0 {
            return Vec::new();
        }
        let t = match file.resolve(arr, block) {
            Target::Block(t) => t,
            Target::Null => return vec![None; n],
            _ => {
                self.unresolved += 1;
                return vec![None; n];
            }
        };
        let Some(ab) = file.body(t) else {
            return vec![None; n];
        };
        let ps = usize::from(e.pointer_size);
        (0..n)
            .map(|i| e.ptr(ab, i * ps).and_then(|p| self.id_at(p, t)))
            .collect()
    }

    fn detail(&mut self, n: usize) {
        let file = self.file;
        let block = self.ids[n].block;
        let s = file.blocks[block as usize].head.sdna;
        let kind = self.ids[n].kind.clone();
        let Some(body) = file.body(block) else {
            return;
        };
        // fields, with their places
        let mut fields = Vec::new();
        let mut spans = Vec::new();
        for names in fields_for(&kind) {
            let Some(f) = file.sdna.field_any(s, names) else {
                continue;
            };
            if let Some(v) = number_text(file, body, f.offset as usize, f) {
                fields.push((f.name.clone(), v));
                spans.push(FieldSpan {
                    field: f.name.clone(),
                    type_name: file.sdna.type_name(f.type_index).to_owned(),
                    offset: file.blocks[block as usize].head.body_offset + u64::from(f.offset),
                    len: f.size,
                });
            }
        }
        let (named, hidden) = name_values(&kind, &fields, file.header.version);
        self.ids[n].fields = fields;
        self.ids[n].named = named;
        self.ids[n].hidden = hidden;
        self.ids[n].spans.extend(spans);
        if self.ids[n].struct_name != "ID" {
            self.ids[n].materials = self.slots(block, s);
        }
        let detail = match kind.as_str() {
            "scene" => {
                let master = self.ptr_field(block, 0, s, "master_collection");
                let (objects, collections) = match master.map(|p| file.resolve(p, block)) {
                    Some(Target::Block(m)) if file.body(m).is_some() => {
                        let ms = file.blocks[m as usize].head.sdna;
                        (self.list(m, ms, "gobject"), self.list(m, ms, "children"))
                    }
                    Some(Target::Null) | None => (self.list(block, s, "base"), Vec::new()),
                    Some(_) => {
                        self.unresolved += 1;
                        (Vec::new(), Vec::new())
                    }
                };
                Detail::Scene {
                    objects,
                    collections,
                    camera: self.id_field(block, s, "camera"),
                    world: self.id_field(block, s, "world"),
                }
            }
            "collection" if self.ids[n].struct_name != "ID" => Detail::Collection {
                objects: self.list(block, s, "gobject"),
                collections: self.list(block, s, "children"),
            },
            "object" if self.ids[n].struct_name != "ID" => Detail::Object {
                data: self.id_field(block, s, "data"),
                parent: self.id_field(block, s, "parent"),
            },
            "library" if self.ids[n].struct_name != "ID" => {
                let file_name = ["filepath", "name"].iter().find_map(|f| {
                    let fd = file.sdna.field(s, f)?;
                    if fd.is_pointer() || file.sdna.type_name(fd.type_index) != "char" {
                        return None;
                    }
                    let at = fd.offset as usize;
                    let raw = cstr(body.get(at..at + fd.size as usize)?);
                    let path = String::from_utf8_lossy(raw);
                    let base = path.rsplit(['/', '\\']).next().unwrap_or("").to_owned();
                    (!base.is_empty()).then_some(base)
                });
                Detail::Library { file: file_name }
            }
            _ => Detail::Other,
        };
        self.ids[n].detail = detail;
    }

    /// The token a pointer is replaced by in a content digest.
    fn token(&self, d: &mut DigestStream, owner: u32, ptr: u64, from: u32) {
        let file = self.file;
        match file.resolve(ptr, from) {
            Target::Null => d.update(b"N"),
            Target::Unresolved => d.update(b"U"),
            Target::Ambiguous => d.update(b"A"),
            Target::Block(t) => {
                if let Some(&id) = self.of_block.get(&t) {
                    d.update(b"I");
                    self.identity(d, id);
                } else {
                    match file.blocks[t as usize].owner {
                        Some(o) if o == owner => {
                            d.update(b"L");
                            d.update(&u64::from(t - o).to_le_bytes());
                        }
                        Some(o) => {
                            d.update(b"E");
                            if let Some(&id) = self.of_block.get(&o) {
                                self.identity(d, id);
                            }
                            d.update(&u64::from(t.saturating_sub(o)).to_le_bytes());
                        }
                        None => {
                            d.update(b"G");
                            d.update(&file.blocks[t as usize].head.code);
                        }
                    }
                }
            }
        }
    }

    /// A data-block's identity in a digest: its code, name and library's name.
    fn identity(&self, d: &mut DigestStream, id: usize) {
        let i = &self.ids[id];
        d.update(i.code.as_bytes());
        d.update(&(i.name_bytes.len() as u64).to_le_bytes());
        d.update(&i.name_bytes);
        match i.library {
            Some(l) => {
                let n = &self.ids[l].name_bytes;
                d.update(&(n.len() as u64).to_le_bytes());
                d.update(n);
            }
            None => d.update(&u64::MAX.to_le_bytes()),
        }
    }

    /// The content digest of data-block `n` and whether it is stable across saves.
    fn content(&self, n: usize, layouts: &mut Layouts<'_>) -> (Digest, bool) {
        let file = self.file;
        let first = self.ids[n].block;
        let mut members: Vec<u32> = vec![first];
        let mut j = first as usize + 1;
        while j < file.blocks.len()
            && file.blocks[j].head.code == DATA
            && file.blocks[j].owner == Some(first)
        {
            members.push(j as u32);
            j += 1;
        }
        let e = file.endian();
        let ps = usize::from(e.pointer_size);
        // raw blocks a pointer-to-pointers field names: arrays of pointers
        let mut arrays: BTreeSet<u32> = BTreeSet::new();
        for &m in &members {
            let s = file.blocks[m as usize].head.sdna;
            if m != first && s == 0 {
                continue;
            }
            let Some(body) = file.body(m) else { continue };
            let size = file.sdna.struct_def(s).map_or(0, |d| d.size as usize);
            if size == 0 {
                continue;
            }
            for slot in layouts.of(s).to_vec() {
                if let SlotKind::Ptr { depth } = slot.kind {
                    if depth < 2 {
                        continue;
                    }
                    for k in 0..body.len() / size {
                        if let Some(p) = e.ptr(body, k * size + slot.offset as usize) {
                            if let Target::Block(t) = file.resolve(p, m) {
                                if file.blocks[t as usize].head.sdna == 0
                                    && file.blocks[t as usize].owner == Some(first)
                                {
                                    arrays.insert(t);
                                }
                            }
                        }
                    }
                }
            }
        }
        let mut d = Digest::stream();
        d.update(b"k2-mpc-blend/content/v1");
        let mut stable = true;
        for &m in &members {
            let b = &file.blocks[m as usize];
            let s = b.head.sdna;
            let typed = m == first || s != 0;
            d.update(&b.head.code);
            d.update(
                (if typed {
                    file.sdna.struct_name(s).unwrap_or("?")
                } else {
                    "raw"
                })
                .as_bytes(),
            );
            d.update(&[0]);
            d.update(&b.head.count.to_le_bytes());
            d.update(&b.head.len.to_le_bytes());
            let Some(body) = file.body(m) else {
                if let Some(x) = file.body_digest(m) {
                    d.update(x.as_bytes());
                }
                if typed && !layouts.of(s).iter().all(|x| x.kind == SlotKind::Skip) {
                    stable = false;
                }
                continue;
            };
            let size = file.sdna.struct_def(s).map_or(0, |x| x.size as usize);
            if typed && size > 0 && body.len().is_multiple_of(size) {
                let slots = layouts.of(s).to_vec();
                for k in 0..body.len() / size {
                    let base = k * size;
                    let mut at = 0usize;
                    for slot in &slots {
                        let o = slot.offset as usize;
                        if o < at {
                            continue;
                        }
                        d.update(&body[base + at..base + o]);
                        match slot.kind {
                            SlotKind::Skip => d.update(b"S"),
                            SlotKind::Func => d.update(b"F"),
                            SlotKind::Ptr { .. } => {
                                let p = e.ptr(body, base + o).unwrap_or(0);
                                self.token(&mut d, first, p, m);
                            }
                        }
                        at = o + slot.len as usize;
                    }
                    d.update(&body[base + at.min(size)..base + size]);
                }
            } else if arrays.contains(&m) {
                for k in 0..body.len() / ps {
                    let p = e.ptr(body, k * ps).unwrap_or(0);
                    self.token(&mut d, first, p, m);
                }
                d.update(&body[body.len() / ps * ps..]);
            } else {
                d.update(body);
            }
        }
        (d.finish(), stable)
    }
}
