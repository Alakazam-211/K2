//! The strict zip subset of format v1. STORE and DEFLATE only; no ZIP64, encryption, data
//! descriptors or multi-disk; every byte of the file belongs to a header or an entry. Sizes are
//! judged from the central directory before anything is inflated, and inflating is held to the
//! declared size and the ratio limit while it streams, because declared sizes can lie.

use crate::limits::Limits;
use crate::refusal::{Refusal, RefusalCode as C};
use miniz_oxide::deflate::compress_to_vec;
use miniz_oxide::inflate::stream::{inflate, InflateState};
use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};
use std::collections::BTreeSet;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_END: u32 = 0x0605_4b50;
const SIG_ZIP64_LOCATOR: u32 = 0x0706_4b50;
const END_LEN: u64 = 22;
const CENTRAL_LEN: usize = 46;
const LOCAL_LEN: usize = 30;
/// The central directory's size is capped so a forged one can't take much memory.
const CENTRAL_MAX: u64 = 64 * 1024 * 1024;
const CHUNK: usize = 64 * 1024;
/// A part name's limits.
const NAME_MAX_BYTES: usize = 128;
const SEGMENT_MAX_BYTES: usize = 80;
const SEGMENTS_MAX: usize = 4;
/// Flags a reader accepts: the deflate level hints and the UTF-8 name flag.
const FLAGS_OK: u16 = 0b0000_1000_0000_0110;
/// Encryption, strong encryption, and masked headers.
const FLAGS_ENCRYPTED: u16 = 0b0010_0000_0100_0001;

pub(crate) const METHOD_STORE: u16 = 0;
pub(crate) const METHOD_DEFLATE: u16 = 8;

fn le16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn le32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn read_err(e: std::io::Error) -> Refusal {
    if e.kind() == ErrorKind::UnexpectedEof {
        Refusal::new(C::ZipMalformed, "the file ends inside a record")
    } else {
        Refusal::new(C::IoError, e.to_string())
    }
}

fn read_at<R: Read + Seek>(src: &mut R, offset: u64, buf: &mut [u8]) -> Result<(), Refusal> {
    src.seek(SeekFrom::Start(offset)).map_err(read_err)?;
    src.read_exact(buf).map_err(read_err)
}

/// One entry, checked against its local header and the file's layout.
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub name: String,
    pub method: u16,
    pub crc: u32,
    pub csize: u64,
    pub usize_: u64,
    pub data_offset: u64,
}

/// A legal part name: 1 to 4 segments of `[A-Za-z0-9._-]`, no dot at either end of a segment, no
/// name Windows reserves. That rules out traversal, absolute paths, backslashes and directories.
pub(crate) fn name_ok(name: &str) -> bool {
    if name.len() > NAME_MAX_BYTES {
        return false;
    }
    let segments: Vec<&str> = name.split('/').collect();
    if segments.len() > SEGMENTS_MAX {
        return false;
    }
    segments.iter().all(|seg| {
        !seg.is_empty()
            && seg.len() <= SEGMENT_MAX_BYTES
            && !seg.starts_with('.')
            && !seg.ends_with('.')
            && seg
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
            && !windows_reserved(seg)
    })
}

fn windows_reserved(seg: &str) -> bool {
    let stem = seg.split('.').next().unwrap_or(seg).to_ascii_lowercase();
    match stem.as_str() {
        "con" | "prn" | "aux" | "nul" => true,
        _ => {
            let b = stem.as_bytes();
            b.len() == 4
                && (stem.starts_with("com") || stem.starts_with("lpt"))
                && b[3].is_ascii_digit()
        }
    }
}

fn part_limit(name: &str, l: &Limits) -> u64 {
    match name {
        "manifest.json" => l.manifest_bytes,
        "board.json" => l.board_json_bytes,
        _ => l.part_bytes,
    }
}

fn check_extra(extra: &[u8], name: &str) -> Result<(), Refusal> {
    let mut p = 0;
    while p < extra.len() {
        if p + 4 > extra.len() {
            return Err(Refusal::at(C::ZipMalformed, name, "a broken extra field"));
        }
        let id = le16(extra, p);
        let len = usize::from(le16(extra, p + 2));
        if id == 1 {
            return Err(Refusal::at(C::Zip64, name, "a ZIP64 extra field"));
        }
        p += 4 + len;
        if p > extra.len() {
            return Err(Refusal::at(C::ZipMalformed, name, "a broken extra field"));
        }
    }
    Ok(())
}

/// Output so far against input so far.
fn ratio_ok(out: u64, consumed: u64, l: &Limits) -> bool {
    out <= l.ratio_floor || out <= consumed.saturating_mul(l.ratio)
}

struct Central {
    entry: Entry,
    local_offset: u64,
    flags: u16,
    raw_name: Vec<u8>,
}

/// Reads and checks the end record, the central directory, every local header and the layout.
pub(crate) fn read_index<R: Read + Seek>(src: &mut R, l: &Limits) -> Result<Vec<Entry>, Refusal> {
    let len = src.seek(SeekFrom::End(0)).map_err(read_err)?;
    let no_end = || Refusal::new(C::ZipNoEndRecord, "no end record at the end of the file");
    if len < END_LEN {
        return Err(no_end());
    }
    let end_at = len - END_LEN;
    let mut end = [0u8; END_LEN as usize];
    read_at(src, end_at, &mut end)?;
    if le32(&end, 0) != SIG_END || le16(&end, 20) != 0 {
        // Is there an end record further back? Then the bytes after it are stray.
        let tail_len = len.min(END_LEN + 65_535);
        let mut tail = vec![0u8; tail_len as usize];
        read_at(src, len - tail_len, &mut tail)?;
        let sig = SIG_END.to_le_bytes();
        return Err(if tail.windows(4).any(|w| w == sig) {
            Refusal::new(C::ZipStrayBytes, "bytes after the end record")
        } else {
            no_end()
        });
    }
    let (disk, central_disk) = (le16(&end, 4), le16(&end, 6));
    let (here, total) = (le16(&end, 8), le16(&end, 10));
    let (central_size, central_at) = (le32(&end, 12), le32(&end, 16));
    if here == u16::MAX || total == u16::MAX || central_size == u32::MAX || central_at == u32::MAX {
        return Err(Refusal::new(C::Zip64, "a ZIP64 marker in the end record"));
    }
    if disk != 0 || central_disk != 0 || here != total {
        return Err(Refusal::new(C::ZipMultiDisk, "more than one disk"));
    }
    if u64::from(total) > l.entries {
        return Err(Refusal::new(
            C::LimitEntries,
            format!("{total} entries, above {}", l.entries),
        ));
    }
    let central_at = u64::from(central_at);
    let central_size = u64::from(central_size);
    let central_end = central_at + central_size;
    if central_end > end_at {
        return Err(Refusal::new(
            C::ZipMalformed,
            "the central directory passes the end record",
        ));
    }
    if central_end < end_at {
        if end_at - central_end == 20 {
            let mut sig = [0u8; 4];
            read_at(src, central_end, &mut sig)?;
            if le32(&sig, 0) == SIG_ZIP64_LOCATOR {
                return Err(Refusal::new(C::Zip64, "a ZIP64 locator"));
            }
        }
        return Err(Refusal::new(
            C::ZipStrayBytes,
            "bytes between the central directory and the end record",
        ));
    }
    if central_size > CENTRAL_MAX {
        return Err(Refusal::new(
            C::LimitEntries,
            format!("a central directory over {CENTRAL_MAX} bytes"),
        ));
    }
    let mut central = vec![0u8; central_size as usize];
    read_at(src, central_at, &mut central)?;

    let mut entries: Vec<Central> = Vec::with_capacity(usize::from(total));
    let mut seen = BTreeSet::new();
    let mut total_bytes = 0u64;
    let mut p = 0usize;
    for _ in 0..total {
        let c = parse_central(&central, &mut p)?;
        let name = &c.entry.name;
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(Refusal::at(
                C::ZipNameDuplicate,
                name.as_str(),
                "two entries with one name, exactly or by case",
            ));
        }
        let max = part_limit(name, l);
        if c.entry.usize_ > max {
            return Err(Refusal::at(
                C::LimitPartBytes,
                name.as_str(),
                format!("{} bytes, above {max}", c.entry.usize_),
            ));
        }
        total_bytes += c.entry.usize_;
        if total_bytes > l.total_bytes {
            return Err(Refusal::new(
                C::LimitTotalBytes,
                format!("over {} bytes in all", l.total_bytes),
            ));
        }
        if !ratio_ok(c.entry.usize_, c.entry.csize, l) {
            return Err(Refusal::at(
                C::LimitRatio,
                name.as_str(),
                format!("declares more than {}:1", l.ratio),
            ));
        }
        entries.push(c);
    }
    if p != central.len() {
        return Err(Refusal::new(
            C::ZipMalformed,
            "the central directory holds more than its count says",
        ));
    }

    for c in &mut entries {
        check_local(src, c, central_at)?;
    }
    check_layout(&entries, central_at)?;
    Ok(entries.into_iter().map(|c| c.entry).collect())
}

fn parse_central(b: &[u8], p: &mut usize) -> Result<Central, Refusal> {
    let short = || Refusal::new(C::ZipMalformed, "a short central directory");
    let h = *p;
    if h + CENTRAL_LEN > b.len() {
        return Err(short());
    }
    if le32(b, h) != SIG_CENTRAL {
        return Err(Refusal::new(C::ZipMalformed, "a bad central header"));
    }
    let made_by_host = b[h + 5];
    let flags = le16(b, h + 8);
    let method = le16(b, h + 10);
    let crc = le32(b, h + 16);
    let csize = le32(b, h + 20);
    let usize_ = le32(b, h + 24);
    let name_len = usize::from(le16(b, h + 28));
    let extra_len = usize::from(le16(b, h + 30));
    let comment_len = usize::from(le16(b, h + 32));
    let disk_start = le16(b, h + 34);
    let external = le32(b, h + 38);
    let local_offset = le32(b, h + 42);
    let name_at = h + CENTRAL_LEN;
    let next = name_at + name_len + extra_len + comment_len;
    if next > b.len() {
        return Err(short());
    }
    *p = next;
    let raw_name = b[name_at..name_at + name_len].to_vec();
    let printable = String::from_utf8_lossy(&raw_name).into_owned();
    let extra = &b[name_at + name_len..name_at + name_len + extra_len];

    if flags & FLAGS_ENCRYPTED != 0 {
        return Err(Refusal::at(C::ZipEncrypted, printable, "encrypted"));
    }
    if flags & !FLAGS_OK != 0 {
        return Err(Refusal::at(
            C::ZipFlags,
            printable,
            format!("flags {flags:#06x}: a data descriptor or something unknown"),
        ));
    }
    if csize == u32::MAX || usize_ == u32::MAX || local_offset == u32::MAX {
        return Err(Refusal::at(C::Zip64, printable, "a ZIP64 marker"));
    }
    check_extra(extra, &printable)?;
    if disk_start != 0 {
        return Err(Refusal::at(
            C::ZipMultiDisk,
            printable,
            "starts on another disk",
        ));
    }
    if method != METHOD_STORE && method != METHOD_DEFLATE {
        return Err(Refusal::at(
            C::ZipMethod,
            printable,
            format!("method {method}"),
        ));
    }
    if comment_len != 0 {
        return Err(Refusal::at(C::ZipMalformed, printable, "an entry comment"));
    }
    const FILE_TYPE: u32 = 0o170_000;
    const REGULAR: u32 = 0o100_000;
    let unix_type = (external >> 16) & FILE_TYPE;
    let not_regular = (made_by_host == 3 && unix_type != 0 && unix_type != REGULAR)
        || external & 0x10 != 0   // the directory attribute
        || external & 0x400 != 0; // a Windows reparse point
    if not_regular {
        return Err(Refusal::at(
            C::ZipNotRegular,
            printable,
            "not a regular file",
        ));
    }
    let name = match std::str::from_utf8(&raw_name) {
        Ok(n) if name_ok(n) => n.to_string(),
        _ => {
            return Err(Refusal::at(
                C::ZipNameInvalid,
                printable,
                "not a legal part name",
            ))
        }
    };
    if method == METHOD_STORE && csize != usize_ {
        return Err(Refusal::at(
            C::ZipSizeMismatch,
            name,
            "a stored entry's two sizes differ",
        ));
    }
    Ok(Central {
        entry: Entry {
            name,
            method,
            crc,
            csize: u64::from(csize),
            usize_: u64::from(usize_),
            data_offset: 0,
        },
        local_offset: u64::from(local_offset),
        flags,
        raw_name,
    })
}

fn check_local<R: Read + Seek>(
    src: &mut R,
    c: &mut Central,
    central_at: u64,
) -> Result<(), Refusal> {
    let name = c.entry.name.clone();
    let malformed = || Refusal::at(C::ZipMalformed, name.as_str(), "a bad local header");
    if c.local_offset + LOCAL_LEN as u64 > central_at {
        return Err(malformed());
    }
    let mut h = [0u8; LOCAL_LEN];
    read_at(src, c.local_offset, &mut h)?;
    if le32(&h, 0) != SIG_LOCAL {
        return Err(malformed());
    }
    let name_len = usize::from(le16(&h, 26));
    let extra_len = usize::from(le16(&h, 28));
    let mut tail = vec![0u8; name_len + extra_len];
    src.read_exact(&mut tail).map_err(read_err)?;
    let same = le16(&h, 6) == c.flags
        && le16(&h, 8) == c.entry.method
        && le32(&h, 14) == c.entry.crc
        && u64::from(le32(&h, 18)) == c.entry.csize
        && u64::from(le32(&h, 22)) == c.entry.usize_
        && tail[..name_len] == c.raw_name[..];
    if !same {
        return Err(Refusal::at(
            C::ZipHeaderMismatch,
            name,
            "the local header and the central directory disagree",
        ));
    }
    check_extra(&tail[name_len..], &name)?;
    c.entry.data_offset = c.local_offset + (LOCAL_LEN + name_len + extra_len) as u64;
    Ok(())
}

/// Entries must fill the file from its first byte to the central directory, with nothing shared
/// and nothing left over.
fn check_layout(entries: &[Central], central_at: u64) -> Result<(), Refusal> {
    let mut order: Vec<&Central> = entries.iter().collect();
    order.sort_by_key(|c| c.local_offset);
    let mut expected = 0u64;
    for c in order {
        if c.local_offset < expected {
            return Err(Refusal::at(
                C::ZipOverlap,
                c.entry.name.as_str(),
                "shares bytes with another entry",
            ));
        }
        if c.local_offset > expected {
            return Err(Refusal::at(
                C::ZipStrayBytes,
                c.entry.name.as_str(),
                "bytes before this entry belong to none",
            ));
        }
        expected = c.entry.data_offset + c.entry.csize;
    }
    if expected > central_at {
        return Err(Refusal::new(
            C::ZipOverlap,
            "an entry runs into the central directory",
        ));
    }
    if expected < central_at {
        return Err(Refusal::new(
            C::ZipStrayBytes,
            "bytes before the central directory belong to no entry",
        ));
    }
    Ok(())
}

/// Reads one entry's bytes, holding it to its declared size, the ratio limit and its CRC.
pub(crate) fn read_entry<R: Read + Seek>(
    src: &mut R,
    e: &Entry,
    l: &Limits,
) -> Result<Vec<u8>, Refusal> {
    src.seek(SeekFrom::Start(e.data_offset)).map_err(read_err)?;
    let out = if e.method == METHOD_STORE {
        let mut out = vec![0u8; e.csize as usize];
        src.read_exact(&mut out).map_err(read_err)?;
        out
    } else {
        inflate_entry(src, e, l)?
    };
    if crc32(&out) != e.crc {
        return Err(Refusal::at(
            C::ZipCrc,
            e.name.as_str(),
            "the CRC doesn't match",
        ));
    }
    Ok(out)
}

fn inflate_entry<R: Read>(src: &mut R, e: &Entry, l: &Limits) -> Result<Vec<u8>, Refusal> {
    let mut input = vec![0u8; CHUNK];
    let mut remaining = e.csize;
    let mut sink = Vec::with_capacity((e.usize_ as usize).min(1 << 20));
    let mut feed = |buf: &mut [u8]| -> Result<usize, Refusal> {
        let n = remaining.min(buf.len() as u64) as usize;
        src.read_exact(&mut buf[..n]).map_err(read_err)?;
        remaining -= n as u64;
        Ok(n)
    };
    inflate_checked(
        &mut feed, &mut input, e.csize, e.usize_, &mut sink, &e.name, l,
    )?;
    Ok(sink)
}

/// The inflate loop, shared by the reader and by the writer's check of its own output.
fn inflate_checked(
    feed: &mut dyn FnMut(&mut [u8]) -> Result<usize, Refusal>,
    input: &mut [u8],
    csize: u64,
    declared: u64,
    out: &mut Vec<u8>,
    name: &str,
    l: &Limits,
) -> Result<(), Refusal> {
    let invalid = || Refusal::at(C::ZipDeflateInvalid, name, "broken deflate data");
    let mismatch = || {
        Refusal::at(
            C::ZipSizeMismatch,
            name,
            "the data doesn't end at its declared size",
        )
    };
    let mut state = InflateState::new_boxed(DataFormat::Raw);
    let mut output = vec![0u8; CHUNK];
    let (mut pos, mut have) = (0usize, 0usize);
    let mut consumed = 0u64;
    let mut read_total = 0u64;
    loop {
        if pos == have && read_total < csize {
            have = feed(input)?;
            read_total += have as u64;
            pos = 0;
        }
        let r = inflate(&mut state, &input[pos..have], &mut output, MZFlush::None);
        pos += r.bytes_consumed;
        consumed += r.bytes_consumed as u64;
        if out.len() as u64 + r.bytes_written as u64 > declared {
            return Err(mismatch());
        }
        out.extend_from_slice(&output[..r.bytes_written]);
        if !ratio_ok(out.len() as u64, consumed, l) {
            return Err(Refusal::at(
                C::LimitRatio,
                name,
                format!("inflates past {}:1", l.ratio),
            ));
        }
        match r.status {
            Ok(MZStatus::StreamEnd) => break,
            Ok(_) | Err(MZError::Buf) => {
                let stuck = r.bytes_consumed == 0 && r.bytes_written == 0;
                if stuck && pos == have && read_total == csize {
                    return Err(invalid());
                }
            }
            Err(_) => return Err(invalid()),
        }
    }
    if out.len() as u64 != declared || consumed != csize {
        return Err(mismatch());
    }
    Ok(())
}

/// Deflates data for writing, or says to store it: stored when deflate saves nothing, or when the
/// result would fail this crate's own ratio check on the way back in.
pub(crate) fn deflate_for_write(data: &[u8], l: &Limits) -> Option<Vec<u8>> {
    let packed = compress_to_vec(data, 6);
    if packed.len() >= data.len() {
        return None;
    }
    if data.len() as u64 > l.ratio_floor {
        let mut offset = 0usize;
        let mut feed = |buf: &mut [u8]| -> Result<usize, Refusal> {
            let n = (packed.len() - offset).min(buf.len());
            buf[..n].copy_from_slice(&packed[offset..offset + n]);
            offset += n;
            Ok(n)
        };
        let mut input = vec![0u8; CHUNK];
        let mut sink = Vec::with_capacity(data.len());
        let check = inflate_checked(
            &mut feed,
            &mut input,
            packed.len() as u64,
            data.len() as u64,
            &mut sink,
            "board.json",
            l,
        );
        if check.is_err() {
            return None;
        }
    }
    Some(packed)
}

/// A deterministic writer: fixed timestamps and attributes, entries in the order given.
pub(crate) struct ZipWriter<'w, W: Write> {
    out: &'w mut W,
    pos: u64,
    central: Vec<u8>,
    count: u16,
}

fn write_err(e: std::io::Error) -> Refusal {
    Refusal::new(C::IoError, e.to_string())
}

impl<'w, W: Write> ZipWriter<'w, W> {
    pub(crate) fn new(out: &'w mut W) -> Self {
        ZipWriter {
            out,
            pos: 0,
            central: Vec::new(),
            count: 0,
        }
    }

    /// Adds one entry: `body` is what goes in the file, `plain` the uncompressed bytes.
    pub(crate) fn add(
        &mut self,
        name: &str,
        method: u16,
        plain: &[u8],
        body: &[u8],
    ) -> Result<(), Refusal> {
        let crc = crc32(plain);
        let offset = self.pos as u32;
        let mut common = Vec::with_capacity(26);
        common.extend_from_slice(&20u16.to_le_bytes()); // version needed
        common.extend_from_slice(&0u16.to_le_bytes()); // flags
        common.extend_from_slice(&method.to_le_bytes());
        common.extend_from_slice(&0u16.to_le_bytes()); // time
        common.extend_from_slice(&0x0021u16.to_le_bytes()); // date: 1980-01-01
        common.extend_from_slice(&crc.to_le_bytes());
        common.extend_from_slice(&(body.len() as u32).to_le_bytes());
        common.extend_from_slice(&(plain.len() as u32).to_le_bytes());
        common.extend_from_slice(&(name.len() as u16).to_le_bytes());
        common.extend_from_slice(&0u16.to_le_bytes()); // extra length

        self.out
            .write_all(&SIG_LOCAL.to_le_bytes())
            .map_err(write_err)?;
        self.out.write_all(&common).map_err(write_err)?;
        self.out.write_all(name.as_bytes()).map_err(write_err)?;
        self.out.write_all(body).map_err(write_err)?;
        self.pos += (LOCAL_LEN + name.len() + body.len()) as u64;

        self.central.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
        self.central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        self.central.extend_from_slice(&common);
        self.central.extend_from_slice(&0u16.to_le_bytes()); // comment length
        self.central.extend_from_slice(&0u16.to_le_bytes()); // disk
        self.central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        self.central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        self.central.extend_from_slice(&offset.to_le_bytes());
        self.central.extend_from_slice(name.as_bytes());
        self.count += 1;
        Ok(())
    }

    /// Writes the central directory and the end record. Returns the file's size.
    pub(crate) fn finish(self) -> Result<u64, Refusal> {
        self.out.write_all(&self.central).map_err(write_err)?;
        let mut end = Vec::with_capacity(END_LEN as usize);
        end.extend_from_slice(&SIG_END.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        end.extend_from_slice(&self.count.to_le_bytes());
        end.extend_from_slice(&self.count.to_le_bytes());
        end.extend_from_slice(&(self.central.len() as u32).to_le_bytes());
        end.extend_from_slice(&(self.pos as u32).to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        self.out.write_all(&end).map_err(write_err)?;
        self.out.flush().map_err(write_err)?;
        Ok(self.pos + self.central.len() as u64 + END_LEN)
    }
}

const CRC_TABLES: [[u32; 256]; 8] = crc_tables();

const fn crc_tables() -> [[u32; 256]; 8] {
    let mut t = [[0u32; 256]; 8];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[0][i] = c;
        i += 1;
    }
    let mut i = 0;
    while i < 256 {
        let mut s = 1;
        while s < 8 {
            let p = t[s - 1][i];
            t[s][i] = (p >> 8) ^ t[0][(p & 0xFF) as usize];
            s += 1;
        }
        i += 1;
    }
    t
}

/// CRC-32 (IEEE), slicing by eight.
pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    let mut chunks = data.chunks_exact(8);
    for w in &mut chunks {
        let lo = u32::from_le_bytes([w[0], w[1], w[2], w[3]]) ^ c;
        let hi = u32::from_le_bytes([w[4], w[5], w[6], w[7]]);
        c = CRC_TABLES[7][(lo & 0xFF) as usize]
            ^ CRC_TABLES[6][((lo >> 8) & 0xFF) as usize]
            ^ CRC_TABLES[5][((lo >> 16) & 0xFF) as usize]
            ^ CRC_TABLES[4][(lo >> 24) as usize]
            ^ CRC_TABLES[3][(hi & 0xFF) as usize]
            ^ CRC_TABLES[2][((hi >> 8) & 0xFF) as usize]
            ^ CRC_TABLES[1][((hi >> 16) & 0xFF) as usize]
            ^ CRC_TABLES[0][(hi >> 24) as usize];
    }
    for &b in chunks.remainder() {
        c = (c >> 8) ^ CRC_TABLES[0][((c ^ u32::from(b)) & 0xFF) as usize];
    }
    !c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inflating_stops_at_the_declared_size() {
        let plain = vec![b' '; 1024 * 1024];
        let packed = compress_to_vec(&plain, 6);
        let declared = 1000u64;
        let mut offset = 0usize;
        let mut feed = |buf: &mut [u8]| -> Result<usize, Refusal> {
            let n = (packed.len() - offset).min(buf.len());
            buf[..n].copy_from_slice(&packed[offset..offset + n]);
            offset += n;
            Ok(n)
        };
        let mut input = vec![0u8; CHUNK];
        let mut sink = Vec::new();
        let r = inflate_checked(
            &mut feed,
            &mut input,
            packed.len() as u64,
            declared,
            &mut sink,
            "x",
            &Limits::V1,
        )
        .expect_err("refused");
        assert_eq!(r.code, C::ZipSizeMismatch);
        assert!(
            sink.len() as u64 <= declared,
            "kept {} bytes past the declared size",
            sink.len()
        );
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(
            crc32(b"1234567890123456789"),
            crc_slow(b"1234567890123456789")
        );
    }

    fn crc_slow(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c = (c >> 8) ^ CRC_TABLES[0][((c ^ u32::from(b)) & 0xFF) as usize];
        }
        !c
    }
}
