//! The structure index's typed reads (`docs/SESSIONS.md`, "Reading the index"):
//! what a host's views call, each with a connection, the rights asked on every call, and an
//! App viewer's handles. The verbs ([`BlobSession::run_tool`]) are a parser over these.
//!
//! Rights: read for the current version; read and history for any other version, a diff and
//! a blame (as `show --version`, `log` and `blame` are). Asked before anything is read, and
//! before a cursor is looked at.

use super::struct_index::Target;
use super::*;
use crate::index::read::NodeChangeKind;
use crate::index::read::{self, ReadRefusal, Walk};
use crate::index::{
    Identity, IndexBlame, IndexDiff, IndexView, ListQuery, NodeBlame, NodeKey, NodeLabel,
    NodesRead, Outline, SaveCrossing, BLAME_WALK_MAX,
};

/// The most addresses one node read takes (`show`, `claim --node`).
pub const INDEX_ADDRS_MAX: usize = 256;

/// An outside save a read crosses (`docs/SESSIONS.md`, "Across an outside save"): the save,
/// the indexes of the version before it and of the save (when both are there), and what the
/// pack says of its writer.
pub(super) struct Crossing {
    pub(super) save: VersionId,
    /// (before, after).
    pub(super) views: Option<(IndexView, IndexView)>,
    /// The version whose index is missing (the save, or the version before it).
    pub(super) missing: Option<VersionId>,
    /// `None`: not asked (an index is missing).
    pub(super) pairing: Option<SavePairing>,
}

/// Whether `v` is an outside save: another program's save taken in, or kept as a conflict
/// (never the kit's write, a verb's, a restore or a merge).
pub(super) fn is_outside_save(v: &BlobVersion) -> bool {
    v.reason == BlobReason::External
}

impl BlobSession {
    /// Admit `conn` for a structure read of `versions` (`None`: the current one): read, and
    /// history for a version that isn't current, or for every read when `history`. Returns
    /// the pack's index and one target per version, each version as of this call.
    pub(super) fn admit_index(
        &self,
        conn: ConnId,
        versions: &[Option<VersionId>],
        history: bool,
        action: Action,
    ) -> Result<(IndexSpec, Vec<Target>)> {
        let now = self.now();
        let mut st = self.lock()?;
        #[cfg(any(test, feature = "differential"))]
        let check = crate::differential::index_mutant()
            != Some(crate::differential::IndexMutant::ReadSkipsRights);
        #[cfg(not(any(test, feature = "differential")))]
        let check = true;
        if check {
            self.admit(&mut st, conn, Need::Read, action, now)?;
            let old = versions.iter().any(|v| v.is_some_and(|v| v != st.vid));
            if history || old {
                self.admit(&mut st, conn, Need::History, action, now)?;
            }
        }
        let spec = self
            .pack
            .index_spec()
            .ok_or(SessionError::IndexUnsupported)?;
        let targets = versions
            .iter()
            .map(|v| self.target_of(&st, Some(v.unwrap_or(st.vid))))
            .collect::<Result<Vec<_>>>()?;
        Ok((spec, targets))
    }

    /// One version's index, read for `conn` ([`BlobSession::admit_index`]).
    fn read_view(&self, conn: ConnId, version: Option<VersionId>) -> Result<IndexView> {
        let (spec, t) = self.admit_index(conn, &[version], false, Action::Read)?;
        self.load_view(&t[0], &spec)
    }

    /// The view index of a version (`None`: the current one), from its structure index through
    /// the pack's view schema ([`crate::view_index::ViewIndex::blob`]):
    /// `tier` blob, `version` set. Rights and refusals as [`BlobSession::index_list`]
    /// (read, and history for a version that isn't current; pending, not indexed). `Ok(None)`,
    /// with nothing read, when the pack has no view schema or the schema no blob mapping.
    pub fn view_index(
        &self,
        conn: ConnId,
        version: Option<VersionId>,
    ) -> Result<Option<crate::view_index::ViewIndex>> {
        if self.pack.view_schema().and_then(|s| s.blob).is_none() {
            return Ok(None);
        }
        let view = self.read_view(conn, version)?;
        Ok(crate::view_index::ViewIndex::blob(&*self.pack, &view))
    }

    /// **The outline** of a version (the `outline`): every node the query keeps, in
    /// outline order, each with its `#N`, key, kind, name and place, without text,
    /// attributes or hashes. `under` keeps the nodes below one node, by any address: one that
    /// names nothing is `MPC-E172`, one that names several `MPC-E170`
    /// ([`SessionError::IndexRead`]). Needs read (and history for a version that isn't current).
    /// Refused [`SessionError::IndexUnsupported`] (`MPC-E711`) for a type with no index,
    /// [`SessionError::IndexPending`] (`MPC-E713`, retry) before a job has run,
    /// [`SessionError::NotIndexed`] (`MPC-E712`) past a cap, unreadable or purged. Never
    /// paged: the verbs page ([`crate::index::READ_BUDGET`]).
    pub fn index_list(&self, conn: ConnId, q: &ListQuery) -> Result<Outline> {
        let out = self
            .read_view(conn, q.version)
            .and_then(|view| Walk::new(&view).outline(q).map_err(read_err));
        self.for_viewer(conn, out, |_, v| v)
    }

    /// **Nodes by address** (the `node`): one entry per address, in the order given,
    /// each the node in full with its ancestors and its descendants `depth` levels down; `None`
    /// for an address that names nothing, which `missing` lists with its candidates.
    /// The kit's one address grammar ([`crate::addr`]): a key (`@k:…`), a native ID, `#N` (in
    /// the version read: `NodesRead::version`), a name. A name or ID two nodes share refuses
    /// the read, `MPC-E170` with every holder as a candidate ([`SessionError::IndexRead`]);
    /// one that doesn't parse is [`SessionError::BadPosition`]. More than
    /// [`INDEX_ADDRS_MAX`] addresses: [`SessionError::BadPosition`]. Rights as
    /// [`BlobSession::index_list`].
    pub fn index_nodes(
        &self,
        conn: ConnId,
        addrs: &[String],
        version: Option<VersionId>,
        depth: u16,
    ) -> Result<NodesRead> {
        let out = self.index_nodes_inner(conn, addrs, version, depth);
        self.for_viewer(conn, out, |_, v| v)
    }

    fn index_nodes_inner(
        &self,
        conn: ConnId,
        addrs: &[String],
        version: Option<VersionId>,
        depth: u16,
    ) -> Result<NodesRead> {
        let view = self.read_view(conn, version)?;
        check_addrs(addrs)?;
        nodes_of(&Walk::new(&view), addrs, depth)
    }

    /// **What changed** from version `from` to version `to`, node by node: added, removed,
    /// moved, renamed, its properties, content or text ([`IndexDiff`]). Needs read and
    /// history. Both versions need an index ([`SessionError::IndexPending`] for the first
    /// that hasn't one yet).
    pub fn index_diff(&self, conn: ConnId, from: VersionId, to: VersionId) -> Result<IndexDiff> {
        let out = self.index_diff_inner(conn, from, to);
        self.for_viewer(conn, out, |_, v| v)
    }

    fn index_diff_inner(&self, conn: ConnId, from: VersionId, to: VersionId) -> Result<IndexDiff> {
        let (spec, t) = self.admit_index(conn, &[Some(from), Some(to)], true, Action::History)?;
        let a = self.load_view(&t[0], &spec)?;
        let b = self.load_view(&t[1], &spec)?;
        let mut changes = read::diff(&a, &b);
        let versions = t[1].store.blob_versions(self.doc_id)?;
        let crossings = self.crossings(&spec, &t[1].store, &t[1].space, &versions, from, to)?;
        // a save whose writer reissues keys: nothing pairs across it
        let reissued = crossings
            .iter()
            .rev()
            .find(|c| c.pairing == Some(SavePairing::Reissues));
        if let Some(c) = reissued.filter(|_| {
            !plant_on!(
                crate::differential::tier0_verb_mutant()
                    == Some(crate::differential::Tier0VerbMutant::ReissuesAnswered)
            )
        }) {
            return Err(SessionError::KeysReissued {
                save: c.save,
                safe_from: c.save,
                to,
            });
        }
        for ch in &mut changes {
            if ch.changes.contains(&NodeChangeKind::Added)
                || ch.changes.contains(&NodeChangeKind::Removed)
            {
                continue;
            }
            for c in &crossings {
                match (&c.views, c.pairing) {
                    (_, Some(SavePairing::Proven)) => {}
                    (Some((before, after)), _) => {
                        if let (Some(was), Some(now)) =
                            (node_of(before, &ch.key), node_of(after, &ch.key))
                        {
                            if let Some(judged) = self.judged_across(&was.fingerprint, now) {
                                ch.identity = Some(Identity::Doubt {
                                    save: c.save,
                                    judged,
                                });
                                break;
                            }
                        }
                    }
                    (None, _) => {
                        if ch.identity.is_none() {
                            ch.identity = c.missing.map(|v| Identity::Unchecked { not_indexed: v });
                        }
                    }
                }
            }
        }
        let uncounted = plant_on!(
            crate::differential::tier0_verb_mutant()
                == Some(crate::differential::Tier0VerbMutant::KeysKeptUncounted)
        );
        let saves = crossings
            .iter()
            .map(|c| SaveCrossing {
                version: c.save,
                pairing: c.pairing.map_or("unchecked", SavePairing::name),
                keys_kept: c
                    .views
                    .as_ref()
                    .filter(|_| !uncounted)
                    .map(|(before, after)| keys_kept(before, after)),
            })
            .collect();
        Ok(IndexDiff {
            from,
            to,
            changes,
            saves,
        })
    }

    /// The outside saves in `(from, to]` of `versions` (the doc's, oldest first), each with
    /// the indexes of the version before it and of the save, read from the stored rows, and
    /// what the pack says of its writer ([`BlobPack::save_pairing`]). An index that isn't
    /// there leaves the save unchecked.
    pub(super) fn crossings(
        &self,
        spec: &IndexSpec,
        store: &Arc<DocStore>,
        space: &str,
        versions: &[(VersionId, BlobVersion)],
        from: VersionId,
        to: VersionId,
    ) -> Result<Vec<Crossing>> {
        let load = |v: VersionId, ver: &BlobVersion| -> Result<Option<IndexView>> {
            let t = Target {
                store: store.clone(),
                space: space.to_owned(),
                version: v,
                ver: ver.clone(),
                next: v + 1,
            };
            match self.load_view(&t, spec) {
                Ok(view) => Ok(Some(view)),
                // the store failing is the read failing; any other miss leaves it unchecked
                Err(e @ SessionError::Store(_)) => Err(e),
                Err(_) => Ok(None),
            }
        };
        let mut out = Vec::new();
        for (k, (vid, ver)) in versions.iter().enumerate() {
            if k == 0 || *vid <= from || *vid > to || !is_outside_save(ver) {
                continue;
            }
            let (pvid, pver) = &versions[k - 1];
            let before = load(*pvid, pver)?;
            let after = load(*vid, ver)?;
            let missing = match (&before, &after) {
                (None, _) => Some(*pvid),
                (_, None) => Some(*vid),
                _ => None,
            };
            let (views, pairing) = match (before, after) {
                (Some(before), Some(after)) => {
                    let pairing = self.pack.save_pairing(&SaveFacts {
                        version: *vid,
                        kept_conflict: ver.conflict_copy.is_some(),
                        before: &before,
                        after: &after,
                    });
                    (Some((before, after)), Some(pairing))
                }
                _ => (None, None),
            };
            out.push(Crossing {
                save: *vid,
                views,
                missing,
                pairing,
            });
        }
        Ok(out)
    }

    /// The pack's judgment of a key across an outside save ([`BlobPack::judge_across_save`]):
    /// `was` the node just before the save, `now` the node in it. `kind_changed` or
    /// `identity_doubt` when in doubt.
    pub(super) fn judged_across(&self, was: &Fingerprint, now: &IndexNode) -> Option<&'static str> {
        judged_across(&*self.pack, was, now)
    }

    /// **Blame by node** (on the structure): for every node of version `version` (`None`:
    /// the current one), in outline order, the version that made it as it is and who set
    /// that version. It walks back through the doc's versions while the node is unchanged
    /// (its record, content, text, name, parent and fields; a shift among siblings is not a
    /// change), using only indexes that exist: a version with no index stops the walk
    /// (`stopped_at`, `missing`), as does [`BLAME_WALK_MAX`] versions, and the rows still open
    /// say `settled: false`. Needs read and history. The version's own index must exist.
    pub fn index_blame(&self, conn: ConnId, version: Option<VersionId>) -> Result<IndexBlame> {
        let out = self.index_blame_inner(conn, version);
        self.for_viewer(conn, out, |l, mut b| {
            for r in &mut b.rows {
                r.author = l.pid(&r.author);
                r.on_behalf_of = r.on_behalf_of.take().map(|p| l.pid(&p));
            }
            b
        })
    }

    fn index_blame_inner(&self, conn: ConnId, version: Option<VersionId>) -> Result<IndexBlame> {
        let (spec, mut t) = self.admit_index(conn, &[version], true, Action::Blame)?;
        let t = t.remove(0);
        let view = self.load_view(&t, &spec)?;
        let walk = Walk::new(&view);
        let versions = t.store.blob_versions(self.doc_id)?;
        let mut made: Vec<VersionId> = vec![t.version; view.nodes.len()];
        let mut open: Vec<usize> = walk.order.clone();
        let (mut stopped_at, mut missing) = (None, Vec::new());
        let mut newer_blob = t.ver.blob;
        let back = versions.iter().rev().filter(|(v, _)| *v < t.version);
        for (walked, (vid, ver)) in back.enumerate() {
            if open.is_empty() {
                break;
            }
            if walked == BLAME_WALK_MAX {
                stopped_at = Some(*vid);
                break;
            }
            // the same bytes as the version after it: nothing changed there
            if ver.blob == newer_blob && !ver.redacted {
                for &i in &open {
                    made[i] = *vid;
                }
                continue;
            }
            let older = Target {
                store: t.store.clone(),
                space: t.space.clone(),
                version: *vid,
                ver: ver.clone(),
                next: *vid + 1,
            };
            let prev = match self.load_view(&older, &spec) {
                Ok(p) => p,
                Err(SessionError::IndexPending { .. } | SessionError::NotIndexed { .. }) => {
                    stopped_at = Some(*vid);
                    missing.push(*vid);
                    break;
                }
                Err(e) => return Err(e),
            };
            let pw = Walk::new(&prev);
            open.retain(|&i| {
                let same = pw
                    .index_of(view.nodes[i].key.as_str())
                    .is_some_and(|j| read::unchanged(&prev.nodes[j], &view.nodes[i]));
                if same {
                    made[i] = *vid;
                }
                same
            });
            newer_blob = ver.blob;
        }
        let settled_all = stopped_at.is_none();
        let open: std::collections::HashSet<usize> = open.into_iter().collect();
        // each version's record, looked up once
        let by_id: std::collections::HashMap<VersionId, &BlobVersion> =
            versions.iter().map(|(id, b)| (*id, b)).collect();
        let mut rows = Vec::with_capacity(view.nodes.len());
        for &i in &walk.order {
            let v = made[i];
            let b = match by_id.get(&v) {
                Some(b) => (*b).clone(),
                None => t.ver.clone(),
            };
            let settled = settled_all || !open.contains(&i);
            // a row that ends on an outside save says so (changed in it, or renumbered by it)
            let outside_save = settled
                && is_outside_save(&b)
                && !plant_on!(
                    crate::differential::tier0_verb_mutant()
                        == Some(crate::differential::Tier0VerbMutant::BlameLabelMissing)
                );
            rows.push(NodeBlame {
                key: view.nodes[i].key.clone(),
                version: v,
                author: b.author,
                author_name: b.author_name,
                at_ms: b.at_ms,
                via: b.via,
                on_behalf_of: b.on_behalf_of,
                settled,
                verb: b.verb,
                outside_save,
            });
        }
        Ok(IndexBlame {
            version: t.version,
            rows,
            stopped_at,
            missing,
        })
    }

    /// **What a roster shows** for nodes people are on (their view region,
    /// [`BlobSession::set_region`], is a node key): each node's name, kind and whether it is
    /// shown, in the current version; `None` for a key that names no node now. Needs read.
    /// The roster itself stays a plain string: the host joins the two.
    pub fn node_labels(&self, conn: ConnId, keys: &[NodeKey]) -> Result<Vec<Option<NodeLabel>>> {
        let out = self.read_view(conn, None).and_then(|view| {
            if keys.len() > INDEX_ADDRS_MAX {
                return Err(SessionError::BadPosition(format!(
                    "{} keys in one call; at most {INDEX_ADDRS_MAX}",
                    keys.len()
                )));
            }
            let walk = Walk::new(&view);
            Ok(keys
                .iter()
                .map(|k| {
                    walk.index_of(k.as_str()).map(|i| {
                        let n = &view.nodes[i];
                        NodeLabel {
                            key: n.key.clone(),
                            name: n.name.clone(),
                            kind: n.kind.clone(),
                            visible: n.visible_in_tree,
                        }
                    })
                })
                .collect())
        });
        self.for_viewer(conn, out, |_, v| v)
    }
}

/// At most [`INDEX_ADDRS_MAX`] addresses, and at least one.
pub(super) fn check_addrs(addrs: &[String]) -> Result<()> {
    if addrs.is_empty() {
        return Err(SessionError::BadPosition(
            "name at least one node (a key, an ID, #N or a name)".into(),
        ));
    }
    if addrs.len() > INDEX_ADDRS_MAX {
        return Err(SessionError::BadPosition(format!(
            "{} addresses in one read; at most {INDEX_ADDRS_MAX}",
            addrs.len()
        )));
    }
    Ok(())
}

/// [`Walk::nodes`], its refusals as session errors.
pub(super) fn nodes_of(walk: &Walk<'_>, addrs: &[String], depth: u16) -> Result<NodesRead> {
    walk.nodes(addrs, depth).map_err(read_err)
}

/// A read's refusal as a session error: an address that doesn't parse is
/// [`SessionError::BadPosition`], one with its own id [`SessionError::IndexRead`].
fn read_err(e: ReadRefusal) -> SessionError {
    match e {
        ReadRefusal::Parse(e) => SessionError::BadPosition(e.to_string()),
        ReadRefusal::Report(r) => SessionError::IndexRead(r),
    }
}

/// The keys present on both sides of a save, of those present before it.
fn keys_kept(before: &IndexView, after: &IndexView) -> (u32, u32) {
    let now: std::collections::HashSet<&str> = after.nodes.iter().map(|n| n.key.as_str()).collect();
    let kept = before
        .nodes
        .iter()
        .filter(|n| now.contains(n.key.as_str()))
        .count();
    (
        u32::try_from(kept).unwrap_or(u32::MAX),
        u32::try_from(before.nodes.len()).unwrap_or(u32::MAX),
    )
}

/// The node holding `key` in `v`, if any.
pub(super) fn node_of<'a>(v: &'a IndexView, key: &NodeKey) -> Option<&'a IndexNode> {
    v.nodes.iter().find(|n| &n.key == key)
}

/// [`BlobPack::judge_across_save`] as a word: `kind_changed` or `identity_doubt` when in doubt.
pub(super) fn judged_across(
    pack: &dyn BlobPack,
    was: &Fingerprint,
    now: &IndexNode,
) -> Option<&'static str> {
    if plant_on!(
        crate::differential::tier0_verb_mutant()
            == Some(crate::differential::Tier0VerbMutant::ReuseReadsAttached)
    ) {
        return None;
    }
    match pack.judge_across_save(was, None, now) {
        AnchorJudgment::KindChanged => Some("kind_changed"),
        AnchorJudgment::IdentityDoubt => Some("identity_doubt"),
        _ => None,
    }
}
