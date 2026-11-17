//! The container's decompression over hand-made streams: literal runs (short and extended),
//! short, medium and long matches (overlapping their own output, with trailing literals,
//! with zero-extended lengths), and the streams that must be refused.

mod support;

use k2_mpc_dwg::lz77::decompress;
use k2_mpc_dwg::Limits;
use support::{literal, ok, refused};

fn run(parts: &[&[u8]], size: usize) -> Result<Vec<u8>, k2_mpc_dwg::CadError> {
    decompress(&parts.concat(), size, &Limits::default())
}

/// 0x4010 bytes that don't repeat with a short period: a long match's source.
fn far_data() -> Vec<u8> {
    (0..0x4010usize)
        .map(|i| (i.wrapping_mul(7) + (i >> 8)) as u8)
        .collect()
}

#[test]
pub fn matches_and_literals() {
    ok(run(&[b"\x11"], 0), Vec::new(), "the end mark alone");
    ok(
        run(&[&literal(b"HELLO"), b"\x11"], 5),
        b"HELLO".to_vec(),
        "a literal run",
    );
    ok(
        run(&[&literal(b"ABCD"), &[0x9C, 0x00, 0x11]], 12),
        b"ABCDABCDABCD".to_vec(),
        "a short match, 8 bytes from 4 back, overlapping what it writes",
    );
    ok(
        run(&[&literal(b"ABCD"), &[0x5E, 0x00, b'X', b'Y', 0x11]], 10),
        b"ABCDABCDXY".to_vec(),
        "a short match with two trailing literals",
    );
    ok(
        run(
            &[&literal(b"ABCD"), &[0x9C, 0x00], &literal(b"WXYZ"), b"\x11"],
            16,
        ),
        b"ABCDABCDABCDWXYZ".to_vec(),
        "a literal run after a match with no trailing literals",
    );
    ok(
        run(&[&literal(b"ABCDEFGH"), &[0x2E, 0x1C, 0x00, 0x11]], 24),
        b"ABCDEFGH".repeat(3),
        "a medium match, 16 bytes from 8 back",
    );
    let abcd = |n: usize| b"ABCD".iter().copied().cycle().take(n).collect::<Vec<u8>>();
    ok(
        run(&[&literal(b"ABCD"), &[0x20, 0x05, 0x0C, 0x00, 0x11]], 42),
        abcd(42),
        "a medium match with an extended length: 5 + 0x21",
    );
    ok(
        run(
            &[&literal(b"ABCD"), &[0x20, 0x00, 0x02, 0x0C, 0x00, 0x11]],
            294,
        ),
        abcd(294),
        "an extended length with a zero byte: 0xFF + 2 + 0x21",
    );
    let data = far_data();
    let mut want = data.clone();
    want.extend_from_slice(&data[16..20]);
    ok(
        run(&[&literal(&data), &[0x12, 0x00, 0x00, 0x11]], want.len()),
        want,
        "a long match, 4 bytes from 0x4000 back, after an extended literal run",
    );
    let mut want = data.clone();
    want.extend_from_slice(&data[15..27]);
    ok(
        run(
            &[&literal(&data), &[0x10, 0x03, 0x04, 0x00, 0x11]],
            want.len(),
        ),
        want,
        "a long match with an extended length, 12 bytes from 0x4001 back",
    );
}

#[test]
pub fn streams_refused() {
    refused(
        run(&[&literal(b"ABCD"), &[0x9C, 0x01, 0x11]], 64),
        "DWG-E004",
        "a match 8 bytes back with 4 written",
    );
    refused(
        run(&[&literal(b"ABCD"), &[0x01, 0x41, 0x11]], 64),
        "DWG-E004",
        "a literal run right after a literal run",
    );
    refused(
        run(&[&literal(b"HELLO"), b"\x11"], 4),
        "DWG-E005",
        "a literal run past the declared size",
    );
    refused(
        run(&[&literal(b"ABCD"), &[0x9C, 0x00, 0x11]], 8),
        "DWG-E005",
        "a match past the declared size",
    );
    refused(run(&[&literal(b"HELLO")], 64), "DWG-E002", "no end mark");
    refused(
        run(&[&[0x02, b'H']], 64),
        "DWG-E002",
        "a literal run past the stream's end",
    );
    refused(
        run(&[&[0x9C]], 64),
        "DWG-E002",
        "a short match without its second byte",
    );
    let small = Limits {
        max_page: 16,
        ..Limits::default()
    };
    refused(
        decompress(b"\x11", 17, &small),
        "DWG-E005",
        "a declared size over the page limit",
    );
}
