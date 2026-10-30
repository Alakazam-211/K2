//! Rules over [`SpanIds`] (replicas).
//!
//! - `erase.span_includes_tombstones` (E-ERASE-1): a span purge covers the elements from the
//!   span's first visible character through its last, plus every tombstone strictly
//!   between; tombstones just outside the span are excluded. A secret typed with a typo
//!   fixed inside the selection is erased whole, the typo's tombstones included, here and on
//!   every replica the purge reaches; a deleted copy pasted against either end of the
//!   selection survives (a purge by text reaches it: `erase.versions`). A span that is
//!   empty, reversed, out of range or off a character boundary is refused and changes
//!   nothing.
//!
//! The byte scan ([`crate::caps::Purge::holds`]) has its planted negative: it finds the
//! secret and the fixed typo (and the multi-byte tombstone) before the purge. The suite also
//! runs the rule on the model's wrong span purges ([`crate::model::SpanMutant`]: visible
//! elements only; the tombstones against the ends too), and it must fail.

use crate::caps::{Events, SpanIds};
use crate::erasure::secret;
use crate::{del, ins, rep, TextReplica};

/// Every rule over [`SpanIds`], by id.
pub const RULES: &[&str] = &["erase.span_includes_tombstones"];

/// Run rule `id` on engine `E`. An unknown id panics.
pub fn run<E: SpanIds>(id: &str) {
    match id {
        "erase.span_includes_tombstones" => span_includes_tombstones::<E>(),
        _ => panic!("no span rule {id:?}"),
    }
}

fn edit<E: TextReplica>(r: &mut E, hunks: Vec<crate::Hunk>) {
    if let Err(e) = r.apply(&hunks) {
        panic!("{}: refused {hunks:?}: {e:?}", E::ENGINE);
    }
}

fn deliver<E: Events>(r: &mut E, b: &E::Batch, what: &str) {
    if let Err(e) = r.deliver(b) {
        panic!("{}: {what}: refused with {e:?}", E::ENGINE);
    }
}

fn purge_span<E: SpanIds>(r: &mut E, start: usize, end: usize) -> E::Batch {
    match r.purge_span(start, end) {
        Ok(b) => b,
        Err(e) => panic!("{}: span purge {start}..{end} refused: {e:?}", E::ENGINE),
    }
}

/// `erase.span_includes_tombstones`
pub fn span_includes_tombstones<E: SpanIds>() {
    typo_inside::<E>();
    for side in [Side::After, Side::Before] {
        copy_outside::<E>(side);
    }
    refusals_and_multi_byte::<E>();
}

/// A secret typed with a typo, fixed inside the selection: the span purge erases the typo
/// (a tombstone strictly inside the span) with the secret, here and where it is delivered.
fn typo_inside<E: SpanIds>() {
    let seed = "head\ntail\n";
    let (s, typo, k) = (secret(), secret(), secret());
    let mut a = E::replica(101, seed);
    let mut b = E::replica(102, seed);
    // a types the secret with `typo` in place of its middle, then fixes it
    let line = format!("k: {}{typo}{} ;\nkeep {k}\n", &s[..12], &s[20..]);
    edit(&mut a, vec![ins(5, &line)]);
    let at = "head\nk: ".len() + 12;
    edit(&mut a, vec![rep(at, at + typo.len(), &s[12..20])]);
    let before = a.text();
    assert_eq!(
        before,
        format!("head\nk: {s} ;\nkeep {k}\ntail\n"),
        "{}",
        E::ENGINE
    );
    let to_b = a.missing_for(&b);
    deliver(&mut b, &to_b, "a's line to b");
    // the planted negative: the scan finds the secret and the fixed typo
    for (n, r) in [("a", &a), ("b", &b)] {
        assert!(
            r.holds(&s),
            "{}: the scan misses the secret on {n}",
            E::ENGINE
        );
        assert!(
            r.holds(&typo),
            "{}: the scan misses the fixed typo on {n}",
            E::ENGINE
        );
    }

    let start = before.find(&s).expect("live");
    let p = purge_span(&mut a, start, start + s.len());
    let want = before.replace(&s, "");
    assert_eq!(a.text(), want, "{}", E::ENGINE);
    deliver(&mut b, &p, "the span purge to b");
    for (n, r) in [("a", &a), ("b", &b)] {
        assert_eq!(r.text(), want, "{}: on {n}", E::ENGINE);
        assert!(
            !r.holds(&s),
            "{}: the span purge left the secret on {n}",
            E::ENGINE
        );
        assert!(
            !r.holds(&typo),
            "{}: a typo deleted inside the span survived its purge on {n}",
            E::ENGINE
        );
        assert!(
            r.holds(&k),
            "{}: the span purge erased other content on {n}",
            E::ENGINE
        );
    }
    assert_eq!(a.version(), b.version(), "{}", E::ENGINE);
}

/// Where a deleted copy was pasted, against the selection.
#[derive(Debug, Clone, Copy)]
enum Side {
    After,
    Before,
}

/// A copy of the secret pasted right against one end of it and deleted: the span purge of
/// the secret leaves the copy (a tombstone just outside the span) alone.
fn copy_outside<E: SpanIds>(side: Side) {
    let seed = "head\n";
    let s = secret();
    let mut c = E::replica(103, seed);
    let mut d = E::replica(104, seed);
    edit(&mut c, vec![ins(5, &format!("x {s} y\n"))]);
    let (start, end) = (7, 7 + s.len());
    let at = match side {
        Side::After => end,
        Side::Before => start,
    };
    edit(&mut c, vec![ins(at, &s)]);
    edit(&mut c, vec![del(at, at + s.len())]);
    let before = c.text();
    assert_eq!(before, format!("head\nx {s} y\n"), "{}", E::ENGINE);
    let to_d = c.missing_for(&d);
    deliver(&mut d, &to_d, "c's edits to d");

    let p = purge_span(&mut c, start, end);
    deliver(&mut d, &p, "the span purge to d");
    for (n, r) in [("c", &c), ("d", &d)] {
        assert_eq!(r.text(), "head\nx  y\n", "{}: {side:?}, on {n}", E::ENGINE);
        assert!(
            r.holds(&s),
            "{}: the span purge erased a deleted copy just outside the span ({side:?}) on {n}",
            E::ENGINE
        );
    }
    assert_eq!(c.version(), d.version(), "{}", E::ENGINE);
}

/// Refused spans change nothing; a span over multi-byte text with a multi-byte tombstone
/// inside.
fn refusals_and_multi_byte<E: SpanIds>() {
    let snap = |r: &E| (r.text(), r.version());
    let mut e = E::replica(105, "");
    let empty = snap(&e);
    for (start, end) in [(0, 0), (0, 1)] {
        assert!(
            e.purge_span(start, end).is_err(),
            "{}: span {start}..{end} of an empty text not refused",
            E::ENGINE
        );
        assert_eq!(snap(&e), empty, "{}: a refused span purge", E::ENGINE);
    }

    // a(0) é(1..3) 🎉(3..7) \n(7) b(8); 9 bytes
    let mut r = E::replica(106, "aé🎉\nb");
    let before = snap(&r);
    let bad = [
        (3, 3),  // empty
        (7, 3),  // reversed
        (0, 10), // out of range
        (10, 11),
        (2, 3), // off a character boundary
        (0, 2),
        (1, 4),
        (6, 8),
        (4, 4),
    ];
    for (start, end) in bad {
        assert!(
            r.purge_span(start, end).is_err(),
            "{}: span {start}..{end} not refused",
            E::ENGINE
        );
        assert_eq!(
            snap(&r),
            before,
            "{}: a refused span purge ({start}..{end}) changed the doc",
            E::ENGINE
        );
    }

    // 🎉 deleted: "aé\nb"; the span "é\n" (1..4) has its tombstone inside
    edit(&mut r, vec![del(3, 7)]);
    assert_eq!(r.text(), "aé\nb", "{}", E::ENGINE);
    assert!(
        r.holds("🎉"),
        "{}: the scan misses the tombstone",
        E::ENGINE
    );
    purge_span(&mut r, 1, 4);
    assert_eq!(r.text(), "ab", "{}", E::ENGINE);
    assert!(
        !r.holds("🎉"),
        "{}: a multi-byte tombstone inside the span survived its purge",
        E::ENGINE
    );
    assert!(r.holds("a") && r.holds("b"), "{}", E::ENGINE);
}
