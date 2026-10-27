//! Text diff to minimal edits.
//!
//! An edit (CLI verb, external file change, restore) arrives as "old text -> new text".
//! [`hunks`] turns that into the smallest set of replacements, so the CRDT only touches
//! what changed and concurrent typing elsewhere (even in the same line) survives:
//!
//! 1. trim the common prefix and suffix, to the character (agent edits are usually local);
//! 2. a line diff of the middle;
//! 3. inside each changed group of lines, trim again to the character, then:
//!    - a character diff when the region is only insertions or only deletions
//!      ("bc" -> "1bc2" is two insertions, not a replace);
//!    - otherwise a word diff. A char diff of "brown" -> "red" keeps the shared "r" and
//!      splits the change in two, which merges badly ("rEDown").
//!
//! `hunks` is a pure function of its input: the three-way merge relies on two diffs of the
//! same text being identical. So the diffs are bounded by counted work, never by a clock:
//! one `Budget` per call, `BUDGET_BASE + BUDGET_PER_BYTE * (old.len() + new.len())` units
//! (see `text/myers.rs`), shared by the line diff and every region's char and word diff,
//! so the worst case is O(input) work. When it runs out the result is still a correct diff,
//! just a coarser one, and the same one every time.
//!
//! Ties (several minimal diffs) are broken the same way every time: the common prefix is
//! trimmed first, so a change inside a repeated run sits after the run's shared start; inside
//! the middle, Myers' bisect decides. `tie_break_is_pinned` fixes the choice (undo spec D2).

use super::myers::{self, Budget, Change};

/// Replace `old[start..end]` (byte offsets, on char boundaries) with `insert`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub start: usize,
    pub end: usize,
    pub insert: String,
}

impl Hunk {
    pub fn is_insertion(&self) -> bool {
        self.start == self.end
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HunkError {
    #[error("hunk {index} is out of order or overlaps the previous one")]
    Order { index: usize },
    #[error("hunk {index} is out of range or not on a character boundary")]
    Range { index: usize },
    #[error("hunk {index} changes nothing")]
    Empty { index: usize },
}

/// Above this, a changed group of lines is replaced whole instead of word-diffed.
const WORD_DIFF_MAX: usize = 64 * 1024;

/// The minimal hunks turning `old` into `new`: sorted, non-overlapping, never empty.
pub fn hunks(old: &str, new: &str) -> Vec<Hunk> {
    if old == new {
        return Vec::new();
    }
    let pre = common_prefix(old, new);
    let suf = common_suffix(&old[pre..], &new[pre..]);
    let (om, nm) = (&old[pre..old.len() - suf], &new[pre..new.len() - suf]);
    let mut out = Vec::new();
    let mut budget = Budget::for_input(old.len() + new.len());
    if om.is_empty() || nm.is_empty() || (!om.contains('\n') && !nm.contains('\n')) {
        region_hunks(om, nm, pre, &mut out, &mut budget);
    } else {
        line_then_words(om, nm, pre, &mut out, &mut budget);
    }
    coalesce(out)
}

/// Apply hunks to `old`. Panics on invalid hunks; call [`validate`] first for untrusted input.
pub fn apply(old: &str, hunks: &[Hunk]) -> String {
    let mut s =
        String::with_capacity(old.len() + hunks.iter().map(|h| h.insert.len()).sum::<usize>());
    let mut at = 0;
    for h in hunks {
        s.push_str(&old[at..h.start]);
        s.push_str(&h.insert);
        at = h.end;
    }
    s.push_str(&old[at..]);
    s
}

/// Check hunks against `old`: in order, non-overlapping, in range, on char boundaries, non-empty.
pub fn validate(old: &str, hunks: &[Hunk]) -> Result<(), HunkError> {
    let mut prev_end = 0usize;
    let mut prev_insert_at = None;
    for (index, h) in hunks.iter().enumerate() {
        if h.start > h.end
            || h.end > old.len()
            || !old.is_char_boundary(h.start)
            || !old.is_char_boundary(h.end)
        {
            return Err(HunkError::Range { index });
        }
        if index > 0
            && (h.start < prev_end || (h.is_insertion() && prev_insert_at == Some(h.start)))
        {
            return Err(HunkError::Order { index });
        }
        if h.is_insertion() && h.insert.is_empty() {
            return Err(HunkError::Empty { index });
        }
        prev_insert_at = h.is_insertion().then_some(h.start);
        prev_end = h.end;
    }
    Ok(())
}

fn common_prefix(a: &str, b: &str) -> usize {
    let n = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    floor_boundary(a, n)
}

fn common_suffix(a: &str, b: &str) -> usize {
    let n = a
        .bytes()
        .rev()
        .zip(b.bytes().rev())
        .take_while(|(x, y)| x == y)
        .count();
    // keep the suffix on a char boundary in both strings (they share these bytes)
    let mut n = n;
    while n > 0 && !a.is_char_boundary(a.len() - n) {
        n -= 1;
    }
    n
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn line_then_words(om: &str, nm: &str, base: usize, out: &mut Vec<Hunk>, budget: &mut Budget) {
    let (ol, nl) = (lines(om), lines(nm));
    let (ostart, nstart) = (offsets(&ol), offsets(&nl));
    for c in myers::diff(&ol, &nl, budget) {
        let o = &om[ostart[c.old.start]..ostart[c.old.end]];
        let n = &nm[nstart[c.new.start]..nstart[c.new.end]];
        region_hunks(o, n, base + ostart[c.old.start], out, budget);
    }
}

/// Lines, each with its `\n`.
pub(crate) fn lines(s: &str) -> Vec<&str> {
    s.split_inclusive('\n').collect()
}

/// Characters.
fn chars(s: &str) -> Vec<&str> {
    s.char_indices()
        .map(|(i, c)| &s[i..i + c.len_utf8()])
        .collect()
}

/// Runs of whitespace and runs of everything else.
fn words(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut last_ws = None;
    for (i, c) in s.char_indices() {
        let ws = c.is_whitespace();
        if last_ws.is_some_and(|l| l != ws) {
            out.push(&s[start..i]);
            start = i;
        }
        last_ws = Some(ws);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Byte offset of each token start, plus the total at the end.
fn offsets(tokens: &[&str]) -> Vec<usize> {
    let mut v = Vec::with_capacity(tokens.len() + 1);
    let mut at = 0;
    for t in tokens {
        v.push(at);
        at += t.len();
    }
    v.push(at);
    v
}

fn region_hunks(o: &str, n: &str, base: usize, out: &mut Vec<Hunk>, budget: &mut Budget) {
    if o == n {
        return;
    }
    // Trim to the character at both edges.
    let pre = common_prefix(o, n);
    let suf = common_suffix(&o[pre..], &n[pre..]);
    let (o, n, base) = (&o[pre..o.len() - suf], &n[pre..n.len() - suf], base + pre);
    let whole = |out: &mut Vec<Hunk>| {
        out.push(Hunk {
            start: base,
            end: base + o.len(),
            insert: n.to_owned(),
        })
    };
    if o.is_empty() || n.is_empty() || o.len() > WORD_DIFF_MAX || n.len() > WORD_DIFF_MAX {
        return whole(out);
    }
    let (oc, nc) = (chars(o), chars(n));
    let changes = myers::diff(&oc, &nc, budget);
    let one_sided =
        changes.iter().all(|c| c.old.is_empty()) || changes.iter().all(|c| c.new.is_empty());
    let single_token = |s: &str| !s.contains(char::is_whitespace);
    if !one_sided && single_token(o) && single_token(n) {
        return whole(out);
    }
    let (ot, nt, changes) = if one_sided {
        (oc, nc, changes)
    } else {
        let (ow, nw) = (words(o), words(n));
        let changes = myers::diff(&ow, &nw, budget);
        (ow, nw, changes)
    };
    let (ostart, nstart) = (offsets(&ot), offsets(&nt));
    out.extend(changes.into_iter().map(|Change { old, new }| Hunk {
        start: base + ostart[old.start],
        end: base + ostart[old.end],
        insert: n[nstart[new.start]..nstart[new.end]].to_owned(),
    }));
}

/// Merge hunks that touch, so the result is canonical.
pub(crate) fn coalesce(v: Vec<Hunk>) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = Vec::with_capacity(v.len());
    for h in v {
        if let Some(last) = out.last_mut() {
            if last.end == h.start {
                last.end = h.end;
                last.insert.push_str(&h.insert);
                continue;
            }
        }
        out.push(h);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn check(old: &str, new: &str) -> Vec<Hunk> {
        let h = hunks(old, new);
        validate(old, &h).unwrap();
        assert_eq!(apply(old, &h), new, "apply(hunks) must reproduce new");
        h
    }

    #[test]
    fn one_word_change_is_one_small_hunk() {
        let old = "# Title\n\nThe quick brown fox jumps.\nSecond line.\n";
        let new = "# Title\n\nThe quick red fox jumps.\nSecond line.\n";
        let h = check(old, new);
        assert_eq!(h.len(), 1);
        assert!(
            h[0].end - h[0].start <= 5 && h[0].insert.len() <= 3,
            "{h:?}"
        );
    }

    #[test]
    fn identical_is_empty_and_edges() {
        assert!(check("same", "same").is_empty());
        check("", "new\n");
        check("old\n", "");
        check("a🎉b", "a🎊b");
        check("日本語\n", "日本\n語\n");
    }

    #[test]
    fn multi_line_edit_keeps_untouched_lines_out() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB!\nc\nd\nE\n";
        let h = check(old, new);
        assert_eq!(h.len(), 2, "{h:?}");
        assert!(h.iter().all(|x| !x.insert.contains("c\nd")));
    }

    #[test]
    fn validate_rejects_bad_hunks() {
        let s = "héllo";
        assert!(validate(
            s,
            &[Hunk {
                start: 2,
                end: 2,
                insert: "x".into()
            }]
        )
        .is_err()); // inside é
        assert!(validate(
            s,
            &[Hunk {
                start: 0,
                end: 9,
                insert: String::new()
            }]
        )
        .is_err());
        assert!(validate(
            s,
            &[
                Hunk {
                    start: 3,
                    end: 4,
                    insert: String::new()
                },
                Hunk {
                    start: 1,
                    end: 2,
                    insert: String::new()
                }
            ]
        )
        .is_err());
        assert!(validate(
            s,
            &[Hunk {
                start: 1,
                end: 1,
                insert: String::new()
            }]
        )
        .is_err());
        assert!(validate(
            s,
            &[
                Hunk {
                    start: 1,
                    end: 1,
                    insert: "a".into()
                },
                Hunk {
                    start: 1,
                    end: 1,
                    insert: "b".into()
                }
            ]
        )
        .is_err());
    }

    #[test]
    fn large_inputs_stay_correct() {
        let old: String = (0..20_000)
            .map(|i| format!("line {i} lorem ipsum\n"))
            .collect();
        let mut new = old
            .replace("line 777 ", "LINE 777 ")
            .replace("line 15000 lorem", "x");
        new.insert_str(0, "# head\n");
        let h = check(&old, &new);
        // a big doc with three small edits stays three small hunks (the budget scales with input)
        assert_eq!(h.len(), 3, "{h:?}");
        assert!(h.iter().all(|x| x.end - x.start <= 32), "{h:?}");
    }

    /// No clock: a worst case for the line diff (every line moved) finishes on counted work
    /// alone, and gives a correct diff that is the same on every call.
    #[test]
    fn pathological_input_is_bounded_correct_and_deterministic() {
        let lines: Vec<String> = (0..30_000).map(|i| format!("{i} lorem\n")).collect();
        let old = lines.concat();
        let new: String = lines.iter().rev().map(String::as_str).collect();
        let h = check(&old, &new);
        assert_eq!(hunks(&old, &new), h);
    }

    /// Undo spec D2: where several minimal diffs exist, the choice is fixed. These cases pin
    /// it; changing any of them is a spec change. The common prefix is trimmed first, so a
    /// change inside a repeated run lands after the run's shared start (an appended copy of
    /// the last line is an append at the end); inside the middle, Myers' bisect decides.
    #[test]
    fn tie_break_is_pinned() {
        let h = |start, end, insert: &str| Hunk {
            start,
            end,
            insert: insert.into(),
        };
        let cases: &[(&str, &str, Vec<Hunk>)] = &[
            ("aa", "aaa", vec![h(2, 2, "a")]),
            ("xaay", "xay", vec![h(2, 3, "")]),
            ("ab\n", "ab\nab\n", vec![h(3, 3, "ab\n")]),
            (
                "p a a q\n",
                "r a a a s\n",
                vec![h(0, 1, "r"), h(6, 7, "a s")],
            ),
            (
                "x\na\na\ny\n",
                "z\na\na\na\nw\n",
                vec![h(0, 1, "z\na"), h(6, 7, "w")],
            ),
            ("c a b\n", "a b c\n", vec![h(0, 2, ""), h(5, 5, " c")]),
            ("1 2\n", "2 1\n", vec![h(0, 2, ""), h(3, 3, " 1")]),
        ];
        for (o, n, want) in cases {
            assert_eq!(&check(o, n), want, "{o:?} -> {n:?}");
        }
    }

    /// `PROPTEST_CASES` overrides the default count (`with_cases` alone would ignore it).
    fn cases(default: u32) -> u32 {
        std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    fn text() -> impl Strategy<Value = String> {
        proptest::string::string_regex("([ab日🎉\n é]|ab|\n\n){0,40}").unwrap()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(cases(2000)))]
        #[test]
        fn hunks_round_trip(old in text(), new in text()) {
            let h = hunks(&old, &new);
            prop_assert!(validate(&old, &h).is_ok());
            prop_assert_eq!(apply(&old, &h), new);
        }

        #[test]
        fn hunks_stay_inside_the_changed_middle(core in text(), pre in text(), suf in text(), x in text()) {
            let old = format!("{pre}{core}{suf}");
            let new = format!("{pre}{x}{suf}");
            let p = common_prefix(&old, &new);
            let s = common_suffix(&old[p..], &new[p..]);
            for h in hunks(&old, &new) {
                prop_assert!(h.start >= p, "hunk {:?} starts inside the common prefix ({})", h, p);
                prop_assert!(h.end <= old.len() - s, "hunk {:?} ends inside the common suffix ({})", h, s);
            }
        }

        /// The merge compares hunks from two diffs of the same base, so `hunks` must be a pure
        /// function (same input, same hunks) and canonical (no two hunks touch).
        #[test]
        fn hunks_are_deterministic_and_canonical(old in text(), new in text()) {
            let h = hunks(&old, &new);
            prop_assert_eq!(&hunks(&old, &new), &h);
            for w in h.windows(2) {
                prop_assert!(w[0].end < w[1].start, "touching hunks {:?}", w);
            }
        }
    }
}
