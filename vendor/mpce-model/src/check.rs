//! The automated Def. 4 checker and the strong-list-spec check (PRD: adjudicates oracle
//! mismatches). Generic over element IDs `K`, whose `Ord` is the ID order.
//!
//! Interpretations, as accepted by K2 Cores for the Quint model (`spec/quint/text.qnt`):
//! - a list state is the element sequence *including tombstones*; the origins are those
//!   recorded at insertion (§V-A defines the right origin over the tombstone-inclusive list);
//! - A and B in Def. 4 range over elements, not `start`/`end`;
//! - "unless Lemma 5 says otherwise" means: unless Lemma 5 (i) and (ii) both hold (Thm 9's
//!   proof); `start` is an ancestor of every element, so (ii) never holds when
//!   `A.leftOrigin = start`;
//! - ID order for (3): `K`'s `Ord`.
//!
//! [Fugue] Def. 4: "(1) If A is the left origin of B, and B appears earlier in the list than
//! any other element that has A as left origin, then A and B are consecutive list elements.
//! (2) If B is the right origin of A, and A appears later in the list than any other element
//! that has B as right origin, then A and B are consecutive list elements, unless Lemma 5
//! below says otherwise. (3) If A and B have the same left origin and the same right origin,
//! then the element with the lower ID appears earlier in the list."
//! Lemma 5: "i. A and B have different left origins; and ii. there is a C in the current
//! list state such that A.leftOrigin ≺ C ≺ B and C is not a descendant of A.leftOrigin in
//! the left-origin tree."

use crate::tree::Origin;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;

/// Left and right origins of each element.
pub type Origins<K> = BTreeMap<K, (Origin<K>, Origin<K>)>;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Def4Violation<K> {
    /// (1): `b` is the earliest element with left origin `a`, but not right after it
    Forward { a: K, b: K },
    /// (2): `a` is the last element with right origin `b`, not right before it, and Lemma 5
    /// grants no exception
    Backward { a: K, b: K },
    /// (3): same origins, `lower` has the lower ID but comes after `higher`
    Tiebreak { lower: K, higher: K },
    /// Lemma 5's conclusion (A ≺ C) fails for this A, B, C
    Lemma5 { a: K, b: K, c: K },
    /// an element of the list has no recorded origins
    UnknownElement { x: K },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ListSpecViolation<K> {
    /// two lists order a pair of elements oppositely (no common total order exists)
    Cycle { x: K, y: K },
    /// (b): an element is not strictly between its origins in some list
    NotBetweenOrigins { list: usize, x: K },
    /// an element appears twice in one list
    Duplicate { list: usize, x: K },
    /// an element's origin is missing from a list that contains it (not causally closed)
    MissingOrigin { list: usize, x: K },
    UnknownElement { x: K },
}

fn pos_of<K: Ord + Copy>(list: &[K]) -> BTreeMap<K, usize> {
    list.iter().enumerate().map(|(i, &x)| (x, i)).collect()
}

/// `c` is a strict descendant of `p` in the left-origin tree (`start` is the root).
fn left_desc<K: Ord + Copy>(origins: &Origins<K>, c: K, p: Origin<K>) -> bool {
    let target = match p {
        Origin::Start => return true,
        Origin::Elem(k) => k,
        Origin::End => return false,
    };
    let mut cur = c;
    for _ in 0..=origins.len() {
        match origins.get(&cur).map(|o| o.0) {
            Some(Origin::Elem(k)) if k == target => return true,
            Some(Origin::Elem(k)) => cur = k,
            _ => return false,
        }
    }
    false
}

/// Def. 4 (1), (2) with the Lemma 5 exception (or without it, `with_exception = false`),
/// and (3), on one list state. Also checks Lemma 5's conclusion.
pub fn def4_with<K: Ord + Copy + Debug>(list: &[K], origins: &Origins<K>, with_exception: bool) -> Result<(), Def4Violation<K>> {
    for &x in list {
        if !origins.contains_key(&x) {
            return Err(Def4Violation::UnknownElement { x });
        }
    }
    let o = |x: K| origins[&x];
    let n = list.len();
    let pos = pos_of(list);
    let lpos = |p: Origin<K>| -> isize {
        match p {
            Origin::Start => -1,
            Origin::Elem(k) => pos.get(&k).map_or(-1, |&i| i as isize),
            Origin::End => n as isize,
        }
    };
    // (1)
    for (ia, &a) in list.iter().enumerate() {
        if let Some(ib) = (0..n).find(|&j| o(list[j]).0 == Origin::Elem(a)) {
            if ib != ia + 1 {
                return Err(Def4Violation::Forward { a, b: list[ib] });
            }
        }
    }
    // (2) and Lemma 5
    for (ib, &b) in list.iter().enumerate() {
        let ia = match (0..n).rev().find(|&j| o(list[j]).1 == Origin::Elem(b)) {
            Some(i) => i,
            None => continue,
        };
        let a = list[ia];
        let p = o(a).0;
        let i_holds = p != o(b).0;
        let cs: Vec<usize> = (0..n).filter(|&ic| lpos(p) < ic as isize && ic < ib && !left_desc(origins, list[ic], p)).collect();
        if i_holds {
            if let Some(&ic) = cs.iter().find(|&&ic| ic <= ia) {
                return Err(Def4Violation::Lemma5 { a, b, c: list[ic] });
            }
        }
        let exception = with_exception && i_holds && !cs.is_empty();
        if ib != ia + 1 && !exception {
            return Err(Def4Violation::Backward { a, b });
        }
    }
    // (3)
    for (ia, &a) in list.iter().enumerate() {
        for (ib, &b) in list.iter().enumerate() {
            if o(a) == o(b) && a < b && ia > ib {
                return Err(Def4Violation::Tiebreak { lower: a, higher: b });
            }
        }
    }
    Ok(())
}

/// Def. 4 exactly (with the Lemma 5 exception).
pub fn def4<K: Ord + Copy + Debug>(list: &[K], origins: &Origins<K>) -> Result<(), Def4Violation<K>> {
    def4_with(list, origins, true)
}

/// The strong list specification over a set of list states (each tombstone-inclusive):
/// (a) there is one total order consistent with every list (the union of the lists' orders
/// is acyclic), and (b) in every list each element lies strictly between its origins. With
/// (a), (b) in each list carries over to any total order extending the union.
pub fn strong_list_spec<K: Ord + Copy + Debug>(lists: &[Vec<K>], origins: &Origins<K>) -> Result<(), ListSpecViolation<K>> {
    let mut succ: BTreeMap<K, BTreeSet<K>> = BTreeMap::new();
    for (li, l) in lists.iter().enumerate() {
        let pos = pos_of(l);
        if pos.len() != l.len() {
            let mut seen = BTreeSet::new();
            let x = *l.iter().find(|x| !seen.insert(**x)).unwrap();
            return Err(ListSpecViolation::Duplicate { list: li, x });
        }
        for (i, &x) in l.iter().enumerate() {
            let (left, right) = *origins.get(&x).ok_or(ListSpecViolation::UnknownElement { x })?;
            let lp = match left {
                Origin::Start => -1,
                Origin::Elem(k) => *pos.get(&k).ok_or(ListSpecViolation::MissingOrigin { list: li, x })? as isize,
                Origin::End => return Err(ListSpecViolation::NotBetweenOrigins { list: li, x }),
            };
            let rp = match right {
                Origin::End => l.len() as isize,
                Origin::Elem(k) => *pos.get(&k).ok_or(ListSpecViolation::MissingOrigin { list: li, x })? as isize,
                Origin::Start => return Err(ListSpecViolation::NotBetweenOrigins { list: li, x }),
            };
            if !(lp < i as isize && (i as isize) < rp) {
                return Err(ListSpecViolation::NotBetweenOrigins { list: li, x });
            }
        }
        for w in l.windows(2) {
            succ.entry(w[0]).or_default().insert(w[1]);
        }
    }
    // acyclicity of the union of "x directly before y" edges (Kahn's algorithm)
    let nodes: BTreeSet<K> = succ.iter().flat_map(|(k, v)| std::iter::once(*k).chain(v.iter().copied())).collect();
    let mut indeg: BTreeMap<K, usize> = nodes.iter().map(|&k| (k, 0)).collect();
    for v in succ.values() {
        for y in v {
            *indeg.get_mut(y).unwrap() += 1;
        }
    }
    let mut ready: Vec<K> = indeg.iter().filter(|(_, &d)| d == 0).map(|(&k, _)| k).collect();
    let mut done = 0;
    while let Some(x) = ready.pop() {
        done += 1;
        if let Some(v) = succ.get(&x) {
            for y in v {
                let d = indeg.get_mut(y).unwrap();
                *d -= 1;
                if *d == 0 {
                    ready.push(*y);
                }
            }
        }
    }
    if done != nodes.len() {
        // report one pair on the cycle: some x with a successor y that is also before x
        for (x, v) in &succ {
            for y in v {
                if indeg[x] > 0 && indeg[y] > 0 {
                    return Err(ListSpecViolation::Cycle { x: *x, y: *y });
                }
            }
        }
        let x = *nodes.iter().next().unwrap();
        return Err(ListSpecViolation::Cycle { x, y: x });
    }
    Ok(())
}
