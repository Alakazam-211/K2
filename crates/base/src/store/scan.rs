//! Byte scans for purged content (K-PURGE-3, N6): does any copy of a secret remain?
//!
//! A needle matches where it appears **whole**, and also where **any 8-byte window** of it
//! appears ([`MIN_MATCH`]): a secret typed in two batches, or split across two records or
//! two pages, leaves no whole copy but leaves windows. A needle shorter than 8 bytes can
//! only match whole.
//!
//! The same rule decides what a purge redacts in records it didn't target directly (a
//! version's text, another change's revert record): every byte a needle window covers.
//! So a purge followed by a scan for the same needles finds nothing in those records.
//!
//! A scan **reports**; it never judges. Gate tests plant random 32-byte secrets and expect
//! zero findings; a production check shows findings outside live text to a person (engine
//! 1.4 note 30): a short needle can match innocent text.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::ops::Range;
use std::path::Path;

/// Bytes in a needle window: a match of this many bytes of a needle counts.
pub const MIN_MATCH: usize = 8;

/// One match of needle `needle` (its index in the list given): `len` bytes at `offset` of
/// the scanned bytes. Overlapping and adjacent window matches of one needle are merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Finding {
    pub needle: usize,
    pub offset: u64,
    pub len: u64,
}

/// Needles prepared for scanning.
pub(crate) struct Needles {
    /// Every 8-byte window of a long needle → the first needle it came from.
    windows: HashMap<[u8; MIN_MATCH], usize>,
    /// The windows' first two bytes, as a bit set: most positions are rejected here.
    first: Vec<u64>,
    /// Needles shorter than a window, matched whole.
    whole: Vec<(usize, Vec<u8>)>,
}

impl Needles {
    /// Needles of [`MIN_MATCH`] bytes or more match by window; shorter ones match whole when
    /// `short` holds, and are ignored otherwise. Empty needles are ignored.
    pub(crate) fn new<'a>(needles: impl IntoIterator<Item = &'a [u8]>, short: bool) -> Self {
        let mut n = Needles {
            windows: HashMap::new(),
            first: vec![0; 1024],
            whole: Vec::new(),
        };
        for (i, b) in needles.into_iter().enumerate() {
            if b.len() >= MIN_MATCH {
                for w in b.windows(MIN_MATCH) {
                    let k: [u8; MIN_MATCH] = w.try_into().expect("window size");
                    n.windows.entry(k).or_insert(i);
                    let p = u16::from_le_bytes([w[0], w[1]]) as usize;
                    n.first[p / 64] |= 1 << (p % 64);
                }
            } else if short && !b.is_empty() {
                n.whole.push((i, b.to_vec()));
            }
        }
        n
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.windows.is_empty() && self.whole.is_empty()
    }

    /// Every match in `hay`, unmerged, as `(needle, start, end)`.
    fn raw(&self, hay: &[u8], out: &mut Vec<(usize, usize, usize)>) {
        if !self.windows.is_empty() && hay.len() >= MIN_MATCH {
            for i in 0..=hay.len() - MIN_MATCH {
                let p = u16::from_le_bytes([hay[i], hay[i + 1]]) as usize;
                if self.first[p / 64] & (1 << (p % 64)) == 0 {
                    continue;
                }
                let k: [u8; MIN_MATCH] = hay[i..i + MIN_MATCH].try_into().expect("window size");
                if let Some(&n) = self.windows.get(&k) {
                    out.push((n, i, i + MIN_MATCH));
                }
            }
        }
        for (n, w) in &self.whole {
            if hay.len() >= w.len() {
                for i in 0..=hay.len() - w.len() {
                    if &hay[i..i + w.len()] == w.as_slice() {
                        out.push((*n, i, i + w.len()));
                    }
                }
            }
        }
    }

    /// Every match in `hay`, merged per needle.
    pub(crate) fn find(&self, hay: &[u8]) -> Vec<Finding> {
        let mut raw = Vec::new();
        self.raw(hay, &mut raw);
        merge(raw.into_iter().map(|(n, s, e)| (n, s as u64, e as u64)))
    }

    /// The ranges of `hay` any needle covers, merged, in order.
    pub(crate) fn spans(&self, hay: &[u8]) -> Vec<Range<usize>> {
        let mut raw = Vec::new();
        self.raw(hay, &mut raw);
        let mut r: Vec<Range<usize>> = raw.into_iter().map(|(_, s, e)| s..e).collect();
        r.sort_by_key(|x| (x.start, x.end));
        let mut out: Vec<Range<usize>> = Vec::with_capacity(r.len());
        for x in r {
            match out.last_mut() {
                Some(last) if x.start <= last.end => last.end = last.end.max(x.end),
                _ => out.push(x),
            }
        }
        out
    }

    /// The spans of `text` any needle covers, widened to character boundaries.
    pub(crate) fn text_spans(&self, text: &str) -> Vec<Range<usize>> {
        let mut out: Vec<Range<usize>> = Vec::new();
        for mut r in self.spans(text.as_bytes()) {
            while !text.is_char_boundary(r.start) {
                r.start -= 1;
            }
            while !text.is_char_boundary(r.end) {
                r.end += 1;
            }
            match out.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => out.push(r),
            }
        }
        out
    }

    /// Whether any needle matches in `hay`.
    pub(crate) fn hits(&self, hay: &[u8]) -> bool {
        !self.spans(hay).is_empty()
    }
}

/// Merge overlapping or adjacent matches of the same needle.
fn merge(raw: impl Iterator<Item = (usize, u64, u64)>) -> Vec<Finding> {
    let mut v: Vec<(usize, u64, u64)> = raw.collect();
    v.sort_unstable();
    let mut out: Vec<Finding> = Vec::new();
    for (n, s, e) in v {
        match out.last_mut() {
            Some(f) if f.needle == n && s <= f.offset + f.len => {
                f.len = f.len.max(e - f.offset);
            }
            _ => out.push(Finding {
                needle: n,
                offset: s,
                len: e - s,
            }),
        }
    }
    out.sort_by_key(|f| (f.offset, f.needle));
    out
}

/// Every place in `hay` where a needle appears whole or by an 8-byte window (module docs).
pub fn scan_bytes(hay: &[u8], needles: &[&[u8]]) -> Vec<Finding> {
    Needles::new(needles.iter().copied(), true).find(hay)
}

/// [`scan_bytes`] over a file, streamed in 1 MiB chunks (constant memory; matches that
/// cross a chunk boundary are found).
pub fn scan_file(path: &Path, needles: &[&[u8]]) -> io::Result<Vec<Finding>> {
    const CHUNK: usize = 1 << 20;
    let n = Needles::new(needles.iter().copied(), true);
    // the longest match is a window or a short needle: never more than MIN_MATCH bytes
    let keep = MIN_MATCH - 1;
    let mut f = File::open(path)?;
    let mut buf: Vec<u8> = Vec::with_capacity(CHUNK + keep);
    let mut base: u64 = 0; // file offset of buf[0]
    let mut raw = Vec::new();
    let mut found = Vec::new();
    loop {
        let old = buf.len();
        buf.resize(old + CHUNK, 0);
        let got = loop {
            match f.read(&mut buf[old..]) {
                Ok(k) => break k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        };
        buf.truncate(old + got);
        raw.clear();
        n.raw(&buf, &mut raw);
        found.extend(
            raw.iter()
                .map(|&(i, s, e)| (i, base + s as u64, base + e as u64)),
        );
        if got == 0 {
            break;
        }
        let cut = buf.len().saturating_sub(keep);
        buf.drain(..cut);
        base += cut as u64;
    }
    found.sort_unstable();
    found.dedup();
    Ok(merge(found.into_iter()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_windows_and_split_copies() {
        let secret = b"0123456789abcdef-SECRET";
        // a whole copy
        let f = scan_bytes(b"xx0123456789abcdef-SECRETyy", &[secret]);
        assert_eq!(
            f,
            vec![Finding {
                needle: 0,
                offset: 2,
                len: secret.len() as u64
            }]
        );
        // split in two with other bytes between: both halves are found
        let f = scan_bytes(b"0123456789a....bcdef-SECRET", &[secret]);
        assert_eq!(f.len(), 2, "{f:?}");
        // a 7-byte piece is below the window: not found
        assert!(scan_bytes(b"..0123456..", &[secret]).is_empty());
        // short needles match whole only
        assert_eq!(scan_bytes(b"a pin 4821 here", &[b"4821"]).len(), 1);
        assert!(scan_bytes(b"482", &[b"4821"]).is_empty());
        // several needles keep their index
        let f = scan_bytes(b"..BBBBBBBBBB..", &[b"AAAAAAAAAA", b"BBBBBBBBBB"]);
        assert_eq!(f[0].needle, 1);
    }

    #[test]
    fn text_spans_cover_matches_at_char_boundaries() {
        let n = Needles::new([&b"\xc3\xa9t\xc3\xa9-secret"[..]], true);
        let t = "before été-secret after";
        let sp = n.text_spans(t);
        assert_eq!(sp.len(), 1);
        assert_eq!(&t[sp[0].clone()], "été-secret");
        // ignored when short is off
        assert!(Needles::new([&b"abc"[..]], false).is_empty());
    }

    #[test]
    fn file_scan_finds_matches_across_chunk_boundaries() {
        let dir = std::env::temp_dir().join(format!("k2mb-scan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f");
        let secret = b"S3CRET-across-the-boundary!";
        let mut data = vec![b'.'; (1 << 20) - 10];
        data.extend_from_slice(secret);
        data.extend(vec![b'.'; 100]);
        std::fs::write(&p, &data).unwrap();
        let f = scan_file(&p, &[secret]).unwrap();
        assert_eq!(
            f,
            vec![Finding {
                needle: 0,
                offset: (1 << 20) - 10,
                len: secret.len() as u64
            }]
        );
        assert_eq!(f, scan_bytes(&data, &[secret]));
        std::fs::write(&p, vec![b'.'; 3 << 20]).unwrap();
        assert!(scan_file(&p, &[secret]).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
