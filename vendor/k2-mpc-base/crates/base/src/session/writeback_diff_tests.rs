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
use crate::doc::DocError;
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
        Self::new_with(tag, seed, full, true)
    }

    /// `decoded`: a decoded disk replica, as before v0.23 (else the text doc's texts only).
    fn new_with(tag: &str, seed: u64, full: bool, decoded: bool) -> Self {
        Self::new_opts(tag, seed, full, decoded, false)
    }

    /// `cache`: the engine keeps its own text cache, as before v0.24 (else the session owns
    /// the doc's texts).
    fn new_opts(tag: &str, seed: u64, full: bool, decoded: bool, cache: bool) -> Self {
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
        // the catch-up this compares is a decoded replica's (a text doc's replica holds its
        // texts since v0.23; a structured doc's still catches up)
        let mut side = Side::new(EditPath::MainDoc).with_client_seed(seed);
        if decoded {
            side = side.decoded_replica();
        }
        if cache {
            side = side.engine_text_cache();
        }
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
        // the sync check (v0.9.4): a clean doc's replica is the doc, on its elements
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
            side.s = crate::differential::with_decoded_replica(|| {
                side.mgr
                    .open_file(Arc::new(MD), &k, &file(&side.dir), &side.dir.join("ws"))
                    .unwrap()
            });
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

// ----- v0.23: a text doc's disk replica holds its texts only -----

/// The text-only replica against the decoded one (v0.23), in lockstep: one side keeps the
/// disk replica decoded, as before (a whole Yrs doc, brought forward by the changes since each
/// sync), the other its texts only, decoding the synced state the store keeps only for an
/// outside save its doc doesn't hold. After every call: the same outcome, byte-identical
/// files, the same live docs, the same stored synced states, the same replica texts (the merge
/// base), the sync audit clean on both, and the text side's replica is texts. Counts how often
/// the stored states were byte-identical (equal content may encode differently).
fn sequence_text_replica(seed: u64, steps: usize) -> Result<(Counts, usize, usize), String> {
    let mut rng = SmallRng::seed_from_u64(seed);
    let mut old = Side2::new_with("dec", seed, false, true);
    let mut new = Side2::new_with("txt", seed, false, false);
    let mut k = Counts::default();
    let (mut same_bytes, mut compared) = (0usize, 0usize);
    for n in 0..steps {
        let op = rng.gen_range(0..100);
        let who = rng.gen_range(0..WHO.len());
        let mut r2 = rng.clone();
        let a = old.step(&mut rng, op, who, n);
        let b = new.step(&mut r2, op, who, n);
        let at = |what: &str| format!("seed {seed}, step {n} (op {op}): {what}");
        if a != b {
            return Err(at(&format!(
                "outcomes differ:\n  decoded: {a}\n  texts: {b}"
            )));
        }
        if std::fs::read(file(&old.dir)).unwrap() != std::fs::read(file(&new.dir)).unwrap() {
            return Err(at("the files differ"));
        }
        let (la, _, known, sa) = old.s.writeback_state_for_tests().unwrap();
        let (lb, _, _, sb) = new.s.writeback_state_for_tests().unwrap();
        if !same_doc(&la, &lb) {
            return Err(at("the live docs differ"));
        }
        if !same_opt(&sa, &sb) {
            return Err(at("the stored synced states differ"));
        }
        if let (Some(x), Some(y)) = (&sa, &sb) {
            compared += 1;
            same_bytes += usize::from(x == y);
        }
        if old.s.disk_replica_texts_for_tests().unwrap()
            != new.s.disk_replica_texts_for_tests().unwrap()
        {
            return Err(at("the replicas' texts differ"));
        }
        match new.s.disk_replica_kind_for_tests().unwrap() {
            Some(("text" | "stored-text", _)) => {}
            other => return Err(at(&format!("the text side's replica is {other:?}"))),
        }
        if (61..=72).contains(&op) && a.starts_with("Ok(Ingested") {
            if known {
                k.outside_held += 1;
            } else {
                k.outside_not_held += 1;
            }
        }
        if (73..=85).contains(&op) {
            k.writebacks += 1;
        }
        for (side, s) in [("decoded", &old.s), ("texts", &new.s)] {
            let drift = s.audit_disk_sync().unwrap();
            if !drift.is_empty() {
                return Err(at(&format!("the {side} side's sync audit: {drift:?}")));
            }
        }
    }
    Ok((k, same_bytes, compared))
}

#[test]
fn the_text_replica_matches_the_decoded_replica() {
    let seeds: u64 = std::env::var("WB_DIFF_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    let mut t = Counts::default();
    let (mut bytes, mut compared) = (0, 0);
    for seed in 0..seeds {
        match sequence_text_replica(seed, 200) {
            Ok((k, b, c)) => {
                t.writebacks += k.writebacks;
                t.outside_held += k.outside_held;
                t.outside_not_held += k.outside_not_held;
                bytes += b;
                compared += c;
            }
            Err(e) => panic!("{e}"),
        }
    }
    eprintln!("[text replica] {seeds} sequences of 200 calls: {} flushes; outside saves: {} the doc then held, {} it didn't (the synced state decoded from the store); byte-identical files and replica texts; the same live docs and stored states ({bytes} of {compared} stored states byte-identical); the sync audit clean after every call", t.writebacks, t.outside_held, t.outside_not_held);
    assert!(t.writebacks > 0, "no flush");
    assert!(t.outside_held > 0, "no outside save the doc then held");
    assert!(
        t.outside_not_held > 0,
        "no outside save the doc didn't hold"
    );
}

/// The one path that needs the replica's elements, step by step (v0.23): an edit the file
/// doesn't have, then an outside save. Both sides merge it from the same base text and store
/// the save as a client of its own on the synced state; the text side decodes that state from
/// the store. The files, live docs and stored states match; then the write-back matches too.
#[test]
fn an_outside_save_the_doc_doesnt_hold_stores_what_the_decoded_replica_stored() {
    let mut old = Side2::new_with("rare-dec", 7, false, true);
    let mut new = Side2::new_with("rare-txt", 7, false, false);
    for side in [&mut old, &mut new] {
        let (s, c) = (side.s.clone(), side.conns[1]);
        with_side(&mut side.side, || {
            s.replace_text(c, ROOT, "one\ntwo\nthree\n").unwrap();
            s.flush(c).unwrap();
            // a change the file doesn't have yet
            s.replace_text(c, ROOT, "one\ntwo\nthree\nlive four\n")
                .unwrap();
        });
        std::fs::write(file(&side.dir), "zero\none\ntwo\nthree\n").unwrap();
        let out = with_side(&mut side.side, || s.check_disk()).unwrap();
        assert!(matches!(out, DiskCheck::Ingested { .. }), "{out:?}");
    }
    assert_eq!(
        new.s.disk_replica_kind_for_tests().unwrap().map(|k| k.0),
        Some("text")
    );
    assert_eq!(
        old.s.disk_replica_kind_for_tests().unwrap().map(|k| k.0),
        Some("doc")
    );
    let (la, _, known_a, sa) = old.s.writeback_state_for_tests().unwrap();
    let (lb, _, known_b, sb) = new.s.writeback_state_for_tests().unwrap();
    assert!(
        !known_a && !known_b,
        "the doc holds more than the file on both"
    );
    assert_eq!(new.text(), "zero\none\ntwo\nthree\nlive four\n");
    assert_eq!(old.text(), new.text());
    assert!(same_doc(&la, &lb), "the live docs");
    assert!(same_opt(&sa, &sb), "the stored synced states");
    eprintln!(
        "[text replica] the save the doc doesn't hold: stored states byte-identical: {}",
        sa == sb
    );
    assert_eq!(
        old.s.disk_replica_texts_for_tests().unwrap(),
        new.s.disk_replica_texts_for_tests().unwrap(),
        "the merge base: the file's text"
    );
    for side in [&mut old, &mut new] {
        let (s, c) = (side.s.clone(), side.conns[1]);
        assert!(with_side(&mut side.side, || s.flush(c)).unwrap().written);
    }
    assert_eq!(
        std::fs::read(file(&old.dir)).unwrap(),
        std::fs::read(file(&new.dir)).unwrap()
    );
    let (_, _, _, sa) = old.s.writeback_state_for_tests().unwrap();
    let (_, _, _, sb) = new.s.writeback_state_for_tests().unwrap();
    assert!(
        same_opt(&sa, &sb),
        "the stored synced states after the write-back"
    );
    for s in [&old.s, &new.s] {
        assert_eq!(s.audit_disk_sync().unwrap(), vec![]);
    }
}

/// The text-only replica holds the file's texts and nothing else (v0.23), through an open, a
/// write-back, an outside save the doc then holds, one it doesn't, and a restart: never a
/// decoded doc. The reference side (the switch) still keeps a decoded doc.
#[test]
fn a_text_docs_disk_replica_holds_its_texts_only() {
    let mut new = Side2::new_with("kind-txt", 3, false, false);
    let kind = |s: &Arc<Session>| s.disk_replica_kind_for_tests().unwrap().unwrap();
    let (s, c) = (new.s.clone(), new.conns[1]);
    assert!(
        matches!(kind(&s).0, "text" | "stored-text"),
        "opened: {:?}",
        kind(&s)
    );
    s.replace_text(c, ROOT, "one\ntwo\nthree\nfour\n").unwrap();
    s.flush(c).unwrap();
    assert_eq!(
        kind(&s),
        ("text", "one\ntwo\nthree\nfour\n".len()),
        "written back"
    );
    std::fs::write(file(&new.dir), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    s.check_disk().unwrap();
    assert_eq!(kind(&s).0, "text", "an outside save the doc then holds");
    s.replace_text(c, ROOT, "1\ntwo\nthree\nfour\nfive\n")
        .unwrap();
    std::fs::write(file(&new.dir), "one\ntwo\nthree\nfour\nfive\nsix\n").unwrap();
    s.check_disk().unwrap();
    assert_eq!(
        kind(&s),
        ("text", "one\ntwo\nthree\nfour\nfive\nsix\n".len()),
        "an outside save the doc doesn't hold: the file's text"
    );
    new.mgr.release().unwrap();
    let (dir, host) = (new.dir.clone(), new.host.clone());
    let (mgr, s) = with_side(&mut new.side, || open(&dir, &host));
    new.mgr = mgr;
    new.s = s.clone();
    assert!(
        matches!(kind(&s).0, "text" | "stored-text"),
        "reopened: {:?}",
        kind(&s)
    );
    new.join_all();
    // the close wrote the doc back: an edit, then its write-back, decodes the stored texts
    s.replace_text(new.conns[1], ROOT, "1\n").unwrap();
    assert!(s.flush(new.conns[1]).unwrap().written);
    assert_eq!(kind(&s), ("text", 2), "after the reopen's write-back");
    // the reference side keeps the decoded doc
    let old = Side2::new_with("kind-dec", 3, false, true);
    let (s, c) = (old.s.clone(), old.conns[1]);
    s.replace_text(c, ROOT, "one\ntwo\n").unwrap();
    s.flush(c).unwrap();
    assert_eq!(kind(&s).0, "doc");
}

/// The on-demand decode is checked, and refused typed, never merged on (v0.23): a stored
/// synced state that isn't the one the session synced (another length), or that holds other
/// texts than the replica's (the same length recorded), fails the outside save the doc doesn't
/// hold with `DocError::Load`; the doc keeps its text and version (the save isn't taken).
#[test]
fn a_stored_synced_state_not_the_replicas_is_refused() {
    for record in [false, true] {
        let new = Side2::new_with(
            if record { "refuse-t" } else { "refuse-n" },
            5,
            false,
            false,
        );
        let (s, c) = (new.s.clone(), new.conns[1]);
        s.replace_text(c, ROOT, "one\ntwo\n").unwrap();
        s.flush(c).unwrap();
        s.replace_text(c, ROOT, "one\ntwo\nlive\n").unwrap();
        let before = (new.text(), s.status().unwrap().version);
        s.plant_synced_row_for_tests("stale\n", record).unwrap();
        std::fs::write(file(&new.dir), "zero\none\ntwo\n").unwrap();
        let why = if record {
            "its texts aren't the disk replica's"
        } else {
            "it isn't the state this session synced"
        };
        match s.check_disk() {
            Err(SessionError::Doc(DocError::Load(m))) => {
                assert!(m.contains(why), "record {record}: {m}")
            }
            other => panic!("record {record}: refused, not {other:?}"),
        }
        assert_eq!(
            (new.text(), s.status().unwrap().version),
            before,
            "record {record}: the save isn't taken"
        );
    }
}

// ----- v0.24: the session owns a text doc's texts (no engine copy) -----

/// The session-owned texts against the engine's own cache (v0.24), in lockstep over the same
/// seeded calls: the same outcomes, byte-identical files, live docs and stored states, the
/// same replica texts, the sync audit clean on both, and the owning side's engine keeps no
/// text. Counts the live docs compared byte for byte.
fn sequence_owned_texts(seed: u64, steps: usize) -> Result<(usize, usize), String> {
    let mut rng = SmallRng::seed_from_u64(seed);
    let mut old = Side2::new_opts("cache", seed, false, false, true);
    let mut new = Side2::new_opts("owned", seed, false, false, false);
    let (mut bytes, mut compared) = (0usize, 0usize);
    for n in 0..steps {
        let op = rng.gen_range(0..100);
        let who = rng.gen_range(0..WHO.len());
        let mut r2 = rng.clone();
        let a = old.step(&mut rng, op, who, n);
        let b = new.step(&mut r2, op, who, n);
        let at = |what: &str| format!("seed {seed}, step {n} (op {op}): {what}");
        if a != b {
            return Err(at(&format!("outcomes differ:\n  cache: {a}\n  owned: {b}")));
        }
        if std::fs::read(file(&old.dir)).unwrap() != std::fs::read(file(&new.dir)).unwrap() {
            return Err(at("the files differ"));
        }
        let (la, _, _, sa) = old.s.writeback_state_for_tests().unwrap();
        let (lb, _, _, sb) = new.s.writeback_state_for_tests().unwrap();
        if !same_doc(&la, &lb) {
            return Err(at("the live docs differ"));
        }
        compared += 1;
        bytes += usize::from(la == lb);
        if !same_opt(&sa, &sb) {
            return Err(at("the stored synced states differ"));
        }
        if old.text() != new.text() {
            return Err(at("the texts differ"));
        }
        if new.s.engine_text_cache_bytes_for_tests().unwrap() != 0 {
            return Err(at("the owning side's engine keeps a text"));
        }
        for (side, s) in [("cache", &old.s), ("owned", &new.s)] {
            let drift = s.audit_disk_sync().unwrap();
            if !drift.is_empty() {
                return Err(at(&format!("the {side} side's sync audit: {drift:?}")));
            }
        }
    }
    Ok((bytes, compared))
}

#[test]
fn the_session_owned_texts_match_the_engines_cache() {
    let seeds: u64 = std::env::var("WB_DIFF_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    let (mut bytes, mut compared) = (0, 0);
    for seed in 0..seeds {
        match sequence_owned_texts(seed, 200) {
            Ok((b, c)) => {
                bytes += b;
                compared += c;
            }
            Err(e) => panic!("{e}"),
        }
    }
    eprintln!("[owned texts] {seeds} sequences of 200 calls: the same outcomes, files, texts, stored states and replica texts; {bytes} of {compared} live docs byte-identical; the owning side's engine kept no text; the sync audit clean after every call");
    assert!(compared > 0, "control: compared");
}

/// The session owns a text doc's texts (v0.24): after edits, a write-back and an outside
/// save, the engine keeps no copy; on the reference side it keeps one per root.
#[test]
fn a_text_docs_engine_keeps_no_text_of_its_own() {
    let new = Side2::new_opts("own-txt", 9, false, false, false);
    let old = Side2::new_opts("own-cache", 9, false, false, true);
    for (side, owned) in [(&new, true), (&old, false)] {
        let (s, c) = (side.s.clone(), side.conns[1]);
        s.replace_text(c, ROOT, "one\ntwo\nthree\n").unwrap();
        s.flush(c).unwrap();
        s.replace_text(c, ROOT, "one\ntwo\nthree\nfour\n").unwrap();
        std::fs::write(file(&side.dir), "zero\none\ntwo\nthree\n").unwrap();
        s.check_disk().unwrap();
        let cached = s.engine_text_cache_bytes_for_tests().unwrap();
        if owned {
            assert_eq!(cached, 0, "row: the owning side's engine keeps no text");
        } else {
            assert!(
                cached > 0,
                "control: the reference side's engine caches its text"
            );
        }
    }
    assert_eq!(new.text(), old.text(), "row: the same text on both");
}

// ----- v0.24: a text doc's texts shared by their holders until the live text changes -----

/// One text, one allocation (v0.24): after a write-back with no edit since, the disk replica's
/// text is the live text's own; so is a read base taken with no edit since, and the replica's
/// after an outside save the doc then holds. An edit gives the live text a new one and leaves
/// the others where they were.
#[test]
fn shared_texts_are_one_allocation_until_an_edit() {
    let side = Side2::new_with("share-mem", 11, false, false);
    let (s, c) = (side.s.clone(), side.conns[1]);
    let sharing = |s: &Arc<Session>| s.text_sharing_for_tests().unwrap();
    s.replace_text(c, ROOT, "one\ntwo\nthree\nfour\n").unwrap();
    assert!(s.flush(c).unwrap().written, "control: written back");
    s.read(c).unwrap();
    assert_eq!(
        sharing(&s),
        (Some(true), Some(true)),
        "row: after a write-back and a read with no edit since, the replica's and the read \
         base's texts are the live text's allocation"
    );
    s.replace_text(c, ROOT, "one\ntwo\nthree\nfour\nfive\n")
        .unwrap();
    assert_eq!(
        sharing(&s),
        (Some(false), Some(false)),
        "row: after sharing, an edit gives the live text an allocation of its own"
    );
    s.read(c).unwrap();
    assert_eq!(
        sharing(&s),
        (Some(false), Some(true)),
        "row: a read base taken with no edit since is the live text's allocation"
    );
    assert!(s.flush(c).unwrap().written, "control: written back");
    assert_eq!(
        sharing(&s),
        (Some(true), Some(true)),
        "row: after the write-back, the replica's text is the live text's allocation"
    );
    std::fs::write(file(&side.dir), "zero\none\ntwo\nthree\nfour\nfive\n").unwrap();
    s.check_disk().unwrap();
    assert_eq!(
        side.text(),
        "zero\none\ntwo\nthree\nfour\nfive\n",
        "control: the save is taken in"
    );
    assert_eq!(
        sharing(&s),
        (Some(true), Some(true)),
        "row: after an outside save the doc then holds, the replica's text is the live text's \
         allocation"
    );
}

/// After sharing, an edit leaves the disk replica's text and the read base's exactly as they
/// were (v0.24), while the live text changes; a `write --base` at the shared read base then
/// merges from that base's text.
#[test]
fn an_edit_after_shared_texts_leaves_the_replica_and_read_base_as_they_were() {
    let side = Side2::new_with("share-edit", 12, false, false);
    let (s, c) = (side.s.clone(), side.conns[1]);
    let t1 = "one\ntwo\nthree\nnew\n";
    s.replace_text(c, ROOT, t1).unwrap();
    s.read(c).unwrap();
    let v1 = s.status().unwrap().version;
    assert!(s.flush(c).unwrap().written, "control: written back");
    assert_eq!(
        s.text_sharing_for_tests().unwrap(),
        (Some(true), Some(true)),
        "control: the replica's, the read base's and the live text are one allocation"
    );
    s.replace_text(c, ROOT, "one\ntwo\nthree\nnew\nfour\n")
        .unwrap();
    let want = Some(vec![(ROOT.to_owned(), t1.to_owned())]);
    assert_eq!(
        s.disk_replica_texts_for_tests().unwrap(),
        want,
        "row: after sharing, an edit leaves the replica's text as it was"
    );
    assert_eq!(
        s.read_base_texts_for_tests(v1).unwrap(),
        want,
        "row: after sharing, an edit leaves the read base's text as it was"
    );
    assert_eq!(
        side.text(),
        "one\ntwo\nthree\nnew\nfour\n",
        "row: the live text changes"
    );
    // an agent's write at the shared base: its own change merged onto the live text
    s.write_base(side.conns[2], ROOT, v1, "ONE\ntwo\nthree\nnew\n")
        .unwrap();
    assert_eq!(
        side.text(),
        "ONE\ntwo\nthree\nnew\nfour\n",
        "row: after sharing, a write at the shared base merges from that base's text"
    );
}

/// Sharing, then outside saves (v0.24): one the doc then holds, and one it doesn't (an edit
/// unwritten), then the write-back, in lockstep with a decoded replica (which never shares its
/// text): the same outcomes, files, live texts and replica texts, and the ones expected.
#[test]
fn outside_saves_after_shared_texts_match_the_decoded_replica() {
    let new = Side2::new_with("share-out-t", 13, false, false);
    let old = Side2::new_with("share-out-d", 13, false, true);
    let run = |side: &Side2| {
        let (s, c) = (side.s.clone(), side.conns[1]);
        let mut seen = Vec::new();
        s.replace_text(c, ROOT, "one\ntwo\nthree\nfour\n").unwrap();
        s.read(c).unwrap();
        assert!(s.flush(c).unwrap().written, "control: written back");
        let shared = s.text_sharing_for_tests().unwrap();
        // a save the doc then holds
        std::fs::write(file(&side.dir), "zero\none\ntwo\nthree\nfour\n").unwrap();
        let out = format!("{:?}", s.check_disk().unwrap());
        seen.push((
            out.split(" {").next().unwrap_or("").to_owned(),
            std::fs::read_to_string(file(&side.dir)).unwrap(),
            side.text(),
            s.disk_replica_texts_for_tests().unwrap(),
        ));
        // an edit unwritten, then a save the doc doesn't hold
        s.replace_text(c, ROOT, "zero\none\ntwo\nthree\nfour\nfive\n")
            .unwrap();
        std::fs::write(file(&side.dir), "zero\none\nTWO\nthree\nfour\n").unwrap();
        let out = format!("{:?}", s.check_disk().unwrap());
        seen.push((
            out.split(" {").next().unwrap_or("").to_owned(),
            std::fs::read_to_string(file(&side.dir)).unwrap(),
            side.text(),
            s.disk_replica_texts_for_tests().unwrap(),
        ));
        // (whether it wrote is compared, not asserted: the sides must agree)
        let written = s.flush(c).unwrap().written;
        seen.push((
            format!("written {written}"),
            std::fs::read_to_string(file(&side.dir)).unwrap(),
            side.text(),
            s.disk_replica_texts_for_tests().unwrap(),
        ));
        assert_eq!(s.audit_disk_sync().unwrap(), vec![], "the sync audit");
        (seen, shared)
    };
    let ((a, shared), (b, dec_shared)) = (run(&new), run(&old));
    assert_eq!(
        (shared, dec_shared),
        ((Some(true), Some(true)), (None, Some(true))),
        "control: the texts are shared before the saves (a decoded replica holds a doc)"
    );
    assert_eq!(
        a, b,
        "row: after sharing, the same outcomes, files and texts as a decoded replica"
    );
    let replica = |t: &str| Some(vec![(ROOT.to_owned(), t.to_owned())]);
    let merged = "zero\none\nTWO\nthree\nfour\nfive\n";
    assert_eq!(
        (a[1].1.as_str(), a[1].2.as_str(), &a[1].3),
        (
            "zero\none\nTWO\nthree\nfour\n",
            merged,
            &replica("zero\none\nTWO\nthree\nfour\n")
        ),
        "row: after sharing, the save the doc doesn't hold merges onto the unwritten edit"
    );
    assert_eq!(
        (a[2].1.as_str(), a[2].2.as_str(), &a[2].3),
        (merged, merged, &replica(merged)),
        "row: after sharing, the write-back writes the merged text"
    );
    assert!(
        a[0].0.starts_with("Ingested") && a[1].0.starts_with("Ingested"),
        "control: both saves are taken in: {a:?}"
    );
    assert_eq!(a[2].0, "written true", "control: the write-back wrote");
}
