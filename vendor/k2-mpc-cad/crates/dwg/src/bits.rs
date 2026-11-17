//! The bit-coded values a DWG file's sections are written in, read most significant bit first
//! from a bit stream with an end: a read past the end is refused, typed
//! ([`crate::error::TRUNCATED`]), never filled in.
//!
//! From the specification's chapter on bit codes and data definitions; our own code.

use k2_mpc_cad_model::error::CadError;

use crate::error::{BAD_VALUE, TRUNCATED};
use crate::mutant;

/// A handle reference: its code and its value, as written ([`Handle::resolve`] gives the handle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle {
    /// The reference's kind: 2 to 5 absolute (an owner or a pointer, soft or hard); 6, 8, 0xA
    /// and 0xC relative to the referring object's own handle.
    pub code: u8,
    pub value: u64,
}

impl Handle {
    /// The handle referred to, from an object whose own handle is `own`: `None` for a code
    /// with no rule, or a relative reference outside the handles' range.
    pub fn resolve(&self, own: u64) -> Option<u64> {
        if mutant::active("HandleNotRelative") && matches!(self.code, 0xA | 0xC) {
            return Some(self.value);
        }
        match self.code {
            2..=5 => Some(self.value),
            6 => own.checked_add(1),
            8 => own.checked_sub(1),
            0xA => own.checked_add(self.value),
            0xC => own.checked_sub(self.value),
            _ => None,
        }
    }
}

/// An entity's colour, as written: an index, and what the flags say follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    /// The colour index (the flags taken out).
    pub index: u16,
    /// A true colour, when the flags say one follows.
    pub rgb: Option<u32>,
    /// A colour-book reference sits in the handle stream.
    pub book: bool,
    /// A transparency, when the flags say one follows.
    pub transparency: Option<u32>,
}

/// A bit stream over a byte slice, from a start bit to an end bit.
#[derive(Debug, Clone)]
pub struct Bits<'a> {
    buf: &'a [u8],
    pos: usize,
    end: usize,
}

impl<'a> Bits<'a> {
    /// The whole of `buf`.
    pub fn new(buf: &'a [u8]) -> Self {
        Bits {
            buf,
            pos: 0,
            end: buf.len() * 8,
        }
    }

    /// `buf` from bit `start` to bit `end`, each held within the buffer.
    pub fn span(buf: &'a [u8], start: usize, end: usize) -> Self {
        let all = buf.len() * 8;
        let end = end.min(all);
        Bits {
            buf,
            pos: start.min(end),
            end,
        }
    }

    /// The next bit's place.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// The bits left before the end.
    pub fn left(&self) -> usize {
        self.end - self.pos
    }

    fn short(&self, n: usize) -> CadError {
        CadError::new(
            TRUNCATED,
            format!("{n} bits wanted, {} left", self.left()),
            "the file is cut short or damaged: open a copy the app that made it saves again",
        )
        .at(self.pos / 8)
    }

    fn bad(&self, why: String) -> CadError {
        CadError::new(
            BAD_VALUE,
            why,
            "the file is damaged: open a copy the app that made it saves again",
        )
        .at(self.pos / 8)
    }

    /// `n` bits (at most 64), the first the most significant.
    pub fn bits(&mut self, n: u32) -> Result<u64, CadError> {
        let n = n.min(64) as usize;
        if n > self.left() {
            if mutant::active("ReadPastEndZeroFills") {
                self.pos = self.end;
                return Ok(0);
            }
            return Err(self.short(n));
        }
        let mut v = 0u64;
        for _ in 0..n {
            let byte = self.buf[self.pos >> 3];
            v = (v << 1) | u64::from((byte >> (7 - (self.pos & 7))) & 1);
            self.pos += 1;
        }
        Ok(v)
    }

    /// `B`: one bit.
    pub fn b(&mut self) -> Result<bool, CadError> {
        Ok(self.bits(1)? == 1)
    }

    /// `BB`: two bits.
    pub fn bb(&mut self) -> Result<u8, CadError> {
        Ok(self.bits(2)? as u8)
    }

    /// `RC`: a byte.
    pub fn rc(&mut self) -> Result<u8, CadError> {
        Ok(self.bits(8)? as u8)
    }

    /// `RS`: two bytes, little-endian.
    pub fn rs(&mut self) -> Result<u16, CadError> {
        let lo = u16::from(self.rc()?);
        Ok(lo | (u16::from(self.rc()?) << 8))
    }

    /// `RL`: four bytes, little-endian.
    pub fn rl(&mut self) -> Result<u32, CadError> {
        let lo = u32::from(self.rs()?);
        Ok(lo | (u32::from(self.rs()?) << 16))
    }

    /// `RD`: an IEEE double, eight bytes little-endian.
    pub fn rd(&mut self) -> Result<f64, CadError> {
        let mut b = [0u8; 8];
        for x in &mut b {
            *x = self.rc()?;
        }
        Ok(f64::from_le_bytes(b))
    }

    /// `BS`: a two-bit code, then a short (`00`), a byte (`01`), nothing for 0 (`10`) or for 256
    /// (`11`).
    pub fn bs(&mut self) -> Result<u16, CadError> {
        let mut code = self.bb()?;
        if mutant::active("BsCodesSwapped") && code < 2 {
            code ^= 1;
        }
        match code {
            0 => self.rs(),
            1 => Ok(u16::from(self.rc()?)),
            2 => Ok(0),
            _ => Ok(256),
        }
    }

    /// `BL`: a two-bit code, then a long (`00`), a byte (`01`), nothing for 0 (`10`); `11` is
    /// refused.
    pub fn bl(&mut self) -> Result<u32, CadError> {
        match self.bb()? {
            0 => self.rl(),
            1 => Ok(u32::from(self.rc()?)),
            2 => Ok(0),
            _ => Err(self.bad("a BL with code 3, which no value has".into())),
        }
    }

    /// `BD`: a two-bit code, then a double (`00`), nothing for 1.0 (`01`) or 0.0 (`10`); `11` is
    /// refused.
    pub fn bd(&mut self) -> Result<f64, CadError> {
        match self.bb()? {
            0 => self.rd(),
            1 => Ok(1.0),
            2 => Ok(0.0),
            _ => Err(self.bad("a BD with code 3, which no value has".into())),
        }
    }

    /// `DD`: a double relative to `default`: the default itself (`00`); four bytes that replace
    /// its first four (`01`); six bytes, the first two replacing its fifth and sixth bytes and
    /// the next four its first four (`10`); a whole double (`11`). Bytes little-endian.
    pub fn dd(&mut self, default: f64) -> Result<f64, CadError> {
        let mut b = default.to_le_bytes();
        match self.bb()? {
            0 => return Ok(default),
            1 => {
                for x in &mut b[0..4] {
                    *x = self.rc()?;
                }
            }
            2 => {
                if mutant::active("DdHalvesSwapped") {
                    for x in &mut b[0..4] {
                        *x = self.rc()?;
                    }
                    b[4] = self.rc()?;
                    b[5] = self.rc()?;
                } else {
                    b[4] = self.rc()?;
                    b[5] = self.rc()?;
                    for x in &mut b[0..4] {
                        *x = self.rc()?;
                    }
                }
            }
            _ => return self.rd(),
        }
        Ok(f64::from_le_bytes(b))
    }

    /// `UMC`: an unsigned modular number, bytes of seven bits each, least significant first,
    /// the high bit set on every byte but the last. Longer than nine bytes is refused.
    pub fn umc(&mut self) -> Result<u64, CadError> {
        let mut v = 0u64;
        for k in 0..9 {
            let b = self.rc()?;
            v |= u64::from(b & 0x7F) << (7 * k);
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(self.bad("a modular number longer than nine bytes".into()))
    }

    /// `MC`: a signed modular number, as [`Bits::umc`], but the last byte's bit 0x40 is the sign
    /// and its six low bits the last digits.
    pub fn mc(&mut self) -> Result<i64, CadError> {
        let mut v = 0i64;
        for k in 0..9 {
            let b = self.rc()?;
            if b & 0x80 != 0 {
                v |= i64::from(b & 0x7F) << (7 * k);
                continue;
            }
            if mutant::active("McSignLost") {
                return Ok(v | (i64::from(b & 0x7F) << (7 * k)));
            }
            v |= i64::from(b & 0x3F) << (7 * k);
            return Ok(if b & 0x40 != 0 { -v } else { v });
        }
        Err(self.bad("a modular number longer than nine bytes".into()))
    }

    /// `MS`: a modular short, words of fifteen bits each (little-endian), least significant
    /// first, bit 0x8000 set on every word but the last. Longer than four words is refused.
    pub fn ms(&mut self) -> Result<u64, CadError> {
        let mut v = 0u64;
        for k in 0..4 {
            let w = self.rs()?;
            v |= u64::from(w & 0x7FFF) << (15 * k);
            if w & 0x8000 == 0 {
                return Ok(v);
            }
        }
        Err(self.bad("a modular short longer than four words".into()))
    }

    /// `H`: a handle reference: a byte holding the code (high four bits) and the length (low
    /// four), then that many bytes, most significant first. A length over 8 is refused.
    pub fn h(&mut self) -> Result<Handle, CadError> {
        let first = self.rc()?;
        let (code, count) = (first >> 4, first & 0x0F);
        if count > 8 {
            return Err(self.bad(format!("a handle {count} bytes long; at most 8")));
        }
        let mut value = 0u64;
        for _ in 0..count {
            value = (value << 8) | u64::from(self.rc()?);
        }
        Ok(Handle { code, value })
    }

    /// An entity's colour: a `BS` holding the index and its flags; flag 0x4000 says a
    /// colour-book reference sits in the handle stream (and no true colour follows, though
    /// 0x8000 is set with it: the 2004 to 2018 samples' book colours read only so), else 0x8000
    /// adds a true colour (`BL`); 0x2000 adds a transparency (`BL`, after the colour).
    pub fn cmc(&mut self) -> Result<Colour, CadError> {
        let v = self.bs()?;
        let rgb = if v & 0x8000 != 0 && (v & 0x4000 == 0 || mutant::active("BookColourHasRgb")) {
            Some(self.bl()?)
        } else {
            None
        };
        let transparency = if v & 0x2000 != 0 {
            Some(self.bl()?)
        } else {
            None
        };
        Ok(Colour {
            index: v & 0x1FFF,
            rgb,
            book: v & 0x4000 != 0,
            transparency,
        })
    }

    /// `TU`: a text, its length in UTF-16 units as a `BS`, then the units (`RS` each). A length
    /// longer than the bits left is refused before anything is allocated; a unit that isn't
    /// UTF-16 reads as U+FFFD.
    pub fn tu(&mut self) -> Result<String, CadError> {
        let n = usize::from(self.bs()?);
        if mutant::active("TuLengthUnchecked") {
            let mut units: Vec<u16> = Vec::with_capacity(n);
            for _ in 0..n {
                units.push(self.rs()?);
            }
            return Ok(String::from_utf16_lossy(&units));
        }
        if n * 16 > self.left() {
            return Err(self.short(n * 16));
        }
        let mut units = Vec::with_capacity(n);
        for _ in 0..n {
            units.push(self.rs()?);
        }
        Ok(String::from_utf16_lossy(&units))
    }

    /// Moves to bit `at`, held within the stream's span.
    pub fn seek(&mut self, at: usize) {
        self.pos = at.min(self.end);
    }

    /// Skips `n` bits, refused when fewer are left.
    pub fn skip(&mut self, n: usize) -> Result<(), CadError> {
        if n > self.left() {
            return Err(self.short(n));
        }
        self.pos += n;
        Ok(())
    }

    /// `3B`: one to three bits, read until a zero bit or the third bit (0 to 7).
    pub fn three_b(&mut self) -> Result<u8, CadError> {
        let mut v = 0u8;
        for _ in 0..3 {
            let b = self.b()?;
            v = (v << 1) | u8::from(b);
            if !b {
                break;
            }
        }
        Ok(v)
    }

    /// `BLL`: a length in bytes in three bits, then that many bytes, least significant first.
    /// (The specification describes the length as a `3B`; the 2010 to 2018 samples' graphic
    /// sizes read only as three plain bits.)
    pub fn bll(&mut self) -> Result<u64, CadError> {
        let n = self.bits(3)?;
        let mut v = 0u64;
        for k in 0..n {
            v |= u64::from(self.rc()?) << (8 * k);
        }
        Ok(v)
    }

    /// `2RD`: two raw doubles.
    pub fn rd2(&mut self) -> Result<[f64; 2], CadError> {
        Ok([self.rd()?, self.rd()?])
    }

    /// `3RD`: three raw doubles.
    pub fn rd3(&mut self) -> Result<[f64; 3], CadError> {
        Ok([self.rd()?, self.rd()?, self.rd()?])
    }

    /// `2BD`: two bit doubles.
    pub fn bd2(&mut self) -> Result<[f64; 2], CadError> {
        Ok([self.bd()?, self.bd()?])
    }

    /// `3BD`: three bit doubles.
    pub fn bd3(&mut self) -> Result<[f64; 3], CadError> {
        Ok([self.bd()?, self.bd()?, self.bd()?])
    }

    /// `BE`: an extrusion: a bit, set for (0, 0, 1); else three bit doubles.
    pub fn be(&mut self) -> Result<[f64; 3], CadError> {
        if self.b()? {
            Ok([0.0, 0.0, 1.0])
        } else {
            self.bd3()
        }
    }

    /// `BT`: a thickness: a bit, set for 0.0; else a bit double.
    pub fn bt(&mut self) -> Result<f64, CadError> {
        if self.b()? {
            Ok(0.0)
        } else {
            self.bd()
        }
    }

    /// `OT`: an object type: a two-bit code, then a byte (`00`), a byte plus 0x1F0 (`01`), or a
    /// short (`10`, `11`).
    pub fn ot(&mut self) -> Result<u16, CadError> {
        match self.bb()? {
            0 => Ok(u16::from(self.rc()?)),
            1 => {
                let add = if mutant::active("OtOffsetMissing") {
                    0
                } else {
                    0x1F0
                };
                Ok(u16::from(self.rc()?) + add)
            }
            _ => self.rs(),
        }
    }
}
