//! An independent zip writer for test fixtures: none of the crate's own code. It writes every
//! layout the reader must reproduce: stored and deflated entries at three levels (with the
//! level bits set), data descriptors with and without their signature, local and central
//! extra fields that differ, entry and archive comments, times, the UTF-8 flag, folder
//! entries, and the version and attribute fields of other writers.
#![allow(dead_code)]

use std::io::Write;

use flate2::write::DeflateEncoder;
use flate2::Compression;

/// Where an entry's sizes and CRC go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desc {
    /// In the local header.
    None,
    /// In a data descriptor with its signature.
    Sig,
    /// In a data descriptor without one.
    NoSig,
}

/// One entry and how it is written.
#[derive(Debug, Clone)]
pub struct ZEntry {
    pub name: String,
    pub content: Vec<u8>,
    /// 0 (stored) or 8 (deflate).
    pub method: u16,
    /// The deflate level, 0..=9.
    pub level: u32,
    pub desc: Desc,
    pub local_extra: Vec<u8>,
    pub central_extra: Vec<u8>,
    pub comment: Vec<u8>,
    pub time: u16,
    pub date: u16,
    pub made_by: u16,
    pub external_attr: u32,
    /// Set bit 11 (the name is UTF-8).
    pub utf8: bool,
    /// Damage: bytes written after the local record (a gap).
    pub junk_after: Vec<u8>,
    /// Damage: no local record is written; the directory points at `offset` instead.
    pub nested_at: Option<u32>,
}

impl ZEntry {
    pub fn new(name: &str, content: &[u8]) -> ZEntry {
        ZEntry {
            name: name.to_owned(),
            content: content.to_vec(),
            method: 8,
            level: 6,
            desc: Desc::None,
            local_extra: Vec::new(),
            central_extra: Vec::new(),
            comment: Vec::new(),
            time: 0x6000,
            date: 0x5A21,
            made_by: 20,
            external_attr: 0,
            utf8: false,
            junk_after: Vec::new(),
            nested_at: None,
        }
    }

    /// The compressed data.
    pub fn data(&self) -> Vec<u8> {
        if self.method == 0 {
            self.content.clone()
        } else {
            deflate(&self.content, self.level)
        }
    }

    fn flags(&self) -> u16 {
        let mut flags = 0u16;
        if self.method == 8 {
            flags |= match self.level {
                0..=2 => 0b100,
                9 => 0b010,
                _ => 0,
            };
        }
        if self.desc != Desc::None {
            flags |= 1 << 3;
        }
        if self.utf8 {
            flags |= 1 << 11;
        }
        flags
    }

    /// The whole local record: header, data and any descriptor.
    pub fn local_record(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let data = self.data();
        let crc = crc32(&self.content);
        let (lcrc, lc, lu) = if self.desc == Desc::None {
            (crc, data.len() as u32, self.content.len() as u32)
        } else {
            (0, 0, 0)
        };
        p32(&mut out, 0x0403_4B50);
        p16(&mut out, 20);
        p16(&mut out, self.flags());
        p16(&mut out, self.method);
        p16(&mut out, self.time);
        p16(&mut out, self.date);
        p32(&mut out, lcrc);
        p32(&mut out, lc);
        p32(&mut out, lu);
        p16(&mut out, self.name.len() as u16);
        p16(&mut out, self.local_extra.len() as u16);
        out.extend_from_slice(self.name.as_bytes());
        out.extend_from_slice(&self.local_extra);
        out.extend_from_slice(&data);
        match self.desc {
            Desc::None => {}
            Desc::Sig => {
                p32(&mut out, 0x0807_4B50);
                p32(&mut out, crc);
                p32(&mut out, data.len() as u32);
                p32(&mut out, self.content.len() as u32);
            }
            Desc::NoSig => {
                p32(&mut out, crc);
                p32(&mut out, data.len() as u32);
                p32(&mut out, self.content.len() as u32);
            }
        }
        out
    }
}

/// The CRC-32 of zip, written out here (no library).
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn deflate(content: &[u8], level: u32) -> Vec<u8> {
    let mut w = DeflateEncoder::new(Vec::new(), Compression::new(level));
    w.write_all(content).expect("deflate");
    w.finish().expect("deflate")
}

fn p16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn p32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

/// A zip file of `entries`, in order, with an archive comment.
pub fn build(entries: &[ZEntry], archive_comment: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut cd = Vec::new();
    for e in entries {
        let data = e.data();
        let crc = crc32(&e.content);
        let flags = e.flags();
        let offset = match e.nested_at {
            Some(at) => at,
            None => {
                let at = out.len() as u32;
                out.extend_from_slice(&e.local_record());
                out.extend_from_slice(&e.junk_after);
                at
            }
        };
        p32(&mut cd, 0x0201_4B50);
        p16(&mut cd, e.made_by);
        p16(&mut cd, 20);
        p16(&mut cd, flags);
        p16(&mut cd, e.method);
        p16(&mut cd, e.time);
        p16(&mut cd, e.date);
        p32(&mut cd, crc);
        p32(&mut cd, data.len() as u32);
        p32(&mut cd, e.content.len() as u32);
        p16(&mut cd, e.name.len() as u16);
        p16(&mut cd, e.central_extra.len() as u16);
        p16(&mut cd, e.comment.len() as u16);
        p16(&mut cd, 0);
        p16(&mut cd, 0);
        p32(&mut cd, e.external_attr);
        p32(&mut cd, offset);
        cd.extend_from_slice(e.name.as_bytes());
        cd.extend_from_slice(&e.central_extra);
        cd.extend_from_slice(&e.comment);
    }
    let cd_off = out.len() as u32;
    out.extend_from_slice(&cd);
    p32(&mut out, 0x0605_4B50);
    p16(&mut out, 0);
    p16(&mut out, 0);
    p16(&mut out, entries.len() as u16);
    p16(&mut out, entries.len() as u16);
    p32(&mut out, cd.len() as u32);
    p32(&mut out, cd_off);
    p16(&mut out, archive_comment.len() as u16);
    out.extend_from_slice(archive_comment);
    out
}

/// A tiny deterministic generator (xorshift), so fixtures are the same on every machine.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}
