//! Text conformance: the kit's semantics for the `Text` primitive, generic over engines.
//!
//! Rules (ids are stable; they key each engine's `KNOWN_DEVIATIONS`):
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
//! - `text.edit_atomic`: a multi-hunk edit reaches other replicas as one change, never half
//!   (split delivery is `text.edit_atomic_split`, over [`crate::caps::Events`]).
//! - `text.scalar_safe` (E-TEXT-9): the unit is the Unicode scalar value; an offset that
//!   isn't a scalar boundary (or is out of range) is refused with nothing changed, and no
//!   edit, local or merged, ever splits a scalar.
//!
//! Each rule is a function generic over the engine. It panics when the engine breaks the
//! rule (or, for a listed deviation, when the deviation stops happening), and returns the
//! final replicas so an engine with extra checkers can run them ([`crate::model::check_lists`]).
//! Writers are agents 101, 102, 103 (the seed is [`crate::SEED_AGENT`]); tie-breaks
//! depend on that numeric order.

use k2_mpc_base::text::diff;

use crate::{deviates, Hunk, Net, TextReplica};

/// Every text rule, by id. [`run`] runs any of them.
pub const RULES: &[&str] = &[
    "text.converge",
    "text.no_interleave_forward",
    "text.no_interleave_backward",
    "text.no_interleave_backward_multi",
    "text.keep_concurrent_insert",
    "text.delete_once",
    "text.edit_atomic",
    "text.yata_separating_case",
    "text.yata_paper_a19",
    "text.scalar_safe",
];

/// Run rule `id` on engine `E`; returns the final replicas. An unknown id panics.
pub fn run<E: TextReplica>(id: &str) -> Net<E> {
    match id {
        "text.converge" => converge::<E>(),
        "text.no_interleave_forward" => no_interleave_forward::<E>(),
        "text.no_interleave_backward" => no_interleave_backward::<E>(),
        "text.no_interleave_backward_multi" => no_interleave_backward_multi::<E>(),
        "text.keep_concurrent_insert" => keep_concurrent_insert::<E>(),
        "text.delete_once" => delete_once::<E>(),
        "text.edit_atomic" => edit_atomic::<E>(),
        "text.yata_separating_case" => yata_separating_case::<E>(),
        "text.yata_paper_a19" => yata_paper_a19::<E>(),
        "text.scalar_safe" => scalar_safe::<E>(),
        _ => panic!("no text rule {id:?}"),
    }
}

/// `text.converge`: 60 seeded random scripts, 4 writers, partial syncs, then a full sync.
/// Returns the last script's replicas.
pub fn converge<E: TextReplica>() -> Net<E> {
    use rand::{rngs::SmallRng, Rng, SeedableRng};
    let mut last = None;
    for seed in 0..60u64 {
        let mut rng = SmallRng::seed_from_u64(seed);
        let mut n = Net::<E>::new(&[100, 101, 102, 103], "seed line\nsecond line\n");
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
                    n.delete(i, at, end);
                }
            } else {
                n.insert(i, at, ["a", "é", "🎉", "\n", "word "][step % 5]);
            }
            if rng.gen_bool(0.3) {
                let (a, b) = (rng.gen_range(0..4), rng.gen_range(0..4));
                if a != b {
                    n.sync(a, b);
                }
            }
        }
        n.sync_all();
        last = Some(n);
    }
    last.expect("at least one script")
}

/// `text.no_interleave_forward`
pub fn no_interleave_forward<E: TextReplica>() -> Net<E> {
    let mut n = Net::<E>::new(&[101, 102], "[]");
    // A types "abc" and B types "xyz", one keystroke at a time, at the same spot, concurrently.
    for (k, (a, b)) in ["a", "b", "c"].iter().zip(["x", "y", "z"]).enumerate() {
        n.insert(0, 1 + k, a);
        n.insert(1, 1 + k, b);
    }
    let t = n.sync_all();
    let ok = t.contains("abc") && t.contains("xyz");
    assert_eq!(
        ok,
        !deviates::<E>("text.no_interleave_forward"),
        "{}: got {t:?}",
        E::ENGINE
    );
    n
}

/// `text.no_interleave_backward`
pub fn no_interleave_backward<E: TextReplica>() -> Net<E> {
    let mut n = Net::<E>::new(&[101, 102], "[]");
    // Each keystroke goes *before* the previous one: typing "cba" backward reads "abc".
    for (a, b) in ["c", "b", "a"].iter().zip(["z", "y", "x"]) {
        n.insert(0, 1, a);
        n.insert(1, 1, b);
    }
    let t = n.sync_all();
    let ok = t.contains("abc") && t.contains("xyz");
    assert_eq!(
        ok,
        !deviates::<E>("text.no_interleave_backward"),
        "{}: got {t:?}",
        E::ENGINE
    );
    n
}

/// `text.keep_concurrent_insert`
pub fn keep_concurrent_insert<E: TextReplica>() -> Net<E> {
    let mut n = Net::<E>::new(&[101, 102], "keep [delete this part] keep\n");
    let (s, e) = (5, "keep [delete this part]".len());
    n.delete(0, s, e);
    n.insert(1, "keep [delete ".len(), "TYPED ");
    let t = n.sync_all();
    assert_eq!(
        t.contains("TYPED "),
        !deviates::<E>("text.keep_concurrent_insert"),
        "{}: got {t:?}",
        E::ENGINE
    );
    assert!(
        !t.contains("delete this"),
        "{}: the delete must still apply around the insert: {t:?}",
        E::ENGINE
    );
    n
}

/// `text.delete_once`
pub fn delete_once<E: TextReplica>() -> Net<E> {
    let mut n = Net::<E>::new(&[101, 102, 103], "0123456789\n");
    n.delete(0, 2, 6);
    n.delete(1, 2, 6);
    n.delete(2, 4, 8); // overlapping
    let t = n.sync_all();
    assert_eq!(
        t == "0189\n",
        !deviates::<E>("text.delete_once"),
        "{}: got {t:?}",
        E::ENGINE
    );
    n
}

/// `text.edit_atomic`
pub fn edit_atomic<E: TextReplica>() -> Net<E> {
    let mut n = Net::<E>::new(&[101, 102], "alpha\nbeta\ngamma\n");
    let cur = n.text(0);
    let new = "ALPHA\nbeta\nGAMMA\n";
    let hunks = diff::hunks(&cur, new);
    assert!(hunks.len() >= 2, "test needs a multi-hunk edit: {hunks:?}");
    n.edit(0, hunks);
    assert_eq!(n.text(0), new, "{}: the batch applied locally", E::ENGINE);
    // One sync delivers all of it, or (before) none of it.
    assert_eq!(n.text(1), "alpha\nbeta\ngamma\n");
    n.sync(0, 1);
    assert_eq!(
        n.text(1) == new,
        !deviates::<E>("text.edit_atomic"),
        "{}: got {:?}",
        E::ENGINE,
        n.text(1)
    );
    n
}

/// `text.no_interleave_backward_multi`
#[allow(clippy::needless_range_loop)] // indices are the writers and keystrokes
pub fn no_interleave_backward_multi<E: TextReplica>() -> Net<E> {
    // Three writers type backward at one spot; between keystrokes some of them see each
    // other's earlier keystrokes (the shape that defeats non-maximal orderings).
    let runs = [["c", "b", "a"], ["f", "e", "d"], ["z", "y", "x"]];
    let mut n = Net::<E>::new(&[101, 102, 103], "[]");
    for k in 0..3 {
        for w in 0..3 {
            n.insert(w, 1, runs[w][k]);
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
    // strong list spec. The order depends on the ID tie-break (agents 101/102/103 here).
    // The reference model (FugueMax) must give it too.
    assert_eq!(
        t,
        "[abdecfxyz]",
        "{}: three-writer backward script: unexpected order",
        E::ENGINE
    );
    assert!(!deviates::<E>("text.no_interleave_backward_multi"));
    n
}

/// `text.yata_separating_case`: a script where YATA (Yrs/Yjs) and FugueMax really differ
/// (found by a differential search over 2 × 10^5 random scripts, 2026-10-27).
/// R0/R1/R2 are agents 102/103/101. R1 types `e`; R1 → R2; R0 types `a`; R2 types `p`;
/// R2 → R1 (R1 sees `[pe]`); R1 types `f` right before its own `e`; then a full sync.
/// FugueMax (Diamond Types, the reference model) keeps R1's `fe` whole: `[apfe]`. Yrs and
/// Yjs split it: `[pfae]`. Every intermediate replica is identical in both; only the final
/// merge differs. A deviating engine is expected to be YATA-class: its order is pinned.
pub fn yata_separating_case<E: TextReplica>() -> Net<E> {
    let mut n = Net::<E>::new(&[102, 103, 101], "[]");
    n.insert(1, 1, "e");
    n.sync(1, 2);
    n.insert(0, 1, "a");
    n.insert(2, 1, "p");
    n.sync(2, 1);
    assert_eq!(n.text(1), "[pe]", "{}", E::ENGINE);
    n.insert(1, 2, "f");
    assert_eq!(n.text(1), "[pfe]", "{}", E::ENGINE);
    let t = n.sync_all();
    if deviates::<E>("text.yata_separating_case") {
        assert_eq!(
            t,
            "[pfae]",
            "{}: the known YATA order changed: re-check the deviation",
            E::ENGINE
        );
    } else {
        assert_eq!(
            t,
            "[apfe]",
            "{}: FugueMax keeps the writer's run `fe` whole",
            E::ENGINE
        );
    }
    n
}

/// `text.yata_paper_a19`: the Fugue paper's own example (arXiv 2305.00583v3, App. A.1.9).
/// Empty doc; R0/R1/R2 are agents 101/102/103 (the paper's 1/2/3, order-preserving).
/// R2 inserts `b`; R2 → R0; R0 inserts `a` at 0; R1 inserts `x` at 0; R1 → R0; full sync.
/// FugueMax (Diamond Types, the reference model) and Loro give `xab`; Yrs and Yjs give
/// `axb`, as the paper reports for Yjs. A deviating engine's order is pinned.
pub fn yata_paper_a19<E: TextReplica>() -> Net<E> {
    let mut n = Net::<E>::new(&[101, 102, 103], "");
    n.insert(2, 0, "b");
    n.sync(2, 0);
    n.insert(0, 0, "a");
    n.insert(1, 0, "x");
    n.sync(1, 0);
    let t = n.sync_all();
    if deviates::<E>("text.yata_paper_a19") {
        assert_eq!(
            t,
            "axb",
            "{}: the known YATA order changed: re-check the deviation",
            E::ENGINE
        );
    } else {
        assert_eq!(
            t,
            "xab",
            "{}: FugueMax orders the paper's A.1.9 example as xab",
            E::ENGINE
        );
    }
    n
}

/// `text.scalar_safe` (E-TEXT-9).
pub fn scalar_safe<E: TextReplica>() -> Net<E> {
    use rand::{rngs::SmallRng, Rng, SeedableRng};

    // 1. Every hunk with an end off a scalar boundary or out of range is refused, alone or
    //    next to a valid hunk, and nothing changes. 1-, 2-, 3- and 4-byte scalars.
    let seed = "a\u{e9}\u{4e2d}\u{1f389}\n";
    let len = seed.len();
    let bad = |k: usize| k > len || !seed.is_char_boundary(k);
    let mut r = E::replica(101, seed);
    let mut refused = 0;
    for start in 0..=len + 1 {
        for end in start..=len + 1 {
            if !(bad(start) || bad(end)) {
                continue;
            }
            for insert in ["", "x", "\u{1f44d}"] {
                if start == end && insert.is_empty() {
                    continue;
                }
                let h = Hunk {
                    start,
                    end,
                    insert: insert.into(),
                };
                // alone, and after a valid hunk (the whole batch is refused)
                for batch in [vec![h.clone()], vec![crate::ins(0, "ok"), h]] {
                    assert!(
                        r.apply(&batch).is_err(),
                        "{}: accepted {batch:?} on {seed:?}",
                        E::ENGINE
                    );
                    assert_eq!(
                        r.text(),
                        seed,
                        "{}: a refused batch changed text",
                        E::ENGINE
                    );
                    refused += 1;
                }
            }
        }
    }
    assert!(refused > 100, "the rule must try many offsets: {refused}");

    // 2. Concurrent edits right next to multi-byte scalars, merged: none is split.
    let mut n = Net::<E>::new(&[101, 102, 103], "\u{1f389}\u{e9}\u{4e2d}\u{1f389}\n");
    n.delete(0, 0, 4); // the first 4-byte scalar
    n.edit(1, vec![crate::ins(4, "x"), crate::ins(6, "y")]); // around the 2-byte one
    n.edit(2, vec![crate::rep(9, 13, "\u{1f44d}")]); // replace the last 4-byte one
    let t = n.sync_all();
    assert_eq!(t, "x\u{e9}y\u{4e2d}\u{1f44d}\n", "{}", E::ENGINE);

    // 3. Random scripts of multi-byte inserts and deletes at scalar boundaries (a skin-tone
    //    emoji is two scalars: deleting one of them is allowed, splitting either is not).
    //    The merged text holds only whole inserted scalars.
    let pieces = [
        "\u{1f389}",
        "\u{e9}",
        "\u{4e2d}",
        "a",
        "\u{1f44d}\u{1f3fd}",
        "e\u{301}",
    ];
    let allowed: Vec<char> = pieces.iter().flat_map(|p| p.chars()).collect();
    for s in 0..20u64 {
        let mut rng = SmallRng::seed_from_u64(1000 + s);
        let mut n = Net::<E>::new(&[101, 102, 103], "");
        for step in 0..60 {
            let i = rng.gen_range(0..3);
            let t = n.text(i);
            let bounds: Vec<usize> = (0..=t.len()).filter(|k| t.is_char_boundary(*k)).collect();
            let at = bounds[rng.gen_range(0..bounds.len())];
            if rng.gen_bool(0.3) && at < t.len() {
                let end = bounds[bounds.iter().position(|&b| b == at).unwrap() + 1];
                n.delete(i, at, end);
            } else {
                n.insert(i, at, pieces[step % pieces.len()]);
            }
            if rng.gen_bool(0.3) {
                let (a, b) = (rng.gen_range(0..3), rng.gen_range(0..3));
                if a != b {
                    n.sync(a, b);
                }
            }
        }
        let t = n.sync_all();
        assert!(
            t.chars().all(|c| allowed.contains(&c)),
            "{}: script {s}: a character no one typed: {t:?}",
            E::ENGINE
        );
    }
    n
}
