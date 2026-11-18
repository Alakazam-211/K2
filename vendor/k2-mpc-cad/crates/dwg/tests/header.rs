//! The header variables over a hand-made 2004 section: every variable in its order, most of
//! them zero, a few set (LTSCALE, PDMODE, PDSIZE, FILLMODE, PSLTSCALE, MIRRTEXT, the next
//! handle, model space's extents, INSUNITS), read back; and a header longer than its variables
//! refused.

mod support;

use k2_mpc_dwg::crc::{crc16, CRC16_SEED};
use k2_mpc_dwg::frame::Ver;
use k2_mpc_dwg::header::header;
use support::object::{b, bd, bd3, bl, bs, h, rc, rd, tu, tv, F};
use support::{pack, refused};

fn n(k: usize, f: F) -> F {
    (0..k).flat_map(|_| f.clone()).collect()
}

/// An object colour, zero.
fn cmc() -> F {
    [bs(0), bl(0), rc(0)].concat()
}

/// One space's block of variables: base, extents, limits, elevation, UCS, its orthographic
/// UCS and the six origins.
fn space(min: [f64; 3], max: [f64; 3]) -> F {
    [
        bd3([0.0; 3]),
        bd3(min),
        bd3(max),
        rd(0.0),
        rd(0.0),
        rd(0.0),
        rd(0.0),
        bd(0.0),
        n(3, bd3([0.0; 3])),
        h(5, 0),
        h(5, 0),
        bs(0),
        h(5, 0),
        n(6, bd3([0.0; 3])),
    ]
    .concat()
}

/// The 2004 header's variables, in order, `extra` zero shorts after them.
fn variables(extra: usize) -> F {
    let mut f = [n(4, bd(0.0)), n(4, tv(b"")), n(2, bl(0))].concat();
    // DIMASO .. LIMCHECK: FILLMODE (5th) and PSLTSCALE (7th) set
    f.extend([0, 0, 0, 0, 0, 1, 0, 1, 0].iter().map(|&x| (x, 1)));
    f.extend(b(false));
    // USRTIMER .. PELLIPSE: MIRRTEXT (5th) set
    f.extend([0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0].iter().map(|&x| (x, 1)));
    // PROXYGRAPHICS .. PDMODE: PDMODE 3
    f.extend([n(7, bs(0)), bs(3)].concat());
    f.extend(n(3, bl(0)));
    f.extend(n(19, bs(0)));
    // LTSCALE 2.5 .. PDSIZE 0.5 (8th) .. CELTSCALE
    f.extend([bd(2.5), n(6, bd(0.0)), bd(0.5), n(13, bd(0.0))].concat());
    f.extend(tv(b""));
    f.extend(n(11, bl(0)));
    f.extend(cmc());
    // the next handle, in the data
    f.extend(h(0, 0x400));
    f.extend(n(5, h(5, 0)));
    f.extend(bd(0.0));
    f.extend(space([0.0; 3], [0.0; 3]));
    f.extend(space([1.0, 2.0, 0.0], [10.0, 20.0, 0.0]));
    f.extend([n(2, tv(b"")), n(9, bd(0.0))].concat());
    f.extend([n(6, b(false)), n(3, bs(0)), n(8, bd(0.0))].concat());
    f.extend([bd(0.0), b(false), bs(0), n(4, b(false)), n(3, cmc())].concat());
    f.extend([n(11, bs(0)), n(2, b(false)), n(4, bs(0)), b(false), bs(0)].concat());
    f.extend(
        [
            n(5, h(5, 0)),
            n(2, bs(0)),
            n(12, h(5, 0)),
            n(2, bs(0)),
            n(2, tv(b"")),
        ]
        .concat(),
    );
    f.extend(n(5, h(5, 0)));
    // the lineweight flags (0: lineweights shown), INSUNITS 4, CEPSNTYPE 0, two GUIDs
    f.extend([bl(0), bs(4), bs(0), n(2, tv(b""))].concat());
    f.extend([n(6, rc(0)), n(2, bs(0)), n(2, rc(0)), tv(b"")].concat());
    f.extend(n(5, h(5, 0)));
    // the last four shorts, and any more
    f.extend(n(4 + extra, bs(0)));
    f
}

/// A header section: the sentinel, the data's size, the data, its CRC-16, the sentinel's
/// complement.
fn section(data: &[u8]) -> Vec<u8> {
    let sentinel: Vec<u8> = (0..16u8).map(|i| i.wrapping_mul(29) ^ 0x3C).collect();
    let mut s = sentinel.clone();
    s.extend_from_slice(&(data.len() as u32).to_le_bytes());
    s.extend_from_slice(data);
    let crc = crc16(CRC16_SEED, &s[16..]);
    s.extend_from_slice(&crc.to_le_bytes());
    s.extend(sentinel.iter().map(|x| !x));
    s
}

#[test]
pub fn header_variables_2004() {
    let got = header(&section(&pack(&variables(0))), Ver::R2004, 0).expect("reads");
    assert_eq!(
        (got.ltscale, got.pdmode, got.pdsize),
        (2.5, 3, 0.5),
        "LTSCALE, PDMODE, PDSIZE"
    );
    assert!(
        got.fillmode && got.psltscale && got.mirrtext,
        "FILLMODE, PSLTSCALE, MIRRTEXT"
    );
    assert!(got.lwdisplay, "lineweights shown");
    assert_eq!(
        (got.insunits, got.handseed),
        (4, 0x400),
        "INSUNITS and the next handle"
    );
    assert_eq!(
        (got.extmin, got.extmax),
        ([1.0, 2.0, 0.0], [10.0, 20.0, 0.0]),
        "model space's extents"
    );
    assert!(
        got.trailing_bits < 8,
        "only padding after the last variable"
    );
    // two shorts more than the variables: the header holds data its read didn't take
    refused(
        header(&section(&pack(&variables(4))), Ver::R2004, 0),
        "DWG-E003",
        "a header longer than its variables",
    );
}

fn width(f: &F) -> usize {
    f.iter().map(|x| x.1 as usize).sum()
}

/// The 2018 header's variables, in order, as three streams (the data, the texts, the handles):
/// most zero; LTSCALE 2.5, PDMODE 3, the next handle, INSUNITS 4. `menu`: MENUNAME among the
/// texts; `extra`: that many more empty texts after the format's.
fn variables_2018(menu: bool, extra: usize) -> (F, F, F) {
    let (mut d, mut s, mut hs): (F, F, F) = (Vec::new(), Vec::new(), Vec::new());
    // REQUIREDVERSIONS (a BLL of no bytes), four doubles, four texts, two longs
    d.push((0, 3));
    d.extend(n(4, bd(0.0)));
    s.extend(n(4, tu("")));
    d.extend(n(2, bl(0)));
    d.extend(n(9 + 1 + 11, b(false)));
    d.extend([n(7, bs(0)), bs(3)].concat());
    d.extend(n(3, bl(0)));
    d.extend(n(19, bs(0)));
    d.extend([bd(2.5), n(20, bd(0.0))].concat());
    if menu {
        s.extend(tu(""));
    }
    d.extend(n(11, bl(0)));
    d.extend(cmc());
    d.extend(h(0, 0x400));
    // CLAYER (layer 0x10), then the other five
    hs.extend([h(5, 0x10), n(5, h(5, 0))].concat());
    d.extend(bd(0.0));
    for _ in 0..2 {
        d.extend([bd3([0.0; 3]), bd3([0.0; 3]), bd3([0.0; 3])].concat());
        d.extend([n(4, rd(0.0)), bd(0.0), n(3, bd3([0.0; 3]))].concat());
        hs.extend(n(2, h(5, 0)));
        d.extend(bs(0));
        hs.extend(h(5, 0));
        d.extend(n(6, bd3([0.0; 3])));
    }
    // DIMPOST, DIMAPOST; the dimension variables
    s.extend(n(2, tu("")));
    d.extend(n(9, bd(0.0)));
    d.extend([bd(0.0), bd(0.0), bs(0), cmc()].concat());
    d.extend([n(6, b(false)), n(4, bs(0)), n(9, bd(0.0))].concat());
    d.extend([b(false), bs(0), n(4, b(false)), n(3, cmc())].concat());
    d.extend(
        [
            n(11, bs(0)),
            n(2, b(false)),
            n(4, bs(0)),
            b(false),
            bs(0),
            b(false),
        ]
        .concat(),
    );
    d.extend([b(false), bd(0.0)].concat());
    s.extend(tu(""));
    d.extend(bd(0.0));
    s.extend(tu(""));
    hs.extend(n(8, h(5, 0)));
    d.extend(n(2, bs(0)));
    hs.extend(n(12, h(5, 0)));
    d.extend(n(2, bs(0)));
    s.extend(n(2, tu("")));
    hs.extend(n(7, h(5, 0)));
    // the lineweight flags, INSUNITS 4, CEPSNTYPE 0; the two GUIDs
    d.extend([bl(0), bs(4), bs(0)].concat());
    s.extend(n(2, tu("")));
    d.extend([n(6, rc(0)), n(2, bs(0)), n(2, rc(0))].concat());
    s.extend(tu(""));
    hs.extend(n(5, h(5, 0)));
    d.extend(
        [
            b(false),
            n(2, bl(0)),
            n(6, bd(0.0)),
            n(2, rc(0)),
            n(6, bd(0.0)),
        ]
        .concat(),
    );
    d.extend(
        [
            bs(0),
            rc(0),
            n(3, bd(0.0)),
            bl(0),
            n(4, rc(0)),
            b(false),
            cmc(),
        ]
        .concat(),
    );
    hs.extend(n(3, h(5, 0)));
    d.extend([rc(0), bd(0.0)].concat());
    s.extend(n(extra, tu("")));
    (d, s, hs)
}

/// A 2018 header section: the sentinel, the data's size, a second size field, the data (its
/// size in bits, the variables, the texts with their size and flag, the handles), its CRC-16,
/// the sentinel's complement.
fn section_2018((d, s, hs): (F, F, F)) -> Vec<u8> {
    let mut body = d;
    body.extend(s.iter().copied());
    body.extend(support::bytes(&(width(&s) as u16).to_le_bytes()));
    body.push((1, 1));
    let mut all = support::bytes(&((32 + width(&body)) as u32).to_le_bytes());
    all.extend(body);
    all.extend(hs);
    let data = pack(&all);
    let sentinel: Vec<u8> = (0..16u8).map(|i| i.wrapping_mul(29) ^ 0x3C).collect();
    let mut out = sentinel.clone();
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&data);
    let crc = crc16(CRC16_SEED, &out[16..]);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend(sentinel.iter().map(|x| !x));
    out
}

/// header_variables_2018: a 2018 header reads to its data's end with every text the format
/// names (MENUNAME among them), none left over; texts past those are tolerated and counted; a
/// text stream short of them is refused.
#[test]
pub fn header_variables_2018() {
    let got = header(&section_2018(variables_2018(true, 0)), Ver::R2018, 0)
        .unwrap_or_else(|e| panic!("header_variables_2018: refused: {}", e.summary()));
    assert_eq!(
        (got.ltscale, got.pdmode, got.insunits, got.handseed),
        (2.5, 3, 4, 0x400),
        "header_variables_2018: LTSCALE, PDMODE, INSUNITS, the next handle"
    );
    assert_eq!(
        got.clayer, 0x10,
        "header_variables_2018: the current layer (CLAYER)"
    );
    assert_eq!(
        got.extra_string_bits, 0,
        "header_variables_2018: every text the format names read, none left"
    );
    let more = header(&section_2018(variables_2018(true, 2)), Ver::R2018, 0).unwrap_or_else(|e| {
        panic!(
            "header_variables_2018: extra texts refused: {}",
            e.summary()
        )
    });
    assert_eq!(
        more.extra_string_bits, 4,
        "header_variables_2018: two empty texts more, tolerated and counted"
    );
    refused(
        header(&section_2018(variables_2018(false, 0)), Ver::R2018, 0),
        "DWG-E002",
        "header_variables_2018: a text stream without MENUNAME",
    );
}
