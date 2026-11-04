//! Shared test vectors for the JS client's tagged-key minting (`client/src/mint.ts`): writer
//! tags, key parsing, formatting, the next number and the retired mark, as `k2_mpc_base::mint`
//! gives them. The client must match each one exactly (`client/test/mint.test.ts`).
//!
//! `CLIENT_VECTORS_WRITE=1` writes `client/test/vectors/mint.json`; otherwise the file is
//! checked against what the rules give now, so a stale file fails.

use std::fmt::Write as _;

use k2_mpc_base::doc::{MAX_CLIENT_ID, MAX_KIT_CLIENT};
use k2_mpc_base::mint::{next_for, retired_mark, MintError, TaggedKey, WriterTag};
use k2_mpc_base::{Key, Value};

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
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len() as u64) as usize]
    }
}

fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn err_kind(e: &MintError) -> &'static str {
    match e {
        MintError::BadPrefix(_) => "bad_prefix",
        MintError::TooLong { .. } => "too_long",
        MintError::Exhausted { .. } => "exhausted",
        _ => unreachable!("a new MintError needs its vector kind"),
    }
}

/// Clients: the edges, then random ones over every tag length.
fn clients(rng: &mut Rng) -> Vec<u64> {
    let mut v = vec![
        0,
        1,
        2,
        9,
        10,
        35,
        36,
        1295,
        1296,
        MAX_CLIENT_ID - 1,
        MAX_CLIENT_ID,
        MAX_CLIENT_ID + 1,
    ];
    for bits in 2..=53 {
        v.push(rng.next() & ((1u64 << bits) - 1));
    }
    v
}

/// Keys around the tagged form: canonical, near misses, and the other grammar's forms.
fn keys(rng: &mut Rng) -> Vec<String> {
    let mut v: Vec<String> = [
        "r1",
        "r1.",
        "r01.2",
        "r0.2",
        "r.2",
        "1.2",
        "abcd1.2",
        "r1.1",
        "r1.0",
        "r1.02",
        "r1.2.3",
        "r1x.2",
        "r1_2",
        "r1.2-",
        "r1.2gosa7pa2gv",
        "r1.2gosa7pa2gw",
        "a18446744073709551615.2",
        "a18446744073709551616.2",
        "abc999999999.2gosa7pa2gv",
        "i5.zz",
        "i5.a",
        "z9.10",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let prefixes = ["i", "lnk", "ab", "a", "abcd", "I", "", "x1"];
    let tags = [
        "2",
        "z",
        "10",
        "2gosa7pa2gv",
        "0",
        "1",
        "02",
        "zzzzzzzzzzz",
        "",
        "a.b",
    ];
    let nums = ["1", "7", "0", "007", "999999999", "18446744073709551615"];
    while v.len() < 300 {
        let k = format!(
            "{}{}.{}",
            rng.pick(&prefixes),
            rng.pick(&nums),
            rng.pick(&tags)
        );
        if Key::new(k.as_str()).is_ok() {
            v.push(k);
        }
    }
    v
}

fn vectors() -> String {
    let mut rng = Rng(0x6d69_6e74_5f76_3031);
    let mut out = String::from(
        "{\"about\":\"k2-mpc-base mint vectors for the JS client (client/src/mint.ts): writer \
         tags, key parsing, formatting, next_for and retired_mark. n and marks are decimal \
         strings (u64). Written by crates/base/tests/mint_vectors.rs (CLIENT_VECTORS_WRITE=1).\",\n",
    );
    // tags
    out.push_str("\"tags\":[\n");
    for (i, c) in clients(&mut rng).into_iter().enumerate() {
        if i > 0 {
            out.push_str(",\n");
        }
        let _ = write!(out, "{{\"client\":{c},\"tag\":");
        match WriterTag::of(c) {
            Some(t) => {
                json_str(&mut out, &t.to_string());
                assert_eq!(WriterTag::parse(&t.to_string()), Some(t));
            }
            None => out.push_str("null"),
        }
        out.push('}');
    }
    out.push_str("],\n\"parse\":[\n");
    let ks = keys(&mut rng);
    for (i, k) in ks.iter().enumerate() {
        if i > 0 {
            out.push_str(",\n");
        }
        out.push_str("{\"key\":");
        json_str(&mut out, k);
        out.push_str(",\"tagged\":");
        let key = Key::new(k.as_str()).unwrap();
        match TaggedKey::parse(&key) {
            Some(t) => {
                out.push_str("{\"prefix\":");
                json_str(&mut out, t.prefix());
                let _ = write!(out, ",\"n\":\"{}\",\"tag\":", t.n());
                json_str(&mut out, &t.tag().to_string());
                out.push('}');
            }
            None => out.push_str("null"),
        }
        out.push('}');
    }
    // format: prefix, n, tag
    out.push_str("],\n\"format\":[\n");
    let tags: Vec<WriterTag> = [2, 36, 99_999, MAX_KIT_CLIENT]
        .into_iter()
        .map(|c| WriterTag::of(c).unwrap())
        .collect();
    let mut first = true;
    for p in ["i", "abc", "", "abcd", "A"] {
        for n in [0u64, 1, 999_999_999, 1_000_000_000, u64::MAX] {
            for &t in &tags {
                if !first {
                    out.push_str(",\n");
                }
                first = false;
                out.push_str("{\"prefix\":");
                json_str(&mut out, p);
                let _ = write!(out, ",\"n\":\"{n}\",\"tag\":");
                json_str(&mut out, &t.to_string());
                out.push(',');
                match TaggedKey::new(p, n, t).and_then(|k| k.to_key()) {
                    Ok(k) => {
                        out.push_str("\"key\":");
                        json_str(&mut out, k.as_str());
                    }
                    Err(e) => {
                        let _ = write!(out, "\"error\":\"{}\"", err_kind(&e));
                    }
                }
                out.push('}');
            }
        }
    }
    // next_for: random places drawn from the parse keys, a retired mark, a prefix and tag
    out.push_str("],\n\"next\":[\n");
    let valid: Vec<Key> = ks.iter().map(|k| Key::new(k.as_str()).unwrap()).collect();
    for i in 0..200 {
        if i > 0 {
            out.push_str(",\n");
        }
        let places: Vec<&Key> = (0..rng.below(8))
            .map(|_| &valid[rng.below(valid.len() as u64) as usize])
            .collect();
        let retired = match rng.below(4) {
            0 => 0,
            1 => rng.below(10),
            2 => 999_999_999,
            _ => u64::MAX - rng.below(2),
        };
        let prefix = rng.pick(&["i", "lnk", "ab", "a", "z", "abcd"]);
        let tag = tags[rng.below(tags.len() as u64) as usize];
        out.push_str("{\"places\":[");
        for (j, k) in places.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            json_str(&mut out, k.as_str());
        }
        let _ = write!(out, "],\"retired\":\"{retired}\",\"prefix\":");
        json_str(&mut out, prefix);
        out.push_str(",\"tag\":");
        json_str(&mut out, &tag.to_string());
        out.push(',');
        match next_for(places, retired, prefix, tag) {
            Ok(n) => {
                let _ = write!(out, "\"n\":\"{n}\"");
            }
            Err(e) => {
                let _ = write!(out, "\"error\":\"{}\"", err_kind(&e));
            }
        }
        out.push('}');
    }
    // retired marks: random mark maps
    out.push_str("],\n\"marks\":[\n");
    let roots = ["items", "links", "itemsx"];
    let vals = [
        "4",
        "9",
        "0",
        "+5",
        " 5",
        "x",
        "",
        "18446744073709551615",
        "18446744073709551616",
    ];
    for i in 0..120 {
        if i > 0 {
            out.push_str(",\n");
        }
        let tag = tags[rng.below(tags.len() as u64) as usize];
        let mut marks: Vec<(String, Value)> = Vec::new();
        for _ in 0..rng.below(6) {
            let t = tags[rng.below(tags.len() as u64) as usize];
            let d = tags[rng.below(tags.len() as u64) as usize];
            let root = rng.pick(&roots);
            let p = rng.pick(&["i", "ab"]);
            let k = match rng.below(3) {
                0 => format!("mint/{root}/{p}.{t}"),
                1 => format!("mint/{root}/{p}.{t}/{d}"),
                _ => format!("mint/{root}/{p}.{t}0/{d}"),
            };
            let v = if rng.below(8) == 0 {
                Value::Null
            } else {
                Value::str(rng.pick(&vals))
            };
            marks.push((k, v));
        }
        out.push_str("{\"marks\":[");
        for (j, (k, v)) in marks.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            out.push('[');
            json_str(&mut out, k);
            out.push(',');
            match v.as_str() {
                Some(s) => json_str(&mut out, s),
                None => out.push_str("null"),
            }
            out.push(']');
        }
        let mark = retired_mark(
            marks.iter().map(|(k, v)| (k.as_str(), v)),
            "items",
            "i",
            tag,
        );
        out.push_str("],\"root\":\"items\",\"prefix\":\"i\",\"tag\":");
        json_str(&mut out, &tag.to_string());
        let _ = write!(out, ",\"mark\":\"{mark}\"}}");
    }
    out.push_str("\n]}\n");
    out
}

#[test]
fn mint_vectors_for_the_js_client() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../client/test/vectors/mint.json");
    let got = vectors();
    if std::env::var_os("CLIENT_VECTORS_WRITE").is_some() {
        std::fs::write(&path, &got).unwrap();
        println!("wrote {}", path.display());
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "{} is missing (CLIENT_VECTORS_WRITE=1 writes it)",
            path.display()
        )
    });
    assert!(
        want == got,
        "{} differs from what k2_mpc_base::mint gives (CLIENT_VECTORS_WRITE=1 rewrites it)",
        path.display()
    );
}
