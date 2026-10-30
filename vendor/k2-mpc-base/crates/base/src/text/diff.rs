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
    hunks_with(old, new, false)
}

/// [`hunks`], with the line diff made on whole lines: the common prefix and suffix are
/// trimmed to line boundaries of both texts before it (each changed group is still trimmed
/// to the character), so lines that share a start or an end are never paired one shifted.
pub fn line_hunks(old: &str, new: &str) -> Vec<Hunk> {
    hunks_with(old, new, true)
}

/// The outside-save merge's sides (v0.9.7): [`line_hunks`], its pure line insertions and
/// deletions slid to line boundaries where they can be ([`super::lines::align_insertions`],
/// [`super::lines::align_deletions`]), then every hunk grown to whole lines
/// ([`super::lines::to_whole_lines`]): it starts at a line start, ends at a line start or at
/// the end of the text, and writes whole lines (its text ends with "\n" unless it runs to the
/// end). Two changes in one line are one hunk; changes of different lines stay apart where
/// they only meet at a line start. Deterministic, valid, and gives `new`; [`hunks`] and
/// [`line_hunks`] are unchanged.
pub fn whole_line_hunks(old: &str, new: &str) -> Vec<Hunk> {
    use super::lines;
    let h = lines::align_deletions(old, lines::align_insertions(old, line_hunks(old, new)));
    lines::to_whole_lines(old, &h)
}

fn hunks_with(old: &str, new: &str, whole_lines: bool) -> Vec<Hunk> {
    if old == new {
        return Vec::new();
    }
    let pre = common_prefix(old, new);
    let suf = common_suffix(&old[pre..], &new[pre..]);
    let (om, nm) = (&old[pre..old.len() - suf], &new[pre..new.len() - suf]);
    let mut out = Vec::new();
    let mut budget = Budget::for_input(old.len() + new.len());
    if !whole_lines
        && (om.is_empty() || nm.is_empty() || (!om.contains('\n') && !nm.contains('\n')))
    {
        region_hunks(om, nm, pre, &mut out, &mut budget);
    } else if whole_lines {
        // (v0.9.7: whole_lines always trims to line starts first, even when the changed
        // middle is one-sided or within a line: a text kept after the change stays kept)
        let lpre = old[..pre].rfind('\n').map_or(0, |k| k + 1);
        // the suffix from a line start of **both** texts (v0.9.7): a boundary that starts a
        // line of `old` but not of `new` would leave the middle's new text ending inside a
        // line, kept text then read as part of a change; past the first "\n" inside the
        // suffix both texts agree
        let line_start = |t: &str, p: usize| p == 0 || t.as_bytes()[p - 1] == b'\n';
        let mut lsuf = suf;
        while lsuf > 0 && !(line_start(old, old.len() - lsuf) && line_start(new, new.len() - lsuf))
        {
            let s = old.len() - lsuf;
            lsuf = old[s..].find('\n').map_or(0, |k| lsuf - (k + 1));
        }
        let (om, nm) = (&old[lpre..old.len() - lsuf], &new[lpre..new.len() - lsuf]);
        line_then_words(om, nm, lpre, &mut out, &mut budget);
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

/// `h` (on `old`) trimmed to the characters it changes: the common prefix and suffix of the
/// text it deletes and the text it inserts are left out (prefix first, never past a character
/// boundary). `None` when nothing is left (it would write what is already there). Applying
/// the result gives the same text; its range lies within `h`'s.
pub fn trim_hunk(old: &str, h: &Hunk) -> Option<Hunk> {
    let del = &old[h.start..h.end];
    let ins = h.insert.as_str();
    let pre: usize = del
        .chars()
        .zip(ins.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let (del, ins) = (&del[pre..], &ins[pre..]);
    let suf: usize = del
        .chars()
        .rev()
        .zip(ins.chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let (start, end) = (h.start + pre, h.end - suf);
    let insert = &ins[..ins.len() - suf];
    (start != end || !insert.is_empty()).then(|| Hunk {
        start,
        end,
        insert: insert.to_owned(),
    })
}

/// Every hunk of `hunks` (on `old`) [`trim_hunk`]ed; the ones that change nothing dropped.
pub fn trim_hunks(old: &str, hunks: &[Hunk]) -> Vec<Hunk> {
    hunks.iter().filter_map(|h| trim_hunk(old, h)).collect()
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

    /// D2 for `write --base`'s sides (v0.9.8): `line_hunks` pairs whole lines first, so where
    /// `hunks`' character trim reads a change one line off (the next line shares its first
    /// characters), `line_hunks` reads the line that changed. These cases pin its choices;
    /// changing any of them is a spec change (undo spec D2, O0).
    #[test]
    fn line_tie_break_is_pinned() {
        let h = |start, end, insert: &str| Hunk {
            start,
            end,
            insert: insert.into(),
        };
        let cases: &[(&str, &str, Vec<Hunk>, Vec<Hunk>)] = &[
            // (old, new, line_hunks, hunks)
            ("aa", "aaa", vec![h(2, 2, "a")], vec![h(2, 2, "a")]),
            (
                "ab\n",
                "ab\nab\n",
                vec![h(3, 3, "ab\n")],
                vec![h(3, 3, "ab\n")],
            ),
            // a line inserted before a line with the same start
            (
                "ab\n",
                "aX\nab\n",
                vec![h(0, 0, "aX\n")],
                vec![h(1, 1, "X\na")],
            ),
            // a line deleted before a line with the same start
            (
                "p22:2150\np10:2205\n",
                "p10:2205\n",
                vec![h(0, 9, "")],
                vec![h(1, 10, "")],
            ),
            // a word inside a line: the same as hunks
            (
                "alpha beta\ngamma\n",
                "alpha BETA\ngamma\n",
                vec![h(6, 10, "BETA")],
                vec![h(6, 10, "BETA")],
            ),
        ];
        for (o, n, want_line, want) in cases {
            let l = line_hunks(o, n);
            validate(o, &l).unwrap();
            assert_eq!(apply(o, &l), *n);
            assert_eq!(&l, want_line, "line_hunks {o:?} -> {n:?}");
            assert_eq!(&check(o, n), want, "hunks {o:?} -> {n:?}");
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

        /// `line_hunks` (the outside-save merge's sides): valid, a round trip, deterministic
        /// and canonical, as `hunks` is.
        #[test]
        fn line_hunks_round_trip(old in text(), new in text()) {
            let h = line_hunks(&old, &new);
            prop_assert!(validate(&old, &h).is_ok());
            prop_assert_eq!(apply(&old, &h), new.clone());
            prop_assert_eq!(&line_hunks(&old, &new), &h);
            for w in h.windows(2) {
                prop_assert!(w[0].end < w[1].start, "touching hunks {:?}", w);
            }
        }

        /// `whole_line_hunks` (the outside-save merge's sides, v0.9.7): valid, a round trip,
        /// deterministic; every hunk whole lines (starts at a line start, ends at one or at
        /// the end, writes whole lines), none overlapping, no two insertions at one point.
        #[test]
        fn whole_line_hunks_are_whole_lines(old in text(), new in text()) {
            let h = whole_line_hunks(&old, &new);
            prop_assert!(validate(&old, &h).is_ok(), "{:?}", h);
            prop_assert_eq!(apply(&old, &h), new.clone());
            prop_assert_eq!(&whole_line_hunks(&old, &new), &h);
            for x in &h {
                prop_assert!(super::super::lines::is_whole_lines(&old, x), "{:?} of {:?}", x, h);
            }
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

        /// D2 for `write --base`'s sides (v0.9.8, [`line_hunks`]): every hunk lies inside the
        /// changed middle widened to whole lines: it starts at or after the line start at or
        /// before the common prefix's end, and ends at or before the common suffix's largest
        /// part that starts a line of both texts. So a line both texts keep whole around the
        /// change is never in a hunk (`hunks` keeps its own property above).
        #[test]
        fn line_hunks_stay_inside_the_line_aligned_middle(core in text(), pre in text(), suf in text(), x in text()) {
            let old = format!("{pre}{core}{suf}");
            let new = format!("{pre}{x}{suf}");
            let p = common_prefix(&old, &new);
            let s = common_suffix(&old[p..], &new[p..]);
            let lp = old[..p].rfind('\n').map_or(0, |k| k + 1);
            let starts = |t: &str, q: usize| q == 0 || t.as_bytes()[q - 1] == b'\n';
            let ls = (0..=s)
                .rev()
                .find(|&k| k == 0 || (starts(&old, old.len() - k) && starts(&new, new.len() - k)))
                .unwrap_or(0);
            for h in line_hunks(&old, &new) {
                prop_assert!(h.start >= lp, "hunk {:?} starts before the line of the common prefix's end ({})", h, lp);
                prop_assert!(h.end <= old.len() - ls, "hunk {:?} ends inside the common suffix's whole lines ({})", h, ls);
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
