//! A region's window at element identity (spec v1.7, U8–U10), the oracle's own.
//!
//! The window is held as its **elements**: each character is either the base's (the text
//! the change left in the region; by its byte offset there) or foreign (inserted by a later
//! change). Theirs (U10) is read off the elements: between two surviving base characters
//! (or the window's ends), the base characters missing and the foreign characters present
//! are one edit, in base coordinates. Ours is the inverse of the change's own edits. No
//! string is ever diffed here.
//!
//! **v1.8 (U8 by root):** at merge time the live window is read against the base by a key
//! per character ([`read`]): the **provenance root** (U23) for the merge, so text someone
//! removed and put back is the base's again; the element's **id** for the v1.7 reading,
//! which the oracle keeps as a cross-check of the carried theirs ([`carry_theirs`]).

use super::hunk::{self, Edit, Prov};

/// One element of a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum El {
    /// The base character starting at this byte of the base.
    Base(usize, char),
    Foreign(char),
}

impl El {
    fn ch(self) -> char {
        match self {
            El::Base(_, c) | El::Foreign(c) => c,
        }
    }
}

/// The window's elements, from the base and theirs.
fn elements(base: &str, theirs: &[Edit]) -> Vec<El> {
    let mut out = Vec::new();
    let mut at = 0;
    let push_base = |out: &mut Vec<El>, from: usize, to: usize| {
        out.extend(
            base[from..to]
                .char_indices()
                .map(|(i, c)| El::Base(from + i, c)),
        );
    };
    for e in theirs {
        push_base(&mut out, at, e.start);
        out.extend(e.insert.chars().map(El::Foreign));
        at = e.end;
    }
    push_base(&mut out, at, base.len());
    out
}

/// Theirs, read off the elements (canonical: D2's form).
fn theirs_of(base: &str, els: &[El]) -> Vec<Edit> {
    let mut out = Vec::new();
    let mut next = 0; // the base byte after the last surviving base character
    let mut ins = String::new();
    for el in els {
        match *el {
            El::Base(i, c) => {
                if i > next || !ins.is_empty() {
                    out.push(Edit {
                        start: next,
                        end: i,
                        insert: std::mem::take(&mut ins),
                    });
                }
                next = i + c.len_utf8();
            }
            El::Foreign(c) => ins.push(c),
        }
    }
    if base.len() > next || !ins.is_empty() {
        out.push(Edit {
            start: next,
            end: base.len(),
            insert: ins,
        });
    }
    out
}

/// The live window read against the base (U8, v1.8): theirs (canonical edits on `base`),
/// the provenance roots of what each theirs edit inserts (`win_provs`' characters), and each
/// window character's base index or `None` (foreign). `base_keys` and `win_keys` are one key
/// per character (roots for the rule, element ids for the v1.7 reading). **The greedy rule:**
/// scanning the window left to right, a character whose key is base character *i*'s counts
/// as base *i* when *i* is greater than the last base index counted (the least such *i*);
/// every other character is foreign.
pub fn read(
    base: &str,
    base_keys: &[Prov],
    win: &str,
    win_keys: &[Prov],
    win_provs: &[Prov],
) -> (Vec<Edit>, Vec<Vec<Prov>>, Vec<Option<usize>>) {
    let bchars: Vec<(usize, char)> = base.char_indices().collect();
    debug_assert_eq!(bchars.len(), base_keys.len());
    debug_assert_eq!(win.chars().count(), win_keys.len());
    let mut counted = Vec::with_capacity(win_keys.len());
    let mut last: Option<usize> = None;
    for k in win_keys {
        let from = last.map_or(0, |l| l + 1);
        let i = (from..base_keys.len()).find(|&i| base_keys[i] == *k);
        if i.is_some() {
            last = i;
        }
        counted.push(i);
    }
    // theirs off the counted base characters, as theirs_of does
    let (mut out, mut provs) = (Vec::new(), Vec::new());
    let (mut next, mut ins, mut ip) = (0, String::new(), Vec::new());
    for ((c, i), p) in win.chars().zip(&counted).zip(win_provs) {
        match *i {
            Some(i) => {
                let (at, bc) = bchars[i];
                if at > next || !ins.is_empty() {
                    out.push(Edit {
                        start: next,
                        end: at,
                        insert: std::mem::take(&mut ins),
                    });
                    provs.push(std::mem::take(&mut ip));
                }
                next = at + bc.len_utf8();
            }
            None => {
                ins.push(c);
                ip.push(*p);
            }
        }
    }
    if base.len() > next || !ins.is_empty() {
        out.push(Edit {
            start: next,
            end: base.len(),
            insert: ins,
        });
        provs.push(ip);
    }
    (out, provs, counted)
}

/// The element index at window byte `at`.
fn index_at(els: &[El], at: usize) -> usize {
    let mut b = 0;
    for (i, el) in els.iter().enumerate() {
        if b >= at {
            return i;
        }
        b += el.ch().len_utf8();
    }
    assert_eq!(b, at, "a window offset past its end");
    els.len()
}

/// Carry a region's window `[start, end]` through a later change `es` (canonical, in the
/// text before it) on its root: U9's taint (an edit deleting across an edge), and, when the
/// region isn't tainted, every edit inside the window (U8: within the bounds; a pure
/// insertion exactly at an edge is inside) applied to its elements. Returns the new
/// theirs, or `None` when the region is (now) tainted.
pub fn carry_theirs(
    start: usize,
    end: usize,
    base: &str,
    theirs: &[Edit],
    es: &[Edit],
) -> Option<Vec<Edit>> {
    if hunk::crosses(start, es) || hunk::crosses(end, es) {
        return None;
    }
    let mut els = elements(base, theirs);
    // front to back, each edit's window offset shifted by what the earlier ones changed
    let mut shift: isize = 0;
    for e in es {
        let inside = if e.is_insertion() {
            start <= e.start && e.start <= end
        } else {
            start <= e.start && e.end <= end
        };
        if !inside {
            continue;
        }
        let s = (e.start as isize - start as isize + shift) as usize;
        let t = (e.end as isize - start as isize + shift) as usize;
        let (i, j) = (index_at(&els, s), index_at(&els, t));
        els.splice(i..j, e.insert.chars().map(El::Foreign));
        shift += e.growth();
    }
    Some(theirs_of(base, &els))
}

/// The window after dropping its first `n` bytes, which must be foreign (U8: elements an
/// insertion at a shared edge put in both windows belong to the left region). Panics when
/// they aren't: never on a correct carry.
pub fn drop_front(base: &str, theirs: &[Edit], n: usize) -> Vec<Edit> {
    let mut els = elements(base, theirs);
    let k = index_at(&els, n);
    assert!(
        els[..k].iter().all(|e| matches!(e, El::Foreign(_))),
        "a shared edge's elements must be foreign: {base:?} {theirs:?} {n}"
    );
    els.drain(..k);
    theirs_of(base, &els)
}

/// The inverse of `edits` (canonical, applied to `old`): edits on the new text that give
/// `old` back (U17: ours of a redo region).
pub fn inverse(old: &str, edits: &[Edit]) -> Vec<Edit> {
    let mut out = Vec::with_capacity(edits.len());
    let mut shift: isize = 0;
    for e in edits {
        let s = (e.start as isize + shift) as usize;
        out.push(Edit {
            start: s,
            end: s + e.insert.len(),
            insert: old[e.start..e.end].to_owned(),
        });
        shift += e.growth();
    }
    out
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
    fn f3_an_insertion_at_the_start_is_in_front() {
        // p1's region "b" at [0, 1); p2 inserts "b" at 0
        let th = carry_theirs(0, 1, "b", &[], &[e(0, 0, "b")]).unwrap();
        assert_eq!(th, vec![e(0, 0, "b")]);
        // at the end: after the base
        let th = carry_theirs(0, 1, "b", &[], &[e(1, 1, "b")]).unwrap();
        assert_eq!(th, vec![e(1, 1, "b")]);
    }

    #[test]
    fn read_by_key_is_greedy() {
        let p = |k: u32| Prov(1, k);
        // base "ab" (roots 0, 1); window "bXab": the first b counts as base 1, so the a
        // after it is foreign and the last b too (1 isn't greater than 1)
        let (th, provs, counted) = read(
            "ab",
            &[p(0), p(1)],
            "bXab",
            &[p(1), p(9), p(0), p(1)],
            &[p(1), p(9), p(0), p(1)],
        );
        assert_eq!(counted, vec![Some(1), None, None, None]);
        assert_eq!(th, vec![e(0, 1, ""), e(2, 2, "Xab")]);
        assert_eq!(provs, vec![vec![], vec![p(9), p(0), p(1)]]);
        // no restoration: the window's own elements, read by identity
        let (th, _, _) = read(
            "ab",
            &[p(0), p(1)],
            "aYb",
            &[p(0), p(5), p(1)],
            &[p(0), p(5), p(1)],
        );
        assert_eq!(th, vec![e(1, 1, "Y")]);
    }

    #[test]
    fn composes_and_drops() {
        // base "abc" at [2, 5): X at 3, then "Xb" deleted
        let th = carry_theirs(2, 5, "abc", &[], &[e(3, 3, "X")]).unwrap();
        assert_eq!(th, vec![e(1, 1, "X")]);
        let th = carry_theirs(2, 6, "abc", &th, &[e(3, 5, "")]).unwrap();
        assert_eq!(th, vec![e(1, 2, "")]);
        // across an edge: tainted
        assert!(carry_theirs(2, 5, "abc", &[], &[e(1, 3, "")]).is_none());
        // an edit outside, and a replacement starting at the end, are not inside
        let th = carry_theirs(2, 5, "abc", &[], &[e(0, 1, "q"), e(5, 6, "r")]).unwrap();
        assert!(th.is_empty());
        // drop a shared edge's insertion from the front
        assert_eq!(drop_front("ab", &[e(0, 0, "YX")], 1), vec![e(0, 0, "X")]);
        assert_eq!(inverse("abc", &[e(1, 2, "XY")]), vec![e(1, 3, "b")]);
    }
}
