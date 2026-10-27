//! Positions in a text root: byte offsets, `line:col`, and how positions move through edits.
//!
//! Inside the kit a position is a UTF-8 byte offset into a root's current text, always on a
//! character boundary. People and command-line verbs speak `line:col` instead
//! ([`LineCol`]: 1-based lines, 1-based columns counted in Unicode scalar values).
//!
//! Stored positions (carets, viewing ranges, undo regions) are carried through every change
//! by [`transform`], so they keep pointing at the same text while others edit.

use crate::text::diff::Hunk;

/// Which way a position goes when text is inserted exactly at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bias {
    /// The position stays where it is; the inserted text lands after it.
    Before,
    /// The position moves to the end of the inserted text (a caret pushed along by typing,
    /// as a Yjs relative position does).
    After,
}

/// Where byte offset `pos` of the old text lands after `hunks` (sorted, non-overlapping,
/// in old-text coordinates, as [`crate::text::diff::validate`] accepts) are applied.
///
/// - before a hunk: unchanged;
/// - at the start of a replacement or deletion: unchanged (it stays at the start);
/// - strictly inside a replaced or deleted range: snaps to the start of the hunk;
/// - at the end of a replaced range, or after it: shifted by the size change;
/// - exactly at an insertion: per `bias`.
///
/// A char-boundary `pos <= old.len()` maps to a char boundary `<= new.len()`.
pub fn transform(pos: usize, hunks: &[Hunk], bias: Bias) -> usize {
    let mut delta: isize = 0;
    for h in hunks {
        if pos < h.start {
            break;
        }
        let grow = h.insert.len() as isize - (h.end - h.start) as isize;
        if h.start == h.end {
            if pos == h.start && bias == Bias::Before {
                break;
            }
            delta += grow;
            continue;
        }
        if pos == h.start {
            break;
        }
        if pos < h.end {
            return (h.start as isize + delta) as usize;
        }
        delta += grow;
    }
    (pos as isize + delta) as usize
}

/// A 1-based line and a 1-based column counted in characters (Unicode scalar values).
/// The column after the last character of a line is `chars + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LineCol {
    pub line: usize,
    pub col: usize,
}

/// Number of lines in `text`: one more than its `\n` count (a final newline is followed by
/// an empty last line, as editors show it).
pub fn line_count(text: &str) -> usize {
    text.bytes().filter(|&b| b == b'\n').count() + 1
}

/// The 1-based line holding byte offset `byte` (clamped to the text).
pub fn line_of(text: &str, byte: usize) -> usize {
    let b = byte.min(text.len());
    text.as_bytes()[..b].iter().filter(|&&c| c == b'\n').count() + 1
}

/// `line:col` of byte offset `byte`, or `None` when it is past the end or not on a
/// character boundary.
pub fn line_col(text: &str, byte: usize) -> Option<LineCol> {
    if !text.is_char_boundary(byte) {
        return None;
    }
    let line_start = text[..byte].rfind('\n').map_or(0, |i| i + 1);
    Some(LineCol {
        line: line_of(text, byte),
        col: text[line_start..byte].chars().count() + 1,
    })
}

/// Byte offset where 1-based `line` starts, or `None` when the text has fewer lines.
pub fn line_start(text: &str, line: usize) -> Option<usize> {
    if line == 0 {
        return None;
    }
    if line == 1 {
        return Some(0);
    }
    text.bytes()
        .enumerate()
        .filter(|&(_, b)| b == b'\n')
        .nth(line - 2)
        .map(|(i, _)| i + 1)
}

/// Byte offset of the end of the line starting at `start` (its `\n`, or the end of text).
pub(crate) fn line_end_from(text: &str, start: usize) -> usize {
    text[start..].find('\n').map_or(text.len(), |i| start + i)
}

/// Byte offset of `at`, or `None` when the line doesn't exist or the column is past the
/// line's end (`chars + 1` is the end itself).
pub fn offset(text: &str, at: LineCol) -> Option<usize> {
    if at.col == 0 {
        return None;
    }
    let start = line_start(text, at.line)?;
    let end = line_end_from(text, start);
    let line = &text[start..end];
    if at.col == 1 {
        return Some(start);
    }
    match line.char_indices().nth(at.col - 1) {
        Some((i, _)) => Some(start + i),
        None if line.chars().count() + 1 == at.col => Some(end),
        None => None,
    }
}

/// A caret or selection in one root, in byte offsets of its current text. `anchor == head`
/// is a plain caret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub root: String,
    pub anchor: usize,
    pub head: usize,
}

/// Lines `first..=last` (1-based, inclusive) of one root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineRange {
    pub root: String,
    pub first: usize,
    pub last: usize,
}

/// A position as a caller gives it: a byte offset, or `line:col` (`cursor --at L:C`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At {
    Byte(usize),
    Line(LineCol),
}

impl At {
    /// The byte offset in `text`, or `None` when out of range or inside a character.
    pub fn resolve(self, text: &str) -> Option<usize> {
        match self {
            At::Byte(b) => text.is_char_boundary(b).then_some(b),
            At::Line(lc) => offset(text, lc),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::diff;
    use proptest::prelude::*;

    fn h(start: usize, end: usize, insert: &str) -> Hunk {
        Hunk {
            start,
            end,
            insert: insert.into(),
        }
    }

    #[test]
    fn transform_rules() {
        // "hello world": replace "world" (6..11) with "there!"
        let hs = [h(6, 11, "there!")];
        assert_eq!(transform(0, &hs, Bias::After), 0, "before: unchanged");
        assert_eq!(
            transform(6, &hs, Bias::After),
            6,
            "at a replacement's start: stays"
        );
        assert_eq!(transform(8, &hs, Bias::After), 6, "inside: snaps to start");
        assert_eq!(transform(11, &hs, Bias::After), 12, "at its end: shifts");
        // insertion at 5
        let ins = [h(5, 5, "XY")];
        assert_eq!(transform(5, &ins, Bias::Before), 5);
        assert_eq!(transform(5, &ins, Bias::After), 7);
        assert_eq!(transform(6, &ins, Bias::Before), 8);
        // insertion then a replacement starting at the same point
        let both = [h(3, 3, "ab"), h(3, 5, "")];
        assert_eq!(transform(3, &both, Bias::Before), 3);
        assert_eq!(transform(3, &both, Bias::After), 5);
        assert_eq!(
            transform(4, &both, Bias::After),
            5,
            "inside the deletion, after the insert"
        );
        assert_eq!(transform(5, &both, Bias::After), 5);
    }

    #[test]
    fn line_col_round_trips() {
        let t = "ab\nçé🎉x\n\nlast";
        assert_eq!(line_col(t, 0), Some(LineCol { line: 1, col: 1 }));
        assert_eq!(line_col(t, 2), Some(LineCol { line: 1, col: 3 }));
        let x = t.find('x').unwrap();
        assert_eq!(line_col(t, x), Some(LineCol { line: 2, col: 4 }));
        assert_eq!(line_col(t, x - 1), None, "inside the emoji");
        assert_eq!(offset(t, LineCol { line: 2, col: 4 }), Some(x));
        assert_eq!(
            offset(t, LineCol { line: 2, col: 5 }),
            Some(x + 1),
            "end of line"
        );
        assert_eq!(offset(t, LineCol { line: 2, col: 6 }), None);
        assert_eq!(offset(t, LineCol { line: 3, col: 1 }), Some(x + 2));
        assert_eq!(offset(t, LineCol { line: 4, col: 5 }), Some(t.len()));
        assert_eq!(offset(t, LineCol { line: 5, col: 1 }), None);
        assert_eq!(offset(t, LineCol { line: 0, col: 1 }), None);
        assert_eq!(line_count(t), 4);
        assert_eq!(line_count(""), 1);
        for (i, _) in t.char_indices().chain([(t.len(), ' ')]) {
            assert_eq!(offset(t, line_col(t, i).unwrap()), Some(i), "at {i}");
        }
    }

    fn text() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            prop_oneof![
                Just('a'),
                Just('b'),
                Just('\n'),
                Just('é'),
                Just('🎉'),
                Just('中')
            ],
            0..40,
        )
        .prop_map(|v| v.into_iter().collect())
    }

    proptest! {
        /// Positions land on char boundaries within the new text, keep their order, and
        /// text untouched by any hunk keeps its position relative to its neighbours.
        #[test]
        fn transform_stays_on_boundaries(old in text(), new in text(), picks in proptest::collection::vec(0usize..64, 1..12)) {
            let hs = diff::hunks(&old, &new);
            let bounds: Vec<usize> = old.char_indices().map(|(i, _)| i).chain([old.len()]).collect();
            let mut prev: Option<(usize, usize)> = None;
            let mut sorted: Vec<usize> = picks.iter().map(|p| bounds[p % bounds.len()]).collect();
            sorted.sort();
            for pos in sorted {
                for bias in [Bias::Before, Bias::After] {
                    let n = transform(pos, &hs, bias);
                    prop_assert!(n <= new.len(), "{pos} -> {n} past end {}", new.len());
                    prop_assert!(new.is_char_boundary(n), "{pos} -> {n} inside a char");
                }
                let n = transform(pos, &hs, Bias::After);
                if let Some((pp, pn)) = prev {
                    prop_assert!(pp > pos || pn <= n, "order broken: {pp}->{pn}, {pos}->{n}");
                }
                prev = Some((pos, n));
                // a position outside every hunk keeps the character after it
                if hs.iter().all(|h| pos < h.start || pos > h.end) && pos < old.len() {
                    let c = old[pos..].chars().next().unwrap();
                    prop_assert_eq!(new[n..].chars().next(), Some(c));
                }
            }
        }
    }
}
