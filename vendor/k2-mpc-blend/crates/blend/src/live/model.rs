//! The scene as the live store holds it ([`LiveScene`]): read from a file ([`read`]) and
//! written as the kit's roots under the keys it is given ([`LiveScene::to_content`]).
//!
//! **Roots**:
//!
//! | Root | Kind | Holds |
//! |---|---|---|
//! | `colls` | MovableList | key `c<n>`, value `scene` (a scene's master collection) or `collection`; `c0`, value `none`, holds the objects no scene or collection holds (only when there are such) |
//! | `coll_props` | Map, composite with `colls` | `<coll>\x1f<field>` |
//! | `objs` | MovableList | key `o<n>`, value the object's type word (`mesh`, `camera`, `light`, `empty`...) |
//! | `obj_props` | Map, composite with `objs` | `<obj>\x1f<field>` |
//! | `scene` | Map | the file's facts (version, header form, pointer size, byte order, storage, the longest name, the unit scale every scene holds when every scene is metric and they agree) and the key marks |
//!
//! **Order** is the file's: collections and objects in the order the file holds their
//! data-blocks, `c0` last. The decode always yields them so, which is what the carrier's
//! pairing relies on: an outside save is taken in through the same order.
//!
//! **Object fields:** `coll` (the first collection holding it, in file order: its container),
//! `also` (the others, comma-separated; only when there are others), `name`, `nid` (the file
//! key, `@ob:Cube` or `@li:lib.blend/ob:Cube`; a live rename sets it with `name`), `data` (its
//! data's kind and key, `mesh @me:Cube`; none for an empty), `parent` (the parent object's live
//! key, or its file key when the parent isn't listed), `xform` (**one field**:
//! `lx,ly,lz,rx,ry,rz,sx,sy,sz`, each `f32` as its shortest round-trip decimal), `rotmode`
//! (the rotation mode's word), `quat` (`w,x,y,z`, when it rotates by quaternion), `hidden`,
//! `noselect`, `norender` (`restrictflag` bits 1, 2 and 4, `0`/`1`) and `edit` ([`EditMode`]).
//!
//! **Collection fields:** `name`, `nid` (`@gr:Props`, a scene `@sc:Scene`), `parent` (the
//! collection or scene holding it first, by live key) and `hidden` (a collection's own viewport
//! flag; a scene's is `0`). `c0` has none.
//!
//! **Keys are minted** (`c<n>`, `o<n>`, in file order at import): a file's names don't fit the
//! kit's key grammar, and a key is only as stable as the name. The marks (`mark/c`, `mark/o`)
//! never go down. **Declines:** a file the reader refuses; a version outside the writer's
//! ([`crate::write::WRITE_FROM`] to [`crate::write::WRITE_TO`], `live_version`); a pointer the
//! model couldn't resolve on a listed object, collection or scene (`live_ambiguous`); a
//! transform that isn't finite (`live_value`); a file over [`LiveLimits::max_file_bytes`]
//! (`live_too_large`).
//!
//! **What drives a transform** (`edit` `a`): an object whose own pointers, read by the names the
//! file gives them, show animation data holding an action, NLA tracks or drivers (`adt`), a
//! constraint (`constraints`), or an action named on the object itself (`action`). The app
//! recomputes such a transform on load, so a placement wouldn't stick (**HYPOTHESIS**, to be
//! confirmed on saved files; refusing is the safe side). Read by field name only, nothing
//! modelled: an action that animates something other than the transform marks the object too
//! (refused, never wrong). A file whose schema has no such fields marks nothing.
//!
//! **What only the file holds** ([`PlaceFacts`], on each [`LiveObj`], never in the store): how
//! an object is parented (`partype`), its parent inverse matrix (`parentinv`), its axis and
//! angle (`rotAxis`, `rotAngle`), whether its delta transforms are identity, and whether its
//! transform is driven. The view's placement ([`super::place`]) reads them from the carrier.

use std::collections::{BTreeMap, HashMap};

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Key, Value};
use k2_mpc_base::session::Declined;
use k2_mpc_base::table::CELL_SEP;

use crate::compress::Compression;
use crate::error::{ReadError, ReadLimits};
use crate::header::HeaderForm;
use crate::mutant;
use crate::reader::{read_blend, BlendFile, Endian, Target};
use crate::scene::{read_scene, Detail, IdBlock, SceneModel};
use crate::write::{decompress, WRITE_FROM, WRITE_TO};

/// The collections' list root.
pub const COLLS: &str = "colls";
/// The objects' list root.
pub const OBJS: &str = "objs";
/// The collections' fields.
pub const COLL_PROPS: &str = "coll_props";
/// The objects' fields.
pub const OBJ_PROPS: &str = "obj_props";
/// The file's facts and the key marks.
pub const SCENE: &str = "scene";

/// Field names (the module's tables say what each holds).
#[allow(missing_docs)]
pub mod f {
    pub const COLL: &str = "coll";
    pub const ALSO: &str = "also";
    pub const NAME: &str = "name";
    pub const NID: &str = "nid";
    pub const DATA: &str = "data";
    pub const PARENT: &str = "parent";
    pub const XFORM: &str = "xform";
    pub const ROTMODE: &str = "rotmode";
    pub const QUAT: &str = "quat";
    pub const HIDDEN: &str = "hidden";
    pub const NOSELECT: &str = "noselect";
    pub const NORENDER: &str = "norender";
    pub const EDIT: &str = "edit";
}

/// The scene map's keys: the file's facts, then the key marks (the highest `c<n>` and `o<n>`
/// minted).
#[allow(missing_docs)]
pub mod s {
    pub const VERSION: &str = "version";
    pub const HEADER: &str = "header";
    pub const POINTER_SIZE: &str = "pointer_size";
    pub const ENDIAN: &str = "endian";
    pub const STORAGE: &str = "storage";
    /// The longest name an object's name field holds, in bytes.
    pub const NAME_MAX: &str = "name_max";
    /// The unit scale every scene holds (`unit.scale_length`), only when every scene's unit
    /// system is metric and the scales agree (`super::METRIC_SYSTEM`): the view's space
    /// ([`super::super::place::view_space`]).
    pub const UNIT_SCALE: &str = "unit_scale";
    pub const MARK_C: &str = "mark/c";
    pub const MARK_O: &str = "mark/o";
}

/// The key of `c0`: the objects no scene or collection holds.
pub const NONE_COLL: &str = "c0";

/// The most bytes of a file that opens live, stored (compressed or not): K2's cap on a file it
/// opens live, kept after the carrier-storage measurement (`tests/live_storage.rs`): a
/// replaced carrier is removed at once, so storage grows by one whole file per **version** the
/// kit cuts, not per write-back (100 write-backs of one object, with the kit's default config,
/// kept one version: the blob folder went from one copy of the file to two, at 8 and at 32 MiB,
/// plain and zstd). The cost that grows with size is time: a write-back of a 32 MiB file took
/// about 1.2 s plain and 1.9 s zstd (p50), each one reading and storing the whole file again.
pub const MAX_FILE_BYTES: u64 = 32 << 20;
/// The most bytes after decompression a live file may hold.
pub const MAX_RAW_BYTES: u64 = 512 << 20;

/// The live tier's bounds: the reader's, and the file-size cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveLimits {
    /// The reader's limits, for the decode, the splice and outside saves.
    pub read: ReadLimits,
    /// The most bytes of a file that opens live, stored ([`MAX_FILE_BYTES`]).
    pub max_file_bytes: u64,
}

impl Default for LiveLimits {
    fn default() -> Self {
        let d = ReadLimits::default();
        LiveLimits {
            read: ReadLimits::new(
                MAX_RAW_BYTES,
                d.max_blocks,
                d.max_retained_bytes,
                d.retain_block_max,
                d.max_sdna_bytes,
                d.max_window_bytes,
            ),
            max_file_bytes: MAX_FILE_BYTES,
        }
    }
}

/// What an object's transform and fields allow live ([`f::EDIT`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditMode {
    /// `1`: place, visibility and rename.
    Full,
    /// `t`: its rotation isn't Euler (quaternion or axis and angle): `--rot` is refused; the
    /// location, scale, visibility and name are written.
    NoRotation,
    /// `a`: animation, a driver or a constraint drives its transform: `object place` is refused
    /// (the app would recompute it on load); visibility and name are written.
    Driven,
    /// `0`: nothing is written (linked from a library, or a field the writer can't find).
    Never,
}

impl EditMode {
    /// The field's value.
    pub fn as_str(self) -> &'static str {
        match self {
            EditMode::Full => "1",
            EditMode::NoRotation => "t",
            EditMode::Driven => "a",
            EditMode::Never => "0",
        }
    }

    /// [`EditMode::as_str`]'s inverse.
    pub fn parse(s: &str) -> Option<EditMode> {
        match s {
            "1" => Some(EditMode::Full),
            "t" => Some(EditMode::NoRotation),
            "a" => Some(EditMode::Driven),
            "0" => Some(EditMode::Never),
            _ => None,
        }
    }
}

/// A collection's kind, its list value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollKind {
    /// A scene's master collection.
    Scene,
    /// A collection.
    Collection,
    /// `c0`: no collection.
    None,
}

impl CollKind {
    /// The list value.
    pub fn as_str(self) -> &'static str {
        match self {
            CollKind::Scene => "scene",
            CollKind::Collection => "collection",
            CollKind::None => "none",
        }
    }
}

/// A collection (or a scene's master collection) as the store holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveColl {
    /// What it is.
    pub kind: CollKind,
    /// Its name (a scene's for a master collection).
    pub name: String,
    /// Its file key, or empty.
    pub nid: String,
    /// The collection holding it first, by index.
    pub parent: Option<usize>,
    /// Its own viewport flag.
    pub hidden: bool,
}

/// An object's parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parent {
    /// No parent.
    None,
    /// A listed object, by index.
    Obj(usize),
    /// A data-block that isn't listed, by its file key.
    Other(String),
}

/// An object as the store holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveObj {
    /// The list value: its type word.
    pub kind: String,
    /// Its container, by index into the collections.
    pub coll: usize,
    /// The other collections holding it, by index.
    pub also: Vec<usize>,
    /// Its name.
    pub name: String,
    /// Its file key, or empty when over the kit's length limit.
    pub nid: String,
    /// Its data's kind and key, or empty.
    pub data: String,
    /// Its parent.
    pub parent: Parent,
    /// `None` when the file's schema lacks a transform field the writer needs.
    pub xform: Option<[f32; 9]>,
    /// The rotation mode's word, or its number when unnamed.
    pub rotmode: String,
    /// `w,x,y,z`, when it rotates by quaternion.
    pub quat: Option<[f32; 4]>,
    /// `restrictflag` bits 1, 2 and 4: hidden in viewports, not selectable, not rendered.
    pub hide: [bool; 3],
    /// What may be written live.
    pub edit: EditMode,
    /// What only the file holds of its placement ([`PlaceFacts`]): never in the store; the
    /// view's placement reads it from the carrier.
    pub place: PlaceFacts,
}

/// What only the file holds of an object's placement, read by the names the file's own schema
/// gives the fields. The store holds none of it: the view's placement
/// ([`super::place`]) reads it from the carrier.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PlaceFacts {
    /// How it is parented (`partype`); `None` when the schema has no such field.
    pub partype: Option<i64>,
    /// The parent inverse matrix (`parentinv`), its 16 floats as stored (column-major);
    /// `None` when the schema has no such field.
    pub parentinv: Option<[f32; 16]>,
    /// Its axis and angle (`rotAxis`, `rotAngle`); `None` when the schema lacks either.
    pub axis_angle: Option<([f32; 3], f32)>,
    /// Its delta transforms are identity, or the schema has none: `false` when one isn't,
    /// or one can't be read (`dloc`, `drot`, `dquat`, `dscale`, `drotAngle`).
    pub deltas_identity: bool,
    /// Something drives its transform ([`driven`]), whether or not it is written live.
    pub driven: bool,
}

/// A field read: the schema lacks it, it isn't the shape asked for (or the body ends), or its
/// value.
enum Read<T> {
    Missing,
    Bad,
    Got(T),
}

/// `N` floats of field `name` of struct `s` in `body`.
fn floats_of<const N: usize>(file: &BlendFile, s: u32, body: &[u8], name: &str) -> Read<[f32; N]> {
    let Some(f) = file.sdna.field(s, name) else {
        return Read::Missing;
    };
    if f.is_pointer() || file.sdna.type_name(f.type_index) != "float" || f.array_len as usize != N {
        return Read::Bad;
    }
    let e = file.endian();
    let mut out = [0f32; N];
    for (i, x) in out.iter_mut().enumerate() {
        match e.f32(body, f.offset as usize + 4 * i) {
            Some(v) => *x = v,
            None => return Read::Bad,
        }
    }
    Read::Got(out)
}

/// An integer field `name` of struct `s` in `body`.
fn int_of(file: &BlendFile, s: u32, body: &[u8], name: &str) -> Read<i64> {
    let Some(f) = file.sdna.field(s, name) else {
        return Read::Missing;
    };
    if f.is_pointer() || f.array_len != 1 {
        return Read::Bad;
    }
    let (e, at) = (file.endian(), f.offset as usize);
    let v = match file.sdna.type_name(f.type_index) {
        "short" | "int16_t" => e.i16(body, at).map(i64::from),
        "ushort" | "uint16_t" => e.u16(body, at).map(i64::from),
        "int" | "int32_t" => e.i32(body, at).map(i64::from),
        "char" | "uchar" | "uint8_t" => body.get(at).map(|b| i64::from(*b)),
        "int8_t" => body.get(at).map(|b| i64::from(*b as i8)),
        _ => None,
    };
    v.map_or(Read::Bad, Read::Got)
}

/// What only the file holds of the placement of the object in `block` ([`PlaceFacts`]).
pub fn place_facts(file: &BlendFile, block: u32) -> PlaceFacts {
    let driven = driven(file, block);
    let s = file.blocks[block as usize].head.sdna;
    let Some(body) = file.body(block) else {
        return PlaceFacts {
            driven,
            ..PlaceFacts::default()
        };
    };
    fn got<T>(r: Read<T>) -> Option<T> {
        match r {
            Read::Got(v) => Some(v),
            _ => None,
        }
    }
    // each delta: missing is identity (the file holds none); unreadable is not
    fn ident<const N: usize>(r: Read<[f32; N]>, want: [f32; N]) -> bool {
        match r {
            Read::Missing => true,
            Read::Bad => false,
            Read::Got(v) => v == want,
        }
    }
    let dscale = match floats_of::<3>(file, s, body, "dscale") {
        Read::Missing => floats_of::<3>(file, s, body, "dsize"),
        r => r,
    };
    let deltas_identity = ident(floats_of::<3>(file, s, body, "dloc"), [0.0; 3])
        && ident(floats_of::<3>(file, s, body, "drot"), [0.0; 3])
        && ident(floats_of::<4>(file, s, body, "dquat"), [1.0, 0.0, 0.0, 0.0])
        && ident(dscale, [1.0; 3])
        && ident(floats_of::<1>(file, s, body, "drotAngle"), [0.0]);
    let axis = got(floats_of::<3>(file, s, body, "rotAxis"));
    let angle = got(floats_of::<1>(file, s, body, "rotAngle"));
    PlaceFacts {
        partype: got(int_of(file, s, body, "partype")),
        parentinv: got(floats_of::<16>(file, s, body, "parentinv")),
        axis_angle: axis.zip(angle).map(|(a, t)| (a, t[0])),
        deltas_identity,
        driven,
    }
}

/// The unit system that gives a scene a real-world scale: 1 (**HYPOTHESIS**: metric; the
/// system numbers' meanings aren't confirmed by a saved file, `PROVENANCE.md`).
pub const METRIC_SYSTEM: u8 = 1;

/// The unit scale every scene of `file` holds (`unit.scale_length`), as the store writes a
/// float, when **every scene's unit system is [`METRIC_SYSTEM`]** and their scales agree;
/// `None` (no real-world scale known) when a scene lacks either field, one can't be read, a
/// scene's system is another (0, 2, ...: systems that differ count so too), or two scales
/// differ.
fn unit_scale(file: &BlendFile, model: &SceneModel) -> Option<String> {
    let mut seen: Option<f32> = None;
    for id in &model.ids {
        if !matches!(id.detail, Detail::Scene { .. }) {
            continue;
        }
        let s = file.blocks[id.block as usize].head.sdna;
        let body = file.body(id.block)?;
        let (sat, sf) = file.sdna.field_path(s, "unit.system")?;
        let one_byte = matches!(
            file.sdna.type_name(sf.type_index),
            "char" | "uchar" | "uint8_t"
        );
        if sf.is_pointer() || !one_byte || sf.array_len != 1 {
            return None;
        }
        let mut system = *body.get(sat as usize)?;
        if mutant::active("UnitSystemZeroAsMetric") && system == 0 {
            // planted: a scene of unit system 0 read as metric
            system = METRIC_SYSTEM;
        }
        if system != METRIC_SYSTEM {
            return None;
        }
        let (at, f) = file.sdna.field_path(s, "unit.scale_length")?;
        if f.is_pointer() || file.sdna.type_name(f.type_index) != "float" || f.array_len != 1 {
            return None;
        }
        let v = file.endian().f32(body, at as usize)?;
        if !v.is_finite() || seen.is_some_and(|w| w.to_bits() != v.to_bits()) {
            return None;
        }
        seen = Some(v);
    }
    seen.map(ftext)
}

/// A file as the live store holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveScene {
    /// In file order; `c0` last when there.
    pub colls: Vec<LiveColl>,
    /// In file order.
    pub objs: Vec<LiveObj>,
    /// The scene map's facts.
    pub facts: Vec<(&'static str, String)>,
    /// How the file is stored.
    pub storage: Compression,
}

fn decline(code: &'static str, message: impl Into<String>) -> Declined {
    Declined {
        code,
        message: message.into(),
    }
}

/// A reader's refusal as a decline (fixed text only).
pub fn read_declined(e: ReadError) -> Declined {
    let code = match &e {
        ReadError::NotBlend => "not_blend",
        ReadError::Unknown { .. } => "unknown_form",
        ReadError::Limit { .. } => "live_too_large",
        _ => "unreadable",
    };
    decline(code, e.to_string())
}

/// A float as the store writes it: its shortest decimal that parses back to the same bits.
pub fn ftext(x: f32) -> String {
    if mutant::active("FloatRounded") {
        return format!("{x:.4}");
    }
    x.to_string()
}

/// Floats as one comma-separated value.
pub fn floats_text(v: &[f32]) -> String {
    v.iter().map(|x| ftext(*x)).collect::<Vec<_>>().join(",")
}

/// `n` finite floats from one comma-separated value, or `None`.
pub fn parse_floats<const N: usize>(s: &str) -> Option<[f32; N]> {
    let mut out = [0f32; N];
    let mut parts = s.split(',');
    for x in &mut out {
        let v: f32 = parts.next()?.trim().parse().ok()?;
        if !v.is_finite() {
            return None;
        }
        *x = v;
    }
    parts.next().is_none().then_some(out)
}

/// A composite map key: `<elem>\x1f<field>`.
pub fn prop(elem: &Key, field: &str) -> String {
    format!("{}{CELL_SEP}{field}", elem.as_str())
}

/// `c<n>`.
pub fn coll_key(n: u64) -> Key {
    Key::new(format!("c{n}")).expect("c<u64> is a list key")
}

/// `o<n>`.
pub fn obj_key(n: u64) -> Key {
    Key::new(format!("o{n}")).expect("o<u64> is a list key")
}

/// The number of a key `<prefix><n>` (no sign, no leading zero), or `None`.
pub fn key_num(k: &str, prefix: char) -> Option<u64> {
    let rest = k.strip_prefix(prefix)?;
    if rest.is_empty() || (rest.len() > 1 && rest.starts_with('0')) {
        return None;
    }
    if !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

/// Reads `bytes` into the live scene, or declines.
pub fn read(bytes: &[u8], limits: &LiveLimits) -> Result<LiveScene, Declined> {
    if bytes.len() as u64 > limits.max_file_bytes {
        return Err(decline(
            "live_too_large",
            format!(
                "the file is {} bytes, over the live cap of {}",
                bytes.len(),
                limits.max_file_bytes
            ),
        ));
    }
    let (how, raw) = decompress(bytes, &limits.read).map_err(read_declined)?;
    let file = read_blend(&mut &raw[..], &limits.read).map_err(read_declined)?;
    let v = file.header.version;
    if !(WRITE_FROM..=WRITE_TO).contains(&v) {
        return Err(decline(
            "live_version",
            super::errors::live_version_message(&file.header.version_text()),
        ));
    }
    let model = read_scene(&file).map_err(read_declined)?;
    from_model(&file, &raw, &model, how)
}

/// The `f32`s of a span of `n` floats.
fn span_floats<const N: usize>(
    id: &IdBlock,
    names: &[&str],
    raw: &[u8],
    e: Endian,
) -> Option<[f32; N]> {
    let s = names
        .iter()
        .find_map(|n| id.spans.iter().find(|s| s.field == *n))?;
    if s.type_name != "float" || s.len as usize != 4 * N {
        return None;
    }
    let mut out = [0f32; N];
    for (i, x) in out.iter_mut().enumerate() {
        *x = e.f32(raw, s.offset as usize + 4 * i)?;
    }
    Some(out)
}

fn named<'a>(id: &'a IdBlock, n: &str) -> Option<&'a str> {
    id.named
        .iter()
        .find(|(k, _)| k == n)
        .map(|(_, v)| v.as_str())
}

fn field<'a>(id: &'a IdBlock, n: &str) -> Option<&'a str> {
    id.fields
        .iter()
        .find(|(k, _)| k == n)
        .map(|(_, v)| v.as_str())
}

/// A pointer field of struct `s` at `path` (`adt`, `constraints.first`) in `body`: `Some(0)` for
/// null, `None` when the schema has no such single pointer (the reader can't tell).
fn ptr_at(file: &BlendFile, s: u32, body: &[u8], path: &str) -> Option<u64> {
    let (at, f) = file.sdna.field_path(s, path)?;
    if f.pointer_depth != 1 || f.function_pointer {
        return None;
    }
    file.endian().ptr(body, at as usize)
}

/// Whether the object in `block` has its transform driven: animation data holding an action,
/// NLA tracks or drivers, or a constraint. A field the schema has whose pointer the reader can't
/// follow counts as driven; a field the schema lacks counts as nothing.
pub fn driven(file: &BlendFile, block: u32) -> bool {
    let s = file.blocks[block as usize].head.sdna;
    let Some(body) = file.body(block) else {
        return true;
    };
    let has = |path: &str| file.sdna.field_path(s, path).is_some();
    // a constraint; an action named on the object itself (an older field the schemas still
    // carry): counted as driving it, the safe side
    for path in ["constraints.first", "action"] {
        if has(path) && ptr_at(file, s, body, path) != Some(0) {
            return true;
        }
    }
    if !has("adt") {
        return false;
    }
    let adt = match ptr_at(file, s, body, "adt") {
        Some(0) => return false,
        Some(p) => p,
        None => return true,
    };
    let Target::Block(b) = file.resolve(adt, block) else {
        return true;
    };
    let as_ = file.blocks[b as usize].head.sdna;
    let Some(ab) = file.body(b) else {
        return true;
    };
    ["action", "nla_tracks.first", "drivers.first"]
        .iter()
        .any(|p| ptr_at(file, as_, ab, p) != Some(0))
}

/// The model's indexes of the collections the live scene lists, in its order (the file's):
/// each scene (its master collection) and each collection. `c0`, when there, comes after them.
pub fn coll_blocks(model: &SceneModel) -> Vec<usize> {
    model
        .ids
        .iter()
        .enumerate()
        .filter(|(_, id)| match &id.detail {
            Detail::Scene { .. } => true,
            Detail::Collection { .. } => id.struct_name != "ID",
            _ => false,
        })
        .map(|(i, _)| i)
        .collect()
}

/// Each listed collection's child collections ([`coll_blocks`]' order), by their place in that
/// order, in the order the file lists them: a scene's are its master collection's. A child that
/// isn't listed, or the collection itself, is left out.
pub fn coll_children(model: &SceneModel) -> Vec<Vec<usize>> {
    let blocks = coll_blocks(model);
    let at: HashMap<usize, usize> = blocks.iter().enumerate().map(|(c, &i)| (i, c)).collect();
    blocks
        .iter()
        .enumerate()
        .map(|(me, &i)| match &model.ids[i].detail {
            Detail::Scene { collections, .. } | Detail::Collection { collections, .. } => {
                collections
                    .iter()
                    .filter_map(|k| at.get(k).copied())
                    .filter(|&k| k != me)
                    .collect()
            }
            _ => Vec::new(),
        })
        .collect()
}

/// The live scene of a model of `file` (`raw` its decompressed bytes, `how` its storage).
pub fn from_model(
    file: &BlendFile,
    raw: &[u8],
    model: &SceneModel,
    how: Compression,
) -> Result<LiveScene, Declined> {
    let ids = &model.ids;
    let e = file.endian();
    let ambiguous = |i: usize| {
        decline(
            "live_ambiguous",
            format!(
                "{} holds pointers the reader couldn't resolve",
                ids[i].key.as_deref().unwrap_or("a data-block")
            ),
        )
    };
    // the collections and scenes, in file order
    let mut colls: Vec<LiveColl> = Vec::new();
    let mut coll_of: HashMap<usize, usize> = HashMap::new();
    let mut coll_ids: Vec<usize> = Vec::new();
    for i in coll_blocks(model) {
        let id = &ids[i];
        let kind = match &id.detail {
            Detail::Scene { .. } => CollKind::Scene,
            _ => CollKind::Collection,
        };
        if id.unresolved > 0 {
            return Err(ambiguous(i));
        }
        coll_of.insert(i, colls.len());
        coll_ids.push(i);
        colls.push(LiveColl {
            kind,
            name: id.name.clone(),
            nid: id.key.clone().unwrap_or_default(),
            parent: None,
            hidden: kind == CollKind::Collection && id.hidden == Some(true),
        });
    }
    // the objects, in file order
    let mut obj_of: HashMap<usize, usize> = HashMap::new();
    let mut obj_ids: Vec<usize> = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        if matches!(id.detail, Detail::Object { .. }) && id.struct_name == "Object" {
            obj_of.insert(i, obj_ids.len());
            obj_ids.push(i);
        }
    }
    // what holds what: each holder in file order
    let mut holders: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut coll_parent: HashMap<usize, usize> = HashMap::new();
    for (ci, &i) in coll_ids.iter().enumerate() {
        let (objects, collections) = match &ids[i].detail {
            Detail::Scene {
                objects,
                collections,
                ..
            }
            | Detail::Collection {
                objects,
                collections,
            } => (objects, collections),
            _ => continue,
        };
        for o in objects {
            if let Some(&oi) = obj_of.get(o) {
                let h = holders.entry(oi).or_default();
                if !h.contains(&ci) {
                    h.push(ci);
                }
            }
        }
        for c in collections {
            if let Some(&cc) = coll_of.get(c) {
                if cc != ci {
                    coll_parent.entry(cc).or_insert(ci);
                }
            }
        }
    }
    for (cc, p) in coll_parent {
        colls[cc].parent = Some(p);
    }
    let none_needed = (0..obj_ids.len()).any(|oi| !holders.contains_key(&oi));
    let none = colls.len();
    if none_needed {
        colls.push(LiveColl {
            kind: CollKind::None,
            name: String::new(),
            nid: String::new(),
            parent: None,
            hidden: false,
        });
    }
    let mut objs = Vec::with_capacity(obj_ids.len());
    for (oi, &i) in obj_ids.iter().enumerate() {
        let id = &ids[i];
        if id.unresolved > 0 {
            return Err(ambiguous(i));
        }
        let (data, parent) = match &id.detail {
            Detail::Object { data, parent } => (*data, *parent),
            _ => (None, None),
        };
        let hs = holders.get(&oi).cloned().unwrap_or_default();
        let (coll, also) = match hs.split_first() {
            Some((first, rest)) => (*first, rest.to_vec()),
            None => (none, Vec::new()),
        };
        let kind = named(id, "object_type").map_or_else(
            || format!("type{}", field(id, "type").unwrap_or("-")),
            str::to_owned,
        );
        let loc = span_floats::<3>(id, &["loc"], raw, e);
        let rot = span_floats::<3>(id, &["rot"], raw, e);
        let size = span_floats::<3>(id, &["scale", "size"], raw, e);
        let xform = match (loc, rot, size) {
            (Some(l), Some(r), Some(s)) => {
                let x = [l[0], l[1], l[2], r[0], r[1], r[2], s[0], s[1], s[2]];
                if x.iter().any(|v| !v.is_finite()) {
                    return Err(decline(
                        "live_value",
                        format!(
                            "{}: a transform value isn't finite",
                            id.key.as_deref().unwrap_or("an object")
                        ),
                    ));
                }
                Some(x)
            }
            _ => None,
        };
        let rotmode = named(id, "rotation_mode")
            .or_else(|| field(id, "rotmode"))
            .unwrap_or("")
            .to_owned();
        let quat = if rotmode == "quaternion" {
            span_floats::<4>(id, &["quat"], raw, e).filter(|q| q.iter().all(|v| v.is_finite()))
        } else {
            None
        };
        let flag: Option<i64> = field(id, "restrictflag").and_then(|v| v.parse().ok());
        let hide = flag.map_or([false; 3], |v| [v & 1 != 0, v & 2 != 0, v & 4 != 0]);
        let writable = id.library.is_none()
            && id.key.is_some()
            && xform.is_some()
            && flag.is_some()
            && id.spans.iter().any(|s| s.field == "name" && s.len > 3);
        let place = place_facts(file, id.block);
        let edit = if !writable {
            EditMode::Never
        } else if place.driven {
            EditMode::Driven
        } else if rotmode == "quaternion" || rotmode == "axis_angle" {
            EditMode::NoRotation
        } else {
            EditMode::Full
        };
        let key = |d: usize| ids[d].key.clone().unwrap_or_else(|| "-".to_owned());
        objs.push(LiveObj {
            kind,
            coll,
            also,
            name: id.name.clone(),
            nid: id.key.clone().unwrap_or_default(),
            data: data.map_or_else(String::new, |d| format!("{} {}", ids[d].kind, key(d))),
            parent: match parent {
                None => Parent::None,
                Some(p) => match obj_of.get(&p) {
                    Some(&po) => Parent::Obj(po),
                    None => Parent::Other(key(p)),
                },
            },
            xform,
            rotmode,
            quat,
            hide,
            edit,
            place,
        });
    }
    let h = &file.header;
    let name_max = obj_ids
        .iter()
        .find_map(|&i| ids[i].spans.iter().find(|s| s.field == "name"))
        .map_or(0, |s| s.len.saturating_sub(3));
    let mut facts = vec![
        (s::VERSION, h.version_text()),
        (
            s::HEADER,
            match h.form {
                HeaderForm::Classic => "classic",
                _ => "large",
            }
            .to_owned(),
        ),
        (s::POINTER_SIZE, h.pointer_size.to_string()),
        (
            s::ENDIAN,
            if h.big_endian { "big" } else { "little" }.to_owned(),
        ),
        (s::STORAGE, how.as_str().to_owned()),
        (s::NAME_MAX, name_max.to_string()),
    ];
    if let Some(scale) = unit_scale(file, model) {
        facts.push((s::UNIT_SCALE, scale));
    }
    Ok(LiveScene {
        colls,
        objs,
        facts,
        storage: how,
    })
}

impl LiveScene {
    /// The keys an import mints: `c1..` for the collections in order (`c0` for no collection),
    /// `o1..` for the objects.
    pub fn minted_keys(&self) -> (Vec<Key>, Vec<Key>) {
        let mut n = 0;
        let ck = self
            .colls
            .iter()
            .map(|c| match c.kind {
                CollKind::None => Key::new(NONE_COLL).expect("c0"),
                _ => {
                    n += 1;
                    coll_key(n)
                }
            })
            .collect();
        let ok = (1..=self.objs.len() as u64).map(obj_key).collect();
        (ck, ok)
    }

    /// The marks of the keys [`LiveScene::minted_keys`] gives.
    pub fn minted_marks(&self) -> (u64, u64) {
        let c = self
            .colls
            .iter()
            .filter(|c| c.kind != CollKind::None)
            .count();
        (c as u64, self.objs.len() as u64)
    }

    /// Elements and fields: what the kit's keyed limit counts.
    pub fn entries(&self) -> usize {
        let (ck, ok) = self.minted_keys();
        let (cp, op) = self.props(&ck, &ok);
        self.colls.len() + self.objs.len() + cp.len() + op.len() + self.facts.len() + 2
    }

    /// The composite maps' entries, under keys `ck` and `ok`.
    fn props(&self, ck: &[Key], ok: &[Key]) -> (BTreeMap<String, Value>, BTreeMap<String, Value>) {
        let mut cp = BTreeMap::new();
        for (c, k) in self.colls.iter().zip(ck) {
            if c.kind == CollKind::None {
                continue;
            }
            let mut put = |fld: &str, v: String| {
                cp.insert(prop(k, fld), Value::str(v));
            };
            put(f::NAME, c.name.clone());
            if !c.nid.is_empty() {
                put(f::NID, c.nid.clone());
            }
            if let Some(p) = c.parent {
                // planted: the collection named one level too high (its holder's holder)
                let p = match self.colls[p].parent {
                    Some(up) if mutant::active("CollParentWrong") => up,
                    _ => p,
                };
                put(f::PARENT, ck[p].as_str().to_owned());
            }
            put(f::HIDDEN, u8::from(c.hidden).to_string());
        }
        let mut op = BTreeMap::new();
        for (o, k) in self.objs.iter().zip(ok) {
            let mut put = |fld: &str, v: String| {
                op.insert(prop(k, fld), Value::str(v));
            };
            put(f::COLL, ck[o.coll].as_str().to_owned());
            if !o.also.is_empty() {
                let also: Vec<&str> = o.also.iter().map(|&c| ck[c].as_str()).collect();
                put(f::ALSO, also.join(","));
            }
            put(f::NAME, o.name.clone());
            if !o.nid.is_empty() {
                put(f::NID, o.nid.clone());
            }
            if !o.data.is_empty() {
                put(f::DATA, o.data.clone());
            }
            match &o.parent {
                Parent::None => {}
                Parent::Obj(p) => put(f::PARENT, ok[*p].as_str().to_owned()),
                Parent::Other(s) => put(f::PARENT, s.clone()),
            }
            if let Some(x) = &o.xform {
                put(f::XFORM, floats_text(x));
            }
            if !o.rotmode.is_empty() {
                put(f::ROTMODE, o.rotmode.clone());
            }
            if let Some(q) = &o.quat {
                put(f::QUAT, floats_text(q));
            }
            put(f::HIDDEN, u8::from(o.hide[0]).to_string());
            put(f::NOSELECT, u8::from(o.hide[1]).to_string());
            put(f::NORENDER, u8::from(o.hide[2]).to_string());
            put(f::EDIT, o.edit.as_str().to_owned());
        }
        (cp, op)
    }

    /// The kit's roots under keys `ck` (one per collection, in order) and `ok` (one per object),
    /// with the marks `marks` (`mark/c`, `mark/o`).
    pub fn to_content(&self, ck: &[Key], ok: &[Key], marks: (u64, u64)) -> Vec<(String, Neutral)> {
        let colls = self
            .colls
            .iter()
            .zip(ck)
            .map(|(c, k)| (k.clone(), Value::str(c.kind.as_str())))
            .collect();
        let objs = self
            .objs
            .iter()
            .zip(ok)
            .map(|(o, k)| (k.clone(), Value::str(o.kind.clone())))
            .collect();
        let (cp, op) = self.props(ck, ok);
        let mut sm: BTreeMap<String, Value> = self
            .facts
            .iter()
            .map(|(k, v)| ((*k).to_owned(), Value::str(v.clone())))
            .collect();
        sm.insert(s::MARK_C.to_owned(), Value::str(marks.0.to_string()));
        sm.insert(s::MARK_O.to_owned(), Value::str(marks.1.to_string()));
        vec![
            (COLLS.to_owned(), Neutral::List(colls)),
            (OBJS.to_owned(), Neutral::List(objs)),
            (COLL_PROPS.to_owned(), Neutral::Map(cp)),
            (OBJ_PROPS.to_owned(), Neutral::Map(op)),
            (SCENE.to_owned(), Neutral::Map(sm)),
        ]
    }
}
