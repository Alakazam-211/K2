//! Rules over [`Purge`] (replicas).
//!
//! - `erase.purge` (E-ERASE-1; K-PURGE-5): a purge of a live span is an event that deletes
//!   it and erases its content, here and on every replica it reaches, while IDs, lengths and
//!   positions survive: every other character keeps its ID (blame), anchors on and around
//!   the span still resolve (to the gap where it was), and purge-then-merge equals
//!   merge-then-purge with a concurrent edit (one inside the purged span: it stays, at the
//!   gap).
//! - `erase.no_readmit` (E-ERASE-6): purged content re-sent by any replica (the old,
//!   content-bearing events) is accepted as known and changes nothing; it is never held
//!   again. A replica whose frontier predates the purge gets the purge with the events and
//!   never holds the content.
//!
//! The byte scan ([`Purge::holds`]) has its planted negative: every replica holding the
//! secret is found before the purge. Purge state is compared only once both replicas hold
//! the purge, and a batch never splits a purged insert from its purge.

use crate::caps::{Anchors, Blame, Blamed, Events, Purge, Side};
use crate::erasure::secret;
use crate::{ins, TextReplica};

/// Every rule over [`Purge`], by id. `erase.purge` also needs [`Blame`] and [`Anchors`].
pub const RULES: &[&str] = &["erase.purge", "erase.no_readmit"];

/// Run rule `id` on engine `E`. An unknown id panics.
pub fn run<E: Purge + Blame + Anchors>(id: &str) {
    match id {
        "erase.purge" => purge::<E>(),
        "erase.no_readmit" => no_readmit::<E>(),
        _ => panic!("no purge rule {id:?}"),
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

/// Purge `needle`'s (only) live copy on `r`; returns the batch and where it was.
pub(crate) fn purge_live<E: Purge>(r: &mut E, needle: &str) -> (E::Batch, usize) {
    let t = r.text();
    let at = t
        .find(needle)
        .unwrap_or_else(|| panic!("{}: {needle:?} not live", E::ENGINE));
    match r.purge(at, at + needle.len()) {
        Ok(b) => (b, at),
        Err(e) => panic!("{}: purge refused: {e:?}", E::ENGINE),
    }
}

/// `blame` without the characters at bytes `start..end` of the text it covers.
pub(crate) fn blame_without<I: Clone>(b: &[Blamed<I>], start: usize, end: usize) -> Vec<Blamed<I>> {
    let mut at = 0;
    let mut out = Vec::new();
    for x in b {
        if at < start || at >= end {
            out.push(x.clone());
        }
        at += x.ch.len_utf8();
    }
    out
}

/// `erase.purge`
pub fn purge<E: Purge + Blame + Anchors>() {
    let seed = "head\ntail\n";
    let (s, k) = (secret(), secret());
    let mut a = E::replica(101, seed);
    let mut b = E::replica(102, seed);
    let mut c = E::replica(103, seed);
    // the secret, and in the same batch a line that stays
    edit(&mut a, vec![ins(5, &format!("k: {s} é\nkeep {k}\n"))]);
    let full = a.missing_for(&b);
    deliver(&mut b, &full, "a's line to b");
    deliver(&mut c, &full, "a's line to c");
    let before = a.text();
    // the planted negative: every replica holding it is found
    for (n, r) in [("a", &a), ("b", &b), ("c", &c)] {
        assert!(
            r.holds(&s),
            "{}: the scan misses the secret on {n}",
            E::ENGINE
        );
    }
    let at = before.find(&s).expect("live");
    let end = at + s.len();
    let anchors = [
        (
            "tail",
            a.anchor(before.find("tail").expect("tail"), Side::Right),
        ),
        ("inside", a.anchor(at + 10, Side::Right)),
        ("last", a.anchor(end, Side::Left)),
        ("after", a.anchor(end, Side::Right)),
    ];
    let blame_before = a.blame();
    // b edits concurrently with the purge: inside the span, and at the end
    let bt = b.text();
    edit(&mut b, vec![ins(at + 16, "<b>"), ins(bt.len(), "b 🎉\n")]);

    // the purge: deleted and erased, nothing else changes
    let (p, _) = purge_live(&mut a, &s);
    let want = before.replace(&s, "");
    assert_eq!(a.text(), want, "{}", E::ENGINE);
    assert!(!a.holds(&s), "{}: purged content is still held", E::ENGINE);
    assert!(a.holds(&k), "{}: the purge erased other content", E::ENGINE);
    let kept = blame_without(&blame_before, at, end);
    assert_eq!(
        a.blame(),
        kept,
        "{}: other characters changed ID",
        E::ENGINE
    );
    let gap = at;
    let where_now = |r: &E, which: &str| -> Option<usize> {
        let (_, an) = anchors.iter().find(|(n, _)| *n == which).expect("anchor");
        r.resolve(
            an.as_ref()
                .unwrap_or_else(|| panic!("{}: no anchor {which}", E::ENGINE)),
        )
    };
    let tail_at = want.find("tail").expect("tail");
    for r in [&a] {
        assert_eq!(where_now(r, "tail"), Some(tail_at), "{}: tail", E::ENGINE);
        for which in ["inside", "last", "after"] {
            assert_eq!(
                where_now(r, which),
                Some(gap),
                "{}: an anchor on or by the purged span ({which})",
                E::ENGINE
            );
        }
    }

    // it reaches a replica that had the content
    deliver(&mut c, &p, "the purge to c");
    assert_eq!(c.text(), want, "{}", E::ENGINE);
    assert!(!c.holds(&s), "{}: the purge didn't reach c", E::ENGINE);
    assert_eq!(c.blame(), kept, "{}", E::ENGINE);
    assert_eq!(where_now(&c, "inside"), Some(gap), "{}: on c", E::ENGINE);
    assert_eq!(where_now(&c, "tail"), Some(tail_at), "{}: on c", E::ENGINE);

    // purge then merge = merge then purge: a and b (b's concurrent edit) exchange
    let to_a = b.missing_for(&a);
    let to_b = a.missing_for(&b);
    deliver(&mut a, &to_a, "b's edit to a");
    deliver(&mut b, &to_b, "the purge to b");
    let merged = format!("{}b 🎉\n", before.replace(&s, "<b>"));
    assert_eq!(a.text(), merged, "{}", E::ENGINE);
    assert_eq!(b.text(), merged, "{}", E::ENGINE);
    assert_eq!(a.version(), b.version(), "{}", E::ENGINE);
    assert!(!b.holds(&s), "{}: the purge didn't reach b", E::ENGINE);
    assert!(b.holds("b 🎉"), "{}: the purge erased b's edit", E::ENGINE);
    assert_eq!(b.blame(), a.blame(), "{}", E::ENGINE);

    // edits at the gap still merge
    edit(&mut a, vec![ins(gap, "new ")]);
    edit(&mut b, vec![ins(gap + 4, "B")]);
    let to_c = a.missing_for(&c);
    deliver(&mut c, &to_c, "to c");
    let to_c = b.missing_for(&c);
    deliver(&mut c, &to_c, "to c");
    let to_a = c.missing_for(&a);
    deliver(&mut a, &to_a, "to a");
    let to_b = c.missing_for(&b);
    deliver(&mut b, &to_b, "to b");
    assert_eq!(a.text(), c.text(), "{}", E::ENGINE);
    assert_eq!(b.text(), c.text(), "{}", E::ENGINE);
    for r in [&a, &b, &c] {
        assert!(!r.holds(&s), "{}", E::ENGINE);
        assert!(r.holds(&k), "{}", E::ENGINE);
    }
}

/// `erase.no_readmit`
pub fn no_readmit<E: Purge + Blame + Anchors>() {
    let seed = "head\n";
    let s = secret();
    let mut a = E::replica(101, seed);
    let mut b = E::replica(102, seed);
    let v0 = a.version();
    edit(&mut a, vec![ins(5, &format!("s: {s}\n"))]);
    // what a client held before the purge: the content-bearing events
    let old = a.events_since(&v0);
    deliver(&mut b, &old, "a's events to b");
    assert!(b.holds(&s), "{}: the planted negative", E::ENGINE);
    let (_, _) = purge_live(&mut a, &s);
    let snap = |r: &E| (r.text(), r.version());
    let after = snap(&a);

    // re-sent: b's events (all it has) and the old batch, again: known, nothing changes
    let resent = b.events_since(&v0);
    for (what, batch) in [("b re-sends", &resent), ("the old batch", &old)] {
        let r = a.deliver(batch);
        assert_eq!(r, Ok(()), "{}: {what}: re-sent events refused", E::ENGINE);
        assert_eq!(snap(&a), after, "{}: {what} changed the doc", E::ENGINE);
        assert!(
            !a.holds(&s),
            "{}: {what} re-admitted purged content",
            E::ENGINE
        );
    }

    // the stale replica gets the purge
    let to_b = a.missing_for(&b);
    deliver(&mut b, &to_b, "the purge to b");
    assert_eq!(snap(&b), after, "{}", E::ENGINE);
    assert!(!b.holds(&s), "{}: b still holds purged content", E::ENGINE);
    // and re-sending the old batch to it changes nothing either
    deliver(&mut b, &old, "the old batch to b");
    assert_eq!(snap(&b), after, "{}", E::ENGINE);
    assert!(!b.holds(&s), "{}", E::ENGINE);

    // a replica whose frontier predates the content gets the purge with it: never holds it
    let mut c = E::replica(103, seed);
    let all = a.events_since(&v0);
    deliver(&mut c, &all, "everything to a new replica");
    assert_eq!(snap(&c), after, "{}", E::ENGINE);
    assert!(
        !c.holds(&s),
        "{}: a new replica got purged content",
        E::ENGINE
    );
    // then the old batch arrives late (a client re-sending): dropped
    deliver(&mut c, &old, "the old batch, late");
    assert_eq!(snap(&c), after, "{}", E::ENGINE);
    assert!(!c.holds(&s), "{}: content re-admitted on c", E::ENGINE);

    // a replica that got the old content first, then everything
    let mut d = E::replica(104, seed);
    deliver(&mut d, &old, "the old batch to d");
    assert!(d.holds(&s), "{}", E::ENGINE);
    let to_d = a.missing_for(&d);
    deliver(&mut d, &to_d, "the purge to d");
    assert_eq!(snap(&d), after, "{}", E::ENGINE);
    assert!(!d.holds(&s), "{}", E::ENGINE);
    assert_eq!(d.blame(), a.blame(), "{}", E::ENGINE);
}
