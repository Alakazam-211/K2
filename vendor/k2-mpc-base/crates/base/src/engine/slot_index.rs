//! A MovableList root's **slot index** on Yrs (`structured.rs`): the root's slots in their
//! array order, as an order-statistic tree (a treap with parent links), with each slot's
//! "the element shows here" flag, and the position and delete registers by key.
//!
//! It answers what planning a single insert or delete needs, each in O(log n): where a key's
//! slot sits in the array (`rank`), how many shown elements come before it (`live_rank`), how
//! many come before an array position (`live_before`). The engine keeps one per list root
//! between changes, so such a change reads what it touches, not the whole list: it is built
//! once from a full read, updated by each change the engine makes (a single op in O(log n); a
//! change the full read planned, from that plan's result), and dropped by anything else that
//! changes the doc (a merged update, a reset).

use std::collections::HashMap;
use std::sync::Arc;

use crate::elem::{Key, WriteId};

const NIL: u32 = u32::MAX;

#[derive(Clone)]
struct Node {
    left: u32,
    right: u32,
    parent: u32,
    prio: u32,
    /// Nodes in this subtree.
    size: u32,
    /// Shown elements in this subtree.
    lives: u32,
    live: bool,
}

/// One list root's slots and registers, as the doc holds them after the last change.
#[derive(Clone)]
pub(crate) struct SlotIndex {
    nodes: Vec<Node>,
    /// Each node's slot: (slot id, key).
    slots: Vec<(Arc<str>, Key)>,
    root: u32,
    seed: u32,
    /// Every key the list holds a position register for → the node of its slot (`None`: the
    /// register names no slot of the array).
    pub(crate) cur: HashMap<Key, Option<u32>>,
    /// Deleted keys → the delete's write.
    pub(crate) dead: HashMap<Key, WriteId>,
}

impl SlotIndex {
    pub(crate) fn new() -> Self {
        Self {
            nodes: Vec::new(),
            slots: Vec::new(),
            root: NIL,
            seed: 0x2545_f491,
            cur: HashMap::new(),
            dead: HashMap::new(),
        }
    }

    /// Slots in array order, each with its shown flag, then the registers.
    pub(crate) fn of_slots(
        slots: impl IntoIterator<Item = ((Arc<str>, Key), bool)>,
        cur: HashMap<Key, Option<u32>>,
        dead: HashMap<Key, WriteId>,
    ) -> Self {
        let mut ix = Self::new();
        for (slot, live) in slots {
            ix.push(slot, live);
        }
        ix.cur = cur;
        ix.dead = dead;
        ix
    }

    /// Slots in the array.
    pub(crate) fn len(&self) -> usize {
        self.size(self.root) as usize
    }

    pub(crate) fn is_live(&self, x: u32) -> bool {
        self.nodes[x as usize].live
    }

    fn size(&self, x: u32) -> u32 {
        if x == NIL {
            0
        } else {
            self.nodes[x as usize].size
        }
    }

    fn lives(&self, x: u32) -> u32 {
        if x == NIL {
            0
        } else {
            self.nodes[x as usize].lives
        }
    }

    fn pull(&mut self, x: u32) {
        let (l, r) = (self.nodes[x as usize].left, self.nodes[x as usize].right);
        let size = 1 + self.size(l) + self.size(r);
        let lives = u32::from(self.nodes[x as usize].live) + self.lives(l) + self.lives(r);
        let n = &mut self.nodes[x as usize];
        n.size = size;
        n.lives = lives;
        for c in [l, r] {
            if c != NIL {
                self.nodes[c as usize].parent = x;
            }
        }
    }

    fn merge(&mut self, a: u32, b: u32) -> u32 {
        if a == NIL {
            return b;
        }
        if b == NIL {
            return a;
        }
        if self.nodes[a as usize].prio > self.nodes[b as usize].prio {
            let r = self.nodes[a as usize].right;
            let m = self.merge(r, b);
            self.nodes[a as usize].right = m;
            self.pull(a);
            a
        } else {
            let l = self.nodes[b as usize].left;
            let m = self.merge(a, l);
            self.nodes[b as usize].left = m;
            self.pull(b);
            b
        }
    }

    /// The first `k` nodes of `x`'s subtree, and the rest.
    fn split(&mut self, x: u32, k: u32) -> (u32, u32) {
        if x == NIL {
            return (NIL, NIL);
        }
        let l = self.nodes[x as usize].left;
        if self.size(l) >= k {
            let (a, b) = self.split(l, k);
            self.nodes[x as usize].left = b;
            self.pull(x);
            if a != NIL {
                self.nodes[a as usize].parent = NIL;
            }
            (a, x)
        } else {
            let r = self.nodes[x as usize].right;
            let (a, b) = self.split(r, k - self.size(l) - 1);
            self.nodes[x as usize].right = a;
            self.pull(x);
            if b != NIL {
                self.nodes[b as usize].parent = NIL;
            }
            (x, b)
        }
    }

    fn prio(&mut self) -> u32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed
    }

    fn node(&mut self, slot: (Arc<str>, Key), live: bool) -> u32 {
        let x = self.nodes.len() as u32;
        let prio = self.prio();
        self.nodes.push(Node {
            left: NIL,
            right: NIL,
            parent: NIL,
            prio,
            size: 1,
            lives: u32::from(live),
            live,
        });
        self.slots.push(slot);
        x
    }

    fn push(&mut self, slot: (Arc<str>, Key), live: bool) -> u32 {
        let x = self.node(slot, live);
        let root = self.merge(self.root, x);
        self.set_root(root);
        x
    }

    fn set_root(&mut self, root: u32) {
        self.root = root;
        if root != NIL {
            self.nodes[root as usize].parent = NIL;
        }
    }

    /// A new slot at array position `p`; returns its node.
    pub(crate) fn insert_at(&mut self, p: usize, slot: (Arc<str>, Key), live: bool) -> u32 {
        engine_visits!(1);
        let x = self.node(slot, live);
        let (a, b) = self.split(self.root, p as u32);
        let left = self.merge(a, x);
        let root = self.merge(left, b);
        self.set_root(root);
        x
    }

    /// `x`'s position in the array: the slots before it.
    pub(crate) fn rank(&self, x: u32) -> usize {
        self.rank_by(x, |ix, y| ix.size(y), |_, _| 1)
    }

    /// The shown elements before slot `x` in the array.
    pub(crate) fn live_rank(&self, x: u32) -> usize {
        self.rank_by(x, |ix, y| ix.lives(y), |ix, y| u32::from(ix.is_live(y)))
    }

    fn rank_by(
        &self,
        x: u32,
        sub: impl Fn(&Self, u32) -> u32,
        me: impl Fn(&Self, u32) -> u32,
    ) -> usize {
        let mut n = sub(self, self.nodes[x as usize].left);
        let mut y = x;
        let mut steps = 1u64;
        loop {
            let p = self.nodes[y as usize].parent;
            if p == NIL {
                break;
            }
            if self.nodes[p as usize].right == y {
                n += sub(self, self.nodes[p as usize].left) + me(self, p);
            }
            y = p;
            steps += 1;
        }
        engine_visits!(steps);
        n as usize
    }

    /// The shown elements among the first `k` slots of the array.
    pub(crate) fn live_before(&self, k: usize) -> usize {
        let (mut x, mut k, mut n) = (self.root, k as u32, 0u32);
        let mut steps = 0u64;
        while x != NIL && k > 0 {
            steps += 1;
            let l = self.nodes[x as usize].left;
            let ls = self.size(l);
            if k <= ls {
                x = l;
            } else {
                n += self.lives(l) + u32::from(self.is_live(x));
                k -= ls + 1;
                x = self.nodes[x as usize].right;
            }
        }
        engine_visits!(steps);
        n as usize
    }

    /// Show slot `x`'s element or not.
    pub(crate) fn set_live(&mut self, x: u32, live: bool) {
        engine_visits!(1);
        self.nodes[x as usize].live = live;
        let mut y = x;
        while y != NIL {
            let (l, r) = (self.nodes[y as usize].left, self.nodes[y as usize].right);
            let lives = u32::from(self.nodes[y as usize].live) + self.lives(l) + self.lives(r);
            self.nodes[y as usize].lives = lives;
            y = self.nodes[y as usize].parent;
        }
    }
}
