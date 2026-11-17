//! The header variables over a hand-made 2004 section: every variable in its order, most of
//! them zero, a few set (LTSCALE, PDMODE, PDSIZE, FILLMODE, PSLTSCALE, MIRRTEXT, the next
//! handle, model space's extents, INSUNITS), read back; and a header longer than its variables
//! refused.

mod support;

use k2_mpc_dwg::crc::{crc16, CRC16_SEED};
use k2_mpc_dwg::frame::Ver;
use k2_mpc_dwg::header::header;
use support::object::{b, bd, bd3, bl, bs, h, rc, rd, tv, F};
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
