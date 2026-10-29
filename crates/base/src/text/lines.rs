//! Line-aligned insertions (and, for outside saves, deletions), for edits that merge.
//!
//! A minimal diff trims the common prefix to the character, so adding the line
//! `agent-17` after the line `agent-1` comes out as inserting `"7\nagent-1"` in the middle
//! of `agent-1`, not `"agent-17\n"` at a line start. Both give the same text. As a CRDT
//! change they don't merge the same, though: an insertion is anchored to the characters
//! around it, and another insertion made concurrently at the same anchor (an outside edit
//! adding a line next to it) lands inside the first one's line (`editor-187`). Each run
//! survives, but lines get spliced.
//!
//! [`align_insertions`] slides each pure insertion that holds a line break along the text
//! (the standard diff "slider" rotation, which never changes the result) to start at a line
//! start and end with `\n` when such a position exists within its slack. Line-aligned
//! insertions anchor to line breaks, so concurrent line edits keep whole lines.

use super::diff::Hunk;

/// How far an insertion slides, in characters, at most.
const MAX_SLIDE: usize = 4096;

/// `hunks` (sorted, non-overlapping, against `old`) with pure insertions slid to line
/// boundaries where possible. Applying the result to `old` gives the same text; hunks keep
/// their order and never come to touch a neighbour.
pub fn align_insertions(old: &str, mut hunks: Vec<Hunk>) -> Vec<Hunk> {
    for i in 0..hunks.len() {
        let h = &hunks[i];
        if h.start != h.end || !h.insert.contains('\n') {
            continue;
        }
        // stay strictly between the neighbours
        let lo = if i == 0 { 0 } else { hunks[i - 1].end + 1 };
        let hi = hunks
            .get(i + 1)
            .map_or(old.len(), |n| n.start.saturating_sub(1));
        if let Some(slid) = slide(old, h, lo, hi) {
            hunks[i] = slid;
        }
    }
    hunks
}

/// `hunks` (sorted, non-overlapping, against `old`) with pure deletions that hold a line
/// break slid to whole lines where possible: to start at a line start and end with `\n`
/// (v0.9.5). The same rotation as [`align_insertions`]: applying the result to `old` gives
/// the same text; hunks keep their order and never come to touch a neighbour.
///
/// A minimal diff can express deleting the line `p01:1` before `p01:2` as deleting
/// `1\np01:`, across the break. Merged with another side's line inserted at that line start
/// (an outside save, [`super::merge::merge3_keep_both`]), the insertion would fall inside the
/// deletion and the two sides' lines would be spliced; aligned, it touches the deletion's
/// end and lands whole.
pub fn align_deletions(old: &str, mut hunks: Vec<Hunk>) -> Vec<Hunk> {
    for i in 0..hunks.len() {
        let h = &hunks[i];
        if !h.insert.is_empty() || !old[h.start..h.end].contains('\n') {
            continue;
        }
        let lo = if i == 0 { 0 } else { hunks[i - 1].end + 1 };
        let hi = hunks
            .get(i + 1)
            .map_or(old.len(), |n| n.start.saturating_sub(1));
        if let Some(slid) = slide_deletion(old, h.start, h.end, lo, hi) {
            hunks[i] = slid;
        }
    }
    hunks
}

fn whole_lines(old: &str, start: usize, end: usize) -> bool {
    (start == 0 || old.as_bytes()[start - 1] == b'\n') && old.as_bytes()[end - 1] == b'\n'
}

fn slide_deletion(old: &str, start: usize, end: usize, lo: usize, hi: usize) -> Option<Hunk> {
    if whole_lines(old, start, end) {
        return None;
    }
    let found = |start: usize, end: usize| {
        Some(Hunk {
            start,
            end,
            insert: String::new(),
        })
    };
    // left: the character before the deletion equals its last one
    let (mut s, mut e) = (start, end);
    for _ in 0..MAX_SLIDE {
        let (Some(c), Some(d)) = (old[..s].chars().next_back(), old[..e].chars().next_back())
        else {
            break;
        };
        if c != d || s - c.len_utf8() < lo {
            break;
        }
        s -= c.len_utf8();
        e -= c.len_utf8();
        if whole_lines(old, s, e) {
            return found(s, e);
        }
    }
    // right: the character after it equals its first one
    let (mut s, mut e) = (start, end);
    for _ in 0..MAX_SLIDE {
        let (Some(c), Some(d)) = (old[s..].chars().next(), old[e..].chars().next()) else {
            break;
        };
        if c != d || e + c.len_utf8() > hi {
            break;
        }
        s += c.len_utf8();
        e += c.len_utf8();
        if whole_lines(old, s, e) {
            return found(s, e);
        }
    }
    None
}

fn aligned(old: &str, at: usize, insert: &str) -> bool {
    (at == 0 || old.as_bytes()[at - 1] == b'\n') && insert.ends_with('\n')
}

fn slide(old: &str, h: &Hunk, lo: usize, hi: usize) -> Option<Hunk> {
    if aligned(old, h.start, &h.insert) {
        return None;
    }
    let found = |start: usize, insert: String| {
        Some(Hunk {
            start,
            end: start,
            insert,
        })
    };
    // left: the character before the insertion equals its last one
    let (mut at, mut ins) = (h.start, h.insert.clone());
    for _ in 0..MAX_SLIDE {
        let Some(c) = old[..at].chars().next_back() else {
            break;
        };
        if at - c.len_utf8() < lo || !ins.ends_with(c) {
            break;
        }
        ins.truncate(ins.len() - c.len_utf8());
        ins.insert(0, c);
        at -= c.len_utf8();
        if aligned(old, at, &ins) {
            return found(at, ins);
        }
    }
    // right: the character after it equals its first one
    let (mut at, mut ins) = (h.start, h.insert.clone());
    for _ in 0..MAX_SLIDE {
        let Some(c) = old[at..].chars().next() else {
            break;
        };
        if at + c.len_utf8() > hi || !ins.starts_with(c) {
            break;
        }
        ins.remove(0);
        ins.push(c);
        at += c.len_utf8();
        if aligned(old, at, &ins) {
            return found(at, ins);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::diff;
    use proptest::prelude::*;

    fn align(old: &str, new: &str) -> Vec<Hunk> {
        align_insertions(old, diff::hunks(old, new))
    }

    #[test]
    fn line_insertions_move_to_line_starts() {
        // "agent1-17" added before the line "agent1-1": the diff trims "x\nagent1-1" and
        // inserts "7\nagent1-1" mid-line
        let old = "x\nagent1-1\nbase 5\n";
        let new = "x\nagent1-17\nagent1-1\nbase 5\n";
        let raw = diff::hunks(old, new);
        assert!(!aligned(old, raw[0].start, &raw[0].insert), "{raw:?}");
        let a = align(old, new);
        assert_eq!(a.len(), 1);
        assert!(aligned(old, a[0].start, &a[0].insert), "{a:?}");
        assert_eq!(diff::apply(old, &a), new);
        // a line added after "base 3" (diffed as "\nagent0-5" before its newline)
        let old = "base 3\nbase 4\n";
        let new = "base 3\nagent0-5\nbase 4\n";
        let a = align(old, new);
        assert_eq!(
            a,
            vec![Hunk {
                start: 7,
                end: 7,
                insert: "agent0-5\n".into()
            }]
        );
        // insertions inside a line, and replacements, are left alone
        assert_eq!(align("ab\n", "aXb\n"), diff::hunks("ab\n", "aXb\n"));
        assert_eq!(align("ab\n", "aX\nb\n"), diff::hunks("ab\n", "aX\nb\n"));
        // the end of a file without a final newline: nothing to align to
        assert_eq!(align("a", "a\nb"), diff::hunks("a", "a\nb"));
        // multi-byte characters slide by whole characters
        let old = "日本\n日本\n";
        let new = "日本\n日本\n日本\n";
        let a = align(old, new);
        assert!(aligned(old, a[0].start, &a[0].insert));
        assert_eq!(diff::apply(old, &a), new);
    }

    #[test]
    fn line_deletions_move_to_whole_lines() {
        let old = "x\np01:1\np01:2\n";
        let new = "x\np01:2\n";
        let a = align_deletions(old, diff::hunks(old, new));
        assert_eq!(
            a,
            vec![Hunk {
                start: 2,
                end: 8,
                insert: String::new()
            }]
        );
        assert_eq!(diff::apply(old, &a), new);
        // deletions inside a line, and replacements, are left alone
        assert_eq!(
            align_deletions("abc\n", diff::hunks("abc\n", "ac\n")),
            diff::hunks("abc\n", "ac\n")
        );
        assert_eq!(
            align_deletions("ab\ncd\n", diff::hunks("ab\ncd\n", "aXd\n")),
            diff::hunks("ab\ncd\n", "aXd\n")
        );
    }

    proptest! {
        #[test]
        fn deletions_same_text_valid_hunks(
            old in "[ab\n日]{0,24}",
            new in "[ab\n日]{0,24}",
        ) {
            let h = align_insertions(&old, diff::hunks(&old, &new));
            let a = align_deletions(&old, h.clone());
            prop_assert!(diff::validate(&old, &a).is_ok(), "{:?}", a);
            prop_assert_eq!(diff::apply(&old, &a), new);
            prop_assert_eq!(a.len(), h.len());
            for w in a.windows(2) {
                prop_assert!(w[0].end < w[1].start, "touching: {:?}", a);
            }
        }

        #[test]
        fn same_text_valid_hunks(
            old in "[ab\n日]{0,24}",
            new in "[ab\n日]{0,24}",
        ) {
            let h = diff::hunks(&old, &new);
            let a = align_insertions(&old, h.clone());
            prop_assert!(diff::validate(&old, &a).is_ok(), "{:?}", a);
            prop_assert_eq!(diff::apply(&old, &a), new);
            prop_assert_eq!(a.len(), h.len());
            for w in a.windows(2) {
                prop_assert!(w[0].end < w[1].start, "touching: {:?}", a);
            }
        }
    }
}
