//! Hunks over plain strings, the oracle's own: canonical form (D2), apply, carry (U8, U9),
//! and the provenance roots of what a hunk inserts (U23, v1.8).

use k2_mpc_base::text::diff::Hunk;

use super::ChangeId;

/// A provenance root (U23, v1.8), the oracle's naming: the change that first inserted the
/// element and the element's ordinal in that change. The import's elements are change 0
/// (numbered across roots, in order); an element an edit, `write --base`, `replace_text`,
/// `restore` or purge inserts is its own root; one an undo, redo or revert inserts records
/// the root of the element it puts back (path-compressed: never a chain). Also an element's
/// own id (the same naming: its change and ordinal), which the rules never read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Prov(pub ChangeId, pub u32);

/// Replace `[start, end)` (UTF-8 byte offsets, on character boundaries) with `insert`. The
/// oracle's own hunk, `Hash`-able for state enumeration; convertible from and to the kit's
/// [`Hunk`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub insert: String,
}

impl Edit {
    pub fn is_insertion(&self) -> bool {
        self.start == self.end
    }

    /// How much longer the text gets.
    pub fn growth(&self) -> isize {
        self.insert.len() as isize - (self.end - self.start) as isize
    }
}

impl From<Hunk> for Edit {
    fn from(h: Hunk) -> Self {
        Edit {
            start: h.start,
            end: h.end,
            insert: h.insert,
        }
    }
}

impl From<Edit> for Hunk {
    fn from(e: Edit) -> Self {
        Hunk {
            start: e.start,
            end: e.end,
            insert: e.insert,
        }
    }
}

/// D2's canonical form of sorted, non-overlapping edits: empty ones dropped, touching ones
/// joined (`[1,2)→x` then `[2,3)→y` is `[1,3)→xy`; an insertion at a range's start or end
/// joins it).
pub fn canonical(edits: Vec<Edit>) -> Vec<Edit> {
    let mut out: Vec<Edit> = Vec::with_capacity(edits.len());
    for e in edits {
        if e.start == e.end && e.insert.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.end == e.start => {
                last.end = e.end;
                last.insert.push_str(&e.insert);
            }
            _ => out.push(e),
        }
    }
    out
}

/// [`canonical`] for edits with the provenance roots of what each inserts (one per inserted
/// character): joined edits join their roots.
pub fn canonical_provs(edits: Vec<(Edit, Vec<Prov>)>) -> Vec<(Edit, Vec<Prov>)> {
    let mut out: Vec<(Edit, Vec<Prov>)> = Vec::with_capacity(edits.len());
    for (e, p) in edits {
        if e.start == e.end && e.insert.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some((last, lp)) if last.end == e.start => {
                last.end = e.end;
                last.insert.push_str(&e.insert);
                lp.extend(p);
            }
            _ => out.push((e, p)),
        }
    }
    out
}

/// The provenance roots of what each of `edits` (canonical, on `base`) inserts, read off
/// `target`'s roots (one per character of `apply(base, edits)`): an inserted character's
/// root is the target character's it becomes.
pub fn provs_of(base: &str, edits: &[Edit], target: &[Prov]) -> Vec<Vec<Prov>> {
    let text = apply(base, edits);
    debug_assert_eq!(text.chars().count(), target.len(), "one root per character");
    let mut out = Vec::with_capacity(edits.len());
    let mut shift: isize = 0;
    for e in edits {
        let at = (e.start as isize + shift) as usize;
        let i = text[..at].chars().count();
        out.push(target[i..i + e.insert.chars().count()].to_vec());
        shift += e.growth();
    }
    out
}

/// The characters of `text` in its byte range `[start, end)`, as character indices.
pub fn chars_in(text: &str, start: usize, end: usize) -> std::ops::Range<usize> {
    let i = text[..start].chars().count();
    i..i + text[start..end].chars().count()
}

/// `old` with `edits` (sorted, non-overlapping, valid on `old`) applied.
pub fn apply(old: &str, edits: &[Edit]) -> String {
    let mut out = String::with_capacity(old.len());
    let mut at = 0;
    for e in edits {
        out.push_str(&old[at..e.start]);
        out.push_str(&e.insert);
        at = e.end;
    }
    out.push_str(&old[at..]);
    out
}

/// Which way a carried bound goes for an insertion exactly at it (U8: an insertion at a
/// region's start or end is pulled inside the region).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// A region's start: stays before an insertion at it.
    Start,
    /// A region's end: moves past an insertion at it.
    End,
}

/// Carry a bound through a later change (U8), like a caret:
/// - an edit entirely before it (ending at or before it) shifts it, except a pure insertion
///   exactly at it, which goes per `side` (pulled inside the region);
/// - an edit entirely after it (starting at or after it) leaves it alone;
/// - **reading R1** (the spec leaves it open; the region is tainted then, U9): a bound
///   strictly inside a deleted or replaced range moves to where that range was (the edit's
///   start, shifted by the edits before it).
pub fn carry(pos: usize, edits: &[Edit], side: Side) -> usize {
    let mut shift: isize = 0;
    for e in edits {
        if e.is_insertion() && e.start == pos {
            if side == Side::End {
                shift += e.growth();
            }
            continue;
        }
        if e.end <= pos {
            shift += e.growth();
        } else if e.start < pos {
            return (e.start as isize + shift) as usize;
        }
    }
    (pos as isize + shift) as usize
}

/// U9: an edit deleting across `pos` (starting strictly before it, ending strictly after).
pub fn crosses(pos: usize, edits: &[Edit]) -> bool {
    edits.iter().any(|e| e.start < pos && pos < e.end)
}

/// The 1-based line holding byte `at` of `text`.
pub fn line_at(text: &str, at: usize) -> usize {
    1 + text.as_bytes()[..at]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(s: usize, t: usize, i: &str) -> Edit {
        Edit {
            start: s,
            end: t,
            insert: i.into(),
        }
    }

    #[test]
    fn carry_rules() {
        // insertion exactly at the bound: per side
        assert_eq!(carry(3, &[e(3, 3, "xy")], Side::Start), 3);
        assert_eq!(carry(3, &[e(3, 3, "xy")], Side::End), 5);
        // a replacement ending at the bound is before it; one starting at it is after it
        assert_eq!(carry(3, &[e(1, 3, "xyz")], Side::Start), 4);
        assert_eq!(carry(3, &[e(3, 5, "xyz")], Side::End), 3);
        // strictly inside: where the range was
        assert_eq!(carry(3, &[e(0, 1, ""), e(2, 5, "q")], Side::End), 1);
        assert!(crosses(3, &[e(2, 5, "")]));
        assert!(!crosses(3, &[e(3, 5, "")]));
    }

    #[test]
    fn canonical_joins_touching() {
        assert_eq!(
            canonical(vec![e(1, 2, "x"), e(2, 3, "y"), e(5, 5, ""), e(7, 7, "z")]),
            vec![e(1, 3, "xy"), e(7, 7, "z")]
        );
        assert_eq!(
            canonical(vec![e(2, 2, "a"), e(2, 2, "b")]),
            vec![e(2, 2, "ab")]
        );
    }
}
