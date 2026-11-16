//! `unzip ZIP MEMBER OUT`: write one member's bytes to OUT. `unzip ZIP --all DIR`: take every
//! member out under DIR.
//!
//! The reader is our own (stored and deflated members, zip64, an archive comment, bytes in
//! front of the archive), and reads as Python's zipfile does, which the packs used before:
//! names are UTF-8 when the member says so and cp437 otherwise; a member named twice is the
//! last one; CRC-32 and the local header's name are checked; an encrypted member, another
//! compression method, a member that overlaps the next refuse. `--all` keeps every path under
//! DIR: a leading `/`, and `.`, `..` and empty parts of a name, are dropped; a file member whose
//! name is left empty refuses. Exit 4 when the zip or the member isn't there, 1 on a damaged or
//! unsupported archive.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::{Ctx, Fail, Report, OK};

#[derive(clap::Args)]
pub struct Args {
    /// The zip archive
    zip: PathBuf,
    /// The member to take out (its name in the archive)
    #[arg(required_unless_present = "all", requires = "out")]
    member: Option<String>,
    /// Where the member's bytes go
    out: Option<PathBuf>,
    /// Take every member out under this folder instead
    #[arg(long, value_name = "DIR", conflicts_with_all = ["member", "out"])]
    all: Option<PathBuf>,
}

/// cp437's upper half (0x80 to 0xFF); the lower half is ASCII.
const CP437_HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";

fn cp437(bytes: &[u8]) -> String {
    let high: Vec<char> = CP437_HIGH.chars().collect();
    bytes
        .iter()
        .map(|b| {
            if *b < 0x80 {
                *b as char
            } else {
                high[(*b - 0x80) as usize]
            }
        })
        .collect()
}

fn bad(msg: impl Into<String>) -> Fail {
    Fail::error(format!("unzip: {}", msg.into()))
}

fn u16_at(d: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([d[i], d[i + 1]])
}
fn u32_at(d: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]])
}
fn u64_at(d: &[u8], i: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[i..i + 8]);
    u64::from_le_bytes(b)
}

/// `len` bytes at `at`, or fewer at the end of the data (as a file read gives them).
fn slice(d: &[u8], at: i128, len: usize) -> &[u8] {
    if at < 0 || at as u128 >= d.len() as u128 {
        return &[];
    }
    let at = at as usize;
    &d[at..at.saturating_add(len).min(d.len())]
}

/// One member, as the central directory lists it.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The name, up to a NUL byte.
    pub name: String,
    /// The name as stored (decoded), for the local header check.
    orig: String,
    flags: u16,
    method: u16,
    crc: u32,
    csize: u64,
    size: u64,
    offset: i128,
    end_offset: i128,
}

const EOCD: &[u8] = b"PK\x05\x06";
const EOCD64_LOC: u32 = 0x0706_4b50;
const EOCD64: &[u8] = b"PK\x06\x06";

/// The end record: (central directory size, its offset, where the record starts).
fn end_record(d: &[u8]) -> Result<(u64, u64, i128), Fail> {
    let not_zip = || bad("not a zip file");
    let n = d.len();
    if n < 22 {
        return Err(not_zip());
    }
    let (rec, at) = if &d[n - 22..n - 18] == EOCD && d[n - 2..] == [0, 0] {
        (&d[n - 22..], n - 22)
    } else {
        let start = n.saturating_sub(65535 + 22);
        let pos = d[start..]
            .windows(4)
            .rposition(|w| w == EOCD)
            .ok_or_else(not_zip)?;
        let at = start + pos;
        if at + 22 > n {
            return Err(not_zip());
        }
        (&d[at..at + 22], at)
    };
    let mut size = u32_at(rec, 12) as u64;
    let mut offset = u32_at(rec, 16) as u64;
    let mut location = at as i128;
    // zip64: the locator right before the end record, the zip64 record before it
    let loc = at as i128 - 20;
    if loc >= 0 {
        let l = &d[loc as usize..loc as usize + 20];
        if u32_at(l, 0) == EOCD64_LOC {
            let (diskno, reloff, disks) = (u32_at(l, 4), u64_at(l, 8) as i128, u32_at(l, 16));
            if diskno != 0 || disks > 1 {
                return Err(bad("zip files that span several disks are not supported"));
            }
            let mut off = loc - 56;
            if reloff > off {
                return Err(bad("corrupt zip64 end of central directory locator"));
            }
            let mut extrasz = off - reloff;
            let mut rec64 = slice(d, reloff, 56);
            if rec64.len() != 56 {
                return Err(bad("corrupt zip64 end of central directory"));
            }
            if !rec64.starts_with(EOCD64) && reloff != off {
                rec64 = slice(d, off, 56);
                extrasz = 0;
                if rec64.len() != 56 {
                    return Err(bad("corrupt zip64 end of central directory"));
                }
            }
            if !rec64.starts_with(EOCD64) {
                return Err(bad("zip64 end of central directory record not found"));
            }
            let sz = u64_at(rec64, 4) as i128;
            let (dirsize, diroffset) = (u64_at(rec64, 40), u64_at(rec64, 48));
            if diroffset as i128 + dirsize as i128 != reloff || sz + 12 != 56 + extrasz {
                return Err(bad("corrupt zip64 end of central directory record"));
            }
            size = dirsize;
            offset = diroffset;
            off -= extrasz;
            location = off;
        }
    }
    Ok((size, offset, location))
}

/// The central directory: every member, in order.
pub fn entries(d: &[u8]) -> Result<Vec<Entry>, Fail> {
    let (size_cd, offset_cd, location) = end_record(d)?;
    let concat = location - size_cd as i128 - offset_cd as i128;
    let start_dir = offset_cd as i128 + concat;
    if start_dir < 0 {
        return Err(bad("bad offset for the central directory"));
    }
    let cd = slice(d, start_dir, size_cd as usize);
    let mut out = Vec::new();
    let mut p = 0usize;
    while (p as u64) < size_cd {
        if cd.len() < p + 46 {
            return Err(bad("truncated central directory"));
        }
        let h = &cd[p..p + 46];
        if &h[..4] != b"PK\x01\x02" {
            return Err(bad("bad magic number for the central directory"));
        }
        let extract_version = h[6];
        let flags = u16_at(h, 8);
        let method = u16_at(h, 10);
        let crc = u32_at(h, 16);
        let mut csize = u32_at(h, 20) as u64;
        let mut usize_ = u32_at(h, 24) as u64;
        let (nlen, xlen, clen) = (
            u16_at(h, 28) as usize,
            u16_at(h, 30) as usize,
            u16_at(h, 32) as usize,
        );
        let mut header_offset = u32_at(h, 42) as u64;
        let at = p + 46;
        let raw_name = &cd[at.min(cd.len())..(at + nlen).min(cd.len())];
        let extra = &cd[(at + nlen).min(cd.len())..(at + nlen + xlen).min(cd.len())];
        let orig = if flags & 0x800 != 0 {
            String::from_utf8(raw_name.to_vec()).map_err(|_| bad("a member name is not UTF-8"))?
        } else {
            cp437(raw_name)
        };
        let mut name = orig.split('\0').next().unwrap_or("").to_string();
        if extract_version > 63 {
            return Err(bad(format!(
                "zip file version {:.1} is not supported",
                extract_version as f64 / 10.0
            )));
        }
        // the extra fields: zip64 sizes and offset, and the Unicode path
        let mut x = extra;
        while x.len() >= 4 {
            let (tp, ln) = (u16_at(x, 0), u16_at(x, 2) as usize);
            if ln + 4 > x.len() {
                return Err(bad(format!("corrupt extra field {tp:04x} (size={ln})")));
            }
            let data = &x[4..ln + 4];
            if tp == 0x0001 {
                let mut q = data;
                let mut take = |field: &str| -> Result<u64, Fail> {
                    if q.len() < 8 {
                        return Err(bad(format!("corrupt zip64 extra field: {field} not found")));
                    }
                    let v = u64_at(q, 0);
                    q = &q[8..];
                    Ok(v)
                };
                if usize_ == 0xFFFF_FFFF {
                    usize_ = take("file size")?;
                }
                if csize == 0xFFFF_FFFF {
                    csize = take("compress size")?;
                }
                if header_offset == 0xFFFF_FFFF {
                    header_offset = take("header offset")?;
                }
            } else if tp == 0x7075 && data.len() >= 5 {
                let (version, name_crc) = (data[0], u32_at(data, 1));
                if version == 1 && name_crc == crc32(raw_name) {
                    let up = std::str::from_utf8(&data[5..])
                        .map_err(|_| bad("corrupt unicode path extra field (0x7075)"))?;
                    if !up.is_empty() {
                        name = up.split('\0').next().unwrap_or("").to_string();
                    }
                }
            } else if tp == 0x7075 {
                return Err(bad("corrupt unicode path extra field (0x7075)"));
            }
            x = &x[ln + 4..];
        }
        out.push(Entry {
            name,
            orig,
            flags,
            method,
            crc,
            csize,
            size: usize_,
            offset: header_offset as i128 + concat,
            end_offset: 0,
        });
        p += 46 + nlen + xlen + clen;
    }
    // each member ends where the next one (by offset) starts, the last at the directory
    let mut order: Vec<usize> = (0..out.len()).collect();
    order.sort_by_key(|i| out[*i].offset);
    let mut end = start_dir;
    for i in order.into_iter().rev() {
        out[i].end_offset = end;
        end = out[i].offset;
    }
    Ok(out)
}

/// CRC-32 (IEEE), as zip uses it.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for b in data {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Opens a member: checks its local header; `Ok` with where its data starts. The checks that
/// come before anything is written.
fn open(d: &[u8], e: &Entry) -> Result<usize, Fail> {
    let h = slice(d, e.offset, 30);
    if h.len() != 30 {
        return Err(bad("truncated file header"));
    }
    if &h[..4] != b"PK\x03\x04" {
        return Err(bad("bad magic number for file header"));
    }
    let (lflags, nlen, xlen) = (u16_at(h, 6), u16_at(h, 26) as usize, u16_at(h, 28) as usize);
    let lname = slice(d, e.offset + 30, nlen);
    if e.flags & 0x20 != 0 {
        return Err(bad("compressed patched data (flag bit 5) is not supported"));
    }
    if e.flags & 0x40 != 0 {
        return Err(bad("strong encryption (flag bit 6) is not supported"));
    }
    let lname = if lflags & 0x800 != 0 {
        String::from_utf8(lname.to_vec()).map_err(|_| bad("a member name is not UTF-8"))?
    } else {
        cp437(lname)
    };
    if lname != e.orig {
        return Err(bad(format!(
            "file name in directory {:?} and header {:?} differ",
            e.orig, lname
        )));
    }
    let start = e.offset + 30 + nlen as i128 + xlen as i128;
    if start + e.csize as i128 > e.end_offset && e.end_offset != e.offset {
        return Err(bad(format!(
            "overlapped entries: {:?} (possible zip bomb)",
            e.orig
        )));
    }
    if e.flags & 1 != 0 {
        return Err(bad(format!(
            "{:?} is encrypted, and no password is taken",
            e.name
        )));
    }
    if e.method != 0 && e.method != 8 {
        return Err(bad(format!(
            "{:?}: compression method {} is not supported",
            e.name, e.method
        )));
    }
    if start < 0 || start as u128 > usize::MAX as u128 {
        return Err(bad("bad member offset"));
    }
    Ok(start as usize)
}

/// A member's bytes, after [`open`]: decompressed and checked against its CRC-32.
fn data(d: &[u8], e: &Entry, start: usize) -> Result<Vec<u8>, Fail> {
    let raw = slice(d, start as i128, e.csize as usize);
    let mut out = if e.method == 8 {
        miniz_oxide::inflate::decompress_to_vec(raw)
            .map_err(|_| bad(format!("{:?}: the compressed data is damaged", e.name)))?
    } else {
        if raw.len() < e.size.min(e.csize) as usize {
            return Err(bad(format!(
                "{:?}: the archive ends inside the member",
                e.name
            )));
        }
        raw.to_vec()
    };
    out.truncate(e.size as usize);
    if crc32(&out) != e.crc {
        return Err(bad(format!("bad CRC-32 for {:?}", e.name)));
    }
    Ok(out)
}

/// Where a member goes under `dir`, as Python's extractall puts it.
pub fn target(dir: &Path, name: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = name
        .split('/')
        .filter(|p| !p.is_empty() && *p != "." && *p != "..")
        .collect();
    if parts.is_empty() {
        return None;
    }
    let mut t = dir.to_path_buf();
    for p in parts {
        t.push(p);
    }
    Some(t)
}

pub fn run(a: &Args, ctx: &Ctx) -> Result<Report, Fail> {
    let zp = ctx.path(&a.zip);
    let d = std::fs::read(&zp).map_err(|e| Fail::io(&zp, e))?;
    let all = entries(&d)?;
    if let Some(dir) = &a.all {
        let dir = ctx.path(dir);
        let mut files = 0;
        for listed in &all {
            // as Python's extractall: by name, so a name listed twice takes the last member both times
            let e = all
                .iter()
                .rev()
                .find(|x| x.name == listed.name)
                .unwrap_or(listed);
            let is_dir = e.name.ends_with('/');
            let t = match target(&dir, &e.name) {
                Some(t) => t,
                None if is_dir => dir.clone(),
                None => return Err(bad(format!("{:?}: an empty file name", e.name))),
            };
            if let Some(up) = t.parent() {
                if !up.exists() {
                    std::fs::create_dir_all(up).map_err(|err| Fail::io(up, err))?;
                }
            }
            if is_dir {
                if !t.is_dir() {
                    std::fs::create_dir(&t).map_err(|err| Fail::io(&t, err))?;
                }
                continue;
            }
            let start = open(&d, e)?;
            // as before: the file is created before its data is read, so a damaged member
            // leaves it empty
            std::fs::write(&t, b"").map_err(|err| Fail::io(&t, err))?;
            let bytes = data(&d, e, start)?;
            std::fs::write(&t, &bytes).map_err(|err| Fail::io(&t, err))?;
            files += 1;
        }
        return Ok(Report::new(
            OK,
            String::new(),
            json!({ "members": all.len(), "files": files }),
        ));
    }
    let member = a.member.as_deref().expect("clap: a member or --all");
    let out = a.out.as_ref().expect("clap: a member takes OUT");
    let e = all
        .iter()
        .rev()
        .find(|e| e.name == member)
        .ok_or_else(|| Fail::not_found(format!("unzip: no member named {member:?}")))?;
    let start = open(&d, e)?;
    let bytes = data(&d, e, start)?;
    let op = ctx.path(out);
    std::fs::write(&op, &bytes).map_err(|err| Fail::io(&op, err))?;
    Ok(Report::new(
        OK,
        String::new(),
        json!({ "member": member, "bytes": bytes.len() }),
    ))
}
