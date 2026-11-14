//! Bounded big-endian cursors over in-memory box payloads, a bit reader, and
//! child-box iteration. Every read is checked; nothing indexes out of range.

use crate::error::{Error, FourCC, Limit, Limits, Result};

/// A cursor over one box's payload. Errors name the box.
pub(crate) struct Cur<'a> {
    data: &'a [u8],
    pos: usize,
    pub fourcc: FourCC,
}

impl<'a> Cur<'a> {
    pub fn new(data: &'a [u8], fourcc: FourCC) -> Cur<'a> {
        Cur { data, pos: 0, fourcc }
    }
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return Err(Error::Truncated(self.fourcc));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }
    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.data[self.pos..];
        self.pos = self.data.len();
        s
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N]> {
        let s = self.bytes(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        Ok(a)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.arr::<1>()?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.arr()?))
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.arr()?))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.arr()?))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.arr()?))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.arr()?))
    }
    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.arr()?))
    }
    pub fn fourcc_field(&mut self) -> Result<FourCC> {
        Ok(FourCC(self.arr()?))
    }
    /// A FullBox header: (version, flags).
    pub fn full_box(&mut self) -> Result<(u8, u32)> {
        let v = self.u32()?;
        Ok(((v >> 24) as u8, v & 0x00ff_ffff))
    }
    /// A v0/v1 field: u32 for version 0, u64 for version 1.
    pub fn u32_or_u64(&mut self, version: u8) -> Result<u64> {
        match version {
            0 => Ok(self.u32()? as u64),
            1 => self.u64(),
            v => Err(Error::UnsupportedVersion { fourcc: self.fourcc, version: v }),
        }
    }
    /// Checks a declared entry count against the bytes left and the entry
    /// limit before anything is allocated for it.
    pub fn entries(&self, count: u64, entry_size: u64, limits: &Limits) -> Result<usize> {
        if count > limits.max_entries {
            return Err(Error::LimitReached(Limit::Entries));
        }
        let need = count.checked_mul(entry_size).ok_or(Error::Truncated(self.fourcc))?;
        if need > self.remaining() as u64 {
            return Err(Error::Truncated(self.fourcc));
        }
        Ok(count as usize)
    }
}

/// An MSB-first bit reader (AV1 sequence headers, AudioSpecificConfig).
pub(crate) struct Bits<'a> {
    data: &'a [u8],
    bit: usize,
    fourcc: FourCC,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8], fourcc: FourCC) -> Bits<'a> {
        Bits { data, bit: 0, fourcc }
    }
    pub fn bit(&mut self) -> Result<bool> {
        let byte = self.data.get(self.bit / 8).ok_or(Error::Truncated(self.fourcc))?;
        let v = (byte >> (7 - (self.bit % 8))) & 1;
        self.bit += 1;
        Ok(v == 1)
    }
    /// Up to 32 bits.
    pub fn bits(&mut self, n: u32) -> Result<u32> {
        debug_assert!(n <= 32);
        let mut v: u32 = 0;
        for _ in 0..n.min(32) {
            v = (v << 1) | self.bit()? as u32;
        }
        Ok(v)
    }
    /// AV1 `uvlc()`.
    pub fn uvlc(&mut self) -> Result<u32> {
        let mut leading = 0u32;
        while !self.bit()? {
            leading += 1;
            if leading >= 32 {
                return Err(Error::Invalid { fourcc: self.fourcc, what: "uvlc longer than 32 bits" });
            }
        }
        let v = self.bits(leading)?;
        Ok(((1u64 << leading) - 1 + v as u64).min(u32::MAX as u64) as u32)
    }
}

/// Counts boxes and depth against the limits during one parse.
pub(crate) struct Ctx {
    pub limits: Limits,
    pub boxes: u64,
}

impl Ctx {
    pub fn new(limits: Limits) -> Ctx {
        Ctx { limits, boxes: 0 }
    }
    pub fn count_box(&mut self) -> Result<()> {
        self.boxes += 1;
        if self.boxes > self.limits.max_boxes {
            return Err(Error::LimitReached(Limit::Boxes));
        }
        Ok(())
    }
    pub fn depth(&self, depth: u32) -> Result<()> {
        if depth > self.limits.max_depth {
            return Err(Error::LimitReached(Limit::Depth));
        }
        Ok(())
    }
}

/// One child box inside an in-memory parent.
#[derive(Clone, Copy)]
pub(crate) struct BoxRef<'a> {
    pub fourcc: FourCC,
    pub payload: &'a [u8],
}

/// Splits a parent's payload into its child boxes. Size 0 runs to the end of
/// the parent; size 1 takes the 64-bit size; `uuid` skips its 16-byte type.
pub(crate) fn children<'a>(data: &'a [u8], parent: FourCC, depth: u32, ctx: &mut Ctx) -> Result<Vec<BoxRef<'a>>> {
    ctx.depth(depth)?;
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        let rest = &data[pos..];
        if rest.len() < 8 {
            return Err(Error::Truncated(parent));
        }
        ctx.count_box()?;
        let size32 = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as u64;
        let fourcc = FourCC([rest[4], rest[5], rest[6], rest[7]]);
        let mut header = 8u64;
        let size = match size32 {
            0 => rest.len() as u64,
            1 => {
                if rest.len() < 16 {
                    return Err(Error::Truncated(fourcc));
                }
                header = 16;
                u64::from_be_bytes([rest[8], rest[9], rest[10], rest[11], rest[12], rest[13], rest[14], rest[15]])
            }
            s => s,
        };
        if fourcc == FourCC(*b"uuid") {
            header += 16;
        }
        if size < header {
            return Err(Error::BadBoxSize { fourcc, size });
        }
        if size > rest.len() as u64 {
            return Err(Error::PastParent { fourcc });
        }
        let size = size as usize;
        out.push(BoxRef { fourcc, payload: &rest[header as usize..size] });
        pos += size;
    }
    Ok(out)
}

/// The single child of a given type, or `None`; a second one is an error.
pub(crate) fn find_one<'a>(kids: &[BoxRef<'a>], fourcc: &[u8; 4]) -> Result<Option<BoxRef<'a>>> {
    let mut found = None;
    for k in kids {
        if k.fourcc.0 == *fourcc {
            if found.is_some() {
                return Err(Error::Duplicate(k.fourcc));
            }
            found = Some(*k);
        }
    }
    Ok(found)
}

/// The single required child of a given type.
pub(crate) fn require<'a>(kids: &[BoxRef<'a>], parent: FourCC, fourcc: &[u8; 4]) -> Result<BoxRef<'a>> {
    find_one(kids, fourcc)?.ok_or(Error::Missing { parent, child: FourCC(*fourcc) })
}
