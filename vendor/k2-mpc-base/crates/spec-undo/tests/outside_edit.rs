//! The oracle on the one-writer soak's undo finding (v0.9.3; the kit's side is
//! `crates/base/tests/outside_edit_undo.rs`).
//!
//! Seed `X\nA\nA\n`; a0 inserts `B\n` between the `A`s; an outside edit, made on the file's
//! older state, inserts `B\nA\n` at the same spot, and the engine's merge puts it **before**
//! a0's `B\n`: `X A [B A] B A`. The spec has the engine report that change at element-identity
//! positions (SPEC-UNDO, preamble): the copy inserted at 4. Its undo by a0 then applies and
//! removes a0's own `B\n` (the window is a0's line, untouched). Reported as a diff of the live
//! text instead (an insertion at 8, after a0's line), the oracle's elements no longer match
//! the engine's, but its undo still applies: a conflict there is no reading of the spec.

use spec_undo::model::{Bounds, Edit, Mode, Model, Outcome, Request};

fn e(start: usize, end: usize, insert: &str) -> Edit {
    Edit {
        start,
        end,
        insert: insert.into(),
    }
}

fn undo_after(outside_at: usize) -> (Outcome, String) {
    let mut m = Model::new(vec!["X\nA\nA\n".into()], 1, Bounds::default());
    m.edit("a0", None, vec![(0, vec![e(4, 4, "B\n")])]);
    assert_eq!(m.texts[0], "X\nA\nB\nA\n");
    m.edit(
        "kit:external",
        None,
        vec![(0, vec![e(outside_at, outside_at, "B\nA\n")])],
    );
    assert_eq!(m.texts[0], "X\nA\nB\nA\nB\nA\n");
    let (out, _) = m.undo("a0", None, &Request::new(1, Mode::All));
    (out, m.texts[0].clone())
}

#[test]
fn undo_after_an_outside_copy_applies_in_the_oracle() {
    // reported at element identity (the copy inserted at 4, before a0's line): a0's undo
    // applies and removes a0's own B; the copy's B stays at 4
    let (out, text) = undo_after(4);
    assert!(matches!(out, Outcome::Applied { steps: 1, .. }), "{out:?}");
    assert_eq!(text, "X\nA\nB\nA\nA\n");
    let mut m = Model::new(vec!["X\nA\nA\n".into()], 1, Bounds::default());
    m.edit("a0", None, vec![(0, vec![e(4, 4, "B\n")])]);
    m.edit("kit:external", None, vec![(0, vec![e(4, 4, "B\nA\n")])]);
    m.undo("a0", None, &Request::new(1, Mode::All));
    let owners: Vec<&str> = m.elems[0]
        .iter()
        .map(|el| m.who[usize::from(el.owner)].as_str())
        .collect();
    assert_eq!(owners[4], "kit:external", "{owners:?}");
    // reported as a diff of the live text (an insertion at 8, after a0's line): the oracle
    // still applies (a0's line is untouched in its elements too), but its elements are no
    // longer the engine's, so the text it removes is another B than the engine would
    let (out, text) = undo_after(8);
    assert!(matches!(out, Outcome::Applied { steps: 1, .. }), "{out:?}");
    assert_eq!(text, "X\nA\nA\nB\nA\n");
}
