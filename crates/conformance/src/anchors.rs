//! Rules over [`Anchors`].
//!
//! - `text.cursor_stable` (E-TEXT-10): a relative position resolves to the same place on
//!   every replica after any concurrent edits: just after its character (`Left`) or just
//!   before it (`Right`), with text inserted concurrently at that gap kept on the other
//!   side; an anchor whose character was deleted resolves to the gap where it was. Anchors
//!   made on one replica resolve on every replica.

use crate::caps::{Anchors, Side};
use crate::Net;

/// Every rule over [`Anchors`], by id.
pub const RULES: &[&str] = &["text.cursor_stable"];

/// Run rule `id` on engine `E`. An unknown id panics.
pub fn run<E: Anchors>(id: &str) {
    match id {
        "text.cursor_stable" => cursor_stable::<E>(),
        _ => panic!("no anchors rule {id:?}"),
    }
}

fn anchor<E: Anchors>(r: &E, at: usize, side: Side) -> E::Anchor {
    r.anchor(at, side).unwrap_or_else(|| {
        panic!(
            "{}: no anchor at {at} {side:?} of {:?}",
            E::ENGINE,
            r.text()
        )
    })
}

/// `text.cursor_stable`
pub fn cursor_stable<E: Anchors>() {
    // Unedited, an anchor resolves where it was made, on every boundary with a character
    // on its side (multi-byte characters included).
    let r = E::replica(101, "a🎉é\nz");
    let t = r.text();
    for at in (0..=t.len()).filter(|&k| t.is_char_boundary(k)) {
        for side in [Side::Left, Side::Right] {
            let has = match side {
                Side::Left => at > 0,
                Side::Right => at < t.len(),
            };
            if has {
                assert_eq!(
                    r.resolve(&anchor(&r, at, side)),
                    Some(at),
                    "{}: {at} {side:?} unedited",
                    E::ENGINE
                );
            }
        }
    }

    let seed = "0123456789\n";
    let mut n = Net::<E>::new(&[101, 102, 103], seed);
    // anchors on replica 0 (101): before `0`; after `2` and before `3` (one gap, both
    // sides); after `5` and before `6` (both deleted below)
    let r0 = anchor(&n.r[0], 0, Side::Right);
    let l3 = anchor(&n.r[0], 3, Side::Left);
    let r3 = anchor(&n.r[0], 3, Side::Right);
    let l6 = anchor(&n.r[0], 6, Side::Left);
    let r6 = anchor(&n.r[0], 6, Side::Right);
    // replica 1 (102) types `Xé` in the gap between `2` and `3`, and anchors on its own text
    n.insert(1, 3, "Xé");
    let xb = anchor(&n.r[1], 3, Side::Right); // before `X`
    let ea = anchor(&n.r[1], 6, Side::Left); // after `é`
                                             // a replica that hasn't received `X` yet can't resolve its anchor
    assert_eq!(
        n.r[0].resolve(&xb),
        None,
        "{}: `X` not received yet",
        E::ENGINE
    );
    // the gap's two anchors on replica 1, before any sync: `Xé` lands between them
    assert_eq!(
        n.r[1].resolve(&l3),
        Some(3),
        "{}: left of the insert",
        E::ENGINE
    );
    assert_eq!(
        n.r[1].resolve(&r3),
        Some(6),
        "{}: right of the insert",
        E::ENGINE
    );
    // replica 2 (103) deletes `56`; replica 0 types `🎉` at the start
    n.delete(2, 5, 7);
    assert_eq!(
        n.r[2].resolve(&l6),
        Some(5),
        "{}: deleted `5`, locally",
        E::ENGINE
    );
    assert_eq!(
        n.r[2].resolve(&r6),
        Some(5),
        "{}: deleted `6`, locally",
        E::ENGINE
    );
    n.insert(0, 0, "🎉");
    let t = n.sync_all();
    assert_eq!(t, "🎉012Xé34789\n", "{}", E::ENGINE);

    let want = [
        ("before `0`", &r0, 4),
        ("after `2`", &l3, 7),
        ("before `3`", &r3, 10),
        ("after the deleted `5`", &l6, 12),
        ("before the deleted `6`", &r6, 12),
        ("before `X`", &xb, 7),
        ("after `é`", &ea, 10),
    ];
    for (i, r) in n.r.iter().enumerate() {
        for (what, a, at) in want {
            assert_eq!(
                r.resolve(a),
                Some(at),
                "{}: replica {i}: anchor {what} in {t:?}",
                E::ENGINE
            );
        }
    }

    // later edits around the gaps keep them: delete `X`, type at the end
    n.delete(2, 7, 8);
    n.insert(1, n.text(1).len(), "end");
    let t = n.sync_all();
    assert_eq!(t, "🎉012é34789\nend", "{}", E::ENGINE);
    for (i, r) in n.r.iter().enumerate() {
        for (what, a, at) in [
            ("after `2`", &l3, 7),
            ("before `3`", &r3, 9),
            ("before the deleted `X`", &xb, 7),
            ("after the deleted `5`", &l6, 11),
        ] {
            assert_eq!(
                r.resolve(a),
                Some(at),
                "{}: replica {i}: {what} in {t:?}",
                E::ENGINE
            );
        }
    }
}
