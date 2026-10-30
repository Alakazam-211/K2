//! Rules over [`Versions`] (doc level: one doc that agents edit).
//!
//! - `hist.version_stable` (E-HIST-1): a version always shows the same state, through later
//!   edits by anyone, deletes of its text, restores and further versions; two versions of
//!   one state show the same.
//! - `hist.restore_forward` (E-HIST-2; K-HIST-8): restoring version V makes the text equal
//!   V's, as one new change by the restorer; history is never rewritten (every version still
//!   shows what it did); restoring the version the doc is at changes nothing; a restore is
//!   undone by restoring the version before it.
//! - `hist.restore_minimal` (K-HIST-8; undo spec D1): a restore is the kit's operation on
//!   every engine, an edit of the kit's pipeline (`align_insertions(diff::hunks(live,
//!   target))`). Restoring a version whose text the doc has (the current one, or one the
//!   doc came back to) emits no events; otherwise the hunks the engine applied are exactly
//!   the pipeline's, as it reports them (one change by the restorer).
//!
//! Purged content in versions is `erase.versions` ([`crate::erasure`]).

use k2_mpc_base::text::{diff, lines::align_insertions};

use crate::caps::{Change, Versions};
use crate::{del, ins, rep, Hunk};

/// Every rule over [`Versions`], by id.
pub const RULES: &[&str] = &[
    "hist.version_stable",
    "hist.restore_forward",
    "hist.restore_minimal",
];

/// Run rule `id` on engine `V`. An unknown id panics.
pub fn run<V: Versions>(id: &str) {
    match id {
        "hist.version_stable" => version_stable::<V>(),
        "hist.restore_forward" => restore_forward::<V>(),
        "hist.restore_minimal" => restore_minimal::<V>(),
        _ => panic!("no versions rule {id:?}"),
    }
}

const A: u64 = 101;
const B: u64 = 102;
const R: u64 = 103;

pub(crate) fn engine<V: Versions>() -> &'static str {
    <V::Engine as crate::TextReplica>::ENGINE
}

pub(crate) fn edit<V: Versions>(d: &mut V, agent: u64, hunks: Vec<Hunk>) {
    if let Err(e) = d.edit(agent, &hunks) {
        panic!("{}: {agent} refused {hunks:?}: {e:?}", engine::<V>());
    }
}

/// `old` → `new` in the current text (first occurrence), as `agent`.
pub(crate) fn swap<V: Versions>(d: &mut V, agent: u64, old: &str, new: &str) {
    let t = d.text();
    let at = t
        .find(old)
        .unwrap_or_else(|| panic!("{}: {old:?} not in {t:?}", engine::<V>()));
    edit(d, agent, vec![rep(at, at + old.len(), new)]);
}

pub(crate) fn show<V: Versions>(d: &V, v: &V::VersionId) -> String {
    d.show(v)
        .unwrap_or_else(|e| panic!("{}: show {v:?}: {e:?}", engine::<V>()))
}

pub(crate) fn restore<V: Versions>(d: &mut V, agent: u64, v: &V::VersionId) -> Option<Change> {
    d.restore(agent, v)
        .unwrap_or_else(|e| panic!("{}: restore {v:?}: {e:?}", engine::<V>()))
}

/// Every recorded version still shows its text.
fn all_stable<V: Versions>(d: &V, seen: &[(V::VersionId, String)], at: &str) {
    for (i, (v, t)) in seen.iter().enumerate() {
        assert_eq!(
            &show(d, v),
            t,
            "{}: {at}: version {i} ({v:?}) shows another state",
            engine::<V>()
        );
    }
}

/// `hist.version_stable`
pub fn version_stable<V: Versions>() {
    let seed = "one\ntwo\nthree\n";
    let mut d = V::open(seed);
    let e = engine::<V>();
    let mut seen: Vec<(V::VersionId, String)> = Vec::new();
    let cut = |d: &mut V, seen: &mut Vec<(V::VersionId, String)>, by: u64| {
        let v = d.version(by);
        seen.push((v, d.text()));
    };
    cut(&mut d, &mut seen, A);
    assert_eq!(seen[0].1, seed, "{e}");

    // edits by two agents, multi-byte text, one multi-hunk batch
    edit(&mut d, A, vec![ins(4, "é and 🎉 ")]);
    let t = d.text();
    let end = t.len();
    edit(&mut d, B, vec![rep(0, 3, "ONE"), ins(end, "four\n")]);
    cut(&mut d, &mut seen, B);
    all_stable(&d, &seen, "after edits");

    // the version's text is deleted and rewritten: it still shows it
    swap(&mut d, A, "é and 🎉 ", "");
    swap(&mut d, B, "three\n", "");
    swap(&mut d, A, "ONE", "uno");
    all_stable(&d, &seen, "after deleting a version's text");
    cut(&mut d, &mut seen, A);

    // two versions of one state show the same
    let again = d.version(B);
    assert_eq!(show(&d, &again), d.text(), "{e}");
    assert_eq!(
        show(&d, &again),
        seen.last().expect("a version").1,
        "{e}: two versions of one state"
    );

    // a restore, then more edits and versions: none of the earlier ones moves
    let first = seen[0].0.clone();
    restore(&mut d, R, &first);
    assert_eq!(d.text(), seed, "{e}");
    cut(&mut d, &mut seen, R);
    let t = d.text();
    edit(&mut d, B, vec![ins(t.len(), "after restore\n")]);
    cut(&mut d, &mut seen, B);
    all_stable(&d, &seen, "after a restore");

    // the empty text is a state like any other
    let t = d.text();
    edit(&mut d, A, vec![del(0, t.len())]);
    cut(&mut d, &mut seen, A);
    assert_eq!(seen.last().expect("a version").1, "", "{e}");
    edit(&mut d, A, vec![ins(0, "again\n")]);
    all_stable(&d, &seen, "at the end");
}

/// `hist.restore_forward`
pub fn restore_forward<V: Versions>() {
    let seed = "alpha\nbeta\ngamma\n";
    let mut d = V::open(seed);
    let e = engine::<V>();
    let v0 = d.version(A);

    // a edits; v1; b deletes some of a's text and adds its own
    swap(&mut d, A, "beta", "BETA é");
    let t = d.text();
    edit(&mut d, A, vec![ins(t.len(), "delta 🎉\n")]);
    let v1 = d.version(A);
    let t1 = d.text();
    swap(&mut d, B, "BETA é\n", "");
    swap(&mut d, B, "alpha", "ALPHA");
    let v2 = d.version(B);
    let t2 = d.text();
    assert_eq!(t2, "ALPHA\ngamma\ndelta 🎉\n", "{e}");

    // restore v0 as r: one new change by r, the text is v0's again
    let c = restore(&mut d, R, &v0);
    assert_eq!(
        c,
        Some(Change {
            author: R,
            roots: vec![0]
        }),
        "{e}: a restore is one change by the restorer"
    );
    assert_eq!(d.text(), seed, "{e}");
    assert_eq!(d.text(), show(&d, &v0), "{e}");
    // history is not rewritten
    assert_eq!(show(&d, &v1), t1, "{e}: a restore rewrote version 1");
    assert_eq!(show(&d, &v2), t2, "{e}: a restore rewrote version 2");

    // restoring the version the doc is at does nothing
    let here = d.version(R);
    assert_eq!(
        restore(&mut d, R, &here),
        None,
        "{e}: restoring the current version"
    );
    assert_eq!(d.text(), seed, "{e}");

    // a restore is undone by restoring the version before it, by anyone
    let c = restore(&mut d, A, &v2);
    assert_eq!(c.map(|c| c.author), Some(A), "{e}");
    assert_eq!(d.text(), t2, "{e}");

    // restore v1 (text both deleted and live now), then keep editing on top of it
    let c = restore(&mut d, B, &v1);
    assert_eq!(c.map(|c| c.author), Some(B), "{e}");
    assert_eq!(d.text(), t1, "{e}");
    let t = d.text();
    edit(&mut d, A, vec![ins(t.len(), "epsilon\n")]);
    assert_eq!(d.text(), format!("{t1}epsilon\n"), "{e}");
    for (v, t) in [(&v0, seed), (&v1, t1.as_str()), (&v2, t2.as_str())] {
        assert_eq!(show(&d, v), t, "{e}: after restores");
    }
}

fn restored<V: Versions>(d: &mut V, v: &V::VersionId) -> Option<(Change, Vec<Hunk>)> {
    d.restore_applied(R, v)
        .unwrap_or_else(|e| panic!("{}: restore {v:?}: {e:?}", engine::<V>()))
}

/// The kit's edit pipeline (undo spec D1).
fn pipeline(live: &str, target: &str) -> Vec<Hunk> {
    align_insertions(live, diff::hunks(live, target))
}

/// `hist.restore_minimal`
pub fn restore_minimal<V: Versions>() {
    let e = engine::<V>();
    let first = "agent-1\nagent-17\n";
    let mut d = V::open(first);
    let v0 = d.version(R);
    // equal text: no events, whether the doc never moved or came back by hand
    assert_eq!(
        restored(&mut d, &v0),
        None,
        "{e}: restoring the current text"
    );
    edit(&mut d, A, vec![del(0, 8)]);
    edit(&mut d, B, vec![ins(0, "agent-1\n")]);
    assert_eq!(d.text(), first, "{e}");
    assert_eq!(
        restored(&mut d, &v0),
        None,
        "{e}: restoring a text the doc has again"
    );

    // otherwise the applied hunks are the pipeline's: here a line-aligned insertion where a
    // plain minimal diff would insert mid-line
    edit(&mut d, A, vec![del(0, 8)]);
    let live = d.text();
    assert_eq!(live, "agent-17\n", "{e}");
    let want = pipeline(&live, first);
    assert_eq!(want, vec![ins(0, "agent-1\n")], "the pipeline itself");
    assert_ne!(
        diff::hunks(&live, first),
        want,
        "the case must tell them apart"
    );
    let (c, applied) = restored(&mut d, &v0).unwrap_or_else(|| panic!("{e}: a restore"));
    assert_eq!(applied, want, "{e}: the applied hunks of a restore");
    assert_eq!(
        c,
        Change {
            author: R,
            roots: vec![0]
        },
        "{e}"
    );
    assert_eq!(d.text(), first, "{e}");

    // several hunks: two distant changes in one batch and an append by someone else
    let mut d = V::open("one\ntwo\nthree\nfour\nfive\n");
    let v = d.version(R);
    edit(&mut d, A, vec![rep(4, 7, "2"), rep(19, 23, "5")]);
    let end = d.text().len();
    edit(&mut d, B, vec![ins(end, "six\n")]);
    let live = d.text();
    assert_eq!(live, "one\n2\nthree\nfour\n5\nsix\n", "{e}");
    let want = pipeline(&live, "one\ntwo\nthree\nfour\nfive\n");
    assert!(want.len() >= 2, "{want:?}");
    let (_, applied) = restored(&mut d, &v).unwrap_or_else(|| panic!("{e}: a restore"));
    assert_eq!(applied, want, "{e}: the applied hunks of a restore");
    assert_eq!(d.text(), "one\ntwo\nthree\nfour\nfive\n", "{e}");
}
