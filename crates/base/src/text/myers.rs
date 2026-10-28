//! Myers' diff (linear space, divide and conquer on the middle snake) over tokens, bounded
//! by a work budget instead of a clock.
//!
//! A wall-clock deadline makes a diff depend on machine load: the same `(old, new)` gives
//! fine hunks on an idle machine and a coarse replace on a busy one. The three-way merge
//! compares hunks from two diffs of the same base, so it needs the diff to be a pure
//! function of its input. Here the bound is counted work, never time.
//!
//! One work unit is one token comparison, one diagonal step or one slot of a search array.
//! Tokens are interned to integers first (O(input) hashing), so a unit is O(1). When the
//! budget runs out, every sub-problem not yet solved becomes a single replacement: the
//! result is still a correct diff, just coarser, and the same one every time.
//!
//! Total work for one [`diff`] is at most `budget + O(n + m)`: each sub-problem spends at
//! least one unit, one started with budget left can overshoot by at most its own size
//! (a final snake or trim), and once the budget is spent each remaining sub-problem costs
//! O(1).

use std::collections::HashMap;
use std::ops::Range;

/// Units given to every diff, however small its input.
const BUDGET_BASE: usize = 1 << 22;
/// Extra units per input byte, so large inputs with small changes stay exact while the worst
/// case (Myers is O((n + m) * D)) stays linear in the input.
const BUDGET_PER_BYTE: usize = 16;

/// A changed stretch: tokens `old` were replaced by tokens `new`. Never both empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Change {
    pub old: Range<usize>,
    pub new: Range<usize>,
}

/// Work left for a diff, shared by every diff run for one input (see the module docs).
#[derive(Debug)]
pub(crate) struct Budget {
    left: usize,
    spent: usize,
}

impl Budget {
    pub(crate) fn new(units: usize) -> Self {
        Budget {
            left: units,
            spent: 0,
        }
    }

    /// The budget for diffing inputs of `bytes` bytes in total.
    pub(crate) fn for_input(bytes: usize) -> Self {
        Self::new(BUDGET_BASE.saturating_add(BUDGET_PER_BYTE.saturating_mul(bytes)))
    }

    fn spend(&mut self, units: usize) {
        self.left = self.left.saturating_sub(units);
        self.spent = self.spent.saturating_add(units);
    }

    fn exhausted(&self) -> bool {
        self.left == 0
    }

    #[cfg(test)]
    pub(crate) fn spent(&self) -> usize {
        self.spent
    }
}

/// The changes turning `old` into `new`, in order, never touching each other. Minimal (by
/// tokens inserted plus deleted) while the budget lasts.
pub(crate) fn diff(old: &[&str], new: &[&str], budget: &mut Budget) -> Vec<Change> {
    let (a, b) = intern(old, new);
    let mut out = Vec::new();
    conquer(&a, 0, &b, 0, budget, &mut out);
    out
}

/// Token ids in order of first appearance: deterministic whatever the hasher's seed.
fn intern<'a>(old: &[&'a str], new: &[&'a str]) -> (Vec<u32>, Vec<u32>) {
    let mut ids: HashMap<&'a str, u32> = HashMap::with_capacity(old.len() + new.len());
    let mut id = |t: &'a str| {
        let next = ids.len() as u32;
        *ids.entry(t).or_insert(next)
    };
    let a = old.iter().map(|t| id(t)).collect();
    let b = new.iter().map(|t| id(t)).collect();
    (a, b)
}

fn push(out: &mut Vec<Change>, old: Range<usize>, new: Range<usize>) {
    if old.is_empty() && new.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut() {
        if last.old.end == old.start && last.new.end == new.start {
            last.old.end = old.end;
            last.new.end = new.end;
            return;
        }
    }
    out.push(Change { old, new });
}

/// Diff `a` (at token `ao` of old) against `b` (at token `bo` of new).
fn conquer(a: &[u32], ao: usize, b: &[u32], bo: usize, budget: &mut Budget, out: &mut Vec<Change>) {
    if budget.exhausted() {
        return push(out, ao..ao + a.len(), bo..bo + b.len());
    }
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let (a, b, ao, bo) = (&a[pre..], &b[pre..], ao + pre, bo + pre);
    let suf = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a, b) = (&a[..a.len() - suf], &b[..b.len() - suf]);
    budget.spend(pre + suf + 1);
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 {
        return push(out, ao..ao + n, bo..bo + m);
    }
    match middle(a, b, budget) {
        // A split strictly inside the box: both halves are smaller, so this terminates.
        Some((x, y)) if x <= n && y <= m && (x, y) != (0, 0) && (x, y) != (n, m) => {
            conquer(&a[..x], ao, &b[..y], bo, budget, out);
            conquer(&a[x..], ao + x, &b[y..], bo + y, budget, out);
        }
        _ => push(out, ao..ao + n, bo..bo + m),
    }
}

/// Where an optimal path crosses from the forward search into the backward one, found by
/// running both at once until they overlap. `a` and `b` are non-empty and share no first or
/// last token. `None` when the budget runs out first.
fn middle(a: &[u32], b: &[u32], budget: &mut Budget) -> Option<(usize, usize)> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max_d = (n + m + 1) / 2;
    let off = max_d;
    let len = 2 * max_d as usize + 2;
    budget.spend(2 * len);
    if budget.exhausted() {
        return None;
    }
    // vf[off + k]: furthest x on diagonal k (x - y = k) from (0, 0);
    // vb[off + k]: furthest x on diagonal k from (n, m), measured backwards.
    let mut vf = vec![-1isize; len];
    let mut vb = vec![-1isize; len];
    vf[off as usize + 1] = 0;
    vb[off as usize + 1] = 0;
    let delta = n - m;
    // With an odd delta the forward search is the one to meet the backward one.
    let front = delta % 2 != 0;
    // Diagonals that left the box are not walked again.
    let (mut k1start, mut k1end, mut k2start, mut k2end) = (0, 0, 0, 0);
    for d in 0..max_d {
        let mut k1 = -d + k1start;
        while k1 <= d - k1end {
            if budget.exhausted() {
                return None;
            }
            let i = (off + k1) as usize;
            let mut x = if k1 == -d || (k1 != d && vf[i - 1] < vf[i + 1]) {
                vf[i + 1]
            } else {
                vf[i - 1] + 1
            };
            let mut y = x - k1;
            let x0 = x;
            while x >= 0 && y >= 0 && x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            budget.spend(1 + (x - x0) as usize);
            vf[i] = x;
            if x > n {
                k1end += 2;
            } else if y > m {
                k1start += 2;
            } else if front {
                let j = off + delta - k1;
                if j >= 0 && (j as usize) < len && vb[j as usize] != -1 && x >= n - vb[j as usize] {
                    return (y >= 0).then_some((x as usize, y as usize));
                }
            }
            k1 += 2;
        }
        let mut k2 = -d + k2start;
        while k2 <= d - k2end {
            if budget.exhausted() {
                return None;
            }
            let i = (off + k2) as usize;
            let mut x = if k2 == -d || (k2 != d && vb[i - 1] < vb[i + 1]) {
                vb[i + 1]
            } else {
                vb[i - 1] + 1
            };
            let mut y = x - k2;
            let x0 = x;
            while x >= 0
                && y >= 0
                && x < n
                && y < m
                && a[(n - x - 1) as usize] == b[(m - y - 1) as usize]
            {
                x += 1;
                y += 1;
            }
            budget.spend(1 + (x - x0) as usize);
            vb[i] = x;
            if x > n {
                k2end += 2;
            } else if y > m {
                k2start += 2;
            } else if !front {
                let j = off + delta - k2;
                if j >= 0 && (j as usize) < len && vf[j as usize] != -1 {
                    let x1 = vf[j as usize];
                    let y1 = x1 - (j - off);
                    if x1 >= n - x {
                        return (x1 >= 0 && y1 >= 0).then_some((x1 as usize, y1 as usize));
                    }
                }
            }
            k2 += 2;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn chars(s: &str) -> Vec<&str> {
        s.char_indices()
            .map(|(i, c)| &s[i..i + c.len_utf8()])
            .collect()
    }

    /// Rebuild `new` from `old` and the changes; panics if they are out of order or touch.
    fn rebuild(old: &[&str], new: &[&str], changes: &[Change]) -> Vec<String> {
        let mut out = Vec::new();
        let (mut oi, mut ni) = (0, 0);
        for (k, c) in changes.iter().enumerate() {
            assert!(
                !(c.old.is_empty() && c.new.is_empty()),
                "empty change {c:?}"
            );
            if k > 0 {
                assert!(
                    c.old.start > oi || c.new.start > ni,
                    "changes touch: {changes:?}"
                );
            }
            assert_eq!(c.old.start - oi, c.new.start - ni, "equal runs must match");
            out.extend(old[oi..c.old.start].iter().map(|s| s.to_string()));
            out.extend(new[c.new.clone()].iter().map(|s| s.to_string()));
            oi = c.old.end;
            ni = c.new.end;
        }
        out.extend(old[oi..].iter().map(|s| s.to_string()));
        out
    }

    fn lcs(a: &[&str], b: &[&str]) -> usize {
        let mut row = vec![0usize; b.len() + 1];
        for x in a {
            let mut diag = 0;
            for (j, y) in b.iter().enumerate() {
                let up = row[j + 1];
                row[j + 1] = if x == y { diag + 1 } else { up.max(row[j]) };
                diag = up;
            }
        }
        row[b.len()]
    }

    #[test]
    fn small_cases() {
        for (o, n) in [
            ("", "abc"),
            ("abc", ""),
            ("abc", "abc"),
            ("abcabba", "cbabac"),
            ("brown", "red"),
            ("ab", "ba"),
        ] {
            let (o, n) = (chars(o), chars(n));
            let c = diff(&o, &n, &mut Budget::for_input(64));
            assert_eq!(
                rebuild(&o, &n, &c),
                n.iter().map(|s| s.to_string()).collect::<Vec<_>>()
            );
            let cost: usize = c.iter().map(|c| c.old.len() + c.new.len()).sum();
            assert_eq!(cost, o.len() + n.len() - 2 * lcs(&o, &n), "{c:?}");
        }
    }

    /// No clock anywhere: a worst case for Myers (nothing in common, or every token in
    /// common but in reverse order) stops within budget + O(n + m) units, and the coarse
    /// answer is still a correct diff.
    #[test]
    fn pathological_input_stops_within_the_work_bound() {
        let old: Vec<String> = (0..50_000).map(|i| format!("{i}\n")).collect();
        let reversed: Vec<String> = old.iter().rev().cloned().collect();
        let disjoint: Vec<String> = (0..50_000).map(|i| format!("x{i}\n")).collect();
        let o: Vec<&str> = old.iter().map(String::as_str).collect();
        for new in [&reversed, &disjoint] {
            let n: Vec<&str> = new.iter().map(String::as_str).collect();
            let units = 1 << 20;
            let mut budget = Budget::new(units);
            let c = diff(&o, &n, &mut budget);
            assert!(budget.exhausted(), "the case must actually hit the bound");
            let bound = units + 4 * (o.len() + n.len()) + 16;
            assert!(
                budget.spent() <= bound,
                "spent {} > {bound}",
                budget.spent()
            );
            assert_eq!(rebuild(&o, &n, &c), *new);
            // and again: the same answer
            assert_eq!(diff(&o, &n, &mut Budget::new(units)), c);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2000))]
        #[test]
        fn minimal_and_correct(o in "[abc]{0,30}", n in "[abc]{0,30}") {
            let (o, n) = (chars(&o), chars(&n));
            let c = diff(&o, &n, &mut Budget::for_input(64));
            prop_assert_eq!(rebuild(&o, &n, &c), n.iter().map(|s| s.to_string()).collect::<Vec<_>>());
            let cost: usize = c.iter().map(|c| c.old.len() + c.new.len()).sum();
            prop_assert_eq!(cost, o.len() + n.len() - 2 * lcs(&o, &n));
        }

        #[test]
        fn any_budget_gives_a_correct_diff(o in "[abc]{0,30}", n in "[abc]{0,30}", units in 0usize..400) {
            let (o, n) = (chars(&o), chars(&n));
            let mut budget = Budget::new(units);
            let c = diff(&o, &n, &mut budget);
            prop_assert_eq!(rebuild(&o, &n, &c), n.iter().map(|s| s.to_string()).collect::<Vec<_>>());
            prop_assert!(budget.spent() <= units + 4 * (o.len() + n.len()) + 16);
        }
    }
}
