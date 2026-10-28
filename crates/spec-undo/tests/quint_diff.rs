//! The Quint model's diff (`spec/quint/undo_diff.qnt`) is the kit's `diff::hunks` (spec
//! D1–D2) on every text the model diffs. [`qdiff`] transcribes the Quint module definition by
//! definition; it equals `diff::hunks` on **every** pair of texts over `{a, b}` up to
//! [`DIFF_MAX`] long, and differs at `DIFF_MAX + 1` (so `DIFF_MAX` is a hard limit). The
//! model's cut keeps every text it diffs (every region's before, after and window) within
//! `MAXLEN` (`bounded::tests::the_cut_bounds_every_diffed_string`), and every instance has
//! `MAXLEN ≤ DIFF_MAX`, so on every state a bound reaches the model's diff **is**
//! `diff::hunks`. `align_insertions` is not involved: the merge (U10, §4) diffs with
//! `diff::hunks` alone, and the bounded world's edits are explicit hunks.

use k2_mpc_base::text::diff;

/// `DIFF_MAX` in `undo_diff.qnt`.
const DIFF_MAX: usize = 4;

type H = (usize, usize, Vec<u8>);

fn prefix_len(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

fn suffix_len(a: &[u8], b: &[u8]) -> usize {
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

/// `embed`: the right-greedy embedding of `short` in `long`, and what it couldn't place.
fn embed(long: &[u8], short: &[u8]) -> (Vec<bool>, usize) {
    let mut keep = vec![false; long.len()];
    let mut j = short.len();
    for i in (0..long.len()).rev() {
        if j > 0 && long[i] == short[j - 1] {
            keep[i] = true;
            j -= 1;
        }
    }
    (keep, j)
}

/// `hunks` of `undo_diff.qnt`.
fn qdiff(old: &[u8], new: &[u8]) -> Vec<H> {
    if old == new {
        return Vec::new();
    }
    let p = prefix_len(old, new);
    let (o1, n1) = (&old[p..], &new[p..]);
    let s = suffix_len(o1, n1);
    let (om, nm) = (&o1[..o1.len() - s], &n1[..n1.len() - s]);
    let whole = vec![(p, p + om.len(), nm.to_vec())];
    if om.is_empty() || nm.is_empty() {
        return whole;
    }
    let del = om.len() > nm.len();
    let (long, short) = if del { (om, nm) } else { (nm, om) };
    let (keep, left) = embed(long, short);
    if left > 0 {
        return whole;
    }
    let (mut out, mut si, mut in_run, mut st) = (Vec::new(), 0, false, 0);
    for i in 0..=long.len() {
        let gap = i < long.len() && !keep[i];
        if in_run && !gap {
            out.push(if del {
                (p + st, p + i, Vec::new())
            } else {
                (p + si, p + si, long[st..i].to_vec())
            });
        }
        if i < long.len() && !gap {
            si += 1;
        }
        if gap && !in_run {
            st = i;
        }
        in_run = gap;
    }
    out
}

/// Every text over `{a, b}` up to `max` long.
fn texts(max: usize) -> Vec<Vec<u8>> {
    let mut out = vec![Vec::new()];
    let mut k = 0;
    while out.len() < (1 << (max + 1)) - 1 {
        for c in [b'a', b'b'] {
            let mut w = out[k].clone();
            w.push(c);
            out.push(w);
        }
        k += 1;
    }
    out
}

fn kit(a: &[u8], b: &[u8]) -> Vec<H> {
    let (a, b) = (
        std::str::from_utf8(a).unwrap(),
        std::str::from_utf8(b).unwrap(),
    );
    diff::hunks(a, b)
        .into_iter()
        .map(|h| (h.start, h.end, h.insert.into_bytes()))
        .collect()
}

#[test]
fn the_quint_diff_is_diff_hunks_up_to_diff_max() {
    let ts = texts(DIFF_MAX);
    assert_eq!(ts.len(), 31);
    for a in &ts {
        for b in &ts {
            assert_eq!(
                qdiff(a, b),
                kit(a, b),
                "{:?} -> {:?}",
                std::str::from_utf8(a),
                std::str::from_utf8(b)
            );
        }
    }
}

/// `DIFF_MAX` is the limit: one letter more and, on some pairs, Myers keeps other characters
/// than the right-greedy embedding does.
#[test]
fn the_quint_diff_differs_past_diff_max() {
    let ts = texts(DIFF_MAX + 1);
    let differ = ts
        .iter()
        .flat_map(|a| ts.iter().map(move |b| (a, b)))
        .filter(|(a, b)| qdiff(a, b) != kit(a, b))
        .count();
    assert!(differ > 0);
}

/// `undo_diff.qnt` says what this file checks.
#[test]
fn the_quint_module_names_the_limit() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/quint/undo_diff.qnt");
    let q = std::fs::read_to_string(path).expect("undo_diff.qnt");
    assert!(q.contains(&format!("pure val DIFF_MAX: int = {DIFF_MAX}")));
}
