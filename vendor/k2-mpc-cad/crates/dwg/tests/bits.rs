//! The bit-coded values over hand-written bits: every code of each coded type, read off a byte
//! boundary (a two-bit code comes first), and the reads that must be refused.

mod support;

use k2_mpc_dwg::bits::{Bits, Colour, Handle};
use support::{bytes, coded, ok, pack, refused};

#[test]
pub fn fixed_and_coded_values() {
    // B, BB, RC, RS, RL, RD after one bit: none of them on a byte boundary
    let mut f = vec![(1, 1), (0b10, 2)];
    f.extend(bytes(&[0xA5, 0x34, 0x12, 0x78, 0x56, 0x34, 0x12]));
    f.extend(bytes(&1.5f64.to_le_bytes()));
    let buf = pack(&f);
    let mut r = Bits::new(&buf);
    ok(r.b(), true, "B");
    ok(r.bb(), 2, "BB");
    ok(r.rc(), 0xA5, "RC");
    ok(r.rs(), 0x1234, "RS, little-endian");
    ok(r.rl(), 0x1234_5678, "RL, little-endian");
    ok(r.rd(), 1.5, "RD");
    // BS: a short, a byte, 0, 256
    let mut f = coded(0b00, &[0x34, 0x12]);
    f.extend(coded(0b01, &[0xAB]));
    f.extend([(0b10, 2), (0b11, 2)]);
    let buf = pack(&f);
    let mut r = Bits::new(&buf);
    for (want, what) in [
        (0x1234, "BS 00"),
        (0xAB, "BS 01"),
        (0, "BS 10"),
        (256, "BS 11"),
    ] {
        ok(r.bs(), want, what);
    }
    // BL: a long, a byte, 0; code 3 refused
    let mut f = coded(0b00, &[0x78, 0x56, 0x34, 0x12]);
    f.extend(coded(0b01, &[0xCD]));
    f.extend([(0b10, 2), (0b11, 2)]);
    let buf = pack(&f);
    let mut r = Bits::new(&buf);
    for (want, what) in [(0x1234_5678, "BL 00"), (0xCD, "BL 01"), (0, "BL 10")] {
        ok(r.bl(), want, what);
    }
    refused(r.bl(), "DWG-E003", "BL code 3");
    // BD: a double, 1.0, 0.0; code 3 refused
    let mut f = coded(0b00, &(-2.25f64).to_le_bytes());
    f.extend([(0b01, 2), (0b10, 2), (0b11, 2)]);
    let buf = pack(&f);
    let mut r = Bits::new(&buf);
    for (want, what) in [(-2.25, "BD 00"), (1.0, "BD 01"), (0.0, "BD 10")] {
        ok(r.bd(), want, what);
    }
    refused(r.bd(), "DWG-E003", "BD code 3");
}

#[test]
pub fn default_relative_doubles() {
    // the default 1.0 is, little-endian: 00 00 00 00 00 00 F0 3F
    let mut f = vec![(0b00, 2)];
    f.extend(coded(0b01, &[0x01, 0x00, 0x00, 0x00]));
    f.extend(coded(0b10, &[0x08, 0x00, 0x02, 0x00, 0x00, 0x00]));
    f.extend(coded(0b11, &7.0f64.to_le_bytes()));
    let buf = pack(&f);
    let mut r = Bits::new(&buf);
    ok(r.dd(1.0), 1.0, "DD 00: the default");
    ok(
        r.dd(1.0),
        f64::from_bits(0x3FF0_0000_0000_0001),
        "DD 01: the first four bytes replaced",
    );
    ok(
        r.dd(1.0),
        f64::from_bits(0x3FF0_0008_0000_0002),
        "DD 10: the fifth and sixth bytes, then the first four",
    );
    ok(r.dd(1.0), 7.0, "DD 11: a whole double");
}

#[test]
pub fn modular_numbers() {
    let buf = [
        0x82, 0x24, // MC 4610
        0xF0, 0x40, // MC -112
        0x05, // MC 5
        0x45, // MC -5
        0x82, 0x24, // UMC 4610
        0xFF, 0x7F, // UMC 0x3FFF
        0x31, 0x80, 0x02, 0x00, // MS 0x10031
        0x34, 0x12, // MS 0x1234
    ];
    let mut r = Bits::new(&buf);
    ok(r.mc(), 4610, "MC, two bytes");
    ok(r.mc(), -112, "MC, negative, the sign in the last byte");
    ok(r.mc(), 5, "MC, one byte");
    ok(r.mc(), -5, "MC, one byte, negative");
    ok(r.umc(), 4610, "UMC");
    ok(r.umc(), 0x3FFF, "UMC, seven bits in its last byte");
    ok(r.ms(), 0x1_0031, "MS, two words");
    ok(r.ms(), 0x1234, "MS, one word");
    let long = [0x80u8; 10];
    refused(
        Bits::new(&long).umc(),
        "DWG-E003",
        "a modular number over nine bytes",
    );
    let long = [0x00u8, 0x80, 0x00, 0x80, 0x00, 0x80, 0x00, 0x80, 0x00, 0x00];
    refused(
        Bits::new(&long).ms(),
        "DWG-E003",
        "a modular short over four words",
    );
}

#[test]
pub fn handle_references() {
    let buf = [
        0x51, 0x2A, // code 5, one byte: 0x2A
        0x32, 0x01, 0x02, // code 3, two bytes, most significant first: 0x0102
        0x60, // code 6: own + 1
        0x80, // code 8: own - 1
        0xA1, 0x05, // code 0xA: own + 5
        0xC1, 0x05, // code 0xC: own - 5
        0x40, // code 4, no bytes: 0
        0x29, // a length of 9: refused
    ];
    let mut r = Bits::new(&buf);
    let own = 0x100;
    let mut read = |what: &str| -> Handle {
        r.h()
            .unwrap_or_else(|e| panic!("{what}: refused: {}", e.summary()))
    };
    let got: Vec<(u8, u64, Option<u64>)> = [
        "absolute",
        "absolute, two bytes",
        "plus one",
        "minus one",
        "plus an offset",
        "minus an offset",
        "a null pointer",
    ]
    .iter()
    .map(|w| {
        let h = read(w);
        (h.code, h.value, h.resolve(own))
    })
    .collect();
    assert_eq!(
        got,
        [
            (5, 0x2A, Some(0x2A)),
            (3, 0x0102, Some(0x0102)),
            (6, 0, Some(0x101)),
            (8, 0, Some(0xFF)),
            (0xA, 5, Some(0x105)),
            (0xC, 5, Some(0xFB)),
            (4, 0, Some(0)),
        ],
        "each code, from an object whose handle is 0x100"
    );
    refused(r.h(), "DWG-E003", "a handle nine bytes long");
    let low = Handle {
        code: 0xC,
        value: 5,
    };
    assert_eq!(low.resolve(3), None, "a relative reference below handle 0");
}

#[test]
pub fn colours_texts_and_types() {
    // a colour index alone; a true colour; a colour book (its colour in the handle stream, no
    // true colour after it) and a transparency
    let mut f = coded(0b01, &[0x07]);
    f.extend(coded(0b00, &[0x00, 0x80]));
    f.extend(coded(0b00, &[0x78, 0x56, 0x34, 0x12]));
    f.extend(coded(0b00, &[0x00, 0xE0]));
    f.extend(coded(0b01, &[0x22]));
    // TU "Ab": a length of 2, then two UTF-16 units
    f.extend(coded(0b01, &[0x02]));
    f.extend(bytes(&[0x41, 0x00, 0x62, 0x00]));
    // OT: a byte, a byte plus 0x1F0, a short
    f.extend(coded(0b00, &[0x13]));
    f.extend(coded(0b01, &[0x02]));
    f.extend(coded(0b10, &[0xF4, 0x01]));
    let buf = pack(&f);
    let mut r = Bits::new(&buf);
    let colour = |index, rgb, book, transparency| Colour {
        index,
        rgb,
        book,
        transparency,
    };
    ok(r.cmc(), colour(7, None, false, None), "a colour index");
    ok(
        r.cmc(),
        colour(0, Some(0x1234_5678), false, None),
        "a true colour",
    );
    ok(
        r.cmc(),
        colour(0, None, true, Some(0x22)),
        "a colour book and a transparency",
    );
    ok(r.tu(), "Ab".to_owned(), "TU");
    ok(r.ot(), 0x13, "OT 00: a byte");
    ok(r.ot(), 0x1F2, "OT 01: a byte plus 0x1F0");
    ok(r.ot(), 0x01F4, "OT 10: a short");
}

#[test]
pub fn past_the_end_refused() {
    let buf = [0xFFu8];
    let mut r = Bits::new(&buf);
    ok(r.bits(4), 0xF, "four of eight bits");
    refused(r.rc(), "DWG-E002", "eight bits with four left");
    let mut r = Bits::span(&buf, 0, 6);
    refused(r.bits(7), "DWG-E002", "seven bits from a six-bit span");
    // a TU whose length is longer than the bits left: refused, nothing read
    let buf = pack(&coded(0b01, &[0x40]));
    refused(
        Bits::new(&buf).tu(),
        "DWG-E002",
        "a text longer than its bytes",
    );
}
