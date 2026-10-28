//! The foreign hunks a region carries (undo spec v1.7, U10).
//!
//! An undo, redo or revert merges two **exact hunk sets** on a region's base (the text the
//! change left in the region): **ours**, the inverse of the change's own hunks, and
//! **theirs**, every later hunk that touched the region's carried window, composed into
//! one canonical set in base coordinates. Neither side is ever found by diffing strings:
//! in repeated text (`b` and an inserted `b`) a diff can't tell which element is whose, so
//! a merge on diffed sides can delete another participant's identical element (finding
//! F3).
//!
//! Theirs is kept as **shapes** `(start, end, inserted length)`: the inserted text is
//! always the live window's, so it is read from there when the region is merged
//! ([`materialize`]). The applied-hunk stream reports every change at element-identity
//! positions (E-HIST-5), so composing its hunks gives exactly which base elements are gone
//! and which foreign elements stand where.
//!
//! **Composition.** The window is a sequence of pieces: runs of surviving base elements and
//! runs of foreign elements. A later hunk inside the window (window coordinates) removes
//! the pieces it covers and puts its own inserted run in their place. The canonical form
//! is read off the surviving base runs: between two of them (or the window's ends), the
//! base elements missing and the foreign runs present make one hunk. Sorted,
//! non-overlapping, never empty, joined when touching: D2's form, so the merge's pair rules
//! (O1–O5) read it exactly as they read a diff's hunks.
//!
//! **Window membership** (U8): a later hunk is inside the window when it lies within its
//! carried bounds; a pure insertion exactly at the start or end is inside (pulled in). A
//! hunk deleting across an edge taints the region (U9): a tainted region always conflicts,
//! so its theirs is no longer kept. At a shared edge (U8) an insertion belongs to the left
//! region only: the right region's carried start moves past it ([`trim_front`]).

use super::position::Bias;
use crate::text::diff::Hunk;

/// One foreign hunk in a region's base coordinates: `(start, end, inserted length)`.
pub(crate) type Shape = (usize, usize, usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Piece {
    /// Surviving base elements `start..end`.
    Base(usize, usize),
    /// A run of foreign elements, this many bytes long.
    Foreign(usize),
}

impl Piece {
    fn len(self) -> usize {
        match self {
            Piece::Base(s, e) => e - s,
            Piece::Foreign(n) => n,
        }
    }

    /// The part of this piece from byte `a` to byte `b` of it.
    fn slice(self, a: usize, b: usize) -> Piece {
        match self {
            Piece::Base(s, _) => Piece::Base(s + a, s + b),
            Piece::Foreign(_) => Piece::Foreign(b - a),
        }
    }
}

/// The window's pieces, in order.
fn pieces(base_len: usize, hs: &[Shape]) -> Vec<Piece> {
    let mut out = Vec::with_capacity(2 * hs.len() + 1);
    let mut at = 0;
    for &(s, e, n) in hs {
        if s > at {
            out.push(Piece::Base(at, s));
        }
        if n > 0 {
            out.push(Piece::Foreign(n));
        }
        at = e;
    }
    if base_len > at {
        out.push(Piece::Base(at, base_len));
    }
    out
}

/// The canonical shapes of a window given as pieces.
fn canonical(base_len: usize, ps: &[Piece]) -> Vec<Shape> {
    let mut out = Vec::new();
    let (mut at, mut ins) = (0usize, 0usize);
    for p in ps {
        match *p {
            Piece::Base(s, e) => {
                if s > at || ins > 0 {
                    out.push((at, s, ins));
                }
                ins = 0;
                at = e;
            }
            Piece::Foreign(n) => ins += n,
        }
    }
    if base_len > at || ins > 0 {
        out.push((at, base_len, ins));
    }
    out
}

/// The window's length: the base's, changed by every foreign hunk.
pub(crate) fn window_len(base_len: usize, hs: &[Shape]) -> usize {
    let grow: isize = hs
        .iter()
        .map(|&(s, e, n)| n as isize - (e - s) as isize)
        .sum();
    (base_len as isize + grow) as usize
}

/// Compose one later hunk `(s, e, n)` given in **window** coordinates (`s <= e <=` the
/// window's length) into `hs`.
pub(crate) fn compose(hs: &mut Vec<Shape>, base_len: usize, s: usize, e: usize, n: usize) {
    let ps = pieces(base_len, hs);
    let mut out = Vec::with_capacity(ps.len() + 2);
    let mut at = 0;
    let mut placed = false;
    for p in ps {
        let (lo, hi) = (at, at + p.len());
        at = hi;
        // the part before the hunk
        if lo < s {
            out.push(p.slice(0, hi.min(s) - lo));
        }
        // the hunk's own run, right after everything that ends at or before its start
        if !placed && hi > s {
            if n > 0 {
                out.push(Piece::Foreign(n));
            }
            placed = true;
        }
        // the part after the hunk
        if hi > e {
            out.push(p.slice(lo.max(e) - lo, hi - lo));
        }
    }
    if !placed && n > 0 {
        out.push(Piece::Foreign(n));
    }
    debug_assert!(at >= e, "a hunk past the window's end");
    *hs = canonical(base_len, &out);
}

/// Drop the first `n` bytes of the window: foreign elements an insertion at a shared edge
/// put there, which belong to the left region (U8). `false` when the window doesn't start
/// with `n` bytes of foreign elements (then the caller treats the region as tainted).
pub(crate) fn trim_front(hs: &mut Vec<Shape>, n: usize) -> bool {
    if n == 0 {
        return true;
    }
    match hs.first_mut() {
        Some(first) if first.0 == 0 && first.2 >= n => {
            first.2 -= n;
            if first.1 == 0 && first.2 == 0 {
                hs.remove(0);
            }
            true
        }
        _ => false,
    }
}

/// Theirs as hunks with their inserted text, read from the live `window`; `None` when the
/// window's surviving base runs don't match `base` (a carry error: never on a correct
/// stream).
pub(crate) fn materialize(hs: &[Shape], base: &str, window: &str) -> Option<Vec<Hunk>> {
    if window.len() != window_len(base.len(), hs) {
        return None;
    }
    let mut out = Vec::with_capacity(hs.len());
    let (mut w, mut b) = (0usize, 0usize);
    for &(s, e, n) in hs {
        let keep = s - b;
        if window.get(w..w + keep)? != base.get(b..s)? {
            return None;
        }
        w += keep;
        out.push(Hunk {
            start: s,
            end: e,
            insert: window.get(w..w + n)?.to_owned(),
        });
        w += n;
        b = e;
    }
    (window.get(w..)? == base.get(b..)?).then_some(out)
}

/// What carrying one window edge through a change found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Crossed {
    pub start: bool,
    pub end: bool,
}

/// Carry a window `[start, end]` (in the text before the change) through one change's
/// canonical hunks `(start, end, inserted length)` on its root: which edges a hunk
/// deletes across (U9), and, unless `keep` is false (a tainted region keeps no theirs),
/// every hunk inside the window composed into `theirs`. The caller moves the bounds
/// themselves (start [`super::position::Bias::Before`], end `After`).
pub(crate) fn carry_window(
    start: usize,
    end: usize,
    hunks: &[Shape],
    theirs: &mut Vec<Shape>,
    base_len: usize,
    keep: bool,
) -> Crossed {
    let crosses = |p: usize| hunks.iter().any(|&(s, e, _)| s < p && p < e);
    let c = Crossed {
        start: crosses(start),
        end: crosses(end),
    };
    if !keep || c.start || c.end {
        theirs.clear();
        return c;
    }
    // back to front: the earlier hunks keep their window offsets
    for &(s, e, n) in hunks.iter().rev() {
        let inside = if s == e {
            start <= s && s <= end
        } else {
            start <= s && e <= end
        };
        if inside {
            compose(theirs, base_len, s - start, e - start, n);
        }
    }
    debug_assert!(theirs
        .iter()
        .all(|&(s, e, n)| s <= e && e <= base_len && (s < e || n > 0)));
    c
}

/// The inverse of `hunks` (canonical, applied to `old`): hunks on the new text that give
/// `old` back.
pub(crate) fn inverse(old: &str, hunks: &[Hunk]) -> Vec<Hunk> {
    let mut shift: isize = 0;
    hunks
        .iter()
        .map(|h| {
            let s = (h.start as isize + shift) as usize;
            shift += h.insert.len() as isize - (h.end - h.start) as isize;
            Hunk {
                start: s,
                end: s + h.insert.len(),
                insert: old[h.start..h.end].to_owned(),
            }
        })
        .collect()
}

/// [`super::position::transform`] on shapes (the store keeps later changes as shapes).
pub(crate) fn transform_shape(pos: usize, hunks: &[Shape], bias: Bias) -> usize {
    let mut delta: isize = 0;
    for &(s, e, n) in hunks {
        if pos < s {
            break;
        }
        let grow = n as isize - (e - s) as isize;
        if s == e {
            if pos == s && bias == Bias::Before {
                break;
            }
            delta += grow;
            continue;
        }
        if pos == s {
            break;
        }
        if pos < e {
            return (s as isize + delta) as usize;
        }
        delta += grow;
    }
    (pos as isize + delta) as usize
}

/// A change's hunks as shapes.
pub(crate) fn shapes(hunks: &[Hunk]) -> Vec<Shape> {
    hunks
        .iter()
        .map(|h| (h.start, h.end, h.insert.len()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::diff;
    use proptest::prelude::*;

    /// A window as elements: `Ok(base index)` or `Err(foreign char)`.
    type El = Result<usize, char>;

    fn expand(base: &str, hs: &[Hunk]) -> Vec<El> {
        let mut out = Vec::new();
        let mut at = 0;
        for h in hs {
            out.extend((at..h.start).map(Ok));
            out.extend(h.insert.chars().map(Err));
            at = h.end;
        }
        out.extend((at..base.len()).map(Ok));
        out
    }

    /// The reference: theirs by element identity, from the element list.
    fn collapse(base: &str, els: &[El]) -> Vec<Hunk> {
        let mut out = Vec::new();
        let (mut at, mut ins) = (0, String::new());
        for x in els {
            match x {
                Ok(i) => {
                    if *i > at || !ins.is_empty() {
                        out.push(Hunk {
                            start: at,
                            end: *i,
                            insert: std::mem::take(&mut ins),
                        });
                    }
                    at = i + 1;
                }
                Err(c) => ins.push(*c),
            }
        }
        if base.len() > at || !ins.is_empty() {
            out.push(Hunk {
                start: at,
                end: base.len(),
                insert: ins,
            });
        }
        out
    }

    #[test]
    fn f3_insertion_at_the_window_start_is_before_the_base() {
        // base "b" (p1's replacement); p2 inserts "b" at the window's start
        let mut hs = Vec::new();
        compose(&mut hs, 1, 0, 0, 1);
        assert_eq!(hs, vec![(0, 0, 1)]);
        let th = materialize(&hs, "b", "bb").unwrap();
        assert_eq!(
            th,
            vec![Hunk {
                start: 0,
                end: 0,
                insert: "b".into()
            }]
        );
        // a diff would say the insertion is after the base's own `b`
        assert_eq!(diff::hunks("b", "bb")[0].start, 1);
    }

    #[test]
    fn a_deletion_of_foreign_text_and_base_joins() {
        // base "abc"; X inserted at 1 ("aXbc"); then "Xb" deleted ("ac")
        let mut hs = Vec::new();
        compose(&mut hs, 3, 1, 1, 1);
        compose(&mut hs, 3, 1, 3, 0);
        assert_eq!(hs, vec![(1, 2, 0)]);
        assert_eq!(materialize(&hs, "abc", "ac").unwrap()[0].insert, "");
        // a mismatching window is refused
        assert!(materialize(&hs, "abc", "ab").is_none());
    }

    #[test]
    fn trim_front_drops_a_shared_edge_insertion() {
        let mut hs = vec![(0, 1, 3)];
        assert!(trim_front(&mut hs, 2));
        assert_eq!(hs, vec![(0, 1, 1)]);
        let mut hs = vec![(0, 0, 2), (3, 4, 0)];
        assert!(trim_front(&mut hs, 2));
        assert_eq!(hs, vec![(3, 4, 0)]);
        assert!(!trim_front(&mut vec![(1, 1, 2)], 1));
    }

    fn draw() -> impl Strategy<Value = (String, Vec<(u16, u8, String)>)> {
        (
            "[ab]{0,6}",
            prop::collection::vec((any::<u16>(), 0u8..3, "[xy]{0,2}"), 0..6),
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2048))]
        /// The shape transform is the position transform.
        #[test]
        fn transform_shape_is_transform(
            raw in prop::collection::vec((0usize..12, 0usize..3, 0usize..3), 0..4),
            pos in 0usize..14,
            before in any::<bool>(),
        ) {
            let mut hs: Vec<Hunk> = Vec::new();
            for (s, d, n) in raw {
                let s = s.max(hs.last().map_or(0, |h| h.end + 1));
                if (d == 0 && n == 0) || s + d > 14 { continue; }
                hs.push(Hunk { start: s, end: s + d, insert: "x".repeat(n) });
            }
            let bias = if before { Bias::Before } else { Bias::After };
            prop_assert_eq!(
                transform_shape(pos, &shapes(&hs), bias),
                crate::session::position::transform(pos, &hs, bias)
            );
        }

        /// Composing shapes equals the element-level reference, and materialising gives
        /// back hunks that rebuild the window.
        #[test]
        fn compose_equals_the_element_reference((base, ops) in draw()) {
            let mut hs: Vec<Shape> = Vec::new();
            let mut els: Vec<El> = expand(&base, &[]);
            let mut window = base.clone();
            for (pos, del, ins) in ops {
                let s = pos as usize % (window.len() + 1);
                let e = (s + del as usize).min(window.len());
                if s == e && ins.is_empty() {
                    continue;
                }
                compose(&mut hs, base.len(), s, e, ins.len());
                els.splice(s..e, ins.chars().map(Err));
                window.replace_range(s..e, &ins);
                let want = collapse(&base, &els);
                let got = materialize(&hs, &base, &window).expect("window matches");
                prop_assert_eq!(&got, &want);
                prop_assert_eq!(diff::apply(&base, &got), window.clone());
            }
        }
    }
}
