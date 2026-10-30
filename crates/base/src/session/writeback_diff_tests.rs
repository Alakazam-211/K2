//! The file write-back's differential test (v0.9.7): the disk replica kept in step by the
//! changes since the last write-back, against the old path (reloaded from a whole encode of
//! the doc on every write-back, the test-only `full_writeback` switch).
//!
//! Two file-backed sessions run one seeded sequence of calls in lockstep (client IDs from the
//! same seed): edits, multi-hunk edits, undo, redo, revert, outside edits to the file, flushes,
//! ticks, restarts and purges; outside saves both ones the doc then holds (the replica becomes
//! the doc, brought forward like a write-back) and ones it doesn't (the doc has unsaved
//! changes). After **every** call they must hold byte-identical files,
//! disk replicas (encoded) and stored synced states, and the in-step side's replica must be
//! its live doc's state right after each write-back. The control: a session whose catch-up
//! list loses an update is caught.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use super::*;
use crate::differential::{with_side, Path as EditPath, Side};
use crate::store::{DocKey, DocStore};

struct H {
    now: AtomicU64,
    mismatches: AtomicU64,
}

impl Host for H {
    fn rights(&self, who: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(who.pid == "own")
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, ev: &AuditEvent) {
        if matches!(ev, AuditEvent::DiskReplicaMismatch { .. }) {
            self.mismatches.fetch_add(1, Ordering::SeqCst);
        }
    }
}

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);

/// Two sessions' encoded docs hold the same content: byte for byte, or, when the bytes differ,
/// the same inserts, deletions and texts (two docs fed the same changes may split equal
/// content into blocks differently upstream, so equal docs can encode differently).
fn same_doc(a: &[u8], b: &[u8]) -> bool {
    a == b
        || matches!(
            (crate::doc::Doc::load(MD.schema(), a), crate::doc::Doc::load(MD.schema(), b)),
            (Ok(x), Ok(y)) if x.same_content(&y)
        )
}

fn same_opt(a: &Option<Vec<u8>>, b: &Option<Vec<u8>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => same_doc(a, b),
        (None, None) => true,
        _ => false,
    }
}
const ROOT: &str = TextFilePack::ROOT;
const WHO: [(&str, ParticipantKind); 4] = [
    ("own", ParticipantKind::Person),
    ("ann", ParticipantKind::Person),
    ("bot1", ParticipantKind::Agent),
    ("bot2", ParticipantKind::Agent),
];

struct Side2 {
    side: Side,
    dir: PathBuf,
    host: Arc<H>,
    mgr: SessionManager,
    s: Arc<Session>,
    conns: Vec<ConnId>,
}

impl Drop for Side2 {
    fn drop(&mut self) {
        let _ = self.mgr.release();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn file(dir: &Path) -> PathBuf {
    dir.join("ws").join("a.md")
}

fn open(dir: &Path, host: &Arc<H>) -> (SessionManager, Arc<Session>) {
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let c = Config {
        edit_rate_per_s: 1_000_000,
        edit_burst: 1_000_000,
        idle_leave_ms: u64::MAX / 4,
        compact_entries: 64,
        writeback_check_every: 1,
        ..Config::default()
    };
    let mgr = SessionManager::new(store, host.clone(), c);
    let k = DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    };
    let s = mgr
        .open_file(Arc::new(MD), &k, &file(dir), &dir.join("ws"))
        .unwrap();
    (mgr, s)
}

impl Side2 {
    fn new(tag: &str, seed: u64, full: bool) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k2mb-wbdiff-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ws")).unwrap();
        std::fs::write(file(&dir), "one\ntwo\nthree\n").unwrap();
        let host = Arc::new(H {
            now: AtomicU64::new(1_000_000),
            mismatches: AtomicU64::new(0),
        });
        let mut side = Side::new(EditPath::MainDoc).with_client_seed(seed);
        if full {
            side = side.full_writeback();
        }
        let (mgr, s) = with_side(&mut side, || open(&dir, &host));
        let mut me = Self {
            side,
            dir,
            host,
            mgr,
            s,
            conns: Vec::new(),
        };
        me.join_all();
        me
    }

    fn join_all(&mut self) {
        let s = self.s.clone();
        self.conns = with_side(&mut self.side, || {
            WHO.iter()
                .map(|(pid, kind)| {
                    let p = Participant::new(*pid, *pid, "#123456", *kind, "cli");
                    s.join(p, Stay::Held).unwrap().conn
                })
                .collect()
        });
    }

    fn text(&self) -> String {
        self.s.read(self.conns[0]).unwrap().text_by_root[0]
            .1
            .clone()
    }

    fn step(&mut self, rng: &mut SmallRng, op: u32, who: usize, n: usize) -> String {
        self.host.now.fetch_add(17, Ordering::SeqCst);
        let s = self.s.clone();
        let c = self.conns[who];
        let t = self.text();
        let lines: Vec<usize> = std::iter::once(0)
            .chain(t.match_indices('\n').map(|(k, _)| k + 1))
            .filter(|&k| k < t.len())
            .collect();
        let pick = |rng: &mut SmallRng| {
            lines[rng
                .gen_range(0..lines.len().max(1))
                .min(lines.len().saturating_sub(1))]
        };
        let out = match op {
            0..=39 => {
                // one or two insertions (a multi-hunk edit), sometimes a line deleted
                let mut new = t.clone();
                let at = if lines.is_empty() { 0 } else { pick(rng) };
                new.insert_str(at, &format!("{}-{n} line\n", WHO[who].0));
                if rng.gen_bool(0.4) && lines.len() > 3 {
                    let k = new.find('\n').unwrap() + 1;
                    new.replace_range(..k, "");
                }
                if rng.gen_bool(0.3) {
                    new.push_str(&format!("tail {n}\n"));
                }
                format!(
                    "{:?}",
                    with_side(&mut self.side, || s.replace_text(c, ROOT, &new))
                )
            }
            40..=49 => format!(
                "{:?}",
                with_side(&mut self.side, || s
                    .undo_with(c, &UndoRequest::steps(1).mode(UndoMode::Partial)))
            ),
            50..=55 => format!("{:?}", with_side(&mut self.side, || s.redo(c, 1))),
            56..=60 => {
                let v = s.status().unwrap().version;
                let seq = rng.gen_range(1..=v.max(1));
                format!("{:?}", with_side(&mut self.side, || s.revert(c, seq, true)))
            }
            61..=66 => {
                // an outside edit to the file made on the live text, then the check that
                // merges it: the doc then holds the file's text (the replica becomes the doc)
                std::fs::write(file(&self.dir), format!("outside {n}\n{t}")).unwrap();
                format!("{:?}", with_side(&mut self.side, || s.check_disk()))
            }
            67..=72 => {
                // an outside edit made on the file as it is: with unsaved changes in the doc,
                // the doc then holds more than the file (the replica takes the save as a
                // client of its own); with none, as above
                let on_disk = std::fs::read_to_string(file(&self.dir)).unwrap();
                std::fs::write(file(&self.dir), format!("{on_disk}outside {n}\n")).unwrap();
                format!("{:?}", with_side(&mut self.side, || s.check_disk()))
            }
            73..=85 => format!("{:?}", with_side(&mut self.side, || s.flush(c))),
            86..=91 => {
                self.host.now.fetch_add(10_000, Ordering::SeqCst);
                let mgr = &self.mgr;
                format!(
                    "{:?}",
                    with_side(&mut self.side, || mgr.tick().map(|r| r.written.len()))
                )
            }
            92..=95 => {
                let scope = match rng.gen_range(0..3) {
                    0 => PurgeScope::History,
                    1 => PurgeScope::Text(format!("{}-{} line", WHO[who].0, n.saturating_sub(3))),
                    _ => {
                        let v = s.status().unwrap().version;
                        let from = rng.gen_range(1..=v.max(1));
                        PurgeScope::Changes { from, to: from }
                    }
                };
                format!(
                    "{:?}",
                    with_side(&mut self.side, || s
                        .purge(self.conns[0], PurgeRequest::new(scope)))
                    .map(|o| o.live_matches)
                )
            }
            _ => {
                self.mgr.release().unwrap();
                let (dir, host) = (self.dir.clone(), self.host.clone());
                let (mgr, s) = with_side(&mut self.side, || open(&dir, &host));
                self.mgr = mgr;
                self.s = s;
                self.join_all();
                "restarted".into()
            }
        };
        // file hashes are keyed by each store's own random key: not compared
        out.split("hash: ").next().unwrap_or("").to_owned()
    }
}

/// What a sequence exercised.
#[derive(Default, Debug)]
struct Counts {
    /// Flushes, and those that left the replica in step.
    writebacks: usize,
    in_step: usize,
    /// Outside saves ingested: the doc then held the file's text (the replica became the doc),
    /// or not (the replica took the save as a client of its own).
    outside_held: usize,
    outside_not_held: usize,
    /// Outside saves whose replica was brought forward by the catch-up list (no reload).
    outside_caught_up: usize,
}

/// One seeded sequence; the first difference, if any.
fn sequence(seed: u64, steps: usize) -> Result<Counts, String> {
    let mut rng = SmallRng::seed_from_u64(seed);
    let mut old = Side2::new("old", seed, true);
    let mut new = Side2::new("new", seed, false);
    let mut k = Counts::default();
    for n in 0..steps {
        let op = rng.gen_range(0..100);
        let who = rng.gen_range(0..WHO.len());
        let mut r2 = rng.clone();
        let caught_before = new.s.replica_caught_up_for_tests().unwrap();
        let a = old.step(&mut rng, op, who, n);
        let b = new.step(&mut r2, op, who, n);
        let at = |what: &str| format!("seed {seed}, step {n} (op {op}): {what}");
        if a != b {
            return Err(at(&format!("outcomes differ:\n  old: {a}\n  new: {b}")));
        }
        let fa = std::fs::read(file(&old.dir)).unwrap();
        let fb = std::fs::read(file(&new.dir)).unwrap();
        if fa != fb {
            return Err(at("the files differ"));
        }
        let (la, ra, _, sa) = old.s.writeback_state_for_tests().unwrap();
        let (lb, rb, known, sb) = new.s.writeback_state_for_tests().unwrap();
        if !same_doc(&la, &lb) {
            return Err(at("the live docs differ"));
        }
        if !same_opt(&ra, &rb) {
            return Err(at("the disk replicas differ"));
        }
        if !same_opt(&sa, &sb) {
            return Err(at("the stored synced states differ"));
        }
        if a.starts_with("Ok(FlushOutcome { written: true") || (73..=85).contains(&op) {
            k.writebacks += 1;
            if known {
                k.in_step += 1;
            }
        }
        if (61..=72).contains(&op) && a.starts_with("Ok(Ingested") {
            if known {
                k.outside_held += 1;
            } else {
                k.outside_not_held += 1;
            }
            if new.s.replica_caught_up_for_tests().unwrap() > caught_before {
                k.outside_caught_up += 1;
            }
        }
        if new.host.mismatches.load(Ordering::SeqCst) != 0 {
            return Err(at("the in-step side reported a replica mismatch"));
        }
        // K-AUD-1's sync check (v0.9.4): a clean doc's replica is the doc, on its elements
        for (side, s) in [("old", &old.s), ("new", &new.s)] {
            let drift = s.audit_disk_sync().unwrap();
            if !drift.is_empty() {
                return Err(at(&format!("the {side} side's sync audit: {drift:?}")));
            }
        }
    }
    Ok(k)
}

#[test]
fn incremental_writeback_matches_the_full_reencode() {
    let seeds: u64 = std::env::var("WB_DIFF_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    let mut t = Counts::default();
    for seed in 0..seeds {
        match sequence(seed, 200) {
            Ok(k) => {
                t.writebacks += k.writebacks;
                t.in_step += k.in_step;
                t.outside_held += k.outside_held;
                t.outside_not_held += k.outside_not_held;
                t.outside_caught_up += k.outside_caught_up;
            }
            Err(e) => panic!("{e}"),
        }
    }
    eprintln!("[writeback diff] {seeds} sequences of 200 calls: {} flushes, {} with the replica in step; outside saves: {} the doc then held ({} brought forward by the catch-up list), {} it didn't; byte-identical files; the same live docs, replicas and stored states; the sync audit clean after every call", t.writebacks, t.in_step, t.outside_held, t.outside_caught_up, t.outside_not_held);
    assert!(t.in_step > 0, "the in-step path never ran");
    assert!(t.outside_held > 0, "no outside save the doc then held");
    assert!(
        t.outside_not_held > 0,
        "no outside save the doc didn't hold"
    );
    assert!(
        t.outside_caught_up > 0,
        "no outside save brought the replica forward by the catch-up list"
    );
}

/// The controls: a catch-up list that lost an update. A lost insert is caught on every
/// write-back by the state vectors (O(clients)), check or not; a lost delete leaves them
/// equal, so only the sampled byte check sees it: with it on every write-back the mismatch is
/// reported and the replica reloaded (right); with it off the replica is left wrong, which
/// the comparison with the live doc shows.
#[test]
fn a_lost_update_is_caught_by_the_check() {
    for check in [true, false] {
        let mut side = Side2::new(if check { "ctl-on" } else { "ctl-off" }, 1, false);
        if !check {
            // the same session with the check off
            side.mgr.release().unwrap();
            let store = Arc::new(DocStore::open(side.dir.join("docs.redb")).unwrap());
            let c = Config {
                writeback_check_every: 0,
                ..Config::default()
            };
            side.mgr = SessionManager::new(store, side.host.clone(), c);
            let k = DocKey {
                space: "ws".into(),
                path: "a.md".into(),
            };
            side.s = side
                .mgr
                .open_file(Arc::new(MD), &k, &file(&side.dir), &side.dir.join("ws"))
                .unwrap();
            side.join_all();
        }
        let (s, c) = (side.s.clone(), side.conns[1]);
        // in step: a write-back first
        s.replace_text(c, ROOT, "one\ntwo\nthree\nfour\n").unwrap();
        s.flush(c).unwrap();
        // a lost insert: the state vectors differ
        s.replace_text(c, ROOT, "one\ntwo\nthree\nfour\nfive\n")
            .unwrap();
        assert!(s.lose_catch_up_for_tests().unwrap(), "an update to lose");
        s.flush(c).unwrap();
        let (live, replica, _, _) = s.writeback_state_for_tests().unwrap();
        assert_eq!(
            side.host.mismatches.load(Ordering::SeqCst),
            1,
            "a lost insert, reported"
        );
        assert_eq!(replica.unwrap(), live, "and the replica reloaded");
        // a lost delete: the state vectors are equal
        s.replace_text(c, ROOT, "one\ntwo\nfour\nfive\n").unwrap();
        assert!(s.lose_catch_up_for_tests().unwrap(), "an update to lose");
        s.flush(c).unwrap();
        let (live, replica, _, _) = s.writeback_state_for_tests().unwrap();
        let reported = side.host.mismatches.load(Ordering::SeqCst);
        if check {
            assert_eq!(reported, 2, "the check reports the lost delete");
            assert_eq!(replica.unwrap(), live, "and reloads the replica");
        } else {
            assert_eq!(reported, 1);
            assert_ne!(replica.unwrap(), live, "unchecked, the replica is wrong");
        }
    }
}
