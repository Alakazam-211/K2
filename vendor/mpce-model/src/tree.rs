//! The Fugue tree (Weidner & Kleppmann, "The Art of the Fugue", arXiv 2305.00583v3,
//! Algorithm 1) with FugueMax's right-sibling rule (Def. 6). This is the one tree
//! implementation; `text::Doc` and the `mpce-enum` cross-check both use it.
//!
//! Generic over the element ID type `K`, whose `Ord` is the ID order of the paper ("the
//! lexicographic order of their IDs").
//!
//! - Placement (Algorithm 1 lines 25-28): a new element between `leftOrigin` and
//!   `rightOrigin` becomes a right child of `leftOrigin` if that has no right children,
//!   else a left child of `rightOrigin`.
//! - Delivery (lines 30-38): a left child goes to the least index i with
//!   `node.id < leftSibs[i]`. A right child goes, under FugueMax (Def. 6), to the least
//!   index i with `node.rightOrigin ≻ rightSibs[i].rightOrigin`, or equal right origins and
//!   `node.id < rightSibs[i].id`. Under plain Fugue (kept as a mutant) it goes by ID only.
//!   `≻` is the current list order, with `end` after every element.
//! - Order (lines 12-20): depth-first in-order traversal (left children, the node, right
//!   children), tombstones included; the tree never removes a node.

/// A left origin is `Start` or an element; a right origin is an element or `End`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Origin<K> {
    Start,
    Elem(K),
    End,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Parent<K> {
    Root,
    Node(K),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Side {
    Left,
    Right,
}

/// How right-side siblings are ordered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SiblingRule {
    /// Def. 6: reverse order of right origins, then ID
    FugueMax,
    /// Algorithm 1 as written (plain Fugue): by ID only. A mutant for FugueMax checks.
    Fugue,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TreeError {
    /// the parent node is not in the tree
    MissingParent,
    /// a right child's right origin is not in the tree
    MissingRightOrigin,
    /// a left child of `end`, or a right origin of `start`: impossible placements
    BadPlacement,
}

/// Algorithm 1 lines 25-28, given whether `left` already has right children in the
/// creator's tree.
pub fn placement<K: Copy>(left: Origin<K>, right: Origin<K>, left_has_right_child: bool) -> Result<(Parent<K>, Side), TreeError> {
    let lp = match left {
        Origin::Start => Parent::Root,
        Origin::Elem(k) => Parent::Node(k),
        Origin::End => return Err(TreeError::BadPlacement),
    };
    if !left_has_right_child {
        Ok((lp, Side::Right))
    } else {
        match right {
            // Thm 1's proof: if leftOrigin has right children, rightOrigin is one of its
            // descendants, so it is an element
            Origin::Elem(k) => Ok((Parent::Node(k), Side::Left)),
            _ => Err(TreeError::BadPlacement),
        }
    }
}

#[derive(Clone, Debug)]
struct Node<K> {
    id: K,
    right_origin: Origin<K>,
    left: Vec<K>,
    right: Vec<K>,
}

/// A Fugue tree. Deliberately simple: nodes in a Vec, linear lookups.
#[derive(Clone, Debug)]
pub struct Tree<K> {
    rule: SiblingRule,
    root_left: Vec<K>,
    root_right: Vec<K>,
    nodes: Vec<Node<K>>,
}

impl<K: Copy + Ord> Tree<K> {
    pub fn new(rule: SiblingRule) -> Tree<K> {
        Tree { rule, root_left: Vec::new(), root_right: Vec::new(), nodes: Vec::new() }
    }

    pub fn rule(&self) -> SiblingRule {
        self.rule
    }

    pub fn contains(&self, id: K) -> bool {
        self.nodes.iter().any(|n| n.id == id)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn node(&self, id: K) -> Option<&Node<K>> {
        self.nodes.iter().find(|n| n.id == id)
    }

    fn kids(&self, p: Parent<K>, side: Side) -> Option<&Vec<K>> {
        match (p, side) {
            (Parent::Root, Side::Left) => Some(&self.root_left),
            (Parent::Root, Side::Right) => Some(&self.root_right),
            (Parent::Node(k), s) => self.node(k).map(|n| if s == Side::Left { &n.left } else { &n.right }),
        }
    }

    fn kids_mut(&mut self, p: Parent<K>, side: Side) -> Option<&mut Vec<K>> {
        match (p, side) {
            (Parent::Root, Side::Left) => Some(&mut self.root_left),
            (Parent::Root, Side::Right) => Some(&mut self.root_right),
            (Parent::Node(k), s) => self.nodes.iter_mut().find(|n| n.id == k).map(|n| if s == Side::Left { &mut n.left } else { &mut n.right }),
        }
    }

    /// Whether a node (or the root) has right children.
    pub fn has_right_children(&self, p: Parent<K>) -> bool {
        self.kids(p, Side::Right).is_some_and(|v| !v.is_empty())
    }

    /// Algorithm 1 lines 25-28 on this tree.
    pub fn placement(&self, left: Origin<K>, right: Origin<K>) -> Result<(Parent<K>, Side), TreeError> {
        let lp = match left {
            Origin::Start => Parent::Root,
            Origin::Elem(k) => Parent::Node(k),
            Origin::End => return Err(TreeError::BadPlacement),
        };
        placement(left, right, self.has_right_children(lp))
    }

    /// The list order, tombstones included (Algorithm 1 lines 12-20).
    pub fn order(&self) -> Vec<K> {
        let mut out = Vec::with_capacity(self.nodes.len());
        for &c in &self.root_left {
            self.walk(c, &mut out);
        }
        for &c in &self.root_right {
            self.walk(c, &mut out);
        }
        out
    }

    fn walk(&self, id: K, out: &mut Vec<K>) {
        let n = match self.node(id) {
            Some(n) => n,
            None => return,
        };
        for &c in &n.left {
            self.walk(c, out);
        }
        out.push(id);
        for &c in &n.right {
            self.walk(c, out);
        }
    }

    /// Algorithm 1 line 24: the node after `left` in the traversal including tombstones,
    /// or `End`.
    pub fn next_after(&self, left: Origin<K>) -> Origin<K> {
        let order = self.order();
        let next = match left {
            Origin::Start => order.first().copied(),
            Origin::Elem(k) => order.iter().position(|&x| x == k).and_then(|i| order.get(i + 1).copied()),
            Origin::End => None,
        };
        next.map_or(Origin::End, Origin::Elem)
    }

    /// Algorithm 1 lines 30-38 (with Def. 6 for right children). Returns Ok(false) if the
    /// node is already in the tree (idempotent delivery).
    pub fn integrate(&mut self, id: K, parent: Parent<K>, side: Side, right_origin: Origin<K>) -> Result<bool, TreeError> {
        if self.contains(id) {
            return Ok(false);
        }
        if let Parent::Node(p) = parent {
            if !self.contains(p) {
                return Err(TreeError::MissingParent);
            }
        }
        if right_origin == Origin::Start {
            return Err(TreeError::BadPlacement);
        }
        let i = match side {
            Side::Left => {
                let sibs = self.kids(parent, Side::Left).ok_or(TreeError::MissingParent)?;
                sibs.iter().position(|&s| id < s).unwrap_or(sibs.len())
            }
            Side::Right => {
                let sibs = self.kids(parent, Side::Right).ok_or(TreeError::MissingParent)?;
                match self.rule {
                    SiblingRule::Fugue => sibs.iter().position(|&s| id < s).unwrap_or(sibs.len()),
                    SiblingRule::FugueMax => {
                        let order = self.order();
                        let pos = |o: Origin<K>| -> Option<usize> {
                            match o {
                                Origin::End => Some(usize::MAX),
                                Origin::Elem(k) => order.iter().position(|&x| x == k),
                                Origin::Start => None,
                            }
                        };
                        let ro = pos(right_origin).ok_or(TreeError::MissingRightOrigin)?;
                        let mut at = sibs.len();
                        for (j, &s) in sibs.iter().enumerate() {
                            let so = self.node(s).map(|n| n.right_origin).ok_or(TreeError::MissingParent)?;
                            let rs = pos(so).ok_or(TreeError::MissingRightOrigin)?;
                            if ro > rs || (ro == rs && id < s) {
                                at = j;
                                break;
                            }
                        }
                        at
                    }
                }
            }
        };
        self.kids_mut(parent, side).ok_or(TreeError::MissingParent)?.insert(i, id);
        self.nodes.push(Node { id, right_origin, left: Vec::new(), right: Vec::new() });
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paper_fig3_shape() {
        // two concurrent left children of the same node are ordered by ID
        let mut t: Tree<u32> = Tree::new(SiblingRule::FugueMax);
        t.integrate(10, Parent::Root, Side::Right, Origin::End).unwrap();
        t.integrate(2, Parent::Node(10), Side::Left, Origin::Elem(10)).unwrap();
        t.integrate(1, Parent::Node(10), Side::Left, Origin::Elem(10)).unwrap();
        assert_eq!(t.order(), vec![1, 2, 10]);
        assert_eq!(t.integrate(1, Parent::Node(10), Side::Left, Origin::Elem(10)), Ok(false));
    }
}
