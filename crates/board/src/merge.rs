//! A three-way merge of whole objects: a copy of a board, the board it started from, and the file as
//! it is now. It's for a copy that went away and came back, and for a save whose file moved on.
//!
//! **The limit.** One writer at a time, with no presence. A merge works on whole objects: when one
//! object changed on both sides, that's a typed conflict with both edits kept, and nothing is merged
//! at all. It's never last-writer-wins, and it never merges inside an object. Order is the one
//! exception, as everywhere in this format: a move never conflicts, and ours stands.

use crate::hash::Sha256;
use crate::json::{Fields, Value};
use crate::limits::Limits;
use crate::model::{Board, Id, Object, Person, RefState, Reference};
use crate::ops::OrderList;
use crate::package::Manifest;
use crate::refusal::Refusal;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

/// Why one object can't merge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    /// Changed on both sides, and not the same.
    BothChanged,
    /// We deleted it; they changed it.
    OursDeleted,
    /// They deleted it; we changed it.
    TheirsDeleted,
    /// Added on both sides with one id, and not the same.
    BothAdded,
}

impl ConflictKind {
    /// A stable string.
    pub fn as_str(self) -> &'static str {
        match self {
            ConflictKind::BothChanged => "both_changed",
            ConflictKind::OursDeleted => "ours_deleted",
            ConflictKind::TheirsDeleted => "theirs_deleted",
            ConflictKind::BothAdded => "both_added",
        }
    }
}

/// One object that can't merge, with every side kept.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectConflict {
    /// The object.
    pub id: Id,
    /// Why.
    pub kind: ConflictKind,
    /// As it started, if it was there.
    pub base: Option<Object>,
    /// Ours, if it's there.
    pub ours: Option<Object>,
    /// Theirs, if it's there.
    pub theirs: Option<Object>,
}

/// The settings, changed on both sides.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsConflict {
    /// As they started.
    pub base: Fields,
    /// Ours.
    pub ours: Fields,
    /// Theirs.
    pub theirs: Fields,
}

/// One token holding two different people.
#[derive(Clone, Debug, PartialEq)]
pub struct PersonConflict {
    /// The token.
    pub token: Id,
    /// As it started, if it was there.
    pub base: Option<Person>,
    /// Ours.
    pub ours: Person,
    /// Theirs.
    pub theirs: Person,
}

/// A top-level unknown field changed on both sides.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtraConflict {
    /// Its name.
    pub name: String,
    /// As it started, if it was there.
    pub base: Option<Value>,
    /// Ours, if it's there.
    pub ours: Option<Value>,
    /// Theirs, if it's there.
    pub theirs: Option<Value>,
}

/// The most conflicts a [`MergeConflicts`] keeps in full. Past it, each conflict is only counted.
pub const MERGE_CONFLICTS_MAX: usize = 256;

/// How many conflicts there were of each kind: every one, whether it's kept in full or not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConflictTotals {
    /// The settings: 0 or 1.
    pub settings: u64,
    /// Top-level unknown fields.
    pub extra: u64,
    /// People.
    pub people: u64,
    /// Objects.
    pub objects: u64,
}

impl ConflictTotals {
    /// Every conflict, of every kind.
    pub fn all(&self) -> u64 {
        self.settings + self.extra + self.people + self.objects
    }
}

/// Everything that doesn't merge, all at once: one pass counts it all. Nothing has been merged, and
/// both sides stay as they are.
///
/// The first [`MERGE_CONFLICTS_MAX`] conflicts are kept in full, with every side's values, in this
/// order: the settings, then top-level unknown fields by name, then people by token, then objects by
/// id. `totals` counts every conflict of each kind, and `truncated` says some were counted but not
/// kept.
///
/// It's modelled on a refusal's `bad_ids`, `bad_ids_total` and `bad_ids_truncated`, with two
/// differences. It counts four kinds, each with its own total, in one stated order. And it counts as
/// the merge runs, so past the cap nothing of a conflict is held, where `bad_ids` gathers every id it
/// finds and cuts the list at the end.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MergeConflicts {
    /// Objects, in id order.
    pub objects: Vec<ObjectConflict>,
    /// The settings.
    pub settings: Option<SettingsConflict>,
    /// People, in token order.
    pub people: Vec<PersonConflict>,
    /// Top-level unknown fields, in name order.
    pub extra: Vec<ExtraConflict>,
    /// Every conflict, counted by kind.
    pub totals: ConflictTotals,
    /// Whether the lists stop short of the totals.
    pub truncated: bool,
}

impl MergeConflicts {
    /// How many conflicts are kept in full.
    pub fn kept(&self) -> usize {
        usize::from(self.settings.is_some())
            + self.extra.len()
            + self.people.len()
            + self.objects.len()
    }

    /// Whether one more conflict, already counted, is kept in full. Past the cap it's only counted, and
    /// nothing of it is cloned.
    fn room(&mut self) -> bool {
        if self.kept() < MERGE_CONFLICTS_MAX {
            return true;
        }
        self.truncated = true;
        false
    }
}

impl fmt::Display for MergeConflicts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let t = &self.totals;
        write!(
            f,
            "{} conflicts: {} objects, {} people and {} other things changed on both sides, and \
nothing was merged: \
one writer at a time, whole objects, never last-writer-wins, and both edits are kept. \
{} of {} are shown in full",
            t.all(),
            t.objects,
            t.people,
            t.settings + t.extra,
            self.kept(),
            t.all()
        )
    }
}

/// Why a merge gave no board.
#[derive(Clone, Debug, PartialEq)]
pub enum MergeError {
    /// The same thing changed on both sides.
    Conflicts(Box<MergeConflicts>),
    /// The result wouldn't be valid, typed.
    Refused(Refusal),
}

impl From<Refusal> for MergeError {
    fn from(r: Refusal) -> MergeError {
        MergeError::Refused(r)
    }
}

/// A position ours chose.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderNote {
    /// What moved.
    pub id: Id,
    /// In which list.
    pub list: OrderList,
    /// True when theirs moved it too, so ours stands over theirs.
    pub over_theirs: bool,
}

/// A clean merge.
#[derive(Clone, Debug, PartialEq)]
pub struct Merged {
    /// The board.
    pub board: Board,
    /// Objects taken from ours: changed or added there.
    pub from_ours: Vec<Id>,
    /// Objects taken from theirs: changed or added there.
    pub from_theirs: Vec<Id>,
    /// Objects gone, from either side.
    pub deleted: Vec<Id>,
    /// Moves of ours that stand.
    pub moved: Vec<OrderNote>,
    /// References with no target that the base didn't have: a target went, and what named it stayed.
    /// They're kept, and a reader computes their state.
    pub orphans: Vec<Reference>,
}

fn index(b: &Board) -> HashMap<&Id, &Object> {
    b.objects.iter().map(|o| (&o.id, o)).collect()
}

/// Which of `list` kept their order against `base`: the longest increasing run of base positions.
fn stable<'a>(base: &[&'a Id], list: &[&'a Id]) -> BTreeSet<&'a Id> {
    let at: HashMap<&Id, usize> = base.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let items: Vec<(usize, &'a Id)> = list
        .iter()
        .filter_map(|id| at.get(*id).map(|p| (*p, *id)))
        .collect();
    let mut tails: Vec<usize> = Vec::new(); // index into items, by run length
    let mut back: Vec<Option<usize>> = vec![None; items.len()];
    for i in 0..items.len() {
        let slot = tails.partition_point(|t| items[*t].0 < items[i].0);
        back[i] = slot.checked_sub(1).map(|s| tails[s]);
        if slot == tails.len() {
            tails.push(i);
        } else {
            tails[slot] = i;
        }
    }
    let mut out = BTreeSet::new();
    let mut walk = tails.last().copied();
    while let Some(i) = walk {
        out.insert(items[i].1);
        walk = back[i];
    }
    out
}

/// A list that takes a move in constant time. Slot 0 is the place before the first and after the
/// last. It runs under a host's writer lock, on a file anyone could have built, so no step scans.
struct Chain<'a> {
    slots: HashMap<&'a Id, usize>,
    ids: Vec<Option<&'a Id>>,
    prev: Vec<usize>,
    next: Vec<usize>,
}

impl<'a> Chain<'a> {
    fn new() -> Chain<'a> {
        Chain {
            slots: HashMap::new(),
            ids: vec![None],
            prev: vec![0],
            next: vec![0],
        }
    }

    fn contains(&self, id: &Id) -> bool {
        self.slots.contains_key(id)
    }

    fn insert_after(&mut self, at: usize, id: &'a Id) {
        let slot = self.ids.len();
        let after = self.next[at];
        self.ids.push(Some(id));
        self.prev.push(at);
        self.next.push(after);
        self.next[at] = slot;
        self.prev[after] = slot;
        self.slots.insert(id, slot);
    }

    fn push_back(&mut self, id: &'a Id) {
        self.insert_after(self.prev[0], id);
    }

    fn remove(&mut self, id: &Id) {
        if let Some(slot) = self.slots.remove(id) {
            let (before, after) = (self.prev[slot], self.next[slot]);
            self.next[before] = after;
            self.prev[after] = before;
        }
    }

    fn into_list(self) -> Vec<Id> {
        let mut out = Vec::with_capacity(self.slots.len());
        let mut at = self.next[0];
        while at != 0 {
            if let Some(id) = self.ids[at] {
                out.push(id.clone());
            }
            at = self.next[at];
        }
        out
    }
}

/// Merges one order: theirs as it stands, with what's gone taken out, then what ours moved or added
/// put directly above its nearest neighbour below in ours. If none is there, the bottom. What ours
/// has on top stays on top.
fn merge_order(
    base: &[&Id],
    ours: &[&Id],
    theirs: &[&Id],
    keep: &BTreeSet<&Id>,
    list: OrderList,
    notes: &mut Vec<OrderNote>,
) -> Vec<Id> {
    let in_base: HashSet<&Id> = base.iter().copied().collect();
    let in_theirs: HashSet<&Id> = theirs.iter().copied().collect();
    let ours_stable = stable(base, ours);
    let theirs_stable = stable(base, theirs);
    let mut result = Chain::new();
    for id in theirs {
        if keep.contains(*id) {
            result.push_back(id);
        }
    }
    // What ours has above everything that stayed put, it wants on top.
    let top_from = ours
        .iter()
        .rposition(|id| keep.contains(*id) && ours_stable.contains(*id))
        .map_or(0, |i| i + 1);
    // The nearest one below in ours that's in the list by now.
    let mut below: Option<&Id> = None;
    for (i, id) in ours.iter().enumerate() {
        if !keep.contains(*id) {
            continue;
        }
        let added = !in_base.contains(*id);
        let moved = !added && !ours_stable.contains(*id);
        if added || moved {
            result.remove(id);
            let at = if i >= top_from {
                result.prev[0]
            } else {
                below
                    .and_then(|b| result.slots.get(b).copied())
                    .unwrap_or(0)
            };
            result.insert_after(at, id);
            if moved {
                notes.push(OrderNote {
                    id: (*id).clone(),
                    list,
                    over_theirs: in_theirs.contains(*id) && !theirs_stable.contains(*id),
                });
            }
        }
        if result.contains(id) {
            below = Some(*id);
        }
    }
    result.into_list()
}

fn three_way<T: PartialEq + Clone>(
    base: Option<&T>,
    ours: Option<&T>,
    theirs: Option<&T>,
) -> Option<Option<T>> {
    if ours == theirs {
        return Some(ours.cloned());
    }
    if ours == base {
        return Some(theirs.cloned());
    }
    if theirs == base {
        return Some(ours.cloned());
    }
    None
}

/// What merging the objects gave, when nothing conflicted.
struct ObjectsMerged<'a> {
    objects: HashMap<&'a Id, Object>,
    from_ours: Vec<Id>,
    from_theirs: Vec<Id>,
    deleted: Vec<Id>,
}

fn merge_settings(
    base: &Board,
    ours: &Board,
    theirs: &Board,
    conflicts: &mut MergeConflicts,
) -> Fields {
    match three_way(
        Some(&base.settings),
        Some(&ours.settings),
        Some(&theirs.settings),
    ) {
        Some(Some(s)) => s,
        _ => {
            conflicts.totals.settings += 1;
            if conflicts.room() {
                conflicts.settings = Some(SettingsConflict {
                    base: base.settings.clone(),
                    ours: ours.settings.clone(),
                    theirs: theirs.settings.clone(),
                });
            }
            Fields::new()
        }
    }
}

fn merge_extra(
    base: &Board,
    ours: &Board,
    theirs: &Board,
    conflicts: &mut MergeConflicts,
) -> Fields {
    let mut extra = Fields::new();
    let names: BTreeSet<&String> = base
        .extra
        .keys()
        .chain(ours.extra.keys())
        .chain(theirs.extra.keys())
        .collect();
    for name in names {
        let merged: Option<Option<Value>> = three_way(
            base.extra.get(name),
            ours.extra.get(name),
            theirs.extra.get(name),
        );
        match merged {
            Some(Some(v)) => {
                extra.insert(name.clone(), v);
            }
            Some(None) => {}
            None => {
                conflicts.totals.extra += 1;
                if conflicts.room() {
                    conflicts.extra.push(ExtraConflict {
                        name: name.clone(),
                        base: base.extra.get(name).cloned(),
                        ours: ours.extra.get(name).cloned(),
                        theirs: theirs.extra.get(name).cloned(),
                    });
                }
            }
        }
    }
    extra
}

/// People merge by union: a token nobody removed stays, so what names it still means one person.
fn merge_people(
    base: &Board,
    ours: &Board,
    theirs: &Board,
    conflicts: &mut MergeConflicts,
) -> BTreeMap<Id, Person> {
    let mut people: BTreeMap<Id, Person> = BTreeMap::new();
    let tokens: BTreeSet<&Id> = base
        .people
        .keys()
        .chain(ours.people.keys())
        .chain(theirs.people.keys())
        .collect();
    for token in tokens {
        let (bp, op, tp) = (
            base.people.get(token),
            ours.people.get(token),
            theirs.people.get(token),
        );
        let merged = match (op, tp) {
            (Some(x), None) | (None, Some(x)) => Some(x.clone()),
            (None, None) => bp.cloned(),
            (Some(_), Some(_)) => three_way(bp, op, tp).flatten(),
        };
        match merged {
            Some(p) => {
                people.insert(token.clone(), p);
            }
            None => {
                if let (Some(x), Some(y)) = (op, tp) {
                    conflicts.totals.people += 1;
                    if conflicts.room() {
                        conflicts.people.push(PersonConflict {
                            token: token.clone(),
                            base: bp.cloned(),
                            ours: x.clone(),
                            theirs: y.clone(),
                        });
                    }
                }
            }
        }
    }
    people
}

fn merge_objects<'a>(
    base: &'a Board,
    ours: &'a Board,
    theirs: &'a Board,
    conflicts: &mut MergeConflicts,
) -> ObjectsMerged<'a> {
    let (b, o, t) = (index(base), index(ours), index(theirs));
    let mut ids: BTreeSet<&Id> = BTreeSet::new();
    ids.extend(b.keys().copied());
    ids.extend(o.keys().copied());
    ids.extend(t.keys().copied());

    let mut objects: HashMap<&Id, Object> = HashMap::new();
    let mut from_ours = Vec::new();
    let mut from_theirs = Vec::new();
    let mut deleted = Vec::new();
    for id in &ids {
        let (bo, oo, to) = (
            b.get(*id).copied(),
            o.get(*id).copied(),
            t.get(*id).copied(),
        );
        match three_way(bo, oo, to) {
            Some(Some(object)) => {
                if oo == Some(&object) && oo != bo {
                    from_ours.push((*id).clone());
                }
                if to == Some(&object) && to != bo && oo != to {
                    from_theirs.push((*id).clone());
                }
                objects.insert(*id, object);
            }
            Some(None) => {
                if bo.is_some() {
                    deleted.push((*id).clone());
                }
            }
            None => {
                let kind = match (bo, oo, to) {
                    (None, _, _) => ConflictKind::BothAdded,
                    (_, None, _) => ConflictKind::OursDeleted,
                    (_, _, None) => ConflictKind::TheirsDeleted,
                    _ => ConflictKind::BothChanged,
                };
                conflicts.totals.objects += 1;
                if conflicts.room() {
                    conflicts.objects.push(ObjectConflict {
                        id: (*id).clone(),
                        kind,
                        base: bo.cloned(),
                        ours: oo.cloned(),
                        theirs: to.cloned(),
                    });
                }
            }
        }
    }
    ObjectsMerged {
        objects,
        from_ours,
        from_theirs,
        deleted,
    }
}

/// Merges `ours` and `theirs`, both from `base`. If anything can't merge, it's all typed, and there's
/// no board: both sides are kept as they are, and the error holds the first
/// [`MERGE_CONFLICTS_MAX`] conflicts, in the order [`MergeConflicts`] states.
pub fn merge3(base: &Board, ours: &Board, theirs: &Board) -> Result<Merged, MergeError> {
    let mut conflicts = MergeConflicts::default();
    // The order the conflicts are kept in, so the first of them are always the same ones.
    let settings = merge_settings(base, ours, theirs, &mut conflicts);
    let extra = merge_extra(base, ours, theirs, &mut conflicts);
    let people = merge_people(base, ours, theirs, &mut conflicts);
    let merged = merge_objects(base, ours, theirs, &mut conflicts);

    if conflicts != MergeConflicts::default() {
        return Err(MergeError::Conflicts(Box::new(conflicts)));
    }

    let ids_of = |b: &'_ Board| -> Vec<Id> { b.objects.iter().map(|o| o.id.clone()).collect() };
    let (base_ids, ours_ids, theirs_ids) = (ids_of(base), ids_of(ours), ids_of(theirs));
    let objects = merged.objects;
    let keep: BTreeSet<&Id> = objects.keys().copied().collect();
    let mut moved = Vec::new();
    let order = merge_order(
        &base_ids.iter().collect::<Vec<_>>(),
        &ours_ids.iter().collect::<Vec<_>>(),
        &theirs_ids.iter().collect::<Vec<_>>(),
        &keep,
        OrderList::Objects,
        &mut moved,
    );
    let frames = merge_order(
        &base.frames.iter().collect::<Vec<_>>(),
        &ours.frames.iter().collect::<Vec<_>>(),
        &theirs.frames.iter().collect::<Vec<_>>(),
        &keep,
        OrderList::Frames,
        &mut moved,
    );
    let mut list = Vec::with_capacity(order.len());
    for id in &order {
        if let Some(object) = objects.get(id) {
            list.push(object.clone());
        }
    }
    let board = Board {
        settings,
        people,
        objects: list,
        frames,
        extra,
    };
    board.validate(&Limits::V1)?;

    let was_orphaned: BTreeSet<(Id, &'static str)> = base
        .references()
        .into_iter()
        .filter(|r| r.state == RefState::Orphaned)
        .map(|r| (r.from, r.field))
        .collect();
    let orphans = board
        .references()
        .into_iter()
        .filter(|r| r.state == RefState::Orphaned)
        .filter(|r| !was_orphaned.contains(&(r.from.clone(), r.field)))
        .collect();

    Ok(Merged {
        board,
        from_ours: merged.from_ours,
        from_theirs: merged.from_theirs,
        deleted: merged.deleted,
        moved,
        orphans,
    })
}

/// Whether a copy's manifest says it came from this board. A copy names what it came from by hash;
/// a merge against anything else would judge changes that nobody made.
pub fn base_matches(copy: &Manifest, base: &Board) -> Result<bool, Refusal> {
    let Some(named) = &copy.base else {
        return Ok(false);
    };
    Ok(named.sha256 == Sha256::of(&base.to_canonical_json()?))
}
