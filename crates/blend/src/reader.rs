//! The reader: the file header, then every file block in order to the end block, forward only,
//! under [`ReadLimits`]. What it keeps is bounded: every block's header, the schema block, each
//! data-block's own struct, and small data blocks while they fit in
//! [`ReadLimits::max_retained_bytes`]. A larger body is digested as it passes and dropped.
//!
//! **Blocks.** Each has a four-byte code, a body length, the saving program's memory address of
//! the data (the "old address"), the index of its struct in the schema, and how many structs
//! the body holds. A code of two capital letters and two NULs (`OB\0\0`) starts a data-block
//! (an ID): an object, a material, a scene... The `DATA` blocks after it are its own data.
//! `DNA1` is the schema; `ENDB` ends the file.
//!
//! **Pointers** are old addresses. Sample files show that two blocks can share one, so a
//! pointer resolves ([`BlendFile::resolve`]) to the one block with that address; else to the
//! one owned by the same data-block as the pointer; else to the one data-block among them; else
//! it is ambiguous, never a guess. Schema index 0 also marks raw (untyped) data, so a body is
//! read as structs only through a field that says what it points to.
//!
//! **For later writes.** Every block keeps its body's offset in the decompressed stream, so a
//! field's place in the file is the body's offset plus the field's offset in the schema.

use std::collections::HashMap;
use std::io::Read;

use k2_mpc_base::index::Digest;

use crate::compress::{self, Compression};
use crate::error::{ReadError, ReadLimits};
use crate::header::{parse_header, Header, HeaderForm};
use crate::sdna::Sdna;

/// A block's code as bytes.
pub type Code = [u8; 4];

/// The schema block's code.
pub const DNA1: Code = *b"DNA1";
/// The end block's code.
pub const ENDB: Code = *b"ENDB";
/// The code of a data-block's own data.
pub const DATA: Code = *b"DATA";

/// Whether `code` starts a data-block: two capital letters, then two NULs.
pub fn is_id_code(code: &Code) -> bool {
    code[0].is_ascii_uppercase() && code[1].is_ascii_uppercase() && code[2] == 0 && code[3] == 0
}

/// The header of one block, and where it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlockHead {
    /// `OB\0\0`, `DATA`, `DNA1`...
    pub code: Code,
    /// Index into the schema's structs (0 also marks raw data).
    pub sdna: u32,
    /// The saving program's address of the data.
    pub old: u64,
    /// The body's length in bytes.
    pub len: u64,
    /// How many structs the body holds.
    pub count: u64,
    /// Where the body starts in the decompressed stream.
    pub body_offset: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Body {
    Kept(u32),
    Digested(u32),
    None,
}

/// One block, read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    /// Its header.
    pub head: BlockHead,
    /// The data-block this block belongs to: itself for a data-block, the one before it for a
    /// `DATA` block; `None` for anything else.
    pub owner: Option<u32>,
    body: Body,
}

impl Block {
    /// Whether it starts a data-block ([`is_id_code`]).
    pub fn is_id(&self) -> bool {
        is_id_code(&self.head.code)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Addr {
    One(u32),
    Many(Vec<u32>),
}

/// What a pointer names ([`BlendFile::resolve`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// A null pointer.
    Null,
    /// The block at this index.
    Block(u32),
    /// No block has that address (memory the file doesn't hold).
    Unresolved,
    /// Several blocks have it, and nothing tells them apart.
    Ambiguous,
}

/// Reads numbers in a file's byte order and pointer size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endian {
    /// Big-endian numbers.
    pub big: bool,
    /// 4 or 8.
    pub pointer_size: u8,
}

macro_rules! read_num {
    ($name:ident, $t:ty, $n:literal) => {
        #[doc = concat!("A `", stringify!($t), "` at `at`, if `b` holds one there.")]
        pub fn $name(&self, b: &[u8], at: usize) -> Option<$t> {
            let s: [u8; $n] = b.get(at..at.checked_add($n)?)?.try_into().ok()?;
            Some(if self.big {
                <$t>::from_be_bytes(s)
            } else {
                <$t>::from_le_bytes(s)
            })
        }
    };
}

impl Endian {
    read_num!(u16, u16, 2);
    read_num!(i16, i16, 2);
    read_num!(u32, u32, 4);
    read_num!(i32, i32, 4);
    read_num!(u64, u64, 8);
    read_num!(i64, i64, 8);
    read_num!(f32, f32, 4);
    read_num!(f64, f64, 8);

    /// A pointer at `at`, widened to 64 bits.
    pub fn ptr(&self, b: &[u8], at: usize) -> Option<u64> {
        if self.pointer_size == 8 {
            self.u64(b, at)
        } else {
            self.u32(b, at).map(u64::from)
        }
    }
}

/// A file, read: its header, schema and blocks, with what was kept of the bodies.
#[derive(Debug)]
pub struct BlendFile {
    /// The file header.
    pub header: Header,
    /// How the file was stored.
    pub compression: Compression,
    /// The file's own schema.
    pub sdna: Sdna,
    /// Every block, in file order, the end block last.
    pub blocks: Vec<Block>,
    /// Bytes read from the decompressed stream, the end block's header included.
    pub raw_len: u64,
    bodies: Vec<Vec<u8>>,
    digests: Vec<Digest>,
    addr: HashMap<u64, Addr>,
}

impl BlendFile {
    /// The file's byte order and pointer size.
    pub fn endian(&self) -> Endian {
        Endian {
            big: self.header.big_endian,
            pointer_size: self.header.pointer_size,
        }
    }

    /// Block `i`'s body, when it was kept.
    pub fn body(&self, i: u32) -> Option<&[u8]> {
        match self.blocks.get(i as usize)?.body {
            Body::Kept(k) => self.bodies.get(k as usize).map(Vec::as_slice),
            _ => None,
        }
    }

    /// The digest of block `i`'s body as stored, when it wasn't kept.
    pub fn body_digest(&self, i: u32) -> Option<&Digest> {
        match self.blocks.get(i as usize)?.body {
            Body::Digested(d) => self.digests.get(d as usize),
            _ => None,
        }
    }

    /// What the pointer `ptr`, read from block `from`, names.
    pub fn resolve(&self, ptr: u64, from: u32) -> Target {
        if ptr == 0 {
            return Target::Null;
        }
        match self.addr.get(&ptr) {
            None => Target::Unresolved,
            Some(Addr::One(i)) => Target::Block(*i),
            Some(Addr::Many(v)) => {
                let owner = self.blocks.get(from as usize).and_then(|b| b.owner);
                if owner.is_some() {
                    let mut same = v
                        .iter()
                        .filter(|&&j| self.blocks[j as usize].owner == owner);
                    if let (Some(&j), None) = (same.next(), same.next()) {
                        return Target::Block(j);
                    }
                }
                let mut ids = v.iter().filter(|&&j| self.blocks[j as usize].is_id());
                if let (Some(&j), None) = (ids.next(), ids.next()) {
                    return Target::Block(j);
                }
                Target::Ambiguous
            }
        }
    }

    /// How many blocks share an old address with another block.
    pub fn shared_addresses(&self) -> usize {
        self.addr
            .values()
            .map(|a| match a {
                Addr::One(_) => 0,
                Addr::Many(v) => v.len(),
            })
            .sum()
    }
}

/// The decompressed stream, counted against [`ReadLimits::max_raw_bytes`].
struct Raw<'a> {
    inner: Box<dyn Read + 'a>,
    at: u64,
    max: u64,
}

impl Raw<'_> {
    /// Reads `buf` whole. `Ok(false)` when the stream ended before the first byte.
    fn fill(&mut self, buf: &mut [u8], what: &'static str) -> Result<bool, ReadError> {
        let end = self.at.saturating_add(buf.len() as u64);
        if end > self.max {
            return Err(ReadError::Limit {
                what: "bytes after decompression",
                limit: self.max,
            });
        }
        let mut got = 0;
        while got < buf.len() {
            match self.inner.read(&mut buf[got..]) {
                Ok(0) => {
                    if got == 0 {
                        return Ok(false);
                    }
                    return Err(ReadError::truncated(what));
                }
                Ok(n) => got += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(compress::read_error(e)),
            }
        }
        self.at = end;
        Ok(true)
    }

    /// Reads as much of `buf` as the stream has; the count.
    fn up_to(&mut self, buf: &mut [u8]) -> Result<usize, ReadError> {
        let mut got = 0;
        while got < buf.len() {
            match self.inner.read(&mut buf[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(compress::read_error(e)),
            }
        }
        self.at += got as u64;
        Ok(got)
    }

    fn exact(&mut self, buf: &mut [u8], what: &'static str) -> Result<(), ReadError> {
        if buf.is_empty() || self.fill(buf, what)? {
            Ok(())
        } else {
            Err(ReadError::truncated(what))
        }
    }
}

/// Reads a whole file from `src` under `limits`: forward only, stopping after the end block.
pub fn read_blend(src: &mut dyn Read, limits: &ReadLimits) -> Result<BlendFile, ReadError> {
    let opened = compress::open(src, limits)?;
    let compression = opened.compression;
    let mut raw = Raw {
        inner: opened.stream,
        at: 0,
        max: limits.max_raw_bytes,
    };
    let mut first = [0u8; 17];
    let got = raw.up_to(&mut first[..12])?;
    if got < 12 {
        // too short for any header: a file that isn't one, or one cut short
        parse_header(&first[..got])?;
        return Err(ReadError::truncated("the file header"));
    }
    let got = if first[7].is_ascii_digit() {
        12 + raw.up_to(&mut first[12..17])?
    } else {
        12
    };
    let header = parse_header(&first[..got])?;
    let e = Endian {
        big: header.big_endian,
        pointer_size: header.pointer_size,
    };
    let ps = usize::from(header.pointer_size);
    let hl = header.block_header_len();

    let mut blocks: Vec<Block> = Vec::new();
    let mut bodies: Vec<Vec<u8>> = Vec::new();
    let mut digests: Vec<Digest> = Vec::new();
    let mut addr: HashMap<u64, Addr> = HashMap::new();
    let mut kept: u64 = 0;
    let mut current: Option<u32> = None;
    let mut chunk = vec![0u8; 64 << 10];
    let mut schema: Option<u32> = None;
    loop {
        let mut bh = [0u8; 32];
        if !raw.fill(&mut bh[..hl], "a block header")? {
            return Err(ReadError::truncated("the file ends before its end block"));
        }
        let code: Code = bh[..4].try_into().expect("4 bytes");
        let bad = || ReadError::malformed("a block header doesn't read");
        let (len, old, sdna, count) = match header.form {
            HeaderForm::Classic => {
                let len = e.i32(&bh, 4).ok_or_else(bad)?;
                let old = e.ptr(&bh, 8).ok_or_else(bad)?;
                let sdna = e.i32(&bh, 8 + ps).ok_or_else(bad)?;
                let count = e.i32(&bh, 12 + ps).ok_or_else(bad)?;
                if len < 0 || sdna < 0 || count < 0 {
                    return Err(ReadError::malformed(
                        "a block header holds a negative number",
                    ));
                }
                (len as u64, old, sdna as u32, count as u64)
            }
            HeaderForm::Large => {
                let sdna = e.u32(&bh, 4).ok_or_else(bad)?;
                let old = e.u64(&bh, 8).ok_or_else(bad)?;
                let len = e.i64(&bh, 16).ok_or_else(bad)?;
                let count = e.i64(&bh, 24).ok_or_else(bad)?;
                if len < 0 || count < 0 {
                    return Err(ReadError::malformed(
                        "a block header holds a negative number",
                    ));
                }
                (len as u64, old, sdna, count as u64)
            }
        };
        if blocks.len() as u64 >= limits.max_blocks {
            return Err(ReadError::Limit {
                what: "blocks",
                limit: limits.max_blocks,
            });
        }
        let index = blocks.len() as u32;
        let head = BlockHead {
            code,
            sdna,
            old,
            len,
            count,
            body_offset: raw.at,
        };
        if code == ENDB {
            blocks.push(Block {
                head,
                owner: None,
                body: Body::None,
            });
            // the stream must end here (a compressed one is decoded to its end, so a damaged
            // tail or a bad checksum is seen)
            let mut one = [0u8; 1];
            if len != 0 || raw.up_to(&mut one)? != 0 {
                return Err(ReadError::malformed("bytes after the end block"));
            }
            break;
        }
        let id = is_id_code(&code);
        let owner = if id {
            current = Some(index);
            Some(index)
        } else if code == DATA {
            current
        } else {
            current = None;
            None
        };
        let keep = if code == DNA1 {
            if schema.is_some() {
                return Err(ReadError::malformed("the file has two schema blocks"));
            }
            if len > limits.max_sdna_bytes {
                return Err(ReadError::Limit {
                    what: "the schema block",
                    limit: limits.max_sdna_bytes,
                });
            }
            schema = Some(index);
            true
        } else if id {
            if kept.saturating_add(len) > limits.max_retained_bytes {
                return Err(ReadError::Limit {
                    what: "bytes kept of data-blocks",
                    limit: limits.max_retained_bytes,
                });
            }
            true
        } else {
            code == DATA
                && len <= limits.retain_block_max
                && kept.saturating_add(len) <= limits.max_retained_bytes
        };
        let body = if keep {
            let n =
                usize::try_from(len).map_err(|_| ReadError::malformed("a block is too long"))?;
            let mut b = vec![0u8; n];
            raw.exact(&mut b, "a block's body")?;
            if code != DNA1 {
                kept += len;
            }
            bodies.push(b);
            Body::Kept((bodies.len() - 1) as u32)
        } else {
            let mut d = Digest::stream();
            let mut left = len;
            while left > 0 {
                let n = left.min(chunk.len() as u64) as usize;
                raw.exact(&mut chunk[..n], "a block's body")?;
                d.update(&chunk[..n]);
                left -= n as u64;
            }
            digests.push(d.finish());
            Body::Digested((digests.len() - 1) as u32)
        };
        if old != 0 {
            match addr.get_mut(&old) {
                None => {
                    addr.insert(old, Addr::One(index));
                }
                Some(Addr::One(first)) => {
                    let first = *first;
                    addr.insert(old, Addr::Many(vec![first, index]));
                }
                Some(Addr::Many(v)) => v.push(index),
            }
        }
        blocks.push(Block { head, owner, body });
    }
    let schema = schema.ok_or(ReadError::malformed("the file has no schema block"))?;
    let sdna = match blocks[schema as usize].body {
        Body::Kept(k) => Sdna::parse(&bodies[k as usize], header.pointer_size, header.big_endian)?,
        _ => return Err(ReadError::malformed("the file has no schema block")),
    };
    // the schema's bytes aren't needed again
    if let Body::Kept(k) = blocks[schema as usize].body {
        bodies[k as usize] = Vec::new();
    }
    Ok(BlendFile {
        header,
        compression,
        sdna,
        blocks,
        raw_len: raw.at,
        bodies,
        digests,
        addr,
    })
}
