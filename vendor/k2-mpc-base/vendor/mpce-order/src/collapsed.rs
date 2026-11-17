//! A reference model of collapsed runs: a FugueMax list whose elements
//! below a causally stable set C are **collapsed** to their list position and two bits (the
//! element has a left child, a right child, in the tree below C), with the tree kept only for
//! the elements above C (resident).
//!
//! - **Integration** of a node (parent, side, right origin): its siblings are the parent's
//!   resident children on that side (none below C can be one: [`super::tree::placement`]
//!   makes a new node the right child of its left origin only if that has no right child in
//!   its creator's tree, else the left child of the next node, which then has no left child,
//!   and a creator's tree contains C); it goes after the rightmost descendant of the sibling
//!   before it (resident: a node above C has no descendant in C), or right after its parent,
//!   or before the leftmost descendant of the next left sibling, or right before its parent.
//!   Def. 6 compares right origins by list position.
//! - **Generation** (Algorithm 1 line 25): the left origin has a right child iff it has a
//!   resident right child, or it is collapsed with its right bit set; the root has one iff the
//!   list is not empty.
//! - **Admission** ([`Collapsed::admit`]): the base first (a base lacking part of C is
//!   [`Refuse::StaleBase`]: `MPCE-E182` for a client, re-based; `MPCE-E264` for a relay, resynced), then
//!   the exact rule C and the defence-in-depth rule A against forged placements, each forged
//!   shape with its own code (`MPCE-E265` right, `MPCE-E266` left).
//! - **Collapse** of the resident elements in C: each takes its bits (a child **in C** on that
//!   side), leaves its siblings' list and keeps its resident children in its own; a child
//!   collapsing sets its (collapsed) parent's bit on its side. Both bits are monotone.
//! - **A cut**: the list with, per element, either the bits or (parent, side, right origin);
//!   loading rebuilds the children lists from list order (siblings are in list order).
//!
//! It is the oracle the engine's collapsed runs are checked against, and it is checked here,
//! against [`super::tree::Tree`] (the full FugueMax tree), by `pt_collapsed_equals_the_full_tree`.

use std::collections::BTreeMap;

use crate::tree::{placement, Origin, Parent, Side, TreeError};

/// An element: collapsed (with its two bits) or resident (with its placement).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elem<K> {
    /// Below C. `left_child`: it has a left child in the tree below C (bit 0);
    /// `right_child`: it has a right child there (bit 1, `0x80`).
    Collapsed { left_child: bool, right_child: bool },
    Resident { parent: Parent<K>, side: Side, right_origin: Origin<K> },
}

/// The check of the forged-placement admission that refused an op.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Check {
    /// Rule C (and A): a right child of a collapsed element whose right bit is set (`E265`).
    RightBit,
    /// Rule C: a right child of the root while anything is collapsed (`E265`).
    RootRight,
    /// Rule C: a left child of a collapsed element whose left origin is not the nearest
    /// collapsed element before it (or the start, with none) (`E266`).
    LeftOrigin,
    /// Rule C: a left child of a collapsed element whose nearest collapsed predecessor has no
    /// right child below C (`E266`).
    LeftOriginBit,
    /// Rule A, defence in depth: a left child of a collapsed element whose left bit is set
    /// (`E266`). Logically redundant given rule C: it never fires on what C admits.
    LeftBit,
    /// A left child of the end (malformed).
    LeftOfEnd,
}

impl Check {
    /// The refusal code the engine gives it.
    pub fn code(self) -> &'static str {
        match self {
            Check::RightBit | Check::RootRight => "MPCE-E265",
            Check::LeftOrigin | Check::LeftOriginBit | Check::LeftBit => "MPCE-E266",
            Check::LeftOfEnd => "malformed",
        }
    }
}

/// Why [`Collapsed::admit`] refused an op.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refuse {
    /// Its base lacks part of C (`E182` for a client, `E264` for a relay): never "forged".
    StaleBase,
    /// A forged placement.
    Forged(Check),
}

/// The collapsed list.
#[derive(Clone, Debug)]
pub struct Collapsed<K> {
    list: Vec<K>,
    elems: BTreeMap<K, Elem<K>>,
    /// Resident children per (parent, side), in sibling order (root as `None`).
    kids: BTreeMap<(Option<K>, bool), Vec<K>>,
    /// Planted (the "bit ignored" bug): generation reads no collapsed element's right bit.
    pub ignore_bit: bool,
    /// Planted (the forged-placement gap): [`Collapsed::admit`] admits every placement its base allows.
    pub no_admission: bool,
    /// Planted: checks of the admission removed.
    pub skip: Vec<Check>,
    /// Planted: the forged-placement checks run before the base check (an honest client on a
    /// stale base could then be refused as forged).
    pub forged_first: bool,
}

fn key<K: Copy>(p: Parent<K>, side: Side) -> (Option<K>, bool) {
    (
        match p {
            Parent::Root => None,
            Parent::Node(k) => Some(k),
        },
        side == Side::Right,
    )
}

impl<K: Copy + Ord + std::fmt::Debug> Collapsed<K> {
    /// The empty list.
    pub fn new() -> Self {
        Collapsed { list: Vec::new(), elems: BTreeMap::new(), kids: BTreeMap::new(), ignore_bit: false, no_admission: false, skip: Vec::new(), forged_first: false }
    }

    /// The list order, tombstones included.
    pub fn order(&self) -> &[K] {
        &self.list
    }

    pub fn contains(&self, k: K) -> bool {
        self.elems.contains_key(&k)
    }

    /// An element's state.
    pub fn elem(&self, k: K) -> Option<Elem<K>> {
        self.elems.get(&k).copied()
    }

    fn pos(&self, k: K) -> Option<usize> {
        self.list.iter().position(|x| *x == k)
    }

    /// Whether `p` has a right child (Algorithm 1 line 25).
    pub fn has_right_children(&self, p: Parent<K>) -> bool {
        if self.kids.get(&key(p, Side::Right)).is_some_and(|v| !v.is_empty()) {
            return true;
        }
        match p {
            Parent::Root => !self.list.is_empty(),
            Parent::Node(k) => self.right_bit(k) && !self.ignore_bit,
        }
    }

    /// A local insert between `left` and the next element: its placement (parent, side) and
    /// right origin, as Algorithm 1 lines 24-28.
    pub fn generate(&self, left: Origin<K>) -> Result<(Parent<K>, Side, Origin<K>), TreeError> {
        let right = match left {
            Origin::Start => self.list.first().copied().map_or(Origin::End, Origin::Elem),
            Origin::Elem(k) => self.pos(k).and_then(|i| self.list.get(i + 1)).copied().map_or(Origin::End, Origin::Elem),
            Origin::End => return Err(TreeError::BadPlacement),
        };
        let lp = match left {
            Origin::Elem(k) => Parent::Node(k),
            _ => Parent::Root,
        };
        let (parent, side) = placement(left, right, self.has_right_children(lp))?;
        Ok((parent, side, right))
    }

    /// Whether `k` is collapsed.
    pub fn is_collapsed(&self, k: K) -> bool {
        matches!(self.elems.get(&k), Some(Elem::Collapsed { .. }))
    }

    /// Whether anything is collapsed.
    pub fn any_collapsed(&self) -> bool {
        self.elems.values().any(|e| matches!(e, Elem::Collapsed { .. }))
    }

    /// Collapsed `k`'s right bit: it has a right child below C.
    fn right_bit(&self, k: K) -> bool {
        matches!(self.elems.get(&k), Some(Elem::Collapsed { right_child: true, .. }))
    }

    /// Collapsed `k`'s left bit: it has a left child below C.
    fn left_bit(&self, k: K) -> bool {
        matches!(self.elems.get(&k), Some(Elem::Collapsed { left_child: true, .. }))
    }

    fn on(&self, c: Check) -> bool {
        !self.skip.contains(&c)
    }

    /// Admission of an op (left origin `left`, side, right origin `right`; the parent is `left`
    /// for a right child, `right` for a left child) whose creator's base holds C or not
    /// (`base_holds_c`). **The base first**: an honest client on a stale base is re-based
    /// (`MPCE-E182`) or resynced (`MPCE-E264`), never refused as forged. Then the admission against
    /// forged placements, in this order:
    ///
    /// - **Rule C, exact** (with the right bit alone; a placement it refuses is impossible for a
    ///   creator whose state holds C, and one it admits is integrated exactly as the full tree
    ///   would):
    ///   - **a right child of a collapsed `L`** (or of the root while anything is collapsed): `L`
    ///     must have no right child below C (its right bit clear; the root: nothing collapsed),
    ///     since an honest creator makes a right child only of a left origin with none. Its
    ///     siblings are then all resident, the same on every replica (`E265`);
    ///   - **a left child of a collapsed `R`**: the left origin must be the nearest collapsed
    ///     element before `R` in the list (or the start, with none before), and have a right
    ///     child below C (the root: `R` itself is collapsed, so it has). Then `R` is the
    ///     leftmost element below C of that right subtree, so `R` has no left child below C,
    ///     and its siblings are all resident. (A resident left origin can't precede a
    ///     collapsed `R` this way: `R` would be in its subtree.) (`E266`)
    ///   - a parent that is resident: its children are all resident; nothing below C is
    ///     involved.
    /// - **Rule A, defence in depth**: a right child of a collapsed element with its right bit
    ///   set (rule C's first case), and a left child of a collapsed element with its left bit
    ///   set (`E266`). The left-bit check is **logically redundant given rule C** (above: what
    ///   C admits has no left child below C); the PT counts it never firing on what C admits.
    ///   (Conversely, for convergence alone the left bit is enough for left children: a left
    ///   child of collapsed `R` is placed differently below C only when `R` has left siblings
    ///   there, i.e. a left child below C. Rule C's left clauses refuse more: everything no
    ///   honest creator makes. The planted PT shows each side alone keeps convergence and
    ///   removing both diverges; both stay.)
    pub fn admit(&self, base_holds_c: bool, left: Origin<K>, side: Side, right: Origin<K>) -> Result<(), Refuse> {
        if self.forged_first {
            self.forged(left, side, right).map_err(Refuse::Forged)?;
        }
        if !base_holds_c {
            return Err(Refuse::StaleBase);
        }
        self.forged(left, side, right).map_err(Refuse::Forged)
    }

    /// The forged-placement checks alone (rule C, then rule A).
    pub fn forged(&self, left: Origin<K>, side: Side, right: Origin<K>) -> Result<(), Check> {
        if self.no_admission {
            return Ok(());
        }
        match side {
            Side::Right => match left {
                Origin::Start if self.any_collapsed() && self.on(Check::RootRight) => Err(Check::RootRight),
                Origin::Elem(l) if self.is_collapsed(l) && self.right_bit(l) && self.on(Check::RightBit) => Err(Check::RightBit),
                _ => Ok(()),
            },
            Side::Left => match right {
                Origin::End => Err(Check::LeftOfEnd),
                Origin::Elem(r) if self.is_collapsed(r) => {
                    let Some(i) = self.pos(r) else { return Err(Check::LeftOrigin) };
                    let prev = self.list[..i].iter().rev().find(|k| self.is_collapsed(**k)).copied();
                    // rule C: the left origin is the nearest collapsed element before R ...
                    let origin_ok = match (left, prev) {
                        (Origin::Start, None) => true,
                        (Origin::Elem(l), Some(p)) => l == p,
                        _ => false,
                    };
                    if !origin_ok && self.on(Check::LeftOrigin) {
                        return Err(Check::LeftOrigin);
                    }
                    // ... with a right child below C (the root has one: R is collapsed)
                    let bit_ok = match left {
                        Origin::Elem(l) => self.right_bit(l),
                        _ => true,
                    };
                    if !bit_ok && self.on(Check::LeftOriginBit) {
                        return Err(Check::LeftOriginBit);
                    }
                    // rule A (defence in depth, redundant given C): R has no left child below C
                    if self.left_bit(r) && self.on(Check::LeftBit) {
                        return Err(Check::LeftBit);
                    }
                    Ok(())
                }
                _ => Ok(()),
            },
        }
    }

    fn ro_pos(&self, o: Origin<K>) -> Result<usize, TreeError> {
        match o {
            Origin::End => Ok(usize::MAX),
            Origin::Elem(k) => self.pos(k).ok_or(TreeError::MissingRightOrigin),
            Origin::Start => Err(TreeError::BadPlacement),
        }
    }

    fn resident_ro(&self, k: K) -> Result<Origin<K>, TreeError> {
        match self.elems.get(&k) {
            Some(Elem::Resident { right_origin, .. }) => Ok(*right_origin),
            _ => Err(TreeError::MissingParent),
        }
    }

    fn leftmost(&self, mut x: K) -> K {
        while let Some(&c) = self.kids.get(&(Some(x), false)).and_then(|v| v.first()) {
            x = c;
        }
        x
    }

    fn rightmost(&self, mut x: K) -> K {
        while let Some(&c) = self.kids.get(&(Some(x), true)).and_then(|v| v.last()) {
            x = c;
        }
        x
    }

    /// Integrate node `id` (Algorithm 1 lines 30-38 with Def. 6, by list position below C).
    /// `Ok(false)` if it is already here.
    pub fn integrate(&mut self, id: K, parent: Parent<K>, side: Side, right_origin: Origin<K>) -> Result<bool, TreeError> {
        if self.contains(id) {
            return Ok(false);
        }
        if let Parent::Node(p) = parent {
            if !self.contains(p) {
                return Err(TreeError::MissingParent);
            }
        }
        let sibs = self.kids.get(&key(parent, side)).cloned().unwrap_or_default();
        let (i, at) = match side {
            Side::Left => {
                let p = match parent {
                    Parent::Node(p) => p,
                    Parent::Root => return Err(TreeError::BadPlacement),
                };
                let i = sibs.iter().position(|&s| id < s).unwrap_or(sibs.len());
                let before = match sibs.get(i) {
                    Some(&s) => self.leftmost(s),
                    None => p,
                };
                (i, self.pos(before).ok_or(TreeError::MissingParent)?)
            }
            Side::Right => {
                let ro = self.ro_pos(right_origin)?;
                let mut i = sibs.len();
                for (j, &s) in sibs.iter().enumerate() {
                    let rs = self.ro_pos(self.resident_ro(s)?)?;
                    if ro > rs || (ro == rs && id < s) {
                        i = j;
                        break;
                    }
                }
                let after = match i.checked_sub(1).and_then(|j| sibs.get(j)) {
                    Some(&prev) => Some(self.rightmost(prev)),
                    None => match parent {
                        Parent::Node(p) => Some(p),
                        Parent::Root => None,
                    },
                };
                let at = match after {
                    Some(a) => self.pos(a).ok_or(TreeError::MissingParent)? + 1,
                    None => 0,
                };
                (i, at)
            }
        };
        self.list.insert(at, id);
        self.elems.insert(id, Elem::Resident { parent, side, right_origin });
        self.kids.entry(key(parent, side)).or_default().insert(i, id);
        Ok(true)
    }

    /// Collapse every resident element `in_c` says is in C (C is causally stable and
    /// down-closed: the caller's claim).
    pub fn collapse(&mut self, in_c: impl Fn(K) -> bool) {
        let ks: Vec<K> = self.elems.iter().filter(|(k, e)| matches!(e, Elem::Resident { .. }) && in_c(**k)).map(|(k, _)| *k).collect();
        // every child of a resident element is resident (a child has its parent in its past,
        // and C is down-closed), so its children are all listed under it; each bit says
        // whether one of them on that side is **in C** (collapsing in this batch): the tree
        // below C only, a resident child never sets it (the adversarial PT found an honest right
        // child of an element whose concurrent resident child had set it). Taken before any of
        // the batch leaves its list
        let has = |kids: &BTreeMap<(Option<K>, bool), Vec<K>>, k: K, right: bool| kids.get(&(Some(k), right)).is_some_and(|v| v.iter().any(|c| in_c(*c)));
        let bits: Vec<(K, bool, bool)> = ks.iter().map(|k| (*k, has(&self.kids, *k, false), has(&self.kids, *k, true))).collect();
        let mut parents: Vec<(K, Side)> = Vec::new();
        for (k, left_child, right_child) in bits {
            let Some(Elem::Resident { parent, side, .. }) = self.elems.get(&k).copied() else { continue };
            if let Some(v) = self.kids.get_mut(&key(parent, side)) {
                v.retain(|x| *x != k);
            }
            if let Parent::Node(p) = parent {
                parents.push((p, side));
            }
            self.elems.insert(k, Elem::Collapsed { left_child, right_child });
        }
        // a child that collapses now joins the tree below C: its parent (collapsed now or
        // before, as C is down-closed) has a child on that side there. The bits describe the
        // tree below the current C: a resident child never sets one, its collapse does
        for (p, side) in parents {
            if let Some(Elem::Collapsed { left_child, right_child }) = self.elems.get_mut(&p) {
                match side {
                    Side::Left => *left_child = true,
                    Side::Right => *right_child = true,
                }
            }
        }
    }

    /// The cut: per element in list order, its state.
    pub fn cut(&self) -> Vec<(K, Elem<K>)> {
        self.list.iter().filter_map(|k| self.elems.get(k).map(|e| (*k, *e))).collect()
    }

    /// Load a cut: the children lists from list order (siblings are in list order).
    pub fn load(cut: &[(K, Elem<K>)]) -> Self {
        let mut c = Collapsed::new();
        for (k, e) in cut {
            c.list.push(*k);
            c.elems.insert(*k, *e);
            if let Elem::Resident { parent, side, .. } = e {
                c.kids.entry(key(*parent, *side)).or_default().push(*k);
            }
        }
        c
    }

    /// Carry the planted switches over to a reloaded list.
    fn planted_like(mut self, o: &Self) -> Self {
        self.ignore_bit = o.ignore_bit;
        self.no_admission = o.no_admission;
        self.skip = o.skip.clone();
        self.forged_first = o.forged_first;
        self
    }

    /// Cut and load again (the planted switches kept).
    pub fn reload(&self) -> Self {
        Collapsed::load(&self.cut()).planted_like(self)
    }
}

impl<K: Copy + Ord + std::fmt::Debug> Default for Collapsed<K> {
    fn default() -> Self {
        Self::new()
    }
}

/// The equivalence PT, with a standing adversarial check: three peers with concurrent local inserts and causal
/// deliveries, plus an offline client D of A's and an adversarial creator. Participant A keeps a
/// [`Collapsed`] list (collapsing at random causally stable sets C, and cut and reloaded at
/// random), and a full [`crate::tree::Tree`] twin fed the same events; B and C keep full trees
/// only and never compact. A's local inserts are typed mostly inside collapsed text. After
/// every step: A's order equals its twin's; every local insert A makes declares exactly the
/// placement its twin's full tree would (Algorithm 1 line 25 on the full tree); replicas holding
/// the same events have the same order; both bits are monotone. Honest events are never refused
/// as forged; D's events on a base lacking part of C are refused as a stale base (then D
/// resyncs from A), never as forged. Forgeries (the two forged shapes and random ones) are
/// refused, or admitted and then placed exactly as the full tree places them. Planted: the
/// removed checks, the bit ignored, a C that isn't causally stable, a cleared bit (either), the
/// forged checks before the base check: each must be caught.
#[cfg(test)]
mod pt {
    use std::collections::BTreeSet;

    use super::*;
    use crate::tree::{SiblingRule, Tree};

    type K = (u8, u32);

    /// Agents: A, B, C are 1, 2, 3; D (A's offline client) is 4; forgers are 0 and 9 (below and
    /// above every honest agent, so a forged left sibling sorts on either side of honest ones).
    const D: usize = 3;

    struct Rng(u64);
    impl Rng {
        fn below(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n.max(1)
        }
    }

    #[derive(Clone)]
    struct Ev {
        id: K,
        left: Origin<K>,
        parent: Parent<K>,
        side: Side,
        ro: Origin<K>,
        past: BTreeSet<K>,
    }

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Plant {
        None,
        IgnoreBit,
        Eager,
        /// the forged-placement gap: forged placements admitted (no admission at all)
        NoAdmission,
        /// checks removed
        Skip(&'static [Check]),
        /// the forged checks before the base check: an honest stale client refused as forged
        ForgedFirst,
        /// a set right bit cleared at a collapse (the bits are monotone)
        ClearBit,
        /// a set left bit cleared at a collapse (monotone too)
        ClearLeftBit,
        /// a set bit cleared after the history's last placement (nothing reads it again): only
        /// the monotonicity assertion can catch it
        ClearShadowedBit,
    }

    #[derive(Default, Debug)]
    struct Stats {
        local_in_collapsed: u64,
        local_mid_collapsed: u64,
        collapses: u64,
        loads: u64,
        compared_with_b: u64,
        forged: u64,
        forged_refused: u64,
        forged_admitted: u64,
        refused_by: BTreeMap<Check, u64>,
        /// rule A's left-bit check refusing what rule C admitted (must stay 0: redundant)
        a_only: u64,
        /// rule C's left clauses refusing a left child whose parent has no left child below C
        /// (which rule A alone would admit, and place as the full tree does)
        c_only_left: u64,
        left_bits_set: u64,
        d_sent: u64,
        d_admitted: u64,
        d_stale: u64,
        /// D's stale events the forged checks would have refused (why the base goes first)
        d_stale_would_be_forged: u64,
        d_resyncs: u64,
    }

    fn run(seed: u64, steps: usize, plant: Plant, st: &mut Stats) -> Result<(), String> {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut a: Collapsed<K> = Collapsed::new();
        a.ignore_bit = plant == Plant::IgnoreBit;
        a.no_admission = plant == Plant::NoAdmission;
        a.skip = match plant {
            Plant::Skip(c) => c.to_vec(),
            _ => Vec::new(),
        };
        a.forged_first = plant == Plant::ForgedFirst;
        let mut forger_seq = [0u32; 2];
        let mut twin: Tree<K> = Tree::new(SiblingRule::FugueMax);
        // B, C and D: full trees
        let mut full: Vec<Tree<K>> = vec![Tree::new(SiblingRule::FugueMax); 3];
        let mut known: Vec<BTreeSet<K>> = vec![BTreeSet::new(); 4];
        let mut seq = [0u32; 4];
        let mut evs: Vec<Ev> = Vec::new();
        // D's unsent events, in order
        let mut outbox: Vec<Ev> = Vec::new();
        let mut c_set: BTreeSet<K> = BTreeSet::new();
        let mut bits: BTreeMap<K, (bool, bool)> = BTreeMap::new();
        for step in 0..steps {
            let ctx = |m: String| format!("seed {seed} step {step}: {m}");
            let roll = rng.below(100);
            if roll < 8 && !a.order().is_empty() {
                // an adversarial creator whose state is A's: a forged placement, one of the two
                // forged shapes or random. A refuses it (then nobody gets it: A's host is the only
                // way in) or admits it, and then places it exactly as its full twin.
                let order = a.order().to_vec();
                let pick = |rng: &mut Rng| order[rng.below(order.len() as u64) as usize];
                let (left, side, right) = match rng.below(3) {
                    0 => {
                        // shape 1: a right child of a (collapsed) element, maybe with a right child
                        let l = pick(&mut rng);
                        (Origin::Elem(l), Side::Right, a.generate(Origin::Elem(l)).map(|g| g.2).unwrap_or(Origin::End))
                    }
                    1 => {
                        // shape 2: a left child of a (collapsed) element, any left origin
                        let r = pick(&mut rng);
                        let l = if rng.below(4) == 0 { Origin::Start } else { Origin::Elem(pick(&mut rng)) };
                        (l, Side::Left, Origin::Elem(r))
                    }
                    _ => {
                        let l = if rng.below(5) == 0 { Origin::Start } else { Origin::Elem(pick(&mut rng)) };
                        let r = if rng.below(5) == 0 { Origin::End } else { Origin::Elem(pick(&mut rng)) };
                        (l, if rng.below(2) == 0 { Side::Left } else { Side::Right }, r)
                    }
                };
                if side == Side::Left && right == Origin::End {
                    continue;
                }
                let parent = match (side, left, right) {
                    (Side::Right, Origin::Elem(l), _) => Parent::Node(l),
                    (Side::Right, _, _) => Parent::Root,
                    (Side::Left, _, Origin::Elem(r)) => Parent::Node(r),
                    _ => continue,
                };
                st.forged += 1;
                // the forger's base is A's state: it holds C
                match a.admit(true, left, side, right) {
                    Err(Refuse::Forged(c)) => {
                        st.forged_refused += 1;
                        *st.refused_by.entry(c).or_insert(0) += 1;
                        if plant == Plant::None {
                            if c == Check::LeftBit {
                                st.a_only += 1;
                            }
                            if matches!(c, Check::LeftOrigin | Check::LeftOriginBit) && !matches!(right, Origin::Elem(r) if a.left_bit(r)) {
                                st.c_only_left += 1;
                            }
                        }
                    }
                    Err(Refuse::StaleBase) => return Err(ctx("a base holding C refused as stale".into())),
                    Ok(()) => {
                        st.forged_admitted += 1;
                        let f = rng.below(2) as usize;
                        let id = ([0u8, 9u8][f], forger_seq[f]);
                        forger_seq[f] += 1;
                        a.integrate(id, parent, side, right).map_err(|e| ctx(format!("A forged {e:?}")))?;
                        twin.integrate(id, parent, side, right).map_err(|e| ctx(format!("twin forged {e:?}")))?;
                        let past = known[0].clone();
                        known[0].insert(id);
                        evs.push(Ev { id, left, parent, side, ro: right, past });
                    }
                }
            } else if roll < 45 {
                // a local insert (A, B, C, or D offline)
                let r = rng.below(4) as usize;
                let order: Vec<K> = if r == 0 { a.order().to_vec() } else { full[r - 1].order() };
                // A types inside collapsed text most of the time: after a collapsed element
                let collapsed: Vec<usize> = (0..order.len()).filter(|&i| r == 0 && a.is_collapsed(order[i])).collect();
                let i = if !collapsed.is_empty() && rng.below(100) < 70 {
                    collapsed[rng.below(collapsed.len() as u64) as usize] + 1
                } else {
                    rng.below(order.len() as u64 + 1) as usize
                };
                let left = i.checked_sub(1).and_then(|j| order.get(j)).copied().map_or(Origin::Start, Origin::Elem);
                let id = (r as u8 + 1, seq[r]);
                seq[r] += 1;
                let (parent, side, ro) = if r == 0 {
                    let g = a.generate(left).map_err(|e| ctx(format!("generate {e:?}")))?;
                    let right = twin.next_after(left);
                    let (p, s) = twin.placement(left, right).map_err(|e| ctx(format!("twin placement {e:?}")))?;
                    if let Origin::Elem(l) = left {
                        if a.is_collapsed(l) {
                            st.local_in_collapsed += 1;
                            if matches!(right, Origin::Elem(rr) if a.is_collapsed(rr)) {
                                st.local_mid_collapsed += 1;
                            }
                        }
                    }
                    if g != (p, s, right) {
                        return Err(ctx(format!("A declares {g:?}, the full tree {:?}", (p, s, right))));
                    }
                    // A's own op passes its own admission
                    if let Err(x) = a.admit(true, left, s, right) {
                        return Err(ctx(format!("A's own local insert refused {x:?}")));
                    }
                    g
                } else {
                    let right = full[r - 1].next_after(left);
                    let (p, s) = full[r - 1].placement(left, right).map_err(|e| ctx(format!("placement {e:?}")))?;
                    (p, s, right)
                };
                let past = known[r].clone();
                if r == 0 {
                    a.integrate(id, parent, side, ro).map_err(|e| ctx(format!("A integrate {e:?}")))?;
                    twin.integrate(id, parent, side, ro).map_err(|e| ctx(format!("twin {e:?}")))?;
                } else {
                    full[r - 1].integrate(id, parent, side, ro).map_err(|e| ctx(format!("full {e:?}")))?;
                }
                known[r].insert(id);
                let ev = Ev { id, left, parent, side, ro, past };
                if r == D {
                    outbox.push(ev);
                } else {
                    evs.push(ev);
                }
            } else if roll < 52 {
                // D sends its oldest unsent event to A: admitted when its base holds C, else a
                // stale base (never "forged"), after which D resyncs from A
                if outbox.is_empty() {
                    continue;
                }
                let e = outbox.remove(0);
                st.d_sent += 1;
                let holds = c_set.is_subset(&e.past);
                match a.admit(holds, e.left, e.side, e.ro) {
                    Ok(()) if holds => {
                        st.d_admitted += 1;
                        a.integrate(e.id, e.parent, e.side, e.ro).map_err(|x| ctx(format!("A from D {x:?}")))?;
                        twin.integrate(e.id, e.parent, e.side, e.ro).map_err(|x| ctx(format!("twin from D {x:?}")))?;
                        known[0].insert(e.id);
                        evs.push(e);
                    }
                    Err(Refuse::StaleBase) if !holds => {
                        st.d_stale += 1;
                        if a.forged(e.left, e.side, e.ro).is_err() {
                            st.d_stale_would_be_forged += 1;
                        }
                        // E182 / E264: D drops its unsent changes and resyncs from A's state
                        st.d_resyncs += 1;
                        outbox.clear();
                        full[D - 1] = twin.clone();
                        known[D] = known[0].clone();
                    }
                    other => {
                        return Err(ctx(format!("an honest client's event {:?} (base holds C: {holds}) got {other:?}", e.id)));
                    }
                }
            } else if roll < 85 {
                // a causal delivery (D only receives what A holds: A is its host)
                let r = rng.below(4) as usize;
                let ready: Vec<usize> = (0..evs.len())
                    .filter(|&j| !known[r].contains(&evs[j].id) && evs[j].past.is_subset(&known[r]) && (r != D || known[0].contains(&evs[j].id)))
                    .collect();
                if let Some(&j) = ready.get(rng.below(ready.len().max(1) as u64) as usize) {
                    let e = evs[j].clone();
                    if r == 0 {
                        // an honest event (or a forged one A admitted itself) is never refused
                        if e.id.0 != 9 && e.id.0 != 0 {
                            let holds = c_set.is_subset(&e.past);
                            if let Err(x) = a.admit(holds, e.left, e.side, e.ro) {
                                let show: Vec<(K, bool)> = a.order().iter().map(|k| (*k, a.is_collapsed(*k))).collect();
                                return Err(ctx(format!("an honest event {:?} refused {x:?}: left {:?} side {:?} parent {:?} ro {:?}; A {show:?}", e.id, e.left, e.side, e.parent, e.ro)));
                            }
                        }
                        a.integrate(e.id, e.parent, e.side, e.ro).map_err(|x| ctx(format!("A deliver {x:?}")))?;
                        twin.integrate(e.id, e.parent, e.side, e.ro).map_err(|x| ctx(format!("twin deliver {x:?}")))?;
                    } else {
                        full[r - 1].integrate(e.id, e.parent, e.side, e.ro).map_err(|x| ctx(format!("deliver {x:?}")))?;
                    }
                    known[r].insert(e.id);
                }
            } else if roll < 95 {
                // collapse below C: causally stable among the peers A, B, C (held by all, in the
                // past of every event not yet everywhere); D is offline, so C may pass its base.
                // Planted Eager: everything A holds
                let all: BTreeSet<K> = known[0].intersection(&known[1]).copied().collect::<BTreeSet<K>>().intersection(&known[2]).copied().collect();
                let mut c: BTreeSet<K> = all.clone();
                for e in evs.iter().filter(|e| !all.contains(&e.id)) {
                    c.retain(|x| e.past.contains(x));
                }
                if plant == Plant::Eager {
                    c = known[0].clone();
                }
                c_set.extend(c);
                let cs = c_set.clone();
                a.collapse(|k| cs.contains(&k));
                let clear = |a: &mut Collapsed<K>, left: bool| {
                    for e in a.elems.values_mut() {
                        if let Elem::Collapsed { left_child, right_child } = e {
                            let b = if left { left_child } else { right_child };
                            if *b {
                                *b = false;
                                return;
                            }
                        }
                    }
                };
                match plant {
                    Plant::ClearBit => clear(&mut a, false),
                    Plant::ClearLeftBit => clear(&mut a, true),
                    _ => {}
                }
                st.collapses += 1;
            } else {
                // A is cut and reloaded
                a = a.reload();
                st.loads += 1;
            }
            if plant == Plant::ClearShadowedBit && step + 1 == steps {
                if let Some(Elem::Collapsed { right_child, .. }) = a.elems.values_mut().find(|e| matches!(e, Elem::Collapsed { right_child: true, .. })) {
                    *right_child = false;
                }
            }
            // both bits are monotone: a set bit never clears, an element once collapsed stays so
            for (k, (wl, wr)) in bits.iter() {
                match a.elems.get(k) {
                    Some(Elem::Collapsed { left_child, right_child }) if (*left_child || !*wl) && (*right_child || !*wr) => {}
                    other => return Err(ctx(format!("{k:?}'s bit went from {:?} to {other:?}", (wl, wr)))),
                }
            }
            bits = a
                .elems
                .iter()
                .filter_map(|(k, e)| match e {
                    Elem::Collapsed { left_child, right_child } => Some((*k, (*left_child, *right_child))),
                    _ => None,
                })
                .collect();
            if a.order() != twin.order().as_slice() {
                return Err(ctx(format!("A's order {:?} != the full tree's {:?}", a.order(), twin.order())));
            }
            for r in 1..3 {
                if known[r] == known[0] {
                    st.compared_with_b += 1;
                    if full[r - 1].order() != twin.order() {
                        return Err(ctx("replicas with the same events differ".into()));
                    }
                }
            }
        }
        st.left_bits_set += bits.values().filter(|(l, _)| *l).count() as u64;
        Ok(())
    }

    fn env(name: &str, default: u64) -> u64 {
        std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
    }

    #[test]
    fn pt_collapsed_equals_the_full_tree() {
        let seeds = env("MPCE_COLLAPSED_SEEDS", 2000);
        let mut st = Stats::default();
        for seed in 0..seeds {
            run(seed, 120, Plant::None, &mut st).unwrap_or_else(|e| panic!("{e}"));
        }
        println!("COLLAPSED-PT seeds={seeds} {st:?}");
        assert!(st.local_in_collapsed > 0 && st.local_mid_collapsed > 0 && st.collapses > 0 && st.loads > 0 && st.compared_with_b > 0, "{st:?}");
        assert!(st.forged_refused > 0 && st.forged_admitted > 0, "the forgeries weren't both refused and admitted: {st:?}");
        for c in [Check::RightBit, Check::RootRight, Check::LeftOrigin, Check::LeftOriginBit] {
            assert!(st.refused_by.get(&c).is_some_and(|n| *n > 0), "rule C's {c:?} never refused a forgery: {st:?}");
        }
        assert!(st.left_bits_set > 0, "no left bit was ever set: {st:?}");
        // rule A's left-bit check is redundant given rule C: it never refuses what C admits
        assert_eq!(st.a_only, 0, "{st:?}");
        assert!(st.c_only_left > 0, "rule C never refused a left child that rule A admits: {st:?}");
        assert!(st.d_admitted > 0 && st.d_stale > 0 && st.d_resyncs > 0, "D never both admitted and stale: {st:?}");
        assert!(st.d_stale_would_be_forged > 0, "no stale event would have looked forged (the base-first order untested): {st:?}");
    }

    /// One planted variant per removed check, each showing a divergence from the full tree
    /// (or an integration it can't make), and the others. Removing one of the left-child checks
    /// alone is not caught: for a left child of a collapsed `R`, the full tree differs from the
    /// collapsed one exactly when `R` has a left child below C (its left siblings there), which
    /// rule A's left bit refuses; rule C's left clauses refuse a superset (whatever no honest
    /// creator makes). So each side is enough for convergence alone: rule A's left bit is
    /// redundant given rule C (§4.4), and rule C's left clauses given rule A's left bit; both
    /// stay (C is the exact honesty rule; A is defence in depth). Removing all three diverges.
    #[test]
    fn pt_collapsed_planted_negatives_are_caught() {
        const LEFT_ALL: &[Check] = &[Check::LeftOrigin, Check::LeftOriginBit, Check::LeftBit];
        let seeds = env("MPCE_COLLAPSED_PLANT_SEEDS", 4000);
        for p in [
            Plant::IgnoreBit,
            Plant::Eager,
            Plant::ClearBit,
            Plant::ClearLeftBit,
            Plant::ClearShadowedBit,
            Plant::NoAdmission,
            Plant::Skip(&[Check::RightBit]),
            Plant::Skip(&[Check::RootRight]),
            Plant::Skip(LEFT_ALL),
            Plant::ForgedFirst,
        ] {
            let mut st = Stats::default();
            let caught = (0..seeds).find_map(|seed| run(seed, 120, p, &mut st).err());
            let Some(msg) = caught else { panic!("planted {p:?} not caught in {seeds} seeds") };
            match p {
                // no placement reads those bits after the clearing: only the monotonicity
                // assertion sees it
                Plant::ClearShadowedBit | Plant::ClearLeftBit => assert!(msg.contains("'s bit went from"), "{p:?} caught by something else: {msg}"),
                // a removed check must show as a divergence (or an integration the full tree
                // can't make), never as anything else
                Plant::Skip(_) | Plant::NoAdmission => {
                    assert!(msg.contains("!= the full tree's") || msg.contains("twin forged") || msg.contains("replicas with the same events differ"), "{p:?} not a divergence: {msg}")
                }
                Plant::ForgedFirst => assert!(msg.contains("an honest client's event") && msg.contains("Forged"), "{p:?}: {msg}"),
                _ => {}
            }
            println!("COLLAPSED-PT planted {p:?}: {msg}");
        }
        // one left-child check removed (or rule C's two left clauses): the other side still
        // guards convergence, nothing diverges
        let quiet_seeds = seeds / 2;
        for skip in [&[Check::LeftBit][..], &[Check::LeftOrigin], &[Check::LeftOriginBit], &[Check::LeftOrigin, Check::LeftOriginBit]] {
            let mut st = Stats::default();
            let quiet = (0..quiet_seeds).find_map(|seed| run(seed, 120, Plant::Skip(skip), &mut st).err());
            assert!(quiet.is_none(), "removing {skip:?} alone diverged: {quiet:?}");
            println!("COLLAPSED-PT planted Skip({skip:?}): no divergence in {quiet_seeds} seeds (the other left-child guard holds)");
        }
    }
}
