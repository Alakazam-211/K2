//! The first `merge_order`, with its scans, kept here for one thing: to hold the shipped one to the
//! same results. It isn't shipped.

use k2_mpc_board::*;
use std::collections::{BTreeSet, HashMap};

pub fn stable<'a>(base: &[&'a Id], list: &[&'a Id]) -> BTreeSet<&'a Id> {
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
pub fn merge_order(
    base: &[&Id],
    ours: &[&Id],
    theirs: &[&Id],
    keep: &BTreeSet<&Id>,
    notes: &mut Vec<(Id, bool)>,
) -> Vec<Id> {
    let in_base: BTreeSet<&Id> = base.iter().copied().collect();
    let ours_stable = stable(base, ours);
    let theirs_stable = stable(base, theirs);
    let mut result: Vec<Id> = theirs
        .iter()
        .filter(|id| keep.contains(**id))
        .map(|id| (*id).clone())
        .collect();
    let mut placed: BTreeSet<Id> = result.iter().cloned().collect();
    // What ours has above everything that stayed put, it wants on top.
    let top_from = ours
        .iter()
        .rposition(|id| keep.contains(*id) && ours_stable.contains(*id))
        .map_or(0, |i| i + 1);
    for (i, id) in ours.iter().enumerate() {
        if !keep.contains(*id) {
            continue;
        }
        let added = !in_base.contains(*id);
        let moved = !added && !ours_stable.contains(*id);
        if !added && !moved {
            continue;
        }
        if placed.remove(*id) {
            if let Some(from) = result.iter().position(|x| x == *id) {
                result.remove(from);
            }
        }
        let at = if i >= top_from {
            result.len()
        } else {
            ours[..i]
                .iter()
                .rev()
                .find(|below| placed.contains(**below))
                .and_then(|below| result.iter().position(|x| x == *below))
                .map_or(0, |p| p + 1)
        };
        result.insert(at, (*id).clone());
        placed.insert((*id).clone());
        if moved {
            notes.push((
                (*id).clone(),
                in_base.contains(*id) && theirs.contains(id) && !theirs_stable.contains(*id),
            ));
        }
    }
    result
}
