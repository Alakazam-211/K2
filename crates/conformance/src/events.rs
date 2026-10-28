//! Rules over [`Events`]: delivery semantics (E-CORE-2, -3, -4) and split delivery of one
//! atomic edit (K-CT-3).
//!
//! - `core.idempotent`: re-delivering any known event, or any subset of known events,
//!   changes nothing; any grouping and repetition of the same events gives the same state.
//! - `core.causal`: a batch is accepted only if every parent is known or in the batch, and
//!   parents never form a cycle; otherwise it is refused (`MissingDeps` or `Cycle`) and
//!   nothing changes; the replica then accepts the missing events and the batch as normal.
//! - `core.no_equivocation`: a batch carrying a known event ID with different content is
//!   refused as `Equivocation`, and nothing changes.
//! - `text.edit_atomic_split`: a multi-hunk edit reaches another replica whole or not at
//!   all, whatever the delivery is split into: its batch before its dependencies, apart from
//!   them, together with them, or relayed through a third replica.
//!
//! A batch is only ever made by the engine ([`Events::events_since`],
//! [`Events::missing_for`], [`Events::cyclic_batch`]): no rule builds engine bytes.
//! Writers are agents 101, 102, ...; the seed is [`crate::SEED_AGENT`]'s.

use k2_mpc_base::text::diff;

use crate::caps::{EventRefusal, Events};
use crate::{deviates, ins, Hunk, TextReplica};

/// Every rule over [`Events`], by id.
pub const RULES: &[&str] = &[
    "core.idempotent",
    "core.causal",
    "core.no_equivocation",
    "text.edit_atomic_split",
];

/// Run rule `id` on engine `E`. An unknown id panics.
pub fn run<E: Events>(id: &str) {
    match id {
        "core.idempotent" => idempotent::<E>(),
        "core.causal" => causal::<E>(),
        "core.no_equivocation" => no_equivocation::<E>(),
        "text.edit_atomic_split" => edit_atomic_split::<E>(),
        _ => panic!("no events rule {id:?}"),
    }
}

/// What a rule compares to say "nothing changed".
fn snap<E: TextReplica>(r: &E) -> (String, E::Version) {
    (r.text(), r.version())
}

fn edit<E: TextReplica>(r: &mut E, hunks: Vec<Hunk>) {
    if let Err(e) = r.apply(&hunks) {
        panic!("{}: refused {hunks:?}: {e:?}", E::ENGINE);
    }
}

/// Deliver `b`; a refusal fails the rule.
fn deliver<E: Events>(r: &mut E, b: &E::Batch, what: &str) {
    if let Err(e) = r.deliver(b) {
        panic!("{}: {what}: refused with {e:?}", E::ENGINE);
    }
}

/// Deliver `b` and assert it changed nothing.
fn deliver_known<E: Events>(r: &mut E, b: &E::Batch, what: &str) {
    let before = snap(r);
    deliver(r, b, what);
    assert_eq!(
        snap(r),
        before,
        "{}: {what}: re-delivering known events changed the replica",
        E::ENGINE
    );
}

/// `core.idempotent` (E-CORE-2).
pub fn idempotent<E: Events>() {
    let seed = "one\ntwo\nthree\n";
    let mut a = E::replica(101, seed);
    let mut b = E::replica(102, seed);
    let v0 = a.version();
    edit(&mut a, vec![ins(0, "A1 ")]);
    let e1 = a.events_since(&v0);
    let v1 = a.version();
    // a multi-hunk edit with a delete and a replace
    let cur = a.text();
    edit(&mut a, diff::hunks(&cur, "A1 one\nTWO\nthree 🎉\n"));
    let e2 = a.events_since(&v1);
    let all = a.events_since(&v0);
    // b edits concurrently
    let vb0 = b.version();
    edit(&mut b, vec![ins(seed.len(), "b é\n")]);
    let eb = b.events_since(&vb0);

    // c gets everything once, then again, then each subset again
    let mut c = E::replica(103, seed);
    deliver(&mut c, &all, "a's events");
    assert_eq!(c.text(), a.text(), "{}", E::ENGINE);
    deliver_known(&mut c, &all, "all of a's events again");
    deliver_known(&mut c, &e1, "a's first edit again");
    deliver_known(&mut c, &e2, "a's second edit again");
    deliver(&mut c, &eb, "b's events");
    deliver_known(&mut c, &all, "a's events after b's");
    deliver_known(&mut c, &eb, "b's events again");

    // d gets the same events grouped and repeated otherwise: the same state
    let mut d = E::replica(104, seed);
    deliver(&mut d, &eb, "b's events first");
    deliver(&mut d, &e1, "a's first edit");
    deliver_known(&mut d, &e1, "a's first edit again");
    deliver(&mut d, &e2, "a's second edit");
    deliver_known(&mut d, &all, "all of a's events in one batch, again");
    deliver_known(&mut d, &eb, "b's events again");
    assert_eq!(
        snap(&d),
        snap(&c),
        "{}: grouping changed the result",
        E::ENGINE
    );

    // echo: a replica's own events delivered back to it, and a full exchange repeated
    deliver_known(&mut a, &all, "a's own events, echoed");
    let (to_a, to_b) = (b.missing_for(&a), a.missing_for(&b));
    deliver(&mut a, &to_a, "b to a");
    deliver(&mut b, &to_b, "a to b");
    assert_eq!(snap(&a), snap(&b), "{}: a and b diverged", E::ENGINE);
    assert_eq!(snap(&a), snap(&c), "{}: a and c diverged", E::ENGINE);
    let everything = a.events_since(&v0);
    for r in [&mut a, &mut b, &mut c, &mut d] {
        deliver_known(r, &everything, "everything again");
    }
    // nothing missing: an empty batch is a no-op too
    let none = a.missing_for(&b);
    deliver_known(&mut b, &none, "an empty batch");
}

/// `core.causal` (E-CORE-3).
///
/// Three cases: the batch skips the sender's own earlier event; the batch's event depends
/// on another agent's event the receiver lacks; and (when the engine can express one) a
/// batch whose events' parents form a cycle. Each must be refused with nothing changed,
/// and the receiver must then take the missing events and the batch as normal.
///
/// A cycle's members can never all have their parents known, so an engine that delivers
/// in causal order refuses a cycle as `MissingDeps`; either refusal meets the requirement
/// ("refused with `MissingDeps` or `Cycle`").
pub fn causal<E: Events>() {
    let seed = "base\n";
    let mut a = E::replica(101, seed);
    let v0 = a.version();
    edit(&mut a, vec![ins(0, "first ")]);
    let (t1, e1, v1) = (a.text(), a.events_since(&v0), a.version());
    let cur = a.text();
    edit(&mut a, diff::hunks(&cur, "FIRST base\nsecond\n"));
    let e2 = a.events_since(&v1);

    // 1. the sender's own earlier event is missing
    let mut c = E::replica(103, seed);
    let before = snap(&c);
    let r = c.deliver(&e2);
    assert_eq!(
        r,
        Err(EventRefusal::MissingDeps),
        "{}: a batch whose sender's earlier edit is missing",
        E::ENGINE
    );
    assert_eq!(
        snap(&c),
        before,
        "{}: a refused batch changed it",
        E::ENGINE
    );
    deliver(&mut c, &e1, "the missing edit");
    assert_eq!(c.text(), t1, "{}", E::ENGINE);
    deliver(&mut c, &e2, "the batch, now causal");
    assert_eq!(c.text(), a.text(), "{}", E::ENGINE);

    // 2. another agent's event is missing: b edits right after a's first edit
    let mut b = E::replica(102, seed);
    deliver(&mut b, &e1, "a's first edit to b");
    let vb = b.version();
    edit(&mut b, vec![ins("first ".len(), "b-after-first ")]);
    let eb = b.events_since(&vb);
    let mut d = E::replica(104, seed);
    let before = snap(&d);
    let r = d.deliver(&eb);
    assert!(
        r.is_err(),
        "{}: accepted an event whose parent (another agent's) it doesn't have",
        E::ENGINE
    );
    assert_eq!(
        d.text(),
        before.0,
        "{}: a batch with a missing parent changed the text",
        E::ENGINE
    );
    let clean = r == Err(EventRefusal::MissingDeps)
        && snap(&d) == before
        && d.deliver(&e1).is_ok()
        && d.deliver(&eb).is_ok()
        && d.text() == b.text();
    assert_eq!(
        clean,
        !deviates::<E>("core.causal"),
        "{}: another agent's missing parent: refusal {r:?}, then recovered: {clean}",
        E::ENGINE
    );

    // 3. a cycle, where the engine's events can express one
    if let Some(cy) = E::cyclic_batch() {
        let mut f = E::replica(105, seed);
        let before = snap(&f);
        let r = f.deliver(&cy);
        assert!(
            matches!(r, Err(EventRefusal::Cycle | EventRefusal::MissingDeps)),
            "{}: a cyclic batch: {r:?}",
            E::ENGINE
        );
        assert_eq!(
            snap(&f),
            before,
            "{}: a refused cycle changed it",
            E::ENGINE
        );
        deliver(
            &mut f,
            &a.events_since(&v0),
            "a's events after a refused cycle",
        );
        assert_eq!(f.text(), a.text(), "{}", E::ENGINE);
    }
}

/// `core.no_equivocation` (E-CORE-4): two replicas edit as the same agent from the same
/// state, so their first events have the same ID and different content (`a` vs `b`, one
/// character each). A replica holding one refuses the other.
pub fn no_equivocation<E: Events>() {
    let seed = "xy\n";
    let mut a = E::replica(101, seed);
    let mut forked = E::replica(101, seed);
    let v0 = a.version();
    edit(&mut a, vec![ins(0, "a")]);
    edit(&mut forked, vec![ins(0, "b")]);
    let ea = a.events_since(&v0);
    let ef = forked.events_since(&v0);

    let mut c = E::replica(102, seed);
    deliver(&mut c, &ea, "a's event");
    let before = snap(&c);
    let r = c.deliver(&ef);
    assert_eq!(snap(&c), before, "{}: equivocation changed it", E::ENGINE);
    assert_eq!(c.text(), "axy\n", "{}", E::ENGINE);
    let refused = r == Err(EventRefusal::Equivocation);
    if deviates::<E>("core.no_equivocation") {
        // the pinned behaviour: accepted as already known, silently, first content kept
        assert_eq!(
            r,
            Ok(()),
            "{}: the known deviation changed (it now refuses with {r:?}): re-check it",
            E::ENGINE
        );
    } else {
        assert!(refused, "{}: equivocation not refused: {r:?}", E::ENGINE);
    }
    // the refusal leaves the replica working
    deliver_known(&mut c, &ea, "a's event again");
    edit(&mut c, vec![ins(0, ">")]);
    assert_eq!(c.text(), ">axy\n", "{}", E::ENGINE);
}

/// `text.edit_atomic_split` (K-CT-3): `text.edit_atomic` under split delivery.
///
/// a makes edit E1, then a multi-hunk edit E2. Another replica gets E2's batch before
/// E1's (refused: nothing of E2 shows), then E1's, then E2's; another gets both in one
/// batch; another gets them relayed one at a time through a third replica; and a replica
/// with a concurrent edit gets E2 alone after E1. At every step a replica's text is the
/// seed, the text after E1 or the text after E2 (merged with its own edit): never part of
/// E2.
///
/// A split inside one edit's own events is not expressible through [`Events`] (rules only
/// name versions a replica has had); engines whose edits are one event make it moot.
pub fn edit_atomic_split<E: Events>() {
    let seed = "alpha\nbeta\ngamma\n";
    let mut a = E::replica(101, seed);
    let v0 = a.version();
    edit(&mut a, vec![ins(seed.len(), "delta\n")]);
    let (t1, e1, v1) = (a.text(), a.events_since(&v0), a.version());
    let t2 = "ALPHA\nbeta\nGAMMA\ndelta!\n";
    let hunks = diff::hunks(&t1, t2);
    assert!(
        hunks.len() >= 3,
        "the rule needs a multi-hunk edit: {hunks:?}"
    );
    edit(&mut a, hunks);
    assert_eq!(a.text(), t2, "{}", E::ENGINE);
    let e2 = a.events_since(&v1);
    let e12 = a.events_since(&v0);

    // E2 before its dependency, then in order
    let mut c = E::replica(102, seed);
    let r = c.deliver(&e2);
    assert!(
        r.is_err(),
        "{}: E2 accepted before E1, which it depends on",
        E::ENGINE
    );
    assert_eq!(c.text(), seed, "{}: part of E2 showed early", E::ENGINE);
    deliver(&mut c, &e1, "E1");
    assert_eq!(c.text(), t1, "{}", E::ENGINE);
    deliver(&mut c, &e2, "E2");
    assert_eq!(c.text(), t2, "{}", E::ENGINE);

    // together
    let mut d = E::replica(103, seed);
    deliver(&mut d, &e12, "E1 and E2 in one batch");
    assert_eq!(d.text(), t2, "{}", E::ENGINE);

    // relayed one edit at a time through g
    let mut g = E::replica(104, seed);
    let mut f = E::replica(105, seed);
    deliver(&mut g, &e1, "E1 to the relay");
    let relayed = g.missing_for(&f);
    deliver(&mut f, &relayed, "the relay's E1");
    assert_eq!(f.text(), t1, "{}", E::ENGINE);
    deliver(&mut g, &e2, "E2 to the relay");
    let relayed = g.missing_for(&f);
    deliver(&mut f, &relayed, "the relay's E2");
    assert_eq!(f.text(), t2, "{}", E::ENGINE);

    // with a concurrent edit on another line: all of E2's hunks show, or none
    let mut h = E::replica(106, seed);
    edit(&mut h, vec![ins("alpha\n".len(), "(h)")]);
    deliver(&mut h, &e1, "E1 to h");
    assert_eq!(h.text(), "alpha\n(h)beta\ngamma\ndelta\n", "{}", E::ENGINE);
    deliver(&mut h, &e2, "E2 to h");
    assert_eq!(h.text(), "ALPHA\n(h)beta\nGAMMA\ndelta!\n", "{}", E::ENGINE);
}
