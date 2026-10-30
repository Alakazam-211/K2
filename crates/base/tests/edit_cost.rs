//! What an agent edit costs as a doc's history grows (v0.9.2, the soak's throughput finding;
//! v0.9.3, the new path).
//!
//! Until v0.9.3 an agent edit was made on a replica built from the doc's full engine state
//! and dropped (the old ephemeral-replica rule: `tests/ephemeral_replicas.rs`); since, on the
//! doc itself as the client ([`Path::MainDoc`]; ARCHITECTURE rule 6). The engine's state keeps an
//! item for every insertion ever made, deleted ones included (GC keeps their IDs and lengths,
//! not their content), so under churn (lines added and removed, the text staying the same
//! size) the state, and with it each edit's replica, grows with the number of changes.
//!
//! `edit_cost_grows_with_the_engine_state` (ignored; a few seconds in release) prints the
//! time per edit against the changes made and the encoded state's size, through the public
//! `Doc` API only (no session, no store, no fsync):
//!
//! ```sh
//! cargo test -p k2-mpc-base --release --test edit_cost -- --ignored --nocapture
//! ```
//!
//! `edit_cost_at_sizes` (v0.9.3; ignored, minutes in release: run it on a node) measures an
//! edit at **1.2k, 40k and ~300k changes** of history (`EDIT_COST_SIZES`, comma-separated),
//! through each edit path ([`Path`]). Building 300k changes through today's path would take
//! hours (that is the finding), so the history is built by the same churn on plain engine
//! docs, one per client, each kept in sync by applying every update as it is made (the
//! engine's own bytes, never hand-made), and the doc is loaded from the result. At the first
//! size the doc so built is checked **byte-identical** to one built through the kit, so the
//! two histories are the same. Then, on the production path, the canary's cost: each of
//! 5,000 edits timed alone with the runtime check off and at its default rate (one in 1,000),
//! from the same state: p50, p99, p99.9, max and mean.

use std::time::Instant;

use std::collections::HashMap;

use k2_mpc_base::differential::{with_side, Path, Side};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::{Doc, Edit, Kind, Schema};
use yrs::updates::decoder::Decode;
use yrs::{OffsetKind, Options, ReadTxn, StateVector, Text, TextRef, Transact, Update};

/// The edit paths measured: the throwaway replica (every edit's path before v0.9.3) and
/// v0.9.3's (on the doc itself, as the client).
const PATHS: &[Path] = &[Path::Throwaway, Path::MainDoc];

const S: Schema = Schema {
    name: "test",
    roots: &[(ROOT, Kind::Text)],
};
const ROOT: &str = "source";

/// One churn step by `client`: a new line in, the oldest line out (two edits).
fn churn(doc: &mut Doc, client: u64, n: u64) {
    let t = doc.text(ROOT).unwrap();
    let mid = t[..t.len() / 2].rfind('\n').map_or(0, |k| k + 1);
    let ins = Edit::Text {
        root: ROOT.into(),
        hunks: vec![Hunk {
            start: mid,
            end: mid,
            insert: format!("line {n} by {client}\n"),
        }],
    };
    doc.edit_as(client, &[ins]).unwrap();
    let t = doc.text(ROOT).unwrap();
    let first = t.find('\n').unwrap() + 1;
    let del = Edit::Text {
        root: ROOT.into(),
        hunks: vec![Hunk {
            start: 0,
            end: first,
            insert: String::new(),
        }],
    };
    doc.edit_as(client, &[del]).unwrap();
}

#[test]
#[ignore = "a measurement: run in release with --nocapture"]
fn edit_cost_grows_with_the_engine_state() {
    let mut doc = Doc::new(S).unwrap();
    let seed: String = (0..300).map(|i| format!("seed line {i}\n")).collect();
    doc.replace_text_as(2, ROOT, &seed).unwrap();
    let mut changes = 0u64;
    let mut rows = Vec::new();
    for checkpoint in [1_000u64, 5_000, 10_000, 20_000, 40_000] {
        while changes < checkpoint {
            churn(&mut doc, 100 + changes % 32, changes);
            changes += 2;
        }
        // time 100 churn steps (200 edits) at this history
        let t0 = Instant::now();
        for k in 0..100 {
            churn(&mut doc, 100 + (changes + k) % 32, changes + k);
        }
        changes += 200;
        let per_edit_us = t0.elapsed().as_secs_f64() * 1e6 / 200.0;
        let state = doc.encode_state().unwrap().len();
        let text = doc.text(ROOT).unwrap().len();
        eprintln!(
            "[edit cost] {changes} changes: {per_edit_us:.0} µs per edit, engine state {state} B, text {text} B"
        );
        rows.push((changes, per_edit_us, state));
    }
    // the cost follows the engine state, not the text (which stays ~300 lines)
    let (first, last) = (rows[0], rows[rows.len() - 1]);
    assert!(
        last.2 > first.2 * 4,
        "the state grows with history: {rows:?}"
    );
}

/// The history builder: one plain engine doc per client (and the doc's own, client 1), all
/// kept identical by applying every update to every one as it is made, plus the text.
struct Builder {
    docs: HashMap<u64, (yrs::Doc, TextRef)>,
    text: String,
    changes: u64,
}

fn engine_doc(client: u64) -> (yrs::Doc, TextRef) {
    let d = yrs::Doc::with_options(Options {
        client_id: yrs::block::ClientID::new(client),
        offset_kind: OffsetKind::Utf16,
        skip_gc: false,
        ..Options::default()
    });
    let t = d.get_or_insert_text(ROOT);
    (d, t)
}

impl Builder {
    fn new(clients: impl IntoIterator<Item = u64>) -> Self {
        let mut docs: HashMap<u64, (yrs::Doc, TextRef)> = HashMap::new();
        for c in std::iter::once(1).chain(clients) {
            docs.insert(c, engine_doc(c));
        }
        Self {
            docs,
            text: String::new(),
            changes: 0,
        }
    }

    /// One change by `client`: delete `del` bytes at byte `at`, insert `ins` there (the
    /// text is ASCII here, so bytes are UTF-16 units), applied everywhere.
    fn edit(&mut self, client: u64, at: usize, del: usize, ins: &str) {
        let (d, t) = &self.docs[&client];
        let u = {
            let mut txn = d.transact_mut();
            if del > 0 {
                t.remove_range(&mut txn, at as u32, del as u32);
            }
            if !ins.is_empty() {
                t.insert(&mut txn, at as u32, ins);
            }
            txn.encode_update_v1()
        };
        for (c, (other, _)) in &self.docs {
            if *c != client {
                other
                    .transact_mut()
                    .apply_update(Update::decode_v1(&u).unwrap())
                    .unwrap();
            }
        }
        self.text.replace_range(at..at + del, ins);
        self.changes += 1;
    }

    /// [`churn`]'s two edits, made here.
    fn churn(&mut self, client: u64, n: u64) {
        let mid = self.text[..self.text.len() / 2]
            .rfind('\n')
            .map_or(0, |k| k + 1);
        self.edit(client, mid, 0, &format!("line {n} by {client}\n"));
        let first = self.text.find('\n').unwrap() + 1;
        self.edit(client, 0, first, "");
    }

    /// The kit's doc, loaded from the history built.
    fn doc(&self) -> Doc {
        let main = &self.docs[&1].0;
        let state = main
            .transact()
            .encode_state_as_update_v2(&StateVector::default());
        Doc::load(S, &state).unwrap()
    }
}

const CLIENTS: u64 = 32;
const SEED_LINES: usize = 300;

fn seed_text() -> String {
    (0..SEED_LINES)
        .map(|i| format!("seed line {i}\n"))
        .collect()
}

/// A history of `changes` changes: the seed (one change by client 2), then churn by 32 agents.
fn build(changes: u64) -> Builder {
    let mut b = Builder::new((100..100 + CLIENTS).chain([2]));
    b.edit(2, 0, 0, &seed_text());
    while b.changes < changes {
        let n = b.changes;
        b.churn(100 + n % CLIENTS, n);
    }
    b
}

#[test]
#[ignore = "a measurement: minutes in release, on a node (--nocapture)"]
fn edit_cost_at_sizes() {
    let sizes: Vec<u64> = std::env::var("EDIT_COST_SIZES")
        .unwrap_or_else(|_| "1200,40000,300000".into())
        .split(',')
        .map(|v| v.trim().parse().expect("EDIT_COST_SIZES: numbers"))
        .collect();
    // the builder makes the kit's history: at the first size, byte-identical to the kit's own
    {
        let n = sizes[0];
        let b = build(n);
        let mut kit = Doc::new(S).unwrap();
        kit.replace_text_as(2, ROOT, &seed_text()).unwrap();
        let mut changes = 1;
        while changes < n {
            churn(&mut kit, 100 + changes % CLIENTS, changes);
            changes += 2;
        }
        assert_eq!(kit.text(ROOT).unwrap(), b.text);
        assert_eq!(kit.state_vector().len(), b.doc().state_vector().len());
        // state equality across two docs is content equality: upstream may split equal
        // content into blocks differently (k2-yrs k2.4's note)
        let built = b.doc();
        let bytes = kit.encode_state().unwrap() == built.encode_state().unwrap();
        assert!(
            bytes || kit.same_content(&built),
            "the builder's history must be the kit's"
        );
        eprintln!(
            "[edit cost] the builder's history at {n} changes is the kit's: {}",
            if bytes {
                "byte for byte"
            } else {
                "the same content (state vector, delete set, text), the bytes differ"
            }
        );
    }
    for &n in &sizes {
        let t0 = Instant::now();
        let b = build(n);
        let built = t0.elapsed().as_secs_f64();
        let state = b.doc().encode_state().unwrap();
        for &path in PATHS {
            let mut doc = Doc::load(S, &state).unwrap();
            let mut side = Side::new(path);
            // warm up: 10 churn steps, then time 100 (200 edits)
            let per_edit_us = with_side(&mut side, || {
                for k in 0..10 {
                    churn(&mut doc, 100 + (n + k) % CLIENTS, n + k);
                }
                let t = Instant::now();
                for k in 10..110 {
                    churn(&mut doc, 100 + (n + k) % CLIENTS, n + k);
                }
                t.elapsed().as_secs_f64() * 1e6 / 200.0
            });
            eprintln!(
                "[edit cost] {n} changes: {per_edit_us:.0} µs per edit through {path:?} \
                 (engine state {} B, text {} B; history built in {built:.1} s)",
                state.len(),
                b.text.len()
            );
            let job = k2_mpc_base::differential::runrec_job();
            eprintln!(
                "RUNREC {{\"job\":{job},\"run\":\"edit_cost_{n}_{}\",\"tool\":\"edit_cost\",\
                 \"status\":\"COMPLETE\",\"changes\":{n},\"edits\":200,\"warmup_steps\":10,\
                 \"per_edit_us\":{},\"state_bytes\":{},\"text_bytes\":{},\"build_s\":{}}}",
                format!("{path:?}").to_lowercase(),
                per_edit_us.round() as u64,
                state.len(),
                b.text.len(),
                built.round() as u64
            );
        }
        // the canary's cost on the production path: each edit timed alone, the check off and
        // at the default rate, from the same state
        for every in [0, k2_mpc_base::doc::DEFAULT_EDIT_CHECK_EVERY] {
            let mut doc = Doc::load(S, &state).unwrap();
            doc.set_edit_check_every(every);
            let mut side = Side::new(Path::MainDoc);
            let mut times = with_side(&mut side, || {
                for k in 0..10 {
                    churn(&mut doc, 100 + (n + k) % CLIENTS, n + k);
                }
                let mut times = Vec::with_capacity(CANARY_EDITS as usize);
                for k in 10..10 + CANARY_EDITS / 2 {
                    let c = 100 + (n + k) % CLIENTS;
                    let t = Instant::now();
                    churn_half(&mut doc, c, n + k, true);
                    times.push(t.elapsed().as_secs_f64() * 1e6);
                    let t = Instant::now();
                    churn_half(&mut doc, c, n + k, false);
                    times.push(t.elapsed().as_secs_f64() * 1e6);
                }
                times
            });
            assert!(!doc.edit_path_fallback(), "the canary found a mismatch");
            times.sort_by(f64::total_cmp);
            let q = |p: f64| times[((times.len() as f64 * p) as usize).min(times.len() - 1)];
            let mean = times.iter().sum::<f64>() / times.len() as f64;
            let job = k2_mpc_base::differential::runrec_job();
            eprintln!(
                "RUNREC {{\"job\":{job},\"run\":\"canary_{n}_{}\",\"tool\":\"edit_cost\",\
                 \"status\":\"COMPLETE\",\"changes\":{n},\"edits\":{},\"check_every\":{every},\
                 \"median_us\":{},\"ninety_ninth_us\":{},\"three_nines_us\":{},\"max_us\":{},\"mean_ns\":{}}}",
                if every == 0 {
                    "off".to_owned()
                } else {
                    every.to_string()
                },
                times.len(),
                q(0.50).round() as u64,
                q(0.99).round() as u64,
                q(0.999).round() as u64,
                times[times.len() - 1].round() as u64,
                (mean * 1000.0).round() as u64
            );
            eprintln!(
                "[edit cost] {n} changes, MainDoc, check {}: {} edits timed alone: p50 {:.0} µs, \
                 p99 {:.0} µs, p99.9 {:.0} µs, max {:.0} µs, mean {:.1} µs",
                if every == 0 {
                    "off".to_owned()
                } else {
                    format!("1 in {every}")
                },
                times.len(),
                q(0.50),
                q(0.99),
                q(0.999),
                times[times.len() - 1],
                mean
            );
        }
    }
}

/// Edits timed alone for the canary's cost (half inserts, half deletes).
const CANARY_EDITS: u64 = 5_000;

/// One of [`churn`]'s two edits: the insert, or the delete.
fn churn_half(doc: &mut Doc, client: u64, n: u64, insert: bool) {
    let t = doc.text(ROOT).unwrap();
    let hunks = if insert {
        let mid = t[..t.len() / 2].rfind('\n').map_or(0, |k| k + 1);
        vec![Hunk {
            start: mid,
            end: mid,
            insert: format!("line {n} by {client}\n"),
        }]
    } else {
        let first = t.find('\n').unwrap() + 1;
        vec![Hunk {
            start: 0,
            end: first,
            insert: String::new(),
        }]
    };
    doc.edit_as(
        client,
        &[Edit::Text {
            root: ROOT.into(),
            hunks,
        }],
    )
    .unwrap();
}
