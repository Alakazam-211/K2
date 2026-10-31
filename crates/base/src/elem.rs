//! Structured kinds as data: element keys, values, list and map ops, and what a change did.
//!
//! Engine-free, like the rest of the public API: a list element is named by its [`Key`] (an
//! identity the caller chooses and that lives in the element's value), never by an engine
//! offset or ID; a write is named by an opaque [`WriteId`].
//!
//! - [`Key`], [`Value`]: identities and values (A2);
//! - [`ListOp`], [`MapOp`], [`Anchor`]: edits (A3; carried by [`crate::Edit::List`] and
//!   [`crate::Edit::Map`]);
//! - [`ElemState`], [`Facet`]: readers (A4, [`crate::Doc::elem`], [`crate::Doc::winner`]);
//! - [`Applied`], [`ListApplied`], [`MapApplied`], [`Effect`]: the applied-change stream (A5,
//!   [`crate::Doc::edit_applied`]);
//! - [`chain`], [`reorder`]: **the move rule** (E8/E9): a move of several elements (a block
//!   move, a sort) is generated as a forward chain, each move anchored right after the
//!   previous one's new place. Generate multi-element moves only through these.
//!
//! Semantics (the kit's, on every engine; `docs/PRIMITIVES.md`):
//! - **MovableList:** insert, move, delete, restore, set. A key is never reused, live or
//!   deleted (E2): an insert of a key the list has ever held is refused; bring it back with
//!   [`ListOp::Restore`]. Delete hides the element and keeps its value and place; restore
//!   shows it again where it was (E1: undelete; a delete concurrent with a restore wins).
//!   A move or set of a deleted element is legal and stays hidden (E10: checks are per op,
//!   never "the element is live").
//! - **Map:** set and delete by string key, last writer wins per key.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

use crate::doc::ClientId;
use crate::text::diff::Hunk;

/// Longest list [`Key`], in bytes.
pub const MAX_KEY_LEN: usize = 24;

/// Longest map key, in bytes.
pub const MAX_MAP_KEY_LEN: usize = 256;

/// Why a key is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("a key may not be empty")]
    Empty,
    #[error("key is {len} bytes; the limit is {max}")]
    TooLong { len: usize, max: usize },
    #[error("key byte {at} ({byte:#04x}) is not allowed")]
    BadByte { at: usize, byte: u8 },
}

/// A list element's identity: 1 to [`MAX_KEY_LEN`] bytes of `[0-9a-z._-]`. Unique within its
/// list and never reused. It is the element's value, not the engine's element ID, so it
/// survives an engine migration (CSV contract §1.2).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(String);

impl Key {
    pub fn new(s: impl Into<String>) -> Result<Self, KeyError> {
        let s = s.into();
        if s.is_empty() {
            return Err(KeyError::Empty);
        }
        if s.len() > MAX_KEY_LEN {
            return Err(KeyError::TooLong {
                len: s.len(),
                max: MAX_KEY_LEN,
            });
        }
        if let Some((at, &byte)) = s
            .as_bytes()
            .iter()
            .enumerate()
            .find(|(_, b)| !matches!(b, b'0'..=b'9' | b'a'..=b'z' | b'.' | b'_' | b'-'))
        {
            return Err(KeyError::BadByte { at, byte });
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Key {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for Key {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for Key {
    type Error = KeyError;
    fn try_from(s: &str) -> Result<Self, KeyError> {
        Key::new(s)
    }
}

impl std::str::FromStr for Key {
    type Err = KeyError;
    fn from_str(s: &str) -> Result<Self, KeyError> {
        Key::new(s)
    }
}

/// A map key's rule: at most [`MAX_MAP_KEY_LEN`] bytes, no NUL.
pub fn check_map_key(key: &str) -> Result<(), KeyError> {
    if key.len() > MAX_MAP_KEY_LEN {
        return Err(KeyError::TooLong {
            len: key.len(),
            max: MAX_MAP_KEY_LEN,
        });
    }
    match key.bytes().position(|b| b == 0) {
        Some(at) => Err(KeyError::BadByte { at, byte: 0 }),
        None => Ok(()),
    }
}

/// A value held by a list element or a map key. A register: a set replaces it whole.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum Value {
    /// No value (a list element inserted without one).
    #[default]
    Null,
    Str(String),
}

impl Value {
    pub fn str(s: impl Into<String>) -> Self {
        Value::Str(s.into())
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            Value::Null => None,
        }
    }
}

/// Where an insert or a move puts an element, by identity. `After`/`Before` name an element
/// of the same list, live or deleted (a deleted element keeps its place).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Anchor {
    Start,
    End,
    After(Key),
    Before(Key),
}

/// One op on a MovableList root ([`crate::Edit::List`]). Ops apply in order, each against
/// the state the previous ones left.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListOp {
    /// A new element. `key` must never have been in the list.
    Insert { key: Key, at: Anchor, value: Value },
    /// Move an element. A move of a deleted element moves where it would come back.
    Move { key: Key, to: Anchor },
    /// Hide an element; its value and place are kept.
    Delete { key: Key },
    /// Show a deleted element again, where it was, with its value (undelete).
    Restore { key: Key },
    /// Replace an element's value.
    Set { key: Key, value: Value },
    /// Remove a **deleted** element for good: its slot and registers leave the list, so it
    /// can't be restored and stops taking space (`rows drop-deleted`, K-CSV-7). Not erasure:
    /// versions and change records keep it. Refused for a live element. Its key is retired
    /// by whoever mints keys (a table's counters), never by the list, which forgets it.
    Drop { key: Key },
}

impl ListOp {
    /// The element the op is about.
    pub fn key(&self) -> &Key {
        match self {
            ListOp::Insert { key, .. }
            | ListOp::Move { key, .. }
            | ListOp::Delete { key }
            | ListOp::Restore { key }
            | ListOp::Set { key, .. }
            | ListOp::Drop { key } => key,
        }
    }

    pub fn kind(&self) -> ListOpKind {
        match self {
            ListOp::Insert { .. } => ListOpKind::Insert,
            ListOp::Move { .. } => ListOpKind::Move,
            ListOp::Delete { .. } => ListOpKind::Delete,
            ListOp::Restore { .. } => ListOpKind::Restore,
            ListOp::Set { .. } => ListOpKind::Set,
            ListOp::Drop { .. } => ListOpKind::Drop,
        }
    }
}

/// One op on a Map root ([`crate::Edit::Map`]). Keys follow [`check_map_key`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapOp {
    Set { key: String, value: Value },
    Delete { key: String },
}

impl MapOp {
    pub fn key(&self) -> &str {
        match self {
            MapOp::Set { key, .. } | MapOp::Delete { key } => key,
        }
    }

    pub fn kind(&self) -> MapOpKind {
        match self {
            MapOp::Set { .. } => MapOpKind::Set,
            MapOp::Delete { .. } => MapOpKind::Delete,
        }
    }
}

/// The move rule (E8/E9): move `keys`, in this order, to `to`, as a **forward chain**: the
/// first to `to`, each next one right after the previous one's new place. The new places
/// form one run, so a block move or a sort costs O(1) engine index entries instead of O(n),
/// and two concurrent block moves into one gap don't interleave. `keys` must be distinct and
/// must not include an `After`/`Before` anchor's key.
pub fn chain(keys: &[Key], to: Anchor) -> Vec<ListOp> {
    let mut out = Vec::with_capacity(keys.len());
    let mut at = to;
    for k in keys {
        out.push(ListOp::Move {
            key: k.clone(),
            to: at,
        });
        at = Anchor::After(k.clone());
    }
    out
}

/// The moves that turn the live order `current` into `target` (the same keys, reordered), as
/// one forward chain ([`chain`]): the common prefix and suffix stay put, and the elements
/// between are moved, in target order, right after the last element of the prefix (or to the
/// start). `None` when the two are not permutations of each other.
///
/// Not the minimal move set: a sort that moves one row from the top to the bottom moves every
/// row between. One run per sort is what keeps a sort's cost on the engine's index O(1)
/// (E8).
pub fn reorder(current: &[Key], target: &[Key]) -> Option<Vec<ListOp>> {
    if current.len() != target.len() {
        return None;
    }
    let mut a: Vec<&Key> = current.iter().collect();
    let mut b: Vec<&Key> = target.iter().collect();
    a.sort_unstable();
    b.sort_unstable();
    if a != b || a.windows(2).any(|w| w[0] == w[1]) {
        return None;
    }
    let n = current.len();
    let pre = current
        .iter()
        .zip(target)
        .take_while(|(x, y)| x == y)
        .count();
    if pre == n {
        return Some(Vec::new());
    }
    let suf = current[pre..]
        .iter()
        .rev()
        .zip(target[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let to = match pre {
        0 => Anchor::Start,
        p => Anchor::After(target[p - 1].clone()),
    };
    Some(chain(&target[pre..n - suf], to))
}

/// What a reader finds for a key ([`crate::Doc::elem`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElemState {
    /// Shown, at this index of the live order.
    Live(usize),
    /// Known (inserted once) and hidden.
    Deleted,
    /// Never inserted.
    Unknown,
}

/// A list element's three registers, or a map key's value ([`crate::Doc::winner`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Facet {
    Value,
    Position,
    Existence,
}

/// The identity of one write: a participant's change (its client and the change's place in
/// that client's history in this doc) and the op's index in the change. Opaque: compare,
/// hash and print it, take its client; nothing else about it is meaningful. Unique within a
/// doc among changes that write: a change that writes nothing (every op a no-op, no update)
/// shares its change number with the same client's next change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WriteId {
    client: ClientId,
    change: u64,
    op: u32,
}

impl WriteId {
    pub(crate) fn new(client: ClientId, change: u64, op: u32) -> Self {
        Self { client, change, op }
    }

    /// The participant's client that wrote it.
    pub fn client(&self) -> ClientId {
        self.client
    }

    /// True when both writes were made by one change.
    pub fn same_change(&self, other: &WriteId) -> bool {
        self.client == other.client && self.change == other.change
    }

    /// Its stored spelling: `<client>.<change>.<op>`, base 36.
    pub(crate) fn encode(&self) -> String {
        format!(
            "{}.{}.{}",
            base36(self.client),
            base36(self.change),
            base36(u64::from(self.op))
        )
    }

    pub(crate) fn decode(s: &str) -> Option<Self> {
        let mut it = s.split('.');
        let client = unbase36(it.next()?)?;
        let change = unbase36(it.next()?)?;
        let op = u32::try_from(unbase36(it.next()?)?).ok()?;
        if it.next().is_some() || client > crate::doc::MAX_CLIENT_ID {
            return None;
        }
        Some(Self { client, change, op })
    }
}

impl fmt::Display for WriteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

fn base36(mut n: u64) -> String {
    const D: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut b = Vec::with_capacity(13);
    loop {
        b.push(D[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    b.reverse();
    String::from_utf8(b).expect("ascii")
}

fn unbase36(s: &str) -> Option<u64> {
    if s.is_empty() || s.len() > 13 || (s.len() > 1 && s.starts_with('0')) {
        return None;
    }
    let mut n: u64 = 0;
    for c in s.bytes() {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'z' => c - b'a' + 10,
            _ => return None,
        };
        n = n.checked_mul(36)?.checked_add(u64::from(d))?;
    }
    Some(n)
}

/// What one change did, root by root, in apply order (A5). Subscribers, undo, revert and
/// blame work over it, as they do over hunks for text.
///
/// `W` is the write identity: a [`WriteId`] in a change made here ([`crate::Doc::edit_applied`]);
/// `Option<WriteId>` in a stream derived for a merged update ([`crate::Doc::applied_since`],
/// [`keyed_diff`]), where the engine may not know who wrote what (`None`). Another engine's
/// adapter may use its own identity (an event ID); [`Applied::map_write`] converts.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied<W = WriteId> {
    /// A text root's hunks: as given in a local change; a minimal diff in a derived stream.
    Text { root: String, hunks: Vec<Hunk> },
    List {
        root: String,
        ops: Vec<ListApplied<W>>,
    },
    Map {
        root: String,
        ops: Vec<MapApplied<W>>,
    },
}

impl<W> Applied<W> {
    /// The root it is about.
    pub fn root(&self) -> &str {
        match self {
            Applied::Text { root, .. } | Applied::List { root, .. } | Applied::Map { root, .. } => {
                root
            }
        }
    }

    /// The same stream with every write identity (op writes and effect winners) mapped by `f`.
    pub fn map_write<V>(self, mut f: impl FnMut(W) -> V) -> Applied<V> {
        match self {
            Applied::Text { root, hunks } => Applied::Text { root, hunks },
            Applied::List { root, ops } => Applied::List {
                root,
                ops: ops.into_iter().map(|o| o.map_write(&mut f)).collect(),
            },
            Applied::Map { root, ops } => Applied::Map {
                root,
                ops: ops.into_iter().map(|o| o.map_write(&mut f)).collect(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListOpKind {
    Insert,
    Move,
    Delete,
    Restore,
    Set,
    /// A deleted element removed for good ([`ListOp::Drop`]): nothing shows before or after.
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MapOpKind {
    Set,
    Delete,
}

/// What an op did to what readers see (E4). `W` as in [`Applied`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Effect<W = WriteId> {
    /// It shows: the element appeared, moved, vanished or changed value in the live order
    /// (or the map key changed).
    Visible,
    /// It was written and wins its register, but its element is deleted, so nothing shows
    /// until a restore (a move or set of a deleted element).
    Hidden,
    /// Nothing changed (a delete of a deleted element, a restore of a live one, a delete of
    /// an absent map key). `winner` is the facet's current winner, when the doc knows it
    /// ([`crate::Doc::winner`]).
    None { winner: Option<W> },
}

impl<W> Effect<W> {
    /// The same effect with its winner mapped by `f`.
    pub fn map_write<V>(self, f: impl FnOnce(W) -> V) -> Effect<V> {
        match self {
            Effect::Visible => Effect::Visible,
            Effect::Hidden => Effect::Hidden,
            Effect::None { winner } => Effect::None {
                winner: winner.map(f),
            },
        }
    }
}

/// One list op as applied. Indices are positions in the live order (`None`: not shown) just
/// before and just after this op, so replaying the ops in order over the old live order
/// gives the new one. Values: `None` only before an insert. `W` as in [`Applied`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListApplied<W = WriteId> {
    pub kind: ListOpKind,
    pub key: Key,
    pub write: W,
    pub index_before: Option<usize>,
    pub index_after: Option<usize>,
    pub value_before: Option<Value>,
    pub value_after: Option<Value>,
    pub effect: Effect<W>,
}

impl<W> ListApplied<W> {
    /// The same op with its write identities mapped by `f`.
    pub fn map_write<V>(self, mut f: impl FnMut(W) -> V) -> ListApplied<V> {
        ListApplied {
            kind: self.kind,
            key: self.key,
            write: f(self.write),
            index_before: self.index_before,
            index_after: self.index_after,
            value_before: self.value_before,
            value_after: self.value_after,
            effect: self.effect.map_write(f),
        }
    }
}

/// One map op as applied. `before`/`after`: the key's value (`None`: absent). `W` as in
/// [`Applied`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapApplied<W = WriteId> {
    pub kind: MapOpKind,
    pub key: String,
    pub write: W,
    pub before: Option<Value>,
    pub after: Option<Value>,
    pub effect: Effect<W>,
}

impl<W> MapApplied<W> {
    /// The same op with its write identities mapped by `f`.
    pub fn map_write<V>(self, mut f: impl FnMut(W) -> V) -> MapApplied<V> {
        MapApplied {
            kind: self.kind,
            key: self.key,
            write: f(self.write),
            before: self.before,
            after: self.after,
            effect: self.effect.map_write(f),
        }
    }
}

/// A change as made by [`crate::Doc::edit_applied`]: the update to broadcast and log
/// (`None` when nothing was written), and what each op did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edited {
    pub update: Option<Vec<u8>>,
    pub applied: Vec<Applied>,
}

/// A doc's content by key, hidden list elements included: what a keyed diff ([`keyed_diff`])
/// compares. From [`crate::Doc::keyed_state`]; engine-free, so another engine's adapter can
/// build one from its own readers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyedState {
    /// Every root, in schema order.
    pub roots: Vec<(String, KeyedRoot)>,
}

/// One root of a [`KeyedState`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyedRoot {
    Text(String),
    /// A MovableList: the live elements in order, every deleted element's value, and
    /// `places`: every element, live and deleted, in place order (a deleted element sits
    /// where a restore would show it), so a reader that restores (a `write --base` merge, a
    /// version restore) puts it back where it was. Empty in a state built without the doc
    /// (tests, a version): readers then put deleted elements after the live ones.
    List {
        live: Vec<(Key, Value)>,
        hidden: BTreeMap<Key, Value>,
        places: Vec<Key>,
    },
    Map(BTreeMap<String, Value>),
}

/// What changed from `before` to `after` (two states of one doc, the second later), as an
/// applied stream: **the stream of a merged update** (CSV seam design §4.2). A change made
/// here reports its own ops ([`crate::Doc::edit_applied`]); a merged update (another
/// replica's changes) is derived by diffing the keyed states around it. Keys are identities,
/// so the diff is exact up to the choice of moves.
///
/// Per changed root, in schema order:
/// - **text**: a minimal diff ([`crate::text::diff::hunks`]), not the edits as made;
/// - **list**: deletes (old order), then the elements placed in the new order, left to right
///   (inserts of unknown keys, restores of hidden ones, and moves: a minimal set, every
///   element outside a longest run that kept its relative order, each put right after its new
///   left neighbour), then sets of changed values: shown ([`Effect::Visible`], at the new
///   index) or of deleted elements ([`Effect::Hidden`]). Indices are live-order positions just
///   before and just after each op, as in a local stream, so replaying the ops over the old
///   live order gives the new one;
/// - **map**: sets and deletes of changed keys, in key order.
///
/// Each op's write is `winner(root, key, facet)` now: the position for an insert or move,
/// existence for a delete or restore, the value for a set (and for a map op). `None` where
/// the engine knows no writer (on Yrs, a restore and a map delete: a removal keeps no
/// identity).
///
/// Not reported, because no reader can see it: a move of a deleted element (its place shows
/// only when it is restored, and the restore then reports where), an element inserted and
/// deleted within the merged changes, and writes that changed nothing (a move that lost, a
/// set to the same value; a local stream reports those with [`Effect::None`]). O(roots ×
/// elements + moved elements × elements).
pub fn keyed_diff<W>(
    before: &KeyedState,
    after: &KeyedState,
    mut winner: impl FnMut(&str, &str, Facet) -> Option<W>,
) -> Vec<Applied<Option<W>>> {
    let mut out = Vec::new();
    for (root, a) in &after.roots {
        let Some((_, b)) = before.roots.iter().find(|(r, _)| r == root) else {
            continue;
        };
        match (b, a) {
            (KeyedRoot::Text(b), KeyedRoot::Text(a)) if b != a => out.push(Applied::Text {
                root: root.clone(),
                hunks: crate::text::diff::hunks(b, a),
            }),
            (
                KeyedRoot::List {
                    live: bl,
                    hidden: bh,
                    ..
                },
                KeyedRoot::List {
                    live: al,
                    hidden: ah,
                    ..
                },
            ) => {
                let ops = diff_list(bl, bh, al, ah, &mut |k, f| winner(root, k, f));
                if !ops.is_empty() {
                    out.push(Applied::List {
                        root: root.clone(),
                        ops,
                    });
                }
            }
            (KeyedRoot::Map(b), KeyedRoot::Map(a)) => {
                let ops = diff_map(b, a, &mut |k| winner(root, k, Facet::Value));
                if !ops.is_empty() {
                    out.push(Applied::Map {
                        root: root.clone(),
                        ops,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

/// The indices of a longest strictly increasing subsequence of `seq`.
pub(crate) fn lis(seq: &[usize]) -> Vec<usize> {
    // tails[l]: index into seq of the smallest tail of an increasing run of length l + 1
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![usize::MAX; seq.len()];
    for (i, &x) in seq.iter().enumerate() {
        let l = tails.partition_point(|&t| seq[t] < x);
        if l > 0 {
            prev[i] = tails[l - 1];
        }
        if l == tails.len() {
            tails.push(i);
        } else {
            tails[l] = i;
        }
    }
    let mut out = Vec::with_capacity(tails.len());
    let mut at = tails.last().copied();
    while let Some(i) = at {
        out.push(i);
        at = (prev[i] != usize::MAX).then_some(prev[i]);
    }
    out.reverse();
    out
}

fn diff_list<W>(
    bl: &[(Key, Value)],
    bh: &BTreeMap<Key, Value>,
    al: &[(Key, Value)],
    ah: &BTreeMap<Key, Value>,
    winner: &mut dyn FnMut(&str, Facet) -> Option<W>,
) -> Vec<ListApplied<Option<W>>> {
    let a_index: HashMap<&Key, usize> = al.iter().enumerate().map(|(i, (k, _))| (k, i)).collect();
    let b_value: HashMap<&Key, &Value> = bl.iter().map(|(k, v)| (k, v)).collect();
    let mut ops = Vec::new();
    let mut op = |kind, key: &Key, facet, ib, ia, vb: Option<&Value>, va: Option<&Value>, e| {
        ops.push(ListApplied {
            kind,
            key: key.clone(),
            write: winner(key.as_str(), facet),
            index_before: ib,
            index_after: ia,
            value_before: vb.cloned(),
            value_after: va.cloned(),
            effect: e,
        })
    };
    // the live order as the ops so far left it
    let mut w: Vec<Key> = bl.iter().map(|(k, _)| k.clone()).collect();
    // deletes, in the old order
    for (k, v) in bl {
        if !a_index.contains_key(k) {
            let i = w.iter().position(|x| x == k).expect("in the order");
            w.remove(i);
            op(
                ListOpKind::Delete,
                k,
                Facet::Existence,
                Some(i),
                None,
                Some(v),
                Some(v),
                Effect::Visible,
            );
        }
    }
    // the elements that stay put: a longest run of kept ones in the same relative order
    let kept: Vec<&Key> = w.iter().collect();
    let seq: Vec<usize> = kept.iter().map(|k| a_index[k]).collect();
    let stay: HashSet<Key> = lis(&seq).into_iter().map(|i| kept[i].clone()).collect();
    // place the rest in the new order, each right after its new left neighbour
    for (i, (k, v)) in al.iter().enumerate() {
        if stay.contains(k) {
            continue;
        }
        let to = match i {
            0 => 0,
            _ => w.iter().position(|x| *x == al[i - 1].0).expect("placed") + 1,
        };
        if let Some(vb) = b_value.get(k) {
            let from = w.iter().position(|x| x == k).expect("in the order");
            let to = if from < to { to - 1 } else { to };
            if from == to {
                // already right after its new left neighbour: nothing moves
                continue;
            }
            w.remove(from);
            w.insert(to, k.clone());
            op(
                ListOpKind::Move,
                k,
                Facet::Position,
                Some(from),
                Some(to),
                Some(vb),
                Some(vb),
                Effect::Visible,
            );
        } else if let Some(vh) = bh.get(k) {
            w.insert(to, k.clone());
            op(
                ListOpKind::Restore,
                k,
                Facet::Existence,
                None,
                Some(to),
                Some(vh),
                Some(vh),
                Effect::Visible,
            );
        } else {
            w.insert(to, k.clone());
            op(
                ListOpKind::Insert,
                k,
                Facet::Position,
                None,
                Some(to),
                None,
                Some(v),
                Effect::Visible,
            );
        }
    }
    debug_assert!(
        w.iter().eq(al.iter().map(|(k, _)| k)),
        "the placements give the new order"
    );
    // changed values: shown, then hidden
    for (i, (k, v)) in al.iter().enumerate() {
        if let Some(vb) = b_value.get(k).copied().or_else(|| bh.get(k)) {
            if vb != v {
                op(
                    ListOpKind::Set,
                    k,
                    Facet::Value,
                    Some(i),
                    Some(i),
                    Some(vb),
                    Some(v),
                    Effect::Visible,
                );
            }
        }
    }
    for (k, v) in ah {
        if let Some(vb) = b_value.get(k).copied().or_else(|| bh.get(k)) {
            if vb != v {
                op(
                    ListOpKind::Set,
                    k,
                    Facet::Value,
                    None,
                    None,
                    Some(vb),
                    Some(v),
                    Effect::Hidden,
                );
            }
        }
    }
    ops
}

fn diff_map<W>(
    b: &BTreeMap<String, Value>,
    a: &BTreeMap<String, Value>,
    winner: &mut dyn FnMut(&str) -> Option<W>,
) -> Vec<MapApplied<Option<W>>> {
    let keys: BTreeSet<&String> = b.keys().chain(a.keys()).collect();
    keys.into_iter()
        .filter_map(|k| {
            let (vb, va) = (b.get(k), a.get(k));
            if vb == va {
                return None;
            }
            Some(MapApplied {
                kind: if va.is_some() {
                    MapOpKind::Set
                } else {
                    MapOpKind::Delete
                },
                key: k.clone(),
                write: winner(k),
                before: vb.cloned(),
                after: va.cloned(),
                effect: Effect::Visible,
            })
        })
        .collect()
}

/// Why a structured op is refused (nothing of the change is applied).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpRefusal {
    #[error("key {0:?} was never in this list")]
    UnknownKey(String),
    #[error(
        "key {0:?} is already in this list (live or deleted); keys are never reused: restore it"
    )]
    KeyExists(String),
    #[error("element {0:?} can't be moved next to itself")]
    SelfAnchor(String),
    #[error("element {0:?} is live; only a deleted element can be dropped")]
    NotDeleted(String),
    #[error(transparent)]
    BadMapKey(#[from] KeyError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> Key {
        Key::new(s).unwrap()
    }

    #[test]
    fn keys_are_checked() {
        assert!(Key::new("r1").is_ok());
        assert!(Key::new("c12.w3_x-y").is_ok());
        assert!(Key::new("a".repeat(24)).is_ok());
        assert_eq!(Key::new(""), Err(KeyError::Empty));
        assert_eq!(
            Key::new("a".repeat(25)),
            Err(KeyError::TooLong { len: 25, max: 24 })
        );
        for bad in ["R1", "r 1", "r/1", "r\x1f1", "é", "r\u{0}"] {
            assert!(
                matches!(Key::new(bad), Err(KeyError::BadByte { .. })),
                "{bad:?}"
            );
        }
        assert!(check_map_key("r1\x1fc2").is_ok());
        assert!(check_map_key("").is_ok());
        assert!(check_map_key(&"x".repeat(256)).is_ok());
        assert!(check_map_key(&"x".repeat(257)).is_err());
        assert_eq!(
            check_map_key("a\0b"),
            Err(KeyError::BadByte { at: 1, byte: 0 })
        );
    }

    #[test]
    fn write_ids_round_trip() {
        for (c, ch, op) in [
            (2u64, 0u64, 0u32),
            (crate::doc::MAX_CLIENT_ID, u64::MAX, u32::MAX),
            (36, 35, 37),
        ] {
            let w = WriteId::new(c, ch, op);
            assert_eq!(WriteId::decode(&w.encode()), Some(w));
        }
        for bad in [
            "",
            "1.2",
            "1.2.3.4",
            "1..3",
            "01.2.3",
            "A.2.3",
            "zzzzzzzzzzzzzz.1.1",
            "1.2.zzzzzzz",
        ] {
            assert_eq!(WriteId::decode(bad), None, "{bad:?}");
        }
        assert_eq!(WriteId::new(36, 1, 0).encode(), "10.1.0");
        assert!(WriteId::new(5, 9, 0).same_change(&WriteId::new(5, 9, 3)));
        assert!(!WriteId::new(5, 9, 0).same_change(&WriteId::new(6, 9, 0)));
    }

    #[test]
    fn chain_anchors_each_move_after_the_previous() {
        let keys = [k("a"), k("b"), k("c")];
        assert_eq!(
            chain(&keys, Anchor::Before(k("z"))),
            vec![
                ListOp::Move {
                    key: k("a"),
                    to: Anchor::Before(k("z"))
                },
                ListOp::Move {
                    key: k("b"),
                    to: Anchor::After(k("a"))
                },
                ListOp::Move {
                    key: k("c"),
                    to: Anchor::After(k("b"))
                },
            ]
        );
        assert!(chain(&[], Anchor::End).is_empty());
    }

    #[test]
    fn reorder_trims_the_common_ends_and_chains_the_middle() {
        let v = |s: &str| s.split(' ').map(k).collect::<Vec<_>>();
        assert_eq!(reorder(&v("a b c"), &v("a b c")), Some(vec![]));
        assert_eq!(
            reorder(&v("a b c d"), &v("a c b d")),
            Some(chain(&v("c b"), Anchor::After(k("a"))))
        );
        assert_eq!(
            reorder(&v("a b c d"), &v("d c b a")),
            Some(chain(&v("d c b a"), Anchor::Start))
        );
        assert_eq!(reorder(&v("a b"), &v("a c")), None);
        assert_eq!(reorder(&v("a b"), &v("a")), None);
        assert_eq!(reorder(&v("a a"), &v("a a")), None);
    }
}
