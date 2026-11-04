//! Undo for a caller with no stack in the crate: the group that takes one board back to another.

use crate::json::{Fields, Value};
use crate::model::{Board, Id, Object};
use crate::ops::{Group, Op, Position};
use crate::refusal::Refusal;
use std::collections::{BTreeMap, BTreeSet, HashMap};

fn walk_to<F: FnMut(&Id, Position)>(current: &mut Vec<Id>, wanted: &[Id], mut mv: F) {
    for (i, id) in wanted.iter().enumerate() {
        if current.get(i) == Some(id) {
            continue;
        }
        let to = if i == 0 {
            Position::Back
        } else {
            Position::After(vec![wanted[i - 1].clone()])
        };
        mv(id, to);
        if let Some(from) = current.iter().position(|x| x == id) {
            current.remove(from);
        }
        current.insert(i, id.clone());
    }
}

/// The group that takes `after` back to `before`, for an applied group's undo. Every op names a
/// revision of `after`, so if someone has changed an object since, it conflicts and changes nothing.
///
/// It restores objects with their locks, z-order, presentation order and settings. A person added
/// stays in `people`: there's no op to remove one, and a token nothing uses does no harm. Top-level
/// unknown fields aren't restored either: no op writes them.
pub fn inverse(before: &Board, after: &Board, group_id: &str) -> Result<Group, Refusal> {
    let was: HashMap<&Id, &Object> = before.objects.iter().map(|o| (&o.id, o)).collect();
    let now: HashMap<&Id, &Object> = after.objects.iter().map(|o| (&o.id, o)).collect();

    let mut unlock = Vec::new();
    let mut restore = Vec::new();
    let mut dels = Vec::new();
    let mut adds = Vec::new();
    let mut moves = Vec::new();
    let mut relock: BTreeMap<Id, Option<Value>> = BTreeMap::new();

    // The order, with deletes and adds done: adds land on top, then everything moves into place.
    let mut objects: Vec<Id> = after
        .objects
        .iter()
        .filter(|o| was.contains_key(&o.id))
        .map(|o| o.id.clone())
        .collect();
    objects.extend(
        before
            .objects
            .iter()
            .filter(|o| !now.contains_key(&o.id))
            .map(|o| o.id.clone()),
    );
    let wanted: Vec<Id> = before.objects.iter().map(|o| o.id.clone()).collect();
    let mut moved: BTreeSet<Id> = BTreeSet::new();
    walk_to(&mut objects, &wanted, |id, to| {
        moved.insert(id.clone());
        moves.push(Op::Z { id: id.clone(), to });
    });
    let mut frames: Vec<Id> = after
        .frames
        .iter()
        .filter(|f| before.frames.contains(f))
        .cloned()
        .collect();
    frames.extend(
        before
            .frames
            .iter()
            .filter(|f| !after.frames.contains(f))
            .cloned(),
    );
    walk_to(&mut frames, &before.frames, |id, to| {
        moved.insert(id.clone());
        moves.push(Op::FrameOrder { id: id.clone(), to });
    });

    let locked = |o: &Object| matches!(o.fields.get("locked"), Some(Value::Bool(true)));

    for o in &after.objects {
        let rev = o.rev()?;
        let Some(old) = was.get(&o.id) else {
            // Added by the group: it goes.
            if locked(o) {
                unlock.push(Op::Set {
                    id: o.id.clone(),
                    rev,
                    set: one_lock(false),
                    unset: Vec::new(),
                });
            }
            dels.push(Op::Del {
                id: o.id.clone(),
                rev,
            });
            continue;
        };
        let differs = **old != *o;
        if !differs && !moved.contains(&o.id) {
            continue;
        }
        if locked(o) {
            unlock.push(Op::Set {
                id: o.id.clone(),
                rev,
                set: one_lock(false),
                unset: Vec::new(),
            });
            relock.insert(o.id.clone(), old.fields.get("locked").cloned());
        } else if old.fields.get("locked") != o.fields.get("locked") {
            relock.insert(o.id.clone(), old.fields.get("locked").cloned());
        }
        let mut names: BTreeSet<&String> = o.fields.keys().collect();
        names.extend(old.fields.keys());
        let mut clear = Vec::new();
        let mut set = Fields::new();
        for name in names {
            if name == "locked" || o.fields.get(name) == old.fields.get(name) {
                continue;
            }
            // A list that merges by id would keep sub-fields, so the field is cleared first.
            clear.push(name.clone());
            if let Some(v) = old.fields.get(name) {
                set.insert(name.clone(), v.clone());
            }
        }
        if !clear.is_empty() {
            restore.push(Op::Set {
                id: o.id.clone(),
                rev,
                set: Fields::new(),
                unset: clear,
            });
        }
        if !set.is_empty() {
            restore.push(Op::Set {
                id: o.id.clone(),
                rev,
                set,
                unset: Vec::new(),
            });
        }
    }

    for o in &before.objects {
        if !now.contains_key(&o.id) {
            let mut object = (*o).clone();
            // Moves of a locked object refuse, so the lock goes back on last.
            if let Some(lock) = object.fields.remove("locked") {
                relock.insert(o.id.clone(), Some(lock));
            }
            adds.push(Op::Add {
                object,
                at: Position::Front,
            });
        }
    }

    let mut ops = unlock;
    ops.extend(restore);
    ops.extend(dels);
    ops.extend(adds);
    ops.extend(moves);

    let rev_of = |id: &Id| -> Result<crate::hash::Sha256, Refusal> {
        match now.get(id).or_else(|| was.get(id)) {
            Some(o) => o.rev(),
            None => Ok(crate::hash::Sha256::of(b"")),
        }
    };
    for (id, lock) in relock {
        let rev = rev_of(&id)?;
        match lock {
            Some(v) => ops.push(Op::Set {
                id,
                rev,
                set: [("locked".to_string(), v)].into_iter().collect(),
                unset: Vec::new(),
            }),
            None => ops.push(Op::Set {
                id,
                rev,
                set: Fields::new(),
                unset: vec!["locked".to_string()],
            }),
        }
    }

    if before.settings != after.settings {
        let mut unset = Vec::new();
        for name in after.settings.keys() {
            if !before.settings.contains_key(name) {
                unset.push(name.clone());
            }
        }
        ops.push(Op::Meta {
            rev: after.settings_rev()?,
            set: before.settings.clone(),
            unset,
        });
    }

    Ok(Group {
        id: group_id.to_string(),
        by: None,
        ops,
    })
}

fn one_lock(v: bool) -> Fields {
    [("locked".to_string(), Value::Bool(v))]
        .into_iter()
        .collect()
}
