//! The checks the container is written with: the CRC-16 over objects, the object map and the
//! header and classes sections; the CRC-32 over the file header; and the page checksum over
//! each page's header and data.
//!
//! From the specification's chapters on the 2004 file format (the file header, the system and
//! data pages) and on the object map; our own code.

use crate::mutant;

/// The seed the container's CRC-16s start from.
pub const CRC16_SEED: u16 = 0xC0C1;

/// The CRC-16 of `data` from `seed`: the reflected polynomial 0xA001, no final XOR.
pub fn crc16(seed: u16, data: &[u8]) -> u16 {
    let mut c = if mutant::active("Crc16SeedIgnored") {
        0
    } else {
        seed
    };
    for &b in data {
        c ^= u16::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xA001
            } else {
                c >> 1
            };
        }
    }
    c
}

/// The CRC-32 of `data`: the reflected polynomial 0xEDB88320, seeded and finished with all ones
/// (zlib's).
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = u32::MAX;
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
    if mutant::active("Crc32NoFinalXor") {
        c
    } else {
        !c
    }
}

/// The page checksum of `data` from `seed`: two running sums modulo 0xFFF1, reduced every
/// 0x15B0 bytes, the second in the high half. From seed 1 it is Adler-32.
pub fn page_checksum(seed: u32, data: &[u8]) -> u32 {
    let mut s1 = u64::from(seed & 0xFFFF);
    let mut s2 = u64::from(seed >> 16);
    for block in data.chunks(0x15B0) {
        for &b in block {
            s1 += u64::from(b);
            s2 += s1;
        }
        s1 %= 0xFFF1;
        s2 %= 0xFFF1;
    }
    // both below 0xFFF1 here: they fit their halves
    let (s1, s2) = (s1 as u32, s2 as u32);
    if mutant::active("ChecksumHalvesSwapped") {
        (s1 << 16) | s2
    } else {
        (s2 << 16) | s1
    }
}
