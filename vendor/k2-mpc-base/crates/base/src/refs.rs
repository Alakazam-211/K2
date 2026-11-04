//! References to list elements, and their state: a reference is **never moved or deleted
//! silently**. A pack's data can point at an element of a MovableList (a link's end, a child's
//! parent, a comment's anchor) by its [`Key`]. When the element goes, or changes kind, or the
//! references form a cycle, the reference stays where it is and its state says so; nothing is
//! stored, and every replica computes the same state from the same converged doc.
//!
//! Engine-free, over a [`View`]. The states:
//! - [`RefState::Attached`]: the element is live and of a kind the reference accepts;
//! - [`RefState::Suspect`]: the reference holds but needs a look ([`SuspectWhy`]): the element
//!   changed kind, its container is gone, or it closes a cycle;
//! - [`RefState::Orphaned`]: the element is deleted or unknown. The reference keeps its
//!   [`Ref::hint`] (a pack value, such as the last point a link drew to) so it can still be
//!   drawn, and comes back attached when the element is restored.
//!
//! Why not refuse at write time: the kit checks no write against concurrent state (E10,
//! `docs/PRIMITIVES.md`, "Value checks belong to the pack"); a link written while its target
//! is deleted concurrently stays legal, and orphaned.

use std::collections::{BTreeMap, HashMap};

use crate::elem::{Applied, Key, ListOpKind, Value};
use crate::session::View;

/// A reference to element `key` of MovableList `root`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ref {
    pub root: String,
    pub key: Key,
    /// A pack value to draw an orphan with (a last point, a label); the kit never reads it.
    pub hint: Option<String>,
}

impl Ref {
    pub fn new(root: impl Into<String>, key: Key) -> Self {
        Self {
            root: root.into(),
            key,
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// Why a reference that holds needs a look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SuspectWhy {
    /// The element is live, but its value is not of a kind the reference accepts.
    KindChanged,
    /// The element's container is gone: the root isn't a list in the doc, or (for
    /// [`resolve_parents`]) the parent it names isn't live.
    ContainerGone,
    /// The element closes a cycle of parents; it is where the cycle is cut (the smallest key
    /// in the cycle), and reads as having no parent.
    Cycle,
}

/// A reference's state, computed from the doc; never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RefState {
    Attached,
    Suspect(SuspectWhy),
    Orphaned,
}

/// The state of `r` in `view`. `kind_ok` is the pack's rule for the element's value (a link's
/// end accepts a shape, not a connector, say); `|_| true` accepts any.
pub fn resolve(view: &View, r: &Ref, kind_ok: &dyn Fn(&Value) -> bool) -> RefState {
    let Some(live) = view.list(&r.root) else {
        return RefState::Suspect(SuspectWhy::ContainerGone);
    };
    match live.iter().find(|(k, _)| *k == r.key) {
        Some((_, v)) => state_of(v, kind_ok),
        None => RefState::Orphaned,
    }
}

fn state_of(v: &Value, kind_ok: &dyn Fn(&Value) -> bool) -> RefState {
    if kind_ok(v) {
        RefState::Attached
    } else {
        RefState::Suspect(SuspectWhy::KindChanged)
    }
}

/// Every reference in `refs` with its state, in the same order: **one entry per reference**,
/// orphans included (a reference never drops out).
pub fn resolve_all(
    view: &View,
    refs: &[Ref],
    kind_ok: &dyn Fn(&Value) -> bool,
) -> Vec<(Ref, RefState)> {
    let mut index: HashMap<&str, Option<HashMap<&Key, &Value>>> = HashMap::new();
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        let live = index.entry(r.root.as_str()).or_insert_with(|| {
            view.list(&r.root)
                .map(|l| l.iter().map(|(k, v)| (k, v)).collect())
        });
        let s = match live {
            None => RefState::Suspect(SuspectWhy::ContainerGone),
            Some(l) => match l.get(&r.key) {
                Some(v) => state_of(v, kind_ok),
                None => RefState::Orphaned,
            },
        };
        if s == RefState::Orphaned && planted_orphan_drops_link() {
            continue;
        }
        out.push((r.clone(), s));
    }
    out
}

/// The parent references of list `root`'s live elements: for each live element whose
/// `parent_of` names a parent, its state:
/// - [`RefState::Attached`]: the parent is live in `root`;
/// - [`SuspectWhy::ContainerGone`]: the parent is deleted or unknown (the element reads as
///   having no parent, and keeps its own place);
/// - [`SuspectWhy::Cycle`]: the element closes a cycle of live parents. Each cycle is cut at
///   its **smallest key**, the same on every replica whatever the list's order or the order
///   the writes arrived in; the rest of the cycle stays attached.
///
/// Elements with no parent aren't in the map.
pub fn resolve_parents(
    view: &View,
    root: &str,
    parent_of: &dyn Fn(&Key) -> Option<Key>,
) -> BTreeMap<Key, RefState> {
    let mut out = BTreeMap::new();
    let Some(live) = view.list(root) else {
        return out;
    };
    let pos: HashMap<&Key, usize> = live.iter().enumerate().map(|(i, (k, _))| (k, i)).collect();
    // the parent edge of each live element, by position, where the parent is live too
    let mut next: Vec<Option<usize>> = vec![None; live.len()];
    for (i, (k, _)) in live.iter().enumerate() {
        let Some(p) = parent_of(k) else { continue };
        match pos.get(&p) {
            Some(&j) => {
                next[i] = Some(j);
                out.insert(k.clone(), RefState::Attached);
            }
            None => {
                out.insert(k.clone(), RefState::Suspect(SuspectWhy::ContainerGone));
            }
        }
    }
    // cycles in the functional graph `next`: walk from each unvisited node, colouring it
    // with the walk's number; meeting the walk's own colour closes a cycle
    let mut colour: Vec<usize> = vec![0; live.len()];
    for start in 0..live.len() {
        if colour[start] != 0 {
            continue;
        }
        let walk = start + 1;
        let mut at = start;
        loop {
            colour[at] = walk;
            match next[at] {
                Some(j) if colour[j] == 0 => at = j,
                Some(j) if colour[j] == walk => {
                    // the cycle through j
                    let mut cycle = vec![j];
                    let mut c = next[j].expect("on a cycle");
                    while c != j {
                        cycle.push(c);
                        c = next[c].expect("on a cycle");
                    }
                    let cut = if planted_cycle_by_order() {
                        *cycle.iter().min().expect("non-empty")
                    } else {
                        *cycle
                            .iter()
                            .min_by(|a, b| live[**a].0.cmp(&live[**b].0))
                            .expect("non-empty")
                    };
                    out.insert(live[cut].0.clone(), RefState::Suspect(SuspectWhy::Cycle));
                    break;
                }
                _ => break,
            }
        }
    }
    out
}

/// The references a change detached: each reference in `refs` that was
/// [`RefState::Attached`] in `before` (the doc before the change) and isn't after `applied`
/// (the change's applied stream), with its state after. For the notices a change sends ("your
/// link's end was deleted"): a delete or drop of the element orphans it, a set to a value
/// `kind_ok` refuses makes it suspect; a restore or insert in the same change undoes a delete.
pub fn orphaned_by<W>(
    before: &View,
    applied: &[Applied<W>],
    refs: &[Ref],
    kind_ok: &dyn Fn(&Value) -> bool,
) -> Vec<(Ref, RefState)> {
    // each touched element's end state in this change: Some(value) live, None gone
    let mut end: HashMap<(&str, &Key), Option<&Value>> = HashMap::new();
    for a in applied {
        let Applied::List { root, ops } = a else {
            continue;
        };
        for o in ops {
            let after = match o.kind {
                ListOpKind::Delete | ListOpKind::Drop => None,
                ListOpKind::Insert | ListOpKind::Restore | ListOpKind::Set => {
                    match o.value_after.as_ref() {
                        Some(v) => Some(v),
                        None => continue,
                    }
                }
                ListOpKind::Move => continue,
            };
            if matches!(o.effect, crate::elem::Effect::Visible) {
                end.insert((root.as_str(), &o.key), after);
            }
        }
    }
    let mut out = Vec::new();
    for r in refs {
        let Some(after) = end.get(&(r.root.as_str(), &r.key)) else {
            continue;
        };
        if resolve(before, r, kind_ok) != RefState::Attached {
            continue;
        }
        let s = match after {
            None => RefState::Orphaned,
            Some(v) => state_of(v, kind_ok),
        };
        if s != RefState::Attached {
            out.push((r.clone(), s));
        }
    }
    out
}

/// The planted fault [`crate::differential::RefMutant::OrphanDropsLink`]: [`resolve_all`]
/// leaves orphaned references out.
#[cfg(any(test, feature = "differential"))]
fn planted_orphan_drops_link() -> bool {
    crate::differential::ref_mutant() == Some(crate::differential::RefMutant::OrphanDropsLink)
}

#[cfg(not(any(test, feature = "differential")))]
fn planted_orphan_drops_link() -> bool {
    false
}

/// The planted fault [`crate::differential::RefMutant::CycleByOrder`]: [`resolve_parents`]
/// cuts a cycle at its first element in the list's order, not its smallest key.
#[cfg(any(test, feature = "differential"))]
fn planted_cycle_by_order() -> bool {
    crate::differential::ref_mutant() == Some(crate::differential::RefMutant::CycleByOrder)
}

#[cfg(not(any(test, feature = "differential")))]
fn planted_cycle_by_order() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elem::{Effect, KeyedRoot, KeyedState, ListApplied};

    fn key(s: &str) -> Key {
        Key::new(s).unwrap()
    }

    fn view(live: &[(&str, &str)], hidden: &[&str]) -> View {
        View::new(KeyedState {
            roots: vec![
                (
                    "items".into(),
                    KeyedRoot::List {
                        live: live.iter().map(|(k, v)| (key(k), Value::str(*v))).collect(),
                        hidden: hidden.iter().map(|k| (key(k), Value::str("box"))).collect(),
                        places: Vec::new(),
                    },
                ),
                ("props".into(), KeyedRoot::Map(BTreeMap::new())),
            ],
        })
    }

    fn any(_: &Value) -> bool {
        true
    }

    fn boxes(v: &Value) -> bool {
        v.as_str() == Some("box")
    }

    #[test]
    fn states_of_one_reference() {
        let v = view(&[("a", "box"), ("b", "line")], &["c"]);
        let r = |k: &str| Ref::new("items", key(k));
        assert_eq!(resolve(&v, &r("a"), &boxes), RefState::Attached);
        assert_eq!(
            resolve(&v, &r("b"), &boxes),
            RefState::Suspect(SuspectWhy::KindChanged)
        );
        assert_eq!(resolve(&v, &r("b"), &any), RefState::Attached);
        assert_eq!(resolve(&v, &r("c"), &any), RefState::Orphaned, "deleted");
        assert_eq!(resolve(&v, &r("zz"), &any), RefState::Orphaned, "unknown");
        for root in ["props", "nothing"] {
            assert_eq!(
                resolve(&v, &Ref::new(root, key("a")), &any),
                RefState::Suspect(SuspectWhy::ContainerGone),
                "{root}"
            );
        }
    }

    #[test]
    fn resolve_all_keeps_every_reference_and_its_hint() {
        let v = view(&[("a", "box")], &["c"]);
        let refs = vec![
            Ref::new("items", key("a")),
            Ref::new("items", key("c")).with_hint("10,20"),
            Ref::new("gone", key("a")),
            Ref::new("items", key("a")).with_hint("again"),
        ];
        let got = resolve_all(&v, &refs, &any);
        assert_eq!(got.len(), refs.len(), "one entry per reference: {got:?}");
        assert_eq!(got[1].0.hint.as_deref(), Some("10,20"));
        let states: Vec<RefState> = got.iter().map(|(_, s)| *s).collect();
        assert_eq!(
            states,
            [
                RefState::Attached,
                RefState::Orphaned,
                RefState::Suspect(SuspectWhy::ContainerGone),
                RefState::Attached
            ]
        );
        for (r, s) in &got {
            assert_eq!(*s, resolve(&v, r, &any));
        }
    }

    fn parents<'a>(edges: &'a [(&'a str, &'a str)]) -> impl Fn(&Key) -> Option<Key> + 'a {
        move |k: &Key| {
            edges
                .iter()
                .find(|(c, _)| *c == k.as_str())
                .map(|(_, p)| key(p))
        }
    }

    #[test]
    fn parents_cycles_cut_at_the_smallest_key() {
        // d -> b -> c -> d is a cycle (cut at b); e -> d hangs off it; a has no parent;
        // f's parent is deleted; g is its own parent
        let edges = [
            ("d", "b"),
            ("b", "c"),
            ("c", "d"),
            ("e", "d"),
            ("f", "x"),
            ("g", "g"),
        ];
        let live = [
            ("e", ""),
            ("d", ""),
            ("a", ""),
            ("c", ""),
            ("b", ""),
            ("f", ""),
            ("g", ""),
        ];
        let v = view(&live, &["x"]);
        let got = resolve_parents(&v, "items", &parents(&edges));
        let want: BTreeMap<Key, RefState> = [
            ("b", RefState::Suspect(SuspectWhy::Cycle)),
            ("c", RefState::Attached),
            ("d", RefState::Attached),
            ("e", RefState::Attached),
            ("f", RefState::Suspect(SuspectWhy::ContainerGone)),
            ("g", RefState::Suspect(SuspectWhy::Cycle)),
        ]
        .into_iter()
        .map(|(k, s)| (key(k), s))
        .collect();
        assert_eq!(got, want);
        // the list's order changes nothing
        let mut rev = live;
        rev.reverse();
        assert_eq!(
            resolve_parents(&view(&rev, &["x"]), "items", &parents(&edges)),
            want
        );
    }

    fn op(kind: ListOpKind, k: &str, after: Option<&str>) -> ListApplied<()> {
        ListApplied {
            kind,
            key: key(k),
            write: (),
            index_before: None,
            index_after: None,
            value_before: None,
            value_after: after.map(Value::str),
            effect: Effect::Visible,
        }
    }

    #[test]
    fn orphaned_by_a_change() {
        let before = view(
            &[("a", "box"), ("b", "box"), ("c", "box"), ("d", "box")],
            &[],
        );
        let applied = vec![Applied::List {
            root: "items".into(),
            ops: vec![
                op(ListOpKind::Delete, "a", None),
                op(ListOpKind::Set, "b", Some("line")),
                op(ListOpKind::Delete, "c", None),
                op(ListOpKind::Restore, "c", Some("box")),
                op(ListOpKind::Move, "d", None),
            ],
        }];
        let refs: Vec<Ref> = ["a", "b", "c", "d", "zz"]
            .iter()
            .map(|k| Ref::new("items", key(k)).with_hint(*k))
            .collect();
        let got = orphaned_by(&before, &applied, &refs, &boxes);
        assert_eq!(
            got,
            vec![
                (refs[0].clone(), RefState::Orphaned),
                (refs[1].clone(), RefState::Suspect(SuspectWhy::KindChanged)),
            ]
        );
    }
}
