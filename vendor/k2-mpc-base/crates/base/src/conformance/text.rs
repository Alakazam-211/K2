//! Text conformance: K2's semantics for the `Text` primitive.
//!
//! Rules (ids are stable; they key `KNOWN_DEVIATIONS`):
//! - `text.converge`: replicas that saw the same edits hold the same text, in any delivery order.
//! - `text.no_interleave_forward`: two people typing forward at the same spot get two
//!   contiguous runs, never interleaved characters.
//! - `text.no_interleave_backward`: the same when typing backward (each char inserted
//!   before the previous one).
//! - `text.no_interleave_backward_multi`: backward typing by three writers who partly see
//!   each other's keystrokes. Some interleaving here is **unavoidable for every engine**
//!   (after the first sync, writer 1 types `e` before `c`, so every order has e < c < f).
//!   The rule asserts convergence and the order the reference engines agree on.
//! - `text.keep_concurrent_insert`: text inserted inside a range someone else deleted
//!   concurrently survives (no lost typing).
//! - `text.delete_once`: two concurrent deletes of one range delete it once, nothing more.
//! - `text.yata_separating_case`: the minimal script where YATA and FugueMax differ; a
//!   FugueMax engine keeps one writer's run whole (`[apfe]`). Yrs deviates (`[pfae]`).
//! - `text.yata_paper_a19`: the Fugue paper's A.1.9 example; FugueMax `xab`, Yrs `axb`.
//! - `text.edit_atomic`: a multi-hunk edit reaches other replicas as one change, never half.

use super::harness::{deviates, Net};
use crate::{Kind, Schema};

const S: Schema = Schema {
    name: "conformance-text",
    roots: &[("t", Kind::Text)],
};

fn contiguous(text: &str, run: &str) -> bool {
    text.contains(run)
}

#[test]
fn converge_under_any_delivery_order() {
    use rand::{rngs::SmallRng, Rng, SeedableRng};
    for seed in 0..60u64 {
        let mut rng = SmallRng::seed_from_u64(seed);
        let mut n = Net::new(S, 4, "seed line\nsecond line\n");
        for step in 0..120 {
            let i = rng.gen_range(0..4);
            let t = n.text(i);
            let at = (0..=t.len())
                .filter(|k| t.is_char_boundary(*k))
                .nth(rng.gen_range(0..=t.chars().count()))
                .unwrap_or(0);
            if rng.gen_bool(0.25) && t.len() > 4 {
                let mut end = (at + rng.gen_range(1..4)).min(t.len());
                while !t.is_char_boundary(end) {
                    end += 1;
                }
                if end > at {
                    n.delete(i, 100 + i as u64, at, end);
                }
            } else {
                n.insert(
                    i,
                    100 + i as u64,
                    at,
                    ["a", "é", "🎉", "\n", "word "][step % 5],
                );
            }
            if rng.gen_bool(0.3) {
                let (a, b) = (rng.gen_range(0..4), rng.gen_range(0..4));
                if a != b {
                    n.sync(a, b);
                }
            }
        }
        n.sync_all();
    }
}

#[test]
fn no_interleave_forward() {
    let mut n = Net::new(S, 2, "[]");
    // A types "abc" and B types "xyz", one keystroke at a time, at the same spot, concurrently.
    for (k, (a, b)) in ["a", "b", "c"].iter().zip(["x", "y", "z"]).enumerate() {
        n.insert(0, 101, 1 + k, a);
        n.insert(1, 102, 1 + k, b);
    }
    let t = n.sync_all();
    let ok = contiguous(&t, "abc") && contiguous(&t, "xyz");
    assert_eq!(ok, !deviates("text.no_interleave_forward"), "got {t:?}");
}

#[test]
fn no_interleave_backward() {
    let mut n = Net::new(S, 2, "[]");
    // Each keystroke goes *before* the previous one: typing "cba" backward reads "abc".
    for (a, b) in ["c", "b", "a"].iter().zip(["z", "y", "x"]) {
        n.insert(0, 101, 1, a);
        n.insert(1, 102, 1, b);
    }
    let t = n.sync_all();
    let ok = contiguous(&t, "abc") && contiguous(&t, "xyz");
    assert_eq!(ok, !deviates("text.no_interleave_backward"), "got {t:?}");
}

#[test]
fn keep_concurrent_insert_inside_deleted_range() {
    let mut n = Net::new(S, 2, "keep [delete this part] keep\n");
    let (s, e) = (5, "keep [delete this part]".len());
    n.delete(0, 101, s, e);
    n.insert(1, 102, "keep [delete ".len(), "TYPED ");
    let t = n.sync_all();
    assert_eq!(
        t.contains("TYPED "),
        !deviates("text.keep_concurrent_insert"),
        "got {t:?}"
    );
    assert!(
        !t.contains("delete this"),
        "the delete must still apply around the insert: {t:?}"
    );
}

#[test]
fn delete_once() {
    let mut n = Net::new(S, 3, "0123456789\n");
    n.delete(0, 101, 2, 6);
    n.delete(1, 102, 2, 6);
    n.delete(2, 103, 4, 8); // overlapping
    let t = n.sync_all();
    assert_eq!(t == "0189\n", !deviates("text.delete_once"), "got {t:?}");
}

#[test]
fn multi_hunk_edit_is_atomic() {
    use crate::text::diff;
    use crate::Edit;
    let mut n = Net::new(S, 2, "alpha\nbeta\ngamma\n");
    let cur = n.text(0);
    let new = "ALPHA\nbeta\nGAMMA\n";
    let hunks = diff::hunks(&cur, new);
    assert!(hunks.len() >= 2, "test needs a multi-hunk edit: {hunks:?}");
    n.r[0]
        .edit_as(
            101,
            &[Edit::Text {
                root: "t".into(),
                hunks,
            }],
        )
        .unwrap();
    // One sync delivers all of it, or (before) none of it.
    assert_eq!(n.text(1), "alpha\nbeta\ngamma\n");
    n.sync(0, 1);
    assert_eq!(n.text(1) == new, !deviates("text.edit_atomic"));
}

#[test]
#[allow(clippy::needless_range_loop)] // indices are the writers and keystrokes
fn no_interleave_backward_three_writers_with_partial_syncs() {
    // Three writers type backward at one spot; between keystrokes some of them see each
    // other's earlier keystrokes (the shape that defeats non-maximal orderings).
    let runs = [["c", "b", "a"], ["f", "e", "d"], ["z", "y", "x"]];
    let mut n = Net::new(S, 3, "[]");
    for k in 0..3 {
        for w in 0..3 {
            n.insert(w, 101 + w as u64, 1, runs[w][k]);
        }
        if k == 0 {
            n.sync(0, 1); // writer 1 sees writer 0's first keystroke
        }
        if k == 1 {
            n.sync(2, 0); // writer 0 sees writer 2's first two
        }
    }
    let t = n.sync_all();
    // Cross-checked 2026-10-27 on Yrs 0.28.0, Yjs 13.6.33 and 14.0.0-16, Diamond Types
    // 1.0.0 (FugueMax) and Loro 1.17.1: all give this order, identically at every step.
    // Contiguous `abc`/`def`/`xyz` is impossible on this script for any engine meeting the
    // strong list spec. The order depends on the ID tie-break (clients 101/102/103 here).
    // The native engine's Quint model must reproduce it (E-TEXT-1).
    assert_eq!(
        t, "[abdecfxyz]",
        "three-writer backward script: unexpected order"
    );
    assert!(!deviates("text.no_interleave_backward_multi"));
}

/// `text.yata_separating_case`: a script where YATA (Yrs/Yjs) and FugueMax really differ
/// (found by K2 Engine's differential search over 2 × 10^5 random scripts, 2026-10-27).
/// R0/R1/R2 are clients 102/103/101. R1 types `e`; R1 → R2; R0 types `a`; R2 types `p`;
/// R2 → R1 (R1 sees `[pe]`); R1 types `f` right before its own `e`; then a full sync.
/// FugueMax (Diamond Types) keeps R1's `fe` whole: `[apfe]`. Yrs and Yjs split it:
/// `[pfae]`. Every intermediate replica is identical in both; only the final merge differs.
#[test]
fn yata_separating_case() {
    let mut n = Net::new(S, 3, "[]");
    n.insert(1, 103, 1, "e");
    n.sync(1, 2);
    n.insert(0, 102, 1, "a");
    n.insert(2, 101, 1, "p");
    n.sync(2, 1);
    assert_eq!(n.text(1), "[pe]");
    n.insert(1, 103, 2, "f");
    assert_eq!(n.text(1), "[pfe]");
    let t = n.sync_all();
    if deviates("text.yata_separating_case") {
        assert_eq!(
            t, "[pfae]",
            "yrs's known YATA order changed: re-check the deviation"
        );
    } else {
        assert_eq!(t, "[apfe]", "FugueMax keeps the writer's run `fe` whole");
    }
}

/// `text.yata_paper_a19`: the Fugue paper's own example (arXiv 2305.00583v3, App. A.1.9).
/// Empty doc; R0/R1/R2 are clients 101/102/103 (the paper's 1/2/3, order-preserving).
/// R2 inserts `b`; R2 → R0; R0 inserts `a` at 0; R1 inserts `x` at 0; R1 → R0; full sync.
/// FugueMax (Diamond Types) and Loro give `xab`; Yrs and Yjs give `axb`, as the paper
/// reports for Yjs (cross-checked 2026-10-27 on k2n8).
#[test]
fn yata_paper_a19() {
    let mut n = Net::new(S, 3, "");
    n.insert(2, 103, 0, "b");
    n.sync(2, 0);
    n.insert(0, 101, 0, "a");
    n.insert(1, 102, 0, "x");
    n.sync(1, 0);
    let t = n.sync_all();
    if deviates("text.yata_paper_a19") {
        assert_eq!(
            t, "axb",
            "yrs's known YATA order changed: re-check the deviation"
        );
    } else {
        assert_eq!(t, "xab", "FugueMax orders the paper's A.1.9 example as xab");
    }
}
