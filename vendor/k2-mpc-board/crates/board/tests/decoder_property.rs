//! A host opens files other people send. Random damage to a valid file, or to its JSON, must give a
//! typed refusal or a valid board. The seeds are fixed, so a run is the same every time, and no
//! damaged bytes are kept in this repo: anything found becomes a fix and a hand-written regression.

use k2_mpc_board::json;
use k2_mpc_board::*;
use std::io::Cursor;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const PERSON: &str = "p-0123456789abcdef";

fn board_text(blob: &Sha256) -> String {
    format!(
        "{{\"frames\":[\"f-1\"],\"objects\":[\
{{\"created\":{{\"at\":\"2026-11-03T10:00:00Z\",\"by\":\"{PERSON}\"}},\"id\":\"f-1\",\"title\":\"One\",\"type\":\"frame\"}},\
{{\"color\":\"#aabbcc\",\"id\":\"s-1\",\"link\":\"https://example.com\",\"reactions\":{{\"x\":[\"{PERSON}\"]}},\"tags\":[\"a\"],\"text\":\"caf\u{e9} \u{1F600} \\n\",\"type\":\"sticky\",\"votes\":{{\"{PERSON}\":1}},\"x\":0.1,\"y\":-2.5e3}},\
{{\"from\":{{\"hint\":{{\"x\":1,\"y\":2}},\"id\":\"s-1\"}},\"id\":\"c-1\",\"to\":{{\"x\":1,\"y\":2}},\"type\":\"connector\"}},\
{{\"id\":\"p-1\",\"points\":[[0,0],[1,2],[3,4]],\"type\":\"pen\"}},\
{{\"columns\":[{{\"id\":\"a\"}}],\"id\":\"b-1\",\"rows\":[{{\"cells\":{{\"a\":\"x\"}},\"id\":\"r\"}}],\"type\":\"table\"}},\
{{\"checks\":[{{\"done\":true,\"id\":\"c\",\"text\":\"t\"}}],\"id\":\"k-1\",\"type\":\"card\"}},\
{{\"id\":\"q-1\",\"replies\":[{{\"at\":\"2026-11-03T10:00:00.5+02:00\",\"by\":\"{PERSON}\",\"id\":\"r\",\"text\":\"t\"}}],\"target\":\"s-1\",\"type\":\"comment\"}},\
{{\"crop\":{{\"h\":1,\"w\":1,\"x\":0,\"y\":0}},\"id\":\"i-1\",\"src\":{{\"blob\":\"{blob}\"}},\"type\":\"image\"}},\
{{\"id\":\"m-1\",\"parent\":\"m-1\",\"type\":\"mind\"}},\
{{\"id\":\"n-1\",\"later\":[1,null,{{\"a\":[true,false]}}],\"type\":\"future\"}}\
],\"people\":{{\"{PERSON}\":{{\"name\":\"Ann\"}}}},\"settings\":{{\"title\":\"T\",\"voting\":{{\"active\":true}}}}}}"
    )
}

fn sample() -> (String, Vec<u8>) {
    let mut parts = MemParts::new();
    let blob = parts.add_blob("image/png", vec![7u8; 300]);
    parts.set_preview(vec![9u8; 40]);
    parts.add(
        "extra/x.bin",
        "future",
        "application/x-y",
        vec![1u8; 20],
        Fields::new(),
    );
    let text = board_text(&blob);
    let board = Board::from_json(text.as_bytes(), &Limits::V1).expect("the sample board");
    let mut out = Vec::new();
    write(
        &mut out,
        &WriteMeta::new(Lineage::from_bytes([3; 16])),
        &board,
        &mut parts,
        &Limits::V1,
    )
    .expect("the sample file");
    (text, out)
}

/// Either refused with a known code, or valid all the way through: readable, rewritable, reopenable.
fn file_holds_up_inner(bytes: Vec<u8>) {
    let known = |r: &Refusal| RefusalCode::ALL.contains(&r.code);
    let _ = package_guard_of(Cursor::new(bytes.clone()), &Limits::V1);
    match Package::open(Cursor::new(bytes), &Limits::V1) {
        Err(r) => assert!(known(&r), "an unknown code: {r}"),
        Ok(mut pkg) => {
            pkg.board()
                .validate(&Limits::V1)
                .expect("an opened board is valid");
            let _ = pkg.board().references();
            let _ = pkg.verify_parts();
            let board = pkg.board().clone();
            let meta = pkg.manifest().write_meta();
            let mut out = Vec::new();
            if write(&mut out, &meta, &board, &mut pkg, &Limits::V1).is_ok() {
                let again =
                    Package::open(Cursor::new(out), &Limits::V1).expect("our own output opens");
                assert_eq!(again.board(), &board, "the board changed on rewrite");
            }
        }
    }
}

/// The same, and the decoder must not stop without an answer: that fails here, at this assert.
fn file_holds_up(bytes: Vec<u8>) {
    let result = std::panic::catch_unwind(|| file_holds_up_inner(bytes));
    assert!(result.is_ok(), "the decoder stopped without a refusal");
}

fn json_holds_up(bytes: &[u8]) {
    match Board::from_json(bytes, &Limits::V1) {
        Err(r) => assert!(RefusalCode::ALL.contains(&r.code), "an unknown code: {r}"),
        Ok(board) => {
            let text = board.to_canonical_json().expect("a valid board prints");
            let again =
                Board::from_json(&text, &Limits::V1).expect("its canonical form reads back");
            assert_eq!(again, board, "the canonical form changed the board");
            assert_eq!(
                again.to_canonical_json().expect("prints"),
                text,
                "canonical isn't stable"
            );
            let _ = board.references();
            let _ = board.blob_refs();
        }
    }
    if let Ok(v) = json::parse(bytes, &Limits::V1) {
        let text = json::to_canonical(&v).expect("a parsed value prints");
        assert_eq!(json::parse(&text, &Limits::V1).expect("reads back"), v);
    }
}

const FLIPS: usize = 4000;

#[test]
fn byte_flips_in_a_file_are_refused_or_valid() {
    let (_, file) = sample();
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    for _ in 0..FLIPS {
        let mut f = file.clone();
        for _ in 0..=rng.below(3) {
            let at = rng.below(f.len());
            f[at] ^= 1 << rng.below(8);
        }
        file_holds_up(f);
    }
}

#[test]
fn random_bytes_in_a_file_are_refused_or_valid() {
    let (_, file) = sample();
    let mut rng = Rng(0x2BAD_F00D_1234_5677);
    for _ in 0..FLIPS {
        let mut f = file.clone();
        let at = rng.below(f.len());
        f[at] = rng.below(256) as u8;
        file_holds_up(f);
    }
}

#[test]
fn flips_in_headers_only_are_refused_or_valid() {
    // Most bytes of a file are bodies. Aim at the central directory and end record too.
    let (_, file) = sample();
    let central = file
        .windows(4)
        .position(|w| w == [0x50, 0x4b, 0x01, 0x02])
        .expect("a central directory");
    let mut rng = Rng(0x0F0F_1234_ABCD_5555);
    for _ in 0..FLIPS {
        let mut f = file.clone();
        let at = central + rng.below(f.len() - central);
        f[at] = match rng.below(4) {
            0 => 0xFF,
            1 => 0,
            _ => rng.below(256) as u8,
        };
        file_holds_up(f);
    }
}

#[test]
fn counts_that_lie_in_both_fields_are_refused_or_valid() {
    // One flip can't change both of the end record's counts, and they must agree to get further.
    let (_, file) = sample();
    let end = file.len() - 22;
    for count in 0..=40u16 {
        let mut f = file.clone();
        f[end + 8..end + 10].copy_from_slice(&count.to_le_bytes());
        f[end + 10..end + 12].copy_from_slice(&count.to_le_bytes());
        file_holds_up(f);
    }
}

#[test]
fn truncations_of_a_file_are_refused() {
    let (_, file) = sample();
    for len in 0..file.len() {
        let r = Package::open(Cursor::new(file[..len].to_vec()), &Limits::V1);
        assert!(r.is_err(), "a file cut to {len} bytes opened");
    }
}

#[test]
fn byte_flips_in_the_json_are_refused_or_valid() {
    let (text, _) = sample();
    let bytes = text.as_bytes();
    let mut rng = Rng(0x5555_AAAA_1234_0001);
    for _ in 0..(FLIPS * 5) {
        let mut b = bytes.to_vec();
        for _ in 0..=rng.below(2) {
            let at = rng.below(b.len());
            b[at] ^= 1 << rng.below(8);
        }
        json_holds_up(&b);
    }
}

#[test]
fn truncations_of_the_json_are_refused() {
    let (text, _) = sample();
    let bytes = text.as_bytes();
    for len in 0..bytes.len() {
        assert!(
            Board::from_json(&bytes[..len], &Limits::V1).is_err(),
            "a board cut to {len} bytes was accepted"
        );
    }
}

#[test]
fn nesting_past_the_limit_is_refused_not_followed() {
    for open in [&b"["[..], b"{\"a\":"] {
        let deep: Vec<u8> = open
            .iter()
            .copied()
            .cycle()
            .take(open.len() * 5_000_000)
            .collect();
        let r = json::parse(&deep, &Limits::V1).expect_err("refused");
        assert_eq!(r.code, RefusalCode::LimitDepth);
    }
}
