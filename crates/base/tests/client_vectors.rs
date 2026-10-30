//! Shared test vectors for the JS client (`client/`, K1): how [`position::transform`] moves
//! every character boundary of a text through a hunk set, with both biases. The client's
//! `position.ts` must reproduce each one exactly (`client/test/vectors.test.ts`).
//!
//! `CLIENT_VECTORS_WRITE=1` writes `client/test/vectors/transform.json`; otherwise the file
//! is checked against what the rules give now, so a stale file fails.

use k2_mpc_base::session::position::{transform, Bias};
use k2_mpc_base::text::diff::{self, Hunk};
use std::fmt::Write as _;

/// A small deterministic generator (xorshift64*), so the vectors don't depend on a crate's
/// sequence.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const BITS: [&str; 8] = ["a", "b", " ", "\n", "é", "日", "🎉", "xy"];

fn text(rng: &mut Rng, max: usize) -> String {
    (0..rng.below(max + 1))
        .map(|_| BITS[rng.below(BITS.len())])
        .collect()
}

fn boundaries(t: &str) -> Vec<usize> {
    t.char_indices().map(|(i, _)| i).chain([t.len()]).collect()
}

/// Random hunks on `t`, valid for [`diff::validate`]: sorted, non-overlapping, on char
/// boundaries; insertions, deletions, replacements, and an insertion followed by a deletion
/// or replacement starting at the same point (which canonical diffs never produce but
/// `transform` accepts).
fn hunks(rng: &mut Rng, t: &str) -> Vec<Hunk> {
    if rng.below(3) == 0 {
        return diff::hunks(t, &text(rng, 12));
    }
    let b = boundaries(t);
    let mut cuts: Vec<usize> = (0..2 * (1 + rng.below(3)))
        .map(|_| b[rng.below(b.len())])
        .collect();
    cuts.sort_unstable();
    let mut out: Vec<Hunk> = Vec::new();
    for c in cuts.chunks(2) {
        let (s, mut e) = (c[0], c[1]);
        if out.last().is_some_and(|l| s < l.end) {
            continue;
        }
        if rng.below(3) == 0 {
            e = s;
        }
        let ins = text(rng, 2);
        if s == e && ins.is_empty() {
            continue;
        }
        // two insertions at one point aren't valid
        if s == e && out.last().is_some_and(|l| l.is_insertion() && l.start == s) {
            continue;
        }
        if s < e && rng.below(4) == 0 && out.last().map_or(true, |l| l.end < s || !l.is_insertion())
        {
            // an insertion, then a deletion or replacement from the same point
            out.push(Hunk {
                start: s,
                end: s,
                insert: if ins.is_empty() {
                    "+".into()
                } else {
                    ins.clone()
                },
            });
            out.push(Hunk {
                start: s,
                end: e,
                insert: String::new(),
            });
            continue;
        }
        out.push(Hunk {
            start: s,
            end: e,
            insert: ins,
        });
    }
    out
}

fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn list(out: &mut String, v: &[usize]) {
    out.push('[');
    for (i, n) in v.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "{n}");
    }
    out.push(']');
}

fn vectors() -> String {
    let mut rng = Rng(0x6b32_6d70_635f_6b31);
    let mut out = String::from(
        "{\"about\":\"k2-mpc-base position::transform vectors for the JS client: for each case, \
         every char boundary of base (positions, UTF-8 bytes) through hunks ([start, end, insert], \
         UTF-8 bytes) with Bias::Before and Bias::After. Written by crates/base/tests/client_vectors.rs \
         (CLIENT_VECTORS_WRITE=1).\",\"cases\":[\n",
    );
    let mut n = 0;
    while n < 600 {
        let base = text(&mut rng, 10);
        let hs = hunks(&mut rng, &base);
        diff::validate(&base, &hs).expect("generated hunks are valid");
        let positions = boundaries(&base);
        let before: Vec<usize> = positions
            .iter()
            .map(|&p| transform(p, &hs, Bias::Before))
            .collect();
        let after: Vec<usize> = positions
            .iter()
            .map(|&p| transform(p, &hs, Bias::After))
            .collect();
        if n > 0 {
            out.push_str(",\n");
        }
        out.push_str("{\"base\":");
        json_str(&mut out, &base);
        out.push_str(",\"hunks\":[");
        for (i, h) in hs.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(out, "[{},{},", h.start, h.end);
            json_str(&mut out, &h.insert);
            out.push(']');
        }
        out.push_str("],\"positions\":");
        list(&mut out, &positions);
        out.push_str(",\"before\":");
        list(&mut out, &before);
        out.push_str(",\"after\":");
        list(&mut out, &after);
        out.push('}');
        n += 1;
    }
    out.push_str("\n]}\n");
    out
}

#[test]
fn transform_vectors_for_the_js_client() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../client/test/vectors/transform.json");
    let now = vectors();
    if std::env::var("CLIENT_VECTORS_WRITE").is_ok() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &now).unwrap();
        return;
    }
    let on_disk = std::fs::read_to_string(&path)
        .expect("client/test/vectors/transform.json: write it with CLIENT_VECTORS_WRITE=1");
    assert!(
        on_disk == now,
        "the client's transform vectors are stale: regenerate with CLIENT_VECTORS_WRITE=1"
    );
}
