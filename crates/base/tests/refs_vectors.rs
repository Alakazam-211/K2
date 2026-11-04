//! Shared test vectors for the JS client's reference states (`client/src/refs.ts`): `resolve`,
//! `resolve_all` and `resolve_parents` as `k2_mpc_base::refs` gives them, on random lists. The
//! client must match each one exactly (`client/test/refs.test.ts`).
//!
//! `CLIENT_VECTORS_WRITE=1` writes `client/test/vectors/refs.json`; otherwise the file is
//! checked against what the rules give now, so a stale file fails.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use k2_mpc_base::elem::{KeyedRoot, KeyedState};
use k2_mpc_base::refs::{resolve, resolve_all, resolve_parents, Ref, RefState, SuspectWhy};
use k2_mpc_base::session::View;
use k2_mpc_base::{Key, Value};

/// xorshift64*, so the vectors don't depend on a crate's sequence.
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

const KEYS: [&str; 10] = ["a", "b", "c", "d", "e", "f", "g", "h", "i1.2", "z"];

fn word(s: RefState) -> &'static str {
    match s {
        RefState::Attached => "attached",
        RefState::Orphaned => "orphaned",
        RefState::Suspect(SuspectWhy::KindChanged) => "kind_changed",
        RefState::Suspect(SuspectWhy::ContainerGone) => "container_gone",
        RefState::Suspect(SuspectWhy::Cycle) => "cycle",
    }
}

fn q(s: &str) -> String {
    // keys and values here are plain ASCII
    format!("\"{s}\"")
}

fn boxes(v: &Value) -> bool {
    v.as_str() == Some("box")
}

fn any(_: &Value) -> bool {
    true
}

fn vectors() -> String {
    let mut rng = Rng(0x7265_6673_5f76_3031);
    let mut out = String::from(
        "{\"about\":\"k2-mpc-base refs vectors for the JS client (client/src/refs.ts): per case, \
         the live elements of list items ([key, value]), references (root, key, hint), the kind \
         rule (box: only the value box is attached; any) and parents ({child: parent}); the \
         states of resolve_all and resolve_parents. Written by crates/base/tests/refs_vectors.rs \
         (CLIENT_VECTORS_WRITE=1).\",\"cases\":[\n",
    );
    for case in 0..300 {
        // a random live order of a random subset
        let mut keys: Vec<&str> = KEYS.iter().copied().filter(|_| rng.below(3) != 0).collect();
        for i in (1..keys.len()).rev() {
            keys.swap(i, rng.below(i + 1));
        }
        let live: Vec<(Key, Value)> = keys
            .iter()
            .map(|k| {
                let v = match rng.below(4) {
                    0 => Value::Null,
                    1 => Value::str("line"),
                    _ => Value::str("box"),
                };
                (Key::new(*k).unwrap(), v)
            })
            .collect();
        let view = View::new(KeyedState {
            roots: vec![
                (
                    "items".into(),
                    KeyedRoot::List {
                        live: live.clone(),
                        hidden: BTreeMap::new(),
                        places: Vec::new(),
                    },
                ),
                ("props".into(), KeyedRoot::Map(BTreeMap::new())),
            ],
        });
        let refs: Vec<Ref> = (0..rng.below(8))
            .map(|i| {
                let root = ["items", "items", "items", "props", "gone"][rng.below(5)];
                let r = Ref::new(root, Key::new(KEYS[rng.below(KEYS.len())]).unwrap());
                if rng.below(2) == 0 {
                    r.with_hint(format!("h{i}"))
                } else {
                    r
                }
            })
            .collect();
        let kind_box = rng.below(2) == 0;
        let kind: &dyn Fn(&Value) -> bool = if kind_box { &boxes } else { &any };
        let mut parents: BTreeMap<&str, &str> = BTreeMap::new();
        for k in KEYS {
            if rng.below(2) == 0 {
                parents.insert(k, KEYS[rng.below(KEYS.len())]);
            }
        }
        let states = resolve_all(&view, &refs, kind);
        for (r, s) in &states {
            assert_eq!(*s, resolve(&view, r, kind));
        }
        let ps = resolve_parents(&view, "items", &|k: &Key| {
            parents.get(k.as_str()).map(|p| Key::new(*p).unwrap())
        });
        if case > 0 {
            out.push_str(",\n");
        }
        out.push_str("{\"live\":[");
        for (i, (k, v)) in live.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let v = v.as_str().map_or("null".to_owned(), q);
            let _ = write!(out, "[{},{v}]", q(k.as_str()));
        }
        out.push_str("],\"refs\":[");
        for (i, r) in refs.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let h = r.hint.as_deref().map_or("null".to_owned(), q);
            let _ = write!(
                out,
                "{{\"root\":{},\"key\":{},\"hint\":{h}}}",
                q(&r.root),
                q(r.key.as_str())
            );
        }
        let _ = write!(
            out,
            "],\"kind\":{},\"parents\":{{",
            q(if kind_box { "box" } else { "any" })
        );
        for (i, (c, p)) in parents.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(out, "{}:{}", q(c), q(p));
        }
        out.push_str("},\"states\":[");
        for (i, (_, s)) in states.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&q(word(*s)));
        }
        out.push_str("],\"parent_states\":{");
        for (i, (k, s)) in ps.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(out, "{}:{}", q(k.as_str()), q(word(*s)));
        }
        out.push_str("}}");
    }
    out.push_str("\n]}\n");
    out
}

#[test]
fn refs_vectors_for_the_js_client() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../client/test/vectors/refs.json");
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
        "{} differs from what k2_mpc_base::refs gives (CLIENT_VECTORS_WRITE=1 rewrites it)",
        path.display()
    );
}
