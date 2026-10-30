//! Oracles for the text diff (`text::diff`, `text::lines`) and the three-way merge
//! (`text::merge`): what `write --base`, undo and outside edits rely on.
//!
//! - **diff:** `hunks(old, new)` is valid on `old` (sorted, in range, on char boundaries,
//!   never empty), canonical (no two hunks touch, none is a no-op), pure (the same twice),
//!   and applying it gives `new`; `align_insertions` keeps all of that and never makes
//!   hunks touch.
//! - **identities:** nothing changed on one side gives the other; the same change on both
//!   sides gives it once.
//! - **symmetry:** a clean merge is symmetric up to the order of two appends at the end of
//!   a non-empty text (O5: theirs first).
//! - **the end-of-text rule only adds clean merges:** a clean window merge is the clean
//!   whole-text merge.
//! - **a clean merge holds both sides' changes, exactly:** its text is the base with every
//!   hunk of theirs and every hunk of ours not identical to one of theirs applied, in base
//!   order (at one point: insertions before replacements, theirs' insertion before ours);
//!   its hunks are valid on theirs and give its text.
//! - **conflicts** are disjoint, in order, inside the base, on char boundaries, with
//!   `base_text` the base's bytes and sane line numbers.
//! - **hunk-set merge:** `merge3_hunks` on write --base's sides (`merge::write_base_sides`:
//!   whole lines on the closed texts, v0.9.7.1) is `merge3_window` on the closed texts (the
//!   window then decides the final "\n").
//! - **whole lines:** every line of a clean merge is a whole line of base, ours or theirs.

use k2_mpc_base::text::diff::{self, Hunk};
use k2_mpc_base::text::lines;
use k2_mpc_base::text::merge::{self, Clean, Merge};

pub fn check(base: &str, ours: &str, theirs: &str) {
    for (a, b) in [(base, ours), (base, theirs), (ours, theirs)] {
        check_diff(a, b);
    }
    // identities
    assert_eq!(
        clean(base, base, ours).text,
        ours,
        "nothing changed on ours"
    );
    assert_eq!(
        clean(base, ours, base).text,
        ours,
        "nothing changed on theirs"
    );
    assert_eq!(
        clean(base, ours, ours).text,
        ours,
        "the same change on both sides"
    );

    let whole = merge::merge3(base, ours, theirs);
    let swapped = merge::merge3(base, theirs, ours);
    check_shape(base, theirs, &whole);
    check_shape(base, ours, &swapped);
    let (sa, sb) = merge::write_base_sides(base, ours, theirs);
    for at_end in [false, true] {
        let w = merge::merge3_window(base, ours, theirs, at_end);
        check_shape(base, theirs, &w);
        let sets = merge::merge3_hunks(&merge::closed(base), &sa, &sb, at_end);
        match (&sets, &w) {
            (Merge::Clean(s), Merge::Clean(c)) => assert_eq!(
                s.text,
                merge::closed(&c.text),
                "merge3_hunks on write --base's sides is merge3_window (at_end {at_end})"
            ),
            (Merge::Conflicts(s), Merge::Conflicts(c)) => assert_eq!(s.len(), c.len()),
            _ => panic!("merge3_hunks on write --base's sides and merge3_window disagree (at_end {at_end}): {sets:?} vs {w:?}"),
        }
        if let Merge::Clean(c) = &w {
            check_union(base, ours, theirs, at_end, c);
            check_whole_lines(base, ours, theirs, &c.text);
        }
    }
    // the end-of-text rule only adds clean merges
    if let Merge::Clean(w) = merge::merge3_window(base, ours, theirs, false) {
        match &whole {
            Merge::Clean(c) => assert_eq!(c.text, w.text, "O5 changed a clean merge"),
            Merge::Conflicts(c) => panic!("O5 turned a clean merge into conflicts: {c:?}"),
        }
    }
    // symmetric up to the order of two appends at the end
    if let (Merge::Clean(x), Merge::Clean(y)) = (&whole, &swapped) {
        let cb = merge::closed(base);
        let eof = |new: &str| {
            diff::whole_line_hunks(&cb, &merge::closed(new))
                .pop()
                .filter(|h| h.is_insertion() && h.start == cb.len() && !cb.is_empty())
                .map(|h| h.insert)
        };
        let (xt, yt) = (merge::closed(&x.text), merge::closed(&y.text));
        match (eof(ours), eof(theirs)) {
            (Some(eo), Some(et)) if eo != et => {
                let tail = eo.len() + et.len();
                assert!(
                    xt.ends_with(&format!("{et}{eo}")),
                    "theirs' append first: {:?}",
                    x.text
                );
                assert!(yt.ends_with(&format!("{eo}{et}")), "swapped: {:?}", y.text);
                assert_eq!(
                    &xt[..xt.len() - tail],
                    &yt[..yt.len() - tail],
                    "symmetric before the appends"
                );
            }
            _ => assert_eq!(x.text, y.text, "a clean merge is symmetric"),
        }
    } else {
        assert_eq!(
            matches!(whole, Merge::Clean(_)),
            matches!(swapped, Merge::Clean(_)),
            "clean one way, conflicts the other"
        );
    }
}

fn clean(base: &str, ours: &str, theirs: &str) -> Clean {
    match merge::merge3(base, ours, theirs) {
        Merge::Clean(c) => c,
        Merge::Conflicts(c) => panic!("expected a clean merge: {c:?}"),
    }
}

fn check_canonical(old: &str, h: &[Hunk], what: &str) {
    diff::validate(old, h).unwrap_or_else(|e| panic!("{what}: invalid hunks {h:?}: {e}"));
    for w in h.windows(2) {
        assert!(w[0].end < w[1].start, "{what}: hunks touch: {h:?}");
    }
    for x in h {
        assert_ne!(&old[x.start..x.end], x.insert, "{what}: a no-op hunk {x:?}");
    }
}

fn check_diff(old: &str, new: &str) {
    let h = diff::hunks(old, new);
    check_canonical(old, &h, "hunks");
    assert_eq!(
        diff::apply(old, &h),
        new,
        "applying hunks(old, new) gives new"
    );
    assert_eq!(diff::hunks(old, new), h, "hunks is pure");
    assert_eq!(
        h.is_empty(),
        old == new,
        "no hunks exactly when nothing changed"
    );
    let a = lines::align_insertions(old, h.clone());
    assert_eq!(a.len(), h.len(), "align_insertions keeps the hunks");
    check_canonical(old, &a, "align_insertions");
    assert_eq!(diff::apply(old, &a), new, "aligned hunks give new");
}

fn check_shape(base: &str, theirs: &str, m: &Merge) {
    match m {
        Merge::Clean(c) => {
            diff::validate(theirs, &c.hunks)
                .unwrap_or_else(|e| panic!("clean hunks invalid on theirs: {:?}: {e}", c.hunks));
            assert_eq!(
                diff::apply(theirs, &c.hunks),
                c.text,
                "clean hunks give the text"
            );
        }
        Merge::Conflicts(cs) => {
            assert!(!cs.is_empty(), "conflicts, but none listed");
            let mut prev_end = None;
            for c in cs {
                let r = &c.base;
                assert!(
                    r.start <= r.end && r.end <= base.len(),
                    "conflict outside the base: {c:?}"
                );
                assert!(
                    base.is_char_boundary(r.start) && base.is_char_boundary(r.end),
                    "conflict off a char boundary: {c:?}"
                );
                assert_eq!(c.base_text, &base[r.clone()], "conflict base_text");
                let line = base[..r.start].matches('\n').count() + 1;
                assert_eq!(c.lines.0, line, "conflict start line: {c:?}");
                assert!(c.lines.0 <= c.lines.1, "conflict lines: {c:?}");
                if let Some(e) = prev_end {
                    assert!(
                        e <= r.start,
                        "conflicts out of order or overlapping: {cs:?}"
                    );
                }
                prev_end = Some(r.end);
            }
        }
    }
}

/// A clean merge's text is every change of theirs and every change of ours (but those
/// identical to one of theirs) applied to the base, in base order.
fn check_union(base: &str, ours: &str, theirs: &str, at_end: bool, c: &Clean) {
    let (a, b) = merge::write_base_sides(base, ours, theirs);
    let base = &*merge::closed(base);
    // (hunk, rank at one point: 0 theirs' insertion, 1 ours' insertion, 2 a replacement)
    let mut all: Vec<(&Hunk, u8)> = b
        .iter()
        .map(|h| (h, if h.is_insertion() { 0 } else { 2 }))
        .collect();
    for h in &a {
        if !b.contains(h) {
            all.push((h, if h.is_insertion() { 1 } else { 2 }));
        }
    }
    all.sort_by_key(|(h, rank)| (h.start, *rank));
    let mut out = String::new();
    let mut at = 0;
    for (h, _) in &all {
        assert!(
            h.start >= at,
            "a clean merge (at_end {at_end}) of overlapping changes: ours {a:?}, theirs {b:?}"
        );
        out.push_str(&base[at..h.start]);
        out.push_str(&h.insert);
        at = h.end;
    }
    out.push_str(&base[at..]);
    assert_eq!(
        merge::closed(&c.text),
        out,
        "a clean merge (at_end {at_end}) holds both sides' changes: ours {a:?}, theirs {b:?}"
    );
}

/// Every line of a clean `write --base` merge is a whole line of base, ours or theirs
/// (v0.9.7.1: no merge inside a line, so no splice and no join).
fn check_whole_lines(base: &str, ours: &str, theirs: &str, result: &str) {
    let key = |l: &str| l.strip_suffix('\n').unwrap_or(l).to_owned();
    let known: std::collections::HashSet<String> = [base, ours, theirs]
        .iter()
        .flat_map(|t| t.split_inclusive('\n'))
        .map(key)
        .collect();
    for l in result.split_inclusive('\n') {
        assert!(
            known.contains(&key(l)),
            "a clean merge holds a line no one wrote: {l:?} in {result:?}"
        );
    }
}
