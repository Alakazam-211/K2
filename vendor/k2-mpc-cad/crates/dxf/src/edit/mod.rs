//! The edit verbs: each a pure function of one version's bytes and its arguments to a **plan**,
//! the byte ranges to replace, which the kit applies (`docs/SESSIONS.md` in the kit, "Verbs on a
//! blob doc"). Every byte outside the ranges is the input's.
//!
//! - `attrib set @h:INSERT --tag T --value V [--nth N]`: one attribute's value;
//! - `entity set @h:E… [--layer L] [--color N|bylayer|byblock] [--rgb #rrggbb]
//!   [--ltype NAME|bylayer|byblock]`: display properties;
//! - `entity move @h:E… --by dx,dy --unit U`: each point of each entity, its followers' too;
//! - `entity add line|polyline|text … --unit U`: one new entity;
//! - `entity delete @h:E…`: entities with their followers and what they own;
//! - `entity rotate @h:E… --by DEG --about x,y --unit U` and `entity scale @h:E… --factor F
//!   --about x,y --unit U`: each entity turned or scaled, its followers with it;
//! - `block insert --block NAME|@h:B --at x,y --unit U [--rotate DEG] [--scale S]
//!   [--attr TAG=VALUE…]`: a new block reference with its attributes;
//! - `text set @h:E --value S`: a single-line text's value.
//!
//! **The write model.** A plan replaces value lines (never a code line or a line ending),
//! inserts whole pairs before a pair, or removes whole pairs; new pairs are written in the file's
//! own style. Units are keyed by handle. A new handle is above every handle the file
//! owns or points at and above `$HANDSEED − 1`, and `$HANDSEED` is raised past it; nothing else
//! in the header is written (no clock, no randomness: the same call on the same bytes plans the
//! same bytes). A move adds in decimal on the file's own tokens.
//!
//! **The rules** ([`rules`]) are asked before any plan: they are the same for a person and an
//! agent. **The self-check** ([`check`]) reads the output again before the plan returns:
//! `DXF-E090` when it doesn't read back as planned, and nothing is planned.

pub mod check;
pub(crate) mod insert;
mod num;
mod ops;
pub mod rules;
mod write;
mod xform;

use std::collections::{BTreeMap, HashMap};

use k2_mpc_base::errors::Value;
use k2_mpc_cad_model::Doc;

pub use write::{apply, Splice};

use crate::error::{
    CadError, HANDLES_OFF, NOT_FOUND, UNIT_MISMATCH, USAGE, VALUE_NOT_WRITABLE, VERSION_UNSUPPORTED,
};
use crate::structure::Structure;
use crate::{convert, handle, mutant, structure, token, Limits};

/// Every edit verb, by name.
pub const NAMES: [&str; 9] = [
    "attrib set",
    "entity set",
    "entity move",
    "entity add",
    "entity delete",
    "entity rotate",
    "entity scale",
    "block insert",
    "text set",
];

/// The most entities one call may name (`DXF-E050` past it).
pub const EDIT_MAX: usize = 10_000;
/// The most vertices a new polyline may have.
pub const VERTICES_MAX: usize = 100_000;
/// The longest single-line value (a text's, an attribute's), in bytes as written.
pub const VALUE_MAX: usize = 255;
/// The most objects a delete removes with an entity (its extension dictionary and what that
/// owns).
pub const OWNED_MAX: usize = 10_000;

/// A plan, and what it is about.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Planned {
    /// The ranges, in byte order (none: no change).
    pub spans: Vec<Splice>,
    /// The output: the input with the ranges applied.
    pub output: Vec<u8>,
    /// The index keys of the entities it changes or removes.
    pub touched: Vec<String>,
    /// The index keys of the entities it adds.
    pub created: Vec<String>,
    /// The address the result is about.
    pub viewing: Option<String>,
    /// One line for a person.
    pub stdout: String,
    /// The plan as data: `cad.change/1` (the entities it changes, adds and removes), with the
    /// verb, its word, `$HANDSEED`'s new value and the plan's counts.
    pub data: Value,
    /// The verb's past-tense word.
    pub word: &'static str,
}

/// What a verb hands back besides its ranges.
#[derive(Debug, Default)]
pub(crate) struct Out {
    pub word: &'static str,
    pub stdout: String,
    pub viewing: Option<String>,
    /// Entities removed (their heads).
    pub removed: Vec<u64>,
    /// `$HANDSEED`'s new value, when raised.
    pub handseed: Option<String>,
}

/// One version, read once for a plan.
pub(crate) struct Cx<'s, 'a> {
    pub s: &'s Structure<'a>,
    pub doc: &'s Doc,
    /// Entity index by handle.
    pub by_h: HashMap<u64, usize>,
    /// Object index (in [`Structure::objects`]) by handle.
    pub objects: HashMap<u64, usize>,
    /// The highest handle owned or pointed at, `$HANDSEED − 1` included.
    pub hmax: u64,
    /// The file writes text as UTF-8 (2007 and later).
    pub utf8: bool,
    /// The file spells handles in upper case.
    pub upper: bool,
    /// The width the file right-aligns integers to, when it does.
    pub int_width: Option<usize>,
}

impl<'s, 'a> Cx<'s, 'a> {
    fn new(bytes: &[u8], s: &'s Structure<'a>, doc: &'s Doc) -> Result<Self, CadError> {
        if doc.version.as_str() < "AC1015" {
            return Err(CadError::new(
                VERSION_UNSUPPORTED,
                "the drawing is older than DXF 2000 (AC1015), which the edit verbs don't write",
                "save it as DXF 2000 or later in the app, then retry",
            ));
        }
        if doc.handles != "on" {
            return Err(CadError::new(
                HANDLES_OFF,
                "the drawing's entities don't all have handles",
                "save the drawing again in the app (it gives every object a handle)",
            ));
        }
        if let Err(why) = crate::merge::unit_map(bytes) {
            return Err(CadError::new(
                HANDLES_OFF,
                format!("the drawing can't be keyed by handle: {why}"),
                "save the drawing again in the app (it gives every object one handle)",
            ));
        }
        let by_h = doc
            .entities
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.handle.map(|h| (h, i)))
            .collect();
        let objects = object_index(s);
        let mut hmax = doc.hmax.unwrap_or(0);
        if mutant::active("HmaxOwnedOnly") {
            hmax = (0..s.tok.pairs.len())
                .filter(|&p| matches!(s.tok.pairs[p].code, 5 | 105))
                .filter_map(|p| handle::parse(s.tok.value(p)))
                .max()
                .unwrap_or(0);
        }
        if let Some(seed) = doc.handseed {
            hmax = hmax.max(seed.saturating_sub(1));
        }
        let (mut lower, mut upper) = (0u32, 0u32);
        let mut width = None;
        for p in 0..s.tok.pairs.len() {
            let c = s.tok.pairs[p].code;
            let v = s.tok.value(p);
            if c == 5 {
                lower += u32::from(v.iter().any(u8::is_ascii_lowercase));
                upper += u32::from(v.iter().any(u8::is_ascii_uppercase));
            }
            if width.is_none() && (60..80).contains(&c) && v.first() == Some(&b' ') {
                width = Some(v.len());
            }
        }
        Ok(Cx {
            s,
            doc,
            by_h,
            objects,
            hmax,
            utf8: doc.version.as_str() >= "AC1021",
            upper: upper >= lower,
            int_width: width,
        })
    }

    /// The entity `addr` names (`@h:HANDLE`): its index.
    pub fn entity(&self, addr: &str) -> Result<usize, CadError> {
        handle::from_addr(addr)
            .and_then(|h| self.by_h.get(&h).copied())
            .ok_or_else(|| {
                CadError::new(
                    NOT_FOUND,
                    format!("{addr}: no entity has that address"),
                    "`entity find` gives entities' addresses",
                )
            })
    }

    /// A handle in the file's spelling.
    pub fn hex(&self, h: u64) -> String {
        if self.upper {
            format!("{h:X}")
        } else {
            format!("{h:x}")
        }
    }

    /// An integer as the file writes integers (right-aligned to its width, when it aligns).
    pub fn int(&self, n: i64) -> Vec<u8> {
        match self.int_width {
            Some(w) => format!("{n:>w$}").into_bytes(),
            None => n.to_string().into_bytes(),
        }
    }

    /// `--unit U` against the drawing's unit.
    pub fn unit(&self, given: Option<&str>, verb: &str) -> Result<(), CadError> {
        let word = self.doc.units.word;
        let Some(g) = given else {
            return Err(CadError::new(
                USAGE,
                format!("{verb} needs --unit: the drawing's unit is {word}"),
                format!("the call with --unit {}", unit_flag(word)),
            ));
        };
        let ok = g.eq_ignore_ascii_case(word)
            || (g.eq_ignore_ascii_case("drawing") && matches!(word, "unitless" | "unknown"));
        if ok {
            Ok(())
        } else {
            Err(CadError::new(
                UNIT_MISMATCH,
                format!("--unit isn't the drawing's unit ({word})"),
                format!("the call with --unit {}", unit_flag(word)),
            ))
        }
    }
}

fn unit_flag(word: &str) -> &str {
    if matches!(word, "unitless" | "unknown") {
        "drawing"
    } else {
        word
    }
}

/// Each object of `s` (in [`Structure::objects`]) by its handle.
pub(crate) fn object_index(s: &Structure<'_>) -> HashMap<u64, usize> {
    s.objects
        .iter()
        .enumerate()
        .filter_map(|(i, u)| {
            (u.first + 1..u.end)
                .find(|&p| s.tok.pairs[p].code == 5)
                .and_then(|p| handle::parse(s.tok.value(p)))
                .map(|h| (h, i))
        })
        .collect()
}

/// Code page 1252's bytes 0x80 to 0x9F, as characters.
const CP1252_HIGH: [u16; 32] = [
    0x20AC, 0xFFFD, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0xFFFD, 0x017D, 0xFFFD, 0xFFFD, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0xFFFD, 0x017E, 0x0178,
];

/// A single-line value as the file writes text: UTF-8 from 2007 on; before, the file's code page
/// (1252 as the reader reads it; any other: ASCII), and `\U+XXXX` for a character it can't hold.
/// Refused: a line break or control character, a `\U+` the reader would take as an escape, a
/// character past U+FFFF in a file before 2007, a value over [`VALUE_MAX`] bytes as written.
pub(crate) fn encode_text(cx: &Cx<'_, '_>, v: &str) -> Result<Vec<u8>, CadError> {
    let bad = |why: &str| {
        CadError::new(
            VALUE_NOT_WRITABLE,
            format!("the value can't be written on one line here: {why}"),
            "give a value that fits",
        )
    };
    if v.chars().any(char::is_control) {
        return Err(bad("a line break or control character"));
    }
    if v.to_ascii_uppercase().contains("\\U+") {
        return Err(bad("a \\U+ sequence, which the format reads as an escape"));
    }
    let out = if cx.utf8 || mutant::active("CodepageIgnored") {
        v.as_bytes().to_vec()
    } else {
        let cp1252 = cx.doc.encoding.eq_ignore_ascii_case("ansi_1252");
        let mut o = Vec::with_capacity(v.len());
        for ch in v.chars() {
            let u = ch as u32;
            if u < 0x80 || (cp1252 && (0xA0..=0xFF).contains(&u)) {
                o.push(u as u8);
            } else if let Some(k) = CP1252_HIGH
                .iter()
                .position(|&c| cp1252 && u32::from(c) == u && c != 0xFFFD)
            {
                o.push(0x80 + k as u8);
            } else if u <= 0xFFFF {
                o.extend_from_slice(format!("\\U+{u:04X}").as_bytes());
            } else {
                return Err(bad("a character this file's code page can't name"));
            }
        }
        o
    };
    if out.len() > VALUE_MAX {
        return Err(bad("longer than 255 bytes"));
    }
    Ok(out)
}

/// Plans edit verb `verb` on `bytes` with `args` (the words after the verb).
pub fn plan(bytes: &[u8], verb: &str, args: &[&str]) -> Result<Planned, CadError> {
    if !NAMES.contains(&verb) {
        return Err(CadError::new(
            USAGE,
            format!("no edit verb {verb:?}"),
            format!("the edit verbs: {}", NAMES.join(", ")),
        ));
    }
    let limits = Limits::default();
    let t = token::tokenize(bytes, &limits)?;
    let s = structure::walk(t, &limits)?;
    let doc = convert::doc(&s);
    let cx = Cx::new(bytes, &s, &doc)?;
    let mut w = write::Writer::new(&s);
    let mut it = check::Intent {
        hmax: cx.hmax,
        ..check::Intent::default()
    };
    let mut out = Out::default();
    match verb {
        "attrib set" => ops::attrib_set(&cx, &mut w, &mut it, &mut out, args)?,
        "entity set" => ops::entity_set(&cx, &mut w, &mut it, &mut out, args)?,
        "entity move" => ops::entity_move(&cx, &mut w, &mut it, &mut out, args)?,
        "entity add" => ops::entity_add(&cx, &mut w, &mut it, &mut out, args)?,
        "entity rotate" => xform::entity_rotate(&cx, &mut w, &mut it, &mut out, args)?,
        "entity scale" => xform::entity_scale(&cx, &mut w, &mut it, &mut out, args)?,
        "block insert" => insert::block_insert(&cx, &mut w, &mut it, &mut out, args)?,
        "text set" => ops::text_set(&cx, &mut w, &mut it, &mut out, args)?,
        _ => ops::entity_delete(&cx, &mut w, &mut it, &mut out, args)?,
    }
    let (inserted, removed) = (w.inserted, w.removed);
    let spans = w.finish()?;
    let output = apply(bytes, &spans);
    if !spans.is_empty() {
        check::self_check(bytes, &output, &it, inserted, removed)?;
    }
    let touched: Vec<String> = it
        .changed
        .iter()
        .chain(&out.removed)
        .map(|&h| handle::key(h))
        .collect();
    let created: Vec<String> = it.created.iter().map(|&h| handle::key(h)).collect();
    let after_doc;
    let after = if spans.is_empty() {
        &doc
    } else {
        let t = token::tokenize(&output, &limits)?;
        let s2 = structure::walk(t, &limits)?;
        after_doc = convert::doc(&s2);
        &after_doc
    };
    let changed: Vec<u64> = it.changed.iter().copied().collect();
    let added: Vec<u64> = it.created.iter().copied().collect();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for sp in &spans {
        *counts
            .entry(if sp.lo == sp.hi {
                "inserts"
            } else if sp.with.is_empty() {
                "removals"
            } else {
                "replacements"
            })
            .or_default() += 1;
    }
    let mut data =
        match k2_mpc_cad_model::change::of_edit(&doc, after, &changed, &added, &out.removed) {
            Value::Obj(kv) => kv,
            _ => Vec::new(),
        };
    data.push(("verb".into(), Value::from(verb)));
    data.push(("word".into(), Value::from(out.word)));
    data.push((
        "handseed".into(),
        out.handseed.clone().map_or(Value::Null, Value::from),
    ));
    data.push((
        "plan".into(),
        Value::obj([
            ("spans", Value::from(spans.len())),
            (
                "replacements",
                Value::from(counts.get("replacements").copied().unwrap_or(0)),
            ),
            (
                "inserts",
                Value::from(counts.get("inserts").copied().unwrap_or(0)),
            ),
            (
                "removals",
                Value::from(counts.get("removals").copied().unwrap_or(0)),
            ),
            ("pairs_inserted", Value::from(inserted)),
            ("pairs_removed", Value::from(removed)),
        ]),
    ));
    let data = Value::Obj(data);
    let stdout = if spans.is_empty() {
        format!("no change: {}\n", out.stdout)
    } else {
        format!("{}\n", out.stdout)
    };
    Ok(Planned {
        spans,
        output,
        touched,
        created,
        viewing: out.viewing,
        stdout,
        data,
        word: out.word,
    })
}

/// The verb's past-tense word.
pub fn word(verb: &str) -> &'static str {
    match verb {
        "attrib set" => "set",
        "entity set" => "changed",
        "entity move" => "moved",
        "entity add" => "added",
        "entity rotate" => "rotated",
        "entity scale" => "scaled",
        "block insert" => "inserted",
        "text set" => "edited",
        _ => "deleted",
    }
}
