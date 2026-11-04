//! Builds zip files byte by byte for the vectors, so each one breaks exactly one rule. Written
//! apart from the crate's own writer: it shares no code with it, and its manifests are text.
#![allow(dead_code)]

use sha2::{Digest, Sha256};

pub fn sha_hex(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

pub const STORE: u16 = 0;
pub const DEFLATE: u16 = 8;

#[derive(Clone)]
pub struct E {
    pub name: Vec<u8>,
    pub data: Vec<u8>,
    pub method: u16,
    pub flags: u16,
    pub local_flags: Option<u16>,
    pub made_by: u16,
    pub external: u32,
    pub extra_central: Vec<u8>,
    pub extra_local: Vec<u8>,
    pub crc: Option<u32>,
    pub csize: Option<u32>,
    pub usize_: Option<u32>,
    pub body: Option<Vec<u8>>,
    pub local_name: Option<Vec<u8>>,
    pub local_method: Option<u16>,
    pub local_crc: Option<u32>,
    pub local_csize: Option<u32>,
    pub local_usize: Option<u32>,
    pub comment: Vec<u8>,
    pub disk_start: u16,
    pub offset: Option<u32>,
    pub no_local: bool,
    pub gap_after: Vec<u8>,
}

impl E {
    pub fn new(name: &str, data: &[u8], method: u16) -> E {
        E {
            name: name.as_bytes().to_vec(),
            data: data.to_vec(),
            method,
            flags: 0,
            local_flags: None,
            made_by: 20,
            external: 0,
            extra_central: Vec::new(),
            extra_local: Vec::new(),
            crc: None,
            csize: None,
            usize_: None,
            body: None,
            local_name: None,
            local_method: None,
            local_crc: None,
            local_csize: None,
            local_usize: None,
            comment: Vec::new(),
            disk_start: 0,
            offset: None,
            no_local: false,
            gap_after: Vec::new(),
        }
    }

    fn compressed(&self) -> Vec<u8> {
        match (&self.body, self.method) {
            (Some(b), _) => b.clone(),
            (None, DEFLATE) => miniz_oxide::deflate::compress_to_vec(&self.data, 6),
            _ => self.data.clone(),
        }
    }

    /// This entry's local header, name, extra and body, as they'd sit in the file.
    pub fn local_bytes(&self) -> Vec<u8> {
        let body = self.compressed();
        let name = self.local_name.clone().unwrap_or_else(|| self.name.clone());
        let mut o = Vec::new();
        o.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        o.extend_from_slice(&20u16.to_le_bytes());
        o.extend_from_slice(&self.local_flags.unwrap_or(self.flags).to_le_bytes());
        o.extend_from_slice(&self.local_method.unwrap_or(self.method).to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        o.extend_from_slice(&0x21u16.to_le_bytes());
        let crc = self.crc.unwrap_or_else(|| crc32(&self.data));
        o.extend_from_slice(&self.local_crc.unwrap_or(crc).to_le_bytes());
        let csize = self.csize.unwrap_or(body.len() as u32);
        o.extend_from_slice(&self.local_csize.unwrap_or(csize).to_le_bytes());
        let usize_ = self.usize_.unwrap_or(self.data.len() as u32);
        o.extend_from_slice(&self.local_usize.unwrap_or(usize_).to_le_bytes());
        o.extend_from_slice(&(name.len() as u16).to_le_bytes());
        o.extend_from_slice(&(self.extra_local.len() as u16).to_le_bytes());
        o.extend_from_slice(&name);
        o.extend_from_slice(&self.extra_local);
        o.extend_from_slice(&body);
        o
    }
}

#[derive(Clone, Default)]
pub struct Forge {
    pub entries: Vec<E>,
    pub prefix: Vec<u8>,
    pub trailing: Vec<u8>,
    pub end_comment: Vec<u8>,
    pub before_end: Vec<u8>,
    pub disk: u16,
    pub count: Option<u16>,
    pub central_size: Option<u32>,
    pub central_at: Option<u32>,
}

impl Forge {
    pub fn new(entries: Vec<E>) -> Forge {
        Forge {
            entries,
            ..Forge::default()
        }
    }

    pub fn build(&self) -> Vec<u8> {
        let mut out = self.prefix.clone();
        let mut offsets = Vec::new();
        for e in &self.entries {
            offsets.push(out.len() as u32);
            if !e.no_local {
                out.extend_from_slice(&e.local_bytes());
                out.extend_from_slice(&e.gap_after);
            }
        }
        let central_at = out.len() as u32;
        let mut central = Vec::new();
        for (e, computed) in self.entries.iter().zip(&offsets) {
            let body = e.compressed();
            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&e.made_by.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&e.flags.to_le_bytes());
            central.extend_from_slice(&e.method.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0x21u16.to_le_bytes());
            central.extend_from_slice(&e.crc.unwrap_or_else(|| crc32(&e.data)).to_le_bytes());
            central.extend_from_slice(&e.csize.unwrap_or(body.len() as u32).to_le_bytes());
            central.extend_from_slice(&e.usize_.unwrap_or(e.data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            central.extend_from_slice(&(e.extra_central.len() as u16).to_le_bytes());
            central.extend_from_slice(&(e.comment.len() as u16).to_le_bytes());
            central.extend_from_slice(&e.disk_start.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&e.external.to_le_bytes());
            central.extend_from_slice(&e.offset.unwrap_or(*computed).to_le_bytes());
            central.extend_from_slice(&e.name);
            central.extend_from_slice(&e.extra_central);
            central.extend_from_slice(&e.comment);
        }
        out.extend_from_slice(&central);
        out.extend_from_slice(&self.before_end);
        let count = self.count.unwrap_or(self.entries.len() as u16);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&self.disk.to_le_bytes());
        out.extend_from_slice(&self.disk.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(
            &self
                .central_size
                .unwrap_or(central.len() as u32)
                .to_le_bytes(),
        );
        out.extend_from_slice(&self.central_at.unwrap_or(central_at).to_le_bytes());
        out.extend_from_slice(&(self.end_comment.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.end_comment);
        out.extend_from_slice(&self.trailing);
        out
    }
}

/// One part to place in a forged package.
pub struct P {
    pub name: &'static str,
    pub media: &'static str,
    pub role: &'static str,
    pub data: Vec<u8>,
}

pub const LINEAGE: &str = "0123456789abcdef0123456789abcdef";

/// A manifest as canonical text, written by hand: members sorted, parts sorted by name.
pub fn manifest_text(
    base: Option<&str>,
    parts: &[(String, String, String, usize, String)],
) -> String {
    let mut sorted: Vec<_> = parts.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let list: Vec<String> = sorted
        .iter()
        .map(|(name, media, role, bytes, hash)| {
            format!(
                "{{\"bytes\":{bytes},\"media_type\":\"{media}\",\"name\":\"{name}\",\"role\":\"{role}\",\"sha256\":\"{hash}\"}}"
            )
        })
        .collect();
    let base = base
        .map(|b| format!("\"base\":{{\"sha256\":\"{b}\"}},"))
        .unwrap_or_default();
    format!(
        "{{{base}\"format\":\"k5board\",\"lineage\":\"{LINEAGE}\",\"min_reader\":1,\"parts\":[{}],\"version\":1}}",
        list.join(",")
    )
}

/// A valid package: its entries (manifest first, stored; board deflated; the rest stored) and its
/// canonical manifest text.
pub fn package(board: &str, others: Vec<P>) -> (Vec<E>, String) {
    let mut rows = vec![(
        "board.json".to_string(),
        "application/json".to_string(),
        "board".to_string(),
        board.len(),
        sha_hex(board.as_bytes()),
    )];
    for p in &others {
        rows.push((
            p.name.to_string(),
            p.media.to_string(),
            p.role.to_string(),
            p.data.len(),
            sha_hex(&p.data),
        ));
    }
    let manifest = manifest_text(None, &rows);
    let mut entries = vec![
        E::new("manifest.json", manifest.as_bytes(), STORE),
        E::new("board.json", board.as_bytes(), DEFLATE),
    ];
    for p in &others {
        entries.push(E::new(p.name, &p.data, STORE));
    }
    (entries, manifest)
}
