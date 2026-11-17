//! One object's streams and the data every object and every entity begins with.
//!
//! An object's bits after its size fields are three streams:
//!
//! - **the data**: its type, its own handle, its extension data (`EED`), then, for an entity,
//!   the graphic flag (and the graphic, skipped), the entity mode, the reactor count, the
//!   extension dictionary and data-store flags, the colour (`ENC`), the
//!   linetype scale, the linetype, plot style and material flags, the shadow flags, the visual
//!   style flags, the invisibility and the lineweight; then the kind's own fields;
//! - **the strings** (2007 and later): every text field of the object, read in the order the
//!   data names them, from a stream at the data's end (its size and a flag bit last);
//! - **the handles**: the owner (an entity's only when its entity mode is 0), the reactors, the
//!   extension dictionary, then for an entity its colour book colour, layer, linetype,
//!   material, plot style and visual styles, then the kind's own.
//!
//! The data's end is known from the head (the handle stream's size in bits for 2010 and later,
//! the data's size in bits for 2004): a kind's read must end exactly there ([`Obj::finish`]),
//! so a field read out of place is refused, never passed on as a value.
//!
//! From the specification's chapters on the objects' and entities' common data, their handle
//! data and the extension data, as facts; our own code.

use k2_mpc_cad_model::error::CadError;

use crate::bits::{Bits, Colour, Handle};
use crate::classes::{string_stream, tv};
use crate::crc::{crc16, CRC16_SEED};
use crate::error::{BAD_CONTAINER, BAD_VALUE, TRUNCATED, UNSUPPORTED_VERSION};
use crate::mutant;

const NEXT: &str = "the file is damaged: open a copy the app that made it saves again";

/// The versions this reader reads, oldest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ver {
    R2004,
    R2010,
    R2013,
    R2018,
}

impl Ver {
    /// The version a stamp names, within the family this reader reads.
    pub fn of(stamp: &str) -> Result<Ver, CadError> {
        Ok(match stamp {
            "AC1018" => Ver::R2004,
            "AC1024" => Ver::R2010,
            "AC1027" => Ver::R2013,
            "AC1032" => Ver::R2018,
            _ => {
                return Err(CadError::new(
                    UNSUPPORTED_VERSION,
                    "a DWG version outside the 2004 to 2018 family",
                    "save the drawing as a 2004 to 2018 DWG file",
                ))
            }
        })
    }

    /// Texts in a string stream (2007 and later).
    pub fn strings(self) -> bool {
        self >= Ver::R2010
    }
}

/// One application's extension data: its application (an `APPID` handle) and its items.
#[derive(Debug, Clone, PartialEq)]
pub struct Eed {
    pub app: u64,
    pub items: Vec<XItem>,
}

/// One extension data item, by its group code (1000 and up).
#[derive(Debug, Clone, PartialEq)]
pub enum XItem {
    /// 1000.
    Str(String),
    /// 1002: `true` opens a group, `false` closes it.
    Brace(bool),
    /// 1003 (a layer) and 1005 (an entity): a handle.
    Handle(i32, u64),
    /// 1004.
    Bytes(Vec<u8>),
    /// 1010 to 1013.
    Point(i32, [f64; 3]),
    /// 1040 to 1042.
    Real(i32, f64),
    /// 1070.
    Short(i16),
    /// 1071.
    Long(i32),
}

/// An object opened for its kind's read: its head, its extension data and its streams.
#[derive(Debug, Clone)]
pub struct Obj<'a> {
    pub ver: Ver,
    pub kind: u16,
    pub handle: u64,
    pub eed: Vec<Eed>,
    /// The data stream, from after the extension data.
    pub d: Bits<'a>,
    /// The string stream, when the object has one (2007 and later).
    strings: Option<Bits<'a>>,
    /// The handle stream.
    pub hs: Bits<'a>,
    /// Where the data ends: the string stream's start, else its flag bit (2007 and later); the
    /// handle stream's start (2004).
    pub data_end: usize,
}

fn bad(why: impl Into<String>, at: usize) -> CadError {
    CadError::new(BAD_VALUE, why, NEXT).at(at)
}

/// The Windows code page 1252 characters at 0x80 to 0x9F (the rest is Latin-1).
const CP1252_HIGH: [u16; 32] = [
    0x20AC, 0xFFFD, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0xFFFD, 0x017D, 0xFFFD, 0xFFFD, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0xFFFD, 0x017E, 0x0178,
];

/// A 2004 text's bytes as characters: the code page 1252 table (the samples' and the most
/// common code page), which keeps ASCII exact.
pub fn cp1252(s: &str) -> String {
    s.chars()
        .map(|c| match u32::from(c) {
            x @ 0x80..=0x9F => {
                char::from_u32(u32::from(CP1252_HIGH[(x - 0x80) as usize])).unwrap_or('\u{FFFD}')
            }
            _ => c,
        })
        .collect()
}

impl<'a> Obj<'a> {
    /// A text field (`TV`): from the string stream (2007 and later; empty when the object has
    /// none), else inline.
    pub fn tv(&mut self) -> Result<String, CadError> {
        if self.ver.strings() {
            match self.strings.as_mut() {
                Some(s) => s.tu(),
                None => Ok(String::new()),
            }
        } else {
            Ok(cp1252(&tv(&mut self.d)?))
        }
    }

    /// A handle reference from the handle stream, resolved against the object's own handle:
    /// `None` for a null reference.
    pub fn h(&mut self) -> Result<Option<u64>, CadError> {
        let r: Handle = self.hs.h()?;
        if r.value == 0 && matches!(r.code, 0..=5) {
            return Ok(None);
        }
        Ok(r.resolve(self.handle).filter(|&h| h != 0))
    }

    /// An object's colour (`CMC`, 2004 and later): its index, its true colour and flags, and
    /// its colour and book names when the flags say they follow.
    pub fn cmc(&mut self) -> Result<Colour, CadError> {
        let index = self.d.bs()?;
        let rgb = self.d.bl()?;
        let flags = self.d.rc()?;
        if flags & 1 != 0 {
            self.tv()?;
        }
        if flags & 2 != 0 {
            self.tv()?;
        }
        Ok(Colour {
            index: if rgb >> 24 == 0xC3 {
                (rgb & 0xFF) as u16
            } else {
                index
            },
            rgb: (rgb >> 24 == 0xC2).then_some(rgb & 0x00FF_FFFF),
            book: flags & 2 != 0,
            transparency: None,
        })
    }

    /// Checks the kind's read ended exactly at the data's end.
    pub fn finish(&self) -> Result<(), CadError> {
        let at = self.d.pos();
        if at != self.data_end && !mutant::active("DataEndUnchecked") {
            return Err(bad(
                format!(
                    "an object of type {} read to bit {at} of data ending at bit {}",
                    self.kind, self.data_end
                ),
                at / 8,
            ));
        }
        Ok(())
    }

    /// Bits between where the read stands and the data's end (negative: past it).
    pub fn gap(&self) -> i64 {
        self.data_end as i64 - self.d.pos() as i64
    }
}

/// `n` bytes of `b` from `*i`, moving `*i` past them.
fn take<'b>(b: &'b [u8], i: &mut usize, n: usize) -> Option<&'b [u8]> {
    let s = b.get(*i..i.checked_add(n)?)?;
    *i += n;
    Some(s)
}

fn f64_at(s: &[u8]) -> f64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&s[..8]);
    f64::from_le_bytes(a)
}

/// One extension data item of code `code` (its group code less 1000) at `*i`.
fn eed_item(b: &[u8], i: &mut usize, code: u8, ver: Ver) -> Option<XItem> {
    let g = 1000 + i32::from(code);
    Some(match code {
        0 if ver.strings() => {
            let n = take(b, i, 2)?;
            let n = usize::from(u16::from_le_bytes([n[0], n[1]]));
            let u = take(b, i, n.checked_mul(2)?)?;
            let units: Vec<u16> = u
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            XItem::Str(String::from_utf16_lossy(&units))
        }
        0 => {
            let n = usize::from(take(b, i, 1)?[0]);
            take(b, i, 2)?;
            let s = take(b, i, n)?;
            XItem::Str(cp1252(
                &s.iter().map(|&c| char::from(c)).collect::<String>(),
            ))
        }
        2 => XItem::Brace(take(b, i, 1)?[0] == 0),
        3 | 5 => {
            let mut h = [0u8; 8];
            h.copy_from_slice(take(b, i, 8)?);
            XItem::Handle(g, u64::from_be_bytes(h))
        }
        4 => {
            let n = usize::from(take(b, i, 1)?[0]);
            XItem::Bytes(take(b, i, n)?.to_vec())
        }
        10..=13 => {
            let s = take(b, i, 24)?;
            XItem::Point(g, [f64_at(&s[0..8]), f64_at(&s[8..16]), f64_at(&s[16..24])])
        }
        40..=42 => XItem::Real(g, f64_at(take(b, i, 8)?)),
        70 => {
            let s = take(b, i, 2)?;
            XItem::Short(i16::from_le_bytes([s[0], s[1]]))
        }
        71 => {
            let s = take(b, i, 4)?;
            XItem::Long(i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        }
        _ => return None,
    })
}

/// The extension data items of one application's bytes. An item the table doesn't name, or
/// one cut short, ends the list (the bytes from it on are kept as one 1004 item).
fn eed_items(b: &[u8], ver: Ver) -> Vec<XItem> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        let at = i;
        let code = b[i];
        i += 1;
        match eed_item(b, &mut i, code, ver) {
            Some(x) => out.push(x),
            None => {
                out.push(XItem::Bytes(b[at..].to_vec()));
                break;
            }
        }
    }
    out
}

/// Opens the object at `location` in `objects` (the `AcDb:AcDbObjects` section, decompressed)
/// for a file of `ver`: its head read, its CRC-16 checked, its streams found, its extension
/// data read. The data stream stands after the extension data.
pub fn open(objects: &[u8], location: u64, ver: Ver) -> Result<Obj<'_>, CadError> {
    let short = |what: &str, at: usize| {
        CadError::new(
            TRUNCATED,
            format!("{what} runs past the end of its section"),
            NEXT,
        )
        .at(at)
    };
    let at = usize::try_from(location)
        .ok()
        .filter(|&a| a < objects.len())
        .ok_or_else(|| short("an object's location", objects.len()))?;
    let mut b = Bits::span(objects, at * 8, objects.len() * 8);
    let size = usize::try_from(b.ms()?).map_err(|_| bad("an object's size overflows", at))?;
    let modern = ver >= Ver::R2010;
    let handle_bits = if modern {
        usize::try_from(b.umc()?).map_err(|_| bad("a handle stream's size overflows", at))?
    } else {
        0
    };
    let start = b.pos() / 8;
    let end = start
        .checked_add(size)
        .filter(|&e| e + 2 <= objects.len())
        .ok_or_else(|| short("an object", at))?;
    let stored = u16::from_le_bytes([objects[end], objects[end + 1]]);
    let calc = crc16(CRC16_SEED, &objects[at..end]);
    if calc != stored && !mutant::active("ObjectCrcSkipped") {
        return Err(CadError::new(
            BAD_CONTAINER,
            format!("the object at {at}: its CRC-16 is {stored:04X}, its bytes give {calc:04X}"),
            NEXT,
        )
        .at(at));
    }
    let mut d = Bits::span(objects, start * 8, end * 8);
    let (kind, handles_at) = if modern {
        let kind = d.ot()?;
        let h = (end * 8)
            .checked_sub(handle_bits)
            .filter(|&h| h >= start * 8)
            .ok_or_else(|| bad("a handle stream larger than its object", at))?;
        (kind, h)
    } else {
        let kind = d.bs()?;
        let bits = d.rl()? as usize;
        let h = (start * 8)
            .checked_add(bits)
            .filter(|&h| h <= end * 8)
            .ok_or_else(|| bad("an object's data size past its end", at))?;
        (kind, h)
    };
    let handle = d.h()?.value;
    // the data stream ends where the handles begin
    let mut d = Bits::span(objects, d.pos(), handles_at);
    let (strings, data_end) = if ver.strings() {
        let s = string_stream(objects, handles_at)?;
        let data_end = match &s {
            Some(s) => s.pos(),
            None => handles_at.saturating_sub(1),
        };
        if data_end < d.pos() {
            return Err(bad("a string stream reaching into the object's head", at));
        }
        (s, data_end)
    } else {
        (None, handles_at)
    };
    let mut eed = Vec::new();
    loop {
        let n = usize::from(d.bs()?);
        if n == 0 {
            break;
        }
        let app = d.h()?.value;
        if n * 8 > d.left() {
            return Err(short("an object's extension data", d.pos() / 8));
        }
        let mut raw = Vec::with_capacity(n);
        for _ in 0..n {
            raw.push(d.rc()?);
        }
        eed.push(Eed {
            app,
            items: eed_items(&raw, ver),
        });
    }
    Ok(Obj {
        ver,
        kind,
        handle,
        eed,
        d,
        strings,
        hs: Bits::span(objects, handles_at, end * 8),
        data_end,
    })
}

/// What every object (not an entity) has after its extension data: its reactor count and
/// flags, and from the handle stream its owner, reactors and extension dictionary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObjCommon {
    pub owner: Option<u64>,
    pub reactors: Vec<u64>,
    pub xdic: Option<u64>,
}

/// Reads an object's common data and its common handles.
pub fn object_common(o: &mut Obj<'_>) -> Result<ObjCommon, CadError> {
    let n = o.d.bl()? as usize;
    let xdic_missing = o.d.b()?;
    if o.ver >= Ver::R2013 {
        o.d.b()?;
    }
    let owner = o.h()?;
    let reactors = reactors(o, n)?;
    let xdic = if xdic_missing { None } else { o.h()? };
    Ok(ObjCommon {
        owner,
        reactors,
        xdic,
    })
}

/// `n` reactor handles, refused when the handle stream can't hold them (a handle is 8 bits at
/// least) before anything is allocated.
fn reactors(o: &mut Obj<'_>, n: usize) -> Result<Vec<u64>, CadError> {
    if n.saturating_mul(8) > o.hs.left() {
        return Err(bad(
            "more reactors than the handle stream holds",
            o.hs.pos() / 8,
        ));
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        if let Some(h) = o.h()? {
            out.push(h);
        }
    }
    Ok(out)
}

/// What every entity has before its kind's fields, and its common handles.
#[derive(Debug, Clone, PartialEq)]
pub struct EntCommon {
    /// 0: the owner is in the handles; 1: paper space; 2: model space.
    pub entmode: u8,
    pub colour: Colour,
    pub lt_scale: f64,
    /// 0 by layer, 1 by block, 2 continuous, 3 a linetype handle.
    pub lt_flags: u8,
    pub ps_flags: u8,
    pub mat_flags: u8,
    pub invisible: bool,
    /// The lineweight's index ([`lineweight`]).
    pub lineweight: u8,
    pub owner: Option<u64>,
    pub reactors: Vec<u64>,
    pub xdic: Option<u64>,
    pub book: Option<u64>,
    pub layer: Option<u64>,
    pub ltype: Option<u64>,
    pub plotstyle: Option<u64>,
    pub material: Option<u64>,
}

/// A lineweight's index as hundredths of a millimetre, `-1` by layer, `-2` by block, `-3` the
/// default; `None` for an index no writer uses.
pub fn lineweight(index: u8) -> Option<i64> {
    const WEIGHTS: [i64; 24] = [
        0, 5, 9, 13, 15, 18, 20, 25, 30, 35, 40, 50, 53, 60, 70, 80, 90, 100, 106, 120, 140, 158,
        200, 211,
    ];
    match index {
        0..=23 => Some(WEIGHTS[usize::from(index)]),
        29 => Some(-1),
        30 => Some(-2),
        31 => Some(-3),
        _ => None,
    }
}

/// Reads an entity's common data and its common handles: the data stream then stands at the
/// kind's first field, the handle stream at the kind's first handle.
pub fn entity_common(o: &mut Obj<'_>) -> Result<EntCommon, CadError> {
    if o.d.b()? {
        // the graphic: its size, then its bytes, skipped
        let n = if o.ver >= Ver::R2010 && mutant::active("BllAsThreeB") {
            let k = o.d.three_b()?;
            let mut v = 0u64;
            for i in 0..k {
                v |= u64::from(o.d.rc()?) << (8 * i);
            }
            v
        } else if o.ver >= Ver::R2010 {
            o.d.bll()?
        } else {
            u64::from(o.d.rl()?)
        };
        let bits = usize::try_from(n)
            .ok()
            .and_then(|n| n.checked_mul(8))
            .ok_or_else(|| bad("a graphic larger than its object", o.d.pos() / 8))?;
        o.d.skip(bits)?;
    }
    let entmode = o.d.bb()?;
    let n = o.d.bl()? as usize;
    let xdic_missing = o.d.b()?;
    if o.ver >= Ver::R2013 {
        o.d.b()?;
    }
    // the specification lists a links flag here, "always 1 from 2004"; the 2004 to 2018
    // samples write none (every entity's read ends at its data's end only without it)
    if mutant::active("LinksBitRead") {
        o.d.b()?;
    }
    let colour = o.d.cmc()?;
    let lt_scale = o.d.bd()?;
    let lt_flags = o.d.bb()?;
    let ps_flags = o.d.bb()?;
    let mut mat_flags = 0;
    let mut styles = [false; 3];
    if o.ver.strings() {
        mat_flags = o.d.bb()?;
        o.d.rc()?;
    }
    if o.ver >= Ver::R2010 {
        for s in &mut styles {
            *s = o.d.b()?;
        }
    }
    let invisible = o.d.bs()? & 1 != 0;
    let lineweight = o.d.rc()?;
    // the handles
    let owner = if entmode == 0 { o.h()? } else { None };
    let reactors = reactors(o, n)?;
    let xdic = if xdic_missing { None } else { o.h()? };
    let book = if colour.book { o.h()? } else { None };
    let (layer, ltype) = if mutant::active("LtypeBeforeLayer") && lt_flags == 3 {
        let lt = o.h()?;
        (o.h()?, lt)
    } else {
        let layer = o.h()?;
        (layer, if lt_flags == 3 { o.h()? } else { None })
    };
    let material = if mat_flags == 3 { o.h()? } else { None };
    let plotstyle = if ps_flags == 3 { o.h()? } else { None };
    for s in styles {
        if s {
            o.h()?;
        }
    }
    Ok(EntCommon {
        entmode,
        colour,
        lt_scale,
        lt_flags,
        ps_flags,
        mat_flags,
        invisible,
        lineweight,
        owner,
        reactors,
        xdic,
        book,
        layer,
        ltype,
        plotstyle,
        material,
    })
}
