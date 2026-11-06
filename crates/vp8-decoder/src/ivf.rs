//! A reader for IVF, the simple container the VP8 test vectors use.
//!
//! An IVF file is a 32-byte header (`DKIF`, version, header size, FourCC,
//! width, height, frame rate, frame count) followed by frames, each a 12-byte
//! header (little-endian size and 64-bit timestamp) and its data.

use std::fmt;

/// The IVF file header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct IvfHeader {
    /// The codec's FourCC, `VP80` for VP8.
    pub fourcc: [u8; 4],
    /// Width from the header (the frames carry their own size).
    pub width: u16,
    /// Height from the header.
    pub height: u16,
    /// Time base numerator and denominator.
    pub timebase: (u32, u32),
    /// The frame count the header claims.
    pub frame_count: u32,
}

/// One IVF frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct IvfFrame<'a> {
    /// Presentation timestamp in time-base units.
    pub timestamp: u64,
    /// The frame data (one raw VP8 frame).
    pub data: &'a [u8],
}

/// A malformed IVF file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IvfError(&'static str);

impl fmt::Display for IvfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad IVF file: {}", self.0)
    }
}

impl std::error::Error for IvfError {}

/// Iterates over the frames of an IVF file held in memory.
pub struct IvfReader<'a> {
    header: IvfHeader,
    data: &'a [u8],
    pos: usize,
}

impl<'a> IvfReader<'a> {
    /// Parses the file header.
    pub fn new(data: &'a [u8]) -> Result<Self, IvfError> {
        if data.len() < 32 || &data[..4] != b"DKIF" {
            return Err(IvfError("no DKIF signature"));
        }
        let u16_at = |i: usize| u16::from_le_bytes([data[i], data[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        let header_len = u16_at(6) as usize;
        if header_len < 32 || header_len > data.len() {
            return Err(IvfError("bad header length"));
        }
        let header = IvfHeader {
            fourcc: [data[8], data[9], data[10], data[11]],
            width: u16_at(12),
            height: u16_at(14),
            timebase: (u32_at(20), u32_at(16)),
            frame_count: u32_at(24),
        };
        Ok(IvfReader { header, data, pos: header_len })
    }

    /// The file header.
    pub fn header(&self) -> IvfHeader {
        self.header
    }
}

impl<'a> Iterator for IvfReader<'a> {
    type Item = Result<IvfFrame<'a>, IvfError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos == self.data.len() {
            return None;
        }
        let rest = &self.data[self.pos..];
        if rest.len() < 12 {
            self.pos = self.data.len();
            return Some(Err(IvfError("truncated frame header")));
        }
        let size = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        let mut ts = [0u8; 8];
        ts.copy_from_slice(&rest[4..12]);
        if rest.len() - 12 < size {
            self.pos = self.data.len();
            return Some(Err(IvfError("truncated frame")));
        }
        self.pos += 12 + size;
        Some(Ok(IvfFrame {
            timestamp: u64::from_le_bytes(ts),
            data: &rest[12..12 + size],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_frames() {
        let mut f = Vec::new();
        f.extend_from_slice(b"DKIF");
        f.extend_from_slice(&0u16.to_le_bytes());
        f.extend_from_slice(&32u16.to_le_bytes());
        f.extend_from_slice(b"VP80");
        f.extend_from_slice(&176u16.to_le_bytes());
        f.extend_from_slice(&144u16.to_le_bytes());
        f.extend_from_slice(&30u32.to_le_bytes());
        f.extend_from_slice(&1u32.to_le_bytes());
        f.extend_from_slice(&2u32.to_le_bytes());
        f.extend_from_slice(&0u32.to_le_bytes());
        for (i, payload) in [&b"abc"[..], &b"de"[..]].iter().enumerate() {
            f.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            f.extend_from_slice(&(i as u64).to_le_bytes());
            f.extend_from_slice(payload);
        }
        let r = IvfReader::new(&f).expect("header");
        assert_eq!(r.header().fourcc, *b"VP80");
        assert_eq!((r.header().width, r.header().height), (176, 144));
        let frames: Vec<_> = r.map(|f| f.expect("frame")).collect();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].data, b"de");
        assert_eq!(frames[1].timestamp, 1);
    }

    #[test]
    fn rejects_truncation() {
        let mut f = vec![0u8; 32];
        f[..4].copy_from_slice(b"DKIF");
        f[6] = 32;
        f.extend_from_slice(&100u32.to_le_bytes());
        f.extend_from_slice(&[0u8; 8]);
        f.extend_from_slice(b"short");
        let mut r = IvfReader::new(&f).expect("header");
        assert_eq!(r.next(), Some(Err(IvfError("truncated frame"))));
        assert_eq!(r.next(), None);
    }
}
