//! What an edit changed, in lines: the ranges to flash and the `(+added -removed)` summary.

use super::position::LineRange;
use crate::text::diff::{self, Hunk};
use crate::text::myers::{self, Budget};

fn newlines(b: &[u8]) -> usize {
    b.iter().filter(|&&c| c == b'\n').count()
}

/// Lines of `after` touched by `hunks` (in `before` coordinates), merged when they touch.
/// A pure deletion marks the line where it happened.
pub(crate) fn changed_lines(root: &str, after: &str, hunks: &[Hunk]) -> Vec<LineRange> {
    let a = after.as_bytes();
    let mut out: Vec<LineRange> = Vec::new();
    let (mut delta, mut counted, mut line) = (0isize, 0usize, 1usize);
    for h in hunks {
        let ns = (h.start as isize + delta) as usize;
        let ne = ns + h.insert.len();
        delta += h.insert.len() as isize - (h.end - h.start) as isize;
        line += newlines(&a[counted..ns]);
        counted = ns;
        let last = if ne > ns {
            line + newlines(&a[ns..ne - 1])
        } else {
            line
        };
        match out.last_mut() {
            Some(prev) if prev.last + 1 >= line => prev.last = prev.last.max(last),
            _ => out.push(LineRange {
                root: root.to_owned(),
                first: line,
                last,
            }),
        }
    }
    out
}

/// `(lines added, lines removed)` as a line diff (`diff -u`) would count them, computed only
/// over the lines the hunks touch.
pub(crate) fn line_counts(before: &str, after: &str, hunks: &[Hunk]) -> (usize, usize) {
    let b = before.as_bytes();
    let line_start = |p: usize| {
        b[..p]
            .iter()
            .rposition(|&c| c == b'\n')
            .map_or(0, |i| i + 1)
    };
    let line_end = |p: usize| {
        b[p..]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(b.len(), |i| p + i + 1)
    };
    // groups of hunks over disjoint line spans: (old start, old end, shift before, own shift)
    let mut groups: Vec<(usize, usize, isize, isize)> = Vec::new();
    let mut delta = 0isize;
    for h in hunks {
        let s = line_start(h.start);
        let e = line_end(if h.end > h.start { h.end - 1 } else { h.start });
        let grow = h.insert.len() as isize - (h.end - h.start) as isize;
        match groups.last_mut() {
            Some(g) if s < g.1 => {
                g.1 = g.1.max(e);
                g.3 += grow;
            }
            _ => groups.push((s, e, delta, grow)),
        }
        delta += grow;
    }
    let (mut added, mut removed) = (0, 0);
    // One work budget for the whole count, so it is deterministic and O(input) (see text/myers.rs).
    let mut budget = Budget::for_input(before.len() + after.len());
    for (s, e, d0, d) in groups {
        let old = &before[s..e];
        let new = &after[(s as isize + d0) as usize..(e as isize + d0 + d) as usize];
        for c in myers::diff(&diff::lines(old), &diff::lines(new), &mut budget) {
            added += c.new.len();
            removed += c.old.len();
        }
    }
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::diff;

    fn sum(before: &str, after: &str) -> (Vec<(usize, usize)>, (usize, usize)) {
        let hs = diff::hunks(before, after);
        let ch = changed_lines("r", after, &hs)
            .into_iter()
            .map(|r| (r.first, r.last))
            .collect();
        (ch, line_counts(before, after, &hs))
    }

    #[test]
    fn counts_like_diff() {
        assert_eq!(sum("a\nb\nc\n", "a\nB\nc\n"), (vec![(2, 2)], (1, 1)));
        assert_eq!(sum("a\nb\nc\n", "a\nb\nx\ny\nc\n"), (vec![(3, 4)], (2, 0)));
        assert_eq!(sum("a\nb\nc\n", "a\nc\n"), (vec![(2, 2)], (0, 1)));
        assert_eq!(
            sum("a\nb\nc\nd\ne\n", "A\nb\nc\nd\nE\n"),
            (vec![(1, 1), (5, 5)], (2, 2))
        );
        assert_eq!(sum("word here\n", "word there\n"), (vec![(1, 1)], (1, 1)));
        assert_eq!(sum("", "x\ny\n"), (vec![(1, 2)], (2, 0)));
        assert_eq!(
            sum("x", "x\ny"),
            (vec![(1, 2)], (2, 1)),
            "no final newline: like diff -u"
        );
    }
}
