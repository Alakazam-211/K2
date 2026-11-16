//! The hidden-by rule: whether an object is hidden in the scene drawn, and by what. One rule for
//! the mesh read's `hidden` and the live view's `hidden_by` (`docs/WIRE.md`, "Hidden by"):
//!
//! 1. The object's own viewport flag is set: [`HiddenBy::Itself`], whatever its collections are.
//! 2. Otherwise, when some path from the scene's master collection down through collections to
//!    a collection holding the object passes no hidden collection: shown (`None`). A master
//!    collection is never hidden.
//! 3. Otherwise, when no path reaches the object: `None` (it isn't in the scene drawn).
//! 4. Otherwise: [`HiddenBy::Collection`], the outermost hidden collection (the one nearest the
//!    master collection) on the first path a depth-first walk finds from the master collection,
//!    each collection's children visited in the order the file lists them, a collection holding
//!    the object ending the path, and each collection visited once.
//!
//! Collections are numbered `0..n` by the caller ([`HiddenWalk::new`]); the walk is made once
//! per scene and answers each object from its own flag and the collections holding it
//! ([`HiddenWalk::of`]). The shared test vectors are `tests/data/hidden-by.json`.

use crate::mutant;

/// What hides an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HiddenBy {
    /// Its own viewport flag.
    Itself,
    /// The collection numbered so (the rule's step 4).
    Collection(usize),
}

/// The walk of one scene's collections, from its master collection.
#[derive(Debug, Clone)]
pub struct HiddenWalk {
    /// Each collection's place in the depth-first walk; `None`: not reached.
    order: Vec<Option<u32>>,
    /// Each reached collection's outermost hidden collection on its first path.
    outer: Vec<Option<usize>>,
    /// Each collection reached through no hidden collection, itself shown.
    shown: Vec<bool>,
}

impl HiddenWalk {
    /// The walk of `children.len()` collections from `master`: `children[c]`, collection `c`'s
    /// child collections in the file's order; `hidden(c)`, its own viewport flag (never read for
    /// `master`). A child number out of range is passed over.
    pub fn new(master: usize, children: &[Vec<usize>], hidden: &dyn Fn(usize) -> bool) -> Self {
        let n = children.len();
        let mut walk = HiddenWalk {
            order: vec![None; n],
            outer: vec![None; n],
            shown: vec![false; n],
        };
        if master >= n {
            return walk;
        }
        let is_hidden = |c: usize| c != master && hidden(c);
        // depth first, each collection once: a collection is taken off the stack with the
        // outermost hidden collection of the path that pushed it
        let mut stack: Vec<(usize, Option<usize>)> = vec![(master, None)];
        let mut next = 0u32;
        while let Some((c, above)) = stack.pop() {
            if walk.order[c].is_some() {
                continue;
            }
            walk.order[c] = Some(next);
            next += 1;
            let outer = if mutant::active("HiddenByInnermost") {
                // planted: the innermost hidden collection named
                if is_hidden(c) {
                    Some(c)
                } else {
                    above
                }
            } else {
                above.or(is_hidden(c).then_some(c))
            };
            walk.outer[c] = outer;
            for &k in children[c].iter().rev() {
                if k < n && walk.order[k].is_none() {
                    stack.push((k, outer));
                }
            }
        }
        // shown: reached through shown collections only
        let mut stack = vec![master];
        walk.shown[master] = true;
        while let Some(c) = stack.pop() {
            for &k in &children[c] {
                if k < n && !walk.shown[k] && !is_hidden(k) {
                    walk.shown[k] = true;
                    stack.push(k);
                }
            }
        }
        walk
    }

    /// Whether the walk reaches collection `c`: the scene holds it.
    pub fn reached(&self, c: usize) -> bool {
        self.order.get(c).is_some_and(Option::is_some)
    }

    /// Whether any of `holders` is reached: the scene holds an object they hold.
    pub fn holds(&self, holders: &[usize]) -> bool {
        holders.iter().any(|&c| self.reached(c))
    }

    /// The rule for an object whose own viewport flag is `own`, held by the collections
    /// `holders` (in any order; numbers out of range are passed over): what hides it, or `None`
    /// when it is shown or isn't in the scene.
    pub fn of(&self, own: bool, holders: &[usize]) -> Option<HiddenBy> {
        let by_collection = || {
            if holders
                .iter()
                .any(|&c| self.shown.get(c).copied().unwrap_or(false))
            {
                return None;
            }
            holders
                .iter()
                .filter_map(|&c| Some((self.order.get(c).copied().flatten()?, c)))
                .min()
                .and_then(|(_, c)| self.outer[c])
                .map(HiddenBy::Collection)
        };
        if mutant::active("HiddenByCollectionFirst") {
            // planted: the collections asked before the object's own flag
            if let Some(h) = by_collection() {
                return Some(h);
            }
            return own.then_some(HiddenBy::Itself);
        }
        if own {
            return Some(HiddenBy::Itself);
        }
        by_collection()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_master_flag_is_never_read() {
        let w = HiddenWalk::new(0, &[vec![1], vec![]], &|_| true);
        assert_eq!(w.of(false, &[0]), None);
        assert_eq!(w.of(false, &[1]), Some(HiddenBy::Collection(1)));
        assert!(w.reached(1));
    }

    #[test]
    fn out_of_range_is_passed_over() {
        let w = HiddenWalk::new(0, &[vec![7], vec![]], &|_| false);
        assert_eq!(w.of(false, &[9]), None);
        assert!(!w.holds(&[1, 9]));
        assert_eq!(w.of(true, &[9]), Some(HiddenBy::Itself));
        let none = HiddenWalk::new(3, &[vec![]], &|_| false);
        assert!(!none.reached(0));
    }
}
