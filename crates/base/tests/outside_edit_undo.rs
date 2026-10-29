//! Undo after an outside edit that repeats the undone text (v0.9.3, the one-writer soak's
//! finding).
//!
//! An outside edit is made on the file's last synced state and merged into the live doc by
//! the engine, so where its elements land among the live doc's is the engine's choice. The
//! undo spec (SPEC-UNDO, preamble) wants every applied change reported **at element-identity
//! positions**, so that repeated text can't hide a wrong position: regions are carried
//! through those hunks. A diff of the live text before and after the merge can't do that:
//! when the merge puts a copy of a participant's line just before it, the diff reports the
//! insertion after it, and the participant's region ends up on the copy.
//!
//! The shape: the file holds `X\nA\nA\n`; agent a0 types `B\n` between the `A`s (live
//! `X\nA\nB\nA\n`, not yet written back); the file is rewritten as `X\nA\nB\nA\nA\n` (from
//! its synced state: `B\nA\n` between the `A`s), and the merge puts that before or after
//! a0's `B\n`, by client order. With the copy first (`X A [B A] B A`), a diff of the live
//! text reports `B\nA\n` inserted after a0's line and leaves a0's region on the copy's `B`.
//! a0's undo must remove **a0's** `B\n` and leave the outside edit's copy, whichever order the
//! merge chose: live `X\nA\nB\nA\nA\n`, the `B` by the outside edit.
//!
//! Since v0.9.5 an outside save is a three-way merge of texts onto the live doc, not a CRDT
//! merge, so the order no longer depends on client IDs: two insertions at one point keep the
//! live one first (`X A B [B A] A`) for every seed. The run checks that, and the undo as
//! before.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::differential::{with_side, Path as EditPath, Side};
use k2_mpc_base::session::{
    AuditEvent, Config, Host, Participant, ParticipantKind, Rights, SessionManager, Stay,
    TextFilePack, UndoOutcome, EXTERNAL_PID,
};
use k2_mpc_base::store::{DocKey, DocStore};

struct H(AtomicU64);

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
const ROOT: &str = TextFilePack::ROOT;

/// One run with client IDs from `seed`: (whether the outside copy landed before a0's line,
/// the undo's outcome, the text after, the authors of its lines after).
fn run(seed: u64) -> (bool, UndoOutcome, String, Vec<String>) {
    let dir = std::env::temp_dir().join(format!("k2mb-outside-undo-{}-{seed}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let file = dir.join("ws").join("a.md");
    std::fs::write(&file, "X\nA\nA\n").unwrap();
    let mut side = Side::new(EditPath::MainDoc).with_client_seed(seed);
    let out = with_side(&mut side, || {
        let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
        let mgr = SessionManager::new(store, Arc::new(H(AtomicU64::new(1_000))), Config::default());
        let k = DocKey {
            space: "ws".into(),
            path: "a.md".into(),
        };
        let s = mgr
            .open_file(Arc::new(MD), &k, &file, &dir.join("ws"))
            .unwrap();
        let a0 = s
            .join(
                Participant::new("a0", "a0", "#123456", ParticipantKind::Agent, "cli"),
                Stay::Held,
            )
            .unwrap()
            .conn;
        s.replace_text(a0, ROOT, "X\nA\nB\nA\n").unwrap();
        // the file, from its synced state `X A A` (no write-back yet): `B\nA\n` between the
        // two `A`s, where a0 typed its `B`
        std::fs::write(&file, "X\nA\nB\nA\nA\n").unwrap();
        s.check_disk().unwrap();
        let authors = |s: &k2_mpc_base::Session| -> Vec<String> {
            s.blame(a0, ROOT, None)
                .unwrap()
                .lines
                .iter()
                .map(|l| l.author.clone())
                .collect()
        };
        let before = authors(&s);
        let merged = s.read(a0).unwrap().text_by_root[0].1.clone();
        // the outside copy first (`X A [B A] B A`: line 3 is the outside edit's, line 5
        // a0's) or after a0's line (`X A B [B A] A`)
        let copy_first = match merged.as_str() {
            "X\nA\nB\nA\nB\nA\n" => true,
            "X\nA\nB\nB\nA\nA\n" => false,
            other => panic!("seed {seed}: an unexpected merge {other:?}"),
        };
        assert_eq!(
            before[2] == EXTERNAL_PID,
            copy_first,
            "seed {seed}: {before:?}"
        );
        let undone = s.undo(a0, 1).unwrap();
        let text = s.read(a0).unwrap().text_by_root[0].1.clone();
        let after = authors(&s);
        mgr.release().unwrap();
        (copy_first, undone, text, after)
    });
    let _ = std::fs::remove_dir_all(&dir);
    out
}

#[test]
fn undo_after_an_outside_edit_repeating_the_line_removes_the_agents_own() {
    let mut orders = [0u32; 2];
    for seed in 0..24 {
        let (copy_first, undone, text, authors) = run(seed);
        orders[usize::from(copy_first)] += 1;
        assert!(
            matches!(undone, UndoOutcome::Applied { .. }),
            "seed {seed} (outside copy first: {copy_first}): a0's undo must apply, got {undone:?}"
        );
        assert_eq!(text, "X\nA\nB\nA\nA\n", "seed {seed}");
        assert_eq!(
            authors[2], EXTERNAL_PID,
            "seed {seed}: the B left is the outside edit's, not a0's: {authors:?}"
        );
    }
    // v0.9.5: one order whatever the client IDs, the live line first
    assert_eq!(orders, [24, 0], "orders seen");
}
