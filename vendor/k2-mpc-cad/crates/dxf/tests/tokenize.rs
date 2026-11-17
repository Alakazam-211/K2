//! The tokenizer: writing the tokens back gives the input's bytes over every byte variant of our
//! drawings; what isn't code/value pairs, binary DXF and lines over the limit are refused, typed,
//! with the byte offset.

use k2_mpc_dxf::error::{BINARY, NOT_PAIRS, OVER_BOUND};
use k2_mpc_dxf::testgen::{self, Eol, Ver};
use k2_mpc_dxf::token::tokenize;
use k2_mpc_dxf::Limits;

#[test]
pub fn tokenize_reemit_identical() {
    let variants = testgen::byte_variants();
    let mut pairs = 0usize;
    for (name, bytes) in &variants {
        let t = tokenize(bytes, &Limits::default()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            t.pairs.len() > 50,
            "{name}: only {} pairs read",
            t.pairs.len()
        );
        pairs += t.pairs.len();
        let back = t.reemit();
        if back != *bytes {
            let at = back
                .iter()
                .zip(bytes.iter())
                .position(|(a, b)| a != b)
                .unwrap_or(back.len().min(bytes.len()));
            panic!(
                "{name}: written back differs at byte {at} ({} bytes back, {} in)",
                back.len(),
                bytes.len()
            );
        }
    }
    println!(
        "tokenize_reemit_identical: {} variants, {pairs} pairs written back byte for byte",
        variants.len()
    );
}

#[test]
pub fn line_endings_and_padding_are_measured() {
    for (eol, want) in [(Eol::Lf, "lf"), (Eol::CrLf, "crlf"), (Eol::Mixed, "mixed")] {
        let mut s = testgen::sampler(Ver::R2000);
        s.opts.eol = eol;
        let b = s.build().bytes;
        let t = tokenize(&b, &Limits::default()).unwrap();
        assert_eq!(t.eol(), want, "{eol:?}");
    }
    let mut s = testgen::sampler(Ver::R2000);
    s.opts.pad = true;
    let b = s.build().bytes;
    let t = tokenize(&b, &Limits::default()).unwrap();
    let (padded, plain) = t.padding();
    assert!(padded > 0 && plain == 0, "padded {padded}, plain {plain}");
}

#[test]
pub fn bom_and_tail_are_carried() {
    let mut s = testgen::sampler(Ver::R2013);
    s.opts.bom = true;
    s.opts.tail = b"\x1aafter the end".to_vec();
    let b = s.build().bytes;
    let t = tokenize(&b, &Limits::default()).unwrap();
    assert!(t.bom, "the byte-order mark was not seen");
    assert!(t.eof, "the end marker was not seen");
    assert_eq!(
        t.tail_at,
        b.len() - 14,
        "the tail starts after the end marker's line"
    );
    assert_eq!(t.reemit(), b);
}

#[test]
pub fn not_pairs_refused() {
    let good = testgen::sampler(Ver::R2000).build().bytes;
    // an odd line count: drop the file's last line
    let cut = good[..good.len() - 1]
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|i| i + 1)
        .unwrap();
    let odd = &good[..cut];
    let e = tokenize(odd, &Limits::default()).unwrap_err();
    assert_eq!(e.code, NOT_PAIRS, "{e}");
    assert_eq!(e.code.id, "DXF-E002");
    let last_line = odd[..odd.len() - 1]
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|i| i + 1)
        .unwrap();
    assert_eq!(e.offset, Some(last_line as u64), "{e}");

    // a code that isn't an integer
    let mut bad = b"0\nSECTION\n2\nHEADER\nX9\n$ACADVER\n".to_vec();
    bad.extend_from_slice(b"1\nAC1015\n0\nENDSEC\n0\nEOF\n");
    let e = tokenize(&bad, &Limits::default()).unwrap_err();
    assert_eq!(e.code, NOT_PAIRS, "{e}");
    assert_eq!(e.offset, Some(19), "{e}");

    // binary DXF
    let mut bin = b"AutoCAD Binary DXF\r\n\x1a\x00".to_vec();
    bin.extend_from_slice(&[0u8; 16]);
    let e = tokenize(&bin, &Limits::default()).unwrap_err();
    assert_eq!(e.code, BINARY, "{e}");
    assert_eq!(e.code.id, "DXF-E001");
}

#[test]
pub fn over_bound_refused() {
    let mut s = testgen::sampler(Ver::R2018);
    s.model
        .push(testgen::Ent::text([0.0, 0.0], 1.0, &"x".repeat(5000)));
    let b = s.build().bytes;
    let tight = Limits {
        max_line: 4096,
        ..Limits::default()
    };
    let e = tokenize(&b, &tight).unwrap_err();
    assert_eq!(e.code, OVER_BOUND, "{e}");
    assert_eq!(e.code.id, "DXF-E003");
    assert!(e.why.contains("line"), "{e}");
}

#[test]
pub fn a_file_without_an_end_marker_reads() {
    let mut s = testgen::sampler(Ver::R2000);
    s.opts.eof = false;
    let b = s.build().bytes;
    let t = tokenize(&b, &Limits::default()).unwrap();
    assert!(!t.eof);
    assert_eq!(t.tail_at, b.len());
    assert_eq!(t.reemit(), b);
}
