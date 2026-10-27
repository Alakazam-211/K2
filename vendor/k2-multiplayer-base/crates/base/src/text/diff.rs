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
//! Diffs run under deadlines. When one runs out, the result is still a correct diff,
//! just a coarser one.

use std::time::{Duration, Instant};

use similar::{Algorithm, ChangeTag, DiffOp, TextDiff};

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

const LINE_DEADLINE: Duration = Duration::from_millis(50);
const WORD_DEADLINE: Duration = Duration::from_millis(20);
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
    if om.is_empty() || nm.is_empty() || (!om.contains('\n') && !nm.contains('\n')) {
        region_hunks(om, nm, pre, &mut out);
    } else {
        line_then_words(om, nm, pre, &mut out);
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

fn line_then_words(om: &str, nm: &str, base: usize, out: &mut Vec<Hunk>) {
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .deadline(Instant::now() + LINE_DEADLINE)
        .diff_lines(om, nm);
    let ostart = offsets(diff.old_slices());
    let nstart = offsets(diff.new_slices());
    // Walk ops by their lengths with our own cursors: `similar` can report an Insert's
    // old index out of sequence, but the lengths are always consistent.
    let (mut oi, mut ni) = (0usize, 0usize);
    let mut region: Option<(usize, usize)> = None; // (old line, new line) where the region starts
    let flush = |r: (usize, usize), oi: usize, ni: usize, out: &mut Vec<Hunk>| {
        let o = &om[ostart[r.0]..ostart[oi]];
        let n = &nm[nstart[r.1]..nstart[ni]];
        region_hunks(o, n, base + ostart[r.0], out);
    };
    for op in diff.ops() {
        let (ol, nl) = (op.old_range().len(), op.new_range().len());
        match *op {
            DiffOp::Equal { .. } => {
                if let Some(r) = region.take() {
                    flush(r, oi, ni, out);
                }
            }
            _ => {
                region.get_or_insert((oi, ni));
            }
        }
        oi += ol;
        ni += nl;
    }
    if let Some(r) = region {
        flush(r, oi, ni, out);
    }
}

/// Byte offset of each slice start, plus the total at the end.
fn offsets(slices: &[&str]) -> Vec<usize> {
    let mut v = Vec::with_capacity(slices.len() + 1);
    let mut at = 0;
    for s in slices {
        v.push(at);
        at += s.len();
    }
    v.push(at);
    v
}

fn region_hunks(o: &str, n: &str, base: usize, out: &mut Vec<Hunk>) {
    if o == n {
        return;
    }
    // Trim to the character at both edges.
    let pre = common_prefix(o, n);
    let suf = common_suffix(&o[pre..], &n[pre..]);
    let (o, n, base) = (&o[pre..o.len() - suf], &n[pre..n.len() - suf], base + pre);
    if o.is_empty() || n.is_empty() || o.len() > WORD_DIFF_MAX || n.len() > WORD_DIFF_MAX {
        out.push(Hunk {
            start: base,
            end: base + o.len(),
            insert: n.to_owned(),
        });
        return;
    }
    let chars = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .deadline(Instant::now() + WORD_DEADLINE)
        .diff_chars(o, n);
    let mut tags = chars
        .iter_all_changes()
        .map(|c| c.tag())
        .filter(|t| *t != ChangeTag::Equal);
    let one_sided = match tags.next() {
        None => true,
        Some(first) => tags.all(|t| t == first),
    };
    let single_token = |s: &str| !s.contains(char::is_whitespace);
    if !one_sided && single_token(o) && single_token(n) {
        out.push(Hunk {
            start: base,
            end: base + o.len(),
            insert: n.to_owned(),
        });
        return;
    }
    let words;
    let diff = if one_sided {
        &chars
    } else {
        words = TextDiff::configure()
            .algorithm(Algorithm::Myers)
            .deadline(Instant::now() + WORD_DEADLINE)
            .diff_words(o, n);
        &words
    };
    let mut at = base;
    let mut cur: Option<Hunk> = None;
    for ch in diff.iter_all_changes() {
        let v = ch.value();
        match ch.tag() {
            ChangeTag::Equal => {
                if let Some(h) = cur.take() {
                    out.push(h);
                }
                at += v.len();
            }
            ChangeTag::Delete => {
                let h = cur.get_or_insert_with(|| Hunk {
                    start: at,
                    end: at,
                    insert: String::new(),
                });
                h.end += v.len();
                at += v.len();
            }
            ChangeTag::Insert => {
                cur.get_or_insert_with(|| Hunk {
                    start: at,
                    end: at,
                    insert: String::new(),
                })
                .insert
                .push_str(v);
            }
        }
    }
    if let Some(h) = cur {
        out.push(h);
    }
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
        check(&old, &new);
    }

    fn text() -> impl Strategy<Value = String> {
        proptest::string::string_regex("([ab日🎉\n é]|ab|\n\n){0,40}").unwrap()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2000))]
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
    }
}
