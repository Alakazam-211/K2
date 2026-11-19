//! The zip layer of a package: a bounded zip reader, and a writer that keeps every untouched
//! entry byte for byte (the reason this isn't the `zip` crate: its raw copy rewrites headers).
//!
//! **What a byte-exact round trip needs.** A zip file is a run of local records (a header, the
//! compressed data, an optional data descriptor), then a central directory of one record per
//! entry, then an end record. Each of these carries fields a writer chooses: the version made
//! by and needed, the general-purpose flags (bits 1-2 record the deflate level the writer
//! asked for; bit 3 says a data descriptor follows the data, with or without its optional
//! signature; bit 11 says the name is UTF-8), the DOS time and date, extra fields (which may
//! differ between the local and the central record), per-entry comments, the internal and
//! external attributes, and the archive comment. The compressed bytes themselves depend on
//! the compressor and its level: two deflaters, or one at two levels, give different bytes for
//! the same content. So nothing can be re-encoded and come back the same. The writer instead
//! **copies**: an untouched entry's whole local record and its central record are copied as
//! they are, and only the central record's offset field is patched (it moves when an entry
//! before it changed size). The end record is the template's, with its counts, the
//! directory's size and offset patched, and its comment kept.
//!
//! **The reader accepts only a package it can reproduce exactly:** local records in central
//! directory order, each starting where the last ended, then the directory, then the end
//! record; so a package is its records and directory, and nothing hides between them. Then
//! [`Package::rewrite`] (every entry raw) gives back the input bytes, which callers check.
//!
//! **A changed entry** ([`Source::Replaced`]) keeps its local and central headers (name,
//! times, extra fields, comment, attributes, method) and gets new content, compressed again
//! with this crate's deflater when the entry was deflated; its data descriptor is dropped (the
//! sizes and CRC go in the headers). So its content is exact, and its compressed bytes are
//! ours, not the original writer's. **A new entry** ([`Source::New`]) is deflated with a fixed
//! layout (version 2.0, the DOS epoch as its time, no extra fields), so the same content gives
//! the same bytes every time.
//!
//! Refused (a typed [`Decline`], never a panic), each before anything it guards is allocated:
//!
//! | Code | What |
//! |---|---|
//! | `too_large`, `zip_entries`, `zip_ratio` | over a [`Limits`] bound (file, entry, total, count, ratio) |
//! | `not_zip` | no end record |
//! | `zip_layout` | a gap, data before the first record or after the end record, a directory out of place |
//! | `zip_overlap` | a record that starts inside the one before (nested entries, the classic bomb) |
//! | `zip_local_mismatch` | a local header that disagrees with the directory |
//! | `zip_descriptor` | a data descriptor that disagrees with the directory |
//! | `zip64`, `zip_multi_disk` | zip64 in any form (end records, sentinels, extra fields); split archives |
//! | `encrypted` | an entry's encryption flags |
//! | `zip_method` | anything but stored and deflate |
//! | `zip_name` | a name too long, not UTF-8 or ASCII, absolute, with a backslash, `.`, `..`, an empty or control-character segment |
//! | `zip_duplicate` | two entries with one name, case folded (part names compare so in OPC) |
//! | `zip_corrupt` | a truncated structure, or content that doesn't inflate to its size and CRC |

use std::collections::HashSet;
use std::io::{Read, Write};
use std::ops::Range;

use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;

use crate::error::Decline;
use crate::limits::Limits;
use crate::mutant;

const SIG_LOCAL: u32 = 0x0403_4B50;
const SIG_CENTRAL: u32 = 0x0201_4B50;
const SIG_EOCD: u32 = 0x0605_4B50;
const SIG_LOCATOR: u32 = 0x0706_4B50;
const SIG_DESCRIPTOR: u32 = 0x0807_4B50;
const EOCD_LEN: usize = 22;
const CENTRAL_LEN: usize = 46;
const LOCAL_LEN: usize = 30;
const FLAG_ENCRYPTED: u16 = 1;
const FLAG_DESCRIPTOR: u16 = 1 << 3;
const FLAG_STRONG: u16 = 1 << 6;
const FLAG_UTF8: u16 = 1 << 11;
const FLAG_MASKED: u16 = 1 << 13;
const METHOD_STORED: u16 = 0;
const METHOD_DEFLATE: u16 = 8;
const EXTRA_ZIP64: u16 = 1;

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    let s = b.get(i..i.checked_add(2)?)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    let s = b.get(i..i.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn truncated(what: &str) -> Decline {
    Decline::new("zip_corrupt", format!("truncated {what}"))
}

/// One entry, in directory order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub method: u16,
    pub flags: u16,
    pub crc: u32,
    pub compressed: u32,
    pub size: u32,
    /// The whole local record: header, data and any descriptor.
    record: Range<usize>,
    /// The local header's length (fixed part, name and extra).
    header_len: usize,
    /// The compressed data.
    data: Range<usize>,
    /// The central directory record.
    central: Range<usize>,
}

/// A package, borrowing the file's bytes.
#[derive(Debug, Clone)]
pub struct Package<'a> {
    bytes: &'a [u8],
    entries: Vec<Entry>,
    eocd: Range<usize>,
}

/// What [`write()`] puts in the package for one entry.
#[derive(Debug, Clone, Copy)]
pub enum Source<'p, 'a> {
    /// The entry's local record, copied byte for byte.
    Raw(&'p Package<'a>, usize),
    /// The entry's record and directory fields with new content (its method kept).
    Replaced(&'p Package<'a>, usize, &'p [u8]),
    /// A new entry: its name and content, deflated, with a fixed layout.
    New(&'p str, &'p [u8]),
}

fn has_zip64_extra(mut extra: &[u8]) -> bool {
    while let (Some(id), Some(len)) = (u16_at(extra, 0), u16_at(extra, 2)) {
        if id == EXTRA_ZIP64 {
            return true;
        }
        match extra.get(4 + len as usize..) {
            Some(rest) => extra = rest,
            None => return false,
        }
    }
    false
}

fn check_name(raw: &[u8], flags: u16, limits: &Limits) -> Result<String, Decline> {
    let bad = |why: &str| Decline::new("zip_name", why.to_owned());
    if raw.len() > limits.max_name_len {
        return Err(bad("an entry name is too long"));
    }
    if raw.is_empty() {
        return Err(bad("an entry has no name"));
    }
    if flags & FLAG_UTF8 == 0 && !raw.is_ascii() {
        return Err(bad("an entry name isn't UTF-8 or ASCII"));
    }
    let name = std::str::from_utf8(raw).map_err(|_| bad("an entry name isn't UTF-8"))?;
    if name.starts_with('/') {
        return Err(bad("an entry name is absolute"));
    }
    if name.contains('\\') {
        return Err(bad("an entry name holds a backslash"));
    }
    // a directory entry ends with one '/'
    let path = name.strip_suffix('/').unwrap_or(name);
    for seg in path.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(bad("an entry name escapes the package"));
        }
        if seg.chars().any(char::is_control) {
            return Err(bad("an entry name holds a control character"));
        }
    }
    Ok(name.to_owned())
}

/// Find the end record: the last signature whose comment runs exactly to the end of the file.
fn find_eocd(b: &[u8]) -> Result<usize, Decline> {
    if b.len() < EOCD_LEN {
        return Err(Decline::new("not_zip", "too short to be a zip file"));
    }
    let lowest = b.len().saturating_sub(EOCD_LEN + u16::MAX as usize);
    let mut seen = false;
    for i in (lowest..=b.len() - EOCD_LEN).rev() {
        if u32_at(b, i) == Some(SIG_EOCD) {
            seen = true;
            if u16_at(b, i + 20).map(|c| i + EOCD_LEN + c as usize) == Some(b.len()) {
                return Ok(i);
            }
        }
    }
    if seen {
        Err(Decline::new(
            "zip_layout",
            "bytes after the zip file's end record",
        ))
    } else {
        Err(Decline::new("not_zip", "no zip end record"))
    }
}

impl<'a> Package<'a> {
    /// Read a package. Refuses anything in the module's table, and checks the sizes every
    /// entry declares; [`Package::inflate`] holds the content to them.
    pub fn read(b: &'a [u8], limits: &Limits) -> Result<Package<'a>, Decline> {
        if b.len() > limits.max_file {
            return Err(Decline::new("too_large", "the file is over the size limit"));
        }
        let eocd = find_eocd(b)?;
        let field16 = |o: usize| u16_at(b, eocd + o).ok_or_else(|| truncated("end record"));
        let field32 = |o: usize| u32_at(b, eocd + o).ok_or_else(|| truncated("end record"));
        let (disk, cd_disk, here, total) = (field16(4)?, field16(6)?, field16(8)?, field16(10)?);
        let (cd_size, cd_off) = (field32(12)?, field32(16)?);
        if here == u16::MAX || total == u16::MAX || cd_size == u32::MAX || cd_off == u32::MAX {
            return Err(Decline::new("zip64", "a zip64 archive"));
        }
        if eocd >= 20 && u32_at(b, eocd - 20) == Some(SIG_LOCATOR) {
            return Err(Decline::new("zip64", "a zip64 locator"));
        }
        if disk != 0 || cd_disk != 0 || here != total {
            return Err(Decline::new("zip_multi_disk", "a split archive"));
        }
        if total as usize > limits.max_entries {
            return Err(Decline::new("zip_entries", "too many entries"));
        }
        let (cd_off, cd_size) = (cd_off as usize, cd_size as usize);
        if cd_off.checked_add(cd_size) != Some(eocd) {
            return Err(Decline::new(
                "zip_layout",
                "the central directory doesn't end at the end record",
            ));
        }
        let mut entries: Vec<Entry> = Vec::with_capacity(total as usize);
        let mut seen = HashSet::with_capacity(total as usize);
        let mut total_size = 0u64;
        let mut at = cd_off;
        for _ in 0..total {
            let e = Self::read_entry(b, &mut at, eocd, limits)?;
            if !seen.insert(e.name.to_ascii_lowercase()) && !mutant::active("zip_no_duplicates") {
                return Err(Decline::new(
                    "zip_duplicate",
                    format!("two entries named {:?}", e.name),
                ));
            }
            total_size = total_size.saturating_add(u64::from(e.size));
            if total_size > limits.max_total {
                return Err(Decline::new(
                    "too_large",
                    "the uncompressed total is over the limit",
                ));
            }
            entries.push(e);
        }
        if at != eocd {
            return Err(Decline::new(
                "zip_layout",
                "the central directory has more than its count",
            ));
        }
        // the records tile the file up to the directory, in directory order
        let mut end = 0;
        for e in &entries {
            if mutant::active("zip_no_tiling") {
                break;
            }
            if e.record.start < end {
                return Err(Decline::new(
                    "zip_overlap",
                    format!("the entry {:?} overlaps the one before", e.name),
                ));
            }
            if e.record.start > end {
                return Err(Decline::new(
                    "zip_layout",
                    format!("a gap before the entry {:?}", e.name),
                ));
            }
            end = e.record.end;
        }
        if end != cd_off && !mutant::active("zip_no_tiling") {
            return Err(Decline::new(
                "zip_layout",
                "bytes between the last entry and the directory",
            ));
        }
        Ok(Package {
            bytes: b,
            entries,
            eocd: eocd..b.len(),
        })
    }

    fn read_entry(
        b: &[u8],
        at: &mut usize,
        cd_end: usize,
        limits: &Limits,
    ) -> Result<Entry, Decline> {
        let start = *at;
        let f16 = |o: usize| u16_at(b, start + o).ok_or_else(|| truncated("directory record"));
        let f32_ = |o: usize| u32_at(b, start + o).ok_or_else(|| truncated("directory record"));
        if f32_(0)? != SIG_CENTRAL {
            return Err(Decline::new(
                "zip_layout",
                "not a central directory record where one should be",
            ));
        }
        let (flags, method) = (f16(8)?, f16(10)?);
        let (crc, compressed, size) = (f32_(16)?, f32_(20)?, f32_(24)?);
        let (nlen, xlen, clen) = (f16(28)? as usize, f16(30)? as usize, f16(32)? as usize);
        let (disk, offset) = (f16(34)?, f32_(42)?);
        let name_at = start + CENTRAL_LEN;
        let end = name_at + nlen + xlen + clen;
        if end > cd_end {
            return Err(truncated("directory record"));
        }
        *at = end;
        let raw_name = &b[name_at..name_at + nlen];
        let extra = &b[name_at + nlen..name_at + nlen + xlen];
        let name = check_name(raw_name, flags, limits)?;
        if compressed == u32::MAX
            || size == u32::MAX
            || offset == u32::MAX
            || has_zip64_extra(extra)
        {
            return Err(Decline::new("zip64", format!("{name:?} uses zip64")));
        }
        if disk != 0 {
            return Err(Decline::new("zip_multi_disk", "an entry on another disk"));
        }
        if flags & (FLAG_ENCRYPTED | FLAG_STRONG | FLAG_MASKED) != 0 {
            return Err(Decline::new("encrypted", format!("{name:?} is encrypted")));
        }
        if method != METHOD_STORED && method != METHOD_DEFLATE {
            return Err(Decline::new(
                "zip_method",
                format!("{name:?} uses compression method {method}"),
            ));
        }
        if method == METHOD_STORED && compressed != size {
            return Err(Decline::new(
                "zip_corrupt",
                format!("{name:?} is stored with two sizes"),
            ));
        }
        if u64::from(size) > limits.max_entry {
            return Err(Decline::new(
                "too_large",
                format!("{name:?} is over the entry limit"),
            ));
        }
        if u64::from(size) >= limits.ratio_floor
            && u64::from(size) / u64::from(compressed.max(1)) > limits.max_ratio
        {
            return Err(Decline::new(
                "zip_ratio",
                format!("{name:?} expands too far"),
            ));
        }
        Self::read_local(
            b,
            offset as usize,
            Entry {
                name,
                method,
                flags,
                crc,
                compressed,
                size,
                record: 0..0,
                header_len: 0,
                data: 0..0,
                central: start..end,
            },
        )
    }

    /// The entry's local record, held to its directory record.
    fn read_local(b: &[u8], off: usize, mut e: Entry) -> Result<Entry, Decline> {
        let mismatch = |why: &str| {
            Decline::new(
                "zip_local_mismatch",
                format!("{:?}'s local header {why}", e.name),
            )
        };
        let f16 = |o: usize| u16_at(b, off + o).ok_or_else(|| truncated("local header"));
        let f32_ = |o: usize| u32_at(b, off + o).ok_or_else(|| truncated("local header"));
        if f32_(0)? != SIG_LOCAL {
            return Err(mismatch("is missing"));
        }
        let (lflags, lmethod) = (f16(6)?, f16(8)?);
        let (lcrc, lcomp, lsize) = (f32_(14)?, f32_(18)?, f32_(22)?);
        let (nlen, xlen) = (f16(26)? as usize, f16(28)? as usize);
        let name_at = off + LOCAL_LEN;
        let data_at = name_at + nlen + xlen;
        if data_at > b.len() {
            return Err(truncated("local header"));
        }
        if &b[name_at..name_at + nlen] != e.name.as_bytes() {
            return Err(mismatch("names another file"));
        }
        if lmethod != e.method {
            return Err(mismatch("has another method"));
        }
        if lflags & (FLAG_ENCRYPTED | FLAG_STRONG | FLAG_MASKED) != 0 {
            return Err(Decline::new(
                "encrypted",
                format!("{:?} is encrypted", e.name),
            ));
        }
        if lflags & FLAG_DESCRIPTOR != e.flags & FLAG_DESCRIPTOR {
            return Err(mismatch("disagrees on the data descriptor"));
        }
        if has_zip64_extra(&b[name_at + nlen..data_at]) {
            return Err(Decline::new("zip64", format!("{:?} uses zip64", e.name)));
        }
        let descriptor = e.flags & FLAG_DESCRIPTOR != 0;
        if !descriptor && (lcrc != e.crc || lcomp != e.compressed || lsize != e.size) {
            return Err(mismatch("has other sizes or another CRC"));
        }
        let data_end = data_at
            .checked_add(e.compressed as usize)
            .filter(|&x| x <= b.len())
            .ok_or_else(|| truncated("entry data"))?;
        let mut end = data_end;
        if descriptor {
            let agrees = |i: usize| {
                u32_at(b, i) == Some(e.crc)
                    && u32_at(b, i + 4) == Some(e.compressed)
                    && u32_at(b, i + 8) == Some(e.size)
            };
            if u32_at(b, data_end) == Some(SIG_DESCRIPTOR) && agrees(data_end + 4) {
                end = data_end + 16;
            } else if agrees(data_end) {
                end = data_end + 12;
            } else {
                return Err(Decline::new(
                    "zip_descriptor",
                    format!(
                        "{:?}'s data descriptor disagrees with the directory",
                        e.name
                    ),
                ));
            }
        }
        e.record = off..end;
        e.header_len = data_at - off;
        e.data = data_at..data_end;
        Ok(e)
    }

    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// An entry's whole local record, as it is in the file.
    pub fn record_bytes(&self, i: usize) -> Option<&'a [u8]> {
        self.entries.get(i).map(|e| &self.bytes[e.record.clone()])
    }

    /// An entry's index by name, case folded.
    pub fn find(&self, name: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.name.eq_ignore_ascii_case(name))
    }

    /// An entry's content, held to its declared size and CRC. Never allocates past the size
    /// the directory declared, which [`Package::read`] has bounded.
    pub fn inflate(&self, i: usize) -> Result<Vec<u8>, Decline> {
        let e = self
            .entries
            .get(i)
            .ok_or_else(|| Decline::new("zip_corrupt", "no such entry"))?;
        let raw = &self.bytes[e.data.clone()];
        let corrupt = |why: &str| Decline::new("zip_corrupt", format!("{:?} {why}", e.name));
        let out = if e.method == METHOD_STORED {
            raw.to_vec()
        } else {
            let mut out = Vec::with_capacity(e.size as usize);
            DeflateDecoder::new(raw)
                .take(u64::from(e.size) + 1)
                .read_to_end(&mut out)
                .map_err(|_| corrupt("doesn't inflate"))?;
            out
        };
        if out.len() != e.size as usize {
            return Err(corrupt("isn't the size it declares"));
        }
        if crc32fast::hash(&out) != e.crc && !mutant::active("zip_no_crc") {
            return Err(corrupt("fails its CRC"));
        }
        Ok(out)
    }

    /// The package written back with every entry raw: the file's bytes, exactly.
    pub fn rewrite(&self) -> Result<Vec<u8>, Decline> {
        let items: Vec<Source<'_, 'a>> = (0..self.entries.len())
            .map(|i| Source::Raw(self, i))
            .collect();
        write(self, &items)
    }
}

fn deflate(content: &[u8]) -> Result<Vec<u8>, Decline> {
    let mut w = DeflateEncoder::new(Vec::new(), Compression::default());
    w.write_all(content)
        .map_err(|_| Decline::new("zip_corrupt", "can't deflate"))?;
    w.finish()
        .map_err(|_| Decline::new("zip_corrupt", "can't deflate"))
}

fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn to_u32(v: usize) -> Result<u32, Decline> {
    u32::try_from(v).map_err(|_| Decline::new("too_large", "a package over 4 GiB"))
}

/// Write a package: `items` in order, each record and directory record from its source, only
/// offsets patched for a raw entry; the end record is `template`'s (its comment kept), with
/// the counts and offsets of what was written.
pub fn write(template: &Package<'_>, items: &[Source<'_, '_>]) -> Result<Vec<u8>, Decline> {
    let mut out = Vec::new();
    let mut directory = Vec::new();
    for item in items {
        let offset = to_u32(out.len())?;
        let (pkg, i, new) = match *item {
            Source::Raw(p, i) if !mutant::active("zip_recompress_all") => (p, i, None),
            Source::Raw(p, i) => {
                let c = p.inflate(i)?;
                let e = &p.entries[i];
                write_replaced(&mut out, &mut directory, p, e, &c, offset)?;
                continue;
            }
            Source::Replaced(p, i, c) => (p, i, Some(c)),
            Source::New(name, c) => {
                write_new(&mut out, &mut directory, name, c, offset)?;
                continue;
            }
        };
        let e = pkg
            .entries
            .get(i)
            .ok_or_else(|| Decline::new("zip_corrupt", "no such entry"))?;
        if let Some(c) = new {
            write_replaced(&mut out, &mut directory, pkg, e, c, offset)?;
        } else {
            out.extend_from_slice(&pkg.bytes[e.record.clone()]);
            let at = directory.len();
            directory.extend_from_slice(&pkg.bytes[e.central.clone()]);
            put32(&mut directory, at + 42, offset);
        }
    }
    let cd_off = to_u32(out.len())?;
    let cd_size = to_u32(directory.len())?;
    let count = u16::try_from(items.len())
        .map_err(|_| Decline::new("zip_entries", "too many entries to write"))?;
    out.extend_from_slice(&directory);
    let at = out.len();
    out.extend_from_slice(&template.bytes[template.eocd.clone()]);
    put16(&mut out, at + 8, count);
    put16(&mut out, at + 10, count);
    put32(&mut out, at + 12, cd_size);
    put32(&mut out, at + 16, cd_off);
    Ok(out)
}

/// One range of a package's bytes and what replaces it: the `len` bytes at `at` become `with`
/// (`len` 0: `with` is inserted at `at`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub at: usize,
    pub len: usize,
    pub with: Vec<u8>,
}

/// What [`write()`] gives for `items`, as spans over `template`'s bytes: in order of `at`,
/// apart (two ranges that would touch are one), inside the file. An entry left out is its record's
/// range replaced by nothing; a replaced entry is its record's range replaced by its new
/// record; a new entry is inserted where it goes; the directory and the end record are one
/// range replaced by the new ones (with any new entries at the end before them). Every other
/// byte is the template's, copied in place. Each [`Source::Raw`] and [`Source::Replaced`] must
/// be the template's own entry, in its order, each at most once (`zip_layout` otherwise: such
/// a write isn't the template with ranges replaced).
pub fn write_plan(template: &Package<'_>, items: &[Source<'_, '_>]) -> Result<Vec<Span>, Decline> {
    let not_spliced = || {
        Decline::new(
            "zip_layout",
            "a write that isn't the package with ranges replaced",
        )
    };
    let entries = &template.entries;
    // where the directory starts: new entries at the end are inserted there
    let cd_start = template.eocd.start - directory_len(template);
    let mut spans: Vec<Span> = Vec::new();
    let mut directory = Vec::new();
    // the output's length so far, and the next template entry not yet placed
    let (mut written, mut next) = (0usize, 0usize);
    let left_out = |spans: &mut Vec<Span>, from: usize, to: usize| {
        for e in &entries[from..to] {
            push_span(spans, e.record.start, e.record.len(), Vec::new());
        }
    };
    for item in items {
        let own = match *item {
            Source::Raw(p, i) | Source::Replaced(p, i, _) => {
                if !std::ptr::eq(p.bytes, template.bytes) || i < next || i >= entries.len() {
                    return Err(not_spliced());
                }
                left_out(&mut spans, next, i);
                next = i + 1;
                Some(&entries[i])
            }
            Source::New(..) => None,
        };
        let offset = to_u32(written)?;
        match (emit(item, offset, &mut directory)?, own) {
            (Record::Copied(r), _) => written += r.len(),
            (Record::Written(b), Some(e)) => {
                written += b.len();
                push_span(&mut spans, e.record.start, e.record.len(), b);
            }
            (Record::Written(b), None) => {
                written += b.len();
                // inserted before the next template entry's record, or before the directory
                let at = entries.get(next).map_or(cd_start, |e| e.record.start);
                push_span(&mut spans, at, 0, b);
            }
        }
    }
    left_out(&mut spans, next, entries.len());
    let tail = tail(template, items.len(), to_u32(written)?, directory)?;
    push_span(&mut spans, cd_start, template.bytes.len() - cd_start, tail);
    Ok(spans)
}

/// `bytes` with each of `spans` applied: every byte outside them copied, each range replaced
/// by its bytes. The spans must be in order of `at`, not overlapping, inside `bytes`
/// (`zip_layout` otherwise).
pub fn apply(bytes: &[u8], spans: &[Span]) -> Result<Vec<u8>, Decline> {
    let bad = || Decline::new("zip_layout", "ranges out of order or outside the file");
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0usize;
    for s in spans {
        let end = s.at.checked_add(s.len).ok_or_else(bad)?;
        if s.at < at || end > bytes.len() {
            return Err(bad());
        }
        out.extend_from_slice(&bytes[at..s.at]);
        out.extend_from_slice(&s.with);
        at = end;
    }
    out.extend_from_slice(&bytes[at..]);
    Ok(out)
}

/// Add a range after the last: one that touches it joins it.
fn push_span(spans: &mut Vec<Span>, at: usize, len: usize, with: Vec<u8>) {
    if len == 0 && with.is_empty() {
        return;
    }
    if let Some(last) = spans.last_mut() {
        let end = last.at + last.len;
        // planted: a range that doesn't touch the last joins it anyway, dropping the template's
        // bytes between them
        if end == at || (end < at && mutant::active("opc_plan_skips_gap")) {
            last.len = at + len - last.at;
            last.with.extend_from_slice(&with);
            return;
        }
    }
    spans.push(Span { at, len, with });
}

/// The template's directory's length: from its offset to the end record.
fn directory_len(template: &Package<'_>) -> usize {
    template.entries.iter().map(|e| e.central.len()).sum()
}

/// One entry's local record as [`write`] puts it: the template's bytes, copied, or new bytes.
enum Record<'a> {
    Copied(&'a [u8]),
    Written(Vec<u8>),
}

/// One item's local record (written at `offset`), its directory record appended to
/// `directory`.
fn emit<'a>(
    item: &Source<'_, 'a>,
    offset: u32,
    directory: &mut Vec<u8>,
) -> Result<Record<'a>, Decline> {
    let mut rec = Vec::new();
    match *item {
        Source::Raw(p, i) if !mutant::active("zip_recompress_all") => {
            let e = p
                .entries
                .get(i)
                .ok_or_else(|| Decline::new("zip_corrupt", "no such entry"))?;
            let at = directory.len();
            directory.extend_from_slice(&p.bytes[e.central.clone()]);
            put32(directory, at + 42, offset);
            return Ok(Record::Copied(&p.bytes[e.record.clone()]));
        }
        Source::Raw(p, i) => {
            let c = p.inflate(i)?;
            let e = &p.entries[i];
            write_replaced(&mut rec, directory, p, e, &c, offset)?;
        }
        Source::Replaced(p, i, c) => {
            let e = p
                .entries
                .get(i)
                .ok_or_else(|| Decline::new("zip_corrupt", "no such entry"))?;
            write_replaced(&mut rec, directory, p, e, c, offset)?;
        }
        Source::New(name, c) => write_new(&mut rec, directory, name, c, offset)?,
    }
    Ok(Record::Written(rec))
}

/// The directory, then the template's end record (its comment kept) with the count, the
/// directory's size and its offset (`cd_off`) of what was written.
fn tail(
    template: &Package<'_>,
    count: usize,
    cd_off: u32,
    mut directory: Vec<u8>,
) -> Result<Vec<u8>, Decline> {
    let cd_size = to_u32(directory.len())?;
    let count = u16::try_from(count)
        .map_err(|_| Decline::new("zip_entries", "too many entries to write"))?;
    let at = directory.len();
    directory.extend_from_slice(&template.bytes[template.eocd.clone()]);
    put16(&mut directory, at + 8, count);
    put16(&mut directory, at + 10, count);
    put32(&mut directory, at + 12, cd_size);
    put32(&mut directory, at + 16, cd_off);
    Ok(directory)
}

fn write_replaced(
    out: &mut Vec<u8>,
    directory: &mut Vec<u8>,
    pkg: &Package<'_>,
    e: &Entry,
    content: &[u8],
    offset: u32,
) -> Result<(), Decline> {
    let data = if e.method == METHOD_STORED {
        content.to_vec()
    } else {
        deflate(content)?
    };
    let crc = crc32fast::hash(content);
    let (csize, size) = (to_u32(data.len())?, to_u32(content.len())?);
    let flags = e.flags & !FLAG_DESCRIPTOR;
    let at = out.len();
    out.extend_from_slice(&pkg.bytes[e.record.start..e.record.start + e.header_len]);
    put16(out, at + 6, flags);
    put32(out, at + 14, crc);
    put32(out, at + 18, csize);
    put32(out, at + 22, size);
    out.extend_from_slice(&data);
    let at = directory.len();
    directory.extend_from_slice(&pkg.bytes[e.central.clone()]);
    put16(directory, at + 8, flags);
    put32(directory, at + 16, crc);
    put32(directory, at + 20, csize);
    put32(directory, at + 24, size);
    put32(directory, at + 42, offset);
    Ok(())
}

/// Version 2.0: what deflate needs.
const VERSION_20: u16 = 20;
/// 1980-01-01, the DOS epoch: a fixed time, so a new entry's bytes depend only on its content.
const DOS_DATE_EPOCH: u16 = (1 << 5) | 1;

fn write_new(
    out: &mut Vec<u8>,
    directory: &mut Vec<u8>,
    name: &str,
    content: &[u8],
    offset: u32,
) -> Result<(), Decline> {
    // what the reader would refuse, the writer never writes
    check_name(name.as_bytes(), FLAG_UTF8, &Limits::default())?;
    let nlen = u16::try_from(name.len())
        .map_err(|_| Decline::new("zip_name", "a new entry's name is too long"))?;
    let data = deflate(content)?;
    let crc = crc32fast::hash(content);
    let (csize, size) = (to_u32(data.len())?, to_u32(content.len())?);
    let flags = if name.is_ascii() { 0 } else { FLAG_UTF8 };
    let mut local = vec![0u8; LOCAL_LEN];
    put32(&mut local, 0, SIG_LOCAL);
    put16(&mut local, 4, VERSION_20);
    put16(&mut local, 6, flags);
    put16(&mut local, 8, METHOD_DEFLATE);
    put16(&mut local, 10, 0);
    put16(&mut local, 12, DOS_DATE_EPOCH);
    put32(&mut local, 14, crc);
    put32(&mut local, 18, csize);
    put32(&mut local, 22, size);
    put16(&mut local, 26, nlen);
    put16(&mut local, 28, 0);
    out.extend_from_slice(&local);
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(&data);
    let mut central = vec![0u8; CENTRAL_LEN];
    put32(&mut central, 0, SIG_CENTRAL);
    put16(&mut central, 4, VERSION_20);
    put16(&mut central, 6, VERSION_20);
    put16(&mut central, 8, flags);
    put16(&mut central, 10, METHOD_DEFLATE);
    put16(&mut central, 12, 0);
    put16(&mut central, 14, DOS_DATE_EPOCH);
    put32(&mut central, 16, crc);
    put32(&mut central, 20, csize);
    put32(&mut central, 24, size);
    put16(&mut central, 28, nlen);
    put32(&mut central, 42, offset);
    directory.extend_from_slice(&central);
    directory.extend_from_slice(name.as_bytes());
    Ok(())
}
