//! The class list (the `AcDb:Classes` section): the classes a drawing's custom objects are
//! numbered by, from 500.
//!
//! The section: a 16-byte sentinel; its data's size (`RL`), for 2018, and for 2010 and later
//! with maintenance version above 3, a second `RL`; the data; a CRC-16 over the sizes and the data;
//! the sentinel's complement. The data: for 2007 and later its size in bits (`RL`, counted from
//! that field's own start) and the strings in a string stream at the data's end; then the
//! largest class number (`BS`), two `RC`s and a `B`; then, for each class: its number, its proxy
//! flags (`BS` each), its application, C++ and DXF names (`TU` from the string stream for 2007
//! and later, `TV` inline before), whether it was a zombie (`B`), its item class id (`BS`), its
//! instance count, the version and maintenance version it was added in and two more longs (`BL`
//! each).
//!
//! From the specification's chapter on the classes section, as facts; our own code.

use k2_mpc_cad_model::error::CadError;

use crate::bits::Bits;
use crate::crc::{crc16, CRC16_SEED};
use crate::error::{BAD_CONTAINER, TRUNCATED};
use crate::mutant;

const NEXT: &str = "the file is damaged: open a copy the app that made it saves again";

fn bad(why: impl Into<String>, at: usize) -> CadError {
    CadError::new(BAD_CONTAINER, why, NEXT).at(at)
}

/// A class: its number, its names, what kind of item it makes, how many instances it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Class {
    pub number: u16,
    pub proxy_flags: u16,
    pub app_name: String,
    pub cpp_name: String,
    pub dxf_name: String,
    pub zombie: bool,
    /// 0x1F2 for an entity, 0x1F3 for an object.
    pub item_class_id: u16,
    pub instances: u32,
}

fn rl(b: &[u8], at: usize) -> Result<u32, CadError> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| {
            CadError::new(
                TRUNCATED,
                "the classes section is shorter than its header",
                NEXT,
            )
            .at(at)
        })
}

/// `TV`: a length (`BS`), then that many bytes in the drawing's code page; read here as
/// Latin-1, which keeps ASCII names exact.
pub(crate) fn tv(b: &mut Bits) -> Result<String, CadError> {
    let n = usize::from(b.bs()?);
    if n * 8 > b.left() {
        return Err(CadError::new(TRUNCATED, "a text longer than its bytes", NEXT).at(b.pos() / 8));
    }
    let mut s = String::with_capacity(n);
    for _ in 0..n {
        s.push(char::from(b.rc()?));
    }
    Ok(s.trim_end_matches('\0').to_owned())
}

/// The string stream of data ending at bit `end`: a flag bit just before the end, then (going
/// back) a 15-bit size, with a second word when its top bit is set; the strings before that.
pub(crate) fn string_stream<'a>(buf: &'a [u8], end: usize) -> Result<Option<Bits<'a>>, CadError> {
    let at = |p: usize| Bits::span(buf, p, end);
    let Some(flag_at) = end.checked_sub(1) else {
        return Ok(None);
    };
    if !at(flag_at).b()? {
        return Ok(None);
    }
    let low_at = flag_at
        .checked_sub(16)
        .ok_or_else(|| bad("a string stream without its size", 0))?;
    let low = at(low_at).rs()?;
    let (size, size_at) = if low & 0x8000 != 0 {
        let high_at = low_at
            .checked_sub(16)
            .ok_or_else(|| bad("a string stream without its size", 0))?;
        let high = at(high_at).rs()?;
        (
            (usize::from(low & 0x7FFF)) | (usize::from(high) << 15),
            high_at,
        )
    } else {
        (usize::from(low), low_at)
    };
    let start = size_at
        .checked_sub(size)
        .ok_or_else(|| bad("a string stream larger than its data", 0))?;
    Ok(Some(Bits::span(buf, start, size_at)))
}

/// The classes in `section` (the `AcDb:Classes` section, decompressed), for a file of `version`
/// and `maintenance` version, its CRC-16 and closing sentinel checked.
pub fn classes(section: &[u8], version: &str, maintenance: u8) -> Result<Vec<Class>, CadError> {
    let since_2007 = matches!(version, "AC1021" | "AC1024" | "AC1027" | "AC1032");
    let since_2010 = matches!(version, "AC1024" | "AC1027" | "AC1032");
    let size = rl(section, 16)? as usize;
    let data_at: usize = if version == "AC1032" || since_2010 && maintenance > 3 {
        24
    } else {
        20
    };
    let data_end = data_at
        .checked_add(size)
        .filter(|&e| e + 2 + 16 <= section.len())
        .ok_or_else(|| {
            CadError::new(
                TRUNCATED,
                "the classes section is shorter than its size",
                NEXT,
            )
            .at(16)
        })?;
    let stored = u16::from_le_bytes([section[data_end], section[data_end + 1]]);
    let calc = crc16(CRC16_SEED, &section[16..data_end]);
    if calc != stored && !mutant::active("ClassCrcSkipped") {
        return Err(bad(
            format!("the classes section's CRC-16 is {stored:04X}, its bytes give {calc:04X}"),
            data_end,
        ));
    }
    let closing = &section[data_end + 2..data_end + 18];
    if closing.iter().zip(&section[0..16]).any(|(c, s)| *c != !*s)
        && !mutant::active("ClassCrcSkipped")
    {
        return Err(bad(
            "the classes section's closing sentinel isn't its opening one's complement",
            data_end + 2,
        ));
    }
    let (mut d, mut strings) = if since_2007 {
        let bits = rl(section, data_at)? as usize;
        let end = (data_at * 8)
            .checked_add(bits)
            .filter(|&e| e <= data_end * 8)
            .ok_or_else(|| bad("the classes data's bit size runs past its end", data_at))?;
        let strings = string_stream(section, end)?;
        (Bits::span(section, (data_at + 4) * 8, end), strings)
    } else {
        (Bits::span(section, data_at * 8, data_end * 8), None)
    };
    let largest = d.bs()?;
    d.rc()?;
    d.rc()?;
    d.b()?;
    let count = if mutant::active("ClassCountOffByOne") {
        largest.saturating_sub(500)
    } else {
        largest.saturating_sub(499)
    };
    let mut out = Vec::new();
    for _ in 0..count {
        let number = d.bs()?;
        let proxy_flags = d.bs()?;
        let mut names = [String::new(), String::new(), String::new()];
        for n in &mut names {
            *n = match strings.as_mut() {
                Some(s) if !mutant::active("ClassStringsInline") => s.tu()?,
                Some(_) => d.tu()?,
                None if since_2007 => String::new(),
                None => tv(&mut d)?,
            };
        }
        let zombie = d.b()?;
        let item_class_id = d.bs()?;
        let instances = d.bl()?;
        for _ in 0..4 {
            d.bl()?;
        }
        let [app_name, cpp_name, dxf_name] = names;
        out.push(Class {
            number,
            proxy_flags,
            app_name,
            cpp_name,
            dxf_name,
            zombie,
            item_class_id,
            instances,
        });
    }
    Ok(out)
}
