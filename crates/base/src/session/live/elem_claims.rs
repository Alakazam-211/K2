//! Claims on elements of a list root, on a keyed doc that isn't a table. A child of `live`, so
//! it works on the session's own state.
//!
//! - **A claim names keys.** It covers those elements of one MovableList root and no others: it
//!   follows moves (keys never change) and never grows, so an element inserted beside a claimed
//!   one stays outside it.
//! - **A map key counts as its element.** A write to a key of a Map root the pack declares
//!   [composite](Pack::composite) touches the element the key starts with.
//! - **Touching a claim:** a list op naming a claimed key, or a map op whose key maps to one. An
//!   op that only anchors on a claimed key (an insert or a move placed after it) doesn't touch
//!   it. Under [`ClaimPolicy::Respect`] an agent's change touching another's claim is refused
//!   (exit 5) before anything applies. An op of the claimant's own that lands ends it
//!   ([`ClaimEnd::Edited`]) unless it is held; a change that lands nothing, and an op with no
//!   effect in one that does, leave it as it was.
//! - **Deleted elements stay named.** A restore brings one back under the claim. The claim ends
//!   ([`ClaimEnd::Deleted`]) when none of its elements is live.
//! - **A table keeps its own claims.** Rows, columns and cells are spans in the current order
//!   (`grid_presence`), and [`Session::claim_elems`] refuses a table, so the two kinds never
//!   have to overlap each other.
//!
//! Everything is memory only, as for text (`session::awareness`).

use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::elem::{Applied as ElemApplied, Effect, ElemState, Key};
use crate::session::awareness::{
    Claim, ClaimEnd, ClaimInfo, ElemSpan, Reservation, ReservationEnd, ReservationInfo,
    CLAIM_KEYS_MAX,
};
use crate::session::error::ElemsClaim;
use crate::session::pack::elem_of_map_key;

/// Elements by list root: what a change is about.
pub(super) type Touched = BTreeMap<String, BTreeSet<Key>>;

/// Adds a map op's element, through the pack's composite roots.
fn add_map_key(
    t: &mut Touched,
    composites: &[(&'static str, &'static str)],
    root: &str,
    key: &str,
) {
    if planted_map_keys_unmapped() {
        return;
    }
    if let Some((list, k)) = elem_of_map_key(composites, root, key) {
        t.entry(list.to_owned()).or_default().insert(k);
    }
}

/// The elements a change names, before it applies: its list ops' keys, and its map ops' keys
/// mapped through the pack's composite roots. What the claim policy judges.
fn touched_elems(composites: &[(&'static str, &'static str)], edits: &[Edit]) -> Touched {
    let mut out = Touched::new();
    for e in edits {
        match e {
            Edit::List { root, ops } => {
                out.entry(root.clone())
                    .or_default()
                    .extend(ops.iter().map(|o| o.key().clone()));
            }
            Edit::Map { root, ops } => {
                for o in ops {
                    add_map_key(&mut out, composites, root, o.key());
                }
            }
            _ => {}
        }
    }
    out
}

/// The elements a change that applied really touched, from its applied stream: an op with no
/// effect (a restore of a live element, a delete of an absent key) touches nothing. What ends a
/// claim.
fn landed_elems(composites: &[(&'static str, &'static str)], stream: &[ElemApplied]) -> Touched {
    let landed = |e: &Effect| !matches!(e, Effect::None { .. });
    let mut out = Touched::new();
    for a in stream {
        match a {
            ElemApplied::List { root, ops } => {
                for o in ops.iter().filter(|o| landed(&o.effect)) {
                    out.entry(root.clone()).or_default().insert(o.key.clone());
                }
            }
            ElemApplied::Map { root, ops } => {
                for o in ops.iter().filter(|o| landed(&o.effect)) {
                    add_map_key(&mut out, composites, root, &o.key);
                }
            }
            _ => {}
        }
    }
    out
}

fn is_touched(t: &Touched, s: &ElemSpan) -> bool {
    t.get(s.root.as_str())
        .is_some_and(|k| !k.is_disjoint(&s.keys))
}

fn refusal(
    s: &ElemSpan,
    id: awareness::ClaimId,
    by: &str,
    name: &str,
    remaining_ms: u64,
    note: Option<String>,
) -> SessionError {
    SessionError::ElemsClaimed(Box::new(ElemsClaim {
        id,
        by: by.to_owned(),
        name: name.to_owned(),
        root: s.root.clone(),
        keys: s.keys.iter().cloned().collect(),
        remaining_ms,
        note,
    }))
}

impl Session {
    /// Claim elements of the MovableList root `root` by key for `ttl_ms`
    /// ([`Config::claim_default_ms`] when `None`, at most [`Config::claim_max_ms`]; `Some(0)` is
    /// refused). Needs edit. Advisory, as [`Session::claim`]: it refuses nobody's edit unless the
    /// host asks for that on the edit ([`ClaimPolicy::Respect`]), and never a person's.
    ///
    /// It covers those elements and, through [`Pack::composite`], the map keys that belong to
    /// them. It ends on the claimant's own edit touching it (unless `hold`), on release, time or
    /// leave, and with [`ClaimEnd::Deleted`] when none of its elements is live.
    ///
    /// Errors: another participant's live claim on one of the keys gives
    /// [`SessionError::ElemsClaimed`] (exit 5); the caller's own overlapping claim that others
    /// wait for gives [`SessionError::ClaimContended`]; a key that isn't a live element, or a
    /// root that isn't a list, gives [`SessionError::BadPosition`]; no keys, more than
    /// [`CLAIM_KEYS_MAX`], a note over [`awareness::CLAIM_NOTE_MAX`] bytes, a table doc (claim its rows,
    /// columns or cells with [`Session::claim_span`]) or the limits ([`Config::claims_max`],
    /// [`Config::claims_per_participant`]) give [`SessionError::BadClaim`]. `hold` is bounded by
    /// the claim's time, as any claim's: `ttl_ms` is clamped to [`Config::claim_max_ms`].
    pub fn claim_elems(
        &self,
        conn: ConnId,
        root: &str,
        keys: &[Key],
        ttl_ms: Option<u64>,
        note: Option<String>,
        hold: bool,
    ) -> Result<ClaimInfo> {
        let out = self.claim_elems_inner(conn, root, keys, ttl_ms, note, hold);
        // an App viewer (K5) sees the others by handle
        self.for_viewer(conn, out, |l, c| l.claim(c))
    }

    /// [`Session::claim_elems`] as the caller's own pids see it.
    pub(super) fn claim_elems_inner(
        &self,
        conn: ConnId,
        root: &str,
        keys: &[Key],
        ttl_ms: Option<u64>,
        note: Option<String>,
        hold: bool,
    ) -> Result<ClaimInfo> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Edit, Action::Claim, now)?;
        self.expire_claims(st, now);
        let ttl = self.claim_ttl(ttl_ms.unwrap_or(self.config.claim_default_ms))?;
        if self.is_grid() {
            return Err(SessionError::BadClaim(format!(
                "{root}: a table's claims are rows, columns or cells, not elements"
            )));
        }
        if self.schema.kind_of(root) != Some(Kind::MovableList) {
            return Err(SessionError::BadPosition(format!(
                "{root} is not a list root: only a list's elements can be claimed"
            )));
        }
        if keys.is_empty() {
            return Err(SessionError::BadClaim(
                "a claim on elements names at least one".into(),
            ));
        }
        if keys.len() > CLAIM_KEYS_MAX {
            return Err(SessionError::BadClaim(format!(
                "{} elements in one claim; at most {CLAIM_KEYS_MAX}",
                keys.len()
            )));
        }
        Self::check_claim_note(note.as_deref())?;
        for k in keys {
            if !matches!(st.doc.elem(root, k)?, ElemState::Live(_)) {
                return Err(SessionError::BadPosition(format!(
                    "{root} {k} is not live (unknown or deleted)"
                )));
            }
        }
        let span = ElemSpan {
            root: root.to_owned(),
            keys: keys.iter().cloned().collect(),
        };
        let pid = st.members[i].p.pid.clone();
        let overlaps = |other: &ElemSpan| span.overlaps(other);
        // a reservation handed to someone else binds agents only
        if st.members[i].p.kind == ParticipantKind::Agent {
            if let Some((r, s)) = st.aware.reservations.iter().find_map(|r| {
                let s = r.elems.as_ref().filter(|s| r.pid != pid && overlaps(s))?;
                Some((r, s))
            }) {
                return Err(refusal(s, r.claim, &r.pid, &r.name, r.remaining(now), None));
            }
        }
        for c in &st.aware.claims {
            let Some(other) = c.elems.as_ref().filter(|o| overlaps(o)) else {
                continue;
            };
            if c.pid != pid {
                return Err(refusal(
                    other,
                    c.id,
                    &c.pid,
                    &c.name,
                    c.remaining(now),
                    c.note.clone(),
                ));
            }
            if c.contended() {
                return Err(SessionError::ClaimContended {
                    id: c.id,
                    waiters: c.waiters.len(),
                    remaining_ms: c.remaining(now),
                });
            }
        }
        let cfg = &self.config;
        if st.aware.claims.len() >= cfg.claims_max {
            return Err(SessionError::BadClaim(format!(
                "this doc has {} live claims (the most)",
                st.aware.claims.len()
            )));
        }
        let mine = st.aware.claims.iter().filter(|c| c.pid == pid).count();
        if mine >= cfg.claims_per_participant {
            return Err(SessionError::BadClaim(format!(
                "you hold {mine} claims here (the most); release one"
            )));
        }
        // the caller's own reservation there is used: the rest of its queue waits for this
        let mut waiters = std::collections::VecDeque::new();
        if let Some(k) = st
            .aware
            .reservations
            .iter()
            .position(|r| r.pid == pid && r.elems.as_ref().is_some_and(overlaps))
        {
            let r = st.aware.reservations.remove(k);
            waiters = r.queue.clone();
            waiters.retain(|(_, p)| *p != pid);
            let held = r.elems.as_ref().expect("an element reservation");
            self.emit(AuditEvent::ReservationEnded {
                key: st.key.clone(),
                claim: r.claim,
                pid: r.pid.clone(),
                how: ReservationEnd::Claimed,
                lines: Self::elem_lines(st, held),
                version: st.version,
                next_waiter: waiters.front().cloned(),
            });
        }
        let m = &st.members[i];
        let claim = Claim {
            id: awareness::ClaimId::fresh(),
            pid: pid.clone(),
            name: m.p.name.clone(),
            conn: m.conn,
            root: root.to_owned(),
            s: 0,
            e: 0,
            taken_ms: now,
            expires_ms: now.saturating_add(ttl),
            note,
            hold,
            for_edit: false,
            waiters,
            edited: false,
            grid: None,
            elems: Some(span),
        };
        let info = Self::elem_claim_info(st, &claim, now);
        st.aware.claims.push(claim);
        st.roster.record(
            RosterEvent::ClaimStartedElems {
                who: Who::of(&pid),
                claim: info.id,
                root: root.to_owned(),
                keys: match &info.span {
                    awareness::ClaimSpan::Elems { keys, .. } => keys.clone(),
                    _ => unreachable!("an element claim's span"),
                },
                note: info.note.clone(),
                hold,
                expires_ms: info.expires_ms,
                version: st.version,
            },
            self.config.roster_queue,
        );
        self.emit(AuditEvent::ClaimTaken {
            key: st.key.clone(),
            claim: info.id,
            pid,
            conn: info.conn,
            lines: info.lines.clone(),
            expires_ms: info.expires_ms,
            hold,
            for_edit: false,
        });
        self.log_activity(
            st,
            i,
            ActivityKind::Claim,
            "claim",
            Some(info.lines.clone()),
            now,
            Some(info.id),
        );
        Ok(info)
    }

    /// The list root and the 1-based live positions of the span's first and last live elements
    /// (`1, 1` with none live): the `lines` of an element claim's events, for display only.
    pub(super) fn elem_lines(st: &State, s: &ElemSpan) -> LineRange {
        let live = s.keys.iter().filter_map(|k| match st.doc.elem(&s.root, k) {
            Ok(ElemState::Live(i)) => Some(i + 1),
            _ => None,
        });
        let (first, last) = live
            .fold(None, |acc: Option<(usize, usize)>, p| match acc {
                Some((a, b)) => Some((a.min(p), b.max(p))),
                None => Some((p, p)),
            })
            .unwrap_or((1, 1));
        LineRange {
            root: s.root.clone(),
            first,
            last,
        }
    }

    pub(super) fn elem_claim_info(st: &State, c: &Claim, now: u64) -> ClaimInfo {
        let span = c.elems.as_ref().expect("an element claim");
        let mut info = c.info("", now);
        info.lines = Self::elem_lines(st, span);
        info.span = span.span();
        info.start = 0;
        info.end = 0;
        info
    }

    pub(super) fn elem_reservation_info(st: &State, r: &Reservation, now: u64) -> ReservationInfo {
        let span = r.elems.as_ref().expect("an element reservation");
        let mut info = r.info("", now);
        info.lines = Self::elem_lines(st, span);
        info.span = span.span();
        info.start = 0;
        info.end = 0;
        info
    }

    /// The claim policy for element claims, for an agent (the caller checked the policy and the
    /// kind): a change touching another participant's element claim, or an element reservation
    /// handed to someone else, is refused before anything applies.
    pub(super) fn check_elem_claims(
        &self,
        st: &State,
        i: usize,
        edits: &[Edit],
        now: u64,
    ) -> Result<()> {
        let any = st.aware.claims.iter().any(|c| c.elems.is_some())
            || st.aware.reservations.iter().any(|r| r.elems.is_some());
        if !any {
            return Ok(());
        }
        let t = touched_elems(self.pack.composite(), edits);
        let pid = &st.members[i].p.pid;
        for r in &st.aware.reservations {
            if let Some(s) = r
                .elems
                .as_ref()
                .filter(|s| r.pid != *pid && is_touched(&t, s))
            {
                return Err(refusal(s, r.claim, &r.pid, &r.name, r.remaining(now), None));
            }
        }
        for c in &st.aware.claims {
            if let Some(s) = c
                .elems
                .as_ref()
                .filter(|s| c.pid != *pid && is_touched(&t, s))
            {
                return Err(refusal(
                    s,
                    c.id,
                    &c.pid,
                    &c.name,
                    c.remaining(now),
                    c.note.clone(),
                ));
            }
        }
        Ok(())
    }

    /// The elements a change that just applied touched, for [`Session::mark_elem_claims`]; empty
    /// with no element claim to end.
    pub(super) fn elems_landed(&self, st: &State, stream: &[ElemApplied]) -> Touched {
        let any = st.aware.claims.iter().any(|c| c.elems.is_some())
            || st.aware.reservations.iter().any(|r| r.elems.is_some());
        if !any {
            return Touched::new();
        }
        landed_elems(self.pack.composite(), stream)
    }

    /// After a structured change by member `i` applied, and only then, so a change that doesn't
    /// land leaves nothing for the next one: its own element claims it touched end once it is
    /// durable, unless held, and an element reservation handed to it that it touched is used.
    pub(super) fn mark_elem_claims(&self, st: &mut State, i: usize, touched: &Touched) {
        if touched.is_empty() {
            return;
        }
        let pid = st.members[i].p.pid.clone();
        for c in &mut st.aware.claims {
            if c.pid == pid && !c.hold && c.elems.as_ref().is_some_and(|s| is_touched(touched, s)) {
                c.edited = true;
            }
        }
        for r in &mut st.aware.reservations {
            if r.pid == pid && r.elems.as_ref().is_some_and(|s| is_touched(touched, s)) {
                r.edited = true;
            }
        }
    }

    /// The planted fault [`crate::differential::ClaimMutant::MarkBeforeApply`]: marks from what
    /// the change names, before it applies. Without the `differential` feature it does nothing.
    pub(super) fn planted_early_elem_marks(&self, st: &mut State, i: usize, edits: &[Edit]) {
        if planted_mark_before_apply() {
            self.mark_elem_claims(st, i, &touched_elems(self.pack.composite(), edits));
        }
    }

    /// After a structured change (`stream`, applied and logged), by anyone or from outside: an
    /// element claim or reservation on a list root the change touched ends when none of its
    /// elements is live. Nothing passes on from a claim that ends this way.
    pub(super) fn elem_claims_after(&self, st: &mut State, stream: &[ElemApplied], now: u64) {
        let any = st.aware.claims.iter().any(|c| c.elems.is_some())
            || st.aware.reservations.iter().any(|r| r.elems.is_some());
        if !any {
            return;
        }
        let lists: BTreeSet<&str> = stream
            .iter()
            .filter_map(|a| match a {
                ElemApplied::List { root, .. } => Some(root.as_str()),
                _ => None,
            })
            .collect();
        if lists.is_empty() {
            return;
        }
        let doc = &st.doc;
        let gone = |s: &ElemSpan| {
            lists.contains(s.root.as_str())
                && !s
                    .keys
                    .iter()
                    .any(|k| matches!(doc.elem(&s.root, k), Ok(ElemState::Live(_))))
        };
        let claims: Vec<awareness::ClaimId> = st
            .aware
            .claims
            .iter()
            .filter(|c| c.elems.as_ref().is_some_and(gone))
            .map(|c| c.id)
            .collect();
        let reservations: Vec<awareness::ClaimId> = st
            .aware
            .reservations
            .iter()
            .filter(|r| r.elems.as_ref().is_some_and(gone))
            .map(|r| r.claim)
            .collect();
        for id in claims {
            if let Some(k) = st.aware.claims.iter().position(|c| c.id == id) {
                self.end_claim(st, k, ClaimEnd::Deleted, now, false);
            }
        }
        for id in reservations {
            if let Some(k) = st.aware.reservations.iter().position(|r| r.claim == id) {
                self.end_reservation(st, k, ReservationEnd::Deleted, now);
            }
        }
    }
}

/// The planted fault [`crate::differential::ClaimMutant::MapKeysUnmapped`]: map writes aren't
/// mapped to their elements. Without the `differential` feature there is no hook: `false`.
#[cfg(any(test, feature = "differential"))]
fn planted_map_keys_unmapped() -> bool {
    crate::differential::claim_mutant() == Some(crate::differential::ClaimMutant::MapKeysUnmapped)
}

#[cfg(not(any(test, feature = "differential")))]
fn planted_map_keys_unmapped() -> bool {
    false
}

#[cfg(any(test, feature = "differential"))]
fn planted_mark_before_apply() -> bool {
    crate::differential::claim_mutant() == Some(crate::differential::ClaimMutant::MarkBeforeApply)
}

#[cfg(not(any(test, feature = "differential")))]
fn planted_mark_before_apply() -> bool {
    false
}
