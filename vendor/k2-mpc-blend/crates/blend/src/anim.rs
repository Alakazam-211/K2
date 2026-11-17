//! What drives an object's channels ([`drives`]): the properties an action animates (the
//! object's own, the one its animation data holds aside, and each NLA strip's) and the ones a
//! driver drives, each by the property path and array index its F-curve names (`location`,
//! `2`; a visibility bit by its property, `hide_viewport`, [`VISIBILITY_PATHS`]); whether a
//! constraint acts on its transform, and the kinds of its constraints. One reading, shared: the
//! writer refuses a change to a driven channel and warns of a transform written under
//! constraints ([`mod@crate::write`]), and the live tier marks an object whose transform
//! anything drives ([`crate::live::model`]).
//!
//! **Why:** the program evaluates an object's animation when it opens a file and keeps the
//! evaluated values, so a value written to an animated channel doesn't hold: a location written
//! as (-3, 2, 1) whose z an action animates reads back with x and y kept and z re-evaluated from
//! the action, in the program's 5.2.4 and 5.3.0 alike (`PROVENANCE.md`). A channel nothing
//! animates keeps what is written.
//!
//! **Read by name through the file's own schema:**
//! - an object's `adt` (its animation data), `constraints` and `action` (an older field on the
//!   object itself);
//! - animation data's `action` and `tmpact` (the action held aside while a strip is edited),
//!   each with the slot it names (`slot_handle`, `tmp_slot_handle`) where the schema has one,
//!   and its lists `drivers` (F-curves) and `nla_tracks`;
//! - a track's `strips`, and a strip's `act` with its `action_slot_handle` and its own `strips`
//!   (a strip holding strips);
//! - an action's `curves` list, and its channel bags: `strip_keyframe_data_array` (with its
//!   `_num`), each keyframe data's `channelbag_array`, each bag's `slot_handle` and
//!   `fcurve_array`. A bag counts when its slot is the one named, or when no slot is named; the
//!   `curves` list always counts (the program's saves hold the slot's F-curves there too);
//! - an F-curve's `rna_path` (its own block, the text up to its NUL) and `array_index`;
//! - a constraint's `data` (the block of the constraint's own struct, whose name by the file's
//!   schema is the constraint's kind, `bTrackToConstraint`) and `next`.
//!
//! **The safe side:** a field the schema lacks counts as nothing; anything the reading can't
//! follow (a pointer no block has, or several, a body the reader didn't keep, a field of
//! another shape, a count out of range, an action the schema gives neither a list nor channel
//! bags, an action on the object itself, more than [`MAX_STEPS`] elements) makes
//! [`Drives::unknown`] say why, and every channel counts as driven. The constraints' kinds are
//! read on their own, with their own [`MAX_STEPS`]: a constraint the reading can't name, or a
//! list it can't follow past, is [`UNKNOWN_KIND`], and never makes a channel driven.

use std::collections::{BTreeMap, HashSet};

use crate::reader::{BlendFile, Target};

/// The most blocks one object's reading follows: past it, [`Drives::unknown`].
pub const MAX_STEPS: usize = 4096;

/// The deepest strips held in strips one reading follows: past it, [`Drives::unknown`].
pub const MAX_DEPTH: usize = 8;

/// The kind [`Drives::constraint_kinds`] gives a constraint the reading can't name: its data
/// null, a pointer no block has, a struct name that isn't a plain name, or a list it can't
/// follow past.
pub const UNKNOWN_KIND: &str = "unknown";

/// The properties of an object's visibility bits, as an F-curve names them, each with its bit
/// in the object's `restrictflag` (disabled in viewports 1, not selectable 2, disabled in
/// renders 4): animated, the bit shown is the program's evaluation, not the stored one.
pub const VISIBILITY_PATHS: &[(&str, u64)] =
    &[("hide_viewport", 1), ("hide_select", 2), ("hide_render", 4)];

/// The longest constraint kind [`Drives::constraint_kinds`] names; a longer struct name is
/// [`UNKNOWN_KIND`].
pub const KIND_MAX: usize = 64;

/// The properties of an object's transform: animated, the object's placement is the program's
/// evaluation of them, not the stored values ([`Drives::transform`]).
pub const TRANSFORM_PATHS: &[&str] = &[
    "location",
    "rotation_euler",
    "rotation_quaternion",
    "rotation_axis_angle",
    "rotation_mode",
    "scale",
    "delta_location",
    "delta_rotation_euler",
    "delta_rotation_quaternion",
    "delta_scale",
];

/// What drives an object's channels ([`drives`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Drives {
    /// Each channel animated, as its F-curve names it (`("location", 2)`), with what animates it
    /// first in the reading's order: `an action`, `an NLA strip's action` or `a driver`.
    pub channels: BTreeMap<(String, i64), &'static str>,
    /// A constraint acts on the object's transform.
    pub constraint: bool,
    /// Each kind of constraint the object has, once, in the list's order: the struct its data
    /// is, by the file's own schema (`bTrackToConstraint`), or [`UNKNOWN_KIND`] for one the
    /// reading can't name. Empty when it has none.
    pub constraint_kinds: Vec<String>,
    /// Why the reading couldn't tell (fixed text): every channel counts as driven.
    pub unknown: Option<&'static str>,
}

impl Drives {
    /// What drives component `index` of the property at `path` (`location`, `2`): what animates
    /// it ([`Drives::channels`]), or the reading's [`Drives::unknown`]; `None` when nothing does.
    /// A constraint drives no channel: it acts on the transform the channels give.
    pub fn driven(&self, path: &str, index: usize) -> Option<&'static str> {
        if let Some(why) = self.unknown {
            return Some(why);
        }
        let i = i64::try_from(index).ok()?;
        self.channels.get(&(path.to_owned(), i)).copied()
    }

    /// What drives any component of the property at `path`, as [`Drives::driven`].
    pub fn driven_any(&self, path: &str) -> Option<&'static str> {
        if let Some(why) = self.unknown {
            return Some(why);
        }
        self.channels
            .iter()
            .find(|((p, _), _)| p == path)
            .map(|(_, by)| *by)
    }

    /// Whether anything drives the object's transform: a constraint, a channel of
    /// [`TRANSFORM_PATHS`] animated, or a reading that couldn't tell.
    pub fn transform(&self) -> bool {
        self.constraint
            || self.unknown.is_some()
            || self
                .channels
                .keys()
                .any(|(p, _)| TRANSFORM_PATHS.contains(&p.as_str()))
    }
}

/// What a step of the reading gives, or why it couldn't tell.
type Step<T> = Result<T, &'static str>;

/// A pointer field: the schema lacks it (`None`), or its value (0 for null).
type Ptr = Option<u64>;

struct Walk<'f> {
    file: &'f BlendFile,
    steps: usize,
    out: Drives,
}

/// What drives the channels of the object in block `block` of `file` (the module's rules).
pub fn drives(file: &BlendFile, block: u32) -> Drives {
    let mut w = Walk {
        file,
        steps: 0,
        out: Drives::default(),
    };
    if let Err(why) = w.object(block) {
        w.out.unknown = Some(why);
    }
    if w.out.constraint {
        let mut k = Walk {
            file,
            steps: 0,
            out: Drives::default(),
        };
        let mut kinds = Vec::new();
        if k.constraints(block, &mut kinds).is_err() {
            add_kind(&mut kinds, UNKNOWN_KIND);
        }
        if kinds.is_empty() {
            kinds.push(UNKNOWN_KIND.to_owned());
        }
        w.out.constraint_kinds = kinds;
    }
    w.out
}

/// `kind` added to `kinds` unless it is there.
fn add_kind(kinds: &mut Vec<String>, kind: &str) {
    if !kinds.iter().any(|k| k == kind) {
        kinds.push(kind.to_owned());
    }
}

/// Whether `name` is a plain struct name the reading gives as a kind: 1 to [`KIND_MAX`] bytes of
/// ASCII letters, digits and `_`.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= KIND_MAX
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// The text of an unresolved pointer.
const UNRESOLVED: &str = "its animation data names a block the file doesn't hold, or several";

impl<'f> Walk<'f> {
    /// Block `b`'s struct and body, counted as a step.
    fn block(&mut self, b: u32) -> Step<(u32, &'f [u8])> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return Err("its animation data has more elements than the reading follows");
        }
        let file = self.file;
        let s = file.blocks.get(b as usize).ok_or(UNRESOLVED)?.head.sdna;
        let body = file
            .body(b)
            .ok_or("a block of its animation data wasn't kept by the reader")?;
        Ok((s, body))
    }

    /// The block pointer `p` (read from block `from`) names: `None` for null.
    fn target(&self, p: u64, from: u32) -> Step<Option<u32>> {
        match self.file.resolve(p, from) {
            Target::Null => Ok(None),
            Target::Block(b) => Ok(Some(b)),
            _ => Err(UNRESOLVED),
        }
    }

    /// The single pointer at `path` of struct `s` in `body`.
    fn ptr(&self, s: u32, body: &[u8], path: &str) -> Step<Ptr> {
        let Some((at, f)) = self.file.sdna.field_path(s, path) else {
            return Ok(None);
        };
        if f.pointer_depth != 1 || f.function_pointer || f.array_len != 1 {
            return Err("a field of its animation data has another shape");
        }
        self.file
            .endian()
            .ptr(body, at as usize)
            .map(Some)
            .ok_or("a block of its animation data ends early")
    }

    /// The pointer-to-pointers field `name` (`fcurve_array`) of struct `s` in `body`.
    fn ptr_array(&self, s: u32, body: &[u8], name: &str) -> Step<Ptr> {
        let Some(f) = self.file.sdna.field(s, name) else {
            return Ok(None);
        };
        if f.pointer_depth != 2 || f.function_pointer || f.array_len != 1 {
            return Err("a field of its animation data has another shape");
        }
        self.file
            .endian()
            .ptr(body, f.offset as usize)
            .map(Some)
            .ok_or("a block of its animation data ends early")
    }

    /// The 32-bit integer field `name` of struct `s` in `body`.
    fn int(&self, s: u32, body: &[u8], name: &str) -> Step<Option<i64>> {
        let Some(f) = self.file.sdna.field(s, name) else {
            return Ok(None);
        };
        if f.is_pointer() || f.array_len != 1 || f.size != 4 {
            return Err("a field of its animation data has another shape");
        }
        let at = f.offset as usize;
        let e = self.file.endian();
        let v = match self.file.sdna.type_name(f.type_index) {
            "int" | "int32_t" => e.i32(body, at).map(i64::from),
            "uint" | "uint32_t" => e.u32(body, at).map(i64::from),
            _ => return Err("a field of its animation data has another shape"),
        };
        v.map(Some)
            .ok_or("a block of its animation data ends early")
    }

    fn object(&mut self, block: u32) -> Step<()> {
        let (s, body) = self.block(block)?;
        if let Some(c) = self.ptr(s, body, "constraints.first")? {
            self.out.constraint = c != 0;
        }
        if self.ptr(s, body, "action")?.is_some_and(|p| p != 0) {
            return Err("an action is set on the object itself (an older field)");
        }
        let Some(adt) = self.ptr(s, body, "adt")? else {
            return Ok(());
        };
        let Some(ab) = self.target(adt, block)? else {
            return Ok(());
        };
        let (as_, abody) = self.block(ab)?;
        for (act, slot, by) in [
            ("action", "slot_handle", "an action"),
            ("tmpact", "tmp_slot_handle", "an action"),
        ] {
            if let Some(p) = self.ptr(as_, abody, act)? {
                let slot = self.int(as_, abody, slot)?;
                self.action(p, ab, slot, by)?;
            }
        }
        if let Some(first) = self.ptr(as_, abody, "drivers.first")? {
            self.fcurve_list(first, ab, "a driver")?;
        }
        if let Some(first) = self.ptr(as_, abody, "nla_tracks.first")? {
            let mut seen = HashSet::new();
            let (mut p, mut from) = (first, ab);
            while let Some(t) = self.target(p, from)? {
                if !seen.insert(t) {
                    return Err("a list of its animation data loops");
                }
                let (ts, tbody) = self.block(t)?;
                if let Some(strips) = self.ptr(ts, tbody, "strips.first")? {
                    self.strips(strips, t, 0)?;
                }
                let Some(next) = self.ptr(ts, tbody, "next")? else {
                    break;
                };
                (p, from) = (next, t);
            }
        }
        Ok(())
    }

    /// The kinds of the constraints of the object in `block`, into `kinds`: each constraint's
    /// data's struct name, or [`UNKNOWN_KIND`] for one it can't name. `Err` for a list it can't
    /// follow past.
    fn constraints(&mut self, block: u32, kinds: &mut Vec<String>) -> Step<()> {
        let (s, body) = self.block(block)?;
        let Some(first) = self.ptr(s, body, "constraints.first")? else {
            return Ok(());
        };
        let mut seen = HashSet::new();
        let (mut p, mut from) = (first, block);
        while let Some(c) = self.target(p, from)? {
            if !seen.insert(c) {
                return Err("a list of its constraints loops");
            }
            let (cs, cbody) = self.block(c)?;
            let kind = match self.ptr(cs, cbody, "data") {
                Ok(Some(d)) => match self.target(d, c) {
                    Ok(Some(db)) => self
                        .file
                        .blocks
                        .get(db as usize)
                        .and_then(|b| self.file.sdna.struct_name(b.head.sdna))
                        .filter(|n| plain_name(n)),
                    _ => None,
                },
                _ => None,
            };
            add_kind(kinds, kind.unwrap_or(UNKNOWN_KIND));
            let Some(next) = self.ptr(cs, cbody, "next")? else {
                break;
            };
            (p, from) = (next, c);
        }
        Ok(())
    }

    /// The strips from `first` (read from block `from`): each strip's action, and the strips it
    /// holds, `depth` deep.
    fn strips(&mut self, first: u64, from: u32, depth: usize) -> Step<()> {
        if depth > MAX_DEPTH {
            return Err("its NLA strips hold strips deeper than the reading follows");
        }
        let mut seen = HashSet::new();
        let (mut p, mut from) = (first, from);
        while let Some(t) = self.target(p, from)? {
            if !seen.insert(t) {
                return Err("a list of its animation data loops");
            }
            let (s, body) = self.block(t)?;
            if let Some(act) = self.ptr(s, body, "act")? {
                let slot = self.int(s, body, "action_slot_handle")?;
                self.action(act, t, slot, "an NLA strip's action")?;
            }
            if let Some(inner) = self.ptr(s, body, "strips.first")? {
                self.strips(inner, t, depth + 1)?;
            }
            let Some(next) = self.ptr(s, body, "next")? else {
                break;
            };
            (p, from) = (next, t);
        }
        Ok(())
    }

    /// The action pointer `p` names (read from block `from`): its `curves` list, and the
    /// channel bags of `slot` (every bag when `None`).
    fn action(&mut self, p: u64, from: u32, slot: Option<i64>, by: &'static str) -> Step<()> {
        let Some(a) = self.target(p, from)? else {
            return Ok(());
        };
        let (s, body) = self.block(a)?;
        let list = self.ptr(s, body, "curves.first")?;
        let kfd = self.ptr_array(s, body, "strip_keyframe_data_array")?;
        if list.is_none() && kfd.is_none() {
            return Err("an action's F-curves are in fields the reading doesn't know");
        }
        if let Some(first) = list {
            self.fcurve_list(first, a, by)?;
        }
        let Some(arr) = kfd else {
            return Ok(());
        };
        let n = self.int(s, body, "strip_keyframe_data_array_num")?;
        for (k, kfrom) in self.pointers(arr, n, a)? {
            let Some(kb) = self.target(k, kfrom)? else {
                continue;
            };
            let (ks, kbody) = self.block(kb)?;
            let Some(bags) = self.ptr_array(ks, kbody, "channelbag_array")? else {
                return Err("an action's F-curves are in fields the reading doesn't know");
            };
            let n = self.int(ks, kbody, "channelbag_array_num")?;
            for (c, cfrom) in self.pointers(bags, n, kb)? {
                let Some(cb) = self.target(c, cfrom)? else {
                    continue;
                };
                let (cs, cbody) = self.block(cb)?;
                let handle = self.int(cs, cbody, "slot_handle")?;
                if slot.is_some() && handle.is_some() && slot != handle {
                    continue;
                }
                let Some(fcs) = self.ptr_array(cs, cbody, "fcurve_array")? else {
                    return Err("an action's F-curves are in fields the reading doesn't know");
                };
                let n = self.int(cs, cbody, "fcurve_array_num")?;
                for (f, ffrom) in self.pointers(fcs, n, cb)? {
                    if let Some(fb) = self.target(f, ffrom)? {
                        self.fcurve(fb, by)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// The `n` pointers of the array pointer `arr` names (read from block `from`), each with the
    /// array's block (what it was read from). A null array holds none; a count the schema
    /// lacks, below zero or past the array's block, can't be told.
    fn pointers(&mut self, arr: u64, n: Option<i64>, from: u32) -> Step<Vec<(u64, u32)>> {
        let Some(b) = self.target(arr, from)? else {
            return Ok(Vec::new());
        };
        let n = n.ok_or("an action's array has no count the reading knows")?;
        let n = usize::try_from(n).map_err(|_| "an action's array has a count below zero")?;
        let (_, body) = self.block(b)?;
        let ps = usize::from(self.file.header.pointer_size);
        if n > MAX_STEPS || n.saturating_mul(ps) > body.len() {
            return Err("an action's array holds fewer pointers than its count");
        }
        let e = self.file.endian();
        (0..n)
            .map(|i| {
                e.ptr(body, i * ps)
                    .map(|p| (p, b))
                    .ok_or("a block of its animation data ends early")
            })
            .collect()
    }

    /// The F-curves of the list from `first` (read from block `from`).
    fn fcurve_list(&mut self, first: u64, from: u32, by: &'static str) -> Step<()> {
        let mut seen = HashSet::new();
        let (mut p, mut from) = (first, from);
        while let Some(f) = self.target(p, from)? {
            if !seen.insert(f) {
                return Err("a list of its animation data loops");
            }
            self.fcurve(f, by)?;
            let (s, body) = self.block(f)?;
            let Some(next) = self.ptr(s, body, "next")? else {
                break;
            };
            (p, from) = (next, f);
        }
        Ok(())
    }

    /// The channel the F-curve in block `f` animates: its `rna_path` and `array_index`. A null
    /// path animates nothing.
    fn fcurve(&mut self, f: u32, by: &'static str) -> Step<()> {
        let (s, body) = self.block(f)?;
        let path = self
            .ptr(s, body, "rna_path")?
            .ok_or("an F-curve has no path the reading knows")?;
        let index = self
            .int(s, body, "array_index")?
            .ok_or("an F-curve has no array index the reading knows")?;
        let Some(pb) = self.target(path, f)? else {
            return Ok(());
        };
        let (_, text) = self.block(pb)?;
        let end = text
            .iter()
            .position(|&c| c == 0)
            .ok_or("an F-curve's path holds no end")?;
        let path = std::str::from_utf8(&text[..end]).map_err(|_| "an F-curve's path isn't text")?;
        self.out
            .channels
            .entry((path.to_owned(), index))
            .or_insert(by);
        Ok(())
    }
}
