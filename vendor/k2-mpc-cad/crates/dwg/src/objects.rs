//! The object map (the `AcDb:Handles` section) and each object's head in the
//! `AcDb:AcDbObjects` section:
//!
//! - the map is blocks, each a big-endian size (counting itself), then pairs of a handle delta
//!   (`UMC`) and a location delta (`MC`), both starting from 0 in each block, then a big-endian
//!   CRC-16 over the block; a block of size 2 ends the map;
//! - an object at its location: its size (`MS`), for 2010 and later its handle stream's size in
//!   bits (`UMC`), then its bits: its type (`OT` for 2010 and later, `BS` before), for 2004 its
//!   size in bits (`RL`), its own handle (`H`); then, after its size, a CRC-16 over the object
//!   from its location.
//!
//! From the specification's chapters on the object map and on the objects' common data, as
//! facts; our own code.

use k2_mpc_cad_model::error::CadError;

use crate::bits::Bits;
use crate::crc::{crc16, CRC16_SEED};
use crate::error::{BAD_CONTAINER, OVER_BOUND, TRUNCATED};
use crate::{mutant, Limits};

const NEXT: &str = "the file is damaged: open a copy the app that made it saves again";

fn bad(why: impl Into<String>, at: usize) -> CadError {
    CadError::new(BAD_CONTAINER, why, NEXT).at(at)
}

fn short(what: &str, at: usize) -> CadError {
    CadError::new(
        TRUNCATED,
        format!("{what} runs past the end of its section"),
        NEXT,
    )
    .at(at)
}

/// An object in the map: its handle and its location in the objects section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapEntry {
    pub handle: u64,
    pub location: u64,
}

/// The object map in `section` (the `AcDb:Handles` section, decompressed), each block's CRC-16
/// checked. A map with more entries than [`Limits::max_objects`] is refused as it reaches the
/// limit, before its entries grow the heap any further.
pub fn object_map(section: &[u8], limits: &Limits) -> Result<Vec<MapEntry>, CadError> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let (mut handle, mut location) = (0u64, 0i64);
    loop {
        let head = section
            .get(i..i + 2)
            .ok_or_else(|| short("the object map", i))?;
        let size = usize::from(u16::from_be_bytes([head[0], head[1]]));
        if size == 2 {
            return Ok(out);
        }
        if size < 2 {
            return Err(bad(format!("an object map block of size {size}"), i));
        }
        let end = i + size;
        let block = section
            .get(i..end)
            .ok_or_else(|| short("an object map block", i))?;
        let stored = section
            .get(end..end + 2)
            .ok_or_else(|| short("an object map block's CRC", end))?;
        let stored = u16::from_be_bytes([stored[0], stored[1]]);
        let calc = crc16(CRC16_SEED, block);
        if calc != stored && !mutant::active("ObjectMapCrcSkipped") {
            return Err(bad(
                format!("an object map block's CRC-16 is {stored:04X}, its bytes give {calc:04X}"),
                i,
            ));
        }
        if !mutant::active("MapDeltasNotReset") {
            handle = 0;
            location = 0;
        }
        let mut b = Bits::new(&block[2..]);
        while b.left() > 0 {
            let dh = b.umc()?;
            let dl = b.mc()?;
            handle = handle
                .checked_add(dh)
                .ok_or_else(|| bad("the object map's handles overflow", i))?;
            location = location
                .checked_add(dl)
                .filter(|l| *l >= 0)
                .ok_or_else(|| bad("an object map location before the section's start", i))?;
            if out.len() >= limits.max_objects && !mutant::active("MapEntriesUnbounded") {
                return Err(CadError::new(
                    OVER_BOUND,
                    format!(
                        "the object map lists more than {} objects, the limit",
                        limits.max_objects
                    ),
                    "raise the object limit for a file you trust",
                )
                .at(i));
            }
            out.push(MapEntry {
                handle,
                location: location as u64,
            });
        }
        i = end + 2;
    }
}

/// An object's head: where it is, its size, its type, its own handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectHead {
    pub location: u64,
    /// Its data's size in bytes (after the size fields, before the CRC).
    pub size: u64,
    pub kind: u16,
    pub handle: u64,
}

/// Whether `version` writes objects the 2010 way (a handle stream size, an `OT` type).
fn since_2010(version: &str) -> bool {
    !mutant::active("VersionLayoutIgnored") && matches!(version, "AC1024" | "AC1027" | "AC1032")
        || mutant::active("VersionLayoutIgnored")
}

/// The head of the object at `location` in `objects` (the `AcDb:AcDbObjects` section,
/// decompressed), for a file of `version`, its CRC-16 checked.
pub fn object_head(objects: &[u8], location: u64, version: &str) -> Result<ObjectHead, CadError> {
    let at = usize::try_from(location)
        .ok()
        .filter(|&a| a < objects.len())
        .ok_or_else(|| short("an object's location", objects.len()))?;
    let mut b = Bits::span(objects, at * 8, objects.len() * 8);
    let size = b.ms()?;
    let modern = since_2010(version);
    if modern {
        b.umc()?;
    }
    let start = b.pos() / 8;
    let size_bytes = usize::try_from(size).map_err(|_| bad("an object's size overflows", at))?;
    let end = start
        .checked_add(size_bytes)
        .filter(|&e| e + 2 <= objects.len())
        .ok_or_else(|| short("an object", at))?;
    let stored = u16::from_le_bytes([objects[end], objects[end + 1]]);
    let calc = crc16(CRC16_SEED, &objects[at..end]);
    if calc != stored && !mutant::active("ObjectCrcSkipped") {
        return Err(bad(
            format!("the object at {at}: its CRC-16 is {stored:04X}, its bytes give {calc:04X}"),
            at,
        ));
    }
    let mut d = Bits::span(objects, start * 8, end * 8);
    let kind = if modern { d.ot()? } else { d.bs()? };
    if !modern {
        d.rl()?;
    }
    let handle = d.h()?.value;
    Ok(ObjectHead {
        location,
        size,
        kind,
        handle,
    })
}
