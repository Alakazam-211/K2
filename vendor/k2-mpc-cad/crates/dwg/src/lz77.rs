//! The container's compression (the 2004 family's LZ77 variant), decompressed into at most the
//! size the page declares. A declared size over [`Limits::max_page`] is refused before anything
//! is allocated; output past the declared size is refused as it would be written, so a small
//! stream can't grow the heap past the page.
//!
//! The opcodes: a literal run (`0x00`–`0x0F`, first or after a match with no trailing
//! literals); long matches (`0x10`, `0x12`–`0x1F`, offsets from 0x3FFF); medium matches
//! (`0x20`–`0x3F`); short matches (`0x40`–`0xFF`); the end (`0x11`). A match carries up to three
//! trailing literals in its low bits; long lengths add 0xFF per zero byte.
//!
//! From the specification's chapter on the 2004 file format (compression); our own code.

use k2_mpc_cad_model::error::CadError;

use crate::error::{BAD_STREAM, OVER_BOUND, TRUNCATED};
use crate::{mutant, Limits};

const NEXT: &str = "the file is damaged: open a copy the app that made it saves again";

struct Dec<'a> {
    src: &'a [u8],
    i: usize,
    out: Vec<u8>,
    max: usize,
}

impl Dec<'_> {
    fn byte(&mut self) -> Result<u8, CadError> {
        let b = *self.src.get(self.i).ok_or_else(|| {
            CadError::new(
                TRUNCATED,
                "the compressed stream ends before its end mark",
                NEXT,
            )
            .at(self.i)
        })?;
        self.i += 1;
        Ok(b)
    }

    /// Room for `n` more bytes within the declared size.
    fn room(&self, n: usize) -> Result<(), CadError> {
        if mutant::active("BombBoundAtEnd") {
            return Ok(());
        }
        if n > self.max - self.out.len() {
            return Err(CadError::new(
                OVER_BOUND,
                format!(
                    "the page decompresses past its declared size of {} bytes",
                    self.max
                ),
                NEXT,
            )
            .at(self.i));
        }
        Ok(())
    }

    /// A literal run's length from its opcode: the low four bits plus 3, or, when they are 0,
    /// 0x0F plus 0xFF per zero byte plus the first non-zero byte, plus 3.
    fn literal_len(&mut self, op: u8) -> Result<usize, CadError> {
        let mut n = usize::from(op & 0x0F);
        if n == 0 {
            n = self.long_len()?.saturating_add(0x0F);
        }
        Ok(n.saturating_add(3))
    }

    /// 0xFF per zero byte, plus the first non-zero byte.
    fn long_len(&mut self) -> Result<usize, CadError> {
        let mut v = 0usize;
        loop {
            match self.byte()? {
                0 => v = v.saturating_add(0xFF),
                b => return Ok(v.saturating_add(usize::from(b))),
            }
        }
    }

    /// The two bytes after a long or medium match's opcode: the offset, and the trailing
    /// literals' count in the first byte's low two bits.
    fn offset_pair(&mut self) -> Result<(usize, usize), CadError> {
        let a = self.byte()?;
        let b = self.byte()?;
        Ok((
            usize::from(a >> 2) | (usize::from(b) << 6),
            usize::from(a & 3),
        ))
    }

    fn literal(&mut self, n: usize) -> Result<(), CadError> {
        self.room(n)?;
        let run = self
            .i
            .checked_add(n)
            .and_then(|end| self.src.get(self.i..end))
            .ok_or_else(|| {
                CadError::new(TRUNCATED, "a literal run past the stream's end", NEXT).at(self.i)
            })?;
        self.out.extend_from_slice(run);
        self.i += n;
        Ok(())
    }

    /// `n` bytes copied from `offset + 1` bytes back.
    fn copy(&mut self, n: usize, offset: usize) -> Result<(), CadError> {
        let back = if mutant::active("MatchOffsetOffByOne") {
            offset
        } else {
            offset + 1
        };
        let start = match self.out.len().checked_sub(back) {
            Some(s) if back > 0 => s,
            _ if mutant::active("MatchBeforeStartAllowed") => 0,
            _ => {
                return Err(CadError::new(
                    BAD_STREAM,
                    format!(
                        "a match reaches {back} bytes back, with {} written",
                        self.out.len()
                    ),
                    NEXT,
                )
                .at(self.i))
            }
        };
        self.room(n)?;
        for k in 0..n {
            let b = self.out[start + k];
            self.out.push(b);
        }
        Ok(())
    }
}

/// `src` decompressed, at most `size` bytes (the size its page declares): fewer when the
/// stream ends sooner, the caller comparing. Refused, typed: a declared size over
/// [`Limits::max_page`] ([`OVER_BOUND`], before anything is allocated), output past `size`
/// ([`OVER_BOUND`]), a match before the start or an opcode out of place ([`BAD_STREAM`]), a
/// stream that ends before its end mark ([`TRUNCATED`]).
pub fn decompress(src: &[u8], size: usize, limits: &Limits) -> Result<Vec<u8>, CadError> {
    if size > limits.max_page && !mutant::active("PageSizeUnbounded") {
        return Err(CadError::new(
            OVER_BOUND,
            format!(
                "a page declares {size} bytes decompressed, over the limit of {}",
                limits.max_page
            ),
            "raise the page limit for a file you trust",
        ));
    }
    let mut d = Dec {
        src,
        i: 0,
        out: Vec::with_capacity(size),
        max: size,
    };
    let mut op = d.byte()?;
    if op & 0xF0 == 0 {
        let n = d.literal_len(op)?;
        d.literal(n)?;
        op = d.byte()?;
    }
    while op != 0x11 {
        let (n, offset, trailing) = match op {
            0x10 => {
                let n = d.long_len()?.saturating_add(9);
                let (o, t) = d.offset_pair()?;
                (n, o + 0x3FFF, t)
            }
            0x12..=0x1F => {
                let (o, t) = d.offset_pair()?;
                (usize::from(op & 0x0F) + 2, o + 0x3FFF, t)
            }
            0x20 => {
                let n = d.long_len()?.saturating_add(0x21);
                let (o, t) = d.offset_pair()?;
                (n, o, t)
            }
            0x21..=0x3F => {
                let (o, t) = d.offset_pair()?;
                (usize::from(op) - 0x1E, o, t)
            }
            0x40..=0xFF => {
                let op2 = d.byte()?;
                (
                    usize::from(op >> 4) - 1,
                    (usize::from(op2) << 2) | usize::from((op & 0x0C) >> 2),
                    usize::from(op & 3),
                )
            }
            _ => {
                return Err(CadError::new(
                    BAD_STREAM,
                    format!("opcode 0x{op:02X} where a match or the end must come"),
                    NEXT,
                )
                .at(d.i - 1))
            }
        };
        d.copy(n, offset)?;
        if trailing == 0 {
            op = d.byte()?;
            if op & 0xF0 == 0 {
                let n = d.literal_len(op)?;
                d.literal(n)?;
                op = d.byte()?;
            }
        } else {
            d.literal(trailing)?;
            op = d.byte()?;
        }
    }
    if d.out.len() > d.max {
        return Err(CadError::new(
            OVER_BOUND,
            format!(
                "the page decompresses past its declared size of {} bytes",
                d.max
            ),
            NEXT,
        ));
    }
    Ok(d.out)
}
