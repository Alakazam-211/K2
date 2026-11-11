//! Minting keys, and carrying them across versions: a node the file gives no unique ID takes
//! a key once, and keeps it while it can be recognised. Tried in order against the
//! neighbouring version's minted nodes: **kind + name + parent path** (exactly one on each
//! side; a tie never pairs), then **position** (same parent path, kind and sibling order; a
//! weak match, an anchor reads it as suspect). Anything else mints a new key. A weak or failed
//! match never moves anything; it only changes what the node's key is here.

use std::collections::{HashMap, HashSet};

use super::row::{mint_needed, KeysRow, ReadyRow};
use crate::store::VersionId;

/// One row's nodes, each with its path interned against a table shared with the other row.
struct Paths {
    /// Node index → its path id (parent path id, kind, name).
    id: Vec<u32>,
    /// Node index → its parent's path id (`u32::MAX` at the root).
    parent: Vec<u32>,
}

fn intern_paths<'a>(row: &'a ReadyRow, table: &mut HashMap<(u32, &'a str, &'a str), u32>) -> Paths {
    let mut id: Vec<u32> = Vec::with_capacity(row.nodes.len());
    let mut parent = Vec::with_capacity(row.nodes.len());
    for n in &row.nodes {
        let p = n
            .parent
            .and_then(|i| id.get(i as usize).copied())
            .unwrap_or(u32::MAX);
        let next = table.len() as u32;
        let this = *table
            .entry((p, n.kind.as_str(), n.name.as_str()))
            .or_insert(next);
        id.push(this);
        parent.push(p);
    }
    Paths { id, parent }
}

/// The minted keys for version `version` of a row: carried from `neighbour` (a version of the
/// same doc, with its keys) where a node can be recognised, new otherwise. New keys are
/// `r<version>.<n>`.
pub(crate) fn mint_keys(
    version: VersionId,
    row: &ReadyRow,
    neighbour: Option<(&ReadyRow, &KeysRow)>,
    pack: &str,
    format: u32,
) -> KeysRow {
    let needed = mint_needed(row);
    let mut out = KeysRow::empty(pack, format);
    if needed.is_empty() {
        return out;
    }
    if let Some((prev, prev_keys)) = neighbour {
        let mut table = HashMap::new();
        let now_paths = intern_paths(row, &mut table);
        let prev_paths = intern_paths(prev, &mut table);
        let prev_minted: Vec<usize> = prev_keys.minted.keys().map(|k| *k as usize).collect();
        let now_set: HashSet<usize> = needed.iter().copied().collect();

        let mut now_at: HashMap<u32, Vec<usize>> = HashMap::new();
        for &i in &needed {
            now_at.entry(now_paths.id[i]).or_default().push(i);
        }
        let mut prev_at: HashMap<u32, Vec<usize>> = HashMap::new();
        for &j in &prev_minted {
            if let Some(&id) = prev_paths.id.get(j) {
                prev_at.entry(id).or_default().push(j);
            }
        }
        let mut used: HashSet<usize> = HashSet::new();
        // kind + name + parent path: exactly one on each side
        for (id, is) in &now_at {
            if let (1, Some(js)) = (is.len(), prev_at.get(id)) {
                if let (1, Some(&j)) = (js.len(), js.first()) {
                    if used.insert(j) {
                        let key = prev_keys.minted[&(j as u32)].0.clone();
                        out.minted.insert(is[0] as u32, (key, false));
                    }
                }
            }
        }
        // position: same parent path, kind and sibling order; still exactly one on each side
        let pos_key = |p: u32, kind: &str, order: u32| (p, kind.to_owned(), order);
        let mut prev_pos: HashMap<(u32, String, u32), Vec<usize>> = HashMap::new();
        for &j in &prev_minted {
            if used.contains(&j) {
                continue;
            }
            let Some(&p) = prev_paths.parent.get(j) else {
                continue;
            };
            let n = &prev.nodes[j];
            prev_pos
                .entry(pos_key(p, &n.kind, n.order))
                .or_default()
                .push(j);
        }
        let mut now_pos: HashMap<(u32, String, u32), Vec<usize>> = HashMap::new();
        for &i in now_set.iter() {
            if out.minted.contains_key(&(i as u32)) {
                continue;
            }
            let n = &row.nodes[i];
            now_pos
                .entry(pos_key(now_paths.parent[i], &n.kind, n.order))
                .or_default()
                .push(i);
        }
        for (k, is) in &now_pos {
            if is.len() != 1 {
                continue;
            }
            if let Some(js) = prev_pos.get(k) {
                if js.len() == 1 && used.insert(js[0]) {
                    let key = prev_keys.minted[&(js[0] as u32)].0.clone();
                    out.minted.insert(is[0] as u32, (key, true));
                }
            }
        }
    }
    let mut n = 0u64;
    for i in needed {
        if out.minted.contains_key(&(i as u32)) {
            continue;
        }
        n += 1;
        out.minted
            .insert(i as u32, (format!("r{version}.{n}"), false));
    }
    out
}
