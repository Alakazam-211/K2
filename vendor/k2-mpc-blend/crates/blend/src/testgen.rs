//! Test-only (feature `testgen`): builds BLEND files for the tests from a schema written here,
//! never from another program's. A file is a header, the blocks a test asks for, the schema
//! block and the end block; [`zstd_frames`] and [`gzip`] store it the two compressed ways, and
//! [`BigFile`] streams a file of any size without holding it. [`SceneBuilder`] writes scenes.

use std::collections::HashMap;
use std::io::{self, Read, Write};

use crate::header::HeaderForm;
use crate::reader::Code;

mod scene;
pub mod scenes;
pub use scene::{
    CameraSpec, Channel, Driven, MeshLayouts, MeshSpec, ObjectSpec, Render, SceneBuilder,
    ACTION_SLOTS_FROM, CONSTRAINT_STRUCTS, EMBEDDED_ID_FLAG, IDENTITY,
};

/// A schema to write: types with their lengths, and structs of (type, name) fields.
#[derive(Debug, Clone)]
pub struct SchemaBuilder {
    pointer_size: u8,
    names: Vec<String>,
    types: Vec<(String, u16)>,
    structs: Vec<(u16, Vec<(u16, u16)>)>,
}

impl SchemaBuilder {
    /// The basic types (`char`, `uchar`, `short`, `ushort`, `int`, `float`, `double`,
    /// `int64_t`, `void`) and struct 0, `Link` (`*next`, `*prev`), standing in for raw data.
    pub fn new(pointer_size: u8) -> Self {
        let mut s = Self {
            pointer_size,
            names: Vec::new(),
            types: Vec::new(),
            structs: Vec::new(),
        };
        for (t, l) in [
            ("char", 1),
            ("uchar", 1),
            ("short", 2),
            ("ushort", 2),
            ("int", 4),
            ("float", 4),
            ("double", 8),
            ("int64_t", 8),
            ("void", 0),
        ] {
            s.add_type(t, l);
        }
        s.add_struct("Link", &[("Link", "*next"), ("Link", "*prev")]);
        s
    }

    /// A type (or the one already there).
    pub fn add_type(&mut self, name: &str, len: u16) -> u16 {
        if let Some(i) = self.types.iter().position(|(n, _)| n == name) {
            return i as u16;
        }
        self.types.push((name.to_owned(), len));
        (self.types.len() - 1) as u16
    }

    fn name(&mut self, n: &str) -> u16 {
        if let Some(i) = self.names.iter().position(|x| x == n) {
            return i as u16;
        }
        self.names.push(n.to_owned());
        (self.names.len() - 1) as u16
    }

    /// The bytes a field takes, as a reader computes them.
    pub fn field_size(&self, ty: &str, name: &str) -> usize {
        let elem = if name.starts_with('*') || name.starts_with("(*") {
            usize::from(self.pointer_size)
        } else {
            self.types
                .iter()
                .find(|(n, _)| n == ty)
                .map_or(0, |(_, l)| usize::from(*l))
        };
        let dims: usize = name
            .split('[')
            .skip(1)
            .map(|p| {
                p.split(']')
                    .next()
                    .unwrap_or("1")
                    .parse::<usize>()
                    .unwrap_or(1)
            })
            .product();
        elem * dims
    }

    /// A struct of `fields` (type, written name): its length is theirs added up. Field types
    /// not yet known must be structs added before, or pointers. Returns its struct index.
    pub fn add_struct(&mut self, name: &str, fields: &[(&str, &str)]) -> u32 {
        let len: usize = fields.iter().map(|(t, n)| self.field_size(t, n)).sum();
        let t = self.add_type(name, len as u16);
        self.types[usize::from(t)].1 = len as u16;
        let fs = fields
            .iter()
            .map(|(ft, n)| {
                let ft = self.add_type(ft, 0);
                (ft, self.name(n))
            })
            .collect();
        self.structs.push((t, fs));
        (self.structs.len() - 1) as u32
    }

    /// Adds a raw struct as given, unchecked (for malformed schemas): its type's length is set
    /// to `len`.
    pub fn add_struct_raw(&mut self, name: &str, len: u16, fields: &[(&str, &str)]) -> u32 {
        let t = self.add_type(name, len);
        self.types[usize::from(t)].1 = len;
        let fs = fields
            .iter()
            .map(|(ft, n)| {
                let ft = self.add_type(ft, 0);
                (ft, self.name(n))
            })
            .collect();
        self.structs.push((t, fs));
        (self.structs.len() - 1) as u32
    }

    /// Struct `name`'s index.
    pub fn index(&self, name: &str) -> u32 {
        self.structs
            .iter()
            .position(|(t, _)| self.types[usize::from(*t)].0 == name)
            .unwrap_or_else(|| panic!("no struct {name}")) as u32
    }

    /// Struct `name`'s length.
    pub fn len_of(&self, name: &str) -> usize {
        usize::from(
            self.types
                .iter()
                .find(|(n, _)| n == name)
                .unwrap_or_else(|| panic!("no type {name}"))
                .1,
        )
    }

    /// The offset of field `field` (bare name) in struct `name`.
    pub fn offset(&self, name: &str, field: &str) -> usize {
        let (_, fs) = &self.structs[self.index(name) as usize];
        let mut off = 0;
        for (ft, n) in fs {
            let written = &self.names[usize::from(*n)];
            let bare: String = written
                .trim_start_matches(['(', '*'])
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if bare == field {
                return off;
            }
            off += self.field_size(&self.types[usize::from(*ft)].0, written);
        }
        panic!("no field {field} in {name}")
    }

    /// The schema block's body, in the given byte order.
    pub fn encode(&self, big: bool) -> Vec<u8> {
        let u32b = |v: u32| {
            if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let u16b = |v: u16| {
            if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let pad = |b: &mut Vec<u8>| {
            while !b.len().is_multiple_of(4) {
                b.push(0);
            }
        };
        let mut b = b"SDNANAME".to_vec();
        b.extend(u32b(self.names.len() as u32));
        for n in &self.names {
            b.extend(n.as_bytes());
            b.push(0);
        }
        pad(&mut b);
        b.extend(b"TYPE");
        b.extend(u32b(self.types.len() as u32));
        for (n, _) in &self.types {
            b.extend(n.as_bytes());
            b.push(0);
        }
        pad(&mut b);
        b.extend(b"TLEN");
        for (_, l) in &self.types {
            b.extend(u16b(*l));
        }
        pad(&mut b);
        b.extend(b"STRC");
        b.extend(u32b(self.structs.len() as u32));
        for (t, fs) in &self.structs {
            b.extend(u16b(*t));
            b.extend(u16b(fs.len() as u16));
            for (ft, n) in fs {
                b.extend(u16b(*ft));
                b.extend(u16b(*n));
            }
        }
        b
    }
}

/// One block to write.
#[derive(Debug, Clone)]
pub struct RawBlock {
    /// Its code.
    pub code: Code,
    /// Its struct index.
    pub sdna: u32,
    /// Its old address.
    pub old: u64,
    /// Its struct count.
    pub count: u64,
    /// Its body.
    pub body: Vec<u8>,
}

/// A file to write.
#[derive(Debug, Clone)]
pub struct FileBuilder {
    /// The header form.
    pub form: HeaderForm,
    /// 4 or 8 (the newer form: 8).
    pub pointer_size: u8,
    /// Big-endian (the classic form only).
    pub big: bool,
    /// `403`, `501`...
    pub version: u16,
    /// The schema.
    pub schema: SchemaBuilder,
    /// The blocks, before the schema and end blocks.
    pub blocks: Vec<RawBlock>,
    /// Leave the schema block out.
    pub no_schema: bool,
    /// Leave the end block out.
    pub no_end: bool,
}

impl FileBuilder {
    /// An empty file of this form, with the basic schema.
    pub fn new(form: HeaderForm, pointer_size: u8, big: bool, version: u16) -> Self {
        Self {
            form,
            pointer_size,
            big,
            version,
            schema: SchemaBuilder::new(pointer_size),
            blocks: Vec::new(),
            no_schema: false,
            no_end: false,
        }
    }

    /// 8-byte pointers, little-endian, the classic form, version 4.03.
    pub fn classic() -> Self {
        Self::new(HeaderForm::Classic, 8, false, 403)
    }

    /// The newer form, version 5.01.
    pub fn large() -> Self {
        Self::new(HeaderForm::Large, 8, false, 501)
    }

    /// Adds a block of struct `name` (count 1 unless the body holds more).
    pub fn block(&mut self, code: &[u8], name: &str, old: u64, body: Vec<u8>) -> &mut Self {
        let sdna = self.schema.index(name);
        let len = self.schema.len_of(name).max(1);
        let count = (body.len() / len).max(1) as u64;
        self.raw(code, sdna, old, count, body)
    }

    /// Adds a block as given.
    pub fn raw(
        &mut self,
        code: &[u8],
        sdna: u32,
        old: u64,
        count: u64,
        body: Vec<u8>,
    ) -> &mut Self {
        let mut c = [0u8; 4];
        c[..code.len().min(4)].copy_from_slice(&code[..code.len().min(4)]);
        self.blocks.push(RawBlock {
            code: c,
            sdna,
            old,
            count,
            body,
        });
        self
    }

    /// The file header's bytes.
    pub fn header_bytes(&self) -> Vec<u8> {
        match self.form {
            HeaderForm::Classic => format!(
                "BLENDER{}{}{:03}",
                if self.pointer_size == 8 { '-' } else { '_' },
                if self.big { 'V' } else { 'v' },
                self.version
            )
            .into_bytes(),
            _ => format!("BLENDER17-01v{:04}", self.version).into_bytes(),
        }
    }

    /// A block header's bytes.
    pub fn block_header(&self, b: &RawBlock, len: u64) -> Vec<u8> {
        let mut h = b.code.to_vec();
        let big = self.big;
        let u32b = |v: u32| {
            if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        match self.form {
            HeaderForm::Classic => {
                h.extend(u32b(len as u32));
                if self.pointer_size == 8 {
                    h.extend(if big {
                        b.old.to_be_bytes()
                    } else {
                        b.old.to_le_bytes()
                    });
                } else {
                    h.extend(u32b(b.old as u32));
                }
                h.extend(u32b(b.sdna));
                h.extend(u32b(b.count as u32));
            }
            _ => {
                h.extend(b.sdna.to_le_bytes());
                h.extend(b.old.to_le_bytes());
                h.extend(len.to_le_bytes());
                h.extend(b.count.to_le_bytes());
            }
        }
        h
    }

    /// The whole file, uncompressed.
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = self.header_bytes();
        let mut all = self.blocks.clone();
        if !self.no_schema {
            all.push(RawBlock {
                code: *b"DNA1",
                sdna: 0,
                old: 0,
                count: 1,
                body: self.schema.encode(self.big),
            });
        }
        if !self.no_end {
            all.push(RawBlock {
                code: *b"ENDB",
                sdna: 0,
                old: 0,
                count: 0,
                body: Vec::new(),
            });
        }
        for b in &all {
            out.extend(self.block_header(b, b.body.len() as u64));
            out.extend(&b.body);
        }
        out
    }
}

/// A number written in a file's byte order.
pub fn put(buf: &mut [u8], at: usize, bytes_le: &[u8], big: bool) {
    let n = bytes_le.len();
    if big {
        for (i, b) in bytes_le.iter().rev().enumerate() {
            buf[at + i] = *b;
        }
    } else {
        buf[at..at + n].copy_from_slice(bytes_le);
    }
}

/// `data` as zstd frames of at most `frame` bytes each, every frame of raw blocks
/// (RFC 8878: single-segment, content size given), with a skippable frame after the last.
pub fn zstd_frames(data: &[u8], frame: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for piece in data.chunks(frame.max(1)) {
        zstd_frame(&mut out, piece);
    }
    if data.is_empty() {
        zstd_frame(&mut out, &[]);
    }
    // a skippable frame (a seek table, say)
    out.extend(0x184D_2A5Eu32.to_le_bytes());
    out.extend(8u32.to_le_bytes());
    out.extend([7u8; 8]);
    out
}

/// `data` as zstd frames of at most `frame` bytes each, every frame of raw blocks, then a seek
/// table in the seekable layout (a skippable frame of magic `0x184D2A5E` holding each frame's
/// stored and decompressed size, no checksums, then the frame count, a descriptor byte and the
/// magic `0x8F92EAB1`).
pub fn zstd_seekable(data: &[u8], frame: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut sizes: Vec<(u32, u32)> = Vec::new();
    let pieces: Vec<&[u8]> = if data.is_empty() {
        vec![&[]]
    } else {
        data.chunks(frame.max(1)).collect()
    };
    for piece in pieces {
        let at = out.len();
        zstd_frame(&mut out, piece);
        sizes.push(((out.len() - at) as u32, piece.len() as u32));
    }
    let mut table = Vec::new();
    for (stored, plain) in &sizes {
        table.extend(stored.to_le_bytes());
        table.extend(plain.to_le_bytes());
    }
    table.extend((sizes.len() as u32).to_le_bytes());
    table.push(0);
    table.extend(0x8F92_EAB1u32.to_le_bytes());
    out.extend(0x184D_2A5Eu32.to_le_bytes());
    out.extend((table.len() as u32).to_le_bytes());
    out.extend(table);
    out
}

/// One single-segment frame of raw blocks holding `piece`, with an 8-byte content size.
pub fn zstd_frame(out: &mut Vec<u8>, piece: &[u8]) {
    out.extend(0xFD2F_B528u32.to_le_bytes());
    // FCS 8 bytes (flag 3), single segment, no checksum, no dictionary
    out.push(0b1110_0000);
    out.extend((piece.len() as u64).to_le_bytes());
    let blocks: Vec<&[u8]> = if piece.is_empty() {
        vec![&[]]
    } else {
        piece.chunks(100_000).collect()
    };
    for (i, b) in blocks.iter().enumerate() {
        let last = u32::from(i + 1 == blocks.len());
        let h = last | ((b.len() as u32) << 3); // block type 0: raw
        out.extend(&h.to_le_bytes()[..3]);
        out.extend(*b);
    }
}

/// A frame header asking for a window of 2^(10 + exponent) bytes (not single-segment), with
/// one empty last block.
pub fn zstd_window_frame(exponent: u8, dictionary: bool) -> Vec<u8> {
    let mut out = 0xFD2F_B528u32.to_le_bytes().to_vec();
    out.push(if dictionary { 0b0000_0001 } else { 0 });
    out.push(exponent << 3);
    if dictionary {
        out.push(9);
    }
    out.extend(&1u32.to_le_bytes()[..3]); // last, raw, empty
    out
}

/// `data`, gzipped.
pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(data).expect("gzip into memory");
    e.finish().expect("gzip into memory")
}

/// A file streamed as it is read, never held: the header, a data-block, then `blocks` data
/// blocks of `block_len` bytes each (a repeating pattern), the schema and the end block.
pub struct BigFile {
    parts: Vec<Part>,
    at: usize,
    off: u64,
    /// Total bytes the file has.
    pub total: u64,
}

enum Part {
    Bytes(Vec<u8>),
    /// `left` blocks of `header` then `body_len` pattern bytes.
    Repeat {
        header: Vec<u8>,
        body_len: u64,
        left: u64,
    },
}

impl BigFile {
    /// The file. `file` gives the form, schema and leading blocks (written as given).
    pub fn new(file: &FileBuilder, blocks: u64, block_len: u64) -> Self {
        let mut head = file.header_bytes();
        for b in &file.blocks {
            head.extend(file.block_header(b, b.body.len() as u64));
            head.extend(&b.body);
        }
        let data = RawBlock {
            code: *b"DATA",
            sdna: 0,
            old: 0x7000_0000,
            count: 1,
            body: Vec::new(),
        };
        let header = file.block_header(&data, block_len);
        let mut tail = Vec::new();
        let dna = RawBlock {
            code: *b"DNA1",
            sdna: 0,
            old: 0,
            count: 1,
            body: file.schema.encode(file.big),
        };
        tail.extend(file.block_header(&dna, dna.body.len() as u64));
        tail.extend(&dna.body);
        let end = RawBlock {
            code: *b"ENDB",
            sdna: 0,
            old: 0,
            count: 0,
            body: Vec::new(),
        };
        tail.extend(file.block_header(&end, 0));
        let total =
            head.len() as u64 + blocks * (header.len() as u64 + block_len) + tail.len() as u64;
        Self {
            parts: vec![
                Part::Bytes(head),
                Part::Repeat {
                    header,
                    body_len: block_len,
                    left: blocks,
                },
                Part::Bytes(tail),
            ],
            at: 0,
            off: 0,
            total,
        }
    }
}

impl Read for BigFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let Some(part) = self.parts.get_mut(self.at) else {
                return Ok(0);
            };
            match part {
                Part::Bytes(b) => {
                    let rest = &b[self.off as usize..];
                    if rest.is_empty() {
                        self.at += 1;
                        self.off = 0;
                        continue;
                    }
                    let n = rest.len().min(buf.len());
                    buf[..n].copy_from_slice(&rest[..n]);
                    self.off += n as u64;
                    return Ok(n);
                }
                Part::Repeat {
                    header,
                    body_len,
                    left,
                } => {
                    if *left == 0 {
                        self.at += 1;
                        self.off = 0;
                        continue;
                    }
                    // within one block: `off` counts its header, then its body
                    let hl = header.len() as u64;
                    let n = if self.off < hl {
                        let rest = &header[self.off as usize..];
                        let n = rest.len().min(buf.len());
                        buf[..n].copy_from_slice(&rest[..n]);
                        n
                    } else {
                        let body_at = self.off - hl;
                        let n = (*body_len - body_at).min(buf.len() as u64) as usize;
                        for (i, x) in buf[..n].iter_mut().enumerate() {
                            *x = ((body_at + i as u64) % 251) as u8;
                        }
                        n
                    };
                    self.off += n as u64;
                    if self.off == hl + *body_len {
                        *left -= 1;
                        self.off = 0;
                    }
                    return Ok(n);
                }
            }
        }
    }
}

/// Little helpers for building bodies at a schema's offsets.
#[derive(Debug, Clone, Default)]
pub struct Body {
    /// The bytes.
    pub bytes: Vec<u8>,
    big: bool,
    ptr8: bool,
}

impl Body {
    /// A zeroed body of `len` bytes.
    pub fn new(len: usize, big: bool, pointer_size: u8) -> Self {
        Self {
            bytes: vec![0; len],
            big,
            ptr8: pointer_size == 8,
        }
    }
    /// A pointer at `at`.
    pub fn ptr(&mut self, at: usize, v: u64) -> &mut Self {
        if self.ptr8 {
            put(&mut self.bytes, at, &v.to_le_bytes(), self.big);
        } else {
            put(&mut self.bytes, at, &(v as u32).to_le_bytes(), self.big);
        }
        self
    }
    /// An `i32` at `at`.
    pub fn i32(&mut self, at: usize, v: i32) -> &mut Self {
        put(&mut self.bytes, at, &v.to_le_bytes(), self.big);
        self
    }
    /// An `i16` at `at`.
    pub fn i16(&mut self, at: usize, v: i16) -> &mut Self {
        put(&mut self.bytes, at, &v.to_le_bytes(), self.big);
        self
    }
    /// `f32`s from `at` on.
    pub fn f32s(&mut self, at: usize, v: &[f32]) -> &mut Self {
        for (i, x) in v.iter().enumerate() {
            put(&mut self.bytes, at + 4 * i, &x.to_le_bytes(), self.big);
        }
        self
    }
    /// Bytes at `at`.
    pub fn raw(&mut self, at: usize, v: &[u8]) -> &mut Self {
        self.bytes[at..at + v.len()].copy_from_slice(v);
        self
    }
}

/// Old addresses for a test file, handed out in order.
#[derive(Debug, Clone, Default)]
pub struct Addresses {
    next: u64,
    named: HashMap<String, u64>,
}

impl Addresses {
    /// The address of `name`, handed out on first use.
    pub fn of(&mut self, name: &str) -> u64 {
        if let Some(a) = self.named.get(name) {
            return *a;
        }
        self.next += 0x40;
        let a = 0x1_0000_0000 + self.next;
        self.named.insert(name.to_owned(), a);
        a
    }
}
