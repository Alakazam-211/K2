//! The writer: fixed-size fields of an object, patched in place. An object's location
//! (`loc`), rotation (`rot`, Euler), scale (`size`, or `scale` where a file names it so), the
//! three visibility bits of its `restrictflag`, and its name (inside the name field's fixed
//! length). Nothing else is written, no block changes size, and no pointer moves.
//!
//! **Every other byte stays.** The file is decompressed whole, the fields' bytes are replaced
//! where the file's own schema puts them (the scene model's [`FieldSpan`]s), and the result is
//! read again before anything is returned: the same blocks at the same places, the new values
//! read back, and no byte changed outside the patched fields. A compressed file is written back
//! compressed the same way, whole (gzip through `flate2`, zstd through `ruzstd`'s encoder), and
//! decoded once more to check the encoder: its own bytes differ, its content is the patched
//! file's.
//!
//! **Addresses** are the input file's keys: every edit in a list names its object as the file
//! held it, a rename earlier in the list included. The last change to a field wins.
//!
//! **Refused, typed** ([`WriteError`]): a file the reader refuses; a version outside
//! [`WRITE_FROM`]`..=`[`WRITE_TO`] (the releases whose saves pinned the bits, `PROVENANCE.md`);
//! a key that names no object, or several; an object linked from a library (its data belongs to
//! another file); a field the file's schema doesn't have, or has with another type or size; an
//! Euler rotation for an object that rotates by quaternion or axis and angle; a name that is
//! empty, holds a NUL, doesn't fit, or another object of the file has; a value that isn't
//! finite. Nothing is written in part: an edit list is applied whole or refused.
//!
//! **A rename** writes the name field only (the rest of the field zeroed). Other files that link
//! the object by name don't follow it.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use crate::compress::{self, Compression};
use crate::error::{ReadError, ReadLimits};
use crate::reader::{read_blend, BlendFile, Endian};
use crate::scene::{key_of, read_scene, FieldSpan, IdBlock, SceneModel, NAMES_FROM, NAMES_TO};

/// The oldest file version the writer writes.
pub const WRITE_FROM: u16 = NAMES_FROM;
/// The newest file version the writer writes.
pub const WRITE_TO: u16 = NAMES_TO;
/// The most bytes of the file one zstd frame the writer makes holds.
pub const ZSTD_FRAME: usize = 8 << 20;

/// One change to one object.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Change {
    /// `loc`.
    Location([f32; 3]),
    /// `rot`, Euler angles in radians, in the object's own rotation order.
    Rotation([f32; 3]),
    /// `size` (or `scale`).
    Scale([f32; 3]),
    /// The `restrictflag` bits: `None` leaves a bit as it is.
    Visibility {
        /// Disabled in viewports (bit 1).
        viewport: Option<bool>,
        /// Not selectable (bit 2).
        select: Option<bool>,
        /// Disabled in renders (bit 4).
        render: Option<bool>,
    },
    /// The name, without the type code.
    Name(String),
}

/// A change to the object with this key (`@ob:Cube`) in the input file.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Edit {
    /// The object's key in the input file's scene model.
    pub object: String,
    /// What changes.
    pub change: Change,
}

impl Edit {
    /// `change` to `object`.
    pub fn new(object: impl Into<String>, change: Change) -> Self {
        Self {
            object: object.into(),
            change,
        }
    }
}

/// Why nothing was written.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WriteError {
    /// The reader refuses the file.
    #[error("the file can't be read: {0}")]
    Read(#[from] ReadError),
    /// A version this writer doesn't write.
    #[error("version {version} is outside the versions this writer writes")]
    Version {
        /// The file's version (`405` for 4.05).
        version: u16,
    },
    /// No data-block has the key.
    #[error("nothing is {0}")]
    NotFound(String),
    /// Several data-blocks have the key (the file names two alike).
    #[error("several data-blocks are {0}")]
    Ambiguous(String),
    /// The key names a data-block that isn't an object.
    #[error("{0} isn't an object")]
    NotAnObject(String),
    /// The object is linked from a library: its data belongs to another file.
    #[error("{0} is linked from a library")]
    Linked(String),
    /// The object's struct has no such field, or has it with another type or size.
    #[error("the file's schema has no field {field} of the expected type and size")]
    Field {
        /// The field (`loc`, `restrictflag`, `name`).
        field: &'static str,
    },
    /// An Euler rotation for an object that rotates another way.
    #[error("the object rotates by {mode}, not by Euler angles")]
    RotationMode {
        /// `quaternion` or `axis_angle`.
        mode: String,
    },
    /// A name that can't be written.
    #[error("the name can't be written: {why}")]
    Name {
        /// Why (fixed text).
        why: &'static str,
    },
    /// A value that isn't finite.
    #[error("a value isn't finite")]
    Value,
    /// The writer's own check of the result failed: nothing is returned.
    #[error("the written file didn't check: {why}")]
    Check {
        /// What disagreed (fixed text).
        why: &'static str,
    },
}

/// A written file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Written {
    /// The file, stored the way the input was.
    pub bytes: Vec<u8>,
    /// How it is stored.
    pub compression: Compression,
    /// The fields written, with their places in the decompressed file.
    pub patched: Vec<FieldSpan>,
}

/// The file decompressed whole, under `limits`.
pub fn decompress(bytes: &[u8], limits: &ReadLimits) -> Result<(Compression, Vec<u8>), ReadError> {
    let mut src = bytes;
    let opened = compress::open(&mut src, limits)?;
    let mut raw = Vec::new();
    opened
        .stream
        .take(limits.max_raw_bytes.saturating_add(1))
        .read_to_end(&mut raw)
        .map_err(compress::read_error)?;
    if raw.len() as u64 > limits.max_raw_bytes {
        return Err(ReadError::Limit {
            what: "bytes after decompression",
            limit: limits.max_raw_bytes,
        });
    }
    Ok((opened.compression, raw))
}

/// `raw` stored as `how`, checked by decoding it again.
fn store(raw: Vec<u8>, how: Compression) -> Result<Vec<u8>, WriteError> {
    let failed = |why| WriteError::Check { why };
    let out = match how {
        Compression::Gzip => {
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(&raw).map_err(|_| failed("gzip failed"))?;
            e.finish().map_err(|_| failed("gzip failed"))?
        }
        Compression::Zstd => {
            // frames of at most ZSTD_FRAME bytes, so no frame asks for a larger window
            let mut out = Vec::new();
            for chunk in raw.chunks(ZSTD_FRAME) {
                out.extend(ruzstd::encoding::compress_to_vec(
                    chunk,
                    ruzstd::encoding::CompressionLevel::Fastest,
                ));
            }
            out
        }
        _ => return Ok(raw),
    };
    let limits = ReadLimits {
        max_raw_bytes: raw.len() as u64,
        max_window_bytes: u64::MAX,
        ..ReadLimits::default()
    };
    match decompress(&out, &limits) {
        Ok((h, back)) if h == how && back == raw => Ok(out),
        _ => Err(failed(
            "the compressed file doesn't decode to the patched one",
        )),
    }
}

/// The one object `key` names, writable.
fn object<'m>(model: &'m SceneModel, key: &str) -> Result<&'m IdBlock, WriteError> {
    let mut hits = model.ids.iter().filter(|i| i.key.as_deref() == Some(key));
    let id = match (hits.next(), hits.next()) {
        (None, _) => return Err(WriteError::NotFound(key.to_owned())),
        (Some(_), Some(_)) => return Err(WriteError::Ambiguous(key.to_owned())),
        (Some(id), None) => id,
    };
    if id.library.is_some() {
        return Err(WriteError::Linked(key.to_owned()));
    }
    if id.kind != "object" || id.struct_name != "Object" {
        return Err(WriteError::NotAnObject(key.to_owned()));
    }
    Ok(id)
}

/// The span of the first of `names` the object has, of a type in `ty` and `len` bytes.
fn span<'a>(
    id: &'a IdBlock,
    names: &[&str],
    field: &'static str,
    ty: &[&str],
    len: Option<u32>,
) -> Result<&'a FieldSpan, WriteError> {
    names
        .iter()
        .find_map(|n| id.spans.iter().find(|s| s.field == *n))
        .filter(|s| ty.contains(&s.type_name.as_str()) && len.is_none_or(|l| s.len == l))
        .ok_or(WriteError::Field { field })
}

fn put_f32s(out: &mut [u8], at: usize, v: &[f32; 3], e: Endian) -> Result<(), WriteError> {
    if v.iter().any(|x| !x.is_finite()) {
        return Err(WriteError::Value);
    }
    for (i, x) in v.iter().enumerate() {
        let b = if e.big {
            x.to_be_bytes()
        } else {
            x.to_le_bytes()
        };
        out[at + 4 * i..at + 4 * i + 4].copy_from_slice(&b);
    }
    Ok(())
}

/// An unsigned integer of `len` bytes at `at`.
fn get_uint(b: &[u8], at: usize, len: usize, e: Endian) -> u64 {
    let mut v = 0u64;
    for i in 0..len {
        let byte = u64::from(b[at + if e.big { i } else { len - 1 - i }]);
        v = (v << 8) | byte;
    }
    v
}

fn put_uint(b: &mut [u8], at: usize, len: usize, e: Endian, v: u64) {
    for i in 0..len {
        let shift = 8 * if e.big { len - 1 - i } else { i };
        b[at + i] = (v >> shift) as u8;
    }
}

/// What a written object should read as, folded from the edits.
#[derive(Debug, Default)]
struct Want {
    name: Option<String>,
    loc: Option<[f32; 3]>,
    rot: Option<[f32; 3]>,
    scale: Option<[f32; 3]>,
    bits: [Option<bool>; 3],
}

/// Applies `edits`, in order, to the file `bytes` holds, and returns it written; or refuses
/// and writes nothing.
pub fn write(bytes: &[u8], edits: &[Edit], limits: &ReadLimits) -> Result<Written, WriteError> {
    let (how, raw) = decompress(bytes, limits)?;
    let file = read_blend(&mut &raw[..], limits)?;
    let version = file.header.version;
    if !(WRITE_FROM..=WRITE_TO).contains(&version) {
        return Err(WriteError::Version { version });
    }
    let model = read_scene(&file)?;
    let e = file.endian();
    let mut out = raw.clone();
    let mut patched: Vec<FieldSpan> = Vec::new();
    let mut wants: BTreeMap<String, Want> = BTreeMap::new();
    // the names the file's own objects hold, as the edits go
    let mut names: BTreeMap<Vec<u8>, String> = model
        .ids
        .iter()
        .filter(|i| i.code == "OB" && i.library.is_none())
        .filter_map(|i| Some((i.name_bytes.clone(), i.key.clone()?)))
        .collect();
    for edit in edits {
        let id = object(&model, &edit.object)?;
        let want = wants.entry(edit.object.clone()).or_default();
        let s = match &edit.change {
            Change::Location(v) => {
                let s = span(id, &["loc"], "loc", &["float"], Some(12))?;
                let shift = if crate::mutant::active("write_wrong_offset") {
                    4
                } else {
                    0
                };
                put_f32s(&mut out, s.offset as usize + shift, v, e)?;
                want.loc = Some(*v);
                s
            }
            Change::Rotation(v) => {
                let s = span(id, &["rot"], "rot", &["float"], Some(12))?;
                if let Some((_, mode)) = id.named.iter().find(|(n, _)| n == "rotation_mode") {
                    if mode == "quaternion" || mode == "axis_angle" {
                        return Err(WriteError::RotationMode { mode: mode.clone() });
                    }
                }
                put_f32s(&mut out, s.offset as usize, v, e)?;
                want.rot = Some(*v);
                s
            }
            Change::Scale(v) => {
                let s = span(id, &["scale", "size"], "size", &["float"], Some(12))?;
                put_f32s(&mut out, s.offset as usize, v, e)?;
                want.scale = Some(*v);
                s
            }
            Change::Visibility {
                viewport,
                select,
                render,
            } => {
                let s = span(
                    id,
                    &["restrictflag"],
                    "restrictflag",
                    &[
                        "short", "ushort", "char", "uchar", "int", "int16_t", "uint16_t",
                    ],
                    None,
                )?;
                let (at, len) = (s.offset as usize, s.len as usize);
                if !matches!(len, 1 | 2 | 4) {
                    return Err(WriteError::Field {
                        field: "restrictflag",
                    });
                }
                let mut v = get_uint(&out, at, len, e);
                for (i, (on, bit)) in [(viewport, 1u64), (select, 2), (render, 4)]
                    .into_iter()
                    .enumerate()
                {
                    match on {
                        Some(true) => v |= bit,
                        Some(false) => v &= !bit,
                        None => continue,
                    }
                    want.bits[i] = *on;
                }
                put_uint(&mut out, at, len, e, v);
                s
            }
            Change::Name(n) => {
                let s = span(id, &["name"], "name", &["char"], None)?;
                let nb = n.as_bytes();
                if nb.is_empty() {
                    return Err(WriteError::Name { why: "it is empty" });
                }
                if nb.contains(&0) {
                    return Err(WriteError::Name {
                        why: "it holds a NUL",
                    });
                }
                // the type code, the name, a NUL
                if nb.len() + 3 > s.len as usize {
                    return Err(WriteError::Name {
                        why: "it doesn't fit the name field",
                    });
                }
                if names.get(nb).is_some_and(|k| *k != edit.object)
                    && !crate::mutant::active("write_names_not_unique")
                {
                    return Err(WriteError::Name {
                        why: "another object has it",
                    });
                }
                names.retain(|_, k| *k != edit.object);
                names.insert(nb.to_vec(), edit.object.clone());
                let at = s.offset as usize;
                let field = &mut out[at..at + s.len as usize];
                field.fill(0);
                field[..2].copy_from_slice(b"OB");
                field[2..2 + nb.len()].copy_from_slice(nb);
                want.name = Some(n.clone());
                s
            }
        };
        if !patched.iter().any(|p| p.offset == s.offset) {
            patched.push(s.clone());
        }
    }
    check(&file, &raw, &out, &patched, &model, &wants, limits)?;
    let bytes = store(out, how)?;
    Ok(Written {
        bytes,
        compression: how,
        patched,
    })
}

/// The result reads to the same blocks, changed only inside the patched fields, with every
/// object's last values read back.
fn check(
    before: &BlendFile,
    raw: &[u8],
    out: &[u8],
    patched: &[FieldSpan],
    model_before: &SceneModel,
    wants: &BTreeMap<String, Want>,
    limits: &ReadLimits,
) -> Result<(), WriteError> {
    let fail = |why| Err(WriteError::Check { why });
    if out.len() != raw.len() {
        return fail("the file changed length");
    }
    let inside = |i: usize| {
        patched
            .iter()
            .any(|s| (s.offset as usize..(s.offset + u64::from(s.len)) as usize).contains(&i))
    };
    if raw
        .iter()
        .zip(out)
        .enumerate()
        .any(|(i, (a, b))| a != b && !inside(i))
    {
        return fail("a byte outside the patched fields changed");
    }
    let Ok(after) = read_blend(&mut &out[..], limits) else {
        return fail("the written file doesn't read");
    };
    if after.blocks.len() != before.blocks.len()
        || after
            .blocks
            .iter()
            .zip(&before.blocks)
            .any(|(a, b)| a.head != b.head || a.owner != b.owner)
    {
        return fail("the blocks moved");
    }
    let Ok(model) = read_scene(&after) else {
        return fail("the written file doesn't model");
    };
    if model.ids.len() != model_before.ids.len() {
        return fail("the data-blocks changed");
    }
    for (key, want) in wants {
        let Some(was) = model_before
            .ids
            .iter()
            .position(|i| i.key.as_deref() == Some(key))
        else {
            return fail("an edited object isn't found again");
        };
        let id = &model.ids[was];
        let now_key = match &want.name {
            Some(n) => key_of("OB", n.as_bytes(), None),
            None => Some(key.clone()),
        };
        let field = |n: &str| {
            id.fields
                .iter()
                .find(|(f, _)| f == n)
                .map(|(_, v)| v.clone())
        };
        let named = |n: &str| {
            id.named
                .iter()
                .find(|(f, _)| f == n)
                .map(|(_, v)| v == "true")
        };
        let floats = |v: &[f32; 3]| Some(v.map(|x| x.to_string()).join(","));
        let ok = id.key == now_key
            && want.name.as_ref().is_none_or(|n| id.name == *n)
            && want.loc.is_none_or(|v| field("loc") == floats(&v))
            && want.rot.is_none_or(|v| field("rot") == floats(&v))
            && want
                .scale
                .is_none_or(|v| field("scale").or_else(|| field("size")) == floats(&v))
            && ["hide_viewport", "hide_select", "hide_render"]
                .iter()
                .zip(want.bits)
                .all(|(n, w)| w.is_none_or(|w| named(n) == Some(w)));
        if !ok {
            return fail("a value didn't read back");
        }
    }
    Ok(())
}
