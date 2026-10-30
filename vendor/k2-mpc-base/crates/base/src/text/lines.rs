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

/// The hunks an edit given as a new text applies (verbs through `edit_with`, `replace_text`,
/// `restore`, tier 0's promotion): [`super::diff::line_hunks`] (lines paired whole first,
/// then a character or word diff inside each changed group of lines), with pure insertions
/// slid to line starts ([`align_insertions`]). Pairing lines first keeps a change to the
/// lines it changed: with [`super::diff::hunks`]' character trim, deleting `seed:2` from
/// `seed:2\nseed:3\n` deleted `2\nseed:` instead, joining the next line's `3` to text of the
/// deleted line, and D1 then made that joined word, `seed:3`, the deleter's (blame named
/// them for a line they never wrote; v0.9.7.1, v0.9.8). These hunks name the elements the
/// change removes and the engine keeps.
pub fn edit_hunks(old: &str, new: &str) -> Vec<Hunk> {
    align_insertions(old, super::diff::line_hunks(old, new))
}

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

/// Whether `h` changes whole lines of `old`: it starts at a line start, ends at a line start
/// or at the end of the text, and what it writes is whole lines (or nothing): its text ends
/// with "\n" unless it runs to the end of the text.
pub fn is_whole_lines(old: &str, h: &Hunk) -> bool {
    let line_start = |p: usize| p == 0 || old.as_bytes()[p - 1] == b'\n';
    line_start(h.start)
        && (h.end == old.len() || line_start(h.end))
        && (h.insert.is_empty() || h.insert.ends_with('\n') || h.end == old.len())
}

/// `hunks` (sorted, non-overlapping, against `old`) each grown to whole lines of `old`
/// ([`is_whole_lines`]; v0.9.7, the outside save's sides): a hunk that isn't whole lines
/// takes in the rest of its first line and runs on to the end of the line its text ends
/// in, so it writes whole lines. Hunks whose grown ranges overlap are joined (one hunk per
/// group of lines); hunks of different lines that only meet at a line start stay apart, so
/// a change made identically to one line on both sides of a merge is still one hunk there
/// (O4). Applying the result to `old` gives the same text; no two of its hunks overlap, and
/// no two insertions share a point.
pub fn to_whole_lines(old: &str, hunks: &[Hunk]) -> Vec<Hunk> {
    // each hunk's range, grown
    let ranges: Vec<(usize, usize)> = hunks
        .iter()
        .map(|h| {
            if is_whole_lines(old, h) {
                return (h.start, h.end);
            }
            let s = old[..h.start].rfind('\n').map_or(0, |k| k + 1);
            let mut e = h.end;
            // whether what the hunk writes from `s` up to `e` ends a line (or is empty):
            // `old[s..h.start]` holds no "\n"
            let mut closed = if h.insert.is_empty() {
                s == h.start
            } else {
                h.insert.ends_with('\n')
            };
            loop {
                let boundary = e == old.len() || (e > 0 && old.as_bytes()[e - 1] == b'\n');
                if boundary && (closed || e == old.len()) {
                    break;
                }
                let next = old[e..].find('\n').map_or(old.len(), |k| e + k + 1);
                closed = old.as_bytes()[next - 1] == b'\n';
                e = next;
            }
            (s, e)
        })
        .collect();
    let mut out: Vec<Hunk> = Vec::with_capacity(hunks.len());
    let mut k = 0;
    while k < hunks.len() {
        let (s, mut e) = ranges[k];
        let first = k;
        k += 1;
        // join what overlaps; at one point, two insertions (never apart: one point holds
        // one insertion)
        while k < hunks.len() && (ranges[k].0 < e || (ranges[k] == (e, e) && s == e)) {
            e = e.max(ranges[k].1);
            k += 1;
        }
        let local: Vec<Hunk> = hunks[first..k]
            .iter()
            .map(|h| Hunk {
                start: h.start - s,
                end: h.end - s,
                insert: h.insert.clone(),
            })
            .collect();
        let insert = super::diff::apply(&old[s..e], &local);
        if insert != old[s..e] {
            out.push(Hunk {
                start: s,
                end: e,
                insert,
            });
        }
    }
    debug_assert!(super::diff::validate(old, &out).is_ok());
    debug_assert!(out.iter().all(|h| is_whole_lines(old, h)), "{out:?}");
    out
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

/// How a pack's text splits into tokens, for D1 (undo spec v1.10, the authored-ranges
/// amendment): two adjacent characters are in one token when [`Tokens::joins`] says so. A
/// token never spans a line break.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tokens {
    /// Whitespace-delimited words (prose, Markdown, plain text). The default.
    #[default]
    Words,
    /// Identifier runs (`[A-Za-z0-9_]`, and any non-ASCII letter or digit); every other
    /// character is a token of its own.
    Code,
    /// Cells: runs between commas and tabs.
    Cells,
    /// Every character a token of its own: D1 widens nothing, so it is the identity (the
    /// undo model check's single-letter alphabets, where the G1 evidence was taken).
    Chars,
    /// Whole lines. No pack uses it: the oracle's `rewrite-whole-line` mutant.
    Lines,
}

impl Tokens {
    /// Whether `a` followed by `b` is inside one token.
    pub fn joins(self, a: char, b: char) -> bool {
        if a == '\n' || b == '\n' || a == '\r' || b == '\r' {
            return false;
        }
        let ident = |c: char| c == '_' || c.is_alphanumeric();
        match self {
            Tokens::Words => !a.is_whitespace() && !b.is_whitespace(),
            Tokens::Code => ident(a) && ident(b),
            Tokens::Cells => !matches!(a, ',' | '\t') && !matches!(b, ',' | '\t'),
            Tokens::Chars => false,
            Tokens::Lines => true,
        }
    }
}

/// Whether byte offset `at` of `s` is strictly inside a token: the characters on both
/// sides of it join.
fn inside(s: &str, at: usize, tokens: Tokens) -> bool {
    match (s[..at].chars().next_back(), s[at..].chars().next()) {
        (Some(a), Some(b)) => tokens.joins(a, b),
        _ => false,
    }
}

/// Widen `[lo, hi)` of `s` to the tokens it is inside of at either end.
fn widen(s: &str, mut lo: usize, mut hi: usize, tokens: Tokens) -> (usize, usize) {
    while inside(s, lo, tokens) {
        lo -= s[..lo].chars().next_back().map_or(1, char::len_utf8);
    }
    while inside(s, hi, tokens) {
        hi += s[hi..].chars().next().map_or(1, char::len_utf8);
    }
    (lo, hi)
}

/// D1 (undo spec v1.10, the authored-ranges amendment): a change is never built from
/// someone else's text. `hunks` are a diff-based change's canonical hunks against `old`
/// (its authored ranges: every hunk of the diff, or ours' hunks for `write --base`).
///
/// Each hunk's range is widened to the tokens of the **new** text it lies inside of: the
/// tokens that hold what it inserted, or that it joined by deleting what separated them.
/// Never past a line break. Inside a widened range, an old character the diff kept stays
/// only if `owned(at)` says the changer owns it (its provenance root's author is the
/// changer); every other kept character is deleted and inserted afresh, as part of the
/// change. So no token of the result holds both the changer's new text and someone else's
/// old text. A token the change only shortened, or split with a separator, is left to its
/// authors. Characters the changer owns are reused as before, so an edit of one's own text
/// is unchanged.
///
/// Returns canonical hunks (D2: sorted, non-overlapping, coalesced when touching). Applied
/// to `old` they give the same text as `hunks`. O(the widened ranges).
pub fn authored_hunks(
    old: &str,
    hunks: &[Hunk],
    tokens: Tokens,
    owned: &dyn Fn(usize) -> bool,
) -> Vec<Hunk> {
    if hunks.is_empty() {
        return Vec::new();
    }
    let new = super::diff::apply(old, hunks);
    // each hunk's window in old coordinates: widened in old, and in new mapped back
    let mut windows: Vec<(usize, usize)> = Vec::with_capacity(hunks.len());
    let mut delta: isize = 0;
    for (k, h) in hunks.iter().enumerate() {
        let (mut lo, mut hi) = (h.start, h.end);
        let ns = (h.start as isize + delta) as usize;
        let ne = ns + h.insert.len();
        let (nlo, nhi) = widen(&new, ns, ne, tokens);
        // new text before the hunk (back to the previous one) is old text shifted by delta;
        // after it (up to the next one), shifted by the delta past this hunk
        let prev_end = if k == 0 { 0 } else { hunks[k - 1].end };
        let after = delta + h.insert.len() as isize - (h.end - h.start) as isize;
        let next_start = hunks.get(k + 1).map_or(old.len(), |n| n.start);
        let old_lo = ((nlo as isize - delta).max(prev_end as isize)) as usize;
        let old_hi = ((nhi as isize - after).min(next_start as isize)) as usize;
        lo = lo.min(old_lo.min(h.start));
        hi = hi.max(old_hi.max(h.end));
        windows.push((lo, hi));
        delta = after;
    }
    // windows that overlap or touch are one
    windows.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(windows.len());
    for (lo, hi) in windows {
        match merged.last_mut() {
            Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
            _ => merged.push((lo, hi)),
        }
    }
    // inside every window: the kept characters the changer doesn't own are rewritten
    let mut out: Vec<Hunk> = hunks.to_vec();
    for (lo, hi) in merged {
        let mut at = lo;
        for h in hunks.iter().filter(|h| h.start >= lo && h.end <= hi) {
            rewrite_foreign(old, at, h.start, owned, &mut out);
            at = h.end;
        }
        rewrite_foreign(old, at, hi, owned, &mut out);
    }
    out.sort_by_key(|h| (h.start, h.end));
    let out = super::diff::coalesce(out);
    debug_assert_eq!(super::diff::apply(old, &out), new, "D1 keeps the text");
    out
}

/// Every character of `old[lo..hi)` (kept by the diff) that the changer doesn't own,
/// rewritten: deleted and inserted afresh, as a hunk of the same text.
fn rewrite_foreign(
    old: &str,
    lo: usize,
    hi: usize,
    owned: &dyn Fn(usize) -> bool,
    out: &mut Vec<Hunk>,
) {
    let mut at = lo;
    while at < hi {
        let c = old[at..].chars().next().expect("on a character boundary");
        let end = at + c.len_utf8();
        if !owned(at) {
            out.push(Hunk {
                start: at,
                end,
                insert: c.to_string(),
            });
        }
        at = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The owner of each character of `old`, by letter: `o` is the changer's, anything else
    /// someone else's.
    fn owned_by(map: &'static str) -> impl Fn(usize) -> bool {
        move |at: usize| map.as_bytes().get(at) == Some(&b'o')
    }

    fn d1(old: &str, new: &str, owners: &'static str) -> Vec<Hunk> {
        let hs = align_insertions(old, crate::text::diff::hunks(old, new));
        let out = authored_hunks(old, &hs, Tokens::Words, &owned_by(owners));
        assert_eq!(crate::text::diff::apply(old, &out), new);
        out
    }

    fn h(start: usize, end: usize, insert: &str) -> Hunk {
        Hunk {
            start,
            end,
            insert: insert.into(),
        }
    }

    #[test]
    fn d1_a_foreign_token_is_rewritten_whole() {
        // the soak's case: p14 replaces p16's line; the diff would keep p16's p1 and 9
        let old = "seed\np16:3029\n";
        let owners = "xxxxxxxxxxxxxxx";
        assert_eq!(
            d1(old, "seed\np14:3169\n", owners),
            vec![h(5, 13, "p14:3169")]
        );
    }

    #[test]
    fn d1_own_text_is_reused_as_before() {
        let old = "seed\np16:3027\n";
        let owners = "xxxxxooooooooox";
        assert_eq!(d1(old, "seed\np16:3029\n", owners), vec![h(12, 13, "9")]);
    }

    #[test]
    fn d1_only_the_touched_token_is_rewritten() {
        let old = "The quick brown fox jumps over\n";
        let owners = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        // jumps -> leaps: the diff keeps "ps"; the token is rewritten, its neighbours not
        assert_eq!(
            d1(old, "The quick brown fox leaps over\n", owners),
            vec![h(20, 25, "leaps")]
        );
    }

    #[test]
    fn d1_whitespace_and_line_breaks_bound_a_token() {
        let old = "alpha beta\ngamma\n";
        let owners = "xxxxxxxxxxxxxxxxxx";
        // a word typed between two words touches neither
        assert_eq!(
            d1(old, "alpha new beta\ngamma\n", owners),
            vec![h(6, 6, "new ")]
        );
        // a line typed between two lines touches neither
        assert_eq!(
            d1(old, "alpha beta\nline\ngamma\n", owners),
            vec![h(11, 11, "line\n")]
        );
    }

    #[test]
    fn d1_typing_into_or_onto_a_foreign_word_takes_the_word() {
        let old = "foo bar\n";
        let owners = "xxxxxxxx";
        // into the middle
        assert_eq!(d1(old, "fXoo bar\n", owners), vec![h(0, 3, "fXoo")]);
        // onto the end: the new token "foobaz" is built from foo
        assert_eq!(d1(old, "foobaz bar\n", owners), vec![h(0, 3, "foobaz")]);
        // deleting the end of a foreign word leaves what is left of it to its author
        assert_eq!(d1(old, "f bar\n", owners), vec![h(1, 3, "")]);
        // splitting a foreign word with a line break or a space takes nothing
        assert_eq!(d1(old, "fo\no bar\n", owners), vec![h(2, 2, "\n")]);
        assert_eq!(d1(old, "fo o bar\n", owners), vec![h(2, 2, " ")]);
        // joining two foreign words by deleting the space builds a token: it is taken
        assert_eq!(d1(old, "foobar\n", owners), vec![h(0, 7, "foobar")]);
    }

    #[test]
    fn d1_mixed_ownership_keeps_the_changer_s_characters() {
        // "ab" is the changer's, "cd" someone else's, one token "abcd"
        let old = "abcd\n";
        let owners = "ooxxx";
        // changing the d: c is foreign and kept by the diff, so it is rewritten; a, b stay
        assert_eq!(d1(old, "abcX\n", owners), vec![h(2, 4, "cX")]);
    }

    proptest::proptest! {
        /// D1 keeps the text, stays canonical, changes nothing when the changer owns every
        /// character, and only ever adds rewrites of kept characters it doesn't own.
        #[test]
        fn d1_keeps_the_text_and_canonical_form(
            old in "[ab :\n]{0,24}",
            new in "[ab :\n]{0,24}",
            mask in proptest::collection::vec(proptest::bool::ANY, 24),
        ) {
            let hs = align_insertions(&old, crate::text::diff::hunks(&old, &new));
            let owned = |at: usize| mask.get(at).copied().unwrap_or(false);
            let out = authored_hunks(&old, &hs, Tokens::Words, &owned);
            proptest::prop_assert_eq!(crate::text::diff::apply(&old, &out), new.clone());
            for w in out.windows(2) {
                proptest::prop_assert!(w[0].end < w[1].start, "canonical: {:?}", out);
            }
            let all = authored_hunks(&old, &hs, Tokens::Words, &|_| true);
            proptest::prop_assert_eq!(all, crate::text::diff::coalesce(hs.clone()));
            // every original hunk lies inside one of the output's
            for h in &hs {
                proptest::prop_assert!(
                    out.iter().any(|o| o.start <= h.start && h.end <= o.end),
                    "{:?} not covered by {:?}", h, out
                );
            }
        }
    }

    proptest::proptest! {
        /// With every character its own token (the undo model check's alphabets), D1 is the
        /// identity on any change and any ownership: the G1 evidence stands as taken.
        #[test]
        fn d1_is_the_identity_on_character_tokens(
            old in "[ab\n]{0,16}",
            new in "[ab\n]{0,16}",
            mask in proptest::collection::vec(proptest::bool::ANY, 16),
        ) {
            let hs = align_insertions(&old, crate::text::diff::hunks(&old, &new));
            let owned = |at: usize| mask.get(at).copied().unwrap_or(false);
            proptest::prop_assert_eq!(
                authored_hunks(&old, &hs, Tokens::Chars, &owned),
                crate::text::diff::coalesce(hs.clone())
            );
        }
    }

    #[test]
    fn d1_tokens_follow_the_rule() {
        assert!(Tokens::Words.joins('a', ':'));
        assert!(!Tokens::Words.joins('a', ' '));
        assert!(!Tokens::Words.joins('a', '\n'));
        assert!(Tokens::Code.joins('a', '_'));
        assert!(!Tokens::Code.joins('a', '.'));
        assert!(Tokens::Cells.joins('a', ' '));
        assert!(!Tokens::Cells.joins('a', ','));
    }
    use crate::text::diff;
    use proptest::prelude::*;

    fn align(old: &str, new: &str) -> Vec<Hunk> {
        align_insertions(old, diff::hunks(old, new))
    }

    /// An edit given as a new text pairs lines first (v0.9.7.1): a deleted or inserted line
    /// next to a line with the same start is that line, never its end and the neighbour's
    /// start; inside a line the diff still goes to the character.
    #[test]
    fn edit_hunks_pair_lines_first() {
        let h = |start, end, insert: &str| Hunk {
            start,
            end,
            insert: insert.into(),
        };
        let old = "seed:1\nseed:2\nseed:3\n";
        // deleting seed:2: the line (diff::hunks deletes "2\nseed:")
        assert_eq!(edit_hunks(old, "seed:1\nseed:3\n"), vec![h(7, 14, "")]);
        assert_eq!(diff::hunks(old, "seed:1\nseed:3\n"), vec![h(12, 19, "")]);
        // inserting seed:9 before seed:3: at the line start
        assert_eq!(
            edit_hunks(old, "seed:1\nseed:2\nseed:9\nseed:3\n"),
            vec![h(14, 14, "seed:9\n")]
        );
        // a word inside a line: the character diff
        assert_eq!(
            edit_hunks(old, "seed:1\nseed:X\nseed:3\n"),
            vec![h(12, 13, "X")]
        );
        for new in [
            "",
            "x\n",
            "seed:1\n",
            "seed:3\nseed:2\nseed:1\n",
            "seed:1\nseed:2",
        ] {
            let e = edit_hunks(old, new);
            assert!(diff::validate(old, &e).is_ok());
            assert_eq!(diff::apply(old, &e), new);
        }
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
