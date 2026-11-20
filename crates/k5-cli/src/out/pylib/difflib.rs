//! Python 3.12's `difflib.unified_diff(a, b, fromfile, tofile, n=ctx)`
//! with its default `SequenceMatcher(None, a, b)` (autojunk on: in a `b`
//! of 200 lines or more, a line found more than `len(b) // 100 + 1`
//! times is never the start of a match), line for line as Python yields
//! them.

use std::collections::HashMap;

/// One opcode: its tag and the two ranges.
type Op = (&'static str, usize, usize, usize, usize);

/// `SequenceMatcher(None, a, b)`.
struct Matcher<'a> {
    a: &'a [String],
    b: &'a [String],
    b2j: HashMap<&'a str, Vec<usize>>,
}

impl<'a> Matcher<'a> {
    fn new(a: &'a [String], b: &'a [String]) -> Self {
        let mut b2j: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, elt) in b.iter().enumerate() {
            b2j.entry(elt.as_str()).or_default().push(i);
        }
        let n = b.len();
        if n >= 200 {
            let ntest = n / 100 + 1;
            b2j.retain(|_, idxs| idxs.len() <= ntest);
        }
        Matcher { a, b, b2j }
    }

    /// `find_longest_match(alo, ahi, blo, bhi)` (no junk: only the
    /// popular lines are left out of `b2j`).
    fn longest(&self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> (usize, usize, usize) {
        let (a, b) = (self.a, self.b);
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, line) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut newj2len: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = self.b2j.get(line.as_str()) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j
                        .checked_sub(1)
                        .and_then(|p| j2len.get(&p))
                        .copied()
                        .unwrap_or(0)
                        + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && a[besti + bestsize] == b[bestj + bestsize]
        {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    fn matching_blocks(&self) -> Vec<(usize, usize, usize)> {
        let (la, lb) = (self.a.len(), self.b.len());
        let mut queue = vec![(0, la, 0, lb)];
        let mut blocks = Vec::new();
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.longest(alo, ahi, blo, bhi);
            if k > 0 {
                blocks.push((i, j, k));
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        blocks.sort_unstable();
        let (mut i1, mut j1, mut k1) = (0, 0, 0);
        let mut out = Vec::new();
        for (i2, j2, k2) in blocks {
            if i1 + k1 == i2 && j1 + k1 == j2 {
                k1 += k2;
            } else {
                if k1 > 0 {
                    out.push((i1, j1, k1));
                }
                (i1, j1, k1) = (i2, j2, k2);
            }
        }
        if k1 > 0 {
            out.push((i1, j1, k1));
        }
        out.push((la, lb, 0));
        out
    }

    fn opcodes(&self) -> Vec<Op> {
        let (mut i, mut j) = (0, 0);
        let mut answer = Vec::new();
        for (ai, bj, size) in self.matching_blocks() {
            let tag = if i < ai && j < bj {
                "replace"
            } else if i < ai {
                "delete"
            } else if j < bj {
                "insert"
            } else {
                ""
            };
            if !tag.is_empty() {
                answer.push((tag, i, ai, j, bj));
            }
            i = ai + size;
            j = bj + size;
            if size > 0 {
                answer.push(("equal", ai, i, bj, j));
            }
        }
        answer
    }

    fn grouped(&self, n: usize) -> Vec<Vec<Op>> {
        let mut codes = self.opcodes();
        if codes.is_empty() {
            codes.push(("equal", 0, 1, 0, 1));
        }
        if codes[0].0 == "equal" {
            let (t, i1, i2, j1, j2) = codes[0];
            codes[0] = (
                t,
                i1.max(i2.saturating_sub(n)),
                i2,
                j1.max(j2.saturating_sub(n)),
                j2,
            );
        }
        let last = codes.len() - 1;
        if codes[last].0 == "equal" {
            let (t, i1, i2, j1, j2) = codes[last];
            codes[last] = (t, i1, i2.min(i1 + n), j1, j2.min(j1 + n));
        }
        let nn = n + n;
        let mut groups = Vec::new();
        let mut group = Vec::new();
        for (tag, mut i1, i2, mut j1, j2) in codes {
            if tag == "equal" && i2 - i1 > nn {
                group.push((tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n)));
                groups.push(std::mem::take(&mut group));
                i1 = i1.max(i2.saturating_sub(n));
                j1 = j1.max(j2.saturating_sub(n));
            }
            group.push((tag, i1, i2, j1, j2));
        }
        if !group.is_empty() && !(group.len() == 1 && group[0].0 == "equal") {
            groups.push(group);
        }
        groups
    }
}

/// `_format_range_unified(start, stop)`.
fn range(start: usize, stop: usize) -> String {
    let mut beginning = start + 1;
    let length = stop - start;
    if length == 1 {
        return beginning.to_string();
    }
    if length == 0 {
        beginning -= 1;
    }
    format!("{beginning},{length}")
}

/// `difflib.unified_diff(a, b, fromfile, tofile, n=n)`: the lines it
/// yields, each as Python gives it (a line of `a` or `b` keeps its own
/// ending, or has none).
pub fn unified_diff(
    a: &[String],
    b: &[String],
    fromfile: &str,
    tofile: &str,
    n: usize,
) -> Vec<String> {
    let m = Matcher::new(a, b);
    let mut out = Vec::new();
    for (gi, group) in m.grouped(n).iter().enumerate() {
        if gi == 0 {
            out.push(format!("--- {fromfile}\n"));
            out.push(format!("+++ {tofile}\n"));
        }
        let (first, last) = (group[0], group[group.len() - 1]);
        out.push(format!(
            "@@ -{} +{} @@\n",
            range(first.1, last.2),
            range(first.3, last.4)
        ));
        for &(tag, i1, i2, j1, j2) in group {
            if tag == "equal" {
                out.extend(a[i1..i2].iter().map(|l| format!(" {l}")));
                continue;
            }
            if tag == "replace" || tag == "delete" {
                out.extend(a[i1..i2].iter().map(|l| format!("-{l}")));
            }
            if tag == "replace" || tag == "insert" {
                out.extend(b[j1..j2].iter().map(|l| format!("+{l}")));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(t: &str) -> Vec<String> {
        t.split_inclusive('\n').map(str::to_string).collect()
    }

    /// Pinned from Python 3.12: `"".join(difflib.unified_diff(...))`.
    #[test]
    fn as_python_writes_it() {
        let a = lines("a\nb\nc\nd\ne\nf\ng\nh\n");
        let b = lines("a\nB\nc\nd\ne\nf\ng\nH\nx");
        assert_eq!(
            unified_diff(&a, &b, "a/f", "b/f", 3).concat(),
            "--- a/f\n+++ b/f\n@@ -1,8 +1,9 @@\n a\n-b\n+B\n c\n d\n e\n f\n g\n-h\n+H\n+x"
        );
        assert_eq!(
            unified_diff(&a, &b, "a/f", "b/f", 1).concat(),
            "--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n@@ -7,2 +7,3 @@\n g\n-h\n+H\n+x"
        );
        assert_eq!(
            unified_diff(&[], &lines("x\n"), "/dev/null", "b/n", 3).concat(),
            "--- /dev/null\n+++ b/n\n@@ -0,0 +1 @@\n+x\n"
        );
        assert_eq!(
            unified_diff(&lines("x\ny\n"), &[], "a/n", "/dev/null", 0).concat(),
            "--- a/n\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-x\n-y\n"
        );
        assert!(unified_diff(&a, &a, "a", "b", 3).is_empty());
        assert!(unified_diff(&[], &[], "a", "b", 3).is_empty());
    }

    /// Autojunk: in a `b` of 200 lines, a line found more than 3 times is
    /// no match's start, but a match grows over it at its ends: Python
    /// keeps the first 100 lines and replaces the rest.
    #[test]
    fn popular_lines() {
        let a: Vec<String> = (0..200).map(|_| "x\n".to_string()).collect();
        let mut b = a.clone();
        b[100] = "y\n".to_string();
        let d = unified_diff(&a, &b, "a", "b", 0);
        assert_eq!(d[2], "@@ -101,100 +101,100 @@\n");
        assert_eq!(d.len(), 203);
    }
}
