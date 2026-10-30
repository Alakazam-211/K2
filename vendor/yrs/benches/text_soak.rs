//! A soak-shaped text history: one doc edited by many clients in small transactions, with the
//! cost of one small transaction measured as the history grows.
//!
//! The doc is a list of lines kept between `MIN_LINES` and `MAX_LINES`, so the live text stays
//! small while the history (and its deleted items) grows. Half the clients ("agents") edit the
//! doc itself through `Doc::transact_mut_as`; the other half ("people") edit a replica, whose
//! updates the doc applies (and the replica applies the doc's). Edits are small (a line added,
//! removed or replaced, or a few characters typed into a line) and mostly near each client's
//! own cursor.
//!
//! For each agent transaction the bench times three phases, as a host does: the ops (from
//! `transact_mut_as` to the last text op), `encode_update_v1`, and the commit (the drop). It
//! prints the mean of each phase over the `SOAK_WINDOW` versions before each checkpoint.
//!
//! Environment: `SOAK_VERSIONS` (default 100000), `SOAK_WINDOW` (1000), `SOAK_SEED` (1),
//! `SOAK_PEOPLE_SHARE` (percent of versions made on the replica, default 50),
//! `SOAK_CHECKPOINTS` (comma-separated, default 10000,25000,50000,100000), `SOAK_FULL_SV=1`
//! (the doc runs with full state vectors), `SOAK_CHURN` (per mille: the chance that an edit's
//! client first rejoins under a fresh client ID, as hosts give a rejoining session; default 0),
//! `SOAK_SHADOW` (the position index's shadow rate, default the doc's), `SOAK_RUNS=0` (the
//! doc steps through every deleted block: [Doc::set_deleted_runs]), `SOAK_SLOW_US` (log each
//! transaction slower than this, with the position index's counters; default 0, never).
//!
//! ```sh
//! cargo bench -p yrs --bench text_soak
//! ```

use std::time::{Duration, Instant};

use fastrand::Rng;
use yrs::updates::decoder::Decode;
use yrs::{
    ClientID, Doc, GetString, OffsetKind, Options, ReadTxn, Text, TextRef, Transact, Update,
};

const CLIENTS: u64 = 32;
const MIN_LINES: usize = 8;
const MAX_LINES: usize = 300;
const SEED_LINES: usize = 40;

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn new_doc(client: u64) -> Doc {
    Doc::with_options(Options {
        client_id: ClientID::new(client),
        offset_kind: OffsetKind::Utf16,
        skip_gc: false,
        ..Options::default()
    })
}

#[derive(Default, Clone, Copy)]
struct Phase {
    n: u64,
    /// The slowest transaction (all three phases) and its version.
    max: (Duration, u64),
    ops: Duration,
    encode: Duration,
    commit: Duration,
}

impl Phase {
    fn add(&mut self, v: u64, ops: Duration, encode: Duration, commit: Duration) {
        self.n += 1;
        if ops + encode + commit > self.max.0 {
            self.max = (ops + encode + commit, v);
        }
        self.ops += ops;
        self.encode += encode;
        self.commit += commit;
    }
    fn us(d: Duration, n: u64) -> f64 {
        d.as_secs_f64() * 1e6 / n.max(1) as f64
    }
    fn row(&self) -> (f64, f64, f64, f64) {
        let o = Self::us(self.ops, self.n);
        let e = Self::us(self.encode, self.n);
        let c = Self::us(self.commit, self.n);
        (o, e, c, o + e + c)
    }
}

/// One edit, at line granularity, against the mirror of the text.
enum Edit {
    /// Insert `text` at UTF-16 offset `at`.
    Insert(u32, String),
    /// Remove `len` units at `at`.
    Remove(u32, u32),
    /// Remove `len` at `at`, then insert `text` there.
    Replace(u32, u32, String),
}

struct World {
    lines: Vec<String>,
    cursors: Vec<usize>,
    next_token: u64,
    rng: Rng,
}

impl World {
    fn offset_of(&self, line: usize) -> u32 {
        self.lines[..line].iter().map(|l| l.len() as u32).sum()
    }

    fn token(&mut self, client: u64) -> String {
        self.next_token += 1;
        format!("c{client}-t{}\n", self.next_token)
    }

    /// The next edit for `client`, applied to the mirror.
    fn next_edit(&mut self, client: u64) -> Edit {
        let n = self.lines.len();
        let c = client as usize;
        // mostly local: near this client's cursor; sometimes anywhere
        let line = if self.rng.u8(0..100) < 80 {
            let cur = self.cursors[c].min(n);
            let lo = cur.saturating_sub(2);
            let hi = (cur + 2).min(n);
            self.rng.usize(lo..=hi)
        } else {
            self.rng.usize(0..=n)
        };
        let roll = self.rng.u8(0..100);
        let (ins_p, del_p) = if n >= MAX_LINES {
            (10, 70)
        } else if n <= MIN_LINES {
            (80, 0)
        } else {
            (40, 30)
        };
        if roll < ins_p || line >= n {
            let t = self.token(client);
            let at = self.offset_of(line.min(n));
            self.lines.insert(line.min(n), t.clone());
            self.cursors[c] = line.min(n) + 1;
            Edit::Insert(at, t)
        } else if roll < ins_p + del_p {
            let at = self.offset_of(line);
            let len = self.lines[line].len() as u32;
            self.lines.remove(line);
            self.cursors[c] = line;
            Edit::Remove(at, len)
        } else if roll < 90 {
            let at = self.offset_of(line);
            let len = self.lines[line].len() as u32;
            let t = self.token(client);
            self.lines[line] = t.clone();
            self.cursors[c] = line + 1;
            Edit::Replace(at, len, t)
        } else {
            // type a few characters inside the line, before its newline
            let l = &self.lines[line];
            let within = self.rng.usize(0..l.len());
            let typed: String = (0..self.rng.usize(1..4))
                .map(|_| self.rng.alphanumeric())
                .collect();
            let at = self.offset_of(line) + within as u32;
            self.lines[line].insert_str(within, &typed);
            self.cursors[c] = line;
            Edit::Insert(at, typed)
        }
    }
}

fn apply_edit(t: &TextRef, txn: &mut yrs::TransactionMut, e: &Edit) {
    match e {
        Edit::Insert(at, s) => t.insert(txn, *at, s),
        Edit::Remove(at, len) => t.remove_range(txn, *at, *len),
        Edit::Replace(at, len, s) => {
            t.remove_range(txn, *at, *len);
            t.insert(txn, *at, s);
        }
    }
}

fn main() {
    // `cargo bench` passes `--bench`; nothing else is read from argv
    let versions = env_u64("SOAK_VERSIONS", 100_000);
    let window = env_u64("SOAK_WINDOW", 1000);
    let seed = env_u64("SOAK_SEED", 1);
    let people_share = env_u64("SOAK_PEOPLE_SHARE", 50);
    let full_sv = env_u64("SOAK_FULL_SV", 0) == 1;
    let churn = env_u64("SOAK_CHURN", 0);
    let slow_us = env_u64("SOAK_SLOW_US", 0);
    // each logical client's current client ID
    let mut ids: Vec<u64> = (1..=CLIENTS).collect();
    let mut next_id = 10_000u64;
    let mut checkpoints: Vec<u64> = std::env::var("SOAK_CHECKPOINTS")
        .unwrap_or_else(|_| "10000,25000,50000,100000".into())
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .filter(|&c| c <= versions)
        .collect();
    checkpoints.sort_unstable();

    // the doc (the host's) and one replica standing for the people's clients
    let doc = new_doc(1_000_000);
    let replica = new_doc(2_000_000);
    if full_sv {
        doc.set_full_state_vectors(true);
    }
    if let Ok(v) = std::env::var("SOAK_SHADOW") {
        doc.set_position_shadow_rate(v.parse().expect("SOAK_SHADOW"));
    }
    let runs = env_u64("SOAK_RUNS", 1) == 1;
    doc.set_deleted_runs(runs);
    let text = doc.get_or_insert_text("t");
    let rtext = replica.get_or_insert_text("t");

    let mut world = World {
        lines: Vec::new(),
        cursors: vec![0; CLIENTS as usize],
        next_token: 0,
        rng: Rng::with_seed(seed),
    };
    {
        let mut txn = doc.transact_mut_as(ClientID::new(999));
        for _ in 0..SEED_LINES {
            let t = world.token(999);
            let at = world.offset_of(world.lines.len());
            world.lines.push(t.clone());
            text.insert(&mut txn, at, &t);
        }
        let u = txn.encode_update_v1();
        drop(txn);
        replica
            .transact_mut()
            .apply_update(Update::decode_v1(&u).unwrap())
            .unwrap();
    }

    println!(
        "text_soak: versions={versions} window={window} seed={seed} people_share={people_share}% full_sv={full_sv} churn={churn}permille runs={runs}"
    );
    println!(
        "{:>8} {:>9} {:>9} {:>9} {:>9} {:>6} {:>8} {:>8} {:>7} {:>8} {:>8} {:>6}",
        "version",
        "ops_us",
        "encode_us",
        "commit_us",
        "total_us",
        "lines",
        "state_B",
        "upd_B",
        "clients",
        "max_us",
        "max_at",
        "runs"
    );

    let mut phase = Phase::default();
    let mut upd_bytes: u64 = 0;
    let mut rows: Vec<(u64, (f64, f64, f64, f64))> = Vec::new();
    let mut cp = 0usize;
    let started = Instant::now();
    for v in 1..=versions {
        let person = world.rng.u64(0..100) < people_share;
        let client = if person {
            CLIENTS / 2 + world.rng.u64(0..CLIENTS / 2)
        } else {
            world.rng.u64(0..CLIENTS / 2)
        };
        let edit = world.next_edit(client);
        if churn > 0 && world.rng.u64(0..1000) < churn {
            next_id += 1;
            ids[client as usize] = next_id;
        }
        let id = ids[client as usize];
        if person {
            // made on the replica, applied to the doc as a remote update
            let u = {
                let mut txn = replica.transact_mut_as(ClientID::new(id));
                apply_edit(&rtext, &mut txn, &edit);
                txn.encode_update_v1()
            };
            doc.transact_mut()
                .apply_update(Update::decode_v1(&u).unwrap())
                .unwrap();
        } else {
            let st0 = doc.position_index_stats();
            let t0 = Instant::now();
            let mut txn = doc.transact_mut_as(ClientID::new(id));
            apply_edit(&text, &mut txn, &edit);
            let t1 = Instant::now();
            let u = txn.encode_update_v1();
            let t2 = Instant::now();
            drop(txn);
            let t3 = Instant::now();
            if slow_us > 0 && (t3 - t0).as_micros() as u64 > slow_us {
                let st1 = doc.position_index_stats();
                let what = match &edit {
                    Edit::Insert(a, t) => format!("insert at {a}, {} units", t.len()),
                    Edit::Remove(a, l) => format!("remove at {a}, {l} units"),
                    Edit::Replace(a, l, t) => format!("replace at {a}, {l} by {} units", t.len()),
                };
                eprintln!(
                    "slow: v{v} {:?} {what}; lookups +{}, marker hits +{}, walks +{}, shadow checks +{}",
                    t3 - t0,
                    st1.lookups - st0.lookups,
                    st1.marker_hits - st0.marker_hits,
                    st1.walks - st0.walks,
                    st1.shadow_checks - st0.shadow_checks
                );
            }
            if v + window > checkpoints.get(cp).copied().unwrap_or(u64::MAX) {
                phase.add(v, t1 - t0, t2 - t1, t3 - t2);
                upd_bytes += u.len() as u64;
            }
            replica
                .transact_mut()
                .apply_update(Update::decode_v1(&u).unwrap())
                .unwrap();
        }
        if Some(v) == checkpoints.get(cp).copied() {
            let r = phase.row();
            let blocks = doc
                .transact()
                .encode_state_as_update_v1(&Default::default())
                .len();
            println!(
                "{:>8} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>6} {:>8} {:>8.0} {:>7} {:>8.0} {:>8} {:>6}",
                v,
                r.0,
                r.1,
                r.2,
                r.3,
                world.lines.len(),
                blocks,
                upd_bytes as f64 / phase.n.max(1) as f64,
                doc.transact().state_vector().len(),
                phase.max.0.as_secs_f64() * 1e6,
                phase.max.1,
                {
                    // the text's recorded run entries (the larger of its two maps)
                    let (a, b) = AsRef::<yrs::branch::Branch>::as_ref(&text).deleted_runs_len();
                    a.max(b)
                }
            );
            rows.push((v, r));
            // the mirror and both docs agree
            let want: String = world.lines.concat();
            assert_eq!(text.get_string(&doc.transact()), want, "doc text at v{v}");
            assert_eq!(
                rtext.get_string(&replica.transact()),
                want,
                "replica text at v{v}"
            );
            phase = Phase::default();
            upd_bytes = 0;
            cp += 1;
        }
    }
    if let (Some(first), Some(last)) = (rows.first(), rows.last()) {
        println!(
            "growth v{}->v{}: ops {:.2}x, encode {:.2}x, commit {:.2}x, total {:.2}x",
            first.0,
            last.0,
            last.1 .0 / first.1 .0,
            last.1 .1 / first.1 .1,
            last.1 .2 / first.1 .2,
            last.1 .3 / first.1 .3
        );
    }
    println!(
        "position index: {:?}; wall {:.1}s",
        doc.position_index_stats(),
        started.elapsed().as_secs_f64()
    );
}
