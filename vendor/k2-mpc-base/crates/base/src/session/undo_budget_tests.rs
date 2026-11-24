//! The undo history at the size it holds (v0.24). Each kept batch is trimmed to what it holds,
//! and [`Config::undo_max_bytes`] bounds the bytes held (capacities and slots), not the texts'
//! lengths alone.
//!
//! Plant (`MPC_BETA_PLANT`): `undo-batch-unshrunk` (a batch kept as built) fails
//! `a_kept_batch_holds_only_what_it_needs` at its slot assert.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::*;
use crate::store::{DocKey, DocStore};

struct H;

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        1_000_000
    }
    fn on_event(&self, _: &AuditEvent) {}
}

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);

/// A session on a doc with no file, and one person in it.
fn open(tag: &str, c: Config) -> (std::path::PathBuf, SessionManager, Arc<Session>, ConnId) {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-undo-budget-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mgr = SessionManager::new(store, Arc::new(H), c);
    let k = DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    };
    let seed: String = (0..20)
        .map(|i| format!("line{i:02} {}\n", "x".repeat(100)))
        .collect();
    let s = mgr.open(Arc::new(MD), &k, Some(seed.as_bytes())).unwrap();
    let c = s
        .join(
            Participant::new("ann", "Ann", "#123456", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    (dir, mgr, s, c)
}

fn config() -> Config {
    Config {
        edit_rate_per_s: 1_000_000,
        edit_burst: 1_000_000,
        // every edit its own batch
        undo_group_idle_ms: 0,
        ..Config::default()
    }
}

/// Edit `i`: line `2i`'s word capitalised, the same size every time, with an untouched line
/// between any two edits (no edit carries another's region).
fn add(s: &Session, c: ConnId, i: u64) {
    let (from, to) = (format!("line{:02}", 2 * i), format!("LINE{:02}", 2 * i));
    s.edit_with(c, TextFilePack::ROOT, |t| {
        Ok::<_, ()>(t.replacen(&from, &to, 1))
    })
    .unwrap()
    .expect("the edit changes the text");
}

/// Every kept batch holds its regions exactly (no spare slots: a list built by pushes keeps
/// four for one), and its texts and hunk lists at their lengths.
#[test]
fn a_kept_batch_holds_only_what_it_needs() {
    let (dir, mgr, s, c) = open("slots", config());
    for i in 0..5 {
        add(&s, c, i);
    }
    let slots = s.undo_slots_for_tests("ann").unwrap();
    assert_eq!(slots.len(), 5, "control: five batches kept");
    for (k, (regions, cap, _, trimmed)) in slots.iter().enumerate() {
        assert_eq!(
            (regions, cap),
            (&1, &1),
            "row: batch {k} holds its one region in one slot"
        );
        assert!(trimmed, "row: batch {k}'s texts and hunk lists are trimmed");
    }
    mgr.release().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The budget bounds the bytes held: the history's count is the sum of its batches' (each at
/// least its slots and its texts' capacities); with the budget at four and a half batches,
/// four are kept and the oldest go first, so undoing everything kept leaves the text as the
/// fourth-newest edit found it.
#[test]
fn the_undo_budget_bounds_the_bytes_held_oldest_first() {
    // one batch's bytes, from a probe with no byte bound
    let (dir, mgr, s, c) = open("probe", config());
    add(&s, c, 0);
    add(&s, c, 1);
    let one = s.undo_slots_for_tests("ann").unwrap()[1].2;
    assert_eq!(
        s.undo_bytes_for_tests("ann").unwrap(),
        s.undo_slots_for_tests("ann")
            .unwrap()
            .iter()
            .map(|b| b.2)
            .sum::<usize>(),
        "row: the history counts its batches' bytes"
    );
    assert!(
        one > std::mem::size_of::<super::undo::Batch>()
            + std::mem::size_of::<super::undo::Region>(),
        "row: a batch counts its slots and what its region holds: {one}"
    );
    mgr.release().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    // the bound at four and a half batches
    let (dir, mgr, s, c) = open(
        "bound",
        Config {
            undo_max_bytes: one * 9 / 2,
            ..config()
        },
    );
    let mut after = vec![s.read(c).unwrap().text_by_root[0].1.clone()];
    for i in 0..8 {
        add(&s, c, i);
        after.push(s.read(c).unwrap().text_by_root[0].1.clone());
    }
    let held = s.undo_bytes_for_tests("ann").unwrap();
    assert!(
        held <= one * 9 / 2,
        "row: the history holds at most the bound: {held}"
    );
    let kept = s.undo_slots_for_tests("ann").unwrap().len();
    assert_eq!(kept, 4, "row: four batches fit the bound");
    match s.undo(c, 100).unwrap() {
        UndoOutcome::Applied { steps, .. } => assert_eq!(steps, 4, "row: four steps undone"),
        o => panic!("row: the kept batches undo: {o:?}"),
    }
    assert_eq!(
        s.read(c).unwrap().text_by_root[0].1,
        after[8 - 4],
        "row: the oldest batches went first: undoing every kept one leaves edit 4's text"
    );
    mgr.release().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
