//! The container's checks against published check values: the CRC-16 (from seed 0, the
//! catalogue's CRC-16/ARC), the CRC-32 (zlib's) and the page checksum (from seed 1, Adler-32),
//! over the catalogue's `123456789` and over a 20,000-byte run that crosses the checksum's
//! reduction blocks.

mod support;

use k2_mpc_dwg::crc::{crc16, crc32, page_checksum, CRC16_SEED};

/// 20,000 bytes, every value, across several of the page checksum's 0x15B0-byte blocks.
fn long_run() -> Vec<u8> {
    (0..20_000u32)
        .map(|i| (i.wrapping_mul(31).wrapping_add(7) & 0xFF) as u8)
        .collect()
}

#[test]
pub fn crc16_vectors() {
    assert_eq!(
        crc16(0, b"123456789"),
        0xBB3D,
        "the catalogue's check value"
    );
    assert_eq!(crc16(CRC16_SEED, b""), CRC16_SEED, "nothing read: the seed");
    assert_eq!(
        crc16(CRC16_SEED, b"123456789"),
        0x7E6C,
        "from the container's seed"
    );
    assert_eq!(crc16(CRC16_SEED, &long_run()), 0xAF97);
}

#[test]
pub fn crc32_vectors() {
    assert_eq!(
        crc32(b"123456789"),
        0xCBF4_3926,
        "the catalogue's check value"
    );
    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32(&long_run()), 0xAF84_1263, "zlib's, over the long run");
}

#[test]
pub fn page_checksum_vectors() {
    assert_eq!(
        page_checksum(1, b"123456789"),
        0x091E_01DE,
        "Adler-32's check value"
    );
    assert_eq!(
        page_checksum(1, &long_run()),
        0xA42C_EB2B,
        "zlib's Adler-32, across blocks"
    );
    assert_eq!(
        page_checksum(1, &[0xFF; 12_000]),
        0x09B1_B3D3,
        "every byte at its largest"
    );
    assert_eq!(
        page_checksum(0x0002_0003, b""),
        0x0002_0003,
        "nothing read: the seed"
    );
}
