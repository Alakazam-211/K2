//! Claims by node (slice A5, `docs/SESSIONS.md`, "Claims on nodes"): an agent or a person
//! says which nodes of a blob doc's structure index they are working on. The CSV claim model,
//! cut to what a whole-file doc needs:
//!
//! - **Advisory.** A claim never refuses a write, a restore or a lease: the lease guards the
//!   file, and a claim only tells the others (the roster, `claims`, the events). Nothing in
//!   the write path reads claims.
//! - **By key.** A claim names node keys, which carry across versions; it never grows. Two
//!   participants can't claim the same node: the second is refused [`SessionError::
//!   NodesClaimed`] (exit 5, as any claim). The same participant may hold several.
//! - **Memory only**, as every claim: gone when the doc unloads. Limits as for text and grids
//!   ([`Config::claims_max`], [`Config::claims_per_participant`],
//!   [`crate::session::CLAIM_KEYS_MAX`] nodes, [`crate::session::CLAIM_NOTE_MAX`] note bytes).
//! - **Ends** on release, expiry, the claiming connection leaving, and
//!   [`ClaimEnd::Deleted`] once a version's index holds none of its nodes. Never on an edit:
//!   a whole-file save can't say a claimant was done.

use std::collections::BTreeSet;

use super::*;
use crate::index::read::Walk;
use crate::index::NodeKey;
use crate::session::awareness::{
    ClaimEnd, ClaimId, ClaimInfo, ClaimSpan, CLAIM_KEYS_MAX, CLAIM_NOTE_MAX,
};
use crate::session::position::LineRange;
use crate::session::NodesClaim;

/// The root a node claim's display lines name ([`ClaimInfo::lines`]).
pub const NODE_CLAIM_ROOT: &str = "index";

/// One live claim on nodes.
#[derive(Debug, Clone)]
pub(super) struct NodeClaim {
    id: ClaimId,
    pid: String,
    name: String,
    conn: ConnId,
    keys: BTreeSet<NodeKey>,
    taken_ms: u64,
    expires_ms: u64,
    note: Option<String>,
    /// The version whose index last held its nodes.
    seen: VersionId,
    /// `#N` of its first and last node there: what [`ClaimInfo::lines`] shows.
    shown: (usize, usize),
}

impl NodeClaim {
    fn info(&self, now: u64) -> ClaimInfo {
        ClaimInfo {
            id: self.id,
            pid: self.pid.clone(),
            name: self.name.clone(),
            conn: self.conn,
            lines: self.lines(),
            span: ClaimSpan::Nodes {
                keys: self.keys.iter().cloned().collect(),
            },
            start: 0,
            end: 0,
            taken_ms: self.taken_ms,
            expires_ms: self.expires_ms,
            remaining_ms: self.expires_ms.saturating_sub(now),
            note: self.note.clone(),
            // nothing but release, time, leave or deletion ends it
            hold: true,
            for_edit: false,
            waiters: Vec::new(),
        }
    }

    fn lines(&self) -> LineRange {
        LineRange {
            root: NODE_CLAIM_ROOT.to_owned(),
            first: self.shown.0,
            last: self.shown.1,
        }
    }
}

/// `#N` of the first and last of `keys` in `walk` (`1, 1` when none is there).
fn shown(walk: &Walk<'_>, keys: &BTreeSet<NodeKey>) -> (usize, usize) {
    let pos: Vec<usize> = keys
        .iter()
        .filter_map(|k| walk.index_of(k.as_str()))
        .map(|i| walk.pos[i] as usize)
        .collect();
    match (pos.iter().min(), pos.iter().max()) {
        (Some(a), Some(b)) => (*a, *b),
        _ => (1, 1),
    }
}

impl BlobSession {
    fn claim_ttl(&self, ttl_ms: Option<u64>) -> Result<u64> {
        let ttl = ttl_ms.unwrap_or(self.config.claim_default_ms);
        if ttl == 0 {
            return Err(SessionError::BadClaim(
                "a claim needs a TTL above 0 ms".into(),
            ));
        }
        Ok(ttl.min(self.config.claim_max_ms.max(1)))
    }

    fn end_node_claim(&self, st: &mut BState, k: usize, how: ClaimEnd) {
        let c = st.node_claims.remove(k);
        self.emit(AuditEvent::ClaimEnded {
            key: st.key.clone(),
            claim: c.id,
            pid: c.pid.clone(),
            how,
            lines: c.lines(),
            version: st.vid,
            next_waiter: None,
            waiters: Vec::new(),
        });
    }

    /// End the claims taken through `conn` (it left, was kicked, idled out or lost read).
    pub(super) fn end_node_claims_of(&self, st: &mut BState, conn: ConnId) {
        while let Some(k) = st.node_claims.iter().position(|c| c.conn == conn) {
            self.end_node_claim(st, k, ClaimEnd::Left);
        }
    }

    /// End claims that ran out. Cheap, under the session lock; the check against a newer
    /// version's index is [`BlobSession::refresh_node_claims`], off it.
    pub(super) fn sweep_node_claims(&self, st: &mut BState, now: u64) {
        while let Some(k) = st.node_claims.iter().position(|c| now >= c.expires_ms) {
            self.end_node_claim(st, k, ClaimEnd::Expired);
        }
        #[cfg(any(test, feature = "differential"))]
        if crate::differential::index_mutant()
            == Some(crate::differential::IndexMutant::ClaimsLoadUnderLock)
        {
            // the planted fault: the index decoded here, with the lock held, on every call
            let Some(spec) = self.pack.index_spec() else {
                return;
            };
            let Ok(t) = self.target_of(st, None) else {
                return;
            };
            if let Ok(view) = self.load_view(&t, &spec) {
                self.apply_view(st, &Walk::new(&view), t.version);
            }
        }
    }

    /// Check the node claims against the current version's index, when some haven't been
    /// (`AuditEvent::ClaimEnded` with [`ClaimEnd::Deleted`] for those whose nodes are all gone
    /// from it). The index is decoded **off the session lock**, at most once per version: the
    /// lock is taken to see whether there is anything to check, released while the index loads,
    /// and taken again to apply it, re-checking each claim. An index that isn't ready, or can't
    /// be read now, is looked at again on a later call. Called before the roster, `claims` and
    /// `tick`.
    pub(super) fn refresh_node_claims(&self) {
        let Some(spec) = self.pack.index_spec() else {
            return;
        };
        let t = {
            let Ok(st) = self.lock() else {
                return;
            };
            if st.node_claims.iter().all(|c| c.seen >= st.vid) {
                return;
            }
            let Ok(t) = self.target_of(&st, None) else {
                return;
            };
            t
        };
        // not ready: nothing to decode, and nothing to decide yet
        if !matches!(
            self.status_of(&t, &spec),
            Ok(crate::index::IndexStatus::Ready { .. })
        ) {
            return;
        }
        let Ok(view) = self.load_view(&t, &spec) else {
            return;
        };
        let walk = Walk::new(&view);
        let Ok(mut st) = self.lock() else {
            return;
        };
        self.apply_view(&mut st, &walk, t.version);
    }

    /// Apply version `vid`'s index to the claims not yet checked against it (or a later one):
    /// carried, or ended as deleted.
    fn apply_view(&self, st: &mut BState, walk: &Walk<'_>, vid: VersionId) {
        let mut k = 0;
        while k < st.node_claims.len() {
            let c = &mut st.node_claims[k];
            if c.seen >= vid {
                k += 1;
                continue;
            }
            let any = c
                .keys
                .iter()
                .any(|key| walk.index_of(key.as_str()).is_some());
            #[cfg(any(test, feature = "differential"))]
            let any = any
                || crate::differential::index_mutant()
                    == Some(crate::differential::IndexMutant::ClaimOutlivesNodes);
            if any {
                c.seen = vid;
                c.shown = shown(walk, &c.keys);
                k += 1;
            } else {
                self.end_node_claim(st, k, ClaimEnd::Deleted);
            }
        }
    }

    /// Whether someone other than `pid` holds a node claim (the planted fault that would
    /// make a claim block a write; nothing in the write path asks this).
    #[cfg(any(test, feature = "differential"))]
    pub(super) fn claimed_by_other(st: &BState, pid: &str) -> bool {
        st.node_claims.iter().any(|c| c.pid != pid)
    }

    /// The live node claims, as `conn` may see them (for the roster).
    pub(super) fn node_claims_of(st: &BState, pid: &str, now: u64) -> Vec<ClaimInfo> {
        st.node_claims
            .iter()
            .filter(|c| c.pid == pid)
            .map(|c| c.info(now))
            .collect()
    }

    /// **Claim nodes** of the structure index by key (A5) for `ttl_ms` ([`Config::claim_default_ms`]
    /// when `None`, at most [`Config::claim_max_ms`]; `Some(0)` is refused), with an optional
    /// note. Needs edit, as any claim. Every key must name a node of the current version's
    /// index (else [`SessionError::BadPosition`]; no index yet: [`SessionError::IndexPending`]).
    ///
    /// **Advisory:** it refuses nobody's write; the lease guards the file. Another
    /// participant's live claim naming one of the same nodes refuses this one
    /// ([`SessionError::NodesClaimed`], exit 5). No keys, more than
    /// [`crate::session::CLAIM_KEYS_MAX`], a note over [`crate::session::CLAIM_NOTE_MAX`]
    /// bytes, or the limits ([`Config::claims_max`], [`Config::claims_per_participant`]):
    /// [`SessionError::BadClaim`]. It ends on release, expiry, its connection leaving, and
    /// once a version's index holds none of its nodes ([`ClaimEnd::Deleted`]).
    pub fn claim_nodes(
        &self,
        conn: ConnId,
        keys: &[NodeKey],
        ttl_ms: Option<u64>,
        note: Option<String>,
    ) -> Result<ClaimInfo> {
        let out = self.claim_nodes_inner(conn, keys, ttl_ms, note);
        self.for_viewer(conn, out, |l, c| l.claim(c))
    }

    fn claim_nodes_inner(
        &self,
        conn: ConnId,
        keys: &[NodeKey],
        ttl_ms: Option<u64>,
        note: Option<String>,
    ) -> Result<ClaimInfo> {
        // what needs no index, then the index, off the session lock
        let (spec, t) = {
            let now = self.now();
            let mut st = self.lock()?;
            let st = &mut *st;
            self.admit(st, conn, Need::Edit, Action::Claim, now)?;
            self.claim_shape(keys, ttl_ms, note.as_deref())?;
            let spec = self
                .pack
                .index_spec()
                .ok_or(SessionError::IndexUnsupported)?;
            (spec, self.target_of(st, None)?)
        };
        let view = self.load_view(&t, &spec)?;
        let walk = Walk::new(&view);
        for k in keys {
            if walk.index_of(k.as_str()).is_none() {
                return Err(SessionError::BadPosition(format!(
                    "{k} is not a node of version {}",
                    t.version
                )));
            }
        }
        // the lock again: the connection, the others' claims and the limits as they are now
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Edit, Action::Claim, now)?;
        self.sweep_node_claims(st, now);
        let ttl = self.claim_ttl(ttl_ms)?;
        let keys: BTreeSet<NodeKey> = keys.iter().cloned().collect();
        let pid = st.members[i].p.pid.clone();
        #[cfg(any(test, feature = "differential"))]
        let refuse = crate::differential::index_mutant()
            != Some(crate::differential::IndexMutant::ClaimOverlapAllowed);
        #[cfg(not(any(test, feature = "differential")))]
        let refuse = true;
        if let Some(c) = st
            .node_claims
            .iter()
            .find(|c| refuse && c.pid != pid && !c.keys.is_disjoint(&keys))
        {
            return Err(SessionError::NodesClaimed(Box::new(NodesClaim {
                id: c.id,
                by: c.pid.clone(),
                name: c.name.clone(),
                keys: c.keys.intersection(&keys).cloned().collect(),
                remaining_ms: c.expires_ms.saturating_sub(now),
                note: c.note.clone(),
            })));
        }
        let cfg = &self.config;
        if st.node_claims.len() >= cfg.claims_max {
            return Err(SessionError::BadClaim(format!(
                "this doc has {} live claims (the most)",
                st.node_claims.len()
            )));
        }
        let mine = st.node_claims.iter().filter(|c| c.pid == pid).count();
        if mine >= cfg.claims_per_participant {
            return Err(SessionError::BadClaim(format!(
                "you hold {mine} claims here (the most); release one"
            )));
        }
        let c = NodeClaim {
            id: ClaimId::fresh(),
            pid,
            name: st.members[i].p.name.clone(),
            conn,
            shown: shown(&walk, &keys),
            keys,
            taken_ms: now,
            expires_ms: now.saturating_add(ttl),
            note,
            seen: st.vid,
        };
        self.emit(AuditEvent::ClaimTaken {
            key: st.key.clone(),
            claim: c.id,
            pid: c.pid.clone(),
            conn,
            lines: c.lines(),
            expires_ms: c.expires_ms,
            hold: true,
            for_edit: false,
        });
        let info = c.info(now);
        st.node_claims.push(c);
        Ok(info)
    }

    /// What a claim may be, before any index is read: a TTL above 0, one to
    /// [`CLAIM_KEYS_MAX`] keys, a note of at most [`CLAIM_NOTE_MAX`] bytes.
    fn claim_shape(&self, keys: &[NodeKey], ttl_ms: Option<u64>, note: Option<&str>) -> Result<()> {
        self.claim_ttl(ttl_ms)?;
        if keys.is_empty() {
            return Err(SessionError::BadClaim(
                "a claim on nodes names at least one".into(),
            ));
        }
        if keys.len() > CLAIM_KEYS_MAX {
            return Err(SessionError::BadClaim(format!(
                "{} nodes in one claim; at most {CLAIM_KEYS_MAX}",
                keys.len()
            )));
        }
        if let Some(n) = note.filter(|n| n.len() > CLAIM_NOTE_MAX) {
            return Err(SessionError::BadClaim(format!(
                "a note of {} bytes; at most {CLAIM_NOTE_MAX}",
                n.len()
            )));
        }
        Ok(())
    }

    /// The caller's pid's node claim `id`, or its only one.
    fn own_node_claim(st: &BState, pid: &str, id: Option<ClaimId>) -> Result<usize> {
        match id {
            Some(id) => {
                let k = st
                    .node_claims
                    .iter()
                    .position(|c| c.id == id)
                    .ok_or(SessionError::NoClaim(Some(id)))?;
                if st.node_claims[k].pid != pid {
                    // no pid in the message: an App viewer sees others only by handle, and a
                    // handle can't be put into a string after the fact
                    return Err(SessionError::BadClaim(format!(
                        "{id} is another participant's claim, not yours"
                    )));
                }
                Ok(k)
            }
            None => {
                let mine: Vec<usize> = (0..st.node_claims.len())
                    .filter(|k| st.node_claims[*k].pid == pid)
                    .collect();
                match mine.as_slice() {
                    [] => Err(SessionError::NoClaim(None)),
                    [k] => Ok(*k),
                    _ => Err(SessionError::BadClaim(format!(
                        "you hold {} claims here; name one",
                        mine.len()
                    ))),
                }
            }
        }
    }

    /// Extend a node claim of the caller's pid to `now + ttl_ms` (as
    /// [`BlobSession::claim_nodes`]). `id: None` is the caller's only claim here. Needs edit.
    /// [`SessionError::NoClaim`] (exit 4) when there is none; someone else's, or several and
    /// no id: [`SessionError::BadClaim`] (exit 2).
    pub fn renew_claim(
        &self,
        conn: ConnId,
        id: Option<ClaimId>,
        ttl_ms: Option<u64>,
    ) -> Result<ClaimInfo> {
        let out = self.renew_claim_inner(conn, id, ttl_ms);
        self.for_viewer(conn, out, |l, c| l.claim(c))
    }

    fn renew_claim_inner(
        &self,
        conn: ConnId,
        id: Option<ClaimId>,
        ttl_ms: Option<u64>,
    ) -> Result<ClaimInfo> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Edit, Action::Claim, now)?;
        self.sweep_node_claims(st, now);
        let pid = st.members[i].p.pid.clone();
        let k = Self::own_node_claim(st, &pid, id)?;
        let ttl = self.claim_ttl(ttl_ms)?;
        let c = &mut st.node_claims[k];
        c.expires_ms = now.saturating_add(ttl);
        Ok(c.info(now))
    }

    /// Release the caller's node claim `id`, or (`None`) every claim of the caller's pid here.
    /// Returns those released (empty when none was: the `claim --release` verb exits 1). Needs
    /// read (someone who lost edit can still let go).
    pub fn release_claim(&self, conn: ConnId, id: Option<ClaimId>) -> Result<Vec<ClaimId>> {
        let out = self.release_claim_inner(conn, id);
        self.for_viewer(conn, out, |_, v| v)
    }

    fn release_claim_inner(&self, conn: ConnId, id: Option<ClaimId>) -> Result<Vec<ClaimId>> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Read, Action::Claim, now)?;
        self.sweep_node_claims(st, now);
        let pid = st.members[i].p.pid.clone();
        let mut out = Vec::new();
        while let Some(k) = st
            .node_claims
            .iter()
            .position(|c| c.pid == pid && id.map_or(true, |x| x == c.id))
        {
            out.push(st.node_claims[k].id);
            self.end_node_claim(st, k, ClaimEnd::Released);
        }
        Ok(out)
    }

    /// Every live node claim on the doc, oldest first. Needs read.
    pub fn claims(&self, conn: ConnId) -> Result<Vec<ClaimInfo>> {
        self.refresh_node_claims();
        let out = self.claims_inner(conn);
        self.for_viewer(conn, out, |l, v| {
            v.into_iter().map(|c| l.claim(c)).collect()
        })
    }

    fn claims_inner(&self, conn: ConnId) -> Result<Vec<ClaimInfo>> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        self.admit(st, conn, Need::Read, Action::Claim, now)?;
        self.sweep_node_claims(st, now);
        Ok(st.node_claims.iter().map(|c| c.info(now)).collect())
    }
}
