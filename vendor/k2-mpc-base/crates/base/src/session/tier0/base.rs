//! Per-node `--base V` on a file type's own verbs (`docs/SESSIONS.md`, "Verbs on a blob doc",
//! "`--base` per node"): "I read version V; apply my call to head if nothing it changes was
//! changed since V in a way that overlaps." The verb runs at V as well as at head, so a node
//! that left the call's selection since V is still seen.

use std::collections::{BTreeMap, BTreeSet};

use super::index_read::node_of;
use super::struct_index::Target;
use super::verbs::{check_plan, Ran};
use super::*;
use crate::index::read::{self as iread, NodeChange, NodeChangeKind};
use crate::index::{IndexView, KeyClass, NodeKey};
use crate::session::BaseConflict;
use crate::tools::ToolRunError;

macro_rules! planted {
    ($m:ident) => {
        plant_on!(
            crate::differential::tier0_verb_mutant()
                == Some(crate::differential::Tier0VerbMutant::$m)
        )
    };
}

/// The base side of a call: version `v`, its index, the verb's change there, and what changed
/// between `v` and head.
pub(super) struct BaseSide {
    pub(super) v: VersionId,
    /// The verb's change at `v` (`D_V`).
    d_v: Vec<NodeChange>,
    /// `v`'s index and head's, each node by key, for the weak-key check.
    view_v: IndexView,
    /// What changed between `v` and head (`Moved`).
    moved: Vec<NodeChange>,
}

/// What the base side found: go on at head, or the pack refused the call at `v`.
pub(super) enum BaseRun {
    Side(Box<BaseSide>),
    Refused {
        entry: &'static crate::errors::ErrorEntry,
        why: String,
        next: String,
    },
}

/// The node-level changes: either one overlaps any other change to the node.
fn node_level(c: &NodeChange) -> Option<NodeChangeKind> {
    use NodeChangeKind as K;
    [K::Added, K::Removed, K::Moved, K::Renamed, K::Content]
        .into_iter()
        .find(|k| c.changes.contains(k))
}

/// The field two changes to one node share (`None`: they don't overlap). Either change being
/// node-level overlaps (the field names it); text overlaps only text; props overlap when they
/// share an entry.
fn overlap(mine: &NodeChange, theirs: &NodeChange) -> Option<String> {
    if planted!(BaseNodeLevelOnly) {
        return Some(
            theirs
                .changes
                .iter()
                .next()
                .map_or("record", |k| k.as_str())
                .to_owned(),
        );
    }
    if let Some(k) = node_level(theirs).or_else(|| node_level(mine)) {
        return Some(k.as_str().to_owned());
    }
    let text = NodeChangeKind::Text;
    if mine.changes.contains(&text) && theirs.changes.contains(&text) {
        return Some("text".to_owned());
    }
    let props = NodeChangeKind::Props;
    if mine.changes.contains(&props) && theirs.changes.contains(&props) {
        return mine
            .props
            .iter()
            .find(|p| theirs.props.contains(p))
            .cloned();
    }
    None
}

impl BlobSession {
    /// The base side of a call guarded by `--base v` on head `t` (head's index `head`): `v`'s
    /// index (pending: `MPC-E713`; none: `MPC-E712`), the verb run at `v` (its change `D_V`),
    /// the outside saves in `(v, head]` (each must be [`SavePairing::Proven`]: else
    /// `MPC-E719`), and what changed between `v` and head.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn base_side(
        &self,
        v: VersionId,
        t: &Target,
        spec: &IndexSpec,
        head: &IndexView,
        verb: &BlobVerb,
        args: &[String],
        json: bool,
    ) -> std::result::Result<BaseRun, ToolRunError> {
        if planted!(BaseWholeDoc) {
            return Err(SessionError::Stale {
                expected: v,
                actual: t.version,
            }
            .into());
        }
        let vt = match self.index_target(Some(v)) {
            Ok(vt) => vt,
            Err(SessionError::Store(StoreError::NoVersion(..))) => {
                return Err(SessionError::Stale {
                    expected: v,
                    actual: t.version,
                }
                .into())
            }
            Err(e) => return Err(e.into()),
        };
        let view_v = self.load_view(&vt, spec)?;
        // the verb at V: what it would have changed on what the caller read
        let (d_v, touched) = if planted!(BaseHeadOnly) {
            (Vec::new(), Vec::new())
        } else {
            match self.run_verb_on(&vt, Some(&view_v), verb, args, json)? {
                Ran::Out(o) => {
                    let plan = o.plan.clone().unwrap_or_default();
                    let touched: Vec<String> =
                        o.touched.iter().chain(&o.created).cloned().collect();
                    if plan.is_empty() {
                        (Vec::new(), touched)
                    } else {
                        check_plan(&plan.spans, vt.ver.size, &self.config)?;
                        let removed: u64 = plan.spans.iter().map(|s| s.len).sum();
                        let inserted: u64 = plan.spans.iter().map(|s| s.with.len() as u64).sum();
                        let size_after = vt.ver.size - removed + inserted;
                        let (_, d, _) =
                            self.check_output(&vt, spec, &view_v, &plan, &o, size_after)?;
                        (d, touched)
                    }
                }
                Ran::Refused { entry, why, next } => {
                    return Ok(BaseRun::Refused { entry, why, next })
                }
            }
        };
        // the outside saves since V: only a writer the pack proves keeps keys may lie between
        if !planted!(PairingAssumed) {
            let versions = t
                .store
                .blob_versions(self.doc_id)
                .map_err(SessionError::from)?;
            let crossings = self.crossings(spec, &t.store, &t.space, &versions, v, t.version)?;
            if let Some(c) = crossings
                .iter()
                .find(|c| c.pairing != Some(SavePairing::Proven))
            {
                let mut addresses: Vec<String> =
                    d_v.iter().map(|c| c.key.as_str().to_owned()).collect();
                addresses.extend(touched);
                addresses.sort();
                addresses.dedup();
                return Err(SessionError::BaseAcrossUnprovenSave {
                    save: c.save,
                    base: v,
                    head: t.version,
                    addresses,
                }
                .into());
            }
        }
        // what changed between V and head (the planted fault: V read for head)
        let moved = if planted!(PlanOnStaleHead) {
            Vec::new()
        } else {
            iread::diff(&view_v, head)
        };
        Ok(BaseRun::Side(Box::new(BaseSide {
            v,
            d_v,
            view_v,
            moved,
        })))
    }

    /// The per-node check of a `--base` call: each node the verb changes at V or at head
    /// (`D_V ∪ D_H`), but those it creates, that was changed since V in a way that overlaps
    /// refuses `MPC-E723`, naming each with the field, who changed it and in which version; then
    /// a node whose key the kit minted refuses `MPC-E724`.
    pub(super) fn check_base(
        &self,
        b: &BaseSide,
        t: &Target,
        spec: &IndexSpec,
        head: &IndexView,
        d_h: &[NodeChange],
    ) -> Result<()> {
        // keys the verb creates (absent from V at V, or from head at head) never conflict
        let created: BTreeSet<&NodeKey> = if planted!(CreatedKeysConflict) {
            BTreeSet::new()
        } else {
            b.d_v
                .iter()
                .chain(d_h)
                .filter(|c| c.changes.contains(&NodeChangeKind::Added))
                .map(|c| &c.key)
                .collect()
        };
        let moved: BTreeMap<&NodeKey, &NodeChange> = b.moved.iter().map(|c| (&c.key, c)).collect();
        let mut seen = BTreeSet::new();
        let mut found: Vec<(NodeKey, String)> = Vec::new();
        for c in b.d_v.iter().chain(d_h) {
            if created.contains(&c.key) || !seen.insert(&c.key) {
                continue;
            }
            // both sides' changes to this node, then against what changed since V
            let mine = merged(&c.key, &b.d_v, d_h);
            if let Some(m) = moved.get(&c.key) {
                if let Some(field) = overlap(&mine, m) {
                    found.push((c.key.clone(), field));
                }
            }
        }
        if !found.is_empty() && !planted!(BaseIgnored) {
            let conflicts = self.who_changed(b.v, t, spec, &found)?;
            let addresses = conflicts.iter().map(|c| c.address.clone()).collect();
            return Err(SessionError::BaseMoved {
                base: b.v,
                head: t.version,
                addresses,
                conflicts,
            });
        }
        if planted!(WeakKeyGuarded) {
            return Ok(());
        }
        let minted = |k: &NodeKey| {
            [&b.view_v, head]
                .iter()
                .filter_map(|v| node_of(v, k))
                .any(|n| matches!(n.key_class, KeyClass::Minted { .. }))
        };
        let mut weak: Vec<String> = b
            .d_v
            .iter()
            .chain(d_h)
            .map(|c| &c.key)
            .filter(|&k| !created.contains(k) && minted(k))
            .map(|k| k.as_str().to_owned())
            .collect();
        weak.sort();
        weak.dedup();
        if !weak.is_empty() {
            return Err(SessionError::BaseWeakKey {
                head: t.version,
                keys: weak,
            });
        }
        Ok(())
    }

    /// For each conflicting node, the newest version in `(v, head]` that changed it, and its
    /// author (head's when no stored index shows it).
    fn who_changed(
        &self,
        v: VersionId,
        t: &Target,
        spec: &IndexSpec,
        found: &[(NodeKey, String)],
    ) -> Result<Vec<BaseConflict>> {
        let versions = t.store.blob_versions(self.doc_id)?;
        let mut views: Vec<(VersionId, &BlobVersion, Option<IndexView>)> = Vec::new();
        for (vid, ver) in versions
            .iter()
            .filter(|(vid, _)| *vid >= v && *vid <= t.version)
        {
            let target = Target {
                store: t.store.clone(),
                space: t.space.clone(),
                version: *vid,
                ver: ver.clone(),
            };
            views.push((*vid, ver, self.load_view(&target, spec).ok()));
        }
        let mut out = Vec::new();
        for (key, field) in found {
            let mut by = (t.version, t.ver.author.clone());
            let mut prev: Option<&IndexView> = None;
            for (vid, ver, view) in &views {
                let Some(view) = view.as_ref() else {
                    continue;
                };
                if let Some(p) = prev {
                    if changed(p, view, key) {
                        by = (*vid, ver.author.clone());
                    }
                }
                prev = Some(view);
            }
            out.push(BaseConflict {
                address: key.as_str().to_owned(),
                key: key.as_str().to_owned(),
                field: field.clone(),
                by: by.1,
                change: by.0,
            });
        }
        Ok(out)
    }
}

/// One node's changes at V and at head, as one change (their kinds and props together).
fn merged(key: &NodeKey, d_v: &[NodeChange], d_h: &[NodeChange]) -> NodeChange {
    let mut it = d_v.iter().chain(d_h).filter(|c| &c.key == key);
    let mut m = it.next().expect("the node's own change").clone();
    for c in it {
        m.changes.extend(c.changes.iter().copied());
        for p in &c.props {
            if !m.props.contains(p) {
                m.props.push(p.clone());
            }
        }
    }
    m
}

/// Whether the node `key` differs between two indexes (came, went, or another record, name,
/// parent, content or text).
fn changed(a: &IndexView, b: &IndexView, key: &NodeKey) -> bool {
    match (node_of(a, key), node_of(b, key)) {
        (Some(x), Some(y)) => {
            x.record_hash != y.record_hash
                || x.parent != y.parent
                || x.name != y.name
                || x.content_hash != y.content_hash
                || x.text != y.text
        }
        (None, None) => false,
        _ => true,
    }
}
