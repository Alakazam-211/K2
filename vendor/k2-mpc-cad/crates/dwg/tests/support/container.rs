//! A synthetic container builder, for the tests only: it writes a DWG container's structure
//! (the file header and its second copy, data pages, the section map, the page map with gaps)
//! around section bytes the test chooses. It writes no drawing objects and is not a file
//! generator; it lives in the tests and is never shipped. Its app-info section holds only our
//! own generic writer name ([`WRITER`]), and nothing else that names a writer.

use k2_mpc_dwg::crc::{crc32, page_checksum};
use k2_mpc_dwg::mutant;

use super::literal;

/// The one writer name the builder writes.
pub const WRITER: &str = "container test builder";
/// Every app-info value a container from this builder may hold.
pub const ALLOWED_WRITERS: &[&str] = &[WRITER];
/// The name of the section the builder's app-info goes in.
pub const APP_INFO: &str = "AcDb:AppInfo";

/// A section to write: its bytes, cut into pages of `per_page` bytes.
#[derive(Clone)]
pub struct Sec {
    pub name: String,
    pub id: u32,
    pub data: Vec<u8>,
    pub per_page: usize,
    pub compressed: bool,
    pub encrypted: bool,
    /// A page count to write into the section map in place of the true one.
    pub pages_claimed: Option<u32>,
    /// A size to write into the section map in place of the true one.
    pub size_claimed: Option<u64>,
}

/// A section of `data` in pages of `per_page` bytes.
pub fn sec(name: &str, id: u32, data: Vec<u8>, per_page: usize, compressed: bool) -> Sec {
    Sec {
        name: name.to_owned(),
        id,
        data,
        per_page,
        compressed,
        encrypted: false,
        pages_claimed: None,
        size_claimed: None,
    }
}

/// A container to write.
#[derive(Clone)]
pub struct Spec {
    pub version: [u8; 6],
    pub security: u32,
    pub sections: Vec<Sec>,
    /// A gap (0x40 bytes of free space) after each of these data pages (counted from 0).
    pub gaps_after: Vec<usize>,
}

/// What was written: the bytes, and where each data page and gap went.
pub struct Built {
    pub bytes: Vec<u8>,
    /// Each data page's number and address, in file order.
    pub data_pages: Vec<(u32, u64)>,
    /// Each gap's address and size.
    pub gaps: Vec<(u64, u64)>,
}

fn mask() -> [u8; 0x6C] {
    let mut m = [0u8; 0x6C];
    let mut seed: u32 = 1;
    for x in m.iter_mut() {
        seed = seed.wrapping_mul(0x0003_43FD).wrapping_add(0x0026_9EC3);
        *x = ((seed >> 16) & 0xFF) as u8;
    }
    m
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// `data` as a compressed stream: one literal run, then the end mark.
fn stream(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return vec![0x11];
    }
    let mut s = literal(data);
    s.push(0x11);
    s
}

fn pad(v: &mut Vec<u8>) {
    while v.len() % 0x20 != 0 {
        v.push(0);
    }
}

/// A system page: its 0x14-byte header (type, sizes, compression 2, checksum), then `data`
/// compressed.
fn system_page(kind: u32, data: &[u8]) -> Vec<u8> {
    let comp = stream(data);
    let mut head = vec![0u8; 0x14];
    put32(&mut head, 0, kind);
    put32(&mut head, 4, data.len() as u32);
    put32(&mut head, 8, comp.len() as u32);
    put32(&mut head, 12, 2);
    let check = page_checksum(page_checksum(0, &head), &comp);
    put32(&mut head, 16, check);
    let mut page = head;
    page.extend_from_slice(&comp);
    pad(&mut page);
    page
}

/// The app-info section: our writer name, UTF-16LE.
fn app_info(id: u32) -> Sec {
    let name = if mutant::active("AppInfoForeign") {
        "another writer"
    } else {
        WRITER
    };
    let data: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    sec(APP_INFO, id, data, 0x7400, false)
}

/// The container `spec` describes.
pub fn build(spec: &Spec) -> Built {
    let mut sections = spec.sections.clone();
    let next_id = sections.iter().map(|s| s.id).max().unwrap_or(0) + 1;
    sections.push(app_info(next_id));
    let mut out = vec![0u8; 0x100];
    let mut entries: Vec<(i32, u32)> = Vec::new();
    let mut data_pages = Vec::new();
    let mut gaps = Vec::new();
    let mut number = 1u32;
    let mut map_pages: Vec<Vec<(u32, u32, u64)>> = Vec::new();
    let mut page_index = 0usize;
    for s in &sections {
        let mut listed = Vec::new();
        let chunks: Vec<&[u8]> = if s.data.is_empty() {
            Vec::new()
        } else {
            s.data.chunks(s.per_page.max(1)).collect()
        };
        for (k, chunk) in chunks.iter().enumerate() {
            let address = out.len() as u64;
            let start = (k * s.per_page) as u64;
            let comp = if s.compressed {
                stream(chunk)
            } else {
                chunk.to_vec()
            };
            let mut head = [0u32; 8];
            head[0] = 0x4163_043B;
            head[1] = s.id;
            head[2] = comp.len() as u32;
            head[3] = s.per_page as u32;
            head[4] = start as u32;
            head[5] = (start >> 32) as u32;
            head[7] = page_checksum(0, &comp);
            let mut plain = [0u8; 32];
            for (j, v) in head.iter().enumerate() {
                put32(&mut plain, 4 * j, *v);
            }
            head[6] = page_checksum(head[7], &plain);
            let m = 0x4164_536B ^ (address as u32);
            let mut page = Vec::new();
            for v in head {
                page.extend_from_slice(&(v ^ m).to_le_bytes());
            }
            page.extend_from_slice(&comp);
            pad(&mut page);
            entries.push((number as i32, page.len() as u32));
            data_pages.push((number, address));
            listed.push((number, chunk.len() as u32, start));
            out.extend_from_slice(&page);
            number += 1;
            if spec.gaps_after.contains(&page_index) {
                gaps.push((out.len() as u64, 0x40));
                entries.push((-(gaps.len() as i32), 0x40));
                out.extend_from_slice(&[0u8; 0x40]);
            }
            page_index += 1;
        }
        map_pages.push(listed);
    }
    // the section map
    let mut sm = vec![0u8; 20];
    put32(&mut sm, 0, sections.len() as u32);
    put32(&mut sm, 4, 2);
    put32(&mut sm, 8, 0x7400);
    for (s, listed) in sections.iter().zip(&map_pages) {
        let mut d = vec![0u8; 96];
        put64(&mut d, 0, s.size_claimed.unwrap_or(s.data.len() as u64));
        put32(&mut d, 8, s.pages_claimed.unwrap_or(listed.len() as u32));
        put32(&mut d, 12, s.per_page as u32);
        put32(&mut d, 20, if s.compressed { 2 } else { 1 });
        put32(&mut d, 24, s.id);
        put32(&mut d, 28, u32::from(s.encrypted));
        d[32..32 + s.name.len()].copy_from_slice(s.name.as_bytes());
        for &(n, size, start) in listed {
            let mut e = vec![0u8; 16];
            put32(&mut e, 0, n);
            put32(&mut e, 4, size);
            put64(&mut e, 8, start);
            d.extend_from_slice(&e);
        }
        sm.extend_from_slice(&d);
    }
    let sm_number = number;
    let sm_page = system_page(0x4163_003B, &sm);
    entries.push((sm_number as i32, sm_page.len() as u32));
    out.extend_from_slice(&sm_page);
    // the page map, listing itself
    let pm_number = sm_number + 1;
    let pm_at = out.len() as u64;
    let map_of = |own_size: u32| -> Vec<u8> {
        let mut m = Vec::new();
        for &(n, size) in entries.iter().chain([(pm_number as i32, own_size)].iter()) {
            m.extend_from_slice(&n.to_le_bytes());
            m.extend_from_slice(&size.to_le_bytes());
            if n < 0 {
                m.extend_from_slice(&[0u8; 16]);
            }
        }
        m
    };
    let own = system_page(0x4163_0E3B, &map_of(0)).len() as u32;
    let pm_page = system_page(0x4163_0E3B, &map_of(own));
    assert_eq!(pm_page.len() as u32, own, "the page map's own size");
    out.extend_from_slice(&pm_page);
    // the file header, masked, with its CRC; its second copy at the end
    let second = out.len() as u64;
    let mut h = [0u8; 0x6C];
    h[0..12].copy_from_slice(b"AcFssFcAJMB\0");
    put32(&mut h, 0x10, 0x6C);
    put32(&mut h, 0x14, 4);
    put32(&mut h, 0x28, pm_number);
    put64(&mut h, 0x2C, second);
    put64(&mut h, 0x34, second);
    put32(&mut h, 0x3C, gaps.len() as u32);
    put32(&mut h, 0x40, pm_number);
    put32(&mut h, 0x50, pm_number);
    put64(&mut h, 0x54, pm_at - 0x100);
    put32(&mut h, 0x5C, sm_number);
    put32(&mut h, 0x60, pm_number);
    put32(&mut h, 0x64, gaps.len() as u32);
    let crc = crc32(&h);
    put32(&mut h, 0x68, crc);
    let m = mask();
    let masked: Vec<u8> = h.iter().zip(m.iter()).map(|(a, b)| a ^ b).collect();
    out[0..6].copy_from_slice(&spec.version);
    out[0x13..0x15].copy_from_slice(&30u16.to_le_bytes());
    put32(&mut out, 0x18, spec.security);
    out[0x80..0xEC].copy_from_slice(&masked);
    out.extend_from_slice(&masked);
    Built {
        bytes: out,
        data_pages,
        gaps,
    }
}

/// The three sections most rows use: one over several compressed pages, one uncompressed in one
/// page, one over several compressed pages with a short last page.
pub fn standard() -> Spec {
    let pattern =
        |n: usize, k: u8| -> Vec<u8> { (0..n).map(|i| (i.wrapping_mul(13) as u8) ^ k).collect() };
    Spec {
        version: *b"AC1032",
        security: 0,
        sections: vec![
            sec("AcDb:Header", 1, pattern(300, 0x11), 128, true),
            sec("AcDb:Classes", 2, pattern(40, 0x22), 0x7400, false),
            sec("AcDb:AcDbObjects", 3, pattern(1000, 0x33), 400, true),
        ],
        gaps_after: Vec::new(),
    }
}
