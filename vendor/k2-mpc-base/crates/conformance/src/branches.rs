//! Rules over [`Branches`] and [`BranchAt`] (replicas).
//!
//! - `hist.branch_merge` (E-HIST-8, path (a)): a branch's edits integrate by CRDT merge:
//!   merging equals delivering both sides' events directly to a replica of the base, never
//!   conflicts (both appends at the end land), is idempotent, and works both ways and from
//!   a branch of a branch.
//! - `hist.merge_any_base` (E-HIST-9): the same from a branch rooted at any past version,
//!   while the main line has moved on (deleted the text the branch edits, inserted around
//!   it), and from a past version of a branch; merge order doesn't matter.

use crate::caps::{BranchAt, Branches, Events};
use crate::{del, ins, rep, Hunk, TextReplica};

/// Every rule over [`Branches`], by id.
pub const RULES: &[&str] = &["hist.branch_merge"];

/// Every rule over [`BranchAt`], by id.
pub const AT_RULES: &[&str] = &["hist.merge_any_base"];

/// Run rule `id` on engine `E`. An unknown id panics.
pub fn run<E: Branches>(id: &str) {
    match id {
        "hist.branch_merge" => branch_merge::<E>(),
        _ => panic!("no branches rule {id:?}"),
    }
}

/// Run rule `id` on engine `E`. An unknown id panics.
pub fn run_at<E: BranchAt>(id: &str) {
    match id {
        "hist.merge_any_base" => merge_any_base::<E>(),
        _ => panic!("no branch-at rule {id:?}"),
    }
}

fn edit<E: TextReplica>(r: &mut E, hunks: Vec<Hunk>) {
    if let Err(e) = r.apply(&hunks) {
        panic!("{}: refused {hunks:?}: {e:?}", E::ENGINE);
    }
}

fn deliver<E: Events>(r: &mut E, b: &E::Batch, what: &str) {
    if let Err(e) = r.deliver(b) {
        panic!("{}: {what}: refused with {e:?}", E::ENGINE);
    }
}

fn merge<E: Branches>(into: &mut E, from: &E, what: &str) {
    if let Err(e) = into.merge(from) {
        panic!("{}: merge {what}: refused with {e:?}", E::ENGINE);
    }
}

fn snap<E: TextReplica>(r: &E) -> (String, E::Version) {
    (r.text(), r.version())
}

/// `r`'s text with `old` (first occurrence) replaced by `new`, as hunks.
fn swap<E: TextReplica>(r: &E, old: &str, new: &str) -> Vec<Hunk> {
    let t = r.text();
    let at = t
        .find(old)
        .unwrap_or_else(|| panic!("{}: {old:?} not in {t:?}", E::ENGINE));
    vec![rep(at, at + old.len(), new)]
}

fn end<E: TextReplica>(r: &E) -> usize {
    r.text().len()
}

/// `hist.branch_merge`
pub fn branch_merge<E: Branches>() {
    let seed = "line one\nline two\n";
    let mut main = E::replica(101, seed);
    let v0 = main.version();
    let mut br = main.branch(201);
    assert_eq!(
        snap(&br),
        snap(&main),
        "{}: a branch holds its base",
        E::ENGINE
    );

    // both sides edit a line each, and both append at the end
    let h = swap(&main, "one", "ONE");
    edit(&mut main, h);
    let n = end(&main);
    edit(&mut main, vec![ins(n, "main end\n")]);
    let n = br.text().find("two").expect("two") + 3;
    edit(&mut br, vec![ins(n, " é")]);
    let n = end(&br);
    edit(&mut br, vec![ins(n, "branch end 🎉\n")]);
    let (em, eb) = (main.events_since(&v0), br.events_since(&v0));

    // merge = delivering both sides' events directly
    let mut direct = E::replica(103, seed);
    deliver(&mut direct, &em, "main's events");
    deliver(&mut direct, &eb, "the branch's events");
    merge(&mut main, &br, "the branch into main");
    assert_eq!(
        snap(&main),
        snap(&direct),
        "{}: merge vs direct delivery",
        E::ENGINE
    );
    let t = main.text();
    assert!(
        t.contains("main end\n")
            && t.contains("branch end 🎉\n")
            && t.starts_with("line ONE\nline two é\n"),
        "{}: an edit was lost: {t:?}",
        E::ENGINE
    );
    // idempotent, and the other way
    let merged = snap(&main);
    merge(&mut main, &br, "the branch again");
    assert_eq!(
        snap(&main),
        merged,
        "{}: a second merge changed main",
        E::ENGINE
    );
    merge(&mut br, &main, "main into the branch");
    assert_eq!(snap(&br), merged, "{}: merged both ways differ", E::ENGINE);

    // a branch of a branch, merged straight into main
    let mut sub = br.branch(202);
    edit(&mut br, vec![ins(0, "> ")]);
    let h = swap(&sub, "line two", "line 2");
    edit(&mut sub, h);
    merge(&mut main, &sub, "a branch of a branch into main");
    merge(&mut main, &br, "the branch again, after");
    merge(&mut br, &main, "back");
    merge(&mut sub, &main, "back");
    assert_eq!(snap(&main), snap(&br), "{}", E::ENGINE);
    assert_eq!(snap(&main), snap(&sub), "{}", E::ENGINE);
    let t = main.text();
    assert!(
        t.starts_with("> line ONE\nline 2 é\n"),
        "{}: {t:?}",
        E::ENGINE
    );
}

/// `hist.merge_any_base`
pub fn merge_any_base<E: BranchAt>() {
    let seed = "a\nbb\nccc\ndddd\n";
    let mut main = E::replica(101, seed);
    let v0 = main.version();
    let h = swap(&main, "bb", "BB é");
    edit(&mut main, h);
    let v1 = main.version();
    let t1 = main.text();
    // main moves on: deletes the line a branch from v0 edits, inserts around another
    let h = swap(&main, "BB é\n", "");
    edit(&mut main, h);
    let h = swap(&main, "ccc", "<ccc>");
    edit(&mut main, h);
    let n = end(&main);
    edit(&mut main, vec![ins(n, "main 🎉\n")]);

    // branches at the past versions hold exactly those states
    let at = |r: &E, v: &E::Version, agent: u64| -> E {
        r.branch_at(v, agent)
            .unwrap_or_else(|e| panic!("{}: branch at {v:?}: {e:?}", E::ENGINE))
    };
    let mut b0 = at(&main, &v0, 201);
    let mut b1 = at(&main, &v1, 202);
    assert_eq!(b0.text(), seed, "{}: a branch at v0", E::ENGINE);
    assert_eq!(b1.text(), t1, "{}: a branch at v1", E::ENGINE);
    let (vb0, vb1) = (b0.version(), b1.version());
    // b0 edits inside the text main deleted, and appends; b1 deletes what main changed
    let h = swap(&b0, "bb", "b-b");
    edit(&mut b0, h);
    let n = end(&b0);
    edit(&mut b0, vec![ins(n, "b0\n")]);
    let t = b1.text();
    let c = t.find("ccc").expect("ccc");
    edit(&mut b1, vec![ins(0, "b1 "), del(c, c + 2)]);
    // a branch from a past version of b0
    let mut b00 = at(&b0, &vb0, 203);
    assert_eq!(b00.text(), seed, "{}", E::ENGINE);
    edit(&mut b00, vec![ins(1, "!")]);

    // direct delivery of every side's events to a replica of the seed
    let mut direct = E::replica(104, seed);
    for (what, b) in [
        ("main", main.events_since(&v0)),
        ("b0", b0.events_since(&vb0)),
        ("b1", b1.events_since(&vb1)),
        ("b00", b00.events_since(&vb0)),
    ] {
        deliver(&mut direct, &b, what);
    }
    // a second copy of main, merged in another order
    let mut other = E::replica(105, seed);
    deliver(&mut other, &main.events_since(&v0), "main to its copy");

    for (what, b) in [("b0", &b0), ("b1", &b1), ("b00", &b00)] {
        merge(&mut main, b, what);
    }
    for (what, b) in [("b00", &b00), ("b1", &b1), ("b0", &b0)] {
        merge(&mut other, b, what);
    }
    assert_eq!(
        snap(&main),
        snap(&direct),
        "{}: merge vs direct delivery",
        E::ENGINE
    );
    assert_eq!(
        snap(&other),
        snap(&main),
        "{}: merge order mattered",
        E::ENGINE
    );
    let merged = snap(&main);
    for (what, b) in [("b0", &b0), ("b1", &b1), ("b00", &b00)] {
        merge(&mut main, b, what);
        assert_eq!(snap(&main), merged, "{}: merging {what} again", E::ENGINE);
    }
    let t = main.text();
    for want in ["b1 ", "a!", "b0\n", "main 🎉\n"] {
        assert!(t.contains(want), "{}: {want:?} lost: {t:?}", E::ENGINE);
    }
    // the branches catch up
    for b in [&mut b0, &mut b1, &mut b00] {
        merge(b, &main, "main into a branch");
        assert_eq!(snap(b), merged, "{}", E::ENGINE);
    }
}
