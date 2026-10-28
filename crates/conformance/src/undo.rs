//! Rules over [`Undo`].
//!
//! - `hist.undo_atomic` (E-HIST-6, K-CT-4; undo spec U13, U17): applying an undo or redo is
//!   **one atomic engine batch across all its roots, attributed to the undoer**, and
//!   `--steps N` is one log append; all or nothing (a conflicting step applies nothing).
//! - `hist.undo_partial` (K-HIST-13; undo spec U8, U11, U14–U15): a region is one hunk of
//!   the batch; `--partial` applies every region that turns back cleanly and skips exactly
//!   the set all-or-nothing `undo` refuses on (across roots, as one batch); a step whose
//!   every region conflicts is not applied and stays on top until `--drop` removes it (and
//!   a dropped step is not redone); with `--steps N` it stops at a blocked step.
//! - `hist.redo` (E-HIST-6, K-HIST-2; U7, U17): redo reapplies exactly the regions the undo
//!   applied (not the skipped ones), as one batch by the redoer; `--steps 2` redoes in
//!   order; another participant's batch leaves your redo stack alone; your own new batch
//!   clears it.
//! - `hist.undo_own_only` (undo spec U2, U10, U10a by root (v1.8), U18a, U23), per
//!   **element**: an undo, redo or revert by an agent removes only elements whose
//!   **provenance root** its reversed change inserted: here, every element it deletes has a
//!   root that agent inserted, or is one that agent itself put back (a redo removing what
//!   its undo restored); every element it inserts is new and attributed to it. Repeated
//!   text is where a re-diff of the window would pick someone else's identical characters
//!   (two identical appended lines; a run `aaaa` grown by two agents): the text would look
//!   right and the elements would be wrong. An insertion at a region's start repeating its
//!   text (finding F3): undo, redo and revert merge exact hunk sets (undo spec v1.7, U10),
//!   never diffs of strings. And text read **by root** (v1.8): a change undone and redone
//!   has its roots live again, so its revert applies (regression (a)); when another agent
//!   edits inside a region and undoes it, what it put back has the region's roots, so the
//!   region's own undo applies (regression (b)).

use std::collections::BTreeSet;

use crate::caps::{Applied, Change, ElemBlame, Region, Undo, UndoRefusal};
use crate::{deviates, ins, rep, Hunk};

/// Every rule over [`Undo`], by id.
pub const RULES: &[&str] = &[
    "hist.undo_atomic",
    "hist.undo_partial",
    "hist.redo",
    "hist.undo_own_only",
];

/// Run rule `id` on engine `U`. An unknown id panics.
pub fn run<U: Undo>(id: &str) {
    match id {
        "hist.undo_atomic" => undo_atomic::<U>(),
        "hist.undo_partial" => undo_partial::<U>(),
        "hist.redo" => redo::<U>(),
        "hist.undo_own_only" => undo_own_only::<U>(),
        _ => panic!("no undo rule {id:?}"),
    }
}

const A: u64 = 101;
const B: u64 = 102;

fn engine<U: Undo>() -> &'static str {
    <U::Engine as crate::TextReplica>::ENGINE
}

fn edit<U: Undo>(d: &mut U, agent: u64, edits: &[(usize, Vec<Hunk>)]) {
    if let Err(e) = d.edit(agent, edits) {
        panic!("{}: {agent} refused {edits:?}: {e:?}", engine::<U>());
    }
}

/// `old` → `new` in root `root`'s current text (first occurrence).
fn swap<U: Undo>(d: &U, root: usize, old: &str, new: &str) -> (usize, Vec<Hunk>) {
    let t = &d.texts()[root];
    let at = t
        .find(old)
        .unwrap_or_else(|| panic!("{old:?} not in root {root}: {t:?}"));
    (root, vec![rep(at, at + old.len(), new)])
}

fn one_change(author: u64, roots: &[usize]) -> Vec<Change> {
    vec![Change {
        author,
        roots: roots.to_vec(),
    }]
}

/// `hist.undo_atomic`
pub fn undo_atomic<U: Undo>() {
    let seeds = ["r0 one\nr0 two\n", "r1 one\nr1 two\n"];
    let mut d = U::open(seeds);
    let e = engine::<U>();

    // 1. one step over both roots: one change, both roots, by the undoer; redo the same
    let e1 = [swap(&d, 0, "one", "ONE"), swap(&d, 1, "two", "TWO")];
    edit(&mut d, A, &e1);
    let end = d.texts()[0].len();
    edit(&mut d, B, &[(0, vec![ins(end, "b\n")])]);
    let done = d.texts();
    assert_eq!(done, ["r0 ONE\nr0 two\nb\n", "r1 one\nr1 TWO\n"], "{e}");
    let u = d
        .undo(A, 1)
        .unwrap_or_else(|r| panic!("{e}: undo refused: {r:?}"));
    assert_eq!(
        u,
        Applied {
            steps: 1,
            changes: one_change(A, &[0, 1])
        },
        "{e}: an undo over two roots"
    );
    assert_eq!(
        d.texts(),
        ["r0 one\nr0 two\nb\n", "r1 one\nr1 two\n"],
        "{e}"
    );
    let r = d
        .redo(A, 1)
        .unwrap_or_else(|r| panic!("{e}: redo refused: {r:?}"));
    assert_eq!(
        r,
        Applied {
            steps: 1,
            changes: one_change(A, &[0, 1])
        },
        "{e}: a redo over two roots"
    );
    assert_eq!(d.texts(), done, "{e}");
    // b's undo is b's: attributed to b, a's change stays
    let u = d
        .undo(B, 1)
        .unwrap_or_else(|r| panic!("{e}: b's undo refused: {r:?}"));
    assert_eq!(u.changes, one_change(B, &[0]), "{e}");
    assert_eq!(d.texts(), ["r0 ONE\nr0 two\n", "r1 one\nr1 TWO\n"], "{e}");

    // 2. `--steps 2`: two batches (one per root) undone as one log append
    let ed = [swap(&d, 0, "two", "2")];
    edit(&mut d, A, &ed);
    let ed = [swap(&d, 1, "one", "1")];
    edit(&mut d, A, &ed);
    let before = d.texts();
    assert_eq!(before, ["r0 ONE\nr0 2\n", "r1 1\nr1 TWO\n"], "{e}");
    let u = d
        .undo(A, 2)
        .unwrap_or_else(|r| panic!("{e}: undo 2 refused: {r:?}"));
    assert_eq!(u.steps, 2, "{e}");
    assert_eq!(d.texts(), ["r0 ONE\nr0 two\n", "r1 one\nr1 TWO\n"], "{e}");
    let r = d
        .redo(A, 2)
        .unwrap_or_else(|r| panic!("{e}: redo 2 refused: {r:?}"));
    assert_eq!(r.steps, 2, "{e}");
    assert_eq!(d.texts(), before, "{e}");
    for (what, got) in [("undo", &u), ("redo", &r)] {
        assert!(
            got.changes.iter().all(|c| c.author == A),
            "{e}: {what} --steps 2 not attributed to the undoer: {got:?}"
        );
        let roots: Vec<usize> = {
            let mut v: Vec<usize> = got.changes.iter().flat_map(|c| c.roots.clone()).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        assert_eq!(roots, [0, 1], "{e}: {what} --steps 2 roots: {got:?}");
        if deviates::<U::Engine>("hist.undo_atomic") {
            // the pinned behaviour: one change per step, each on its own root
            assert!(
                got.changes.len() == 2 && got.changes.iter().all(|c| c.roots.len() == 1),
                "{e}: {what} --steps 2: the known deviation changed ({got:?}): re-check it"
            );
        } else {
            assert_eq!(
                got.changes,
                one_change(A, &[0, 1]),
                "{e}: {what} --steps 2 must be one change"
            );
        }
    }

    // 3. all or nothing: the older step conflicts, so the newer one isn't applied either
    let ed = [swap(&d, 0, "ONE", "uno")];
    edit(&mut d, A, &ed);
    let ed = [swap(&d, 1, "TWO", "dos")];
    edit(&mut d, A, &ed);
    let ed = [swap(&d, 0, "uno", "UNO")];
    edit(&mut d, B, &ed);
    let before = d.texts();
    let r = d.undo(A, 2);
    assert!(
        r.is_err(),
        "{e}: a conflicting step must refuse the undo: {r:?}"
    );
    assert_eq!(d.texts(), before, "{e}: a refused undo changed the doc");
    // the newer step alone is fine
    let u = d
        .undo(A, 1)
        .unwrap_or_else(|r| panic!("{e}: undo 1 refused: {r:?}"));
    assert_eq!(u.changes, one_change(A, &[1]), "{e}");
    assert_eq!(d.texts(), ["r0 UNO\nr0 2\n", "r1 1\nr1 TWO\n"], "{e}");
}

/// a changes root 0's lines 1 and 5 and root 1's line in one batch (three regions); b then
/// edits inside root 0's line 5.
fn three_regions<U: Undo>() -> U {
    let mut d = U::open(["one\ntwo\nthree\nfour\nfive\n", "r1 x\n"]);
    edit(
        &mut d,
        A,
        &[
            (0, vec![rep(0, 3, "ONE"), rep(19, 23, "FIVE")]),
            (1, vec![rep(3, 4, "X")]),
        ],
    );
    let ed = [swap(&d, 0, "FIVE", "FIxVE")];
    edit(&mut d, B, &ed);
    assert_eq!(
        d.texts(),
        ["ONE\ntwo\nthree\nfour\nFIxVE\n", "r1 X\n"],
        "{}",
        engine::<U>()
    );
    d
}

/// `hist.undo_partial`
pub fn undo_partial<U: Undo>() {
    let e = engine::<U>();
    let mut d = three_regions::<U>();
    let before = d.texts();
    // all or nothing: refused, exactly the conflicting region reported, nothing applied
    let conflicts = match d.undo(A, 1) {
        Err(UndoRefusal::Conflicts { step: 1, regions }) => regions,
        other => panic!("{e}: a conflicting undo: {other:?}"),
    };
    assert_eq!(
        conflicts,
        [Region {
            root: 0,
            range: 19..24,
            ours: "five".into(),
            theirs: "FIxVE".into(),
        }],
        "{e}: the conflict report"
    );
    assert_eq!(d.texts(), before, "{e}: a refused undo changed the doc");
    // --partial: applies the clean regions (both roots, one batch), skips exactly that set
    let p = d
        .undo_partial(A, 1)
        .unwrap_or_else(|r| panic!("{e}: undo --partial refused: {r:?}"));
    assert_eq!(p.skipped, conflicts, "{e}: --partial skipped another set");
    assert_eq!(p.blocked, None, "{e}");
    assert_eq!(
        p.applied,
        Applied {
            steps: 1,
            changes: one_change(A, &[0, 1])
        },
        "{e}: a partial undo is one batch by the undoer"
    );
    assert_eq!(
        d.texts(),
        ["one\ntwo\nthree\nfour\nFIxVE\n", "r1 x\n"],
        "{e}"
    );

    // a step whose every region conflicts stays on top; --drop removes it
    let mut d = U::open(["alpha\nbeta\n", "r1\n"]);
    let ed = [swap(&d, 0, "alpha", "ALPHA")];
    edit(&mut d, A, &ed);
    let ed = [swap(&d, 0, "beta", "BETA")];
    edit(&mut d, A, &ed);
    let ed = [swap(&d, 0, "BETA", "BExTA")];
    edit(&mut d, B, &ed);
    for _ in 0..2 {
        match d.undo_partial(A, 1) {
            Err(UndoRefusal::Blocked(r)) => {
                assert_eq!(r.len(), 1, "{e}: {r:?}");
                assert_eq!(r[0].theirs, "BExTA", "{e}");
            }
            other => panic!("{e}: a blocked step: {other:?}"),
        }
        assert_eq!(d.texts()[0], "ALPHA\nBExTA\n", "{e}");
    }
    d.drop_top(A)
        .unwrap_or_else(|r| panic!("{e}: --drop refused: {r:?}"));
    let u = d
        .undo(A, 1)
        .unwrap_or_else(|r| panic!("{e}: the older step: {r:?}"));
    assert_eq!(u.steps, 1, "{e}");
    assert_eq!(d.texts()[0], "alpha\nBExTA\n", "{e}");
    // the dropped step isn't on the redo stack: redo gives back only the alpha change
    let r = d.redo(A, 5).unwrap_or_else(|r| panic!("{e}: redo: {r:?}"));
    assert_eq!(r.steps, 1, "{e}: a dropped step was redone");
    assert_eq!(d.texts()[0], "ALPHA\nBExTA\n", "{e}");

    // --steps 3 --partial, newest first: c undoes, the b step is blocked and stays
    let mut d = U::open(["a\nb\nc\n", "r1\n"]);
    for (who, old, new) in [
        (A, "a\n", "A\n"),
        (A, "b\n", "B\n"),
        (B, "B", "Z"),
        (A, "c\n", "C\n"),
    ] {
        let ed = [swap(&d, 0, old, new)];
        edit(&mut d, who, &ed);
    }
    let p = d
        .undo_partial(A, 3)
        .unwrap_or_else(|r| panic!("{e}: undo --steps 3 --partial: {r:?}"));
    assert_eq!(p.applied.steps, 1, "{e}");
    assert_eq!(p.blocked.as_ref().map(Vec::len), Some(1), "{e}: {p:?}");
    assert_eq!(d.texts()[0], "A\nZ\nc\n", "{e}");
    // all or nothing with 3 steps refuses at step 1 (now the blocked one)
    assert!(
        matches!(d.undo(A, 3), Err(UndoRefusal::Conflicts { step: 1, .. })),
        "{e}"
    );
    assert_eq!(d.texts()[0], "A\nZ\nc\n", "{e}");
}

/// `hist.redo`
pub fn redo<U: Undo>() {
    let e = engine::<U>();
    // redo after a partial undo reapplies only what the undo applied
    let mut d = three_regions::<U>();
    let done = d.texts();
    d.undo_partial(A, 1)
        .unwrap_or_else(|r| panic!("{e}: undo --partial: {r:?}"));
    let r = d
        .redo(A, 1)
        .unwrap_or_else(|r| panic!("{e}: redo refused: {r:?}"));
    assert_eq!(
        r,
        Applied {
            steps: 1,
            changes: one_change(A, &[0, 1])
        },
        "{e}: a redo is one batch by the redoer"
    );
    assert_eq!(d.texts(), done, "{e}: redo after --partial");
    // and undo turns that back again
    d.undo(A, 1)
        .unwrap_or_else(|r| panic!("{e}: undo after redo: {r:?}"));
    assert_eq!(
        d.texts(),
        ["one\ntwo\nthree\nfour\nFIxVE\n", "r1 x\n"],
        "{e}"
    );

    // two steps redone in order; another participant's batch leaves the stack alone
    let mut d = U::open(["x\n", "y\n"]);
    edit(&mut d, A, &[(0, vec![ins(2, "1\n")])]);
    edit(&mut d, A, &[(0, vec![ins(4, "2\n")])]);
    let u = d
        .undo(A, 2)
        .unwrap_or_else(|r| panic!("{e}: undo 2: {r:?}"));
    assert_eq!(u.steps, 2, "{e}");
    assert_eq!(d.texts()[0], "x\n", "{e}");
    edit(&mut d, B, &[(1, vec![ins(0, "b ")])]);
    let r = d
        .redo(A, 2)
        .unwrap_or_else(|r| panic!("{e}: redo after another's batch: {r:?}"));
    assert_eq!(r.steps, 2, "{e}");
    assert!(r.changes.iter().all(|c| c.author == A), "{e}: {r:?}");
    assert_eq!(d.texts(), ["x\n1\n2\n", "b y\n"], "{e}");
    // b's redo stack is b's: a has nothing to redo now, b's undo and redo are its own
    assert_eq!(d.redo(A, 1), Err(UndoRefusal::Nothing), "{e}");
    d.undo(B, 1)
        .unwrap_or_else(|r| panic!("{e}: b's undo: {r:?}"));
    assert_eq!(d.texts(), ["x\n1\n2\n", "y\n"], "{e}");
    // your own new batch clears your redo stack (U7)
    d.undo(A, 1).unwrap_or_else(|r| panic!("{e}: undo: {r:?}"));
    edit(&mut d, A, &[(0, vec![ins(0, "a ")])]);
    assert_eq!(d.redo(A, 1), Err(UndoRefusal::Nothing), "{e}: U7");
    let r = d
        .redo(B, 1)
        .unwrap_or_else(|r| panic!("{e}: b's redo after a's batch: {r:?}"));
    assert_eq!(r.changes, one_change(B, &[1]), "{e}");
    assert_eq!(d.texts(), ["a x\n1\n", "b y\n"], "{e}");
}

fn elements<U: Undo>(d: &U) -> Vec<(usize, ElemBlame)> {
    (0..2)
        .flat_map(|r| d.blame(r).into_iter().map(move |b| (r, b)))
        .collect()
}

/// Run `op` as `who`; every element it deleted must have a root `who` inserted, or be an
/// element `who` put back itself (U10a by root, v1.8); every element it inserted new and
/// `who`'s.
fn own_only<U: Undo>(
    d: &mut U,
    who: u64,
    what: &str,
    op: impl FnOnce(&mut U) -> Result<Applied, UndoRefusal>,
) {
    let e = engine::<U>();
    let before = elements(d);
    let r = op(d);
    assert!(r.is_ok(), "{e}: {what} by {who}: {r:?}");
    let after = elements(d);
    let ids = |v: &[(usize, ElemBlame)]| -> BTreeSet<(usize, (u64, u32))> {
        v.iter().map(|(r, b)| (*r, b.id)).collect()
    };
    let (was, now) = (ids(&before), ids(&after));
    for (r, b) in before.iter().filter(|(r, b)| !now.contains(&(*r, b.id))) {
        assert!(
            b.root_agent == who || b.agent == who,
            "{e}: {what} by {who} deleted root {r}'s {:?} of agent {} (change {:?}) whose \
             provenance root is agent {}'s (change {:?}): neither its own root nor its own \
             restoration",
            b.ch,
            b.agent,
            b.change,
            b.root_agent,
            b.root_change
        );
    }
    for (r, b) in after.iter().filter(|(r, b)| !was.contains(&(*r, b.id))) {
        assert_eq!(
            b.agent, who,
            "{e}: {what} by {who} inserted root {r}'s {:?} attributed to {}",
            b.ch, b.agent
        );
    }
}

/// `hist.undo_own_only`
pub fn undo_own_only<U: Undo>() {
    let e = engine::<U>();
    let mut d = U::open(["x\n", "baab\n"]);
    // two agents append the same line; each undoes its own
    edit(&mut d, A, &[(0, vec![ins(2, "a\n")])]);
    let c1 = d
        .blame(0)
        .iter()
        .find(|b| b.agent == A)
        .and_then(|b| b.change)
        .unwrap_or_else(|| panic!("{e}: no change id for A's line"));
    edit(&mut d, B, &[(0, vec![ins(4, "a\n")])]);
    own_only(&mut d, A, "undo", |d| d.undo(A, 1));
    assert_eq!(d.texts()[0], "x\na\n", "{e}");
    own_only(&mut d, A, "redo", |d| d.redo(A, 1));
    assert_eq!(d.texts()[0], "x\na\na\n", "{e}");
    own_only(&mut d, B, "undo", |d| d.undo(B, 1));
    assert_eq!(d.texts()[0], "x\na\n", "{e}");
    // regression (a): c1's own line was undone and redone; the line there is the redo's, new
    // elements (U18a) whose root is c1's (U23), so reverting c1 removes it (v1.7 conflicted)
    own_only(&mut d, A, "revert", |d| d.revert(A, c1));
    assert_eq!(d.texts()[0], "x\n", "{e}");
    assert!(
        d.blame(0).iter().all(|b| b.root_agent != A),
        "{e}: a root of c1 is still live"
    );
    // revert: two agents append the same line again; A's revert removes A's
    edit(&mut d, A, &[(0, vec![ins(2, "a\n")])]);
    let c2 = d
        .blame(0)
        .iter()
        .find(|b| b.agent == A)
        .and_then(|b| b.change)
        .unwrap_or_else(|| panic!("{e}: no change id for A's line"));
    edit(&mut d, B, &[(0, vec![ins(2, "a\n")])]);
    own_only(&mut d, A, "revert", |d| d.revert(A, c2));
    assert_eq!(d.texts()[0], "x\na\n", "{e}");
    own_only(&mut d, B, "undo", |d| d.undo(B, 1));
    assert_eq!(d.texts()[0], "x\n", "{e}");

    // a run grown by two agents: `baab` -> `baaab` (A at 2) -> `baaaab` (B at 3)
    edit(&mut d, A, &[(1, vec![ins(2, "a")])]);
    edit(&mut d, B, &[(1, vec![ins(3, "a")])]);
    own_only(&mut d, A, "undo", |d| d.undo(A, 1));
    own_only(&mut d, B, "undo", |d| d.undo(B, 1));
    assert_eq!(d.texts()[1], "baab\n", "{e}");
    own_only(&mut d, B, "redo", |d| d.redo(B, 1));
    own_only(&mut d, A, "redo", |d| d.redo(A, 1));

    // F3 (undo spec v1.7, exact hunk sets): A replaces `x` with `b`; B inserts `b` right in
    // front of it, at the region's start. A's undo turns back its own `b` and leaves B's
    // (a merge on diffed sides put B's `b` after A's and deleted it), so B's undo applies.
    edit(&mut d, A, &[(0, vec![rep(0, 1, "b")])]);
    edit(&mut d, B, &[(0, vec![ins(0, "b")])]);
    own_only(&mut d, A, "undo", |d| d.undo(A, 1));
    assert_eq!(d.texts()[0], "bx\n", "{e}: B's b stays in front");
    own_only(&mut d, B, "undo", |d| d.undo(B, 1));
    assert_eq!(d.texts()[0], "x\n", "{e}");

    // regression (b) (v1.8, by root): B edits inside A's region and undoes it. While B's
    // edit stands, A's undo conflicts; once B undid it, what B put back is B's elements with
    // A's roots, so A's undo applies and removes them (they are A's by root).
    edit(&mut d, A, &[(0, vec![rep(0, 1, "AAA")])]);
    edit(&mut d, B, &[(0, vec![rep(1, 2, "B")])]);
    assert_eq!(d.texts()[0], "ABA\n", "{e}");
    assert!(
        matches!(d.undo(A, 1), Err(UndoRefusal::Conflicts { .. })),
        "{e}: A's undo over B's live edit"
    );
    own_only(&mut d, B, "undo", |d| d.undo(B, 1));
    assert_eq!(d.texts()[0], "AAA\n", "{e}");
    let mid = d.blame(0)[1].clone();
    assert_eq!(
        (mid.agent, mid.root_agent),
        (B, A),
        "{e}: B's restoration is rooted at A's element"
    );
    own_only(&mut d, A, "undo", |d| d.undo(A, 1));
    assert_eq!(d.texts()[0], "x\n", "{e}");
}
