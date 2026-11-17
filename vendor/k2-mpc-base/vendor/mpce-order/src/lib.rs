//! The order layer of the reference models: a FugueMax list of opaque slot IDs.
//!
//! Slot order and per-element registers are separate crates with no
//! dependency between them. This crate is the slot order. It never sees an element, a value,
//! a delete or a register write: a slot is inserted with its placement, and the visible order
//! is the slot order filtered by a `shown` predicate the caller passes in. Which slots are
//! shown is the composition's business (`mpce-model`'s `list`, `mlist`), decided from the
//! register layer (`mpce-reg`).
//!
//! - [`tree`]: the Fugue tree (Algorithm 1) with FugueMax's right-sibling rule (Def. 6). It
//!   is the one tree implementation: `mpce-model` re-exports it as `mpce_model::tree` (text,
//!   the tree model and `mpce-enum` use it there).
//! - [`SlotOrder`]: slots in that tree. Generation ([`SlotOrder::place_after`]) reads the
//!   creator's order to pick a placement, which travels with the op; integration
//!   ([`SlotOrder::insert`]) applies a placement. Neither reads anything but slots.
#![forbid(unsafe_code)]

pub mod collapsed;
pub mod tree;

use tree::{Origin, Parent, Side, SiblingRule, Tree, TreeError};

/// Where a new slot goes: what Algorithm 1 broadcasts (parent, side and, for FugueMax, the
/// right origin) plus the left origin (Def. 4 needs it, and so do the formal models, which
/// integrate by origins).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Placement<K> {
    pub left: Origin<K>,
    pub right: Origin<K>,
    pub parent: Parent<K>,
    pub side: Side,
}

/// The slot order: every slot ever inserted (a slot is never removed), in FugueMax order.
#[derive(Clone, Debug)]
pub struct SlotOrder<K> {
    tree: Tree<K>,
}

impl<K: Copy + Ord> Default for SlotOrder<K> {
    fn default() -> Self {
        SlotOrder::new()
    }
}

impl<K: Copy + Ord> SlotOrder<K> {
    /// A FugueMax slot order.
    pub fn new() -> SlotOrder<K> {
        SlotOrder::with_rule(SiblingRule::FugueMax)
    }

    /// A slot order with an explicit right-sibling rule (`SiblingRule::Fugue` is a mutant).
    pub fn with_rule(rule: SiblingRule) -> SlotOrder<K> {
        SlotOrder { tree: Tree::new(rule) }
    }

    pub fn rule(&self) -> SiblingRule {
        self.tree.rule()
    }

    pub fn contains(&self, slot: K) -> bool {
        self.tree.contains(slot)
    }

    pub fn len(&self) -> usize {
        self.tree.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    /// Generation: the placement of a new slot just after `left` (`Origin::Start`, or a slot
    /// of this order). The right origin is the next slot after `left`, tombstones included
    /// (Algorithm 1 line 24), and parent and side follow lines 25-28 on this order's tree.
    pub fn place_after(&self, left: Origin<K>) -> Result<Placement<K>, TreeError> {
        if let Origin::Elem(k) = left {
            if !self.tree.contains(k) {
                return Err(TreeError::MissingParent);
            }
        }
        let right = self.tree.next_after(left);
        let (parent, side) = self.tree.placement(left, right)?;
        Ok(Placement { left, right, parent, side })
    }

    /// Integration: insert `slot` at placement `p`. Ok(false) if the slot is already here
    /// (idempotent).
    pub fn insert(&mut self, slot: K, p: &Placement<K>) -> Result<bool, TreeError> {
        self.tree.integrate(slot, p.parent, p.side, p.right)
    }

    /// Every slot in order, tombstones included.
    pub fn slots(&self) -> Vec<K> {
        self.tree.order()
    }

    /// The visible order: the slots for which `shown` holds, in slot order.
    pub fn visible(&self, mut shown: impl FnMut(K) -> bool) -> Vec<K> {
        self.tree.order().into_iter().filter(|&s| shown(s)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_after_and_filter() {
        let mut o: SlotOrder<u32> = SlotOrder::new();
        let p = o.place_after(Origin::Start).unwrap();
        assert!(o.insert(10, &p).unwrap());
        let p = o.place_after(Origin::Elem(10)).unwrap();
        assert_eq!(p.right, Origin::End);
        assert!(o.insert(20, &p).unwrap());
        // between 10 and 20: right origin is 20
        let p = o.place_after(Origin::Elem(10)).unwrap();
        assert_eq!(p.right, Origin::Elem(20));
        assert!(o.insert(15, &p).unwrap());
        assert_eq!(o.slots(), vec![10, 15, 20]);
        assert!(!o.insert(15, &p).unwrap());
        assert_eq!(o.visible(|s| s != 15), vec![10, 20]);
        assert_eq!(o.place_after(Origin::Elem(99)), Err(TreeError::MissingParent));
    }

    #[test]
    fn concurrent_same_spot_by_id() {
        // two replicas insert after 10 concurrently; both orders converge
        let mut a: SlotOrder<u32> = SlotOrder::new();
        a.insert(10, &a.place_after(Origin::Start).unwrap()).unwrap();
        let mut b = a.clone();
        let pa = a.place_after(Origin::Elem(10)).unwrap();
        let pb = b.place_after(Origin::Elem(10)).unwrap();
        a.insert(30, &pa).unwrap();
        b.insert(20, &pb).unwrap();
        a.insert(20, &pb).unwrap();
        b.insert(30, &pa).unwrap();
        assert_eq!(a.slots(), b.slots());
        assert_eq!(a.slots(), vec![10, 20, 30]);
    }
}
