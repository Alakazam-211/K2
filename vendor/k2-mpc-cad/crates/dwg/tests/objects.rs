//! The object map, objects' heads and the class list over hand-made bytes: map blocks with
//! their deltas and CRCs, an object's head in the 2004 and in the 2010 layout, and a class list
//! in each layout (its names in a string stream, or inline).

mod support;

use k2_mpc_dwg::classes::{classes, Class};
use k2_mpc_dwg::crc::{crc16, CRC16_SEED};
use k2_mpc_dwg::objects::{object_head, object_map, MapEntry, ObjectHead};
use k2_mpc_dwg::Limits;
use support::{bytes, coded, pack, refused};

/// Bit fields, `(value, width)`.
type Fields = Vec<(u64, u32)>;

/// A map block: its big-endian size, its entries, its big-endian CRC-16.
fn block(entries: &[u8]) -> Vec<u8> {
    let mut b = ((entries.len() + 2) as u16).to_be_bytes().to_vec();
    b.extend_from_slice(entries);
    let crc = crc16(CRC16_SEED, &b);
    b.extend_from_slice(&crc.to_be_bytes());
    b
}

fn map_bytes() -> Vec<u8> {
    let mut m = block(&[0x01, 0x10, 0x02, 0x28]); // +1 at +16, +2 at +40
    m.extend(block(&[0x05, 0xE4, 0x00, 0x01, 0x54])); // from 0: +5 at +100, +1 at -20
    m.extend_from_slice(&[0x00, 0x02]); // the end
    m
}

#[test]
pub fn object_map_blocks() {
    let got = object_map(&map_bytes(), &Limits::default())
        .map(|v| v.iter().map(|e| (e.handle, e.location)).collect::<Vec<_>>());
    assert_eq!(
        got,
        Ok(vec![(1, 16), (3, 56), (5, 100), (6, 80)]),
        "each block's deltas start from 0"
    );
    let one = MapEntry {
        handle: 1,
        location: 16,
    };
    assert_eq!(
        object_map(&map_bytes(), &Limits::default()).map(|v| v[0]),
        Ok(one)
    );
}

#[test]
pub fn object_map_checked() {
    let mut damaged = map_bytes();
    damaged[3] ^= 0x01;
    refused(
        object_map(&damaged, &Limits::default()),
        "DWG-E006",
        "a map block's byte changed",
    );
    let mut before = block(&[0x01, 0x41]); // +1 at -1
    before.extend_from_slice(&[0x00, 0x02]);
    refused(
        object_map(&before, &Limits::default()),
        "DWG-E006",
        "a location before the start",
    );
    refused(
        object_map(&[0x00, 0x09, 0x01], &Limits::default()),
        "DWG-E002",
        "a block past the end",
    );
}

/// An object at byte 3 of a buffer: its size and head fields, then its CRC-16 from its start.
fn object(size_fields: &[u8], data: &[u8]) -> (Vec<u8>, u64) {
    let mut buf = vec![0xEE, 0xEE, 0xEE];
    let at = buf.len();
    buf.extend_from_slice(size_fields);
    buf.extend_from_slice(data);
    let crc = crc16(CRC16_SEED, &buf[at..]);
    buf.extend_from_slice(&crc.to_le_bytes());
    buf.extend_from_slice(&[0xEE, 0xEE]);
    (buf, at as u64)
}

/// A 2010-layout object: 4 data bytes (`MS` 4), a handle stream of 8 bits (`UMC`), then the
/// type 0x13 (`OT` 00) and the handle 0x2A (`H`, code 0, one byte).
fn modern() -> (Vec<u8>, u64) {
    let mut f = coded(0b00, &[0x13]);
    f.extend(bytes(&[0x01, 0x2A]));
    object(&[0x04, 0x00, 0x08], &pack(&f))
}

/// A 2004-layout object: 9 data bytes (`MS` 9), then the type 0x30 (`BS` 01), its size in bits
/// (`RL`), the handle 0x0105 (`H`, code 0, two bytes).
fn older() -> (Vec<u8>, u64) {
    let mut f = coded(0b01, &[0x30]);
    f.extend(bytes(&[0x50, 0x00, 0x00, 0x00, 0x02, 0x01, 0x05]));
    object(&[0x09, 0x00], &pack(&f))
}

#[test]
pub fn object_heads_by_version() {
    let (buf, at) = modern();
    assert_eq!(
        object_head(&buf, at, "AC1032"),
        Ok(ObjectHead {
            location: at,
            size: 4,
            kind: 0x13,
            handle: 0x2A
        }),
        "the 2010 layout"
    );
    let (buf, at) = older();
    assert_eq!(
        object_head(&buf, at, "AC1018"),
        Ok(ObjectHead {
            location: at,
            size: 9,
            kind: 0x30,
            handle: 0x0105
        }),
        "the 2004 layout"
    );
}

#[test]
pub fn object_crc_checked() {
    let (mut buf, at) = modern();
    buf[at as usize + 4] ^= 0x01;
    refused(
        object_head(&buf, at, "AC1032"),
        "DWG-E006",
        "an object's byte changed",
    );
    let (buf, at) = modern();
    refused(
        object_head(&buf[..at as usize + 6], at, "AC1032"),
        "DWG-E002",
        "an object past its section's end",
    );
}

/// `TU` fields: a length (`BS` 01), then the UTF-16 units.
fn tu(s: &str) -> Fields {
    let units: Vec<u16> = s.encode_utf16().collect();
    let mut f = coded(0b01, &[units.len() as u8]);
    for u in units {
        f.extend(bytes(&u.to_le_bytes()));
    }
    f
}

/// `TV` fields: a length (`BS` 01), then the bytes.
fn tv(s: &str) -> Fields {
    let mut f = coded(0b01, &[s.len() as u8]);
    f.extend(bytes(s.as_bytes()));
    f
}

/// One class's fields before and after its names: number, proxy flags (`BS` 10: 0), then
/// zombie, item class id, instances and four zero longs.
fn class_fields(number: u16, item: u16, instances: u8) -> (Fields, Fields) {
    let mut before = coded(0b00, &number.to_le_bytes());
    before.push((0b10, 2));
    let mut after = vec![(0, 1)];
    after.extend(coded(0b00, &item.to_le_bytes()));
    after.extend(coded(0b01, &[instances]));
    after.extend([(0b10, 2), (0b10, 2), (0b10, 2), (0b10, 2)]);
    (before, after)
}

/// A classes section: the sentinel, the size (and a second long when `extra`), the data, the
/// CRC-16 over the sizes and the data, the sentinel's complement.
fn section(data: &[u8], extra: bool) -> Vec<u8> {
    let sentinel: Vec<u8> = (0..16u8).map(|i| i.wrapping_mul(37) ^ 0x5A).collect();
    let mut s = sentinel.clone();
    s.extend_from_slice(&(data.len() as u32).to_le_bytes());
    if extra {
        s.extend_from_slice(&[0, 0, 0, 0]);
    }
    s.extend_from_slice(data);
    let crc = crc16(CRC16_SEED, &s[16..]);
    s.extend_from_slice(&crc.to_le_bytes());
    s.extend(sentinel.iter().map(|b| !b));
    s
}

fn want() -> Vec<Class> {
    let class = |number, cpp: &str, dxf: &str, item, instances| Class {
        number,
        proxy_flags: 0,
        app_name: "TestApp".into(),
        cpp_name: cpp.into(),
        dxf_name: dxf.into(),
        zombie: false,
        item_class_id: item,
        instances,
    };
    vec![
        class(500, "TestClassA", "TESTA", 0x1F3, 3),
        class(501, "TestClassB", "TESTB", 0x1F2, 1),
    ]
}

/// The 2010 layout: the names in a string stream at the data's end.
fn modern_classes() -> Vec<u8> {
    let mut d = coded(0b00, &501u16.to_le_bytes());
    d.extend([(0, 8), (0, 8), (1, 1)]);
    let mut strings = Vec::new();
    for (n, cpp, dxf, item, inst) in [
        (500, "TestClassA", "TESTA", 0x1F3, 3),
        (501, "TestClassB", "TESTB", 0x1F2, 1),
    ] {
        let (before, after) = class_fields(n, item, inst);
        d.extend(before);
        d.extend(after);
        strings.extend(tu("TestApp"));
        strings.extend(tu(cpp));
        strings.extend(tu(dxf));
    }
    let width = |f: &[(u64, u32)]| f.iter().map(|x| x.1 as usize).sum::<usize>();
    let string_bits = width(&strings);
    let mut all = d.clone();
    all.extend(strings);
    all.extend(bytes(&(string_bits as u16).to_le_bytes()));
    all.push((1, 1));
    let bit_size = 32 + width(&all);
    let mut data = (bit_size as u32).to_le_bytes().to_vec();
    data.extend(pack(&all));
    section(&data, true)
}

/// The 2004 layout: the names inline.
fn older_classes() -> Vec<u8> {
    let mut d = coded(0b00, &501u16.to_le_bytes());
    d.extend([(0, 8), (0, 8), (1, 1)]);
    for (n, cpp, dxf, item, inst) in [
        (500, "TestClassA", "TESTA", 0x1F3, 3),
        (501, "TestClassB", "TESTB", 0x1F2, 1),
    ] {
        let (before, after) = class_fields(n, item, inst);
        d.extend(before);
        d.extend(tv("TestApp"));
        d.extend(tv(cpp));
        d.extend(tv(dxf));
        d.extend(after);
    }
    section(&pack(&d), false)
}

#[test]
pub fn classes_from_a_string_stream() {
    assert_eq!(
        classes(&modern_classes(), "AC1032", 4),
        Ok(want()),
        "the 2010 layout"
    );
    assert_eq!(
        classes(&modern_classes(), "AC1032", 0),
        Ok(want()),
        "2018 writes the second size long whatever its maintenance version"
    );
}

#[test]
pub fn classes_inline() {
    assert_eq!(
        classes(&older_classes(), "AC1018", 0),
        Ok(want()),
        "the 2004 layout"
    );
}

#[test]
pub fn classes_checked() {
    let whole = classes(&modern_classes(), "AC1032", 4).map(|v| v.len());
    assert_eq!(whole, Ok(2), "the undamaged list reads");
    let mut damaged = modern_classes();
    damaged[30] ^= 0x01;
    refused(
        classes(&damaged, "AC1032", 4),
        "DWG-E006",
        "a class byte changed",
    );
    let mut damaged = modern_classes();
    let n = damaged.len();
    damaged[n - 1] ^= 0x01;
    refused(
        classes(&damaged, "AC1032", 4),
        "DWG-E006",
        "the closing sentinel changed",
    );
}
