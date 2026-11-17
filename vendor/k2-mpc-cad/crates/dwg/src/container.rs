//! The container of a DWG file in the 2004 to 2018 family, read-only:
//!
//! - **the file header**: the version stamp, the maintenance byte, the code page and the
//!   security flags in plain bytes, then a 0x6C-byte header masked with a fixed sequence,
//!   holding the page map's and the section map's places and a CRC-32 over itself; its second
//!   copy, at the end of the file, must be the same bytes;
//! - **the page map**: a system page listing every page's number and size in file order from
//!   byte 0x100, a negative number marking a gap (free space) with four more fields;
//! - **the section map**: a system page listing each section's name, id, size, page size,
//!   whether it is compressed or encrypted, and its pages (number, data size, start offset);
//! - **data pages**: a 32-byte header masked with the page's address, holding its section, its
//!   sizes, its start offset and two checksums (the data's, and the header's seeded with it).
//!
//! Every check is made; a file that fails one is refused, typed, naming the byte. An encrypted
//! or password-protected drawing (the security flags set, an encrypted section, a security
//! section) is refused before any of its sections is read.
//!
//! From the specification's chapter on the 2004 file format (the file header, the system pages,
//! the page and section maps, the data pages); our own code.

use k2_mpc_cad_model::error::{CadError, NOT_FOUND};

use crate::crc::{crc32, page_checksum};
use crate::error::{BAD_CONTAINER, ENCRYPTED, NOT_DWG, OVER_BOUND, TRUNCATED, UNSUPPORTED_VERSION};
use crate::lz77::decompress;
use crate::{mutant, Limits};

const MAGIC: &[u8; 12] = b"AcFssFcAJMB\0";
const PAGE_MAP: u32 = 0x4163_0E3B;
const SECTION_MAP: u32 = 0x4163_003B;
const DATA_PAGE: u32 = 0x4163_043B;
const DATA_MASK: u32 = 0x4164_536B;
const NEXT: &str = "the file is damaged: open a copy the app that made it saves again";

/// A file's container, as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    /// The version stamp (`AC1032`).
    pub version: String,
    pub maintenance: u8,
    pub code_page: u16,
    pub security_flags: u32,
    /// Every page in the page map, in file order.
    pub pages: Vec<Page>,
    /// The gaps (free space) between pages.
    pub gaps: Vec<Gap>,
    pub sections: Vec<Section>,
}

/// A page in the page map: its number, where it starts, its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub number: u32,
    pub address: u64,
    pub size: u64,
}

/// Free space between pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    pub address: u64,
    pub size: u64,
}

/// A section in the section map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub id: u32,
    /// Its whole size, decompressed.
    pub size: u64,
    /// One page's largest decompressed size.
    pub max_page: u32,
    pub compressed: bool,
    pub encrypted: bool,
    pub pages: Vec<SectionPage>,
}

/// One of a section's pages: its number in the page map, its data's size, where its data
/// starts in the section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionPage {
    pub number: u32,
    pub data_size: u32,
    pub start: u64,
}

impl Container {
    /// The section named `name`.
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }
}

fn bad(why: impl Into<String>, at: usize) -> CadError {
    CadError::new(BAD_CONTAINER, why, NEXT).at(at)
}

fn le<const N: usize>(b: &[u8], at: usize) -> Result<[u8; N], CadError> {
    at.checked_add(N)
        .and_then(|end| b.get(at..end))
        .and_then(|s| <[u8; N]>::try_from(s).ok())
        .ok_or_else(|| {
            CadError::new(
                TRUNCATED,
                format!("{N} bytes wanted at byte {at}, past the end"),
                NEXT,
            )
            .at(at)
        })
}

fn rl(b: &[u8], at: usize) -> Result<u32, CadError> {
    Ok(u32::from_le_bytes(le::<4>(b, at)?))
}

fn rll(b: &[u8], at: usize) -> Result<u64, CadError> {
    Ok(u64::from_le_bytes(le::<8>(b, at)?))
}

fn place(v: u64, what: &str) -> Result<usize, CadError> {
    usize::try_from(v).map_err(|_| bad(format!("{what} at byte {v}, past any file"), 0))
}

/// The fixed sequence the file header is masked with.
fn header_mask() -> [u8; 0x6C] {
    let mut seed: u32 = 1;
    let mut m = [0u8; 0x6C];
    for x in &mut m {
        seed = seed.wrapping_mul(0x343FD).wrapping_add(0x0026_9EC3);
        *x = (seed >> 16) as u8;
    }
    m
}

/// A system page's data: its type checked, its checksum checked, decompressed.
fn system_page(
    bytes: &[u8],
    address: u64,
    want: u32,
    what: &str,
    limits: &Limits,
) -> Result<Vec<u8>, CadError> {
    let at = place(address, what)?;
    let head = le::<0x14>(bytes, at)?;
    let (kind, size, csize, ctype, check) = (
        rl(&head, 0)?,
        rl(&head, 4)?,
        rl(&head, 8)?,
        rl(&head, 12)?,
        rl(&head, 16)?,
    );
    if kind != want {
        return Err(bad(format!("the page at byte {at} is not the {what}"), at));
    }
    let data = (at + 0x14)
        .checked_add(csize as usize)
        .and_then(|end| bytes.get(at + 0x14..end))
        .ok_or_else(|| {
            CadError::new(
                TRUNCATED,
                format!("the {what} runs past the end of the file"),
                NEXT,
            )
            .at(at)
        })?;
    let mut zeroed = head;
    zeroed[16..20].fill(0);
    let calc = page_checksum(page_checksum(0, &zeroed), data);
    if calc != check {
        return Err(bad(
            format!("the {what}'s checksum is {check:08X}, its bytes give {calc:08X}"),
            at,
        ));
    }
    match ctype {
        2 => decompress(data, size as usize, limits),
        1 => Ok(data.to_vec()),
        _ => Err(bad(format!("the {what} has compression type {ctype}"), at)),
    }
}

/// The container of `bytes`, a whole DWG file, every check made.
pub fn read_container(bytes: &[u8], limits: &Limits) -> Result<Container, CadError> {
    if bytes.len() as u64 > limits.max_bytes {
        return Err(CadError::new(
            OVER_BOUND,
            format!(
                "the file is {} bytes, over the limit of {}",
                bytes.len(),
                limits.max_bytes
            ),
            "raise the file limit for a file you trust",
        ));
    }
    let stamp = bytes.get(0..6).unwrap_or(&[]);
    if !stamp.starts_with(b"AC10") {
        return Err(CadError::new(
            NOT_DWG,
            "not a DWG file: no version stamp",
            "open the file with the reader for its format",
        ));
    }
    let version = String::from_utf8_lossy(stamp).into_owned();
    let family = matches!(stamp, b"AC1018" | b"AC1024" | b"AC1027" | b"AC1032");
    if !family && !mutant::active("VersionByPrefix") {
        return Err(CadError::new(
            UNSUPPORTED_VERSION,
            format!("version {version}: this reader reads the 2004 to 2018 family (AC1018, AC1024, AC1027, AC1032)"),
            "save the drawing as DWG 2018 or as DXF in the app that made it",
        ));
    }
    if bytes.len() < 0x100 {
        return Err(CadError::new(
            TRUNCATED,
            "the file is shorter than its header",
            NEXT,
        ));
    }
    let security_flags = rl(bytes, 0x18)?;
    let encryption_checked = !mutant::active("EncryptionIgnored");
    if security_flags != 0 && encryption_checked {
        return Err(encrypted(format!(
            "the file's security flags are set (0x{security_flags:X})"
        )));
    }
    // the masked header, its CRC and its second copy
    let mask = header_mask();
    let mut h = [0u8; 0x6C];
    for (k, x) in h.iter_mut().enumerate() {
        *x = bytes[0x80 + k] ^ mask[k];
    }
    if h[0..12] != MAGIC[..] {
        return Err(bad("the file header's id is not the 2004 family's", 0x80));
    }
    let crc = rl(&h, 0x68)?;
    let mut zeroed = h;
    zeroed[0x68..0x6C].fill(0);
    let calc = crc32(&zeroed);
    if calc != crc && !mutant::active("HeaderCrcSkipped") {
        return Err(bad(
            format!("the file header's CRC-32 is {crc:08X}, its bytes give {calc:08X}"),
            0x80,
        ));
    }
    let second = place(rll(&h, 0x34)?, "the file header's second copy")?;
    match second
        .checked_add(0x6C)
        .and_then(|end| bytes.get(second..end))
    {
        Some(copy) if copy == &bytes[0x80..0xEC] => {}
        Some(_) => {
            return Err(bad(
                "the file header's second copy differs from the first",
                second,
            ))
        }
        None => {
            return Err(bad(
                "the file header's second copy lies past the end of the file",
                second,
            ))
        }
    }
    // the page map
    let map_at = rll(&h, 0x54)?
        .checked_add(0x100)
        .ok_or_else(|| bad("the page map's place overflows", 0x80))?;
    let map = system_page(bytes, map_at, PAGE_MAP, "page map", limits)?;
    let (mut pages, mut gaps) = (Vec::new(), Vec::new());
    let (mut i, mut address) = (0usize, 0x100u64);
    while i < map.len() {
        let number = rl(&map, i)? as i32;
        let size = u64::from(rl(&map, i + 4)?);
        i += 8;
        if number < 0 {
            le::<16>(&map, i)?;
            i += 16;
            gaps.push(Gap { address, size });
            if mutant::active("GapsNotAdvanced") {
                continue;
            }
        } else {
            pages.push(Page {
                number: number as u32,
                address,
                size,
            });
        }
        address = address
            .checked_add(size)
            .ok_or_else(|| bad("the page map's sizes overflow", 0))?;
    }
    // the section map
    let section_map = rl(&h, 0x5C)?;
    let sm_page = pages
        .iter()
        .find(|p| p.number == section_map)
        .ok_or_else(|| {
            bad(
                format!("the section map's page {section_map} is not in the page map"),
                0x80,
            )
        })?;
    let sm = system_page(bytes, sm_page.address, SECTION_MAP, "section map", limits)?;
    let count = rl(&sm, 0)?;
    let mut sections = Vec::new();
    let mut i = 20usize;
    for _ in 0..count {
        let size = rll(&sm, i)?;
        let page_count = rl(&sm, i + 8)?;
        let max_page = rl(&sm, i + 12)?;
        let compression = rl(&sm, i + 20)?;
        let id = rl(&sm, i + 24)?;
        let encrypted_flag = rl(&sm, i + 28)?;
        let raw_name = le::<64>(&sm, i + 32)?;
        let name_end = raw_name.iter().position(|&b| b == 0).unwrap_or(64);
        let name = String::from_utf8_lossy(&raw_name[..name_end]).into_owned();
        i += 96;
        let left = sm.len().saturating_sub(i);
        if (page_count as usize).saturating_mul(16) > left && !mutant::active("PageCountTrusted") {
            return Err(bad(
                format!(
                    "section {name:?} lists {page_count} pages, more than the section map holds"
                ),
                0,
            ));
        }
        let mut section_pages = Vec::with_capacity(page_count as usize);
        for _ in 0..page_count {
            section_pages.push(SectionPage {
                number: rl(&sm, i)?,
                data_size: rl(&sm, i + 4)?,
                start: rll(&sm, i + 8)?,
            });
            i += 16;
        }
        if mutant::active("SectionPagesOffByOne") {
            section_pages.pop();
        }
        if encryption_checked && (encrypted_flag != 0 || name == "AcDb:Security") {
            return Err(encrypted(format!("section {name:?} is encrypted")));
        }
        sections.push(Section {
            name,
            id,
            size,
            max_page,
            compressed: compression == 2,
            encrypted: encrypted_flag != 0,
            pages: section_pages,
        });
    }
    for s in &sections {
        for p in &s.pages {
            if !pages.iter().any(|q| q.number == p.number) {
                return Err(bad(
                    format!(
                        "section {:?} names page {}, which the page map hasn't",
                        s.name, p.number
                    ),
                    0,
                ));
            }
        }
    }
    Ok(Container {
        version,
        maintenance: bytes[0x0B],
        code_page: u16::from_le_bytes([bytes[0x13], bytes[0x14]]),
        security_flags,
        pages,
        gaps,
        sections,
    })
}

fn encrypted(why: String) -> CadError {
    CadError::new(
        ENCRYPTED,
        format!("{why}: an encrypted or password-protected drawing is not read"),
        "open it in the app that made it, remove the password, and save a copy",
    )
}

/// The section `name` of `bytes` (whose container is `c`), decompressed, each of its pages
/// checked: the page's type and section, its data checksum, its header checksum (seeded with
/// the data's), its start offset. A section larger than [`Limits::max_section`] is refused
/// before anything is allocated.
pub fn read_section(
    bytes: &[u8],
    c: &Container,
    name: &str,
    limits: &Limits,
) -> Result<Vec<u8>, CadError> {
    let s = c.section(name).ok_or_else(|| {
        CadError::new(
            NOT_FOUND,
            format!("no section {name:?} in the section map"),
            "list the container's sections",
        )
    })?;
    if s.size > limits.max_section && !mutant::active("SectionSizeTrusted") {
        return Err(CadError::new(
            OVER_BOUND,
            format!(
                "section {name:?} is {} bytes, over the limit of {}",
                s.size, limits.max_section
            ),
            "raise the section limit for a file you trust",
        ));
    }
    let size = place(s.size, "a section's end")?;
    let mut out = vec![0u8; size];
    let checks = !mutant::active("ChecksumsSkipped");
    for p in &s.pages {
        let page = c
            .pages
            .iter()
            .find(|q| q.number == p.number)
            .ok_or_else(|| bad(format!("page {} is not in the page map", p.number), 0))?;
        let at = place(page.address, "a data page")?;
        let raw = le::<32>(bytes, at)?;
        let m = DATA_MASK ^ (page.address as u32);
        let mut hd = [0u32; 8];
        for (k, x) in hd.iter_mut().enumerate() {
            *x = rl(&raw, 4 * k)? ^ m;
        }
        let [kind, section, csize, _page_size, start_lo, start_hi, head_check, data_check] = hd;
        if kind != DATA_PAGE {
            return Err(bad(format!("the page at byte {at} is not a data page"), at));
        }
        if section != s.id {
            return Err(bad(
                format!(
                    "the page at byte {at} belongs to section {section}, not {}",
                    s.id
                ),
                at,
            ));
        }
        let data = (at + 32)
            .checked_add(csize as usize)
            .and_then(|end| bytes.get(at + 32..end))
            .ok_or_else(|| {
                CadError::new(TRUNCATED, "a data page runs past the end of the file", NEXT).at(at)
            })?;
        let calc = page_checksum(0, data);
        let mut head = [0u8; 32];
        for (k, v) in hd.iter().enumerate() {
            head[4 * k..4 * k + 4].copy_from_slice(&v.to_le_bytes());
        }
        head[24..28].fill(0);
        let head_calc = page_checksum(calc, &head);
        if checks && calc != data_check {
            return Err(bad(
                format!("the data page at byte {at}: its checksum is {data_check:08X}, its bytes give {calc:08X}"),
                at,
            ));
        }
        if checks && head_calc != head_check {
            return Err(bad(
                format!("the data page at byte {at}: its header's checksum is {head_check:08X}, its bytes give {head_calc:08X}"),
                at,
            ));
        }
        let start = (u64::from(start_hi) << 32) | u64::from(start_lo);
        if start != p.start {
            return Err(bad(
                format!(
                    "the data page at byte {at} starts at {start}, the section map says {}",
                    p.start
                ),
                at,
            ));
        }
        let decoded;
        let data = if s.compressed {
            decoded = decompress(data, s.max_page as usize, limits)?;
            &decoded[..]
        } else {
            data
        };
        let start = place(start, "a page's start")?;
        if start > size {
            return Err(bad(
                format!("the data page at byte {at} starts past its section's end"),
                at,
            ));
        }
        let end = start.saturating_add(data.len()).min(size);
        out[start..end].copy_from_slice(&data[..end - start]);
    }
    Ok(out)
}
